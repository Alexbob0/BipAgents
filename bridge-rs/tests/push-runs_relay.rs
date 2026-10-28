//! The push relay: the route this bridge offers other installs, the client a member install sends through, and the
//! publisher's relay for devices registered there (Python: the relay parts of tests/test_relay_and_discovery.py).

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use bipbridge::apns::SendOptions;
use bipbridge::app::Services;
use bipbridge::config::parse_config;
use bipbridge::relay::RelayClient;
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

const MEMBER_KEY: &str = "member-relay-key-0123456789abcdef012345";

fn token() -> String {
    "ab".repeat(32)
}

#[tokio::test]
async fn relay_forwards_to_apns_with_the_admin_key() {
    let dir = tempfile::tempdir().unwrap();
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &Urls::default());
    data["relay"] = json!({"clients": {"chi": MEMBER_KEY}, "per_hour": 3});
    data["apns"]["base_url"] = json!(apns.url);
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let push = json!({"token": token(), "environment": "sandbox", "payload": {"aps": {"alert": {"title": "Vie", "body": "Prêt"}}}});
    assert_eq!(app.send_json("POST", "/v1/relay/push", &[], &push).await.status, StatusCode::UNAUTHORIZED);
    let (k, v) = auth();
    // the bridge key is not a relay key
    let resp = app.send_json("POST", "/v1/relay/push", &[(k, v.as_str())], &push).await;
    assert_eq!(resp.status, StatusCode::UNAUTHORIZED);
    assert_eq!(resp.json(), json!({"detail": "unauthorized"}));
    let member_auth = format!("Bearer {MEMBER_KEY}");
    let member = [("authorization", member_auth.as_str())];
    let resp = app.send_json("POST", "/v1/relay/push", &member, &push).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json(), json!({"status": 200, "reason": null, "apns_id": "abc"}));
    assert_eq!(apns.payloads().last().unwrap()["aps"]["alert"]["body"], "Prêt");
    assert!(apns.requests.lock().unwrap().last().unwrap().path.ends_with(&token()));
    let mut bad = push.clone();
    bad["token"] = json!("not-hex");
    let resp = app.send_json("POST", "/v1/relay/push", &member, &bad).await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    assert_eq!(resp.json(), json!({"detail": "invalid push"}));
    let mut big = push.clone();
    big["payload"] = json!({"aps": {}, "blob": "x".repeat(5000)});
    let resp = app.send_json("POST", "/v1/relay/push", &member, &big).await;
    assert_eq!(resp.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(resp.json(), json!({"detail": "payload too large"}));
    for _ in 0..2 {
        app.send_json("POST", "/v1/relay/push", &member, &push).await;
    }
    let resp = app.send_json("POST", "/v1/relay/push", &member, &push).await;
    assert_eq!(resp.status, StatusCode::TOO_MANY_REQUESTS); // 3 per hour here
    assert_eq!(resp.json(), json!({"detail": "quota exceeded"}));
}

#[tokio::test]
async fn relay_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let push = json!({"token": token(), "environment": "sandbox", "payload": {"aps": {}}});
    let member_auth = format!("Bearer {MEMBER_KEY}");
    let resp = app.send_json("POST", "/v1/relay/push", &[("authorization", member_auth.as_str())], &push).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
    assert_eq!(resp.json(), json!({"detail": "not found"}));
}

#[tokio::test]
async fn relay_client_reports_the_relays_answer() {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    let url = spawn_fake(Router::new().route(
        "/v1/relay/push",
        post(move |request: Request| {
            let record = record.clone();
            async move {
                let (parts, body) = request.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let data: Value = serde_json::from_slice(&body).unwrap();
                record.lock().unwrap().push(Seen {
                    method: parts.method.to_string(),
                    path: parts.uri.path().to_string(),
                    query: None,
                    headers: parts.headers,
                    body,
                });
                if data["token"] == "dead" {
                    return Json(json!({"status": 410, "reason": "Unregistered"})).into_response();
                }
                Json(json!({"status": 200, "apns_id": "x"})).into_response()
            }
        }),
    ))
    .await;
    let sender = RelayClient::new(&format!("{url}/"), MEMBER_KEY, reqwest::Client::new());
    let options = SendOptions { collapse_id: Some("c1".into()), ..SendOptions::new("production") };
    let ok = sender.send(&token(), &json!({"aps": {}}), &options).await;
    let dead = sender.send("dead", &json!({"aps": {}}), &SendOptions::new("production")).await;
    assert!(ok.ok());
    assert_eq!(ok.apns_id.as_deref(), Some("x"));
    assert!(dead.token_is_dead());
    let seen = seen.lock().unwrap();
    assert_eq!(seen[0].path, "/v1/relay/push");
    assert_eq!(seen[0].header("authorization"), Some(format!("Bearer {MEMBER_KEY}")));
    let body = seen[0].json();
    assert_eq!(body["collapse_id"], "c1");
    assert_eq!(
        body,
        json!({"token": token(), "environment": "production", "payload": {"aps": {}}, "push_type": "alert", "priority": 10,
               "collapse_id": "c1"})
    );
}

#[tokio::test]
async fn relay_unreachable_is_a_soft_failure() {
    let sender = RelayClient::new("http://127.0.0.1:9", MEMBER_KEY, reqwest::Client::new());
    let result = sender.send(&token(), &json!({}), &SendOptions::new("sandbox")).await;
    assert!(!result.ok() && !result.token_is_dead()); // the device stays registered
}

