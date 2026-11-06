//! The phone exit (docs/phone-exit.md, protocol v1; behind `[phone_exit] enabled`, off by default): an agent's HTTPS
//! connections to a few sites leave through its person's phone instead of this server's datacenter address, after a
//! refusal and a card the person accepted. No Tailscale, no VPN: the phone opens ONE outbound WebSocket to this bridge
//! (`GET /v1/exit/tunnel`), and the TCP streams are multiplexed over it.
//!
//! ```text
//! agent's Chrome ─CONNECT─▶ bipagents-egress ─(unix socket, « EXIT host 443 agent »)─▶ this hub
//!                                                           ─(WebSocket frames)─▶ phone ─TCP─▶ site
//! ```
//!
//! - **TLS stays end to end**: the hub and the phone copy bytes; neither decrypts. The only thing either reads is the
//!   ClientHello's server name, to refuse a stream whose SNI is not the host it asked for (domain fronting).
//! - **Sessions, not sockets** (store.rs): what the person accepted (an agent, its sites); the limits (idle, time,
//!   streams per minute) and the MB follow the session across the phone's reconnections. No data cap (Alex, 6/11).
//! - **Refused here, before the phone is asked** (and again on the phone, which has the last word): an address
//!   literal, a local name, a port other than 443, a name resolving here to a private address, a site that is not in
//!   a live session or « Toujours » for this agent, an agent that is not the person's own. A site sent to the phone
//!   never falls back to the datacenter: no phone, the stream is refused (`paused`) and the agent is told why
//!   (`WHY`, agent.rs), a « Toujours » site gets a card to restart the relay.
//! - One log line per stream (host, agent, bytes, seconds, how it ended), one journal entry and one Boîte line per
//!   session. Never content.

pub mod agent;
pub mod checks;
pub mod store;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::body::Bytes as Body;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, put};
use axum::Router;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::app::AppState;
use crate::config::Config;
use crate::py;
use crate::web::{self, ApiResult, HttpError};

pub use checks::{
    client_hello_sni, frame, name_refused, parse_frame, public_ip, Sni, CHUNK, MAX_PAYLOAD, T_BYE, T_DATA, T_END,
    T_FIN, T_HELLO, T_OPEN, T_OPEN_ERR, T_OPEN_OK, T_PAUSE, T_RESUME, T_RST, T_SESSIONS, T_STATS, T_USAGE, T_WELCOME,
    T_WINDOW, WINDOW,
};
use store::Session;

const HELLO_WAIT: Duration = Duration::from_secs(10);
const OPEN_WAIT: Duration = Duration::from_secs(25);
const HELLO_FIRST: Duration = Duration::from_secs(15);
const RESOLVE_WAIT: Duration = Duration::from_secs(3);
const PING_EVERY: Duration = Duration::from_secs(20);
const USAGE_EVERY: u64 = 5;
const FLUSH_EVERY: u64 = 10;
const CLIENT_HELLO_MAX: usize = 16 * 1024 + 5;
/// A session accepted this recently, or paused this recently, is waited for (`start_wait_seconds`).
const ACCEPTED_GRACE: f64 = 60.0;
const PAUSED_GRACE: Duration = Duration::from_secs(30);
/// The agent's notes (`WHY`) are kept this long.
const NOTE_KEEP: Duration = Duration::from_secs(120);

/// `[phone_exit]` of bridge.toml (none of it is a setting of the person: theirs are in store.rs).
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// SHA-256 (hex) of a debug token accepted instead of the bridge key (tests, debug builds). Never the token.
    pub token_sha256: String,
    /// Per session: the internal time limit (generous), minutes without any stream (0: none). Fractions are fine.
    pub max_minutes: f64,
    pub idle_minutes: f64,
    /// New streams per minute per tunnel (human pace).
    pub connections_per_minute: u64,
    /// The hub resolves each name too and refuses one that points at a private address here (default). Off: only the
    /// phone judges (a debug switch, to test the phone's own refusals end to end).
    pub hub_resolve: bool,
    /// A stream for a session the phone is about to carry (just accepted, or reconnecting after a network change)
    /// waits this long for it instead of being refused at once (seconds).
    pub start_wait_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            token_sha256: String::new(),
            max_minutes: 120.0,
            idle_minutes: 10.0,
            connections_per_minute: 120,
            hub_resolve: true,
            start_wait_seconds: 15.0,
        }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let mut out = Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(false),
            token_sha256: table.get("token_sha256").and_then(Value::as_str).unwrap_or("").trim().to_lowercase(),
            ..Settings::default()
        };
        let real =
            |key: &str, default: f64| table.get(key).and_then(Value::as_f64).filter(|v| *v >= 0.0).unwrap_or(default);
        out.max_minutes = real("max_minutes", out.max_minutes);
        out.idle_minutes = real("idle_minutes", out.idle_minutes);
        out.connections_per_minute =
            table.get("connections_per_minute").and_then(Value::as_u64).unwrap_or(out.connections_per_minute);
        out.hub_resolve = table.get("hub_resolve").and_then(Value::as_bool).unwrap_or(true);
        out.start_wait_seconds = real("start_wait_seconds", out.start_wait_seconds);
        out
    }
}

/// Where the agents' proxy reaches the hub (mounted into `bipagents-egress` as `/bip/exit`).
pub fn socket_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("phone-exit")
}

pub const SOCKET: &str = "hub.sock";

pub fn socket_path(config: &Config) -> PathBuf {
    socket_dir(config).join(SOCKET)
}

/// The `podman run` arguments the agents' proxy gets when the phone exit is on (sandbox.rs `proxy_command`); the
/// sites themselves come with its policy file (sandbox_sites.rs `policy_of`, `phone`).
pub fn proxy_args(config: &Config) -> Vec<String> {
    if !config.phone_exit.enabled {
        return Vec::new();
    }
    vec![
        "-v".into(),
        format!("{}:/bip/exit:z", socket_dir(config).display()),
        "-e".into(),
        format!("BIP_EXIT_SOCK=/bip/exit/{SOCKET}"),
    ]
}

/// The agent's data is forgotten with it (sandbox.rs, an agent removed).
pub fn forget(config: &Config, agent: &str) {
    end_sessions_where(config, "off", |s| s.agent == agent);
    let _ = store::change(config, |s| {
        s.cards.retain(|c| c.agent != agent);
        s.journal.retain(|j| j.agent != agent);
    });
}

// --- the hub ---------------------------------------------------------------------------------------------------------

enum Incoming {
    Data(Vec<u8>),
    Fin,
    Rst,
}

struct Slot {
    opened: Option<oneshot::Sender<Result<(), String>>>,
    incoming: mpsc::UnboundedSender<Incoming>,
    credit: Arc<Semaphore>,
    session: String,
    agent: String,
    host: String,
}

/// A session's counters while the bridge runs (persisted into the store every few seconds and when it ends).
struct Live {
    up: AtomicU64,
    down: AtomicU64,
    streams: AtomicU64,
    open: AtomicU64,
    hosts: Mutex<HashMap<String, u64>>,
    last_active: Mutex<Instant>,
    paused_since: Mutex<Option<Instant>>,
    dirty: AtomicBool,
}

