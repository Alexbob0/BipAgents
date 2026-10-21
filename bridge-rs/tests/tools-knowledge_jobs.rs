//! Port of bridge/tests/test_jobs.py: background tasks run in their own conversation of a fake Hermes, stopped at
//! their budget, paused, resumed, forgotten, proposed by an agent, followed again after a restart; mail scans listed
//! with them; the routes.

mod common;

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use bipbridge::config::{parse_config, AgentConfig, Config, SharedConfig};
use bipbridge::hermes::HermesClient;
use bipbridge::jobs::{self, Jobs, TellJob};
use bipbridge::mail_scan::{self, Scans};
use bipbridge::Dict;
use common::{config_dict, spawn_fake, Urls};
use serde_json::{json, Map, Value};

/// A gateway with one conversation at a time: each poll of its counters adds `step` input tokens while a run goes
/// on; a run ends after `rounds` polls of its status, or when stopped.
#[derive(Default)]
struct Gateway {
    rounds: u32,
    step: i64,
    output: String,
    requests: Vec<(String, String, Option<Value>)>,
    runs: Map<String, Value>,
    counters: [i64; 3],
    messages: Vec<Value>,
    forget_runs: bool,
}

type Fake = Arc<Mutex<Gateway>>;

async fn handle(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().to_string();
    let full = request.uri().path().to_string();
    let bytes: Bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Option<Value> = if bytes.is_empty() { None } else { serde_json::from_slice(&bytes).ok() };
    let mut g = fake.lock().unwrap();
    g.requests.push((method.clone(), full.clone(), body.clone()));
    let path = full.strip_prefix("/p/coach").unwrap_or(&full).to_string();
    let parts: Vec<&str> = path.split('/').collect();
    if method == "POST" && path == "/api/sessions" {
        return Json(json!({"session": {"id": "s1", "title": body.unwrap()["title"]}})).into_response();
    }
    if method == "GET" && path == "/api/sessions/s1" {
        if g.runs.values().any(|r| r["status"] == "running") {
            g.counters[0] += g.step;
            g.counters[1] += g.step / 20;
        }
        let c = g.counters;
        return Json(
            json!({"session": {"id": "s1", "input_tokens": c[0], "output_tokens": c[1], "cache_read_tokens": c[2]}}),
        )
        .into_response();
    }
    if method == "GET" && path == "/api/sessions/s1/messages" {
        return Json(json!({"messages": g.messages})).into_response();
    }
    if method == "POST" && path == "/v1/runs" {
        let run_id = format!("r{}", g.runs.len() + 1);
        g.runs.insert(run_id.clone(), json!({"status": "running", "polls": 0, "input": body.unwrap()["input"]}));
        return (StatusCode::ACCEPTED, Json(json!({"run_id": run_id, "status": "started"}))).into_response();
    }
    if method == "POST" && path.ends_with("/stop") {
        g.runs[parts[3]]["status"] = json!("cancelled");
        return Json(json!({"status": "stopping"})).into_response();
    }
    if method == "GET" && path.starts_with("/v1/runs/") {
        let (rounds, forget, output) = (g.rounds, g.forget_runs, g.output.clone());
        let Some(run) = g.runs.get_mut(parts[3]).filter(|_| !forget) else {
            return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response();
        };
        let polls = run["polls"].as_u64().unwrap() + 1;
        run["polls"] = json!(polls);
        if run["status"] == "running" && polls >= rounds as u64 {
            run["status"] = json!("completed");
            return Json(json!({"status": "completed", "output": output, "usage": {"input_tokens": 100_000, "output_tokens": 5_000}}))
                .into_response();
        }
        return Json(json!({"status": run["status"]})).into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn gateway(rounds: u32, step: i64) -> (Fake, String) {
    let fake: Fake = Arc::new(Mutex::new(Gateway {
        rounds,
        step,
        output: "Rapport : trois écoles comparées, page « Écoles ».".into(),
        ..Default::default()
    }));
    let url = spawn_fake(Router::new().fallback(any(handle)).with_state(fake.clone())).await;
    (fake, url)
}

type Told = Arc<Mutex<Vec<(String, Option<String>, String, String)>>>;

fn setup(dir: &std::path::Path, url: &str) -> (Config, AgentConfig) {
    let mut dict = config_dict(dir, &Urls::default());
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": format!("{url}/p/coach"), "hermes_key": "k"});
    let config = parse_config(&dict, None).unwrap();
    let agent = config.agent(Some("coach")).unwrap().clone();
    (config, agent)
}

fn board(config: &Config, told: Option<&Told>) -> Arc<Jobs> {
    let mut jobs =
        Jobs::new(SharedConfig::new(config.clone()), HermesClient::new(reqwest::Client::new()), None, None, None);
    jobs.poll = 0.01;
    if let Some(told) = told {
        let told = told.clone();
        let tell: TellJob = Arc::new(move |agent, session, line, _job, body| {
            told.lock().unwrap().push((agent, session, line, body));
            Box::pin(async {})
        });
        jobs.tell = Some(tell);
    }
    Arc::new(jobs)
}

fn id_of(job: &Dict) -> String {
    job["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_task_runs_in_its_own_conversation_and_ends_with_its_report() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(3, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let job = board
        .create(
            &agent,
            "Écoles du quartier",
            "Compare les trois écoles près de chez nous.",
            1_000_000,
            Some("research"),
        )
        .await
        .unwrap();
    assert_eq!((job["state"].as_str(), job["session_id"].as_str()), (Some("running"), Some("s1")));
    board.wait(&id_of(&job)).await;
    let done = board.view(&id_of(&job)).unwrap();
    assert_eq!(done["state"], "done");
    assert!(done["report"].as_str().unwrap().starts_with("Rapport"));
    // The run's own usage at the end, not the counters' guess.
    assert_eq!(done["tokens"], json!({"input": 100_000, "output": 5_000, "cached": 0}));
    let posted: Vec<Value> = fake
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|(m, _, _)| m == "POST")
        .map(|(_, _, b)| b.clone().unwrap_or(Value::Null))
        .collect();
    assert_eq!(posted[0]["title"], "🧩 Écoles du quartier");
    assert_eq!(posted[1]["session_id"], "s1");
    assert_eq!(posted[1]["instructions"], jobs::INSTRUCTIONS);
    assert!(posted[1]["input"].as_str().unwrap().starts_with("Écoles du quartier\n\nCompare"));
    assert_eq!(
        told.lock().unwrap().clone(),
        vec![(
            "coach".into(),
            Some("s1".into()),
            "Tâche de fond terminée : Écoles du quartier".into(),
            "Tâche de fond terminée".into()
        )]
    );
    let listed = board.list(None);
    assert_eq!(listed[0]["id"], json!(id_of(&job)));
    assert!(!listed[0].contains_key("base") && !listed[0].contains_key("run_id"));
}

#[tokio::test]
async fn a_task_stops_at_its_budget_and_goes_on_with_a_bigger_one() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(50, 60_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let job = board.create(&agent, "Gros tri", "Range tout.", 100_000, None).await.unwrap();
    let id = id_of(&job);
    board.wait(&id).await;
    let paused = board.view(&id).unwrap();
    assert_eq!((paused["state"].as_str(), paused["reason"].as_str()), (Some("paused"), Some("budget")));
    assert!(fake
        .lock()
        .unwrap()
        .requests
        .iter()
        .any(|(m, p, b)| m == "POST" && p == "/p/coach/v1/runs/r1/stop" && b.is_none()));
    let before = paused["tokens"]["input"].as_i64().unwrap();
    assert!(before >= 100_000 * 20 / 21); // counted from the conversation's counters
    assert_eq!(told.lock().unwrap().last().unwrap().3, "Tâche de fond en pause");
    // Resuming without more budget is refused; with more, the same conversation goes on.
    assert!(board.resume(&id, None, None).await.unwrap_err().downcast_ref::<jobs::JobError>().is_some());
    fake.lock().unwrap().rounds = 2;
    board.resume(&id, Some(5_000_000), Some("Continue, et termine par ton rapport.")).await.unwrap();
    board.wait(&id).await;
    let done = board.view(&id).unwrap();
    assert_eq!((done["state"].as_str(), done["runs"].as_i64()), (Some("done"), Some(2)));
    assert_eq!(done["tokens"]["input"].as_i64().unwrap(), before + 100_000); // the second run's own usage added
    let g = fake.lock().unwrap();
    assert_eq!(g.runs["r2"]["input"], "Continue, et termine par ton rapport.");
    assert_eq!(g.requests.iter().filter(|(m, p, _)| m == "POST" && p.ends_with("/api/sessions")).count(), 1);
}

#[tokio::test]
async fn the_person_pauses_resumes_and_forgets_a_task() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 10).await;
    let (config, agent) = setup(dir.path(), &url);
    let board = board(&config, None);
    let job = board.create(&agent, "Long", "Fais-le.", 1_000_000, None).await.unwrap();
    let id = id_of(&job);
    let again = board.create(&agent, "Autre", "Encore.", 1_000_000, None).await.unwrap_err();
    assert!(again.to_string().contains("already"));
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(board.pause(&id).await.unwrap()["state"], "stopping");
    board.wait(&id).await;
    let view = board.view(&id).unwrap();
    assert_eq!((view["state"].as_str(), view["reason"].as_str()), (Some("paused"), Some("stopped")));
    board.resume(&id, None, None).await.unwrap();
    assert_eq!(fake.lock().unwrap().runs["r2"]["input"], jobs::CONTINUE);
    board.forget(&id).await.unwrap();
    assert!(jobs::load(&config, &id).is_none());
    assert_eq!(fake.lock().unwrap().runs["r2"]["status"], "cancelled");
}

