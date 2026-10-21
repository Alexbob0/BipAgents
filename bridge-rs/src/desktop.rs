//! The agents' desktop, live in the app: watch what the agent does in its browser, take over with a tap.
//!
//! Hermes runs a desktop per agent (« Bot Desktop »: Xvnc and Xfce, the agent's visible Chromium) and serves it as
//! RFB on a private Unix socket, `<agent's HERMES_HOME>/bot-desktop/rfb.sock` (or inside its sandbox, reached with
//! `docker exec`). The bridge relays it to the app over a WebSocket opened with a short-lived ticket, and shows it
//! with noVNC (static/novnc, MPL-2.0).
//!
//! Watching sends no input: the bridge drops keyboard, pointer, clipboard and resize messages unless the person
//! took over. « Prendre la main » writes Hermes' own lease (`bot-desktop/lease.json`, holder « human »), which
//! stops the agent's browser and computer-use tools until the person hands back.
//!
//! An agent whose browser runs on a VNC server set up by hand (like Vie's stealth Chrome) gives its address
//! (`desktop_vnc`: `ws://127.0.0.1:6080/websockify` for a websockify, or `tcp://127.0.0.1:5901`) and its VNC
//! password file (`desktop_vnc_password_file`): the bridge logs in itself and shows it the same way, so the
//! password never reaches the phone and the VNC port need not be published. Taking over there only lets input
//! through: such a browser has no Hermes lease to pause. (`desktop_url` still opens a viewer page as it is.)
//!
//! Port of `bridge/bipbridge/desktop.py` (owner group: browsers-places).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

pub const VIEWER_ID: &str = "bipagents-app";
pub const TICKET_SECONDS: f64 = 600.0;
/// The fixed key vncpasswd files are obscured with.
pub const VNCPASSWD_KEY: [u8; 8] = [23, 82, 107, 6, 35, 78, 88, 7];

/// What runs in a sandbox to reach the desktop's socket (Python, the sandbox has it).
pub const RELAY: &str = "import os,socket,sys,threading\n\
s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1])\n\
def up():\n\
\x20 while True:\n\
\x20   d=os.read(0,65536)\n\
\x20   if not d: break\n\
\x20   s.sendall(d)\n\
\x20 s.shutdown(socket.SHUT_WR)\n\
threading.Thread(target=up,daemon=True).start()\n\
while True:\n\
\x20 d=s.recv(65536)\n\
\x20 if not d: break\n\
\x20 os.write(1,d)\n";

/// What went wrong reaching a desktop: Python's `OSError` / `ConnectionError` / `TimeoutError` (tried again while a
/// browser wakes) or its `ValueError` (not).
#[derive(Debug, Clone, thiserror::Error)]
pub enum DesktopError {
    #[error("{0}")]
    Os(String),
    #[error("{0}")]
    Value(String),
}

fn os_error(err: std::io::Error) -> DesktopError {
    DesktopError::Os(err.to_string())
}

// -- input gate --------------------------------------------------------------------------------------

/// Client → server RFB (security « None »): passes the handshake, forces the shared flag, frames every
/// message and drops input (key, pointer, clipboard, resize, QEMU key) unless `allow_input()` says yes.
pub struct InputGate {
    pub allow_input: Box<dyn Fn() -> bool + Send + Sync>,
    pub buffer: Vec<u8>,
    /// ProtocolVersion (12) + security type (1) + ClientInit (1)
    pub handshake: usize,
}

pub const INPUT: [u8; 5] = [4, 5, 6, 251, 255];

impl InputGate {
    pub fn new(allow_input: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        InputGate { allow_input: Box::new(allow_input), buffer: Vec::new(), handshake: 14 }
    }