impl Live {
    fn of(s: &Session) -> Live {
        Live {
            up: AtomicU64::new(s.up),
            down: AtomicU64::new(s.down),
            streams: AtomicU64::new(s.streams),
            open: AtomicU64::new(0),
            hosts: Mutex::new(s.hosts.iter().map(|(k, v)| (k.clone(), *v)).collect()),
            last_active: Mutex::new(Instant::now()),
            paused_since: Mutex::new(None),
            dirty: AtomicBool::new(false),
        }
    }

    fn touch(&self) {
        *self.last_active.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

    fn carried(&self, host: &str, up: u64, down: u64) {
        self.up.fetch_add(up, Ordering::Relaxed);
        self.down.fetch_add(down, Ordering::Relaxed);
        *self.hosts.lock().unwrap_or_else(|e| e.into_inner()).entry(host.to_string()).or_default() += up + down;
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn used(&self) -> u64 {
        self.up.load(Ordering::Relaxed) + self.down.load(Ordering::Relaxed)
    }

    fn paused(&self, yes: bool) {
        let mut p = self.paused_since.lock().unwrap_or_else(|e| e.into_inner());
        *p = if yes { Some(p.unwrap_or_else(Instant::now)) } else { None };
    }

    fn copy_into(&self, s: &mut Session) {
        s.up = self.up.load(Ordering::Relaxed);
        s.down = self.down.load(Ordering::Relaxed);
        s.streams = self.streams.load(Ordering::Relaxed);
        s.hosts = self.hosts.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.clone(), *v)).collect();
    }
}

/// One phone's WebSocket: its streams and the sessions it carries.
pub struct Tunnel {
    id: u64,
    device: String,
    network: Mutex<String>,
    out: mpsc::Sender<Vec<u8>>,
    streams: Mutex<HashMap<u32, Slot>>,
    next_stream: AtomicU32,
    started: Instant,
    up: AtomicU64,
    down: AtomicU64,
    opened: AtomicU64,
    minute: Mutex<(Instant, u64)>,
    stop: CancellationToken,
    why: Mutex<Option<String>>,
    carried: Mutex<Vec<String>>,
    /// The phone's `PAUSE` (`wifi_only`, `user`, `network`), until its `RESUME`.
    paused: Mutex<Option<String>>,
}

impl Tunnel {
    fn end(&self, why: &str) {
        let mut w = self.why.lock().unwrap_or_else(|e| e.into_inner());
        if w.is_none() {
            *w = Some(why.to_string());
        }
        drop(w);
        self.stop.cancel();
    }

    async fn send(&self, kind: u8, stream: u32, payload: &[u8]) -> bool {
        tokio::select! {
            sent = self.out.send(frame(kind, stream, payload)) => sent.is_ok(),
            _ = self.stop.cancelled() => false,
        }
    }

    /// A control frame from anywhere (a blocking thread too): queued, or sent by a task when the queue is full.
    fn control(&self, kind: u8, payload: Value) {
        let f = frame(kind, 0, payload.to_string().as_bytes());
        if let Err(mpsc::error::TrySendError::Full(f)) = self.out.try_send(f) {
            let out = self.out.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = out.send(f).await;
                });
            }
        }
    }

    fn carries(&self, id: &str) -> bool {
        self.carried.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|c| c == id)
    }

    fn carried(&self) -> Vec<String> {
        self.carried.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn pause_reason(&self) -> Option<String> {
        self.paused.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn slot_count(&self) -> usize {
        self.streams.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// The session's streams cut (it ended).
    fn cut(&self, session: &str) {
        let mut streams = self.streams.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<u32> = streams.iter().filter(|(_, s)| s.session == session).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(slot) = streams.remove(&id) {
                let _ = slot.incoming.send(Incoming::Rst);
            }
        }
    }
}

/// A refusal told to the agent: its site, the code, when.
type Note = (String, String, Instant);

#[derive(Default)]
pub struct Hub {
    current: Mutex<Option<Arc<Tunnel>>>,
    tunnels: AtomicU64,
    live: Mutex<HashMap<String, Arc<Live>>>,
    /// Per agent: (site, refusal code, when) — what the browser tool tells it (`WHY`).
    notes: Mutex<HashMap<String, Vec<Note>>>,
}

pub fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(Hub::default)
}

impl Hub {
    fn tunnel(&self) -> Option<Arc<Tunnel>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| !t.stop.is_cancelled())
    }

    /// The tunnel carrying this session now.
    fn carrier(&self, id: &str) -> Option<Arc<Tunnel>> {
        self.tunnel().filter(|t| t.carries(id))
    }

    fn live_of(&self, s: &Session) -> Arc<Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).entry(s.id.clone()).or_insert_with(|| Arc::new(Live::of(s))).clone()
    }

    fn live(&self, id: &str) -> Option<Arc<Live>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()
    }

    fn note(&self, agent: &str, host: &str, code: &str) {
        let site = crate::mail_tidy::registrable(host);
        let mut notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        let list = notes.entry(agent.to_string()).or_default();
        list.retain(|(s, _, at)| *s != site && at.elapsed() < NOTE_KEEP);
        list.push((site, code.to_string(), Instant::now()));
    }

    /// The last refusal for this agent's site (code, seconds ago).
    pub fn why(&self, agent: &str, host: &str) -> Option<(String, u64)> {
        let site = crate::mail_tidy::registrable(&checks::host_of(host));
        let notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        notes
            .get(agent)?
            .iter()
            .rev()
            .find(|(s, _, at)| *s == site && at.elapsed() < NOTE_KEEP)
            .map(|(_, code, at)| (code.clone(), at.elapsed().as_secs()))
    }

    pub fn status(&self) -> Value {
        match self.tunnel() {
            Some(t) => json!({
                "connected": true, "session": t.id, "device": t.device,
                "network": t.network.lock().unwrap_or_else(|e| e.into_inner()).clone(),
                "seconds": t.started.elapsed().as_secs(), "up": t.up.load(Ordering::Relaxed),
                "down": t.down.load(Ordering::Relaxed), "streams": t.slot_count(),
                "opened": t.opened.load(Ordering::Relaxed), "sessions": t.carried(), "paused": t.pause_reason(),
            }),
            None => json!({"connected": false}),
        }
    }
}

/// A session as the apps see it, its counters live.
fn session_json(s: &Session) -> Value {
    let mut s = s.clone();
    if let Some(live) = hub().live(&s.id) {
        live.copy_into(&mut s);
    }
    s.json()
}

/// The live counters into the store (`only`: that session; None: every dirty one).
fn flush(config: &Config, only: Option<&str>) {
    let lives: Vec<(String, Arc<Live>)> = hub()
        .live
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|(id, l)| only.map_or(l.dirty.load(Ordering::Relaxed), |o| o == id.as_str()))
        .map(|(id, l)| (id.clone(), l.clone()))
        .collect();
    if lives.is_empty() {
        return;
    }
    let _ = store::change(config, |s| {
        for session in s.sessions.iter_mut() {
            if let Some((_, live)) = lives.iter().find(|(id, _)| *id == session.id) {
                live.dirty.store(false, Ordering::Relaxed);
                live.copy_into(session);
            }
        }
    });
}

