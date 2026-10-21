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
/// Events after which buffered text is a complete thought worth speaking right away.
pub const FLUSH_TYPES: [&str; 4] = ["tool.started", "approval.request", "assistant.completed", "message.completed"];

pub fn is_delta(event_type: &str) -> bool {
    DELTA_TYPES.contains(&event_type)
}

pub fn is_terminal(event_type: &str) -> bool {
    TERMINAL_TYPES.contains(&event_type)
}

pub fn is_flush(event_type: &str) -> bool {
    FLUSH_TYPES.contains(&event_type)
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
#[derive(Debug, Default)]
pub struct SSEParser {
    event_name: Option<String>,
    data: Vec<String>,
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
            return Self::build(None, stripped); // bare JSON line (NDJSON-style), tolerated
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
        Self::build(name.as_deref(), &payload)
    }

    fn build(name: Option<&str>, payload: &str) -> Option<SSEEvent> {
        if payload.trim() == "[DONE]" {
            return Some(SSEEvent::new("done", Map::new(), payload));
        }
        let mut data = Map::new();
        if !payload.trim().is_empty() {
            match serde_json::from_str::<Value>(payload) {
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
        Some(SSEEvent::new(etype, data, payload))
    }
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

    /// `GET /v1/runs/{id}`: status, session, and the pending `approval` of a run parked on one.
    pub async fn run(&self, agent: &AgentConfig, run_id: &str) -> Result<Map<String, Value>, HermesError> {
        let data = self.get_json(agent, &format!("/v1/runs/{run_id}"), &[]).await?;
        Ok(match data {
            Value::Object(m) => m,
            _ => Map::new(),
        })
    }

    /// `GET /v1/capabilities`: answers when the agent's gateway is up.
    pub async fn capabilities(&self, agent: &AgentConfig) -> Result<Value, HermesError> {
        self.get_json(agent, "/v1/capabilities", &[]).await
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

    /// `GET /api/sessions` (most recent first).
    pub async fn list_sessions(&self, agent: &AgentConfig, limit: i64) -> Result<Vec<Map<String, Value>>, HermesError> {
        let data = self.get_json(agent, "/api/sessions", &[("limit", limit.to_string())]).await?;
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

    async fn send(
        &self,
        agent: &AgentConfig,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        timeout: f64,
    ) -> Result<Value, HermesError> {
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

    /// `POST /v1/runs` in a conversation: the run's id (it goes on in Hermes; follow it with `run`).
    pub async fn start_run(
        &self,
        agent: &AgentConfig,
        text: &str,
        session_id: &str,
        instructions: Option<&str>,
    ) -> Result<String, HermesError> {
        let mut body = Map::new();
        body.insert("input".into(), Value::String(text.to_string()));
        body.insert("session_id".into(), Value::String(session_id.to_string()));
        if let Some(instructions) = instructions.filter(|i| !i.is_empty()) {
            // Hermes adds them to its system prompt for this run only
            body.insert("instructions".into(), Value::String(instructions.to_string()));
        }
        let data = self.send(agent, reqwest::Method::POST, "/v1/runs", Some(Value::Object(body)), 15.0).await?;
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

    /// The events of `GET /v1/runs/{run_id}/events` until the server closes the stream. Fails with
    /// `HermesError::Http` on a non-200 answer (before any event), with a transport error when unreachable.
    /// Timeouts as in Python: 5 s to connect, 60 s between two reads (Hermes sends keepalives every 10 s).
    pub async fn run_events(&self, agent: &AgentConfig, run_id: &str) -> Result<EventStream, HermesError> {
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
        Ok(Box::pin(iter_sse(lines_of(chunks))))
    }

    /// `POST /v1/runs/{id}/approval`: Hermes' status and its JSON answer (or `{"detail": <text>}`).
    pub async fn approve(
        &self,
        agent: &AgentConfig,
        run_id: &str,
        choice: &str,
        request_id: Option<&str>,
    ) -> Result<(u16, Value), HermesError> {
        let mut body = Map::new();
        body.insert("choice".into(), Value::String(choice.to_string()));
        if let Some(rid) = request_id.filter(|r| !r.is_empty()) {
            body.insert("request_id".into(), Value::String(rid.to_string()));
        }
        let resp = self
            .client
            .post(format!("{}/v1/runs/{run_id}/approval", agent.hermes_url))
            .json(&Value::Object(body))
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
