//! Conversations owned by the bridge (docs/conversations.md): one stable thread id per conversation of an agent,
//! whatever Hermes does with titles and session ids; a thread is a chain of Hermes sessions (its segments: Hermes
//! goes on in a child session after compressing a long conversation); one timeline the apps draw, with stable item
//! ids, the Boîte's messages and what is still pending (the run going on, the queue, approvals).
//!
//! ```text
//! GET  /v1/threads?agent=[&include_hidden=1]                    {threads: [Thread]}
//! GET  /v1/threads?kind=room                                    {threads: [room thread]} (rooms.rs)
//! GET  /v1/threads/resolve?agent=&session_id= | &c=             {thread, tip}
//! GET  /v1/threads/events[?agent=&thread=]                      SSE: thread, notification, read, share; `thread`: on screen
//! GET  /v1/threads/{thread}[?agent=]                            Thread with its segments (`main` needs agent)
//! PATCH /v1/threads/{thread} {title, agent?}                    renamed
//! DELETE /v1/threads/{thread}[?agent=]                          its sessions deleted (the main thread stays, empty)
//! POST /v1/threads/{thread}/segments {session_id, agent?}       a session appended (migrations)
//! GET  /v1/threads/{thread}/timeline?agent=&limit=&before=&after=&around=&before_count=&after_count=
//! GET  /v1/threads/{thread}/items/{item}
//! GET  /v1/threads/{thread}/attachments/{attachment}
//! POST /v1/threads/{thread}/messages {client_id, text, attachments?…}   the person's message (send.rs)
//! POST /v1/threads/{thread}/stop                                        the turn going on stopped
//! GET  /v1/threads/{thread}/sends/{client_id}?agent=                    a sent message until its reply (follow.rs)
//! ```
//!
//! A shared agent's (`agent=<agent>__<household>`): the list and timeline pages relayed to the owner's bridge
//! (agent_shares.rs).
//!
//! Tables in bridge.db (store.rs): `threads`, `thread_segments`, `thread_sends`. Reads Hermes through
//! [`source::Source`]: the agent's `state.db` (no call to Hermes) or its API (cached).
//!
//! No Python counterpart.

pub mod follow;
pub mod layout;
pub mod send;
pub mod source;
pub mod timeline;
#[cfg(test)]
mod timeline_guard;

use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Digest;
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::HermesClient;
use crate::store::{iso, utcnow, Store};
use crate::web::{self, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use source::{ApiCache, Session, Source};
use timeline::{Item, Want};

/// The title Hermes' Bot Mode gives the agent's permanent conversation: the one constant of the bridge for it (peers.rs
/// keeps Hermes' peer-protocol own). Read to adopt an existing place's main conversation (docs/conversations.md §8),
/// given to the main session the bridge makes (welcome.rs, agents_admin.rs); a thread's identity never depends on it.
pub const LEGACY_MAIN_TITLE: &str = "Bot Chat";
/// A record naming each agent's main session (`{agent: session_id}`), written by a migration that moves the person's
/// conversation to another session (botchat-v0225): read before any title.
pub const MAIN_HINTS: &str = "main-sessions.json";
pub const LIMIT_DEFAULT: usize = 50;
pub const LIMIT_MAX: usize = 500;
/// Sessions read when listing or adopting.
const LISTED: usize = 200;

/// A thread as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Thread {
    pub id: String,
    pub agent: String,
    pub kind: String,
    pub title: Option<String>,
    pub origin: Value,
    pub hidden: bool,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl Thread {
    fn of(row: &crate::store::Row) -> Thread {
        let s = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Thread {
            id: s("id"),
            agent: s("agent"),
            kind: s("kind"),
            title: row.get("title").and_then(Value::as_str).map(str::to_string),
            origin: row
                .get("origin")
                .and_then(Value::as_str)
                .and_then(|o| serde_json::from_str(o).ok())
                .unwrap_or(Value::Null),
            hidden: row.get("hidden").and_then(Value::as_i64).unwrap_or(0) != 0,
            version: row.get("version").and_then(Value::as_i64).unwrap_or(0),
            created_at: s("created_at"),
            updated_at: s("updated_at"),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "agent": self.agent, "kind": self.kind, "title": self.title, "hidden": self.hidden,
               "origin": self.origin, "version": self.version, "created_at": self.created_at,
               "updated_at": self.updated_at})
    }
}

/// A segment of a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub session_id: String,
    pub position: u32,
    pub linked: String,
}

type Param = rusqlite::types::Value;

fn text(v: &str) -> Param {
    Param::Text(v.to_string())
}

/// `t_` + 26 characters, sortable by creation time.
pub fn new_id(prefix: &str) -> String {
    let millis = chrono::Utc::now().timestamp_millis().max(0) as u64;
    format!("{prefix}{millis:013x}{}", &crate::py::uuid4_hex()[..13])
}

/// The legacy main title (case and spaces aside).
fn titled_main(session: &Session) -> bool {
    session.title.as_deref().map(str::trim).is_some_and(|t| t.eq_ignore_ascii_case(LEGACY_MAIN_TITLE))
}

/// A routine's run: `cron_<job>_<date>_<time>`, or a `local-<sha256>` session on v0.22.5 that is not a peer's own
/// « Bot Chat » (patch 0021 makes those `local-…` too).
fn routine_session(session: &Session) -> bool {
    session.id.starts_with("cron_")
        || session.source.as_deref() == Some("cron")
        || (session.id.starts_with("local-") && !titled_main(session))
}

/// The service: the registry, the timelines, the events.
pub struct Threads {
    config: SharedConfig,
    store: Store,
    hermes: HermesClient,
    cache: Arc<ApiCache>,
    /// One adoption or resolution at a time (two requests never make two threads for one session).
    registry: tokio::sync::Mutex<()>,
    events: broadcast::Sender<(String, String, Value)>,
    /// When each thread's line was last looked at through Hermes' API (its continuations): not again before
    /// `source::TIP_TTL` unless the thread was touched. A store read (state.db) costs Hermes nothing: always.
    checked: std::sync::Mutex<std::collections::HashMap<String, f64>>,
    /// Where the Live calls' records are (live_calls.rs).
    briefs: Option<Arc<crate::live_brief::Briefs>>,
    /// The threads on screen: an app holds `/v1/threads/events?thread=<id>` open (presence: no push for them).
    on_screen: std::sync::Mutex<std::collections::HashMap<String, usize>>,
    /// The agents' queues (msg_queue.rs): a message waiting there shows in `pending.queue`, not as a sent item.
    queued: std::sync::OnceLock<Weak<crate::msg_queue::Queues>>,
}

/// One app showing a thread, for as long as its event stream is open.
pub struct OnScreen {
    threads: Arc<Threads>,
    thread: String,
}

impl Drop for OnScreen {
    fn drop(&mut self) {
        let mut shown = self.threads.on_screen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = shown.get_mut(&self.thread) {
            *n -= 1;
            if *n == 0 {
                shown.remove(&self.thread);
            }
        }
    }
}

pub fn threads_of(state: &AppState) -> Arc<Threads> {
    state
        .threads
        .get_or_init(|| {
            let mut threads = Threads::new(state.config.clone(), state.store.clone(), state.hermes.clone());
            threads.briefs = Some(state.briefs.clone());
            let _ = threads.queued.set(Arc::downgrade(&crate::msg_queue::queues_of(state)));
            let threads = Arc::new(threads);
            let _ = state.hub.threads.set(Arc::downgrade(&threads));
            let _ = state.push.threads.set(Arc::downgrade(&threads));
            threads
        })
        .clone()
}

impl Threads {
    pub fn new(config: SharedConfig, store: Store, hermes: HermesClient) -> Threads {
        let (events, _) = broadcast::channel(512);
        Threads {
            config,
            store,
            hermes,
            cache: Arc::default(),
            registry: tokio::sync::Mutex::new(()),
            events,
            checked: Default::default(),
            briefs: None,
            on_screen: Default::default(),
            queued: Default::default(),
        }
    }

    /// The thread is on an app's screen (its event stream open): a notification for it is journaled, read, not
    /// pushed.
    pub fn is_on_screen(&self, thread: &str) -> bool {
        self.on_screen.lock().unwrap_or_else(|e| e.into_inner()).contains_key(thread)
    }

