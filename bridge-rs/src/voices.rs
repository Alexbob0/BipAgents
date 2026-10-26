//! Which voice speaks for what: each agent has a Bip (Pocket) and, when the person has their own voice server, a voice
//! there; `[voice_server]` says which uses go to the server (the Live, the replies to voice notes, the reading of
//! messages and reports), the Bip speaks for the rest.
//!
//! - `GET  /v1/voices`                  -> the server's voices (the app shows them as « Voix 1, Voix 2… ») and its uses
//! - `PUT  /v1/voices/uses`             `{"live": true, "replies": false, "reading": true}`
//! - `PUT  /v1/agents/{id}/voices`      `{"voice": "pocket:galet", "server_voice": "server:fr/af_bella"}`
//!
//! Port of `bridge/bipbridge/voices.py` (owner group: voice-speech).

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config, VOICE_USES};
use crate::py;
use crate::voice_speech::pystr;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};

/// The agent's voice for `use`: its server voice when that use goes to the server, else its Bip.
pub fn voice_for(config: &Config, agent: Option<&AgentConfig>, r#use: Option<&str>) -> String {
    if let (Some(agent), Some(u), Some(server)) = (agent, r#use, config.voice_server.as_ref()) {
        if VOICE_USES.contains(&u) && server.uses.get(u).copied().unwrap_or(true) {
            if let Some(voice) = agent.server_voice.as_deref().filter(|v| !v.is_empty()) {
                return voice.to_string();
            }
        }
    }
    agent.and_then(|a| a.voice.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| config.default_voice.clone())
}

/// The Bip standing in for the agent's server voice when the server fails or is too slow.
pub fn relay_for(agent: Option<&AgentConfig>) -> Option<String> {
    agent.and_then(|a| a.voice.clone()).filter(|v| v.starts_with("pocket:"))
}

/// `values` written in the `[header]` table of a TOML file (replaced where they are, else added at its end; the
/// table is created when missing). None removes a key. Other lines stay as they are.
pub fn set_section_keys(path: &str, header: &str, values: &[(String, Option<Value>)]) -> anyhow::Result<()> {
    // Python's read_text translates \r\n and \r (universal newlines) before splitlines
    let text = std::fs::read_to_string(path)?;
    let mut lines = pystr::splitlines(&text);
    let title = format!("[{header}]");
    let start = match lines.iter().position(|line| pystr::strip(line) == title) {
        Some(i) => i,
        None => {
            lines.push(String::new());
            lines.push(title);
            lines.len() - 1
        }
    };
    let mut end = (start + 1..lines.len()).find(|&i| pystr::strip(&lines[i]).starts_with('[')).unwrap_or(lines.len());
    while end > start + 1 && pystr::strip(&lines[end - 1]).is_empty() {
        end -= 1; // new keys go before the blank lines that end the table
    }
    let mut pending: IndexMap<String, Option<Value>> = values.iter().cloned().collect();
    let mut kept: Vec<String> = Vec::new();
    for line in &lines[start + 1..end] {
        let key = if line.contains('=') && !pystr::lstrip(line).starts_with('#') {
            pystr::strip(line.split('=').next().unwrap_or("")).to_string()
        } else {
            String::new()
        };
        if let Some(value) = pending.shift_remove(&key) {
            if let Some(value) = value {
                kept.push(format!("{key} = {}", toml_value(&value)));
            }
        } else {
            kept.push(line.clone());
        }
    }
    for (key, value) in &pending {
        if let Some(value) = value {
            kept.push(format!("{key} = {}", toml_value(value)));
        }
    }
    let mut out: Vec<String> = lines[..start + 1].to_vec();
    out.extend(kept);
    out.extend_from_slice(&lines[end..]);
    let joined = out.join("\n");
    std::fs::write(path, format!("{}\n", joined.trim_end_matches('\n')))?;
    Ok(())
}

