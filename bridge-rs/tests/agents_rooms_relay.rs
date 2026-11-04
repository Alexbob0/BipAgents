//! Group discussions, the relay (rooms.rs, rooms_handoff.rs; Alex, 2026-10-31: the agents spoke « chacun leur
//! tour »). Without a goal one agent answers and the hand goes back to the person (a colleague only when named, one
//! hop); a message that looks like a task gets a goal proposed; once accepted, the agents hand the turn to each other
//! (`NEXT:`), the conductor picks when nobody is named, a `PASS` stays silent, and where it stands follows.

mod common;

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bipbridge::config::parse_config;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Fake {
    /// Agent → its next replies.
    script: Arc<Mutex<HashMap<String, VecDeque<String>>>>,
    /// Run id → its output.
    outputs: Arc<Mutex<HashMap<String, String>>>,
    /// (agent, the run's body), in order.
    runs: Arc<Mutex<Vec<(String, Value)>>>,
    /// The small model's answers to the conductor, in order.
    conductor: Arc<Mutex<VecDeque<String>>>,
    asked: Arc<Mutex<Vec<String>>>,
    /// The small model's route (default: pm).
    router: Arc<Mutex<Option<String>>>,
    /// Agent → the events its next run streams (`model.usage`, `tool.started`…); such a run goes on until stopped,
    /// then ends `cancelled` with no usage, as Hermes' runs do.
    streams: Arc<Mutex<HashMap<String, VecDeque<Vec<Value>>>>>,
    /// Run id → its events, while it goes on.
    live: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    /// The runs stopped, in order.
    stopped: Arc<Mutex<Vec<String>>>,
}

async fn backend(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let out = |v: Value| axum::Json(v).into_response();
    if path.ends_with("/chat/completions") {
        let system = body["messages"][0]["content"].as_str().unwrap_or("").to_string();
        fake.asked.lock().unwrap().push(system.clone());
        let answer = if system.contains("coordinate a team") {
            fake.conductor.lock().unwrap().pop_front().unwrap_or_else(|| "none — done".into())
        } else if system.contains("route a message") {
            fake.router.lock().unwrap().clone().unwrap_or_else(|| "pm".into())
        } else if system.contains("real piece of work") {
            "App web de liste de courses".into()
        } else {
            "none".into()
        };
        return out(json!({"choices": [{"message": {"content": answer}}],
                          "usage": {"prompt_tokens": 100, "completion_tokens": 5}}));
    }
    let agent = path.trim_start_matches("/p/").split('/').next().unwrap_or("").to_string();
    if method == "POST" && path.ends_with("/api/sessions") {
        return out(json!({"id": format!("s-{agent}")}));
    }
    if method == "POST" && path.ends_with("/v1/runs") {
        let reply = fake.script.lock().unwrap().get_mut(&agent).and_then(|q| q.pop_front()).unwrap_or("Ok.".into());
        let mut runs = fake.runs.lock().unwrap();
        runs.push((agent.clone(), body));
        let id = format!("run-{}", runs.len());
        fake.outputs.lock().unwrap().insert(id.clone(), reply);
        if let Some(events) = fake.streams.lock().unwrap().get_mut(&agent).and_then(|q| q.pop_front()) {
            fake.live.lock().unwrap().insert(id.clone(), events);
        }
        return out(json!({"run_id": id}));
    }
    if method == "POST" && path.ends_with("/stop") {
        let id = path.trim_end_matches("/stop").rsplit('/').next().unwrap_or("").to_string();
        fake.stopped.lock().unwrap().push(id);
        return out(json!({"status": "stopping"}));
    }
    let run_of = |path: &str| path.trim_end_matches("/events").rsplit('/').next().unwrap_or("").to_string();
    let live = |id: &str| fake.live.lock().unwrap().get(id).cloned();
    let stopped = |id: &str| fake.stopped.lock().unwrap().iter().any(|s| s == id);
    if method == "GET" && path.ends_with("/events") {
        let id = run_of(&path);
        if let Some(events) = live(&id) {
            let fake = fake.clone();
            let stream = async_stream::stream! {
                tokio::time::sleep(Duration::from_millis(200)).await;
                for event in events {
                    yield Ok::<_, std::io::Error>(axum::body::Bytes::from(format!("data: {event}\n\n")));
                }
                for _ in 0..300 {
                    if fake.stopped.lock().unwrap().contains(&id) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                yield Ok(axum::body::Bytes::from(format!("data: {}\n\n", json!({"event": "run.cancelled", "run_id": id}))));
            };
            return Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from_stream(stream))
                .unwrap();
        }
    }
    if method == "GET" && path.contains("/v1/runs/") && live(&run_of(&path)).is_some() {
        let id = run_of(&path);
        return out(if stopped(&id) { json!({"status": "cancelled", "output": ""}) } else { json!({"status": "running"}) });
    }
    if method == "GET" && path.contains("/v1/runs/") {
        let id = path.rsplit('/').next().unwrap_or("").to_string();
        let output = fake.outputs.lock().unwrap().get(&id).cloned().unwrap_or_default();
        return out(
            json!({"status": "completed", "output": output, "usage": {"input_tokens": 1000, "output_tokens": 50}}),
        );
    }
    out(json!({"ok": true}))
}

