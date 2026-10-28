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
/// Hermes' words, the space cards' (storage_watch.rs) and the tip cards' (feature_tips.rs).
const CHOICES: [&str; 11] =
    ["once", "session", "always", "deny", "keep_last", "archive", "send", "delete", "try", "later", "never"];
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
        anyhow::bail!("choice: once, session, always or deny (a space card: keep_last, archive, send or delete)");
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

/// A deleted agent: its decisions go with it, and what it asked.
pub fn forget(config: &Config, agent: &str) {
    let _ = std::fs::remove_file(file(config, agent));
    let _ = std::fs::remove_file(asked_file(config, agent));
}

// --- what the agents asked -----------------------------------------------------------------------------------------
//
// Each approval Hermes asked (the run hub's, runs.rs), answered or not, one JSON line in
// `<data_dir>/approvals/<agent>.asked.jsonl`, the last `ASKED_KEPT`: the time, the run and request, Hermes' pattern key,
// the tool (an MCP tool's name from the trust gate), the command as Hermes showed it (already redacted by Hermes, cut),
// the bridge's plain sentence. Read by `bipbridge approvals` to see what an agent asks and why (owner's request
// 2026-10-26: « encore trop de demandes d'accord »). Nothing of the vault, no key, no environment.

/// Per agent.
pub const ASKED_KEPT: usize = 500;
const ASKED_COMMAND_CHARS: usize = 400;

fn asked_file(config: &Config, agent: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("approvals").join(format!("{agent}.asked.jsonl"))
}

/// The tool behind a request: an MCP tool's name (the trust gate's « MCP tool 'X' on UNTRUSTED server 'Y' »), a
/// plugin rule's tool, else Hermes' pattern key (« execute_code », « recursive delete »…).
pub fn tool_of(command: &str, pattern_key: &str) -> String {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix("MCP tool '") {
        if let Some((tool, after)) = rest.split_once('\'') {
            let server = after.split('\'').nth(1).unwrap_or("");
            return if server.is_empty() { tool.to_string() } else { format!("{server}.{tool}") };
        }
    }
    if let Some((tool, _)) = command.split_once(" (plugin approval rule)") {
        return tool.trim_matches(|c| c == '<' || c == '>').to_string();
    }
    if pattern_key.is_empty() {
        "?".to_string()
    } else {
        pattern_key.to_string()
    }
}

/// Keeps one request (`item`: the run hub's pending item). The same request seen again (a restart replays it): once.
pub fn asked(config: &Config, agent: &str, item: &Map<String, Value>) -> anyhow::Result<()> {
    let get = |k: &str| item.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let command = get("command");
    let mut entry = Map::new();
    entry.insert("at".into(), item.get("created_at").cloned().unwrap_or_else(|| json!(crate::py::time_time())));
    for key in ["run_id", "request_id", "session_id", "room_id", "pattern_key", "description", "bip_summary"] {
        entry.insert(key.into(), text(item.get(key), 1000));
    }
    entry.insert("tool".into(), json!(tool_of(&command, &get("pattern_key"))));
    entry.insert("command".into(), text(Some(&json!(command)), ASKED_COMMAND_CHARS));
    let _guard = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let path = asked_file(config, agent);
    let kept = read_asked(config, agent);
    let id = key(&entry);
    if kept.iter().any(|e| key(e) == id) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if kept.len() < ASKED_KEPT + 100 {
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(out, "{}", Value::Object(entry))?;
    } else {
        let mut kept = kept;
        kept.push(entry);
        let start = kept.len().saturating_sub(ASKED_KEPT);
        let body: String = kept[start..].iter().map(|e| format!("{}\n", Value::Object(e.clone()))).collect();
        let part = path.with_extension("jsonl.part");
        crate::tools_knowledge::replace_with(&part, &path, body.as_bytes())?;
    }
    Ok(())
}

fn read_asked(config: &Config, agent: &str) -> Vec<Map<String, Value>> {
    let Ok(body) = std::fs::read_to_string(asked_file(config, agent)) else { return Vec::new() };
    body.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter_map(|v| v.as_object().cloned()).collect()
}

