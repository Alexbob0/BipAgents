//! The Carnet's tools for the agents (the bridge's MCP server, agent_tools.rs), on by default for every agent. Carnet
//! v2 (internal/notes/carnet-v2/contract.md §2, texts exact: they go to the model with every message, measured by
//! tests/token_anatomy.rs against scripts/token-lab/carnet-baseline.json):
//!
//!     notebook_search   direct answers first (« Pointure · Léa Martin: 36 »), then 5 pages × 3 lines; without words,
//!                       the pages per kind and the 10 latest
//!     notebook_read     one page: its « En bref » and a section, or the whole page (8 000 characters); 6 different
//!                       pages a turn (reads.rs)
//!     notebook_write    facts on a page found by name, nickname, contact, e-mail or phone (made if missing; several
//!                       match: refused, the agent asks), a label said again replaces the old fact, `fix` corrects;
//!                       `forget` only shows the person what would be erased, in a card they confirm in the app
//!                       (forget.rs); `ask` in the schema already (B3)
//!     notebook_task     list, add (with a date: a phone reminder too; `important`), tick off, propose, decline
//!     notebook_record   precise records: purchases, subscriptions, contracts, papers, bookings; short list, `get`
//!     notebook_fill     (fill.rs) « Remplir mon Carnet » in the background
//!
//! No approval asked: filing is the point, and every change can be undone from the page's history. An agent the
//! person gave their health to also reads and writes the private health part (`private: true`); no other agent sees
//! it, and a health fact written by anyone never lands in the shared part (lexicon::is_health). The answers never ask
//! the agent to say what it filed: the line under its answer shows it (reads.rs `lines`).

use serde_json::{json, Value};

use super::reads::{self, Turn};
use super::{lexicon, Part, Who};
use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::Dict;

pub const SEARCH: &str = "notebook_search";
pub const READ: &str = "notebook_read";
pub const WRITE: &str = "notebook_write";
pub const TASK: &str = "notebook_task";
pub const RECORD: &str = "notebook_record";
pub const TOOL_NAMES: [&str; 5] = [SEARCH, READ, WRITE, TASK, RECORD];

/// A whole page read: this many characters at most.
pub const READ_CHARS: usize = 8000;
/// Direct answers and pages a search gives at most, and lines a page.
pub const ANSWERS: usize = 5;
pub const PAGES: usize = 5;
pub const LINES: usize = 3;
/// Pages a search without words lists (the latest).
pub const LATEST: usize = 10;

const ID_NUMBER: &str = "Not saved: ID numbers are never kept.";
const SEVERAL: &str = "Ask which one.";

fn private_property() -> Value {
    json!({"type": "boolean", "description": "health fact (body, sport, sleep, care): their private health part, never elsewhere"})
}

/// The section ids by kind, as `notebook_write` describes them.
fn sections_described() -> String {
    super::KINDS
        .iter()
        .map(|kind| {
            let ids: Vec<&str> = super::template::ids(kind).into_iter().filter(|id| *id != "other").collect();
            format!("{kind}: {}", ids.join(" "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The tools' definitions (`notebook_fill` apart: fill.rs); `health`: the agent may use the private health part.
pub fn tools(health: bool) -> Vec<Value> {
    let kinds = json!(super::KINDS);
    let mut list = vec![
        json!({
            "name": SEARCH,
            "description": "Search the person's Carnet (the notebook all their agents share). Gives direct answers first \
(label, answer, page), then up to 5 pages with their matching lines. Without words: pages per kind and the 10 latest.",
            "inputSchema": {"type": "object", "properties": {
                "words": {"type": "string", "description": "names or a few words"},
                "kind": {"type": "string", "enum": kinds}}},
        }),
        json!({
            "name": READ,
            "description": "Read one page: its « En bref » and the section asked, or the whole page. Only pages that help \
answer: 6 a turn at most.",
            "inputSchema": {"type": "object", "properties": {
                "page": {"type": "string", "description": "name, nickname or path"},
                "section": {"type": "string", "description": "a section id, as in notebook_write"}},
                "required": ["page"]},
        }),
        json!({
            "name": WRITE,
            "description": "Write on a Carnet page, found by name, nickname, contact id, e-mail or phone (made if missing; \
several match: ask which). One section per call, one short fact per item, « Label : value » when \
short (« Pointure : 36 »): a label already there is replaced, nothing is repeated. Close family and friends get a person \
page; others are one line on their group's area page. fix corrects; forget only shows the person what would be erased \
(they confirm in the app). ask (not clarify): one question for a fact this page lacks, never health or money unless they \
raised it. Never passwords, codes or ID numbers.",
            "inputSchema": {"type": "object", "properties": {
                "page": {"type": "string", "description": "« Léa Martin », « Rénovation cuisine », « Maison »"},
                "kind": {"type": "string", "description": "for a new page; self: « Toi »; trip: one journey, « Kyoto, avr. 2026 »", "enum": kinds},
                "section": {"type": "string", "description": sections_described(), "enum": super::template::all_ids()},
                "facts": {"type": "array", "items": {"type": "string"}, "maxItems": 20},
                "summary": {"type": "string", "description": "new « En bref », 2–3 lines"},
                "fix": {"type": "array", "items": {"type": "object", "properties": {
                    "old": {"type": "string"}, "new": {"type": "string", "description": "empty removes"}},
                    "required": ["old", "new"]}},
                "forget": {"type": "array", "items": {"type": "string"}, "maxItems": 5, "description": "facts to forget; the page's name: all of it"},
                "ask": {"type": "object", "properties": {
                    "question": {"type": "string"},
                    "chips": {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 4}},
                    "required": ["question", "chips"]},
                "aliases": {"type": "array", "items": {"type": "string"}},
                "contact": {"type": "string", "description": "contact id, e-mail or phone"},
                "close": {"type": "boolean", "description": "family or close friend"},
                "start": {"type": "string", "description": "trip, YYYY-MM-DD"},
                "end": {"type": "string", "description": "trip, YYYY-MM-DD"}},
                "required": ["page"]},
        }),
        json!({
            "name": TASK,
            "description": "The person's tasks, few and real (a birthday or someone else's deadline is a fact). add only \
when they ask (origin asked) or commit to a deadline (own); title in the infinitive; due also sets a phone reminder when \
shared; important only when they insisted. An idea of yours: propose first (the answer says if you may), ask in one \
sentence, then add with origin accepted and its proposal id, or decline (never: not this kind again). done and list \
(open tasks, waiting proposals).",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["add", "done", "list", "propose", "decline"]},
                "title": {"type": "string"},
                "due": {"type": "string", "description": "YYYY-MM-DD or YYYY-MM-DDTHH:MM"},
                "page": {"type": "string"},
                "origin": {"type": "string", "enum": super::ORIGINS},
                "important": {"type": "boolean"},
                "kind": {"type": "string", "description": "for propose", "enum": super::PROPOSAL_KINDS},
                "proposal": {"type": "string"},
                "never": {"type": "boolean"},
                "id": {"type": "string"}},
                "required": ["action"]},
        }),
        json!({
            "name": RECORD,
            "description": "Precise records, merged when given twice. Fields by type: purchase date item store price class \
(goods, service, ticket, food, digital, travel, fee, other) warranty_end (only if known); subscription service price \
period renewal cancel; contract what provider number end; paper document holder expires (never its number); booking date \
(YYYY-MM-DD HH:MM) what with reference trip. Dates YYYY-MM-DD; source: the document's reference. list gives short lines, \
get one record. Reminders come by themselves. Never card numbers or codes.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["add", "update", "remove", "list", "get"]},
                "type": {"type": "string", "enum": super::records::TYPES.iter().map(|t| t.id).collect::<Vec<_>>()},
                "id": {"type": "string"},
                "fields": {"type": "object", "additionalProperties": {"type": "string"}}},
                "required": ["action"]},
        }),
    ];
    if health {
        for tool in list.iter_mut() {
            if [WRITE, TASK].contains(&tool["name"].as_str().unwrap_or("")) {
                tool["inputSchema"]["properties"]["private"] = private_property();
            }
        }
    }
    list
}

