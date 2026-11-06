//! How much the person validates (Alex, 6/11: « J'aimerai quand même éviter un trop grand nombre de validations par
//! l'utilisateur s'il est OK pour valider la plupart automatiquement. Il faut un bouton "me faire valider la plupart" /
//! "que les critiques". » — and « Quand je parle des validations, je parle de manière générale dans toute l'app. »).
//!
//! The daily message's hour and the loop's period can be moved for the black-box tests (`BIPAGENTS_APPROVAL_DIGEST_HOUR`,
//! `BIPAGENTS_APPROVAL_DIGEST_EVERY`).
//!
//! Two levels, for the person (every agent of this place), and per agent if they want:
//!
//! - **« Tout valider moi-même »** (`all`, the default, what every place did before): every card is asked.
//! - **« Seulement le critique »** (`critical`): what is not critical is approved by the bridge at once, as the person's
//!   « Oui » would be (the same code: the same caps, brakes, sandbox and undo), not pushed, and logged as an « auto »
//!   line in the Boîte (what, which agent, when, « Annuler » when it can be undone); once a day, at `DIGEST_HOUR`, one
//!   Boîte message sums the day's up for each agent that had some (the only push). The agents are told, in their
//!   personality (`RULE`), to go ahead without asking « shall I…? » for what is not critical: the confirmation questions
//!   that are really validations go too.
//!
//! ONE table says what is critical (`TABLE`): anything that spends money or uses a card, a password, a secret or a
//! login, sends anything in the person's name, deletes for good, runs new code with secrets (a connector), shares with
//! other people or lets them in, runs a command on the person's machine, uses their home connection (the phone exit),
//! or goes past a budget. Everything the bridge asks is classified with it: its own cards (`classify_card`), Hermes'
//! approvals (`classify_hermes`), and the steps that are not cards (`task_start`, `task_more`, `guide_proposal`…).
//!
//! An older app knows nothing of this: the level stays « Tout valider » until a newer app sets it (`chosen`), and the
//! lines it can't show are summed up in the daily Boîte message it shows like any other.
//!
//!     GET  /v1/approval-level                  {level, chosen, agents: {agent: level}, kinds: [{kind, critical, undo}]}
//!     PUT  /v1/approval-level                  {level}                                  → the same
//!     PUT  /v1/agents/{agent}/approval-level   {level | null (follows the person's)}    → the same
//!     GET  /v1/approvals/auto?agent=&limit=    {items: [{id, agent, kind, summary, at, run_id, session_id, undo,
//!                                                        undone}]}   newest first
//!     POST /v1/approvals/auto/{id}/undo        {ok, item} | 409 {code: not_undoable | undone, detail} | 404
//!
//!     data_dir/approval-level.json (0600)      the levels, the auto lines (`KEEP_DAYS`), the digests sent
//!
//! No Python counterpart.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

const FILE: &str = "approval-level.json";
/// Auto lines kept this long.
const KEEP_DAYS: f64 = 30.0;
/// The daily message, from this local hour.
pub const DIGEST_HOUR: u32 = 19;
/// A background task an agent proposes starts by itself up to this budget (`jobs::BUDGETS` « medium »).
pub const TASK_AUTO_MAX_EUR: f64 = 0.50;

// -- the table ---------------------------------------------------------------------------------------------------------

/// One kind of validation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rule {
    pub kind: &'static str,
    /// Always asked, whatever the level.
    pub critical: bool,
    /// An auto-approved one can be undone from its Boîte line.
    pub undo: bool,
    /// Where it comes from (a card's run id prefix, Hermes, a route).
    pub from: &'static str,
    /// Why, in one line.
    pub why: &'static str,
}

const fn rule(kind: &'static str, critical: bool, undo: bool, from: &'static str, why: &'static str) -> Rule {
    Rule { kind, critical, undo, from, why }
}

