//! Standing mail rules (Alex 2026-11-03: his company mailbox fills up with a mail at every server created; he wants it
//! sorted with nothing to do, and the trail kept: filed, never deleted). A rule is a mail_bulk action « from now on »:
//!
//! - The agent proposes one with `mail_bulk(action, from | domain | list_id | groups domain:/list:, rule: true,
//!   mark_read?)` — move:<folder>, archive, trash or mark_read, optionally also marked read. Only a sender, a domain or
//!   a list chooses (never the subject or the text: mail content; never an age or the whole folder; never message ids).
//!   It makes ONE card (`bip-mailrule-<id>`, the generic card): the rule in a plain sentence and how many mails already
//!   there it matches. Trash only when the action said so (the card says « à la corbeille » in words).
//! - On « Oui » the bridge files the existing matches (a thread, resumed after a restart), then every new matching mail
//!   as it arrives: triggers_mail.rs watches the mailbox by IMAP IDLE (or a short poll) and hands each new message's
//!   headers to `sort_new`, which acts on a second connection — no model, no card. A missing folder is created.
//!   One rule per mail (the oldest rule first); a Message-ID a rule already filed is never filed again (put back by
//!   the undo or by the person: it stays where they put it).
//! - Never: starred/flagged mails, drafts, the person's own mails, Sent/Drafts as source or destination, a permanent
//!   delete, a send.
//! - Per mailbox, listed on the agent's page (`GET /v1/mail/rules?agent=`), each with its counters (filed N times, the
//!   last at), « Arrêter » (`DELETE /v1/mail/rules/{id}`), the undo of its last run (`POST /v1/mail/rules/{id}/undo`).
//!   The agent also has `mail_bulk(action: rules | stop_rule | undo_rule, rule_id)`.
//! - Once a week, a line in the agent's Boîte (no push): « Rangé cette semaine : 14 mails de hetzner.com → Serveurs,
//!   6 mails de stripe.com → Factures. »
//!
//!     data_dir/mail/rules.json (0600)   the cards, the rules, what they filed (35 days), the digests sent
//!
//! Not a Python module: Rust bridge only (owner group: tools-knowledge).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::mail_tidy::{self, Action, Called, Folders, Row, Selector};
use crate::mailbox::{self, Connect, Connection, MailError};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

pub const RUN_PREFIX: &str = "bip-mailrule-";
/// The `mail_bulk` actions handled here.
pub const WORDS: [&str; 3] = ["rules", "stop_rule", "undo_rule"];
pub const MAX_PER_AGENT: usize = 30;
pub const CARD_DAYS: f64 = 7.0;
/// What the rules filed, kept for the weekly line.
pub const LOG_DAYS: f64 = 35.0;
/// Message-IDs a rule remembers having filed (never twice).
pub const FILED_IDS: usize = 2000;
/// The weekly line: from Monday this hour (local time).
pub const DIGEST_HOUR: u32 = 8;
const SAMPLE: usize = 10;
const DAY: f64 = 86400.0;

// -- what a rule chooses ----------------------------------------------------------------------------------------------

/// A sender, a domain or a list: what the sender sets, never what the mail says.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Match {
    pub from: String,
    pub domain: String,
    pub list_id: String,
}

impl Match {
    /// The rule's part of a mail_bulk selector. Err: plain words for the agent.
    pub fn from_selector(s: &Selector) -> Result<Match, String> {
        if !s.subject_contains.is_empty() {
            return Err("a rule never chooses by the subject or the text (mail content): name a sender (from), a \
domain or a list (list_id)"
                .into());
        }
        if s.older_than_days.is_some() || s.unread_only || s.everything {
            return Err("a rule takes a sender (from), a domain or a list (list_id) only: no age, no unread_only, \
never the whole folder"
                .into());
        }
        let mut m = Match {
            from: s.from.trim().to_string(),
            domain: s.domain.trim().trim_start_matches('@').to_string(),
            list_id: s.list_id.trim().to_string(),
        };
        for g in &s.groups {
            match g.split_once(':') {
                Some(("domain", d)) if m.domain.is_empty() => m.domain = d.to_string(),
                Some(("list", l)) if m.list_id.is_empty() => m.list_id = l.to_string(),
                _ => {
                    return Err(format!(
                        "{g}: a rule takes one domain:… or list:… group (promo and newsletters are guesses, not a \
sender)"
                    ))
                }
            }
        }
        if m.from.is_empty() && m.domain.is_empty() && m.list_id.is_empty() {
            return Err("name a sender (from), a domain or a list (list_id)".into());
        }
        if !m.from.is_empty() && (!m.from.contains('@') || m.from.contains(char::is_whitespace)) {
            return Err("from: a whole address (billing@shop.example); for all of a sender's addresses, its domain".into());
        }
        if !m.domain.is_empty() && (!m.domain.contains('.') || m.domain.contains(['@', ' ', '/'])) {
            return Err("domain: like hetzner.com".into());
        }
        Ok(m)
    }

    fn selector(&self) -> Selector {
        Selector {
            from: self.from.clone(),
            domain: self.domain.clone(),
            list_id: self.list_id.clone(),
            ..Selector::default()
        }
    }

    pub fn matches(&self, row: &Row) -> bool {
        self.selector().matches(row, "")
    }

    /// What it is shown as: the list, else the address, else the domain.
    pub fn label(&self) -> String {
        [&self.list_id, &self.from, &self.domain].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default()
    }
}

// -- the store --------------------------------------------------------------------------------------------------------

/// One run of a rule: what it moved and marked (by Message-ID, for the undo).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub at: f64,
    pub count: usize,
    pub moved_ids: Vec<String>,
    pub marked_ids: Vec<String>,
    pub undone: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub agent: String,
    pub account: String,
    pub mailbox: String,
    /// Where it looks (the inbox).
    pub folder: String,
    pub matcher: Match,
    /// Archive, Move or Trash; None: only marked read.
    pub action: Option<Action>,
    pub mark_read: bool,
    /// Where moved mail goes (resolved when proposed).
    pub dest: String,
    /// The card's: the mails already there it matches, a few of them.
    pub existing: usize,
    pub sample: Vec<String>,
    /// asked (a card), on
    pub status: String,
    /// The card made, then the rule turned on.
    pub at: f64,
    /// The mails already there filed (after « Oui »).
    pub existing_done: bool,
    pub applied: u64,
    pub runs: u64,
    pub last_at: f64,
    pub last: Option<Run>,
    pub filed_ids: Vec<String>,
    pub error: String,
}