fn args_list(arguments: &Dict, key: &str) -> Vec<String> {
    match arguments.get(key) {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).filter(|s| !s.trim().is_empty()).collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn s(arguments: &Dict, key: &str) -> String {
    tk::str_or_empty(arguments.get(key)).trim().to_string()
}

fn flag(arguments: &Dict, key: &str) -> bool {
    arguments.get(key).map(crate::py::truthy).unwrap_or(false)
}

/// The change queued for the other agents it concerns (news.rs): « Passed on to Budget. », or nothing.
fn tell(config: &Config, agent: &str, part: Part, path: &str, title: &str, extra: &str, text: &str) -> String {
    let told = super::news::queue(config, agent, part, path, title, extra, text, crate::py::time_time());
    if told.is_empty() {
        String::new()
    } else {
        format!(" Passed on to {}.", super::news::names_text(&told, "en"))
    }
}

fn marker(part: Part) -> &'static str {
    if part == Part::Health {
        " [private health part]"
    } else {
        ""
    }
}

/// The pages an agent's tools may reach: the kinds of pages, never « À ranger » (what waits for the nightly pass).
fn reachable(page: &super::Page) -> bool {
    page.kind() != "inbox"
}

/// One call of a Carnet tool by `agent`, in `turn`. Err: a file that cannot be written.
pub fn call(config: &Config, agent: &str, name: &str, arguments: &Dict, turn: &Turn) -> anyhow::Result<Dict> {
    reads::with_turn(turn, || call_in(config, agent, name, arguments, turn))
}

fn call_in(config: &Config, agent: &str, name: &str, arguments: &Dict, turn: &Turn) -> anyhow::Result<Dict> {
    if !super::enabled(config, agent) {
        return Ok(tool_text(
            "The Carnet is off for you: the person switches « Carnet » on in your page of the app.",
            true,
        ));
    }
    let health = super::health_agent(config, agent);
    if !health && flag(arguments, "private") {
        return Ok(tool_text(
            "Health facts go only to the agent the person chose for their health: don't file them.",
            true,
        ));
    }
    let caught = |err: anyhow::Error| -> anyhow::Result<Dict> {
        match err.downcast_ref::<super::NotebookError>() {
            Some(message) => Ok(tool_text(&message.0, true)),
            None => Err(err),
        }
    };
    let result = match name {
        SEARCH => search(config, agent, arguments, turn, health),
        READ => read(config, agent, arguments, turn, health),
        WRITE => write(config, agent, arguments, turn, health),
        TASK => task(config, agent, arguments, health),
        RECORD => record(config, agent, arguments, turn),
        _ => Ok(tool_text(&format!("unknown tool: {name}"), true)),
    };
    result.or_else(caught)
}

fn readable(health: bool) -> Vec<Part> {
    if health {
        vec![Part::Main, Part::Health]
    } else {
        vec![Part::Main]
    }
}

// -- search ----------------------------------------------------------------------------------------------------------

/// A fact that answers a question (« Léa chausse du combien ? » → Léa Martin's « Pointure : 36 »).
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub part: Part,
    pub path: String,
    pub page: String,
    pub page_id: String,
    /// « Pointure » (empty: a fact with no label).
    pub label: String,
    pub answer: String,
    pub date: String,
}

fn word_in(word: &str, words: &[&str]) -> bool {
    words.iter().any(|w| *w == word || (word.chars().count() >= 3 && w.starts_with(word)))
}

/// The facts that answer `text` in the given parts, the most precise first (the page named, then the latest), 5 at
/// most. A question about a label (« pointure », « chausse du ») only finds facts of that label; names alone answer
/// nothing (they find the page).
pub fn answers(config: &Config, parts: &[Part], text: &str) -> Vec<Answer> {
    let (words, labels) = lexicon::query(text);
    if words.is_empty() && labels.is_empty() {
        return Vec::new();
    }
    let lang = super::language(config);
    let brief = super::fold(super::words(&lang).brief);
    let mut found: Vec<(usize, Answer)> = Vec::new();
    for part in parts {
        for (path, page) in super::all_pages(config, *part) {
            if !reachable(&page) || !super::KINDS.contains(&page.kind().as_str()) {
                continue;
            }
            let names = super::fold(&format!("{} {}", page.title, page.aliases().join(" ")));
            let names: Vec<&str> = names.split(' ').collect();
            let named: Vec<&String> = words.iter().filter(|w| word_in(w, &names)).collect();
            let rest: Vec<&String> = words.iter().filter(|w| !named.contains(w)).collect();
            if labels.is_empty() && rest.is_empty() {
                continue;
            }
            for (section, facts) in page.sections() {
                if super::fold(&section) == brief {
                    continue;
                }
                for fact in facts.iter().filter(|f| f.item) {
                    let split = lexicon::split_label(&fact.text);
                    if !labels.is_empty() {
                        let key = split.as_ref().map(|(l, _)| lexicon::label_key(l)).unwrap_or_default();
                        if !labels.contains(&key.as_str()) {
                            continue;
                        }
                    }
                    let folded = super::fold(&format!("{section} {}", fact.text));
                    let fact_words: Vec<&str> = folded.split(' ').collect();
                    if !rest.iter().all(|w| word_in(w, &fact_words)) {
                        continue;
                    }
                    let (label, answer) = split.unwrap_or((String::new(), fact.text.clone()));
                    found.push((
                        named.len(),
                        Answer {
                            part: *part,
                            path: path.clone(),
                            page: page.title.clone(),
                            page_id: page.id(),
                            label,
                            answer,
                            date: fact.date.clone(),
                        },
                    ));
                }
            }
        }
    }
    // the page the question names first, when it names one
    let best = found.iter().map(|(n, _)| *n).max().unwrap_or(0);
    found.retain(|(n, _)| *n == best);
    found.sort_by_key(|a| std::cmp::Reverse(a.1.date.clone()));
    found.into_iter().map(|(_, a)| a).take(ANSWERS).collect()
}

/// A search answer from a page past the turn's cap: its title only.
const LIMIT: &str = "limit reached (ask the person if needed)";

