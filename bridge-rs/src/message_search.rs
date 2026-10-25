//! The apps' search through every conversation of the person's agents, not only the latest one.
//!
//! - `GET /v1/search/messages?q=…[&agent=…][&limit=…]` -> `{"query", "results": [...], "searched", "unsearched",
//!   "partial"}`, newest first. Each result: `agent`, `session_id` (the conversation as the app lists it: a
//!   compression continuation is shown under its first session), `session_title`, `message_id`, `role`
//!   (`person` or `agent`), `timestamp` (Unix seconds), `date` (ISO 8601), `snippet` (plain) and `parts`
//!   (the snippet cut into `{"text", "hit"}` pieces, the matched words with `hit: true`).
//!
//! The rule is the apps' SearchMatch: every word of the query must be in the message, case, accents and width
//! aside (« ecole » finds « École », a word may be part of a longer one).
//!
//! Hermes keeps each profile's sessions in `<profile>/state.db` (SQLite). Its FTS5 indexes (`messages_fts`, word
//! tokens; `messages_fts_trigram`) match whole tokens or need three letters per term, and are rebuilt in steps
//! (high-water marks), so a hit list from them would differ from what the apps match locally: the store is read
//! read-only and scanned newest first instead, stopping at `limit` hits per agent or at the time cap (`partial`).
//! Only what the person and the agent said is searched: no tool outputs, system rows, hidden scaffolding
//! (`display_kind = 'hidden'`, model-only rows), compression summaries, rewound rows or sub-agent runs; sessions
//! Bot Mode hides are skipped except the agent's « Bot Chat » (the app's « Discussion »).
//!
//! Only the agents of this bridge whose profile folder it knows are searched (the ones it manages, or set up by
//! hand with `host_home`); the others are listed in `unsearched` and the app matches their latest conversation as
//! before. Profiles found on the server but not claimed are never searched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError, QueryMap};

pub const DEFAULT_LIMIT: usize = 40;
pub const MAX_LIMIT: usize = 100;
/// The whole search stops after this long; what was found is returned with `partial: true`.
pub const TIME_CAP: Duration = Duration::from_millis(1500);
/// Characters of a snippet, about.
const SNIPPET_CHARS: usize = 140;
const BEFORE_CHARS: usize = 40;
const BOT_CHAT_TITLE: &str = "Bot Chat";
/// A parent ending this way makes its child a reset session the app lists (Hermes' `_RESET_END_REASONS`).
const RESET_END_REASONS: [&str; 6] =
    ["session_reset", "session_switch", "idle", "daily", "suspended", "resume_pending_expired"];

pub fn router() -> Router<AppState> {
    Router::new().route("/search/messages", get(search_route))
}

async fn search_route(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let query = web::query_str(&q, "q", true, Some(200))?.unwrap_or_default();
    let limit = web::query_int(&q, "limit", Some(DEFAULT_LIMIT as i64), Some(1), Some(MAX_LIMIT as i64))?
        .unwrap_or(DEFAULT_LIMIT as i64) as usize;
    let config = state.config();
    let agents: Vec<AgentConfig> = match q.get("agent").filter(|a| !a.trim().is_empty()) {
        Some(name) => vec![config.agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))?],
        None => config.agents.values().cloned().collect(),
    };
    let stores: Vec<(String, Option<PathBuf>)> =
        agents.iter().map(|a| (a.name.clone(), store_path(&config, &a.name))).collect();
    let words = words(&query);
    let found = tokio::task::spawn_blocking(move || search_stores(&stores, &words, limit, TIME_CAP))
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let mut out = found.to_json();
    out["query"] = json!(query);
    Ok(web::ok(out))
}

/// The agent's Hermes store when this bridge knows its profile folder and the store is there.
pub fn store_path(config: &Config, agent: &str) -> Option<PathBuf> {
    let agent = crate::agents_admin::editable(config, agent).ok()?;
    let path = crate::agents_admin::profile_folder(config, &agent).join("state.db");
    path.is_file().then_some(path)
}

// -- folding: the apps' SearchMatch.fold ------------------------------------------------------------------------