/// What a rule filed, for the weekly line.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Filed {
    pub rule: String,
    pub agent: String,
    pub label: String,
    /// move, archive, trash, read
    pub kind: String,
    pub folder: String,
    pub at: f64,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Store {
    cards: Vec<Rule>,
    rules: Vec<Rule>,
    filed: Vec<Filed>,
    /// agent → the ISO week of its last weekly line, and when.
    digests: BTreeMap<String, (String, f64)>,
}

static LOCK: Mutex<()> = Mutex::new(());
static ACTIVE: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("mail").join("rules.json")
}

fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    let now = py::time_time();
    store.cards.retain(|c| now - c.at < CARD_DAYS * DAY);
    store.filed.retain(|f| now - f.at < LOG_DAYS * DAY);
    let file = store_path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(&store).map_err(std::io::Error::other)?;
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)?;
    Ok(out)
}

/// The rules on, for one agent or all.
pub fn rules_of(config: &Config, agent: Option<&str>) -> Vec<Rule> {
    load(config).rules.into_iter().filter(|r| agent.is_none_or(|a| a == r.agent)).collect()
}

pub fn cards_of(config: &Config, agent: Option<&str>) -> Vec<Rule> {
    load(config).cards.into_iter().filter(|c| agent.is_none_or(|a| a == c.agent)).collect()
}

/// Whether some rule watches this mailbox.
pub fn has_rules(config: &Config, account: &str) -> bool {
    load(config).rules.iter().any(|r| r.account == account)
}

/// (mailbox, agent) of every rule whose agent still exists (triggers_mail.rs: the mailboxes to watch).
pub fn watched(config: &Config) -> Vec<(String, String)> {
    load(config)
        .rules
        .into_iter()
        .filter(|r| config.agent(Some(&r.agent)).is_some())
        .map(|r| (r.account, r.agent))
        .collect()
}

// -- the words --------------------------------------------------------------------------------------------------------

fn who(lang: &str, m: &Match) -> String {
    if !m.list_id.is_empty() {
        let list = crate::approval_text::pick(lang, "la liste", "the list", "der Liste", "la lista");
        return format!("{list} {}", m.list_id);
    }
    m.label()
}

/// « Les mails de hetzner.com vont dans « Serveurs », marqués comme lus » (`capital` false: « les mails … »).
pub fn line(lang: &str, rule: &Rule, capital: bool) -> String {
    let w = who(lang, &rule.matcher);
    let mr = rule.mark_read;
    let text = match lang {
        "fr" => {
            let what = match &rule.action {
                Some(Action::Move(f)) if mr => format!("vont dans « {f} », marqués comme lus"),
                Some(Action::Move(f)) => format!("vont dans « {f} »"),
                Some(Action::Archive) if mr => "sont archivés et marqués comme lus".into(),
                Some(Action::Archive) => "sont archivés".into(),
                Some(Action::Trash) => "vont à la corbeille".into(),
                _ => "sont marqués comme lus".into(),
            };
            format!("les mails de {w} {what}")
        }
        "de" => {
            let what = match &rule.action {
                Some(Action::Move(f)) if mr => format!("kommen in „{f}“, als gelesen markiert"),
                Some(Action::Move(f)) => format!("kommen in „{f}“"),
                Some(Action::Archive) if mr => "werden archiviert und als gelesen markiert".into(),
                Some(Action::Archive) => "werden archiviert".into(),
                Some(Action::Trash) => "kommen in den Papierkorb".into(),
                _ => "werden als gelesen markiert".into(),
            };
            format!("die Mails von {w} {what}")
        }
        "es" => {
            let what = match &rule.action {
                Some(Action::Move(f)) if mr => format!("van a «{f}», marcados como leídos"),
                Some(Action::Move(f)) => format!("van a «{f}»"),
                Some(Action::Archive) if mr => "se archivan y se marcan como leídos".into(),
                Some(Action::Archive) => "se archivan".into(),
                Some(Action::Trash) => "van a la papelera".into(),
                _ => "se marcan como leídos".into(),
            };
            format!("los correos de {w} {what}")
        }
        _ => {
            let what = match &rule.action {
                Some(Action::Move(f)) if mr => format!("go to “{f}”, marked as read"),
                Some(Action::Move(f)) => format!("go to “{f}”"),
                Some(Action::Archive) if mr => "are archived and marked as read".into(),
                Some(Action::Archive) => "are archived".into(),
                Some(Action::Trash) => "go to the trash".into(),
                _ => "are marked as read".into(),
            };
            format!("e-mails from {w} {what}")
        }
    };
    if capital {
        let mut chars = text.chars();
        chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
    } else {
        text
    }
}

/// The card's sentence: « Coach propose une règle : désormais, les mails de hetzner.com vont dans « Serveurs » — 14
/// déjà là ».
pub fn sentence(lang: &str, agent: &str, rule: &Rule) -> String {
    let start = match lang {
        "fr" => format!("{agent} propose une règle : désormais, "),
        "de" => format!("{agent} schlägt eine Regel vor: ab jetzt "),
        "es" => format!("{agent} propone una regla: a partir de ahora, "),
        _ => format!("{agent} suggests a rule: from now on, "),
    };
    let mut text = start + &line(lang, rule, false);
    if rule.existing > 0 {
        let n = rule.existing;
        text.push_str(&match lang {
            "fr" => format!(" — {n} déjà là"),
            "de" => format!(" — {n} schon da"),
            "es" => format!(" — {n} ya están"),
            _ => format!(" — {n} already there"),
        });
    }
    text
}

