//! Security review, confirmation of the R4 fix (6ac2e310d): the fallback of a full room for lists of passes.
//!
//! Lists now keep only ids the relay signed for their mailbox, so filling the 250,000 takes 250,000 genuine passes,
//! each under the quota of 20 an hour per address. That quota counts IPv6 by /64: one /48 (a free tunnel) is 65,536
//! of them, 1.3 million passes an hour; a few hundred IPv4 addresses do it in days, and passes last ten years. Once
//! the room is full, a mailbox that says its list for the first time — every mailbox after the relay restarts, or one
//! whose bridge was off or cut for more than `away` across a sweep — stays « never said » and accepts *every* pass it
//! ever gave, the ones withdrawn included. A household unlinked (N2) reaches the mailbox again: it tells whether the
//! home's computer is on, holds its link's share of the room, sends up to its bytes of the hour.
//!
//! Fails on 6ac2e310d; should pass once a withdrawal holds whatever the room (a per-mailbox generation that passes
//! carry, or withdrawn ids kept apart from the lists and never dropped).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};

fn config(family: FamilyConfig, seal_keys: &str) -> Config {
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
        family,
    }
}

fn family() -> FamilyConfig {
    FamilyConfig {
        mailboxes_per_ip_hour: 1_000_000,
        pickup: Duration::from_secs(2),
        poll: Duration::from_secs(6),
        away: Duration::from_secs(10),
        answer_start: Duration::from_secs(5),
        ..FamilyConfig::default()
    }
}

#[derive(Clone)]
struct Mailbox {
    id: String,
    token: String,
}

/// A relay with these sealing keys (the same keys: the same relay restarted).
async fn relay(seal_keys: &str) -> String {
    let relay = Arc::new(Relay::new(config(family(), seal_keys), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    format!("http://{addr}")
}

async fn mailbox(http: &reqwest::Client, url: &str) -> Mailbox {
    let body: Value =
        http.post(format!("{url}/v1/family/mailbox")).json(&json!({})).send().await.unwrap().json().await.unwrap();
    Mailbox { id: body["mailbox"].as_str().unwrap().into(), token: body["token"].as_str().unwrap().into() }
}

/// A pass `callee` gives `holder`: (pass, its id).
async fn pass(http: &reqwest::Client, url: &str, callee: &Mailbox, holder: &Mailbox) -> (String, String) {
    let made = http.post(format!("{url}/v1/family/pass")).bearer_auth(&callee.token).json(&json!({"for": holder.id}));
    let made: Value = made.send().await.unwrap().json().await.unwrap();
    (made["pass"].as_str().unwrap().to_string(), made["id"].as_str().unwrap().to_string())
}

/// A long poll saying the passes this mailbox accepts: (status, call id).
async fn poll(http: &reqwest::Client, url: &str, mailbox: &Mailbox, passes: &str) -> (u16, String) {
    let got = http
        .get(format!("{url}/v1/family/next"))
        .bearer_auth(&mailbox.token)
        .header("x-bip-passes", passes)
        .send()
        .await
        .unwrap();
    let call = got.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    (got.status().as_u16(), call)
}

/// Léa's bridge polls saying it accepts only Paul's pass (Alex's withdrawn); Alex calls with his: (polled, called).
async fn alex_calls_after_his_pass_was_withdrawn(
    http: &reqwest::Client,
    url: &str,
    lea: &Mailbox,
    alex: &Mailbox,
    for_alex: &str,
    paul_id: &str,
) -> (u16, u16) {
    let poller = {
        let (http, url, lea, paul_id) = (http.clone(), url.to_string(), lea.clone(), paul_id.to_string());
        tokio::spawn(async move {
            let (status, call) = poll(&http, &url, &lea, &paul_id).await;
            if status == 200 {
                let answer = http.post(format!("{url}/v1/family/answer/{call}")).bearer_auth(&lea.token);
                let _ = answer.body(Vec::new()).send().await;
            }
            status
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let called = http
        .post(format!("{url}/v1/family/call/{}", lea.id))
        .bearer_auth(&alex.token)
        .header("x-bip-pass", for_alex)
        .body(b"sealed".to_vec())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map(|r| r.status().as_u16())
        .unwrap_or(0);
    (poller.await.unwrap(), called)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pass_withdrawn_stays_withdrawn_when_the_room_for_lists_is_full() {
    let key = Sealer::generate_key();
    let http = reqwest::Client::new();
    let url = relay(&key).await;
    let (lea, alex, paul) = (mailbox(&http, &url).await, mailbox(&http, &url).await, mailbox(&http, &url).await);
    let (for_alex, _) = pass(&http, &url, &lea, &alex).await;
    let (_, paul_id) = pass(&http, &url, &lea, &paul).await;

    // the control: Léa unlinked Alex, her list says Paul's pass only: Alex's call reads « away »
    let control = alex_calls_after_his_pass_was_withdrawn(&http, &url, &lea, &alex, &for_alex, &paul_id).await;
    assert_eq!(control, (204, 503), "the control: a pass withdrawn is refused");

    // the relay restarts (same keys: tokens and passes still good, every list forgotten)
    let url = relay(&key).await;
    // a stranger's 250 mailboxes, each with 1,000 genuine passes made for itself, poll first and fill the room
    let holder = mailbox(&http, &url).await;
    let mut makers = Vec::new();
    for _ in 0..250 {
        let (http, url, holder) = (http.clone(), url.clone(), holder.clone());
        makers.push(tokio::spawn(async move {
            let theirs = mailbox(&http, &url).await;
            let mut ids = Vec::with_capacity(1000);
            for _ in 0..1000 {
                ids.push(pass(&http, &url, &theirs, &holder).await.1);
            }
            (theirs, ids.join(","))
        }));
    }
    let mut polls = Vec::new();
    for maker in makers {
        let (theirs, ids) = maker.await.unwrap();
        let (http, url) = (http.clone(), url.clone());
        polls.push(tokio::spawn(async move { poll(&http, &url, &theirs, &ids).await }));
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // then Léa's bridge polls again, its list as before: Paul's pass only
    let attacked = alex_calls_after_his_pass_was_withdrawn(&http, &url, &lea, &alex, &for_alex, &paul_id).await;
    for p in &polls {
        p.abort();
    }
    assert_eq!(
        attacked,
        (204, 503),
        "with the room for lists full, Léa's list was not kept and Alex's withdrawn pass reached her bridge again"
    );
}
