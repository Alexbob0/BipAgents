//! Run hub: one upstream `/v1/runs/{id}/events` subscription per (agent, run), fanned out.
//!
//! Why one subscription: a Hermes run keeps a single event buffer, so two independent subscribers
//! (the voice WebSocket and the background approval watcher) could steal events from each other.
//! The hub keeps a replay backlog so a voice client that attaches after `POST /v1/watch` still
//! hears the answer from its beginning.
//!
//! Followers are the voice WebSocket and the app's SSE (`GET /v1/runs/{id}/events`): while one is attached
//! the user sees the run. Push rule: on `approval.request` for a *watched* run with no
//! follower, push an `APPROVAL` notification (once per request); when such a run completes, push a
//! « reply ready » alert that opens its conversation.
//!
//! Every run the bridge starts or relays is followed here, whoever looks at it (`keep`, 2026-10-27): the door's Hermes
//! relay (lan.rs) serves a run's events from the hub and follows the runs started through it, as does a peer's message
//! (peers.rs), so the brake (loop_brake.rs) counts and stops every turn, not only those the app follows here.
//!
//! Long-work safety net (2026-10-22, `[jobs] handoff`): a conversation's run that goes past ~15 tool calls or ~3
//! minutes is steered once to hand the rest over to a background task (`handoff`). Since 2026-10-24 (owner's report:
//! a clothes search handed to a sub-agent stopped silently, twice): never while a sub-agent (`delegate_task`) works
//! for the run, nor while the agent browses with its visible browser (the person watches it live; own thresholds,
//! never by default), and the time spent in either does not count. A steered run that still ends without handing over
//! has its task started by the bridge (`rescue`), the conversation told by a `bridge.notice` event before the end.
//!
//!
//! Concurrency (Python runs all this on one event loop): the hub's map of runs and its pending approvals sit
//! behind one lock, each run's state behind its own; the hub's lock is always taken first. Dispatching an event,
//! adding or removing a listener happen whole under these locks, as one step of Python's loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::StreamExt;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::{is_delta, is_terminal, HermesClient, HermesError, SSEEvent};
use crate::push::PushService;
use crate::store::Store;
use crate::Dict;

pub const BACKLOG_EVENTS: usize = 5000;
/// An app re-attaching later still replays the whole run.
pub const FINISHED_RETENTION_SECONDS: f64 = 900.0;
pub const BACKOFF_START: f64 = 0.5;
pub const BACKOFF_MAX: f64 = 8.0;
/// ~5 min of consecutive connection failures ends even a watch.
pub const GIVE_UP_AFTER: u32 = 40;
/// Consecutive connection failures before voice listeners are told.
pub const UNREACHABLE_AFTER: u32 = 3;
/// The safety net's steer starts with this (the apps hide such rows: HermesKit SteerMarker, Android Hermes.kt).
pub const HANDOFF_MARK: &str = "[bipagents:handoff]";
pub const HANDOFF_TEXT: &str = "This is long: hand the rest over to a background task now (start_task, within the \
default budget, or propose_task), tell your person in one sentence, and stop here.";
/// Tools that hand work over already (the agent's MCP tools, maybe with Hermes' `mcp_<server>_` prefix).
pub const HANDOFF_TOOLS: [&str; 3] = ["start_task", "propose_task", "notebook_fill"];
/// Hermes' sub-agents: the tool, and the lifecycle events its children send on the parent's run.
pub const DELEGATE_TOOL: &str = "delegate_task";
pub const SUBAGENT_START: &str = "subagent.start";
pub const SUBAGENT_DONE: &str = "subagent.complete";
/// The bridge's own words in a run's stream (after the safety net's rescue): `{code, text, job_id?, title?}`; the
/// apps show them as a notice. Codes: `handoff_started`, `handoff_proposed`, `handoff_failed`; the brake's
/// (loop_brake.rs) `loop_stopped` (`rescue: true` when a rescue turn follows), `loop_rescue_started` (`run_id`: the
/// rescue turn's, whose events follow in this same stream; nothing shown) and `loop_rescue_failed`.
pub const NOTICE: &str = "bridge.notice";
/// Each model call's real tokens as it ends (Hermes patch 0020): the call's `input_tokens`, the run's `totals`.
pub const MODEL_USAGE: &str = "model.usage";
/// Present while a run goes on, rewritten every minute (`RunHub::mark_busy_forever`), for the seat agent's beat.
pub const BUSY_MARKER: &str = ".local/state/bipagents/busy";

/// A run's real tokens so far, from its last `model.usage` event (Hermes patch 0020). An older Hermes gives a run's
/// tokens only at its end, none for a run stopped before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunUsage {
    /// The run's model calls so far.
    pub calls: u32,
    /// What they read (cache reads included), wrote, and read from the cache, in all.
    pub input: i64,
    pub output: i64,
    pub cached: i64,
    /// What the last call read: the next one reads at least as much (a turn's conversation only grows).
    pub last_input: i64,
}

impl RunUsage {
    /// A `model.usage` event's numbers (None without its `totals`).
    pub fn of(data: &Map<String, Value>) -> Option<RunUsage> {
        let totals = data.get("totals")?.as_object()?;
        let n = |m: &Map<String, Value>, k: &str| m.get(k).and_then(|v| crate::py::py_int(v).ok()).unwrap_or(0).max(0);
        Some(RunUsage {
            calls: n(data, "call") as u32,
            input: n(totals, "input_tokens"),
            output: n(totals, "output_tokens"),
            cached: n(totals, "cache_read_tokens"),
            last_input: n(data, "input_tokens"),
        })
    }

    pub fn total(&self) -> i64 {
        self.input + self.output
    }

    /// What the run will have taken once its next model call is done.
    pub fn after_next_call(&self) -> i64 {
        self.total() + self.last_input
    }

    /// In the shape of a run's terminal `usage` (jobs.rs counts a task's tokens from it).
    pub fn as_usage(&self) -> Dict {
        let mut usage = Dict::new();
        usage.insert("input_tokens".into(), json!(self.input));
        usage.insert("output_tokens".into(), json!(self.output));
        usage.insert("cache_read_tokens".into(), json!(self.cached));
        usage
    }
}

/// What the hub knows of a run it follows (a room's turn, rooms.rs; a background task, jobs.rs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunProgress {
    /// Its tool steps so far (`SubscriptionState::tool_steps`).
    pub tool_steps: u32,
    /// The brake stopped it.
    pub braked: bool,
    /// Its real tokens so far (None from an older Hermes).
    pub usage: Option<RunUsage>,
    /// The hub stopped it at its token limit (`limit_tokens`).
    pub capped: bool,
}

/// How far a chain of runs taking over from each other is followed (`followed_through`, `remove_listener`): a turn, its
/// continuation, its restart, a rescue turn… (a bound, never a cycle).
const CHAIN_MAX: usize = 6;

/// A run whose end on a promise may be continued (runs.rs `go_on`): a person's conversation turn an app started and
/// follows, not a background task's, a routine's or any unattended turn, a group discussion's, the brake's rescue
/// turn or a continuation itself, nor a turn the brake stopped, that handed work over or hit its token limit.
fn go_on_fits(st: &SubscriptionState) -> bool {
    !st.background
        && !st.unattended
        && st.rescue_of.is_none()
        && st.go_on_of.is_none()
        && !st.braked
        && !st.handed_off
        && !st.rescued
        && !st.capped
        && st.end_push
        && st.room_id.is_none()
        && !st.guest
        && !st.resumed
        && st.restarted_as.is_none()
        && st.restart_of.is_none()
        && (st.watched || st.ever_followed)
}

fn handoff_tool(tool: &str) -> bool {
    HANDOFF_TOOLS.iter().any(|t| tool == *t || tool.ends_with(&format!("_{t}")))
}

/// The agent's visible browser (skill visible-browser: the `stealth` MCP server's tools, `mcp_stealth_*`; the vault's
/// `browser_vault`, `vault_login` and `vault_*_login` on that same browser; Hermes' own `browser_*`).
pub fn browser_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    tool.contains("stealth")
        || tool.starts_with("browser_")
        || tool.contains("vault_create_login")
        || tool.ends_with("vault_login")
        || tool.contains("vault_confirm_login")
}

fn delegate_tool(tool: &str) -> bool {
    tool == DELEGATE_TOOL || tool.ends_with(&format!("_{DELEGATE_TOOL}"))
}

/// A handing-over tool that answered it did not (start_task's « Not started: … », a refused budget, no jobs here).
fn handoff_refused(event: &SSEEvent) -> bool {
    let preview = event.data.get("preview").and_then(Value::as_str).unwrap_or("");
    event.data.get("error").and_then(Value::as_bool).unwrap_or(false)
        || preview.contains("Not started")
        || preview.contains("aren't available here")
        || preview.contains("may take without your person's go")
        || preview.contains("enough of your proposals")
}

/// A steered run whose final words are at least this long answered after all: no rescue.
pub const RESCUE_REPLY_CHARS: usize = 600;
/// The push of a task the bridge started for a stopped run.
pub const RESCUE_BODY: &str = "Continue en tâche de fond";

/// The person's request in a conversation: its last message from them that is theirs (not the safety net's steer,
/// another bridge marker or a sub-task's report).
pub fn person_request(messages: &[Dict]) -> String {
    for message in messages.iter().rev() {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if role != "user" {
            continue;
        }
        let text = crate::cronwatch::message_text(message);
        if text.is_empty()
            || text.contains("[bipagents:")
            || crate::cronwatch::REPORT_PREFIXES.iter().any(|p| text.starts_with(p))
        {
            continue;
        }
        return text;
    }
    String::new()
}

/// The rescued task's title: the request's beginning (the person's words, so their language).
pub fn rescue_title(request: &str) -> String {
    let squeezed = crate::tools_knowledge::squeeze(request);
    let head = crate::tools_knowledge::head(&squeezed, 60).trim().to_string();
    if head.is_empty() {
        "Suite de la conversation".into()
    } else if head.chars().count() < squeezed.chars().count() {
        format!("{head}…")
    } else {
        head
    }
}

/// What the rescued task is told: the request, the steps already done in the conversation, the words said there.
pub fn rescue_prompt(request: &str, steps: &[String], said: &str) -> String {
    let mut out = String::from(
        "Your person asked you this in a conversation, and the conversation stopped before the work was finished:\n",
    );
    out.push_str(&format!(
        "« {} »\n",
        if request.is_empty() { "(their last request in that conversation)" } else { request }
    ));
    if !steps.is_empty() {
        let skip = steps.len().saturating_sub(30);
        out.push_str("\nWhat you already did there (don't redo it unless you need to):\n");
        if skip > 0 {
            out.push_str(&format!("- … {skip} earlier steps\n"));
        }
        for step in &steps[skip..] {
            out.push_str(step);
            out.push('\n');
        }
    }
    let said = said.trim();
    if !said.is_empty() {
        let chars: Vec<char> = said.chars().collect();
        let tail: String = chars[chars.len().saturating_sub(1500)..].iter().collect();
        out.push_str(&format!("\nWhat you had written there so far:\n{tail}\n"));
    }
    out.push_str("\nFinish the work here on your own, then end with your report.");
    out
}

