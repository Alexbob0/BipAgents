//! The run hub: one upstream SSE per run fanned out to its followers, the backlog replayed with merged deltas,
//! approvals / questions / « reply ready » pushed when nobody follows, /v1/approve forwarded, watched runs resumed
//! after a restart (Python: the approval and run parts of tests/test_voice_and_approvals.py; the voice
//! WebSocket's followers are listeners of kind « voice » here).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bipbridge::app::AppState;
use bipbridge::config::{parse_config, AgentConfig, SharedConfig};
use bipbridge::hermes::HermesClient;
use bipbridge::push::{PushService, Sender};
use bipbridge::runs::{Listener, RunHub, BACKLOG_EVENTS};
use bipbridge::store::Store;
use common::fakes::*;
use common::*;
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

struct Client {
    _dir: tempfile::TempDir,
    app: TestApp,
    backend: FakeBackend,
    apns: FakeApns,
}

impl Client {
    fn state(&self) -> AppState {
        self.app.app.state.clone()
    }
    fn agent(&self) -> AgentConfig {
        self.state().config.get().agent(Some("wellness")).cloned().unwrap()
    }
    async fn register(&self) {
        let resp = self
            .post("/v1/devices", json!({"token": TOKEN, "environment": "sandbox", "agent_ids": ["wellness"]}))
            .await;
        assert_eq!(resp.status, StatusCode::OK);
    }
    async fn post(&self, path: &str, body: Value) -> TestResponse {
        let (k, v) = auth();
        self.app.send_json("POST", path, &[(k, v.as_str())], &body).await
    }
    async fn get(&self, path: &str) -> TestResponse {
        let (k, v) = auth();
        self.app.get(path, &[(k, v.as_str())]).await
    }
    fn script(&self, run_id: &str, items: Vec<RunItem>) {
        self.backend.with(|s| s.runs.insert(run_id.into(), items));
    }
    async fn close(self) {
        self.backend.release.set();
        self.app.shutdown().await;
    }
}

/// conftest's `client` fixture (the background tasks off: none of these tests needs them).
async fn client() -> Client {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &backend.urls());
    data["apns"]["base_url"] = json!(apns.url);
    let app = TestApp::new(parse_config(&data, None).unwrap());
    Client { _dir: dir, app, backend, apns }
}

fn text(s: String) -> RunItem {
    RunItem::Text(s)
}

fn approval_event(run_id: &str, request_id: &str) -> RunItem {
    text(sse(
        "approval.request",
        json!({"run_id": run_id, "request_id": request_id, "command": "rm -rf build/",
                                        "choices": ["once", "session", "deny"]}),
    ))
}

fn count(apns: &FakeApns) -> usize {
    apns.requests.lock().unwrap().len()
}

