//! How a conversation's turns lay out, decided once for every app (thin apps, brick 7): which lines are the agent's
//! work (folded into « Détail du travail »), which texts answer the turn (its time and « Écouter », the words a spoken
//! reply reads), where the app puts its own first word for a turn the agent started working in without one, and how
//! an exchange with another agent folds. Until 7/11 each app worked this out from the thread itself (iOS
//! `TurnLayout`, `TurnAcknowledgement`, `RowLayout`, `ConversationView.exchangeOwners`; Android `ThreadLayout`), with
//! small differences (Android folded answered cards into an exchange; it ended a work fold's « acknowledgement »
//! window at a message added to the turn, the iPhone did not).
//!
//! Each item of a turn that is over carries `layout` (docs/conversations.md §6.3), keyed by the screen's lines: an
//! item draws one line, a message with reasoning two (`<id>.r` above `<id>`), consecutive tools one card (keyed by its
//! first tool's id):
//!
//! - `fold`: the line opening its work fold (a tool's card, the agent's short commentary between tools);
//!   `reasoning_fold`: the same for the message's reasoning line;
//! - `turn_end`: this text is its turn's reply (the last real text before the next message starting a turn);
//! - `answer`: this text is part of what its turn answers (read aloud after a voice note): never the agent's
//!   acknowledgement nor its commentary between tools;
//! - `ack: {line, index}`: the app's own first word goes before this item's first line; `line` is the tool card whose
//!   call number `index` (among its calls, a `tool_call` batch unwrapped) is the turn's first real call: the app
//!   words it from that call (« Je lance le navigateur sur kiabi.com… »);
//! - `exchange`: the peer message opening the exchange this line folds into; `request: true` on a peer message asking
//!   this agent something (not answering a `message_agent` it sent); `answers: <name>` on the first reply to a peer's
//!   request.
//!
//! The turn still going (a run in the conversation) carries none: what it is decides as it ends, and a page holding it
//! is read again then. The apps keep their own rule for the lines they stream live, and for an older bridge.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use super::timeline::Item;

/// The longest text that still counts as commentary between tools: an answer written before the agent goes on
/// working (updating its notes…) stays in sight.
pub const COMMENTARY_LENGTH: usize = 200;
/// How far back a peer's message looks for this agent's `message_agent` to that peer (an answer, not a request).
const ASKED_WITHIN: usize = 40;

#[derive(Debug, Clone, PartialEq)]
enum Kind {
    /// A message from the person (or a peer) that starts a turn.
    Prompt,
    /// A message from the person added to the answer in progress.
    Added,
    /// The agent's words: their length and whether they carry a file.
    Reply { text: String, length: usize, media: bool },
    /// Calls: (name, arguments, preview).
    Tools(Vec<(String, Option<String>, Option<String>)>),
    Reasoning,
    /// A Boîte message placed at its time (a scheduled report): ends an exchange.
    Scheduled,
    /// An answered card (stays in sight in an exchange).
    Card,
    Other,
}

#[derive(Debug, Clone)]
struct Line {
    /// Its key on screen: the item's id, `<id>.r` for a reasoning line, the first tool's id for a card.
    key: String,
    /// The items it draws (a card: its tools).
    items: Vec<usize>,
    kind: Kind,
    /// A peer's message: its handle and name.
    peer: Option<(String, String)>,
}

fn s<'a>(item: &'a Item, key: &str) -> &'a str {
    item.body.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The screen's lines, as the apps draw the items (iOS `chatItems(from:)`, Android `TimelineItems.items`).
