//! « Ce que tes agents savent de toi » (Carnet v2, decision 9 of internal/notes/carnet-v2/spec.md, contract.md §3.4):
//! one page, ready to show, made by the bridge: what every agent always has in mind (the common memory, `USER.md`),
//! what each read in the Carnet and when (the reads journal, reads.rs: 90 days), and what they never see. No page's
//! text but « Toujours en tête »: the reads are page names. The home's tile (`tile`) leads to it.
//!
//!     GET /v1/notebook/known?agent=&lang=

use std::collections::HashMap;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{lexicon, Part};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// Rows of « Lu récemment » at most.
pub const ROWS_MAX: usize = 200;
/// A row gathers one turn's reads; reads without a turn (the news), those of the same minute.
const NEWS_MINUTES: f64 = 60.0;

pub struct Texts {
    pub title: &'static str,
    pub all: &'static str,
    pub always: &'static str,
    pub reads: &'static str,
    pub today: &'static str,
    pub yesterday: &'static str,
    pub older: &'static str,
    pub empty: &'static str,
    pub unseen: &'static str,
    pub health_line: &'static str,
    pub archives_line: &'static str,
    pub footer: &'static str,
    pub gone: &'static str,
    pub old_agent: &'static str,
    pub health: &'static str,
    pub tile: &'static str,
    pub tile_one: &'static str,
    pub tile_many: &'static str,
}

/// The footer is the app's one sentence of honesty: only what is true by contract (no « pas d'entraînement »: the
/// model's provider is not fixed in the privacy policy yet, internal/notes/carnet-v2/build-notes.md B2).
pub fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts {
            title: "Ce que tes agents savent de toi", all: "Tous", always: "Toujours en tête", reads: "Lu récemment",
            today: "Aujourd'hui", yesterday: "Hier", older: "Plus ancien : effacé après 90 jours.",
            empty: "Aucune lecture pour l'instant.", unseen: "Ce qu'ils ne voient pas",
            health_line: "Ta santé : seulement ton agent Santé.", archives_line: "Tes archives : seulement l'agent relié.",
            footer: "Pour te répondre, le modèle d'IA reçoit seulement ce que ton agent lui envoie. Kerfy Studio ne lit pas ton Carnet. Pas de pub.",
            gone: "Page oubliée", old_agent: "Ancien agent", health: "Santé", tile: "Ce que tes agents savent",
            tile_one: "1 page lue ces 7 jours", tile_many: "{n} pages lues ces 7 jours",
        },
        "de" => Texts {
            title: "Was deine Agenten über dich wissen", all: "Alle", always: "Immer im Kopf", reads: "Kürzlich gelesen",
            today: "Heute", yesterday: "Gestern", older: "Älter: nach 90 Tagen gelöscht.", empty: "Noch nichts gelesen.",
            unseen: "Was sie nicht sehen", health_line: "Deine Gesundheit: nur dein Gesundheits-Agent.",
            archives_line: "Deine Archive: nur der verbundene Agent.",
            footer: "Um dir zu antworten, bekommt das KI-Modell nur, was dein Agent ihm schickt. Kerfy Studio liest dein Notizbuch nicht. Keine Werbung.",
            gone: "Vergessene Seite", old_agent: "Früherer Agent", health: "Gesundheit", tile: "Was deine Agenten wissen",
            tile_one: "1 Seite in 7 Tagen gelesen", tile_many: "{n} Seiten in 7 Tagen gelesen",
        },
        "es" => Texts {
            title: "Lo que tus agentes saben de ti", all: "Todos", always: "Siempre presente", reads: "Leído hace poco",
            today: "Hoy", yesterday: "Ayer", older: "Más antiguo: borrado a los 90 días.",
            empty: "Ninguna lectura por ahora.", unseen: "Lo que no ven", health_line: "Tu salud: solo tu agente de Salud.",
            archives_line: "Tus archivos: solo el agente vinculado.",
            footer: "Para responderte, el modelo de IA solo recibe lo que tu agente le envía. Kerfy Studio no lee tu Cuaderno. Sin publicidad.",
            gone: "Página olvidada", old_agent: "Agente anterior", health: "Salud", tile: "Lo que saben tus agentes",
            tile_one: "1 página leída en 7 días", tile_many: "{n} páginas leídas en 7 días",
        },
        _ => Texts {
            title: "What your agents know about you", all: "All", always: "Always in mind", reads: "Read recently",
            today: "Today", yesterday: "Yesterday", older: "Older: erased after 90 days.", empty: "No reads yet.",
            unseen: "What they don't see", health_line: "Your health: only your Health agent.",
            archives_line: "Your archives: only the agent they're linked to.",
            footer: "To answer you, the AI model only gets what your agent sends it. Kerfy Studio doesn't read your Notebook. No ads.",
            gone: "Forgotten page", old_agent: "Former agent", health: "Health", tile: "What your agents know",
            tile_one: "1 page read in 7 days", tile_many: "{n} pages read in 7 days",
        },
    }
}

