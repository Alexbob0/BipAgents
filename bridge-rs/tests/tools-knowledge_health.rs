//! The person's health data (health.rs): the phone's uploads, what is kept, the « Santé » switch and the agent's
//! `health` tool, listed only for an agent that has it.

mod common;

use std::sync::{Arc, Mutex};

use bipbridge::agents_admin::{self, Run};
use bipbridge::config::{parse_config, Config};
use bipbridge::{agent_tools, health, souls, Dict};
use chrono::{Duration, NaiveDate};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "health-agent-token";

/// An agent this install manages (served by the hub under /p/), its profile in a temporary HERMES_HOME.
fn with_agent(dir: &std::path::Path) -> (Config, std::path::PathBuf) {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    let folder = dir.join(".hermes/profiles/coach");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("SOUL.md"), souls::with_language("Tu es Coach.\n", Some("fr"))).unwrap();
    // Its Hermes already holds the tools' entry with its token (kept when the entry changes).
    std::fs::write(
        folder.join("config.yaml"),
        format!("mcp_servers:\n  bipagents:\n    url: http://old\n    headers:\n      Authorization: Bearer {TOKEN}\n"),
    )
    .unwrap();
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("coach".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    (config, folder)
}

fn recording_run(commands: &Arc<Mutex<Vec<Vec<String>>>>) -> Run {
    let commands = commands.clone();
    Arc::new(move |args, _env| {
        commands.lock().unwrap().push(args);
        Box::pin(async { Ok(String::new()) })
    })
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

async fn tool_names(app: &TestApp) -> Vec<String> {
    rpc(app, "tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

fn day(date: NaiveDate, n: i64) -> Value {
    json!({"date": date.format("%Y-%m-%d").to_string(), "steps": 8000 + n * 100, "active_kcal": 450, "exercise_min": 30,
           "resting_hr": 58, "hrv_ms": 42.5, "weight_kg": 71.0 + (n as f64) / 10.0,
           "sleep": {"asleep_min": 420, "in_bed_min": 450, "start": "2026-10-19T23:12:00+02:00",
                     "end": "2026-10-20T07:02:00+02:00"},
           "workouts": [{"type": "running", "start": "2026-10-20T18:00:00+02:00", "duration_min": 32,
                         "distance_m": 5200, "kcal": 350}]})
}

#[tokio::test]
async fn the_phone_sends_its_days_only_once_an_agent_has_health_on() {
    let dir = tempfile::tempdir().unwrap();
    let (config, folder) = with_agent(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/health-data", &h).await.json()["agents"], json!([]));
    assert_eq!(app.get("/v1/health-data", &[]).await.status, 401); // the bridge key, as every device route
    let today = chrono::Local::now().date_naive();
    let body = json!({"days": [day(today, 0)]});
    assert_eq!(app.send_json("PUT", "/v1/health-data/days", &h, &body).await.status, 409); // nobody has it on
    assert!(!tool_names(&app).await.contains(&"health".to_string()));

    let commands = Arc::default();
    agents_admin::set_health(&config, config.agent(Some("coach")).unwrap(), true, &recording_run(&commands))
        .await
        .unwrap();
    // The rule in its personality (the person never sees it), its tools' entry changed so Hermes lists them again.
    let soul = std::fs::read_to_string(folder.join("SOUL.md")).unwrap();
    assert!(soul.contains(souls::HEALTH_RULE) && soul.contains("Tu es Coach."));
    assert!(soul.contains(souls::NOTEBOOK_RULE)); // the Carnet, on by default
    assert!(soul.contains(souls::PAYMENTS_RULE)); // every agent's (handover.rs)
    let health_variant = soul.contains(souls::SHARED_MEMORY_HEALTH_RULE) && !soul.contains(souls::SHARED_MEMORY_RULE);
    assert!(health_variant, "in the circle as the health agent: what it learns stays its own");
    let plain = souls::split_notebook(&souls::split_health(&souls::split_payments(&soul)).0).0;
    let plain = souls::split_shared_memory(&plain).0;
    assert_eq!(souls::split_language(&plain).0.trim(), "Tu es Coach.");
    let urls: Vec<String> = commands.lock().unwrap().iter().map(|a: &Vec<String>| a[6].clone()).collect();
    assert!(urls[0].ends_with(&format!(
        "?v={}",
        agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()))
    )));
    assert_ne!(urls[0], agent_tools::tools_url(&config));
    assert!(tool_names(&app).await.contains(&"health".to_string()));
    // A new switch reads activity, sleep and heart; weight is in « body », chosen here too.
    assert_eq!(health::categories(&config, "coach"), vec!["activity", "sleep", "heart"]);
    let before = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));
    agents_admin::set_health_categories(
        &config,
        config.agent(Some("coach")).unwrap(),
        &["activity".into(), "sleep".into(), "heart".into(), "body".into()],
        &recording_run(&commands),
    )
    .await
    .unwrap();
    let after = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));
    assert_ne!(before, after); // the tool's description changed: its Hermes fetches the list again
    assert!(commands.lock().unwrap().iter().any(|a| a.get(6).is_some_and(|u| u.ends_with(&format!("?v={after}")))));

    let days: Vec<Value> = (0..30).map(|n| day(today - Duration::days(29 - n), n)).collect();
    let sent = app.send_json("PUT", "/v1/health-data/days", &h, &json!({"days": days})).await;
    assert_eq!(sent.json(), json!({"stored": 30, "days": 30}));
    let status = app.get("/v1/health-data", &h).await.json();
    assert_eq!((&status["agents"], &status["days"]), (&json!(["coach"]), &json!(30)));
    assert_eq!(status["shared"], json!(["activity", "sleep", "heart", "body"]));
    assert_eq!(status["categories"], json!({"coach": ["activity", "sleep", "heart", "body"]}));
    assert_eq!(status["last"], json!(today.format("%Y-%m-%d").to_string()));
    use std::os::unix::fs::PermissionsExt;
    let file = dir.path().join("data/health/days").join(format!("{}.json", today.format("%Y-%m-%d")));
    assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);

    // The summary by default (the last 7 days shared), one line per day when asked.
    let summary = rpc(&app, "tools/call", json!({"name": "health", "arguments": {}})).await;
    let text = text_of(&summary);
    assert_eq!(summary["isError"], false);
    assert!(text.contains("(7 days with data; shared with you: activity, sleep, heart, body)"), "{text}");
    assert!(text.contains("Sleep: 7 h 00 asleep on average over 7 nights") && text.contains("23:12–07:02"), "{text}");
    assert!(text.contains("Steps: 10600 a day") && text.contains("Workouts: 7 — running ×7 (224 min, 36.4 km)"));
    assert!(text.contains("Resting heart rate: 58 bpm") && text.contains("+0.6 kg"), "{text}");
    let first = (today - Duration::days(2)).format("%Y-%m-%d").to_string();
    let lines = text_of(
        &rpc(
            &app,
            "tools/call",
            json!({"name": "health", "arguments": {"from": first, "metrics": ["steps", "workouts"], "detail": "days"}}),
        )
        .await,
    );
    assert_eq!(lines.lines().count(), 4, "{lines}");
    assert!(
        lines.contains(&format!(
            "{first}: 10700 steps · 450 active kcal · 30 min exercise · running 32 min 5.2 km 350 kcal"
        )),
        "{lines}"
    );
    assert!(!lines.contains("sleep 7 h") && !lines.contains("kg ·"), "{lines}");
    let wrong = rpc(&app, "tools/call", json!({"name": "health", "arguments": {"from": "hier"}})).await;
    assert_eq!(wrong["isError"], true);

    // Turned off: the tool goes, so do the days.
    agents_admin::set_health(&config, config.agent(Some("coach")).unwrap(), false, &recording_run(&commands))
        .await
        .unwrap();
    assert!(!std::fs::read_to_string(folder.join("SOUL.md")).unwrap().contains(souls::HEALTH_START));
    assert!(!tool_names(&app).await.contains(&"health".to_string()));
    assert_eq!(rpc(&app, "tools/call", json!({"name": "health", "arguments": {}})).await["isError"], true);
    assert_eq!(app.get("/v1/health-data", &h).await.json()["days"], 0);
    assert!(!dir.path().join("data/health/days").exists());
}

