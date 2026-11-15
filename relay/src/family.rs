//! The family rendezvous (2026-11-15): two households' bridges that can't reach each other (each at home behind its
//! own box) link and talk through the relay, both connecting out. The model is the bridge's own family_relay.rs (a
//! home pulling what a reachable place keeps for it), held here instead, for any two bridges.
//!
//! - A bridge takes a **mailbox**: `POST /v1/family/mailbox` → a random id (16 bytes, base64url) and a token that
//!   holds it, sealed with the relay's keys (`fm1.`, like the push tokens: nothing stored, any copy opens it). The id
//!   is what another household calls; only the token reads the mailbox. Renewed with the old token, same id.
//! - The bridge keeps one long poll on its mailbox (`GET /v1/family/next`, [`FamilyConfig::poll`]).
//! - Another bridge calls it with its own token (`POST /v1/family/call/<id>`, `x-bip-kind: call|invite`): the body
//!   waits here, in memory, until that mailbox's poll takes it ([`FamilyConfig::pickup`]), then the caller gets the
//!   answer the other bridge streams (`POST /v1/family/answer/<call>`) as it comes.
//!
//! What passes is sealed end to end by the two bridges (bridge-rs family_seal.rs, a key of each link that only the two
//! households hold): the relay never reads it, and a change it made would be refused by the bridge. What it sees:
//! the bridges' addresses, which mailbox calls which, when, how many bytes. What it keeps: nothing on disk; quotas
//! in memory on hashes, forgotten within the hour; no log line for a call that went through (no record of who talks
//! to whom), only for refusals (a mailbox's 8-hex fingerprint) and one count an hour.
//!
//! Abuse: no call without a mailbox, no address but a mailbox (no open proxy), calls and bytes counted per mailbox,
//! as caller and as callee, sizes capped, mailboxes made per address counted, a mailbox refused by its fingerprint
//! (`RELAY_BLOCKED`). One copy of the relay holds the rendezvous (it is in memory): run one, or share mailboxes out
//! by their id.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use futures_util::StreamExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio::time::Instant;

use crate::limits::Quota;
use crate::routes::Relay;
use crate::seal::now;

pub const MAILBOX_PREFIX: &str = "fm1.";
const MAILBOX_AAD: &[u8] = b"bipagents-family-mailbox-v1";
/// How long a mailbox token lasts; the bridges renew theirs well before (same mailbox).
pub const MAILBOX_LIFETIME_SECS: u64 = 400 * 86_400;
/// An ended token may still be renewed for this long (a bridge switched off for months keeps its mailbox).
const RENEW_GRACE_SECS: u64 = 400 * 86_400;

/// The family relay's settings (config.rs: `RELAY_FAMILY…`).
#[derive(Clone, Debug)]
pub struct FamilyConfig {
    pub enabled: bool,
    /// A call's request, sealed (`RELAY_FAMILY_REQUEST_MB`, 16).
    pub request_bytes: usize,
    /// Its answer (`RELAY_FAMILY_ANSWER_MB`, 64).
    pub answer_bytes: usize,
    /// Calls an hour, per mailbox as caller and as callee (`RELAY_FAMILY_CALLS_PER_HOUR`, 3,000).
    pub calls_per_hour: u32,
    /// Bytes an hour per mailbox, in KiB (`RELAY_FAMILY_MB_PER_HOUR`, 2,048 MiB).
    pub kib_per_hour: u32,
    /// Mailboxes made or renewed an hour, per address (`RELAY_FAMILY_MAILBOXES_PER_IP_HOUR`, 20).
    pub mailboxes_per_ip_hour: u32,
    /// Calls waiting for one mailbox.
    pub queue: usize,
    /// Bytes of all the calls waiting to be taken, every mailbox together (a call is taken within seconds).
    pub queued_bytes: usize,
    /// Long polls open at once on one mailbox (a bridge restarted leaves one behind for a moment).
    pub polls_per_mailbox: usize,
    /// How long a call waits for its mailbox's bridge to take it.
    pub pickup: Duration,
    /// Then for its answer to begin (a household message's turn may last 600 s).
    pub answer_start: Duration,
    /// One long poll.
    pub poll: Duration,
    /// A mailbox with no poll open for this long is away: its calls fail at once.
    pub away: Duration,
}

