//! A Live call lands in the conversation it continued, by the bridge (live_calls.rs): at the hang-up, after its last
//! turn ends in the background, or when the app never said (nothing for IDLE_SECONDS); older apps keep their own way.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use bipbridge::live_calls::{end_idle, Ending, LiveCalls, OpenCall, IDLE_SECONDS};
use bipbridge::py::time_time;
use common::{auth, config_dict, spawn_fake, TestApp, HERMES_KEY};
use serde_json::{json, Value};

#[derive(Default)]
struct Hermes {
    /// id -> (row, messages)
    sessions: std::collections::HashMap<String, (Value, Vec<Value>)>,
    /// run id -> status
    runs: std::collections::HashMap<String, String>,
}

type Fake = Arc<Mutex<Hermes>>;

async fn handle(State(fake): State<Fake>, request: Request) -> Response {
    assert_eq!(request.headers()["authorization"], format!("Bearer {HERMES_KEY}"));
    let path = request.uri().path().trim_matches('/').to_string();
    let parts: Vec<&str> = path.split('/').collect();
    let h = fake.lock().unwrap();
    match parts.as_slice() {
        ["api", "sessions", id] => match h.sessions.get(*id) {
            Some((row, _)) => Json(json!({"session": row})).into_response(),
            None => (StatusCode::NOT_FOUND, Json(json!({"error": "Session not found"}))).into_response(),
        },
        ["api", "sessions", id, "messages"] => match h.sessions.get(*id) {
            Some((_, messages)) => Json(json!({"messages": messages})).into_response(),
            None => (StatusCode::NOT_FOUND, Json(json!({"error": "Session not found"}))).into_response(),
        },
        ["v1", "runs", id] => match h.runs.get(*id) {
            Some(status) => Json(json!({"run_id": id, "status": status})).into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        },
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

struct Setup {
    _dir: tempfile::TempDir,
    fake: Fake,
    app: TestApp,
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let fake: Fake = Arc::new(Mutex::new(Hermes::default()));
    fake.lock()
        .unwrap()
        .sessions
        .insert("api_main".into(), (json!({"id": "api_main", "title": "Discussion", "message_count": 4}), vec![]));
    let url = spawn_fake(Router::new().fallback(any(handle)).with_state(fake.clone())).await;
    let urls = common::Urls { hermes: url, ..Default::default() };
    let app = TestApp::new(bipbridge::config::parse_config(&config_dict(dir.path(), &urls), None).unwrap());
    Setup { _dir: dir, fake, app }
}

fn call(started: f64, last: f64, messages: Vec<Value>) -> (Value, Vec<Value>) {
    (
        json!({"id": "live1", "title": "Live · 10:05", "started_at": started, "last_active": last,
            "message_count": messages.len()}),
        messages,
    )
}

fn said() -> Vec<Value> {
    vec![
        json!({"role": "user", "content": "<!--bip:call-start\nThe person just started a voice call\n-->"}),
        json!({"role": "assistant", "content": "Salut !"}),
        json!({"role": "user", "content": "Fais-moi une fiche des boulangeries du quartier"}),
        json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1"}]}),
        json!({"role": "tool", "content": "{\"ok\": true}"}),
        json!({"role": "assistant", "content": "La voilà, je te la mets dans la conversation.\nMEDIA:/bipagents/pages/boulangeries.html"}),
    ]
}

async fn records_of(app: &TestApp, session: &str) -> Vec<Value> {
    let (k, v) = auth();
    app.get(&format!("/v1/live/calls?agent=Wellness&session={session}"), &[(k, v.as_str())]).await.json()["calls"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The background end, waited for (a few seconds at most).
async fn wait_for(app: &TestApp, session: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..100 {
        let records = records_of(app, session).await;
        if done(&records) {
            return records;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the call never landed in {session}");
}

#[tokio::test]
async fn a_hung_up_call_lands_in_its_conversation() {
    let s = setup().await;
    let now = time_time();
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - 250.0, now - 5.0, said()));
    let (k, v) = auth();
    let a = [(k, v.as_str())];
    let started = s
        .app
        .send_json(
            "POST",
            "/v1/live/calls/start",
            &a,
            &json!({"agent": "Wellness", "session": "api_main", "live_session": "live1"}),
        )
        .await;
    assert_eq!(started.json(), json!({"ok": true, "deposit": true}));
    let name = s.app.app.state.config().agent(Some("Wellness")).unwrap().name.clone();
    let calls = LiveCalls::of(&s.app.app.state.briefs);
    assert_eq!(calls.open_calls().len(), 1);
    // no call yet: the conversation's state says nothing of calls (older apps see the same answer as before)
    let state = s.app.get("/v1/sessions/api_main/state?agent=Wellness", &a).await.json();
    assert!(state.get("calls").is_none(), "{state}");

    let ended = s
        .app
        .send_json(
            "POST",
            "/v1/live/calls",
            &a,
            &json!({"agent": "Wellness", "session": "api_main", "live_session": "live1", "deposit": true}),
        )
        .await;
    assert_eq!(ended.json(), json!({"ok": true, "deposit": true}));
    let records = wait_for(&s.app, "api_main", |r| !r.is_empty()).await;
    let record = &records[0];
    assert_eq!(record["id"], "live1");
    assert_eq!(record["working"], false);
    assert_eq!(record["late"], false);
    assert_eq!(record["ended_by"], "hangup");
    assert!(record["seconds"].as_f64().unwrap() >= 240.0, "{record}");
    assert_eq!(record["media"], json!(["/bipagents/pages/boulangeries.html"]));
    let lines = record["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 3, "the opening and the tools left out: {record}");
    assert_eq!(lines[1], json!({"role": "user", "text": "Fais-moi une fiche des boulangeries du quartier"}));
    assert!(record.get("recapped").is_none(), "bookkeeping not listed");
    assert!(calls.open_calls().is_empty());
    // the conversation's state changes: the app on it reloads
    let state = s.app.get("/v1/sessions/api_main/state?agent=Wellness", &a).await.json();
    assert!(state["calls"].as_str().unwrap().starts_with("1@"), "{state}");
    // the agent gets the call with the next message written there, the page named
    let taken = s
        .app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_main"}))
        .await
        .json();
    let recap = common::without_now(taken["recap"].as_str().unwrap());
    assert!(recap.starts_with("<!--bip:call\nVoice call of "), "{recap}");
    assert!(recap.contains("The person: Fais-moi une fiche"));
    assert!(recap.contains("MEDIA:/bipagents/pages/boulangeries.html"));
    // and its next briefs know it
    assert_eq!(s.app.app.state.briefs.load(&name, "api_main")["calls"][0]["live_session"], "live1");
}

#[tokio::test]
async fn an_older_app_keeps_its_own_way() {
    let s = setup().await;
    let now = time_time();
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - 100.0, now, said()));
    let (k, v) = auth();
    let a = [(k, v.as_str())];
    let ended = s
        .app
        .send_json(
            "POST",
            "/v1/live/calls",
            &a,
            &json!({"agent": "Wellness", "session": "api_main", "live_session": "live1"}),
        )
        .await;
    assert_eq!(ended.json(), json!({"ok": true})); // no `deposit`: the app keeps the recap (CallRecaps)
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(records_of(&s.app, "api_main").await.is_empty());
    let taken = s
        .app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_main"}))
        .await
        .json();
    assert_eq!(common::without_now(taken["recap"].as_str().unwrap()), "", "no second recap from the bridge");
}

#[tokio::test]
async fn a_turn_going_on_at_the_hang_up_ends_in_the_background() {
    let s = setup().await;
    let now = time_time();
    let mut messages = said();
    messages.truncate(5); // the person asked, a tool runs
    {
        let mut h = s.fake.lock().unwrap();
        h.sessions.insert("live1".into(), call(now - 120.0, now, messages));
        h.runs.insert("run_7".into(), "running".into());
    }
    let agent = s.app.app.state.config().agent(Some("Wellness")).unwrap().clone();
    let mut ending = Ending::hangup(agent, "api_main", "live1", Some("run_7".into()));
    ending.poll = 0.05;
    let state = s.app.app.state.clone();
    let task = tokio::spawn(async move { bipbridge::live_calls::end_call(&state, ending).await });
    // shown at once, « still working »
    let records = wait_for(&s.app, "api_main", |r| !r.is_empty()).await;
    assert_eq!(records[0]["working"], true);
    assert_eq!(records[0]["media"], json!([]));
    // the turn ends: its answer and its page land there
    {
        let mut h = s.fake.lock().unwrap();
        h.sessions.insert("live1".into(), call(now - 120.0, time_time(), said()));
        h.runs.insert("run_7".into(), "completed".into());
    }
    let record = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
    assert_eq!(record["working"], false);
    assert_eq!(record["late"], true);
    assert_eq!(record["media"], json!(["/bipagents/pages/boulangeries.html"]));
    let records = records_of(&s.app, "api_main").await;
    assert_eq!(records.len(), 1, "the same call, updated");
    assert_eq!(records[0]["late"], true);
}

#[tokio::test]
async fn a_turn_hermes_keeps_no_run_for_is_read_from_its_messages() {
    let s = setup().await;
    let now = time_time();
    let mut messages = said();
    messages.truncate(3); // asked, no answer yet; no run on /v1/runs (a chat/stream turn)
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - 60.0, now, messages));
    let agent = s.app.app.state.config().agent(Some("Wellness")).unwrap().clone();
    let mut ending = Ending::hangup(agent, "api_main", "live1", Some("stream_run".into()));
    ending.poll = 0.05;
    let state = s.app.app.state.clone();
    let task = tokio::spawn(async move { bipbridge::live_calls::end_call(&state, ending).await });
    wait_for(&s.app, "api_main", |r| r.first().is_some_and(|c| c["working"] == true)).await;
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - 60.0, time_time(), said()));
    let record = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap().unwrap();
    assert_eq!(record["late"], true);
    assert_eq!(record["lines"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn a_call_the_app_never_ended_lands_after_a_quiet_while() {
    let s = setup().await;
    let now = time_time();
    let name = s.app.app.state.config().agent(Some("Wellness")).unwrap().name.clone();
    let calls = LiveCalls::of(&s.app.app.state.briefs);
    calls
        .note_start(OpenCall {
            agent: name.clone(),
            session: "api_main".into(),
            live_session: "live1".into(),
            started_at: now - IDLE_SECONDS - 400.0,
        })
        .unwrap();
    // still active a moment ago: left alone
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - IDLE_SECONDS - 400.0, now - 30.0, said()));
    assert_eq!(end_idle(&s.app.app.state, now).await, 0);
    assert_eq!(calls.open_calls().len(), 1);
    // quiet for long enough (the app was quit): ended, from its last activity
    let last = now - IDLE_SECONDS - 60.0;
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - IDLE_SECONDS - 400.0, last, said()));
    assert_eq!(end_idle(&s.app.app.state, now).await, 1);
    assert!(calls.open_calls().is_empty());
    let records = records_of(&s.app, "api_main").await;
    assert_eq!(records[0]["ended_by"], "idle");
    assert_eq!(records[0]["seconds"], json!(340.0));
    // the app comes back and hangs up later: the same call, no second recap of what was given already
    let (k, v) = auth();
    let a = [(k, v.as_str())];
    let first = s
        .app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_main"}))
        .await
        .json();
    assert!(first["recap"].as_str().unwrap().contains("bip:call"));
    let agent = s.app.app.state.config().agent(Some("Wellness")).unwrap().clone();
    bipbridge::live_calls::end_call(&s.app.app.state, Ending::hangup(agent, "api_main", "live1", None)).await.unwrap();
    assert_eq!(records_of(&s.app, "api_main").await.len(), 1);
    let again = s
        .app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_main"}))
        .await
        .json();
    // nothing new said since: no second recap
    let again = common::without_now(again["recap"].as_str().unwrap());
    assert!(!again.contains("The person:"), "{again}");
}

