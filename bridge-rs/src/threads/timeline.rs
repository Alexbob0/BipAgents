//! A thread's timeline: its segments' Hermes rows turned into the items the apps draw (docs/conversations.md §6.3),
//! the Boîte's messages placed among them by time, and pages cut from it. Pure: the rows come in, items go out.
//!
//! The bridge is the only reader of Hermes rows and of its own markers: `<!--bip:…-->` notes, steer wrappers, the
//! brake's rescue mark, `MEDIA:` lines, `[Pièce jointe : …]` lines, photo markers, tool-call markup a model wrote as
//! text. The apps get visible words and structured attachments.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::source::Row;

/// Hermes' failed-turn texts (its row closing a turn that failed or was stopped).
const FAILED_PARTIAL: &str =
    "This turn did not complete. Some actions may already have run; verify their effects before resending.";
const FAILED_NOT_PROCESSED: &str =
    "Your request was not processed. Send it again if you still want me to carry it out.";
const STEER_OPEN: &str = "[OUT-OF-BAND USER MESSAGE";
const STEER_CLOSE: &str = "[/OUT-OF-BAND USER MESSAGE]";
/// The bridge's own steers (runs.rs `HANDOFF_MARK`, loop_brake.rs): never the person's words.
const BRIDGE_STEER: &str = "[bipagents:";
const RESCUE_MARK: &str = "[bipagents:loop_rescue]";
/// A tool's result kept in an item at most (characters).
const RESULT_MAX: usize = 2000;
/// Leading rows of a continuation compared with its parent's last rows (a copied tail is not shown twice).
const TAIL: usize = 40;

static NOTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<!--\s*bip:[\s\S]*?(?:-->|$)").expect("valid regex"));
static PHOTO_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*\[(?:screenshot|image envoyée plus tôt|image)\][ \t]*\n?").expect("valid regex")
});
static SYNC_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\[Synchronous link:[^\]]*(?:\]|$)").expect("valid regex"));
static DOCUMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*\[Pièce jointe : (.+?) \(([^,()]+), ([^)]*)\) → (\S+)\][ \t]*\n?").expect("valid regex")
});
static MEDIA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*(?:\n|$)").expect("valid regex"));
static TOOL_MARKUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<tool_call>[\s\S]*?(?:</tool_call>|$)").expect("valid regex"));
static TOOL_TAGS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"</?(?:arg_key|arg_value|tool_call|tool_response|function_calls?|invoke|parameter)\b[^>]*>")
        .expect("valid regex")
});
/// An agent renamed itself in this turn (guide.rs `rename_yourself`): `<!--bip:renamed <agent id> <name>-->`.
static RENAMED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--bip:renamed ([^\s>]+) ([^\n]*?)-->").expect("valid regex"));
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid regex"));

/// One item of a timeline, before it is served.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    /// Where it reads: (segment, display order, sub-position).
    pub pos: (u32, i64, u32),
    pub at: f64,
    pub turn: String,
    pub segment: String,
    /// Hermes' row number (0 for the bridge's own items): where an attachment's bytes are read.
    pub row: i64,
    /// `kind` and the kind's own fields.
    pub body: Map<String, Value>,
}

impl Item {
    pub fn cursor(&self) -> String {
        cursor_of(self.pos)
    }

    pub fn kind(&self) -> &str {
        self.body.get("kind").and_then(Value::as_str).unwrap_or("")
    }

    pub fn text(&self) -> &str {
        self.body.get("text").and_then(Value::as_str).unwrap_or("")
    }

    /// As served (docs/conversations.md §6.3): `turn` only when another item's, no empty `attachments`, `segment`
    /// with `debug` only (every byte goes to a phone, often on a mobile network).
    pub fn to_json(&self, thread: &str, debug: bool) -> Value {
        let mut out = Map::new();
        out.insert("id".into(), json!(self.id));
        for (k, v) in &self.body {
            out.insert(k.clone(), v.clone());
        }
        if let Some(Value::Array(atts)) = out.get_mut("attachments") {
            for att in atts.iter_mut().filter_map(Value::as_object_mut) {
                if att.get("kind").and_then(Value::as_str) == Some("image") {
                    let id = att.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                    att.insert("url".into(), json!(format!("/v1/threads/{thread}/attachments/{id}")));
                }
            }
        }
        if out.get("attachments").and_then(Value::as_array).is_some_and(Vec::is_empty) {
            out.remove("attachments");
        }
        out.insert("at".into(), json!(iso(self.at)));
        out.insert("cursor".into(), json!(self.cursor()));
        if self.turn != self.id {
            out.insert("turn".into(), json!(self.turn));
        }
        if debug {
            out.insert("segment".into(), json!(self.segment));
        }
        Value::Object(out)
    }
}

