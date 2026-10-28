//! Live progress of an agent's long work on the person's phone (decided 2026-10-26, after « Muze »): the iPhone's Live
//! Activity (Dynamic Island, lock screen) and Android's ongoing progress notification.
//!
//! The app starts the activity itself (iOS only lets an app in front start one) when a long task starts: a background
//! task (« Tâche de fond », jobs.rs), a conversation's run that goes on, a group discussion (rooms.rs). While the app runs
//! it updates the activity on its own; once it is in the background, the bridge does, through APNs `liveactivity`
//! pushes to the activity's own push token (Android: an FCM data message to the device's token, the app rebuilds its
//! notification). The app registers that token here:
//!
//!     POST   /v1/live-activities           {agent, run_id | job_id | room_id, token, environment?}  → {id, target, state}
//!     DELETE /v1/live-activities/{token}   the activity ended on the phone (or the person dismissed it)
//!
//! `environment` as for `/v1/devices`: « sandbox » / « production » (an iPhone's activity token, APNs), « fcm » (an
//! Android device's FCM token), « relay » (a token sealed by the publisher's relay). Kept in memory only: an activity
//! lives a few hours at most, and one the bridge forgot after a restart is updated by the app when it opens.
//!
//! **What is sent** (`content-state`, decoded by the app's `LiveTaskAttributes.ContentState`): the phase (`working`,
//! `approval`, `question`, `done`, `failed`, `stopped`), the running tool's name (the app words it, as its working line:
//! « Je compare les prix… »), the sub-tasks (`delegate_task`'s sub-agents) started and done, and in a group discussion
//! the agent speaking. Never what the agent says; the site a browser is on only with `[push] previews` (the title the
//! person sees comes from the app, set when it started the activity: it never goes through Apple).
//!
//! **How often**: a step at most every `MIN_GAP_SECONDS` per activity (the latest one sent at the end of the gap, never
//! a backlog), a change of phase at once (an approval, a question), and the end as APNs' `end` event with a dismissal a
//! few minutes later (« Ton résultat est prêt »). Step updates go at priority 5 (not counted in Apple's budget), the
//! phases and the end at 10. While the app follows the run (its SSE is attached), steps aren't pushed: it updates the
//! activity itself.
//!
//! **Relay**: a bridge with its own Apple key sends straight to APNs (topic `<bundle>.push-type.liveactivity`); one
//! relaying through an administrator's bridge (`[push] relay_url`) passes `push_type: liveactivity` on (relay.rs takes
//! it). The publisher's relay (relay/) forwards them since 2026-10-26 (its `scrub_live` keeps codes only: no `detail`);
//! a relay not updated yet refuses them (400), logged once, and the app's own updates still work. Android needs nothing
//! new on either relay.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{delete, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::apns::SendOptions;
use crate::app::AppState;
use crate::logs::redact6;
use crate::push::PushService;
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::Dict;

/// At most one step update per activity in this many seconds.
pub const MIN_GAP_SECONDS: f64 = 5.0;
/// The finished activity stays this long on the lock screen (« Ton résultat est prêt »), then iOS removes it.
pub const DISMISS_AFTER_SECONDS: f64 = 4.0 * 60.0;
/// iOS ends an activity after 8 hours (12 on the lock screen): a registration is forgotten after that.
pub const MAX_AGE_SECONDS: f64 = 12.0 * 3600.0;
/// An update older than this shows as stale (the bridge stopped hearing from the run).
pub const STALE_AFTER_SECONDS: f64 = 30.0 * 60.0;
/// Registrations kept at most (the oldest go first).
pub const MAX_ENTRIES: usize = 64;

/// The tool Hermes' sub-agents run under (runs.rs).
const DELEGATE: &str = "delegate_task";

// -- the state an activity shows --------------------------------------------------------------------------------------

/// What an activity shows now, built from its run's events (the same events the app's working line reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveState {
    /// `working`, `approval`, `question`, `done`, `failed`, `stopped`.
    pub phase: String,
    /// The tool running (its raw name: the apps word it), none while the agent thinks.
    pub tool: Option<String>,
    /// The site the browser or a web read is on (« kiabi.fr »): sent only with `[push] previews`.
    pub detail: Option<String>,
    /// In a group discussion: the agent whose turn it is (its display name).
    pub speaker: Option<String>,
    pub subtasks: u32,
    pub subtasks_done: u32,
}

