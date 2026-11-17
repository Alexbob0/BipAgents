//! A card « Oui / Non » the bridge asks the person while an agent's call waits (security audit of 17/11), for what
//! must never happen on the agent's word alone:
//!
//! - **a form sent to a site not allowed yet** (audit 1.2, Alex « Oui, 1re fois par site »): the agent's browser
//!   refuses the first form it would send to a site (browser_mcp.rs `guard`), the agent calls `ask_to_send`
//!   (form_send.rs) and the card asks « Envoyer sur ce site ? » with « Oui » (this time) and « Toujours pour ce site »
//!   (kept in « Sites autorisés »);
//! - **an event deleted from the person's calendar** (audit 1.3, Alex: `calendar_delete` critical, as `mail_trash`):
//!   the phone's calendar or a mailbox's (CalDAV, deleted at once there, no trash).
//!
//! Each card is listed with the approvals (`GET /v1/approvals`, run id `bip-ok-<id>`, choices `once` / `deny`, its
//! kind in `bip_ok.kind`, the plain sentence in `bip_summary`, what it is about in `bip_detail`, and when « Toujours »
//! is offered its words in `bip_always`): the apps draw it as any approval, the « Toujours » button under it, and
//! answer it with `POST /v1/approve` (`once`, `always`, `deny`). It is placed in the turn's conversation and pushed.
//! No answer within `[browser] consent_seconds`: nothing done, the agent says it waits. A turn nobody follows (a
//! routine…): the card is held as the night's other approvals (held_approvals.rs) and its « Oui » later lets that very
//! call (its command and what it showed) through once.
//!
//! The kinds are critical (approval_level.rs: always asked, whatever the level). Not a Python module.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::Notify;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::{py, Dict};

/// The run id of a card (the apps show it as any card of the bridge's).
pub const RUN_PREFIX: &str = "bip-ok-";
/// The tools whose call waits for the person's answer on this card (turn_status.rs `HOLDING`).
pub const HOLDS_THE_CALL: &[&str] = &[crate::form_send::TOOL, "calendar_delete"];
/// A form sent to a site not allowed yet for this agent.
pub const SITE_FORM: &str = "site_form";
/// An event deleted from the person's calendar.
pub const CALENDAR_DELETE: &str = "calendar_delete";

/// What the card shows and what it is for.
#[derive(Debug, Clone)]
pub struct Card {
    /// `SITE_FORM` or `CALENDAR_DELETE`.
    pub kind: &'static str,
    /// The tool whose call waits (its turn found by it).
    pub tool: &'static str,
    /// What a held « Oui » grants exactly (held_approvals.rs `take_grant`).
    pub command: String,
    pub summary: String,
    pub detail: String,
    /// The « Toujours » button's words, when it is offered.
    pub always: Option<String>,
}

/// The person's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Once,
    Always,
    Deny,
    /// Nobody answered in time.
    NoAnswer,
    /// Nobody follows this turn: the card waits for the person (held_approvals.rs).
    Held,
}

struct Ask {
    /// The bridge whose card it is (its data folder: one per process, but tests run several in one).
    place: String,
    agent: String,
    card: Card,
    at: f64,
    answer: Option<Answer>,
    notify: Arc<Notify>,
}

static ASKS: LazyLock<Mutex<HashMap<String, Ask>>> = LazyLock::new(Default::default);

fn asks() -> std::sync::MutexGuard<'static, HashMap<String, Ask>> {
    ASKS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The kind of a card's item (`bip_ok.kind`), as approval_level.rs names it; an unknown one: `SITE_FORM` (critical).
pub fn kind_of(item: &Dict) -> &'static str {
    match item.get("bip_ok").and_then(|o| o.get("kind")).and_then(Value::as_str) {
        Some(CALENDAR_DELETE) => CALENDAR_DELETE,
        _ => SITE_FORM,
    }
}

