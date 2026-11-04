//! Hermes 0.22.5's Runs API as the bridge speaks it (hermes.rs, hermes_prompts.rs), against a fake api_server that
//! answers as its session authority: an approval as exactly the prompt the run's status shows waiting, a run lost in a
//! restart settled (never sent again), run starts tried again with one `Idempotency-Key`, an accepted-but-queued start
//! never resent, and a prompt's events read with their `request_id`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bipbridge::config::AgentConfig;
use bipbridge::hermes::HermesClient;
use common::*;
use futures::StreamExt;
use serde_json::{json, Value};

/// What the fake received: (method, path, Idempotency-Key, body).
type Seen = Arc<Mutex<Vec<(String, String, Option<String>, Value)>>>;
/// An answer of `POST /v1/runs`: its status and an extra header.
type StartAnswer = (u16, Option<(&'static str, &'static str)>);

#[derive(Clone, Default)]
struct Fake {
    seen: Seen,
    /// Answers of `POST /v1/runs`, in turn (then 202 with `run_7`).
    starts: Arc<Mutex<Vec<StartAnswer>>>,
}

async fn record(fake: &Fake, request: Request) -> Value {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let key = request.headers().get("idempotency-key").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    fake.seen.lock().unwrap().push((method, path, key, value.clone()));
    value
}

fn refusal(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({"error": {"message": code, "type": "invalid_request_error", "code": code}}))).into_response()
}

async fn run_status(Path(id): Path<String>) -> Json<Value> {
    if id == "run_lost" {
        return Json(json!({"object": "hermes.run", "run_id": id, "status": "unknown", "session_id": "api_s",
                           "admission_id": "adm_9", "execution_generation": 7, "pending_controls": [],
                           "output": "", "usage": {}}));
    }
    Json(json!({"object": "hermes.run", "run_id": id, "status": "waiting_for_approval", "session_id": "api_s",
                "admission_id": "adm_1", "execution_generation": 3, "output": "", "usage": {},
                "pending_controls": [
                    {"kind": "approval", "prompt_id": "p1", "execution_generation": 3, "command": "ls",
                     "description": "list", "choices": ["once", "session", "deny"]},
                    {"kind": "clarify", "prompt_id": "c1", "execution_generation": 3,
                     "question": "Quel train ?", "choices": ["9h", "14h"], "multi_select": false}]}))
}

async fn approval(State(fake): State<Fake>, request: Request) -> Response {
    let body = record(&fake, request).await;
    // gateway/platforms/api_server_authority_runs.py `respond_run`: exactly these three fields
    let mut keys: Vec<String> = body.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    keys.sort();
    if keys != ["choice", "execution_generation", "request_id"] {
        return refusal(StatusCode::BAD_REQUEST, "invalid_params");
    }
    if body["execution_generation"] != 3 {
        return refusal(StatusCode::CONFLICT, "stale_generation");
    }
    Json(json!({"run_id": "run_1", "status": "resolved", "prompt_id": body["request_id"]})).into_response()
}

async fn clarify(State(fake): State<Fake>, request: Request) -> Response {
    let body = record(&fake, request).await;
    let mut keys: Vec<String> = body.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default();
    keys.sort();
    if keys != ["answer", "execution_generation", "request_id"] {
        return refusal(StatusCode::BAD_REQUEST, "invalid_params");
    }
    Json(json!({"run_id": "run_1", "status": "resolved", "prompt_id": body["request_id"]})).into_response()
}

async fn resolve_unknown(State(fake): State<Fake>, request: Request) -> Response {
    let body = record(&fake, request).await;
    if body != json!({"admission_id": "adm_9", "execution_generation": 7}) {
        return refusal(StatusCode::BAD_REQUEST, "invalid_params");
    }
    Json(json!({"run_id": "run_lost", "outcome": "interrupted"})).into_response()
}

async fn start(State(fake): State<Fake>, request: Request) -> Response {
    record(&fake, request).await;
    let next = {
        let mut starts = fake.starts.lock().unwrap();
        if starts.is_empty() {
            None
        } else {
            Some(starts.remove(0))
        }
    };
    match next {
        None => (StatusCode::ACCEPTED, Json(json!({"run_id": "run_7", "status": "started"}))).into_response(),
        Some((status, header)) => {
            let mut response = refusal(StatusCode::from_u16(status).unwrap(), "storage_unavailable");
            if let Some((name, value)) = header {
                response.headers_mut().insert(name, value.parse().unwrap());
            }
            response
        }
    }
}

