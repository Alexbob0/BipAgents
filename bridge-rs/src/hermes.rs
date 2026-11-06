//! Hermes api_server client: lenient SSE parsing of `GET /v1/runs/{id}/events` and approvals.
//!
//! Also the text plumbing other modules need to read a streamed body as httpx does: [`Utf8Decoder`] (incremental,
//! invalid bytes replaced) and [`LineDecoder`] (httpx's `aiter_lines`: Python's `splitlines` separators, a `\r`
//! held back until the next chunk), and [`httpx_error_name`] (the httpx exception name a transport error shows
//! as, e.g. in `/v1/status`).

use std::pin::Pin;
use std::time::Duration;

use futures::{Stream, StreamExt};
use serde_json::{json, Map, Value};

use crate::config::AgentConfig;
use crate::py;

pub const DELTA_TYPES: [&str; 3] = ["message.delta", "assistant.delta", "response.output_text.delta"];
pub const TERMINAL_TYPES: [&str; 6] =
    ["run.completed", "run.failed", "run.cancelled", "run.interrupted", "done", "error"];

pub fn is_delta(event_type: &str) -> bool {
    DELTA_TYPES.contains(&event_type)
}

pub fn is_terminal(event_type: &str) -> bool {
    TERMINAL_TYPES.contains(&event_type)
}

/// The `guest_turn` mark of a run or a session chat (Hermes patch 0018): someone other than the agent's person writes
/// this turn, which may use `tools` only (exact names, `mcp__bipagents__<tool>` or `web_search`).
pub fn guest_turn(tools: &[String]) -> Value {
    serde_json::json!({ "tools": tools })
}

#[derive(Debug, Clone, PartialEq)]
pub struct SSEEvent {
    pub r#type: String,
    pub data: Map<String, Value>,
    pub raw: String,
}

impl SSEEvent {
    pub fn new(event_type: impl Into<String>, data: Map<String, Value>, raw: impl Into<String>) -> Self {
        SSEEvent { r#type: event_type.into(), data, raw: raw.into() }
    }

    /// Delta text for delta events (`delta`, `text` or `content`; nested dicts tolerated).
    pub fn text(&self) -> String {
        for key in ["delta", "text", "content"] {
            match self.data.get(key) {
                Some(Value::String(s)) => return s.clone(),
                Some(Value::Object(inner)) => {
                    let content = inner.get("content").filter(|v| py::truthy(v)).or_else(|| inner.get("text"));
                    if let Some(Value::String(s)) = content {
                        return s.clone();
                    }
                }
                _ => {}
            }
        }
        String::new()
    }

    /// Final answer carried by a `run.completed` event, if any.
    pub fn final_text(&self) -> String {
        for key in ["output", "final_output", "response", "text", "content", "result"] {
            if let Some(Value::String(s)) = self.data.get(key) {
                if !s.trim().is_empty() {
                    return s.clone();
                }
            }
        }
        String::new()
    }
}

/// Line-oriented, lenient SSE parser.
///
/// Type comes from the JSON `type` (or `event`) field, else the SSE `event:` line. Comment
/// lines (`: keepalive`) are ignored. A bare JSON line outside SSE framing is accepted too.
///
/// Half an emoji: a delta whose JSON holds a lone UTF-16 surrogate escape (`"\ud83d"`, then `"\ude0a"` in the next
/// one: a router cutting a JavaScript string between two tokens) is not valid JSON for serde; it was passed on as an
/// unparsed `message` that both apps dropped, the character lost. Such halves are read here and joined across the
/// deltas of the stream; a half without its partner becomes U+FFFD.
#[derive(Debug, Default)]
pub struct SSEParser {
    event_name: Option<String>,
    data: Vec<String>,
    /// The high half of a character whose low half should start the next delta.
    high: Option<u32>,
}

impl SSEParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed_line(&mut self, line: &str) -> Option<SSEEvent> {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let stripped = line.trim();
        if stripped.starts_with('{') && self.data.is_empty() && self.event_name.is_none() {
            return self.build(None, stripped); // bare JSON line (NDJSON-style), tolerated
        }
        let (name, value) = match line.split_once(':') {
            Some((n, v)) => (n, v),
            None => (line, ""),
        };
        let value = value.strip_prefix(' ').unwrap_or(value);
        if name == "event" {
            let v = value.trim();
            self.event_name = if v.is_empty() { None } else { Some(v.to_string()) };
        } else if name == "data" {
            self.data.push(value.to_string());
        }
        None // id:, retry: and unknown fields are ignored
    }

    pub fn finish(&mut self) -> Option<SSEEvent> {
        self.dispatch()
    }

    fn dispatch(&mut self) -> Option<SSEEvent> {
        if self.data.is_empty() && self.event_name.is_none() {
            return None;
        }
        let name = self.event_name.take();
        let payload = std::mem::take(&mut self.data).join("\n");
        self.build(name.as_deref(), &payload)
    }

    fn build(&mut self, name: Option<&str>, payload: &str) -> Option<SSEEvent> {
        if payload.trim() == "[DONE]" {
            return Some(SSEEvent::new("done", Map::new(), payload));
        }
        let mut data = Map::new();
        let mut halves = false;
        if !payload.trim().is_empty() {
            let parsed = serde_json::from_str::<Value>(payload).or_else(|err| match mark_lone_surrogates(payload) {
                Some(marked) => {
                    halves = true;
                    serde_json::from_str::<Value>(&marked)
                }
                None => Err(err),
            });
            match parsed {
                Ok(Value::Object(parsed)) => data = parsed,
                Ok(other) => {
                    data.insert("value".into(), other);
                }
                Err(_) => {
                    data.insert("text".into(), Value::String(payload.to_string()));
                }
            }
        }
        let etype = [data.get("type"), data.get("event")]
            .into_iter()
            .flatten()
            .find(|v| py::truthy(v))
            .map(py::py_str)
            .or_else(|| name.filter(|n| !n.is_empty()).map(str::to_string))
            .unwrap_or_else(|| "message".into());
        if !halves && self.high.is_none() {
            return Some(SSEEvent::new(etype, data, payload));
        }
        if is_delta(&etype) {
            // the halves joined across deltas; a high half at the end waits for the next one
            if let Some((key, Value::String(text))) =
                ["delta", "text", "content"].iter().find_map(|k| data.get(*k).map(|v| (*k, v.clone())))
            {
                data.insert(key.into(), Value::String(self.join_halves(&text)));
            }
        } else {
            self.high = None;
        }
        let mut value = Value::Object(data);
        unmark(&mut value);
        let raw = py::Dumps::new().unicode().dumps(&value);
        let data = match value {
            Value::Object(data) => data,
            _ => Map::new(),
        };
        Some(SSEEvent::new(etype, data, raw))
    }

