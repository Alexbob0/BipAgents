//! How a conversation's turns lay out, decided once for every app (thin apps, brick 7): which lines are the agent's
//! work (folded into « Détail du travail »), which text is a turn's reply (its time and « Écouter »), where the app
//! puts its own first word for a turn the agent started working in without one, and how an exchange with another agent
//! folds. Until 7/11 each app worked this out from the thread itself (iOS `TurnLayout`, `TurnAcknowledgement`,
//! `RowLayout`, `ConversationView.exchangeOwners`; Android `ThreadLayout`), with small differences (Android folded
//! answered cards into an exchange; it ended a turn's « acknowledgement » window at a message added to it, the iPhone
//! did not).
//!
//! The items of a page, in the turns that are over, carry `layout` (docs/conversations.md §6.3); keys name the
//! screen's lines: an item draws one line, a message with reasoning two (`<id>.r` above `<id>`), consecutive tools one
//! card (keyed by its first tool's id):
//!
//! - `work: true`: its line is the agent's work (a tool card, the agent's short commentary between calls), folded
//!   with the work lines right before and after it; `reasoning_work`: the same for the message's reasoning line;
//! - `turn_end: true`: its text is its turn's reply (the last real text before the next message starting a turn);
//! - `ack: {line, index}`: the app's own first word goes before this item's first line; `line` is the tool card whose
//!   call number `index` (among its calls, a `tool_call` batch unwrapped) is the turn's first real call: the app
//!   words it from that call (« Je lance le navigateur sur kiabi.com… »);
//! - `exchange`: the peer message opening the exchange this line folds into; `request: true` on a peer message asking
//!   this agent something (not answering a `message_agent` it sent); `answers: <name>` on the first reply to a peer's
//!   request.
//!
//! An item carries `layout` only when there is something to say; the page says which items are decided
//! (`layout: {open}`, the first item of the turn still going: what it is decides as it ends, and the app keeps its own
//! rule for it, also for the lines it streams live, and with an older bridge). Lines are worked out over the whole
//! thread (a turn, an exchange, starts before the page), fields only for the page's items.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::timeline::Item;

/// The longest text that still counts as commentary between tools: an answer written before the agent goes on
/// working (updating its notes…) stays in sight.
pub const COMMENTARY_LENGTH: usize = 200;
/// How far back a peer's message looks for this agent's `message_agent` to that peer (an answer, not a request).
const ASKED_WITHIN: usize = 40;

/// A call: its name, its arguments, its preview (the result, else the arguments).
type Call<'a> = (&'a str, Option<&'a str>, Option<&'a str>);

#[derive(Debug, Clone, PartialEq)]
enum Kind<'a> {
    /// A message from the person (or a peer) that starts a turn.
    Prompt,
    /// A message from the person added to the answer in progress.
    Added,
    /// The agent's words: their length, whether they carry a file, whether they are Hermes' line closing a stopped
    /// turn (never an answer).
    Reply { length: usize, media: bool, interruption: bool },
    Tools(Vec<Call<'a>>),
    Reasoning,
    /// A Boîte message placed at its time (a scheduled report): ends an exchange.
    Scheduled,
    /// An answered card (stays in sight in an exchange).
    Card,
    Other,
}

#[derive(Debug, Clone)]
struct Line<'a> {
    /// Its key on screen: the item's id, `<id>.r` for a reasoning line, the first tool's id for a card.
    key: String,
    /// The items it draws (a card: its tools).
    items: Vec<usize>,
    kind: Kind<'a>,
    /// A peer's message: its handle and name.
    peer: Option<(&'a str, &'a str)>,
}

fn s<'a>(item: &'a Item, key: &str) -> &'a str {
    item.body.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The screen's lines, as the apps draw the items (iOS `chatItems(from:)`, Android `TimelineItems.items`).
