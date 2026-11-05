//! Where a card shows: the bridge's own cards (`bip-mail-`, `bip-mailrule-`, `bip-trig-`, a connector's, a site's…)
//! in the conversation whose turn made them, like Hermes' approvals, and in the Boîte; answered in either place, gone
//! from both on every device (Alex, 2026-11-04: the mail cards were only in the Boîte, never where Vie asked).
//!
//! - A card made by a tool call is placed when the call ends (agent_tools.rs), or while it runs (`watch`): the turn's
//!   conversation (`session`) and
//!   the conversation that one reports to (`report`): itself for a person's conversation, the conversation a
//!   background task was asked from (jobs.rs `from_session`), none (the main one) for a task asked from nowhere, a
//!   routine's run or another hidden conversation. A card made with no turn (a routine run by Hermes' cron) has
//!   neither: the main conversation shows it. A card never placed shows in no conversation.
//! - Hermes' own approvals of a background task follow the same rule (`report_of`): in the task's conversation, and in
//!   the one it reports to.
//! - Answered (app.rs `approve`): the conversations it showed in are touched (their `pending` refreshed on every
//!   device), the decision kept in the conversation it showed in, its notification read everywhere.
//!
//! - EVERY card that waits for the person shows inline (Alex, 2026-11-05: a secret card, a site, a connector were in
//!   the Boîte only, never where the agent asked): the generic ones (mail, rules, triggers) and those with their own
//!   screen (a secret, a site, a connector, an account), each drawn by the apps' own card. A card made while a tool call
//!   still runs (`ask_secret` waits for the answer; a site from the egress proxy or the sandbox's tools; a connector)
//!   is placed by `watch` within a second, with the turn going on. Not placed, so never inline: the place's own cards
//!   (space, tips: no conversation asked them) and a card older than `FRESH_SECONDS` when first seen (one made before
//!   this, Alex's 28/10 card drawn at the end of the Discussion); nor a placed card waiting for more than `STALE_DAYS`.
//! - A card answered or gone through its own route (the secret's secure field, the site's screen…): `watch` sees it go
//!   and does what `answered` does (its conversations touched, its notifications read on every device).
//!
//! Kept in `<data_dir>/card-places.json` (`{run_id: {agent, session, report, at}}`), the last `KEEP_DAYS`.
//!
//! No Python counterpart.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::{py, Dict};

const KEEP_DAYS: f64 = 30.0;
const FILE: &str = "card-places.json";
/// A card first seen unplaced later than this after it was made: never placed (made before placements, or while the
/// bridge was off), the Boîte only.
const FRESH_SECONDS: f64 = 120.0;
/// A card waiting longer than this: the Boîte only, no more in a conversation.
const STALE_DAYS: f64 = 3.0;
/// The cards of the place itself, asked by no conversation: the Boîte only.
const NOT_IN_CONVERSATIONS: [&str; 2] = [crate::storage_watch::RUN_PREFIX, crate::feature_tips::RUN_PREFIX];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub agent: String,
    /// The conversation (Hermes session) whose turn made the card.
    pub session: Option<String>,
    /// The conversation it reports to (None: the main one).
    pub report: Option<String>,
    #[serde(default)]
    pub at: f64,
    /// The turn (run) that asked it; None: no turn, or placed before 5/11 afternoon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// What it asks, in the person's words (the card's sentence): told to the agent when it resumes.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
}

impl Place {
    /// Shown in a conversation made of `sessions` (`main`: the agent's main conversation).
    pub fn shows_in(&self, sessions: &HashSet<&str>, main: bool) -> bool {
        let within = |s: &Option<String>| s.as_deref().is_some_and(|s| sessions.contains(s));
        within(&self.session) || within(&self.report) || (self.report.is_none() && main)
    }
}

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join(FILE)
}

