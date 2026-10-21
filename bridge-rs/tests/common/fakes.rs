//! conftest.py's fakes as real servers on 127.0.0.1: Kyutai, Pocket, Hermes (runs SSE + approvals + sessions), ntfy
//! (one `FakeBackend`, four servers) and APNs (`FakeApns`). Their recorded requests and their knobs are shared with
//! the test through `Arc<Mutex<…>>`, as the Python fakes' attributes.
//!
//! Differences with Python's MockTransport: each fake host is its own server (`backend.urls()` gives them to
//! `config_dict`); a Hermes run whose id starts with « down » answers 200 then breaks the connection (Python raises
//! ConnectError before any answer) — point an agent at `http://127.0.0.1:9` for a refused connection.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use super::{spawn_fake, Urls, HERMES_KEY};

/// `threading.Event`: set once, waited for (with a timeout).
#[derive(Default)]
pub struct Flag {
    set: AtomicBool,
    notify: tokio::sync::Notify,
}

impl Flag {
    pub fn new() -> Arc<Flag> {
        Arc::new(Flag::default())
    }
    pub fn set(&self) {
        self.set.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    pub fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }
    /// `wait_flag(flag, timeout)`.
    pub async fn wait(&self, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_set() {
            let notified = self.notify.notified();
            if self.is_set() {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
    }
}

/// Deterministic fake audio: one int16 sample per character.
pub fn pcm_for(text: &str) -> Vec<u8> {
    text.chars().flat_map(|c| ((c as u32 % 3000) as i16).to_le_bytes()).collect()
}

pub fn make_wav(pcm: &[u8], rate: u32, channels: u16, bits: u16, tag: u16) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend(tag.to_le_bytes());
    fmt.extend(channels.to_le_bytes());
    fmt.extend(rate.to_le_bytes());
    fmt.extend((rate * block as u32).to_le_bytes());
    fmt.extend(block.to_le_bytes());
    fmt.extend(bits.to_le_bytes());
    let mut out = b"RIFF".to_vec();
    out.extend(((4 + 8 + fmt.len() + 8 + pcm.len()) as u32).to_le_bytes());
    out.extend(b"WAVE");
    out.extend(b"fmt ");
    out.extend((fmt.len() as u32).to_le_bytes());
    out.extend(&fmt);
    out.extend(b"data");
    out.extend((pcm.len() as u32).to_le_bytes());
    out.extend(pcm);
    out
}

/// `make_wav(pcm)` with conftest's defaults (24 kHz, mono, 16 bits, PCM).
pub fn wav(pcm: &[u8]) -> Vec<u8> {
    make_wav(pcm, 24000, 1, 16, 1)
}

/// `sse(event_type, **data)`: `event: <t>\ndata: {"event": <t>, ...}\n\n` (Python's json.dumps).
pub fn sse(event_type: &str, data: Value) -> String {
    let mut payload = serde_json::Map::new();
    payload.insert("event".into(), json!(event_type));
    if let Value::Object(map) = data {
        for (k, v) in map {
            payload.insert(k, v);
        }
    }
    format!("event: {event_type}\ndata: {}\n\n", bipbridge::py::dumps(&Value::Object(payload)))
}

/// One step of a scripted run: SSE text, or a flag to wait for.
#[derive(Clone)]
pub enum RunItem {
    Text(String),
    Wait(Arc<Flag>),
}

/// A request a fake received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Seen {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    pub fn header(&self, name: &str) -> Option<String> {
        self.headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string)
    }
}

async fn seen_of(request: Request) -> Seen {
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    Seen {
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().map(str::to_string),
        headers: parts.headers,
        body,
    }
}

/// The fakes' knobs and records (conftest's FakeBackend attributes).
pub struct BackendState {
    pub kyutai_inputs: Vec<Value>,
    pub kyutai_delay: Duration,
    pub kyutai_status: u16,
    /// False: an older Kyutai without /v1/audio/stream.
    pub kyutai_streaming: bool,
    pub pocket_inputs: Vec<Value>,
    pub pocket_status: u16,
    /// run_id -> its SSE script.
    pub runs: HashMap<String, Vec<RunItem>>,
    pub run_status: HashMap<String, u16>,
    pub run_requests: Vec<Seen>,
    pub approvals: Vec<Value>,
    pub approval_response: (u16, Value),
    pub ntfy_lines: HashMap<String, Vec<String>>,
    pub ntfy_requests: Vec<Seen>,
}

