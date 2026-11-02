//! The Carnet's language changed after it has pages (2026-11-02: a place claimed in French, used in English). Its
//! folders and its own pages are named in its language (« Personnes/ », « Toi.md », « Tâches/Tâches.md »…), and its
//! pages' sections too (« ## Qui c'est »). `PATCH /v1/notebook/settings {language}` moves them all to the new
//! language's names, safely:
//!
//! - every move is worked out and checked first (a page already where one would go: nothing is moved, the person is
//!   told); then each page is moved as one change of the log (`action: "language"`, `moved_from`/`moved_to`: a restore
//!   moves it back), its sections re-titled in the new language (template::normalize), the own pages' titles
//!   (« Toi » → « You ») renamed with the old name kept as an alias (links to it still find it);
//! - the people's and subjects' own names never change; nor do the facts and their attributions;
//! - `state.json`'s paths follow (the pages' paths kept there, the folders shared); the empty old folders are removed;
//! - the « Nouveau » list leaves these changes out (housekeeping, not news: mod.rs `recent_changes`).
//!
//! What the apps show does not depend on it (api.rs `shown_language`: the app's language); this only changes what the
//! agents write and what the person sees in Obsidian.

use std::collections::{BTreeMap, HashSet};

use serde_json::{json, Value};

use super::{part_root, read_page_text, words, Page, Part, Who, Words};
use crate::config::Config;

/// The action of these changes in the log.
pub const ACTION: &str = "language";

/// The folder `name` (one path component) in the new language: a folder of the Carnet's own, else as it is.
fn folder(old: &Words, new: &Words, name: &str) -> String {
    [
        (old.people, new.people),
        (old.subjects, new.subjects),
        (old.areas, new.areas),
        (old.tasks, new.tasks),
        (old.trips, new.trips),
        (old.records, new.records),
        (old.archives, new.archives),
    ]
    .iter()
    .find(|(o, _)| *o == name)
    .map(|(_, n)| n.to_string())
    .unwrap_or_else(|| name.to_string())
}

/// A record type's table named in the old language (« Achats »): its name in the new one.
fn record_title(old_lang: &str, new_lang: &str, stem: &str) -> Option<String> {
    let t = super::records::kind(stem)?;
    let index = |lang: &str| match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    };
    (t.titles[index(old_lang)] == stem).then(|| t.titles[index(new_lang)].to_string())
}

/// The page's own name (its file's stem) in the new language, when it is one of the Carnet's own pages.
fn own_name(old: &Words, new: &Words, parents: &[&str], stem: &str) -> Option<String> {
    match parents {
        [] if stem == old.me => Some(new.me.into()),
        [] if stem == old.inbox => Some(new.inbox.into()),
        [] if stem == old.documents => Some(new.documents.into()),
        [p] if *p == old.tasks && stem == old.tasks => Some(new.tasks.into()),
        [p] if *p == old.areas && stem == old.preferences => Some(new.preferences.into()),
        [p] if *p == old.areas && stem == old.health => Some(new.health.into()),
        [p] | [_, p] if *p == old.records => record_title(old.lang, new.lang, stem),
        _ => None,
    }
}

/// A page's path in the new language (`Personnes/Marc.md` → `People/Marc.md`, `Toi.md` → `You.md`,
/// `Archives/Fiches/Achats.md` → `Archive/Records/Purchases.md`).
pub fn moved_path(old: &Words, new: &Words, path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    let Some((file, parents)) = parts.split_last() else { return path.to_string() };
    let stem = file.strip_suffix(".md").unwrap_or(file);
    let mut out: Vec<String> = Vec::new();
    for (i, name) in parents.iter().enumerate() {
        // a folder of the Carnet's at the top, or the trips' and records' under the archives
        let own = i == 0 || (i == 1 && parents[0] == old.archives);
        out.push(if own { folder(old, new, name) } else { name.to_string() });
    }
    let name = own_name(old, new, parents, stem).unwrap_or_else(|| stem.to_string());
    out.push(format!("{name}.md"));
    out.join("/")
}