fn toml_value(value: &Value) -> String {
    match value {
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        other => py::Dumps::new().unicode().dumps(other), // a TOML basic string for str, a number for int
    }
}

#[derive(Debug, Deserialize)]
pub struct Uses {
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub live: Option<bool>,
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub replies: Option<bool>,
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub reading: Option<bool>,
}

impl Validate for Uses {}

#[derive(Debug, Deserialize)]
pub struct AgentAppearance {
    pub category: String,
    #[serde(default)]
    pub color: Option<String>,
    /// The costume chosen for its Bip (« Tenue »): "" clears it; left out (an older app), it stays.
    #[serde(default)]
    pub outfit: Option<String>,
    /// Event costumes (Halloween, Noël…) on or off for it; left out, it stays.
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub costumes: Option<bool>,
}

pub const COLOR_PATTERN: &str = "^#[0-9A-Fa-f]{6}$";

fn is_color(value: &str) -> bool {
    // pydantic-core checks patterns with Rust's regex: `$` is the very end
    let chars: Vec<char> = value.chars().collect();
    chars.len() == 7 && chars[0] == '#' && chars[1..].iter().all(|c| c.is_ascii_hexdigit())
}

impl Validate for AgentAppearance {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "category", Some(&self.category), 20);
        web::check_max_len(errors, "outfit", self.outfit.as_deref(), 30);
        if let Some(color) = &self.color {
            if !is_color(color) {
                errors.push(
                    FieldError::new(
                        "string_pattern_mismatch",
                        web::body_loc("color"),
                        format!("String should match pattern '{COLOR_PATTERN}'"),
                        json!(color),
                    )
                    .with_ctx(json!({"pattern": COLOR_PATTERN})),
                );
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentVoices {
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub server_voice: Option<String>,
}

impl Validate for AgentVoices {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "voice", self.voice.as_deref(), 300);
        web::check_max_len(errors, "server_voice", self.server_voice.as_deref(), 300);
    }
}

fn opt(value: &Option<String>) -> Option<String> {
    value.clone().filter(|v| !v.is_empty())
}

fn opt_json(value: &Option<String>) -> Value {
    value.clone().map(Value::String).unwrap_or(Value::Null)
}

async fn list_voices(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let server = match &config.voice_server {
        None => Value::Null,
        Some(server) => json!({"voices": state.tts.server_voices(300.0).await, "uses": server.uses}),
    };
    Ok(web::ok(json!({"pocket": state.tts.pocket.is_some(), "server": server})))
}

async fn set_uses(State(state): State<AppState>, Json(body): Json<Uses>) -> ApiResult<Response> {
    let config = state.config();
    if config.voice_server.is_none() {
        return Err(HttpError::new(409, "no voice server"));
    }
    let changed: Vec<(String, bool)> = [("live", body.live), ("replies", body.replies), ("reading", body.reading)]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k.to_string(), v)))
        .collect();
    if let Some(source) = &config.source_path {
        let values: Vec<(String, Option<Value>)> = changed.iter().map(|(k, v)| (k.clone(), Some(json!(v)))).collect();
        let source = source.clone();
        blocking(move || set_section_keys(&source, "voice_server", &values)).await?;
    }
    let uses = state.config.update(|c| match c.voice_server.as_mut() {
        Some(server) => {
            for (k, v) in &changed {
                server.uses.insert(k.clone(), *v);
            }
            server.uses.clone()
        }
        None => IndexMap::new(),
    });
    Ok(web::ok(json!(uses)))
}

async fn set_agent_voices(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AgentVoices>,
) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    let (voice, server_voice) = (opt(&body.voice), opt(&body.server_voice));
    if let Some(source) = &config.source_path {
        let values = vec![
            ("voice".to_string(), voice.clone().map(Value::String)),
            ("server_voice".to_string(), server_voice.clone().map(Value::String)),
        ];
        let (source, header) = (source.clone(), format!("agents.{}", agent.name));
        blocking(move || set_section_keys(&source, &header, &values)).await?;
    }
    let mut changed = agent;
    changed.voice = voice.clone();
    changed.server_voice = server_voice.clone();
    state.config.put_agent(changed);
    Ok(web::ok(json!({"voice": opt_json(&voice), "server_voice": opt_json(&server_voice)})))
}

