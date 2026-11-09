//! Two linked households when only one of them can be reached: a self-install at home (its address on a tailnet or a
//! home network) linked to a place of the offer (a public address). The home's bridge reaches the place; the place
//! cannot reach the home. So the home fetches what the place has for it.
//!
//! - At the link (peers.rs `join`), a bridge whose own address is not reachable from the internet
//!   ([`crate::households::private_address`]) asks to pull (`"pull": true`); the other bridge keeps a relay token in
//!   that household's entry (`"relay"`) and answers `"relay": true`; the joiner keeps `"pull": true` in its own.
//! - On the reachable side, every call to that household goes to [`reach_url`]: a loopback listener of this bridge
//!   ([`serve_loopback`]) that keeps the request until the household's bridge takes it (`GET /family/relay/next`, a
//!   long poll with its key), then streams back the answer it sends (`POST /family/relay/answer/<id>`). The callers
//!   (peers.rs, family_news.rs, agent_shares.rs…) are unchanged: they call an address, with the key they always send.
//! - On the pulling side, [`pull_forever`] keeps one long poll per such household, runs each request through this
//!   bridge's own routes in-process (only `/family/…`, checked by its key as any other household call) and posts the
//!   answer back, streamed.
//!
//! A request nobody takes within [`PICKUP`] (the home's bridge off, its network down) fails as a household that does
//! not answer always has (502), so every caller keeps its own handling of an unreachable relative.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};

use crate::app::AppState;
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

/// How long a request waits for the household's bridge to take it.
pub const PICKUP: Duration = Duration::from_secs(15);
/// How long it then waits for the answer to begin (a household message's turn may last 600 s).
pub const ANSWER: Duration = Duration::from_secs(660);
/// One long poll.
pub const POLL: Duration = Duration::from_secs(25);

struct Pending {
    id: String,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Bytes,
    picked: oneshot::Sender<()>,
    answer: oneshot::Sender<Answer>,
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Body,
}

/// Requests waiting for each household (by id), and the bell its long poll waits on.
static QUEUES: LazyLock<Mutex<HashMap<String, VecDeque<Pending>>>> = LazyLock::new(Default::default);
static BELLS: LazyLock<Mutex<HashMap<String, Arc<Notify>>>> = LazyLock::new(Default::default);
/// Requests taken, waiting for their answer: id → (household, where the answer goes).
static WAITING: LazyLock<Mutex<HashMap<String, (String, oneshot::Sender<Answer>)>>> = LazyLock::new(Default::default);
/// The loopback listener's port.
static PORT: OnceLock<u16> = OnceLock::new();
/// This bridge's routes, for the requests a pulling household runs in-process.
static ROUTER: Mutex<Option<Router>> = Mutex::new(None);
/// The households a long poll runs for.
static PULLING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

fn bell(household: &str) -> Arc<Notify> {
    BELLS.lock().unwrap_or_else(|e| e.into_inner()).entry(household.to_string()).or_default().clone()
}

/// Hop-by-hop headers and those the other side sets itself.
fn kept(name: &str) -> bool {
    !matches!(
        name,
        "host" | "connection" | "keep-alive" | "transfer-encoding" | "te" | "trailer" | "upgrade" | "content-length"
            | "proxy-connection"
    ) && !name.starts_with("x-bip-relay-")
}

fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(n, _)| kept(n.as_str()))
        .filter_map(|(n, v)| v.to_str().ok().map(|v| (n.as_str().to_string(), v.to_string())))
        .collect()
}

fn put_headers(target: &mut HeaderMap, pairs: &[(String, String)]) {
    for (name, value) in pairs {
        if !kept(&name.to_ascii_lowercase()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value)) {
            target.append(n, v);
        }
    }
}

/// The relay token of a household's entry, when its calls go through the relay.
fn token_of(entry: &Dict) -> Option<&str> {
    entry.get("relay").and_then(Value::as_str).filter(|t| t.len() >= 16)
}

