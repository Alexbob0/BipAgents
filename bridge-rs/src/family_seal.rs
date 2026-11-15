//! What two linked households' bridges say to each other through Kerfy Studio's relay (family_rendezvous.rs), sealed
//! so that the relay only ever carries bytes it can neither read nor change unseen.
//!
//! - **The link's key.** At the link (peers.rs `join` and `family_accept`), each bridge makes an X25519 key for that
//!   once; the two public halves cross in the invitation's exchange and both bridges derive the same 32 bytes, with
//!   the invitation's one-time code as salt (HKDF-SHA256): a relay that swapped the halves does not have the code.
//!   Kept in each side's households.json entry (`seal`, 0600), nowhere else.
//! - **A call.** A fresh 32-byte salt; the request's key and its answer's key are derived from the link's key, that
//!   salt, and which way it goes (`i2j`: the inviter calls, `j2i`: the joiner calls): a call sent back to its sender
//!   doesn't open there, an answer doesn't open as another call's. The bytes travel in frames (ChaCha20-Poly1305, the
//!   nonce is the frame's number, the last frame is marked): a frame changed, dropped, moved, or the end cut, and the
//!   whole is refused.
//! - **Sent again.** A request says when it was made (`t`); one further than [`WINDOW`] from now, or whose salt was
//!   seen already ([`Seen`]: in memory and in `family-relay-seen` next to the database, so a restart forgets nothing)
//!   is refused.
//! - **The invitation's own exchange** (before the link has a key) is sealed the same way with a key derived from the
//!   invitation's code alone (`inv`), which only the two people's apps ever had (a QR code shown, or the account's
//!   box sealed for the other person's devices).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use hmac::{Hmac, Mac};
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, CHACHA20_POLY1305};
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, X25519};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The request's first byte.
pub const VERSION: u8 = 1;
pub const SALT_LEN: usize = 32;
/// The most bytes one frame carries.
pub const FRAME: usize = 64 * 1024;
const TAG: usize = 16;
/// How far from now a request's time may be (bridges' clocks differ a little).
pub const WINDOW: f64 = 300.0;
/// How long a request's salt is kept as seen: longer than the window on both sides.
pub const KEEP_SEEN: f64 = 2.0 * WINDOW + 60.0;

type HmacSha256 = Hmac<Sha256>;

/// HKDF-SHA256 (RFC 5869), one block of 32 bytes.
pub fn hkdf(salt: &[u8], ikm: &[u8], info: &[u8]) -> [u8; 32] {
    let mut extract = <HmacSha256 as Mac>::new_from_slice(salt).expect("HMAC takes any key");
    extract.update(ikm);
    let prk = extract.finalize().into_bytes();
    let mut expand = <HmacSha256 as Mac>::new_from_slice(&prk).expect("HMAC takes any key");
    expand.update(info);
    expand.update(&[1]);
    expand.finalize().into_bytes().into()
}

/// A link key as households.json keeps it (base64).
pub fn encode_key(key: &[u8; 32]) -> String {
    STANDARD.encode(key)
}

pub fn decode_key(text: &str) -> Option<[u8; 32]> {
    STANDARD.decode(text.trim()).ok()?.try_into().ok()
}

/// The invitation's exchange's key, from its code alone (`inv`).
pub fn invite_key(code: &str) -> [u8; 32] {
    hkdf(b"bip-family-invite-v1", code.as_bytes(), b"invite")
}

/// One side's half of a link's key, made for that one link.
pub struct Handshake {
    private: EphemeralPrivateKey,
    public: Vec<u8>,
}

impl Handshake {
    pub fn new() -> anyhow::Result<Self> {
        let rng = ring::rand::SystemRandom::new();
        let private = EphemeralPrivateKey::generate(&X25519, &rng).map_err(|_| anyhow::anyhow!("no key"))?;
        let public = private.compute_public_key().map_err(|_| anyhow::anyhow!("no key"))?.as_ref().to_vec();
        Ok(Self { private, public })
    }

    /// Our half, as it travels (base64url).
    pub fn public(&self) -> String {
        URL_SAFE_NO_PAD.encode(&self.public)
    }

