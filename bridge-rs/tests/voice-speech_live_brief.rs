//! Port of tests/test_live_brief.py: the Live calls' brief (the last exchanges, the key points the agent keeps up to
//! date in a throwaway session, the calls that continued a conversation) and its routes.

mod common;

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use bipbridge::config::{AgentConfig, SharedConfig};
use bipbridge::hermes::HermesClient;
use bipbridge::live_brief::{
    is_conversation, is_ghost, visible, Briefs, GHOST_AGE_SECONDS, MIN_MESSAGES, POINTS_TITLE, SCRATCH_CHECKS,
};
use bipbridge::py::time_time;
use common::{auth, config_dict, spawn_fake, TestApp, HERMES_KEY};
use serde_json::{json, Map, Value};

#[derive(Default)]
struct Hermes {
    sessions: Vec<Value>,
    messages: std::collections::HashMap<String, Vec<Value>>,
    chats: Vec<Value>,
    patched: Vec<Value>,
    deleted: Vec<String>,
    reply: String,
    /// Hermes' titler writing a deleted scratch session back (an empty row titled from the request), `revive` times.
    revive: usize,
}

impl Hermes {
    fn add(&mut self, id: &str, messages: Vec<Value>, title: &str, last_active: f64) {
        self.sessions.retain(|s| s["id"] != id);
        self.sessions
            .push(json!({"id": id, "title": title, "message_count": messages.len(), "last_active": last_active}));
        self.messages.insert(id.to_string(), messages);
    }
}

type Fake = Arc<Mutex<Hermes>>;

