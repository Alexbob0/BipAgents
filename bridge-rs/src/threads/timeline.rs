//! A thread's timeline: its segments' Hermes rows turned into the items the apps draw (docs/conversations.md §6.3),
//! the Boîte's messages placed among them by time, and pages cut from it. Pure: the rows come in, items go out.
//!
//! The bridge is the only reader of Hermes rows and of its own markers: `<!--bip:…-->` notes, steer wrappers, the
//! brake's rescue mark, `MEDIA:` lines, `[Pièce jointe : …]` lines, photo markers, tool-call markup a model wrote as
//! text. The apps get visible words and structured attachments.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::NaiveDate;
use chrono_tz::Tz;
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
const RESCUE_MARK: &str = "[bipagents:loop_rescue]";
/// Hermes' own word to the agent, posted as a user turn, when a process it ran in the background ends: never shown.
const PROCESS_NOTICE: &str = "[IMPORTANT: Background process";
/// Hermes patch 0023's turn notes (the memory written since the prompt, a run's instructions): joined to the API copy
/// of the person's message (`api_content`), not its `content`; left out should one ever land in the words.
const TURN_NOTES: [&str; 2] =
    ["[System: Your memory changed after", "[System: Instructions for this conversation, from the app"];
/// A tool's result kept in an item at most (characters).
const RESULT_MAX: usize = 2000;
/// Leading rows of a continuation compared with its parent's last rows (a copied tail is not shown twice).
const TAIL: usize = 40;

static NOTES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<!--\s*bip:[\s\S]*?(?:-->|$)").expect("valid regex"));
static PHOTO_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*\[(?:screenshot|image envoyée plus tôt|image)\][ \t]*\n?").expect("valid regex")
});
/// A photo Hermes (v0.22.5) saved for a message: `[Image attached at: <path>]` (the bytes are in that file, not in the
/// row): an image attachment, served from the file (mod.rs `attachment_route`), never words of the message.
pub static IMAGE_AT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*\[Image attached at: ([^\]\n]+)\][ \t]*\n?").expect("valid regex"));
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
/// A bridge mark (`[bipagents:…]`) starting a line: from there on, words for the agent only.
static BRIDGE_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*\[bipagents:[a-z_]+\]").expect("valid regex"));
/// A steer of the bridge's in Hermes' out-of-band wrapper, inside other words (a tool result it was joined to).
static BRIDGE_STEER_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[OUT-OF-BAND USER MESSAGE[^\]\n]*\]\s*\[bipagents:[\s\S]*?(?:\[/OUT-OF-BAND USER MESSAGE\]|$)")
        .expect("valid regex")
});
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
    let saved = IMAGE_AT.find_iter(&text).count();
    text = IMAGE_AT.replace_all(&text, "").to_string();
    text = TOOL_MARKUP.replace_all(&text, "").to_string();
    text = TOOL_TAGS.replace_all(&text, "").to_string();
    let mut attachments = Vec::new();
    // one photo is told by its part, its `[screenshot]` mark and its saved file alike: the most of the three
    for _ in 0..images.max(marked).max(saved) {
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
    let (text, files) = place_media(&text);
    for mut found in files {
        found["id"] = json!(format!("{id}.a{}", attachments.len()));
        attachments.push(found);
    }
    (text, attachments)
}

/// A text without its `MEDIA:` lines (tidied) and the files they name, each with its `offset`: where its line was, in
/// characters of that text, right after the words before it (the apps cut the text there for a page in its place).
pub fn place_media(text: &str) -> (String, Vec<Value>) {
    let mut out = String::with_capacity(text.len());
    let mut files = Vec::new();
    let mut last = 0;
    for caps in MEDIA.captures_iter(text) {
        let whole = caps.get(0).expect("match");
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        files.push(file_attachment(&caps[1], Some(tidy(&out).chars().count())));
    }
    out.push_str(&text[last..]);
    (tidy(&out), files)
}

