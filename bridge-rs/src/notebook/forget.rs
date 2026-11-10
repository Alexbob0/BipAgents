//! « Oublie ça » (Carnet v2, decision 10 of internal/notes/carnet-v2/spec.md, contract.md §3.5): what the person
//! asks to forget is erased everywhere the bridge keeps it, without a model, after a preview the person confirms in
//! the app. Never by an agent alone: `notebook_write {forget}` only makes the preview and puts a card in the
//! conversation (`card`), the token that erases is given only by the app's route (`POST /v1/notebook/forget/preview`),
//! once, for 10 minutes. The conversations themselves are never touched (Alex, 9/11: « la conv est une trace »).
//!
//! What matches (`Target::hit`, no model): a line holding the text asked (folded), or the same value under a label of
//! the same family (lexicon.rs: « chausse du 36 », « fait du 36 » for « Pointure : 36 ») with the page's name or one
//! of its aliases (« Léa fait du 36 »); on the page itself the name is implied. A whole page: the page, and every line
//! that names it. A label and its value without any name, in an agent's memory, is « unsure »: shown in the preview
//! (`unsure`) and erased too.
//!
//! Where (`plan`, then `apply` under the Carnet's lock): the vault's pages (`private/` too, « À ranger » and the
//! records are pages); `history/versions/*.md` (the line becomes « (effacé) », the file keeps its name); the `summary`
//! of `log.jsonl`'s entries; the common memory (`shared-user.json`) and every agent's `USER.md`, `MEMORY.md` and
//! `knowledge/` pages; `news.json`; the archives' indexes (the documents taken out and never indexed again:
//! archives/index.rs); `reads.jsonl` (a page's reads); the family Carnet on this place (the person's own lines only);
//! and `state.json` `forgotten`: fingerprints (hashes, never the text) of the texts and of their label and value with
//! the page's names, read by `note`, « À ranger », the nightly pass and the common memory (`is_forgotten`), so an old
//! turn does not write it back, even said another way. A whole page's fingerprint lasts 30 days (the person may start
//! a page of that name again later). `state.json` itself keeps no text of it afterwards (`scrub_state`): the agents'
//! open « Oublie ça » cards about it closed, the question, follow-up and proposal cards about it removed, the nightly
//! pass's tried lines kept as fingerprints. The erasure is logged as `forgotten`, without its text. The offline copy's ETag
//! changes with the content (offline.rs).
//!
//!     notebook/state.json   forgotten: [{t: [hash], f: [hash], n: [hash], until?}], cards: [{id, at, agent, session,
//!                           card: "forget", state, request, view}]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::reads::Span;
use super::{fold, lexicon, Page, Part, Who};
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

/// How long a preview's token may confirm.
pub const TOKEN_SECONDS: f64 = 600.0;
/// Texts one request forgets at most.
pub const TEXTS_MAX: usize = 5;
/// A whole page's fingerprint lasts this long.
const PAGE_DAYS: f64 = 30.0;
/// Fingerprints kept at most.
const FORGOTTEN_MAX: usize = 2000;
/// What an erased line becomes in the history.
pub const ERASED: &str = "(effacé)";

/// Phrases that put a value under a label, said in passing, that the lexicon's table leaves out (they mean other
/// things too: « fait du vélo »); only to find a fact said another way, never to write one.
const LOOSE: &[(&str, &[&str])] = &[("shoe_size", &["fait du", "fait une", "porte du", "wears a", "tragt"])];

/// What the person asks to forget: facts (≤ 5) of a page, or a whole page (`page` alone).
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize, PartialEq)]
pub struct Request {
    #[serde(default)]
    pub texts: Vec<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub private: bool,
}

/// A refusal for the app (contract.md §5): its code.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The page named is not there any more.
    PageGone,
    /// Several pages have that name.
    Several(Vec<String>),
    /// Nothing anywhere matches.
    Nothing,
}

// -- texts -------------------------------------------------------------------------------------------------------------

pub struct Texts {
    pub one: &'static str,
    pub many: &'static str,
    pub page: &'static str,
    pub found: &'static str,
    pub carnet: &'static str,
    pub memory: &'static str,
    pub common: &'static str,
    pub history: &'static str,
    pub archive: &'static str,
    pub family: &'static str,
    pub conversation: &'static str,
    pub mail: &'static str,
    pub backups: &'static str,
    pub confirm: &'static str,
    pub cancel: &'static str,
    pub warning: &'static str,
    pub done: &'static str,
    pub nothing: &'static str,
    pub health: &'static str,
}

pub fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts {
            one: "Oublier « {x} » ?", many: "Oublier ces {n} faits ?", page: "Oublier la page « {x} » ?",
            found: "Trouvé dans :", carnet: "Carnet · {x}", memory: "Mémoire de {x}",
            common: "Mémoire commune de tes agents", history: "Historique du Carnet",
            archive: "Archive {x} (retiré de la recherche)", family: "Carnet de la famille (visible par ta famille)",
            conversation: "Reste dans la conversation où tu l'as dit.", mail: "Le mail lui-même reste dans ta boîte.",
            backups: "Disparaît aussi des sauvegardes sous 30 jours.", confirm: "Oublier", cancel: "Annuler",
            warning: "On ne pourra pas le remettre.", done: "Oublié.", nothing: "Rien à oublier ici.", health: "Santé",
        },
        "de" => Texts {
            one: "« {x} » vergessen?", many: "Diese {n} Fakten vergessen?", page: "Die Seite « {x} » vergessen?",
            found: "Gefunden in:", carnet: "Notizbuch · {x}", memory: "Gedächtnis von {x}",
            common: "Gemeinsames Gedächtnis deiner Agenten", history: "Verlauf des Notizbuchs",
            archive: "Archiv {x} (aus der Suche entfernt)", family: "Familien-Notizbuch (für deine Familie sichtbar)",
            conversation: "Bleibt im Gespräch, in dem du es gesagt hast.",
            mail: "Die E-Mail selbst bleibt in deinem Postfach.",
            backups: "Verschwindet auch innerhalb von 30 Tagen aus den Sicherungen.", confirm: "Vergessen",
            cancel: "Abbrechen", warning: "Das lässt sich nicht rückgängig machen.", done: "Vergessen.",
            nothing: "Hier gibt es nichts zu vergessen.", health: "Gesundheit",
        },
        "es" => Texts {
            one: "¿Olvidar « {x} »?", many: "¿Olvidar estos {n} datos?", page: "¿Olvidar la página « {x} »?",
            found: "Encontrado en:", carnet: "Cuaderno · {x}", memory: "Memoria de {x}",
            common: "Memoria común de tus agentes", history: "Historial del Cuaderno",
            archive: "Archivo {x} (quitado de la búsqueda)", family: "Cuaderno de la familia (visible para tu familia)",
            conversation: "Se queda en la conversación donde lo dijiste.", mail: "El correo en sí se queda en tu buzón.",
            backups: "También desaparece de las copias de seguridad en 30 días.", confirm: "Olvidar",
            cancel: "Cancelar", warning: "No se podrá recuperar.", done: "Olvidado.", nothing: "Nada que olvidar aquí.",
            health: "Salud",
        },
        _ => Texts {
            one: "Forget « {x} »?", many: "Forget these {n} facts?", page: "Forget the page « {x} »?",
            found: "Found in:", carnet: "Notebook · {x}", memory: "Memory of {x}",
            common: "Your agents' shared memory", history: "Notebook history",
            archive: "Archive {x} (taken out of search)", family: "Family notebook (seen by your family)",
            conversation: "Stays in the conversation where you said it.",
            mail: "The e-mail itself stays in your mailbox.", backups: "Also gone from the backups within 30 days.",
            confirm: "Forget", cancel: "Cancel", warning: "It can't be put back.", done: "Forgotten.",
            nothing: "Nothing to forget here.", health: "Health",
        },
    }
}

// -- what matches ------------------------------------------------------------------------------------------------------

pub(super) fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_string()
}

/// A line's fact: its list mark, checkbox and attribution left out.
fn fact_of(line: &str) -> String {
    let line = line.trim().trim_start_matches(['-', '*']).trim_start();
    let line = line.strip_prefix("[ ]").or_else(|| line.strip_prefix("[x]")).unwrap_or(line).trim();
    super::split_attribution(line).0
}