    /// The agent's main thread is on an app's screen (a store read, nothing asked of Hermes): read_shares.rs looks at
    /// a shared project thread's viewers sooner while its owner has it open.
    pub async fn main_on_screen(&self, agent: &str) -> bool {
        if self.on_screen.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            return false;
        }
        let sql = "SELECT * FROM threads WHERE agent = ? AND kind = 'main'";
        let rows = self.store.run(sql, vec![text(agent)]).await.unwrap_or_default();
        rows.first().map(Thread::of).is_some_and(|main| self.is_on_screen(&main.id))
    }

    pub fn show(self: &Arc<Self>, thread: &str) -> OnScreen {
        *self.on_screen.lock().unwrap_or_else(|e| e.into_inner()).entry(thread.to_string()).or_insert(0) += 1;
        OnScreen { threads: self.clone(), thread: thread.to_string() }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String, Value)> {
        self.events.subscribe()
    }

    /// An event for the apps (`thread`, `notification`, `read`, `share`: a read share's people changed, read_shares.rs).
    pub fn publish(&self, agent: &str, kind: &str, data: Value) {
        let _ = self.events.send((agent.to_string(), kind.to_string(), data));
    }

    pub fn source(&self, agent: &AgentConfig) -> Source {
        Source::of(&self.config.get(), agent, &self.hermes, &self.cache)
    }

    fn agent(&self, name: &str) -> Option<AgentConfig> {
        self.config.get().agent(Some(name)).cloned()
    }

    // -- registry -------------------------------------------------------------------------------------------------

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<Thread>> {
        let rows = self.store.run("SELECT * FROM threads WHERE id = ?", vec![text(id)]).await?;
        Ok(rows.first().map(Thread::of))
    }

    pub async fn segments(&self, thread: &str) -> anyhow::Result<Vec<Segment>> {
        let rows = self
            .store
            .run(
                "SELECT session_id, position, linked FROM thread_segments WHERE thread_id = ? ORDER BY position",
                vec![text(thread)],
            )
            .await?;
        Ok(rows
            .iter()
            .map(|r| Segment {
                session_id: r["session_id"].as_str().unwrap_or("").to_string(),
                position: r["position"].as_i64().unwrap_or(0) as u32,
                linked: r["linked"].as_str().unwrap_or("").to_string(),
            })
            .collect())
    }

    /// The thread holding `session` already (None: none yet, or the store unreadable).
    pub async fn thread_of(&self, agent: &AgentConfig, session: &str) -> Option<Thread> {
        let id = self.thread_of_session(&agent.name, session).await.ok().flatten()?;
        self.get(&id).await.ok().flatten()
    }

    async fn thread_of_session(&self, agent: &str, session: &str) -> anyhow::Result<Option<String>> {
        let rows = self
            .store
            .run(
                "SELECT thread_id FROM thread_segments WHERE agent = ? AND session_id = ?",
                vec![text(agent), text(session)],
            )
            .await?;
        Ok(rows.first().and_then(|r| r["thread_id"].as_str()).map(str::to_string))
    }

    async fn create(
        &self,
        agent: &str,
        kind: &str,
        title: Option<&str>,
        origin: Value,
        hidden: bool,
    ) -> anyhow::Result<Thread> {
        let now = iso(utcnow());
        let id = new_id("t_");
        let origin = if origin.is_null() { Param::Null } else { text(&origin.to_string()) };
        let title = title.map(text).unwrap_or(Param::Null);
        self.store
            .run(
                "INSERT INTO threads (id, agent, kind, title, origin, hidden, version, created_at, updated_at) \
                 VALUES (?,?,?,?,?,?,0,?,?)",
                vec![
                    text(&id),
                    text(agent),
                    text(kind),
                    title,
                    origin,
                    Param::Integer(hidden as i64),
                    text(&now),
                    text(&now),
                ],
            )
            .await?;
        Ok(self.get(&id).await?.expect("just inserted"))
    }

    async fn append(&self, agent: &str, thread: &str, session: &str, linked: &str) -> anyhow::Result<()> {
        self.store
            .run(
                "INSERT OR IGNORE INTO thread_segments (agent, session_id, thread_id, position, linked, added_at) \
                 VALUES (?, ?, ?, (SELECT COALESCE(MAX(position) + 1, 0) FROM thread_segments WHERE thread_id = ?), ?, ?)",
                vec![text(agent), text(session), text(thread), text(thread), text(linked), text(&iso(utcnow()))],
            )
            .await?;
        Ok(())
    }

    /// The thread changed: its version grows, the apps are told.
    pub async fn bump(&self, thread: &Thread, reason: &str) -> anyhow::Result<i64> {
        let now = iso(utcnow());
        self.store
            .run(
                "UPDATE threads SET version = version + 1, updated_at = ? WHERE id = ?",
                vec![text(&now), text(&thread.id)],
            )
            .await?;
        let version = self.get(&thread.id).await?.map(|t| t.version).unwrap_or(thread.version + 1);
        self.publish(
            &thread.agent,
            "thread",
            json!({"thread": thread.id, "agent": thread.agent, "version": version, "reason": reason}),
        );
        Ok(version)
    }

    /// The session's whole line: parents up to the root, continuations down to the tip.
    async fn line_of(&self, source: &Source, session: Session) -> anyhow::Result<Vec<Session>> {
        let mut line = vec![session];
        for _ in 0..64 {
            let first = &line[0];
            let Some(parent_id) = first.parent.clone() else { break };
            match source.session(&parent_id).await? {
                Some(parent) if first.continues(&parent) && !line.iter().any(|s| s.id == parent.id) => {
                    line.insert(0, parent)
                }
                _ => break,
            }
        }
        for _ in 0..64 {
            match source.continuation(line.last().expect("not empty")).await? {
                Some(child) if !line.iter().any(|s| s.id == child.id) => line.push(child),
                _ => break,
            }
        }
        Ok(line)
    }

    /// The session the person's main conversation is in now: a migration's record, else the conversation Hermes'
    /// Bot Mode titled (the newest shown one, then a hidden one).
    async fn main_session(&self, agent: &AgentConfig, source: &Source) -> anyhow::Result<Option<Session>> {
        let hints = std::path::Path::new(&self.config.get().data_dir()).join(MAIN_HINTS);
        let hinted = std::fs::read_to_string(hints)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v.get(&agent.name).and_then(Value::as_str).map(str::to_string));
        if let Some(id) = hinted {
            if let Some(session) = source.session(&id).await? {
                return Ok(Some(session));
            }
        }
        let titled: Vec<Session> = source
            .sessions(LISTED)
            .await?
            .into_iter()
            // a peer's own « Bot Chat » (patch 0021: a hidden local- session) is never the person's
            .filter(|s| titled_main(s) && !s.id.starts_with("local-"))
            .collect();
        Ok(titled.iter().find(|s| !s.hidden).or(titled.first()).cloned())
    }

    /// The agent's main thread, adopted from its existing conversation (docs/conversations.md §8): the first time, and
    /// again while it has no segment (a new agent before its first conversation, Hermes having moved it aside).
    pub async fn main(&self, agent: &AgentConfig) -> anyhow::Result<Thread> {
        let found = |rows: Vec<crate::store::Row>| rows.first().map(Thread::of);
        let sql = "SELECT * FROM threads WHERE agent = ? AND kind = 'main'";
        if let Some(main) = found(self.store.run(sql, vec![text(&agent.name)]).await?) {
            if !self.segments(&main.id).await?.is_empty() {
                return Ok(main);
            }
        }
        let _guard = self.registry.lock().await;
        let existing = found(self.store.run(sql, vec![text(&agent.name)]).await?);
        if let Some(main) = &existing {
            if !self.segments(&main.id).await?.is_empty() {
                return Ok(main.clone());
            }
        }
        let source = self.source(agent);
        if let (Some(main), Source::Api(..)) = (&existing, &source) {
            // an empty main thread looked for through Hermes' API at most every TIP_TTL (a store read is free)
            let (now, key) = (crate::py::time_time(), format!("main:{}", agent.name));
            let mut checked = self.checked.lock().unwrap_or_else(|e| e.into_inner());
            if checked.get(&key).is_some_and(|at| now - at < source::TIP_TTL) {
                return Ok(main.clone());
            }
            checked.insert(key, now);
        }
        // Hermes asleep or down: an error, tried again at the next request (nothing half made)
        let session =
            self.main_session(agent, &source).await.map_err(|e| e.context("main conversation not adopted yet"))?;
        let line = match session {
            Some(s) => self.line_of(&source, s).await?,
            None => Vec::new(),
        };
        // a session another thread holds (adopted as a side one before) moves to the main thread
        for s in &line {
            if let Some(other) = self.thread_of_session(&agent.name, &s.id).await? {
                self.store
                    .run(
                        "DELETE FROM thread_segments WHERE agent = ? AND session_id = ?",
                        vec![text(&agent.name), text(&s.id)],
                    )
                    .await?;
                if self.segments(&other).await?.is_empty() {
                    self.store.run("DELETE FROM threads WHERE id = ?", vec![text(&other)]).await?;
                }
            }
        }
        let thread = match existing {
            Some(main) => main,
            None => self.create(&agent.name, "main", None, Value::Null, false).await?,
        };
        for (i, s) in line.iter().enumerate() {
            self.append(&agent.name, &thread.id, &s.id, if i == 0 { "adopted" } else { "compression" }).await?;
        }
        if !line.is_empty() {
            tracing::info!(agent = %agent.name, thread = %thread.id, segments = line.len(), "main thread adopted");
        }
        Ok(thread)
    }

    /// The thread holding a Hermes session (a segment, a continuation of one, or a new thread for a root session).
    pub async fn resolve(&self, agent: &AgentConfig, session_id: &str) -> anyhow::Result<Option<Thread>> {
        let main = self.main(agent).await?;
        if let Some(id) = self.thread_of_session(&agent.name, session_id).await? {
            return self.get(&id).await;
        }
        let _guard = self.registry.lock().await;
        if let Some(id) = self.thread_of_session(&agent.name, session_id).await? {
            return self.get(&id).await;
        }
        let source = self.source(agent);
        let Some(session) = source.session(session_id).await? else { return Ok(None) };
        let line = self.line_of(&source, session.clone()).await?;
        // a line one of whose sessions is known: the rest is appended to that thread
        for s in &line {
            if let Some(id) = self.thread_of_session(&agent.name, &s.id).await? {
                let segments = self.segments(&id).await?;
                for later in line.iter().skip_while(|x| x.id != s.id).skip(1) {
                    if !segments.iter().any(|g| g.session_id == later.id) {
                        self.append(&agent.name, &id, &later.id, "compression").await?;
                    }
                }
                return self.get(&id).await;
            }
        }
        let root = &line[0];
        // the person's conversation appearing while the main thread is still empty: it is the main thread's
        if titled_main(root)
            && !root.hidden
            && !root.id.starts_with("local-")
            && self.segments(&main.id).await?.is_empty()
        {
            for (i, s) in line.iter().enumerate() {
                self.append(&agent.name, &main.id, &s.id, if i == 0 { "adopted" } else { "compression" }).await?;
            }
            return self.get(&main.id).await;
        }
        let (kind, hidden) = if routine_session(root) {
            ("routine", true)
        } else if root.id.starts_with("local-") || (titled_main(root) && root.hidden) {
            ("pair", true) // the agents' own conversation (Bot Mode's peers)
        } else {
            ("side", root.hidden)
        };
        let origin = if kind == "routine" {
            json!({"kind": "routine", "routine": crate::cronwatch::job_id(&root.id)})
        } else {
            Value::Null
        };
        let thread = self.create(&agent.name, kind, root.title.as_deref(), origin, hidden).await?;
        for (i, s) in line.iter().enumerate() {
            self.append(&agent.name, &thread.id, &s.id, if i == 0 { "root" } else { "compression" }).await?;
        }
        Ok(Some(thread))
    }

    /// A thread by id, or `main` for the agent's.
    pub async fn named(&self, agent: Option<&AgentConfig>, id: &str) -> ApiResult<(AgentConfig, Thread)> {
        if id == "main" {
            let agent = agent.ok_or_else(|| HttpError::new(422, "agent is required with main"))?;
            return Ok((agent.clone(), self.main(agent).await.map_err(unavailable)?));
        }
        let thread = self.get(id).await?.ok_or_else(HttpError::not_found)?;
        if agent.is_some_and(|a| a.name != thread.agent) {
            return Err(HttpError::not_found());
        }
        let agent = self.agent(&thread.agent).ok_or_else(|| HttpError::new(404, "unknown agent"))?;
        Ok((agent, thread))
    }

    /// The segments, with the continuations Hermes made since (appended).
    pub async fn line(&self, agent: &AgentConfig, thread: &Thread) -> anyhow::Result<Vec<Segment>> {
        let mut segments = self.segments(&thread.id).await?;
        let Some(tip) = segments.last().cloned() else { return Ok(segments) };
        let source = self.source(agent);
        if matches!(source, Source::Api(..)) {
            let now = crate::py::time_time();
            let mut checked = self.checked.lock().unwrap_or_else(|e| e.into_inner());
            if checked.get(&thread.id).is_some_and(|at| now - at < source::TIP_TTL) {
                return Ok(segments);
            }
            checked.insert(thread.id.clone(), now);
        }
        let Some(mut current) = source.session(&tip.session_id).await? else { return Ok(segments) };
        let mut grew = false;
        for _ in 0..16 {
            match source.continuation(&current).await? {
                Some(child) if !segments.iter().any(|s| s.session_id == child.id) => {
                    self.store
                        .run(
                            "DELETE FROM thread_segments WHERE agent = ? AND session_id = ?",
                            vec![text(&agent.name), text(&child.id)],
                        )
                        .await?;
                    self.append(&agent.name, &thread.id, &child.id, "compression").await?;
                    current = child;
                    grew = true;
                }
                _ => break,
            }
        }
        if grew {
            segments = self.segments(&thread.id).await?;
            source.forget(&tip.session_id);
            self.bump(thread, "segment").await?;
        }
        Ok(segments)
    }

    /// A conversation the bridge made for work of its own (a routine's run: routines.rs) adopted as a hidden thread of
    /// `kind`, named `title` (the run's screen says what it is): its id (None: not adopted). A session another thread took first (a list or an event resolving it as a
    /// side one) moves here, so the work never shows in the person's conversations.
    pub async fn adopt(&self, agent: &AgentConfig, session: &str, title: &str, kind: &str, origin: Value) -> Option<String> {
        let _guard = self.registry.lock().await;
        let made = async {
            if let Some(id) = self.thread_of_session(&agent.name, session).await? {
                match self.get(&id).await? {
                    Some(held) if held.kind == kind && held.hidden => return Ok(id),
                    held => {
                        tracing::warn!(agent = %agent.name, session = %session, thread = %id,
                            kind = %held.as_ref().map_or("", |t| t.kind.as_str()), "work conversation taken by another thread: moved");
                        self.store
                            .run("DELETE FROM thread_segments WHERE agent = ? AND session_id = ?",
                                 vec![text(&agent.name), text(session)])
                            .await?;
                        if let Some(held) = held.filter(|t| t.kind != "main") {
                            if self.segments(&held.id).await?.is_empty() {
                                self.store.run("DELETE FROM threads WHERE id = ?", vec![text(&held.id)]).await?;
                            }
                        }
                    }
                }
            }
            let thread = self.create(&agent.name, kind, Some(title).filter(|t| !t.is_empty()), origin, true).await?;
            self.append(&agent.name, &thread.id, session, "root").await?;
            anyhow::Ok(thread.id)
        };
        match made.await {
            Ok(id) => Some(id),
            Err(exc) => {
                tracing::warn!(agent = %agent.name, error = %format!("{exc:#}"), "work conversation not adopted");
                None
            }
        }
    }

    /// Appends a session as the thread's next segment (a migration moving the conversation, a test).
    pub async fn link(&self, agent: &AgentConfig, thread: &Thread, session: &str) -> anyhow::Result<()> {
        let _guard = self.registry.lock().await;
        self.store
            .run(
                "DELETE FROM thread_segments WHERE agent = ? AND session_id = ?",
                vec![text(&agent.name), text(session)],
            )
            .await?;
        let linked = if self.segments(&thread.id).await?.is_empty() { "root" } else { "moved" };
        self.append(&agent.name, &thread.id, session, linked).await?;
        self.bump(thread, "segment").await?;
        Ok(())
    }

    /// Something happened in this session (a run ended, a Boîte message, a notification): its thread's version
    /// grows and the apps are told. Quiet when the session is no thread's yet.
    pub async fn touch(&self, agent: &str, session: Option<&str>, reason: &str) {
        let Some(cfg) = self.agent(agent) else { return };
        let thread = match session.filter(|s| !s.is_empty()) {
            Some(s) => {
                self.source(&cfg).forget(s);
                match self.thread_of_session(agent, s).await {
                    Ok(Some(id)) => self.get(&id).await.ok().flatten(),
                    _ => self.resolve(&cfg, s).await.ok().flatten(),
                }
            }
            None => self.main(&cfg).await.ok(),
        };
        if let Some(thread) = thread {
            self.checked.lock().unwrap_or_else(|e| e.into_inner()).remove(&thread.id);
            if let Err(exc) = self.bump(&thread, reason).await {
                tracing::warn!(agent = %agent, error = %format!("{exc:#}"), "thread not bumped");
            }
        }
    }

    /// The agent's task list in a run of this session changed (steps.rs): the thread's open screens are told, with the
    /// list, without a new version (nothing new to fetch; the stored item comes with the turn's rows). Apps from
    /// before 6/11 skip a hint whose version they hold.
    pub async fn steps_changed(&self, agent: &AgentConfig, session: &str, run_id: &str, steps: Value) {
        let Some(thread) = self.thread_of(agent, session).await else { return };
        self.publish(
            &thread.agent,
            "thread",
            json!({"thread": thread.id, "agent": thread.agent, "version": thread.version, "reason": "steps",
                   "run_id": run_id, "steps": steps}),
        );
    }

    // -- timeline -------------------------------------------------------------------------------------------------

    /// Every item of the thread, in order: its segments' rows, the Boîte's messages among them.
    pub async fn items(&self, agent: &AgentConfig, thread: &Thread, segments: &[Segment]) -> anyhow::Result<Vec<Item>> {
        let source = self.source(agent);
        let mut items = Vec::new();
        let mut turn = String::new();
        let mut previous: Option<Arc<Vec<source::Row>>> = None;
        for (i, segment) in segments.iter().enumerate() {
            let rows = source.rows(&segment.session_id, i + 1 == segments.len()).await?;
            let skip = match (&previous, segment.linked.as_str()) {
                (Some(parent), "compression") => timeline::copied_tail(&agent.name, &segment.session_id, &rows, parent),
                _ => HashSet::new(),
            };
            items.extend(timeline::segment_items(
                &agent.name,
                segment.position,
                &segment.session_id,
                &rows,
                &skip,
                &mut turn,
            ));
            previous = Some(rows);
        }
        with_services(&self.config.get(), &agent.name, &mut items);
        crate::steps::fold(&mut items, &crate::handback::language_of(&self.config.get(), agent));
        if thread.kind != "main" {
            // a run without the person that had nothing to tell (« [SILENT] », or the code word translated): no reply
            items.retain(|i| !timeline::silent_reply(i));
        }
        let mut extra = self.boite(agent, thread, segments).await?;
        extra.extend(self.kept(agent, segments).await?);
        let mut items = timeline::merge(items, extra);
        let queued: Vec<String> = self.queued.get().and_then(Weak::upgrade).map_or_else(Vec::new, |q| {
            q.get(&agent.name).items.into_iter().filter_map(|i| i.client_id).collect()
        });
        self.with_sends(agent, thread, &mut items, &queued).await?;
        self.with_files(agent, thread, &mut items).await?;
        with_maps_once(self.config.get(), agent, &mut items).await;
        without_missing_pages(self.config.get(), &mut items).await;
        with_document_sizes(agent, &mut items).await;
        Ok(items)
    }

    /// The files kept for the agent's messages (`attach`): added to those its `MEDIA:` lines give, stale ones left
    /// out, numbered after them.
    async fn with_files(&self, agent: &AgentConfig, thread: &Thread, items: &mut [Item]) -> anyhow::Result<()> {
        let sql = "SELECT message_id, files FROM thread_files WHERE agent = ? AND thread_id = ?";
        let rows = self.store.run(sql, vec![text(&agent.name), text(&thread.id)]).await?;
        for row in &rows {
            let Some(item) = items.iter_mut().find(|i| row["message_id"].as_str() == Some(i.id.as_str())) else { continue };
            let Some(Value::Array(kept)) = row["files"].as_str().and_then(|f| serde_json::from_str(f).ok()) else { continue };
            let mut atts = match item.body.get("attachments") {
                Some(Value::Array(own)) => own.clone(),
                _ => Vec::new(),
            };
            for a in kept {
                if a["stale"] == json!(true) {
                    atts.retain(|o| o["path"] != a["path"]);
                } else if !atts.iter().any(|o| o["path"] == a["path"]) {
                    atts.push(a);
                }
            }
            for (n, a) in atts.iter_mut().enumerate() {
                a["id"] = json!(format!("{}.a{n}", item.id));
            }
            if atts.is_empty() {
                item.body.remove("attachments");
            } else {
                item.body.insert("attachments".into(), Value::Array(atts));
            }
        }
        Ok(())
    }

    /// Files a background turn made (background_turn.rs), attached to the agent's message `message` whatever its words
    /// say: the timeline gives them as its attachments (`kind` from the path, as a `MEDIA:` line's).
    pub async fn attach(&self, agent: &AgentConfig, thread: &str, message: &str, files: &[String]) {
        self.attach_with(agent, thread, message, files, &[]).await
    }

    /// [`Self::attach`], and the `MEDIA:` lines of `stale` files left out of the message's attachments (an older
    /// podcast's path the model wrote from memory, an earlier version of a file it made again: made_files.rs).
    pub async fn attach_with(&self, agent: &AgentConfig, thread: &str, message: &str, files: &[String], stale: &[String]) {
        let mut atts: Vec<Value> = files.iter().map(|p| timeline::file_attachment(p, None)).collect();
        atts.extend(stale.iter().map(|p| json!({"path": p, "stale": true})));
        let sql = "INSERT INTO thread_files (agent, message_id, thread_id, files, created_at) VALUES (?, ?, ?, ?, ?) \
                   ON CONFLICT (agent, message_id) DO UPDATE SET files = excluded.files";
        let params = vec![text(&agent.name), text(message), text(thread), text(&Value::Array(atts).to_string()),
                          text(&iso(utcnow()))];
        if let Err(exc) = self.store.run(sql, params).await {
            tracing::warn!(agent = %agent.name, error = %format!("{exc:#}"), "files not attached to their message");
        }
    }

    /// The Boîte's messages that belong to this thread: tagged with one of its sessions; for the main thread also
    /// those tagged with nothing or with a routine's run (a report without a conversation of its own).
    async fn boite(&self, agent: &AgentConfig, thread: &Thread, segments: &[Segment]) -> anyhow::Result<Vec<Item>> {
        let rows = self
            .store
            .run("SELECT id, title, text, created_at, session_id, audio_path, attachments FROM outbox WHERE agent = ? \
                  ORDER BY created_at",
                 vec![text(&agent.name)])
            .await?;
        let sessions: HashSet<&str> = segments.iter().map(|s| s.session_id.as_str()).collect();
        let mut out = Vec::new();
        for row in &rows {
            let session = row["session_id"].as_str().filter(|s| !s.is_empty());
            let mine = match session {
                Some(s) if sessions.contains(s) => true,
                Some(s) if thread.kind == "main" => {
                    // a report tagged with a routine's run (no conversation of its own) or a session Hermes forgot
                    let holder = match self.thread_of_session(&agent.name, s).await? {
                        Some(id) => self.get(&id).await?,
                        None => self.resolve(agent, s).await.unwrap_or(None),
                    };
                    holder.is_none_or(|h| h.kind == "routine")
                }
                None => thread.kind == "main",
                _ => false,
            };
            if !mine {
                continue;
            }
            let id = format!("o_{}", row["id"].as_str().unwrap_or(""));
            let at = row["created_at"]
                .as_str()
                .and_then(|c| crate::store::parse_since(c).ok())
                .map(|d| d.timestamp_micros() as f64 / 1e6)
                .unwrap_or(0.0);
            let (words, mut attachments) = timeline::visible(row["text"].as_str().unwrap_or(""), &id, 0);
            if let Some(Value::Array(kept)) = row["attachments"].as_str().and_then(|a| serde_json::from_str(a).ok()) {
                attachments.extend(kept);
            }
            for (n, a) in attachments.iter_mut().enumerate() {
                a["id"] = json!(format!("{id}.a{n}"));
            }
            let body = json!({"kind": "boite", "text": words, "title": row["title"], "attachments": attachments,
                              "boite": {"outbox_id": row["id"], "audio": !row["audio_path"].is_null()}});
            out.push(Item {
                id,
                pos: (0, 0, 0),
                at,
                turn: String::new(),
                segment: session.unwrap_or("").to_string(),
                row: 0,
                body: body.as_object().cloned().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// What the bridge keeps for the thread besides Hermes: the approvals answered (approvals_log.rs: Hermes' history
    /// has none) and the Live calls' records (live_calls.rs), each an item at its time.
    async fn kept(&self, agent: &AgentConfig, segments: &[Segment]) -> anyhow::Result<Vec<Item>> {
        let (config, name) = (self.config.get(), agent.name.clone());
        let sessions: Vec<String> = segments.iter().map(|s| s.session_id.clone()).collect();
        let calls = self.briefs.as_ref().map(|b| crate::live_calls::LiveCalls::of(b));
        let found = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for session in &sessions {
                for entry in crate::approvals_log::list(&config, &name, Some(session)) {
                    let at = entry.get("at").and_then(Value::as_f64).unwrap_or(0.0);
                    let key = (entry.get("run_id").and_then(Value::as_str).unwrap_or(""),
                               entry.get("request_id").and_then(Value::as_str).unwrap_or(""));
                    let mut body = serde_json::Map::new();
                    body.insert("kind".into(), json!("approval"));
                    body.insert("approval".into(), json!({
                        "run_id": entry.get("run_id"), "request_id": entry.get("request_id"), "command": entry.get("command"),
                        "description": entry.get("description"), "topic": entry.get("topic"),
                        "choice": entry.get("choice"), "state": "answered"}));
                    out.push((answered_id(Some(key.0), Some(key.1)), at, session.clone(), body));
                }
                for record in calls.as_ref().map(|c| c.records(&name, session)).unwrap_or_default() {
                    let Some(id) = record.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
                    let at = record.get("started_at").and_then(Value::as_f64).unwrap_or(0.0);
                    let mut call = record.clone();
                    if let Some(m) = call.as_object_mut() {
                        m.remove("recapped");
                    }
                    let mut body = serde_json::Map::new();
                    body.insert("kind".into(), json!("call"));
                    body.insert("call".into(), call);
                    out.push((format!("c_{id}"), at, session.clone(), body));
                }
            }
            out
        })
        .await?;
        Ok(found
            .into_iter()
            .map(|(id, at, segment, body)| Item { id, pos: (0, 0, 0), at, turn: String::new(), segment, row: 0, body })
            .collect())
    }

    /// The message a notification is about, once stored: the last agent message of the thread whose words are the
    /// reply's (or start like them), else, without words, the last agent message. Waits `wait` seconds at most.
    pub async fn find_message(
        &self,
        agent: &AgentConfig,
        session: &str,
        words: Option<&str>,
        wait: f64,
    ) -> Option<(String, String)> {
        let thread = self.resolve(agent, session).await.ok().flatten()?;
        let wanted = words.map(|w| timeline::visible(w, "x", 0).0).filter(|w| !w.is_empty());
        let head: Option<String> = wanted.as_ref().map(|w| w.chars().take(40).collect());
        let deadline = std::time::Instant::now() + Duration::from_secs_f64(wait.max(0.0));
        loop {
            let segments = self.line(agent, &thread).await.ok()?;
            if let Some(tip) = segments.last() {
                self.source(agent).forget(&tip.session_id);
            }
            if let Ok(items) = self.items(agent, &thread, &segments).await {
                let found = items
                    .iter()
                    .rev()
                    .filter(|i| i.kind() == "message" && i.body.get("role") == Some(&json!("agent")))
                    .find(|i| match (&wanted, &head) {
                        (Some(w), Some(h)) => i.text() == w || i.text().starts_with(h.as_str()),
                        _ => true,
                    });
                if let Some(item) = found {
                    return Some((thread.id.clone(), item.id.clone()));
                }
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
}

/// The item an approval (or a question) becomes once answered: the same id while it waits (`pending`), so a card is
/// one row on every device, answered in place.
pub fn answered_id(run_id: Option<&str>, request_id: Option<&str>) -> String {
    let key = format!("{}/{}", run_id.unwrap_or(""), request_id.unwrap_or(""));
    format!("a_{}", &hex::encode(sha2::Sha256::digest(key.as_bytes()))[..24])
}

/// Hermes not reachable while adopting: the apps try again (503), nothing half made.
fn unavailable(exc: anyhow::Error) -> HttpError {
    tracing::warn!(error = %format!("{exc:#}"), "thread not available");
    HttpError::new(503, "the agent's conversations can't be read right now")
}

// -- routes -----------------------------------------------------------------------------------------------------------

/// A shared agent's (`<agent>__<household>`): relayed to the owner's bridge (agent_shares.rs).
async fn shared(state: &AppState, q: &QueryMap, what: &str) -> Option<ApiResult<Response>> {
    let agent = q.get("agent")?;
    crate::agent_shares::relay_thread(state, agent, what, q).await
}

fn agent_param(state: &AppState, q: &QueryMap) -> ApiResult<Option<AgentConfig>> {
    agent_param_of(state, q.get("agent"))
}

fn number(q: &QueryMap, key: &str, default: usize) -> ApiResult<usize> {
    match q.get(key) {
        None | Some("") => Ok(default),
        Some(v) => v.parse::<usize>().map_err(|_| HttpError::new(422, format!("{key}: a number"))),
    }
}

/// The thread as served, with its tip, segments and last message.
async fn served(threads: &Threads, agent: &AgentConfig, thread: &Thread, with_segments: bool) -> anyhow::Result<Value> {
    let segments = threads.line(agent, thread).await?;
    let mut out = thread.to_json();
    out["tip"] = json!(segments.last().map(|s| s.session_id.clone()));
    if with_segments {
        out["segments"] =
            json!(segments.iter().map(|s| json!({"session_id": s.session_id, "linked": s.linked})).collect::<Vec<_>>());
    }
    out["unread"] = json!(crate::notifications::unread_in(&threads.store, &thread.id).await.unwrap_or(0));
    Ok(out)
}

/// A room as a thread, the room itself under `room` and its unread replies.
async fn room_served(state: &AppState, room: &crate::rooms::Room) -> Value {
    let mut out = crate::rooms::thread_view(room);
    out["room"] = crate::rooms::room_view(&state.config(), room, Some(&*state.hub));
    out["unread"] = json!(crate::notifications::unread_in(&state.store, &crate::rooms::thread_id(&room.id)).await.unwrap_or(0));
    out
}

async fn list(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    if q.get("kind") == Some("room") {
        let mut out = Vec::new();
        for room in crate::rooms::all(&state.config()) {
            out.push(room_served(&state, &room).await);
        }
        return Ok(web::ok(json!({"threads": out})));
    }
    if let Some(relayed) = shared(&state, &q, "").await {
        return relayed;
    }
    let agent = agent_param(&state, &q)?.ok_or_else(|| HttpError::new(422, "agent is required"))?;
    let threads = threads_of(&state);
    let include_hidden = q.get("include_hidden").is_some_and(|v| v == "1" || v == "true");
    let main = threads.main(&agent).await.map_err(unavailable)?;
    // every session Hermes keeps (not only the last 200): each root one is a thread (adopted at first sight)
    let sessions = threads.source(&agent).all_sessions().await.map_err(unavailable)?;
    let mut last_active: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for session in &sessions {
        let thread = match threads.thread_of_session(&agent.name, &session.id).await? {
            Some(id) => threads.get(&id).await?,
            None if session.parent.is_none() => threads.resolve(&agent, &session.id).await?,
            None => None,
        };
        if let Some(t) = thread {
            let entry = last_active.entry(t.id).or_insert(0.0);
            *entry = entry.max(session.last_active);
        }
    }
    let rows = threads.store.run("SELECT * FROM threads WHERE agent = ?", vec![text(&agent.name)]).await?;
    // only what Hermes still keeps (a Live call's throwaway session, a deleted conversation: gone from the list)
    let mut all: Vec<Thread> = rows
        .iter()
        .map(Thread::of)
        .filter(|t| (include_hidden || !t.hidden) && (t.id == main.id || last_active.contains_key(&t.id)))
        .collect();
    all.sort_by(|a, b| {
        let key = |t: &Thread| (t.id != main.id, -last_active.get(&t.id).copied().unwrap_or(0.0) as i64);
        key(a).cmp(&key(b))
    });
    let mut out = Vec::new();
    for t in &all {
        let mut v = t.to_json();
        let segs = threads.segments(&t.id).await?;
        v["tip"] = json!(segs.last().map(|s| s.session_id.clone()));
        v["last_active"] = json!(last_active.get(&t.id));
        v["unread"] = json!(crate::notifications::unread_in(&threads.store, &t.id).await.unwrap_or(0));
        out.push(v);
    }
    Ok(web::ok(json!({"threads": out})))
}

async fn one(State(state): State<AppState>, UrlPath(id): UrlPath<String>, q: QueryMap) -> ApiResult<Response> {
    if crate::rooms::of_thread(&id).is_some() {
        return Ok(web::ok(room_served(&state, &room_of(&state, &id)?).await));
    }
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    Ok(web::ok(served(&threads, &agent, &thread, true).await.map_err(unavailable)?))
}

async fn resolve(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = agent_param(&state, &q)?.ok_or_else(|| HttpError::new(422, "agent is required"))?;
    let threads = threads_of(&state);
    let thread = match (q.get("session_id").filter(|s| !s.is_empty()), q.get("c").filter(|s| !s.is_empty())) {
        (Some(session), _) => {
            threads.resolve(&agent, session).await.map_err(unavailable)?.ok_or_else(HttpError::not_found)?
        }
        (None, Some(c)) => threads.named(Some(&agent), c).await?.1,
        _ => return Err(HttpError::new(422, "session_id or c is required")),
    };
    let out = served(&threads, &agent, &thread, true).await.map_err(unavailable)?;
    let tip = out["tip"].clone();
    Ok(web::ok(json!({"thread": out, "tip": tip})))
}

#[derive(Debug, Clone, Deserialize)]
pub struct LinkBody {
    pub session_id: String,
    #[serde(default)]
    pub agent: Option<String>,
}

impl Validate for LinkBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !crate::app::SESSION_ID.is_match(&self.session_id) {
            errors.push(FieldError::new(
                "value_error",
                vec![json!("body"), json!("session_id")],
                "invalid session id",
                json!(self.session_id),
            ));
        }
    }
}

async fn link(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<LinkBody>,
) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param_of(&state, body.agent.as_deref())?.as_ref(), &id).await?;
    threads.link(&agent, &thread, &body.session_id).await?;
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    Ok(web::ok(served(&threads, &agent, &thread, true).await.map_err(unavailable)?))
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
}

