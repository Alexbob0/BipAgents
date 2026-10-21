//! HermesClient against a fake Hermes api_server: the calls, their bodies and headers, the errors, and the run
//! events' SSE stream read in odd chunks.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Query, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use bipbridge::config::AgentConfig;
use bipbridge::hermes::{HermesClient, HermesError};
use common::*;
use futures::StreamExt;
use serde_json::{json, Value};

type Seen = Arc<Mutex<Vec<(String, String, Value, String)>>>;

async fn record(seen: &Seen, request: Request) -> Value {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let auth = format!(
        "{} | {}",
        request.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or(""),
        request.headers().get("accept").and_then(|v| v.to_str().ok()).unwrap_or("")
    );
    let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    seen.lock().unwrap().push((method, uri, value.clone(), auth));
    value
}

async fn fake() -> (String, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let router = Router::new()
        .route("/v1/capabilities", get(|| async { Json(json!({"runs": true})) }))
        .route("/v1/runs/{id}", get(|Path(id): Path<String>| async move { Json(json!({"run_id": id, "status": "running"})) }))
        .route(
            "/api/sessions",
            get(|Query(q): Query<std::collections::HashMap<String, String>>| async move {
                Json(json!({"sessions": [{"id": "a", "limit": q.get("limit")}, "not a dict", {"id": "b"}]}))
            })
            .post({
                let s = s.clone();
                move |request: Request| {
                    let s = s.clone();
                    async move {
                        record(&s, request).await;
                        (StatusCode::CREATED, Json(json!({"session": {"id": "new-1"}})))
                    }
                }
            }),
        )
        .route("/api/sessions/wrapped", get(|| async { Json(json!({"session": {"id": "wrapped", "message_count": 3}})) }))
        .route("/api/sessions/plain", get(|| async { Json(json!({"id": "plain"})) }))
        .route("/api/sessions/text", get(|| async { "not json" }))
        .route(
            "/api/sessions/{id}/messages",
            get(|Query(q): Query<std::collections::HashMap<String, String>>| async move {
                Json(json!([{"role": "user", "inline": q.get("inline_images")}, 5]))
            }),
        )
        .route(
            "/api/sessions/{id}/chat",
            post({
                let s = s.clone();
                move |request: Request| {
                    let s = s.clone();
                    async move {
                        let body = record(&s, request).await;
                        Json(json!({"message": {"role": "assistant", "content": format!("echo: {}", body["message"].as_str().unwrap_or(""))}}))
                    }
                }
            }),
        )
        .route(
            "/api/sessions/{id}",
            patch({
                let s = s.clone();
                move |request: Request| {
                    let s = s.clone();
                    async move {
                        record(&s, request).await;
                        StatusCode::NO_CONTENT
                    }
                }
            })
            .delete(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "x".repeat(600)) }),
        )
        .route(
            "/v1/runs",
            post({
                let s = s.clone();
                move |request: Request| {
                    let s = s.clone();
                    async move {
                        record(&s, request).await;
                        (StatusCode::ACCEPTED, Json(json!({"run_id": "run_7", "status": "started"})))
                    }
                }
            }),
        )
        .route("/v1/runs/{id}/stop", post(|| async { Json(json!({"stopped": true})) }))
        .route(
            "/v1/runs/{id}/approval",
            post({
                let s = s.clone();
                move |request: Request| {
                    let s = s.clone();
                    async move {
                        record(&s, request).await;
                        (StatusCode::CONFLICT, "plain text refusal").into_response()
                    }
                }
            }),
        )
        .route(
            "/v1/runs/{id}/events",
            get({
                let s = s.clone();
                move |Path(id): Path<String>, request: Request| {
                    let s = s.clone();
                    async move {
                        record(&s, request).await;
                        if id == "missing" {
                            return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response();
                        }
                        // the SSE text cut at awkward places (inside a line, between \r and \n, inside a UTF-8 char)
                        let text = ": keepalive\r\n\r\nevent: message.delta\r\ndata: {\"delta\": \"Bonjour é\"}\r\n\r\ndata: {\"type\": \"run.completed\", \"output\": \"Fini\"}\n\n";
                        let bytes = text.as_bytes().to_vec();
                        let cuts = [5, 13, 14, 40, 53, 54, bytes.len()];
                        let mut chunks = Vec::new();
                        let mut start = 0;
                        for end in cuts {
                            chunks.push(Ok::<_, std::io::Error>(bytes[start..end].to_vec()));
                            start = end;
                        }
                        let stream = futures::stream::iter(chunks).then(|c| async move {
                            tokio::time::sleep(Duration::from_millis(5)).await;
                            c
                        });
                        Response::builder().header("content-type", "text/event-stream").body(Body::from_stream(stream)).unwrap()
                    }
                }
            }),
        );
    (spawn_fake(router).await, seen)
}