/// One printed line per request, the newest last: « 2026-10-25 21:14  wellness  execute_code  → once (app)  <command> ».
pub fn report(config: &Config, agents: &[String], last: usize) -> Vec<String> {
    let mut rows: Vec<(f64, String)> = Vec::new();
    for agent in agents {
        let decisions = list(config, agent, None);
        for entry in read_asked(config, agent) {
            let at = entry.get("at").and_then(Value::as_f64).unwrap_or(0.0);
            let id = key(&entry);
            let answer = match decisions.iter().find(|d| key(d) == id) {
                Some(d) => {
                    let choice = d.get("choice").and_then(Value::as_str).unwrap_or("?");
                    match d.get("via").and_then(Value::as_str) {
                        Some(via) => format!("→ {choice} ({via})"),
                        None => format!("→ {choice}"),
                    }
                }
                None => "→ (no answer kept)".to_string(),
            };
            let get = |k: &str| entry.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let command: String = get("command").split_whitespace().collect::<Vec<_>>().join(" ");
            let command = if command.chars().count() > 120 {
                format!("{}…", command.chars().take(119).collect::<String>())
            } else {
                command
            };
            let when = chrono::DateTime::from_timestamp((at.max(0.0)) as i64, 0)
                .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "?".into());
            let room = if get("room_id").is_empty() { String::new() } else { "  [room]".to_string() };
            rows.push((at, format!("{when}  {agent}  {}  {answer}{room}  {command}", get("tool"))));
        }
    }
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let start = rows.len().saturating_sub(last);
    rows[start..].iter().map(|(_, line)| line.clone()).collect()
}

const USAGE: &str = "usage: bipbridge approvals [--agent AGENT] [--last N] [--config CONFIG]\n\
                     The approvals the agents asked (the newest last): time, agent, tool, answer, command.\n";

/// `bipbridge approvals [--agent X] [--last N]`: reads the bridge's own logs only (no Hermes, no network).
pub fn cli(argv: &[String]) -> i32 {
    let (mut config_path, mut agent, mut last) = (None::<String>, None::<String>, 30usize);
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" | "--agent" | "--last" if i + 1 < argv.len() => {
                let value = argv[i + 1].clone();
                match argv[i].as_str() {
                    "--config" => config_path = Some(value),
                    "--agent" => agent = Some(value),
                    _ => match value.parse::<usize>() {
                        Ok(n) if n > 0 => last = n,
                        _ => {
                            eprint!("{USAGE}");
                            return 2;
                        }
                    },
                }
                i += 2;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprint!("{USAGE}");
                eprintln!("bipbridge approvals: unknown argument {other}");
                return 2;
            }
        }
    }
    let config = match crate::config::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(exc) => {
            eprintln!("config error: {exc}");
            return 2;
        }
    };
    let agents: Vec<String> = match &agent {
        Some(name) => match config.agent(Some(name)) {
            Some(a) => vec![a.name.clone()],
            None => {
                eprintln!("{name}: no such agent");
                return 2;
            }
        },
        None => config.agents.keys().cloned().collect(),
    };
    let lines = report(&config, &agents, last);
    if lines.is_empty() {
        println!("No approval asked since this bridge keeps them (2026-10-26).");
    }
    for line in lines {
        println!("{line}");
    }
    0
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

    #[test]
    fn requests_are_kept_once_and_reported_with_their_answer() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        let code = fields(json!({"run_id": "r1", "request_id": "q1", "pattern_key": "execute_code", "created_at": 100.0,
                                 "command": "execute_code <<'PY'\nimport sqlite3\nPY", "bip_summary": "Wellness veut…"}));
        asked(&config, "wellness", &code).unwrap();
        asked(&config, "wellness", &code).unwrap(); // a restart replays it: once
        let gate = fields(json!({"run_id": "r2", "request_id": "q2", "pattern_key": "mcp_elicitation", "created_at": 200.0,
                                 "room_id": "g1",
                                 "command": "MCP tool 'notion-create-pages' on UNTRUSTED server 'notion' wants to run."}));
        asked(&config, "wellness", &gate).unwrap();
        record(&config, "wellness", &fields(json!({"run_id": "r1", "request_id": "q1", "choice": "once", "via": "app"})))
            .unwrap();
        let lines = report(&config, &["wellness".to_string()], 30);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("  wellness  execute_code  → once (app)  execute_code <<'PY' import sqlite3 PY"), "{}", lines[0]);
        assert!(lines[1].contains("  wellness  notion.notion-create-pages  → (no answer kept)  [room]  MCP tool"), "{}", lines[1]);
        assert_eq!(report(&config, &["wellness".to_string()], 1).len(), 1);
        assert_eq!(tool_of("<reminders_add> (plugin approval rule)", ""), "reminders_add");
        assert_eq!(tool_of("rm -rf x", "recursive delete"), "recursive delete");
        forget(&config, "wellness");
        assert!(report(&config, &["wellness".to_string()], 30).is_empty());
    }
}
