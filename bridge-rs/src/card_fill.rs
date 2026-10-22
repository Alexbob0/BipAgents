//! A payment card typed into the agent's visible browser (browsers.rs), by the bridge, over Chrome's DevTools
//! protocol: the card's numbers go from the bridge's vault to the page's fields and nowhere else (not the agent, not
//! its Hermes, not a log).
//!
//! The page is the agent's current tab: its address is Chrome's own (the main frame's URL in the frame tree, which a
//! page cannot fake), the one the card's allowed sites are checked against and the person is shown. Its fields are
//! found in every frame, payment providers' cross-origin frames included: each frame (and each out-of-process frame,
//! reached through `Target.setAutoAttach`) gets an isolated world of its own where `assets/card_fill.js` looks for
//! the card's fields (autocomplete tokens first, then names, labels and placeholders). Each field is focused and the
//! text typed (`Input.insertText`, as a person typing: the providers' own formatting runs), else set the way a
//! framework sees it; a select (the expiry's month and year) gets its option. Nothing is clicked: the agent presses
//! « Pay » itself once the card is in.

use std::collections::VecDeque;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};

const SCRIPT: &str = include_str!("../assets/card_fill.js");
const WORLD: &str = "bipagents-card";
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// How long out-of-process frames take to be reported once auto-attach is on.
const ATTACH_WAIT: Duration = Duration::from_millis(300);
const MAX_SESSIONS: usize = 30;
/// What the relay looks for while a payment holds the browser.
pub const HOLD_HEADER: &str = "x-bip-card";

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsMessage = tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FillError {
    /// Nothing answers on its DevTools port (stopped, or the person holds the browser in the app).
    #[error("the browser cannot be reached")]
    Unreachable,
    /// No web page open.
    #[error("no web page is open")]
    NoPage,
    #[error("browser protocol: {0}")]
    Protocol(String),
}

fn protocol(text: impl Into<String>) -> FillError {
    FillError::Protocol(text.into())
}

/// One field found: its frame's world, its index there, its kind (number, exp, exp_month, exp_year, csc, name,
/// given_name, family_name, postal) and how the expiry is written (`MM/YY`, `MM/YYYY`, `MMYY`; `YY`, `YYYY`).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub session: String,
    pub context: i64,
    pub frame_url: String,
    pub index: u64,
    pub kind: String,
    pub tag: String,
    pub fmt: String,
    pub empty: bool,
}

/// What to type. Not `Debug`: never printed; wiped from memory when dropped (the security code, given by the person
/// for this payment only, included).
#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
pub struct CardValues {
    pub number: String,
    pub exp_month: u32,
    pub exp_year: u32,
    pub csc: String,
    pub name: String,
    pub postal_code: String,
}

impl CardValues {
    /// The text for a field, None when the card has nothing for it.
    fn text_for(&self, field: &Field) -> Option<String> {
        let (mm, yyyy, yy) =
            (format!("{:02}", self.exp_month), self.exp_year.to_string(), format!("{:02}", self.exp_year % 100));
        let (given, family) = match self.name.trim().split_once(' ') {
            Some((g, f)) => (g.to_string(), f.trim().to_string()),
            None => (self.name.trim().to_string(), String::new()),
        };
        let text = match field.kind.as_str() {
            "number" => self.number.clone(),
            "exp" => match field.fmt.as_str() {
                "MM/YYYY" => format!("{mm}/{yyyy}"),
                "MMYY" => format!("{mm}{yy}"),
                _ => format!("{mm}/{yy}"),
            },
            "exp_month" => mm,
            "exp_year" => {
                if field.fmt == "YY" {
                    yy
                } else {
                    yyyy
                }
            }
            "csc" => self.csc.clone(),
            "name" => self.name.trim().to_string(),
            "given_name" => given,
            "family_name" => family,
            "postal" => self.postal_code.trim().to_string(),
            _ => String::new(),
        };
        (!text.is_empty()).then_some(text)
    }
}

/// A DevTools connection to the agent's browser (the browser endpoint, sessions flattened).
pub struct Cdp {
    ws: Ws,
    next: u64,
    events: VecDeque<Value>,
}

