//! The relay end to end, with Apple and Google played by a local server.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bipagents_relay::apns::ApnsConfig;
use bipagents_relay::config::Config;
use bipagents_relay::fcm::ServiceAccount;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};

#[derive(Default)]
struct Seen {
    apple: Vec<(String, HeaderMap, Value)>,
    google: Vec<(HeaderMap, Value)>,
    google_tokens: usize,
    reports: Vec<(HeaderMap, Value)>,
}

type Shared = Arc<Mutex<Seen>>;

async fn apple(
    State(seen): State<Shared>,
    Path(device): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    seen.lock().unwrap().apple.push((device.clone(), headers, body));
    if device.starts_with("dead") || device.starts_with("deadbeef") {
        return (StatusCode::GONE, Json(json!({"reason": "Unregistered"}))).into_response();
    }
    (StatusCode::OK, [("apns-id", "11111111-2222-3333-4444-555555555555")], "").into_response()
}

async fn google_token(State(seen): State<Shared>) -> Json<Value> {
    seen.lock().unwrap().google_tokens += 1;
    Json(json!({"access_token": "access-1", "expires_in": 3599}))
}

async fn google_send(State(seen): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let device = body["message"]["token"].as_str().unwrap_or_default().to_string();
    seen.lock().unwrap().google.push((headers, body));
    if device.starts_with("dead") {
        let error = json!({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}});
        return (StatusCode::NOT_FOUND, Json(error)).into_response();
    }
    Json(json!({"name": "projects/bip/messages/42"})).into_response()
}

async fn reports(State(seen): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
    seen.lock().unwrap().reports.push((headers, body));
    Json(json!({"ok": true}))
}

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    format!("http://{addr}")
}

struct Harness {
    relay: String,
    seen: Shared,
    http: reqwest::Client,
}

async fn harness(per_device: u32) -> Harness {
    harness_with(per_device, Vec::new(), Sealer::generate_key()).await
}

async fn harness_with(per_device: u32, blocked: Vec<String>, seal_keys: String) -> Harness {
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    let seen: Shared = Arc::default();
    let fake = Router::new()
        .route("/3/device/{device}", post(apple))
        .route("/token", post(google_token))
        .route("/v1/projects/{project}/messages:send", post(google_send))
        .route("/reports", post(reports))
        .with_state(seen.clone());
    let fake = serve(fake).await;
    let fake_reports = fake.clone();
    let apple_key = p256::SecretKey::random(&mut rand::rngs::OsRng).to_pkcs8_pem(LineEnding::LF).unwrap();
    let google_key = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 2048).unwrap();
    let google_key = rsa::pkcs8::EncodePrivateKey::to_pkcs8_pem(&google_key, rsa::pkcs8::LineEnding::LF).unwrap();
    let config = Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        seal_keys,
        apns: Some(ApnsConfig {
            key_pem: apple_key.to_string(),
            key_id: "KEY1234567".into(),
            team_id: "TEAM123456".into(),
            topic: "io.github.bipagents".into(),
            url_override: Some(fake.clone()),
        }),
        fcm: Some(ServiceAccount {
            project_id: "bip".into(),
            client_email: "relay@bip.iam.gserviceaccount.com".into(),
            private_key: google_key.to_string(),
            token_uri: format!("{fake}/token"),
        }),
        fcm_send_base: Some(fake),
        pushes_per_device_hour: per_device,
        pushes_per_ip_hour: 1000,
        registrations_per_ip_hour: 1000,
        trust_forwarded_for: false,
        report_url: Some(format!("{fake_reports}/reports")),
        report_secret: Some("relay-report-secret".into()),
        reports_per_ip_hour: 2,
        blocked,
    };
    let relay = Arc::new(Relay::new(config, reqwest::Client::new()).unwrap());
    Harness { relay: serve(router(relay)).await, seen, http: reqwest::Client::new() }
}

impl Harness {
    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        let response = self.http.post(format!("{}{path}", self.relay)).json(&body).send().await.unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap_or(Value::Null))
    }

    async fn register(&self, platform: &str, token: &str) -> String {
        let (status, body) = self.post("/v1/register", json!({"platform": platform, "token": token})).await;
        assert_eq!(status, 200, "{body}");
        body["relay_token"].as_str().unwrap().to_string()
    }
}