impl Default for FamilyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            request_bytes: 16 << 20,
            answer_bytes: 64 << 20,
            calls_per_hour: 3000,
            kib_per_hour: 2048 * 1024,
            mailboxes_per_ip_hour: 20,
            queue: 64,
            queued_bytes: 256 << 20,
            polls_per_mailbox: 4,
            pickup: Duration::from_secs(15),
            answer_start: Duration::from_secs(660),
            poll: Duration::from_secs(25),
            away: Duration::from_secs(10),
        }
    }
}

/// What a mailbox token holds.
#[derive(Serialize, Deserialize)]
struct Claims {
    #[serde(rename = "m")]
    id: String,
    #[serde(rename = "n")]
    iat: u64,
    #[serde(rename = "x")]
    exp: u64,
}

/// A call waiting for its mailbox's poll.
struct Call {
    id: String,
    size: usize,
    from: String,
    kind: &'static str,
    body: Bytes,
    picked: oneshot::Sender<()>,
    answer: oneshot::Sender<Body>,
}

#[derive(Default)]
struct Counts {
    calls: AtomicU64,
    answered: AtomicU64,
    refused: AtomicU64,
    bytes: AtomicU64,
}

/// The rendezvous, in memory.
pub struct Rendezvous {
    pub config: FamilyConfig,
    queues: Mutex<HashMap<String, VecDeque<Call>>>,
    bells: Mutex<HashMap<String, Arc<Notify>>>,
    /// Calls taken, waiting for their answer: call id → (the mailbox that took it, where the answer goes).
    waiting: Mutex<HashMap<String, (String, oneshot::Sender<Body>)>>,
    /// Each mailbox's polls: how many are open, and when one last began or ended.
    seen: Mutex<HashMap<String, (usize, Instant)>>,
    /// Bytes of the calls waiting in the queues.
    queued: std::sync::atomic::AtomicUsize,
    calls_out: Quota,
    calls_in: Quota,
    kib: Quota,
    registrations: Quota,
    counts: Counts,
    last_report: Mutex<Instant>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 22 base64url characters: 16 bytes.
pub fn valid_mailbox(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A mailbox's short fingerprint for the logs and `RELAY_BLOCKED`: never the id itself.
pub fn fingerprint(id: &str) -> String {
    Sha256::digest(id.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn kib(bytes: usize) -> u32 {
    u32::try_from(bytes.div_ceil(1024)).unwrap_or(u32::MAX)
}

impl Rendezvous {
    pub fn new(config: FamilyConfig) -> Self {
        Self {
            calls_out: Quota::new(config.calls_per_hour),
            calls_in: Quota::new(config.calls_per_hour),
            kib: Quota::new(config.kib_per_hour),
            registrations: Quota::new(config.mailboxes_per_ip_hour),
            config,
            queues: Mutex::default(),
            bells: Mutex::default(),
            waiting: Mutex::default(),
            seen: Mutex::default(),
            queued: std::sync::atomic::AtomicUsize::new(0),
            counts: Counts::default(),
            last_report: Mutex::new(Instant::now()),
        }
    }

    fn bell(&self, mailbox: &str) -> Arc<Notify> {
        lock(&self.bells).entry(mailbox.to_string()).or_default().clone()
    }

    /// How long a call may wait for this mailbox's bridge: `pickup` while a poll is open, what is left of `away`
    /// just after one ended, None when it does not poll (the call fails at once).
    fn pickup(&self, mailbox: &str) -> Option<Duration> {
        let seen = lock(&self.seen);
        let (open, last) = seen.get(mailbox)?;
        if *open > 0 {
            return Some(self.config.pickup);
        }
        self.config.away.checked_sub(last.elapsed()).filter(|left| !left.is_zero())
    }

    /// Mailboxes with a poll open now.
    pub fn online(&self) -> usize {
        lock(&self.seen).values().filter(|(open, _)| *open > 0).count()
    }

    /// Every minute (main.rs, with the push quotas): the quotas forget what is about to be an hour old, the mailboxes
    /// gone for an hour are forgotten, and once an hour one line of counts.
    pub fn sweep(&self) {
        self.calls_out.sweep();
        self.calls_in.sweep();
        self.kib.sweep();
        self.registrations.sweep();
        let hour = Duration::from_secs(3600);
        let gone: Vec<String> = lock(&self.seen)
            .iter()
            .filter(|(_, (open, last))| *open == 0 && last.elapsed() > hour)
            .map(|(id, _)| id.clone())
            .collect();
        {
            let mut seen = lock(&self.seen);
            let mut bells = lock(&self.bells);
            let mut queues = lock(&self.queues);
            for id in &gone {
                seen.remove(id);
                bells.remove(id);
                if let Some(left) = queues.remove(id) {
                    self.queued.fetch_sub(left.iter().map(|c| c.size).sum(), Ordering::Relaxed);
                }
            }
            bells.retain(|id, _| seen.contains_key(id) || queues.contains_key(id));
            queues.retain(|_, q| !q.is_empty());
        }
        let mut last = lock(&self.last_report);
        if last.elapsed() >= hour {
            *last = Instant::now();
            let take = |c: &AtomicU64| c.swap(0, Ordering::Relaxed);
            tracing::info!(
                calls = take(&self.counts.calls),
                answered = take(&self.counts.answered),
                refused = take(&self.counts.refused),
                bytes = take(&self.counts.bytes),
                online = self.online(),
                "family relay: the last hour"
            );
        }
    }
}

/// One open poll, counted while it lasts (also when its connection drops).
struct Polling<'a> {
    rendezvous: &'a Rendezvous,
    mailbox: String,
}

impl<'a> Polling<'a> {
    fn open(rendezvous: &'a Rendezvous, mailbox: &str) -> Option<Self> {
        let mut seen = lock(&rendezvous.seen);
        let entry = seen.entry(mailbox.to_string()).or_insert((0, Instant::now()));
        if entry.0 >= rendezvous.config.polls_per_mailbox {
            return None;
        }
        *entry = (entry.0 + 1, Instant::now());
        Some(Polling { rendezvous, mailbox: mailbox.to_string() })
    }
}

impl Drop for Polling<'_> {
    fn drop(&mut self) {
        if let Some(entry) = lock(&self.rendezvous.seen).get_mut(&self.mailbox) {
            *entry = (entry.0.saturating_sub(1), Instant::now());
        }
    }
}

/// A call kept for a mailbox: out of its queue and of the answers awaited when its caller goes, whenever that is.
struct Kept<'a> {
    rendezvous: &'a Rendezvous,
    mailbox: String,
    id: String,
}

impl Drop for Kept<'_> {
    fn drop(&mut self) {
        if let Some(queue) = lock(&self.rendezvous.queues).get_mut(&self.mailbox) {
            if let Some(at) = queue.iter().position(|c| c.id == self.id) {
                if let Some(call) = queue.remove(at) {
                    self.rendezvous.queued.fetch_sub(call.size, Ordering::Relaxed);
                }
            }
        }
        lock(&self.rendezvous.waiting).remove(&self.id);
    }
}

