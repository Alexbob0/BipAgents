//! Bot Chat turns taken alone -> pushes, sub-task reports left without a reply -> pushes (routines: routines.rs)
//! (Python: tests/test_cronwatch.py). Hermes is a small fake; the outbox, the pushes and the hub are the app's own.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bipbridge::config::{parse_config, AgentConfig};
use bipbridge::cronwatch::{CronWatcher, Presence};
use bipbridge::runs::Listener;
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

const DEVICE: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[derive(Default)]
struct HermesState {
    sessions: Vec<Value>,

    messages: HashMap<String, Vec<Value>>,
    message_calls: usize,
    /// Hermes fails to read any session's messages for now.
    failing: bool,
}

#[derive(Clone, Default)]
struct FakeHermes(Arc<Mutex<HermesState>>);

impl FakeHermes {
    fn with<R>(&self, f: impl FnOnce(&mut HermesState) -> R) -> R {
        f(&mut self.0.lock().unwrap())
    }
    fn set_count(&self, sid: &str, count: i64) {
        self.with(|s| {
            for session in s.sessions.iter_mut() {
                if session["id"] == sid {
                    session["message_count"] = json!(count);
                }
            }
        });
    }
    fn push_messages(&self, sid: &str, more: Value) {
        self.with(|s| s.messages.entry(sid.to_string()).or_default().extend(more.as_array().unwrap().iter().cloned()));
    }
}

/// Hermes' list: the sessions started from another one (`parent_session_id`) only with `include_children`, and only
/// the ones of a `source` when it is given.
async fn sessions(State(h): State<FakeHermes>, Query(q): Query<HashMap<String, String>>) -> Json<Value> {
    let children = q.get("include_children").is_some_and(|v| v == "1" || v == "true");
    let rows: Vec<Value> = h
        .with(|s| s.sessions.clone())
        .into_iter()
        .filter(|r| children || r["parent_session_id"].as_str().is_none_or(str::is_empty))
        .filter(|r| q.get("source").is_none_or(|source| r["source"].as_str() == Some(source.as_str())))
        .collect();
    Json(json!({"sessions": rows}))
}

/// A session's messages (`broken_…`: Hermes fails to read it).
async fn messages(State(h): State<FakeHermes>, Path(sid): Path<String>) -> Response {
    if sid.starts_with("broken_") || h.with(|s| s.failing) {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "database is locked"}))).into_response();
    }
    Json(json!({"messages": h.with(|s| {
        s.message_calls += 1;
        s.messages.get(&sid).cloned().unwrap_or_default()
    })}))
    .into_response()
}

/// One session (`GET /api/sessions/{sid}`): 404 when Hermes no longer has it.
async fn session(State(h): State<FakeHermes>, Path(sid): Path<String>) -> Response {
    match h.with(|s| s.sessions.iter().find(|r| r["id"] == sid.as_str()).cloned()) {
        Some(row) => Json(json!({"session": row})).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "Session not found"}))).into_response(),
    }
}

/// A run that goes on (the app follows it): its stream never ends.
async fn events() -> Response {
    let stream = futures::stream::pending::<Result<bytes::Bytes, std::io::Error>>();
    Response::builder().header("content-type", "text/event-stream").body(axum::body::Body::from_stream(stream)).unwrap()
}

struct World {
    _dir: tempfile::TempDir,
    app: TestApp,
    cron: CronWatcher,
    hermes: FakeHermes,
    apns: FakeApns,
    agent: AgentConfig,
}