/// A cursor: where the item reads (segment, display order, sub-position). Opaque to the apps.
pub fn cursor_of(pos: (u32, i64, u32)) -> String {
    format!("{}.{}.{}", pos.0, pos.1.max(0), pos.2)
}

/// A cursor's position (its item may be gone).
pub fn cursor_pos(cursor: &str) -> Option<(u32, i64, u32)> {
    let mut parts = cursor.splitn(3, '.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn iso(at: f64) -> String {
    let micros = (at * 1_000_000.0).round() as i64;
    chrono::DateTime::from_timestamp_micros(micros).map(crate::store::iso).unwrap_or_default()
}

/// A message's stable id: Hermes' `message_uid`, else a digest of where the row is.
pub fn message_id(agent: &str, session: &str, row: &Row) -> String {
    match &row.uid {
        Some(uid) => format!("m_{uid}"),
        None => {
            let digest = Sha256::digest(format!("{agent}\0{session}\0{}", row.row).as_bytes());
            format!("m_{}", &hex::encode(digest)[..24])
        }
    }
}

/// The words of a content (a string, or parts) and how many photos it carries.
fn words_and_images(content: &Value) -> (String, usize) {
    match content {
        Value::String(s) => (s.clone(), 0),
        Value::Array(parts) => {
            let mut words = Vec::new();
            let mut images = 0;
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("image_url" | "input_image" | "image") => images += 1,
                    _ => {
                        if let Some(t) = part.get("text").and_then(Value::as_str).or_else(|| part.as_str()) {
                            words.push(t.to_string());
                        }
                    }
                }
            }
            (words.join("\n"), images)
        }
        _ => (String::new(), 0),
    }
}

fn tidy(text: &str) -> String {
    BLANKS.replace_all(text, "\n\n").trim().to_string()
}

/// What a message shows of its raw text, and the attachments its lines name: (text, attachments). `id`: the
/// message's id (attachments are `<id>.a<n>`); `images`: photos in its parts.
pub fn visible(raw: &str, id: &str, images: usize) -> (String, Vec<Value>) {
    let mut text = NOTES.replace_all(raw, "").to_string();
    text = SYNC_LINK.replace_all(&text, "").to_string();
    let marked = PHOTO_MARK.find_iter(&text).count();
    text = PHOTO_MARK.replace_all(&text, "").to_string();
    text = TOOL_MARKUP.replace_all(&text, "").to_string();
    text = TOOL_TAGS.replace_all(&text, "").to_string();
    let mut attachments = Vec::new();
    for _ in 0..images.max(marked) {
        let n = attachments.len();
        attachments.push(json!({"id": format!("{id}.a{n}"), "kind": "image", "name": null, "mime": "image/jpeg",
                                "size": null, "path": null}));
    }
    for caps in DOCUMENT.captures_iter(&text) {
        let n = attachments.len();
        attachments.push(json!({"id": format!("{id}.a{n}"), "kind": "document", "name": caps[1].to_string(),
                                "mime": caps[2].to_string(), "size": caps[3].to_string(), "path": caps[4].to_string()}));
    }
    text = DOCUMENT.replace_all(&text, "").to_string();
    // MEDIA lines: where each was (in the text without them) for an inline page
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in MEDIA.captures_iter(&text) {
        let whole = caps.get(0).expect("match");
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        let path = caps[1].to_string();
        let n = attachments.len();
        attachments.push(json!({"id": format!("{id}.a{n}"), "kind": media_kind(&path),
                                "name": path.rsplit('/').next().unwrap_or(&path), "mime": mime_of(&path),
                                "size": null, "path": path, "offset": out.trim_start().chars().count()}));
    }
    out.push_str(&text[last..]);
    (tidy(&out), attachments)
}

