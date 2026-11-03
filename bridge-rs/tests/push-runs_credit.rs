//! The plan's AI credit used (credit.rs, Alex 2026-10-26): the agents' routines skip their runs (routines.rs) with one
//! note in the Boîte of each agent that has some (never one per run, never twice); they run again when it is back.

mod common;

use bipbridge::config::parse_config;
use bipbridge::credit::{self, Credit, State};
use common::{auth, config_dict, TestApp, Urls};
use serde_json::json;

#[tokio::test]
async fn one_note_while_the_credit_is_used_for_an_agent_with_routines() {
    let dir = tempfile::tempdir().unwrap();
    let data = config_dict(dir.path(), &Urls::default());
    let app = TestApp::new(parse_config(&data, None).unwrap());
    let state = app.app.state.clone();
    let config = state.config();
    let agent = config.agents.values().next().unwrap().clone();
    let outbox = || async {
        let (k, v) = auth();
        app.get("/v1/outbox", &[(k, v.as_str())]).await.json()["items"].as_array().unwrap().clone()
    };
    let full = Credit { state: State::Full, resumes: Some(4.0e9) };
    // no routine: nothing to say
    credit::check(&state, full, Some(false)).await;
    assert!(outbox().await.is_empty());
    bipbridge::routines::create(&config, &agent.name, "Météo", "0 7 * * *", "Dis-moi la météo.", None).unwrap();
    // ok: nothing said
    credit::check(&state, Credit::OK, None).await;
    assert!(outbox().await.is_empty());
    // full: one note in the agent's Boîte, then never again this month (every minute, stopped too)
    credit::check(&state, full, Some(false)).await;
    let items = outbox().await;
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0]["text"].as_str().unwrap_or_default().contains("your routines are paused"), "{items:?}");
    credit::check(&state, full, Some(true)).await;
    credit::check(&state, Credit { state: State::Stopped, resumes: Some(4.0e9) }, Some(true)).await;
    assert_eq!(outbox().await.len(), 1);
    assert!(json!(bipbridge::routines::list(&config, None)[0].enabled) == json!(true)); // never paused: skipped
}
