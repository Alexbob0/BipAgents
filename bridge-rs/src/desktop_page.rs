//! « Bureau » without the black screen: what the agent's browser is doing now, and on a phone, the page itself.
//!
//! - `GET /v1/agents/{id}/desktop/state`: stopped, starting, idle (its screen up, no page or a blank one) or active
//!   (a page open), with that page's site. The apps show a friendly empty state until the agent browses, then switch
//!   to the live view by themselves. Nothing is started by asking. `working`: its browser tools were used this last
//!   minute (the conversation's « <agent> navigue… »); `update_browser`: its image predates this page view (its
//!   DevTools relay tells no version), the apps say « bipagents update ».
//! - `POST /v1/agents/{id}/desktop/open`: the person's « Ouvrir le navigateur »: its container woken and Chrome
//!   started on its screen (browsers::open_browser).
//! - `/desktop/<ticket>/page/` and its WebSocket: on a phone, the active tab's page only (no taskbar, no window),
//!   as JPEG frames from Chrome's own screencast (CDP `Page.startScreencast`, through the container's DevTools relay),
//!   a few per second and only when the page changes. While the person holds the browser (« Prendre la main »,
//!   whose hold lets the bridge's connections through with its token), that tab renders as on a phone (device
//!   metrics, a mobile user agent, touch) and their taps, scrolls and keys go to it; handing it back clears the
//!   overrides and reloads the page before the agent's tools resume. Watching, the phone may ask to follow the
//!   agent (`{"type": "follow", "on": true}`): where it acts comes as `{"type": "focus", …}` when it moves, read
//!   without touching the page (desktop_read.rs, which also makes the « Lecture » view). A state « update »: the
//!   person holds the browser and its relay, too old, lets the bridge through no more.
//!
//! Only for an agent's own browser container (browsers.rs): a desktop set up by hand, or Hermes' Bot Desktop, keeps
//! the full desktop view (desktop.rs).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};

/// Pages that show nothing yet: a new tab, a blank one.
pub const BLANK: [&str; 6] = [
    "about:blank",
    "chrome://newtab",
    "chrome://new-tab-page",
    "chrome-search://",
    "chrome://welcome",
    "edge://newtab",
];
/// How long the bridge's « Ouvrir le navigateur » counts as starting.
pub const STARTING_SECONDS: f64 = 90.0;
/// The agent's browser tools touched its `busy` file this recently: it is starting it.
pub const BUSY_SECONDS: u64 = 15;
/// Its browser tools were used this recently: it is browsing (the state's `working`).
pub const WORKING_SECONDS: u64 = 60;
/// At most this many frames per second reach the phone (Chrome waits for each frame's ack before the next).
pub const FRAME_INTERVAL: Duration = Duration::from_millis(200);
/// The card in the conversation (`?card=1`): about one small frame a second, light on the phone's battery and data.
pub const CARD_FRAME_INTERVAL: Duration = Duration::from_millis(1000);
/// The viewer page (`assets/desktop_page.html`).
pub static PAGE: LazyLock<String> = LazyLock::new(|| include_str!("../assets/desktop_page.html").to_string());

type WsClient = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsMessage = tokio_tungstenite::tungstenite::Message;

// -- what the browser does ----------------------------------------------------------------------------------------

/// Agents whose browser the person asked to open, and since when.
static OPENING: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn opening(agent: &str) -> bool {
    let items = OPENING.lock().unwrap_or_else(|e| e.into_inner());
    items.get(agent).is_some_and(|since| crate::py::time_time() - since < STARTING_SECONDS)
}

fn set_opening(agent: &str, on: bool) {
    let mut items = OPENING.lock().unwrap_or_else(|e| e.into_inner());
    if on {
        items.insert(agent.to_string(), crate::py::time_time());
    } else {
        items.remove(agent);
    }
}

/// Its DevTools port, when the agent has its own browser container.
pub fn cdp_port(config: &Config, agent: &str) -> Option<u16> {
    crate::browsers::ports_of(config, agent)?.get("cdp").copied()
}

/// A tab of the agent's Chrome, as its DevTools list it (the most recently active first).
#[derive(Debug, Clone, PartialEq)]
pub struct Tab {
    pub id: String,
    pub url: String,
    pub title: String,
}

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_default()
});

