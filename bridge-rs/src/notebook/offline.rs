//! The Carnet's direct answers, the same online and offline (Carnet v2, decision 13 of internal/notes/carnet-v2/
//! spec.md, contract.md §6): the bridge's search answers « Pointure de Léa ? » with « 36 » (tools::answers, the one
//! function the agents' `notebook_search` uses too), and the phone keeps a copy of the facts that can be said that way
//! (`GET /v1/notebook/offline`, `snapshot`) to answer the same question without a connection, by the same rule
//! (`answer`, the reference the apps follow: contracts/carnet/offline-parity.json).
//!
//! The copy never holds health (the private part, Domaines › Santé, any fact `lexicon::is_health` sees, wherever it is
//! written), the archives (their documents, the pages filed away), the tasks, nor numbers (an ID, a contract's or a
//! booking's reference: `numbered`). Its ETag is its content's hash: a write or an « Oublie ça » changes it.

use std::collections::BTreeMap;

use chrono::Datelike;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{fold, lexicon, Part};
use crate::config::Config;

/// The format's version.
pub const VERSION: i64 = 1;

struct Texts {
    as_of: &'static str,
    none: &'static str,
    noted: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { as_of: "Hors connexion · à jour du {date}", none: "Rien trouvé hors connexion.", noted: "noté le {date}" },
        "de" => Texts { as_of: "Offline · Stand {date}", none: "Offline nichts gefunden.", noted: "notiert am {date}" },
        "es" => Texts { as_of: "Sin conexión · al día del {date}", none: "Nada encontrado sin conexión.", noted: "anotado el {date}" },
        _ => Texts { as_of: "Offline · as of {date}", none: "Nothing found offline.", noted: "noted {date}" },
    }
}

/// « 3 oct. », « Oct 3 », « 3. Okt. », « 3 oct. ».
pub fn day(date: chrono::NaiveDate, lang: &str) -> String {
    let i = super::lang_index(lang).min(3);
    let month = super::trip_auto::MONTHS[i][date.month0() as usize];
    match i {
        1 => format!("{month} {}", date.day()),
        2 => format!("{}. {month}", date.day()),
        _ => format!("{} {month}", date.day()),
    }
}

/// « noté le 3 oct. » for a fact's date (YYYY-MM-DD), empty without one.
pub fn when(date: &str, lang: &str) -> String {
    match chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
        Ok(d) => texts(lang).noted.replace("{date}", &day(d, lang)),
        Err(_) => String::new(),
    }
}

/// A reference number in a fact (an ID, a contract's, a booking's): a word of 6 characters or more with 3 digits or
/// more. A phone number under its label stays (it is the person's contact, not a reference).
pub fn numbered(fact: &str) -> bool {
    if lexicon::id_number(fact) {
        return true;
    }
    if lexicon::split_label(fact).is_some_and(|(l, _)| lexicon::label_key(&l) == "phone") {
        return false;
    }
    fact.split(|c: char| !c.is_alphanumeric())
        .any(|w| w.chars().count() >= 6 && w.chars().filter(|c| c.is_ascii_digit()).count() >= 3)
}

/// The labels' folded key in `lang` (« pointure ») and every phrase that means it, in the four languages.
fn labels(lang: &str) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for l in lexicon::LABELS {
        let key = fold(lexicon::name_of(l, lang));
        let mut said: Vec<String> = l
            .words
            .iter()
            .chain(l.joins.iter())
            .map(|w| fold(w))
            .chain(l.names.iter().map(|n| fold(n)))
            .filter(|w| !w.is_empty() && *w != key)
            .collect();
        said.sort();
        said.dedup();
        out.insert(key, said);
    }
    out
}

/// A fact's label as its key: one of the table's by its name in `lang`, else the label folded.
fn label_word(label: &str, lang: &str) -> String {
    match lexicon::label_of(label) {
        Some(l) => fold(lexicon::name_of(l, lang)),
        None => fold(label),
    }
}

fn words_of(parts: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in parts {
        for w in fold(p).split(' ').filter(|w| !w.is_empty() && !lexicon::STOP.contains(w)) {
            if !out.iter().any(|x| x == w) {
                out.push(w.to_string());
            }
        }
    }
    out
}

/// Whether a page may be in the copy: the main part's pages of a kind, not archived, not health, never the papers
/// (papers.rs: what someone's ID is, even without its number, stays on the place).
fn shareable(config: &Config, path: &str, page: &super::Page) -> bool {
    let lang = super::language(config);
    let w = super::words(&lang);
    let kind = page.kind();
    let papers = super::records::kind("paper").is_some_and(|t| path == super::records::path_of(&lang, t));
    let kinds_ok = (super::KINDS.contains(&kind.as_str()) || kind == "records") && !papers;
    let health = fold(&page.title) == fold(w.health) || fold(&page.title) == "health" || lexicon::is_health(&page.title);
    kinds_ok && !health && !w.archived(path)
}

