//! The core routes (Python: tests/test_http_routes.py's auth, agents and devices parts, tests/test_lan.py's routes,
//! and the FastAPI behaviours every route shares: 401, 404, 405, 422).

mod common;

use axum::extract::{Path, Request};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use common::*;
use serde_json::{json, Value};

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn app() -> (tempfile::TempDir, TestApp) {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    (dir, TestApp::new(config))
}

#[tokio::test]
async fn routes_require_bridge_key() {
    let (_dir, app) = app();
    let variants: Vec<Vec<(&str, String)>> = vec![
        vec![],
        vec![("authorization", "Bearer nope".into())],
        vec![("authorization", BRIDGE_KEY.into())],
        vec![("authorization", format!("Basic {BRIDGE_KEY}"))],
    ];
    for headers in &variants {
        let headers: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        for (method, path) in [
            ("GET", "/v1/outbox"),
            ("POST", "/v1/tts/sentence"),
            ("POST", "/v1/devices"),
            ("DELETE", "/v1/devices/abc"),
            ("POST", "/v1/files"),
            ("GET", "/v1/agents"),
            ("POST", "/v1/watch"),
            ("POST", "/v1/approve"),
            ("GET", "/v1/approvals"),
        ] {
            let resp = app.request(method, path, &headers, None).await;
            assert_eq!(resp.status, StatusCode::UNAUTHORIZED, "{method} {path}");
            assert_eq!(resp.header("www-authenticate").as_deref(), Some("Bearer"));
            assert_eq!(resp.json(), json!({"detail": "unauthorized"}));
        }
    }
}

#[tokio::test]
async fn websocket_requires_bridge_key() {
    let (_dir, app) = app();
    let resp = app.get("/v1/voice", &[("authorization", "Bearer wrong")]).await;
    assert_eq!(resp.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn wrong_keys_block_an_address_for_a_while() {
    let (_dir, app) = app();
    for _ in 0..20 {
        assert_eq!(app.get("/v1/agents", &[("authorization", "Bearer nope")]).await.status, StatusCode::UNAUTHORIZED);
    }
    let (k, v) = auth();
    let resp = app.get("/v1/agents", &[(k, &v)]).await;
    assert_eq!(resp.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(resp.json(), json!({"detail": "too many wrong keys, try again later"}));
}

#[tokio::test]
async fn agents_listing() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let resp = app.get("/v1/agents", &[(k, &v)]).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(
        resp.json(),
        json!({"bridge": {"url": null, "lan": null},
               "agents": [{"id": "wellness", "name": "Wellness", "voice": "4193", "server_voice": null, "category": null,
                           "color": null, "uploads": true, "inbox": true, "url": null, "key": null}],
               "unclaimed": []})
    );
    // FastAPI's JSON: compact, keys in the dict's order.
    assert!(resp
        .text()
        .starts_with("{\"bridge\":{\"url\":null,\"lan\":null},\"agents\":[{\"id\":\"wellness\",\"name\":\"Wellness\""));
}

#[tokio::test]
async fn device_register_update_delete() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = app
        .send_json(
            "POST",
            "/v1/devices",
            &h,
            &json!({"token": TOKEN.to_uppercase(), "environment": "sandbox", "agent_ids": ["Wellness"]}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let body = resp.json();
    assert_eq!(body["token"], TOKEN);
    assert_eq!(body["agent_ids"], json!(["wellness"]));
    let keys: Vec<String> = body.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["token", "environment", "agent_ids", "created_at", "updated_at"]);
    let resp = app
        .send_json("POST", "/v1/devices", &h, &json!({"token": TOKEN, "environment": "production", "agent_ids": []}))
        .await;
    assert_eq!(resp.json()["environment"], "production");
    assert_eq!(resp.json()["agent_ids"], json!([]));
    assert_eq!(app.request("DELETE", &format!("/v1/devices/{TOKEN}"), &h, None).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.request("DELETE", &format!("/v1/devices/{TOKEN}"), &h, None).await.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn device_validation() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(
        app.send_json("POST", "/v1/devices", &h, &json!({"token": "not-hex"})).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.send_json("POST", "/v1/devices", &h, &json!({"token": TOKEN, "environment": "dev"})).await.status,
        StatusCode::BAD_REQUEST
    );
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"token": TOKEN, "agent_ids": ["ghost"]})).await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    assert_eq!(resp.json()["detail"]["agent_ids"], json!(["ghost"]));
    assert_eq!(resp.json()["detail"]["error"], "unknown agent_ids");
    // environment defaults to [apns].environment
    assert_eq!(
        app.send_json("POST", "/v1/devices", &h, &json!({"token": TOKEN})).await.json()["environment"],
        "sandbox"
    );
    // the relay needs [push] service_url; FCM tokens are checked as FCM's
    let resp = app
        .send_json(
            "POST",
            "/v1/devices",
            &h,
            &json!({"token": format!("r1.{}", "x".repeat(40)), "environment": "relay"}),
        )
        .await;
    assert_eq!(
        (resp.status, resp.json()),
        (StatusCode::CONFLICT, json!({"detail": "this bridge has no push relay set up"}))
    );
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"token": "short", "environment": "fcm"})).await;
    assert_eq!(resp.json(), json!({"detail": "invalid FCM token"}));
    let fcm = "fcmTOKEN:abc_def-0123456789";
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"token": fcm, "environment": "fcm"})).await;
    assert_eq!(resp.json()["token"], fcm); // case kept
}