    /// A delta's text with its marked halves (`mark_lone_surrogates`) made characters again: a high half and the low
    /// half after it (in this delta or the next) one character, a half alone U+FFFD.
    fn join_halves(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for c in text.chars() {
            match half(c) {
                Some(high @ 0xD800..=0xDBFF) => {
                    if self.high.replace(high).is_some() {
                        out.push('\u{FFFD}');
                    }
                }
                Some(low) => match self.high.take() {
                    Some(high) => {
                        let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    None => out.push('\u{FFFD}'),
                },
                None => {
                    if self.high.take().is_some() {
                        out.push('\u{FFFD}');
                    }
                    out.push(c);
                }
            }
        }
        out
    }
}

/// Where a lone surrogate's mark lives: the private-use character U+F0000 + (surrogate − 0xD800).
const HALF_MARK: u32 = 0xF0000;

/// The surrogate a mark stands for.
fn half(c: char) -> Option<u32> {
    let code = c as u32;
    (HALF_MARK..HALF_MARK + 0x800).contains(&code).then(|| code - HALF_MARK + 0xD800)
}

/// Marks left once the halves are joined (outside a delta, or never paired) become U+FFFD.
fn unmark(value: &mut Value) {
    match value {
        Value::String(text) if text.chars().any(|c| half(c).is_some()) => {
            *text = text.chars().map(|c| if half(c).is_some() { '\u{FFFD}' } else { c }).collect();
        }
        Value::Array(items) => items.iter_mut().for_each(unmark),
        Value::Object(map) => map.values_mut().for_each(unmark),
        _ => {}
    }
}

/// JSON text with each lone UTF-16 surrogate escape (`\ud83d` without its low half after it, or a low half alone)
/// written as its mark (`HALF_MARK`), so serde reads it; None when it has none.
fn mark_lone_surrogates(payload: &str) -> Option<String> {
    fn escape_at(bytes: &[u8], at: usize) -> Option<u32> {
        let escape = bytes.get(at..at + 6)?;
        if escape[0] != b'\\' || escape[1] != b'u' {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
    }
    let bytes = payload.as_bytes();
    let mut out = String::with_capacity(payload.len());
    let (mut at, mut from) = (0, 0);
    while at < bytes.len() {
        if bytes[at] != b'\\' {
            at += 1;
            continue;
        }
        match escape_at(bytes, at) {
            Some(0xD800..=0xDBFF) if matches!(escape_at(bytes, at + 6), Some(0xDC00..=0xDFFF)) => at += 12,
            Some(code @ 0xD800..=0xDFFF) => {
                out.push_str(&payload[from..at]);
                out.push(char::from_u32(HALF_MARK + code - 0xD800).unwrap_or('\u{FFFD}'));
                at += 6;
                from = at;
            }
            _ => at += 2, // another escape (`\\`, `\"`, `é`): its first two bytes skipped, the rest is plain
        }
    }
    if from == 0 {
        return None;
    }
    out.push_str(&payload[from..]);
    Some(out)
}

/// Incremental UTF-8 decoding with invalid bytes replaced (U+FFFD), a character split across chunks kept whole.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn decode(&mut self, chunk: &[u8], last: bool) -> String {
        let mut buf = std::mem::take(&mut self.pending);
        buf.extend_from_slice(chunk);
        if !last {
            // Hold back an incomplete sequence at the very end.
            let tail_start = buf.len().saturating_sub(3);
            for start in (tail_start..buf.len()).rev() {
                let b = buf[start];
                if b & 0xC0 == 0x80 {
                    continue; // a continuation byte: look further back
                }
                let needed = if b >= 0xF0 {
                    4
                } else if b >= 0xE0 {
                    3
                } else if b >= 0xC0 {
                    2
                } else {
                    1
                };
                if needed > buf.len() - start
                    && std::str::from_utf8(&buf[start..]).err().map(|e| e.error_len().is_none()).unwrap_or(false)
                {
                    self.pending = buf.split_off(start);
                }
                break;
            }
        }
        String::from_utf8_lossy(&buf).into_owned()
    }
}

const NEWLINE_CHARS: [char; 10] =
    ['\n', '\r', '\u{0b}', '\u{0c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}', '\u{2029}'];