/// `watcher(config_dict)` (bot = false) or `bot_watcher(config_dict)` (bot = true: hub, pushes and presence).
async fn world(bot: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let hermes = FakeHermes::default();
    let url = spawn_fake(
        Router::new()
            .route("/api/sessions", get(sessions))
            .route("/api/sessions/{sid}", get(session))
            .route("/api/sessions/{sid}/messages", get(messages))
            .route("/v1/runs/{run}/events", get(events))
            .fallback(|| async { (StatusCode::NOT_FOUND, Json(json!({"error": "no route"}))).into_response() })
            .with_state(hermes.clone()),
    )
    .await;
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &Urls { hermes: url, ..Urls::default() });
    data["apns"]["base_url"] = json!(apns.url);
    // the agents' profiles (their routines' `cron/jobs.json`) in the test's own folder
    data["hermes"] = json!({"home": dir.path().join("hermes").to_string_lossy()});
    // the push alone, as before the agent wrote its own message (handback.rs; the black-box suite covers that)
    data["cron"]["subtask_handback"] = json!(false);
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let s = app.app.state.clone();
    let (k, v) = auth();
    let resp = app
        .send_json(
            "POST",
            "/v1/devices",
            &[(k, v.as_str())],
            &json!({"token": DEVICE, "environment": "sandbox", "agent_ids": ["wellness"]}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let (hub, push, presence) = if bot {
        (Some(s.hub.clone()), Some(s.push.clone()), Some(Arc::new(Presence::new(120.0))))
    } else {
        (None, None, None)
    };
    let cron = CronWatcher::new(s.config.clone(), s.hermes.clone(), s.outbox.clone(), 60.0, hub, push, presence);
    let agent = s.config.get().agent(Some("wellness")).cloned().unwrap();
    World { _dir: dir, app, cron, hermes, apns, agent }
}

impl World {
    async fn poll(&self) {
        self.cron.poll(&self.agent).await.unwrap()
    }
    /// The pushes, as Python's FakePush records them: (agent, text or ("subtask", line), session_id).
    async fn replies(&self) -> Vec<(String, String, String)> {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let mut out = Vec::new();
        for p in self.apns.payloads() {
            // the journal row's text (GET /v1/notifications/{n}, what the extension shows)
            let (k, v) = auth();
            let row = self.app.get(&format!("/v1/notifications/{}", p["n"].as_str().unwrap()), &[(k, v.as_str())]).await.json();
            let text = row["preview"].as_str().map(str::to_string);
            out.push({
                let shown = if row["type"] == "subtask" && p["aps"]["alert"]["body"] == "notif.subtask" {
                    format!("subtask:{}", text.unwrap_or_else(|| "None".into()))
                } else {
                    text.unwrap_or_default()
                };
                (p["agent"].as_str().unwrap().to_string(), shown, row["session_id"].as_str().unwrap_or("").to_string())
            });
        }
        out
    }
    /// The app follows a run of `session_id` (Python's FakeHub.followed).
    fn follow(&self, session_id: &str) -> Listener {
        let listener = self.app.app.state.hub.subscribe(&self.agent, &format!("run_{session_id}"), "app", false);
        listener.sub.set_session_id(Some(session_id.to_string()));
        listener
    }
}


fn reply(agent: &str, text: &str, sid: &str) -> (String, String, String) {
    (agent.to_string(), text.to_string(), sid.to_string())
}

fn bot_chat(w: &World, sid: &str, count: i64, history: Value) {
    w.hermes.with(|s| {
        s.sessions = vec![json!({"id": sid, "title": "Bot Chat", "message_count": count})];
        s.messages.insert(sid.into(), history.as_array().unwrap().clone());
    });
}

#[tokio::test]
async fn bot_chat_turn_taken_alone_is_pushed_once() {
    let w = world(true).await;
    bot_chat(
        &w,
        "api_bot",
        4,
        json!([{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Ancienne réponse"}]),
    );
    w.poll().await; // baseline: nothing old is pushed
    assert!(w.replies().await.is_empty());
    w.hermes.set_count("api_bot", 6);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "[Routine] Point de midi"}, {"role": "assistant", "content": "Wellness dit que ta nuit était bonne."}]),
    );
    w.poll().await; // first sight: wait until it stops changing
    w.poll().await;
    assert_eq!(w.replies().await, [reply("wellness", "Wellness dit que ta nuit était bonne.", "api_bot")]);
    let payload = w.apns.payloads()[0].clone();
    assert_eq!(payload["r"], "session:api_bot");
    assert_eq!(payload["t"], "reply");
    assert_eq!(w.apns.requests.lock().unwrap()[0].header("apns-collapse-id").as_deref(), None); // a reply never replaces another (one journal row each)
    w.poll().await;
    assert_eq!(w.replies().await.len(), 1);
    w.app.shutdown().await;
}

