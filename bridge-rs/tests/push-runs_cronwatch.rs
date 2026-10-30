//! Cron sessions -> outbox, Bot Chat turns taken alone -> pushes, sub-task reports left without a reply -> pushes
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
    if sid.starts_with("broken_") {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": "database is locked"}))).into_response();
    }
    Json(json!({"messages": h.with(|s| {
        s.message_calls += 1;
        s.messages.get(&sid).cloned().unwrap_or_default()
    })}))
    .into_response()
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
async fn a_restart_corrects_a_routine_s_reply_without_pushing_it_again() {
    // aibox 2026-10-26: after `bipagents update`, the morning podcast (stored that morning by the older bridge, its
    // text written differently) was read again within the window, replaced, and pushed a second time.
    let w = world(false).await;
    let sid = "cron_fc344ed09026_20261005_075008";
    w.hermes.with(|s| {
        s.sessions = vec![json!({"id": sid, "title": "Podcast du matin"})];
        s.messages.insert(sid.into(), vec![json!({"role": "assistant", "content": "Bonjour Alex !"})]);
    });
    assert_eq!(w.poll(now()).await, 0);
    assert_eq!(w.poll(now()).await, 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let pushed = w.apns.payloads().len();
    assert_eq!(pushed, 1);
    let s = w.app.app.state.clone();
    let id = format!("hermes-session:{sid}");
    assert!(s.store.replace_outbox("wellness", &id, "ce que l'ancien bridge avait écrit", None).await.unwrap().is_some());
    // the bridge restarts: a new watcher, nothing in memory
    let again = CronWatcher::new(s.config.clone(), s.hermes.clone(), s.outbox.clone(), 60.0, 6.0, None, None, None, None);
    assert_eq!(again.poll(&w.agent, Some(now())).await.unwrap(), 0);
    assert_eq!(again.poll(&w.agent, Some(now())).await.unwrap(), 1); // corrected in the Boîte
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(w.outbox().await[0]["text"], "Bonjour Alex !");
    assert_eq!(w.apns.payloads().len(), pushed); // and not pushed again
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

/// The watcher as the app starts it (app.rs `start_background`): the store, the hub, the pushes and the presence.
fn production_watcher(w: &World) -> CronWatcher {
    let s = w.app.app.state.clone();
    CronWatcher::new(
        s.config.clone(),
        s.hermes.clone(),
        s.outbox.clone(),
        30.0,
        12.0,
        Some(s.store.clone()),
        Some(s.hub.clone()),
        Some(s.push.clone()),
        Some(Arc::new(Presence::new(45.0))),
    )
}

/// A morning routine (Hermes cron) that made a two-voice podcast with create_podcast and ended on its text and the
/// podcast's `MEDIA:` line, listed among the agent's other conversations.
fn podcast_routine(w: &World, sid: &str, started: f64, audio: &str) {
    let prompt = "<!--bip:now Friday, October 30, 2026, 07:53, Europe/Paris (UTC+01:00)-->\nPrépare le podcast du matin.";
    w.hermes.with(|s| {
        s.sessions = vec![
            json!({"id": "api_bot", "title": "Bot Chat", "source": "api_server", "message_count": 2,
                   "started_at": started - 86_400.0}),
            json!({"id": "api_chat", "title": "Courses", "source": "api_server", "message_count": 4,
                   "started_at": started - 3_600.0}),
            json!({"id": sid, "title": "Routine du matin", "source": "cron", "message_count": 4,
                   "started_at": started}),
        ];
        s.messages.insert(
            "api_bot".into(),
            vec![json!({"role": "user", "content": "Salut"}), json!({"role": "assistant", "content": "Salut !"})],
        );
        s.messages.insert(
            sid.into(),
            vec![
                json!({"role": "user", "content": prompt}),
                json!({"role": "assistant", "content": "",
                       "tool_calls": [{"id": "c1", "type": "function", "function": {
                           "name": "mcp_bipagents_create_podcast", "arguments": "{\"title\": \"Nuit\"}"}}]}),
                json!({"role": "tool", "tool_call_id": "c1",
                       "content": format!("MEDIA:{audio}\nAudio ready (1 min 56 s): to give it to the person, put the \
MEDIA line above alone at the end of your reply, exactly as it is.")}),
                json!({"role": "assistant",
                       "content": format!("🎙️ Ta nuit en deux voix : 7 h 12 de sommeil, score 81.\n\nMEDIA:{audio}"),
                       "tool_calls": []}),
            ],
        );
    });
}

#[tokio::test]
async fn a_routines_podcast_reaches_the_boite_and_is_pushed() {
    // aibox 2026-10-29 and 30: the 07:53 routine made its two-voice podcast, its reply ended on the right MEDIA line,
    // the job was « ok »… and nothing reached the app.
    let w = world(false).await;
    let cron = production_watcher(&w);
    let started = chrono::Local::now() - chrono::Duration::minutes(3);
    let audio = podcast_made(&w, started);
    let sid = format!("cron_5d1c0f2a9b3e_{}", started.format("%Y%m%d_%H%M%S"));
    podcast_routine(&w, &sid, started.timestamp() as f64, &audio);
    let now = chrono::Local::now().naive_local();
    let mut stored = 0;
    for _ in 0..3 {
        stored += cron.poll(&w.agent, Some(now)).await.unwrap();
    }
    assert_eq!(stored, 1);
    let items = w.outbox().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["session_id"], json!(sid));
    assert!(items[0]["text"].as_str().unwrap().ends_with(&format!("MEDIA:{audio}")), "{:?}", items[0]["text"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let pushed = w.apns.payloads();
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    w.app.shutdown().await;
}

/// The podcast's file in the bridge's audio folder; its `MEDIA:` path. Not noted in `made_files::MADE`: the reply
/// carries it already, and that list is the process's (it would reach the other tests' routines).
fn podcast_made(w: &World, started: chrono::DateTime<chrono::Local>) -> String {
    let config = w.app.app.state.config.get();
    let file = format!("{}-podcast.m4a", started.format("%Y%m%d-%H%M%S"));
    let folder = bipbridge::agent_tools::audio_root(&config).join("wellness");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(&file), b"m4a").unwrap();
    format!("/bipagents/audio/wellness/{file}")
}

#[tokio::test]
async fn a_routine_hermes_continued_after_compressing_its_context_is_delivered() {
    // The routine read the sleep data and the last days' outputs, then Hermes compressed its context: the cron session
    // ends on a tool's result (end_reason « compression ») and the run goes on in a new session started from it, which
    // Hermes' plain list leaves out. Its final reply, podcast included, must reach the app.
    let w = world(false).await;
    let cron = production_watcher(&w);
    let started = chrono::Local::now() - chrono::Duration::minutes(4);
    let at = started.timestamp() as f64;
    let audio = podcast_made(&w, started);
    let root = format!("cron_5d1c0f2a9b3e_{}", started.format("%Y%m%d_%H%M%S"));
    let tip = format!("{}_a1b2c3", (started + chrono::Duration::minutes(2)).format("%Y%m%d_%H%M%S"));
    let helper = format!("{}_d4e5f6", (started + chrono::Duration::minutes(1)).format("%Y%m%d_%H%M%S"));
    w.hermes.with(|s| {
        s.sessions = vec![
            json!({"id": tip, "source": "cron", "parent_session_id": root, "started_at": at + 120.0, "title": null}),
            json!({"id": helper, "source": "cron", "parent_session_id": root, "started_at": at + 60.0}),
            json!({"id": "api_bot", "title": "Bot Chat", "source": "api_server", "message_count": 2, "started_at": at - 9e4}),
            json!({"id": root, "title": "Routine du matin", "source": "cron", "end_reason": "compression",
                   "message_count": 3, "started_at": at}),
        ];
        s.messages.insert(
            root.clone(),
            vec![
                json!({"role": "user", "content": "Prépare le podcast du matin."}),
                json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c1", "type": "function",
                       "function": {"name": "mcp_bipagents_health_read", "arguments": "{}"}}]}),
                json!({"role": "tool", "tool_call_id": "c1", "content": "{\"sleep\": \"7 h 12\"}"}),
            ],
        );
        // a sub-agent's run started from the routine: never delivered on its own
        s.messages.insert(helper.clone(), vec![json!({"role": "assistant", "content": "Script du sous-agent prêt."})]);
        s.messages.insert(
            tip.clone(),
            vec![
                json!({"role": "user", "content": "[CONTEXT COMPACTION] Résumé : nuit de 7 h 12, score 81…"}),
                json!({"role": "assistant", "content": "", "tool_calls": [{"id": "c2", "type": "function",
                       "function": {"name": "mcp_bipagents_create_podcast", "arguments": "{}"}}]}),
                json!({"role": "tool", "tool_call_id": "c2", "content": format!("MEDIA:{audio}\nAudio ready (1 min 56 s).")}),
                json!({"role": "assistant", "content": format!("🎙️ Ta nuit en deux voix.\n\nMEDIA:{audio}")}),
            ],
        );
    });
    let now = chrono::Local::now().naive_local();
    let mut stored = 0;
    for _ in 0..3 {
        stored += cron.poll(&w.agent, Some(now)).await.unwrap();
    }
    assert_eq!(stored, 1);
    let items = w.outbox().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["text"], json!(format!("🎙️ Ta nuit en deux voix.\n\nMEDIA:{audio}")));
    assert_eq!(items[0]["title"], "Routine du matin");
    assert_eq!(items[0]["session_id"], json!(root)); // one message per run, under the routine's own session
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(w.apns.payloads().len(), 1);
    for _ in 0..2 {
        assert_eq!(cron.poll(&w.agent, Some(now)).await.unwrap(), 0); // once
    }
    w.app.shutdown().await;
}