/// THE table: every validation the bridge asks, critical or not. Anything not in it is critical (`rule_of`).
pub const TABLE: &[Rule] = &[
    // -- not critical: approved at once under « Seulement le critique »
    rule("mail_archive", false, true, "bip-mail- archive", "filed in the archive, nothing deleted; undone by putting them back"),
    rule("mail_mark_read", false, true, "bip-mail- mark_read", "only a flag; undone by marking them unread"),
    rule("mail_move", false, true, "bip-mail- move", "moved to a folder, nothing deleted; undone by moving them back"),
    rule("mail_rule", false, true, "bip-mailrule- (not trash)", "a sort rule that files or marks read, never deletes; stopped and its last run put back"),
    rule("routine_way", false, false, "bip-rout-", "the routine runs anyway; only whether its reports are pushed (the agent's suggestion kept, changed on its page)"),
    rule("task_start", false, true, "propose_task (≤ 0,50 €)", "a background task within a small budget of the month's credit; paused from its line"),
    rule("service_write", false, false, "Hermes: MCP tool that creates or changes, on a service the person connected", "writes in the person's own service (a Notion page, a note): nothing sent to anyone, nothing deleted"),
    rule("files_write", false, false, "Hermes: bipagents_files (not delete)", "files in the agent's own space"),
    // -- critical: always asked
    rule("payment", true, false, "Hermes: card_pay, Link, hand-over elicitations", "spends money or uses a payment card"),
    rule("secret", true, false, "bip-secret-", "a password, key or code typed for the agent"),
    rule("vault_login", true, false, "bip-login-", "uses one of the person's logins and passwords"),
    rule("connector", true, false, "bip-conn-", "runs new code with the person's secrets"),
    rule("site_send", true, false, "bip-site-", "data leaves to a site the person never validated (an upload, an export): reading sites never asks"),
    rule("mail_trash", true, false, "bip-mail- trash", "deletes (to the trash, then gone)"),
    rule("mail_rule_trash", true, false, "bip-mailrule- trash", "deletes every new matching mail"),
    rule("held_mail", true, false, "agent_email held", "sends an e-mail in the person's name to a new address"),
    rule("family_send", true, false, "tell_family", "sends a message to the family"),
    rule("meet", true, false, "meet", "shares the person's free time with another household"),
    rule("files_delete", true, false, "Hermes: bipagents_files shared_files_delete", "deletes files for good (a shared folder others read)"),
    rule("service_send", true, false, "Hermes: MCP tool that sends, deletes, pays, or isn't recognised", "sends, deletes or pays in an outside service in the person's name"),
    rule("elicitation", true, false, "Hermes: an MCP server's own question", "an outside server asks the person directly"),
    rule("command", true, false, "Hermes: terminal command, script", "runs a command on the person's machine (an isolated sandbox never asks)"),
    rule("own_instructions", true, false, "Hermes: protected_instruction_file", "the agent changing its own instructions"),
    rule("plugin_rule", true, false, "Hermes: a plugin's rule", "a rule the person or a plugin set to always ask"),
    rule("computer_use", true, false, "Hermes: cua:", "acts on the person's computer screen"),
    rule("trigger", true, false, "bip-trig-", "lets an e-mail or a link from outside start the agent"),
    rule("phone_exit", true, false, "bip-exit-", "uses the person's phone and home connection"),
    rule("task_large", true, false, "propose_task (> 0,50 €)", "a big part of the month's credit"),
    rule("task_more", true, false, "a task's « Continuer (+…) »", "spends past the budget the person agreed"),
    rule("guide_proposal", true, false, "propose_agent, propose_team, propose_room, suggest_switch, propose_profile", "creates new agents or rooms (new voices that cost credit, named by the person) or rewrites an agent's role"),
    rule("space", true, false, "bip-space-", "the place is full: which files to delete or send"),
    rule("tip", true, false, "bip-tip-", "not a validation: a tip the person reads"),
    rule("relative", true, false, "a relative's turn", "a relative's own approvals: theirs to answer"),
    rule("question", true, false, "Hermes clarify", "a question needs the person's words (confirmations: the agents don't ask them, RULE)"),
];

/// The kind's rule; an unknown kind is critical.
pub fn rule_of(kind: &str) -> Rule {
    TABLE.iter().find(|r| r.kind == kind).copied().unwrap_or(Rule {
        kind: "unknown",
        critical: true,
        undo: false,
        from: "",
        why: "not recognised: asked",
    })
}

fn field(item: &Dict, obj: &str, key: &str) -> String {
    item.get(obj).and_then(|o| o.get(key)).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The kind of one of the bridge's own cards (`/v1/approvals`' shape).
pub fn classify_card(item: &Dict) -> &'static str {
    let run = item.get("run_id").and_then(Value::as_str).unwrap_or("");
    if run.starts_with(crate::mail_tidy::RUN_PREFIX) {
        let action = field(item, "bip_mail", "action");
        return match action.as_str() {
            "archive" => "mail_archive",
            "mark_read" => "mail_mark_read",
            a if a.starts_with("move:") => "mail_move",
            _ => "mail_trash", // trash, or anything new: asked
        };
    }
    if run.starts_with(crate::mail_rules::RUN_PREFIX) {
        let action = field(item, "bip_mail_rule", "action");
        let known = action.split('+').all(|a| a == "archive" || a == "mark_read" || a.starts_with("move:"));
        return if known && !action.is_empty() { "mail_rule" } else { "mail_rule_trash" };
    }
    const PREFIXES: [(&str, &str); 10] = [
        (crate::routines::RUN_PREFIX, "routine_way"),
        (crate::sandbox_sites::RUN_PREFIX, "site_send"),
        (crate::vault_logins::RUN_PREFIX, "vault_login"),
        (crate::secrets::RUN_PREFIX, "secret"),
        (crate::connectors::RUN_PREFIX, "connector"),
        (crate::storage_watch::RUN_PREFIX, "space"),
        (crate::feature_tips::RUN_PREFIX, "tip"),
        (crate::triggers::RUN_PREFIX, "trigger"),
        ("bip-exit-", "phone_exit"), // phone_exit (branch phone-exit-mvp)
        ("bip-", "unknown"),
    ];
    PREFIXES.iter().find(|(p, _)| run.starts_with(p)).map(|(_, k)| *k).unwrap_or("unknown")
}

static TRUST_GATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^MCP tool '([^']+)' on UNTRUSTED server '([^']+)'").expect("valid regex"));
static WORDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("valid regex"));

/// A tool that only creates or changes something in the person's own service, by its name's words (anything that
/// sends, shares, deletes, pays, or reaches people, money, mail or a calendar is not).
fn writes_only(tool: &str) -> bool {
    let lower = tool.to_lowercase();
    let words: Vec<&str> = WORDS.split(&lower).filter(|w| !w.is_empty()).collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(w));
    const RISKY: [&str; 42] = [
        "delete", "remove", "trash", "unlink", "rm", "destroy", "purge", "drop", "clear", "wipe", "send", "post",
        "reply", "publish", "comment", "forward", "share", "invite", "notify", "message", "dm", "chat", "pay",
        "payment", "charge", "transfer", "order", "buy", "purchase", "refund", "mail", "email", "sms", "call",
        "calendar", "event", "contact", "contacts", "member", "permission", "permissions", "tweet",
    ];
    const WRITES: [&str; 20] = [
        "create", "add", "new", "insert", "append", "write", "save", "update", "edit", "patch", "set", "rename",
        "modify", "change", "move", "duplicate", "copy", "tag", "label", "upsert",
    ];
    !has(&RISKY) && has(&WRITES)
}