fn media_kind(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.contains("/maps/") || lower.ends_with(".map.json") {
        "map"
    } else if lower.contains("/pages/") && lower.ends_with(".html") {
        "page"
    } else if [".mp3", ".m4a", ".wav", ".ogg", ".opus"].iter().any(|e| lower.ends_with(e)) {
        "audio"
    } else if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".heic"].iter().any(|e| lower.ends_with(e)) {
        "picture"
    } else {
        "file"
    }
}

fn mime_of(path: &str) -> String {
    mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream").to_string()
}

/// The renames a reply carries, the last one per agent: `[{agent, name}]` (the apps update the name at once).
pub fn renamed_in(raw: &str) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for caps in RENAMED.captures_iter(raw) {
        let (agent, name) = (caps[1].to_string(), caps[2].trim().to_string());
        if name.is_empty() {
            continue;
        }
        out.retain(|r| r["agent"].as_str() != Some(agent.as_str()));
        out.push(json!({"agent": agent, "name": name}));
    }
    out
}

fn steer_words(text: &str) -> Option<String> {
    let t = text.trim();
    if !t.starts_with(STEER_OPEN) || !t.ends_with(STEER_CLOSE) {
        return None;
    }
    let header = t[STEER_OPEN.len()..].find(']')? + STEER_OPEN.len();
    let end = t.len() - STEER_CLOSE.len();
    (header < end).then(|| t[header + 1..end].trim().to_string())
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max - 1).collect::<String>())
    }
}

/// The calls of an assistant row: (call id, name, arguments).
fn calls_of(row: &Row) -> Vec<(Option<String>, String, String)> {
    let Some(Value::Array(calls)) = &row.tool_calls else { return Vec::new() };
    calls
        .iter()
        .filter_map(Value::as_object)
        .map(|c| {
            let f = c.get("function").and_then(Value::as_object);
            let name =
                f.and_then(|f| f.get("name")).or_else(|| c.get("name")).and_then(Value::as_str).unwrap_or("tool");
            let args = f.and_then(|f| f.get("arguments")).or_else(|| c.get("arguments"));
            let args = match args {
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
                None => String::new(),
            };
            (c.get("id").and_then(Value::as_str).map(str::to_string), name.to_string(), args)
        })
        .collect()
}

/// A peer's message in the agent's Bot Chat (« Message from … (@x): »): its sender.
fn peer_of(text: &str) -> Option<String> {
    crate::cronwatch::TEAMMATE_PREFIX.captures(text).map(|c| c[1].to_string())
}

