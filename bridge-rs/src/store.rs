//! SQLite storage (devices, outbox, ntfy cursors). sqlite3 calls run in a worker thread.
//!
//! The same file, schema and queries as the Python bridge (`bridge.db`, WAL, mode 0600): an install switches
//! between the two without losing anything. Rows come back as JSON objects with the columns in table order, as
//! Python's `dict(row)`.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rusqlite::types::ValueRef;
use rusqlite::{params_from_iter, Connection};
use serde_json::{json, Map, Value};

use crate::py;

pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS devices (
    token TEXT PRIMARY KEY,
    environment TEXT NOT NULL,
    agent_ids TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS outbox (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    ntfy_id TEXT,
    title TEXT,
    text TEXT NOT NULL,
    created_at TEXT NOT NULL,
    sent_at TEXT,
    session_id TEXT,
    audio_path TEXT,
    UNIQUE (agent, ntfy_id)
);
CREATE INDEX IF NOT EXISTS outbox_created_at ON outbox (created_at);
CREATE TABLE IF NOT EXISTS cron_jobs (
    agent TEXT NOT NULL,
    job TEXT NOT NULL,
    name TEXT,
    notify INTEGER NOT NULL DEFAULT 1,
    last_seen TEXT,
    PRIMARY KEY (agent, job)
);
CREATE TABLE IF NOT EXISTS watched_runs (
    agent TEXT NOT NULL,
    run_id TEXT NOT NULL,
    session_id TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (agent, run_id)
);
CREATE TABLE IF NOT EXISTS ntfy_cursor (
    agent TEXT PRIMARY KEY,
    last_id TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
";

/// A row as Python's `dict(row)`.
pub type Row = Map<String, Value>;

pub fn utcnow() -> DateTime<Utc> {
    Utc::now()
}

/// Canonical, lexicographically sortable UTC timestamp: 2026-10-04T08:15:00.123Z.
pub fn iso(dt: DateTime<Utc>) -> String {
    format!("{}{:03}Z", dt.format("%Y-%m-%dT%H:%M:%S."), dt.timestamp_subsec_micros() / 1000)
}

/// ISO 8601 (with `Z` or an offset; naive = UTC) or Unix seconds. Err: Python's ValueError.
pub fn parse_since(value: &str) -> Result<DateTime<Utc>, String> {
    let value = value.trim();
    if let Ok(seconds) = py::py_float(&Value::String(value.to_string())) {
        if seconds.is_finite() {
            let micros = (seconds * 1_000_000.0).round() as i64;
            return Utc.timestamp_micros(micros).single().ok_or_else(|| "year is out of range".to_string());
        }
        if seconds.is_nan() {
            return Err("cannot convert float NaN to integer".into());
        }
        return Err("cannot convert float infinity to integer".into());
    }
    let mut text = value.to_string();
    if text.ends_with('Z') || text.ends_with('z') {
        text.truncate(text.len() - 1);
        text.push_str("+00:00");
    }
    fromisoformat(&text)
        .ok_or_else(|| format!("Invalid isoformat string: {}", py::py_repr(&Value::String(text.clone()))))
}

/// Python 3.11+'s `datetime.fromisoformat` (the forms an app or a person sends), as UTC.
pub fn fromisoformat(text: &str) -> Option<DateTime<Utc>> {
    let chars: Vec<char> = text.chars().collect();
    // The date: YYYY-MM-DD or YYYYMMDD.
    let (date, rest) = if chars.len() >= 10 && chars[4] == '-' && chars[7] == '-' {
        (NaiveDate::parse_from_str(&chars[..10].iter().collect::<String>(), "%Y-%m-%d").ok()?, &chars[10..])
    } else if chars.len() >= 8 && chars[..8].iter().all(|c| c.is_ascii_digit()) {
        (NaiveDate::parse_from_str(&chars[..8].iter().collect::<String>(), "%Y%m%d").ok()?, &chars[8..])
    } else {
        return None;
    };
    if rest.is_empty() {
        return Some(Utc.from_utc_datetime(&date.and_time(NaiveTime::MIN)));
    }
    // Any single character separates the date from the time.
    let time_text: String = rest[1..].iter().collect();
    let (time_part, offset_seconds) = split_offset(&time_text)?;
    let time = parse_time(&time_part)?;
    let naive = NaiveDateTime::new(date, time);
    let utc = naive - Duration::seconds(offset_seconds);
    Some(Utc.from_utc_datetime(&utc))
}

fn split_offset(text: &str) -> Option<(String, i64)> {
    if let Some(pos) = text.find(['+', '-']) {
        let (time, off) = text.split_at(pos);
        let sign = if off.starts_with('-') { -1 } else { 1 };
        let off = &off[1..];
        let digits: String = off.chars().filter(|c| c.is_ascii_digit()).collect();
        let (h, m, s) = match digits.len() {
            2 => (digits[..2].parse::<i64>().ok()?, 0, 0),
            4 => (digits[..2].parse::<i64>().ok()?, digits[2..4].parse::<i64>().ok()?, 0),
            6.. => {
                (digits[..2].parse::<i64>().ok()?, digits[2..4].parse::<i64>().ok()?, digits[4..6].parse::<i64>().ok()?)
            }
            _ => return None,
        };
        if h >= 24 || m >= 60 {
            return None;
        }
        Some((time.to_string(), sign * (h * 3600 + m * 60 + s)))
    } else {
        Some((text.to_string(), 0))
    }
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    let (main, frac) = match text.split_once(['.', ',']) {
        Some((a, b)) => (a, Some(b)),
        None => (text, None),
    };
    let digits: String = main.chars().filter(|c| *c != ':').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m, s) = match digits.len() {
        2 => (digits[..2].parse().ok()?, 0, 0),
        4 => (digits[..2].parse().ok()?, digits[2..4].parse().ok()?, 0),
        6 => (digits[..2].parse().ok()?, digits[2..4].parse().ok()?, digits[4..6].parse().ok()?),
        _ => return None,
    };
    let micros = match frac {
        Some(f) if !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()) => {
            let mut f6: String = f.chars().take(6).collect();
            while f6.len() < 6 {
                f6.push('0');
            }
            f6.parse::<u32>().ok()?
        }
        Some(_) => return None,
        None => 0,
    };
    NaiveTime::from_hms_micro_opt(h, m, s, micros)
}