async fn handle(State(fake): State<Fake>, request: Request) -> Response {
    assert_eq!(request.headers()["authorization"], format!("Bearer {HERMES_KEY}"));
    let method = request.method().clone();
    let path = request.uri().path().trim_matches('/').to_string();
    let request_query = request.uri().query().map(str::to_string);
    let parts: Vec<&str> = path.split('/').collect();
    let raw: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Map<String, Value> = if raw.is_empty() { Map::new() } else { serde_json::from_slice(&raw).unwrap() };
    let mut h = fake.lock().unwrap();
    if parts == ["api", "sessions"] && method == "GET" {
        let query = request_query.unwrap_or_default();
        let param = |name: &str| -> Option<usize> {
            query.split('&').find_map(|kv| kv.strip_prefix(&format!("{name}="))?.parse().ok())
        };
        let (offset, limit) = (param("offset").unwrap_or(0), param("limit").unwrap_or(50));
        let mut listed = h.sessions.clone();
        listed.sort_by(|a, b| {
            b["last_active"].as_f64().unwrap().partial_cmp(&a["last_active"].as_f64().unwrap()).unwrap()
        });
        let listed: Vec<Value> = listed.into_iter().skip(offset).take(limit).collect();
        return Json(json!({"sessions": listed})).into_response();
    }
    if parts == ["api", "sessions"] && method == "POST" {
        let id = format!("api_scratch_{}", h.chats.len());
        let title = body["title"].as_str().unwrap().to_string();
        h.add(&id, vec![], &title, time_time());
        return (StatusCode::CREATED, Json(json!({"object": "hermes.session", "session": {"id": id}}))).into_response();
    }
    if parts.len() == 3 && method == "PATCH" {
        let mut entry = Map::new();
        entry.insert("id".into(), json!(parts[2]));
        entry.extend(body.clone());
        h.patched.push(Value::Object(entry));
        if let Some(session) = h.sessions.iter_mut().find(|s| s["id"] == parts[2]) {
            for (k, v) in body {
                session[k] = v;
            }
        }
        return Json(json!({})).into_response();
    }
    if parts.len() == 3 && method == "DELETE" {
        h.deleted.push(parts[2].to_string());
        let before = h.sessions.len();
        h.sessions.retain(|s| s["id"] != parts[2]);
        if h.sessions.len() == before {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "Session not found"}))).into_response();
        }
        let chatted = h.chats.iter().any(|c| c["session"] == parts[2]);
        if chatted && h.revive > 0 {
            h.revive -= 1;
            let id = parts[2].to_string();
            h.sessions.push(json!({"id": id, "title": "Notes on the podcast", "source": "unknown", "message_count": 0,
                                   "last_active": time_time(), "started_at": time_time()}));
            h.messages.insert(id, vec![]);
        }
        return Json(json!({})).into_response();
    }
    if parts.len() == 4 && parts[3] == "messages" {
        return Json(json!({"messages": h.messages.get(parts[2]).cloned().unwrap_or_default()})).into_response();
    }
    if parts.len() == 4 && parts[3] == "chat" {
        let mut entry = Map::new();
        entry.insert("session".into(), json!(parts[2]));
        entry.extend(body);
        h.chats.push(Value::Object(entry));
        return Json(json!({"message": {"role": "assistant", "content": h.reply}})).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn fake_hermes() -> (Fake, String) {
    let fake: Fake = Arc::new(Mutex::new(Hermes {
        reply: "- Alex prépare un podcast pour jeudi.\n- Il dort mal depuis lundi.".into(),
        ..Default::default()
    }));
    let url = spawn_fake(Router::new().fallback(any(handle)).with_state(fake.clone())).await;
    (fake, url)
}

/// `n` messages, the person and the agent in turn (with a tool call in between now and then).
fn talk(n: usize, start: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for i in start..start + n {
        out.push(json!({"role": if i % 2 == 0 { "user" } else { "assistant" }, "content": format!("message {i}")}));
        if i % 5 == 4 {
            out.push(json!({"role": "tool", "content": "{\"result\": 1}"}));
        }
    }
    out
}

struct Setup {
    _dir: tempfile::TempDir,
    fake: Fake,
    briefs: Briefs,
    agent: AgentConfig,
}

async fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = fake_hermes().await;
    let urls = common::Urls { hermes: url, ..Default::default() };
    let config = bipbridge::config::parse_config(&config_dict(dir.path(), &urls), None).unwrap();
    let agent = config.agent(Some("Wellness")).unwrap().clone();
    let briefs = Briefs::new(
        dir.path().join("briefs").to_str().unwrap(),
        HermesClient::new(reqwest::Client::new()),
        SharedConfig::new(config),
    );
    Setup { _dir: dir, fake, briefs, agent }
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[tokio::test]
async fn a_brief_is_the_last_exchanges_without_tools_or_hidden_text() {
    let s = setup().await;
    let mut messages = talk(12, 0);
    messages.push(json!({"role": "user", "content": "<!--bip:call\nThe person: hello\n-->\nEt demain ?"}));
    s.fake.lock().unwrap().add("s1", messages, "Discussion", 0.0);
    let brief = s.briefs.brief(&s.agent, "s1").await.unwrap();
    assert_eq!(brief["session"], "s1");
    assert_eq!(brief["points_at"], Value::Null);
    let text = brief["text"].as_str().unwrap();
    assert!(text.contains("This is a voice call"));
    assert!(text.contains("- The person: Et demain ?"));
    assert!(text.contains("message 11") && !text.contains("message 3")); // the last ones only
    assert!(!text.contains("result") && !text.contains("bip:"));
}

#[tokio::test]
async fn the_key_points_are_written_in_a_throwaway_session_then_kept() {
    let s = setup().await;
    s.fake.lock().unwrap().add("s1", talk(14, 0), "Discussion", 0.0);
    assert!(s.briefs.refresh(&s.agent, "s1", Some(14)).await.unwrap());
    let chat = s.fake.lock().unwrap().chats[0].clone();
    let session = chat["session"].as_str().unwrap().to_string();
    assert!(session.starts_with("api_scratch"));
    assert_eq!(chat["model_options"], json!({"reasoning": {"enabled": false}}));
    let message = chat["message"].as_str().unwrap();
    assert!(message.contains("message 13") && message.contains("(none yet)") && message.contains("Use no tool"));
    // the title again with « user » provenance (Hermes' titler leaves it alone), unique, and hidden
    let patched = s.fake.lock().unwrap().patched.clone();
    assert_eq!(patched.len(), 1);
    let title = patched[0]["title"].as_str().unwrap();
    assert!(title.starts_with(&format!("{POINTS_TITLE} · ")) && title.len() == POINTS_TITLE.len() + " · ".len() + 8);
    assert_eq!(patched[0]["hidden"], true);
    assert_eq!(patched[0]["id"], json!(session));
    assert_eq!(s.fake.lock().unwrap().deleted, vec![session.clone()]); // nothing left in the agent's conversations
    let brief = s.briefs.brief(&s.agent, "s1").await.unwrap();
    assert!(brief["text"].as_str().unwrap().contains("Alex prépare un podcast"));
    assert!(brief["points_at"].as_f64().unwrap() > 0.0);
    // Next time: the previous points and only the messages since.
    {
        let mut h = s.fake.lock().unwrap();
        let mut all = talk(14, 0);
        all.extend(talk(6, 14));
        h.add("s1", all, "Discussion", 0.0);
        h.reply = "- Podcast jeudi, invité confirmé.".into();
    }
    assert!(s.briefs.refresh(&s.agent, "s1", Some(20)).await.unwrap());
    let again = s.fake.lock().unwrap().chats[1]["message"].as_str().unwrap().to_string();
    assert!(
        again.contains("Alex prépare un podcast") && again.contains("message 15") && !again.contains("message 2\n")
    );
    assert!(s.briefs.brief(&s.agent, "s1").await.unwrap()["text"].as_str().unwrap().contains("invité confirmé"));
    // the file as Python writes it
    let saved = s.briefs.load(&s.agent.name, "s1");
    assert_eq!(saved.keys().collect::<Vec<_>>(), ["points", "points_at", "upto", "count"]);
    assert_eq!(saved["count"], 20);
}

#[tokio::test]
async fn which_conversations_are_due() {
    let s = setup().await;
    let (briefs, name) = (&s.briefs, s.agent.name.clone());
    let now = time_time();
    let long_quiet = json!({"id": "a", "title": "Discussion", "message_count": 30, "last_active": now - 600.0});
    let with = |k: &str, v: Value| {
        let mut m = obj(long_quiet.clone());
        m.insert(k.into(), v);
        m
    };
    assert!(briefs.due(&name, &obj(long_quiet.clone()), now));
    assert!(!briefs.due(&name, &with("message_count", json!(MIN_MESSAGES - 1)), now)); // short: no points
    assert!(!briefs.due(&name, &with("last_active", json!(now - 5.0)), now)); // a turn is going on
    for title in ["Live · 10:05", POINTS_TITLE] {
        assert!(!briefs.due(&name, &with("title", json!(title)), now));
    }
    assert!(!briefs.due(&name, &with("id", json!("cron_podcast_20261020_075000")), now));
    briefs.save(&name, "a", &obj(json!({"count": 27}))).unwrap();
    assert!(!briefs.due(&name, &obj(long_quiet.clone()), now)); // 3 new messages only
    briefs.save(&name, "a", &obj(json!({"count": 24}))).unwrap();
    assert!(briefs.due(&name, &obj(long_quiet.clone()), now)); // 6 new
    briefs.save(&name, "a", &obj(json!({"count": 50}))).unwrap();
    assert!(briefs.due(&name, &obj(long_quiet.clone()), now)); // compressed since: fewer than covered
                                                               // an ISO date (Hermes' older answers)
    assert!(!briefs.due(&name, &with("last_active", json!(chrono::Utc::now().to_rfc3339())), now));
}

#[tokio::test]
async fn one_conversation_at_a_time_in_the_background() {
    let s = setup().await;
    {
        let mut h = s.fake.lock().unwrap();
        h.add("old", talk(20, 0), "Discussion", time_time() - 900.0);
        h.add("busy", talk(20, 0), "Discussion", time_time());
        h.add("live", talk(20, 0), "Live · 09:00", time_time() - 900.0);
    }
    assert!(s.briefs.tick(None).await.unwrap());
    {
        let h = s.fake.lock().unwrap();
        assert_eq!(h.chats.len(), 1);
        assert!(h.chats[0]["message"].as_str().unwrap().contains("message 19"));
        let count = h.sessions.iter().find(|x| x["id"] == "old").unwrap()["message_count"].clone();
        assert_eq!(s.briefs.load(&s.agent.name, "old")["count"], count);
    }
    assert!(!s.briefs.tick(None).await.unwrap()); // nothing new since
    assert_eq!(s.fake.lock().unwrap().chats.len(), 1);
}

#[tokio::test]
async fn a_call_continues_in_the_next_briefs() {
    let s = setup().await;
    {
        let mut h = s.fake.lock().unwrap();
        h.add("s1", talk(4, 0), "Discussion", 0.0);
        h.add(
            "live1",
            vec![
                json!({"role": "user", "content": "<!--bip:call-start-->"}),
                json!({"role": "assistant", "content": "Coucou, ça va ?"}),
                json!({"role": "user", "content": "Oui, je pars à Lyon demain."}),
                json!({"role": "assistant", "content": "Bon voyage !"}),
            ],
            "Live · 10:05",
            0.0,
        );
    }
    s.briefs.note_call(&s.agent.name, "s1", "live1").unwrap();
    let text = s.briefs.brief(&s.agent, "s1").await.unwrap()["text"].as_str().unwrap().to_string();
    assert!(text.contains("Earlier voice calls") && text.contains("je pars à Lyon demain") && !text.contains("bip:"));
    let old = json!({"calls": [{"live_session": "live1", "at": time_time() - 2.0 * 86400.0}]});
    s.briefs.save(&s.agent.name, "s1", &obj(old)).unwrap();
    let text = s.briefs.brief(&s.agent, "s1").await.unwrap()["text"].as_str().unwrap().to_string();
    assert!(!text.contains("Earlier voice calls")); // a day old at most
}

#[test]
fn hidden_text_and_conversations() {
    assert_eq!(visible("<!--bip:call\nsecret\n-->\nBonjour"), "Bonjour");
    assert!(is_conversation(&obj(json!({"id": "api_1", "title": "Bot Chat"}))));
    assert!(!is_conversation(&obj(json!({"id": "api_2", "title": "Discussion", "hidden": true}))));
    // an empty row Hermes wrote back after a deletion, even a fresh one
    let ghost = json!({"id": "api_3", "title": "Notes on a cable", "source": "unknown", "message_count": 0});
    assert!(!is_conversation(&obj(ghost)));
    assert!(is_conversation(&obj(
        json!({"id": "api_4", "title": "Nouvelle", "source": "api_server", "message_count": 0})
    )));
    assert!(is_conversation(&obj(json!({"id": "api_5", "title": "Old", "source": "unknown", "message_count": 4}))));
}

#[tokio::test]
async fn routes() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = fake_hermes().await;
    let urls = common::Urls { hermes: url, ..Default::default() };
    let app = TestApp::new(bipbridge::config::parse_config(&config_dict(dir.path(), &urls), None).unwrap());
    {
        let mut h = fake.lock().unwrap();
        h.add("api_old", talk(4, 0), "Discussion", 1.0);
        h.add("api_new", talk(6, 0), "Discussion", 2.0);
        h.add("Live", talk(2, 0), "Live · 11:00", 3.0);
    }
    let (k, v) = auth();
    let a = [(k, v.as_str())];
    assert_eq!(app.get("/v1/live/brief?agent=Wellness", &[]).await.status, 401);
    let latest = app.get("/v1/live/brief?agent=Wellness", &a).await.json();
    assert_eq!(latest["session"], "api_new"); // the latest conversation, not the call
    assert!(latest["text"].as_str().unwrap().contains("message 5"));
    let chosen = app.get("/v1/live/brief?agent=Wellness&session=api_old", &a).await.json();
    assert_eq!(chosen["session"], "api_old");
    assert_eq!(app.get("/v1/live/brief?agent=Nobody", &a).await.status, 404);
    assert_eq!(app.get("/v1/live/brief", &a).await.status, 422);
    let done = app
        .send_json(
            "POST",
            "/v1/live/calls",
            &a,
            &json!({"agent": "Wellness", "session": "api_old", "live_session": "Live"}),
        )
        .await;
    assert_eq!(done.json(), json!({"ok": true}));
    let name = app.app.state.config().agent(Some("Wellness")).unwrap().name.clone();
    assert_eq!(app.app.state.briefs.load(&name, "api_old")["calls"][0]["live_session"], "Live");
    // What the call said waits here for the next written message, whichever device writes it, then is gone.
    let first = "<!--bip:call\nVoice call of 10:05 in between:\nThe person: je pars à Lyon\nYou: Bon voyage\n-->";
    let second = "<!--bip:call\nVoice call of 18:40 in between:\nThe person: je suis rentré\nYou: Super\n-->";
    for recap in [first, second] {
        let resp = app
            .send_json(
                "POST",
                "/v1/live/recaps",
                &a,
                &json!({"agent": "Wellness", "session": "api_old", "recap": recap}),
            )
            .await;
        assert_eq!(resp.json(), json!({"ok": true}));
    }
    let recaps = dir.path().join("data").join("call-recaps").join(&name).join("api_old.txt");
    assert_eq!(std::fs::read_to_string(&recaps).unwrap(), format!("{first}\u{1e}{second}"));
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&recaps).unwrap().permissions().mode() & 0o777, 0o600);
    let taken = app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_old"}))
        .await
        .json();
    let (now, recap) = common::split_now(taken["recap"].as_str().unwrap());
    assert!(now.is_some(), "the current time first");
    assert_eq!(recap, format!("{first}\n{second}"));
    let again = app
        .send_json("POST", "/v1/live/recaps/take", &a, &json!({"agent": "Wellness", "session": "api_old"}))
        .await
        .json();
    assert_eq!(common::without_now(again["recap"].as_str().unwrap()), "", "only the current time");
    let resp = app
        .send_json("POST", "/v1/live/recaps", &[], &json!({"agent": "Wellness", "session": "api_old", "recap": "x"}))
        .await;
    assert_eq!(resp.status, 401);
    let resp = app
        .send_json("POST", "/v1/live/recaps", &a, &json!({"agent": "Wellness", "session": "api_old", "recap": ""}))
        .await;
    assert_eq!(resp.status, 422);
    assert_eq!(resp.json()["detail"][0]["type"], "string_too_short");
}