/// One segment's rows → items. `skip`: rows already shown (a continuation's copied tail), by message id.
pub fn segment_items(
    agent: &str,
    position: u32,
    session: &str,
    rows: &[Row],
    skip: &HashSet<String>,
    turn: &mut String,
) -> Vec<Item> {
    let results: HashMap<&str, &Row> =
        rows.iter().filter(|r| r.role == "tool").filter_map(|r| r.tool_call_id.as_deref().map(|id| (id, r))).collect();
    let mut items = Vec::new();
    for row in rows {
        if !row.shown || row.summary || row.display_kind.as_deref() == Some("hidden") {
            continue;
        }
        let id = message_id(agent, session, row);
        if skip.contains(&id) {
            continue;
        }
        let (raw, images) = words_and_images(&row.content);
        let item = |id: String, sub: u32, turn: &str, body: Value| Item {
            id,
            pos: (position, row.order, sub),
            at: row.timestamp,
            turn: turn.to_string(),
            segment: session.to_string(),
            row: row.row,
            body: match body {
                Value::Object(m) => m,
                _ => Map::new(),
            },
        };
        match row.role.as_str() {
            "user" => {
                let steered = steer_words(&raw);
                if steered.as_deref().is_some_and(|w| w.starts_with(BRIDGE_STEER)) {
                    continue;
                }
                let words = steered.clone().unwrap_or_else(|| raw.clone());
                if let Some(rest) = words.trim().strip_prefix(RESCUE_MARK) {
                    let line = rest.trim().split("\n\n").next().unwrap_or("").trim().to_string();
                    // Hermes' failed-turn row just before is replaced by this notice
                    if items.last().is_some_and(|i: &Item| {
                        i.body.get("notice").and_then(|n| n.get("code")) == Some(&json!("failed_turn"))
                    }) {
                        items.pop();
                    }
                    if !line.is_empty() {
                        items.push(item(
                            id,
                            0,
                            turn,
                            json!({"kind": "notice", "notice": {"code": "loop_rescue", "text": line}}),
                        ));
                    }
                    continue;
                }
                let (text, attachments) = visible(&words, &id, images);
                if text.is_empty() && attachments.is_empty() {
                    continue;
                }
                *turn = id.clone();
                let mut body = json!({"kind": "message", "role": "person", "text": text, "attachments": attachments});
                if steered.is_some() || row.display_kind.as_deref() == Some("steer") {
                    body["added_during_run"] = json!(true);
                }
                if let Some(peer) = peer_of(&text) {
                    body["role"] = json!("peer");
                    body["from"] = json!(peer);
                }
                items.push(item(id, 0, turn, body));
            }
            "assistant" => {
                let calls = calls_of(row);
                let trimmed = raw.trim();
                if calls.is_empty()
                    && (row.display_kind.as_deref() == Some("failed_turn")
                        || trimmed == FAILED_PARTIAL
                        || trimmed == FAILED_NOT_PROCESSED)
                {
                    items.push(item(
                        id,
                        0,
                        turn,
                        json!({"kind": "notice", "notice": {"code": "failed_turn", "text": trimmed}}),
                    ));
                    continue;
                }
                let (text, attachments) = visible(&raw, &id, images);
                let renamed = renamed_in(&raw);
                if !text.is_empty() || !attachments.is_empty() || !renamed.is_empty() {
                    let mut body =
                        json!({"kind": "message", "role": "agent", "text": text, "attachments": attachments});
                    if !renamed.is_empty() {
                        body["renamed"] = Value::Array(renamed);
                    }
                    if let Some(r) = &row.reasoning {
                        body["reasoning"] = json!(r);
                    }
                    items.push(item(id.clone(), 0, turn, body));
                }
                for (n, (call_id, name, args)) in calls.into_iter().enumerate() {
                    let result =
                        call_id.as_deref().and_then(|c| results.get(c)).map(|r| words_and_images(&r.content).0);
                    let status = match &result {
                        None => "running",
                        Some(r) if r.trim_start().starts_with("{\"error\"") => "failed",
                        Some(_) => "done",
                    };
                    let tool = json!({"name": name, "call_id": call_id, "arguments": cut(&args, RESULT_MAX),
                                      "result": result.map(|r| cut(&r, RESULT_MAX)), "status": status, "via": null});
                    items.push(item(format!("{id}.t{n}"), 1 + n as u32, turn, json!({"kind": "tool", "tool": tool})));
                }
            }
            _ => {}
        }
    }
    if turn.is_empty() {
        if let Some(first) = items.first().map(|f| f.id.clone()) {
            for i in items.iter_mut().filter(|i| i.turn.is_empty()) {
                i.turn = first.clone();
            }
            *turn = first;
        }
    }
    items
}

/// The copied tail of a continuation: its leading rows that repeat the parent's last ones (same uid, or same role and
/// words), by message id.
pub fn copied_tail(agent: &str, session: &str, rows: &[Row], parent: &[Row]) -> HashSet<String> {
    let key = |r: &Row| (r.role.clone(), words_and_images(&r.content).0.trim().to_string());
    let tail: Vec<&Row> = parent.iter().rev().take(TAIL).collect();
    let uids: HashSet<&str> = tail.iter().filter_map(|r| r.uid.as_deref()).collect();
    let keys: HashSet<(String, String)> = tail.iter().map(|r| key(r)).collect();
    let mut out = HashSet::new();
    for row in rows.iter().filter(|r| !r.summary && r.role != "system") {
        let same = row.uid.as_deref().is_some_and(|u| uids.contains(u)) || keys.contains(&key(row));
        if !same {
            break;
        }
        out.insert(message_id(agent, session, row));
    }
    out
}

