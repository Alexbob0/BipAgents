//! People who don't need a page of their own yet (carnet review, 2026-11-03; Alex: « une règle par quantité
//! d'info ? »). Close family (partner, children, parents, brothers and sisters, grandparents) always have their own
//! page; someone else (a cousin, a colleague, a godfather, a neighbour) is a line on their group's page — an area
//! page « Famille élargie », « Amis », « Collègues » or « Connaissances » — written « Name : fact », until there are
//! `PROMOTE_FACTS` facts about them: then their own page. Measured on the review's lab scenarios (GLM-5.3 Flash and
//! DeepSeek V4.1 Flash): someone mentioned once yields two facts (who they are, one detail), someone who comes back
//! three or more; a family chat's peripheral people (a godfather, a neighbour) one each.
//!
//! - The agents follow it from notebook_note's description; the WhatsApp import (whatsapp_import/fill.rs) applies it
//!   itself through `file_person`.
//! - At each nightly pass (`settle_locked`, free): a group's lines about someone who has their own page move there
//!   (a fact is never on two pages), and those about someone with `PROMOTE_FACTS` lines or more become their page. A
//!   group page left empty goes with the empty pages (archive.rs).

use std::collections::BTreeMap;

use super::{Page, Part, Who, fold, template};
use crate::config::Config;

/// Facts about someone that earn them their own page.
pub const PROMOTE_FACTS: usize = 3;

/// The groups: the WhatsApp import's relation, and the page's title in fr, en, de, es.
const GROUPS: [(&str, [&str; 4]); 4] = [
    ("family", ["Famille élargie", "Extended family", "Weitere Familie", "Familia extendida"]),
    ("friend", ["Amis", "Friends", "Freunde", "Amigos"]),
    ("work", ["Collègues", "Colleagues", "Kollegen", "Colegas"]),
    ("other", ["Connaissances", "Acquaintances", "Bekannte", "Conocidos"]),
];

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

/// A relation's group page title in the Carnet's language (« family » → « Famille élargie »).
pub fn group_title(relation: &str, lang: &str) -> &'static str {
    let group = GROUPS.iter().find(|g| g.0 == relation).unwrap_or(&GROUPS[3]);
    group.1[lang_index(lang)]
}

/// Whether a page is one of the groups (its title in any language).
pub fn is_group(title: &str) -> bool {
    GROUPS.iter().any(|g| g.1.iter().any(|t| fold(t) == fold(title)))
}

/// Words saying someone is close family (folded), and those saying they are not (they win: « cousin, fils de Martine »).
const CLOSE: &[&str] = &[
    "mere",
    "maman",
    "pere",
    "papa",
    "soeur",
    "frere",
    "mari",
    "femme",
    "epouse",
    "epoux",
    "conjoint",
    "conjointe",
    "compagnon",
    "compagne",
    "grand mere",
    "grand pere",
    "fils",
    "fille",
    "enfant",
    "mother",
    "mom",
    "mum",
    "father",
    "dad",
    "sister",
    "brother",
    "husband",
    "wife",
    "son",
    "daughter",
    "grandmother",
    "grandfather",
    "mutter",
    "vater",
    "schwester",
    "bruder",
    "sohn",
    "tochter",
    "madre",
    "padre",
    "hermana",
    "hermano",
    "hijo",
    "hija",
];
const NOT_CLOSE: &[&str] = &[
    "cousin",
    "cousine",
    "oncle",
    "tante",
    "neveu",
    "niece",
    "beau",
    "belle",
    "parrain",
    "marraine",
    "voisin",
    "voisine",
    "collegue",
    "ami",
    "amie",
    "uncle",
    "aunt",
    "nephew",
    "godfather",
    "godmother",
    "neighbour",
    "neighbor",
    "colleague",
    "friend",
    "tio",
    "tia",
];

/// Whether facts about someone say they are close family (« Ma sœur », « Mère, vit à Lyon »; not « Femme de
/// [[Julien]] »: someone else's).
pub fn is_close(facts: &[String]) -> bool {
    static OF_SOMEONE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(\w+)\s+(?:de|d'|d’|of|von|del)\s*\[*\p{Lu}").expect("valid regex")
    });
    let text = facts.join(" · ");
    let others: Vec<String> = OF_SOMEONE.captures_iter(&text).map(|c| fold(&c[1])).collect();
    let plain = format!(" {} ", fold(&text).replace(|c: char| !c.is_alphanumeric(), " "));
    let has = |w: &str| plain.contains(&format!(" {w} "));
    CLOSE.iter().any(|w| has(w) && !others.iter().any(|o| o == w)) && !NOT_CLOSE.iter().any(|w| has(w))
}

/// The name a group line is about (« Emma, cousine… : … », « Paul Lambert : parrain d'Emma »), and the fact without
/// it. None: no name in front.
pub fn line_name(text: &str) -> Option<(String, String)> {
    let cut = text.find([',', ':'])?;
    let name = text[..cut].trim().trim_start_matches("[[").trim_end_matches("]]").trim();
    let words: Vec<&str> = name.split_whitespace().collect();
    if words.is_empty() || words.len() > 4 || !words.iter().all(|w| w.chars().next().is_some_and(char::is_uppercase)) {
        return None;
    }
    let rest = text[cut + 1..].trim().trim_start_matches(':').trim().to_string();
    (!rest.is_empty()).then(|| (name.to_string(), rest))
}

/// [`file_person_locked`], the lock taken.
pub fn file_person(
    config: &Config,
    name: &str,
    relation: &str,
    facts: &[String],
    who: &Who,
) -> anyhow::Result<super::Written> {
    let _guard = super::lock();
    file_person_locked(config, name, relation, facts, who)
}