struct Inner {
    path: String,
    conn: Mutex<Option<Connection>>,
}

/// An outbox message's `created_at`, a millisecond after the previous one when two land in the same millisecond.
/// The apps page with `since=<last created_at>` (strictly after): Python, slower, practically never stores two in one
/// millisecond; Rust does (two ntfy lines read at once), and the second would never be fetched.
fn outbox_stamp() -> String {
    static LAST: std::sync::Mutex<Option<DateTime<Utc>>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let mut now = utcnow();
    if let Some(previous) = *last {
        let next = previous + chrono::Duration::milliseconds(1);
        if iso(now) <= iso(previous) {
            now = next;
        }
    }
    *last = Some(now);
    iso(now)
}

/// The bridge's database. Cheap to clone (one connection, shared, behind a lock, as in Python).
#[derive(Clone)]
pub struct Store {
    inner: Arc<Inner>,
}

/// The optional columns of a new outbox message (`add_outbox`'s keyword arguments).
#[derive(Debug, Clone, Default)]
pub struct NewOutbox {
    pub title: Option<String>,
    pub ntfy_id: Option<String>,
    pub sent_at: Option<String>,
    pub session_id: Option<String>,
}

type Param = rusqlite::types::Value;

fn text(v: &str) -> Param {
    Param::Text(v.to_string())
}

fn opt(v: Option<&str>) -> Param {
    v.map(text).unwrap_or(Param::Null)
}

fn row_to_map(row: &rusqlite::Row<'_>, names: &[String]) -> rusqlite::Result<Row> {
    let mut map = Map::new();
    for (i, name) in names.iter().enumerate() {
        let value = match row.get_ref(i)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(n) => json!(n),
            ValueRef::Real(f) => json!(f),
            ValueRef::Text(t) => Value::String(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Value::String(String::from_utf8_lossy(b).into_owned()),
        };
        map.insert(name.clone(), value);
    }
    Ok(map)
}

