//! A Live call continues a conversation from a short brief of it instead of its whole history (100k tokens and more:
//! a minute of reading for a local model, and a cache broken for the written conversation). The brief:
//!
//! - the key points, which the agent itself keeps up to date in the background: in a throwaway session (hidden, deleted
//!   at once), from its previous points and the messages since, once a conversation has had 6 new messages and been quiet
//!   a minute (its turn is over, the model server's cache is still warm); one at a time, never for Live, scheduled-task
//!   or throwaway sessions;
//! - the last exchanges, word for word, read when the brief is asked for: always up to date;
//! - the calls of the last day that continued it.
//!
//! The apps give it to the call's own short session as instructions (Hermes adds them to the end of its system prompt),
//! the same text at every turn of the call so that the model server's cache holds.
//!
//!     GET  /v1/live/brief?agent=&session=   {session, text, points_at}   (without a session: the agent's latest one)
//!     POST /v1/live/calls                   {agent, session, live_session}   a call that just ended
//!     POST /v1/live/recaps                  {agent, session, recap}   what the call said, for the next written message
//!     POST /v1/live/recaps/take             {agent, session} -> {recap}   taken by the device that writes it (any of
//!                                           the person's: the recap waits here, not on the phone that made the call)
//!
//! Messages and points stay on this server (data folder « briefs », one file per conversation).
//!
//! The throwaway sessions must never come back. Hermes (517b5e10f) titles a session in a background thread after its
//! first turn (`agent/title_generator.py` `auto_title_session`), and that thread bills its model call to the session
//! (`agent/aux_accounting.py` → `hermes_state_usage.py` `record_auxiliary_usage`), which upserts the session row
//! (`_insert_session_row(session_id, "unknown")`): after our DELETE, a new empty, visible row then took the generated
//! title — an empty conversation named after the person's subjects, every hour or two. So:
//!
//! - the scratch session's title is set with a PATCH (Hermes' « user » provenance): the titler sees a title it must
//!   not replace and stops before any model call (`_has_upgraded_title`); the title is unique (Hermes refuses a
//!   title another session holds);
//! - the scratch ids are kept (« briefs/scratch.json ») and deleted again a few times over the next half hour, and at
//!   the bridge's start, until Hermes answers 404 (that path only ever deletes the ids it created);
//! - the empty rows already written back are swept (`is_ghost`): an `api_` id (minted by `POST /api/sessions`) whose
//!   source is Hermes' accounting placeholder « unknown », no message (count and transcript), not pinned, ten
//!   minutes old at least.
//!
//! Port of `bridge/bipbridge/live_brief.py` (owner group: voice-speech).

use std::sync::{LazyLock, Mutex};

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::{Local, TimeZone};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, SharedConfig};
use crate::cronwatch::is_cron_session;
use crate::hermes::{HermesClient, HermesError};
use crate::py;
use crate::voice_speech::pystr;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::Dict;

pub const RECENT_MESSAGES: usize = 8;
/// Characters of call recaps waiting for one conversation.
pub const RECAPS_MAX: usize = 8000;
/// Between two calls' recaps in their file (a recap holds line breaks).
pub const RECAP_SEPARATOR: &str = "\u{1e}";
static RECAP_LOCK: Mutex<()> = Mutex::new(());
pub const MESSAGE_CHARS: usize = 700;
pub const RECENT_CHARS: usize = 6000;
/// New messages before the key points are written again.
pub const POINTS_EVERY: usize = 6;
/// Shorter conversations: the last exchanges say it all.
pub const MIN_MESSAGES: usize = 10;
/// The turn is over.
pub const QUIET_SECONDS: f64 = 60.0;
pub const LOOP_SECONDS: f64 = 120.0;
pub const CALLS_KEPT: usize = 3;
pub const CALL_HOURS: f64 = 24.0;
pub const CALL_CHARS: usize = 2500;
pub const POINTS_TITLE: &str = "BipAgents · notes";
/// After a scratch session's deletion, when it is deleted again (seconds): Hermes' titler may write it back meanwhile.
pub const SCRATCH_CHECKS: [f64; 6] = [10.0, 30.0, 90.0, 300.0, 900.0, 1800.0];
/// A scratch session Hermes still keeps past the last check (a turn still running…): again every half hour…
pub const SCRATCH_RETRY_SECONDS: f64 = 1800.0;
/// …for two days at most.
pub const SCRATCH_KEPT_SECONDS: f64 = 2.0 * 86400.0;
/// While its chat runs, a scratch session is not deleted by the sweep (a crash meanwhile: deleted at the next start).
pub const SCRATCH_CHAT_SECONDS: f64 = 300.0;
pub const SWEEP_SECONDS: f64 = 5.0;
/// The empty rows Hermes wrote back: looked for 20 s after the start, then every 6 hours.
pub const GHOSTS_FIRST_SECONDS: f64 = 20.0;
pub const GHOSTS_EVERY_SECONDS: f64 = 6.0 * 3600.0;
pub const GHOST_AGE_SECONDS: f64 = 600.0;
const GHOST_PAGE: i64 = 200;
const GHOST_PAGES: i64 = 10;
static SCRATCH_LOCK: Mutex<()> = Mutex::new(());
pub const LIVE_TITLE: &str = "Live";
/// What the apps put in a message for the agent only (a call's opening, a call's recap): never shown to the person.
static HIDDEN: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?s)<!--\s*bip:.*?-->").unwrap());