fn fold_char(c: char, out: &mut String) {
    let c = match c {
        '\u{2019}' => '\'',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{3000}' => ' ',
        _ => c,
    };
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
        return;
    }
    for lower in c.to_lowercase() {
        for d in std::iter::once(lower).nfd() {
            if !matches!(
                get_general_category(d),
                GeneralCategory::NonspacingMark | GeneralCategory::EnclosingMark | GeneralCategory::SpacingMark
            ) {
                out.push(d);
            }
        }
    }
}

/// Lower case, no accents, `’` as `'`.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        fold_char(c, &mut out);
    }
    out
}

/// The typed words, folded, empty ones dropped.
pub fn words(query: &str) -> Vec<String> {
    fold(query).split_whitespace().map(str::to_string).collect()
}

/// The text folded, with for each byte of the folded text the index of the original char it came from.
fn fold_mapped(chars: &[char]) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(chars.len());
    let mut map = Vec::with_capacity(chars.len());
    let mut buf = String::new();
    for (i, c) in chars.iter().enumerate() {
        buf.clear();
        fold_char(*c, &mut buf);
        out.push_str(&buf);
        map.extend(std::iter::repeat_n(i, buf.len()));
    }
    (out, map)
}

/// Whether every word is in the text (the cheap check on every row).
fn matches_all(text: &str, words: &[String]) -> bool {
    if text.is_ascii() {
        let lower = text.to_ascii_lowercase();
        words.iter().all(|w| lower.contains(w.as_str()))
    } else {
        let folded = fold(text);
        words.iter().all(|w| folded.contains(w.as_str()))
    }
}

/// Runs of whitespace as one space, trimmed.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The snippet around the first matched word, cut into plain and matched pieces.
pub fn snippet(text: &str, words: &[String]) -> (String, Vec<(String, bool)>) {
    let chars: Vec<char> = one_line(text).chars().collect();
    let (folded, map) = fold_mapped(&chars);
    // every occurrence of every word, as [start, end) in chars
    let mut hits: Vec<(usize, usize)> = Vec::new();
    for word in words.iter().filter(|w| !w.is_empty()) {
        let mut from = 0;
        while let Some(at) = folded[from..].find(word.as_str()) {
            let start = from + at;
            let end = start + word.len();
            hits.push((map[start], map[end - 1] + 1));
            from = start + word.len().max(1);
        }
    }
    hits.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in hits {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    let first = merged.first().map(|h| h.0).unwrap_or(0);
    let mut start = first.saturating_sub(BEFORE_CHARS);
    if start > 0 {
        // to the start of a word
        while start < first && !chars[start - 1].is_whitespace() {
            start += 1;
        }
    }
    let mut end = (start + SNIPPET_CHARS).min(chars.len());
    if end < chars.len() {
        let mut back = end;
        while back > first + 1 && !chars[back].is_whitespace() {
            back -= 1;
        }
        if back > start + SNIPPET_CHARS / 2 {
            end = back;
        }
    }
    let mut parts: Vec<(String, bool)> = Vec::new();
    let mut push = |text: String, hit: bool| {
        if text.is_empty() {
            return;
        }
        match parts.last_mut() {
            Some(last) if last.1 == hit => last.0.push_str(&text),
            _ => parts.push((text, hit)),
        }
    };
    if start > 0 {
        push("…".into(), false);
    }
    let mut at = start;
    for (s, e) in merged.iter().copied() {
        let (s, e) = (s.max(start), e.min(end));
        if s >= e {
            continue;
        }
        push(chars[at..s].iter().collect(), false);
        push(chars[s..e].iter().collect(), true);
        at = e;
    }
    if at < end {
        push(chars[at..end].iter().collect(), false);
    }
    if end < chars.len() {
        push("…".into(), false);
    }
    let plain = parts.iter().map(|p| p.0.as_str()).collect();
    (plain, parts)
}

/// The text of a message: its content, or the text parts of a multimodal one (a JSON list).
fn message_text(content: &str) -> String {
    let trimmed = content.trim_start();
    if trimmed.starts_with('[') {
        if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(trimmed) {
            let texts: Vec<&str> = items
                .iter()
                .filter_map(|i| match i {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(o) if o.get("type").and_then(Value::as_str) == Some("text") => {
                        o.get("text").and_then(Value::as_str)
                    }
                    _ => None,
                })
                .collect();
            return texts.join(" ");
        }
    }
    content.to_string()
}

// -- the store --------------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Hit {
    pub agent: String,
    pub session_id: String,
    pub session_title: Option<String>,
    pub message_id: i64,
    pub role: &'static str,
    pub timestamp: f64,
    pub snippet: String,
    pub parts: Vec<(String, bool)>,
}

