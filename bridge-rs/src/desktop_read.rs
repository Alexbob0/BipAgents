//! Reading the agent's page on a phone without touching it: where the agent acts (follow-zoom) and the page's text
//! (« Lecture »). The agent keeps browsing as a desktop browser; nothing here changes its tab's size, scroll or DOM.
//!
//! - **Where it acts**: the focused element (the field it types into, the button or link it just clicked: Chrome
//!   focuses them), else the page's main column, as a rectangle of the viewport. Read with one `Runtime.evaluate` in
//!   an *isolated world* of the tab's main frame (`Page.createIsolatedWorld`, as an extension's content script): the
//!   page's own scripts can neither see nor patch that world's globals, and the expression only reads
//!   `document.activeElement` and `getBoundingClientRect()`; it installs no listener and leaves nothing behind. The
//!   page view keeps one world per connection (a new one only after a navigation), polled about once a second and only
//!   while frames come (the page changed).
//! - **Lecture**: Chrome's `DOMSnapshot.captureSnapshot`, which runs no script at all in the page: its nodes, which of
//!   them are rendered and their `display`/`visibility`. The text is put together here (`reader`), headings,
//!   paragraphs, list items, quotes and images' descriptions, from the page's main content when it has one.
//!
//! Routes: `GET /v1/agents/{id}/desktop/focus`, `GET /v1/agents/{id}/desktop/reader` (bridge key) and
//! `GET /desktop/{ticket}/page/reader` (the page view's ticket). The page view's WebSocket also pushes the focus while
//! the phone follows the agent (desktop_page.rs).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::desktop_page::{cdp_port, is_blank, site, tabs, Cdp, Tab};
use crate::web::{self, ApiResult, HttpError};

/// Read in an isolated world: what has the focus (through open shadow roots), else the page's main column.
pub const FOCUS_JS: &str = "(() => {\
const v = {w: innerWidth, h: innerHeight};\
const box = e => { const r = e.getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; };\
let e = document.activeElement;\
while (e && e.shadowRoot && e.shadowRoot.activeElement) e = e.shadowRoot.activeElement;\
if (e && e !== document.body && e !== document.documentElement) {\
const tag = e.tagName.toLowerCase();\
const field = e.isContentEditable || tag === 'textarea' || tag === 'select' || (tag === 'input' && \
!/^(button|submit|reset|checkbox|radio|image|range|color|file|hidden)$/i.test(e.type || ''));\
return {v, r: box(e), kind: field ? 'field' : 'element'};\
}\
const m = document.querySelector('main, article, [role=main]');\
return m ? {v, r: box(m), kind: 'content'} : {v};\
})()";

/// Where the agent acts, in the viewport's CSS pixels (clipped to it).
#[derive(Debug, Clone, PartialEq)]
pub struct Focus {
    pub viewport: (f64, f64),
    /// (x, y, width, height, kind: field, element or content)
    pub target: Option<(f64, f64, f64, f64, String)>,
}

impl Focus {
    pub fn to_json(&self) -> Value {
        json!({
            "viewport": {"w": self.viewport.0, "h": self.viewport.1},
            "target": self.target.as_ref().map(|(x, y, w, h, kind)| json!({"x": x, "y": y, "w": w, "h": h, "kind": kind})),
        })
    }
}

/// The evaluation's value as a `Focus`: a target off screen (scrolled away) or of no size is none.
pub fn focus_from(value: &Value) -> Focus {
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).filter(|n| n.is_finite());
    let vw = num(value.pointer("/v/w")).unwrap_or(0.0).max(0.0);
    let vh = num(value.pointer("/v/h")).unwrap_or(0.0).max(0.0);
    let target = (|| {
        let r = value.get("r")?.as_array()?;
        let (x, y, w, h) = (num(r.first())?, num(r.get(1))?, num(r.get(2))?, num(r.get(3))?);
        let (left, top) = (x.max(0.0), y.max(0.0));
        let (right, bottom) = ((x + w).min(vw), (y + h).min(vh));
        if right - left < 2.0 || bottom - top < 2.0 {
            return None;
        }
        let kind = value
            .get("kind")
            .and_then(Value::as_str)
            .filter(|k| matches!(*k, "field" | "element" | "content"))
            .unwrap_or("element");
        let round = |n: f64| (n * 10.0).round() / 10.0;
        Some((round(left), round(top), round(right - left), round(bottom - top), kind.to_string()))
    })();
    Focus { viewport: (vw, vh), target }
}