#[tokio::test]
async fn at_most_400_days_are_kept_and_odd_days_are_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = with_agent(dir.path());
    health::set_enabled(&config, "coach", true).unwrap();
    health::set_categories(&config, "coach", &["activity", "body"]).unwrap();
    let today = NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
    let days: Vec<health::Day> = (0..450)
        .map(|n| serde_json::from_value(day(today - Duration::days(n), n)).unwrap())
        .chain([serde_json::from_value(json!({"date": "2026-11-30", "steps": 1})).unwrap()]) // the future
        .collect();
    assert_eq!(health::store(&config, days, today).unwrap(), 400); // more than 400 days back, or ahead: skipped
    let more = vec![serde_json::from_value(json!({"date": "2026-10-22", "steps": 12})).unwrap()];
    assert_eq!(health::store(&config, more, today).unwrap(), 1);
    let mut kept: Vec<String> = std::fs::read_dir(dir.path().join("data/health/days"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    kept.sort();
    assert_eq!(kept.len(), health::KEPT_DAYS);
    assert_eq!(kept.last().unwrap(), "2026-10-22.json");
    assert_eq!(kept[0], "2025-09-18.json");

    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let bad = app.send_json("PUT", "/v1/health-data/days", &h, &json!({"days": [{"date": "21/10/2026"}]})).await;
    assert_eq!(bad.status, 422);
    let many: Vec<Value> = (0..401).map(|_| json!({"date": "2026-10-20"})).collect();
    assert_eq!(app.send_json("PUT", "/v1/health-data/days", &h, &json!({"days": many})).await.status, 422);
    // An agent deleted: its switch goes, and the days with the last one.
    health::forget(&config, "coach");
    assert!(health::agents(&config).is_empty() && !dir.path().join("data/health/days").exists());
    let empty: Dict = Dict::new();
    assert!(text_of(&Value::Object(health::call(&config, "coach", &empty))).contains("no health category"));
    health::set_enabled(&config, "coach", true).unwrap();
    assert!(text_of(&Value::Object(health::call(&config, "coach", &empty))).starts_with("No health data yet"));
}

/// Two agents with their own categories: each reads only its own, the place keeps only what some agent reads, and a
/// category no agent reads any more leaves the days kept.
#[tokio::test]
async fn each_agent_reads_only_its_categories() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = with_agent(dir.path());
    health::set_enabled(&config, "coach", true).unwrap();
    health::set_enabled(&config, "vie", true).unwrap();
    assert_eq!(health::set_categories(&config, "coach", &["activity"]).unwrap(), Some(vec!["activity"]));
    health::set_categories(&config, "vie", &["sleep", "glucose", "cycle"]).unwrap();
    assert_eq!(health::set_categories(&config, "nobody", &["sleep"]).unwrap(), None);
    let today = chrono::Local::now().date_naive();
    let date = today.format("%Y-%m-%d").to_string();
    let sent: health::Day = serde_json::from_value(json!({"date": date, "steps": 9000, "flights": 8,
        "sleep": {"asleep_min": 400, "deep_min": 60}, "glucose_mmol": 5.8, "menstrual_flow": "light",
        "weight_kg": 70, "resting_hr": 60, "kcal_eaten": 2000}))
    .unwrap();
    health::store(&config, vec![sent], today).unwrap();
    let file = dir.path().join(format!("data/health/days/{date}.json"));
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    // Weight, heart and food: no agent reads them, the place doesn't keep them.
    assert_eq!(
        kept,
        json!({"date": date, "steps": 9000.0, "flights": 8.0, "sleep": {"asleep_min": 400.0, "deep_min": 60.0},
               "glucose_mmol": 5.8, "menstrual_flow": "light"})
    );
    let days = json!({"detail": "days", "days": 1}).as_object().unwrap().clone();
    let coach = text_of(&Value::Object(health::call(&config, "coach", &days)));
    assert!(coach.contains("9000 steps · 8 flights") && coach.contains("shared with you: activity"), "{coach}");
    assert!(!coach.contains("sleep") && !coach.contains("glucose") && !coach.contains("flow"), "{coach}");
    let vie = text_of(&Value::Object(health::call(&config, "vie", &days)));
    assert!(vie.contains("sleep 6 h 40 [deep 1 h 00]") && vie.contains("glucose 5.8 mmol/L"), "{vie}");
    assert!(vie.contains("flow light") && !vie.contains("steps"), "{vie}");
    // Asking for another's category: refused, never shown.
    let other = json!({"categories": ["activity"]}).as_object().unwrap().clone();
    let refused = Value::Object(health::call(&config, "vie", &other));
    assert_eq!(refused["isError"], true);
    assert!(text_of(&refused).contains("some of sleep, glucose, cycle"));
    let old = json!({"metrics": ["sleep", "steps"]}).as_object().unwrap().clone();
    let partly = text_of(&Value::Object(health::call(&config, "vie", &old)));
    assert!(partly.contains("Sleep: 6 h 40") && !partly.contains("Steps"), "{partly}");

    // The cycle no longer read by anyone: gone from the days kept; steps stay (coach reads them).
    health::set_categories(&config, "vie", &["sleep", "glucose"]).unwrap();
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(
        kept.get("menstrual_flow").is_none() && kept["steps"] == json!(9000.0) && kept["glucose_mmol"] == json!(5.8)
    );
    health::set_enabled(&config, "vie", false).unwrap();
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(kept, json!({"date": date, "steps": 9000.0, "flights": 8.0}));
    // Back on: the defaults again, not what it had; switched on again while on: its choice stays, even none.
    health::set_enabled(&config, "vie", true).unwrap();
    assert_eq!(health::categories(&config, "vie"), vec!["activity", "sleep", "heart"]);
    health::set_categories::<&str>(&config, "vie", &[]).unwrap();
    health::set_enabled(&config, "vie", true).unwrap();
    assert!(health::categories(&config, "vie").is_empty() && health::enabled(&config, "vie"));
}
