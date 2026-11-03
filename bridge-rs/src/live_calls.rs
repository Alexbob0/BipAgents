//! What a Live call did lands in the conversation it continued, by the bridge, whatever the phone does after the call
//! (2026-10-31, Alex: the card the Bip was making during a call never arrived once he left the call).
//!
//! A call runs in its own short session (live_brief.rs). When it ends (hung up, the app quit, or nothing said for
//! `IDLE_SECONDS`), the bridge puts into the conversation it continued:
//!
//! - a call record the apps show at its time in the thread: « Appel de N min », the call's words to unfold, and the
//!   files, pages and cards the agent made during it (its `MEDIA:` lines), shown as in a written reply;
//! - a recap for the agent only (« <!--bip:call … --> »), joined to the next message written there (the recaps' way,
//!   live_brief.rs `keep_recap`), with the files made named in it so the agent can come back to them.
//!
//! A turn still going on at the hang-up is not stopped: the bridge waits for it (`TURN_WAIT_SECONDS` at most), the call
//! record shows « still working » meanwhile, then its answer and its files land there with a notification, like a
//! background task that ends.
//!
//! The apps say a call started (`POST /v1/live/calls/start`, which makes the call's session and lets the bridge end
//! it alone when the app is quit) and when it ended:
//!
//!     POST /v1/live/calls/start   {agent, session?, prelude?}  -> {ok, live_session}
//!     POST /v1/live/calls         {agent, session, live_session, run_id?, started_at?}  (live_brief.rs)
//!     GET  /v1/live/calls?agent=&session=  -> {calls: [record…]}  the conversation's calls, oldest first
//!
//! A record: `{id, live_session, started_at, ended_at, seconds, lines: [{role, text}], media: [path…], working, late,
//! ended_by}`. Kept in the data folder (« live-calls/<agent>/<session>.json », the last `RECORDS_KEPT`); the calls under
//! way in « live-calls/open.json ».

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::hermes::HermesError;
use crate::live_brief::{visible, Briefs};
use crate::voice_speech::pystr;
use crate::{py, Dict};

/// Nothing said in a call for this long, and no turn going on: it ended (the app quit, the phone off).
pub const IDLE_SECONDS: f64 = 600.0;
/// How often the calls under way are looked at.
pub const LOOP_SECONDS: f64 = 60.0;
/// A turn going on at the hang-up is waited for this long at most.
pub const TURN_WAIT_SECONDS: f64 = 1200.0;
/// How often that turn is looked at.
pub const POLL_SECONDS: f64 = 3.0;
/// A call under way forgotten past this (its session gone, a bridge long down).
pub const OPEN_KEPT_SECONDS: f64 = 2.0 * 86400.0;
/// The calls kept per conversation.
pub const RECORDS_KEPT: usize = 30;
/// One line of a record, at most (characters).
pub const LINE_CHARS: usize = 4000;
/// The words of a recap, at most (the apps' CallRecaps limit).
pub const RECAP_CHARS: usize = 2500;

static FILE_LOCK: Mutex<()> = Mutex::new(());
/// The calls being ended now (data folder, agent, live session): one end at a time per call.
static ENDING: LazyLock<Mutex<HashSet<(String, String, String)>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// `re.sub(r"[^\w.-]", "_", name)`, at most 120 characters.
fn safe_name(name: &str) -> String {
    let safe: String = name.chars().map(|c| if pystr::is_word(c) || c == '.' || c == '-' { c } else { '_' }).collect();
    pystr::head(&safe, 120)
}

/// A call under way.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenCall {
    pub agent: String,
    pub session: String,
    pub live_session: String,
    pub started_at: f64,
}

impl OpenCall {
    fn of(value: &Value) -> Option<OpenCall> {
        Some(OpenCall {
            agent: value.get("agent")?.as_str()?.to_string(),
            session: value.get("session")?.as_str()?.to_string(),
            live_session: value.get("live_session")?.as_str()?.to_string(),
            started_at: value.get("started_at")?.as_f64()?,
        })
    }

    fn json(&self) -> Value {
        json!({"agent": self.agent, "session": self.session, "live_session": self.live_session,
               "started_at": self.started_at})
    }
}

/// The calls' files, beside the briefs' folder.
pub struct LiveCalls {
    pub folder: String,
}

impl LiveCalls {
    pub fn new(folder: &str) -> Self {
        LiveCalls { folder: folder.to_string() }
    }

