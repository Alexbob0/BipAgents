//! Where a thread's sessions and rows are read. Hermes' API is the reference: every agent can be read through it
//! ([`Source::Api`]). The agent's Hermes store (`<profile>/state.db`, read-only, no call to Hermes) is read instead
//! when the bridge reaches the profile folder, because v0.22.5's API does not give all a timeline needs
//! (internal/notes/finalise.md, measured on the black-box fake):
//! - `message_uid`: the stable message ids (`m_<uid>`, kept by the apps and by `thread_messages`) and the in-place
//!   compaction's rows (a summary taking the uid of the rows it absorbed: `timeline::in_place_compaction`);
//! - the inactive rows a summary absorbed (the API, `include_compacted`, gives Hermes' own display list: compacted rows
//!   too, but each message at its newest copy's row id, without uid or flags: build server, 2026-11-04);
//! - hidden sessions in the list (routines, the Bot Chat): `/api/sessions` lists them only for a title search.
//!
//! What the bridge reads of state.db is [`STATE_DB_READS`]: a store without one of them is logged as an error once
//! (the read goes on with a neutral value), its unit test checks them against the pinned Hermes' schema
//! (tests/golden/hermes_state_schema.sql), and scripts/hermes-schema-check.py against any Hermes checkout (the weekly
//! upstream check). One agent always uses the same source, so the ids made from its rows never change between reads.
//!
//! The API source caches what can't change: a segment that is not the tip any more (Hermes never writes into a
//! compressed session again) is read once; the tip is read again only after the thread was touched (a run's end, a
//! Boîte message, a notification) or `TIP_TTL` later. An app opening a conversation costs Hermes nothing then.
//! Hermes gives a segment's whole lineage (root → segment, each row with its `session_id`), 500 rows a page: the
//! segment's own rows are kept, page after page.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::config::{AgentConfig, Config};
use crate::hermes::HermesClient;
use crate::Dict;

/// Every table and column of Hermes' state.db the bridge reads (threads, message_search.rs).
/// The one list: scripts/hermes-schema-check.py parses it.
pub const STATE_DB_READS: &[(&str, &[&str])] = &[
    (
        "sessions",
        &["id", "parent_session_id", "title", "source", "model_config", "end_reason", "started_at", "ended_at", "hidden"],
    ),
    (
        "messages",
        &[
            "id",
            "session_id",
            "role",
            "content",
            "timestamp",
            "message_uid",
            "tool_calls",
            "tool_call_id",
            "tool_name",
            "display_kind",
            "display_metadata",
            "reasoning",
            "active",
            "compacted",
            "_compressed_summary",
            "display_order",
        ],
    ),
];

/// The tables and columns of [`STATE_DB_READS`] the store at `conn` doesn't have (`table` or `table.column`).
pub fn missing_columns(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut missing = Vec::new();
    for (table, wanted) in STATE_DB_READS {
        let have = columns(conn, table)?;
        if have.is_empty() {
            missing.push(table.to_string());
            continue;
        }
        missing.extend(wanted.iter().filter(|c| !have.iter().any(|h| h == *c)).map(|c| format!("{table}.{c}")));
    }
    Ok(missing)
}

/// Logged once per store: a Hermes whose state.db lost what the bridge reads (an update past the pinned one).
fn check_schema(path: &std::path::Path, conn: &Connection) {
    static CHECKED: std::sync::LazyLock<Mutex<std::collections::HashSet<PathBuf>>> = std::sync::LazyLock::new(Default::default);
    if !CHECKED.lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_path_buf()) {
        return;
    }
    let missing = missing_columns(conn).unwrap_or_default();
    if !missing.is_empty() {
        tracing::error!(target: "bipbridge::threads", store = %path.display(), missing = %missing.join(", "),
                        "Hermes' state.db changed: columns the bridge reads are gone (scripts/hermes-schema-check.py)");
    }
}

/// How long the API source trusts a tip it read without being told of a change.
pub const TIP_TTL: f64 = 30.0;
/// Rows kept in the API cache at most (all agents), before the oldest segments are dropped.
const CACHE_ROWS: usize = 50_000;

