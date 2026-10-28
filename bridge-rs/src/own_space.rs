//! An agent moves into its own space by itself the first time it needs to keep a secret (Alex, 2026-10-28:
//! « Pourquoi on complique ? … Le moment où ils rentrent un mot de passe, on annonce juste que ça se met en place et
//! voilà. »).
//!
//! Credentials are only held by an agent in its own space (vault.rs `holder`). An agent of the shared hub used not to
//! see `ask_secret`, the connectors' tools nor the vault's account tools, and improvised (« tape ton mot de passe dans
//! le navigateur »). Now they are offered to it too (never to the family's agent nor a project agent, never on an
//! install that doesn't protect the vault), and its first call moves it at once, with no card:
//!
//! - the conversation shows one plain line (`bridge.notice`, code `own_space_moving`, the apps word it in the
//!   person's language): « Je prépare un endroit sûr pour garder ton accès Garmin… (environ 1 min) »;
//! - the move is the app's (agents_admin.rs `move_into_space`: conversations, memory and files go with it), except
//!   that the hub keeps serving the profile until the turn that called ends there; then the conversations are copied
//!   once more (that turn's end included) and the hub forgets it;
//! - the call then goes on as if the agent had always been there: the secret card, the connector's card…;
//! - a move that fails: one plain line (`own_space_failed`), the agent is told to say it will try again later, and
//!   the next calls within `RETRY_AFTER` get the same answer without trying.
//!
//! **Since 2026-10-28 evening** (Alex: « Fais ce soir »), a person's own agents are no longer separated from each
//! other: in a hub that only the person's agents use, an agent holds credentials right where it is (vault.rs
//! `holds_in_hub`), and nothing moves. What must stay apart from the person's passwords is any agent other people
//! talk to (vault.rs `others_facing`: the family's agent, project agents, agents shared with a relative, agents linked
//! with another household's). As long as the hub hosts one of those, the move above is what happens, as before; the
//! credentials already kept in the hub stay there (logged at start and at each move, `Vault::warn_hub_holders`) until
//! their agent moves or the person takes them back. The trade-off, plainly: one of the person's agents tricked by what
//! it read could reach another of their agents' passwords; it costs no ~250 MB container per agent.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::Dict;

pub const MOVING: &str = "own_space_moving";
pub const FAILED: &str = "own_space_failed";
/// After a failed move, the calls within so long are answered without trying again.
pub const RETRY_AFTER: Duration = Duration::from_secs(120);
/// How long the hub keeps serving a moved agent's profile for a turn still going on there.
const HUB_WAIT: Duration = Duration::from_secs(30 * 60);

/// An agent of the shared hub that moves into its own space by itself when it needs to keep a secret: one that may not
/// hold it in the hub (`vault::holds_in_hub`: the hub hosts an agent other people talk to, or its tools have no
/// sandbox of their own), never the family's agent nor a project agent, never on an install that doesn't protect the
/// vault (`unprotected_install`). Asked again at each call.
pub fn may_move(config: &Config, agent: &AgentConfig) -> bool {
    crate::vault::holder(config, agent).as_deref() == Some("needs_own_space")
        && !crate::family_agent::is_family(config, &agent.name)
        && !crate::project_agent::is_project(config, &agent.name)
}

/// The agent may hold credentials now, or will once moved: the tools that keep some are offered.
pub fn may_hold(config: &Config, agent: &AgentConfig) -> bool {
    crate::vault::holder(config, agent).is_none() || may_move(config, agent)
}

/// The conversation's line while it moves (the apps show their own words for the code; older ones this).
pub fn moving_text(service: &str) -> String {
    if service.is_empty() {
        "Je prépare un endroit sûr pour garder ton accès… (environ 1 min)".into()
    } else {
        format!("Je prépare un endroit sûr pour garder ton accès {service}… (environ 1 min)")
    }
}

pub const FAILED_TEXT: &str = "Je n'ai pas pu préparer l'endroit sûr pour garder ton accès. Je réessaie un peu plus tard.";

/// What the agent's call gets when the move failed.
pub const FAILED_FOR_AGENT: &str = "Setting up a safe place to keep this access did not work this time; nothing was \
asked or kept. Tell the person in one plain sentence that you will try again a bit later (no technical words, never \
another way to give you the password), and call again later.";