fn preview(agent: &str) -> Value {
    json!({"aps": {"alert": {"title": "Vie", "body": "Ton rendez-vous chez le médecin est à 9h"},
                   "mutable-content": 1, "category": "MESSAGE", "thread-id": agent},
           "agent": agent, "outbox_id": "out-1", "session_id": "s-1"})
}

#[tokio::test]
async fn an_iphone_gets_its_notification_without_the_message_and_its_token_stays_hidden() {
    let h = harness(100).await;
    let device = "ab".repeat(32);
    let relay_token = h.register("apns", &device.to_uppercase()).await;
    assert!(!relay_token.contains(&device), "the device token is sealed");
    let (status, body) = h
        .post(
            "/v1/relay/push",
            json!({"token": relay_token, "environment": "relay", "payload": preview("vie"),
                                       "push_type": "alert", "priority": 10, "collapse_id": "out-1"}),
        )
        .await;
    assert_eq!((status, body["status"].as_u64()), (200, Some(200)));
    assert_eq!(body["apns_id"], "11111111-2222-3333-4444-555555555555");
    let seen = h.seen.lock().unwrap();
    let (to, headers, sent) = &seen.apple[0];
    assert_eq!(to, &device);
    assert_eq!(sent["aps"]["alert"], json!({"title": "Vie", "body": "Nouveau message", "loc-key": "Nouveau message"}));
    assert_eq!(sent["outbox_id"], "out-1");
    assert_eq!(headers["apns-topic"], "io.github.bipagents");
    assert_eq!(headers["apns-push-type"], "alert");
    assert_eq!(headers["apns-priority"], "10");
    assert_eq!(headers["apns-collapse-id"], "out-1");
    assert!(headers["authorization"].to_str().unwrap().starts_with("bearer ey"));
}

#[tokio::test]
async fn dead_and_foreign_tokens_are_told_apart() {
    let h = harness(100).await;
    let dead = h.register("apns", &format!("deadbeef{}", "0".repeat(56))).await;
    let (_, body) = h.post("/v1/relay/push", json!({"token": dead, "payload": preview("vie")})).await;
    assert_eq!((body["status"].as_u64(), body["reason"].as_str()), (Some(410), Some("Unregistered")));
    // Sealed by another relay (or a retired key): dead too, the bridge forgets it and the app registers again.
    let other = Sealer::new(&[[9u8; 32]]).unwrap().seal(&bipagents_relay::seal::Device {
        platform: bipagents_relay::seal::Platform::Apns,
        environment: "production".into(),
        token: "ab".repeat(32),
    });
    let (_, body) = h.post("/v1/relay/push", json!({"token": other, "payload": preview("vie")})).await;
    assert_eq!((body["status"].as_u64(), body["reason"].as_str()), (Some(410), Some("BadRelayToken")));
    // A raw device token is a misconfigured bridge, not a dead device: refused, nothing forgotten.
    let (status, _) = h.post("/v1/relay/push", json!({"token": "ab".repeat(32), "payload": preview("vie")})).await;
    assert_eq!(status, 400);
    assert!(h.seen.lock().unwrap().apple.len() == 1, "only the registered dead device reached Apple");
}

#[tokio::test]
async fn an_android_phone_gets_strings_and_dead_tokens_read_as_410() {
    let h = harness(100).await;
    let relay_token = h.register("fcm", "fcm-token_ABCDEFGHIJKLMNOPQRSTUVWXYZ:0123").await;
    let (_, body) =
        h.post("/v1/relay/push", json!({"token": relay_token, "payload": preview("vie"), "priority": 5})).await;
    assert_eq!(body["status"].as_u64(), Some(200));
    {
        let seen = h.seen.lock().unwrap();
        let (headers, sent) = &seen.google[0];
        assert_eq!(headers["authorization"], "Bearer access-1");
        let message = &sent["message"];
        assert_eq!(message["token"], "fcm-token_ABCDEFGHIJKLMNOPQRSTUVWXYZ:0123");
        assert_eq!(message["data"]["body"], "Nouveau message");
        assert_eq!(message["data"]["agent"], "vie");
        assert_eq!(message["android"]["priority"], "NORMAL");
    }
    let dead = h.register("fcm", "dead-token-ABCDEFGHIJKLMNOPQRSTUVWXYZ").await;
    let (_, body) = h.post("/v1/relay/push", json!({"token": dead, "payload": preview("vie")})).await;
    assert_eq!((body["status"].as_u64(), body["reason"].as_str()), (Some(410), Some("UNREGISTERED")));
    assert_eq!(h.seen.lock().unwrap().google_tokens, 1, "one access token for both sends");
}

