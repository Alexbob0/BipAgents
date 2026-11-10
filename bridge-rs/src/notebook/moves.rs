//! « Déplacer vers… » (Alex, 10/11: « il faudrait qu'on puisse les bouger nous-mêmes »): the person moves a point,
//! or a whole section, to another page and section, from the app (api.rs `POST /v1/notebook/move`). Each point keeps
//! its line as written (who noted it, when); both pages get a « move » line in their history; « Annuler » moves them
//! back (`undo`, the last `MOVES_KEPT` moves).
//!
//! A move is also a correction (`corrections` in state.json): the agent that noted those points is not told off, it
//! is shown the way. When that agent writes again, on the page they were moved from, a fact like them (a same label,
//! or two distinctive words in common with one of them, or anything for the section moved whole) is filed where the
//! person put them (`redirect`, called by `note_locked`, no model), and the tool's answer says so — that is how the
//! agent learns. Undoing the move drops its correction; a move back drops the opposite one.

use std::cell::Cell;

use serde_json::{json, Value};

use super::{fold, lexicon, refuse, split_attribution, template, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// The moves kept for « Annuler ».
pub const MOVES_KEPT: usize = 50;
/// The corrections kept (the newest).
pub const CORRECTIONS_KEPT: usize = 200;
/// Points one move takes at most.
pub const POINTS_MAX: usize = 100;

/// Why a move is not made (the app gets a code for each: api.rs).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Refused {
    #[error("no such page")]
    NoPage,
    #[error("nothing to move")]
    Nothing,
    #[error("a health fact stays in the private health part")]
    Health,
    #[error("the move can no longer be undone")]
    Gone,
}

/// Where points go: a page by its path, or a new page (title and kind) in a part.
#[derive(Debug, Clone, Default)]
pub struct Target {
    pub part: Option<Part>,
    pub path: String,
    pub title: String,
    pub kind: String,
}

/// A move made.
#[derive(Debug, Clone, Default)]
pub struct Moved {
    pub id: String,
    pub moved: usize,
    pub to_part: Option<Part>,
    pub to_path: String,
    pub to_title: String,
    /// The agents that will file such points there from now on (their names).
    pub learnt: Vec<String>,
}

/// One point taken from a page: its line as it was, its section, its text and who noted it.
#[derive(Debug, Clone)]
struct Taken {
    line: String,
    section: String,
    text: String,
    who: String,
    date: String,
}

fn words_of(lang: &str) -> (&'static str, &'static str, &'static str) {
    match lang {
        "fr" => ("Déplacé vers « {} »", "Venu de « {} »", "Déplacement annulé"),
        "de" => ("Verschoben nach « {} »", "Kam von « {} »", "Verschieben rückgängig"),
        "es" => ("Movido a « {} »", "Venido de « {} »", "Movimiento deshecho"),
        _ => ("Moved to « {} »", "Came from « {} »", "Move undone"),
    }
}

fn item_text(line: &str) -> &str {
    let t = line.trim();
    t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).unwrap_or(t)
}

/// The words of a fact that tell it apart (folded, 5 letters or more, not a number, not a stop word), 12 at most.
pub fn distinctive(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in fold(text).split(' ') {
        if w.chars().count() < 5 || w.chars().all(|c| c.is_ascii_digit()) || lexicon::STOP.contains(&w) {
            continue;
        }
        if !out.iter().any(|o| o == w) {
            out.push(w.to_string());
        }
        if out.len() >= 12 {
            break;
        }
    }
    out
}

/// The heading in `lines` that means `section` on a page of `kind`: its title or id, or the same section of the
/// outline.
fn heading_of(lines: &[String], kind: &str, section: &str) -> Option<String> {
    let wanted = fold(section);
    let id = template::resolve(kind, section);
    lines.iter().filter_map(|l| l.strip_prefix("## ")).map(|n| n.trim().to_string()).find(|name| {
        fold(name) == wanted || (id.is_some() && template::resolve(kind, name) == id)
    })
}