fn load(config: &Config) -> BTreeMap<String, Place> {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The card `run_id`'s place (None: never placed, the main conversation shows it).
pub fn of(config: &Config, run_id: &str) -> Option<Place> {
    load(config).remove(run_id)
}

fn keep(config: &Config, run_id: &str, place: Place) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(config);
    let now = py::time_time();
    all.retain(|_, p| now - p.at < KEEP_DAYS * 86400.0);
    all.insert(run_id.to_string(), Place { at: now, ..place });
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(&all).map_err(std::io::Error::other)?;
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

/// Every card of the bridge's own waiting for `agent` (None: every agent), in Hermes' approvals' shape.
pub fn bridge_cards(state: &AppState, config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut items = Vec::new();
    items.extend(crate::sandbox_sites::pending_items(config, agent));
    items.extend(crate::vault_logins::pending_items(config, agent));
    items.extend(crate::secrets::pending_items(config, agent));
    items.extend(crate::connectors::pending_items(state, config, agent));
    items.extend(crate::storage_watch::pending_items(config, agent));
    items.extend(crate::mail_tidy::pending_items(config, agent));
    items.extend(crate::mail_rules::pending_items(config, agent));
    items.extend(crate::feature_tips::pending_items(config, agent));
    items.extend(crate::triggers::pending_items(config, agent));
    items
}

fn run_id_of(item: &Dict) -> String {
    item.get("run_id").and_then(Value::as_str).unwrap_or("").to_string()
}

// -- one card for requests of the same kind -------------------------------------------------------------------------
//
// Alex, 2026-11-04: « réunir ce genre de demandes en masse en une seule ». Each kind says whether its cards fold and
// on what (`fold_key`): the same agent's cards with the same key are ONE card (the oldest leads: its run id answers
// all of them, `members`), its sentence the lead's with how many more, « Voir la liste » every one of them. A payment,
// a secret, an account, a site, a connector, a space or a tip never folds (each is its own decision, with its own
// screen); a mail tidy card also joins the waiting one in its own store (mail_tidy.rs `same_decision`).

/// What a card folds on (None: never folds).
fn fold_key(item: &Dict) -> Option<String> {
    let run = run_id_of(item);
    let agent = item.get("agent").and_then(Value::as_str).unwrap_or("");
    let field = |obj: &str, key: &str| {
        item.get(obj).and_then(|o| o.get(key)).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
    };
    if run.starts_with(crate::mail_tidy::RUN_PREFIX) {
        // the same action on the same folder of the same mailbox
        let (action, mailbox, folder) =
            (field("bip_mail", "action")?, field("bip_mail", "mailbox")?, field("bip_mail", "folder")?);
        return Some(format!("{agent}\u{1f}mail\u{1f}{action}\u{1f}{mailbox}\u{1f}{folder}"));
    }
    if run.starts_with(crate::mail_rules::RUN_PREFIX) {
        // standing rules doing the same thing in the same mailbox (several senders to « Factures »)
        let (action, mailbox) = (field("bip_mail_rule", "action")?, field("bip_mail_rule", "mailbox")?);
        return Some(format!("{agent}\u{1f}mailrule\u{1f}{action}\u{1f}{mailbox}"));
    }
    None
}

fn summary_of(item: &Dict) -> String {
    item.get("bip_summary").or_else(|| item.get("description")).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The cards with the same fold key as one, in the order of their leads (the oldest of each).
pub fn fold(config: &Config, items: Vec<Dict>) -> Vec<Dict> {
    let mut out: Vec<(Option<String>, Vec<Dict>)> = Vec::new();
    for item in items {
        let key = fold_key(&item);
        match key.as_ref().and_then(|k| out.iter_mut().find(|(o, _)| o.as_ref() == Some(k))) {
            Some((_, group)) => group.push(item),
            None => out.push((key, vec![item])),
        }
    }
    out.into_iter().map(|(_, group)| merged(config, group)).collect()
}

fn merged(config: &Config, mut group: Vec<Dict>) -> Dict {
    if group.len() == 1 {
        return group.remove(0);
    }
    let at = |d: &Dict| d.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
    group.sort_by(|a, b| at(a).total_cmp(&at(b)));
    let mut lead = group[0].clone();
    let agent = lead.get("agent").and_then(Value::as_str).unwrap_or("").to_string();
    let lang = crate::mail_tidy::language(config, &agent);
    let more = group.len() - 1;
    let tail = match lang.as_str() {
        "fr" => format!(" (+{more} {} du même genre)", if more == 1 { "demande" } else { "demandes" }),
        "de" => format!(" (+{more} {} dieser Art)", if more == 1 { "Anfrage" } else { "Anfragen" }),
        "es" => format!(" (+{more} {} del mismo tipo)", if more == 1 { "solicitud" } else { "solicitudes" }),
        _ => format!(" (+{more} more of the same kind)"),
    };
    let summary = format!("{}{tail}", summary_of(&lead));
    let mut detail: Vec<String> = group.iter().map(|i| format!("• {}", summary_of(i))).collect();
    for item in &group {
        if let Some(d) = item.get("bip_detail").and_then(Value::as_str).filter(|d| !d.is_empty()) {
            detail.push(String::new());
            detail.push(d.to_string());
        }
    }
    lead.insert("bip_summary".into(), json!(summary));
    lead.insert("description".into(), json!(summary));
    lead.insert("bip_detail".into(), json!(detail.join("\n")));
    lead.insert("bip_folded".into(), json!(group.iter().map(run_id_of).collect::<Vec<_>>()));
    lead
}

/// The requests a card's « Oui » / « Non » answers: those it folds (its lead's run id), else itself.
pub fn members(state: &AppState, agent: &str, run_id: &str) -> Vec<String> {
    let config = state.config();
    let cards = bridge_cards(state, &config, Some(agent));
    let Some(item) = cards.iter().find(|i| run_id_of(i) == run_id) else { return vec![run_id.to_string()] };
    let Some(key) = fold_key(item) else { return vec![run_id.to_string()] };
    let mut group: Vec<&Dict> = cards.iter().filter(|i| fold_key(i).as_ref() == Some(&key)).collect();
    let at = |d: &Dict| d.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
    group.sort_by(|a, b| at(a).total_cmp(&at(b)));
    if group.first().map(|lead| run_id_of(lead)) != Some(run_id.to_string()) {
        return vec![run_id.to_string()]; // a member answered by its own id (an older app, a notification): itself
    }
    group.into_iter().map(run_id_of).collect()
}

/// The run ids of the agent's cards now (before a tool call: the new ones after it are its).
pub fn waiting(state: &AppState, agent: &str) -> HashSet<String> {
    bridge_cards(state, &state.config(), Some(agent)).iter().map(run_id_of).collect()
}

/// The conversation `session` reports to: itself for a person's conversation, a background task's asker (None: the
/// main one), None for a routine's run or another hidden conversation.
pub async fn report_of(state: &AppState, agent: &AgentConfig, session: &str) -> Option<String> {
    let (config, name, wanted) = (state.config(), agent.name.clone(), session.to_string());
    let task = tokio::task::spawn_blocking(move || {
        crate::jobs::tasks(&config).into_iter().find(|j| {
            j.get("agent").and_then(Value::as_str) == Some(name.as_str())
                && j.get("session_id").and_then(Value::as_str) == Some(wanted.as_str())
        })
    })
    .await
    .ok()
    .flatten();
    if let Some(task) = task {
        return task.get("from_session").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    }
    let threads = crate::threads::threads_of(state);
    match threads.thread_of(agent, session).await {
        Some(thread) if thread.hidden => None,
        _ => Some(session.to_string()),
    }
}

/// After a tool call of `agent` in the turn `run_id`: the cards it made (not in `before`) placed in that turn's
/// conversation.
pub async fn place_new(state: &AppState, agent: &AgentConfig, before: &HashSet<String>, run_id: Option<&str>) {
    let placed = load(&state.config());
    let made: Vec<String> = waiting(state, &agent.name)
        .into_iter()
        .filter(|r| !before.contains(r) && !placed.contains_key(r) && placeable(r))
        .collect();
    if made.is_empty() {
        return;
    }
    place(state, agent, &made, run_id).await;
}

/// A card of this kind can show in a conversation.
fn placeable(run_id: &str) -> bool {
    !NOT_IN_CONVERSATIONS.iter().any(|p| run_id.starts_with(p))
}

/// `made` placed in the conversation of the turn `run_id` (None: no turn, the main conversation), those showing it
/// touched.
async fn place(state: &AppState, agent: &AgentConfig, made: &[String], run_id: Option<&str>) {
    let session = run_id
        .and_then(|r| state.hub.subscription(&agent.name, r))
        .and_then(|s| s.session_id())
        .filter(|s| !s.is_empty());
    let report = match session.as_deref() {
        Some(s) => report_of(state, agent, s).await,
        None => None,
    };
    let place = Place {
        agent: agent.name.clone(),
        session: session.clone(),
        report: report.clone(),
        at: 0.0,
        run: run_id.map(str::to_string),
        summary: String::new(),
    };
    let config = state.config();
    let cards = bridge_cards(state, &config, Some(&agent.name));
    for run_id in made {
        let summary = cards.iter().find(|i| run_id_of(i) == *run_id).map(summary_of).unwrap_or_default();
        if let Err(err) = keep(&config, run_id, Place { summary, ..place.clone() }) {
            tracing::warn!(agent = %agent.name, error = %err, "card place not kept");
        }
    }
    let threads = crate::threads::threads_of(state);
    // the conversations showing it now: their `pending` on every device
    threads.touch(&agent.name, report.as_deref(), "asked").await;
    if session.is_some() && session != report {
        threads.touch(&agent.name, session.as_deref(), "asked").await;
    }
}

fn created_at(item: &Dict) -> f64 {
    item.get("created_at").and_then(Value::as_f64).unwrap_or(0.0)
}

/// The bridge's cards shown in a conversation of `agent` made of `sessions`: every kind but the place's own, placed
/// there (`watch`, `place_new`), waiting for less than `STALE_DAYS`. Each keeps its `created_at`: the apps draw it at
/// its time.
pub fn cards_in(state: &AppState, agent: &str, sessions: &HashSet<&str>, main: bool) -> Vec<Dict> {
    let config = state.config();
    let places = load(&config);
    let now = py::time_time();
    let shown = bridge_cards(state, &config, Some(agent))
        .into_iter()
        .filter(|item| now - created_at(item) < STALE_DAYS * 86400.0)
        .filter(|item| places.get(&run_id_of(item)).is_some_and(|place| place.shows_in(sessions, main)))
        .collect();
    fold(&config, shown)
}

/// A bridge card listed in the Boîte: the conversation it shows in, for the row to open.
pub fn with_session(config: &Config, mut item: Dict) -> Dict {
    if let Some(place) = of(config, &run_id_of(&item)) {
        item.insert("session_id".into(), json!(place.report.or(place.session)));
    }
    item
}

/// A card answered (any device, the Boîte or the conversation): the conversations showing it touched, the decision
/// kept in the one it showed in, its notifications read on every device.
pub async fn answered(state: &AppState, agent: &AgentConfig, run_id: &str, choice: &str) {
    note_answered(run_id);
    let config = state.config();
    // the card's id: its request id in the approvals' shape, its notification's `ref`
    let id = run_id.strip_prefix("bip-").and_then(|r| r.split_once('-')).map(|(_, id)| id).unwrap_or(run_id);
    let place = of(&config, run_id).unwrap_or_default();
    let threads = crate::threads::threads_of(state);
    // the decision as an item of the conversation it showed in (approvals_log.rs merges it with what was kept)
    let shown = match place.report.clone().or_else(|| place.session.clone()) {
        Some(s) => Some(s),
        None => match threads.main(agent).await {
            Ok(main) => threads.line(agent, &main).await.ok().and_then(|l| l.last().map(|s| s.session_id.clone())),
            Err(_) => None,
        },
    };
    if let Some(session) = shown.as_deref() {
        let mut fields = serde_json::Map::new();
        fields.insert("run_id".into(), json!(run_id));
        fields.insert("request_id".into(), json!(id));
        fields.insert("choice".into(), json!(choice));
        fields.insert("session_id".into(), json!(session));
        if let Err(err) = crate::approvals_log::record(&config, &agent.name, &fields) {
            tracing::debug!(agent = %agent.name, error = %err, "card decision not placed");
        }
    }
    threads.touch(&agent.name, place.report.as_deref(), "answered").await;
    if place.session.is_some() && place.session != place.report {
        threads.touch(&agent.name, place.session.as_deref(), "answered").await;
    }
    crate::notifications::read_card(state, &agent.name, id, run_id).await;
    if !place.agent.is_empty() {
        resume(state, agent, &place, choice);
    }
}

/// The cards `answered` did, the last 200 (`watch` leaves them).
static ANSWERED: Mutex<std::collections::VecDeque<String>> = Mutex::new(std::collections::VecDeque::new());

fn note_answered(run_id: &str) {
    let mut done = ANSWERED.lock().unwrap_or_else(|e| e.into_inner());
    if done.len() >= 200 {
        done.pop_front();
    }
    done.push_back(run_id.to_string());
}

/// Answered through `/v1/approve` (`answered` did it all): forgotten here.
fn was_answered(run_id: &str) -> bool {
    let mut done = ANSWERED.lock().unwrap_or_else(|e| e.into_inner());
    let at = done.iter().position(|r| r == run_id);
    at.and_then(|i| done.remove(i)).is_some()
}

/// The agent's tool that makes a card of this kind while it runs (`turn_of` prefers the turn calling it).
fn asking_tool(run_id: &str) -> &'static str {
    if run_id.starts_with(crate::secrets::RUN_PREFIX) {
        crate::connectors::ASK_SECRET
    } else if run_id.starts_with(crate::connectors::RUN_PREFIX) {
        crate::connectors::REGISTER
    } else if run_id.starts_with(crate::sandbox_sites::RUN_PREFIX) {
        crate::sandbox_sites::REQUEST
    } else {
        ""
    }
}

const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Every second: a card made while a tool call still runs (or with no turn) placed in its turn's conversation, and a
/// card answered elsewhere than `/v1/approve` (or withdrawn) gone from the conversations that showed it.
pub async fn watch(state: AppState) {
    let mut before: Option<HashSet<String>> = None;
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        let (s, config) = (state.clone(), state.config());
        let Ok(cards) = tokio::task::spawn_blocking(move || bridge_cards(&s, &config, None)).await else { continue };
        let config = state.config();
        let places = load(&config);
        let now = py::time_time();
        let ids: HashSet<String> = cards.iter().map(run_id_of).collect();
        // new and unplaced: its turn's conversation (one of each agent's cards at a time: they share the turn)
        let mut made: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for item in &cards {
            let run = run_id_of(item);
            // a card asked again after it was placed (its time moved on: sandbox_sites.rs `ask`) is placed again
            let placed = places.get(&run).is_some_and(|p| created_at(item) <= p.at + 1.0);
            if placed || !placeable(&run) || now - created_at(item) > FRESH_SECONDS {
                continue;
            }
            let agent = item.get("agent").and_then(Value::as_str).unwrap_or("").to_string();
            made.entry(agent).or_default().push(run);
        }
        for (name, runs) in made {
            // by the agent's id (its profile, `vie`, `test-agent`), never its shown name: a renamed agent's cards too
            let Some(agent) = config.agent(Some(&name)).cloned() else { continue };
            // the turn whose last call is the tool that makes such a card, else the one that called a tool last
            let tool = runs.first().map(|r| asking_tool(r)).unwrap_or("");
            let turn = state.hub.turn_of(&agent.name, tool).map(|t| t.run_id);
            place(&state, &agent, &runs, turn.as_deref()).await;
        }
        // gone without `/v1/approve`: what `answered` does, but the decision (its own route kept it)
        if let Some(before) = before.take() {
            for run in before.difference(&ids) {
                if was_answered(run) {
                    continue;
                }
                let Some(place) = places.get(run) else { continue };
                let Some(agent) = config.agent(Some(&place.agent)).cloned() else { continue };
                gone(&state, &agent, run, place).await;
            }
        }
        let placed = load(&config);
        before = Some(ids.into_iter().filter(|r| placed.contains_key(r)).collect());
    }
}

