//! What the agents read in the Carnet (Carnet v2, decisions 7-8 of internal/notes/carnet-v2/spec.md): only the pages
//! that help, 6 different pages a turn at most (20 for « Remplir mon Carnet », fill.rs), each read written in a
//! journal the person sees, and one line under the agent's answer, made by the bridge (« A consulté 2 pages · Noté
//! dans Léa »), instead of the agent saying it.
//!
//!     notebook/history/reads.jsonl   one line per page and per turn: {at, agent, session, run, page_id, section?,
//!                                    how: read|search|record|news, part?}: never the page's content; kept 90 days
//!                                    (the nightly pass, `purge`)
//!
//! The turn a tool call belongs to (`Turn`: its run, its Hermes session, its cap) comes from the MCP handler
//! (agent_tools.rs), which knows the run the hub follows; the Carnet's writes of that call carry the same `session`
//! and `run` in `log.jsonl` (`commit`), so the line under the answer finds both (`lines`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use super::Part;
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

/// Different pages a turn may read.
pub const PAGES_PER_TURN: usize = 6;
/// … and « Remplir mon Carnet »'s background task (fill.rs).
pub const PAGES_PER_FILL: usize = 20;
/// The journal keeps this many days.
pub const KEPT_DAYS: f64 = 90.0;
/// A tool call with no run the hub follows (a routine): its turn is this long.
const UNFOLLOWED_SECONDS: f64 = 600.0;
/// The turns remembered (their pages), this long.
const TURN_SECONDS: f64 = 4.0 * 3600.0;

/// The turn of a Carnet tool call: its run, its Hermes session, its page cap.
#[derive(Debug, Clone)]
pub struct Turn {
    pub run: Option<String>,
    pub session: Option<String>,
    pub cap: usize,
}

impl Default for Turn {
    fn default() -> Self {
        Turn { run: None, session: None, cap: PAGES_PER_TURN }
    }
}

thread_local! {
    /// The turn the Carnet's code on this thread works for (`with_turn`): `commit` writes its session and run.
    static CURRENT: RefCell<Option<Turn>> = const { RefCell::new(None) };
}

/// `work` done for `turn` (the tool call's blocking work, on one thread).
pub fn with_turn<T>(turn: &Turn, work: impl FnOnce() -> T) -> T {
    CURRENT.with(|c| *c.borrow_mut() = Some(turn.clone()));
    let out = work();
    CURRENT.with(|c| *c.borrow_mut() = None);
    out
}

/// The session and run of the turn the code works for, for a line of `log.jsonl`.
pub fn current() -> Option<(Option<String>, Option<String>)> {
    CURRENT.with(|c| c.borrow().as_ref().map(|t| (t.session.clone(), t.run.clone())))
}

#[derive(Default)]
struct Seen {
    at: f64,
    logged: HashSet<String>,
    read: HashSet<String>,
}

static TURNS: LazyLock<Mutex<HashMap<String, Seen>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn key(agent: &str, turn: &Turn, now: f64) -> String {
    match &turn.run {
        Some(run) => format!("{agent}\u{1f}{run}"),
        None => format!("{agent}\u{1f}@{}", (now / UNFOLLOWED_SECONDS) as i64),
    }
}

fn seen<T>(agent: &str, turn: &Turn, now: f64, work: impl FnOnce(&mut Seen) -> T) -> T {
    let mut turns = TURNS.lock().unwrap_or_else(|e| e.into_inner());
    turns.retain(|_, s| now - s.at < TURN_SECONDS);
    let entry = turns.entry(key(agent, turn, now)).or_default();
    entry.at = now;
    work(entry)
}

/// Whether the turn may read page `id`: one it read already, or a new one under its cap.
pub fn may_read(agent: &str, turn: &Turn, id: &str) -> bool {
    seen(agent, turn, py::time_time(), |s| {
        if s.read.contains(id) {
            return true;
        }
        if s.read.len() >= turn.cap {
            return false;
        }
        s.read.insert(id.to_string());
        true
    })
}

/// Unix seconds to the millisecond: a read made just after the person's message stays in its turn.
fn at_ms(now: f64) -> f64 {
    (now * 1000.0).round() / 1000.0
}

fn path(config: &Config) -> PathBuf {
    super::root(config).join("history").join("reads.jsonl")
}