/// The page's text in the new language: its sections' names, its own title (the old one kept as an alias).
fn retitled(old: &Words, new: &Words, path: &str, text: &str) -> String {
    let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(".md");
    let mut page = Page::parse(text, stem);
    let kind = page.kind();
    if !super::template::outline(&kind).is_empty() {
        page.body = super::template::normalize(&kind, new.lang, new.brief, &page.body).body;
    } else if kind == "tasks" {
        page.body = page
            .body
            .lines()
            .map(|l| if l.trim_end() == format!("## {}", old.archived) { format!("## {}", new.archived) } else { l.to_string() })
            .collect::<Vec<_>>()
            .join("\n")
            + if page.body.ends_with('\n') { "\n" } else { "" };
    }
    let parts: Vec<&str> = path.split('/').collect();
    let parents = &parts[..parts.len().saturating_sub(1)];
    let own = own_name(old, new, parents, stem);
    let renamed = match (kind.as_str(), own) {
        ("self", _) if page.title == old.me => Some(new.me.to_string()),
        (_, Some(name)) if page.title == stem => Some(name),
        _ => None,
    };
    if let Some(title) = renamed.filter(|t| *t != page.title) {
        let mut aliases = page.aliases();
        if !aliases.iter().any(|a| super::fold(a) == super::fold(&page.title)) {
            aliases.push(page.title.clone());
        }
        page.set_aliases(&aliases);
        page.title = title;
    }
    page.render()
}