/// A placed card no longer waiting: the conversations showing it touched, its notifications read on every device.
async fn gone(state: &AppState, agent: &AgentConfig, run_id: &str, place: &Place) {
    let threads = crate::threads::threads_of(state);
    threads.touch(&agent.name, place.report.as_deref(), "answered").await;
    if place.session.is_some() && place.session != place.report {
        threads.touch(&agent.name, place.session.as_deref(), "answered").await;
    }
    let id = run_id.strip_prefix("bip-").and_then(|r| r.split_once('-')).map(|(_, id)| id).unwrap_or(run_id);
    crate::notifications::read_card(state, &agent.name, id, run_id).await;
    // answered through its own route (the secure field, the sites' or the connector's screen): the decision it kept;
    // none (withdrawn, expired): nobody answered, nothing to resume
    let config = state.config();
    let decided = crate::approvals_log::list(&config, &agent.name, None)
        .into_iter()
        .rev()
        .find(|e| e.get("run_id").and_then(Value::as_str) == Some(run_id))
        .and_then(|e| e.get("choice").and_then(Value::as_str).map(str::to_string));
    if let Some(choice) = decided {
        resume(state, agent, place, &choice);
    }
}

// -- a card answered late: the agent goes on -------------------------------------------------------------------------
//
// Alex, 2026-11-05: « Valider une carte bien longtemps après devrait quand même relancer l'agent. Là il s'arrête et
// attend qu'on lui dise d'y aller. » A card answered once the turn that asked it is over (its tool call stopped
// waiting: `ask_secret`, `connector_register` after 330 s; a site's or a mail card's call never waits; the bridge
// restarted; a card with no turn) starts ONE turn of the agent in the conversation that asked (a background task: its
// own), saying what was decided. Answered while that turn still goes on: nothing more (the waiting call gets it). The
// cards answered within `RESUME_GATHER` in the same conversation (a folded card's members, several at once) make one
// turn. The turn goes through background_turn.rs: the month's credit, the brake, `RESUME_BUDGET`, a busy conversation
// waited for; its reply lands like a hand-back's (pushed unless on screen); « [SILENT] » lands nothing.