pub const POINTS_REQUEST: &str =
    "<!--bip:notes-->[Request from the BipAgents app, not from the person. Use no tool and save nothing to \
memory: only answer with the text asked for.]
You keep short notes so that a voice call can continue a conversation with the person. Here are your previous notes, \
then the messages since.

Previous notes:
{points}

Messages since:
{messages}

Write the updated notes: what is being discussed, the decisions taken, what you promised or still have to do, the \
names, dates and facts the person gave, their mood if it matters. At most 12 short lines, most important first, in \
the language of the conversation. Only the notes.";

/// `POINTS_REQUEST.format(points=…, messages=…)` (the values are not read for placeholders again).
pub fn points_request(points: &str, messages: &str) -> String {
    let (head, rest) = POINTS_REQUEST.split_once("{points}").expect("template");
    let (middle, tail) = rest.split_once("{messages}").expect("template");
    format!("{head}{points}{middle}{messages}{tail}")
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A Python exception without an error type of its own here (`KeyError`…), named for the logs.
#[derive(Debug, thiserror::Error)]
#[error("{1}")]
struct PyError(&'static str, String);

/// `type(exc).__name__`.
fn error_name(err: &anyhow::Error) -> &'static str {
    if let Some(e) = err.downcast_ref::<HermesError>() {
        return e.type_name();
    }
    if let Some(e) = err.downcast_ref::<PyError>() {
        return e.0;
    }
    if err.downcast_ref::<std::io::Error>().is_some() {
        return "OSError";
    }
    "Exception"
}

fn is_hermes_http(err: &anyhow::Error) -> bool {
    matches!(err.downcast_ref::<HermesError>(), Some(HermesError::Http { .. }))
}

pub fn visible(text: &str) -> String {
    pystr::strip(&HIDDEN.replace_all(text, "")).to_string()
}

fn content(message: &Dict) -> String {
    match message.get("content") {
        Some(Value::Array(parts)) => {
            let joined: Vec<String> = parts
                .iter()
                .filter_map(|p| p.as_object())
                .map(|p| match p.get("text") {
                    Some(v) if py::truthy(v) => py::py_str(v),
                    _ => String::new(),
                })
                .collect();
            visible(&joined.join(" "))
        }
        Some(Value::String(s)) => visible(s),
        _ => String::new(),
    }
}

/// (role, text) of what the person and the agent said, tools and machinery left out.
pub fn exchanges(messages: &[Dict]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !(role == "user" || role == "assistant")
            || message.get("display_kind").and_then(Value::as_str) == Some("hidden")
        {
            continue;
        }
        let text = content(message);
        if !text.is_empty() {
            out.push((role.to_string(), text));
        }
    }
    out
}

pub fn transcript(pairs: &[(String, String)], limit: usize) -> String {
    let line = |role: &str, text: &str| -> String {
        let short = if pystr::len(text) <= limit {
            text.to_string()
        } else {
            let head = pystr::head(text, limit);
            let cut = match head.rfind(' ') {
                Some(i) => head[..i].to_string(),
                None => head,
            };
            cut + "…"
        };
        let prefix = if role == "user" { "- The person: " } else { "- You: " };
        prefix.to_string() + &pystr::split_ws(&short).join(" ")
    };
    pairs.iter().map(|(role, text)| line(role, text)).collect::<Vec<_>>().join("\n")
}

/// Python's `_seconds`: a number, or an ISO date (naive: local time, as `datetime.timestamp()`).
pub fn seconds(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Bool(b) => Some(*b as i64 as f64),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let text = s.replace('Z', "+00:00");
            let parsed = crate::store::fromisoformat(&text)?;
            let tail: String = text.chars().skip(10).collect();
            if tail.contains('+') || tail.contains('-') {
                Some(parsed.timestamp_micros() as f64 / 1e6)
            } else {
                let local = Local.from_local_datetime(&parsed.naive_utc()).earliest()?;
                Some(local.timestamp_micros() as f64 / 1e6)
            }
        }
        _ => None,
    }
}

