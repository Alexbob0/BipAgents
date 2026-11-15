//! The family rendezvous (2026-11-15): two households' bridges that can't reach each other (each at home behind its
//! own box) link and talk through the relay, both connecting out. The model is the bridge's own family_relay.rs (a
//! home pulling what a reachable place keeps for it), held here instead, for any two bridges.
//!
//! - A bridge takes a **mailbox**: `POST /v1/family/mailbox` → a random id (16 bytes, base64url) and a token that
//!   holds it, sealed with the relay's keys (`fm1.`, like the push tokens: nothing stored, any copy opens it). Only
//!   the token reads the mailbox. Renewed with the old token, same id.
//! - **Only the households it linked with may call it**: at the link, each bridge asks the relay for a **pass** to its
//!   own mailbox (`POST /v1/family/pass`, `fp1.`, sealed too: the mailbox it opens, an id of its own, an end) and gives
//!   it to the other one, sealed in their exchange. A call (`POST /v1/family/call/<id>`, `x-bip-pass`) without a pass
//!   for that very mailbox is refused exactly as a mailbox whose bridge is off is (503 `away`): a stranger who learnt
//!   the id learns nothing, not even whether the home's computer is on, and can't use up what the household may
//!   receive (quotas per pass, not per mailbox).
//! - An **invitation** has a mailbox of its own (`{"invite_until": …}`), ending with it: its id is what the QR code
//!   carries, never the household's. Calls to it need no pass (`x-bip-kind: invite`, 64 KiB at most), only while its
//!   bridge polls it: whoever holds the invitation may introduce itself, nothing more.
//! - The bridge keeps one long poll per mailbox it needs (`GET /v1/family/next`, [`FamilyConfig::poll`]); a call waits
//!   here until the poll takes it ([`FamilyConfig::pickup`]); the caller then gets the answer as the other bridge
//!   streams it (`POST /v1/family/answer/<call>`).
//!
//! What passes is sealed end to end by the two bridges (bridge-rs family_seal.rs). What the relay sees: the bridges'
//! addresses, which mailbox calls which, when, how many bytes, and, while a bridge polls, that it is on and from where.
//! What it keeps: nothing on disk; quotas in memory on hashes, forgotten within the hour; no log line for a call that
//! went through, only for some refusals (with a keyed fingerprint of one mailbox, [`Rendezvous::fingerprint`]) and
//! one count an hour.
//!
//! Memory: a call's bytes are reserved before they are read ([`Budget`]: every call together, and each caller,
//! callee and address its share), against its declared length, and read within [`FamilyConfig::body_timeout`]; a
//! refused call's body is never kept (read and dropped while it keeps coming). One copy of the relay holds the rendezvous (it is in
//! memory): run one, or share mailboxes out by their id.

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
pub const PASS_PREFIX: &str = "fp1.";
const PASS_AAD: &[u8] = b"bipagents-family-pass-v1";
/// How long a mailbox token, and a pass, lasts; the bridges renew theirs well before (same mailbox, same pass id).
pub const MAILBOX_LIFETIME_SECS: u64 = 400 * 86_400;
/// An ended token or pass may still be renewed for this long (a bridge switched off for months keeps its links).
const RENEW_GRACE_SECS: u64 = 400 * 86_400;
/// An invitation's mailbox lasts at most this long (an invitation sent through the account waits a week).
const INVITE_MAX_SECS: u64 = 8 * 86_400;
/// An invitation's exchange is small (the bridge's introduction).
const INVITE_REQUEST_BYTES: usize = 64 * 1024;

