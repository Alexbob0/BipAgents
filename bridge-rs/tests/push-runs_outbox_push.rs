//! Boîte messages: ingestion, audio pre-synthesis, MESSAGE push, outbox routes, APNs client (Python:
//! tests/test_outbox_push.py).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use bipbridge::apns::{ApnsClient, SendOptions};
use bipbridge::config::{parse_config, ApnsConfig};
use bipbridge::outbox::BoiteMessage;
use bipbridge::store::Store;
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DEAD: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

fn boite(id: &str, text: &str, title: Option<&str>, session: Option<&str>) -> BoiteMessage {
    BoiteMessage {
        id: Some(id.into()),
        title: title.map(str::to_string),
        session_id: session.map(str::to_string),
        ..BoiteMessage::new(text)
    }
}

fn ends_with(apns: &FakeApns, token: &str) -> usize {
    apns.requests.lock().unwrap().iter().filter(|r| r.path.ends_with(token)).count()
}

#[tokio::test]
async fn a_message_lands_in_outbox_with_audio_and_push() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let (production, sandbox) = (FakeApns::start().await, FakeApns::start().await);
    let mut data = config_dict(dir.path(), &backend.urls());
    data["apns"]["base_url"] = json!(production.url);
    data["apns"]["sandbox_url"] = json!(sandbox.url);
    let config = parse_config(&data, None).unwrap();
    // Devices registered before the messages flow: created straight in the DB before startup.
    let store = Store::open(&config.outbox.db_path).unwrap();
    store.upsert_device(TOKEN, "sandbox", &["wellness".to_string()]).await.unwrap();
    store.upsert_device(DEAD, "production", &[]).await.unwrap();
    store.close();
    production.responses.lock().unwrap().insert(DEAD.into(), (410, Some("Unregistered".into())));

    let client = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let outbox = client.app.state.outbox.clone();
    let agent = config.agent(Some("wellness")).cloned().unwrap();
    let first = boite("m1", "Bonjour **Sam**, as-tu bien dormi ?", Some("Check-in"), Some("sess_42"));
    assert!(outbox.ingest(&agent, first.clone(), true).await.unwrap().is_some());
    assert!(outbox.ingest(&agent, first, true).await.unwrap().is_none()); // the same key again: stored once
    outbox.ingest(&agent, boite("m2", "Pense à boire de l'eau.", None, None), true).await.unwrap();
    // m1 -> TOKEN + DEAD (410, then deleted), m2 -> TOKEN (+ DEAD if it raced the deletion)
    assert!(wait_until(|| ends_with(&sandbox, TOKEN) >= 2, Duration::from_secs(10)).await);

    let items = client.get("/v1/outbox", &h).await.json()["items"].clone();
    let texts: Vec<&str> = items.as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Bonjour **Sam**, as-tu bien dormi ?", "Pense à boire de l'eau."]);
    let first = items[0].clone();
    assert_eq!(first["agent"], "wellness");
    assert_eq!(first["title"], "Check-in");
    assert_eq!(first["session_id"], "sess_42");
    assert_eq!(first["has_audio"], true);
    assert!(first["created_at"].as_str().unwrap().ends_with('Z'));

    // Audio was pre-synthesized as mp3 from the normalized text.
    let mp3: Vec<Value> =
        backend.with(|s| s.kyutai_inputs.iter().filter(|b| b["response_format"] == "mp3").cloned().collect());
    assert_eq!(mp3[0]["input"], "Bonjour Sam, as-tu bien dormi ?");
    assert_eq!(mp3[0]["voice"], "4193");
    let id = first["id"].as_str().unwrap();
    let audio = client.get(&format!("/v1/outbox/{id}/audio"), &h).await;
    assert_eq!(audio.status, StatusCode::OK);
    assert_eq!(audio.header("content-type").as_deref(), Some("audio/mpeg"));
    assert!(audio.body.starts_with(b"ID3"));

    // Push: MESSAGE payload, sandbox host for the sandbox device, the neutral line (no words in clear).
    let pushed = sandbox.requests.lock().unwrap()[0].clone();
    // the pointer only (docs/conversations.md §9.3); no threads service here: no `c`
    let mut payload = pushed.json();
    let fields = payload.as_object_mut().unwrap();
    assert!(fields.remove("n").unwrap().as_str().unwrap().starts_with("n_"));
    fields.remove("c");
    assert_eq!(
        payload,
        json!({"aps": {"alert": {"loc-key": "notif.boite", "body": "notif.boite"},
                       "thread-id": "wellness", "mutable-content": 1, "category": "MESSAGE", "sound": "default",
                       "badge": 1},
               "v": 2, "t": "boite", "agent": "wellness", "m": format!("o_{id}"), "r": id})
    );
    assert!(String::from_utf8_lossy(&pushed.body).starts_with("{\"aps\":{\"alert\":")); // compact, aps first
    assert_eq!(pushed.header("apns-topic").as_deref(), Some("io.github.bipagents"));
    assert_eq!(pushed.header("apns-push-type").as_deref(), Some("alert"));
    assert_eq!(pushed.path, format!("/3/device/{TOKEN}"));
    // The 410 token was deleted after its first failure.
    let dead_calls = ends_with(&production, DEAD);
    assert!((1..=2).contains(&dead_calls), "{dead_calls}");
    {
        let db = rusqlite::Connection::open(&config.outbox.db_path).unwrap();
        let mut stmt = db.prepare("SELECT token FROM devices").unwrap();
        let tokens: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(tokens, [TOKEN]);
    }

    // Single item + since filtering + agent filter
    let one = client.get(&format!("/v1/outbox/{id}"), &h).await.json();
    assert_eq!(one["text"], first["text"]);
    let since: String =
        url::form_urlencoded::byte_serialize(first["created_at"].as_str().unwrap().as_bytes()).collect();
    let later = client.get(&format!("/v1/outbox?since={since}"), &h).await.json()["items"].clone();
    let texts: Vec<&str> = later.as_array().unwrap().iter().map(|i| i["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Pense à boire de l'eau."]);
    assert_eq!(client.get("/v1/outbox?agent=ghost", &h).await.status, StatusCode::NOT_FOUND);
    assert_eq!(client.get("/v1/outbox?since=hier", &h).await.status, StatusCode::BAD_REQUEST);
    assert_eq!(client.get("/v1/outbox/nope", &h).await.status, StatusCode::NOT_FOUND);
    client.shutdown().await;
    backend.release.set();
}

