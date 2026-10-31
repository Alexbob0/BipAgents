//! After a restore, the messages the phones kept put back into the place (Alex, 2026-10-31; design:
//! internal/next/restore-reconcile.md).
//!
//! A place that died is restored from its last backup (installer/bipinstall/backup.py `restore`, hourly backups):
//! what was said between that backup and the breakdown is gone from the place, but the phones often kept it (their
//! copy of each conversation, the last 300 messages). The person's paired phones send it back here, conversation by
//! conversation; the bridge puts the missing messages into the agent's Hermes conversation, at their place and once
//! each, so the person sees them again and the agent reads them as its history.
//!
//! ```text
//! GET  /v1/restore/reconcile[?agent=&session_id=]
//!      {restore: {restored_at, backup_at, until} | null, open,
//!       sessions: [{agent, session_id, count, devices, updated, recovered: [{at, role}]}]}
//! POST /v1/restore/reconcile
//!      {agent, session_id, title?, device?, messages: [{id, role, text, at, tools?: [name], attachments?: [{kind, name}]}]}
//!   →  {session_id, created, inserted, already, outside, ignored, moved, recovered: [{id, role, at}], sentence}
//! ```
//!
//! - **When**: only after a restore the installer noted (`<data_dir>/restored.json`: `restored_at`, `backup_at`), for
//!   `WINDOW_SECONDS`, and only messages between the backup and the restore (before the backup a missing message was
//!   deleted on purpose; after the restore it comes from the new place). Else 409.
//! - **Which are missing**: Hermes' message numbers start again where the backup left them, so the phone's ids name
//!   other messages after a restore. A phone's message is there when the place has one of the same role at the same
//!   server time (`SAME_TIME`: Android keeps milliseconds), or with the same visible words within `SAME_TEXT_WITHIN`.
//!   Rewound or compacted rows count as there. Several phones: each sends its copy, only what is still missing goes
//!   in (calls can be repeated safely).
//! - **Where**: real Hermes rows (`user` / `assistant`, never a tool row), with their own time, in the conversation's
//!   live continuation (a compressed conversation goes on in a child session, as Hermes resumes it). Hermes orders by
//!   row number: rows written since the restore that come later are copied after them (same columns, same
//!   `message_uid`) and the old ones deleted, in one transaction. A conversation made after the backup is made again
//!   with its id and title. Each row keeps in `display_metadata` (neither the model nor the apps read it)
//!   `{"bip_recovered": {restore, device, phone_id}}`.
//! - **For the agent**: two hidden notes (`<!--bip:restore …-->`, user rows, the apps never show them) around each
//!   batch: the messages that follow were recovered from the phone after a restore on <date>; at the end, that what
//!   they tell already happened (no action done again), the tools it had used, and what is lost all the same.
//!   Nothing is run: no turn, no tool.
//! - **For the person**: one sentence back (`sentence`), and this route's `GET` says which messages were recovered
//!   (role + time) so the app marks them.
//! - **Safety**: under /v1, the bridge key only (the person's paired phones). Not while a turn of the agent goes on in
//!   that conversation (409 busy: the run hub follows it, or Hermes holds its turn lease). Limits: `MAX_MESSAGES` per
//!   call, `MAX_TEXT` characters per message, the body's 4 MB.
//!
//! The rows are written in the agent's `state.db` directly (as message_search.rs reads it): Hermes' API has no way to
//! put a message at a given place with its own time. Only the columns present are used; Hermes' triggers fill its
//! search index, display order and missing `message_uid`s, and it computes its display identities again at its next
//! read. A trace of each batch is kept in `<data_dir>/restore-reconcile.json`, started again at the next restore.
//!
//! No Python counterpart.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rusqlite::{params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::web::{
    self, body_loc, check_max_items, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, QueryMap,
    Validate,
};

