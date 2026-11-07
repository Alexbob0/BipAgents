//! An agent's waiting messages (« En attente », Alex 2026-11-02: « File de messages visible par agent… et un moyen de
//! le pousser au milieu d'une exécution, pour rajouter du contexte ou corriger »).
//!
//! While an agent works on a turn, a message the person sends goes into that turn by default (Hermes v0.22.5's
//! `POST /v1/runs/{id}/steer`: delivered as its own user message right after the agent's current tool calls, without
//! stopping it; the apps mark it « Ajouté pendant la réponse »). « Après » puts it here instead: the bridge keeps it,
//! per agent, on disk, and starts it as the next turn of its conversation once the turn going on there ends, one after
//! another. Every device of the person reads the same list (`GET`, the run stream's `bridge.queue` events, the
//! queue's own event stream, `session_state`), and changes it the same way:
//!
//!     GET    /v1/agents/{id}/queue                    the agent's queue (below)
//!     POST   /v1/agents/{id}/queue/order              {ids: [...]}  those first, in that order; the rest after
//!     DELETE /v1/agents/{id}/queue/{item}             cancelled
//!     POST   /v1/agents/{id}/queue/{item}/now         into the turn going on (a steer), else started at once
//!     POST   /v1/agents/{id}/queue/pause | resume     paused, nothing new starts (the turn going on finishes)
//!     GET    /v1/agents/{id}/queue/events             SSE: `queue` (the whole queue, at once and at each change),
//!                                                     `queue.started` {run_id, session_id, item_id, text}
//!
//! Messages come in through the conversation's one sending route (threads/send.rs, `POST /v1/threads/{id}/messages`):
//! into the turn going on (a steer), or here with « Après », or when the turn takes no words (files, a turn just
//! ending).
//!
//! The queue: `{agent, version, paused, items: [{id, session_id, text, created_at, late, client_id, attachments?,
//! voice?}], running:
//! {item_id, run_id, session_id, text, started_at} | null}`; `version` grows at each change (an app keeps the highest it saw), `late`:
//! words a turn no longer took (Hermes' `pending_steer` on its end), first in line. `attachments`: the message's files
//! as the apps show them (`{kind, name, mime, size, path}`, a photo kept as a file by threads/send.rs), `voice`: sent
//! as a voice note (its words are the transcript) — a message of files alone has no `text` (6/11: its row was blank).
//!
//! What the bridge decides (the run hub tells it each run's end, runs.rs):
//! - the next item starts when nothing runs in its conversation (the hub's runs, else Hermes asked about the last run
//!   the queue knows there) and the queue isn't paused; one item at a time per agent;
//! - a turn the person stopped, or that failed, pauses the queue when items wait in its conversation (nothing goes on
//!   behind a stop, nor into a model that just failed); « Reprendre » goes on;
//! - « reply ready » is pushed once, at the end of the last turn (the hub skips it while more follows), under the
//!   agent's « Me prévenir » switch as before (push.rs);
//! - words steered through the bridge that the turn did not take come back first in the queue (the apps leave them
//!   to the bridge when it has this feature: `agent_queue` in /health).
//!
//! Rooms, background tasks and Live calls run in their own conversations: no item is ever in them, so they neither
//! wait for the queue nor hold it.
//!
//! No Python counterpart. Files: `data_dir/queue/<agent>.json` (0600, folder 0700).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::{HermesClient, HermesError};
use crate::runs::RunHub;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};

