//! The approvals the person answered, kept for the conversation (owner's request 2026-10-22: an answered « Oui » /
//! « Non » vanished or came back when he reopened the app, as Hermes' stored history has no approval rows). The bridge
//! records each decision it relays (`/v1/approve`, from a notification) and each one an app reports after answering
//! Hermes itself (`POST /v1/approvals/answered`): the session, the run, the time, the request as the app showed it
//! (its command, description and topic), the decision and where it was given. The apps put them back in the thread at
//! their place, as final rows. One JSON line per decision in `<data_dir>/approvals/<agent>.jsonl`, the last 1000 kept.
//!
//! Not a Python module: Rust bridge only (owner group: push-runs).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Map, Value};

use crate::config::Config;

/// Per agent.
pub const KEEP: usize = 1000;
const CHOICES: [&str; 4] = ["once", "session", "always", "deny"];
static WRITING: Mutex<()> = Mutex::new(());

fn file(config: &Config, agent: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("approvals").join(format!("{agent}.jsonl"))
}

fn text(value: Option<&Value>, max: usize) -> Value {
    match value.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => json!(s.chars().take(max).collect::<String>()),
        None => Value::Null,
    }
}

/// The decision's identity: its run and request (or its command when Hermes gave no request id).
fn key(entry: &Map<String, Value>) -> String {
    let get = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let request = get("request_id");
    format!("{}/{}", get("run_id"), if request.is_empty() { get("command") } else { request })
}

/// Keeps one decision (a later one for the same request replaces it). `fields`: session_id, run_id, request_id,
/// command, description, topic, choice, via (« app », « notification »), at (Unix seconds; now when absent).
pub fn record(config: &Config, agent: &str, fields: &Map<String, Value>) -> anyhow::Result<Map<String, Value>> {
    let choice = fields.get("choice").and_then(Value::as_str).map(|c| c.trim().to_lowercase()).unwrap_or_default();
    if !CHOICES.contains(&choice.as_str()) {
        anyhow::bail!("choice: once, session, always or deny");
    }
    let run_id = text(fields.get("run_id"), 200);
    if run_id.is_null() {
        anyhow::bail!("run_id is required");
    }
    let mut entry = Map::new();
    entry.insert("session_id".into(), text(fields.get("session_id"), 200));
    entry.insert("run_id".into(), run_id);
    entry.insert("request_id".into(), text(fields.get("request_id"), 200));
    entry.insert("command".into(), text(fields.get("command"), 4000));
    entry.insert("description".into(), text(fields.get("description"), 1000));
    entry.insert("topic".into(), text(fields.get("topic"), 40));
    entry.insert("choice".into(), json!(choice));
    entry.insert("via".into(), text(fields.get("via"), 20));
    let at = fields.get("at").and_then(Value::as_f64).filter(|t| *t > 0.0).unwrap_or_else(crate::py::time_time);
    entry.insert("at".into(), json!(at));
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = file(config, agent);
    let mut kept = read(config, agent);
    let id = key(&entry);
    let before = kept.len();
    // answered again (a notification, then the app): the last word, with what the first one knew
    if let Some(earlier) = kept.iter().find(|e| key(e) == id) {
        for (name, value) in earlier {
            if entry.get(name).is_none_or(Value::is_null) {
                entry.insert(name.clone(), value.clone());
            }
        }
    }
    kept.retain(|e| key(e) != id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if kept.len() == before && kept.len() < KEEP + 200 {
        // a new one: appended
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(out, "{}", Value::Object(entry.clone()))?;
    } else {
        kept.push(entry.clone());
        let start = kept.len().saturating_sub(KEEP);
        let body: String = kept[start..].iter().map(|e| format!("{}\n", Value::Object(e.clone()))).collect();
        let part = path.with_extension("jsonl.part");
        crate::tools_knowledge::replace_with(&part, &path, body.as_bytes())?;
    }
    Ok(entry)
}

fn read(config: &Config, agent: &str) -> Vec<Map<String, Value>> {
    let Ok(body) = std::fs::read_to_string(file(config, agent)) else { return Vec::new() };
    body.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| v.as_object().cloned()).collect()
}

/// An agent's decisions, the oldest first; only one session's when `session` is given.
pub fn list(config: &Config, agent: &str, session: Option<&str>) -> Vec<Map<String, Value>> {
    let mut items: Vec<Map<String, Value>> = read(config, agent)
        .into_iter()
        .filter(|e| session.is_none() || e.get("session_id").and_then(Value::as_str) == session)
        .collect();
    let at = |e: &Map<String, Value>| e.get("at").and_then(Value::as_f64).unwrap_or(0.0);
    items.sort_by(|a, b| at(a).partial_cmp(&at(b)).unwrap_or(std::cmp::Ordering::Equal));
    items
}

/// A deleted agent: its decisions go with it.
pub fn forget(config: &Config, agent: &str) {
    let _ = std::fs::remove_file(file(config, agent));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn decisions_are_kept_once_per_request() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        let first = json!({"session_id": "s1", "run_id": "r1", "request_id": "q1", "command": "rm x", "choice": "once",
                           "at": 100.0, "via": "app"});
        record(&config, "home", &fields(first)).unwrap();
        record(
            &config,
            "home",
            &fields(json!({"session_id": "s2", "run_id": "r2", "choice": "deny", "at": 50.0,
                                                "command": "curl"})),
        )
        .unwrap();
        // the same request answered again (from a notification, then in the app): one row, the last word
        record(
            &config,
            "home",
            &fields(json!({"session_id": "s1", "run_id": "r1", "request_id": "q1",
                                                "choice": "DENY", "at": 120.0})),
        )
        .unwrap();
        let all = list(&config, "home", None);
        assert_eq!(all.len(), 2);
        assert_eq!((all[0]["run_id"].as_str(), all[1]["choice"].as_str()), (Some("r2"), Some("deny")));
        assert_eq!(list(&config, "home", Some("s1")).len(), 1);
        assert!(record(&config, "home", &fields(json!({"run_id": "r3", "choice": "maybe"}))).is_err());
        assert!(record(&config, "home", &fields(json!({"choice": "once"}))).is_err());
        forget(&config, "home");
        assert!(list(&config, "home", None).is_empty());
    }
}