impl Validate for ThreadPatch {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if self.title.as_ref().is_some_and(|t| t.chars().count() > 200) {
            errors.push(FieldError::new(
                "too_long",
                vec![json!("body"), json!("title")],
                "200 characters at most",
                Value::Null,
            ));
        }
    }
}

/// Renames a thread: the bridge's title (the main thread's Hermes session keeps its own, Bot Mode reads it).
async fn rename(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<ThreadPatch>,
) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param_of(&state, body.agent.as_deref())?.as_ref(), &id).await?;
    let title = body.title.as_deref().map(str::trim).filter(|t| !t.is_empty());
    threads
        .store
        .run(
            "UPDATE threads SET title = ? WHERE id = ?",
            vec![title.map(text).unwrap_or(Param::Null), text(&thread.id)],
        )
        .await?;
    if thread.kind != "main" {
        // the Historique's other readers (Hermes' own list, an older tool) see the same name
        if let (Some(tip), Some(t)) = (threads.segments(&thread.id).await?.last(), title) {
            let mut fields = serde_json::Map::new();
            fields.insert("title".into(), json!(t));
            let _ = threads.hermes.update_session(&agent, &tip.session_id, fields).await;
        }
    }
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    threads.bump(&thread, "title").await?;
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    Ok(web::ok(served(&threads, &agent, &thread, true).await.map_err(unavailable)?))
}

