//! Pages in shared conversations (Alex, 2026-11-09, « oui, lundi »; internal/notes/shared-pages.md).
//!
//! **Whose turn a tool call is** (`who`). The bridge's MCP tools used to close by agent: while a relative's turn went
//! on (agent_shares.rs), or while a conversation shown read-only to friends was busy (read_shares.rs), every call of
//! that agent was closed — the owner's own turns included. Now:
//! - the owner's turns are the owner's, shared conversation or not: everything open, as in their own conversations
//!   (read_shares.rs keeps only its instructions and the group mark of the pages made there);
//! - a relative's turn (« a friend asks », agent_shares.rs) keeps the closures of 2026-10-25 (`RELATIVE_TOOLS`,
//!   Hermes patch 0018: nothing of the owner's is read) with one tool more, `create_page`;
//! - another household's agent or a relative's discussion (peers.rs, rooms_remote.rs): unchanged, closed for the
//!   agent while it goes on.
//!
//! A call is the owner's only when the run hub sees the owner's run inside that very tool and no relative's run
//! inside it ([`crate::runs::RunHub::runs_in_tool`]); when a relative's turn goes on and the hub can't tell, the call
//! is the relative's (closed). Hermes refuses a relative's run every tool outside its guest list on its own side
//! (patch 0018), so the bridge's closure is the second lock.
//!
//! **A page a friend asked for** (`create_page` in a relative's turn) gets three safeguards:
//! - (a) visible only to the group: the page is served to the relative's bridge only for her own household
//!   (`GET /family/s/{agent}/pages/{page}`, the household key, a page of this ledger made for that household), never
//!   by a public link; its HTML carries `<meta name="bipagents-group">` and the apps hide « Partager » for it. A page
//!   made in the owner's turn of a conversation shown to friends carries the same mark (its viewers have it);
//! - (b) marked: a line « Faite à la demande de Marie » at the top of the page (pages.rs `Mark`), and its record keeps
//!   `asked_by`;
//! - (c) a limit: `[shared_pages] friend_pages_per_day` new pages per relative (household) and per day, on this
//!   bridge (default `DEFAULT_PER_DAY`; 0: none), refused past it in plain words. Updating one of her own pages
//!   doesn't count; a page that isn't hers (the owner's, another relative's) can't be updated from her turn.
//!
//! `<data>/shared-pages.json` (0600): `{made: [{agent, page, household, conversation, name, at}]}`.
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// New pages one relative may ask for in a day, by default.
pub const DEFAULT_PER_DAY: u32 = 5;
/// The most `friend_pages_per_day` takes.
pub const MAX_PER_DAY: u32 = 100;
/// Pages the ledger remembers (the oldest go first).
const LEDGER_MAX: usize = 2000;

/// `[shared_pages]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub friend_pages_per_day: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { friend_pages_per_day: DEFAULT_PER_DAY }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let per_day = table
            .get("friend_pages_per_day")
            .and_then(Value::as_i64)
            .map(|n| n.clamp(0, MAX_PER_DAY as i64) as u32)
            .unwrap_or(DEFAULT_PER_DAY);
        Settings { friend_pages_per_day: per_day }
    }
}

/// The relative whose turn a call belongs to.
#[derive(Debug, Clone, PartialEq)]
pub struct Asker {
    pub household: String,
    pub conversation: String,
    /// Their first name as their app sent it (else their household's name).
    pub name: String,
    pub run: String,
}

/// Who a tool call of an agent belongs to.
#[derive(Debug, Clone, PartialEq)]
pub enum Who {
    /// The agent's person (in any conversation, shared ones included).
    Owner,
    /// A relative talking to a shared agent (agent_shares.rs).
    Friend(Asker),
    /// Another household's agent, or a relative's discussion (peers.rs, rooms_remote.rs).
    Guest,
}

/// Whose turn the call of `tool` by `agent` is (the module's comment).
pub fn who(state: &AppState, agent: &str, tool: &str) -> Who {
    let config = state.config();
    if crate::rooms_remote::guest_turn_going(&config, agent) || crate::peers::household_turn(agent) {
        return Who::Guest;
    }
    let going = crate::agent_shares::going_turns(&config, agent);
    decide(going, &state.hub.runs_in_tool(agent, tool))
}

