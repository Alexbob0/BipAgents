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

/// Whether the person's own page names them among their close family (« Mère : Françoise, vit à Lyon » in « Toi »'s
/// family section): a group chat says who is who to the others, not to the person (the lab's family chat, 2026-11-03:
/// « Maman d'Hugo et Inès » for the person's sister).
fn named_on_self(config: &Config, name: &str) -> bool {
    let Some(me) = super::find(config, &[Part::Main], &super::words(&super::language(config)).self_path()) else {
        return false;
    };
    let first = fold(name.split_whitespace().next().unwrap_or(""));
    if first.chars().count() < 3 {
        return false;
    }
    me.page.sections().into_iter().filter(|(title, _)| template::resolve("self", title) == Some("family")).any(
        |(_, facts)| {
            facts.iter().any(|f| {
                let words = format!(" {} ", fold(&f.text).replace(|c: char| !c.is_alphanumeric(), " "));
                words.contains(&format!(" {first} ")) && is_close(std::slice::from_ref(&f.text))
            })
        },
    )
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
    if own || is_close(facts) || named_on_self(config, name) || facts.len() >= PROMOTE_FACTS {
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
        let mut leaving: Vec<(String, Vec<String>)> = Vec::new();
        for (name, lines) in by_name.values() {
            let own = super::find(config, &[Part::Main], name).is_some_and(|f| f.page.kind() == "person");
            if !own && lines.len() < PROMOTE_FACTS {
                continue;
            }
            leaving.push((name.clone(), lines.iter().map(|(_, f)| f.clone()).collect()));
            gone.extend(lines.iter().map(|(i, _)| *i));
        }
        if gone.is_empty() {
            continue;
        }
        // the group first, without their lines nor their names (a folded page's names: its links resolve here until
        // the person has a page again)
        let Some(before) = super::read_page_text(config, Part::Main, &path) else { continue };
        let mut rest: Page = page.clone();
        let lang = super::language(config);
        let kept: Vec<&str> =
            page.body.lines().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, l)| l).collect();
        rest.body = template::normalize("area", &lang, super::words(&lang).brief, &kept.join("\n")).body;
        let names: Vec<String> = rest
            .aliases()
            .into_iter()
            .filter(|a| {
                !leaving.iter().any(|(n, _)| fold(n) == fold(a) || fold(a).starts_with(&format!("{} ", fold(n))))
            })
            .collect();
        rest.set_aliases(&names);
        rest.touch();
        super::commit(config, Part::Main, &path, Some(&before), Some(&rest.render()), who, "tidy", &page.title)?;
        for (name, facts) in &leaving {
            super::note_locked(config, Part::Main, "person", name, "", facts, &[], "", "", who)?;
            moved += 1;
        }
    }
    Ok(moved)
}

/// A light page's group, from the words of its facts (« Cousine, sœur de Julien » → family).
fn relation_of(text: &str) -> &'static str {
    let plain = format!(" {} ", fold(text).replace(|c: char| !c.is_alphanumeric(), " "));
    let has = |words: &[&str]| words.iter().any(|w| plain.contains(&format!(" {w} ")));
    if has(&[
        "cousin", "cousine", "oncle", "tante", "neveu", "niece", "beau", "belle", "uncle", "aunt", "nephew", "onkel",
        "cousine", "primo", "prima", "tio", "tia", "sobrino",
    ]) {
        "family"
    } else if has(&[
        "collegue",
        "colleague",
        "chef",
        "manager",
        "boss",
        "kollege",
        "kollegin",
        "colega",
        "travail",
        "bureau",
        "work",
    ]) {
        "work"
    } else if has(&["ami", "amie", "copain", "copine", "friend", "freund", "freundin", "amigo", "amiga"]) {
        "friend"
    } else {
        "other"
    }
}

