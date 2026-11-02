//! Sealed notifications (refonte phase 5): what an agent said never reaches Apple, Google or a relay in clear.
//!
//! Each phone makes a key pair when it registers (`POST /v1/devices {push_key}`: its P-256 public key, uncompressed,
//! base64) and keeps the private part on the device (iOS Keychain shared with the Notification Service Extension,
//! Android Keystore-wrapped preferences). For a device with a key, every alert push carries:
//!
//! - in clear, a neutral line chosen by type (`aps.alert.loc-key` = `notif.<type>`, translated by both apps' catalogs,
//!   no title, no words of the agent);
//! - `enc`: `{v, t, title, body, sub?}` sealed for that device only. The phone opens it offline and shows the real
//!   text; when it can't, the neutral line stays.
//!
//! The box (contracts/notifications/sealed.json has the vectors the three sides check):
//!
//! ```text
//! enc = base64( E (65, uncompressed P-256) · nonce (12) · AES-256-GCM(plaintext, aad = agent) · tag (16) )
//! key = HKDF-SHA256(ikm = ECDH(e, device).x, salt = E · device public key, info = « bipagents-notif-v1 »)
//! ```
//!
//! The construction is the accounts' box (read_shares.rs `seal_box`, AccountCrypto on the phones), with its own
//! `info` (a notification key is never a box key) and the agent's key as additional data: an `enc` moved onto another
//! agent's push doesn't open. One ephemeral key per push and per device.

use base64::engine::general_purpose::STANDARD as STD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Value};
use sha2::Sha256;

use crate::Dict;

pub const INFO: &[u8] = b"bipagents-notif-v1";
/// The sealed text's format (`v` inside the box).
pub const VERSION: u64 = 1;
/// `enc` at most (base64 characters): the relay drops a longer one.
pub const ENC_MAX: usize = 3000;
/// The whole push at most (JSON bytes, `enc` included): APNs takes 4096, FCM's data as much, a relay's envelope and
/// FCM's own keys need a little room.
pub const PAYLOAD_MAX: usize = 3800;
/// The sealed body at most (characters): what the notifications show (iOS cuts after about 4 lines, Android's big text
/// a little more).
pub const BODY_MAX: usize = 1000;
/// The sealed title at most (characters): an agent's name, a group discussion's after it.
pub const TITLE_MAX: usize = 120;
/// Bytes a box adds to its plaintext: ephemeral key, nonce, tag.
pub const OVERHEAD: usize = 65 + 12 + 16;

/// The journal's types (docs/conversations.md §9.1): each has its neutral line `notif.<type>`, in
/// contracts/notifications/loc-keys.json with its translations.
pub const TYPES: [&str; 16] = [
    "reply",
    "question",
    "approval",
    "room_reply",
    "boite",
    "task",
    "subtask",
    "scan",
    "family",
    "email_held",
    "site",
    "login",
    "connector",
    "secret",
    "mail_card",
    "meet",
];

/// The neutral line of a type, as the phones' catalogs know it.
pub fn loc_key(kind: &str) -> String {
    format!("notif.{kind}")
}

/// What a notification says, before it is sealed for each device.
#[derive(Clone, Debug, PartialEq)]
pub struct Sealed {
    /// One of `TYPES`.
    pub kind: &'static str,
    /// The agent's shown name (« Vie », « Vie · Courses » for a group discussion).
    pub title: String,
    /// What it said (empty: the phone shows the neutral line under the title).
    pub body: String,
    /// A Boîte message's own title, when it has one.
    pub sub: Option<String>,
}

impl Sealed {
    pub fn new(kind: &'static str, title: &str, body: &str) -> Self {
        Sealed { kind, title: short(title, TITLE_MAX), body: clean(body), sub: None }
    }

    /// The plaintext inside the box, its body cut to `chars` characters at most.
    pub fn plaintext(&self, chars: usize) -> Vec<u8> {
        let mut body = self.body.clone();
        if body.chars().count() > chars {
            body = format!("{}…", body.chars().take(chars.saturating_sub(1)).collect::<String>().trim_end());
        }
        let mut doc = Map::new();
        doc.insert("v".into(), json!(VERSION));
        doc.insert("t".into(), json!(self.kind));
        doc.insert("title".into(), json!(self.title));
        doc.insert("body".into(), json!(body));
        if let Some(sub) = self.sub.as_deref().filter(|s| !s.is_empty()) {
            doc.insert("sub".into(), json!(sub));
        }
        serde_json::to_vec(&Value::Object(doc)).expect("JSON")
    }
}