/// Sessions' states (`active`/`paused`) as the tunnel says.
fn set_states(config: &Config, ids: &[String], state: &str, tunnel: Option<&Tunnel>) {
    if ids.is_empty() {
        return;
    }
    for id in ids {
        if let Some(live) = hub().live(id) {
            live.paused(state == "paused");
            if state == "active" {
                live.touch();
            }
        }
    }
    let network = tunnel.map(|t| t.network.lock().unwrap_or_else(|e| e.into_inner()).clone());
    let _ = store::change(config, |s| {
        for session in s.sessions.iter_mut().filter(|x| ids.contains(&x.id)) {
            session.state = state.into();
            if let (Some(t), Some(network)) = (tunnel, network.as_ref()) {
                session.ever_active = true;
                session.network = network.clone();
                session.device = t.device.clone();
            }
        }
    });
}

/// A session over: out of the live ones into the journal, its streams cut, the phone told (`END`); a tunnel left
/// with nothing to carry says `BYE no_session`.
pub fn end_session(config: &Config, id: &str, why: &str) -> Option<Session> {
    let live = hub().live.lock().unwrap_or_else(|e| e.into_inner()).remove(id);
    let ended = store::change(config, |s| {
        if let (Some(live), Some(session)) = (live.as_ref(), s.sessions.iter_mut().find(|x| x.id == id)) {
            live.copy_into(session);
        }
        store::end_in(s, id, why)
    })
    .ok()
    .flatten()?;
    if let Some(t) = hub().tunnel() {
        let left = {
            let mut carried = t.carried.lock().unwrap_or_else(|e| e.into_inner());
            let had = carried.len();
            carried.retain(|c| c != id);
            (had != carried.len()).then_some(carried.len())
        };
        if let Some(left) = left {
            t.cut(id);
            t.control(T_END, json!({"id": id, "why": why}));
            if left == 0 {
                t.end("no_session");
            }
        }
    }
    tracing::info!(target: "bipbridge::phone_exit", session = %ended.id, agent = %ended.agent,
        sites = %ended.sites.join(","), up = ended.up, down = ended.down, streams = ended.streams, ended = %why,
        "phone exit session ended");
    Some(ended)
}

fn end_sessions_where(config: &Config, why: &str, f: impl Fn(&Session) -> bool) -> usize {
    let ids: Vec<String> = store::load(config).sessions.iter().filter(|s| f(s)).map(|s| s.id.clone()).collect();
    ids.iter().filter(|id| end_session(config, id, why).is_some()).count()
}

/// The agent's sessions for this site ended (`ask_phone_exit` with `done`).
pub fn end_sessions_of(config: &Config, agent: &str, site: &str, why: &str) -> usize {
    end_sessions_where(config, why, |s| s.agent == agent && s.sites.iter().any(|x| x == site))
}

fn refused_line(host: &str, agent: &str, reason: &str) {
    tracing::info!(target: "bipbridge::phone_exit", host = %host, agent = %agent, reason = %reason, "phone exit refused");
}

/// Which of these sessions a phone may carry (and why not the others).
fn admit(config: &Config, ids: &[String]) -> (Vec<Session>, Vec<Value>) {
    let store = store::load(config);
    let (mut carry, mut refused) = (Vec::new(), Vec::new());
    for id in ids.iter().take(32) {
        match store.sessions.iter().find(|s| s.id == *id) {
            _ if !store.settings.allowed => refused.push(json!({"id": id, "why": "off"})),
            Some(s) if store::owned(config, &s.agent) => carry.push(s.clone()),
            Some(_) => refused.push(json!({"id": id, "why": "not_allowed"})),
            None => {
                let why = match store.journal.iter().rev().find(|s| s.id == *id) {
                    Some(s) => format!("ended:{}", s.ended.as_deref().unwrap_or("")),
                    None => "unknown".into(),
                };
                refused.push(json!({"id": id, "why": why}));
            }
        }
    }
    (carry, refused)
}

fn welcome_payload(carry: &[Session], refused: &[Value]) -> Vec<u8> {
    let sessions: Vec<Value> = carry.iter().map(session_json).collect();
    json!({"v": checks::VERSION, "sessions": sessions, "refused": refused}).to_string().into_bytes()
}

fn welcome(carry: &[Session], refused: &[Value]) -> Vec<u8> {
    frame(T_WELCOME, 0, &welcome_payload(carry, refused))
}