static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);
static FAILURES: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

fn lock_of(agent: &str) -> Arc<tokio::sync::Mutex<()>> {
    LOCKS.lock().unwrap_or_else(|e| e.into_inner()).entry(agent.to_string()).or_default().clone()
}

fn failed_recently(agent: &str) -> bool {
    FAILURES.lock().unwrap_or_else(|e| e.into_inner()).get(agent).is_some_and(|at| at.elapsed() < RETRY_AFTER)
}

fn notice(state: &AppState, agent: &str, code: &str, text: &str, service: &str) {
    let mut data = Map::new();
    data.insert("type".into(), json!(crate::runs::NOTICE));
    data.insert("code".into(), json!(code));
    data.insert("text".into(), json!(text));
    data.insert("service".into(), json!(service));
    state.hub.notice_agent(agent, data);
}

/// The agent as it is when it may keep secrets, else moved into its own space now (the conversation says so), its
/// new configuration returned. Err: the plain result for the agent's call (the move failed).
pub async fn ready(state: &AppState, agent: &AgentConfig, service: &str) -> Result<AgentConfig, Dict> {
    if !may_move(&state.config(), agent) {
        return Ok(agent.clone());
    }
    let lock = lock_of(&agent.name);
    let _one_at_a_time = lock.lock_owned().await;
    // another call may have moved it meanwhile
    let config = state.config();
    let current = config.agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
    if !may_move(&config, &current) {
        return Ok(current);
    }
    if failed_recently(&current.name) {
        return Err(tool_text(FAILED_FOR_AGENT, true));
    }
    state.vault.warn_hub_holders(&config);
    notice(state, &current.name, MOVING, &moving_text(service), service);
    tracing::info!(target: "bipbridge::own_space", agent = %current.name, "moving into its own space to keep a secret");
    let hub_url = current.hermes_url.clone();
    match crate::agents_admin::move_into_space(state, &current, true).await {
        Ok(moved) => {
            FAILURES.lock().unwrap_or_else(|e| e.into_inner()).remove(&current.name);
            let (state2, name, new_url) = (state.clone(), current.name.clone(), moved.hermes_url.clone());
            let home = moved.hermes_home.clone();
            tokio::spawn(async move { finish(state2, name, hub_url, new_url, home).await });
            Ok(moved)
        }
        Err(exc) => {
            tracing::warn!(target: "bipbridge::own_space", agent = %current.name, error = ?exc, "move into its own space failed");
            FAILURES.lock().unwrap_or_else(|e| e.into_inner()).insert(current.name.clone(), Instant::now());
            notice(state, &current.name, FAILED, FAILED_TEXT, service);
            Err(tool_text(FAILED_FOR_AGENT, true))
        }
    }
}