/// Whether folded `needle` is in folded `hay` as whole words.
fn has_words(hay: &str, needle: &str) -> bool {
    !needle.is_empty() && format!(" {hay} ").contains(&format!(" {needle} "))
}

/// The (label, value) a text says, folded: « Pointure : 36 » and « Léa chausse du 36 » both give (shoe_size, 36);
/// after a label's phrase, the next one to three words are each a possible value.
pub fn forms(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    if let Some((label, value)) = lexicon::split_label(text) {
        out.push((lexicon::label_key(&label), fold(&value)));
    }
    let folded = fold(text);
    let tokens: Vec<&str> = folded.split(' ').filter(|t| !t.is_empty()).collect();
    for l in lexicon::LABELS {
        let loose: &[&str] = LOOSE.iter().find(|(id, _)| *id == l.id).map(|(_, p)| *p).unwrap_or(&[]);
        let names: Vec<String> = l.names.iter().map(|n| fold(n)).collect();
        let phrases = l.words.iter().chain(l.joins.iter()).chain(loose.iter()).map(|p| p.to_string()).chain(names);
        for phrase in phrases {
            let words: Vec<&str> = phrase.split(' ').collect();
            let n = words.len();
            for i in 0..tokens.len().saturating_sub(n - 1) {
                if tokens[i..i + n] != words[..] {
                    continue;
                }
                let rest = &tokens[i + n..];
                for k in 1..=rest.len().min(3) {
                    out.push((l.id.to_string(), rest[..k].join(" ")));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// How sure a line is to be what the person asks to forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Sure,
    Unsure,
}

/// What a request forgets, resolved.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// The texts asked, folded.
    texts: Vec<String>,
    /// Their (label, value) forms.
    forms: Vec<(String, String)>,
    /// The page's names (title, aliases), folded; and their words that name it alone (a first name, a nickname).
    names: Vec<String>,
    first: Vec<String>,
    /// The page forgotten whole (its part, path and id), or the pages the facts are on.
    pub whole: Option<(Part, String, String)>,
    pub pages: Vec<(Part, String)>,
    /// The person's own page (their memory lines are about them: the name is implied).
    pub about_self: bool,
    /// No page named: the texts are forgotten wherever they are written.
    pub anywhere: bool,
    /// What the title shows (« Pointure : 36 », the page's name).
    pub shown: String,
    pub count: usize,
}

impl Target {
    /// Whether `line` is what the target forgets. `on_page`: a line of the page itself (its name implied);
    /// `memory`: a line of an agent's memory, where a label and value without a name is shown as unsure.
    pub fn hit(&self, line: &str, on_page: bool, memory: bool) -> Option<Hit> {
        let fact = fact_of(line);
        let folded = fold(&fact);
        if folded.is_empty() || folded == fold(ERASED) {
            return None;
        }
        let named = self.names.iter().any(|n| has_words(&folded, n)) || self.links(line);
        let by_name = self.first.iter().any(|n| has_words(&folded, n));
        if self.whole.is_some() {
            if on_page || named {
                return Some(Hit::Sure);
            }
            return (by_name && memory).then_some(Hit::Unsure);
        }
        // the text itself: anywhere when no page was named, else on the page or with its name (Marc's own
        // « Pointure : 36 » is another fact)
        if self.texts.iter().any(|t| has_words(&folded, t)) {
            if self.anywhere || on_page || named || by_name {
                return Some(Hit::Sure);
            }
            return memory.then_some(Hit::Unsure);
        }
        let mine = forms(&fact);
        if !self.forms.iter().any(|f| mine.contains(f)) {
            return None;
        }
        if on_page || named || by_name {
            Some(Hit::Sure)
        } else if memory {
            Some(Hit::Unsure)
        } else {
            None
        }
    }

    fn links(&self, line: &str) -> bool {
        super::links_in(line).iter().any(|l| self.names.contains(&fold(l)))
    }
}

/// The request resolved: the page found by name (several: refused), the texts' forms.
pub fn target(config: &Config, request: &Request) -> Result<Target, Refused> {
    let parts = [Part::Main, Part::Health];
    let lang = super::language(config);
    let texts: Vec<String> = request.texts.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).take(TEXTS_MAX).collect();
    let mut t = Target { count: texts.len(), ..Default::default() };
    let mut pages: Vec<super::Found> = Vec::new();
    t.anywhere = request.page.as_deref().is_none_or(|p| p.trim().is_empty());
    if let Some(name) = request.page.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        let found: Vec<super::Found> =
            super::candidates(config, &parts, name, "").into_iter().filter(|f| f.page.kind() != "inbox").collect();
        if found.len() > 1 {
            return Err(Refused::Several(found.iter().map(|f| f.page.title.clone()).collect()));
        }
        match found.into_iter().next() {
            Some(f) => pages.push(f),
            None if texts.is_empty() => return Err(Refused::PageGone),
            None => t.anywhere = true,
        }
    }
    for text in &texts {
        let fact = fact_of(text);
        t.texts.push(fold(&fact));
        let shaped = lexicon::reshape(&fact, &lang);
        for f in [&fact, &shaped] {
            if let Some((label, value)) = lexicon::split_label(f) {
                t.forms.push((lexicon::label_key(&label), fold(&value)));
            }
        }
    }
    t.forms.sort();
    t.forms.dedup();
    if pages.is_empty() && !texts.is_empty() {
        // the facts alone (« Toujours en tête »): the pages that hold them give the names
        for part in parts {
            for (path, page) in super::all_pages(config, part) {
                if page.body.lines().any(|l| {
                    let f = fold(&fact_of(l));
                    t.texts.iter().any(|x| has_words(&f, x))
                }) {
                    pages.push(super::Found { part, path, page });
                }
            }
        }
    }
    for f in &pages {
        let mut names = vec![fold(&f.page.title)];
        names.extend(f.page.aliases().iter().map(|a| fold(a)));
        for n in &names {
            if !t.names.contains(n) {
                t.names.push(n.clone());
            }
        }
        if f.page.kind() == "person" {
            if let Some(first) = names.first().and_then(|n| n.split(' ').next()) {
                if first.chars().count() >= 2 && !t.first.contains(&first.to_string()) {
                    t.first.push(first.to_string());
                }
            }
        }
        if f.page.kind() == "self" {
            t.about_self = true;
        }
        t.pages.push((f.part, f.path.clone()));
    }
    t.names.retain(|n| n.chars().count() >= 2);
    if texts.is_empty() {
        let f = &pages[0];
        let id = if f.page.id().is_empty() { f.path.clone() } else { f.page.id() };
        t.whole = Some((f.part, f.path.clone(), id));
        t.shown = f.page.title.clone();
    } else {
        t.shown = fact_of(&texts[0]);
    }
    Ok(t)
}

// -- fingerprints --------------------------------------------------------------------------------------------------

/// The fingerprints of a target, for `state.json` `forgotten`.
fn fingerprint(t: &Target, now: f64) -> Value {
    let names: Vec<String> = t.names.iter().chain(t.first.iter()).map(|n| hash(n)).collect();
    let mut entry = json!({"t": t.texts.iter().map(|x| hash(x)).collect::<Vec<_>>(),
                           "f": t.forms.iter().map(|(l, v)| hash(&format!("{l}|{v}"))).collect::<Vec<_>>(),
                           "n": names, "at": now.round(), "anywhere": t.anywhere});
    if t.whole.is_some() {
        entry["until"] = json!((now + PAGE_DAYS * 86400.0).round());
        entry["page"] = json!(true);
    }
    entry
}

/// Every run of 1 to 4 words of a folded text.
fn windows(folded: &str) -> Vec<String> {
    let tokens: Vec<&str> = folded.split(' ').filter(|t| !t.is_empty()).collect();
    let mut out = Vec::new();
    for n in 1..=4usize {
        for w in tokens.windows(n) {
            out.push(w.join(" "));
        }
    }
    out
}

/// Whether `fact`, about the page `page` (its title; empty: unknown), is something the person asked to forget: the
/// same text, or its label and value with one of the page's names (`page`, or a name in the fact). Read by `note`,
/// « À ranger » and the common memory before they write.
pub fn is_forgotten(config: &Config, fact: &str, page: &str) -> bool {
    let state = super::read_state(config);
    let Some(Value::Array(prints)) = state.get("forgotten") else { return false };
    if prints.is_empty() {
        return false;
    }
    let now = py::time_time();
    let fact_text = fact_of(fact);
    let folded = fold(&fact_text);
    if folded.is_empty() {
        return false;
    }
    let mut named: HashSet<String> = windows(&folded).iter().map(|w| hash(w)).collect();
    let page_folded = fold(page);
    if !page_folded.is_empty() {
        named.insert(hash(&page_folded));
        if let Some(first) = page_folded.split(' ').next() {
            named.insert(hash(first));
        }
    }
    let pieces: HashSet<String> = windows(&folded).iter().map(|w| hash(w)).collect();
    let forms: Vec<String> = forms(&fact_text).iter().map(|(l, v)| hash(&format!("{l}|{v}"))).collect();
    prints.iter().any(|p| {
        if p.get("until").and_then(Value::as_f64).is_some_and(|u| u < now) {
            return false;
        }
        let list = |k: &str| -> Vec<&str> {
            p.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
        };
        let names = list("n");
        let has_name = names.iter().any(|n| named.contains(*n));
        if p.get("page").and_then(Value::as_bool).unwrap_or(false) {
            return has_name;
        }
        let anywhere = p.get("anywhere").and_then(Value::as_bool).unwrap_or(true);
        if list("t").iter().any(|t| *t == hash(&folded) || pieces.contains(*t)) && (anywhere || has_name) {
            return true;
        }
        (has_name || names.is_empty()) && list("f").iter().any(|f| forms.iter().any(|x| x == f))
    })
}

// -- the plan ------------------------------------------------------------------------------------------------------

/// One file of an agent's memory to change.
struct Memory {
    agent: String,
    folder: PathBuf,
    which: &'static str,
}

/// What a confirmation erases, and what the preview shows.
#[derive(Default)]
pub struct Plan {
    pages: Vec<(Part, String, String, Option<String>)>,
    versions: Vec<(PathBuf, String)>,
    log: Option<String>,
    memories: Vec<Memory>,
    knowledge: Vec<(PathBuf, String)>,
    common: bool,
    news: bool,
    reads: bool,
    archives: Vec<(PathBuf, String)>,
    family: Option<Dict>,
    pub places: Vec<String>,
    pub unsure: Vec<String>,
    pub mail: bool,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.places.is_empty() && self.unsure.is_empty()
    }

    fn place(&mut self, text: String) {
        if !self.places.contains(&text) {
            self.places.push(text);
        }
    }
}

