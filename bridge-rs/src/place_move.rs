//! A planned move of this place to another server of the offer (installer/cloud/place-move.py, decided 2026-10-26):
//! the seat agents copy the place while it runs, then, in a quiet moment, the bridge drains it, the server stops it,
//! the last changes go over and the new server starts it with the same keys; the apps follow its new door as after an
//! outage. About a minute without the agents, nothing lost.
//!
//!     GET  /v1/place/move            what the move needs to pick its moment: turns and background tasks running now,
//!                                    each routine's next time, and the move's own state (idle, draining, switching)
//!     POST /v1/place/move/drain      {until?, back_in?, switching?}  the apps' writes answered 503 « moving » (they
//!                                    keep the person's sends), running background tasks paused at their next step,
//!                                    new ones queued, routines due before `until` paused; again with
//!                                    `switching: true` just before the server stops this place
//!     POST /v1/place/move/resume     after the switch (on the new server) or when the move is called off: the
//!                                    paused and queued tasks go on, the routines resume and those whose time passed
//!                                    run now (caught up)
//!
//! The move's state lives in `<data_dir>/place-move.json`: it is copied with the place, so the bridge on the new
//! server starts still « moving » until the move's last step resumes it there.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult};
use crate::Dict;

pub const FILE: &str = "place-move.json";
/// What the apps are told to expect (seconds), unless the seat agent says otherwise.
pub const BACK_IN: i64 = 60;
/// Routines due this long after the window's end are paused too (their run would start during the switch).
pub const MARGIN: f64 = 120.0;

static MOVING: OnceLock<AtomicBool> = OnceLock::new();

pub fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

pub fn read(file: &Path) -> Dict {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| match v {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default()
}

fn write(file: &Path, data: &Dict) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension("tmp");
    let mut fh = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    fh.write_all(&serde_json::to_vec(&Value::Object(data.clone())).unwrap_or_default())?;
    fh.sync_all()?;
    std::fs::rename(tmp, file)
}

fn flag(config: &Config) -> &'static AtomicBool {
    MOVING.get_or_init(|| AtomicBool::new(is_moving(&read(&path(config)))))
}

fn is_moving(data: &Dict) -> bool {
    matches!(data.get("state").and_then(Value::as_str), Some("draining") | Some("switching"))
}

/// The place is moving: background tasks queue instead of starting (jobs.rs).
pub fn moving(config: &Config) -> bool {
    flag(config).load(Ordering::SeqCst)
}

/// A request the move refuses: the apps' writes (their turns through the Hermes proxy, files, settings), never the
/// agents' own tools (a turn finishing during the drain still uses them) nor this module's routes.
pub fn refused(method: &str, path: &str) -> bool {
    if matches!(method, "GET" | "HEAD" | "OPTIONS") {
        return false;
    }
    let app = path.starts_with("/v1/") || path.starts_with("/hermes/");
    app && !path.starts_with("/v1/agent-tools/") && !path.starts_with("/v1/place/move")
}

/// The apps' answer while moving: 503, `Retry-After`, what to say (« Ton espace déménage, de retour dans une minute »).
pub fn moving_answer(data: &Dict) -> Response {
    let back_in = data.get("back_in").and_then(Value::as_i64).unwrap_or(BACK_IN);
    let state = data.get("state").and_then(Value::as_str).unwrap_or("draining");
    let mut response =
        web::json_response(503, &json!({"detail": "moving", "moving": {"state": state, "back_in": back_in}}));
    if let Ok(v) = back_in.to_string().parse() {
        response.headers_mut().insert("retry-after", v);
    }
    response
}

/// Middleware around the whole app (app.rs): while moving, the apps' writes wait (503) — the person's sends stay in
/// their app until the place is back.
pub async fn guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let config = state.config();
    if moving(&config) && refused(request.method().as_str(), request.uri().path()) {
        return moving_answer(&read(&path(&config)));
    }
    next.run(request).await
}

// -- the agents' routines (Hermes cron, through each agent's gateway) -----------------------------------------------------