impl Default for BackendState {
    fn default() -> Self {
        BackendState {
            kyutai_inputs: Vec::new(),
            kyutai_delay: Duration::ZERO,
            kyutai_status: 200,
            kyutai_streaming: true,
            pocket_inputs: Vec::new(),
            pocket_status: 200,
            runs: HashMap::new(),
            run_status: HashMap::new(),
            run_requests: Vec::new(),
            approvals: Vec::new(),
            approval_response: (200, json!({"object": "hermes.run.approval_response", "resolved": 1})),
            ntfy_lines: HashMap::new(),
            ntfy_requests: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct FakeBackend {
    pub state: Arc<Mutex<BackendState>>,
    /// Set to close the streams that stay open (runs, ntfy); the test's runtime ending closes them too.
    pub release: Arc<Flag>,
    pub kyutai_active: Arc<AtomicUsize>,
    pub kyutai_max_active: Arc<AtomicUsize>,
    pub kyutai_url: String,
    pub pocket_url: String,
    pub hermes_url: String,
    pub ntfy_url: String,
}

impl FakeBackend {
    pub async fn start() -> FakeBackend {
        let state = Arc::new(Mutex::new(BackendState::default()));
        let release = Flag::new();
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let mut fake = FakeBackend {
            state,
            release,
            kyutai_active: active,
            kyutai_max_active: max_active,
            kyutai_url: String::new(),
            pocket_url: String::new(),
            hermes_url: String::new(),
            ntfy_url: String::new(),
        };
        fake.kyutai_url = spawn_fake(Router::new().fallback(any(kyutai)).with_state(fake.clone())).await;
        fake.pocket_url = spawn_fake(Router::new().fallback(any(pocket)).with_state(fake.clone())).await;
        fake.hermes_url = spawn_fake(Router::new().fallback(any(hermes)).with_state(fake.clone())).await;
        fake.ntfy_url = spawn_fake(Router::new().fallback(any(ntfy)).with_state(fake.clone())).await;
        fake
    }

    /// The backends' URLs for `config_dict`.
    pub fn urls(&self) -> Urls {
        Urls {
            kyutai: self.kyutai_url.clone(),
            pocket: self.pocket_url.clone(),
            ntfy: self.ntfy_url.clone(),
            hermes: self.hermes_url.clone(),
        }
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut BackendState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }
}

fn bytes_response(status: u16, content_type: &str, body: Vec<u8>) -> Response {
    Response::builder().status(status).header("content-type", content_type).body(Body::from(body)).unwrap()
}

async fn kyutai(State(fake): State<FakeBackend>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        return Json(json!({"status": "ok"})).into_response();
    }
    let seen = seen_of(request).await;
    let body = seen.json();
    let n = fake.kyutai_active.fetch_add(1, Ordering::SeqCst) + 1;
    fake.kyutai_max_active.fetch_max(n, Ordering::SeqCst);
    let (delay, status, streaming) = fake.with(|s| {
        s.kyutai_inputs.push(body.clone());
        (s.kyutai_delay, s.kyutai_status, s.kyutai_streaming)
    });
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    fake.kyutai_active.fetch_sub(1, Ordering::SeqCst);
    if status != 200 {
        return bytes_response(status, "text/plain; charset=utf-8", b"boom".to_vec());
    }
    let input = body["input"].as_str().unwrap_or("").to_string();
    if seen.path == "/v1/audio/stream" {
        if !streaming {
            return (StatusCode::NOT_FOUND, Json(json!({"detail": "Not Found"}))).into_response();
        }
        let pcm = pcm_for(&input);
        let half = pcm.len() / 2 + 1; // odd split on purpose: chunks need not be sample-aligned
        let half = half.min(pcm.len());
        let chunks = vec![Ok::<_, std::io::Error>(pcm[..half].to_vec()), Ok(pcm[half..].to_vec())];
        return Response::builder()
            .header("content-type", "application/octet-stream")
            .body(Body::from_stream(futures::stream::iter(chunks)))
            .unwrap();
    }
    if body["response_format"] == "wav" {
        return bytes_response(200, "audio/wav", wav(&pcm_for(&input)));
    }
    let mut out = b"ID3".to_vec();
    out.extend(input.as_bytes());
    bytes_response(200, "audio/mpeg", out)
}

/// Pocket TTS speaks the Kyutai API; its fake audio is the text reversed (tells engines apart).
async fn pocket(State(fake): State<FakeBackend>, request: Request) -> Response {
    if request.uri().path() == "/health" {
        let ok = fake.with(|s| s.pocket_status == 200);
        let body = json!({"status": "ok", "voices": {"fr": ["colibri", "galet", "lumen", "mousse", "ours"]}});
        return (if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE }, Json(body)).into_response();
    }
    let seen = seen_of(request).await;
    let body = seen.json();
    let status = fake.with(|s| {
        s.pocket_inputs.push(body.clone());
        s.pocket_status
    });
    if status != 200 {
        return bytes_response(status, "text/plain; charset=utf-8", b"pocket down".to_vec());
    }
    let input = body["input"].as_str().unwrap_or("").to_string();
    let pcm = pcm_for(&input.chars().rev().collect::<String>());
    if seen.path == "/v1/audio/stream" {
        return bytes_response(200, "application/octet-stream", pcm);
    }
    if body["response_format"] == "wav" {
        return bytes_response(200, "audio/wav", wav(&pcm));
    }
    let mut out = b"ID3pocket".to_vec();
    out.extend(input.as_bytes());
    bytes_response(200, "audio/mpeg", out)
}

async fn hermes(State(fake): State<FakeBackend>, request: Request) -> Response {
    let seen = seen_of(request).await;
    let parts: Vec<String> = seen.path.trim_matches('/').split('/').map(str::to_string).collect();
    if seen.header("authorization").as_deref() != Some(&format!("Bearer {HERMES_KEY}")) {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error": "bad key"}))).into_response();
    }
    if parts.len() == 4 && parts[0] == "v1" && parts[1] == "runs" && parts[3] == "events" {
        let run_id = parts[2].clone();
        let (script, status) = fake.with(|s| {
            s.run_requests.push(seen.clone());
            (s.runs.get(&run_id).cloned(), s.run_status.get(&run_id).copied())
        });
        if run_id.starts_with("down") {
            let broken =
                futures::stream::iter(vec![Err::<Bytes, std::io::Error>(std::io::Error::other("connection refused"))]);
            return Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(broken))
                .unwrap();
        }
        let Some(script) = script else {
            return (StatusCode::from_u16(status.unwrap_or(404)).unwrap(), Json(json!({"error": "not found"})))
                .into_response();
        };
        let release = fake.release.clone();
        let stream = async_stream::stream! {
            for item in script {
                match item {
                    RunItem::Wait(flag) => flag.wait(Duration::from_secs(10)).await,
                    RunItem::Text(text) => yield Ok::<_, std::io::Error>(Bytes::from(text)),
                }
            }
            // stay open like a real SSE stream until the test releases it
            release.wait(Duration::from_secs(30)).await;
        };
        return Response::builder()
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(stream))
            .unwrap();
    }
    if parts.len() == 3 && parts[0] == "api" && parts[1] == "sessions" {
        return Json(json!({"id": parts[2], "title": "Bot Chat", "message_count": 7})).into_response();
    }
    if parts.len() == 4 && parts[0] == "v1" && parts[1] == "runs" && parts[3] == "approval" {
        let (status, payload) = fake.with(|s| {
            s.approvals.push(json!({"run_id": parts[2], "body": seen.json()}));
            s.approval_response.clone()
        });
        return (StatusCode::from_u16(status).unwrap(), Json(payload)).into_response();
    }
    (StatusCode::NOT_FOUND, Json(json!({"error": "no route"}))).into_response()
}