fn append(config: &Config, entry: &Dict) {
    let file = path(config);
    let done = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut fh = std::fs::OpenOptions::new().append(true).create(true).open(&file)?;
        fh.write_all((py::Dumps::new().unicode().dumps(&Value::Object(entry.clone())) + "\n").as_bytes())?;
        py::chmod(&file.to_string_lossy(), 0o600)
    })();
    if let Err(err) = done {
        tracing::warn!(error = %err, "Carnet read not written in its journal");
    }
}

/// Page `id` read by `agent` in `turn` (`how`: read, search, record): one line in the journal per page and per turn.
#[allow(clippy::too_many_arguments)]
pub fn log(config: &Config, agent: &str, turn: &Turn, part: Part, id: &str, section: Option<&str>, how: &str) {
    if id.is_empty() {
        return;
    }
    let now = py::time_time();
    let first = seen(agent, turn, now, |s| s.logged.insert(id.to_string()));
    if !first {
        return;
    }
    let mut entry = Dict::new();
    entry.insert("at".into(), json!(at_ms(now)));
    entry.insert("agent".into(), json!(agent));
    entry.insert("session".into(), json!(turn.session));
    entry.insert("run".into(), json!(turn.run));
    entry.insert("page_id".into(), json!(id));
    if let Some(section) = section.filter(|s| !s.is_empty()) {
        entry.insert("section".into(), json!(section));
    }
    entry.insert("how".into(), json!(how));
    if part == Part::Health {
        entry.insert("part".into(), json!(part.as_str()));
    }
    append(config, &entry);
}

/// The Carnet's news given to `agent` at the start of a turn (news.rs): their pages, `how: news`.
pub fn log_news(config: &Config, agent: &str, pages: &[(Part, String)]) {
    let now = py::time_time();
    let mut done: HashSet<String> = HashSet::new();
    for (part, path) in pages {
        let Some(text) = super::read_page_text(config, *part, path) else { continue };
        let id = super::Page::parse(&text, "").id();
        if id.is_empty() || !done.insert(id.clone()) {
            continue;
        }
        let mut entry = Dict::new();
        entry.insert("at".into(), json!(at_ms(now)));
        entry.insert("agent".into(), json!(agent));
        entry.insert("session".into(), Value::Null);
        entry.insert("run".into(), Value::Null);
        entry.insert("page_id".into(), json!(id));
        entry.insert("how".into(), json!("news"));
        if *part == Part::Health {
            entry.insert("part".into(), json!(part.as_str()));
        }
        append(config, &entry);
    }
}

/// The journal's lines, oldest first.
pub fn entries(config: &Config) -> Vec<Dict> {
    tk::read_text(&path(config))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| match serde_json::from_str::<Value>(l) {
            Ok(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect()
}

/// The nightly pass: lines older than 90 days dropped. Returns how many.
pub fn purge(config: &Config, now: f64) -> std::io::Result<usize> {
    let file = path(config);
    let Ok(text) = tk::read_text(&file) else { return Ok(0) };
    let mut kept: Vec<&str> = Vec::new();
    let mut gone = 0;
    for line in text.lines() {
        let at = serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("at").and_then(Value::as_f64));
        match at {
            Some(at) if now - at <= KEPT_DAYS * 86400.0 => kept.push(line),
            _ => gone += 1,
        }
    }
    if gone > 0 {
        let body = if kept.is_empty() { String::new() } else { kept.join("\n") + "\n" };
        let temporary = file.with_extension("tmp");
        py::write_with_mode(&temporary.to_string_lossy(), body.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &file)?;
    }
    Ok(gone)
}

/// « Oublie ça » of a whole page (forget.rs): its reads dropped from the journal. Returns how many.
pub fn drop_page(config: &Config, id: &str) -> std::io::Result<usize> {
    let file = path(config);
    let Ok(text) = tk::read_text(&file) else { return Ok(0) };
    let mut kept: Vec<&str> = Vec::new();
    let mut gone = 0;
    for line in text.lines() {
        let page = serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("page_id").and_then(Value::as_str).map(str::to_string));
        if page.as_deref() == Some(id) {
            gone += 1;
        } else {
            kept.push(line);
        }
    }
    if gone > 0 {
        let body = if kept.is_empty() { String::new() } else { kept.join("\n") + "\n" };
        let temporary = file.with_extension("tmp");
        py::write_with_mode(&temporary.to_string_lossy(), body.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &file)?;
    }
    Ok(gone)
}