fn ids_in(payload: &Value) -> Vec<String> {
    payload
        .get("sessions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|s| s.chars().take(64).collect()).collect())
        .unwrap_or_default()
}

/// The tunnel's frames from the phone, until it ends.
async fn read_phone(state: AppState, tunnel: Arc<Tunnel>, mut stream: futures::stream::SplitStream<WebSocket>) {
    loop {
        let message = tokio::select! {
            m = stream.next() => m,
            _ = tunnel.stop.cancelled() => return,
        };
        let data = match message {
            Some(Ok(Message::Binary(b))) => b,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
            Some(Ok(Message::Text(_))) => {
                tunnel.end("protocol");
                return;
            }
            Some(Ok(Message::Close(_))) | None => {
                tunnel.end("phone-closed");
                return;
            }
            Some(Err(_)) => {
                tunnel.end("phone-lost");
                return;
            }
        };
        let Some((kind, id, payload)) = parse_frame(&data) else {
            tunnel.end("protocol");
            return;
        };
        let info = || serde_json::from_slice::<Value>(payload).unwrap_or(Value::Null);
        match kind {
            T_OPEN_OK | T_OPEN_ERR => {
                let mut streams = tunnel.streams.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(opened) = streams.get_mut(&id).and_then(|slot| slot.opened.take()) {
                    let answer =
                        if kind == T_OPEN_OK { Ok(()) } else { Err(checks::code_of(&String::from_utf8_lossy(payload))) };
                    let _ = opened.send(answer);
                }
            }
            T_DATA => {
                tunnel.down.fetch_add(payload.len() as u64, Ordering::Relaxed);
                let streams = tunnel.streams.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = streams.get(&id) {
                    let _ = slot.incoming.send(Incoming::Data(payload.to_vec()));
                }
            }
            T_WINDOW if payload.len() == 4 => {
                let more = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                let streams = tunnel.streams.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = streams.get(&id) {
                    // never past what a peer could have been owed
                    if slot.credit.available_permits() + more <= 4 * WINDOW as usize {
                        slot.credit.add_permits(more);
                    }
                }
            }
            T_FIN | T_RST => {
                let streams = tunnel.streams.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = streams.get(&id) {
                    let _ = slot.incoming.send(if kind == T_FIN { Incoming::Fin } else { Incoming::Rst });
                }
            }
            T_STATS => {
                if let Some(network) = info().get("network").and_then(Value::as_str) {
                    *tunnel.network.lock().unwrap_or_else(|e| e.into_inner()) = network.chars().take(16).collect();
                }
            }
            T_PAUSE | T_RESUME => {
                let why = info().get("why").and_then(Value::as_str).map(checks::code_of).unwrap_or_else(|| "user".into());
                *tunnel.paused.lock().unwrap_or_else(|e| e.into_inner()) = (kind == T_PAUSE).then(|| why.clone());
                let state_name = if kind == T_PAUSE { "paused" } else { "active" };
                set_states(&state.config(), &tunnel.carried(), state_name, Some(&tunnel));
                tracing::info!(target: "bipbridge::phone_exit", tunnel = tunnel.id, why = %why, state = state_name,
                    "phone exit: the phone paused or resumed");
            }
            T_SESSIONS => {
                let config = state.config();
                let (mut carry, refused) = admit(&config, &ids_in(&info()));
                let now: Vec<String> = carry.iter().map(|s| s.id.clone()).collect();
                let before = std::mem::replace(&mut *tunnel.carried.lock().unwrap_or_else(|e| e.into_inner()), now.clone());
                let dropped: Vec<String> = before.into_iter().filter(|b| !now.contains(b)).collect();
                for s in &carry {
                    hub().live_of(s);
                }
                set_states(&config, &dropped, "paused", None);
                let resumed = if tunnel.pause_reason().is_some() { "paused" } else { "active" };
                set_states(&config, &now, resumed, Some(&tunnel));
                for s in carry.iter_mut() {
                    s.state = resumed.into();
                }
                if !tunnel.send(T_WELCOME, 0, &welcome_payload(&carry, &refused)).await {
                    return;
                }
                if now.is_empty() {
                    tunnel.end("no_session");
                    return;
                }
            }
            T_END => {
                let id = info().get("id").and_then(Value::as_str).unwrap_or("").to_string();
                if tunnel.carries(&id) {
                    end_session(&state.config(), &id, "user_stop");
                }
            }
            T_BYE => {
                let why = checks::code_of(&String::from_utf8_lossy(payload));
                if why == "user_stop" {
                    let config = state.config();
                    for id in tunnel.carried() {
                        end_session(&config, &id, "user_stop");
                    }
                }
                tunnel.end(&format!("phone-bye:{why}"));
                return;
            }
            _ => {}
        }
    }
}

/// `GET /v1/exit/tunnel`: the phone's WebSocket (the bridge key, or the debug token).
async fn tunnel_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let config = state.config();
    let settings = config.phone_exit.clone();
    if !settings.enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let presented = crate::auth::bearer_of(&headers);
    let by_token = !settings.token_sha256.is_empty()
        && presented.as_deref().is_some_and(|p| {
            let seen = hex::encode(Sha256::digest(p.as_bytes()));
            crate::auth::key_matches(&settings.token_sha256, Some(&seen))
        });
    if !(by_token || crate::auth::key_matches(&config.bridge_key, presented.as_deref())) {
        tracing::info!(target: "bipbridge::phone_exit", "phone exit: tunnel refused (credentials)");
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(ws) = ws else { return StatusCode::BAD_REQUEST.into_response() };
    ws.max_message_size(MAX_PAYLOAD + 64)
        .max_frame_size(MAX_PAYLOAD + 64)
        .on_upgrade(move |socket| run_tunnel(state, socket))
}

async fn run_tunnel(state: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    // HELLO first: who the phone is, its network, the sessions it carries
    let hello = match tokio::time::timeout(HELLO_WAIT, stream.next()).await {
        Ok(Some(Ok(Message::Binary(b)))) => b,
        _ => return,
    };
    let Some((T_HELLO, _, payload)) = parse_frame(&hello) else { return };
    let info: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("").chars().take(64).collect::<String>();
    let config = state.config();
    let (mut carry, refused) = admit(&config, &ids_in(&info));
    if carry.is_empty() {
        tracing::info!(target: "bipbridge::phone_exit", device = %text("device"), refused = %json!(refused),
            "phone exit: tunnel with no session");
        let _ = sink.send(Message::Binary(Bytes::from(welcome(&carry, &refused)))).await;
        let _ = sink.send(Message::Binary(Bytes::from(frame(T_BYE, 0, b"no_session")))).await;
        let _ = sink.close().await;
        return;
    }
    let the_hub = hub();
    let (out, mut frames) = mpsc::channel::<Vec<u8>>(256);
    let tunnel = Arc::new(Tunnel {
        id: the_hub.tunnels.fetch_add(1, Ordering::Relaxed) + 1,
        device: text("device"),
        network: Mutex::new(text("network")),
        out,
        streams: Mutex::new(HashMap::new()),
        next_stream: AtomicU32::new(1),
        started: Instant::now(),
        up: AtomicU64::new(0),
        down: AtomicU64::new(0),
        opened: AtomicU64::new(0),
        minute: Mutex::new((Instant::now(), 0)),
        stop: CancellationToken::new(),
        why: Mutex::new(None),
        carried: Mutex::new(carry.iter().map(|s| s.id.clone()).collect()),
        paused: Mutex::new(None),
    });
    // one tunnel at a time: a new one (the phone reconnecting after a network change) replaces the one before
    if let Some(old) = the_hub.current.lock().unwrap_or_else(|e| e.into_inner()).replace(tunnel.clone()) {
        old.end("replaced");
    }
    for s in &carry {
        the_hub.live_of(s);
    }
    set_states(&config, &tunnel.carried(), "active", Some(&tunnel));
    for s in carry.iter_mut() {
        s.state = "active".into();
    }
    tracing::info!(target: "bipbridge::phone_exit", tunnel = tunnel.id, device = %tunnel.device,
        network = %text("network"), version = %text("app"), sessions = %tunnel.carried().join(","),
        "phone exit: tunnel open");
    if sink.send(Message::Binary(Bytes::from(welcome(&carry, &refused)))).await.is_err() {
        tunnel.end("phone-lost");
    }
    let writer = {
        let tunnel = tunnel.clone();
        async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.tick().await;
            let mut usage = tokio::time::interval(Duration::from_secs(USAGE_EVERY));
            usage.tick().await;
            loop {
                tokio::select! {
                    f = frames.recv() => {
                        let Some(f) = f else { break };
                        if sink.send(Message::Binary(Bytes::from(f))).await.is_err() {
                            tunnel.end("phone-lost");
                            break;
                        }
                    }
                    _ = ping.tick() => {
                        if sink.send(Message::Ping(Bytes::from_static(b"p"))).await.is_err() {
                            tunnel.end("phone-lost");
                            break;
                        }
                    }
                    _ = usage.tick() => {
                        let sessions: Vec<Value> = tunnel.carried().iter().filter_map(|id| hub().live(id).map(|l| {
                            let state = if tunnel.pause_reason().is_some() { "paused" } else { "active" };
                            json!({"id": id, "used_bytes": l.used(), "state": state})
                        })).collect();
                        let f = frame(T_USAGE, 0, json!({"sessions": sessions}).to_string().as_bytes());
                        if sink.send(Message::Binary(Bytes::from(f))).await.is_err() {
                            tunnel.end("phone-lost");
                            break;
                        }
                    }
                    _ = tunnel.stop.cancelled() => break,
                }
            }
            // what was queued before the end (an END) still goes, then the reason
            while let Ok(f) = frames.try_recv() {
                let _ = sink.send(Message::Binary(Bytes::from(f))).await;
            }
            let why = tunnel.why.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
            if !why.starts_with("phone") {
                let _ = sink.send(Message::Binary(Bytes::from(frame(T_BYE, 0, why.as_bytes())))).await;
            }
            let _ = sink.close().await;
        }
    };
    tokio::join!(read_phone(state.clone(), tunnel.clone(), stream), writer);
    let why = tunnel.why.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    // every stream still open ends with it; the agent told its pages were cut by a change of network
    for (_, slot) in tunnel.streams.lock().unwrap_or_else(|e| e.into_inner()).drain() {
        let _ = slot.incoming.send(Incoming::Rst);
        if why == "replaced" || why.starts_with("phone-lost") || why == "phone-closed" {
            the_hub.note(&slot.agent, &slot.host, "network_changed");
        }
    }
    {
        let mut current = the_hub.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|t| t.id == tunnel.id) {
            *current = None;
        }
    }
    // its sessions wait for the phone to come back (unless the new tunnel carries them)
    let config = state.config();
    let carrier = the_hub.tunnel();
    let waiting: Vec<String> =
        tunnel.carried().into_iter().filter(|id| !carrier.as_ref().is_some_and(|t| t.carries(id))).collect();
    flush(&config, None);
    let live_ids: Vec<String> = store::load(&config).sessions.iter().map(|s| s.id.clone()).collect();
    let waiting: Vec<String> = waiting.into_iter().filter(|id| live_ids.contains(id)).collect();
    set_states(&config, &waiting, "paused", None);
    tracing::info!(target: "bipbridge::phone_exit", tunnel = tunnel.id, device = %tunnel.device,
        seconds = tunnel.started.elapsed().as_secs(), up = tunnel.up.load(Ordering::Relaxed),
        down = tunnel.down.load(Ordering::Relaxed), streams = tunnel.opened.load(Ordering::Relaxed), ended = %why,
        "phone exit: tunnel closed");
}