/// The focus, read in `world` (an isolated world of the tab's main frame, made on first use and again after a
/// navigation took the previous one away).
pub async fn focus(cdp: &mut Cdp, world: &mut Option<i64>) -> Result<Focus, String> {
    for attempt in 0..2 {
        let context = match *world {
            Some(id) => id,
            None => {
                let id = isolated_world(cdp).await?;
                *world = Some(id);
                id
            }
        };
        let answer = cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": FOCUS_JS, "contextId": context, "returnByValue": true, "silent": true}),
            )
            .await;
        match answer {
            Ok(result) if result.get("exceptionDetails").is_none() => {
                return Ok(focus_from(result.pointer("/result/value").unwrap_or(&Value::Null)));
            }
            Ok(_) if attempt == 0 => *world = None,
            Err(_) if attempt == 0 => *world = None, // its world went with a navigation: a new one
            Ok(result) => return Err(result.to_string()),
            Err(error) => return Err(error),
        }
    }
    Err("no world".into())
}

async fn isolated_world(cdp: &mut Cdp) -> Result<i64, String> {
    let tree = cdp.call("Page.getFrameTree", json!({})).await?;
    let frame = tree.pointer("/frameTree/frame/id").and_then(Value::as_str).ok_or("no frame")?.to_string();
    let world = cdp.call("Page.createIsolatedWorld", json!({"frameId": frame, "worldName": "bipagents-view"})).await?;
    world.get("executionContextId").and_then(Value::as_i64).ok_or_else(|| "no world".to_string())
}

// -- « Lecture » ---------------------------------------------------------------------------------------------------

/// At most this much text in a reader view (a very long page is cut).
pub const READER_CHARS: usize = 200_000;
pub const READER_BLOCKS: usize = 2_000;

/// Elements whose content is never read: code, the page's menus, side and bottom bars, form controls.
const SKIP: [&str; 17] = [
    "SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "HEAD", "SVG", "CANVAS", "IFRAME", "OBJECT", "NAV", "ASIDE", "FOOTER",
    "BUTTON", "SELECT", "OPTION", "TEXTAREA", "DIALOG",
];
const SKIP_ROLES: [&str; 6] = ["navigation", "complementary", "contentinfo", "dialog", "alertdialog", "menu"];

/// One block of the reader view.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Paragraph(String),
    Item(u8, String),
    Quote(String),
    Image(String),
}

impl Block {
    pub fn to_json(&self) -> Value {
        match self {
            Block::Heading(level, text) => json!({"t": "h", "level": level, "text": text}),
            Block::Paragraph(text) => json!({"t": "p", "text": text}),
            Block::Item(depth, text) => json!({"t": "li", "depth": depth, "text": text}),
            Block::Quote(text) => json!({"t": "quote", "text": text}),
            Block::Image(alt) => json!({"t": "img", "text": alt}),
        }
    }
    fn text(&self) -> &str {
        match self {
            Block::Heading(_, t) | Block::Paragraph(t) | Block::Item(_, t) | Block::Quote(t) | Block::Image(t) => t,
        }
    }
}

/// What a block of the page is read as.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Heading(u8),
    Paragraph,
    Item(u8),
    Quote,
}

/// The first document of a `DOMSnapshot.captureSnapshot` answer, indexed.
struct Snapshot<'a> {
    strings: Vec<&'a str>,
    kinds: Vec<i64>,
    names: Vec<String>,
    values: Vec<i64>,
    attributes: Vec<Vec<(String, String)>>,
    children: Vec<Vec<usize>>,
    /// rendered nodes: (display, visibility)
    laid: HashMap<usize, (String, String)>,
}

