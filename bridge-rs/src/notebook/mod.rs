//! The « Carnet » (EN Notebook, DE Notizbuch, ES Cuaderno), decided 2026-10-22: everything the person told their agents,
//! organised, kept up to date by the agents themselves and readable in the app and outside it (Obsidian). One Carnet
//! per install (per person), shared by all their agents: a person's page gathers what every agent knows. Agents reach
//! it only through their tools (tools.rs), never from their sandbox; the apps through `/v1/notebook…` (api.rs); a
//! nightly « rangement » files what the agents missed with a small model, under a hard monthly cap (tidy.rs).
//!
//! On disk, under the bridge's data dir (plain Markdown, Obsidian-compatible: YAML frontmatter, `[[links]]`, tasks as
//! checkbox lines with a 📅 date, Obsidian Tasks' format):
//!
//!     notebook/carnet/                   the vault (the Obsidian export is this folder)
//!         Personnes/Marc Dupont.md       one page per person (People, Personen, Personas)
//!         Sujets/Cuisine.md              projects and subjects (Subjects, Themen, Temas)
//!         Domaines/Maison.md             areas of life (Areas, Bereiche, Áreas)
//!         Tâches/Tâches.md               the dated tasks (Tasks, Aufgaben, Tareas)
//!         Documents.md                   the index of the files the agents made or received (written, not kept)
//!         À ranger.md                    what waits for the nightly pass (the agents' memories, at first)
//!     notebook/private/                  the private health part: the same layout, read only by the agents the person
//!                                        gave their health to (health.rs) and by the person (the app)
//!     notebook/history/log.jsonl         every change: who (an agent, « you », the nightly pass), when, which page
//!     notebook/history/versions/<h>.md   each page's text before and after each change (by content hash)
//!     notebook/state.json                settings (language, mentions, nightly), agents with it off, the nightly
//!                                        pass's cursors, spending and runs, the tasks' reminder links
//!
//! Each fact is a list line ending with its attribution, readable as it is and clean in Obsidian:
//! `- Anniversaire : 12 mars — *Léa, 2026-10-22*`. History is kept by the bridge (versions by content hash plus a log)
//! rather than in a git repository: nothing to install or configure on a minimal server, one write per change, no
//! repository to repair after a crash or a full disk, and the vault exported to Obsidian stays a plain folder. A
//! restore is itself a change, so it can be undone too.
//!
//! No Python counterpart (owner group: tools-knowledge).

pub mod api;
pub mod tidy;
pub mod tools;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// One writer at a time (the tools run in threads, the routes and the nightly pass too). Taken before the reminders'
/// lock when both are needed, never after.
static LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> std::sync::MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A message for the agent or the app (a 400), not a failure of the bridge.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct NotebookError(pub String);

fn refuse<T>(message: impl Into<String>) -> anyhow::Result<T> {
    Err(NotebookError(message.into()).into())
}

// -- words ---------------------------------------------------------------------------------------------------------

/// The Carnet's words in the install's language: folder and page names (what the person sees in Obsidian).
pub struct Words {
    pub lang: &'static str,
    pub people: &'static str,
    pub subjects: &'static str,
    pub areas: &'static str,
    pub tasks: &'static str,
    pub inbox: &'static str,
    pub documents: &'static str,
    /// The person, in an attribution (« — *toi, 2026-10-22* »).
    pub you: &'static str,
    /// A fact's section when the agent gives none.
    pub notes: &'static str,
    /// The health area's page (always in the private part).
    pub health: &'static str,
    /// The private part's folder in an export that includes it.
    pub private: &'static str,
    /// The short summary every page starts with (its first section).
    pub brief: &'static str,
}

pub const WORDS: [Words; 4] = [
    Words {
        lang: "fr",
        people: "Personnes",
        subjects: "Sujets",
        areas: "Domaines",
        tasks: "Tâches",
        inbox: "À ranger",
        documents: "Documents",
        you: "toi",
        notes: "Infos",
        health: "Santé",
        private: "Santé (privé)",
        brief: "En bref",
    },
    Words {
        lang: "en",
        people: "People",
        subjects: "Subjects",
        areas: "Areas",
        tasks: "Tasks",
        inbox: "To sort",
        documents: "Documents",
        you: "you",
        notes: "Notes",
        health: "Health",
        private: "Health (private)",
        brief: "In brief",
    },
    Words {
        lang: "de",
        people: "Personen",
        subjects: "Themen",
        areas: "Bereiche",
        tasks: "Aufgaben",
        inbox: "Zu sortieren",
        documents: "Dokumente",
        you: "du",
        notes: "Notizen",
        health: "Gesundheit",
        private: "Gesundheit (privat)",
        brief: "Kurz gesagt",
    },
    Words {
        lang: "es",
        people: "Personas",
        subjects: "Temas",
        areas: "Áreas",
        tasks: "Tareas",
        inbox: "Por ordenar",
        documents: "Documentos",
        you: "tú",
        notes: "Notas",
        health: "Salud",
        private: "Salud (privado)",
        brief: "En resumen",
    },
];

pub fn words(lang: &str) -> &'static Words {
    WORDS.iter().find(|w| w.lang == lang).unwrap_or(&WORDS[1])
}

/// The kinds of pages an agent writes, in the frontmatter's `type`.
pub const KINDS: [&str; 3] = ["person", "subject", "area"];

impl Words {
    pub fn folder(&self, kind: &str) -> Option<&'static str> {
        match kind {
            "person" => Some(self.people),
            "subject" => Some(self.subjects),
            "area" => Some(self.areas),
            "task" | "tasks" => Some(self.tasks),
            _ => None,
        }
    }

    pub fn kind_of(&self, folder: &str) -> Option<&'static str> {
        [("person", self.people), ("subject", self.subjects), ("area", self.areas), ("tasks", self.tasks)]
            .into_iter()
            .find(|(_, f)| *f == folder)
            .map(|(k, _)| k)
    }

    /// `Tâches/Tâches.md`.
    pub fn tasks_path(&self) -> String {
        format!("{0}/{0}.md", self.tasks)
    }

    pub fn inbox_path(&self) -> String {
        format!("{}.md", self.inbox)
    }
}

// -- where it lives ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    Main,
    Health,
}

impl Part {
    pub fn as_str(self) -> &'static str {
        match self {
            Part::Main => "main",
            Part::Health => "health",
        }
    }

    pub fn parse(text: &str) -> Part {
        if text == "health" {
            Part::Health
        } else {
            Part::Main
        }
    }
}

pub fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("notebook")
}

pub fn part_root(config: &Config, part: Part) -> PathBuf {
    root(config).join(match part {
        Part::Main => "carnet",
        Part::Health => "private",
    })
}

fn history_dir(config: &Config) -> PathBuf {
    root(config).join("history")
}

/// A relative path inside a part (`Personnes/Marc Dupont.md`), checked: plain names only, a Markdown file.
pub fn checked(relative: &str) -> anyhow::Result<PathBuf> {
    let relative = relative.trim().trim_start_matches('/');
    let path = Path::new(relative);
    let plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    let depth = path.components().count();
    if relative.is_empty() || !plain || depth > 2 || !relative.ends_with(".md") || relative.contains('\0') {
        return refuse(format!("not a page of the Carnet: {relative}"));
    }
    Ok(path.to_path_buf())
}

fn write_file(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("md.part");
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, path)
}

pub(crate) fn read_page_text(config: &Config, part: Part, relative: &str) -> Option<String> {
    let path = checked(relative).ok()?;
    tk::read_text(&part_root(config, part).join(path)).ok()
}

// -- state ---------------------------------------------------------------------------------------------------------

fn state_path(config: &Config) -> PathBuf {
    root(config).join("state.json")
}