    /// The link's key from their half and the invitation's code (`we_invited`: whether this bridge made the
    /// invitation). None for a half that is no X25519 key, or one that gives nothing (a low-order point).
    pub fn link_key(self, theirs: &str, code: &str, we_invited: bool) -> Option<[u8; 32]> {
        let their: [u8; 32] = URL_SAFE_NO_PAD.decode(theirs.trim()).ok()?.try_into().ok()?;
        let shared = agree_ephemeral(self.private, &UnparsedPublicKey::new(&X25519, their), |s| s.to_vec()).ok()?;
        if shared.iter().all(|b| *b == 0) {
            return None;
        }
        let (inviter, joiner) =
            if we_invited { (&self.public[..], &their[..]) } else { (&their[..], &self.public[..]) };
        let salt = Sha256::digest([b"bip-family-link-v1".as_slice(), code.as_bytes()].concat());
        let info = [b"bip-family-link-v1".as_slice(), inviter, joiner].concat();
        Some(hkdf(&salt, &shared, &info))
    }
}

/// Which way a call goes: what its keys are derived with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// The inviter of the link calls the joiner.
    InviterCalls,
    JoinerCalls,
    /// The invitation's own exchange.
    Invite,
}

impl Way {
    fn label(self) -> &'static str {
        match self {
            Way::InviterCalls => "i2j",
            Way::JoinerCalls => "j2i",
            Way::Invite => "inv",
        }
    }

    /// Our calls in a link where we are `role` (`inviter` or `joiner`).
    pub fn out(role: &str) -> Option<Way> {
        match role {
            "inviter" => Some(Way::InviterCalls),
            "joiner" => Some(Way::JoinerCalls),
            _ => None,
        }
    }

    /// Their calls to us.
    pub fn incoming(role: &str) -> Option<Way> {
        match role {
            "inviter" => Some(Way::JoinerCalls),
            "joiner" => Some(Way::InviterCalls),
            _ => None,
        }
    }
}

/// The frames of one direction of one call.
pub struct Framer {
    key: LessSafeKey,
    n: u64,
    ended: bool,
}

impl Framer {
    fn new(base: &[u8; 32], salt: &[u8], way: Way, answer: bool) -> Self {
        let info = format!("bip-family-v1 {} {}", if answer { "ans" } else { "req" }, way.label());
        let key = hkdf(salt, base, info.as_bytes());
        let key = LessSafeKey::new(UnboundKey::new(&CHACHA20_POLY1305, &key).expect("a 32-byte key"));
        Self { key, n: 0, ended: false }
    }

    fn nonce(&self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[4..].copy_from_slice(&self.n.to_be_bytes());
        Nonce::assume_unique_for_key(nonce)
    }

    /// One frame: its length (4 bytes, big-endian), then `[last] data` sealed.
    pub fn seal(&mut self, data: &[u8], last: bool) -> Vec<u8> {
        let mut plain = Vec::with_capacity(1 + data.len() + TAG);
        plain.push(u8::from(last));
        plain.extend_from_slice(data);
        self.key.seal_in_place_append_tag(self.nonce(), Aad::empty(), &mut plain).expect("sealing does not fail");
        self.n += 1;
        let mut out = Vec::with_capacity(4 + plain.len());
        out.extend_from_slice(&(plain.len() as u32).to_be_bytes());
        out.extend_from_slice(&plain);
        out
    }

    /// `data` in as many frames as it takes, the last one marked when `last`.
    pub fn seal_all(&mut self, data: &[u8], last: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut pieces = data.chunks(FRAME).peekable();
        if pieces.peek().is_none() {
            return self.seal(&[], last);
        }
        while let Some(piece) = pieces.next() {
            out.extend(self.seal(piece, last && pieces.peek().is_none()));
        }
        out
    }

