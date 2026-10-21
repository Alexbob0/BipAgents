//! `WS /v1/voice`: follow a Hermes run and stream synthesized sentences.
//!
//! Client -> server (text frames, JSON):
//!   {"type":"follow","agent":"wellness","run_id":"run_…","voice":"5476","format":"pcm16","from_seq":0}
//!   {"type":"cancel"}
//!   {"type":"ping"}
//! Server -> client:
//!   {"type":"following","run_id","agent","format","sample_rate","channels"}
//!   {"type":"sentence","seq":n,"text":"…","format","sample_rate","channels","bytes":len}  + 1 binary frame
//!   {"type":"done","run_id","reason":"completed|failed|cancelled|interrupted|error|stream_closed"}
//!   {"type":"error","code","message"[,"seq"]}
//!   {"type":"pong"}
//!
//! Port of `bridge/bipbridge/voice_ws.py` (owner group: voice-speech).

use std::sync::{Arc, Mutex};

use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::{AbortHandle, JoinHandle};

use crate::app::AppState;
use crate::auth::{websocket_authorized, ClientAddr};
use crate::config::AgentConfig;
use crate::hermes::{is_delta, is_flush, is_terminal};
use crate::py;
use crate::textproc::SentenceSplitter;
use crate::tts::{format_info, Audio, SynthOptions, TtsError, INTERACTIVE};
use crate::voices::voice_for;

/// `TERMINAL_REASONS`.
pub fn terminal_reason(event_type: &str) -> &'static str {
    match event_type {
        "run.completed" => "completed",
        "run.failed" => "failed",
        "run.cancelled" => "cancelled",
        "run.interrupted" => "interrupted",
        "done" => "completed",
        "error" => "failed",
        _ => "completed",
    }
}

type Sink = Arc<tokio::sync::Mutex<Option<SplitSink<WebSocket, Message>>>>;
type SynthTasks = Arc<Mutex<Vec<(u64, AbortHandle)>>>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn dumps(payload: &Value) -> String {
    py::Dumps::new().unicode().dumps(payload) // json.dumps(payload, ensure_ascii=False)
}

/// The sending half, shared by the session and its follow task (Python's `_send_lock`).
#[derive(Clone)]
struct Sender {
    sink: Sink,
}

impl Sender {
    async fn send_json(&self, payload: Value) {
        let mut guard = self.sink.lock().await;
        if let Some(sink) = guard.as_mut() {
            if sink.send(Message::Text(dumps(&payload).into())).await.is_err() {
                *guard = None; // no longer connected
            }
        }
    }

    /// Keep header and its binary frame adjacent.
    async fn send_sentence(&self, header: Value, audio: bytes::Bytes) {
        let mut guard = self.sink.lock().await;
        if let Some(sink) = guard.as_mut() {
            if sink.send(Message::Text(dumps(&header).into())).await.is_err()
                || sink.send(Message::Binary(audio)).await.is_err()
            {
                *guard = None;
            }
        }
    }
}

pub struct VoiceSession {
    state: AppState,
    sender: Sender,
    task: Option<JoinHandle<()>>,
    synth_tasks: SynthTasks,
}

impl VoiceSession {
    fn new(state: AppState, sender: Sender) -> Self {
        VoiceSession { state, sender, task: None, synth_tasks: Arc::new(Mutex::new(Vec::new())) }
    }

    async fn handle(&mut self, message: serde_json::Map<String, Value>) {
        let mtype = message.get("type").cloned().unwrap_or(Value::Null);
        match mtype.as_str() {
            Some("follow") => self.follow(&message).await,
            Some("cancel") => {
                if self.stop().await {
                    let run_id = message.get("run_id").cloned().unwrap_or(Value::Null);
                    self.sender.send_json(json!({"type": "done", "run_id": run_id, "reason": "cancelled"})).await;
                }
            }
            Some("ping") => self.sender.send_json(json!({"type": "pong"})).await,
            _ => {
                self.sender
                    .send_json(json!({"type": "error", "code": "unknown_message", "message": py::py_str(&mtype)}))
                    .await
            }
        }
    }