/// [`who`] once the relatives' turns going on and the runs inside the tool are known.
pub fn decide(going: Vec<Asker>, inside: &[String]) -> Who {
    if going.is_empty() {
        return Who::Owner;
    }
    let theirs = going.iter().find(|g| inside.contains(&g.run));
    match theirs {
        Some(asker) => Who::Friend(asker.clone()),
        None if !inside.is_empty() => Who::Owner,
        // the hub can't tell (the call came before its start was seen): a relative's, closed
        None => Who::Friend(going.into_iter().next().expect("not empty")),
    }
}

// -- the ledger -----------------------------------------------------------------------------------------------------

static LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-pages.json")
}

fn load(file: &PathBuf) -> Vec<Value> {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("made").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn save(file: &PathBuf, made: &[Value]) -> anyhow::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = file.with_extension("tmp");
    py::write_with_mode(&tmp.to_string_lossy(), py::dumps(&json!({"made": made})).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, file)?;
    Ok(())
}

fn s(v: &Value, key: &str) -> String {
    tk::str_or_empty(v.get(key))
}

/// The local day of a Unix time.
fn day_of(at: f64) -> Option<chrono::NaiveDate> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(at as i64, 0).single().map(|t| t.date_naive())
}

/// New pages made for `household` on the day of `at`.
fn made_today(made: &[Value], household: &str, at: f64) -> u32 {
    let today = day_of(at);
    made.iter()
        .filter(|m| s(m, "household") == household && day_of(m.get("at").and_then(Value::as_f64).unwrap_or(0.0)) == today)
        .count() as u32
}

/// Whether `agent`'s page `page` was made for `household`.
pub fn made_for(config: &Config, agent: &str, page: &str, household: &str) -> bool {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(&path(config)).iter().any(|m| s(m, "agent") == agent && s(m, "page") == page && s(m, "household") == household)
}

/// The pages of `agent` made for `household` (the timeline shows those to her).
pub fn pages_for(config: &Config, agent: &str, household: &str) -> Vec<String> {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(&path(config))
        .iter()
        .filter(|m| s(m, "agent") == agent && s(m, "household") == household)
        .map(|m| s(m, "page"))
        .collect()
}

/// A new page made for a relative, noted.
pub fn note(config: &Config, agent: &str, page: &str, asker: &Asker) {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(config);
    let mut made = load(&file);
    if made.iter().any(|m| s(m, "agent") == agent && s(m, "page") == page) {
        return;
    }
    made.push(json!({"agent": agent, "page": page, "household": asker.household, "conversation": asker.conversation,
                     "name": asker.name, "at": py::time_time()}));
    let skip = made.len().saturating_sub(LEDGER_MAX);
    if let Err(e) = save(&file, &made[skip..]) {
        tracing::warn!(error = %e, "a page made for a relative not noted");
    }
}

/// A deleted agent: its pages leave the ledger.
pub fn forget(config: &Config, agent: &str) {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(config);
    let made = load(&file);
    let kept: Vec<Value> = made.iter().filter(|m| s(m, "agent") != agent).cloned().collect();
    if kept.len() != made.len() {
        let _ = save(&file, &kept);
    }
}

// -- create_page in a relative's turn -----------------------------------------------------------------------------