impl<'a> Snapshot<'a> {
    fn parse(answer: &'a Value) -> Option<Snapshot<'a>> {
        let strings: Vec<&str> =
            answer.get("strings")?.as_array()?.iter().map(|s| s.as_str().unwrap_or_default()).collect();
        let document = answer.get("documents")?.as_array()?.first()?;
        let nodes = document.get("nodes")?;
        let ints = |key: &str| -> Vec<i64> {
            nodes
                .get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect())
                .unwrap_or_default()
        };
        let parents = ints("parentIndex");
        let count = parents.len();
        let text = |i: i64| -> &str { usize::try_from(i).ok().and_then(|i| strings.get(i)).copied().unwrap_or("") };
        let names: Vec<String> = ints("nodeName").into_iter().map(|i| text(i).to_uppercase()).collect();
        let attributes: Vec<Vec<(String, String)>> = match nodes.get("attributes").and_then(Value::as_array) {
            Some(lists) => lists
                .iter()
                .map(|list| {
                    let items: Vec<i64> = list
                        .as_array()
                        .map(|a| a.iter().map(|v| v.as_i64().unwrap_or(-1)).collect())
                        .unwrap_or_default();
                    items
                        .chunks(2)
                        .filter(|p| p.len() == 2)
                        .map(|p| (text(p[0]).to_lowercase(), text(p[1]).to_string()))
                        .collect()
                })
                .collect(),
            None => Vec::new(),
        };
        let mut children = vec![Vec::new(); count];
        for (i, &parent) in parents.iter().enumerate() {
            if let Some(list) = usize::try_from(parent).ok().and_then(|p| children.get_mut(p)) {
                list.push(i);
            }
        }
        let mut laid = HashMap::new();
        if let Some(layout) = document.get("layout") {
            let index = layout.get("nodeIndex").and_then(Value::as_array).cloned().unwrap_or_default();
            let styles = layout.get("styles").and_then(Value::as_array).cloned().unwrap_or_default();
            for (n, node) in index.iter().enumerate() {
                let Some(node) = node.as_u64().map(|n| n as usize) else { continue };
                let style = styles.get(n).and_then(Value::as_array);
                let at = |k: usize| style.and_then(|s| s.get(k)).and_then(Value::as_i64).map(text).unwrap_or("");
                laid.insert(node, (at(0).to_string(), at(1).to_string()));
            }
        }
        Some(Snapshot {
            kinds: ints("nodeType"),
            values: ints("nodeValue"),
            strings,
            names,
            attributes,
            children,
            laid,
        })
    }

    fn attribute(&self, node: usize, name: &str) -> Option<&str> {
        self.attributes.get(node)?.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn value(&self, node: usize) -> &str {
        self.values
            .get(node)
            .and_then(|&i| usize::try_from(i).ok())
            .and_then(|i| self.strings.get(i))
            .copied()
            .unwrap_or("")
    }

    fn skipped(&self, node: usize) -> bool {
        let name = self.names[node].as_str();
        SKIP.contains(&name)
            || name.starts_with("::")
            || self.attribute(node, "hidden").is_some()
            || self.attribute(node, "aria-hidden") == Some("true")
            || self.attribute(node, "role").is_some_and(|r| SKIP_ROLES.contains(&r.to_lowercase().as_str()))
    }

    /// The page's main content (`main`, `article`, `role=main`), when it has one with text.
    fn main(&self) -> Option<usize> {
        (0..self.names.len()).find(|&i| {
            self.kinds.get(i) == Some(&1)
                && (matches!(self.names[i].as_str(), "MAIN" | "ARTICLE") || self.attribute(i, "role") == Some("main"))
                && self.laid.contains_key(&i)
        })
    }
}

struct Reading<'s, 'a> {
    snap: &'s Snapshot<'a>,
    blocks: Vec<Block>,
    line: String,
    chars: usize,
    lists: u8,
}

impl Reading<'_, '_> {
    fn full(&self) -> bool {
        self.chars >= READER_CHARS || self.blocks.len() >= READER_BLOCKS
    }