    async fn follow(&mut self, message: &serde_json::Map<String, Value>) {
        let config = self.state.config();
        // config.agent(message.get("agent")): str() of any truthy value
        let name = message.get("agent").filter(|v| py::truthy(v)).map(py::py_str);
        let agent = config.agent(name.as_deref()).cloned();
        let or_empty = |key: &str| -> String {
            match message.get(key) {
                Some(v) if py::truthy(v) => py::py_str(v),
                _ => String::new(),
            }
        };
        let run_id = or_empty("run_id").trim().to_string();
        let fmt = match or_empty("format") {
            f if f.is_empty() => "pcm16".to_string(),
            f => f,
        };
        let Some(agent) = agent else {
            self.sender.send_json(json!({"type": "error", "code": "unknown_agent", "message": "agent inconnu"})).await;
            return;
        };
        if run_id.is_empty() || run_id.chars().count() > 256 || run_id.contains('/') {
            self.sender
                .send_json(json!({"type": "error", "code": "invalid_run_id", "message": "run_id invalide"}))
                .await;
            return;
        }
        if format_info(&fmt).is_none() || fmt == "mp3" {
            self.sender.send_json(json!({"type": "error", "code": "invalid_format", "message": fmt})).await;
            return;
        }
        let from_seq = match message.get("from_seq") {
            Some(v) if py::truthy(v) => py::py_int(v).unwrap_or(0).max(0),
            _ => 0,
        };
        self.stop().await;
        let voice = match or_empty("voice") {
            v if v.is_empty() => voice_for(&config, Some(&agent), Some("live")),
            v => v,
        };
        self.sender
            .send_json(json!({"type": "following", "run_id": run_id, "agent": agent.name, "format": fmt,
                              "sample_rate": 24000, "channels": 1}))
            .await;
        tracing::info!(target: "bipbridge.voice", agent = %agent.name, run_id = %run_id, format = %fmt, "voice follow");
        let follow = Follow {
            state: self.state.clone(),
            sender: self.sender.clone(),
            synth_tasks: self.synth_tasks.clone(),
            watch: config.watch_followed_runs,
            sentence_max_chars: config.limits.sentence_max_chars,
        };
        self.task = Some(tokio::spawn(follow.run(agent, run_id, voice, fmt, from_seq as u64)));
    }

    /// Cancel the current follow (SSE subscription + queued synthesis). True if one was active.
    async fn stop(&mut self) -> bool {
        let task = self.task.take();
        for (_, synth) in std::mem::take(&mut *lock(&self.synth_tasks)) {
            synth.abort();
        }
        let Some(task) = task else { return false };
        if task.is_finished() {
            return false;
        }
        task.abort();
        let _ = task.await;
        true
    }

    async fn close(&mut self) {
        self.stop().await;
    }
}

struct Follow {
    state: AppState,
    sender: Sender,
    synth_tasks: SynthTasks,
    watch: bool,
    sentence_max_chars: usize,
}

type Item = (u64, String, u64, JoinHandle<Result<Audio, TtsError>>);

