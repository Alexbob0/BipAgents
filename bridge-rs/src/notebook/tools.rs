//! The Carnet's tools for the agents (the bridge's MCP server, agent_tools.rs), on by default for every agent:
//!
//!     notebook_search   find pages by name, alias, contact id or words (search before writing: update, don't duplicate)
//!     notebook_read     one page, whole
//!     notebook_note     file facts about a person, a subject or an area (the page made if missing), its « En bref »
//!     notebook_edit     correct or remove a fact, rewrite a section or the « En bref »
//!     notebook_task     list, add (with a date: a phone reminder too) or tick off a task
//!     notebook_record   precise records: purchases, subscriptions, contracts and insurance, papers, bookings
//!
//! No approval asked: filing is the point, and every change can be undone from the page's history. An agent the
//! person gave their health to also reads and writes the private health part (`private: true`); no other agent sees
//! it, and the health page of the shared part is refused to them.

use serde_json::{json, Value};

use super::{Part, Who};
use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::Dict;

pub const SEARCH: &str = "notebook_search";
pub const READ: &str = "notebook_read";
pub const NOTE: &str = "notebook_note";
pub const EDIT: &str = "notebook_edit";
pub const TASK: &str = "notebook_task";
pub const RECORD: &str = "notebook_record";
pub const TOOL_NAMES: [&str; 6] = [SEARCH, READ, NOTE, EDIT, TASK, RECORD];

fn private_property() -> Value {
    json!({"type": "boolean", "description": "true for health facts: they go to the person's private health part, \
which only you (the agent they chose for health) and they can read. Never put health facts anywhere else."})
}

