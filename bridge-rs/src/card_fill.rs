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
//!
//! The card is never typed into the document the agent prepared (security audit 2026-11-11, BV-1): an agent misled by
//! a page could have run script in it before asking (a listener copying what is typed, fields of its own found first).
//! Under the payment's hold, `fresh` closes every other tab, removes the site's service workers and reloads the page
//! from the network; the fields are looked for in that new document only, and only in frames of the page's own site,
//! of the card's sites or of a known payment provider (`Frames`). After the payment the tab is left on an empty page
//! (`blank`) unless the page moved on to a new document by itself.

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
/// How often a page being reloaded or left is looked at.
const POLL: Duration = Duration::from_millis(200);
/// Marks the element it is called on (in the card script's world: the iframe holding the card's fields) as part of
/// the card's form, for `findButton`.
const ANCHOR: &str = "function () { globalThis.__bipPay.anchor(this); return true; }";

/// The payment providers whose frames hold a card form inside a shop's page (hosted fields: Stripe Elements, Adyen,
/// Checkout.com Frames, Mollie, PayPlug, Braintree, PayPal, Square, Shopify, Lyra/PayZen/SystemPay, Stancer,
/// Worldline, Recurly, Chargebee): a frame on one of these sites (or below one) may get the card. Any other frame of
/// another site than the page's own and the card's is left alone; a page whose card form is only there ends as a
/// hand-over to the person (card_pay.rs).
pub const PROVIDERS: &[&str] = &[
    "js.stripe.com",
    "adyen.com",
    "checkout.com",
    "mollie.com",
    "payplug.com",
    "braintreegateway.com",
    "paypal.com",
    "squarecdn.com",
    "squareup.com",
    "pci.shopifyinc.com",
    "lyra.com",
    "payzen.eu",
    "systempay.fr",
    "stancer.com",
    "worldline-solutions.com",
    "recurly.com",
    "chargebee.com",
];

/// Which frames' fields a card may be typed into: the page's own site (its host, a subdomain of it, `www.` aside),
/// one of the card's sites (none: no more than the page's own), a known payment provider (`PROVIDERS`). Only https
/// frames (or a test page on this machine), never one without a web address (about:blank, data:, blob:).
#[derive(Debug, Clone, PartialEq)]
pub struct Frames {
    top: String,
    sites: Vec<String>,
}

fn at_or_below(host: &str, site: &str) -> bool {
    !site.is_empty() && (host == site || host.ends_with(&format!(".{site}")))
}

impl Frames {
    /// The rule for the page at `top_url` and a card for `sites`.
    pub fn new(top_url: &str, sites: &[String]) -> Frames {
        let top = origin_of(top_url).map(|(_, host)| host).unwrap_or_default();
        let sites = sites.iter().map(|s| s.trim().trim_end_matches('.').to_lowercase()).filter(|s| !s.is_empty());
        Frames { top, sites: sites.collect() }
    }

    pub fn allows(&self, frame_url: &str) -> bool {
        let Some((scheme, host)) = origin_of(frame_url) else { return false };
        if !secure(&scheme, &host) {
            return false;
        }
        let own = self.top.strip_prefix("www.").unwrap_or(&self.top);
        at_or_below(&host, own)
            || self.sites.iter().any(|s| at_or_below(&host, s))
            || PROVIDERS.iter().any(|p| at_or_below(&host, p))
    }
}

/// One field of each kind, from the frames `rule` allows (all of them when None): the first found, the page's own
/// before a frame's.
pub fn pick(found: &[Field], rule: Option<&Frames>) -> Vec<Field> {
    let mut out: Vec<Field> = Vec::new();
    for field in found.iter().filter(|f| rule.is_none_or(|r| r.allows(&f.frame_url))) {
        if !out.iter().any(|f| f.kind == field.kind) {
            out.push(field.clone());
        }
    }
    out
}

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
    /// The payment was stopped meanwhile (cancel_payment, payments suspended): nothing more was typed.
    #[error("the payment was stopped")]
    Stopped,
    /// A card field would not keep the keyboard's focus (a dialog or a widget of the page took it): nothing was typed
    /// into it, nor anywhere else.
    #[error("a card field would not take the keyboard's focus")]
    Focus,
}

