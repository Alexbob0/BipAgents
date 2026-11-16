//! Security review, confirmation of the third-pass fixes (2d7b6f834).
//!
//! - The lists of passes a mailbox accepts are bounded in all (`ACCEPTED_MAX`, 250,000 ids); once full, a list that
//!   is said again keeps only ids it had already. A mailbox whose list is new (one never said, or forgotten by the
//!   sweep after a gap in its polls, or every mailbox after the relay restarts) gets an *empty* list: it then refuses
//!   every call, as if all its links were removed. The ids are not checked (any 32 characters), so 250 mailboxes of a
//!   stranger, each polling with 1,000 made-up ids, fill it, and every household that polls after them is cut off.
//!   Fails on 2d7b6f834.
//! - The minimal body rate, written against the configured rate (whatever it is set to) rather than 128 KiB/s: what
//!   it is there for is that callers sending next to nothing (one byte, then one every few seconds) no longer hold the
//!   room. A control without a minimal rate shows the attack holds; with the default and with 16 KiB/s it must not.
//!   Passes on 2d7b6f834 (and would at 16 KiB/s).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

fn config(family: FamilyConfig) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        seal_keys: Sealer::generate_key(),
        apns: None,
        fcm: None,
        fcm_send_base: None,
        pushes_per_device_hour: 10,
        pushes_per_ip_hour: 10,
        registrations_per_ip_hour: 10,
        // the addresses below come in X-Forwarded-For, as Caddy gives them in production
        trust_forwarded_for: true,
        report_url: None,
        report_secret: None,
        reports_per_ip_hour: 1,
        blocked: Vec::new(),
        guichet_keys: Vec::new(),
        r1_until: 0,
        family,
    }
}

/// 8 MiB held at most (each caller, callee and address an eighth: 1 MiB), calls of 1 MiB at most.
fn small() -> FamilyConfig {
    FamilyConfig {
        request_bytes: 1 << 20,
        queued_bytes: 8 << 20,
        mailboxes_per_ip_hour: 1000,
        pickup: Duration::from_secs(2),
        poll: Duration::from_secs(3),
        away: Duration::from_secs(2),
        answer_start: Duration::from_secs(5),
        ..FamilyConfig::default()
    }
}

#[derive(Clone)]
struct Mailbox {
    id: String,
    token: String,
}

struct Harness {
    url: String,
    addr: SocketAddr,
    http: reqwest::Client,
    passes: Mutex<HashMap<(String, String), String>>,
}