// --- the agents' proxy's side (and the browser tool's) ------------------------------------------------------------------

/// The hub's socket for the agents' proxy and the browser tool, and the sessions' watch (started with the bridge).
pub async fn serve(state: AppState) {
    let config = state.config();
    if !config.phone_exit.enabled {
        return;
    }
    let dir = socket_dir(&config);
    if let Err(exc) = std::fs::create_dir_all(&dir) {
        tracing::warn!(target: "bipbridge::phone_exit", error = %exc, "phone exit: no socket folder");
        return;
    }
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(SOCKET);
    let _ = std::fs::remove_file(&path);
    let listener = match tokio::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(exc) => {
            tracing::warn!(target: "bipbridge::phone_exit", error = %exc, "phone exit: socket not bound");
            return;
        }
    };
    // the proxy's file with the routes as they are now (a session that outlived a restart)
    let _ = crate::sandbox_sites::ensure_policy(&config);
    tracing::info!(target: "bipbridge::phone_exit", "phone exit: on");
    tokio::spawn(watch(state.clone()));
    loop {
        let Ok((conn, _)) = listener.accept().await else { continue };
        let state = state.clone();
        tokio::spawn(async move { local_stream(state, conn).await });
    }
}

/// Every second: the sessions' limits (time, idle), the person's « Autoriser » and the feature switched off, the
/// cards past their time; every few seconds the counters saved; each ended session's line in the Boîte.
async fn watch(state: AppState) {
    let mut tick: u64 = 0;
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        tick += 1;
        let config = state.config();
        let s = config.phone_exit.clone();
        let store = store::load(&config);
        let now = py::time_time();
        if !s.enabled || !store.settings.allowed {
            for session in &store.sessions {
                end_session(&config, &session.id, "off");
            }
        } else {
            for session in &store.sessions {
                let live = hub().live_of(session);
                let idle = live.last_active.lock().unwrap_or_else(|e| e.into_inner()).elapsed();
                if session.max_minutes > 0.0 && now - session.accepted_at >= session.max_minutes * 60.0 {
                    end_session(&config, &session.id, "expired");
                } else if s.idle_minutes > 0.0
                    && live.open.load(Ordering::Relaxed) == 0
                    && idle.as_secs_f64() >= s.idle_minutes * 60.0
                {
                    end_session(&config, &session.id, "idle");
                }
            }
        }
        if store.cards.iter().any(|c| now - c.at >= store::CARD_TTL) {
            let _ = store::change(&config, |s| s.cards.retain(|c| now - c.at < store::CARD_TTL));
        }
        if tick.is_multiple_of(FLUSH_EVERY) {
            flush(&config, None);
        }
        tell_boite(&state).await;
    }
}

/// Each ended session's line in the Boîte, once (silent: it is a record, not news).
async fn tell_boite(state: &AppState) {
    let config = state.config();
    let untold: Vec<Session> = store::load(&config).journal.into_iter().filter(|s| !s.told).collect();
    if untold.is_empty() {
        return;
    }
    for s in &untold {
        let Some(agent) = config.agent(Some(&s.agent)) else { continue };
        let lang = store::lang_of(&config, &s.agent);
        let line = store::journal_line(lang, &store::shown_name(&config, &s.agent), s);
        let message = crate::outbox::BoiteMessage {
            id: Some(format!("phone-exit:{}", s.id)),
            title: Some(store::journal_title(lang).to_string()),
            ..crate::outbox::BoiteMessage::new(line)
        };
        if let Err(e) = state.outbox.ingest(agent, message, false).await {
            tracing::warn!(target: "bipbridge::phone_exit", error = %format!("{e:#}"), "phone exit: Boîte line not stored");
        }
    }
    let ids: Vec<String> = untold.iter().map(|s| s.id.clone()).collect();
    let _ = store::change(&config, |st| {
        for j in st.journal.iter_mut().filter(|j| ids.contains(&j.id)) {
            j.told = true;
        }
    });
}

/// The refusal before the phone is asked for a name and port (None: go on to the sessions).
pub fn precheck(settings: &Settings, host: &str, port: u16) -> Option<&'static str> {
    if !settings.enabled {
        return Some("off");
    }
    if let Some(why) = name_refused(host) {
        return Some(why);
    }
    if port != 443 {
        return Some("port");
    }
    None
}

/// The session this agent's stream to `host` goes through, and the tunnel carrying it; else the refusal code. A
/// session the phone is about to carry (just accepted, or the phone reconnecting) is waited for a little. A
/// « Toujours » site with no session gets a card (restart the relay).
async fn decide(state: &AppState, host: &str, agent: &str) -> Result<(String, Arc<Tunnel>), String> {
    let config = state.config();
    if !store::owned(&config, agent) {
        return Err("not_allowed".into());
    }
    let deadline = Instant::now() + Duration::from_secs_f64(config.phone_exit.start_wait_seconds);
    loop {
        let store = store::load(&config);
        if !store.settings.allowed {
            return Err("off".into());
        }
        if let Some(s) = store.session_for(agent, host) {
            if let Some(t) = hub().carrier(&s.id) {
                return match t.pause_reason() {
                    Some(why) if why == "wifi_only" => Err(why),
                    Some(_) => Err("paused".into()),
                    None => Ok((s.id.clone(), t)),
                };
            }
            let live = hub().live_of(s);
            let paused_recently =
                live.paused_since.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|p| p.elapsed() < PAUSED_GRACE);
            let starting = s.state == "accepted" && py::time_time() - s.accepted_at < ACCEPTED_GRACE;
            if (starting || paused_recently) && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            return Err("paused".into());
        }
        if let Some(ended) = store.recent_end(agent, host, py::time_time()) {
            return Err(match ended.ended.as_deref() {
                Some("expired") => "expired",
                Some("off") => "off",
                _ => "paused",
            }
            .into());
        }
        if store.always_site(host).is_some() {
            let (c, a, h) = (config.clone(), agent.to_string(), host.to_string());
            let asked = tokio::task::spawn_blocking(move || agent::ask(&c, &a, &h, "relay stopped")).await;
            if let Ok(Ok(agent::Asked::Card(card, true))) = asked {
                agent::notify(&state.push, &config, &card).await;
            }
            return Err("paused".into());
        }
        return Err("not_allowed".into());
    }
}

