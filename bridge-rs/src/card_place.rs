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
//! - A card's notification still unread when its card waits no more (never placed, gone before `watch` followed it,
//!   a Hermes approval lost with its run): read every minute by `sweep` (notifications.rs `read_gone`), so no badge
//!   counts a card the Boîte cannot show (Alex's iPhone, 5/11).
//!
//! - Answered, any way, at any time: the agent goes on exactly once with the answer (`settle`, below).
//!
//! Kept in `<data_dir>/card-places.json` (`{run_id: {agent, session, report, at, run, summary, decided, taken}}`), the
//! last `KEEP_DAYS`.
//!
//! No Python counterpart.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

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
    /// The person's answer, not told to the agent yet (`settle` resumes it once the asking turn is over).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided: Option<Decision>,
    /// The tool call that waited for it (`ask_secret`, `connector_register`) returned the answer to its turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub taken: bool,
}

/// An answer the agent was not told yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub choice: String,
    pub at: f64,
    /// The asking turn as the hub followed it when the answer came: since when, and its tool calls so far (the calls
    /// after the answer tell whether the turn learnt it by itself).
    #[serde(default)]
    pub followed_since: f64,
    #[serde(default)]
    pub calls: u32,
    /// The bridge process that started its resumed turn (`BOOT`): another one's means the bridge restarted meanwhile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resuming: Option<String>,
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
    save(config, &all)
}

/// The card `run_id`'s place changed by `change` (kept as it is when there is none). Its result, if any.
fn update<T>(config: &Config, run_id: &str, change: impl FnOnce(&mut Place) -> T) -> Option<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(config);
    let place = all.get_mut(run_id)?;
    let out = change(place);
    if let Err(err) = save(config, &all) {
        tracing::warn!(run_id, error = %err, "card place not kept");
    }
    Some(out)
}

fn save(config: &Config, all: &BTreeMap<String, Place>) -> std::io::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(all).map_err(std::io::Error::other)?;
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
    items.extend(crate::phone_exit::store::pending_items(config, agent));
    items.extend(crate::routines::pending_items(config, agent));
    items.extend(crate::agent_hops::pending_items(config, agent));
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

/// A card just made by the agent's tool `tool` outside the agent tools' route (the files' MCP: `sandbox_sites_request`):
/// placed now, in the turn calling it, before its notification goes (the main-next battery, 6/11: a sites card
/// answered within a second, before `watch` placed it, was never told to the agent).
pub async fn place_made(state: &AppState, agent: &AgentConfig, run_id: &str, tool: &str) {
    if !placeable(run_id) {
        return;
    }
    let turn = state.hub.turn_of(&agent.name, tool).map(|t| t.run_id);
    place(state, agent, &[run_id.to_string()], turn.as_deref()).await;
}

/// A card made for another conversation than the calling turn's (agent_hops.rs: the one a chain of messages between
/// agents started from, while another agent's turn hit the cap): placed in the turn `run_id` of `agent` (None: its
/// main conversation).
pub async fn place_for(state: &AppState, agent: &AgentConfig, card: &str, run_id: Option<&str>) {
    place(state, agent, &[card.to_string()], run_id).await;
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
        ..Place::default()
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
    decide(state, run_id, &place, choice);
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
    } else if run_id.starts_with(crate::phone_exit::store::RUN_PREFIX) {
        crate::phone_exit::agent::TOOL_NAME
    } else {
        ""
    }
}