/// Answers gathered into one turn.
const RESUME_GATHER: std::time::Duration = std::time::Duration::from_secs(2);
/// Tokens a resumed turn may take.
pub const RESUME_BUDGET: i64 = 300_000;

/// What each answer said: (what the card asked, allowed), by (agent, conversation), until its turn starts.
type Decisions = Vec<(String, bool)>;
static RESUMES: Mutex<BTreeMap<(String, String), Decisions>> = Mutex::new(BTreeMap::new());

/// The turn that asked the card no longer goes on (none, ended, or unknown to this bridge: restarted since).
fn turn_over(state: &AppState, place: &Place) -> bool {
    match place.run.as_deref() {
        None => true,
        Some(run) => state.hub.subscription(&place.agent, run).is_none_or(|s| s.finished() || s.terminal().is_some()),
    }
}

/// Where the agent goes on: the conversation whose turn asked (a task's own), else the one it showed in, else main.
pub fn resume_in(place: &Place) -> String {
    place.session.clone().or_else(|| place.report.clone()).unwrap_or_else(|| "main".to_string())
}

/// Allowed (`once`, `always`…) or refused (`deny`).
pub fn allowed(choice: &str) -> bool {
    !matches!(choice, "deny" | "denied" | "refuse" | "refused" | "no")
}