fn detail(lang: &str, rule: &Rule) -> String {
    let mut lines = vec![format!("{} › {}", rule.mailbox, rule.folder)];
    if rule.action == Some(Action::Trash) {
        lines.push(
            crate::approval_text::pick(
                lang,
                "La corbeille de ta messagerie les garde un temps.",
                "Your mailbox's trash keeps them for a while.",
                "Der Papierkorb deines Postfachs behält sie eine Weile.",
                "La papelera de tu correo los guarda un tiempo.",
            )
            .to_string(),
        );
    }
    lines.extend(rule.sample.iter().cloned());
    if rule.existing > rule.sample.len() {
        lines.push(format!("… +{}", rule.existing - rule.sample.len()));
    }
    lines.join("\n")
}

fn action_word(rule: &Rule) -> String {
    let mut word = rule.action.as_ref().map(Action::word).unwrap_or_else(|| "mark_read".into());
    if rule.mark_read && rule.action.is_some() {
        word.push_str("+mark_read");
    }
    word
}

/// A card in Hermes' approvals' shape (`GET /v1/approvals`).
pub fn item_of(config: &Config, card: &Rule) -> Dict {
    let lang = mail_tidy::language(config, &card.agent);
    let summary = sentence(&lang, &mail_tidy::shown_agent(config, &card.agent), card);
    let item = json!({
        "agent": card.agent,
        "run_id": format!("{RUN_PREFIX}{}", card.id),
        "request_id": card.id,
        "command": format!("mail_rule {} · {} · {} › {}", action_word(card), card.matcher.label(), card.mailbox,
            card.folder),
        "description": summary,
        "choices": ["once", "deny"],
        "created_at": card.at,
        "session_id": null,
        "bip_summary": summary,
        "bip_detail": detail(&lang, card),
        "bip_mail_rule": {
            "action": action_word(card), "match": card.matcher.label(), "existing": card.existing,
            "mailbox": card.mailbox, "folder": card.folder, "sample": card.sample,
        },
    });
    item.as_object().cloned().unwrap_or_default()
}

pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    cards_of(config, agent).iter().map(|c| item_of(config, c)).collect()
}

/// The app's row: one plain line, the counters, what can be done.
pub fn view(config: &Config, rule: &Rule) -> Value {
    let lang = mail_tidy::language(config, &rule.agent);
    let undo = rule.last.as_ref().filter(|r| !r.undone && r.count > 0);
    json!({
        "id": rule.id, "agent": rule.agent, "account": rule.account, "mailbox": rule.mailbox, "folder": rule.folder,
        "line": line(&lang, rule, true), "action": action_word(rule), "dest": rule.dest,
        "from": rule.matcher.from, "domain": rule.matcher.domain, "list_id": rule.matcher.list_id,
        "applied": rule.applied, "runs": rule.runs, "last_at": if rule.last_at > 0.0 { json!(rule.last_at) } else { Value::Null },
        "created_at": rule.at, "can_undo": undo.is_some(), "last_count": undo.map(|r| r.count).unwrap_or(0),
        "error": if rule.error.is_empty() { Value::Null } else { json!(rule.error) },
    })
}