fn search(config: &Config, agent: &str, arguments: &Dict, turn: &Turn, health: bool) -> anyhow::Result<Dict> {
    let words = s(arguments, "words");
    let kind = s(arguments, "kind");
    let parts = readable(health);
    let w = super::words(&super::language(config));
    if words.is_empty() {
        let mut pages: Vec<(Part, String, super::Page)> = Vec::new();
        for part in &parts {
            for (path, page) in super::all_pages(config, *part) {
                let k = page.kind();
                if super::KINDS.contains(&k.as_str()) && (kind.is_empty() || k == kind) {
                    pages.push((*part, path, page));
                }
            }
        }
        if pages.is_empty() {
            return Ok(tool_text("The Carnet is empty for now.", false));
        }
        let counts: Vec<String> = super::KINDS
            .iter()
            .filter_map(|k| {
                let n = pages.iter().filter(|(_, _, p)| p.kind() == *k).count();
                (n > 0).then(|| format!("{k} {n}"))
            })
            .collect();
        let updated = |p: &super::Page| tk::str_or_empty(p.front.get("updated"));
        pages.sort_by_key(|a| std::cmp::Reverse(updated(&a.2)));
        let mut out = vec![format!("Pages: {}", counts.join(" · "))];
        for (part, path, page) in pages.iter().take(LATEST) {
            out.push(format!("{} ({}, {path}){}", page.title, page.kind(), marker(*part)));
        }
        return Ok(tool_text(&out.join("\n"), false));
    }
    let found = answers(config, &parts, &words);
    // the pages the words name first (their title or a nickname), then those that only hold them
    let (wanted, _) = lexicon::query(&words);
    let names_it = |page: &super::Page| {
        let names = super::fold(&format!("{} {}", page.title, page.aliases().join(" ")));
        let names: Vec<&str> = names.split(' ').collect();
        wanted.iter().any(|w| word_in(w, &names))
    };
    let mut pages: Vec<super::Hit> =
        super::search(config, &parts, &words, &kind).into_iter().filter(|(_, _, page, _)| reachable(page)).collect();
    pages.sort_by_key(|(_, _, page, _)| !names_it(page));
    pages.truncate(PAGES);
    if found.is_empty() && pages.is_empty() {
        return Ok(tool_text("Nothing found in the Carnet.", false));
    }
    let mut out: Vec<String> = Vec::new();
    // every page whose content a search shows counts against the turn's cap and goes to the reads journal; past the
    // cap, its title only
    let shown = |part: Part, id: &str| {
        let ok = reads::may_read(agent, turn, id);
        if ok {
            reads::log(config, agent, turn, part, id, None, "search");
        }
        ok
    };
    if !found.is_empty() {
        out.push("Answers:".into());
        for a in &found {
            let id = if a.page_id.is_empty() { a.path.as_str() } else { a.page_id.as_str() };
            if !shown(a.part, id) {
                out.push(format!("{}: {LIMIT}{}", a.page, marker(a.part)));
                continue;
            }
            let head = if a.label.is_empty() { a.page.clone() } else { format!("{} · {}", a.label, a.page) };
            let date = if a.date.is_empty() { String::new() } else { format!(" ({})", a.date) };
            out.push(format!("{head}: {}{date}{}", a.answer, marker(a.part)));
        }
    }
    if !pages.is_empty() {
        out.push("Pages:".into());
        for (part, path, page, lines) in &pages {
            let past = if w.archived(path) { " [archived]" } else { "" };
            let id = if page.id().is_empty() { path.clone() } else { page.id() };
            let open = shown(*part, &id);
            let limit = if open { "" } else { " — limit reached" };
            out.push(format!("{} ({}, {path}){}{past}{limit}", page.title, page.kind(), marker(*part)));
            if !open {
                continue;
            }
            for line in lines.iter().take(LINES) {
                out.push(format!("  {}", super::split_attribution(line.trim_start_matches("- ")).0));
            }
        }
    }
    Ok(tool_text(&out.join("\n"), false))
}

// -- read ------------------------------------------------------------------------------------------------------------

fn one_page(config: &Config, parts: &[Part], query: &str, kind: &str) -> Result<Option<super::Found>, String> {
    let found: Vec<super::Found> =
        super::candidates(config, parts, query, kind).into_iter().filter(|f| reachable(&f.page)).collect();
    if found.len() > 1 {
        let names: Vec<String> = found.iter().map(|f| f.page.title.clone()).collect();
        return Err(format!("Several pages: {}. {SEVERAL}", names.join(", ")));
    }
    Ok(found.into_iter().next())
}

fn read(config: &Config, agent: &str, arguments: &Dict, turn: &Turn, health: bool) -> anyhow::Result<Dict> {
    let found = match one_page(config, &readable(health), &s(arguments, "page"), "") {
        Err(several) => return Ok(tool_text(&several, true)),
        Ok(None) => {
            return Ok(tool_text("No such page in the Carnet (notebook_search finds pages by name or words).", true))
        }
        Ok(Some(found)) => found,
    };
    let id = if found.page.id().is_empty() { found.path.clone() } else { found.page.id() };
    if !reads::may_read(agent, turn, &id) {
        return Ok(tool_text("Limit reached: ask the person which page to read.", false));
    }
    let lang = super::language(config);
    let w = super::words(&lang);
    let kind = found.page.kind();
    let body = super::in_outline(config, &found.page);
    let mut out = format!("# {}{}\n", found.page.title, marker(found.part));
    let aliases = found.page.aliases();
    if !aliases.is_empty() {
        out.push_str(&format!("Also: {}\n", aliases.join(", ")));
    }
    let wanted = s(arguments, "section");
    let section = if wanted.is_empty() { None } else { super::template::resolve(&kind, &wanted) };
    match section {
        Some(id) => {
            let title = super::template::title(&kind, id, &lang);
            let brief = super::summary_of(&body, w.brief);
            if !brief.is_empty() {
                out.push_str(&format!("## {}\n{brief}\n", w.brief));
            }
            let lines = section_lines(&body, &title);
            out.push_str(&format!("## {title}\n{}\n", if lines.is_empty() { "(empty)".to_string() } else { lines }));
        }
        None => {
            let text = body.trim();
            let cut = tk::head(text, READ_CHARS);
            out.push_str(cut);
            if cut.len() < text.len() {
                out.push_str("\n(cut)");
            }
        }
    }
    reads::log(config, agent, turn, found.part, &id, section, "read");
    Ok(tool_text(out.trim_end(), false))
}

/// Whether section `section` of page `page` (main part) has a fact already.
fn section_filled(config: &Config, page: &str, section: &str) -> bool {
    let Ok(Some(found)) = one_page(config, &[Part::Main], page, "") else { return false };
    let kind = found.page.kind();
    let Some(id) = super::template::resolve(&kind, section) else { return false };
    let title = super::template::title(&kind, id, &super::language(config));
    !section_lines(&super::in_outline(config, &found.page), &title).is_empty()
}

/// The lines of section `title` in `body` (none: empty).
fn section_lines(body: &str, title: &str) -> String {
    let mut inside = false;
    let mut out: Vec<&str> = Vec::new();
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            inside = super::fold(name) == super::fold(title);
            continue;
        }
        if inside && !line.trim().is_empty() {
            out.push(line);
        }
    }
    out.join("\n")
}

// -- write -----------------------------------------------------------------------------------------------------------