fn refuse(status: StatusCode, code: &str, detail: &str) -> Response {
    (status, Json(json!({"code": code, "detail": detail}))).into_response()
}

fn new_id(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn seal(relay: &Relay, id: &str, at: u64) -> (String, u64) {
    let claims = Claims { id: id.to_string(), iat: at, exp: at + MAILBOX_LIFETIME_SECS };
    let plain = serde_json::to_vec(&claims).expect("claims serialize");
    (format!("{MAILBOX_PREFIX}{}", relay.sealer.seal_bytes(&plain, MAILBOX_AAD)), claims.exp)
}

fn open(relay: &Relay, token: &str) -> Option<Claims> {
    let sealed = token.strip_prefix(MAILBOX_PREFIX)?;
    let claims: Claims = serde_json::from_slice(&relay.sealer.open_bytes(sealed, MAILBOX_AAD)?).ok()?;
    valid_mailbox(&claims.id).then_some(claims)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))?.trim())
}

/// The mailbox whose token this request carries (401: none, or ended; 403: blocked).
#[allow(clippy::result_large_err)] // the refusal is the handler's answer as it is, made once per request
fn authenticated(relay: &Relay, headers: &HeaderMap) -> Result<String, Response> {
    let claims = bearer(headers).and_then(|t| open(relay, t));
    let Some(claims) = claims else {
        return Err(refuse(
            StatusCode::UNAUTHORIZED,
            "unknown",
            "not a mailbox token: take one with /v1/family/mailbox",
        ));
    };
    if now() >= claims.exp {
        return Err(refuse(StatusCode::UNAUTHORIZED, "expired", "this mailbox token has ended: renew it"));
    }
    if relay.is_blocked_id(&fingerprint(&claims.id)) {
        tracing::info!(mailbox = %fingerprint(&claims.id), "family relay: blocked mailbox refused");
        relay.family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "blocked",
            "this mailbox is blocked: write to support@bipagents.com",
        ));
    }
    Ok(claims.id)
}