/// Hermes' titler bills its model call to the scratch session, which writes the row back after its deletion (an empty
/// conversation titled from the request): the bridge deletes it again until Hermes answers 404, then forgets it.
#[tokio::test]
async fn a_scratch_session_written_back_is_deleted_again() {
    let s = setup().await;
    {
        let mut h = s.fake.lock().unwrap();
        h.add("s1", talk(14, 0), "Discussion", 0.0);
        h.revive = 1;
    }
    assert!(s.briefs.refresh(&s.agent, "s1", Some(14)).await.unwrap());
    let scratch = s.fake.lock().unwrap().chats[0]["session"].as_str().unwrap().to_string();
    assert!(s.fake.lock().unwrap().sessions.iter().any(|x| x["id"] == scratch)); // back, as on the owner's phone
    assert_eq!(s.briefs.scratch_pending(), vec![(s.agent.name.clone(), scratch.clone())]);
    let now = time_time();
    assert_eq!(s.briefs.sweep_scratch(now, false).await.unwrap(), 0); // not due yet
    assert_eq!(s.fake.lock().unwrap().deleted.len(), 1);
    assert_eq!(s.briefs.sweep_scratch(now + SCRATCH_CHECKS[0] + 1.0, false).await.unwrap(), 1);
    assert!(!s.fake.lock().unwrap().sessions.iter().any(|x| x["id"] == scratch));
    // Checked again over the next half hour (404 each time), then forgotten.
    let mut t = now + SCRATCH_CHECKS[0] + 1.0;
    for _ in 1..SCRATCH_CHECKS.len() {
        assert_eq!(s.briefs.scratch_pending().len(), 1);
        t += 1900.0;
        assert_eq!(s.briefs.sweep_scratch(t, false).await.unwrap(), 0);
    }
    assert!(s.briefs.scratch_pending().is_empty());
    // Only that id was ever deleted: the conversation is untouched.
    assert!(s.fake.lock().unwrap().deleted.iter().all(|id| *id == scratch));
    assert!(s.fake.lock().unwrap().sessions.iter().any(|x| x["id"] == "s1"));
}