    /// The next frame's data at the front of `buffer` (taken out of it): None when it isn't all there yet.
    fn open_next(&mut self, buffer: &mut Vec<u8>) -> Result<Option<(Vec<u8>, bool)>, Broken> {
        if self.ended {
            return if buffer.is_empty() { Ok(None) } else { Err(Broken) }; // bytes after the end
        }
        if buffer.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        if !(1 + TAG..=1 + FRAME + TAG).contains(&len) {
            return Err(Broken);
        }
        if buffer.len() < 4 + len {
            return Ok(None);
        }
        let mut sealed: Vec<u8> = buffer.drain(..4 + len).skip(4).collect();
        let plain = self.key.open_in_place(self.nonce(), Aad::empty(), &mut sealed).map_err(|_| Broken)?;
        self.n += 1;
        let (flag, data) = plain.split_first().ok_or(Broken)?;
        let last = match flag {
            0 => false,
            1 => true,
            _ => return Err(Broken),
        };
        self.ended = last;
        Ok(Some((data.to_vec(), last)))
    }
}

/// Not sealed by the key expected, changed, or cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Broken;

/// A request: its version, its salt, then frames: the head (JSON: method, path, headers, time), then the body.
pub fn seal_request(base: &[u8; 32], way: Way, head: &Value, body: &[u8]) -> (Vec<u8>, [u8; SALT_LEN]) {
    use ring::rand::SecureRandom;
    let mut salt = [0u8; SALT_LEN];
    ring::rand::SystemRandom::new().fill(&mut salt).expect("randomness");
    let mut framer = Framer::new(base, &salt, way, false);
    let mut wire = vec![VERSION];
    wire.extend_from_slice(&salt);
    wire.extend(framer.seal(head.to_string().as_bytes(), body.is_empty()));
    if !body.is_empty() {
        wire.extend(framer.seal_all(body, true));
    }
    (wire, salt)
}

/// What [`seal_request`] sealed with this key and way: (salt, head, body). Every frame must open and the last be
/// marked, with nothing after it.
pub fn open_request(base: &[u8; 32], way: Way, wire: &[u8]) -> Result<([u8; SALT_LEN], Value, Vec<u8>), Broken> {
    let (&version, rest) = wire.split_first().ok_or(Broken)?;
    if version != VERSION || rest.len() < SALT_LEN {
        return Err(Broken);
    }
    let (salt, frames) = rest.split_at(SALT_LEN);
    let salt: [u8; SALT_LEN] = salt.try_into().map_err(|_| Broken)?;
    let mut framer = Framer::new(base, &salt, way, false);
    let mut buffer = frames.to_vec();
    let (head, mut last) = framer.open_next(&mut buffer)?.ok_or(Broken)?;
    let head: Value = serde_json::from_slice(&head).map_err(|_| Broken)?;
    let mut body = Vec::new();
    while !last {
        let (data, end) = framer.open_next(&mut buffer)?.ok_or(Broken)?;
        body.extend(data);
        last = end;
    }
    if !buffer.is_empty() {
        return Err(Broken);
    }
    Ok((salt, head, body))
}

/// The answer's frames, sealed as they go (the head first: status and headers).
pub fn answer_sealer(base: &[u8; 32], salt: &[u8; SALT_LEN], way: Way) -> Framer {
    Framer::new(base, salt, way, true)
}

/// The answer's frames, opened as its bytes come.
pub struct AnswerOpener {
    framer: Framer,
    buffer: Vec<u8>,
}

impl AnswerOpener {
    pub fn new(base: &[u8; 32], salt: &[u8; SALT_LEN], way: Way) -> Self {
        Self { framer: Framer::new(base, salt, way, true), buffer: Vec::new() }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next frame (data, last), None while it isn't all there.
    pub fn next_frame(&mut self) -> Result<Option<(Vec<u8>, bool)>, Broken> {
        self.framer.open_next(&mut self.buffer)
    }

    /// Whether the last frame came.
    pub fn ended(&self) -> bool {
        self.framer.ended
    }
}

/// Requests seen (their salt, until when it is kept), in memory and in a file next to the database.
pub struct Seen {
    path: PathBuf,
    items: Mutex<Option<HashMap<[u8; SALT_LEN], f64>>>,
}

/// Why a request opened but is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stale {
    /// Made too long ago, or in the future (a clock far off).
    Time,
    /// Seen already: sent again.
    Replay,
}

impl Seen {
    pub fn new(data_dir: &str) -> Self {
        Self { path: PathBuf::from(crate::py::join(data_dir, "family-relay-seen")), items: Mutex::new(None) }
    }