async fn app_with(fake: &Fake, rooms: Value) -> (TestApp, tempfile::TempDir) {
    let url = common::spawn_fake(Router::new().fallback(backend).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["outbox"] = json!({"db_path": dir.path().join("data/bridge.db").to_string_lossy()});
    d["notebook"] = json!({"base_url": url, "model": "small", "key": "k"});
    d["rooms"] = rooms;
    let mut table = serde_json::Map::new();
    for (a, name) in
        [("pm", "Chef de projet"), ("design", "Design"), ("front", "Front"), ("back", "Back"), ("qa", "Tests")]
    {
        table.insert(
            a.to_string(),
            json!({"display_name": name, "hermes_url": format!("{url}/p/{a}"),
                   "hermes_key": format!("{a}-key-{}", "x".repeat(24))}),
        );
    }
    d["agents"] = Value::Object(table);
    let app = TestApp {
        app: bipbridge::app::create_app(
            parse_config(&d, None).unwrap(),
            bipbridge::app::AppOptions { start_background: false },
        )
        .unwrap(),
        peer: "198.51.100.7:50000".parse().unwrap(),
    };
    (app, dir)
}

fn script(fake: &Fake, agent: &str, replies: &[&str]) {
    fake.script.lock().unwrap().entry(agent.into()).or_default().extend(replies.iter().map(|r| r.to_string()));
}

/// The room's timeline (docs/conversations.md §6.6: a room is the thread `room:<id>`).
async fn timeline(app: &TestApp, id: &str) -> Value {
    let (k, v) = auth();
    app.get(&format!("/v1/threads/room:{id}/timeline?limit=500"), &[(k, v.as_str())]).await.json()
}

/// The room's timeline once nobody answers (and `n` items at least).
async fn settled(app: &TestApp, id: &str, n: usize) -> Value {
    let mut page = Value::Null;
    for _ in 0..400 {
        page = timeline(app, id).await;
        if page["pending"]["room"]["answering"] == json!([]) && page["items"].as_array().unwrap().len() >= n {
            tokio::time::sleep(Duration::from_millis(300)).await; // a conductor call may still queue someone
            page = timeline(app, id).await;
            if page["pending"]["room"]["answering"] == json!([]) {
                return page;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the room never settled: {page}");
}

fn speakers(page: &Value) -> Vec<String> {
    page["items"].as_array().unwrap().iter().map(|m| m["from"].as_str().unwrap().to_string()).collect()
}

/// The person's message, with its app id (`POST /v1/threads/room:<id>/messages`).
fn person(text: &str) -> Value {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    json!({"text": text, "client_id": format!("relay-{:06}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed))})
}

#[tokio::test]
async fn a_discussion_gets_one_answer_and_a_colleague_only_when_named() {
    let fake = Fake::default();
    let (app, _dir) = app_with(&fake, json!({})).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let made = app.send_json("POST", "/v1/rooms", &h, &json!({"participants": ["pm", "design", "front"]})).await.json();
    let id = made["id"].as_str().unwrap().to_string();
    assert_eq!(made["goal"], "");
    assert!(made["goal_proposed"].is_null());
    let path = format!("/v1/threads/room:{id}/messages");

    // no @, nothing the rule tells apart: the small model picks ONE agent; it answers, the hand comes back
    script(&fake, "pm", &["Bonne question : je dirais mardi.\nNEXT: person"]);
    let sent = app.send_json("POST", &path, &h, &person("Quel jour pour la démo ?")).await.json();
    assert_eq!(sent["route"], json!({"by": "model", "agents": ["pm"]}), "{sent}");
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "pm"]);
    assert_eq!(page["items"][1]["text"], "Bonne question : je dirais mardi.");
    assert_eq!(page["items"][1]["room"]["handoff"], json!({"to": "person", "why": "", "by": "agent"}));
    assert!(page["pending"]["room"]["goal_proposed"].is_null(), "a question is no task: {page}");
    // the run was asked for the mark, the discussion's way
    let rules = fake.runs.lock().unwrap()[0].1["instructions"].as_str().unwrap().to_string();
    assert!(rules.contains("end with the line `NEXT: person`") && !rules.contains("DONE:"), "{rules}");

    // a colleague named because really needed: one hop, then back to the person (its own NEXT is not followed)
    script(&fake, "design", &["Il faut voir si c'est faisable.\nNEXT: @front — dire si le dégradé est faisable"]);
    script(&fake, "front", &["Faisable en CSS.\nNEXT: @pm — valider"]);
    app.send_json("POST", &path, &h, &person("@design un dégradé pour le bouton ?")).await;
    let page = settled(&app, &id, 5).await;
    assert_eq!(speakers(&page), vec!["person", "pm", "person", "design", "front"]);
    assert_eq!(page["items"][3]["room"]["handoff"]["to"], "front");
    assert_eq!(page["items"][3]["room"]["handoff"]["why"], "dire si le dégradé est faisable");
    assert_eq!(page["items"][4]["room"]["handoff"]["to"], "person"); // hop 1 = max_hops: no third agent
    let front_input = fake.runs.lock().unwrap().last().unwrap().1["input"].as_str().unwrap().to_string();
    assert!(
        front_input.contains("Design (@design) hands you the turn: dire si le dégradé est faisable"),
        "{front_input}"
    );
    // no conductor in a discussion
    assert!(!fake.asked.lock().unwrap().iter().any(|s| s.contains("coordinate a team")));
}

#[tokio::test]
async fn a_task_gets_a_goal_proposed_then_the_team_relays_toward_it() {
    let fake = Fake::default();
    let (app, _dir) = app_with(&fake, json!({})).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let made = app
        .send_json("POST", "/v1/rooms", &h, &json!({"participants": ["pm", "design", "front", "back", "qa"]}))
        .await
        .json();
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/v1/threads/room:{id}/messages");

    // a task: one agent answers (still a discussion), and a goal is proposed beside it
    script(
        &fake,
        "pm",
        &["Je propose : 1 écran liste, ajout, cocher.\nDONE: périmètre\nNEXT: person — valider le périmètre"],
    );
    app.send_json("POST", &path, &h, &person("Faites-moi une petite app web de liste de courses")).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "pm"]);
    assert_eq!(page["items"][1]["text"], "Je propose : 1 écran liste, ajout, cocher.");
    let mut proposed = page["pending"]["room"]["goal_proposed"].clone();
    for _ in 0..50 {
        if !proposed.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        proposed = timeline(&app, &id).await["pending"]["room"]["goal_proposed"].clone();
    }
    assert_eq!(proposed["text"], "App web de liste de courses");
    assert_eq!(page["pending"]["room"]["goal"], "");

    // accepted (« start »): the conductor picks who goes on; then the agents hand the turn to each other
    fake.conductor
        .lock()
        .unwrap()
        .extend(["design — maquette de l'écran".to_string(), "person — choisir le nom".to_string()]);
    script(&fake, "design", &["Maquette : une liste, un champ.\nDONE: maquette\nNEXT: @front — coder l'écran"]);
    script(&fake, "front", &["Écran codé.\nDONE: écran liste\nNEXT: @back — stocker la liste"]);
    script(&fake, "back", &["PASS\nNEXT: @qa — rien à stocker côté serveur, tester"]);
    script(&fake, "qa", &["Tests ok sur l'ajout.\nDONE: tests ajout"]); // no NEXT: the conductor decides
    let room = app
        .send_json(
            "PATCH",
            &format!("/v1/rooms/{id}"),
            &h,
            &json!({"goal": "App web de liste de courses", "start": true}),
        )
        .await
        .json();
    assert_eq!(room["goal"], "App web de liste de courses");
    assert!(room["goal_proposed"].is_null());
    let page = settled(&app, &id, 6).await;
    assert_eq!(speakers(&page), vec!["person", "pm", "design", "front", "back", "qa"], "{page}");
    let m = page["items"].as_array().unwrap();
    assert_eq!(m[1]["room"]["handoff"], json!({"to": "design", "why": "maquette de l'écran", "by": "conductor"}));
    assert_eq!(m[2]["room"]["handoff"], json!({"to": "front", "why": "coder l'écran", "by": "agent"}));
    assert_eq!(m[4]["kind"], "notice"); // the pass: silent, one discreet line
    assert_eq!(m[4]["notice"]["text"], "Back has nothing to add."); // in the agent's language (English here)
    assert_eq!(m[4]["room"]["handoff"]["to"], "qa");
    assert_eq!(m[5]["room"]["handoff"], json!({"to": "person", "why": "choisir le nom", "by": "conductor"}));
    assert_eq!(page["pending"]["room"]["progress"]["blocked"], "choisir le nom");
    assert_eq!(page["pending"]["room"]["progress"]["done"], json!(["Design : maquette", "Front : écran liste", "Tests : tests ajout"]));
    assert_eq!(page["pending"]["room"]["handoffs"].as_array().unwrap().last().unwrap()["message"], m[5]["id"]);
    // the team's rules and the goal reached each turn
    let runs = fake.runs.lock().unwrap().clone();
    let (agent, body) = &runs[1];
    assert_eq!(agent, "design");
    assert!(body["instructions"].as_str().unwrap().contains("DONE: <what you just did"));
    let input = body["input"].as_str().unwrap();
    assert!(input.contains("Goal your person accepted: App web de liste de courses"), "{input}");
    assert!(input.contains("The discussion's lead hands you the turn: maquette de l'écran"), "{input}");
    // the person's answer goes back to the agent that asked (team work)
    script(&fake, "qa", &["Noté.\nNEXT: none"]);
    let sent = app.send_json("POST", &path, &h, &person("Appelle-la Panier")).await.json();
    assert_eq!(sent["route"]["agents"], json!(["qa"]), "{sent}");
    settled(&app, &id, 8).await;
    // the small calls counted on the room
    let room = app.get(&format!("/v1/threads/room:{id}"), &h).await.json()["room"].clone();
    assert!(room["small_tokens"]["input"].as_i64().unwrap() >= 300, "{room}");
}

#[tokio::test]
async fn team_work_stops_at_its_budget_and_breaks_a_back_and_forth() {
    let fake = Fake::default();
    let (app, dir) = app_with(&fake, json!({"team_turns": 4})).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let made = app
        .send_json("POST", "/v1/rooms", &h, &json!({"participants": ["pm", "design", "front"], "goal": "Un site"}))
        .await
        .json();
    assert_eq!(made["goal"], "Un site");
    let id = made["id"].as_str().unwrap().to_string();
    // pm and design would hand the turn to each other forever
    script(&fake, "pm", &["Un.\nNEXT: @design — encore", "Trois.\nNEXT: @design — encore"]);
    script(&fake, "design", &["Deux.\nNEXT: @pm — encore"]);
    script(&fake, "front", &["Quatre.\nNEXT: @pm — encore"]);
    fake.conductor.lock().unwrap().push_back("front — casser la boucle".into());
    app.send_json("POST", &format!("/v1/threads/room:{id}/messages"), &h, &person("@pm on y va")).await;
    let page = settled(&app, &id, 5).await;
    // pm design pm, then a 4th turn between them refused: the conductor picks front; then 4 turns: the budget
    assert_eq!(speakers(&page), vec!["person", "pm", "design", "pm", "front"], "{page}");
    let m = page["items"].as_array().unwrap();
    assert_eq!(m[3]["room"]["handoff"], json!({"to": "front", "why": "casser la boucle", "by": "conductor"}));
    assert_eq!(m[4]["room"]["handoff"], json!({"to": "person", "why": "", "by": "budget"}));
    let refused = fake.asked.lock().unwrap().len();
    assert_eq!(refused, 1, "one conductor call");
    // each agent's calls and their tokens kept, for its next turns' cap (no tool step: one call a turn)
    let kept: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(format!("data/rooms/{id}.json"))).unwrap()).unwrap();
    assert_eq!(kept["calls"]["pm"], json!({"calls": 2, "tokens": 2100}), "{kept}");
    assert_eq!(kept["calls"]["front"], json!({"calls": 1, "tokens": 1050}));
}

#[tokio::test]
async fn a_task_goes_to_one_agent_and_a_turn_without_words_is_wrapped_up() {
    let fake = Fake::default();
    let (app, _dir) = app_with(&fake, json!({})).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let made = app
        .send_json("POST", "/v1/rooms", &h, &json!({"participants": ["pm", "design", "front", "back", "qa"]}))
        .await
        .json();
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/v1/threads/room:{id}/messages");

    // the small model names two agents for a piece of work: one only (2026-10-31: Back and Front at once)
    *fake.router.lock().unwrap() = Some("back, front".into());
    // its run ends without a word: one short turn without tools says what it did, never an empty turn
    script(&fake, "back", &["", "J'ai posé le schéma de la base, il reste l'API.\nNEXT: person"]);
    let sent = app.send_json("POST", &path, &h, &person("Faites-moi une petite app web de liste de courses")).await.json();
    assert_eq!(sent["route"], json!({"by": "model", "agents": ["back"]}), "{sent}");
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "back"], "{page}");
    assert_eq!(page["items"][1]["role"], "agent");
    assert_eq!(page["items"][1]["text"], "J'ai posé le schéma de la base, il reste l'API.");
    let runs = fake.runs.lock().unwrap();
    assert_eq!(runs.len(), 2);
    let wrap = runs[1].1["input"].as_str().unwrap();
    assert!(wrap.contains("ended without an answer") && wrap.contains("Do not call any tool"), "{wrap}");
}

