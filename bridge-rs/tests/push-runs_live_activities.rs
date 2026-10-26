//! Live Activities (live_activities.rs): the app registers an activity's push token for a run, a background task or a
//! group discussion; the bridge pushes its steps (throttled), its phases (at once) and its end to APNs as `liveactivity`
//! pushes.

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use bipbridge::app::AppState;
use bipbridge::config::parse_config;
use common::fakes::*;
use common::*;
use serde_json::{json, Value};

const ACTIVITY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const GAP: f64 = 0.4;

struct Client {
    _dir: tempfile::TempDir,
    app: TestApp,
    backend: FakeBackend,
    apns: FakeApns,
}

impl Client {
    fn state(&self) -> AppState {
        self.app.app.state.clone()
    }
    async fn post(&self, path: &str, body: Value) -> TestResponse {
        let (k, v) = auth();
        self.app.send_json("POST", path, &[(k, v.as_str())], &body).await
    }
    async fn delete(&self, path: &str) -> TestResponse {
        let (k, v) = auth();
        self.app.send_json("DELETE", path, &[(k, v.as_str())], &Value::Null).await
    }
    fn script(&self, run_id: &str, items: Vec<RunItem>) {
        self.backend.with(|s| s.runs.insert(run_id.into(), items));
    }
    /// The live activity pushes seen by the fake APNs: (topic, push type, priority, payload).
    fn live(&self) -> Vec<(String, String, String, Value)> {
        self.apns
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.header("apns-push-type").as_deref() == Some("liveactivity"))
            .map(|r| {
                (
                    r.header("apns-topic").unwrap_or_default(),
                    r.header("apns-push-type").unwrap_or_default(),
                    r.header("apns-priority").unwrap_or_default(),
                    r.json(),
                )
            })
            .collect()
    }
    async fn close(self) {
        self.backend.release.set();
        self.app.shutdown().await;
    }
}

async fn client() -> Client {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &backend.urls());
    data["apns"]["base_url"] = json!(apns.url);
    let app = TestApp::new(parse_config(&data, None).unwrap());
    app.app.state.push.live.set_gap(GAP);
    Client { _dir: dir, app, backend, apns }
}

fn text(s: String) -> RunItem {
    RunItem::Text(s)
}

fn tool(run_id: &str, name: &str) -> RunItem {
    text(sse("tool.started", json!({"run_id": run_id, "tool": name, "preview": "https://www.kiabi.fr/robes"})))
}