    fn length(&self) -> Result<Option<usize>, String> {
        let b = &self.buffer;
        let kind = b[0];
        let fixed = match kind {
            0 => Some(20),
            3 => Some(10),
            4 => Some(8),
            5 => Some(6),
            150 => Some(10),
            _ => None,
        };
        if let Some(n) = fixed {
            return Ok(Some(n));
        }
        match kind {
            2 => Ok(if b.len() < 4 { None } else { Some(4 + 4 * u16::from_be_bytes([b[2], b[3]]) as usize) }),
            6 => {
                if b.len() < 8 {
                    return Ok(None);
                }
                let size = i32::from_be_bytes([b[4], b[5], b[6], b[7]]).unsigned_abs() as usize;
                if size > 256 * 1024 {
                    return Err("clipboard too large".into());
                }
                Ok(Some(8 + size))
            }
            248 => Ok(if b.len() < 9 { None } else { Some(9 + b[8] as usize) }),
            251 => Ok(if b.len() < 7 { None } else { Some(8 + 16 * b[6] as usize) }),
            255 => Ok(Some(12)),
            other => Err(format!("unknown RFB message {other}")),
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.buffer.extend_from_slice(chunk);
        if self.handshake > 0 {
            let n = self.handshake.min(self.buffer.len());
            let mut take: Vec<u8> = self.buffer.drain(..n).collect();
            self.handshake -= take.len();
            if self.handshake == 0 && !take.is_empty() {
                // ClientInit: always shared, never kick the others off
                let last = take.len() - 1;
                take[last] = 1;
            }
            out.extend(take);
        }
        while !self.buffer.is_empty() {
            let length = self.length()?;
            let Some(length) = length.filter(|&l| self.buffer.len() >= l) else { break };
            let message: Vec<u8> = self.buffer.drain(..length).collect();
            if !INPUT.contains(&message[0]) || (self.allow_input)() {
                out.extend(message);
            }
        }
        Ok(out)
    }
}

// -- Hermes' lease -----------------------------------------------------------------------------------

pub fn desktop_dir(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    let root = match agent.space.as_deref().filter(|s| !s.is_empty()) {
        Some(space) => Some(space.to_string()),
        None if config.hermes.enabled() && agent.hermes_url.starts_with(&format!("{}/p/", config.hermes.hub_url)) => {
            Some(config.hermes.home.clone())
        }
        None => None,
    };
    root.filter(|r| !r.is_empty()).map(|r| PathBuf::from(r).join("profiles").join(&agent.name).join("bot-desktop"))
}

pub fn read_lease(folder: &Path) -> Dict {
    let fallback = |holder: &str| {
        let mut d = Dict::new();
        d.insert("holder".into(), json!(holder));
        d.insert("viewer_id".into(), Value::Null);
        d.insert("since".into(), json!(crate::py::time_time()));
        d.insert("reason".into(), json!(""));
        d.insert("epoch".into(), json!(0));
        d
    };
    match std::fs::read(folder.join("lease.json")) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fallback("agent"),
        Err(_) => fallback("human"), // fail safe
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => fallback("human"),
        },
    }
}

/// The same file, lock and fields as Hermes' own lease (tools/bot_desktop/lease.py).
pub fn set_lease(folder: &Path, human: bool, reason: &str) -> anyhow::Result<Dict> {
    use std::os::unix::io::AsRawFd;
    crate::py::makedirs(&folder.to_string_lossy(), 0o700)?;
    let lock = std::fs::OpenOptions::new().create(true).append(true).read(true).open(folder.join("lease.lock"))?;
    // SAFETY: flock on a file descriptor this function owns; released when `lock` closes.
    unsafe {
        libc::flock(lock.as_raw_fd(), libc::LOCK_EX);
    }
    let mut lease = read_lease(folder);
    let holder = if human { "human" } else { "agent" };
    if lease.get("holder").and_then(Value::as_str) != Some(holder)
        || (human && lease.get("viewer_id").and_then(Value::as_str) != Some(VIEWER_ID))
    {
        let epoch = match lease.get("epoch") {
            None => 0,
            Some(v) => crate::py::py_int(v).map_err(|e| anyhow::anyhow!(e))?,
        };
        lease.insert("holder".into(), json!(holder));
        lease.insert("viewer_id".into(), if human { json!(VIEWER_ID) } else { Value::Null });
        lease.insert("since".into(), json!(crate::py::time_time()));
        lease.insert("reason".into(), json!(if human { reason } else { "" }));
        lease.insert("epoch".into(), json!(epoch + 1));
        let tmp = folder.join("lease.json.tmp");
        crate::py::write_with_mode(
            &tmp.to_string_lossy(),
            crate::py::dumps(&Value::Object(lease.clone())).as_bytes(),
            0o600,
        )?;
        std::fs::rename(&tmp, folder.join("lease.json"))?;
    }
    drop(lock);
    Ok(lease)
}

pub fn app_holds(folder: &Path) -> bool {
    let lease = read_lease(folder);
    lease.get("holder").and_then(Value::as_str) == Some("human")
        && lease.get("viewer_id").and_then(Value::as_str) == Some(VIEWER_ID)
}

// -- the RFB stream ------------------------------------------------------------------------------------

type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;
type WsClient = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsMessage = tokio_tungstenite::tungstenite::Message;

/// The desktop's side, read half: a stream (Unix socket, TCP, docker exec) or a websockify WebSocket.
pub enum UpReader {
    Stream(BoxRead),
    Ws(SplitStream<WsClient>),
}

/// The desktop's side, write half.
pub enum UpWriter {
    Stream(BoxWrite),
    Ws(SplitSink<WsClient, WsMessage>),
}

/// Bytes to and from a desktop: a stream (Unix socket, TCP, docker exec) or a websockify WebSocket.
pub struct Upstream {
    reader: UpReader,
    writer: UpWriter,
    buffer: Vec<u8>,
    child: Option<tokio::process::Child>,
}

impl Upstream {
    pub fn stream(reader: BoxRead, writer: BoxWrite) -> Upstream {
        Upstream { reader: UpReader::Stream(reader), writer: UpWriter::Stream(writer), buffer: Vec::new(), child: None }
    }

