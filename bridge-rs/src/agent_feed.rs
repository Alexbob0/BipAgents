//! A personal agent shared with relatives, read-only (Alex, 12/11; internal/notes/personal-share-readonly.md): the
//! relative reads the owner's own conversations with it — his messages, its replies, their photos, files, pages and
//! maps, a reply while it is being written — and nothing else: not its memory, its tool steps, its cards, the Boîte's
//! messages; she never writes, asks or answers (agent_shares.rs refuses, 403 `shared_read_only`). Which conversations:
//! the owner's choice per household (agent_shares.rs `Window`): from the share on, or all of them.
//!
//! **The owner's side** (the household key):
//!
//!     GET /family/s/{agent}/feed                                     {threads: [{id, kind, title, last_active,
//!                                                                     version}], history, since}
//!     GET /family/s/{agent}/feed/{thread}/timeline?…                 the read-only page (threads `read_only_page`)
//!     GET /family/s/{agent}/feed/{thread}/events                     SSE `thread` hints
//!     GET /family/s/{agent}/feed/{thread}/runs/{run}/events          SSE the reply's words (deltas, its end)
//!     GET /family/s/{agent}/feed/{thread}/attachments/{attachment}   the file
//!
//! A conversation the owner's list doesn't show (hidden, a room, another agent's), an item before the share with
//! `from_now`, anything of the family's agent: 404.
//!
//! **The relative's side** (her bridge, the routes of her own agents with `agent=<agent>__<household>`, relayed by
//! agent_shares.rs): `GET /v1/threads` (each thread `new`, `read_only`), `…/{thread}/timeline` (marks it seen),
//! `GET /v1/threads/events`, `GET /v1/runs/{run}/events`, and each attachment by its `url`
//! `GET /v1/shared-agents/{id}/threads/{thread}/attachments/{attachment}`. What she opened: `seen` {thread: version}
//! in her `shared-with-me.json`.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Map, Value};

use crate::agent_shares::{self as shares, Window};
use crate::app::AppState;
use crate::config::AgentConfig;
use crate::threads::timeline::Item;
use crate::threads::Thread;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError, QueryMap};

/// An item's fields the owner's app keeps to itself.
const PRIVATE_FIELDS: [&str; 6] = ["reasoning", "client_id", "renamed", "tool", "steps", "steered"];
/// An attachment's fields a relative gets (no `path`: she opens it by its `url`).
const ATTACHMENT_FIELDS: [&str; 6] = ["id", "kind", "name", "mime", "size", "offset"];
/// The owner's list kept this long on her bridge for « Nouveau » (`has_new`).
const LIST_SECONDS: f64 = 30.0;

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

// -- the owner's side ---------------------------------------------------------------------------------------------

/// The personal agent the calling household reads, and what of it (404: not shared with her, or the family's agent).
fn reader(state: &AppState, headers: &HeaderMap, agent: &str) -> ApiResult<(String, AgentConfig, Window)> {
    let (household, agent) = shares::shared_agent(state, headers, agent)?;
    let window = shares::window(&state.config(), &agent.name, &household).ok_or_else(HttpError::not_found)?;
    Ok((household, agent, window))
}

/// One of the owner's conversations as his own list shows it (`main`: his main one), else 404.
async fn shown_thread(state: &AppState, agent: &AgentConfig, id: &str) -> ApiResult<Thread> {
    let (_, thread) = crate::threads::threads_of(state).named(Some(agent), id).await?;
    if thread.hidden || thread.agent != agent.name {
        return Err(HttpError::not_found());
    }
    Ok(thread)
}

/// What she reads of a conversation: the owner's messages and the agent's replies, from `from` on.
pub fn readable(item: &Item, from: f64) -> bool {
    item.kind() == "message" && (from <= 0.0 || item.at >= from)
}

