//! The phone exit, phase 0 (a prototype behind `[phone_exit] enabled`, off by default; design: the study
//! « Le téléphone comme sortie réseau de l'agent », §3.1 and §5): an agent's HTTPS connections to a few listed sites
//! leave through its person's phone instead of this server's datacenter address. No Tailscale, no VPN: the phone opens
//! ONE outbound WebSocket to this bridge (`GET /v1/exit/tunnel`), and the TCP streams are multiplexed over it.
//!
//! ```text
//! agent's Chrome ─CONNECT─▶ bipagents-egress ─(unix socket, « EXIT host 443 agent »)─▶ this hub
//!                                                           ─(WebSocket frames)─▶ phone ─TCP─▶ site
//! ```
//!
//! - **TLS stays end to end**: the hub and the phone copy bytes; neither decrypts. The only thing either reads is the
//!   ClientHello's server name, to refuse a stream whose SNI is not the host it asked for (domain fronting).
//! - **Refused here, before the phone is asked** (and again on the phone, which has the last word, and resolves the
//!   name itself so the LAN check happens where the LAN is): a host not on the list, an address literal, a name of a
//!   local network (`.local`, no dot…), a port other than 443, a name that resolves here to a private, loopback,
//!   link-local, shared (100.64/10), NAT64 (64:ff9b::/96, 64:ff9b:1::/48), 6to4 (2002::/16) or IPv4-mapped private
//!   address. A site sent to the phone never falls back to the datacenter: no phone, the stream is refused (`paused`).
//! - **Per session** (one WebSocket): a cap on bytes and on time, an idle timeout (no stream for 10 min: `BYE idle`),
//!   new streams per minute. One line in the log per stream: host, agent, bytes, seconds, how it ended. Never content.
//!
//! Frames (binary WebSocket messages): `type u8 | stream u32 BE | length u32 BE | payload`. See `T_*`.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::app::AppState;
use crate::config::Config;

pub const T_HELLO: u8 = 1;
pub const T_OPEN: u8 = 2;
pub const T_OPEN_OK: u8 = 3;
pub const T_OPEN_ERR: u8 = 4;
pub const T_DATA: u8 = 5;
pub const T_WINDOW: u8 = 6;
pub const T_FIN: u8 = 7;
pub const T_RST: u8 = 8;
pub const T_STATS: u8 = 11;
pub const T_BYE: u8 = 12;

/// Each side may have this many bytes of a stream in flight before the other's `WINDOW` (a big download over 4G
/// would otherwise fill this bridge's memory).
pub const WINDOW: u32 = 256 * 1024;
/// A `DATA` frame's payload at most.
pub const CHUNK: usize = 16 * 1024;
/// A frame's payload at most (anything bigger ends the tunnel).
pub const MAX_PAYLOAD: usize = 64 * 1024;
const HELLO_WAIT: Duration = Duration::from_secs(10);
const OPEN_WAIT: Duration = Duration::from_secs(25);
const HELLO_FIRST: Duration = Duration::from_secs(15);
const RESOLVE_WAIT: Duration = Duration::from_secs(3);
const PING_EVERY: Duration = Duration::from_secs(20);
const CLIENT_HELLO_MAX: usize = 16 * 1024 + 5;