/// Python's `str.splitlines()` (no ends kept).
pub fn splitlines(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if NEWLINE_CHARS.contains(&c) {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            lines.push(std::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// httpx's `LineDecoder`: lines from text chunks, as `aiter_lines` gives them.
#[derive(Debug, Default)]
pub struct LineDecoder {
    buffer: Vec<String>,
    trailing_cr: bool,
}

impl LineDecoder {
    pub fn decode(&mut self, text: &str) -> Vec<String> {
        let mut text = text.to_string();
        // We always push a trailing `\r` into the next decode iteration.
        if self.trailing_cr {
            text.insert(0, '\r');
            self.trailing_cr = false;
        }
        if text.ends_with('\r') {
            self.trailing_cr = true;
            text.pop();
        }
        if text.is_empty() {
            return Vec::new();
        }
        let trailing_newline = text.chars().last().map(|c| NEWLINE_CHARS.contains(&c)).unwrap_or(false);
        let mut lines = splitlines(&text);
        if lines.len() == 1 && !trailing_newline {
            // No new lines, buffer the input and continue.
            self.buffer.push(lines.remove(0));
            return Vec::new();
        }
        if !self.buffer.is_empty() {
            let head = self.buffer.concat() + lines.first().map(String::as_str).unwrap_or("");
            if lines.is_empty() {
                lines.push(head);
            } else {
                lines[0] = head;
            }
            self.buffer.clear();
        }
        if !trailing_newline {
            // If the last segment of splitlines is not newline terminated, then drop it from our output and start a
            // new buffer.
            if let Some(last) = lines.pop() {
                self.buffer = vec![last];
            }
        }
        lines
    }

    pub fn flush(&mut self) -> Vec<String> {
        if self.buffer.is_empty() && !self.trailing_cr {
            return Vec::new();
        }
        let line = self.buffer.concat();
        self.buffer.clear();
        self.trailing_cr = false;
        vec![line]
    }
}

/// A body read line by line, as httpx's `aiter_lines()`: chunks decoded, then split.
pub fn lines_of<S, E>(chunks: S) -> impl Stream<Item = Result<String, E>> + Send
where
    S: Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: Send + 'static,
{
    async_stream::stream! {
        let mut utf8 = Utf8Decoder::default();
        let mut lines = LineDecoder::default();
        futures::pin_mut!(chunks);
        while let Some(chunk) = chunks.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = utf8.decode(&bytes, false);
                    if text.is_empty() { continue; }
                    for line in lines.decode(&text) {
                        yield Ok(line);
                    }
                }
                Err(e) => { yield Err(e); return; }
            }
        }
        let rest = utf8.decode(&[], true);
        if !rest.is_empty() {
            for line in lines.decode(&rest) {
                yield Ok(line);
            }
        }
        for line in lines.flush() {
            yield Ok(line);
        }
    }
}

/// SSE events from lines (`iter_sse`).
pub fn iter_sse<S, E>(lines: S) -> impl Stream<Item = Result<SSEEvent, E>> + Send
where
    S: Stream<Item = Result<String, E>> + Send + 'static,
    E: Send + 'static,
{
    async_stream::stream! {
        let mut parser = SSEParser::new();
        futures::pin_mut!(lines);
        while let Some(line) = lines.next().await {
            match line {
                Ok(line) => {
                    if let Some(event) = parser.feed_line(&line) {
                        yield Ok(event);
                    }
                }
                Err(e) => { yield Err(e); return; }
            }
        }
        if let Some(last) = parser.finish() {
            yield Ok(last);
        }
    }
}

/// The name of the httpx exception a transport error would be in Python (`ConnectError`, `ReadTimeout`…).
pub fn httpx_error_name(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        if err.is_connect() {
            "ConnectTimeout"
        } else {
            "ReadTimeout"
        }
    } else if err.is_connect() {
        "ConnectError"
    } else if err.is_decode() {
        "DecodingError"
    } else if err.is_body() {
        "ReadError"
    } else if err.is_builder() || err.is_request() {
        "RemoteProtocolError"
    } else {
        "HTTPError"
    }
}

/// What a Hermes call fails with: `HermesHTTPError` (Hermes answered, not as hoped), an httpx transport error, or
/// a body that is not JSON (Python's `ValueError` from `resp.json()`).
#[derive(Debug, thiserror::Error)]
pub enum HermesError {
    /// `HermesHTTPError(status, body)`: its text is « Hermes HTTP <status> ».
    #[error("Hermes HTTP {status}")]
    Http { status: u16, body: String },
    /// `httpx.HTTPError`: unreachable, refused, timed out.
    #[error("{0}")]
    Transport(#[from] reqwest::Error),
    /// A read timed out while streaming (httpx's `ReadTimeout`).
    #[error("read timed out")]
    ReadTimeout,
    /// `ValueError`: the answer was not JSON.
    #[error("{0}")]
    Decode(String),
}

impl HermesError {
    pub fn http(status: u16, body: impl Into<String>) -> Self {
        HermesError::Http { status, body: body.into() }
    }