/// The Boîte's messages placed among the Hermes items by time (each after the last item not later than it).
pub fn merge(items: Vec<Item>, mut boite: Vec<Item>) -> Vec<Item> {
    if boite.is_empty() {
        return items;
    }
    boite.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut out = Vec::with_capacity(items.len() + boite.len());
    let mut extra = boite.into_iter().peekable();
    let mut last = (0u32, 0i64);
    let mut sub = 100u32;
    for item in items {
        while extra.peek().is_some_and(|b| b.at < item.at) {
            let mut b = extra.next().expect("peeked");
            b.pos = (last.0, last.1, sub);
            sub += 1;
            b.turn = out.last().map(|i: &Item| i.turn.clone()).unwrap_or_else(|| b.id.clone());
            out.push(b);
        }
        if (item.pos.0, item.pos.1) != last {
            last = (item.pos.0, item.pos.1);
            sub = 100;
        }
        out.push(item);
    }
    for mut b in extra {
        b.pos = (last.0, last.1, sub);
        sub += 1;
        b.turn = out.last().map(|i: &Item| i.turn.clone()).unwrap_or_else(|| b.id.clone());
        out.push(b);
    }
    out
}

/// One page of a timeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
    pub more_before: bool,
    pub more_after: bool,
    pub anchor: Option<String>,
}

/// What a request asks of the timeline.
#[derive(Debug, Clone, Default)]
pub struct Want {
    pub limit: usize,
    pub before: Option<String>,
    pub after: Option<String>,
    pub around: Option<String>,
    pub before_count: usize,
    /// None: everything after the anchor.
    pub after_count: Option<usize>,
}

/// The index of the first item at or after a cursor's position (the item itself, or where it was).
fn at_cursor(items: &[Item], cursor: &str) -> (usize, bool) {
    let pos = cursor_pos(cursor).unwrap_or((0, 0, 0));
    match items.iter().position(|it| it.pos >= pos) {
        Some(i) => (i, items[i].pos == pos),
        None => (items.len(), false),
    }
}