/// `[phone_exit]` of bridge.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    /// The sites that leave through the phone: `doctolib.fr` (that name only) or `*.doctolib.fr` (its subdomains).
    pub hosts: Vec<String>,
    /// The agents whose browsing may use it (empty: every agent of this place).
    pub agents: Vec<String>,
    /// SHA-256 (hex) of the prototype's debug token, which the demo phone presents (`Authorization: Bearer`). The
    /// bridge key works too. Never the token itself in bridge.toml.
    pub token_sha256: String,
    /// Per session (one WebSocket): MB carried both ways, minutes, minutes without any stream (0: none). Fractions
    /// are fine (the tests use seconds).
    pub max_mb: f64,
    pub max_minutes: f64,
    pub idle_minutes: f64,
    pub connections_per_minute: u64,
    /// The hub resolves each name too and refuses one that points at a private address here (default). Off: only the
    /// phone judges (a debug switch, to test the phone's own refusals end to end).
    pub hub_resolve: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            hosts: Vec::new(),
            agents: Vec::new(),
            token_sha256: String::new(),
            max_mb: 200.0,
            max_minutes: 60.0,
            idle_minutes: 10.0,
            connections_per_minute: 120,
            hub_resolve: true,
        }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let mut out = Settings::default();
        let list = |key: &str| -> Vec<String> {
            table
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        };
        let number = |key: &str, default: u64| table.get(key).and_then(Value::as_u64).unwrap_or(default);
        out.enabled = table.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        out.hosts = list("hosts").into_iter().map(|h| h.trim_end_matches('.').to_string()).collect();
        out.agents = table
            .get("agents")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        out.token_sha256 = table.get("token_sha256").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
        let real =
            |key: &str, default: f64| table.get(key).and_then(Value::as_f64).filter(|v| *v >= 0.0).unwrap_or(default);
        out.max_mb = real("max_mb", out.max_mb);
        out.max_minutes = real("max_minutes", out.max_minutes);
        out.idle_minutes = real("idle_minutes", out.idle_minutes);
        out.connections_per_minute = number("connections_per_minute", out.connections_per_minute);
        out.hub_resolve = table.get("hub_resolve").and_then(Value::as_bool).unwrap_or(true);
        out
    }

    /// The agents' proxy chains the phone for this agent and host.
    pub fn routes(&self, agent: &str, host: &str) -> bool {
        self.enabled
            && (self.agents.is_empty() || self.agents.iter().any(|a| a == agent))
            && host_listed(&self.hosts, host)
    }
}

/// Where the agents' proxy reaches the hub (mounted into `bipagents-egress` as `/bip/exit`).
pub fn socket_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("phone-exit")
}

pub const SOCKET: &str = "hub.sock";

/// The `podman run` arguments the agents' proxy gets when the phone exit is on (sandbox.rs `proxy_command`).
pub fn proxy_args(config: &Config) -> Vec<String> {
    let s = &config.phone_exit;
    if !s.enabled || s.hosts.is_empty() {
        return Vec::new();
    }
    vec![
        "-v".into(),
        format!("{}:/bip/exit:z", socket_dir(config).display()),
        "-e".into(),
        format!("BIP_EXIT_SOCK=/bip/exit/{SOCKET}"),
        "-e".into(),
        format!("BIP_EXIT_HOSTS={}", s.hosts.join(",")),
    ]
}

// --- the checks ------------------------------------------------------------------------------------------------------

pub fn host_listed(patterns: &[String], host: &str) -> bool {
    let host = host.trim().trim_end_matches('.').to_lowercase();
    patterns.iter().any(|p| match p.strip_prefix("*.") {
        Some(base) => host.len() > base.len() + 1 && host.ends_with(&format!(".{base}")),
        None => *p == host,
    })
}

const LOCAL_SUFFIXES: [&str; 9] =
    ["localhost", "local", "internal", "lan", "home", "arpa", "test", "invalid", "localdomain"];

/// Why a requested name is refused before anything (None: a public-looking name).
pub fn name_refused(host: &str) -> Option<&'static str> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<IpAddr>().is_ok() || looks_numeric(host) {
        return Some("literal-ip");
    }
    if host.len() > 253
        || host.is_empty()
        || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        || host.starts_with('.')
        || host.contains("..")
    {
        return Some("bad-name");
    }
    let last = host.rsplit('.').next().unwrap_or("");
    if !host.contains('.')
        || LOCAL_SUFFIXES.contains(&last)
        || !(last.bytes().all(|b| b.is_ascii_alphabetic()) || last.starts_with("xn--"))
    {
        return Some("local-name");
    }
    None
}