/// The lines of `text` that `keep` keeps, those it drops replaced by `with` (None: dropped). Returns the new text
/// when something changed, and the unsure lines.
fn lines_without(text: &str, hit: impl Fn(&str) -> Option<Hit>, with: Option<&str>) -> (Option<String>, Vec<String>) {
    let mut out: Vec<String> = Vec::new();
    let mut unsure = Vec::new();
    let mut changed = false;
    for line in text.lines() {
        // headings and the frontmatter's lines stay (a page's outline, its name)
        let structural = line.starts_with('#') || line.starts_with("---") || line.trim().is_empty();
        match (!structural).then(|| hit(line)).flatten() {
            Some(h) => {
                changed = true;
                if h == Hit::Unsure {
                    unsure.push(fact_of(line));
                }
                if let Some(with) = with {
                    let lead: String = line.chars().take_while(|c| c.is_whitespace() || *c == '-' || *c == '*').collect();
                    out.push(format!("{lead}{with}"));
                }
            }
            None => out.push(line.to_string()),
        }
    }
    if !changed {
        return (None, unsure);
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') {
        joined.push('\n');
    }
    (Some(joined), unsure)
}

/// The agents whose memory may hold it: this install's own (the family's or others' agents are not the person's).
fn agents(config: &Config) -> Vec<(String, String, PathBuf)> {
    let facing = crate::vault::others_facing_agents(config);
    config
        .agents
        .values()
        .filter(|a| !facing.contains(&a.name.to_lowercase()))
        .map(|a| {
            let shown = if a.display_name.is_empty() { a.name.clone() } else { a.display_name.clone() };
            (a.name.clone(), shown, crate::agents_admin::profile_folder(config, a))
        })
        .filter(|(_, _, f)| f.is_dir())
        .collect()
}

fn page_label(config: &Config, part: Part, page: &Page, lang: &str, t: &Texts) -> String {
    if part == Part::Health {
        return t.carnet.replace("{x}", t.health);
    }
    let _ = config;
    t.carnet.replace("{x}", &super::reads::short(page, lang))
}

/// Every place `target` is found, without changing anything. `person`: the person's name in the family Carnet.
pub fn plan(config: &Config, target: &Target, lang: &str, person: &str) -> Plan {
    let t = texts(lang);
    let mut plan = Plan::default();
    // 1. the vault: the page itself, and the lines elsewhere
    for part in [Part::Main, Part::Health] {
        for (path, page) in super::all_pages(config, part) {
            let Some(text) = super::read_page_text(config, part, &path) else { continue };
            let own = target.pages.contains(&(part, path.clone()));
            if own && target.whole.as_ref().is_some_and(|(p, w, _)| *p == part && *w == path) {
                plan.place(page_label(config, part, &page, lang, &t));
                plan.pages.push((part, path.clone(), text, None));
                continue;
            }
            let on_page = own || (target.about_self && page.kind() == "self");
            let (new, _) = lines_without(&text, |l| target.hit(l, on_page, false), None);
            if let Some(new) = new {
                plan.place(page_label(config, part, &page, lang, &t));
                plan.pages.push((part, path, text, Some(new)));
            }
        }
    }
    // 2. the history: versions and the log's summaries
    let versions = super::history_dir(config).join("versions");
    let whole_id = target.whole.as_ref().map(|(_, _, id)| id.clone());
    for entry in std::fs::read_dir(&versions).into_iter().flatten().flatten() {
        let file = entry.path();
        if file.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = tk::read_text(&file) else { continue };
        let page = Page::parse(&text, "");
        if whole_id.as_deref().is_some_and(|id| page.id() == id) {
            plan.versions.push((file, format!("{ERASED}\n")));
            continue;
        }
        let on_page = target.names.contains(&fold(&page.title)) || (target.about_self && page.kind() == "self");
        if let (Some(new), _) = lines_without(&text, |l| target.hit(l, on_page, false), Some(ERASED)) {
            plan.versions.push((file, new));
        }
    }
    let log = super::history_dir(config).join("log.jsonl");
    if let Ok(text) = tk::read_text(&log) {
        let mut changed = false;
        let mut out: Vec<String> = Vec::new();
        for line in text.lines() {
            let Ok(Value::Object(mut e)) = serde_json::from_str::<Value>(line) else {
                out.push(line.to_string());
                continue;
            };
            let path = tk::str_or_empty(e.get("path"));
            let summary = tk::str_or_empty(e.get("summary"));
            let of_page = target.whole.as_ref().is_some_and(|(_, w, _)| *w == path);
            let on_page = target.pages.iter().any(|(_, p)| *p == path);
            let hit = of_page
                || summary.split(" ; ").any(|s| target.hit(s, on_page, false).is_some())
                || (target.whole.is_some() && target.names.contains(&fold(&tk::str_or_empty(e.get("title")))));
            if hit && summary != ERASED {
                e.insert("summary".into(), json!(ERASED));
                if target.whole.is_some() {
                    // a page's path is its name too
                    for k in ["title", "path", "moved_to", "moved_from"] {
                        if e.contains_key(k) {
                            e.insert(k.into(), json!(ERASED));
                        }
                    }
                }
                changed = true;
                out.push(py::Dumps::new().unicode().dumps(&Value::Object(e)));
            } else {
                out.push(line.to_string());
            }
        }
        if changed {
            plan.log = Some(out.join("\n") + "\n");
        }
    }
    if !plan.versions.is_empty() || plan.log.is_some() {
        plan.place(t.history.to_string());
    }
    // 3. the agents' memories: USER.md, MEMORY.md, knowledge/
    for (agent, shown, folder) in agents(config) {
        for which in ["user", "notes"] {
            let Some(entries) = crate::memory_edit::read_locked(&folder, which) else { continue };
            let on_page = which == "user" && target.about_self;
            let mut found = false;
            for e in &entries {
                if let Some(h) = target.hit(e, on_page, true) {
                    found = true;
                    if h == Hit::Unsure {
                        let line = format!("{} : « {} »", t.memory.replace("{x}", &shown), tk::head(&fact_of(e), 120));
                        if !plan.unsure.contains(&line) {
                            plan.unsure.push(line);
                        }
                    } else {
                        plan.place(t.memory.replace("{x}", &shown));
                    }
                }
            }
            if found {
                plan.memories.push(Memory { agent: agent.clone(), folder: folder.clone(), which });
            }
        }
        for page in crate::memory_edit::pages(&folder) {
            let relative = tk::str_or_empty(page.get("path"));
            let Ok(file) = crate::memory_edit::page_path(&folder, &relative) else { continue };
            let Ok(text) = tk::read_text(&file) else { continue };
            let (new, unsure) = lines_without(&text, |l| target.hit(l, false, true), None);
            if let Some(new) = new {
                if unsure.len() < text.lines().filter(|l| target.hit(l, false, true).is_some()).count() {
                    plan.place(t.memory.replace("{x}", &shown));
                }
                for u in unsure {
                    let line = format!("{} : « {} »", t.memory.replace("{x}", &shown), tk::head(&u, 120));
                    if !plan.unsure.contains(&line) {
                        plan.unsure.push(line);
                    }
                }
                plan.knowledge.push((file, new));
            }
        }
    }
    if crate::shared_memory::forget(config, &|e| target.hit(e, target.about_self, true).is_some(), true) > 0 {
        plan.common = true;
        if !plan.places.iter().any(|p| p.starts_with(&t.memory.replace("{x}", ""))) {
            plan.place(t.common.to_string());
        }
    }
    // 4. the news waiting for the agents
    if super::news::forget_facts(config, &|path, text| {
        target.whole.as_ref().is_some_and(|(_, w, _)| *w == path)
            || text.split(" ; ").any(|s| target.hit(s, target.pages.iter().any(|(_, p)| p == path), false).is_some())
    }, true) > 0
    {
        plan.news = true;
        plan.place(t.history.to_string());
    }
    // 5. the reads of a page forgotten whole
    if let Some(id) = &whole_id {
        if super::reads::entries(config).iter().any(|e| tk::str_or_empty(e.get("page_id")) == *id) {
            plan.reads = true;
            plan.place(t.history.to_string());
        }
    }
    // 6. the archives' indexes
    for archive in crate::archives::all(config) {
        let path = crate::archives::index_path(config, &archive);
        if !path.exists() {
            continue;
        }
        let Ok(index) = crate::archives::index::Index::open(&path) else { continue };
        for expr in archive_queries(target) {
            if index.matching(&expr).map(|m| !m.is_empty()).unwrap_or(false) {
                plan.place(t.archive.replace("{x}", &archive.name));
                plan.mail |= archive.kind == "mail";
                plan.archives.push((path.clone(), expr));
            }
        }
    }
    // 7. the family Carnet kept on this place: the person's own lines
    if let Some(data) = family(config, target, person) {
        plan.place(t.family.to_string());
        plan.family = Some(data);
    }
    plan
}