/// Items an agent's queue holds at most.
pub const ITEMS_MAX: usize = 30;
/// One item's words at most (a message, not a document).
pub const TEXT_MAX: usize = 20_000;
/// How often the queues are looked at besides the runs' ends (a run the hub didn't follow, a start that failed).
pub const TICK_SECONDS: f64 = 10.0;
/// The event the apps get in a run's stream when the queue changes (live only, not replayed).
pub const RUN_EVENT: &str = "bridge.queue";
/// Run states during which a conversation is busy (Hermes v0.22.5: `queued`, `claimed`, `running`,
/// `waiting_for_approval`, `stopping`, `unknown` until resolved).
const BUSY_STATES: [&str; 6] = ["queued", "claimed", "running", "waiting_for_approval", "stopping", "unknown"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Item {
    pub id: String,
    pub session_id: String,
    pub text: String,
    pub created_at: String,
    #[serde(default)]
    pub late: bool,
    /// The app's id for the message (threads/send.rs): its timeline item carries it once Hermes has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// What goes to Hermes when it starts (notes, photos), kept off the apps' answers; else `text`.
    #[serde(default, skip_serializing)]
    pub input: Option<Value>,
    /// Its files as the apps show them (never their bytes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Value>,
    /// Sent as a voice note.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voice: bool,
    /// The phone it was sent from (threads/send.rs), for its turn (`RunHub::set_phone`); not shown to the apps.
    #[serde(default, skip_serializing)]
    pub phone: Option<String>,
}

/// A message the sending route hands to the queue: its app id and Hermes' input.
#[derive(Debug, Clone, Default)]
pub struct Sent {
    pub client_id: Option<String>,
    pub input: Option<Value>,
    /// Its files as the apps show them (`Attachment::shown`).
    pub attachments: Vec<Value>,
    pub voice: bool,
    /// The phone it was sent from (« ios », « android »), when the app said.
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Running {
    pub item_id: String,
    pub run_id: String,
    pub session_id: String,
    pub text: String,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Steer {
    run_id: String,
    text: String,
    #[serde(default)]
    client_id: Option<String>,
}

/// One agent's queue, as kept on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AgentQueue {
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub running: Option<Running>,
    /// The last run the queue knows of in each conversation (the one an item waits for, the one it started).
    #[serde(default)]
    runs: BTreeMap<String, String>,
    /// Words steered through the bridge into runs going on: those a run did not take come back in the queue.
    #[serde(default)]
    steers: Vec<Steer>,
}

impl AgentQueue {
    /// What the apps read.
    pub fn snapshot(&self, agent: &str) -> Value {
        json!({"agent": agent, "version": self.version, "paused": self.paused, "items": self.items,
               "running": self.running})
    }

    /// The same, for one conversation only (its items; `running` when it runs there).
    pub fn snapshot_of(&self, agent: &str, session_id: &str) -> Value {
        let items: Vec<&Item> = self.items.iter().filter(|i| i.session_id == session_id).collect();
        let running = self.running.as_ref().filter(|r| r.session_id == session_id);
        json!({"agent": agent, "version": self.version, "paused": self.paused, "items": items, "running": running})
    }

    fn changed(&mut self) {
        self.version += 1;
        // nothing left to wait: no pause kept for later (a forgotten pause would hold tomorrow's « Après »)
        if self.items.is_empty() {
            self.paused = false;
        }
    }

    fn waits_in(&self, session_id: &str) -> bool {
        self.items.iter().any(|i| i.session_id == session_id)
    }

    /// Ids listed first, in that order; the rest after, as they were. Unknown ids are left out.
    pub fn reorder(&mut self, ids: &[String]) {
        let mut rest = std::mem::take(&mut self.items);
        let mut ordered = Vec::with_capacity(rest.len());
        for id in ids {
            if let Some(at) = rest.iter().position(|i| &i.id == id) {
                ordered.push(rest.remove(at));
            }
        }
        ordered.extend(rest);
        self.items = ordered;
    }

    /// A run of the agent ended: what changes in the queue, and whether more work follows in its conversation (no
    /// « reply ready » then).
    pub fn run_ended(
        &mut self,
        run_id: &str,
        session_id: Option<&str>,
        kind: &str,
        pending_steer: Option<&str>,
    ) -> (bool, bool) {
        let mut changed = false;
        if self.running.as_ref().is_some_and(|r| r.run_id == run_id) {
            self.running = None;
            changed = true;
        }
        let session = session_id
            .map(str::to_string)
            .or_else(|| self.runs.iter().find(|(_, r)| r.as_str() == run_id).map(|(s, _)| s.clone()));
        if let Some(s) = session.as_deref() {
            if self.runs.get(s).is_some_and(|r| r == run_id) {
                self.runs.remove(s);
            }
        }
        // words steered through the bridge that the turn never took: first in line
        let steered: Vec<Steer> = self.steers.iter().filter(|s| s.run_id == run_id).cloned().collect();
        self.steers.retain(|s| s.run_id != run_id);
        if let (Some(s), Some(pending)) = (session.as_deref(), pending_steer.map(str::trim).filter(|p| !p.is_empty())) {
            if let Some(first) = steered.iter().find(|t| pending.contains(t.text.trim())) {
                let sent = Sent { client_id: first.client_id.clone(), ..Sent::default() };
                self.items.insert(0, new_item(s, pending, true, sent));
                changed = true;
            }
        }
        let Some(s) = session else {
            if changed {
                self.changed();
            }
            return (changed, false);
        };
        if self.waits_in(&s)
            && matches!(kind, "run.cancelled" | "run.canceled" | "run.interrupted" | "run.failed")
            && !self.paused
        {
            // stopped by the person, or failed: nothing goes on behind it until « Reprendre »
            self.paused = true;
            changed = true;
        }
        if changed {
            self.changed();
        }
        (changed, !self.paused && self.waits_in(&s))
    }
}

fn new_item(session_id: &str, text: &str, late: bool, sent: Sent) -> Item {
    Item {
        id: format!("q{}", crate::py::token_urlsafe(9).replace(['-', '_'], "x")),
        session_id: session_id.to_string(),
        text: text.to_string(),
        created_at: crate::store::iso(crate::store::utcnow()),
        late,
        client_id: sent.client_id,
        input: sent.input,
        attachments: sent.attachments,
        voice: sent.voice,
        phone: sent.phone,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Every agent's queue, the runs they start, and the devices told of each change.
pub struct Queues {
    config: SharedConfig,
    hermes: HermesClient,
    hub: Arc<RunHub>,
    state: Mutex<HashMap<String, AgentQueue>>,
    /// One look at the queues at a time (a run's end and a tick, two devices adding at once).
    advancing: tokio::sync::Mutex<()>,
    events: broadcast::Sender<(String, String, Value)>,
    me: Weak<Queues>,
}

impl Queues {
    pub fn new(config: SharedConfig, hermes: HermesClient, hub: Arc<RunHub>) -> Arc<Queues> {
        let (events, _) = broadcast::channel(256);
        Arc::new_cyclic(|me| Queues {
            config,
            hermes,
            hub,
            state: Mutex::new(HashMap::new()),
            advancing: tokio::sync::Mutex::new(()),
            events,
            me: me.clone(),
        })
    }

    fn folder(&self) -> PathBuf {
        PathBuf::from(self.config.get().data_dir()).join("queue")
    }

    fn path_of(&self, agent: &str) -> PathBuf {
        self.folder().join(format!("{agent}.json"))
    }

    fn read(&self, agent: &str) -> AgentQueue {
        std::fs::read_to_string(self.path_of(agent))
            .ok()
            .and_then(|text| serde_json::from_str::<AgentQueue>(&text).ok())
            .unwrap_or_default()
    }

    fn write(&self, agent: &str, queue: &AgentQueue) {
        use std::os::unix::fs::DirBuilderExt;
        let folder = self.folder();
        if !folder.is_dir() {
            if let Some(parent) = folder.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::DirBuilder::new().mode(0o700).create(&folder);
        }
        let path = self.path_of(agent);
        let temporary = path.with_extension("tmp");
        let text = serde_json::to_string(queue).unwrap_or_default();
        let written = crate::py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)
            .and_then(|_| std::fs::rename(&temporary, &path));
        if let Err(exc) = written {
            tracing::error!(agent = %agent, error = %exc, "queue: not saved");
        }
    }

    /// The agent's queue (read from disk the first time).
    pub fn get(&self, agent: &str) -> AgentQueue {
        let mut state = lock(&self.state);
        state.entry(agent.to_string()).or_insert_with(|| self.read(agent)).clone()
    }

    /// Changes the agent's queue: kept on disk and told to every device when `change` says it changed.
    fn update<T>(&self, agent: &str, change: impl FnOnce(&mut AgentQueue) -> (bool, T)) -> (AgentQueue, T) {
        let (queue, changed, out) = self.update_quiet(agent, change);
        if changed {
            self.tell(agent, "queue", queue.snapshot(agent));
        }
        (queue, out)
    }

    /// [`Self::update`] without telling the devices (the caller does, outside any lock of the hub's).
    fn update_quiet<T>(&self, agent: &str, change: impl FnOnce(&mut AgentQueue) -> (bool, T)) -> (AgentQueue, bool, T) {
        let (queue, changed, out) = {
            let mut state = lock(&self.state);
            let queue = state.entry(agent.to_string()).or_insert_with(|| self.read(agent));
            let (changed, out) = change(queue);
            if changed {
                self.write(agent, queue);
            }
            (queue.clone(), changed, out)
        };
        (queue, changed, out)
    }

    fn tell(&self, agent: &str, kind: &str, data: Value) {
        let _ = self.events.send((agent.to_string(), kind.to_string(), data.clone()));
        if let Value::Object(map) = data {
            let mut map = map;
            map.insert("kind".into(), json!(kind));
            self.hub.tell_agent_live(agent, RUN_EVENT, map);
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String, Value)> {
        self.events.subscribe()
    }

    /// Looks at the agent's queue soon (a task of its own: never inside the hub's locks).
    pub fn kick(&self, agent: &str) {
        let (Some(me), Ok(runtime)) = (self.me.upgrade(), tokio::runtime::Handle::try_current()) else { return };
        let agent = agent.to_string();
        runtime.spawn(async move { me.advance(&agent).await });
    }

    /// The run hub's word at each run's end (inside its locks: quick, no I/O but the queue's small file). Returns
    /// whether more of the queue follows in that conversation (the hub then pushes no « reply ready »).
    pub fn run_ended(
        &self,
        agent: &str,
        run_id: &str,
        session_id: Option<&str>,
        kind: &str,
        pending_steer: Option<&str>,
    ) -> bool {
        let known = {
            let state = lock(&self.state);
            state.get(agent).cloned()
        };
        // an agent whose queue was never read: only its file can say (none, for most agents)
        if known.is_none() && !self.path_of(agent).exists() {
            return false;
        }
        let (queue, changed, (_, follows)) = self.update_quiet(agent, |q| {
            let (changed, follows) = q.run_ended(run_id, session_id, kind, pending_steer);
            (changed, (changed, follows))
        });
        if follows {
            tracing::info!(agent = %agent, run_id = %run_id, "queue: the next message follows");
        }
        // told and looked at in a task of its own: the hub's locks are held here
        if let (Some(me), Ok(runtime)) = (self.me.upgrade(), tokio::runtime::Handle::try_current()) {
            let agent = agent.to_string();
            runtime.spawn(async move {
                if changed {
                    me.tell(&agent, "queue", queue.snapshot(&agent));
                }
                me.advance(&agent).await;
            });
        }
        follows
    }

    /// A message to send after the turn going on (`waits_for`: the run the app follows there).
    pub fn add(
        &self,
        agent: &AgentConfig,
        session_id: &str,
        text: &str,
        waits_for: Option<&str>,
        front: bool,
        sent: Sent,
    ) -> Result<AgentQueue, &'static str> {
        let (queue, added) = self.update(&agent.name, |q| {
            if q.items.len() >= ITEMS_MAX {
                return (false, false);
            }
            let item = new_item(session_id, text.trim(), false, sent);
            if front {
                q.items.insert(0, item);
            } else {
                q.items.push(item);
            }
            if let Some(run) = waits_for {
                if q.running.as_ref().is_none_or(|r| r.session_id != session_id) {
                    q.runs.insert(session_id.to_string(), run.to_string());
                }
            }
            q.changed();
            (true, true)
        });
        if !added {
            return Err("full");
        }
        tracing::info!(agent = %agent.name, items = queue.items.len(), front = front, "queue: message added");
        self.kick(&agent.name);
        Ok(queue)
    }

    pub fn remove(&self, agent: &str, item_id: &str) -> Option<AgentQueue> {
        let (queue, found) = self.update(agent, |q| {
            let before = q.items.len();
            q.items.retain(|i| i.id != item_id);
            let found = q.items.len() != before;
            if found {
                q.changed();
            }
            (found, found)
        });
        found.then_some(queue)
    }

    pub fn reorder(&self, agent: &str, ids: &[String]) -> AgentQueue {
        let (queue, _) = self.update(agent, |q| {
            let before = q.items.clone();
            q.reorder(ids);
            let changed = q.items != before;
            if changed {
                q.changed();
            }
            (changed, ())
        });
        self.kick(agent);
        queue
    }

    pub fn set_paused(&self, agent: &str, paused: bool) -> AgentQueue {
        let (queue, _) = self.update(agent, |q| {
            // a pause with nothing waiting is no pause (`changed`)
            let changed = q.paused != paused && !(paused && q.items.is_empty());
            if changed {
                q.paused = paused;
                q.changed();
            }
            (changed, ())
        });
        tracing::info!(agent = %agent, paused = queue.paused, "queue: paused or resumed");
        if !paused {
            self.kick(agent);
        }
        queue
    }

    /// The run going on in a conversation, as the hub follows it.
    pub fn running_run(&self, agent: &str, session_id: &str) -> Option<String> {
        self.hub.session_run(agent, session_id)
    }

    /// Words into the turn going on in `session_id` (`run_id`: the one the app follows). Ok(Some(run)) when Hermes
    /// took them; Ok(None) when no turn takes words now (the caller queues them).
    pub async fn steer(
        &self,
        agent: &AgentConfig,
        session_id: &str,
        run_id: Option<&str>,
        text: &str,
        client_id: Option<&str>,
    ) -> Result<Option<String>, HermesError> {
        let Some(run) = run_id.map(str::to_string).or_else(|| self.running_run(&agent.name, session_id)) else {
            return Ok(None);
        };
        match self.hermes.steer_run(agent, &run, text).await {
            Ok(()) => {
                // kept quietly: nothing the apps show changed
                self.update_quiet(&agent.name, |q| {
                    q.steers.push(Steer {
                        run_id: run.clone(),
                        text: text.trim().to_string(),
                        client_id: client_id.map(str::to_string),
                    });
                    let extra = q.steers.len().saturating_sub(50);
                    q.steers.drain(..extra);
                    (true, ())
                });
                tracing::info!(agent = %agent.name, run_id = %run, "steer: words added to the turn going on");
                Ok(Some(run))
            }
            Err(HermesError::Http { status, .. }) if [404, 409, 410].contains(&status) => Ok(None),
            Err(exc) => Err(exc),
        }
    }

    /// Whether a run goes on in the agent's conversation: the hub's runs, else the last run the queue knows there.
    async fn busy(&self, agent: &AgentConfig, session_id: &str) -> bool {
        if self.hub.session_run(&agent.name, session_id).is_some() {
            return true;
        }
        let Some(run) = self.get(&agent.name).runs.get(session_id).cloned() else { return false };
        if self.hub.subscription(&agent.name, &run).is_some() {
            // followed by the hub and over (else `session_run` had it): its end is ours already
            self.forget_run(&agent.name, session_id, &run);
            return false;
        }
        match self.hermes.run(agent, &run).await {
            Ok(status) => {
                let state = status.get("status").and_then(Value::as_str).unwrap_or("").to_string();
                if BUSY_STATES.contains(&state.as_str()) {
                    // followed from now on: its end starts what waits
                    self.hub.keep(agent, &run, Some(session_id));
                    true
                } else {
                    self.forget_run(&agent.name, session_id, &run);
                    false
                }
            }
            Err(HermesError::Http { status, .. }) if [404, 410].contains(&status) => {
                self.forget_run(&agent.name, session_id, &run);
                false
            }
            Err(_) => true, // Hermes not reached: asked again at the next look
        }
    }

    fn forget_run(&self, agent: &str, session_id: &str, run_id: &str) {
        self.update(agent, |q| {
            let known = q.runs.get(session_id).is_some_and(|r| r == run_id);
            if known {
                q.runs.remove(session_id);
            }
            if q.running.as_ref().is_some_and(|r| r.run_id == run_id) {
                q.running = None;
                q.changed();
            }
            (known, ())
        });
    }

    /// Starts the agent's next item when nothing holds it (paused, a turn going on in its conversation, or the
    /// queue's own turn still running).
    pub async fn advance(&self, agent_name: &str) {
        let _one = self.advancing.lock().await;
        let Some(agent) = self.config.get().agent(Some(agent_name)).cloned() else { return };
        let queue = self.get(&agent.name);
        let Some(head) = queue.items.first().cloned().filter(|_| !queue.paused) else { return };
        if self.busy(&agent, &head.session_id).await {
            return;
        }
        if let Some(running) = queue.running.as_ref().filter(|r| r.session_id != head.session_id) {
            if self.busy(&agent, &running.session_id).await {
                return;
            }
        }
        self.start(&agent, &head).await;
    }

    /// Starts an item as a turn of its conversation (taken out of the list once Hermes has it).
    async fn start(&self, agent: &AgentConfig, item: &Item) -> Option<String> {
        let input = item.input.clone().unwrap_or_else(|| Value::String(item.text.clone()));
        let run_id = match self.hermes.start_run_input(agent, input, &item.session_id, None).await {
            Ok(run_id) => run_id,
            Err(exc) => {
                tracing::warn!(agent = %agent.name, error = %exc, "queue: the next message did not start (tried again soon)");
                return None;
            }
        };
        // watched: kept across a restart, « reply ready » pushed at the end if nobody looks (and nothing follows)
        self.hub.watch(agent, &run_id, Some(&item.session_id), true);
        self.hub.keep(agent, &run_id, Some(&item.session_id));
        self.hub.set_phone(&agent.name, &run_id, item.phone.as_deref());
        let started = crate::store::iso(crate::store::utcnow());
        self.update(&agent.name, |q| {
            q.items.retain(|i| i.id != item.id);
            q.running = Some(Running {
                item_id: item.id.clone(),
                run_id: run_id.clone(),
                session_id: item.session_id.clone(),
                text: item.text.clone(),
                started_at: started,
            });
            q.runs.insert(item.session_id.clone(), run_id.clone());
            q.changed();
            (true, ())
        });
        tracing::info!(agent = %agent.name, run_id = %run_id, "queue: next message started");
        self.tell(
            &agent.name,
            "queue.started",
            json!({"agent": agent.name, "run_id": run_id, "session_id": item.session_id, "item_id": item.id,
                   "text": item.text}),
        );
        Some(run_id)
    }

    /// « Envoyer maintenant »: into the turn going on in its conversation, else started at once (even paused).
    pub async fn send_now(&self, agent: &AgentConfig, item_id: &str) -> Result<Value, HttpError> {
        let queue = self.get(&agent.name);
        let item = queue.items.iter().find(|i| i.id == item_id).cloned().ok_or_else(HttpError::not_found)?;
        let steerable = item.input.as_ref().is_none_or(Value::is_string);
        let steered = match steerable {
            true => self.steer(agent, &item.session_id, None, &item.text, item.client_id.as_deref()).await.map_err(hermes_error)?,
            false => None,
        };
        if let Some(run) = steered {
            self.hub.set_phone(&agent.name, &run, item.phone.as_deref());
            let queue = self.remove(&agent.name, item_id).unwrap_or_else(|| self.get(&agent.name));
            return Ok(json!({"steered": true, "run_id": run, "queue": queue.snapshot(&agent.name)}));
        }
        let _one = self.advancing.lock().await;
        if self.busy(agent, &item.session_id).await {
            // a turn just started there and takes no words yet: first in line instead
            let mut ids = vec![item.id.clone()];
            ids.extend(self.get(&agent.name).items.iter().map(|i| i.id.clone()));
            let queue = self.reorder(&agent.name, &ids);
            return Ok(json!({"steered": false, "queue": queue.snapshot(&agent.name)}));
        }
        let run = self
            .start(agent, &item)
            .await
            .ok_or_else(|| HttpError::new(502, "your agent isn't answering right now: try again in a moment"))?;
        Ok(json!({"steered": false, "run_id": run, "queue": self.get(&agent.name).snapshot(&agent.name)}))
    }

    /// Every agent with items waiting, looked at again (a run's end the hub never saw, a start that failed, a restart).
    pub async fn run_forever(self: Arc<Self>) {
        // the queues kept on disk before a restart
        if let Ok(entries) = std::fs::read_dir(self.folder()) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if let Some(agent) = name.strip_suffix(".json") {
                    let _ = self.get(agent);
                }
            }
        }
        loop {
            let agents: Vec<String> = {
                let state = lock(&self.state);
                state.iter().filter(|(_, q)| !q.items.is_empty() && !q.paused).map(|(a, _)| a.clone()).collect()
            };
            for agent in agents {
                self.advance(&agent).await;
            }
            tokio::time::sleep(Duration::from_secs_f64(TICK_SECONDS)).await;
        }
    }
}

fn hermes_error(exc: HermesError) -> HttpError {
    match exc.status() {
        Some(401) | Some(403) => HttpError::new(502, "your agent refused this request"),
        _ => HttpError::new(502, "your agent isn't answering right now: try again in a moment"),
    }
}

/// The queues of this bridge, made with its state (and told of each run's end by the hub).
pub fn queues_of(state: &AppState) -> Arc<Queues> {
    state
        .queues
        .get_or_init(|| {
            let queues = Queues::new(state.config.clone(), state.hermes.clone(), state.hub.clone());
            let _ = state.hub.queues.set(Arc::downgrade(&queues));
            queues
        })
        .clone()
}

// -- routes -----------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Order {
    pub ids: Vec<String>,
}