async fn local_stream(state: AppState, conn: tokio::net::UnixStream) {
    let (read, mut write) = conn.into_split();
    let mut read = BufReader::with_capacity(CHUNK, read);
    let mut line = String::new();
    let got = tokio::time::timeout(Duration::from_secs(10), (&mut read).take(512).read_line(&mut line)).await;
    if !matches!(got, Ok(Ok(n)) if n > 0) {
        return;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    match parts.as_slice() {
        ["EXIT", host, port, agent] => {
            let host = host.to_lowercase().trim_end_matches('.').to_string();
            let port = port.parse::<u16>().unwrap_or(0);
            exit_stream(state, read, write, host, port, agent.to_string()).await;
        }
        // the browser tool: why the phone exit refused this agent's site lately
        ["WHY", agent, host] => {
            let answer = match hub().why(agent, host) {
                Some((code, ago)) => format!("{code} {ago}\n"),
                None => "NONE\n".into(),
            };
            let _ = write.write_all(answer.as_bytes()).await;
        }
        // the browser tool: the agent's page is a challenge for servers; its person asked (a card) if possible
        ["ASK", agent, host, reason @ ..] => {
            let (config, a, h, r) = (state.config(), agent.to_string(), host.to_string(), reason.join(" "));
            let asked = tokio::task::spawn_blocking(move || agent::ask(&config, &a, &h, &r)).await;
            let answer = match asked {
                Ok(Ok(agent::Asked::Card(card, made))) => {
                    if made {
                        agent::notify(&state.push, &state.config(), &card).await;
                        tracing::info!(target: "bipbridge::phone_exit", agent = %card.agent, site = %card.site,
                            reason = %card.reason, "phone exit card made (a challenge page)");
                        "CARD\n".to_string()
                    } else {
                        "WAITING\n".to_string()
                    }
                }
                Ok(Ok(agent::Asked::Live(_))) => "LIVE\n".into(),
                Ok(Ok(agent::Asked::No(code))) => format!("NO {code}\n"),
                _ => "NO error\n".into(),
            };
            let _ = write.write_all(answer.as_bytes()).await;
        }
        _ => {}
    }
}

async fn exit_stream(
    state: AppState,
    mut read: BufReader<tokio::net::unix::OwnedReadHalf>,
    mut write: tokio::net::unix::OwnedWriteHalf,
    host: String,
    port: u16,
    agent: String,
) {
    let settings = state.config().phone_exit.clone();
    let refuse = |reason: &str| format!("ERR {reason}\n");
    let refused = |code: &str| {
        refused_line(&host, &agent, code);
        hub().note(&agent, &host, code);
    };
    if let Some(why) = precheck(&settings, &host, port) {
        refused(why);
        let _ = write.write_all(refuse(why).as_bytes()).await;
        return;
    }
    let (session, tunnel) = match decide(&state, &host, &agent).await {
        Ok(found) => found,
        Err(why) => {
            // never the datacenter instead: the agent is told why, its person asked when it makes sense
            refused(&why);
            let _ = write.write_all(refuse(&why).as_bytes()).await;
            return;
        }
    };
    // The phone resolves the name and checks what it found (its network is where its LAN is). This side refuses a
    // name that points here at a private address; a name it cannot resolve is the phone's to judge.
    if settings.hub_resolve {
        if let Ok(Ok(found)) = tokio::time::timeout(RESOLVE_WAIT, tokio::net::lookup_host((host.as_str(), port))).await
        {
            if found.into_iter().any(|a| !public_ip(a.ip())) {
                refused("private");
                let _ = write.write_all(refuse("private").as_bytes()).await;
                return;
            }
        }
    }
    let too_many = {
        let mut minute = tunnel.minute.lock().unwrap_or_else(|e| e.into_inner());
        if minute.0.elapsed() >= Duration::from_secs(60) {
            *minute = (Instant::now(), 0);
        }
        minute.1 += 1;
        settings.connections_per_minute > 0 && minute.1 > settings.connections_per_minute
    };
    if too_many {
        refused("rate");
        let _ = write.write_all(refuse("rate").as_bytes()).await;
        return;
    }
    let Some(live) = hub().live(&session) else {
        refused("paused");
        let _ = write.write_all(refuse("paused").as_bytes()).await;
        return;
    };
    let id = tunnel.next_stream.fetch_add(1, Ordering::Relaxed);
    let (opened_tx, opened_rx) = oneshot::channel();
    let (incoming_tx, mut incoming) = mpsc::unbounded_channel();
    let credit = Arc::new(Semaphore::new(WINDOW as usize));
    tunnel.streams.lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Slot {
            opened: Some(opened_tx),
            incoming: incoming_tx,
            credit: credit.clone(),
            session: session.clone(),
            agent: agent.clone(),
            host: host.clone(),
        },
    );
    let forget = |t: &Tunnel| {
        t.streams.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    };
    let started = Instant::now();
    let open = json!({"host": host, "port": port, "agent": agent, "session": session}).to_string();
    live.touch();
    live.open.fetch_add(1, Ordering::Relaxed);
    struct Open(Arc<Live>);
    impl Drop for Open {
        fn drop(&mut self) {
            self.0.open.fetch_sub(1, Ordering::Relaxed);
            self.0.touch();
        }
    }
    let _open = Open(live.clone());
    if !tunnel.send(T_OPEN, id, open.as_bytes()).await {
        forget(&tunnel);
        refused("paused");
        let _ = write.write_all(refuse("paused").as_bytes()).await;
        return;
    }
    let answer = tokio::select! {
        a = tokio::time::timeout(OPEN_WAIT, opened_rx) => match a {
            Ok(Ok(a)) => a,
            Ok(Err(_)) => Err("paused".to_string()),
            Err(_) => Err("timeout".to_string()),
        },
        _ = tunnel.stop.cancelled() => Err("paused".to_string()),
    };
    if let Err(why) = answer {
        forget(&tunnel);
        if why == "timeout" {
            let _ = tunnel.send(T_RST, id, b"").await;
        }
        refused_line(&host, &agent, &format!("phone:{why}"));
        hub().note(&agent, &host, &why);
        let _ = write.write_all(refuse(&why).as_bytes()).await;
        return;
    }
    let open_ms = started.elapsed().as_millis();
    tunnel.opened.fetch_add(1, Ordering::Relaxed);
    live.streams.fetch_add(1, Ordering::Relaxed);
    if write.write_all(b"OK\n").await.is_err() {
        forget(&tunnel);
        let _ = tunnel.send(T_RST, id, b"").await;
        return;
    }
    // The ClientHello: its server name must be the host asked for (else a CONNECT to an allowed site could carry
    // another site's name on the same CDN).
    let mut first = Vec::with_capacity(2048);
    let hello = async {
        loop {
            match client_hello_sni(&first) {
                Sni::NeedMore if first.len() < CLIENT_HELLO_MAX => {}
                other => return other,
            }
            let mut buf = vec![0u8; CHUNK];
            match read.read(&mut buf).await {
                Ok(0) | Err(_) if first.is_empty() => return Sni::Closed,
                Ok(0) | Err(_) => return Sni::NotTls,
                Ok(n) => first.extend_from_slice(&buf[..n]),
            }
        }
    };
    let sni = tokio::time::timeout(HELLO_FIRST, hello).await.unwrap_or(Sni::NotTls);
    let fine = matches!(&sni, Sni::Name(n) if *n == host);
    if !fine {
        forget(&tunnel);
        let _ = tunnel.send(T_RST, id, b"").await;
        if sni == Sni::Closed {
            // Chrome's preconnects: opened, never used. Not a refusal.
            tracing::debug!(target: "bipbridge::phone_exit", host = %host, agent = %agent, "phone exit stream closed before hello");
            tracing::info!(target: "bipbridge::phone_exit", session = %session, stream = id, host = %host,
                agent = %agent, up = 0u64, down = 0u64, open_ms = open_ms as u64, seconds = started.elapsed().as_secs(),
                ended = "closed_before_hello", "phone exit stream");
            return;
        }
        let why = match sni {
            Sni::Name(_) => "sni_mismatch",
            Sni::NoName => "sni_missing",
            _ => "not_tls",
        };
        refused(why);
        return;
    }
    let up = Arc::new(AtomicU64::new(0));
    let down = Arc::new(AtomicU64::new(0));
    // client → phone, within the phone's window
    let upward = {
        let tunnel = tunnel.clone();
        let credit = credit.clone();
        let up = up.clone();
        let live = live.clone();
        let host = host.clone();
        async move {
            let mut pending = first;
            loop {
                let mut at = 0;
                while at < pending.len() {
                    let take = (pending.len() - at).min(CHUNK);
                    let permit = tokio::select! {
                        p = credit.acquire_many(take as u32) => p,
                        _ = tunnel.stop.cancelled() => return "tunnel",
                    };
                    match permit {
                        Ok(p) => p.forget(),
                        Err(_) => return "tunnel",
                    }
                    if !tunnel.send(T_DATA, id, &pending[at..at + take]).await {
                        return "tunnel";
                    }
                    tunnel.up.fetch_add(take as u64, Ordering::Relaxed);
                    up.fetch_add(take as u64, Ordering::Relaxed);
                    live.carried(&host, take as u64, 0);
                    at += take;
                }
                live.touch();
                let mut buf = vec![0u8; CHUNK];
                let n = tokio::select! {
                    r = read.read(&mut buf) => r,
                    _ = tunnel.stop.cancelled() => return "tunnel",
                };
                match n {
                    Ok(0) => {
                        let _ = tunnel.send(T_FIN, id, b"").await;
                        return "fin";
                    }
                    Ok(n) => {
                        buf.truncate(n);
                        pending = buf;
                    }
                    Err(_) => {
                        let _ = tunnel.send(T_RST, id, b"").await;
                        return "client-reset";
                    }
                }
            }
        }
    };
    // phone → client, its window given back as the client takes the bytes
    let downward = {
        let tunnel = tunnel.clone();
        let down = down.clone();
        let live = live.clone();
        let host = host.clone();
        async move {
            let mut owed: usize = 0;
            loop {
                let item = tokio::select! {
                    i = incoming.recv() => i,
                    _ = tunnel.stop.cancelled() => return "tunnel",
                };
                match item {
                    Some(Incoming::Data(data)) => {
                        if write.write_all(&data).await.is_err() {
                            let _ = tunnel.send(T_RST, id, b"").await;
                            return "client-reset";
                        }
                        down.fetch_add(data.len() as u64, Ordering::Relaxed);
                        live.carried(&host, 0, data.len() as u64);
                        live.touch();
                        owed += data.len();
                        if owed >= (WINDOW / 4) as usize {
                            let _ = tunnel.send(T_WINDOW, id, &(owed as u32).to_be_bytes()).await;
                            owed = 0;
                        }
                    }
                    Some(Incoming::Fin) => {
                        let _ = write.shutdown().await;
                        return "fin";
                    }
                    Some(Incoming::Rst) | None => return "site-reset",
                }
            }
        }
    };
    // a stream ends when both halves did, or one was reset (the other is then dropped)
    let (a, b) = {
        tokio::pin!(upward);
        tokio::pin!(downward);
        let mut up_end = None;
        let mut down_end = None;
        while up_end.is_none() || down_end.is_none() {
            tokio::select! {
                e = &mut upward, if up_end.is_none() => {
                    up_end = Some(e);
                    if e != "fin" { break; }
                }
                e = &mut downward, if down_end.is_none() => {
                    down_end = Some(e);
                    if e != "fin" { break; }
                }
            }
        }
        (up_end.unwrap_or("-"), down_end.unwrap_or("-"))
    };
    forget(&tunnel);
    if a != "fin" || b != "fin" {
        let _ = tunnel.send(T_RST, id, b"").await;
    }
    tracing::info!(target: "bipbridge::phone_exit", session = %session, stream = id, host = %host, agent = %agent,
        up = up.load(Ordering::Relaxed), down = down.load(Ordering::Relaxed), open_ms = open_ms as u64,
        seconds = started.elapsed().as_secs(), ended = %format!("{a}/{b}"), "phone exit stream");
}