#[tokio::test]
async fn fastapi_like_errors() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    // unknown path: 404 {"detail": "Not Found"}, even behind the key
    let resp = app.get("/v1/nothing-here", &h).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::NOT_FOUND, json!({"detail": "Not Found"})));
    let resp = app.get("/nowhere", &[]).await;
    assert_eq!(resp.json(), json!({"detail": "Not Found"}));
    // wrong method: 405 with FastAPI's detail
    let resp = app.request("PATCH", "/v1/devices", &h, None).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::METHOD_NOT_ALLOWED, json!({"detail": "Method Not Allowed"})));
    assert_eq!(resp.header("allow").as_deref(), Some("POST"));
    // FastAPI's routes answer no HEAD
    let resp = app.request("HEAD", "/v1/agents", &h, None).await;
    assert_eq!((resp.status, resp.header("allow").as_deref()), (StatusCode::METHOD_NOT_ALLOWED, Some("GET")));
    // a missing body, invalid JSON, a missing field, a wrong type, a too long string: 422 with pydantic's list
    let resp = app.request("POST", "/v1/devices", &h, None).await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        resp.json(),
        json!({"detail": [{"type": "missing", "loc": ["body"], "msg": "Field required", "input": null}]})
    );
    let resp =
        app.request("POST", "/v1/devices", &[h[0], ("content-type", "application/json")], Some("{nope".into())).await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp.json()["detail"][0]["type"], "json_invalid");
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"environment": "sandbox"})).await;
    assert_eq!(
        resp.json()["detail"][0],
        json!({"type": "missing", "loc": ["body", "token"], "msg": "Field required", "input": {"environment": "sandbox"}})
    );
    let resp = app.send_json("POST", "/v1/devices", &h, &json!({"token": 5})).await;
    assert_eq!(resp.json()["detail"][0]["type"], "string_type");
    let resp = app
        .send_json("POST", "/v1/approve", &h, &json!({"agent": "wellness", "run_id": "r", "choice": "x".repeat(33)}))
        .await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp.json()["detail"][0]["type"], "string_too_long");
    // query parameters
    let resp = app.get("/v1/outbox?limit=0", &h).await;
    assert_eq!(resp.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(resp.json()["detail"][0]["loc"], json!(["query", "limit"]));
    let resp = app.get("/v1/pages", &h).await;
    assert_eq!(
        resp.json()["detail"][0],
        json!({"type": "missing", "loc": ["query", "agent"], "msg": "Field required", "input": null})
    );
}