#[tokio::test]
async fn a_call_where_nothing_was_said_leaves_nothing() {
    let s = setup().await;
    let now = time_time();
    let opening = said()[..2].to_vec();
    s.fake.lock().unwrap().sessions.insert("live1".into(), call(now - 20.0, now, opening));
    let agent = s.app.app.state.config().agent(Some("Wellness")).unwrap().clone();
    assert!(bipbridge::live_calls::end_call(&s.app.app.state, Ending::hangup(agent, "api_main", "live1", None))
        .await
        .is_none());
    assert!(records_of(&s.app, "api_main").await.is_empty());
}

#[tokio::test]
async fn the_routes_check_what_they_get() {
    let s = setup().await;
    let (k, v) = auth();
    let a = [(k, v.as_str())];
    assert_eq!(s.app.get("/v1/live/calls?agent=Wellness&session=x", &[]).await.status, 401);
    assert_eq!(s.app.get("/v1/live/calls?agent=Nobody&session=x", &a).await.status, 404);
    assert_eq!(s.app.get("/v1/live/calls?agent=Wellness", &a).await.status, 422);
    assert_eq!(s.app.get("/v1/live/calls?agent=Wellness&session=x", &a).await.json(), json!({"calls": []}));
    let resp = s
        .app
        .send_json(
            "POST",
            "/v1/live/calls/start",
            &a,
            &json!({"agent": "Wellness", "session": "", "live_session": "l"}),
        )
        .await;
    assert_eq!(resp.status, 422);
}
