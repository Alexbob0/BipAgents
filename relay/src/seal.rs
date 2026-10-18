//! Relay tokens: a device's push token, sealed with a key only the relay holds (XChaCha20-Poly1305).
//!
//! The relay keeps nothing: the app registers its Apple or Google token and gets back a relay token, which it
//! gives to its bridge; each push brings the relay token back and the relay opens it. Several keys may be set:
//! the first seals, all open, so a key can be replaced without cutting anyone off.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};

pub const PREFIX: &str = "r1.";
const AAD: &[u8] = b"bipagents-relay-token-v1";
const NONCE_LEN: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Apns,
    Fcm,
}

/// What a relay token holds.
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

    pub fn seal(&self, device: &Device) -> String {
        let plain = serde_json::to_vec(device).expect("a device serializes");
        let mut nonce = [0u8; NONCE_LEN];
        rand::rngs::OsRng.fill_bytes(&mut nonce);
        let sealed = self.keys[0]
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &plain, aad: AAD })
            .expect("encryption with a valid key does not fail");
        let mut out = Vec::with_capacity(NONCE_LEN + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(out))
    }

    /// The device inside a relay token; `None` when it was not sealed by one of these keys (or was altered).
    pub fn open(&self, token: &str) -> Option<Device> {
        let raw = URL_SAFE_NO_PAD.decode(token.strip_prefix(PREFIX)?).ok()?;
        if raw.len() <= NONCE_LEN {
            return None;
        }
        let (nonce, sealed) = raw.split_at(NONCE_LEN);
        self.keys.iter().find_map(|key| {
            let plain = key.decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad: AAD }).ok()?;
            serde_json::from_slice(&plain).ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Device {
        Device { platform: Platform::Apns, environment: "production".into(), token: "ab".repeat(32) }
    }

    #[test]
    fn a_sealed_token_opens_only_with_its_key_and_hides_the_device_token() {
        let sealer = Sealer::new(&[[7u8; 32]]).unwrap();
        let token = sealer.seal(&device());
        assert!(token.starts_with(PREFIX) && !token.contains(&"ab".repeat(32)));
        assert_eq!(sealer.open(&token), Some(device()));
        assert_ne!(token, sealer.seal(&device()), "a fresh nonce each time");
        assert_eq!(Sealer::new(&[[8u8; 32]]).unwrap().open(&token), None);
        let mut altered = token.clone().into_bytes();
        let last = altered.len() - 2;
        altered[last] = if altered[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(sealer.open(std::str::from_utf8(&altered).unwrap()), None);
        assert_eq!(sealer.open("r1.!!"), None);
        assert_eq!(sealer.open(&"ab".repeat(32)), None);
    }

    #[test]
    fn a_new_key_seals_while_the_old_one_still_opens() {
        let old = Sealer::new(&[[1u8; 32]]).unwrap();
        let rotated = Sealer::new(&[[2u8; 32], [1u8; 32]]).unwrap();
        let before = old.seal(&device());
        assert_eq!(rotated.open(&before), Some(device()));
        assert_eq!(old.open(&rotated.seal(&device())), None);
    }

    #[test]
    fn keys_are_read_from_base64() {
        let key = Sealer::generate_key();
        assert!(Sealer::from_base64_list(&format!("{key}, {}", Sealer::generate_key())).is_ok());
        assert!(Sealer::from_base64_list("").is_err());
        assert!(Sealer::from_base64_list("c2hvcnQ=").is_err());
    }
}