#[tokio::test]
async fn member_install_sends_through_the_relay() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["apns"] = json!({});
    data["push"] = json!({"relay_url": "https://admin.example.ts.net:8643", "relay_key": MEMBER_KEY});
    let services = Services::new(parse_config(&data, None).unwrap()).unwrap();
    assert_eq!(services.push_mode, Some("relay"));
    assert!(services.apns.is_none());
    services.store.close();
}

#[test]
fn relay_config_checks() {
    let dir = tempfile::tempdir().unwrap();
    let clients = dir.path().join("relay-clients");
    std::fs::write(&clients, format!("# one install per line\nchi {MEMBER_KEY}\n")).unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["relay"] = json!({"clients_file": clients.to_string_lossy()});
    let config = parse_config(&data, None).unwrap();
    assert_eq!(config.relay.clients.get(MEMBER_KEY).map(String::as_str), Some("chi"));
    assert_eq!(config.relay.clients.len(), 1);
    data["relay"] = json!({"clients": {"chi": "short"}});
    let err = parse_config(&data, None).err().unwrap();
    assert!(err.to_string().contains("32 characters"), "{err}");
    data["relay"] = json!({});
    data["push"] = json!({"relay_url": "https://admin:8643"});
    let err = parse_config(&data, None).err().unwrap();
    assert!(err.to_string().contains("relay_key"), "{err}");
}

#[tokio::test]
async fn devices_registered_with_the_publishers_relay_go_through_it() {
    let dir = tempfile::tempdir().unwrap();
    let sent: Arc<Mutex<Vec<Seen>>> = Arc::new(Mutex::new(Vec::new()));
    let record = sent.clone();
    let url = spawn_fake(Router::new().fallback(post(move |request: Request| {
        let record = record.clone();
        async move {
            let (parts, body) = request.into_parts();
            let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            let data: Value = serde_json::from_slice(&body).unwrap();
            record.lock().unwrap().push(Seen {
                method: parts.method.to_string(),
                path: parts.uri.path().to_string(),
                query: None,
                headers: parts.headers,
                body,
            });
            let token = data["token"].as_str().unwrap();
            // Apple's « gone », or the relay's own: a token past its 90 days
            let reason = if token.ends_with("dead") {
                json!("Unregistered")
            } else if token.ends_with("ended") {
                json!("ExpiredRelayToken")
            } else {
                Value::Null
            };
            Json(json!({"status": if reason.is_null() { 200 } else { 410 }, "reason": reason}))
        }
    })))
    .await;
    let mut data = config_dict(dir.path(), &Urls::default());
    data["push"] = json!({"service_url": format!("{url}/")});
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let services = app.app.state.clone();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let live = format!("r2.{}", "Ab_-".repeat(15));
    let dead = format!("r2.{}dead", "x".repeat(56));
    let ended = format!("r2.{}ended", "y".repeat(56));
    assert_eq!(app.get("/health", &[]).await.json()["push_relay"], true);
    // A token of the relay's first format (no end): the relay refuses it, so this bridge does too.
    let first = app
        .send_json(
            "POST",
            "/v1/devices",
            &h,
            &json!({"token": format!("r1.{}", "A".repeat(60)), "environment": "relay"}),
        )
        .await;
    assert_eq!(first.status, StatusCode::BAD_REQUEST);
    for token in [&live, &dead, &ended] {
        let device = app
            .send_json(
                "POST",
                "/v1/devices",
                &h,
                &json!({"token": token, "environment": "relay", "agent_ids": ["wellness"]}),
            )
            .await
            .json();
        assert_eq!(device["token"], json!(token)); // kept as is: case-sensitive
        assert_eq!(device["environment"], "relay");
    }
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"token": token(), "environment": "relay"})).await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    assert_eq!(services.push.notify_message("wellness", "out-1", "Le texte du message", None, None).await, 1);
    {
        let sent = sent.lock().unwrap();
        let mut tokens: Vec<String> = sent.iter().map(|r| r.json()["token"].as_str().unwrap().to_string()).collect();
        tokens.sort();
        let mut expected = vec![live.clone(), dead.clone(), ended.clone()];
        expected.sort();
        assert_eq!(tokens, expected);
        assert!(sent.iter().all(|r| r.path == "/v1/relay/push" && r.header("authorization").is_none()));
    }
    // The relay said the second device is gone, and the third's token ended: both forgotten here (the app registers
    // again with a new token).
    assert!(services.store.get_device(&dead).await.unwrap().is_none());
    assert!(services.store.get_device(&ended).await.unwrap().is_none());
    assert!(services.store.get_device(&live).await.unwrap().is_some());
    // The app takes it back: its relay token, as it is.
    assert_eq!(app.request("DELETE", &format!("/v1/devices/{live}"), &h, None).await.status, StatusCode::NO_CONTENT);
    assert!(services.store.get_device(&live).await.unwrap().is_none());
}

#[tokio::test]
async fn a_bridge_without_the_relay_refuses_relay_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    assert_eq!(app.get("/health", &[]).await.json()["push_relay"], false);
    let (k, v) = auth();
    let resp = app
        .send_json(
            "POST",
            "/v1/devices",
            &[(k, v.as_str())],
            &json!({"token": format!("r1.{}", "A".repeat(60)), "environment": "relay"}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::CONFLICT);
}