async fn hermes_call(
    client: &reqwest::Client,
    agent: &AgentConfig,
    method: reqwest::Method,
    path: &str,
) -> Result<Value, String> {
    let response = client
        .request(method, format!("{}{}", agent.hermes_url.trim_end_matches('/'), path))
        .bearer_auth(&agent.hermes_key)
        .header("content-type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let text = response.text().await.map_err(|e| e.to_string())?;
    if !(200..300).contains(&status) {
        return Err(format!("{status}"));
    }
    Ok(serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// An ISO time (Hermes' `next_run_at`) in Unix seconds.
pub fn epoch(text: &str) -> Option<f64> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(text) {
        return Some(t.timestamp_millis() as f64 / 1000.0);
    }
    let naive = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    use chrono::TimeZone;
    chrono::Local.from_local_datetime(&naive).single().map(|t| t.timestamp_millis() as f64 / 1000.0)
}

/// An agent's routines: (id, name, enabled, next time).
async fn routines(client: &reqwest::Client, agent: &AgentConfig) -> Vec<(String, String, bool, Option<f64>)> {
    let Ok(data) = hermes_call(client, agent, reqwest::Method::GET, "/api/jobs").await else { return Vec::new() };
    let list = data.get("jobs").cloned().unwrap_or(data);
    let Value::Array(items) = list else { return Vec::new() };
    items
        .iter()
        .filter_map(|job| {
            let id = job.get("id")?.as_str()?.to_string();
            let name = job.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let enabled = job.get("enabled").and_then(Value::as_bool).unwrap_or(true)
                && job.get("state").and_then(Value::as_str) != Some("paused");
            let next = job.get("next_run_at").and_then(Value::as_str).and_then(epoch);
            Some((id, name, enabled, next))
        })
        .collect()
}

/// The routines to pause for a window ending at `until`: enabled, next time before it (with the margin).
pub fn due_before(found: &[(String, String, bool, Option<f64>)], until: f64) -> Vec<(String, f64)> {
    found
        .iter()
        .filter(|(_, _, enabled, next)| *enabled && next.is_some_and(|n| n <= until + MARGIN))
        .map(|(id, _, _, next)| (id.clone(), next.unwrap_or(0.0)))
        .collect()
}

// -- the routes -------------------------------------------------------------------------------------------------------------

async fn status_of(state: &AppState) -> Dict {
    let config = state.config();
    let data = read(&path(&config));
    let jobs: Vec<Value> = crate::jobs::tasks(&config)
        .into_iter()
        .filter(|j| matches!(j.get("state").and_then(Value::as_str), Some("running") | Some("stopping")))
        .map(|j| json!({"id": j.get("id"), "agent": j.get("agent"), "state": j.get("state")}))
        .collect();
    let mut found = Vec::new();
    let mut next_routine: Option<f64> = None;
    for agent in config.agents.values() {
        for (id, name, enabled, next) in routines(&state.hermes.client, agent).await {
            if enabled {
                if let Some(n) = next {
                    next_routine = Some(next_routine.map_or(n, |m: f64| m.min(n)));
                }
            }
            found.push(json!({"agent": agent.name, "id": id, "name": name, "enabled": enabled, "next_run_at": next}));
        }
    }
    let turns = state.hub.running();
    let mut out = Dict::new();
    out.insert("state".into(), data.get("state").cloned().unwrap_or(json!("idle")));
    out.insert("since".into(), data.get("since").cloned().unwrap_or(Value::Null));
    out.insert("back_in".into(), data.get("back_in").cloned().unwrap_or(Value::Null));
    out.insert("turns_running".into(), json!(turns));
    out.insert("jobs_running".into(), json!(jobs));
    out.insert("routines".into(), json!(found));
    out.insert("next_routine_at".into(), json!(next_routine));
    out.insert("paused_jobs".into(), data.get("paused_jobs").cloned().unwrap_or(json!([])));
    out.insert("postponed".into(), data.get("postponed").cloned().unwrap_or(json!([])));
    out.insert("quiet".into(), json!(turns == 0 && jobs.is_empty()));
    out
}

async fn status_route(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(status_of(&state).await)))
}