const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Every second: a card made while a tool call still runs (or with no turn) placed in its turn's conversation, and a
/// card answered elsewhere than `/v1/approve` (or withdrawn) gone from the conversations that showed it.
pub async fn watch(state: AppState) {
    let mut before: Option<HashSet<String>> = None;
    let started = py::time_time();
    let mut last_look = started;
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
        // gone without `/v1/approve`: what `answered` does, but the decision (its own route kept it). Seen waiting at
        // the last look; or never seen (placed since the last look, or this is the first look after a start: answered
        // within that second) and answered through its own route since.
        let first = before.is_none();
        let seen = before.take().unwrap_or_default();
        for (run, place) in places.iter().filter(|(run, _)| !ids.contains(*run)) {
            let unseen = !seen.contains(run);
            let recent = if first { now - place.at < STALE_DAYS * 86400.0 } else { place.at >= last_look - 1.0 };
            if unseen && !recent {
                continue;
            }
            if was_answered(run) {
                continue;
            }
            let Some(agent) = config.agent(Some(&place.agent)).cloned() else { continue };
            if unseen
                && (place.decided.is_some()
                    || place.taken
                    || decision_since(&config, &agent.name, run, started.max(place.at - 1.0)).is_none())
            {
                continue;
            }
            gone(&state, &agent, run, place).await;
        }
        last_look = now;
        let placed = load(&config);
        before = Some(ids.into_iter().filter(|r| placed.contains_key(r)).collect());
        // the answers not told yet whose turn is over: the agent goes on
        settle(&state).await;
    }
}

const SWEEP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Every minute (the first a minute after the start: the hub follows its runs again by then), on every install
/// (Hermes' own approvals too): the rows of cards gone that nothing read (a secret not answered in time, never placed,
/// gone before `watch` followed it, a Hermes approval lost with its run) read, so the badges count only what the Boîte
/// shows (Alex's iPhone, 5/11).
pub async fn sweep(state: AppState) {
    loop {
        tokio::time::sleep(SWEEP_EVERY).await;
        let (s, config) = (state.clone(), state.config());
        let Ok(cards) = tokio::task::spawn_blocking(move || bridge_cards(&s, &config, None)).await else { continue };
        crate::notifications::read_gone(&state, &waiting_refs(&state, &cards), FRESH_SECONDS as i64).await;
    }
}

/// The ids a waiting card's notification has as `ref`: the bridge's cards (their run id, their id, their request id)
/// and Hermes' approvals (run and request ids), a relative's agent's too.
fn waiting_refs(state: &AppState, cards: &[Dict]) -> HashSet<String> {
    let mut refs = HashSet::new();
    let mut hermes = state.hub.pending_approvals(None);
    hermes.extend(crate::agent_shares::pending_mine(None));
    for item in cards.iter().chain(hermes.iter()) {
        let run = run_id_of(item);
        if let Some((_, id)) = run.strip_prefix("bip-").and_then(|r| r.split_once('-')) {
            refs.insert(id.to_string());
        }
        if let Some(request) = item.get("request_id").and_then(Value::as_str) {
            refs.insert(request.to_string());
        }
        refs.insert(run);
    }
    refs
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
    if let Some(choice) = decision_since(&state.config(), &agent.name, run_id, 0.0) {
        decide(state, run_id, place, &choice);
    }
}

/// The last answer to card `run_id` its own route kept (approvals_log.rs), given at `since` or later.
fn decision_since(config: &Config, agent: &str, run_id: &str, since: f64) -> Option<String> {
    crate::approvals_log::list(config, agent, None)
        .into_iter()
        .rev()
        .find(|e| e.get("run_id").and_then(Value::as_str) == Some(run_id))
        .filter(|e| e.get("at").and_then(Value::as_f64).unwrap_or(0.0) >= since)
        .and_then(|e| e.get("choice").and_then(Value::as_str).map(str::to_string))
}