/// The tools' definitions; `health`: the agent may use the private health part.
pub fn tools(health: bool) -> Vec<Value> {
    let mut list = vec![
        json!({
            "name": SEARCH,
            "description": "Search the person's Carnet (their notebook, shared by all their agents): pages about people, \
        subjects and areas of their life, and their tasks. Finds a page by its name, a nickname (alias), a phone contact's id, \
        or words in it. Always search before filing something, to update the page that exists instead of making a new one. \
        Without words: the list of pages (of one kind if given).",
            "inputSchema": {"type": "object", "properties": {
                "words": {"type": "string", "description": "a name, a nickname or a few words"},
                "kind": {"type": "string", "enum": ["person", "subject", "area", "trip"]}}},
        }),
        json!({
            "name": READ,
            "description": "Read one page of the Carnet, whole: its summary (« En bref »), its facts by section with who \
        wrote them and when, and its links.",
            "inputSchema": {"type": "object", "properties": {
                "page": {"type": "string", "description": "the page's name, a nickname, or its path from a search"}},
                "required": ["page"]},
        }),
        json!({
            "name": NOTE,
            "description": "File durable facts in the Carnet: about your person themself (kind self: their « Toi » page; who \
        they are, family, work, likes and habits, goals, how their agents should behave with them), a person (birthday, \
        family, preferences, what was agreed, recent exchanges), a subject or project (decisions, status, next steps, people involved) or an area of life (home, \
        money, work, family…) or a trip (kind trip: one page per journey, named by place and month « Amsterdam, déc. 2021 », never « Voyages »; give start and end; a \
        subject that turns out to be a trip becomes one when you file it with kind trip, its content kept). The page is found \
        by its name, a nickname or the contact's id, and made if it doesn't exist. \
        Facts already there are not repeated. Every page has the same fixed sections for its kind: give the section \
        the facts belong to (one call per section); one short fact per item. Write each fact short and dated when it matters (« Anniversaire : 14 novembre », \
        « Devis plombier accepté le 22/10 : 480 € »); link pages with [[Name]]. Give the page's « En bref » when it changes: \
        2 or 3 short lines saying what matters most now. Not for small talk, one-off questions, passwords or codes, or what \
        the person asked not to keep.",
            "inputSchema": {"type": "object", "properties": {
                "page": {"type": "string", "description": "the person's, subject's or area's name (« Marc Dupont », « Rénovation cuisine », « Maison »)"},
                "kind": {"type": "string", "enum": ["person", "subject", "area", "self", "trip"], "description": "for a new page; self: your person themself (their « Toi » page, one per Carnet); trip: a journey (also turns an existing subject into a trip)"},
                "facts": {"type": "array", "items": {"type": "string"}, "maxItems": 20, "description": "short facts, one per item"},
                "section": {"type": "string", "enum": super::template::all_ids(), "description": super::template::described()},
                "summary": {"type": "string", "description": "the page's new « En bref », 2–3 short lines (when what matters most changes)"},
                "aliases": {"type": "array", "items": {"type": "string"}, "description": "other names or nicknames"},
                "contact": {"type": "string", "description": "the phone contact's id, when you know it (from contacts_search)"},
                "close": {"type": "boolean", "description": "true when the person says this person is close to them (family, close friends): their dates (birthday…) are then shown in « Dates à venir »"},
                "start": {"type": "string", "description": "a trip's first day, YYYY-MM-DD"},
                "end": {"type": "string", "description": "a trip's last day, YYYY-MM-DD"},
                "destination": {"type": "string", "description": "a trip's destination (« Kyoto, Japon »)"}},
                "required": ["page", "facts"]},
        }),
        json!({
            "name": EDIT,
            "description": "Correct the Carnet when something changed or was wrong: replace a fact (old: words of the fact, \
        new: the corrected fact; new empty removes it), or rewrite a whole section (section and text, e.g. a subject's status), \
        and/or rewrite the page's « En bref » (summary). Read the page first. split_trips: true on a page mixing several \
        journeys (« Voyages » with flights of different years): the Carnet proposes to the person to split it into one trip \
        page each (they answer « Séparer » in the app; nothing is moved before).",
            "inputSchema": {"type": "object", "properties": {
                "page": {"type": "string"},
                "old": {"type": "string", "description": "words of the fact to correct"},
                "new": {"type": "string", "description": "the corrected fact (empty: remove it)"},
                "section": {"type": "string", "enum": super::template::all_ids(), "description": "the section to rewrite, by its id (as in notebook_note)"},
                "text": {"type": "string", "description": "the section's new content (list lines « - … »)"},
                "summary": {"type": "string", "description": "the page's new « En bref », 2–3 short lines"},
                "split_trips": {"type": "boolean", "description": "true: propose to the person to split this page into its journeys (one trip page each)"}},
                "required": ["page"]},
        }),
        json!({
            "name": TASK,
            "description": "The person's tasks in the Carnet: few and real. A fact is not a task: a birthday or someone \
        else's deadline is a dated fact on its page (notebook_note), never a task by itself. action add only when the person \
        asks (« rappelle-moi… », « il faut que je… »: origin asked) or states their own commitment with a deadline (origin \
        own); with a date (due: YYYY-MM-DD, or YYYY-MM-DDTHH:MM) it also becomes a reminder on their phone when they share \
        their reminders with you. To suggest a task yourself, first action propose (kind, title): only when clearly useful; \
        the Carnet says whether you may (a few a week at most, never a kind the person declined); if it gives an id, ask \
        the person in one sentence; yes: action add with origin accepted and that proposal id; no: action decline with the \
        id (never: true when they don't want this kind at all). action done ticks one off (its id); action list: the open ones, \
        and the Carnet's own proposals waiting for the person's answer (a warranty ending, a renewal, a paper expiring).",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["add", "done", "list", "propose", "decline"]},
                "title": {"type": "string"},
                "due": {"type": "string"},
                "page": {"type": "string", "description": "the page it belongs to (a person, a subject, an area)"},
                "origin": {"type": "string", "enum": ["asked", "own", "accepted"]},
                "kind": {"type": "string", "enum": super::PROPOSAL_KINDS, "description": "for propose"},
                "proposal": {"type": "string", "description": "the proposal's id (for add with origin accepted, and decline)"},
                "never": {"type": "boolean", "description": "for decline: the person doesn't want this kind of proposal any more"},
                "id": {"type": "string", "description": "for done: the task's id (from list)"}},
                "required": ["action"]},
        }),
        json!({
            "name": RECORD,
            "description": "Precise records in the person's Carnet, one line each with every detail, instead of vague \
        facts: a purchase (date, item, store, price, class: goods for a durable thing, else service, ticket (event, \
        registration), food, digital, travel, fee or other; end of warranty: leave it out when unknown, the EU's \
        two-year legal guarantee is computed for durable goods only), a subscription (service, price, period, next renewal, how to cancel), a contract or an \
        insurance (type, provider, number, end), a paper (document: passport, ID card, driving licence…, expiry) or a \
        booking (date, with time if any: YYYY-MM-DD HH:MM; what, with whom, booking reference, the trip it belongs to). \
        Dates as YYYY-MM-DD. source: the reference of the document it comes from when you have one. action add (the \
        same record given twice is merged: same item and date, same service, same contract number, same document, same \
        booking reference), update (its id, only the fields that change), remove (its id), list (one type, or all; \
        the archived ones marked: past trips' bookings, ended subscriptions, purchases out of warranty…), archive or \
        unarchive (its id: the Carnet archives what is past by itself at night). \
        The Carnet proposes the reminders itself (a warranty ending, a renewal, a paper expiring): don't add tasks for \
        them. Never card numbers, codes or passwords.",
            "inputSchema": {"type": "object", "properties": {
                "action": {"type": "string", "enum": ["add", "update", "remove", "list", "archive", "unarchive"]},
                "type": {"type": "string", "enum": super::records::TYPES.iter().map(|t| t.id).collect::<Vec<_>>()},
                "id": {"type": "string", "description": "the record's id (from add or list), for update and remove"},
                "fields": {"type": "object", "description": "the record's fields (those of its type)", "properties":
                    super::records::all_fields().iter().map(|f| (f.to_string(), json!({"type": "string"}))).collect::<serde_json::Map<String, Value>>()}},
                "required": ["action"]},
        }),
    ];
    if health {
        for tool in list.iter_mut() {
            let name = tool["name"].as_str().unwrap_or("").to_string();
            if [NOTE, TASK].contains(&name.as_str()) {
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

/// What the agent says after filing (or not: the person turned these mentions off).
fn mention(config: &Config, what: &str) -> String {
    if super::mentions(config) {
        format!("\nKeep your answer to the person as it was, and add at its end one short line in their language, e.g. « Noté dans ton Carnet : {what} ».")
    } else {
        "\nThe person turned these mentions off: don't mention it.".into()
    }
}

/// One call of a Carnet tool by `agent`. Err: a file that cannot be written.
pub fn call(config: &Config, agent: &str, name: &str, arguments: &Dict) -> anyhow::Result<Dict> {
    if !super::enabled(config, agent) {
        return Ok(tool_text(
            "The Carnet is off for you: the person switches « Carnet » on in your page of the app.",
            true,
        ));
    }
    let health = super::health_agent(config, agent);
    let readable: Vec<Part> = if health { vec![Part::Main, Part::Health] } else { vec![Part::Main] };
    let private = health && arguments.get("private").map(crate::py::truthy).unwrap_or(false);
    if !health && arguments.get("private").map(crate::py::truthy).unwrap_or(false) {
        return Ok(tool_text(
            "Health facts go only to the agent the person chose for their health: don't file them.",
            true,
        ));
    }
    let who = Who::agent(config, agent);
    let w = super::words(&super::language(config));
    let caught = |err: anyhow::Error| -> anyhow::Result<Dict> {
        match err.downcast_ref::<super::NotebookError>() {
            Some(message) => Ok(tool_text(&message.0, true)),
            None => Err(err),
        }
    };
    let marker = |part: Part| if part == Part::Health { " [private health part]" } else { "" };
    match name {
        SEARCH => {
            let words = s(arguments, "words");
            let kind = s(arguments, "kind");
            let found = super::search(config, &readable, &words, &kind);
            if found.is_empty() {
                return Ok(tool_text(
                    if words.is_empty() { "The Carnet is empty for now." } else { "Nothing found in the Carnet." },
                    false,
                ));
            }
            let mut out: Vec<String> = Vec::new();
            for (part, path, page, lines) in found.iter().take(if words.is_empty() { 150 } else { 20 }) {
                let aliases = page.aliases();
                let mut head = format!("{} ({path}){}", page.title, marker(*part));
                if w.archived(path) {
                    head.push_str(" [archived: past]");
                }
                if !aliases.is_empty() {
                    head.push_str(&format!(" — also: {}", aliases.join(", ")));
                }
                out.push(head);
                for line in lines {
                    out.push(format!("  {line}"));
                }
            }
            Ok(tool_text(&out.join("\n"), false))
        }
        READ => match super::find(config, &readable, &s(arguments, "page")) {
            Some(found) => {
                let text = super::read_page_text(config, found.part, &found.path).unwrap_or_default();
                Ok(tool_text(&format!("{}{}\n{}", found.path, marker(found.part), tk::head(&text, 20000)), false))
            }
            None => Ok(tool_text("No such page in the Carnet (notebook_search finds pages by name or words).", true)),
        },
        NOTE => {
            let page = s(arguments, "page");
            let kind = {
                let k = s(arguments, "kind");
                if k.is_empty() {
                    "person".to_string()
                } else {
                    k
                }
            };
            let is_health_page = super::fold(&page) == super::fold(w.health) || super::fold(&page) == "health";
            if is_health_page && !health {
                return Ok(tool_text(
                    "Health facts go only to the agent the person chose for their health: don't file them.",
                    true,
                ));
            }
            let part = if private || (health && is_health_page) { Part::Health } else { Part::Main };
            let facts = args_list(arguments, "facts");
            let trip_only =
                kind == "trip" || ["start", "end", "destination"].iter().any(|k| !s(arguments, k).is_empty());
            if facts.is_empty() && s(arguments, "summary").is_empty() && arguments.get("close").is_none() && !trip_only
            {
                return Ok(tool_text("give the facts to file", true));
            }
            match super::note(
                config,
                part,
                &kind,
                &page,
                &s(arguments, "section"),
                &facts,
                &args_list(arguments, "aliases"),
                &s(arguments, "contact"),
                &s(arguments, "summary"),
                &who,
            )
            .and_then(|written| {
                // « c'est un proche »: their dates are shown (the person's word, kept on the page)
                if let Some(Value::Bool(close)) = arguments.get("close") {
                    if part == Part::Main {
                        super::set_close(config, &written.path, *close, &who)?;
                    }
                }
                // a trip's dates and destination, in its frontmatter
                let (start, end, destination) =
                    (s(arguments, "start"), s(arguments, "end"), s(arguments, "destination"));
                if !(start.is_empty() && end.is_empty() && destination.is_empty()) {
                    let changed =
                        super::trip::set_details(config, part, &written.path, &start, &end, &destination, &who)?;
                    if changed {
                        return Ok(super::Written { changed: true, ..written });
                    }
                }
                Ok(written)
            }) {
                Ok(written) if written.added.is_empty() && !written.created && written.changed => Ok(tool_text(
                    &format!("The Carnet's trip page is up to date: {} ({}).", written.title, written.path),
                    false,
                )),
                Ok(written) if written.added.is_empty() && !written.created => {
                    Ok(tool_text(&format!("Already in the Carnet ({}): nothing new to file.", written.title), false))
                }
                Ok(written) => {
                    let what = format!("{} — {}", written.title, written.added.join(" ; "));
                    let made = if written.created { "new page" } else { "updated" };
                    let under = if written.sections.is_empty() {
                        String::new()
                    } else {
                        format!(" Under: {}.", written.sections.join(", "))
                    };
                    Ok(tool_text(
                        &format!(
                            "Filed in the Carnet: {} ({made}, {}{}), {} fact(s).{under}{}",
                            written.title,
                            written.path,
                            marker(part),
                            written.added.len(),
                            mention(config, &what)
                        ),
                        false,
                    ))
                }
                Err(err) => caught(err),
            }
        }
        EDIT if arguments.get("split_trips").map(crate::py::truthy).unwrap_or(false) => {
            use super::trip_split::Proposed;
            match super::trip_split::propose(config, &s(arguments, "page")) {
                Ok(Proposed::Made(_, text) | Proposed::Open(_, text)) => Ok(tool_text(
                    &format!("Proposed to the person in the app: « {text} ». Nothing moves until they say « Séparer »; tell them in one sentence."),
                    false,
                )),
                Ok(Proposed::Declined) => Ok(tool_text("The person chose to leave this page as it is.", false)),
                Ok(Proposed::Nothing) => Ok(tool_text(
                    "This page holds one journey at most (or isn't a trip or a list of trips): nothing to split.",
                    false,
                )),
                Err(err) => caught(err),
            }
        }
        EDIT => {
            let text = arguments.get("text").map(|_| s(arguments, "text"));
            if s(arguments, "old").is_empty() && text.is_none() && s(arguments, "summary").is_empty() {
                return Ok(tool_text("give old (and new), or section and text, or summary", true));
            }
            match super::edit(
                config,
                &readable,
                &s(arguments, "page"),
                &s(arguments, "old"),
                &s(arguments, "new"),
                &s(arguments, "section"),
                text.as_deref(),
                &s(arguments, "summary"),
                &who,
            ) {
                Ok((written, summary)) => Ok(tool_text(
                    &format!("Corrected in the Carnet: {} ({summary}).{}", written.title, mention(config, &summary)),
                    false,
                )),
                Err(err) => caught(err),
            }
        }
        TASK => {
            let action = s(arguments, "action");
            let parts: Vec<Part> = if private { vec![Part::Health] } else { readable.clone() };
            match action.as_str() {
                "list" => {
                    let mut lines = Vec::new();
                    for part in &parts {
                        for task in super::open_tasks(config, *part) {
                            let due = if task.due.is_empty() { String::new() } else { format!(" · due {}", task.due) };
                            lines.push(format!("- id {} · {}{due}{}", task.id, task.title, marker(*part)));
                        }
                    }
                    let waiting: Vec<String> = super::open_proposals(config)
                        .iter()
                        .filter(|p| tk::str_or_empty(p.get("agent")).is_empty())
                        .filter(|p| tk::str_or_empty(p.get("kind")) != super::trip_split::KIND)
                        .map(|p| {
                            format!(
                                "- proposal {} · {} · due {}",
                                tk::str_or_empty(p.get("id")),
                                tk::str_or_empty(p.get("title")),
                                tk::str_or_empty(p.get("due"))
                            )
                        })
                        .collect();
                    if !waiting.is_empty() {
                        lines.push(format!(
                            "Proposed by the Carnet, waiting for the person's answer (they answer in the app; if it fits \
the conversation, ask them in one sentence: yes is action add with origin accepted and the proposal id):\n{}",
                            waiting.join("\n")
                        ));
                    }
                    if lines.is_empty() {
                        Ok(tool_text("No open task.", false))
                    } else {
                        Ok(tool_text(&lines.join("\n"), false))
                    }
                }
                "propose" => match super::propose(config, agent, &s(arguments, "kind"), &s(arguments, "title"), &s(arguments, "due")) {
                    Ok(id) => Ok(tool_text(
                        &format!("You may propose it (proposal {id}): ask the person in one short sentence. Yes: action add with origin accepted and proposal {id}. No: action decline with proposal {id} (never: true if they don't want this kind at all)."),
                        false,
                    )),
                    Err(err) => caught(err),
                },
                "decline" => match super::decline(config, &s(arguments, "proposal"), arguments.get("never").map(crate::py::truthy).unwrap_or(false), &who) {
                    Ok(Some(fact)) => Ok(tool_text(&format!("Noted: no more proposals of this kind (in the Carnet: « {fact} »)."), false)),
                    Ok(None) => Ok(tool_text("Noted: not added.", false)),
                    Err(err) => caught(err),
                },
                "add" => {
                    let part = if private { Part::Health } else { Part::Main };
                    match super::add_task(
                        config,
                        part,
                        &s(arguments, "title"),
                        &s(arguments, "due"),
                        &s(arguments, "page"),
                        &s(arguments, "origin"),
                        &s(arguments, "proposal"),
                        &who,
                    ) {
                        Ok(super::Added { task, reminder, existing: true, dated }) => {
                            let due = if task.due.is_empty() { String::new() } else { format!(", due {}", task.due) };
                            let date = if dated { " It had no date: it now has this one." } else { "" };
                            let phone =
                                if reminder { " It will also appear in the person's phone reminders." } else { "" };
                            Ok(tool_text(
                                &format!(
                                    "Already in the Carnet as ^{}: {}{due}. Not added again.{date}{phone}",
                                    task.id, task.title
                                ),
                                false,
                            ))
                        }
                        Ok(super::Added { task, reminder, .. }) => {
                            let phone =
                                if reminder { " It will also appear in the person's phone reminders." } else { "" };
                            let due = if task.due.is_empty() { String::new() } else { format!(", due {}", task.due) };
                            Ok(tool_text(
                                &format!(
                                    "Task in the Carnet: {} (id {}{due}).{phone}{}",
                                    task.title,
                                    task.id,
                                    mention(config, &task.title)
                                ),
                                false,
                            ))
                        }
                        Err(err) => caught(err),
                    }
                }
                "done" => match super::set_task_done(config, &parts, &s(arguments, "id"), true, &who, false) {
                    Ok(Some(task)) => Ok(tool_text(&format!("Done: {}.", task.title), false)),
                    Ok(None) => Ok(tool_text("unknown task id (action list gives them)", true)),
                    Err(err) => caught(err),
                },
                _ => Ok(tool_text("action is add, done, list, propose or decline", true)),
            }
        }
        RECORD => record(config, arguments, &who).or_else(caught),
        _ => Ok(tool_text(&format!("unknown tool: {name}"), true)),
    }
}

fn record(config: &Config, arguments: &Dict, who: &Who) -> anyhow::Result<Dict> {
    use super::records::{self, Upserted};
    let action = s(arguments, "action");
    let kind = records::kind(&s(arguments, "type"));
    let fields: Dict = match arguments.get("fields") {
        Some(Value::Object(f)) => f.clone(),
        _ => Dict::new(),
    };
    match action.as_str() {
        "list" => {
            let kinds: Vec<&records::Kind> = match kind {
                Some(k) => vec![k],
                None => records::TYPES.iter().collect(),
            };
            let mut lines: Vec<String> = Vec::new();
            for k in kinds {
                for r in records::records(config, k) {
                    lines.push(format!(
                        "- {} id {} · {}",
                        k.id,
                        tk::str_or_empty(r.get("id")),
                        records::summary_of(k, &r)
                    ));
                }
                for r in records::archived_records(config, k) {
                    lines.push(format!(
                        "- {} id {} [archived] · {}",
                        k.id,
                        tk::str_or_empty(r.get("id")),
                        records::summary_of(k, &r)
                    ));
                }
            }
            if lines.is_empty() {
                return Ok(tool_text("No record yet.", false));
            }
            Ok(tool_text(&lines.join("\n"), false))
        }
        "add" | "update" => {
            let Some(kind) = kind else {
                return Ok(tool_text("type is purchase, subscription, contract, paper or booking", true));
            };
            let (record, done) = records::upsert(config, kind, &s(arguments, "id"), &fields, action == "update", who)?;
            let line = records::summary_of(kind, &record);
            let id = tk::str_or_empty(record.get("id"));
            let what = match done {
                Upserted::Added => "Recorded in the Carnet",
                Upserted::Updated => "Updated in the Carnet",
                Upserted::Same => "Already in the Carnet",
            };
            let mention = if done == Upserted::Same { String::new() } else { mention(config, &line) };
            Ok(tool_text(&format!("{what}: {} (id {id}): {line}.{mention}", kind.id), false))
        }
        "remove" => {
            let Some(kind) = kind else {
                return Ok(tool_text("type is purchase, subscription, contract, paper or booking", true));
            };
            match records::remove(config, kind, &s(arguments, "id"), who)? {
                Some(r) => {
                    Ok(tool_text(&format!("Removed from the Carnet: {}.", records::summary_of(kind, &r)), false))
                }
                None => Ok(tool_text("unknown record id (action list gives them)", true)),
            }
        }
        "archive" | "unarchive" => {
            let Some(kind) = kind else {
                return Ok(tool_text("type is purchase, subscription, contract, paper or booking", true));
            };
            let archived = action == "archive";
            match super::archive::set_record_archived(config, kind, &s(arguments, "id"), archived, who)? {
                Some(r) => Ok(tool_text(
                    &format!(
                        "{} {}.",
                        if archived { "Archived:" } else { "Back from the archives:" },
                        records::summary_of(kind, &r)
                    ),
                    false,
                )),
                None => Ok(tool_text("unknown record id there (action list gives them)", true)),
            }
        }
        _ => Ok(tool_text("action is add, update, remove, list, archive or unarchive", true)),
    }
}