impl Validate for Order {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

fn agent_of(state: &AppState, agent_id: &str) -> ApiResult<AgentConfig> {
    crate::app::agent_or_404(&state.config(), Some(agent_id))
}

async fn get_queue(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    Ok(web::ok(queues_of(&state).get(&agent.name).snapshot(&agent.name)))
}

async fn order_items(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<Order>,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    Ok(web::ok(queues_of(&state).reorder(&agent.name, &body.ids).snapshot(&agent.name)))
}

async fn delete_item(
    State(state): State<AppState>,
    UrlPath((agent_id, item_id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    let queue = queues_of(&state).remove(&agent.name, &item_id).ok_or_else(HttpError::not_found)?;
    tracing::info!(agent = %agent.name, "queue: message cancelled");
    Ok(web::ok(queue.snapshot(&agent.name)))
}

async fn send_now(
    State(state): State<AppState>,
    UrlPath((agent_id, item_id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    Ok(web::ok(queues_of(&state).send_now(&agent, &item_id).await?))
}

async fn pause(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    Ok(web::ok(queues_of(&state).set_paused(&agent.name, true).snapshot(&agent.name)))
}

async fn resume(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    Ok(web::ok(queues_of(&state).set_paused(&agent.name, false).snapshot(&agent.name)))
}

async fn queue_events(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    let queues = queues_of(&state);
    let mut events = queues.subscribe();
    let first = queues.get(&agent.name).snapshot(&agent.name);
    let name = agent.name.clone();
    let line =
        |kind: &str, data: &Value| bytes::Bytes::from(format!("event: {kind}\ndata: {}\n\n", crate::py::dumps(data)));
    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(line("queue", &first));
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(crate::app::SSE_KEEPALIVE_SECONDS), events.recv()).await {
                Err(_) => yield Ok(bytes::Bytes::from_static(b": keepalive\n\n")),
                Ok(Ok((agent, kind, data))) if agent == name => yield Ok(line(&kind, &data)),
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(broadcast::error::RecvError::Closed)) => break,
            }
        }
    };
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert("content-type", axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    headers.insert("x-accel-buffering", axum::http::HeaderValue::from_static("no"));
    Ok(response)
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/queue", get(get_queue))
        .route("/agents/{agent_id}/queue/order", post(order_items))
        .route("/agents/{agent_id}/queue/pause", post(pause))
        .route("/agents/{agent_id}/queue/resume", post(resume))
        .route("/agents/{agent_id}/queue/events", get(queue_events))
        .route("/agents/{agent_id}/queue/{item_id}", delete(delete_item))
        .route("/agents/{agent_id}/queue/{item_id}/now", post(send_now))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, session: &str) -> Item {
        Item {
            id: id.into(),
            session_id: session.into(),
            text: format!("text {id}"),
            created_at: "t".into(),
            late: false,
            client_id: None,
            input: None,
            attachments: Vec::new(),
            voice: false,
            phone: None,
        }
    }

    #[test]
    fn a_message_of_files_alone_says_its_files() {
        // 6/11: a photo sent alone waited as an empty row (« En attente (1) », nothing to read)
        let photo = json!({"kind": "image", "name": "photo.jpg", "mime": "image/jpeg", "size": 1234, "path": "/u/2026-11/a-photo.jpg"});
        let sent = Sent { client_id: Some("c1".into()), input: None, attachments: vec![photo.clone()], voice: false, phone: None };
        let mut q = queue_of(&[]);
        q.items.push(new_item("s1", "", false, sent));
        let shown = q.snapshot("maison");
        assert_eq!(shown["items"][0]["text"], "");
        assert_eq!(shown["items"][0]["attachments"][0], photo);
        assert!(shown["items"][0].get("voice").is_none() && shown["items"][0].get("input").is_none());
        let voice = Sent { voice: true, ..Sent::default() };
        q.items.push(new_item("s1", "rappelle-moi", false, voice));
        let shown = q.snapshot("maison");
        assert_eq!(shown["items"][1]["voice"], true);
        assert!(shown["items"][1].get("attachments").is_none());
        // a queue written by a bridge from before reads back without them
        let old: Item = serde_json::from_value(json!({"id": "q1", "session_id": "s1", "text": "x", "created_at": "t"})).unwrap();
        assert!(old.attachments.is_empty() && !old.voice);
    }

    fn queue_of(ids: &[(&str, &str)]) -> AgentQueue {
        AgentQueue { items: ids.iter().map(|(i, s)| item(i, s)).collect(), ..Default::default() }
    }

    #[test]
    fn reordering_puts_the_listed_ones_first() {
        let mut q = queue_of(&[("a", "s"), ("b", "s"), ("c", "s"), ("d", "s")]);
        q.reorder(&["c".into(), "nope".into(), "a".into()]);
        let ids: Vec<&str> = q.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b", "d"]);
    }

    #[test]
    fn a_completed_turn_lets_the_next_one_follow() {
        let mut q = queue_of(&[("a", "s1")]);
        q.runs.insert("s1".into(), "r1".into());
        let (changed, follows) = q.run_ended("r1", Some("s1"), "run.completed", None);
        assert!(!changed && follows && !q.paused);
        assert!(q.runs.is_empty());
        // another conversation's end: nothing follows there
        assert_eq!(q.run_ended("r9", Some("s2"), "run.completed", None), (false, false));
    }

    #[test]
    fn a_stopped_or_failed_turn_pauses_what_waits_in_its_conversation() {
        for kind in ["run.cancelled", "run.failed", "run.interrupted"] {
            let mut q = queue_of(&[("a", "s1")]);
            let (changed, follows) = q.run_ended("r1", Some("s1"), kind, None);
            assert!(changed && !follows && q.paused, "{kind}");
            assert_eq!(q.version, 1);
        }
        // nothing waiting there: no pause
        let mut q = queue_of(&[("a", "s2")]);
        assert_eq!(q.run_ended("r1", Some("s1"), "run.cancelled", None), (false, false));
        assert!(!q.paused);
    }

    #[test]
    fn words_the_turn_did_not_take_come_back_first() {
        let mut q = queue_of(&[("a", "s1")]);
        q.steers.push(Steer { run_id: "r1".into(), text: "et en bio".into(), client_id: Some("c-steer-1".into()) });
        let (changed, follows) = q.run_ended("r1", Some("s1"), "run.completed", Some("et en bio"));
        assert!(changed && follows);
        assert_eq!(q.items.len(), 2);
        assert!(q.items[0].late && q.items[0].text == "et en bio" && q.items[0].session_id == "s1");
        assert_eq!(q.items[0].client_id.as_deref(), Some("c-steer-1"), "still the app's message");
        assert!(q.steers.is_empty());
        // words steered straight at Hermes (not through the bridge) are the app's own to send
        let mut q = AgentQueue::default();
        assert_eq!(q.run_ended("r2", Some("s1"), "run.completed", Some("autre chose")), (false, false));
        assert!(q.items.is_empty());
    }

    #[test]
    fn the_queues_own_turn_ending_is_forgotten() {
        let mut q = queue_of(&[]);
        q.running = Some(Running {
            item_id: "a".into(),
            run_id: "r3".into(),
            session_id: "s1".into(),
            text: "x".into(),
            started_at: "t".into(),
        });
        q.runs.insert("s1".into(), "r3".into());
        let (changed, follows) = q.run_ended("r3", None, "run.completed", None);
        assert!(changed && !follows && q.running.is_none() && q.runs.is_empty());
    }

    #[test]
    fn an_empty_queue_keeps_no_pause() {
        let mut q = queue_of(&[("a", "s1")]);
        q.paused = true;
        q.items.clear();
        q.changed();
        assert!(!q.paused);
    }

    #[test]
    fn a_conversations_snapshot_shows_only_its_items() {
        let q = queue_of(&[("a", "s1"), ("b", "s2")]);
        let one = q.snapshot_of("maison", "s2");
        assert_eq!(one["items"].as_array().unwrap().len(), 1);
        assert_eq!(one["items"][0]["id"], "b");
        assert_eq!(q.snapshot("maison")["items"].as_array().unwrap().len(), 2);
        // what is kept for the bridge only stays out
        assert!(one.get("runs").is_none() && one.get("steers").is_none());
    }

    #[tokio::test]
    async fn kept_on_disk_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config.agents.insert(
            "maison".into(),
            crate::config::AgentConfig {
                name: "maison".into(),
                hermes_url: "http://127.0.0.1:9".into(),
                hermes_key: "x".into(),
                ..Default::default()
            },
        );
        let shared = SharedConfig::new(config.clone());
        let hermes = HermesClient::new(reqwest::Client::new());
        let hub = Arc::new(RunHub::new(hermes.clone(), None, 60, None));
        let queues = Queues::new(shared.clone(), hermes.clone(), hub.clone());
        let agent = config.agents["maison"].clone();
        queues.add(&agent, "s1", "  puis range  ", Some("r1"), false, Sent::default()).unwrap();
        let mut events = queues.subscribe();
        queues.add(&agent, "s1", "et appelle", None, true, Sent::default()).unwrap();
        let (who, kind, data) = events.recv().await.unwrap();
        assert_eq!((who.as_str(), kind.as_str()), ("maison", "queue"));
        assert_eq!(data["items"][0]["text"], "et appelle");
        assert_eq!(data["items"][1]["text"], "puis range");
        let path = dir.path().join("queue").join("maison.json");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        // read again by another bridge (a restart)
        let again = Queues::new(shared, hermes, hub);
        let q = again.get("maison");
        assert_eq!(q.items.len(), 2);
        assert_eq!(q.runs.get("s1").map(String::as_str), Some("r1"));
        assert_eq!(q.version, 2);
        for i in 2..ITEMS_MAX {
            again.add(&agent, "s1", &format!("m{i}"), None, false, Sent::default()).unwrap();
        }
        assert_eq!(again.add(&agent, "s1", "one more", None, false, Sent::default()).unwrap_err(), "full");
    }
}