#[tokio::test]
async fn registration_is_checked() {
    let c = client().await;
    let ok = c.post("/v1/live-activities", json!({"agent": "wellness", "run_id": "run_1", "token": ACTIVITY})).await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.json()["run_id"], "run_1");
    assert_eq!(ok.json()["state"], json!({"phase": "working", "subtasks": 0, "subtasksDone": 0}));
    assert_eq!(c.state().push.live.len(), 1);
    // the same token again replaces its registration
    c.post("/v1/live-activities", json!({"agent": "wellness", "job_id": "t0123456789ab", "token": ACTIVITY})).await;
    assert_eq!(c.state().push.live.len(), 1);

    let refused = [
        (json!({"agent": "wellness", "token": ACTIVITY}), 400),
        (json!({"agent": "wellness", "run_id": "r", "job_id": "t0123456789ab", "token": ACTIVITY}), 400),
        (json!({"agent": "wellness", "run_id": "run 1", "token": ACTIVITY}), 400),
        (json!({"agent": "wellness", "run_id": "r", "token": "not-hex"}), 400),
        (json!({"agent": "wellness", "run_id": "r", "token": ACTIVITY, "environment": "elsewhere"}), 400),
        // no publisher's relay set up on this bridge
        (
            json!({"agent": "wellness", "run_id": "r", "token": format!("r1.{}", "x".repeat(60)), "environment": "relay"}),
            409,
        ),
        (json!({"agent": "nobody", "run_id": "r", "token": ACTIVITY}), 404),
    ];
    for (body, status) in refused {
        assert_eq!(c.post("/v1/live-activities", body.clone()).await.status.as_u16(), status, "{body}");
    }
    let gone = c.delete(&format!("/v1/live-activities/{ACTIVITY}")).await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert!(c.state().push.live.is_empty());
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn steps_are_throttled_phases_and_the_end_go_at_once() {
    let c = client().await;
    let start = Flag::new();
    let approval = Flag::new();
    let end = Flag::new();
    c.script(
        "run_l",
        vec![
            RunItem::Wait(start.clone()),
            tool("run_l", "browser_navigate"),
            tool("run_l", "web_search"),
            tool("run_l", "web_extract"),
            RunItem::Wait(approval.clone()),
            text(sse("approval.request", json!({"run_id": "run_l", "request_id": "req_1", "command": "ls"}))),
            RunItem::Wait(end.clone()),
            text(sse("run.completed", json!({"run_id": "run_l", "output": "Voilà."}))),
        ],
    );
    let resp = c.post("/v1/live-activities", json!({"agent": "wellness", "run_id": "run_l", "token": ACTIVITY})).await;
    assert_eq!(resp.status, StatusCode::OK);
    c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_l"})).await;
    start.set();

    // the first step at once, the two next ones folded into one at the end of the gap
    assert!(wait_until(|| !c.live().is_empty(), Duration::from_secs(5)).await);
    let (topic, _, priority, first) = c.live()[0].clone();
    assert_eq!(topic, "io.github.bipagents.push-type.liveactivity");
    assert_eq!(priority, "5");
    assert_eq!(first["aps"]["event"], "update");
    assert_eq!(first["aps"]["content-state"]["tool"], "browser_navigate");
    assert!(first["aps"]["content-state"].get("detail").is_none()); // previews off: no site through Apple
    tokio::time::sleep(Duration::from_secs_f64(GAP / 4.0)).await;
    assert_eq!(c.live().len(), 1);
    assert!(wait_until(|| c.live().len() >= 2, Duration::from_secs(5)).await);
    tokio::time::sleep(Duration::from_secs_f64(GAP * 1.5)).await;
    let live = c.live();
    assert_eq!(live.len(), 2, "{live:?}");
    assert_eq!(live[1].3["aps"]["content-state"]["tool"], "web_extract");
    assert!(live[1].3["aps"]["timestamp"].as_i64() > live[0].3["aps"]["timestamp"].as_i64());

    // an approval at once, priority 10
    approval.set();
    assert!(wait_until(|| c.live().len() >= 3, Duration::from_secs(5)).await);
    let (_, _, priority, waiting) = c.live()[2].clone();
    assert_eq!((priority.as_str(), &waiting["aps"]["content-state"]["phase"]), ("10", &json!("approval")));

    // the end: « done », dismissed a few minutes later, and the registration let go
    end.set();
    assert!(wait_until(|| c.live().len() >= 4, Duration::from_secs(5)).await);
    let (_, _, _, done) = c.live()[3].clone();
    assert_eq!(done["aps"]["event"], "end");
    assert_eq!(done["aps"]["content-state"]["phase"], "done");
    assert!(done["aps"]["dismissal-date"].as_i64().unwrap() > done["aps"]["timestamp"].as_i64().unwrap());
    tokio::time::sleep(Duration::from_secs_f64(GAP * 2.0)).await;
    assert_eq!(c.live().len(), 4);
    c.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_background_task_is_followed_through_its_run_and_dead_tokens_are_dropped() {
    let c = client().await;
    let config = c.state().config();
    let mut job = serde_json::Map::new();
    for (k, v) in [
        ("id", json!("t0123456789ab")),
        ("kind", json!("task")),
        ("agent", json!("wellness")),
        ("title", json!("Restaurants à Bordeaux")),
        ("state", json!("running")),
        ("run_id", json!("run_job")),
    ] {
        job.insert(k.into(), v);
    }
    bipbridge::jobs::save(&config, &job).unwrap();
    let start = Flag::new();
    c.script(
        "run_job",
        vec![
            RunItem::Wait(start.clone()),
            text(sse("subagent.start", json!({"run_id": "run_job", "goal": "Chercher"}))),
            text(sse("run.failed", json!({"run_id": "run_job", "error": "boom"}))),
        ],
    );
    c.apns.responses.lock().unwrap().insert(OTHER.into(), (410, Some("Unregistered".into())));
    for token in [ACTIVITY, OTHER] {
        let resp = c
            .post("/v1/live-activities", json!({"agent": "wellness", "job_id": "t0123456789ab", "token": token}))
            .await;
        assert_eq!(resp.status, StatusCode::OK);
    }
    // another run of the same agent moves nothing
    c.script("run_other", vec![tool("run_other", "web_search"), text(sse("run.completed", json!({})))]);
    c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_other"})).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(c.live().is_empty());

    c.post("/v1/watch", json!({"agent": "wellness", "run_id": "run_job"})).await;
    start.set();
    let sent_to = |token: &str| c.apns.requests.lock().unwrap().iter().filter(|r| r.path.ends_with(token)).count();
    assert!(wait_until(|| sent_to(ACTIVITY) >= 2, Duration::from_secs(5)).await);
    let mine: Vec<Value> =
        c.apns.requests.lock().unwrap().iter().filter(|r| r.path.ends_with(ACTIVITY)).map(|r| r.json()).collect();
    assert_eq!(mine[0]["aps"]["content-state"]["subtasks"], 1);
    assert_eq!(mine[0]["aps"]["content-state"]["tool"], "delegate_task");
    assert_eq!(mine[1]["aps"]["event"], "end");
    assert_eq!(mine[1]["aps"]["content-state"]["phase"], "failed");
    // the dead token is forgotten at its first refusal; the other ended
    assert!(
        wait_until(|| c.state().push.live.snapshot().iter().all(|e| e.token != OTHER), Duration::from_secs(5)).await
    );
    c.close().await;
}