    pub fn websocket(socket: WsClient) -> Upstream {
        let (sink, stream) = socket.split();
        Upstream { reader: UpReader::Ws(stream), writer: UpWriter::Ws(sink), buffer: Vec::new(), child: None }
    }

    /// What comes next (empty: the desktop closed).
    pub async fn read(&mut self) -> Vec<u8> {
        if !self.buffer.is_empty() {
            return std::mem::take(&mut self.buffer);
        }
        read_raw(&mut self.reader).await.unwrap_or_default()
    }

    pub async fn readexactly(&mut self, n: usize) -> Result<Vec<u8>, DesktopError> {
        while self.buffer.len() < n {
            let chunk = read_raw(&mut self.reader).await.unwrap_or_default();
            if chunk.is_empty() {
                return Err(DesktopError::Os("desktop closed the connection".into()));
            }
            self.buffer.extend(chunk);
        }
        Ok(self.buffer.drain(..n).collect())
    }

    pub async fn write(&mut self, data: &[u8]) -> Result<(), DesktopError> {
        write_raw(&mut self.writer, data).await
    }

    pub async fn close(mut self) {
        close_writer(&mut self.writer, &mut self.child).await;
    }
}

async fn read_raw(reader: &mut UpReader) -> Result<Vec<u8>, DesktopError> {
    match reader {
        UpReader::Stream(r) => {
            let mut buf = vec![0u8; 65536];
            let n = r.read(&mut buf).await.map_err(os_error)?;
            buf.truncate(n);
            Ok(buf)
        }
        UpReader::Ws(stream) => loop {
            match stream.next().await {
                None | Some(Err(_)) => return Ok(Vec::new()),
                Some(Ok(WsMessage::Binary(b))) => return Ok(b.to_vec()),
                Some(Ok(WsMessage::Text(t))) => return Ok(t.as_bytes().to_vec()),
                Some(Ok(WsMessage::Close(_))) => return Ok(Vec::new()),
                Some(Ok(_)) => continue,
            }
        },
    }
}

async fn write_raw(writer: &mut UpWriter, data: &[u8]) -> Result<(), DesktopError> {
    match writer {
        UpWriter::Stream(w) => {
            w.write_all(data).await.map_err(os_error)?;
            w.flush().await.map_err(os_error)
        }
        UpWriter::Ws(sink) => sink
            .send(WsMessage::Binary(Bytes::copy_from_slice(data)))
            .await
            .map_err(|e| DesktopError::Os(e.to_string())),
    }
}

async fn close_writer(writer: &mut UpWriter, child: &mut Option<tokio::process::Child>) {
    match writer {
        UpWriter::Stream(w) => {
            let _ = w.shutdown().await;
        }
        UpWriter::Ws(sink) => {
            let _ = sink.close().await;
        }
    }
    if let Some(mut child) = child.take() {
        let _ = child.start_kill();
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
}

/// (reader, writer, close) on the desktop: its socket on this host, or inside its sandbox.
pub async fn open_rfb(folder: &Path) -> Result<Upstream, DesktopError> {
    let marker = folder.join("sandbox.json");
    if marker.exists() {
        let text = std::fs::read_to_string(&marker).map_err(os_error)?;
        let info: Value = serde_json::from_str(&text).map_err(|e| DesktopError::Value(e.to_string()))?;
        let container = info.get("container").filter(|c| crate::py::truthy(c)).map(crate::py::py_str);
        let Some(container) = container else {
            return Err(DesktopError::Os("this agent's desktop runs in a sandbox the bridge cannot reach".into()));
        };
        let docker = info
            .get("docker")
            .filter(|d| crate::py::truthy(d))
            .map(crate::py::py_str)
            .unwrap_or_else(|| "docker".into());
        let dir = info.get("dir").map(crate::py::py_str).ok_or_else(|| DesktopError::Value("'dir'".into()))?;
        let mut child = tokio::process::Command::new(&docker)
            .args(["exec", "-i", "-u", "pn", &container, "python3", "-c", RELAY, &format!("{dir}/rfb.sock")])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(os_error)?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(DesktopError::Os("docker exec without its pipes".into()));
        };
        let mut up = Upstream::stream(Box::new(stdout), Box::new(stdin));
        up.child = Some(child);
        return Ok(up);
    }
    let sock = folder.join("rfb.sock");
    if !sock.exists() {
        return Err(DesktopError::Os("the desktop is not running".into()));
    }
    let stream = tokio::net::UnixStream::connect(&sock).await.map_err(os_error)?;
    let (r, w) = stream.into_split();
    Ok(Upstream::stream(Box::new(r), Box::new(w)))
}

// -- VNC set up by hand: the bridge logs in ------------------------------------------------------------

fn reverse_bits(data: &[u8]) -> Vec<u8> {
    data.iter().map(|b| b.reverse_bits()).collect()
}

