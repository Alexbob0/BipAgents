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
            "pm".into()
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
        return out(json!({"run_id": id}));
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

/// The room's messages once nobody answers (and `n` messages at least).
async fn settled(app: &TestApp, id: &str, n: usize) -> Value {
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let path = format!("/v1/rooms/{id}/messages");
    let mut page = Value::Null;
    for _ in 0..400 {
        page = app.get(&path, &h).await.json();
        if page["answering"] == json!([]) && page["messages"].as_array().unwrap().len() >= n {
            tokio::time::sleep(Duration::from_millis(300)).await; // a conductor call may still queue someone
            page = app.get(&path, &h).await.json();
            if page["answering"] == json!([]) {
                return page;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the room never settled: {page}");
}

fn speakers(page: &Value) -> Vec<String> {
    page["messages"].as_array().unwrap().iter().map(|m| m["from"].as_str().unwrap().to_string()).collect()
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
    let path = format!("/v1/rooms/{id}/messages");

    // no @, nothing the rule tells apart: the small model picks ONE agent; it answers, the hand comes back
    script(&fake, "pm", &["Bonne question : je dirais mardi.\nNEXT: person"]);
    let sent = app.send_json("POST", &path, &h, &json!({"text": "Quel jour pour la démo ?"})).await.json();
    assert_eq!(sent["route"], json!({"by": "model", "agents": ["pm"]}), "{sent}");
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "pm"]);
    assert_eq!(page["messages"][1]["text"], "Bonne question : je dirais mardi.");
    assert_eq!(page["messages"][1]["handoff"], json!({"to": "person", "why": "", "by": "agent"}));
    assert!(page["goal_proposed"].is_null(), "a question is no task: {page}");
    // the run was asked for the mark, the discussion's way
    let rules = fake.runs.lock().unwrap()[0].1["instructions"].as_str().unwrap().to_string();
    assert!(rules.contains("end with the line `NEXT: person`") && !rules.contains("DONE:"), "{rules}");

    // a colleague named because really needed: one hop, then back to the person (its own NEXT is not followed)
    script(&fake, "design", &["Il faut voir si c'est faisable.\nNEXT: @front — dire si le dégradé est faisable"]);
    script(&fake, "front", &["Faisable en CSS.\nNEXT: @pm — valider"]);
    app.send_json("POST", &path, &h, &json!({"text": "@design un dégradé pour le bouton ?"})).await;
    let page = settled(&app, &id, 5).await;
    assert_eq!(speakers(&page), vec!["person", "pm", "person", "design", "front"]);
    assert_eq!(page["messages"][3]["handoff"]["to"], "front");
    assert_eq!(page["messages"][3]["handoff"]["why"], "dire si le dégradé est faisable");
    assert_eq!(page["messages"][4]["handoff"]["to"], "person"); // hop 1 = max_hops: no third agent
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
    let path = format!("/v1/rooms/{id}/messages");

    // a task: one agent answers (still a discussion), and a goal is proposed beside it
    script(
        &fake,
        "pm",
        &["Je propose : 1 écran liste, ajout, cocher.\nDONE: périmètre\nNEXT: person — valider le périmètre"],
    );
    app.send_json("POST", &path, &h, &json!({"text": "Faites-moi une petite app web de liste de courses"})).await;
    let page = settled(&app, &id, 2).await;
    assert_eq!(speakers(&page), vec!["person", "pm"]);
    assert_eq!(page["messages"][1]["text"], "Je propose : 1 écran liste, ajout, cocher.");
    let mut proposed = page["goal_proposed"].clone();
    for _ in 0..50 {
        if !proposed.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        proposed = app.get(&path, &h).await.json()["goal_proposed"].clone();
    }
    assert_eq!(proposed["text"], "App web de liste de courses");
    assert_eq!(page["goal"], "");

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
    let m = page["messages"].as_array().unwrap();
    assert_eq!(m[1]["handoff"], json!({"to": "design", "why": "maquette de l'écran", "by": "conductor"}));
    assert_eq!(m[2]["handoff"], json!({"to": "front", "why": "coder l'écran", "by": "agent"}));
    assert_eq!(m[4]["kind"], "notice"); // the pass: silent, one discreet line
    assert_eq!(m[4]["text"], "Back has nothing to add."); // in the agent's language (English here)
    assert_eq!(m[4]["handoff"]["to"], "qa");
    assert_eq!(m[5]["handoff"], json!({"to": "person", "why": "choisir le nom", "by": "conductor"}));
    assert_eq!(page["progress"]["blocked"], "choisir le nom");
    assert_eq!(page["progress"]["done"], json!(["Design : maquette", "Front : écran liste", "Tests : tests ajout"]));
    assert_eq!(page["handoffs"].as_array().unwrap().last().unwrap()["message"], m[5]["id"]);
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
    let sent = app.send_json("POST", &path, &h, &json!({"text": "Appelle-la Panier"})).await.json();
    assert_eq!(sent["route"]["agents"], json!(["qa"]), "{sent}");
    settled(&app, &id, 8).await;
    // the small calls counted on the room
    let room = app.get(&format!("/v1/rooms/{id}"), &h).await.json();
    assert!(room["small_tokens"]["input"].as_i64().unwrap() >= 300, "{room}");
}

#[tokio::test]
async fn team_work_stops_at_its_budget_and_breaks_a_back_and_forth() {
    let fake = Fake::default();
    let (app, _dir) = app_with(&fake, json!({"team_turns": 4})).await;
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
    app.send_json("POST", &format!("/v1/rooms/{id}/messages"), &h, &json!({"text": "@pm on y va"})).await;
    let page = settled(&app, &id, 5).await;
    // pm design pm, then a 4th turn between them refused: the conductor picks front; then 4 turns: the budget
    assert_eq!(speakers(&page), vec!["person", "pm", "design", "pm", "front"], "{page}");
    let m = page["messages"].as_array().unwrap();
    assert_eq!(m[3]["handoff"], json!({"to": "front", "why": "casser la boucle", "by": "conductor"}));
    assert_eq!(m[4]["handoff"], json!({"to": "person", "why": "", "by": "budget"}));
    let refused = fake.asked.lock().unwrap().len();
    assert_eq!(refused, 1, "one conductor call");
}