/// Deletes a thread: its Hermes sessions and its journal rows. The main thread keeps its id and starts again empty
/// (its next conversation becomes its first segment).
async fn delete_thread(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    q: QueryMap,
) -> ApiResult<Response> {
    if let Some(relayed) = shared(&state, &q, &format!("{id}:delete")).await {
        return relayed;
    }
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let _guard = threads.registry.lock().await;
    for segment in threads.segments(&thread.id).await? {
        match threads.hermes.delete_session(&agent, &segment.session_id).await {
            Ok(()) | Err(crate::hermes::HermesError::Http { status: 404, .. }) => {}
            Err(exc) => return Err(HttpError::new(502, format!("Hermes refused the deletion: {}", exc.type_name()))),
        }
    }
    threads.store.run("DELETE FROM thread_segments WHERE thread_id = ?", vec![text(&thread.id)]).await?;
    threads.store.run("DELETE FROM notifications WHERE thread_id = ?", vec![text(&thread.id)]).await?;
    // its messages sent and never stored go with it: kept, they came back as « Pas envoyé » in the main thread
    // started again under the same id (6/11, release journeys)
    threads.store.run("DELETE FROM thread_sends WHERE thread_id = ?", vec![text(&thread.id)]).await?;
    if thread.kind != "main" {
        threads.store.run("DELETE FROM threads WHERE id = ?", vec![text(&thread.id)]).await?;
    }
    threads.publish(&agent.name, "thread", json!({"thread": thread.id, "agent": agent.name, "reason": "deleted"}));
    tracing::info!(agent = %agent.name, thread = %thread.id, kind = %thread.kind, "thread deleted");
    Ok(web::no_content())
}

