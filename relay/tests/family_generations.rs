//! R7 (review of R5, 1594a864a): a pass made after a link was removed must be newer than the pass withdrawn, whatever
//! the relay's clock says. Generations are the relay's time in milliseconds, kept above the last one given only while
//! it runs: a relay restarted with its clock behind (no NTP yet, a machine restored) makes passes *older* than one it
//! made before, and a withdrawal that relies on the kept households' new passes being newer fails silently. The
//! bridge says which passes were withdrawn (`after`) when it asks for a pass: its generation is then forced above
//! theirs (ids the relay gave for that very mailbox; anything else counts for nothing).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

fn config(seal_keys: &str) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        seal_keys: seal_keys.to_string(),
        apns: None,
        fcm: None,
        fcm_send_base: None,
        pushes_per_device_hour: 10,
        pushes_per_ip_hour: 10,
        registrations_per_ip_hour: 10,
        trust_forwarded_for: false,
        report_url: None,
        report_secret: None,
        reports_per_ip_hour: 1,
        blocked: Vec::new(),
        guichet_keys: Vec::new(),
        r1_until: 0,
        family: FamilyConfig { mailboxes_per_ip_hour: 1000, ..FamilyConfig::default() },
    }
}

fn hmac(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let (mut inner, mut outer) = ([0x36u8; 64], [0x5cu8; 64]);
    for (i, k) in key.iter().enumerate() {
        inner[i] ^= k;
        outer[i] ^= k;
    }
    let inside = Sha256::new().chain_update(inner).chain_update(message).finalize();
    Sha256::new().chain_update(outer).chain_update(inside).finalize().into()
}

/// A pass id for `mailbox` of this generation, as the relay with this sealing key makes one (its clock then at
/// `generation`).
fn made_at(seal_key: &str, mailbox: &str, generation: u64) -> String {
    let key: [u8; 32] =
        Sha256::new().chain_update(b"bipagents-family-pass-id-v1").chain_update(seal_key.as_bytes()).finalize().into();
    let mut signed = generation.to_be_bytes().to_vec();
    signed.extend_from_slice(&[7u8; 8]);
    let mut message = mailbox.as_bytes().to_vec();
    message.push(b':');
    message.extend_from_slice(&signed);
    let mut raw = signed;
    raw.extend_from_slice(&hmac(&key, &message)[..9]);
    URL_SAFE_NO_PAD.encode(raw)
}

fn generation(id: &str) -> u64 {
    let raw = URL_SAFE_NO_PAD.decode(id).unwrap();
    u64::from_be_bytes(raw[..8].try_into().unwrap())
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

#[tokio::test]
async fn a_pass_made_after_a_withdrawal_is_newer_than_the_one_withdrawn_whatever_the_relay_clock() {
    let key = Sealer::generate_key();
    let relay = Arc::new(Relay::new(config(&key), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (url, http) = (format!("http://{addr}"), reqwest::Client::new());
    let mailbox = || async {
        let made: Value =
            http.post(format!("{url}/v1/family/mailbox")).json(&json!({})).send().await.unwrap().json().await.unwrap();
        (made["mailbox"].as_str().unwrap().to_string(), made["token"].as_str().unwrap().to_string())
    };
    let ((lea, lea_token), (paul, _)) = (mailbox().await, mailbox().await);
    let pass = |body: Value| {
        let ask = http.post(format!("{url}/v1/family/pass")).bearer_auth(&lea_token).json(&body).send();
        async move {
            let got = ask.await.unwrap();
            assert_eq!(got.status(), 200);
            got.json::<Value>().await.unwrap()["id"].as_str().unwrap().to_string()
        }
    };
    // the pass Léa gave Alex, made while the relay's clock was a day ahead (it has been set right since)
    let withdrawn = made_at(&key, &lea, now_ms() + 86_400_000);
    // the relay's clock alone: a pass made now is older than it, and Alex's would still work
    let plain = pass(json!({"for": paul})).await;
    assert!(generation(&plain) < generation(&withdrawn), "the relay's clock is a day behind the pass withdrawn");
    // Léa says which pass was withdrawn: Paul's new pass is newer than it
    let after = pass(json!({"for": paul, "after": [withdrawn]})).await;
    assert!(generation(&after) > generation(&withdrawn), "a pass made after the withdrawal is older than the one withdrawn");
    // another mailbox's pass, or a made-up id, counts for nothing (and is no error)
    let theirs = made_at(&key, &paul, now_ms() + 3 * 86_400_000);
    let ignored = pass(json!({"for": paul, "after": [theirs, "AAAA"]})).await;
    assert!(generation(&ignored) < generation(&theirs), "another mailbox's pass moved Léa's generations");
}