pub fn read_state(config: &Config) -> Dict {
    match tk::read_text(&state_path(config)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

pub fn write_state(config: &Config, state: &Dict) -> std::io::Result<()> {
    let path = state_path(config);
    std::fs::create_dir_all(root(config))?;
    let temporary = path.with_extension("tmp");
    py::write_with_mode(
        &temporary.to_string_lossy(),
        py::Dumps::new().unicode().dumps(&Value::Object(state.clone())).as_bytes(),
        0o600,
    )?;
    std::fs::rename(&temporary, &path)
}

/// Changes `state.json` under the lock the caller holds.
pub fn update_state(config: &Config, change: impl FnOnce(&mut Dict)) -> std::io::Result<Dict> {
    let mut state = read_state(config);
    change(&mut state);
    write_state(config, &state)?;
    Ok(state)
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// The Carnet's language: set once (its folders are named in it), from the first agent's language, else English.
pub fn language(config: &Config) -> String {
    let state = read_state(config);
    if let Some(lang) = state.get("language").and_then(Value::as_str).filter(|l| WORDS.iter().any(|w| w.lang == *l)) {
        return lang.to_string();
    }
    for agent in config.agents.values() {
        if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
            continue;
        }
        let Ok(soul) = crate::agents_admin::read_soul(config, agent) else { continue };
        if let (_, Some(lang)) = crate::souls::split_language(&soul) {
            return lang;
        }
    }
    let browser = crate::souls::agent_language(Some(&config.browser.language));
    if config.browser.language.is_empty() {
        "en".into()
    } else {
        browser
    }
}

/// The language fixed at the first write (the folders keep their names).
fn fix_language(config: &Config) -> std::io::Result<String> {
    let lang = language(config);
    if read_state(config).get("language").and_then(Value::as_str) != Some(lang.as_str()) {
        update_state(config, |s| {
            s.insert("language".into(), json!(lang));
        })?;
    }
    Ok(lang)
}

/// Whether the agents tell the person in one line when they file something (on by default; the app's switch).
pub fn mentions(config: &Config) -> bool {
    read_state(config).get("mentions").map(py::truthy).unwrap_or(true)
}

pub const NIGHTLY: [&str; 3] = ["nightly", "weekly", "never"];

/// How often the nightly pass runs: « nightly » (the default), « weekly » or « never ».
pub fn nightly(config: &Config) -> String {
    match read_state(config).get("nightly").and_then(Value::as_str) {
        Some(n) if NIGHTLY.contains(&n) => n.to_string(),
        _ => "nightly".into(),
    }
}

/// Whether `agent` has the Carnet's tools (on by default, at creation and for the agents made before it).
pub fn enabled(config: &Config, agent: &str) -> bool {
    !strings(read_state(config).get("off")).iter().any(|a| a == agent)
}

/// The switch of `agent`. Returns whether it changed.
pub fn set_enabled(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let _guard = lock();
    let mut off = strings(read_state(config).get("off"));
    let before = off.clone();
    off.retain(|a| a != agent);
    if !on {
        off.push(agent.to_string());
    }
    if off == before {
        return Ok(false);
    }
    update_state(config, |s| {
        s.insert("off".into(), json!(off));
    })?;
    Ok(true)
}

/// A deleted agent: its switch and cursors go (what it wrote stays: it is the person's).
pub fn forget(config: &Config, agent: &str) {
    let _guard = lock();
    if !state_path(config).exists() {
        return;
    }
    let result = update_state(config, |s| {
        let off: Vec<String> = strings(s.get("off")).into_iter().filter(|a| a != agent).collect();
        s.insert("off".into(), json!(off));
        if let Some(Value::Object(cursors)) = s.get_mut("cursors") {
            cursors.remove(agent);
        }
    });
    if let Err(err) = result {
        tracing::warn!(agent = %agent, error = %err, "notebook switch not removed");
    }
}

/// The agent's personality has the Carnet's rule exactly when its tools are on (an agent set up by hand is left as
/// it is). Best effort: called whenever its tools are registered (start, creation, a switch).
pub fn sync_soul(config: &Config, agent: &crate::config::AgentConfig) {
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        return;
    }
    let path = crate::agents_admin::profile_folder(config, agent).join("SOUL.md");
    let Ok(soul) = std::fs::read_to_string(&path) else { return };
    let changed = crate::souls::with_notebook(&soul, enabled(config, &agent.name));
    if changed != soul {
        if let Err(err) = std::fs::write(&path, changed) {
            tracing::warn!(agent = %agent.name, error = %err, "notebook rule not written");
        }
    }
}

/// Whether `agent` may read and write the private health part: the person gave it their health (health.rs).
pub fn health_agent(config: &Config, agent: &str) -> bool {
    crate::health::enabled(config, agent)
}

// -- who -----------------------------------------------------------------------------------------------------------

/// Who made a change: an agent (`by` its id), the person (`you`), or the nightly pass on an agent's behalf.
#[derive(Debug, Clone)]
pub struct Who {
    /// An agent's id, « you » or « nightly ».
    pub by: String,
    /// The name in the attribution (the agent's name, « toi »…).
    pub name: String,
    /// The agent the fact came from (the nightly pass files what an agent heard).
    pub agent: Option<String>,
}

impl Who {
    pub fn agent(config: &Config, name: &str) -> Who {
        let shown = config.agent(Some(name)).map(|a| a.display_name.clone()).unwrap_or_else(|| name.to_string());
        Who { by: name.to_string(), name: shown, agent: Some(name.to_string()) }
    }

    pub fn you(config: &Config) -> Who {
        Who { by: "you".into(), name: words(&language(config)).you.into(), agent: None }
    }

    pub fn nightly(config: &Config, agent: &str) -> Who {
        let mut who = Who::agent(config, agent);
        who.by = "nightly".into();
        who
    }
}

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

// -- pages ---------------------------------------------------------------------------------------------------------

/// A page: its frontmatter, its title (the `# ` line, else the file's name) and the rest.
#[derive(Debug, Clone, Default)]
pub struct Page {
    pub front: Dict,
    pub title: String,
    pub body: String,
}

static ATTRIBUTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*?)\s+— \*([^*,]{1,80}), (\d{4}-\d{2}-\d{2})\*\s*$").expect("valid regex"));
static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]|#]+)(?:[#|][^\]]*)?\]\]").expect("valid regex"));

/// « Anniversaire : 12 mars — *Léa, 2026-10-22* » → (the fact, Some((who, date))).
pub fn split_attribution(line: &str) -> (String, Option<(String, String)>) {
    match ATTRIBUTION.captures(line) {
        Some(c) => (c[1].trim().to_string(), Some((c[2].trim().to_string(), c[3].to_string()))),
        None => (line.trim().to_string(), None),
    }
}

pub fn attribution(who: &Who, date: &str) -> String {
    format!(" — *{}, {date}*", who.name.replace(['*', ','], " ").trim())
}

