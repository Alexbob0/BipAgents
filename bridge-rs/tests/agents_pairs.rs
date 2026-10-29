//! Two agents' own conversation (agent_pairs.rs): the target answers in a hidden conversation of its own for the pair,
//! made once and used again, never in its Bot Chat; the bridge keeps the thread and its exchanges, listed for both
//! agents by the person's app only.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bipbridge::agent_pairs::{self, Answer};
use bipbridge::config::parse_config;
use common::{auth, config_dict, spawn_fake, TestApp, Urls};
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Fake {
    /// (method, path, body) of each call.
    calls: Arc<Mutex<Vec<(String, String, Value)>>>,
}

async fn hermes(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let n = {
        let mut calls = fake.calls.lock().unwrap();
        calls.push((method.clone(), path.clone(), body));
        calls.len()
    };
    match (method.as_str(), path.as_str()) {
        ("POST", p) if p.ends_with("/api/sessions") => {
            (axum::http::StatusCode::CREATED, axum::Json(json!({"session": {"id": format!("s{n}")}}))).into_response()
        }
        ("PATCH", _) => axum::Json(json!({})).into_response(),
        ("POST", p) if p.ends_with("/v1/runs") => {
            (axum::http::StatusCode::ACCEPTED, axum::Json(json!({"run_id": format!("r{n}")}))).into_response()
        }
        ("GET", p) if p.contains("/v1/runs/") => {
            axum::Json(json!({"status": "completed", "output": "  Sept heures.  "})).into_response()
        }
        _ => (axum::http::StatusCode::NOT_FOUND, "no").into_response(),
    }
}

#[tokio::test]
async fn the_target_answers_in_the_pairs_own_conversation() {
    let fake = Fake::default();
    let url = spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["agents"] = json!({
        "Quotidien": {"hermes_url": format!("{url}/p/quotidien"), "hermes_key": "q".repeat(32)},
        "Coach": {"hermes_url": format!("{url}/p/coach"), "hermes_key": "c".repeat(32)},
    });
    let config = parse_config(&d, None).unwrap();
    let asker = config.agent(Some("quotidien")).unwrap().clone();
    let target = config.agent(Some("coach")).unwrap().clone();
    let http = reqwest::Client::new();

    let answer = agent_pairs::ask(&http, None, &config, &asker, &target, "Combien d'heures ?", 30.0).await;
    assert_eq!(answer, Answer::Text("Sept heures.".into()));
    let answer = agent_pairs::ask(&http, None, &config, &asker, &target, "Et hier ?", 30.0).await;
    assert_eq!(answer, Answer::Text("Sept heures.".into()));

    let calls = fake.calls.lock().unwrap().clone();
    let created: Vec<_> = calls.iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/api/sessions")).collect();
    assert_eq!(created.len(), 1, "one conversation for the pair, used again");
    assert_eq!(created[0].1, "/p/coach/api/sessions");
    let id = agent_pairs::pair_id("quotidien", "coach");
    assert_eq!(created[0].2["title"], json!(format!("BipAgents · ⇄ {id}")));
    assert!(calls.iter().any(|(m, _, b)| m == "PATCH" && b["hidden"] == json!(true)));
    let runs: Vec<_> = calls.iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/v1/runs")).collect();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|(_, p, b)| p == "/p/coach/v1/runs" && b["session_id"] == json!("s1")));
    let input = runs[0].2["input"].as_str().unwrap();
    assert!(input.contains("<!--bip:pair\nThis is your own conversation with Quotidien (@quotidien)"), "{input}");
    assert!(input.ends_with("Message from Quotidien (@quotidien): Combien d'heures ?"), "{input}");

    // kept by the bridge: one exchange of four messages, the same pair for both agents
    let pair = agent_pairs::get(&config, &id).unwrap();
    assert_eq!(pair.agents, vec!["coach".to_string(), "quotidien".to_string()]);
    assert_eq!(pair.sessions.get("coach").map(String::as_str), Some("s1"));
    let kinds: Vec<_> = pair.messages.iter().map(|m| (m.from.as_str(), m.kind.as_str())).collect();
    assert_eq!(kinds, [("quotidien", "ask"), ("coach", "answer"), ("quotidien", "ask"), ("coach", "answer")]);
    assert_eq!(pair.exchanges.len(), 1);
    assert_eq!(pair.exchanges[0].count, 4);
    assert_eq!(agent_pairs::of_agent(&config, "quotidien").len(), 1);
    assert_eq!(agent_pairs::of_agent(&config, "coach").len(), 1);

    // the person's app reads them; nobody else
    let app = TestApp::new(config.clone());
    let (name, value) = auth();
    let listed = app.get("/v1/agent-pairs?agent=coach", &[(name, value.as_str())]).await;
    assert_eq!(listed.status, 200);
    let view = &listed.json()["pairs"][0];
    assert_eq!(view["messages"], json!(4));
    assert_eq!(view["agents"][1]["name"], json!("Quotidien"));
    assert_eq!(view["exchanges"][0]["from"], json!("quotidien"));
    let thread = app.get(&format!("/v1/agent-pairs/{id}"), &[(name, value.as_str())]).await;
    assert_eq!(thread.json()["thread"].as_array().unwrap().len(), 4);
    assert_eq!(app.get("/v1/agent-pairs?agent=coach", &[]).await.status, 401);
    assert_eq!(app.get("/v1/agent-pairs?agent=ghost", &[(name, value.as_str())]).await.status, 404);
}