fn body_of(bytes: &Bytes) -> Dict {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(m)) => m,
        _ => Dict::new(),
    }
}

/// The drain: the apps' writes refused, running tasks paused at their next step, routines due in the window paused.
pub async fn drain(state: &AppState, asked: &Dict) -> anyhow::Result<Dict> {
    let config = state.config();
    let file = path(&config);
    let now = crate::py::time_time();
    let mut data = read(&file);
    if !is_moving(&data) {
        data = Dict::new();
        data.insert("since".into(), json!(now));
        data.insert("paused_jobs".into(), json!([]));
        data.insert("postponed".into(), json!([]));
    }
    let switching = asked.get("switching").and_then(Value::as_bool).unwrap_or(false);
    data.insert("state".into(), json!(if switching { "switching" } else { "draining" }));
    data.insert("back_in".into(), json!(asked.get("back_in").and_then(Value::as_i64).unwrap_or(BACK_IN)));
    write(&file, &data)?;
    flag(&config).store(true, Ordering::SeqCst);
    // Background tasks: stopped at their next step (Hermes interrupts between tool calls), paused « moving ».
    let jobs = crate::jobs::jobs_of(state);
    let mut paused: Vec<Value> = data.get("paused_jobs").and_then(Value::as_array).cloned().unwrap_or_default();
    for job in crate::jobs::tasks(&config) {
        if job.get("state").and_then(Value::as_str) != Some("running") {
            continue;
        }
        let id = job.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if jobs.pause_for_move(&id).await.unwrap_or(false) && !paused.contains(&json!(id)) {
            paused.push(json!(id));
        }
    }
    data.insert("paused_jobs".into(), json!(paused));
    // Routines due before the window ends: paused now, resumed and caught up after the switch.
    let until = asked.get("until").and_then(Value::as_f64).unwrap_or(now + 900.0);
    let mut postponed: Vec<Value> = data.get("postponed").and_then(Value::as_array).cloned().unwrap_or_default();
    for agent in config.agents.values() {
        let found = routines(&state.hermes.client, agent).await;
        for (id, due) in due_before(&found, until) {
            let known = postponed.iter().any(|p| p["agent"] == json!(agent.name) && p["id"] == json!(id));
            if known {
                continue;
            }
            let call = format!("/api/jobs/{id}/pause");
            if hermes_call(&state.hermes.client, agent, reqwest::Method::POST, &call).await.is_ok() {
                postponed.push(json!({"agent": agent.name, "id": id, "due": due}));
            }
        }
    }
    data.insert("postponed".into(), json!(postponed));
    write(&file, &data)?;
    tracing::info!(state = ?data.get("state"), tasks = paused.len(), routines = postponed.len(), "place moving");
    Ok(status_of(state).await)
}

async fn drain_route(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(drain(&state, &body_of(&body)).await?)))
}