fn status_line(rule: &Rule) -> String {
    let last = if rule.last_at > 0.0 {
        chrono::DateTime::from_timestamp(rule.last_at as i64, 0)
            .map(|d| d.with_timezone(&chrono::Local).format(" · last %Y-%m-%d %H:%M").to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    format!(
        "Rule {} · {} · {} · {} filed{last}",
        rule.id,
        rule.mailbox,
        line("en", rule, true),
        rule.applied
    )
}

/// mail_bulk status's lines for the rules and their cards.
pub fn status_lines(config: &Config, agent: &str) -> Vec<String> {
    let store = load(config);
    let mut out: Vec<String> = store
        .cards
        .iter()
        .filter(|c| c.agent == agent)
        .map(|c| format!("Rule card {} · {} · waiting for the person", c.id, line("en", c, true)))
        .collect();
    out.extend(store.rules.iter().filter(|r| r.agent == agent).map(status_line));
    out
}

// -- the card ---------------------------------------------------------------------------------------------------------

/// A rule's card from the folder's scan. Err: plain words for the agent.
#[allow(clippy::too_many_arguments)]
pub fn propose(
    config: &Config,
    agent: &str,
    account: &Dict,
    scan: &mail_tidy::Scan,
    folders: &Folders,
    action: Option<Action>,
    mark_read: bool,
    matcher: Match,
    own: &[String],
) -> Result<Rule, String> {
    if folders.protected(&scan.folder) {
        return Err(format!("{} is the person's sent mail or drafts: never touched", scan.folder));
    }
    let dest = match &action {
        None => String::new(),
        Some(Action::Archive) => folders.archive().0,
        Some(Action::Trash) => match folders.trash() {
            Some(t) => t,
            None => return Err("this mailbox shows no trash folder: file them in a folder instead".into()),
        },
        Some(Action::Move(f)) => {
            if folders.protected(f) {
                return Err(format!("{f} is the sent mail or drafts: not a destination"));
            }
            f.clone()
        }
        Some(_) => return Err("action: move:<folder>, archive, mark_read or trash".into()),
    };
    if action.is_none() && !mark_read {
        return Err("a rule needs an action".into());
    }
    if !dest.is_empty() && dest == scan.folder {
        return Err(format!("the mails are already in {dest}"));
    }
    let account_id = mailbox::field(account, "id");
    let store = load(config);
    let same = |r: &&Rule| r.account == account_id && r.folder == scan.folder && r.matcher == matcher;
    if let Some(r) = store.rules.iter().find(same) {
        return Err(format!("there is already a rule for {} (rule {}): stop it first to change it", matcher.label(), r.id));
    }
    if store.rules.iter().filter(|r| r.agent == agent).count() >= MAX_PER_AGENT {
        return Err(format!("at most {MAX_PER_AGENT} rules: stop one first"));
    }
    let mut chosen: Vec<&Row> = scan
        .rows
        .iter()
        .filter(|r| !mail_tidy::protected(r, own) && matcher.matches(r))
        .filter(|r| action.is_some() || !r.seen)
        .collect();
    chosen.sort_by_key(|r| std::cmp::Reverse(r.uid));
    let sample =
        chosen.iter().take(SAMPLE).map(|r| format!("{} · {} · {}", r.date, r.from, tk::head(&r.subject, 70))).collect();
    Ok(Rule {
        id: py::token_hex(6),
        agent: agent.to_string(),
        account: account_id,
        mailbox: mailbox::shown_name(account),
        folder: scan.folder.clone(),
        matcher,
        action,
        mark_read,
        dest,
        existing: chosen.len(),
        sample,
        status: "asked".into(),
        at: py::time_time(),
        ..Rule::default()
    })
}

/// A card answered: refused (gone) or approved (the rule on; its existing matches to file with `start`). None: no card.
pub fn answer(config: &Config, agent: &str, id: &str, allow: bool, via: &str) -> anyhow::Result<Option<Rule>> {
    let card = change(config, |s| {
        let at = s.cards.iter().position(|c| c.id == id && c.agent == agent)?;
        let mut card = s.cards.remove(at);
        if allow {
            card.status = "on".into();
            card.at = py::time_time();
            card.existing_done = card.existing == 0;
            s.rules.push(card.clone());
        } else {
            card.status = "refused".into();
        }
        Some(card)
    })?;
    let Some(card) = card else { return Ok(None) };
    let item = item_of(config, &card);
    let mut fields = serde_json::Map::new();
    for k in ["run_id", "request_id", "command", "description", "bip_summary"] {
        if let Some(v) = item.get(k) {
            fields.insert(k.into(), v.clone());
        }
    }
    fields.insert("topic".into(), json!("mail"));
    fields.insert("choice".into(), json!(if allow { "once" } else { "deny" }));
    fields.insert("via".into(), json!(via));
    if let Err(err) = crate::approvals_log::record(config, agent, &fields) {
        tracing::warn!(agent = %agent, error = %err, "mail rule decision not kept");
    }
    Ok(Some(card))
}

/// « Arrêter cette règle » (the person, or the agent for its own). What it filed stays filed.
pub fn stop(config: &Config, agent: Option<&str>, id: &str) -> std::io::Result<Option<Rule>> {
    change(config, |s| {
        let at = s.rules.iter().position(|r| r.id == id && agent.is_none_or(|a| a == r.agent))?;
        Some(s.rules.remove(at))
    })
}

// -- applying ---------------------------------------------------------------------------------------------------------

/// The rule's action on these messages of its folder (already chosen: matching, not the person's own), the flags
/// checked again just before. Opens the folder for writing; makes the destination when missing.
pub fn apply_rows(conn: &mut Connection, rule: &Rule, rows: &[Row]) -> anyhow::Result<Run> {
    let mut run = Run { at: py::time_time(), ..Run::default() };
    let filed: HashSet<&String> = rule.filed_ids.iter().collect();
    let rows: Vec<&Row> = rows.iter().filter(|r| r.message_id.is_empty() || !filed.contains(&r.message_id)).collect();
    if rows.is_empty() {
        return Ok(run);
    }
    if !rule.dest.is_empty() {
        let folders = Folders::of(conn)?;
        if !folders.exists(&rule.dest) {
            let _ = conn.create(&mailbox::quote(&rule.dest));
        }
    }
    let (status, _) = conn.select(&mailbox::quote(&rule.folder), false)?;
    if status != "OK" {
        return Err(MailError(format!("no folder {}", rule.folder)).into());
    }
    let ids: HashMap<u32, &Row> = rows.iter().map(|r| (r.uid, *r)).collect();
    let uids: Vec<u32> = rows.iter().map(|r| r.uid).collect();
    for batch in uids.chunks(mail_tidy::ACT_BATCH) {
        // flags again: starred since, or gone
        let (status, data) = conn.uid("FETCH", &[&mail_tidy::uid_set(batch), "(UID FLAGS)"])?;
        let now = if status == "OK" { mail_tidy::parse_fetch(&data) } else { Vec::new() };
        let keep: Vec<&Row> = now.iter().filter(|r| !r.flagged && !r.draft).filter_map(|r| ids.get(&r.uid).copied()).collect();
        if keep.is_empty() {
            continue;
        }
        let unseen: HashSet<u32> = now.iter().filter(|r| !r.seen).map(|r| r.uid).collect();
        if rule.mark_read {
            let to_mark: Vec<u32> = keep.iter().map(|r| r.uid).filter(|u| unseen.contains(u)).collect();
            if !to_mark.is_empty()
                && mail_tidy::ok(&conn.uid("STORE", &[&mail_tidy::uid_set(&to_mark), "+FLAGS.SILENT", "(\\Seen)"]))
            {
                run.marked_ids.extend(
                    keep.iter().filter(|r| to_mark.contains(&r.uid)).map(|r| r.message_id.clone()).filter(|m| !m.is_empty()),
                );
            }
        }
        let set = mail_tidy::uid_set(&keep.iter().map(|r| r.uid).collect::<Vec<_>>());
        match &rule.action {
            Some(action) => {
                if mail_tidy::act(conn, action, &set, &rule.dest)? {
                    run.count += keep.len();
                    run.moved_ids.extend(keep.iter().map(|r| r.message_id.clone()).filter(|m| !m.is_empty()));
                }
            }
            None => run.count += keep.iter().filter(|r| unseen.contains(&r.uid)).count(),
        }
    }
    Ok(run)
}

fn filed_kind(rule: &Rule) -> &'static str {
    match rule.action {
        Some(Action::Move(_)) => "move",
        Some(Action::Archive) => "archive",
        Some(Action::Trash) => "trash",
        _ => "read",
    }
}

/// A run kept: the counters, the trail for the weekly line, the Message-IDs never filed twice.
fn record(config: &Config, id: &str, run: Run, error: &str) -> std::io::Result<()> {
    change(config, |s| {
        let Some(rule) = s.rules.iter_mut().find(|r| r.id == id) else { return };
        rule.error = error.to_string();
        if run.count == 0 && run.moved_ids.is_empty() && run.marked_ids.is_empty() {
            return;
        }
        rule.applied += run.count as u64;
        rule.runs += 1;
        rule.last_at = run.at;
        let mut ids: Vec<String> = run.moved_ids.iter().chain(run.marked_ids.iter()).cloned().collect();
        ids.retain(|m| !rule.filed_ids.contains(m));
        rule.filed_ids.extend(ids);
        let over = rule.filed_ids.len().saturating_sub(FILED_IDS);
        rule.filed_ids.drain(..over);
        let filed = Filed {
            rule: rule.id.clone(),
            agent: rule.agent.clone(),
            label: rule.matcher.label(),
            kind: filed_kind(rule).into(),
            folder: rule.dest.clone(),
            at: run.at,
            count: run.count,
        };
        rule.last = Some(run);
        if filed.count > 0 {
            s.filed.push(filed);
        }
    })
}

fn account_of(config: &Config, rule: &Rule) -> Option<Dict> {
    let accounts = mailbox::of(config).of_agent(&rule.agent).ok()?;
    accounts.into_iter().find(|a| mailbox::field(a, "id") == rule.account)
}

/// Files the mails already there for a rule just turned on (its folder read again, headers only).
pub fn run_existing(config: &Config, id: &str, opener: &Connect) -> anyhow::Result<()> {
    let Some(rule) = load(config).rules.into_iter().find(|r| r.id == id) else { return Ok(()) };
    if rule.existing_done {
        return Ok(());
    }
    let Some(account) = account_of(config, &rule) else {
        record(config, id, Run::default(), "this mailbox is no longer connected to the agent")?;
        return Ok(());
    };
    let own = mail_tidy::own_addresses(&account);
    let mut conn = opener(&account)?;
    let result = (|| -> anyhow::Result<Run> {
        let scan = mail_tidy::scan(&mut conn, &rule.folder, Duration::from_secs(mail_tidy::SCAN_SECONDS))?;
        let rows: Vec<Row> = scan
            .rows
            .iter()
            .filter(|r| !mail_tidy::protected(r, &own) && rule.matcher.matches(r))
            .cloned()
            .collect();
        apply_rows(&mut conn, &rule, &rows)
    })();
    let _ = conn.logout();
    mail_tidy::forget_scans(&rule.account);
    let run = result?;
    tracing::info!(rule = %rule.id, agent = %rule.agent, count = run.count, "mail rule: existing mails filed");
    record(config, id, run, "")?;
    change(config, |s| {
        if let Some(r) = s.rules.iter_mut().find(|r| r.id == id) {
            r.existing_done = true;
        }
    })?;
    Ok(())
}

/// `run_existing` on a thread of its own (once at a time per rule).
pub fn start(config: &Config, id: &str, opener: Connect) {
    let config = config.clone();
    let id = id.to_string();
    if !ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone()) {
        return;
    }
    std::thread::spawn(move || {
        if let Err(err) = run_existing(&config, &id, &opener) {
            tracing::warn!(rule = %id, error = %format!("{err:#}"), "mail rule: existing mails not filed yet");
            let message = match err.downcast_ref::<MailError>() {
                Some(m) => m.0.clone(),
                None => "the mailbox didn't answer; it goes on later".into(),
            };
            let _ = record(&config, &id, Run::default(), &message);
        }
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    });
}