/// A timeline request's page (`limit`, `before`, `after`, `around`, `before_count`, `after_count`).
pub fn want_of(q: &QueryMap) -> ApiResult<Want> {
    Ok(Want {
        chosen: q.get("limit").is_none_or(|v| v.is_empty()),
        limit: number(q, "limit", LIMIT_DEFAULT)?.clamp(1, LIMIT_MAX),
        before: q.get("before").filter(|v| !v.is_empty()).map(str::to_string),
        after: q.get("after").filter(|v| !v.is_empty()).map(str::to_string),
        around: q.get("around").filter(|v| !v.is_empty()).map(str::to_string),
        before_count: number(q, "before_count", 20)?.min(LIMIT_MAX),
        after_count: match q.get("after_count") {
            None | Some("") | Some("all") => None,
            Some(_) => Some(number(q, "after_count", 0)?),
        },
    })
}

async fn timeline_route(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    q: QueryMap,
) -> ApiResult<Response> {
    if crate::rooms::of_thread(&id).is_some() {
        let room = room_of(&state, &id)?;
        return Ok(web::ok(crate::rooms::timeline(&state.config(), &room, Some(&*state.hub), &want_of(&q)?)));
    }
    if let Some(relayed) = shared(&state, &q, &format!("{id}/timeline")).await {
        return relayed;
    }
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let want = want_of(&q)?;
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    // the day dividers where the phone is (`tz`, its IANA zone), else where the person said they live
    let zone =
        q.get("tz").and_then(crate::dates::parse_zone).unwrap_or_else(|| crate::dates::load(&state.config()).zone());
    let days = timeline::days(&items, zone);
    let Waiting { tip, approvals, questions, run } = waiting_here(&state, &agent, &thread, &segments, &items).await;
    let (page, open) = conversation_page(items, &want, &days, run.is_some());
    let queue =
        tip.as_deref().map(|t| crate::msg_queue::queues_of(&state).get(&agent.name).snapshot_of(&agent.name, t));
    let mut out_thread = thread.to_json();
    out_thread["tip"] = json!(tip);
    // `started`: the working line's clock on every phone, also on one that reopens the conversation mid-turn
    let run = run.map(|(r, started)| json!({"run_id": r, "status": "running", "started": started}));
    let pending = json!({"run": run, "queue": queue,
                         "approvals": approvals, "questions": questions});
    let mut answer = page.answer(out_thread, thread.version, pending, q.get("debug").is_some_and(|d| d == "1"));
    answer["layout"] = json!({"open": open});
    Ok(web::ok(answer))
}

