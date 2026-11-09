//! The voice decider's escalation (9/11, phase 2: the decision is made on the phone). At each pause of a Live call
//! the phone decides by itself whether the Bip answers now, keeps listening or files a note (its own small model,
//! VoiceDecider.swift / VoiceDecider.kt). When that model is unsure (≈ 4 % of the pauses) it asks here, and this asks
//! the agent's own model for one word. No model is loaded in the bridge.
//!
//!     POST /v1/voice/escalate  {agent, mode: live|micro, text, before?: [..3]}  ->  {decision: <label>|null, ms}
//!
//! `decision` null: the agent's model didn't answer within `TIMEOUT` (2.5 s), refused, or has no OpenAI-compatible
//! API: the phone keeps its own model's answer. The phone gives up after 3 s (and on a 404 from an older bridge): the
//! turn then goes, as before the decider. Nothing is kept, and the words are never logged: only the decision and its
//! time.

use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::web::{self, check_max_len, ApiResult, FieldError, Json, Validate};

/// The agent's model is given this long; past it the phone keeps its own answer.
pub const TIMEOUT: Duration = Duration::from_millis(2500);

pub const LABELS: [&str; 4] = ["ecouter", "preparer", "lancer", "noter"];

/// The model's answer in one word: the first label it starts with, else the first it contains.
pub fn parse(text: &str) -> Option<&'static str> {
    let t = text.trim().to_lowercase();
    LABELS.iter().find(|l| t.starts_with(**l)).or_else(|| LABELS.iter().find(|l| t.contains(**l))).copied()
}

#[derive(Debug, Deserialize)]
pub struct Ask {
    pub agent: String,
    #[serde(default = "live")]
    pub mode: String,
    pub text: String,
    #[serde(default)]
    pub before: Vec<String>,
}

fn live() -> String {
    "live".into()
}

const ITEM_CHARS: usize = 4000;

impl Validate for Ask {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        let field = |name: &str, msg: &str| FieldError::new("value_error", vec![json!("body"), json!(name)], msg, Value::Null);
        check_max_len(errors, "agent", Some(&self.agent), 200);
        check_max_len(errors, "text", Some(&self.text), ITEM_CHARS);
        if !["live", "micro"].contains(&self.mode.as_str()) {
            errors.push(field("mode", "live or micro"));
        }
        if self.text.trim().is_empty() {
            errors.push(field("text", "words"));
        }
        if self.before.len() > 20 || self.before.iter().any(|s| s.chars().count() > ITEM_CHARS) {
            errors.push(field("before", "20 utterances at most, 4000 characters each"));
        }
    }
}

/// The spike's short prompt (research/voice-decider/llm_decider.py), hardened with its errors (practice repeated, a
/// question inside a text read aloud, a question to someone else).
pub const SYSTEM: &str = "You decide, at each pause of a user's speech, what a phone voice assistant does. Reply with one word:
ecouter = stay silent (not addressed to the assistant: accent practice, repeating or reading aloud, talking to someone else; or unfinished, filler)
preparer = a request to the assistant has begun but is not finished
lancer = answer now (finished question/request to the assistant, or \"vas-y\"/\"c'est tout\"/\"go ahead\"/its name + a question)
noter = only file the information and say \"OK, noted\" (\"note que\", \"pour info\", \"remember that\", or a fact clearly meant to be kept)
A sentence repeated for practice, a question inside a text being read, or a question to another person in the room is ecouter.";

pub fn user_message(ask: &Ask, agent_name: &str, language: &str) -> String {
    let mode = if ask.mode == "live" { "live call" } else { "dictation mic" };
    let start = ask.before.len().saturating_sub(3);
    let context = if ask.before.is_empty() {
        "- (nothing)".to_string()
    } else {
        ask.before[start..].iter().map(|c| format!("- {c}")).collect::<Vec<_>>().join("\n")
    };
    format!(
        "Mode: {mode}. Assistant's name: {agent_name}. Language: {language}.\nEarlier chunks:\n{context}\nLast chunk (just before the pause): {}\nAction:",
        ask.text.trim()
    )
}

/// An OpenAI-compatible model to ask.
pub struct ModelCall<'a> {
    pub base_url: &'a str,
    pub key: &'a str,
    pub model: &'a str,
}

/// The one-word question to `call`: its label, or None (refused, unreachable, no label in the reply). A GLM through
/// EUrouter thinks at its floor (model_route.rs), hence room for a few hundred tokens.
pub async fn ask_model(
    http: &reqwest::Client,
    call: &ModelCall<'_>,
    provider_order: &[String],
    ask: &Ask,
    agent_name: &str,
    language: &str,
) -> Option<&'static str> {
    let mut body = json!({"model": call.model, "temperature": 0, "max_tokens": 300,
                          "messages": [{"role": "system", "content": SYSTEM},
                                       {"role": "user", "content": user_message(ask, agent_name, language)}]});
    crate::model_route::add_to_body(provider_order, call.base_url, &mut body);
    let mut request = http.post(format!("{}/chat/completions", call.base_url.trim_end_matches('/'))).json(&body);
    if !call.key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", call.key));
    }
    let response = request.send().await.ok()?;
    if !response.status().is_success() {
        tracing::info!(status = response.status().as_u16(), "voice escalation: the agent's model refused");
        return None;
    }
    let data: Value = response.json().await.ok()?;
    parse(data.pointer("/choices/0/message/content").and_then(Value::as_str).unwrap_or(""))
}

async fn escalate_route(State(state): State<AppState>, Json(ask): Json<Ask>) -> ApiResult<Response> {
    let started = Instant::now();
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&ask.agent))?;
    let shown = if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() };
    let decision = match crate::mail_scan::model_of(&config, &agent) {
        Ok(model) => {
            let language = crate::souls::language_name(&crate::handback::language_of(&config, &agent));
            let call = ModelCall { base_url: &model["base_url"], key: &model["key"], model: &model["model"] };
            let asked = ask_model(&state.http, &call, &config.model_provider_order, &ask, &shown, language);
            tokio::time::timeout(TIMEOUT, asked).await.ok().flatten()
        }
        Err(_) => None,
    };
    let ms = (started.elapsed().as_secs_f64() * 10_000.0).round() / 10.0;
    tracing::info!(agent = %agent.name, mode = %ask.mode, decision = decision.unwrap_or("none"), ms, "voice escalation");
    Ok(web::ok(json!({"decision": decision, "ms": ms})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/voice/escalate", post(escalate_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_word() {
        assert_eq!(parse(" Lancer."), Some("lancer"));
        assert_eq!(parse("Action: noter"), Some("noter"));
        assert_eq!(parse("?"), None);
    }

    #[test]
    fn the_question() {
        let ask = Ask { agent: "ours".into(), mode: "live".into(), text: " le dentiste ".into(),
                        before: ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect() };
        let m = user_message(&ask, "Ours", "French");
        assert!(m.starts_with("Mode: live call. Assistant's name: Ours. Language: French.\nEarlier chunks:\n- b\n- c\n- d\n"), "{m}");
        assert!(m.ends_with("Last chunk (just before the pause): le dentiste\nAction:"), "{m}");
    }
}
