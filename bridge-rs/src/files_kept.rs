//! How long the files sent to the agents are kept, the person's choice per place (decided 2026-10-21): 30 days (the
//! default, `[limits] upload_retention_days` until they choose), a year, or always. They count in the place's 5 GB
//! (place_backup.rs: the backups measure them, and uploads are refused once it is full).
//!
//!     GET   /v1/files/settings     {kept_days, choices, used_bytes, allowance_bytes}   (kept_days 0: always)
//!     PATCH /v1/files/settings     {kept_days} → the same
//!     data_dir/files.json          {"kept_days": N}
//!     data_dir/sent-photos.json    [{agent, path, at}]: the photos sent in a conversation (threads/send.rs
//!                                  `keep_photos`), each erased after `PHOTO_DAYS` unless the Carnet holds it
//!
//! A photo sent in a conversation and never filed is erased after 30 days whatever the choice (Carnet v2, Alex 9/11):
//! « filed » is a photo the Carnet names (a page holds its file's name) or one under `notebook/` (the Papiers, B3).
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

/// A sent photo never filed is kept this long.
pub const PHOTO_DAYS: f64 = 30.0;

fn photos_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("sent-photos.json")
}

fn photos(config: &Config) -> Vec<Value> {
    std::fs::read_to_string(photos_path(config))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn save_photos(config: &Config, list: &[Value]) -> std::io::Result<()> {
    let file = photos_path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!(list)).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

/// A photo sent to `agent`, kept with its uploads at `path` (the agent's view of it): noted, for `purge_photos`.
pub fn note_photo(config: &Config, agent: &str, path: &str, now: f64) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut list = photos(config);
    list.push(json!({"agent": agent, "path": path, "at": now.round()}));
    save_photos(config, &list)
}

/// The photo's file on this machine: under the agent's uploads, by its path as the agent sees it.
fn photo_file(config: &Config, agent: &str, path: &str) -> Option<PathBuf> {
    let a = config.agent(Some(agent))?;
    let host = a.upload_dir_host.as_deref().filter(|s| !s.is_empty())?;
    let container = a.upload_dir_container.as_deref().filter(|s| !s.is_empty())?;
    let relative = path.strip_prefix(container)?.trim_start_matches('/');
    let relative = Path::new(relative);
    if relative.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return None;
    }
    Some(Path::new(host).join(relative))
}

/// Whether the Carnet holds the photo: a page names its file, or it is under `notebook/`.
fn filed(config: &Config, file: &Path) -> bool {
    let notebook = crate::notebook::root(config);
    if file.starts_with(&notebook) {
        return true;
    }
    let Some(name) = file.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    [crate::notebook::Part::Main, crate::notebook::Part::Health].into_iter().any(|part| {
        crate::notebook::all_pages(config, part).iter().any(|(_, page)| page.body.contains(&name))
    })
}

/// The sent photos older than `PHOTO_DAYS` that the Carnet does not hold, erased. Returns how many; the agents whose
/// uploads changed come with them (their sandbox's copy to sync).
pub fn purge_photos(config: &Config, now: f64) -> (usize, Vec<String>) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let list = photos(config);
    let mut kept: Vec<Value> = Vec::new();
    let mut removed = 0;
    let mut agents: Vec<String> = Vec::new();
    for item in list {
        let agent = item.get("agent").and_then(Value::as_str).unwrap_or("").to_string();
        let path = item.get("path").and_then(Value::as_str).unwrap_or("").to_string();
        let at = item.get("at").and_then(Value::as_f64).unwrap_or(now);
        let Some(file) = photo_file(config, &agent, &path).filter(|f| f.is_file()) else { continue };
        if now - at < PHOTO_DAYS * 86400.0 {
            kept.push(item);
            continue;
        }
        if filed(config, &file) {
            continue; // the Carnet's now: no longer followed
        }
        if std::fs::remove_file(&file).is_ok() {
            removed += 1;
            if !agents.contains(&agent) {
                agents.push(agent);
            }
        } else {
            kept.push(item);
        }
    }
    if let Err(err) = save_photos(config, &kept) {
        tracing::warn!(error = %err, "sent photos' list not saved");
    }
    (removed, agents)
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

    #[test]
    fn a_sent_photo_never_filed_is_erased_after_30_days() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::notebook::tests_config(dir.path());
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(uploads.join("2026-10")).unwrap();
        let agent = "quotidien".to_string();
        config.agents.insert(agent.clone(), crate::config::AgentConfig {
            name: agent.clone(),
            display_name: "Quotidien".into(),
            upload_dir_host: Some(uploads.to_string_lossy().into_owned()),
            upload_dir_container: Some("/uploads".into()),
            ..Default::default()
        });
        let now = 1_800_000_000.0;
        for (name, age) in [("a1-old.jpg", 31.0), ("b2-filed.jpg", 31.0), ("c3-new.jpg", 2.0)] {
            std::fs::write(uploads.join("2026-10").join(name), b"jpg").unwrap();
            note_photo(&config, &agent, &format!("/uploads/2026-10/{name}"), now - age * 86400.0).unwrap();
        }
        let who = crate::notebook::Who::you(&config);
        crate::notebook::note(&config, crate::notebook::Part::Main, "subject", "Maison", "", &["Photo du compteur : b2-filed.jpg".into()], &[], "", "", &who).unwrap();
        let (removed, agents) = purge_photos(&config, now);
        assert_eq!((removed, agents), (1, vec![agent.clone()]));
        assert!(!uploads.join("2026-10/a1-old.jpg").exists());
        assert!(uploads.join("2026-10/b2-filed.jpg").exists(), "filed in the Carnet: kept");
        assert!(uploads.join("2026-10/c3-new.jpg").exists(), "2 days old: kept");
        assert_eq!(photos(&config).len(), 1, "only the new one is still followed");
        // a path that leads out of the uploads is never followed
        note_photo(&config, &agent, "/uploads/../etc/passwd", now - 40.0 * 86400.0).unwrap();
        assert_eq!(purge_photos(&config, now).0, 0);
    }
}