/// The points of `body` to move: those whose text is one of `texts` (folded), or every point of `section`.
fn take(body: &str, kind: &str, texts: &[String], section: Option<&str>) -> (Vec<String>, Vec<Taken>) {
    let lines = super::body_lines(body);
    let wanted: Vec<String> = texts.iter().map(|t| fold(item_text(t))).filter(|t| !t.is_empty()).collect();
    let heading = section.and_then(|s| heading_of(&lines, kind, s));
    let mut kept: Vec<String> = Vec::new();
    let mut taken: Vec<Taken> = Vec::new();
    let mut current = String::new();
    for line in lines {
        if let Some(name) = line.strip_prefix("## ") {
            current = name.trim().to_string();
            kept.push(line);
            continue;
        }
        let is_item = line.trim().starts_with("- ") || line.trim().starts_with("* ");
        let (text, by) = split_attribution(item_text(&line));
        let chosen = is_item
            && taken.len() < POINTS_MAX
            && match &heading {
                Some(h) => fold(h) == fold(&current),
                None => {
                    let f = fold(&text);
                    wanted.contains(&f) && !taken.iter().any(|t| fold(&t.text) == f)
                }
            };
        if chosen {
            let (who, date) = by.unwrap_or_default();
            taken.push(Taken { line: line.trim().to_string(), section: current.clone(), text, who, date });
        } else {
            kept.push(line);
        }
    }
    (kept, taken)
}

/// `line` added at the end of section `title` of `lines` (made if missing), unless the same fact is there.
fn insert(lines: &mut Vec<String>, title: &str, line: &str) -> bool {
    let fact = fold(&split_attribution(item_text(line)).0);
    if lines.iter().any(|l| !l.starts_with('#') && fold(&split_attribution(item_text(l)).0) == fact) {
        return false;
    }
    match super::section_index(lines, title) {
        Some(start) => {
            let mut at = super::section_end(lines, start);
            while at > start + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.insert(at, line.to_string());
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("## {title}"));
            lines.push(line.to_string());
        }
    }
    true
}

fn joined(lines: &[String]) -> String {
    let mut text = lines.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim_matches('\n').to_string() + "\n"
}

/// A page's body put in its kind's outline (pages without one as they are).
fn shaped(config: &Config, kind: &str, body: &str) -> String {
    let lang = super::language(config);
    template::normalize(kind, &lang, super::words(&lang).brief, body).body
}