/// Sessions in one page of Hermes' list (its API serves at most 200).
const PAGE: usize = 200;
/// Sessions read at most for the whole list.
const ALL_MAX: usize = 10_000;

/// A Hermes session, as far as threads go.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Session {
    pub id: String,
    pub parent: Option<String>,
    pub title: Option<String>,
    pub source: Option<String>,
    pub model_config: Option<String>,
    pub end_reason: Option<String>,
    /// None when the store has no such column (an older Hermes).
    pub has_end_reason: bool,
    pub started_at: f64,
    pub last_active: f64,
    pub hidden: bool,
}

impl Session {
    /// A child Hermes resumes in after compressing `parent`'s context: not a branch, a sub-agent's run or a reset.
    /// The one rule for every reader (cronwatch.rs, threads).
    pub fn continues(&self, parent: &Session) -> bool {
        if self.parent.as_deref() != Some(parent.id.as_str()) {
            return false;
        }
        let config = self.model_config.as_deref().unwrap_or("");
        if ["_branched_from", "_delegate_from", "_reset_from"].iter().any(|k| config.contains(&format!("\"{k}\""))) {
            return false;
        }
        if self.source.as_deref() == Some("tool") {
            return false;
        }
        // a store that says how a session ended: only a compression goes on in a child
        !parent.has_end_reason || parent.end_reason.as_deref() == Some("compression")
    }

    fn of_api(row: &Dict) -> Option<Session> {
        let s = |k: &str| row.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
        let f = |keys: &[&str]| keys.iter().find_map(|k| row.get(*k).and_then(Value::as_f64)).unwrap_or(0.0);
        Some(Session {
            id: s("id").or_else(|| s("session_id"))?,
            parent: s("parent_session_id"),
            title: s("title"),
            source: s("source"),
            model_config: match row.get("model_config") {
                Some(Value::String(t)) => Some(t.clone()),
                Some(v @ Value::Object(_)) => Some(v.to_string()),
                _ => None,
            },
            end_reason: s("end_reason"),
            has_end_reason: row.contains_key("end_reason"),
            started_at: f(&["started_at", "created_at", "created"]),
            last_active: f(&["last_active", "updated_at", "ended_at", "started_at"]),
            hidden: row.get("hidden").map(crate::py::truthy).unwrap_or(false),
        })
    }
}

/// One stored message of a session, in display order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    /// Hermes' row number (or the position in the API's list).
    pub row: i64,
    /// The display order (state.db's `display_order`, else the row number).
    pub order: i64,
    pub uid: Option<String>,
    pub role: String,
    /// A string, or a list of parts (text, image).
    pub content: Value,
    pub tool_calls: Option<Value>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub timestamp: f64,
    pub display_kind: Option<String>,
    pub reasoning: Option<String>,
    /// Shown: active, or compacted (Hermes still displays a compacted row), but never a compaction's shortened copy of
    /// a tool result whose archived original is shown (`later_tool_copy_sql`; Hermes 0.23's API leaves it out itself).
    pub shown: bool,
    /// A compression's summary (never shown).
    pub summary: bool,
}

impl Row {
    fn of_api(index: usize, row: &Dict) -> Row {
        let s = |k: &str| row.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string);
        let id = row.get("id").and_then(Value::as_i64).unwrap_or(index as i64);
        let active = row.get("active").map(crate::py::truthy).unwrap_or(true);
        let compacted = row.get("compacted").map(crate::py::truthy).unwrap_or(false);
        Row {
            row: id,
            order: index as i64,
            uid: s("message_uid"),
            role: s("role").unwrap_or_default().to_lowercase(),
            content: row.get("content").cloned().or_else(|| row.get("text").cloned()).unwrap_or(Value::Null),
            tool_calls: row.get("tool_calls").filter(|v| !v.is_null()).cloned(),
            tool_call_id: s("tool_call_id"),
            tool_name: s("tool_name").or_else(|| s("name")),
            timestamp: ["timestamp", "created_at", "ts"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_f64))
                .unwrap_or(0.0),
            display_kind: s("display_kind"),
            reasoning: s("reasoning").or_else(|| s("reasoning_content")),
            shown: active || compacted,
            summary: row.get("_compressed_summary").map(crate::py::truthy).unwrap_or(false),
        }
    }
}