/// The kind of a Hermes approval (`approval.request`'s data: command, description, pattern_key).
pub fn classify_hermes(data: &Map<String, Value>) -> &'static str {
    let text = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let (command, key) = (text("command"), text("pattern_key"));
    if command.starts_with("Card payment pay_") {
        return "payment";
    }
    if let Some(caps) = TRUST_GATE.captures(&command) {
        let (tool, server) = (caps[1].to_string(), caps[2].to_string());
        if server == crate::sandbox::SERVER_NAME {
            return if tool.contains("delete") || !writes_only(&tool) && tool != "shared_files_write" {
                "files_delete"
            } else {
                "files_write"
            };
        }
        if server.contains("pay") || server.contains("checkout") || server.contains("link") {
            return "payment";
        }
        return if writes_only(&tool) { "service_write" } else { "service_send" };
    }
    match key.as_str() {
        "mcp_elicitation" => "elicitation",
        "protected_instruction_file" => "own_instructions",
        k if k.starts_with("cua:") => "computer_use",
        _ if command.contains("(plugin approval rule)") => "plugin_rule",
        _ => "command",
    }
}

// -- the levels --------------------------------------------------------------------------------------------------------

pub const ALL: &str = "all";
pub const CRITICAL: &str = "critical";

fn level_word(text: &str) -> Option<&'static str> {
    match text.trim().to_lowercase().as_str() {
        ALL | "everything" | "tout" => Some(ALL),
        CRITICAL | "critical_only" | "critique" => Some(CRITICAL),
        _ => None,
    }
}

/// An approved-by-itself line of the Boîte.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub id: String,
    pub agent: String,
    pub kind: String,
    pub summary: String,
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// What undoes it (None: nothing to undo).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<Undo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_at: Option<f64>,
    /// Summed up in a daily message already.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub digested: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "what", rename_all = "snake_case")]
pub enum Undo {
    /// mail_tidy's job: its messages put back.
    MailJob { job: String },
    /// mail_rules' rule: stopped, its last run put back.
    MailRule { rule: String },
    /// jobs.rs' task: paused.
    Task { job: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Book {
    /// None: never chosen (« Tout valider »).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    level: Option<String>,
    #[serde(default)]
    agents: BTreeMap<String, String>,
    #[serde(default)]
    lines: Vec<Line>,
    /// agent → the local day of its last daily message (« 2026-11-06 »).
    #[serde(default)]
    digests: BTreeMap<String, String>,
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
    book.lines.retain(|l| now - l.at < KEEP_DAYS * 86400.0);
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
        tracing::warn!(target: "bipbridge::approval_level", error = %err, "approval level not kept");
    }
    out
}

/// The person's level (« all » until a newer app set it).
pub fn person_level(config: &Config) -> &'static str {
    load(config).level.as_deref().and_then(level_word).unwrap_or(ALL)
}

/// `agent`'s level: its own when the person set one, else the person's.
pub fn level_for(config: &Config, agent: &str) -> &'static str {
    let book = load(config);
    book.agents
        .get(agent)
        .and_then(|l| level_word(l))
        .or_else(|| book.level.as_deref().and_then(level_word))
        .unwrap_or(ALL)
}

/// Approved by the bridge at once: `agent` is « Seulement le critique » and the kind is not critical.
pub fn auto_for(config: &Config, agent: &str, kind: &str) -> bool {
    !rule_of(kind).critical && level_for(config, agent) == CRITICAL
}

pub fn set_level(config: &Config, level: &str) -> Result<(), String> {
    let level = level_word(level).ok_or("level: « all » or « critical »")?;
    change(config, |b| b.level = Some(level.to_string()));
    Ok(())
}

pub fn set_agent_level(config: &Config, agent: &str, level: Option<&str>) -> Result<(), String> {
    let level = match level {
        Some(l) => Some(level_word(l).ok_or("level: « all », « critical » or null")?),
        None => None,
    };
    change(config, |b| match level {
        Some(l) => {
            b.agents.insert(agent.to_string(), l.to_string());
        }
        None => {
            b.agents.remove(agent);
        }
    });
    Ok(())
}

pub fn view(config: &Config) -> Value {
    let book = load(config);
    let agents: Map<String, Value> = book
        .agents
        .iter()
        .filter(|(a, _)| config.agents.contains_key(a.as_str()))
        .filter_map(|(a, l)| level_word(l).map(|l| (a.clone(), json!(l))))
        .collect();
    let kinds: Vec<Value> =
        TABLE.iter().map(|r| json!({"kind": r.kind, "critical": r.critical, "undo": r.undo})).collect();
    json!({
        "level": book.level.as_deref().and_then(level_word).unwrap_or(ALL),
        "chosen": book.level.is_some(),
        "agents": agents,
        "kinds": kinds,
    })
}

/// A deleted agent: its level and lines go.
pub fn forget(config: &Config, agent: &str) {
    change(config, |b| {
        b.agents.remove(agent);
        b.lines.retain(|l| l.agent != agent);
        b.digests.remove(agent);
    });
}

// -- the agents' rule (their personality) -------------------------------------------------------------------------------

pub const RULE_START: &str = "<!-- bipagents:approval-level -->";
pub const RULE_END: &str = "<!-- /bipagents:approval-level -->";
pub const RULE: &str = concat!(
    "Your person chose to validate only what is critical. Don't ask them « shall I…? » or « do you want me to…? » ",
    "before what is not critical (reading, sorting, archiving or moving mail, a sort rule, a draft, a page, files in your ",
    "own space, a routine, a background task within its budget, a note in a service they connected): do it, then say ",
    "what you did; the app shows it to them with a way to undo it. Ask first, always, before what is critical: paying or ",
    "using a card, a password, a secret or a login, sending anything in their name, deleting for good, sharing with ",
    "other people."
);