fn lines(items: &[Item]) -> Vec<Line<'_>> {
    let mut out: Vec<Line> = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let files = item.body.get("attachments").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        let line = |key: String, kind| Line { key, items: vec![n], kind, peer: None };
        match item.kind() {
            "message" => match s(item, "role") {
                "agent" | "assistant" => {
                    if !s(item, "reasoning").trim().is_empty() {
                        out.push(line(format!("{}.r", item.id), Kind::Reasoning));
                    }
                    let text = item.text();
                    if !text.is_empty() || files {
                        let (length, interruption) = (text.trim().chars().count(), interruption_line(text));
                        out.push(line(item.id.clone(), Kind::Reply { length, media: files, interruption }));
                    }
                }
                role => {
                    if item.text().is_empty() && !files {
                        continue;
                    }
                    let added = item.body.get("added_during_run").and_then(Value::as_bool) == Some(true);
                    let mut l = line(item.id.clone(), if added { Kind::Added } else { Kind::Prompt });
                    if role == "peer" {
                        let handle = item.body.get("from").and_then(Value::as_str).unwrap_or("agent");
                        l.peer = Some((handle, item.body.get("from_name").and_then(Value::as_str).unwrap_or(handle)));
                    }
                    out.push(l);
                }
            },
            "tool" => {
                let Some(tool) = item.body.get("tool") else { continue };
                let field = |k: &str| tool.get(k).and_then(Value::as_str);
                let call = (field("name").unwrap_or(""), field("arguments"), field("result").or_else(|| field("arguments")));
                match out.last_mut() {
                    Some(Line { kind: Kind::Tools(calls), items, .. }) => {
                        calls.push(call);
                        items.push(n);
                    }
                    _ => out.push(line(item.id.clone(), Kind::Tools(vec![call]))),
                }
            }
            "boite" => out.push(line(item.id.clone(), Kind::Scheduled)),
            "approval" => out.push(line(item.id.clone(), Kind::Card)),
            // the agent's « Étapes » card: a line of its own (a card after it is another one)
            "steps" if item.body.get("steps").is_some_and(|s| !s.is_null()) => out.push(line(item.id.clone(), Kind::Other)),
            _ if super::timeline::draws(item) => out.push(line(item.id.clone(), Kind::Other)),
            _ => {}
        }
    }
    out
}

/// Hermes' own line closing a turn it stopped (`abort_turn_on_interrupt`): not an answer from the agent.
pub fn interruption_line(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("Operation interrupted") || t.starts_with("[interrupt:") || t == "[response interrupted]"
}

/// The turns: ranges of lines, each from a message starting one (the first may start without one).
fn turns(lines: &[Line]) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start + 1;
        while end < lines.len() && lines[end].kind != Kind::Prompt {
            end += 1;
        }
        out.push(start..end);
        start = end;
    }
    out
}

fn real_reply(kind: &Kind) -> bool {
    matches!(kind, Kind::Reply { interruption: false, .. })
}

/// The agent's work lines (iOS `TurnLayout.workFolds`): tools, reasoning, and the agent's short commentary between
/// tools — a text followed in its turn by a tool, then by another real text; the words before the turn's first tool
/// (its acknowledgement) never are. The apps fold consecutive work lines together.
fn work(lines: &[Line], turns: &[Range<usize>]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    for turn in turns {
        let mut tools_yet = false;
        for i in turn.clone() {
            out[i] = match &lines[i].kind {
                Kind::Tools(_) => {
                    tools_yet = true;
                    true
                }
                Kind::Reasoning => true,
                Kind::Reply { length, media, interruption } => {
                    let later = &lines[i + 1..turn.end];
                    tools_yet
                        && !media
                        && !interruption
                        && *length <= COMMENTARY_LENGTH
                        && later.iter().any(|l| matches!(l.kind, Kind::Tools(_)))
                        && later.iter().any(|l| real_reply(&l.kind))
                }
                _ => false,
            };
        }
    }
    out
}

/// Each line that is its turn's reply (iOS `RowLayout.turnEnds`): the last real text before the next message starting
/// a turn (or the end).
fn turn_ends(lines: &[Line]) -> Vec<bool> {
    let mut ends = vec![false; lines.len()];
    let mut decided: Option<bool> = None;
    for i in (0..lines.len()).rev() {
        ends[i] = decided.unwrap_or(true);
        match &lines[i].kind {
            Kind::Prompt => decided = Some(true),
            kind if real_reply(kind) => decided = Some(false),
            _ => {}
        }
    }
    ends
}