async fn events() -> Response {
    let text = concat!(
        "data: {\"event\": \"approval.request\", \"run_id\": \"run_1\", \"kind\": \"approval\", \"prompt_id\": \"p1\", ",
        "\"execution_generation\": 3, \"command\": \"ls\", \"choices\": [\"once\", \"deny\"]}\n\n",
        "data: {\"event\": \"approval.settled\", \"run_id\": \"run_1\", \"prompt_id\": \"p1\", ",
        "\"execution_generation\": 3}\n\n",
        "data: {\"event\": \"run.completed\", \"run_id\": \"run_1\", \"output\": \"Fini\", ",
        "\"usage\": {\"input_tokens\": 5, \"output_tokens\": 2, \"total_tokens\": 7}}\n\n",
    );
    Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from(text)).unwrap()
}

async fn fake_hermes() -> (AgentConfig, Fake) {
    let fake = Fake::default();
    let router = Router::new()
        .route("/v1/runs", post(start))
        .route("/v1/runs/{id}", get(run_status))
        .route("/v1/runs/{id}/approval", post(approval))
        .route("/v1/runs/{id}/clarify", post(clarify))
        .route("/v1/runs/{id}/resolve-unknown", post(resolve_unknown))
        .route("/v1/runs/{id}/events", get(events))
        .with_state(fake.clone());
    let url = spawn_fake(router).await;
    let agent = AgentConfig {
        name: "lea".into(),
        display_name: "Lea".into(),
        hermes_url: url,
        hermes_key: HERMES_KEY.into(),
        ..Default::default()
    };
    (agent, fake)
}

fn posted(fake: &Fake, path: &str) -> Vec<Value> {
    fake.seen.lock().unwrap().iter().filter(|(m, p, _, _)| m == "POST" && p == path).map(|s| s.3.clone()).collect()
}

#[tokio::test]
async fn an_approval_goes_as_the_exact_prompt_waiting() {
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, fake) = fake_hermes().await;
    // the one named, or the only approval waiting
    let (status, payload) = hermes.approve(&agent, "run_1", "once", Some("p1")).await.unwrap();
    assert_eq!((status, payload["status"].clone()), (200, json!("resolved")));
    let (status, _) = hermes.approve(&agent, "run_1", "deny", None).await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        posted(&fake, "/v1/runs/run_1/approval"),
        [
            json!({"request_id": "p1", "execution_generation": 3, "choice": "once"}),
            json!({"request_id": "p1", "execution_generation": 3, "choice": "deny"}),
        ]
    );
    // a prompt no longer waiting is not sent at all
    let (status, _) = hermes.approve(&agent, "run_1", "once", Some("p_gone")).await.unwrap();
    assert_eq!(status, 409);
    assert_eq!(posted(&fake, "/v1/runs/run_1/approval").len(), 2);
}

#[tokio::test]
async fn a_skipped_question_goes_as_hermes_skip() {
    // Hermes 0.22.5 reads an empty answer as no reply (the batch ends « timed out »): « Passer » sends its skip word
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, fake) = fake_hermes().await;
    let (status, _) = hermes.answer_prompt(&agent, "run_1", "clarify", "", Some("c1")).await.unwrap();
    assert_eq!(status, 200);
    let (status, _) = hermes.answer_prompt(&agent, "run_1", "clarify", "14h", Some("c1")).await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(
        posted(&fake, "/v1/runs/run_1/clarify"),
        [
            json!({"request_id": "c1", "execution_generation": 3, "answer": "\u{0}skipped"}),
            json!({"request_id": "c1", "execution_generation": 3, "answer": "14h"}),
        ]
    );
    // a question no longer waiting: 409, nothing sent
    let (status, _) = hermes.answer_prompt(&agent, "run_1", "clarify", "", Some("c_gone")).await.unwrap();
    assert_eq!(status, 409);
    assert_eq!(posted(&fake, "/v1/runs/run_1/clarify").len(), 2);
}