/// Where this bridge calls a linked household: its address, or this bridge's loopback relay for a household that pulls.
pub fn reach_url(entry: &Dict) -> String {
    if let (Some(token), Some(port)) = (token_of(entry), PORT.get()) {
        return format!("http://127.0.0.1:{port}/{token}");
    }
    entry.get("url").map(crate::py::py_str).unwrap_or_default()
}

/// Whether this household's calls go through the relay (it pulls them).
pub fn relayed(entry: &Dict) -> bool {
    token_of(entry).is_some()
}

/// Whether we pull this household's calls (our address can't be reached from theirs).
pub fn pulls(entry: &Dict) -> bool {
    entry.get("pull").and_then(Value::as_bool).unwrap_or(false)
}

/// A new relay token (kept in the household's entry on the reachable side).
pub fn new_token() -> String {
    crate::py::token_urlsafe(24)
}

/// This bridge's routes, for the in-process requests (app.rs, once the app is assembled).
pub fn set_router(router: Router) {
    *ROUTER.lock().unwrap_or_else(|e| e.into_inner()) = Some(router);
}

// -- the reachable side ---------------------------------------------------------------------------------------------

/// The loopback listener the callers reach a pulling household at (started once, in the background).
pub async fn serve_loopback(state: AppState) {
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "family relay: no loopback listener");
            return;
        }
    };
    let Ok(address) = listener.local_addr() else { return };
    let _ = PORT.set(address.port());
    let app = Router::new().fallback(relay_in).with_state(state);
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!(error = %e, "family relay listener stopped");
    }
}

fn household_of_token(state: &AppState, token: &str) -> Option<String> {
    let data = state.households.load();
    let households = data.get("households")?.as_object()?;
    households
        .iter()
        .find(|(_, v)| {
            v.get("relay")
                .and_then(Value::as_str)
                .is_some_and(|t| crate::auth::constant_time_eq(t.as_bytes(), token.as_bytes()))
        })
        .map(|(id, _)| id.clone())
}

fn unreachable() -> Response {
    axum::response::IntoResponse::into_response(HttpError::new(502, "their side does not answer right now"))
}

/// `http://127.0.0.1:<port>/<token>/family/…`: a call to a pulling household, kept until its bridge takes it.
async fn relay_in(State(state): State<AppState>, request: Request) -> Response {
    let path_and_query = request.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
    let trimmed = path_and_query.trim_start_matches('/');
    let (token, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
    let Some(household) = household_of_token(&state, token) else {
        return axum::response::IntoResponse::into_response(HttpError::not_found());
    };
    let method = request.method().as_str().to_string();
    let headers = header_pairs(request.headers());
    let Ok(body) = axum::body::to_bytes(request.into_body(), crate::body_limit::HERMES_BYTES as usize).await else {
        return axum::response::IntoResponse::into_response(HttpError::new(413, "request body too large"));
    };
    let id = crate::py::token_urlsafe(16);
    let (picked_tx, picked_rx) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();
    let pending = Pending {
        id: id.clone(),
        method,
        path: format!("/{rest}"),
        headers,
        body,
        picked: picked_tx,
        answer: answer_tx,
    };
    QUEUES.lock().unwrap_or_else(|e| e.into_inner()).entry(household.clone()).or_default().push_back(pending);
    bell(&household).notify_one();
    if !matches!(tokio::time::timeout(PICKUP, picked_rx).await, Ok(Ok(()))) {
        // nobody took it: the household's bridge is off or out of reach
        if let Some(queue) = QUEUES.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&household) {
            queue.retain(|p| p.id != id);
        }
        tracing::info!(household = %household, "family relay: not taken (their bridge does not poll)");
        return unreachable();
    }
    let answer = match tokio::time::timeout(ANSWER, answer_rx).await {
        Ok(Ok(answer)) => answer,
        _ => {
            WAITING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return unreachable();
        }
    };
    let mut response = Response::new(answer.body);
    *response.status_mut() = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY);
    put_headers(response.headers_mut(), &answer.headers);
    response
}

