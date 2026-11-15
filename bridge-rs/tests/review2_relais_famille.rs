//! Security review, second pass (c18c86384): the fix of F1 checked where the first test can no longer look. An
//! answer now starts with a salt of its own (32 bytes), so `two_answers_to_one_request_never_share_their_keystream`
//! (review_relais_famille.rs) compares bytes of those salts and would pass whatever the keys. Here the sealed data
//! are compared where they are. Expected to pass on c18c86384.

use bipbridge::family_seal::{answer_sealer, AnswerOpener, Way, SALT_LEN};

#[test]
fn two_answers_to_one_request_carry_their_own_salt_and_never_share_a_keystream() {
    let (key, salt) = ([7u8; 32], [9u8; SALT_LEN]);
    let first = br#"{"s":200,"h":[],"note":"the agenda: dentist on Monday"}"#;
    let second = br#"{"s":200,"h":[],"note":"the agenda: lawyer on Tuesday"}"#;
    assert_eq!(first.len(), second.len());
    let a = answer_sealer(&key, &salt, Way::JoinerCalls).seal(first, false);
    let b = answer_sealer(&key, &salt, Way::JoinerCalls).seal(second, false);
    assert_ne!(a[..SALT_LEN], b[..SALT_LEN], "each answer its own salt");
    // then the frame: its length (4 bytes), then [last flag, data…] sealed, then the tag
    let at = SALT_LEN + 4 + 1;
    let (sa, sb) = (&a[at..at + first.len()], &b[at..at + second.len()]);
    let sealed_xor: Vec<u8> = sa.iter().zip(sb.iter()).map(|(x, y)| x ^ y).collect();
    let plain_xor: Vec<u8> = first.iter().zip(second.iter()).map(|(x, y)| x ^ y).collect();
    assert_ne!(sealed_xor, plain_xor, "one keystream for both answers");
    // each still opens as the answer to that request (its salt read first)
    for (wire, plain) in [(&a, &first[..]), (&b, &second[..])] {
        let mut opener = AnswerOpener::new(&key, &salt, Way::JoinerCalls);
        opener.push(wire);
        assert_eq!(opener.next_frame().unwrap().unwrap().0, plain);
    }
    // another request's salt: refused
    let mut other = AnswerOpener::new(&key, &[8u8; SALT_LEN], Way::JoinerCalls);
    other.push(&a);
    assert!(other.next_frame().is_err());
}
