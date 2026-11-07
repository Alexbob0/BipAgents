//! The notification journal (docs/conversations.md §9): every notification of an agent is a row here first, then a
//! push that points at it. One entry ([`crate::push::PushService::emit`]), one dedupe key per event (a run's end and
//! the cron watcher seeing the same reply make one row), one rules table for « Me prévenir », the conversation and
//! the message it is about (`c`, `m`: the push is sent once the reply is stored, with its id), read state shared by
//! every device.
//!
//! ```text
//! GET    /v1/notifications?agent=&thread=&since=&unread=1&limit=     newest first
//! GET    /v1/notifications/unread                                    {total, agents, threads, boite}
//! GET    /v1/notifications/{id}                                      one row, with its text
//!
//! `total` (the badges: the app icon's, the Boîte tab's) counts what the Boîte shows and nothing else (`BOITE_*`):
//! its messages unread and the cards waiting (a card's row stays unread only while its card waits: `read_gone`).
//! `boite` is that count by agent (an app adds its own agents'). `agents` and `threads` count every unread row (the
//! home's dots: a room's reply, a held e-mail…). Alex's iPhone, 5/11: « Boîte 1 » for ever on an empty Boîte, the row
//! of a card gone counted.
//!
//! A `boite` row carries its message as the Boîte shows it (`boite`: title, the whole text, attachments, has_audio,
//! audio_url — the audio at `/v1/outbox/{id}/audio`): the apps read the Boîte from here alone. Its `created_at` is
//! the message's arrival (the outbox's, one per millisecond), not its push's: the apps list by it, so the Boîte shows
//! its messages in the order they came, however long each one's audio took (`since` sees it by that date too).
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
    /// When it happened, when that is not now (`created_at`, `iso`): a Boîte message's arrival.
    pub created_at: Option<&'a str>,
}

/// Inserts the row unless its dedupe key is taken: false then (the event was journaled already).
pub async fn insert(store: &Store, row: &NewRow<'_>) -> anyhow::Result<bool> {
    // the words only: a reply's files are in its conversation, never its `MEDIA:` lines here (4/11)
    let cut: Option<String> =
        row.text.map(|t| crate::media::without_media_lines(t, |_| true).chars().take(TEXT_MAX).collect());
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
                text(&row.created_at.map_or_else(|| iso(utcnow()), str::to_string)),
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

/// The rows the Boîte shows: its messages (the types the apps list, InboxStore.swift / Inbox.kt) and the cards
/// waiting (listed from `/v1/approvals`, each with its row). Not the Boîte's: a room's reply (the room on the home), a
/// held e-mail (the agent's e-mail screen), a date to find (its own card). The place's own cards (space, tips) have no
/// row: never counted.
pub const BOITE_MESSAGES: [&str; 7] = ["boite", "reply", "subtask", "task", "scan", "family", "question"];
pub const BOITE_CARDS: [&str; 6] = ["approval", "site", "login", "connector", "secret", "mail_card"];

fn quoted(types: &[&str]) -> String {
    types.iter().map(|t| format!("'{t}'")).collect::<Vec<_>>().join(",")
}

fn boite_types() -> String {
    quoted(&[&BOITE_MESSAGES[..], &BOITE_CARDS[..]].concat())
}

/// Unread rows the Boîte shows (the badges).
pub async fn unread_total(store: &Store) -> anyhow::Result<i64> {
    let sql = format!(
        "SELECT COUNT(*) AS n FROM notifications WHERE read_at IS NULL AND hidden = 0 AND type IN ({})",
        boite_types()
    );
    let rows = store.run(&sql, vec![]).await?;
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
    let sql = format!(
        "SELECT agent, thread_id, type IN ({}) AS shown, COUNT(*) AS n FROM notifications \
         WHERE read_at IS NULL AND hidden = 0 GROUP BY agent, thread_id, shown",
        boite_types()
    );
    let rows = store.run(&sql, vec![]).await?;
    let (mut agents, mut threads, mut boite, mut total) = (Map::new(), Map::new(), Map::new(), 0i64);
    let add = |map: &mut Map<String, Value>, key: &str, n: i64| {
        let prior = map.get(key).and_then(Value::as_i64).unwrap_or(0);
        map.insert(key.to_string(), json!(prior + n));
    };
    for r in &rows {
        let n = r["n"].as_i64().unwrap_or(0);
        let a = r["agent"].as_str().unwrap_or("");
        add(&mut agents, a, n);
        if let Some(t) = r["thread_id"].as_str() {
            add(&mut threads, t, n);
        }
        // the badges: what the Boîte shows only
        if r["shown"].as_i64().unwrap_or(0) == 1 {
            total += n;
            add(&mut boite, a, n);
        }
    }
    Ok(json!({"total": total, "agents": agents, "threads": threads, "boite": boite}))
}

/// The rows served, a Boîte row with its message (`boite`, see the module's doc).
async fn served_rows(state: &AppState, rows: &[crate::store::Row], full: bool) -> anyhow::Result<Vec<Value>> {
    let ids: Vec<&str> =
        rows.iter().filter(|r| r["type"] == "boite").filter_map(|r| r.get("ref_id").and_then(Value::as_str)).collect();
    let mut messages = std::collections::HashMap::new();
    if !ids.is_empty() {
        let marks = vec!["?"; ids.len()].join(",");
        let found = state
            .store
            .run(&format!("SELECT * FROM outbox WHERE id IN ({marks})"), ids.iter().map(|i| text(i)).collect())
            .await?;
        for row in &found {
            let item = crate::app::outbox_item(row, state);
            let boite = json!({"title": item["title"], "text": item["text"], "attachments": item["attachments"],
                               "has_audio": item["has_audio"], "audio_url": item["audio_url"]});
            messages.insert(item["id"].as_str().unwrap_or("").to_string(), boite);
        }
    }
    Ok(rows
        .iter()
        .map(|r| {
            let mut out = served(r, full);
            if let Some(boite) = r.get("ref_id").and_then(Value::as_str).and_then(|i| messages.get(i)) {
                out["boite"] = boite.clone();
            }
            out
        })
        .collect())
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
    Ok(web::ok(json!({"notifications": served_rows(&state, &rows, false).await?})))
}

async fn unread(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(counts(&state.store).await?))
}