impl Cdp {
    /// Chrome's DevTools on 127.0.0.1:`port` (the browser container's relay, browser/devtools_relay.py); `hold`: the
    /// token of the payment holding the browser (the relay lets only this connection through meanwhile).
    pub async fn connect(port: u16, hold: Option<&str>) -> Result<Cdp, FillError> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let http =
            reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| protocol(e.to_string()))?;
        let mut asking = http.get(format!("http://127.0.0.1:{port}/json/version"));
        if let Some(token) = hold {
            asking = asking.header(HOLD_HEADER, token);
        }
        let version: Value = asking
            .send()
            .await
            .map_err(|_| FillError::Unreachable)?
            .json()
            .await
            .map_err(|_| FillError::Unreachable)?;
        let url = version.get("webSocketDebuggerUrl").and_then(Value::as_str).ok_or(FillError::Unreachable)?;
        // Chrome names its own address in the container: only the path is kept (the relay sends it on).
        let path = url.find("/devtools/").map(|i| &url[i..]).ok_or(FillError::Unreachable)?;
        let mut request =
            format!("ws://127.0.0.1:{port}{path}").into_client_request().map_err(|_| FillError::Unreachable)?;
        if let Some(token) = hold {
            let value = token.parse().map_err(|_| protocol("bad hold token"))?;
            request.headers_mut().insert(HOLD_HEADER, value);
        }
        let connecting = tokio_tungstenite::connect_async(request);
        let (ws, _) = tokio::time::timeout(CALL_TIMEOUT, connecting)
            .await
            .map_err(|_| FillError::Unreachable)?
            .map_err(|_| FillError::Unreachable)?;
        Ok(Cdp { ws, next: 0, events: VecDeque::new() })
    }

    async fn read_one(&mut self, deadline: tokio::time::Instant) -> Result<Option<Value>, FillError> {
        loop {
            let message = match tokio::time::timeout_at(deadline, self.ws.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(FillError::Unreachable),
                Ok(Some(Err(_))) => return Err(FillError::Unreachable),
                Ok(Some(Ok(m))) => m,
            };
            match message {
                WsMessage::Text(text) => {
                    return serde_json::from_str::<Value>(text.as_str()).map(Some).map_err(|e| protocol(e.to_string()))
                }
                WsMessage::Close(_) => return Err(FillError::Unreachable),
                _ => continue,
            }
        }
    }

    /// One command (in `session` when given); its result, or the browser's error message.
    pub async fn call(&mut self, method: &str, params: Value, session: Option<&str>) -> Result<Value, FillError> {
        self.next += 1;
        let id = self.next;
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            message["sessionId"] = json!(s);
        }
        self.ws.send(WsMessage::text(message.to_string())).await.map_err(|_| FillError::Unreachable)?;
        let deadline = tokio::time::Instant::now() + CALL_TIMEOUT;
        loop {
            let Some(answer) = self.read_one(deadline).await? else {
                return Err(protocol(format!("{method}: no answer")));
            };
            if answer.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = answer.get("error") {
                    let text = error.get("message").and_then(Value::as_str).unwrap_or("error");
                    return Err(protocol(format!("{method}: {text}")));
                }
                return Ok(answer.get("result").cloned().unwrap_or(json!({})));
            }
            if answer.get("method").is_some() {
                self.events.push_back(answer);
            }
        }
    }

    /// Events coming in for `wait`.
    async fn drain(&mut self, wait: Duration) -> Result<(), FillError> {
        let deadline = tokio::time::Instant::now() + wait;
        while let Some(event) = self.read_one(deadline).await? {
            if event.get("method").is_some() {
                self.events.push_back(event);
            }
        }
        Ok(())
    }

    /// An expression's value in a world (`context`) of a session.
    async fn evaluate(&mut self, session: &str, context: i64, expression: &str) -> Result<Value, FillError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expression, "contextId": context, "returnByValue": true, "awaitPromise": false}),
                Some(session),
            )
            .await?;
        if let Some(details) = result.get("exceptionDetails") {
            let text = details.get("text").and_then(Value::as_str).unwrap_or("exception");
            return Err(protocol(format!("script: {text}")));
        }
        Ok(result.get("result").and_then(|r| r.get("value")).cloned().unwrap_or(Value::Null))
    }
}

/// The agent's current tab, attached.
pub struct Page {
    pub cdp: Cdp,
    pub session: String,
}

fn frames_of(tree: &Value, out: &mut Vec<(String, String)>) {
    if let Some(frame) = tree.get("frame") {
        let id = frame.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let url = frame.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        if !id.is_empty() {
            out.push((id, url));
        }
    }
    if let Some(Value::Array(children)) = tree.get("childFrames") {
        for child in children {
            frames_of(child, out);
        }
    }
}