#[tokio::test]
async fn outbox_and_cron_jobs() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let store = &app.app.state.store;
    let item = store.add_outbox("wellness", "Bonjour", Default::default()).await.unwrap().unwrap();
    let id = item["id"].as_str().unwrap().to_string();
    let resp = app.get("/v1/outbox?agent=Wellness", &h).await;
    assert_eq!(resp.status, StatusCode::OK);
    let body = resp.json();
    assert_eq!(body["items"][0]["id"], id.as_str());
    assert_eq!(body["items"][0]["has_audio"], false);
    assert_eq!(body["items"][0]["audio_url"], format!("/v1/outbox/{id}/audio"));
    assert!(body["server_time"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        app.get("/v1/outbox?since=yesterday", &h).await.json(),
        json!({"detail": "invalid since (ISO 8601 expected)"})
    );
    assert_eq!(app.get("/v1/outbox?since=2999-01-01T00:00:00Z", &h).await.json()["items"], json!([]));
    assert_eq!(app.get("/v1/outbox?agent=ghost", &h).await.json(), json!({"detail": "unknown agent"}));
    assert_eq!(app.get(&format!("/v1/outbox/{id}"), &h).await.json()["text"], "Bonjour");
    assert_eq!(app.get("/v1/outbox/nope", &h).await.json(), json!({"detail": "not found"}));

    store.note_cron_job("wellness", "j1", Some("Matin")).await.unwrap();
    let jobs = app.get("/v1/cron-jobs?agent=wellness", &h).await.json();
    assert_eq!(jobs["jobs"][0]["notify"], true);
    let resp = app.send_json("PUT", "/v1/cron-jobs/wellness/j1", &h, &json!({"notify": false})).await;
    assert_eq!(resp.json(), json!({"agent": "wellness", "job": "j1", "notify": false}));
    let resp = app.send_json("PUT", "/v1/cron-jobs/wellness/nope", &h, &json!({"notify": "yes"})).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::NOT_FOUND, json!({"detail": "unknown job"})));
}

async fn fake_hermes() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let router = Router::new()
        .route("/v1/capabilities", get(|| async { Json(json!({"ok": true})) }))
        .route(
            "/api/sessions/{id}",
            get(|Path(id): Path<String>, request: Request| async move {
                if request.headers().get("authorization").and_then(|v| v.to_str().ok())
                    != Some(&format!("Bearer {HERMES_KEY}"))
                {
                    return (StatusCode::UNAUTHORIZED, Json(json!({"error": "bad key"}))).into_response();
                }
                Json(json!({"id": id, "title": "Bot Chat", "message_count": 7})).into_response()
            }),
        )
        .route(
            "/v1/runs/{id}/approval",
            post(|Json(body): Json<Value>| async move {
                (StatusCode::CONFLICT, Json(json!({"error": "nothing pending", "got": body})))
            }),
        )
        .route(
            "/v1/runs/run_1/events",
            get(|| async {
                ([("content-type", "text/event-stream")], "data: {\"event\": \"message.delta\", \"delta\": \"Hi\"}\n\n")
            }),
        )
        .fallback(any(move |request: Request| {
            let log = log.clone();
            async move {
                let uri = request.uri().to_string();
                let auth =
                    request.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
                log.lock().unwrap().push(format!("{uri} {auth}"));
                let echo: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                (StatusCode::CREATED, Json(json!({"echo": echo})))
            }
        }));
    (spawn_fake(router).await, seen)
}

