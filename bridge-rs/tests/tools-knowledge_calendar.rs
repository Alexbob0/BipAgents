//! The person's phone calendar (calendar.rs): the copy the phone sends, the agents allowed (and their tools, listed
//! only for them), the agents' requests waiting for the phone.

mod common;

use bipbridge::config::{parse_config, Config};
use bipbridge::{agent_tools, calendar};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "calendar-agent-token";

fn with_agent(dir: &std::path::Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("coach".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
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

async fn tool(app: &TestApp, name: &str, arguments: Value) -> (bool, String) {
    let result = rpc(app, "tools/call", json!({"name": name, "arguments": arguments})).await;
    (result["isError"] == true, result["content"][0]["text"].as_str().unwrap_or("").to_string())
}

async fn tool_names(app: &TestApp) -> Vec<String> {
    rpc(app, "tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_phone_shares_its_calendar_and_does_what_the_agents_ask() {
    let dir = tempfile::tempdir().unwrap();
    let config = with_agent(dir.path());
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    assert_eq!(app.get("/v1/calendar", &[]).await.status, 401);
    let names = tool_names(&app).await;
    assert!(!names.iter().any(|n| n.starts_with("calendar_"))); // not shared with it: not listed
    assert!(tool(&app, "calendar_list", json!({})).await.0);
    let tools_before = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));

    assert_eq!(app.send_json("PUT", "/v1/calendar/agents", &h, &json!({"agents": ["ghost"]})).await.status, 400);
    let allowed = app.send_json("PUT", "/v1/calendar/agents", &h, &json!({"agents": ["Coach", "coach"]})).await;
    assert_eq!(allowed.json(), json!({"agents": ["coach"]}));
    assert_eq!(tool_names(&app).await.iter().filter(|n| n.starts_with("calendar_")).count(), 4);
    let tools_after = agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("coach")).unwrap()));
    assert_ne!(tools_before, tools_after); // a new entry: its Hermes fetches the list again

    let copy = json!({"from": "2026-10-14", "to": "2026-12-20",
        "calendars": [{"id": "C1", "title": "Perso", "color": "#3CC37A"}, {"id": "C2", "title": "Travail"}],
        "events": [
            {"id": "e1", "calendar": "C1", "title": "Dentiste", "start": "2026-10-22T10:00:00+02:00",
             "end": "2026-10-22T10:30:00+02:00", "all_day": false, "location": "12 rue Haute"},
            {"id": "e2", "calendar": "C2", "title": "Salon", "start": "2026-10-23", "end": "2026-10-25", "all_day": true,
             "notes": "Stand B4"},
            {"id": "e3", "calendar": "C2", "title": "Point équipe", "start": "2026-10-21T09:00:00+02:00",
             "end": "2026-10-21T09:30:00+02:00", "recurring": true},
            {"id": "e4", "calendar": "C1", "title": "Plus tard", "start": "2026-11-30T09:00:00+01:00",
             "end": "2026-11-30T10:00:00+01:00"}]});
    assert_eq!(app.send_json("PUT", "/v1/calendar", &h, &copy).await.json(), json!({"events": 4}));
    let too_long = json!({"events": [{"id": "x".repeat(301), "start": "2026-10-22"}]});
    assert_eq!(app.send_json("PUT", "/v1/calendar", &h, &too_long).await.status, 422);

    let (failed, listed) = tool(&app, "calendar_list", json!({"from": "2026-10-21", "to": "2026-10-24"})).await;
    assert!(!failed, "{listed}");
    assert!(listed.starts_with("From 2026-10-21 to 2026-10-24, as the phone shared it 0 min ago, for 2026-10-14 to 2026-12-20:"));
    let lines: Vec<&str> = listed.lines().skip(1).collect();
    assert_eq!(lines[0], "- id e3 · Travail · 2026-10-21 09:00–09:30 · Point équipe · repeats");
    assert_eq!(lines[1], "- id e1 · Perso · 2026-10-22 10:00–10:30 · Dentiste · at 12 rue Haute");
    assert_eq!(lines[2], "- id e2 · Travail · 2026-10-23 → 2026-10-25 all day · Salon");
    assert_eq!(lines[3], "  Stand B4");
    assert!(!listed.contains("Plus tard"));
    let (_, work) = tool(&app, "calendar_list", json!({"from": "2026-10-24", "to": "2026-10-24", "calendar": "travail"})).await;
    assert!(work.contains("Salon") && !work.contains("Dentiste"), "{work}"); // a 3-day event seen on its last day
    let (_, none) = tool(&app, "calendar_list", json!({"from": "2027-01-01", "to": "2027-01-02"})).await;
    assert!(none.starts_with("No event from 2027-01-01 to 2027-01-02"), "{none}");
    assert!(tool(&app, "calendar_list", json!({"from": "demain"})).await.0);

    let (failed, asked) =
        tool(&app, "calendar_add", json!({"title": "Banque", "start": "2026-10-30T10:00", "duration_minutes": 45,
                                         "calendar": "Perso", "location": "Agence"}))
        .await;
    assert!(!failed && asked.contains("next sync"), "{asked}");
    assert!(tool(&app, "calendar_add", json!({"title": " ", "start": "2026-10-30"})).await.0);
    assert!(tool(&app, "calendar_add", json!({"title": "x", "start": "bientôt"})).await.0);
    assert!(tool(&app, "calendar_add", json!({"title": "x", "start": "2026-10-30T10:00", "end": "2026-10-30T09:00"})).await.0);
    assert!(!tool(&app, "calendar_add", json!({"title": "Congés", "start": "2026-12-21", "end": "2026-12-24"})).await.0);
    assert!(tool(&app, "calendar_update", json!({"id": "zz", "title": "x"})).await.0);
    assert!(tool(&app, "calendar_update", json!({"id": "e3", "title": "x"})).await.1.contains("repeating"));
    assert!(tool(&app, "calendar_update", json!({"id": "e1"})).await.0); // nothing to change
    assert!(!tool(&app, "calendar_update", json!({"id": "e1", "start": "2026-10-22T11:00:00+02:00",
                                                  "end": "2026-10-22T11:30:00+02:00"})).await.0);
    assert!(!tool(&app, "calendar_delete", json!({"id": "e2"})).await.0);

    let queue = app.get("/v1/calendar/queue", &h).await.json()["queue"].clone();
    let ops: Vec<(String, Value)> =
        queue.as_array().unwrap().iter().map(|op| (op["op"].as_str().unwrap().to_string(), op["args"].clone())).collect();
    assert_eq!(ops.len(), 4);
    assert_eq!(ops[0], ("add".into(), json!({"title": "Banque", "start": "2026-10-30T10:00:00", "end": "2026-10-30T10:45:00",
        "all_day": false, "calendar": "Perso", "location": "Agence", "notes": ""})));
    assert_eq!((ops[1].1["start"].clone(), ops[1].1["end"].clone(), ops[1].1["all_day"].clone()),
               (json!("2026-12-21"), json!("2026-12-24"), json!(true)));
    assert_eq!(ops[2], ("update".into(), json!({"id": "e1", "start": "2026-10-22T11:00:00+02:00", "end": "2026-10-22T11:30:00+02:00"})));
    assert_eq!(ops[3], ("delete".into(), json!({"id": "e2"})));
    assert_eq!(queue[0]["agent"], "coach");

    let first = queue[0]["id"].as_str().unwrap();
    let answered = app.send_json("POST", &format!("/v1/calendar/queue/{first}"), &h, &json!({"done": true})).await;
    assert_eq!(answered.json(), json!({"ok": true}));
    let failed = queue[3]["id"].as_str().unwrap();
    app.send_json("POST", &format!("/v1/calendar/queue/{failed}"), &h, &json!({"done": false, "error": "no such event"})).await;
    assert_eq!(app.send_json("POST", "/v1/calendar/queue/nope", &h, &json!({"done": true})).await.status, 404);
    let seen = app.get("/v1/calendar", &h).await.json();
    assert_eq!((&seen["agents"], &seen["calendars"], &seen["events"], &seen["pending"]),
               (&json!(["coach"]), &json!(2), &json!(4), &json!(2)));
    assert_eq!((&seen["from"], &seen["to"]), (&json!("2026-10-14"), &json!("2026-12-20")));

    use std::os::unix::fs::PermissionsExt;
    let stored = dir.path().join("data/calendar.json");
    assert_eq!(std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777, 0o600);

    // No longer shared with it: its tools go and it is told so.
    assert_eq!(app.send_json("PUT", "/v1/calendar/agents", &h, &json!({"agents": []})).await.json(), json!({"agents": []}));
    assert!(!calendar::enabled(&config, "coach"));
    assert!(!tool_names(&app).await.iter().any(|n| n.starts_with("calendar_")));
    assert!(tool(&app, "calendar_list", json!({})).await.1.contains("hasn't shared"));
}

#[tokio::test]
async fn health_advertises_the_calendar() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(with_agent(dir.path()));
    let features = app.get("/health", &[]).await.json()["features"].clone();
    assert!(features.as_array().unwrap().contains(&json!("calendar")));
}
