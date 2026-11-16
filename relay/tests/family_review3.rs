//! Security review, third pass (d06356b5d): what the fix of N1 leaves. A call's room is now reserved as its bytes
//! come, by doubling (what is allocated, never past the length said), and a body is cut after 5 s without data, but
//! may take 30 s in all. A caller that sends half its length at once then one byte every few seconds holds the whole
//! length for 30 s: eight addresses hold the relay's whole room by sending half of it every 30 s (in production
//! 128 MiB per 30 s, about 35 Mbit/s). Fails on d06356b5d; should pass once a slow body is cut sooner (a minimum
//! rate), or the room reserved is what came.

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

/// « Huit adresses tiennent tout le relais, en envoyant la moitié de la place » : each attacker's call says 1 MiB,
/// sends 512 KiB and one byte (its room doubles to 1 MiB), then one byte every 3 s (never 5 s without data). The eight
/// together hold the whole room (8 MiB here, 256 MiB in production) for 30 s: every other household's call is « busy ».
#[tokio::test]
async fn half_a_body_then_a_trickle_cannot_hold_the_whole_relay() {
    let relay = harness(FamilyConfig { poll: Duration::from_secs(30), away: Duration::from_secs(10), ..small() }).await;
    let (lea, alex) = (relay.mailbox().await, relay.mailbox().await);
    let for_alex = relay.link(&lea, &alex).await;
    let mut pairs = Vec::new();
    for _ in 0..8 {
        let (caller, target) = (relay.mailbox().await, relay.mailbox().await);
        let pass = relay.link(&target, &caller).await;
        pairs.push((caller, target, pass));
    }
    // each attacker's own mailbox polls (and takes nothing: no call of theirs ever ends)
    let polls: Vec<_> = pairs
        .iter()
        .map(|(_, target, _)| {
            let (relay, target) = (relay.clone(), target.clone());
            tokio::spawn(async move { relay.next(&target).await.0 })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut trickles = Vec::new();
    for (i, (caller, target, pass)) in pairs.iter().enumerate() {
        let mut socket = relay.declare_and_stall(caller, target, pass, 1 << 20, &format!("2001:db8:{i}::1")).await;
        let half = vec![0u8; (512 << 10) + 1];
        socket.write_all(&half).await.unwrap();
        trickles.push(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                if socket.write_all(&[0u8]).await.is_err() {
                    break;
                }
            }
        }));
    }
    tokio::time::sleep(Duration::from_secs(6)).await; // past 5 s since the half came: the trickle keeps it all held
    let (polled, called) = relay.call_taken(&alex, &lea, &for_alex).await;
    for t in &trickles {
        t.abort();
    }
    for p in &polls {
        p.abort();
    }
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call to Léa was refused ({called}): eight addresses hold the whole relay, each with half its share sent"
    );
}
