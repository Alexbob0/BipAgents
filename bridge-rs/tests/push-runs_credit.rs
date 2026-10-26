//! The plan's AI credit used (credit.rs, Alex 2026-10-26): the agents' routines paused in Hermes' cron files with one
//! note in each agent's Boîte (never one per run, never twice), resumed when the credit is back.

mod common;

use bipbridge::config::parse_config;
use bipbridge::credit::{self, Credit, State};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

#[tokio::test]
async fn routines_pause_with_one_note_while_the_credit_is_used_and_resume_when_it_is_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["hermes"] = json!({"home": dir.path().join("hermes").to_string_lossy()});
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let state = app.app.state.clone();
    let config = state.config();
    let agent = config.agents.values().next().unwrap().clone();
    let file = bipbridge::agents_admin::profile_folder(&config, &agent).join("cron").join("jobs.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let routine = json!({"id": "j1", "name": "Météo", "enabled": true, "state": "scheduled",
                         "schedule": {"kind": "cron", "expr": "0 7 * * *"}});
    std::fs::write(&file, json!({"jobs": [routine]}).to_string()).unwrap();
    let job = || -> Value {
        serde_json::from_str::<Value>(&std::fs::read_to_string(&file).unwrap()).unwrap()["jobs"][0].clone()
    };
    let outbox = || async {
        let (k, v) = auth();
        app.get("/v1/outbox", &[(k, v.as_str())]).await.json()["items"].as_array().unwrap().clone()
    };

    // ok: nothing touched, nothing said
    credit::check(&state, Credit::OK, None).await;
    assert_eq!(job()["state"], "scheduled");
    assert!(outbox().await.is_empty());
    // full: the routine paused, one note in the agent's Boîte
    let full = Credit { state: State::Full, resumes: Some(4.0e9) };
    credit::check(&state, full, Some(false)).await;
    assert_eq!((job()["enabled"].clone(), job()["paused_reason"].clone()), (json!(false), json!(credit::PAUSE_REASON)));
    let items = outbox().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0]["text"].as_str().unwrap_or_default().contains("your routines are paused"), "{items:?}");
    // every minute after, and stopped (the grace used too): paused still, no second note
    credit::check(&state, full, Some(true)).await;
    credit::check(&state, Credit { state: State::Stopped, resumes: Some(4.0e9) }, Some(true)).await;
    assert_eq!(job()["state"], "paused");
    assert_eq!(outbox().await.len(), 1);
    // the credit back: resumed by itself
    credit::check(&state, Credit::OK, Some(true)).await;
    assert_eq!((job()["enabled"].clone(), job()["state"].clone()), (json!(true), json!("scheduled")));
    assert_eq!(job()["paused_reason"], Value::Null);
}