// --- the app's routes (the bridge key) -----------------------------------------------------------------------------

fn overview(config: &Config) -> Value {
    let store = store::load(config);
    let sites: Vec<Value> = store
        .sites
        .iter()
        .map(|(site, s)| json!({"site": site, "mode": s.mode, "minimal": store.minimal(site), "at": s.at}))
        .collect();
    json!({
        "enabled": config.phone_exit.enabled,
        "settings": {"allowed": store.settings.allowed, "wifi_only": store.settings.wifi_only,
                     "minimal_route": store.settings.minimal_route},
        "sites": sites,
        "sessions": store.sessions.iter().map(session_json).collect::<Vec<_>>(),
        "tunnel": hub().status(),
    })
}

fn on(config: &Config) -> ApiResult<()> {
    if config.phone_exit.enabled {
        Ok(())
    } else {
        Err(HttpError::new(404, "the phone exit is off on this place"))
    }
}

fn body_of(body: &Body) -> ApiResult<Value> {
    if body.len() > 16 * 1024 {
        return Err(HttpError::new(413, "too big"));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

fn io(e: impl std::fmt::Display) -> HttpError {
    HttpError::new(500, e.to_string())
}

async fn overview_route(State(state): State<AppState>) -> Response {
    web::ok(overview(&state.config()))
}

/// `PUT /v1/exit/settings`: « Autoriser », « Seulement en Wi-Fi », the minimal route.
async fn settings_route(State(state): State<AppState>, body: Body) -> ApiResult<Response> {
    let config = state.config();
    on(&config)?;
    let body = body_of(&body)?;
    let flag = |k: &str| -> ApiResult<Option<bool>> {
        match body.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(HttpError::new(422, format!("{k}: true or false"))),
        }
    };
    let (allowed, wifi_only, minimal) = (flag("allowed")?, flag("wifi_only")?, flag("minimal_route")?);
    store::change(&config, |s| {
        if let Some(v) = allowed {
            s.settings.allowed = v;
        }
        if let Some(v) = wifi_only {
            s.settings.wifi_only = v;
            for session in s.sessions.iter_mut() {
                session.wifi_only = v;
            }
        }
        if let Some(v) = minimal {
            s.settings.minimal_route = v;
        }
    })
    .map_err(io)?;
    if allowed == Some(false) {
        end_sessions_where(&config, "off", |_| true);
    }
    Ok(web::ok(overview(&config)))
}

/// `PUT /v1/exit/sites/{site}`: Toujours / Jamais / Demander, and its minimal route.
async fn site_route(State(state): State<AppState>, UrlPath(raw): UrlPath<String>, body: Body) -> ApiResult<Response> {
    let config = state.config();
    on(&config)?;
    let site = checks::site_of(&raw).map_err(|why| HttpError::new(422, format!("not a site ({why})")))?;
    let body = body_of(&body)?;
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or("ask").to_string();
    if !["always", "never", "ask"].contains(&mode.as_str()) {
        return Err(HttpError::new(422, "mode: always, never or ask"));
    }
    let minimal = match body.get("minimal") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(HttpError::new(422, "minimal: true or false")),
    };
    store::change(&config, |s| {
        let kept = s.sites.get(&site).and_then(|x| x.minimal);
        s.sites.insert(site.clone(), store::Site { mode: mode.clone(), minimal: minimal.or(kept), at: py::time_time() });
        if mode == "never" {
            s.cards.retain(|c| c.site != site);
        }
    })
    .map_err(io)?;
    if mode == "never" {
        end_sessions_where(&config, "user_stop", |s| s.sites.contains(&site));
    }
    Ok(web::ok(overview(&config)))
}