/// Written by the installer at the end of a restore, in the bridge's data folder.
pub const RESTORED: &str = "restored.json";
/// The batches put back since the last restore.
pub const LOG: &str = "restore-reconcile.json";
/// How long after a restore the phones may send back what they kept.
pub const WINDOW_SECONDS: f64 = 14.0 * 86_400.0;
/// A phone keeps the last 300 messages of a conversation.
pub const MAX_MESSAGES: usize = 300;
pub const MAX_TEXT: usize = 32_000;
pub const MAX_ID: usize = 128;
pub const MAX_TOOLS: usize = 30;
pub const MAX_ATTACHMENTS: usize = 20;
/// Same role at this close a server time: the same message (Android truncates to the millisecond).
pub const SAME_TIME: f64 = 0.0015;
/// Same role and same visible words this close: the same message.
pub const SAME_TEXT_WITHIN: f64 = 120.0;
/// A message the phone has from just after the restore (its clock is the server's) still counts in the window.
pub const AFTER_RESTORE_SLACK: f64 = 60.0;
/// Rows written since the restore copied after the recovered ones, at most.
pub const MAX_MOVED: usize = 5_000;
/// How the agent's notes start (the apps hide every `<!--bip:` note).
pub const NOTE_OPEN: &str = "<!--bip:restore\n";
/// The notes sit this far before the first and after the last message of their batch.
const NOTE_GAP: f64 = 0.001;
const LOG_BATCHES: usize = 2_000;

/// One write at a time (the log, and two phones sending the same conversation).
static WRITING: Mutex<()> = Mutex::new(());

pub fn router() -> Router<AppState> {
    Router::new().route("/restore/reconcile", get(status_route).post(reconcile_route))
}

// -- the restore ------------------------------------------------------------------------------------------------

/// When this place was restored, and from which backup (`None`: an older backup whose manifest had no time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Restore {
    pub restored_at: f64,
    pub backup_at: Option<f64>,
}

impl Restore {
    pub fn until(&self) -> f64 {
        self.restored_at + WINDOW_SECONDS
    }

    pub fn open(&self, now: f64) -> bool {
        now <= self.until()
    }

    fn to_json(self) -> Value {
        json!({"restored_at": self.restored_at, "backup_at": self.backup_at, "until": self.until()})
    }
}

/// The installer's note in `data_dir`, if there was a restore.
pub fn restore_of(data_dir: &Path) -> Option<Restore> {
    let data: Value = serde_json::from_str(&std::fs::read_to_string(data_dir.join(RESTORED)).ok()?).ok()?;
    let restored_at = data.get("restored_at")?.as_f64()?;
    Some(Restore { restored_at, backup_at: data.get("backup_at").and_then(Value::as_f64) })
}

// -- what a phone sends -----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhoneAttachment {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// A message of the phone's copy: Hermes' id and time (the server's), its role and text as Hermes gave them, the
/// tools the app showed under it and the files it showed with it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PhoneMessage {
    #[serde(default)]
    pub id: String,
    pub role: String,
    #[serde(default)]
    pub text: String,
    /// Unix seconds, as Hermes gave them.
    pub at: f64,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub attachments: Vec<PhoneAttachment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReconcileBody {
    pub agent: String,
    pub session_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    pub messages: Vec<PhoneMessage>,
}

impl Validate for ReconcileBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "agent", Some(self.agent.as_str()), 1);
        check_max_len(errors, "agent", Some(self.agent.as_str()), 100);
        check_min_len(errors, "session_id", Some(self.session_id.as_str()), 1);
        check_max_len(errors, "session_id", Some(self.session_id.as_str()), 200);
        check_max_len(errors, "title", self.title.as_deref(), 300);
        check_max_len(errors, "device", self.device.as_deref(), 80);
        check_max_items(errors, "messages", Some(self.messages.as_slice()), MAX_MESSAGES);
        for (i, m) in self.messages.iter().enumerate() {
            let loc = |field: &str| vec![json!("body"), json!("messages"), json!(i), json!(field)];
            let too_long = |field: &str, max: usize| {
                FieldError::new("too_long", loc(field), format!("at most {max}"), Value::Null)
            };
            if m.id.chars().count() > MAX_ID {
                errors.push(too_long("id", MAX_ID));
            }
            if m.text.chars().count() > MAX_TEXT {
                errors.push(too_long("text", MAX_TEXT));
            }
            if m.tools.len() > MAX_TOOLS || m.tools.iter().any(|t| t.chars().count() > 100) {
                errors.push(too_long("tools", MAX_TOOLS));
            }
            if m.attachments.len() > MAX_ATTACHMENTS
                || m.attachments.iter().any(|a| a.name.chars().count() > 200 || a.kind.chars().count() > 40)
            {
                errors.push(too_long("attachments", MAX_ATTACHMENTS));
            }
            if !m.at.is_finite() || m.at <= 0.0 {
                errors.push(FieldError::new("value_error", loc("at"), "a Unix time in seconds", json!(m.at)));
            }
        }
        if self.session_id.contains(['\r', '\n', '\0']) {
            errors.push(FieldError::new("value_error", body_loc("session_id"), "not a conversation id", Value::Null));
        }
    }
}