fn caller(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let header =
        headers.get("authorization").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    let token = header.strip_prefix("Bearer ").unwrap_or(&header).trim().to_string();
    let household = if token.is_empty() { None } else { state.households.caller(&token) };
    let household = household.ok_or_else(|| HttpError::new(401, "unknown household"))?;
    match state.households.get(&household) {
        Some(entry) if relayed(&entry) => Ok(household),
        _ => Err(HttpError::new(404, "this household is reached at its own address")),
    }
}

/// `GET /family/relay/next` (the household's key): the next request for it, or 204 after [`POLL`].
async fn next(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let household = caller(&state, &headers)?;
    let bell = bell(&household);
    let deadline = tokio::time::Instant::now() + POLL;
    loop {
        let found = QUEUES.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&household).and_then(VecDeque::pop_front);
        if let Some(pending) = found {
            if pending.picked.send(()).is_err() {
                continue; // its caller gave up meanwhile
            }
            WAITING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(pending.id.clone(), (household.clone(), pending.answer));
            let body = base64::engine::general_purpose::STANDARD.encode(&pending.body);
            return Ok(web::ok(json!({"id": pending.id, "method": pending.method, "path": pending.path,
                                      "headers": pending.headers, "body": body})));
        }
        if tokio::time::timeout_at(deadline, bell.notified()).await.is_err() {
            return Ok(web::no_content());
        }
    }
}

/// `POST /family/relay/answer/<id>` (the household's key): the answer's status and headers in `x-bip-relay-status`
/// and `x-bip-relay-headers` (a JSON list of pairs), its body streamed as it comes.
async fn answer(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    request: Request,
) -> ApiResult<Response> {
    let household = caller(&state, request.headers())?;
    let waiting = {
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        match waiting.get(&id) {
            Some((owner, _)) if *owner == household => waiting.remove(&id).map(|(_, tx)| tx),
            _ => None,
        }
    };
    let Some(tx) = waiting else { return Err(HttpError::new(404, "no such request")) };
    let headers = request.headers();
    let status = headers
        .get("x-bip-relay-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(502);
    let pairs: Vec<(String, String)> = headers
        .get("x-bip-relay-headers")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| serde_json::from_str(v).ok())
        .unwrap_or_default();
    let (chunks_tx, chunks_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let stream = futures::stream::unfold(chunks_rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) });
    if tx.send(Answer { status, headers: pairs, body: Body::from_stream(stream) }).is_err() {
        return Ok(web::no_content()); // the caller went away
    }
    let mut body = request.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(std::io::Error::other);
        let failed = chunk.is_err();
        if chunks_tx.send(chunk).await.is_err() || failed {
            break;
        }
    }
    Ok(web::no_content())
}

/// The routes a pulling household's bridge calls (its key). Mounted at the root, with the other household routes.
pub fn household_router() -> Router<AppState> {
    Router::new().route("/family/relay/next", get(next)).route("/family/relay/answer/{id}", post(answer))
}

// -- the pulling side -----------------------------------------------------------------------------------------------