/// A conversation's page as `timeline_route` serves it, and its `layout.open`: the items `want` asks for among the
/// thread's (`items`), dated (`days`, of the whole thread), laid out; `running`: a turn goes on in the conversation.
/// Pure: the guard (`timeline_guard.rs`) reads every small conversation every way through it (7/11: a page built here
/// was a 500 that neither the unit tests nor the black-box suite had asked for).
fn conversation_page(
    mut items: Vec<Item>,
    want: &Want,
    days: &[Option<(chrono::NaiveDate, bool)>],
    running: bool,
) -> (timeline::Page, Option<String>) {
    // how each turn that is over lays out (its work, its reply, the app's first word, exchanges), decided once for every
    // app on the whole thread (thin apps, brick 7); `layout.open`: the first item of the turn going on, undecided
    let shown = timeline::range(&items, want);
    let decided = layout::layout(&items, shown.clone(), running);
    let open = open_on_page(&items, decided.open, &shown);
    for (n, fields) in decided.fields {
        items[n].body.insert("layout".into(), Value::Object(fields));
    }
    let page = timeline::page(items, want).with_days(days);
    // a sent message no turn took (nothing runs here now, Hermes never stored it): « Pas envoyé », not « being sent »
    let page = if running { page } else { page.with_lost() };
    (page, open)
}

/// The page's `layout.open`: the first item of the turn going on, named among the page's items (the page's first when
/// the turn began before it), none when it begins after the page or when the page holds no item. A catch-up with
/// nothing new while a turn goes on (`after=` its last item) is such a page: it was a 500 (7/11, release journeys
/// J13 and J19: the iPhone, catching up on a card placed with no item, or reopened on its copy mid-turn, lost the
/// card, the run going on and its « Arrêter », and said « Ton ordinateur a eu un souci »).
fn open_on_page(items: &[Item], open: Option<usize>, shown: &std::ops::Range<usize>) -> Option<String> {
    open.filter(|n| !shown.is_empty() && *n < shown.end)
        .and_then(|n| items.get(n.max(shown.start)))
        .map(|i| i.id.clone())
}

/// What waits for the person in a conversation: its tip, its cards placed among its items (`after`), the run going on
/// there (the timeline's `pending`; a sent message's follower, follow.rs).
struct Waiting {
    tip: Option<String>,
    approvals: Vec<Value>,
    questions: Vec<Value>,
    run: Option<(String, f64)>,
}

async fn waiting_here(state: &AppState, agent: &AgentConfig, thread: &Thread, segments: &[Segment], items: &[Item]) -> Waiting {
    let tip = segments.last().map(|s| s.session_id.clone());
    let sessions: HashSet<&str> = segments.iter().map(|s| s.session_id.as_str()).collect();
    // what waits for the person here, each with the id of the item it becomes once answered (one card on every device)
    let waiting = |all: Vec<crate::store::Row>| -> Vec<Value> {
        all.into_iter()
            .filter(|a| a.get("session_id").and_then(Value::as_str).is_some_and(|s| sessions.contains(s)))
            .map(|mut a| {
                let id = answered_id(a.get("run_id").and_then(Value::as_str), a.get("request_id").and_then(Value::as_str));
                a.insert("id".into(), json!(id));
                Value::Object(a)
            })
            .collect()
    };
    let main = thread.kind == "main";
    let mut asked = Vec::new();
    let mut elsewhere = Vec::new();
    for approval in state.hub.pending_approvals(Some(&agent.name)) {
        let session = approval.get("session_id").and_then(Value::as_str).unwrap_or("").to_string();
        if session.is_empty() || sessions.contains(session.as_str()) {
            asked.push(approval);
            continue;
        }
        // a background task's or a routine's (its own hidden conversation): also where it reports (card_place.rs)
        let report = crate::card_place::report_of(state, agent, &session).await;
        let place = crate::card_place::Place { session: Some(session.clone()), report, ..Default::default() };
        if place.report.as_deref() != Some(session.as_str()) && place.shows_in(&sessions, main) {
            elsewhere.push(approval);
        }
    }
    // the bridge's own cards made in this conversation's turns, or by work that reports here (card_place.rs)
    elsewhere.extend(crate::card_place::cards_in(state, &agent.name, &sessions, main));
    let mut approvals = waiting(asked);
    approvals.extend(elsewhere.into_iter().map(|mut a| {
        let id = answered_id(a.get("run_id").and_then(Value::as_str), a.get("request_id").and_then(Value::as_str));
        a.insert("id".into(), json!(id));
        a.insert("session_id".into(), json!(tip)); // shown here, at the conversation's end
        Value::Object(a)
    }));
    let mut questions = waiting(state.hub.pending_questions(&agent.name));
    // where each waiting card goes among the items (`after`), decided once for every app (thin apps, brick 5)
    timeline::place_waiting(items, &mut approvals);
    timeline::place_waiting(items, &mut questions);
    let run = tip.as_deref().and_then(|t| state.hub.session_run_since(&agent.name, t));
    Waiting { tip, approvals, questions, run }
}

/// `GET /v1/threads/{thread}/sends/{client_id}`: where a sent message is, until its reply (follow.rs: the quick talk).
async fn sends_route(
    State(state): State<AppState>,
    UrlPath((id, client_id)): UrlPath<(String, String)>,
    q: QueryMap,
) -> ApiResult<Response> {
    if !send::CLIENT_ID.is_match(&client_id) {
        return Err(HttpError::new(422, "client_id: 8 to 64 letters, digits, - or _"));
    }
    if let Some(relayed) = shared(&state, &q, &format!("{id}/sends/{client_id}")).await {
        return relayed;
    }
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    let Waiting { approvals, questions, run, .. } = waiting_here(&state, &agent, &thread, &segments, &items).await;
    let live = run.and_then(|(run_id, _)| {
        let (tool, answer) = state.hub.glance(&agent.name, &run_id)?;
        Some(follow::Live { run_id, tool, answer })
    });
    let queued = crate::msg_queue::queues_of(&state)
        .get(&agent.name)
        .items
        .iter()
        .any(|i| i.client_id.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&client_id)));
    Ok(web::ok(follow::state_of(&items, &approvals, &questions, live.as_ref(), queued, &client_id, &thread.id)))
}

/// A thread's items and the agent whose files they are: a conversation's, or a room's (`None`: each item's `from`).
async fn all_items(state: &AppState, id: &str, q: &QueryMap) -> ApiResult<(Option<AgentConfig>, String, Vec<Item>)> {
    if crate::rooms::of_thread(id).is_some() {
        return Ok((None, id.to_string(), room_of(state, id)?.messages.iter().map(crate::rooms::item).collect()));
    }
    let threads = threads_of(state);
    let (agent, thread) = threads.named(agent_param(state, q)?.as_ref(), id).await?;
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    Ok((Some(agent), thread.id, items))
}

async fn item_route(
    State(state): State<AppState>,
    UrlPath((id, item)): UrlPath<(String, String)>,
    q: QueryMap,
) -> ApiResult<Response> {
    let (_, thread, items) = all_items(&state, &id, &q).await?;
    let found = items.iter().find(|i| i.id == item).ok_or_else(HttpError::not_found)?;
    Ok(web::ok(found.to_json(&thread, q.get("debug").is_some_and(|d| d == "1"))))
}

/// A connector's calls with their connector's name (`tool.service`, connectors::service_in): the apps and the
/// automatic « Étapes » say it instead of the tool's own name. A call through the tool search by the call it wraps.
fn with_services(config: &crate::config::Config, agent: &str, items: &mut [Item]) {
    let mut connectors = None;
    for item in items.iter_mut().filter(|i| i.kind() == "tool") {
        let Some(tool) = item.body.get_mut("tool").and_then(Value::as_object_mut) else { continue };
        let mut name = tool.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        if name == "tool_call" {
            let args = tool.get("arguments").and_then(Value::as_str).unwrap_or("");
            let inner = serde_json::from_str::<Value>(args).ok().and_then(|a| {
                let call = a.get("calls").and_then(|c| c.get(0)).cloned().unwrap_or(a);
                call.get("name").and_then(Value::as_str).map(str::to_string)
            });
            name = inner.unwrap_or(name);
        }
        if crate::connectors::slug_of_tool(&name).is_none() {
            continue;
        }
        let list = connectors.get_or_insert_with(|| crate::connectors::of_agent(config, agent));
        if let Some(service) = crate::connectors::service_in(list, &name) {
            tool.insert("service".into(), json!(service));
        }
    }
}