fn truthy_key(session: &Dict, key: &str) -> bool {
    session.get(key).map(py::truthy).unwrap_or(false)
}

/// An empty row Hermes wrote back after its deletion (see the module's doc): only `POST /api/sessions` mints `api_`
/// ids and it gives them its own source, so an `api_` row with the accounting placeholder « unknown » and no message
/// was upserted by Hermes' usage accounting, not created by anyone. Never a pinned one, a Bot Chat, a scheduled task's
/// or a call's, and only past ten minutes. The transcript is checked as well before deleting (`sweep_ghosts`).
pub fn is_ghost(session: &Dict, now: f64) -> bool {
    let id = session.get("id").and_then(Value::as_str).unwrap_or("");
    let source = session.get("source").and_then(Value::as_str).unwrap_or("");
    let title = session.get("title").and_then(Value::as_str).unwrap_or("");
    let old = |key: &str| seconds(session.get(key)).is_some_and(|at| now - at >= GHOST_AGE_SECONDS);
    id.starts_with("api_")
        && source == "unknown"
        && as_int(session.get("message_count")) == Some(0)
        && !truthy_key(session, "pinned")
        && !title.eq_ignore_ascii_case("Bot Chat")
        && !title.starts_with(LIVE_TITLE)
        && !is_cron_session(session)
        && old("started_at")
        && session.get("last_active").is_none_or(|v| v.is_null() || old("last_active"))
}

/// A throwaway session to delete again (« briefs/scratch.json »).
#[derive(Debug, Clone, PartialEq)]
struct Scratch {
    agent: String,
    id: String,
    at: f64,
    checks: usize,
    next: f64,
}