async fn drain(listener: &mut Listener) -> Vec<String> {
    let mut types = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(10), listener.next()).await {
        types.push(event.r#type);
    }
    types
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_pushes_approval_when_nobody_follows() {
    let c = client().await;
    c.register().await;
    let gate = Flag::new();
    c.script(
        "run_w",
        vec![
            text(sse("message.delta", json!({"run_id": "run_w", "delta": "Je supprime le dossier."}))),
            approval_event("run_w", "req_1"),
            approval_event("run_w", "req_1"), // duplicate must not push twice
            RunItem::Wait(gate.clone()),
            text(sse("run.completed", json!({"run_id": "run_w"}))),
        ],
    );
    let resp = c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_w"})).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json()["watching"], true);
    assert!(wait_until(|| count(&c.apns) >= 1, Duration::from_secs(5)).await);
    let payload = c.apns.payloads()[0].clone();
    assert_eq!(payload["aps"]["category"], "APPROVAL");
    assert_eq!(payload["aps"]["alert"]["body"], "Approbation requise");
    assert_eq!(payload["aps"]["thread-id"], "wellness");
    assert_eq!(payload["aps"]["mutable-content"], 1);
    assert_eq!(
        (&payload["run_id"], &payload["request_id"], &payload["agent"]),
        (&json!("run_w"), &json!("req_1"), &json!("wellness"))
    );
    assert_eq!(payload["choices"], json!(["once", "session", "deny"]));
    assert_eq!(c.apns.requests.lock().unwrap()[0].header("apns-collapse-id").as_deref(), Some("req_1"));

    let pending = c.get("/v1/approvals").await.json()["items"].clone();
    assert_eq!(pending.as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["command"], "rm -rf build/");
    assert_eq!(pending[0]["request_id"], "req_1");

    gate.set();
    // Run finished while watched and unfollowed -> silent push for resync; approval cleared.
    assert!(wait_until(|| count(&c.apns) >= 2, Duration::from_secs(5)).await);
    let silent = c.apns.payloads()[1].clone();
    assert_eq!(
        silent,
        json!({"aps": {"content-available": 1}, "agent": "wellness", "reason": "run_finished", "run_id": "run_w", "status": "run.completed"})
    );
    assert_eq!(c.apns.requests.lock().unwrap()[1].header("apns-push-type").as_deref(), Some("background"));
    assert!(wait_until(|| true, Duration::from_millis(50)).await);
    assert_eq!(c.get("/v1/approvals").await.json()["items"], json!([]));
    assert_eq!(c.apns.payloads().iter().filter(|p| p["aps"]["category"] == "APPROVAL").count(), 1);
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_approval_push_while_voice_client_follows() {
    let c = client().await;
    c.register().await;
    let gate = Flag::new();
    c.script(
        "run_f",
        vec![
            text(sse("message.delta", json!({"run_id": "run_f", "delta": "Je dois demander. "}))),
            RunItem::Wait(gate.clone()),
            approval_event("run_f", "req_2"),
            text(sse("run.completed", json!({"run_id": "run_f"}))),
        ],
    );
    let state = c.state();
    let mut listener = state.hub.subscribe(&c.agent(), "run_f", "voice", true);
    assert_eq!(listener.next().await.unwrap().r#type, "message.delta");
    c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_f"})).await;
    gate.set();
    assert_eq!(drain(&mut listener).await, ["approval.request", "run.completed"]);
    drop(listener);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count(&c.apns), 0); // follower attached: the app shows the approval itself
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unwatched_run_never_pushes() {
    let c = client().await;
    c.register().await;
    c.script("run_u", vec![approval_event("run_u", "req_3"), text(sse("run.completed", json!({"run_id": "run_u"})))]);
    // watch_followed_runs = false: the voice follower does not watch
    let mut listener = c.state().hub.subscribe(&c.agent(), "run_u", "voice", false);
    drain(&mut listener).await;
    drop(listener);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(count(&c.apns), 0);
    c.close().await;
}

#[tokio::test]
async fn approve_forwards_to_hermes() {
    let c = client().await;
    let resp = c
        .post("/v1/approve", json!({"agent": "wellness", "run_id": "run_9", "request_id": "req_9", "choice": "Once"}))
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json()["resolved"], 1);
    assert_eq!(
        c.backend.with(|s| s.approvals.clone()),
        [json!({"run_id": "run_9", "body": {"choice": "once", "request_id": "req_9"}})]
    );
    c.backend.with(|s| s.approval_response = (409, json!({"error": {"code": "approval_not_pending"}})));
    let resp = c.post("/v1/approve", json!({"agent": "wellness", "run_id": "run_9", "choice": "deny"})).await;
    assert_eq!(resp.status, StatusCode::CONFLICT);
    assert_eq!(c.backend.with(|s| s.approvals.last().unwrap()["body"].clone()), json!({"choice": "deny"}));
    let resp = c.post("/v1/approve", json!({"agent": "wellness", "run_id": "../x", "choice": "once"})).await;
    assert_eq!(resp.status, StatusCode::BAD_REQUEST);
    let resp = c.post("/v1/approve", json!({"agent": "ghost", "run_id": "r", "choice": "once"})).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
    c.close().await;
}

/// (event, data) pairs of an SSE response; keepalives skipped.
fn read_sse(body: &str) -> Vec<(String, Value)> {
    let mut events = Vec::new();
    let mut name = String::new();
    for line in body.lines() {
        if let Some(n) = line.strip_prefix("event: ") {
            name = n.to_string();
        } else if let Some(d) = line.strip_prefix("data: ") {
            events.push((name.clone(), serde_json::from_str(d).unwrap()));
        }
    }
    events
}

fn names(events: &[(String, Value)]) -> Vec<&str> {
    events.iter().map(|(n, _)| n.as_str()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn app_follows_run_through_bridge_and_replays_on_reattach() {
    let c = client().await;
    c.script(
        "run_a",
        vec![
            text(sse("tool.started", json!({"run_id": "run_a", "tool": "web_search"}))),
            text(sse("message.delta", json!({"run_id": "run_a", "delta": "Bonjour "}))),
            text(sse("message.delta", json!({"run_id": "run_a", "delta": "Alex."}))),
            text(sse("run.completed", json!({"run_id": "run_a", "output": "Bonjour Alex."}))),
        ],
    );
    let url = "/v1/runs/run_a/events?agent=wellness&session_id=api_123";
    let resp = c.get(url).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert!(resp.header("content-type").unwrap().starts_with("text/event-stream"));
    let first = read_sse(&resp.text());
    assert_eq!(names(&first), ["tool.started", "message.delta", "message.delta", "run.completed"]);
    assert_eq!(first.last().unwrap().1["output"], "Bonjour Alex.");
    // Re-attaching (app reopened, connection lost) replays the whole run from the bridge's backlog,
    // consecutive deltas merged.
    let replay = read_sse(&c.get(url).await.text());
    assert_eq!(names(&replay), ["tool.started", "message.delta", "run.completed"]);
    assert_eq!(replay[1].1["delta"], "Bonjour Alex.");
    assert_eq!(c.backend.with(|s| s.run_requests.len()), 1); // one Hermes subscription for both
    c.close().await;
}

#[tokio::test]
async fn app_route_validates_agent_and_ids() {
    let c = client().await;
    assert_eq!(c.get("/v1/runs/run_a/events?agent=nope").await.status, StatusCode::NOT_FOUND);
    assert_eq!(c.get("/v1/runs/run_a/events?agent=wellness&session_id=bad%20id").await.status, StatusCode::BAD_REQUEST);
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reply_ready_pushed_when_the_app_left() {
    let c = client().await;
    c.register().await;
    let gate = Flag::new();
    c.script(
        "run_r",
        vec![
            text(sse("message.delta", json!({"run_id": "run_r", "delta": "Je cherche."}))),
            RunItem::Wait(gate.clone()),
            text(sse("run.completed", json!({"run_id": "run_r", "output": "Le train de 9h est à 19 €."}))),
        ],
    );
    // what the SSE route does, then the app leaves the conversation
    let mut listener = c.state().hub.subscribe(&c.agent(), "run_r", "app", true);
    listener.sub.set_session_id(Some("api_9".into()));
    assert_eq!(listener.sub.followers(), 1);
    listener.close();
    let sub = listener.sub.clone();
    assert_eq!(sub.followers(), 0);
    gate.set();
    assert!(wait_until(|| count(&c.apns) >= 1, Duration::from_secs(5)).await);
    let payload = c.apns.payloads()[0].clone();
    assert_eq!(payload["kind"], "reply");
    assert_eq!(payload["session_id"], "api_9");
    assert_eq!(payload["run_id"], "run_r");
    assert_eq!(payload["aps"]["category"], "MESSAGE");
    assert_eq!(payload["aps"]["alert"]["body"], "Ta réponse est prête.");
    // The text stays off Apple: the extension fetches it from the bridge.
    let reply = c.get(&format!("/v1/replies/{}", payload["reply_id"].as_str().unwrap())).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["text"], "Le train de 9h est à 19 €.");
    assert_eq!(c.get("/v1/replies/nope").await.status, StatusCode::NOT_FOUND);
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn replay_backlog_merges_deltas_so_long_replies_keep_their_start() {
    let c = client().await;
    let words: Vec<String> = (0..BACKLOG_EVENTS + 500).map(|i| format!("mot{i} ")).collect();
    let mut items = vec![text(sse("message.delta", json!({"run_id": "run_long", "delta": "Début : "})))];
    items.extend(words.iter().map(|w| text(sse("message.delta", json!({"run_id": "run_long", "delta": w})))));
    items.push(text(sse("tool.started", json!({"run_id": "run_long", "tool": "web_search"}))));
    items.push(text(sse("message.delta", json!({"run_id": "run_long", "delta": "Fin."}))));
    items.push(text(sse("run.completed", json!({"run_id": "run_long", "output": "…"}))));
    c.script("run_long", items);
    read_sse(&c.get("/v1/runs/run_long/events?agent=wellness").await.text()); // first follower: every delta, live
    let replay = read_sse(&c.get("/v1/runs/run_long/events?agent=wellness").await.text()); // late follower: the backlog
    assert_eq!(names(&replay), ["message.delta", "tool.started", "message.delta", "run.completed"]);
    assert_eq!(replay[0].1["delta"], json!(format!("Début : {}", words.concat())));
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn question_pushed_when_the_app_left() {
    let c = client().await;
    c.register().await;
    let gate = Flag::new();
    c.script(
        "run_q",
        vec![
            RunItem::Wait(gate.clone()),
            text(sse("clarify.request", json!({"run_id": "run_q", "request_id": "clr_1",
                                               "questions": [{"id": "q1", "question": "Quel train ?", "choices": ["9h", "14h"]}]}))),
            RunItem::Wait(Flag::new()), // the run waits for the answer
        ],
    );
    let mut listener = c.state().hub.subscribe(&c.agent(), "run_q", "app", true);
    listener.sub.set_session_id(Some("api_7".into()));
    listener.close();
    gate.set();
    assert!(wait_until(|| count(&c.apns) >= 1, Duration::from_secs(5)).await);
    let payload = c.apns.payloads()[0].clone();
    assert_eq!(payload["kind"], "question");
    assert_eq!(payload["request_id"], "clr_1");
    assert_eq!(payload["session_id"], "api_7");
    assert_eq!(payload["aps"]["alert"]["body"], "Une question pour toi");
    assert_eq!(payload["aps"]["mutable-content"], 1);
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_question_contract_is_pushed_too() {
    // Hermes' upstream clarify (one question per `clarify_id`) is pushed like ours.
    let c = client().await;
    c.register().await;
    let gate = Flag::new();
    c.script(
        "run_u",
        vec![
            RunItem::Wait(gate.clone()),
            text(sse(
                "clarify.request",
                json!({"run_id": "run_u", "clarify_id": "cq_9", "question": "Quel train ?",
                                               "choices": ["9h", "14h"], "multi_select": false}),
            )),
            RunItem::Wait(Flag::new()),
        ],
    );
    let mut listener = c.state().hub.subscribe(&c.agent(), "run_u", "app", true);
    listener.close();
    gate.set();
    assert!(wait_until(|| count(&c.apns) >= 1, Duration::from_secs(5)).await);
    assert_eq!(c.apns.payloads()[0]["request_id"], "cq_9");
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn approval_is_pushed_when_the_app_stops_following() {
    let c = client().await;
    c.register().await;
    let (asked, finish) = (Flag::new(), Flag::new());
    c.script(
        "run_l",
        vec![
            text(sse("message.delta", json!({"run_id": "run_l", "delta": "Je génère le podcast. "}))),
            RunItem::Wait(asked.clone()),
            approval_event("run_l", "req_l"),
            RunItem::Wait(finish.clone()),
            text(sse("run.completed", json!({"run_id": "run_l"}))),
        ],
    );
    let mut listener = c.state().hub.subscribe(&c.agent(), "run_l", "voice", true);
    assert_eq!(listener.next().await.unwrap().r#type, "message.delta");
    c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_l"})).await;
    asked.set();
    let mut pending = false;
    for _ in 0..250 {
        if !c.get("/v1/approvals").await.json()["items"].as_array().unwrap().is_empty() {
            pending = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(pending);
    assert_eq!(count(&c.apns), 0); // on screen: the app shows it
                                   // Left the conversation with the approval unanswered: it is pushed now, once.
    listener.close();
    assert!(
        wait_until(|| c.apns.payloads().iter().any(|p| p["aps"]["category"] == "APPROVAL"), Duration::from_secs(5))
            .await
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    let approvals: Vec<Value> = c.apns.payloads().into_iter().filter(|p| p["aps"]["category"] == "APPROVAL").collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0]["request_id"], "req_l");
    finish.set();
    c.close().await;
}

#[derive(Clone, Default)]
struct Runs(Arc<Mutex<HashMap<String, Value>>>);

async fn run_status(State(runs): State<Runs>, Path(run_id): Path<String>) -> Response {
    match runs.0.lock().unwrap().get(&run_id).cloned() {
        Some(run) => Json(run).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "gone"}))).into_response(),
    }
}

async fn hanging_events() -> Response {
    let stream = futures::stream::pending::<Result<bytes::Bytes, std::io::Error>>();
    Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn watched_runs_resume_after_a_restart_with_their_pending_approval() {
    let dir = tempfile::tempdir().unwrap();
    let runs = Runs::default();
    let url = spawn_fake(
        Router::new()
            .route("/v1/runs/{run_id}", get(run_status))
            .route("/v1/runs/{run_id}/events", get(hanging_events))
            .with_state(runs.clone()),
    )
    .await;
    let agent = AgentConfig {
        name: "vie".into(),
        display_name: "Vie".into(),
        hermes_url: url,
        hermes_key: "k".repeat(40),
        ..Default::default()
    };
    let store = Store::open(&dir.path().join("b.db").to_string_lossy()).unwrap();
    let http = reqwest::Client::new();
    let before = Arc::new(RunHub::new(HermesClient::new(http.clone()), None, 3600, Some(store.clone())));
    before.watch(&agent, "run_wait", Some("api_bot"), true);
    before.watch(&agent, "run_done", None, true);
    before.watch(&agent, "run_gone", None, true);
    tokio::time::sleep(Duration::from_millis(50)).await;
    before.close().await;
    {
        let mut r = runs.0.lock().unwrap();
        r.insert(
            "run_wait".into(),
            json!({"status": "waiting_for_approval", "session_id": "api_bot",
                   "approval": {"request_id": "req_x", "command": "python3 -c 'tts health'", "choices": ["once", "session", "deny"]}}),
        );
        r.insert("run_done".into(), json!({"status": "completed"}));
    }
    // the pushes: a real PushService to a fake APNs, one device for the agent
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &Urls::default());
    data["apns"]["base_url"] = json!(apns.url);
    let config = parse_config(&data, None).unwrap();
    let mut apns_config = config.apns.clone();
    apns_config.base_url = apns.url.clone();
    store.upsert_device(TOKEN, "sandbox", &["vie".to_string()]).await.unwrap();
    let sender = Sender::Apns(Arc::new(bipbridge::apns::ApnsClient::new(apns_config, http.clone())));
    let push = Arc::new(PushService::new(SharedConfig::new(config), store.clone(), Some(sender), None, None));
    let after = Arc::new(RunHub::new(HermesClient::new(http), Some(push), 3600, Some(store.clone())));
    let mut agents = IndexMap::new();
    agents.insert("vie".to_string(), agent.clone());
    assert_eq!(after.clone().resume(agents).await, 1);
    let pending: Vec<(Value, Value, Value)> = after
        .pending_approvals(Some("vie"))
        .iter()
        .map(|p| (p["run_id"].clone(), p["request_id"].clone(), p["session_id"].clone()))
        .collect();
    assert_eq!(pending, [(json!("run_wait"), json!("req_x"), json!("api_bot"))]);
    // nobody follows: pushed, with its session
    assert!(wait_until(|| count(&apns) >= 1, Duration::from_secs(5)).await);
    let payload = apns.payloads()[0].clone();
    assert_eq!(payload["aps"]["category"], "APPROVAL");
    assert_eq!(payload["session_id"], "api_bot");
    assert_eq!(payload["request_id"], "req_x");
    // finished and unknown ones forgotten
    let watched: Vec<Value> = store.watched_runs(3600.0).await.unwrap().iter().map(|r| r["run_id"].clone()).collect();
    assert_eq!(watched, [json!("run_wait")]);
    after.close().await;
    store.close();
}