fn agent(url: &str) -> AgentConfig {
    AgentConfig {
        name: "lea".into(),
        display_name: "Lea".into(),
        hermes_url: url.into(),
        hermes_key: HERMES_KEY.into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn calls_and_answers() {
    let (url, seen) = fake().await;
    let hermes = HermesClient::new(reqwest::Client::new());
    let a = agent(&url);
    assert_eq!(hermes.capabilities(&a).await.unwrap(), json!({"runs": true}));
    assert_eq!(hermes.run(&a, "r1").await.unwrap()["status"], "running");
    let sessions = hermes.list_sessions(&a, 20).await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["limit"], "20");
    assert_eq!(hermes.session(&a, "wrapped").await.unwrap()["message_count"], 3);
    assert_eq!(hermes.session(&a, "plain").await.unwrap()["id"], "plain");
    assert!(matches!(hermes.session(&a, "text").await, Err(HermesError::Decode(_))));
    let messages = hermes.session_messages(&a, "s1").await.unwrap();
    assert_eq!((messages.len(), messages[0]["inline"].clone()), (1, json!("false")));
    assert_eq!(hermes.create_session(&a, "Live").await.unwrap(), "new-1");
    let mut fields = serde_json::Map::new();
    fields.insert("hidden".into(), json!(true));
    hermes.update_session(&a, "s1", fields).await.unwrap();
    let err = hermes.delete_session(&a, "s1").await.unwrap_err();
    assert_eq!(err.status(), Some(500));
    assert_eq!(err.to_string(), "Hermes HTTP 500");
    if let HermesError::Http { body, .. } = &err {
        assert_eq!(body.len(), 500);
    }
    let mut extra = serde_json::Map::new();
    extra.insert("hidden".into(), json!(true));
    assert_eq!(hermes.session_chat(&a, "s1", "salut", Some(extra), 240.0).await.unwrap(), "echo: salut");
    assert_eq!(hermes.start_run(&a, "Bonjour", "s1", Some("be brief")).await.unwrap(), "run_7");
    assert_eq!(hermes.start_run(&a, "Bonjour", "s1", None).await.unwrap(), "run_7");
    hermes.stop_run(&a, "run_7").await.unwrap();
    let (status, payload) = hermes.approve(&a, "run_7", "once", Some("req-1")).await.unwrap();
    assert_eq!((status, payload), (409, json!({"detail": "plain text refusal"})));

    let seen = seen.lock().unwrap().clone();
    let bodies: Vec<(String, String, Value)> =
        seen.iter().map(|(m, u, b, _)| (m.clone(), u.clone(), b.clone())).collect();
    assert_eq!(
        bodies,
        vec![
            ("POST".into(), "/api/sessions".into(), json!({"title": "Live"})),
            ("PATCH".into(), "/api/sessions/s1".into(), json!({"hidden": true})),
            ("POST".into(), "/api/sessions/s1/chat".into(), json!({"message": "salut", "hidden": true})),
            (
                "POST".into(),
                "/v1/runs".into(),
                json!({"input": "Bonjour", "session_id": "s1", "instructions": "be brief"})
            ),
            ("POST".into(), "/v1/runs".into(), json!({"input": "Bonjour", "session_id": "s1"})),
            ("POST".into(), "/v1/runs/run_7/approval".into(), json!({"choice": "once", "request_id": "req-1"})),
        ]
    );
    assert!(seen.iter().all(|(_, _, _, auth)| auth == &format!("Bearer {HERMES_KEY} | application/json")));
}

#[tokio::test]
async fn run_events_stream_and_errors() {
    let (url, seen) = fake().await;
    let hermes = HermesClient::new(reqwest::Client::new());
    let a = agent(&url);
    let events: Vec<_> = hermes.run_events(&a, "run_1").await.unwrap().collect().await;
    let events: Vec<_> = events.into_iter().map(|e| e.unwrap()).collect();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].r#type.as_str(), events[0].text()), ("message.delta", "Bonjour é".to_string()));
    assert_eq!((events[1].r#type.as_str(), events[1].final_text()), ("run.completed", "Fini".to_string()));
    assert!(seen.lock().unwrap().last().unwrap().3.ends_with("| text/event-stream"));

    let err = hermes.run_events(&a, "missing").await.err().unwrap();
    assert_eq!(err.status(), Some(404));
    let down = agent("http://127.0.0.1:9");
    let err = hermes.run_events(&down, "r").await.err().unwrap();
    assert!(err.is_transport());
    assert_eq!(err.type_name(), "ConnectError");
    let err = hermes.capabilities(&down).await.unwrap_err();
    assert_eq!(err.type_name(), "ConnectError");
}