/// The FTS5 queries that find the target in an archive: each text's words with a name of the page when the text
/// does not hold one; a whole page by its full name.
fn archive_queries(target: &Target) -> Vec<String> {
    use crate::archives::index::match_expr;
    let mut out = Vec::new();
    if target.whole.is_some() {
        for name in target.names.iter().filter(|n| n.contains(' ') || n.chars().count() >= 4) {
            if let Some((expr, _)) = match_expr(&format!("\"{name}\""), false) {
                out.push(expr);
            }
        }
        return out;
    }
    for text in &target.texts {
        let name = target.first.first().or(target.names.first()).filter(|n| !has_words(text, n)).cloned().unwrap_or_default();
        let query: String = text
            .split(' ')
            .filter(|w| !lexicon::STOP.contains(w))
            .chain(name.split(' ').filter(|w| !w.is_empty()))
            .collect::<Vec<_>>()
            .join(" ");
        if let Some((expr, words)) = match_expr(&query, false) {
            if words.len() >= 2 {
                out.push(expr);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn family_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("family-notebook.json")
}

/// The family Carnet without the person's lines that match (None: nothing of theirs matches).
fn family(config: &Config, target: &Target, person: &str) -> Option<Dict> {
    let text = tk::read_text(&family_path(config)).ok()?;
    let Ok(Value::Object(mut data)) = serde_json::from_str::<Value>(&text) else { return None };
    let mine = |v: &Value| !person.is_empty() && tk::str_or_empty(v.get("by")) == person;
    let mut changed = false;
    if let Some(Value::Array(lists)) = data.get_mut("lists") {
        for list in lists.iter_mut() {
            if let Some(Value::Array(items)) = list.get_mut("items") {
                let before = items.len();
                items.retain(|i| !(mine(i) && target.hit(&tk::str_or_empty(i.get("text")), false, false).is_some()));
                changed |= items.len() != before;
            }
        }
    }
    if let Some(Value::Array(pages)) = data.get_mut("pages") {
        for page in pages.iter_mut() {
            if !mine(page) {
                continue;
            }
            let text = tk::str_or_empty(page.get("text"));
            if let (Some(new), _) = lines_without(&text, |l| target.hit(l, false, false), None) {
                page["text"] = json!(new);
                changed = true;
            }
        }
    }
    changed.then_some(data)
}

/// The plan applied, under the Carnet's lock. Returns what was erased (counts by place, never text).
pub fn apply(config: &Config, target: &Target, lang: &str, person: &str) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let plan = plan(config, target, lang, person);
    let mut erased = 0;
    let who = Who::you(config);
    for (part, path, _, after) in &plan.pages {
        let relative = super::checked(path)?;
        let file = super::part_root(config, *part).join(&relative);
        match after {
            Some(text) => super::write_file(&file, text)?,
            None => {
                if file.is_file() {
                    std::fs::remove_file(&file)?;
                }
            }
        }
        // the log's line says that something was forgotten here, never what
        let mut entry = Dict::new();
        entry.insert("id".into(), json!(format!("c{}", py::token_hex(6))));
        entry.insert("at".into(), json!(tk::now_iso_seconds()));
        entry.insert("by".into(), json!(who.by));
        entry.insert("name".into(), json!(who.name));
        entry.insert("agent".into(), Value::Null);
        entry.insert("part".into(), json!(part.as_str()));
        entry.insert("path".into(), json!(if after.is_none() { ERASED } else { path.as_str() }));
        entry.insert("title".into(), json!(if after.is_none() { ERASED.to_string() } else { Page::parse(after.as_deref().unwrap_or(""), "").title }));
        entry.insert("action".into(), json!("forgotten"));
        entry.insert("summary".into(), json!(""));
        entry.insert("before".into(), Value::Null);
        entry.insert("after".into(), json!(after.as_deref().map(|t| super::keep_version(config, t)).transpose()?));
        super::append_log(config, &entry)?;
        erased += 1;
    }
    for (file, text) in &plan.versions {
        let temporary = file.with_extension("part");
        py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, file)?;
        erased += 1;
    }
    if let Some(text) = &plan.log {
        let log = super::history_dir(config).join("log.jsonl");
        // the lines written since the plan's read are kept as they are
        let now = tk::read_text(&log).unwrap_or_default();
        let planned_lines = text.lines().count();
        let added: Vec<&str> = now.lines().skip(planned_lines).collect();
        let mut body = text.clone();
        for line in added {
            body.push_str(line);
            body.push('\n');
        }
        let temporary = log.with_extension("tmp");
        py::write_with_mode(&temporary.to_string_lossy(), body.as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &log)?;
        erased += 1;
    }
    for m in &plan.memories {
        let hit = |e: &String| target.hit(e, m.which == "user" && target.about_self, true).is_some();
        let done = crate::memory_edit::update_entries(&m.folder, m.which, |now| {
            let next: Vec<String> = now.iter().filter(|e| !hit(e)).cloned().collect();
            (next.len() != now.len()).then_some(next)
        });
        match done {
            Ok(_) => erased += 1,
            Err(err) => tracing::warn!(agent = %m.agent, error = %err, "a forgotten fact not taken out of a memory"),
        }
    }
    for (file, text) in &plan.knowledge {
        tk::replace_with(&file.with_extension("bipagents"), file, text.as_bytes())?;
        erased += 1;
    }
    // the common memory's records, whether or not an entry is common (a key given, a « Toi » fact seen)
    {
        erased += crate::shared_memory::forget(config, &|e| target.hit(e, target.about_self, true).is_some(), false);
    }
    if plan.news {
        erased += super::news::forget_facts(config, &|path, text| {
            target.whole.as_ref().is_some_and(|(_, w, _)| *w == path)
                || text.split(" ; ").any(|s| target.hit(s, target.pages.iter().any(|(_, p)| p == path), false).is_some())
        }, false);
    }
    if plan.reads {
        if let Some((_, _, id)) = &target.whole {
            erased += super::reads::drop_page(config, id)?;
        }
    }
    for (path, expr) in &plan.archives {
        match crate::archives::index::Index::open(path).and_then(|mut index| index.forget(expr)) {
            Ok(n) => erased += n,
            Err(err) => tracing::warn!(error = %err, "a forgotten fact not taken out of an archive's index"),
        }
    }
    if let Some(data) = &plan.family {
        let file = family_path(config);
        let temporary = file.with_extension("tmp");
        py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&Value::Object(data.clone())).as_bytes(), 0o600)?;
        std::fs::rename(&temporary, &file)?;
        erased += 1;
    }
    let now = py::time_time();
    let print = fingerprint(target, now);
    // the agents' own « Oublie ça » cards with nothing left to forget: closed with this one (bench O1, 9/11)
    let moot: HashSet<String> = open_forget_cards(config)
        .into_iter()
        .filter(|(_, request)| matches!(view(config, request, lang, person), Err(Refused::Nothing | Refused::PageGone)))
        .map(|(id, _)| id)
        .collect();
    super::update_state(config, |state| {
        erased += scrub_state(state, target, &moot);
        let list = state.entry("forgotten").or_insert_with(|| json!([]));
        if let Value::Array(items) = list {
            items.retain(|p| p.get("until").and_then(Value::as_f64).is_none_or(|u| u >= now));
            items.push(print);
            let extra = items.len().saturating_sub(FORGOTTEN_MAX);
            items.drain(..extra);
        }
    })?;
    tracing::info!(places = erased, "Carnet: forgotten");
    Ok(erased)
}