impl Page {
    /// The tab the agent is on: the only web page, else the visible one.
    pub async fn open(port: u16, hold: Option<&str>) -> Result<Page, FillError> {
        let mut cdp = Cdp::connect(port, hold).await?;
        let targets = cdp.call("Target.getTargets", json!({}), None).await?;
        let pages: Vec<String> = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
                    .filter(|t| {
                        let url = t.get("url").and_then(Value::as_str).unwrap_or("");
                        url.starts_with("https://") || url.starts_with("http://")
                    })
                    .filter_map(|t| t.get("targetId").and_then(Value::as_str).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut chosen: Option<String> = None;
        for target in &pages {
            let attached =
                cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await?;
            let session = attached.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
            if pages.len() == 1 {
                chosen = Some(session);
                break;
            }
            let visible = cdp
                .call(
                    "Runtime.evaluate",
                    json!({"expression": "document.visibilityState", "returnByValue": true}),
                    Some(&session),
                )
                .await
                .ok()
                .and_then(|r| r.get("result").and_then(|r| r.get("value")).and_then(Value::as_str).map(str::to_string));
            if chosen.is_none() && visible.as_deref() == Some("visible") {
                chosen = Some(session);
            } else {
                let _ = cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None).await;
            }
        }
        let session = chosen.ok_or(FillError::NoPage)?;
        Ok(Page { cdp, session })
    }

