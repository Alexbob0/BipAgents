//! Conversations owned by the bridge (docs/conversations.md): one stable thread id per conversation of an agent,
//! whatever Hermes does with titles and session ids; a thread is a chain of Hermes sessions (its segments: Hermes
//! goes on in a child session after compressing a long conversation); one timeline the apps draw, with stable item
//! ids, the Boîte's messages and what is still pending (the run going on, the queue, approvals).
//!
//! ```text
//! GET  /v1/threads?agent=[&include_hidden=1]                    {threads: [Thread]}
//! GET  /v1/threads/resolve?agent=&session_id= | &c=             {thread, tip}
//! GET  /v1/threads/events[?agent=]                              SSE: thread, notification, read
//! GET  /v1/threads/{thread}[?agent=]                            Thread with its segments (`main` needs agent)
//! POST /v1/threads/{thread}/segments {session_id, agent?}       a session appended (migrations)
//! GET  /v1/threads/{thread}/timeline?agent=&limit=&before=&after=&around=&before_count=&after_count=
//! GET  /v1/threads/{thread}/items/{item}
//! GET  /v1/threads/{thread}/attachments/{attachment}
//! ```
//!
//! Tables in bridge.db (store.rs): `threads`, `thread_segments`, `thread_messages`. Reads Hermes through
//! [`source::Source`]: the agent's `state.db` (no call to Hermes) or its API (cached).
//!
//! No Python counterpart.

pub mod source;
pub mod timeline;

use std::collections::HashSet;
use std::sync::{Arc, Weak};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use sha2::Digest;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::app::AppState;
use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::HermesClient;
use crate::store::{iso, utcnow, Store};
use crate::web::{self, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use source::{ApiCache, Session, Source};
use timeline::{Item, Want};

/// The title Hermes' Bot Mode gives the agent's permanent conversation: read once, to adopt an existing place's main
/// conversation (docs/conversations.md §8). Nothing else here depends on a title.
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
            origin: row.get("origin").and_then(Value::as_str).and_then(|o| serde_json::from_str(o).ok()).unwrap_or(Value::Null),
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
}