// -- the line under the answer ---------------------------------------------------------------------------------------

/// A turn of a conversation: its first item's id, its Hermes session, from when to when.
#[derive(Debug, Clone)]
pub struct Span {
    pub turn: String,
    pub session: String,
    pub from: f64,
    pub to: f64,
}

struct Texts {
    page: &'static str,
    pages: &'static str,
    noted: &'static str,
    all: &'static str,
    gone: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { page: "A consulté 1 page", pages: "A consulté {n} pages", noted: "Noté dans {names}", all: "Tout voir", gone: "Page oubliée" },
        "de" => Texts { page: "1 Seite gelesen", pages: "{n} Seiten gelesen", noted: "Notiert in {names}", all: "Alle ansehen", gone: "Vergessene Seite" },
        "es" => Texts { page: "Consultó 1 página", pages: "Consultó {n} páginas", noted: "Anotado en {names}", all: "Ver todo", gone: "Página olvidada" },
        _ => Texts { page: "Read 1 page", pages: "Read {n} pages", noted: "Noted in {names}", all: "See all", gone: "Forgotten page" },
    }
}

/// A page as the line names it: a person by their first name, the person's own page « Toi », else its title.
pub(crate) fn short(page: &super::Page, lang: &str) -> String {
    match page.kind().as_str() {
        "self" => super::words(lang).me.to_string(),
        "person" => page.title.split_whitespace().next().unwrap_or(&page.title).to_string(),
        _ => page.title.clone(),
    }
}

fn at_of(entry: &Dict) -> f64 {
    match entry.get("at") {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => chrono::DateTime::parse_from_rfc3339(s).map(|d| d.timestamp() as f64).unwrap_or(0.0),
        _ => 0.0,
    }
}

/// The actions of `log.jsonl` that are an agent's own writing (not a move, a merge or the nightly pass).
const NOTED: [&str; 9] = ["create", "note", "correct", "section", "brief", "trip", "task_add", "record_add", "record_update"];

