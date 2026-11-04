//! The notification journal (docs/conversations.md §9): every notification of an agent is a row here first, then a
//! push that points at it. One entry ([`crate::push::PushService::emit`]), one dedupe key per event (a run's end and
//! the cron watcher seeing the same reply make one row), one rules table for « Me prévenir », the conversation and
//! the message it is about (`c`, `m`: the push is sent once the reply is stored, with its id), read state shared by
//! every device.
//!
//! ```text
//! GET    /v1/notifications?agent=&thread=&since=&unread=1&limit=     newest first
//! GET    /v1/notifications/unread                                    {total, agents, threads}
//! GET    /v1/notifications/rules                                     the rules table
//! GET    /v1/notifications/{id}                                      one row, with its text
//! POST   /v1/notifications/read  {ids?, agent?, thread?}             → {read: [ids], unread}
//! DELETE /v1/notifications/{id}                                      hidden from the Boîte
//! ```
//!
//! The row's text is kept here (the Notification Service Extension reads it: `GET /v1/notifications/{n}`; it replaced
//! the push service's in-memory texts and `/v1/replies`). Rows older than `KEEP_DAYS` go.
//!
//! No Python counterpart.

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::store::{iso, utcnow, Store};
use crate::web::{self, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::Dict;

pub const KEEP_DAYS: i64 = 90;
/// A row's text kept at most (characters).
pub const TEXT_MAX: usize = 4000;

/// What a notification is about (closed: each has its rule and its contract fixture).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Reply,
    Question,
    Approval,
    RoomReply,
    Boite,
    Task,
    Subtask,
    Scan,
    Family,
    EmailHeld,
    Site,
    Login,
    Connector,
    Secret,
    MailCard,
    Meet,
}

/// Where a Boîte message comes from (its rule differs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoiteFrom {
    /// A background task's report handed back (« il a fini »: follows the switch).
    Task,
    /// A routine's report (each routine has its own mute, cronwatch.rs).
    Routine,
    /// Credit, storage, imports, watchers: their sources decide.
    Other,
}

impl Kind {
    pub const ALL: [Kind; 16] = [
        Kind::Reply,
        Kind::Question,
        Kind::Approval,
        Kind::RoomReply,
        Kind::Boite,
        Kind::Task,
        Kind::Subtask,
        Kind::Scan,
        Kind::Family,
        Kind::EmailHeld,
        Kind::Site,
        Kind::Login,
        Kind::Connector,
        Kind::Secret,
        Kind::MailCard,
        Kind::Meet,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Reply => "reply",
            Kind::Question => "question",
            Kind::Approval => "approval",
            Kind::RoomReply => "room_reply",
            Kind::Boite => "boite",
            Kind::Task => "task",
            Kind::Subtask => "subtask",
            Kind::Scan => "scan",
            Kind::Family => "family",
            Kind::EmailHeld => "email_held",
            Kind::Site => "site",
            Kind::Login => "login",
            Kind::Connector => "connector",
            Kind::Secret => "secret",
            Kind::MailCard => "mail_card",
            Kind::Meet => "meet",
        }
    }

    /// The card keys of `notify_bridge_card` (connectors.rs, secrets.rs, agent_tools.rs).
    pub fn of_card(key: &str) -> Kind {
        match key {
            "secret_request" => Kind::Secret,
            "mail_request" => Kind::MailCard,
            _ => Kind::Connector,
        }
    }

    /// The agent's « Me prévenir » switch silences it (the rules table, docs/conversations.md §9.2).
    pub fn follows_switch(self, from: Option<BoiteFrom>) -> bool {
        match self {
            Kind::Reply | Kind::RoomReply | Kind::Subtask | Kind::Scan | Kind::Task => true,
            Kind::Boite => from == Some(BoiteFrom::Task),
            // approvals and security asks, questions (adopted 2026-11-02), family notes: always
            _ => false,
        }
    }

    /// The neutral body code the apps translate.
    pub fn body_key(self) -> String {
        format!("notif.{}", self.as_str())
    }
}