fn write(config: &Config, agent: &str, arguments: &Dict, turn: &Turn, health: bool) -> anyhow::Result<Dict> {
    let page = s(arguments, "page");
    if page.is_empty() {
        return Ok(tool_text("give the page", true));
    }
    let facts = args_list(arguments, "facts");
    let summary = s(arguments, "summary");
    let fixes: Vec<(String, String)> = match arguments.get("fix") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|f| f.as_object())
            .map(|f| (s(f, "old"), s(f, "new")))
            .filter(|(old, _)| !old.is_empty())
            .collect(),
        _ => Vec::new(),
    };
    let texts = facts.iter().chain(std::iter::once(&summary)).chain(fixes.iter().map(|(_, new)| new));
    if texts.into_iter().any(|t| lexicon::id_number(t)) {
        return Ok(tool_text(ID_NUMBER, true));
    }
    let w = super::words(&super::language(config));
    let health_page = super::fold(&page) == super::fold(w.health) || super::fold(&page) == "health";
    if health_page && !health {
        return Ok(tool_text(
            "Health facts go only to the agent the person chose for their health: don't file them.",
            true,
        ));
    }
    let part = if (health && flag(arguments, "private")) || (health && health_page) { Part::Health } else { Part::Main };
    let who = Who::agent(config, agent);
    let kind = match s(arguments, "kind") {
        k if k.is_empty() => "person".to_string(),
        k => k,
    };
    let (aliases, contact) = (args_list(arguments, "aliases"), s(arguments, "contact"));
    let (start, end) = (s(arguments, "start"), s(arguments, "end"));
    let close = arguments.get("close").and_then(Value::as_bool);
    let mut out: Vec<String> = Vec::new();
    // the question of the day: only for a section still empty before this call (ask.rs)
    let ask = arguments.get("ask").filter(|a| !a.is_null());
    let filled = ask.is_some() && section_filled(config, &page, &s(arguments, "section"));
    let writes = !facts.is_empty()
        || !summary.is_empty()
        || !aliases.is_empty()
        || !contact.is_empty()
        || close.is_some()
        || !start.is_empty()
        || !end.is_empty()
        || kind == "trip";
    if writes {
        let written = super::note(config, part, &kind, &page, &s(arguments, "section"), &facts, &aliases, &contact, &summary, &who)
            .and_then(|written| {
                if written.path.is_empty() {
                    return Ok(written);
                }
                // « c'est un proche »: their dates are shown (the person's word, kept on the page)
                if let (Some(close), Part::Main) = (close, part) {
                    super::set_close(config, &written.path, close, &who)?;
                }
                // a trip's dates, in its frontmatter (its destination from its name)
                if !(start.is_empty() && end.is_empty())
                    && super::trip::set_details(config, part, &written.path, &start, &end, &who)?
                {
                    return Ok(super::Written { changed: true, ..written });
                }
                Ok(written)
            })?;
        if !written.path.is_empty() && written.part == Some(Part::Main) {
            if let Some(text) = super::read_page_text(config, Part::Main, &written.path) {
                let id = super::Page::parse(&text, "").id();
                reads::wrote(agent, turn, if id.is_empty() { &written.path } else { &id });
            }
        }
        let apart = if written.apart > 0 { " Health facts saved apart: not in the shared part." } else { "" };
        // the person moved such facts elsewhere before (moves.rs): they went there, and the agent is told
        if !written.elsewhere.is_empty() {
            let names: Vec<String> = written.elsewhere.iter().map(|t| format!("« {t} »")).collect();
            out.push(format!("Filed on {} instead: your person moved such facts there.", names.join(", ")));
        }
        if !written.elsewhere.is_empty() && written.added.is_empty() && !written.created {
            // nothing more to say about the page named
        } else if written.path.is_empty() {
            out.push("Saved apart (health): not in the shared part.".into());
        } else if !written.added.is_empty() {
            let n = written.added.len();
            let replaced = if written.replaced > 0 { format!(", {} replaced", written.replaced) } else { String::new() };
            let new = if written.created { ", new page" } else { "" };
            let told =
                tell(config, agent, written.part.unwrap_or(part), &written.path, &written.title, "", &written.added.join(" ; "));
            out.push(format!(
                "Saved: {} › {} ({n} fact{}{replaced}{new}).{apart}{told}",
                written.title,
                written.section_ids.join(", "),
                if n == 1 { "" } else { "s" }
            ));
        } else if written.created || written.changed || !summary.is_empty() || close.is_some() {
            let told = if summary.is_empty() {
                String::new()
            } else {
                tell(config, agent, written.part.unwrap_or(part), &written.path, &written.title, "", &summary)
            };
            out.push(format!("Saved: {}.{apart}{told}", written.title));
        } else {
            out.push(format!("Already there: {}.{apart}", written.title));
        }
    }
    if !fixes.is_empty() {
        let found = match one_page(config, &readable(health), &page, "") {
            Err(several) => return Ok(tool_text(&several, true)),
            Ok(None) => return Ok(tool_text(&format!("No page « {page} » in the Carnet."), true)),
            Ok(Some(found)) => found,
        };
        let mut done = 0;
        let mut changes: Vec<String> = Vec::new();
        for (old, new) in &fixes {
            match super::edit(config, &[found.part], &found.path, old, new, "", None, "", None, &who) {
                Ok((written, change)) => {
                    done += 1;
                    if !change.is_empty() {
                        changes.push(change);
                    }
                    if written.apart > 0 {
                        out.push("Health facts saved apart: not in the shared part.".into());
                    }
                }
                Err(err) if err.downcast_ref::<super::NotebookError>().is_some() => {
                    out.push(format!("Not changed on {}: « {old} » ({err}).", found.page.title));
                }
                Err(err) => return Err(err),
            }
        }
        if done > 0 {
            let told = tell(config, agent, found.part, &found.path, &found.page.title, "", &changes.join(" ; "));
            out.push(format!("Fixed: {} ({done}).{told}", found.page.title));
        }
    }
    if let Some(ask) = ask {
        let answer = match one_page(config, &[Part::Main], &page, "") {
            Ok(Some(found)) => super::ask::from_agent(config, agent, turn, &found, &s(arguments, "section"), ask, filled, crate::py::time_time())?,
            _ => super::ask::NO.to_string(),
        };
        out.push(answer);
    }
    let forget = args_list(arguments, "forget");
    if !forget.is_empty() {
        out.push(super::forget::ask_from_agent(config, agent, &page, &forget, health)?);
    }
    if out.is_empty() {
        return Ok(tool_text("give facts, fix or summary", true));
    }
    Ok(tool_text(&out.join("\n"), false))
}

// -- tasks -----------------------------------------------------------------------------------------------------------

fn task(config: &Config, agent: &str, arguments: &Dict, health: bool) -> anyhow::Result<Dict> {
    let private = health && flag(arguments, "private");
    let parts: Vec<Part> = if private { vec![Part::Health] } else { readable(health) };
    let who = Who::agent(config, agent);
    match s(arguments, "action").as_str() {
        "list" => {
            let mut lines = Vec::new();
            for part in &parts {
                for task in super::open_tasks(config, *part) {
                    let due = if task.due.is_empty() { String::new() } else { format!(" · due {}", task.due) };
                    lines.push(format!("- [{}] {}{due}{}", task.id, task.title, marker(*part)));
                }
            }
            let waiting: Vec<String> = super::open_proposals(config)
                .iter()
                .filter(|p| tk::str_or_empty(p.get("agent")).is_empty())
                .filter(|p| tk::str_or_empty(p.get("kind")) != super::trip_split::KIND)
                .map(|p| {
                    format!(
                        "- proposal [{}] {} · due {}",
                        tk::str_or_empty(p.get("id")),
                        tk::str_or_empty(p.get("title")),
                        tk::str_or_empty(p.get("due"))
                    )
                })
                .collect();
            if !waiting.is_empty() {
                lines.push("Waiting for the person's answer (yes: add with origin accepted and the proposal id):".into());
                lines.extend(waiting);
            }
            if lines.is_empty() {
                return Ok(tool_text("No open task.", false));
            }
            Ok(tool_text(&lines.join("\n"), false))
        }
        "propose" => {
            let id = super::propose(config, agent, &s(arguments, "kind"), &s(arguments, "title"), &s(arguments, "due"))?;
            Ok(tool_text(&format!("You may propose it [proposal {id}]: ask the person in one short sentence."), false))
        }
        "decline" => match super::decline(config, &s(arguments, "proposal"), flag(arguments, "never"), &who)? {
            Some(fact) => Ok(tool_text(&format!("Noted: no more proposals of this kind (« {fact} »)."), false)),
            None => Ok(tool_text("Noted: not added.", false)),
        },
        "add" => {
            let part = if private { Part::Health } else { Part::Main };
            let title = s(arguments, "title");
            if lexicon::id_number(&title) {
                return Ok(tool_text(ID_NUMBER, true));
            }
            let added = super::add_task(
                config,
                part,
                &title,
                &s(arguments, "due"),
                &s(arguments, "page"),
                &s(arguments, "origin"),
                &s(arguments, "proposal"),
                &who,
            )?;
            if flag(arguments, "important") {
                super::set_important(config, &added.task.id, true)?;
            }
            let task = &added.task;
            let due = if task.due.is_empty() { String::new() } else { format!(", due {}", task.due) };
            let phone = if added.reminder { " Phone reminder too." } else { "" };
            if added.existing {
                let date = if added.dated { " It now has this date." } else { "" };
                return Ok(tool_text(&format!("Already there [{}]: {}{due}.{date}{phone}", task.id, task.title), false));
            }
            let tasks = super::words(&super::language(config)).tasks_path();
            let told = tell(config, agent, part, &tasks, "", &s(arguments, "page"), &format!("task: {}{due}", task.title));
            Ok(tool_text(&format!("Added [{}]: {}{due}.{phone}{told}", task.id, task.title), false))
        }
        "done" => match super::set_task_done(config, &parts, &s(arguments, "id"), true, &who, false)? {
            Some(task) => Ok(tool_text(&format!("Done: {}.", task.title), false)),
            None => Ok(tool_text("unknown task id (action list gives them)", true)),
        },
        _ => Ok(tool_text("action is add, done, list, propose or decline", true)),
    }
}