#[tokio::test]
async fn at_the_start_every_scratch_session_is_deleted_again() {
    let s = setup().await;
    {
        let mut h = s.fake.lock().unwrap();
        h.add("s1", talk(14, 0), "Discussion", 0.0);
        h.revive = 1;
    }
    assert!(s.briefs.refresh(&s.agent, "s1", Some(14)).await.unwrap());
    let scratch = s.fake.lock().unwrap().chats[0]["session"].as_str().unwrap().to_string();
    // A new bridge (same data folder): all of them, due or not.
    let again = Briefs::new(&s.briefs.folder, HermesClient::new(reqwest::Client::new()), s.briefs.config.clone());
    assert_eq!(again.sweep_scratch(time_time(), true).await.unwrap(), 1);
    assert!(!s.fake.lock().unwrap().sessions.iter().any(|x| x["id"] == scratch));
    assert_eq!(again.scratch_pending().len(), 1); // still checked over the next half hour
}

fn ghost(id: &str, age: f64) -> Value {
    let at = time_time() - age;
    json!({"id": id, "title": "Notes on a cable", "source": "unknown", "message_count": 0, "started_at": at,
           "last_active": at})
}

#[test]
fn which_sessions_are_ghosts() {
    let now = time_time();
    let old = GHOST_AGE_SECONDS + 60.0;
    let with = |k: &str, v: Value| {
        let mut m = obj(ghost("api_1761200000_ab12cd34", old));
        m.insert(k.into(), v);
        m
    };
    assert!(is_ghost(&obj(ghost("api_1761200000_ab12cd34", old)), now));
    assert!(!is_ghost(&obj(ghost("api_1761200000_ab12cd34", 60.0)), now)); // too young
    assert!(!is_ghost(&with("message_count", json!(2)), now));
    assert!(!is_ghost(&with("message_count", Value::Null), now)); // unknown count: never
    assert!(!is_ghost(&with("source", json!("api_server")), now)); // created by someone (a new conversation)
    assert!(!is_ghost(&with("id", json!("20261023_101500_ab12cd")), now)); // not minted by the API server
    assert!(!is_ghost(&with("id", json!("cron_podcast_20261023_070000")), now));
    assert!(!is_ghost(&with("pinned", json!(true)), now));
    assert!(!is_ghost(&with("title", json!("Bot Chat")), now));
    assert!(!is_ghost(&with("title", json!("Live · 10:05")), now));
    assert!(!is_ghost(&with("last_active", json!(now - 30.0)), now)); // active a moment ago
    let mut no_start = obj(ghost("api_1", old));
    no_start.remove("started_at");
    assert!(!is_ghost(&no_start, now));
}

