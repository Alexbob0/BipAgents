//! A key of its own for each of the person's devices, besides the bridge key: one device lost is one key removed, the
//! others keep working. A device the person approves from another (the guichet's accounts) gets its key from this other
//! device, which asks the bridge for it and sends it sealed. Only their SHA-256 is kept here.
//!
//!     GET    /v1/device-keys        {devices: [{id, name, created, seen, me}]}
//!     POST   /v1/device-keys        {name}  ->  {id, key}          a key for another device of the person
//!     DELETE /v1/device-keys/{id}   that device can't come in any more
//!
//!     data_dir/device-keys.json   [{id, name, hash, created, seen}]   (chmod 600)
//!
//! Port of `bridge/bipbridge/device_keys.py` (owner group: agents).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::auth::Caller;
use crate::config::Config;
use crate::py::Clock;
use crate::web::{self, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::Dict;

pub const MAX_DEVICES: usize = 20;
pub const SEEN_EVERY: f64 = 3600.0;

fn hash(key: &str) -> String {
    hex::encode(Sha256::digest(key.as_bytes()))
}

static LOCK: Mutex<()> = Mutex::new(());

pub struct DeviceKeys {
    pub path: PathBuf,
    pub clock: Clock,
}

impl DeviceKeys {
    pub fn new(path: PathBuf) -> Self {
        DeviceKeys { path, clock: crate::py::wall_clock() }
    }

    fn read(&self) -> Vec<Dict> {
        let Ok(text) = std::fs::read_to_string(&self.path) else { return Vec::new() };
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Array(rows)) => rows
                .into_iter()
                .filter_map(|r| match r {
                    Value::Object(m)
                        if m.get("id").map(crate::py::truthy).unwrap_or(false)
                            && m.get("hash").map(crate::py::truthy).unwrap_or(false) =>
                    {
                        Some(m)
                    }
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn write(&self, rows: &[Dict]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let text = crate::py::dumps(&Value::Array(rows.iter().cloned().map(Value::Object).collect()));
        crate::py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &self.path)
    }

    /// The device whose key this is (its id), else None. Every key compared, in constant time.
    pub fn match_key(&self, presented: Option<&str>) -> Option<String> {
        let presented = presented.filter(|p| !p.is_empty())?;
        let wanted = hash(presented);
        let rows = self.read();
        let mut found: Option<Dict> = None;
        for row in &rows {
            let stored = row.get("hash").map(crate::py::py_str).unwrap_or_default();
            if crate::auth::constant_time_eq(stored.as_bytes(), wanted.as_bytes()) {
                found = Some(row.clone());
            }
        }
        let found = found?;
        let now = (self.clock)();
        let seen = found.get("seen").and_then(Value::as_f64).unwrap_or(0.0);
        if now - seen > SEEN_EVERY {
            let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let mut rows = self.read();
            for row in rows.iter_mut() {
                if row.get("id") == found.get("id") {
                    row.insert("seen".into(), json!(now));
                }
            }
            let _ = self.write(&rows);
        }
        found.get("id").map(crate::py::py_str)
    }

    /// A key for another device of the person: (its id, the key). Err (Python's `ValueError`): too many devices.
    pub fn mint(&self, name: &str) -> anyhow::Result<(String, String)> {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows = self.read();
        if rows.len() >= MAX_DEVICES {
            anyhow::bail!("too many devices: remove one first");
        }
        let (device, key) = (crate::py::token_hex(6), crate::py::token_urlsafe(32));
        let words: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(60).collect();
        let mut row = Dict::new();
        row.insert("id".into(), json!(device));
        row.insert("name".into(), json!(if words.is_empty() { "?".to_string() } else { words }));
        row.insert("hash".into(), json!(hash(&key)));
        row.insert("created".into(), json!((self.clock)()));
        row.insert("seen".into(), Value::Null);
        rows.push(row);
        self.write(&rows)?;
        Ok((device, key))
    }

    pub fn remove(&self, device: &str) -> bool {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let rows = self.read();
        let kept: Vec<Dict> = rows.iter().filter(|r| r.get("id") != Some(&json!(device))).cloned().collect();
        if kept.len() == rows.len() {
            return false;
        }
        if let Err(exc) = self.write(&kept) {
            tracing::error!(exc = %exc, "device keys not written");
        }
        true
    }

    pub fn clear(&self) {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(exc) = self.write(&[]) {
            tracing::error!(exc = %exc, "device keys not written");
        }
    }

    pub fn listed(&self, me: Option<&str>) -> Vec<Dict> {
        self.read()
            .into_iter()
            .map(|r| {
                let mut out = Dict::new();
                let id = r.get("id").cloned().unwrap_or(Value::Null);
                out.insert("id".into(), id.clone());
                let name = r.get("name").filter(|n| crate::py::truthy(n)).cloned().unwrap_or_else(|| json!("?"));
                out.insert("name".into(), name);
                out.insert("created".into(), r.get("created").cloned().unwrap_or(Value::Null));
                out.insert("seen".into(), r.get("seen").cloned().unwrap_or(Value::Null));
                out.insert("me".into(), json!(me.map(|m| id == json!(m)).unwrap_or(false)));
                out
            })
            .collect()
    }
}

/// `data_dir/device-keys.json`.
pub fn of(config: &Config) -> DeviceKeys {
    DeviceKeys::new(PathBuf::from(config.data_dir()).join("device-keys.json"))
}

#[derive(Debug, Deserialize)]
struct NewDevice {
    name: String,
}

impl Validate for NewDevice {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "name", Some(&self.name), 1);
        check_max_len(errors, "name", Some(&self.name), 60);
    }
}

async fn listed(State(state): State<AppState>, caller: Caller) -> Response {
    web::ok(json!({"devices": of(&state.config()).listed(caller.device.as_deref())}))
}

async fn mint(State(state): State<AppState>, Json(body): Json<NewDevice>) -> ApiResult<Response> {
    let keys = of(&state.config());
    match tokio::task::spawn_blocking(move || keys.mint(&body.name)).await.map_err(anyhow::Error::from)? {
        Ok((device, key)) => Ok(web::ok(json!({"id": device, "key": key}))),
        Err(exc) => Err(HttpError::new(409, exc.to_string())),
    }
}

async fn removed(State(state): State<AppState>, Path(device): Path<String>) -> ApiResult<Response> {
    if !of(&state.config()).remove(&device) {
        return Err(HttpError::new(404, "no such device"));
    }
    Ok(web::ok(json!({"ok": true})))
}

/// `device_keys_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/device-keys", get(listed).post(mint)).route("/device-keys/{device}", delete(removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_key_matches_and_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-keys.json");
        let key = "k".repeat(43);
        std::fs::write(
            &path,
            format!(
                "[{{\"id\": \"abc\", \"name\": \"iPad\", \"hash\": \"{}\", \"created\": 1, \"seen\": null}}]",
                hash(&key)
            ),
        )
        .unwrap();
        let keys = DeviceKeys::new(path.clone());
        assert_eq!(keys.match_key(Some(&key)).as_deref(), Some("abc"));
        assert_eq!(keys.match_key(Some("nope")), None);
        assert_eq!(keys.match_key(None), None);
        let rows: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(rows[0]["seen"].as_f64().unwrap() > 0.0);
    }
}