/// The open « Oublie ça » cards of `state.json`: (id, request).
fn open_forget_cards(config: &Config) -> Vec<(String, Request)> {
    let state = super::read_state(config);
    let Some(Value::Array(items)) = state.get("cards") else { return Vec::new() };
    items
        .iter()
        .filter(|c| tk::str_or_empty(c.get("card")) == "forget" && tk::str_or_empty(c.get("state")) == "open")
        .filter_map(|c| {
            let request = serde_json::from_value(c.get("request")?.clone()).ok()?;
            Some((tk::str_or_empty(c.get("id")), request))
        })
        .collect()
}

/// Every string of a JSON value.
fn strings_in(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| strings_in(v, out)),
        Value::Object(map) => map.values().for_each(|v| strings_in(v, out)),
        _ => {}
    }
}

/// Whether one of `value`'s texts is what `target` forgets; `path`: the page it is about, if any.
fn holds(target: &Target, value: &Value, path: &str) -> bool {
    let on_page = !path.is_empty()
        && (target.pages.iter().any(|(_, p)| p == path) || target.whole.as_ref().is_some_and(|(_, w, _)| w == path));
    if on_page && target.whole.is_some() {
        return true;
    }
    let mut texts = Vec::new();
    strings_in(value, &mut texts);
    // a question of the day answered keeps its label and the choice apart: the fact they make
    let (label, choice) = (tk::str_or_empty(value.get("label")), tk::str_or_empty(value.get("choice")));
    if !label.is_empty() && !choice.is_empty() {
        texts.push(format!("{label} : {choice}"));
    }
    texts.iter().any(|t| target.hit(t, on_page, false).is_some())
}

/// What `state.json` itself still says of the target (audit of 10/11, bench O1): every text the Carnet's own state
/// keeps of it goes, the conversations' cards included (the messages themselves stay in the conversation):
/// - « Oublie ça » cards: an open one about it (or in `moot`: nothing left to forget) is closed as done, its text
///   gone (an answered one never keeps its text);
/// - a question of the day's card or a follow-up's card about it: removed (its question, choice or task title);
/// - a task proposal about it (its title, page, a split's trips): removed;
/// - `refile_tried` keeps fingerprints since 10/11: an older entry (a raw text) is dropped.
///
/// Returns how many entries changed.
fn scrub_state(state: &mut Dict, target: &Target, moot: &HashSet<String>) -> usize {
    let mut changed = 0;
    if let Some(Value::Array(items)) = state.get_mut("cards") {
        let before = items.len();
        items.retain(|c| tk::str_or_empty(c.get("card")) == "forget" || !holds(target, c, &tk::str_or_empty(c.get("path"))));
        changed += before - items.len();
        for c in items.iter_mut().filter(|c| tk::str_or_empty(c.get("card")) == "forget") {
            let open = tk::str_or_empty(c.get("state")) == "open";
            let page = c.get("request").and_then(|r| r.get("page")).and_then(Value::as_str).unwrap_or("");
            let about = target.anywhere || target.names.contains(&fold(page));
            let hit = moot.contains(&tk::str_or_empty(c.get("id")))
                || c.get("request").is_some_and(|r| {
                    let mut texts = Vec::new();
                    strings_in(r, &mut texts);
                    texts.iter().any(|t| target.hit(t, about, false).is_some())
                });
            if open && !hit {
                continue;
            }
            if let Value::Object(card) = c {
                let had = card.remove("view").is_some() | card.remove("request").is_some();
                if open {
                    card.insert("state".into(), json!("done"));
                }
                changed += usize::from(had || open);
            }
        }
    }
    if let Some(Value::Array(items)) = state.get_mut("proposals") {
        let before = items.len();
        items.retain(|p| !holds(target, p, &tk::str_or_empty(p.get("page"))));
        changed += before - items.len();
    }
    if let Some(Value::Array(items)) = state.get_mut("refile_tried") {
        let before = items.len();
        items.retain(|t| t.as_str().is_some_and(is_print));
        changed += before - items.len();
    }
    changed
}

