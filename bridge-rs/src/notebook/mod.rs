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
//!         Voyages/Lisbonne — nov. 2025.md  trips (Trips, Reisen, Viajes: trip.rs), apart from the subjects
//!         Fiches/Achats.md               records (Records, Unterlagen, Registros: records.rs)
//!         Archives/…                     what is past, moved by the nightly pass (archive.rs): Archives/Voyages,
//!                                        Archives/Fiches/Achats.md… (Archive, Archiv, Archivo)
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
pub mod archive;
pub mod fill;
pub mod records;
pub mod template;
pub mod tidy;
pub mod tools;
pub mod trip;
pub mod trip_auto;

use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use chrono::Datelike;
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
    /// The tasks page's section for proposed tasks left untouched long after their date (never deleted).
    pub archived: &'static str,
    /// The area page where the person's preferences about the Carnet go (« Pas de rappels pour les anniversaires »).
    pub preferences: &'static str,
    /// The nightly pass and the re-filing, in the history (« rangement »).
    pub tidy: &'static str,
    /// The person's own page (« Toi »), at the vault's root, never among « Personnes ».
    pub me: &'static str,
    /// The folder of the records' pages (purchases, subscriptions, contracts, papers, bookings: records.rs).
    pub records: &'static str,
    /// The trips' folder (trip.rs), apart from the subjects.
    pub trips: &'static str,
    /// What is past (archive.rs): past trips, bookings that are over, ended subscriptions and contracts, replaced
    /// papers, purchases out of warranty — moved here by the nightly pass, never deleted.
    pub archives: &'static str,
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
        archived: "Archivées",
        preferences: "Préférences",
        tidy: "rangement",
        me: "Toi",
        records: "Fiches",
        trips: "Voyages",
        archives: "Archives",
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
        archived: "Archived",
        preferences: "Preferences",
        tidy: "tidying",
        me: "You",
        records: "Records",
        trips: "Trips",
        archives: "Archive",
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
        archived: "Archiviert",
        preferences: "Vorlieben",
        tidy: "Aufräumen",
        me: "Du",
        records: "Unterlagen",
        trips: "Reisen",
        archives: "Archiv",
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
        archived: "Archivadas",
        preferences: "Preferencias",
        tidy: "orden",
        me: "Tú",
        records: "Registros",
        trips: "Viajes",
        archives: "Archivo",
    },
];

pub fn words(lang: &str) -> &'static Words {
    WORDS.iter().find(|w| w.lang == lang).unwrap_or(&WORDS[1])
}

/// The kinds of pages an agent writes, in the frontmatter's `type`. A trip (« Voyage », trip.rs) has its own outline,
/// layout and folder (« Voyages »; a past one in « Archives/Voyages »): it is not listed among the subjects.
pub const KINDS: [&str; 5] = ["person", "subject", "area", "self", "trip"];

impl Words {
    pub fn folder(&self, kind: &str) -> Option<&'static str> {
        match kind {
            "person" => Some(self.people),
            "subject" => Some(self.subjects),
            "trip" => Some(self.trips),
            "area" => Some(self.areas),
            "task" | "tasks" => Some(self.tasks),
            _ => None,
        }
    }

    pub fn kind_of(&self, folder: &str) -> Option<&'static str> {
        [
            ("person", self.people),
            ("subject", self.subjects),
            ("area", self.areas),
            ("tasks", self.tasks),
            ("trip", self.trips),
        ]
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

    /// `Toi.md`: the person's own page.
    pub fn self_path(&self) -> String {
        format!("{}.md", self.me)
    }

    /// `Archives/Voyages`: the past trips' folder.
    pub fn past_trips(&self) -> String {
        format!("{}/{}", self.archives, self.trips)
    }

    /// Whether a page's path is in the archives (« Archives/… »).
    pub fn archived(&self, path: &str) -> bool {
        path.starts_with(&format!("{}/", self.archives))
    }
}

/// The words an agent may use for the person themself (« moi », « l'utilisateur »…), folded.
const SELF_WORDS: [&str; 22] = [
    "moi",
    "toi",
    "moi meme",
    "l utilisateur",
    "utilisatrice",
    "me",
    "myself",
    "you",
    "yourself",
    "the user",
    "user",
    "ich",
    "mich",
    "du",
    "nutzer",
    "benutzer",
    "yo",
    "tu",
    "mi",
    "usuario",
    "usuaria",
    "el usuario",
];