/// Where an agent's sessions are read.
#[derive(Clone)]
pub enum Source {
    Db(PathBuf),
    Api(HermesClient, Box<AgentConfig>, Arc<ApiCache>),
}

/// When a segment's rows were read, and the rows.
type Cached = (f64, Arc<Vec<Row>>);

/// The API source's rows per (agent, session).
#[derive(Default)]
pub struct ApiCache {
    rows: Mutex<HashMap<(String, String), Cached>>,
}

impl ApiCache {
    /// The thread was touched: its tip is read again next time.
    pub fn forget(&self, agent: &str, session: &str) {
        self.rows.lock().unwrap_or_else(|e| e.into_inner()).remove(&(agent.to_string(), session.to_string()));
    }

    fn get(&self, agent: &str, session: &str, tip: bool) -> Option<Arc<Vec<Row>>> {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        let (at, kept) = rows.get(&(agent.to_string(), session.to_string()))?;
        (!tip || crate::py::time_time() - at < TIP_TTL).then(|| kept.clone())
    }

    fn put(&self, agent: &str, session: &str, kept: Arc<Vec<Row>>) {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        rows.insert((agent.to_string(), session.to_string()), (crate::py::time_time(), kept));
        while rows.values().map(|(_, r)| r.len()).sum::<usize>() > CACHE_ROWS && rows.len() > 1 {
            let oldest = rows.iter().min_by(|a, b| a.1 .0.total_cmp(&b.1 .0)).map(|(k, _)| k.clone());
            match oldest {
                Some(key) => rows.remove(&key),
                None => break,
            };
        }
    }
}

impl Source {
    pub fn of(config: &Config, agent: &AgentConfig, hermes: &HermesClient, cache: &Arc<ApiCache>) -> Source {
        match crate::message_search::store_path(config, &agent.name) {
            Some(path) => Source::Db(path),
            None => Source::Api(hermes.clone(), Box::new(agent.clone()), cache.clone()),
        }
    }

    /// Every session the agent's Hermes keeps (continuations included), most recent first, at most `limit`.
    pub async fn sessions(&self, limit: usize) -> anyhow::Result<Vec<Session>> {
        match self {
            Source::Db(path) => {
                let path = path.clone();
                tokio::task::spawn_blocking(move || db_sessions(&path, None, limit)).await?
            }
            Source::Api(hermes, agent, _) => {
                let params = [
                    ("limit", limit.min(200).to_string()),
                    ("include_children", "1".into()),
                    ("include_hidden", "1".into()),
                ];
                let rows = hermes.list_sessions_with(agent, &params).await?;
                let mut out: Vec<Session> = rows.iter().filter_map(Session::of_api).collect();
                out.sort_by(|a, b| b.last_active.total_cmp(&a.last_active));
                Ok(out)
            }
        }
    }

    /// Every session the agent's Hermes keeps, read page after page (its API serves at most `PAGE` at a time), most
    /// recent first; at most `ALL_MAX`. A Hermes that ignores `offset` gives the same page again: the reading stops.
    pub async fn all_sessions(&self) -> anyhow::Result<Vec<Session>> {
        let Source::Api(hermes, agent, _) = self else { return self.sessions(ALL_MAX).await };
        let mut out: Vec<Session> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        while out.len() < ALL_MAX {
            let params = [
                ("limit", PAGE.to_string()),
                ("offset", seen.len().to_string()),
                ("include_children", "1".into()),
                ("include_hidden", "1".into()),
            ];
            let rows = hermes.list_sessions_with(agent, &params).await?;
            let before = out.len();
            out.extend(rows.iter().filter_map(Session::of_api).filter(|s| seen.insert(s.id.clone())));
            if rows.is_empty() || out.len() == before {
                break;
            }
        }
        out.sort_by(|a, b| b.last_active.total_cmp(&a.last_active));
        Ok(out)
    }

    /// One session (None when Hermes doesn't know it).
    pub async fn session(&self, id: &str) -> anyhow::Result<Option<Session>> {
        match self {
            Source::Db(path) => {
                let (path, id) = (path.clone(), id.to_string());
                Ok(tokio::task::spawn_blocking(move || db_sessions(&path, Some(&id), 1)).await??.pop())
            }
            Source::Api(hermes, agent, _) => match hermes.session(agent, id).await {
                Ok(row) => Ok(Session::of_api(&row).filter(|s| s.id == id)),
                Err(crate::hermes::HermesError::Http { status: 404, .. }) => Ok(None),
                Err(e) => Err(e.into()),
            },
        }
    }