/// The facts a direct answer can give, in the copy's form, the main part only, health, archives and numbers left out.
pub fn facts(config: &Config, shown: &str) -> Vec<Value> {
    let lang = super::language(config);
    let brief = fold(super::words(&lang).brief);
    let mut out = Vec::new();
    for (path, page) in super::all_pages(config, Part::Main) {
        if !shareable(config, &path, &page) {
            continue;
        }
        let mut names = vec![page.title.clone()];
        names.extend(page.aliases());
        for (section, list) in page.sections() {
            if fold(&section) == brief || lexicon::is_health(&section) {
                continue;
            }
            for fact in list.iter().filter(|f| f.item) {
                let text = fact.text.trim();
                if text.is_empty() || lexicon::is_health(text) || numbered(text) || text.contains("[[archive:") {
                    continue;
                }
                let (label, answer) = lexicon::split_label(text).unwrap_or((String::new(), text.to_string()));
                let key = if label.is_empty() { String::new() } else { label_word(&label, &lang) };
                let mut parts: Vec<&str> = names.iter().map(String::as_str).collect();
                parts.push(&key);
                parts.push(&answer);
                out.push(json!({"page": page.title, "path": path, "label": label, "answer": answer,
                                "date": fact.date, "when": when(&fact.date, shown), "words": words_of(&parts)}));
            }
        }
    }
    out
}

/// The copy for the phone (contract.md §6), and its ETag.
pub fn snapshot(config: &Config, shown: &str, now: f64) -> (Value, String) {
    let lang = super::language(config);
    let t = texts(shown);
    let pages: Vec<Value> = super::all_pages(config, Part::Main)
        .into_iter()
        .filter(|(path, page)| shareable(config, path, page))
        .map(|(path, page)| {
            let aliases: Vec<String> = page.aliases().iter().map(|a| fold(a)).collect();
            json!({"title": page.title, "path": path, "kind": page.kind(), "aliases": aliases})
        })
        .collect();
    let today = chrono::DateTime::from_timestamp(now as i64, 0)
        .map(|d| d.with_timezone(&chrono::Local).date_naive())
        .unwrap_or_else(|| chrono::Local::now().date_naive());
    let mut body = json!({"v": VERSION, "lang": shown, "as_of": t.as_of.replace("{date}", &day(today, shown)),
                          "none": t.none, "stop": lexicon::STOP, "labels": labels(&lang),
                          "facts": facts(config, shown), "pages": pages});
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(crate::py::dumps(&body).as_bytes()))[..24]);
    body["generated_at"] = json!(now.round());
    (body, etag)
}

/// The question's words as the copy's rule reads them: folded, a label's phrase replaced by its key (the longest
/// first), the stop words dropped.
pub fn question_words(snapshot: &Value, question: &str) -> Vec<String> {
    let mut folded = format!(" {} ", fold(question));
    let mut phrases: Vec<(String, String)> = Vec::new();
    if let Some(labels) = snapshot.get("labels").and_then(Value::as_object) {
        for (key, said) in labels {
            for s in said.as_array().into_iter().flatten().filter_map(Value::as_str) {
                phrases.push((s.to_string(), key.clone()));
            }
        }
    }
    phrases.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    for (said, key) in phrases {
        let padded = format!(" {said} ");
        while folded.contains(&padded) {
            folded = folded.replacen(&padded, &format!(" {key} "), 1);
        }
    }
    let stop: Vec<&str> = snapshot.get("stop").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let mut out: Vec<String> = Vec::new();
    for w in folded.split(' ').filter(|w| !w.is_empty() && !stop.contains(w)) {
        if !out.iter().any(|x| x == w) {
            out.push(w.to_string());
        }
    }
    out
}