/// The rules table as served (`GET /v1/notifications/rules`).
pub fn rules() -> Value {
    let mut rows: Vec<Value> = Kind::ALL
        .iter()
        .filter(|k| **k != Kind::Boite)
        .map(|k| json!({"type": k.as_str(), "follows_switch": k.follows_switch(None)}))
        .collect();
    for (from, name) in [(BoiteFrom::Task, "task"), (BoiteFrom::Routine, "routine"), (BoiteFrom::Other, "other")] {
        rows.push(json!({"type": "boite", "from": name, "follows_switch": Kind::Boite.follows_switch(Some(from))}));
    }
    json!({"rules": rows})
}

type Param = rusqlite::types::Value;

fn text(v: &str) -> Param {
    Param::Text(v.to_string())
}

fn opt(v: Option<&str>) -> Param {
    v.map(text).unwrap_or(Param::Null)
}

/// A new row.
#[derive(Debug, Clone, Default)]
pub struct NewRow<'a> {
    pub id: &'a str,
    pub agent: &'a str,
    pub kind: &'a str,
    pub thread: Option<&'a str>,
    pub message: Option<&'a str>,
    pub ref_id: Option<&'a str>,
    pub session: Option<&'a str>,
    pub title: &'a str,
    pub text: Option<&'a str>,
    pub state: &'a str,
    pub dedupe: &'a str,
}

/// Inserts the row unless its dedupe key is taken: false then (the event was journaled already).
pub async fn insert(store: &Store, row: &NewRow<'_>) -> anyhow::Result<bool> {
    let cut: Option<String> = row.text.map(|t| t.chars().take(TEXT_MAX).collect());
    let n = store
        .run_count(
            "INSERT OR IGNORE INTO notifications (id, agent, type, thread_id, message_id, ref_id, session_id, title, \
             body_key, preview, state, created_at, push, dedupe_key) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,'pending',?)",
            vec![
                text(row.id),
                text(row.agent),
                text(row.kind),
                opt(row.thread),
                opt(row.message),
                opt(row.ref_id),
                opt(row.session),
                text(row.title),
                text(&format!("notif.{}", row.kind)),
                opt(cut.as_deref()),
                text(row.state),
                text(&iso(utcnow())),
                text(row.dedupe),
            ],
        )
        .await?;
    if n > 0 && rand::random::<u8>() == 0 {
        let cutoff = iso(utcnow() - chrono::Duration::days(KEEP_DAYS));
        let _ = store.run_count("DELETE FROM notifications WHERE created_at < ?", vec![text(&cutoff)]).await;
    }
    Ok(n > 0)
}

pub async fn set_push(store: &Store, id: &str, push: &str) -> anyhow::Result<()> {
    store.run("UPDATE notifications SET push = ? WHERE id = ?", vec![text(push), text(id)]).await?;
    Ok(())
}

/// One row read now.
pub async fn mark_read(store: &Store, id: &str) -> anyhow::Result<()> {
    store
        .run(
            "UPDATE notifications SET read_at = ? WHERE id = ? AND read_at IS NULL",
            vec![text(&iso(utcnow())), text(id)],
        )
        .await?;
    Ok(())
}

/// Unread rows in all (the app icon's badge).
pub async fn unread_total(store: &Store) -> anyhow::Result<i64> {
    let rows =
        store.run("SELECT COUNT(*) AS n FROM notifications WHERE read_at IS NULL AND hidden = 0", vec![]).await?;
    Ok(rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0))
}

/// Unread rows of a thread.
pub async fn unread_in(store: &Store, thread: &str) -> anyhow::Result<i64> {
    let rows = store
        .run(
            "SELECT COUNT(*) AS n FROM notifications WHERE thread_id = ? AND read_at IS NULL AND hidden = 0",
            vec![text(thread)],
        )
        .await?;
    Ok(rows.first().and_then(|r| r["n"].as_i64()).unwrap_or(0))
}

fn served(row: &crate::store::Row, full: bool) -> Value {
    let preview =
        row.get("preview").and_then(Value::as_str).map(
            |p| {
                if full {
                    p.to_string()
                } else {
                    crate::push::preview(p, 160)
                }
            },
        );
    json!({
        "id": row["id"], "agent": row["agent"], "type": row["type"], "c": row["thread_id"], "m": row["message_id"],
        "ref": row["ref_id"], "session_id": row["session_id"], "title": row["title"], "body": row["body_key"],
        "preview": preview, "state": row["state"], "created_at": row["created_at"], "read_at": row["read_at"],
        "push": row["push"],
    })
}