#[derive(Deserialize, Default)]
struct Register {
    #[serde(default)]
    token: Option<String>,
}

/// `POST /v1/family/mailbox {token?}`: a new mailbox, or the same one renewed.
async fn mailbox(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let family = &relay.family;
    if !family.registrations.allow(relay.client_ip(peer, &headers).as_bytes()) {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::TOO_MANY_REQUESTS, "quota", "too many mailboxes from this address");
    }
    let Ok(asked) = (if body.iter().all(u8::is_ascii_whitespace) {
        Ok(Register::default())
    } else {
        serde_json::from_slice::<Register>(&body)
    }) else {
        return refuse(StatusCode::BAD_REQUEST, "invalid", "invalid request");
    };
    let at = now();
    let id = match asked.token.as_deref() {
        Some(old) => match open(&relay, old) {
            Some(claims) if at < claims.exp.saturating_add(RENEW_GRACE_SECS) => claims.id,
            _ => return refuse(StatusCode::UNAUTHORIZED, "unknown", "not a mailbox token of this relay"),
        },
        None => new_id(16),
    };
    if relay.is_blocked_id(&fingerprint(&id)) {
        tracing::info!(mailbox = %fingerprint(&id), "family relay: blocked mailbox refused");
        return refuse(StatusCode::FORBIDDEN, "blocked", "this mailbox is blocked: write to support@bipagents.com");
    }
    let (token, expires) = seal(&relay, &id, at);
    Json(json!({"mailbox": id, "token": token, "expires": expires})).into_response()
}

/// `GET /v1/family/next` (the mailbox's token): the next call for it, or 204 after a poll.
async fn next(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    let mailbox = match authenticated(&relay, &headers) {
        Ok(m) => m,
        Err(refused) => return refused,
    };
    let family = &relay.family;
    let Some(_open) = Polling::open(family, &mailbox) else {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "polls", "too many polls open on this mailbox");
    };
    let bell = family.bell(&mailbox);
    let deadline = Instant::now() + family.config.poll;
    loop {
        let found = lock(&family.queues).get_mut(&mailbox).and_then(VecDeque::pop_front);
        if let Some(call) = found {
            family.queued.fetch_sub(call.size, Ordering::Relaxed);
            if call.picked.send(()).is_err() {
                continue; // its caller gave up meanwhile
            }
            lock(&family.waiting).insert(call.id.clone(), (mailbox.clone(), call.answer));
            let mut response = Response::new(Body::from(call.body));
            let headers = response.headers_mut();
            headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
            for (name, value) in
                [("x-bip-call", call.id.as_str()), ("x-bip-from", call.from.as_str()), ("x-bip-kind", call.kind)]
            {
                if let Ok(value) = HeaderValue::from_str(value) {
                    headers.insert(name, value);
                }
            }
            return response;
        }
        if tokio::time::timeout_at(deadline, bell.notified()).await.is_err() {
            return StatusCode::NO_CONTENT.into_response();
        }
    }
}