/// `10.1`, `0x7f.1`, `2130706433`: forms some resolvers take as an IPv4 address.
fn looks_numeric(host: &str) -> bool {
    let last = host.rsplit('.').next().unwrap_or("");
    !last.is_empty() && (last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x"))
}

/// An address of the internet: not private, loopback, link-local, shared (100.64/10), multicast, reserved, nor an
/// IPv4 wrapped in IPv6 (NAT64, its local-use prefix, 6to4: refused whole), nor an IPv4-mapped address of these.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let nat64 = s[0] == 0x64 && s[1] == 0xff9b && (s[2..6].iter().all(|&x| x == 0) || s[2] == 1);
            let six_to_four = s[0] == 0x2002;
            let unique_local = (s[0] & 0xfe00) == 0xfc00;
            let link_local = (s[0] & 0xffc0) == 0xfe80;
            let global_unicast = (s[0] & 0xe000) == 0x2000;
            let documentation = s[0] == 0x2001 && s[1] == 0xdb8;
            let teredo_orchid = s[0] == 0x2001 && s[1] < 0x200;
            global_unicast
                && !(nat64 || six_to_four || unique_local || link_local || documentation || teredo_orchid)
                && !v6.is_loopback()
                && !v6.is_unspecified()
        }
    }
}

/// What the first bytes of a stream say about its TLS server name.
#[derive(Debug, PartialEq, Eq)]
pub enum Sni {
    NeedMore,
    Name(String),
    NoName,
    NotTls,
}

/// The ClientHello's server name, from the stream's first bytes (one handshake record; a ClientHello split over
/// several records is not something browsers send, and is refused as `NotTls`).
pub fn client_hello_sni(buf: &[u8]) -> Sni {
    if buf.is_empty() {
        return Sni::NeedMore;
    }
    if buf[0] != 0x16 {
        return Sni::NotTls;
    }
    if buf.len() < 5 {
        return Sni::NeedMore;
    }
    let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if len > 16 * 1024 + 256 {
        return Sni::NotTls;
    }
    if buf.len() < 5 + len {
        return Sni::NeedMore;
    }
    let body = &buf[5..5 + len];
    parse_hello(body).unwrap_or(Sni::NotTls)
}

fn parse_hello(b: &[u8]) -> Option<Sni> {
    // handshake: type 1 (ClientHello), u24 length
    if *b.first()? != 1 || b.len() < 4 {
        return None;
    }
    let hs_len = ((b[1] as usize) << 16) | ((b[2] as usize) << 8) | b[3] as usize;
    let h = b.get(4..4 + hs_len)?;
    let mut at = 2 + 32; // version, random
    let sid = *h.get(at)? as usize;
    at += 1 + sid;
    let suites = u16::from_be_bytes([*h.get(at)?, *h.get(at + 1)?]) as usize;
    at += 2 + suites;
    let comp = *h.get(at)? as usize;
    at += 1 + comp;
    if at == h.len() {
        return Some(Sni::NoName);
    }
    let ext_len = u16::from_be_bytes([*h.get(at)?, *h.get(at + 1)?]) as usize;
    at += 2;
    let exts = h.get(at..at + ext_len)?;
    let mut e = 0;
    while e + 4 <= exts.len() {
        let kind = u16::from_be_bytes([exts[e], exts[e + 1]]);
        let size = u16::from_be_bytes([exts[e + 2], exts[e + 3]]) as usize;
        let data = exts.get(e + 4..e + 4 + size)?;
        if kind == 0 {
            // server_name: list length, then (type 0, u16 length, name)
            let mut n = 2;
            while n + 3 <= data.len() {
                let kind = data[n];
                let size = u16::from_be_bytes([data[n + 1], data[n + 2]]) as usize;
                let name = data.get(n + 3..n + 3 + size)?;
                if kind == 0 {
                    return Some(Sni::Name(String::from_utf8_lossy(name).to_lowercase()));
                }
                n += 3 + size;
            }
            return Some(Sni::NoName);
        }
        e += 4 + size;
    }
    Some(Sni::NoName)
}

// --- frames ----------------------------------------------------------------------------------------------------------