/// The `[[links]]` in a text, in order, each once.
pub fn links_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in LINK.captures_iter(text) {
        let name = c[1].trim().to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Lower-case, accents and punctuation folded, spaces squeezed: what « the same name » or « the same fact » means.
pub fn fold(text: &str) -> String {
    let plain = tk::ascii_fold(&text.to_lowercase());
    let spaced: String = plain.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' }).collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A page's file name from its title: what no file system or Obsidian link accepts taken out.
pub fn file_name(title: &str) -> String {
    let cleaned: String =
        title.chars().map(|c| if "/\\:*?\"<>|#^[]\n\r\t".contains(c) || c.is_control() { ' ' } else { c }).collect();
    let squeezed = tk::squeeze(&cleaned);
    let name = tk::head(squeezed.trim_matches(['.', ' ']), 80).trim().to_string();
    if name.is_empty() {
        "Page".into()
    } else {
        name
    }
}

/// A YAML scalar: plain when that reads back as the same string, else JSON-quoted (valid YAML).
fn yaml_scalar(text: &str) -> String {
    let plain = !text.is_empty()
        && text.chars().all(|c| c.is_alphanumeric() || " -_.()'’".contains(c))
        && !text.starts_with(['-', ' ', '\'', '.'])
        && !text.ends_with(' ')
        && text.parse::<f64>().is_err()
        && !["true", "false", "yes", "no", "null", "on", "off", "~"].contains(&text.to_lowercase().as_str());
    if plain {
        text.to_string()
    } else {
        py::dumps(&Value::String(text.to_string()))
    }
}

fn yaml_value(key: &str, value: &Value) -> String {
    match value {
        Value::Array(items) if items.is_empty() => format!("{key}: []\n"),
        Value::Array(items) => {
            let mut out = format!("{key}:\n");
            for item in items {
                let text = match item {
                    Value::String(s) => yaml_scalar(s),
                    other => py::dumps(other),
                };
                out.push_str(&format!("  - {text}\n"));
            }
            out
        }
        Value::String(s) if key == "updated" && s.len() == 10 => format!("{key}: {s}\n"),
        Value::String(s) => format!("{key}: {}\n", yaml_scalar(s)),
        Value::Null => format!("{key}:\n"),
        other => format!("{key}: {}\n", py::dumps(other)),
    }
}

impl Page {
    pub fn new(kind: &str, title: &str) -> Page {
        let mut front = Dict::new();
        front.insert("type".into(), json!(kind));
        front.insert("aliases".into(), json!([]));
        Page { front, title: title.to_string(), body: String::new() }
    }

    /// A page's text. `fallback`: the title when it has no `# ` line (the file's name).
    pub fn parse(text: &str, fallback: &str) -> Page {
        let mut front = Dict::new();
        let mut rest = text;
        if let Some(after) = text.strip_prefix("---\n") {
            if let Some(end) = after.find("\n---\n").map(|i| (i, i + 5)).or_else(|| {
                after.strip_suffix("\n---").map(|a| (a.len(), after.len())).or(if after.starts_with("---\n") {
                    Some((0, 4))
                } else {
                    None
                })
            }) {
                let yaml = &after[..end.0];
                if let Ok(Value::Object(map)) = serde_yaml::from_str::<Value>(yaml) {
                    front = map;
                }
                rest = &after[end.1..];
            }
        }
        let rest = rest.trim_start_matches('\n');
        let (title, body) = match rest.strip_prefix("# ") {
            Some(after) => {
                let (line, body) = after.split_once('\n').unwrap_or((after, ""));
                (line.trim().to_string(), body.trim_start_matches('\n').to_string())
            }
            None => (fallback.to_string(), rest.to_string()),
        };
        // dates written plain come back as strings: serde_yaml has no date type
        Page { front, title, body }
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.front.is_empty() {
            out.push_str("---\n");
            for (key, value) in &self.front {
                out.push_str(&yaml_value(key, value));
            }
            out.push_str("---\n");
        }
        out.push_str(&format!("# {}\n", self.title));
        let body = self.body.trim_matches('\n');
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        out
    }

    pub fn kind(&self) -> String {
        tk::str_or_empty(self.front.get("type"))
    }

    pub fn aliases(&self) -> Vec<String> {
        match self.front.get("aliases") {
            Some(Value::Array(items)) => items.iter().map(py::py_str).filter(|a| !a.trim().is_empty()).collect(),
            Some(Value::String(one)) if !one.trim().is_empty() => vec![one.clone()],
            _ => Vec::new(),
        }
    }

    pub fn contact(&self) -> String {
        match self.front.get("contact") {
            Some(Value::Null) | None => String::new(),
            Some(v) => py::py_str(v),
        }
    }

    pub fn set_aliases(&mut self, aliases: &[String]) {
        let mut kept: Vec<String> = Vec::new();
        for a in aliases {
            let a = tk::head(&tk::squeeze(a), 80).to_string();
            if !a.is_empty() && fold(&a) != fold(&self.title) && !kept.iter().any(|k| fold(k) == fold(&a)) {
                kept.push(a);
            }
        }
        self.front.insert("aliases".into(), json!(kept));
    }

    pub fn touch(&mut self) {
        self.front.insert("updated".into(), json!(today()));
    }

    /// Whether `query` names this page: its title, an alias or its contact id.
    pub fn named(&self, query: &str) -> bool {
        let wanted = fold(query);
        !wanted.is_empty()
            && (fold(&self.title) == wanted
                || self.aliases().iter().any(|a| fold(a) == wanted)
                || (!self.contact().is_empty() && self.contact() == query.trim()))
    }

    /// The facts, by section: (section, [(fact, who, date)]); lines before any section under « ».
    pub fn sections(&self) -> Vec<(String, Vec<Fact>)> {
        let mut out: Vec<(String, Vec<Fact>)> = vec![(String::new(), Vec::new())];
        for line in self.body.lines() {
            if let Some(name) = line.strip_prefix("## ") {
                out.push((name.trim().to_string(), Vec::new()));
                continue;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let item = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")).unwrap_or(trimmed);
            let (text, by) = split_attribution(item);
            let (who, date) = by.unwrap_or_default();
            if let Some(last) = out.last_mut() {
                last.1.push(Fact { text, who, date, item: trimmed.starts_with("- ") || trimmed.starts_with("* ") });
            }
        }
        if out[0].1.is_empty() {
            out.remove(0);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub text: String,
    pub who: String,
    pub date: String,
    /// A list line (a fact), not a paragraph.
    pub item: bool,
}

/// The section's line index in `lines` (`## name`, compared folded).
fn section_index(lines: &[String], section: &str) -> Option<usize> {
    let wanted = fold(section);
    lines.iter().position(|l| l.strip_prefix("## ").map(|n| fold(n) == wanted).unwrap_or(false))
}

fn section_end(lines: &[String], start: usize) -> usize {
    lines[start + 1..]
        .iter()
        .position(|l| l.starts_with("## ") || l.starts_with("# "))
        .map(|i| start + 1 + i)
        .unwrap_or(lines.len())
}

fn body_lines(body: &str) -> Vec<String> {
    body.trim_matches('\n').lines().map(str::to_string).collect()
}

/// The facts not already there (folded, without their attribution) added at the end of `section` (made if missing).
/// Returns the new body and the facts added.
pub fn add_facts(body: &str, section: &str, facts: &[String], who: &Who, date: &str) -> (String, Vec<String>) {
    let mut lines = body_lines(body);
    let known: HashSet<String> = lines
        .iter()
        .map(|l| {
            let item = l.trim().strip_prefix("- ").unwrap_or(l.trim());
            fold(&split_attribution(item).0)
        })
        .collect();
    let mut added: Vec<String> = Vec::new();
    for fact in facts {
        let fact = tk::squeeze(fact.trim().trim_start_matches("- ").trim());
        let fact = tk::head(&fact, 600).to_string();
        let folded = fold(&fact);
        if folded.is_empty() || known.contains(&folded) || added.iter().any(|a| fold(a) == folded) {
            continue;
        }
        added.push(fact);
    }
    if added.is_empty() {
        return (body.to_string(), added);
    }
    let new_lines: Vec<String> = added.iter().map(|f| format!("- {f}{}", attribution(who, date))).collect();
    match section_index(&lines, section) {
        Some(start) => {
            let end = section_end(&lines, start);
            let mut at = end;
            while at > start + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            for (n, line) in new_lines.into_iter().enumerate() {
                lines.insert(at + n, line);
            }
        }
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("## {}", tk::head(&tk::squeeze(section), 80)));
            lines.extend(new_lines);
        }
    }
    (lines.join("\n") + "\n", added)
}

/// The first fact containing `old` (folded) replaced by `new` with a new attribution, or removed when `new` is empty.
/// None when no fact matches.
pub fn correct_fact(body: &str, old: &str, new: &str, who: &Who, date: &str) -> Option<(String, String)> {
    let wanted = fold(old);
    if wanted.is_empty() {
        return None;
    }
    let mut lines = body_lines(body);
    let index = lines.iter().position(|l| {
        !l.starts_with('#') && {
            let item = l.trim().strip_prefix("- ").unwrap_or(l.trim());
            let (text, _) = split_attribution(item);
            fold(&text).contains(&wanted)
        }
    })?;
    let was = {
        let item = lines[index].trim().strip_prefix("- ").unwrap_or(lines[index].trim()).to_string();
        split_attribution(&item).0
    };
    let new = tk::head(&tk::squeeze(new.trim().trim_start_matches("- ")), 600).to_string();
    if new.is_empty() {
        lines.remove(index);
    } else {
        lines[index] = format!("- {new}{}", attribution(who, date));
    }
    Some((lines.join("\n") + "\n", was))
}

/// `section`'s content replaced by `text` (made if missing; an empty text removes it). List lines without an
/// attribution get one.
pub fn set_section(body: &str, section: &str, text: &str, who: &Who, date: &str) -> String {
    let mut lines = body_lines(body);
    let new: Vec<String> = text
        .trim_matches('\n')
        .lines()
        .map(|l| {
            let trimmed = l.trim_end();
            if let Some(item) = trimmed.strip_prefix("- ") {
                if split_attribution(item).1.is_none() && !item.trim().is_empty() {
                    return format!("- {}{}", item.trim(), attribution(who, date));
                }
            }
            trimmed.to_string()
        })
        .collect();
    match section_index(&lines, section) {
        Some(start) => {
            let end = section_end(&lines, start);
            let mut replacement: Vec<String> = Vec::new();
            if !new.is_empty() {
                replacement.push(lines[start].clone());
                replacement.extend(new);
                if end < lines.len() {
                    replacement.push(String::new());
                }
            }
            lines.splice(start..end, replacement);
        }
        None if !new.is_empty() => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push(format!("## {}", tk::head(&tk::squeeze(section), 80)));
            lines.extend(new);
        }
        None => {}
    }
    lines.join("\n") + "\n"
}

/// The page's « En bref » (its summary: the section named `brief`, first on the page), as plain text.
pub fn summary_of(body: &str, brief: &str) -> String {
    let lines = body_lines(body);
    let Some(start) = section_index(&lines, brief) else { return String::new() };
    let end = section_end(&lines, start);
    lines[start + 1..end].iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The body with its « En bref » set to `text` (2–3 short lines, no attribution: it is rewritten as facts change),
/// first on the page. An empty text leaves it as it was.
pub fn set_summary(body: &str, brief: &str, text: &str) -> String {
    let text: Vec<String> = text
        .trim()
        .lines()
        .map(|l| tk::squeeze(l.trim()))
        .filter(|l| !l.is_empty())
        .take(4)
        .map(|l| tk::head(&l, 300).to_string())
        .collect();
    if text.is_empty() {
        return body.to_string();
    }
    let mut lines = body_lines(body);
    if let Some(start) = section_index(&lines, brief) {
        let end = section_end(&lines, start);
        lines.drain(start..end);
        while lines.get(start).is_some_and(|l| l.trim().is_empty()) {
            lines.remove(start);
        }
    }
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    let mut out = vec![format!("## {brief}")];
    out.extend(text);
    if !lines.is_empty() {
        out.push(String::new());
    }
    out.extend(lines);
    out.join("\n") + "\n"
}

/// Every page of a part, (relative path, page), in path order. The tasks' page and the generated ones included.
pub fn all_pages(config: &Config, part: Part) -> Vec<(String, Page)> {
    let base = part_root(config, part);
    let mut found: Vec<(String, Page)> = Vec::new();
    let mut visit = |dir: &Path, prefix: &str| {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
            if is_link || !path.is_file() || !name.ends_with(".md") || name.starts_with('.') {
                continue;
            }
            let Ok(text) = tk::read_text(&path) else { continue };
            let stem = name.trim_end_matches(".md");
            found.push((format!("{prefix}{name}"), Page::parse(&text, stem)));
        }
    };
    visit(&base, "");
    for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let path = entry.path();
        let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
        if path.is_dir() && !is_link && !entry.file_name().to_string_lossy().starts_with('.') {
            let folder = entry.file_name().to_string_lossy().into_owned();
            visit(&path, &format!("{folder}/"));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// A page found by its path, title, alias or contact id, in the given parts (in order).
#[derive(Debug, Clone)]
pub struct Found {
    pub part: Part,
    pub path: String,
    pub page: Page,
}

pub fn find(config: &Config, parts: &[Part], query: &str) -> Option<Found> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    for part in parts {
        if query.ends_with(".md") {
            if let Some(text) = read_page_text(config, *part, query) {
                let stem = Path::new(query).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                return Some(Found {
                    part: *part,
                    path: query.trim_start_matches('/').to_string(),
                    page: Page::parse(&text, &stem),
                });
            }
        }
    }
    for part in parts {
        for (path, page) in all_pages(config, *part) {
            if page.named(query) {
                return Some(Found { part: *part, path, page });
            }
        }
    }
    None
}

// -- history -------------------------------------------------------------------------------------------------------

fn version_id(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..20].to_string()
}

fn keep_version(config: &Config, text: &str) -> std::io::Result<String> {
    let id = version_id(text);
    let folder = history_dir(config).join("versions");
    let path = folder.join(format!("{id}.md"));
    if !path.exists() {
        std::fs::create_dir_all(&folder)?;
        let temporary = folder.join(format!(".{id}.part"));
        py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &path)?;
    }
    Ok(id)
}

pub fn version_text(config: &Config, id: &str) -> Option<String> {
    if id.len() != 20 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    tk::read_text(&history_dir(config).join("versions").join(format!("{id}.md"))).ok()
}

/// A change written: the page's new text (None: deleted), its versions kept, a line in the log. The caller holds
/// the lock. Returns the log's line.
#[allow(clippy::too_many_arguments)]
pub fn commit(
    config: &Config,
    part: Part,
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    who: &Who,
    action: &str,
    summary: &str,
) -> anyhow::Result<Dict> {
    let relative = checked(path)?;
    fix_language(config)?;
    let file = part_root(config, part).join(&relative);
    match after {
        Some(text) => write_file(&file, text)?,
        None => {
            if file.is_file() {
                std::fs::remove_file(&file)?;
            }
        }
    }
    let before_id = before.map(|t| keep_version(config, t)).transpose()?;
    let after_id = after.map(|t| keep_version(config, t)).transpose()?;
    let title = after
        .or(before)
        .map(|t| {
            Page::parse(t, Path::new(path).file_stem().map(|s| s.to_string_lossy()).as_deref().unwrap_or("")).title
        })
        .unwrap_or_default();
    let mut entry = Dict::new();
    entry.insert("id".into(), json!(format!("c{}", py::token_hex(6))));
    entry.insert("at".into(), json!(tk::now_iso_seconds()));
    entry.insert("by".into(), json!(who.by));
    entry.insert("name".into(), json!(who.name));
    entry.insert("agent".into(), json!(who.agent));
    entry.insert("part".into(), json!(part.as_str()));
    entry.insert("path".into(), json!(path));
    entry.insert("title".into(), json!(title));
    entry.insert("action".into(), json!(action));
    entry.insert("summary".into(), json!(tk::head(&tk::squeeze(summary), 300)));
    entry.insert("before".into(), json!(before_id));
    entry.insert("after".into(), json!(after_id));
    let log = history_dir(config).join("log.jsonl");
    std::fs::create_dir_all(history_dir(config))?;
    let mut fh = std::fs::OpenOptions::new().append(true).create(true).open(&log)?;
    fh.write_all((py::Dumps::new().unicode().dumps(&Value::Object(entry.clone())) + "\n").as_bytes())?;
    py::chmod(&log.to_string_lossy(), 0o600)?;
    Ok(entry)
}

/// The changes, the newest first: all of them, a part's, or one page's.
pub fn history(config: &Config, part: Option<Part>, path: Option<&str>, limit: usize) -> Vec<Dict> {
    let text = tk::read_text(&history_dir(config).join("log.jsonl")).unwrap_or_default();
    let mut out: Vec<Dict> = Vec::new();
    for line in text.lines().rev() {
        let Ok(Value::Object(entry)) = serde_json::from_str::<Value>(line) else { continue };
        if part.is_some_and(|p| entry.get("part").and_then(Value::as_str) != Some(p.as_str())) {
            continue;
        }
        if path.is_some_and(|p| entry.get("path").and_then(Value::as_str) != Some(p)) {
            continue;
        }
        out.push(entry);
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// The page as it was right after change `change` (as it was before it, for a deletion), written back as a new
/// change by `who`.
pub fn restore(config: &Config, change: &str, who: &Who) -> anyhow::Result<Dict> {
    let _guard = lock();
    let Some(entry) = history(config, None, None, usize::MAX)
        .into_iter()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(change))
    else {
        return refuse("no such change");
    };
    let version = match entry.get("after").and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => tk::str_or_empty(entry.get("before")),
    };
    let Some(text) = version_text(config, &version) else { return refuse("this version is no longer kept") };
    let part = Part::parse(&tk::str_or_empty(entry.get("part")));
    let path = tk::str_or_empty(entry.get("path"));
    let current = read_page_text(config, part, &path);
    if current.as_deref() == Some(text.as_str()) {
        return refuse("the page is already as it was then");
    }
    let date = tk::str_or_empty(entry.get("at"));
    commit(
        config,
        part,
        &path,
        current.as_deref(),
        Some(&text),
        who,
        "restore",
        &format!("as of {}", tk::head(&date, 10)),
    )
}

// -- what the agents and the person do -------------------------------------------------------------------------------

/// What a write did, for the tool's answer and the app.
#[derive(Debug, Clone, Default)]
pub struct Written {
    pub part: Option<Part>,
    pub path: String,
    pub title: String,
    pub created: bool,
    pub added: Vec<String>,
}

/// Facts filed about `title` (a person, a subject or an area): its page found by title, alias or contact id (in this
/// part), made if missing, the facts not there yet added to `section`. `aliases` and `contact` added to the page.
#[allow(clippy::too_many_arguments)]
pub fn note(
    config: &Config,
    part: Part,
    kind: &str,
    title: &str,
    section: &str,
    facts: &[String],
    aliases: &[String],
    contact: &str,
    summary: &str,
    who: &Who,
) -> anyhow::Result<Written> {
    let _guard = lock();
    note_locked(config, part, kind, title, section, facts, aliases, contact, summary, who)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn note_locked(
    config: &Config,
    part: Part,
    kind: &str,
    title: &str,
    section: &str,
    facts: &[String],
    aliases: &[String],
    contact: &str,
    summary: &str,
    who: &Who,
) -> anyhow::Result<Written> {
    let lang = fix_language(config)?;
    let w = words(&lang);
    let title = tk::head(&tk::squeeze(title.trim()), 80).to_string();
    if title.is_empty() {
        return refuse("give the page's name (the person's, the subject's or the area's)");
    }
    let found = find(config, &[part], &title).or_else(|| {
        let contact = contact.trim();
        if contact.is_empty() {
            None
        } else {
            find(config, &[part], contact)
        }
    });
    let (path, before, mut page, created) = match found {
        Some(f) => {
            let before = read_page_text(config, part, &f.path);
            (f.path, before, f.page, false)
        }
        None => {
            let Some(folder) = w.folder(kind).filter(|_| KINDS.contains(&kind)) else {
                return refuse("kind is person, subject or area");
            };
            let mut path = format!("{folder}/{}.md", file_name(&title));
            let mut n = 2;
            while part_root(config, part).join(&path).exists() {
                path = format!("{folder}/{} {n}.md", file_name(&title));
                n += 1;
            }
            (path, None, Page::new(kind, &title), true)
        }
    };
    let mut names = page.aliases();
    names.extend(aliases.iter().cloned());
    if !created && fold(&title) != fold(&page.title) && !page.named(&title) {
        names.push(title.clone());
    }
    page.set_aliases(&names);
    if !contact.trim().is_empty() && page.contact().is_empty() {
        page.front.insert("contact".into(), json!(tk::head(contact.trim(), 200)));
    }
    let section = if section.trim().is_empty() { w.notes.to_string() } else { section.trim().to_string() };
    let (body, added) = add_facts(&page.body, &section, facts, who, &today());
    let body = set_summary(&body, w.brief, summary);
    let summary_changed = summary_of(&body, w.brief) != summary_of(&page.body, w.brief);
    let aliases_changed = before.as_deref().map(|b| Page::parse(b, "").aliases() != page.aliases()).unwrap_or(false);
    if added.is_empty() && !created && !aliases_changed && !summary_changed {
        return Ok(Written { part: Some(part), path, title: page.title, created, added });
    }
    page.body = body;
    page.touch();
    let text = page.render();
    let summary = if !added.is_empty() {
        added.join(" · ")
    } else if summary_changed {
        format!("{} : {}", w.brief, tk::head(&summary_of(&page.body, w.brief), 200))
    } else {
        "aliases".to_string()
    };
    commit(
        config,
        part,
        &path,
        before.as_deref(),
        Some(&text),
        who,
        if created { "create" } else { "note" },
        &summary,
    )?;
    Ok(Written { part: Some(part), path, title: page.title, created, added })
}

/// A fact corrected (`new` empty: removed), or a section replaced (`section` and `text`), and/or the page's « En
/// bref » rewritten (`brief`).
#[allow(clippy::too_many_arguments)]
pub fn edit(
    config: &Config,
    parts: &[Part],
    query: &str,
    old: &str,
    new: &str,
    section: &str,
    text: Option<&str>,
    brief: &str,
    who: &Who,
) -> anyhow::Result<(Written, String)> {
    let _guard = lock();
    let Some(found) = find(config, parts, query) else {
        return refuse(format!("no page « {query} » in the Carnet (search first, or file it with notebook_note)"));
    };
    let before = read_page_text(config, found.part, &found.path);
    let mut page = found.page;
    let date = today();
    let w = words(&language(config));
    let (body, action, summary) = match text {
        None if old.trim().is_empty() && !brief.trim().is_empty() => {
            (page.body.clone(), "brief", format!("{} : {}", w.brief, tk::head(&tk::squeeze(brief), 200)))
        }
        Some(text) => {
            if section.trim().is_empty() {
                return refuse("give the section to replace");
            }
            (
                set_section(&page.body, section, text, who, &date),
                "section",
                format!("{} : {}", section.trim(), tk::head(&tk::squeeze(text), 200)),
            )
        }
        None => match correct_fact(&page.body, old, new, who, &date) {
            Some((body, was)) => {
                let summary =
                    if new.trim().is_empty() { format!("− {was}") } else { format!("{was} → {}", new.trim()) };
                (body, "correct", summary)
            }
            None => return refuse(format!("no fact containing « {old} » on {} (read it first)", page.title)),
        },
    };
    page.body = set_summary(&body, w.brief, brief);
    page.touch();
    let rendered = page.render();
    if before.as_deref() == Some(rendered.as_str()) {
        return refuse("nothing changed");
    }
    commit(config, found.part, &found.path, before.as_deref(), Some(&rendered), who, action, &summary)?;
    Ok((
        Written { part: Some(found.part), path: found.path, title: page.title, created: false, added: Vec::new() },
        summary,
    ))
}

/// The person's own edit of a page (its whole text), or a new page. Recorded as theirs.
pub fn write_page(config: &Config, part: Part, path: &str, text: &str, who: &Who) -> anyhow::Result<Dict> {
    let _guard = lock();
    checked(path)?;
    if text.len() > 200_000 {
        return refuse("the page is too long (200 KB at most)");
    }
    let before = read_page_text(config, part, path);
    let text = if text.ends_with('\n') { text.to_string() } else { format!("{text}\n") };
    if before.as_deref() == Some(text.as_str()) {
        return refuse("nothing changed");
    }
    commit(
        config,
        part,
        path,
        before.as_deref(),
        Some(&text),
        who,
        if before.is_some() { "edit" } else { "create" },
        "",
    )
}

pub fn delete_page(config: &Config, part: Part, path: &str, who: &Who) -> anyhow::Result<Dict> {
    let _guard = lock();
    checked(path)?;
    let Some(before) = read_page_text(config, part, path) else { return refuse("no such page") };
    commit(config, part, path, Some(&before), None, who, "delete", "")
}

/// Page `from` merged into page `into` (same part): its facts appended under their sections, its title and aliases
/// become aliases, it is removed. The nightly pass's « merge ».
pub fn merge_locked(config: &Config, part: Part, from: &str, into: &str, who: &Who) -> anyhow::Result<Option<String>> {
    let (Some(a), Some(b)) = (find(config, &[part], from), find(config, &[part], into)) else { return Ok(None) };
    if a.path == b.path || a.page.kind() == "tasks" || b.page.kind() == "tasks" {
        return Ok(None);
    }
    let before_into = read_page_text(config, part, &b.path);
    let before_from = read_page_text(config, part, &a.path);
    let mut page = b.page.clone();
    let mut names = page.aliases();
    names.push(a.page.title.clone());
    names.extend(a.page.aliases());
    page.set_aliases(&names);
    if page.contact().is_empty() && !a.page.contact().is_empty() {
        page.front.insert("contact".into(), json!(a.page.contact()));
    }
    let mut body = page.body.clone();
    for (section, facts) in a.page.sections() {
        let section = if section.is_empty() { words(&language(config)).notes.to_string() } else { section };
        let mut lines = body_lines(&body);
        for fact in facts {
            let line = if fact.who.is_empty() {
                format!("- {}", fact.text)
            } else {
                format!("- {} — *{}, {}*", fact.text, fact.who, fact.date)
            };
            if lines.iter().any(|l| fold(&split_attribution(l.trim().trim_start_matches("- ")).0) == fold(&fact.text)) {
                continue;
            }
            match section_index(&lines, &section) {
                Some(start) => {
                    let mut at = section_end(&lines, start);
                    while at > start + 1 && lines[at - 1].trim().is_empty() {
                        at -= 1;
                    }
                    lines.insert(at, line);
                }
                None => {
                    if !lines.is_empty() {
                        lines.push(String::new());
                    }
                    lines.push(format!("## {section}"));
                    lines.push(line);
                }
            }
        }
        body = lines.join("\n") + "\n";
    }
    page.body = body;
    page.touch();
    let summary = format!("{} → {}", a.page.title, page.title);
    commit(config, part, &b.path, before_into.as_deref(), Some(&page.render()), who, "merge", &summary)?;
    commit(config, part, &a.path, before_from.as_deref(), None, who, "merge", &summary)?;
    Ok(Some(summary))
}

// -- tasks ---------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: String,
    pub done: bool,
    pub title: String,
    pub due: String,
    pub done_on: String,
    pub links: Vec<String>,
    pub who: String,
    pub date: String,
    pub part: Part,
}

static TASK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*- \[([ xX])\] (.*?)(?:\s+\^(t[0-9a-f]{6,16}))?\s*$").expect("valid regex"));
static DUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*📅\s*(\d{4}-\d{2}-\d{2})").expect("valid regex"));
static DONE_ON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*✅\s*(\d{4}-\d{2}-\d{2})").expect("valid regex"));
static DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}-\d{2}").expect("valid regex"));