#[tokio::test]
async fn an_agent_proposes_a_task_the_person_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let told: Told = Default::default();
    let board = board(&config, Some(&told));
    let arguments: Dict = serde_json::from_value(
        json!({"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"}),
    )
    .unwrap();
    let result = jobs::propose_tool(Some(&board), &agent, &arguments).await.unwrap();
    assert_eq!(result["isError"], false);
    assert!(result["content"][0]["text"].as_str().unwrap().contains("Tâches de fond"));
    assert!(result["content"][0]["text"].as_str().unwrap().contains("300,000"));
    let proposed = board.list(None)[0].clone();
    assert_eq!((proposed["state"].as_str(), proposed["proposed_by"].as_str()), (Some("proposed"), Some("agent")));
    assert_eq!(proposed["max_tokens"], 300_000);
    assert!(!fake.lock().unwrap().requests.iter().any(|(_, p, _)| p.ends_with("/v1/runs"))); // nothing spent before the go
    let first = told.lock().unwrap()[0].clone();
    assert_eq!((first.3.as_str(), first.1), ("Tâche de fond proposée", None));
    let id = id_of(&proposed);
    board.start(&id, Some(500_000)).await.unwrap();
    board.wait(&id).await;
    assert_eq!(board.view(&id).unwrap()["state"], "done");
}