/// New messages of a mailbox's inbox (their headers, triggers_mail.rs) sorted by its rules: one rule per message, the
/// oldest rule first; one connection only when some rule matches. The number of messages filed.
pub fn sort_new(config: &Config, account_id: &str, rows: &[Row], opener: &Connect) -> usize {
    let rules: Vec<Rule> = load(config)
        .rules
        .into_iter()
        .filter(|r| r.account == account_id && r.folder.eq_ignore_ascii_case(crate::triggers_mail::FOLDER))
        .collect();
    if rules.is_empty() || rows.is_empty() {
        return 0;
    }
    let mut accounts: HashMap<String, Option<Dict>> = HashMap::new();
    let mut work: Vec<(Rule, Vec<Row>)> = Vec::new();
    let mut taken: HashSet<u32> = HashSet::new();
    for rule in rules {
        let account = accounts.entry(rule.agent.clone()).or_insert_with(|| account_of(config, &rule)).clone();
        let Some(account) = account else { continue };
        let own = mail_tidy::own_addresses(&account);
        let mine: Vec<Row> = rows
            .iter()
            .filter(|r| !taken.contains(&r.uid) && !mail_tidy::protected(r, &own) && rule.matcher.matches(r))
            .filter(|r| r.message_id.is_empty() || !rule.filed_ids.contains(&r.message_id))
            .cloned()
            .collect();
        if mine.is_empty() {
            continue;
        }
        taken.extend(mine.iter().map(|r| r.uid));
        work.push((rule, mine));
    }
    let Some(account) = work.first().and_then(|(r, _)| accounts.get(&r.agent).cloned().flatten()) else { return 0 };
    let mut conn = match opener(&account) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(account = %account_id, error = %e.0, "mail rules: mailbox not reached");
            return 0;
        }
    };
    let mut filed = 0;
    for (rule, mine) in work {
        match apply_rows(&mut conn, &rule, &mine) {
            Ok(run) => {
                filed += run.count;
                tracing::info!(rule = %rule.id, agent = %rule.agent, count = run.count, "mail rule: new mail filed");
                let _ = record(config, &rule.id, run, "");
            }
            Err(err) => {
                tracing::warn!(rule = %rule.id, error = %format!("{err:#}"), "mail rule: new mail not filed");
                let message = err.downcast_ref::<MailError>().map(|m| m.0.clone()).unwrap_or_default();
                let _ = record(config, &rule.id, Run::default(), &message);
            }
        }
    }
    let _ = conn.logout();
    mail_tidy::forget_scans(account_id);
    filed
}

