//! The family rendezvous end to end (family.rs): two bridges that can't reach each other, each holding a long poll
//! to the relay, one calling the other's mailbox and getting its answer streamed back. What passes is sealed by the
//! bridges (bridge-rs family_seal.rs): here it is any bytes, the relay never reads them.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn config(family: FamilyConfig, blocked: Vec<String>, seal_keys: String) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        seal_keys,
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
        blocked,
        guichet_keys: Vec::new(),
        r1_until: 0,
        family,
    }
}

/// Short waits, so that the tests don't take the real ones (15 s to be taken, 25 s polls).
fn quick() -> FamilyConfig {
    FamilyConfig {
        pickup: Duration::from_secs(2),
        poll: Duration::from_secs(3),
        away: Duration::from_secs(2),
        answer_start: Duration::from_secs(5),
        ..FamilyConfig::default()
    }
}

struct Harness {
    url: String,
    addr: SocketAddr,
    http: reqwest::Client,
}

async fn harness(family: FamilyConfig) -> Harness {
    harness_with(family, Vec::new(), Sealer::generate_key()).await
}

async fn harness_with(family: FamilyConfig, blocked: Vec<String>, seal_keys: String) -> Harness {
    let relay = Arc::new(Relay::new(config(family, blocked, seal_keys), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Harness { url: format!("http://{addr}"), addr, http: reqwest::Client::new() }
}

struct Mailbox {
    id: String,
    token: String,
}

impl Harness {
    async fn mailbox(&self) -> Mailbox {
        let answer = self.http.post(format!("{}/v1/family/mailbox", self.url)).json(&json!({})).send().await.unwrap();
        assert_eq!(answer.status(), 200);
        let body: Value = answer.json().await.unwrap();
        Mailbox { id: body["mailbox"].as_str().unwrap().into(), token: body["token"].as_str().unwrap().into() }
    }

    /// One long poll: (status, call id, from, kind, body).
    async fn next(&self, mailbox: &Mailbox) -> (u16, String, String, String, Vec<u8>) {
        let answer =
            self.http.get(format!("{}/v1/family/next", self.url)).bearer_auth(&mailbox.token).send().await.unwrap();
        let header =
            |name: &str| answer.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        let (status, call, from, kind) =
            (answer.status().as_u16(), header("x-bip-call"), header("x-bip-from"), header("x-bip-kind"));
        (status, call, from, kind, answer.bytes().await.unwrap().to_vec())
    }

    fn call(&self, from: &Mailbox, to: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        self.http.post(format!("{}/v1/family/call/{to}", self.url)).bearer_auth(&from.token).body(body)
    }

    async fn answer(&self, by: &Mailbox, call: &str, body: Vec<u8>) -> u16 {
        let sent = self.http.post(format!("{}/v1/family/answer/{call}", self.url)).bearer_auth(&by.token).body(body);
        // 0: the relay stopped reading it (an answer cut)
        sent.send().await.map(|r| r.status().as_u16()).unwrap_or(0)
    }
}

#[tokio::test]
async fn a_bridge_gets_a_mailbox_and_renews_it_keeping_its_id() {
    let relay = harness(quick()).await;
    let first = relay.mailbox().await;
    assert_eq!(first.id.len(), 22);
    assert!(first.token.starts_with("fm1.") && !first.token.contains(&first.id), "the id is sealed in its token");
    assert_ne!(relay.mailbox().await.id, first.id, "each bridge its own mailbox");
    let renewed: Value = relay
        .http
        .post(format!("{}/v1/family/mailbox", relay.url))
        .json(&json!({"token": first.token}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(renewed["mailbox"], first.id.as_str());
    assert_ne!(renewed["token"], first.token.as_str());
    let forged = relay
        .http
        .post(format!("{}/v1/family/mailbox", relay.url))
        .json(&json!({"token": format!("fm1.{}", "A".repeat(80))}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 401);
}

#[tokio::test]
async fn a_call_reaches_the_polling_mailbox_and_its_answer_streams_back() {
    let relay = Arc::new(harness(quick()).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    // Léa's bridge polls; Alex's calls her mailbox
    let poller = {
        let relay = relay.clone();
        let lea = Mailbox { id: lea.id.clone(), token: lea.token.clone() };
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(relay.call(&alex, &lea.id, b"sealed request".to_vec()).send());
    let (status, id, from, kind, body) = poller.await.unwrap();
    assert_eq!(
        (status, from.as_str(), kind.as_str(), body.as_slice()),
        (200, alex.id.as_str(), "call", &b"sealed request"[..])
    );
    // her answer, streamed: the first bytes reach Alex before its end
    let mut socket = tokio::net::TcpStream::connect(relay.addr).await.unwrap();
    let head = format!(
        "POST /v1/family/answer/{id} HTTP/1.1\r\nHost: relay\r\nAuthorization: Bearer {}\r\nTransfer-Encoding: chunked\r\n\r\n",
        lea.token
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    socket.write_all(b"6\r\nsealed\r\n").await.unwrap();
    let mut answer = call.await.unwrap().unwrap();
    assert_eq!(answer.status(), 200);
    let first = tokio::time::timeout(Duration::from_secs(3), answer.chunk()).await.expect("streamed").unwrap().unwrap();
    assert_eq!(&first[..], b"sealed");
    socket.write_all(b"7\r\n answer\r\n0\r\n\r\n").await.unwrap();
    let mut rest = Vec::new();
    while let Some(chunk) = answer.chunk().await.unwrap() {
        rest.extend_from_slice(&chunk);
    }
    assert_eq!(rest, b" answer");
    let mut ack = vec![0u8; 64];
    let read = socket.read(&mut ack).await.unwrap();
    assert!(String::from_utf8_lossy(&ack[..read]).starts_with("HTTP/1.1 204"));
}

#[tokio::test]
async fn a_mailbox_nobody_polls_is_away_at_once() {
    let relay = harness(quick()).await;
    let (alex, off) = (relay.mailbox().await, relay.mailbox().await);
    let started = Instant::now();
    let answer = relay.call(&alex, &off.id, b"x".to_vec()).send().await.unwrap();
    assert_eq!(answer.status(), 503);
    assert_eq!(answer.json::<Value>().await.unwrap()["code"], "away");
    assert!(started.elapsed() < Duration::from_secs(1));
    // a mailbox that does not exist reads the same (nothing tells who has one)
    let unknown = relay.call(&alex, "AAAAAAAAAAAAAAAAAAAAAA", b"x".to_vec()).send().await.unwrap();
    assert_eq!(unknown.status(), 503);
    // not a mailbox at all
    assert_eq!(relay.call(&alex, "nope", b"x".to_vec()).send().await.unwrap().status(), 404);
}

#[tokio::test]
async fn a_call_its_mailbox_does_not_take_in_time_fails_as_away() {
    let relay = Arc::new(harness(quick()).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    // Léa's bridge polled a moment ago (204 after 3 s), then went: Alex's call waits for it at most `pickup`
    assert_eq!(relay.next(&lea).await.0, 204);
    let started = Instant::now();
    let answer = relay.call(&alex, &lea.id, b"x".to_vec()).send().await.unwrap();
    assert_eq!(answer.status(), 503);
    assert!(started.elapsed() >= Duration::from_secs(1) && started.elapsed() < Duration::from_secs(4));
}

#[tokio::test]
async fn nobody_calls_reads_or_answers_without_its_own_mailbox() {
    let relay = Arc::new(harness(quick()).await);
    let (alex, lea, other) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    let anonymous = relay.http.post(format!("{}/v1/family/call/{}", relay.url, lea.id)).body("x").send().await.unwrap();
    assert_eq!(anonymous.status(), 401);
    let forged = Mailbox { id: lea.id.clone(), token: format!("fm1.{}", "B".repeat(90)) };
    assert_eq!(relay.next(&forged).await.0, 401, "only her token reads her mailbox");
    // the push relay's tokens are no mailbox tokens
    let push = Mailbox { id: String::new(), token: "r2.AAAA".into() };
    assert_eq!(relay.next(&push).await.0, 401);
    assert_eq!(relay.call(&alex, &alex.id, b"x".to_vec()).send().await.unwrap().status(), 400, "no call to itself");

    let poller = {
        let relay = relay.clone();
        let lea = Mailbox { id: lea.id.clone(), token: lea.token.clone() };
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(relay.call(&alex, &lea.id, b"for lea".to_vec()).send());
    let (_, id, ..) = poller.await.unwrap();
    // another bridge can't answer a call it did not take
    assert_eq!(relay.answer(&other, &id, b"forged".to_vec()).await, 404);
    assert_eq!(relay.answer(&lea, &id, b"hers".to_vec()).await, 204);
    let answer = call.await.unwrap().unwrap();
    assert_eq!(answer.bytes().await.unwrap().as_ref(), b"hers");
    assert_eq!(relay.answer(&lea, &id, b"again".to_vec()).await, 404, "one answer per call");
}

#[tokio::test]
async fn a_request_too_large_or_too_many_calls_are_refused() {
    let family = FamilyConfig { request_bytes: 1024, calls_per_hour: 3, ..quick() };
    let relay = Arc::new(harness(family).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    let _poll = {
        let relay = relay.clone();
        let lea = Mailbox { id: lea.id.clone(), token: lea.token.clone() };
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let big = relay.call(&alex, &lea.id, vec![7u8; 2048]).send().await.unwrap();
    assert_eq!(big.status(), 413);
    // three calls an hour here (the default: 3,000): the fourth is refused before reaching her mailbox
    for _ in 0..2 {
        let _ = relay.call(&alex, &lea.id, b"x".to_vec()).timeout(Duration::from_millis(300)).send().await;
    }
    let refused = relay.call(&alex, &lea.id, b"x".to_vec()).send().await.unwrap();
    assert_eq!(refused.status(), 429);
    assert_eq!(refused.json::<Value>().await.unwrap()["code"], "quota");
}

#[tokio::test]
async fn an_answer_past_its_size_reaches_the_caller_cut() {
    let family = FamilyConfig { answer_bytes: 1024, ..quick() };
    let relay = Arc::new(harness(family).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    let poller = {
        let relay = relay.clone();
        let lea = Mailbox { id: lea.id.clone(), token: lea.token.clone() };
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(relay.call(&alex, &lea.id, b"x".to_vec()).send());
    let (_, id, ..) = poller.await.unwrap();
    relay.answer(&lea, &id, vec![1u8; 4096]).await;
    // cut before its first bytes (the caller's request fails) or on the way (its body ends in an error)
    let mut got = 0;
    let cut = match call.await.unwrap() {
        Err(_) => true,
        Ok(mut answer) => loop {
            match answer.chunk().await {
                Ok(Some(chunk)) => got += chunk.len(),
                Ok(None) => break false,
                Err(_) => break true,
            }
        },
    };
    assert!(cut && got <= 1024, "the caller sees a cut answer ({got} bytes)");
}

#[tokio::test]
async fn a_blocked_mailbox_is_refused() {
    let key = Sealer::generate_key();
    let relay = harness_with(quick(), Vec::new(), key.clone()).await;
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    let fingerprint: String = {
        use sha2::Digest;
        sha2::Sha256::digest(alex.id.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
    };
    // the same relay restarted with Alex's mailbox blocked (`RELAY_BLOCKED`, by the fingerprint its refusals log)
    let blocked = harness_with(quick(), vec![fingerprint], key).await;
    assert_eq!(blocked.next(&alex).await.0, 403);
    assert_eq!(blocked.call(&alex, &lea.id, b"x".to_vec()).send().await.unwrap().status(), 403);
    let renew = blocked.http.post(format!("{}/v1/family/mailbox", blocked.url)).json(&json!({"token": alex.token}));
    assert_eq!(renew.send().await.unwrap().status(), 403);
    assert_eq!(blocked.next(&lea).await.0, 204, "the others keep theirs");
    let health: Value = relay.http.get(format!("{}/health", relay.url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["family"]["enabled"], true);
}

#[tokio::test]
async fn the_family_relay_can_be_turned_off() {
    let relay = harness(FamilyConfig { enabled: false, ..quick() }).await;
    let answer = relay.http.post(format!("{}/v1/family/mailbox", relay.url)).json(&json!({})).send().await.unwrap();
    assert_eq!(answer.status(), 404);
    let health: Value = relay.http.get(format!("{}/health", relay.url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["family"]["enabled"], false);
}