// -- matching ---------------------------------------------------------------------------------------------------

const STEER_OPEN: &str = "[OUT-OF-BAND USER MESSAGE";
const STEER_CLOSE: &str = "[/OUT-OF-BAND USER MESSAGE]";

/// The words a person sees: the text parts of a multimodal row, without the bridge's notes, without Hermes' wrapper
/// of a message added during an answer, spaces squeezed.
pub fn visible_words(content: &str) -> String {
    let mut text = crate::message_search::message_text(content);
    let trimmed = text.trim();
    if trimmed.starts_with(STEER_OPEN) && trimmed.ends_with(STEER_CLOSE) {
        if let Some(header) = trimmed[STEER_OPEN.len()..].find(']') {
            let start = STEER_OPEN.len() + header + 1;
            let end = trimmed.len() - STEER_CLOSE.len();
            if start <= end {
                text = trimmed[start..end].to_string();
            }
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A row of the place's conversation, for the comparison.
#[derive(Debug, Clone)]
pub struct Known {
    pub role: String,
    pub at: f64,
    pub words: String,
}

/// Whether the place already has this message of the phone.
pub fn already_there(message: &PhoneMessage, words: &str, known: &[Known]) -> bool {
    known.iter().any(|k| {
        k.role == message.role
            && ((k.at - message.at).abs() <= SAME_TIME
                || (!words.is_empty() && k.words == words && (k.at - message.at).abs() <= SAME_TEXT_WITHIN))
    })
}

// -- the store --------------------------------------------------------------------------------------------------

/// What one call did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Outcome {
    /// The conversation the messages went into (its live continuation).
    pub session_id: String,
    /// The conversation was made again (it began after the backup).
    pub created: bool,
    pub inserted: Vec<Recovered>,
    /// The two notes' `message_uid`s (when the store has the column).
    pub notes: Vec<String>,
    pub already: usize,
    pub outside: usize,
    pub ignored: usize,
    pub moved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recovered {
    pub id: String,
    pub role: String,
    pub at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// A turn of the agent goes on in this conversation.
    Busy,
    /// Too much was written since the restore to put the messages before it.
    TooLate,
    /// Not a Hermes store this bridge can write.
    Unreadable(String),
}

impl From<rusqlite::Error> for Refusal {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy) | Some(rusqlite::ErrorCode::DatabaseLocked) => Refusal::Busy,
            _ => Refusal::Unreadable(e.to_string()),
        }
    }
}

/// One phone's copy of one conversation.
pub struct Batch<'a> {
    pub session_id: &'a str,
    pub title: Option<&'a str>,
    pub device: &'a str,
    pub messages: &'a [PhoneMessage],
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

fn has_table(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1", [name], |_| Ok(()))
        .optional()?
        .is_some())
}

/// A child Hermes resumes in (a compression's continuation), not a branch, a sub-agent's run or a reset.
fn continuation(conn: &Connection, session: &str, cols: &[String]) -> rusqlite::Result<Option<String>> {
    if !cols.iter().any(|c| c == "parent_session_id") {
        return Ok(None);
    }
    let mut filters = String::new();
    if cols.iter().any(|c| c == "model_config") {
        for key in ["_branched_from", "_delegate_from", "_reset_from"] {
            filters.push_str(&format!(" AND COALESCE(model_config, '') NOT LIKE '%\"{key}\"%'"));
        }
    }
    if cols.iter().any(|c| c == "source") {
        filters.push_str(" AND COALESCE(source, '') != 'tool'");
    }
    let order = if cols.iter().any(|c| c == "started_at") { "started_at DESC, id DESC" } else { "id DESC" };
    conn.query_row(
        &format!("SELECT id FROM sessions WHERE parent_session_id = ?1{filters} ORDER BY {order} LIMIT 1"),
        [session],
        |row| row.get::<_, String>(0),
    )
    .optional()
}