fn data_of(card: &Card) -> Dict {
    json!({"command": card.command, "description": card.summary, "bip_summary": card.summary,
           "bip_detail": card.detail})
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let place = config.data_dir();
    let mut items: Vec<(f64, Dict)> = asks()
        .iter()
        .filter(|(_, a)| a.place == place && a.answer.is_none() && agent.is_none_or(|n| n == a.agent))
        .filter_map(|(id, a)| {
            let mut item = json!({
                "agent": a.agent, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id,
                "command": a.card.command, "description": a.card.summary, "bip_summary": a.card.summary,
                "bip_detail": a.card.detail, "choices": ["once", "deny"], "created_at": a.at, "session_id": null,
                "bip_ok": {"kind": a.card.kind},
            });
            if let Some(always) = &a.card.always {
                item["bip_always"] = json!(always);
                item["bip_always_kind"] = json!(a.card.kind);
            }
            item.as_object().cloned().map(|d| (a.at, d))
        })
        .collect();
    items.sort_by(|a, b| a.0.total_cmp(&b.0));
    items.into_iter().map(|(_, d)| d).collect()
}

/// The answer a choice word means here: `always` only where the card offers it; anything not a yes is a no.
fn answer_of(choice: &str, offers_always: bool) -> Answer {
    let word = choice.trim().to_lowercase();
    if word == "always" && offers_always {
        Answer::Always
    } else if crate::card_place::allowed(&word) {
        Answer::Once
    } else {
        Answer::Deny
    }
}

/// A card answered (`POST /v1/approve`): the choice it was taken as, None when no such card waits.
pub fn answer(config: &Config, agent: &str, id: &str, choice: &str, via: &str) -> Option<&'static str> {
    let (card, taken) = {
        let mut table = asks();
        let place = config.data_dir();
        let ask = table.get_mut(id).filter(|a| a.place == place && a.agent == agent && a.answer.is_none())?;
        let taken = answer_of(choice, ask.card.always.is_some());
        ask.answer = Some(taken);
        ask.notify.notify_waiters();
        (ask.card.clone(), taken)
    };
    let word = match taken {
        Answer::Always => "always",
        Answer::Once => "once",
        _ => "deny",
    };
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id, "command": card.command,
        "description": card.summary, "topic": card.kind, "choice": word, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(agent, error = %exc, "card decision not kept");
        }
    }
    tracing::info!(agent, kind = card.kind, choice = word, "person's card answered");
    Some(word)
}

/// Takes the card away when the call ends, whatever way.
struct CardGone(String);

impl Drop for CardGone {
    fn drop(&mut self) {
        asks().remove(&self.0);
    }
}

/// The person's answer to `card`, for `agent`'s call (see the module's comment).
pub async fn ask(state: &AppState, agent: &AgentConfig, card: Card) -> Answer {
    let config = state.config();
    let data = data_of(&card);
    let sha = crate::held_approvals::args_sha(&data);
    let granted = {
        let (c, name, command) = (config.clone(), agent.name.clone(), card.command.clone());
        tokio::task::spawn_blocking(move || crate::held_approvals::take_grant(&c, &name, &command, &sha))
            .await
            .ok()
            .flatten()
    };
    if granted.is_some() {
        return Answer::Once; // the « Oui » the person gave to the card held for this very call
    }
    let turn = state.hub.turn_of(&agent.name, card.tool).map(|t| t.run_id);
    let unattended = turn.as_deref().and_then(|run| state.hub.nobody_there_for(&agent.name, run).map(|s| (run, s)));
    if let Some((run, session)) = unattended {
        hold(state, agent, run, session, data).await;
        return Answer::Held;
    }
    let id = py::uuid4_hex()[..12].to_string();
    let notify = Arc::new(Notify::new());
    let line = card.summary.clone();
    asks().insert(
        id.clone(),
        Ask { place: config.data_dir(), agent: agent.name.clone(), card, at: py::time_time(), answer: None,
              notify: notify.clone() },
    );
    let _gone = CardGone(id.clone());
    let run = format!("{RUN_PREFIX}{id}");
    crate::card_place::place_for(state, agent, &run, turn.as_deref()).await;
    state.push.notify_bridge_card(&agent.name, &line, "mail_request", &run).await;
    let wait = Duration::from_secs(u64::from(config.browser.consent_seconds.max(1)));
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let wake = notify.notified();
        match asks().get(&id).map(|a| a.answer) {
            Some(Some(answer)) => return answer,
            Some(None) => {}
            None => return Answer::Deny,
        }
        if tokio::time::timeout_at(deadline, wake).await.is_err() {
            return Answer::NoAnswer;
        }
    }
}