/// The person's documents with their file's own size, when the file is in the agent's uploads as the bridge sees them
/// (`[Pièce jointe : …]` gives it in words only, rounded): never a size the apps cannot read.
/// Each map once in a message (7/11, Alex's Fairphone: a trip's map in the page placed in the text, and again at the
/// reply's end): a map's own page (`map_image`'s `carte-<hex>`, made_files.rs `map_of_page`) left out of a message
/// whose other page holds that map (its card, pages.rs `maps_of`), as the turn's hand-over no longer adds it; this
/// one for the replies kept before. The other attachments keep their ids.
async fn with_maps_once(config: Arc<crate::config::Config>, agent: &AgentConfig, items: &mut [Item]) {
    let prefix = format!("{}/{}/", crate::pages::PREFIX, agent.name);
    let mut wanted: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(Value::Array(atts)) = item.body.get("attachments") else { continue };
        let pages: Vec<&str> = atts.iter().filter_map(|a| own_page(a, &prefix)).collect();
        let (maps, others): (Vec<&str>, Vec<&str>) =
            pages.into_iter().partition(|p| crate::made_files::map_of_page(p).is_some());
        if !maps.is_empty() && !others.is_empty() {
            wanted.push((i, others.into_iter().map(str::to_string).collect()));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let name = agent.name.clone();
    let held = tokio::task::spawn_blocking(move || {
        let maps_of = |pages: &[String]| -> Vec<String> {
            pages.iter().flat_map(|p| crate::pages::maps_of(&config, &name, p)).collect()
        };
        wanted.into_iter().map(|(i, pages)| (i, maps_of(&pages))).collect::<Vec<(usize, Vec<String>)>>()
    })
    .await
    .unwrap_or_default();
    for (i, maps) in held {
        if let Some(Value::Array(atts)) = items[i].body.get_mut("attachments") {
            atts.retain(|a| {
                own_page(a, &prefix).and_then(crate::made_files::map_of_page).is_none_or(|id| !maps.contains(&id))
            });
        }
    }
}

/// The bridge's pages an item names that aren't kept here left out (pages::missing: a name the model made up, a page
/// deleted since): never a page that opens on a 404. Its ids numbered again.
async fn without_missing_pages(config: Arc<crate::config::Config>, items: &mut [Item]) {
    let prefix = format!("{}/", crate::pages::PREFIX);
    let wanted: Vec<String> = items
        .iter()
        .filter_map(|i| i.body.get("attachments").and_then(Value::as_array))
        .flatten()
        .filter_map(|a| a.get("path").and_then(Value::as_str).filter(|p| p.starts_with(&prefix)).map(str::to_string))
        .collect();
    if wanted.is_empty() {
        return;
    }
    let missing = tokio::task::spawn_blocking(move || {
        wanted.into_iter().filter(|p| crate::pages::missing(&config, p)).collect::<HashSet<String>>()
    })
    .await
    .unwrap_or_default();
    if missing.is_empty() {
        return;
    }
    for item in items.iter_mut() {
        let Some(Value::Array(atts)) = item.body.get_mut("attachments") else { continue };
        let before = atts.len();
        atts.retain(|a| !a.get("path").and_then(Value::as_str).is_some_and(|p| missing.contains(p)));
        if atts.len() == before {
            continue;
        }
        for (n, a) in atts.iter_mut().enumerate() {
            a["id"] = json!(format!("{}.a{n}", item.id));
        }
        if atts.is_empty() {
            item.body.remove("attachments");
        }
    }
}

/// An attachment's path when it is one of the agent's pages (`prefix`: `/bipagents/pages/<agent>/`).
fn own_page<'a>(attachment: &'a Value, prefix: &str) -> Option<&'a str> {
    attachment.get("path").and_then(Value::as_str).filter(|p| p.starts_with(prefix))
}

async fn with_document_sizes(agent: &AgentConfig, items: &mut [Item]) {
    let (Some(host), Some(seen)) = (
        agent.upload_dir_host.clone().filter(|h| !h.is_empty()),
        agent.upload_dir_container.clone().filter(|c| !c.is_empty()),
    ) else {
        return;
    };
    let root = format!("{}/", seen.trim_end_matches('/'));
    let mut wanted: Vec<(usize, usize, String)> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(Value::Array(atts)) = item.body.get("attachments") else { continue };
        for (n, a) in atts.iter().enumerate() {
            let Some(path) = a.get("path").and_then(Value::as_str).filter(|_| a["kind"] == "document") else { continue };
            let Some(rest) = path.strip_prefix(&root) else { continue };
            if rest.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                continue;
            }
            wanted.push((i, n, format!("{}/{rest}", host.trim_end_matches('/'))));
        }
    }
    if wanted.is_empty() {
        return;
    }
    let sizes = tokio::task::spawn_blocking(move || {
        wanted
            .into_iter()
            .filter_map(|(i, n, p)| std::fs::metadata(&p).ok().filter(|m| m.is_file()).map(|m| (i, n, m.len())))
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    for (i, n, size) in sizes {
        if let Some(Value::Array(atts)) = items[i].body.get_mut("attachments") {
            atts[n]["size"] = json!(size);
        }
    }
}

async fn attachment_route(
    State(state): State<AppState>,
    UrlPath((id, attachment)): UrlPath<(String, String)>,
    q: QueryMap,
) -> ApiResult<Response> {
    let (message, n) = attachment.rsplit_once(".a").ok_or_else(HttpError::not_found)?;
    let n: usize = n.parse().map_err(|_| HttpError::not_found())?;
    let (agent, _, items) = all_items(&state, &id, &q).await?;
    let item = items.iter().find(|i| i.id == message).ok_or_else(HttpError::not_found)?;
    let att = item.body.get("attachments").and_then(|a| a.get(n)).cloned().ok_or_else(HttpError::not_found)?;
    if let Some(path) = att.get("path").and_then(Value::as_str) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = axum::http::StatusCode::TEMPORARY_REDIRECT;
        let enc = |v: &str| percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string();
        let owner = agent.as_ref().map(|a| a.name.as_str()).or(item.body.get("from").and_then(Value::as_str));
        let target = format!("/v1/media?path={}&agent={}", enc(path), enc(owner.unwrap_or_default()));
        response
            .headers_mut()
            .insert("location", axum::http::HeaderValue::from_str(&target).map_err(anyhow::Error::from)?);
        return Ok(response);
    }
    // a photo of the message: the n-th image part of its row, decoded (a room's files all have a path)
    let agent = agent.ok_or_else(HttpError::not_found)?;
    let content = threads_of(&state).source(&agent).content_with_images(&item.segment, item.row).await.map_err(unavailable)?;
    let images: Vec<String> = match &content {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("image_url" | "input_image" | "image")))
            .filter_map(|p| {
                p.get("image_url")
                    .and_then(|u| u.get("url").or(Some(u)))
                    .or_else(|| p.get("url"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        _ => Vec::new(),
    };
    let (mime, bytes) = match images.get(n) {
        Some(data) => {
            let (head, b64) =
                data.strip_prefix("data:").and_then(|d| d.split_once(',')).ok_or_else(HttpError::not_found)?;
            let mime = head.split(';').next().unwrap_or("application/octet-stream").to_string();
            (mime, base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| HttpError::not_found())?)
        }
        // Hermes v0.22.5 keeps a sent photo in a file of the agent's profile: `[Image attached at: <path>]`
        None => saved_photo(&state, &agent, &content, n).await.ok_or_else(HttpError::not_found)?,
    };
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert("content-type", axum::http::HeaderValue::from_str(&mime).map_err(anyhow::Error::from)?);
    response
        .headers_mut()
        .insert("cache-control", axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"));
    Ok(response)
}

/// The n-th photo a message names by its saved file (`[Image attached at: <path>]`): (mime, bytes), read only inside
/// the agent's own Hermes profile (never another path a row could name).
async fn saved_photo(state: &AppState, agent: &AgentConfig, content: &Option<Value>, n: usize) -> Option<(String, Vec<u8>)> {
    let text = match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => {
            parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
        }
        _ => return None,
    };
    let path = timeline::IMAGE_AT.captures_iter(&text).nth(n)?.get(1)?.as_str().trim().to_string();
    let root = crate::agents_admin::profile_folder(&state.config(), agent);
    let (root, file) = (tokio::fs::canonicalize(&root).await.ok()?, tokio::fs::canonicalize(&path).await.ok()?);
    if !file.starts_with(&root) {
        return None;
    }
    let mime = match file.extension().and_then(|e| e.to_str()).map(str::to_lowercase).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("heic") => "image/heic",
        _ => return None,
    };
    Some((mime.to_string(), tokio::fs::read(&file).await.ok()?))
}

async fn events(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    // a shared agent's conversation: the owner's hints for it (agent_shares.rs)
    if let (Some(id), Some(cid)) = (q.get("agent").filter(|a| a.contains(crate::households::SEPARATOR)), q.get("thread")) {
        return crate::agent_shares::relay_stream(&state, id, cid, "events").await;
    }
    let agent = agent_param(&state, &q)?;
    let threads = threads_of(&state);
    // `thread=<id|main|room:id>`: that conversation is on screen while the stream is open (presence)
    let shown = match q.get("thread").filter(|t| !t.is_empty()) {
        Some(id) if crate::rooms::of_thread(id).is_some() => Some(threads.show(room_of(&state, id).map(|_| id)?)),
        Some(id) => Some(threads.show(&threads.named(agent.as_ref(), id).await?.1.id)),
        None => None,
    };
    let only = agent.map(|a| a.name);
    let room = q.get("thread").filter(|t| crate::rooms::of_thread(t).is_some()).map(str::to_string);
    // a room's hints are keyed by its thread: for the streams that follow it and the home's (no agent, no thread)
    let home = only.is_none() && shown.is_none();
    let wanted = move |key: &str| match &room {
        Some(room) => key == room,
        None if crate::rooms::of_thread(key).is_some() => home,
        None => only.as_deref().is_none_or(|o| o == key),
    };
    let mut events = threads.subscribe();
    let line =
        |kind: &str, data: &Value| bytes::Bytes::from(format!("event: {kind}\ndata: {}\n\n", crate::py::dumps(data)));
    let stream = async_stream::stream! {
        let _shown = shown;   // held as long as the app reads the stream
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b": threads\n\n"));
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(crate::app::SSE_KEEPALIVE_SECONDS), events.recv()).await {
                Err(_) => yield Ok(bytes::Bytes::from_static(b": keepalive\n\n")),
                Ok(Ok((key, kind, data))) if wanted(&key) => yield Ok(line(&kind, &data)),
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => yield Ok(line("lagged", &json!({}))),
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
        .route("/threads", get(list))
        .route("/threads/resolve", get(resolve))
        .route("/threads/events", get(events))
        .route("/threads/{thread}", get(one).patch(rename).delete(delete_thread))
        .route("/threads/{thread}/segments", post(link))
        .route("/threads/{thread}/timeline", get(timeline_route))
        .route("/threads/{thread}/items/{item}", get(item_route))
        .route("/threads/{thread}/attachments/{attachment}", get(attachment_route))
        .route("/threads/{thread}/messages", post(send_route))
        .route("/threads/{thread}/stop", post(stop_route))
        .route("/threads/{thread}/sends/{client_id}", get(sends_route))
}

