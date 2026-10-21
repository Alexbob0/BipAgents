//! How long the files sent to the agents are kept, the person's choice per place (decided 2026-10-21): 30 days (the
//! default, `[limits] upload_retention_days` until they choose), a year, or always. They count in the place's 5 GB
//! (place_backup.rs: the backups measure them, and uploads are refused once it is full).
//!
//!     GET   /v1/files/settings     {kept_days, choices, used_bytes, allowance_bytes}   (kept_days 0: always)
//!     PATCH /v1/files/settings     {kept_days} → the same
//!     data_dir/files.json          {"kept_days": N}
//!
//! No Python counterpart.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::py;
use crate::web::{self, ApiResult, FieldError, Json, Validate};

static LOCK: Mutex<()> = Mutex::new(());
/// 30 days, a year, always (0).
pub const CHOICES: [i64; 3] = [30, 365, 0];

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("files.json")
}

/// The days the files are kept; 0: they are never deleted.
pub fn kept_days(config: &Config) -> i64 {
    let text = std::fs::read_to_string(path(config)).unwrap_or_default();
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("kept_days").and_then(Value::as_i64))
        .filter(|d| CHOICES.contains(d))
        .unwrap_or(config.limits.upload_retention_days)
}

pub fn set_kept_days(config: &Config, days: i64) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!({"kept_days": days})).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

fn size_of(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => size_of(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0, // links are not followed
        })
        .sum()
}

/// What the files sent to this place's agents take now.
pub fn used_bytes(config: &Config) -> u64 {
    let mut roots: Vec<&str> =
        config.agents.values().filter_map(|a| a.upload_dir_host.as_deref()).filter(|r| !r.is_empty()).collect();
    roots.sort();
    roots.dedup();
    roots.into_iter().map(|r| size_of(Path::new(r))).sum()
}

fn settings(config: &Config) -> Value {
    json!({"kept_days": kept_days(config), "choices": CHOICES, "used_bytes": used_bytes(config),
           "allowance_bytes": crate::place_backup::ALLOWANCE_BYTES})
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesSettings {
    #[serde(default, deserialize_with = "web::lax::opt_int")]
    pub kept_days: Option<i64>,
}

impl Validate for FilesSettings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(days) = self.kept_days.filter(|d| !CHOICES.contains(d)) {
            errors.push(FieldError::new(
                "literal_error",
                vec![json!("body"), json!("kept_days")],
                "Input should be 30, 365 or 0",
                json!(days),
            ));
        }
    }
}

async fn get_settings(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let out = tokio::task::spawn_blocking(move || settings(&config)).await.map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

async fn patch_settings(State(state): State<AppState>, Json(body): Json<FilesSettings>) -> ApiResult<Response> {
    let config = state.config();
    let out = tokio::task::spawn_blocking(move || -> std::io::Result<Value> {
        if let Some(days) = body.kept_days {
            set_kept_days(&config, days)?;
            tracing::info!(days = days, "files kept changed");
        }
        Ok(settings(&config))
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/files/settings", get(get_settings).patch(patch_settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_kept_and_the_default_from_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        assert_eq!(kept_days(&config), 30);
        set_kept_days(&config, 0).unwrap();
        assert_eq!(kept_days(&config), 0);
        std::fs::write(path(&config), r#"{"kept_days": 12}"#).unwrap(); // not a choice: the default
        assert_eq!(kept_days(&config), 30);
    }
}