/// The undo of a rule's last run: moved mails back in its folder, marked ones unread again. The number put back.
pub fn undo_last(config: &Config, agent: Option<&str>, id: &str, opener: &Connect) -> Result<usize, String> {
    let Some(rule) = load(config).rules.into_iter().find(|r| r.id == id && agent.is_none_or(|a| a == r.agent)) else {
        return Err("no such rule (mail_bulk action rules lists them)".into());
    };
    let Some(run) = rule.last.clone().filter(|r| !r.undone && r.count > 0) else {
        return Err("nothing to undo: its last run was undone already, or it filed nothing yet".into());
    };
    let Some(account) = account_of(config, &rule) else {
        return Err("this mailbox is no longer connected to the agent".into());
    };
    let mut conn = opener(&account).map_err(|e| e.0)?;
    let done = (|| -> anyhow::Result<usize> {
        let mut back = 0;
        if let Some(action) = rule.action.as_ref().filter(|_| !run.moved_ids.is_empty()) {
            let (status, _) = conn.select(&mailbox::quote(&rule.dest), false)?;
            if status == "OK" {
                let uids = mail_tidy::find_by_ids(&mut conn, &run.moved_ids)?;
                // Gmail's archive keeps everything in All Mail: copied back into the inbox
                let gmail_archive = *action == Action::Archive && rule.dest.contains("]/");
                let back_action =
                    if gmail_archive { Action::Copy(rule.folder.clone()) } else { Action::Move(rule.folder.clone()) };
                if !uids.is_empty() && mail_tidy::act(&mut conn, &back_action, &mail_tidy::uid_set(&uids), &rule.folder)? {
                    back = uids.len();
                }
            }
        }
        if !run.marked_ids.is_empty() {
            let (status, _) = conn.select(&mailbox::quote(&rule.folder), false)?;
            if status == "OK" {
                let uids = mail_tidy::find_by_ids(&mut conn, &run.marked_ids)?;
                if !uids.is_empty()
                    && mail_tidy::ok(&conn.uid("STORE", &[&mail_tidy::uid_set(&uids), "-FLAGS.SILENT", "(\\Seen)"]))
                {
                    back = back.max(uids.len());
                }
            }
        }
        Ok(back)
    })();
    let _ = conn.logout();
    mail_tidy::forget_scans(&rule.account);
    let back = done.map_err(|e| match e.downcast_ref::<MailError>() {
        Some(m) => m.0.clone(),
        None => "the mailbox didn't answer; try again later".into(),
    })?;
    change(config, |s| {
        if let Some(r) = s.rules.iter_mut().find(|r| r.id == id) {
            if let Some(last) = r.last.as_mut() {
                last.undone = true;
            }
        }
    })
    .map_err(|e| e.to_string())?;
    tracing::info!(rule = %id, back, "mail rule: last run undone");
    Ok(back)
}

/// The rules turned on whose existing mails a restart left unfiled, started again.
pub fn resume(config: &Config, opener: &Connect) -> usize {
    let left: Vec<String> = load(config)
        .rules
        .into_iter()
        .filter(|r| !r.existing_done && !ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).contains(&r.id))
        .map(|r| r.id)
        .collect();
    for id in &left {
        start(config, id, opener.clone());
    }
    left.len()
}

// -- the weekly line --------------------------------------------------------------------------------------------------

fn iso_week(now: &chrono::DateTime<chrono::Local>) -> String {
    let w = now.iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

fn dest_word(lang: &str, f: &Filed) -> String {
    match f.kind.as_str() {
        "move" => f.folder.clone(),
        "archive" => crate::approval_text::pick(lang, "archives", "archive", "Archiv", "archivo").into(),
        "trash" => crate::approval_text::pick(lang, "corbeille", "trash", "Papierkorb", "papelera").into(),
        _ => crate::approval_text::pick(lang, "lus", "read", "gelesen", "leídos").into(),
    }
}

/// « Rangé cette semaine : 14 mails de hetzner.com → Serveurs, 6 mails de stripe.com → Factures. » None: nothing filed.
pub fn digest_text(lang: &str, filed: &[Filed]) -> Option<String> {
    let mut sums: BTreeMap<(String, String), usize> = BTreeMap::new();
    for f in filed.iter().filter(|f| f.count > 0) {
        *sums.entry((f.label.clone(), dest_word(lang, f))).or_default() += f.count;
    }
    if sums.is_empty() {
        return None;
    }
    let mut rows: Vec<((String, String), usize)> = sums.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut parts: Vec<String> = rows
        .iter()
        .take(6)
        .map(|((label, dest), n)| match lang {
            "fr" => format!("{n} mails de {label} → {dest}"),
            "de" => format!("{n} Mails von {label} → {dest}"),
            "es" => format!("{n} correos de {label} → {dest}"),
            _ => format!("{n} e-mails from {label} → {dest}"),
        })
        .collect();
    if rows.len() > 6 {
        let more = rows.len() - 6;
        parts.push(match lang {
            "fr" => format!("+{more} autres"),
            "de" => format!("+{more} weitere"),
            "es" => format!("+{more} más"),
            _ => format!("+{more} more"),
        });
    }
    let start = crate::approval_text::pick(
        lang,
        "Rangé cette semaine : ",
        "Filed this week: ",
        "Diese Woche einsortiert: ",
        "Ordenado esta semana: ",
    );
    Some(format!("{start}{}.", parts.join(", ")))
}

pub fn digest_title(lang: &str) -> &'static str {
    crate::approval_text::pick(lang, "Courrier rangé", "Mail filed", "Post einsortiert", "Correo ordenado")
}