fn key(text: &str) -> String {
    format!("k-{}", &hex::encode(Sha256::digest(super::fold(text).as_bytes()))[..8])
}

fn at_of(entry: &Dict) -> f64 {
    entry.get("at").and_then(Value::as_f64).unwrap_or(0.0)
}

/// « Toujours en tête »: the common memory (an agent's own `USER.md` with `agent`), health left out.
fn always(config: &Config, agent: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let own: Vec<(String, std::path::PathBuf)> = config
        .agents
        .values()
        .filter(|a| agent.is_empty() || a.name == agent)
        .filter(|a| super::enabled(config, &a.name))
        .map(|a| (a.name.clone(), crate::agents_admin::profile_folder(config, a)))
        .collect();
    let common = if agent.is_empty() { crate::shared_memory::common_texts(config) } else { Vec::new() };
    let from_files = || {
        own.iter()
            .filter_map(|(_, folder)| crate::memory_edit::read_locked(folder, "user"))
            .flatten()
            .collect::<Vec<String>>()
    };
    let entries = if common.is_empty() { from_files() } else { common };
    for e in entries {
        let text = crate::shared_memory::strip_stamps(e.trim());
        if text.is_empty() || lexicon::is_health(&text) {
            continue;
        }
        if !lines.iter().any(|l| super::fold(l) == super::fold(&text)) {
            lines.push(text);
        }
    }
    lines
}

/// The pages of the reads, by id: (part, path, page).
fn pages_by_id(config: &Config) -> HashMap<String, (Part, String, super::Page)> {
    [Part::Main, Part::Health]
        .into_iter()
        .flat_map(|part| super::all_pages(config, part).into_iter().map(move |(p, page)| (page.id(), (part, p, page))))
        .filter(|(id, _)| !id.is_empty())
        .collect()
}

/// The page, ready to show (contract.md §3.4).
pub fn view(config: &Config, agent: &str, lang: &str, now: f64) -> Value {
    let t = texts(lang);
    let mut agents = vec![json!({"id": "", "name": t.all})];
    for a in config.agents.values().filter(|a| super::enabled(config, &a.name)) {
        let name = if a.display_name.is_empty() { a.name.clone() } else { a.display_name.clone() };
        agents.push(json!({"id": a.name, "name": name, "color": a.color}));
    }
    let always_lines: Vec<Value> = always(config, agent).iter().map(|l| json!({"key": key(l), "text": l})).collect();
    // « Lu récemment »: one row per turn (its run, else its session), the newest first
    let by_id = pages_by_id(config);
    let mut rows: Vec<(f64, String, Vec<Value>)> = Vec::new();
    let mut seen_turn: HashMap<String, usize> = HashMap::new();
    let entries = super::reads::entries(config);
    for e in entries.iter().rev() {
        let who = tk::str_or_empty(e.get("agent"));
        if !agent.is_empty() && who != agent {
            continue;
        }
        let at = at_of(e);
        if now - at > super::reads::KEPT_DAYS * 86400.0 {
            continue;
        }
        let turn = match (e.get("run").and_then(Value::as_str), e.get("session").and_then(Value::as_str)) {
            (Some(run), _) => format!("{who}\u{1f}{run}"),
            (None, Some(session)) => format!("{who}\u{1f}{session}\u{1f}{}", (at / 600.0) as i64),
            _ => format!("{who}\u{1f}@{}", (at / NEWS_MINUTES) as i64),
        };
        let health = by_id.get(&tk::str_or_empty(e.get("page_id"))).is_some_and(|(p, _, _)| *p == Part::Health)
            || tk::str_or_empty(e.get("part")) == Part::Health.as_str();
        let page = match by_id.get(&tk::str_or_empty(e.get("page_id"))) {
            _ if health => json!({"title": t.health, "path": null, "locked": true}),
            Some((_, path, page)) => json!({"title": super::reads::short(page, lang), "path": path}),
            None => json!({"title": t.gone, "path": null}),
        };
        let index = match seen_turn.get(&turn) {
            Some(i) => *i,
            None => {
                if rows.len() >= ROWS_MAX {
                    continue;
                }
                rows.push((at, who.clone(), Vec::new()));
                seen_turn.insert(turn, rows.len() - 1);
                rows.len() - 1
            }
        };
        if !rows[index].2.contains(&page) {
            rows[index].2.push(page);
        }
    }
    let today = chrono::DateTime::from_timestamp(now as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local).date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    let mut days: Vec<Value> = Vec::new();
    for (at, who, pages) in rows {
        let Some(local) = chrono::DateTime::from_timestamp(at as i64, 0).map(|d| d.with_timezone(&chrono::Local)) else {
            continue;
        };
        let date = local.date_naive();
        let label = match (today - date).num_days() {
            0 => t.today.to_string(),
            1 => t.yesterday.to_string(),
            _ => super::offline::day(date, lang),
        };
        let a = config.agent(Some(&who));
        let name = match a {
            Some(a) if !a.display_name.is_empty() => a.display_name.clone(),
            Some(a) => a.name.clone(),
            None => t.old_agent.to_string(),
        };
        let text = pages.iter().filter_map(|p| p.get("title").and_then(Value::as_str)).collect::<Vec<_>>().join(", ");
        let row = json!({"agent": who, "name": name, "color": a.and_then(|a| a.color.clone()), "pages": pages,
                         "text": text, "time": local.format("%H:%M").to_string()});
        match days.last_mut() {
            Some(d) if d["day"] == json!(label) => {
                if let Some(Value::Array(list)) = d.get_mut("rows") {
                    list.push(row);
                }
            }
            _ => days.push(json!({"day": label, "rows": [row]})),
        }
    }
    let empty = days.is_empty();
    let mut unseen: Vec<&str> = Vec::new();
    if super::any_health_agent(config) || !super::all_pages(config, Part::Health).is_empty() {
        unseen.push(t.health_line);
    }
    if !crate::archives::all(config).is_empty() {
        unseen.push(t.archives_line);
    }
    json!({"title": t.title, "agents": agents,
           "always": {"title": t.always, "lines": always_lines},
           "reads": {"title": t.reads, "days": days, "older": if empty { Value::Null } else { json!(t.older) },
                     "empty": if empty { json!(t.empty) } else { Value::Null }},
           "unseen": {"title": t.unseen, "lines": unseen},
           "footer": t.footer})
}