    /// The HTTP status of a `HermesHTTPError`.
    pub fn status(&self) -> Option<u16> {
        match self {
            HermesError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// An `httpx.HTTPError` in Python (what routes turn into « Hermes unreachable »).
    pub fn is_transport(&self) -> bool {
        matches!(self, HermesError::Transport(_) | HermesError::ReadTimeout)
    }

    /// `type(exc).__name__` in Python.
    pub fn type_name(&self) -> &'static str {
        match self {
            HermesError::Http { .. } => "HermesHTTPError",
            HermesError::Transport(e) => httpx_error_name(e),
            HermesError::ReadTimeout => "ReadTimeout",
            HermesError::Decode(_) => "JSONDecodeError",
        }
    }
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<SSEEvent, HermesError>> + Send>>;

/// The first `n` characters (Python's `text[:n]`).
/// Just claimed (cloud_setup.rs): the gateway restarting, or the agent still being made, is waited for.
async fn ready(agent: &AgentConfig) {
    crate::cloud_setup::READINESS.wait(&agent.name, crate::cloud_setup::READY_WAIT).await;
}

/// Before a turn: a new agent's first one waits for its tools (agents_admin.rs `tools_loaded`).
async fn turn_ready(agent: &AgentConfig) {
    crate::mcp_sessions::SESSIONS.first_turn(&agent.name, crate::mcp_sessions::FIRST_TURN_WAIT).await;
}

pub fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

#[derive(Clone)]
pub struct HermesClient {
    pub client: reqwest::Client,
}

impl HermesClient {
    pub fn new(client: reqwest::Client) -> Self {
        HermesClient { client }
    }

    fn headers(agent: &AgentConfig, accept: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(v) = format!("Bearer {}", agent.hermes_key).parse() {
            headers.insert("Authorization", v);
        }
        if let Ok(v) = accept.parse() {
            headers.insert("Accept", v);
        }
        headers
    }

    async fn get_json(&self, agent: &AgentConfig, path: &str, params: &[(&str, String)]) -> Result<Value, HermesError> {
        ready(agent).await;
        let resp = self
            .client
            .get(format!("{}{}", agent.hermes_url, path))
            .query(params)
            .headers(Self::headers(agent, "application/json"))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let body = resp.text().await?;
        if status != 200 {
            return Err(HermesError::http(status, head(&body, 500)));
        }
        serde_json::from_str(&body).map_err(|e| HermesError::Decode(e.to_string()))
    }

    /// `GET /v1/runs/{id}`: status (`queued`, `running`, `waiting_for_approval`, then its end), session, and the
    /// prompts a parked run waits for (`pending_controls`, hermes_prompts.rs). A run Hermes reports `unknown` comes back
    /// settled (`resolve_unknown`).
    pub async fn run(&self, agent: &AgentConfig, run_id: &str) -> Result<Map<String, Value>, HermesError> {
        let data = self.get_json(agent, &format!("/v1/runs/{run_id}"), &[]).await?;
        let run = match data {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        if run.get("status").and_then(Value::as_str) == Some("unknown") {
            if let Some(settled) = self.resolve_unknown(agent, run_id, &run).await {
                return Ok(settled);
            }
        }
        Ok(run)
    }

    /// A run whose gateway restarted mid-turn (`unknown`): whether its turn finished is unknown, and the
    /// conversation's next turns wait until someone says it won't. The bridge never sends such a turn again: it
    /// acknowledges the loss (`POST /v1/runs/{id}/resolve-unknown` with the exact admission and generation the status
    /// gives), which settles the run `interrupted`. The run as settled; None when Hermes refused (asked again at the
    /// next poll).
    async fn resolve_unknown(
        &self,
        agent: &AgentConfig,
        run_id: &str,
        run: &Map<String, Value>,
    ) -> Option<Map<String, Value>> {
        let admission = run.get("admission_id").and_then(Value::as_str)?;
        let generation = crate::hermes_prompts::generation(run)?;
        let body = json!({"admission_id": admission, "execution_generation": generation});
        let path = format!("/v1/runs/{run_id}/resolve-unknown");
        match self.send(agent, reqwest::Method::POST, &path, Some(body), 15.0).await {
            Ok(_) => {
                tracing::info!(agent = %agent.name, run_id = %run_id, "run lost in a Hermes restart, settled interrupted");
                let mut settled = run.clone();
                settled.insert("status".into(), json!("interrupted"));
                settled.insert("error".into(), json!(LOST_IN_RESTART));
                Some(settled)
            }
            Err(exc) => {
                tracing::warn!(agent = %agent.name, run_id = %run_id, error = %exc, "unknown run not settled");
                None
            }
        }
    }

    /// `GET /v1/capabilities`: answers when the agent's gateway is up.
    pub async fn capabilities(&self, agent: &AgentConfig) -> Result<Value, HermesError> {
        self.get_json(agent, "/v1/capabilities", &[]).await
    }

    /// The rows of a `GET /api/sessions` answer, whichever key holds them: `data` (Hermes v0.22.5's
    /// `{"object": "list", "data": [...]}`), `sessions` or `items`, or a bare list.
    pub fn session_rows(data: Value) -> Vec<Map<String, Value>> {
        Self::items(data, ["sessions", "data", "items"])
    }

    fn items(data: Value, keys: [&str; 3]) -> Vec<Map<String, Value>> {
        let items = match data {
            Value::Object(mut m) => {
                // data.get("sessions", data.get("data", data.get("items")))
                let mut found = None;
                for key in keys {
                    if let Some(v) = m.remove(key) {
                        found = Some(v);
                        break;
                    }
                }
                found.unwrap_or(Value::Null)
            }
            other => other,
        };
        match items {
            Value::Array(list) => list
                .into_iter()
                .filter_map(|item| match item {
                    Value::Object(m) => Some(m),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `GET /api/jobs?include_disabled=true`: the jobs Hermes kept before the bridge ran the routines (read once by
    /// routines.rs `migrate`).
    pub async fn routines(&self, agent: &AgentConfig) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self.get_json(agent, "/api/jobs", &[("include_disabled", "true".to_string())]).await?;
        Ok(Self::items(data, ["jobs", "data", "items"]))
    }

    /// `DELETE /api/jobs/{id}`: a job moved to the bridge's routines (routines.rs `migrate`).
    pub async fn delete_routine(&self, agent: &AgentConfig, id: &str) -> Result<(), HermesError> {
        self.send(agent, reqwest::Method::DELETE, &format!("/api/jobs/{id}"), None, 15.0).await?;
        Ok(())
    }

    /// `GET /api/sessions` (most recent first).
    pub async fn list_sessions(&self, agent: &AgentConfig, limit: i64) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self.get_json(agent, "/api/sessions", &[("limit", limit.to_string())]).await?;
        Ok(Self::items(data, ["sessions", "data", "items"]))
    }

    /// `GET /api/sessions` with these parameters (threads: `include_children`, `include_hidden`).
    pub async fn list_sessions_with(
        &self,
        agent: &AgentConfig,
        params: &[(&str, String)],
    ) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self.get_json(agent, "/api/sessions", params).await?;
        Ok(Self::items(data, ["sessions", "data", "items"]))
    }

    /// The agent's « Bot Chat » session(s) (its single thread), hidden ones included, oldest first (read_shares.rs:
    /// a project agent's whole thread shared).
    pub async fn bot_chat_sessions(&self, agent: &AgentConfig) -> Result<Vec<Map<String, Value>>, HermesError> {
        let params = [("limit", "200".to_string()), ("title", crate::threads::LEGACY_MAIN_TITLE.to_string()), ("include_hidden", "1".into())];
        let data = self.get_json(agent, "/api/sessions", &params).await?;
        let mut rows: Vec<Map<String, Value>> = Self::items(data, ["sessions", "data", "items"])
            .into_iter()
            .filter(crate::cronwatch::is_bot_chat)
            .collect();
        let started = |row: &Map<String, Value>| {
            ["started_at", "created_at", "created"]
                .iter()
                .find_map(|k| row.get(*k).and_then(Value::as_f64))
                .unwrap_or(0.0)
        };
        rows.sort_by(|a, b| started(a).total_cmp(&started(b)));
        Ok(rows)
    }

    /// `GET /api/sessions?limit=&offset=`: one page of the list (Hermes reads at most 200 at a time).
    pub async fn list_sessions_page(
        &self,
        agent: &AgentConfig,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<Map<String, Value>>, HermesError> {
        let params = [("limit", limit.to_string()), ("offset", offset.to_string())];
        let data = self.get_json(agent, "/api/sessions", &params).await?;
        Ok(Self::items(data, ["sessions", "data", "items"]))
    }

    /// `GET /api/sessions/{id}`.
    pub async fn session(&self, agent: &AgentConfig, session_id: &str) -> Result<Map<String, Value>, HermesError> {
        let data = self.get_json(agent, &format!("/api/sessions/{session_id}"), &[]).await?;
        Ok(match data {
            Value::Object(mut m) => match m.remove("session") {
                Some(Value::Object(inner)) => inner,
                Some(other) => {
                    m.insert("session".into(), other);
                    m
                }
                None => m,
            },
            _ => Map::new(),
        })
    }

    /// `GET /api/sessions/{id}/messages` (oldest first).
    pub async fn session_messages(
        &self,
        agent: &AgentConfig,
        session_id: &str,
    ) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self
            .get_json(agent, &format!("/api/sessions/{session_id}/messages"), &[("inline_images", "false".into())])
            .await?;
        Ok(Self::items(data, ["messages", "data", "items"]))
    }

    /// The same with the photos inline (`data:` URIs: megabytes), for the few times they are needed (read_shares.rs).
    pub async fn session_messages_with_images(
        &self,
        agent: &AgentConfig,
        session_id: &str,
    ) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self
            .get_json(agent, &format!("/api/sessions/{session_id}/messages"), &[("inline_images", "true".into())])
            .await?;
        Ok(Self::items(data, ["messages", "data", "items"]))
    }

    /// One page of a conversation's rows, oldest first (`limit` 500 at most, Hermes' cap), compacted rows included:
    /// v0.22.5 gives the whole compression lineage root → `session_id`, each row with its own `session_id`
    /// (threads/source.rs keeps the segment's own rows).
    pub async fn session_messages_page(
        &self,
        agent: &AgentConfig,
        session_id: &str,
        offset: usize,
        images: bool,
    ) -> Result<Vec<Map<String, Value>>, HermesError> {
        let params = [
            ("order", "oldest".to_string()),
            ("limit", "500".to_string()),
            ("offset", offset.to_string()),
            ("include_compacted", "true".to_string()),
            ("inline_images", images.to_string()),
        ];
        let data = self.get_json(agent, &format!("/api/sessions/{session_id}/messages"), &params).await?;
        Ok(Self::items(data, ["messages", "data", "items"]))
    }

    async fn send(
        &self,
        agent: &AgentConfig,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: f64,
    ) -> Result<Value, HermesError> {
        ready(agent).await;
        let mut request = self
            .client
            .request(method, format!("{}{}", agent.hermes_url, path))
            .headers(Self::headers(agent, "application/json"))
            .timeout(Duration::from_secs_f64(timeout));
        if let Some(body) = body {
            request = request.json(&body);
        }
        let resp = request.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        if ![200, 201, 202, 204].contains(&status) {
            return Err(HermesError::http(status, head(&text, 500)));
        }
        Ok(serde_json::from_str(&text).unwrap_or_else(|_| json!({})))
    }

    /// `POST /api/sessions`: the new session's id.
    pub async fn create_session(&self, agent: &AgentConfig, title: &str) -> Result<String, HermesError> {
        let data =
            self.send(agent, reqwest::Method::POST, "/api/sessions", Some(json!({"title": title})), 15.0).await?;
        let inner = match data.get("session") {
            Some(s @ Value::Object(_)) => s.clone(),
            _ => data,
        };
        match inner.get("id") {
            Some(Value::String(id)) => Ok(id.clone()),
            _ => Err(HermesError::http(502, "no session id")),
        }
    }

    /// `PATCH /api/sessions/{id}` (title, hidden, archived…).
    pub async fn update_session(
        &self,
        agent: &AgentConfig,
        session_id: &str,
        fields: Map<String, Value>,
    ) -> Result<(), HermesError> {
        self.send(
            agent,
            reqwest::Method::PATCH,
            &format!("/api/sessions/{session_id}"),
            Some(Value::Object(fields)),
            15.0,
        )
        .await?;
        Ok(())
    }

    pub async fn delete_session(&self, agent: &AgentConfig, session_id: &str) -> Result<(), HermesError> {
        self.send(agent, reqwest::Method::DELETE, &format!("/api/sessions/{session_id}"), None, 15.0).await?;
        Ok(())
    }

    /// `POST /api/sessions/{id}/chat`: one turn, its reply's text. `timeout`: Python's default is 240 s.
    pub async fn session_chat(
        &self,
        agent: &AgentConfig,
        session_id: &str,
        message: &str,
        extra: Option<Map<String, Value>>,
        timeout: f64,
    ) -> Result<String, HermesError> {
        turn_ready(agent).await;
        let mut body = Map::new();
        body.insert("message".into(), Value::String(message.to_string()));
        for (k, v) in extra.unwrap_or_default() {
            body.insert(k, v);
        }
        let data = self
            .send(
                agent,
                reqwest::Method::POST,
                &format!("/api/sessions/{session_id}/chat"),
                Some(Value::Object(body)),
                timeout,
            )
            .await?;
        Ok(data.get("message").and_then(|m| m.get("content")).and_then(Value::as_str).unwrap_or("").to_string())
    }

    /// `POST /api/sessions/{id}/chat/stream`: one turn, Hermes' SSE answer as it comes (a Live call's, live_turns.rs).
    pub async fn chat_stream(&self, agent: &AgentConfig, session_id: &str, body: Value) -> Result<reqwest::Response, HermesError> {
        turn_ready(agent).await;
        ready(agent).await;
        let url = format!("{}/api/sessions/{session_id}/chat/stream", agent.hermes_url);
        let resp = self.client.post(url).headers(Self::headers(agent, "text/event-stream")).json(&body).send().await?;
        let status = resp.status().as_u16();
        if status != 200 {
            return Err(HermesError::http(status, head(&resp.text().await.unwrap_or_default(), 500)));
        }
        Ok(resp)
    }

    /// `POST /v1/runs` in a conversation: the run's id (it goes on in Hermes; follow it with `run`).
    pub async fn start_run(
        &self,
        agent: &AgentConfig,
        text: &str,
        session_id: &str,
        instructions: Option<&str>,
    ) -> Result<String, HermesError> {
        self.start_run_with(agent, text, session_id, instructions, None).await
    }

    /// [`Self::start_run`] with this run's `model_options` (e.g. its thinking level, `{"reasoning": {...}}`).
    pub async fn start_run_with(
        &self,
        agent: &AgentConfig,
        text: &str,
        session_id: &str,
        instructions: Option<&str>,
        model_options: Option<Value>,
    ) -> Result<String, HermesError> {
        self.start_run_body(agent, Value::String(text.to_string()), session_id, instructions, model_options, None).await
    }

    /// [`Self::start_run`] with an `input` as Hermes takes it: a string, or a list of messages (photos: one user message
    /// whose content is the text and image parts, threads/send.rs).
    pub async fn start_run_input(
        &self,
        agent: &AgentConfig,
        input: Value,
        session_id: &str,
        instructions: Option<&str>,
    ) -> Result<String, HermesError> {
        self.start_run_body(agent, input, session_id, instructions, None, None).await
    }

    /// [`Self::start_run`] for a turn someone other than the agent's person writes (a relative, a member of the
    /// family's agent, a relative's discussion): Hermes offers it only `tools` and none of its own tools that reach
    /// the person's data or act, and leaves the person's profile (USER.md) out of its prompt (patch 0018,
    /// `guest_turn`). A Hermes without the patch ignores the field: the bridge's own closures still hold.
    pub async fn start_guest_run(
        &self,
        agent: &AgentConfig,
        text: &str,
        session_id: &str,
        instructions: Option<&str>,
        tools: &[String],
    ) -> Result<String, HermesError> {
        self.start_run_body(agent, Value::String(text.to_string()), session_id, instructions, None, Some(tools)).await
    }

    async fn start_run_body(
        &self,
        agent: &AgentConfig,
        input: Value,
        session_id: &str,
        instructions: Option<&str>,
        model_options: Option<Value>,
        guest_tools: Option<&[String]>,
    ) -> Result<String, HermesError> {
        turn_ready(agent).await;
        let mut body = Map::new();
        if let Some(tools) = guest_tools {
            body.insert("guest_turn".into(), guest_turn(tools));
        }
        if let Some(options) = model_options {
            body.insert("model_options".into(), options);
        }
        body.insert("input".into(), input);
        body.insert("session_id".into(), Value::String(session_id.to_string()));
        if let Some(instructions) = instructions.filter(|i| !i.is_empty()) {
            // Hermes (patch 0023) tells them in a note on the run's message, once per conversation and again when
            // they change, never in its system prompt (the same prompt for all the agent's conversations: cache)
            body.insert("instructions".into(), Value::String(instructions.to_string()));
        }
        let body = Value::Object(body);
        // One key for every try: Hermes reserves it before the turn starts, and a try it already took answers its run
        // (202, `Idempotency-Replayed`) instead of starting another.
        let key = format!("bip-{}", crate::py::token_urlsafe(24));
        let url = format!("{}/v1/runs", agent.hermes_url);
        let mut attempt = 1;
        let data = loop {
            ready(agent).await;
            let sent = self
                .client
                .post(&url)
                .headers(Self::headers(agent, "application/json"))
                .header("Idempotency-Key", &key)
                .json(&body)
                .timeout(Duration::from_secs(15))
                .send()
                .await;
            let wait = match sent {
                // refused outright: Hermes is down, nothing reached it
                Err(exc) if exc.is_connect() || attempt >= START_ATTEMPTS => return Err(exc.into()),
                Err(exc) => {
                    tracing::info!(agent = %agent.name, error = httpx_error_name(&exc), "run start lost, tried again");
                    1.0
                }
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry = retry_after(status, resp.headers());
                    let queued = admission_queued(status, resp.headers());
                    let text = resp.text().await?;
                    if [200, 201, 202].contains(&status) {
                        break serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}));
                    }
                    if queued {
                        tracing::warn!(agent = %agent.name, "run accepted while its gateway restarts: not sent again");
                    }
                    match retry.filter(|_| attempt < START_ATTEMPTS) {
                        Some(wait) => wait,
                        None => return Err(HermesError::http(status, head(&text, 500))),
                    }
                }
            };
            attempt += 1;
            tokio::time::sleep(Duration::from_secs_f64(wait)).await;
        };
        match data.get("run_id") {
            Some(Value::String(id)) => Ok(id.clone()),
            _ => Err(HermesError::http(502, "no run id")),
        }
    }