/// The agent's weekly line when it is due (once an ISO week, from Monday `DIGEST_HOUR`), marked sent: (id, text).
pub fn take_digest(config: &Config, agent: &str, lang: &str, now: chrono::DateTime<chrono::Local>) -> Option<(String, String)> {
    if now.weekday() == chrono::Weekday::Mon && now.hour() < DIGEST_HOUR {
        return None;
    }
    let week = iso_week(&now);
    let at = now.timestamp() as f64;
    change(config, |s| {
        let since = match s.digests.get(agent) {
            Some((w, _)) if *w == week => return None,
            Some((_, last)) => last.max(at - 7.0 * DAY),
            None => at - 7.0 * DAY,
        };
        let filed: Vec<Filed> = s.filed.iter().filter(|f| f.agent == agent && f.at > since && f.at <= at).cloned().collect();
        if filed.is_empty() && !s.rules.iter().any(|r| r.agent == agent) {
            return None; // nothing ever: no mark either
        }
        s.digests.insert(agent.to_string(), (week.clone(), at));
        digest_text(lang, &filed).map(|text| (format!("mail-rules-week:{agent}:{week}"), text))
    })
    .ok()
    .flatten()
}

/// Every few minutes: the existing mails a restart left, the weekly lines in the Boîtes (no push).
pub async fn run_forever(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let config = state.config();
        let c = config.clone();
        let due = tokio::task::spawn_blocking(move || {
            resume(&c, &mailbox::default_connect());
            let store = load(&c);
            let agents: HashSet<String> =
                store.rules.iter().map(|r| r.agent.clone()).chain(store.filed.iter().map(|f| f.agent.clone())).collect();
            let now = chrono::Local::now();
            agents
                .into_iter()
                .filter_map(|a| {
                    let agent = c.agent(Some(&a))?.clone();
                    let lang = crate::handback::language_of(&c, &agent);
                    take_digest(&c, &a, &lang, now).map(|(id, text)| (agent, lang, id, text))
                })
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        for (agent, lang, id, text) in due {
            let mut note = serde_json::Map::new();
            note.insert("message".into(), json!(text));
            note.insert("title".into(), json!(digest_title(&lang)));
            note.insert("id".into(), json!(id));
            if let Err(e) = state.outbox.ingest(&agent, &note, false, false).await {
                tracing::warn!(agent = %agent.name, error = %format!("{e:#}"), "mail rules' weekly line not stored");
            }
        }
    }
}

// -- the agent's side (mail_bulk) -------------------------------------------------------------------------------------

/// `mail_bulk` with `rule: true`, or `rules`, `stop_rule`, `undo_rule`.
pub fn call(
    config: &Config,
    agent: &str,
    word: &str,
    arguments: &Dict,
    account: &Dict,
    folder: &str,
    opener: &Connect,
) -> anyhow::Result<Called> {
    let said = mail_tidy::said;
    let rule_id = tk::strip(&tk::str_or_empty(arguments.get("rule_id"))).to_string();
    match word {
        "rules" => {
            let lines = status_lines(config, agent);
            let text = if lines.is_empty() { "No mail rule.".to_string() } else { lines.join("\n") };
            return Ok(said(&text, false));
        }
        "stop_rule" => {
            return Ok(match stop(config, Some(agent), &rule_id)? {
                Some(r) => said(&format!("Rule {} stopped: new mail stays where it lands. What it filed stays filed.", r.id), false),
                None => said("no such rule (action rules lists them)", true),
            });
        }
        "undo_rule" => {
            return Ok(match undo_last(config, Some(agent), &rule_id, opener) {
                Ok(n) => said(&format!("{n} e-mails put back as they were. The rule stays on for new mail."), false),
                Err(e) => said(&e, true),
            });
        }
        _ => {}
    }
    let parsed = match Action::parse(&tk::str_or_empty(arguments.get("action"))) {
        Ok(a) => a,
        Err(e) => return Ok(said(&e, true)),
    };
    let wants_read = arguments.get("mark_read").is_some_and(py::truthy);
    let (action, mark_read) = match parsed {
        Action::MarkRead => (None, true),
        other => (Some(other), wants_read),
    };
    let matcher = match Match::from_selector(&Selector::from_args(arguments)) {
        Ok(m) => m,
        Err(e) => return Ok(said(&e, true)),
    };
    let mut conn = opener(account)?;
    let made = (|| -> anyhow::Result<Result<Rule, String>> {
        let folders = Folders::of(&mut conn)?;
        if folders.protected(folder) {
            return Ok(Err(format!("{folder} is the person's sent mail or drafts: never touched")));
        }
        let scan = mail_tidy::cached(account, folder, &mut conn, false)?;
        let own = mail_tidy::own_addresses(account);
        Ok(propose(config, agent, account, &scan, &folders, action, mark_read, matcher, &own))
    })();
    let _ = conn.logout();
    let card = match made {
        Ok(Ok(card)) => card,
        Ok(Err(e)) => return Ok(said(&e, true)),
        Err(exc) => match exc.downcast_ref::<MailError>() {
            Some(m) => return Ok(said(&m.0, true)),
            None => return Err(exc),
        },
    };
    change(config, |s| s.cards.push(card.clone()))?;
    tracing::info!(agent = %agent, card = %card.id, existing = card.existing, "mail rule card");
    Ok(Called {
        result: tool_text(
            &format!(
                "Rule card {} made: {} ({} already there). It waits in the person's approvals; nothing changes before \
their Oui. Tell them in one sentence.",
                card.id,
                line("en", &card, true),
                card.existing
            ),
            false,
        ),
        card: None,
        rule: Some(card),
    })
}

// -- the app's routes -------------------------------------------------------------------------------------------------

async fn list_route(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let agent = web::query_str(&q, "agent", false, Some(200))?.filter(|a| !a.is_empty());
    let items: Vec<Value> = rules_of(&config, agent.as_deref())
        .iter()
        .filter(|r| config.agent(Some(&r.agent)).is_some())
        .map(|r| view(&config, r))
        .collect();
    Ok(web::ok(json!({"items": items})))
}

async fn stop_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let stopped = tokio::task::spawn_blocking(move || stop(&config, None, &id))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    match stopped {
        Some(r) => {
            tracing::info!(rule = %r.id, agent = %r.agent, "mail rule stopped");
            Ok(web::ok(json!({"ok": true})))
        }
        None => Err(HttpError::new(404, "no such rule")),
    }
}