#[tokio::test]
async fn a_bot_chat_reply_whose_read_failed_is_pushed_at_the_next_poll() {
    let w = world(true).await;
    bot_chat(&w, "api_bot", 2, json!([{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Salut"}]));
    w.poll().await; // baseline
    w.hermes.set_count("api_bot", 4);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "[Routine] Point"}, {"role": "assistant", "content": "Tout va bien."}]),
    );
    w.hermes.with(|s| s.failing = true);
    w.poll().await; // the count moved, but Hermes could not be read: not counted as seen
    w.poll().await;
    w.hermes.with(|s| s.failing = false);
    w.poll().await; // read: a candidate
    w.poll().await; // unchanged: pushed
    assert_eq!(w.replies().await, [reply("wellness", "Tout va bien.", "api_bot")]);
    w.app.shutdown().await;
}

#[tokio::test]
async fn bot_chat_reply_followed_or_on_screen_is_not_pushed() {
    let mut w = world(true).await;
    bot_chat(
        &w,
        "api_bot",
        2,
        json!([{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]),
    );
    w.poll().await;
    w.hermes.set_count("api_bot", 4);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "Et demain ?"}, {"role": "assistant", "content": "Grand soleil."}]),
    );
    let listener = w.follow("api_bot"); // the app sent it and followed the run
    for _ in 0..2 {
        w.poll().await;
    }
    drop(listener);
    w.cron.hub = None; // followed.clear(): the run is not this conversation's any more
    w.hermes.set_count("api_bot", 6);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "Message from 🤖 Vie (@vie): ?"}, {"role": "assistant", "content": "Vu."}]),
    );
    w.cron.presence.as_ref().unwrap().touch("wellness", "api_bot"); // the conversation is open on the phone
    for _ in 0..2 {
        w.poll().await;
    }
    assert!(w.replies().await.is_empty());
    w.app.shutdown().await;
}

#[tokio::test]
async fn answer_to_another_agent_is_not_pushed() {
    let w = world(true).await;
    bot_chat(
        &w,
        "api_bot",
        2,
        json!([{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]),
    );
    w.poll().await;
    w.hermes.set_count("api_bot", 4);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "Message from 🤖 Vie (@vie): Comment était la nuit d'Alex ?"},
               {"role": "assistant", "content": "Nuit correcte, 7 h 10, score 82."}]),
    );
    for _ in 0..2 {
        w.poll().await;
    }
    assert!(w.replies().await.is_empty()); // Vie reports back to the user: one notification, from Vie
    w.app.shutdown().await;
}