// -- records ---------------------------------------------------------------------------------------------------------

/// The fields a record's short line leaves out (its numbers, references and sources: `get` gives them).
const NOT_LISTED: [&str; 3] = ["number", "reference", "source"];

/// A record's short line (`list`): what it is, where or whose, its price, its next date; never a number.
fn short_line(kind: &super::records::Kind, record: &Dict) -> String {
    let head = super::records::summary_head(kind, record);
    let mut parts = vec![head.clone()];
    for f in kind.fields.iter().filter(|f| !NOT_LISTED.contains(&f.id)) {
        let v = tk::str_or_empty(record.get(f.id));
        let shown = ["holder", "provider", "store", "price"].contains(&f.id) || f.id == kind.next;
        if shown && !v.is_empty() && v != head {
            parts.push(v);
        }
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

/// A record as an add or an update left it: every field but its numbers (the class and warranty the Carnet set).
fn written_line(kind: &super::records::Kind, record: &Dict) -> String {
    kind.fields
        .iter()
        .filter(|f| !NOT_LISTED.contains(&f.id))
        .map(|f| tk::str_or_empty(record.get(f.id)))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A record's whole line (`get`): every field by its id.
fn whole_line(kind: &super::records::Kind, record: &Dict) -> String {
    kind.fields
        .iter()
        .filter_map(|f| {
            let v = tk::str_or_empty(record.get(f.id));
            (!v.is_empty()).then(|| format!("{}: {v}", f.id))
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The record of id `id`, in its type's table (live or archived), with that type.
fn record_by_id(config: &Config, kind: Option<&'static super::records::Kind>, id: &str) -> Option<(&'static super::records::Kind, Dict, bool)> {
    let kinds: Vec<&'static super::records::Kind> = match kind {
        Some(k) => vec![k],
        None => super::records::TYPES.iter().collect(),
    };
    for k in kinds {
        if let Some(r) = super::records::records(config, k).into_iter().find(|r| tk::str_or_empty(r.get("id")) == id) {
            return Some((k, r, false));
        }
        if let Some(r) = super::records::archived_records(config, k).into_iter().find(|r| tk::str_or_empty(r.get("id")) == id) {
            return Some((k, r, true));
        }
    }
    None
}

fn record_page_id(config: &Config, kind: &super::records::Kind, archived: bool) -> String {
    let lang = super::language(config);
    let path =
        if archived { super::records::archive_path_of(&lang, kind) } else { super::records::path_of(&lang, kind) };
    super::read_page_text(config, Part::Main, &path)
        .map(|t| super::Page::parse(&t, "").id())
        .filter(|id| !id.is_empty())
        .unwrap_or(path)
}

fn record(config: &Config, agent: &str, arguments: &Dict, turn: &Turn) -> anyhow::Result<Dict> {
    use super::records::{self, Upserted};
    let who = Who::agent(config, agent);
    let action = s(arguments, "action");
    let kind = records::kind(&s(arguments, "type"));
    let mut fields: Dict = match arguments.get("fields") {
        Some(Value::Object(f)) => f.clone(),
        _ => Dict::new(),
    };
    // a booking's « with » (the tool's word) is its provider
    if let Some(with) = fields.remove("with") {
        fields.entry("provider".to_string()).or_insert(with);
    }
    let types = "type is purchase, subscription, contract, paper or booking";
    match action.as_str() {
        "list" => {
            let kinds: Vec<&records::Kind> = match kind {
                Some(k) => vec![k],
                None => records::TYPES.iter().collect(),
            };
            let mut lines: Vec<String> = Vec::new();
            for k in kinds {
                let live = records::records(config, k);
                let past = records::archived_records(config, k);
                if !live.is_empty() {
                    reads::log(config, agent, turn, Part::Main, &record_page_id(config, k, false), None, "record");
                }
                for (r, archived) in live.iter().map(|r| (r, false)).chain(past.iter().map(|r| (r, true))) {
                    let mark = if archived { " (archived)" } else { "" };
                    lines.push(format!("- {} [{}]{mark} {}", k.id, tk::str_or_empty(r.get("id")), short_line(k, r)));
                }
            }
            Ok(tool_text(if lines.is_empty() { "No record yet.".to_string() } else { lines.join("\n") }.as_str(), false))
        }
        "get" => match record_by_id(config, kind, &s(arguments, "id")) {
            Some((k, r, archived)) => {
                reads::log(config, agent, turn, Part::Main, &record_page_id(config, k, archived), None, "record");
                let mark = if archived { " (archived)" } else { "" };
                Ok(tool_text(&format!("{} [{}]{mark}: {}", k.id, tk::str_or_empty(r.get("id")), whole_line(k, &r)), false))
            }
            None => Ok(tool_text("unknown record id (action list gives them)", true)),
        },
        "add" | "update" => {
            let Some(kind) = kind.or_else(|| record_by_id(config, None, &s(arguments, "id")).map(|(k, _, _)| k)) else {
                return Ok(tool_text(types, true));
            };
            let numbered = fields.iter().any(|(k, v)| {
                let v = tk::str_or_empty(Some(v));
                lexicon::id_number(&v) || (kind.id == "paper" && k == "number" && !v.trim().is_empty())
            });
            if numbered {
                return Ok(tool_text(ID_NUMBER, true));
            }
            if kind.id == "paper" {
                super::papers::clean_fields(config, &mut fields);
            }
            let (record, done) = records::upsert(config, kind, &s(arguments, "id"), &fields, action == "update", &who)?;
            if kind.id == "paper" && done != Upserted::Same {
                super::papers::remind(config, &record, &who, chrono::Local::now().date_naive())?;
            }
            let line = written_line(kind, &record);
            let id = tk::str_or_empty(record.get("id"));
            let (what, told) = match done {
                Upserted::Added | Upserted::Updated => {
                    let path = records::path_of(&super::language(config), kind);
                    let title = path.rsplit('/').next().unwrap_or("").trim_end_matches(".md").to_string();
                    let told = tell(config, agent, Part::Main, &path, &title, "", &records::summary_of(kind, &record));
                    (if done == Upserted::Added { "Added" } else { "Updated" }, told)
                }
                Upserted::Same => ("Already there", String::new()),
            };
            Ok(tool_text(&format!("{what} [{id}]: {} · {line}.{told}", kind.id), false))
        }
        "remove" => {
            let Some(kind) = kind.or_else(|| record_by_id(config, None, &s(arguments, "id")).map(|(k, _, _)| k)) else {
                return Ok(tool_text(types, true));
            };
            match records::remove(config, kind, &s(arguments, "id"), &who)? {
                Some(r) => Ok(tool_text(&format!("Removed: {} · {}.", kind.id, short_line(kind, &r)), false)),
                None => Ok(tool_text("unknown record id (action list gives them)", true)),
            }
        }
        _ => Ok(tool_text("action is add, update, remove, list or get", true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/carnet-tools-v2.json")).unwrap()
    }

    #[test]
    fn the_tools_are_the_contracts_byte_for_byte() {
        let want = exact();
        let mut got: Vec<Value> = tools(false);
        got.push(super::super::fill::tool());
        for tool in &got {
            let name = tool["name"].as_str().unwrap();
            assert_eq!(
                serde_json::to_string(tool).unwrap(),
                serde_json::to_string(&want[name]).unwrap(),
                "{name} differs from tests/fixtures/carnet-tools-v2.json"
            );
        }
        assert_eq!(got.len(), 6);
        let health = tools(true);
        for name in [WRITE, TASK] {
            let tool = health.iter().find(|t| t["name"] == name).unwrap();
            assert_eq!(tool["inputSchema"]["properties"]["private"], want["private"]);
        }
        assert!(health.iter().filter(|t| t["inputSchema"]["properties"].get("private").is_some()).count() == 2);
        assert_eq!(crate::souls::NOTEBOOK_RULE, want["NOTEBOOK_RULE"].as_str().unwrap());
        // the section ids it lists are the outlines'
        let write = got.iter().find(|t| t["name"] == WRITE).unwrap();
        assert_eq!(write["inputSchema"]["properties"]["section"]["enum"], json!(super::super::template::all_ids()));
    }

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        for name in ["quotidien", "sante"] {
            let agent = crate::config::AgentConfig { name: name.into(), display_name: name.into(), ..Default::default() };
            config.agents.insert(name.into(), agent);
        }
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    fn run(config: &Config, agent: &str, name: &str, arguments: Value, turn: &Turn) -> (String, bool) {
        let Value::Object(arguments) = arguments else { panic!("an object") };
        let result = call(config, agent, name, &arguments, turn).unwrap();
        (result["content"][0]["text"].as_str().unwrap().to_string(), result["isError"] == json!(true))
    }

    fn turn(run_id: &str) -> Turn {
        Turn { run: Some(run_id.into()), session: Some(format!("s-{run_id}")), cap: reads::PAGES_PER_TURN, busy: false }
    }

    fn pages(config: &Config) -> Vec<String> {
        super::super::all_pages(config, Part::Main).into_iter().map(|(p, _)| p).collect()
    }

    fn page_text(config: &Config, path: &str) -> String {
        super::super::read_page_text(config, Part::Main, path).unwrap_or_default()
    }

    #[test]
    fn a_shared_landline_or_family_email_never_joins_two_people() {
        let (_dir, config) = setup();
        let t = turn("run_family");
        let w = |args: Value| run(&config, "quotidien", WRITE, args, &t);
        w(json!({"page": "Marc Dupont", "kind": "person", "facts": ["Voisin"], "contact": "01 23 45 67 89"}));
        // a new person with the family's landline: not written on Marc's page, the agent asks
        let (said, error) =
            w(json!({"page": "Léa Dupont", "kind": "person", "facts": ["Aime le jazz"], "contact": "01 23 45 67 89"}));
        assert!(error && said.contains("Same contact as « Marc Dupont »"), "{said}");
        assert!(!page_text(&config, "Personnes/Marc Dupont.md").contains("jazz"));
        assert_eq!(pages(&config), vec!["Personnes/Marc Dupont.md".to_string()]);
        // both pages there: the shared landline or the family e-mail never merges them
        w(json!({"page": "Léa Dupont", "kind": "person", "facts": ["Aime le jazz"]}));
        w(json!({"page": "Marc Dupont", "kind": "person", "facts": ["Joue au foot"], "contact": "famille.dupont@example.org"}));
        for contact in ["+33 1 23 45 67 89", "Famille.Dupont@example.org"] {
            let (said, error) = w(json!({"page": "Léa Dupont", "kind": "person", "facts": ["Aime le thé"], "contact": contact}));
            assert!(error && said.contains("Same contact as « Marc Dupont »"), "{contact}: {said}");
            assert_eq!(pages(&config).len(), 2, "{:?}", pages(&config));
        }
        assert!(!page_text(&config, "Personnes/Marc Dupont.md").contains("Léa"));
        // a nickname of Marc with that landline: Marc's page
        let (said, _) = w(json!({"page": "Marco", "kind": "person", "facts": ["Aime le vélo"], "contact": "01 23 45 67 89"}));
        assert!(said.starts_with("Saved: Marc Dupont"), "{said}");
    }

    #[test]
    fn a_summary_or_a_fix_goes_through_the_health_and_forget_filters() {
        let (_dir, config) = setup();
        let t = turn("run_filters");
        let w = |args: Value| run(&config, "quotidien", WRITE, args, &t);
        let lea = "Personnes/Léa Martin.md";
        w(json!({"page": "Léa Martin", "kind": "person", "facts": ["Aime le dessin", "Aime le jazz"]}));
        let (said, _) = w(json!({"page": "Léa Martin", "summary": "Diabétique, sous insuline"}));
        assert!(said.contains("apart"), "{said}");
        let (said, _) = w(json!({"page": "Léa Martin", "fix": [{"old": "Aime le dessin", "new": "Allergique à la pénicilline"}]}));
        assert!(said.contains("apart"), "{said}");
        let text = page_text(&config, lea);
        assert!(!text.contains("insuline") && !text.contains("pénicilline"), "{text}");
        let (found, _) = run(&config, "quotidien", SEARCH, json!({"words": "insuline pénicilline"}), &turn("run_look"));
        assert!(!found.contains("insuline") && !found.contains("pénicilline"), "{found}");
        // forgotten: an old turn's summary or fix does not write it back
        let request = super::super::forget::Request { texts: vec!["Pointure : 36".into()], page: Some("Léa Martin".into()), private: false };
        w(json!({"page": "Léa Martin", "section": "tastes", "facts": ["Pointure : 36"]}));
        let shown = super::super::forget::preview(&config, &request, "fr", "Alex", None).unwrap();
        super::super::forget::confirm(&config, shown["token"].as_str().unwrap(), "Alex").unwrap().unwrap();
        w(json!({"page": "Léa Martin", "summary": "Pointure : 36"}));
        let (said, _) = w(json!({"page": "Léa Martin", "fix": [{"old": "Aime le jazz", "new": "Pointure : 36"}]}));
        assert!(said.contains("not written back"), "{said}");
        let text = page_text(&config, lea);
        assert!(!text.contains("36") && text.contains("Aime le jazz"), "{text}");
    }

    #[test]
    fn searches_count_against_the_turns_cap_and_go_to_the_journal() {
        let (_dir, config) = setup();
        let t = turn("run_fill");
        for n in 0..10 {
            let page = format!("Ami {n} Martin");
            let fact = if n < 5 { "Aime le jazz" } else { "Joue du piano" };
            run(&config, "quotidien", WRITE, json!({"page": page, "kind": "person", "facts": [fact]}), &t);
        }
        let look = turn("run_two_searches");
        let (first, _) = run(&config, "quotidien", SEARCH, json!({"words": "jazz"}), &look);
        let (second, _) = run(&config, "quotidien", SEARCH, json!({"words": "piano"}), &look);
        // the pages whose lines were shown (an indented line under them, or a direct answer)
        let mut opened: std::collections::HashSet<String> = std::collections::HashSet::new();
        for out in [&first, &second] {
            let lines: Vec<&str> = out.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let Some(name) = line.split(" (").next().filter(|n| n.starts_with("Ami ")) else { continue };
                let shown = lines.get(i + 1).is_some_and(|next| next.starts_with("  "));
                if shown {
                    opened.insert(name.to_string());
                }
            }
            for line in lines.iter().filter(|l| l.contains(" · Ami ") || (l.starts_with("Ami ") && l.contains(": ") && !l.contains("limit"))) {
                if let Some(name) = line.split(" · ").nth(1).or(Some(line)).and_then(|l| l.split(':').next()) {
                    opened.insert(name.trim().to_string());
                }
            }
        }
        assert!(opened.len() <= reads::PAGES_PER_TURN, "{} pages shown: {opened:?}\n{first}\n{second}", opened.len());
        assert!(first.contains("limit reached") || second.contains("limit reached"), "{first}\n{second}");
        let logged: std::collections::HashSet<String> = reads::entries(&config)
            .iter()
            .filter(|e| e.get("run").and_then(Value::as_str) == Some("run_two_searches"))
            .map(|e| tk::str_or_empty(e.get("page_id")))
            .collect();
        assert!(logged.len() <= reads::PAGES_PER_TURN && logged.len() >= opened.len(), "{logged:?} vs {opened:?}");
        assert!(reads::entries(&config).iter().any(|e| e.get("how").and_then(Value::as_str) == Some("search")));
    }

    #[test]
    fn writes_merge_instead_of_duplicating() {
        let (_dir, config) = setup();
        let t = turn("run_merge");
        let w = |args: Value| run(&config, "quotidien", WRITE, args, &t);
        let (said, _) = w(json!({"page": "Léa Martin", "kind": "person", "section": "tastes", "facts": ["Pointure : 35"],
                                 "contact": "Lea@Example.org"}));
        assert_eq!(said, "Saved: Léa Martin › tastes (1 fact, new page).");
        // the same e-mail under another name: the same page, the name kept as a nickname
        let (said, _) = w(json!({"page": "Léa M.", "contact": "lea@example.org", "facts": ["Aime le jazz"]}));
        assert!(said.starts_with("Saved: Léa Martin › "), "{said}");
        // a first name alone, one person has it: there, « Léa » becomes a nickname
        let (said, _) = w(json!({"page": "Léa", "section": "tastes", "facts": ["Chausse du 36"]}));
        assert_eq!(said, "Saved: Léa Martin › tastes (1 fact, 1 replaced).");
        let page = super::super::find(&config, &[Part::Main], "Personnes/Léa Martin.md").unwrap().page;
        assert!(page.aliases().contains(&"Léa".to_string()) && page.aliases().contains(&"Léa M.".to_string()));
        assert_eq!(page.emails(), ["lea@example.org"]);
        let text = super::super::read_page_text(&config, Part::Main, "Personnes/Léa Martin.md").unwrap();
        assert!(text.contains("- Pointure : 36") && !text.contains("Pointure : 35"), "{text}");
        // the old value stays in the page's history
        let history = super::super::history(&config, Some(Part::Main), Some("Personnes/Léa Martin.md"), 10);
        let before = super::super::version_text(&config, history[0]["before"].as_str().unwrap()).unwrap();
        assert!(before.contains("Pointure : 35"));
        // said again exactly: nothing repeated
        let (said, _) = w(json!({"page": "Léa", "section": "tastes", "facts": ["Pointure : 36"]}));
        assert_eq!(said, "Already there: Léa Martin.");
        // two Léa: nothing written, the agent asks
        w(json!({"page": "Léa Petit", "kind": "person", "facts": ["Collègue de Marc"]}));
        let before = pages(&config);
        let (said, error) = w(json!({"page": "Léa", "facts": ["Aime le thé"]}));
        assert!(error && said == "Several pages: Léa Martin, Léa Petit. Ask which one.", "{said}");
        assert_eq!(pages(&config), before);
        // the phone of one page given on another: merged at the write, never « Léa Petit 2 »
        w(json!({"page": "Paul Durand", "kind": "person", "facts": ["Voisin"], "contact": "06 12 34 56 78"}));
        let (said, _) =
            w(json!({"page": "Paulo", "kind": "person", "facts": ["Joue du piano"], "contact": "+33 6 12 34 56 78"}));
        assert!(said.starts_with("Saved: Paul Durand"), "{said}");
        w(json!({"page": "Paul Durand", "kind": "person", "facts": ["Joue au tennis"]}));
        assert!(!pages(&config).iter().any(|p| p.ends_with(" 2.md")), "{:?}", pages(&config));
    }

    #[test]
    fn a_health_fact_never_lands_in_the_shared_part() {
        let (_dir, config) = setup();
        let t = turn("run_health");
        // no health agent: « À ranger », which no agent reads
        let (said, _) = run(&config, "quotidien", WRITE, json!({"page": "Zoé Roux", "kind": "person",
            "facts": ["Allergique à la pénicilline", "Aime le dessin"]}), &t);
        assert!(said.contains("Health facts saved apart"), "{said}");
        let zoe = super::super::read_page_text(&config, Part::Main, "Personnes/Zoé Roux.md").unwrap();
        assert!(zoe.contains("Aime le dessin") && !zoe.contains("pénicilline"), "{zoe}");
        let (_, inbox) = super::super::inbox(&config);
        assert!(inbox.iter().any(|l| l.contains("pénicilline")));
        let (found, _) = run(&config, "quotidien", SEARCH, json!({"words": "pénicilline"}), &t);
        assert_eq!(found, "Nothing found in the Carnet.");
        // a health agent: its private part
        crate::health::set_enabled(&config, "sante", true).unwrap();
        let (said, _) = run(&config, "quotidien", WRITE, json!({"page": "Zoé Roux", "facts": ["Asthme léger"]}), &t);
        assert_eq!(said, "Saved apart (health): not in the shared part.");
        let private = super::super::read_page_text(&config, Part::Health, "Personnes/Zoé Roux.md").unwrap();
        assert!(private.contains("Asthme léger"));
        // ID numbers: never kept
        let (said, error) = run(&config, "quotidien", WRITE, json!({"page": "Zoé Roux", "facts": ["Passeport : 12AB34567"]}), &t);
        assert!(error && said == ID_NUMBER);
    }

    #[test]
    fn only_useful_pages_reach_the_model() {
        let (_dir, config) = setup();
        let t = turn("run_setup");
        for n in 0..8 {
            run(&config, "quotidien", WRITE, json!({"page": format!("Sujet {n}"), "kind": "subject", "section": "decisions",
                "facts": [format!("Décision {n}")]}), &t);
        }
        run(&config, "quotidien", WRITE, json!({"page": "Léa Martin", "kind": "person", "section": "tastes",
            "facts": ["Pointure : 36", "Aime le jazz"], "summary": "Sœur de Marc"}), &t);
        // without words: the counts and the 10 latest, never every title
        let (all, _) = run(&config, "quotidien", SEARCH, json!({}), &t);
        assert!(all.starts_with("Pages: person 1 · subject 8
"), "{all}");
        assert_eq!(all.lines().count(), 1 + 9);
        // a question: the answer first
        let reader = turn("run_reader");
        let (found, _) = run(&config, "quotidien", SEARCH, json!({"words": "Léa chausse du combien"}), &reader);
        let today = super::super::today();
        assert!(found.starts_with(&format!("Answers:\nPointure · Léa Martin: 36 ({today})\nPages:\nLéa Martin (person, Personnes/Léa Martin.md)")), "{found}");
        // a section: « En bref » and that section only
        let (read, _) = run(&config, "quotidien", READ, json!({"page": "Léa", "section": "tastes"}), &reader);
        assert!(read.starts_with("# Léa Martin\n") && read.contains("## En bref\nSœur de Marc") && read.contains("Pointure : 36"), "{read}");
        // six different pages a turn; the same one again is no new page
        for n in 0..5 {
            let (text, _) = run(&config, "quotidien", READ, json!({"page": format!("Sujet {n}")}), &reader);
            assert!(text.starts_with(&format!("# Sujet {n}")), "{text}");
        }
        assert!(run(&config, "quotidien", READ, json!({"page": "Léa Martin"}), &reader).0.starts_with("# Léa Martin"));
        let (limit, _) = run(&config, "quotidien", READ, json!({"page": "Sujet 6"}), &reader);
        assert_eq!(limit, "Limit reached: ask the person which page to read.");
        // the journal: one line per page and per turn, never the content
        let lines: Vec<Dict> = reads::entries(&config).into_iter().filter(|e| e["run"] == json!("run_reader")).collect();
        assert_eq!(lines.len(), 6, "{lines:?}");
        assert_eq!((lines[0]["how"].as_str(), lines[1].get("section")), (Some("search"), None));
        assert!(lines.iter().all(|l| !serde_json::to_string(l).unwrap().contains("jazz")));
        // the background « Remplir mon Carnet »: 20
        let fill = Turn { cap: reads::PAGES_PER_FILL, ..turn("run_fill") };
        for n in 0..8 {
            assert!(run(&config, "quotidien", READ, json!({"page": format!("Sujet {n}")}), &fill).0.starts_with("# Sujet"));
        }
    }

    #[test]
    fn records_list_short_and_get_the_detail() {
        let (_dir, config) = setup();
        let t = turn("run_records");
        let (said, _) = run(&config, "quotidien", RECORD, json!({"action": "add", "type": "contract",
            "fields": {"what": "Assurance habitation", "provider": "MAIF", "number": "HAB-778812", "end": "2027-03-01"}}), &t);
        assert!(said.starts_with("Added [r") && !said.contains("HAB-778812"), "{said}");
        let (list, _) = run(&config, "quotidien", RECORD, json!({"action": "list"}), &t);
        assert!(list.contains("Assurance habitation · MAIF · 2027-03-01") && !list.contains("HAB-778812"), "{list}");
        let id = list.split('[').nth(1).unwrap().split(']').next().unwrap().to_string();
        let (got, _) = run(&config, "quotidien", RECORD, json!({"action": "get", "id": id}), &t);
        assert!(got.contains("number: HAB-778812"), "{got}");
        let (said, error) = run(&config, "quotidien", RECORD, json!({"action": "add", "type": "paper",
            "fields": {"document": "Passeport", "holder": "Léa", "number": "12AB34567"}}), &t);
        assert!(error && said == ID_NUMBER);
        let (said, _) = run(&config, "quotidien", RECORD, json!({"action": "add", "type": "booking",
            "fields": {"date": "2026-12-01", "what": "Hôtel", "with": "Ibis"}}), &t);
        assert!(said.contains("Ibis"), "a booking's « with » is its provider: {said}");
        // papers: one per document and holder
        for holder in ["Léa", "Marc", "Léa"] {
            run(&config, "quotidien", RECORD, json!({"action": "add", "type": "paper",
                "fields": {"document": "Passeport", "holder": holder, "expires": "2031-03-12"}}), &t);
        }
        let (list, _) = run(&config, "quotidien", RECORD, json!({"action": "list", "type": "paper"}), &t);
        assert_eq!(list.lines().count(), 2, "{list}");
        assert!(list.contains("Passeport · Léa · 2031-03-12") && list.contains("Passeport · Marc"), "{list}");
    }

    #[test]
    fn the_line_under_the_answer() {
        let (_dir, config) = setup();
        let t = turn("run_line");
        run(&config, "quotidien", WRITE, json!({"page": "Léa Martin", "kind": "person", "facts": ["Aime le jazz"]}), &turn("run_before"));
        let from = crate::py::time_time() - 1.0;
        run(&config, "quotidien", READ, json!({"page": "Léa Martin", "section": "tastes"}), &t);
        let span = |session: &str| reads::Span { turn: "m1".into(), session: session.into(), from, to: f64::MAX };
        let lines = reads::lines(&config, "quotidien", "fr", &[span("s-run_line")]);
        assert_eq!(lines["m1"]["text"], "A consulté 1 page");
        assert_eq!(lines["m1"]["pages"][0], json!({"title": "Léa · Goûts et préférences", "path": "Personnes/Léa Martin.md", "section": "tastes"}));
        run(&config, "quotidien", WRITE, json!({"page": "Toi", "kind": "self", "facts": ["Habite à Lyon"]}), &t);
        run(&config, "quotidien", READ, json!({"page": "Toi"}), &t);
        let lines = reads::lines(&config, "quotidien", "fr", &[span("s-run_line")]);
        assert_eq!(lines["m1"]["text"], "A consulté 2 pages · Noté dans Toi");
        assert_eq!(lines["m1"]["all"], "Tout voir");
        assert_eq!(reads::lines(&config, "quotidien", "en", &[span("s-run_line")])["m1"]["text"], "Read 2 pages · Noted in You");
        // another session, another turn: nothing
        assert!(reads::lines(&config, "quotidien", "fr", &[span("s-other")]).is_empty());
        // « Lignes Noté » off: only what was read
        super::super::update_state(&config, |s| {
            s.insert("mentions".into(), json!(false));
        })
        .unwrap();
        assert_eq!(reads::lines(&config, "quotidien", "fr", &[span("s-run_line")])["m1"]["text"], "A consulté 2 pages");
        // a page deleted since
        super::super::delete_page(&config, Part::Main, "Personnes/Léa Martin.md", &Who::you(&config)).unwrap();
        let lines = reads::lines(&config, "quotidien", "fr", &[span("s-run_line")]);
        assert_eq!(lines["m1"]["pages"][0], json!({"title": "Page oubliée", "path": null}));
    }

    #[test]
    fn the_app_writes_a_section_over_what_it_read() {
        let (_dir, config) = setup();
        let t = turn("run_app");
        run(&config, "quotidien", WRITE, json!({"page": "Léa Martin", "kind": "person", "facts": ["Aime le jazz"]}), &t);
        let path = "Personnes/Léa Martin.md";
        let read = super::super::rev(&super::super::read_page_text(&config, Part::Main, path).unwrap());
        // an agent writes in between
        run(&config, "quotidien", WRITE, json!({"page": "Léa Martin", "section": "tastes", "facts": ["Aime le rock"]}), &t);
        let you = Who::you(&config);
        let stale = super::super::edit(&config, &[Part::Main], path, "", "", "tastes", Some("- Aime le jazz"), "", Some(&read), &you);
        let changed = stale.unwrap_err();
        let now = super::super::rev(&super::super::read_page_text(&config, Part::Main, path).unwrap());
        assert_eq!(changed.downcast_ref::<super::super::PageChanged>().unwrap().0, now);
        assert!(super::super::read_page_text(&config, Part::Main, path).unwrap().contains("Aime le rock"), "nothing lost");
        super::super::edit(&config, &[Part::Main], path, "", "", "tastes", Some("- Aime le jazz"), "", Some(&now), &you).unwrap();
        // a new page from the app; one of that name already there: that one
        assert_eq!(super::super::new_page(&config, Part::Main, "subject", "Jardin", &you).unwrap(), Ok("Sujets/Jardin.md".to_string()));
        assert_eq!(super::super::new_page(&config, Part::Main, "person", "Léa Martin", &you).unwrap(), Err(path.to_string()));
    }
}