// -- a card answered: the agent goes on, once ------------------------------------------------------------------------
//
// Alex, 2026-11-05: « Valider une carte bien longtemps après devrait quand même relancer l'agent. Là il s'arrête et
// attend qu'on lui dise d'y aller. » Then, the same evening, « tout de suite » neither: a site's or a mail card's call
// never waits, so an answer given while the turn still went on (its reply streaming, other tools running) reached
// nobody, and the agent never learnt it.
//
// Every answer (`answered`, or `gone` through the card's own route) is kept with the card's place (`decided`) until
// the agent knows it:
// - a tool call waiting for it (`ask_secret`, `connector_register`) returns it to its turn and says so (`taken`):
//   nothing more;
// - else, once the turn that asked is over (ended, failed, stopped; unknown to the hub after a restart: as Hermes says
//   it), ONE turn of the agent in the conversation that asked (a background task: its own) says what was decided,
//   unless that turn learnt it by itself (it called the card's own tool again after the answer: `told_by`). A card
//   with no turn: at once.
// `settle` looks every second (`watch`), an answer being left `TAKE_GRACE` to the call waiting for it. The answers of
// the same conversation within `RESUME_GATHER` make one turn (a folded card's members, several at once). The turn goes
// through background_turn.rs: the month's credit, the brake, `RESUME_BUDGET`, a busy conversation waited for (still
// busy, or Hermes unreachable: tried again); its reply lands like a hand-back's (pushed unless on screen); « [SILENT] »
// lands nothing. Exactly once: an answer leaves its place when its turn has started (`resuming` holds it meanwhile;
// another bridge process's `resuming` is a restart before the start: resumed then).
// Hermes' own approvals are not kept here: Hermes' waiting call gets the answer, and once it stopped waiting the run
// was told it was refused and the card went (runs.rs `forget_pending`).

/// Answers gathered into one turn.
const RESUME_GATHER: std::time::Duration = std::time::Duration::from_secs(2);
/// Tokens a resumed turn may take.
pub const RESUME_BUDGET: i64 = 300_000;
/// An answer younger than this is left to the tool call waiting for it (`taken`).
const TAKE_GRACE: f64 = 1.0;
/// The same for a secret's or a connector's card asked with no turn the hub follows (a routine of Hermes' cron): its
/// call may still wait, and a connector's answer reaches it only once its tools are listed (connectors.rs `answer`).
const TAKE_GRACE_NO_TURN: f64 = 30.0;

/// Seconds an answer to `card` is left to a call that may wait for it.
fn take_grace(card: &str, place: &Place) -> f64 {
    let waits = [crate::secrets::RUN_PREFIX, crate::connectors::RUN_PREFIX].iter().any(|p| card.starts_with(p));
    if waits && place.run.is_none() {
        TAKE_GRACE_NO_TURN
    } else {
        TAKE_GRACE
    }
}
/// Seconds between two questions to Hermes about a turn the hub no longer follows.
const ASK_HERMES_EVERY: f64 = 10.0;

/// This bridge process (`Decision::resuming`).
static BOOT: LazyLock<String> = LazyLock::new(py::uuid4_hex);

/// What each answer said: (the card's run id, what it asked, allowed), by (agent, conversation), until its turn starts.
type Decisions = Vec<(String, String, bool)>;
static RESUMES: Mutex<BTreeMap<(String, String), Decisions>> = Mutex::new(BTreeMap::new());
/// When Hermes was last asked about a turn (`turn_now`).
static ASKED: Mutex<BTreeMap<String, f64>> = Mutex::new(BTreeMap::new());

/// A tool call waiting for card `run_id` returned its answer to the turn: the agent knows it.
/// A waiting card whose content changed (a site joined a sites card already on screen): the conversations showing it
/// read it again (`thread` hint), so a phone answers the card as it is (5/11, « cartes à revalider »). Never placed:
/// the main conversation shows it.
pub async fn changed(state: &AppState, agent: &str, run_id: &str) {
    let place = load(&state.config()).get(run_id).cloned();
    let threads = crate::threads::threads_of(state);
    let (session, report) = place.map(|p| (p.session, p.report)).unwrap_or_default();
    threads.touch(agent, report.as_deref(), "changed").await;
    if session.is_some() && session != report {
        threads.touch(agent, session.as_deref(), "changed").await;
    }
}

pub fn taken(config: &Config, run_id: &str) {
    update(config, run_id, |p| p.taken = true);
}