static CUT_NAMES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""name"\s*:\s*"([A-Za-z][A-Za-z0-9_]*_[A-Za-z0-9_]*)""#).expect("valid regex"));

/// The calls inside a `tool_call` (the apps' `ToolCalls.unwrap`), else the call itself. Hermes' labels (« Scheduling »)
/// only name live previews, never stored arguments: a batch without a readable name stays the dispatcher.
fn unwrapped(name: &str, arguments: Option<&str>) -> Vec<String> {
    let same = || vec![name.to_string()];
    let text = arguments.map(str::trim).unwrap_or("");
    if name != "tool_call" || text.is_empty() {
        return same();
    }
    if let Ok(json) = serde_json::from_str::<Value>(text) {
        if !json.is_object() {
            return same();
        }
        let mut calls = json.get("calls").cloned();
        if let Some(Value::String(s)) = &calls {
            calls = serde_json::from_str(s).ok();
        }
        if calls.is_none() && json.get("name").is_some() {
            calls = Some(json.clone());
        }
        if let Some(single) = calls.as_ref().filter(|c| c.is_object()) {
            calls = Some(Value::Array(vec![single.clone()]));
        }
        let found: Vec<String> = calls
            .as_ref()
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        return if found.is_empty() { same() } else { found };
    }
    if text.starts_with('{') || text.starts_with('[') {
        let names: Vec<String> = CUT_NAMES.captures_iter(text).map(|c| c[1].to_string()).collect();
        if !names.is_empty() {
            return names;
        }
    }
    same()
}

/// A tool's kind id (the apps' `ToolLabel.kind(of:server:)`): its name without its MCP server, exact names, then
/// prefixes, the agent's browser, another server's mail tools.
pub fn kind_of(tool: &str) -> &'static str {
    let (server, name) = if let Some(rest) = tool.strip_prefix("mcp__") {
        match rest.split_once("__") {
            Some((server, name)) => (Some(server), name),
            None => (None, rest),
        }
    } else {
        ["bipagents_files", "bipagents_pay", "bipagents", "stealth"]
            .iter()
            .find_map(|server| tool.strip_prefix(&format!("mcp_{server}_")).map(|name| (Some(*server), name)))
            .unwrap_or((None, tool))
    };
    let name = name.to_lowercase();
    if name.trim().is_empty() {
        return "tool";
    }
    let kinds = crate::tool_kinds::KINDS;
    if let Some(k) = kinds.iter().find(|k| k.names.contains(&name.as_str())) {
        return k.id;
    }
    if let Some(k) = kinds.iter().find(|k| k.prefixes.iter().any(|p| name.starts_with(p))) {
        return k.id;
    }
    if server == Some("stealth") || name.contains("browser") {
        return "browser";
    }
    if name.contains("mail") || name.contains("inbox") {
        return mail_kind(&name);
    }
    "unknown"
}

/// Another server's mail tool by its verb (the apps' `ToolLabel.mailKind`): a draft or a sending, an attachment, a
/// reading, else a search.
fn mail_kind(name: &str) -> &'static str {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphabetic()).collect();
    let any = |of: &[&str]| words.iter().any(|w| of.contains(w));
    if any(&["draft", "drafts", "send", "compose", "reply", "forward"]) {
        "mailDraft"
    } else if name.contains("attachment") {
        "mailAttachment"
    } else if any(&["read", "get", "fetch", "open", "message", "thread"]) {
        "mailRead"
    } else {
        "mailSearch"
    }
}

fn bookkeeping(tool: &str) -> bool {
    crate::tool_kinds::BOOKKEEPING.contains(&kind_of(tool))
}