    fn load(&self, now: f64) -> HashMap<[u8; SALT_LEN], f64> {
        let mut out = HashMap::new();
        for line in std::fs::read_to_string(&self.path).unwrap_or_default().lines() {
            let mut parts = line.split_whitespace();
            let (Some(salt), Some(until)) = (parts.next(), parts.next()) else { continue };
            let (Ok(salt), Ok(until)) = (hex::decode(salt), until.parse::<f64>()) else { continue };
            if let (Ok(salt), true) = (<[u8; SALT_LEN]>::try_from(salt), until > now) {
                out.insert(salt, until);
            }
        }
        out
    }

    /// A request made at `t` with this salt: taken (and kept as seen), or why not.
    pub fn take(&self, salt: &[u8; SALT_LEN], t: f64, now: f64) -> Result<(), Stale> {
        if !t.is_finite() || (now - t).abs() > WINDOW {
            return Err(Stale::Time);
        }
        let mut guard = self.items.lock().unwrap_or_else(|e| e.into_inner());
        let items = guard.get_or_insert_with(|| self.load(now));
        if items.contains_key(salt) {
            return Err(Stale::Replay);
        }
        let until = now + KEEP_SEEN;
        items.insert(*salt, until);
        if items.len() > 4096 {
            items.retain(|_, kept| *kept > now);
            let text: String = items.iter().map(|(s, u)| format!("{} {u}\n", hex::encode(s))).collect();
            let _ = crate::py::write_with_mode(&self.path.to_string_lossy(), text.as_bytes(), 0o600);
        } else {
            let line = format!("{} {until}\n", hex::encode(salt));
            let appended = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .and_then(|mut f| f.write_all(line.as_bytes()));
            if let Err(e) = appended {
                tracing::warn!(error = %e, "family relay: a call seen not written down");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn linked() -> ([u8; 32], [u8; 32]) {
        let (inviter, joiner) = (Handshake::new().unwrap(), Handshake::new().unwrap());
        let (i_half, j_half) = (inviter.public(), joiner.public());
        let code = "the-invitations-code";
        (inviter.link_key(&j_half, code, true).unwrap(), joiner.link_key(&i_half, code, false).unwrap())
    }

    #[test]
    fn both_sides_derive_the_same_link_key_and_only_with_the_code() {
        let (a, b) = linked();
        assert_eq!(a, b);
        let (inviter, joiner) = (Handshake::new().unwrap(), Handshake::new().unwrap());
        let (i_half, j_half) = (inviter.public(), joiner.public());
        let a = inviter.link_key(&j_half, "code-1", true).unwrap();
        let b = joiner.link_key(&i_half, "code-2", false).unwrap();
        assert_ne!(a, b, "a relay that swapped the halves without the code gets another key");
        assert!(Handshake::new().unwrap().link_key("short", "c", true).is_none());
        assert!(Handshake::new().unwrap().link_key(&URL_SAFE_NO_PAD.encode([0u8; 32]), "c", true).is_none());
        assert_eq!(decode_key(&encode_key(&a)), Some(a));
    }

    #[test]
    fn a_request_opens_with_its_key_and_way_only() {
        let (key, _) = linked();
        let head =
            json!({"m": "POST", "p": "/family/p/cuisine/v1/runs", "h": [["authorization", "Bearer k"]], "t": 1.0});
        let body = vec![7u8; FRAME * 2 + 10];
        let (wire, salt) = seal_request(&key, Way::JoinerCalls, &head, &body);
        let (opened_salt, opened, got) = open_request(&key, Way::JoinerCalls, &wire).unwrap();
        assert_eq!((opened_salt, opened, got), (salt, head.clone(), body));
        // sent back to its sender, or as another link's
        assert_eq!(open_request(&key, Way::InviterCalls, &wire).map(|_| ()), Err(Broken));
        let (other, _) = linked();
        assert_eq!(open_request(&other, Way::JoinerCalls, &wire).map(|_| ()), Err(Broken));
        // any bit changed, a frame cut, bytes after the end
        for at in [0, 1, 20, 40, wire.len() / 2, wire.len() - 1] {
            let mut changed = wire.clone();
            changed[at] ^= 1;
            assert_eq!(open_request(&key, Way::JoinerCalls, &changed).map(|_| ()), Err(Broken), "bit {at}");
        }
        assert!(open_request(&key, Way::JoinerCalls, &wire[..wire.len() - 100]).is_err());
        let mut longer = wire.clone();
        longer.extend(b"more");
        assert!(open_request(&key, Way::JoinerCalls, &longer).is_err());
        // an empty body: the head alone, marked last
        let (wire, _) = seal_request(&key, Way::InviterCalls, &head, b"");
        assert_eq!(open_request(&key, Way::InviterCalls, &wire).unwrap().2, Vec::<u8>::new());
    }

    #[test]
    fn an_answer_opens_frame_by_frame_and_a_cut_one_never_ends() {
        let (key, _) = linked();
        let (_, salt) = seal_request(&key, Way::InviterCalls, &json!({}), b"");
        let mut sealer = answer_sealer(&key, &salt, Way::InviterCalls);
        let mut wire = sealer.seal(br#"{"s":200,"h":[]}"#, false);
        wire.extend(sealer.seal_all(b"hello ", false));
        wire.extend(sealer.seal_all(b"world", false));
        wire.extend(sealer.seal(b"", true));
        let mut opener = AnswerOpener::new(&key, &salt, Way::InviterCalls);
        let mut got = Vec::new();
        for byte in &wire {
            opener.push(&[*byte]); // one byte at a time: frames open only once whole
            while let Some((data, _)) = opener.next_frame().unwrap() {
                got.push(data);
            }
        }
        assert!(opener.ended());
        assert_eq!(got[1..].concat(), b"hello world");
        // another call's answer (another salt), or the request's way of keys: refused
        let (_, other_salt) = seal_request(&key, Way::InviterCalls, &json!({}), b"");
        let mut other = AnswerOpener::new(&key, &other_salt, Way::InviterCalls);
        other.push(&wire);
        assert_eq!(other.next_frame(), Err(Broken));
        // cut before the last frame: never ended
        let mut cut = AnswerOpener::new(&key, &salt, Way::InviterCalls);
        cut.push(&wire[..wire.len() - 5]);
        while let Ok(Some(_)) = cut.next_frame() {}
        assert!(!cut.ended());
    }

    #[test]
    fn a_request_is_taken_once_and_only_while_fresh_even_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().to_string_lossy().into_owned();
        let seen = Seen::new(&data);
        let now = 1_800_000_000.0;
        let salt = [3u8; SALT_LEN];
        assert_eq!(seen.take(&salt, now - 10.0, now), Ok(()));
        assert_eq!(seen.take(&salt, now - 10.0, now), Err(Stale::Replay));
        assert_eq!(seen.take(&[4u8; SALT_LEN], now - WINDOW - 1.0, now), Err(Stale::Time));
        assert_eq!(seen.take(&[4u8; SALT_LEN], now + WINDOW + 1.0, now), Err(Stale::Time));
        assert_eq!(seen.take(&[4u8; SALT_LEN], f64::NAN, now), Err(Stale::Time));
        // the bridge restarted: what it saw is still known
        let again = Seen::new(&data);
        assert_eq!(again.take(&salt, now - 5.0, now + 5.0), Err(Stale::Replay));
        // once well past the window, forgotten (and refused for its time anyway)
        let later = Seen::new(&data);
        assert_eq!(later.take(&salt, now, now + KEEP_SEEN + 1.0), Err(Stale::Time));
    }

    #[test]
    fn the_ways_of_a_link() {
        assert_eq!(Way::out("inviter"), Way::incoming("joiner"));
        assert_eq!(Way::out("joiner"), Some(Way::JoinerCalls));
        assert_eq!(Way::incoming("inviter"), Some(Way::JoinerCalls));
        assert_eq!(Way::out("other"), None);
        assert_eq!(hkdf(b"s", b"k", b"i"), hkdf(b"s", b"k", b"i"));
        assert_ne!(invite_key("a"), invite_key("b"));
    }
}