impl Default for LiveState {
    fn default() -> Self {
        LiveState { phase: "working".into(), tool: None, detail: None, speaker: None, subtasks: 0, subtasks_done: 0 }
    }
}

/// What an event changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    None,
    /// The step (tool, sub-tasks, speaker): throttled.
    Step,
    /// The phase (an approval, a question, back to work): sent at once.
    Phase,
    /// The run ended: the activity's end.
    End,
}

impl LiveState {
    pub fn finished(&self) -> bool {
        matches!(self.phase.as_str(), "done" | "failed" | "stopped")
    }

    /// The app's `ContentState` (camelCase, Swift's synthesized Codable): ints always, the rest only when known.
    pub fn content_state(&self, previews: bool) -> Value {
        let mut state = Map::new();
        state.insert("phase".into(), json!(self.phase));
        if let Some(tool) = &self.tool {
            state.insert("tool".into(), json!(tool));
        }
        if let (true, Some(detail)) = (previews, &self.detail) {
            state.insert("detail".into(), json!(detail));
        }
        if let Some(speaker) = &self.speaker {
            state.insert("speaker".into(), json!(speaker));
        }
        state.insert("subtasks".into(), json!(self.subtasks));
        state.insert("subtasksDone".into(), json!(self.subtasks_done));
        Value::Object(state)
    }

    /// Takes one run event in. `room`: a group discussion's turn (its end is not the activity's: the next agent may
    /// speak; rooms.rs ends it when nobody is left to answer).
    pub fn apply(&mut self, kind: &str, data: &Map<String, Value>, room: bool) -> Change {
        let text = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let before = self.clone();
        let back_to_work = |state: &mut LiveState| {
            if state.phase == "approval" || state.phase == "question" {
                state.phase = "working".into();
            }
        };
        match kind {
            "tool.started" => {
                let tool = if text("tool").is_empty() { text("name") } else { text("tool") };
                if tool.is_empty() {
                    return Change::None;
                }
                self.detail = site(&text("preview")).filter(|_| looks_like_web(&tool));
                self.tool = Some(crate::push::pyx::head(&tool, 64).to_string());
                back_to_work(self);
            }
            "subagent.start" | "subagent.started" => {
                self.subtasks += 1;
                self.tool = Some(DELEGATE.into());
                self.detail = None;
                back_to_work(self);
            }
            "subagent.complete" | "subagent.completed" => {
                self.subtasks_done += 1;
                self.subtasks = self.subtasks.max(self.subtasks_done);
            }
            "approval.request" => self.phase = "approval".into(),
            "clarify.request" => self.phase = "question".into(),
            "approval.responded" | "clarify.responded" | "clarify.cancelled" | "clarify.retired" => back_to_work(self),
            _ if crate::hermes::is_terminal(kind) => {
                if room {
                    self.tool = None;
                    self.detail = None;
                    back_to_work(self);
                } else {
                    self.phase = match kind {
                        "run.completed" | "done" => "done",
                        "run.cancelled" | "run.interrupted" => "stopped",
                        _ => "failed",
                    }
                    .into();
                    return Change::End;
                }
            }
            _ => return Change::None,
        }
        if self.phase != before.phase {
            Change::Phase
        } else if *self != before {
            Change::Step
        } else {
            Change::None
        }
    }
}

fn looks_like_web(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    tool.contains("browser") || tool.contains("stealth") || tool.contains("web") || tool.contains("fetch")
}

/// The host of the first web address in a tool's preview, « www. » dropped.
pub fn site(preview: &str) -> Option<String> {
    let start = preview.find("http://").map(|i| i + 7).or_else(|| preview.find("https://").map(|i| i + 8))?;
    let host: String = preview[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-')
        .collect::<String>()
        .to_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).trim_end_matches('.').to_string();
    (host.contains('.') && host.len() <= 80).then_some(host)
}

/// When a step may go: now, or after how long (the end of the gap since the last one sent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gate {
    Now,
    Later(f64),
}

