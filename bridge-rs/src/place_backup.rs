//! This place's backups (installer/bipinstall/backup.py, every hour to the guichet), for its app: the key its phones
//! keep, to give it back to a new place after an outage (cloud_setup.py's restore), and when the last backup worked.
//!
//!     GET  /v1/backup        {enabled, key?, last_ok?, last_error?, failed_at?, used_bytes?, allowance_bytes}
//!     POST /v1/backup/now    one now (the hourly timer's service), e.g. before the person moves their agents
//!
//! Only on a place of the all-included offer (its settings exist); elsewhere `enabled` is false.
//!
//! The person's space (decided 2026-10-21): 5 GB of their own things — files, photos, notes, memory, conversations — as
//! the last backup measured them (everything it takes, caches, code and images apart). Once full, files and notes sent
//! to the bridge are refused (507) until some go; the guichet keeps more for the backups' history.
//!
//! Port of `bridge/bipbridge/place_backup.py` (owner group: browsers-places).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::web::{self, ApiResult};
use crate::Dict;

pub const SETTINGS: &str = ".config/bipagents/backup.json";
pub const STATE: &str = ".local/state/bipagents/backup.json";
pub const UNIT: &str = "bipagents-backup.service";
pub const ALLOWANCE_BYTES: i64 = 5 * (1 << 30);

/// `$HOME`, read each time (Python: `Path(os.environ.get("HOME") or Path.home())`).
pub fn home() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(crate::py::expanduser("~")),
    }
}

fn read(path: &Path) -> Dict {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| match v {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

pub fn status(home: &Path) -> Dict {
    let settings = read(&home.join(SETTINGS));
    let truthy = |k: &str| settings.get(k).map(crate::py::truthy).unwrap_or(false);
    let mut out = Dict::new();
    if !truthy("url") || !truthy("password") {
        out.insert("enabled".into(), json!(false));
        return out;
    }
    let state = read(&home.join(STATE));
    out.insert("enabled".into(), json!(true));
    out.insert("key".into(), settings["password"].clone());
    out.insert("allowance_bytes".into(), json!(ALLOWANCE_BYTES));
    for k in ["last_ok", "last_error", "failed_at"] {
        if let Some(v) = state.get(k).filter(|v| !v.is_null()) {
            out.insert(k.into(), v.clone());
        }
    }
    if let Some(bytes) = state.get("bytes").and_then(Value::as_i64) {
        out.insert("used_bytes".into(), json!(bytes));
    }
    out
}

/// True on a place of the offer whose last backup found its person's 5 GB used up.
pub fn storage_full(home: &Path) -> bool {
    let current = status(home);
    let used = current.get("used_bytes").and_then(Value::as_i64).unwrap_or(0);
    crate::py::truthy(&current["enabled"]) && used >= ALLOWANCE_BYTES
}

/// The files and notes sent to the bridge refused once the place's 5 GB are used up.
pub async fn refuse_when_full() -> ApiResult<()> {
    let full = tokio::task::spawn_blocking(|| storage_full(&home())).await.map_err(anyhow::Error::from)?;
    if full {
        return Err(crate::web::HttpError::new(507, "no room left: the 5 GB of your agents' space are used up"));
    }
    Ok(())
}

async fn backup_status() -> ApiResult<Response> {
    let out = tokio::task::spawn_blocking(|| status(&home())).await.map_err(anyhow::Error::from)?;
    Ok(web::ok(Value::Object(out)))
}

async fn backup_now() -> ApiResult<Response> {
    if !crate::py::truthy(&status(&home())["enabled"]) {
        return Ok(web::ok(json!({"started": false})));
    }
    let code = tokio::process::Command::new("systemctl")
        .args(["--user", "start", "--no-block", UNIT])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map_err(anyhow::Error::from)?;
    Ok(web::ok(json!({"started": code.success()})))
}

/// `place_backup_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/backup", get(backup_status)).route("/backup/now", post(backup_now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_and_the_last_backup_never_the_token() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Value::Object(status(dir.path())), json!({"enabled": false}));
        std::fs::create_dir_all(dir.path().join(".config/bipagents")).unwrap();
        std::fs::write(
            dir.path().join(SETTINGS),
            r#"{"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}"#,
        )
        .unwrap();
        let five_gb = ALLOWANCE_BYTES;
        assert_eq!(
            Value::Object(status(dir.path())),
            json!({"enabled": true, "key": "the-key", "allowance_bytes": five_gb})
        );
        std::fs::create_dir_all(dir.path().join(".local/state/bipagents")).unwrap();
        std::fs::write(dir.path().join(STATE), r#"{"last_ok": 1790000000.0, "snapshot": "abc", "last_error": null}"#)
            .unwrap();
        assert_eq!(
            Value::Object(status(dir.path())),
            json!({"enabled": true, "key": "the-key", "allowance_bytes": five_gb, "last_ok": 1790000000.0})
        );
        assert!(!storage_full(dir.path()));
        std::fs::write(dir.path().join(STATE), format!(r#"{{"last_ok": 1790000000.0, "bytes": {five_gb}}}"#)).unwrap();
        assert!(storage_full(dir.path()));
    }
}