    /// The page's address as Chrome knows it (its main frame), not as the page or the agent says.
    pub async fn url(&mut self) -> Result<String, FillError> {
        let session = self.session.clone();
        let tree = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await?;
        tree.get("frameTree")
            .and_then(|t| t.get("frame"))
            .and_then(|f| f.get("url"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| protocol("no main frame"))
    }

    /// Every frame of the page, with the session that reaches it: the page's own, then each out-of-process frame's.
    async fn frames(&mut self) -> Result<Vec<(String, String, String)>, FillError> {
        let mut sessions = vec![self.session.clone()];
        let mut out = Vec::new();
        let mut index = 0;
        while index < sessions.len() && index < MAX_SESSIONS {
            let session = sessions[index].clone();
            index += 1;
            let Ok(tree) = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await else { continue };
            let mut frames = Vec::new();
            frames_of(tree.get("frameTree").unwrap_or(&Value::Null), &mut frames);
            out.extend(frames.into_iter().map(|(id, url)| (session.clone(), id, url)));
            let auto = json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true});
            if self.cdp.call("Target.setAutoAttach", auto, Some(&session)).await.is_err() {
                continue;
            }
            self.cdp.drain(ATTACH_WAIT).await?;
            while let Some(event) = self.cdp.events.pop_front() {
                if event.get("method").and_then(Value::as_str) != Some("Target.attachedToTarget") {
                    continue;
                }
                let params = event.get("params").cloned().unwrap_or(Value::Null);
                let kind = params.get("targetInfo").and_then(|t| t.get("type")).and_then(Value::as_str);
                if let (Some("iframe"), Some(child)) = (kind, params.get("sessionId").and_then(Value::as_str)) {
                    if !sessions.iter().any(|s| s == child) {
                        sessions.push(child.to_string());
                    }
                }
            }
        }
        Ok(out)
    }

    /// The card's fields, frame by frame (a frame seen twice keeps the reading that found the most).
    pub async fn fields(&mut self) -> Result<Vec<Field>, FillError> {
        let mut found: Vec<(String, Vec<Field>)> = Vec::new();
        for (session, frame_id, frame_url) in self.frames().await? {
            let world = json!({"frameId": frame_id, "worldName": WORLD, "grantUniveralAccess": false});
            let Ok(created) = self.cdp.call("Page.createIsolatedWorld", world, Some(&session)).await else { continue };
            let Some(context) = created.get("executionContextId").and_then(Value::as_i64) else { continue };
            let expression = format!("{SCRIPT}\n;globalThis.__bipPay.detect()");
            let Ok(Value::Array(items)) = self.cdp.evaluate(&session, context, &expression).await else { continue };
            let fields: Vec<Field> = items
                .iter()
                .filter_map(|item| {
                    Some(Field {
                        session: session.clone(),
                        context,
                        frame_url: frame_url.clone(),
                        index: item.get("i")?.as_u64()?,
                        kind: item.get("kind")?.as_str()?.to_string(),
                        tag: item.get("tag").and_then(Value::as_str).unwrap_or("input").to_string(),
                        fmt: item.get("fmt").and_then(Value::as_str).unwrap_or("").to_string(),
                        empty: item.get("empty").and_then(Value::as_bool).unwrap_or(true),
                    })
                })
                .collect();
            match found.iter_mut().find(|(id, _)| *id == frame_id) {
                Some((_, kept)) if kept.len() >= fields.len() => {}
                Some((_, kept)) => *kept = fields,
                None => found.push((frame_id, fields)),
            }
        }
        // One field of each kind: the first found (the page's own before a frame's).
        let mut out: Vec<Field> = Vec::new();
        for field in found.into_iter().flat_map(|(_, f)| f) {
            if !out.iter().any(|f| f.kind == field.kind) {
                out.push(field);
            }
        }
        Ok(out)
    }

    async fn object(&mut self, field: &Field) -> Option<String> {
        let result = self
            .cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": format!("globalThis.__bipPay.el({})", field.index), "contextId": field.context}),
                Some(&field.session),
            )
            .await
            .ok()?;
        result.get("result").and_then(|r| r.get("objectId")).and_then(Value::as_str).map(str::to_string)
    }

    async fn holds(&mut self, field: &Field, text: &str) -> bool {
        let digits_only = matches!(field.kind.as_str(), "number" | "exp" | "exp_month" | "exp_year" | "csc");
        let expression = format!("globalThis.__bipPay.holds({}, {digits_only})", field.index);
        let wanted = if digits_only {
            text.chars().filter(char::is_ascii_digit).collect::<String>()
        } else {
            text.trim().to_lowercase()
        };
        matches!(self.cdp.evaluate(&field.session, field.context, &expression).await, Ok(Value::String(v)) if v == wanted)
    }

    /// The card typed into its fields: the kinds filled, in order. A postcode the page already holds is left alone.
    pub async fn fill(&mut self, fields: &[Field], card: &CardValues) -> Result<Vec<String>, FillError> {
        let mut filled = Vec::new();
        for field in fields {
            let Some(text) = card.text_for(field) else { continue };
            if field.kind == "postal" && !field.empty {
                continue;
            }
            let ok = if field.tag == "select" {
                let expression =
                    format!("globalThis.__bipPay.choose({}, {}, {})", field.index, card.exp_month, card.exp_year);
                self.cdp.evaluate(&field.session, field.context, &expression).await? == json!(true)
            } else {
                self.type_into(field, &text).await?
            };
            if ok {
                filled.push(field.kind.clone());
            }
        }
        Ok(filled)
    }

    /// The card's fields emptied (a payment given up, or left unfinished): nothing of the card stays in the page.
    pub async fn clear(&mut self) -> Result<usize, FillError> {
        let fields = self.fields().await?;
        let mut cleared = 0;
        for field in fields.iter().filter(|f| f.tag != "select") {
            let set = format!("globalThis.__bipPay.set({}, '')", field.index);
            if self.cdp.evaluate(&field.session, field.context, &set).await.is_ok() {
                cleared += 1;
            }
        }
        Ok(cleared)
    }

    /// An isolated world in the page's main frame, with the script loaded.
    async fn main_world(&mut self) -> Result<i64, FillError> {
        let session = self.session.clone();
        let tree = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await?;
        let frame = tree
            .get("frameTree")
            .and_then(|t| t.get("frame"))
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .ok_or_else(|| protocol("no main frame"))?
            .to_string();
        let world = json!({"frameId": frame, "worldName": WORLD, "grantUniveralAccess": false});
        let created = self.cdp.call("Page.createIsolatedWorld", world, Some(&session)).await?;
        let context = created.get("executionContextId").and_then(Value::as_i64).ok_or_else(|| protocol("no world"))?;
        self.cdp.evaluate(&session, context, SCRIPT).await?;
        Ok(context)
    }

    /// The page's pay button clicked (the one showing `wanted`, else a « pay » button, the amount helping): its
    /// text, or None when there is no such button.
    pub async fn click_pay(&mut self, wanted: &str, amount_digits: &str) -> Result<Option<String>, FillError> {
        let context = self.main_world().await?;
        let session = self.session.clone();
        let find = format!(
            "globalThis.__bipPay.findButton({}, {})",
            serde_json::to_string(wanted).unwrap_or_default(),
            serde_json::to_string(amount_digits).unwrap_or_default()
        );
        let Value::String(text) = self.cdp.evaluate(&session, context, &find).await? else { return Ok(None) };
        self.cdp.evaluate(&session, context, "globalThis.__bipPay.clickButton()").await?;
        Ok(Some(text))
    }

    /// The bank asks the person to confirm (3-D Secure): a frame of an authentication server, or the page says so.
    pub async fn bank_asks(&mut self) -> bool {
        let frames = self.frames().await.unwrap_or_default();
        let bank = regex::Regex::new(
            r"(?i)3ds|threeds|/acs|acs\.|challenge|authenticat|securecode|verifiedbyvisa|cardinalcommerce|3dsecure|3d-secure",
        )
        .ok();
        if frames.iter().any(|(_, _, url)| bank.as_ref().is_some_and(|b| b.is_match(url))) {
            return true;
        }
        let Ok(context) = self.main_world().await else { return false };
        let session = self.session.clone();
        matches!(self.cdp.evaluate(&session, context, "globalThis.__bipPay.bankAsks()").await, Ok(Value::Bool(true)))
    }

    async fn type_into(&mut self, field: &Field, text: &str) -> Result<bool, FillError> {
        let prepare = format!("globalThis.__bipPay.prepare({})", field.index);
        self.cdp.evaluate(&field.session, field.context, &prepare).await?;
        let object = self.object(field).await;
        if let Some(object) = &object {
            let _ = self.cdp.call("DOM.focus", json!({"objectId": object}), Some(&field.session)).await;
        }
        // Typed where the field lives (an out-of-process frame has its own input), else through the page.
        let typed = self.cdp.call("Input.insertText", json!({"text": text}), Some(&field.session)).await;
        if typed.is_err() && field.session != self.session {
            let page = self.session.clone();
            let _ = self.cdp.call("Input.insertText", json!({"text": text}), Some(&page)).await;
        }
        let mut ok = self.holds(field, text).await;
        if !ok {
            if let Some(object) = &object {
                let set = json!({
                    "objectId": object,
                    "functionDeclaration": "function (value) { const p = globalThis.__bipPay; return p.set(p.els.indexOf(this), value); }",
                    "arguments": [{"value": text}],
                    "returnByValue": true,
                });
                let _ = self.cdp.call("Runtime.callFunctionOn", set, Some(&field.session)).await;
                ok = self.holds(field, text).await;
            }
        }
        let blur = format!("globalThis.__bipPay.blur({})", field.index);
        let _ = self.cdp.evaluate(&field.session, field.context, &blur).await;
        Ok(ok)
    }
}