/// Why `create_page` is refused in `asker`'s turn (None: it may go on). `arguments` as the agent sent them.
pub fn refused(config: &Config, agent: &str, asker: &Asker, arguments: &Dict) -> Option<Dict> {
    let page = tk::strip(&tk::str_or_empty(arguments.get("page"))).to_lowercase();
    if !page.is_empty() {
        if made_for(config, agent, &page, &asker.household) {
            return None; // one of her own pages, updated: never counted
        }
        tracing::info!(agent = %agent, household = %asker.household, "a relative's turn may not change a page that isn't theirs");
        return Some(tk::tool_failure(
            &format!(
                "{page}: not a page made for {} in this conversation. Here you can only make a new page, or change one \
you made for them; your person's pages stay theirs.",
                asker.name
            ),
            "I can make you a new page, but not change one that wasn't made for you.",
        ));
    }
    let limit = config.shared_pages.friend_pages_per_day;
    let today = {
        let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        made_today(&load(&path(config)), &asker.household, py::time_time())
    };
    if today < limit {
        return None;
    }
    tracing::info!(agent = %agent, household = %asker.household, limit = limit, "a relative's pages for the day: limit reached");
    let plain = if limit == 0 {
        "Pages can't be made for you here: ask in another way, I answer in text.".to_string()
    } else {
        format!(
            "You already asked for {limit} page{} today, the most for one day here: you can ask for more tomorrow. I \
can answer in text meanwhile.",
            if limit == 1 { "" } else { "s" }
        )
    };
    Some(tk::tool_failure(
        &format!("No page: {} reached today's limit of new pages ({limit}). Answer in text instead.", asker.name),
        &plain,
    ))
}

/// The mark of a page made in `asker`'s turn (pages.rs).
pub fn mark_for(config: &Config, agent: &str, asker: &Asker) -> crate::pages::Mark {
    crate::pages::Mark {
        group: true,
        asked_by: Some(asker.name.clone()),
        lang: crate::names::agent_language(config, agent),
    }
}

/// The arguments a relative's turn may give `create_page`: its Carnet link left out (the owner's Carnet stays closed).
pub fn friend_arguments(arguments: &Dict) -> Dict {
    let mut kept = arguments.clone();
    kept.remove("notebook");
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asker(run: &str, household: &str) -> Asker {
        Asker { household: household.into(), conversation: "c1".into(), name: "Marie".into(), run: run.into() }
    }

    #[test]
    fn a_call_is_the_owners_unless_a_relatives_run_may_have_made_it() {
        // nobody else talks to the agent: the owner's
        assert_eq!(decide(vec![], &[]), Who::Owner);
        assert_eq!(decide(vec![], &["r-owner".into()]), Who::Owner);
        // Marie's turn goes on, the owner's run is inside the tool, hers isn't: the owner's
        assert_eq!(decide(vec![asker("r-marie", "marie")], &["r-owner".into()]), Who::Owner);
        // hers is inside it (the owner's too, or not): hers
        assert_eq!(decide(vec![asker("r-marie", "marie")], &["r-owner".into(), "r-marie".into()]),
                   Who::Friend(asker("r-marie", "marie")));
        // the hub can't tell: hers (closed)
        assert_eq!(decide(vec![asker("r-marie", "marie")], &[]), Who::Friend(asker("r-marie", "marie")));
        // two relatives: the one inside the tool
        assert_eq!(decide(vec![asker("r-marie", "marie"), asker("r-paul", "paul")], &["r-paul".into()]),
                   Who::Friend(asker("r-paul", "paul")));
    }

    #[test]
    fn the_days_count_is_per_household_and_per_local_day() {
        let now = py::time_time();
        let made = vec![
            json!({"household": "marie", "at": now}),
            json!({"household": "marie", "at": now - 3.0 * 86400.0}),
            json!({"household": "paul", "at": now}),
        ];
        assert_eq!(made_today(&made, "marie", now), 1);
        assert_eq!(made_today(&made, "paul", now), 1);
        assert_eq!(made_today(&made, "lea", now), 0);
    }

    #[test]
    fn settings_read_the_limit_within_bounds() {
        assert_eq!(Settings::from_table(&json!({})).friend_pages_per_day, DEFAULT_PER_DAY);
        assert_eq!(Settings::from_table(&json!({"friend_pages_per_day": 2})).friend_pages_per_day, 2);
        assert_eq!(Settings::from_table(&json!({"friend_pages_per_day": 0})).friend_pages_per_day, 0);
        assert_eq!(Settings::from_table(&json!({"friend_pages_per_day": 5000})).friend_pages_per_day, MAX_PER_DAY);
    }

    #[test]
    fn a_relatives_page_has_no_carnet_link() {
        let mut args = Dict::new();
        args.insert("html".into(), json!("<p>x</p>"));
        args.insert("notebook".into(), json!("Santé"));
        let kept = friend_arguments(&args);
        assert!(kept.get("notebook").is_none() && kept.get("html").is_some());
    }
}