/// A `POST /v1/runs` relayed to an agent's Hermes (the door's relay, lan.rs; a peer's message, peers.rs: `guest`): its
/// answer (a small JSON) read whole to follow the run it started (`RunHub::keep`, for the brake), and given back
/// unchanged.
pub async fn keep_started(
    hub: &Arc<RunHub>,
    agent: &AgentConfig,
    request: &[u8],
    response: axum::response::Response,
    guest: bool,
) -> axum::response::Response {
    if !response.status().is_success() {
        return response;
    }
    let (parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, 4 << 20).await else {
        let mut failed = axum::response::Response::new(axum::body::Body::from("Hermes' answer was lost"));
        *failed.status_mut() = axum::http::StatusCode::BAD_GATEWAY;
        return failed;
    };
    let answer: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let answer = answer.get("run").filter(|r| r.is_object()).unwrap_or(&answer);
    let run_id = answer.get("run_id").or_else(|| answer.get("id")).and_then(Value::as_str);
    let session = serde_json::from_slice::<Value>(request)
        .ok()
        .and_then(|b| b.get("session_id").and_then(Value::as_str).map(str::to_string));
    if let Some(run_id) = run_id.filter(|r| crate::app::RUN_ID.is_match(r)) {
        let sub = hub.keep(agent, run_id, session.as_deref());
        if guest {
            lock(&sub.state).guest = true;
        }
        tracing::info!(agent = %agent.name, run_id = %run_id, "relayed run followed");
    }
    axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Consecutive text deltas are merged in the replay backlog: a long reply is thousands of token
/// deltas, which would push its beginning out of the bounded backlog (a late follower then saw the
/// reply start mid-sentence). Live listeners still get every delta as it comes.
pub fn append_to_backlog(backlog: &mut VecDeque<SSEEvent>, event: &SSEEvent) {
    if let Some(last) = backlog.back_mut() {
        if let (Some(Value::String(delta)), Some(Value::String(before))) =
            (event.data.get("delta"), last.data.get("delta"))
        {
            if is_delta(&event.r#type) && last.r#type == event.r#type {
                let mut data = last.data.clone();
                data.insert("delta".into(), Value::String(format!("{before}{delta}")));
                let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
                *last = SSEEvent::new(event.r#type.clone(), data, raw);
                return;
            }
        }
    }
    if backlog.len() >= BACKLOG_EVENTS {
        backlog.pop_front();
    }
    backlog.push_back(event.clone());
}

/// An approval's or a question's card with the id of the timeline item it becomes once answered
/// (threads::answered_id): one row on every device, waiting then answered.
fn with_item_id(sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
    let request = if event.r#type == "clarify.request" { clarify_id(&event.data) } else { as_str(event.data.get("request_id")) };
    let mut data = event.data;
    data.insert("item_id".into(), json!(crate::threads::answered_id(Some(&sub.run_id), request.as_deref())));
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(event.r#type, data, raw)
}

/// Hermes' `todo.updated` (patch 0026: the agent's task list after each of its Todo calls) → the bridge's
/// `steps.updated`, the card's list in the apps' words (steps.rs, docs/conversations.md §6.3b). Hermes' own event is
/// not passed on.
fn as_steps(sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
    let Some(steps) = crate::steps::of_event(&event.data) else { return event };
    let mut data = Map::new();
    data.insert("event".into(), json!(crate::steps::EVENT));
    data.insert("run_id".into(), event.data.get("run_id").cloned().unwrap_or_else(|| json!(sub.run_id)));
    for key in ["timestamp", "seq"] {
        if let Some(value) = event.data.get(key) {
            data.insert(key.into(), value.clone());
        }
    }
    data.insert("steps".into(), steps);
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(crate::steps::EVENT, data, raw)
}

/// A turn's end whose output or error is Hermes' failed-turn line (« This turn did not complete… »): `stopped`
/// (`partial` | `not_processed`, threads::timeline::stopped_kind) says which, so the apps say it in their words without
/// reading Hermes' English (thin apps, 5/11).
fn with_stopped(event: SSEEvent) -> SSEEvent {
    let stopped = ["output", "error"]
        .iter()
        .filter_map(|k| as_str(event.data.get(*k)))
        .find_map(|text| crate::threads::timeline::stopped_kind(&text));
    let Some(stopped) = stopped else { return event };
    let mut data = event.data;
    data.insert("stopped".into(), json!(stopped));
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(event.r#type, data, raw)
}

/// `_clarify_id(data)`: a question's id (`request_id`: its prompt's, hermes_prompts.rs `named`; or `clarify_id`).
pub fn clarify_id(data: &Map<String, Value>) -> Option<String> {
    for key in ["request_id", "clarify_id"] {
        if let Some(Value::String(id)) = data.get(key) {
            return Some(id.clone());
        }
    }
    None
}

fn sha16(text: &str) -> String {
    let digest = hex::encode(Sha256::digest(text.as_bytes()));
    digest[..16].to_string()
}

fn as_str(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// One follower of a run (the voice WebSocket, an app's SSE, the bridge's own watch). Events arrive on `queue`
/// (the backlog first); `None` is the end. Dropped or `close`d, it stops following.
pub struct Listener {
    pub sub: Arc<RunSubscription>,
    pub kind: String,
    pub queue: mpsc::UnboundedReceiver<Option<SSEEvent>>,
    pub closed: bool,
    hub: Option<Arc<RunHub>>,
    id: u64,
}

impl Listener {
    /// The next event; `None` (or a closed queue) at the end of the run.
    pub async fn next(&mut self) -> Option<SSEEvent> {
        self.queue.recv().await.flatten()
    }

    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            if let Some(hub) = &self.hub {
                hub.remove_listener(self.id, &self.sub);
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.close();
    }
}

struct Slot {
    id: u64,
    kind: String,
    tx: mpsc::UnboundedSender<Option<SSEEvent>>,
}

/// A run the hub follows (one Hermes subscription, however many listeners).
pub struct RunSubscription {
    pub agent: AgentConfig,
    pub run_id: String,
    pub state: Mutex<SubscriptionState>,
}

pub struct SubscriptionState {
    pub watched: bool,
    pub finished: bool,
    pub finished_at: Option<f64>,
    pub terminal: Option<String>,
    /// For the « reply ready » push.
    pub session_id: Option<String>,
    /// The run's final text (run.completed), to recognise its reply.
    pub output: Option<String>,
    /// « reply ready » at its end (a background task tells its own end: jobs.py).
    pub end_push: bool,
    /// The replay backlog (`deque(maxlen=BACKLOG_EVENTS)`, consecutive deltas merged).
    pub backlog: VecDeque<SSEEvent>,
    /// The agent's task list as the run left it last (« Étapes », steps.rs): a background task's card reads it.
    pub steps: Option<Value>,
    /// The run's calls so far for the automatic list (steps.rs), its words' language, and whether the agent wrote a
    /// list of its own (it wins for the rest of the run).
    pub actions: Vec<crate::steps::Action>,
    pub steps_lang: Option<String>,
    pub own_steps: bool,
    /// What was pushed already (approvals by request, questions by `clarify:<id>`).
    pub pushed: HashSet<String>,
    /// The long-work safety net (`handoff`): when the hub began following the run, its tool calls so far, whether the
    /// agent already handed work over (start_task, propose_task, notebook_fill), and whether the bridge steered.
    pub followed_since: f64,
    pub tool_calls: u32,
    /// The run's tool steps (the calls started together before any ended count once: one model answer each), and
    /// the calls started and not ended yet (rooms.rs estimates a turn's tokens from the steps).
    pub tool_steps: u32,
    pub tools_open: u32,
    pub handed_off: bool,
    pub steered: bool,
    /// The tool calls that count for the safety net (neither browsing nor during a sub-agent's work; from 0 again
    /// after a sub-agent), and the run's last tool names (to tell a browsing run).
    pub counted_calls: u32,
    pub recent_tools: VecDeque<String>,
    /// `delegate_task` calls not answered yet, sub-agents started and not done (no steer meanwhile).
    pub delegating: u32,
    pub subagents: u32,
    /// The end of the last stretch that does not count (a sub-agent's work, browsing): the clock starts again there.
    pub exempt_until: f64,
    /// A handing-over tool answered it could not; the bridge's rescue was done (or tried).
    pub handoff_failed: bool,
    pub rescued: bool,
    /// The group discussion whose hidden conversation runs this (rooms.rs): its approvals open the room.
    pub room_id: Option<String>,
    /// The phone the person's message came from (« ios », « android »: threads/send.rs), when it said: where a
    /// reminder asked in this turn goes (reminders.rs `phone_for`).
    pub phone: Option<String>,
    /// The run called the agent's browser tools: at its end, its browser stops soon unless used again
    /// (browsers::run_ended).
    pub used_browser: bool,
    /// A background task's run (jobs.rs): the brake's higher limits (loop_brake.rs).
    pub background: bool,
    /// A turn nobody started from an app (background_turn.rs: a routine, a watcher's, a task, a resumed one): while no
    /// app follows it, what it asks is kept by the bridge, never waited for inside Hermes (held_approvals.rs).
    pub unattended: bool,
    /// The brake's count of the run's tool calls, and whether it stopped the run.
    pub turn: crate::loop_brake::TurnCount,
    pub braked: bool,
    /// The brake's line when it stopped the run (`stop_looping`).
    pub brake_said: Option<String>,
    /// The brake stopped this conversation turn and a rescue turn is to follow at its end (`loop_rescue`): why, and
    /// the agent's language.
    pub loop_rescue: Option<(crate::loop_brake::Over, String)>,
    /// This run is the rescue turn of that run (its limits: `rescue_limits`; never rescued itself).
    pub rescue_of: Option<String>,
    /// This run is the continuation of that run, which ended on a promise (`go_on`; never continued itself).
    pub go_on_of: Option<String>,
    /// This run's end waits for that run, its continuation (`go_on`) or the brake's rescue turn (`loop_rescue`), whose
    /// events follow in its stream: that one is the conversation's turn now (`session_run`: its stop, words steered).
    pub continued_by: Option<String>,
    /// The person tapped Stop on this run (threads/send.rs `stop`): even once it ended, no continuation (`go_on`).
    pub stop_asked: bool,
    /// A message of another household's agent (peers.rs): a guest's turn, never continued (`go_on`).
    pub guest: bool,
    /// Followed again after a bridge restart (`resume`): what started it (a person, a routine, a continuation…) is
    /// not known any more, so it is never continued (`go_on`).
    pub resumed: bool,
    /// A continuation the bridge stopped right after its start (the person stepped in): its end is no Stop of theirs
    /// for the queue (msg_queue.rs `run_ended` pauses behind a stopped turn).
    pub dropped: bool,
    /// The turn's own instructions (a shared conversation's, read_shares.rs) and options (its thinking), for its
    /// continuation (`go_on`).
    pub instructions: Option<String>,
    pub model_options: Option<Value>,
    /// Followed for the brake whoever looks (`keep`: a run started or followed through the door's Hermes relay, a
    /// peer's message): kept alive like a watched run, without its pushes.
    pub kept: bool,
    /// An app or a Live call has followed the run (a kept run nobody looked at is not « followed » for cronwatch's
    /// Bot Chat push: `followed_recently`).
    pub ever_followed: bool,
    /// The run's last tool call (bare name) and when: which run a call of the bridge's tools belongs to (`turn_of`).
    pub last_tool: String,
    pub last_tool_at: f64,
    /// The run's real tokens so far (`model.usage`): None from a Hermes without patch 0020.
    pub usage: Option<RunUsage>,
    /// A room's turn (rooms.rs), a background task's run (jobs.rs): the tokens it may take. Once its real ones and its
    /// next call would pass them, the hub stops it as that call's tool step starts (`capped`).
    pub token_limit: Option<i64>,
    /// A background task's run (jobs.rs): the euros it may take at its prices (task_cost.rs), checked the same way.
    pub cost_limit: Option<(crate::task_cost::Prices, f64)>,
    pub capped: bool,
    /// The reply's `MEDIA:` lines, out of what the apps get (media.rs).
    pub media: crate::media::MediaFilter,
    /// The turn did something (a call that changes things, a card asked): a message sent now goes in at its next
    /// step, never cutting it (turn_restart.rs).
    pub acted: bool,
    /// The person's words this run was stopped for, to start again with at its end (`restart`).
    pub restart: Option<crate::turn_restart::Plan>,
    /// The run that took over from this one (`restart`): this one is no longer the conversation's run.
    pub restarted_as: Option<String>,
    /// This run is the restart of that run (`restart`), whose stream shows it: a conversation turn like it (its cards
    /// seen through it, the long-work and brake nets, a task's conversation), never continued itself.
    pub restart_of: Option<String>,
    /// What the agent does now, for the apps that ask (turn_status.rs, `[status]`): made at the run's first event.
    pub status: Option<crate::turn_status::Tracker>,
    /// The person said « Continuer » at the brake's two thirds: this turn's limits raised once (`brake_action`).
    pub brake_raised: bool,
    /// The stop the brake's last warning announced (« Arrêter » takes it at once).
    pub loop_over: Option<crate::loop_brake::Over>,
    listeners: Vec<Slot>,
    cancel: Option<CancellationToken>,
    task: Option<JoinHandle<()>>,
}

impl Default for SubscriptionState {
    fn default() -> Self {
        SubscriptionState {
            watched: false,
            finished: false,
            finished_at: None,
            terminal: None,
            session_id: None,
            output: None,
            end_push: true,
            backlog: VecDeque::new(),
            steps: None,
            actions: Vec::new(),
            steps_lang: None,
            own_steps: false,
            pushed: HashSet::new(),
            followed_since: crate::py::time_time(),
            tool_calls: 0,
            tool_steps: 0,
            tools_open: 0,
            handed_off: false,
            steered: false,
            counted_calls: 0,
            recent_tools: VecDeque::new(),
            delegating: 0,
            subagents: 0,
            exempt_until: 0.0,
            handoff_failed: false,
            rescued: false,
            room_id: None,
            phone: None,
            used_browser: false,
            background: false,
            unattended: false,
            turn: crate::loop_brake::TurnCount::default(),
            braked: false,
            brake_said: None,
            loop_rescue: None,
            rescue_of: None,
            go_on_of: None,
            continued_by: None,
            stop_asked: false,
            guest: false,
            resumed: false,
            dropped: false,
            instructions: None,
            model_options: None,
            kept: false,
            ever_followed: false,
            last_tool: String::new(),
            last_tool_at: 0.0,
            usage: None,
            token_limit: None,
            cost_limit: None,
            capped: false,
            media: Default::default(),
            acted: false,
            restart: None,
            restarted_as: None,
            restart_of: None,
            status: None,
            brake_raised: false,
            loop_over: None,
            listeners: Vec::new(),
            cancel: None,
            task: None,
        }
    }
}

impl SubscriptionState {
    fn followers(&self) -> usize {
        self.listeners.iter().filter(|l| l.kind == "voice" || l.kind == "app").count()
    }

    fn send_all(&self, event: Option<&SSEEvent>) {
        for listener in &self.listeners {
            let _ = listener.tx.send(event.cloned());
        }
    }
}

impl RunSubscription {
    pub fn new(agent: AgentConfig, run_id: &str) -> Self {
        RunSubscription { agent, run_id: run_id.to_string(), state: Mutex::new(SubscriptionState::default()) }
    }

    pub fn key(&self) -> (String, String) {
        (self.agent.name.clone(), self.run_id.clone())
    }

    /// The turn did something already (turn_restart.rs): a call that changes things, a card asked.
    pub fn acted(&self) -> bool {
        lock(&self.state).acted
    }

    /// The run is to start again with the person's new words at its end (`RunHub::interrupt`).
    pub fn restarting(&self) -> bool {
        lock(&self.state).restart.is_some()
    }

    /// Listeners showing the run to the user (voice WebSocket, app SSE).
    pub fn followers(&self) -> usize {
        lock(&self.state).followers()
    }

    pub fn finished(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).finished
    }

    pub fn set_session_id(&self, session_id: Option<String>) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).session_id = session_id;
    }

    pub fn session_id(&self) -> Option<String> {
        lock(&self.state).session_id.clone()
    }

    /// The phone the turn was asked from (`RunHub::set_phone`).
    pub fn phone(&self) -> Option<String> {
        lock(&self.state).phone.clone()
    }

    pub fn watched(&self) -> bool {
        lock(&self.state).watched
    }

    /// How the run ended (`run.completed`…), once it did.
    pub fn terminal(&self) -> Option<String> {
        lock(&self.state).terminal.clone()
    }

    pub fn output(&self) -> Option<String> {
        lock(&self.state).output.clone()
    }
}

type Key = (String, String);

#[derive(Default)]
struct HubInner {
    subs: HashMap<Key, Arc<RunSubscription>>,
    /// (agent, run_id, dedupe) -> the approval Hermes waits for.
    pending: IndexMap<(String, String, String), Dict>,
}

pub struct RunHub {
    pub hermes: HermesClient,
    pub push: Option<Arc<PushService>>,
    /// Watched runs survive a restart (`resume`).
    pub store: Option<Store>,
    pub watch_max_seconds: u64,
    pub retention_seconds: f64,
    /// For the files a reply hands the person (`MEDIA:` lines), kept when the run ends (sent_files.rs).
    pub config: Option<SharedConfig>,
    /// The background tasks (jobs.rs), for the safety net's rescue: set by `jobs_of`.
    pub jobs: std::sync::OnceLock<std::sync::Weak<crate::jobs::Jobs>>,
    /// The agents' waiting messages (msg_queue.rs): told each run's end, they say whether more follows in its
    /// conversation (no « reply ready » then). Set by `queues_of`.
    pub queues: std::sync::OnceLock<std::sync::Weak<crate::msg_queue::Queues>>,
    /// The conversations (threads.rs): told each run's end (their version grows, the apps are told).
    pub threads: crate::threads::Link,
    /// What turns nobody follows ask, handed to the bridge (held_approvals.rs `start`); unset (tests): Hermes waits.
    pub unattended: std::sync::OnceLock<mpsc::UnboundedSender<crate::held_approvals::Unattended>>,
    inner: Mutex<HubInner>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    next_id: AtomicU64,
}

/// Why the bridge answered a Hermes approval itself.
#[derive(Debug, Clone, PartialEq)]
enum Auto {
    /// The person's level (« Seulement le critique »).
    Level,
    /// One of the person's « Toujours ».
    Always,
    /// What the person approved later on a held card (its id).
    Held(String),
}

enum Outcome {
    Done,
    Cancelled,
    Timeout,
    Crashed(anyhow::Error),
}

impl RunHub {
    pub fn new(
        hermes: HermesClient,
        push: Option<Arc<PushService>>,
        watch_max_seconds: u64,
        store: Option<Store>,
    ) -> Self {
        RunHub {
            hermes,
            push,
            store,
            watch_max_seconds,
            retention_seconds: FINISHED_RETENTION_SECONDS,
            config: None,
            jobs: std::sync::OnceLock::new(),
            queues: std::sync::OnceLock::new(),
            threads: Default::default(),
            unattended: std::sync::OnceLock::new(),
            inner: Mutex::new(HubInner::default()),
            tasks: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
    }

    pub fn with_config(mut self, config: SharedConfig) -> Self {
        self.config = Some(config);
        self
    }

    // -- public API ------------------------------------------------------------------------------

    /// A listener on the run (its backlog replayed first). `kind`: "voice", "app" or "watch".
    pub fn subscribe(self: &Arc<Self>, agent: &AgentConfig, run_id: &str, kind: &str, watch: bool) -> Listener {
        let mut inner = lock(&self.inner);
        let sub = self.get_or_start(&mut inner, agent, run_id);
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut st = lock(&sub.state);
            if watch {
                st.watched = true;
            }
            if kind == "voice" || kind == "app" {
                st.ever_followed = true;
            }
            for event in &st.backlog {
                let _ = tx.send(Some(event.clone()));
            }
            if st.finished {
                let _ = tx.send(None);
            }
            st.listeners.push(Slot { id, kind: kind.to_string(), tx });
        }
        Listener { sub, kind: kind.to_string(), queue: rx, closed: false, hub: Some(self.clone()), id }
    }

    /// Approvals and questions nobody sees are pushed; so is the end, unless `end_push` is false (never set back
    /// to true by a later watch: a restart's resume() watches again the runs a background task follows).
    pub fn watch(
        self: &Arc<Self>,
        agent: &AgentConfig,
        run_id: &str,
        session_id: Option<&str>,
        end_push: bool,
    ) -> Arc<RunSubscription> {
        let mut inner = lock(&self.inner);
        let sub = self.get_or_start(&mut inner, agent, run_id);
        let (remember, session) = {
            let mut st = lock(&sub.state);
            st.watched = true;
            if !end_push {
                st.end_push = false;
            }
            if let Some(s) = session_id.filter(|s| !s.is_empty()) {
                if st.session_id.as_deref().map(str::is_empty).unwrap_or(true) {
                    st.session_id = Some(s.to_string());
                }
            }
            (!st.finished, st.session_id.clone())
        };
        drop(inner);
        if let (Some(store), true) = (self.store.clone(), remember) {
            let (name, run) = (agent.name.clone(), run_id.to_string());
            self.spawn(async move {
                if let Err(exc) = store.remember_run(&name, &run, session.as_deref()).await {
                    tracing::error!(target: "asyncio", exc = %format!("{exc:#}"), "Task exception was never retrieved");
                }
            });
        }
        sub
    }

    /// The run is a turn of a group discussion (rooms.rs): its approvals carry `room_id`, in the push and in
    /// `/v1/approvals`, so tapping them opens the room. Also marks what it already waits for.
    pub fn set_room(&self, agent: &str, run_id: &str, room_id: &str) {
        let mut inner = lock(&self.inner);
        if let Some(sub) = inner.subs.get(&(agent.to_string(), run_id.to_string())) {
            lock(&sub.state).room_id = Some(room_id.to_string());
        }
        for ((a, r, _), item) in inner.pending.iter_mut() {
            if a == agent && r == run_id {
                item.insert("room_id".into(), json!(room_id));
            }
        }
    }

    /// A run the hub follows (a room's turn, rooms.rs; a background task, jobs.rs): its tool steps, its real tokens so
    /// far, whether the brake or its token limit stopped it. None once the hub let it go.
    pub fn run_progress(&self, agent: &str, run_id: &str) -> Option<RunProgress> {
        let inner = lock(&self.inner);
        inner.subs.get(&(agent.to_string(), run_id.to_string())).map(|sub| {
            let st = lock(&sub.state);
            RunProgress { tool_steps: st.tool_steps, braked: st.braked, usage: st.usage, capped: st.capped }
        })
    }

    /// The brake's line (in the agent's language) when it stopped the run, for a turn whose person gets no other word
    /// (background_turn.rs `Ran::stopped`, jobs.rs).
    pub fn brake_said(&self, agent: &str, run_id: &str) -> Option<String> {
        let inner = lock(&self.inner);
        let sub = inner.subs.get(&(agent.to_string(), run_id.to_string()))?;
        let st = lock(&sub.state);
        st.braked.then(|| st.brake_said.clone()).flatten()
    }

    /// A room's turn (rooms.rs) or a background task's run (jobs.rs) may take `limit` tokens: stopped by the hub before
    /// a model call that would pass them (`cap`), on its real tokens (Hermes patch 0020).
    pub fn limit_tokens(&self, agent: &str, run_id: &str, limit: i64) {
        let inner = lock(&self.inner);
        if let Some(sub) = inner.subs.get(&(agent.to_string(), run_id.to_string())) {
            lock(&sub.state).token_limit = Some(limit);
        }
    }

    /// A background task's run may take `euros` at `prices` (its calls' fresh input, cache reads and output, as the
    /// month's credit counts them): stopped by the hub before a model call that would pass them (`cap`).
    pub fn limit_cost(&self, agent: &str, run_id: &str, prices: crate::task_cost::Prices, euros: f64) {
        let inner = lock(&self.inner);
        if let Some(sub) = inner.subs.get(&(agent.to_string(), run_id.to_string())) {
            lock(&sub.state).cost_limit = Some((prices, euros));
        }
    }

    /// Follow the run for the brake (loop_brake.rs) whoever looks at it, without the pushes of `watch`: a run started
    /// or followed through the door's Hermes relay (lan.rs), a peer's message (peers.rs). Before 2026-10-27 evening
    /// those runs were invisible to the hub: never counted nor stopped, their writes counted per agent.
    pub fn keep(self: &Arc<Self>, agent: &AgentConfig, run_id: &str, session_id: Option<&str>) -> Arc<RunSubscription> {
        let mut inner = lock(&self.inner);
        let sub = self.get_or_start(&mut inner, agent, run_id);
        let mut st = lock(&sub.state);
        st.kept = true;
        if let Some(s) = session_id.filter(|s| !s.is_empty()) {
            if st.session_id.as_deref().map(str::is_empty).unwrap_or(true) {
                st.session_id = Some(s.to_string());
            }
        }
        drop(st);
        sub
    }

    /// After a restart: watch again the runs that were watched, and rebuild what Hermes still waits for (an
    /// approval or a question asked before the restart is not replayed by its event stream, but kept in the run's
    /// status: hermes_prompts.rs `pending_requests`).
    pub async fn resume(self: Arc<Self>, agents: IndexMap<String, AgentConfig>) -> usize {
        let Some(store) = self.store.clone() else { return 0 };
        let mut resumed = 0;
        let outcome: anyhow::Result<()> = async {
            for row in store.watched_runs(self.watch_max_seconds as f64).await? {
                let agent_name = crate::py::py_str(row.get("agent").unwrap_or(&Value::Null));
                let run_id = crate::py::py_str(row.get("run_id").unwrap_or(&Value::Null));
                let Some(agent) = agents.get(&agent_name) else {
                    store.forget_run(&agent_name, &run_id).await?;
                    continue;
                };
                let run = match self.hermes.run(agent, &run_id).await {
                    Ok(run) => run,
                    Err(HermesError::Http { status, .. }) => {
                        if status == 404 || status == 410 {
                            store.forget_run(&agent.name, &run_id).await?;
                        }
                        continue;
                    }
                    Err(_) => continue, // Hermes restarting: try again at the next bridge start
                };
                let status =
                    run.get("status").filter(|s| crate::py::truthy(s)).map(crate::py::py_str).unwrap_or_default();
                if ["completed", "failed", "cancelled", "canceled", "interrupted"].contains(&status.as_str()) {
                    store.forget_run(&agent.name, &run_id).await?;
                    continue;
                }
                let session =
                    as_str(row.get("session_id")).filter(|s| !s.is_empty()).or_else(|| as_str(run.get("session_id")));
                let sub = self.watch(agent, &run_id, session.as_deref(), true);
                // who started it is not known any more: never continued if it ends on a promise (go_on.rs)
                lock(&sub.state).resumed = true;
                for asked in crate::hermes_prompts::pending_requests(&run_id, &run) {
                    self.dispatch(&sub, asked);
                }
                resumed += 1;
                tracing::info!(agent = %agent.name, run_id = %run_id, status = %status, "run watch resumed");
            }
            Ok(())
        }
        .await;
        if let Err(exc) = outcome {
            tracing::error!(target: "asyncio", exc = %format!("{exc:#}"), "Task exception was never retrieved");
        }
        resumed
    }

    /// `reply` belongs to an app-followed run of this session (going on, or ended less than `within` seconds
    /// ago): it was shown live or already pushed as « reply ready ».
    ///
    /// A later turn the agent takes on its own (a background sub-task's result it now delivers) is *not* that
    /// reply, even within the window: it is recognised by text, the finished run's output, so it gets its push.
    pub fn followed_recently(&self, agent: &str, session_id: &str, reply: Option<&str>, within: f64) -> bool {
        let now = crate::py::time_time();
        let squash = crate::push::pyx::squash;
        let inner = lock(&self.inner);
        for sub in inner.subs.values() {
            let st = lock(&sub.state);
            if sub.agent.name != agent || st.session_id.as_deref() != Some(session_id) {
                continue;
            }
            if st.kept && !st.watched && !st.ever_followed {
                continue; // followed for the brake only (a peer's message): its reply still pushed
            }
            if !st.finished {
                return true;
            }
            let finished_at = st.finished_at.filter(|t| *t != 0.0).unwrap_or(now);
            if finished_at <= now - within {
                continue;
            }
            let (Some(reply), Some(output)) = (reply, st.output.as_deref()) else {
                if finished_at > now - 60.0 {
                    return true; // output unknown: only a reply right after the run
                }
                continue;
            };
            let reply = squash(reply);
            if !reply.is_empty() && squash(output).contains(&reply) {
                return true;
            }
        }
        false
    }

    /// The conversation whose run is asking for something now (a tool call such as start_task, which carries no
    /// session): the agent's runs going on in a person's conversation (not a background task's own), the one that
    /// just called a handing-over tool first, else the one followed last. None when none is known.
    pub fn asking_session(&self, agent: &str) -> Option<String> {
        let inner = lock(&self.inner);
        let mut best: Option<(bool, f64, String)> = None;
        for sub in inner.subs.values() {
            if sub.agent.name != agent {
                continue;
            }
            let st = lock(&sub.state);
            let Some(session) = st.session_id.clone().filter(|s| !s.is_empty()) else { continue };
            // a continuation or rescue turn (its end told by the turn it follows) is that conversation's too
            if st.finished
                || !(st.end_push || st.go_on_of.is_some() || st.rescue_of.is_some() || st.restart_of.is_some())
            {
                continue;
            }
            let handing = st.recent_tools.back().is_some_and(|t| handoff_tool(t));
            let key = (handing, st.followed_since);
            if best.as_ref().is_none_or(|(h, f, _)| (key.0 && !*h) || (key.0 == *h && key.1 > *f)) {
                best = Some((key.0, key.1, session));
            }
        }
        best.map(|(_, _, session)| session)
    }

    /// Runs the hub follows that haven't ended, but those in `skip` (background tasks' runs, paused apart): the
    /// person's turns a planned move waits for (place_move.rs).
    pub fn running(&self, skip: &[String]) -> usize {
        let inner = lock(&self.inner);
        inner.subs.iter().filter(|(key, sub)| !skip.contains(&key.1) && !lock(&sub.state).finished).count()
    }

    /// Runs the hub follows that work now: not ended, not waiting for their person's answer (a card, a question), but
    /// those in `skip` (background tasks' runs). What an update waits for before the bridge restarts
    /// (update_drain.rs): a run waiting for its person loses nothing then (Hermes keeps it, the hub resumes it).
    pub fn working(&self, skip: &[String]) -> usize {
        let inner = lock(&self.inner);
        inner
            .subs
            .iter()
            .filter(|(key, sub)| {
                !skip.contains(&key.1)
                    && !lock(&sub.state).finished
                    && !inner.pending.keys().any(|(agent, run, _)| *agent == key.0 && *run == key.1)
            })
            .count()
    }

    /// A run of the agent goes on now (a turn, a room's, a Live call's, a background task's): agent_restart.rs waits.
    pub fn agent_running(&self, agent: &str) -> bool {
        let inner = lock(&self.inner);
        inner.subs.values().any(|sub| {
            let st = lock(&sub.state);
            sub.agent.name == agent && !st.finished && st.terminal.is_none()
        })
    }

    /// The agent's runs going on at this Hermes address (its turns still in the shared hub while it moves,
    /// own_space.rs).
    pub fn agent_runs_at(&self, agent: &str, hermes_url: &str) -> usize {
        let inner = lock(&self.inner);
        inner
            .subs
            .values()
            .filter(|sub| sub.agent.name == agent && sub.agent.hermes_url == hermes_url && !lock(&sub.state).finished)
            .count()
    }

    /// A word of the bridge (`bridge.notice`, `data` with its `code` and `text`) in the agent's runs going on now:
    /// the apps show it in the conversation. Returns how many runs got it.
    pub fn notice_agent(&self, agent: &str, data: Map<String, Value>) -> usize {
        let subs: Vec<Arc<RunSubscription>> = lock(&self.inner)
            .subs
            .values()
            .filter(|sub| sub.agent.name == agent && !lock(&sub.state).finished)
            .cloned()
            .collect();
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        for sub in &subs {
            self.dispatch(sub, SSEEvent::new(NOTICE, data.clone(), raw.clone()));
        }
        subs.len()
    }

    /// A run of the agent goes on in this conversation (the hub follows it).
    pub fn session_busy(&self, agent: &str, session_id: &str) -> bool {
        let inner = lock(&self.inner);
        inner.subs.values().any(|sub| {
            let st = lock(&sub.state);
            sub.agent.name == agent && !st.finished && st.session_id.as_deref() == Some(session_id)
        })
    }

    /// The run going on in this conversation (not ended, its end not reached yet), if the hub follows one.
    pub fn session_run(&self, agent: &str, session_id: &str) -> Option<String> {
        self.session_run_since(agent, session_id).map(|(run, _)| run)
    }

    /// `session_run` with when the hub began following it (Unix seconds): the turn's start for a run the bridge started
    /// (an app's send), the apps' working-line clock (`pending.run.started`) whichever phone follows it.
    /// A turn whose end waits for its continuation or rescue turn (`continued_by`) is not it: that one is (the apps'
    /// Stop, words steered, the queue), since the turn's start.
    pub fn session_run_since(&self, agent: &str, session_id: &str) -> Option<(String, f64)> {
        let inner = lock(&self.inner);
        let (mut going, mut since) = (None, f64::INFINITY);
        for sub in inner.subs.values() {
            let st = lock(&sub.state);
            // a run stopped for new words and started again (`restarted_as`) is not the conversation's any more
            if sub.agent.name != agent
                || st.finished
                || st.terminal.is_some()
                || st.restarted_as.is_some()
                || st.dropped
                || st.session_id.as_deref() != Some(session_id)
            {
                continue;
            }
            since = since.min(st.followed_since);
            if st.continued_by.is_none() && going.is_none() {
                going = Some(sub.run_id.clone());
            }
        }
        going.map(|run| (run, since))
    }

    /// What a run followed now is doing, as a quick talk shows it (threads/follow.rs): the tool going on (None: it
    /// thinks or writes) and its words so far (the backlog's deltas).
    pub fn glance(&self, agent: &str, run_id: &str) -> Option<(Option<String>, String)> {
        let sub = self.subscription(agent, run_id)?;
        let st = lock(&sub.state);
        let tool = (st.tools_open > 0 && !st.last_tool.is_empty()).then(|| st.last_tool.clone());
        let answer = st
            .backlog
            .iter()
            .filter(|e| e.r#type == "message.delta")
            .filter_map(|e| e.data.get("delta").and_then(Value::as_str))
            .collect();
        Some((tool, answer))
    }

    /// An event for the apps following the agent's runs going on now, live only (not replayed to a later follower:
    /// the agent's message queue changed, msg_queue.rs).
    pub fn tell_agent_live(&self, agent: &str, kind: &str, data: Map<String, Value>) {
        let inner = lock(&self.inner);
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        let event = SSEEvent::new(kind, data, raw);
        for sub in inner.subs.values().filter(|sub| sub.agent.name == agent) {
            let st = lock(&sub.state);
            if !st.finished && st.terminal.is_none() {
                st.send_all(Some(&event));
            }
        }
    }

    pub fn subscription(&self, agent: &str, run_id: &str) -> Option<Arc<RunSubscription>> {
        lock(&self.inner).subs.get(&(agent.to_string(), run_id.to_string())).cloned()
    }

    /// The phone the person's latest words in this run came from (a message that started it, or words steered into
    /// it); nothing said (an older app), the run keeps what it had.
    pub fn set_phone(&self, agent: &str, run_id: &str, phone: Option<&str>) {
        if let (Some(phone), Some(sub)) = (phone, self.subscription(agent, run_id)) {
            lock(&sub.state).phone = Some(phone.to_string());
        }
    }

    /// The turn's instructions and options as it was started (threads/send.rs), for a continuation of it (`go_on`).
    pub fn set_turn(&self, agent: &str, run_id: &str, instructions: Option<&str>, options: Option<Value>) {
        if let Some(sub) = self.subscription(agent, run_id) {
            let mut st = lock(&sub.state);
            st.instructions = instructions.map(str::to_string);
            st.model_options = options;
        }
    }

    /// The person tapped Stop on this run (threads/send.rs `stop`), whether Hermes still had it or not.
    pub fn ask_stop(&self, agent: &str, run_id: &str) {
        if let Some(sub) = self.subscription(agent, run_id) {
            lock(&sub.state).stop_asked = true;
        }
    }

    /// The approvals Hermes waits for, each with `created_at` in Unix seconds.
    pub fn pending_approvals(&self, agent: Option<&str>) -> Vec<Dict> {
        self.pending_of(agent, false)
    }

    /// The questions Hermes waits for (a run's `clarify.request`, until settled or the run's end): every device of
    /// the person shows the same card, gone once answered on one (threads' `pending.questions`).
    pub fn pending_questions(&self, agent: &str) -> Vec<Dict> {
        self.pending_of(Some(agent), true)
    }

    /// The questions waiting, one agent's or every agent's (None): the Boîte's list (`GET /v1/approvals`).
    pub fn pending_questions_of(&self, agent: Option<&str>) -> Vec<Dict> {
        self.pending_of(agent, true)
    }

    fn pending_of(&self, agent: Option<&str>, questions: bool) -> Vec<Dict> {
        let inner = lock(&self.inner);
        let mut items: Vec<Dict> = inner
            .pending
            .values()
            .filter(|v| agent.is_none() || v.get("agent").and_then(Value::as_str) == agent)
            .filter(|v| (v.get("kind").and_then(Value::as_str) == Some("question")) == questions)
            .cloned()
            .collect();
        let created = |item: &Dict| item.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
        items.sort_by(|a, b| created(a).partial_cmp(&created(b)).unwrap_or(std::cmp::Ordering::Equal));
        items
    }

    pub fn forget_approval(&self, agent: &str, run_id: &str, request_id: Option<&str>) {
        let request_id = request_id.map(|r| json!(r));
        Self::forget_pending(&mut lock(&self.inner), agent, run_id, request_id.as_ref());
    }

    pub async fn close(&self) {
        let subs: Vec<Arc<RunSubscription>> = lock(&self.inner).subs.values().cloned().collect();
        let mut handles = Vec::new();
        for sub in subs {
            let mut st = lock(&sub.state);
            if let Some(cancel) = &st.cancel {
                cancel.cancel();
            }
            if let Some(handle) = st.task.take() {
                handles.push(handle);
            }
        }
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *lock(&self.tasks));
        for handle in handles.into_iter().chain(tasks) {
            let _ = handle.await;
        }
        lock(&self.inner).subs.clear();
    }

    // -- internals -------------------------------------------------------------------------------

    fn forget_pending(inner: &mut HubInner, agent: &str, run_id: &str, request_id: Option<&Value>) {
        let any = request_id.map(|r| !crate::py::truthy(r)).unwrap_or(true);
        inner.pending.retain(|key, item| {
            let matches = key.0 == agent
                && key.1 == run_id
                && (any || {
                    let rid = item.get("request_id").unwrap_or(&Value::Null);
                    rid.is_null() || Some(rid) == request_id
                });
            !matches
        });
    }

    fn get_or_start(self: &Arc<Self>, inner: &mut HubInner, agent: &AgentConfig, run_id: &str) -> Arc<RunSubscription> {
        let key = (agent.name.clone(), run_id.to_string());
        if let Some(sub) = inner.subs.get(&key) {
            return sub.clone();
        }
        let sub = Arc::new(RunSubscription::new(agent.clone(), run_id));
        inner.subs.insert(key, sub.clone());
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(self.clone().upstream(sub.clone(), cancel.clone()));
        let mut st = lock(&sub.state);
        st.cancel = Some(cancel);
        st.task = Some(handle);
        drop(st);
        sub
    }

    fn remove_listener(&self, id: u64, sub: &Arc<RunSubscription>) {
        let mut inner = lock(&self.inner);
        let mut st = lock(&sub.state);
        let was_followed = st.followers() > 0;
        st.listeners.retain(|l| l.id != id);
        let unfollowed = was_followed && st.followers() == 0;
        if unfollowed && st.watched && !st.finished {
            self.push_unseen(&inner, sub, &mut st);
        }
        // the cards of its continuation or rescue turn were seen through it: unseen now too
        let next = st.continued_by.clone().or(st.restarted_as.clone()).filter(|_| unfollowed && !st.finished);
        if st.listeners.is_empty() && !st.watched && !st.kept && !st.finished {
            if let Some(cancel) = &st.cancel {
                cancel.cancel();
            }
            let key = sub.key();
            if inner.subs.get(&key).map(|s| Arc::ptr_eq(s, sub)).unwrap_or(false) {
                inner.subs.remove(&key);
            }
        }
        drop(st);
        // down the chain of runs that took over (a continuation restarted by new words, a restart's rescue turn…)
        let mut next = next;
        for _ in 0..CHAIN_MAX {
            let Some(run) = next.and_then(|n| inner.subs.get(&(sub.agent.name.clone(), n)).cloned()) else { break };
            if Arc::ptr_eq(&run, sub) {
                break;
            }
            let mut st = lock(&run.state);
            if st.watched && st.followers() == 0 && !st.finished {
                self.push_unseen(&inner, &run, &mut st);
            }
            next = st.continued_by.clone().or(st.restarted_as.clone());
        }
    }

    /// A continuation, rescue turn or restart shows in the stream of the turn it follows (`go_on`, `loop_rescue`,
    /// `restart`), and so on up the chain (a continuation restarted by new words: its restart shows through the
    /// continuation, which shows through the turn): whoever follows one of them sees its cards, so they are not pushed.
    /// `inner` held, then this run's state, then each one up the chain in turn.
    fn followed_through(inner: &HubInner, sub: &RunSubscription, st: &SubscriptionState) -> bool {
        let of = |st: &SubscriptionState| st.go_on_of.clone().or(st.rescue_of.clone()).or(st.restart_of.clone());
        let mut up = of(st);
        for _ in 0..CHAIN_MAX {
            let Some(run) = up.filter(|r| *r != sub.run_id) else { return false };
            let Some(origin) = inner.subs.get(&(sub.agent.name.clone(), run)) else { return false };
            let origin = lock(&origin.state);
            if origin.followers() > 0 {
                return true;
            }
            up = of(&origin);
        }
        false
    }

    /// The app stopped showing the run (left the conversation, screen locked) while an approval or a
    /// question was waiting: it was on screen, so never pushed. Push it now, or the run waits unseen until
    /// Hermes' timeout.
    /// A relative's turn (a member's in a family agent's common conversation, family_thread.rs): its cards are hers,
    /// answered from her bridge, never by this person's level nor pushed to this person's devices.
    fn someone_elses(&self, run_id: &str) -> bool {
        self.config.as_ref().is_some_and(|c| crate::agent_shares::relative_run(&c.get(), run_id))
    }

    fn push_unseen(&self, inner: &HubInner, sub: &RunSubscription, st: &mut SubscriptionState) {
        let Some(push) = self.push.clone() else { return };
        if self.someone_elses(&sub.run_id) {
            return;
        }
        for ((agent, run_id, dedupe), item) in &inner.pending {
            if *agent == sub.agent.name && *run_id == sub.run_id && !st.pushed.contains(dedupe) {
                st.pushed.insert(dedupe.clone());
                tracing::info!(agent = %agent, run_id = %run_id, "approval push (left unanswered)");
                let (push, agent, run_id, session, room) =
                    (push.clone(), agent.clone(), run_id.clone(), st.session_id.clone(), st.room_id.clone());
                let request_id = as_str(item.get("request_id"));
                let command = as_str(item.get("bip_summary")).or_else(|| as_str(item.get("command")));
                let choices = match item.get("choices") {
                    Some(Value::Array(c)) => Some(c.clone()),
                    _ => None,
                };
                self.spawn(async move {
                    push.notify_approval_values(
                        &agent,
                        &run_id,
                        request_id.as_deref(),
                        choices.as_deref(),
                        command.as_deref(),
                        session.as_deref(),
                        room.as_deref(),
                    )
                    .await;
                });
            }
        }
        let answered: HashSet<Option<String>> = st
            .backlog
            .iter()
            .filter(|e| e.r#type == crate::hermes_prompts::CLARIFY_SETTLED)
            .map(|e| clarify_id(&e.data))
            .collect();
        let questions: Vec<SSEEvent> = st
            .backlog
            .iter()
            .filter(|e| e.r#type == "clarify.request" && !answered.contains(&clarify_id(&e.data)))
            .cloned()
            .collect();
        for event in questions {
            self.on_clarify(inner, sub, st, &event);
        }
    }

    /// A background task, kept until it ends (Python's `_spawn`); none outside a runtime (a listener dropped late).
    fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let handle = runtime.spawn(future);
        let mut tasks = lock(&self.tasks);
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
    }

    async fn upstream(self: Arc<Self>, sub: Arc<RunSubscription>, cancel: CancellationToken) {
        let limit = Duration::from_secs(self.watch_max_seconds);
        // what the agent does now: a wait grown abnormal is said without an event of the run's (turn_status.rs). Only
        // with `[status] enabled`: off, nothing runs (review B4, 17/11)
        let on = self.config.as_ref().is_some_and(|c| c.get().status.enabled);
        let ticker = on.then(|| {
            let (hub, sub) = (self.clone(), sub.clone());
            tokio::spawn(async move {
                let every = Duration::from_secs_f64(crate::turn_status::TICK_SECONDS);
                loop {
                    tokio::time::sleep(every).await;
                    if !hub.status_tick(&sub) {
                        break;
                    }
                }
            })
        });
        let outcome = tokio::select! {
            _ = cancel.cancelled() => Outcome::Cancelled,
            result = tokio::time::timeout(limit, self.upstream_loop(&sub)) => match result {
                Err(_) => Outcome::Timeout,
                Ok(Ok(())) => Outcome::Done,
                Ok(Err(exc)) => Outcome::Crashed(exc),
            },
        };
        match outcome {
            Outcome::Timeout => {
                let data = json!({"type": "bridge.error", "code": "watch_timeout"});
                self.dispatch(&sub, SSEEvent::new("bridge.error", data.as_object().cloned().unwrap_or_default(), ""));
            }
            Outcome::Crashed(exc) => {
                tracing::error!(exc = %format!("{exc:#}"), agent = %sub.agent.name, run_id = %sub.run_id, "run subscription crashed");
            }
            Outcome::Done | Outcome::Cancelled => {}
        }
        if let Some(ticker) = ticker {
            ticker.abort();
        }
        {
            let mut st = lock(&sub.state);
            st.finished = true;
            st.finished_at = Some(crate::py::time_time());
            st.send_all(None);
        }
        let retention = Duration::from_secs_f64(self.retention_seconds.max(0.0));
        let hub = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(retention).await;
            let mut inner = lock(&hub.inner);
            let key = sub.key();
            if inner.subs.get(&key).map(|s| Arc::ptr_eq(s, &sub)).unwrap_or(false) {
                inner.subs.remove(&key);
            }
        });
    }

    async fn upstream_loop(self: &Arc<Self>, sub: &Arc<RunSubscription>) -> anyhow::Result<()> {
        let mut backoff = BACKOFF_START;
        let mut failures: u32 = 0;
        loop {
            let mut error: Option<HermesError> = None;
            match self.hermes.run_events(&sub.agent, &sub.run_id).await {
                Err(exc) => error = Some(exc),
                Ok(mut events) => {
                    while let Some(item) = events.next().await {
                        match item {
                            Err(exc) => {
                                error = Some(exc);
                                break;
                            }
                            Ok(event) => {
                                backoff = BACKOFF_START;
                                failures = 0;
                                let terminal = is_terminal(&event.r#type);
                                let event = if terminal {
                                    // its statuses end here (a long turn's with its summary), whatever turn takes
                                    // over (a restart, a rescue…)
                                    self.status_finish(sub, &event);
                                    self.flush_media(sub);
                                    self.rescue(sub, &event).await;
                                    // a turn stopped for the person's new words: the end of the one restarted
                                    let event = self.restart(sub, event).await;
                                    // a turn the brake stopped: its rescue turn's end instead (loop_rescue)
                                    let event = self.loop_rescue(sub, event).await;
                                    // a turn that ended on a promise: its continuation's end instead (go_on.rs),
                                    // already handed over by the continuation (this turn made nothing)
                                    match self.go_on(sub, event).await {
                                        (end, Some(own)) => self.with_own_files(sub, own, end).await,
                                        (event, None) => {
                                            // the person's words that came while go_on decided start the turn
                                            // again (turn_restart.rs: they win over the promise's continuation)
                                            let event = self.restart(sub, event).await;
                                            // what the turn made and the files its reply cites, as attachments
                                            self.hand_over(sub, event).await
                                        }
                                    }
                                } else {
                                    event
                                };
                                self.dispatch(sub, event);
                                if terminal {
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
            match error {
                None => {}
                Some(HermesError::Http { status, .. }) => {
                    tracing::warn!(agent = %sub.agent.name, run_id = %sub.run_id, status = status, "run events refused");
                    if [400, 401, 403, 404, 410].contains(&status) {
                        let code = if status == 404 || status == 410 { "run_not_found" } else { "hermes_refused" };
                        let data = json!({"type": "bridge.error", "code": code, "status": status});
                        self.dispatch(
                            sub,
                            SSEEvent::new("bridge.error", data.as_object().cloned().unwrap_or_default(), ""),
                        );
                        return Ok(());
                    }
                }
                Some(exc) if exc.is_transport() => {
                    failures += 1;
                    tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, error = exc.type_name(), failures = failures, "run events stream lost");
                    if failures == UNREACHABLE_AFTER {
                        // Tell live listeners (not stored in the backlog: the watch may still recover).
                        let data = json!({"type": "bridge.error", "code": "hermes_unreachable"});
                        let notice = SSEEvent::new("bridge.error", data.as_object().cloned().unwrap_or_default(), "");
                        let st = lock(&sub.state);
                        st.send_all(Some(&notice));
                        if !st.watched && !st.kept {
                            return Ok(());
                        }
                    }
                    if failures >= GIVE_UP_AFTER {
                        return Ok(());
                    }
                }
                Some(exc) => return Err(exc.into()),
            }
            {
                let st = lock(&sub.state);
                if st.listeners.is_empty() && !st.watched && !st.kept {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs_f64(backoff)).await;
            backoff = (backoff * 2.0).min(BACKOFF_MAX);
        }
    }

    /// The open screens of the run's conversation: its list changed (`thread`, `reason: steps`, the version kept).
    fn steps_hint(&self, sub: &RunSubscription, session: Option<String>, steps: Value) {
        let (Some(threads), Some(session)) = (crate::threads::upgrade(&self.threads), session) else { return };
        let (agent, run_id) = (sub.agent.clone(), sub.run_id.clone());
        self.spawn(async move { threads.steps_changed(&agent, &session, &run_id, steps).await });
    }

    /// A connector's call with its connector's name (`service`: « Garmin », connectors::service_of): the apps and the
    /// automatic « Étapes » say it instead of the tool's own name.
    fn with_service(&self, sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
        let tool = crate::loop_brake::event_tool(&event.data);
        if crate::connectors::slug_of_tool(&tool).is_none() || event.data.contains_key("service") {
            return event;
        }
        let Some(shared) = self.config.as_ref() else { return event };
        let Some(service) = crate::connectors::service_of(&shared.get(), &sub.agent.name, &tool) else { return event };
        let mut data = event.data;
        data.insert("service".into(), json!(service));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// What the agent does now after this event (turn_status.rs), as a `turn.status` of the run's stream: only with
    /// `[status] enabled`, and only for the apps that ask (`status=1`, app.rs `listener_events_with`).
    fn status_of(&self, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) -> Option<SSEEvent> {
        if event.r#type == crate::turn_status::EVENT {
            return None;
        }
        let config = self.config.as_ref()?.get();
        if !config.status.enabled {
            return None;
        }
        let now = crate::py::time_time();
        if st.status.is_none() {
            let lang = crate::handback::language_of(&config, &sub.agent);
            let local = crate::turn_status::local_model(&config, &sub.agent);
            st.status = Some(crate::turn_status::Tracker::new(now, &lang, local));
        }
        let status = st.status.as_mut()?.on_event(&event.r#type, &event.data, now, &config.status)?;
        self.keep_took(sub, st, &status, now);
        Some(self.status_event(sub, status))
    }

    /// A long turn's summary kept for its last reply (threads `with_took`): numbers only.
    fn keep_took(&self, sub: &RunSubscription, st: &SubscriptionState, status: &Map<String, Value>, now: f64) {
        let (Some(took), Some(store), Some(session)) = (status.get("took").cloned(), self.store.clone(), st.session_id.clone())
        else {
            return;
        };
        let (agent, run_id) = (sub.agent.name.clone(), sub.run_id.clone());
        let started = status.get("started").and_then(Value::as_f64).unwrap_or(now);
        self.spawn(async move {
            if let Err(err) = store.save_turn_time(&agent, &run_id, &session, started, now, &took).await {
                tracing::info!(agent = %agent, run_id = %run_id, error = %format!("{err:#}"), "turn time not kept");
            }
        });
    }

    /// The run's own end from Hermes (before a restart's or a rescue's take over): its last status, a long turn's
    /// summary (`done`, `took`), then nothing more for it.
    fn status_finish(&self, sub: &RunSubscription, terminal: &SSEEvent) {
        let Some(config) = self.config.as_ref().map(|c| c.get()) else { return };
        let mut st = lock(&sub.state);
        let now = crate::py::time_time();
        let status = st.status.as_mut().and_then(|t| {
            let status = t.on_event(&terminal.r#type, &terminal.data, now, &config.status);
            t.end();
            status
        });
        if let Some(status) = status {
            self.keep_took(sub, &st, &status, now);
            let event = self.status_event(sub, status);
            append_to_backlog(&mut st.backlog, &event);
            st.send_all(Some(&event));
        }
    }

    fn status_event(&self, sub: &RunSubscription, mut status: Map<String, Value>) -> SSEEvent {
        status.insert("run_id".into(), json!(sub.run_id));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(status.clone()));
        SSEEvent::new(crate::turn_status::EVENT, status, raw)
    }

    /// The hub's clock for one run (`upstream`'s ticker): a wait grown abnormal said now. false once the run is over.
    pub fn status_tick(&self, sub: &RunSubscription) -> bool {
        let Some(config) = self.config.as_ref().map(|c| c.get()) else { return false };
        if !config.status.enabled {
            return false; // switched off meanwhile: the clock stops
        }
        let mut st = lock(&sub.state);
        if st.finished {
            return false;
        }
        let now = crate::py::time_time();
        let Some(status) = st.status.as_mut().and_then(|t| t.tick(now, &config.status)) else { return true };
        let event = self.status_event(sub, status);
        append_to_backlog(&mut st.backlog, &event);
        st.send_all(Some(&event));
        true
    }

    /// A call started or ended in a run without a list of the agent's own: the automatic list (steps.rs), as a
    /// `steps.updated` when it changed (only from its 2nd step).
    fn auto_steps(&self, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) -> Option<SSEEvent> {
        let tool = crate::loop_brake::event_tool(&event.data);
        let call_id = event.data.get("tool_call_id").and_then(Value::as_str).map(str::to_string);
        if event.r#type == "tool.started" {
            let preview = event.data.get("preview").and_then(Value::as_str).unwrap_or("");
            let site = crate::steps::site_of(&tool, preview);
            let service = event.data.get("service").and_then(Value::as_str).map(str::to_string);
            st.actions.push(crate::steps::Action { tool, site, service, running: true, call_id });
        } else {
            let open = st.actions.iter().position(|a| a.running && call_id.is_some() && a.call_id == call_id);
            let open = open.or_else(|| st.actions.iter().position(|a| a.running && a.tool == tool));
            st.actions[open?].running = false;
        }
        if st.steps_lang.is_none() {
            let lang = self.config.as_ref().map(|c| crate::handback::language_of(&c.get(), &sub.agent));
            st.steps_lang = Some(lang.unwrap_or_else(|| "fr".into()));
        }
        let steps = crate::steps::auto_steps(&st.actions, st.steps_lang.as_deref().unwrap_or("fr"))?;
        if st.steps.as_ref() == Some(&steps) {
            return None;
        }
        st.steps = Some(steps.clone());
        self.steps_hint(sub, st.session_id.clone(), steps.clone());
        let mut data = Map::new();
        data.insert("event".into(), json!(crate::steps::EVENT));
        data.insert("run_id".into(), json!(sub.run_id));
        data.insert("steps".into(), steps);
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        Some(SSEEvent::new(crate::steps::EVENT, data, raw))
    }

    fn dispatch(&self, sub: &Arc<RunSubscription>, event: SSEEvent) {
        let mut inner = lock(&self.inner);
        let mut st = lock(&sub.state);
        let event = if event.r#type == "approval.request" {
            self.with_always(sub, self.in_plain_words(sub, &st, event))
        } else {
            event
        };
        let event = if matches!(event.r#type.as_str(), "approval.request" | "clarify.request") {
            with_item_id(sub, event)
        } else {
            event
        };
        let event = if matches!(event.r#type.as_str(), "tool.started" | "tool.completed" | "tool.failed") {
            self.with_service(sub, event)
        } else {
            event
        };
        let event = if event.r#type == "run.completed" { self.no_reply_in_plain_words(sub, event) } else { event };
        let event = if event.r#type == "run.failed" { self.reply_cut_in_plain_words(sub, &st, event) } else { event };
        let event = if event.r#type == "run.failed" { self.account_failure_in_plain_words(sub, event) } else { event };
        let event =
            if matches!(event.r#type.as_str(), "run.completed" | "run.failed") { with_stopped(event) } else { event };
        let event = if event.r#type == crate::steps::HERMES_EVENT { as_steps(sub, event) } else { event };
        if event.r#type == crate::steps::EVENT {
            st.own_steps = true;
            st.steps = event.data.get("steps").cloned();
            self.steps_hint(sub, st.session_id.clone(), st.steps.clone().unwrap_or_default());
        }
        let auto = if matches!(event.r#type.as_str(), "tool.started" | "tool.completed") && !st.own_steps {
            self.auto_steps(sub, &mut st, &event)
        } else {
            None
        };
        // the model's words as Hermes keeps them (to recognise its reply), the apps' without MEDIA lines
        let written = as_str(event.data.get("output"));
        let delta = is_delta(&event.r#type);
        let mut shown = st.media.event(event);
        shown.extend(auto);
        // each step's time, the turn's (turn_status.rs), for the apps that read them
        if let Some(tracker) = st.status.as_mut() {
            let now = crate::py::time_time();
            for e in shown.iter_mut().filter(|e| e.r#type == crate::steps::EVENT) {
                let Some(steps) = e.data.get("steps").cloned() else { continue };
                e.data.insert("steps".into(), tracker.steps_with_times(&steps, now));
                e.raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(e.data.clone()));
            }
        }
        let statuses: Vec<SSEEvent> = shown.iter().filter_map(|e| self.status_of(sub, &mut st, e)).collect();
        // a long turn's summary before its end (an app stops reading there), any other status after its event
        if shown.iter().any(|e| is_terminal(&e.r#type)) {
            for event in &statuses {
                append_to_backlog(&mut st.backlog, event);
                st.send_all(Some(event));
            }
        } else {
            shown.extend(statuses);
        }
        for event in &shown {
            append_to_backlog(&mut st.backlog, event);
            st.send_all(Some(event));
            if let (Some(push), false) = (&self.push, event.r#type == crate::turn_status::EVENT) {
                // the Live Activities following this run (its own, its task's, its discussion's): live_activities.rs
                let followed = st.followers() > 0;
                push.live_event(&sub.agent.name, &sub.run_id, st.room_id.as_deref(), &event.r#type, &event.data, followed);
            }
        }
        if delta || shown.is_empty() {
            return; // its words and files passed on: nothing else to do with a delta
        }
        let event = shown.remove(0);
        let kind = event.r#type.as_str();
        if kind == "tool.started" || kind == "tool.completed" || kind == SUBAGENT_START || kind == SUBAGENT_DONE {
            self.handoff(sub, &mut st, &event);
        }
        if kind == MODEL_USAGE {
            // the latest totals (a stream replayed after a reconnection repeats them)
            let usage = RunUsage::of(&event.data);
            if let Some(usage) = usage.filter(|u| st.usage.is_none_or(|seen| u.total() >= seen.total())) {
                st.usage = Some(usage);
            }
        }
        if kind == "tool.started" {
            let tool = crate::loop_brake::event_tool(&event.data);
            st.last_tool = crate::loop_brake::bare(&tool).to_string();
            st.last_tool_at = crate::py::time_time();
            self.cap(sub, &mut st);
            self.brake(sub, &mut st, &event);
            // a use of a feature to discover (feature_tips.rs: a podcast made, the Carnet filled…)
            if let (Some(shared), true) = (self.config.clone(), crate::feature_tips::feature_for_tool(&tool).is_some()) {
                self.spawn(async move {
                    let config = shared.get();
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::feature_tips::tool_used(&config, &tool, crate::py::time_time())
                    })
                    .await;
                });
            }
        } else if kind == "tool.completed" {
            // a call that failed: the identical call right after is a retry, not « the same call again »
            let tool = crate::loop_brake::event_tool(&event.data);
            st.turn.completed(&tool, crate::loop_brake::completed_with_error(&event.data));
        }
        if kind == "tool.started" {
            let tool = event.data.get("tool").or_else(|| event.data.get("name")).and_then(Value::as_str).unwrap_or("");
            if browser_tool(tool) {
                st.used_browser = true;
            }
            if !crate::turn_restart::read_only(&crate::loop_brake::event_tool(&event.data)) {
                st.acted = true;
            }
        } else if matches!(kind, "approval.request" | "clarify.request" | SUBAGENT_START) {
            st.acted = true;
        }
        if kind == "approval.request" {
            self.on_approval(&mut inner, sub, &mut st, &event);
        } else if kind == "clarify.request" {
            Self::keep_question(&mut inner, sub, &st, &event);
            if !self.hold_question(sub, &st, &event) {
                self.on_clarify(&inner, sub, &mut st, &event);
            }
        } else if kind == crate::hermes_prompts::APPROVAL_SETTLED || kind == crate::hermes_prompts::CLARIFY_SETTLED {
            Self::forget_pending(&mut inner, &sub.agent.name, &sub.run_id, event.data.get("request_id"));
        } else if is_terminal(kind) {
            st.terminal = Some(kind.to_string());
            if st.used_browser && !Self::browser_still_used(&inner, sub) {
                if let Some(shared) = self.config.as_ref() {
                    crate::browsers::run_ended(&shared.get(), &sub.agent.name);
                }
            }
            let output = as_str(event.data.get("output"));
            if written.is_some() {
                st.output = written.clone();
            }
            Self::forget_pending(&mut inner, &sub.agent.name, &sub.run_id, None);
            // the files it hands the person kept (sent_files.rs): its lines and its attachments
            let handed: String = event.data.get("attachments").and_then(Value::as_array).into_iter().flatten()
                .filter_map(|a| a["path"].as_str()).map(|p| format!("\nMEDIA:{p}")).collect();
            let shown = output.clone().map(|o| o + &handed);
            if let (Some(shared), Some(text)) =
                (self.config.clone(), shown.filter(|o| kind == "run.completed" && o.contains("MEDIA:")))
            {
                let agent = sub.agent.clone();
                self.spawn(async move {
                    let config = shared.get();
                    let _ = tokio::task::spawn_blocking(move || {
                        let agent = config.agent(Some(&agent.name)).cloned().unwrap_or(agent);
                        crate::sent_files::capture_reply(&config, &agent, &text);
                    })
                    .await;
                });
            }
            if let Some(store) = self.store.clone() {
                let (agent, run_id) = sub.key();
                self.spawn(async move {
                    if let Err(exc) = store.forget_run(&agent, &run_id).await {
                        tracing::error!(target: "asyncio", exc = %format!("{exc:#}"), "Task exception was never retrieved");
                    }
                });
            }
            if let Some(threads) = crate::threads::upgrade(&self.threads) {
                let (agent, session) = (sub.agent.name.clone(), st.session_id.clone());
                self.spawn(async move { threads.touch(&agent, session.as_deref(), "run").await });
            }
            // not idle yet for a restart (agent_restart.rs): its person reads the answer and may write again at once
            crate::agent_restart::note_turn_end(&sub.agent.name);
            // the agent's message queue: what waits starts now, and while more follows here no « reply ready »
            let follows = self.queues.get().and_then(std::sync::Weak::upgrade).is_some_and(|queues| {
                let pending = as_str(event.data.get("pending_steer"));
                // a continuation the bridge stopped (the person wrote while it started): not their Stop, nothing paused
                let kind = if st.dropped { "run.completed" } else { kind };
                queues.run_ended(&sub.agent.name, &sub.run_id, st.session_id.as_deref(), kind, pending.as_deref())
            });
            if let (true, Some(push)) = (st.watched && st.end_push && st.followers() == 0 && !follows, self.push.clone()) {
                let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
                match written.filter(|o| kind == "run.completed" && !o.is_empty()) {
                    Some(output) => {
                        tracing::info!(agent = %agent, run_id = %run_id, "reply push");
                        self.spawn(async move {
                            push.notify_reply(&agent, &run_id, &output, session.as_deref()).await;
                        });
                    }
                    None => {
                        let mut extra = Map::new();
                        extra.insert("run_id".into(), json!(run_id));
                        extra.insert("status".into(), json!(kind));
                        self.spawn(async move {
                            push.notify_silent(&agent, "run_finished", extra).await;
                        });
                    }
                }
            }
        }
    }

    /// A run goes on now (a conversation's turn, a background task's, a Live call's: every run the hub follows).
    pub fn any_running(&self) -> bool {
        let inner = lock(&self.inner);
        inner.subs.values().any(|sub| {
            let st = lock(&sub.state);
            !st.finished && st.terminal.is_none()
        })
    }

    /// `~/.local/state/bipagents/busy` (`BUSY_MARKER`) kept fresh, every `every` seconds, while a run goes on; gone when
    /// none does. The guichet's seat agent (installer/cloud/seat-agent.py, root) reads it for the beat's `busy`: a
    /// marker older than a few minutes is a bridge that stopped, not a busy place.
    pub async fn mark_busy_forever(self: Arc<Self>, home: PathBuf, every: f64) {
        let marker = home.join(BUSY_MARKER);
        loop {
            if self.any_running() {
                if let Some(parent) = marker.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = std::fs::write(&marker, format!("{}", crate::py::time_time().trunc() as i64));
            } else {
                let _ = std::fs::remove_file(&marker);
            }
            tokio::time::sleep(Duration::from_secs_f64(every)).await;
        }
    }

    /// Another run of the agent that called its browser's tools still goes on (a background task's, another
    /// conversation's): its browser stays for it.
    fn browser_still_used(inner: &HubInner, sub: &Arc<RunSubscription>) -> bool {
        inner.subs.values().any(|other| {
            !Arc::ptr_eq(other, sub) && other.agent.name == sub.agent.name && {
                let st = lock(&other.state);
                st.used_browser && !st.finished && st.terminal.is_none()
            }
        })
    }

    /// The brake (loop_brake.rs, 2026-10-27): a run calling one tool again and again, the same call again, or far too
    /// many tools is stopped, and the conversation told in plain words (a `bridge.notice` with code `loop_stopped`, in
    /// the stream before the run's end; a push when nobody follows it). Not while a sub-agent works for the run (its
    /// calls are its own); a background task's run (`mark_background`) and a browsing one get the higher limits.
    /// A room's turn or a background task's run whose last model call asked for tools (`tool.started`): stopped now,
    /// before its next call, once its real tokens and that call (reading at least what the last one did) would pass its
    /// limit. A call that answered asks for none: the turn ends on its own. In the bench of 2026-11-01 a DeepSeek turn reached 249k for
    /// a 200k cap, its calls reading 41k each after big file reads, on an estimate of 20k a call.
    fn cap(&self, sub: &RunSubscription, st: &mut SubscriptionState) {
        let Some(usage) = st.usage else { return };
        let past_tokens = st.token_limit.is_some_and(|limit| usage.after_next_call() >= limit);
        let past_cost = st.cost_limit.is_some_and(|(p, euros)| p.of_usage(&usage) + p.next_call(&usage) >= euros);
        if st.capped || st.finished || !(past_tokens || past_cost) {
            return;
        }
        st.capped = true;
        tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, used = usage.total(), next_call = usage.last_input,
            limit = ?st.token_limit, euros = ?st.cost_limit.map(|(_, e)| e),
            "a run's next call would pass its limit, stopped");
        let (hermes, agent, run_id) = (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone());
        self.spawn(async move {
            if let Err(err) = hermes.stop_run(&agent, &run_id).await {
                tracing::info!(agent = %agent.name, run_id = %run_id, error = %err, "the token limit's stop refused");
            }
        });
    }

    fn brake(&self, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        if st.braked || st.finished || st.delegating > 0 || st.subagents > 0 {
            return;
        }
        let config = self.config.as_ref().map(|shared| shared.get());
        let settings = config.as_ref().map(|c| c.brake.clone()).unwrap_or_default();
        if !settings.enabled {
            return;
        }
        let tool = crate::loop_brake::event_tool(&event.data);
        if tool.is_empty() {
            return;
        }
        let browser = browser_tool(&tool);
        let rescuing = st.rescue_of.is_some();
        let mut limits = if rescuing { settings.rescue_limits() } else { settings.limits(st.background) };
        if (browser || st.used_browser) && !st.background && !rescuing {
            // a browsing turn: the browser's own calls on top (its own limits, browser_calls and page_reads, well
            // under Hermes' cap of 150, so a page it can't read ends on the plain line, not a silent failure)
            limits.tool_calls += limits.browser_calls;
        }
        if st.brake_raised {
            limits = limits.raised();
        }
        let signature = crate::loop_brake::event_signature(&event.data);
        let noted = st.turn.note(&tool, signature.as_deref(), browser, &limits);
        // a limit getting close (about two thirds): the agent is told once, in a steer (after its current tool calls,
        // whichever tools they are: Hermes' own as the bridge's), before any stop. Not in a rescue turn (tight).
        if let (Some(warning), true) = (st.turn.take_warning(), settings.warn && !rescuing && noted.is_none()) {
            tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, tool_calls = st.turn.total,
                reason = %warning.reason(), "brake_warned: the agent told it is going round in circles");
            let (hermes, agent, run_id, text) =
                (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone(), warning.text());
            self.spawn(async move {
                if let Err(err) = hermes.steer_run(&agent, &run_id, &text).await {
                    tracing::info!(agent = %agent.name, run_id = %run_id, error = %err, "brake_warned: steer refused");
                }
            });
            // and the person (turn_status.rs, « Je tourne en rond »): go on, stop, or help
            st.loop_over = Some(crate::loop_brake::over_of(&warning));
            let status_settings = config.as_ref().map(|c| c.status.clone()).unwrap_or_default();
            let found = crate::turn_status::Loop::of(&warning);
            let now = crate::py::time_time();
            if let Some(status) = st.status.as_mut().and_then(|t| t.looping(found, now, &status_settings)) {
                let event = self.status_event(sub, status);
                append_to_backlog(&mut st.backlog, &event);
                st.send_all(Some(&event));
            }
        }
        let Some(over) = noted else { return };
        self.stop_looping(sub, st, over, rescuing, &settings);
    }

    /// The bridge's tool called again with the very same arguments in a run it follows, after its « already done »
    /// (loop_brake.rs `Turns::check`): `calls` such calls in all. Hermes' run events carry no arguments for the
    /// bridge's tools (an empty preview), so the brake's `same_call` can't see them: it hears it from the tool here,
    /// and stops the run at that limit like any loop (release bench of 4/11: an after-call turn made its page, then
    /// asked for it 9 times more until its token limit, 333k tokens).
    pub fn same_write_again(&self, agent: &str, run_id: &str, tool: &str, calls: u32) {
        let Some(sub) = lock(&self.inner).subs.get(&(agent.to_string(), run_id.to_string())).cloned() else { return };
        let settings = self.config.as_ref().map(|c| c.get().brake.clone()).unwrap_or_default();
        let mut st = lock(&sub.state);
        if !settings.enabled || st.braked || st.finished {
            return;
        }
        let rescuing = st.rescue_of.is_some();
        let limits = if rescuing { settings.rescue_limits() } else { settings.limits(st.background) };
        if calls >= limits.same_call {
            let over = crate::loop_brake::Over::SameCall(crate::loop_brake::bare(tool).to_string());
            self.stop_looping(&sub, &mut st, over, rescuing, &settings);
        }
    }

    /// The person's answer to « Je tourne en rond » (turn_status.rs `looping`, `POST /v1/runs/{id}/brake`): `continue`
    /// raises this turn's limits once (the agent told, out of sight), `stop` takes the brake's stop at once (its plain
    /// line, its rescue turn: what it found, what blocked it). Errors: (status, code).
    pub fn brake_action(&self, agent: &str, run_id: &str, action: &str) -> Result<Value, (u16, &'static str)> {
        if !matches!(action, "continue" | "stop") {
            return Err((400, "unknown_action"));
        }
        let sub = lock(&self.inner).subs.get(&(agent.to_string(), run_id.to_string())).cloned().ok_or((404, "run_gone"))?;
        let config = self.config.as_ref().map(|c| c.get());
        let brake = config.as_ref().map(|c| c.brake.clone()).unwrap_or_default();
        let status_settings = config.as_ref().map(|c| c.status.clone()).unwrap_or_default();
        let mut st = lock(&sub.state);
        if st.finished || st.terminal.is_some() {
            return Err((404, "run_gone"));
        }
        if st.braked {
            return Err((409, "not_looping"));
        }
        if action == "continue" && st.brake_raised {
            return Err((409, "already_continued"));
        }
        let looping = st.status.as_ref().is_some_and(|t| t.is_looping());
        let Some(over) = st.loop_over.clone().filter(|_| looping) else { return Err((409, "not_looping")) };
        if action == "stop" {
            let rescuing = st.rescue_of.is_some();
            tracing::info!(agent = %agent, run_id = %run_id, "brake: the person said stop");
            self.stop_looping(&sub, &mut st, over, rescuing, &brake);
            return Ok(json!({"ok": true, "stopped": true}));
        }
        st.brake_raised = true;
        tracing::info!(agent = %agent, run_id = %run_id, "brake: the person said go on, limits raised once");
        let now = crate::py::time_time();
        if let Some(status) = st.status.as_mut().and_then(|t| t.continued(now, &status_settings)) {
            let event = self.status_event(&sub, status);
            append_to_backlog(&mut st.backlog, &event);
            st.send_all(Some(&event));
        }
        let (hermes, agent_cfg, run) = (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone());
        self.spawn(async move {
            let text = format!("{} A note from the app, not from your person: your person says to go on; you may take \
more steps in this turn. Don't repeat what failed: change approach.", crate::loop_brake::WARN_MARK);
            if let Err(err) = hermes.steer_run(&agent_cfg, &run, &text).await {
                tracing::info!(agent = %agent_cfg.name, run_id = %run, error = %err, "brake: go-on steer refused");
            }
        });
        Ok(json!({"ok": true, "continued": true}))
    }

    /// A run going round in circles stopped: the plain line in its stream, the rescue turn planned when it is a
    /// person's conversation, its own push when nobody looks, Hermes' stop.
    fn stop_looping(
        &self,
        sub: &RunSubscription,
        st: &mut SubscriptionState,
        over: crate::loop_brake::Over,
        rescuing: bool,
        settings: &crate::loop_brake::Settings,
    ) {
        let config = self.config.as_ref().map(|shared| shared.get());
        st.braked = true;
        // The safety net's rescue would start the same work again in a background task.
        st.rescued = true;
        let lang = config
            .as_ref()
            .map(|c| {
                let agent = c.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
                crate::handback::language_of(c, &agent)
            })
            .unwrap_or_else(|| "en".to_string());
        // A person's conversation turn (not a background task's, a routine's, a group discussion's, a peer's message,
        // nor a rescue turn itself): one short turn follows at its end to say what happened (`loop_rescue`).
        let rescue = settings.rescue
            && !st.background
            && !rescuing
            && (st.end_push || st.go_on_of.is_some() || st.restart_of.is_some())
            && st.room_id.is_none()
            && (st.watched || st.ever_followed);
        let data = crate::loop_brake::notice(&over, &lang, rescue);
        let text = data.get("text").and_then(Value::as_str).unwrap_or("").to_string();
        // the whole line (with what to do next), for a background turn whose person gets no other word
        st.brake_said = Some(crate::loop_brake::notice_text(&over, &lang));
        tracing::warn!(agent = %sub.agent.name, run_id = %sub.run_id, tool_calls = st.turn.total,
            background = st.background, rescue = rescue, rescue_turn = rescuing,
            reason = %crate::loop_brake::reason(&over), "going round in circles: run stopped");
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        let notice = SSEEvent::new(NOTICE, data, raw);
        append_to_backlog(&mut st.backlog, &notice);
        st.send_all(Some(&notice));
        if rescue {
            // the rescue turn's reply is the run's end (and its push when nobody looks)
            st.loop_rescue = Some((over, lang));
        }
        let unseen = st.followers() == 0 && ((st.watched && st.end_push) || (st.kept && !st.background));
        if let (true, false, false, Some(push)) = (unseen, rescue, rescuing, self.push.clone()) {
            // its own push instead of the run's end one (whose text would be the loop's last words)
            st.end_push = false;
            let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
            self.spawn(async move {
                push.notify_reply(&agent, &run_id, &text, session.as_deref()).await;
            });
        }
        let (hermes, agent, run_id) = (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone());
        self.spawn(async move {
            if let Err(err) = hermes.stop_run(&agent, &run_id).await {
                tracing::warn!(agent = %agent.name, run_id = %run_id, error = %err, "going round in circles: stop refused");
            }
        });
    }

    /// The person wrote again while this conversation turn goes on (threads/send.rs): when it did nothing yet (it
    /// only thought, wrote, or read: turn_restart.rs), it is to start again with the new words (`plan`), and the caller
    /// stops it now. False when it acted already (the words go in at its next step: Hermes' steer), when it is not a
    /// turn the person started (a routine's, a task's, a room's, a rescue), or when it already is to start again (the
    /// words wait for the turn after).
    pub fn interrupt(&self, agent: &str, run_id: &str, plan: crate::turn_restart::Plan) -> bool {
        let Some(sub) = self.subscription(agent, run_id) else { return false };
        let mut st = lock(&sub.state);
        let person_turn = st.session_id.is_some()
            && !st.unattended
            && !st.background
            && st.room_id.is_none()
            && st.rescue_of.is_none()
            && st.loop_rescue.is_none()
            && !st.braked
            && !st.capped
            && st.token_limit.is_none()
            && st.cost_limit.is_none();
        if st.finished || st.terminal.is_some() || st.acted || !person_turn || st.restart.is_some() || st.dropped {
            return false;
        }
        st.restart = Some(plan);
        tracing::info!(agent = %agent, run_id = %run_id, "restart: new words before the turn did anything, it starts again");
        true
    }

    /// The plan of `interrupt` withdrawn (the stop could not be asked): the words go the usual way.
    pub fn withdraw_restart(&self, agent: &str, run_id: &str) -> Option<crate::turn_restart::Plan> {
        let sub = self.subscription(agent, run_id)?;
        let mut st = lock(&sub.state);
        st.restart.take()
    }

    /// A run stopped for the person's new words (`interrupt`) ended: the same conversation's next turn starts at once
    /// with them, told its answer was cut (`turn_restart::NOTE`; only the next message when the run ended by itself
    /// meanwhile). Like the brake's rescue turn, its events go on in this run's stream after a `loop_rescue_started`
    /// notice with its run id (`reason: restarted`; the apps adopt it, nothing shown), and its end is this run's
    /// end (« reply ready », the queue). This run is no longer the conversation's (`restarted_as`, set with the plan
    /// cleared under one lock): a stop, a steer or another message goes to the new one; the new one is linked back
    /// (`restart_of`: its cards seen through this run's stream, the conversation nets). While it starts, the plan stays
    /// here: a second message meanwhile gets no plan of its own and waits first in line in the queue (threads/send.rs:
    /// never steered into a turn about to start again; Hermes' steer only if it raced that check). The person's Stop
    /// withdraws the plan (`withdraw_restart`): nothing starts, or the run just started is stopped; words they write
    /// after their Stop make a plan of their own, which starts (the plan read again before each try). Hermes refusing
    /// the start: the words wait first in line in the queue.
    async fn restart(self: &Arc<Self>, sub: &Arc<RunSubscription>, terminal: SSEEvent) -> SSEEvent {
        let (planned, session, watched, phone, since) = {
            let st = lock(&sub.state);
            (st.restart.is_some(), st.session_id.clone(), st.watched, st.phone.clone(), st.followed_since)
        };
        let (true, Some(session)) = (planned, session) else { return terminal };
        let agent = sub.agent.clone();
        let cut = matches!(terminal.r#type.as_str(), "run.cancelled" | "run.canceled" | "run.interrupted");
        // the plan as it is now: None once the person's Stop withdrew it, another one if they wrote again after it
        let current = || lock(&sub.state).restart.clone();
        let same = |a: &crate::turn_restart::Plan, b: &crate::turn_restart::Plan| {
            a.client_id == b.client_id && a.said == b.said
        };
        let mut attempt = 0u64;
        let (next, plan) = loop {
            let Some(plan) = current() else {
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, "restart: stopped by the person, nothing starts");
                return terminal;
            };
            let started = self
                .hermes
                .start_run_input(&agent, plan.input(cut), &session, plan.instructions.as_deref(), plan.options.clone())
                .await;
            let refused = match started {
                Ok(id) if crate::app::RUN_ID.is_match(&id) => {
                    // the plan done and the new run the conversation's, at once (no message falls between them), unless
                    // the plan was withdrawn (Stop) or replaced (words after a Stop) while it started
                    let now = {
                        let mut inner = lock(&self.inner);
                        let mut st = lock(&sub.state);
                        match st.restart.clone() {
                            Some(now) if same(&now, &plan) => {
                                st.restart = None;
                                st.restarted_as = Some(id.clone());
                                drop(st);
                                let restarted = self.get_or_start(&mut inner, &agent, &id);
                                let mut st = lock(&restarted.state);
                                st.kept = true;
                                st.watched = watched;
                                st.end_push = false; // this run's end tells it
                                st.session_id = Some(session.clone());
                                st.phone = plan.phone.clone().or(phone.clone());
                                st.followed_since = since; // the turn's clocks (safety net, working line) from its start
                                st.restart_of = Some(sub.run_id.clone());
                                None
                            }
                            other => Some(other),
                        }
                    };
                    let Some(other) = now else { break (id, plan) };
                    tracing::info!(agent = %agent.name, run_id = %sub.run_id, next = %id,
                        "restart: stopped by the person while it started, the new turn stopped");
                    if let Err(err) = self.hermes.stop_run(&agent, &id).await {
                        tracing::warn!(agent = %agent.name, run_id = %id, error = %err, "restart: stop refused");
                    }
                    if other.is_none() {
                        return terminal;
                    }
                    false // their words written after the Stop: started in turn
                }
                Ok(_) => true,
                Err(err) => {
                    tracing::info!(agent = %agent.name, run_id = %sub.run_id, error = %err, attempt = attempt,
                        "restart: new turn not started yet");
                    attempt += 1;
                    attempt >= 5
                }
            };
            if refused {
                // Hermes takes no new turn: the words wait first in line (none when the Stop withdrew them)
                let kept = lock(&sub.state).restart.take();
                let Some(plan) = kept else { return terminal };
                tracing::warn!(agent = %agent.name, run_id = %sub.run_id, "restart: no new turn, the words wait in line");
                if let Some(queues) = self.queues.get().and_then(std::sync::Weak::upgrade) {
                    let sent = crate::msg_queue::Sent {
                        client_id: plan.client_id.clone(),
                        input: Some(plan.input(cut)),
                        phone: plan.phone.clone().or(phone.clone()),
                        ..crate::msg_queue::Sent::default()
                    };
                    if queues.add_first(&agent, &session, &plan.said, sent).is_err() {
                        tracing::warn!(agent = %agent.name, "restart: the queue is full, the words are lost");
                    }
                }
                return terminal;
            }
            // Hermes may still be closing the stopped turn (the conversation busy for a moment)
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(400 * attempt)).await;
            }
        };
        tracing::info!(agent = %agent.name, run_id = %sub.run_id, next = %next, cut = cut, "restart: new turn started");
        if crate::turn_thinking::thought(plan.options.as_ref()) {
            crate::turn_thinking::thinking_run(&next);
        }
        let mut listener = self.subscribe(&agent, &next, "forward", false);
        let mut data = Map::new();
        data.insert("type".into(), json!(NOTICE));
        data.insert("code".into(), json!("loop_rescue_started"));
        data.insert("reason".into(), json!("restarted"));
        data.insert("run_id".into(), json!(next));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        self.forward(sub, &SSEEvent::new(NOTICE, data, raw));
        let end = loop {
            match listener.next().await {
                Some(event) if is_terminal(&event.r#type) => break Some(event),
                Some(event) => self.forward(sub, &event),
                None => break None,
            }
        };
        listener.close();
        end.unwrap_or(terminal)
    }

    /// The brake's rescue turn (2026-10-28, loop_brake.rs; Alex: a bare « I stopped » helps nobody): a person's
    /// conversation turn the brake stopped is followed, at its end, by ONE short turn of the same agent in the same
    /// conversation, told out of sight why it was stopped and to answer with what it has (`rescue_input`). Its events
    /// go on in this run's stream, after a `loop_rescue_started` notice that carries its run id (the app following
    /// this run sees the agent's reply come under the brake's short line), and its end is this run's end (the
    /// « reply ready » push, when nobody looks, is its reply). It has tight limits (`rescue_limits`), the stopped
    /// tool refused, and is never rescued itself. When it can't start, is stopped too, fails, runs past
    /// `rescue_seconds` or says nothing: a `loop_rescue_failed` notice with what the person can do, and this run's
    /// own end.
    async fn loop_rescue(self: &Arc<Self>, sub: &Arc<RunSubscription>, terminal: SSEEvent) -> SSEEvent {
        let (plan, session) = {
            let mut st = lock(&sub.state);
            (st.loop_rescue.take(), st.session_id.clone())
        };
        let Some((over, lang)) = plan else { return terminal };
        let settings = self.config.as_ref().map(|c| c.get().brake.clone()).unwrap_or_default();
        let agent = sub.agent.clone();
        let session = match session.filter(|s| !s.is_empty()) {
            Some(s) => Some(s),
            None => self.hermes.run(&agent, &sub.run_id).await.ok().and_then(|r| as_str(r.get("session_id"))),
        };
        let mut started = None;
        if let Some(session) = &session {
            let mut input = crate::loop_brake::rescue_input(&over, &lang);
            if let Some(config) = &self.config {
                input = crate::dates::now::stamped(&config.get(), &agent, &input);
            }
            // Hermes may still be closing the stopped turn (the conversation busy for a moment)
            for attempt in 0..4u64 {
                match self.hermes.start_run(&agent, &input, session, None).await {
                    Ok(id) if crate::app::RUN_ID.is_match(&id) => {
                        started = Some(id);
                        break;
                    }
                    Ok(_) => break,
                    Err(err) => {
                        tracing::info!(agent = %agent.name, run_id = %sub.run_id, error = %err, attempt = attempt,
                            "going round in circles: rescue turn not started yet");
                        if attempt < 3 {
                            tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                        }
                    }
                }
            }
        }
        let Some(rescue_id) = started else {
            tracing::warn!(agent = %agent.name, run_id = %sub.run_id, "going round in circles: no rescue turn");
            self.loop_rescue_failed(sub, &over, &lang);
            return terminal;
        };
        tracing::info!(agent = %agent.name, run_id = %sub.run_id, rescue = %rescue_id,
            "going round in circles: rescue turn started");
        let watched = lock(&sub.state).watched;
        {
            let mut inner = lock(&self.inner);
            let rescue = self.get_or_start(&mut inner, &agent, &rescue_id);
            let mut st = lock(&rescue.state);
            st.kept = true;
            // watched like the turn it follows (its approvals pushed when nobody looks), without the app's /v1/watch
            st.watched = watched;
            st.end_push = false; // this run's end tells it
            st.session_id = session.clone();
            st.rescue_of = Some(sub.run_id.clone());
            st.turn.banned = crate::loop_brake::stopped_tool(&over);
            drop(st);
            lock(&sub.state).continued_by = Some(rescue_id.clone());
        }
        // Its browser reads no page until its end (bip-browser-mcp refuses at once): the token lab's rescue turn
        // (2026-11-07) read the page it was stopped for again and waited the full 120 s. Not while another run of the
        // agent goes on (a background task browsing): the browser is the agent's, not the turn's.
        let browser_held = {
            let inner = lock(&self.inner);
            let others = inner.subs.values().any(|other| {
                other.agent.name == agent.name && other.run_id != sub.run_id && other.run_id != rescue_id && {
                    let st = lock(&other.state);
                    !st.finished && st.terminal.is_none()
                }
            });
            match (others, self.config.as_ref()) {
                (false, Some(shared)) => {
                    let deadline = crate::py::time_time() + settings.rescue_seconds + 5.0;
                    crate::browsers::rescue_started(&shared.get(), &agent.name, deadline)
                }
                _ => false,
            }
        };
        if browser_held {
            tracing::info!(agent = %agent.name, run_id = %rescue_id, "rescue turn: its browser reads no page");
        }
        let mut listener = self.subscribe(&agent, &rescue_id, "forward", false);
        let mut data = Map::new();
        data.insert("type".into(), json!(NOTICE));
        data.insert("code".into(), json!("loop_rescue_started"));
        data.insert("run_id".into(), json!(rescue_id));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        self.forward(sub, &SSEEvent::new(NOTICE, data, raw));
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(settings.rescue_seconds);
        let (mut end, mut said) = (None, String::new());
        loop {
            match tokio::time::timeout_at(deadline, listener.next()).await {
                Ok(Some(event)) if is_terminal(&event.r#type) => {
                    end = Some(event);
                    break;
                }
                // its own brake's line, the stream's hiccups: what the person reads is decided below
                Ok(Some(event)) if event.r#type == NOTICE || event.r#type == "bridge.error" => {}
                Ok(Some(event)) => {
                    if is_delta(&event.r#type) {
                        said.push_str(&event.text());
                    }
                    self.forward(sub, &event);
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!(agent = %agent.name, run_id = %rescue_id, "rescue turn too long: stopped");
                    if let Err(err) = self.hermes.stop_run(&agent, &rescue_id).await {
                        tracing::warn!(agent = %agent.name, run_id = %rescue_id, error = %err, "rescue turn: stop refused");
                    }
                    break;
                }
            }
        }
        let braked = lock(&listener.sub.state).braked;
        listener.close();
        if browser_held {
            if let Some(shared) = self.config.as_ref() {
                crate::browsers::rescue_ended(&shared.get(), &agent.name);
            }
        }
        match end {
            Some(end)
                if end.r#type == "run.completed"
                    && !braked
                    && !(said.trim().is_empty() && end.final_text().trim().is_empty()
                        && end.data.get("attachments").and_then(Value::as_array).is_none_or(Vec::is_empty)) =>
            {
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, rescue = %rescue_id,
                    "going round in circles: the rescue turn answered");
                end
            }
            _ => {
                tracing::warn!(agent = %agent.name, run_id = %sub.run_id, rescue = %rescue_id, braked = braked,
                    "going round in circles: the rescue turn did not answer");
                self.loop_rescue_failed(sub, &over, &lang);
                terminal
            }
        }
    }

    /// The rescue turn couldn't help: the person reads what they can do (`loop_rescue_failed`, under the brake's short
    /// line), and gets it pushed when nobody looks.
    fn loop_rescue_failed(&self, sub: &RunSubscription, over: &crate::loop_brake::Over, lang: &str) {
        let mut data = Map::new();
        data.insert("type".into(), json!(NOTICE));
        data.insert("code".into(), json!("loop_rescue_failed"));
        data.insert("reason".into(), json!(crate::loop_brake::reason(over)));
        data.insert("text".into(), json!(crate::loop_brake::notice_tail(over, lang)));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        self.forward(sub, &SSEEvent::new(NOTICE, data, raw));
        let mut st = lock(&sub.state);
        let unseen = st.followers() == 0 && st.watched && st.end_push;
        if let (true, Some(push)) = (unseen, self.push.clone()) {
            st.end_push = false;
            let text = crate::loop_brake::notice_text(over, lang);
            let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
            self.spawn(async move {
                push.notify_reply(&agent, &run_id, &text, session.as_deref()).await;
            });
        }
    }

    /// A person's conversation turn that ended on a promise with no work done (go_on.rs; Alex 13/11: « Je te prépare
    /// 2-3 trouvailles », the turn over with no search): ONE continuation of the same agent in the same conversation,
    /// told out of sight to do it now or to start a background task (`go_on::input`). Its events go on in this run's
    /// stream after a `loop_rescue_started` notice with its run id (the apps adopt it: its stop, its checks), and its
    /// end is this run's end (the « reply ready » push, when nobody looks, is its reply). It has the conversation's
    /// limits, the long-work net's hand-over and the brake's rescue, and is never continued itself. Never in a family
    /// agent's conversation, a guest's turn, a run followed again after a restart, nor once the person stepped in (Stop,
    /// words steered, a message waiting), checked again while it starts. It has the turn's instructions and options.
    /// Returns the end, and this run's own end when that one is the continuation's (handed over already; this run's
    /// files are merged in: `with_own_files`). When it can't start, fails or says nothing: this run's own end.
    async fn go_on(self: &Arc<Self>, sub: &Arc<RunSubscription>, terminal: SSEEvent) -> (SSEEvent, Option<SSEEvent>) {
        if terminal.r#type != "run.completed" {
            return (terminal, None);
        }
        let Some(shared) = self.config.clone() else { return (terminal, None) };
        // never in a family agent's common conversation (its creator's turns included), nor when switched off
        if !shared.get().brake.go_on || crate::family_thread::is_common(&shared.get(), &sub.agent.name) {
            return (terminal, None);
        }
        let (session, watched, followed, phone, instructions, options) = {
            let st = lock(&sub.state);
            let tools: Vec<String> = st
                .backlog
                .iter()
                .filter(|e| e.r#type == "tool.started")
                .map(|e| crate::loop_brake::event_tool(&e.data))
                .collect();
            if !go_on_fits(&st) || !crate::go_on::did_nothing(tools.iter().map(String::as_str)) {
                return (terminal, None);
            }
            (
                st.session_id.clone(),
                st.watched,
                st.ever_followed,
                st.phone.clone(),
                st.instructions.clone(),
                st.model_options.clone(),
            )
        };
        let said = terminal.final_text();
        // their next words already wait (words steered into the turn): they say what comes next
        let steered = as_str(terminal.data.get("pending_steer")).is_some_and(|p| !p.trim().is_empty());
        if !crate::go_on::promises(&said) || steered || self.someone_elses(&sub.run_id) {
            return (terminal, None);
        }
        let queues = self.queues.get().and_then(std::sync::Weak::upgrade);
        // words of theirs went into the turn (« laisse tomber »…): its last words answer them, nothing to go on with
        if queues.as_ref().is_some_and(|q| q.steered_into(&sub.agent.name, &sub.run_id)) {
            return (terminal, None);
        }
        let agent = sub.agent.clone();
        let session = match session.filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => match self.hermes.run(&agent, &sub.run_id).await.ok().and_then(|r| as_str(r.get("session_id"))) {
                Some(s) => s,
                None => return (terminal, None),
            },
        };
        // the person stepped in since the turn ended: Stop tapped, words steered in, or a message waiting in line for
        // this conversation (Hermes refused it to the ended turn): what they said decides, not the promise
        let stepped_in = || {
            // their words came meanwhile and are to start the turn again (turn_restart.rs, consumed after go_on)
            let here = {
                let st = lock(&sub.state);
                st.stop_asked || st.restart.is_some()
            };
            here || queues
                .as_ref()
                .is_some_and(|q| q.waits_in(&agent.name, &session) || q.steered_into(&agent.name, &sub.run_id))
        };
        if stepped_in() {
            return (terminal, None);
        }
        let input = crate::dates::now::stamped(&shared.get(), &agent, &crate::go_on::input(&said));
        let mut started = None;
        // Hermes may still be closing the turn (the conversation busy for a moment)
        for attempt in 0..4u64 {
            if stepped_in() {
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, "promise with no work: the person stepped in");
                return (terminal, None);
            }
            // with the turn's instructions (a shared conversation's) and its options (its thinking)
            match self.hermes.start_run_with(&agent, &input, &session, instructions.as_deref(), options.clone()).await {
                Ok(id) if crate::app::RUN_ID.is_match(&id) => {
                    started = Some(id);
                    break;
                }
                Ok(_) => break,
                Err(err) => {
                    tracing::info!(agent = %agent.name, run_id = %sub.run_id, error = %err, attempt = attempt,
                        "promise with no work: continuation not started yet");
                    if attempt < 3 {
                        tokio::time::sleep(Duration::from_millis(500 * (attempt + 1))).await;
                    }
                }
            }
        }
        let Some(next_id) = started else {
            tracing::warn!(agent = %agent.name, run_id = %sub.run_id, "promise with no work: no continuation");
            return (terminal, None);
        };
        tracing::info!(agent = %agent.name, run_id = %sub.run_id, continuation = %next_id,
            "promise with no work: continuation started");
        {
            let mut inner = lock(&self.inner);
            let next = self.get_or_start(&mut inner, &agent, &next_id);
            let mut st = lock(&next.state);
            st.kept = true;
            // watched like the turn it follows (its approvals pushed when nobody looks), without the app's /v1/watch
            st.watched = watched;
            st.ever_followed = followed;
            st.end_push = false; // this run's end tells it
            st.session_id = Some(session.clone());
            st.go_on_of = Some(sub.run_id.clone());
            // where a reminder it sets goes: the phone of the person's words
            if st.phone.is_none() {
                st.phone = phone;
            }
            drop(st);
            lock(&sub.state).continued_by = Some(next_id.clone());
        }
        // they stepped in while it started (from now on their Stop and words reach it): stopped, the turn's own end
        if stepped_in() {
            tracing::info!(agent = %agent.name, run_id = %sub.run_id, continuation = %next_id,
                "promise with no work: the person stepped in, continuation stopped");
            lock(&sub.state).continued_by = None;
            if let Some(next) = self.subscription(&agent.name, &next_id) {
                lock(&next.state).dropped = true;
            }
            if let Err(err) = self.hermes.stop_run(&agent, &next_id).await {
                tracing::warn!(agent = %agent.name, run_id = %next_id, error = %err, "continuation: stop refused");
            }
            return (terminal, None);
        }
        let mut listener = self.subscribe(&agent, &next_id, "forward", false);
        let mut data = Map::new();
        data.insert("type".into(), json!(NOTICE));
        data.insert("code".into(), json!("loop_rescue_started"));
        data.insert("run_id".into(), json!(next_id));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        self.forward(sub, &SSEEvent::new(NOTICE, data, raw));
        // its words start on a line of their own under the promise already shown
        let gap = json!({"event": "message.delta", "run_id": next_id, "delta": "\n\n"});
        let raw = crate::py::Dumps::new().unicode().dumps(&gap);
        self.forward(sub, &SSEEvent::new("message.delta", gap.as_object().cloned().unwrap_or_default(), raw));
        let deadline = tokio::time::Instant::now() + Duration::from_secs_f64(crate::go_on::MAX_SECONDS);
        let (mut end, mut words) = (None, String::new());
        loop {
            match tokio::time::timeout_at(deadline, listener.next()).await {
                Ok(Some(event)) if is_terminal(&event.r#type) => {
                    end = Some(event);
                    break;
                }
                Ok(Some(event)) if event.r#type == "bridge.error" => {}
                Ok(Some(event)) => {
                    if is_delta(&event.r#type) {
                        words.push_str(&event.text());
                    }
                    self.forward(sub, &event);
                }
                Ok(None) => break,
                Err(_) => {
                    // it goes on alone: its own end tells the person (its « reply ready » when nobody looks)
                    tracing::warn!(agent = %agent.name, run_id = %next_id, "continuation followed too long: let go");
                    lock(&listener.sub.state).end_push = watched;
                    break;
                }
            }
        }
        let stopped = lock(&listener.sub.state).stop_asked;
        listener.close();
        match end {
            // the person stopped it: the turn ends stopped, not on its promise
            Some(end) if stopped || end.r#type == "run.cancelled" => {
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, continuation = %next_id,
                    "promise with no work: the continuation was stopped");
                (end, None)
            }
            // its words, or files: Hermes' « no reply » line alone (made plain: `bip_no_reply`) is no answer, the turn
            // keeps its own end; after words of its own (cut short), they are its answer
            Some(end)
                if end.r#type == "run.completed"
                    && !(end.data.get("bip_no_reply").and_then(Value::as_bool) == Some(true)
                        && words.trim().is_empty())
                    && !(words.trim().is_empty()
                        && end.final_text().trim().is_empty()
                        && end.data.get("attachments").and_then(Value::as_array).is_none_or(Vec::is_empty)) =>
            {
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, continuation = %next_id,
                    "promise with no work: the continuation answered");
                (end, Some(terminal))
            }
            _ => {
                tracing::warn!(agent = %agent.name, run_id = %sub.run_id, continuation = %next_id,
                    "promise with no work: the continuation did not answer");
                (terminal, None)
            }
        }
    }

    /// The continuation's end (handed over already in its own run) with the files this turn's own reply cites or made
    /// (« Tes idées sont dans /envoi/idees.md. Je te prépare aussi les liens »): kept on the promise's message, and
    /// joined to the end the app reads, once each.
    async fn with_own_files(&self, sub: &Arc<RunSubscription>, own: SSEEvent, end: SSEEvent) -> SSEEvent {
        let own = self.hand_over(sub, own).await;
        let paths = |data: &Map<String, Value>| -> Vec<String> {
            data.get("attachments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| as_str(a.get("path")))
                .collect()
        };
        let (have, theirs) = (paths(&end.data), paths(&own.data));
        let own_stale = own.data.get("stale").and_then(Value::as_array).cloned().unwrap_or_default();
        if theirs.iter().all(|p| have.contains(p)) && own_stale.is_empty() {
            return end;
        }
        let mut data = end.data;
        let mut items: Vec<Value> = data.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
        items.extend(
            own.data
                .get("attachments")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|a| as_str(a.get("path")).is_none_or(|p| !have.contains(&p)))
                .cloned(),
        );
        data.insert("attachments".into(), json!(items));
        let mut stale: Vec<Value> = data.get("stale").and_then(Value::as_array).cloned().unwrap_or_default();
        for s in own_stale {
            if !stale.contains(&s) {
                stale.push(s);
            }
        }
        data.insert("stale".into(), json!(stale));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(end.r#type, data, raw)
    }

    /// What the turn made (made_files.rs: audio, pages, maps) and the files its reply only cites from the agent's own
    /// folders (cited_files.rs), handed over as the end's `attachments` (structured items; `stale`: the `MEDIA:` lines
    /// of older files the model wrote from memory) and kept on the stored message (threads.rs `attach_with`): the model's
    /// words are never rewritten for files (2026-11-03). The vault's chips and a rename still join its words (marks).
    async fn hand_over(&self, sub: &Arc<RunSubscription>, event: SSEEvent) -> SSEEvent {
        if event.r#type != "run.completed" {
            return event;
        }
        let agent = sub.agent.name.as_str();
        let mut made = crate::made_files::MADE.in_run(agent, &sub.run_id);
        // the « Coordonnées » a listing gave in the turn: a chip for those the reply uses (vault_contacts.rs)
        let mut listed = crate::vault_contacts::listed_in_run(agent, &sub.run_id);
        if let Some(other) = as_str(event.data.get("run_id")).filter(|r| *r != sub.run_id) {
            made.extend(crate::made_files::MADE.in_run(agent, &other));
            listed.extend(crate::vault_contacts::listed_in_run(agent, &other));
        }
        let said = lock(&sub.state).media.said().to_string();
        let written = as_str(event.data.get("output")).filter(|o| !o.trim().is_empty()).unwrap_or(said.clone());
        made.extend(crate::vault_contacts::used_marks(&written, &listed));
        let (marks, files): (Vec<String>, Vec<String>) = made.into_iter().partition(|m| crate::made_files::is_mark(m));
        // files: the lines the old hand-over would have added or dropped, as items
        // each map once: what the reply's pages hold of the turn's maps (read only when it made one)
        let pages_config = self.config.as_ref().map(|shared| shared.get());
        let maps_of =
            |path: &str| pages_config.as_ref().map(|c| crate::pages::maps_of(c, agent, path)).unwrap_or_default();
        let fixed =
            crate::made_files::fix_reply_with(&written, agent, &files, &maps_of).unwrap_or_else(|| written.clone());
        let (_, before) = crate::media::extract_media(&written);
        let (_, after) = crate::media::extract_media(&fixed);
        let mut added: Vec<String> = after.iter().filter(|p| !before.contains(p)).cloned().collect();
        let stale: Vec<String> = before.iter().filter(|p| !after.contains(p)).cloned().collect();
        if let (Some(shared), true) = (self.config.clone(), crate::cited_files::may_cite(&written)) {
            let (agent, text) = (sub.agent.clone(), written.clone());
            let cited = tokio::task::spawn_blocking(move || {
                let config = shared.get();
                let agent = config.agent(Some(&agent.name)).cloned().unwrap_or(agent);
                crate::cited_files::cited(&config, &agent, &text)
            })
            .await
            .unwrap_or_default();
            added.extend(cited.into_iter().filter(|p| !before.contains(p) && !after.contains(p)));
        }
        let mut data = event.data;
        // files the end hands over already (a rescue turn's own end, handed over in its own run): once
        let have: Vec<String> = data
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|a| as_str(a.get("path")))
            .collect();
        added.retain(|p| !have.contains(p));
        let marked = crate::made_files::fix_reply(&written, agent, &marks);
        if let Some(fixed) = &marked {
            if !said.trim().is_empty() {
                // the screen gets the marks it was not streamed
                let lines: String = marks.iter().filter(|m| !said.contains(m.as_str())).map(|m| format!("\n\n{m}")).collect();
                let delta = json!({"event": "message.delta", "run_id": data.get("run_id").cloned().unwrap_or(json!(sub.run_id)),
                                   "delta": lines});
                let raw = crate::py::Dumps::new().unicode().dumps(&delta);
                self.forward(sub, &SSEEvent::new("message.delta", delta.as_object().cloned().unwrap_or_default(), raw));
            }
            data.insert("output".into(), json!(fixed));
        }
        if added.is_empty() && stale.is_empty() && marked.is_none() {
            return SSEEvent::new(event.r#type, data, event.raw);
        }
        if !added.is_empty() || !stale.is_empty() {
            tracing::info!(agent = %agent, run_id = %sub.run_id, files = added.len(), stale = stale.len(),
                "files of the turn handed over as attachments");
            let mut items: Vec<Value> = data.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
            items.extend(added.iter().map(|p| crate::threads::timeline::file_attachment(p, None)));
            data.insert("attachments".into(), json!(items));
            data.insert("stale".into(), json!(stale));
            self.attach_handed_over(sub, &written, added, stale).await;
        }
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// The files handed over, kept on the stored message as its attachments (threads.rs `attach_with`): the timeline
    /// shows them at every read. Kept before the run's end is passed on (4/11): the apps read the timeline at once when
    /// the end comes, and a page catching up (`after=`) never gives again an item it gave, so a reply read before its
    /// files were kept stayed without them on that phone. A message Hermes has not stored yet is waited for
    /// `INLINE_WAIT` at most here, then in the background, the thread signalled once its files are kept.
    async fn attach_handed_over(&self, sub: &Arc<RunSubscription>, written: &str, added: Vec<String>, stale: Vec<String>) {
        const INLINE_WAIT: f64 = 3.0;
        let (Some(threads), Some(session)) = (crate::threads::upgrade(&self.threads), lock(&sub.state).session_id.clone())
        else {
            return;
        };
        let (agent, wait) = (sub.agent.clone(), self.message_wait());
        if let Some((thread, message)) = threads.find_message(&agent, &session, Some(written), wait.min(INLINE_WAIT)).await {
            threads.attach_with(&agent, &thread, &message, &added, &stale).await;
            return;
        }
        let written = written.to_string();
        self.spawn(async move {
            match threads.find_message(&agent, &session, Some(&written), (wait - INLINE_WAIT).max(0.0)).await {
                Some((thread, message)) => {
                    threads.attach_with(&agent, &thread, &message, &added, &stale).await;
                    threads.touch(&agent.name, Some(&session), "files").await;
                }
                None => tracing::warn!(agent = %agent.name, session = %session,
                    "handed-over files not attached: the reply's message not found"),
            }
        });
    }

    fn message_wait(&self) -> f64 {
        self.config.as_ref().map(|c| c.get().push_message_wait_seconds).unwrap_or(10.0)
    }

    /// The reply's last words held back as maybe a `MEDIA:` line (media.rs), given before its end.
    fn flush_media(&self, sub: &RunSubscription) {
        let held = lock(&sub.state).media.flush_events("message.delta", &json!(sub.run_id));
        for event in &held {
            self.forward(sub, event);
        }
    }

    /// An event shown in this run's stream as it is (the rescue turn's, the bridge's word), without this run's own
    /// handling of it (brake, approvals, end).
    fn forward(&self, sub: &RunSubscription, event: &SSEEvent) {
        let mut st = lock(&sub.state);
        append_to_backlog(&mut st.backlog, event);
        st.send_all(Some(event));
    }

    /// A turn nobody started from an app (background_turn.rs `follow`): what it asks while no app follows it is kept
    /// by the bridge (held_approvals.rs).
    pub fn mark_unattended(&self, agent: &str, run_id: &str) {
        let inner = lock(&self.inner);
        if let Some(sub) = inner.subs.get(&(agent.to_string(), run_id.to_string())) {
            lock(&sub.state).unattended = true;
        }
    }

    /// Nobody can answer the run now: a turn nobody started from an app, no app following it, not a room's, not a
    /// relative's, and the bridge takes such requests.
    fn nobody_there(&self, sub: &RunSubscription, st: &SubscriptionState) -> bool {
        st.unattended
            && st.followers() == 0
            && st.room_id.is_none()
            && self.unattended.get().is_some()
            && !self.someone_elses(&sub.run_id)
    }

    /// `agent`'s run `run_id` when nobody can answer it now (`nobody_there`): Some(its session), else None. A bridge
    /// tool that would ask the person holds its card instead (terms_consent.rs, held_approvals.rs).
    pub fn nobody_there_for(&self, agent: &str, run_id: &str) -> Option<Option<String>> {
        let sub = lock(&self.inner).subs.get(&(agent.to_string(), run_id.to_string())).cloned()?;
        let st = lock(&sub.state);
        self.nobody_there(&sub, &st).then(|| st.session_id.clone())
    }

    /// A question of a run nobody can answer now, handed to the bridge (answered at once). False: asked as usual.
    fn hold_question(&self, sub: &RunSubscription, st: &SubscriptionState, event: &SSEEvent) -> bool {
        if !self.nobody_there(sub, st) {
            return false;
        }
        let Some(tx) = self.unattended.get() else { return false };
        let request = crate::held_approvals::Unattended::Question {
            agent: sub.agent.clone(),
            run_id: sub.run_id.clone(),
            request_id: clarify_id(&event.data),
        };
        tx.send(request).is_ok()
    }

    /// A background task's run (jobs.rs): the brake's higher limits.
    pub fn mark_background(&self, agent: &str, run_id: &str) {
        let inner = lock(&self.inner);
        if let Some(sub) = inner.subs.get(&(agent.to_string(), run_id.to_string())) {
            lock(&sub.state).background = true;
        }
    }

    /// The agent's runs going on that are inside a call of the bridge's tool `tool` right now (their last
    /// `tool.started` was it, not yet completed): who a tool call may belong to when several people's turns go on at
    /// once (shared_pages.rs `who`). Empty when the hub has not seen the call start yet.
    pub fn runs_in_tool(&self, agent: &str, tool: &str) -> Vec<String> {
        let tool = crate::loop_brake::bare(tool);
        let inner = lock(&self.inner);
        let mut runs: Vec<String> = Vec::new();
        for sub in inner.subs.values().filter(|sub| sub.agent.name == agent) {
            let st = lock(&sub.state);
            let inside = !st.finished && st.terminal.is_none() && st.tools_open > 0 && st.last_tool == tool;
            if inside && !runs.contains(&sub.run_id) {
                runs.push(sub.run_id.clone());
            }
        }
        runs
    }

    /// The agent's runs going on that are inside a call of `tool` right now, whatever MCP server Hermes named it after
    /// (`mcp_bipagents_pay_pay_with_card`, `mcp__bipagents_pay__pay_with_card`: `bare` keeps the server's name there),
    /// the call not completed yet: whose payment it is (urgent_pay.rs `refuses_payment`).
    pub fn runs_calling(&self, agent: &str, tool: &str) -> Vec<String> {
        let suffix = format!("_{tool}");
        let inner = lock(&self.inner);
        let mut runs: Vec<String> = Vec::new();
        for sub in inner.subs.values().filter(|sub| sub.agent.name == agent) {
            let st = lock(&sub.state);
            let named = st.last_tool == tool || st.last_tool.ends_with(&suffix);
            if !st.finished && st.terminal.is_none() && st.tools_open > 0 && named && !runs.contains(&sub.run_id) {
                runs.push(sub.run_id.clone());
            }
        }
        runs
    }

    /// The agent's runs going on that are inside a call whose tool `is` says, right now (their last `tool.started`, not
    /// completed yet), with their Hermes session: whose call it is, from the call's own turn (vault_line.rs `note`),
    /// never « the run that called a tool last ».
    pub fn runs_in_call(&self, agent: &str, is: impl Fn(&str) -> bool) -> Vec<(String, Option<String>)> {
        let inner = lock(&self.inner);
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        for sub in inner.subs.values().filter(|sub| sub.agent.name == agent) {
            let st = lock(&sub.state);
            let inside = !st.finished && st.terminal.is_none() && st.tools_open > 0 && is(&st.last_tool);
            if inside && !out.iter().any(|(run, _)| *run == sub.run_id) {
                out.push((sub.run_id.clone(), st.session_id.clone()));
            }
        }
        out
    }

    /// The turn a call of the bridge's tool `tool` by `agent` belongs to (loop_brake.rs, its MCP counts): the agent's
    /// run going on that the hub follows; with several, the one whose last call was this tool, else the one that
    /// called a tool last. None: no run of the agent followed (a routine, Hermes' cron).
    pub fn turn_of(&self, agent: &str, tool: &str) -> Option<crate::loop_brake::Turn> {
        let tool = crate::loop_brake::bare(tool);
        let inner = lock(&self.inner);
        let mut best: Option<((bool, f64), crate::loop_brake::Turn)> = None;
        for sub in inner.subs.values().filter(|sub| sub.agent.name == agent) {
            let st = lock(&sub.state);
            if st.finished || st.terminal.is_some() || st.braked {
                continue;
            }
            let key = (st.last_tool == tool, st.last_tool_at.max(st.followed_since));
            if best.as_ref().is_none_or(|(k, _)| key > *k) {
                let turn = crate::loop_brake::Turn {
                    run_id: sub.run_id.clone(),
                    background: st.background,
                    banned: st.turn.banned.clone(),
                };
                best = Some((key, turn));
            }
        }
        best.map(|(_, turn)| turn)
    }

    /// The long-work safety net (decided 2026-10-22, independent of the model): a run in a person's conversation (not a
    /// background task's own, whose watch has no end push) that passes `[jobs] handoff_tool_calls` tool calls or
    /// `handoff_seconds` is steered once, at a tool boundary, to hand the rest over to a background task. Not when the
    /// agent already did (start_task, propose_task, notebook_fill); never while a sub-agent works for the run (the
    /// steer reached the agent with the sub-agent's result and stopped it there, 2026-10-24); a browsing run (a
    /// browser tool among its last `handoff_browser_window` calls) by `handoff_browser_*` only (never by default), and
    /// neither the calls nor the time spent browsing or delegating count for the others. The apps hide the steered
    /// words (HANDOFF_MARK).
    fn handoff(&self, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        let tool = event.data.get("tool").or_else(|| event.data.get("name")).and_then(Value::as_str).unwrap_or("");
        let now = crate::py::time_time();
        if matches!(event.r#type.as_str(), "tool.completed" | "tool.failed") {
            st.tools_open = st.tools_open.saturating_sub(1);
        }
        match event.r#type.as_str() {
            "tool.started" => {
                st.tool_calls += 1;
                if st.tools_open == 0 {
                    st.tool_steps += 1;
                }
                st.tools_open += 1;
                if st.recent_tools.len() >= 100 {
                    st.recent_tools.pop_front();
                }
                st.recent_tools.push_back(tool.to_string());
                // what the conversation read, for the amber line of a tool card asked later in it (review R4)
                crate::tool_asks::note_read(&sub.agent.name, st.session_id.as_deref(), tool);
                if handoff_tool(tool) {
                    st.handed_off = true;
                }
                if delegate_tool(tool) {
                    st.delegating += 1;
                } else if !browser_tool(tool) && st.delegating == 0 && st.subagents == 0 {
                    st.counted_calls += 1;
                }
            }
            "tool.completed" => {
                if delegate_tool(tool) {
                    st.delegating = st.delegating.saturating_sub(1);
                    st.exempt_until = now;
                    st.counted_calls = 0;
                }
                if browser_tool(tool) {
                    st.exempt_until = now;
                }
                if handoff_tool(tool) && handoff_refused(event) {
                    tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, tool = tool, "long run: hand-over refused");
                    st.handed_off = false;
                    st.handoff_failed = true;
                }
            }
            SUBAGENT_START => st.subagents += 1,
            SUBAGENT_DONE => {
                st.subagents = st.subagents.saturating_sub(1);
                st.exempt_until = now;
                st.counted_calls = 0;
            }
            _ => {}
        }
        // a run kept for the brake only (a peer's message nobody looks at): no steer, no rescue, as before
        let held = st.delegating > 0 || st.subagents > 0 || (st.kept && !st.watched && !st.ever_followed);
        // a continuation (go_on) is a person's conversation turn like the one it follows: long work goes to a task
        let conversation = st.end_push || st.go_on_of.is_some() || st.restart_of.is_some();
        if st.steered || st.braked || st.handed_off || !conversation || st.finished || held {
            return;
        }
        if !matches!(event.r#type.as_str(), "tool.started" | "tool.completed") {
            return;
        }
        let Some(shared) = self.config.as_ref() else { return };
        let jobs = shared.get().jobs.clone();
        if !jobs.handoff {
            return;
        }
        let window = jobs.handoff_browser_window.max(1) as usize;
        let browsing = st.recent_tools.iter().rev().take(window).any(|t| browser_tool(t));
        let over = if browsing {
            let seconds = now - st.followed_since;
            (jobs.handoff_browser_tool_calls > 0 && st.tool_calls >= jobs.handoff_browser_tool_calls)
                || (jobs.handoff_browser_seconds > 0.0 && seconds >= jobs.handoff_browser_seconds)
        } else {
            let seconds = now - st.followed_since.max(st.exempt_until);
            st.counted_calls >= jobs.handoff_tool_calls || seconds >= jobs.handoff_seconds
        };
        if !over {
            return;
        }
        st.steered = true;
        let seconds = now - st.followed_since;
        let (hermes, agent, run_id, calls) =
            (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone(), st.tool_calls);
        tracing::info!(agent = %agent.name, run_id = %run_id, tool_calls = calls, seconds = seconds.round() as i64,
            browsing = browsing, "long run: handing over to a background task (steer)");
        self.spawn(async move {
            if let Err(err) = hermes.steer_run(&agent, &run_id, &format!("{HANDOFF_MARK} {HANDOFF_TEXT}")).await {
                tracing::warn!(agent = %agent.name, run_id = %run_id, error = %err, "long run: steer refused");
            }
        });
    }

    /// The safety net's second half (2026-10-24: never end silently): a run it steered that ends (completed or failed)
    /// without handing over (no start_task, propose_task or notebook_fill that worked) and without a real answer (its
    /// final text shorter than `RESCUE_REPLY_CHARS`: « je continue en arrière-plan » said but not done, or nothing) has
    /// its task started by the bridge: the person's request (the conversation's last message from them) and what the
    /// run did so far (its tool steps, its words), within `[jobs] direct_budget_eur`. Refused (another task runs, too
    /// many at once…): proposed instead, the card with « Démarrer » in the conversation; that refused too: said. The
    /// conversation is told by a `bridge.notice` event (before the run's end, so a follower shows it), and a push.
    async fn rescue(self: &Arc<Self>, sub: &Arc<RunSubscription>, terminal: &SSEEvent) {
        if !matches!(terminal.r#type.as_str(), "run.completed" | "run.failed") {
            return;
        }
        let Some(shared) = self.config.clone() else { return };
        let config = shared.get();
        if !config.jobs.handoff || !config.jobs.handoff_rescue {
            return;
        }
        let output = terminal.final_text();
        let (session, steps, said) = {
            let mut st = lock(&sub.state);
            if !st.steered
                || st.handed_off
                || !(st.end_push || st.go_on_of.is_some() || st.restart_of.is_some())
                || st.rescued
            {
                return;
            }
            let said: String = st.backlog.iter().filter(|e| is_delta(&e.r#type)).map(|e| e.text()).collect();
            // The words after its last tool step: its answer, if it finished after all.
            let last_tool = st.backlog.iter().rposition(|e| e.r#type.starts_with("tool."));
            let answer: String = st
                .backlog
                .iter()
                .skip(last_tool.map(|i| i + 1).unwrap_or(0))
                .filter(|e| is_delta(&e.r#type))
                .map(|e| e.text())
                .collect();
            if output.trim().chars().count().max(answer.trim().chars().count()) >= RESCUE_REPLY_CHARS {
                return;
            }
            st.rescued = true;
            let steps: Vec<String> = st
                .backlog
                .iter()
                .filter(|e| e.r#type == "tool.started")
                .filter_map(|e| {
                    let tool = as_str(e.data.get("tool")).unwrap_or_default();
                    if tool.is_empty() {
                        return None;
                    }
                    let preview = as_str(e.data.get("preview")).unwrap_or_default();
                    let preview: String = preview.chars().take(160).collect();
                    Some(if preview.is_empty() { format!("- {tool}") } else { format!("- {tool}: {preview}") })
                })
                .collect();
            (st.session_id.clone(), steps, said)
        };
        let Some(jobs) = self.jobs.get().and_then(std::sync::Weak::upgrade) else {
            tracing::warn!(agent = %sub.agent.name, run_id = %sub.run_id, "long run: no background tasks to hand over to");
            return;
        };
        let agent = sub.agent.clone();
        let session = match session.filter(|s| !s.is_empty()) {
            Some(s) => Some(s),
            None => self.hermes.run(&agent, &sub.run_id).await.ok().and_then(|r| as_str(r.get("session_id"))),
        };
        let request = match &session {
            Some(sid) => {
                self.hermes.session_messages(&agent, sid).await.map(|m| person_request(&m)).unwrap_or_default()
            }
            None => String::new(),
        };
        let title = rescue_title(&request);
        let prompt = rescue_prompt(&request, &steps, &said);
        let budget = crate::jobs::direct_budget_eur(&config);
        let started = tokio::time::timeout(
            Duration::from_secs(45),
            jobs.create_from(&agent, &title, &prompt, budget, None, "asked", session.as_deref()),
        )
        .await
        .unwrap_or_else(|_| Err(crate::jobs::JobError("the agent did not answer in time".into()).into()));
        let mut notice = Map::new();
        notice.insert("type".into(), json!(NOTICE));
        notice.insert("title".into(), json!(title));
        match started {
            Ok(job) => {
                let job_id = crate::tools_knowledge::str_or_empty(job.get("id"));
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, job = %job_id,
                    "long run: the bridge started the background task");
                notice.insert("code".into(), json!("handoff_started"));
                notice.insert("job_id".into(), json!(job_id));
                notice.insert(
                    "text".into(),
                    json!(format!(
                        "La suite continue en tâche de fond : « {title} ». Je te préviens quand c'est fini."
                    )),
                );
                if let Some(push) = self.push.clone() {
                    let (name, session) = (agent.name.clone(), session.clone());
                    let line = format!("{RESCUE_BODY} : {title}");
                    self.spawn(async move {
                        push.notify_job(&name, session.as_deref(), &line, &job_id, RESCUE_BODY).await;
                    });
                }
            }
            Err(err) => {
                let reason = format!("{err:#}");
                tracing::info!(agent = %agent.name, run_id = %sub.run_id, error = %reason,
                    "long run: the background task could not start, proposed");
                match jobs.propose_from(&agent, &title, &prompt, "small", session.as_deref()).await {
                    Ok(job) => {
                        notice.insert("code".into(), json!("handoff_proposed"));
                        notice.insert("job_id".into(), json!(crate::tools_knowledge::str_or_empty(job.get("id"))));
                        notice.insert("reason".into(), json!(reason));
                        notice.insert(
                            "text".into(),
                            json!(format!(
                                "Je n'ai pas pu continuer en tâche de fond ({reason}). La tâche « {title} » t'attend : \
touche « Démarrer » pour reprendre."
                            )),
                        );
                    }
                    Err(again) => {
                        let reason = format!("{again:#}");
                        tracing::warn!(agent = %agent.name, run_id = %sub.run_id, error = %reason,
                            "long run: the background task could not be proposed either");
                        notice.insert("code".into(), json!("handoff_failed"));
                        notice.insert("reason".into(), json!(reason));
                        let text = format!(
                            "Je me suis arrêté avant la fin et je n'ai pas pu continuer en tâche de fond ({reason}). \
Écris « Reprends » pour que je continue."
                        );
                        notice.insert("text".into(), json!(text));
                        if let Some(push) = self.push.clone() {
                            let (name, run_id, session) = (agent.name.clone(), sub.run_id.clone(), session.clone());
                            self.spawn(async move {
                                push.notify_reply(&name, &run_id, &text, session.as_deref()).await;
                            });
                        }
                    }
                }
            }
        }
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(notice.clone()));
        self.dispatch(sub, SSEEvent::new(NOTICE, notice, raw));
    }

    /// A question kept while Hermes waits for its answer (`pending_questions`).
    fn keep_question(inner: &mut HubInner, sub: &RunSubscription, st: &SubscriptionState, event: &SSEEvent) {
        let request_id = clarify_id(&event.data);
        let mut item = Map::new();
        item.insert("kind".into(), json!("question"));
        item.insert("agent".into(), json!(sub.agent.name));
        item.insert("run_id".into(), json!(sub.run_id));
        item.insert("request_id".into(), json!(request_id));
        item.insert("session_id".into(), json!(st.session_id));
        item.insert("created_at".into(), json!(crate::py::time_time()));
        for key in ["questions", "question", "choices", "execution_generation"] {
            if let Some(value) = event.data.get(key) {
                item.insert(key.into(), value.clone());
            }
        }
        let key = format!("clarify:{}", request_id.unwrap_or_else(|| sha16(&event.raw)));
        inner.pending.insert((sub.agent.name.clone(), sub.run_id.clone(), key), item);
    }

    /// A question (docs/hermes-clarify-api.md) nobody sees: push it once, like an approval.
    fn on_clarify(&self, inner: &HubInner, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        let data = &event.data;
        let request_id = clarify_id(data);
        let dedupe =
            format!("clarify:{}", request_id.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| sha16(&event.raw)));
        let Some(push) = self.push.clone() else { return };
        if !st.watched
            || st.followers() > 0
            || Self::followed_through(inner, sub, st)
            || st.pushed.contains(&dedupe)
            || self.someone_elses(&sub.run_id)
        {
            return;
        }
        let questions = match data.get("questions") {
            Some(Value::Array(q)) => q.clone(),
            _ => Vec::new(),
        };
        let first = match questions.first() {
            Some(Value::Object(q)) => q,
            _ => data,
        };
        let question = as_str(first.get("question"));
        st.pushed.insert(dedupe);
        tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, "question push");
        let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
        self.spawn(async move {
            push.notify_question(&agent, &run_id, request_id.as_deref(), question.as_deref(), session.as_deref()).await;
        });
    }

    /// An approval in plain words (approval_text.rs): `bip_summary` and `bip_detail` added to the event before anyone
    /// sees it (the conversation's stream, the room, the Boîte, the push), in the agent's language, with its name and
    /// the tool it started just before.
    fn in_plain_words(&self, sub: &RunSubscription, st: &SubscriptionState, event: SSEEvent) -> SSEEvent {
        if event.data.contains_key("bip_summary") {
            return event;
        }
        // a script « dans son espace » only when the agent's programs really run in its sandbox (audit #14); on the
        // offer no command line in the sentence
        let (lang, place) = match &self.config {
            Some(shared) => {
                let config = shared.get();
                let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
                let place = crate::approval_text::Place::of(crate::sandbox::isolates(&config, &agent), config.cloud);
                (crate::handback::language_of(&config, &agent), place)
            }
            None => ("en".to_string(), crate::approval_text::Place::Machine),
        };
        let last = st.backlog.iter().rev().find(|e| e.r#type == "tool.started").map(|e| {
            let text = |key: &str| e.data.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            let tool = text("tool");
            crate::approval_text::LastTool { name: if tool.is_empty() { text("name") } else { tool }, preview: text("preview") }
        });
        let name = if sub.agent.display_name.trim().is_empty() { &sub.agent.name } else { &sub.agent.display_name };
        let Some(described) = crate::approval_text::describe(name, &lang, &event.data, last.as_ref(), place) else { return event };
        let mut data = event.data;
        data.insert("bip_summary".into(), json!(described.summary));
        if let Some(detail) = described.detail {
            data.insert("bip_detail".into(), json!(detail));
        }
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// An approval whose kind a « Toujours » may cover (approval_level.rs): its answer `bip_always` (the button's words)
    /// and `bip_always_kind` added before anyone sees it. A relative's turn never.
    fn with_always(&self, sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
        let Some(shared) = &self.config else { return event };
        if event.data.contains_key("bip_always") || self.someone_elses(&sub.run_id) {
            return event;
        }
        let config = shared.get();
        let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
        let Some((label, key)) = crate::approval_level::hermes_always(&config, &agent, &event.data) else { return event };
        let mut data = event.data;
        data.insert("bip_always".into(), json!(label));
        data.insert("bip_always_kind".into(), json!(key));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// A run that ended on Hermes' own « ⚠️ No reply: … /model » explanation, alone or after a fragment of an answer,
    /// ends on our plain line instead (no_reply.rs), marked `bip_no_reply` for the apps.
    fn no_reply_in_plain_words(&self, sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
        let output = as_str(event.data.get("output")).unwrap_or_default();
        let no_reply =
            crate::no_reply::is_hermes_no_reply(&output) || crate::no_reply::words_before(&output).is_some();
        // a thinking turn without an answer: the agent's next turns answer without thinking (turn_thinking.rs)
        crate::turn_thinking::run_ended(&sub.agent.name, &sub.run_id, !no_reply && !output.trim().is_empty());
        if !no_reply {
            return event;
        }
        let lang = match &self.config {
            Some(shared) => {
                let config = shared.get();
                let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
                crate::handback::language_of(&config, &agent)
            }
            None => "en".to_string(),
        };
        let name = if sub.agent.display_name.trim().is_empty() { &sub.agent.name } else { &sub.agent.display_name };
        tracing::warn!(agent = %sub.agent.name, run_id = %sub.run_id, "turn ended without an answer: plain line shown");
        let told = crate::no_reply::in_plain_words(&output, name, &lang).unwrap_or_else(|| crate::no_reply::plain(name, &lang));
        let mut data = event.data;
        data.insert("output".into(), json!(told));
        data.insert("bip_no_reply".into(), json!(true));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// A turn Hermes ended because the model's reply was cut mid tool-call (reply_cut.rs): the person reads one plain
    /// line (the model's limit, not BipAgents), marked `bip_reply_cut`, and the agent's next turn in this conversation
    /// is told what to do (write in parts, or sum up).
    fn reply_cut_in_plain_words(&self, sub: &RunSubscription, st: &SubscriptionState, event: SSEEvent) -> SSEEvent {
        let error = as_str(event.data.get("error")).unwrap_or_default();
        let Some(cut) = crate::reply_cut::kind(&error) else { return event };
        let lang = match &self.config {
            Some(shared) => {
                let config = shared.get();
                let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
                crate::handback::language_of(&config, &agent)
            }
            None => "en".to_string(),
        };
        if let Some(session) = st.session_id.as_deref().filter(|s| !s.is_empty()) {
            crate::reply_cut::noted(&sub.agent.name, session, cut);
        }
        tracing::warn!(agent = %sub.agent.name, run_id = %sub.run_id, cut = ?cut, "reply cut mid tool-call: plain line shown");
        let name = if sub.agent.display_name.trim().is_empty() { &sub.agent.name } else { &sub.agent.display_name };
        let mut data = event.data;
        data.insert("error".into(), json!(crate::reply_cut::person_line(cut, name, &lang)));
        data.insert("bip_reply_cut".into(), json!(true));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    /// A turn that failed on a model chosen for this agent at home (model_choice.rs): one plain sentence (check the
    /// model, or go back to the install's default), never the provider's words nor the plan's credit notice.
    fn account_failure_in_plain_words(&self, sub: &RunSubscription, event: SSEEvent) -> SSEEvent {
        // a reply cut by the model's length limit is no problem of the model chosen
        if event.data.contains_key("bip_reply_cut") {
            return event;
        }
        let Some(shared) = &self.config else { return event };
        let config = shared.get();
        let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
        let Some(line) = crate::model_choice::failure_line(&config, &agent) else { return event };
        let mut data = event.data;
        data.insert("error".into(), json!(line));
        data.insert("own_model".into(), json!(true));
        let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
        SSEEvent::new(event.r#type, data, raw)
    }

    fn on_approval(&self, inner: &mut HubInner, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        let data = &event.data;
        let request_id = as_str(data.get("request_id"));
        let command = as_str(data.get("command"));
        // what the notification says: the plain sentence when there is one (previews on), else the command
        let shown = as_str(data.get("bip_summary")).or_else(|| command.clone());
        let dedupe = match request_id.as_deref().filter(|r| !r.is_empty()) {
            Some(r) => r.to_string(),
            None => sha16(command.as_deref().filter(|c| !c.is_empty()).unwrap_or(&event.raw)),
        };
        let choices = match data.get("choices") {
            Some(Value::Array(c)) => c.clone(),
            _ => vec![json!("once"), json!("deny")],
        };
        let mut item = Map::new();
        item.insert("agent".into(), json!(sub.agent.name));
        item.insert("run_id".into(), json!(sub.run_id));
        item.insert("request_id".into(), json!(request_id));
        item.insert("command".into(), json!(command));
        item.insert("description".into(), json!(as_str(data.get("description"))));
        item.insert("choices".into(), Value::Array(choices.clone()));
        item.insert("created_at".into(), json!(crate::py::time_time()));
        item.insert("session_id".into(), json!(st.session_id));
        if let Some(room) = &st.room_id {
            item.insert("room_id".into(), json!(room));
        }
        // the prompt's generation: its answer names it (hermes_prompts.rs)
        if let Some(generation) = crate::hermes_prompts::generation(data) {
            item.insert("execution_generation".into(), json!(generation));
        }
        // the apps' own reading needs Hermes' pattern key; the plain words (in_plain_words) go with it
        for key in ["pattern_key", "bip_summary", "bip_detail", "bip_always", "bip_always_kind"] {
            if let Some(value) = data.get(key).filter(|v| v.is_string()) {
                item.insert(key.into(), value.clone());
            }
        }
        // kept for `bipbridge approvals`: what the agents ask, answered or not (approvals_log.rs)
        if let Some(shared) = &self.config {
            if let Err(err) = crate::approvals_log::asked(&shared.get(), &sub.agent.name, &item) {
                tracing::warn!(agent = %sub.agent.name, error = %err, "approval request not kept");
            }
        }
        // not critical, and the person validates only what is: answered « once » at once, nothing pushed, a line in the
        // Boîte (approval_level.rs); kept waiting until Hermes settles it, so a failed answer still shows in the Boîte
        let elsewhere = self.someone_elses(&sub.run_id);
        // exactly what the person approved on a held card (held_approvals.rs): answered « once », used up
        let granted = self.config.as_ref().map(|c| c.get()).filter(|_| !elsewhere).and_then(|config| {
            let grant = crate::held_approvals::take_grant(&config, &sub.agent.name, command.as_deref().unwrap_or(""))?;
            Some((config, crate::approval_level::classify_hermes(data), Auto::Held(grant.card)))
        });
        let auto = granted.or_else(|| {
            self.config.as_ref().map(|c| c.get()).filter(|_| !elsewhere).and_then(|config| {
                crate::approval_level::auto_hermes(&config, &sub.agent.name, &sub.run_id, data)
                    .map(|(kind, always)| (config, kind, if always { Auto::Always } else { Auto::Level }))
            })
        });
        let answering = auto.map(|(config, kind, why)| (config, kind, why, item.clone()));
        let held = item.clone();
        inner.pending.insert((sub.agent.name.clone(), sub.run_id.clone(), dedupe.clone()), item);
        if let Some((config, kind, why, mut fields)) = answering {
            let (hermes, agent, run_id) = (self.hermes.clone(), sub.agent.clone(), sub.run_id.clone());
            let (summary, session) = (shown.clone().unwrap_or_default(), st.session_id.clone());
            tracing::info!(agent = %agent.name, run_id = %run_id, kind, why = ?why, "approval answered by the bridge");
            self.spawn(async move {
                match hermes.answer_prompt(&agent, &run_id, "approval", "once", request_id.as_deref()).await {
                    Ok((status, _)) if (200..300).contains(&status) => {
                        fields.insert("choice".into(), json!("once"));
                        fields.insert("via".into(), json!(if matches!(why, Auto::Held(_)) { "held" } else { "auto" }));
                        fields.remove("created_at");
                        if let Err(err) = crate::approvals_log::record(&config, &agent.name, &fields) {
                            tracing::warn!(agent = %agent.name, error = %err, "approval decision not kept");
                        }
                        if !matches!(why, Auto::Held(_)) {
                            let always = why == Auto::Always;
                            crate::approval_level::record_hermes(&config, &agent.name, kind, &summary, &run_id, session.as_deref(), always);
                        }
                    }
                    other => {
                        tracing::warn!(agent = %agent.name, run_id = %run_id, answer = ?other.map(|r| r.0), "approval not answered by the level: left to the person");
                    }
                }
            });
            return;
        }
        // nobody can answer now (a routine at night…): kept by the bridge as its own card, the turn ends now
        if self.nobody_there(sub, st) && !st.pushed.contains(&dedupe) {
            let request = crate::held_approvals::Unattended::Approval {
                agent: sub.agent.clone(),
                run_id: sub.run_id.clone(),
                session: st.session_id.clone(),
                item: held,
            };
            if self.unattended.get().is_some_and(|tx| tx.send(request).is_ok()) {
                st.pushed.insert(dedupe);
                tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, "approval in a turn nobody follows: held");
                return;
            }
        }
        let followers = st.followers() + usize::from(Self::followed_through(inner, sub, st));
        let push = match self.push.clone() {
            Some(push) if st.watched && followers == 0 && !st.pushed.contains(&dedupe) && !elsewhere => push,
            _ => {
                tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, watched = st.watched, followers = followers, "approval seen, no push");
                return;
            }
        };
        st.pushed.insert(dedupe);
        tracing::info!(agent = %sub.agent.name, run_id = %sub.run_id, "approval push");
        let (agent, run_id, session, room) =
            (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone(), st.room_id.clone());
        self.spawn(async move {
            push.notify_approval_values(
                &agent,
                &run_id,
                request_id.as_deref(),
                Some(&choices),
                shown.as_deref(),
                session.as_deref(),
                room.as_deref(),
            )
            .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(kind: &str, text: &str) -> SSEEvent {
        let data = json!({"event": kind, "delta": text});
        SSEEvent::new(kind, data.as_object().cloned().unwrap(), crate::py::dumps(&data))
    }

    #[test]
    fn backlog_merges_consecutive_deltas() {
        let mut backlog = VecDeque::new();
        append_to_backlog(&mut backlog, &delta("message.delta", "Bon"));
        append_to_backlog(&mut backlog, &delta("message.delta", "jour é"));
        append_to_backlog(&mut backlog, &SSEEvent::new("tool.started", Map::new(), "{}"));
        append_to_backlog(&mut backlog, &delta("message.delta", "x"));
        append_to_backlog(&mut backlog, &delta("assistant.delta", "y"));
        assert_eq!(backlog.len(), 4);
        assert_eq!(backlog[0].data["delta"], "Bonjour é");
        assert_eq!(backlog[0].raw, "{\"event\": \"message.delta\", \"delta\": \"Bonjour é\"}");
        let mut full = VecDeque::new();
        for i in 0..BACKLOG_EVENTS + 3 {
            append_to_backlog(&mut full, &SSEEvent::new(format!("e{i}"), Map::new(), ""));
        }
        assert_eq!(full.len(), BACKLOG_EVENTS);
        assert_eq!(full[0].r#type, "e3");
    }

    /// Hermes' failed-turn line as a run's output or error: `stopped` says which, for the apps' own words.
    #[test]
    fn a_failed_turns_end_says_which_it_is() {
        let end = |kind: &str, data: Value| with_stopped(SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}"));
        let partial = "This turn did not complete. Some actions may already have run; verify their effects before resending.";
        assert_eq!(end("run.completed", json!({"output": format!("{partial}\n")})).data["stopped"], "partial");
        let nothing = "Your request was not processed. Send it again if you still want me to carry it out.";
        assert_eq!(end("run.failed", json!({"error": nothing})).data["stopped"], "not_processed");
        let reply = end("run.completed", json!({"output": "Voilà."}));
        assert!(!reply.data.contains_key("stopped"));
        assert_eq!(reply.raw, "{}", "an ordinary end passes as it came");
    }

    #[test]
    fn a_runs_start_is_when_the_hub_began_following_it() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent, "run_1"));
        let since = {
            let mut st = lock(&sub.state);
            st.session_id = Some("api_bot".into());
            st.followed_since
        };
        let key = sub.key();
        lock(&hub.inner).subs.insert(key, sub);
        assert_eq!(hub.session_run_since("vie", "api_bot"), Some(("run_1".into(), since)));
        assert_eq!(hub.session_run("vie", "api_bot").as_deref(), Some("run_1"));
        assert_eq!(hub.session_run_since("vie", "api_other"), None);
    }

    /// turn_restart.rs: a person's turn that did nothing yet is to start again with new words; one that acted, a
    /// routine's, or one already restarting is not; a restarted run is no longer its conversation's run.
    #[test]
    fn new_words_restart_a_turn_only_before_it_acted() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let add = |run: &str, session: &str, change: &dyn Fn(&mut SubscriptionState)| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), run));
            {
                let mut st = lock(&sub.state);
                st.session_id = Some(session.into());
                change(&mut st);
            }
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let plan = || crate::turn_restart::Plan { said: "Seulement les publiques".into(), ..Default::default() };
        let thinking = add("r1", "api_1", &|_| {});
        assert!(hub.interrupt("vie", "r1", plan()));
        assert!(thinking.restarting());
        assert!(!hub.interrupt("vie", "r1", plan()), "already to start again: the next words wait");
        assert_eq!(hub.withdraw_restart("vie", "r1").map(|p| p.said), Some("Seulement les publiques".into()));
        assert!(!thinking.restarting());
        lock(&thinking.state).acted = true;
        assert!(!hub.interrupt("vie", "r1", plan()), "it acted: steered at its next step");
        add("r2", "api_2", &|st| st.unattended = true);
        assert!(!hub.interrupt("vie", "r2", plan()), "a routine's turn is never cut");
        add("r3", "api_3", &|st| st.room_id = Some("room".into()));
        assert!(!hub.interrupt("vie", "r3", plan()));
        assert!(!hub.interrupt("vie", "unknown", plan()));
        // r5 restarted as r4: r4 is the conversation's run now
        let cut = add("r5", "api_5", &|_| {});
        assert_eq!(hub.session_run("vie", "api_5").as_deref(), Some("r5"));
        add("r4", "api_5", &|_| {});
        lock(&cut.state).restarted_as = Some("r4".into());
        assert_eq!(hub.session_run("vie", "api_5").as_deref(), Some("r4"));
    }

    /// What makes a turn « acted » as its events come (dispatch).
    #[tokio::test]
    async fn a_call_that_changes_things_or_a_card_marks_the_turn_acted() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let event = |kind: &str, data: Value| SSEEvent::new(kind, data.as_object().cloned().unwrap_or_default(), "");
        for (events, acted) in [
            (vec![event("reasoning.delta", json!({"text": "hmm"})), event("message.delta", json!({"delta": "Voici"}))], false),
            (vec![event("tool.started", json!({"tool": "web_search"})), event("tool.completed", json!({"tool": "web_search"}))], false),
            (vec![event("tool.started", json!({"tool": "mcp__bipagents__mail_send"}))], true),
            (vec![event("approval.request", json!({"command": "ls"}))], true),
        ] {
            let sub = Arc::new(RunSubscription::new(agent.clone(), "r1"));
            lock(&sub.state).session_id = Some("api_bot".into());
            for e in events {
                hub.dispatch(&sub, e);
            }
            assert_eq!(sub.acted(), acted);
        }
    }

    /// A turn whose end `go_on` holds while its continuation runs, and the continuation, linked as `go_on` links them.
    fn turn_and_continuation(hub: &RunHub) -> (Arc<RunSubscription>, Arc<RunSubscription>) {
        let vie = || AgentConfig { name: "vie".into(), ..Default::default() };
        let turn = Arc::new(RunSubscription::new(vie(), "run_vie"));
        {
            let mut st = lock(&turn.state);
            st.session_id = Some("api_c".into());
            st.watched = true;
            st.ever_followed = true;
            st.followed_since -= 60.0;
            st.continued_by = Some("run_next".into());
        }
        let next = Arc::new(RunSubscription::new(vie(), "run_next"));
        {
            let mut st = lock(&next.state);
            st.kept = true;
            st.watched = true;
            st.ever_followed = true;
            st.end_push = false;
            st.session_id = Some("api_c".into());
            st.go_on_of = Some("run_vie".into());
        }
        lock(&hub.inner).subs.insert(turn.key(), turn.clone());
        lock(&hub.inner).subs.insert(next.key(), next.clone());
        (turn, next)
    }

    /// Review 13/11 of go_on: the apps' Stop (POST /v1/threads/{id}/stop), the words steered (threads/send.rs) and the
    /// queue take `session_run`; with the turn and its continuation both going on, the hub's map order picked either.
    #[test]
    fn during_the_continuation_the_conversations_turn_is_the_continuation() {
        let mut wrong = 0;
        for _ in 0..40 {
            let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
            let (turn, _) = turn_and_continuation(&hub);
            let started = lock(&turn.state).followed_since;
            match hub.session_run_since("vie", "api_c") {
                Some((run, since)) if run == "run_next" => assert_eq!(since, started, "the turn's start"),
                _ => wrong += 1,
            }
        }
        assert_eq!(wrong, 0, "in {wrong} of 40 hubs the conversation's turn was the one Hermes had already ended");
    }

    /// Review of the merge with steer-interrupt (F3): go_on gave up right after the continuation's start (the person
    /// stepped in: `continued_by` cleared, the continuation `dropped` and being stopped, the turn's restart to come).
    /// Until Hermes' end of the dropped run arrives, the conversation's run stays the turn, and new words never restart
    /// the dropped run.
    #[test]
    fn a_dropped_continuation_is_never_the_conversations_run() {
        let mut wrong = 0;
        for _ in 0..40 {
            let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
            let (turn, next) = turn_and_continuation(&hub);
            lock(&turn.state).continued_by = None;
            lock(&next.state).dropped = true;
            if hub.session_run("vie", "api_c").as_deref() != Some("run_vie") {
                wrong += 1;
            }
        }
        assert_eq!(wrong, 0, "in {wrong} of 40 hubs the conversation's run was the dropped continuation");
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let (_, next) = turn_and_continuation(&hub);
        lock(&next.state).dropped = true;
        let plan = crate::turn_restart::Plan { said: "plutôt en bio".into(), ..Default::default() };
        assert!(!hub.interrupt("vie", "run_next", plan), "a dropped continuation is never restarted");
    }

    /// The continuation is told to start_task: the task's conversation (jobs.rs `asking_session`) is its own, even
    /// while another conversation of the agent was followed later.
    #[test]
    fn a_task_the_continuation_starts_belongs_to_its_conversation() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let (_, next) = turn_and_continuation(&hub);
        let now = crate::py::time_time();
        {
            let mut st = lock(&next.state);
            st.followed_since = now - 20.0;
            st.recent_tools.push_back("mcp_bipagents_start_task".into());
        }
        let other =
            Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_other"));
        {
            let mut st = lock(&other.state);
            st.session_id = Some("api_other".into());
            st.followed_since = now - 30.0;
        }
        lock(&hub.inner).subs.insert(other.key(), other);
        assert_eq!(hub.asking_session("vie").as_deref(), Some("api_c"));
    }

    /// Reviews 13/11: after a restart the hub follows again every watched run (a continuation the iOS app adopted, a
    /// routine's turn) as if a person started it: never continued. A household's message (peers.rs) is a guest's
    /// turn: never continued either.
    #[test]
    fn a_run_followed_again_after_a_restart_or_a_guests_is_never_continued() {
        let fits = |set: fn(&mut SubscriptionState)| {
            let mut st = SubscriptionState { watched: true, ..Default::default() };
            set(&mut st);
            go_on_fits(&st)
        };
        assert!(fits(|_| {}));
        assert!(!fits(|st| st.guest = true));
        assert!(!fits(|st| st.resumed = true));
        // a turn stopped for the person's new words and started again with them (turn_restart.rs): they were heard,
        // like words steered in, so its end is never continued
        assert!(!fits(|st| st.restarted_as = Some("r2".into())));
        // nor the turn that took over (its words answered: the turn it restarted is their turn)
        assert!(!fits(|st| st.restart_of = Some("r1".into())));
    }

    #[tokio::test]
    async fn a_later_delivery_after_a_followed_reply_is_pushed() {
        let agent = AgentConfig {
            name: "vie".into(),
            display_name: "Vie".into(),
            hermes_url: "http://h".into(),
            hermes_key: "k".repeat(40),
            ..Default::default()
        };
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let sub = Arc::new(RunSubscription::new(agent, "run_1"));
        {
            let mut st = lock(&sub.state);
            st.session_id = Some("api_bot".into());
            st.finished = true;
            st.finished_at = Some(crate::py::time_time() - 120.0);
            st.output = Some("C'est lancé : un sous-agent prépare le podcast, je te le livre dès qu'il a fini.".into());
        }
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        // The reply the app followed live: no push.
        assert!(hub.followed_recently(
            "vie",
            "api_bot",
            Some("C'est lancé : un sous-agent prépare le podcast, je te le livre dès qu'il a fini."),
            600.0
        ));
        // Two minutes later Vie delivers the sub-task's result on its own: a new reply, pushed.
        assert!(!hub.followed_recently(
            "vie",
            "api_bot",
            Some("Voilà ton podcast ! MEDIA:/home/hermes/.hermes/media/podcast.mp3"),
            600.0
        ));
        // Another conversation is never concerned.
        assert!(!hub.followed_recently("vie", "api_other", Some("C'est lancé"), 600.0));
        // A run still going: its replies are on screen.
        lock(&sub.state).finished = false;
        assert!(hub.followed_recently("vie", "api_bot", Some("anything"), 600.0));
        // Output unknown: only right after the run.
        {
            let mut st = lock(&sub.state);
            st.finished = true;
            st.output = None;
            st.finished_at = Some(crate::py::time_time() - 30.0);
        }
        assert!(hub.followed_recently("vie", "api_bot", Some("x"), 600.0));
        lock(&sub.state).finished_at = Some(crate::py::time_time() - 90.0);
        assert!(!hub.followed_recently("vie", "api_bot", Some("x"), 600.0));
    }

    #[tokio::test]
    async fn hermes_task_list_reaches_the_apps_as_the_cards_list() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent, "run_steps"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        let todos = json!({"event": "todo.updated", "run_id": "run_steps", "seq": 4, "revision": 2,
                           "todos": [{"id": "1", "content": "Lire", "status": "completed"},
                                     {"id": "2", "content": "Trier", "status": "in_progress"}]});
        hub.dispatch(&sub, SSEEvent::new("todo.updated", todos.as_object().cloned().unwrap(), "{}"));
        let st = lock(&sub.state);
        let kinds: Vec<&str> = st.backlog.iter().map(|e| e.r#type.as_str()).collect();
        assert_eq!(kinds, ["steps.updated"], "Hermes' own event is not passed on");
        let event = &st.backlog[0];
        assert_eq!(event.data["steps"]["items"][1], json!({"text": "Trier", "status": "in_progress"}));
        assert_eq!((event.data["seq"].as_i64(), event.data["run_id"].as_str()), (Some(4), Some("run_steps")));
        let raw: Value = serde_json::from_str(&event.raw).unwrap();
        assert_eq!(raw["event"], "steps.updated");
        assert_eq!(raw["steps"]["done"], 1);
        assert_eq!(st.steps.as_ref().unwrap()["total"], 2, "kept for a background task's card");
    }

    /// Le coffre rassure, review R3 (17/11): whose call of the vault it is — the turns of the agent inside a call of
    /// that very tool now (its last `tool.started`, not completed), by Hermes' own names, never « the turn that called a
    /// tool last ».
    #[tokio::test]
    async fn the_turns_inside_a_call_of_a_tool_are_found_by_its_hermes_names() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let event = |kind: &str, data: Value| SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}");
        let turn = |agent: &str, run: &str, session: &str| {
            let sub = Arc::new(RunSubscription::new(AgentConfig { name: agent.into(), ..Default::default() }, run));
            lock(&sub.state).session_id = Some(session.into());
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let started = |sub: &Arc<RunSubscription>, tool: &str, id: &str| {
            hub.dispatch(sub, event("tool.started", json!({"tool": tool, "tool_call_id": id})));
        };
        let vault = |name: &str| crate::vault_line::names_vault_tool(name, crate::vault_logins::LOGIN);
        let login = turn("vie", "run_login", "api_login");
        started(&login, "mcp_bipagents_vault_vault_login", "a");
        assert_eq!(hub.runs_in_call("vie", vault), [("run_login".to_string(), Some("api_login".to_string()))]);
        // another conversation's turn of Vie, a tool used after it: still the login's alone
        let other = turn("vie", "run_other", "api_other");
        started(&other, "web_search", "b");
        // another agent logging in, a connector's tool of that name: never Vie's
        started(&turn("paul", "run_paul", "api_paul"), "mcp_bipagents_vault_vault_login", "c");
        started(&turn("vie", "run_conn", "api_conn"), "mcp__bipconn_x__vault_login", "d");
        assert_eq!(hub.runs_in_call("vie", vault), [("run_login".to_string(), Some("api_login".to_string()))]);
        // a relative's conversation with the shared agent logging in too: two turns inside it
        let lea = turn("vie", "run_lea", "api_lea");
        started(&lea, "mcp__bipagents_vault__vault_login", "e");
        let mut both: Vec<String> = hub.runs_in_call("vie", vault).into_iter().map(|(run, _)| run).collect();
        both.sort();
        assert_eq!(both, ["run_lea", "run_login"]);
        // its call completed; the login's turn ended (stopped) before its fill was recorded: none left
        hub.dispatch(&lea, event("tool.completed", json!({"tool": "mcp__bipagents_vault__vault_login", "tool_call_id": "e"})));
        lock(&login.state).finished = true;
        assert!(hub.runs_in_call("vie", vault).is_empty());
    }

    #[tokio::test]
    async fn a_run_without_its_own_list_gets_the_automatic_one_until_the_agent_writes_one() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent, "run_auto"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        let event = |kind: &str, data: Value| SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}");
        let steps_events = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| e.r#type == "steps.updated").cloned().collect::<Vec<_>>()
        };
        hub.dispatch(&sub, event("tool.started", json!({"tool": "mcp__bipagents__mail_search", "tool_call_id": "a"})));
        hub.dispatch(&sub, event("tool.completed", json!({"tool": "mcp__bipagents__mail_search", "tool_call_id": "a"})));
        assert!(steps_events(&sub).is_empty(), "one step: no card");
        hub.dispatch(&sub, event("tool.started", json!({"tool": "mcp__stealth__navigate", "tool_call_id": "b",
                                                        "preview": "https://www.sncf-connect.com/billets"})));
        hub.dispatch(&sub, event("tool.completed", json!({"tool": "mcp__stealth__navigate", "tool_call_id": "b"})));
        // a calculation is the agent's own workings: still two useful steps, no card
        hub.dispatch(&sub, event("tool.started", json!({"tool": "execute_code", "tool_call_id": "x"})));
        hub.dispatch(&sub, event("tool.completed", json!({"tool": "execute_code", "tool_call_id": "x"})));
        assert!(steps_events(&sub).is_empty(), "two useful steps: no card");
        // the 3rd useful step starts: the card
        hub.dispatch(&sub, event("tool.started", json!({"tool": "mcp__bipagents__create_page", "tool_call_id": "p"})));
        let got = steps_events(&sub);
        assert_eq!(got.len(), 1);
        let items = &got[0].data["steps"]["items"];
        assert_eq!(items[0], json!({"text": "Chercher dans tes mails", "status": "done"}));
        assert_eq!(items[1], json!({"text": "Ouvrir le site sncf-connect.com", "status": "done"}));
        assert_eq!(items[2], json!({"text": "Préparer une page", "status": "in_progress"}));
        let raw: Value = serde_json::from_str(&got[0].raw).unwrap();
        assert_eq!(raw["event"], "steps.updated");
        hub.dispatch(&sub, event("tool.completed", json!({"tool": "mcp__bipagents__create_page", "tool_call_id": "p"})));
        assert_eq!(steps_events(&sub).len(), 2);
        assert_eq!(steps_events(&sub)[1].data["steps"]["done"], 3);
        // the agent's own list: it wins for the rest of the run
        let todos = json!({"todos": [{"id": "1", "content": "Mon plan", "status": "in_progress"}], "revision": 1});
        hub.dispatch(&sub, event("todo.updated", todos));
        hub.dispatch(&sub, event("tool.started", json!({"tool": "web_search", "tool_call_id": "c"})));
        let last = steps_events(&sub);
        assert_eq!(last.len(), 3);
        assert_eq!(last[2].data["steps"]["items"][0]["text"], "Mon plan");
        assert_eq!(lock(&sub.state).steps.as_ref().unwrap()["items"][0]["text"], "Mon plan");
    }

    #[tokio::test]
    async fn the_end_of_the_last_run_that_used_the_browser_is_told_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("browsers");
        std::fs::create_dir_all(root.join("vie")).unwrap();
        let data = json!({"bridge_key": "k".repeat(40), "browser": {"root": root.to_string_lossy()}});
        let config = SharedConfig::new(crate::config::parse_config(&data, None).unwrap());
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None).with_config(config);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let event = |kind: &str, data: Value| SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}");
        let ended = root.join("vie").join("ended");
        let (chat, job, other) = (run("run_chat"), run("run_job"), run("run_other"));
        hub.dispatch(&chat, event("tool.started", json!({"tool": "mcp_stealth_navigate"})));
        hub.dispatch(&job, event("tool.started", json!({"tool": "mcp_stealth_click_element"})));
        hub.dispatch(&other, event("tool.started", json!({"tool": "web_search"})));
        assert!(hub.any_running()); // the seat agent's `busy` (mark_busy_forever)
        // The conversation's run ends while a background task still uses the browser: it stays.
        hub.dispatch(&chat, event("run.completed", json!({"output": "Voilà."})));
        assert!(!ended.exists());
        // A run that never used it ends: nothing said either.
        hub.dispatch(&other, event("run.completed", json!({"output": "Ok."})));
        assert!(!ended.exists());
        // The task fails: the browser's work is over (bip-browser-mcp stops it 3 minutes later unless used again).
        hub.dispatch(&job, event("run.failed", json!({"error": "x"})));
        assert!(ended.exists());
        assert!(!hub.any_running());
        // An agent without a browser set up gets no file.
        let lone = Arc::new(RunSubscription::new(AgentConfig { name: "lea".into(), ..Default::default() }, "r"));
        hub.dispatch(&lone, event("tool.started", json!({"tool": "browser_navigate"})));
        hub.dispatch(&lone, event("run.completed", json!({})));
        assert!(!root.join("lea").exists());
    }

    /// What the agent does now (turn_status.rs): a `turn.status` after the event that
    /// changed it, in the run's backlog (an app that comes back gets it), only with `[status] enabled`.
    #[tokio::test]
    async fn what_the_agent_does_reaches_the_runs_stream_only_when_on() {
        let event = |kind: &str, data: Value| SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}");
        let statuses = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| e.r#type == crate::turn_status::EVENT).map(|e| e.data.clone())
                .collect::<Vec<_>>()
        };
        for enabled in [false, true] {
            let data = json!({"bridge_key": "k".repeat(40), "status": {"enabled": enabled, "page_slow": 1}});
            let config = SharedConfig::new(crate::config::parse_config(&data, None).unwrap());
            let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None).with_config(config);
            let sub = Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_s"));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            hub.dispatch(&sub, event("tool.started", json!({"tool": "mcp_stealth_navigate", "tool_call_id": "a",
                                                            "preview": "https://www.sncf-connect.com/home?x=1"})));
            hub.dispatch(&sub, event("message.delta", json!({"delta": "…"})));
            if !enabled {
                assert!(statuses(&sub).is_empty(), "off by default");
                assert!(!hub.status_tick(&sub), "off: no clock (review B4)");
                continue;
            }
            let first = statuses(&sub);
            assert_eq!(first.len(), 1, "{first:?}");
            assert_eq!((first[0]["state"].as_str(), first[0]["site"].as_str()), (Some("tool"), Some("sncf-connect.com")));
            assert_eq!(first[0]["run_id"], "run_s");
            let kinds: Vec<String> = lock(&sub.state).backlog.iter().map(|e| e.r#type.clone()).collect();
            assert_eq!(&kinds[..2], ["tool.started", crate::turn_status::EVENT], "after the event that changed it");
            // a wait grown abnormal is said by the hub's clock, once
            tokio::time::sleep(Duration::from_millis(1100)).await;
            assert!(hub.status_tick(&sub));
            assert!(hub.status_tick(&sub));
            let all = statuses(&sub);
            assert_eq!(all.len(), 2, "{all:?}");
            assert_eq!(all[1]["slow"], true);
            // the run's end ends them
            hub.dispatch(&sub, event("run.completed", json!({"output": "Voilà."})));
            tokio::time::sleep(Duration::from_millis(1100)).await;
            hub.status_tick(&sub);
            assert_eq!(statuses(&sub).len(), 2);
        }
    }

    /// Review (17/11, blocking): `[status]` off (the default), nothing new runs. Today each followed run gets a clock
    /// waking every 2 s for its whole life (`upstream`'s ticker), taking the run's lock to say nothing. Off: no clock
    /// (not spawned, and `status_tick` says it is over at once).
    #[tokio::test]
    async fn review_statuses_off_no_clock_ticks_for_a_run() {
        let data = json!({"bridge_key": "k".repeat(40)});
        let config = SharedConfig::new(crate::config::parse_config(&data, None).unwrap());
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None).with_config(config);
        let sub = Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_off"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        assert!(!hub.status_tick(&sub), "off by default: no clock every 2 s for each run");
    }

    /// Review (17/11, blocking): R3 (review-battery-settings, 16/11) again. `listener_events_with` used to write every
    /// event, so its keepalive per `recv` was « 25 s after the last bytes ». It now leaves `turn.status` out for an app
    /// that doesn't ask (an app from before, the door's relay): each status left out starts the 25 s again, writing
    /// nothing. A status 20 s after the last bytes (the model slow at 20 s) puts the next bytes at 45 s: Android reads
    /// with 45 s, the iPhone 60 s.
    #[tokio::test]
    async fn review_an_app_that_reads_no_status_keeps_alive_while_statuses_are_left_out() {
        use http_body_util::BodyExt;
        let hub = Arc::new(RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None));
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent.clone(), "run_k"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        let listener = hub.subscribe(&agent, "run_k", "app", false);
        let mut body = crate::app::listener_events_with(listener, None, false).into_body();
        let feeder = {
            let sub = sub.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                    let data = json!({"event": crate::turn_status::EVENT, "state": "model", "slow": true});
                    let event = SSEEvent::new(crate::turn_status::EVENT, data.as_object().cloned().unwrap(), "{}");
                    lock(&sub.state).send_all(Some(&event));
                }
            })
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(28);
        let mut seen = String::new();
        let alive = loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(left, body.frame()).await {
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        seen.push_str(&String::from_utf8_lossy(data));
                    }
                    if seen.contains(": keepalive") {
                        break true;
                    }
                }
                _ => break false,
            }
        };
        feeder.abort();
        assert!(!seen.contains("turn.status"), "a status for an app that didn't ask: {seen:?}");
        assert!(alive, "no keepalive in 28 s (a status left out at 20 s): {seen:?}");
    }

    /// The brake's two thirds told to the person too (« Je tourne en rond », 16/11): « Continuer » raises this turn's
    /// limits once, « Arrêter » takes the brake's own stop (its plain line, its rescue turn) at once.
    #[tokio::test]
    async fn the_brakes_two_thirds_are_told_to_the_person_who_may_go_on_once_or_stop() {
        let data = json!({"bridge_key": "k".repeat(40), "status": {"enabled": true}});
        let config = SharedConfig::new(crate::config::parse_config(&data, None).unwrap());
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None).with_config(config);
        let agent = AgentConfig { name: "daily".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let add = |n: &str| {
            let data = json!({"tool": "mcp__bipagents__calendar_add", "tool_call_id": n, "preview": "{\"title\": \"Dentiste\"}"});
            SSEEvent::new("tool.started", data.as_object().cloned().unwrap(), "{}")
        };
        let looping = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| e.r#type == crate::turn_status::EVENT && e.data["state"] == "looping").count()
        };
        assert_eq!(hub.brake_action("daily", "nope", "continue"), Err((404, "run_gone")));
        let (going, stopping) = (run("run_go"), run("run_stop"));
        hub.dispatch(&going, add("a"));
        assert_eq!(hub.brake_action("daily", "run_go", "continue"), Err((409, "not_looping")));
        assert_eq!(hub.brake_action("daily", "run_go", "jump"), Err((400, "unknown_action")));
        hub.dispatch(&going, add("b"));
        assert_eq!(looping(&going), 1, "told at the second identical write (stopped at the third)");
        let said = lock(&going.state).backlog.iter().rev().find(|e| e.r#type == crate::turn_status::EVENT).unwrap().data.clone();
        assert_eq!(said["actions"], json!(["continue", "stop", "help"]));
        assert!(hub.brake_action("daily", "run_go", "continue").is_ok());
        assert_eq!(hub.brake_action("daily", "run_go", "continue"), Err((409, "already_continued")));
        hub.dispatch(&going, add("c"));
        assert!(!lock(&going.state).braked, "the third goes on: its limits raised once");
        hub.dispatch(&stopping, add("a"));
        hub.dispatch(&stopping, add("b"));
        assert!(hub.brake_action("daily", "run_stop", "stop").is_ok());
        assert!(lock(&stopping.state).braked);
        assert!(lock(&stopping.state).backlog.iter().any(|e| e.r#type == NOTICE && e.data["code"] == "loop_stopped"));
        assert_eq!(hub.brake_action("daily", "run_stop", "continue"), Err((409, "not_looping")), "stopped already");
    }

    #[tokio::test]
    async fn a_turn_going_round_in_circles_is_stopped_and_said() {
        // 2026-10-27, token lab: calendar_add and calendar_list in turns, 262 times in one turn
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "daily".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let started = |tool: &str| SSEEvent::new("tool.started", json!({"tool": tool}).as_object().cloned().unwrap(), "{}");
        let notices = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| e.r#type == NOTICE).map(|e| e.data.clone()).collect::<Vec<_>>()
        };
        let (chat, task) = (run("run_chat"), run("run_task"));
        hub.mark_background("daily", "run_task");
        assert!(lock(&task.state).background && !lock(&chat.state).background);
        for i in 0..30 {
            let tool = if i % 2 == 0 { "mcp__bipagents__calendar_add" } else { "mcp__bipagents__calendar_list" };
            hub.dispatch(&chat, started(tool));
            hub.dispatch(&task, started(tool));
        }
        let said = notices(&chat);
        assert_eq!(said.len(), 1, "told once");
        assert_eq!(said[0]["code"], json!("loop_stopped"));
        assert_eq!(said[0]["text"], json!("I stopped: I was going round in circles with the calendar. Tell me if you want me to try again."));
        let st = lock(&chat.state);
        assert!(st.braked && st.rescued && st.turn.total == 21);
        drop(st);
        // the background task's run goes further (40 of one tool)
        assert!(notices(&task).is_empty() && !lock(&task.state).braked);
    }

    /// Release bench of 4/11: Hermes' run events carry no arguments for the bridge's tools (`"preview": ""`), so the
    /// same page asked for 10 times in an after-call turn was never « the same call »; the tool tells the hub instead.
    #[tokio::test]
    async fn the_same_write_asked_again_stops_the_run_at_its_same_call() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "daily".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let started = SSEEvent::new("tool.started", json!({"tool": "mcp__bipagents__create_page", "preview": ""})
            .as_object().cloned().unwrap(), "{}");
        let notices = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| e.r#type == NOTICE).map(|e| e.data.clone()).collect::<Vec<_>>()
        };
        let (chat, task) = (run("run_chat"), run("run_task"));
        hub.mark_background("daily", "run_task");
        for calls in 1..=4u32 {
            hub.dispatch(&chat, started.clone());
            hub.dispatch(&task, started.clone());
            if calls > 1 {
                hub.same_write_again("daily", "run_chat", "mcp__bipagents__create_page", calls);
                hub.same_write_again("daily", "run_task", "mcp__bipagents__create_page", calls);
            }
            // a conversation's turn at its 3rd identical call, not before
            assert_eq!(lock(&chat.state).braked, calls >= 3, "chat at {calls}");
        }
        assert_eq!(notices(&chat).len(), 1);
        assert_eq!(notices(&chat)[0]["code"], json!("loop_stopped"));
        // a background turn at the task's same_call (5)
        assert!(!lock(&task.state).braked);
        hub.dispatch(&task, started.clone());
        hub.same_write_again("daily", "run_task", "mcp__bipagents__create_page", 5);
        assert!(lock(&task.state).braked && notices(&task).len() == 1);
        // its line kept for whoever tells its person (background_turn.rs `Ran::stopped`): nobody follows a background turn
        let said = hub.brake_said("daily", "run_task").unwrap_or_default();
        assert!(said.starts_with("I stopped") && said.ends_with('.'), "{said}");
        assert!(hub.brake_said("daily", "run_chat").is_some() && hub.brake_said("daily", "run_gone").is_none());
        // nothing followed: nothing to stop
        hub.same_write_again("daily", "run_gone", "mcp__bipagents__create_page", 9);
    }

    #[tokio::test]
    async fn the_bridges_tools_are_counted_in_the_run_that_calls_them() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "daily".into(), ..Default::default() };
        assert_eq!(hub.turn_of("daily", "reminders_add"), None, "no run: a routine's turn");
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let started = |tool: &str| SSEEvent::new("tool.started", json!({"tool": tool}).as_object().cloned().unwrap(), "{}");
        let (chat, task) = (run("run_chat"), run("run_task"));
        hub.mark_background("daily", "run_task");
        hub.dispatch(&chat, started("mcp__bipagents__reminders_add"));
        hub.dispatch(&task, started("mcp_bipagents_notebook_search"));
        let turn = |tool: &str| hub.turn_of("daily", tool).map(|t| (t.run_id, t.background));
        assert_eq!(turn("mcp__bipagents__reminders_add"), Some(("run_chat".into(), false)));
        assert_eq!(turn("notebook_search"), Some(("run_task".into(), true)));
        assert_eq!(turn("calendar_add"), Some(("run_task".into(), true)), "else the one that called a tool last");
        assert_eq!(hub.turn_of("other", "calendar_add"), None);
        lock(&task.state).finished = true;
        assert_eq!(turn("notebook_search"), Some(("run_chat".into(), false)));
    }

    /// urgent_pay.rs `refuses_payment` tells the rehearsal's own pay_with_card from another turn of the agent by the
    /// run inside that very call (review 2, 13/11). Hermes names a tool after its MCP server, here `bipagents_pay` /
    /// `bipagents_link` (`mcp__bipagents_pay__pay_with_card`, `mcp_bipagents_pay_confirm_payment`). Two runs of Vie:
    /// the rehearsal (in the watcher's conversation) and the person's own conversation, each calling a tool a moment
    /// after the other.
    #[tokio::test]
    async fn a_payment_is_told_to_the_run_that_calls_the_pay_tool() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let started = |tool: &str| SSEEvent::new("tool.started", json!({"tool": tool}).as_object().cloned().unwrap(), "{}");
        let (rehearsal, chat) = (run("run_rehearsal"), run("run_chat"));
        hub.mark_background("vie", "run_rehearsal");
        let pause = || std::thread::sleep(std::time::Duration::from_millis(20));
        let turn = |tool: &str| hub.runs_calling("vie", tool).join(",");
        let mut seen = Vec::new();
        // 1. the rehearsal calls pay_with_card (it must be refused); the person's turn calls a tool just after
        for name in ["mcp_bipagents_pay_pay_with_card", "mcp__bipagents_pay__pay_with_card"] {
            hub.dispatch(&rehearsal, started(name));
            pause();
            hub.dispatch(&chat, started("mcp_bipagents_notebook_search"));
            seen.push((name, "rehearsal's own call", turn(crate::card_pay::PAY), "run_rehearsal"));
            pause();
        }
        hub.dispatch(&rehearsal, started("mcp_bipagents_link_pay_with_link"));
        pause();
        hub.dispatch(&chat, started("mcp_bipagents_notebook_search"));
        seen.push(("mcp_bipagents_link_pay_with_link", "rehearsal's own call", turn(crate::link_wallet::PAY), "run_rehearsal"));
        pause();
        // 2. the person's own turn pays for real (never refused as a rehearsal's); the rehearsal browses just after
        hub.dispatch(&chat, started("mcp_bipagents_pay_pay_with_card"));
        pause();
        hub.dispatch(&rehearsal, started("mcp__stealth__get_page_content"));
        seen.push(("mcp_bipagents_pay_pay_with_card", "person's real payment", turn(crate::card_pay::PAY), "run_chat"));
        let wrong: Vec<_> = seen.iter().filter(|(_, _, got, want)| got != want).collect();
        assert!(wrong.is_empty(), "runs_calling(vie, pay tool) picked the wrong run: {wrong:?}");
    }

    #[tokio::test]
    async fn a_browsing_turn_is_stopped_before_hermes_cap() {
        // 2026-10-27, the bench: 145 page reads, cut by Hermes' cap of 150 with no answer
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "daily".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent, "run_browse"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        let started = |tool: &str| SSEEvent::new("tool.started", json!({"tool": tool, "preview": "{}"}).as_object().cloned().unwrap(), "{}");
        hub.dispatch(&sub, started("mcp__stealth__navigate"));
        for _ in 0..145 {
            hub.dispatch(&sub, started("mcp__stealth__get_page_content"));
        }
        let st = lock(&sub.state);
        assert!(st.braked);
        assert_eq!(st.turn.page_reads, 16);
        let notice = st.backlog.iter().find(|e| e.r#type == NOTICE).unwrap();
        assert_eq!(notice.data["reason"], json!("the browser read the page too many times"));
    }

    #[tokio::test]
    async fn a_rooms_turn_is_stopped_before_a_call_that_would_pass_its_limit() {
        // 2026-11-01, the rooms bench: a DeepSeek turn's calls read 41k each after big file reads, 249k for a 200k cap
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "front".into(), ..Default::default() };
        let run = |id: &str| {
            let sub = Arc::new(RunSubscription::new(agent.clone(), id));
            lock(&hub.inner).subs.insert(sub.key(), sub.clone());
            sub
        };
        let usage = |call: u32, input: i64, total_in: i64, total_out: i64| {
            let data = json!({"event": MODEL_USAGE, "call": call, "input_tokens": input, "output_tokens": 400,
                              "totals": {"input_tokens": total_in, "output_tokens": total_out, "cache_read_tokens": 1000}});
            SSEEvent::new(MODEL_USAGE, data.as_object().cloned().unwrap(), "{}")
        };
        let started = || SSEEvent::new("tool.started", json!({"tool": "read_file"}).as_object().cloned().unwrap(), "{}");
        let (turn, task) = (run("run_turn"), run("run_task"));
        hub.limit_tokens("front", "run_turn", 170_000);
        hub.dispatch(&turn, usage(1, 20_000, 20_000, 400));
        hub.dispatch(&turn, started());
        hub.dispatch(&turn, usage(2, 41_000, 61_000, 800));
        hub.dispatch(&turn, started());
        hub.dispatch(&turn, usage(3, 45_000, 106_000, 1_200));
        hub.dispatch(&turn, started());
        let progress = hub.run_progress("front", "run_turn").unwrap();
        assert!(!progress.capped, "106k and a call of 45k: still under 170k");
        assert_eq!(progress.usage, Some(RunUsage { calls: 3, input: 106_000, output: 1_200, cached: 1000, last_input: 45_000 }));
        // its 4th call ends at 156k: a 5th would read 50k at least, past 170k; stopped as it asks for tools
        hub.dispatch(&turn, usage(4, 50_000, 156_000, 1_600));
        assert!(!hub.run_progress("front", "run_turn").unwrap().capped, "not before it asks for tools: may be its answer");
        hub.dispatch(&turn, started());
        let progress = hub.run_progress("front", "run_turn").unwrap();
        assert!(progress.capped);
        assert_eq!(progress.usage.map(|u| (u.total(), u.after_next_call())), Some((157_600, 207_600)));
        // a replayed stream's older totals never go back
        hub.dispatch(&turn, usage(2, 41_000, 61_000, 800));
        assert_eq!(hub.run_progress("front", "run_turn").unwrap().usage.map(|u| u.calls), Some(4));
        // a run without a limit (a task) is only counted; a Hermes without patch 0020: no usage at all
        hub.dispatch(&task, usage(9, 90_000, 400_000, 9_000));
        hub.dispatch(&task, started());
        let progress = hub.run_progress("front", "run_task").unwrap();
        assert!(!progress.capped);
        assert_eq!(progress.usage.unwrap().as_usage(),
            json!({"input_tokens": 400_000, "output_tokens": 9_000, "cache_read_tokens": 1000}).as_object().cloned().unwrap());
        let old = run("run_old");
        hub.limit_tokens("front", "run_old", 170_000);
        hub.dispatch(&old, started());
        let progress = hub.run_progress("front", "run_old").unwrap();
        assert!(progress.usage.is_none() && !progress.capped);
        assert_eq!(RunUsage::of(json!({"call": 1, "input_tokens": 5}).as_object().unwrap()), None, "no totals");
    }

    #[tokio::test]
    async fn a_turn_without_an_answer_ends_on_a_plain_line() {
        let hub = RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None);
        let agent = AgentConfig { name: "wellness".into(), display_name: "Wellness".into(), ..Default::default() };
        let sub = Arc::new(RunSubscription::new(agent, "run_1"));
        lock(&hub.inner).subs.insert(sub.key(), sub.clone());
        let event = |data: Value| SSEEvent::new("run.completed", data.as_object().cloned().unwrap(), "{}");
        hub.dispatch(
            &sub,
            event(json!({"output": "⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries. \
                                    Send `continue` to try again, or switch models with /model."})),
        );
        let ended = lock(&sub.state).backlog.back().cloned().unwrap();
        assert_eq!(ended.data["output"], json!("Wellness couldn't answer this time. Try again, or write “continue”."));
        assert_eq!(ended.data["bip_no_reply"], json!(true));
        assert!(!ended.raw.contains("/model"));
        // Hermes' explanation after a fragment (a stream that stopped early): the fragment, then the plain line
        let sub = Arc::new(RunSubscription::new(
            AgentConfig { name: "wellness".into(), display_name: "Wellness".into(), ..Default::default() },
            "run_3",
        ));
        hub.dispatch(
            &sub,
            event(json!({"output": "Je regarde ta nuit et\n\n⚠️ No reply: streaming stopped early and only a partial \
                                    response was recovered. Send `continue` to resume from where it stopped."})),
        );
        let ended = lock(&sub.state).backlog.back().cloned().unwrap();
        assert_eq!(
            ended.data["output"],
            json!("Je regarde ta nuit et\n\nWellness couldn't answer this time. Try again, or write “continue”.")
        );
        // a real answer is left alone
        let sub = Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_2"));
        hub.dispatch(&sub, event(json!({"output": "⚠️ Il va pleuvoir."})));
        let ended = lock(&sub.state).backlog.back().cloned().unwrap();
        assert_eq!(ended.data["output"], json!("⚠️ Il va pleuvoir."));
        assert!(!ended.data.contains_key("bip_no_reply"));
    }

    #[tokio::test]
    async fn the_audio_made_in_a_turn_is_handed_over_by_its_reply() {
        // 2026-10-28, aibox: a podcast made, the reply carried the day before's path from memory
        let hub = Arc::new(RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None));
        let today = "/bipagents/audio/podcaster/20261028-075253-podcast.m4a";
        let yesterday = "/bipagents/audio/podcaster/20261027-095308-podcast.m4a";
        let ended = |output: &str| {
            SSEEvent::new("run.completed", json!({"output": output}).as_object().cloned().unwrap(), "{}")
        };
        let turn = |id: &str, said: &str| {
            let sub = Arc::new(RunSubscription::new(AgentConfig { name: "podcaster".into(), ..Default::default() }, id));
            hub.dispatch(&sub, delta("message.delta", said));
            sub
        };
        let deltas = |sub: &RunSubscription| {
            lock(&sub.state).backlog.iter().filter(|e| is_delta(&e.r#type)).map(|e| e.text()).collect::<String>()
        };
        crate::made_files::MADE.note("podcaster", today, Some("run_missing"), 1.0);
        crate::made_files::MADE.note("podcaster", today, Some("run_stale"), 1.0);
        crate::made_files::MADE.note("podcaster", today, Some("run_right"), 1.0);
        let files = |end: &SSEEvent| -> Vec<String> {
            end.data.get("attachments").and_then(Value::as_array).into_iter().flatten()
                .filter_map(|a| a["path"].as_str().map(str::to_string)).collect()
        };
        // missing: the end's attachment (2026-11-03: the model's words untouched, no line added)
        let sub = turn("run_missing", "Ton podcast est prêt.");
        let end = hub.hand_over(&sub, ended("Ton podcast est prêt.")).await;
        assert_eq!(end.data["output"], json!("Ton podcast est prêt."));
        assert_eq!(files(&end), [today]);
        assert_eq!(deltas(&sub), "Ton podcast est prêt.", "nothing added to the screen's words");
        // stale: today's as the attachment, yesterday's line marked stale
        let stale = format!("Podcast du jour.\nMEDIA:{yesterday}");
        let sub = turn("run_stale", &stale);
        let end = hub.hand_over(&sub, ended(&stale)).await;
        assert_eq!((files(&end), end.data["stale"].clone()), (vec![today.to_string()], json!([yesterday])));
        // right, and nothing made in the turn (yesterday's asked again): untouched
        for (run, text) in [("run_right", format!("Podcast du jour.\nMEDIA:{today}")), ("run_nothing", format!("Le voici.\nMEDIA:{yesterday}"))] {
            let sub = turn(run, &text);
            let end = hub.hand_over(&sub, ended(&text)).await;
            assert_eq!(end.data["output"], json!(text));
            assert!(!end.data.contains_key("attachments") && !end.data.contains_key("stale"));
        }
    }

    #[tokio::test]
    async fn the_files_a_reply_names_reach_the_apps_as_attachments_only() {
        // 4/11: no `MEDIA:` line in what the apps get, deltas (even a line cut across them) and the end's output alike
        let hub = Arc::new(RunHub::new(HermesClient::new(reqwest::Client::new()), None, 7200, None));
        let sub = Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_m"));
        for said in ["Voici le plan :\nMED", "IA:/bipagents/pages/vie/plan.html\nLa ", "suite.\n  MEDIA: /envoi/devis.pdf"] {
            hub.dispatch(&sub, delta("message.delta", said));
        }
        hub.flush_media(&sub);
        let written = "Voici le plan :\nMEDIA:/bipagents/pages/vie/plan.html\nLa suite.\n  MEDIA: /envoi/devis.pdf";
        hub.dispatch(&sub, SSEEvent::new("run.completed", json!({"output": written}).as_object().cloned().unwrap(), "{}"));
        let st = lock(&sub.state);
        let deltas: String = st.backlog.iter().filter(|e| is_delta(&e.r#type)).map(|e| e.text()).collect();
        assert_eq!(deltas, "Voici le plan :\nLa suite.\n");
        assert!(st.backlog.iter().all(|e| !e.raw.contains("MEDIA:")));
        // each line a file event at once, in its place among the words
        let order: Vec<String> = st.backlog.iter()
            .map(|e| if e.r#type == crate::media::FILE { format!("[{}@{}]", e.data["path"].as_str().unwrap(), e.data["offset"]) } else { e.text() })
            .filter(|t| !t.is_empty()).collect();
        assert_eq!(order[..3], ["Voici le plan :\n".to_string(), "[/bipagents/pages/vie/plan.html@16]".into(), "La suite.\n".into()]);
        assert_eq!(order[3], "[/envoi/devis.pdf@26]");
        let end = st.backlog.back().unwrap();
        assert_eq!(end.data["output"], json!("Voici le plan :\nLa suite."));
        let files: Vec<(&str, &str, i64)> = end.data["attachments"].as_array().unwrap().iter()
            .map(|a| (a["path"].as_str().unwrap(), a["kind"].as_str().unwrap(), a["offset"].as_i64().unwrap())).collect();
        assert_eq!(files, [("/bipagents/pages/vie/plan.html", "page", 15), ("/envoi/devis.pdf", "file", 25)]);
        // the bridge still reads the reply as written (its reply recognised, its files kept)
        assert_eq!(st.output.as_deref(), Some(written));
        assert_eq!(st.media.said(), "Voici le plan :\nMEDIA:/bipagents/pages/vie/plan.html\nLa suite.\n  MEDIA: /envoi/devis.pdf");
        drop(st);
        // words held back as maybe a MEDIA line are given at the end
        let sub = Arc::new(RunSubscription::new(AgentConfig { name: "vie".into(), ..Default::default() }, "run_n"));
        hub.dispatch(&sub, delta("message.delta", "Merci\nME"));
        hub.flush_media(&sub);
        let deltas: String = lock(&sub.state).backlog.iter().filter(|e| is_delta(&e.r#type)).map(|e| e.text()).collect();
        assert_eq!(deltas, "Merci\nME");
    }

    #[test]
    fn the_safety_nets_kinds_of_tools() {
        for tool in ["mcp_stealth_navigate", "stealth_spawn_browser", "mcp__stealth__click_element", "browser_vault"] {
            assert!(browser_tool(tool), "{tool}");
        }
        assert!(browser_tool("mcp_bipagents_vault_create_login"));
        assert!(!browser_tool("web_search") && !browser_tool("mcp_bipagents_create_page"));
        assert!(delegate_tool("delegate_task") && !delegate_tool("delegate"));
        assert!(handoff_tool("mcp_bipagents_start_task") && handoff_tool("propose_task"));
        let refused =
            |data: Value| handoff_refused(&SSEEvent::new("tool.completed", data.as_object().cloned().unwrap(), ""));
        assert!(refused(json!({"tool": "start_task", "error": true})));
        assert!(refused(
            json!({"tool": "start_task", "preview": "Not started: this agent is already on a background task."})
        ));
        assert!(!refused(json!({"tool": "start_task", "error": false, "preview": "Started in the background"})));
    }

    #[test]
    fn a_rescued_task_takes_the_persons_request() {
        let messages: Vec<Dict> = serde_json::from_value(json!([
            {"role": "user", "content": "Cherche-moi une veste en laine bleue, taille M, moins de 150 €"},
            {"role": "assistant", "content": "Je regarde."},
            {"role": "user", "content": "[bipagents:handoff] This is long: hand the rest over"},
            {"role": "user", "content": "[ASYNC DELEGATION d1] done"},
        ]))
        .unwrap();
        let request = person_request(&messages);
        assert_eq!(request, "Cherche-moi une veste en laine bleue, taille M, moins de 150 €");
        assert_eq!(rescue_title(&request), "Cherche-moi une veste en laine bleue, taille M, moins de 150…");
        assert_eq!(rescue_title("Trie mes mails"), "Trie mes mails");
        assert_eq!(rescue_title(""), "Suite de la conversation");
        let prompt = rescue_prompt(&request, &["- mcp_stealth_navigate: zalando.fr".into()], "Je regarde.");
        assert!(prompt.contains("« Cherche-moi une veste") && prompt.contains("- mcp_stealth_navigate: zalando.fr"));
        assert!(prompt.contains("What you had written there so far:\nJe regarde.") && prompt.ends_with("your report."));
        let many: Vec<String> = (0..40).map(|i| format!("- web_search: {i}")).collect();
        let prompt = rescue_prompt("", &many, "");
        assert!(prompt.contains("- … 10 earlier steps\n- web_search: 10\n") && prompt.contains("(their last request"));
    }

    #[test]
    fn forgetting_approvals() {
        let mut inner = HubInner::default();
        let item = |rid: Value| json!({"request_id": rid}).as_object().cloned().unwrap();
        inner.pending.insert(("a".into(), "r".into(), "1".into()), item(json!("req_1")));
        inner.pending.insert(("a".into(), "r".into(), "2".into()), item(Value::Null));
        inner.pending.insert(("a".into(), "r".into(), "3".into()), item(json!("req_3")));
        inner.pending.insert(("a".into(), "other".into(), "4".into()), item(json!("req_1")));
        RunHub::forget_pending(&mut inner, "a", "r", Some(&json!("req_1")));
        let left: Vec<&String> = inner.pending.keys().map(|k| &k.2).collect();
        assert_eq!(left, ["3", "4"]);
        RunHub::forget_pending(&mut inner, "a", "r", Some(&json!("")));
        let left: Vec<&String> = inner.pending.keys().map(|k| &k.2).collect();
        assert_eq!(left, ["4"]);
    }
}