/// The `carnet_line` item's body of each turn that read or wrote in the Carnet (contract.md §4), by turn; nothing
/// for a turn without. `lang`: the person's (the agent's language).
pub fn lines(config: &Config, agent: &str, lang: &str, spans: &[Span]) -> HashMap<String, Value> {
    let mut out = HashMap::new();
    if spans.is_empty() || !super::root(config).is_dir() {
        return out;
    }
    let (from, to) = spans.iter().fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(s.from), b.max(s.to)));
    let in_window = |e: &Dict| {
        let at = at_of(e);
        at >= from - 1.0 && at <= to + 1.0 && tk::str_or_empty(e.get("agent")) == agent
    };
    let reads: Vec<Dict> = entries(config).into_iter().filter(|e| in_window(e)).collect();
    let writes: Vec<Dict> = super::history(config, None, None, usize::MAX)
        .into_iter()
        .filter(|e| in_window(e) && tk::str_or_empty(e.get("by")) == agent)
        .filter(|e| NOTED.contains(&tk::str_or_empty(e.get("action")).as_str()))
        .collect();
    if reads.is_empty() && writes.is_empty() {
        return out;
    }
    let t = texts(lang);
    let by_id: HashMap<String, (Part, String, super::Page)> = [Part::Main, Part::Health]
        .into_iter()
        .flat_map(|part| super::all_pages(config, part).into_iter().map(move |(p, page)| (page.id(), (part, p, page))))
        .filter(|(id, _)| !id.is_empty())
        .collect();
    let health_word = super::words(lang).health;
    let show_noted = super::mentions(config);
    // each line in one turn: the latest of its session that started before it (a second of slack for the time the
    // message's row was written)
    let owner = |e: &Dict| -> Option<usize> {
        let (at, session) = (at_of(e), tk::str_or_empty(e.get("session")));
        spans.iter().rposition(|s| s.session == session && at >= s.from - 1.0 && at < s.to)
    };
    for (index, span) in spans.iter().enumerate() {
        let inside = |e: &Dict| owner(e) == Some(index);
        let mut pages: Vec<Value> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for e in reads.iter().filter(|e| inside(e)) {
            let id = tk::str_or_empty(e.get("page_id"));
            if ids.contains(&id) {
                continue;
            }
            ids.push(id.clone());
            let section = tk::str_or_empty(e.get("section"));
            let view = match by_id.get(&id) {
                Some((Part::Health, _, _)) => json!({"title": health_word, "path": null, "locked": true}),
                Some((_, path, page)) => {
                    let mut title = short(page, lang);
                    let kind = page.kind();
                    if !section.is_empty() && super::template::outline(&kind).iter().any(|s| s.id == section) {
                        title = format!("{title} · {}", super::template::title(&kind, &section, lang));
                    }
                    let mut v = json!({"title": title, "path": path});
                    if !section.is_empty() {
                        v["section"] = json!(section);
                    }
                    v
                }
                None if tk::str_or_empty(e.get("part")) == Part::Health.as_str() => {
                    json!({"title": health_word, "path": null, "locked": true})
                }
                None => json!({"title": t.gone, "path": null}),
            };
            pages.push(view);
        }
        let mut noted: Vec<Value> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for e in writes.iter().filter(|e| inside(e)) {
            let path = tk::str_or_empty(e.get("path"));
            if noted.iter().any(|n| n["path"] == json!(path)) {
                continue;
            }
            let health = tk::str_or_empty(e.get("part")) == Part::Health.as_str();
            let found = by_id.values().find(|(part, p, _)| *p == path && (*part == Part::Health) == health);
            let name = match found {
                _ if health => health_word.to_string(),
                Some((_, _, page)) => short(page, lang),
                None => tk::str_or_empty(e.get("title")),
            };
            if name.is_empty() {
                continue;
            }
            names.push(name.clone());
            noted.push(json!({"title": name, "path": if health { Value::Null } else { json!(path) },
                              "change_id": tk::str_or_empty(e.get("id"))}));
        }
        if !show_noted {
            noted.clear();
            names.clear();
        }
        let mut parts: Vec<String> = Vec::new();
        match pages.len() {
            0 => {}
            1 => parts.push(t.page.to_string()),
            n => parts.push(t.pages.replace("{n}", &n.to_string())),
        }
        if !names.is_empty() {
            let shown = if names.len() > 2 {
                format!("{} +{}", names[..2].join(", "), names.len() - 2)
            } else {
                super::news::names_text(&names, lang)
            };
            parts.push(t.noted.replace("{names}", &shown));
        }
        if parts.is_empty() {
            continue;
        }
        out.insert(
            span.turn.clone(),
            json!({"kind": "carnet_line", "text": parts.join(" · "), "pages": pages, "noted": noted, "all": t.all}),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_reads_six_pages_at_most_and_its_reads_are_logged_once() {
        let turn = Turn { run: Some("run_cap".into()), session: Some("s1".into()), cap: PAGES_PER_TURN };
        for i in 0..6 {
            assert!(may_read("cap-agent", &turn, &format!("p{i}")));
        }
        assert!(may_read("cap-agent", &turn, "p0"), "a page read again is no new page");
        assert!(!may_read("cap-agent", &turn, "p6"));
        let fill = Turn { run: Some("run_fill".into()), session: None, cap: PAGES_PER_FILL };
        assert!((0..20).all(|i| may_read("cap-agent", &fill, &format!("p{i}"))));
        assert!(!may_read("cap-agent", &fill, "p20"));
        let other = Turn { run: Some("run_other".into()), ..turn.clone() };
        assert!(may_read("cap-agent", &other, "p6"), "another turn has its own pages");
    }

    #[test]
    fn the_journal_keeps_ninety_days() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::notebook::tests_config(dir.path());
        let turn = Turn { run: Some("run_j".into()), session: Some("s".into()), cap: 6 };
        log(&config, "lea", &turn, Part::Main, "p1", Some("tastes"), "read");
        log(&config, "lea", &turn, Part::Main, "p1", None, "read");
        log(&config, "lea", &turn, Part::Health, "p2", None, "search");
        let lines = entries(&config);
        assert_eq!(lines.len(), 2, "one line per page and per turn");
        assert_eq!(lines[0]["section"], "tastes");
        assert_eq!(lines[1]["part"], "health");
        assert!(lines.iter().all(|l| l.keys().all(|k| ["at", "agent", "session", "run", "page_id", "section", "how", "part"].contains(&k.as_str()))));
        let now = py::time_time();
        assert_eq!(purge(&config, now).unwrap(), 0);
        assert_eq!(purge(&config, now + 91.0 * 86400.0).unwrap(), 2);
        assert!(entries(&config).is_empty());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(path(&config)).unwrap().permissions().mode() & 0o777, 0o600);
    }
}