#[tokio::test]
async fn session_state_status_and_approvals_through_hermes() {
    let (url, _) = fake_hermes().await;
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls { hermes: url, ..Default::default() }));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = app.get("/v1/sessions/s1/state?agent=wellness", &h).await;
    assert_eq!(resp.json(), json!({"session_id": "s1", "message_count": 7, "updated_at": null}));
    assert!(app.app.state.presence.viewing("wellness", "s1"));
    assert_eq!(
        app.get("/v1/sessions/bad%20id/state?agent=wellness", &h).await.json(),
        json!({"detail": "invalid session_id"})
    );
    assert_eq!(app.get("/v1/sessions/s1/state?agent=ghost", &h).await.json(), json!({"detail": "unknown agent"}));

    let status = app.get("/v1/status", &h).await.json();
    assert_eq!(status, json!({"agents": {"wellness": {"ok": true}}, "model": null, "protected": false}));

    // Hermes' answer is passed on as it is (status and JSON)
    let resp = app
        .send_json("POST", "/v1/approve", &h, &json!({"agent": "wellness", "run_id": "run_9", "choice": " ONCE "}))
        .await;
    assert_eq!(resp.status, StatusCode::CONFLICT);
    assert_eq!(resp.json(), json!({"error": "nothing pending", "got": {"choice": "once"}}));
    let resp = app
        .send_json("POST", "/v1/approve", &h, &json!({"agent": "wellness", "run_id": "bad id", "choice": "once"}))
        .await;
    assert_eq!(resp.json(), json!({"detail": "invalid run_id"}));
}

#[tokio::test]
async fn hermes_down_is_502_and_status_says_why() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = app.get("/v1/sessions/s1/state?agent=wellness", &h).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::BAD_GATEWAY, json!({"detail": "Hermes unreachable"})));
    let resp =
        app.send_json("POST", "/v1/approve", &h, &json!({"agent": "wellness", "run_id": "r1", "choice": "once"})).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::BAD_GATEWAY, json!({"detail": "Hermes unreachable"})));
    let status = app.get("/v1/status", &h).await.json();
    assert_eq!(status["agents"]["wellness"], json!({"ok": false, "error": "ConnectError"}));
}

fn lan_config(dir: &std::path::Path, hermes: &str) -> bipbridge::config::Config {
    let mut dict = config_dict(dir, &Urls { hermes: hermes.into(), ..Default::default() });
    dict["lan"] = json!({"enabled": true, "address": "192.168.1.177", "port": 8650});
    dict["public_url"] = json!("https://box.example.ts.net:8643");
    dict["agents"]["Wellness"]["public_url"] = json!("https://box.example.ts.net:8642");
    bipbridge::config::parse_config(&dict, None).unwrap()
}

#[tokio::test]
async fn pairing_route_and_hermes_relay() {
    let (url, seen) = fake_hermes().await;
    let dir = tempfile::tempdir().unwrap();
    let config = lan_config(dir.path(), &url);
    bipbridge::lan::ensure_certificate(&config).unwrap();
    let print = bipbridge::lan::fingerprint(&config).unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    assert_eq!(app.get("/v1/pairing", &[]).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        app.get("/v1/pairing", &[(k, &v)]).await.json(),
        json!({"lan": {"url": "https://192.168.1.177:8650", "fingerprint": print}})
    );
    // the agents listing gives the key when the agent has a tailnet address
    assert_eq!(app.get("/v1/agents", &[(k, &v)]).await.json()["agents"][0]["key"], HERMES_KEY);

    let hermes_auth = format!("Bearer {HERMES_KEY}");
    let posted = app
        .send_json(
            "POST",
            "/hermes/wellness/v1/runs?x=1",
            &[("authorization", &hermes_auth)],
            &json!({"input": "Salut"}),
        )
        .await;
    assert_eq!(posted.status, StatusCode::CREATED);
    assert_eq!(posted.json(), json!({"echo": {"input": "Salut"}}));
    assert_eq!(seen.lock().unwrap().last().unwrap(), &format!("/v1/runs?x=1 {hermes_auth}")); // Hermes checks its own key

    let events = app.get("/hermes/wellness/v1/runs/run_1/events", &[("authorization", &hermes_auth)]).await;
    assert!(events.header("content-type").unwrap().starts_with("text/event-stream"));
    assert!(events.text().contains("message.delta"));

    assert_eq!(app.get("/hermes/nobody/v1/runs", &[]).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/hermes/wellness/admin", &[]).await.json(), json!({"detail": "not found"}));
    // only Hermes' API paths
}

#[tokio::test]
async fn relay_only_reaches_known_agents_and_hermes_routes() {
    let (_dir, app) = app();
    assert_eq!(app.get("/hermes/nobody/v1/capabilities", &[]).await.status, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/hermes/wellness/etc/passwd", &[]).await.status, StatusCode::NOT_FOUND);
    // Hermes down behind the door: 502
    let resp = app.get("/hermes/wellness/v1/capabilities", &[]).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::BAD_GATEWAY, json!({"detail": "Hermes unreachable"})));
}