    /// `POST /v1/runs/{id}/stop`: Hermes interrupts the run (its status then turns to cancelled).
    pub async fn stop_run(&self, agent: &AgentConfig, run_id: &str) -> Result<(), HermesError> {
        self.send(agent, reqwest::Method::POST, &format!("/v1/runs/{run_id}/stop"), None, 15.0).await?;
        Ok(())
    }

    /// `POST /v1/runs/{id}/steer`: words delivered to the running turn after its next tool batch (409 unless running).
    pub async fn steer_run(&self, agent: &AgentConfig, run_id: &str, text: &str) -> Result<(), HermesError> {
        let body = serde_json::json!({"input": text});
        self.send(agent, reqwest::Method::POST, &format!("/v1/runs/{run_id}/steer"), Some(body), 15.0).await?;
        Ok(())
    }

    /// The events of `GET /v1/runs/{run_id}/events` until the server closes the stream. Fails with
    /// `HermesError::Http` on a non-200 answer (before any event), with a transport error when unreachable.
    /// Timeouts as in Python: 5 s to connect, 60 s between two reads (Hermes sends keepalives every 10 s).
    pub async fn run_events(&self, agent: &AgentConfig, run_id: &str) -> Result<EventStream, HermesError> {
        ready(agent).await;
        let url = format!("{}/v1/runs/{run_id}/events", agent.hermes_url);
        let request = self.client.get(url).headers(Self::headers(agent, "text/event-stream")).send();
        let resp = match tokio::time::timeout(Duration::from_secs(65), request).await {
            Ok(r) => r?,
            Err(_) => return Err(HermesError::ReadTimeout),
        };
        let status = resp.status().as_u16();
        if status != 200 {
            let body = resp.bytes().await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            return Err(HermesError::http(status, head(&body, 500)));
        }
        let chunks = read_with_timeout(resp.bytes_stream(), Duration::from_secs(60));
        // a prompt's events with the `request_id` the bridge and the apps read (hermes_prompts.rs)
        Ok(Box::pin(iter_sse(lines_of(chunks)).map(|event| event.map(crate::hermes_prompts::named))))
    }