/// The card `place` answered with `choice`: kept until the agent knows it (`settle`). A card never placed was asked
/// in no conversation: nobody to tell.
fn decide(state: &AppState, run_id: &str, place: &Place, choice: &str) {
    if place.agent.is_empty() {
        return;
    }
    let followed = place.run.as_deref().and_then(|r| state.hub.subscription(&place.agent, r)).map(|sub| {
        let st = sub.state.lock().unwrap_or_else(|e| e.into_inner());
        (st.followed_since, st.tool_calls)
    });
    let (followed_since, calls) = followed.unwrap_or_default();
    let decision = Decision { choice: choice.to_string(), at: py::time_time(), followed_since, calls, resuming: None };
    let config = state.config();
    if update(&config, run_id, |p| p.decided = Some(decision.clone())).is_none() {
        // its place dropped meanwhile (`watch` read it before): kept again
        if let Err(err) = keep(&config, run_id, Place { decided: Some(decision), ..place.clone() }) {
            tracing::warn!(run_id, error = %err, "card answer not kept");
        }
    }
}

/// The answer leaves the card's place: the agent knows it, or never will.
fn forget(config: &Config, run_id: &str) {
    update(config, run_id, |p| p.decided = None);
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
        "<!--bip:card\nNot from the person: they answered in the app, and your turn ended before you were told:\n{}\nGo \
         on with what you were doing from where it stopped, now that you have the answer (don't ask for it again). \
         Refused: don't do it and don't ask again; say in one short sentence what that changes. If nothing is left to \
         do or say, {}.\n-->",
        lines.join("\n"),
        crate::background_turn::SILENT_RULE
    )
}

/// The card's own tools whose result tells its answer (the site asked again: « You may send to it already »).
fn telling_tools(card: &str) -> &'static [&'static str] {
    const SITES: [&str; 2] = [crate::sandbox_sites::REQUEST, crate::sandbox_sites::LIST];
    const SECRET: [&str; 1] = [crate::connectors::ASK_SECRET];
    const CONNECTOR: [&str; 1] = [crate::connectors::REGISTER];
    const PHONE_EXIT: [&str; 1] = [crate::phone_exit::agent::TOOL_NAME];
    if card.starts_with(crate::phone_exit::store::RUN_PREFIX) {
        return &PHONE_EXIT;
    }
    if card.starts_with(crate::sandbox_sites::RUN_PREFIX) {
        &SITES
    } else if card.starts_with(crate::secrets::RUN_PREFIX) {
        &SECRET
    } else if card.starts_with(crate::connectors::RUN_PREFIX) {
        &CONNECTOR
    } else {
        &[]
    }
}

/// The turn learnt card `card`'s answer by itself: it called the card's own tool after the answer. `recent`, `calls`
/// and `followed_since`: the turn's last tool names, its count of calls and since when the hub follows it (another
/// following, after a restart, counts again from its start: not comparable, so not learnt).
fn told_by(card: &str, recent: &VecDeque<String>, calls: u32, followed_since: f64, decision: &Decision) -> bool {
    let tools = telling_tools(card);
    if tools.is_empty() || (followed_since - decision.followed_since).abs() > 1e-3 || calls < decision.calls {
        return false;
    }
    let after = (calls - decision.calls) as usize;
    // as Hermes names them: `mcp_bipagents_files_sandbox_sites_request`, `mcp_bipagents_connectors_ask_secret`
    let is = |name: &str, tool: &str| name == tool || name.strip_suffix(tool).is_some_and(|head| head.ends_with('_'));
    recent.iter().rev().take(after).any(|t| tools.iter().any(|tool| is(crate::loop_brake::bare(t), tool)))
}

/// Where the turn that asked a card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnNow {
    /// Still going on: it may learn the answer yet.
    Going,
    /// Over, and it learnt the answer by itself.
    Told,
    /// Over without knowing it (or no turn).
    Over,
}