impl Task {
    pub fn parse(line: &str, part: Part) -> Option<Task> {
        let c = TASK.captures(line)?;
        let (rest, by) = split_attribution(&c[2]);
        let due = DUE.captures(&rest).map(|d| d[1].to_string()).unwrap_or_default();
        let done_on = DONE_ON.captures(&rest).map(|d| d[1].to_string()).unwrap_or_default();
        let title = DONE_ON.replace_all(&DUE.replace_all(&rest, ""), "").trim().to_string();
        let links = links_in(&title);
        let (who, date) = by.unwrap_or_default();
        Some(Task {
            id: c.get(3).map(|m| m.as_str().to_string()).unwrap_or_default(),
            done: &c[1] != " ",
            title,
            due,
            done_on,
            links,
            who,
            date,
            part,
        })
    }

    pub fn render(&self) -> String {
        let mut line = format!("- [{}] {}", if self.done { "x" } else { " " }, self.title);
        if !self.due.is_empty() {
            line.push_str(&format!(" 📅 {}", self.due));
        }
        if self.done && !self.done_on.is_empty() {
            line.push_str(&format!(" ✅ {}", self.done_on));
        }
        if !self.who.is_empty() {
            line.push_str(&format!(" — *{}, {}*", self.who, self.date));
        }
        if !self.id.is_empty() {
            line.push_str(&format!(" ^{}", self.id));
        }
        line
    }