/// Whether the payment typing the card still holds the browser (card_pay.rs): looked at before each step.
pub type Still<'a> = &'a (dyn Fn() -> bool + Sync);

/// A field the card was typed into: its frame and its node (Chrome's backend node id, the same for every DevTools
/// connection while its document lives; 0 when Chrome did not say), to empty exactly it afterwards (`clear`).
#[derive(Debug, Clone, PartialEq)]
pub struct Typed {
    pub frame: String,
    pub node: i64,
}

/// Empties the field it is called on, the way a framework sees it; a select goes back to its first option.
const EMPTY_FIELD: &str = "function () { if (this.tagName === 'SELECT') { this.selectedIndex = 0; } else { \
const proto = this instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; \
Object.getOwnPropertyDescriptor(proto, 'value').set.call(this, ''); } \
this.dispatchEvent(new Event('input', {bubbles: true})); this.dispatchEvent(new Event('change', {bubbles: true})); \
return true; }";

fn protocol(text: impl Into<String>) -> FillError {
    FillError::Protocol(text.into())
}

/// One field found: its frame's world, its index there, its kind (number, exp, exp_month, exp_year, csc, name,
/// given_name, family_name, postal) and how the expiry is written (`MM/YY`, `MM/YYYY`, `MMYY`; `YY`, `YYYY`).
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub session: String,
    pub context: i64,
    /// Its frame's id (the same for every DevTools connection).
    pub frame: String,
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
    /// While the bridge reloads or leaves the payment page: a page's « leave this page? » is answered yes (a person
    /// who reloads would answer it too), so it can't hold the reload or the blank page up.
    leave: bool,
}

