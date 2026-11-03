//! Where a thread's sessions and rows are read: the agent's Hermes store (`<profile>/state.db`, read-only, no call to
//! Hermes at all) when the bridge knows the profile folder, else Hermes' API. One agent always uses the same source,
//! so the ids made from its rows never change between reads.
//!
//! The API source caches what can't change: a segment that is not the tip any more (Hermes never writes into a
//! compressed session again) is read once; the tip is read again only after the thread was touched (a run's end, a
//! Boîte message, a notification) or `TIP_TTL` later. An app opening a conversation costs Hermes nothing then.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use crate::config::{AgentConfig, Config};
use crate::hermes::HermesClient;
use crate::Dict;

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
    /// The one rule for every reader (restore_reconcile.rs, cronwatch.rs, threads).
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
    /// Shown: active, or compacted (Hermes still displays a compacted row).
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
                let rows = match hermes.session_messages(agent, session).await {
                    Ok(rows) => rows,
                    Err(crate::hermes::HermesError::Http { status: 404, .. }) => Vec::new(),
                    Err(e) => return Err(e.into()),
                };
                let rows = Arc::new(rows.iter().enumerate().map(|(i, r)| Row::of_api(i, r)).collect::<Vec<_>>());
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
                let rows = hermes.session_messages_with_images(agent, session).await?;
                Ok(rows.iter().enumerate().map(|(i, r)| Row::of_api(i, r)).find(|r| r.row == row).map(|r| r.content))
            }
        }
    }

    pub fn forget(&self, session: &str) {
        if let Source::Api(_, agent, cache) = self {
            cache.forget(&agent.name, session);
        }
    }
}

// -- state.db -------------------------------------------------------------------------------------------------------

fn open(path: &PathBuf) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
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
    let sql = format!(
        "SELECT id, role, content, timestamp, {}, {}, {}, {}, {}, {}, {}, {}, {}, {} FROM messages WHERE session_id = ?1 \
         ORDER BY {order}",
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
                shown: active || compacted,
                summary: r.get::<_, Option<i64>>(12)?.unwrap_or(0) != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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
}