async fn counts(store: &Store) -> anyhow::Result<Value> {
    let rows = store
        .run(
            "SELECT agent, thread_id, COUNT(*) AS n FROM notifications WHERE read_at IS NULL AND hidden = 0 \
             GROUP BY agent, thread_id",
            vec![],
        )
        .await?;
    let (mut agents, mut threads, mut total) = (Map::new(), Map::new(), 0i64);
    for r in &rows {
        let n = r["n"].as_i64().unwrap_or(0);
        total += n;
        let a = r["agent"].as_str().unwrap_or("").to_string();
        let prior = agents.get(&a).and_then(Value::as_i64).unwrap_or(0);
        agents.insert(a, json!(prior + n));
        if let Some(t) = r["thread_id"].as_str() {
            threads.insert(t.to_string(), json!(n));
        }
    }
    Ok(json!({"total": total, "agents": agents, "threads": threads}))
}

// -- routes ---------------------------------------------------------------------------------------------------------

async fn list(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let mut sql = "SELECT * FROM notifications WHERE hidden = 0".to_string();
    let mut params = Vec::new();
    if let Some(a) = q.get("agent").filter(|a| !a.is_empty()) {
        sql.push_str(" AND agent = ?");
        params.push(text(a));
    }
    if let Some(t) = q.get("thread").filter(|a| !a.is_empty()) {
        sql.push_str(" AND thread_id = ?");
        params.push(text(t));
    }
    if let Some(s) = q.get("since").filter(|a| !a.is_empty()) {
        let since = crate::store::parse_since(s).map_err(|e| HttpError::new(422, e))?;
        sql.push_str(" AND created_at > ?");
        params.push(text(&iso(since)));
    }
    if q.get("unread").is_some_and(|u| u == "1" || u == "true") {
        sql.push_str(" AND read_at IS NULL");
    }
    let limit = q.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(50).clamp(1, 500);
    sql.push_str(&format!(" ORDER BY created_at DESC, id DESC LIMIT {limit}"));
    let rows = state.store.run(&sql, params).await?;
    let items: Vec<Value> = rows.iter().map(|r| served(r, false)).collect();
    Ok(web::ok(json!({"notifications": items})))
}

async fn unread(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(counts(&state.store).await?))
}

async fn rules_route() -> ApiResult<Response> {
    Ok(web::ok(rules()))
}

async fn one(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let rows = state.store.run("SELECT * FROM notifications WHERE id = ?", vec![text(&id)]).await?;
    let row = rows.first().ok_or_else(HttpError::not_found)?;
    Ok(web::ok(served(row, true)))
}

async fn hide(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let n = state.store.run_count("UPDATE notifications SET hidden = 1 WHERE id = ?", vec![text(&id)]).await?;
    if n == 0 {
        return Err(HttpError::not_found());
    }
    Ok(web::no_content())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ReadBody {
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub thread: Option<String>,
}

impl Validate for ReadBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if self.ids.is_none() && self.agent.is_none() && self.thread.is_none() {
            errors.push(FieldError::new("missing", vec![json!("body")], "ids, agent or thread", Value::Null));
        }
        if self.ids.as_ref().is_some_and(|i| i.len() > 500) {
            errors.push(FieldError::new("too_long", vec![json!("body"), json!("ids")], "500 at most", Value::Null));
        }
    }
}