/// A team room where `front` speaks first; its first run streams `events` and goes on until stopped.
async fn team_turn(fake: &Fake, events: Vec<Value>) -> (TestApp, tempfile::TempDir, String) {
    let (app, dir) = app_with(fake, json!({})).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let made = app
        .send_json("POST", "/v1/rooms", &h, &json!({"participants": ["pm", "front"], "goal": "Une app de courses"}))
        .await
        .json();
    let id = made["id"].as_str().unwrap().to_string();
    fake.streams.lock().unwrap().entry("front".into()).or_default().push_back(events);
    script(fake, "front", &["", "J'ai lu le code, il reste le formulaire.\nNEXT: person"]);
    app.send_json("POST", &format!("/v1/threads/room:{id}/messages"), &h, &person("@front on y va")).await;
    (app, dir, id)
}

/// A tool step's events (each file its own: the brake never sees the same call again).
fn step(file: &str) -> [Value; 2] {
    [json!({"event": "tool.started", "tool": "read_file", "preview": file}),
     json!({"event": "tool.completed", "tool": "read_file", "duration": 0.01, "error": false})]
}

#[tokio::test]
async fn a_team_turn_is_stopped_on_its_real_tokens_before_a_call_past_its_cap() {
    // 2026-11-01, the rooms bench: a DeepSeek turn read ~41k a call after big file reads and reached 249k for a 200k
    // cap, the bridge guessing 20k a call (Hermes gave no tokens before the run's end). Hermes patch 0020 reports
    // each call's (`model.usage`): 20k, 60k, 70k here.
    let usage = |call: u32, input: i64, total_in: i64, total_out: i64| {
        json!({"event": "model.usage", "call": call, "input_tokens": input, "output_tokens": 300,
               "totals": {"input_tokens": total_in, "output_tokens": total_out, "total_tokens": total_in + total_out}})
    };
    let mut events = vec![usage(1, 20_000, 20_000, 300)];
    events.extend(step("app.js"));
    events.push(usage(2, 60_000, 80_000, 600));
    events.extend(step("api.js"));
    events.push(usage(3, 70_000, 150_000, 900));
    events.extend(step("style.css"));
    let fake = Fake::default();
    let (app, dir, id) = team_turn(&fake, events).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "front"], "{page}");
    let said = &page["items"][1];
    assert_eq!(said["text"], "J'ai lu le code, il reste le formulaire.");
    assert_eq!(said["room"]["capped"], true, "{said}");
    // after its 2nd call (80.6k, the next reading 60k at least) it goes on; after its 3rd (150.9k, the next 70k) a
    // 4th would pass 170k (the 200k cap less 30k for the wrap-up): stopped as it asked for tools, though under 200k
    let stopped = fake.stopped.lock().unwrap().clone();
    assert!(!stopped.is_empty() && stopped.iter().all(|r| r == "run-1"), "{stopped:?}");
    // its real tokens (the stopped run comes back without its usage) and its wrap-up's: within the cap
    assert_eq!(said["room"]["tokens"], json!({"input": 151_000, "output": 950}), "{said}");
    let runs = fake.runs.lock().unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs[1].1["input"].as_str().unwrap().contains("reached its limit of tokens"));
    // its real calls and tokens kept, for its next turns' cap
    let kept: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(format!("data/rooms/{id}.json"))).unwrap()).unwrap();
    assert_eq!(kept["calls"]["front"], json!({"calls": 3, "tokens": 150_900}), "{kept}");
}