/// Moves points of page `path` (in `part`) to `to`, under `to_section` (an id of the page's outline; empty: where
/// each point's words say). `texts`: the points (their text, as the app shows them); `section`: a whole section
/// instead (its id or its title). `rev`: the revision the app read (another one: `PageChanged`). `learn`: kept as a
/// correction for the agents that noted them (the person's moves).
#[allow(clippy::too_many_arguments)]
pub fn move_points(
    config: &Config,
    part: Part,
    path: &str,
    rev: Option<&str>,
    texts: &[String],
    section: Option<&str>,
    to: &Target,
    to_section: &str,
    who: &Who,
    learn: bool,
) -> anyhow::Result<Moved> {
    let _guard = super::lock();
    move_locked(config, part, path, rev, texts, section, to, to_section, who, learn)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn move_locked(
    config: &Config,
    part: Part,
    path: &str,
    rev: Option<&str>,
    texts: &[String],
    section: Option<&str>,
    to: &Target,
    to_section: &str,
    who: &Who,
    learn: bool,
) -> anyhow::Result<Moved> {
    super::checked(path)?;
    let lang = super::language(config);
    let Some(before) = super::read_page_text(config, part, path) else { return Err(Refused::NoPage.into()) };
    if let Some(read) = rev.filter(|r| !r.is_empty()) {
        let now = super::rev(&before);
        if now != read {
            return Err(super::PageChanged(now).into());
        }
    }
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut page = Page::parse(&before, &stem);
    let kind = page.kind();
    let body = super::in_outline(config, &page);
    let (kept, taken) = take(&body, &kind, texts, section.filter(|s| !s.trim().is_empty()));
    if taken.is_empty() {
        return Err(Refused::Nothing.into());
    }
    let to_part = to.part.unwrap_or(part);
    if part == Part::Health && to_part == Part::Main && taken.iter().any(|t| lexicon::is_health(&t.text)) {
        return Err(Refused::Health.into());
    }
    let same = to_part == part && (to.path == path || (to.path.is_empty() && fold(&to.title) == fold(&page.title)));
    // the page they go to: this one (a section changed), a page by its path, or a new one
    let w = super::words(&lang);
    let (to_path, to_before, mut to_page) = if same {
        (path.to_string(), None, None)
    } else if to.kind == "inbox" && to_part == Part::Main {
        // « À ranger »: the nightly pass files them (made when missing)
        let inbox = w.inbox_path();
        match super::read_page_text(config, Part::Main, &inbox) {
            Some(text) => (inbox, Some(text.clone()), Some(Page::parse(&text, w.inbox))),
            None => {
                let mut p = Page::new("inbox", w.inbox);
                p.front.remove("aliases");
                (inbox, None, Some(p))
            }
        }
    } else if !to.path.is_empty() {
        super::checked(&to.path)?;
        let Some(text) = super::read_page_text(config, to_part, &to.path) else { return Err(Refused::NoPage.into()) };
        let stem = std::path::Path::new(&to.path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (to.path.clone(), Some(text.clone()), Some(Page::parse(&text, &stem)))
    } else {
        let title = tk::head(&tk::squeeze(to.title.trim()), 80).to_string();
        let kind = if super::KINDS.contains(&to.kind.as_str()) && to.kind != "self" { to.kind.clone() } else { "subject".into() };
        if title.is_empty() {
            return Err(Refused::NoPage.into());
        }
        let Some(folder) = w.folder(&kind) else { return Err(Refused::NoPage.into()) };
        let found = super::candidates(config, &[to_part], &title, &kind);
        match found.into_iter().next() {
            Some(f) => {
                let text = super::read_page_text(config, to_part, &f.path);
                (f.path, text, Some(f.page))
            }
            None => {
                let new_path = super::free_path(config, to_part, folder, &title, "");
                (new_path, None, Some(Page::new(&kind, &title)))
            }
        }
    };
    let target_kind = to_page.as_ref().map(|p| p.kind()).unwrap_or_else(|| kind.clone());
    let target_title = to_page.as_ref().map(|p| p.title.clone()).unwrap_or_else(|| page.title.clone());
    let inbox = target_kind == "inbox";
    // each point's line, its attribution kept; « BipAgents : … » on the page « BipAgents » loses its label
    let mut target_lines =
        super::body_lines(&to_page.as_ref().map(|p| super::in_outline(config, p)).unwrap_or_else(|| super::body_lines(&joined(&kept)).join("\n")));
    let mut moved_lines: Vec<(String, String, String)> = Vec::new();
    for t in &taken {
        let text = match lexicon::split_label(&t.text) {
            Some((label, rest)) if fold(&label) == fold(&target_title) => rest,
            _ => t.text.clone(),
        };
        let title = if inbox {
            if t.who.is_empty() { super::words(&lang).notes.to_string() } else { t.who.clone() }
        } else if template::outline(&target_kind).is_empty() {
            if to_section.trim().is_empty() { t.section.clone() } else { to_section.trim().to_string() }
        } else {
            let id = template::resolve(&target_kind, to_section)
                .unwrap_or_else(|| template::place(&target_kind, "", &text));
            template::title(&target_kind, id, &lang)
        };
        let line = if t.who.is_empty() || t.date.is_empty() {
            format!("- {text}")
        } else {
            format!("- {text} — *{}, {}*", t.who, t.date)
        };
        insert(&mut target_lines, &title, &line);
        moved_lines.push((line, t.section.clone(), t.line.clone()));
    }
    let id = format!("m{}", py::token_hex(6));
    let facts: Vec<String> = taken.iter().map(|t| t.text.clone()).collect();
    let (to_word, from_word, _) = words_of(&lang);
    let listed = tk::head(&facts.join(" · "), 200).to_string();
    let mut extra_from = Dict::new();
    extra_from.insert("points_to".into(), json!(to_path));
    extra_from.insert("move".into(), json!(id));
    let mut extra_to = Dict::new();
    extra_to.insert("points_from".into(), json!(path));
    extra_to.insert("move".into(), json!(id));
    if same {
        page.body = shaped(config, &kind, &joined(&target_lines));
        page.touch();
        let summary = format!("{} : {listed}", to_word.replace("{}", &template_title_list(&moved_lines, &target_lines)));
        super::commit_with(config, part, path, Some(&before), Some(&page.render()), who, "move", &summary, extra_from)?;
    } else {
        page.body = shaped(config, &kind, &joined(&kept));
        page.touch();
        let mut target = to_page.take().unwrap_or_default();
        target.ensure_id();
        target.body = shaped(config, &target_kind, &joined(&target_lines));
        target.touch();
        super::commit_with(
            config,
            to_part,
            &to_path,
            to_before.as_deref(),
            Some(&target.render()),
            who,
            "move",
            &format!("{} : {listed}", from_word.replace("{}", &page.title)),
            extra_to,
        )?;
        super::commit_with(
            config,
            part,
            path,
            Some(&before),
            Some(&page.render()),
            who,
            "move",
            &format!("{} : {listed}", to_word.replace("{}", &target_title)),
            extra_from,
        )?;
    }
    // kept for « Annuler »
    let record = json!({"id": id, "at": py::time_time(), "from_part": part.as_str(), "from_path": path,
        "to_part": to_part.as_str(), "to_path": to_path,
        "lines": moved_lines.iter().map(|(line, section, was)| json!({"line": line, "section": section, "was": was})).collect::<Vec<_>>()});
    // the agents that noted them: shown where such points go
    let mut learnt: Vec<String> = Vec::new();
    let mut corrections: Vec<Value> = Vec::new();
    if learn && !inbox {
        let mut by_agent: Vec<(String, String, Vec<&Taken>)> = Vec::new();
        for t in &taken {
            let Some(agent) = super::assign::agent_named(config, &t.who) else { continue };
            match by_agent.iter_mut().find(|(a, _, _)| *a == agent) {
                Some((_, _, list)) => list.push(t),
                None => by_agent.push((agent, t.who.clone(), vec![t])),
            }
        }
        let section_id = section
            .filter(|s| !s.trim().is_empty())
            .map(|s| template::resolve(&kind, s).map(str::to_string).unwrap_or_else(|| s.trim().to_string()))
            .unwrap_or_default();
        let to_section_id = template::resolve(&target_kind, to_section).map(str::to_string).unwrap_or_default();
        for (agent, name, list) in by_agent {
            let labels: Vec<String> = list
                .iter()
                .filter_map(|t| lexicon::split_label(&t.text).map(|(l, _)| lexicon::label_key(&l)))
                .filter(|l| !l.is_empty())
                .collect();
            let words: Vec<Vec<String>> = list.iter().map(|t| distinctive(&t.text)).filter(|w| !w.is_empty()).collect();
            corrections.push(json!({"id": id, "agent": agent, "from_part": part.as_str(), "from_path": path,
                "from_id": page.id(), "to_part": to_part.as_str(), "to_path": to_path, "to_title": target_title,
                "to_kind": target_kind, "to_section": to_section_id, "section": section_id, "labels": labels,
                "words": words, "at": py::time_time()}));
            learnt.push(name);
        }
    }
    super::update_state(config, |s| {
        let mut moves: Vec<Value> = s.get("moves").and_then(Value::as_array).cloned().unwrap_or_default();
        moves.push(record);
        let over = moves.len().saturating_sub(MOVES_KEPT);
        moves.drain(..over);
        s.insert("moves".into(), Value::Array(moves));
        if !corrections.is_empty() {
            let mut all: Vec<Value> = s.get("corrections").and_then(Value::as_array).cloned().unwrap_or_default();
            // a move back: the opposite correction goes (never two pages sending facts to each other)
            all.retain(|c| {
                !corrections.iter().any(|n| {
                    c["agent"] == n["agent"] && c["from_path"] == n["to_path"] && c["to_path"] == n["from_path"]
                })
            });
            all.extend(corrections);
            let over = all.len().saturating_sub(CORRECTIONS_KEPT);
            all.drain(..over);
            s.insert("corrections".into(), Value::Array(all));
        }
    })?;
    Ok(Moved { id, moved: taken.len(), to_part: Some(to_part), to_path, to_title: target_title, learnt })
}

/// The section a same-page move went to, for its history line.
fn template_title_list(moved: &[(String, String, String)], lines: &[String]) -> String {
    let Some((line, _, _)) = moved.first() else { return String::new() };
    let mut current = String::new();
    for l in lines {
        if let Some(name) = l.strip_prefix("## ") {
            current = name.trim().to_string();
        } else if l.trim() == line.trim() {
            return current;
        }
    }
    current
}

/// « Annuler » on move `id`: its points back where they were (those still on the page they went to). Its correction
/// goes with it.
pub fn undo(config: &Config, id: &str, who: &Who) -> anyhow::Result<(Part, String)> {
    let _guard = super::lock();
    let state = super::read_state(config);
    let Some(record) = state
        .get("moves")
        .and_then(Value::as_array)
        .and_then(|m| m.iter().find(|r| r["id"] == json!(id)).cloned())
    else {
        return Err(Refused::Gone.into());
    };
    let s = |k: &str| tk::str_or_empty(record.get(k));
    let (from_part, to_part) = (Part::parse(&s("from_part")), Part::parse(&s("to_part")));
    let (from_path, to_path) = (s("from_path"), s("to_path"));
    let lines: Vec<(String, String, String)> = record["lines"]
        .as_array()
        .map(|l| {
            l.iter()
                .map(|x| (tk::str_or_empty(x.get("line")), tk::str_or_empty(x.get("section")), tk::str_or_empty(x.get("was"))))
                .collect()
        })
        .unwrap_or_default();
    let lang = super::language(config);
    let (_, _, undone) = words_of(&lang);
    let Some(to_before) = super::read_page_text(config, to_part, &to_path) else { return Err(Refused::Gone.into()) };
    let stem = |p: &str| std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut to_page = Page::parse(&to_before, &stem(&to_path));
    let mut to_lines = super::body_lines(&super::in_outline(config, &to_page));
    let mut back: Vec<(String, String)> = Vec::new();
    for (line, section, was) in &lines {
        let fact = fold(&split_attribution(item_text(line)).0);
        if let Some(at) = to_lines.iter().position(|l| !l.starts_with('#') && fold(&split_attribution(item_text(l)).0) == fact) {
            to_lines.remove(at);
            back.push((section.clone(), was.clone()));
        }
    }
    if back.is_empty() {
        return Err(Refused::Gone.into());
    }
    let listed = tk::head(
        &back.iter().map(|(_, w)| split_attribution(item_text(w)).0).collect::<Vec<_>>().join(" · "),
        200,
    )
    .to_string();
    let same = from_part == to_part && from_path == to_path;
    let mut from_lines = if same {
        to_lines.clone()
    } else {
        match super::read_page_text(config, from_part, &from_path) {
            Some(text) => super::body_lines(&super::in_outline(config, &Page::parse(&text, &stem(&from_path)))),
            None => Vec::new(),
        }
    };
    for (section, was) in &back {
        let title = if section.is_empty() { super::words(&lang).notes.to_string() } else { section.clone() };
        insert(&mut from_lines, &title, was);
    }
    if !same {
        to_page.body = shaped(config, &to_page.kind(), &joined(&to_lines));
        to_page.touch();
        super::commit(config, to_part, &to_path, Some(&to_before), Some(&to_page.render()), who, "move", &format!("{undone} : {listed}"))?;
    }
    let from_before = super::read_page_text(config, from_part, &from_path);
    let mut from_page = match &from_before {
        Some(text) => Page::parse(text, &stem(&from_path)),
        None => return refuse("the page they came from is gone"),
    };
    from_page.body = shaped(config, &from_page.kind(), &joined(&from_lines));
    from_page.touch();
    super::commit(config, from_part, &from_path, from_before.as_deref(), Some(&from_page.render()), who, "move", &format!("{undone} : {listed}"))?;
    super::update_state(config, |st| {
        for key in ["moves", "corrections"] {
            let mut list: Vec<Value> = st.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
            list.retain(|r| r["id"] != json!(id));
            st.insert(key.into(), Value::Array(list));
        }
    })?;
    Ok((from_part, from_path))
}

// -- corrections, at an agent's write ----------------------------------------------------------------------------------

/// Where facts go instead of the page an agent named: a correction's page and section.
#[derive(Debug, Clone, PartialEq)]
pub struct Elsewhere {
    pub part: Part,
    /// The page's path when it is still there, else its title (made again).
    pub page: String,
    pub kind: String,
    pub section: String,
}

thread_local! {
    /// Corrections followed in this write already (a chain of moves never loops).
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Runs `f` one correction deeper (`None`: too deep, nothing redirected).
pub fn deeper<T>(f: impl FnOnce() -> T) -> T {
    DEPTH.with(|d| d.set(d.get() + 1));
    let out = f();
    DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    out
}

pub(crate) fn matches(correction: &Value, kind: &str, section: &str, fact: &str) -> bool {
    let labels: Vec<String> = super::strings(correction.get("labels"));
    if let Some((label, _)) = lexicon::split_label(fact) {
        if labels.contains(&lexicon::label_key(&label)) {
            return true;
        }
    }
    let moved_section = tk::str_or_empty(correction.get("section"));
    if !moved_section.is_empty() {
        let here = template::resolve(kind, section).map(str::to_string).unwrap_or_else(|| section.trim().to_string());
        if fold(&here) == fold(&moved_section) && moved_section != "other" {
            return true;
        }
    }
    let mine = distinctive(fact);
    correction.get("words").and_then(Value::as_array).is_some_and(|sets| {
        sets.iter().any(|set| {
            let set = super::strings(Some(set));
            let common = mine.iter().filter(|w| set.contains(w)).count();
            common >= 2 || (common >= 1 && mine.len() == 1)
        })
    })
}

/// The facts `agent` writes on page `path` (`page`, in `part`) under `section`: those the person's corrections send
/// elsewhere, by where they go, and the ones that stay.
pub fn redirect(
    config: &Config,
    agent: &str,
    part: Part,
    path: &str,
    page: &Page,
    section: &str,
    facts: &[String],
) -> (Vec<String>, Vec<(Elsewhere, Vec<String>)>) {
    if DEPTH.with(|d| d.get()) >= 3 || facts.is_empty() {
        return (facts.to_vec(), Vec::new());
    }
    let state = super::read_state(config);
    let id = page.id();
    let mine: Vec<Value> = state
        .get("corrections")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter(|c| {
                    tk::str_or_empty(c.get("agent")) == agent
                        && Part::parse(&tk::str_or_empty(c.get("from_part"))) == part
                        && (tk::str_or_empty(c.get("from_path")) == path
                            || (!id.is_empty() && tk::str_or_empty(c.get("from_id")) == id))
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if mine.is_empty() {
        return (facts.to_vec(), Vec::new());
    }
    let kind = page.kind();
    let mut stay: Vec<String> = Vec::new();
    let mut away: Vec<(Elsewhere, Vec<String>)> = Vec::new();
    for fact in facts {
        // the newest correction first
        match mine.iter().rev().find(|c| matches(c, &kind, section, fact)) {
            Some(c) => {
                let to_part = Part::parse(&tk::str_or_empty(c.get("to_part")));
                let to_path = tk::str_or_empty(c.get("to_path"));
                let there = super::read_page_text(config, to_part, &to_path).is_some();
                let target = Elsewhere {
                    part: to_part,
                    page: if there { to_path } else { tk::str_or_empty(c.get("to_title")) },
                    kind: tk::str_or_empty(c.get("to_kind")),
                    section: tk::str_or_empty(c.get("to_section")),
                };
                if target.page.is_empty() || (target.part == part && target.page == path) {
                    stay.push(fact.clone());
                    continue;
                }
                match away.iter_mut().find(|(t, _)| *t == target) {
                    Some((_, list)) => list.push(fact.clone()),
                    None => away.push((target, vec![fact.clone()])),
                }
            }
            None => stay.push(fact.clone()),
        }
    }
    (stay, away)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kerfy() -> Who {
        Who { by: "kerfy".into(), name: "Kerfy Studio".into(), agent: Some("kerfy".into()) }
    }

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        let agent = crate::config::AgentConfig { name: "kerfy".into(), display_name: "Kerfy Studio".into(), ..Default::default() };
        config.agents.insert("kerfy".into(), agent);
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    #[test]
    fn distinctive_words() {
        assert_eq!(distinctive("Promotion : Buffer, plan gratuit (Instagram, TikTok, X)"), [
            "promotion", "buffer", "gratuit", "instagram", "tiktok"
        ]);
    }

    #[test]
    fn a_point_moves_with_its_line_and_comes_back() {
        let (_dir, config) = setup();
        super::super::note(&config, Part::Main, "area", "Famille", "matters", &["Judo de Zoé : mercredi 14 h".into()], &[], "", "", &Who::you(&config)).unwrap();
        let written = super::super::note(&config, Part::Main, "area", "Famille", "other",
            &["Panorama concurrentiel : livré le 25/10".into(), "Concurrents : aucun ne cumule agents, famille et serveur UE".into()],
            &[], "", "", &kerfy()).unwrap();
        let path = written.path.clone();
        let to = Target { part: None, path: String::new(), title: "BipAgents".into(), kind: "subject".into() };
        let moved = move_points(&config, Part::Main, &path, None, &["Panorama concurrentiel : livré le 25/10".into()],
            None, &to, "", &Who::you(&config), true).unwrap();
        assert_eq!(moved.moved, 1);
        assert_eq!(moved.learnt, ["Kerfy Studio"]);
        let there = super::super::read_page_text(&config, Part::Main, &moved.to_path).unwrap();
        assert!(there.contains("- Panorama concurrentiel : livré le 25/10 — *Kerfy Studio, "), "{there}");
        let here = super::super::read_page_text(&config, Part::Main, &path).unwrap();
        assert!(!here.contains("Panorama"));
        // the same kind of fact goes there next time
        let again = super::super::note(&config, Part::Main, "area", "Famille", "other",
            &["Panorama concurrentiel : mis à jour le 9/11".into()], &[], "", "", &kerfy()).unwrap();
        assert_eq!(again.elsewhere, ["BipAgents"]);
        let there = super::super::read_page_text(&config, Part::Main, &moved.to_path).unwrap();
        assert!(there.contains("mis à jour le 9/11"), "{there}");
        // undone: back on Famille, the correction gone
        undo(&config, &moved.id, &Who::you(&config)).unwrap();
        let here = super::super::read_page_text(&config, Part::Main, &path).unwrap();
        assert!(here.contains("Panorama concurrentiel : livré le 25/10 — *Kerfy Studio, "), "{here}");
        assert!(super::super::read_state(&config).get("corrections").and_then(Value::as_array).unwrap().is_empty());
        assert!(matches!(undo(&config, &moved.id, &Who::you(&config)).unwrap_err().downcast_ref::<Refused>(), Some(Refused::Gone)));
    }

    #[test]
    fn a_whole_section_moves_and_its_label_goes() {
        let (_dir, config) = setup();
        let text = "---\ntype: area\n---\n# Famille\n\n## BipAgents\n- Promotion : Buffer, plan gratuit — *Kerfy Studio, 2026-10-25*\n- Pas en France : Grok Bot — *Kerfy Studio, 2026-10-25*\n";
        std::fs::create_dir_all(super::super::part_root(&config, Part::Main).join("Domaines")).unwrap();
        std::fs::write(super::super::part_root(&config, Part::Main).join("Domaines/Famille.md"), text).unwrap();
        let shown = super::super::api::page_view(&config, Part::Main, "Domaines/Famille.md").unwrap();
        let other = shown["sections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["facts"].as_array().unwrap().iter().any(|f| f["text"].as_str().unwrap().contains("Promotion")))
            .map(|s| s["title"].as_str().unwrap().to_string())
            .unwrap();
        let to = Target { part: None, path: String::new(), title: "BipAgents".into(), kind: "subject".into() };
        let moved = move_points(&config, Part::Main, "Domaines/Famille.md", None, &[], Some(&other), &to, "status",
            &Who::you(&config), true).unwrap();
        assert!(moved.moved >= 1);
        let there = super::super::read_page_text(&config, Part::Main, &moved.to_path).unwrap();
        assert!(there.contains("## Où on en est\n- Promotion : Buffer, plan gratuit — *Kerfy Studio, 2026-10-25*"), "{there}");
        let here = super::super::read_page_text(&config, Part::Main, "Domaines/Famille.md").unwrap();
        assert!(!here.contains("Promotion"), "{here}");
    }

    #[test]
    fn a_health_fact_stays_in_the_health_part() {
        let (_dir, config) = setup();
        let me = Who::you(&config);
        let w = super::super::note(&config, Part::Health, "area", "Santé", "matters",
            &["Allergie de Zoé : pénicilline".into(), "Présidentielle 2027 : qui sont les candidats ?".into()], &[], "", "", &me).unwrap();
        let to = Target { part: Some(Part::Main), path: String::new(), title: "Actualité".into(), kind: "subject".into() };
        let err = move_points(&config, Part::Health, &w.path, None, &["Allergie de Zoé : pénicilline".into()], None, &to, "", &me, true)
            .unwrap_err();
        assert_eq!(err.downcast_ref::<Refused>(), Some(&Refused::Health));
        let moved = move_points(&config, Part::Health, &w.path, None, &["Présidentielle 2027 : qui sont les candidats ?".into()],
            None, &to, "", &me, true).unwrap();
        assert_eq!(moved.to_part, Some(Part::Main));
    }
}