fn lines(items: &[Item]) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for (n, item) in items.iter().enumerate() {
        let files = item.body.get("attachments").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        let line = |key: String, kind: Kind| Line { key, items: vec![n], kind, peer: None };
        match item.kind() {
            "message" => match s(item, "role") {
                "agent" | "assistant" => {
                    if !s(item, "reasoning").trim().is_empty() {
                        out.push(line(format!("{}.r", item.id), Kind::Reasoning));
                    }
                    let text = item.text().to_string();
                    if !text.is_empty() || files {
                        let length = text.trim().chars().count();
                        out.push(line(item.id.clone(), Kind::Reply { text, length, media: files }));
                    }
                }
                role => {
                    if item.text().is_empty() && !files {
                        continue;
                    }
                    let added = item.body.get("added_during_run").and_then(Value::as_bool) == Some(true);
                    let mut l = line(item.id.clone(), if added { Kind::Added } else { Kind::Prompt });
                    if role == "peer" {
                        let handle = item.body.get("from").and_then(Value::as_str).unwrap_or("agent").to_string();
                        let name = item.body.get("from_name").and_then(Value::as_str).unwrap_or(&handle).to_string();
                        l.peer = Some((handle, name));
                    }
                    out.push(l);
                }
            },
            "tool" => {
                let Some(tool) = item.body.get("tool") else { continue };
                let field = |k: &str| tool.get(k).and_then(Value::as_str).map(str::to_string);
                let call = (field("name").unwrap_or_default(), field("arguments"), field("result").or_else(|| field("arguments")));
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
fn turns(lines: &[Line]) -> Vec<std::ops::Range<usize>> {
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

/// The agent's short commentary between tools: a sentence or two followed in its turn by a tool, then by another real
/// text (or a tool still running). A turn's last real text — its answer — never is.
fn commentary(text: &str, length: usize, media: bool, later: &[Line], running: bool) -> bool {
    if media || length > COMMENTARY_LENGTH || interruption_line(text) {
        return false;
    }
    let tool = later.iter().any(|l| matches!(l.kind, Kind::Tools(_)));
    let reply = later.iter().any(|l| matches!(&l.kind, Kind::Reply { text, .. } if !interruption_line(text)));
    tool && (running || reply)
}

/// Line → the first line of its work fold (iOS `TurnLayout.workFolds`). Work = tools, reasoning, and the agent's short
/// commentary between tools; the words before the turn's first tool (its acknowledgement) never are.
fn work_folds(lines: &[Line], running: bool) -> HashMap<usize, usize> {
    let mut folds = HashMap::new();
    let all = turns(lines);
    let last = all.len();
    for (t, turn) in all.into_iter().enumerate() {
        let turn_running = running && t + 1 == last;
        let mut current: Option<usize> = None;
        let mut tools_yet = false;
        for i in turn.clone() {
            let work = match &lines[i].kind {
                Kind::Tools(_) => {
                    tools_yet = true;
                    true
                }
                Kind::Reasoning => true,
                Kind::Reply { text, length, media } => {
                    tools_yet && commentary(text, *length, *media, &lines[i + 1..turn.end], turn_running)
                }
                _ => false,
            };
            if work {
                let start = *current.get_or_insert(i);
                folds.insert(i, start);
            } else {
                current = None;
            }
        }
    }
    folds
}

/// A turn's answer (iOS `TurnLayout.answer`): its last real text, with the texts right before it when no call came
/// between (a file sent, then its words).
fn answer(lines: &[Line], turn: std::ops::Range<usize>) -> Vec<usize> {
    let mut picked = Vec::new();
    for i in turn.rev() {
        match &lines[i].kind {
            Kind::Reply { text, length, media } if !interruption_line(text) && (*length > 0 || *media) => picked.push(i),
            Kind::Tools(_) if !picked.is_empty() => break,
            _ => {}
        }
    }
    picked
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
            Kind::Reply { text, .. } if !interruption_line(text) => decided = Some(false),
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
        return "mail";
    }
    "unknown"
}

fn bookkeeping(tool: &str) -> bool {
    crate::tool_kinds::BOOKKEEPING.contains(&kind_of(tool))
}

/// The app's own first word (iOS `TurnAcknowledgement.placement`): for a turn whose first real call (not bookkeeping)
/// came before any word of the agent, the line it goes before (the first of the work leading to that call), the card
/// and the call's number in it.
fn acknowledgement(lines: &[Line], turn: std::ops::Range<usize>) -> Option<(usize, usize, usize)> {
    if lines[turn.start].kind != Kind::Prompt {
        return None;
    }
    for i in turn.start + 1..turn.end {
        match &lines[i].kind {
            Kind::Reply { text, length, .. } if *length > 0 && !interruption_line(text) => return None,
            Kind::Tools(calls) => {
                let names: Vec<String> = calls.iter().flat_map(|(n, a, _)| unwrapped(n, a.as_deref())).collect();
                let Some(first) = names.iter().position(|n| !bookkeeping(n)) else { continue };
                let mut before = i;
                while before - 1 > turn.start
                    && matches!(lines[before - 1].kind, Kind::Tools(_) | Kind::Reasoning)
                {
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
            .any(|(n, _, p)| n.contains("message_agent") && p.as_deref().unwrap_or("").to_lowercase().contains(&handle)),
        _ => false,
    })
}

/// `key: value` on the items a line draws (those of a turn that is over).
fn put(out: &mut HashMap<usize, Map<String, Value>>, line: &Line, key: &str, value: Value) {
    for n in &line.items {
        if let Some(fields) = out.get_mut(n) {
            fields.insert(key.to_string(), value.clone());
        }
    }
}

/// Each item's `layout` (the module's doc), for the items of the turns that are over: `running`, a run goes on in the
/// conversation (its last turn is not decided yet).
pub fn layout(items: &[Item], running: bool) -> HashMap<usize, Map<String, Value>> {
    let lines = lines(items);
    let all = turns(&lines);
    // the items of the turn going on: none; a turn starts at its message's item
    let open_from = match (running, all.last()) {
        (true, Some(last)) => lines[last.start].items[0],
        (true, None) => 0,
        _ => items.len(),
    };
    let mut out: HashMap<usize, Map<String, Value>> = (0..open_from).map(|n| (n, Map::new())).collect();
    for (i, start) in work_folds(&lines, running) {
        let key = if lines[i].kind == Kind::Reasoning { "reasoning_fold" } else { "fold" };
        put(&mut out, &lines[i], key, json!(lines[start].key));
    }
    for (i, end) in turn_ends(&lines).into_iter().enumerate() {
        if end && matches!(lines[i].kind, Kind::Reply { .. }) {
            put(&mut out, &lines[i], "turn_end", json!(true));
        }
    }
    for turn in &all {
        for i in answer(&lines, turn.clone()) {
            put(&mut out, &lines[i], "answer", json!(true));
        }
        if let Some((before, card, index)) = acknowledgement(&lines, turn.clone()) {
            // on the item whose first line it goes before (a message's reasoning, a card's first tool)
            let n = lines[before].items[0];
            if let Some(fields) = out.get_mut(&n) {
                fields.insert("ack".into(), json!({"line": lines[card].key, "index": index}));
            }
        }
    }
    // exchanges with other agents (Bot Mode): a peer's request folds with the turn answering it, an answer to this
    // agent's request alone; a scheduled report ends it; an answered card stays in sight
    let mut current: Option<String> = None;
    let mut asker: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        match (&line.kind, &line.peer) {
            (Kind::Prompt | Kind::Added, peer) => {
                current = None;
                asker = None;
                if let Some((handle, name)) = peer {
                    put(&mut out, line, "exchange", json!(line.key));
                    if asks(&lines, i, handle) {
                        put(&mut out, line, "request", json!(true));
                        current = Some(line.key.clone());
                        asker = Some(name.clone());
                    }
                }
            }
            (Kind::Scheduled, _) => current = None,
            (Kind::Card, _) => {}
            (kind, _) => {
                if let Some(owner) = &current {
                    put(&mut out, line, "exchange", json!(owner));
                }
                if matches!(kind, Kind::Reply { .. }) && asker.is_some() {
                    put(&mut out, line, "answers", json!(asker.take()));
                }
            }
        }
    }
    out
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
        let l = layout(&items, false);
        let fold = |n| field(&l, n, "fold").and_then(Value::as_str).map(str::to_string);
        assert_eq!(fold(1), None, "the acknowledgement is in sight");
        assert_eq!((fold(2), fold(3)), (Some("m_1.t0".into()), Some("m_1.t0".into())));
        assert_eq!((fold(4), fold(5)), (None, Some("m_3.t0".into())));
        assert_eq!((fold(6), fold(7), fold(8), fold(9)), (None, None, None, None));
        assert_eq!(fold(10), Some("m_7.t0".into()));
        // the turn's reply: its last text; its answer: the texts after the last call
        assert_eq!(field(&l, 7, "turn_end"), Some(&json!(true)));
        assert_eq!(field(&l, 6, "turn_end"), None);
        assert_eq!((field(&l, 6, "answer"), field(&l, 7, "answer")), (Some(&json!(true)), Some(&json!(true))));
        assert_eq!(field(&l, 9, "answer"), Some(&json!(true)), "bookkeeping after it: still the answer");
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
        let l = layout(&items, false);
        assert_eq!(field(&l, 3, "fold"), None);
        assert_eq!(field(&l, 3, "turn_end"), Some(&json!(true)));
        assert_eq!(field(&l, 5, "answer"), None);
        assert_eq!(field(&l, 3, "answer"), Some(&json!(true)));
    }

    #[test]
    fn the_turn_going_on_is_not_decided() {
        let items = vec![person("m_0", "A"), agent("m_1", "B"), person("m_2", "C"), tool("m_2.t0", "web_search", "{}")];
        let l = layout(&items, true);
        assert_eq!(l.len(), 2, "only the first turn's items: {l:?}");
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
        let l = layout(&items, false);
        // before the reasoning (the work leading to the call), the call 0 of the card m_1.t0
        assert_eq!(field(&l, 1, "ack"), Some(&json!({"line": "m_1.t0", "index": 0})));
        assert_eq!(field(&l, 1, "reasoning_fold"), Some(&json!("m_1.r")));
        assert_eq!((field(&l, 2, "fold"), field(&l, 3, "fold")), (Some(&json!("m_1.r")), Some(&json!("m_1.r"))));
        // bookkeeping first, the search in the same card: the search, before the whole card
        let items = vec![
            person("m_0", "Vestes"),
            tool("m_0.t0", "tool_search", r#"{"queries": ["web"]}"#),
            tool("m_0.t1", "todo", ""),
            tool("m_0.t2", "web_search", r#"{"query": "vestes enfant"}"#),
            agent("m_1", "Voilà."),
        ];
        assert_eq!(field(&layout(&items, false), 1, "ack"), Some(&json!({"line": "m_0.t0", "index": 2})));
        // only bookkeeping, or the agent spoke first: none
        let items = vec![person("m_0", "x"), tool("m_0.t0", "memory", ""), tool("m_0.t1", "skills_list", ""), agent("m_1", "ok")];
        assert!(layout(&items, false).values().all(|f| !f.contains_key("ack")));
        let items = vec![person("m_0", "x"), agent("m_1", "Je lance."), tool("m_1.t0", "web_search", "{}"), agent("m_2", "ok")];
        assert!(layout(&items, false).values().all(|f| !f.contains_key("ack")));
        // a batch: its calls unwrapped
        let items = vec![
            person("m_0", "x"),
            tool("m_0.t0", "tool_call", r#"{"calls": [{"name": "memory"}, {"name": "mcp__bipagents__mail_search", "arguments": {"query": "edf"}}]}"#),
            agent("m_1", "ok"),
        ];
        assert_eq!(field(&layout(&items, false), 1, "ack"), Some(&json!({"line": "m_0.t0", "index": 1})));
    }

    #[test]
    fn kinds_by_name() {
        assert_eq!(kind_of("mcp__bipagents__notebook_note"), "notebookNote");
        assert_eq!(kind_of("mcp_bipagents_files_shared_files_read"), "sharedFiles");
        assert_eq!(kind_of("mcp__stealth__navigate"), "browser");
        assert_eq!(kind_of("todo_write"), "todo");
        assert_eq!(kind_of("gmail_get_message"), "mail");
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
        let l = layout(&items, false);
        assert_eq!((field(&l, 2, "exchange"), field(&l, 2, "request")), (Some(&json!("m_1")), Some(&json!(true))));
        assert_eq!((field(&l, 3, "exchange"), field(&l, 4, "exchange")), (Some(&json!("m_1")), Some(&json!("m_1"))));
        assert_eq!(field(&l, 4, "answers"), Some(&json!("Léa")));
        assert_eq!(field(&l, 5, "exchange"), None, "a scheduled report ends it");
        assert_eq!((field(&l, 7, "exchange"), field(&l, 7, "request")), (Some(&json!("m_4")), None));
        assert_eq!(field(&l, 1, "exchange"), None);
    }
}
