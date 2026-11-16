//! Security review, confirmation of R5 (1594a864a): what the relay keeps of a mailbox is the oldest generation it was
//! last told, by whichever statement came last — a poll's `x-bip-passes` or `POST /v1/family/passes`. Nothing orders
//! two statements: one made before a withdrawal that arrives after it (a poll sent a moment before the link was
//! removed, an announce of the minute job computed before it, two connections) lowers the generation again, and the
//! pass withdrawn works until the next statement (a poll: up to 25 s). Fails on 1594a864a; should pass once a
//! statement older than the one kept is ignored (a sequence number the bridge keeps, or the generation never lowered
//! but from « no pass at all »).

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

#[derive(Clone)]
struct Mailbox {
    id: String,
    token: String,
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

/// Léa's bridge polls saying these passes; a call it takes is answered at once (empty, as a refusal). Its status.
fn poll(http: &reqwest::Client, url: &str, lea: &Mailbox, passes: &str) -> tokio::task::JoinHandle<u16> {
    let (http, url, lea, passes) = (http.clone(), url.to_string(), lea.clone(), passes.to_string());
    tokio::spawn(async move {
        let got = http
            .get(format!("{url}/v1/family/next"))
            .bearer_auth(&lea.token)
            .header("x-bip-passes", passes)
            .send()
            .await
            .unwrap();
        let status = got.status().as_u16();
        let call = got.headers().get("x-bip-call").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        if status == 200 {
            let answer = http.post(format!("{url}/v1/family/answer/{call}")).bearer_auth(&lea.token);
            let _ = answer.body(Vec::new()).send().await;
        }
        status
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_statement_made_before_a_withdrawal_never_undoes_it() {
    let family = FamilyConfig {
        pickup: Duration::from_secs(2),
        poll: Duration::from_secs(4),
        away: Duration::from_secs(10),
        answer_start: Duration::from_secs(5),
        ..FamilyConfig::default()
    };
    let relay = Arc::new(Relay::new(config(family), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let (url, http) = (format!("http://{addr}"), reqwest::Client::new());
    let (lea, alex, paul) = (mailbox(&http, &url).await, mailbox(&http, &url).await, mailbox(&http, &url).await);
    let (for_alex, alex_id) = pass(&http, &url, &lea, &alex).await;
    let (_, paul_old) = pass(&http, &url, &lea, &paul).await;
    let before = format!("{alex_id},{paul_old}");

    // Léa's bridge polls, saying both passes
    let first = poll(&http, &url, &lea, &before);
    tokio::time::sleep(Duration::from_millis(300)).await;
    // she removes the link with Alex: Paul gets a newer pass, and the relay is told at once
    let (_, paul_new) = pass(&http, &url, &lea, &paul).await;
    let told = http.post(format!("{url}/v1/family/passes")).bearer_auth(&lea.token);
    assert_eq!(told.json(&json!({"accept": [paul_new]})).send().await.unwrap().status(), 204);
    // a poll she sent a moment before (its header made before the link was removed) arrives now
    let stale = poll(&http, &url, &lea, &before);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let called = http
        .post(format!("{url}/v1/family/call/{}", lea.id))
        .bearer_auth(&alex.token)
        .header("x-bip-pass", &for_alex)
        .body(b"sealed".to_vec())
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map(|r| r.status().as_u16())
        .unwrap_or(0);
    let (a, b) = (first.await.unwrap(), stale.await.unwrap());
    assert_eq!(called, 503, "Alex's withdrawn pass reached Léa's bridge again (polls: {a}, {b}): a stale statement");
}