impl Scratch {
    fn of(value: &Value) -> Option<Scratch> {
        Some(Scratch {
            agent: value.get("agent")?.as_str()?.to_string(),
            id: value.get("id")?.as_str()?.to_string(),
            at: value.get("at")?.as_f64()?,
            checks: value.get("checks").and_then(Value::as_u64).unwrap_or(0) as usize,
            next: value.get("next").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    fn json(&self) -> Value {
        json!({"agent": self.agent, "id": self.id, "at": self.at, "checks": self.checks, "next": self.next})
    }
}

/// A conversation with the person: not a call, a scheduled task, a throwaway notes session, a sub-agent or an empty row
/// Hermes wrote back after a deletion (its accounting placeholder source « unknown », no message, whatever its age).
pub fn is_conversation(session: &Dict) -> bool {
    let title = match session.get("title") {
        Some(v) if py::truthy(v) => py::py_str(v),
        _ => String::new(),
    };
    let written_back = session.get("source").and_then(Value::as_str) == Some("unknown")
        && as_int(session.get("message_count")) == Some(0);
    !(is_cron_session(session)
        || title.starts_with(LIVE_TITLE)
        || title.starts_with(POINTS_TITLE)
        || written_back
        || truthy_key(session, "is_internal_child")
        || truthy_key(session, "hidden"))
}

/// `re.sub(r"[^\w.-]", "_", name)`.
fn safe_name(name: &str) -> String {
    name.chars().map(|c| if pystr::is_word(c) || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// pathlib's `with_suffix(suffix)` on a file path.
fn with_suffix(path: &str, suffix: &str) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let stem = match name.rfind('.') {
        Some(i) if i > 0 && i < name.len() - 1 => &name[..i],
        _ => name,
    };
    format!("{dir}{stem}{suffix}")
}

/// An int (Python's `isinstance(x, int)`, bool included).
fn as_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

pub struct Briefs {
    pub folder: String,
    pub hermes: HermesClient,
    /// The agents as they are now (Python shares the config's dict).
    pub config: SharedConfig,
    writing: tokio::sync::Mutex<()>,
}

impl Briefs {
    pub fn new(folder: &str, hermes: HermesClient, config: SharedConfig) -> Self {
        Briefs { folder: folder.to_string(), hermes, config, writing: tokio::sync::Mutex::new(()) }
    }

    fn path(&self, agent: &str, session: &str) -> String {
        let safe = pystr::head(&safe_name(session), 120);
        py::join(&py::join(&self.folder, &safe_name(agent)), &format!("{safe}.json"))
    }

    fn recaps(&self, agent: &str, session: &str) -> String {
        let safe = pystr::head(&safe_name(session), 120);
        let folder = self.folder.trim_end_matches('/');
        let sibling = py::join(&py::dirname(folder), "call-recaps");
        py::join(&py::join(&sibling, &safe_name(agent)), &format!("{safe}.txt"))
    }

    /// A call's recap waits for the next message written in `session` (several calls: one after the other, the
    /// oldest dropped beyond RECAPS_MAX characters).
    pub fn keep_recap(&self, agent: &str, session: &str, recap: &str) -> std::io::Result<()> {
        let path = self.recaps(agent, session);
        let _guard = lock(&RECAP_LOCK);
        let mut kept: Vec<String> = match std::fs::read_to_string(&path) {
            Ok(text) => text.split(RECAP_SEPARATOR).map(str::to_string).collect(),
            Err(_) => Vec::new(),
        };
        kept.push(recap.to_string());
        while kept.len() > 1 && kept.iter().map(|r| pystr::len(r)).sum::<usize>() > RECAPS_MAX {
            kept.remove(0);
        }
        py::makedirs(&py::dirname(&path), 0o700)?;
        let temporary = with_suffix(&path, ".tmp");
        py::write_with_mode(&temporary, kept.join(RECAP_SEPARATOR).as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &path)
    }

    pub fn take_recap(&self, agent: &str, session: &str) -> Option<String> {
        let path = self.recaps(agent, session);
        let text = {
            let _guard = lock(&RECAP_LOCK);
            let text = std::fs::read_to_string(&path).ok()?;
            let _ = std::fs::remove_file(&path);
            text
        };
        let joined = text.split(RECAP_SEPARATOR).filter(|r| !r.is_empty()).collect::<Vec<_>>().join("\n");
        (!joined.is_empty()).then_some(joined)
    }

    pub fn load(&self, agent: &str, session: &str) -> Dict {
        match std::fs::read_to_string(self.path(agent, session))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        {
            Some(Value::Object(map)) => map,
            _ => Map::new(),
        }
    }

    pub fn save(&self, agent: &str, session: &str, data: &Dict) -> std::io::Result<()> {
        let path = self.path(agent, session);
        let parent = py::dirname(&path);
        std::fs::create_dir_all(&parent)?;
        py::chmod(&parent, 0o700)?;
        let temporary = with_suffix(&path, ".tmp");
        std::fs::write(&temporary, py::Dumps::new().unicode().dumps(&Value::Object(data.clone())))?;
        std::fs::rename(&temporary, &path)
    }

    /// The agent's most recent conversation.
    pub async fn latest(&self, agent: &AgentConfig) -> Result<Option<String>, HermesError> {
        for session in self.hermes.list_sessions(agent, 10).await? {
            if is_conversation(&session) {
                if let Some(Value::String(id)) = session.get("id") {
                    return Ok(Some(id.clone()));
                }
            }
        }
        Ok(None)
    }

    pub async fn brief(&self, agent: &AgentConfig, session: &str) -> Result<Dict, HermesError> {
        let state = self.load(&agent.name, session);
        let messages = exchanges(&self.hermes.session_messages(agent, session).await?);
        let mut recent: Vec<(String, String)> = Vec::new();
        let mut size = 0;
        let start = messages.len().saturating_sub(RECENT_MESSAGES);
        for (role, text) in messages[start..].iter().rev() {
            size += pystr::len(text).min(MESSAGE_CHARS);
            if !recent.is_empty() && size > RECENT_CHARS {
                break;
            }
            recent.insert(0, (role.clone(), text.clone()));
        }
        let mut parts = vec![
            "This is a voice call with the person. It continues the conversation summed up below: speak as in that \
             conversation, briefly, as one does on the phone."
                .to_string(),
        ];
        if let Some(points) = state.get("points").filter(|v| py::truthy(v)) {
            parts.push(format!(
                "Key points of the conversation so far (your own notes):\n{}",
                pystr::strip(&py::py_str(points))
            ));
        }
        if !recent.is_empty() {
            parts.push(format!("Its last exchanges:\n{}", transcript(&recent, MESSAGE_CHARS)));
        }
        let calls = self.calls(agent, &state).await?;
        if !calls.is_empty() {
            parts.push(calls);
        }
        let mut out = Map::new();
        out.insert("session".into(), json!(session));
        out.insert("text".into(), json!(parts.join("\n\n")));
        out.insert("points_at".into(), state.get("points_at").cloned().unwrap_or(Value::Null));
        Ok(out)
    }

    async fn calls(&self, agent: &AgentConfig, state: &Dict) -> Result<String, HermesError> {
        let mut out: Vec<String> = Vec::new();
        let calls: Vec<Value> = state.get("calls").and_then(Value::as_array).cloned().unwrap_or_default();
        let start = calls.len().saturating_sub(CALLS_KEPT);
        for call in &calls[start..] {
            let at = match call.get("at") {
                Some(v) if py::truthy(v) => py::py_float(v).unwrap_or(0.0),
                _ => 0.0,
            };
            if py::time_time() - at > CALL_HOURS * 3600.0 {
                continue;
            }
            let Some(live_session) = call.get("live_session") else { continue }; // KeyError
            let said = match self.hermes.session_messages(agent, &py::py_str(live_session)).await {
                Ok(messages) => exchanges(&messages),
                Err(HermesError::Http { .. }) => continue,
                Err(other) => return Err(other),
            };
            let text = pystr::head(&transcript(&said, 400), CALL_CHARS);
            if !text.is_empty() {
                let when = Local
                    .timestamp_micros((at * 1e6) as i64)
                    .single()
                    .map(|d| d.format("%d/%m %H:%M").to_string())
                    .unwrap_or_default();
                out.push(format!("Voice call of {when}:\n{text}"));
            }
        }
        Ok(if out.is_empty() {
            String::new()
        } else {
            format!("Earlier voice calls that continued it:\n{}", out.join("\n"))
        })
    }

    pub fn note_call(&self, agent: &str, session: &str, live_session: &str) -> std::io::Result<()> {
        let mut state = self.load(agent, session);
        let mut calls: Vec<Value> = state
            .get("calls")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|call| call.get("live_session").and_then(Value::as_str) != Some(live_session))
            .collect();
        calls.push(json!({"live_session": live_session, "at": py::time_time()}));
        let start = calls.len().saturating_sub(CALLS_KEPT);
        state.insert("calls".into(), Value::Array(calls[start..].to_vec()));
        self.save(agent, session, &state)
    }

    pub fn due(&self, agent: &str, session: &Dict, now: f64) -> bool {
        let count = as_int(session.get("message_count"));
        let last = seconds(session.get("last_active"));
        let Some(count) = count else { return false };
        if count < MIN_MESSAGES as i64 || !is_conversation(session) {
            return false;
        }
        if let Some(last) = last {
            if now - last < QUIET_SECONDS {
                return false;
            }
        }
        let id = py::py_str(session.get("id").unwrap_or(&Value::Null));
        match as_int(self.load(agent, &id).get("count")) {
            None => true,
            Some(done) => count - done >= POINTS_EVERY as i64 || count < done,
        }
    }

    /// The key points written again by the agent, from its previous ones and the messages since.
    pub async fn refresh(&self, agent: &AgentConfig, session: &str, count: Option<i64>) -> anyhow::Result<bool> {
        let _writing = self.writing.lock().await;
        let state = self.load(&agent.name, session);
        let messages = self.hermes.session_messages(agent, session).await?;
        let done = as_int(state.get("upto")).unwrap_or(0);
        // Compressed by Hermes since (fewer messages than covered): start again from the last ones.
        let since: &[Dict] = if 0 < done && done as usize <= messages.len() {
            &messages[done as usize..]
        } else {
            &messages[messages.len().saturating_sub(40)..]
        };
        let text = transcript(&exchanges(since), MESSAGE_CHARS);
        if text.is_empty() {
            return Ok(false);
        }
        let points = match state.get("points") {
            Some(v) if py::truthy(v) => py::py_str(v),
            _ => "(none yet)".to_string(),
        };
        let request = points_request(pystr::strip(&points), &text);
        // Unique: Hermes refuses a title another session holds (one left behind would stop the notes for good).
        let title = format!("{POINTS_TITLE} · {}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let scratch = self.hermes.create_session(agent, &title).await?;
        self.note_scratch(&agent.name, &scratch, py::time_time() + SCRATCH_CHAT_SECONDS)?;
        // The title again through a PATCH: « user » provenance, so Hermes' titler leaves the session alone (no model
        // call billed to it, which would write the row back after its deletion).
        let mut fields = Map::new();
        fields.insert("title".into(), json!(title));
        fields.insert("hidden".into(), Value::Bool(true));
        match self.hermes.update_session(agent, &scratch, fields.clone()).await {
            Ok(()) => {}
            Err(HermesError::Http { .. }) => {
                // an older Hermes without « hidden »: the session shows a few seconds, then it's deleted
                fields.remove("hidden");
                match self.hermes.update_session(agent, &scratch, fields).await {
                    Ok(()) | Err(HermesError::Http { .. }) => {}
                    Err(other) => return Err(other.into()),
                }
            }
            Err(other) => return Err(other.into()),
        }
        // thinking off, or GLM's « low » through EUrouter (« none » makes it answer nothing there)
        let options = crate::agents_admin::quick_turn_options(&self.config.get(), agent);
        let extra = json!({"model_options": options});
        let reply = self.hermes.session_chat(agent, &scratch, &request, extra.as_object().cloned(), 240.0).await;
        let deleted = self.hermes.delete_session(agent, &scratch).await;
        // Deleted or not, deleted again over the next half hour (`sweep_scratch`).
        self.note_scratch(&agent.name, &scratch, py::time_time() + SCRATCH_CHECKS[0])?;
        match deleted {
            Ok(()) => {}
            Err(HermesError::Http { status, .. }) => {
                tracing::warn!(agent = %agent.name, status = status, "notes session not deleted");
            }
            Err(other) => return Err(other.into()),
        }
        let points = visible(&reply?);
        if points.is_empty() {
            return Ok(false);
        }
        let mut state = self.load(&agent.name, session); // a call may have been noted meanwhile
        state.insert("points".into(), json!(pystr::head(&points, 4000)));
        state.insert("points_at".into(), json!(py::time_time()));
        state.insert("upto".into(), json!(messages.len()));
        state.insert("count".into(), json!(count.unwrap_or(messages.len() as i64)));
        self.save(&agent.name, session, &state)?;
        tracing::info!(agent = %agent.name, chars = pystr::len(&points), "live brief points");
        Ok(true)
    }

    fn scratch_path(&self) -> String {
        py::join(&self.folder, "scratch.json")
    }

    fn scratch_list(&self) -> Vec<Scratch> {
        let text = std::fs::read_to_string(self.scratch_path()).unwrap_or_default();
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => {
                map.get("sessions").and_then(Value::as_array).into_iter().flatten().filter_map(Scratch::of).collect()
            }
            _ => Vec::new(),
        }
    }

    fn scratch_save(&self, list: &[Scratch]) -> std::io::Result<()> {
        let path = self.scratch_path();
        if list.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        py::makedirs(&self.folder, 0o700)?;
        let data = json!({"sessions": list.iter().map(Scratch::json).collect::<Vec<_>>()});
        let temporary = with_suffix(&path, ".tmp");
        py::write_with_mode(&temporary, data.to_string().as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &path)
    }

    /// A scratch session to delete (again) at `next`; its checks start over.
    fn note_scratch(&self, agent: &str, id: &str, next: f64) -> std::io::Result<()> {
        let _guard = lock(&SCRATCH_LOCK);
        let mut list = self.scratch_list();
        list.retain(|s| !(s.agent == agent && s.id == id));
        list.push(Scratch { agent: agent.to_string(), id: id.to_string(), at: py::time_time(), checks: 0, next });
        self.scratch_save(&list)
    }

    /// The scratch sessions still to check (agent, id).
    pub fn scratch_pending(&self) -> Vec<(String, String)> {
        let _guard = lock(&SCRATCH_LOCK);
        self.scratch_list().into_iter().map(|s| (s.agent, s.id)).collect()
    }

    /// The scratch sessions due (all of them at the bridge's start) deleted again; one leaves the list once Hermes
    /// answers 404 at its last check (or after two days). Returns how many Hermes still had.
    pub async fn sweep_scratch(&self, now: f64, all: bool) -> std::io::Result<usize> {
        let due: Vec<Scratch> = {
            let _guard = lock(&SCRATCH_LOCK);
            self.scratch_list().into_iter().filter(|s| all || s.next <= now).collect()
        };
        if due.is_empty() {
            return Ok(0);
        }
        let config = self.config.get();
        let mut came_back = 0;
        let mut done: Vec<(Scratch, bool)> = Vec::new(); // (the entry updated, dropped)
        for mut entry in due {
            let Some(agent) = config.agents.values().find(|a| a.name == entry.agent) else {
                done.push((entry, true)); // the agent is gone, its sessions with it
                continue;
            };
            let gone = match self.hermes.delete_session(agent, &entry.id).await {
                Ok(()) => {
                    came_back += 1;
                    tracing::warn!(agent = %agent.name, session = %entry.id, "notes session still there, deleted again");
                    false
                }
                Err(HermesError::Http { status: 404, .. }) => true,
                Err(err) => {
                    tracing::warn!(agent = %agent.name, error = %err.type_name(), "notes session check failed");
                    false
                }
            };
            entry.checks += 1;
            let last = entry.checks >= SCRATCH_CHECKS.len();
            let drop = (gone && last) || now - entry.at > SCRATCH_KEPT_SECONDS;
            entry.next = now
                + if last {
                    SCRATCH_RETRY_SECONDS
                } else {
                    SCRATCH_CHECKS[entry.checks] - SCRATCH_CHECKS[entry.checks - 1]
                };
            done.push((entry, drop));
        }
        let _guard = lock(&SCRATCH_LOCK);
        let mut list = self.scratch_list();
        for (entry, drop) in done {
            let Some(index) = list.iter().position(|s| s.agent == entry.agent && s.id == entry.id) else { continue };
            if list[index].at != entry.at {
                continue; // noted again meanwhile (deleted once more by `refresh`): its own checks
            }
            if drop {
                list.remove(index);
            } else {
                list[index] = entry;
            }
        }
        self.scratch_save(&list)?;
        Ok(came_back)
    }

    /// The empty rows Hermes wrote back (`is_ghost`, and an empty transcript) deleted. Returns how many.
    pub async fn sweep_ghosts(&self, agent: &AgentConfig, now: f64) -> Result<usize, HermesError> {
        let mut ghosts = Vec::new();
        for page in 0..GHOST_PAGES {
            let sessions = self.hermes.list_sessions_page(agent, GHOST_PAGE, page * GHOST_PAGE).await?;
            for session in sessions.iter().filter(|s| is_ghost(s, now)) {
                if let Some(Value::String(id)) = session.get("id") {
                    ghosts.push(id.clone());
                }
            }
            if (sessions.len() as i64) < GHOST_PAGE {
                break;
            }
        }
        let mut deleted = 0;
        for id in ghosts {
            match self.hermes.session_messages(agent, &id).await {
                Ok(messages) if messages.is_empty() => {}
                _ => continue,
            }
            match self.hermes.delete_session(agent, &id).await {
                Ok(()) => deleted += 1,
                Err(HermesError::Http { .. }) => {}
                Err(other) => return Err(other),
            }
        }
        if deleted > 0 {
            tracing::info!(agent = %agent.name, count = deleted, "empty sessions written back by Hermes deleted");
        }
        Ok(deleted)
    }

    /// Every few seconds the scratch sessions due deleted again (all of them at the start); the rows Hermes wrote back
    /// swept 20 s after the start, then every 6 hours.
    pub async fn sweep_forever(self: std::sync::Arc<Self>) {
        let mut all = true;
        let mut ghosts_at = py::time_time() + GHOSTS_FIRST_SECONDS;
        loop {
            let now = py::time_time();
            if let Err(err) = self.sweep_scratch(now, all).await {
                tracing::warn!(error = %err, "notes sessions sweep");
            }
            all = false;
            if now >= ghosts_at {
                ghosts_at = now + GHOSTS_EVERY_SECONDS;
                let config = self.config.get();
                for agent in config.agents.values() {
                    if let Err(err) = self.sweep_ghosts(agent, now).await {
                        tracing::warn!(agent = %agent.name, error = %err.type_name(), "empty sessions sweep");
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs_f64(SWEEP_SECONDS)).await;
        }
    }

    /// Every two minutes, at most one conversation's key points written again (the one waiting the longest).
    pub async fn run(self: std::sync::Arc<Self>) {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            if let Err(exc) = self.tick(None).await {
                // the loop must keep running
                tracing::warn!(error = %error_name(&exc), "live brief loop");
            }
            tokio::time::sleep(std::time::Duration::from_secs_f64(LOOP_SECONDS)).await;
        }
    }

    pub async fn tick(&self, now: Option<f64>) -> anyhow::Result<bool> {
        let now = now.unwrap_or_else(py::time_time);
        let config = self.config.get();
        for agent in config.agents.values() {
            let Ok(sessions) = self.hermes.list_sessions(agent, 10).await else {
                continue; // an agent down must not stop the others
            };
            for session in &sessions {
                if !self.due(&agent.name, session, now) {
                    continue;
                }
                let count = as_int(session.get("message_count"));
                let Some(id) = session.get("id") else {
                    return Err(PyError("KeyError", "'id'".into()).into());
                };
                return match self.refresh(agent, &py::py_str(id), count).await {
                    Ok(done) => Ok(done),
                    Err(exc) if is_hermes_http(&exc) || exc.downcast_ref::<std::io::Error>().is_some() => {
                        tracing::warn!(agent = %agent.name, error = %error_name(&exc), "live brief points failed");
                        Ok(false)
                    }
                    Err(exc) => Err(exc),
                };
            }
        }
        Ok(false)
    }
}

#[derive(Debug, Deserialize)]
pub struct CallEnded {
    pub agent: String,
    pub session: String,
    pub live_session: String,
}

impl Validate for CallEnded {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "agent", Some(&self.agent), 200);
        web::check_max_len(errors, "session", Some(&self.session), 300);
        web::check_max_len(errors, "live_session", Some(&self.live_session), 300);
    }
}

#[derive(Debug, Deserialize)]
pub struct CallRecap {
    pub agent: String,
    pub session: String,
    pub recap: String,
}

impl Validate for CallRecap {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "agent", Some(&self.agent), 200);
        web::check_max_len(errors, "session", Some(&self.session), 300);
        web::check_min_len(errors, "recap", Some(&self.recap), 1);
        web::check_max_len(errors, "recap", Some(&self.recap), 4000);
    }
}