pub fn frame(kind: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    out.push(kind);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn parse_frame(data: &[u8]) -> Option<(u8, u32, &[u8])> {
    if data.len() < 9 {
        return None;
    }
    let stream = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
    let len = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) as usize;
    if len > MAX_PAYLOAD || data.len() != 9 + len {
        return None;
    }
    Some((data[0], stream, &data[9..]))
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
}

/// One phone's WebSocket: its streams and what it carried.
pub struct Tunnel {
    id: u64,
    device: String,
    network: String,
    out: mpsc::Sender<Vec<u8>>,
    streams: Mutex<HashMap<u32, Slot>>,
    next_stream: AtomicU32,
    started: Instant,
    up: AtomicU64,
    down: AtomicU64,
    opened: AtomicU64,
    last_active: Mutex<Instant>,
    minute: Mutex<(Instant, u64)>,
    stop: CancellationToken,
    why: Mutex<Option<String>>,
}

impl Tunnel {
    fn touch(&self) {
        *self.last_active.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }

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

    fn slot_count(&self) -> usize {
        self.streams.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

#[derive(Default)]
pub struct Hub {
    current: Mutex<Option<Arc<Tunnel>>>,
    sessions: AtomicU64,
}

pub fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(Hub::default)
}

impl Hub {
    fn tunnel(&self) -> Option<Arc<Tunnel>> {
        self.current.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| !t.stop.is_cancelled())
    }

    /// The person's « Arrêter » from the bridge's side (`DELETE /v1/exit/session`).
    pub fn stop(&self, why: &str) -> bool {
        match self.tunnel() {
            Some(t) => {
                t.end(why);
                true
            }
            None => false,
        }
    }

    pub fn status(&self) -> Value {
        match self.tunnel() {
            Some(t) => json!({
                "connected": true, "session": t.id, "device": t.device, "network": t.network,
                "seconds": t.started.elapsed().as_secs(), "up": t.up.load(Ordering::Relaxed),
                "down": t.down.load(Ordering::Relaxed), "streams": t.slot_count(),
                "opened": t.opened.load(Ordering::Relaxed),
            }),
            None => json!({"connected": false}),
        }
    }
}

fn refused_line(host: &str, agent: &str, reason: &str) {
    tracing::info!(target: "bipbridge::phone_exit", host = %host, agent = %agent, reason = %reason, "phone exit refused");
}

/// The tunnel's frames from the phone, until it ends.
async fn read_phone(tunnel: Arc<Tunnel>, mut stream: futures::stream::SplitStream<WebSocket>) {
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
        let mut streams = tunnel.streams.lock().unwrap_or_else(|e| e.into_inner());
        match kind {
            T_OPEN_OK | T_OPEN_ERR => {
                if let Some(slot) = streams.get_mut(&id) {
                    if let Some(opened) = slot.opened.take() {
                        let answer = if kind == T_OPEN_OK {
                            Ok(())
                        } else {
                            Err(String::from_utf8_lossy(payload).chars().take(40).collect())
                        };
                        let _ = opened.send(answer);
                    }
                }
            }
            T_DATA => {
                tunnel.down.fetch_add(payload.len() as u64, Ordering::Relaxed);
                if let Some(slot) = streams.get(&id) {
                    let _ = slot.incoming.send(Incoming::Data(payload.to_vec()));
                }
            }
            T_WINDOW if payload.len() == 4 => {
                let more = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                if let Some(slot) = streams.get(&id) {
                    // never past what a peer could have been owed
                    if slot.credit.available_permits() + more <= 4 * WINDOW as usize {
                        slot.credit.add_permits(more);
                    }
                }
            }
            T_FIN => {
                if let Some(slot) = streams.get(&id) {
                    let _ = slot.incoming.send(Incoming::Fin);
                }
            }
            T_RST => {
                if let Some(slot) = streams.get(&id) {
                    let _ = slot.incoming.send(Incoming::Rst);
                }
            }
            T_STATS => {}
            T_BYE => {
                drop(streams);
                tunnel.end(&format!(
                    "phone-bye:{}",
                    String::from_utf8_lossy(payload).chars().take(20).collect::<String>()
                ));
                return;
            }
            _ => {}
        }
        drop(streams);
        tunnel.touch();
    }
}