impl Store {
    pub fn open(path: &str) -> anyhow::Result<Store> {
        if path != ":memory:" {
            let dir = py::dirname(path);
            py::makedirs(if dir.is_empty() { "." } else { &dir }, 0o700)
                .with_context(|| format!("cannot create {dir}"))?;
        }
        let conn = Connection::open(path).with_context(|| format!("cannot open {path}"))?;
        // Python's sqlite3 waits 5 s for a lock by default.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch(SCHEMA)?;
        if path != ":memory:" {
            let _ = py::chmod(path, 0o600);
        }
        Ok(Store { inner: Arc::new(Inner { path: path.to_string(), conn: Mutex::new(Some(conn)) }) })
    }

    pub fn path(&self) -> &str {
        &self.inner.path
    }

    pub fn close(&self) {
        let mut conn = self.inner.conn.lock().unwrap_or_else(|e| e.into_inner());
        *conn = None;
    }

    fn exec_sync(&self, sql: &str, params: &[Param]) -> anyhow::Result<Vec<Row>> {
        let guard = self.inner.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard.as_ref().ok_or_else(|| anyhow!("Cannot operate on a closed database."))?;
        let mut stmt = conn.prepare(sql)?;
        let names: Vec<String> = stmt.column_names().into_iter().map(str::to_string).collect();
        let rows = stmt.query_map(params_from_iter(params.iter()), |row| row_to_map(row, &names))?;
        Ok(rows.collect::<rusqlite::Result<Vec<Row>>>()?)
    }

    fn count_sync(&self, sql: &str, params: &[Param]) -> anyhow::Result<usize> {
        let guard = self.inner.conn.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard.as_ref().ok_or_else(|| anyhow!("Cannot operate on a closed database."))?;
        Ok(conn.execute(sql, params_from_iter(params.iter()))?)
    }

    /// `_run`: the query in a worker thread, its rows.
    pub async fn run(&self, sql: &str, params: Vec<Param>) -> anyhow::Result<Vec<Row>> {
        let store = self.clone();
        let sql = sql.to_string();
        tokio::task::spawn_blocking(move || store.exec_sync(&sql, &params)).await?
    }

    /// `_run_count`: the statement in a worker thread, how many rows it changed.
    pub async fn run_count(&self, sql: &str, params: Vec<Param>) -> anyhow::Result<usize> {
        let store = self.clone();
        let sql = sql.to_string();
        tokio::task::spawn_blocking(move || store.count_sync(&sql, &params)).await?
    }

    // -- scheduled tasks -----------------------------------------------------------------------

    /// Records a job seen by the cron watcher; returns whether its replies should be pushed.
    pub async fn note_cron_job(&self, agent: &str, job: &str, name: Option<&str>) -> anyhow::Result<bool> {
        self.run(
            "INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,1,?) \
             ON CONFLICT(agent, job) DO UPDATE SET name=COALESCE(excluded.name, cron_jobs.name), \
             last_seen=excluded.last_seen",
            vec![text(agent), text(job), opt(name), text(&iso(utcnow()))],
        )
        .await?;
        let rows =
            self.run("SELECT notify FROM cron_jobs WHERE agent=? AND job=?", vec![text(agent), text(job)]).await?;
        Ok(rows.first().map(|r| py::truthy(&r["notify"])).unwrap_or(true))
    }

    // -- runs watched for push (survive a restart) ----------------------------------------------

    pub async fn remember_run(&self, agent: &str, run_id: &str, session_id: Option<&str>) -> anyhow::Result<()> {
        self.run(
            "INSERT INTO watched_runs (agent, run_id, session_id, created_at) VALUES (?,?,?,?) \
             ON CONFLICT(agent, run_id) DO UPDATE SET session_id=COALESCE(excluded.session_id, session_id)",
            vec![text(agent), text(run_id), opt(session_id), text(&iso(utcnow()))],
        )
        .await?;
        Ok(())
    }

    pub async fn forget_run(&self, agent: &str, run_id: &str) -> anyhow::Result<()> {
        self.run("DELETE FROM watched_runs WHERE agent=? AND run_id=?", vec![text(agent), text(run_id)]).await?;
        Ok(())
    }