/// The app's own first word (iOS `TurnAcknowledgement.placement`): for a turn whose first real call (not bookkeeping)
/// came before any word of the agent, the line it goes before (the first of the work leading to that call), the card
/// and the call's number in it.
fn acknowledgement(lines: &[Line], turn: Range<usize>) -> Option<(usize, usize, usize)> {
    if lines[turn.start].kind != Kind::Prompt {
        return None;
    }
    for i in turn.start + 1..turn.end {
        match &lines[i].kind {
            Kind::Reply { length, interruption: false, .. } if *length > 0 => return None,
            Kind::Tools(calls) => {
                let names: Vec<String> = calls.iter().flat_map(|(n, a, _)| unwrapped(n, *a)).collect();
                let Some(first) = names.iter().position(|n| !bookkeeping(n)) else { continue };
                let mut before = i;
                while before - 1 > turn.start && matches!(lines[before - 1].kind, Kind::Tools(_) | Kind::Reasoning) {
                    before -= 1;
                }
                return Some((before, i, first));
            }
            _ => {}
        }
    }
    None
}

/// A peer's message asks this agent something unless this agent sent that peer a `message_agent` just before.
fn asks(lines: &[Line], at: usize, handle: &str) -> bool {
    let handle = handle.to_lowercase();
    !lines[at.saturating_sub(ASKED_WITHIN)..at].iter().any(|l| match &l.kind {
        Kind::Tools(calls) => calls
            .iter()
            .any(|(n, _, p)| n.contains("message_agent") && p.unwrap_or("").to_lowercase().contains(&handle)),
        _ => false,
    })
}

/// What the bridge says of a page's layout: each item's fields (only those with something to say), and the first
/// item of the turn going on (`open`: from it on, nothing is decided).
#[derive(Debug, Default)]
pub struct Layout {
    pub fields: HashMap<usize, Map<String, Value>>,
    pub open: Option<usize>,
}

