//! The person's time zone and standard dates (dates/), from the outside: the phone's zone reported, the agents'
//! profiles given it at once, the routines kept and the travel question answered, the settings, the standard dates
//! route and the Carnet's computed entries, the `upcoming_dates` tool.

mod common;

use std::path::Path;

use bipbridge::agent_tools;
use bipbridge::config::{parse_config, Config};
use common::{auth, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "dates-agent-token";

fn setup(dir: &Path) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    let home = dir.join("hermes");
    dict["hermes"] = json!({"home": home.to_string_lossy()});
    let profile = home.join("profiles").join("wellness");
    std::fs::create_dir_all(profile.join("cron")).unwrap();
    std::fs::write(profile.join("config.yaml"), "model:\n  default: some-model\n").unwrap();
    std::fs::write(
        profile.join("SOUL.md"),
        "Tu es Bip.\n\n<!-- bipagents:language -->\nLanguage: answer in the language your person writes in. When you \
write first (a routine, a report, a reminder), write in French.\n<!-- /bipagents:language -->\n",
    )
    .unwrap();
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
}

fn routines(dir: &Path) {
    let jobs = json!({"jobs": [
        {"id": "a1", "name": "Point du matin", "enabled": true, "state": "scheduled",
         "schedule": {"kind": "cron", "expr": "0 7 * * *", "display": "0 7 * * *"}},
        {"id": "a2", "name": "Bilan du soir", "enabled": true, "schedule": {"kind": "cron", "expr": "0 21 * * *"}},
        {"id": "a3", "name": "En pause", "enabled": false, "schedule": {"kind": "cron", "expr": "0 9 * * *"}},
    ]});
    let file = dir.join("hermes").join("profiles").join("wellness").join("cron").join("jobs.json");
    std::fs::write(file, jobs.to_string()).unwrap();
}

fn profile(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("hermes").join("profiles").join("wellness").join("config.yaml")).unwrap()
}

async fn send(app: &TestApp, method: &str, path: &str, body: Value) -> (u16, Value) {
    let (k, v) = auth();
    let answer = app.send_json(method, path, &[(k, &v)], &body).await;
    let status = answer.status.as_u16();
    (status, if answer.body.is_empty() { Value::Null } else { answer.json() })
}

async fn get(app: &TestApp, path: &str) -> Value {
    let (k, v) = auth();
    let answer = app.get(path, &[(k, &v)]).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()
}

#[tokio::test]
async fn the_phone_zone_reaches_the_agents_and_the_routines_wait_for_the_answer() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(setup(dir.path()));
    routines(dir.path());

    // the key is needed
    assert_eq!(app.get("/v1/time", &[]).await.status, 401);

    let (status, view) = send(&app, "PUT", "/v1/time/device?lang=fr", json!({"time_zone": "Europe/Paris"})).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["time_zone"], "Europe/Paris");
    assert_eq!(view["routines_zone"], "Europe/Paris");
    assert_eq!(view["question"], Value::Null);
    assert_eq!(view["country"], "FR");
    assert_eq!(view["country_guessed"], true);
    assert_eq!(view["school_zones"], json!(["A", "B", "C", "Corse"]));
    let text = profile(dir.path());
    assert!(text.contains("timezone: Europe/Paris"), "{text}");
    assert!(text.contains("cron:\n  timezone: Europe/Paris"), "{text}");

    // to Tokyo: the agents' « now » at once, the routines kept until the person answers
    let (_, view) = send(&app, "PUT", "/v1/time/device?lang=fr", json!({"time_zone": "Asia/Tokyo"})).await;
    assert_eq!(view["time_zone"], "Asia/Tokyo");
    assert_eq!(view["routines_zone"], "Europe/Paris");
    let question = &view["question"];
    assert!(question["text"].as_str().unwrap().starts_with("Tu es maintenant à l'heure de Tokyo (+"), "{question}");
    assert!(question["text"].as_str().unwrap().ends_with("Point du matin à 7 h, Bilan du soir à 21 h."), "{question}");
    assert_eq!(question["move"], "Les passer à l'heure de Tokyo");
    assert_eq!(question["keep"], "Les garder à l'heure de Paris");
    assert_eq!(question["routines"].as_array().unwrap().len(), 2);
    let text = profile(dir.path());
    assert!(text.starts_with("model:") && text.contains("timezone: Asia/Tokyo"), "{text}");
    assert!(text.contains("cron:\n  timezone: Europe/Paris"), "{text}");
    // the question in English for an app in English
    let english = get(&app, "/v1/time?lang=en").await;
    assert_eq!(english["question"]["keep"], "Keep them on Paris time");

    // moved
    let (status, view) = send(&app, "POST", "/v1/time/routines", json!({"choice": "move"})).await;
    assert_eq!(status, 200);
    assert_eq!(view["routines_zone"], "Asia/Tokyo");
    assert_eq!(view["question"], Value::Null);
    assert!(profile(dir.path()).contains("cron:\n  timezone: Asia/Tokyo"));
    let (status, _) = send(&app, "POST", "/v1/time/routines", json!({"choice": "move"})).await;
    assert_eq!(status, 404);
    let (status, _) = send(&app, "POST", "/v1/time/routines", json!({"choice": "maybe"})).await;
    assert_eq!(status, 422);

    // home again: asked, « Revenir comme avant »
    let (_, view) = send(&app, "PUT", "/v1/time/device?lang=fr", json!({"time_zone": "Europe/Paris"})).await;
    assert_eq!(view["question"]["move"], "Revenir comme avant (heure de Paris)");
    assert_eq!(view["question"]["back"], true);

    // an unknown zone is refused
    let (status, _) = send(&app, "PUT", "/v1/time/device", json!({"time_zone": "Mars/Olympus"})).await;
    assert_eq!(status, 422);
    app.shutdown().await;
}