/// The personality without the rule's block.
pub fn split_block(soul: &str) -> (String, bool) {
    let (Some(start), Some(end)) = (soul.find(RULE_START), soul.find(RULE_END)) else {
        return (soul.to_string(), false);
    };
    if end < start {
        return (soul.to_string(), false);
    }
    (format!("{}{}", soul[..start].trim_end(), &soul[end + RULE_END.len()..]), true)
}

/// The personality with the rule's block when `agent` is « Seulement le critique » (agents_admin.rs `soul_text`).
pub fn with_block(config: &Config, agent: &str, soul: &str) -> String {
    let (body, _) = split_block(soul);
    if level_for(config, agent) != CRITICAL {
        return body;
    }
    format!("{}\n\n{RULE_START}\n{RULE}\n{RULE_END}\n", body.trim_end())
}

/// The personalities rendered again after a level changed (the block in or out).
fn render_souls(config: &Config, agents: &[String]) {
    for name in agents {
        if let Some(agent) = config.agent(Some(name)) {
            if let Err(err) = crate::agents_admin::render_soul(config, agent) {
                tracing::warn!(target: "bipbridge::approval_level", agent = %name, error = %format!("{err:#}"), "personality not rendered");
            }
        }
    }
}

// -- the auto lines ------------------------------------------------------------------------------------------------------

/// One approval made by the bridge at once: kept for the Boîte, logged.
pub fn record(config: &Config, line: Line) -> Line {
    let line = Line { id: if line.id.is_empty() { py::token_hex(6) } else { line.id }, at: py::time_time(), ..line };
    tracing::info!(target: "bipbridge::approval_level", agent = %line.agent, kind = %line.kind, summary = %line.summary, "approved by the level");
    change(config, |b| b.lines.push(line.clone()));
    line
}

fn iso(at: f64) -> String {
    let micros = (at * 1_000_000.0).round() as i64;
    let when = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros).unwrap_or_default();
    when.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn line_view(line: &Line) -> Value {
    json!({
        "id": line.id, "agent": line.agent, "kind": line.kind, "summary": line.summary, "at": iso(line.at),
        "run_id": line.run_id, "session_id": line.session_id,
        "undo": line.undo.is_some() && line.undone_at.is_none(), "undone": line.undone_at.is_some(),
    })
}

/// The lines, newest first.
pub fn lines(config: &Config, agent: Option<&str>, limit: usize) -> Vec<Line> {
    let mut all: Vec<Line> = load(config).lines.into_iter().filter(|l| agent.is_none_or(|a| l.agent == a)).collect();
    all.sort_by(|a, b| b.at.total_cmp(&a.at));
    all.truncate(limit);
    all
}

/// Why an undo was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum UndoRefused {
    NotFound,
    NotUndoable,
    Undone,
    Failed(String),
}

/// Line `id` undone (its mails put back, its rule stopped, its task paused).
pub async fn undo(state: &AppState, id: &str) -> Result<Line, UndoRefused> {
    let config = state.config();
    let line = load(&config).lines.into_iter().find(|l| l.id == id).ok_or(UndoRefused::NotFound)?;
    if line.undone_at.is_some() {
        return Err(UndoRefused::Undone);
    }
    let undo = line.undo.clone().ok_or(UndoRefused::NotUndoable)?;
    let (c, agent) = (config.clone(), line.agent.clone());
    match undo {
        Undo::MailJob { job } => {
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                let back = crate::mail_tidy::undo(&c, &agent, &job)?;
                crate::mail_tidy::start(&c, &back.id, crate::mailbox::default_connect());
                Ok(())
            })
            .await
            .map_err(|e| UndoRefused::Failed(e.to_string()))?
            .map_err(UndoRefused::Failed)?;
        }
        Undo::MailRule { rule } => {
            tokio::task::spawn_blocking(move || -> Result<(), String> {
                // its last run put back (nothing filed yet: nothing to put back), then stopped
                let _ = crate::mail_rules::undo_last(&c, Some(&agent), &rule, &crate::mailbox::default_connect());
                crate::mail_rules::stop(&c, Some(&agent), &rule).map_err(|e| e.to_string())?;
                Ok(())
            })
            .await
            .map_err(|e| UndoRefused::Failed(e.to_string()))?
            .map_err(UndoRefused::Failed)?;
        }
        Undo::Task { job } => {
            crate::jobs::jobs_of(state).pause(&job).await.map_err(|e| UndoRefused::Failed(e.to_string()))?;
        }
    }
    let now = py::time_time();
    let done = change(&config, |b| {
        let l = b.lines.iter_mut().find(|l| l.id == id)?;
        l.undone_at = Some(now);
        Some(l.clone())
    });
    tracing::info!(target: "bipbridge::approval_level", agent = %line.agent, kind = %line.kind, "auto approval undone");
    done.ok_or(UndoRefused::NotFound)
}

// -- what the bridge approves at once ---------------------------------------------------------------------------------

/// The choice that approves a card of this kind (the routine's question: its suggestion).
fn approving_choice(item: &Dict, kind: &str) -> String {
    if kind == "routine_way" {
        let suggested = field(item, "bip_routine", "suggested");
        return if suggested == crate::routines::SILENT_CHOICE { suggested } else { crate::routines::NOTIFY.into() };
    }
    "once".into()
}