    fn flush(&mut self, kind: Kind) {
        let text = self.line.split_whitespace().collect::<Vec<_>>().join(" ");
        self.line.clear();
        if text.is_empty() || self.full() || self.blocks.last().is_some_and(|b| b.text() == text) {
            return;
        }
        self.chars += text.len();
        self.blocks.push(match kind {
            Kind::Heading(level) => Block::Heading(level, text),
            Kind::Paragraph => Block::Paragraph(text),
            Kind::Item(depth) => Block::Item(depth, text),
            Kind::Quote => Block::Quote(text),
        });
    }

    fn walk(&mut self, node: usize, kind: Kind) {
        if self.full() {
            return;
        }
        let snap = self.snap;
        match snap.kinds.get(node).copied() {
            Some(3) => {
                if snap.laid.get(&node).is_some_and(|(_, visibility)| visibility != "hidden") {
                    self.line.push_str(snap.value(node));
                }
            }
            Some(1) => {
                if snap.skipped(node) {
                    return;
                }
                let name = snap.names[node].as_str();
                let display = snap.laid.get(&node).map(|(d, _)| d.as_str());
                match name {
                    "BR" => return self.line.push(' '),
                    "IMG" => {
                        let alt = snap.attribute(node, "alt").map(str::trim).unwrap_or("");
                        if display.is_some() && !alt.is_empty() {
                            self.flush(kind);
                            if !self.full() {
                                self.chars += alt.len();
                                self.blocks.push(Block::Image(alt.split_whitespace().collect::<Vec<_>>().join(" ")));
                            }
                        }
                        return;
                    }
                    _ => {}
                }
                let heading = match name.as_bytes() {
                    [b'H', d @ b'1'..=b'6'] => Some(d - b'0'),
                    _ if snap.attribute(node, "role") == Some("heading") => {
                        Some(snap.attribute(node, "aria-level").and_then(|l| l.parse().ok()).unwrap_or(2).clamp(1, 6))
                    }
                    _ => None,
                };
                let is_list = matches!(name, "UL" | "OL" | "MENU");
                let own = if let Some(level) = heading {
                    Some(Kind::Heading(level))
                } else if name == "LI" || display == Some("list-item") {
                    Some(Kind::Item(self.lists.max(1)))
                } else if name == "BLOCKQUOTE" {
                    Some(Kind::Quote)
                } else if name == "P" {
                    Some(Kind::Paragraph)
                } else {
                    None
                };
                let cell = matches!(display, Some("table-cell")) || matches!(name, "TD" | "TH");
                let block = own.is_some()
                    || is_list
                    || (!cell && display.is_some_and(|d| !d.starts_with("inline") && d != "contents" && d != "none"));
                if cell && !self.line.trim().is_empty() {
                    self.line.push_str(" · ");
                }
                if !block {
                    for &child in &snap.children[node] {
                        self.walk(child, kind);
                    }
                    if cell {
                        self.line.push(' ');
                    }
                    return;
                }
                self.flush(kind);
                let inner = own.unwrap_or(kind);
                if is_list {
                    self.lists = self.lists.saturating_add(1);
                }
                for &child in &snap.children[node] {
                    self.walk(child, inner);
                }
                if is_list {
                    self.lists = self.lists.saturating_sub(1);
                }
                self.flush(inner);
            }
            // the document, a shadow root, a fragment
            Some(9) | Some(11) => {
                for &child in &snap.children[node] {
                    self.walk(child, kind);
                }
            }
            _ => {}
        }
    }
}

/// The page's text, as a reader view, from a `DOMSnapshot.captureSnapshot` answer (`computedStyles`: display,
/// visibility): its main content when it has one with some text, else the whole page.
pub fn reader(answer: &Value) -> Vec<Block> {
    let Some(snap) = Snapshot::parse(answer) else { return Vec::new() };
    if snap.names.is_empty() {
        return Vec::new();
    }
    let read = |root: usize| {
        let mut reading = Reading { snap: &snap, blocks: Vec::new(), line: String::new(), chars: 0, lists: 0 };
        reading.walk(root, Kind::Paragraph);
        reading.flush(Kind::Paragraph);
        reading.blocks
    };
    if let Some(main) = snap.main() {
        let blocks = read(main);
        if blocks.iter().map(|b| b.text().len()).sum::<usize>() >= 200 {
            return blocks;
        }
    }
    read(0)
}

/// The page's language (`<html lang>`), when it says.
pub fn language(answer: &Value) -> Option<String> {
    let snap = Snapshot::parse(answer)?;
    let html = (0..snap.names.len()).find(|&i| snap.names[i] == "HTML")?;
    let lang = snap.attribute(html, "lang")?.trim();
    (!lang.is_empty() && lang.len() <= 20).then(|| lang.to_string())
}

/// The front tab's reader view, read through `cdp`.
pub async fn read(cdp: &mut Cdp, tab: &Tab) -> Result<Value, String> {
    let answer = cdp.call("DOMSnapshot.captureSnapshot", json!({"computedStyles": ["display", "visibility"]})).await?;
    let blocks: Vec<Value> = reader(&answer).iter().map(Block::to_json).collect();
    Ok(json!({
        "site": site(&tab.url), "url": tab.url, "title": tab.title, "lang": language(&answer), "blocks": blocks,
    }))
}

// -- routes ----------------------------------------------------------------------------------------------------------

/// A DevTools connection to the agent's front tab (with the person's hold token while they hold it).
async fn front_tab(state: &AppState, agent: &str) -> ApiResult<(Cdp, Tab)> {
    let config = state.config();
    let Some(port) = cdp_port(&config, agent) else {
        return Err(HttpError::json(409, json!({"code": "no_page_view"})));
    };
    let token = crate::browsers::hold_token(&config, agent);
    let list = tabs(port, token.as_deref()).await.map_err(|_| HttpError::json(409, json!({"code": "not_running"})))?;
    let Some(tab) = list.into_iter().next().filter(|t| !is_blank(&t.url)) else {
        return Err(HttpError::json(409, json!({"code": "no_page"})));
    };
    let cdp = Cdp::connect(port, &tab.id, token.as_deref())
        .await
        .map_err(|_| HttpError::json(503, json!({"code": "busy"})))?;
    Ok((cdp, tab))
}

fn own_browser(state: &AppState, agent_id: &str) -> ApiResult<String> {
    let config = state.config();
    let Some(agent) = config.agent(Some(agent_id)) else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    if !agent.desktop_vnc.as_deref().is_some_and(|v| !v.is_empty()) || cdp_port(&config, &agent.name).is_none() {
        return Err(HttpError::json(409, json!({"code": "no_page_view"})));
    }
    Ok(agent.name.clone())
}

async fn reader_of(state: &AppState, agent: &str) -> ApiResult<Response> {
    let (mut cdp, tab) = front_tab(state, agent).await?;
    let answer = read(&mut cdp, &tab).await;
    cdp.close().await;
    answer.map(web::ok).map_err(|_| HttpError::json(503, json!({"code": "busy"})))
}

/// `GET /v1/agents/{id}/desktop/reader`.
pub async fn reader_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = own_browser(&state, &agent_id)?;
    reader_of(&state, &agent).await
}