    /// Beside the briefs (« <data>/live-calls »).
    pub fn of(briefs: &Briefs) -> Self {
        LiveCalls::new(&py::join(&py::dirname(briefs.folder.trim_end_matches('/')), "live-calls"))
    }

    fn open_path(&self) -> String {
        py::join(&self.folder, "open.json")
    }

    fn records_path(&self, agent: &str, session: &str) -> String {
        py::join(&py::join(&self.folder, &safe_name(agent)), &format!("{}.json", safe_name(session)))
    }

    fn write(&self, path: &str, value: &Value) -> std::io::Result<()> {
        py::makedirs(&py::dirname(path), 0o700)?;
        let temporary = format!("{path}.tmp");
        py::write_with_mode(&temporary, py::Dumps::new().unicode().dumps(value).as_bytes(), 0o600)?;
        std::fs::rename(&temporary, path)
    }

    fn read_list(path: &str, key: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(map)) => map.get(key).and_then(Value::as_array).cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The calls under way.
    pub fn open_calls(&self) -> Vec<OpenCall> {
        let _guard = lock(&FILE_LOCK);
        Self::read_list(&self.open_path(), "calls").iter().filter_map(OpenCall::of).collect()
    }

    fn save_open(&self, calls: &[OpenCall]) -> std::io::Result<()> {
        let path = self.open_path();
        if calls.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        self.write(&path, &json!({"calls": calls.iter().map(OpenCall::json).collect::<Vec<_>>()}))
    }

    /// A call started (again: its start kept).
    pub fn note_start(&self, call: OpenCall) -> std::io::Result<()> {
        let _guard = lock(&FILE_LOCK);
        let mut calls: Vec<OpenCall> =
            Self::read_list(&self.open_path(), "calls").iter().filter_map(OpenCall::of).collect();
        if calls.iter().any(|c| c.agent == call.agent && c.live_session == call.live_session) {
            return Ok(());
        }
        calls.push(call);
        self.save_open(&calls)
    }

    /// The call under way, no longer (None: it was not noted).
    pub fn take_open(&self, agent: &str, live_session: &str) -> Option<OpenCall> {
        let _guard = lock(&FILE_LOCK);
        let mut calls: Vec<OpenCall> =
            Self::read_list(&self.open_path(), "calls").iter().filter_map(OpenCall::of).collect();
        let index = calls.iter().position(|c| c.agent == agent && c.live_session == live_session)?;
        let call = calls.remove(index);
        if let Err(err) = self.save_open(&calls) {
            tracing::warn!(error = %err, "live calls under way not saved");
        }
        Some(call)
    }

    /// The conversation's calls, oldest first.
    pub fn records(&self, agent: &str, session: &str) -> Vec<Value> {
        let _guard = lock(&FILE_LOCK);
        Self::read_list(&self.records_path(agent, session), "calls")
    }

    pub fn record(&self, agent: &str, session: &str, id: &str) -> Option<Value> {
        self.records(agent, session).into_iter().find(|r| r.get("id").and_then(Value::as_str) == Some(id))
    }

    /// A call's record, kept (in place of the same call's earlier one).
    pub fn keep(&self, agent: &str, session: &str, record: Value) -> std::io::Result<()> {
        let _guard = lock(&FILE_LOCK);
        let path = self.records_path(agent, session);
        let id = record.get("id").cloned();
        let mut records = Self::read_list(&path, "calls");
        match records.iter().position(|r| r.get("id") == id.as_ref()) {
            Some(index) => records[index] = record,
            None => records.push(record),
        }
        let start = records.len().saturating_sub(RECORDS_KEPT);
        self.write(&path, &json!({"calls": records[start..].to_vec()}))
    }
}

/// A message's text, its parts joined (hidden notes and `MEDIA:` lines still in).
fn raw_text(message: &Dict) -> String {
    match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" ")
        }
        _ => String::new(),
    }
}

/// What the call said (the person's and the agent's words, the opening and tools left out) and the files the agent
/// made in it (its `MEDIA:` paths, in order, once each).
pub fn call_lines(messages: &[Dict]) -> (Vec<(String, String)>, Vec<String>) {
    let mut lines = Vec::new();
    let mut media: Vec<String> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if !(role == "user" || role == "assistant")
            || message.get("display_kind").and_then(Value::as_str) == Some("hidden")
        {
            continue;
        }
        let (words, paths) = crate::media::extract_media(&raw_text(message));
        if role == "assistant" {
            for path in paths {
                if !media.contains(&path) {
                    media.push(path);
                }
            }
        }
        let text = visible(&words);
        if !text.is_empty() {
            lines.push((role.to_string(), pystr::head(&text, LINE_CHARS)));
        }
    }
    (lines, media)
}