/// An item as she gets it: without what the owner's app keeps to itself, its attachments without their path.
pub fn for_relative(mut item: Value) -> Value {
    if let Some(row) = item.as_object_mut() {
        for k in PRIVATE_FIELDS {
            row.remove(k);
        }
        if let Some(Value::Array(atts)) = row.get_mut("attachments") {
            for a in atts.iter_mut() {
                if let Some(att) = a.as_object_mut() {
                    att.retain(|k, _| ATTACHMENT_FIELDS.contains(&k.as_str()));
                }
            }
        }
    }
    item
}

/// The owner's conversations she may read, newest first: those with something since the share (`from_now`), or all.
pub fn listed_for(threads: &[Value], window: Window) -> Vec<Value> {
    threads
        .iter()
        .filter(|t| window.all || t.get("last_active").and_then(Value::as_f64).unwrap_or(0.0) >= window.since)
        .map(|t| json!({"id": t.get("id"), "kind": t.get("kind"), "title": t.get("title"),
                        "last_active": t.get("last_active"), "version": t.get("version")}))
        .collect()
}

async fn served_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
) -> ApiResult<Response> {
    let (_, agent, window) = reader(&state, &headers, &agent)?;
    let all = crate::threads::listed(&state, &agent, false).await?;
    let mut threads = listed_for(&all, window);
    let at = |t: &Value| t.get("last_active").and_then(Value::as_f64).unwrap_or(0.0);
    threads.sort_by(|a, b| at(b).total_cmp(&at(a)));
    let history = if window.all { shares::ALL } else { shares::FROM_NOW };
    Ok(web::ok(json!({"threads": threads, "history": history, "since": window.since})))
}

async fn served_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, thread)): UrlPath<(String, String)>,
    q: QueryMap,
) -> ApiResult<Response> {
    let (_, agent, window) = reader(&state, &headers, &agent)?;
    let thread = shown_thread(&state, &agent, &thread).await?;
    let want = crate::threads::want_of(&q)?;
    let from = window.from();
    let mut page = crate::threads::read_only_page(&state, &agent, &thread, &want, |i| readable(i, from)).await?;
    if let Some(Value::Array(items)) = page.get_mut("items") {
        for item in items.iter_mut() {
            *item = for_relative(std::mem::take(item));
        }
    }
    Ok(web::ok(page))
}

async fn served_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, thread)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (_, agent, _) = reader(&state, &headers, &agent)?;
    let thread = shown_thread(&state, &agent, &thread).await?;
    Ok(shares::thread_hints(&state, thread.id.clone(), thread.id))
}

async fn served_run_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, thread, run_id)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (_, agent, _) = reader(&state, &headers, &agent)?;
    let thread = shown_thread(&state, &agent, &thread).await?;
    let threads = crate::threads::threads_of(&state);
    let segments = threads.line(&agent, &thread).await.map_err(|_| HttpError::new(502, "the agent can't be reached"))?;
    let going = segments.last().and_then(|tip| state.hub.session_run_since(&agent.name, &tip.session_id));
    if going.is_none_or(|(run, _)| run != run_id) {
        return Err(HttpError::new(404, "no such turn"));
    }
    Ok(shares::turn_words(&state, &agent, &run_id, false))
}

async fn served_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, thread, attachment)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (household, agent, window) = reader(&state, &headers, &agent)?;
    let thread = shown_thread(&state, &agent, &thread).await?;
    let (message, n) = attachment.rsplit_once(".a").ok_or_else(HttpError::not_found)?;
    let n: usize = n.parse().map_err(|_| HttpError::not_found())?;
    let from = window.from();
    let items = crate::threads::items_of(&state, &agent, &thread).await?;
    let item = items.iter().find(|i| i.id == message && readable(i, from)).ok_or_else(HttpError::not_found)?;
    let att = item.body.get("attachments").and_then(|a| a.get(n)).cloned().ok_or_else(HttpError::not_found)?;
    tracing::info!(agent = %agent.name, household = %household, "shared agent: a relative opens a file of a conversation");
    if let Some(path) = att.get("path").and_then(Value::as_str) {
        return crate::app::media_file(&state, path, Some(agent.name.clone()), &headers).await;
    }
    let (mime, bytes) = crate::threads::photo_of(&state, &agent, item, n).await?;
    let mut response = Response::new(Body::from(bytes));
    let h = response.headers_mut();
    h.insert("content-type", HeaderValue::from_str(&mime).map_err(anyhow::Error::from)?);
    h.insert("cache-control", HeaderValue::from_static("private, max-age=86400"));
    Ok(response)
}