async fn harness(family: FamilyConfig) -> Arc<Harness> {
    let relay = Arc::new(Relay::new(config(family), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Arc::new(Harness { url: format!("http://{addr}"), addr, http: reqwest::Client::new(), passes: Default::default() })
}

impl Harness {
    async fn mailbox(&self) -> Mailbox {
        let answer = self.http.post(format!("{}/v1/family/mailbox", self.url)).json(&json!({})).send().await.unwrap();
        let body: Value = answer.json().await.unwrap();
        Mailbox { id: body["mailbox"].as_str().unwrap().into(), token: body["token"].as_str().unwrap().into() }
    }

    async fn next(&self, mailbox: &Mailbox) -> (u16, String, Vec<u8>) {
        let answer =
            self.http.get(format!("{}/v1/family/next", self.url)).bearer_auth(&mailbox.token).send().await.unwrap();
        let call = answer.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        (answer.status().as_u16(), call, answer.bytes().await.unwrap().to_vec())
    }

    /// `callee` gives `caller` a pass (the step of a link); returns it.
    async fn link(&self, callee: &Mailbox, caller: &Mailbox) -> String {
        // the pass is for the caller's mailbox (« pour qui », added with the fix of N2; assertions unchanged)
        let made = self.http.post(format!("{}/v1/family/pass", self.url)).bearer_auth(&callee.token);
        let made = made.json(&json!({"for": caller.id})).send().await;
        let pass = made.unwrap().json::<Value>().await.unwrap()["pass"].as_str().unwrap().to_string();
        self.passes.lock().unwrap().insert((caller.id.clone(), callee.id.clone()), pass.clone());
        pass
    }

    /// A call that declares `length` bytes, sends none of them, and stays open (from `address`).
    async fn declare_and_stall(
        &self,
        from: &Mailbox,
        to: &Mailbox,
        pass: &str,
        length: usize,
        address: &str,
    ) -> TcpStream {
        let mut socket = TcpStream::connect(self.addr).await.unwrap();
        let head = format!(
            "POST /v1/family/call/{} HTTP/1.1\r\nHost: relay\r\nAuthorization: Bearer {}\r\nx-bip-kind: call\r\n\
             x-bip-pass: {pass}\r\nX-Forwarded-For: {address}\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {length}\r\n\r\n",
            to.id, from.token
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        socket
    }

    /// `from` calls `to` (with `pass`) while `to`'s bridge polls and answers at once: (poll status, caller status).
    async fn call_taken(self: &Arc<Self>, from: &Mailbox, to: &Mailbox, pass: &str) -> (u16, u16) {
        let poller = {
            let (me, to) = (self.clone(), to.clone());
            tokio::spawn(async move {
                let (status, id, _) = me.next(&to).await;
                if status == 200 {
                    let answer = me.http.post(format!("{}/v1/family/answer/{id}", me.url)).bearer_auth(&to.token);
                    let _ = answer.body(Vec::new()).send().await;
                }
                status
            })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        let call = self
            .http
            .post(format!("{}/v1/family/call/{}", self.url, to.id))
            .bearer_auth(&from.token)
            .header("x-bip-pass", pass)
            .header("X-Forwarded-For", "192.0.2.10")
            .body(b"sealed".to_vec())
            .timeout(Duration::from_secs(8))
            .send()
            .await;
        (poller.await.unwrap(), call.map(|r| r.status().as_u16()).unwrap_or(0))
    }
}

impl Harness {
    /// `callee` gives `caller` a pass: (pass, its id).
    async fn link_with_id(&self, callee: &Mailbox, caller: &Mailbox) -> (String, String) {
        let made = self.http.post(format!("{}/v1/family/pass", self.url)).bearer_auth(&callee.token);
        let made: Value = made.json(&json!({"for": caller.id})).send().await.unwrap().json().await.unwrap();
        (made["pass"].as_str().unwrap().to_string(), made["id"].as_str().unwrap().to_string())
    }

    /// One long poll saying the passes this mailbox accepts (`x-bip-passes`), as the bridges do: (status, call id).
    async fn next_saying(&self, mailbox: &Mailbox, passes: &str) -> (u16, String) {
        let answer = self
            .http
            .get(format!("{}/v1/family/next", self.url))
            .bearer_auth(&mailbox.token)
            .header("x-bip-passes", passes)
            .send()
            .await
            .unwrap();
        let call = answer.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        (answer.status().as_u16(), call)
    }
}

/// « Des inconnus remplissent les listes et coupent les foyers » : 251 mailboxes of a stranger (free: 20 an hour per
/// address, and they last 400 days) poll, each saying 1,000 made-up pass ids. Then Léa's bridge polls saying the one
/// pass it gave Alex: its list is new and the whole is full, so it is kept empty, and Alex's call reads « away ».
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strangers_filling_the_lists_of_passes_never_cut_a_household_off() {
    let family = FamilyConfig { mailboxes_per_ip_hour: 100_000, poll: Duration::from_secs(6), ..small() };
    let relay = harness(family).await;
    let made_up: String = (0..1000).map(|i| format!("{i:032}")).collect::<Vec<_>>().join(",");
    let mut theirs = Vec::new();
    for _ in 0..251 {
        let mailbox = relay.mailbox().await;
        let (relay, made_up) = (relay.clone(), made_up.clone());
        theirs.push(tokio::spawn(async move { relay.next_saying(&mailbox, &made_up).await }));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (lea, alex) = (relay.mailbox().await, relay.mailbox().await);
    let (for_alex, id) = relay.link_with_id(&lea, &alex).await;
    let poller = {
        let (relay, lea) = (relay.clone(), lea.clone());
        tokio::spawn(async move {
            let (status, call) = relay.next_saying(&lea, &id).await;
            if status == 200 {
                let answer = relay.http.post(format!("{}/v1/family/answer/{call}", relay.url)).bearer_auth(&lea.token);
                let _ = answer.body(Vec::new()).send().await;
            }
            status
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let called = relay
        .http
        .post(format!("{}/v1/family/call/{}", relay.url, lea.id))
        .bearer_auth(&alex.token)
        .header("x-bip-pass", &for_alex)
        .header("X-Forwarded-For", "192.0.2.10")
        .body(b"sealed".to_vec())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map(|r| r.status().as_u16())
        .unwrap_or(0);
    let polled = poller.await.unwrap();
    for t in &theirs {
        t.abort();
    }
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call to Léa reads « away » ({called}): a stranger's made-up lists filled the room for lists, and Léa's \
         new list was kept empty"
    );
}

/// 8 addresses × 16 calls, each sending one byte then one every 2 s (never 5 s without data): 128 calls × 64 KiB
/// (the least a body reserves) = the whole room (8 MiB here). (polled, called) for Alex's call to Léa 3 s later.
async fn trickle_then_call(body_rate: usize) -> (u16, u16) {
    let family = FamilyConfig { body_rate, poll: Duration::from_secs(8), away: Duration::from_secs(10), ..small() };
    let relay = harness(family).await;
    let (lea, alex) = (relay.mailbox().await, relay.mailbox().await);
    let for_alex = relay.link(&lea, &alex).await;
    let mut held = Vec::new();
    let mut polls = Vec::new();
    for i in 0..8 {
        let (caller, target) = (relay.mailbox().await, relay.mailbox().await);
        let pass = relay.link(&target, &caller).await;
        polls.push({
            let (relay, target) = (relay.clone(), target.clone());
            tokio::spawn(async move { relay.next(&target).await.0 })
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let address = format!("2001:db8:{i}::1");
        for _ in 0..16 {
            let mut socket = relay.declare_and_stall(&caller, &target, &pass, 1 << 20, &address).await;
            socket.write_all(&[0u8]).await.unwrap();
            held.push(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if socket.write_all(&[0u8]).await.is_err() {
                        break;
                    }
                }
            }));
        }
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let outcome = relay.call_taken(&alex, &lea, &for_alex).await;
    for t in &held {
        t.abort();
    }
    for p in &polls {
        p.abort();
    }
    outcome
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn callers_sending_next_to_nothing_hold_no_room_at_the_configured_rate() {
    // the control: with no minimal rate, the trickle holds the whole room (the attack works here)
    assert_eq!(trickle_then_call(0).await, (204, 503), "the control: without a minimal rate, the room is held");
    // the default, whatever it is set to, and 16 KiB/s (the lower default proposed for slow uplinks)
    for rate in [FamilyConfig::default().body_rate, 16 * 1024] {
        assert_eq!(
            trickle_then_call(rate).await,
            (200, 200),
            "callers sending a byte every 2 s still hold the room with a minimal rate of {} KiB/s",
            rate / 1024
        );
    }
}