#[tokio::test]
async fn after_a_restart_a_finished_run_is_read_from_its_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let (fake, url) = gateway(1000, 40_000).await;
    let (config, agent) = setup(dir.path(), &url);
    let first = board(&config, None);
    let job = first.create(&agent, "Recherche", "Cherche.", 50_000_000, None).await.unwrap();
    let id = id_of(&job);
    first.stop_followers(); // the bridge stops
    {
        let mut g = fake.lock().unwrap();
        g.forget_runs = true; // Hermes restarted meanwhile: the run is gone, its turns stay
        g.messages = vec![
            json!({"role": "user", "content": "Recherche\n\nCherche."}),
            json!({"role": "assistant", "content": "Voici le rapport."}),
        ];
    }
    let again = board(&config, None);
    assert_eq!(again.clone().resume_following().await, 1);
    again.wait(&id).await;
    let view = again.view(&id).unwrap();
    assert_eq!((view["state"].as_str(), view["report"].as_str()), (Some("done"), Some("Voici le rapport.")));
}

#[tokio::test]
async fn mail_scans_are_listed_and_controlled_with_the_tasks() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = setup(dir.path(), "http://127.0.0.1:9");
    let scan: Dict = serde_json::from_value(json!({"agent": "coach", "account": "a1", "label": "Gmail", "state": "paused", "error": "budget",
        "created": 1.0, "ranges": [], "tokens": {"input": 9000, "output": 900, "cached": 0},
        "processed": 30, "pages": 0, "step": {"since": "2026-07-21", "until": "2026-10-21", "max_tokens": 10_000, "mails": 80, "processed": 30}}))
    .unwrap();
    mail_scan::save(&config, &scan).unwrap();
    let scans = Arc::new(Scans::new(None, None, None));
    let board = Jobs::new(
        SharedConfig::new(config.clone()),
        HermesClient::new(reqwest::Client::new()),
        None,
        None,
        Some(scans),
    );
    let listed: Vec<Value> = board.list(None).into_iter().map(Value::Object).collect();
    assert_eq!(
        Value::Array(listed),
        json!([{"id": "mail-scan:coach", "kind": "mail_scan", "agent": "coach", "title": "Gmail",
                "state": "paused", "reason": "budget", "error": null, "created": 1.0, "finished": null,
                "tokens": {"input": 9000, "output": 900, "cached": 0}, "max_tokens": 10_000,
                "progress": {"done": 30, "total": 80}, "processed": 30, "pages": 0}])
    );
    // Forgotten from the jobs' list: its file goes.
    board.forget("mail-scan:coach").await.unwrap();
    assert!(mail_scan::load(&config, "coach").is_none());
    let unknown = board.pause("mail-scan:coach").await.unwrap_err();
    assert_eq!(unknown.to_string(), "unknown job");
}

#[tokio::test]
async fn the_routes_need_the_bridge_key_and_list_the_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let app = common::TestApp::new(common::config(dir.path(), &Urls::default()));
    assert_eq!(app.get("/v1/jobs", &[]).await.status, 401);
    let (k, v) = common::auth();
    let answer = app.get("/v1/jobs", &[(k, &v)]).await;
    assert_eq!(answer.status, 200);
    assert_eq!(
        answer.json(),
        json!({"jobs": [], "budgets": {"small": 300_000, "medium": 1_000_000, "large": 3_000_000}})
    );
    let missing = app.request("POST", "/v1/jobs/t000000000000/pause", &[(k, &v)], None).await;
    assert_eq!((missing.status.as_u16(), missing.json()), (404, json!({"detail": "unknown job"})));
}