/// Keeps one long poll for each household whose calls we pull.
pub async fn pull_forever(state: AppState) {
    loop {
        let data = state.households.load();
        if let Some(households) = data.get("households").and_then(Value::as_object) {
            for (id, entry) in households {
                let Some(entry) = entry.as_object() else { continue };
                if !pulls(entry) {
                    continue;
                }
                let fresh = PULLING.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone());
                if fresh {
                    tokio::spawn(poll(state.clone(), id.clone()));
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn poll(state: AppState, household: String) {
    let mut pause = Duration::from_secs(2);
    loop {
        let Some(entry) = state.households.get(&household).filter(pulls) else { break };
        let url = entry.get("url").map(crate::py::py_str).unwrap_or_default();
        let key = entry.get("key_out").map(crate::py::py_str).unwrap_or_default();
        let answer = crate::door_tls::household_client(&state.http, &entry)
            .get(format!("{url}/family/relay/next"))
            .timeout(POLL + Duration::from_secs(15))
            .bearer_auth(&key)
            .send()
            .await;
        match answer {
            Ok(resp) if resp.status() == StatusCode::OK => {
                pause = Duration::from_secs(2);
                if let Ok(job) = resp.json::<Value>().await {
                    tokio::spawn(run(state.clone(), entry, job));
                }
            }
            Ok(resp) if resp.status() == StatusCode::NO_CONTENT => pause = Duration::from_secs(2),
            Ok(resp) => {
                tracing::info!(household = %household, status = resp.status().as_u16(), "family relay: poll refused");
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(60));
            }
        }
    }
    PULLING.lock().unwrap_or_else(|e| e.into_inner()).remove(&household);
}

/// One request the other household's bridge had for us: run through our own routes, its answer posted back.
async fn run(state: AppState, entry: Dict, job: Value) {
    let id = job.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
    let path = job.get("path").and_then(Value::as_str).unwrap_or("/").to_string();
    let method = job.get("method").and_then(Value::as_str).unwrap_or("GET");
    let pairs: Vec<(String, String)> = job.get("headers").cloned().and_then(|h| serde_json::from_value(h).ok()).unwrap_or_default();
    let body = job
        .get("body")
        .and_then(Value::as_str)
        .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        .unwrap_or_default();
    let router = ROUTER.lock().unwrap_or_else(|e| e.into_inner()).clone();
    // only what a linked household may call anyway (its key is checked there), never the relay itself
    let allowed = path.starts_with("/family/") && !path.starts_with("/family/relay/") && !path.contains("/../");
    let response = match (router, allowed, axum::http::Method::from_bytes(method.as_bytes())) {
        (Some(router), true, Ok(method)) => {
            let mut request = Request::new(Body::from(body));
            *request.method_mut() = method;
            *request.uri_mut() = path.parse().unwrap_or_default();
            put_headers(request.headers_mut(), &pairs);
            match tower::ServiceExt::oneshot(router, request).await {
                Ok(r) => r,
                Err(never) => match never {},
            }
        }
        _ => axum::response::IntoResponse::into_response(HttpError::not_found()),
    };
    let status = response.status().as_u16();
    let headers = serde_json::to_string(&header_pairs(response.headers())).unwrap_or_else(|_| "[]".into());
    let url = entry.get("url").map(crate::py::py_str).unwrap_or_default();
    let key = entry.get("key_out").map(crate::py::py_str).unwrap_or_default();
    let stream = response.into_body().into_data_stream();
    let sent = crate::door_tls::household_client(&state.http, &entry)
        .post(format!("{url}/family/relay/answer/{id}"))
        .timeout(ANSWER + Duration::from_secs(60))
        .bearer_auth(key)
        .header("x-bip-relay-status", status.to_string())
        .header("x-bip-relay-headers", headers)
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await;
    if let Err(e) = sent {
        tracing::info!(error = %e, "family relay: answer not sent");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relayed_household_is_reached_through_the_loopback_listener() {
        let mut entry = Dict::new();
        entry.insert("url".into(), json!("http://100.101.102.103:8642"));
        assert_eq!(reach_url(&entry), "http://100.101.102.103:8642");
        assert!(!relayed(&entry) && !pulls(&entry));
        entry.insert("relay".into(), json!("short"));
        assert!(!relayed(&entry));
        entry.insert("relay".into(), json!(new_token()));
        assert!(relayed(&entry));
        let _ = PORT.set(4321);
        assert!(reach_url(&entry).starts_with("http://127.0.0.1:4321/"));
        assert!(kept("authorization") && !kept("host") && !kept("x-bip-relay-status"));
    }
}