/// The copy's answer to `question` (the reference the apps follow): the facts whose words hold every word of the
/// question, the latest first; a question with no label word answers nothing (it names a page).
pub fn answer(snapshot: &Value, question: &str) -> Option<Value> {
    let wanted = question_words(snapshot, question);
    if wanted.is_empty() {
        return None;
    }
    let mut found: Vec<&Value> = snapshot
        .get("facts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|f| {
            let words: Vec<&str> = f.get("words").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
            wanted.iter().all(|w| words.contains(&w.as_str()))
        })
        .collect();
    // names alone find a page, not an answer: a word of the fact itself must be asked
    found.retain(|f| {
        let page: Vec<String> = fold(f.get("page").and_then(Value::as_str).unwrap_or("")).split(' ').map(str::to_string).collect();
        wanted.iter().any(|w| !page.contains(w))
    });
    found.sort_by_key(|f| std::cmp::Reverse(f.get("date").and_then(Value::as_str).unwrap_or("").to_string()));
    found.first().map(|f| (*f).clone())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::notebook::Who;

    /// A Carnet of three pages, dated 2026-10-03, with an allergy written straight on a person's page (as an old
    /// write could have) and a contract's number.
    pub(crate) fn carnet() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = crate::notebook::tests_config(dir.path());
        let agent = crate::config::AgentConfig { name: "quotidien".into(), display_name: "Quotidien".into(), ..Default::default() };
        config.agents.insert("quotidien".into(), agent);
        crate::notebook::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let who = Who::agent(&config, "quotidien");
        let note = |kind: &str, title: &str, facts: &[&str], aliases: &[&str]| {
            let facts: Vec<String> = facts.iter().map(|f| f.to_string()).collect();
            let aliases: Vec<String> = aliases.iter().map(|f| f.to_string()).collect();
            crate::notebook::note(&config, Part::Main, kind, title, "", &facts, &aliases, "", "", &who).unwrap();
        };
        note("person", "Léa Martin", &["Pointure : 36", "Anniversaire : 12 mars", "Taille : 1,62 m"], &["Léa"]);
        note("person", "Marc Dupont", &["Pointure : 43", "Anniversaire : 3 juin", "Métier : boulanger"], &[]);
        note("self", "Toi", &["Ville : Lyon"], &[]);
        note("subject", "Voiture", &["Contrat : AX123456"], &[]);
        let today = crate::notebook::today();
        for (path, _) in crate::notebook::all_pages(&config, Part::Main) {
            let file = crate::notebook::part_root(&config, Part::Main).join(&path);
            let mut text = std::fs::read_to_string(&file).unwrap().replace(&today, "2026-10-03");
            if path.contains("Léa") {
                text = text.replace("- Pointure : 36", "- Allergies : arachides — *Quotidien, 2026-10-03*\n- Pointure : 36");
            }
            std::fs::write(&file, text).unwrap();
        }
        (dir, config)
    }

    fn parity() -> Value {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contracts/carnet/offline-parity.json");
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }

    /// The snapshot as the parity file keeps it: its date-bound fields out.
    fn stable(mut snapshot: Value) -> Value {
        for k in ["generated_at", "as_of"] {
            snapshot.as_object_mut().unwrap().remove(k);
        }
        snapshot
    }

    #[test]
    fn the_copy_leaves_out_health_numbers_and_archives_and_its_etag_follows_the_content() {
        let (_dir, config) = carnet();
        let (body, etag) = snapshot(&config, "fr", 1_762_690_000.0);
        let text = body.to_string();
        assert!(!text.contains("arachides"), "an allergy on a person's page stays out");
        assert!(!text.contains("AX123456"), "a contract's number stays out");
        assert!(text.contains("\"36\"") && text.contains("Lyon"));
        assert!(body["as_of"].as_str().unwrap().starts_with("Hors connexion · à jour du "));
        assert_eq!(body["none"], "Rien trouvé hors connexion.");
        let lea = body["facts"].as_array().unwrap().iter().find(|f| f["answer"] == "36").unwrap();
        assert_eq!(lea["words"], json!(["lea", "martin", "pointure", "36"]));
        assert_eq!(lea["when"], "noté le 3 oct.");
        assert!(body["labels"]["pointure"].as_array().unwrap().contains(&json!("chausse du")));
        // the same content, the same ETag; a fact more, another
        let (_, again) = snapshot(&config, "fr", 1_762_700_000.0);
        assert_eq!(etag, again);
        let who = Who::you(&config);
        crate::notebook::note(&config, Part::Main, "person", "Marc Dupont", "", &["Couleur préférée : vert".into()], &[], "", "", &who).unwrap();
        assert_ne!(snapshot(&config, "fr", 1_762_700_000.0).1, etag);
        // an archived page is not in the copy
        assert!(numbered("Réservation : ABC123456") && !numbered("Téléphone : +33612345678") && !numbered("Code postal : 75011"));
    }

    #[test]
    fn the_parity_file_is_the_bridges_rule() {
        let (_dir, config) = carnet();
        let (body, _) = snapshot(&config, "fr", 1_762_690_000.0);
        let body = stable(body);
        if std::env::var("BIPAGENTS_WRITE_PARITY").is_ok() {
            println!("PARITY-SNAPSHOT {}", crate::py::Dumps::new().unicode().dumps(&body));
        }
        let file = parity();
        assert_eq!(file["snapshot"], body, "contracts/carnet/offline-parity.json: the snapshot changed (BIPAGENTS_WRITE_PARITY=1)");
        for pair in file["fold"].as_array().unwrap() {
            assert_eq!(fold(pair[0].as_str().unwrap()), pair[1].as_str().unwrap());
        }
        for case in file["cases"].as_array().unwrap() {
            let q = case["q"].as_str().unwrap();
            let offline = answer(&body, q).map(|f| f["answer"].clone()).unwrap_or(Value::Null);
            assert_eq!(offline, case["answer"], "offline: {q}");
            let online = crate::notebook::tools::answers(&config, &[Part::Main], q).first().map(|a| json!(a.answer)).unwrap_or(Value::Null);
            let want = case.get("online").cloned().unwrap_or_else(|| case["answer"].clone());
            assert_eq!(online, want, "online: {q}");
        }
        assert_eq!(file["cases"].as_array().unwrap().len(), 40);
    }
}