/// The page tabs of the Chrome behind `port`, most recently active first (Chrome's own order). `token`: the
/// person's hold, which the container's relay lets through.
pub async fn tabs(port: u16, token: Option<&str>) -> Result<Vec<Tab>, String> {
    let mut request = HTTP.get(format!("http://127.0.0.1:{port}/json/list"));
    if let Some(token) = token {
        request = request.header("X-Bip-Hold", token);
    }
    let answer = request.send().await.map_err(|e| e.to_string())?;
    let list: Value = answer.json().await.map_err(|e| e.to_string())?;
    let items = list.as_array().ok_or("not a list")?;
    Ok(items
        .iter()
        .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
        .filter_map(|t| {
            let url = t.get("url").and_then(Value::as_str).unwrap_or_default();
            if url.starts_with("devtools://") || url.starts_with("chrome-extension://") {
                return None;
            }
            Some(Tab {
                id: t.get("id").and_then(Value::as_str)?.to_string(),
                url: url.to_string(),
                title: t.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
            })
        })
        .collect())
}

pub fn is_blank(url: &str) -> bool {
    url.is_empty() || BLANK.iter().any(|b| url.starts_with(b))
}

/// The site, as a person reads it: `www.lemonde.fr/…` → `lemonde.fr`.
pub fn site(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

fn busy_within(config: &Config, agent: &str, seconds: u64) -> bool {
    std::fs::metadata(crate::browsers::folder(config, agent).join("busy"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(seconds))
}

fn busy_recently(config: &Config, agent: &str) -> bool {
    busy_within(config, agent, BUSY_SECONDS)
}

/// Its browser tools were used this last minute (each call touches `busy`, and every 30 s of a long one): the
/// conversation's « <agent> navigue… ».
fn working_recently(config: &Config, agent: &str) -> bool {
    busy_within(config, agent, WORKING_SECONDS)
}

async fn listening(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
    matches!(tokio::time::timeout(Duration::from_secs(1), connect).await, Ok(Ok(_)))
}

fn holds(state: &AppState, agent: &str) -> bool {
    state.desktop_control.lock().unwrap_or_else(|e| e.into_inner()).get(agent).copied().unwrap_or(false)
}

/// What the agent's own browser container is doing: (state, Chrome runs, the active tab).
async fn browser_state(config: &Config, agent: &str, port: u16) -> (&'static str, bool, Option<Tab>) {
    let starting = opening(agent) || busy_recently(config, agent);
    if !listening(port).await {
        return (if starting { "starting" } else { "stopped" }, false, None);
    }
    let token = crate::browsers::hold_token(config, agent);
    match tabs(port, token.as_deref()).await {
        // its screen is up, no Chrome on it yet (or a payment holds it)
        Err(_) => (if starting { "starting" } else { "idle" }, false, None),
        Ok(list) => match list.into_iter().next() {
            Some(tab) if !is_blank(&tab.url) => ("active", true, Some(tab)),
            tab => ("idle", true, tab),
        },
    }
}

/// `GET /v1/agents/{id}/desktop/state`.
pub async fn state_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    if let Some(port) = cdp_port(&config, &agent.name).filter(|_| has_vnc(&agent)) {
        let (now, browser, tab) = browser_state(&config, &agent.name, port).await;
        // Chrome answers through its relay, which does not know this page view: an image to rebuild.
        let update = browser && crate::desktop_read::relay_outdated(port).await;
        return Ok(web::ok(json!({
            "state": now, "browser": browser, "page": true, "can_open": true,
            "site": tab.as_ref().and_then(|t| site(&t.url)), "title": tab.as_ref().map(|t| t.title.clone()),
            "you_hold": holds(&state, &agent.name),
            "working": now == "active" && working_recently(&config, &agent.name), "update_browser": update,
        })));
    }
    // A desktop set up by hand, a viewer page, Hermes' Bot Desktop: shown whole, as it is.
    let running = has_vnc(&agent)
        || agent.desktop_url.as_deref().is_some_and(|u| !u.is_empty())
        || crate::desktop::desktop_dir(&config, &agent)
            .is_some_and(|f| f.join("rfb.sock").exists() || f.join("sandbox.json").exists());
    if !running {
        return Err(HttpError::json(409, json!({"code": "no_desktop"})));
    }
    Ok(web::ok(json!({
        "state": "active", "browser": true, "page": false, "can_open": false, "site": null, "title": null,
        "you_hold": holds(&state, &agent.name), "working": false, "update_browser": false,
    })))
}

/// `POST /v1/agents/{id}/desktop/open`: « Ouvrir le navigateur ». Answers at once (starting); the browser is up a
/// few seconds later.
pub async fn open_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    let Some(port) = cdp_port(&config, &agent.name).filter(|_| has_vnc(&agent)) else {
        return Err(HttpError::json(409, json!({"code": "cannot_open"})));
    };
    set_opening(&agent.name, true);
    let name = agent.name.clone();
    tokio::spawn(async move {
        if crate::browsers::open_browser(&config, &name).await {
            let deadline = crate::py::monotonic() + 30.0;
            while crate::py::monotonic() < deadline {
                let token = crate::browsers::hold_token(&config, &name);
                if tabs(port, token.as_deref()).await.is_ok_and(|t| !t.is_empty()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        set_opening(&name, false);
    });
    tracing::info!(agent = %agent.name, "browser opened by the person");
    Ok(web::ok(json!({"state": "starting", "browser": false, "page": true, "can_open": true, "site": null,
                      "title": null, "you_hold": holds(&state, &agent.name), "working": false,
                      "update_browser": false})))
}

fn has_vnc(agent: &AgentConfig) -> bool {
    agent.desktop_vnc.as_deref().is_some_and(|v| !v.is_empty())
}

// -- Chrome's DevTools, one tab ------------------------------------------------------------------------------------

/// A DevTools connection to one tab, through the container's relay.
pub struct Cdp {
    sink: SplitSink<WsClient, WsMessage>,
    stream: SplitStream<WsClient>,
    next: u64,
    /// The newest screencast frame that came while waiting for an answer: handed to `receive` next, not lost.
    stashed: Option<Value>,
}

impl Cdp {
    pub async fn connect(port: u16, tab: &str, token: Option<&str>) -> Result<Cdp, String> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        if !tab.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("odd tab id".into());
        }
        let mut request =
            format!("ws://127.0.0.1:{port}/devtools/page/{tab}").into_client_request().map_err(|e| e.to_string())?;
        if let Some(token) = token.and_then(|t| HeaderValue::from_str(t).ok()) {
            request.headers_mut().insert("x-bip-hold", token);
        }
        let mut config = tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default();
        config.max_message_size = Some(64 << 20);
        config.max_frame_size = Some(64 << 20);
        let connecting = tokio_tungstenite::connect_async_with_config(request, Some(config), false);
        let (socket, _) = tokio::time::timeout(Duration::from_secs(5), connecting)
            .await
            .map_err(|_| "timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let (sink, stream) = socket.split();
        Ok(Cdp { sink, stream, next: 0, stashed: None })
    }

    /// Sends a command without waiting for its answer.
    pub async fn send(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next += 1;
        let text = json!({"id": self.next, "method": method, "params": params}).to_string();
        self.sink.send(WsMessage::Text(text.into())).await.map_err(|e| e.to_string())?;
        Ok(self.next)
    }

    /// The next message from Chrome (None: it closed).
    pub async fn receive(&mut self) -> Option<Value> {
        if let Some(frame) = self.stashed.take() {
            return Some(frame);
        }
        loop {
            match self.stream.next().await? {
                Ok(WsMessage::Text(text)) => {
                    if let Ok(value) = serde_json::from_str(&text) {
                        return Some(value);
                    }
                }
                Ok(WsMessage::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    /// A command and its answer. A frame that comes meanwhile is kept for `receive` (an older one kept before it is
    /// acknowledged and dropped): the view never misses the page's last change.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.send(method, params).await?;
        let wait = async {
            loop {
                let message = loop {
                    match self.stream.next().await {
                        Some(Ok(WsMessage::Text(text))) => {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                break value;
                            }
                        }
                        Some(Ok(WsMessage::Close(_)) | Err(_)) | None => return Err("closed".to_string()),
                        Some(Ok(_)) => {}
                    }
                };
                if message.get("id").and_then(Value::as_u64) == Some(id) {
                    return match message.get("error") {
                        Some(error) => Err(error.to_string()),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                }
                if frame_session(&message).is_some() {
                    if let Some(session) = self.stashed.replace(message).as_ref().and_then(frame_session) {
                        self.send("Page.screencastFrameAck", json!({"sessionId": session})).await?;
                    }
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), wait).await.map_err(|_| "timed out".to_string())?
    }

    pub async fn close(mut self) {
        let _ = self.sink.close().await;
    }
}

fn frame_session(message: &Value) -> Option<Value> {
    (message.get("method").and_then(Value::as_str) == Some("Page.screencastFrame"))
        .then(|| message.pointer("/params/sessionId").cloned())
        .flatten()
}

/// What a phone asks for its page view (`?mobile=1&w=390&h=760&dpr=3`): the size it renders a taken-over tab at.
/// `?card=1&w=360&dpr=2`: the small live card in the conversation, to watch only (slow, small frames; the agent's tab
/// is never rendered as a phone for it).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Deserialize)]
pub struct Phone {
    #[serde(default)]
    pub mobile: Option<u8>,
    #[serde(default)]
    pub card: Option<u8>,
    #[serde(default)]
    pub w: Option<f64>,
    #[serde(default)]
    pub h: Option<f64>,
    #[serde(default)]
    pub dpr: Option<f64>,
}

impl Phone {
    pub fn wanted(&self) -> bool {
        self.mobile == Some(1) && !self.is_card()
    }
    pub fn is_card(&self) -> bool {
        self.card == Some(1)
    }
    /// The card's screencast (as wide as the card in pixels, 900 at most, lower quality) and the time between frames.
    pub fn card_cast(&self) -> (Value, Duration) {
        let width = self.w.unwrap_or(360.0).clamp(200.0, 600.0) * self.dpr.unwrap_or(2.0).clamp(1.0, 2.0);
        let width = width.min(900.0).round() as u32;
        (json!({"format": "jpeg", "quality": 45, "maxWidth": width, "maxHeight": width * 2}), CARD_FRAME_INTERVAL)
    }
    /// Its width and height in CSS pixels, and its pixel ratio (2 at most: lighter frames, still sharp).
    pub fn metrics(&self, desktop_height: f64) -> (u32, u32, f64) {
        let width = self.w.unwrap_or(390.0).clamp(280.0, 600.0);
        let height = self.h.unwrap_or(760.0).clamp(400.0, 1200.0).min(desktop_height.max(400.0));
        (width.round() as u32, height.round() as u32, self.dpr.unwrap_or(2.0).clamp(1.0, 2.0))
    }
}

/// A user agent of Chrome on Android, of the same version as this Chrome (`Chrome/131.0.6778.85` → 131).
pub fn mobile_user_agent(product: &str) -> (String, String) {
    let major = product.split('/').nth(1).and_then(|v| v.split('.').next()).filter(|m| !m.is_empty()).unwrap_or("130");
    (
        format!(
            "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Mobile \
             Safari/537.36"
        ),
        major.to_string(),
    )
}

/// The tab renders as on the person's phone (until `restore`): its size, a mobile user agent, touch; then reloaded.
pub async fn make_mobile(cdp: &mut Cdp, phone: &Phone, desktop_height: f64) -> Result<(), String> {
    let (width, height, ratio) = phone.metrics(desktop_height);
    let version = cdp.call("Browser.getVersion", json!({})).await.unwrap_or(Value::Null);
    let (agent, major) = mobile_user_agent(version.get("product").and_then(Value::as_str).unwrap_or(""));
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({"width": width, "height": height, "deviceScaleFactor": ratio, "mobile": true,
               "screenWidth": width, "screenHeight": height}),
    )
    .await?;
    let metadata = json!({"brands": [{"brand": "Chromium", "version": major}, {"brand": "Not?A_Brand", "version": "99"}],
                          "platform": "Android", "platformVersion": "10.0.0", "architecture": "", "model": "K",
                          "mobile": true});
    if cdp
        .call("Emulation.setUserAgentOverride", json!({"userAgent": agent, "userAgentMetadata": metadata}))
        .await
        .is_err()
    {
        cdp.call("Emulation.setUserAgentOverride", json!({"userAgent": agent})).await?;
    }
    cdp.call("Emulation.setTouchEmulationEnabled", json!({"enabled": true, "maxTouchPoints": 5})).await?;
    cdp.call("Page.reload", json!({})).await?;
    Ok(())
}

/// The tab back as the agent left it: its size and touch cleared here, its user agent by closing this connection
/// (Chrome then goes back to the one the agent's tools set), then reloaded from a fresh connection.
pub async fn restore(mut cdp: Cdp, port: u16, tab: &str, token: Option<&str>) {
    let _ = cdp.call("Emulation.clearDeviceMetricsOverride", json!({})).await;
    let _ = cdp.call("Emulation.setTouchEmulationEnabled", json!({"enabled": false})).await;
    cdp.close().await;
    if let Ok(mut fresh) = Cdp::connect(port, tab, token).await {
        let _ = fresh.call("Page.reload", json!({})).await;
        fresh.close().await;
    }
}

// -- the person's taps and keys --------------------------------------------------------------------------------------

/// A key of the page view's key bar: (key, code, Windows key code, text).
fn key(name: &str) -> Option<(&'static str, &'static str, u32, &'static str)> {
    Some(match name {
        "Backspace" => ("Backspace", "Backspace", 8, ""),
        "Enter" => ("Enter", "Enter", 13, "\r"),
        "Tab" => ("Tab", "Tab", 9, ""),
        "Escape" => ("Escape", "Escape", 27, ""),
        "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 37, ""),
        "ArrowUp" => ("ArrowUp", "ArrowUp", 38, ""),
        "ArrowRight" => ("ArrowRight", "ArrowRight", 39, ""),
        "ArrowDown" => ("ArrowDown", "ArrowDown", 40, ""),
        "PageUp" => ("PageUp", "PageUp", 33, ""),
        "PageDown" => ("PageDown", "PageDown", 34, ""),
        _ => return None,
    })
}

/// An address typed in the page view: a site (`lemonde.fr`, `https://…`) or, otherwise, a search.
pub fn address(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > 2000 {
        return None;
    }
    let lower = text.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return url::Url::parse(text).ok().map(|u| u.to_string());
    }
    if !text.contains(char::is_whitespace) && text.contains('.') && !text.contains("://") {
        if let Ok(url) = url::Url::parse(&format!("https://{text}")) {
            return Some(url.to_string());
        }
    }
    let mut url = url::Url::parse("https://duckduckgo.com/").ok()?;
    url.query_pairs_mut().append_pair("q", text);
    Some(url.to_string())
}

/// The viewport the last frame showed, in CSS pixels (taps come as fractions of the picture).
#[derive(Debug, Clone, Copy)]
struct Viewport {
    width: f64,
    height: f64,
}

fn point(value: &Value, view: Viewport, id: usize) -> Option<Value> {
    let x = value.get(0)?.as_f64()?.clamp(0.0, 1.0) * view.width;
    let y = value.get(1)?.as_f64()?.clamp(0.0, 1.0) * view.height;
    Some(json!({"x": x, "y": y, "id": id}))
}

/// One message from the page view, while the person holds the browser.
async fn act(cdp: &mut Cdp, message: &Value, view: Viewport) -> Result<(), String> {
    match message.get("type").and_then(Value::as_str).unwrap_or("") {
        "touch" => {
            let phase = message.get("phase").and_then(Value::as_str).unwrap_or("");
            let kind = match phase {
                "start" => "touchStart",
                "move" => "touchMove",
                "end" => "touchEnd",
                "cancel" => "touchCancel",
                _ => return Ok(()),
            };
            let points: Vec<Value> = match message.get("points").and_then(Value::as_array) {
                Some(items) if kind != "touchEnd" && kind != "touchCancel" => {
                    items.iter().take(5).enumerate().filter_map(|(i, p)| point(p, view, i)).collect()
                }
                _ => Vec::new(),
            };
            cdp.send("Input.dispatchTouchEvent", json!({"type": kind, "touchPoints": points})).await?;
        }
        "text" => {
            let text: String = message.get("text").and_then(Value::as_str).unwrap_or("").chars().take(2000).collect();
            if !text.is_empty() {
                cdp.send("Input.insertText", json!({"text": text})).await?;
            }
        }
        "key" => {
            let Some((name, code, keycode, text)) = key(message.get("key").and_then(Value::as_str).unwrap_or(""))
            else {
                return Ok(());
            };
            let mut down = json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": name,
                                  "code": code, "windowsVirtualKeyCode": keycode, "nativeVirtualKeyCode": keycode});
            if !text.is_empty() {
                down["text"] = json!(text);
            }
            cdp.send("Input.dispatchKeyEvent", down).await?;
            cdp.send(
                "Input.dispatchKeyEvent",
                json!({"type": "keyUp", "key": name, "code": code, "windowsVirtualKeyCode": keycode,
                       "nativeVirtualKeyCode": keycode}),
            )
            .await?;
        }
        "back" => {
            let history = cdp.call("Page.getNavigationHistory", json!({})).await?;
            let index = history.get("currentIndex").and_then(Value::as_i64).unwrap_or(0);
            if index > 0 {
                if let Some(entry) = history.pointer(&format!("/entries/{}/id", index - 1)) {
                    cdp.send("Page.navigateToHistoryEntry", json!({"entryId": entry})).await?;
                }
            }
        }
        "go" => {
            if let Some(url) = address(message.get("url").and_then(Value::as_str).unwrap_or("")) {
                cdp.send("Page.navigate", json!({"url": url})).await?;
            }
        }
        _ => {}
    }
    Ok(())
}

// -- the page view's stream ----------------------------------------------------------------------------------------

/// Sessions that show a tab taken over on a phone, by agent: handing back asks each to restore its tab first.
type Restorers = HashMap<String, Vec<(u64, mpsc::Sender<oneshot::Sender<()>>)>>;
static RESTORERS: LazyLock<Mutex<Restorers>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static SESSION_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Before the person's hold is released: every tab shown on a phone back to desktop rendering, reloaded (5 s at most
/// each), so the agent finds the page as it renders for it.
pub async fn give_back(agent: &str) {
    let senders: Vec<_> = {
        let items = RESTORERS.lock().unwrap_or_else(|e| e.into_inner());
        items.get(agent).map(|v| v.iter().map(|(_, s)| s.clone()).collect()).unwrap_or_default()
    };
    for sender in senders {
        let (done, wait) = oneshot::channel();
        if sender.send(done).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;
        }
    }
}

/// `/desktop/{ticket}/page/`.
pub async fn page(State(state): State<AppState>, UrlPath(ticket): UrlPath<String>) -> ApiResult<Response> {
    if state.desktop_tickets.agent(&ticket).is_none() {
        return Err(HttpError::new(404, "expired"));
    }
    let mut response = Response::new(axum::body::Body::from(PAGE.as_str()));
    let h = response.headers_mut();
    h.insert("cache-control", HeaderValue::from_static("no-store"));
    h.insert("content-type", HeaderValue::from_static("text/html; charset=utf-8"));
    Ok(response)
}

/// `/desktop/{ticket}/page/ws`.
pub async fn stream(
    State(state): State<AppState>,
    UrlPath(ticket): UrlPath<String>,
    Query(phone): Query<Phone>,
    ws: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Ok(ws) = ws else { return HttpError::new(404, "Not Found").into_response() };
    let config = state.config();
    let name = state.desktop_tickets.agent(&ticket);
    let Some(agent) = name.as_deref().and_then(|n| config.agent(Some(n))).cloned() else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if cdp_port(&config, &agent.name).is_none() || !has_vnc(&agent) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(1 << 20).on_upgrade(move |socket| session(state, agent.name, phone, socket))
}

async fn send_json(app: &mut SplitSink<WebSocket, Message>, value: Value) -> bool {
    app.send(Message::Text(value.to_string().into())).await.is_ok()
}

/// What the page view gets while nothing can be shown: until something can, or the person leaves.
async fn wait_a_moment(app_rx: &mut SplitStream<WebSocket>, restore: &mut mpsc::Receiver<oneshot::Sender<()>>) -> bool {
    let sleep = tokio::time::sleep(Duration::from_millis(1500));
    tokio::pin!(sleep);
    loop {
        tokio::select! {
            _ = &mut sleep => return true,
            message = app_rx.next() => match message {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return false,
                _ => {}
            },
            Some(done) = restore.recv() => { let _ = done.send(()); }
        }
    }
}

async fn session(state: AppState, agent: String, phone: Phone, socket: WebSocket) {
    let (mut app_tx, mut app_rx) = socket.split();
    let (restore_tx, mut restore_rx) = mpsc::channel::<oneshot::Sender<()>>(4);
    let id = SESSION_IDS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    RESTORERS.lock().unwrap_or_else(|e| e.into_inner()).entry(agent.clone()).or_default().push((id, restore_tx));
    tracing::info!(agent = %agent, mobile = phone.wanted(), "page view connected");
    let mut desktop_height = 650.0f64;
    let mut last_said = Value::Null;
    // The phone follows where the agent acts (its « Suivre »): the focus pushed as it moves (desktop_read.rs).
    let mut follow = false;
    'outer: loop {
        let config = state.config();
        let Some(port) = cdp_port(&config, &agent) else { break };
        let holding = holds(&state, &agent);
        let token = if holding { crate::browsers::hold_token(&config, &agent) } else { None };
        let list = tabs(port, token.as_deref()).await;
        let Some(tab) = list.as_ref().ok().and_then(|l| l.first().cloned()) else {
            let now = match &list {
                Ok(_) => "idle",
                Err(_) if listening(port).await => "idle",
                Err(_) => "stopped",
            };
            let now = if holding && now == "idle" && list.is_err() { outdated_or(port, now).await } else { now };
            let said = json!({"type": "state", "state": now, "mobile": false, "holding": holding});
            if said != last_said {
                if !send_json(&mut app_tx, said.clone()).await {
                    break;
                }
                last_said = said;
            }
            if !wait_a_moment(&mut app_rx, &mut restore_rx).await {
                break;
            }
            continue;
        };
        let mut cdp = match Cdp::connect(port, &tab.id, token.as_deref()).await {
            Ok(cdp) => cdp,
            Err(_) => {
                // held and not let through: an image whose relay predates the person's hold token
                let now = if holding { outdated_or(port, "paused").await } else { "paused" };
                let said = json!({"type": "state", "state": now, "mobile": false, "holding": holding});
                if said != last_said {
                    if !send_json(&mut app_tx, said.clone()).await {
                        break;
                    }
                    last_said = said;
                }
                if !wait_a_moment(&mut app_rx, &mut restore_rx).await {
                    break;
                }
                continue;
            }
        };
        let mut mobile = false;
        if phone.wanted() && holding {
            match make_mobile(&mut cdp, &phone, desktop_height).await {
                Ok(()) => mobile = true,
                Err(error) => tracing::info!(agent = %agent, error = %error, "page view: not rendered as a phone"),
            }
        }
        let (width, height, ratio) = phone.metrics(desktop_height);
        let (cast, interval) = if phone.is_card() {
            phone.card_cast()
        } else if mobile {
            let cast = json!({"format": "jpeg", "quality": 60, "maxWidth": (width as f64 * ratio) as u32,
                              "maxHeight": (height as f64 * ratio) as u32});
            (cast, FRAME_INTERVAL)
        } else {
            (json!({"format": "jpeg", "quality": 55, "maxWidth": 1366, "maxHeight": 1366}), FRAME_INTERVAL)
        };
        if cdp.send("Page.startScreencast", cast).await.is_err() {
            continue;
        }
        let said = json!({"type": "state", "state": "live", "mobile": mobile, "holding": holding});
        if !send_json(&mut app_tx, said.clone()).await {
            restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
            break;
        }
        last_said = said;
        let mut shown = tab.clone();
        if !send_json(&mut app_tx, page_info(&shown)).await {
            restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
            break;
        }
        let mut view = Viewport { width: 1366.0, height: desktop_height };
        let mut pending: Option<Value> = None;
        let mut next_ack = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut touched: Option<Instant> = None;
        let mut world: Option<i64> = None;
        let mut changed = true; // frames came since the focus was last read
        let mut last_focus = Value::Null;
        loop {
            tokio::select! {
                message = cdp.receive() => {
                    let Some(message) = message else { continue 'outer };
                    if message.get("method").and_then(Value::as_str) != Some("Page.screencastFrame") {
                        continue;
                    }
                    let params = message.get("params").cloned().unwrap_or(Value::Null);
                    let meta = params.get("metadata").cloned().unwrap_or(Value::Null);
                    view.width = meta.get("deviceWidth").and_then(Value::as_f64).unwrap_or(view.width);
                    view.height = meta.get("deviceHeight").and_then(Value::as_f64).unwrap_or(view.height);
                    if !mobile {
                        desktop_height = view.height;
                    }
                    use base64::Engine;
                    let data = params.get("data").and_then(Value::as_str).unwrap_or("");
                    let Ok(jpeg) = base64::engine::general_purpose::STANDARD.decode(data) else { continue };
                    let head = json!({"type": "frame", "w": view.width, "h": view.height});
                    if !send_json(&mut app_tx, head).await
                        || app_tx.send(Message::Binary(Bytes::from(jpeg))).await.is_err()
                    {
                        restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                        break 'outer;
                    }
                    pending = params.get("sessionId").cloned();
                    changed = true;
                    next_ack = Instant::now() + interval;
                    if touched.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                        touched = Some(Instant::now());
                        crate::browsers::busy(&state.config(), &agent); // watched: not stopped as unused
                    }
                }
                _ = tokio::time::sleep_until(next_ack), if pending.is_some() => {
                    if let Some(session) = pending.take() {
                        let _ = cdp.send("Page.screencastFrameAck", json!({"sessionId": session})).await;
                    }
                }
                message = app_rx.next() => match message {
                    Some(Ok(Message::Text(text))) => {
                        let value = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
                        if value.get("type").and_then(Value::as_str) == Some("follow") {
                            follow = value.get("on").and_then(Value::as_bool).unwrap_or(false);
                            (changed, last_focus) = (true, Value::Null);
                            continue;
                        }
                        if !holds(&state, &agent) {
                            continue;
                        }
                        if act(&mut cdp, &value, view).await.is_err() {
                            continue 'outer;
                        }
                    }
                    Some(Ok(_)) => {}
                    None | Some(Err(_)) => {
                        restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                        break 'outer;
                    }
                },
                Some(done) = restore_rx.recv() => {
                    if mobile {
                        restore(cdp, port, &tab.id, token.as_deref()).await;
                    } else {
                        cdp.close().await;
                    }
                    let _ = done.send(());
                    continue 'outer;
                }
                _ = tick.tick() => {
                    if follow && !mobile && changed {
                        changed = false;
                        if let Ok(focus) = crate::desktop_read::focus(&mut cdp, &mut world).await {
                            let mut said = focus.to_json();
                            said["type"] = json!("focus");
                            if said != last_focus {
                                if !send_json(&mut app_tx, said.clone()).await {
                                    restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                                    break 'outer;
                                }
                                last_focus = said;
                            }
                        }
                    }
                    if holds(&state, &agent) != holding {
                        // taken over or handed back: connected again, with or without the hold's token
                        restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                        continue 'outer;
                    }
                    match tabs(port, token.as_deref()).await.ok().and_then(|l| l.into_iter().next()) {
                        Some(now) if now.id != shown.id => {
                            // another tab in front: shown instead
                            restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                            continue 'outer;
                        }
                        Some(now) if now != shown => {
                            shown = now;
                            if !send_json(&mut app_tx, page_info(&shown)).await {
                                restore_or_close(cdp, mobile, port, &tab.id, token.as_deref()).await;
                                break 'outer;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    {
        let mut items = RESTORERS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(list) = items.get_mut(&agent) {
            list.retain(|(i, _)| *i != id);
            if list.is_empty() {
                items.remove(&agent);
            }
        }
    }
    let _ = app_tx.close().await;
    tracing::info!(agent = %agent, "page view closed");
}

/// `fallback`, or « update » when the agent's browser image predates this page view (its relay says no version).
async fn outdated_or(port: u16, fallback: &'static str) -> &'static str {
    if crate::desktop_read::relay_outdated(port).await {
        "update"
    } else {
        fallback
    }
}

async fn restore_or_close(cdp: Cdp, mobile: bool, port: u16, tab: &str, token: Option<&str>) {
    if mobile {
        restore(cdp, port, tab, token).await;
    } else {
        cdp.close().await;
    }
}

fn page_info(tab: &Tab) -> Value {
    json!({"type": "page", "site": site(&tab.url), "url": if is_blank(&tab.url) { "" } else { tab.url.as_str() },
           "title": tab.title})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_reads_as_a_person_reads_it() {
        assert_eq!(site("https://www.lemonde.fr/economie/").as_deref(), Some("lemonde.fr"));
        assert_eq!(site("http://Mairie.Paris.fr:8080/x?y").as_deref(), Some("mairie.paris.fr"));
        assert_eq!(site("chrome://newtab/"), None);
        assert!(is_blank("about:blank") && is_blank("chrome://new-tab-page/") && is_blank(""));
        assert!(!is_blank("https://example.org/"));
    }

    #[test]
    fn an_address_is_a_site_or_a_search() {
        assert_eq!(address("lemonde.fr").as_deref(), Some("https://lemonde.fr/"));
        assert_eq!(address(" https://ex.org/a b ").as_deref(), Some("https://ex.org/a%20b"));
        assert_eq!(address("horaires piscine").as_deref(), Some("https://duckduckgo.com/?q=horaires+piscine"));
        assert_eq!(
            address("javascript:alert(1)").as_deref(),
            Some("https://duckduckgo.com/?q=javascript%3Aalert%281%29")
        );
        assert_eq!(address("  "), None);
    }

    #[test]
    fn a_phone_renders_at_its_own_size_within_the_screen() {
        let phone = Phone { mobile: Some(1), w: Some(393.0), h: Some(852.0), dpr: Some(3.0), ..Phone::default() };
        assert!(phone.wanted());
        assert_eq!(phone.metrics(640.0), (393, 640, 2.0));
        assert_eq!(Phone::default().metrics(2000.0), (390, 760, 2.0));
        assert_eq!(
            Phone { mobile: Some(1), w: Some(9999.0), h: Some(1.0), dpr: Some(0.5), ..Phone::default() }.metrics(900.0),
            (600, 400, 1.0)
        );
        let (agent, major) = mobile_user_agent("Chrome/131.0.6778.85");
        assert_eq!(major, "131");
        assert!(agent.contains("Android") && agent.contains("Chrome/131.0.0.0 Mobile Safari"));
        assert_eq!(mobile_user_agent("").1, "130");
    }

    #[test]
    fn the_conversation_card_gets_small_slow_frames_and_never_a_phone_tab() {
        let card = Phone { card: Some(1), mobile: Some(1), w: Some(358.0), dpr: Some(3.0), ..Phone::default() };
        assert!(card.is_card() && !card.wanted());
        let (cast, interval) = card.card_cast();
        assert_eq!(cast["maxWidth"], 716);
        assert_eq!(cast["quality"], 45);
        assert_eq!(interval, CARD_FRAME_INTERVAL);
        assert!(CARD_FRAME_INTERVAL > FRAME_INTERVAL);
        let wide = Phone { card: Some(1), w: Some(5000.0), dpr: Some(2.0), ..Phone::default() };
        assert_eq!(wide.card_cast().0["maxWidth"], 900);
    }
}