    pub async fn watched_runs(&self, max_age_seconds: f64) -> anyhow::Result<Vec<Row>> {
        let cutoff = iso(utcnow() - Duration::microseconds((max_age_seconds * 1_000_000.0) as i64));
        self.run("DELETE FROM watched_runs WHERE created_at < ?", vec![text(&cutoff)]).await?;
        self.run("SELECT * FROM watched_runs ORDER BY created_at", vec![]).await
    }

    pub async fn cron_job_name(&self, agent: &str, job: &str) -> anyhow::Result<Option<String>> {
        let rows = self.run("SELECT name FROM cron_jobs WHERE agent=? AND job=?", vec![text(agent), text(job)]).await?;
        Ok(rows.first().and_then(|r| r["name"].as_str()).filter(|n| !n.is_empty()).map(str::to_string))
    }

    pub async fn cron_jobs(&self, agent: Option<&str>) -> anyhow::Result<Vec<Row>> {
        let (mut sql, mut params) = ("SELECT * FROM cron_jobs".to_string(), vec![]);
        if let Some(agent) = agent.filter(|a| !a.is_empty()) {
            sql.push_str(" WHERE agent=?");
            params.push(text(agent));
        }
        sql.push_str(" ORDER BY last_seen DESC");
        let rows = self.run(&sql, params).await?;
        Ok(rows
            .into_iter()
            .map(|mut r| {
                let notify = py::truthy(&r["notify"]);
                r.insert("notify".into(), Value::Bool(notify));
                r
            })
            .collect())
    }

    pub async fn set_cron_notify(&self, agent: &str, job: &str, notify: bool) -> anyhow::Result<bool> {
        let n = self
            .run_count(
                "UPDATE cron_jobs SET notify=? WHERE agent=? AND job=?",
                vec![Param::Integer(notify as i64), text(agent), text(job)],
            )
            .await?;
        Ok(n > 0)
    }

    // -- devices -----------------------------------------------------------------------------

    pub async fn upsert_device(&self, token: &str, environment: &str, agent_ids: &[String]) -> anyhow::Result<Row> {
        let now = iso(utcnow());
        let mut ids: Vec<String> = agent_ids.to_vec();
        ids.sort();
        ids.dedup();
        let ids_json = py::dumps(&json!(ids));
        self.run(
            "INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?) \
             ON CONFLICT(token) DO UPDATE SET environment=excluded.environment, \
             agent_ids=excluded.agent_ids, updated_at=excluded.updated_at",
            vec![text(token), text(environment), text(&ids_json), text(&now), text(&now)],
        )
        .await?;
        self.get_device(token).await?.ok_or_else(|| anyhow!("device not stored"))
    }

    pub async fn get_device(&self, token: &str) -> anyhow::Result<Option<Row>> {
        let rows = self.run("SELECT * FROM devices WHERE token=?", vec![text(token)]).await?;
        Ok(rows.first().map(Self::device))
    }

    pub async fn delete_device(&self, token: &str) -> anyhow::Result<bool> {
        Ok(self.run_count("DELETE FROM devices WHERE token=?", vec![text(token)]).await? > 0)
    }

    pub async fn devices_for_agent(&self, agent: &str) -> anyhow::Result<Vec<Row>> {
        let rows = self.run("SELECT * FROM devices", vec![]).await?;
        Ok(rows
            .iter()
            .map(Self::device)
            .filter(|dev| {
                let ids = dev["agent_ids"].as_array().cloned().unwrap_or_default();
                ids.is_empty() || ids.iter().any(|a| a.as_str() == Some(agent))
            })
            .collect())
    }