#[tokio::test]
async fn a_team_turn_without_real_tokens_is_stopped_on_its_estimate() {
    // a Hermes without patch 0020: no `model.usage`, 20k a call and 3k a step estimated from the tool steps; 6 steps
    // ≈ 203k, past the 170k limit
    let events: Vec<Value> = ["a", "b", "c", "d", "e", "f"].iter().flat_map(|f| step(&format!("{f}.js"))).collect();
    let fake = Fake::default();
    let (app, _dir, id) = team_turn(&fake, events).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "front"], "{page}");
    let said = &page["items"][1];
    assert_eq!(said["room"]["capped"], true, "{said}");
    assert_eq!(said["text"], "J'ai lu le code, il reste le formulaire.");
    // the stopped run's tokens are what it was estimated at, then the wrap-up's
    assert_eq!(said["room"]["tokens"], json!({"input": 204_000, "output": 50}), "{said}");
    assert!(fake.stopped.lock().unwrap().iter().all(|r| r == "run-1"));
}

/// Each model call's real tokens (Hermes patch 0020): the call's `input_tokens`, the run's `totals`.
fn usage(call: u32, input: i64, total_in: i64, total_out: i64) -> Value {
    json!({"event": "model.usage", "call": call, "input_tokens": input, "output_tokens": 300,
           "totals": {"input_tokens": total_in, "output_tokens": total_out, "total_tokens": total_in + total_out}})
}

