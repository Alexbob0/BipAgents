//! Cron sessions -> outbox, Bot Chat turns taken alone -> pushes, sub-task reports left without a reply -> pushes
//! (Python: tests/test_cronwatch.py). Hermes is a small fake; the outbox, the pushes and the hub are the app's own.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bipbridge::config::{parse_config, AgentConfig};
use bipbridge::cronwatch::{CronWatcher, Presence};
use bipbridge::runs::Listener;
use chrono::{NaiveDate, NaiveDateTime};
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

const DEVICE: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

#[derive(Default)]
struct HermesState {
    sessions: Vec<Value>,
    messages: HashMap<String, Vec<Value>>,
    message_calls: usize,
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

async fn sessions(State(h): State<FakeHermes>) -> Json<Value> {
    Json(json!({"sessions": h.with(|s| s.sessions.clone())}))
}

async fn messages(State(h): State<FakeHermes>, Path(sid): Path<String>) -> Json<Value> {
    Json(json!({"messages": h.with(|s| {
        s.message_calls += 1;
        s.messages.get(&sid).cloned().unwrap_or_default()
    })}))
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
            .route("/api/sessions/{sid}/messages", get(messages))
            .route("/v1/runs/{run}/events", get(events))
            .fallback(|| async { (StatusCode::NOT_FOUND, Json(json!({"error": "no route"}))).into_response() })
            .with_state(hermes.clone()),
    )
    .await;
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &Urls { hermes: url, ..Urls::default() });
    data["apns"]["base_url"] = json!(apns.url);
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
    let cron =
        CronWatcher::new(s.config.clone(), s.hermes.clone(), s.outbox.clone(), 60.0, 6.0, None, hub, push, presence);
    let agent = s.config.get().agent(Some("wellness")).cloned().unwrap();
    World { _dir: dir, app, cron, hermes, apns, agent }
}

impl World {
    async fn poll(&self, now: NaiveDateTime) -> usize {
        self.cron.poll(&self.agent, Some(now)).await.unwrap()
    }
    async fn outbox(&self) -> Vec<Value> {
        let (k, v) = auth();
        self.app.get("/v1/outbox", &[(k, v.as_str())]).await.json()["items"].as_array().unwrap().clone()
    }
    /// The pushes, as Python's FakePush records them: (agent, text or ("subtask", line), session_id).
    async fn replies(&self) -> Vec<(String, String, String)> {
        tokio::time::sleep(Duration::from_millis(30)).await;
        let push = self.app.app.state.push.clone();
        self.apns
            .payloads()
            .iter()
            .map(|p| {
                let text = p["reply_id"].as_str().and_then(|id| push.text(id));
                let shown = if p["run_id"].as_str().unwrap().starts_with("session:")
                    && p["aps"]["alert"]["body"] == "Sous-tâche terminée"
                {
                    format!("subtask:{}", text.unwrap_or_else(|| "None".into()))
                } else {
                    text.unwrap_or_default()
                };
                (p["agent"].as_str().unwrap().to_string(), shown, p["session_id"].as_str().unwrap_or("").to_string())
            })
            .collect()
    }
    /// The app follows a run of `session_id` (Python's FakeHub.followed).
    fn follow(&self, session_id: &str) -> Listener {
        let listener = self.app.app.state.hub.subscribe(&self.agent, &format!("run_{session_id}"), "app", false);
        listener.sub.set_session_id(Some(session_id.to_string()));
        listener
    }
}

fn now() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 5).unwrap().and_hms_opt(8, 30, 0).unwrap()
}

fn reply(agent: &str, text: &str, sid: &str) -> (String, String, String) {
    (agent.to_string(), text.to_string(), sid.to_string())
}

#[tokio::test]
async fn finished_cron_reply_goes_to_the_outbox_once() {
    let w = world(false).await;
    let sid = "cron_fc344ed09026_20261005_075008";
    w.hermes.with(|s| {
        s.sessions = vec![json!({"id": sid, "title": "Podcast du matin"}), json!({"id": "api_123", "title": "Chat"})];
        s.messages.insert(
            sid.into(),
            vec![
                json!({"role": "user", "content": "Produis le point"}),
                json!({"role": "assistant", "content": "Bonjour Alex !"}),
            ],
        );
    });
    assert_eq!(w.poll(now()).await, 0); // first sight: wait until it stops changing
    assert_eq!(w.poll(now()).await, 1); // unchanged: stored
    let items = w.outbox().await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["text"], "Bonjour Alex !");
    assert_eq!(items[0]["title"], "Podcast du matin");
    assert_eq!(items[0]["session_id"], sid);
    let calls = w.hermes.with(|s| s.message_calls);
    assert_eq!(w.poll(now()).await, 0); // done: not fetched again
    assert_eq!(w.hermes.with(|s| s.message_calls), calls);
    w.app.shutdown().await;
}

#[tokio::test]
async fn silent_old_and_running_sessions_are_skipped() {
    let w = world(false).await;
    w.hermes.with(|s| {
        s.sessions = vec![
            json!({"id": "cron_a_20261005_060000"}),
            json!({"id": "cron_b_20261001_060000"}),
            json!({"id": "cron_c_20261005_082900"}),
        ];
        s.messages.insert("cron_a_20261005_060000".into(), vec![json!({"role": "assistant", "content": "[SILENT]"})]);
        s.messages
            .insert("cron_b_20261001_060000".into(), vec![json!({"role": "assistant", "content": "Vieux message"})]);
        s.messages
            .insert("cron_c_20261005_082900".into(), vec![json!({"role": "user", "content": "Produis le point"})]);
    });
    for _ in 0..3 {
        w.poll(now()).await;
    }
    assert!(w.outbox().await.is_empty());
    w.app.shutdown().await;
}