/// After the switch (or a move called off): everything the drain held goes on.
pub async fn resume(state: &AppState) -> anyhow::Result<Dict> {
    let config = state.config();
    let file = path(&config);
    let data = read(&file);
    let now = crate::py::time_time();
    let mut routines_resumed = 0;
    let mut caught_up = Vec::new();
    let mut failed = Vec::new();
    for item in data.get("postponed").and_then(Value::as_array).cloned().unwrap_or_default() {
        let (Some(name), Some(id)) = (item["agent"].as_str(), item["id"].as_str()) else { continue };
        let Some(agent) = config.agent(Some(name)).cloned() else { continue };
        // Hermes may still be starting on the new server: a few tries.
        let mut done = false;
        for attempt in 0..12 {
            let call = format!("/api/jobs/{id}/resume");
            if hermes_call(&state.hermes.client, &agent, reqwest::Method::POST, &call).await.is_ok() {
                done = true;
                break;
            }
            if attempt < 11 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
        if !done {
            failed.push(json!({"agent": name, "id": id}));
            continue;
        }
        routines_resumed += 1;
        if item["due"].as_f64().is_some_and(|due| due <= now) {
            let call = format!("/api/jobs/{id}/run");
            if hermes_call(&state.hermes.client, &agent, reqwest::Method::POST, &call).await.is_ok() {
                caught_up.push(json!({"agent": name, "id": id}));
            }
        }
    }
    // Clear the state first: the tasks' launches below must not queue again.
    let _ = std::fs::remove_file(&file);
    flag(&config).store(false, Ordering::SeqCst);
    let jobs = crate::jobs::jobs_of(state);
    let mut resumed = Vec::new();
    for job in crate::jobs::tasks(&config) {
        let id = job.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let held = job.get("reason").and_then(Value::as_str) == Some("moving")
            && matches!(job.get("state").and_then(Value::as_str), Some("paused"));
        if held {
            match jobs.resume_after_move(&id).await {
                Ok(_) => resumed.push(json!(id)),
                Err(err) => failed.push(json!({"job": id, "error": err.to_string()})),
            }
        }
    }
    tracing::info!(tasks = resumed.len(), routines = routines_resumed, caught_up = caught_up.len(), "place moved in");
    let mut out = Dict::new();
    out.insert("state".into(), json!("idle"));
    out.insert("resumed_jobs".into(), json!(resumed));
    out.insert("routines_resumed".into(), json!(routines_resumed));
    out.insert("caught_up".into(), json!(caught_up));
    out.insert("failed".into(), json!(failed));
    Ok(out)
}

async fn resume_route(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(resume(&state).await?)))
}

/// `place_move_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/place/move", get(status_route))
        .route("/place/move/drain", post(drain_route))
        .route("/place/move/resume", post(resume_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_writes_wait() {
        assert!(refused("POST", "/hermes/vie/v1/runs"));
        assert!(refused("POST", "/v1/files"));
        assert!(refused("DELETE", "/v1/jobs/t0123456789ab"));
        assert!(refused("PUT", "/v1/agents/vie"));
        assert!(!refused("GET", "/v1/agents"));
        assert!(!refused("GET", "/hermes/vie/api/sessions"));
        assert!(!refused("POST", "/v1/agent-tools/mcp"));
        assert!(!refused("POST", "/v1/place/move/resume"));
        assert!(!refused("POST", "/peer/p/chi/v1/runs"));
        assert!(!refused("POST", "/v1place/move"));
    }

    #[test]
    fn the_answer_says_moving_and_when_back() {
        let mut data = Dict::new();
        data.insert("state".into(), json!("switching"));
        data.insert("back_in".into(), json!(45));
        let response = moving_answer(&data);
        assert_eq!(response.status().as_u16(), 503);
        assert_eq!(response.headers().get("retry-after").unwrap(), "45");
    }

    #[test]
    fn routines_due_in_the_window_are_paused() {
        let found = vec![
            ("a".to_string(), "soon".to_string(), true, Some(1000.0)),
            ("b".to_string(), "later".to_string(), true, Some(5000.0)),
            ("c".to_string(), "paused".to_string(), false, Some(900.0)),
            ("d".to_string(), "edge".to_string(), true, Some(2100.0)),
            ("e".to_string(), "never".to_string(), true, None),
        ];
        assert_eq!(due_before(&found, 2000.0), vec![("a".to_string(), 1000.0), ("d".to_string(), 2100.0)]);
    }

    #[test]
    fn iso_times() {
        assert_eq!(epoch("2026-10-26T15:00:00+00:00"), Some(1793026800.0));
        assert_eq!(epoch("2026-10-26T16:00:00.5+01:00"), Some(1793026800.5));
        assert!(epoch("soon").is_none());
    }

    #[test]
    fn the_state_file_survives_and_says_moving() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        assert!(!is_moving(&read(&file)));
        let mut data = Dict::new();
        data.insert("state".into(), json!("draining"));
        write(&file, &data).unwrap();
        assert!(is_moving(&read(&file)));
        data.insert("state".into(), json!("idle"));
        write(&file, &data).unwrap();
        assert!(!is_moving(&read(&file)));
    }
}