/// DES in ECB mode (Python: TripleDES with the key three times).
fn des(key: &[u8], data: &[u8], decrypt: bool) -> Result<Vec<u8>, DesktopError> {
    use des::cipher::generic_array::GenericArray;
    use des::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
    if !data.len().is_multiple_of(8) {
        return Err(DesktopError::Value(
            "The length of the provided data is not a multiple of the block length.".into(),
        ));
    }
    let cipher = des::Des::new_from_slice(key).map_err(|e| DesktopError::Value(e.to_string()))?;
    let mut out = data.to_vec();
    for block in out.chunks_mut(8) {
        let block = GenericArray::from_mut_slice(block);
        if decrypt {
            cipher.decrypt_block(block);
        } else {
            cipher.encrypt_block(block);
        }
    }
    Ok(out)
}

/// The password in clear, or a `vncpasswd` file (8 bytes obscured with a fixed, public key: not a protection,
/// this file must stay out of the agents' reach like any other key).
pub fn read_vnc_password(path: &str) -> Result<String, DesktopError> {
    let raw = std::fs::read(path).map_err(os_error)?;
    if let Ok(text) = std::str::from_utf8(&raw) {
        let text = crate::browsers::pyx::strip(text);
        if !text.is_empty() && crate::browsers::pyx::isprintable(text) {
            return Ok(text.to_string());
        }
    }
    let plain = des(&reverse_bits(&VNCPASSWD_KEY), &raw[..raw.len().min(8)], true)?;
    let end = plain.iter().position(|&b| b == 0).unwrap_or(plain.len());
    Ok(plain[..end].iter().map(|&b| b as char).collect())
}

/// RFB « VNC Authentication »: the challenge encrypted with DES, keyed by the password's first 8 bytes with each
/// byte's bits reversed (the protocol's quirk).
pub fn vnc_response(password: &str, challenge: &[u8]) -> Vec<u8> {
    let mut key: Vec<u8> =
        password.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).take(8).collect();
    key.resize(8, 0);
    des(&reverse_bits(&key), challenge, false).unwrap_or_default()
}

/// The client side of an RFB 3.8 handshake with the desktop's server, up to (not including) ClientInit.
pub async fn vnc_login(up: &mut Upstream, password: Option<&str>) -> Result<(), DesktopError> {
    let version = up.readexactly(12).await?;
    let minor = std::str::from_utf8(&version[8..11]).ok().and_then(|s| s.trim().parse::<i64>().ok());
    if !version.starts_with(b"RFB 003.") || minor.map(|m| m < 7).unwrap_or(true) {
        if version.starts_with(b"RFB 003.") && minor.is_none() {
            return Err(DesktopError::Value(format!(
                "invalid literal for int() with base 10: {}",
                crate::py::py_repr(&json!(String::from_utf8_lossy(&version[8..11])))
            )));
        }
        return Err(DesktopError::Os(format!("unsupported VNC server {}", bytes_repr(&version))));
    }
    up.write(b"RFB 003.008\n").await?;
    let count = up.readexactly(1).await?[0];
    if count == 0 {
        let size = u32::from_be_bytes(up.readexactly(4).await?.try_into().unwrap_or([0; 4]));
        let reason = up.readexactly(size as usize).await?;
        return Err(DesktopError::Os(String::from_utf8_lossy(&reason).into_owned()));
    }
    let offered = up.readexactly(count as usize).await?;
    match password.filter(|p| !p.is_empty()) {
        Some(password) if offered.contains(&2) => {
            up.write(b"\x02").await?;
            let challenge = up.readexactly(16).await?;
            up.write(&vnc_response(password, &challenge)).await?;
        }
        _ if offered.contains(&1) => up.write(b"\x01").await?,
        _ => return Err(DesktopError::Os("the VNC server wants a password the bridge does not have".into())),
    }
    let result = up.readexactly(4).await?;
    if u32::from_be_bytes(result.try_into().unwrap_or([0; 4])) != 0 {
        return Err(DesktopError::Os("VNC password refused".into()));
    }
    Ok(())
}

/// Python's `repr(bytes)`.
fn bytes_repr(data: &[u8]) -> String {
    let quote = if data.contains(&b'\'') && !data.contains(&b'"') { '"' } else { '\'' };
    let mut out = format!("b{quote}");
    for &b in data {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if b as char == quote => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(b as char),
            b => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote);
    out
}