#[test]
fn job_helpers() {
    use bipbridge::cronwatch::{job_id, job_name};
    assert_eq!(job_id("cron_fc344ed09026_20261005_075008").as_deref(), Some("fc344ed09026"));
    assert_eq!(job_id("api_123"), None);
    assert_eq!(job_name(Some("Podcast du matin · Oct 05 07:53")).as_deref(), Some("Podcast du matin"));
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
    w.poll(now()).await; // baseline: nothing old is pushed
    assert!(w.replies().await.is_empty());
    w.hermes.set_count("api_bot", 6);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "[Routine] Point de midi"}, {"role": "assistant", "content": "Wellness dit que ta nuit était bonne."}]),
    );
    w.poll(now()).await; // first sight: wait until it stops changing
    w.poll(now()).await;
    assert_eq!(w.replies().await, [reply("wellness", "Wellness dit que ta nuit était bonne.", "api_bot")]);
    let payload = w.apns.payloads()[0].clone();
    assert_eq!(payload["run_id"], "session:api_bot");
    assert_eq!(payload["kind"], "reply");
    assert_eq!(w.apns.requests.lock().unwrap()[0].header("apns-collapse-id").as_deref(), Some("session:api_bot"));
    w.poll(now()).await;
    assert_eq!(w.replies().await.len(), 1);
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
    w.poll(now()).await;
    w.hermes.set_count("api_bot", 4);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "Et demain ?"}, {"role": "assistant", "content": "Grand soleil."}]),
    );
    let listener = w.follow("api_bot"); // the app sent it and followed the run
    for _ in 0..2 {
        w.poll(now()).await;
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
        w.poll(now()).await;
    }
    assert!(w.replies().await.is_empty());
    w.app.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn session_state_route_records_presence() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = TestApp::new(config(dir.path(), &backend.urls()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = app.get("/v1/sessions/api_bot/state?agent=wellness", &h).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json()["message_count"], 7);
    assert!(app.app.state.presence.viewing("wellness", "api_bot"));
    assert_eq!(app.get("/v1/sessions/bad%20id/state?agent=wellness", &h).await.status, StatusCode::BAD_REQUEST);
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
    w.poll(now()).await;
    w.hermes.set_count("api_bot", 4);
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "user", "content": "Message from 🤖 Vie (@vie): Comment était la nuit d'Alex ?"},
               {"role": "assistant", "content": "Nuit correcte, 7 h 10, score 82."}]),
    );
    for _ in 0..2 {
        w.poll(now()).await;
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
    w.poll(now()).await;
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
        w.poll(now()).await;
    }
    assert_eq!(w.replies().await, [reply("wellness", "Wellness dit : nuit correcte, 7 h 10.", "api_vie")]);
    w.app.shutdown().await;
}

#[tokio::test]
async fn a_comment_before_a_running_tool_is_not_the_final_reply() {
    let w = world(false).await;
    let calling = json!({"role": "assistant", "content": "execute_code est bloqué en cron — je passe par terminal.",
                         "tool_calls": [{"function": {"name": "terminal", "arguments": "{}"}}]});
    let sid = "cron_fc344ed09026_20261006_075008";
    w.hermes.with(|s| {
        s.sessions = vec![json!({"id": sid, "title": "Podcast du matin"})];
        s.messages.insert(sid.into(), vec![json!({"role": "user", "content": "Produis le podcast"}), calling]);
    });
    let at = NaiveDate::from_ymd_opt(2026, 10, 6).unwrap().and_hms_opt(8, 0, 0).unwrap();
    for _ in 0..3 {
        // the synthesis takes minutes: the comment stays unchanged, it is not taken
        w.poll(at).await;
    }
    assert!(w.outbox().await.is_empty());
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
    w.poll(now()).await;
    let listener = w.follow("api_bot"); // the app followed « Relancé… »
    w.hermes.set_count("api_bot", 3);
    w.hermes.push_messages("api_bot", json!([{"role": "user", "content": REPORT}])); // Hermes: stored, no wake turn
    w.poll(now()).await; // first sight: maybe the agent wakes up
    assert!(w.replies().await.is_empty());
    w.poll(now()).await; // still last: nobody will reply
    assert_eq!(w.replies().await, [reply("wellness", "subtask:🎧 Podcast jeudi0810", "api_bot")]);
    let payload = w.apns.payloads()[0].clone();
    assert_eq!(payload["run_id"], "session:api_bot");
    assert_eq!(
        payload["aps"]["alert"],
        json!({"title": "Wellness", "body": "Sous-tâche terminée", "loc-key": "Sous-tâche terminée"})
    );
    assert_eq!(w.apns.requests.lock().unwrap()[0].header("apns-collapse-id").as_deref(), Some("subtask:api_bot"));
    for _ in 0..2 {
        w.poll(now()).await;
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
    w.poll(now()).await;
    w.hermes.set_count("api_bot", 3);
    w.hermes.push_messages("api_bot", json!([{"role": "user", "content": REPORT}]));
    w.poll(now()).await;
    w.hermes.set_count("api_bot", 4); // this Hermes wakes the agent: it delivers
    w.hermes.push_messages(
        "api_bot",
        json!([{"role": "assistant", "content": "MEDIA:/m/podcast.mp3\nVoilà ton podcast !"}]),
    );
    for _ in 0..3 {
        w.poll(now()).await;
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
    w.poll(now()).await; // baseline, nothing fetched
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
        w.poll(now()).await;
    }
    assert_eq!(w.replies().await, [reply("wellness", "subtask:None", "api_x")]);
    w.app.shutdown().await;
}
