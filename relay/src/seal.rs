//! Relay tokens: a device's push token, sealed with a key only the relay holds (XChaCha20-Poly1305).
//!
//! The relay keeps nothing: the app registers its Apple or Google token and gets back a relay token, which it
//! gives to its bridge (or to the guichet); each push brings the relay token back and the relay opens it. Several
//! keys may be set: the first seals, all open, so a key can be replaced without cutting anyone off.
//!
//! A token (`r2.`) also holds its own id, when it was sealed and when it ends (90 days later), and who may send
//! with it (`aud`: a bridge, or the guichet, which proves itself with its key). An ended token, or one of the first
//! format (`r1.`, no end), reads as a dead device: the sender forgets it and the app registers again (it takes a
//! new token every 30 days on its own). An id can be refused for good (`RELAY_BLOCKED`).
//!
//! Transition: until `RELAY_R1_UNTIL` (2026-12-15 by default) the first format is still taken and still sealed for
//! the apps that ask without an audience (released before 2026-10-28), so older apps, bridges and relays keep
//! working together; each use is logged and `/health` warns. After that date, as above.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "r2.";
/// The first format (no end, no audience): refused, the app registers again.
pub const RETIRED_PREFIX: &str = "r1.";
const AAD: &[u8] = b"bipagents-relay-token-v2";
/// The first format's: still opened until the transition's end (`RELAY_R1_UNTIL`).
const RETIRED_AAD: &[u8] = b"bipagents-relay-token-v1";
const NONCE_LEN: usize = 24;
/// How long a relay token lasts; the apps take a new one after 30 days.
pub const LIFETIME_SECS: u64 = 90 * 24 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Apns,
    Fcm,
}

/// Who may send with a token: any bridge the app gave it to, or the guichet alone (with its key).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Audience {
    #[default]
    Bridge,
    Guichet,
}

/// The device a relay token is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    #[serde(rename = "p")]
    pub platform: Platform,
    /// `sandbox` or `production` for Apple, `fcm` for Google.
    #[serde(rename = "e")]
    pub environment: String,
    #[serde(rename = "t")]
    pub token: String,
}

/// What a relay token holds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    #[serde(flatten)]
    pub device: Device,
    /// 16 hex characters: what the logs show, and what `RELAY_BLOCKED` may name.
    #[serde(rename = "i")]
    pub id: String,
    #[serde(rename = "a")]
    pub aud: Audience,
    /// Sealed at, ends at (Unix seconds).
    #[serde(rename = "n")]
    pub iat: u64,
    #[serde(rename = "x")]
    pub exp: u64,
}

/// Why a relay token does not open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    /// Not sealed by one of these keys, or altered.
    Invalid,
    /// The first format, without an end.
    Retired,
    Expired,
}

/// Unix seconds.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub struct Sealer {
    keys: Vec<XChaCha20Poly1305>,
}

impl Sealer {
    pub fn new(keys: &[[u8; 32]]) -> Result<Self, String> {
        if keys.is_empty() {
            return Err("at least one sealing key is needed".into());
        }
        Ok(Self { keys: keys.iter().map(|k| XChaCha20Poly1305::new(k.into())).collect() })
    }

