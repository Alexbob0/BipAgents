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
//! Long-work safety net (2026-10-22, `[jobs] handoff`): a conversation's run that goes past ~15 tool calls or ~3
//! minutes is steered once to hand the rest over to a background task (`handoff`). Since 2026-10-24 (owner's report:
//! a clothes search handed to a sub-agent stopped silently, twice): never while a sub-agent (`delegate_task`) works
//! for the run, nor while the agent browses with its visible browser (the person watches it live; own thresholds,
//! never by default), and the time spent in either does not count. A steered run that still ends without handing over
//! has its task started by the bridge (`rescue`), the conversation told by a `bridge.notice` event before the end.
//!
//! Port of `bridge/bipbridge/runs.py` (owner group: push-runs).
//!
//! Concurrency (Python runs all this on one event loop): the hub's map of runs and its pending approvals sit
//! behind one lock, each run's state behind its own; the hub's lock is always taken first. Dispatching an event,
//! adding or removing a listener happen whole under these locks, as one step of Python's loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
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
/// A question answered or gone: BipAgents' patch (`clarify.responded` / `clarify.cancelled`, `request_id`) or Hermes'
/// upstream contract (`clarify.retired`, one question per `clarify_id`).
pub const CLARIFY_DONE: [&str; 3] = ["clarify.responded", "clarify.cancelled", "clarify.retired"];
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
/// apps show them as a notice. Codes: `handoff_started`, `handoff_proposed`, `handoff_failed`.
pub const NOTICE: &str = "bridge.notice";

fn handoff_tool(tool: &str) -> bool {
    HANDOFF_TOOLS.iter().any(|t| tool == *t || tool.ends_with(&format!("_{t}")))
}

/// The agent's visible browser (skill visible-browser: the `stealth` MCP server's tools, `mcp_stealth_*`; the vault's
/// `browser_vault_*` and `vault_*_login` on that same browser; Hermes' own `browser_*`).
pub fn browser_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    tool.contains("stealth")
        || tool.starts_with("browser_")
        || tool.contains("vault_create_login")
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