/// Once the turns still going on in the hub have ended: their end copied into the space (unless the space has had a
/// turn of its own since), then the hub forgets the profile.
async fn finish(state: AppState, agent: String, hub_url: String, new_url: String, home: Option<String>) {
    let started = Instant::now();
    let had_turns = state.hub.agent_runs_at(&agent, &hub_url) > 0;
    let mut space_used = false;
    while state.hub.agent_runs_at(&agent, &hub_url) > 0 && started.elapsed() < HUB_WAIT {
        space_used |= state.hub.agent_runs_at(&agent, &new_url) > 0;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    space_used |= state.hub.agent_runs_at(&agent, &new_url) > 0;
    let config = state.config();
    let source = crate::spaces::hub_profile(&config, &agent);
    if let (true, false, Some(home)) = (had_turns, space_used, home) {
        let target = PathBuf::from(home);
        let copied = tokio::task::spawn_blocking(move || crate::spaces::resync_database(&source, &target)).await;
        if !matches!(copied, Ok(Ok(()))) {
            tracing::warn!(target: "bipbridge::own_space", agent = %agent, "the hub turn's end not copied into the space");
        }
    }
    let spaces = crate::spaces::Spaces::new(crate::agents_admin::default_run());
    match spaces.forget_in_hub(&config, &agent).await {
        Ok(()) => tracing::info!(target: "bipbridge::own_space", agent = %agent, "the hub forgot the moved agent"),
        Err(exc) => tracing::warn!(target: "bipbridge::own_space", agent = %agent, error = %exc, "the hub kept the moved agent's profile"),
    }
}

/// How long an agent other people talk to may stay busy before its move out of the hub is given up (tried again when
/// it is shared again).
const APART_WAIT: Duration = Duration::from_secs(6 * 60 * 60);
/// The move waits so long first: the app sets the new agent up (its look, its settings, its first words) without
/// waiting behind a move that holds the agents' lock for a minute.
const APART_SETTLE: Duration = Duration::from_secs(60);

/// An agent other people talk to (vault.rs `others_facing`) leaves the shared hub for its own space once idle, so the
/// hub stays the person's alone and their agents keep their credentials there (Alex, 2026-10-28). Called when the
/// family's agent or a project agent is made and when an agent is shared with a relative. Only on an install that
/// protects the vault (elsewhere nothing holds credentials, nothing to keep apart); a move that fails is logged and
/// the hub keeps it (its agents then move at their first secret, as before).
pub fn apart_when_idle(state: &AppState, agent: &str) {
    let (state, name) = (state.clone(), agent.to_string());
    tokio::spawn(async move { apart(state, name).await });
}

/// Whether `agent` should leave the hub now (`apart_when_idle`).
pub fn should_leave_hub(config: &Config, agent: &AgentConfig) -> bool {
    config.hermes.isolated
        && !agent.isolated
        && crate::vault::in_hub(config, agent)
        && crate::vault::others_facing(config, &agent.name)
}

async fn apart(state: AppState, name: String) {
    tokio::time::sleep(APART_SETTLE).await;
    let started = Instant::now();
    loop {
        let config = state.config();
        let Some(agent) = config.agent(Some(&name)).cloned() else { return };
        if !should_leave_hub(&config, &agent) {
            return;
        }
        if state.hub.agent_runs_at(&name, &agent.hermes_url) == 0 {
            let lock = lock_of(&name);
            let _one_at_a_time = lock.lock_owned().await;
            let config = state.config();
            let Some(agent) = config.agent(Some(&name)).cloned() else { return };
            if !should_leave_hub(&config, &agent) {
                return;
            }
            tracing::info!(target: "bipbridge::own_space", agent = %name, "other people talk to it: moving out of the shared hub");
            match crate::agents_admin::move_into_space(&state, &agent, false).await {
                Ok(_) => tracing::info!(target: "bipbridge::own_space", agent = %name, "in its own space: the hub is the person's alone"),
                Err(exc) => tracing::warn!(target: "bipbridge::own_space", agent = %name, error = ?exc, "move out of the shared hub failed: it stays there, the hub's agents move at their first secret"),
            }
            state.vault.warn_hub_holders(&state.config());
            return;
        }
        if started.elapsed() > APART_WAIT {
            tracing::warn!(target: "bipbridge::own_space", agent = %name, "still busy: stays in the shared hub for now");
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Whether `value` reads as the result of a failed move (tests).
pub fn is_failure(result: &Dict) -> bool {
    result.get("content").and_then(Value::as_array).and_then(|c| c.first()).and_then(|c| c.get("text")).and_then(Value::as_str)
        == Some(FAILED_FOR_AGENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_are_plain() {
        assert_eq!(moving_text("Garmin"), "Je prépare un endroit sûr pour garder ton accès Garmin… (environ 1 min)");
        assert!(moving_text("").ends_with("accès… (environ 1 min)"));
        for text in [moving_text("Garmin").as_str(), FAILED_TEXT, FAILED_FOR_AGENT] {
            for word in ["espace", "space", "sandbox", "container", "conteneur", "Hermes", "profile", "navigateur", "browser"] {
                assert!(!text.contains(word), "{word} in {text}");
            }
        }
        assert!(is_failure(&tool_text(FAILED_FOR_AGENT, true)));
    }

    #[test]
    fn a_failure_is_not_retried_at_once() {
        assert!(!failed_recently("unit-own-space"));
        FAILURES.lock().unwrap().insert("unit-own-space".into(), Instant::now());
        assert!(failed_recently("unit-own-space"));
        FAILURES.lock().unwrap().insert("unit-own-space".into(), Instant::now() - RETRY_AFTER - Duration::from_secs(1));
        assert!(!failed_recently("unit-own-space"));
    }
}