#[tokio::test]
async fn teammate_answer_to_our_own_request_is_pushed() {
    let w = world(true).await;
    bot_chat(
        &w,
        "api_vie",
        2,
        json!([{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]),
    );
    w.poll().await;
    w.hermes.set_count("api_vie", 6);
    w.hermes.push_messages(
        "api_vie",
        json!([
            {"role": "user", "content": "Demande à Wellness comment était ma nuit"},
            {"role": "assistant", "content": "Je demande à Wellness.",
             "tool_calls": [{"function": {"name": "message_agent", "arguments": "{\"target\": \"wellness\", \"message\": \"Nuit d'Alex ?\"}"}}]},
            {"role": "user", "content": "Message from 🤖 Wellness (@wellness): Nuit correcte, 7 h 10."},
            {"role": "assistant", "content": "Wellness dit : nuit correcte, 7 h 10."},
        ]),
    );
    for _ in 0..2 {
        w.poll().await;
    }
    assert_eq!(w.replies().await, [reply("wellness", "Wellness dit : nuit correcte, 7 h 10.", "api_vie")]);
    w.app.shutdown().await;
}

const REPORT: &str =
    "[ASYNC DELEGATION BATCH COMPLETE — deleg_1194136c]\n\nTask 1 (completed): mini podcast generated, \
                      saved to /home/hermes/.hermes/media/podcast_jeudi0810.mp3 (58 s).";

#[tokio::test]
async fn a_report_left_without_a_reply_is_pushed_once() {
    let mut w = world(true).await;
    bot_chat(
        &w,
        "api_bot",
        2,
        json!([{"role": "user", "content": "Un podcast ?"}, {"role": "assistant", "content": "Relancé : le sous-agent régénère le podcast."}]),
    );
    w.poll().await;
    let listener = w.follow("api_bot"); // the app followed « Relancé… »
    w.hermes.set_count("api_bot", 3);
    w.hermes.push_messages("api_bot", json!([{"role": "user", "content": REPORT}])); // Hermes: stored, no wake turn
    w.poll().await; // first sight: maybe the agent wakes up
    assert!(w.replies().await.is_empty());
    w.poll().await; // still last: nobody will reply
    assert_eq!(w.replies().await, [reply("wellness", "subtask:🎧 Podcast jeudi0810", "api_bot")]);
    let payload = w.apns.payloads()[0].clone();
    assert_eq!(payload["t"], "subtask");
    assert_eq!(
        payload["aps"]["alert"],
        json!({"loc-key": "notif.subtask", "body": "notif.subtask"})
    );
    assert_eq!(w.apns.requests.lock().unwrap()[0].header("apns-collapse-id").as_deref(), None);
    for _ in 0..2 {
        w.poll().await;
    }
    assert_eq!(w.replies().await.len(), 1);
    drop(listener);
    w.cron.hub = None;
    w.app.shutdown().await;
}

#[tokio::test]
async fn a_report_the_agent_answers_is_not_pushed_twice() {
    let w = world(true).await;
    bot_chat(
        &w,
        "api_bot",
        2,
        json!([{"role": "user", "content": "Un podcast ?"}, {"role": "assistant", "content": "C'est lancé."}]),
    );
    w.poll().await;
    w.hermes.set_count("api_bot", 3);
    w.hermes.push_messages("api_bot", json!([{"role": "user", "content": REPORT}]));
    w.poll().await;
    w.hermes.set_count("api_bot", 4); // this Hermes wakes the agent: it delivers
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "assistant", "content": "MEDIA:/m/podcast.mp3\nVoilà ton podcast !"}]),
    );
    for _ in 0..3 {
        w.poll().await;
    }
    assert_eq!(w.replies().await, [reply("wellness", "MEDIA:/m/podcast.mp3\nVoilà ton podcast !", "api_bot")]);
    w.app.shutdown().await;
}

#[tokio::test]
async fn a_report_in_another_conversation_is_pushed_unless_on_screen() {
    let w = world(true).await;
    w.hermes.with(|s| {
        s.sessions = vec![
            json!({"id": "api_x", "title": "Recherche", "message_count": 2}),
            json!({"id": "api_y", "title": "Autre", "message_count": 2}),
        ];
        for sid in ["api_x", "api_y"] {
            s.messages.insert(
                sid.into(),
                vec![json!({"role": "user", "content": "Cherche"}), json!({"role": "assistant", "content": "Lancé."})],
            );
        }
    });
    w.poll().await; // baseline, nothing fetched
    assert_eq!(w.hermes.with(|s| s.message_calls), 0);
    for sid in ["api_x", "api_y"] {
        w.hermes.set_count(sid, 3);
        w.hermes.push_messages(
            sid,
            json!([{"role": "user", "content": "[ASYNC DELEGATION BATCH COMPLETE — d] 3 sources found."}]),
        );
    }
    w.cron.presence.as_ref().unwrap().touch("wellness", "api_y");
    for _ in 0..3 {
        w.poll().await;
    }
    assert_eq!(w.replies().await, [reply("wellness", "subtask:None", "api_x")]);
    w.app.shutdown().await;
}
