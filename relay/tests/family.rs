//! The family rendezvous end to end (family.rs): two bridges that can't reach each other, each holding a long poll
//! to the relay, one calling the other's mailbox and getting its answer streamed back. What passes is sealed by the
//! bridges (bridge-rs family_seal.rs): here it is any bytes, the relay never reads them.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bipagents_relay::config::Config;
use bipagents_relay::family::{fingerprint_key, fingerprint_with, FamilyConfig};
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
    seal_keys: String,
    /// The passes given at the links: (caller, callee) → pass.
    passes: std::sync::Mutex<std::collections::HashMap<(String, String), String>>,
}

async fn harness(family: FamilyConfig) -> Harness {
    harness_with(family, Vec::new(), Sealer::generate_key()).await
}

async fn harness_with(family: FamilyConfig, blocked: Vec<String>, seal_keys: String) -> Harness {
    let relay = Arc::new(Relay::new(config(family, blocked, seal_keys.clone()), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Harness { url: format!("http://{addr}"), addr, http: reqwest::Client::new(), seal_keys, passes: Default::default() }
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

    /// `callee` gives `caller` a pass to call it (what the bridges do at a link).
    async fn link(&self, callee: &Mailbox, caller: &Mailbox) -> String {
        let made = self.http.post(format!("{}/v1/family/pass", self.url)).bearer_auth(&callee.token);
        let made = made.json(&json!({"for": caller.id})).send().await.unwrap();
        assert_eq!(made.status(), 200);
        let pass = made.json::<Value>().await.unwrap()["pass"].as_str().unwrap().to_string();
        self.passes.lock().unwrap().insert((caller.id.clone(), callee.id.clone()), pass.clone());
        pass
    }

    /// A call, with the caller's pass for that mailbox when it has one.
    fn call(&self, from: &Mailbox, to: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        let pass = self.passes.lock().unwrap().get(&(from.id.clone(), to.to_string())).cloned();
        let call = self.http.post(format!("{}/v1/family/call/{to}", self.url)).bearer_auth(&from.token).body(body);
        match pass {
            Some(pass) => call.header("x-bip-pass", pass),
            None => call,
        }
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
    relay.link(&lea, &alex).await;
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
    relay.link(&off, &alex).await;
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
    relay.link(&lea, &alex).await;
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
    // no pass for its own mailbox, and no call to itself
    let own = relay.http.post(format!("{}/v1/family/pass", relay.url)).bearer_auth(&alex.token);
    assert_eq!(own.json(&json!({"for": alex.id})).send().await.unwrap().status(), 400);
    assert_eq!(relay.call(&alex, &alex.id, b"x".to_vec()).send().await.unwrap().status(), 503, "no call to itself");
    relay.link(&lea, &alex).await;

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
    relay.link(&lea, &alex).await;
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
    relay.link(&lea, &alex).await;
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
    // the keyed fingerprint its refusals log (family.rs `Rendezvous::fingerprint`)
    let fingerprint = fingerprint_with(&fingerprint_key(&relay.seal_keys), &alex.id);
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

#[tokio::test]
async fn the_bytes_waiting_at_the_relay_are_capped() {
    // 8,000 bytes held at most, 1,000 for one caller (an eighth)
    let family = FamilyConfig { queued_bytes: 8000, ..quick() };
    let relay = Arc::new(harness(family).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    relay.link(&lea, &alex).await;
    // Léa's bridge polled a moment ago but does not take the calls now: they would wait in memory
    assert_eq!(relay.next(&lea).await.0, 204);
    let waiting = tokio::spawn(relay.call(&alex, &lea.id, vec![1u8; 800]).send());
    tokio::time::sleep(Duration::from_millis(300)).await;
    let refused = relay.call(&alex, &lea.id, vec![2u8; 800]).send().await.unwrap();
    assert_eq!(refused.status(), 503);
    assert_eq!(refused.json::<Value>().await.unwrap()["code"], "busy");
    assert_eq!(waiting.await.unwrap().unwrap().status(), 503, "not taken: away");
    // gone from the queue, its bytes too: room again
    let poller = {
        let relay = relay.clone();
        let lea = Mailbox { id: lea.id.clone(), token: lea.token.clone() };
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(relay.call(&alex, &lea.id, vec![3u8; 800]).send());
    let (status, id, ..) = poller.await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(relay.answer(&lea, &id, b"ok".to_vec()).await, 204);
    assert_eq!(call.await.unwrap().unwrap().bytes().await.unwrap().as_ref(), b"ok");
}

impl Harness {
    /// An invitation's mailbox, until `secs` from now.
    async fn invitation(&self, secs: u64) -> Mailbox {
        let until = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() + secs;
        let made = self.http.post(format!("{}/v1/family/mailbox", self.url)).json(&json!({"invite_until": until}));
        let body: Value = made.send().await.unwrap().json().await.unwrap();
        Mailbox { id: body["mailbox"].as_str().unwrap().into(), token: body["token"].as_str().unwrap().into() }
    }
}

#[tokio::test]
async fn an_invitations_mailbox_takes_an_introduction_without_a_pass_and_nothing_else() {
    let relay = Arc::new(harness(quick()).await);
    let (lea, alex) = (relay.mailbox().await, relay.mailbox().await);
    let invite = relay.invitation(600).await;
    assert_ne!(invite.id, lea.id, "the QR code carries the invitation's mailbox, never the household's");
    let introduce = |to: &str, body: Vec<u8>| relay.call(&alex, to, body).header("x-bip-kind", "invite").send();
    // not polled: away, as anything else
    assert_eq!(introduce(&invite.id, b"hello".to_vec()).await.unwrap().status(), 503);
    let poller = {
        let (relay, invite) = (relay.clone(), Mailbox { id: invite.id.clone(), token: invite.token.clone() });
        tokio::spawn(async move { relay.next(&invite).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(introduce(&invite.id, b"sealed introduction".to_vec()));
    let (status, id, from, kind, body) = poller.await.unwrap();
    assert_eq!(
        (status, from.as_str(), kind.as_str(), body.as_slice()),
        (200, alex.id.as_str(), "invite", &b"sealed introduction"[..])
    );
    assert_eq!(relay.answer(&invite, &id, b"sealed welcome".to_vec()).await, 204);
    assert_eq!(call.await.unwrap().unwrap().bytes().await.unwrap().as_ref(), b"sealed welcome");
    // the household's own mailbox takes no introduction without a pass, even while polled
    let poller = {
        let (relay, lea) = (relay.clone(), Mailbox { id: lea.id.clone(), token: lea.token.clone() });
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(introduce(&lea.id, b"hello".to_vec()).await.unwrap().status(), 503);
    assert_eq!(poller.await.unwrap().0, 204, "nothing reached Léa's bridge");
    // an introduction is small; an invitation's mailbox neither calls nor gives passes
    let poller = {
        let (relay, invite) = (relay.clone(), Mailbox { id: invite.id.clone(), token: invite.token.clone() });
        tokio::spawn(async move { relay.next(&invite).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(introduce(&invite.id, vec![0u8; 65 * 1024]).await.unwrap().status(), 413);
    let _ = poller.await;
    assert_eq!(relay.call(&invite, &lea.id, b"x".to_vec()).send().await.unwrap().status(), 403);
    let pass = relay.http.post(format!("{}/v1/family/pass", relay.url)).bearer_auth(&invite.token).send().await;
    assert_eq!(pass.unwrap().status(), 403);
    // an invitation lasts a week at most
    let too_long = relay
        .http
        .post(format!("{}/v1/family/mailbox", relay.url))
        .json(&json!({"invite_until": 4_000_000_000u64}))
        .send()
        .await
        .unwrap();
    assert_eq!(too_long.status(), 400);
}

#[tokio::test]
async fn a_pass_opens_one_mailbox_only_and_is_renewed_as_the_same() {
    let relay = Arc::new(harness(quick()).await);
    let (lea, alex, paul) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    let pass = relay.link(&lea, &alex).await;
    assert!(pass.starts_with("fp1.") && !pass.contains(&lea.id), "the mailbox is sealed in the pass");
    // Léa's pass does not open Paul's mailbox, polled as it is
    relay.passes.lock().unwrap().insert((alex.id.clone(), paul.id.clone()), pass.clone());
    let poller = {
        let (relay, paul) = (relay.clone(), Mailbox { id: paul.id.clone(), token: paul.token.clone() });
        tokio::spawn(async move { relay.next(&paul).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(relay.call(&alex, &paul.id, b"x".to_vec()).send().await.unwrap().status(), 503);
    assert_eq!(poller.await.unwrap().0, 204);
    // renewed by its holder: refused (a pass withdrawn stays withdrawn); by the mailbox it opens: the same pass id
    let by_holder = relay.http.post(format!("{}/v1/family/pass", relay.url)).bearer_auth(&alex.token);
    assert_eq!(by_holder.json(&json!({"renew": pass})).send().await.unwrap().status(), 401);
    let renewed: Value = relay
        .http
        .post(format!("{}/v1/family/pass", relay.url))
        .bearer_auth(&lea.token)
        .json(&json!({"renew": pass}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let renewed = renewed["pass"].as_str().unwrap().to_string();
    assert_ne!(renewed, pass);
    relay.passes.lock().unwrap().insert((alex.id.clone(), lea.id.clone()), renewed);
    let poller = {
        let (relay, lea) = (relay.clone(), Mailbox { id: lea.id.clone(), token: lea.token.clone() });
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let call = tokio::spawn(relay.call(&alex, &lea.id, b"x".to_vec()).send());
    let (status, id, ..) = poller.await.unwrap();
    assert_eq!(status, 200);
    relay.answer(&lea, &id, b"ok".to_vec()).await;
    assert_eq!(call.await.unwrap().unwrap().status(), 200);
    let forged = relay
        .http
        .post(format!("{}/v1/family/pass", relay.url))
        .bearer_auth(&lea.token)
        .json(&json!({"renew": "fp1.AAAA"}))
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), 401);
}

#[tokio::test]
async fn a_call_says_its_length_and_comes_in_time() {
    let family = FamilyConfig { body_timeout: Duration::from_secs(1), ..quick() };
    let relay = Arc::new(harness(family).await);
    let (alex, lea) = (relay.mailbox().await, relay.mailbox().await);
    let pass = relay.link(&lea, &alex).await;
    let poll = {
        let (relay, lea) = (relay.clone(), Mailbox { id: lea.id.clone(), token: lea.token.clone() });
        tokio::spawn(async move { relay.next(&lea).await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let raw = |head: String, body: &'static [u8]| {
        let addr = relay.addr;
        async move {
            let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            let mut answer = vec![0u8; 256];
            let read = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut answer)).await.unwrap().unwrap();
            String::from_utf8_lossy(&answer[..read]).to_string()
        }
    };
    let common = format!(
        "POST /v1/family/call/{} HTTP/1.1\r\nHost: relay\r\nAuthorization: Bearer {}\r\nx-bip-pass: {pass}\r\n",
        lea.id, alex.token
    );
    let chunked = raw(format!("{common}Transfer-Encoding: chunked\r\n\r\n"), b"5\r\nhello\r\n0\r\n\r\n").await;
    assert!(chunked.starts_with("HTTP/1.1 411"), "{chunked}");
    // says 1,000 bytes, sends 10, then nothing: refused once the body time is over, nothing kept
    let slow = raw(format!("{common}Content-Length: 1000\r\n\r\n"), b"0123456789").await;
    assert!(slow.starts_with("HTTP/1.1 408"), "{slow}");
    poll.abort();
}

#[tokio::test]
async fn a_pass_works_for_its_holder_only_and_stops_when_its_mailbox_withdraws_it() {
    let relay = Arc::new(harness(quick()).await);
    let (lea, alex, paul) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    let for_alex = relay.link(&lea, &alex).await;
    relay.link(&lea, &paul).await;
    // a pass for nobody in particular is not given
    let unbound = relay.http.post(format!("{}/v1/family/pass", relay.url)).bearer_auth(&lea.token);
    assert_eq!(unbound.send().await.unwrap().status(), 400);
    // Léa's bridge polls saying it accepts Paul's pass only (Alex unlinked): Alex is refused as by a bridge off
    let ids: Value = relay
        .http
        .post(format!("{}/v1/family/pass", relay.url))
        .bearer_auth(&lea.token)
        .json(&json!({"for": paul.id}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let paul_pass = ids["pass"].as_str().unwrap().to_string();
    relay.passes.lock().unwrap().insert((paul.id.clone(), lea.id.clone()), paul_pass);
    let poll = |accept: String| {
        let (relay, lea) = (relay.clone(), Mailbox { id: lea.id.clone(), token: lea.token.clone() });
        tokio::spawn(async move {
            let answer = relay
                .http
                .get(format!("{}/v1/family/next", relay.url))
                .bearer_auth(&lea.token)
                .header("x-bip-passes", accept)
                .send()
                .await
                .unwrap();
            let call = answer.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
            (answer.status().as_u16(), call)
        })
    };
    let poller = poll(ids["id"].as_str().unwrap().to_string());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(relay.call(&alex, &lea.id, b"x".to_vec()).send().await.unwrap().status(), 503);
    // Paul's still works
    let call = tokio::spawn(relay.call(&paul, &lea.id, b"x".to_vec()).send());
    let (status, id) = poller.await.unwrap();
    assert_eq!(status, 200);
    relay.answer(&lea, &id, b"ok".to_vec()).await;
    assert_eq!(call.await.unwrap().unwrap().status(), 200);
    // Alex's pass from another mailbox (a copy): refused too
    relay.passes.lock().unwrap().insert((paul.id.clone(), lea.id.clone()), for_alex.clone());
    assert_eq!(relay.call(&paul, &lea.id, b"x".to_vec()).send().await.unwrap().status(), 503);
    // withdrawn at once without a poll: none accepted
    let none = relay
        .http
        .post(format!("{}/v1/family/passes", relay.url))
        .bearer_auth(&lea.token)
        .json(&json!({"accept": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 204);
}