/// What the resumed turn reads (a hidden note: not in the conversation's timeline).
pub fn resume_note(decisions: &[(String, bool)]) -> String {
    let lines: Vec<String> = decisions
        .iter()
        .map(|(what, ok)| {
            let what = if what.is_empty() { "your request".to_string() } else { format!("« {what} »") };
            format!("- {what}: {}", if *ok { "ALLOWED" } else { "REFUSED" })
        })
        .collect();
    format!(
        "<!--bip:card\nNot from the person: they answered in the app after your turn had ended:\n{}\nGo on with what \
         you were doing from where it stopped, now that you have the answer (don't ask for it again). Refused: don't \
         do it and don't ask again; say in one short sentence what that changes. If nothing is left to do or say, {}.\n-->",
        lines.join("\n"),
        crate::background_turn::SILENT_RULE
    )
}

/// The card `place` answered with `choice`: the agent resumes in its conversation when the turn that asked is over.
fn resume(state: &AppState, agent: &AgentConfig, place: &Place, choice: &str) {
    if !turn_over(state, place) {
        return;
    }
    let key = (agent.name.clone(), resume_in(place));
    let first = {
        let mut waiting = RESUMES.lock().unwrap_or_else(|e| e.into_inner());
        let list = waiting.entry(key.clone()).or_default();
        list.push((place.summary.clone(), allowed(choice)));
        list.len() == 1
    };
    if !first {
        return;
    }
    let (state, agent) = (state.clone(), agent.clone());
    tokio::spawn(async move {
        tokio::time::sleep(RESUME_GATHER).await;
        let decisions = RESUMES.lock().unwrap_or_else(|e| e.into_inner()).remove(&key).unwrap_or_default();
        if !decisions.is_empty() {
            resume_turn(&state, &agent, &key.1, &decisions).await;
        }
    });
}