fn undo_of(item: &Dict, kind: &str) -> Option<Undo> {
    let id = item.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
    if id.is_empty() {
        return None;
    }
    match kind {
        "mail_archive" | "mail_mark_read" | "mail_move" => Some(Undo::MailJob { job: id }),
        "mail_rule" => Some(Undo::MailRule { rule: id }),
        _ => None,
    }
}

/// Would a card like `item` be approved at once for `agent`? (Its notification is then not sent.)
pub fn auto_card(config: &Config, agent: &str, item: &Dict) -> bool {
    auto_for(config, agent, classify_card(item))
}

/// After a tool call of `agent`: the cards it made (not in `before`) that are not critical, under « Seulement le
/// critique », answered as the person's « Oui » would, logged in the Boîte, told to the agent by the call's result
/// (`taken`: no turn resumed for them). Their sentences.
pub async fn settle_new(
    state: &AppState,
    agent: &AgentConfig,
    before: &std::collections::HashSet<String>,
    session: Option<&str>,
) -> Vec<String> {
    let config = state.config();
    if level_for(&config, &agent.name) != CRITICAL {
        return Vec::new();
    }
    let cards = crate::card_place::bridge_cards(state, &config, Some(&agent.name));
    let mut done = Vec::new();
    for item in cards {
        let run_id = item.get("run_id").and_then(Value::as_str).unwrap_or("").to_string();
        let kind = classify_card(&item);
        if before.contains(&run_id) || !auto_for(&config, &agent.name, kind) {
            continue;
        }
        let choice = approving_choice(&item, kind);
        if !crate::app::answer_bridge_card(state, agent, &run_id, &choice).await {
            continue;
        }
        crate::card_place::taken(&config, &run_id);
        let summary = item.get("bip_summary").and_then(Value::as_str).unwrap_or("").to_string();
        let place = crate::card_place::of(&config, &run_id);
        let session = place.and_then(|p| p.report.or(p.session)).or(session.map(str::to_string));
        record(
            &config,
            Line {
                agent: agent.name.clone(),
                kind: kind.to_string(),
                summary: summary.clone(),
                run_id: Some(run_id),
                session_id: session,
                undo: undo_of(&item, kind),
                ..Line::default()
            },
        );
        done.push(summary);
    }
    done
}

/// What the agent reads in its call's result when its cards were approved at once.
pub fn told(summaries: &[String]) -> String {
    format!(
        "Approved at once (your person validates only what is critical), being done now: {}. No need to wait for \
         their OK; tell them in a few words what you did (the app shows it with « Annuler »).",
        summaries.iter().map(|s| format!("« {s} »")).collect::<Vec<_>>().join(", ")
    )
}

/// `propose_task` under « Seulement le critique » with a budget up to `TASK_AUTO_MAX_EUR`: the task started at once
/// (the same start as the person's « Lancer »: credit, one task per agent, the place's room), logged in the Boîte.
/// None: asked as usual (the caller proposes it).
pub async fn auto_task(jobs: &std::sync::Arc<crate::jobs::Jobs>, agent: &AgentConfig, arguments: &Dict) -> Option<Dict> {
    let config = jobs.config.get();
    if !auto_for(&config, &agent.name, "task_start") {
        return None;
    }
    let budget = match arguments.get("budget") {
        Some(v) if py::truthy(v) => py::py_str(v),
        _ => "medium".into(),
    };
    let euros = crate::jobs::BUDGETS.iter().find(|(n, _)| *n == budget).map(|(_, e)| *e).unwrap_or(0.5);
    let prompt = crate::tools_knowledge::str_or_empty(arguments.get("prompt"));
    if euros > TASK_AUTO_MAX_EUR || prompt.trim().chars().count() < crate::jobs::MIN_PROMPT {
        return None; // a big one: proposed; no task in it: propose_tool says so
    }
    let title = crate::tools_knowledge::str_or_empty(arguments.get("title"));
    let from = jobs.hub.as_ref().and_then(|hub| hub.asking_session(&agent.name));
    let job = match jobs.propose_quiet(agent, &title, crate::tools_knowledge::head(&prompt, 8000), &budget, from.as_deref()).await {
        Ok(job) => job,
        Err(_) => return None, // too many waiting…: propose_tool says why
    };
    let id = crate::tools_knowledge::str_or_empty(job.get("id"));
    match jobs.start(&id, None).await {
        Ok(_) => {
            let shown = if title.trim().is_empty() { crate::tools_knowledge::head(&prompt, 80).to_string() } else { title.clone() };
            record(
                &config,
                Line {
                    agent: agent.name.clone(),
                    kind: "task_start".into(),
                    summary: format!("Tâche de fond lancée : {shown} ({})", crate::jobs::euros_text(euros)),
                    session_id: from,
                    undo: Some(Undo::Task { job: id }),
                    ..Line::default()
                },
            );
            Some(crate::tools_knowledge::tool_text(
                "Started at once (your person lets background tasks within a small budget start by themselves): it \
                 works in the background and reports when done. Tell them so in a few words.",
                false,
            ))
        }
        Err(err) => {
            // not startable now (credit used, another task running…): proposed as usual, told to the person
            jobs.tell_proposed(agent, &job).await;
            Some(crate::tools_knowledge::tool_text(
                &format!("Proposed (it could not start by itself now: {err}). It starts when the person starts it in « Tâches de fond »: tell them so."),
                false,
            ))
        }
    }
}

/// A Hermes approval (runs.rs `on_approval`) the person's level approves at once: true when it does (the caller
/// answers Hermes « once » and pushes nothing). A relative's turn never.
pub fn auto_hermes(config: &Config, agent: &str, run_id: &str, data: &Map<String, Value>) -> Option<&'static str> {
    if crate::agent_shares::relative_run(config, run_id) {
        return None;
    }
    let kind = classify_hermes(data);
    auto_for(config, agent, kind).then_some(kind)
}