/// The light pages folded into their group (Alex, 2026-11-03: yes): someone with fewer than `PROMOTE_FACTS` facts who
/// is not close family (not marked close, not named so on « Toi », no close word) becomes « Name : fact » lines on
/// their group's page, each with its attribution, the page's « En bref » a line too; the page goes, its names become
/// the group's aliases (its links and its tasks still find it). Left as they are: a page the person wrote last, one
/// linked to a phone contact, one shared with the family, one with nothing in it. Every change in the history.
/// Returns how many pages were folded.
pub fn fold_light_locked(config: &Config, who: &Who) -> anyhow::Result<usize> {
    let lang = super::language(config);
    let brief = fold(super::words(&lang).brief);
    let mut folded = 0;
    for (path, page) in super::all_pages(config, Part::Main) {
        if page.kind() != "person" || page.close() || !page.contact().is_empty() {
            continue;
        }
        let facts: Vec<super::Fact> =
            page.sections().into_iter().filter(|(name, _)| fold(name) != brief).flat_map(|(_, f)| f).collect();
        let summary = super::summary_of(&page.body, super::words(&lang).brief);
        let texts: Vec<String> = facts.iter().map(|f| f.text.clone()).chain(std::iter::once(summary.clone())).collect();
        if facts.is_empty()
            || facts.len() >= PROMOTE_FACTS
            || is_close(&texts)
            || named_on_self(config, &page.title)
            || !super::shares_of(config, Part::Main, &path, &page).is_empty()
        {
            continue;
        }
        // written by the person last, or by hand outside the Carnet's history (Obsidian): theirs, left as it is
        let by_person = super::history(config, Some(Part::Main), Some(&path), 1)
            .first()
            .is_none_or(|e| crate::tools_knowledge::str_or_empty(e.get("by")) == "you");
        if by_person {
            continue;
        }
        // its lines, as written (their attribution kept), « En bref » too when it says more
        let name = page.title.clone();
        let mut lines: Vec<String> = facts
            .iter()
            .map(|f| {
                let by = if f.who.is_empty() { String::new() } else { format!(" — *{}, {}*", f.who, f.date) };
                format!("- {name} : {}{by}", f.text)
            })
            .collect();
        if !summary.trim().is_empty() && !facts.iter().any(|f| fold(&f.text) == fold(&summary)) {
            lines.push(format!("- {name} : {}", summary.replace('\n', " ")));
        }
        let group = group_title(relation_of(&texts.join(" ")), &lang);
        let section = template::title("area", "contacts", &lang);
        let (gpath, gbefore, mut gpage) =
            match super::find(config, &[Part::Main], group).filter(|f| f.page.kind() == "area") {
                Some(f) => (f.path.clone(), super::read_page_text(config, Part::Main, &f.path), f.page),
                None => {
                    let folder = super::words(&lang).folder("area").unwrap_or("Domaines").to_string();
                    (format!("{folder}/{}.md", super::file_name(group)), None, Page::new("area", group))
                }
            };
        gpage.ensure_id();
        let mut body = body_with(&gpage.body, &section, &lines);
        body = template::normalize("area", &lang, super::words(&lang).brief, &body).body;
        gpage.body = body;
        let mut names = gpage.aliases();
        for n in std::iter::once(name.clone()).chain(page.aliases()) {
            if !names.iter().any(|a| fold(a) == fold(&n)) {
                names.push(n);
            }
        }
        gpage.set_aliases(&names);
        gpage.touch();
        let summary_line = format!("{name} → {group}");
        super::commit(
            config,
            Part::Main,
            &gpath,
            gbefore.as_deref(),
            Some(&gpage.render()),
            who,
            "tidy",
            &summary_line,
        )?;
        let Some(before) = super::read_page_text(config, Part::Main, &path) else { continue };
        super::commit(config, Part::Main, &path, Some(&before), None, who, "tidy", &summary_line)?;
        folded += 1;
    }
    Ok(folded)
}