/// A turn of the call is going on: the last message is the person's, a tool's, or the agent calling a tool.
pub fn turn_open(messages: &[Dict]) -> bool {
    let Some(last) = messages
        .iter()
        .rev()
        .find(|m| matches!(m.get("role").and_then(Value::as_str), Some("user" | "assistant" | "tool")))
    else {
        return false;
    };
    match last.get("role").and_then(Value::as_str) {
        Some("assistant") => last.get("tool_calls").is_some_and(py::truthy),
        _ => true,
    }
}

/// « 14:05 » in the person's time zone.
fn clock(config: &Config, at: f64) -> String {
    let utc = chrono::DateTime::from_timestamp(at as i64, 0).unwrap_or_default();
    match crate::dates::parse_zone(&crate::dates::load(config).time_zone) {
        Some(zone) => utc.with_timezone(&zone).format("%H:%M").to_string(),
        None => utc.with_timezone(&chrono::Local).format("%H:%M").to_string(),
    }
}

/// The recap for the agent of the call's lines from `from` on (those before were given already), as the apps wrote it
/// (CallRecaps.text), the files made in the call named. None when the person said nothing new.
pub fn recap(lines: &[(String, String)], media: &[String], from: usize, time: &str) -> Option<String> {
    let new = lines.get(from..).unwrap_or(&[]);
    if !new.iter().any(|(role, _)| role == "user") {
        return None;
    }
    let mut said = new
        .iter()
        .map(|(role, text)| {
            let who = if role == "user" { "The person" } else { "You" };
            format!("{who}: {}", pystr::split_ws(text).join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n");
    if pystr::len(&said) > RECAP_CHARS {
        said = pystr::head(&said, RECAP_CHARS) + "…";
    }
    let mut text = format!("<!--bip:call\nVoice call of {time} in between, which continued this conversation:\n{said}");
    if !media.is_empty() {
        let made: Vec<String> = media.iter().map(|p| format!("MEDIA:{p}")).collect();
        text.push_str(&format!("\nWhat you made during the call (the person sees it here):\n{}", made.join("\n")));
    }
    text.push_str("\n-->");
    Some(text)
}

/// How a call ended.
#[derive(Debug, Clone)]
pub struct Ending {
    pub agent: AgentConfig,
    /// The conversation the call continued.
    pub session: String,
    pub live_session: String,
    /// When it started (Unix seconds), as the app says; the call's session and its start notice also count.
    pub started_at: Option<f64>,
    /// When it ended: the hang-up, or its last activity.
    pub ended_at: f64,
    /// « hangup » or « idle ».
    pub ended_by: &'static str,
    /// The turn going on at the hang-up (the app's run id), waited for.
    pub run_id: Option<String>,
    pub poll: f64,
    pub turn_wait: f64,
}

impl Ending {
    pub fn hangup(agent: AgentConfig, session: &str, live_session: &str, run_id: Option<String>) -> Self {
        Ending {
            agent,
            session: session.to_string(),
            live_session: live_session.to_string(),
            started_at: None,
            ended_at: py::time_time(),
            ended_by: "hangup",
            run_id,
            poll: POLL_SECONDS,
            turn_wait: TURN_WAIT_SECONDS,
        }
    }
}

/// Is the turn under way still going on? (run status when Hermes has it, else the session's messages, else the hub)
async fn still_running(state: &AppState, ending: &Ending) -> bool {
    let agent = &ending.agent;
    if state.hub.session_busy(&agent.name, &ending.live_session) {
        return true;
    }
    if let Some(run_id) = &ending.run_id {
        match state.hermes.run(agent, run_id).await {
            Ok(run) => {
                let status = run.get("status").and_then(Value::as_str).unwrap_or("");
                if !status.is_empty() {
                    return !crate::jobs::TERMINAL.contains(&status);
                }
            }
            Err(HermesError::Http { .. }) => {} // a chat/stream turn Hermes keeps no run for: its messages say
            Err(_) => return true,              // Hermes unreachable a moment: again later
        }
    }
    match state.hermes.session_messages(agent, &ending.live_session).await {
        Ok(messages) => ending.run_id.is_some() && turn_open(&messages),
        Err(HermesError::Http { .. }) => false,
        Err(_) => true,
    }
}

/// The call lands in the conversation it continued (see the module's doc). Returns the record kept, None when there
/// was nothing to keep (nothing said) or the call is being ended already.
pub async fn end_call(state: &AppState, ending: Ending) -> Option<Value> {
    let key = (state.briefs.folder.clone(), ending.agent.name.clone(), ending.live_session.clone());
    if !lock(&ENDING).insert(key.clone()) {
        return None;
    }
    let result = end_inner(state, &ending).await;
    lock(&ENDING).remove(&key);
    match result {
        Ok(record) => record,
        Err(err) => {
            tracing::warn!(agent = %ending.agent.name, error = %err, "live call not put in its conversation");
            None
        }
    }
}

async fn end_inner(state: &AppState, ending: &Ending) -> anyhow::Result<Option<Value>> {
    let agent = &ending.agent;
    let calls = LiveCalls::of(&state.briefs);
    let opened = calls.take_open(&agent.name, &ending.live_session);
    let started_hermes = match state.hermes.session(agent, &ending.live_session).await {
        Ok(s) => crate::live_brief::seconds(s.get("started_at")),
        Err(_) => None,
    };
    // the earliest known: the app's own (its first sound), the call's session, the start notice (sent after both)
    let started_at = [ending.started_at, started_hermes, opened.as_ref().map(|o| o.started_at)]
        .into_iter()
        .flatten()
        .fold(ending.ended_at, f64::min);
    let earlier = calls.record(&agent.name, &ending.session, &ending.live_session);
    let recapped = earlier.as_ref().and_then(|r| r.get("recapped")).and_then(Value::as_u64).unwrap_or(0) as usize;
    let build = |lines: &[(String, String)], media: &[String], working: bool, late: bool, recapped: usize| {
        json!({
            "id": ending.live_session,
            "live_session": ending.live_session,
            "started_at": started_at,
            "ended_at": ending.ended_at,
            "seconds": (ending.ended_at - started_at).max(0.0).round(),
            "lines": lines.iter().map(|(role, text)| json!({"role": role, "text": text})).collect::<Vec<_>>(),
            "media": media,
            "working": working,
            "late": late,
            "ended_by": ending.ended_by,
            "recapped": recapped,
        })
    };

    // A turn going on: the call shows at once, « still working », then its answer when it comes.
    let mut late = false;
    if still_running(state, ending).await {
        if let Ok(messages) = state.hermes.session_messages(agent, &ending.live_session).await {
            let (lines, media) = call_lines(&messages);
            if lines.iter().any(|(role, _)| role == "user") {
                calls.keep(&agent.name, &ending.session, build(&lines, &media, true, false, recapped))?;
            }
        }
        let since = py::time_time();
        loop {
            tokio::time::sleep(Duration::from_secs_f64(ending.poll)).await;
            if !still_running(state, ending).await {
                late = true;
                break;
            }
            if py::time_time() - since >= ending.turn_wait {
                tracing::warn!(agent = %agent.name, "live call's last turn still going on, put in as it is");
                break;
            }
        }
    }

    let messages = match state.hermes.session_messages(agent, &ending.live_session).await {
        Ok(m) => m,
        Err(HermesError::Http { status: 404 | 410, .. }) => return Ok(None), // the call's session is gone
        Err(err) => return Err(err.into()),
    };
    let (lines, media) = call_lines(&messages);
    if !lines.iter().any(|(role, _)| role == "user") && media.is_empty() {
        return Ok(None); // the person said nothing (as the apps did: no recap then)
    }
    let config = state.config();
    let time = clock(&config, started_at);
    let note = recap(&lines, &media, recapped, &time);
    let record = build(&lines, &media, false, late, lines.len());
    calls.keep(&agent.name, &ending.session, record.clone())?;
    let (briefs, name, session, live) =
        (state.briefs.clone(), agent.name.clone(), ending.session.clone(), ending.live_session.clone());
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(note) = note {
            briefs.keep_recap(&name, &session, &note)?;
        }
        briefs.note_call(&name, &session, &live)
    })
    .await??;
    tracing::info!(agent = %agent.name, lines = lines.len(), media = media.len(), late = late, by = ending.ended_by,
                   "live call put in its conversation");

    // Its end told, like a background task's: the answer that came after the hang-up, or what was made while nobody
    // was in the app any more.
    if late || (ending.ended_by == "idle" && !media.is_empty()) {
        let answer =
            lines.iter().rev().find(|(role, _)| role == "assistant").map(|(_, t)| t.clone()).unwrap_or_default();
        let key = ending.run_id.clone().unwrap_or_else(|| format!("live:{}", ending.live_session));
        state.push.notify_reply(&agent.name, &key, &answer, Some(&ending.session)).await;
    }
    Ok(Some(record))
}

/// The calls nothing happened in for `IDLE_SECONDS` (the app quit, the phone off): ended here. Returns how many.
pub async fn end_idle(state: &AppState, now: f64) -> usize {
    let calls = LiveCalls::of(&state.briefs);
    let config = state.config();
    let mut ended = 0;
    for call in calls.open_calls() {
        let Some(agent) = config.agents.values().find(|a| a.name == call.agent).cloned() else {
            calls.take_open(&call.agent, &call.live_session); // the agent is gone
            continue;
        };
        if now - call.started_at > OPEN_KEPT_SECONDS {
            calls.take_open(&call.agent, &call.live_session);
            continue;
        }
        if state.hub.session_busy(&agent.name, &call.live_session) {
            continue;
        }
        let last = match state.hermes.session(&agent, &call.live_session).await {
            Ok(session) => crate::live_brief::seconds(session.get("last_active")).unwrap_or(call.started_at),
            Err(HermesError::Http { status: 404 | 410, .. }) => {
                calls.take_open(&call.agent, &call.live_session);
                continue;
            }
            Err(_) => continue, // Hermes down: later
        };
        let last = last.max(call.started_at);
        if now - last < IDLE_SECONDS {
            continue;
        }
        let ending = Ending {
            agent,
            session: call.session.clone(),
            live_session: call.live_session.clone(),
            started_at: Some(call.started_at),
            ended_at: last,
            ended_by: "idle",
            run_id: None,
            poll: POLL_SECONDS,
            turn_wait: 0.0,
        };
        if end_call(state, ending).await.is_some() {
            ended += 1;
        }
    }
    ended
}

/// Every minute, the calls left without an end.
pub async fn run_forever(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs_f64(LOOP_SECONDS)).await;
        end_idle(&state, py::time_time()).await;
    }
}