/// Marks rows read; the person's devices are told (a silent push `reason=read`, the `read` event).
async fn read(State(state): State<AppState>, Json(body): Json<ReadBody>) -> ApiResult<Response> {
    let mut sql = "SELECT id, agent, thread_id FROM notifications WHERE read_at IS NULL".to_string();
    let mut params = Vec::new();
    if let Some(ids) = &body.ids {
        if ids.is_empty() {
            return Ok(web::ok(json!({"read": [], "unread": counts(&state.store).await?})));
        }
        sql.push_str(&format!(" AND id IN ({})", vec!["?"; ids.len()].join(",")));
        params.extend(ids.iter().map(|i| text(i)));
    }
    let mut thread = body.thread.clone();
    if thread.as_deref() == Some("main") {
        let agent = crate::app::agent_or_404(&state.config(), body.agent.as_deref())?;
        thread = Some(crate::threads::threads_of(&state).main(&agent).await?.id);
    }
    if let Some(t) = &thread {
        sql.push_str(" AND thread_id = ?");
        params.push(text(t));
    }
    if let Some(a) = &body.agent {
        sql.push_str(" AND agent = ?");
        params.push(text(a));
    }
    let rows = state.store.run(&sql, params).await?;
    let ids: Vec<String> = rows.iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
    if !ids.is_empty() {
        let mut params = vec![text(&iso(utcnow()))];
        params.extend(ids.iter().map(|i| text(i)));
        state
            .store
            .run(
                &format!("UPDATE notifications SET read_at = ? WHERE id IN ({})", vec!["?"; ids.len()].join(",")),
                params,
            )
            .await?;
    }
    let unread = counts(&state.store).await?;
    // tell the devices, per agent
    let mut by_agent: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for r in &rows {
        by_agent
            .entry(r["agent"].as_str().unwrap_or("").to_string())
            .or_default()
            .push(r["id"].as_str().unwrap_or("").to_string());
    }
    let threads = crate::threads::threads_of(&state);
    for (agent, read) in by_agent {
        threads.publish(&agent, "read", json!({"agent": agent, "thread": thread, "notifications": read}));
        let mut extra = Dict::new();
        extra.insert("n".into(), json!(read.join(","))); // one form for APNs and FCM (data values are strings)
        extra.insert("unread".into(), unread["total"].clone());
        let push = state.push.clone();
        state.spawn(async move {
            push.notify_silent(&agent, "read", extra).await;
        });
    }
    Ok(web::ok(json!({"read": ids, "unread": unread})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/unread", get(unread))
        .route("/notifications/rules", get(rules_route))
        .route("/notifications/read", post(read))
        .route("/notifications/{id}", get(one).delete(hide))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules() {
        // approvals, security asks and questions always notify
        for k in [
            Kind::Approval,
            Kind::Question,
            Kind::EmailHeld,
            Kind::Site,
            Kind::Login,
            Kind::Connector,
            Kind::Secret,
            Kind::MailCard,
            Kind::Meet,
            Kind::Family,
        ] {
            assert!(!k.follows_switch(None), "{k:?}");
        }
        for k in [Kind::Reply, Kind::RoomReply, Kind::Subtask, Kind::Scan, Kind::Task] {
            assert!(k.follows_switch(None), "{k:?}");
        }
        assert!(Kind::Boite.follows_switch(Some(BoiteFrom::Task)));
        assert!(!Kind::Boite.follows_switch(Some(BoiteFrom::Routine)));
        assert!(!Kind::Boite.follows_switch(Some(BoiteFrom::Other)));
        let names: std::collections::HashSet<&str> = Kind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(names.len(), Kind::ALL.len());
        assert_eq!(Kind::of_card("secret_request"), Kind::Secret);
        assert_eq!(rules()["rules"].as_array().unwrap().len(), 18);
    }

    #[tokio::test]
    async fn one_row_per_dedupe_key() {
        let store = Store::open(":memory:").unwrap();
        let row = NewRow {
            id: "n_1",
            agent: "vie",
            kind: "reply",
            title: "Vie",
            state: "open",
            dedupe: "reply:vie:m_1",
            text: Some("Bonjour"),
            ..Default::default()
        };
        assert!(insert(&store, &row).await.unwrap());
        assert!(!insert(&store, &NewRow { id: "n_2", ..row.clone() }).await.unwrap());
        let thread = NewRow { id: "n_3", thread: Some("t_1"), dedupe: "reply:vie:m_2", ..row };
        insert(&store, &thread).await.unwrap();
        assert_eq!(unread_in(&store, "t_1").await.unwrap(), 1);
        assert_eq!(counts(&store).await.unwrap()["total"], 2);
    }
}