async fn site_delete_route(State(state): State<AppState>, UrlPath(raw): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    on(&config)?;
    let site = checks::site_of(&raw).map_err(|why| HttpError::new(422, format!("not a site ({why})")))?;
    store::change(&config, |s| s.sites.remove(&site)).map_err(io)?;
    Ok(web::ok(overview(&config)))
}

async fn sessions_route(State(state): State<AppState>) -> Response {
    let store = store::load(&state.config());
    web::ok(json!({"sessions": store.sessions.iter().map(session_json).collect::<Vec<_>>()}))
}

/// `POST /v1/exit/sessions`: the person starts one from the app (`{"agent", "sites"}`).
async fn session_start_route(State(state): State<AppState>, body: Body) -> ApiResult<Response> {
    let config = state.config();
    on(&config)?;
    let body = body_of(&body)?;
    let agent = body.get("agent").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    if config.agent(Some(&agent)).is_none() {
        return Err(HttpError::new(404, "no such agent"));
    }
    if !store::owned(&config, &agent) {
        return Err(HttpError::new(403, "only for an agent of your own (never a shared or family agent)"));
    }
    let mut sites = Vec::new();
    for raw in body.get("sites").and_then(Value::as_array).into_iter().flatten().take(10) {
        let site = checks::site_of(raw.as_str().unwrap_or(""))
            .map_err(|why| HttpError::new(422, format!("not a site ({why})")))?;
        if !sites.contains(&site) {
            sites.push(site);
        }
    }
    if sites.is_empty() {
        return Err(HttpError::new(422, "sites: at least one"));
    }
    let session = store::change(&config, |s| {
        if !s.settings.allowed {
            return None;
        }
        let session = store::new_session(&config, s, &agent, sites.clone(), "app");
        s.sessions.push(session.clone());
        Some(session)
    })
    .map_err(io)?
    .ok_or_else(|| HttpError::new(409, "switched off in Settings"))?;
    tracing::info!(target: "bipbridge::phone_exit", agent = %agent, sites = %sites.join(","), session = %session.id,
        "phone exit session started from the app");
    Ok(web::ok(json!({"session": session_json(&session), "tunnel": "/v1/exit/tunnel"})))
}

async fn session_stop_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> Response {
    web::ok(json!({"stopped": end_session(&state.config(), &id, "user_stop").is_some()}))
}

async fn status_route(State(state): State<AppState>) -> Response {
    let config = state.config();
    let mut out = hub().status();
    out["enabled"] = json!(config.phone_exit.enabled);
    web::ok(out)
}

/// `DELETE /v1/exit/session`: the kill switch. Every session ends, the tunnel is closed.
async fn stop_route(State(state): State<AppState>) -> Response {
    let config = state.config();
    let ended = end_sessions_where(&config, "user_stop", |_| true);
    let tunnel = hub().tunnel();
    if let Some(t) = &tunnel {
        t.end("user_stop");
    }
    web::ok(json!({"stopped": ended > 0 || tunnel.is_some(), "sessions": ended}))
}

async fn journal_route(State(state): State<AppState>, Query(q): Query<HashMap<String, String>>) -> Response {
    let limit = q.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(50).clamp(1, store::JOURNAL_MAX);
    let store = store::load(&state.config());
    let sessions: Vec<Value> = store.journal.iter().rev().take(limit).map(Session::json).collect();
    web::ok(json!({"sessions": sessions}))
}

/// Under /v1 with the bridge key: the person's settings, sites, sessions, journal, and the kill switch.
pub fn admin_router() -> Router<AppState> {
    Router::new()
        .route("/exit", get(overview_route))
        .route("/exit/settings", put(settings_route))
        .route("/exit/sites/{site}", put(site_route).delete(site_delete_route))
        .route("/exit/sessions", get(sessions_route).post(session_start_route))
        .route("/exit/sessions/{id}", delete(session_stop_route))
        .route("/exit/session", get(status_route).delete(stop_route))
        .route("/exit/journal", get(journal_route))
}

/// Under /v1 without the bridge key: the phone's tunnel (the bridge key or the debug token, checked there).
pub fn open_router() -> Router<AppState> {
    Router::new().route("/exit/tunnel", get(tunnel_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_from_the_table() {
        let s = Settings::from_table(&json!({"enabled": true, "max_minutes": 5, "idle_minutes": 0.5}));
        assert!(s.enabled && s.max_minutes == 5.0 && s.idle_minutes == 0.5 && s.connections_per_minute == 120);
        assert_eq!(Settings::from_table(&json!({})), Settings::default());
        assert!(!Settings::default().enabled && Settings::default().max_minutes == 120.0);
    }

    #[test]
    fn precheck_refuses_before_the_phone() {
        let s = Settings { enabled: true, ..Settings::default() };
        assert_eq!(precheck(&s, "10.0.0.1", 443), Some("literal_ip"));
        assert_eq!(precheck(&s, "localhost", 443), Some("local_name"));
        assert_eq!(precheck(&s, "a.example.com", 80), Some("port"));
        assert_eq!(precheck(&s, "a.example.com", 443), None);
        assert_eq!(precheck(&Settings::default(), "a.example.com", 443), Some("off"));
    }
}