/// Aborts the consumer when the follow task is dropped (cancelled), as Python's `consumer.cancel()`.
struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl Follow {
    async fn run(self, agent: AgentConfig, run_id: String, voice: String, fmt: String, from_seq: u64) {
        let mut listener = self.state.hub.subscribe(&agent, &run_id, "voice", self.watch);
        let mut splitter = SentenceSplitter::new(self.sentence_max_chars);
        let (queue, rx) = mpsc::unbounded_channel::<Option<Item>>();
        let mut seq: u64 = 0;
        let mut spoken: u64 = 0;
        let mut next_task: u64 = 0;
        let tts = self.state.tts.clone();

        let mut enqueue = |sentences: Vec<String>, seq: &mut u64, spoken: &mut u64| {
            for sentence in sentences {
                let n = *seq;
                *seq += 1;
                *spoken += 1;
                if n < from_seq {
                    continue; // already heard before a reconnection
                }
                // Start synthesis now: it queues in the global FIFO, so Kyutai stays busy while
                // earlier sentences are being sent.
                let (tts, text, voice, fmt) = (tts.clone(), sentence.clone(), voice.clone(), fmt.clone());
                let task = tokio::spawn(async move {
                    tts.synthesize(
                        &text,
                        Some(&voice),
                        &fmt,
                        INTERACTIVE,
                        SynthOptions { normalized: true, max_chars: Some(0), relay: None },
                    )
                    .await
                });
                next_task += 1;
                lock(&self.synth_tasks).push((next_task, task.abort_handle()));
                let _ = queue.send(Some((n, sentence, next_task, task)));
            }
        };

        let consumer = tokio::spawn(consume(rx, fmt.clone(), self.sender.clone(), self.synth_tasks.clone()));
        let _guard = AbortOnDrop(consumer.abort_handle());
        let mut reason = "stream_closed";
        while let Some(event) = listener.next().await {
            let etype = event.r#type.as_str();
            if is_delta(etype) {
                enqueue(splitter.feed(&event.text()), &mut seq, &mut spoken);
            } else if is_flush(etype) {
                enqueue(splitter.flush(), &mut seq, &mut spoken);
            } else if etype == "bridge.error" {
                let code = match event.data.get("code") {
                    Some(v) if py::truthy(v) => py::py_str(v),
                    _ => "hermes_error".to_string(),
                };
                self.sender
                    .send_json(json!({"type": "error", "code": code, "message": "flux Hermes indisponible"}))
                    .await;
                reason = "error";
                break;
            } else if is_terminal(etype) {
                reason = terminal_reason(etype);
                enqueue(splitter.flush(), &mut seq, &mut spoken);
                let final_text = event.final_text();
                if spoken == 0 && etype == "run.completed" && !final_text.is_empty() {
                    let mut sentences = splitter.feed(&final_text);
                    sentences.extend(splitter.flush());
                    enqueue(sentences, &mut seq, &mut spoken);
                }
                break;
            }
        }
        enqueue(splitter.flush(), &mut seq, &mut spoken);
        drop(enqueue);
        let _ = queue.send(None);
        let _ = consumer.await;
        self.sender.send_json(json!({"type": "done", "run_id": run_id, "reason": reason})).await;
        tracing::info!(target: "bipbridge.voice", agent = %agent.name, run_id = %run_id, reason = %reason, sentences = seq, "voice done");
        listener.close();
    }
}

async fn consume(mut rx: mpsc::UnboundedReceiver<Option<Item>>, fmt: String, sender: Sender, synth_tasks: SynthTasks) {
    while let Some(Some((seq, sentence, id, task))) = rx.recv().await {
        let result = task.await;
        lock(&synth_tasks).retain(|(n, _)| *n != id);
        let audio = match result {
            Err(_) => return, // cancelled
            Ok(Err(exc)) => {
                tracing::warn!(target: "bipbridge.voice", seq = seq, error = %exc.message, "sentence synthesis failed");
                sender
                    .send_json(json!({"type": "error", "code": "tts_failed", "seq": seq, "message": exc.message}))
                    .await;
                continue;
            }
            Ok(Ok(audio)) => audio,
        };
        let header = json!({"type": "sentence", "seq": seq, "text": sentence, "format": fmt,
                            "sample_rate": audio.sample_rate, "channels": audio.channels, "bytes": audio.data.len()});
        sender.send_sentence(header, audio.data).await;
    }
}

/// `GET /v1/voice` (WebSocket). A wrong key is refused before the upgrade (Python closes with 1008 before
/// accepting, which uvicorn answers with HTTP 403).
pub async fn voice_endpoint(
    State(state): State<AppState>,
    ClientAddr(address): ClientAddr,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    if !websocket_authorized(&state, address.as_deref(), &headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let ws = match ws {
        Ok(ws) => ws,
        Err(rejection) => return rejection.into_response(),
    };
    ws.on_upgrade(move |socket| serve(state, socket))
}

async fn serve(state: AppState, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let sender = Sender { sink: Arc::new(tokio::sync::Mutex::new(Some(sink))) };
    let mut session = VoiceSession::new(state, sender.clone());
    while let Some(Ok(raw)) = stream.next().await {
        let text = match raw {
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue, // answered by the protocol layer, as uvicorn does
            Message::Binary(_) => {
                sender
                    .send_json(json!({"type": "error", "code": "binary_not_supported", "message": "JSON attendu"}))
                    .await;
                continue;
            }
            Message::Text(text) => text,
        };
        match serde_json::from_str::<Value>(text.as_str()) {
            Ok(Value::Object(message)) => session.handle(message).await,
            _ => sender.send_json(json!({"type": "error", "code": "invalid_json", "message": "JSON attendu"})).await,
        }
    }
    session.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons() {
        assert_eq!(terminal_reason("run.cancelled"), "cancelled");
        assert_eq!(terminal_reason("error"), "failed");
        assert_eq!(terminal_reason("done"), "completed");
    }
}