/// Whether `text` is a fingerprint (`hash`), not a text.
pub(super) fn is_print(text: &str) -> bool {
    text.len() == 16 && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// -- the preview, the token ----------------------------------------------------------------------------------------

/// The preview the app and the card show (contract.md §3.5), without its token.
pub fn view(config: &Config, request: &Request, lang: &str, person: &str) -> Result<Value, Refused> {
    let target = target(config, request)?;
    let plan = plan(config, &target, lang, person);
    if plan.is_empty() {
        return Err(Refused::Nothing);
    }
    let t = texts(lang);
    let title = match (&target.whole, target.count) {
        (Some(_), _) => t.page.replace("{x}", &target.shown),
        (None, 1) => t.one.replace("{x}", &target.shown),
        (None, n) => t.many.replace("{n}", &n.to_string()),
    };
    let mut notes = vec![t.conversation.to_string()];
    if plan.mail {
        notes.push(t.mail.to_string());
    }
    notes.push(t.backups.to_string());
    Ok(json!({"title": title, "found_title": t.found,
              "places": plan.places.iter().map(|p| json!({"text": p})).collect::<Vec<_>>(),
              "notes": notes, "unsure": plan.unsure.iter().map(|u| json!({"text": u})).collect::<Vec<_>>(),
              "confirm": t.confirm, "cancel": t.cancel, "warning": t.warning}))
}

struct Pending {
    request: Request,
    lang: String,
    expires: f64,
    card: Option<String>,
}

static TOKENS: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The app's preview: the view and a token (10 minutes, once). Only `POST /v1/notebook/forget/preview` calls it.
pub fn preview(config: &Config, request: &Request, lang: &str, person: &str, card: Option<String>) -> Result<Value, Refused> {
    let mut out = view(config, request, lang, person)?;
    let now = py::time_time();
    let token = format!("fg-{}", py::token_hex(12));
    let mut tokens = TOKENS.lock().unwrap_or_else(|e| e.into_inner());
    tokens.retain(|_, p| p.expires > now);
    tokens.insert(token.clone(), Pending { request: request.clone(), lang: lang.to_string(), expires: now + TOKEN_SECONDS, card });
    out["token"] = json!(token);
    out["expires_at"] = json!(now + TOKEN_SECONDS);
    Ok(out)
}

/// The confirmation: the token's request erased (Err(None): a token unknown, used or too old).
pub fn confirm(config: &Config, token: &str, person: &str) -> anyhow::Result<Option<String>> {
    let pending = {
        let mut tokens = TOKENS.lock().unwrap_or_else(|e| e.into_inner());
        tokens.remove(token)
    };
    let Some(pending) = pending.filter(|p| p.expires > py::time_time()) else { return Ok(None) };
    let target = match target(config, &pending.request) {
        Ok(t) => t,
        Err(_) => return Ok(Some(texts(&pending.lang).done.to_string())),
    };
    apply(config, &target, &pending.lang, person)?;
    // a paper's record forgotten: its photo too (papers.rs)
    super::papers::sweep(config);
    if let Some(card) = &pending.card {
        set_card(config, card, "done")?;
    }
    Ok(Some(texts(&pending.lang).done.to_string()))
}

// -- the card in the conversation ----------------------------------------------------------------------------------

/// `notebook_write {forget}` by `agent`: the preview put in a card of its turn (never erased by the agent). Returns
/// the tool's answer.
pub fn ask_from_agent(config: &Config, agent: &str, page: &str, texts_asked: &[String], health: bool) -> anyhow::Result<String> {
    if texts_asked.iter().any(|t| t.trim().starts_with("fg-")) {
        return Ok("Only the person confirms, in the app.".into());
    }
    let page_folded = fold(page);
    // the page's name alone among the texts: all of it
    let whole = texts_asked.iter().any(|t| fold(t) == page_folded);
    let texts_kept: Vec<String> = if whole { Vec::new() } else { texts_asked.to_vec() };
    let request = Request { texts: texts_kept, page: Some(page.to_string()).filter(|p| !p.trim().is_empty()), private: health };
    let lang = match config.agent(Some(agent)) {
        Some(a) => crate::handback::language_of(config, a),
        None => super::language(config),
    };
    let shown = match view(config, &request, &lang, "") {
        Ok(v) => v,
        Err(Refused::Several(names)) => return Ok(format!("Several pages: {}. Ask which one.", names.join(", "))),
        Err(_) => return Ok("Nothing to forget: not in the Carnet nor in the memories.".into()),
    };
    let id = format!("bip-forget-{}", py::token_hex(4));
    let (session, run) = super::reads::current().unwrap_or((None, None));
    let card = json!({"id": id, "at": py::time_time(), "agent": agent, "session": session, "run": run, "card": "forget",
                      "state": "open", "request": request, "view": shown});
    super::update_state(config, |state| super::ask::push_card(state, card))?;
    Ok("Shown to the person: they confirm in the app.".into())
}

/// A card's state changed (`done`, `cancelled`). Returns whether it was there.
pub fn set_card(config: &Config, id: &str, to: &str) -> std::io::Result<bool> {
    let mut found = false;
    super::update_state(config, |state| {
        if let Some(Value::Array(items)) = state.get_mut("cards") {
            for c in items.iter_mut().filter(|c| c.get("id").and_then(Value::as_str) == Some(id)) {
                c["state"] = json!(to);
                // answered: what it said is not kept (it named what is forgotten)
                if let Value::Object(card) = c {
                    card.remove("view");
                    card.remove("request");
                }
                found = true;
            }
        }
    })?;
    Ok(found)
}

/// A card's request (what the app sends to `forget/preview` with `card`).
pub fn card_request(config: &Config, id: &str) -> Option<Request> {
    let state = super::read_state(config);
    let card = state.get("cards")?.as_array()?.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id))?;
    if card.get("state").and_then(Value::as_str) != Some("open") {
        return None;
    }
    serde_json::from_value(card.get("request")?.clone()).ok()
}