async fn undo_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let c = config.clone();
    let done = tokio::task::spawn_blocking(move || undo_last(&c, None, &id, &mailbox::default_connect()))
        .await
        .map_err(anyhow::Error::from)?;
    match done {
        Ok(count) => Ok(web::ok(json!({"ok": true, "count": count}))),
        Err(why) => Err(HttpError::new(409, why)),
    }
}

/// The app's routes (under /v1, behind the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mail/rules", get(list_route))
        .route("/mail/rules/{id}", delete(stop_route))
        .route("/mail/rules/{id}/undo", post(undo_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    fn row(uid: u32, from: &str) -> Row {
        Row { uid, from: from.into(), subject: "x".into(), message_id: format!("<m{uid}@t>"), ..Row::default() }
    }

    #[test]
    fn a_rule_chooses_by_sender_domain_or_list_only() {
        let m = |v: Value| Match::from_selector(&Selector::from_args(&args(v)));
        assert!(m(json!({})).is_err());
        assert!(m(json!({"subject_contains": "facture"})).is_err(), "mail content");
        assert!(m(json!({"domain": "hetzner.com", "subject_contains": "server"})).is_err());
        assert!(m(json!({"domain": "hetzner.com", "older_than_days": 30})).is_err());
        assert!(m(json!({"everything": true})).is_err());
        assert!(m(json!({"groups": ["promo"]})).is_err(), "a guess, not a sender");
        assert!(m(json!({"from": "hetzner"})).is_err(), "a whole address");
        assert!(m(json!({"domain": "localhost"})).is_err());
        let got = m(json!({"groups": ["domain:hetzner.com"]})).unwrap();
        assert_eq!(got.domain, "hetzner.com");
        let list = m(json!({"list_id": "<news.shop.example>"})).unwrap();
        assert_eq!(list.list_id, "news.shop.example");
        assert!(got.matches(&row(1, "noreply@mail.hetzner.com")));
        assert!(!got.matches(&row(2, "x@nothetzner.com")));
        assert!(!got.matches(&row(3, "a@hetzner.com.evil.example")));
    }

    #[test]
    fn plain_lines_in_each_language() {
        let rule = Rule {
            matcher: Match { domain: "hetzner.com".into(), ..Match::default() },
            action: Some(Action::Move("Serveurs".into())),
            mark_read: true,
            existing: 14,
            ..Rule::default()
        };
        assert_eq!(line("fr", &rule, true), "Les mails de hetzner.com vont dans « Serveurs », marqués comme lus");
        assert_eq!(
            sentence("fr", "Coach", &rule),
            "Coach propose une règle : désormais, les mails de hetzner.com vont dans « Serveurs », marqués comme lus — 14 déjà là"
        );
        assert_eq!(line("en", &rule, true), "E-mails from hetzner.com go to “Serveurs”, marked as read");
        let trash = Rule { action: Some(Action::Trash), mark_read: false, ..rule.clone() };
        assert!(sentence("fr", "Coach", &trash).contains("vont à la corbeille"));
        assert!(detail("fr", &trash).contains("corbeille"));
        let read = Rule { action: None, matcher: Match { list_id: "news.shop.example".into(), ..Match::default() }, ..rule };
        assert_eq!(line("fr", &read, true), "Les mails de la liste news.shop.example sont marqués comme lus");
        assert_eq!(action_word(&read), "mark_read");
    }

    #[test]
    fn the_weekly_line_sums_by_sender_and_folder() {
        let f = |label: &str, kind: &str, folder: &str, count: usize| Filed {
            label: label.into(),
            kind: kind.into(),
            folder: folder.into(),
            count,
            ..Filed::default()
        };
        let filed = vec![
            f("hetzner.com", "move", "Serveurs", 10),
            f("stripe.com", "move", "Factures", 6),
            f("hetzner.com", "move", "Serveurs", 4),
            f("news.shop.example", "trash", "Trash", 0),
        ];
        assert_eq!(
            digest_text("fr", &filed).unwrap(),
            "Rangé cette semaine : 14 mails de hetzner.com → Serveurs, 6 mails de stripe.com → Factures."
        );
        assert!(digest_text("fr", &[]).is_none());
        let many: Vec<Filed> = (0..8).map(|i| f(&format!("s{i}.example"), "archive", "", 8 - i)).collect();
        let text = digest_text("en", &many).unwrap();
        assert!(text.starts_with("Filed this week: 8 e-mails from s0.example → archive") && text.ends_with("+2 more."), "{text}");
    }

    #[test]
    fn the_weekly_line_once_a_week_from_monday_morning() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        use chrono::TimeZone;
        let monday_early = chrono::Local.with_ymd_and_hms(2026, 11, 9, 7, 0, 0).unwrap();
        let monday = chrono::Local.with_ymd_and_hms(2026, 11, 9, 9, 0, 0).unwrap();
        change(&config, |s| {
            s.rules.push(Rule { id: "r1".into(), agent: "coach".into(), ..Rule::default() });
            s.filed.push(Filed {
                rule: "r1".into(),
                agent: "coach".into(),
                label: "hetzner.com".into(),
                kind: "move".into(),
                folder: "Serveurs".into(),
                at: (monday.timestamp() - 3600) as f64,
                count: 3,
            });
        })
        .unwrap();
        assert!(take_digest(&config, "coach", "fr", monday_early).is_none());
        let (id, text) = take_digest(&config, "coach", "fr", monday).unwrap();
        assert_eq!(id, "mail-rules-week:coach:2026-W46");
        assert_eq!(text, "Rangé cette semaine : 3 mails de hetzner.com → Serveurs.");
        assert!(take_digest(&config, "coach", "fr", monday + chrono::Duration::hours(30)).is_none(), "once a week");
        // the next week: nothing filed since, nothing said
        assert!(take_digest(&config, "coach", "fr", monday + chrono::Duration::days(7)).is_none());
        assert!(take_digest(&config, "vie", "fr", monday).is_none(), "no rule, nothing");
    }
}