fn short(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max - 1).collect::<String>().trim_end())
}

/// The words a notification shows: `MEDIA:` lines (files the app shows in the conversation) left out, spaces at the
/// ends trimmed, cut at `BODY_MAX` characters.
pub fn clean(text: &str) -> String {
    let kept: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with("MEDIA:")).collect();
    let text = kept.join("\n");
    short(&text, BODY_MAX)
}

/// A device's key as `POST /v1/devices` gives it: base64 (standard or URL-safe, padding optional) of a P-256 public
/// key, uncompressed (65 bytes, 0x04 · x · y), a point of the curve.
pub fn push_key(given: &str) -> Result<Vec<u8>, &'static str> {
    use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
    let given = given.trim();
    let trimmed = given.trim_end_matches('=');
    let raw = STD
        .decode(given)
        .or_else(|_| STANDARD_NO_PAD.decode(trimmed))
        .or_else(|_| URL_SAFE_NO_PAD.decode(trimmed))
        .map_err(|_| "push_key: base64 expected")?;
    if raw.len() != 65 || raw[0] != 4 {
        return Err("push_key: an uncompressed P-256 public key expected (65 bytes)");
    }
    // a point off the curve fails the agreement: tried once here rather than at every push
    seal(b"", &raw, b"").map_err(|_| "push_key: not a P-256 public key")?;
    Ok(raw)
}

/// A key as the bridge stored it (checked at registration by `push_key`): its raw bytes, or None.
pub fn stored_key(stored: &str) -> Option<Vec<u8>> {
    STD.decode(stored).ok().filter(|raw| raw.len() == 65 && raw[0] == 4)
}

type HmacSha256 = Hmac<Sha256>;

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8]) -> [u8; 32] {
    let mut extract = HmacSha256::new_from_slice(salt).expect("HMAC takes any key");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut expand = HmacSha256::new_from_slice(&prk).expect("HMAC takes any key");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

/// `plaintext` sealed for the device key `recipient` (raw, 65 bytes), base64.
pub fn seal(plaintext: &[u8], recipient: &[u8], aad: &[u8]) -> anyhow::Result<String> {
    use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
    use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &rng).map_err(|_| anyhow::anyhow!("no key"))?;
    let public = ephemeral.compute_public_key().map_err(|_| anyhow::anyhow!("no key"))?.as_ref().to_vec();
    let secret = agree_ephemeral(ephemeral, &UnparsedPublicKey::new(&ECDH_P256, recipient), |shared| shared.to_vec())
        .map_err(|_| anyhow::anyhow!("not a P-256 key"))?;
    let key = hkdf(&secret, &[public.as_slice(), recipient].concat(), INFO);
    let mut nonce = [0u8; 12];
    rng.fill(&mut nonce).map_err(|_| anyhow::anyhow!("no randomness"))?;
    let sealing = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).map_err(|_| anyhow::anyhow!("bad key"))?);
    let mut data = plaintext.to_vec();
    sealing
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), &mut data)
        .map_err(|_| anyhow::anyhow!("not sealed"))?;
    let mut out = public;
    out.extend_from_slice(&nonce);
    out.extend(data);
    Ok(STD.encode(out))
}

fn size(payload: &Dict) -> usize {
    serde_json::to_vec(payload).map(|b| b.len()).unwrap_or(usize::MAX)
}