/// The person's message (send.rs); a shared agent's goes to its owner's bridge (agent_shares.rs).
async fn send_route(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<send::NewMessage>,
) -> ApiResult<Response> {
    if let Some(room) = crate::rooms::of_thread(&id) {
        if !body.attachments.is_empty() {
            return Err(HttpError::new(422, "a discussion takes words only"));
        }
        return Ok(web::ok(crate::rooms::rooms_of(&state).post(room, &body.text, &body.client_id).await?));
    }
    if let Some(relayed) = crate::agent_shares::relay_send(&state, body.agent.as_deref(), &id, &body).await {
        return relayed;
    }
    let threads = threads_of(&state);
    let agent = crate::app::agent_or_404(&state.config(), body.agent.as_deref())?;
    let thread = match id.as_str() {
        "new" => None,
        _ => Some(threads.named(Some(&agent), &id).await?.1),
    };
    Ok(web::ok(send::send(&state, &agent, thread, body).await?))
}

/// A person's message (`POST /v1/threads/{thread}/messages`) carried out whole even when its phone goes away meanwhile
/// (the app killed right after « Envoyer », its network lost). The server drops a request with its connection: one
/// dropped half-way left the message « being sent » with no turn started, or not kept at all (7/11: cut 10 ms in;
/// release journeys J19, the conversation reopened empty). Its body read first, the request then runs apart from the
/// connection, to its end. The outermost layer (app.rs): every await of the request is behind it.
pub async fn send_whole(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if !is_send(request.method(), request.uri().path()) {
        return next.run(request).await;
    }
    let limit = crate::body_limit::limit_for(request.uri().path()).unwrap_or(crate::body_limit::DEFAULT_BYTES);
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, limit as usize).await {
        Ok(bytes) => bytes,
        Err(err) if crate::body_limit::is_too_large(&err) => return crate::body_limit::too_large_response(),
        Err(_) => return HttpError::new(400, "the message did not arrive whole").into_response(),
    };
    let whole = tokio::spawn(next.run(axum::extract::Request::from_parts(parts, Body::from(bytes))));
    whole.await.unwrap_or_else(|_| HttpError::internal().into_response())
}

/// `POST /v1/threads/{thread}/messages`.
fn is_send(method: &axum::http::Method, path: &str) -> bool {
    let thread = path.strip_prefix("/v1/threads/").and_then(|rest| rest.strip_suffix("/messages"));
    method == axum::http::Method::POST && thread.is_some_and(|t| !t.is_empty() && !t.contains('/'))
}

async fn stop_route(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<send::Stop>,
) -> ApiResult<Response> {
    if let Some(room) = crate::rooms::of_thread(&id) {
        let stopped = crate::rooms::rooms_of(&state).stop(room).await?.ok_or_else(HttpError::not_found)?;
        return Ok(web::ok(json!({"stopped": stopped})));
    }
    if let Some(relayed) = crate::agent_shares::relay_stop(&state, body.agent.as_deref(), &id).await {
        return relayed;
    }
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param_of(&state, body.agent.as_deref())?.as_ref(), &id).await?;
    Ok(web::ok(send::stop(&state, &agent, &thread).await?))
}

/// The room a `room:<id>` thread names (404 when none).
fn room_of(state: &AppState, thread: &str) -> ApiResult<crate::rooms::Room> {
    crate::rooms::of_thread(thread).and_then(|id| crate::rooms::load(&state.config(), id)).ok_or_else(HttpError::not_found)
}

fn agent_param_of(state: &AppState, name: Option<&str>) -> ApiResult<Option<AgentConfig>> {
    match name.filter(|a| !a.is_empty()) {
        Some(name) => Ok(Some(crate::app::agent_or_404(&state.config(), Some(name))?)),
        None => Ok(None),
    }
}

/// What the run hub and the push service keep to reach the threads (made with the app).
pub type Link = std::sync::OnceLock<Weak<Threads>>;

pub fn upgrade(link: &Link) -> Option<Arc<Threads>> {
    link.get().and_then(Weak::upgrade)
}

#[cfg(test)]
mod maps_once_tests {
    use super::*;

    fn reply(id: &str, paths: &[&str]) -> Item {
        let atts: Vec<Value> = paths
            .iter()
            .enumerate()
            .map(|(n, p)| json!({"id": format!("{id}.a{n}"), "kind": "page", "path": p}))
            .collect();
        let body = json!({"kind": "message", "role": "agent", "text": "Voici ton trajet.", "attachments": atts});
        Item {
            id: id.into(),
            pos: (0, 0, 0),
            at: 0.0,
            turn: id.into(),
            segment: "s".into(),
            row: 0,
            body: body.as_object().cloned().unwrap_or_default(),
        }
    }

    fn paths(item: &Item) -> Vec<String> {
        item.body["attachments"].as_array().unwrap().iter().map(|a| a["path"].as_str().unwrap().to_string()).collect()
    }

    #[tokio::test]
    async fn a_kept_reply_shows_its_map_once() {
        // a reply kept before the hand-over's fix: the trip's page holding the map, and the map's own page at the end
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::config::Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let folder = crate::pages::root(&config).join("vie");
        std::fs::create_dir_all(&folder).unwrap();
        let card = "<figure class=\"map-card\" data-map=\"m-0123456789\"></figure>";
        std::fs::write(folder.join("trajet.html"), card).unwrap();
        let agent = AgentConfig { name: "vie".into(), ..AgentConfig::default() };
        let (trip, carte) = ("/bipagents/pages/vie/trajet.html", "/bipagents/pages/vie/carte-0123456789.html");
        let other = "/bipagents/pages/vie/carte-aaaaaaaaaa.html";
        let mut items = vec![reply("m_1", &[trip, carte]), reply("m_2", &[carte]), reply("m_3", &[trip, other])];
        with_maps_once(Arc::new(config), &agent, &mut items).await;
        assert_eq!(paths(&items[0]), vec![trip.to_string()]);
        assert_eq!(items[0].body["attachments"][0]["id"], "m_1.a0");
        // the map alone, or another map than the page's: kept
        assert_eq!(paths(&items[1]), vec![carte.to_string()]);
        assert_eq!(paths(&items[2]), vec![trip.to_string(), other.to_string()]);
    }
}

#[cfg(test)]
mod page_open_tests {
    use super::*;

    fn message(id: &str) -> Item {
        let body = json!({"kind": "message", "role": "person", "text": "Raconte"});
        Item {
            id: id.into(),
            pos: (0, 0, 0),
            at: 0.0,
            turn: id.into(),
            segment: "s".into(),
            row: 0,
            body: body.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn a_page_names_the_turn_going_on_only_among_its_items() {
        let items: Vec<Item> = ["m_1", "m_2", "m_3"].into_iter().map(message).collect();
        // the turn going on began at m_2: named on the latest page, the first item of a page that starts after it
        // began, none on a page that ends before it
        assert_eq!(open_on_page(&items, Some(1), &(0..3)).as_deref(), Some("m_2"));
        assert_eq!(open_on_page(&items, Some(1), &(2..3)).as_deref(), Some("m_3"));
        assert_eq!(open_on_page(&items, Some(1), &(0..1)), None);
        // a catch-up with nothing new while it goes on (`after=` the last item): no item to name, never a panic (a 500
        // the iPhone showed as « Ton ordinateur a eu un souci », its card and « Arrêter » lost, 7/11)
        assert_eq!(open_on_page(&items, Some(1), &(3..3)), None);
        assert_eq!(open_on_page(&items, Some(2), &(3..3)), None);
        assert_eq!(open_on_page(&[], Some(0), &(0..0)), None);
        assert_eq!(open_on_page(&items, None, &(0..3)), None);
    }
}

#[cfg(test)]
mod send_whole_tests {
    use super::is_send;
    use axum::http::Method;

    #[test]
    fn only_a_message_sent_runs_apart_from_its_connection() {
        assert!(is_send(&Method::POST, "/v1/threads/main/messages"));
        assert!(is_send(&Method::POST, "/v1/threads/t_001a1b5c/messages"));
        assert!(is_send(&Method::POST, "/v1/threads/room:abc/messages"));
        assert!(!is_send(&Method::GET, "/v1/threads/main/messages"));
        assert!(!is_send(&Method::POST, "/v1/threads/main/stop"));
        assert!(!is_send(&Method::POST, "/v1/threads//messages"));
        assert!(!is_send(&Method::POST, "/v1/threads/main/items/x/messages"));
        assert!(!is_send(&Method::POST, "/v1/messages"));
    }
}