    fn device(row: &Row) -> Row {
        let mut out = Map::new();
        out.insert("token".into(), row["token"].clone());
        out.insert("environment".into(), row["environment"].clone());
        let ids = row["agent_ids"].as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()).unwrap_or(json!([]));
        out.insert("agent_ids".into(), ids);
        out.insert("created_at".into(), row["created_at"].clone());
        out.insert("updated_at".into(), row["updated_at"].clone());
        out
    }

    // -- outbox ------------------------------------------------------------------------------

    /// Insert a message; returns `None` when this ntfy message was already stored.
    pub async fn add_outbox(&self, agent: &str, text_: &str, extra: NewOutbox) -> anyhow::Result<Option<Row>> {
        let item_id = py::uuid4_hex();
        let count = self
            .run_count(
                "INSERT OR IGNORE INTO outbox (id, agent, ntfy_id, title, text, created_at, sent_at, session_id) \
                 VALUES (?,?,?,?,?,?,?,?)",
                vec![
                    text(&item_id),
                    text(agent),
                    opt(extra.ntfy_id.as_deref()),
                    opt(extra.title.as_deref()),
                    text(text_),
                    text(&outbox_stamp()),
                    opt(extra.sent_at.as_deref()),
                    opt(extra.session_id.as_deref()),
                ],
            )
            .await?;
        if count == 0 {
            return Ok(None);
        }
        self.get_outbox(&item_id).await
    }

    /// New text for an already stored message (its audio is redone); `None` if unknown or unchanged.
    pub async fn replace_outbox(
        &self,
        agent: &str,
        ntfy_id: &str,
        text_: &str,
        title: Option<&str>,
    ) -> anyhow::Result<Option<Row>> {
        let rows =
            self.run("SELECT * FROM outbox WHERE agent=? AND ntfy_id=?", vec![text(agent), text(ntfy_id)]).await?;
        let Some(row) = rows.first() else { return Ok(None) };
        if row["text"].as_str() == Some(text_) {
            return Ok(None);
        }
        let id = row["id"].as_str().unwrap_or_default().to_string();
        self.run(
            "UPDATE outbox SET text=?, title=COALESCE(?, title), audio_path=NULL, created_at=? WHERE id=?",
            vec![text(text_), opt(title), text(&outbox_stamp()), text(&id)],
        )
        .await?;
        self.get_outbox(&id).await
    }

    pub async fn set_outbox_audio(&self, item_id: &str, path: &str) -> anyhow::Result<()> {
        self.run("UPDATE outbox SET audio_path=? WHERE id=?", vec![text(path), text(item_id)]).await?;
        Ok(())
    }

    pub async fn get_outbox(&self, item_id: &str) -> anyhow::Result<Option<Row>> {
        Ok(self.run("SELECT * FROM outbox WHERE id=?", vec![text(item_id)]).await?.into_iter().next())
    }

    pub async fn list_outbox(
        &self,
        since: Option<DateTime<Utc>>,
        agent: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<Row>> {
        let mut sql = "SELECT * FROM outbox WHERE 1=1".to_string();
        let mut params = Vec::new();
        if let Some(since) = since {
            sql.push_str(" AND created_at > ?");
            params.push(text(&iso(since)));
        }
        if let Some(agent) = agent.filter(|a| !a.is_empty()) {
            sql.push_str(" AND agent = ?");
            params.push(text(agent));
        }
        sql.push_str(" ORDER BY created_at ASC LIMIT ?");
        params.push(Param::Integer(limit));
        self.run(&sql, params).await
    }

    pub async fn purge_outbox(&self, older_than_days: i64) -> anyhow::Result<Vec<Row>> {
        let cutoff = iso(utcnow() - Duration::days(older_than_days));
        let rows = self.run("SELECT id, audio_path FROM outbox WHERE created_at < ?", vec![text(&cutoff)]).await?;
        self.run("DELETE FROM outbox WHERE created_at < ?", vec![text(&cutoff)]).await?;
        Ok(rows)
    }

    // -- ntfy cursors ------------------------------------------------------------------------

    pub async fn get_cursor(&self, agent: &str) -> anyhow::Result<Option<String>> {
        let rows = self.run("SELECT last_id FROM ntfy_cursor WHERE agent=?", vec![text(agent)]).await?;
        Ok(rows.first().and_then(|r| r["last_id"].as_str()).map(str::to_string))
    }

    pub async fn set_cursor(&self, agent: &str, last_id: &str) -> anyhow::Result<()> {
        self.run(
            "INSERT INTO ntfy_cursor (agent, last_id, updated_at) VALUES (?,?,?) \
             ON CONFLICT(agent) DO UPDATE SET last_id=excluded.last_id, updated_at=excluded.updated_at",
            vec![text(agent), text(last_id), text(&iso(utcnow()))],
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_and_since() {
        let dt = Utc.with_ymd_and_hms(2026, 10, 4, 8, 15, 0).unwrap() + Duration::microseconds(123_999);
        assert_eq!(iso(dt), "2026-10-04T08:15:00.123Z");
        assert_eq!(iso(parse_since("2026-10-04T08:15:00.123Z").unwrap()), "2026-10-04T08:15:00.123Z");
        assert_eq!(iso(parse_since("2026-10-04T10:15:00+02:00").unwrap()), "2026-10-04T08:15:00.000Z");
        assert_eq!(iso(parse_since("2026-10-04T08:15:00").unwrap()), "2026-10-04T08:15:00.000Z");
        assert_eq!(iso(parse_since("2026-10-04").unwrap()), "2026-10-04T00:00:00.000Z");
        assert_eq!(iso(parse_since(" 0 ").unwrap()), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(parse_since("1.5").unwrap()), "1970-01-01T00:00:01.500Z");
        assert!(parse_since("yesterday").is_err());
    }

    #[tokio::test]
    async fn devices_outbox_cron() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("bridge.db");
        let store = Store::open(path.to_str().unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let dev = store.upsert_device("ab", "sandbox", &["b".into(), "a".into(), "a".into()]).await.unwrap();
        assert_eq!(dev["agent_ids"], json!(["a", "b"]));
        let raw = store.run("SELECT agent_ids FROM devices", vec![]).await.unwrap();
        assert_eq!(raw[0]["agent_ids"], "[\"a\", \"b\"]");
        assert_eq!(store.devices_for_agent("a").await.unwrap().len(), 1);
        assert_eq!(store.devices_for_agent("z").await.unwrap().len(), 0);
        assert!(store.delete_device("ab").await.unwrap());
        assert!(!store.delete_device("ab").await.unwrap());

        let item = store
            .add_outbox("lea", "Bonjour", NewOutbox { ntfy_id: Some("n1".into()), ..Default::default() })
            .await
            .unwrap()
            .unwrap();
        let keys: Vec<&String> = item.keys().collect();
        assert_eq!(
            keys,
            ["id", "agent", "ntfy_id", "title", "text", "created_at", "sent_at", "session_id", "audio_path"]
        );
        assert!(store
            .add_outbox("lea", "Bonjour", NewOutbox { ntfy_id: Some("n1".into()), ..Default::default() })
            .await
            .unwrap()
            .is_none());
        assert!(store.replace_outbox("lea", "n1", "Bonjour", None).await.unwrap().is_none());
        let replaced = store.replace_outbox("lea", "n1", "Salut", Some("T")).await.unwrap().unwrap();
        assert_eq!(replaced["text"], "Salut");
        assert_eq!(replaced["title"], "T");
        assert_eq!(store.list_outbox(None, Some("lea"), 10).await.unwrap().len(), 1);

        assert!(store.note_cron_job("lea", "j1", Some("Matin")).await.unwrap());
        assert!(store.set_cron_notify("lea", "j1", false).await.unwrap());
        assert!(!store.note_cron_job("lea", "j1", None).await.unwrap());
        assert_eq!(store.cron_job_name("lea", "j1").await.unwrap().as_deref(), Some("Matin"));
        let jobs = store.cron_jobs(Some("lea")).await.unwrap();
        assert_eq!(jobs[0]["notify"], json!(false));

        store.set_cursor("lea", "c1").await.unwrap();
        store.set_cursor("lea", "c2").await.unwrap();
        assert_eq!(store.get_cursor("lea").await.unwrap().as_deref(), Some("c2"));

        store.remember_run("lea", "r1", None).await.unwrap();
        store.remember_run("lea", "r1", Some("s1")).await.unwrap();
        let runs = store.watched_runs(3600.0).await.unwrap();
        assert_eq!(runs[0]["session_id"], "s1");
        store.forget_run("lea", "r1").await.unwrap();
        assert!(store.watched_runs(3600.0).await.unwrap().is_empty());
        store.close();
    }
}