/// The agent's Bip as the person chose it in the app (category, custom colour): every app shows the same one.
async fn set_agent_appearance(
    State(state): State<AppState>,
    Path(agent_id): Path<String>,
    Json(body): Json<AgentAppearance>,
) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    if !crate::souls::CATEGORIES.contains(&body.category.as_str()) {
        return Err(HttpError::new(400, "unknown category"));
    }
    let category = body.category.clone();
    let color = body.color.as_deref().filter(|c| !c.is_empty()).map(|c| c.to_uppercase());
    // Costumes: what the app sent, else what was kept (an app from before the costumes sends neither).
    let outfit = match &body.outfit {
        Some(o) => Some(o.trim().to_string()).filter(|o| !o.is_empty()),
        None => agent.outfit.clone(),
    };
    let costumes = body.costumes.or(agent.costumes).filter(|on| !on);
    if let Some(source) = &config.source_path {
        let values = vec![
            ("category".to_string(), Some(json!(category))),
            ("color".to_string(), color.clone().map(Value::String)),
            ("outfit".to_string(), outfit.clone().map(Value::String)),
            ("costumes".to_string(), costumes.map(Value::Bool)),
        ];
        let (source, header) = (source.clone(), format!("agents.{}", agent.name));
        blocking(move || set_section_keys(&source, &header, &values)).await?;
    }
    let mut changed = agent;
    changed.category = Some(category.clone());
    changed.color = color.clone();
    changed.outfit = outfit.clone();
    changed.costumes = costumes;
    state.config.put_agent(changed.clone());
    // an agent of no kind of ours (« Autre ») keeps in view what its category's agents do (tool_diet.rs)
    if let Err(exc) = crate::tool_diet::apply(&state.config(), &changed) {
        tracing::warn!(agent = %changed.name, error = %exc, "tools in view not set");
    }
    Ok(web::ok(json!({"category": category, "color": opt_json(&color), "outfit": outfit.unwrap_or_default(),
                      "costumes": costumes.unwrap_or(true)})))
}

async fn blocking(work: impl FnOnce() -> anyhow::Result<()> + Send + 'static) -> ApiResult<()> {
    Ok(tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)??)
}

/// `voices_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/voices", get(list_voices))
        .route("/voices/uses", put(set_uses))
        .route("/agents/{agent_id}/voices", put(set_agent_voices))
        .route("/agents/{agent_id}/appearance", put(set_agent_appearance))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_keys_replaced_added_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.toml");
        std::fs::write(&path, "[a]\nx = 1\n\n[agents.w]\nvoice = \"old\"\n# c = 2\nname = \"W\"\n\n[z]\n").unwrap();
        let p = path.to_str().unwrap();
        set_section_keys(
            p,
            "agents.w",
            &[
                ("voice".into(), None),
                ("server_voice".into(), Some(json!("server:fr/é"))),
                ("name".into(), Some(json!("V"))),
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[a]\nx = 1\n\n[agents.w]\n# c = 2\nname = \"V\"\nserver_voice = \"server:fr/é\"\n\n[z]\n"
        );
        set_section_keys(p, "voice_server", &[("live".into(), Some(json!(false)))]).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().ends_with("[z]\n\n[voice_server]\nlive = false\n"));
    }

    #[test]
    fn colors() {
        assert!(is_color("#a1B2c3"));
        assert!(!is_color("#a1B2c3\n"));
        assert!(!is_color("#a1B2c"));
        assert!(!is_color("a1B2c3f"));
    }
}