async fn one(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let rows = state.store.run("SELECT * FROM notifications WHERE id = ?", vec![text(&id)]).await?;
    rows.first().ok_or_else(HttpError::not_found)?;
    Ok(web::ok(served_rows(&state, &rows, true).await?.remove(0)))
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
/// A card answered (card_place.rs): its notifications (`ref` the card's id) read and marked answered, on every device
/// (the push `read`, with `card`: the Boîte asks its cards again).
pub async fn read_card(state: &AppState, agent: &str, card: &str, run_id: &str) {
    let found = state
        .store
        .run(
            "SELECT id FROM notifications WHERE agent = ? AND ref_id IN (?, ?) AND type IN \
             ('approval', 'site', 'login', 'connector', 'secret', 'mail_card') \
             AND (read_at IS NULL OR state != 'answered')",
            vec![text(agent), text(card), text(run_id)],
        )
        .await;
    let ids: Vec<String> =
        found.unwrap_or_default().iter().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
    if !ids.is_empty() {
        let mut params = vec![text(&iso(utcnow()))];
        params.extend(ids.iter().map(|i| text(i)));
        let sql = format!(
            "UPDATE notifications SET read_at = COALESCE(read_at, ?), state = 'answered' WHERE id IN ({})",
            vec!["?"; ids.len()].join(",")
        );
        if let Err(err) = state.store.run(&sql, params).await {
            tracing::warn!(agent = %agent, error = %format!("{err:#}"), "card notification not read");
        }
    }
    let unread = counts(&state.store).await.unwrap_or_else(|_| json!({"total": 0}));
    crate::threads::threads_of(state).publish(agent, "read", json!({"agent": agent, "notifications": ids, "card": card}));
    let mut extra = Dict::new();
    extra.insert("n".into(), json!(ids.join(",")));
    extra.insert("unread".into(), unread["total"].clone());
    extra.insert("card".into(), json!(card));
    let (push, agent) = (state.push.clone(), agent.to_string());
    state.spawn(async move {
        push.notify_silent(&agent, "read", extra).await;
    });
}

/// The cards' rows still unread whose card waits no more (`waiting`: the run and request ids of every card waiting,
/// the bridge's and Hermes'): read on every device, `gone`. Answered or withdrawn where nothing read its row (a secret
/// typed in its field, a card never placed, a Hermes approval lost with its run), one kept « Boîte 1 » on an empty
/// Boîte (Alex's iPhone, 5/11). A row younger than `fresh` seconds is left: its card may not be listed yet.
pub async fn read_gone(state: &AppState, waiting: &std::collections::HashSet<String>, fresh: i64) -> usize {
    let before = iso(utcnow() - chrono::Duration::seconds(fresh));
    let sql = format!(
        "SELECT id, agent, ref_id FROM notifications WHERE read_at IS NULL AND type IN ({}) AND created_at < ?",
        quoted(&BOITE_CARDS)
    );
    let Ok(rows) = state.store.run(&sql, vec![text(&before)]).await else { return 0 };
    let mut by_agent: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for r in &rows {
        if r.get("ref_id").and_then(Value::as_str).is_some_and(|c| waiting.contains(c)) {
            continue;
        }
        by_agent
            .entry(r["agent"].as_str().unwrap_or("").to_string())
            .or_default()
            .push(r["id"].as_str().unwrap_or("").to_string());
    }
    let ids: Vec<&String> = by_agent.values().flatten().collect();
    if ids.is_empty() {
        return 0;
    }
    let mut params = vec![text(&iso(utcnow()))];
    params.extend(ids.iter().map(|i| text(i)));
    let sql = format!(
        "UPDATE notifications SET read_at = ?, state = 'gone' WHERE read_at IS NULL AND id IN ({})",
        vec!["?"; ids.len()].join(",")
    );
    if let Err(err) = state.store.run(&sql, params).await {
        tracing::warn!(error = %format!("{err:#}"), "gone cards' notifications not read");
        return 0;
    }
    let count = ids.len();
    tracing::info!(count, "notifications of cards gone read");
    let unread = counts(&state.store).await.unwrap_or_else(|_| json!({"total": 0}));
    let threads = crate::threads::threads_of(state);
    for (agent, read) in by_agent {
        threads.publish(&agent, "read", json!({"agent": agent, "notifications": read}));
        let mut extra = Dict::new();
        extra.insert("n".into(), json!(read.join(",")));
        extra.insert("unread".into(), unread["total"].clone());
        let push = state.push.clone();
        state.spawn(async move {
            push.notify_silent(&agent, "read", extra).await;
        });
    }
    count
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(list))
        .route("/notifications/unread", get(unread))
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
        // a reply's words kept, never its MEDIA lines
        let files = NewRow { id: "n_4", dedupe: "reply:vie:m_3", text: Some("Voilà.\nMEDIA:/envoi/a.pdf"), ..thread };
        insert(&store, &files).await.unwrap();
        let rows = store.run("SELECT preview FROM notifications WHERE id = 'n_4'", vec![]).await.unwrap();
        assert_eq!(rows[0]["preview"], json!("Voilà."));
    }

    #[tokio::test]
    async fn the_badges_count_what_the_boite_shows() {
        // Alex's iPhone, 5/11: « Boîte 1 » on an empty Boîte
        let store = Store::open(":memory:").unwrap();
        let row = |id: &'static str, agent: &'static str, kind: &'static str, thread: Option<&'static str>| NewRow {
            id,
            agent,
            kind,
            thread,
            title: "x",
            state: "open",
            dedupe: id,
            ..Default::default()
        };
        for r in [
            row("n_1", "vie", "reply", Some("t_1")),
            row("n_2", "ghibli", "secret", None),
            row("n_3", "vie", "room_reply", Some("room:r1")),
            row("n_4", "ghibli", "email_held", None),
            row("n_5", "ghibli", "meet", None),
        ] {
            insert(&store, &r).await.unwrap();
        }
        let c = counts(&store).await.unwrap();
        // the home's dots: every unread row
        assert_eq!(c["agents"], json!({"vie": 2, "ghibli": 3}));
        assert_eq!(c["threads"], json!({"t_1": 1, "room:r1": 1}));
        // the badges: a message and a card waiting, not a room's reply, a held e-mail or a date to find
        assert_eq!(c["total"], 2);
        assert_eq!(c["boite"], json!({"vie": 1, "ghibli": 1}));
        assert_eq!(unread_total(&store).await.unwrap(), 2);
    }
}