    /// The session Hermes continued `parent` in, if any (the latest started).
    pub async fn continuation(&self, parent: &Session) -> anyhow::Result<Option<Session>> {
        let children: Vec<Session> = match self {
            Source::Db(path) => {
                let (path, id) = (path.clone(), parent.id.clone());
                tokio::task::spawn_blocking(move || db_children(&path, &id)).await??
            }
            Source::Api(..) => {
                self.sessions(200).await?.into_iter().filter(|s| s.parent.as_deref() == Some(&parent.id)).collect()
            }
        };
        Ok(children.into_iter().filter(|c| c.continues(parent)).max_by(|a, b| a.started_at.total_cmp(&b.started_at)))
    }

    /// A session's rows in display order. `tip`: the thread's live end (the API source reads it again when stale).
    pub async fn rows(&self, session: &str, tip: bool) -> anyhow::Result<Arc<Vec<Row>>> {
        match self {
            Source::Db(path) => {
                let (path, id) = (path.clone(), session.to_string());
                Ok(Arc::new(tokio::task::spawn_blocking(move || db_rows(&path, &id)).await??))
            }
            Source::Api(hermes, agent, cache) => {
                if let Some(kept) = cache.get(&agent.name, session, tip) {
                    return Ok(kept);
                }
                let rows = match api_rows(hermes, agent, session, false).await {
                    Ok(rows) => rows,
                    Err(crate::hermes::HermesError::Http { status: 404, .. }) => Vec::new(),
                    Err(e) => return Err(e.into()),
                };
                let rows = Arc::new(rows);
                cache.put(&agent.name, session, rows.clone());
                Ok(rows)
            }
        }
    }

    /// A row's content with its photos inline (`data:` URIs), for the attachment route.
    pub async fn content_with_images(&self, session: &str, row: i64) -> anyhow::Result<Option<Value>> {
        match self {
            Source::Db(path) => {
                let (path, id) = (path.clone(), session.to_string());
                let rows = tokio::task::spawn_blocking(move || db_rows(&path, &id)).await??;
                Ok(rows.into_iter().find(|r| r.row == row).map(|r| r.content))
            }
            Source::Api(hermes, agent, _) => {
                Ok(api_rows(hermes, agent, session, true).await?.into_iter().find(|r| r.row == row).map(|r| r.content))
            }
        }
    }

    pub fn forget(&self, session: &str) {
        if let Source::Api(_, agent, cache) = self {
            cache.forget(&agent.name, session);
        }
    }
}

/// A segment's own rows through Hermes' API, every page (Hermes gives the lineage root → `session`: the rows of the
/// sessions before it are left out). Stops on a short page, or one that brings nothing new (a Hermes without paging).
async fn api_rows(
    hermes: &HermesClient,
    agent: &AgentConfig,
    session: &str,
    images: bool,
) -> Result<Vec<Row>, crate::hermes::HermesError> {
    let (mut rows, mut seen, mut offset) = (Vec::new(), std::collections::HashSet::new(), 0);
    for _ in 0..200 {
        let page = hermes.session_messages_page(agent, session, offset, images).await?;
        let size = page.len();
        let mut fresh = false;
        for item in &page {
            let key = item.get("id").map(Value::to_string).unwrap_or_else(|| format!("@{}", offset + seen.len()));
            if !seen.insert(key) {
                continue;
            }
            fresh = true;
            if item.get("session_id").and_then(Value::as_str).is_none_or(|s| s == session) {
                rows.push(Row::of_api(rows.len(), item));
            }
        }
        offset += size;
        if size < 500 || !fresh {
            break;
        }
    }
    Ok(rows)
}

// -- state.db -------------------------------------------------------------------------------------------------------