pub fn page(items: Vec<Item>, want: &Want) -> Page {
    let n = items.len();
    let limit = want.limit.max(1);
    let (start, end, anchor) = if let Some(around) = &want.around {
        match items.iter().position(|i| &i.id == around || i.id.starts_with(&format!("{around}."))) {
            Some(i) => {
                let start = i.saturating_sub(want.before_count);
                let end = want.after_count.map(|a| (i + 1 + a).min(n)).unwrap_or(n);
                (start, end, Some(items[i].id.clone()))
            }
            None => (n.saturating_sub(limit), n, None),
        }
    } else if let Some(after) = &want.after {
        let (i, found) = at_cursor(&items, after);
        let start = if found { i + 1 } else { i };
        (start.min(n), (start + limit).min(n), None)
    } else if let Some(before) = &want.before {
        let (i, _) = at_cursor(&items, before);
        (i.saturating_sub(limit), i, None)
    } else {
        (n.saturating_sub(limit), n, None)
    };
    let more_before = start > 0;
    let more_after = end < n;
    let items = items.into_iter().skip(start).take(end.saturating_sub(start)).collect();
    Page { items, more_before, more_after, anchor }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, role: &str, content: Value) -> Row {
        Row {
            row: id,
            order: id,
            uid: Some(format!("u{id}")),
            role: role.into(),
            content,
            timestamp: 1000.0 + id as f64,
            shown: true,
            ..Default::default()
        }
    }

    fn items(rows: &[Row]) -> Vec<Item> {
        segment_items("vie", 0, "s1", rows, &HashSet::new(), &mut String::new())
    }

    #[test]
    fn markers_become_text_and_attachments() {
        let raw = "<!--bip:now lundi 9 h-->Voici le plan.\nMEDIA:/bipagents/pages/vie/plan.html\nLa suite.\n[Pièce jointe : devis.pdf (application/pdf, 12 Ko) → /home/x/uploads/devis.pdf]";
        let (text, atts) = visible(raw, "m_1", 0);
        assert_eq!(text, "Voici le plan.\nLa suite.");
        // photos first, then documents, then the agent's files in their order
        assert_eq!(atts[0]["kind"], "document");
        assert_eq!(atts[0]["id"], "m_1.a0");
        assert_eq!(atts[0]["path"], "/home/x/uploads/devis.pdf");
        assert_eq!(atts[1]["kind"], "page");
        assert_eq!(atts[1]["id"], "m_1.a1");
        assert_eq!(atts[1]["offset"], 15);
        let (text, atts) = visible("[screenshot]\nYes", "m_2", 1);
        assert_eq!(text, "Yes");
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0]["kind"], "image");
        let (text, _) = visible("Bonjour<tool_call>search<arg_key>q</arg_key></tool_call>", "m_3", 0);
        assert_eq!(text, "Bonjour");
        let renamed = renamed_in("C'est fait.<!--bip:renamed bip Léo--><!--bip:renamed bip Léon-->");
        assert_eq!(renamed, vec![json!({"agent": "bip", "name": "Léon"})]);
    }

    #[test]
    fn rows_become_items_with_stable_ids() {
        let rows = vec![
            row(1, "user", json!("<!--bip:now x-->Quel train ?")),
            row(2, "assistant", Value::Null),
            row(3, "tool", json!("3 résultats")),
            row(4, "assistant", json!("Le train de 9 h.")),
            row(5, "user", json!("[OUT-OF-BAND USER MESSAGE from the user]\n[bipagents:handoff] hand over\n[/OUT-OF-BAND USER MESSAGE]")),
            row(6, "user", json!("[OUT-OF-BAND USER MESSAGE from the user]\nEt le retour ?\n[/OUT-OF-BAND USER MESSAGE]")),
            row(7, "user", json!("<!--bip:restore note-->")),
            row(8, "system", json!("x")),
        ];
        let mut rows = rows;
        rows[1].tool_calls =
            Some(json!([{"id": "c1", "function": {"name": "web_search", "arguments": "{\"q\":\"train\"}"}}]));
        rows[2].tool_call_id = Some("c1".into());
        let mut hidden = row(9, "assistant", json!("caché"));
        hidden.shown = false;
        rows.push(hidden);
        let mut summary = row(10, "user", json!("résumé"));
        summary.summary = true;
        rows.push(summary);
        let got = items(&rows);
        let ids: Vec<&str> = got.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["m_u1", "m_u2.t0", "m_u4", "m_u6"]);
        assert_eq!(got[0].text(), "Quel train ?");
        assert_eq!(got[1].body["tool"]["status"], "done");
        assert_eq!(got[1].body["tool"]["result"], "3 résultats");
        assert_eq!(got[2].body["role"], "agent");
        assert!(got.iter().take(3).all(|i| i.turn == "m_u1"));
        assert_eq!(got[3].body["added_during_run"], true);
        assert_eq!(got[3].text(), "Et le retour ?");
        // without uid: a digest of where the row is
        let mut plain = row(1, "user", json!("Salut"));
        plain.uid = None;
        let a = items(&[plain.clone()]);
        assert!(a[0].id.starts_with("m_") && a[0].id.len() == 26);
        assert_eq!(a, items(&[plain]));
    }

    #[test]
    fn items_come_in_the_order_the_apps_draw_them() {
        // each assistant row: its words first, then its own calls (their results folded in); the final answer last
        let mut rows = vec![
            row(1, "user", json!("Compare trois écoles")),
            row(2, "assistant", json!("Je cherche d'abord.")),
            row(3, "tool", json!("r1")),
            row(4, "tool", json!("r2")),
            row(5, "assistant", json!("Deux trouvées, je vérifie la troisième.")),
            row(6, "tool", json!("r3")),
            row(7, "assistant", json!("Voici le comparatif.")),
        ];
        rows[1].tool_calls = Some(json!([{"id": "a", "function": {"name": "web_search", "arguments": "{}"}},
                                         {"id": "b", "function": {"name": "web_extract", "arguments": "{}"}}]));
        rows[2].tool_call_id = Some("a".into());
        rows[3].tool_call_id = Some("b".into());
        rows[4].tool_calls = Some(json!([{"id": "c", "function": {"name": "web_search", "arguments": "{}"}}]));
        rows[5].tool_call_id = Some("c".into());
        let got: Vec<String> = items(&rows)
            .iter()
            .map(|i| match i.kind() {
                "tool" => format!("tool:{}", i.body["tool"]["result"].as_str().unwrap()),
                _ => i.text().to_string(),
            })
            .collect();
        assert_eq!(
            got,
            [
                "Compare trois écoles",
                "Je cherche d'abord.",
                "tool:r1",
                "tool:r2",
                "Deux trouvées, je vérifie la troisième.",
                "tool:r3",
                "Voici le comparatif."
            ]
        );
    }

    #[test]
    fn failed_turns_and_rescues_are_notices() {
        let rows = vec![
            row(1, "user", json!("Range le carnet")),
            row(2, "assistant", json!(FAILED_PARTIAL)),
            row(3, "user", json!(format!("{RESCUE_MARK}\nJ'ai arrêté une boucle.\n\nDétails pour l'agent"))),
            row(4, "assistant", json!("C'est fait.")),
        ];
        let got = items(&rows);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].body["notice"]["code"], "loop_rescue");
        assert_eq!(got[1].body["notice"]["text"], "J'ai arrêté une boucle.");
        let only_failed = items(&rows[..2]);
        assert_eq!(only_failed[1].body["notice"]["code"], "failed_turn");
    }

    #[test]
    fn a_copied_tail_is_shown_once() {
        let parent = vec![row(1, "user", json!("A")), row(2, "assistant", json!("B"))];
        let mut child =
            vec![row(10, "user", json!("A")), row(11, "assistant", json!("B")), row(12, "user", json!("C"))];
        child[0].uid = Some("x10".into());
        child[1].uid = Some("u2".into());
        let skip = copied_tail("vie", "s2", &child, &parent);
        assert_eq!(skip.len(), 2);
        let got = segment_items("vie", 1, "s2", &child, &skip, &mut String::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text(), "C");
    }

    fn boite(id: &str, at: f64) -> Item {
        Item {
            id: id.into(),
            pos: (0, 0, 0),
            at,
            turn: String::new(),
            segment: String::new(),
            row: 0,
            body: json!({"kind": "boite", "text": id}).as_object().unwrap().clone(),
        }
    }

    #[test]
    fn boite_messages_by_time_and_pages() {
        let rows: Vec<Row> =
            (1..=6).map(|i| row(i, if i % 2 == 1 { "user" } else { "assistant" }, json!(format!("t{i}")))).collect();
        let all = merge(items(&rows), vec![boite("o_a", 1003.5), boite("o_z", 5000.0), boite("o_0", 10.0)]);
        let ids: Vec<&str> = all.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["o_0", "m_u1", "m_u2", "m_u3", "o_a", "m_u4", "m_u5", "m_u6", "o_z"]);
        let positions: Vec<_> = all.iter().map(|i| cursor_pos(&i.cursor()).unwrap()).collect();
        let mut sorted = positions.clone();
        sorted.sort();
        assert_eq!(positions, sorted, "cursors follow the timeline");
        // latest page, then older
        let p = page(all.clone(), &Want { limit: 3, ..Default::default() });
        assert_eq!(p.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["m_u5", "m_u6", "o_z"]);
        assert!(p.more_before && !p.more_after);
        let older = page(all.clone(), &Want { limit: 3, before: Some(p.items[0].cursor()), ..Default::default() });
        assert_eq!(older.items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["m_u3", "o_a", "m_u4"]);
        // after a cursor: everything newer
        let newer = page(all.clone(), &Want { limit: 50, after: Some(all[4].cursor()), ..Default::default() });
        assert_eq!(newer.items.len(), 4);
        // around: the anchor, two before, all after
        let around =
            page(all.clone(), &Want { limit: 50, around: Some("m_u3".into()), before_count: 2, ..Default::default() });
        assert_eq!(around.anchor.as_deref(), Some("m_u3"));
        assert_eq!(around.items.first().unwrap().id, "m_u1");
        assert_eq!(around.items.last().unwrap().id, "o_z");
        // a cursor whose item is gone still pages from where it was
        let gone = cursor_of((all[3].pos.0, all[3].pos.1, all[3].pos.2 + 50));
        let after_gone = page(all.clone(), &Want { limit: 50, after: Some(gone), ..Default::default() });
        assert_eq!(after_gone.items.first().unwrap().id, "o_a");
    }
}