#[tokio::test]
async fn the_empty_rows_written_back_are_swept() {
    let s = setup().await;
    let old = GHOST_AGE_SECONDS + 600.0;
    {
        let mut h = s.fake.lock().unwrap();
        // Enough sessions for a second page: the ghosts sit far down the list.
        for i in 0..230 {
            h.add(&format!("api_talk{i}"), talk(2, 0), "Discussion", time_time() - i as f64);
        }
        for (id, age) in [("api_ghost1", old), ("api_ghost2", old + 3600.0), ("api_young", 30.0)] {
            h.sessions.push(ghost(id, age));
            h.messages.insert(id.into(), vec![]);
        }
        // a count of 0 but a transcript: kept
        h.sessions.push(ghost("api_counted0", old));
        h.messages.insert("api_counted0".into(), talk(2, 0));
        // a new conversation nobody has written in yet: Hermes' own source, kept
        let mut fresh = ghost("api_fresh", old);
        fresh["source"] = json!("api_server");
        h.sessions.push(fresh);
        let mut pinned = ghost("api_pinned", old);
        pinned["pinned"] = json!(true);
        h.sessions.push(pinned);
    }
    assert_eq!(s.briefs.sweep_ghosts(&s.agent, time_time()).await.unwrap(), 2);
    let mut deleted = s.fake.lock().unwrap().deleted.clone();
    deleted.sort();
    assert_eq!(deleted, ["api_ghost1", "api_ghost2"]);
    assert_eq!(s.briefs.sweep_ghosts(&s.agent, time_time()).await.unwrap(), 0);
}