pub async fn open_vnc(agent: &AgentConfig) -> Result<Upstream, DesktopError> {
    let url = agent.desktop_vnc.clone().unwrap_or_default();
    let password = match agent.desktop_vnc_password_file.as_deref().filter(|f| !f.is_empty()) {
        Some(file) => Some(read_vnc_password(file)?),
        None => None,
    };
    let mut up = if url.starts_with("ws://") || url.starts_with("wss://") {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = url.as_str().into_client_request().map_err(|e| DesktopError::Value(e.to_string()))?;
        request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static("binary"));
        let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        config.max_message_size = None;
        config.max_frame_size = None;
        let connecting = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (socket, _) = tokio::time::timeout(Duration::from_secs(10), connecting)
            .await
            .map_err(|_| DesktopError::Os("timed out during opening handshake".into()))?
            .map_err(|e| match e {
                tokio_tungstenite::tungstenite::Error::Io(io) => os_error(io),
                other => DesktopError::Os(other.to_string()),
            })?;
        Upstream::websocket(socket)
    } else if let Some(rest) = url.strip_prefix("tcp://") {
        let (host, port) = rest.rsplit_once(':').unwrap_or(("", rest));
        let port: u16 = port.trim().parse().map_err(|_| {
            DesktopError::Value(format!("invalid literal for int() with base 10: {}", crate::py::py_repr(&json!(port))))
        })?;
        let stream = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect((host, port)))
            .await
            .map_err(|_| DesktopError::Os("timed out".into()))?
            .map_err(os_error)?;
        let (r, w) = stream.into_split();
        Upstream::stream(Box::new(r), Box::new(w))
    } else {
        return Err(DesktopError::Value("desktop_vnc must be ws://… or tcp://host:port".into()));
    };
    if let Err(exc) = vnc_login(&mut up, password.as_deref()).await {
        up.close().await;
        return Err(exc);
    }
    Ok(up)
}

/// open_vnc, again for `seconds` while it fails: a browser just woken has its VNC server up a moment after its
/// screen.
pub async fn open_vnc_retrying(agent: &AgentConfig, seconds: f64) -> Result<Upstream, DesktopError> {
    let deadline = crate::py::monotonic() + seconds;
    loop {
        match open_vnc(agent).await {
            Ok(up) => return Ok(up),
            Err(DesktopError::Os(e)) => {
                if crate::py::monotonic() >= deadline {
                    return Err(DesktopError::Os(e));
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(other) => return Err(other),
        }
    }
}

// -- the app's side --------------------------------------------------------------------------------------

/// The app's side (the WebSocket), read in exact sizes for the handshake the bridge answers itself.
struct Downstream {
    stream: SplitStream<WebSocket>,
    buffer: Vec<u8>,
}

impl Downstream {
    /// The next message's bytes; None: the app went away.
    async fn receive(&mut self) -> Option<Vec<u8>> {
        loop {
            match self.stream.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return None,
                Some(Ok(Message::Binary(b))) => return Some(b.to_vec()),
                Some(Ok(Message::Text(t))) => return Some(t.as_bytes().to_vec()),
                Some(Ok(_)) => continue,
            }
        }
    }

    async fn readexactly(&mut self, n: usize) -> Option<Vec<u8>> {
        while self.buffer.len() < n {
            let chunk = self.receive().await?;
            self.buffer.extend(chunk);
        }
        Some(self.buffer.drain(..n).collect())
    }
}

/// To the app: no password (it passed Face ID and holds a ticket). Its ClientInit goes on to the server, shared.
async fn serve_handshake(
    app_side: &mut Downstream,
    sink: &mut SplitSink<WebSocket, Message>,
    up: &mut Upstream,
) -> Result<(), ()> {
    let send = |data: &'static [u8]| Message::Binary(Bytes::from_static(data));
    sink.send(send(b"RFB 003.008\n")).await.map_err(|_| ())?;
    app_side.readexactly(12).await.ok_or(())?;
    sink.send(send(b"\x01\x01")).await.map_err(|_| ())?; // one security type: None
    app_side.readexactly(1).await.ok_or(())?;
    sink.send(send(b"\x00\x00\x00\x00")).await.map_err(|_| ())?; // SecurityResult: OK
    app_side.readexactly(1).await.ok_or(())?; // ClientInit
    up.write(b"\x01").await.map_err(|_| ())?; // shared: never kick another viewer off
    Ok(())
}

// -- routes ------------------------------------------------------------------------------------------

/// Short-lived tickets for the desktop page and its stream (ticket → (agent, expiry)).
#[derive(Default)]
pub struct Tickets {
    pub items: Mutex<HashMap<String, (String, f64)>>,
}

impl Tickets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Python's `Tickets.new(agent)`: a ticket for the agent's desktop.
    pub fn issue(&self, agent: &str) -> String {
        let now = crate::py::time_time();
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        items.retain(|_, v| v.1 > now);
        let ticket = crate::py::token_urlsafe(24);
        items.insert(ticket.clone(), (agent.to_string(), now + TICKET_SECONDS));
        ticket
    }

    pub fn agent(&self, ticket: &str) -> Option<String> {
        let items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        items.get(ticket).filter(|v| v.1 > crate::py::time_time()).map(|v| v.0.clone())
    }
}

/// The viewer page (`assets/desktop.html`, once desktop.py's `PAGE`).
pub static PAGE: LazyLock<String> = LazyLock::new(|| include_str!("../assets/desktop.html").to_string());

fn control_of(state: &AppState, agent: &str) -> Option<bool> {
    state.desktop_control.lock().unwrap_or_else(|e| e.into_inner()).get(agent).copied()
}

fn has_vnc(agent: &AgentConfig) -> bool {
    agent.desktop_vnc.as_deref().map(|v| !v.is_empty()).unwrap_or(false)
}