    pub fn view(&self) -> Value {
        json!({"id": self.id, "title": self.title, "due": if self.due.is_empty() { Value::Null } else { json!(self.due) },
               "done": self.done, "done_on": if self.done_on.is_empty() { Value::Null } else { json!(self.done_on) },
               "links": self.links, "by": self.who, "date": self.date, "part": self.part.as_str()})
    }
}

/// The tasks of a part, in the page's order.
pub fn tasks(config: &Config, part: Part) -> Vec<Task> {
    let w = words(&language(config));
    let text = read_page_text(config, part, &w.tasks_path()).unwrap_or_default();
    text.lines().filter_map(|l| Task::parse(l, part)).collect()
}

/// `2026-10-25` or `2026-10-25T09:00…`: the date, checked.
pub fn due_date(due: &str) -> anyhow::Result<String> {
    let due = due.trim();
    if due.is_empty() {
        return Ok(String::new());
    }
    let Some(m) = DATE.find(due) else {
        return refuse("due is a date: YYYY-MM-DD (a time may follow: YYYY-MM-DDTHH:MM)");
    };
    if chrono::NaiveDate::parse_from_str(m.as_str(), "%Y-%m-%d").is_err() {
        return refuse("due is not a real date");
    }
    Ok(m.as_str().to_string())
}