#[tokio::test]
async fn a_conversation_hermes_cannot_read_never_holds_a_routine_back() {
    let w = world(true).await;
    let cron = production_watcher(&w);
    let started = chrono::Local::now() - chrono::Duration::minutes(3);
    let audio = podcast_made(&w, started);
    let sid = format!("cron_5d1c0f2a9b3e_{}", started.format("%Y%m%d_%H%M%S"));
    podcast_routine(&w, &sid, started.timestamp() as f64, &audio);
    w.hermes.with(|s| {
        // listed before the routine, its messages fail every time
        s.sessions.insert(0, json!({"id": "broken_bot", "title": "Bot Chat", "message_count": 9}));
        s.sessions.insert(1, json!({"id": "broken_chat", "title": "Recettes", "message_count": 3}));
    });
    let now = chrono::Local::now().naive_local();
    w.hermes.set_count("broken_chat", 4); // moved: fetched
    let mut stored = 0;
    for _ in 0..3 {
        stored += cron.poll(&w.agent, Some(now)).await.unwrap();
        w.hermes.with(|s| s.sessions[1]["message_count"] = json!(s.sessions[1]["message_count"].as_i64().unwrap() + 1));
    }
    assert_eq!(stored, 1);
    assert_eq!(w.outbox().await.len(), 1);
    w.app.shutdown().await;
}