/// The family relay's settings (config.rs: `RELAY_FAMILY…`).
#[derive(Clone, Debug)]
pub struct FamilyConfig {
    pub enabled: bool,
    /// A call's request, sealed (`RELAY_FAMILY_REQUEST_MB`, 16).
    pub request_bytes: usize,
    /// Its answer (`RELAY_FAMILY_ANSWER_MB`, 64).
    pub answer_bytes: usize,
    /// Calls an hour, per mailbox as caller, and per pass (one household calling another)
    /// (`RELAY_FAMILY_CALLS_PER_HOUR`, 3,000).
    pub calls_per_hour: u32,
    /// Bytes an hour, in KiB: per mailbox as sender, and per pass as what the called household takes
    /// (`RELAY_FAMILY_MB_PER_HOUR`, 2,048 MiB).
    pub kib_per_hour: u32,
    /// Mailboxes and passes made or renewed an hour, per address (`RELAY_FAMILY_MAILBOXES_PER_IP_HOUR`, 20).
    pub mailboxes_per_ip_hour: u32,
    /// Calls waiting for one mailbox.
    pub queue: usize,
    /// Bytes of all the calls held (being read or waiting to be taken), every mailbox together.
    pub queued_bytes: usize,
    /// What one caller, one callee or one address may hold of it (at most an eighth of the whole).
    pub queued_per_party: usize,
    /// How long a call's body may take to come in.
    pub body_timeout: Duration,
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
            queued_per_party: 32 << 20,
            body_timeout: Duration::from_secs(30),
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
    /// `i`: an invitation's mailbox (it is polled and answers; it never calls nor gives passes).
    #[serde(rename = "k", default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

impl Claims {
    fn invitation(&self) -> bool {
        self.kind.as_deref() == Some("i")
    }
}

/// What a pass holds: the mailbox it lets call, its own id (its quotas), its end.
#[derive(Serialize, Deserialize)]
struct PassClaims {
    #[serde(rename = "t")]
    to: String,
    #[serde(rename = "i")]
    id: String,
    #[serde(rename = "x")]
    exp: u64,
}

/// A call waiting for its mailbox's poll.
struct Call {
    id: String,
    from: String,
    kind: &'static str,
    body: Bytes,
    /// Its bytes, reserved until the poll takes it or its caller goes.
    _held: Held,
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

/// The bytes of the calls held, reserved before they are read: every call together, and each caller, callee and
/// address its share.
pub struct Budget {
    all: usize,
    party: usize,
    held: Mutex<(usize, HashMap<String, usize>)>,
}

/// Bytes reserved, given back when dropped.
struct Held {
    budget: Arc<Budget>,
    parties: [String; 3],
    n: usize,
}

impl Budget {
    fn new(config: &FamilyConfig) -> Self {
        let party = config.queued_per_party.min(config.queued_bytes / 8);
        Self { all: config.queued_bytes, party, held: Mutex::new((0, HashMap::new())) }
    }

    fn reserve(self: &Arc<Self>, parties: [String; 3], n: usize) -> Option<Held> {
        let mut held = lock(&self.held);
        if held.0 + n > self.all || parties.iter().any(|p| held.1.get(p).copied().unwrap_or(0) + n > self.party) {
            return None;
        }
        held.0 += n;
        for p in &parties {
            *held.1.entry(p.clone()).or_insert(0) += n;
        }
        Some(Held { budget: self.clone(), parties, n })
    }