async fn turn_now(state: &AppState, card: &str, place: &Place, decision: &Decision) -> TurnNow {
    let Some(run) = place.run.as_deref() else { return TurnNow::Over };
    let followed = state.hub.subscription(&place.agent, run).and_then(|sub| {
        let st = sub.state.lock().unwrap_or_else(|e| e.into_inner());
        if st.terminal.is_some() {
            let told = told_by(card, &st.recent_tools, st.tool_calls, st.followed_since, decision);
            Some(if told { TurnNow::Told } else { TurnNow::Over })
        } else if !st.finished {
            Some(TurnNow::Going)
        } else {
            None // followed no more before its end (the hub's watch gave up): as Hermes says it
        }
    });
    if let Some(now) = followed {
        return now;
    }
    // not followed here (the bridge restarted since, the run long forgotten): its status in Hermes
    let Some(agent) = state.config().agent(Some(&place.agent)).cloned() else { return TurnNow::Over };
    {
        let now = py::time_time();
        let mut asked = ASKED.lock().unwrap_or_else(|e| e.into_inner());
        if asked.get(run).is_some_and(|at| now - at < ASK_HERMES_EVERY) {
            return TurnNow::Going;
        }
        asked.retain(|_, at| now - *at < 3600.0);
        asked.insert(run.to_string(), now);
    }
    match state.hermes.run(&agent, run).await {
        Ok(found) => {
            let status = found.get("status").and_then(Value::as_str).unwrap_or("");
            if crate::jobs::TERMINAL.contains(&status) {
                TurnNow::Over
            } else {
                TurnNow::Going
            }
        }
        Err(crate::hermes::HermesError::Http { status: 404 | 410, .. }) => TurnNow::Over,
        Err(_) => TurnNow::Going, // Hermes restarting: asked again later
    }
}

/// Every second (`watch`): each answer not told yet, its turn over, resumed (see above).
async fn settle(state: &AppState) {
    let config = state.config();
    let now = py::time_time();
    let open: Vec<(String, Place, Decision)> = load(&config)
        .into_iter()
        .filter_map(|(run, place)| place.decided.clone().map(|d| (run, place, d)))
        .collect();
    for (card, place, decision) in open {
        if decision.resuming.as_deref() == Some(BOOT.as_str()) {
            continue; // its turn starting
        }
        if place.taken || now - decision.at > STALE_DAYS * 86400.0 {
            forget(&config, &card);
            continue;
        }
        if now - decision.at < take_grace(&card, &place) {
            continue;
        }
        match turn_now(state, &card, &place, &decision).await {
            TurnNow::Going => {}
            TurnNow::Told => {
                tracing::info!(agent = %place.agent, card = %card, "the turn learnt the card's answer by itself");
                forget(&config, &card);
            }
            TurnNow::Over => resume(state, &card, &place, &decision),
        }
    }
}

/// Card `card`'s answer, not told yet, taken by this process to tell it (false: told, taken by a call, or being told).
fn claim(config: &Config, card: &str) -> bool {
    update(config, card, |p| match p.decided.as_mut() {
        Some(d) if !p.taken && d.resuming.as_deref() != Some(BOOT.as_str()) => {
            d.resuming = Some(BOOT.clone());
            true
        }
        _ => false,
    }) == Some(true)
}

/// The answer `decision` to card `card` claimed by this process, then told in one turn with those of the same
/// conversation answered within `RESUME_GATHER`.
fn resume(state: &AppState, card: &str, place: &Place, decision: &Decision) {
    let config = state.config();
    let Some(agent) = config.agent(Some(&place.agent)).cloned() else {
        forget(&config, card);
        return;
    };
    if !claim(&config, card) {
        return;
    }
    let key = (agent.name.clone(), resume_in(place));
    let first = {
        let mut waiting = RESUMES.lock().unwrap_or_else(|e| e.into_inner());
        let list = waiting.entry(key.clone()).or_default();
        list.push((card.to_string(), place.summary.clone(), allowed(&decision.choice)));
        list.len() == 1
    };
    if !first {
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(RESUME_GATHER).await;
        let decisions = RESUMES.lock().unwrap_or_else(|e| e.into_inner()).remove(&key).unwrap_or_default();
        // one a waiting call returned meanwhile is known already
        let config = state.config();
        let decisions: Decisions = decisions
            .into_iter()
            .filter(|(card, ..)| {
                let taken = of(&config, card).is_none_or(|p| p.taken || p.decided.is_none());
                if taken {
                    forget(&config, card);
                }
                !taken
            })
            .collect();
        if !decisions.is_empty() {
            resume_turn(&state, &agent, &key.1, &decisions).await;
        }
    });
}