/// `https://host[:port]/…` → (scheme, host); None for anything that is not a web page.
pub fn origin_of(url: &str) -> Option<(String, String)> {
    let parsed = url::Url::parse(url).ok()?;
    let scheme = parsed.scheme().to_string();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let host = parsed.host_str()?.trim_end_matches('.').to_lowercase();
    Some((scheme, host))
}

/// A page a card may be typed into: https, or plain http on this machine's own names (a test page).
pub fn secure(scheme: &str, host: &str) -> bool {
    scheme == "https" || matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(kind: &str, fmt: &str) -> Field {
        Field {
            session: "s".into(),
            context: 1,
            frame_url: String::new(),
            index: 0,
            kind: kind.into(),
            tag: "input".into(),
            fmt: fmt.into(),
            empty: true,
        }
    }

    #[test]
    fn each_field_gets_its_text() {
        let card = CardValues {
            number: "4242424242424242".into(),
            exp_month: 3,
            exp_year: 2030,
            csc: "123".into(),
            name: "Alex  Martin Dupont".into(),
            postal_code: String::new(),
        };
        assert_eq!(card.text_for(&field("exp", "MM/YY")).as_deref(), Some("03/30"));
        assert_eq!(card.text_for(&field("exp", "MM/YYYY")).as_deref(), Some("03/2030"));
        assert_eq!(card.text_for(&field("exp", "MMYY")).as_deref(), Some("0330"));
        assert_eq!(card.text_for(&field("exp_year", "YY")).as_deref(), Some("30"));
        assert_eq!(card.text_for(&field("exp_year", "YYYY")).as_deref(), Some("2030"));
        assert_eq!(card.text_for(&field("given_name", "")).as_deref(), Some("Alex"));
        assert_eq!(card.text_for(&field("family_name", "")).as_deref(), Some("Martin Dupont"));
        assert_eq!(card.text_for(&field("postal", "")), None);
    }

    #[test]
    fn pages_cards_may_be_typed_into() {
        assert_eq!(origin_of("https://WWW.Shop.example:8443/pay?x"), Some(("https".into(), "www.shop.example".into())));
        assert_eq!(origin_of("chrome://newtab"), None);
        assert!(secure("https", "shop.example"));
        assert!(!secure("http", "shop.example"));
        assert!(secure("http", "127.0.0.1"));
    }
}
