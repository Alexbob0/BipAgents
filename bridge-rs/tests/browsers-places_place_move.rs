//! A planned move (place_move.rs): the drain refuses the apps' writes with 503 « moving », pauses the routines due in
//! the window through the agent's Hermes, and the resume after the switch resumes them and runs those whose time
//! passed. The only test in this binary: the move's flag is the process's.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::Path;
use axum::routing::{get, post};
use axum::Router;
use common::{auth, config, spawn_fake, TestApp, Urls};
use serde_json::json;

#[tokio::test]
async fn drain_refuses_the_apps_writes_and_resume_catches_the_routines_up() {
    let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let soon = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
    let later = (chrono::Utc::now() + chrono::Duration::hours(5)).to_rfc3339();
    let (c1, c2, c3) = (calls.clone(), calls.clone(), calls.clone());
    let hermes = Router::new()
        .route(
            "/api/jobs",
            get(move || {
                let (soon, later) = (soon.clone(), later.clone());
                async move {
                    axum::Json(json!({"jobs": [
                        {"id": "j1", "name": "every five minutes", "enabled": true, "next_run_at": soon},
                        {"id": "j2", "name": "tonight", "enabled": true, "next_run_at": later},
                    ]}))
                }
            }),
        )
        .route(
            "/api/jobs/{id}/pause",
            post(move |Path(id): Path<String>| async move {
                c1.lock().unwrap().push(format!("pause {id}"));
                axum::Json(json!({"ok": true}))
            }),
        )
        .route(
            "/api/jobs/{id}/resume",
            post(move |Path(id): Path<String>| async move {
                c2.lock().unwrap().push(format!("resume {id}"));
                axum::Json(json!({"ok": true}))
            }),
        )
        .route(
            "/api/jobs/{id}/run",
            post(move |Path(id): Path<String>| async move {
                c3.lock().unwrap().push(format!("run {id}"));
                axum::Json(json!({"ok": true}))
            }),
        );
    let url = spawn_fake(hermes).await;
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(config(dir.path(), &Urls { hermes: url, ..Urls::default() }));
    let (k, v) = auth();
    let key = [(k, v.as_str())];

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
    assert_eq!(drained.json()["postponed"].as_array().unwrap().len(), 1);
    assert_eq!(*calls.lock().unwrap(), vec!["pause j1".to_string()]);

    // The apps' writes wait; their reads and the agents' tools go on.
    let refused = app.send_json("POST", "/hermes/Wellness/v1/runs", &key, &json!({"input": "hi"})).await;
    assert_eq!(refused.status, 503);
    assert_eq!(refused.header("retry-after").as_deref(), Some("50"));
    assert_eq!(refused.json(), json!({"detail": "moving", "moving": {"state": "draining", "back_in": 50}}));
    assert_eq!(app.get("/v1/agents", &key).await.status, 200);

    let switching = app.send_json("POST", "/v1/place/move/drain", &key, &json!({"switching": true})).await;
    assert_eq!(switching.json()["state"], "switching");
    assert_eq!(calls.lock().unwrap().len(), 1, "a routine is paused once");
    // The state file is what the new server's bridge starts with.
    let file = dir.path().join("data").join(bipbridge::place_move::FILE);
    assert!(file.exists());

    let resumed = app.request("POST", "/v1/place/move/resume", &key, None).await.json();
    assert_eq!(resumed["state"], "idle");
    assert_eq!(resumed["routines_resumed"], 1);
    assert_eq!(resumed["caught_up"], json!([]), "not due yet: Hermes runs it at its time");
    assert!(!file.exists());
    assert_eq!(*calls.lock().unwrap(), vec!["pause j1".to_string(), "resume j1".to_string()]);
    let after = app.send_json("POST", "/hermes/Wellness/v1/runs", &key, &json!({"input": "hi"})).await;
    assert_ne!(after.status, 503);

    // A routine whose time passed during the switch runs as soon as the place is back.
    std::fs::write(
        &file,
        json!({"state": "switching", "postponed": [{"agent": "Wellness", "id": "j9", "due": 1000.0}]}).to_string(),
    )
    .unwrap();
    let resumed = app.request("POST", "/v1/place/move/resume", &key, None).await.json();
    assert_eq!(resumed["caught_up"], json!([{"agent": "Wellness", "id": "j9"}]));
    assert!(calls.lock().unwrap().ends_with(&["resume j9".to_string(), "run j9".to_string()]));
}