async fn ntfy(State(fake): State<FakeBackend>, request: Request) -> Response {
    if request.uri().path() == "/v1/health" {
        return Json(json!({"healthy": true})).into_response();
    }
    let seen = seen_of(request).await;
    let topic = seen.path.trim_matches('/').split('/').next().unwrap_or("").to_string();
    let lines = fake.with(|s| {
        s.ntfy_requests.push(seen.clone());
        s.ntfy_lines.remove(&topic).unwrap_or_default()
    });
    let release = fake.release.clone();
    let stream = async_stream::stream! {
        let open = bipbridge::py::dumps(&json!({"id": "open1", "event": "open", "topic": topic}));
        yield Ok::<_, std::io::Error>(Bytes::from(format!("{open}\n")));
        for line in lines {
            yield Ok(Bytes::from(format!("{line}\n")));
        }
        release.wait(Duration::from_secs(30)).await;
    };
    Response::new(Body::from_stream(stream))
}

/// token -> (status, reason).
pub type ApnsAnswers = Arc<Mutex<HashMap<String, (u16, Option<String>)>>>;

/// APNs: records the pushes, answers per token (`responses`).
#[derive(Clone, Default)]
pub struct FakeApns {
    pub requests: Arc<Mutex<Vec<Seen>>>,
    pub responses: ApnsAnswers,
    pub url: String,
}

impl FakeApns {
    /// Its URL goes to `[apns] base_url`.
    pub async fn start() -> FakeApns {
        let mut fake = FakeApns::default();
        let state = fake.clone();
        fake.url = spawn_fake(
            Router::new()
                .route("/3/device/{token}", post(apns))
                .fallback(any(|| async { StatusCode::NOT_FOUND }))
                .with_state(state),
        )
        .await;
        fake
    }

    pub fn payloads(&self) -> Vec<Value> {
        self.requests.lock().unwrap().iter().map(Seen::json).collect()
    }
}

async fn apns(State(fake): State<FakeApns>, Path(token): Path<String>, request: Request) -> Response {
    let seen = seen_of(request).await;
    fake.requests.lock().unwrap().push(seen);
    let (status, reason) = fake.responses.lock().unwrap().get(&token).cloned().unwrap_or((200, None));
    if status == 200 {
        return Response::builder().status(200).header("apns-id", "abc").body(Body::empty()).unwrap();
    }
    (StatusCode::from_u16(status).unwrap(), Json(json!({"reason": reason}))).into_response()
}

/// `wait_until(predicate, timeout)`: polls every 20 ms.
pub async fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    predicate()
}