/// `GET /v1/exit/tunnel`: the phone's WebSocket (the debug token, or the bridge key).
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
    // HELLO first: who the phone is, its network
    let hello = match tokio::time::timeout(HELLO_WAIT, stream.next()).await {
        Ok(Some(Ok(Message::Binary(b)))) => b,
        _ => return,
    };
    let Some((T_HELLO, _, payload)) = parse_frame(&hello) else { return };
    let info: Value = serde_json::from_slice(payload).unwrap_or(Value::Null);
    let text = |k: &str| info.get(k).and_then(Value::as_str).unwrap_or("").chars().take(64).collect::<String>();
    let hub = hub();
    let (out, mut frames) = mpsc::channel::<Vec<u8>>(64);
    let tunnel = Arc::new(Tunnel {
        id: hub.sessions.fetch_add(1, Ordering::Relaxed) + 1,
        device: text("device"),
        network: text("network"),
        out,
        streams: Mutex::new(HashMap::new()),
        next_stream: AtomicU32::new(1),
        started: Instant::now(),
        up: AtomicU64::new(0),
        down: AtomicU64::new(0),
        opened: AtomicU64::new(0),
        last_active: Mutex::new(Instant::now()),
        minute: Mutex::new((Instant::now(), 0)),
        stop: CancellationToken::new(),
        why: Mutex::new(None),
    });
    // one tunnel at a time: a new one (the phone reconnecting) replaces the one before
    if let Some(old) = hub.current.lock().unwrap_or_else(|e| e.into_inner()).replace(tunnel.clone()) {
        old.end("replaced");
    }
    tracing::info!(target: "bipbridge::phone_exit", session = tunnel.id, device = %tunnel.device,
        network = %tunnel.network, version = %text("version"), "phone exit: tunnel open");
    let writer = {
        let tunnel = tunnel.clone();
        async move {
            let mut ping = tokio::time::interval(PING_EVERY);
            ping.tick().await;
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
                    _ = tunnel.stop.cancelled() => break,
                }
            }
            let why = tunnel.why.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
            if !why.starts_with("phone") {
                let _ = sink.send(Message::Binary(Bytes::from(frame(T_BYE, 0, why.as_bytes())))).await;
            }
            let _ = sink.close().await;
        }
    };
    let watchdog = {
        let tunnel = tunnel.clone();
        let state = state.clone();
        async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = tick.tick() => {}
                    _ = tunnel.stop.cancelled() => return,
                }
                let s = state.config().phone_exit.clone();
                let bytes = tunnel.up.load(Ordering::Relaxed) + tunnel.down.load(Ordering::Relaxed);
                let idle = tunnel.last_active.lock().unwrap_or_else(|e| e.into_inner()).elapsed();
                if !s.enabled {
                    tunnel.end("off");
                } else if s.max_minutes > 0.0 && tunnel.started.elapsed().as_secs_f64() >= s.max_minutes * 60.0 {
                    tunnel.end("expired");
                } else if s.max_mb > 0.0 && bytes as f64 >= s.max_mb * 1_000_000.0 {
                    tunnel.end("limit");
                } else if s.idle_minutes > 0.0
                    && tunnel.slot_count() == 0
                    && idle.as_secs_f64() >= s.idle_minutes * 60.0
                {
                    tunnel.end("idle");
                }
            }
        }
    };
    tokio::join!(read_phone(tunnel.clone(), stream), writer, watchdog);
    // every stream still open ends with it
    for (_, slot) in tunnel.streams.lock().unwrap_or_else(|e| e.into_inner()).drain() {
        let _ = slot.incoming.send(Incoming::Rst);
    }
    {
        let mut current = hub.current.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|t| t.id == tunnel.id) {
            *current = None;
        }
    }
    let why = tunnel.why.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    tracing::info!(target: "bipbridge::phone_exit", session = tunnel.id, device = %tunnel.device,
        seconds = tunnel.started.elapsed().as_secs(), up = tunnel.up.load(Ordering::Relaxed),
        down = tunnel.down.load(Ordering::Relaxed), streams = tunnel.opened.load(Ordering::Relaxed), ended = %why,
        "phone exit: tunnel closed");
}