/// The `layout` of the items in `page` (indices into `items`, the whole thread), those of the turns that are over
/// (the module's doc); `running`: a run goes on in the conversation (its last turn is not decided yet).
pub fn layout(items: &[Item], page: Range<usize>, running: bool) -> Layout {
    let lines = lines(items);
    let all = turns(&lines);
    // the items of the turn going on: none (a turn starts at its message's item)
    let open_from = match (running, all.last()) {
        (true, Some(last)) => lines[last.start].items[0],
        (true, None) => 0,
        _ => items.len(),
    };
    let shown = page.start..page.end.min(open_from);
    let mut out: HashMap<usize, Map<String, Value>> = shown.clone().map(|n| (n, Map::new())).collect();
    // only the lines with an item on the page (and the turns holding one) are worked out field by field
    let on_page = |l: &Line| l.items.iter().any(|n| shown.contains(n));
    let put = |out: &mut HashMap<usize, Map<String, Value>>, line: &Line, key: &str, value: Value| {
        for n in &line.items {
            if let Some(fields) = out.get_mut(n) {
                fields.insert(key.to_string(), value.clone());
            }
        }
    };
    let work = work(&lines, &all);
    for line in lines.iter().zip(&work).filter(|(l, w)| **w && on_page(l)).map(|(l, _)| l) {
        let key = if line.kind == Kind::Reasoning { "reasoning_work" } else { "work" };
        put(&mut out, line, key, json!(true));
    }
    for (i, end) in turn_ends(&lines).into_iter().enumerate() {
        if end && matches!(lines[i].kind, Kind::Reply { .. }) && on_page(&lines[i]) {
            put(&mut out, &lines[i], "turn_end", json!(true));
        }
    }
    for turn in all.iter().filter(|t| lines[t.start..t.end].iter().any(&on_page)) {
        if let Some((before, card, index)) = acknowledgement(&lines, turn.clone()) {
            // on the item whose first line it goes before (a message's reasoning, a card's first tool)
            if let Some(fields) = out.get_mut(&lines[before].items[0]) {
                fields.insert("ack".into(), json!({"line": lines[card].key, "index": index}));
            }
        }
    }
    // exchanges with other agents (Bot Mode): a peer's request folds with the turn answering it, an answer to this
    // agent's request alone; a scheduled report ends it; an answered card stays in sight
    let mut current: Option<&str> = None;
    let mut asker: Option<&str> = None;
    for (i, line) in lines.iter().enumerate() {
        match (&line.kind, line.peer) {
            (Kind::Prompt | Kind::Added, peer) => {
                current = None;
                asker = None;
                if let Some((handle, name)) = peer {
                    put(&mut out, line, "exchange", json!(line.key));
                    if asks(&lines, i, handle) {
                        put(&mut out, line, "request", json!(true));
                        current = Some(&line.key);
                        asker = Some(name);
                    }
                }
            }
            (Kind::Scheduled, _) => current = None,
            (Kind::Card, _) => {}
            (kind, _) => {
                if let Some(owner) = current {
                    put(&mut out, line, "exchange", json!(owner));
                }
                if matches!(kind, Kind::Reply { .. }) && asker.is_some() {
                    put(&mut out, line, "answers", json!(asker.take()));
                }
            }
        }
    }
    // an item with nothing to say carries nothing (the page's `layout.open` says which are decided)
    out.retain(|_, fields| !fields.is_empty());
    Layout { fields: out, open: (open_from < items.len()).then_some(open_from) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, body: Value) -> Item {
        Item {
            id: id.into(),
            pos: (0, 0, 0),
            at: 0.0,
            turn: String::new(),
            segment: "s".into(),
            row: 0,
            body: body.as_object().cloned().unwrap_or_default(),
        }
    }
    fn person(id: &str, text: &str) -> Item {
        item(id, json!({"kind": "message", "role": "person", "text": text}))
    }
    fn added(id: &str, text: &str) -> Item {
        item(id, json!({"kind": "message", "role": "person", "text": text, "added_during_run": true}))
    }
    fn agent(id: &str, text: &str) -> Item {
        item(id, json!({"kind": "message", "role": "agent", "text": text}))
    }
    fn tool(id: &str, name: &str, arguments: &str) -> Item {
        item(id, json!({"kind": "tool", "tool": {"name": name, "arguments": arguments, "status": "done"}}))
    }
    fn all(items: &[Item], running: bool) -> HashMap<usize, Map<String, Value>> {
        layout(items, 0..items.len(), running).fields
    }
    fn field<'a>(l: &'a HashMap<usize, Map<String, Value>>, n: usize, key: &str) -> Option<&'a Value> {
        l.get(&n).and_then(|f| f.get(key))
    }

    /// The apps' TurnLayoutTests `steeredTurnKeepsItsAnswerAndMessageInSight`, on items.
    #[test]
    fn a_steered_turn_keeps_its_answer_and_message_in_sight() {
        let items = vec![
            person("m_0", "Remplis mon Carnet"),
            agent("m_1", "Je commence."),
            tool("m_1.t0", "notebook_note", "{}"),
            agent("m_2", "Je continue."),
            added("m_3", "Tu as fini ?"),
            tool("m_3.t0", "notebook_note", "{}"),
            agent("m_4", "Presque, encore une page."),
            agent("m_5", "Ton Carnet est rempli, voilà ce qu'il contient : 12 pages."),
            person("m_6", "Hello ?"),
            agent("m_7", "Coucou Alex ! Je suis là."),
            tool("m_7.t0", "memory", "{}"),
        ];
        let l = all(&items, false);
        let work = |n| field(&l, n, "work") == Some(&json!(true));
        assert!(!work(1), "the acknowledgement is in sight");
        assert!(work(2) && work(3), "the work and the commentary after it");
        assert!(!work(4) && work(5));
        assert!(!work(6) && !work(7) && !work(8) && !work(9));
        assert!(work(10));
        // the turn's reply: its last text
        assert_eq!(field(&l, 7, "turn_end"), Some(&json!(true)));
        assert_eq!(field(&l, 6, "turn_end"), None);
        assert_eq!(field(&l, 9, "turn_end"), Some(&json!(true)), "bookkeeping after it: still the reply");
        // a message from the person says nothing: no layout; the agent's lines one each
        assert!(!l.contains_key(&0) && !l.contains_key(&4) && !l.contains_key(&1));
        // no acknowledgement: the agent spoke first in both turns
        assert!(l.values().all(|f| !f.contains_key("ack")));
    }

    #[test]
    fn a_stopped_turn_keeps_its_answer() {
        let items = vec![
            person("m_0", "Va"),
            agent("m_1", "Je regarde."),
            tool("m_1.t0", "web_search", "{}"),
            agent("m_2", "C'est fait : 12 pages dans ton Carnet."),
            tool("m_2.t0", "web_search", "{}"),
            agent("m_3", "Operation interrupted: handling API error (timeout)."),
        ];
        let l = all(&items, false);
        assert_eq!(field(&l, 3, "work"), None);
        assert_eq!(field(&l, 3, "turn_end"), Some(&json!(true)));
        assert_eq!(field(&l, 5, "turn_end"), Some(&json!(true)), "the end of the thread, as the apps had it");
    }

    #[test]
    fn the_turn_going_on_is_not_decided() {
        let items = vec![person("m_0", "A"), agent("m_1", "B"), person("m_2", "C"), tool("m_2.t0", "web_search", "{}")];
        let l = all(&items, true);
        assert_eq!(l.len(), 1, "only the first turn's agent item: {l:?}");
        assert_eq!((layout(&items, 0..4, true).open, layout(&items, 0..4, false).open), (Some(2), None));
        assert_eq!(field(&l, 1, "turn_end"), Some(&json!(true)));
    }

    /// The apps' TurnAcknowledgementTests, on items.
    #[test]
    fn a_turn_straight_into_its_work_gets_the_apps_first_word() {
        let mut thinking = agent("m_1", "");
        thinking.body.insert("reasoning".into(), json!("hm"));
        let items = vec![
            person("m_0", "Les vestes ?"),
            thinking,
            tool("m_1.t0", "browser_navigate", r#"{"url": "https://www.kiabi.com/vestes"}"#),
            tool("m_1.t1", "browser_click", r#"{"text": "Taille 4 ans"}"#),
            agent("m_2", "Voici les prix des vestes…"),
        ];
        let l = all(&items, false);
        // before the reasoning (the work leading to the call), the call 0 of the card m_1.t0
        assert_eq!(field(&l, 1, "ack"), Some(&json!({"line": "m_1.t0", "index": 0})));
        assert_eq!(field(&l, 1, "reasoning_work"), Some(&json!(true)));
        assert_eq!((field(&l, 2, "work"), field(&l, 3, "work")), (Some(&json!(true)), Some(&json!(true))));
        // bookkeeping first, the search in the same card: the search, before the whole card
        let items = vec![
            person("m_0", "Vestes"),
            tool("m_0.t0", "tool_search", r#"{"queries": ["web"]}"#),
            tool("m_0.t1", "todo", ""),
            tool("m_0.t2", "web_search", r#"{"query": "vestes enfant"}"#),
            agent("m_1", "Voilà."),
        ];
        assert_eq!(field(&all(&items, false), 1, "ack"), Some(&json!({"line": "m_0.t0", "index": 2})));
        // only bookkeeping, or the agent spoke first: none
        let items = vec![person("m_0", "x"), tool("m_0.t0", "memory", ""), tool("m_0.t1", "skills_list", ""), agent("m_1", "ok")];
        assert!(all(&items, false).values().all(|f| !f.contains_key("ack")));
        let items = vec![person("m_0", "x"), agent("m_1", "Je lance."), tool("m_1.t0", "web_search", "{}"), agent("m_2", "ok")];
        assert!(all(&items, false).values().all(|f| !f.contains_key("ack")));
        // a batch: its calls unwrapped
        let items = vec![
            person("m_0", "x"),
            tool("m_0.t0", "tool_call", r#"{"calls": [{"name": "memory"}, {"name": "mcp__bipagents__mail_search", "arguments": {"query": "edf"}}]}"#),
            agent("m_1", "ok"),
        ];
        assert_eq!(field(&all(&items, false), 1, "ack"), Some(&json!({"line": "m_0.t0", "index": 1})));
    }

    /// Only the page's items get fields; the lines are read on the whole thread (the turn began before the page).
    #[test]
    fn a_page_gets_the_fields_of_its_items_only() {
        let items = vec![person("m_0", "Va"), tool("m_0.t0", "web_search", "{}"), agent("m_1", "Voilà.")];
        let l = layout(&items, 1..3, false).fields;
        assert_eq!(l.keys().copied().collect::<std::collections::BTreeSet<_>>(), [1, 2].into());
        assert_eq!(field(&l, 1, "ack"), Some(&json!({"line": "m_0.t0", "index": 0})));
        assert_eq!(field(&l, 2, "turn_end"), Some(&json!(true)));
        assert!(!layout(&items, 2..3, false).fields.contains_key(&1));
    }

    /// The « Étapes » card is a line of its own, as the apps draw it: the cards on each side are two, two folds.
    #[test]
    fn a_steps_card_splits_the_tools_around_it() {
        let items = vec![
            person("m_0", "Prépare le voyage"),
            agent("m_1", "Je m'en occupe."),
            tool("m_1.t0", "web_search", "{}"),
            item("m_1.s", json!({"kind": "steps", "steps": {"done": 1, "items": []}})),
            tool("m_1.t1", "web_search", "{}"),
            agent("m_2", "Voilà le plan."),
        ];
        let l = all(&items, false);
        assert_eq!((field(&l, 2, "work"), field(&l, 3, "work"), field(&l, 4, "work")), (Some(&json!(true)), None, Some(&json!(true))));
    }

    #[test]
    fn kinds_by_name() {
        assert_eq!(kind_of("mcp__bipagents__notebook_note"), "notebookNote");
        assert_eq!(kind_of("mcp_bipagents_files_shared_files_read"), "sharedFiles");
        assert_eq!(kind_of("mcp__stealth__navigate"), "browser");
        assert_eq!(kind_of("todo_write"), "todo");
        assert_eq!(kind_of("gmail_get_message"), "mailRead");
        assert_eq!(kind_of("mcp__gmail__send_email"), "mailDraft");
        assert_eq!(kind_of("mcp__gmail__search_emails"), "mailSearch");
        assert_eq!(kind_of("mcp__bipagents__routine_schedule"), "routine");
        assert_eq!(kind_of("mcp__bipagents__give_file"), "giveFile");
        assert_eq!(kind_of("mcp__bipconn_garmin__get_activities"), "unknown");
        assert_eq!(kind_of(""), "tool");
        assert_eq!(kind_of("fetch_invoices"), "unknown");
        assert!(bookkeeping("tool_search") && bookkeeping("tool_call") && !bookkeeping("web_search"));
        assert_eq!(unwrapped("tool_call", Some(r#"{"calls": [{"arguments": {"name": "Kévin", "page": "Kév"#)), vec!["tool_call"]);
        assert_eq!(unwrapped("tool_call", Some(r#"{"calls": [{"name": "mcp__bipagents__mail_search", "arguments": {"query": "fac"#)),
                   vec!["mcp__bipagents__mail_search"]);
    }

    #[test]
    fn an_exchange_with_another_agent_folds_with_the_turn_answering_it() {
        let ask = item("m_1", json!({"kind": "message", "role": "peer", "text": "Tu peux voir ?", "from": "bip/lea", "from_name": "Léa"}));
        let items = vec![
            person("m_0", "Salut"),
            agent("m_0b", "Salut !"),
            ask,
            tool("m_1.t0", "web_search", "{}"),
            agent("m_2", "Oui, c'est fait."),
            item("o_1", json!({"kind": "boite", "text": "Rapport"})),
            tool("m_3.t0", "message_agent", "{}"),
            item("m_4", json!({"kind": "message", "role": "peer", "text": "Voilà", "from": "bip/lea", "from_name": "Léa"})),
        ];
        // the second one answers this agent's message_agent to bip/lea (its preview names her)
        let mut items = items;
        items[6].body["tool"]["result"] = json!("Sent to bip/lea");
        let l = all(&items, false);
        assert_eq!((field(&l, 2, "exchange"), field(&l, 2, "request")), (Some(&json!("m_1")), Some(&json!(true))));
        assert_eq!((field(&l, 3, "exchange"), field(&l, 4, "exchange")), (Some(&json!("m_1")), Some(&json!("m_1"))));
        assert_eq!(field(&l, 4, "answers"), Some(&json!("Léa")));
        assert_eq!(field(&l, 5, "exchange"), None, "a scheduled report ends it");
        assert_eq!((field(&l, 7, "exchange"), field(&l, 7, "request")), (Some(&json!("m_4")), None));
        assert_eq!(field(&l, 1, "exchange"), None);
    }
}