/// `GET /desktop/{ticket}/page/reader`: the page view's « Lecture ».
pub async fn ticket_reader(State(state): State<AppState>, UrlPath(ticket): UrlPath<String>) -> ApiResult<Response> {
    let Some(name) = state.desktop_tickets.agent(&ticket) else {
        return Err(HttpError::new(404, "expired"));
    };
    let agent = own_browser(&state, &name)?;
    reader_of(&state, &agent).await
}

/// `GET /v1/agents/{id}/desktop/focus`: where the agent acts now (a world of its own for this one reading).
pub async fn focus_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = own_browser(&state, &agent_id)?;
    let (mut cdp, tab) = front_tab(&state, &agent).await?;
    let mut world = None;
    let answer = focus(&mut cdp, &mut world).await;
    cdp.close().await;
    let focus = answer.map_err(|_| HttpError::json(503, json!({"code": "busy"})))?;
    let mut body = focus.to_json();
    body["site"] = json!(site(&tab.url));
    Ok(web::ok(body))
}

// -- the container's relay -------------------------------------------------------------------------------------------

/// The relay's version this bridge needs (browser/devtools_relay.py's RELAY_VERSION).
pub const RELAY_VERSION: u64 = 2;

static RELAYS: LazyLock<Mutex<HashMap<u16, (bool, f64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The agent's browser image predates this bridge's page view: its relay answers no version (Chrome's 404 comes
/// back instead). Unknown (nothing answers, or an old relay closing on a hold) is not outdated, and not kept; an
/// answer is kept five minutes (it changes only with the container).
pub async fn relay_outdated(port: u16) -> bool {
    let now = crate::py::time_time();
    if let Some((outdated, at)) = RELAYS.lock().unwrap_or_else(|e| e.into_inner()).get(&port).copied() {
        if now - at < 300.0 {
            return outdated;
        }
    }
    static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().unwrap_or_default()
    });
    let Ok(answer) = HTTP.get(format!("http://127.0.0.1:{port}/bipagents/relay")).send().await else { return false };
    let version = answer.json::<Value>().await.ok().and_then(|v| v.get("relay").and_then(Value::as_u64));
    let outdated = version.is_none_or(|v| v < RELAY_VERSION);
    RELAYS.lock().unwrap_or_else(|e| e.into_inner()).insert(port, (outdated, now));
    outdated
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A node: (parent, type, name, value, attributes, layout: (display, visibility)).
    type Node<'a> = (i64, i64, &'a str, &'a str, &'a [(&'a str, &'a str)], Option<(&'a str, &'a str)>);

    /// A snapshot as Chrome answers it.
    fn snapshot(nodes: &[Node]) -> Value {
        let mut strings: Vec<String> = Vec::new();
        let mut index = |s: &str| -> i64 {
            if s.is_empty() {
                return -1;
            }
            match strings.iter().position(|x| x == s) {
                Some(i) => i as i64,
                None => {
                    strings.push(s.to_string());
                    (strings.len() - 1) as i64
                }
            }
        };
        let (mut parents, mut types, mut names, mut values, mut attributes) = (vec![], vec![], vec![], vec![], vec![]);
        let (mut laid, mut styles) = (vec![], vec![]);
        for (i, (parent, kind, name, value, attrs, layout)) in nodes.iter().enumerate() {
            parents.push(*parent);
            types.push(*kind);
            names.push(index(name));
            values.push(index(value));
            attributes.push(attrs.iter().flat_map(|(k, v)| [index(k), index(v)]).collect::<Vec<_>>());
            if let Some((display, visibility)) = layout {
                laid.push(i);
                styles.push(vec![index(display), index(visibility)]);
            }
        }
        json!({"strings": strings, "documents": [{"nodes": {"parentIndex": parents, "nodeType": types,
               "nodeName": names, "nodeValue": values, "attributes": attributes},
               "layout": {"nodeIndex": laid, "styles": styles}}]})
    }

    const B: Option<(&str, &str)> = Some(("block", "visible"));
    const I: Option<(&str, &str)> = Some(("inline", "visible"));
    const T: Option<(&str, &str)> = Some(("", "visible"));

    fn texts(blocks: &[Block]) -> Vec<Value> {
        blocks.iter().map(Block::to_json).collect()
    }

    #[test]
    fn a_page_reads_as_headings_paragraphs_and_lists() {
        let page = snapshot(&[
            (-1, 9, "#document", "", &[], B),
            (0, 1, "HTML", "", &[("lang", "fr")], B),
            (1, 1, "BODY", "", &[], B),
            (2, 1, "NAV", "", &[], B),
            (3, 3, "#text", "Accueil Menu", &[], T),
            (2, 1, "H1", "", &[], B),
            (5, 3, "#text", "Horaires de la piscine", &[], T),
            (2, 1, "P", "", &[], B),
            (7, 3, "#text", "Ouverte du ", &[], T),
            (7, 1, "A", "", &[("href", "/x")], I),
            (9, 3, "#text", "lundi", &[], T),
            (7, 3, "#text", "  au\n samedi.", &[], T),
            (2, 1, "UL", "", &[], B),
            (12, 1, "LI", "", &[], Some(("list-item", "visible"))),
            (13, 3, "#text", "Bassin sportif", &[], T),
            (12, 1, "LI", "", &[], Some(("list-item", "visible"))),
            (15, 3, "#text", "Pataugeoire", &[], T),
            (2, 1, "DIV", "", &[], None), // display: none
            (17, 3, "#text", "caché", &[], None),
            (2, 1, "SCRIPT", "", &[], None),
            (19, 3, "#text", "var x = 1;", &[], None),
            (2, 1, "IMG", "", &[("alt", "Le grand bassin")], B),
            (2, 1, "SPAN", "", &[], Some(("inline", "hidden"))),
            (22, 3, "#text", "invisible", &[], Some(("", "hidden"))),
        ]);
        assert_eq!(
            texts(&reader(&page)),
            vec![
                json!({"t": "h", "level": 1, "text": "Horaires de la piscine"}),
                json!({"t": "p", "text": "Ouverte du lundi au samedi."}),
                json!({"t": "li", "depth": 1, "text": "Bassin sportif"}),
                json!({"t": "li", "depth": 1, "text": "Pataugeoire"}),
                json!({"t": "img", "text": "Le grand bassin"}),
            ]
        );
        assert_eq!(language(&page).as_deref(), Some("fr"));
    }

    #[test]
    fn the_main_content_is_read_when_there_is_one_and_tables_by_rows() {
        let long = "Une phrase assez longue pour compter comme un vrai contenu de page. ".repeat(4);
        let page = snapshot(&[
            (-1, 9, "#document", "", &[], B),
            (0, 1, "BODY", "", &[], B),
            (1, 1, "DIV", "", &[("class", "bandeau")], B),
            (2, 3, "#text", "Accepter les cookies", &[], T),
            (1, 1, "MAIN", "", &[], B),
            (4, 1, "P", "", &[], B),
            (5, 3, "#text", &long, &[], T),
            (4, 1, "TABLE", "", &[], Some(("table", "visible"))),
            (7, 1, "TR", "", &[], Some(("table-row", "visible"))),
            (8, 1, "TD", "", &[], Some(("table-cell", "visible"))),
            (9, 3, "#text", "Lundi", &[], T),
            (8, 1, "TD", "", &[], Some(("table-cell", "visible"))),
            (11, 3, "#text", "9 h – 18 h", &[], T),
            (4, 1, "DIV", "", &[("role", "heading"), ("aria-level", "3")], B),
            (13, 3, "#text", "Tarifs", &[], T),
        ]);
        let blocks = reader(&page);
        assert_eq!(blocks[0], Block::Paragraph(long.trim().to_string()));
        assert_eq!(blocks[1], Block::Paragraph("Lundi · 9 h – 18 h".into()));
        assert_eq!(blocks[2], Block::Heading(3, "Tarifs".into()));
        assert_eq!(blocks.len(), 3); // the banner outside <main> is left out
        assert!(reader(&json!({})).is_empty());
    }

    #[test]
    fn the_focus_is_clipped_to_the_viewport() {
        let seen = focus_from(&json!({"v": {"w": 1366, "h": 650}, "r": [100.25, 600, 300, 100], "kind": "field"}));
        assert_eq!(seen.viewport, (1366.0, 650.0));
        assert_eq!(seen.target, Some((100.3, 600.0, 300.0, 50.0, "field".into())));
        assert_eq!(
            focus_from(&json!({"v": {"w": 1366, "h": 650}, "r": [0, -900, 300, 100], "kind": "element"})).target,
            None
        ); // scrolled away
        assert_eq!(focus_from(&json!({"v": {"w": 1366, "h": 650}})).target, None);
        let odd = focus_from(&json!({"v": {"w": 800, "h": 600}, "r": [-10, 10, 900, 20], "kind": "<b>"}));
        assert_eq!(odd.target, Some((0.0, 10.0, 800.0, 20.0, "element".into())));
        assert_eq!(
            odd.to_json(),
            json!({"viewport": {"w": 800.0, "h": 600.0}, "target": {"x": 0.0, "y": 10.0, "w": 800.0, "h": 20.0, "kind": "element"}})
        );
    }
}
