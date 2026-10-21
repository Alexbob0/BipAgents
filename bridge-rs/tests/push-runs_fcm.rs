//! Android notifications: FCM HTTP v1 with the Firebase service account, a data message the app turns into a
//! notification, dead tokens dropped, and Android devices registered next to iPhones (Python: tests/test_fcm.py).

/// A throwaway RSA key made at each run (Python makes a fresh one too; ring cannot generate RSA, the rsa crate can).
static TEST_RSA_KEY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    use rsa::pkcs8::EncodePrivateKey;
    let key = rsa::RsaPrivateKey::new(&mut rand::thread_rng(), 2048).expect("rsa key");
    key.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).expect("pem").to_string()
});

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bipbridge::apns::{ApnsClient, SendOptions};
use bipbridge::config::SharedConfig;
use bipbridge::fcm::{data_message, FcmClient};
use bipbridge::push::{approval_payload, reply_payload, PushService, Sender};
use bipbridge::store::Store;
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

fn account(dir: &std::path::Path) -> String {
    let path = dir.join("fcm.json");
    let body = json!({"project_id": "bipagents-test", "client_email": "push@bipagents-test.iam.gserviceaccount.com",
                      "private_key": &*TEST_RSA_KEY, "token_uri": "https://oauth2.googleapis.com/token"});
    std::fs::write(&path, serde_json::to_string(&body).unwrap()).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn an_apns_payload_becomes_fcm_data() {
    let choices = [json!("once"), json!("deny")];
    let payload = approval_payload("wellness", "Wellness", "run_1", Some("req_1"), Some(&choices), None, Some("s1"));
    let data = data_message(&Value::Object(payload));
    assert_eq!(data["category"], "APPROVAL");
    assert_eq!(data["agent"], "wellness");
    assert_eq!(data["run_id"], "run_1");
    assert_eq!(data["choices"], r#"["once", "deny"]"#);
    assert_eq!(data["session_id"], "s1");
    assert_eq!(data["title"], "Wellness");
    assert!(data.values().all(Value::is_string));
    let reply = data_message(&Value::Object(reply_payload(
        "vie",
        "Vie",
        "run_2",
        None,
        None,
        Some("abc"),
        "Ta réponse est prête.",
    )));
    assert_eq!(reply["kind"], "reply");
    assert_eq!(reply["reply_id"], "abc");
}

#[derive(Clone, Default)]
struct Google {
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn google(axum::extract::State(g): axum::extract::State<Google>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let seen = Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: None,
        headers: parts.headers.clone(),
        body: body.clone(),
    };
    g.seen.lock().unwrap().push(seen.clone());
    if seen.path == "/token" {
        assert!(String::from_utf8_lossy(&body).contains("jwt-bearer"));
        return Json(json!({"access_token": "ya29.test", "expires_in": 3600})).into_response();
    }
    assert_eq!(seen.header("authorization").as_deref(), Some("Bearer ya29.test"));
    let message = seen.json()["message"].clone();
    if message["token"] == "dead:token-0123456789" {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}})),
        )
            .into_response();
    }
    Json(json!({"name": "projects/bipagents-test/messages/1"})).into_response()
}

async fn start_google() -> (Google, String) {
    let g = Google::default();
    let url = spawn_fake(Router::new().fallback(post(google)).with_state(g.clone())).await;
    (g, url)
}