pub fn threads_of(state: &AppState) -> Arc<Threads> {
    state
        .threads
        .get_or_init(|| {
            let mut threads = Threads::new(state.config.clone(), state.store.clone(), state.hermes.clone());
            threads.briefs = Some(state.briefs.clone());
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
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<(String, String, Value)> {
        self.events.subscribe()
    }

    /// An event for the apps (`thread`, `notification`, `read`).
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
            .run("SELECT session_id, position, linked FROM thread_segments WHERE thread_id = ? ORDER BY position", vec![text(thread)])
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

    async fn thread_of_session(&self, agent: &str, session: &str) -> anyhow::Result<Option<String>> {
        let rows = self
            .store
            .run("SELECT thread_id FROM thread_segments WHERE agent = ? AND session_id = ?", vec![text(agent), text(session)])
            .await?;
        Ok(rows.first().and_then(|r| r["thread_id"].as_str()).map(str::to_string))
    }

    async fn create(&self, agent: &str, kind: &str, title: Option<&str>, origin: Value, hidden: bool) -> anyhow::Result<Thread> {
        let now = iso(utcnow());
        let id = new_id("t_");
        let origin = if origin.is_null() { Param::Null } else { text(&origin.to_string()) };
        let title = title.map(text).unwrap_or(Param::Null);
        self.store
            .run(
                "INSERT INTO threads (id, agent, kind, title, origin, hidden, version, created_at, updated_at) \
                 VALUES (?,?,?,?,?,?,0,?,?)",
                vec![text(&id), text(agent), text(kind), title, origin, Param::Integer(hidden as i64), text(&now), text(&now)],
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
            .run("UPDATE threads SET version = version + 1, updated_at = ? WHERE id = ?", vec![text(&now), text(&thread.id)])
            .await?;
        let version = self.get(&thread.id).await?.map(|t| t.version).unwrap_or(thread.version + 1);
        self.publish(&thread.agent, "thread", json!({"thread": thread.id, "agent": thread.agent, "version": version, "reason": reason}));
        Ok(version)
    }

    /// The session's whole line: parents up to the root, continuations down to the tip.
    async fn line_of(&self, source: &Source, session: Session) -> anyhow::Result<Vec<Session>> {
        let mut line = vec![session];
        for _ in 0..64 {
            let first = &line[0];
            let Some(parent_id) = first.parent.clone() else { break };
            match source.session(&parent_id).await? {
                Some(parent) if first.continues(&parent) && !line.iter().any(|s| s.id == parent.id) => line.insert(0, parent),
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

    /// The agent's main thread, adopted from its existing conversation the first time (docs/conversations.md §8).
    pub async fn main(&self, agent: &AgentConfig) -> anyhow::Result<Thread> {
        let found = self
            .store
            .run("SELECT * FROM threads WHERE agent = ? AND kind = 'main'", vec![text(&agent.name)])
            .await?;
        if let Some(row) = found.first() {
            return Ok(Thread::of(row));
        }
        let _guard = self.registry.lock().await;
        let found = self.store.run("SELECT * FROM threads WHERE agent = ? AND kind = 'main'", vec![text(&agent.name)]).await?;
        if let Some(row) = found.first() {
            return Ok(Thread::of(row));
        }
        let source = self.source(agent);
        let session = match self.main_session(agent, &source).await {
            Ok(s) => s,
            Err(exc) => {
                // Hermes asleep or down: try again at the next request rather than making an empty main thread
                return Err(exc.context("main conversation not adopted yet"));
            }
        };
        let line = match session {
            Some(s) => self.line_of(&source, s).await?,
            None => Vec::new(),
        };
        // a session another thread holds (adopted as a side one before) moves to the main thread
        for s in &line {
            self.store
                .run("DELETE FROM thread_segments WHERE agent = ? AND session_id = ?", vec![text(&agent.name), text(&s.id)])
                .await?;
        }
        let thread = self.create(&agent.name, "main", None, Value::Null, false).await?;
        for (i, s) in line.iter().enumerate() {
            self.append(&agent.name, &thread.id, &s.id, if i == 0 { "adopted" } else { "compression" }).await?;
        }
        tracing::info!(agent = %agent.name, thread = %thread.id, segments = line.len(), "main thread adopted");
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
        let (kind, hidden) = if routine_session(root) {
            ("routine", true)
        } else if root.id.starts_with("local-") || (titled_main(root) && root.hidden) {
            ("pair", true) // the agents' own conversation (Bot Mode's peers)
        } else {
            ("side", root.hidden)
        };
        let origin = if kind == "routine" {
            let job = crate::cronwatch::routine_of(&self.config.get(), agent, &root.id);
            json!({"kind": "routine", "routine": job})
        } else {
            Value::Null
        };
        let thread = self.create(&agent.name, kind, root.title.as_deref(), origin, hidden).await?;
        for (i, s) in line.iter().enumerate() {
            self.append(&agent.name, &thread.id, &s.id, if i == 0 { "root" } else { "compression" }).await?;
        }
        let _ = main;
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
                        .run("DELETE FROM thread_segments WHERE agent = ? AND session_id = ?", vec![text(&agent.name), text(&child.id)])
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

    /// Appends a session as the thread's next segment (a migration moving the conversation, a test).
    pub async fn link(&self, agent: &AgentConfig, thread: &Thread, session: &str) -> anyhow::Result<()> {
        let _guard = self.registry.lock().await;
        self.store
            .run("DELETE FROM thread_segments WHERE agent = ? AND session_id = ?", vec![text(&agent.name), text(session)])
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
            items.extend(timeline::segment_items(&agent.name, segment.position, &segment.session_id, &rows, &skip, &mut turn));
            previous = Some(rows);
        }
        self.with_client_ids(agent, &mut items).await?;
        let mut extra = self.boite(agent, thread, segments).await?;
        extra.extend(self.kept(agent, segments).await?);
        Ok(timeline::merge(items, extra))
    }

    /// The `client_id` and the files the app sent with each of its messages (thread_messages).
    async fn with_client_ids(&self, agent: &AgentConfig, items: &mut [Item]) -> anyhow::Result<()> {
        let rows = self
            .store
            .run("SELECT message_id, client_id, attachments FROM thread_messages WHERE agent = ?", vec![text(&agent.name)])
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        for item in items.iter_mut() {
            let Some(row) = rows.iter().find(|r| r["message_id"].as_str() == Some(item.id.as_str())) else { continue };
            if let Some(c) = row["client_id"].as_str() {
                item.body.insert("client_id".into(), json!(c));
            }
            if let Some(Value::Array(sent)) = row["attachments"].as_str().and_then(|a| serde_json::from_str(a).ok()) {
                if !sent.is_empty() {
                    let atts: Vec<Value> = sent
                        .into_iter()
                        .enumerate()
                        .map(|(n, mut a)| {
                            a["id"] = json!(format!("{}.a{n}", item.id));
                            a
                        })
                        .collect();
                    item.body.insert("attachments".into(), Value::Array(atts));
                }
            }
        }
        Ok(())
    }

    /// The Boîte's messages that belong to this thread: tagged with one of its sessions; for the main thread also
    /// those tagged with nothing or with a routine's run (a report without a conversation of its own).
    async fn boite(&self, agent: &AgentConfig, thread: &Thread, segments: &[Segment]) -> anyhow::Result<Vec<Item>> {
        let rows = self
            .store
            .run("SELECT id, title, text, created_at, session_id, audio_path FROM outbox WHERE agent = ? ORDER BY created_at",
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
            let at = row["created_at"].as_str().and_then(|c| crate::store::parse_since(c).ok()).map(|d| d.timestamp_micros() as f64 / 1e6).unwrap_or(0.0);
            let (words, attachments) = timeline::visible(row["text"].as_str().unwrap_or(""), &id, 0);
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
                    let key = format!("{}/{}", entry.get("run_id").and_then(Value::as_str).unwrap_or(""),
                                      entry.get("request_id").and_then(Value::as_str).unwrap_or(""));
                    let mut body = serde_json::Map::new();
                    body.insert("kind".into(), json!("approval"));
                    body.insert("approval".into(), json!({
                        "run_id": entry.get("run_id"), "request_id": entry.get("request_id"), "command": entry.get("command"),
                        "description": entry.get("description"), "topic": entry.get("topic"),
                        "choice": entry.get("choice"), "state": "answered"}));
                    out.push((format!("a_{}", &hex::encode(sha2::Sha256::digest(key.as_bytes()))[..24]), at, session.clone(), body));
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
    pub async fn find_message(&self, agent: &AgentConfig, session: &str, words: Option<&str>, wait: f64) -> Option<(String, String)> {
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
                let found = items.iter().rev().filter(|i| i.kind() == "message" && i.body.get("role") == Some(&json!("agent"))).find(|i| {
                    match (&wanted, &head) {
                        (Some(w), Some(h)) => i.text() == w || i.text().starts_with(h.as_str()),
                        _ => true,
                    }
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

/// Hermes not reachable while adopting: the apps try again (503), nothing half made.
fn unavailable(exc: anyhow::Error) -> HttpError {
    tracing::warn!(error = %format!("{exc:#}"), "thread not available");
    HttpError::new(503, "the agent's conversations can't be read right now")
}

// -- routes -----------------------------------------------------------------------------------------------------------

fn agent_param(state: &AppState, q: &QueryMap) -> ApiResult<Option<AgentConfig>> {
    match q.get("agent").filter(|a| !a.is_empty()) {
        Some(name) => Ok(Some(crate::app::agent_or_404(&state.config(), Some(name))?)),
        None => Ok(None),
    }
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
        out["segments"] = json!(segments.iter().map(|s| json!({"session_id": s.session_id, "linked": s.linked})).collect::<Vec<_>>());
    }
    out["unread"] = json!(crate::notifications::unread_in(&threads.store, &thread.id).await.unwrap_or(0));
    Ok(out)
}

async fn list(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = agent_param(&state, &q)?.ok_or_else(|| HttpError::new(422, "agent is required"))?;
    let threads = threads_of(&state);
    let include_hidden = q.get("include_hidden").is_some_and(|v| v == "1" || v == "true");
    let main = threads.main(&agent).await.map_err(unavailable)?;
    // the agent's sessions Hermes keeps: each root one is a thread (adopted at first sight)
    let sessions = threads.source(&agent).sessions(LISTED).await.map_err(unavailable)?;
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
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    Ok(web::ok(served(&threads, &agent, &thread, true).await.map_err(unavailable)?))
}

async fn resolve(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = agent_param(&state, &q)?.ok_or_else(|| HttpError::new(422, "agent is required"))?;
    let threads = threads_of(&state);
    let thread = match (q.get("session_id").filter(|s| !s.is_empty()), q.get("c").filter(|s| !s.is_empty())) {
        (Some(session), _) => threads.resolve(&agent, session).await.map_err(unavailable)?.ok_or_else(HttpError::not_found)?,
        (None, Some(c)) if c.starts_with("room:") => {
            return Ok(web::ok(json!({"thread": {"id": c, "agent": agent.name, "kind": "room"}, "tip": null})))
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
            errors.push(FieldError::new("value_error", vec![json!("body"), json!("session_id")], "invalid session id", json!(self.session_id)));
        }
    }
}

async fn link(State(state): State<AppState>, UrlPath(id): UrlPath<String>, Json(body): Json<LinkBody>) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let agent = match body.agent.as_deref() {
        Some(a) => Some(crate::app::agent_or_404(&state.config(), Some(a))?),
        None => None,
    };
    let (agent, thread) = threads.named(agent.as_ref(), &id).await?;
    threads.link(&agent, &thread, &body.session_id).await?;
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    Ok(web::ok(served(&threads, &agent, &thread, true).await.map_err(unavailable)?))
}

async fn timeline_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, q: QueryMap) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let want = Want {
        limit: number(&q, "limit", LIMIT_DEFAULT)?.clamp(1, LIMIT_MAX),
        before: q.get("before").filter(|v| !v.is_empty()).map(str::to_string),
        after: q.get("after").filter(|v| !v.is_empty()).map(str::to_string),
        around: q.get("around").filter(|v| !v.is_empty()).map(str::to_string),
        before_count: number(&q, "before_count", 20)?.min(LIMIT_MAX),
        after_count: match q.get("after_count") {
            None | Some("") | Some("all") => None,
            Some(_) => Some(number(&q, "after_count", 0)?),
        },
    };
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let thread = threads.get(&thread.id).await?.unwrap_or(thread);
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    let page = timeline::page(items, &want);
    let tip = segments.last().map(|s| s.session_id.clone());
    let sessions: HashSet<&str> = segments.iter().map(|s| s.session_id.as_str()).collect();
    let approvals: Vec<Value> = state
        .hub
        .pending_approvals(Some(&agent.name))
        .into_iter()
        .filter(|a| a.get("session_id").and_then(Value::as_str).is_some_and(|s| sessions.contains(s)))
        .map(Value::Object)
        .collect();
    let run = tip.as_deref().and_then(|t| state.hub.session_run(&agent.name, t));
    let queue = tip.as_deref().map(|t| crate::msg_queue::queues_of(&state).get(&agent.name).snapshot_of(&agent.name, t));
    let mut out_thread = thread.to_json();
    out_thread["tip"] = json!(tip);
    let debug = q.get("debug").is_some_and(|d| d == "1");
    let items: Vec<Value> = page.items.iter().map(|i| i.to_json(&thread.id, debug)).collect();
    Ok(web::ok(json!({
        "thread": out_thread,
        "items": items,
        "first": page.items.first().map(Item::cursor),
        "last": page.items.last().map(Item::cursor),
        "more_before": page.more_before,
        "more_after": page.more_after,
        "anchor": page.anchor,
        "version": thread.version,
        "pending": {"run": run.map(|r| json!({"run_id": r, "status": "running"})), "queue": queue, "approvals": approvals},
    })))
}

async fn item_route(State(state): State<AppState>, UrlPath((id, item)): UrlPath<(String, String)>, q: QueryMap) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    let found = items.iter().find(|i| i.id == item).ok_or_else(HttpError::not_found)?;
    Ok(web::ok(found.to_json(&thread.id, q.get("debug").is_some_and(|d| d == "1"))))
}

async fn attachment_route(
    State(state): State<AppState>,
    UrlPath((id, attachment)): UrlPath<(String, String)>,
    q: QueryMap,
) -> ApiResult<Response> {
    let threads = threads_of(&state);
    let (agent, thread) = threads.named(agent_param(&state, &q)?.as_ref(), &id).await?;
    let (message, n) = attachment.rsplit_once(".a").ok_or_else(HttpError::not_found)?;
    let n: usize = n.parse().map_err(|_| HttpError::not_found())?;
    let segments = threads.line(&agent, &thread).await.map_err(unavailable)?;
    let items = threads.items(&agent, &thread, &segments).await.map_err(unavailable)?;
    let item = items.iter().find(|i| i.id == message).ok_or_else(HttpError::not_found)?;
    let att = item.body.get("attachments").and_then(|a| a.get(n)).cloned().ok_or_else(HttpError::not_found)?;
    if let Some(path) = att.get("path").and_then(Value::as_str) {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = axum::http::StatusCode::TEMPORARY_REDIRECT;
        let enc = |v: &str| percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC).to_string();
        let target = format!("/v1/media?path={}&agent={}", enc(path), enc(&agent.name));
        response.headers_mut().insert("location", axum::http::HeaderValue::from_str(&target).map_err(anyhow::Error::from)?);
        return Ok(response);
    }
    // a photo of the message: the n-th image part of its row, decoded
    let content = threads.source(&agent).content_with_images(&item.segment, item.row).await.map_err(unavailable)?;
    let images: Vec<String> = match content {
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|p| matches!(p.get("type").and_then(Value::as_str), Some("image_url" | "input_image" | "image")))
            .filter_map(|p| {
                p.get("image_url").and_then(|u| u.get("url").or(Some(u))).or_else(|| p.get("url")).and_then(Value::as_str).map(str::to_string)
            })
            .collect(),
        _ => Vec::new(),
    };
    let data = images.get(n).ok_or_else(HttpError::not_found)?;
    let (head, b64) = data.strip_prefix("data:").and_then(|d| d.split_once(',')).ok_or_else(HttpError::not_found)?;
    let mime = head.split(';').next().unwrap_or("application/octet-stream").to_string();
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| HttpError::not_found())?;
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert("content-type", axum::http::HeaderValue::from_str(&mime).map_err(anyhow::Error::from)?);
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("private, max-age=31536000, immutable"));
    Ok(response)
}

async fn events(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let only = agent_param(&state, &q)?.map(|a| a.name);
    let mut events = threads_of(&state).subscribe();
    let line = |kind: &str, data: &Value| bytes::Bytes::from(format!("event: {kind}\ndata: {}\n\n", crate::py::dumps(data)));
    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b": threads\n\n"));
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(crate::app::SSE_KEEPALIVE_SECONDS), events.recv()).await {
                Err(_) => yield Ok(bytes::Bytes::from_static(b": keepalive\n\n")),
                Ok(Ok((agent, kind, data))) if only.as_deref().is_none_or(|o| o == agent) => yield Ok(line(&kind, &data)),
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
        .route("/threads/{thread}", get(one))
        .route("/threads/{thread}/segments", post(link))
        .route("/threads/{thread}/timeline", get(timeline_route))
        .route("/threads/{thread}/items/{item}", get(item_route))
        .route("/threads/{thread}/attachments/{attachment}", get(attachment_route))
}

/// What the run hub and the push service keep to reach the threads (made with the app).
pub type Link = std::sync::OnceLock<Weak<Threads>>;

pub fn upgrade(link: &Link) -> Option<Arc<Threads>> {
    link.get().and_then(Weak::upgrade)
}