    /// Bytes held now, every call together.
    pub fn held(&self) -> usize {
        lock(&self.held).0
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let mut held = lock(&self.budget.held);
        held.0 = held.0.saturating_sub(self.n);
        for p in &self.parties {
            if let Some(n) = held.1.get_mut(p) {
                *n = n.saturating_sub(self.n);
                if *n == 0 {
                    held.1.remove(p);
                }
            }
        }
    }
}

/// The rendezvous, in memory.
pub struct Rendezvous {
    pub config: FamilyConfig,
    queues: Mutex<HashMap<String, VecDeque<Call>>>,
    bells: Mutex<HashMap<String, Arc<Notify>>>,
    /// Calls taken, waiting for their answer: call id → (the mailbox that took it, where the answer goes).
    waiting: Mutex<HashMap<String, (String, oneshot::Sender<Body>)>>,
    /// Each mailbox's polls: how many are open, when one last began or ended, whether it is an invitation's.
    seen: Mutex<HashMap<String, (usize, Instant, bool)>>,
    budget: Arc<Budget>,
    calls_out: Quota,
    calls_in: Quota,
    kib_out: Quota,
    kib_in: Quota,
    registrations: Quota,
    counts: Counts,
    last_report: Mutex<Instant>,
    /// The key of the logs' fingerprints (from the relay's sealing key: the same on every copy, none outside).
    fingerprint_key: [u8; 32],
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 22 base64url characters: 16 bytes.
pub fn valid_mailbox(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// HMAC-SHA256 (RFC 2104), with a key of 32 bytes.
fn hmac(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    let (mut inner, mut outer) = ([0x36u8; 64], [0x5cu8; 64]);
    for (i, k) in key.iter().enumerate() {
        inner[i] ^= k;
        outer[i] ^= k;
    }
    let inside = Sha256::new().chain_update(inner).chain_update(message).finalize();
    Sha256::new().chain_update(outer).chain_update(inside).finalize().into()
}

/// The fingerprint key of a relay set up with these sealing keys (from the first one).
pub fn fingerprint_key(seal_keys: &str) -> [u8; 32] {
    let first = seal_keys.split(',').map(str::trim).find(|k| !k.is_empty()).unwrap_or_default();
    Sha256::new().chain_update(b"bipagents-family-fingerprint-v1").chain_update(first.as_bytes()).finalize().into()
}

/// A mailbox's (or a pass's) 8-hex fingerprint under that key: what the logs show and `RELAY_BLOCKED` may name.
pub fn fingerprint_with(key: &[u8; 32], id: &str) -> String {
    hmac(key, id.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn kib(bytes: usize) -> u32 {
    u32::try_from(bytes.div_ceil(1024)).unwrap_or(u32::MAX)
}

impl Rendezvous {
    pub fn new(config: FamilyConfig, fingerprint_key: [u8; 32]) -> Self {
        Self {
            calls_out: Quota::new(config.calls_per_hour),
            calls_in: Quota::new(config.calls_per_hour),
            kib_out: Quota::new(config.kib_per_hour),
            kib_in: Quota::new(config.kib_per_hour),
            registrations: Quota::new(config.mailboxes_per_ip_hour),
            budget: Arc::new(Budget::new(&config)),
            config,
            queues: Mutex::default(),
            bells: Mutex::default(),
            waiting: Mutex::default(),
            seen: Mutex::default(),
            counts: Counts::default(),
            last_report: Mutex::new(Instant::now()),
            fingerprint_key,
        }
    }

    /// The keyed fingerprint of a mailbox or pass id, for the logs (not computable from the id alone).
    pub fn fingerprint(&self, id: &str) -> String {
        fingerprint_with(&self.fingerprint_key, id)
    }

    fn bell(&self, mailbox: &str) -> Arc<Notify> {
        lock(&self.bells).entry(mailbox.to_string()).or_default().clone()
    }

    /// How long a call may wait for this mailbox's bridge: `pickup` while a poll is open, what is left of `away`
    /// just after one ended, None when it does not poll (the call fails at once).
    fn pickup(&self, mailbox: &str) -> Option<Duration> {
        let seen = lock(&self.seen);
        let (open, last, _) = seen.get(mailbox)?;
        if *open > 0 {
            return Some(self.config.pickup);
        }
        self.config.away.checked_sub(last.elapsed()).filter(|left| !left.is_zero())
    }

    /// Whether this mailbox is an invitation's, polled now (or a moment ago).
    fn invitation_polled(&self, mailbox: &str) -> bool {
        lock(&self.seen).get(mailbox).is_some_and(|(_, _, invitation)| *invitation) && self.pickup(mailbox).is_some()
    }

    /// Mailboxes with a poll open now.
    pub fn online(&self) -> usize {
        lock(&self.seen).values().filter(|(open, _, _)| *open > 0).count()
    }

    /// Bytes of the calls held now.
    pub fn held(&self) -> usize {
        self.budget.held()
    }

    /// Every minute (main.rs, with the push quotas): the quotas forget what is about to be an hour old, the mailboxes
    /// gone for an hour are forgotten, and once an hour one line of counts.
    pub fn sweep(&self) {
        self.calls_out.sweep();
        self.calls_in.sweep();
        self.kib_out.sweep();
        self.kib_in.sweep();
        self.registrations.sweep();
        let hour = Duration::from_secs(3600);
        let gone: Vec<String> = lock(&self.seen)
            .iter()
            .filter(|(_, (open, last, _))| *open == 0 && last.elapsed() > hour)
            .map(|(id, _)| id.clone())
            .collect();
        {
            let mut seen = lock(&self.seen);
            let mut bells = lock(&self.bells);
            let mut queues = lock(&self.queues);
            for id in &gone {
                seen.remove(id);
                bells.remove(id);
                queues.remove(id);
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
    fn open(rendezvous: &'a Rendezvous, mailbox: &str, invitation: bool) -> Option<Self> {
        let mut seen = lock(&rendezvous.seen);
        let entry = seen.entry(mailbox.to_string()).or_insert((0, Instant::now(), invitation));
        if entry.0 >= rendezvous.config.polls_per_mailbox {
            return None;
        }
        *entry = (entry.0 + 1, Instant::now(), invitation);
        Some(Polling { rendezvous, mailbox: mailbox.to_string() })
    }
}

impl Drop for Polling<'_> {
    fn drop(&mut self) {
        if let Some(entry) = lock(&self.rendezvous.seen).get_mut(&self.mailbox) {
            *entry = (entry.0.saturating_sub(1), Instant::now(), entry.2);
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
            queue.retain(|c| c.id != self.id);
        }
        lock(&self.rendezvous.waiting).remove(&self.id);
    }
}

fn refuse(status: StatusCode, code: &str, detail: &str) -> Response {
    (status, Json(json!({"code": code, "detail": detail}))).into_response()
}

/// What a caller is told when the mailbox does not take its call: its bridge is off, or the caller has no pass for
/// it. The same answer for both, so that it tells a stranger nothing.
fn away() -> Response {
    refuse(StatusCode::SERVICE_UNAVAILABLE, "away", "their bridge is not connected to the relay")
}

fn new_id(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    URL_SAFE_NO_PAD.encode(raw)
}

fn seal_token(relay: &Relay, claims: &Claims) -> String {
    let plain = serde_json::to_vec(claims).expect("claims serialize");
    format!("{MAILBOX_PREFIX}{}", relay.sealer.seal_bytes(&plain, MAILBOX_AAD))
}

fn open(relay: &Relay, token: &str) -> Option<Claims> {
    let sealed = token.strip_prefix(MAILBOX_PREFIX)?;
    let claims: Claims = serde_json::from_slice(&relay.sealer.open_bytes(sealed, MAILBOX_AAD)?).ok()?;
    valid_mailbox(&claims.id).then_some(claims)
}

fn seal_pass(relay: &Relay, pass: &PassClaims) -> String {
    let plain = serde_json::to_vec(pass).expect("a pass serializes");
    format!("{PASS_PREFIX}{}", relay.sealer.seal_bytes(&plain, PASS_AAD))
}

fn open_pass(relay: &Relay, pass: &str) -> Option<PassClaims> {
    let sealed = pass.trim().strip_prefix(PASS_PREFIX)?;
    let claims: PassClaims = serde_json::from_slice(&relay.sealer.open_bytes(sealed, PASS_AAD)?).ok()?;
    valid_mailbox(&claims.to).then_some(claims)
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get("authorization")?.to_str().ok()?;
    Some(value.strip_prefix("Bearer ").or_else(|| value.strip_prefix("bearer "))?.trim())
}

/// The mailbox whose token this request carries (401: none, or ended; 403: blocked). Invalid tokens are not logged.
#[allow(clippy::result_large_err)] // the refusal is the handler's answer as it is, made once per request
fn authenticated(relay: &Relay, headers: &HeaderMap) -> Result<Claims, Response> {
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
    let family = &relay.family;
    if relay.is_blocked_id(&family.fingerprint(&claims.id)) {
        tracing::info!(mailbox = %family.fingerprint(&claims.id), "family relay: blocked mailbox refused");
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "blocked",
            "this mailbox is blocked: write to support@bipagents.com",
        ));
    }
    Ok(claims)
}

/// A refused call's body is read and dropped while it keeps coming: past this long with nothing, its connection goes.
const DRAIN_IDLE: Duration = Duration::from_secs(1);

/// A request's body read and dropped (never kept), as long as it says, within the body time, and while it keeps
/// coming: the caller is refused without its connection cut under it, and a caller that stops sending holds nothing
/// past [`DRAIN_IDLE`] (not even the server's read buffer of its connection).
async fn drain(body: Body, at_most: usize, within: Duration) {
    let mut data = body.into_data_stream();
    let mut read = 0usize;
    let _ = tokio::time::timeout(within, async {
        while let Ok(Some(Ok(chunk))) = tokio::time::timeout(DRAIN_IDLE, data.next()).await {
            read += chunk.len();
            if read > at_most {
                break;
            }
        }
    })
    .await;
}

#[derive(Deserialize, Default)]
struct Register {
    #[serde(default)]
    token: Option<String>,
    /// An invitation's mailbox, until then (Unix seconds).
    #[serde(default)]
    invite_until: Option<u64>,
}

/// `POST /v1/family/mailbox {token?, invite_until?}`: a new mailbox, the same one renewed, or an invitation's.
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
    let claims = match (asked.token.as_deref(), asked.invite_until) {
        (Some(old), None) => match open(&relay, old) {
            Some(claims) if !claims.invitation() && at < claims.exp.saturating_add(RENEW_GRACE_SECS) => {
                Claims { id: claims.id, iat: at, exp: at + MAILBOX_LIFETIME_SECS, kind: None }
            }
            _ => return refuse(StatusCode::UNAUTHORIZED, "unknown", "not a mailbox token of this relay"),
        },
        (None, Some(until)) if until > at && until <= at + INVITE_MAX_SECS => {
            // an invitation's mailbox ends an hour after the invitation (its last answer on the way)
            Claims { id: new_id(16), iat: at, exp: until + 3600, kind: Some("i".into()) }
        }
        (None, None) => Claims { id: new_id(16), iat: at, exp: at + MAILBOX_LIFETIME_SECS, kind: None },
        _ => return refuse(StatusCode::BAD_REQUEST, "invalid", "invalid request"),
    };
    if relay.is_blocked_id(&family.fingerprint(&claims.id)) {
        tracing::info!(mailbox = %family.fingerprint(&claims.id), "family relay: blocked mailbox refused");
        return refuse(StatusCode::FORBIDDEN, "blocked", "this mailbox is blocked: write to support@bipagents.com");
    }
    let token = seal_token(&relay, &claims);
    Json(json!({"mailbox": claims.id, "token": token, "expires": claims.exp})).into_response()
}

#[derive(Deserialize, Default)]
struct PassAsked {
    /// A pass held, renewed (the same mailbox, the same id).
    #[serde(default)]
    renew: Option<String>,
}

/// `POST /v1/family/pass {renew?}` (a mailbox's token): a pass to call this mailbox, for a household it links with;
/// or a pass this bridge holds, renewed.
async fn pass(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let claims = match authenticated(&relay, &headers) {
        Ok(c) if !c.invitation() => c,
        Ok(_) => return refuse(StatusCode::FORBIDDEN, "invalid", "an invitation's mailbox gives no pass"),
        Err(refused) => return refused,
    };
    let family = &relay.family;
    if !family.registrations.allow(relay.client_ip(peer, &headers).as_bytes()) {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::TOO_MANY_REQUESTS, "quota", "too many passes from this address");
    }
    let asked: PassAsked = if body.iter().all(u8::is_ascii_whitespace) {
        PassAsked::default()
    } else {
        match serde_json::from_slice(&body) {
            Ok(asked) => asked,
            Err(_) => return refuse(StatusCode::BAD_REQUEST, "invalid", "invalid request"),
        }
    };
    let at = now();
    let pass = match asked.renew.as_deref() {
        Some(old) => match open_pass(&relay, old) {
            Some(p) if at < p.exp.saturating_add(RENEW_GRACE_SECS) => {
                PassClaims { to: p.to, id: p.id, exp: at + MAILBOX_LIFETIME_SECS }
            }
            _ => return refuse(StatusCode::UNAUTHORIZED, "unknown", "not a pass of this relay"),
        },
        None => PassClaims { to: claims.id, id: new_id(12), exp: at + MAILBOX_LIFETIME_SECS },
    };
    Json(json!({"pass": seal_pass(&relay, &pass), "expires": pass.exp})).into_response()
}

/// `GET /v1/family/next` (the mailbox's token): the next call for it, or 204 after a poll.
async fn next(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    let claims = match authenticated(&relay, &headers) {
        Ok(c) => c,
        Err(refused) => return refused,
    };
    let (mailbox, invitation) = (claims.id.clone(), claims.invitation());
    let family = &relay.family;
    let Some(_open) = Polling::open(family, &mailbox, invitation) else {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "polls", "too many polls open on this mailbox");
    };
    let bell = family.bell(&mailbox);
    let deadline = Instant::now() + family.config.poll;
    loop {
        let found = lock(&family.queues).get_mut(&mailbox).and_then(VecDeque::pop_front);
        if let Some(call) = found {
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

/// A refusal once the caller is known: its body read and dropped first (never kept, [`drain`]), as long as it says
/// no more than a call may weigh (past that, answered at once and its connection closed).
async fn refused_after(response: Response, body: Body, declared: Option<usize>, family: &Rendezvous) -> Response {
    family.counts.refused.fetch_add(1, Ordering::Relaxed);
    if let Some(n) = declared.filter(|n| *n <= family.config.request_bytes) {
        drain(body, n, family.config.body_timeout).await;
    }
    response
}

/// `POST /v1/family/call/<mailbox>` (the caller's own mailbox token; `x-bip-kind: call` with its pass in
/// `x-bip-pass`, or `invite` to an invitation's mailbox): the body kept until that mailbox's bridge takes it, then
/// its answer streamed back.
async fn call(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(target): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Response {
    let family = &relay.family;
    let declared =
        headers.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<usize>().ok());
    let body = request.into_body();
    let from = match authenticated(&relay, &headers) {
        Ok(c) if !c.invitation() => c.id,
        Ok(_) => return refuse(StatusCode::FORBIDDEN, "invalid", "an invitation's mailbox does not call"),
        Err(refused) => return refused,
    };
    if !valid_mailbox(&target) {
        return refused_after(refuse(StatusCode::NOT_FOUND, "invalid", "not a mailbox"), body, declared, family).await;
    }
    let kind = match headers.get("x-bip-kind").and_then(|v| v.to_str().ok()).unwrap_or("call") {
        "call" => "call",
        "invite" => "invite",
        _ => {
            let unknown = refuse(StatusCode::BAD_REQUEST, "invalid", "unknown kind of call");
            return refused_after(unknown, body, declared, family).await;
        }
    };
    // the caller's own allowance (what it sends): nothing here depends on the mailbox called
    if !family.calls_out.allow(from.as_bytes()) {
        tracing::info!(mailbox = %family.fingerprint(&from), "family relay: calls over the quota (as caller)");
        let quota = refuse(StatusCode::TOO_MANY_REQUESTS, "quota", "too many calls this hour");
        return refused_after(quota, body, declared, family).await;
    }
    // Whether this mailbox takes calls from this caller: a pass it gave for itself, or an invitation's mailbox being
    // polled. Anything else reads exactly as a mailbox whose bridge is off.
    let pair = if kind == "call" {
        let pass = headers.get("x-bip-pass").and_then(|v| v.to_str().ok()).and_then(|p| open_pass(&relay, p));
        match pass {
            Some(p) if p.to == target && now() < p.exp && !relay.is_blocked_id(&family.fingerprint(&p.id)) => {
                format!("pass:{}", p.id)
            }
            _ => return refused_after(away(), body, declared, family).await,
        }
    } else {
        if !family.invitation_polled(&target) {
            return refused_after(away(), body, declared, family).await;
        }
        format!("invite:{from}:{target}")
    };
    if target == from || relay.is_blocked_id(&family.fingerprint(&target)) {
        return refused_after(away(), body, declared, family).await;
    }
    // what this pair may exchange: counted for it alone, never shared with the mailbox's other callers
    if !family.calls_in.allow(pair.as_bytes()) {
        let quota = refuse(StatusCode::TOO_MANY_REQUESTS, "quota", "too many calls this hour");
        return refused_after(quota, body, declared, family).await;
    }
    let Some(wait) = family.pickup(&target) else {
        return refused_after(away(), body, declared, family).await;
    };
    let limit = if kind == "invite" { INVITE_REQUEST_BYTES } else { family.config.request_bytes };
    let Some(length) = declared else {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::LENGTH_REQUIRED, "length", "a call says its length");
    };
    if length > limit {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::PAYLOAD_TOO_LARGE, "too_large", "request too large");
    }
    if !family.kib_out.allow_n(from.as_bytes(), kib(length)) || !family.kib_in.allow_n(pair.as_bytes(), kib(length)) {
        tracing::info!(mailbox = %family.fingerprint(&from), "family relay: bytes over the quota");
        let quota = refuse(StatusCode::TOO_MANY_REQUESTS, "quota", "too many bytes this hour");
        return refused_after(quota, body, declared, family).await;
    }
    // its bytes reserved before they are read: every call together, the caller's, the callee's, the address's share
    let parties = [format!("from:{from}"), format!("to:{target}"), format!("ip:{}", relay.client_ip(peer, &headers))];
    let Some(held) = family.budget.reserve(parties, length) else {
        let busy = refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", "the relay is busy: try again in a moment");
        return refused_after(busy, body, declared, family).await;
    };
    let read = tokio::time::timeout(family.config.body_timeout, axum::body::to_bytes(body, length)).await;
    let Ok(Ok(body)) = read else {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::REQUEST_TIMEOUT, "slow", "the request did not come in time");
    };
    if lock(&family.queues).get(&target).is_some_and(|q| q.len() >= family.config.queue) {
        family.counts.refused.fetch_add(1, Ordering::Relaxed);
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "busy", "too many calls waiting for this mailbox");
    }
    family.counts.calls.fetch_add(1, Ordering::Relaxed);
    family.counts.bytes.fetch_add(body.len() as u64, Ordering::Relaxed);
    let id = new_id(12);
    let _kept = Kept { rendezvous: family, mailbox: target.clone(), id: id.clone() };
    let (picked_tx, picked_rx) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();
    let call = Call { id: id.clone(), from, kind, body, _held: held, picked: picked_tx, answer: answer_tx };
    lock(&family.queues).entry(target.clone()).or_default().push_back(call);
    family.bell(&target).notify_one();
    if !matches!(tokio::time::timeout(wait, picked_rx).await, Ok(Ok(()))) {
        return away();
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
        Ok(c) => c.id,
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
        if total > family.config.answer_bytes || !family.kib_out.allow_n(mailbox.as_bytes(), kib(chunk.len())) {
            tracing::info!(mailbox = %family.fingerprint(&mailbox), "family relay: answer cut (size or quota)");
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

/// What the public `/health` says of it: whether it is on, nothing of its activity.
pub fn health(family: &Rendezvous) -> Value {
    json!({"enabled": family.config.enabled})
}

/// The family routes (merged by routes.rs when the family relay is on), with their own size limits.
pub fn routes() -> Router<Arc<Relay>> {
    Router::new()
        .route("/v1/family/mailbox", post(mailbox))
        .route("/v1/family/pass", post(pass))
        .route("/v1/family/next", get(next))
        .route("/v1/family/call/{mailbox}", post(call))
        .route("/v1/family/answer/{call}", post(answer))
        .layer(DefaultBodyLimit::disable())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn mailbox_ids_and_their_keyed_fingerprints() {
        let id = new_id(16);
        assert!(valid_mailbox(&id), "{id}");
        assert!(!valid_mailbox("short") && !valid_mailbox(&format!("{}!", &id[..21])));
        let (a, b) = (fingerprint_key("key-a"), fingerprint_key("key-b,other"));
        assert_eq!(fingerprint_with(&a, &id).len(), 8);
        assert_ne!(fingerprint_with(&a, &id), fingerprint_with(&b, &id), "keyed: not computable from the id alone");
        assert_eq!(fingerprint_key(" key-a , key-c"), a, "the first sealing key's");
        assert_ne!(fingerprint_with(&a, &id), hex(&Sha256::digest(id.as_bytes())[..4]));
        assert_eq!(kib(0), 0);
        assert_eq!(kib(1), 1);
        assert_eq!(kib(2048), 2);
    }

    #[test]
    fn hmac_is_rfc_4231s() {
        // RFC 4231 test case 1: key 20 × 0x0b (padded with zeros to 32 bytes: the same HMAC, keys under the block size
        // are padded with zeros)
        let mut key = [0u8; 32];
        key[..20].copy_from_slice(&[0x0b; 20]);
        assert_eq!(hex(&hmac(&key, b"Hi There")), "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    }

    #[tokio::test]
    async fn a_mailbox_polls_while_a_poll_is_open_and_a_moment_after() {
        let family = Rendezvous::new(FamilyConfig::default(), [0u8; 32]);
        assert!(family.pickup("m").is_none(), "never polled: away");
        let open = Polling::open(&family, "m", false).unwrap();
        assert_eq!(family.pickup("m"), Some(family.config.pickup));
        assert_eq!(family.online(), 1);
        assert!(!family.invitation_polled("m"));
        drop(open);
        assert!(family.pickup("m").is_some_and(|d| d <= family.config.away));
        let polls: Vec<_> = (0..4).map(|_| Polling::open(&family, "m", false).unwrap()).collect();
        assert!(Polling::open(&family, "m", false).is_none(), "four polls at most");
        drop(polls);
        let invite = Polling::open(&family, "i", true).unwrap();
        assert!(family.invitation_polled("i"));
        drop(invite);
    }

    #[test]
    fn bytes_are_reserved_for_everyone_and_for_each_party() {
        let config = FamilyConfig { queued_bytes: 800, queued_per_party: 1000, ..FamilyConfig::default() };
        let budget = Arc::new(Budget::new(&config));
        let parties = |c: &str, t: &str, ip: &str| [c.to_string(), t.to_string(), ip.to_string()];
        // a party holds at most an eighth of the whole (100 here)
        let a = budget.reserve(parties("from:a", "to:b", "ip:1"), 100).unwrap();
        assert!(budget.reserve(parties("from:a", "to:c", "ip:2"), 1).is_none(), "the caller's share is full");
        assert!(budget.reserve(parties("from:x", "to:b", "ip:2"), 1).is_none(), "the callee's share is full");
        assert!(budget.reserve(parties("from:x", "to:y", "ip:1"), 1).is_none(), "the address's share is full");
        let others: Vec<_> = (0..7)
            .map(|i| {
                let p = parties(&format!("from:o{i}"), &format!("to:o{i}"), &format!("ip:o{i}"));
                budget.reserve(p, 100).unwrap()
            })
            .collect();
        assert_eq!(budget.held(), 800);
        assert!(budget.reserve(parties("from:z", "to:z", "ip:z"), 1).is_none(), "the whole is full");
        drop(a);
        assert_eq!(budget.held(), 700);
        assert!(budget.reserve(parties("from:a", "to:b", "ip:1"), 100).is_some());
        drop(others);
    }
}