/// `state.json`'s strings and keys that are paths of moved pages, in the new language.
fn moved_state(value: &mut Value, pages: &BTreeMap<String, String>) {
    match value {
        Value::String(s) => {
            if let Some(to) = pages.get(s.as_str()) {
                *s = to.clone();
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| moved_state(v, pages)),
        Value::Object(map) => {
            let old = std::mem::take(map);
            for (key, mut v) in old {
                moved_state(&mut v, pages);
                map.insert(pages.get(&key).cloned().unwrap_or(key), v);
            }
        }
        _ => {}
    }
}

/// The Carnet moved to `lang`'s names (the caller holds the lock). Returns how many pages were moved or re-titled.
pub fn change_locked(config: &Config, lang: &str, who: &Who) -> anyhow::Result<usize> {
    let old_lang = super::language(config);
    let (old, new) = (words(&old_lang), words(lang));
    if old.lang == new.lang {
        super::update_state(config, |s| {
            s.insert("language".into(), json!(new.lang));
        })?;
        return Ok(0);
    }
    // every move worked out and checked before anything is written
    let mut plan: Vec<(Part, String, String, String)> = Vec::new();
    for part in [Part::Main, Part::Health] {
        let mut targets: HashSet<String> = HashSet::new();
        for (path, _) in super::all_pages(config, part) {
            let Some(text) = read_page_text(config, part, &path) else { continue };
            let to = moved_path(old, new, &path);
            super::checked(&to)?;
            if to != path && part_root(config, part).join(&to).exists() {
                return super::refuse(format!("a page is already there: {to}"));
            }
            if !targets.insert(to.clone()) {
                return super::refuse(format!("two pages would go to {to}"));
            }
            let after = retitled(old, new, &path, &text);
            plan.push((part, path, to, after));
        }
    }
    super::update_state(config, |s| {
        s.insert("language".into(), json!(new.lang));
    })?;
    let summary = format!("{} → {}", old.lang, new.lang);
    let mut changed = 0;
    let mut pages: BTreeMap<String, String> = BTreeMap::new();
    for (part, from, to, text) in &plan {
        let before = read_page_text(config, *part, from);
        if from == to && before.as_deref() == Some(text.as_str()) {
            continue;
        }
        super::move_page_locked(config, *part, from, to, text, who, ACTION, &summary)?;
        if from != to && *part == Part::Main {
            pages.insert(from.clone(), to.clone());
        }
        changed += 1;
    }
    // the paths kept in state.json, and the shared folders
    let folders = |name: &str| {
        let parts: Vec<&str> = name.split('/').collect();
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| if i == 0 || (i == 1 && parts[0] == old.archives) { folder(old, new, p) } else { p.to_string() })
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut state = Value::Object(super::read_state(config));
    moved_state(&mut state, &pages);
    // a whole folder shared (`shares.folders`, by its path in the vault)
    if let Some(Value::Object(shared)) = state.get_mut("shares").and_then(|s| s.get_mut("folders")) {
        let old_map = std::mem::take(shared);
        for (key, v) in old_map {
            shared.insert(folders(&key), v);
        }
    }
    if let Value::Object(map) = state {
        super::write_state(config, &map)?;
    }
    // the old folders, when nothing is left in them (deepest first)
    for part in [Part::Main, Part::Health] {
        let base = part_root(config, part);
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        for top in [old.people, old.subjects, old.areas, old.tasks, old.trips, old.records, old.archives] {
            if [new.people, new.subjects, new.areas, new.tasks, new.trips, new.records, new.archives].contains(&top) {
                continue;
            }
            let dir = base.join(top);
            for sub in [old.trips, old.records] {
                dirs.push(dir.join(sub));
            }
            dirs.push(dir);
        }
        for dir in dirs {
            let _ = std::fs::remove_dir(&dir); // only an empty one goes
        }
    }
    tracing::info!(target: "bipbridge::notebook", from = old.lang, to = new.lang, pages = changed, "the Carnet's language changed");
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_in_the_new_language() {
        let (fr, en) = (words("fr"), words("en"));
        assert_eq!(moved_path(fr, en, "Personnes/Marc Dupont.md"), "People/Marc Dupont.md");
        assert_eq!(moved_path(fr, en, "Toi.md"), "You.md");
        assert_eq!(moved_path(fr, en, "À ranger.md"), "To sort.md");
        assert_eq!(moved_path(fr, en, "Tâches/Tâches.md"), "Tasks/Tasks.md");
        assert_eq!(moved_path(fr, en, "Domaines/Préférences.md"), "Areas/Preferences.md");
        assert_eq!(moved_path(fr, en, "Domaines/Maison.md"), "Areas/Maison.md");
        assert_eq!(moved_path(fr, en, "Voyages/Lisbonne — nov. 2025.md"), "Trips/Lisbonne — nov. 2025.md");
        assert_eq!(moved_path(fr, en, "Archives/Voyages/Rome.md"), "Archive/Trips/Rome.md");
        assert_eq!(moved_path(fr, en, "Fiches/Achats.md"), "Records/Purchases.md");
        assert_eq!(moved_path(fr, en, "Archives/Fiches/Achats.md"), "Archive/Records/Purchases.md");
        // a person named like a folder keeps their name
        assert_eq!(moved_path(fr, en, "Personnes/Toi.md"), "People/Toi.md");
        assert_eq!(moved_path(en, fr, "People/Zoé.md"), "Personnes/Zoé.md");
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn a_french_carnet_moved_to_english() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let lea = Who { by: "lea".into(), name: "Léa".into(), agent: Some("lea".into()) };
        let n = |kind: &str, title: &str, section: &str, fact: &str| {
            super::super::note(&config, Part::Main, kind, title, section, &[fact.to_string()], &[], "", "", &lea).unwrap()
        };
        n("self", "Toi", "goals", "Veut courir un marathon");
        n("person", "Marc", "work", "Pilote");
        super::super::add_task(&config, Part::Main, "Appeler Marc", "", "", "asked", "", &lea).unwrap();
        super::super::update_state(&config, |s| {
            s.insert("shares".into(), json!({"folders": {"Personnes": ["zoe"]}, "pages": {}}));
            s.insert("cursor".into(), json!("Personnes/Marc.md"));
        })
        .unwrap();
        let _guard = super::super::lock();
        let moved = change_locked(&config, "en", &Who::you(&config)).unwrap();
        assert!(moved >= 3, "{moved}");
        assert_eq!(super::super::language(&config), "en");
        let me = read_page_text(&config, Part::Main, "You.md").unwrap();
        assert!(me.contains("# You\n") && me.contains("  - Toi\n") && me.contains("## Goals\n"), "{me}");
        let marc = read_page_text(&config, Part::Main, "People/Marc.md").unwrap();
        assert!(marc.contains("## Work\n- Pilote — *Léa, "), "{marc}");
        assert!(read_page_text(&config, Part::Main, "Tasks/Tasks.md").unwrap().contains("Appeler Marc"));
        assert!(!part_root(&config, Part::Main).join("Personnes").exists());
        assert!(!part_root(&config, Part::Main).join("Toi.md").exists());
        let state = super::super::read_state(&config);
        assert_eq!(state["shares"]["folders"], json!({"People": ["zoe"]}));
        assert_eq!(state["cursor"], json!("People/Marc.md"));
        // the moves are housekeeping: not in « Nouveau », restorable from the page's history
        let recent = super::super::recent_changes(&config, Part::Main, 50);
        assert!(recent.iter().all(|e| e["action"] != json!(ACTION)));
        assert!(recent.iter().any(|e| e["path"] == json!("People/Marc.md")) || recent.is_empty());
        let log = super::super::history(&config, Some(Part::Main), Some("People/Marc.md"), 5);
        assert_eq!(log[0]["moved_from"], json!("Personnes/Marc.md"));
        // the agents' next writes go to the English pages
        drop(_guard);
        let again = n("person", "Marc", "tastes", "Aime le jazz");
        assert_eq!(again.path, "People/Marc.md");
        // a page in the way: nothing moved
        let _guard = super::super::lock();
        std::fs::create_dir_all(part_root(&config, Part::Main).join("Personnes")).unwrap();
        std::fs::write(part_root(&config, Part::Main).join("Personnes/Marc.md"), "# Marc\n").unwrap();
        assert!(change_locked(&config, "fr", &Who::you(&config)).is_err());
        assert_eq!(super::super::language(&config), "en");
        assert!(read_page_text(&config, Part::Main, "You.md").is_some());
    }
}