#[tokio::test]
async fn static_files_are_served_from_the_package() {
    let (_dir, app) = app();
    let resp = app.get("/console/static/style.css", &[]).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.header("content-type").as_deref(), Some("text/css; charset=utf-8"));
    let resp = app.get("/console/static/nothing.css", &[]).await;
    assert_eq!((resp.status, resp.json()), (StatusCode::NOT_FOUND, json!({"detail": "Not Found"})));
    let resp = app.request("POST", "/console/static/style.css", &[], None).await;
    assert_eq!((resp.status, resp.header("allow")), (StatusCode::METHOD_NOT_ALLOWED, None));
    assert_eq!(app.request("HEAD", "/console/static/style.css", &[], None).await.status, StatusCode::OK);
    assert_eq!(app.get("/desktop/static/novnc/core/rfb.js", &[]).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_cloud_server_has_no_web_console() {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["cloud"] = json!(true);
    let app = TestApp::new(bipbridge::config::parse_config(&dict, None).unwrap());
    assert_eq!(app.get("/console/static/style.css", &[]).await.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn redirect_slashes_like_starlette() {
    let (_dir, app) = app();
    let (k, v) = auth();
    let resp = app.get("/v1/agents/?x=1", &[(k, &v), ("host", "box:8643")]).await;
    assert_eq!(resp.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(resp.header("location").as_deref(), Some("http://box:8643/v1/agents?x=1"));
    // without the key too (the redirect comes before any route), and never to a path no route has
    assert_eq!(app.get("/v1/agents/", &[]).await.status, StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(app.get("/v1/nothing/", &[]).await.status, StatusCode::NOT_FOUND);
    // a path that only matches with another method is still redirected (Starlette's partial match)
    assert_eq!(app.request("GET", "/v1/devices/", &[], None).await.status, StatusCode::TEMPORARY_REDIRECT);
    // nothing ran for the probe: no device was created
    assert!(app.app.state.store.devices_for_agent("wellness").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_route_not_ported_yet_is_a_plain_500() {
    let (_dir, app) = app();
    let (k, v) = auth();
    // /v1/replies/{id} calls PushService::text, a todo!() until push.rs is ported (then this is a 404).
    let resp = app.get("/v1/replies/x", &[(k, &v)]).await;
    assert!(resp.status == StatusCode::INTERNAL_SERVER_ERROR || resp.status == StatusCode::NOT_FOUND);
    if resp.status == StatusCode::INTERNAL_SERVER_ERROR {
        assert_eq!(resp.text(), "Internal Server Error");
        assert_eq!(resp.header("content-type").as_deref(), Some("text/plain; charset=utf-8"));
    }
}

#[tokio::test]
async fn big_bodies_reach_the_routes() {
    let (url, _) = {
        let router = Router::new()
            .route("/v1/runs", post(|body: axum::body::Bytes| async move { Json(json!({"len": body.len()})) }))
            .layer(axum::extract::DefaultBodyLimit::disable());
        (spawn_fake(router).await, ())
    };
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls { hermes: url, ..Default::default() }));
    let big = json!({"input": "x".repeat(5 * 1024 * 1024)});
    let resp = app.send_json("POST", "/hermes/wellness/v1/runs", &[], &big).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert!(resp.json()["len"].as_u64().unwrap() > 5 * 1024 * 1024);
}