/// `GET /v1/live/calls` payload: the conversation's calls without their bookkeeping.
pub fn listed(records: Vec<Value>) -> Value {
    let calls: Vec<Value> = records
        .into_iter()
        .map(|mut r| {
            if let Some(map) = r.as_object_mut() {
                map.remove("recapped");
            }
            r
        })
        .collect();
    json!({"calls": calls})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(v: Value) -> Dict {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn lines_and_files_of_a_call() {
        let messages = vec![
            m(json!({"role": "user", "content": "<!--bip:call-start\nhello\n-->"})),
            m(json!({"role": "assistant", "content": "Salut !"})),
            m(json!({"role": "user", "content": "Fais-moi une carte des boulangeries"})),
            m(json!({"role": "assistant", "content": "", "tool_calls": [{"id": "1"}]})),
            m(json!({"role": "tool", "content": "{\"ok\": 1}"})),
            m(json!({"role": "assistant", "content": "La voilà.\nMEDIA:/pages/carte.html\n<!--bip:end-->"})),
        ];
        let (lines, media) = call_lines(&messages);
        assert_eq!(
            lines,
            vec![
                ("assistant".to_string(), "Salut !".to_string()),
                ("user".to_string(), "Fais-moi une carte des boulangeries".to_string()),
                ("assistant".to_string(), "La voilà.".to_string()),
            ]
        );
        assert_eq!(media, vec!["/pages/carte.html".to_string()]);
        assert!(!turn_open(&messages));
        assert!(turn_open(&messages[..3]));
        assert!(turn_open(&messages[..4]));
        assert!(turn_open(&messages[..5]));
    }

    #[test]
    fn a_recap_only_of_what_is_new() {
        let lines = vec![
            ("assistant".to_string(), "Salut".to_string()),
            ("user".to_string(), "je pars   à Lyon".to_string()),
            ("assistant".to_string(), "Bon voyage".to_string()),
        ];
        let text = recap(&lines, &[], 0, "10:05").unwrap();
        assert!(text.starts_with("<!--bip:call\nVoice call of 10:05 in between"));
        assert!(text.contains("The person: je pars à Lyon\nYou: Bon voyage\n-->"));
        assert_eq!(recap(&lines, &["/p/a.html".to_string()], 3, "10:05"), None);
        let with = recap(&lines, &["/p/a.html".to_string()], 0, "10:05").unwrap();
        assert!(with.contains("MEDIA:/p/a.html"));
    }
}
