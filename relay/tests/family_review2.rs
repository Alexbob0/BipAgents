//! Security review, second pass (branch family-relay-anywhere c18c86384): what the fix of the first review leaves.
//! A call's bytes are now reserved before they are read — against the length it *declares*, held until the body comes
//! or 30 s pass. A caller that declares and sends nothing holds that room for free. Each test fails on c18c86384 and
//! should pass once fixed.

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

    fn pass(&self, from: &Mailbox, to: &Mailbox) -> String {
        self.passes.lock().unwrap().get(&(from.id.clone(), to.id.clone())).cloned().unwrap_or_default()
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

/// « Un foyer relié (ou délié) coupe les autres liens d'une boîte » : the callee's share (an eighth of the room,
/// 32 MiB by default) is common to everyone who calls it. Paul — a household Léa linked with, or one she unlinked and
/// that kept its pass (passes are never withdrawn) — declares 2 × 16 MiB and sends nothing, every 30 s: every other
/// household's call to Léa is refused « busy » (their bridges show « ses agents ne répondent pas »). It costs Paul two
/// idle connections, no bytes.
#[tokio::test]
async fn one_household_declaring_and_sending_nothing_cannot_starve_the_others_of_a_mailbox() {
    let relay = harness(small()).await;
    let (lea, alex, paul) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    let (for_alex, for_paul) = (relay.link(&lea, &alex).await, relay.link(&lea, &paul).await);
    // Léa's bridge polls (a poll that just ended: her calls may wait a moment)
    assert_eq!(relay.next(&lea).await.0, 204);
    let _held = relay.declare_and_stall(&paul, &lea, &for_paul, 1 << 20, "198.51.100.7").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (polled, called) = relay.call_taken(&alex, &lea, &for_alex).await;
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call to Léa was refused ({called}) while Paul held her share of the room without sending a byte"
    );
}

/// « Huit adresses tiennent tout le relais » : the room (256 MiB) is reserved on the declared length. Eight
/// addresses (eight /64 in IPv6: one free tunnel gives 65,536), each with its own two mailboxes and a pass it gives
/// itself, declare their share and send nothing: every family call of every household is refused « busy », at no
/// cost in bytes (16 idle connections, renewed every 30 s).
#[tokio::test]
async fn a_few_addresses_declaring_and_sending_nothing_cannot_hold_the_whole_relay() {
    let relay = harness(small()).await;
    let (lea, alex) = (relay.mailbox().await, relay.mailbox().await);
    let for_alex = relay.link(&lea, &alex).await;
    let mut pairs = Vec::new();
    for _ in 0..8 {
        let (caller, target) = (relay.mailbox().await, relay.mailbox().await);
        let pass = relay.link(&target, &caller).await;
        pairs.push((caller, target, pass));
    }
    // each attacker's target polled a moment ago (its calls may wait), all at once
    let polls: Vec<_> = pairs
        .iter()
        .map(|(_, target, _)| {
            let (relay, target) = (relay.clone(), target.clone());
            tokio::spawn(async move { relay.next(&target).await.0 })
        })
        .collect();
    for poll in polls {
        assert_eq!(poll.await.unwrap(), 204);
    }
    let mut held = Vec::new();
    for (i, (caller, target, pass)) in pairs.iter().enumerate() {
        held.push(relay.declare_and_stall(caller, target, pass, 1 << 20, &format!("2001:db8:{i}::1")).await);
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (polled, called) = relay.call_taken(&alex, &lea, &for_alex).await;
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call to Léa was refused ({called}): eight addresses hold the whole relay without sending a byte"
    );
}

/// « Un laissez-passer sert à n'importe qui » : a pass is not bound to the mailbox it was given to. Whoever gets it
/// (a copy of households.json, a backup, the household passing it on) calls Léa from a mailbox of its own: it sees
/// whether her computer is on, holds her share (above), uses up that link's allowance.
#[tokio::test]
async fn a_pass_works_only_for_the_household_it_was_given_to() {
    let relay = harness(small()).await;
    let (lea, alex, mallory) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    relay.link(&lea, &alex).await;
    let alexs = relay.pass(&alex, &lea);
    let (polled, called) = relay.call_taken(&mallory, &lea, &alexs).await;
    assert_eq!((polled, called), (204, 503), "Mallory reached Léa's bridge with the pass Léa gave Alex");
}