async fn page(State(state): State<AppState>, UrlPath(ticket): UrlPath<String>) -> ApiResult<Response> {
    let Some(name) = state.desktop_tickets.agent(&ticket) else {
        return Err(HttpError::new(404, "expired"));
    };
    let config = state.config();
    let agent = config.agent(Some(&name));
    let folder = agent.and_then(|a| desktop_dir(&config, a));
    let holds = match agent {
        Some(a) if has_vnc(a) => control_of(&state, &name).unwrap_or(false),
        _ => folder.as_deref().map(app_holds).unwrap_or(false),
    };
    let view_only = if holds { "false" } else { "true" };
    let html = PAGE.replace("__VIEW_ONLY__", view_only);
    let mut response = Response::new(axum::body::Body::from(html));
    let h = response.headers_mut();
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert("content-type", HeaderValue::from_static("text/html; charset=utf-8"));
    Ok(response)
}

/// Closing before accepting: uvicorn answers the upgrade with HTTP 403.
fn refused() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

async fn stream(
    State(state): State<AppState>,
    UrlPath(ticket): UrlPath<String>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    // Not a WebSocket: Starlette has no HTTP route there.
    let Ok(ws) = ws else { return HttpError::new(404, "Not Found").into_response() };
    let config = state.config();
    let name = state.desktop_tickets.agent(&ticket);
    let Some(agent) = name.as_deref().and_then(|n| config.agent(Some(n))).cloned() else { return refused() };
    let vnc = has_vnc(&agent);
    let folder = if vnc { None } else { desktop_dir(&config, &agent) };
    if folder.is_none() && !vnc {
        return refused();
    }
    let own_browser = vnc && crate::browsers::wake(&config, &agent.name, 60.0).await;
    let opened = if vnc {
        open_vnc_retrying(&agent, if own_browser { 10.0 } else { 0.0 }).await
    } else {
        open_rfb(folder.as_deref().unwrap_or(Path::new(""))).await
    };
    let up = match opened {
        Ok(up) => up,
        Err(exc) => {
            let error: String = exc.to_string().chars().take(200).collect();
            tracing::info!(agent = ?crate::logs::J(&name), error = %error, "desktop unavailable");
            return refused();
        }
    };
    ws.protocols(["binary"])
        .max_message_size(16 * 1024 * 1024)
        .max_frame_size(16 * 1024 * 1024)
        .on_upgrade(move |socket| session(state, agent, name, folder, own_browser, up, socket))
}

