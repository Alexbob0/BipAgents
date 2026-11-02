//! « Me prévenir quand il a fini ou qu'il t'attend », per agent (its page in the apps): off, the agent's work ends
//! and its questions wait in the app without a notification. On by default.
//!
//!     PUT  /v1/agents/{agent_id}/notify    {on} → {id, on}   (404 for an unknown agent)
//!     data_dir/agent_notify.json           {"quiet": ["<agent id>", …]}
//!
//! `GET /v1/agents/{agent_id}/settings` gives `notify: {on}` (its presence tells the apps this bridge has it).
//! What stays silent is decided in push.rs: a reply ready, a room turn, a sub-task, a background task or a mailbox
//! scan ended, a question. Approvals and accord requests (site, login, held e-mail, connector, vault) always notify,
//! so an agent is never stuck without the person knowing (Alex, 2026-11-02). The Boîte's scheduled messages (each
//! muted on its own, cronwatch.rs), the family's messages and the Live Activities are not this switch's either.
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::put;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::py;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent_notify.json")
}

fn read_quiet(config: &Config) -> Vec<String> {
    let text = std::fs::read_to_string(path(config)).unwrap_or_default();
    match serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("quiet").cloned()) {
        Some(Value::Array(items)) => items.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// The agent's work and questions notify the person (the default).
pub fn notifies(config: &Config, agent: &str) -> bool {
    !read_quiet(config).iter().any(|a| a == agent)
}

/// Turns the agent's notifications on or off. Returns the state kept.
pub fn set_notifies(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut quiet = read_quiet(config);
    let was_on = !quiet.iter().any(|a| a == agent);
    if was_on == on {
        return Ok(on);
    }
    if on {
        quiet.retain(|a| a != agent);
    } else {
        quiet.push(agent.to_string());
    }
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!({"quiet": quiet})).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)?;
    Ok(on)
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentNotify {
    pub on: bool,
}

impl Validate for AgentNotify {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

async fn put_notify(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentNotify>,
) -> ApiResult<Response> {
    let config = state.config();
    if !config.agents.contains_key(&agent_id) {
        return Err(HttpError::not_found());
    }
    let kept = tokio::task::spawn_blocking(move || set_notifies(&config, &agent_id, body.on).map(|on| (agent_id, on)))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    tracing::info!(agent = %kept.0, on = kept.1, "agent notifications changed");
    Ok(web::ok(json!({"id": kept.0, "on": kept.1})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/agents/{agent_id}/notify", put(put_notify))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.join("bridge.db").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn on_by_default_turned_off_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        assert!(notifies(&config, "maison"));
        assert!(!set_notifies(&config, "maison", false).unwrap());
        set_notifies(&config, "maison", false).unwrap(); // twice: kept once
        set_notifies(&config, "travail", false).unwrap();
        assert!(!notifies(&config, "maison"));
        assert!(notifies(&config, "coach"));
        let kept: Value = serde_json::from_str(&std::fs::read_to_string(path(&config)).unwrap()).unwrap();
        assert_eq!(kept, json!({"quiet": ["maison", "travail"]}));
        assert!(set_notifies(&config, "maison", true).unwrap());
        assert!(notifies(&config, "maison"));
        assert!(!notifies(&config, "travail"));
    }
}