/// `body` with `lines` (list lines, as written) at the end of `section`, made if missing; those already there (same
/// text without attribution) left out.
fn body_with(body: &str, section: &str, lines: &[String]) -> String {
    let mut all = super::body_lines(body);
    let known: Vec<String> =
        all.iter().map(|l| fold(&super::split_attribution(l.trim().trim_start_matches("- ")).0)).collect();
    let new: Vec<String> = lines
        .iter()
        .filter(|l| !known.contains(&fold(&super::split_attribution(l.trim().trim_start_matches("- ")).0)))
        .cloned()
        .collect();
    match super::section_index(&all, section) {
        Some(start) => {
            let mut at = super::section_end(&all, start);
            while at > start + 1 && all[at - 1].trim().is_empty() {
                at -= 1;
            }
            for (n, line) in new.into_iter().enumerate() {
                all.insert(at + n, line);
            }
        }
        None => {
            if !all.is_empty() {
                all.push(String::new());
            }
            all.push(format!("## {section}"));
            all.extend(new);
        }
    }
    all.join("\n") + "\n"
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
    fn light_pages_fold_into_their_group_and_come_back_at_three_facts() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let agent = Who::agent(&config, "lea");
        let _guard = super::super::lock();
        let note = |kind: &str, name: &str, section: &str, facts: &[&str], who: &Who| {
            let facts: Vec<String> = facts.iter().map(|f| f.to_string()).collect();
            super::super::note_locked(&config, Part::Main, kind, name, section, &facts, &[], "", "", who).unwrap()
        };
        let paul = note("person", "Paul Lambert", "who", &["Parrain d'Emma"], &agent);
        let martine =
            note("person", "Martine", "who", &["Tante, mère de [[Julien]]", "Fête ses 70 ans le 12 avril"], &agent);
        let mom = note("person", "Françoise", "who", &["Ma mère"], &agent);
        let mine = note("person", "Thomas", "who", &["Collègue"], &Who::you(&config));
        let julien = note("person", "Julien", "who", &["Cousin", "Aime le vélo", "Cherche un appartement"], &agent);
        assert_eq!(fold_light_locked(&config, &Who::tidy(&config)).unwrap(), 2, "Paul and Martine");
        for (path, stays) in
            [(&paul.path, false), (&martine.path, false), (&mom.path, true), (&mine.path, true), (&julien.path, true)]
        {
            assert_eq!(super::super::read_page_text(&config, Part::Main, path).is_some(), stays, "{path}");
        }
        let family = super::super::find(&config, &[Part::Main], "Famille élargie").unwrap();
        assert!(family.page.body.contains("- Martine : Fête ses 70 ans le 12 avril — *lea, "), "{}", family.page.body);
        let known = super::super::find(&config, &[Part::Main], "Paul Lambert").expect("the old name still resolves");
        assert_eq!(known.page.title, "Connaissances");
        // facts about him now go there, and at three he has his page again (his name leaves the group)
        note("person", "Paul Lambert", "", &["Vit à Angers"], &agent);
        note("person", "Paul Lambert", "", &["Fait du jardinage"], &agent);
        let group = super::super::find(&config, &[Part::Main], "Connaissances").unwrap();
        assert!(group.page.body.contains("Paul Lambert : Vit à Angers"), "{}", group.page.body);
        assert_eq!(settle_locked(&config, &Who::tidy(&config)).unwrap(), 1);
        let back = super::super::find(&config, &[Part::Main], "Paul Lambert").unwrap();
        assert_eq!(back.page.kind(), "person");
        assert!(
            back.page.body.contains("Parrain d'Emma") && back.page.body.contains("Fait du jardinage"),
            "{}",
            back.page.body
        );
        assert_eq!(fold_light_locked(&config, &Who::tidy(&config)).unwrap(), 0, "stable");
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
        // close family named on « Toi » (a group chat says « Maman d'Hugo » for the person's sister)
        super::super::note_locked(
            &config,
            Part::Main,
            "self",
            "Toi",
            "family",
            &["Sœur : Claire, à Nantes".into()],
            &[],
            "",
            "",
            &who,
        )
        .unwrap();
        let claire =
            file_person_locked(&config, "Claire Fournier", "family", &["Maman d'Hugo et Inès".into()], &who).unwrap();
        assert_eq!(claire.title, "Claire Fournier");
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