/// A room without a goal (a discussion) with pm and front; front's next runs stream `streams`.
async fn discussion(fake: &Fake, streams: Vec<Vec<Value>>) -> (TestApp, tempfile::TempDir, String) {
    let (app, dir) = app_with(fake, json!({})).await;
    let (k, v) = auth();
    let made = app.send_json("POST", "/v1/rooms", &[(k, v.as_str())], &json!({"participants": ["pm", "front"]})).await.json();
    fake.streams.lock().unwrap().entry("front".into()).or_default().extend(streams);
    (app, dir, made["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn a_turn_before_the_goal_is_capped_too_and_the_room_goes_on() {
    // 2026-11-01, the rooms bench: the room's first turns, before a goal was accepted, had no cap; Front's took 587k.
    // Alex: « Mettre un plafond aussi au premier tour d'un salon, avant l'objectif ». The same cap as a team turn's
    // (200k, 170k once the wrap-up's 30k are set aside), on each call's real tokens.
    let mut events = vec![usage(1, 20_000, 20_000, 300)];
    events.extend(step("app.js"));
    events.push(usage(2, 60_000, 80_000, 600));
    events.extend(step("api.js"));
    events.push(usage(3, 70_000, 150_000, 900));
    events.extend(step("style.css"));
    let fake = Fake::default();
    let (app, dir, id) = discussion(&fake, vec![events]).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let path = format!("/v1/threads/room:{id}/messages");
    script(&fake, "front", &["", "J'ai commencé l'app, il reste le partage.\nNEXT: person — valider le partage par lien"]);
    app.send_json("POST", &path, &h, &person("@front faites-moi une petite app web de liste de courses")).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(page["pending"]["room"]["goal"], "", "still a discussion: {page}");
    assert_eq!(speakers(&page), vec!["person", "front"], "{page}");
    let said = &page["items"][1];
    assert_eq!(said["role"], "agent");
    assert_eq!(said["text"], "J'ai commencé l'app, il reste le partage.");
    assert_eq!(said["room"]["capped"], true, "{said}");
    // stopped as it asked for tools after its 3rd call (150.9k, a 4th reading 70k at least: past 170k)
    let stopped = fake.stopped.lock().unwrap().clone();
    assert!(!stopped.is_empty() && stopped.iter().all(|r| r == "run-1"), "{stopped:?}");
    assert_eq!(said["room"]["tokens"], json!({"input": 151_000, "output": 950}), "{said}");
    // told to wrap up the discussion's way (the NEXT line, no DONE), then the hand back to the person
    {
        let runs = fake.runs.lock().unwrap();
        assert_eq!(runs.len(), 2);
        let wrap = runs[1].1["input"].as_str().unwrap();
        assert!(wrap.contains("reached its limit of tokens") && wrap.contains("the NEXT line") && !wrap.contains("DONE"));
    }
    assert_eq!(said["room"]["handoff"]["to"], "person", "{said}");
    let kept: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(format!("data/rooms/{id}.json"))).unwrap()).unwrap();
    assert!(kept["current"].is_null() && kept["queue"] == json!([]), "{kept}");

    // the room goes on: the next message is answered as usual
    script(&fake, "pm", &["Le partage par lien suffit.\nNEXT: person"]);
    app.send_json("POST", &path, &h, &person("@pm le partage par lien suffit ?")).await;
    let page = settled(&app, &id, 4).await;
    assert_eq!(speakers(&page), vec!["person", "front", "person", "pm"], "{page}");
    assert_eq!(page["items"][3]["text"], "Le partage par lien suffit.");
}