/// Whether a page name means the person themself (their « Toi » page).
pub fn self_word(name: &str) -> bool {
    let n = fold(name);
    !n.is_empty() && (SELF_WORDS.contains(&n.as_str()) || WORDS.iter().any(|w| fold(w.me) == n))
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
    if relative.is_empty() || !plain || depth > 3 || !relative.ends_with(".md") || relative.contains('\0') {
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

    /// The re-filing of pages into their outline (« rangement »): no agent, no attribution of its own.
    pub fn tidy(config: &Config) -> Who {
        Who { by: "nightly".into(), name: words(&language(config)).tidy.into(), agent: None }
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
/// A curly apostrophe (« aujourd’hui ») folds like a straight one (« aujourd'hui »): both a space, never dropped (the
/// ASCII folding alone dropped it, so « aujourd’hui » and « aujourd'hui » were not the same name).
pub fn fold(text: &str) -> String {
    let plain = tk::ascii_fold(&tk::spaced_punctuation(&text.to_lowercase()));
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
        front.insert("id".into(), json!(new_page_id()));
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

    /// The page's stable id (`p` and 10 hex digits in its frontmatter): it survives renames and moves, for links
    /// from outside the vault (a family share of this page, later).
    pub fn id(&self) -> String {
        tk::str_or_empty(self.front.get("id"))
    }

    /// An id given to a page that has none yet (made in Obsidian, or before ids): first in its frontmatter.
    pub fn ensure_id(&mut self) {
        if self.id().is_empty() {
            let mut front = Dict::new();
            front.insert("id".into(), json!(new_page_id()));
            front.extend(std::mem::take(&mut self.front));
            self.front = front;
        }
    }

    /// A person close to the person (`close: true`, or `proche: true` written by hand): their dates are shown.
    pub fn close(&self) -> bool {
        ["close", "proche"].iter().any(|k| self.front.get(*k).map(py::truthy).unwrap_or(false))
    }

    /// The icon the person chose for it in the app (`icon:`), if any.
    pub fn icon(&self) -> Option<String> {
        self.front.get("icon").and_then(Value::as_str).map(str::trim).filter(|i| !i.is_empty()).map(str::to_string)
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

    /// Whether `query` names this page: its title, an alias, its contact id or its own id.
    pub fn named(&self, query: &str) -> bool {
        let wanted = fold(query);
        !wanted.is_empty()
            && (fold(&self.title) == wanted
                || (!self.id().is_empty() && self.id() == query.trim())
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
            fold(trip::without_box(&split_attribution(item).0))
        })
        .collect();
    let mut added: Vec<String> = Vec::new();
    for fact in facts {
        let fact = tk::squeeze(fact.trim().trim_start_matches("- ").trim());
        let fact = tk::head(&fact, 600).to_string();
        let folded = fold(trip::without_box(&fact));
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
/// attribution keep the one of the same fact already there (the person rewrote the section, not every fact), else get
/// a new one.
pub fn set_section(body: &str, section: &str, text: &str, who: &Who, date: &str) -> String {
    let mut lines = body_lines(body);
    let old: Vec<String> = match section_index(&lines, section) {
        Some(start) => lines[start + 1..section_end(&lines, start)].to_vec(),
        None => Vec::new(),
    };
    let new: Vec<String> = text
        .trim_matches('\n')
        .lines()
        .map(|l| {
            let trimmed = l.trim_end();
            if let Some(item) = trimmed.strip_prefix("- ") {
                if split_attribution(item).1.is_none() && !item.trim().is_empty() {
                    let same = old.iter().find(|o| {
                        let (text, by) = split_attribution(o.trim().trim_start_matches("- "));
                        by.is_some() && fold(&text) == fold(item)
                    });
                    if let Some(same) = same {
                        return same.trim_end().to_string();
                    }
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
    // the vault's root, its folders, and their own folders (« Archives/Voyages »)
    let mut dirs: Vec<(PathBuf, String, usize)> = vec![(base.clone(), String::new(), 0)];
    while let Some((dir, prefix, depth)) = dirs.pop() {
        visit(&dir, &prefix);
        if depth >= 2 {
            continue;
        }
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
            if path.is_dir() && !is_link && !entry.file_name().to_string_lossy().starts_with('.') {
                let folder = entry.file_name().to_string_lossy().into_owned();
                dirs.push((path, format!("{prefix}{folder}/"), depth + 1));
            }
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
    // a path, with or without its « .md » (« Domaines/Santé »)
    let as_path = if query.ends_with(".md") {
        Some(query.to_string())
    } else if query.contains('/') {
        Some(format!("{query}.md"))
    } else {
        None
    };
    for part in parts {
        if let Some(query) = as_path.as_deref() {
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

// -- family shares (phase 2: storage only) --------------------------------------------------------------------------
//
// The owner, 2026-10-22: the person shares only what they select with their family, read-only: a page, or a whole
// folder (« Domaines/Famille », a subject with its pages) including pages added to it later. Kept in state.json
// (`shares`): `{"pages": {<page id>: [<household member>…]}, "folders": {<folder path>: [...]}}` — a page by its stable
// id (it survives renames and moves), a folder by its path in the vault. The private health part is never shared,
// neither as a whole nor page by page. The transport between family places is not built yet.

/// Who a page is shared with: its own list and its folder's (inherited), each once. Empty for the health part.
pub fn shares_of(config: &Config, part: Part, path: &str, page: &Page) -> Vec<String> {
    if part == Part::Health {
        return Vec::new();
    }
    let state = read_state(config);
    let shares = state.get("shares").cloned().unwrap_or(json!({}));
    let mut out = strings(shares.get("pages").and_then(|p| p.get(page.id())));
    if let Some((folder, _)) = path.rsplit_once('/') {
        for who in strings(shares.get("folders").and_then(|f| f.get(folder))) {
            if !out.contains(&who) {
                out.push(who);
            }
        }
    }
    out
}

pub fn new_page_id() -> String {
    format!("p{}", py::token_hex(5))
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
    commit_with(config, part, path, before, after, who, action, summary, Dict::new())
}

/// [`commit`] with more fields in the log's line (a move's other path: `moved_to`, `moved_from`).
#[allow(clippy::too_many_arguments)]
fn commit_with(
    config: &Config,
    part: Part,
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    who: &Who,
    action: &str,
    summary: &str,
    extra: Dict,
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
    entry.extend(extra);
    let log = history_dir(config).join("log.jsonl");
    std::fs::create_dir_all(history_dir(config))?;
    let mut fh = std::fs::OpenOptions::new().append(true).create(true).open(&log)?;
    fh.write_all((py::Dumps::new().unicode().dumps(&Value::Object(entry.clone())) + "\n").as_bytes())?;
    py::chmod(&log.to_string_lossy(), 0o600)?;
    // a tasks page's change, whoever made it and however (a tool, the app, a restore, a page deleted): the tasks that
    // left the Carnet take their phone reminders with them (moved under « Archivées » or to the other part: not gone)
    if is_tasks_path(path) {
        let gone: Vec<String> = {
            let kept = task_ids_in(after.unwrap_or(""));
            task_ids_in(before.unwrap_or("")).into_iter().filter(|id| !kept.contains(id)).collect()
        };
        if !gone.is_empty() {
            let left = all_task_ids(config);
            for task in gone.iter().filter(|t| !left.contains(*t)) {
                if let Err(err) = remove_reminder(config, task) {
                    tracing::warn!(task = %task, error = %format!("{err:#}"), "notebook task's reminder not removed");
                }
            }
        }
    }
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

/// A free path for a page in `folder` (« Voyages/Lisbonne — nov. 2025.md », else « … 2.md »), `own` (its present
/// path) counting as free.
pub fn free_path(config: &Config, part: Part, folder: &str, title: &str, own: &str) -> String {
    let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
    let mut path = format!("{prefix}{}.md", file_name(title));
    let mut n = 2;
    while path != own && part_root(config, part).join(&path).exists() {
        path = format!("{prefix}{} {n}.md", file_name(title));
        n += 1;
    }
    path
}

/// A page moved (and maybe changed: `text`) from `from` to `to`, as two lines in the log that point to each other
/// (`moved_to`, `moved_from`): a restore of either moves it back. The caller holds the lock.
#[allow(clippy::too_many_arguments)]
pub fn move_page_locked(
    config: &Config,
    part: Part,
    from: &str,
    to: &str,
    text: &str,
    who: &Who,
    action: &str,
    summary: &str,
) -> anyhow::Result<()> {
    checked(to)?;
    let before = read_page_text(config, part, from);
    if from == to {
        if before.as_deref() != Some(text) {
            commit(config, part, to, before.as_deref(), Some(text), who, action, summary)?;
        }
        return Ok(());
    }
    if part_root(config, part).join(checked(to)?).exists() {
        return refuse(format!("a page is already there: {to}"));
    }
    let mut there = Dict::new();
    there.insert("moved_from".into(), json!(from));
    commit_with(config, part, to, None, Some(text), who, action, summary, there)?;
    let mut here = Dict::new();
    here.insert("moved_to".into(), json!(to));
    commit_with(config, part, from, before.as_deref(), None, who, action, summary, here)?;
    Ok(())
}

/// The page as it was right after change `change` (as it was before it, for a deletion), written back as a new
/// change by `who`. A move (the archives, a trip's new folder or name) is undone as a move back, under its old name.
pub fn restore(config: &Config, change: &str, who: &Who) -> anyhow::Result<Dict> {
    let _guard = lock();
    let Some(entry) = history(config, None, None, usize::MAX)
        .into_iter()
        .find(|e| e.get("id").and_then(Value::as_str) == Some(change))
    else {
        return refuse("no such change");
    };
    let (moved_from, moved_to) = (tk::str_or_empty(entry.get("moved_from")), tk::str_or_empty(entry.get("moved_to")));
    if !moved_from.is_empty() || !moved_to.is_empty() {
        let part = Part::parse(&tk::str_or_empty(entry.get("part")));
        let path = tk::str_or_empty(entry.get("path"));
        // (where the page was, where it went)
        let (old, new) = if moved_to.is_empty() { (moved_from, path) } else { (path, moved_to) };
        let version = if entry.get("after").map(|a| !a.is_null()).unwrap_or(false) {
            tk::str_or_empty(entry.get("after"))
        } else {
            tk::str_or_empty(entry.get("before"))
        };
        let old_text = history(config, Some(part), Some(&old), usize::MAX)
            .into_iter()
            .find(|e| tk::str_or_empty(e.get("moved_to")) == new)
            .and_then(|e| version_text(config, &tk::str_or_empty(e.get("before"))))
            .or_else(|| version_text(config, &version));
        let Some(old_text) = old_text else { return refuse("this version is no longer kept") };
        if read_page_text(config, part, &new).is_none() {
            return refuse("the page is no longer where it was moved");
        }
        if read_page_text(config, part, &old).is_some() {
            return refuse("a page is already where it was");
        }
        let date = tk::str_or_empty(entry.get("at"));
        move_page_locked(
            config,
            part,
            &new,
            &old,
            &old_text,
            who,
            "restore",
            &format!("as of {}", tk::head(&date, 10)),
        )?;
        // a trip brought back from the archives stays where the person put it
        if tk::str_or_empty(entry.get("action")) == "archive" {
            let id = Page::parse(&old_text, "").id();
            if !id.is_empty() {
                update_state(config, |s| {
                    let mut kept = strings(s.get("archive_kept"));
                    if !kept.contains(&id) {
                        kept.push(id.clone());
                    }
                    s.insert("archive_kept".into(), json!(kept));
                })?;
            }
        }
        return Ok(history(config, Some(part), Some(&old), 1).into_iter().next().unwrap_or_default());
    }
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
    /// The sections the facts went to (their titles).
    pub sections: Vec<String>,
    /// A subject made a trip by this write, or a trip's details changed (trip.rs).
    pub changed: bool,
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
    // the person themself: their one « Toi » page, whatever name the agent gives it
    let is_self = kind == "self" || self_word(&title);
    let found = if is_self {
        find(config, &[part], &w.self_path())
    } else {
        find(config, &[part], &title).or_else(|| {
            let contact = contact.trim();
            if contact.is_empty() {
                None
            } else {
                find(config, &[part], contact)
            }
        })
    };
    let (path, before, mut page, created) = match found {
        Some(f) => {
            let before = read_page_text(config, part, &f.path);
            (f.path, before, f.page, false)
        }
        None if is_self => (w.self_path(), None, Page::new("self", w.me), true),
        None => {
            let Some(folder) = w.folder(kind).filter(|_| KINDS.contains(&kind)) else {
                return refuse("kind is person, subject, area, self or trip");
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
    page.ensure_id();
    // the agent's judgement: a subject that is a trip becomes one (its content kept, put in the trip's outline)
    let converted = kind == "trip" && page.kind() == "subject";
    if converted {
        trip::make_trip(&mut page, &lang);
    }
    let mut names = page.aliases();
    names.extend(aliases.iter().cloned());
    if fold(&title) != fold(&page.title) && !page.named(&title) && !self_word(&title) {
        names.push(title.clone());
    }
    page.set_aliases(&names);
    if !contact.trim().is_empty() && page.contact().is_empty() {
        page.front.insert("contact".into(), json!(tk::head(contact.trim(), 200)));
    }
    // each fact, one a line, under the section of the page's outline it belongs to
    let page_kind = if KINDS.contains(&page.kind().as_str()) { page.kind() } else { kind.to_string() };
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for fact in facts {
        for piece in template::split_fact(fact) {
            let id = template::place(&page_kind, section, &piece);
            // a trip's packing list: one thing to tick a line
            let piece = if page_kind == "trip" && id == "packing" { trip::unticked(&piece) } else { piece };
            let title = if template::outline(&page_kind).is_empty() {
                if section.trim().is_empty() {
                    w.notes.to_string()
                } else {
                    section.trim().to_string()
                }
            } else {
                template::title(&page_kind, id, &lang)
            };
            match groups.iter_mut().find(|(t, _)| *t == title) {
                Some((_, list)) => list.push(piece),
                None => groups.push((title, vec![piece])),
            }
        }
    }
    let mut body = page.body.clone();
    let mut added: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    for (title, list) in &groups {
        let (next, new) = add_facts(&body, title, list, who, &today());
        if !new.is_empty() {
            sections.push(title.clone());
        }
        body = next;
        added.extend(new);
    }
    let body = set_summary(&body, w.brief, summary);
    let body = template::normalize(&page_kind, &lang, w.brief, &body).body;
    let summary_changed = summary_of(&body, w.brief) != summary_of(&page.body, w.brief);
    let aliases_changed = before.as_deref().map(|b| Page::parse(b, "").aliases() != page.aliases()).unwrap_or(false);
    if added.is_empty() && !created && !aliases_changed && !summary_changed && !converted {
        return Ok(Written { part: Some(part), path, title: page.title, created, added, sections, changed: false });
    }
    page.body = body;
    page.touch();
    let text = page.render();
    let summary = if converted && added.is_empty() {
        "trip".to_string()
    } else if !added.is_empty() {
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
        if created {
            "create"
        } else if converted {
            "trip"
        } else {
            "note"
        },
        &summary,
    )?;
    archive_logs(config, part, &path, &mut page, &lang)?;
    // a subject made a trip leaves the subjects for « Voyages » (or the archives, when it is over)
    let mut path = path;
    if part == Part::Main && page.kind() == "trip" {
        let today = chrono::Local::now().date_naive();
        if let Some((to, _)) = archive::place_one_locked(config, &path, &page, today, who)? {
            path = to;
        }
    }
    Ok(Written { part: Some(part), path, title: page.title, created, added, sections, changed: converted })
}

/// The page's dated logs (« Derniers échanges », « Historique ») cut to their latest lines, as a change of its own
/// (« rangement »): the lines taken out stay in the page's history.
fn archive_logs(config: &Config, part: Part, path: &str, page: &mut Page, lang: &str) -> anyhow::Result<()> {
    let (body, removed) = template::trim_logs(&page.kind(), lang, &page.body);
    if removed.is_empty() {
        return Ok(());
    }
    let before = page.render();
    page.body = body;
    let summary: Vec<String> = removed.iter().map(|l| split_attribution(l.trim().trim_start_matches("- ")).0).collect();
    commit(
        config,
        part,
        path,
        Some(&before),
        Some(&page.render()),
        &Who::tidy(config),
        "archive",
        &summary.join(" · "),
    )?;
    Ok(())
}

/// A page's body in its kind's outline (its sections in order, the empty ones left out).
pub fn in_outline(config: &Config, page: &Page) -> String {
    let lang = language(config);
    template::normalize(&page.kind(), &lang, words(&lang).brief, &page.body).body
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
    let lang = language(config);
    let w = words(&lang);
    let kind = page.kind();
    let (body, action, summary) = match text {
        None if old.trim().is_empty() && !brief.trim().is_empty() => {
            (page.body.clone(), "brief", format!("{} : {}", w.brief, tk::head(&tk::squeeze(brief), 200)))
        }
        Some(text) => {
            if section.trim().is_empty() {
                return refuse("give the section to replace");
            }
            let title = if template::outline(&kind).is_empty() {
                section.trim().to_string()
            } else {
                match template::resolve(&kind, section).or((fold(section) == "other").then_some("other")) {
                    Some(id) => template::title(&kind, id, &lang),
                    None => {
                        return refuse(format!(
                            "no section « {} » on a {kind} page: it is one of {}",
                            section.trim(),
                            template::ids(&kind).join(", ")
                        ))
                    }
                }
            };
            (
                set_section(&page.body, &title, text, who, &date),
                "section",
                format!("{title} : {}", tk::head(&tk::squeeze(text), 200)),
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
    page.body = template::normalize(&kind, &lang, w.brief, &set_summary(&body, w.brief, brief)).body;
    page.touch();
    let rendered = page.render();
    if before.as_deref() == Some(rendered.as_str()) {
        return refuse("nothing changed");
    }
    commit(config, found.part, &found.path, before.as_deref(), Some(&rendered), who, action, &summary)?;
    archive_logs(config, found.part, &found.path, &mut page, &lang)?;
    Ok((
        Written {
            part: Some(found.part),
            path: found.path,
            title: page.title,
            created: false,
            added: Vec::new(),
            sections: Vec::new(),
            changed: false,
        },
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
    page.body = in_outline(config, &Page { body, ..page.clone() });
    page.touch();
    let summary = format!("{} → {}", a.page.title, page.title);
    commit(config, part, &b.path, before_into.as_deref(), Some(&page.render()), who, "merge", &summary)?;
    commit(config, part, &a.path, before_from.as_deref(), None, who, "merge", &summary)?;
    Ok(Some(summary))
}

// -- re-filing into the outlines ----------------------------------------------------------------------------------------

/// The outlines' version: pages are re-filed once (no model) when it changes.
pub const OUTLINE_VERSION: i64 = 1;
/// Lines of « Autres » the nightly pass asks its model to place, a night at most (no big one-time bill).
pub const REFILE_PER_NIGHT: usize = 15;
/// The « Autres » lines the model already looked at (not sent again).
const TRIED_KEPT: usize = 500;

fn filed_words(lang: &str, n: usize) -> String {
    match lang {
        "fr" => format!("{n} fait(s) rangé(s)"),
        "de" => format!("{n} Fakt(en) einsortiert"),
        "es" => format!("{n} dato(s) ordenado(s)"),
        _ => format!("{n} fact(s) filed"),
    }
}

/// The pages written before the outlines (or by hand) put in their outline without a model: each fact where its
/// section or its words say (« anniversaire » → Dates, « frère » → Famille et proches…), the rest in « Autres » for
/// the nightly pass. Once per outline version, then on every agent's write to a page. Attributions and dates are
/// kept; each page's change is « rangement » in its history. Returns the pages re-filed.
pub fn refile_existing(config: &Config) -> anyhow::Result<usize> {
    let _guard = lock();
    if read_state(config).get("outline").and_then(Value::as_i64).unwrap_or(0) >= OUTLINE_VERSION {
        return Ok(0);
    }
    let lang = language(config);
    let w = words(&lang);
    let who = Who::tidy(config);
    let mut done = merge_self_locked(config, &who)?;
    for part in [Part::Main, Part::Health] {
        for (path, mut page) in all_pages(config, part) {
            if !KINDS.contains(&page.kind().as_str()) {
                continue;
            }
            let shaped = template::normalize(&page.kind(), &lang, w.brief, &page.body);
            if shaped.body.trim() == page.body.trim() {
                continue;
            }
            let before = page.render();
            page.body = shaped.body;
            let summary = format!("{} : {}", w.tidy, filed_words(&lang, shaped.moved));
            commit(config, part, &path, Some(&before), Some(&page.render()), &who, "refile", &summary)?;
            archive_logs(config, part, &path, &mut page, &lang)?;
            done += 1;
        }
    }
    update_state(config, |s| {
        s.insert("outline".into(), json!(OUTLINE_VERSION));
    })?;
    if done > 0 {
        tracing::info!(pages = done, "notebook pages put in their outline");
    }
    Ok(done)
}

static INTRODUCED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:je m'appelle |je suis |my name is |i am |i'm |ich heiße |ich bin |me llamo |soy )?(\p{Lu}[\p{L}'-]+(?: \p{Lu}[\p{L}'-]+)?)\s*(?:,|:| est | is | ist | es | vit | lives | wohnt | vive )",
    )
    .expect("valid regex")
});

/// The person's names as their agents know them: the « Toi » page's aliases, and the name their memory of the person
/// (USER.md) starts with (« Alex, vit à Lyon »).
fn self_names(config: &Config) -> Vec<String> {
    let w = words(&language(config));
    let mut names: Vec<String> =
        read_page_text(config, Part::Main, &w.self_path()).map(|t| Page::parse(&t, w.me).aliases()).unwrap_or_default();
    for agent in config.agents.values() {
        let folder = crate::agents_admin::profile_folder(config, agent);
        let Ok(found) = crate::memory_edit::read_entries(&folder, "user") else { continue };
        for entry in strings(found.get("entries")).iter().take(3) {
            let first = entry.trim().to_lowercase();
            let Some(c) = INTRODUCED.captures(entry.trim()) else { continue };
            let name = c[1].to_string();
            let generic = ["name", "nom", "user", "utilisateur", "the user", "name ist", "nombre"];
            if (first.starts_with(&name.to_lowercase()) || c.get(0).map(|m| m.start()) == Some(0))
                && !generic.contains(&fold(&name).as_str())
                && !names.iter().any(|n| fold(n) == fold(&name))
            {
                names.push(name);
            }
        }
    }
    names
}

/// The person pages that are the person themself (named « moi », or with their name as their agents know it)
/// merged into their « Toi » page (made if missing): facts with their attribution and dates, aliases (so links to the
/// old name still lead there), the old page's history kept. Once (state `self_merged`). Returns the pages merged.
fn merge_self_locked(config: &Config, who: &Who) -> anyhow::Result<usize> {
    if read_state(config).get("self_merged").map(py::truthy).unwrap_or(false) {
        return Ok(0);
    }
    let w = words(&language(config));
    let names = self_names(config);
    let mine: Vec<String> = all_pages(config, Part::Main)
        .into_iter()
        .filter(|(_, p)| p.kind() == "person")
        .filter(|(_, p)| {
            self_word(&p.title)
                || p.aliases().iter().any(|a| self_word(a))
                || names.iter().any(|n| p.named(n) || fold(&p.title) == fold(n))
        })
        .map(|(path, _)| path)
        .collect();
    let mut merged = 0;
    for path in mine {
        if read_page_text(config, Part::Main, &w.self_path()).is_none() {
            let page = Page::new("self", w.me);
            commit(config, Part::Main, &w.self_path(), None, Some(&page.render()), who, "create", "")?;
        }
        if merge_locked(config, Part::Main, &path, &w.self_path(), who)?.is_some() {
            merged += 1;
        }
    }
    update_state(config, |s| {
        s.insert("self_merged".into(), json!(true));
    })?;
    Ok(merged)
}

/// A line of « Autres » waiting for its section: (page title, kind, the fact without its attribution).
#[derive(Debug, Clone, PartialEq)]
pub struct Unsorted {
    pub page: String,
    pub kind: String,
    pub fact: String,
}

/// The « Autres » lines of the shared part the nightly pass has not tried yet, `limit` at most.
pub fn unsorted(config: &Config, limit: usize) -> Vec<Unsorted> {
    let tried = strings(read_state(config).get("refile_tried"));
    let lang = language(config);
    let mut out = Vec::new();
    for (_, page) in all_pages(config, Part::Main) {
        let kind = page.kind();
        if !KINDS.contains(&kind.as_str()) {
            continue;
        }
        let other = fold(&template::title(&kind, "other", &lang));
        for (section, facts) in page.sections() {
            if fold(&section) != other {
                continue;
            }
            for fact in facts {
                if fact.text.is_empty() || tried.contains(&fold(&fact.text)) {
                    continue;
                }
                out.push(Unsorted { page: page.title.clone(), kind: kind.clone(), fact: fact.text });
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

/// The « Autres » lines the model was shown, not sent again (placed or not).
pub(crate) fn mark_tried(config: &Config, facts: &[Unsorted]) -> std::io::Result<()> {
    if facts.is_empty() {
        return Ok(());
    }
    update_state(config, |s| {
        let mut tried = strings(s.get("refile_tried"));
        for f in facts {
            let key = fold(&f.fact);
            if !tried.contains(&key) {
                tried.push(key);
            }
        }
        let over = tried.len().saturating_sub(TRIED_KEPT);
        tried.drain(..over);
        s.insert("refile_tried".into(), json!(tried));
    })?;
    Ok(())
}

/// A fact of a page's « Autres » moved under `section` (an id of the page's outline), its line kept as it was.
pub(crate) fn refile_locked(
    config: &Config,
    query: &str,
    fact: &str,
    section: &str,
    who: &Who,
) -> anyhow::Result<bool> {
    let Some(found) = find(config, &[Part::Main], query) else { return Ok(false) };
    let kind = found.page.kind();
    let Some(id) = template::resolve(&kind, section).filter(|id| *id != "other") else { return Ok(false) };
    let lang = language(config);
    let other = template::title(&kind, "other", &lang);
    let mut lines = body_lines(&found.page.body);
    let Some(start) = section_index(&lines, &other) else { return Ok(false) };
    let end = section_end(&lines, start);
    let wanted = fold(fact);
    let Some(at) = (start + 1..end).find(|i| {
        let text = split_attribution(lines[*i].trim().trim_start_matches("- ")).0;
        !wanted.is_empty() && fold(&text) == wanted
    }) else {
        return Ok(false);
    };
    let line = lines.remove(at);
    let title = template::title(&kind, id, &lang);
    match section_index(&lines, &title) {
        Some(s) => {
            let mut e = section_end(&lines, s);
            while e > s + 1 && lines[e - 1].trim().is_empty() {
                e -= 1;
            }
            lines.insert(e, line);
        }
        None => {
            lines.push(String::new());
            lines.push(format!("## {title}"));
            lines.push(line);
        }
    }
    let before = read_page_text(config, Part::Main, &found.path);
    let mut page = found.page;
    page.body = in_outline(config, &Page { body: lines.join("\n") + "\n", ..page.clone() });
    let summary = format!("{} : {} → {title}", words(&lang).tidy, tk::head(fact, 150));
    commit(config, Part::Main, &found.path, before.as_deref(), Some(&page.render()), who, "refile", &summary)?;
    archive_logs(config, Part::Main, &found.path, &mut page, &lang)?;
    Ok(true)
}

// -- tasks ---------------------------------------------------------------------------------------------------------
//
// Few and real (the owner, 2026-10-22: « sinon ça sera rempli de fausses tâches »). A fact is not a task: a
// birthday or someone else's deadline stays a dated fact on its page (`upcoming_dates`). A task exists only when the
// person asks (« rappelle-moi… »: origin `asked`), states their own commitment with a deadline (`own`), or accepts a
// proposal from an agent (`accepted`); proposals are budgeted per install (`propose`) and a declined kind is not
// proposed again. Accepted proposals left untouched long after their date are archived by the nightly pass (a section
// of the tasks page, never deleted, not counted).

/// Where a task comes from.
pub const ORIGINS: [&str; 3] = ["asked", "own", "accepted"];
/// Proposals an agent may make, per install, in 7 days.
pub const PROPOSALS_PER_WEEK: usize = 3;
/// What an agent may propose.
pub const PROPOSAL_KINDS: [&str; 9] =
    ["birthday", "anniversary", "gift", "follow_up", "deadline", "other", "warranty", "renewal", "expiry"];
/// An accepted proposal still open this many days after its date (or after it was made, without a date): archived.
pub const ARCHIVE_AFTER_DAYS: i64 = 14;

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
    /// asked, own or accepted (a task written by hand in Obsidian counts as asked).
    pub origin: String,
    /// Under the tasks page's « Archivées ».
    pub archived: bool,
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
            origin: "asked".into(),
            archived: false,
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
               "links": self.links, "by": self.who, "date": self.date, "part": self.part.as_str(),
               "origin": self.origin, "archived": self.archived})
    }
}

fn task_meta(state: &Dict) -> Dict {
    match state.get("tasks") {
        Some(Value::Object(m)) => m.clone(),
        _ => Dict::new(),
    }
}

/// A tasks page's tasks, those under its archive section marked so; their origin from the state.
fn tasks_in(body: &str, part: Part, archived: &str, meta: &Dict) -> Vec<Task> {
    let mut out = Vec::new();
    let mut in_archive = false;
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("## ") {
            in_archive = fold(name) == fold(archived);
            continue;
        }
        if let Some(mut task) = Task::parse(line, part) {
            task.archived = in_archive;
            if let Some(origin) = meta.get(&task.id).and_then(|m| m.get("origin")).and_then(Value::as_str) {
                task.origin = origin.to_string();
            }
            out.push(task);
        }
    }
    out
}

/// The tasks of a part, in the page's order (archived ones included, marked).
pub fn tasks(config: &Config, part: Part) -> Vec<Task> {
    let w = words(&language(config));
    let text = read_page_text(config, part, &w.tasks_path()).unwrap_or_default();
    tasks_in(&text, part, w.archived, &task_meta(&read_state(config)))
}

/// The open tasks that count (not archived), what the person asked or committed to first, then by date.
pub fn open_tasks(config: &Config, part: Part) -> Vec<Task> {
    let mut open: Vec<Task> = tasks(config, part).into_iter().filter(|t| !t.done && !t.archived).collect();
    open.sort_by(|a, b| {
        let rank = |t: &Task| if t.origin == "accepted" { 1 } else { 0 };
        (rank(a), a.due.is_empty(), &a.due).cmp(&(rank(b), b.due.is_empty(), &b.due))
    });
    open
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

/// What `add_task` did.
#[derive(Debug, Clone)]
pub struct Added {
    pub task: Task,
    /// A phone reminder asked for.
    pub reminder: bool,
    /// The same open task was already there: it is returned, nothing added.
    pub existing: bool,
    /// That task had no date: it got this one.
    pub dated: bool,
}

/// A task's title as « the same task » compares it: its `[[links]]` read as their names, folded (case, accents,
/// punctuation, spaces).
fn same_task_key(title: &str) -> String {
    let plain = LINK.replace_all(title, |c: &regex::Captures| c[1].to_string());
    fold(&plain)
}

/// A task line's keys: its whole title, and without the page link added at its end (« … [[Maison]] »).
fn task_keys(title: &str) -> [String; 2] {
    let bare = match title.trim_end().strip_suffix("]]").and_then(|t| t.rfind("[[").map(|i| &title[..i])) {
        Some(head) => same_task_key(head),
        None => same_task_key(title),
    };
    [same_task_key(title), bare]
}

/// A new task in the part's tasks page (`origin`: asked, own or accepted — accepted needs the proposal's id);
/// with a date, it also becomes a phone reminder (when the person shares their reminders with this agent, or it is
/// the person's own task). `due` may carry a time, which the phone gets. The same open task already there (same
/// title once folded, same part, the same date or one of them without) is returned instead (`existing`): when it
/// had no date it takes this one.
#[allow(clippy::too_many_arguments)]
pub fn add_task(
    config: &Config,
    part: Part,
    title: &str,
    due: &str,
    page: &str,
    origin: &str,
    proposal: &str,
    who: &Who,
) -> anyhow::Result<Added> {
    let _guard = lock();
    add_task_locked(config, part, title, due, page, origin, proposal, who)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn add_task_locked(
    config: &Config,
    part: Part,
    title: &str,
    due: &str,
    page: &str,
    origin: &str,
    proposal: &str,
    who: &Who,
) -> anyhow::Result<Added> {
    let lang = fix_language(config)?;
    let w = words(&lang);
    let origin = if origin.trim().is_empty() { "asked" } else { origin.trim() };
    if !ORIGINS.contains(&origin) {
        return refuse("origin is asked (the person asked), own (their own commitment with a deadline) or accepted (they said yes to your proposal)");
    }
    let title = tk::head(&tk::squeeze(title.replace(['\n', '^'], " ").trim()), 300).to_string();
    if title.is_empty() {
        return refuse("a task needs a title");
    }
    if origin == "accepted" {
        let open = proposals(config)
            .into_iter()
            .any(|p| tk::str_or_empty(p.get("id")) == proposal.trim() && tk::str_or_empty(p.get("state")) == "open");
        if !open {
            return refuse("an accepted task needs the id of the proposal the person said yes to (notebook_task action propose gives it)");
        }
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
    let mut title_line = title.clone();
    let page = page.trim();
    if !page.is_empty() && !title_line.contains("[[") {
        let target = find(config, &[part], page).map(|f| f.page.title).unwrap_or_else(|| file_name(page));
        title_line = format!("{title_line} [[{target}]]");
    }
    // the same open task already there (an agent filling the Carnet from its memory asks again): no second one
    let existing = tasks_in(&doc.body, part, w.archived, &task_meta(&read_state(config)));
    let wanted = [same_task_key(&title), same_task_key(&title_line)];
    if let Some(same) = existing.iter().find(|t| {
        !t.done
            && !t.archived
            && (t.due == date || t.due.is_empty() || date.is_empty())
            && task_keys(&t.title).iter().any(|k| !k.is_empty() && wanted.contains(k))
    }) {
        let mut same = Task { links: links_in(&same.title), ..same.clone() };
        if !same.id.is_empty() && same.due.is_empty() && !date.is_empty() {
            // it had no date: it gets this one (and its phone reminder)
            let mut lines = body_lines(&doc.body);
            if let Some(index) = lines.iter().position(|l| Task::parse(l, part).is_some_and(|t| t.id == same.id)) {
                same.due = date.clone();
                lines[index] = same.render();
                doc.body = lines.join("\n") + "\n";
                doc.touch();
                commit(config, part, &path, before.as_deref(), Some(&doc.render()), who, "task_due", &same.title)?;
                let reminder = link_reminder(config, &same, due, who)?;
                return Ok(Added { task: same, reminder, existing: true, dated: true });
            }
        }
        return Ok(Added { task: same, reminder: false, existing: true, dated: false });
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
        origin: origin.to_string(),
        archived: false,
    };
    // the open tasks above the archive section
    let mut lines = body_lines(&doc.body);
    let at = section_index(&lines, w.archived).map(|i| {
        let mut at = i;
        while at > 0 && lines[at - 1].trim().is_empty() {
            at -= 1;
        }
        at
    });
    match at {
        Some(at) => lines.insert(at, task.render()),
        None => lines.push(task.render()),
    }
    doc.body = lines.join("\n") + "\n";
    doc.touch();
    commit(config, part, &path, before.as_deref(), Some(&doc.render()), who, "task_add", &title)?;
    let (id, proposal) = (task.id.clone(), proposal.trim().to_string());
    update_state(config, |s| {
        let mut meta = task_meta(s);
        meta.insert(
            id,
            json!({"origin": origin, "proposal": if proposal.is_empty() { Value::Null } else { json!(proposal) }}),
        );
        s.insert("tasks".into(), Value::Object(meta));
        if !proposal.is_empty() {
            set_proposal_state(s, &proposal, "accepted");
        }
    })?;
    let task = Task { links: links_in(&task.title), ..task };
    let linked = if task.due.is_empty() { false } else { link_reminder(config, &task, due, who)? };
    Ok(Added { task, reminder: linked, existing: false, dated: false })
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

/// Accepted proposals still open `ARCHIVE_AFTER_DAYS` after their date (or after they were made) moved under the
/// tasks page's « Archivées » (the nightly pass; free). Returns how many.
pub fn archive_stale(config: &Config, today_date: chrono::NaiveDate) -> anyhow::Result<usize> {
    let _guard = lock();
    let w = words(&language(config));
    let path = w.tasks_path();
    let meta = task_meta(&read_state(config));
    let mut moved = 0;
    for part in [Part::Main, Part::Health] {
        let Some(before) = read_page_text(config, part, &path) else { continue };
        let mut doc = Page::parse(&before, w.tasks);
        let all = tasks_in(&doc.body, part, w.archived, &meta);
        let stale: Vec<String> = all
            .iter()
            .filter(|t| !t.done && !t.archived && t.origin == "accepted")
            .filter(|t| {
                let since = if t.due.is_empty() { &t.date } else { &t.due };
                chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d")
                    .map(|d| (today_date - d).num_days() > ARCHIVE_AFTER_DAYS)
                    .unwrap_or(false)
            })
            .map(|t| t.id.clone())
            .collect();
        if stale.is_empty() {
            continue;
        }
        let mut lines = body_lines(&doc.body);
        let mut taken: Vec<String> = Vec::new();
        lines.retain(|l| {
            let hit = Task::parse(l, part).is_some_and(|t| stale.contains(&t.id));
            if hit {
                taken.push(l.clone());
            }
            !hit
        });
        match section_index(&lines, w.archived) {
            Some(start) => {
                let end = section_end(&lines, start);
                for (n, l) in taken.iter().enumerate() {
                    lines.insert(end + n, l.clone());
                }
            }
            None => {
                lines.push(String::new());
                lines.push(format!("## {}", w.archived));
                lines.extend(taken.iter().cloned());
            }
        }
        doc.body = lines.join("\n") + "\n";
        doc.touch();
        let who = Who { by: "nightly".into(), name: w.archived.into(), agent: None };
        commit(
            config,
            part,
            &path,
            Some(&before),
            Some(&doc.render()),
            &who,
            "archive",
            &format!("{} tasks", taken.len()),
        )?;
        moved += taken.len();
    }
    Ok(moved)
}

// -- proposals ------------------------------------------------------------------------------------------------------

/// The proposals made (the last ones kept): {id, agent, kind, title, due, at, state: open|accepted|declined}.
pub fn proposals(config: &Config) -> Vec<Dict> {
    match read_state(config).get("proposals") {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_object().cloned()).collect(),
        _ => Vec::new(),
    }
}

fn set_proposal_state(state: &mut Dict, id: &str, to: &str) {
    if let Some(Value::Array(items)) = state.get_mut("proposals") {
        for item in items.iter_mut() {
            if item.get("id").and_then(Value::as_str) == Some(id) {
                item["state"] = json!(to);
            }
        }
    }
}

/// The kinds of proposals the person declined (none is proposed again).
pub fn declined(config: &Config) -> Vec<String> {
    strings(read_state(config).get("declined"))
}

/// Whether an agent may propose a task now: Ok(the proposal's id) within the week's budget and for a kind the person
/// did not decline; Err(why not) otherwise (a refusal for the agent: then it doesn't propose).
pub fn propose(config: &Config, agent: &str, kind: &str, title: &str, due: &str) -> anyhow::Result<String> {
    propose_for(config, agent, kind, title, due, "")
}

/// `propose` with the Carnet page the task would belong to (`page`, its name; the records' reminders: their page).
/// `agent` empty: the Carnet itself (the nightly pass), whose proposals the person answers in the app.
pub fn propose_for(
    config: &Config,
    agent: &str,
    kind: &str,
    title: &str,
    due: &str,
    page: &str,
) -> anyhow::Result<String> {
    let _guard = lock();
    let kind = kind.trim();
    if !PROPOSAL_KINDS.contains(&kind) {
        return refuse(format!("kind is one of {}", PROPOSAL_KINDS.join(", ")));
    }
    if declined(config).iter().any(|k| k == kind) {
        return refuse(format!(
            "The person doesn't want proposals of this kind ({kind}): don't propose it. If they ask, add it as asked."
        ));
    }
    let week_ago = chrono::Utc::now() - chrono::Duration::days(7);
    let recent = proposals(config)
        .iter()
        .filter(|p| {
            chrono::DateTime::parse_from_rfc3339(&tk::str_or_empty(p.get("at"))).map(|d| d > week_ago).unwrap_or(false)
        })
        .count();
    if recent >= PROPOSALS_PER_WEEK {
        return refuse("Enough tasks were proposed to the person this week: don't propose this one.");
    }
    let id = format!("q{}", py::token_hex(4));
    let item = json!({"id": id, "agent": agent, "kind": kind, "title": tk::head(&tk::squeeze(title), 300),
                      "due": tk::head(due.trim(), 40), "at": tk::now_iso_seconds(), "state": "open",
                      "page": tk::head(page.trim(), 200)});
    update_state(config, |s| {
        let mut all = match s.get("proposals") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        all.push(item);
        let excess = all.len().saturating_sub(100);
        all.drain(..excess);
        s.insert("proposals".into(), Value::Array(all));
    })?;
    Ok(id)
}

/// The person said yes to a proposal in the app: its task added (origin accepted, on its page, with its date and so
/// its phone reminder).
pub fn accept(config: &Config, proposal: &str, who: &Who) -> anyhow::Result<(Task, bool)> {
    let _guard = lock();
    let Some(found) = proposals(config).into_iter().find(|p| tk::str_or_empty(p.get("id")) == proposal.trim()) else {
        return refuse("unknown proposal id");
    };
    if tk::str_or_empty(found.get("state")) != "open" {
        return refuse("this proposal was already answered");
    }
    let added = add_task_locked(
        config,
        Part::Main,
        &tk::str_or_empty(found.get("title")),
        &tk::str_or_empty(found.get("due")),
        &tk::str_or_empty(found.get("page")),
        "accepted",
        proposal.trim(),
        who,
    )?;
    // the same task already there (no duplicate): the proposal is answered all the same
    update_state(config, |s| set_proposal_state(s, proposal.trim(), "accepted"))?;
    Ok((added.task, added.reminder))
}

/// The open proposals (the newest first).
pub fn open_proposals(config: &Config) -> Vec<Dict> {
    let mut open: Vec<Dict> =
        proposals(config).into_iter().filter(|p| tk::str_or_empty(p.get("state")) == "open").collect();
    open.reverse();
    open
}

/// The person said no to a proposal: closed, and with `never` its kind is not proposed again, written as a
/// preference on the Carnet's « Préférences » page (readable and undoable by the person).
pub fn decline(config: &Config, proposal: &str, never: bool, who: &Who) -> anyhow::Result<Option<String>> {
    let _guard = lock();
    let Some(found) = proposals(config).into_iter().find(|p| tk::str_or_empty(p.get("id")) == proposal.trim()) else {
        return refuse("unknown proposal id");
    };
    let kind = tk::str_or_empty(found.get("kind"));
    update_state(config, |s| {
        set_proposal_state(s, proposal.trim(), "declined");
        if never {
            let mut kinds = strings(s.get("declined"));
            if !kinds.contains(&kind) {
                kinds.push(kind.clone());
            }
            s.insert("declined".into(), json!(kinds));
        }
    })?;
    if !never {
        return Ok(None);
    }
    let lang = language(config);
    let fact = no_proposals_fact(&lang, &kind);
    let w = words(&lang);
    note_locked(config, Part::Main, "self", w.me, "agents", std::slice::from_ref(&fact), &[], "", "", who)?;
    Ok(Some(fact))
}

fn no_proposals_fact(lang: &str, kind: &str) -> String {
    let index = PROPOSAL_KINDS.iter().position(|k| *k == kind).unwrap_or(5);
    let names: [[&str; 9]; 4] = [
        [
            "anniversaires",
            "fêtes et anniversaires de mariage",
            "cadeaux",
            "relances",
            "échéances",
            "ce genre de tâches",
            "fins de garantie",
            "renouvellements d'abonnements",
            "papiers qui expirent",
        ],
        [
            "birthdays",
            "anniversaries",
            "gifts",
            "follow-ups",
            "deadlines",
            "this kind of task",
            "warranties ending",
            "subscription renewals",
            "papers expiring",
        ],
        [
            "Geburtstage",
            "Jahrestage",
            "Geschenke",
            "Nachfassen",
            "Fristen",
            "diese Art von Aufgaben",
            "endende Garantien",
            "Abo-Verlängerungen",
            "ablaufende Ausweise",
        ],
        [
            "cumpleaños",
            "aniversarios",
            "regalos",
            "seguimientos",
            "plazos",
            "este tipo de tareas",
            "garantías que terminan",
            "renovaciones de suscripciones",
            "documentos que caducan",
        ],
    ];
    let (row, prefix) = match lang {
        "fr" => (0, "Pas de propositions de tâches pour les"),
        "de" => (2, "Keine Aufgabenvorschläge für"),
        "es" => (3, "Sin propuestas de tareas para"),
        _ => (1, "No task proposals for"),
    };
    let name = names[row][index];
    if index == 5 {
        let _ = prefix;
        return match lang {
            "fr" => "Pas de propositions pour ce genre de tâches".into(),
            "de" => "Keine Vorschläge für diese Art von Aufgaben".into(),
            "es" => "Sin propuestas para este tipo de tareas".into(),
            _ => "No proposals for this kind of task".into(),
        };
    }
    format!("{prefix} {name}")
}

// -- dates to come ----------------------------------------------------------------------------------------------------

const MONTHS: [(&str, u32); 52] = [
    ("janvier", 1),
    ("fevrier", 2),
    ("mars", 3),
    ("avril", 4),
    ("mai", 5),
    ("juin", 6),
    ("juillet", 7),
    ("aout", 8),
    ("septembre", 9),
    ("octobre", 10),
    ("novembre", 11),
    ("decembre", 12),
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
    ("januar", 1),
    ("februar", 2),
    ("marz", 3),
    ("juni", 6),
    ("juli", 7),
    ("oktober", 10),
    ("dezember", 12),
    ("enero", 1),
    ("febrero", 2),
    ("marzo", 3),
    ("abril", 4),
    ("mayo", 5),
    ("junio", 6),
    ("julio", 7),
    ("agosto", 8),
    ("septiembre", 9),
    ("octubre", 10),
    ("noviembre", 11),
    ("diciembre", 12),
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
];

static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").expect("valid regex"));
static DAY_MONTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2})(?:er|st|nd|rd|th|\.)?\s+([a-z]{3,10})\b(?:\s+(\d{4}))?").expect("valid regex")
});
static MONTH_DAY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([a-z]{3,10})\s+(\d{1,2})(?:st|nd|rd|th)?\b(?:,?\s+(\d{4}))?").expect("valid regex")
});
static NUMERIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,2})/(\d{1,2})(?:/(\d{4}))?\b").expect("valid regex"));
static YEARLY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)anniv|birthday|geburtstag|cumple|f[eê]te|name ?day|jahrestag|aniversario|mariage|wedding|hochzeit|boda",
    )
    .expect("valid regex")
});

/// A date in a fact: (month, day, year when given). Day-month order first (« 14 novembre », « 14/11 »).
pub fn date_in(text: &str) -> Option<(u32, u32, Option<i32>)> {
    if let Some(c) = ISO_DATE.captures(text) {
        return Some((c[2].parse().ok()?, c[3].parse().ok()?, c[1].parse().ok()));
    }
    let folded = tk::ascii_fold(&text.to_lowercase());
    let month = |name: &str| MONTHS.iter().find(|(m, _)| *m == name).map(|(_, n)| *n);
    for c in DAY_MONTH.captures_iter(&folded) {
        if let (Ok(day), Some(m)) = (c[1].parse::<u32>(), month(&c[2])) {
            return Some((m, day, c.get(3).and_then(|y| y.as_str().parse().ok())));
        }
    }
    for c in MONTH_DAY.captures_iter(&folded) {
        if let (Some(m), Ok(day)) = (month(&c[1]), c[2].parse::<u32>()) {
            return Some((m, day, c.get(3).and_then(|y| y.as_str().parse().ok())));
        }
    }
    if let Some(c) = NUMERIC.captures(&folded) {
        let (day, m): (u32, u32) = (c[1].parse().ok()?, c[2].parse().ok()?);
        if (1..=12).contains(&m) && (1..=31).contains(&day) {
            return Some((m, day, c.get(3).and_then(|y| y.as_str().parse().ok())));
        }
    }
    None
}

/// The dated facts coming in the next `days` days: birthdays and other yearly dates (a fact naming one, or a date
/// without a year) at their next occurrence, one-off dates when still ahead. Only close people's pages unless `all`
/// (then every person, subject and area). Never tasks: the app shows them as « Dates à venir ».
pub fn upcoming_dates(config: &Config, days: i64, all: bool, today_date: chrono::NaiveDate) -> Vec<Value> {
    let w = words(&language(config));
    let mut out: Vec<(i64, Value)> = Vec::new();
    for (path, page) in all_pages(config, Part::Main) {
        let kind = page.kind();
        if !KINDS.contains(&kind.as_str()) || (!all && !(kind == "person" && page.close())) {
            continue;
        }
        for (section, facts) in page.sections() {
            if fold(&section) == fold(w.brief) {
                continue;
            }
            for fact in facts {
                let Some((month, day, year)) = date_in(&fact.text) else { continue };
                let yearly = YEARLY.is_match(&fact.text) || year.is_none();
                let next = if yearly {
                    let this = chrono::NaiveDate::from_ymd_opt(today_date.year(), month, day);
                    match this {
                        Some(d) if d >= today_date => Some(d),
                        _ => chrono::NaiveDate::from_ymd_opt(today_date.year() + 1, month, day),
                    }
                } else {
                    year.and_then(|y| chrono::NaiveDate::from_ymd_opt(y, month, day)).filter(|d| *d >= today_date)
                };
                let Some(next) = next else { continue };
                let until = (next - today_date).num_days();
                if until > days {
                    continue;
                }
                let age = if yearly { year.map(|y| next.year() - y) } else { None };
                out.push((
                    until,
                    json!({"date": next.format("%Y-%m-%d").to_string(), "in_days": until, "yearly": yearly,
                                        "years": age, "fact": fact.text, "page": page.title, "path": path,
                                        "kind": kind, "close": page.close(), "page_id": page.id(),
                                        "by": fact.who, "noted": fact.date}),
                ));
            }
        }
    }
    out.sort_by_key(|a| a.0);
    out.into_iter().map(|(_, v)| v).collect()
}

/// A person page marked close (or not): their dates appear in « Dates à venir ».
pub fn set_close(config: &Config, query: &str, close: bool, who: &Who) -> anyhow::Result<Option<String>> {
    let _guard = lock();
    let Some(found) = find(config, &[Part::Main], query) else { return Ok(None) };
    if found.page.close() == close {
        return Ok(Some(found.path));
    }
    let before = read_page_text(config, Part::Main, &found.path);
    let mut page = found.page;
    page.ensure_id();
    page.front.remove("proche");
    if close {
        page.front.insert("close".into(), json!(true));
    } else {
        page.front.remove("close");
    }
    page.touch();
    commit(
        config,
        Part::Main,
        &found.path,
        before.as_deref(),
        Some(&page.render()),
        who,
        "close",
        if close { "close" } else { "not close" },
    )?;
    Ok(Some(found.path))
}

/// A subject's or an area's icon chosen by the person (`icon:` in its frontmatter: an id of the apps' table,
/// scripts/notebook-icons.py), or taken off (empty: the apps pick one from its words again). Never set by an agent.
pub fn set_icon(config: &Config, query: &str, icon: &str, who: &Who) -> anyhow::Result<Option<String>> {
    let icon = icon.trim();
    if !icon.is_empty()
        && !(icon.len() <= 40 && icon.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
    {
        return Err(NotebookError("an icon is a short id (a-z, 0-9, -)".into()).into());
    }
    let _guard = lock();
    let Some(found) = find(config, &[Part::Main], query) else { return Ok(None) };
    if found.page.icon().unwrap_or_default() == icon {
        return Ok(Some(found.path));
    }
    let before = read_page_text(config, Part::Main, &found.path);
    let mut page = found.page;
    page.ensure_id();
    if icon.is_empty() {
        page.front.remove("icon");
    } else {
        page.front.insert("icon".into(), json!(icon));
    }
    page.touch();
    let summary = if icon.is_empty() { "automatic" } else { icon };
    commit(config, Part::Main, &found.path, before.as_deref(), Some(&page.render()), who, "icon", summary)?;
    Ok(Some(found.path))
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

/// The task a reminder's notes carry the mark of (exactly: `^t12ab…` whole).
fn marked_task(notes: &str) -> Option<String> {
    MARK.captures(notes).and_then(|c| {
        let end = c.get(0)?.end();
        let next = notes[end..].chars().next();
        (!next.is_some_and(|ch| ch.is_ascii_alphanumeric())).then(|| c[1].to_string())
    })
}

/// The phone's open reminders (as it last shared them) carrying `task`'s mark: their ids.
fn open_reminders_of(items: &[Value], task: &str) -> Vec<String> {
    items
        .iter()
        .filter(|i| !i.get("completed").map(py::truthy).unwrap_or(false))
        .filter(|i| marked_task(&tk::str_or_empty(i.get("notes"))).as_deref() == Some(task))
        .map(|i| tk::str_or_empty(i.get("id")))
        .filter(|id| !id.is_empty())
        .collect()
}

fn shared_items(config: &Config) -> Vec<Value> {
    match crate::reminders::read(config).get("items") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// Whether `path` is a tasks page (in any of the Carnet's languages).
fn is_tasks_path(path: &str) -> bool {
    WORDS.iter().any(|w| w.tasks_path() == path)
}

/// The ids of the tasks in a tasks page's text, in order.
fn task_ids_in(text: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for task in text.lines().filter_map(|l| Task::parse(l, Part::Main)) {
        if !task.id.is_empty() && !ids.contains(&task.id) {
            ids.push(task.id);
        }
    }
    ids
}

/// Every task id in the Carnet: both parts, archived and done ones included.
fn all_task_ids(config: &Config) -> HashSet<String> {
    let mut ids = HashSet::new();
    for part in [Part::Main, Part::Health] {
        for w in &WORDS {
            if let Some(text) = read_page_text(config, part, &w.tasks_path()) {
                ids.extend(task_ids_in(&text));
            }
        }
    }
    ids
}

fn ask_removal(config: &Config, reminder: &str, task: &str) -> anyhow::Result<bool> {
    let args = json!({"id": reminder, "mark": reminder_mark(task)});
    Ok(crate::reminders::enqueue_once(config, "notebook", "remove", args)?.is_some())
}

/// A task gone from the Carnet (deleted, not moved): its add cancelled while it still waits for the phone, else the
/// phone asked to remove its reminders (only those carrying its mark); its link dropped. Returns the requests made.
fn remove_reminder(config: &Config, task: &str) -> anyhow::Result<usize> {
    let link = links(&read_state(config)).get(task).cloned();
    let cancelled = match &link {
        Some(l) => crate::reminders::cancel_pending(config, &tk::str_or_empty(l.get("op")))?,
        None => false,
    };
    let mut asked = 0;
    if !cancelled {
        let ids = open_reminders_of(&shared_items(config), task);
        if ids.is_empty() && link.is_some() {
            // added but not shared back yet: the phone finds it by its mark
            asked += usize::from(ask_removal(config, "", task)?);
        }
        for id in ids {
            asked += usize::from(ask_removal(config, &id, task)?);
        }
    }
    if link.is_some() {
        update_state(config, |s| {
            let mut map = links(s);
            map.remove(task);
            s.insert("reminders".into(), Value::Object(map));
        })?;
    }
    Ok(asked)
}

/// The reminders the Carnet's tasks left behind (before 2026-10-23 a task removed kept its reminder): a link to a
/// task no longer in the Carnet removed with its reminder; on the phone's copy, the open reminders of this Carnet's
/// tasks (those it made: a phone shared with several installs carries other Carnets' marks, never touched) whose
/// task is gone, and all but one of several carrying the same task's mark. Idempotent (each request asked once).
/// At the bridge's start, each night and after each phone share. Returns the requests made.
pub fn clean_reminders(config: &Config) -> anyhow::Result<usize> {
    if !root(config).exists() {
        return Ok(0);
    }
    let _guard = lock();
    clean_reminders_locked(config, &shared_items(config))
}

fn clean_reminders_locked(config: &Config, items: &[Value]) -> anyhow::Result<usize> {
    let existing = all_task_ids(config);
    let state = read_state(config);
    let map = links(&state);
    let mut asked = 0;
    for task in map.keys().filter(|t| !existing.contains(*t)) {
        asked += remove_reminder(config, task)?;
    }
    let ours: HashSet<String> = task_meta(&state).keys().chain(map.keys()).cloned().collect();
    let mut kept: HashSet<String> = HashSet::new();
    for item in items {
        if item.get("completed").map(py::truthy).unwrap_or(false) {
            continue;
        }
        let Some(task) = marked_task(&tk::str_or_empty(item.get("notes"))) else { continue };
        let id = tk::str_or_empty(item.get("id"));
        if !ours.contains(&task) || id.is_empty() {
            continue;
        }
        if existing.contains(&task) && kept.insert(task.clone()) {
            continue; // the task's one reminder
        }
        asked += usize::from(ask_removal(config, &id, &task)?);
    }
    Ok(asked)
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
    let item = match data.get("items") {
        Some(Value::Array(items)) => {
            items.iter().find(|i| marked_task(&tk::str_or_empty(i.get("notes"))).as_deref() == Some(task)).cloned()
        }
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
        let Some(task) = marked_task(&notes) else { continue };
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
    // what removed tasks left on the phone, and duplicates, taken off
    if let Err(err) = clean_reminders_locked(config, items) {
        tracing::warn!(error = %format!("{err:#}"), "notebook reminders not cleaned");
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

/// An agent's web page's link to a Carnet page: (its id, its part).
type PageLink = Option<(String, String)>;

/// The files the agents made (audio, pages) or were sent (uploads), where the bridge keeps them: by agent, the newest
/// first, each with the Carnet pages that name it. The web pages the agents made (`create_page`) are there too, as
/// their own type (`page`, with their `page` id), first under the Carnet page they are linked to (`linked`); those of
/// the private health part (made by the health agent, or linked there) only in that part's list (`part` Health), never
/// in the shared one.
pub fn documents(config: &Config, part: Part) -> Vec<Value> {
    let health = part == Part::Health;
    // (agent, page id) → the Carnet page it is linked to
    let mut links: Vec<(String, String, PageLink)> = Vec::new();
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
        if let Some(root) = agent.upload_dir_host.as_deref().filter(|r| !r.is_empty() && !health) {
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
            if !health && file.path().is_file() && !name.starts_with('.') && file_type(&name) == "audio" {
                let media = format!("{}/{}/{name}", crate::agent_tools::MEDIA_PREFIX, agent.name);
                add(&agent.name, name, "made", &file.path(), Some(media), None);
            }
        }
        for page in crate::pages::list_pages(config, &agent.name) {
            if crate::pages::private(&page) != health {
                continue;
            }
            let id = tk::str_or_empty(page.get("page"));
            links.push((agent.name.clone(), format!("{id}.html"), crate::pages::link(&page)));
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
    let pages = all_pages(config, part);
    let mut out: Vec<Value> = Vec::new();
    found.sort_by_key(|d| std::cmp::Reverse(tk::str_or_empty(d.get("date"))));
    for mut item in found {
        let file = tk::str_or_empty(item.get("file"));
        let agent = tk::str_or_empty(item.get("agent"));
        let made_page = links.iter().find(|(a, f, _)| *a == agent && *f == file);
        let linked = made_page.and_then(|(_, _, l)| l.as_ref()).and_then(|(id, _)| {
            pages
                .iter()
                .find(|(_, p)| p.id() == *id)
                .map(|(path, p)| json!({"title": p.title, "path": path, "kind": p.kind()}))
        });
        if let Some((_, f, _)) = made_page {
            item.insert("page".into(), json!(f.trim_end_matches(".html")));
            item.insert("linked".into(), linked.clone().unwrap_or(Value::Null));
        }
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
        let subjects: Vec<Value> = linked.into_iter().chain(subjects).fold(Vec::new(), |mut all, s| {
            if !all.iter().any(|a: &Value| a["path"] == s["path"]) {
                all.push(s);
            }
            all
        });
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
    let docs = documents(config, Part::Main);
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
        // a health agent's memory may hold health data: not copied where the nightly pass's model would read it
        if !enabled(config, &agent.name) || health_agent(config, &agent.name) {
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
        copied += inbox_add_locked(config, agent, &entries, "seed")?;
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

/// `entries` added under the agent's name at the end of « À ranger » (those already there left out), for the nightly
/// pass to sort. Returns how many were added.
fn inbox_add_locked(
    config: &Config,
    agent: &crate::config::AgentConfig,
    entries: &[String],
    action: &str,
) -> anyhow::Result<usize> {
    if entries.is_empty() {
        return Ok(0);
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
    let (body, added) = add_facts(&page.body, &agent.display_name, entries, &who, &today());
    if added.is_empty() {
        return Ok(0);
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
        action,
        &format!("{} entries", added.len()),
    )?;
    Ok(added.len())
}

/// What an agent brought from elsewhere (assistant_import.rs), put in « À ranger » for the nightly pass; the agent is
/// then counted as seeded (its memory, filled by the same import, is not copied a second time). Nothing when the
/// agent has no Carnet, or holds health data. Returns how many lines were added.
pub fn inbox_import(config: &Config, agent: &crate::config::AgentConfig, entries: &[String]) -> anyhow::Result<usize> {
    if !enabled(config, &agent.name) || health_agent(config, &agent.name) {
        return Ok(0);
    }
    let _guard = lock();
    let added = inbox_add_locked(config, agent, entries, "import")?;
    let mut seeded = strings(read_state(config).get("seeded"));
    if !seeded.contains(&agent.name) {
        seeded.push(agent.name.clone());
        update_state(config, |s| {
            s.insert("seeded".into(), json!(seeded));
        })?;
    }
    Ok(added)
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
        page.front.insert("id".into(), json!("p0123456789"));
        page.set_aliases(&["Marco".into(), "marc dupont".into(), "Marco".into()]);
        page.front.insert("contact".into(), json!("ABC:123"));
        page.front.insert("updated".into(), json!("2026-10-22"));
        page.body = "## Infos\n- Aime le jazz\n".into();
        let text = page.render();
        assert_eq!(
            text,
            "---\nid: p0123456789\ntype: person\naliases:\n  - Marco\ncontact: \"ABC:123\"\nupdated: 2026-10-22\n---\n# Marc Dupont\n\n## Infos\n- Aime le jazz\n"
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
    fn dates_are_read_from_facts_in_four_languages() {
        assert_eq!(date_in("Anniversaire : 14 novembre"), Some((11, 14, None)));
        assert_eq!(date_in("Né le 1er mars 1980"), Some((3, 1, Some(1980))));
        assert_eq!(date_in("Birthday: March 3rd"), Some((3, 3, None)));
        assert_eq!(date_in("Geburtstag am 5. Dezember"), Some((12, 5, None)));
        assert_eq!(date_in("Cumpleaños: 9 de agosto"), None); // « de » between: not taken (yet)
        assert_eq!(date_in("Devis à rendre avant le 2026-11-30"), Some((11, 30, Some(2026))));
        assert_eq!(date_in("Mariage le 12/06"), Some((6, 12, None)));
        assert_eq!(date_in("Aime le jazz"), None);
    }

    fn config_in(dir: &Path) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn facts_are_filed_in_the_outline_and_old_pages_refiled() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let who = lea();
        // an old page, from before the outlines
        let old = "---\nid: p0000000001\ntype: person\naliases: []\n---\n# Marc\n\n## Infos\n- Aime le jazz — *Léa, 2026-10-01*\n\
- Grand — *Léa, 2026-10-01*\n\n## Famille\n- Fille : Zoé — *Léa, 2026-10-02*\n";
        write_file(&part_root(&config, Part::Main).join("Personnes/Marc.md"), old).unwrap();
        assert_eq!(refile_existing(&config).unwrap(), 1);
        assert_eq!(refile_existing(&config).unwrap(), 0); // once
        let text = read_page_text(&config, Part::Main, "Personnes/Marc.md").unwrap();
        assert!(text.ends_with(
            "# Marc\n\n## Famille et proches\n- Fille : Zoé — *Léa, 2026-10-02*\n\n## Goûts et préférences\n\
- Aime le jazz — *Léa, 2026-10-01*\n\n## Autres\n- Grand — *Léa, 2026-10-01*\n"
        ));
        let change = &history(&config, None, None, 1)[0];
        assert_eq!((change["action"].as_str(), change["by"].as_str()), (Some("refile"), Some("nightly")));
        assert_eq!(change["summary"].as_str(), Some("rangement : 2 fait(s) rangé(s)"));
        // an agent's facts: split, each under its section, in the outline's order
        let facts = vec!["Anniversaire : 14 novembre; travaille chez Airbus".to_string(), "Habite Lyon".into()];
        let written = note(&config, Part::Main, "person", "Marc", "Infos", &facts, &[], "", "", &who).unwrap();
        assert_eq!(written.sections, vec!["Dates", "Travail", "Qui c'est"]);
        let page = find(&config, &[Part::Main], "Marc").unwrap().page;
        let titles: Vec<String> = page.sections().into_iter().map(|(t, _)| t).collect();
        assert_eq!(titles, ["Qui c'est", "Dates", "Famille et proches", "Travail", "Goûts et préférences", "Autres"]);
        assert_eq!(
            unsorted(&config, 10),
            vec![Unsorted { page: "Marc".into(), kind: "person".into(), fact: "Grand".into() }]
        );
        // the nightly pass's model places what is left; the line keeps its attribution
        assert!(refile_locked(&config, "Marc", "Grand", "who", &Who::tidy(&config)).unwrap());
        let text = read_page_text(&config, Part::Main, "Personnes/Marc.md").unwrap();
        assert!(
            text.contains("## Qui c'est\n- Habite Lyon — *Léa, ") && text.contains("- Grand — *Léa, 2026-10-01*\n")
        );
        assert!(!text.contains("## Autres"));
        // the latest exchanges only: the older ones leave the page as a change of their own
        for n in 1..=8 {
            let fact = format!("2026-10-1{n} : appel numéro {n}");
            note(&config, Part::Main, "person", "Marc", "exchanges", &[fact], &[], "", "", &who).unwrap();
        }
        let page = find(&config, &[Part::Main], "Marc").unwrap().page;
        let exchanges = page.sections().into_iter().find(|(t, _)| t == "Derniers échanges").unwrap().1;
        assert_eq!(exchanges.len(), 6);
        assert!(exchanges[0].text.starts_with("2026-10-13"));
        let archived = history(&config, None, None, 10).into_iter().rfind(|c| c["action"] == "archive").unwrap();
        assert!(archived["summary"].as_str().unwrap().contains("appel numéro 1"));
        // the section to rewrite is one of the outline's
        let refused = edit(&config, &[Part::Main], "Marc", "", "", "Voyages", Some("- x"), "", &who);
        assert!(refused.unwrap_err().to_string().contains("one of who, dates"));
        edit(
            &config,
            &[Part::Main],
            "Marc",
            "",
            "",
            "dates",
            Some("- Anniversaire : 14 novembre\n- Fête : 25 avril"),
            "",
            &who,
        )
        .unwrap();
        let text = read_page_text(&config, Part::Main, "Personnes/Marc.md").unwrap();
        assert!(
            text.contains("## Dates\n- Anniversaire : 14 novembre — *Léa, ")
                && text.contains("- Fête : 25 avril — *Léa, ")
        );
    }

    #[test]
    fn the_person_themself_has_their_own_page() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let who = lea();
        // a person page an agent made about the person themself, before « Toi »
        let old = "---\nid: p0000000002\ntype: person\naliases: []\n---\n# Moi\n\n## Infos\n- Court le dimanche — *Léa, 2026-10-01*\n";
        write_file(&part_root(&config, Part::Main).join("Personnes/Moi.md"), old).unwrap();
        write_file(&part_root(&config, Part::Main).join("Personnes/Zoé.md"), "---\ntype: person\n---\n# Zoé\n")
            .unwrap();
        refile_existing(&config).unwrap();
        assert!(read_page_text(&config, Part::Main, "Personnes/Moi.md").is_none());
        let text = read_page_text(&config, Part::Main, "Toi.md").unwrap();
        assert!(text.contains("type: self") && text.contains("  - Moi\n"), "{text}");
        assert!(text.contains("## Goûts et habitudes\n- Court le dimanche — *Léa, 2026-10-01*"), "{text}");
        assert!(read_page_text(&config, Part::Main, "Personnes/Zoé.md").is_some());
        // the agents' facts about the person go there, whatever name they give it
        let written =
            note(&config, Part::Main, "self", "Alex", "", &["Veut courir un marathon".into()], &[], "", "", &who)
                .unwrap();
        assert_eq!((written.path.as_str(), written.sections.clone()), ("Toi.md", vec!["Objectifs".to_string()]));
        let again =
            note(&config, Part::Main, "person", "Alex", "work", &["Développeur".into()], &[], "", "", &who).unwrap();
        assert_eq!(again.path, "Toi.md"); // « Alex » is now one of the page's names
        let text = read_page_text(&config, Part::Main, "Toi.md").unwrap();
        let order: Vec<usize> =
            ["## Travail", "## Goûts et habitudes", "## Objectifs"].iter().map(|t| text.find(t).unwrap()).collect();
        assert!(order.windows(2).all(|p| p[0] < p[1]), "{text}");
        // how the agents should behave goes there too
        let q = propose(&config, "lea", "birthday", "Anniversaire de Zoé", "").unwrap();
        decline(&config, &q, true, &who).unwrap();
        let text = read_page_text(&config, Part::Main, "Toi.md").unwrap();
        assert!(text
            .contains("## Ce que tes agents doivent savoir\n- Pas de propositions de tâches pour les anniversaires"));
    }

    #[test]
    fn close_people_dates_tasks_and_proposals() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        assert!(config.data_dir().starts_with(&*dir.path().to_string_lossy()));
        update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let who = lea();
        let fact = vec!["Anniversaire : 14 novembre 1980".to_string()];
        note(&config, Part::Main, "person", "Marc Dupont", "Infos", &fact, &[], "", "", &who).unwrap();
        note(&config, Part::Main, "person", "Paul", "Infos", &["Anniversaire : 20 novembre".into()], &[], "", "", &who)
            .unwrap();
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        // only the close people by default, every page with all
        assert!(upcoming_dates(&config, 60, false, today).is_empty());
        set_close(&config, "Marc Dupont", true, &who).unwrap();
        let dates = upcoming_dates(&config, 60, false, today);
        assert_eq!(dates.len(), 1);
        assert_eq!(
            (dates[0]["date"].as_str(), dates[0]["in_days"].as_i64(), dates[0]["years"].as_i64()),
            (Some("2026-11-14"), Some(13), Some(46))
        );
        assert_eq!(upcoming_dates(&config, 60, true, today).len(), 2);
        let page = find(&config, &[Part::Main], "Marc Dupont").unwrap().page;
        assert!(page.close() && page.id().starts_with('p'));
        // a proposal: budgeted, accepted with its id, archived when left long after its date
        assert!(
            add_task(&config, Part::Main, "Cadeau Marc", "2026-11-14", "Marc Dupont", "accepted", "q0", &who).is_err()
        );
        let id = propose(&config, "lea", "gift", "Cadeau pour Marc", "2026-11-14").unwrap();
        let task =
            add_task(&config, Part::Main, "Cadeau pour Marc", "2026-11-14", "Marc Dupont", "accepted", &id, &who)
                .unwrap()
                .task;
        assert_eq!(task.origin, "accepted");
        add_task(&config, Part::Main, "Appeler la banque", "2026-11-03", "", "asked", "", &who).unwrap();
        assert_eq!(open_tasks(&config, Part::Main)[0].title, "Appeler la banque");
        assert_eq!(archive_stale(&config, chrono::NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()).unwrap(), 0);
        assert_eq!(archive_stale(&config, chrono::NaiveDate::from_ymd_opt(2026, 12, 10).unwrap()).unwrap(), 1);
        let all = tasks(&config, Part::Main);
        assert!(all.iter().any(|t| t.archived && t.id == task.id));
        assert_eq!(open_tasks(&config, Part::Main).len(), 1);
        let text = read_page_text(&config, Part::Main, "Tâches/Tâches.md").unwrap();
        assert!(text.contains("\n## Archivées\n- [ ] Cadeau pour Marc [[Marc Dupont]] 📅 2026-11-14"));
        // declined for good: written as a preference, never proposed again
        let other = propose(&config, "lea", "birthday", "Rappel anniversaire Paul", "").unwrap();
        assert_eq!(
            decline(&config, &other, true, &who).unwrap().unwrap(),
            "Pas de propositions de tâches pour les anniversaires"
        );
        assert!(propose(&config, "lea", "birthday", "x", "").is_err());
        assert!(find(&config, &[Part::Main], "Toi")
            .unwrap()
            .page
            .body
            .contains("Pas de propositions de tâches pour les anniversaires"));
        propose(&config, "lea", "follow_up", "y", "").unwrap();
        assert!(propose(&config, "lea", "deadline", "z", "").is_err()); // 3 a week
    }

    fn queue_of(config: &Config) -> Vec<(String, String, Value)> {
        match crate::reminders::read(config).get("queue") {
            Some(Value::Array(q)) => q
                .iter()
                .map(|o| (tk::str_or_empty(o.get("op")), tk::str_or_empty(o.get("state")), o["args"].clone()))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn share_items(config: &Config, items: Value) {
        let mut data = crate::reminders::read(config);
        data.insert("items".into(), items);
        crate::reminders::write(config, &data).unwrap();
    }

    #[test]
    fn a_task_is_not_added_twice_and_takes_its_reminder_when_it_leaves() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let mut data = Dict::new();
        data.insert("agents".into(), json!(["lea"]));
        crate::reminders::write(&config, &data).unwrap();
        let who = lea();
        // the same task asked again (case, accents, punctuation, the page's link): the first one returned
        let first = add_task(&config, Part::Main, "Appeler le dentiste", "2026-11-03", "", "asked", "", &who).unwrap();
        assert!(first.reminder && !first.existing);
        let again =
            add_task(&config, Part::Main, "appeler le DENTISTE !", "2026-11-03T10:00", "", "asked", "", &who).unwrap();
        assert!(again.existing && !again.reminder && again.task.id == first.task.id);
        let undated = add_task(&config, Part::Main, "Appeler le dentiste", "", "", "asked", "", &who).unwrap();
        assert!(undated.existing && undated.task.id == first.task.id);
        // another date, another title: other tasks
        let next = add_task(&config, Part::Main, "Appeler le dentiste", "2026-12-03", "", "asked", "", &who).unwrap();
        assert!(!next.existing);
        let other = add_task(&config, Part::Main, "Appeler le garage", "2026-11-03", "", "asked", "", &who).unwrap();
        assert!(!other.existing);
        // without a date first, then with one: the date and its reminder added to the same task
        let plain = add_task(&config, Part::Main, "Payer la cantine", "", "Maison", "asked", "", &who).unwrap();
        let dated = add_task(&config, Part::Main, "Payer la cantine", "2026-11-06", "", "asked", "", &who).unwrap();
        assert!(dated.existing && dated.dated && dated.reminder && dated.task.id == plain.task.id);
        assert_eq!(tasks(&config, Part::Main).iter().filter(|t| t.title.starts_with("Payer")).count(), 1);
        assert_eq!(queue_of(&config).len(), 4);
        // removed while its add still waits: the add cancelled, nothing sent
        edit(&config, &[Part::Main], "Tâches", "Appeler le garage", "", "", None, "", &who).unwrap();
        let queue = queue_of(&config);
        assert_eq!(queue.iter().filter(|(op, state, _)| op == "add" && state == "cancelled").count(), 1);
        assert!(queue.iter().all(|(op, _, _)| op == "add"));
        // on the phone (shared back): removed by its id and mark
        let mark = reminder_mark(&first.task.id);
        share_items(&config, json!([{"id": "r1", "title": "Appeler le dentiste", "notes": mark}]));
        for (n, (op, _, _)) in queue_of(&config).iter().enumerate() {
            if op == "add" {
                let id = tk::str_or_empty(crate::reminders::read(&config)["queue"][n].get("id"));
                let _ = crate::reminders::cancel_pending(&config, &id); // as if done by the phone
            }
        }
        edit(&config, &[Part::Main], "Tâches", "Appeler le dentiste", "", "", None, "", &who).unwrap();
        let removes: Vec<Value> =
            queue_of(&config).into_iter().filter(|(op, _, _)| op == "remove").map(|(_, _, a)| a).collect();
        assert_eq!(removes, vec![json!({"id": "r1", "mark": mark})]);
        assert!(!links(&read_state(&config)).contains_key(&first.task.id));
        // archived (moved): not gone, nothing asked
        let before = queue_of(&config).len();
        let text = read_page_text(&config, Part::Main, "Tâches/Tâches.md").unwrap();
        let moved = text.replace(&next.task.render(), "") + &format!("\n## Archivées\n{}\n", next.task.render());
        write_page(&config, Part::Main, "Tâches/Tâches.md", &moved, &Who::you(&config)).unwrap();
        assert!(tasks(&config, Part::Main).iter().any(|t| t.id == next.task.id && t.archived));
        assert_eq!(queue_of(&config).len(), before);
        // the cleanup: an orphan link (removed before this fix), duplicates of one mark, another Carnet's mark
        update_state(&config, |s| {
            let mut map = links(s);
            map.insert("t0000aaaa".into(), json!({"op": "gone", "part": "main", "complete": false}));
            s.insert("reminders".into(), Value::Object(map));
            let mut meta = task_meta(s);
            meta.insert("t0000aaaa".into(), json!({"origin": "asked"}));
            s.insert("tasks".into(), Value::Object(meta));
        })
        .unwrap();
        let kept = reminder_mark(&next.task.id);
        share_items(
            &config,
            json!([{"id": "r2", "notes": reminder_mark("t0000aaaa")}, {"id": "r3", "notes": kept},
                   {"id": "r4", "notes": format!("x\n{kept}")}, {"id": "r5", "notes": reminder_mark("t9999ffff")},
                   {"id": "r6", "notes": format!("{kept}0")}, {"id": "r7", "notes": kept, "completed": true}]),
        );
        assert_eq!(clean_reminders(&config).unwrap(), 2);
        assert_eq!(clean_reminders(&config).unwrap(), 0); // asked once
        let removes: Vec<String> = queue_of(&config)
            .into_iter()
            .filter(|(op, _, _)| op == "remove")
            .map(|(_, _, a)| tk::str_or_empty(a.get("id")))
            .collect();
        assert_eq!(removes, vec!["r1", "r2", "r4"]);
        assert!(!links(&read_state(&config)).contains_key("t0000aaaa"));
    }

    #[test]
    fn the_summary_comes_first_and_is_rewritten() {
        let body = "## Infos\n- Aime le jazz\n";
        let with = set_summary(body, "En bref", "Ami d'enfance.\nAnniversaire le 14 novembre.");
        assert_eq!(with, "## En bref\nAmi d'enfance.\nAnniversaire le 14 novembre.\n\n## Infos\n- Aime le jazz\n");
        assert_eq!(summary_of(&with, "en bref"), "Ami d'enfance.\nAnniversaire le 14 novembre.");
        let again = set_summary(&with, "En bref", "Ami d'enfance, vit à Lyon.");
        assert_eq!(again, "## En bref\nAmi d'enfance, vit à Lyon.\n\n## Infos\n- Aime le jazz\n");
        assert_eq!(set_summary(&again, "En bref", "  "), again);
    }

    #[test]
    fn names_and_paths() {
        assert_eq!(file_name("Marc/Dupont: [notes]"), "Marc Dupont notes");
        assert_eq!(fold("Élodie  O'Brien!"), "elodie o brien");
        assert!(checked("Personnes/Marc.md").is_ok());
        assert!(checked("../x.md").is_err() && checked("a/b/c/d.md").is_err() && checked("Personnes/x.txt").is_err());
        assert_eq!(words("de").tasks_path(), "Aufgaben/Aufgaben.md");
        assert_eq!(words("it").lang, "en");
    }
}