/// `_clarify_id(data)`: a question's id (`request_id`, or upstream's `clarify_id`).
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
    /// What was pushed already (approvals by request, questions by `clarify:<id>`).
    pub pushed: HashSet<String>,
    /// The long-work safety net (`handoff`): when the hub began following the run, its tool calls so far, whether the
    /// agent already handed work over (start_task, propose_task, notebook_fill), and whether the bridge steered.
    pub followed_since: f64,
    pub tool_calls: u32,
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
            pushed: HashSet::new(),
            followed_since: crate::py::time_time(),
            tool_calls: 0,
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
    inner: Mutex<HubInner>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    next_id: AtomicU64,
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

    /// After a restart: watch again the runs that were watched, and rebuild what Hermes still waits for (an
    /// approval asked before the restart is not replayed by its event stream, but kept in the run's status).
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
                if let (true, Some(Value::Object(approval))) = (status == "waiting_for_approval", run.get("approval")) {
                    let mut data = approval.clone();
                    data.insert("run_id".into(), json!(run_id));
                    let raw = crate::py::Dumps::new().sort_keys().dumps(&Value::Object(data.clone()));
                    self.dispatch(&sub, SSEEvent::new("approval.request", data, raw));
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
            if st.finished || !st.end_push {
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

    /// A run of the agent goes on in this conversation (the hub follows it).
    pub fn session_busy(&self, agent: &str, session_id: &str) -> bool {
        let inner = lock(&self.inner);
        inner.subs.values().any(|sub| {
            let st = lock(&sub.state);
            sub.agent.name == agent && !st.finished && st.session_id.as_deref() == Some(session_id)
        })
    }

    pub fn subscription(&self, agent: &str, run_id: &str) -> Option<Arc<RunSubscription>> {
        lock(&self.inner).subs.get(&(agent.to_string(), run_id.to_string())).cloned()
    }

    /// What Hermes waits for (approvals, questions), each with `created_at` in Unix seconds.
    pub fn pending_approvals(&self, agent: Option<&str>) -> Vec<Dict> {
        let inner = lock(&self.inner);
        let mut items: Vec<Dict> = inner
            .pending
            .values()
            .filter(|v| agent.is_none() || v.get("agent").and_then(Value::as_str) == agent)
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
        if was_followed && st.followers() == 0 && st.watched && !st.finished {
            self.push_unseen(&inner, sub, &mut st);
        }
        if st.listeners.is_empty() && !st.watched && !st.finished {
            if let Some(cancel) = &st.cancel {
                cancel.cancel();
            }
            let key = sub.key();
            if inner.subs.get(&key).map(|s| Arc::ptr_eq(s, sub)).unwrap_or(false) {
                inner.subs.remove(&key);
            }
        }
    }

    /// The app stopped showing the run (left the conversation, screen locked) while an approval or a
    /// question was waiting: it was on screen, so never pushed. Push it now, or the run waits unseen until
    /// Hermes' timeout.
    fn push_unseen(&self, inner: &HubInner, sub: &RunSubscription, st: &mut SubscriptionState) {
        let Some(push) = self.push.clone() else { return };
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
            .filter(|e| CLARIFY_DONE.contains(&e.r#type.as_str()))
            .map(|e| clarify_id(&e.data))
            .collect();
        let questions: Vec<SSEEvent> = st
            .backlog
            .iter()
            .filter(|e| e.r#type == "clarify.request" && !answered.contains(&clarify_id(&e.data)))
            .cloned()
            .collect();
        for event in questions {
            self.on_clarify(sub, st, &event);
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
                                if terminal {
                                    self.rescue(sub, &event).await;
                                }
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
                        if !st.watched {
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
                if st.listeners.is_empty() && !st.watched {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs_f64(backoff)).await;
            backoff = (backoff * 2.0).min(BACKOFF_MAX);
        }
    }

    fn dispatch(&self, sub: &Arc<RunSubscription>, event: SSEEvent) {
        let mut inner = lock(&self.inner);
        let mut st = lock(&sub.state);
        let event = if event.r#type == "approval.request" { self.in_plain_words(sub, &st, event) } else { event };
        append_to_backlog(&mut st.backlog, &event);
        st.send_all(Some(&event));
        let kind = event.r#type.as_str();
        if kind == "tool.started" || kind == "tool.completed" || kind == SUBAGENT_START || kind == SUBAGENT_DONE {
            self.handoff(sub, &mut st, &event);
        }
        if kind == "tool.started" {
            // a site its browser or web tools open is open to its programs too (sandbox_sites.rs)
            if let Some(shared) = self.config.as_ref() {
                let text = |key: &str| event.data.get(key).and_then(Value::as_str).unwrap_or("");
                let tool = if text("tool").is_empty() { text("name") } else { text("tool") };
                crate::sandbox_sites::note_tool_call(&shared.get(), &sub.agent.name, tool, text("preview"));
            }
        }
        if kind == "approval.request" {
            self.on_approval(&mut inner, sub, &mut st, &event);
        } else if kind == "clarify.request" {
            self.on_clarify(sub, &mut st, &event);
        } else if kind == "approval.responded" {
            Self::forget_pending(&mut inner, &sub.agent.name, &sub.run_id, event.data.get("request_id"));
        } else if is_terminal(kind) {
            st.terminal = Some(kind.to_string());
            let output = as_str(event.data.get("output"));
            if output.is_some() {
                st.output = output.clone();
            }
            Self::forget_pending(&mut inner, &sub.agent.name, &sub.run_id, None);
            if let (Some(shared), Some(text)) =
                (self.config.clone(), output.clone().filter(|o| kind == "run.completed" && o.contains("MEDIA:")))
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
            if let (true, Some(push)) = (st.watched && st.end_push && st.followers() == 0, self.push.clone()) {
                let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
                match output.filter(|o| kind == "run.completed" && !o.is_empty()) {
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
        match event.r#type.as_str() {
            "tool.started" => {
                st.tool_calls += 1;
                if st.recent_tools.len() >= 100 {
                    st.recent_tools.pop_front();
                }
                st.recent_tools.push_back(tool.to_string());
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
        if st.steered || st.handed_off || !st.end_push || st.finished || st.delegating > 0 || st.subagents > 0 {
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
            if !st.steered || st.handed_off || !st.end_push || st.rescued {
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
        let budget = crate::jobs::direct_budget(&config);
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

    /// A question (docs/hermes-clarify-api.md) nobody sees: push it once, like an approval.
    fn on_clarify(&self, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        let data = &event.data;
        let request_id = clarify_id(data);
        let dedupe =
            format!("clarify:{}", request_id.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| sha16(&event.raw)));
        let Some(push) = self.push.clone() else { return };
        if !st.watched || st.followers() > 0 || st.pushed.contains(&dedupe) {
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
        let lang = match &self.config {
            Some(shared) => {
                let config = shared.get();
                let agent = config.agent(Some(&sub.agent.name)).cloned().unwrap_or_else(|| sub.agent.clone());
                crate::handback::language_of(&config, &agent)
            }
            None => "en".to_string(),
        };
        let last = st.backlog.iter().rev().find(|e| e.r#type == "tool.started").map(|e| {
            let text = |key: &str| e.data.get(key).and_then(Value::as_str).unwrap_or("").to_string();
            let tool = text("tool");
            crate::approval_text::LastTool { name: if tool.is_empty() { text("name") } else { tool }, preview: text("preview") }
        });
        let name = if sub.agent.display_name.trim().is_empty() { &sub.agent.name } else { &sub.agent.display_name };
        let Some(described) = crate::approval_text::describe(name, &lang, &event.data, last.as_ref()) else { return event };
        let mut data = event.data;
        data.insert("bip_summary".into(), json!(described.summary));
        if let Some(detail) = described.detail {
            data.insert("bip_detail".into(), json!(detail));
        }
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
        // the apps' own reading needs Hermes' pattern key; the plain words (in_plain_words) go with it
        for key in ["pattern_key", "bip_summary", "bip_detail"] {
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
        inner.pending.insert((sub.agent.name.clone(), sub.run_id.clone(), dedupe.clone()), item);
        let followers = st.followers();
        let push = match self.push.clone() {
            Some(push) if st.watched && followers == 0 && !st.pushed.contains(&dedupe) => push,
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

    #[test]
    fn the_safety_nets_kinds_of_tools() {
        for tool in
            ["mcp_stealth_navigate", "stealth_spawn_browser", "mcp__stealth__click_element", "browser_vault_fill"]
        {
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