    /// Keys as base64 (standard), separated by commas: the first one seals.
    pub fn from_base64_list(list: &str) -> Result<Self, String> {
        let mut keys = Vec::new();
        for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let bytes = STANDARD.decode(item).map_err(|_| "a sealing key is not valid base64".to_string())?;
            let key: [u8; 32] = bytes.try_into().map_err(|_| "a sealing key must be 32 bytes".to_string())?;
            keys.push(key);
        }
        Self::new(&keys)
    }

    pub fn generate_key() -> String {
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        STANDARD.encode(key)
    }

    /// A new token for this device and audience, sealed at `now`, ending `LIFETIME_SECS` later.
    pub fn seal(&self, device: &Device, aud: Audience, now: u64) -> String {
        let mut id = [0u8; 8];
        rand::rngs::OsRng.fill_bytes(&mut id);
        let id = id.iter().map(|b| format!("{b:02x}")).collect();
        self.seal_claims(&Claims { device: device.clone(), id, aud, iat: now, exp: now + LIFETIME_SECS })
    }

    pub fn seal_claims(&self, claims: &Claims) -> String {
        let plain = serde_json::to_vec(claims).expect("claims serialize");
        format!("{PREFIX}{}", self.sealed(&plain, AAD))
    }

    /// A token of the first format (`r1.`: the device only, no end, no audience), as the apps and bridges released
    /// before 2026-10-28 take them. Sealed only during the transition (`RELAY_R1_UNTIL`), for an app that does not
    /// say who the token is for (an older app), or as the fallback for a bridge that takes nothing else.
    pub fn seal_retired(&self, device: &Device) -> String {
        let plain = serde_json::to_vec(device).expect("a device serializes");
        format!("{RETIRED_PREFIX}{}", self.sealed(&plain, RETIRED_AAD))
    }

    /// The device inside a token of the first format, if one of these keys sealed it. Whether it is still taken (the
    /// transition) is the caller's choice: `open` always reads it as retired.
    pub fn open_retired(&self, token: &str) -> Option<Device> {
        let raw = URL_SAFE_NO_PAD.decode(token.strip_prefix(RETIRED_PREFIX)?).ok()?;
        if raw.len() <= NONCE_LEN {
            return None;
        }
        let (nonce, sealed) = raw.split_at(NONCE_LEN);
        self.keys.iter().find_map(|key| {
            let plain = key.decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: RETIRED_AAD }).ok()?;
            serde_json::from_slice(&plain).ok()
        })
    }

    fn sealed(&self, plain: &[u8], aad: &[u8]) -> String {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let sealed = self.keys[0]
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
            .expect("encryption with a valid key does not fail");
        let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        URL_SAFE_NO_PAD.encode(out)
    }

    /// What a relay token holds, if one of these keys sealed it and it has not ended at `now`.
    pub fn open(&self, token: &str, now: u64) -> Result<Claims, Closed> {
        if token.starts_with(RETIRED_PREFIX) {
            return Err(Closed::Retired);
        }
        let raw = token.strip_prefix(PREFIX).and_then(|t| URL_SAFE_NO_PAD.decode(t).ok()).ok_or(Closed::Invalid)?;
        if raw.len() <= NONCE_LEN {
            return Err(Closed::Invalid);
        }
        let (nonce, sealed) = raw.split_at(NONCE_LEN);
        let claims: Claims = self
            .keys
            .iter()
            .find_map(|key| {
                let plain = key.decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: AAD }).ok()?;
                serde_json::from_slice(&plain).ok()
            })
            .ok_or(Closed::Invalid)?;
        if now >= claims.exp {
            return Err(Closed::Expired);
        }
        Ok(claims)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;

    fn device() -> Device {
        Device { platform: Platform::Apns, environment: "production".into(), token: "ab".repeat(32) }
    }

    #[test]
    fn a_sealed_token_opens_only_with_its_key_and_hides_the_device_token() {
        let sealer = Sealer::new(&[[7u8; 32]]).unwrap();
        let token = sealer.seal(&device(), Audience::Bridge, NOW);
        assert!(token.starts_with(PREFIX) && !token.contains(&"ab".repeat(32)));
        let claims = sealer.open(&token, NOW).unwrap();
        assert_eq!(
            (claims.device, claims.aud, claims.iat, claims.exp),
            (device(), Audience::Bridge, NOW, NOW + LIFETIME_SECS)
        );
        assert_eq!(claims.id.len(), 16);
        assert_ne!(token, sealer.seal(&device(), Audience::Bridge, NOW), "a fresh nonce each time");
        assert_eq!(Sealer::new(&[[8u8; 32]]).unwrap().open(&token, NOW), Err(Closed::Invalid));
        let mut altered = token.clone().into_bytes();
        let last = altered.len() - 2;
        altered[last] = if altered[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(sealer.open(std::str::from_utf8(&altered).unwrap(), NOW), Err(Closed::Invalid));
        assert_eq!(sealer.open("r2.!!", NOW), Err(Closed::Invalid));
        assert_eq!(sealer.open(&"ab".repeat(32), NOW), Err(Closed::Invalid));
    }

    #[test]
    fn a_token_ends_after_ninety_days_and_the_first_format_is_retired() {
        let sealer = Sealer::new(&[[7u8; 32]]).unwrap();
        let token = sealer.seal(&device(), Audience::Guichet, NOW);
        assert_eq!(sealer.open(&token, NOW + LIFETIME_SECS - 1).unwrap().aud, Audience::Guichet);
        assert_eq!(sealer.open(&token, NOW + LIFETIME_SECS), Err(Closed::Expired));
        assert_eq!(sealer.open(&format!("r1.{}", "A".repeat(80)), NOW), Err(Closed::Retired));
    }

    #[test]
    fn a_first_format_token_opens_only_through_its_own_door() {
        let sealer = Sealer::new(&[[7u8; 32]]).unwrap();
        let old = sealer.seal_retired(&device());
        assert!(old.starts_with(RETIRED_PREFIX) && !old.contains(&"ab".repeat(32)));
        assert_eq!(sealer.open_retired(&old), Some(device()));
        assert_eq!(sealer.open(&old, NOW), Err(Closed::Retired), "open never takes it");
        assert_eq!(Sealer::new(&[[8u8; 32]]).unwrap().open_retired(&old), None);
        // the two formats can't be passed off as each other
        let new = sealer.seal(&device(), Audience::Bridge, NOW);
        assert_eq!(sealer.open_retired(&new.replacen(PREFIX, RETIRED_PREFIX, 1)), None);
        assert_eq!(sealer.open(&old.replacen(RETIRED_PREFIX, PREFIX, 1), NOW), Err(Closed::Invalid));
    }

    #[test]
    fn a_new_key_seals_while_the_old_one_still_opens() {
        let old = Sealer::new(&[[1u8; 32]]).unwrap();
        let rotated = Sealer::new(&[[2u8; 32], [1u8; 32]]).unwrap();
        let before = old.seal(&device(), Audience::Bridge, NOW);
        assert_eq!(rotated.open(&before, NOW).unwrap().device, device());
        assert_eq!(old.open(&rotated.seal(&device(), Audience::Bridge, NOW), NOW), Err(Closed::Invalid));
    }

    #[test]
    fn keys_are_read_from_base64() {
        let key = Sealer::generate_key();
        assert!(Sealer::from_base64_list(&format!("{key}, {}", Sealer::generate_key())).is_ok());
        assert!(Sealer::from_base64_list("").is_err());
        assert!(Sealer::from_base64_list("c2hvcnQ=").is_err());
    }
}