/// Facts about someone filed by the rule (the caller holds the lock): on their page when they have one, are close
/// family or come with `PROMOTE_FACTS` facts; else as lines « Name : fact » on their group's page. Returns where
/// they went (the page's title).
pub fn file_person_locked(
    config: &Config,
    name: &str,
    relation: &str,
    facts: &[String],
    who: &Who,
) -> anyhow::Result<super::Written> {
    let own = super::find(config, &[Part::Main], name).is_some_and(|f| f.page.kind() == "person");
    if own || is_close(facts) || facts.len() >= PROMOTE_FACTS {
        return super::note_locked(config, Part::Main, "person", name, "", facts, &[], "", "", who);
    }
    let lang = super::language(config);
    let lines: Vec<String> = facts.iter().map(|f| format!("{name} : {f}")).collect();
    let section = template::title("area", "contacts", &lang);
    super::note_locked(config, Part::Main, "area", group_title(relation, &lang), &section, &lines, &[], "", "", who)
}

/// The nightly pass's part (see the module's text). Returns how many people's lines moved.
pub fn settle_locked(config: &Config, who: &Who) -> anyhow::Result<usize> {
    let mut moved = 0;
    for (path, page) in super::all_pages(config, Part::Main) {
        if page.kind() != "area" || !is_group(&page.title) {
            continue;
        }
        // the group's lines by the person they are about
        let mut by_name: BTreeMap<String, (String, Vec<(usize, String)>)> = BTreeMap::new();
        for (i, line) in page.body.lines().enumerate() {
            let Some(item) = line.trim().strip_prefix("- ") else { continue };
            let (text, _) = super::split_attribution(item);
            if let Some((name, fact)) = line_name(&text) {
                by_name.entry(fold(&name)).or_insert_with(|| (name.clone(), Vec::new())).1.push((i, fact));
            }
        }
        let mut gone: Vec<usize> = Vec::new();
        for (name, lines) in by_name.values() {
            let own = super::find(config, &[Part::Main], name).is_some_and(|f| f.page.kind() == "person");
            if !own && lines.len() < PROMOTE_FACTS {
                continue;
            }
            let facts: Vec<String> = lines.iter().map(|(_, f)| f.clone()).collect();
            super::note_locked(config, Part::Main, "person", name, "", &facts, &[], "", "", who)?;
            gone.extend(lines.iter().map(|(i, _)| *i));
            moved += 1;
        }
        if gone.is_empty() {
            continue;
        }
        let Some(before) = super::read_page_text(config, Part::Main, &path) else { continue };
        let mut rest: Page = page.clone();
        let lang = super::language(config);
        let kept: Vec<&str> =
            page.body.lines().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, l)| l).collect();
        rest.body = template::normalize("area", &lang, super::words(&lang).brief, &kept.join("\n")).body;
        rest.touch();
        super::commit(config, Part::Main, &path, Some(&before), Some(&rest.render()), who, "tidy", &page.title)?;
    }
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn close_family_and_names_in_front() {
        assert!(is_close(&["Ma sœur".into()]));
        assert!(is_close(&["Mère, vit à Lyon".into()]));
        assert!(!is_close(&["Cousin, fils de Martine".into()]));
        assert!(!is_close(&["Parrain d'Emma".into()]));
        assert!(!is_close(&["Femme de [[Julien]], mariés cet été".into()]));
        assert_eq!(
            line_name("Emma, cousine, sœur de [[Julien]] : infirmière à Lille").map(|n| n.0),
            Some("Emma".into())
        );
        assert_eq!(line_name("Paul Lambert : parrain d'Emma"), Some(("Paul Lambert".into(), "parrain d'Emma".into())));
        assert_eq!(line_name("Compagne de [[Julien]], mariés cet été"), None);
        assert_eq!(group_title("work", "fr"), "Collègues");
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn a_line_until_three_facts_then_a_page_and_never_twice() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let who = Who::agent(&config, "lea");
        let _guard = super::super::lock();
        let paul = file_person_locked(&config, "Paul Lambert", "friend", &["Parrain d'Emma".into()], &who).unwrap();
        assert_eq!(paul.title, "Amis");
        let mom = file_person_locked(&config, "Françoise", "family", &["Mère, vit à Lyon".into()], &who).unwrap();
        assert_eq!(mom.title, "Françoise", "close family: their own page");
        for fact in ["Emma, cousine : infirmière à Lille", "Emma : de garde ce week-end", "Emma : aide au déménagement"]
        {
            super::super::note_locked(
                &config,
                Part::Main,
                "area",
                "Famille élargie",
                "contacts",
                &[fact.into()],
                &[],
                "",
                "",
                &who,
            )
            .unwrap();
        }
        assert_eq!(settle_locked(&config, &Who::tidy(&config)).unwrap(), 1, "Emma has three lines: her page");
        let emma = super::super::find(&config, &[Part::Main], "Emma").unwrap();
        assert_eq!(emma.page.kind(), "person");
        assert!(emma.page.body.contains("infirmière à Lille"), "{}", emma.page.body);
        let group =
            super::super::read_page_text(&config, Part::Main, "Domaines/Famille élargie.md").unwrap_or_default();
        assert!(!group.contains("Emma"), "never on two pages: {group}");
        // a line about someone who has a page moves there
        super::super::note_locked(
            &config,
            Part::Main,
            "area",
            "Amis",
            "contacts",
            &["Françoise : aime le jardinage".into()],
            &[],
            "",
            "",
            &who,
        )
        .unwrap();
        assert_eq!(settle_locked(&config, &Who::tidy(&config)).unwrap(), 1);
        let text = super::super::read_page_text(&config, Part::Main, &mom.path).unwrap();
        assert!(text.contains("aime le jardinage"), "{text}");
    }
}