// --- the agents' proxy's side ----------------------------------------------------------------------------------------

/// The hub's socket for the agents' proxy, while the phone exit is on (started with the bridge).
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
    tracing::info!(target: "bipbridge::phone_exit", hosts = ?config.phone_exit.hosts, "phone exit: on (prototype)");
    loop {
        let Ok((conn, _)) = listener.accept().await else { continue };
        let state = state.clone();
        tokio::spawn(async move { local_stream(state, conn).await });
    }
}

/// The refusal before the phone is asked (None: go on).
pub async fn precheck(settings: &Settings, host: &str, port: u16, agent: &str) -> Option<&'static str> {
    if !settings.enabled {
        return Some("off");
    }
    if let Some(why) = name_refused(host) {
        return Some(why);
    }
    if port != 443 {
        return Some("port");
    }
    if !settings.routes(agent, host) {
        return Some("not-allowed");
    }
    // The phone resolves the name and checks what it found (its network is where its LAN is). This side refuses a
    // name that points here at a private address; a name it cannot resolve is the phone's to judge.
    if !settings.hub_resolve {
        return None;
    }
    if let Ok(Ok(found)) = tokio::time::timeout(RESOLVE_WAIT, tokio::net::lookup_host((host, port))).await {
        let found: Vec<_> = found.collect();
        if found.iter().any(|a| !public_ip(a.ip())) {
            return Some("private");
        }
    }
    None
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
    let (host, port, agent) = match parts.as_slice() {
        ["EXIT", host, port, agent] => (host.to_lowercase(), port.parse::<u16>().unwrap_or(0), agent.to_string()),
        _ => return,
    };
    let host = host.trim_end_matches('.').to_string();
    let settings = state.config().phone_exit.clone();
    let refuse = |reason: &str| format!("ERR {reason}\n");
    if let Some(why) = precheck(&settings, &host, port, &agent).await {
        refused_line(&host, &agent, why);
        let _ = write.write_all(refuse(why).as_bytes()).await;
        return;
    }
    let Some(tunnel) = hub().tunnel() else {
        // never the datacenter instead: the agent waits, its person is asked
        refused_line(&host, &agent, "paused");
        let _ = write.write_all(refuse("paused").as_bytes()).await;
        return;
    };
    let too_many = {
        let mut minute = tunnel.minute.lock().unwrap_or_else(|e| e.into_inner());
        if minute.0.elapsed() >= Duration::from_secs(60) {
            *minute = (Instant::now(), 0);
        }
        minute.1 += 1;
        settings.connections_per_minute > 0 && minute.1 > settings.connections_per_minute
    };
    if too_many {
        refused_line(&host, &agent, "rate");
        let _ = write.write_all(refuse("rate").as_bytes()).await;
        return;
    }
    let id = tunnel.next_stream.fetch_add(1, Ordering::Relaxed);
    let (opened_tx, opened_rx) = oneshot::channel();
    let (incoming_tx, mut incoming) = mpsc::unbounded_channel();
    let credit = Arc::new(Semaphore::new(WINDOW as usize));
    tunnel
        .streams
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, Slot { opened: Some(opened_tx), incoming: incoming_tx, credit: credit.clone() });
    let forget = |t: &Tunnel| {
        t.streams.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    };
    let started = Instant::now();
    let open = json!({"host": host, "port": port, "agent": agent}).to_string();
    tunnel.touch();
    if !tunnel.send(T_OPEN, id, open.as_bytes()).await {
        forget(&tunnel);
        refused_line(&host, &agent, "paused");
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
        let why: String = why.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let why = if why.is_empty() { "phone".to_string() } else { why };
        refused_line(&host, &agent, &format!("phone:{why}"));
        let _ = write.write_all(refuse(&why).as_bytes()).await;
        return;
    }
    let open_ms = started.elapsed().as_millis();
    tunnel.opened.fetch_add(1, Ordering::Relaxed);
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
        let why = match sni {
            Sni::Name(_) => "sni-mismatch",
            Sni::NoName => "sni-missing",
            _ => "not-tls",
        };
        refused_line(&host, &agent, why);
        return;
    }
    let up = Arc::new(AtomicU64::new(0));
    let down = Arc::new(AtomicU64::new(0));
    // client → phone, within the phone's window
    let upward = {
        let tunnel = tunnel.clone();
        let credit = credit.clone();
        let up = up.clone();
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
                    at += take;
                }
                tunnel.touch();
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
    tunnel.touch();
    tracing::info!(target: "bipbridge::phone_exit", session = tunnel.id, stream = id, host = %host, agent = %agent,
        up = up.load(Ordering::Relaxed), down = down.load(Ordering::Relaxed), open_ms = open_ms as u64,
        seconds = started.elapsed().as_secs(), ended = %format!("{a}/{b}"), "phone exit stream");
}

