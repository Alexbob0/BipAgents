//! Asking another agent of the same person, from any conversation (real-day bench 2026-10-28: « Demande à Coach… »
//! failed with Quotidien linked to Coach).
//!
//! Hermes injects its own `message_agent` only in an agent's canonical « Bot Chat » (tools/bot_mode_dm.py:
//! `message_agent_authorized`, the session's title): in the person's other conversations, the bench's, a routine's
//! or a background task's, the agent has no way to reach another one, whatever links the person turned on, and it
//! hunts for one (18 calls, tool_search ×9). So the bridge offers its own `message_agent` (`mcp__bipagents__
//! message_agent`) to an agent that may write to another agent of this place (the links of « Qui peut écrire à
//! qui », peers.rs; the same ones as `send_files_to_agent`). It checks the link, writes in the target's « Bot Chat »
//! signed as Hermes signs a peer's message (« Message from Quotidien (@quotidien): … », peers.rs `SIGNATURE`), and
//! gives back the target's answer in the same call: the asker can answer its person in the same turn. Another
//! household's agents are never reached here (family_news.rs stays the way there). During a relative's turn or in
//! a shared conversation the tool is closed like the person's other data (agent_shares.rs `RELATIVE_TOOLS`).
//! In the Bot Chat itself, where Hermes injects its own, Hermes patch 0012 takes this one out of the model's list
//! (the bridge can't: Hermes lists an MCP server's tools once, with nothing about the session).
//!
//! No Python counterpart.

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "message_agent";
/// Hermes' own cap on a DM (tools/bot_mode_dm.py `MESSAGE_MAX_CHARS`).
pub const MAX_CHARS: usize = 16000;
/// How long the target may take to answer (one turn of its own: a search, a look at its data).
pub const ANSWER_SECONDS: f64 = 600.0;

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Ask another of your person's agents (one your person lets you write to: see your contacts, \
e.g. bip/coach) and get its answer back in this same call. Write the message yourself: the point first, the precise \
question or what you need. Then answer your person with what it said.",
        "inputSchema": {"type": "object", "properties": {
            "target": {"type": "string", "description": "the other agent, as in your contacts (e.g. bip/coach) or by its name"},
            "message": {"type": "string", "description": "what you ask or tell it"},
        }, "required": ["target", "message"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
    })
}

/// Whether `agent` gets the tool: the person let it write to another agent of this place.
pub fn offered(config: &Config, agent: &str) -> bool {
    crate::agent_files::offered(config, agent)
}

/// The message as the target reads it: signed like a peer's (peers.rs `SIGNATURE`).
pub fn signed(sender: &AgentConfig, message: &str) -> String {
    format!("Message from {} (@{}): {}", crate::agent_files::shown(sender), sender.name, message.trim())
}

/// One call, for `agent`.
pub async fn call(http: &reqwest::Client, config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    let wanted = arguments.get("target").or_else(|| arguments.get("agent")).map(crate::py::py_str).unwrap_or_default();
    let message = arguments.get("message").or_else(|| arguments.get("text")).map(crate::py::py_str).unwrap_or_default();
    if message.trim().is_empty() {
        return tool_text("Give the message to send (`message`).", true);
    }
    if message.chars().count() > MAX_CHARS {
        return tool_text(&format!("The message is too long ({MAX_CHARS} characters at most): shorten it."), true);
    }
    let bare = wanted.trim().trim_start_matches("bip/");
    if crate::households::split(bare).is_some() {
        return tool_failure(
            "Another household's agents aren't reached from here (tell_family proposes it to your person).",
            "I can only write to your own agents from here.",
        );
    }
    let Some(target) = crate::agent_files::recipient(config, &wanted).cloned() else {
        let mine: Vec<String> = crate::agent_files::targets(config, &agent.name).map(|t| format!("bip/{t}")).collect();
        let allowed = if mine.is_empty() { "none yet".to_string() } else { mine.join(", ") };
        return tool_text(&format!("No agent {wanted:?} here. You may write to: {allowed}."), true);
    };
    if target.name == agent.name {
        return tool_text("That is you: name another agent.", true);
    }
    let shown = crate::agent_files::shown(&target).to_string();
    if !crate::agent_files::links(config).iter().any(|(from, to)| *from == agent.name && *to == target.name) {
        return tool_failure(
            &format!("The person hasn't let you write to {shown}."),
            &format!("I can't write to {shown} yet: you can allow it in the app, under « Qui peut écrire à qui »."),
        );
    }
    let hermes = crate::hermes::HermesClient::new(http.clone());
    let session = match hermes.bot_chat_sessions(&target).await {
        Ok(rows) if !rows.is_empty() => {
            rows.last().and_then(|r| r.get("id")).map(crate::py::py_str).filter(|id| !id.is_empty())
        }
        Ok(_) => None,
        Err(exc) => {
            tracing::warn!(agent = %agent.name, target = %target.name, error = %exc, "message to an agent: its conversations not read");
            return unreachable(&shown);
        }
    };
    let session = match session {
        Some(id) => id,
        None => match hermes.create_session(&target, crate::peers::BOT_CHAT_TITLE).await {
            Ok(id) => id,
            Err(exc) => {
                tracing::warn!(agent = %agent.name, target = %target.name, error = %exc, "message to an agent: no Bot Chat");
                return unreachable(&shown);
            }
        },
    };
    match hermes.session_chat(&target, &session, &signed(agent, &message), None, ANSWER_SECONDS).await {
        Ok(reply) if !reply.trim().is_empty() => {
            tracing::info!(agent = %agent.name, target = %target.name, "message to an agent answered");
            tool_text(&format!("{shown} (bip/{}) answered:\n{}", target.name, reply.trim()), false)
        }
        Ok(_) => tool_text(&format!("{shown} got your message but answered nothing."), false),
        Err(exc) => {
            tracing::warn!(agent = %agent.name, target = %target.name, error = %exc, "message to an agent not answered");
            unreachable(&shown)
        }
    }
}

fn unreachable(shown: &str) -> Dict {
    tool_failure(
        &format!("{shown} didn't answer (its server is busy or down). Don't retry now."),
        &format!("{shown} isn't answering right now. Try again a bit later."),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_is_signed_as_a_peers() {
        let sender = AgentConfig { name: "quotidien".into(), display_name: "Quotidien".into(), ..Default::default() };
        let text = signed(&sender, "  combien d'heures ?  ");
        assert_eq!(text, "Message from Quotidien (@quotidien): combien d'heures ?");
        assert_eq!(&crate::peers::SIGNATURE.captures(&text).unwrap()[1], "quotidien");
        assert_eq!(tool()["inputSchema"]["required"], json!(["target", "message"]));
    }
}