#[tokio::test]
async fn the_settings_and_the_standard_dates() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(setup(dir.path()));
    send(&app, "PUT", "/v1/time/device", json!({"time_zone": "Europe/Paris"})).await;

    // a manual zone only once the automatic one is off
    let (_, view) = send(&app, "PATCH", "/v1/time", json!({"time_zone": "America/New_York"})).await;
    assert_eq!(view["time_zone"], "Europe/Paris");
    let (_, view) = send(&app, "PATCH", "/v1/time", json!({"auto": false, "time_zone": "America/New_York"})).await;
    assert_eq!((view["auto"].clone(), view["time_zone"].clone()), (json!(false), json!("America/New_York")));
    assert_eq!(view["country"], "US"); // guessed
                                       // the phone is not followed any more
    let (_, view) = send(&app, "PUT", "/v1/time/device", json!({"time_zone": "Asia/Tokyo"})).await;
    assert_eq!(view["time_zone"], "America/New_York");
    assert_eq!(view["device"], "Asia/Tokyo");
    // back on: the phone's zone again (no routine: nothing to ask)
    let (_, view) = send(&app, "PATCH", "/v1/time", json!({"auto": true})).await;
    assert_eq!(view["time_zone"], "Asia/Tokyo");
    assert_eq!(view["question"], Value::Null);
    assert_eq!(view["country"], Value::Null);

    // France from a postal code: country, school zone, region
    let (status, view) = send(&app, "PATCH", "/v1/time", json!({"country": "FR", "postal_code": "57000"})).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!((view["school_zone"].clone(), view["region"].clone()), (json!("B"), json!("57")));
    assert!(view["regions"].as_array().unwrap().iter().any(|r| r["name"] == "Moselle"));
    let (status, _) = send(&app, "PATCH", "/v1/time", json!({"region": "XX"})).await;
    assert_eq!(status, 422);
    let (status, _) = send(&app, "PATCH", "/v1/time", json!({"country": "JP"})).await;
    assert_eq!(status, 422);
    let (status, _) = send(&app, "PATCH", "/v1/time", json!({"school_zone": "D"})).await;
    assert_eq!(status, 422);

    send(&app, "PATCH", "/v1/time", json!({"auto": false, "time_zone": "Europe/Paris"})).await;
    // the standard dates of a year: holidays (Moselle's Good Friday too), school holidays, clock changes
    let dates = get(&app, "/v1/dates?days=366&lang=fr").await;
    let all = dates["dates"].as_array().unwrap();
    for kind in ["holiday", "school", "clock_change"] {
        assert!(all.iter().any(|d| d["kind"] == kind), "{kind}: {dates}");
    }
    assert!(all.iter().any(|d| d["text"] == "jour férié (Moselle) : Vendredi saint"), "{dates}");
    assert!(all.iter().all(|d| d["in_days"].as_i64().unwrap() >= 0));

    // the Carnet's « Dates à venir »: computed entries, not saved; standard=0 leaves them out
    let carnet = get(&app, "/v1/notebook/dates?days=366").await;
    let computed: Vec<&Value> = carnet["dates"].as_array().unwrap().iter().filter(|d| d["computed"] == true).collect();
    assert!(!computed.is_empty(), "{carnet}");
    assert!(computed.iter().all(|d| d["fact"] == d["text"]));
    let plain = get(&app, "/v1/notebook/dates?days=366&standard=0").await;
    assert!(plain["dates"].as_array().unwrap().iter().all(|d| d["computed"] != true));
    app.shutdown().await;
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

#[tokio::test]
async fn the_agents_tool() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(setup(dir.path()));
    send(&app, "PUT", "/v1/time/device", json!({"time_zone": "Europe/Paris"})).await;
    send(&app, "PATCH", "/v1/time", json!({"country": "FR", "school_zone": "C"})).await;
    let listed = rpc(&app, "tools/list", json!({})).await;
    assert!(listed["tools"].as_array().unwrap().iter().any(|t| t["name"] == "upcoming_dates"));
    let result = rpc(&app, "tools/call", json!({"name": "upcoming_dates", "arguments": {"days": 366}})).await;
    assert_eq!(result["isError"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Time zone: Europe/Paris (now "), "{text}");
    assert!(text.contains("Country: France, French school zone: C."), "{text}");
    // in the agent's language (French: its personality's language block)
    assert!(text.contains("jour férié : Noël") || text.contains("jour férié : Jour de Noël"), "{text}");
    assert!(text.contains("vacances") && text.contains("passage à l'heure"), "{text}");
    app.shutdown().await;
}