impl Hit {
    fn to_json(&self) -> Value {
        let date = chrono::DateTime::from_timestamp_micros((self.timestamp * 1e6) as i64)
            .map(crate::store::iso)
            .unwrap_or_default();
        json!({
            "agent": self.agent, "session_id": self.session_id, "session_title": self.session_title,
            "message_id": self.message_id, "role": self.role, "timestamp": self.timestamp, "date": date,
            "snippet": self.snippet,
            "parts": self.parts.iter().map(|(t, h)| json!({"text": t, "hit": h})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Default)]
pub struct Found {
    pub hits: Vec<Hit>,
    pub searched: Vec<String>,
    pub unsearched: Vec<String>,
    pub partial: bool,
}

impl Found {
    pub fn to_json(&self) -> Value {
        json!({
            "results": self.hits.iter().map(Hit::to_json).collect::<Vec<_>>(),
            "searched": self.searched, "unsearched": self.unsearched, "partial": self.partial,
        })
    }
}

/// One store's hits and whether the time cap cut it short.
type StoreResult = anyhow::Result<(Vec<Hit>, bool)>;

/// Every store, one thread each, merged newest first.
pub fn search_stores(stores: &[(String, Option<PathBuf>)], words: &[String], limit: usize, cap: Duration) -> Found {
    let mut found = Found::default();
    if words.is_empty() {
        found.searched = stores.iter().filter(|s| s.1.is_some()).map(|s| s.0.clone()).collect();
        found.unsearched = stores.iter().filter(|s| s.1.is_none()).map(|s| s.0.clone()).collect();
        return found;
    }
    let deadline = Instant::now() + cap;
    let results: Vec<(String, Option<StoreResult>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = stores
            .iter()
            .map(|(agent, path)| {
                let handle = path.as_ref().map(|p| scope.spawn(move || search_store(agent, p, words, limit, deadline)));
                (agent.clone(), handle)
            })
            .collect();
        handles
            .into_iter()
            .map(|(agent, h)| (agent, h.map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("panicked"))))))
            .collect()
    });
    for (agent, result) in results {
        match result {
            None => found.unsearched.push(agent),
            Some(Err(exc)) => {
                tracing::warn!(target: "bipbridge::search", agent = %agent, error = %exc, "store not searched");
                found.unsearched.push(agent);
            }
            Some(Ok((hits, partial))) => {
                found.partial |= partial;
                found.hits.extend(hits);
                found.searched.push(agent);
            }
        }
    }
    found.hits.sort_by(|a, b| b.timestamp.partial_cmp(&a.timestamp).unwrap_or(std::cmp::Ordering::Equal));
    if found.hits.len() > limit {
        found.hits.truncate(limit);
    }
    found
}

fn columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names)
}

struct SessionRow {
    parent: Option<String>,
    end_reason: Option<String>,
    title: Option<String>,
    hidden: bool,
    marks: (bool, bool, bool), // _branched_from, _reset_from, _delegate_from
}

/// Where a session's messages show in the app: the session itself, its first session for a compression
/// continuation, None for a sub-agent run or a session hidden from the person.
fn listed_session(id: &str, sessions: &HashMap<String, SessionRow>) -> Option<String> {
    let mut current = id.to_string();
    for _ in 0..64 {
        let row = sessions.get(&current)?;
        if row.marks.2 {
            return None; // a sub-agent's run
        }
        let Some(parent_id) = row.parent.as_deref().filter(|p| !p.is_empty()) else { break };
        let Some(parent) = sessions.get(parent_id) else { break };
        let parent_end = parent.end_reason.as_deref().unwrap_or("");
        if parent_end == "compression" {
            current = parent_id.to_string();
            continue;
        }
        if row.marks.0 || row.marks.1 || parent_end == "branched" || RESET_END_REASONS.contains(&parent_end) {
            break; // a branch or a reset: listed on its own
        }
        return None; // a sub-agent's run (older stores, no marker)
    }
    let row = sessions.get(&current)?;
    if row.hidden && row.title.as_deref().map(str::trim) != Some(BOT_CHAT_TITLE) {
        return None;
    }
    Some(current)
}

