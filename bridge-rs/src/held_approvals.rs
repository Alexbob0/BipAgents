//! What an agent asks while nobody is there (Alex 12/11: « une routine de nuit qui range les e-mails à 3 h ne déplace
//! jamais rien : personne ne valide à 3 h, la carte expire, le travail est perdu »).
//!
//! A turn nobody follows (any background turn: a routine, a watcher's or a trigger's wake-up, a background task, a
//! resumed turn after a card — `RunHub::mark_unattended`, with no app following the run, not a room's, not a
//! relative's) no longer waits inside Hermes for an answer that can only come hours later:
//!
//! - **A Hermes approval** (a command, a write in a service…) that neither the person's level nor a « Toujours »
//!   approves becomes a card of the bridge's own (`bip-held-<id>`): the same plain sentence, kept on disk, in the
//!   Boîte and in that turn's conversation (card_place.rs), pushed once. The run is told in a hidden note that it waits
//!   for the person (`HELD_NOTE`), Hermes is answered « deny » at once: the turn ends with its report in seconds.
//!   The same command asked again while its card waits joins that card; similar ones fold into one (card_place.rs
//!   `fold_key`: same agent and kind).
//! - **« Oui » later**: a one-use grant for exactly that command (`GRANT_SECONDS`), then the agent goes on in that
//!   conversation (card_place.rs `settle`, the note says to do exactly that again); the identical approval in that turn
//!   is answered « once » by the bridge (`take_grant`, runs.rs). A routine's turn: its report lands in the Boîte as the
//!   routine's (routines.rs `deliver_resumed`). « Non »: told, nothing done.
//! - **A question** (`clarify`) is answered at once with `QUESTION_NOTE` (nobody can answer now; a card for an OK,
//!   else the report says what is needed) instead of waiting an hour.
//! - **How long**: a card waits until answered, at most `KEEP_SECONDS`, or until its routine's next run (which asks
//!   again if it is still needed).
//!
//! A family member's routine (`by`) keeps today's way (its cards are hers, family_thread.rs).
//!
//!     <data>/held-approvals.json (0600)   {held: [...], grants: [...]}
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::{py, Dict};

pub const RUN_PREFIX: &str = "bip-held-";
const FILE: &str = "held-approvals.json";
/// A held card waits at most this long (or until its routine runs again).
pub const KEEP_SECONDS: f64 = 24.0 * 3600.0;
/// An approved one may be done this long after the « Oui ».
pub const GRANT_SECONDS: f64 = 2.0 * 3600.0;
/// Answered cards are kept this long (the resumed turn reads its command).
const ANSWERED_SECONDS: f64 = 2.0 * 86400.0;

/// What the run reads before Hermes' « denied » (steered in: a hidden note, not in the timeline).
pub const HELD_NOTE: &str = "<!--bip:held\nNot from the person: they are not here now (this turn runs on its own). \
What you just asked waits for them as a card in their app; it was not refused. When they say yes, you will be told and \
do it then. Don't try another way meanwhile. Finish what does not need it, then end with your report, saying in one \
sentence what waits for their OK.\n-->";

/// Hermes' question answered at once in a turn nobody follows.
pub const QUESTION_NOTE: &str = "Nobody can answer now: your person is not here (this turn runs on its own). Don't ask \
questions in this turn. For an action that needs their OK, use your tool that makes a card for it (it waits for them \
and is done when they say yes). Otherwise decide what is reasonable, or end with your report saying what you need from \
them.";

/// One approval kept for the person.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub id: String,
    pub agent: String,
    /// approval_level.rs `classify_hermes`.
    pub kind: String,
    /// The Hermes run that asked, and its conversation.
    pub run: String,
    #[serde(default)]
    pub session: Option<String>,
    pub command: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub pattern_key: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub detail: String,
    /// The routine whose run asked (its next run ends the wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine: Option<String>,
    pub at: f64,
    pub until: f64,
    /// The person's answer (`once` / `deny`) and when: no more waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered: Option<(String, f64)>,
}

/// An approved command its agent may run once.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub agent: String,
    pub command: String,
    pub card: String,
    pub until: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    held: Vec<Held>,
    #[serde(default)]
    grants: Vec<Grant>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