#[tokio::test]
async fn a_capped_turn_whose_wrap_up_reaches_for_a_tool_is_said_plainly() {
    // a turn before a goal stopped at its cap, then its wrap-up (one short call, told not to call any tool) asks for
    // one anyway: the hub stops it within the room kept for it (30k), and the room says so in plain words
    let mut events = vec![usage(1, 20_000, 20_000, 300)];
    events.extend(step("app.js"));
    events.push(usage(2, 160_000, 180_000, 600));
    events.extend(step("api.js"));
    let mut wrap = vec![usage(1, 25_000, 25_000, 200)];
    wrap.extend(step("notes.md"));
    let fake = Fake::default();
    let (app, _dir, id) = discussion(&fake, vec![events, wrap]).await;
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let path = format!("/v1/threads/room:{id}/messages");
    app.send_json("POST", &path, &h, &person("@front faites-moi une petite app web de liste de courses")).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "front"], "{page}");
    let said = &page["items"][1];
    assert_eq!(said["kind"], "notice");
    assert_eq!(said["notice"]["text"], "Front reached the limit of its turn and hands over.");
    assert_eq!(said["room"]["capped"], true, "{said}");
    // both stopped: the turn past 170k, its wrap-up before a 2nd call past 30k; their real tokens counted
    let stopped = fake.stopped.lock().unwrap().clone();
    assert!(stopped.contains(&"run-1".to_string()) && stopped.contains(&"run-2".to_string()), "{stopped:?}");
    assert_eq!(said["room"]["tokens"], json!({"input": 205_000, "output": 800}), "{said}");

    // nothing hangs: the next message is answered
    script(&fake, "pm", &["On reprend demain.\nNEXT: person"]);
    app.send_json("POST", &path, &h, &person("@pm on fait quoi ?")).await;
    let page = settled(&app, &id, 4).await;
    assert_eq!(speakers(&page), vec!["person", "front", "person", "pm"], "{page}");
}