/// The conversation's live end (where Hermes writes next), and every session of its line (for the comparison).
fn line_of(conn: &Connection, session: &str, cols: &[String]) -> rusqlite::Result<(String, Vec<String>)> {
    let mut line = vec![session.to_string()];
    if cols.iter().any(|c| c == "parent_session_id") {
        let mut current = session.to_string();
        for _ in 0..32 {
            let parent: Option<String> = conn
                .query_row("SELECT parent_session_id FROM sessions WHERE id = ?1", [&current], |row| row.get(0))
                .optional()?
                .flatten();
            match parent {
                Some(p) if !p.is_empty() && !line.contains(&p) => {
                    line.insert(0, p.clone());
                    current = p;
                }
                _ => break,
            }
        }
    }
    let mut tip = session.to_string();
    for _ in 0..32 {
        match continuation(conn, &tip, cols)? {
            Some(child) if !line.contains(&child) => {
                line.push(child.clone());
                tip = child;
            }
            _ => break,
        }
    }
    Ok((tip, line))
}

fn turn_leased(conn: &Connection, sessions: &[&str], now: f64) -> rusqlite::Result<bool> {
    if !has_table(conn, "session_turn_leases")? {
        return Ok(false);
    }
    for session in sessions {
        let held = conn
            .query_row(
                "SELECT 1 FROM session_turn_leases WHERE conversation_id = ?1 AND expires_at > ?2",
                rusqlite::params![session, now],
                |_| Ok(()),
            )
            .optional()?;
        if held.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A row to write: a recovered message, or one of the agent's notes.
#[derive(Debug, Clone)]
struct NewRow {
    role: &'static str,
    content: String,
    at: f64,
    meta: Value,
    uid: String,
    /// The phone's message (index in the candidates), None for a note.
    recovered: Option<usize>,
}

/// Put the missing messages of one phone's copy into the agent's store at `path`.
pub fn reconcile(path: &Path, batch: &Batch, restore: Restore, zone: Tz, now: f64) -> Result<Outcome, Refusal> {
    let mut conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let message_cols = columns(&tx, "messages")?;
    let session_cols = columns(&tx, "sessions")?;
    for needed in ["id", "session_id", "role", "content", "timestamp"] {
        if !message_cols.iter().any(|c| c == needed) {
            return Err(Refusal::Unreadable(format!("messages.{needed} missing")));
        }
    }
    if !session_cols.iter().any(|c| c == "id") {
        return Err(Refusal::Unreadable("sessions.id missing".into()));
    }
    let has = |name: &str| message_cols.iter().any(|c| c == name);
    let exists =
        tx.query_row("SELECT 1 FROM sessions WHERE id = ?1", [batch.session_id], |_| Ok(())).optional()?.is_some();
    let (tip, line) = if exists {
        line_of(&tx, batch.session_id, &session_cols)?
    } else {
        (batch.session_id.to_string(), vec![batch.session_id.to_string()])
    };
    if turn_leased(&tx, &[batch.session_id, tip.as_str()], now)? {
        return Err(Refusal::Busy);
    }

    // The place's rows of this conversation, every state (a rewound message stays gone).
    let mut known: Vec<Known> = Vec::new();
    if exists {
        let marks = vec!["?"; line.len()].join(", ");
        let mut statement = tx.prepare(&format!(
            "SELECT role, content, timestamp FROM messages WHERE session_id IN ({marks}) \
             AND role IN ('user', 'assistant')"
        ))?;
        let rows = statement.query_map(params_from_iter(line.iter()), |row| {
            let content = match row.get_ref(1)? {
                rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                    String::from_utf8_lossy(t).into_owned()
                }
                _ => String::new(),
            };
            Ok(Known {
                role: row.get(0)?,
                words: visible_words(&content),
                at: row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
            })
        })?;
        for row in rows {
            known.push(row?);
        }
    }

    // What is missing, in the window.
    let mut outcome = Outcome { session_id: tip.clone(), ..Outcome::default() };
    let mut seen: HashSet<(String, i64)> = HashSet::new();
    let mut found: Vec<&PhoneMessage> = Vec::new();
    let mut missing: Vec<&PhoneMessage> = Vec::new();
    for message in batch.messages {
        if message.role != "user" && message.role != "assistant" {
            outcome.ignored += 1;
            continue;
        }
        if !seen.insert((message.role.clone(), (message.at * 1000.0).round() as i64)) {
            outcome.ignored += 1;
            continue;
        }
        let words = visible_words(&message.text);
        if already_there(message, &words, &known) {
            outcome.already += 1;
            found.push(message);
        } else if message.text.trim().is_empty() {
            outcome.ignored += 1; // a photo alone: its file is not here
        } else {
            missing.push(message);
        }
    }
    // Without the backup's time: after the last message both sides have.
    let from = restore.backup_at.or_else(|| found.iter().map(|m| m.at).reduce(f64::max));
    let mut candidates: Vec<&PhoneMessage> = Vec::new();
    for message in missing {
        match from {
            Some(from) if message.at > from && message.at <= restore.restored_at + AFTER_RESTORE_SLACK => {
                candidates.push(message)
            }
            _ => outcome.outside += 1,
        }
    }
    if candidates.is_empty() {
        return Ok(outcome); // nothing written, the transaction rolled back
    }
    candidates.sort_by(|a, b| a.at.total_cmp(&b.at));

    if !exists {
        let mut names = vec!["id"];
        let mut values: Vec<Value> = vec![json!(batch.session_id)];
        let mut add = |name: &'static str, value: Value| {
            if session_cols.iter().any(|c| c == name) {
                names.push(name);
                values.push(value);
            }
        };
        add("source", json!("api_server"));
        add("started_at", json!(candidates[0].at));
        add("title", batch.title.map(|t| json!(t)).unwrap_or(Value::Null));
        add("message_count", json!(0));
        let marks = vec!["?"; names.len()].join(", ");
        let sql = format!("INSERT INTO sessions ({}) VALUES ({marks})", names.join(", "));
        tx.execute(&sql, params_from_iter(values.iter().map(sql_value)))?;
        outcome.created = true;
    }

    // The rows to write: the batch between its two notes.
    let first = candidates[0].at;
    let last = candidates[candidates.len() - 1].at;
    let mut rows: Vec<NewRow> = Vec::with_capacity(candidates.len() + 2);
    let note_meta = json!({"bip_recovered": {"restore": restore.restored_at, "device": batch.device, "note": true}});
    rows.push(NewRow {
        role: "user",
        content: opening_note(&restore, batch.device, &candidates, zone),
        at: first - NOTE_GAP,
        meta: note_meta.clone(),
        uid: uuid::Uuid::new_v4().simple().to_string(),
        recovered: None,
    });
    for (i, message) in candidates.iter().enumerate() {
        rows.push(NewRow {
            role: if message.role == "user" { "user" } else { "assistant" },
            content: message.text.clone(),
            at: message.at,
            meta: json!({"bip_recovered": {"restore": restore.restored_at, "device": batch.device,
                                           "phone_id": message.id}}),
            uid: uuid::Uuid::new_v4().simple().to_string(),
            recovered: Some(i),
        });
    }
    rows.push(NewRow {
        role: "user",
        content: closing_note(&restore, &candidates, zone),
        at: last + NOTE_GAP,
        meta: note_meta,
        uid: uuid::Uuid::new_v4().simple().to_string(),
        recovered: None,
    });

    // The rows written since that come later: copied after the new ones.
    let split: Option<i64> = tx.query_row(
        "SELECT MIN(id) FROM messages WHERE session_id = ?1 AND timestamp > ?2",
        rusqlite::params![tip, first - NOTE_GAP],
        |row| row.get(0),
    )?;
    let mut tail: Vec<(i64, f64)> = Vec::new();
    if let Some(split) = split {
        let mut statement =
            tx.prepare("SELECT id, timestamp FROM messages WHERE session_id = ?1 AND id >= ?2 ORDER BY id")?;
        let found = statement.query_map(rusqlite::params![tip, split], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<f64>>(1)?.unwrap_or(f64::MAX)))
        })?;
        for row in found {
            tail.push(row?);
        }
    }
    if tail.len() > MAX_MOVED {
        return Err(Refusal::TooLate);
    }
    if let Some(split) = split.filter(|_| !tail.is_empty()) {
        tx.execute("DROP TABLE IF EXISTS temp.bip_tail", [])?;
        tx.execute(
            "CREATE TEMP TABLE bip_tail AS SELECT * FROM messages WHERE session_id = ?1 AND id >= ?2",
            rusqlite::params![tip, split],
        )?;
        tx.execute("DELETE FROM messages WHERE session_id = ?1 AND id >= ?2", rusqlite::params![tip, split])?;
    }

    // Hermes computes these again itself (its triggers, its next read).
    let copied: Vec<&String> = message_cols.iter().filter(|c| c.as_str() != "id").collect();
    let selected: Vec<String> = copied
        .iter()
        .map(|c| match c.as_str() {
            "display_order" | "display_identity" => "NULL".to_string(),
            other => format!("\"{other}\""),
        })
        .collect();
    let copy_sql = format!(
        "INSERT INTO messages ({}) SELECT {} FROM temp.bip_tail WHERE id = ?1",
        copied.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", "),
        selected.join(", ")
    );
    let mut new_names = vec!["session_id", "role", "content", "timestamp"];
    for optional in ["active", "display_metadata", "message_uid"] {
        if has(optional) {
            new_names.push(optional);
        }
    }
    let insert_sql =
        format!("INSERT INTO messages ({}) VALUES ({})", new_names.join(", "), vec!["?"; new_names.len()].join(", "));

    let mut pending = rows.iter().peekable();
    let write_new = |row: &NewRow, outcome: &mut Outcome| -> rusqlite::Result<()> {
        let mut values: Vec<rusqlite::types::Value> =
            vec![tip.clone().into(), row.role.to_string().into(), row.content.clone().into(), row.at.into()];
        for optional in &new_names[4..] {
            values.push(match *optional {
                "active" => 1_i64.into(),
                "display_metadata" => row.meta.to_string().into(),
                _ => row.uid.clone().into(),
            });
        }
        tx.execute(&insert_sql, params_from_iter(values))?;
        let uid = has("message_uid").then(|| row.uid.clone());
        match row.recovered {
            Some(i) => outcome.inserted.push(Recovered {
                id: candidates[i].id.clone(),
                role: candidates[i].role.clone(),
                at: candidates[i].at,
                uid,
            }),
            None => outcome.notes.extend(uid),
        }
        Ok(())
    };
    for (old_id, at) in &tail {
        while pending.peek().is_some_and(|row| row.at < *at) {
            write_new(pending.next().expect("peeked"), &mut outcome)?;
        }
        tx.execute(&copy_sql, [old_id])?;
        outcome.moved += 1;
    }
    for row in pending {
        write_new(row, &mut outcome)?;
    }
    if !tail.is_empty() {
        tx.execute("DROP TABLE temp.bip_tail", [])?;
    }
    if session_cols.iter().any(|c| c == "message_count") {
        tx.execute(
            "UPDATE sessions SET message_count = COALESCE(message_count, 0) + ?1 WHERE id = ?2",
            rusqlite::params![rows.len() as i64, tip],
        )?;
    }
    tx.commit()?;
    Ok(outcome)
}