fn markers(model_config: Option<&str>) -> (bool, bool, bool) {
    let Some(text) = model_config.filter(|t| t.contains("_from")) else {
        return (false, false, false);
    };
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(o)) => {
            let set = |k: &str| o.get(k).is_some_and(|v| !v.is_null());
            (set("_branched_from"), set("_reset_from"), set("_delegate_from"))
        }
        _ => (false, false, false),
    }
}

/// One agent's store, newest message first: its hits (at most `limit`) and whether the time cap cut it short.
pub fn search_store(
    agent: &str,
    path: &Path,
    words: &[String],
    limit: usize,
    deadline: Instant,
) -> anyhow::Result<(Vec<Hit>, bool)> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    conn.busy_timeout(Duration::from_millis(300))?;
    let scols = columns(&conn, "sessions")?;
    let mcols = columns(&conn, "messages")?;
    let has = |cols: &[String], c: &str| cols.iter().any(|x| x == c);
    let scol = |c: &str| if has(&scols, c) { c.to_string() } else { "NULL".to_string() };
    let session_sql = format!(
        "SELECT id, {}, {}, {}, {}, {} FROM sessions",
        scol("parent_session_id"),
        scol("end_reason"),
        scol("title"),
        if has(&scols, "hidden") { "hidden" } else { "0" },
        scol("model_config"),
    );
    let mut sessions: HashMap<String, SessionRow> = HashMap::new();
    {
        let mut stmt = conn.prepare(&session_sql)?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let model_config: Option<String> = row.get(5).ok().flatten();
            sessions.insert(
                id,
                SessionRow {
                    parent: row.get(1).ok().flatten(),
                    end_reason: row.get(2).ok().flatten(),
                    title: row.get(3).ok().flatten(),
                    hidden: row.get::<_, Option<i64>>(4).ok().flatten().unwrap_or(0) != 0,
                    marks: markers(model_config.as_deref()),
                },
            );
        }
    }
    let mut filters = vec!["role IN ('user', 'assistant')".to_string(), "content IS NOT NULL".to_string()];
    if has(&mcols, "active") && has(&mcols, "compacted") {
        filters.push("(active = 1 OR compacted = 1)".into());
    } else if has(&mcols, "active") {
        filters.push("active = 1".into());
    }
    if has(&mcols, "display_kind") {
        filters.push("COALESCE(display_kind, '') <> 'hidden'".into());
    }
    if has(&mcols, "_compressed_summary") {
        filters.push("_compressed_summary = 0".into());
    }
    let display_metadata = if has(&mcols, "display_metadata") { "display_metadata" } else { "NULL" };
    let sql = format!(
        "SELECT id, session_id, role, content, timestamp, {display_metadata} FROM messages WHERE {} ORDER BY id DESC",
        filters.join(" AND ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut listed: HashMap<String, Option<String>> = HashMap::new();
    let mut hits = Vec::new();
    let mut seen = 0usize;
    while let Some(row) = rows.next()? {
        seen += 1;
        if seen.is_multiple_of(512) && Instant::now() >= deadline {
            return Ok((hits, true));
        }
        let content: String = match row.get_ref(3)? {
            rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
            _ => continue,
        };
        if !matches_all(&content, words) {
            continue;
        }
        let text = message_text(&content);
        if !matches_all(&text, words) {
            continue;
        }
        if let Ok(Some(meta)) = row.get::<_, Option<String>>(5) {
            if meta.contains("model_only") {
                let model_only = serde_json::from_str::<Value>(&meta)
                    .ok()
                    .and_then(|v| v.get("model_only").cloned())
                    .is_some_and(|v| crate::py::truthy(&v));
                if model_only {
                    continue;
                }
            }
        }
        let session_id: String = row.get(1)?;
        let shown = listed.entry(session_id.clone()).or_insert_with(|| listed_session(&session_id, &sessions)).clone();
        let Some(shown) = shown else { continue };
        let role: String = row.get(2)?;
        let (snippet, parts) = snippet(&text, words);
        hits.push(Hit {
            agent: agent.to_string(),
            session_title: sessions.get(&shown).and_then(|s| s.title.clone()).filter(|t| !t.trim().is_empty()),
            session_id: shown,
            message_id: row.get(0)?,
            role: if role == "user" { "person" } else { "agent" },
            timestamp: row.get::<_, Option<f64>>(4)?.unwrap_or(0.0),
            snippet,
            parts,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok((hits, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn make_store(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT, parent_session_id TEXT, end_reason TEXT,
                 title TEXT, hidden INTEGER NOT NULL DEFAULT 0, model_config TEXT, started_at REAL);
             CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, role TEXT, content TEXT,
                 timestamp REAL, active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0,
                 display_kind TEXT, display_metadata TEXT, _compressed_summary INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn
    }

    fn session(conn: &Connection, id: &str, parent: Option<&str>, end: Option<&str>, title: Option<&str>, hidden: i64) {
        conn.execute(
            "INSERT INTO sessions (id, source, parent_session_id, end_reason, title, hidden) VALUES (?1, 'api', ?2, ?3, ?4, ?5)",
            rusqlite::params![id, parent, end, title, hidden],
        )
        .unwrap();
    }

    fn message(conn: &Connection, session: &str, role: &str, content: &str, ts: f64) {
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session, role, content, ts],
        )
        .unwrap();
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(30)
    }

    #[test]
    fn folding_matches_the_apps() {
        assert_eq!(fold("École d’été ＡＢ"), "ecole d'ete ab");
        assert_eq!(words("  Ecole   ÉTÉ "), vec!["ecole", "ete"]);
        assert!(matches_all("Rendez-vous à l'École", &words("ecole rendez")));
        assert!(!matches_all("Rendez-vous à l'École", &words("ecole piscine")));
        assert!(matches_all("plain ascii Text", &words("TEXT plain")));
    }

    #[test]
    fn snippet_marks_the_words_in_the_original_text() {
        let (plain, parts) = snippet("La réunion de l'École\nest à 9h", &words("ecole reunion"));
        assert_eq!(plain, "La réunion de l'École est à 9h");
        assert_eq!(
            parts,
            vec![
                ("La ".to_string(), false),
                ("réunion".to_string(), true),
                (" de l'".to_string(), false),
                ("École".to_string(), true),
                (" est à 9h".to_string(), false)
            ]
        );
        let long = format!("{} piscine {}", "mot ".repeat(40), "fin ".repeat(60));
        let (plain, parts) = snippet(&long, &words("piscine"));
        assert!(plain.starts_with('…') && plain.ends_with('…'), "{plain}");
        assert!(plain.chars().count() < 200);
        assert!(parts.iter().any(|p| p.1 && p.0 == "piscine"));
    }

    #[test]
    fn multimodal_content_is_its_text_parts() {
        assert_eq!(
            message_text(r#"[{"type":"text","text":"voici la photo"},{"type":"image_url","image_url":{"url":"x"}}]"#),
            "voici la photo"
        );
        assert_eq!(message_text("[pas du json"), "[pas du json");
    }

    #[test]
    fn the_store_is_searched_newest_first_without_the_hidden_parts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = make_store(&path);
        session(&conn, "s1", None, Some("compression"), Some("Vacances"), 0);
        session(&conn, "s1b", Some("s1"), None, None, 0); // compression continuation: shown as s1
        session(&conn, "sub", Some("s1b"), None, None, 0); // a sub-agent run: skipped
        session(&conn, "bot", None, None, Some("Bot Chat"), 1); // the Discussion: searched
        session(&conn, "peer", None, None, Some("peer"), 1); // Bot Mode's own: skipped
        message(&conn, "s1", "user", "On part à la mer en août ?", 1.0);
        message(&conn, "s1", "assistant", "Oui, la Mer du Nord", 2.0);
        message(&conn, "s1", "tool", "mer mer mer", 3.0);
        message(&conn, "s1b", "user", "Et la mer, il fera chaud ?", 4.0);
        message(&conn, "sub", "assistant", "mer (sous-agent)", 5.0);
        message(&conn, "bot", "user", "La mer depuis la Discussion", 6.0);
        message(&conn, "peer", "user", "mer entre agents", 7.0);
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, display_kind) VALUES ('s1', 'user', 'mer cachée', 8, 'hidden')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, active) VALUES ('s1', 'user', 'mer effacée', 9, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, _compressed_summary) VALUES ('s1b', 'user', 'résumé mer', 10, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (session_id, role, content, timestamp, display_metadata) VALUES ('s1b', 'user', 'mer modèle', 11, '{\"model_only\": true}')",
            [],
        )
        .unwrap();
        let (hits, partial) = search_store("coach", &path, &words("MER"), 10, far()).unwrap();
        assert!(!partial);
        let seen: Vec<(&str, &str, f64)> = hits.iter().map(|h| (h.session_id.as_str(), h.role, h.timestamp)).collect();
        assert_eq!(
            seen,
            vec![("bot", "person", 6.0), ("s1", "person", 4.0), ("s1", "agent", 2.0), ("s1", "person", 1.0)]
        );
        assert_eq!(hits[1].session_title.as_deref(), Some("Vacances"));
        let (hits, _) = search_store("coach", &path, &words("mer août"), 10, far()).unwrap();
        assert_eq!(hits.len(), 1);
        let (hits, _) = search_store("coach", &path, &words("mer"), 2, far()).unwrap();
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn stores_are_merged_and_missing_ones_listed() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.db");
        let b = dir.path().join("b.db");
        let ca = make_store(&a);
        session(&ca, "x", None, None, None, 0);
        message(&ca, "x", "user", "piscine lundi", 10.0);
        let cb = make_store(&b);
        session(&cb, "y", None, None, None, 0);
        message(&cb, "y", "assistant", "La piscine ferme", 20.0);
        let stores = vec![("a".to_string(), Some(a)), ("b".to_string(), Some(b)), ("c".to_string(), None)];
        let found = search_stores(&stores, &words("piscine"), 5, Duration::from_secs(5));
        assert_eq!(found.hits.iter().map(|h| h.agent.as_str()).collect::<Vec<_>>(), vec!["b", "a"]);
        assert_eq!(found.searched, vec!["a", "b"]);
        assert_eq!(found.unsearched, vec!["c"]);
        let json = found.to_json();
        assert_eq!(json["results"][0]["parts"][1], json!({"text": "piscine", "hit": true}));
        assert_eq!(json["results"][0]["date"], "1970-01-01T00:00:20.000Z");
    }

    /// 50k messages: `cargo test --release message_search -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn big_store_speed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let mut conn = make_store(&path);
        let words_fr = [
            "réunion",
            "école",
            "piscine",
            "vacances",
            "médecin",
            "rendez-vous",
            "courses",
            "anniversaire",
            "train",
            "voiture",
            "facture",
            "jardin",
            "dîner",
            "été",
            "hiver",
            "semaine",
            "projet",
            "budget",
            "photo",
            "lettre",
        ];
        let tx = conn.transaction().unwrap();
        for s in 0..500 {
            tx.execute(
                "INSERT INTO sessions (id, source, title) VALUES (?1, 'api', ?2)",
                rusqlite::params![format!("s{s}"), format!("Conversation {s}")],
            )
            .unwrap();
        }
        let mut seed: u64 = 7;
        for i in 0..50_000u64 {
            let mut text = String::new();
            for _ in 0..60 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push_str(words_fr[(seed >> 33) as usize % words_fr.len()]);
                text.push(' ');
            }
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            tx.execute(
                "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![format!("s{}", i % 500), role, text, i as f64],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(conn);
        for q in ["introuvable", "ecole piscine", "zzz yyy"] {
            let started = Instant::now();
            let (hits, partial) =
                search_store("a", &path, &words(q), 40, Instant::now() + Duration::from_secs(10)).unwrap();
            eprintln!("{q:?}: {} hits, partial {partial}, {:?}", hits.len(), started.elapsed());
        }
    }
}
