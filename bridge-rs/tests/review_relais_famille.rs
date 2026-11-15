//! Security review of the family relay (family_seal.rs, branch family-relay-anywhere): replay protection that
//! fails open, and what a request answered twice then gives away. Each test fails on dd94437a8 and should pass once
//! the finding is fixed.

use bipbridge::family_seal::{answer_sealer, Seen, Stale, Way, KEEP_SEEN, SALT_LEN};

/// `Seen::take` accepts a request whose salt it could not write down (it only logs « a call seen not written
/// down »): with the disk full (or the file unwritable), a bridge that restarts forgets every call it took, and the
/// relay, which kept them, sends them again within the 5 minutes: each is run a second time (a message posted twice,
/// an expense added twice) and answered again under the very same keys (next test).
#[test]
fn a_call_not_written_down_as_seen_is_refused_rather_than_taken_twice() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_string_lossy().into_owned();
    // where the list of calls seen goes, something it can't be written to (a full disk reads the same)
    std::fs::create_dir(dir.path().join("family-relay-seen")).unwrap();
    let now = 1_800_000_000.0;
    let salt = [5u8; SALT_LEN];
    let first = Seen::new(&data).take(&salt, now, now);
    // the bridge restarted a minute later; the relay sends the same call again
    let again = Seen::new(&data).take(&salt, now, now + 60.0);
    assert!(
        first.is_err() || again == Err(Stale::Replay),
        "taken ({first:?}) without being kept, then taken again after a restart ({again:?})"
    );
    assert!(KEEP_SEEN > 60.0);
}

/// An answer's key and nonces come from the link's key and the request's salt only: a request answered twice gives
/// two answers sealed with the same key and the same nonces (ChaCha20-Poly1305: same keystream, Poly1305 key
/// reused). The relay that replayed it XORs the two answers and reads one from the other, and may forge frames under
/// that key. An answer with its own fresh salt (or nonce prefix) from the answering bridge would not depend on the
/// replay store never failing.
#[test]
fn two_answers_to_one_request_never_share_their_keystream() {
    let (key, salt) = ([7u8; 32], [9u8; SALT_LEN]);
    let first = br#"{"s":200,"h":[],"note":"the agenda: dentist on Monday"}"#;
    let second = br#"{"s":200,"h":[],"note":"the agenda: lawyer on Tuesday"}"#;
    assert_eq!(first.len(), second.len());
    let a = answer_sealer(&key, &salt, Way::JoinerCalls).seal(first, false);
    let b = answer_sealer(&key, &salt, Way::JoinerCalls).seal(second, false);
    // a frame: its length (4 bytes), then [last flag, data…] sealed, then the tag
    let sealed_xor: Vec<u8> = a[5..5 + first.len()].iter().zip(&b[5..5 + second.len()]).map(|(x, y)| x ^ y).collect();
    let plain_xor: Vec<u8> = first.iter().zip(second.iter()).map(|(x, y)| x ^ y).collect();
    assert_ne!(sealed_xor, plain_xor, "the two answers' XOR is their plain texts' XOR: one keystream for both");
}