#[tokio::test]
async fn sending_exchanges_the_key_for_a_token_and_drops_dead_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let (g, url) = start_google().await;
    let fcm =
        FcmClient::new(&account(dir.path()), reqwest::Client::new(), &url, Some(&format!("{url}/token"))).unwrap();
    let payload = Value::Object(reply_payload("vie", "Vie", "run_2", None, None, None, "Ta réponse est prête."));
    let ok = fcm.send("live:token-0123456789", &payload, &SendOptions::new("fcm")).await;
    let dead = fcm.send("dead:token-0123456789", &payload, &SendOptions::new("fcm")).await;
    assert!(ok.ok());
    assert_eq!(ok.apns_id.as_deref(), Some("projects/bipagents-test/messages/1"));
    assert!(dead.token_is_dead());
    assert_eq!((dead.status, dead.reason.as_deref()), (410, Some("UNREGISTERED")));
    let seen = g.seen.lock().unwrap().clone();
    assert_eq!(seen.iter().filter(|r| r.path == "/token").count(), 1); // the access token is reused
    assert_eq!(seen[1].path, "/v1/projects/bipagents-test/messages:send");
    let message = seen[1].json()["message"].clone();
    assert_eq!(message["android"]["priority"], "HIGH");
    assert_eq!(message["data"]["category"], "MESSAGE");
    // the grant: a form with the RS256 assertion signed by the service account
    let form = String::from_utf8_lossy(&seen[0].body).into_owned();
    let assertion =
        url::form_urlencoded::parse(form.as_bytes()).find(|(k, _)| k == "assertion").unwrap().1.into_owned();
    use base64::Engine;
    let part = |i: usize| -> Value {
        serde_json::from_slice(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(assertion.split('.').nth(i).unwrap()).unwrap(),
        )
        .unwrap()
    };
    assert_eq!(part(0), json!({"alg": "RS256", "typ": "JWT"}));
    let claims = part(1);
    assert_eq!(claims["iss"], "push@bipagents-test.iam.gserviceaccount.com");
    assert_eq!(claims["scope"], "https://www.googleapis.com/auth/firebase.messaging");
    assert_eq!(claims["aud"], format!("{url}/token"));
    assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), 3600);
}

#[tokio::test]
async fn an_android_device_registers_with_its_token_as_is() {
    let dir = tempfile::tempdir().unwrap();
    let client = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let token = format!("eXaMpLe:APA91bH_{}", "x".repeat(140));
    let resp = client
        .send_json("POST", "/v1/devices", &h, &json!({"token": token, "environment": "fcm", "agent_ids": ["wellness"]}))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json()["token"], json!(token)); // FCM tokens are case-sensitive
    let bad = client.send_json("POST", "/v1/devices", &h, &json!({"token": "bad token!", "environment": "fcm"})).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let other =
        client.send_json("POST", "/v1/devices", &h, &json!({"token": "ab".repeat(32), "environment": "other"})).await;
    assert_eq!(other.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn each_device_gets_its_own_service() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let store = Store::open(&dir.path().join("b.db").to_string_lossy()).unwrap();
    store.upsert_device(&"ab".repeat(32), "production", &["wellness".to_string()]).await.unwrap();
    store.upsert_device("x:fcm-token-0123456789", "fcm", &["wellness".to_string()]).await.unwrap();
    let apple = FakeApns::start().await;
    let (g, url) = start_google().await;
    let mut apns_config = config.apns.clone();
    apns_config.base_url = apple.url.clone();
    let apns = Sender::Apns(Arc::new(ApnsClient::new(apns_config, reqwest::Client::new())));
    let fcm = Sender::Fcm(Arc::new(
        FcmClient::new(&account(dir.path()), reqwest::Client::new(), &url, Some(&format!("{url}/token"))).unwrap(),
    ));
    let shared = SharedConfig::new(config);
    let push = PushService::new(shared.clone(), store.clone(), Some(apns.clone()), Some(fcm), None);
    let mut extra = bipbridge::Dict::new();
    extra.insert("run_id".into(), json!("r"));
    assert_eq!(push.notify_silent("wellness", "test", extra.clone()).await, 2);
    let paths: Vec<String> = apple.requests.lock().unwrap().iter().map(|r| r.path.clone()).collect();
    assert_eq!(paths, [format!("/3/device/{}", "ab".repeat(32))]);
    let sent: Vec<Value> =
        g.seen.lock().unwrap().iter().filter(|r| r.path != "/token").map(|r| r.json()["message"].clone()).collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["token"], "x:fcm-token-0123456789");
    // a silent push: NORMAL priority, data only
    assert_eq!(sent[0]["android"], json!({"priority": "NORMAL"}));
    assert_eq!(sent[0]["data"], json!({"silent": "1", "agent": "wellness", "reason": "test", "run_id": "r"}));
    // No Android sender: iPhones still get theirs.
    let alone = PushService::new(shared, store.clone(), Some(apns), None, None);
    assert_eq!(alone.notify_silent("wellness", "test", extra).await, 1);
    store.close();
}