/// An agent's file as an attachment (its id set by the message): `offset`, where its `MEDIA:` line was in the text.
pub fn file_attachment(path: &str, offset: Option<usize>) -> Value {
    let mut a = json!({"id": null, "kind": media_kind(path), "name": path.rsplit('/').next().unwrap_or(path),
                       "mime": mime_of(path), "size": null, "path": path});
    if let Some(at) = offset {
        a["offset"] = json!(at);
    }
    a
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

/// What a person's row says once the app's words for the agent are out (Alex 4/11: the brake's rescue note showed as
/// his own bubble, its `<!--bip:now-->` line in front of the mark): its notes, Hermes' wrapper of words added during
/// the answer, 0023's turn notes. `Err(rescue line)` when the row is the brake's rescue input (its first paragraph,
/// in the person's language: the only words of it they read); `Ok(None)` when nothing of it is the person's (a
/// bridge steer, Hermes' process notice, notes only); else `Ok(Some((words, steered)))`.
fn person_words(raw: &str) -> Result<Option<(String, bool)>, String> {
    let bare = NOTES.replace_all(raw, "");
    let bare = bare.trim();
    let steered = steer_words(bare);
    let mut words = steered.clone().unwrap_or_else(|| bare.to_string());
    if let Some(rest) = words.strip_prefix(RESCUE_MARK) {
        return Err(rest.trim().split("\n\n").next().unwrap_or("").trim().to_string());
    }
    if words.starts_with(PROCESS_NOTICE) {
        return Ok(None);
    }
    words = BRIDGE_STEER_BLOCK.replace_all(&words, "").to_string();
    if let Some(at) = BRIDGE_MARK.find(&words).map(|m| m.start()) {
        words.truncate(at);
    }
    for note in TURN_NOTES {
        if let Some(at) = words.find(note) {
            words.truncate(at);
        }
    }
    let words = words.trim().to_string();
    Ok((!words.is_empty()).then_some((words, steered.is_some())))
}

/// A tool's result as served: the app's notes and the bridge's words to the agent joined to it (the brake's warning
/// after a result, a steer Hermes added to it) left out.
fn result_words(raw: &str) -> String {
    let text = NOTES.replace_all(raw, "");
    let mut text = BRIDGE_STEER_BLOCK.replace_all(&text, "").to_string();
    if let Some(at) = BRIDGE_MARK.find(&text).map(|m| m.start()) {
        text.truncate(at);
    }
    text.trim_end().to_string()
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

/// A peer's message in the agent's Bot Chat (« Message from 🤖 Vie (@vie): … »): (its sender's id, the name it signed
/// with without Hermes' emoji badge, its words alone). The apps draw it from these fields; they never read the
/// « Message from » line (iOS drew it twice).
fn peer_of(text: &str) -> Option<(String, String, String)> {
    static PEER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*Message from (.+?) \(@([\w.-]+)\):\s*([\s\S]*)$").expect("valid regex"));
    let caps = PEER.captures(text)?;
    let signed = caps[1].trim();
    // « 🤖 Wellness » (Hermes signs with an emoji first) → « Wellness »; « Pub & Acquisition » (the bridge's) as it is
    let name = match signed.split_once(' ') {
        Some((badge, rest)) if !badge.chars().any(char::is_alphanumeric) && !rest.trim().is_empty() => rest.trim(),
        _ => signed,
    };
    Some((caps[2].to_string(), name.to_string(), caps[3].trim().to_string()))
}

/// Which of Hermes' failed-turn lines `text` is, exactly (spaces aside: a reply quoting one stays a reply):
/// `partial` (something may have run) or `not_processed` (nothing was done). The apps say it in their words from this
/// (`notice.stopped`, a run's `stopped`), never by reading Hermes' English.
pub fn stopped_kind(text: &str) -> Option<&'static str> {
    match text.trim() {
        FAILED_PARTIAL => Some("partial"),
        FAILED_NOT_PROCESSED => Some("not_processed"),
        _ => None,
    }
}

/// Which rows a segment shows, as (where it reads, whose words). Hermes v0.22.5 compacts a conversation IN its session
/// (no child): the messages it summarizes stay as `compacted` rows; the ones it keeps are copied after them with the
/// same `message_uid` (the originals left inactive, not compacted: « superseded »), and the summary row takes the uid of
/// a message it folded in (that message's rows then all inactive). Each compaction makes new copies. So a message (a
/// uid) is shown when one of its rows is (active or compacted, not a summary, not hidden) or a summary holds its uid,
/// once, at its FIRST row whatever its state: where it was said, the same place after every compaction. Placed at its
/// first shown copy instead, a message moved at each compaction, sometimes ahead of the ones said after it, and the
/// apps stopped paging back when an older page brought a message they held further down
/// (internal/notes/history-compaction.md). A row without uid: as Hermes shows it. The words are the newest shown
/// copy's (the same bytes but for a prune). Lab 2026-11-03: « ok 3 » was lost and « Note 1 », « Note 2 » were drawn
/// twice, with the same id.
fn in_place_compaction(rows: &[Row]) -> Vec<(usize, usize)> {
    let hidden = |r: &Row| r.display_kind.as_deref() == Some("hidden") || r.role == "session_meta";
    // per uid: the copy whose words are shown (the newest shown: the live one when there is one, as Hermes shows it),
    // None when only a summary holds it
    let mut words: HashMap<&str, Option<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() {
        let Some(u) = r.uid.as_deref() else { continue };
        if r.summary {
            words.entry(u).or_insert(None);
        } else if r.shown && !hidden(r) {
            let best = words.entry(u).or_insert(None);
            if best.is_none_or(|b| rows[b].row < r.row) {
                *best = Some(i);
            }
        }
    }
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        if r.summary || hidden(r) {
            continue;
        }
        match r.uid.as_deref() {
            Some(u) => {
                if let Some(shown) = words.get(u) {
                    if seen.insert(u) {
                        out.push((i, shown.unwrap_or(i)));
                    }
                }
            }
            None if r.shown => out.push((i, i)),
            None => {}
        }
    }
    out
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
    for (place, shown) in in_place_compaction(rows) {
        let (row, place) = (&rows[shown], &rows[place]);
        let id = message_id(agent, session, row);
        if skip.contains(&id) {
            continue;
        }
        let (raw, images) = words_and_images(&row.content);
        let item = |id: String, sub: u32, turn: &str, body: Value| Item {
            id,
            pos: (position, place.order, sub),
            at: place.timestamp,
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
                let (words, steered) = match person_words(&raw) {
                    Err(line) => {
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
                    // a photo sent alone still is the person's
                    Ok(None) if images == 0 => continue,
                    Ok(None) => (String::new(), false),
                    Ok(Some(found)) => found,
                };
                let (text, attachments) = visible(&words, &id, images);
                if text.is_empty() && attachments.is_empty() {
                    continue;
                }
                *turn = id.clone();
                let mut body = json!({"kind": "message", "role": "person", "text": text, "attachments": attachments});
                if steered || row.display_kind.as_deref() == Some("steer") {
                    body["added_during_run"] = json!(true);
                }
                if let Some((from, name, words)) = peer_of(&text) {
                    body["role"] = json!("peer");
                    body["from"] = json!(from);
                    body["from_name"] = json!(name);
                    body["text"] = json!(words);
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
                        json!({"kind": "notice", "notice": {"code": "failed_turn", "text": trimmed,
                                                            "stopped": stopped_kind(trimmed)}}),
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
                    if crate::steps::is_todo_call(&name, &args) {
                        // the agent's task list: a `steps` item per call, one per turn after `steps::fold` (§6.3b)
                        let list = call_id
                            .as_deref()
                            .and_then(|c| results.get(c))
                            .and_then(|r| crate::steps::of_result(&words_and_images(&r.content).0));
                        if let Some(steps) = list {
                            let body = json!({"kind": "steps", "steps": steps});
                            items.push(item(format!("{id}.t{n}"), 1 + n as u32, turn, body));
                        }
                        continue;
                    }
                    let result = call_id
                        .as_deref()
                        .and_then(|c| results.get(c))
                        .map(|r| result_words(&words_and_images(&r.content).0));
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

/// An agent's message that only says it has nothing to tell (background_turn.rs `is_silent`), nothing attached.
pub fn silent_reply(item: &Item) -> bool {
    let b = &item.body;
    b.get("kind") == Some(&json!("message"))
        && b.get("role") == Some(&json!("agent"))
        && b.get("attachments").and_then(Value::as_array).is_none_or(|a| a.is_empty())
        && b.get("renamed").is_none()
        && crate::background_turn::is_silent(b.get("text").and_then(Value::as_str).unwrap_or(""))
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

/// How long a stored message names the `s_` item it replaces (`Page::replaced`): its send's two days, and a day more.
const REPLACED_SECONDS: f64 = 3.0 * 86_400.0;

/// One page of a timeline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Page {
    pub items: Vec<Item>,
    pub more_before: bool,
    pub more_after: bool,
    pub anchor: Option<String>,
    /// Where the page starts among the thread's items (its days: `with_days`).
    pub start: usize,
    /// The page is the window: the phone replaces what it holds with it (latest, around, a catch-up too long).
    pub reset: bool,
    /// Else the cursor of the item it joins (held by the phone): an `after=` page goes after it (what the phone held
    /// after it is gone), a `before=` page in front of it (what it held before it is gone).
    pub joins: Option<String>,
}

impl Page {
    /// The answer of `GET /v1/threads/{thread}/timeline` (§6.2): a conversation's or a room's.
    pub fn answer(&self, thread: Value, version: i64, pending: Value, debug: bool) -> Value {
        let id = thread["id"].as_str().unwrap_or_default().to_string();
        let mut out = json!({
            "thread": thread,
            "items": self.items.iter().map(|i| i.to_json(&id, debug)).collect::<Vec<_>>(),
            "first": self.items.first().map(Item::cursor),
            "last": self.items.last().map(Item::cursor),
            "more_before": self.more_before,
            "more_after": self.more_after,
            "anchor": self.anchor,
            "version": version,
            "pending": pending,
            "reset": self.reset,
            "joins": self.joins,
        });
        let replaced = self.replaced();
        if !replaced.is_empty() {
            out["replaced"] = json!(replaced);
        }
        out
    }

    /// The items a phone may still hold that this page's rows replace: a message sent from a phone is the item
    /// `s_<client_id>` until Hermes stores it (send.rs), then its stored row with that `client_id`. A window that read
    /// the `s_` item while the turn ran, then the stored row after its cursor, drew the message twice (iPhone 4/11;
    /// Android never dropped it): the apps remove what `replaced` names, without knowing why. Only rows of the last few
    /// days (an `s_` item lives two days at most, `SENDS_KEPT_SECONDS`), or without a time (an older store).
    fn replaced(&self) -> Vec<String> {
        let since = chrono::Utc::now().timestamp() as f64 - REPLACED_SECONDS;
        self.items
            .iter()
            .filter(|i| i.kind() == "message" && !i.body.contains_key("sending") && (i.at >= since || i.at == 0.0))
            .filter_map(|i| i.body.get("client_id").and_then(Value::as_str))
            .map(|c| format!("s_{c}"))
            .collect()
    }

    /// Its `s_` items lost (send.rs `lost`): `not_sent` instead of `sending` (the apps draw « Pas envoyé », a tap sends
    /// it again under the same id). Called when no turn runs in the conversation.
    pub fn with_lost(mut self) -> Self {
        for item in self.items.iter_mut().filter(|i| super::send::lost(i)) {
            item.body.remove("sending");
            item.body.insert("not_sent".into(), json!(true));
        }
        self
    }

    /// Each page item that draws something gets `day` (its date where the person is, `YYYY-MM-DD`), the first one of
    /// its day in the whole thread `new_day: true` too: the apps put a day divider over it (and over the first dated
    /// row they show). `days`: `days(…)` of the items the page was cut from.
    pub fn with_days(mut self, days: &[Option<(NaiveDate, bool)>]) -> Self {
        for (k, item) in self.items.iter_mut().enumerate() {
            if let Some(Some((day, new))) = days.get(self.start + k) {
                item.body.insert("day".into(), json!(day.format("%Y-%m-%d").to_string()));
                if *new {
                    item.body.insert("new_day".into(), json!(true));
                }
            }
        }
        self
    }
}

/// Whether an item draws something in the apps: words, a file, reasoning, a tool card, a notice, a Boîte message, an
/// answered card, a call. A message with nothing left to show (its words were notes) draws nothing.
pub fn draws(item: &Item) -> bool {
    match item.kind() {
        "message" => {
            !item.text().trim().is_empty()
                || item.body.get("attachments").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
                || item.body.get("reasoning").and_then(Value::as_str).is_some_and(|r| !r.trim().is_empty())
        }
        "notice" => {
            item.body.get("notice").and_then(|n| n.get("text")).and_then(Value::as_str).is_some_and(|t| !t.is_empty())
        }
        "tool" | "boite" | "approval" | "call" => true,
        _ => false,
    }
}

/// The day of each item of a thread where the person is (`zone`), and whether it is the first of that day: None for an
/// item that draws nothing or has no time. The day dividers are decided here, once for every app (5/11: the iPhone drew
/// dividers over rows that draw nothing, « Dimanche 1 novembre »… with nothing under them; Android still did).
pub fn days(items: &[Item], zone: Tz) -> Vec<Option<(NaiveDate, bool)>> {
    let mut previous: Option<NaiveDate> = None;
    items
        .iter()
        .map(|item| {
            if item.at <= 0.0 || !draws(item) {
                return None;
            }
            let day = chrono::DateTime::from_timestamp(item.at.floor() as i64, 0)?.with_timezone(&zone).date_naive();
            let new = previous != Some(day);
            previous = Some(day);
            Some((day, new))
        })
        .collect()
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
    /// The phone gave no `limit`: the bridge chooses the page's size, and a catch-up too long (or a cursor whose item
    /// is gone) is answered with the latest page, `reset`. The same sizes for every app (thin apps, brick 4).
    pub chosen: bool,
}

/// The sizes the bridge chooses (`Want::chosen`): the latest page, a catch-up at most (more: the latest page instead),
/// an older page.
pub const LATEST_PAGE: usize = 100;
pub const CATCH_UP_PAGE: usize = 200;
pub const OLDER_PAGE: usize = 50;

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
    let size = |chosen: usize| if want.chosen { chosen } else { want.limit.max(1) };
    let latest = |limit: usize| (n.saturating_sub(limit), n, None, true, None);
    let (start, end, anchor, reset, joins) = if let Some(around) = &want.around {
        match items.iter().position(|i| &i.id == around || i.id.starts_with(&format!("{around}."))) {
            Some(i) => {
                let start = i.saturating_sub(want.before_count);
                let end = want.after_count.map(|a| (i + 1 + a).min(n)).unwrap_or(n);
                (start, end, Some(items[i].id.clone()), true, None)
            }
            None => latest(size(LATEST_PAGE)),
        }
    } else if let Some(after) = &want.after {
        let (i, found) = at_cursor(&items, after);
        let start = if found { i + 1 } else { i };
        let limit = size(CATCH_UP_PAGE);
        if want.chosen && (!found || n.saturating_sub(start) > limit) {
            // the phone's last item gone (a rewind), or too much since: the latest page, as the window
            latest(LATEST_PAGE)
        } else {
            (start.min(n), (start + limit).min(n), None, false, found.then(|| after.clone()))
        }
    } else if let Some(before) = &want.before {
        let (i, found) = at_cursor(&items, before);
        if want.chosen && !found {
            latest(LATEST_PAGE)
        } else {
            (i.saturating_sub(size(OLDER_PAGE)), i, None, false, found.then(|| before.clone()))
        }
    } else {
        latest(size(LATEST_PAGE))
    };
    let more_before = start > 0;
    let more_after = end < n;
    let items = items.into_iter().skip(start).take(end.saturating_sub(start)).collect();
    Page { items, more_before, more_after, anchor, start, reset, joins }
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
        assert_eq!(atts[1]["offset"], 14);
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
    fn a_silent_reply_is_known() {
        let got = items(&[row(1, "user", json!("Vérifie mes mails")), row(2, "assistant", json!("[PAS UN MOT]")),
                          row(3, "assistant", json!("[Météo] Soleil."))]);
        let silent: Vec<bool> = got.iter().map(silent_reply).collect();
        assert_eq!(silent, vec![false, true, false]);
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
    fn a_turns_todo_calls_are_one_steps_item_where_its_last_call_is() {
        let list = |s1: &str, s2: &str| {
            json!(format!(
                r#"{{"todos": [{{"id": "1", "content": "Lire", "status": "{s1}"}}, {{"id": "2", "content": "Trier", "status": "{s2}"}}], "revision": 1}}"#
            ))
        };
        let mut rows = vec![
            row(1, "user", json!("Trie mes e-mails")),
            row(2, "assistant", json!("Je m'en occupe.")),
            row(3, "tool", list("in_progress", "pending")),
            row(4, "assistant", json!("")),
            row(5, "tool", json!("12 e-mails")),
            row(6, "tool", list("completed", "completed")),
            row(7, "assistant", json!("C'est trié.")),
            row(8, "user", json!("Merci")),
            row(9, "assistant", json!("")),
            row(10, "tool", json!(r#"{"error": "Todo item '1' has empty or missing content."}"#)),
            row(11, "assistant", json!("De rien.")),
        ];
        // directly, then through the tool search with another call in the same row, then a failed write
        rows[1].tool_calls = Some(json!([{"id": "a", "function": {"name": "todo_list", "arguments": "{}"}}]));
        rows[2].tool_call_id = Some("a".into());
        let wrapped = r#"{"calls": [{"name": "todo_list", "arguments": {"todos": []}}]}"#;
        rows[3].tool_calls = Some(json!([{"id": "b", "function": {"name": "mail_search", "arguments": "{}"}},
                                         {"id": "c", "function": {"name": "tool_call", "arguments": wrapped}}]));
        rows[4].tool_call_id = Some("b".into());
        rows[5].tool_call_id = Some("c".into());
        rows[8].tool_calls = Some(json!([{"id": "d", "function": {"name": "todo_list", "arguments": "{}"}}]));
        rows[9].tool_call_id = Some("d".into());
        let mut got = items(&rows);
        crate::steps::fold(&mut got);
        let kinds: Vec<&str> = got.iter().map(Item::kind).collect();
        assert_eq!(kinds, ["message", "message", "tool", "steps", "message", "message", "message"]);
        let steps = &got[3];
        let turn = got[0].id.clone();
        assert_eq!(steps.id, format!("{turn}.steps"));
        assert_eq!(steps.turn, turn);
        assert_eq!(steps.body["steps"]["done"], 2);
        assert_eq!(steps.body["steps"]["items"][1], json!({"text": "Trier", "status": "done"}));
        // where the last call is: after the mail search of the same row, before the reply
        assert_eq!(steps.pos, (0, 4, 2));
        assert!(got.iter().all(|i| i.body.get("tool").is_none_or(|t| t["name"] != "todo_list")));
        assert!(!draws(steps), "no day of its own: an older app never loses a day divider to it");
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
        // which one, for the apps' own words (they never read Hermes' English)
        assert_eq!(only_failed[1].body["notice"]["stopped"], "partial");
        let nothing = items(&[row(1, "user", json!("Range")), row(2, "assistant", json!(format!(" {FAILED_NOT_PROCESSED}\n")))]);
        assert_eq!(nothing[1].body["notice"]["stopped"], "not_processed");
        assert_eq!(stopped_kind(&format!("Hermes dit : {FAILED_PARTIAL}")), None, "a reply quoting it stays a reply");
    }

    /// Another agent's message in Bot Mode's conversation: its sender's id, the name it signed with, its words alone
    /// (iOS drew « Message from … » around the line that already had it).
    #[test]
    fn a_peers_message_comes_as_its_words_and_its_sender() {
        let got = items(&[row(1, "user", json!("Message from 🤖 Vie (@vie): Comment était la nuit ?\n\nBonne journée"))]);
        assert_eq!(got[0].body["role"], "peer");
        assert_eq!(got[0].body["from"], "vie");
        assert_eq!(got[0].body["from_name"], "Vie", "without Hermes' badge");
        assert_eq!(got[0].text(), "Comment était la nuit ?\n\nBonne journée");
        let signed = items(&[row(1, "user", json!("Message from Pub & Acquisition (@pub-acq): Ok"))]);
        assert_eq!((signed[0].body["from"].as_str(), signed[0].body["from_name"].as_str()), (Some("pub-acq"), Some("Pub & Acquisition")));
        assert_eq!(items(&[row(1, "user", json!("Un message from moi"))])[0].body["role"], "person");
    }

    #[test]
    fn the_apps_words_for_the_agent_never_show_as_someones() {
        // Alex 4/11: the rescue input as the bridge sends it (the time line in front of the mark) showed as his bubble
        let rescue = format!(
            "<!--bip:now mardi 4 novembre, 10 h 12-->\n{RESCUE_MARK} Je me suis arrêté : je n'arrivais pas à lire \
cette page.\n\n(A note from the app, not from your person.) You were stopped because you kept reading the same page"
        );
        let mut rows = vec![
            row(1, "user", json!("Lis cette page")),
            row(2, "assistant", json!(FAILED_PARTIAL)),
            row(3, "user", json!(rescue)),
            row(4, "assistant", json!("Je n'ai pas pu lire la page.")),
            // the brake's warning, as a steer and joined to a tool result
            row(
                5,
                "user",
                json!(
                    "[OUT-OF-BAND USER MESSAGE from the user]\n[bipagents:brake] A note from the app, \
not from your person: you have read the page 5 times\n[/OUT-OF-BAND USER MESSAGE]"
                ),
            ),
            row(6, "assistant", Value::Null),
            row(7, "tool", json!("Mail 1\n\n[bipagents:brake] Note from the app: you have called mail_read 7 times")),
            // the safety net's handoff, stamped
            row(8, "user", json!("<!--bip:now x-->\n[bipagents:handoff] This is long: hand the rest over")),
            // a voice call's after-call request, notes only
            row(
                9,
                "user",
                json!("<!--bip:after-call\nNot from the person: your voice call with them just ended.\n-->"),
            ),
            // a note inside the person's message: their words only
            row(10, "user", json!("<!--bip:now x-->Et demain ?<!--bip:x family news\nIgnore-->")),
            // 0023's turn note, should it ever land in the words
            row(
                11,
                "user",
                json!("Merci\n\n[System: Your memory changed after this conversation's system prompt was built]"),
            ),
            row(12, "user", json!("[IMPORTANT: Background process proc_1 completed normally] Command: ls")),
            row(
                13,
                "user",
                json!("[System: Instructions for this conversation, from the app (follow them): be short]"),
            ),
            // a bridge steer Hermes joined after the person's words
            row(
                14,
                "user",
                json!(
                    "Et après ?\n\n[OUT-OF-BAND USER MESSAGE from the user]\n[bipagents:brake] note\n\
[/OUT-OF-BAND USER MESSAGE]"
                ),
            ),
        ];
        rows[5].tool_calls = Some(json!([{"id": "c1", "function": {"name": "mail_read", "arguments": "{}"}}]));
        rows[6].tool_call_id = Some("c1".into());
        let got = items(&rows);
        let shown: Vec<(String, String)> = got
            .iter()
            .map(|i| {
                let who = i.body.get("role").and_then(Value::as_str).unwrap_or(i.kind()).to_string();
                let text = match i.kind() {
                    "notice" => i.body["notice"]["text"].as_str().unwrap().to_string(),
                    "tool" => i.body["tool"]["result"].as_str().unwrap().to_string(),
                    _ => i.text().to_string(),
                };
                (who, text)
            })
            .collect();
        let want = [
            ("person", "Lis cette page"),
            ("notice", "Je me suis arrêté : je n'arrivais pas à lire cette page."),
            ("agent", "Je n'ai pas pu lire la page."),
            ("tool", "Mail 1"),
            ("person", "Et demain ?"),
            ("person", "Merci"),
            ("person", "Et après ?"),
        ];
        assert_eq!(shown, want.map(|(a, b)| (a.to_string(), b.to_string())));
        assert_eq!(got[1].body["notice"]["code"], "loop_rescue");
        for item in &got {
            let served = item.to_json("t", false).to_string();
            assert!(
                !served.contains("bipagents:") && !served.contains("<!--") && !served.contains("[System:"),
                "{served}"
            );
        }
        // a photo sent with notes only still is the person's
        let mut photo = row(20, "user", json!([{"type": "text", "text": "<!--bip:now x-->"}, {"type": "image_url"}]));
        photo.uid = Some("p".into());
        let got = items(&[photo]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].body["attachments"][0]["kind"], "image");
    }

    #[test]
    fn an_in_place_compaction_shows_each_message_once_and_keeps_what_the_summary_absorbed() {
        // as Hermes v0.22.5 left the lab's conversation of 2026-11-03 (state.db rows 151-177, two passes)
        let msg = |id: i64, role: &str, uid: &str, text: &str, active: bool, compacted: bool| {
            let mut r = row(id, role, json!(text));
            r.uid = Some(uid.into());
            r.shown = active || compacted;
            r
        };
        let mut summary = msg(163, "assistant", "u3a", "[PRIOR CONTEXT …]", true, true);
        summary.summary = true;
        let rows = vec![
            msg(151, "user", "u1", "Note 1", false, true),
            msg(152, "assistant", "u1a", "ok 1", false, true),
            msg(154, "user", "u2", "Note 2", false, true),
            msg(155, "assistant", "u2a", "ok 2", false, true),
            msg(156, "user", "u3", "Note 3", false, true),
            msg(157, "assistant", "u3a", "ok 3", false, false),
            msg(160, "user", "u1", "Note 1", false, true),
            msg(161, "assistant", "u1a", "ok 1", false, true),
            msg(162, "user", "u2", "Note 2", false, true),
            summary,
            msg(164, "user", "u4", "Note 4", false, true),
            msg(165, "assistant", "u4a", "ok 4", false, false),
            msg(170, "assistant", "u4a", "ok 4", true, false),
            msg(171, "user", "u5", "Note 5", true, false),
        ];
        let got: Vec<String> = items(&rows).iter().map(|i| i.text().to_string()).collect();
        assert_eq!(got, ["Note 1", "ok 1", "Note 2", "ok 2", "Note 3", "ok 3", "Note 4", "ok 4", "Note 5"]);
        let ids: HashSet<String> = items(&rows).into_iter().map(|i| i.id).collect();
        assert_eq!(ids.len(), 9);
    }

    #[test]
    fn a_compaction_leaves_every_message_at_its_first_row() {
        // as Hermes v0.22.5 left the build server's conversation (internal/notes/history-compaction.md, rows of
        // tests_blackbox/fixtures/hermes_compaction/updated_03{2,3}.json): its 2nd compaction carried « ok 24 »,
        // « Note 25 », « ok 25 » (originals 48-50 superseded, copies 102-104), its 3rd folded « Note 25 » into the
        // summary (119 takes its uid) and carried « ok 25 » again (120)
        let msg = |id: i64, role: &str, uid: &str, text: &str, active: bool, compacted: bool| {
            let mut r = row(id, role, json!(text));
            r.uid = Some(uid.into());
            r.shown = active || compacted;
            r
        };
        let before = vec![
            msg(47, "user", "n24", "Note 24", false, true),
            msg(48, "assistant", "o24", "ok 24", false, false),
            msg(49, "user", "n25", "Note 25", false, false),
            msg(50, "assistant", "o25", "ok 25", false, false),
            msg(102, "assistant", "o24", "ok 24", true, false),
            msg(103, "user", "n25", "Note 25", true, false),
            msg(104, "assistant", "o25", "ok 25", true, false),
        ];
        let mut after = before.clone();
        after[4].shown = true; // compacted
        after[5].shown = false; // superseded
        after[6].shown = false;
        let mut summary = msg(119, "user", "n25", "[CONTEXT COMPACTION …]", true, false);
        summary.summary = true;
        after.extend([summary, msg(120, "assistant", "o25", "ok 25", true, false)]);
        let placed = |rows: &[Row]| -> Vec<(String, String, i64)> {
            items(rows).into_iter().map(|i| (i.id.clone(), i.text().to_string(), i.pos.1)).collect()
        };
        let want = [("m_n24", "Note 24", 47), ("m_o24", "ok 24", 48), ("m_n25", "Note 25", 49), ("m_o25", "ok 25", 50)]
            .map(|(id, text, at)| (id.to_string(), text.to_string(), at));
        assert_eq!(placed(&before), want);
        assert_eq!(placed(&after), want, "a message moved at the compaction");
    }

    #[test]
    fn a_photo_saved_by_hermes_is_an_attachment_not_words() {
        let raw = "Voici une photo.\n\n[Image attached at: /home/bip/.hermes/profiles/vie/cache/documents/x.png]\n[screenshot]";
        let (text, atts) = visible(raw, "m_1", 0);
        assert_eq!(text, "Voici une photo.");
        assert_eq!(atts.len(), 1);
        assert_eq!(atts[0]["kind"], "image");
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

    fn said(id: &str, n: i64, at: f64, body: Value) -> Item {
        Item {
            id: id.into(),
            pos: (0, n, 0),
            at,
            turn: String::new(),
            segment: String::new(),
            row: n,
            body: body.as_object().unwrap().clone(),
        }
    }

    /// 5/11 (Alex, Wellness): « Dimanche 1 novembre »… « Hier » with nothing under them. A day is the person's (their
    /// zone, not the server's), opened by the first row that draws something; one with nothing to show opens none.
    #[test]
    fn days_are_opened_by_rows_that_draw_something_where_the_person_is() {
        let paris: Tz = "Europe/Paris".parse().unwrap();
        // 2026-11-01 22:30 UTC = 23:30 in Paris (1/11); 23:30 UTC = 00:30 in Paris (2/11)
        let sun = 1_793_572_200.0;
        let all = vec![
            said("m_1", 1, sun, json!({"kind": "message", "role": "person", "text": "Bonsoir"})),
            said("m_2", 2, sun + 3600.0, json!({"kind": "message", "role": "agent", "text": "  "})),
            said("m_3", 3, sun + 3700.0, json!({"kind": "message", "role": "agent", "text": "", "reasoning": "hm"})),
            said("m_4", 4, sun + 3800.0, json!({"kind": "message", "role": "agent", "text": "Bonne nuit"})),
            said("m_5", 5, sun + 2.0 * 86_400.0, json!({"kind": "message", "role": "person", "text": ""})),
            said("m_6", 6, sun + 2.0 * 86_400.0 + 60.0, json!({"kind": "tool", "tool": {"name": "x"}})),
            said("m_7", 7, 0.0, json!({"kind": "message", "role": "person", "text": "sans heure"})),
        ];
        let days = days(&all, paris);
        let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(days[0], Some((day("2026-11-01"), true)));
        assert_eq!(days[1], None, "blank words draw nothing");
        assert_eq!(days[2], Some((day("2026-11-02"), true)), "reasoning draws, after midnight in Paris");
        assert_eq!(days[3], Some((day("2026-11-02"), false)));
        assert_eq!(days[4], None, "a message with nothing to show");
        assert_eq!(days[5], Some((day("2026-11-03"), true)));
        assert_eq!(days[6], None, "no time, no day");
        // the same rows in UTC: still the 1st at 23:30 UTC
        assert_eq!(super::days(&all, Tz::UTC)[2], Some((day("2026-11-01"), false)));
        // a page that starts inside a day: its rows have the day, the divider stays where the day began
        let p = page(all.clone(), &Want { limit: 4, ..Default::default() }).with_days(&days);
        let json = p.answer(json!({"id": "t_1"}), 1, Value::Null, false);
        let items = json["items"].as_array().unwrap();
        assert_eq!(items[0]["id"], "m_4");
        assert_eq!(items[0]["day"], "2026-11-02");
        assert!(items[0].get("new_day").is_none());
        assert!(items[1].get("day").is_none());
        assert_eq!(items[2]["new_day"], true);
    }

    /// 4/11 (iPhone): the message sent showed twice, its `s_` item and its stored row. The page names the `s_` items
    /// its stored rows replace; a message still being sent replaces nothing; an old one is not named any more.
    #[test]
    fn a_stored_sent_message_names_the_item_it_replaces() {
        let now = chrono::Utc::now().timestamp() as f64;
        let all = vec![
            said("m_old", 1, now - 432_000.0, json!({"kind": "message", "role": "person", "text": "a", "client_id": "C0"})),
            said("m_1", 2, now - 60.0, json!({"kind": "message", "role": "person", "text": "b", "client_id": "C1"})),
            said("m_2", 3, now - 50.0, json!({"kind": "message", "role": "agent", "text": "ok"})),
            said("s_C2", 4, now, json!({"kind": "message", "role": "person", "text": "c", "client_id": "C2",
                                       "sending": true})),
        ];
        let want = Want { limit: 50, ..Default::default() };
        let json = page(all, &want).answer(json!({"id": "t_1"}), 1, Value::Null, false);
        assert_eq!(json["replaced"], json!(["s_C1"]));
        let none = page(vec![], &want).answer(json!({"id": "t_1"}), 1, Value::Null, false);
        assert!(none.get("replaced").is_none());
    }

    /// Thin apps, brick 4: the bridge chooses the sizes and says how a page goes into the phone's window: `reset` (it is
    /// the window) or `joins` (the cursor it goes after, or in front of). A catch-up too long, or from an item gone, is
    /// the latest page, `reset`. An app giving `limit` (before this) gets the pages as before.
    #[test]
    fn a_page_says_how_it_joins_the_window() {
        let all: Vec<Item> = (0..300)
            .map(|n| said(&format!("m_{n}"), n, 1000.0 + n as f64, json!({"kind": "message", "role": "person", "text": "x"})))
            .collect();
        let chosen = |w: Want| Want { chosen: true, ..w };
        let latest = page(all.clone(), &chosen(Want::default()));
        assert_eq!((latest.items.len(), latest.reset, latest.joins.clone()), (LATEST_PAGE, true, None));
        // a catch-up: after the cursor, joined to it
        let cursor = all[280].cursor();
        let after = page(all.clone(), &chosen(Want { after: Some(cursor.clone()), ..Default::default() }));
        assert_eq!((after.items.len(), after.reset, after.joins.as_deref()), (19, false, Some(cursor.as_str())));
        assert!(!after.more_after);
        // too much since: the latest page instead, as the window
        let far = page(all.clone(), &chosen(Want { after: Some(all[10].cursor()), ..Default::default() }));
        assert!(far.reset && far.joins.is_none() && far.items.last().unwrap().id == "m_299" && far.items.len() == LATEST_PAGE);
        // its item gone (a rewind): the latest page too
        let gone = cursor_of((all[5].pos.0, all[5].pos.1, all[5].pos.2 + 9));
        assert!(page(all.clone(), &chosen(Want { after: Some(gone.clone()), ..Default::default() })).reset);
        // an older page, in front of the window's first item
        let first = all[250].cursor();
        let older = page(all.clone(), &chosen(Want { before: Some(first.clone()), ..Default::default() }));
        assert_eq!((older.items.len(), older.reset, older.joins.as_deref()), (OLDER_PAGE, false, Some(first.as_str())));
        assert_eq!(older.items.last().unwrap().id, "m_249");
        assert!(page(all.clone(), &chosen(Want { before: Some(gone), ..Default::default() })).reset);
        // around a message: the window
        let around = page(all.clone(), &chosen(Want { around: Some("m_100".into()), before_count: 20, ..Default::default() }));
        assert!(around.reset && around.items.first().unwrap().id == "m_80");
        // an app's own limit: as before (no reset for a long catch-up)
        let own = page(all.clone(), &Want { limit: 500, after: Some(all[10].cursor()), ..Default::default() });
        assert!(!own.reset && own.items.len() == 289 && own.joins.is_some());
        let json = after.answer(json!({"id": "t_1"}), 1, Value::Null, false);
        assert_eq!((json["reset"].clone(), json["joins"].clone()), (json!(false), json!(cursor)));
    }
}