/// A new task in the part's tasks page; with a date, it also becomes a phone reminder (when the person shares their
/// reminders with this agent, or it is the person's own task). `reminder_due`: the date or date-time the phone gets.
pub fn add_task(
    config: &Config,
    part: Part,
    title: &str,
    due: &str,
    page: &str,
    who: &Who,
) -> anyhow::Result<(Task, bool)> {
    let _guard = lock();
    add_task_locked(config, part, title, due, page, who)
}

pub(crate) fn add_task_locked(
    config: &Config,
    part: Part,
    title: &str,
    due: &str,
    page: &str,
    who: &Who,
) -> anyhow::Result<(Task, bool)> {
    let lang = fix_language(config)?;
    let w = words(&lang);
    let title = tk::head(&tk::squeeze(title.replace(['\n', '^'], " ").trim()), 300).to_string();
    if title.is_empty() {
        return refuse("a task needs a title");
    }
    let date = due_date(due)?;
    let path = w.tasks_path();
    let before = read_page_text(config, part, &path);
    let mut doc = match &before {
        Some(text) => Page::parse(text, w.tasks),
        None => {
            let mut p = Page::new("tasks", w.tasks);
            p.front.remove("aliases");
            p
        }
    };
    let existing: Vec<Task> = doc.body.lines().filter_map(|l| Task::parse(l, part)).collect();
    if let Some(same) = existing.iter().find(|t| !t.done && fold(&t.title) == fold(&title) && t.due == date) {
        return Ok((same.clone(), false));
    }
    let mut title_line = title.clone();
    let page = page.trim();
    if !page.is_empty() && !title_line.contains("[[") {
        let target = find(config, &[part], page).map(|f| f.page.title).unwrap_or_else(|| file_name(page));
        title_line = format!("{title_line} [[{target}]]");
    }
    let task = Task {
        id: format!("t{}", py::token_hex(4)),
        done: false,
        title: title_line,
        due: date,
        done_on: String::new(),
        links: Vec::new(),
        who: who.name.replace(['*', ','], " ").trim().to_string(),
        date: today(),
        part,
    };
    let mut lines = body_lines(&doc.body);
    lines.push(task.render());
    doc.body = lines.join("\n") + "\n";
    doc.touch();
    commit(config, part, &path, before.as_deref(), Some(&doc.render()), who, "task_add", &title)?;
    let task = Task { links: links_in(&task.title), ..task };
    let linked = if task.due.is_empty() { false } else { link_reminder(config, &task, due, who)? };
    Ok((task, linked))
}

/// A task done (or open again). `from_phone`: ticked on the phone (its reminder is already done: nothing is sent
/// back). Returns the task, None when no task has this id.
pub fn set_task_done(
    config: &Config,
    parts: &[Part],
    id: &str,
    done: bool,
    who: &Who,
    from_phone: bool,
) -> anyhow::Result<Option<Task>> {
    let _guard = lock();
    set_task_done_locked(config, parts, id, done, who, from_phone)
}

fn set_task_done_locked(
    config: &Config,
    parts: &[Part],
    id: &str,
    done: bool,
    who: &Who,
    from_phone: bool,
) -> anyhow::Result<Option<Task>> {
    let w = words(&language(config));
    let path = w.tasks_path();
    let id = id.trim().trim_start_matches('^');
    for part in parts {
        let Some(before) = read_page_text(config, *part, &path) else { continue };
        let mut doc = Page::parse(&before, w.tasks);
        let mut lines = body_lines(&doc.body);
        let Some(index) = lines.iter().position(|l| Task::parse(l, *part).is_some_and(|t| t.id == id)) else {
            continue;
        };
        let mut task = Task::parse(&lines[index], *part).expect("matched");
        if task.done == done {
            return Ok(Some(task));
        }
        task.done = done;
        task.done_on = if done { today() } else { String::new() };
        lines[index] = task.render();
        doc.body = lines.join("\n") + "\n";
        doc.touch();
        let action = if done { "task_done" } else { "task_open" };
        commit(config, *part, &path, Some(&before), Some(&doc.render()), who, action, &task.title)?;
        if done && !from_phone {
            complete_reminder(config, &task.id)?;
        }
        return Ok(Some(task));
    }
    Ok(None)
}

// -- the tasks' phone reminders (reminders.rs: the phone applies the queue) ------------------------------------------

/// What a Carnet task's reminder carries in its notes, so a reminder ticked on the phone ticks the task.
pub fn reminder_mark(task: &str) -> String {
    format!("BipAgents · Carnet ^{task}")
}

static MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"BipAgents · Carnet \^(t[0-9a-f]{6,16})").expect("valid regex"));

fn links(state: &Dict) -> Dict {
    match state.get("reminders") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    }
}

/// A dated task's phone reminder asked for, once: when the phone shares its reminders (with this agent, or any for
/// the person's own task).
fn link_reminder(config: &Config, task: &Task, due: &str, who: &Who) -> anyhow::Result<bool> {
    let data = crate::reminders::read(config);
    let allowed = strings(data.get("agents"));
    let ok = match &who.agent {
        Some(agent) => allowed.contains(agent),
        None => !allowed.is_empty(),
    };
    if !ok || links(&read_state(config)).contains_key(&task.id) {
        return Ok(false);
    }
    let title = LINK.replace_all(&task.title, |c: &regex::Captures| c[1].to_string()).trim().to_string();
    let due = if due.trim().len() > 10 && due.trim().starts_with(&task.due) {
        due.trim().to_string()
    } else {
        task.due.clone()
    };
    let args = json!({"title": tk::head(&title, 300), "due": tk::head(&due, 40), "list": "", "notes": reminder_mark(&task.id)});
    let agent = who.agent.clone().unwrap_or_else(|| "notebook".into());
    let op = crate::reminders::enqueue(config, &agent, "add", args)?;
    let task_id = task.id.clone();
    update_state(config, |s| {
        let mut map = links(s);
        map.insert(task_id, json!({"op": op, "part": task.part.as_str(), "complete": false}));
        s.insert("reminders".into(), Value::Object(map));
    })?;
    Ok(true)
}

/// A task done in the Carnet: its reminder ticked on the phone (asked), or its add still waiting cancelled.
fn complete_reminder(config: &Config, task: &str) -> anyhow::Result<()> {
    let state = read_state(config);
    let Some(link) = links(&state).get(task).cloned() else { return Ok(()) };
    let op = tk::str_or_empty(link.get("op"));
    if crate::reminders::cancel_pending(config, &op)? {
        update_state(config, |s| {
            let mut map = links(s);
            map.remove(task);
            s.insert("reminders".into(), Value::Object(map));
        })?;
        return Ok(());
    }
    let data = crate::reminders::read(config);
    let mark = reminder_mark(task);
    let item = match data.get("items") {
        Some(Value::Array(items)) => items.iter().find(|i| tk::str_or_empty(i.get("notes")).contains(&mark)).cloned(),
        _ => None,
    };
    match item {
        Some(item) if !item.get("completed").map(py::truthy).unwrap_or(false) => {
            crate::reminders::enqueue(config, "notebook", "complete", json!({"id": tk::str_or_empty(item.get("id"))}))?;
            update_state(config, |s| {
                let mut map = links(s);
                map.remove(task);
                s.insert("reminders".into(), Value::Object(map));
            })?;
        }
        Some(_) => {}
        None => {
            // not on the phone yet: ticked when the phone next shares it
            update_state(config, |s| {
                let mut map = links(s);
                if let Some(Value::Object(l)) = map.get_mut(task) {
                    l.insert("complete".into(), json!(true));
                }
                s.insert("reminders".into(), Value::Object(map));
            })?;
        }
    }
    Ok(())
}

/// The phone shared its reminders (`PUT /v1/reminders`): a Carnet task's reminder ticked there ticks the task; a task
/// done in the Carnet before its reminder reached the phone gets its reminder ticked now. No loop: a task ticked from
/// the phone sends nothing back, and each link is used once.
pub fn reminders_synced(config: &Config, items: &[Value]) {
    if !root(config).exists() {
        return;
    }
    let _guard = lock();
    let you = Who::you(config);
    let state = read_state(config);
    let map = links(&state);
    for item in items {
        let notes = tk::str_or_empty(item.get("notes"));
        let Some(c) = MARK.captures(&notes) else { continue };
        let task = c[1].to_string();
        let completed = item.get("completed").map(py::truthy).unwrap_or(false);
        let link = map.get(&task).cloned().unwrap_or(json!({}));
        let result = if completed {
            set_task_done_locked(config, &[Part::Main, Part::Health], &task, true, &you, true).map(|_| ())
        } else if link.get("complete").map(py::truthy).unwrap_or(false) {
            crate::reminders::enqueue(config, "notebook", "complete", json!({"id": tk::str_or_empty(item.get("id"))}))
                .map(|_| ())
                .map_err(anyhow::Error::from)
        } else {
            continue;
        };
        let cleared = update_state(config, |s| {
            let mut m = links(s);
            m.remove(&task);
            s.insert("reminders".into(), Value::Object(m));
        });
        if let Err(err) = result.and(cleared.map(|_| ()).map_err(anyhow::Error::from)) {
            tracing::warn!(task = %task, error = %format!("{err:#}"), "notebook task not synced with its reminder");
        }
    }
}