/// Logs an approval runs.rs answered at once.
pub fn record_hermes(config: &Config, agent: &str, kind: &str, summary: &str, run_id: &str, session: Option<&str>) {
    record(
        config,
        Line {
            agent: agent.to_string(),
            kind: kind.to_string(),
            summary: summary.to_string(),
            run_id: Some(run_id.to_string()),
            session_id: session.map(str::to_string),
            ..Line::default()
        },
    );
}

// -- the daily message ---------------------------------------------------------------------------------------------------

/// The day's message for `agent` when due (`now` local at or after `DIGEST_HOUR`, none sent today, lines not summed
/// up yet): (its id, title, text); the lines then marked.
pub fn take_digest(config: &Config, agent: &str, lang: &str, now: chrono::DateTime<chrono::Local>) -> Option<(String, String, String)> {
    use chrono::Timelike;
    if now.hour() < digest_hour() {
        return None;
    }
    let day = now.format("%Y-%m-%d").to_string();
    change(config, |b| {
        if b.digests.get(agent) == Some(&day) {
            return None;
        }
        let fresh: Vec<&mut Line> = b.lines.iter_mut().filter(|l| l.agent == agent && !l.digested).collect();
        if fresh.is_empty() {
            return None;
        }
        let shown: Vec<String> = fresh.iter().map(|l| format!("- {}", l.summary)).collect();
        for l in fresh {
            l.digested = true;
        }
        b.digests.insert(agent.to_string(), day.clone());
        let n = shown.len();
        let fr = crate::names::names(lang).lang == "fr";
        let (title, head, tail) = if fr {
            (
                "Validé pour toi",
                if n == 1 {
                    "Aujourd'hui, une chose a été validée sans te demander (tu valides seulement le critique) :".to_string()
                } else {
                    format!("Aujourd'hui, {n} choses ont été validées sans te demander (tu valides seulement le critique) :")
                },
                "Tu peux les annuler dans la Boîte, ou tout valider toi-même dans Réglages › Validations.",
            )
        } else {
            (
                "Approved for you",
                format!("Today, {n} thing{} went through without asking you (you validate only what is critical):", if n == 1 { "" } else { "s" }),
                "You can undo them in the Inbox, or validate everything yourself in Settings › Approvals.",
            )
        };
        Some((format!("auto-digest:{agent}:{day}"), title.to_string(), format!("{head}\n{}\n\n{tail}", shown.join("\n"))))
    })
}

/// The daily message's hour (`BIPAGENTS_APPROVAL_DIGEST_HOUR`: the black-box tests' 0).
fn digest_hour() -> u32 {
    std::env::var("BIPAGENTS_APPROVAL_DIGEST_HOUR").ok().and_then(|h| h.trim().parse().ok()).filter(|h| *h < 24).unwrap_or(DIGEST_HOUR)
}

/// Every ten minutes (`BIPAGENTS_APPROVAL_DIGEST_EVERY` seconds: the black-box tests'), for ever: the daily messages
/// due.
pub async fn run_forever(state: AppState) {
    let every = std::env::var("BIPAGENTS_APPROVAL_DIGEST_EVERY").ok().and_then(|s| s.trim().parse().ok()).unwrap_or(600u64);
    loop {
        tokio::time::sleep(Duration::from_secs(every.max(1))).await;
        digest_now(&state).await;
    }
}

/// The daily messages due now, stored in the Boîte (pushed: the day's one notification). How many.
pub async fn digest_now(state: &AppState) -> usize {
    let config = state.config();
    let c = config.clone();
    let due = tokio::task::spawn_blocking(move || {
        let now = chrono::Local::now();
        c.agents
            .values()
            .filter_map(|agent| {
                let lang = crate::handback::language_of(&c, agent);
                take_digest(&c, &agent.name, &lang, now).map(|d| (agent.clone(), d))
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let n = due.len();
    for (agent, (id, title, text)) in due {
        let note = crate::outbox::BoiteMessage { id: Some(id), title: Some(title), ..crate::outbox::BoiteMessage::new(text) };
        if let Err(e) = state.outbox.ingest(&agent, note, true).await {
            tracing::warn!(agent = %agent.name, error = %format!("{e:#}"), "approvals' daily message not stored");
        }
    }
    n
}

// -- routes --------------------------------------------------------------------------------------------------------------

fn level_of_body(body: &Bytes) -> Result<Option<String>, HttpError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| HttpError::new(422, "a JSON body: {level}"))?;
    match value.get("level") {
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(HttpError::new(422, "level: « all », « critical » or null")),
    }
}

async fn get_level(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(view(&state.config())))
}

/// The souls whose block follows the person's level.
fn following(config: &Config) -> Vec<String> {
    let book = load(config);
    config.agents.keys().filter(|a| !book.agents.contains_key(a.as_str())).cloned().collect()
}

async fn put_level(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let level = level_of_body(&body)?.ok_or_else(|| HttpError::new(422, "level: « all » or « critical »"))?;
    set_level(&config, &level).map_err(|e| HttpError::new(422, e))?;
    tracing::info!(target: "bipbridge::approval_level", level = %level, "approval level set");
    let c = config.clone();
    let _ = tokio::task::spawn_blocking(move || render_souls(&c, &following(&c))).await;
    Ok(web::ok(view(&config)))
}

async fn put_agent_level(State(state): State<AppState>, UrlPath(agent): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&agent))?;
    let level = level_of_body(&body)?;
    set_agent_level(&config, &agent.name, level.as_deref()).map_err(|e| HttpError::new(422, e))?;
    tracing::info!(target: "bipbridge::approval_level", agent = %agent.name, level = ?level, "agent's approval level set");
    let c = config.clone();
    let _ = tokio::task::spawn_blocking(move || render_souls(&c, &[agent.name])).await;
    Ok(web::ok(view(&config)))
}