/// The home's tile: {title, text} (the pages read in the last 7 days).
pub fn tile(config: &Config, lang: &str, now: f64) -> Value {
    let t = texts(lang);
    let mut ids: Vec<String> = Vec::new();
    for e in super::reads::entries(config) {
        let id = tk::str_or_empty(e.get("page_id"));
        if now - at_of(&e) <= 7.0 * 86400.0 && !ids.contains(&id) {
            ids.push(id);
        }
    }
    let text = match ids.len() {
        0 => t.empty.to_string(),
        1 => t.tile_one.to_string(),
        n => t.tile_many.replace("{n}", &n.to_string()),
    };
    json!({"title": t.tile, "text": text})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_shows_names_never_page_text_and_ninety_days() {
        let (_dir, config) = super::super::offline::tests::carnet();
        let lea = super::super::find(&config, &[Part::Main], "Léa Martin").unwrap();
        let turn = super::super::reads::Turn { run: Some("r1".into()), session: Some("s1".into()), cap: 6, busy: false };
        super::super::reads::log(&config, "quotidien", &turn, Part::Main, &lea.page.id(), Some("tastes"), "read");
        super::super::reads::log(&config, "ancien", &turn, Part::Main, "p-gone", None, "read");
        let folder = crate::agents_admin::profile_folder(&config, config.agent(Some("quotidien")).unwrap());
        std::fs::create_dir_all(folder.join("memories")).unwrap();
        std::fs::write(folder.join("memories/USER.md"), "Prénom : Alex\n§\nAllergique aux arachides").unwrap();
        let now = crate::py::time_time();
        let v = view(&config, "", "fr", now);
        assert_eq!(v["title"], "Ce que tes agents savent de toi");
        assert_eq!(v["always"]["lines"].as_array().unwrap().iter().map(|l| l["text"].clone()).collect::<Vec<_>>(), [json!("Prénom : Alex")], "health left out");
        let day = &v["reads"]["days"][0];
        assert_eq!(day["day"], "Aujourd'hui");
        let rows = day["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let mine = rows.iter().find(|r| r["agent"] == "quotidien").unwrap();
        assert_eq!(mine["name"], "Quotidien");
        assert_eq!(mine["text"], "Léa");
        let old = rows.iter().find(|r| r["agent"] == "ancien").unwrap();
        assert_eq!((old["name"].as_str(), old["text"].as_str()), (Some("Ancien agent"), Some("Page oubliée")));
        assert_eq!(v["reads"]["older"], "Plus ancien : effacé après 90 jours.");
        assert!(v["footer"].as_str().unwrap().starts_with("Pour te répondre"));
        // no page text: the facts of the pages read never appear
        let text = v.to_string();
        assert!(!text.contains("36") && !text.contains("12 mars") && !text.contains("arachides"), "{text}");
        // one agent only; past 90 days, nothing
        let only = view(&config, "ancien", "fr", now);
        assert_eq!(only["reads"]["days"][0]["rows"].as_array().unwrap().len(), 1);
        let later = view(&config, "", "fr", now + 91.0 * 86400.0);
        assert_eq!(later["reads"]["empty"], "Aucune lecture pour l'instant.");
        assert_eq!(tile(&config, "fr", now)["text"], "2 pages lues ces 7 jours");
        // a fact forgotten leaves « Toujours en tête »
        let request = super::super::forget::Request { texts: vec!["Prénom : Alex".into()], page: None, private: false };
        let shown = super::super::forget::preview(&config, &request, "fr", "", None).unwrap();
        super::super::forget::confirm(&config, shown["token"].as_str().unwrap(), "").unwrap().unwrap();
        assert!(view(&config, "", "fr", now)["always"]["lines"].as_array().unwrap().is_empty());
    }
}