#[tokio::test]
async fn a_run_lost_in_a_restart_is_settled_never_sent_again() {
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, fake) = fake_hermes().await;
    let run = hermes.run(&agent, "run_lost").await.unwrap();
    assert_eq!(run["status"], "interrupted");
    assert_eq!(run["error"], bipbridge::hermes::LOST_IN_RESTART);
    assert_eq!(
        posted(&fake, "/v1/runs/run_lost/resolve-unknown"),
        [json!({"admission_id": "adm_9", "execution_generation": 7})]
    );
    assert!(posted(&fake, "/v1/runs").is_empty());
    // a run that is not lost is read as it is
    assert_eq!(hermes.run(&agent, "run_1").await.unwrap()["status"], "waiting_for_approval");
}

#[tokio::test]
async fn a_run_start_is_tried_again_with_one_key() {
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, fake) = fake_hermes().await;
    // the store busy for a moment (Retry-After), then the start goes through, under the same key
    fake.starts.lock().unwrap().push((503, Some(("retry-after", "0"))));
    assert_eq!(hermes.start_run(&agent, "Bonjour", "api_s", None).await.unwrap(), "run_7");
    let seen = fake.seen.lock().unwrap().clone();
    let keys: Vec<Option<String>> = seen.iter().filter(|s| s.1 == "/v1/runs").map(|s| s.2.clone()).collect();
    assert_eq!(keys.len(), 2);
    assert!(keys[0].as_deref().is_some_and(|k| k.starts_with("bip-") && k.len() > 20));
    assert_eq!(keys[0], keys[1]);
    // the next start has a key of its own
    assert_eq!(hermes.start_run(&agent, "Encore", "api_s", None).await.unwrap(), "run_7");
    let seen = fake.seen.lock().unwrap().clone();
    let last = seen.iter().rfind(|s| s.1 == "/v1/runs").unwrap().2.clone();
    assert_ne!(last, keys[0]);
}

#[tokio::test]
async fn an_accepted_but_queued_start_is_never_sent_again() {
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, fake) = fake_hermes().await;
    fake.starts.lock().unwrap().push((503, Some(("x-should-retry", "false"))));
    let err = hermes.start_run(&agent, "Bonjour", "api_s", None).await.unwrap_err();
    assert_eq!(err.status(), Some(503));
    assert_eq!(posted(&fake, "/v1/runs").len(), 1);
    // nor one Hermes refuses for good, nor a 503 that asks for no wait
    fake.starts.lock().unwrap().push((409, None));
    assert_eq!(hermes.start_run(&agent, "Bonjour", "api_s", None).await.unwrap_err().status(), Some(409));
    fake.starts.lock().unwrap().push((503, None));
    assert_eq!(hermes.start_run(&agent, "Bonjour", "api_s", None).await.unwrap_err().status(), Some(503));
    assert_eq!(posted(&fake, "/v1/runs").len(), 3);
    // a profile parked for long: tried `START_ATTEMPTS` times, then given up
    for _ in 0..bipbridge::hermes::START_ATTEMPTS {
        fake.starts.lock().unwrap().push((503, Some(("retry-after", "0"))));
    }
    let started = std::time::Instant::now();
    assert_eq!(hermes.start_run(&agent, "Bonjour", "api_s", None).await.unwrap_err().status(), Some(503));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(posted(&fake, "/v1/runs").len(), 3 + bipbridge::hermes::START_ATTEMPTS as usize);
}

#[tokio::test]
async fn a_prompts_events_carry_its_request_id() {
    let hermes = HermesClient::new(reqwest::Client::new());
    let (agent, _) = fake_hermes().await;
    let events: Vec<_> = hermes.run_events(&agent, "run_1").await.unwrap().collect().await;
    let events: Vec<_> = events.into_iter().map(Result::unwrap).collect();
    let kinds: Vec<&str> = events.iter().map(|e| e.r#type.as_str()).collect();
    assert_eq!(kinds, ["approval.request", bipbridge::hermes_prompts::APPROVAL_SETTLED, "run.completed"]);
    assert_eq!((&events[0].data["request_id"], &events[1].data["request_id"]), (&json!("p1"), &json!("p1")));
    assert_eq!(events[0].data["execution_generation"], 3);
    // the apps read the events' raw text: it has the request id too
    assert!(events[0].raw.contains("request_id"));
}