fn sql_value(value: &Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Number(n) if n.is_i64() => n.as_i64().unwrap_or_default().into(),
        Value::Number(n) => n.as_f64().unwrap_or_default().into(),
        Value::String(s) => s.clone().into(),
        other => other.to_string().into(),
    }
}

// -- the notes and the sentence -----------------------------------------------------------------------------------

fn local(at: f64, zone: Tz) -> DateTime<Tz> {
    DateTime::<Utc>::from_timestamp(at as i64, 0).unwrap_or_default().with_timezone(&zone)
}

fn day(at: f64, zone: Tz) -> String {
    local(at, zone).format("%-d %b %Y").to_string()
}

fn moment(at: f64, zone: Tz) -> String {
    local(at, zone).format("%-d %b %H:%M").to_string()
}

/// « The 2 messages that follow (30 Oct 14:05 to 30 Oct 14:40) were said », « The message that follows (…) was said ».
fn span(messages: &[&PhoneMessage], zone: Tz) -> String {
    let n = messages.len();
    let first = moment(messages[0].at, zone);
    let last = moment(messages[n - 1].at, zone);
    let when = if first == last { first } else { format!("{first} to {last}") };
    if n == 1 {
        format!("The message that follows ({when}) was said")
    } else {
        format!("The {n} messages that follow ({when}) were said")
    }
}

