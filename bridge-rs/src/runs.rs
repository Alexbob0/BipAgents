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

use crate::config::AgentConfig;
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
            inner: Mutex::new(HubInner::default()),
            tasks: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        }
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
                let (push, agent, run_id, session) =
                    (push.clone(), agent.clone(), run_id.clone(), st.session_id.clone());
                let request_id = as_str(item.get("request_id"));
                let command = as_str(item.get("command"));
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
        append_to_backlog(&mut st.backlog, &event);
        st.send_all(Some(&event));
        let kind = event.r#type.as_str();
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

    fn on_approval(&self, inner: &mut HubInner, sub: &RunSubscription, st: &mut SubscriptionState, event: &SSEEvent) {
        let data = &event.data;
        let request_id = as_str(data.get("request_id"));
        let command = as_str(data.get("command"));
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
        let (agent, run_id, session) = (sub.agent.name.clone(), sub.run_id.clone(), st.session_id.clone());
        self.spawn(async move {
            push.notify_approval_values(
                &agent,
                &run_id,
                request_id.as_deref(),
                Some(&choices),
                command.as_deref(),
                session.as_deref(),
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