// -- search --------------------------------------------------------------------------------------------------------

/// Pages matching `words` (all of them, folded) in their title, aliases or lines: (part, path, page, matching lines).
pub fn search(config: &Config, parts: &[Part], words_text: &str, kind: &str) -> Vec<(Part, String, Page, Vec<String>)> {
    let wanted: Vec<String> =
        fold(words_text).split(' ').filter(|w| !w.is_empty()).map(str::to_string).take(8).collect();
    let mut out = Vec::new();
    for part in parts {
        for (path, page) in all_pages(config, *part) {
            if !kind.is_empty() && page.kind() != kind {
                continue;
            }
            let names = fold(&format!("{} {} {}", page.title, page.aliases().join(" "), page.contact()));
            let mut lines: Vec<String> = Vec::new();
            for line in page.body.lines() {
                let folded = fold(line);
                if !wanted.is_empty()
                    && wanted.iter().all(|w| folded.contains(w.as_str()) || names.contains(w.as_str()))
                {
                    lines.push(tk::head(line.trim(), 240).to_string());
                }
            }
            let by_name = wanted.iter().all(|w| names.contains(w.as_str()));
            if wanted.is_empty() || by_name || !lines.is_empty() {
                if by_name {
                    lines.retain(|l| wanted.iter().any(|w| fold(l).contains(w.as_str())));
                }
                lines.truncate(8);
                out.push((*part, path, page, lines));
            }
        }
    }
    out
}

// -- documents -----------------------------------------------------------------------------------------------------

/// A file's kind, from its name.
pub fn file_type(name: &str) -> &'static str {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "pdf" => "pdf",
        "jpg" | "jpeg" | "png" | "heic" | "heif" | "gif" | "webp" | "tif" | "tiff" => "image",
        "mp3" | "wav" | "m4a" | "aac" | "ogg" | "opus" | "flac" => "audio",
        "mp4" | "mov" | "m4v" | "webm" => "video",
        "html" | "htm" => "page",
        "doc" | "docx" | "odt" | "rtf" | "pages" => "document",
        "xls" | "xlsx" | "ods" | "csv" | "numbers" => "spreadsheet",
        "ppt" | "pptx" | "odp" | "key" => "presentation",
        "txt" | "md" | "json" => "text",
        "zip" => "archive",
        _ => "other",
    }
}

/// The files the agents made (audio, pages) or were sent (uploads), where the bridge keeps them: by agent, the newest
/// first, each with the Carnet pages that name it.
pub fn documents(config: &Config) -> Vec<Value> {
    let mut found: Vec<Dict> = Vec::new();
    let mut add =
        |agent: &str, name: String, origin: &str, path: &Path, media: Option<String>, title: Option<String>| {
            let meta = std::fs::metadata(path).ok();
            let date = tk::mtime(path)
                .and_then(|m| chrono::DateTime::from_timestamp(m as i64, 0))
                .map(|d| d.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
                .unwrap_or_default();
            let mut item = Dict::new();
            item.insert("agent".into(), json!(agent));
            item.insert(
                "agent_name".into(),
                json!(config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or_else(|| agent.to_string())),
            );
            item.insert("name".into(), json!(title.unwrap_or_else(|| name.clone())));
            item.insert("file".into(), json!(name));
            item.insert("origin".into(), json!(origin));
            item.insert("type".into(), json!(file_type(&name)));
            item.insert("size".into(), json!(meta.map(|m| m.len()).unwrap_or(0)));
            item.insert("date".into(), json!(date));
            item.insert("path".into(), json!(media));
            found.push(item);
        };
    for agent in config.agents.values() {
        if let Some(root) = agent.upload_dir_host.as_deref().filter(|r| !r.is_empty()) {
            for month in std::fs::read_dir(root).into_iter().flatten().flatten() {
                if !month.path().is_dir() {
                    continue;
                }
                for file in std::fs::read_dir(month.path()).into_iter().flatten().flatten() {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if !file.path().is_file() || name.starts_with('.') {
                        continue;
                    }
                    // `{id12}-{name}` (files.rs)
                    let shown = match name.split_once('-') {
                        Some((id, rest)) if id.len() == 12 && !rest.is_empty() => rest.to_string(),
                        _ => name.clone(),
                    };
                    add(&agent.name, shown, "received", &file.path(), None, None);
                }
            }
        }
        let audio = crate::agent_tools::audio_root(config).join(&agent.name);
        for file in std::fs::read_dir(&audio).into_iter().flatten().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            if file.path().is_file() && !name.starts_with('.') && file_type(&name) == "audio" {
                let media = format!("{}/{}/{name}", crate::agent_tools::MEDIA_PREFIX, agent.name);
                add(&agent.name, name, "made", &file.path(), Some(media), None);
            }
        }
        for page in crate::pages::list_pages(config, &agent.name) {
            let id = tk::str_or_empty(page.get("page"));
            let file = crate::pages::root(config).join(&agent.name).join(format!("{id}.html"));
            add(
                &agent.name,
                format!("{id}.html"),
                "made",
                &file,
                Some(crate::pages::media_path(&agent.name, &id)),
                Some(tk::str_or_empty(page.get("title"))),
            );
        }
    }
    // the Carnet's pages that name a file (its name, or its name without the extension)
    let pages = all_pages(config, Part::Main);
    let mut out: Vec<Value> = Vec::new();
    found.sort_by_key(|d| std::cmp::Reverse(tk::str_or_empty(d.get("date"))));
    for mut item in found {
        let file = tk::str_or_empty(item.get("file"));
        let stem =
            fold(Path::new(&file).file_stem().map(|s| s.to_string_lossy().into_owned()).as_deref().unwrap_or(""));
        let name = fold(&tk::str_or_empty(item.get("name")));
        let subjects: Vec<Value> = pages
            .iter()
            .filter(|(_, p)| KINDS.contains(&p.kind().as_str()))
            .filter(|(_, p)| {
                let body = fold(&p.body);
                (stem.len() >= 4 && body.contains(&stem)) || (name.len() >= 6 && body.contains(&name))
            })
            .map(|(path, p)| json!({"title": p.title, "path": path, "kind": p.kind()}))
            .collect();
        item.insert("subjects".into(), Value::Array(subjects));
        out.push(Value::Object(item));
    }
    out
}

