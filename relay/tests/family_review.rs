//! Security review of the family rendezvous (family.rs, branch family-relay-anywhere): what a stranger who knows a
//! household's mailbox id can do to it. The id is in every invitation the household shows (its QR code), and every
//! household that calls it or is called by it learns it (`x-bip-from`); it never changes (a renewal keeps it).
//! Each test below fails on dd94437a8 and should pass once the finding is fixed.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};

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
        trust_forwarded_for: false,
        report_url: None,
        report_secret: None,
        reports_per_ip_hour: 1,
        blocked: Vec::new(),
        guichet_keys: Vec::new(),
        r1_until: 0,
        family,
    }
}

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
    http: reqwest::Client,
    /// The passes given at the links (caller, callee) → pass (added with the fix of « un inconnu coupe une famille »:
    /// only the households a mailbox linked with may call it).
    passes: std::sync::Mutex<std::collections::HashMap<(String, String), String>>,
}

#[derive(Clone)]
struct Mailbox {
    id: String,
    token: String,
}

async fn harness(family: FamilyConfig) -> Arc<Harness> {
    let relay = Arc::new(Relay::new(config(family), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    Arc::new(Harness { url: format!("http://{addr}"), http: reqwest::Client::new(), passes: Default::default() })
}

impl Harness {
    async fn mailbox(&self) -> Mailbox {
        let answer = self.http.post(format!("{}/v1/family/mailbox", self.url)).json(&json!({})).send().await.unwrap();
        let body: Value = answer.json().await.unwrap();
        Mailbox { id: body["mailbox"].as_str().unwrap().into(), token: body["token"].as_str().unwrap().into() }
    }

    /// One long poll: (status, call id, body).
    async fn next(&self, mailbox: &Mailbox) -> (u16, String, Vec<u8>) {
        let answer =
            self.http.get(format!("{}/v1/family/next", self.url)).bearer_auth(&mailbox.token).send().await.unwrap();
        let call = answer.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        (answer.status().as_u16(), call, answer.bytes().await.unwrap().to_vec())
    }

    /// `callee` gives `caller` a pass to call it, as the bridges do when they link.
    async fn link(&self, callee: &Mailbox, caller: &Mailbox) {
        // the pass is for the caller's mailbox (« pour qui », added with the fix of N2; assertions unchanged)
        let made = self.http.post(format!("{}/v1/family/pass", self.url)).bearer_auth(&callee.token);
        let made = made.json(&json!({"for": caller.id})).send().await;
        let pass = made.unwrap().json::<Value>().await.unwrap()["pass"].as_str().unwrap().to_string();
        self.passes.lock().unwrap().insert((caller.id.clone(), callee.id.clone()), pass);
    }

    /// A call, with the caller's pass for that mailbox when it was given one.
    fn call(&self, from: &Mailbox, to: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
        let pass = self.passes.lock().unwrap().get(&(from.id.clone(), to.to_string())).cloned();
        let call = self.http.post(format!("{}/v1/family/call/{to}", self.url)).bearer_auth(&from.token).body(body);
        match pass {
            Some(pass) => call.header("x-bip-pass", pass),
            None => call,
        }
    }

    async fn answer(&self, by: &Mailbox, call: &str, body: Vec<u8>) {
        let sent = self.http.post(format!("{}/v1/family/answer/{call}", self.url)).bearer_auth(&by.token).body(body);
        let _ = sent.send().await;
    }

    /// `from` calls `to` while `to`'s bridge polls; `to` takes it and answers at once (as a bridge refusing a call it
    /// can't open does: an empty answer). (status `to`'s poll got, bytes it took, status the caller got.)
    async fn call_taken(self: &Arc<Self>, from: &Mailbox, to: &Mailbox, body: Vec<u8>) -> (u16, usize, u16) {
        let poller = {
            let (me, to) = (self.clone(), to.clone());
            tokio::spawn(async move {
                let (status, id, body) = me.next(&to).await;
                if status == 200 {
                    me.answer(&to, &id, Vec::new()).await;
                }
                (status, body.len())
            })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        let caller = self.call(from, &to.id, body).timeout(Duration::from_secs(8)).send().await;
        let (status, taken) = poller.await.unwrap();
        (status, taken, caller.map(|r| r.status().as_u16()).unwrap_or(0))
    }
}

/// Finding « un inconnu coupe une famille » (1/3): the per-callee call quota is shared by everyone. A stranger's
/// 3,000 tiny calls an hour (5 here) to Léa's mailbox — even while her bridge is off, the quota is counted before the
/// relay looks whether she polls — and Alex, linked with her, gets 429 for the rest of the hour: their link is down,
/// and the stranger can do it again every hour, for as long as Léa keeps her mailbox (forever).
#[tokio::test]
async fn a_stranger_cannot_use_up_the_calls_a_household_may_receive() {
    let relay = harness(FamilyConfig { calls_per_hour: 5, ..quick() }).await;
    let (lea, alex, stranger) = (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    relay.link(&lea, &alex).await; // Alex and Léa are linked; the stranger only knows her mailbox's id
    for _ in 0..5 {
        let junk = relay.call(&stranger, &lea.id, b"junk".to_vec()).send().await.unwrap();
        assert_eq!(junk.status(), 503, "Léa's bridge is off: away");
    }
    let (polled, _, called) = relay.call_taken(&alex, &lea, b"sealed by alex".to_vec()).await;
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call must reach Léa's bridge: a stranger's calls used up her quota (Alex got {called})"
    );
}

/// Finding « un inconnu coupe une famille » (2/3): the bytes quota counts the caller only, never the callee (the
/// design note says « par boîte 2 Gio/h »). Strangers, each within its own allowance, make Léa's home bridge take
/// as much as they like: 3,000 calls × 16 MiB = 48 GiB an hour into a home connection, which saturates it.
#[tokio::test]
async fn the_bytes_a_mailbox_is_made_to_take_are_capped_as_the_bytes_it_sends() {
    // 8 KiB an hour a mailbox (2 GiB by default)
    let relay = harness(FamilyConfig { kib_per_hour: 8, ..quick() }).await;
    let lea = relay.mailbox().await;
    let mut taken = 0;
    for _ in 0..4 {
        let stranger = relay.mailbox().await; // 20 an hour per address, and they last 400 days
        let (_, bytes, _) = relay.call_taken(&stranger, &lea, vec![0u8; 6 * 1024]).await;
        taken += bytes;
    }
    assert!(taken <= 8 * 1024, "Léa's bridge was made to take {taken} bytes this hour, past the 8 KiB of a mailbox");
}

/// Finding « un inconnu coupe une famille » (3/3): presence. A stranger who knows Léa's mailbox id tells whether her
/// home computer is on, at any time, for as long as she keeps it (the relay's tests say a mailbox that does not exist
/// reads as one that is off: one that is on reads otherwise).
#[tokio::test]
async fn a_stranger_cannot_tell_whether_a_household_is_online() {
    let relay = harness(quick()).await;
    let (lea, stranger) = (relay.mailbox().await, relay.mailbox().await);
    let off = relay.call(&stranger, &lea.id, b"anyone home?".to_vec()).send().await.unwrap().status().as_u16();
    let (_, _, on) = relay.call_taken(&stranger, &lea, b"anyone home?".to_vec()).await;
    assert_eq!(off, on, "the stranger sees {off} while Léa's bridge is off and {on} while it is on");
}

/// Finding « un seul client prend la mémoire du relais » (2/2): the bytes waiting at the relay are capped for every
/// mailbox together (256 MiB), never per caller. One caller (two mailboxes of its own: one calls, the other polled a
/// moment ago and takes nothing) keeps that room full — 16 calls of 16 MiB every 10 s, ~200 Mbit/s — and every
/// family call of every household on the relay is refused « busy » meanwhile (their bridges read it as « ses agents
/// ne répondent pas »).
#[tokio::test]
async fn one_caller_cannot_fill_the_room_every_household_shares() {
    let family = FamilyConfig { queued_bytes: 1024 * 1024, ..quick() }; // 256 MiB by default
    let relay = harness(family).await;
    let (attacker, its_own, lea, alex) =
        (relay.mailbox().await, relay.mailbox().await, relay.mailbox().await, relay.mailbox().await);
    // Alex and Léa are linked; the attacker's own mailbox gives its other one a pass too (it may)
    relay.link(&lea, &alex).await;
    relay.link(&its_own, &attacker).await;
    // its own mailbox polled a moment ago (204 after 3 s) and takes nothing: the calls to it wait (≤ 2 s here)
    assert_eq!(relay.next(&its_own).await.0, 204);
    let filling = {
        let call = relay.call(&attacker, &its_own.id, vec![0u8; 1024 * 1024 - 8]).send();
        tokio::spawn(call)
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    // meanwhile Alex calls Léa, whose bridge polls
    let (polled, _, called) = relay.call_taken(&alex, &lea, b"sealed by alex".to_vec()).await;
    let _ = filling.await;
    assert_eq!(
        (polled, called),
        (200, 200),
        "Alex's call to Léa was refused (got {called}) because another caller filled the relay's waiting room"
    );
}