#[test]
fn push_previews_and_silent_payloads() {
    use bipbridge::push::silent_payload;
    let mut extra = bipbridge::Dict::new();
    extra.insert("run_id".into(), json!("r"));
    extra.insert("status".into(), Value::Null);
    assert_eq!(
        Value::Object(silent_payload("vie", "run_finished", &extra)),
        json!({"aps": {"content-available": 1}, "agent": "vie", "reason": "run_finished", "run_id": "r"})
    );
}

fn jwt_part(token: &str, index: usize) -> Value {
    let part = token.split('.').nth(index).unwrap();
    serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[tokio::test]
async fn apns_jwt_refresh_and_retry() {
    let dir = tempfile::tempdir().unwrap();
    let p8 = write_p8(dir.path());
    let fake = FakeApns::start().await;
    let now = Arc::new(Mutex::new(1_800_000_000.0_f64));
    let clock_now = now.clone();
    let clock: bipbridge::py::Clock = Arc::new(move || *clock_now.lock().unwrap());
    let cfg = ApnsConfig {
        team_id: "TEAM123456".into(),
        key_id: "TESTKEY123".into(),
        p8_path: p8.clone(),
        environment: "production".into(),
        base_url: fake.url.clone(),
        ..Default::default()
    };
    let client = ApnsClient::with_clock(cfg.clone(), reqwest::Client::new(), clock.clone());
    let options = SendOptions { push_type: "background".into(), priority: 5, ..SendOptions::new("production") };
    let res = client.send(&"c".repeat(64), &json!({"aps": {"content-available": 1}}), &options).await;
    assert!(res.ok());
    let req = fake.requests.lock().unwrap()[0].clone();
    assert_eq!(req.header("apns-push-type").as_deref(), Some("background"));
    assert_eq!(req.header("apns-priority").as_deref(), Some("5"));
    let authorization = req.header("authorization").unwrap();
    let token = authorization.split_once(' ').unwrap().1;
    let header = jwt_part(token, 0);
    assert_eq!(header["alg"], "ES256");
    assert_eq!(header["kid"], "TESTKEY123");
    assert_eq!(jwt_part(token, 1), json!({"iss": "TEAM123456", "iat": 1_800_000_000i64}));
    // PyJWT's header: keys sorted
    let raw = String::from_utf8(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(token.split('.').next().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(raw, r#"{"alg":"ES256","kid":"TESTKEY123","typ":"JWT"}"#);

    let first_token = client.provider_token(false).unwrap();
    *now.lock().unwrap() += 49.0 * 60.0;
    assert_eq!(client.provider_token(false).unwrap(), first_token);
    *now.lock().unwrap() += 2.0 * 60.0;
    assert_ne!(client.provider_token(false).unwrap(), first_token); // refreshed before Apple's 60 min limit

    // 403 ExpiredProviderToken -> one retry with a fresh token
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let url = spawn_fake(Router::new().route(
        "/3/device/{token}",
        post(move |request: Request| {
            let seen = seen.clone();
            async move {
                let auth = request.headers().get("authorization").unwrap().to_str().unwrap().to_string();
                let n = {
                    let mut calls = seen.lock().unwrap();
                    calls.push(auth);
                    calls.len()
                };
                if n == 1 {
                    return (StatusCode::FORBIDDEN, Json(json!({"reason": "ExpiredProviderToken"}))).into_response();
                }
                StatusCode::OK.into_response()
            }
        }),
    ))
    .await;
    let retry =
        ApnsClient::with_clock(ApnsConfig { base_url: url, ..cfg.clone() }, reqwest::Client::new(), clock.clone());
    *now.lock().unwrap() += 1.0;
    let res = retry.send(&"c".repeat(64), &json!({"aps": {}}), &SendOptions::new("sandbox")).await;
    assert!(res.ok());
    assert_eq!(calls.lock().unwrap().len(), 2);

    let bad_url = spawn_fake(Router::new().route(
        "/3/device/{token}",
        post(|| async { (StatusCode::BAD_REQUEST, Json(json!({"reason": "BadDeviceToken"}))) }),
    ))
    .await;
    let bad = ApnsClient::new(ApnsConfig { base_url: bad_url, ..cfg }, reqwest::Client::new());
    let result = bad.send(&"c".repeat(64), &json!({"aps": {}}), &SendOptions::new("sandbox")).await;
    assert!(result.token_is_dead());
    assert_eq!(result.reason.as_deref(), Some("BadDeviceToken"));
}