/// The note before a batch.
pub fn opening_note(restore: &Restore, device: &str, messages: &[&PhoneMessage], zone: Tz) -> String {
    let backup = restore.backup_at.map(|at| format!(" ({})", moment(at, zone))).unwrap_or_default();
    let phone =
        if device.is_empty() { "your person's phone".to_string() } else { format!("your person's phone ({device})") };
    format!(
        "{NOTE_OPEN}Recovered from the phone after a restore on {}: your server broke down and was restored from its last \
backup{backup}. {} between that backup and the breakdown: this server had lost them, {phone} had kept them. They are \
part of this conversation; read them as such.\n-->",
        day(restore.restored_at, zone),
        span(messages, zone)
    )
}

/// The note after a batch: nothing done again, the tools used, what is lost all the same.
pub fn closing_note(restore: &Restore, messages: &[&PhoneMessage], zone: Tz) -> String {
    let mut tools: Vec<(String, usize)> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for message in messages {
        for tool in &message.tools {
            let tool = tool.trim();
            if tool.is_empty() {
                continue;
            }
            match tools.iter_mut().find(|(name, _)| name == tool) {
                Some((_, count)) => *count += 1,
                None => tools.push((tool.to_string(), 1)),
            }
        }
        for file in &message.attachments {
            let name = file.name.trim();
            let shown = if !name.is_empty() {
                name.to_string()
            } else if file.kind == "image" {
                "a photo".to_string()
            } else {
                "a file".to_string()
            };
            if files.len() < 10 && !files.contains(&shown) {
                files.push(shown);
            }
        }
    }
    let tools_line = if tools.is_empty() {
        String::new()
    } else {
        let listed: Vec<String> = tools
            .iter()
            .take(15)
            .map(|(name, n)| if *n > 1 { format!("{name} ({n})") } else { name.clone() })
            .collect();
        format!(" In these messages you had used: {}.", listed.join(", "))
    };
    let files_line = if files.is_empty() {
        String::new()
    } else {
        format!(" The files and photos shown with them were not recovered: {}.", files.join(", "))
    };
    let since = restore.backup_at.map(|at| format!("from {} to the breakdown", moment(at, zone)));
    format!(
        "{NOTE_OPEN}End of the messages recovered from the phone (restore on {}). What they tell already happened, or \
failed: never do any of it again (no email, message, payment, booking or tool call repeated) unless your person asks \
anew.{tools_line}{files_line} Lost all the same, {}: the files you made, what you added to your memory and to the \
Carnet, and the routines you set up or changed. If these messages rely on one of them, check it is still there before \
using it, and say simply if it is gone.\n-->",
        day(restore.restored_at, zone),
        since.as_deref().unwrap_or("since the last backup")
    )
}

