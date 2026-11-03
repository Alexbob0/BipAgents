//! A planned move (place_move.rs): the drain refuses the apps' writes with 503 « moving » and notes the routines due
//! in the window (the bridge's clock runs none while moving, routines.rs), and the resume after the switch runs those
//! whose time passed. The only test in this binary: the move's flag is the process's.

mod common;

use common::{auth, config, TestApp, Urls};
use serde_json::json;

#[tokio::test]
async fn drain_refuses_the_apps_writes_and_resume_catches_the_routines_up() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let key = [(k, v.as_str())];
    let c = app.app.state.config();
    let agent = c.agents.values().next().unwrap().name.clone();
    let soon = bipbridge::routines::create(&c, &agent, "every five minutes", "every 5m", "x", None).unwrap().id;
    bipbridge::routines::create(&c, &agent, "tonight", "every 5h", "y", None).unwrap();

    let status = app.get("/v1/place/move", &key).await.json();
    assert_eq!(status["state"], "idle");
    assert_eq!(status["quiet"], true);
    assert_eq!(status["routines"].as_array().unwrap().len(), 2);
    assert!(status["next_routine_at"].as_f64().is_some());
    assert_eq!(app.get("/v1/place/move", &[]).await.status, 401);

    let until = chrono::Utc::now().timestamp() as f64 + 600.0;
    let drained = app.send_json("POST", "/v1/place/move/drain", &key, &json!({"until": until, "back_in": 50})).await;
    assert_eq!(drained.status, 200, "{}", drained.text());
    assert_eq!(drained.json()["state"], "draining");
    assert_eq!(drained.json()["postponed"], json!([{"agent": agent, "id": soon, "due": drained.json()["postponed"][0]["due"]}]));

    // The apps' writes wait; their reads and the agents' tools go on.
    let refused = app.send_json("POST", &format!("/hermes/{agent}/v1/runs"), &key, &json!({"input": "hi"})).await;
    assert_eq!(refused.status, 503);
    assert_eq!(refused.header("retry-after").as_deref(), Some("50"));
    assert_eq!(refused.json(), json!({"detail": "moving", "moving": {"state": "draining", "back_in": 50}}));
    assert_eq!(app.get("/v1/agents", &key).await.status, 200);

    let switching = app.send_json("POST", "/v1/place/move/drain", &key, &json!({"switching": true})).await;
    assert_eq!(switching.json()["state"], "switching");
    assert_eq!(switching.json()["postponed"].as_array().unwrap().len(), 1, "a routine is noted once");
    // The state file is what the new server's bridge starts with.
    let file = dir.path().join("data").join(bipbridge::place_move::FILE);
    assert!(file.exists());

    let resumed = app.request("POST", "/v1/place/move/resume", &key, None).await.json();
    assert_eq!(resumed["state"], "idle");
    assert_eq!(resumed["routines_resumed"], 1);
    assert_eq!(resumed["caught_up"], json!([]), "not due yet: the clock runs it at its time");
    assert!(!file.exists());
    let after = app.send_json("POST", &format!("/hermes/{agent}/v1/runs"), &key, &json!({"input": "hi"})).await;
    assert_ne!(after.status, 503);

    // A routine whose time passed during the switch runs as soon as the place is back.
    std::fs::write(&file, json!({"state": "switching", "postponed": [{"agent": agent, "id": soon, "due": 1000.0}]}).to_string())
        .unwrap();
    let resumed = app.request("POST", "/v1/place/move/resume", &key, None).await.json();
    assert_eq!(resumed["caught_up"], json!([{"agent": agent, "id": soon}]));
}