/// `Documents.md`: the index as a page of the vault (written for Obsidian; the app reads `documents`).
pub fn documents_page(config: &Config, documents: &[Value]) -> String {
    let w = words(&language(config));
    let mut out = format!("---\ntype: documents\nupdated: {}\n---\n# {}\n", today(), w.documents);
    let mut agents: Vec<String> = Vec::new();
    for d in documents {
        let a = tk::str_or_empty(d.get("agent_name"));
        if !agents.contains(&a) {
            agents.push(a);
        }
    }
    for agent in agents {
        out.push_str(&format!("\n## {agent}\n"));
        for d in documents.iter().filter(|d| tk::str_or_empty(d.get("agent_name")) == agent) {
            let subjects: Vec<String> = match d.get("subjects") {
                Some(Value::Array(s)) => {
                    s.iter().map(|s| format!("[[{}]]", tk::str_or_empty(s.get("title")))).collect()
                }
                _ => Vec::new(),
            };
            let date = tk::head(&tk::str_or_empty(d.get("date")), 10).to_string();
            let mut line =
                format!("- {} ({}, {date})", tk::str_or_empty(d.get("name")), tk::str_or_empty(d.get("type")));
            if !subjects.is_empty() {
                line.push_str(&format!(" · {}", subjects.join(" ")));
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

// -- export --------------------------------------------------------------------------------------------------------

/// The vault as a zip, for Obsidian (its Documents page written fresh; the private health part only when asked, in a
/// folder of its own).
pub fn export_zip(config: &Config, with_health: bool) -> anyhow::Result<Vec<u8>> {
    let w = words(&language(config));
    let docs = documents(config);
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut parts = vec![(Part::Main, String::new())];
        if with_health {
            parts.push((Part::Health, format!("{}/", w.private)));
        }
        for (part, prefix) in parts {
            for (path, _) in all_pages(config, part) {
                let Some(text) = read_page_text(config, part, &path) else { continue };
                zip.start_file(format!("{prefix}{path}"), options)?;
                zip.write_all(text.as_bytes())?;
            }
        }
        zip.start_file(format!("{}.md", w.documents), options)?;
        zip.write_all(documents_page(config, &docs).as_bytes())?;
        zip.finish()?;
    }
    Ok(buffer.into_inner())
}

// -- seeding -------------------------------------------------------------------------------------------------------

/// What the agents already knew (their Hermes memory: MEMORY.md and USER.md), copied once per agent into the « À
/// ranger » page, without any model: the nightly pass sorts it into pages a little each night, within its budget.
pub fn seed(config: &Config) -> anyhow::Result<usize> {
    let _guard = lock();
    let seeded = strings(read_state(config).get("seeded"));
    let mut copied = 0;
    let mut done: Vec<String> = seeded.clone();
    for agent in config.agents.values() {
        if seeded.contains(&agent.name) || agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
            continue;
        }
        if !enabled(config, &agent.name) {
            continue;
        }
        let folder = crate::agents_admin::profile_folder(config, agent);
        if !folder.is_dir() {
            continue;
        }
        let mut entries: Vec<String> = Vec::new();
        for which in ["user", "notes"] {
            if let Ok(found) = crate::memory_edit::read_entries(&folder, which) {
                for e in strings(found.get("entries")) {
                    let e = tk::squeeze(&e.replace('\n', " "));
                    if !e.is_empty() {
                        entries.push(e);
                    }
                }
            }
        }
        done.push(agent.name.clone());
        if entries.is_empty() {
            continue;
        }
        let lang = fix_language(config)?;
        let w = words(&lang);
        let who = Who::agent(config, &agent.name);
        let path = w.inbox_path();
        let before = read_page_text(config, Part::Main, &path);
        let mut page = match &before {
            Some(t) => Page::parse(t, w.inbox),
            None => {
                let mut p = Page::new("inbox", w.inbox);
                p.front.remove("aliases");
                p
            }
        };
        let (body, added) = add_facts(&page.body, &agent.display_name, &entries, &who, &today());
        if added.is_empty() {
            continue;
        }
        page.body = body;
        page.touch();
        commit(
            config,
            Part::Main,
            &path,
            before.as_deref(),
            Some(&page.render()),
            &who,
            "seed",
            &format!("{} entries", added.len()),
        )?;
        copied += added.len();
    }
    if done != seeded {
        update_state(config, |s| {
            s.insert("seeded".into(), json!(done));
        })?;
    }
    if copied > 0 {
        tracing::info!(entries = copied, "notebook seeded from the agents' memories");
    }
    Ok(copied)
}

/// The « À ranger » page's lines (its facts), for the nightly pass.
pub fn inbox(config: &Config) -> (String, Vec<String>) {
    let w = words(&language(config));
    let path = w.inbox_path();
    let text = read_page_text(config, Part::Main, &path).unwrap_or_default();
    let lines = text.lines().filter(|l| l.starts_with("- ")).map(str::to_string).collect();
    (path, lines)
}

/// Lines taken out of « À ranger » (sorted by the nightly pass).
pub(crate) fn inbox_remove_locked(config: &Config, lines: &[String], who: &Who) -> anyhow::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let w = words(&language(config));
    let path = w.inbox_path();
    let Some(before) = read_page_text(config, Part::Main, &path) else { return Ok(()) };
    let mut page = Page::parse(&before, w.inbox);
    let kept: Vec<&str> = page.body.lines().filter(|l| !lines.iter().any(|x| x == l)).collect();
    // sections left empty go too
    let mut out: Vec<String> = Vec::new();
    for (i, line) in kept.iter().enumerate() {
        if line.starts_with("## ") {
            let next_items = kept[i + 1..].iter().take_while(|l| !l.starts_with("## ")).any(|l| l.starts_with("- "));
            if !next_items {
                continue;
            }
        }
        out.push(line.to_string());
    }
    page.body = out.join("\n").trim_matches('\n').to_string() + "\n";
    page.touch();
    commit(
        config,
        Part::Main,
        &path,
        Some(&before),
        Some(&page.render()),
        who,
        "sorted",
        &format!("{} lines", lines.len()),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lea() -> Who {
        Who { by: "lea".into(), name: "Léa".into(), agent: Some("lea".into()) }
    }

    #[test]
    fn facts_are_added_once_with_their_attribution() {
        let (body, added) = add_facts("", "Infos", &["Anniversaire : 12 mars".into()], &lea(), "2026-10-22");
        assert_eq!(body, "## Infos\n- Anniversaire : 12 mars — *Léa, 2026-10-22*\n");
        assert_eq!(added.len(), 1);
        let (again, added) =
            add_facts(&body, "infos", &["anniversaire: 12 Mars".into(), "Aime le jazz".into()], &lea(), "2026-10-23");
        assert_eq!(added, vec!["Aime le jazz".to_string()]);
        assert_eq!(
            again,
            "## Infos\n- Anniversaire : 12 mars — *Léa, 2026-10-22*\n- Aime le jazz — *Léa, 2026-10-23*\n"
        );
        let (other, _) = add_facts(&again, "Famille", &["Fille : Zoé".into()], &lea(), "2026-10-23");
        assert!(other.ends_with("\n\n## Famille\n- Fille : Zoé — *Léa, 2026-10-23*\n"));
        assert_eq!(
            split_attribution("Aime le jazz — *Léa, 2026-10-23*"),
            ("Aime le jazz".into(), Some(("Léa".into(), "2026-10-23".into())))
        );
        let (fixed, was) = correct_fact(&other, "12 mars", "Anniversaire : 13 mars", &lea(), "2026-10-24").unwrap();
        assert_eq!(was, "Anniversaire : 12 mars");
        assert!(fixed.starts_with("## Infos\n- Anniversaire : 13 mars — *Léa, 2026-10-24*\n"));
        assert!(correct_fact(&fixed, "nowhere", "", &lea(), "2026-10-24").is_none());
        let replaced = set_section(&fixed, "Infos", "- Né le 13 mars", &lea(), "2026-10-25");
        assert!(replaced.starts_with("## Infos\n- Né le 13 mars — *Léa, 2026-10-25*\n\n## Famille"));
    }

    #[test]
    fn pages_round_trip_with_their_frontmatter() {
        let mut page = Page::new("person", "Marc Dupont");
        page.set_aliases(&["Marco".into(), "marc dupont".into(), "Marco".into()]);
        page.front.insert("contact".into(), json!("ABC:123"));
        page.front.insert("updated".into(), json!("2026-10-22"));
        page.body = "## Infos\n- Aime le jazz\n".into();
        let text = page.render();
        assert_eq!(
            text,
            "---\ntype: person\naliases:\n  - Marco\ncontact: \"ABC:123\"\nupdated: 2026-10-22\n---\n# Marc Dupont\n\n## Infos\n- Aime le jazz\n"
        );
        let back = Page::parse(&text, "x");
        assert_eq!(back.title, "Marc Dupont");
        assert_eq!(back.aliases(), vec!["Marco"]);
        assert_eq!(back.contact(), "ABC:123");
        assert!(back.named("marco") && back.named("MARC DUPONT") && back.named("ABC:123") && !back.named("Marc"));
        assert_eq!(back.render(), text);
        assert_eq!(Page::parse("Juste du texte\n", "Note").title, "Note");
    }

    #[test]
    fn tasks_read_and_write_as_obsidian_tasks() {
        let line = "- [ ] Appeler le plombier [[Maison]] 📅 2026-10-25 — *Léa, 2026-10-22* ^t1a2b3c4d";
        let task = Task::parse(line, Part::Main).unwrap();
        assert_eq!(task.title, "Appeler le plombier [[Maison]]");
        assert_eq!((task.due.as_str(), task.id.as_str(), task.who.as_str()), ("2026-10-25", "t1a2b3c4d", "Léa"));
        assert_eq!(task.links, vec!["Maison"]);
        assert_eq!(task.render(), line);
        let done = Task { done: true, done_on: "2026-10-26".into(), ..task };
        assert_eq!(
            done.render(),
            "- [x] Appeler le plombier [[Maison]] 📅 2026-10-25 ✅ 2026-10-26 — *Léa, 2026-10-22* ^t1a2b3c4d"
        );
        assert!(Task::parse(&done.render(), Part::Main).unwrap().done);
        assert_eq!(due_date("2026-10-25T09:30").unwrap(), "2026-10-25");
        assert!(due_date("demain").is_err() && due_date("2026-02-30").is_err());
    }

    #[test]
    fn names_and_paths() {
        assert_eq!(file_name("Marc/Dupont: [notes]"), "Marc Dupont notes");
        assert_eq!(fold("Élodie  O'Brien!"), "elodie o brien");
        assert!(checked("Personnes/Marc.md").is_ok());
        assert!(checked("../x.md").is_err() && checked("a/b/c.md").is_err() && checked("Personnes/x.txt").is_err());
        assert_eq!(words("de").tasks_path(), "Aufgaben/Aufgaben.md");
        assert_eq!(words("it").lang, "en");
    }
}