#[derive(Debug, Deserialize)]
pub struct RecapTaken {
    pub agent: String,
    pub session: String,
}

impl Validate for RecapTaken {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "agent", Some(&self.agent), 200);
        web::check_max_len(errors, "session", Some(&self.session), 300);
    }
}

fn setup(state: &AppState, name: &str) -> ApiResult<AgentConfig> {
    state.config().agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

fn hermes_error(exc: HermesError) -> HttpError {
    match exc {
        HermesError::Http { status, .. } => HttpError::new(if status >= 500 { 502 } else { status }, "hermes"),
        other => anyhow::Error::from(other).into(),
    }
}

async fn live_brief(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let session = web::query_str(&q, "session", false, None)?;
    let config = setup(&state, &agent)?;
    // What the call's turns send as `model_options` (the apps since 2026-10-28; before, always thinking off): GLM
    // through EUrouter can't switch thinking off and answers nothing to « none », so it gets « low ».
    let options = crate::agents_admin::quick_turn_options(&state.config(), &config);
    let chosen = match session.filter(|s| !s.is_empty()) {
        Some(s) => Some(s),
        None => state.briefs.latest(&config).await.map_err(hermes_error)?,
    };
    let Some(chosen) = chosen else {
        return Ok(web::ok(json!({"session": null, "text": "", "points_at": null, "model_options": options})));
    };
    let mut brief = state.briefs.brief(&config, &chosen).await.map_err(hermes_error)?;
    brief.insert("model_options".into(), options);
    Ok(web::ok(Value::Object(brief)))
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> ApiResult<T> {
    let result = tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)?;
    Ok(result.map_err(anyhow::Error::from)?)
}

async fn live_call(State(state): State<AppState>, Json(body): Json<CallEnded>) -> ApiResult<Response> {
    let agent = setup(&state, &body.agent)?;
    let briefs = state.briefs.clone();
    blocking(move || briefs.note_call(&agent.name, &body.session, &body.live_session)).await?;
    Ok(web::ok(json!({"ok": true})))
}

async fn keep_recap(State(state): State<AppState>, Json(body): Json<CallRecap>) -> ApiResult<Response> {
    let agent = setup(&state, &body.agent)?;
    let briefs = state.briefs.clone();
    blocking(move || briefs.keep_recap(&agent.name, &body.session, &body.recap)).await?;
    Ok(web::ok(json!({"ok": true})))
}

async fn take_recap(State(state): State<AppState>, Json(body): Json<RecapTaken>) -> ApiResult<Response> {
    let agent = setup(&state, &body.agent)?;
    let briefs = state.briefs.clone();
    let config = state.config();
    let recap = blocking(move || {
        // The family news waiting for the agent (family_news.rs) joins the same message, the same way.
        let parts: Vec<String> =
            [briefs.take_recap(&agent.name, &body.session), crate::family_news::take_notes(&config, &agent.name)]
                .into_iter()
                .flatten()
                .collect();
        Ok((!parts.is_empty()).then(|| parts.join("\n")))
    })
    .await?;
    Ok(web::ok(json!({"recap": recap})))
}

/// `live_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/live/brief", get(live_brief))
        .route("/live/calls", post(live_call))
        .route("/live/recaps", post(keep_recap))
        .route("/live/recaps/take", post(take_recap))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffixes_as_pathlib() {
        assert_eq!(with_suffix("/a/b/x.json", ".tmp"), "/a/b/x.tmp");
        assert_eq!(with_suffix("/a/b/.json", ".tmp"), "/a/b/.json.tmp");
        assert_eq!(with_suffix("/a/b/a.b.txt", ".tmp"), "/a/b/a.b.tmp");
    }

    #[test]
    fn hidden_parts_and_transcript() {
        assert_eq!(visible("Salut <!-- bip:call\nopen --> toi "), "Salut  toi");
        let pairs =
            vec![("user".to_string(), "un deux trois".to_string()), ("assistant".to_string(), "a  b\nc".to_string())];
        assert_eq!(transcript(&pairs, 9), "- The person: un deux…\n- You: a b c");
        assert_eq!(safe_name("ab/c d.é-1"), "ab_c_d.é-1");
    }
}