async fn resume_turn(state: &AppState, agent: &AgentConfig, conversation: &str, decisions: &[(String, bool)]) {
    use crate::background_turn::{Delivery, Kind, Pipeline, Turn};
    let pipeline = Pipeline::of(state);
    let mut turn = Turn::new(agent, Kind::Card, conversation, resume_note(decisions));
    turn.budget = Some(RESUME_BUDGET);
    tracing::info!(agent = %agent.name, conversation, cards = decisions.len(), "a card answered late: the agent resumes");
    match pipeline.run(&turn).await {
        Ok(ran) if ran.answered() && !crate::background_turn::is_silent(&ran.text) => {
            let config = state.config();
            let title = config.agent(Some(&agent.name)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty());
            let delivery = Delivery {
                agent: agent.clone(),
                session: ran.session.clone(),
                key: format!("handback:card-{}", ran.run_id),
                title: title.unwrap_or_else(|| agent.name.clone()),
                text: ran.text.clone(),
                as_written: true,
                run_id: Some(ran.run_id.clone()),
                notify: true,
                files: Vec::new(),
            };
            pipeline.deliver(&delivery).await;
        }
        Ok(ran) => tracing::info!(agent = %agent.name, status = %ran.status, "resumed turn: nothing to tell"),
        Err(why) => tracing::info!(agent = %agent.name, why = %why, "resumed turn not run"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_shows_where_it_was_asked_and_where_that_reports() {
        let sessions: HashSet<&str> = ["s_main"].into_iter().collect();
        let place = |s: Option<&str>, r: Option<&str>| Place {
            agent: "vie".into(),
            session: s.map(str::to_string),
            report: r.map(str::to_string),
            ..Place::default()
        };
        // asked in the main conversation
        assert!(place(Some("s_main"), Some("s_main")).shows_in(&sessions, true));
        // a task asked from nowhere, a routine: the main conversation (and its own)
        assert!(place(Some("s_task"), None).shows_in(&sessions, true));
        assert!(place(Some("s_task"), None).shows_in(&["s_task"].into_iter().collect(), false));
        // a task asked from a side conversation: there, not in the main one
        assert!(!place(Some("s_task"), Some("s_side")).shows_in(&sessions, true));
        assert!(place(Some("s_task"), Some("s_side")).shows_in(&["s_side"].into_iter().collect(), false));
        // a side conversation's card is not the main one's
        assert!(!place(Some("s_side"), Some("s_side")).shows_in(&sessions, true));
    }

    #[test]
    fn a_late_answer_resumes_where_it_was_asked_saying_what_was_decided() {
        let place = |s: Option<&str>, r: Option<&str>| Place {
            agent: "vie".into(),
            session: s.map(str::to_string),
            report: r.map(str::to_string),
            ..Place::default()
        };
        // a background task in its own conversation, a person's in theirs, no turn in the main one
        assert_eq!(resume_in(&place(Some("s_task"), Some("s_side"))), "s_task");
        assert_eq!(resume_in(&place(Some("s_side"), Some("s_side"))), "s_side");
        assert_eq!(resume_in(&place(None, None)), "main");
        assert!(allowed("once") && allowed("always") && !allowed("deny"));
        let note = resume_note(&[("Vie veut envoyer des données à connect.garmin.com".into(), true), (String::new(), false)]);
        assert!(note.starts_with("<!--bip:card\n") && note.ends_with("-->"));
        assert!(note.contains("- « Vie veut envoyer des données à connect.garmin.com »: ALLOWED"));
        assert!(note.contains("- your request: REFUSED") && note.contains("[SILENT]"));
        // kept with the place, read back by an older file too
        let kept: Place = serde_json::from_str(r#"{"agent": "vie", "session": "s", "report": "s", "at": 1.0}"#).unwrap();
        assert_eq!((kept.run, kept.summary), (None, String::new()));
    }
}