// --- the bridge key's routes (the app, or the operator) --------------------------------------------------------------

async fn status_route(State(state): State<AppState>) -> Json<Value> {
    let config = state.config();
    let mut out = hub().status();
    out["enabled"] = json!(config.phone_exit.enabled);
    out["hosts"] = json!(config.phone_exit.hosts);
    Json(out)
}

async fn stop_route() -> Json<Value> {
    Json(json!({"stopped": hub().stop("user-stop")}))
}

/// Under /v1 with the bridge key: the state, and the kill switch.
pub fn admin_router() -> Router<AppState> {
    Router::new().route("/exit/session", get(status_route).delete(stop_route))
}

/// Under /v1 without the bridge key: the phone's tunnel (its own token).
pub fn open_router() -> Router<AppState> {
    Router::new().route("/exit/tunnel", get(tunnel_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn only_public_addresses() {
        for bad in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.5.4",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.1.1",
            "100.127.255.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "64:ff9b::c0a8:101",
            "64:ff9b::808:808",
            "64:ff9b:1::a00:1",
            "2002:c0a8:101::1",
            "2002:808:808::1",
            "2001:db8::1",
            "2001::1",
        ] {
            assert!(!public_ip(ip(bad)), "{bad}");
        }
        for good in ["8.8.8.8", "151.101.0.223", "100.128.0.1", "::ffff:8.8.8.8", "2a04:4e42::223", "2003::1"] {
            assert!(public_ip(ip(good)), "{good}");
        }
    }

    #[test]
    fn names_never_addresses_or_local() {
        for bad in [
            "192.168.1.1",
            "[::1]",
            "::1",
            "64:ff9b::c0a8:101",
            "10.1",
            "2130706433",
            "0x7f.1",
            "localhost",
            "router",
            "nas.local",
            "box.lan",
            "printer.home",
            "a..b.fr",
            "x.y_z.fr",
            "",
        ] {
            assert!(name_refused(bad).is_some(), "{bad}");
        }
        assert_eq!(name_refused("192.168.1.1"), Some("literal-ip"));
        for good in ["doctolib.fr", "www.doctolib.fr", "xn--bcher-kva.example", "assets.doctolib.fr"] {
            assert_eq!(name_refused(good), None, "{good}");
        }
    }

    #[test]
    fn listed_hosts_exact_or_subdomains() {
        let list = vec!["doctolib.fr".to_string(), "*.doctolib.fr".to_string(), "*.sncf-connect.com".to_string()];
        assert!(host_listed(&list, "doctolib.fr"));
        assert!(host_listed(&list, "WWW.Doctolib.fr."));
        assert!(host_listed(&list, "assets.doctolib.fr"));
        assert!(!host_listed(&list, "sncf-connect.com"));
        assert!(!host_listed(&list, "evil-doctolib.fr"));
        assert!(!host_listed(&list, "doctolib.fr.evil.com"));
        let s = Settings { enabled: true, hosts: list.clone(), agents: vec!["vie".into()], ..Settings::default() };
        assert!(s.routes("vie", "doctolib.fr") && !s.routes("budget", "doctolib.fr"));
        assert!(!Settings { enabled: false, hosts: list, ..Settings::default() }.routes("vie", "doctolib.fr"));
    }

    /// A minimal TLS 1.2/1.3-shaped ClientHello with an optional server name.
    pub(super) fn hello(name: Option<&str>) -> Vec<u8> {
        let mut exts = Vec::new();
        // an unrelated extension first (supported_versions)
        exts.extend_from_slice(&[0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04]);
        if let Some(n) = name {
            let n = n.as_bytes();
            let mut list = vec![0u8];
            list.extend_from_slice(&(n.len() as u16).to_be_bytes());
            list.extend_from_slice(n);
            let mut data = (list.len() as u16).to_be_bytes().to_vec();
            data.extend_from_slice(&list);
            exts.extend_from_slice(&[0, 0]);
            exts.extend_from_slice(&(data.len() as u16).to_be_bytes());
            exts.extend_from_slice(&data);
        }
        let mut body = vec![3, 3];
        body.extend_from_slice(&[7u8; 32]);
        body.push(0); // session id
        body.extend_from_slice(&[0, 2, 0x13, 0x01]);
        body.extend_from_slice(&[1, 0]);
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);
        let mut hs = vec![1, 0, (body.len() >> 8) as u8, body.len() as u8];
        hs.extend_from_slice(&body);
        let mut rec = vec![0x16, 3, 1];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn the_client_hello_names_its_site() {
        let h = hello(Some("www.Doctolib.fr"));
        assert_eq!(client_hello_sni(&h), Sni::Name("www.doctolib.fr".into()));
        assert_eq!(client_hello_sni(&h[..10]), Sni::NeedMore);
        assert_eq!(client_hello_sni(&hello(None)), Sni::NoName);
        assert_eq!(client_hello_sni(b"GET / HTTP/1.1\r\n"), Sni::NotTls);
        let mut broken = hello(Some("a.fr"));
        broken[6] = 0xff; // a handshake length past the record
        assert_eq!(client_hello_sni(&broken), Sni::NotTls);
    }

    #[test]
    fn frames_round_trip_and_bound() {
        let f = frame(T_DATA, 7, b"abc");
        assert_eq!(parse_frame(&f), Some((T_DATA, 7, &b"abc"[..])));
        assert_eq!(parse_frame(&f[..8]), None);
        let mut long = frame(T_DATA, 1, b"");
        long[5..9].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_be_bytes());
        assert_eq!(parse_frame(&long), None);
    }

    #[test]
    fn settings_from_the_table() {
        let s = Settings::from_table(&json!({"enabled": true, "hosts": ["Doctolib.fr.", "*.ameli.fr"], "max_mb": 5}));
        assert!(s.enabled && s.hosts == ["doctolib.fr", "*.ameli.fr"] && s.max_mb == 5.0 && s.idle_minutes == 10.0);
        assert_eq!(Settings::from_table(&json!({})), Settings::default());
        assert!(!Settings::default().enabled);
    }

    #[tokio::test]
    async fn precheck_refuses_before_the_phone() {
        let s =
            Settings { enabled: true, hosts: vec!["*.example.com".into(), "localhost".into()], ..Settings::default() };
        assert_eq!(precheck(&s, "10.0.0.1", 443, "vie").await, Some("literal-ip"));
        assert_eq!(precheck(&s, "localhost", 443, "vie").await, Some("local-name"));
        assert_eq!(precheck(&s, "a.example.com", 80, "vie").await, Some("port"));
        assert_eq!(precheck(&s, "other.org", 443, "vie").await, Some("not-allowed"));
        let off = Settings { enabled: false, ..s.clone() };
        assert_eq!(precheck(&off, "a.example.com", 443, "vie").await, Some("off"));
    }
}