async fn session(
    state: AppState,
    agent: AgentConfig,
    name: Option<String>,
    folder: Option<PathBuf>,
    own_browser: bool,
    mut up: Upstream,
    socket: WebSocket,
) {
    let (mut sink, stream) = socket.split();
    let mut app_side = Downstream { stream, buffer: Vec::new() };
    let mut gate = if has_vnc(&agent) {
        if serve_handshake(&mut app_side, &mut sink, &mut up).await.is_err() {
            up.close().await;
            return;
        }
        let (s, n) = (state.clone(), agent.name.clone());
        let mut gate = InputGate::new(move || control_of(&s, &n).unwrap_or(false));
        gate.handshake = 0;
        gate
    } else {
        let f = folder.clone().unwrap_or_default();
        InputGate::new(move || app_holds(&f))
    };
    tracing::info!(agent = ?crate::logs::J(&name), "desktop viewer connected");
    let Upstream { reader, mut writer, buffer, mut child } = up;
    let mut reader = reader;
    let config = state.config();
    let down = async {
        let mut pending = buffer;
        let mut last_touch = 0.0f64;
        loop {
            let data = if pending.is_empty() {
                read_raw(&mut reader).await.unwrap_or_default()
            } else {
                std::mem::take(&mut pending)
            };
            if data.is_empty() {
                break;
            }
            if sink.send(Message::Binary(Bytes::from(data))).await.is_err() {
                break;
            }
            let now = crate::py::time_time();
            if now - last_touch > 60.0 {
                // watched: Hermes keeps the desktop up, the browser stays on
                last_touch = now;
                if let Some(folder) = &folder {
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(folder.join("activity"))
                        .and_then(|f| f.set_modified(std::time::SystemTime::now()));
                } else if own_browser {
                    crate::browsers::busy(&config, &agent.name);
                }
            }
        }
    };
    let from_app = async {
        if !app_side.buffer.is_empty() {
            let pending = std::mem::take(&mut app_side.buffer);
            match gate.feed(&pending) {
                Ok(out) if !out.is_empty() => {
                    if write_raw(&mut writer, &out).await.is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
        while let Some(message) = app_side.receive().await {
            let Ok(out) = gate.feed(&message) else { return };
            if !out.is_empty() && write_raw(&mut writer, &out).await.is_err() {
                return;
            }
        }
    };
    tokio::select! {
        _ = down => {}
        _ = from_app => {}
    }
    close_writer(&mut writer, &mut child).await;
    let _ = sink.close().await;
}

/// The page and its stream (a short-lived ticket from the admin routes). Mounted at the root.
pub fn desktop_router() -> Router<AppState> {
    Router::new().route("/desktop/{ticket}/", get(page)).route("/desktop/{ticket}/ws", get(stream))
}

async fn open_desktop(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    if has_vnc(&agent) {
        let ticket = state.desktop_tickets.issue(&agent.name);
        // Its browser stopped while unused: started now, while the app opens the viewer.
        let (c, n) = (config.clone(), agent.name.clone());
        tokio::spawn(async move {
            crate::browsers::wake(&c, &n, 60.0).await;
        });
        // Its own browser container (browsers.py): taking over makes its tools wait. Set up by hand: it does not.
        let control = control_of(&state, &agent.name);
        return Ok(web::ok(json!({
            "kind": "novnc", "path": format!("desktop/{ticket}/"), "you_hold": control.unwrap_or(false),
            "holder": if control.unwrap_or(false) { "human" } else { "agent" }, "reason": null,
            "pauses_agent": crate::browsers::ports_of(&config, &agent.name).is_some(),
        })));
    }
    if let Some(url) = agent.desktop_url.as_deref().filter(|u| !u.is_empty()) {
        return Ok(web::ok(json!({"kind": "page", "url": url})));
    }
    let folder = desktop_dir(&config, &agent);
    let Some(folder) = folder.filter(|f| f.join("rfb.sock").exists() || f.join("sandbox.json").exists()) else {
        return Err(HttpError::json(409, json!({"code": "no_desktop"})));
    };
    let ticket = state.desktop_tickets.issue(&agent.name);
    let lease = read_lease(&folder);
    let reason = lease.get("reason").filter(|r| crate::py::truthy(r)).cloned().unwrap_or(Value::Null);
    Ok(web::ok(json!({
        "kind": "novnc", "path": format!("desktop/{ticket}/"), "you_hold": app_holds(&folder),
        "holder": lease.get("holder").cloned().unwrap_or(Value::Null), "reason": reason, "pauses_agent": true,
    })))
}

async fn control(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = config.agent(Some(&agent_id)).cloned();
    // `await request.json()`: a body that is not JSON is an uncaught error (500)
    let body: Value = serde_json::from_slice(&body).map_err(|e| anyhow::anyhow!("JSONDecodeError: {e}"))?;
    let take_of = |body: &Value| -> ApiResult<bool> {
        match body {
            Value::Object(map) => Ok(map.get("take").map(crate::py::truthy).unwrap_or(false)),
            _ => Err(anyhow::anyhow!("AttributeError: object has no attribute 'get'").into()),
        }
    };
    if let Some(agent) = agent.as_ref().filter(|a| has_vnc(a)) {
        // No Hermes lease there: taking over lets the person's input through, and in the agent's own browser
        // container makes its tools wait (browsers.hold) until the person hands it back.
        let take = take_of(&body)?;
        state.desktop_control.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.name.clone(), take);
        crate::browsers::hold(&config, &agent.name, take).await;
        let holder = if take { "human" } else { "agent" };
        tracing::info!(agent = %agent.name, holder = holder, "desktop control");
        return Ok(web::ok(json!({"holder": holder, "you_hold": take})));
    }
    let folder = agent.as_ref().and_then(|a| desktop_dir(&config, a));
    let (Some(agent), Some(folder)) = (agent, folder) else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    let take = take_of(&body)?;
    let lease = set_lease(&folder, take, "Depuis l'app BipAgents")?;
    let holder = lease.get("holder").cloned().unwrap_or(Value::Null);
    tracing::info!(agent = %agent.name, holder = ?crate::logs::J(&holder), "desktop control");
    Ok(web::ok(json!({"holder": holder, "you_hold": app_holds(&folder)})))
}

/// `POST /v1/agents/{id}/desktop` and `…/desktop/control` (mounted under /v1 with the bridge key).
pub fn desktop_admin_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/desktop", post(open_desktop))
        .route("/agents/{agent_id}/desktop/control", post(control))
}

/// noVNC (`static/novnc`, embedded), mounted at /desktop/static/novnc.
pub fn static_files() -> Router<AppState> {
    let dir: &'static include_dir::Dir<'static> =
        crate::assets::STATIC.get_dir("novnc").expect("assets/static/novnc is embedded");
    crate::assets::static_router(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn watching_sends_no_input_and_taking_over_does() {
        let allowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let a = allowed.clone();
        let mut gate = InputGate::new(move || a.load(std::sync::atomic::Ordering::SeqCst));
        let handshake = b"RFB 003.008\n\x01\x00";
        assert_eq!(gate.feed(handshake).unwrap(), b"RFB 003.008\n\x01\x01");
        let mut update = vec![3u8, 0];
        for v in [0u16, 0, 100, 100] {
            update.extend(v.to_be_bytes());
        }
        let mut key = vec![4u8, 1, 0, 0];
        key.extend(0x61u32.to_be_bytes());
        let mut pointer = vec![5u8, 1];
        pointer.extend(10u16.to_be_bytes());
        pointer.extend(10u16.to_be_bytes());
        let mut encodings = vec![2u8, 0];
        encodings.extend(2u16.to_be_bytes());
        encodings.extend(0i32.to_be_bytes());
        encodings.extend((-223i32).to_be_bytes());
        let all: Vec<u8> = [update.clone(), key.clone(), pointer.clone(), encodings.clone()].concat();
        assert_eq!(gate.feed(&all).unwrap(), [update, encodings].concat());
        allowed.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(gate.feed(&key[..3]).unwrap(), b"");
        assert_eq!(gate.feed(&[&key[3..], &pointer[..]].concat()).unwrap(), [key, pointer].concat());
        assert!(gate.feed(&[7u8]).is_err());
    }

    #[test]
    fn the_lease_is_hermes_own_file() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("bot-desktop");
        assert!(!app_holds(&folder));
        let lease = set_lease(&folder, true, "test").unwrap();
        assert_eq!(lease["holder"], "human");
        assert_eq!(lease["epoch"], 1);
        assert!(app_holds(&folder));
        assert_eq!(set_lease(&folder, true, "").unwrap()["epoch"], 1); // already ours: no change
        assert_eq!(set_lease(&folder, false, "").unwrap()["holder"], "agent");
        assert!(!app_holds(&folder));
        std::fs::write(folder.join("lease.json"), "{broken").unwrap();
        assert!(!app_holds(&folder)); // unreadable: a human holds (Hermes' fail-safe)
        assert_eq!(read_lease(&folder)["holder"], "human");
    }

    #[test]
    fn the_vnc_password_file_may_be_clear_or_vncpasswd() {
        let dir = tempfile::tempdir().unwrap();
        let clear = dir.path().join("clear.txt");
        std::fs::write(&clear, "MotVNC-42\n").unwrap();
        assert_eq!(read_vnc_password(&clear.to_string_lossy()).unwrap(), "MotVNC-42");
        // Written by TigerVNC's own `vncpasswd` for « MotVNC-42 » (VNC keeps the first 8 characters).
        let obscured = dir.path().join("passwd");
        std::fs::write(&obscured, hex::decode("5bb576d0733bc57b").unwrap()).unwrap();
        assert_eq!(read_vnc_password(&obscured.to_string_lossy()).unwrap(), "MotVNC-4");
        let challenge: Vec<u8> = (0..16).collect();
        assert_eq!(vnc_response("MotVNC-42", &challenge).len(), 16);
        // What the Python's vnc_response gives for the same inputs.
        assert_eq!(hex::encode(vnc_response("MotVNC-42", &challenge)), "49c26463a09638c7a136d28d2d0d147e");
        assert_eq!(hex::encode(vnc_response("ab", &challenge)), "fe01155de95da3e28adf6cc730f06f08");
    }

    #[tokio::test]
    async fn the_bridge_logs_in_to_a_password_protected_vnc() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let challenge: Vec<u8> = (0..16).collect();

        async fn scenario(password_ok: bool, challenge: Vec<u8>) -> (bool, Vec<u8>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut received = Vec::new();
                s.write_all(b"RFB 003.008\n").await.unwrap();
                let mut buf = [0u8; 12];
                s.read_exact(&mut buf).await.unwrap();
                received.extend_from_slice(&buf);
                s.write_all(b"\x01\x02").await.unwrap(); // one type: VNC Authentication
                let mut one = [0u8; 1];
                s.read_exact(&mut one).await.unwrap();
                received.extend_from_slice(&one);
                s.write_all(&challenge).await.unwrap();
                let mut answer = [0u8; 16];
                s.read_exact(&mut answer).await.unwrap();
                let ok = answer.to_vec() == vnc_response("MotVNC-42", &challenge);
                s.write_all(if ok { b"\x00\x00\x00\x00" } else { b"\x00\x00\x00\x01" }).await.unwrap();
                received
            });
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let (r, w) = stream.into_split();
            let mut up = Upstream::stream(Box::new(r), Box::new(w));
            let result = vnc_login(&mut up, Some(if password_ok { "MotVNC-42" } else { "faux" })).await.is_ok();
            up.close().await;
            (result, server.await.unwrap())
        }

        let (ok, sent) = scenario(true, challenge.clone()).await;
        assert!(ok);
        assert_eq!(sent, b"RFB 003.008\n\x02");
        assert!(!scenario(false, challenge).await.0);
    }

    #[test]
    fn the_page_is_desktop_py_s() {
        assert!(PAGE.starts_with("<!doctype html>") && PAGE.ends_with("</script></body></html>"));
        assert!(PAGE.contains("rfb.viewOnly = __VIEW_ONLY__;"));
        assert!(RELAY.starts_with("import os,socket,sys,threading\ns=socket") && RELAY.ends_with("  os.write(1,d)\n"));
    }
}