/// The `carnet_card` items of `agent`'s turns (contract.md §4), by turn, in the order they were made: « Oublie ça »'s
/// preview (here), the question of the day (ask.rs, under the answer) and a follow-up (followups.rs, at the head of
/// the answer: it is made just before the turn starts).
pub fn cards(config: &Config, agent: &str, lang: &str, spans: &[Span]) -> HashMap<String, Vec<Value>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    let state = super::read_state(config);
    let Some(Value::Array(items)) = state.get("cards") else { return out };
    let t = texts(lang);
    let now = py::time_time();
    for c in items.iter().filter(|c| c.get("agent").and_then(Value::as_str) == Some(agent)) {
        let Some(card) = c.as_object() else { continue };
        let at = c.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        let session = tk::str_or_empty(c.get("session"));
        let kind = tk::str_or_empty(c.get("card"));
        let span = if kind == "follow" {
            spans.iter().find(|s| s.session == session && s.from >= at - 5.0)
        } else {
            spans.iter().rev().find(|s| s.session == session && at >= s.from - 1.0 && at < s.to)
        };
        let Some(span) = span else { continue };
        let body = match kind.as_str() {
            "ask" => super::ask::view(card, lang, now),
            "follow" => super::followups::view(card, lang),
            _ => {
                let state = tk::str_or_empty(c.get("state"));
                let mut body = json!({"kind": "carnet_card", "card": "forget", "id": c.get("id"), "state": state});
                match state.as_str() {
                    "open" => {
                        if let Some(Value::Object(view)) = c.get("view") {
                            for (k, v) in view {
                                body[k] = v.clone();
                            }
                        }
                    }
                    "done" => body["text"] = json!(t.done),
                    _ => {}
                }
                Some(body)
            }
        };
        if let Some(body) = body {
            out.entry(span.turn.clone()).or_default().push(body);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Léa's size, as each place said it (Marc's own « Pointure : 36 » stays: another fact).
    const SECRETS: [&str; 4] = ["fait du 36", "chausse du 36", "lea pointure 36", "36 pour lea"];

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        for (name, shown) in [("quotidien", "Quotidien"), ("cuisine", "Cuisine"), ("budget", "Budget")] {
            let agent = crate::config::AgentConfig { name: name.into(), display_name: shown.into(), ..Default::default() };
            config.agents.insert(name.into(), agent);
        }
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    fn memory(config: &Config, agent: &str, which: &str, entries: &[&str]) {
        let a = config.agent(Some(agent)).unwrap().clone();
        let folder = crate::agents_admin::profile_folder(config, &a);
        std::fs::create_dir_all(folder.join("memories")).unwrap();
        let file = if which == "user" { "USER.md" } else { "MEMORY.md" };
        std::fs::write(folder.join("memories").join(file), entries.join(crate::memory_edit::DELIMITER)).unwrap();
    }

    fn entries(config: &Config, agent: &str, which: &str) -> Vec<String> {
        let a = config.agent(Some(agent)).unwrap().clone();
        crate::memory_edit::read_locked(&crate::agents_admin::profile_folder(config, &a), which).unwrap()
    }

    /// Every file under `root` holding one of `needles` (folded, as bytes too).
    fn holding(root: &std::path::Path, needles: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let bytes = std::fs::read(&path).unwrap_or_default();
                let text = fold(&String::from_utf8_lossy(&bytes));
                if needles.iter().any(|n| text.contains(&fold(n))) {
                    out.push(path.display().to_string());
                }
            }
        }
        out
    }

    fn written(config: &Config) {
        let who = Who::agent(config, "quotidien");
        super::super::note(config, Part::Main, "person", "Léa Martin", "", &["Pointure : 36".into(), "Aime le chocolat".into()], &["Léa".into()], "", "", &who).unwrap();
        super::super::note(config, Part::Main, "person", "Marc Dupont", "", &["Pointure : 36".into()], &[], "", "", &who).unwrap();
        let found = super::super::find(config, &[Part::Main], "Léa Martin").unwrap();
        let turn = super::super::reads::Turn { run: Some("r1".into()), session: Some("s1".into()), cap: 6, busy: false };
        super::super::reads::log(config, "quotidien", &turn, Part::Main, &found.page.id(), None, "read");
        memory(config, "cuisine", "user", &["Léa fait du 36", "Alex habite Lyon"]);
        memory(config, "budget", "notes", &["Léa chausse du 36 depuis l'été", "Cadeau : elle fait du 36", "Loyer : 800 €"]);
        let a = config.agent(Some("cuisine")).unwrap().clone();
        let knowledge = crate::agents_admin::profile_folder(config, &a).join("knowledge");
        std::fs::create_dir_all(&knowledge).unwrap();
        std::fs::write(knowledge.join("cadeaux.md"), "# Cadeaux\n- Léa : pointure 36\n- Marc aime le vin\n").unwrap();
        let shared = json!({"merged": true, "common": [{"text": "Léa fait du 36", "by": "cuisine", "at": 1.0},
                                                       {"text": "Alex habite Lyon", "by": "cuisine", "at": 1.0}],
                            "agents": {"cuisine": {"last": ["Léa fait du 36", "Alex habite Lyon"], "seen": ["lea fait du 36"]}}});
        std::fs::write(super::super::root(config).join("shared-user.json"), shared.to_string()).unwrap();
        let news = json!({"agents": {"cuisine": [{"part": "main", "path": found.path, "by": "Quotidien", "writer": "quotidien",
                                                   "text": "Pointure : 36", "at": py::time_time()}]}});
        std::fs::write(super::super::root(config).join("news.json"), news.to_string()).unwrap();
        let family = json!({"lists": [{"id": "l1", "title": "Courses", "kind": "list", "items": [
            {"id": "i1", "text": "Chaussures pointure 36 pour Léa", "by": "Alex"},
            {"id": "i2", "text": "Chaussures pointure 36 pour Léa", "by": "Marc"}]}], "pages": []});
        std::fs::write(PathBuf::from(config.data_dir()).join("family-notebook.json"), family.to_string()).unwrap();
    }

    #[test]
    fn a_fact_said_another_way_is_found_and_erased_everywhere_but_the_conversations() {
        let (dir, mut config) = setup();
        config.archives = vec![json!({"agent": "quotidien", "id": "mails", "name": "Mails", "kind": "mail", "account": "x"})];
        written(&config);
        let archive = crate::archives::all(&config).remove(0);
        let index_file = crate::archives::index_path(&config, &archive);
        {
            let index = crate::archives::index::Index::open(&index_file).unwrap();
            let doc = crate::archives::extract::Doc { title: "Commande".into(), sender: "Shop".into(), date: "2025-01-02".into(),
                                                      text: "Chaussures pour Léa, pointure 36".into() };
            index.insert("INBOX\u{1f}1\u{1f}7", "INBOX", 0, 0, "mail", &doc, 1).unwrap();
            let other = crate::archives::extract::Doc { text: "Facture d'électricité".into(), ..doc.clone() };
            index.insert("INBOX\u{1f}1\u{1f}8", "INBOX", 0, 0, "mail", &other, 1).unwrap();
        }
        let request = Request { texts: vec!["Pointure : 36".into()], page: Some("Léa".into()), private: false };
        let v = view(&config, &request, "fr", "Alex").unwrap();
        assert_eq!(v["title"], "Oublier « Pointure : 36 » ?");
        let places: Vec<&str> = v["places"].as_array().unwrap().iter().map(|p| p["text"].as_str().unwrap()).collect();
        for want in ["Carnet · Léa", "Historique du Carnet", "Mémoire de Cuisine", "Mémoire de Budget",
                     "Carnet de la famille (visible par ta famille)"] {
            assert!(places.contains(&want), "{want} in {places:?}");
        }
        assert!(!places.iter().any(|p| p.contains("Marc")), "Marc's own size is another fact: {places:?}");
        let unsure: Vec<&str> = v["unsure"].as_array().unwrap().iter().map(|p| p["text"].as_str().unwrap()).collect();
        assert_eq!(unsure, ["Mémoire de Budget : « Cadeau : elle fait du 36 »"]);
        assert!(places.contains(&"Archive Mails (retiré de la recherche)"), "{places:?}");
        assert_eq!(v["notes"][1], "Le mail lui-même reste dans ta boîte.");
        assert_eq!(v["notes"][0], "Reste dans la conversation où tu l'as dit.");
        assert_eq!(v["notes"].as_array().unwrap().last().unwrap(), "Disparaît aussi des sauvegardes sous 30 jours.");
        // the token: once, for the app's route only
        let shown = preview(&config, &request, "fr", "Alex", None).unwrap();
        let token = shown["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("fg-"));
        assert_eq!(confirm(&config, &token, "Alex").unwrap().as_deref(), Some("Oublié."));
        assert_eq!(confirm(&config, &token, "Alex").unwrap(), None, "used once");
        // nothing left anywhere the bridge keeps it (Hermes' conversations are not here)
        let left = holding(dir.path(), &SECRETS);
        assert!(left.is_empty(), "still there: {left:?}");
        // Léa's page as it was, in the history: the line erased there too
        for file in holding(&super::super::root(&config).join("history/versions"), &["Léa Martin"]) {
            let text = fold(&std::fs::read_to_string(&file).unwrap());
            assert!(!text.contains("pointure 36"), "{file}: {text}");
        }
        let news = std::fs::read_to_string(super::super::root(&config).join("news.json")).unwrap();
        assert!(!news.contains("36"), "{news}");
        assert_eq!(entries(&config, "cuisine", "user"), ["Alex habite Lyon"]);
        assert_eq!(entries(&config, "budget", "notes"), ["Loyer : 800 €"]);
        assert_eq!(crate::archives::index::Index::open(&index_file).unwrap().count(), 1, "the mail about Léa's size out of the search");
        let lea = super::super::find(&config, &[Part::Main], "Léa Martin").unwrap();
        assert!(lea.page.body.contains("Aime le chocolat"));
        let marc = super::super::find(&config, &[Part::Main], "Marc Dupont").unwrap();
        assert!(marc.page.body.contains("Pointure : 36"), "Marc's kept");
        let family = std::fs::read_to_string(PathBuf::from(config.data_dir()).join("family-notebook.json")).unwrap();
        assert!(family.contains("\"Marc\"") && !family.contains("\"Alex\""), "only the person's own line: {family}");
        // the log says something was forgotten, never what
        let last = super::super::history(&config, None, None, 1);
        assert_eq!(last[0]["action"], "forgotten");
        assert_eq!(last[0]["summary"], "");
        // an old turn writes it back, said another way: refused
        let who = Who::agent(&config, "cuisine");
        let w = super::super::note(&config, Part::Main, "person", "Léa Martin", "", &["Chausse du 36".into()], &[], "", "", &who).unwrap();
        assert!(w.added.is_empty(), "{:?}", w.added);
        assert!(is_forgotten(&config, "Léa fait du 36", ""));
        assert!(is_forgotten(&config, "Pointure : 36", "Léa Martin"));
        assert!(!is_forgotten(&config, "Pointure : 36", "Marc Dupont"));
        assert!(!is_forgotten(&config, "Aime le chocolat", "Léa Martin"));
        let state = super::super::read_state(&config);
        let prints = state["forgotten"].to_string().to_lowercase();
        assert!(!prints.contains("pointure") && !prints.contains("lea"), "fingerprints only: {prints}");
    }

    #[test]
    fn a_whole_page_goes_with_its_reads_and_the_lines_naming_it() {
        let (dir, config) = setup();
        written(&config);
        let request = Request { texts: vec![], page: Some("Léa Martin".into()), private: false };
        let v = view(&config, &request, "fr", "Alex").unwrap();
        assert_eq!(v["title"], "Oublier la page « Léa Martin » ?");
        let shown = preview(&config, &request, "fr", "Alex", None).unwrap();
        confirm(&config, shown["token"].as_str().unwrap(), "Alex").unwrap().unwrap();
        assert!(super::super::find(&config, &[Part::Main], "Léa Martin").is_none());
        assert!(super::super::reads::entries(&config).is_empty(), "its reads gone");
        let left = holding(&super::super::root(&config), &["Léa Martin", "aime le chocolat"]);
        assert!(left.is_empty(), "{left:?}");
        assert!(holding(dir.path(), &["Léa fait du 36", "Léa chausse du 36"]).is_empty());
        assert_eq!(entries(&config, "cuisine", "user"), ["Alex habite Lyon"]);
    }

    #[test]
    fn refusals_and_the_card_an_agent_puts_in_the_conversation() {
        let (_dir, config) = setup();
        written(&config);
        let who = Who::agent(&config, "quotidien");
        super::super::note(&config, Part::Main, "person", "Léa Petit", "", &["Aime le thé".into()], &[], "", "", &who).unwrap();
        // two Léa: the app is asked which
        let two = Request { texts: vec!["Pointure : 36".into()], page: Some("Léa".into()), private: false };
        assert!(matches!(view(&config, &two, "fr", ""), Err(Refused::Several(n)) if n.len() == 2));
        let gone = Request { texts: vec![], page: Some("Personne".into()), private: false };
        assert_eq!(view(&config, &gone, "fr", "").unwrap_err(), Refused::PageGone);
        let nothing = Request { texts: vec!["Aime le ski".into()], page: Some("Marc Dupont".into()), private: false };
        assert_eq!(view(&config, &nothing, "fr", "").unwrap_err(), Refused::Nothing);
        // the agent: a preview in a card, never a token, nothing erased
        let turn = super::super::reads::Turn { run: Some("r9".into()), session: Some("s9".into()), cap: 6, busy: false };
        let answer = super::super::reads::with_turn(&turn, || {
            ask_from_agent(&config, "quotidien", "Léa Martin", &["Pointure : 36".into()], false).unwrap()
        });
        assert_eq!(answer, "Shown to the person: they confirm in the app.");
        assert_eq!(ask_from_agent(&config, "quotidien", "Léa Martin", &["fg-123".into()], false).unwrap(), "Only the person confirms, in the app.");
        assert!(super::super::find(&config, &[Part::Main], "Léa Martin").unwrap().page.body.contains("Pointure : 36"));
        let state = super::super::read_state(&config);
        let card = &state["cards"][0];
        assert!(card.get("token").is_none() && !card.to_string().contains("fg-"));
        let span = Span { turn: "t1".into(), session: "s9".into(), from: 0.0, to: f64::MAX };
        let made = cards(&config, "quotidien", "fr", std::slice::from_ref(&span));
        let item = &made["t1"][0];
        assert_eq!((item["kind"].as_str(), item["card"].as_str(), item["state"].as_str()), (Some("carnet_card"), Some("forget"), Some("open")));
        assert_eq!(item["title"], "Oublier « Pointure : 36 » ?");
        // the app confirms the card: its preview by the card, then the token
        let id = item["id"].as_str().unwrap().to_string();
        let request = card_request(&config, &id).unwrap();
        let shown = preview(&config, &request, "fr", "", Some(id.clone())).unwrap();
        confirm(&config, shown["token"].as_str().unwrap(), "").unwrap().unwrap();
        let made = cards(&config, "quotidien", "fr", &[span]);
        assert_eq!(made["t1"][0]["state"], "done");
        assert_eq!(made["t1"][0]["text"], "Oublié.");
        assert!(!super::super::read_state(&config)["cards"].to_string().contains("Pointure"), "answered: nothing of it kept");
        assert!(card_request(&config, &id).is_none());
    }

    /// Bench O1 (9/11): the agent's own cards stayed open after the app's confirmation and `refile_tried` kept the
    /// text. Nothing of it is left in `state.json` now: cards, proposals, the nightly pass's tried lines.
    #[test]
    fn the_carnets_own_state_keeps_nothing_of_it() {
        let (_dir, config) = setup();
        written(&config);
        let lea = super::super::find(&config, &[Part::Main], "Léa Martin").unwrap().path;
        let marc = super::super::find(&config, &[Part::Main], "Marc Dupont").unwrap().path;
        let turn = super::super::reads::Turn { run: Some("r9".into()), session: Some("s9".into()), cap: 6, busy: false };
        let answers = super::super::reads::with_turn(&turn, || {
            [ask_from_agent(&config, "quotidien", "Léa Martin", &["Pointure : 36".into()], false).unwrap(),
             ask_from_agent(&config, "quotidien", "Léa Martin", &["Léa chausse du 36".into()], false).unwrap()]
        });
        assert_eq!(answers, ["Shown to the person: they confirm in the app."; 2]);
        super::super::mark_tried(&config, &[super::super::Unsorted { page: "Marc Dupont".into(), kind: "person".into(), fact: "Aime le vin".into() }]).unwrap();
        super::super::update_state(&config, |s| {
            let cards = s.get_mut("cards").unwrap().as_array_mut().unwrap();
            cards.push(json!({"id": "bip-ask-1", "card": "ask", "agent": "quotidien", "state": "done", "path": lea,
                              "title": "Léa Martin", "label": "Pointure", "choice": "36", "question": "Léa chausse du combien ?"}));
            cards.push(json!({"id": "bip-ask-2", "card": "ask", "agent": "quotidien", "state": "done", "path": marc,
                              "title": "Marc Dupont", "label": "Plat préféré", "choice": "pizza", "question": "Son plat ?"}));
            cards.push(json!({"id": "bip-follow-1", "card": "follow", "agent": "quotidien", "state": "open",
                              "title": "Acheter des chaussures pointure 36 pour Léa"}));
            s.insert("proposals".into(), json!([{"id": "q1", "title": "Offrir des chaussures pointure 36 à Léa", "page": "", "state": "open"},
                                                {"id": "q2", "title": "Appeler le plombier", "page": "", "state": "open"}]));
            let mut tried = s["refile_tried"].as_array().unwrap().clone();
            tried.push(json!("lea pointure 36"));
            s.insert("refile_tried".into(), json!(tried));
        })
        .unwrap();
        // the app confirms its own preview (not the agent's cards)
        let request = Request { texts: vec!["Pointure : 36".into()], page: Some("Léa".into()), private: false };
        let shown = preview(&config, &request, "fr", "Alex", None).unwrap();
        confirm(&config, shown["token"].as_str().unwrap(), "Alex").unwrap().unwrap();
        let state = super::super::read_state(&config);
        let text = fold(&Value::Object(state.clone()).to_string());
        for gone in ["pointure", "chausse", "chaussures", "lea martin"] {
            assert!(!text.contains(gone), "{gone} still in state.json: {text}");
        }
        let cards = state["cards"].as_array().unwrap();
        let forget: Vec<&Value> = cards.iter().filter(|c| c["card"] == "forget").collect();
        assert_eq!(forget.len(), 2);
        assert!(forget.iter().all(|c| c["state"] == "done" && c.get("view").is_none() && c.get("request").is_none()), "{forget:?}");
        let ids: Vec<&str> = cards.iter().filter(|c| c["card"] != "forget").filter_map(|c| c["id"].as_str()).collect();
        assert_eq!(ids, ["bip-ask-2"], "Marc's question stays");
        let proposals: Vec<&str> = state["proposals"].as_array().unwrap().iter().filter_map(|p| p["id"].as_str()).collect();
        assert_eq!(proposals, ["q2"]);
        let tried = super::super::strings(state.get("refile_tried"));
        assert_eq!(tried.len(), 1);
        assert!(is_print(&tried[0]), "{tried:?}");
    }

    #[test]
    fn a_token_is_refused_after_ten_minutes() {
        let (_dir, config) = setup();
        written(&config);
        let request = Request { texts: vec!["Aime le chocolat".into()], page: Some("Léa Martin".into()), private: false };
        let shown = preview(&config, &request, "fr", "", None).unwrap();
        let token = shown["token"].as_str().unwrap().to_string();
        TOKENS.lock().unwrap().get_mut(&token).unwrap().expires = py::time_time() - 1.0;
        assert_eq!(confirm(&config, &token, "").unwrap(), None);
        assert!(super::super::find(&config, &[Part::Main], "Léa Martin").unwrap().page.body.contains("chocolat"));
    }

    #[test]
    fn forms_find_a_fact_said_another_way() {
        assert!(forms("Léa chausse du 36").contains(&("shoe_size".into(), "36".into())));
        assert!(forms("Elle fait du 36 depuis l'été").contains(&("shoe_size".into(), "36".into())));
        assert!(forms("Pointure : 36").contains(&("shoe_size".into(), "36".into())));
        assert!(forms("Née le 12 mars").contains(&("birthday".into(), "12 mars".into())));
        assert!(!forms("Aime le vélo").iter().any(|(l, _)| l == "shoe_size"));
    }
}