/// The throttle: phases and the end go at once; a step waits for `gap` seconds since the last update sent.
pub fn gate(now: f64, last_sent: Option<f64>, gap: f64, change: Change) -> Gate {
    match (change, last_sent) {
        (Change::Phase | Change::End, _) | (_, None) => Gate::Now,
        (_, Some(last)) if now - last >= gap => Gate::Now,
        (_, Some(last)) => Gate::Later(gap - (now - last)),
    }
}

// -- the registrations ------------------------------------------------------------------------------------------------

/// What an activity follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Run(String),
    Job(String),
    Room(String),
}

impl Target {
    fn view(&self) -> Value {
        match self {
            Target::Run(id) => json!({"run_id": id}),
            Target::Job(id) => json!({"job_id": id}),
            Target::Room(id) => json!({"room_id": id}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub agent: String,
    pub target: Target,
    pub token: String,
    pub environment: String,
    pub created: f64,
    pub state: LiveState,
    /// The state last sent, and when.
    pub sent: Option<LiveState>,
    pub last_sent: Option<f64>,
    /// APNs ignores an update whose timestamp isn't newer: one per second at least.
    pub last_timestamp: i64,
    /// A deferred step is waiting for the end of the gap.
    pub timer: bool,
    pub ended: bool,
    /// The run a background task's activity follows now (a resumed task runs again under a new id).
    pub run: Option<String>,
    /// An Android device (FCM, or FCM through the publisher's relay): its updates are data messages, not APNs'.
    pub android: bool,
}

/// The registrations, in memory.
pub struct Registry {
    entries: Mutex<Vec<Entry>>,
    /// The gap between two step updates (tests shorten it).
    gap: Mutex<f64>,
    /// Refusals already logged (one line per kind, not per update).
    logged: Mutex<HashSet<String>>,
}

impl Default for Registry {
    fn default() -> Self {
        Registry { entries: Mutex::new(Vec::new()), gap: Mutex::new(MIN_GAP_SECONDS), logged: Mutex::default() }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Registry {
    pub fn set_gap(&self, seconds: f64) {
        *lock(&self.gap) = seconds;
    }

    pub fn gap(&self) -> f64 {
        *lock(&self.gap)
    }

    /// A token registered (again): replaces its earlier registration.
    pub fn register(&self, entry: Entry) {
        let mut entries = lock(&self.entries);
        let now = crate::py::time_time();
        entries.retain(|e| e.token != entry.token && !e.ended && now - e.created < MAX_AGE_SECONDS);
        entries.push(entry);
        while entries.len() > MAX_ENTRIES {
            entries.remove(0);
        }
    }

    pub fn forget(&self, token: &str) -> bool {
        let mut entries = lock(&self.entries);
        let before = entries.len();
        entries.retain(|e| e.token != token);
        entries.len() != before
    }

    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<Entry> {
        lock(&self.entries).clone()
    }

    fn log_once(&self, key: &str) -> bool {
        lock(&self.logged).insert(key.to_string())
    }
}

/// What to send for one registration.
struct Outgoing {
    android: bool,
    token: String,
    environment: String,
    agent: String,
    key: String,
    state: LiveState,
    end: bool,
    urgent: bool,
    timestamp: i64,
}

/// The APNs `liveactivity` payload.
pub fn apns_payload(state: &LiveState, previews: bool, timestamp: i64, end: bool, now: f64) -> Value {
    let mut aps = Map::new();
    aps.insert("timestamp".into(), json!(timestamp));
    aps.insert("event".into(), json!(if end { "end" } else { "update" }));
    aps.insert("content-state".into(), state.content_state(previews));
    if end {
        aps.insert("dismissal-date".into(), json!((now + DISMISS_AFTER_SECONDS) as i64));
    } else {
        aps.insert("stale-date".into(), json!((now + STALE_AFTER_SECONDS) as i64));
    }
    json!({"aps": aps})
}

/// The FCM data for Android's progress notification (strings only, as fcm.rs turns them; `silent` so older apps
/// show nothing).
pub fn android_payload(agent: &str, key: &str, state: &LiveState, previews: bool, end: bool) -> Value {
    let mut payload = json!({
        "aps": {"content-available": 1},
        "agent": agent,
        "reason": "live",
        "live": key,
        "phase": state.phase,
        "subtasks": state.subtasks.to_string(),
        "subtasks_done": state.subtasks_done.to_string(),
        "end": if end { "1" } else { "0" },
    });
    if let Some(tool) = &state.tool {
        payload["tool"] = json!(tool);
    }
    if let (true, Some(detail)) = (previews, &state.detail) {
        payload["detail"] = json!(detail);
    }
    if let Some(speaker) = &state.speaker {
        payload["speaker"] = json!(speaker);
    }
    payload
}

fn key_of(target: &Target) -> String {
    match target {
        Target::Run(id) => format!("run:{id}"),
        Target::Job(id) => format!("job:{id}"),
        Target::Room(id) => format!("room:{id}"),
    }
}

impl PushService {
    /// A run event (runs.rs): every activity following that run — its own, its background task's, its group
    /// discussion's — takes it in and is updated, throttled. `followed`: the app shows the run now (its SSE is
    /// attached) and updates the activity itself, so steps aren't pushed.
    pub fn live_event(
        self: &Arc<Self>,
        agent: &str,
        run_id: &str,
        room_id: Option<&str>,
        kind: &str,
        data: &Map<String, Value>,
        followed: bool,
    ) {
        if self.live.is_empty() {
            return;
        }
        let config = self.config.get();
        let speaker =
            room_id.map(|_| config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or(agent.into()));
        let now = crate::py::time_time();
        let gap = self.live.gap();
        let mut send = Vec::new();
        let mut later = Vec::new();
        {
            let mut entries = lock(&self.live.entries);
            for entry in entries.iter_mut().filter(|e| !e.ended) {
                let room = match &entry.target {
                    Target::Run(id) => {
                        if entry.agent != agent || id != run_id {
                            continue;
                        }
                        false
                    }
                    Target::Job(job_id) => {
                        if entry.agent != agent {
                            continue;
                        }
                        if entry.run.as_deref() != Some(run_id) {
                            let current = crate::jobs::load(&config, job_id)
                                .and_then(|job| job.get("run_id").and_then(Value::as_str).map(str::to_string));
                            if current.as_deref() != Some(run_id) {
                                continue;
                            }
                            entry.run = current;
                        }
                        false
                    }
                    Target::Room(id) => {
                        if room_id != Some(id.as_str()) {
                            continue;
                        }
                        true
                    }
                };
                let mut change = entry.state.apply(kind, data, room);
                if room && entry.state.speaker != speaker && !crate::hermes::is_terminal(kind) {
                    entry.state.speaker = speaker.clone();
                    if change == Change::None {
                        change = Change::Step;
                    }
                }
                if change == Change::None || (change == Change::Step && followed) {
                    continue;
                }
                match gate(now, entry.last_sent, gap, change) {
                    Gate::Now => send.push(outgoing(entry, now, change)),
                    Gate::Later(delay) => {
                        if !entry.timer {
                            entry.timer = true;
                            later.push((entry.token.clone(), delay));
                        }
                    }
                }
            }
        }
        self.live_send(send);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        for (token, delay) in later {
            let this = self.clone();
            runtime.spawn(async move {
                tokio::time::sleep(Duration::from_secs_f64(delay.max(0.0))).await;
                this.live_flush(&token);
            });
        }
    }

    /// A group discussion has nobody left to answer (rooms.rs): its activity ends (« Ton résultat est prêt »).
    pub fn live_room_idle(self: &Arc<Self>, room_id: &str) {
        if self.live.is_empty() {
            return;
        }
        let now = crate::py::time_time();
        let mut send = Vec::new();
        {
            let mut entries = lock(&self.live.entries);
            for entry in entries.iter_mut().filter(|e| !e.ended && e.target == Target::Room(room_id.to_string())) {
                entry.state.phase = "done".into();
                entry.state.tool = None;
                entry.state.speaker = None;
                send.push(outgoing(entry, now, Change::End));
            }
        }
        self.live_send(send);
    }

    /// The end of a gap: the latest step, if it changed since the last one sent.
    fn live_flush(self: &Arc<Self>, token: &str) {
        let now = crate::py::time_time();
        let mut send = Vec::new();
        {
            let mut entries = lock(&self.live.entries);
            if let Some(entry) = entries.iter_mut().find(|e| e.token == token) {
                entry.timer = false;
                if !entry.ended && entry.sent.as_ref() != Some(&entry.state) {
                    send.push(outgoing(entry, now, Change::Step));
                }
            }
        }
        self.live_send(send);
    }

    fn live_send(self: &Arc<Self>, outgoing: Vec<Outgoing>) {
        if outgoing.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        for item in outgoing {
            let this = self.clone();
            runtime.spawn(async move { this.live_deliver(item).await });
        }
    }

    async fn live_deliver(self: Arc<Self>, item: Outgoing) {
        let previews = self.config.get().push_previews;
        let mut device = Dict::new();
        device.insert("environment".into(), json!(item.environment));
        let Some(sender) = self.sender(&device) else {
            if self.live.log_once(&format!("nosender:{}", item.environment)) {
                tracing::info!(env = %item.environment, "live activity: no push sender for this environment");
            }
            return;
        };
        let android = item.android;
        let now = crate::py::time_time();
        let (payload, push_type, priority) = if android {
            (
                android_payload(&item.agent, &item.key, &item.state, previews, item.end),
                "background",
                if item.urgent { 10 } else { 5 },
            )
        } else {
            (
                apns_payload(&item.state, previews, item.timestamp, item.end, now),
                "liveactivity",
                if item.urgent { 10 } else { 5 },
            )
        };
        let options = SendOptions {
            environment: item.environment.clone(),
            push_type: push_type.into(),
            priority,
            collapse_id: None,
            expiration: None,
        };
        let result = match sender.try_send(&item.token, &payload, &options).await {
            Ok(result) => result,
            Err(exc) => {
                tracing::warn!(error = crate::push::pyx::error_name(&exc), "live activity push crashed");
                return;
            }
        };
        if result.ok() {
            tracing::info!(token = %redact6(&item.token), phase = %item.state.phase, end = item.end, "live activity push");
        } else if result.token_is_dead() {
            self.live.forget(&item.token);
            tracing::info!(token = %redact6(&item.token), "live activity token removed");
        } else if self.live.log_once(&format!("{}:{}", item.environment, result.status)) {
            // the publisher's relay refuses `liveactivity` (400) until it forwards them: said once
            tracing::warn!(env = %item.environment, status = result.status, reason = ?result.reason,
                "live activity push refused");
        }
    }
}

fn outgoing(entry: &mut Entry, now: f64, change: Change) -> Outgoing {
    let end = change == Change::End;
    let timestamp = (now as i64).max(entry.last_timestamp + 1);
    entry.last_timestamp = timestamp;
    entry.last_sent = Some(now);
    entry.sent = Some(entry.state.clone());
    if end {
        entry.ended = true;
    }
    Outgoing {
        android: entry.android,
        token: entry.token.clone(),
        environment: entry.environment.clone(),
        agent: entry.agent.clone(),
        key: key_of(&entry.target),
        state: entry.state.clone(),
        end,
        urgent: change != Change::Step,
        timestamp,
    }
}

// -- routes -----------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct LiveRequest {
    pub agent: String,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
    #[serde(default)]
    pub room_id: Option<String>,
    pub token: String,
    #[serde(default)]
    pub environment: Option<String>,
    /// Android: the device's platform when the token is a relay token (`fcm`), as the app knows it.
    #[serde(default)]
    pub platform: Option<String>,
}

impl Validate for LiveRequest {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "token", Some(&self.token), 8192);
        check_max_len(errors, "run_id", self.run_id.as_deref(), 256);
        check_max_len(errors, "job_id", self.job_id.as_deref(), 200);
        check_max_len(errors, "room_id", self.room_id.as_deref(), 200);
    }
}

fn given(value: &Option<String>) -> Option<String> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// The state so far of what the activity follows (a run's backlog replayed), so the first push carries no surprise.
fn state_so_far(state: &AppState, agent: &str, target: &Target) -> (LiveState, Option<String>) {
    let config = state.config();
    let (run, room) = match target {
        Target::Run(id) => (Some(id.clone()), false),
        Target::Job(id) => (
            crate::jobs::load(&config, id)
                .and_then(|job| job.get("run_id").and_then(Value::as_str).map(str::to_string)),
            false,
        ),
        Target::Room(_) => (None, true),
    };
    let mut live = LiveState::default();
    if let Some(sub) = run.as_deref().and_then(|run| state.hub.subscription(agent, run)) {
        let st = sub.state.lock().unwrap_or_else(|e| e.into_inner());
        for event in &st.backlog {
            live.apply(&event.r#type, &event.data, room);
        }
    }
    (live, run)
}

async fn register(State(state): State<AppState>, Json(body): Json<LiveRequest>) -> ApiResult<Response> {
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&body.agent))?;
    let targets: Vec<Target> = [
        given(&body.run_id).map(Target::Run),
        given(&body.job_id).map(Target::Job),
        given(&body.room_id).map(Target::Room),
    ]
    .into_iter()
    .flatten()
    .collect();
    let [target] = targets.as_slice() else {
        return Err(HttpError::new(400, "one of run_id, job_id or room_id"));
    };
    let valid_id = |id: &str| crate::app::RUN_ID.is_match(id);
    match target {
        Target::Run(id) | Target::Job(id) | Target::Room(id) if !valid_id(id) => {
            return Err(HttpError::new(400, "invalid id"));
        }
        _ => {}
    }
    let mut token = body.token.trim().replace([' ', '<', '>'], "");
    let environment = given(&body.environment).unwrap_or_else(|| config.apns.environment.clone());
    match environment.as_str() {
        "relay" => {
            if state.push_service.is_none() {
                return Err(HttpError::new(409, "this bridge has no push relay set up"));
            }
            if !crate::relay::relay_token_taken(&token, crate::relay::unix_now()) {
                return Err(HttpError::new(400, "invalid relay token"));
            }
        }
        "fcm" => {
            if !crate::fcm::FCM_TOKEN.is_match(&token) {
                return Err(HttpError::new(400, "invalid FCM token"));
            }
        }
        "sandbox" | "production" => {
            if !crate::app::DEVICE_TOKEN.is_match(&token) {
                return Err(HttpError::new(400, "invalid push token (hex expected)"));
            }
            token = token.to_lowercase();
        }
        _ => return Err(HttpError::new(400, "environment must be sandbox, production, fcm or relay")),
    }
    let (live, run) = state_so_far(&state, &agent.name, target);
    let android = environment == "fcm" || body.platform.as_deref() == Some("fcm");
    let entry = Entry {
        agent: agent.name.clone(),
        target: target.clone(),
        token: token.clone(),
        environment: environment.clone(),
        created: crate::py::time_time(),
        sent: Some(live.clone()),
        state: live.clone(),
        last_sent: None,
        last_timestamp: 0,
        timer: false,
        ended: false,
        run,
        android,
    };
    state.push.live.register(entry);
    tracing::info!(agent = %agent.name, token = %redact6(&token), env = %environment, target = %key_of(target),
        "live activity registered");
    let mut view = target.view();
    view["agent"] = json!(agent.name);
    view["state"] = live.content_state(true);
    Ok(web::ok(view))
}

async fn unregister(State(state): State<AppState>, UrlPath(token): UrlPath<String>) -> ApiResult<Response> {
    let token = token.trim().to_string();
    if state.push.live.forget(&token) || state.push.live.forget(&token.to_lowercase()) {
        tracing::info!(token = %redact6(&token), "live activity forgotten");
    }
    Ok(web::no_content())
}

pub fn router() -> Router<AppState> {
    Router::new().route("/live-activities", post(register)).route("/live-activities/{token}", delete(unregister))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn the_state_follows_the_runs_events() {
        let mut s = LiveState::default();
        assert_eq!(s.apply("message.delta", &data(json!({"delta": "x"})), false), Change::None);
        // the agent's first words (« Je regarde ça… », ACK_RULE) and its lines between tools are not the run's end
        for kind in ["assistant.completed", "message.completed", "assistant.delta", "reasoning.available"] {
            assert_eq!(s.apply(kind, &data(json!({"text": "Je regarde ça…"})), false), Change::None);
        }
        assert_eq!(s.phase, "working");
        assert_eq!(
            s.apply(
                "tool.started",
                &data(json!({"tool": "browser_navigate", "preview": "https://www.kiabi.fr/x"})),
                false
            ),
            Change::Step
        );
        assert_eq!(s.tool.as_deref(), Some("browser_navigate"));
        assert_eq!(s.detail.as_deref(), Some("kiabi.fr"));
        // the same step again changes nothing; a completed call keeps its line until the next one
        assert_eq!(
            s.apply(
                "tool.started",
                &data(json!({"tool": "browser_navigate", "preview": "https://www.kiabi.fr/x"})),
                false
            ),
            Change::None
        );
        assert_eq!(s.apply("tool.completed", &data(json!({"tool": "browser_navigate"})), false), Change::None);
        assert_eq!(s.apply("subagent.start", &data(json!({"goal": "g"})), false), Change::Step);
        assert_eq!(s.apply("subagent.start", &data(json!({"goal": "h"})), false), Change::Step);
        assert_eq!(s.apply("subagent.complete", &data(json!({})), false), Change::Step);
        assert_eq!((s.subtasks, s.subtasks_done, s.tool.as_deref()), (2, 1, Some("delegate_task")));
        assert_eq!(s.apply("approval.request", &data(json!({"request_id": "r"})), false), Change::Phase);
        assert_eq!(s.phase, "approval");
        assert_eq!(s.apply("approval.responded", &data(json!({})), false), Change::Phase);
        assert_eq!(s.apply("clarify.request", &data(json!({})), false), Change::Phase);
        assert_eq!(s.apply("tool.started", &data(json!({"tool": "web_search"})), false), Change::Phase);
        assert_eq!(s.phase, "working");
        assert_eq!(s.detail, None);
        assert_eq!(s.apply("run.completed", &data(json!({"output": "ok"})), false), Change::End);
        assert_eq!(s.phase, "done");
        let mut failed = LiveState::default();
        assert_eq!(failed.apply("run.failed", &Map::new(), false), Change::End);
        assert_eq!(failed.phase, "failed");
        let mut stopped = LiveState::default();
        stopped.apply("run.cancelled", &Map::new(), false);
        assert_eq!(stopped.phase, "stopped");
        // a discussion's turn ending is not the discussion's end
        let mut room = LiveState { tool: Some("web_search".into()), ..LiveState::default() };
        assert_eq!(room.apply("run.completed", &Map::new(), true), Change::Step);
        assert_eq!((room.phase.as_str(), room.tool.as_deref()), ("working", None));
    }

    #[test]
    fn the_content_state_and_previews() {
        let s = LiveState {
            phase: "working".into(),
            tool: Some("browser_navigate".into()),
            detail: Some("kiabi.fr".into()),
            speaker: None,
            subtasks: 2,
            subtasks_done: 1,
        };
        assert_eq!(
            s.content_state(false),
            json!({"phase": "working", "tool": "browser_navigate", "subtasks": 2, "subtasksDone": 1})
        );
        assert_eq!(s.content_state(true)["detail"], "kiabi.fr");
        let p = apns_payload(&s, false, 1000, true, 2000.0);
        assert_eq!(p["aps"]["event"], "end");
        assert_eq!(p["aps"]["timestamp"], 1000);
        assert_eq!(p["aps"]["dismissal-date"], 2000 + DISMISS_AFTER_SECONDS as i64);
        assert!(p["aps"].get("alert").is_none()); // the reply's own notification tells it
        let u = apns_payload(&s, false, 1000, false, 2000.0);
        assert_eq!(u["aps"]["event"], "update");
        assert!(u["aps"].get("dismissal-date").is_none());
        let a = android_payload("vie", "job:j1", &s, false, false);
        assert_eq!(
            (a["reason"].as_str(), a["live"].as_str(), a["subtasks"].as_str(), a["end"].as_str()),
            (Some("live"), Some("job:j1"), Some("2"), Some("0"))
        );
        assert!(a.get("detail").is_none());
    }

    #[test]
    fn the_throttle() {
        assert_eq!(gate(100.0, None, 5.0, Change::Step), Gate::Now);
        assert_eq!(gate(100.0, Some(94.0), 5.0, Change::Step), Gate::Now);
        assert_eq!(gate(100.0, Some(98.0), 5.0, Change::Step), Gate::Later(3.0));
        assert_eq!(gate(100.0, Some(99.5), 5.0, Change::Phase), Gate::Now);
        assert_eq!(gate(100.0, Some(99.5), 5.0, Change::End), Gate::Now);
    }

    #[test]
    fn sites() {
        assert_eq!(site("{\"url\": \"https://www.Kiabi.fr/robes?x=1\"}").as_deref(), Some("kiabi.fr"));
        assert_eq!(site("no address"), None);
        assert_eq!(site("http://localhost:8080"), None);
    }
}