    /// `POST /v1/runs/{id}/approval`: Hermes' status and its JSON answer (or `{"detail": <text>}`). Hermes takes the
    /// exact prompt, `{request_id, execution_generation, choice}` and nothing else: the one `request_id` names (else the
    /// run's only approval), as the run's status shows it waiting (hermes_prompts.rs). None waiting: 409, nothing sent.
    pub async fn approve(
        &self,
        agent: &AgentConfig,
        run_id: &str,
        choice: &str,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), HermesError> {
        self.answer_prompt(agent, run_id, "approval", choice, request_id).await
    }

    /// `POST /v1/runs/{id}/approval` (`kind` approval: `{…, choice}`) or `/clarify` (`kind` clarify: `{…, answer}`, a
    /// question's words; an empty one skips it: sent as Hermes' skip, `hermes_prompts::SKIPPED`).
    pub async fn answer_prompt(
        &self,
        agent: &AgentConfig,
        run_id: &str,
        kind: &str,
        value: &str,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), HermesError> {
        let run = match self.run(agent, run_id).await {
            Ok(run) => run,
            // the run Hermes does not know (404), or refuses to show: its answer, as an answer's would be
            Err(HermesError::Http { status, body }) => {
                return Ok((status, serde_json::from_str(&body).unwrap_or_else(|_| json!({"detail": body}))));
            }
            Err(exc) => return Err(exc),
        };
        let Some((prompt, generation)) = crate::hermes_prompts::pending_identity(&run, kind, request_id) else {
            return Ok((409, json!({"detail": "nothing of this run waits for this answer"})));
        };
        let field = if kind == "clarify" { "answer" } else { "choice" };
        // Hermes reads an empty answer as no reply at all (the whole batch ends « timed out »): a skip is its sentinel
        let value = if kind == "clarify" && value.trim().is_empty() { crate::hermes_prompts::SKIPPED } else { value };
        let body = json!({"request_id": prompt, "execution_generation": generation, field: value});
        ready(agent).await;
        let resp = self
            .client
            .post(format!("{}/v1/runs/{run_id}/{kind}", agent.hermes_url))
            .json(&body)
            .headers(Self::headers(agent, "application/json"))
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        let status = resp.status().as_u16();
        let text = resp.text().await?;
        let payload = serde_json::from_str(&text).unwrap_or_else(|_| json!({"detail": head(&text, 500)}));
        Ok((status, payload))
    }
}

/// A run start is tried at most this many times (one `Idempotency-Key`: Hermes runs it once).
pub const START_ATTEMPTS: u32 = 4;
/// The longest wait between two tries, in seconds (a parked profile asks 30).
pub const START_WAIT_MAX: f64 = 10.0;
/// The error of a run Hermes lost in a restart (`resolve_unknown`).
pub const LOST_IN_RESTART: &str = "Hermes restarted during this turn";

/// Hermes' « accepted, still queued for the gateway's next owner » (`admission_queued`): a 503 marked
/// `x-should-retry: false`. The turn runs once that gateway is up; sending it again would run it twice.
pub fn admission_queued(status: u16, headers: &reqwest::header::HeaderMap) -> bool {
    status == 503
        && headers.get("x-should-retry").and_then(|v| v.to_str().ok()).is_some_and(|v| v.eq_ignore_ascii_case("false"))
}

/// The wait Hermes asks for before a request is sent again (seconds): a 503 with `Retry-After` (its store busy, the
/// profile draining or parked, a chain moved, a gateway restarting), within `[0.2, START_WAIT_MAX]`. Never for an
/// accepted turn (`admission_queued`).
pub fn retry_after(status: u16, headers: &reqwest::header::HeaderMap) -> Option<f64> {
    if status != 503 || admission_queued(status, headers) {
        return None;
    }
    let seconds = headers.get("retry-after")?.to_str().ok()?.trim().parse::<f64>().ok()?;
    Some(seconds.clamp(0.2, START_WAIT_MAX))
}

/// A body's chunks, each read within `timeout` (httpx's read timeout): a silent stream ends with `ReadTimeout`.
pub fn read_with_timeout<S>(
    chunks: S,
    timeout: Duration,
) -> impl Stream<Item = Result<bytes::Bytes, HermesError>> + Send + 'static
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    async_stream::stream! {
        futures::pin_mut!(chunks);
        loop {
            match tokio::time::timeout(timeout, chunks.next()).await {
                Ok(Some(Ok(bytes))) => yield Ok(bytes),
                Ok(Some(Err(e))) => { yield Err(HermesError::Transport(e)); return; }
                Ok(None) => return,
                Err(_) => { yield Err(HermesError::ReadTimeout); return; }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<SSEEvent> {
        let mut p = SSEParser::new();
        let mut out: Vec<SSEEvent> = splitlines(text).iter().filter_map(|l| p.feed_line(l)).collect();
        out.extend(p.finish());
        out
    }

    #[test]
    fn sse_parsing_is_lenient() {
        let events = parse("event: message.delta\ndata: {\"delta\": \"Bon\"}\n\n: keepalive\n\ndata: {\"type\": \"run.completed\", \"output\": \"Fini\"}\n\n");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].r#type, "message.delta");
        assert_eq!(events[0].text(), "Bon");
        assert_eq!(events[1].r#type, "run.completed");
        assert_eq!(events[1].final_text(), "Fini");
        let bare = parse("{\"event\": \"tool.started\", \"tool\": \"x\"}\n");
        assert_eq!(bare[0].r#type, "tool.started");
        let done = parse("data: [DONE]\n\n");
        assert_eq!(done[0].r#type, "done");
        let text = parse("event: note\ndata: plain words\n\n");
        assert_eq!(text[0].r#type, "note");
        assert_eq!(text[0].data["text"], "plain words");
        let multi = parse("data: {\"a\":\ndata: 1}\n\n");
        assert_eq!(multi[0].data["a"], 1);
        assert_eq!(multi[0].r#type, "message");
        let nested = SSEEvent::new(
            "message.delta",
            serde_json::from_str("{\"delta\": {\"content\": \"\", \"text\": \"x\"}}").unwrap(),
            "",
        );
        assert_eq!(nested.text(), "x");
        let numeric = parse("data: {\"type\": 5}\n\n");
        assert_eq!(numeric[0].r#type, "5");
        let unterminated = parse("event: run.failed\ndata: {}");
        assert_eq!(unterminated[0].r#type, "run.failed");
    }

    #[test]
    fn an_emoji_cut_in_two_utf16_halves_across_deltas_is_joined() {
        // Hermes as a router hands it over (json.dumps, ensure_ascii): 😊 = \ud83d \ude0a, one half per delta
        let events = parse(concat!(
            "data: {\"event\": \"message.delta\", \"run_id\": \"r\", \"delta\": \"\\ud83d\"}\n\n",
            "data: {\"event\": \"message.delta\", \"run_id\": \"r\", \"delta\": \"\\ude0a Bonjour\"}\n\n",
            "data: {\"event\": \"message.delta\", \"run_id\": \"r\", \"delta\": \" \\u00e9t\\u00e9 \\\\ud83d\"}\n\n",
            "data: {\"event\": \"message.delta\", \"run_id\": \"r\", \"delta\": \"\\ud83d\\ude0a\"}\n\n",
        ));
        let deltas: Vec<String> = events.iter().map(|e| e.text()).collect();
        assert!(events.iter().all(|e| e.r#type == "message.delta"), "{events:?}");
        assert_eq!(deltas, ["", "😊 Bonjour", " été \\ud83d", "😊"]);
        // what the apps get is valid JSON with the whole character
        let raw: Value = serde_json::from_str(&events[1].raw).unwrap();
        assert_eq!(raw["delta"], "😊 Bonjour");
        assert_eq!(raw["run_id"], "r");
        // an untouched event keeps Hermes' own text
        assert!(events[3].raw.contains("\\ud83d\\ude0a"));
        // a half without its partner: U+FFFD, never a lost word or an unparsed event
        let lone = parse(concat!(
            "data: {\"event\": \"message.delta\", \"delta\": \"a\\ud83d\"}\n\n",
            "data: {\"event\": \"message.delta\", \"delta\": \"b\"}\n\n",
            "data: {\"event\": \"message.delta\", \"delta\": \"\\ude0ac\"}\n\n",
            "data: {\"event\": \"run.completed\", \"output\": \"x\\ud83d\"}\n\n",
        ));
        let texts: Vec<String> = lone.iter().map(|e| e.text()).collect();
        assert_eq!(texts[..3], ["a", "\u{FFFD}b", "\u{FFFD}c"]);
        assert_eq!(lone[3].r#type, "run.completed");
        assert_eq!(lone[3].final_text(), "x\u{FFFD}");
    }

    #[tokio::test]
    async fn a_line_after_an_emoji_cut_across_chunks_comes_as_soon_as_it_is_whole() {
        // 😊's four bytes in two network reads: the line is given at the second read, not held for a later one
        let line = "data: {\"delta\": \"😊\"}\n\n".as_bytes().to_vec();
        let cut = line.iter().position(|b| *b == 0xF0).unwrap() + 2;
        let (tx, rx) = futures::channel::mpsc::unbounded::<Result<bytes::Bytes, std::io::Error>>();
        let lines = lines_of(rx);
        futures::pin_mut!(lines);
        tx.unbounded_send(Ok(bytes::Bytes::from(line[..cut].to_vec()))).unwrap();
        tx.unbounded_send(Ok(bytes::Bytes::from(line[cut..].to_vec()))).unwrap();
        let first = tokio::time::timeout(Duration::from_millis(200), lines.next()).await.expect("held back");
        assert_eq!(first.unwrap().unwrap(), "data: {\"delta\": \"😊\"}");
        let blank = tokio::time::timeout(Duration::from_millis(200), lines.next()).await.expect("held back");
        assert_eq!(blank.unwrap().unwrap(), "");
    }

    #[test]
    fn line_decoder_like_httpx() {
        let mut d = LineDecoder::default();
        assert_eq!(d.decode("a\r"), Vec::<String>::new());
        assert_eq!(d.decode("\nb\n\nc"), vec!["a", "b", ""]);
        assert_eq!(d.decode("d\u{2028}e"), vec!["cd"]);
        assert_eq!(d.flush(), vec!["e"]);
        let mut d = LineDecoder::default();
        assert_eq!(d.decode("x\r"), Vec::<String>::new());
        assert_eq!(d.decode("y"), vec!["x"]);
        assert_eq!(d.flush(), vec!["y"]);
    }

    #[test]
    fn utf8_across_chunks() {
        let bytes = "é😀".as_bytes();
        let mut d = Utf8Decoder::default();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&d.decode(&[*b], false));
        }
        out.push_str(&d.decode(&[], true));
        assert_eq!(out, "é😀");
        let mut d = Utf8Decoder::default();
        assert_eq!(d.decode(&[0x61, 0xff, 0x62], false), "a\u{fffd}b");
    }
}