/// `POST /v1/family/call/<mailbox>` (the caller's own mailbox token; `x-bip-kind: call|invite`): the body kept
/// until that mailbox's bridge takes it, then its answer streamed back.
async fn call(
    State(relay): State<Arc<Relay>>,
    Path(target): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let from = match authenticated(&relay, &headers) {
        Ok(m) => m,
        Err(refused) => return refused,
    };
    let family = &relay.family;
    let refused = |status, code: &str, detail: &str| {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        refuse(status, code, detail)
    };
    if !valid_mailbox(&target) {
        return refused(StatusCode::NOT_FOUND, "invalid", "not a mailbox");
    }
    if target == from {
        return refused(StatusCode::BAD_REQUEST, "invalid", "a mailbox does not call itself");
    }
    if relay.is_blocked_id(&fingerprint(&target)) {
        return refused(StatusCode::FORBIDDEN, "blocked", "this mailbox is blocked");
    }
    let kind = match headers.get("x-bip-kind").and_then(|v| v.to_str().ok()).unwrap_or("call") {
        "call" => "call",
        "invite" => "invite",
        _ => return refused(StatusCode::BAD_REQUEST, "invalid", "unknown kind of call"),
    };
    // a refusal names the one mailbox over its quota, never the pair
    if !family.calls_out.allow(from.as_bytes()) {
        tracing::info!(mailbox = %fingerprint(&from), "family relay: calls over the quota (as caller)");
        return refused(StatusCode::TOO_MANY_REQUESTS, "quota", "too many calls this hour");
    }
    if !family.calls_in.allow(target.as_bytes()) {
        tracing::info!(mailbox = %fingerprint(&target), "family relay: calls over the quota (as callee)");
        return refused(StatusCode::TOO_MANY_REQUESTS, "quota", "too many calls this hour");
    }
    let Some(wait) = family.pickup(&target) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "away", "their bridge is not connected to the relay");
    };
    if lock(&family.queues).get(&target).is_some_and(|q| q.len() >= family.config.queue) {
        return refused(StatusCode::TOO_MANY_REQUESTS, "busy", "too many calls waiting for this mailbox");
    }
    let Ok(body) = axum::body::to_bytes(request.into_body(), family.config.request_bytes).await else {
        return refused(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "request too large");
    };
    if family.queued.load(Ordering::Relaxed) + body.len() > family.config.queued_bytes {
        tracing::warn!("family relay: too many bytes waiting, call refused");
        return refused(StatusCode::SERVICE_UNAVAILABLE, "busy", "the relay is busy: try again in a moment");
    }
    if !family.kib.allow_n(from.as_bytes(), kib(body.len())) {
        tracing::info!(mailbox = %fingerprint(&from), "family relay: bytes over the quota");
        return refused(StatusCode::TOO_MANY_REQUESTS, "quota", "too many bytes this hour");
    }
    family.counts.calls.fetch_add(1, Ordering::Relaxed);
    family.counts.bytes.fetch_add(body.len() as u64, Ordering::Relaxed);
    let id = new_id(12);
    let _kept = Kept { rendezvous: family, mailbox: target.clone(), id: id.clone() };
    let (picked_tx, picked_rx) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();
    let size = body.len();
    let call = Call { id: id.clone(), size, from, kind, body, picked: picked_tx, answer: answer_tx };
    family.queued.fetch_add(size, Ordering::Relaxed);
    lock(&family.queues).entry(target.clone()).or_default().push_back(call);
    family.bell(&target).notify_one();
    if !matches!(tokio::time::timeout(wait, picked_rx).await, Ok(Ok(()))) {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "away", "their bridge did not take the call");
    }
    let Ok(Ok(answer)) = tokio::time::timeout(family.config.answer_start, answer_rx).await else {
        return refuse(StatusCode::GATEWAY_TIMEOUT, "no_answer", "their bridge did not answer");
    };
    family.counts.answered.fetch_add(1, Ordering::Relaxed);
    let mut response = Response::new(answer);
    response.headers_mut().insert("content-type", HeaderValue::from_static("application/octet-stream"));
    response
}