async fn resume_turn(state: &AppState, agent: &AgentConfig, conversation: &str, decisions: &Decisions) {
    use crate::background_turn::{Delivery, Kind, Pipeline, Turn};
    let pipeline = Pipeline::of(state);
    let told: Vec<(String, bool)> = decisions.iter().map(|(_, what, ok)| (what.clone(), *ok)).collect();
    let mut turn = Turn::new(agent, Kind::Card, conversation, resume_note(&told));
    turn.budget = Some(RESUME_BUDGET);
    tracing::info!(agent = %agent.name, conversation, cards = decisions.len(), "a card answered: the agent resumes");
    let config = state.config();
    let cards: Vec<&str> = decisions.iter().map(|(card, ..)| card.as_str()).collect();
    // started: the answers leave their places (told now, never again)
    let started = |_: &str| cards.iter().for_each(|card| forget(&config, card));
    match pipeline.run_then(&turn, started).await {
        Ok(ran) if ran.answered() && !crate::background_turn::is_silent(&ran.text) => {
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
        Err(why) if again(&why) => {
            // the conversation still busy, Hermes unreachable: tried again (`settle`)
            tracing::info!(agent = %agent.name, why = %why, "resumed turn not run yet: tried again");
            for card in &cards {
                update(&config, card, |p| {
                    if let Some(d) = p.decided.as_mut() {
                        d.resuming = None;
                    }
                });
            }
        }
        Err(why) => {
            tracing::info!(agent = %agent.name, why = %why, "resumed turn not run");
            cards.iter().for_each(|card| forget(&config, card));
        }
    }
}

/// A resumed turn that could not start now but may later.
fn again(why: &crate::background_turn::NotRun) -> bool {
    use crate::background_turn::NotRun;
    match why {
        NotRun::Busy => true,
        NotRun::Refused(err) => err.is_transport(),
        NotRun::Credit | NotRun::Gone => false,
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
        assert!(note.contains("your turn ended before you were told"));
        assert!(note.contains("- « Vie veut envoyer des données à connect.garmin.com »: ALLOWED"));
        assert!(note.contains("- your request: REFUSED") && note.contains("[SILENT]"));
        // kept with the place, read back by an older file too
        let kept: Place = serde_json::from_str(r#"{"agent": "vie", "session": "s", "report": "s", "at": 1.0}"#).unwrap();
        assert_eq!((kept.run, kept.summary, kept.decided, kept.taken), (None, String::new(), None, false));
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.join("bridge.db").to_string_lossy().into_owned();
        config
    }

    fn decision(followed_since: f64, calls: u32) -> Decision {
        Decision { choice: "once".into(), at: 10.0, followed_since, calls, resuming: None }
    }

    #[test]
    fn an_answer_is_kept_until_told_once_whatever_the_order() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        let place = Place { agent: "vie".into(), session: Some("s".into()), run: Some("run_1".into()), ..Place::default() };
        // no place: nothing kept, nothing to tell
        assert!(!claim(&config, "bip-site-a"));
        taken(&config, "bip-site-a");
        assert_eq!(of(&config, "bip-site-a"), None);
        keep(&config, "bip-site-a", place.clone()).unwrap();
        keep(&config, "bip-secret-b", place.clone()).unwrap();
        // placed, not answered: nothing to claim
        assert!(!claim(&config, "bip-site-a"));
        // answered (the turn going on): claimed once, never twice by this process
        update(&config, "bip-site-a", |p| p.decided = Some(decision(5.0, 2)));
        assert!(claim(&config, "bip-site-a"));
        assert!(!claim(&config, "bip-site-a"));
        assert_eq!(of(&config, "bip-site-a").unwrap().decided.unwrap().resuming.as_deref(), Some(BOOT.as_str()));
        // its turn started: told, gone
        forget(&config, "bip-site-a");
        assert!(!claim(&config, "bip-site-a") && of(&config, "bip-site-a").unwrap().decided.is_none());
        // a bridge restarted before its turn started: claimed again by the new process
        update(&config, "bip-site-a", |p| {
            p.decided = Some(Decision { resuming: Some("another-process".into()), ..decision(5.0, 2) })
        });
        assert!(claim(&config, "bip-site-a"));
        // a waiting call returned it, before or after the answer was kept: never claimed
        taken(&config, "bip-secret-b");
        update(&config, "bip-secret-b", |p| p.decided = Some(decision(5.0, 2)));
        assert!(!claim(&config, "bip-secret-b"));
        let kept = of(&config, "bip-secret-b").unwrap();
        assert!(kept.taken && kept.decided.is_some());
        // a call may wait with no turn followed (Hermes' cron): left to it longer; a site's or a mail card's never waits
        let turnless = Place { run: None, ..place.clone() };
        assert_eq!(take_grace("bip-secret-b", &place), TAKE_GRACE);
        assert_eq!(take_grace("bip-secret-b", &turnless), TAKE_GRACE_NO_TURN);
        assert_eq!(take_grace("bip-conn-c", &turnless), TAKE_GRACE_NO_TURN);
        assert_eq!(take_grace("bip-site-a", &turnless), TAKE_GRACE);
        assert_eq!(take_grace("bip-mail-d", &turnless), TAKE_GRACE);
        // kept on disk: read back by the next process, its place unchanged
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(path(&config)).unwrap()).unwrap();
        assert_eq!(raw["bip-secret-b"]["taken"], json!(true));
        assert_eq!(raw["bip-secret-b"]["decided"]["choice"], json!("once"));
        assert_eq!(raw["bip-secret-b"]["session"], json!("s"));
        assert!(raw["bip-site-a"].get("taken").is_none());
    }

    #[test]
    fn a_turn_learnt_the_answer_only_by_calling_the_cards_own_tool_after_it() {
        let recent = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<VecDeque<String>>();
        let tools = recent(&["mcp_bipagents_files_sandbox_sites_request", "terminal", "mcp_bipagents_files_sandbox_sites"]);
        // two calls since the answer (terminal, the sites' list): learnt
        assert!(told_by("bip-site-x", &tools, 3, 5.0, &decision(5.0, 1)));
        // only the call that asked was before the answer: not learnt
        let asked = recent(&["mcp_bipagents_files_sandbox_sites_request", "terminal"]);
        assert!(!told_by("bip-site-x", &asked, 2, 5.0, &decision(5.0, 1)));
        assert!(!told_by("bip-site-x", &tools, 1, 5.0, &decision(5.0, 1)));
        // the hub follows it again after a restart (its count from the start): not comparable
        assert!(!told_by("bip-site-x", &tools, 3, 99.0, &decision(5.0, 1)));
        assert!(!told_by("bip-site-x", &tools, 0, 5.0, &decision(5.0, 1)));
        // a secret asked again; a mail card has no such tool (always told by a resume)
        let secret = recent(&["mcp_bipagents_connectors_ask_secret"]);
        assert!(told_by("bip-secret-x", &secret, 2, 5.0, &decision(5.0, 1)));
        assert!(!told_by("bip-conn-x", &secret, 2, 5.0, &decision(5.0, 1)));
        assert!(!told_by("bip-mail-x", &tools, 3, 5.0, &decision(5.0, 0)));
        // a tool whose name only ends the same way is another tool
        let other = recent(&["mcp_bipagents_files_notsandbox_sites"]);
        assert!(!told_by("bip-site-x", &other, 2, 5.0, &decision(5.0, 1)));
    }
}