#[tokio::test]
async fn quotas_and_validation_hold() {
    let h = harness(2).await;
    let relay_token = h.register("apns", &"cd".repeat(32)).await;
    for _ in 0..2 {
        let (status, _) = h.post("/v1/relay/push", json!({"token": relay_token, "payload": preview("vie")})).await;
        assert_eq!(status, 200);
    }
    let (status, _) = h.post("/v1/relay/push", json!({"token": relay_token, "payload": preview("vie")})).await;
    assert_eq!(status, 429);
    let bad = [
        json!({"platform": "apns", "token": "not-hex"}),
        json!({"platform": "apns", "token": "ab".repeat(32), "environment": "staging"}),
        json!({"platform": "fcm", "token": "short"}),
        json!({"platform": "pigeon", "token": "x"}),
    ];
    for body in bad {
        let (status, _) = h.post("/v1/register", body).await;
        assert!(status == 400 || status == 422, "{status}");
    }
    let (status, _) =
        h.post("/v1/relay/push", json!({"token": relay_token, "payload": preview("vie"), "push_type": "voip"})).await;
    assert_eq!(status, 400);
    let health: Value = h.http.get(format!("{}/health", h.relay)).send().await.unwrap().json().await.unwrap();
    assert_eq!(health, json!({"status": "ok", "apns": true, "fcm": true}));
}

#[tokio::test]
async fn a_device_blocked_for_abuse_gets_nothing() {
    use sha2::{Digest, Sha256};
    let device = "cd".repeat(32);
    let fingerprint: String = Sha256::digest(device.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect();
    let key = Sealer::generate_key();
    let h = harness_with(100, vec![fingerprint], key.clone()).await;
    let (status, body) = h.post("/v1/register", json!({"platform": "apns", "token": device})).await;
    assert_eq!(status, 403, "{body}");
    assert!(body["detail"].as_str().unwrap().contains("support@bipagents.com"));
    // A token sealed before it was blocked: refused too, nothing reaches Apple.
    let sealed = Sealer::from_base64_list(&key).unwrap().seal(&bipagents_relay::seal::Device {
        platform: bipagents_relay::seal::Platform::Apns,
        environment: "production".into(),
        token: device,
    });
    let (status, _) = h.post("/v1/relay/push", json!({"token": sealed, "payload": preview("vie")})).await;
    assert_eq!(status, 403);
    assert!(h.seen.lock().unwrap().apple.is_empty());
    // Others still get theirs.
    let other = h.register("apns", &"ef".repeat(32)).await;
    let (status, _) = h.post("/v1/relay/push", json!({"token": other, "payload": preview("vie")})).await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn a_report_of_a_reply_is_passed_on_with_the_secret_and_kept_nowhere() {
    let h = harness(100).await;
    let report = json!({"reason": "Faux ou dangereux", "comment": "", "text": "Une réponse", "category": "wellness",
                        "app": "1.0 (12)", "model": "iPhone17,1", "platform": "ios", "lang": "fr"});
    let (status, body) = h.post("/v1/report", report.clone()).await;
    assert_eq!(status, 202, "{body}");
    {
        let seen = h.seen.lock().unwrap();
        let (headers, sent) = &seen.reports[0];
        assert_eq!(headers["x-relay-secret"], "relay-report-secret");
        assert_eq!(sent, &report);
    }
    let (status, _) = h.post("/v1/report", json!({"reason": "x", "text": "", "platform": "ios"})).await;
    assert_eq!(status, 400);
    let (status, _) = h.post("/v1/report", json!({"reason": "x", "text": "y", "platform": "windows"})).await;
    assert_eq!(status, 400);
    h.post("/v1/report", report.clone()).await;
    let (status, _) = h.post("/v1/report", report).await;
    assert_eq!(status, 429, "two reports an hour per address in this test");
}