fn load(config: &Config) -> Book {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn change<T>(config: &Config, f: impl FnOnce(&mut Book) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut book = load(config);
    let out = f(&mut book);
    let now = py::time_time();
    book.held.retain(|h| match &h.answered {
        Some((_, at)) => now - at < ANSWERED_SECONDS,
        None => now < h.until,
    });
    book.grants.retain(|g| now < g.until);
    let file = path(config);
    let saved = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = file.with_extension("tmp");
        let text = serde_json::to_string(&book).map_err(std::io::Error::other)?;
        py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &file)
    })();
    if let Err(err) = saved {
        tracing::warn!(target: "bipbridge::held_approvals", error = %err, "held approvals not kept");
    }
    out
}

fn text(data: &Map<String, Value>, key: &str) -> String {
    data.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Approval `data` (the hub's pending item: command, bip_summary…) of `agent`'s run `run` kept as a card, until
/// `until`. The same command already waiting: that card, renewed (`true`: joined).
pub fn hold(
    config: &Config,
    agent: &str,
    run: &str,
    session: Option<&str>,
    data: &Map<String, Value>,
    routine: Option<&str>,
    until: f64,
) -> (Held, bool) {
    let now = py::time_time();
    let command = text(data, "command");
    let summary = Some(text(data, "bip_summary")).filter(|s| !s.is_empty()).unwrap_or_else(|| text(data, "description"));
    let fresh = Held {
        id: py::token_hex(6),
        agent: agent.to_string(),
        kind: crate::approval_level::classify_hermes(data).to_string(),
        run: run.to_string(),
        session: session.map(str::to_string),
        command: command.clone(),
        description: text(data, "description"),
        pattern_key: text(data, "pattern_key"),
        summary,
        detail: text(data, "bip_detail"),
        routine: routine.map(str::to_string),
        at: now,
        until: until.max(now + 60.0),
        answered: None,
    };
    let (held, joined) = change(config, |b| {
        match b.held.iter_mut().find(|h| h.answered.is_none() && h.agent == agent && h.command == command) {
            Some(waiting) => {
                // the same thing asked again (a later run): one card, where it was asked last
                waiting.run = fresh.run.clone();
                waiting.session = fresh.session.clone();
                waiting.routine = fresh.routine.clone();
                waiting.until = fresh.until;
                (waiting.clone(), true)
            }
            None => {
                b.held.push(fresh.clone());
                (fresh, false)
            }
        }
    });
    tracing::info!(target: "bipbridge::held_approvals", agent, card = %held.id, kind = %held.kind, joined, "approval held for the person");
    (held, joined)
}

/// The person's answer to card `id` (`allow`: a one-use grant for its command). None: no such card waiting.
pub fn answer(config: &Config, agent: &str, id: &str, allow: bool) -> Option<Held> {
    let now = py::time_time();
    change(config, |b| {
        let held = b.held.iter_mut().find(|h| h.agent == agent && h.id == id && h.answered.is_none() && now < h.until)?;
        held.answered = Some((if allow { "once" } else { "deny" }.to_string(), now));
        let held = held.clone();
        if allow {
            b.grants.push(Grant {
                agent: agent.to_string(),
                command: held.command.clone(),
                card: held.id.clone(),
                until: now + GRANT_SECONDS,
            });
        }
        tracing::info!(target: "bipbridge::held_approvals", agent, card = id, allow, "held approval answered");
        Some(held)
    })
}

/// A grant for exactly `command` of `agent`, taken (used once). None: nothing approved for it.
pub fn take_grant(config: &Config, agent: &str, command: &str) -> Option<Grant> {
    if command.is_empty() || !path(config).exists() {
        return None;
    }
    let now = py::time_time();
    change(config, |b| {
        let at = b.grants.iter().position(|g| g.agent == agent && g.command == command && now < g.until)?;
        Some(b.grants.remove(at))
    })
}

/// A held card by its id (answered or not).
pub fn get(config: &Config, id: &str) -> Option<Held> {
    load(config).held.into_iter().find(|h| h.id == id)
}

/// The cards waiting, oldest first, in Hermes' approvals' shape.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    let mut held: Vec<Held> = load(config)
        .held
        .into_iter()
        .filter(|h| h.answered.is_none() && now < h.until && agent.is_none_or(|a| a == h.agent))
        .collect();
    held.sort_by(|a, b| a.at.total_cmp(&b.at));
    held.iter().map(item_of).collect()
}

pub fn item_of(h: &Held) -> Dict {
    let item = json!({
        "agent": h.agent,
        "run_id": format!("{RUN_PREFIX}{}", h.id),
        "request_id": h.id,
        "command": h.command,
        "description": if h.description.is_empty() { &h.summary } else { &h.description },
        "choices": ["once", "deny"],
        "created_at": h.at,
        "session_id": null,
        "pattern_key": h.pattern_key,
        "bip_summary": h.summary,
        "bip_detail": h.detail,
        "bip_held": {"kind": h.kind, "until": h.until},
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The line the resumed turn reads for card `card` (None: not a held card).
pub fn resume_words(config: &Config, card: &str, summary: &str, allowed: bool) -> Option<String> {
    let id = card.strip_prefix(RUN_PREFIX)?;
    let held = get(config, id)?;
    let what = if summary.is_empty() { held.summary.clone() } else { summary.to_string() };
    Some(if allowed {
        format!(
            "« {what} » — not done yet: do it yourself now, exactly as you asked it (it goes through without asking \
             this once): {}",
            crate::tools_knowledge::head(&held.command, 600)
        )
    } else {
        format!("« {what} »")
    })
}

// -- the hub's requests ---------------------------------------------------------------------------------------------

/// What the hub hands over from a turn nobody follows (runs.rs).
#[derive(Debug, Clone)]
pub enum Unattended {
    /// An approval Hermes waits for (`item`: the hub's pending item).
    Approval { agent: AgentConfig, run_id: String, session: Option<String>, item: Dict },
    /// A question Hermes waits for.
    Question { agent: AgentConfig, run_id: String, request_id: Option<String> },
}

/// The hub's channel set up, and its reader started (app.rs).
pub fn start(state: &AppState) {
    let (tx, rx) = mpsc::unbounded_channel();
    if state.hub.unattended.set(tx).is_ok() {
        let state = state.clone();
        tokio::spawn(run_forever(state, rx));
    }
}

async fn run_forever(state: AppState, mut rx: mpsc::UnboundedReceiver<Unattended>) {
    while let Some(request) = rx.recv().await {
        let state = state.clone();
        tokio::spawn(async move {
            match request {
                Unattended::Approval { agent, run_id, session, item } => held(&state, &agent, &run_id, session, item).await,
                Unattended::Question { agent, run_id, request_id } => {
                    let answered = state.hermes.answer_prompt(&agent, &run_id, "clarify", QUESTION_NOTE, request_id.as_deref()).await;
                    tracing::info!(target: "bipbridge::held_approvals", agent = %agent.name, run_id = %run_id,
                        status = ?answered.as_ref().map(|r| r.0).ok(), "question in a turn nobody follows: answered at once");
                    state.hub.forget_approval(&agent.name, &run_id, request_id.as_deref());
                }
            }
        });
    }
}

/// The routine whose run conversation is `session` (None: not a routine's).
async fn routine_of(state: &AppState, agent: &AgentConfig, session: Option<&str>) -> Option<crate::routines::Routine> {
    crate::routines::of_session(state, agent, session?).await
}

async fn held(state: &AppState, agent: &AgentConfig, run_id: &str, session: Option<String>, item: Dict) {
    let config = state.config();
    let request_id = item.get("request_id").and_then(Value::as_str).map(str::to_string);
    let routine = routine_of(state, agent, session.as_deref()).await;
    if routine.as_ref().is_some_and(|r| r.by.is_some()) {
        // a family member's routine: hers, today's way (pushed, Hermes waits)
        let summary = item.get("bip_summary").or_else(|| item.get("command")).and_then(Value::as_str).map(str::to_string);
        let choices: Vec<Value> = item.get("choices").and_then(Value::as_array).cloned().unwrap_or_default();
        state
            .push
            .notify_approval_values(&agent.name, run_id, request_id.as_deref(), Some(&choices), summary.as_deref(), session.as_deref(), None)
            .await;
        return;
    }
    let now = py::time_time();
    let until = routine
        .as_ref()
        .and_then(|r| r.next_run)
        .filter(|next| *next > now)
        .map_or(now + KEEP_SECONDS, |next| next.min(now + KEEP_SECONDS));
    let (c, name, run, s, it, r) =
        (config.clone(), agent.name.clone(), run_id.to_string(), session.clone(), item.clone(), routine.map(|r| r.id));
    let Ok((card, joined)) =
        tokio::task::spawn_blocking(move || hold(&c, &name, &run, s.as_deref(), &it, r.as_deref(), until)).await
    else {
        return;
    };
    // placed in the turn's conversation first (an answer from then on resumes the agent there)
    let card_run = format!("{RUN_PREFIX}{}", card.id);
    crate::card_place::place_for(state, agent, &card_run, Some(run_id)).await;
    // the turn told, then Hermes answered: it ends now, the card waits
    if let Err(err) = state.hermes.steer_run(agent, run_id, HELD_NOTE).await {
        tracing::info!(target: "bipbridge::held_approvals", agent = %agent.name, error = %err, "held note not steered");
    }
    let answered = state.hermes.answer_prompt(agent, run_id, "approval", "deny", request_id.as_deref()).await;
    tracing::info!(target: "bipbridge::held_approvals", agent = %agent.name, run_id, card = %card.id,
        status = ?answered.as_ref().map(|r| r.0).ok(), "Hermes told: the card waits for the person");
    state.hub.forget_approval(&agent.name, run_id, request_id.as_deref());
    if !joined {
        state.push.notify_bridge_card(&agent.name, &card.summary, "mail_request", &card_run).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &tempfile::TempDir) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config
    }

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn a_held_approval_waits_joins_the_same_and_is_granted_once() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let ask = data(json!({"command": "MCP tool 'move_page' on UNTRUSTED server 'notion'", "bip_summary": "Vie veut ranger"}));
        let now = py::time_time();
        let (card, joined) = hold(&config, "vie", "run_1", Some("s_rout"), &ask, Some("r1"), now + 3600.0);
        assert!(!joined && card.summary == "Vie veut ranger" && card.kind == "service_write");
        // the next night's same request: the same card, where it was asked last
        let (again, joined) = hold(&config, "vie", "run_2", Some("s_rout2"), &ask, Some("r1"), now + 7200.0);
        assert!(joined && again.id == card.id && again.run == "run_2");
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], format!("{RUN_PREFIX}{}", card.id));
        assert_eq!(items[0]["choices"], json!(["once", "deny"]));
        assert!(pending_items(&config, Some("coach")).is_empty());
        // nothing granted before the « Oui »
        assert!(take_grant(&config, "vie", &card.command).is_none());
        assert!(answer(&config, "coach", &card.id, true).is_none(), "another agent's card");
        let done = answer(&config, "vie", &card.id, true).unwrap();
        assert_eq!(done.answered.as_ref().map(|a| a.0.as_str()), Some("once"));
        assert!(pending_items(&config, None).is_empty());
        assert!(answer(&config, "vie", &card.id, true).is_none(), "answered once");
        // the exact command, once, for that agent only
        assert!(take_grant(&config, "coach", &card.command).is_none());
        assert!(take_grant(&config, "vie", "MCP tool 'delete_page' on UNTRUSTED server 'notion'").is_none());
        assert!(take_grant(&config, "vie", &card.command).is_some());
        assert!(take_grant(&config, "vie", &card.command).is_none(), "used once");
        let words = resume_words(&config, &format!("{RUN_PREFIX}{}", card.id), "", true).unwrap();
        assert!(words.contains("Vie veut ranger") && words.contains("move_page"), "{words}");
        assert!(resume_words(&config, "bip-mail-x", "", true).is_none());
    }

    #[test]
    fn a_refused_or_expired_card_grants_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let ask = data(json!({"command": "rm -rf /tmp/x", "pattern_key": "recursive delete"}));
        let now = py::time_time();
        let (card, _) = hold(&config, "vie", "run_1", None, &ask, None, now + 3600.0);
        answer(&config, "vie", &card.id, false).unwrap();
        assert!(take_grant(&config, "vie", "rm -rf /tmp/x").is_none());
        // past its time (its routine ran again): gone, not answerable
        let (late, _) = hold(&config, "vie", "run_2", None, &data(json!({"command": "ls"})), None, now + 3600.0);
        change(&config, |b| b.held.iter_mut().for_each(|h| h.until = now - 1.0));
        assert!(pending_items(&config, None).is_empty());
        assert!(answer(&config, "vie", &late.id, true).is_none());
    }
}