/// The one sentence the app shows the person under the recovered messages.
pub fn sentence(count: usize, restore: &Restore, zone: Tz) -> String {
    if count == 0 {
        return String::new();
    }
    let messages =
        if count == 1 { "1 message récupéré".to_string() } else { format!("{count} messages récupérés") };
    let after = match restore.backup_at {
        Some(at) => {
            let at = local(at, zone);
            format!("après le {} à {}", at.format("%d/%m"), at.format("%H:%M"))
        }
        None => "après la dernière sauvegarde".to_string(),
    };
    format!(
        "{messages} depuis ton téléphone ; ce que tes agents ont créé ou noté {after} (fichiers, Carnet, mémoire) n'a \
pas pu être récupéré."
    )
}

// -- the trace ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogBatch {
    pub agent: String,
    pub session_id: String,
    /// The conversation the messages went into.
    pub into: String,
    pub device: String,
    pub at: f64,
    pub recovered: Vec<Recovered>,
    #[serde(default)]
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Log {
    pub restored_at: f64,
    pub batches: Vec<LogBatch>,
}

fn log_path(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG)
}

/// The trace of this restore (an older restore's is not this one's).
pub fn read_log(data_dir: &Path, restore: &Restore) -> Log {
    let log: Option<Log> =
        std::fs::read_to_string(log_path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok());
    match log {
        Some(log) if log.restored_at == restore.restored_at => log,
        _ => Log { restored_at: restore.restored_at, batches: Vec::new() },
    }
}

fn write_log(data_dir: &Path, log: &Log) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = log_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(log).unwrap_or_default())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