impl Cdp {
    /// Chrome's DevTools on 127.0.0.1:`port` (the browser container's relay, browser/devtools_relay.py); `hold`: the
    /// token of the payment holding the browser (the relay lets only this connection through meanwhile).
    pub async fn connect(port: u16, hold: Option<&str>) -> Result<Cdp, FillError> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let http =
            reqwest::Client::builder().timeout(Duration::from_secs(5)).build().map_err(|e| protocol(e.to_string()))?;
        let secret = crate::browsers::devtools_secret(port);
        let mut asking = http.get(format!("http://127.0.0.1:{port}/json/version"));
        if let Some(secret) = &secret {
            asking = asking.header(crate::browsers::CDP_HEADER, secret);
        }
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
        if let Some(secret) = secret {
            let value = secret.parse().map_err(|_| protocol("bad DevTools secret"))?;
            request.headers_mut().insert(crate::browsers::CDP_HEADER, value);
        }
        let connecting = tokio_tungstenite::connect_async(request);
        let (ws, _) = tokio::time::timeout(CALL_TIMEOUT, connecting)
            .await
            .map_err(|_| FillError::Unreachable)?
            .map_err(|_| FillError::Unreachable)?;
        Ok(Cdp { ws, next: 0, events: VecDeque::new(), leave: false })
    }

    /// An event read while waiting: a « leave this page? » answered when `leave`, everything else kept for `frames`.
    async fn note(&mut self, event: Value) -> Result<(), FillError> {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "Page.javascriptDialogOpening" {
            let kind = event.get("params").and_then(|p| p.get("type")).and_then(Value::as_str);
            if self.leave && kind == Some("beforeunload") {
                self.next += 1;
                let mut answer =
                    json!({"id": self.next, "method": "Page.handleJavaScriptDialog", "params": {"accept": true}});
                if let Some(session) = event.get("sessionId") {
                    answer["sessionId"] = session.clone();
                }
                self.ws.send(WsMessage::text(answer.to_string())).await.map_err(|_| FillError::Unreachable)?;
            }
            return Ok(());
        }
        if method.starts_with("Target.") {
            self.events.push_back(event);
        }
        Ok(())
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
                self.note(answer).await?;
            }
        }
    }

    /// Events coming in for `wait`.
    async fn drain(&mut self, wait: Duration) -> Result<(), FillError> {
        let deadline = tokio::time::Instant::now() + wait;
        while let Some(event) = self.read_one(deadline).await? {
            if event.get("method").is_some() {
                self.note(event).await?;
            }
        }
        Ok(())
    }

    /// An expression's value in a world (`context`) of a session.
    pub async fn evaluate(&mut self, session: &str, context: i64, expression: &str) -> Result<Value, FillError> {
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
    /// The tab's target id (the same across reloads and navigations).
    pub target: String,
    /// The out-of-process frames' sessions found so far: Chrome reports each one once per connection, and a page is
    /// looked at several times while its form appears (card_pay.rs).
    children: Vec<String>,
    /// The fields `fill` typed into (or began to), in order: what `clear` empties afterwards.
    pub typed: Vec<Typed>,
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
        let mut chosen: Option<(String, String)> = None;
        for target in &pages {
            let attached =
                cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await?;
            let session = attached.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
            if pages.len() == 1 {
                chosen = Some((session, target.clone()));
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
                chosen = Some((session, target.clone()));
            } else {
                let _ = cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None).await;
            }
        }
        let (session, target) = chosen.ok_or(FillError::NoPage)?;
        Ok(Page { cdp, session, target, children: Vec::new(), typed: Vec::new() })
    }

    /// The tab `target` (the one a card was typed into), whatever it shows now; `NoPage` when it is closed.
    pub async fn attach(port: u16, hold: Option<&str>, target: &str) -> Result<Page, FillError> {
        let mut cdp = Cdp::connect(port, hold).await?;
        let attached = cdp
            .call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None)
            .await
            .map_err(|_| FillError::NoPage)?;
        let session = attached.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        if session.is_empty() {
            return Err(FillError::NoPage);
        }
        Ok(Page { cdp, session, target: target.to_string(), children: Vec::new(), typed: Vec::new() })
    }

    /// The main frame's document: its loader id changes with each new document (a reload, a navigation), not when a
    /// one-page site only changes its address.
    pub async fn loader(&mut self) -> Result<String, FillError> {
        let session = self.session.clone();
        let tree = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await?;
        Ok(tree
            .get("frameTree")
            .and_then(|t| t.get("frame"))
            .and_then(|f| f.get("loaderId"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }

    /// A document the agent never touched, for the payment (only under the payment's hold, card_pay.rs). Every other
    /// tab is closed first (an opener, a popup or a sibling of the same site could reach into the reloaded page and
    /// plant its script again), the site's service workers are removed (one registered from the agent's script could
    /// answer the reload, or the payment's own request, with what it likes), then the page is reloaded bypassing the
    /// cache and its new document waited for (`wait` at most). Its address once loaded.
    ///
    /// Nothing of the agent's runs in it: scripts are per document, so what the agent ran in the old one is gone; the
    /// agent's tools can't register a script for every new document (the browser's MCP server runs without its
    /// cdp-functions and dynamic-hooks sections, browser/mcp.sh), and what a DevTools connection registered dies with
    /// it (the relay closed them all when the hold started); its tool calls wait (hold_gate.py), and card_pay.rs waits
    /// for those already running before calling this. `Stopped` as soon as `still` says the payment was stopped.
    pub async fn fresh(&mut self, wait: Duration, still: Still<'_>) -> Result<String, FillError> {
        for other in self.other_tabs().await? {
            let _ = self.cdp.call("Target.closeTarget", json!({"targetId": other}), None).await;
        }
        let mut tries = 0;
        while !self.other_tabs().await?.is_empty() {
            tries += 1;
            if tries > 15 {
                return Err(protocol("another tab would not close"));
            }
            self.cdp.drain(POLL).await?;
        }
        let url = self.url().await?;
        let origin = url::Url::parse(&url).map(|u| u.origin().ascii_serialization()).map_err(|_| protocol("no origin"))?;
        let session = self.session.clone();
        let clear = json!({"origin": origin, "storageTypes": "service_workers"});
        self.cdp.call("Storage.clearDataForOrigin", clear, Some(&session)).await?;
        self.cdp.leave = true;
        self.cdp.call("Page.enable", json!({}), Some(&session)).await?;
        let before = self.loader().await?;
        self.cdp.call("Page.reload", json!({"ignoreCache": true}), Some(&session)).await?;
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if !still() {
                self.cdp.leave = false;
                return Err(FillError::Stopped);
            }
            if self.loader().await.is_ok_and(|now| now != before) {
                let state = self
                    .cdp
                    .call(
                        "Runtime.evaluate",
                        json!({"expression": "document.readyState", "returnByValue": true}),
                        Some(&session),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.get("result")?.get("value")?.as_str().map(str::to_string));
                if matches!(state.as_deref(), Some("interactive" | "complete")) {
                    break;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                self.cdp.leave = false;
                return Err(protocol("the page did not load again"));
            }
            self.cdp.drain(POLL).await?;
        }
        self.cdp.leave = false;
        self.url().await
    }

    /// The browser's other tabs (and windows, popups): their target ids.
    async fn other_tabs(&mut self) -> Result<Vec<String>, FillError> {
        let targets = self.cdp.call("Target.getTargets", json!({}), None).await?;
        Ok(targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|t| t.get("type").and_then(Value::as_str) == Some("page"))
                    .filter_map(|t| t.get("targetId").and_then(Value::as_str))
                    .filter(|id| *id != self.target)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The tab left on an empty page: the document the card was typed into is gone for good (its fields emptied
    /// first, `clear`). When the page won't leave, the tab is closed.
    pub async fn blank(&mut self) -> Result<(), FillError> {
        let session = self.session.clone();
        self.cdp.leave = true;
        let _ = self.cdp.call("Page.enable", json!({}), Some(&session)).await;
        let navigated = self.cdp.call("Page.navigate", json!({"url": "about:blank"}), Some(&session)).await.is_ok();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while navigated && tokio::time::Instant::now() < deadline {
            if self.url().await.is_ok_and(|u| u == "about:blank") {
                self.cdp.leave = false;
                return Ok(());
            }
            self.cdp.drain(POLL).await?;
        }
        self.cdp.leave = false;
        let target = self.target.clone();
        self.cdp.call("Target.closeTarget", json!({"targetId": target}), None).await.map(|_| ())
    }

    /// The tab's history forgotten (`Page.resetNavigationHistory`: only the page shown is left in it), so that
    /// « back » can't bring the document the card was typed into again, from the back/forward cache with its values or
    /// fetched again with its form's values restored (fix review 11/11, B2). Done before the hold is lifted.
    pub async fn forget_history(&mut self) -> Result<(), FillError> {
        let session = self.session.clone();
        self.cdp.call("Page.resetNavigationHistory", json!({}), Some(&session)).await.map(|_| ())
    }

    /// The tab closed (when its history can't be forgotten).
    pub async fn close(&mut self) -> Result<(), FillError> {
        let target = self.target.clone();
        self.cdp.call("Target.closeTarget", json!({"targetId": target}), None).await.map(|_| ())
    }

    /// The main frame's id.
    async fn main_frame(&mut self) -> Result<String, FillError> {
        let session = self.session.clone();
        let tree = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await?;
        tree.get("frameTree")
            .and_then(|t| t.get("frame"))
            .and_then(|f| f.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| protocol("no main frame"))
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
    pub async fn frames(&mut self) -> Result<Vec<(String, String, String)>, FillError> {
        let mut sessions = vec![self.session.clone()];
        sessions.extend(self.children.iter().cloned());
        let mut gone = Vec::new();
        let mut out = Vec::new();
        let mut index = 0;
        while index < sessions.len() && index < MAX_SESSIONS {
            let session = sessions[index].clone();
            index += 1;
            let Ok(tree) = self.cdp.call("Page.getFrameTree", json!({}), Some(&session)).await else {
                gone.push(session); // a frame closed or navigated away: its session ended
                continue;
            };
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
        let main = self.session.clone();
        self.children = sessions.into_iter().filter(|s| *s != main && !gone.contains(s)).collect();
        Ok(out)
    }

    /// The card's fields, one of each kind (`pick`), from every frame (to look for them, or empty them).
    pub async fn fields(&mut self) -> Result<Vec<Field>, FillError> {
        Ok(pick(&self.found().await?, None))
    }

    /// The card's fields a card may be typed into: one of each kind from the frames `rule` allows.
    pub async fn fields_in(&mut self, rule: &Frames) -> Result<Vec<Field>, FillError> {
        Ok(pick(&self.found().await?, Some(rule)))
    }

    /// Every card field of every frame, frame by frame (a frame seen twice keeps the reading that found the most).
    pub async fn found(&mut self) -> Result<Vec<Field>, FillError> {
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
                        frame: frame_id.clone(),
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
        Ok(found.into_iter().flat_map(|(_, f)| f).collect())
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
    /// Before each field, `still` says whether the payment still holds the browser: else `Stopped`, nothing more typed
    /// (cancel_payment or « Suspendre tous les paiements » came while the page was reloaded or typed into).
    pub async fn fill(&mut self, fields: &[Field], card: &CardValues, still: Still<'_>) -> Result<Vec<String>, FillError> {
        // The page counts as the window in front (a browser on a screen nobody clicked can report no focus at all):
        // `focused` then tells exactly which frame and field the keyboard is in. Only for this connection.
        let mut sessions = vec![self.session.clone()];
        for field in fields {
            if !sessions.contains(&field.session) {
                sessions.push(field.session.clone());
            }
        }
        for session in &sessions {
            let _ = self.cdp.call("Emulation.setFocusEmulationEnabled", json!({"enabled": true}), Some(session)).await;
        }
        let mut filled = Vec::new();
        for field in fields {
            let Some(text) = card.text_for(field) else { continue };
            if field.kind == "postal" && !field.empty {
                continue;
            }
            if !still() {
                return Err(FillError::Stopped);
            }
            self.remember(field).await;
            let ok = if field.tag == "select" {
                let expression =
                    format!("globalThis.__bipPay.choose({}, {}, {})", field.index, card.exp_month, card.exp_year);
                self.cdp.evaluate(&field.session, field.context, &expression).await? == json!(true)
            } else {
                self.type_into(field, &text, still).await?
            };
            if ok {
                filled.push(field.kind.clone());
            }
        }
        Ok(filled)
    }

    /// The keyboard's focus is in `field` and its frame (`focused` in the card script).
    async fn has_focus(&mut self, field: &Field) -> bool {
        let expression = format!("globalThis.__bipPay.focused({})", field.index);
        matches!(self.cdp.evaluate(&field.session, field.context, &expression).await, Ok(Value::Bool(true)))
    }

    /// A field about to be typed into, kept in `typed` (its node asked to Chrome) before anything goes into it.
    async fn remember(&mut self, field: &Field) {
        let mut node = 0;
        if let Some(object) = self.object(field).await {
            let described = self.cdp.call("DOM.describeNode", json!({"objectId": object}), Some(&field.session)).await;
            node = described.ok().and_then(|d| d.get("node")?.get("backendNodeId")?.as_i64()).unwrap_or(0);
        }
        self.typed.push(Typed { frame: field.frame.clone(), node });
    }

    /// The fields the card was typed into (`typed`, kept by `fill`) emptied, selects back to their first option: a
    /// payment given up, or left unfinished. Exactly those, found again by their frame and node in whatever
    /// connection this is (fix review 11/11, R4: one field of each kind from any frame was emptied, not always the one
    /// typed into). A field whose node Chrome did not give: every card field of the page emptied instead. Gone fields
    /// (the page moved on) are left. How many were emptied.
    pub async fn clear(&mut self, typed: &[Typed]) -> Result<usize, FillError> {
        if typed.is_empty() {
            return Ok(0);
        }
        let mut cleared = 0;
        if typed.iter().any(|t| t.node == 0) {
            for field in self.found().await? {
                let set = format!("globalThis.__bipPay.set({}, '')", field.index);
                if field.tag != "select" && self.cdp.evaluate(&field.session, field.context, &set).await.is_ok() {
                    cleared += 1;
                }
            }
        }
        let frames = self.frames().await?;
        for field in typed.iter().filter(|t| t.node != 0) {
            // A frame is reported by its parent's session and, out of process, by its own: the one that reaches it.
            for (session, _, _) in frames.iter().filter(|(_, id, _)| *id == field.frame) {
                if self.empty_node(session, field).await {
                    cleared += 1;
                    break;
                }
            }
        }
        Ok(cleared)
    }

    /// The field `field` emptied through `session`, in a world of its frame: whether it was.
    async fn empty_node(&mut self, session: &str, field: &Typed) -> bool {
        let world = json!({"frameId": field.frame, "worldName": WORLD, "grantUniveralAccess": false});
        let Ok(created) = self.cdp.call("Page.createIsolatedWorld", world, Some(session)).await else { return false };
        let Some(context) = created.get("executionContextId").and_then(Value::as_i64) else { return false };
        let resolve = json!({"backendNodeId": field.node, "executionContextId": context});
        let Ok(resolved) = self.cdp.call("DOM.resolveNode", resolve, Some(session)).await else { return false };
        let Some(object) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
            return false;
        };
        let empty = json!({"objectId": object, "functionDeclaration": EMPTY_FIELD, "returnByValue": true});
        self.cdp.call("Runtime.callFunctionOn", empty, Some(session)).await.is_ok()
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

    /// The card form's pay button clicked (the one showing `wanted`, else a « pay » button, the amount helping): its
    /// text, or None when there is no such button. Only a button of the form that holds the card's fields (`rule`:
    /// those it was typed into), or around the frame holding them, else the provider frame's own pay button; never a
    /// link, nor a button elsewhere on the page (fix review 11/11, B2: any button or link could be pressed by its text,
    /// « Aide », and a link took the tab to a new document with the card still typed in the one before).
    pub async fn click_pay(
        &mut self,
        wanted: &str,
        amount_digits: &str,
        rule: &Frames,
    ) -> Result<Option<String>, FillError> {
        let card = pick(&self.found().await?, Some(rule));
        let main = self.main_frame().await?;
        let context = self.main_world().await?;
        let session = self.session.clone();
        self.cdp.evaluate(&session, context, "globalThis.__bipPay.detect()").await?; // the page's own card fields
        // The frames holding the card's fields, by their owner element in the page (an iframe, around which the form is).
        let mut frames: Vec<&str> = card.iter().filter(|f| f.frame != main).map(|f| f.frame.as_str()).collect();
        frames.dedup();
        for frame in frames {
            let Ok(owner) = self.cdp.call("DOM.getFrameOwner", json!({"frameId": frame}), Some(&session)).await else {
                continue;
            };
            let Some(node) = owner.get("backendNodeId").and_then(Value::as_i64) else { continue };
            let resolve = json!({"backendNodeId": node, "executionContextId": context});
            let Ok(resolved) = self.cdp.call("DOM.resolveNode", resolve, Some(&session)).await else { continue };
            let Some(object) = resolved.get("object").and_then(|o| o.get("objectId")).and_then(Value::as_str) else {
                continue;
            };
            let mark = json!({"objectId": object, "functionDeclaration": ANCHOR, "returnByValue": true});
            let _ = self.cdp.call("Runtime.callFunctionOn", mark, Some(&session)).await;
        }
        let find = format!(
            "globalThis.__bipPay.findButton({}, {})",
            serde_json::to_string(wanted).unwrap_or_default(),
            serde_json::to_string(amount_digits).unwrap_or_default()
        );
        if let Value::String(text) = self.cdp.evaluate(&session, context, &find).await? {
            self.cdp.evaluate(&session, context, "globalThis.__bipPay.clickButton()").await?;
            return Ok(Some(text));
        }
        // A provider whose pay button is in its own frame, with the card's number (its world found the fields).
        for field in card.iter().filter(|f| f.kind == "number" && f.frame != main) {
            if let Ok(Value::String(text)) = self.cdp.evaluate(&field.session, field.context, &find).await {
                self.cdp.evaluate(&field.session, field.context, "globalThis.__bipPay.clickButton()").await?;
                return Ok(Some(text));
            }
        }
        Ok(None)
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

    /// `text` typed into `field`, as a person would: only once the keyboard's focus is verified to be in that field
    /// and its frame (focused, else focused again once, else `Focus` and nothing typed): `Input.insertText` types into
    /// whatever has the focus, a dialog that traps it or a chat widget's box in another frame (fix review 11/11, R6).
    async fn type_into(&mut self, field: &Field, text: &str, still: Still<'_>) -> Result<bool, FillError> {
        let prepare = format!("globalThis.__bipPay.prepare({})", field.index);
        let object = self.object(field).await;
        let mut focused = false;
        for _ in 0..2 {
            self.cdp.evaluate(&field.session, field.context, &prepare).await?;
            if let Some(object) = &object {
                let _ = self.cdp.call("DOM.focus", json!({"objectId": object}), Some(&field.session)).await;
            }
            if self.has_focus(field).await {
                focused = true;
                break;
            }
        }
        if !focused {
            return Err(FillError::Focus);
        }
        if !still() {
            return Err(FillError::Stopped);
        }
        // Typed where the field lives (an out-of-process frame has its own input), else through the page (whose
        // focused frame is the field's, just verified).
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
            frame: String::new(),
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

    fn in_frame(kind: &str, frame_url: &str) -> Field {
        Field { frame_url: frame_url.into(), ..field(kind, "") }
    }

    #[test]
    fn a_card_goes_only_to_the_page_s_site_the_card_s_sites_and_known_providers() {
        let shop = Frames::new("https://www.shop.example/checkout?step=pay", &["Shop.example.".into()]);
        assert!(shop.allows("https://www.shop.example/checkout"));
        assert!(shop.allows("https://pay.shop.example/frame")); // below the page's own site, `www.` aside
        assert!(shop.allows("https://js.stripe.com/v3/elements-inner-card.html"));
        assert!(shop.allows("https://checkoutshopper-live.adyen.com/checkoutshopper/securedfields/x.html"));
        assert!(!shop.allows("https://js.psp.example/f")); // a provider nobody knows
        assert!(!shop.allows("https://evil-stripe.com/f") && !shop.allows("https://stripe.com.evil.example/f"));
        assert!(!shop.allows("http://js.stripe.com/f")); // no https
        for no_address in ["about:blank", "about:srcdoc", "data:text/html,<input>", "blob:https://www.shop.example/1"] {
            assert!(!shop.allows(no_address), "{no_address}");
        }
        // The card's sites count too; a card for any site adds none.
        let other = Frames::new("https://tickets.example/pay", &["tickets.example".into(), "pay-tickets.example".into()]);
        assert!(other.allows("https://secure.pay-tickets.example/card"));
        assert!(!Frames::new("https://tickets.example/pay", &[]).allows("https://secure.pay-tickets.example/card"));
        // A test page on this machine: its own host and the card's sites.
        let local = Frames::new("http://127.0.0.1:8123/elements.html", &["localhost".into()]);
        assert!(local.allows("http://127.0.0.1:8123/x") && local.allows("http://localhost:9000/provider.html"));
        assert!(!Frames::new("http://127.0.0.1:8123/elements.html", &[]).allows("http://localhost:9000/provider.html"));
    }

    #[test]
    fn one_field_of_each_kind_from_the_frames_allowed() {
        let rule = Frames::new("https://shop.example/pay", &[]);
        let found = vec![
            in_frame("number", "https://js.psp.example/f"), // a frame of a site nobody knows, found first
            in_frame("name", "https://shop.example/pay"),
            in_frame("number", "https://js.stripe.com/v3/card"),
            in_frame("csc", "https://js.psp.example/f"),
            in_frame("name", "https://js.stripe.com/v3/card"),
        ];
        let kept: Vec<(String, String)> =
            pick(&found, Some(&rule)).into_iter().map(|f| (f.kind, f.frame_url)).collect();
        assert_eq!(
            kept,
            vec![
                ("name".to_string(), "https://shop.example/pay".to_string()),
                ("number".to_string(), "https://js.stripe.com/v3/card".to_string())
            ]
        );
        // Without a rule (to empty the fields): every frame, the first of each kind.
        let all: Vec<String> = pick(&found, None).into_iter().map(|f| f.frame_url).collect();
        assert_eq!(all, vec!["https://js.psp.example/f", "https://shop.example/pay", "https://js.psp.example/f"]);
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