/// `POST /v1/family/answer/<call>` (the token of the mailbox that took it): the answer, streamed to the caller as
/// it comes; cut past the answer's size or the mailbox's bytes of the hour.
async fn answer(
    State(relay): State<Arc<Relay>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let mailbox = match authenticated(&relay, &headers) {
        Ok(m) => m,
        Err(refused) => return refused,
    };
    let family = &relay.family;
    let waiting = {
        let mut waiting = lock(&family.waiting);
        match waiting.get(&id) {
            Some((owner, _)) if *owner == mailbox => waiting.remove(&id).map(|(_, tx)| tx),
            _ => None,
        }
    };
    let Some(tx) = waiting else { return refuse(StatusCode::NOT_FOUND, "unknown", "no such call") };
    let (chunks_tx, chunks_rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(16);
    let stream = futures_util::stream::unfold(chunks_rx, |mut rx| async move { rx.recv().await.map(|c| (c, rx)) });
    if tx.send(Body::from_stream(stream)).is_err() {
        return StatusCode::NO_CONTENT.into_response(); // the caller went away
    }
    let mut total = 0usize;
    let mut body = request.into_body().into_data_stream();
    while let Some(chunk) = body.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                let _ = chunks_tx.send(Err(std::io::Error::other(e))).await;
                break;
            }
        };
        total += chunk.len();
        if total > family.config.answer_bytes || !family.kib.allow_n(mailbox.as_bytes(), kib(chunk.len())) {
            tracing::info!(mailbox = %fingerprint(&mailbox), "family relay: answer cut (size or quota)");
            family.counts.refused.fetch_add(1, Ordering::Relaxed);
            let _ = chunks_tx.send(Err(std::io::Error::other("answer too large"))).await;
            break;
        }
        if chunks_tx.send(Ok(chunk)).await.is_err() {
            break; // the caller went away
        }
    }
    family.counts.bytes.fetch_add(total as u64, Ordering::Relaxed);
    StatusCode::NO_CONTENT.into_response()
}

/// What `/health` says of it.
pub fn health(family: &Rendezvous) -> Value {
    json!({"enabled": family.config.enabled, "online": family.online()})
}

/// The family routes (merged by routes.rs when the family relay is on), with their own size limits.
pub fn routes() -> Router<Arc<Relay>> {
    Router::new()
        .route("/v1/family/mailbox", post(mailbox))
        .route("/v1/family/next", get(next))
        .route("/v1/family/call/{mailbox}", post(call))
        .route("/v1/family/answer/{call}", post(answer))
        .layer(DefaultBodyLimit::disable())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mailbox_ids_and_their_fingerprints() {
        let id = new_id(16);
        assert!(valid_mailbox(&id), "{id}");
        assert!(!valid_mailbox("short") && !valid_mailbox(&format!("{}!", &id[..21])));
        assert_eq!(fingerprint(&id).len(), 8);
        assert_eq!(kib(0), 0);
        assert_eq!(kib(1), 1);
        assert_eq!(kib(2048), 2);
    }

    #[tokio::test]
    async fn a_mailbox_polls_while_a_poll_is_open_and_a_moment_after() {
        let family = Rendezvous::new(FamilyConfig::default());
        assert!(family.pickup("m").is_none(), "never polled: away");
        let open = Polling::open(&family, "m").unwrap();
        assert_eq!(family.pickup("m"), Some(family.config.pickup));
        assert_eq!(family.online(), 1);
        drop(open);
        assert!(family.pickup("m").is_some_and(|d| d <= family.config.away));
        let polls: Vec<_> = (0..4).map(|_| Polling::open(&family, "m").unwrap()).collect();
        assert!(Polling::open(&family, "m").is_none(), "four polls at most");
        drop(polls);
    }
}