/// The push for one device with a key: `payload`'s ids as they are, its alert replaced by the neutral line of
/// `sealed.kind`, and `enc`. The body is cut until the whole fits `PAYLOAD_MAX` and `enc` fits `ENC_MAX`.
pub fn for_device(payload: &Dict, sealed: &Sealed, agent: &str, key: &[u8]) -> anyhow::Result<Dict> {
    let mut out = payload.clone();
    let code = loc_key(sealed.kind);
    if let Some(Value::Object(aps)) = out.get_mut("aps") {
        // `body` too: Firebase's data takes the alert's body (fcm.rs), the Android app translates the code
        aps.insert("alert".into(), json!({"loc-key": code, "body": code}));
    }
    out.remove("enc");
    // the room shown in the title, a family note's words: never next to `aps` in clear (ids only there)
    out.remove("title");
    let room = size(&out) + r#","enc":"""#.len();
    let budget_b64 = ENC_MAX.min(PAYLOAD_MAX.saturating_sub(room));
    let budget = (budget_b64 / 4 * 3).saturating_sub(OVERHEAD);
    // the longest body that fits (a character is 1 to 4 bytes, more once escaped in JSON): searched by halves
    let total = sealed.body.chars().count();
    let mut plaintext = sealed.plaintext(total);
    if plaintext.len() > budget {
        let (mut fits, mut over) = (0usize, total);
        while over - fits > 1 {
            let mid = (fits + over) / 2;
            if sealed.plaintext(mid).len() <= budget {
                fits = mid;
            } else {
                over = mid;
            }
        }
        plaintext = sealed.plaintext(fits);
        if plaintext.len() > budget {
            anyhow::bail!("no room for the sealed text");
        }
    }
    out.insert("enc".into(), json!(seal(&plaintext, key, agent.as_bytes())?));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> Vec<u8> {
        use ring::agreement::{EphemeralPrivateKey, ECDH_P256};
        let key = EphemeralPrivateKey::generate(&ECDH_P256, &ring::rand::SystemRandom::new()).unwrap();
        key.compute_public_key().unwrap().as_ref().to_vec()
    }

    #[test]
    fn keys_are_checked() {
        let raw = device();
        assert_eq!(push_key(&STD.encode(&raw)).unwrap(), raw);
        let url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
        assert_eq!(push_key(&url).unwrap(), raw);
        assert!(push_key("not base64 !").is_err());
        assert!(push_key(&STD.encode([4u8; 33])).is_err());
        let mut off = raw.clone();
        off[64] ^= 1; // y changed: no longer on the curve
        assert!(push_key(&STD.encode(&off)).is_err());
        let mut compressed = vec![2u8];
        compressed.extend(&raw[1..33]);
        assert!(push_key(&STD.encode(&compressed)).is_err());
    }

    #[test]
    fn a_box_is_new_each_time_and_sized_as_said() {
        let raw = device();
        let a = seal(b"abc", &raw, b"vie").unwrap();
        let b = seal(b"abc", &raw, b"vie").unwrap();
        assert_ne!(a, b);
        assert_eq!(STD.decode(&a).unwrap().len(), 3 + OVERHEAD);
    }

    #[test]
    fn the_clear_part_says_nothing() {
        let raw = device();
        let payload = crate::push::reply_payload("vie", "Vie", "run_1", Some("s1"), Some("Le secret"), None, "x");
        let sealed = Sealed::new("reply", "Vie", "Le secret\nMEDIA:/tmp/a.png");
        assert_eq!(sealed.body, "Le secret");
        let out = for_device(&payload, &sealed, "vie", &raw).unwrap();
        assert_eq!(out["aps"]["alert"], json!({"loc-key": "notif.reply", "body": "notif.reply"}));
        assert_eq!(out["run_id"], "run_1");
        assert_eq!(out["session_id"], "s1");
        let text = serde_json::to_string(&out).unwrap();
        assert!(!text.contains("secret") && !text.contains("\"Vie\""), "{text}");
    }

    #[test]
    fn long_texts_fit() {
        let raw = device();
        let payload = crate::push::reply_payload("vie", "Vie", "run_1", Some("s1"), None, Some(&"r".repeat(32)), "x");
        for body in ["é".repeat(5000), "👋".repeat(3000), "\"\\\n".repeat(2000), "a".repeat(999)] {
            let out = for_device(&payload, &Sealed::new("reply", "Vie", &body), "vie", &raw).unwrap();
            assert!(size(&out) <= PAYLOAD_MAX, "{}", size(&out));
            assert!(out["enc"].as_str().unwrap().len() <= ENC_MAX);
        }
        let short = for_device(&payload, &Sealed::new("reply", "Vie", "a".repeat(999).as_str()), "vie", &raw).unwrap();
        // 999 ASCII characters fit whole
        assert_eq!(STD.decode(short["enc"].as_str().unwrap()).unwrap().len() - OVERHEAD, Sealed::new("reply", "Vie", &"a".repeat(999)).plaintext(BODY_MAX).len());
    }
}