/// The card held for a turn nobody follows (held_approvals.rs), placed in that turn's conversation and pushed once.
async fn hold(state: &AppState, agent: &AgentConfig, run: &str, session: Option<String>, data: Dict) {
    let config = state.config();
    let routine = match session.as_deref() {
        Some(s) => crate::routines::of_session(state, agent, s).await,
        None => None,
    };
    let now = py::time_time();
    let until = routine
        .as_ref()
        .and_then(|r| r.next_run)
        .filter(|next| *next > now)
        .map_or(now + crate::held_approvals::KEEP_SECONDS, |next| next.min(now + crate::held_approvals::KEEP_SECONDS));
    let (c, name, r, s, rid) = (config, agent.name.clone(), run.to_string(), session, routine.map(|r| r.id));
    let Ok((card, joined)) = tokio::task::spawn_blocking(move || {
        crate::held_approvals::hold(&c, &name, &r, s.as_deref(), &data, rid.as_deref(), until)
    })
    .await
    else {
        return;
    };
    let card_run = format!("{}{}", crate::held_approvals::RUN_PREFIX, card.id);
    crate::card_place::place_for(state, agent, &card_run, Some(run)).await;
    if !joined {
        state.push.notify_bridge_card(&agent.name, &card.summary, "mail_request", &card_run).await;
    }
    tracing::info!(agent = %agent.name, card = %card.id, "card held for the person (nobody follows the turn)");
}

/// One of four sentences by language (« fr », « de », « es », else English).
pub fn pick<'a>(lang: &str, fr: &'a str, en: &'a str, de: &'a str, es: &'a str) -> &'a str {
    match lang.split(['-', '_']).next().unwrap_or("") {
        "fr" => fr,
        "de" => de,
        "es" => es,
        _ => en,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn always_only_where_the_card_offers_it_and_anything_else_is_a_no() {
        assert_eq!(answer_of("always", true), Answer::Always);
        assert_eq!(answer_of("always", false), Answer::Once);
        assert_eq!(answer_of("once", true), Answer::Once);
        assert_eq!(answer_of("deny", true), Answer::Deny);
        assert_eq!(answer_of("whatever", true), Answer::Deny);
        let item = |kind: &str| json!({"bip_ok": {"kind": kind}}).as_object().cloned().unwrap();
        assert_eq!(kind_of(&item("calendar_delete")), CALENDAR_DELETE);
        assert_eq!(kind_of(&item("x")), SITE_FORM);
    }

    #[test]
    fn a_waiting_card_is_listed_with_its_always_and_answered_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let card = Card { kind: SITE_FORM, tool: crate::form_send::TOOL, command: "ask_to_send shop.example".into(),
                          summary: "Envoyer sur ce site ? Vie veut envoyer un formulaire à shop.example".into(),
                          detail: "message".into(), always: Some("Toujours pour ce site".into()) };
        asks().insert("t1abc".into(), Ask { place: config.data_dir(), agent: "vie".into(), card, at: 1.0,
                                            answer: None, notify: Arc::new(Notify::new()) });
        let mut elsewhere = Config::with_key("k");
        elsewhere.outbox.db_path = dir.path().join("other").join("bridge.db").to_string_lossy().into_owned();
        assert!(pending_items(&elsewhere, None).iter().all(|i| i["request_id"] != "t1abc"), "another bridge's");
        assert_eq!(answer(&elsewhere, "vie", "t1abc", "once", "test"), None);
        let items = pending_items(&config, Some("vie"));
        let item = items.iter().find(|i| i["request_id"] == "t1abc").unwrap();
        assert_eq!(item["run_id"], "bip-ok-t1abc");
        assert_eq!(item["choices"], json!(["once", "deny"]));
        assert_eq!(item["bip_always"], "Toujours pour ce site");
        assert_eq!(crate::approval_level::classify_card(item), SITE_FORM);
        assert!(crate::approval_level::rule_of(SITE_FORM).critical);
        assert!(pending_items(&config, Some("coach")).iter().all(|i| i["request_id"] != "t1abc"));
        assert_eq!(answer(&config, "coach", "t1abc", "always", "test"), None, "another agent's card");
        assert_eq!(answer(&config, "vie", "t1abc", "always", "test"), Some("always"));
        assert_eq!(answer(&config, "vie", "t1abc", "once", "test"), None, "answered once");
        asks().remove("t1abc");
    }
}