/// The conversations put back so far, each with its recovered messages (role and time, for the app's mark).
pub fn sessions_view(log: &Log, agent: Option<&str>, session: Option<&str>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut keys: Vec<(String, String)> = Vec::new();
    for batch in &log.batches {
        let key = (batch.agent.clone(), batch.session_id.clone());
        if agent.is_some_and(|a| a != batch.agent) || session.is_some_and(|s| s != batch.session_id && s != batch.into)
        {
            continue;
        }
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    for (agent, session) in keys {
        let batches: Vec<&LogBatch> =
            log.batches.iter().filter(|b| b.agent == agent && b.session_id == session).collect();
        let mut recovered: Vec<Value> = Vec::new();
        let mut devices: Vec<String> = Vec::new();
        for batch in &batches {
            recovered.extend(batch.recovered.iter().map(|r| json!({"at": r.at, "role": r.role})));
            if !batch.device.is_empty() && !devices.contains(&batch.device) {
                devices.push(batch.device.clone());
            }
        }
        out.push(json!({
            "agent": agent, "session_id": session,
            "into": batches.last().map(|b| b.into.clone()).unwrap_or_default(),
            "count": recovered.len(), "devices": devices,
            "updated": batches.iter().map(|b| b.at).fold(0.0, f64::max),
            "recovered": recovered,
        }));
    }
    out
}

// -- the routes --------------------------------------------------------------------------------------------------

async fn status_route(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let data_dir = PathBuf::from(config.data_dir());
    let now = crate::py::time_time();
    let Some(restore) = restore_of(&data_dir) else {
        return Ok(web::ok(json!({"restore": null, "open": false, "sessions": []})));
    };
    let log = read_log(&data_dir, &restore);
    let sessions = sessions_view(&log, q.get("agent"), q.get("session_id"));
    Ok(web::ok(json!({"restore": restore.to_json(), "open": restore.open(now), "sessions": sessions})))
}

async fn reconcile_route(State(state): State<AppState>, Json(body): Json<ReconcileBody>) -> ApiResult<Response> {
    let config = state.config();
    let data_dir = PathBuf::from(config.data_dir());
    let now = crate::py::time_time();
    let restore =
        restore_of(&data_dir).ok_or_else(|| HttpError::new(409, "this place was not restored: nothing to put back"))?;
    if !restore.open(now) {
        return Err(HttpError::new(409, "the restore is too long ago: nothing is put back any more"));
    }
    let agent = config.agent(Some(&body.agent)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))?;
    let store = crate::message_search::store_path(&config, &agent.name)
        .ok_or_else(|| HttpError::new(404, "this agent's conversations are not on this server"))?;
    if state.hub.session_busy(&agent.name, &body.session_id) {
        return Err(HttpError::json(409, json!({"busy": true, "message": "the agent is answering: try again"})));
    }
    let zone = crate::dates::now::zone_of(&config, &agent);
    let device = crate::tools_knowledge::squeeze(body.device.as_deref().unwrap_or(""));
    let agent_name = agent.name.clone();
    let done = tokio::task::spawn_blocking(move || {
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let batch = Batch {
            session_id: &body.session_id,
            title: body.title.as_deref(),
            device: &device,
            messages: &body.messages,
        };
        let outcome = reconcile(&store, &batch, restore, zone, now)?;
        if !outcome.inserted.is_empty() {
            let mut log = read_log(&data_dir, &restore);
            log.batches.push(LogBatch {
                agent: agent_name.clone(),
                session_id: body.session_id.clone(),
                into: outcome.session_id.clone(),
                device: device.clone(),
                at: now,
                recovered: outcome.inserted.clone(),
                notes: outcome.notes.clone(),
            });
            let over = log.batches.len().saturating_sub(LOG_BATCHES);
            log.batches.drain(..over);
            if let Err(e) = write_log(&data_dir, &log) {
                tracing::warn!(error = %e, "restore-reconcile: trace not written");
            }
        }
        Ok::<Outcome, Refusal>(outcome)
    })
    .await
    .map_err(|e| anyhow::anyhow!(e))?;
    let outcome = match done {
        Ok(outcome) => outcome,
        Err(Refusal::Busy) => {
            return Err(HttpError::json(409, json!({"busy": true, "message": "the agent is answering: try again"})))
        }
        Err(Refusal::TooLate) => {
            return Err(HttpError::new(409, "too much was said since the restore to put these messages back"))
        }
        Err(Refusal::Unreadable(why)) => {
            tracing::warn!(agent = %agent.name, why = %why, "restore-reconcile: store not writable");
            return Err(HttpError::new(409, "this agent's conversations cannot be completed here"));
        }
    };
    if !outcome.inserted.is_empty() {
        tracing::info!(agent = %agent.name, session = %outcome.session_id, inserted = outcome.inserted.len(),
                       moved = outcome.moved, created = outcome.created, "restore-reconcile: messages put back");
    }
    let recovered: Vec<Value> =
        outcome.inserted.iter().map(|r| json!({"id": r.id, "role": r.role, "at": r.at})).collect();
    Ok(web::ok(json!({
        "session_id": outcome.session_id,
        "created": outcome.created,
        "inserted": outcome.inserted.len(),
        "already": outcome.already,
        "outside": outcome.outside,
        "ignored": outcome.ignored,
        "moved": outcome.moved,
        "recovered": recovered,
        "sentence": sentence(outcome.inserted.len(), &restore, zone),
    })))
}
