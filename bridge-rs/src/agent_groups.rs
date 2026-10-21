//! The person's groups of agents (« Perso », « Pro », « Famille » or a name of their own), for the apps' home screen:
//! a free-text name per agent, empty for none. The apps gather the groups by name across places.
//!
//!     PUT  /v1/agents/{agent_id}/group     {group} → {id, group}   ("" removes it; 404 for an unknown agent)
//!     data_dir/agent_groups.json           {"agents": {"<agent id>": "<group>"}}
//!
//! `GET /v1/agents` lists each agent's `group` (the apps offer « Déplacer vers… » only when the key is there), and
//! `POST /v1/agents` takes one for the new agent.
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
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

static LOCK: Mutex<()> = Mutex::new(());
pub const MAX_LEN: usize = 40;

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent_groups.json")
}

fn read_all(config: &Config) -> Dict {
    let text = std::fs::read_to_string(path(config)).unwrap_or_default();
    match serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("agents").cloned()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

/// A group name as kept: trimmed, its runs of spaces and control characters made one space.
pub fn normalize(group: &str) -> String {
    group.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

/// The group of `agent` ("" for none).
pub fn group_of(config: &Config, agent: &str) -> String {
    read_all(config).get(agent).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// Every agent's group, read once (for the listing).
pub fn all(config: &Config) -> Dict {
    read_all(config)
}

/// Puts `agent` in `group` ("" takes it out of its group). Returns the name kept.
pub fn set_group(config: &Config, agent: &str, group: &str) -> std::io::Result<String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let group = normalize(group);
    let mut groups = read_all(config);
    if group.is_empty() {
        if groups.remove(agent).is_none() {
            return Ok(group);
        }
    } else {
        groups.insert(agent.to_string(), json!(group));
    }
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!({"agents": groups})).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)?;
    Ok(group)
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentGroup {
    #[serde(default)]
    pub group: String,
}

impl Validate for AgentGroup {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "group", Some(self.group.trim()), MAX_LEN);
    }
}

async fn put_group(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentGroup>,
) -> ApiResult<Response> {
    let config = state.config();
    if !config.agents.contains_key(&agent_id) {
        return Err(HttpError::not_found());
    }
    let group = tokio::task::spawn_blocking(move || set_group(&config, &agent_id, &body.group).map(|g| (agent_id, g)))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    tracing::info!(agent = %group.0, group = %group.1, "agent group changed");
    Ok(web::ok(json!({"id": group.0, "group": group.1})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/agents/{agent_id}/group", put(put_group))
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
    fn names_kept_tidy_and_removed() {
        assert_eq!(normalize("  Pro   perso\t\n"), "Pro perso");
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        assert_eq!(group_of(&config, "maison"), "");
        assert_eq!(set_group(&config, "maison", " Famille ").unwrap(), "Famille");
        set_group(&config, "travail", "Pro").unwrap();
        assert_eq!(group_of(&config, "maison"), "Famille");
        assert_eq!(set_group(&config, "maison", "  ").unwrap(), "");
        assert_eq!(group_of(&config, "maison"), "");
        assert_eq!(Value::Object(all(&config)), json!({"travail": "Pro"}));
    }
}