/// What a linked household's bridge may call for a personal agent shared with it (its key).
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/s/{agent}/feed", get(served_feed))
        .route("/family/s/{agent}/feed/{thread}/timeline", get(served_timeline))
        .route("/family/s/{agent}/feed/{thread}/events", get(served_events))
        .route("/family/s/{agent}/feed/{thread}/runs/{run_id}/events", get(served_run_events))
        .route("/family/s/{agent}/feed/{thread}/attachments/{attachment}", get(served_attachment))
}

// -- the relative's side ------------------------------------------------------------------------------------------

/// The owner's list as her bridge last had it, per shared agent: (when, threads).
type Lists = HashMap<String, (f64, Vec<Value>)>;
static LISTS: LazyLock<Mutex<Lists>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// What she has opened of shared agent `id`: {thread: version}.
fn seen(state: &AppState, id: &str) -> Map<String, Value> {
    let data = shares::read(&shares::mine_path_of(&state.config()));
    shares::object(&data, "agents")
        .and_then(|a| a.get(id))
        .and_then(|a| a.get("seen"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn mark_seen(state: &AppState, id: &str, thread: &str, version: i64) {
    let kept = shares::update(&shares::mine_path_of(&state.config()), |data| {
        if let Some(Value::Object(a)) = shares::object_mut(data, "agents").get_mut(id) {
            let seen = a.entry("seen").or_insert_with(|| json!({}));
            if !seen.is_object() {
                *seen = json!({});
            }
            seen[thread] = json!(version);
        }
    });
    if let Err(err) = kept {
        tracing::info!(error = %format!("{err:#}"), "shared agent: what she opened was not kept");
    }
}

/// Whether thread `t` (of the owner's list) has something she hasn't opened.
pub fn is_new(t: &Value, seen: &Map<String, Value>) -> bool {
    let version = t.get("version").and_then(Value::as_i64).unwrap_or(0);
    seen.get(&str_of(t, "id")).and_then(Value::as_i64).is_none_or(|v| version > v)
}

async fn owner_list(state: &AppState, id: &str) -> ApiResult<(u16, Value)> {
    let answer = shares::to_owner(state, id, reqwest::Method::GET, "feed", None).await?;
    if answer.0 == 200 {
        let threads = answer.1.get("threads").and_then(Value::as_array).cloned().unwrap_or_default();
        LISTS.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), (crate::py::time_time(), threads));
    }
    Ok(answer)
}

/// « Nouveau » on the shared agent `id`: something in its owner's conversations she hasn't opened (the owner's list
/// asked again past `LIST_SECONDS`; his side not answering: nothing new).
pub async fn has_new(state: &AppState, id: &str) -> bool {
    let cached = LISTS.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned();
    let threads = match cached {
        Some((at, threads)) if crate::py::time_time() - at < LIST_SECONDS => threads,
        _ => match tokio::time::timeout(Duration::from_secs(3), owner_list(state, id)).await {
            Ok(Ok((200, list))) => list.get("threads").and_then(Value::as_array).cloned().unwrap_or_default(),
            _ => return false,
        },
    };
    let seen = seen(state, id);
    threads.iter().any(|t| is_new(t, &seen))
}

/// Her relayed page: its thread under its id as the tip (the apps key a turn by it), each attachment with its `url` on
/// her bridge.
pub fn under_her_bridge(id: &str, mut page: Value) -> Value {
    let thread = page.get("thread").map(|t| str_of(t, "id")).unwrap_or_default();
    if let Some(Value::Object(t)) = page.get_mut("thread") {
        t.insert("tip".into(), json!(thread));
        t.insert("read_only".into(), json!(true));
    }
    if let Some(Value::Array(items)) = page.get_mut("items") {
        for item in items {
            if let Some(Value::Array(atts)) = item.get_mut("attachments") {
                for a in atts {
                    let att = str_of(a, "id");
                    a["url"] = json!(format!("/v1/shared-agents/{id}/threads/{thread}/attachments/{att}"));
                }
            }
        }
    }
    page
}

/// `/v1/threads…?agent=<id>` for a personal shared agent (agent_shares.rs `relay_thread`): her list, a page, and the
/// writes refused.
pub async fn relay_thread(state: &AppState, id: &str, what: &str, q: &QueryMap) -> ApiResult<Response> {
    if what.ends_with(":delete") {
        return Err(shares::read_only_refused());
    }
    if what.is_empty() {
        let (status, list) = owner_list(state, id).await?;
        let seen = seen(state, id);
        let threads: Vec<Value> = list
            .get("threads")
            .and_then(Value::as_array)
            .map(|all| {
                all.iter()
                    .map(|t| json!({"id": t.get("id"), "agent": id, "kind": t.get("kind"), "title": t.get("title"),
                                    "hidden": false, "version": t.get("version"), "last_active": t.get("last_active"),
                                    "new": is_new(t, &seen), "read_only": true}))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(web::json_response(status, &json!({"threads": threads})));
    }
    let Some((thread, "timeline")) = what.split_once('/') else { return Err(HttpError::not_found()) };
    let query: Vec<(String, String)> = q.pairs.iter().filter(|(k, _)| k != "agent").cloned().collect();
    let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(query).finish();
    let (status, page) =
        shares::to_owner(state, id, reqwest::Method::GET, &format!("feed/{thread}/timeline?{encoded}"), None).await?;
    if status != 200 {
        return Ok(web::json_response(status, &page));
    }
    let page = under_her_bridge(id, page);
    let shown = page.get("thread").map(|t| str_of(t, "id")).unwrap_or_default();
    let version = page.get("version").and_then(Value::as_i64).unwrap_or(0);
    if !shown.is_empty() {
        mark_seen(state, id, &shown, version);
    }
    Ok(web::ok(page))
}

/// A stream of the owner's (`events`, `runs/<run>/events` of `thread`), passed on as it comes.
pub async fn relay_stream(state: &AppState, id: &str, thread: &str, path: &str) -> ApiResult<Response> {
    let resp = shares::owner_request(state, id, reqwest::Method::GET, &format!("feed/{thread}/{path}"))?
        .timeout(Duration::from_secs(24 * 3600))
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    if !resp.status().is_success() {
        return Err(HttpError::new(resp.status().as_u16(), "their side refused"));
    }
    let mut chunks = resp.bytes_stream();
    let stream = async_stream::stream! {
        use futures::StreamExt;
        while let Some(chunk) = chunks.next().await {
            match chunk {
                Ok(bytes) => yield Ok::<_, std::io::Error>(bytes),
                Err(_) => break,
            }
        }
    };
    Ok(shares::sse(stream))
}

/// `GET /v1/shared-agents/{id}/threads/{thread}/attachments/{attachment}`: a file of the owner's conversation, from
/// his bridge (a page keeps its sandbox; any other file is never a document of this origin).
async fn relayed_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((id, thread, attachment)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !safe(&thread) || !safe(&attachment) || !shares::read_only(&state.config(), &id) {
        return Err(HttpError::not_found());
    }
    let mut request = shares::owner_request(&state, &id, reqwest::Method::GET, &format!("feed/{thread}/attachments/{attachment}"))?
        .timeout(Duration::from_secs(600));
    for name in ["if-none-match", "if-modified-since", "range"] {
        if let Some(v) = headers.get(name) {
            request = request.header(name, v.clone());
        }
    }
    let resp = request.send().await.map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    if !(resp.status().is_success() || status == 304) {
        return Err(HttpError::new(if status == 404 { 404 } else { 502 }, "this file can't be had right now"));
    }
    let kept: Vec<(&str, HeaderValue)> = ["content-type", "content-length", "content-disposition", "etag", "last-modified",
                                          "accept-ranges", "content-range", "content-security-policy", "cache-control"]
        .into_iter()
        .filter_map(|n| resp.headers().get(n).map(|v| (n, v.clone())))
        .collect();
    let mut response = Response::new(Body::from_stream(resp.bytes_stream()));
    *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK);
    let h = response.headers_mut();
    for (name, value) in kept {
        h.insert(axum::http::HeaderName::from_static(name), value);
    }
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    if !h.contains_key("content-security-policy") {
        h.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'"));
    }
    Ok(response)
}

/// For her app (under /v1, her bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/shared-agents/{id}/threads/{thread}/attachments/{attachment}", get(relayed_attachment))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: &str, at: f64) -> Item {
        Item {
            id: "m_1".into(),
            pos: (1, 1, 0),
            at,
            turn: String::new(),
            segment: "s".into(),
            row: 1,
            body: json!({"kind": kind, "role": "agent", "text": "x"}).as_object().cloned().unwrap(),
        }
    }

    #[test]
    fn she_reads_the_messages_from_the_share_on_or_all_of_them() {
        assert!(readable(&item("message", 200.0), 100.0));
        assert!(!readable(&item("message", 50.0), 100.0), "before the share, `from_now`");
        assert!(readable(&item("message", 50.0), 0.0), "`all`");
        for kind in ["tool", "steps", "boite", "notice", "carnet_line", "card"] {
            assert!(!readable(&item(kind, 200.0), 0.0), "{kind}");
        }
        assert_eq!(Window { all: true, since: 100.0 }.from(), 0.0);
        assert_eq!(Window { all: false, since: 100.0 }.from(), 100.0);
    }

    #[test]
    fn an_item_reaches_her_without_what_the_owner_keeps_and_its_files_by_url() {
        let page = json!({"thread": {"id": "t_1", "tip": "sess"}, "items": [for_relative(json!({
            "id": "m_2", "kind": "message", "role": "agent", "text": "Voici", "reasoning": "secret", "client_id": "C1",
            "attachments": [{"id": "m_2.a0", "kind": "page", "name": "menu.html", "path": "/bipagents/pages/cuisine/menu.html"}]}))]});
        let page = under_her_bridge("cuisine__alex", page);
        let it = &page["items"][0];
        assert!(it.get("reasoning").is_none() && it.get("client_id").is_none());
        assert!(it["attachments"][0].get("path").is_none());
        assert_eq!(it["attachments"][0]["url"], "/v1/shared-agents/cuisine__alex/threads/t_1/attachments/m_2.a0");
        assert_eq!(page["thread"]["tip"], "t_1");
        assert_eq!(page["thread"]["read_only"], true);
    }

    #[test]
    fn the_conversations_listed_are_those_since_the_share_or_all() {
        let threads = [json!({"id": "t_old", "last_active": 50.0, "version": 3}),
                       json!({"id": "t_new", "last_active": 150.0, "version": 1, "unread": 2, "tip": "x"})];
        let ids = |w| listed_for(&threads, w).iter().map(|t| str_of(t, "id")).collect::<Vec<_>>();
        assert_eq!(ids(Window { all: false, since: 100.0 }), ["t_new"]);
        assert_eq!(ids(Window { all: true, since: 100.0 }), ["t_old", "t_new"]);
        assert!(listed_for(&threads, Window { all: true, since: 0.0 })[1].get("tip").is_none());
        let mut seen = Map::new();
        assert!(is_new(&threads[1], &seen));
        seen.insert("t_new".into(), json!(1));
        assert!(!is_new(&threads[1], &seen));
        seen.insert("t_old".into(), json!(2));
        assert!(is_new(&threads[0], &seen));
    }
}