async fn get_auto(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let limit = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100usize).clamp(1, 500);
    let agent = q.get("agent").filter(|a| !a.is_empty());
    let items: Vec<Value> = lines(&config, agent, limit).iter().map(line_view).collect();
    Ok(web::ok(json!({"items": items})))
}

async fn post_undo(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    // a refusal's `code` at the top level ({code, detail}: what the apps read)
    let refused = |code: &str, detail: &str| Ok(web::json_response(409, &json!({"code": code, "detail": detail})));
    match undo(&state, &id).await {
        Ok(line) => Ok(web::ok(json!({"ok": true, "item": line_view(&line)}))),
        Err(UndoRefused::NotFound) => Err(HttpError::not_found()),
        Err(UndoRefused::NotUndoable) => refused("not_undoable", "this one can't be undone from here"),
        Err(UndoRefused::Undone) => refused("undone", "undone already"),
        Err(UndoRefused::Failed(why)) => refused("not_undoable", &why),
    }
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/approval-level", get(get_level).put(put_level))
        .route("/agents/{agent}/approval-level", put(put_agent_level))
        .route("/approvals/auto", get(get_auto))
        .route("/approvals/auto/{id}/undo", post(post_undo))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_in(dir: &tempfile::TempDir) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config
    }

    fn item(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn the_table_is_one_list_with_every_kind_once_and_the_critical_rule() {
        let mut kinds: Vec<&str> = TABLE.iter().map(|r| r.kind).collect();
        kinds.sort();
        let n = kinds.len();
        kinds.dedup();
        assert_eq!(kinds.len(), n, "a kind twice");
        // what Alex named critical
        for kind in ["payment", "secret", "vault_login", "held_mail", "family_send", "mail_trash", "files_delete",
                     "connector", "meet", "command", "phone_exit", "task_more"] {
            assert!(rule_of(kind).critical, "{kind}");
        }
        for kind in ["mail_archive", "mail_move", "mail_mark_read", "mail_rule", "routine_way", "task_start", "files_write"] {
            assert!(!rule_of(kind).critical, "{kind}");
        }
        assert!(rule_of("never heard of").critical);
        // an undo only where one exists
        assert!(rule_of("mail_archive").undo && rule_of("task_start").undo && !rule_of("routine_way").undo);
    }

    #[test]
    fn cards_are_classified_by_their_run_id_and_action() {
        let mail = |a: &str| classify_card(&item(json!({"run_id": "bip-mail-1", "bip_mail": {"action": a}})));
        assert_eq!((mail("archive"), mail("mark_read"), mail("move:Factures"), mail("trash"), mail("purge")),
                   ("mail_archive", "mail_mark_read", "mail_move", "mail_trash", "mail_trash"));
        let rule = |a: &str| classify_card(&item(json!({"run_id": "bip-mailrule-1", "bip_mail_rule": {"action": a}})));
        assert_eq!((rule("move:Serveurs"), rule("archive+mark_read"), rule("mark_read"), rule("trash"), rule("trash+mark_read")),
                   ("mail_rule", "mail_rule", "mail_rule", "mail_rule_trash", "mail_rule_trash"));
        let kind = |r: &str| classify_card(&item(json!({"run_id": r})));
        assert_eq!(kind("bip-rout-x"), "routine_way");
        assert_eq!(kind("bip-site-x"), "site_send");
        assert_eq!(kind("bip-secret-x"), "secret");
        assert_eq!(kind("bip-login-x"), "vault_login");
        assert_eq!(kind("bip-conn-x"), "connector");
        assert_eq!(kind("bip-trig-x"), "trigger");
        assert_eq!(kind("bip-exit-x"), "phone_exit");
        assert_eq!(kind("bip-space-x"), "space");
        assert_eq!(kind("bip-new-x"), "unknown");
    }

    #[test]
    fn hermes_approvals_are_classified_and_only_writes_in_ones_own_service_go_alone() {
        let gate = |tool: &str, server: &str| {
            classify_hermes(&data(json!({"command": format!("MCP tool '{tool}' on UNTRUSTED server '{server}' wants to run"),
                                          "pattern_key": "mcp_elicitation"})))
        };
        assert_eq!(gate("create_page", "notion"), "service_write");
        assert_eq!(gate("update_block", "notion"), "service_write");
        assert_eq!(gate("delete_page", "notion"), "service_send");
        assert_eq!(gate("send_message", "slack"), "service_send");
        assert_eq!(gate("create_event", "google_calendar"), "service_send"); // invitations
        assert_eq!(gate("search", "notion"), "service_send"); // not recognised: asked
        assert_eq!(gate("shared_files_delete", "bipagents_files"), "files_delete");
        assert_eq!(gate("shared_files_write", "bipagents_files"), "files_write");
        assert_eq!(gate("pay_with_card", "bipagents_pay"), "payment");
        let h = |v: Value| classify_hermes(&data(v));
        assert_eq!(h(json!({"command": "Card payment pay_123 34,90 €", "pattern_key": "mcp_elicitation"})), "payment");
        assert_eq!(h(json!({"command": "Allow import?", "pattern_key": "mcp_elicitation"})), "elicitation");
        assert_eq!(h(json!({"command": "rm -rf /tmp/x", "pattern_key": "recursive delete"})), "command");
        assert_eq!(h(json!({"command": "execute_code <<'PY'\nprint(1)\nPY", "pattern_key": "execute_code"})), "command");
        assert_eq!(h(json!({"command": "SOUL.md", "pattern_key": "protected_instruction_file"})), "own_instructions");
        assert_eq!(h(json!({"command": "click", "pattern_key": "cua:click"})), "computer_use");
        assert_eq!(h(json!({"command": "browser_type (plugin approval rule)"})), "plugin_rule");
    }

    #[test]
    fn levels_default_to_everything_and_an_agent_may_differ() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        assert_eq!((person_level(&config), level_for(&config, "vie")), (ALL, ALL));
        assert_eq!(view(&config)["chosen"], false);
        assert!(!auto_for(&config, "vie", "mail_archive"));
        set_level(&config, "critical").unwrap();
        assert_eq!(view(&config)["chosen"], true);
        assert!(auto_for(&config, "vie", "mail_archive") && !auto_for(&config, "vie", "mail_trash"));
        assert!(!auto_for(&config, "vie", "payment") && !auto_for(&config, "vie", "unknown"));
        set_agent_level(&config, "coach", Some("all")).unwrap();
        assert!(!auto_for(&config, "coach", "mail_archive") && auto_for(&config, "vie", "mail_archive"));
        set_agent_level(&config, "coach", None).unwrap();
        assert!(auto_for(&config, "coach", "mail_archive"));
        // back to « Tout valider »: at once
        set_level(&config, "all").unwrap();
        assert!(!auto_for(&config, "vie", "mail_archive"));
        assert!(set_level(&config, "most").is_err());
    }

    #[test]
    fn the_agents_rule_is_in_the_personality_only_under_critical() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let soul = "Tu es Vie.\n";
        assert_eq!(with_block(&config, "vie", soul), soul);
        set_level(&config, "critical").unwrap();
        let with = with_block(&config, "vie", soul);
        assert!(with.contains(RULE) && with.starts_with("Tu es Vie."));
        assert_eq!(with_block(&config, "vie", &with), with, "once");
        set_agent_level(&config, "vie", Some("all")).unwrap();
        assert_eq!(with_block(&config, "vie", &with).trim_end(), "Tu es Vie.");
        assert_eq!(split_block(&with).0.trim_end(), "Tu es Vie.");
    }

    #[test]
    fn lines_are_kept_newest_first_and_summed_up_once_a_day() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        record(&config, Line { agent: "vie".into(), kind: "mail_archive".into(), summary: "A".into(),
                               undo: Some(Undo::MailJob { job: "j".into() }), ..Line::default() });
        std::thread::sleep(std::time::Duration::from_millis(5));
        record(&config, Line { agent: "vie".into(), kind: "routine_way".into(), summary: "B".into(), ..Line::default() });
        record(&config, Line { agent: "coach".into(), kind: "task_start".into(), summary: "C".into(), ..Line::default() });
        let mine = lines(&config, Some("vie"), 10);
        assert_eq!(mine.iter().map(|l| l.summary.as_str()).collect::<Vec<_>>(), ["B", "A"]);
        let v = line_view(&mine[1]);
        assert_eq!((v["undo"].as_bool(), v["undone"].as_bool(), v["kind"].as_str()), (Some(true), Some(false), Some("mail_archive")));
        assert_eq!(line_view(&mine[0])["undo"], false);
        use chrono::TimeZone;
        let morning = chrono::Local.with_ymd_and_hms(2026, 11, 6, 9, 0, 0).unwrap();
        let evening = chrono::Local.with_ymd_and_hms(2026, 11, 6, 19, 30, 0).unwrap();
        assert!(take_digest(&config, "vie", "fr", morning).is_none());
        let (id, title, text) = take_digest(&config, "vie", "fr", evening).unwrap();
        assert_eq!((id.as_str(), title.as_str()), ("auto-digest:vie:2026-11-06", "Validé pour toi"));
        assert!(text.contains("2 choses") && text.contains("- A") && text.contains("- B"), "{text}");
        assert!(take_digest(&config, "vie", "fr", evening).is_none(), "once a day");
        assert!(take_digest(&config, "coach", "en", evening).unwrap().2.contains("Today, 1 thing went"));
        let next = chrono::Local.with_ymd_and_hms(2026, 11, 7, 20, 0, 0).unwrap();
        assert!(take_digest(&config, "vie", "fr", next).is_none(), "nothing new");
    }

    #[test]
    fn a_routines_question_is_approved_with_its_suggestion() {
        let it = |s: Value| approving_choice(&item(json!({"bip_routine": {"suggested": s}})), "routine_way");
        assert_eq!((it(json!("silent")), it(json!("notify")), it(Value::Null)), ("silent".into(), "notify".into(), "notify".into()));
        assert_eq!(approving_choice(&item(json!({})), "mail_move"), "once");
        assert_eq!(undo_of(&item(json!({"request_id": "j1"})), "mail_move"), Some(Undo::MailJob { job: "j1".into() }));
        assert_eq!(undo_of(&item(json!({"request_id": "r1"})), "mail_rule"), Some(Undo::MailRule { rule: "r1".into() }));
        assert_eq!(undo_of(&item(json!({"request_id": "x"})), "routine_way"), None);
    }

    #[test]
    fn only_tools_that_write_in_ones_own_service_go_alone() {
        assert!(writes_only("notion_create_page") && writes_only("append_block_children") && writes_only("update_task"));
        assert!(!writes_only("send_email") && !writes_only("delete_page") && !writes_only("invite_member"));
        assert!(!writes_only("get_page") && !writes_only("create_payment"));
    }
}