fn open(path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    check_schema(path, &conn);
    Ok(conn)
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement.query_map([], |row| row.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
    Ok(names)
}

/// `col` when the table has it, else `fallback` (a constant), as a select expression.
fn pick(cols: &[String], col: &str, fallback: &str) -> String {
    if cols.iter().any(|c| c == col) {
        col.to_string()
    } else {
        format!("{fallback} AS {col}")
    }
}

fn session_select(cols: &[String]) -> String {
    format!(
        "SELECT id, {}, {}, {}, {}, {}, {}, {}, {} FROM sessions",
        pick(cols, "parent_session_id", "NULL"),
        pick(cols, "title", "NULL"),
        pick(cols, "source", "NULL"),
        pick(cols, "model_config", "NULL"),
        pick(cols, "end_reason", "NULL"),
        pick(cols, "started_at", "0"),
        pick(cols, "hidden", "0"),
        if cols.iter().any(|c| c == "ended_at") { "COALESCE(ended_at, started_at) AS last" } else { "0 AS last" },
    )
}

fn session_of(row: &rusqlite::Row<'_>, has_end_reason: bool) -> rusqlite::Result<Session> {
    let num = |i: usize| -> f64 {
        match row.get_ref(i) {
            Ok(rusqlite::types::ValueRef::Real(f)) => f,
            Ok(rusqlite::types::ValueRef::Integer(n)) => n as f64,
            _ => 0.0,
        }
    };
    Ok(Session {
        id: row.get(0)?,
        parent: row.get::<_, Option<String>>(1)?.filter(|p| !p.is_empty()),
        title: row.get(2)?,
        source: row.get(3)?,
        model_config: row.get(4)?,
        end_reason: row.get(5)?,
        has_end_reason,
        started_at: num(6),
        last_active: num(8).max(num(6)),
        hidden: row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
    })
}

fn db_sessions(path: &PathBuf, id: Option<&str>, limit: usize) -> anyhow::Result<Vec<Session>> {
    let conn = open(path)?;
    let cols = columns(&conn, "sessions")?;
    let has_end = cols.iter().any(|c| c == "end_reason");
    let mut sql = session_select(&cols);
    let order = if cols.iter().any(|c| c == "started_at") { "started_at DESC" } else { "rowid DESC" };
    let rows = match id {
        Some(id) => {
            sql.push_str(" WHERE id = ?1");
            conn.query_row(&sql, [id], |r| session_of(r, has_end)).optional()?.into_iter().collect()
        }
        None => {
            sql.push_str(&format!(" ORDER BY {order} LIMIT {limit}"));
            let mut statement = conn.prepare(&sql)?;
            let found = statement.query_map([], |r| session_of(r, has_end))?.collect::<Result<Vec<_>, _>>()?;
            found
        }
    };
    Ok(rows)
}

fn db_children(path: &PathBuf, parent: &str) -> anyhow::Result<Vec<Session>> {
    let conn = open(path)?;
    let cols = columns(&conn, "sessions")?;
    if !cols.iter().any(|c| c == "parent_session_id") {
        return Ok(Vec::new());
    }
    let has_end = cols.iter().any(|c| c == "end_reason");
    let sql = format!("{} WHERE parent_session_id = ?1", session_select(&cols));
    let mut statement = conn.prepare(&sql)?;
    let found = statement.query_map([parent], |r| session_of(r, has_end))?.collect::<Result<Vec<_>, _>>()?;
    Ok(found)
}

fn db_rows(path: &PathBuf, session: &str) -> anyhow::Result<Vec<Row>> {
    let conn = open(path)?;
    let cols = columns(&conn, "messages")?;
    let has = |c: &str| cols.iter().any(|x| x == c);
    let order = if has("display_order") { "COALESCE(display_order, id), id" } else { "id" };
    let later = if ["message_uid", "active", "compacted", "display_order"].iter().all(|c| has(c)) {
        later_tool_copy_sql(has("display_metadata"))
    } else {
        "0".to_string()
    };
    let sql = format!(
        "SELECT id, role, content, timestamp, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {later} FROM messages AS m \
         WHERE session_id = ?1 ORDER BY {order}",
        pick(&cols, "message_uid", "NULL"),
        pick(&cols, "tool_calls", "NULL"),
        pick(&cols, "tool_call_id", "NULL"),
        pick(&cols, "tool_name", "NULL"),
        pick(&cols, "display_kind", "NULL"),
        pick(&cols, "reasoning", "NULL"),
        pick(&cols, "active", "1"),
        pick(&cols, "compacted", "0"),
        pick(&cols, "_compressed_summary", "0"),
        if has("display_order") { "COALESCE(display_order, id) AS ord" } else { "id AS ord" },
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement
        .query_map([session], |r| {
            let content: Option<String> = r.get(2)?;
            let content = match content {
                // a message with photos: Hermes keeps its parts as JSON
                Some(text) if text.starts_with("[{") => serde_json::from_str(&text).unwrap_or(Value::String(text)),
                Some(text) => Value::String(text),
                None => Value::Null,
            };
            let calls: Option<String> = r.get(5)?;
            let timestamp = match r.get_ref(3)? {
                rusqlite::types::ValueRef::Real(f) => f,
                rusqlite::types::ValueRef::Integer(n) => n as f64,
                _ => 0.0,
            };
            let active = r.get::<_, Option<i64>>(10)?.unwrap_or(1) != 0;
            let compacted = r.get::<_, Option<i64>>(11)?.unwrap_or(0) != 0;
            let later_copy = r.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0;
            Ok(Row {
                row: r.get(0)?,
                order: r.get(13)?,
                uid: r.get::<_, Option<String>>(4)?.filter(|u| !u.is_empty()),
                role: r.get::<_, Option<String>>(1)?.unwrap_or_default().to_lowercase(),
                content,
                tool_calls: calls.and_then(|c| serde_json::from_str(&c).ok()).filter(|v: &Value| !v.is_null()),
                tool_call_id: r.get(6)?,
                tool_name: r.get(7)?,
                timestamp,
                display_kind: r.get(8)?,
                reasoning: r.get::<_, Option<String>>(9)?.filter(|t| !t.is_empty()),
                shown: (active || compacted) && !later_copy,
                summary: r.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Whether row `m` is a later copy of an archived tool result: 1 or 0. Hermes v0.23 (#144938, commit 109a8f7c03,
/// hermes_state_compaction_display.py `not_a_later_tool_copy_sql`, the same SQL): a compaction carries a tool result
/// forward as a shortened stub with the same `message_uid` and timestamp, and its display reads skip that stub when
/// an archived (`compacted`), display-visible (not `model_only`) tool row of the same session, uid and timestamp
/// holds an EARLIER slot (`display_order`): the full original is what a reader sees. Identity only, never bodies; a
/// row without a slot is never skipped (NULL compares false); an in-place prune keeps its row's slot, so its stub
/// still shows. Read here for both Hermes versions: on 0.22.5 the stub's shortened words would otherwise be the ones
/// the timeline shows for that message (timeline.rs `in_place_compaction`, the newest shown copy). The API source
/// needs nothing: from 0.23 Hermes' own display list already leaves the stub out. What a compaction's summary row
/// tells a reader (agent/compaction_display.py `compaction_summary`) is model-facing; the timeline shows the
/// messages themselves, as before.
fn later_tool_copy_sql(has_metadata: bool) -> String {
    let visible = if has_metadata {
        " AND COALESCE(json_extract(CASE WHEN json_valid(original.display_metadata) THEN original.display_metadata \
         ELSE json_object() END, '$.model_only'), 0) = 0"
    } else {
        ""
    };
    format!(
        "CASE WHEN m.role = 'tool' AND m.message_uid IS NOT NULL AND m.message_uid <> '' AND EXISTS (SELECT 1 FROM \
         messages AS original WHERE original.session_id = m.session_id AND original.timestamp = m.timestamp \
         AND original.message_uid = m.message_uid AND original.role = 'tool' AND original.active = 0 \
         AND original.compacted = 1 AND +original.display_order < m.display_order{visible}) THEN 1 ELSE 0 END"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, parent: Option<&str>, end: Option<&str>, config: Option<&str>) -> Session {
        Session {
            id: id.into(),
            parent: parent.map(str::to_string),
            end_reason: end.map(str::to_string),
            has_end_reason: true,
            model_config: config.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn only_a_compression_goes_on_in_a_child() {
        let parent = session("a", None, Some("compression"), None);
        assert!(session("b", Some("a"), None, None).continues(&parent));
        assert!(!session("b", Some("x"), None, None).continues(&parent));
        assert!(!session("b", Some("a"), None, Some(r#"{"_delegate_from": "a"}"#)).continues(&parent));
        assert!(!session("b", Some("a"), None, Some(r#"{"_branched_from": "a"}"#)).continues(&parent));
        let mut tool = session("b", Some("a"), None, None);
        tool.source = Some("tool".into());
        assert!(!tool.continues(&parent));
        // a sub-agent's run started from a session that was not compressed
        assert!(!session("s", Some("a"), None, None).continues(&session("a", None, None, None)));
        // a store without end_reason: any plain child
        let mut old = session("a", None, None, None);
        old.has_end_reason = false;
        assert!(session("b", Some("a"), None, None).continues(&old));
    }

    /// The pinned Hermes' state.db (tests/golden/hermes_state_schema.sql, from scripts/hermes-schema-check.py
    /// --write) has every column the bridge reads; a store that lost one is reported, and the bridge's own reads
    /// run on a store made of exactly those columns (none of its SQL reads another one).
    #[test]
    fn the_bridge_reads_only_columns_hermes_has() {
        let golden = include_str!("../../tests/golden/hermes_state_schema.sql")
            .replace("REFERENCES system_prompts(hash)", "REFERENCES sessions(id)");
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&golden).unwrap();
        assert_eq!(missing_columns(&conn).unwrap(), Vec::<String>::new(), "Hermes' schema changed: see STATE_DB_READS");
        let dir = std::env::temp_dir().join(format!("state-schema-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.db");
        let exact = Connection::open(&path).unwrap();
        for (table, cols) in STATE_DB_READS {
            exact.execute_batch(&format!("CREATE TABLE {table} ({});", cols.join(", "))).unwrap();
        }
        exact
            .execute_batch(
                "INSERT INTO sessions (id, started_at, ended_at, hidden) VALUES ('a', 1, 2, 0);
                 INSERT INTO messages (id, session_id, role, content, timestamp, message_uid, active, compacted,
                   _compressed_summary, display_order) VALUES (1, 'a', 'user', 'bonjour', 1, 'u1', 1, 0, 0, 1);",
            )
            .unwrap();
        assert_eq!(db_sessions(&path, None, 10).unwrap()[0].id, "a");
        assert_eq!(db_rows(&path, "a").unwrap()[0].uid.as_deref(), Some("u1"));
        exact.execute_batch("ALTER TABLE sessions DROP COLUMN hidden; ALTER TABLE messages DROP COLUMN message_uid;").unwrap();
        assert_eq!(missing_columns(&exact).unwrap(), ["sessions.hidden", "messages.message_uid"]);
        assert_eq!(db_rows(&path, "a").unwrap()[0].uid, None, "still read, with a neutral value");
    }

    /// Hermes #144938 (v0.23): a compaction's shortened copy of a tool result (same uid and timestamp, a later slot)
    /// is not shown while its archived original is; the call's card reads the original's full output. A store of the
    /// pinned Hermes' schema (tests/golden), rows as v0.23.1 writes them.
    #[test]
    fn a_compacted_tool_result_shows_once_as_its_archived_original() {
        let golden = include_str!("../../tests/golden/hermes_state_schema.sql")
            .replace("REFERENCES system_prompts(hash)", "REFERENCES sessions(id)");
        let dir = std::env::temp_dir().join(format!("state-tool-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(&golden).unwrap();
        let call = |id: &str| format!(r#"[{{"id": "{id}", "function": {{"name": "terminal", "arguments": "{{}}"}}}}]"#);
        let mut insert = conn
            .prepare(
                "INSERT INTO messages (id, session_id, role, content, tool_calls, tool_call_id, timestamp, active, \
                 compacted, _compressed_summary, display_order, message_uid, display_metadata) \
                 VALUES (?1, 'a', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )
            .unwrap();
        conn.execute("INSERT INTO sessions (id, source, started_at) VALUES ('a', 'api_server', 1)", []).unwrap();
        type R<'a> = (i64, &'a str, &'a str, Option<String>, Option<&'a str>, f64, i64, i64, i64, Option<i64>, &'a str, Option<&'a str>);
        let rows: Vec<R> = vec![
            (1, "user", "liste les fichiers", None, None, 1.0, 0, 1, 0, Some(1), "u1", None),
            // 1. archived original, then the compaction's stub: the stub is left out
            (2, "assistant", "", Some(call("c1")), None, 2.0, 0, 1, 0, Some(2), "a1", None),
            (3, "tool", "FULL OUTPUT: a.txt b.txt c.txt", None, Some("c1"), 3.0, 0, 1, 0, Some(3), "t1", None),
            // 2. an original only the model sees: its stub stays shown
            (4, "assistant", "", Some(call("c2")), None, 4.0, 0, 1, 0, Some(4), "a2", None),
            (5, "tool", "model only", None, Some("c2"), 5.0, 0, 1, 0, Some(5), "t2", Some(r#"{"model_only": true}"#)),
            // 3. an in-place prune: same slot, the live stub shows
            (6, "assistant", "", Some(call("c3")), None, 6.0, 0, 1, 0, Some(6), "a3", None),
            (7, "tool", "pruned original", None, Some("c3"), 7.0, 0, 1, 0, Some(7), "t3", None),
            // 4. another timestamp: not a copy
            (8, "assistant", "", Some(call("c4")), None, 8.0, 0, 1, 0, Some(8), "a4", None),
            (9, "tool", "first run", None, Some("c4"), 9.0, 0, 1, 0, Some(9), "t4", None),
            // 5. an original without a slot (legacy, never backfilled): never decides
            (10, "assistant", "", Some(call("c5")), None, 10.0, 0, 1, 0, Some(10), "a5", None),
            (11, "tool", "legacy", None, Some("c5"), 11.0, 0, 1, 0, None, "t5", None),
            (12, "user", "[CONTEXT COMPACTION] …", None, None, 12.0, 1, 0, 1, Some(12), "s1", None),
            (13, "tool", "[output cut: 3 files]", None, Some("c1"), 3.0, 1, 0, 0, Some(13), "t1", None),
            (14, "tool", "[cut]", None, Some("c2"), 5.0, 1, 0, 0, Some(14), "t2", None),
            (15, "tool", "[pruned]", None, Some("c3"), 7.0, 1, 0, 0, Some(7), "t3", None),
            (16, "tool", "second run", None, Some("c4"), 9.5, 1, 0, 0, Some(16), "t4", None),
            (17, "tool", "[legacy cut]", None, Some("c5"), 11.0, 1, 0, 0, Some(17), "t5", None),
        ];
        for r in &rows {
            insert
                .execute(rusqlite::params![r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9, r.10, r.11])
                .unwrap();
        }
        let read = db_rows(&path, "a").unwrap();
        let shown = |row: i64| read.iter().find(|r| r.row == row).unwrap().shown;
        assert!(shown(3) && !shown(13), "the stub of an archived original is left out");
        assert!(shown(14), "an original only the model sees decides nothing");
        assert!(shown(15), "an in-place prune keeps its slot: its stub shows");
        assert!(shown(16) && shown(17), "another timestamp, an original without a slot: not a copy");
        // the timeline: each call's card reads its result once, the full original for c1
        let items = crate::threads::timeline::segment_items("coach", 0, "a", &read, &Default::default(), &mut String::new());
        let result = |call: &str| {
            let tool = |i: &crate::threads::timeline::Item| i.body.get("tool").cloned().unwrap_or(Value::Null);
            items
                .iter()
                .find(|i| i.body.get("kind") == Some(&Value::from("tool")) && tool(i)["call_id"] == call)
                .and_then(|i| tool(i)["result"].as_str().map(str::to_string))
        };
        assert_eq!(result("c1").as_deref(), Some("FULL OUTPUT: a.txt b.txt c.txt"));
        assert_eq!(result("c2").as_deref(), Some("[cut]"));
        assert_eq!(result("c4").as_deref(), Some("second run"));
        // a store without the slot column: nothing is left out (the rule needs it)
        drop(insert);
        conn.execute_batch("ALTER TABLE messages DROP COLUMN display_order;").unwrap();
        assert!(db_rows(&path, "a").unwrap().iter().all(|r| r.shown || r.summary));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
