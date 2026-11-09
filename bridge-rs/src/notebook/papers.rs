//! A paper photographed (Carnet v2, decision 16 of internal/notes/carnet-v2/spec.md, contract.md §3.7): the phone reads
//! the text (iOS Vision, Android ML Kit), sends it with the photo, and the bridge files it without a model — the image
//! never goes to the model on this path, and no number is ever kept:
//!
//! - the text is cleaned first (`scrub`): the MRZ band and any run of 6 or more digits or letters-and-digits are taken
//!   out before anything is stored, journal included (the names and the end date are read from the MRZ before);
//! - the kind of paper (passport, ID card, driving licence, residence permit, Carte Vitale, insurance), its holder
//!   (matched to a person page as a write is, mod.rs `candidates`) and its end date are found by rules;
//! - one `paper` record {document, holder, expires} (the same document and holder: the same record), the photo kept in
//!   `notebook/papers/<record id>.jpg` (0600, apart from the health part, never in the offline copy, shown by the app
//!   after Face ID), and two real tasks (the person wanted them by taking the photo: origin `asked`), 3 months and
//!   1 month before the end, linked to the phone's reminders when it shares them.
//!
//! A paper an agent records from a photo sent in a conversation (`notebook_record type=paper`) goes through the same
//! cleaning and gets the same two tasks (`remind`). « Oublie ça » of the record takes the photo too (`sweep`).
//!
//!     POST /v1/notebook/paper              {text, image? (base64 JPEG or PNG), lang?}   → {record, card, tasks, photo}
//!     GET  /v1/notebook/papers/{id}/photo  the photo
//!     notebook/state.json                  papers {record id: [task ids]}

use std::path::PathBuf;
use std::sync::LazyLock;

use chrono::{Datelike, Months, NaiveDate};
use regex::Regex;
use serde_json::{json, Value};

use super::{fold, records, Part, Who};
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

/// A photo kept at most this big.
pub const PHOTO_MAX: usize = 12 << 20;
/// Characters of text read at most.
pub const TEXT_MAX: usize = 8000;

/// The kinds of paper recognised: id, names (fr, en, de, es), words that say it (folded).
const KINDS: &[(&str, [&str; 4], &[&str])] = &[
    ("passport", ["Passeport", "Passport", "Reisepass", "Pasaporte"], &["passeport", "passport", "reisepass", "pasaporte"]),
    ("residence", ["Titre de séjour", "Residence permit", "Aufenthaltstitel", "Permiso de residencia"],
     &["titre de sejour", "residence permit", "aufenthaltstitel", "aufenthaltserlaubnis", "permiso de residencia", "tarjeta de residencia"]),
    ("id_card", ["Carte d'identité", "ID card", "Personalausweis", "DNI"],
     &["carte nationale d identite", "carte d identite", "identity card", "personalausweis", "documento nacional de identidad"]),
    ("licence", ["Permis de conduire", "Driving licence", "Führerschein", "Permiso de conducir"],
     &["permis de conduire", "driving licence", "driver s license", "driver license", "fuhrerschein", "permiso de conduccion", "carnet de conducir"]),
    ("vitale", ["Carte Vitale", "Carte Vitale", "Carte Vitale", "Carte Vitale"], &["carte vitale"]),
    ("insurance", ["Assurance", "Insurance", "Versicherung", "Seguro"], &["assurance", "insurance", "versicherung", "seguro", "poliza"]),
];

/// The words before an end date (folded).
const ENDS: &[&str] = &[
    "date d expiration", "expiration", "expire", "valable jusqu", "fin de validite", "date of expiry", "expiry", "expires",
    "valid until", "gultig bis", "ablaufdatum", "fecha de caducidad", "caducidad", "valido hasta",
];
/// The words before a surname, given names (folded).
const SURNAMES: &[&str] = &["nom", "surname", "name", "nachname", "apellidos", "apellido"];
const GIVEN: &[&str] = &["prenoms", "prenom", "given names", "given name", "vornamen", "vorname", "nombres", "nombre"];

/// Month names (folded, the first letters), in the four languages.
const MONTHS: &[(&str, u32)] = &[
    ("janv", 1), ("jan", 1), ("ene", 1), ("fevr", 2), ("fev", 2), ("feb", 2), ("mars", 3), ("mar", 3), ("marz", 3),
    ("avr", 4), ("apr", 4), ("abr", 4), ("mai", 5), ("may", 5), ("juin", 6), ("jun", 6), ("juil", 7), ("jul", 7),
    ("aout", 8), ("aug", 8), ("ago", 8), ("sept", 9), ("sep", 9), ("oct", 10), ("okt", 10), ("nov", 11), ("dec", 12),
    ("dez", 12), ("dic", 12),
];

static MRZ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z0-9<]{25,}$").expect("valid regex"));
static RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9]{6,}").expect("valid regex"));
static NUMERIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,2})[./\-](\d{1,2})[./\-](\d{4})\b|\b(\d{4})-(\d{2})-(\d{2})\b").expect("valid regex"));
static WORDED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{1,2})\s+([a-z]{3,5})\.?(?:\s*/?\s*[a-z]{3,5}\.?)?\s+(\d{4})\b").expect("valid regex"));

/// The text read, cleaned: no MRZ line, no run of 6 or more digits or letters-and-digits (a document's number).
pub fn scrub(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !is_mrz(l))
        .map(|l| RUN.replace_all(l, |c: &regex::Captures| if c[0].chars().any(|c| c.is_ascii_digit()) { "•••".to_string() } else { c[0].to_string() }).to_string())
        .collect();
    tk::head(&lines.join("\n"), TEXT_MAX).to_string()
}

fn is_mrz(line: &str) -> bool {
    let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.contains("<<") || MRZ.is_match(&squeezed)
}

/// The MRZ's names (surname, given names) and end date, read before it is taken out.
fn from_mrz(text: &str) -> (Option<(String, String)>, Option<NaiveDate>) {
    let lines: Vec<String> = text.lines().map(|l| l.chars().filter(|c| !c.is_whitespace()).collect::<String>()).filter(|l| is_mrz(l)).collect();
    let mut names = None;
    let mut end = None;
    for (i, line) in lines.iter().enumerate() {
        let letters_only = line.chars().all(|c| c.is_ascii_uppercase() || c == '<');
        // a passport's first line « P<FRAMARTIN<<LEA<<<< », an ID card's last line « MARTIN<<LEA<<<< »
        let body = if line.starts_with("P<") || line.starts_with("PO") { line.get(5..) } else if letters_only { Some(line.as_str()) } else { None };
        if let Some(body) = body.filter(|b| b.contains("<<") && names.is_none()) {
            let (surname, given) = body.split_once("<<").unwrap_or((body, ""));
            let given = given.split("<<").next().unwrap_or("").replace('<', " ");
            let surname = surname.replace('<', " ");
            if surname.chars().any(|c| c.is_ascii_alphabetic()) {
                names = Some((surname.trim().to_string(), given.trim().to_string()));
            }
        }
        // a passport's second line: its end date at 21..27 (YYMMDD)
        if i > 0 && line.len() >= 27 && lines[i - 1].starts_with('P') {
            if let Some(d) = line.get(21..27).and_then(yymmdd) {
                end = Some(d);
            }
        }
    }
    (names, end)
}

fn yymmdd(s: &str) -> Option<NaiveDate> {
    let n: Vec<u32> = [&s[0..2], &s[2..4], &s[4..6]].iter().map(|p| p.parse().ok()).collect::<Option<Vec<_>>>()?;
    NaiveDate::from_ymd_opt(2000 + n[0] as i32, n[1], n[2])
}

/// The dates in `line` (day first, ISO, or « 12 mars 2031 », « 12 MARS/MAR 2031 »).
fn dates_in(line: &str) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    for c in NUMERIC.captures_iter(line) {
        let d = match (c.get(1), c.get(4)) {
            (Some(d), _) => NaiveDate::from_ymd_opt(c[3].parse().unwrap_or(0), c[2].parse().unwrap_or(0), d.as_str().parse().unwrap_or(0)),
            (_, Some(y)) => NaiveDate::from_ymd_opt(y.as_str().parse().unwrap_or(0), c[5].parse().unwrap_or(0), c[6].parse().unwrap_or(0)),
            _ => None,
        };
        out.extend(d);
    }
    let folded = fold(line).replace(" / ", "/");
    for c in WORDED.captures_iter(&folded) {
        let month = MONTHS.iter().find(|(m, _)| c[2].starts_with(m)).map(|(_, n)| *n);
        if let Some(m) = month {
            out.extend(NaiveDate::from_ymd_opt(c[3].parse().unwrap_or(0), m, c[1].parse().unwrap_or(0)));
        }
    }
    out
}

fn has_any(folded: &str, words: &[&str]) -> bool {
    let padded = format!(" {folded} ");
    words.iter().any(|w| padded.contains(&format!(" {w} ")) || (w.ends_with("jusqu") && padded.contains(w)))
}

/// A label line's value: on the same line after the label, else the next line.
fn labelled(lines: &[&str], labels: &[&str]) -> Option<String> {
    for (i, line) in lines.iter().enumerate() {
        let parts: Vec<&str> = line.splitn(2, [':']).collect();
        // « 1. Name » on a driving licence: its field's number left out
        let head = fold(parts[0]).split(' ').skip_while(|w| w.chars().any(|c| c.is_ascii_digit())).collect::<Vec<_>>().join(" ");
        // « Nom / Surname », « Nom » alone: the head is made of labels only (whole phrases, the longest first)
        let mut rest = format!(" {head} ");
        let mut phrases: Vec<&str> = labels.to_vec();
        phrases.sort_by_key(|p| std::cmp::Reverse(p.len()));
        for p in phrases {
            rest = rest.replace(&format!(" {p} "), " ");
        }
        let is_label = !head.is_empty() && rest.split_whitespace().all(|w| ["et", "and", "und", "y"].contains(&w));
        if !is_label || head.split(' ').count() > 5 {
            continue;
        }
        let value = parts.get(1).map(|v| v.trim()).filter(|v| !v.is_empty()).map(str::to_string).or_else(|| lines.get(i + 1).map(|v| v.trim().to_string()));
        if let Some(v) = value.filter(|v| !v.is_empty() && !v.chars().any(|c| c.is_ascii_digit())) {
            return Some(v.split(',').next().unwrap_or(&v).trim().to_string());
        }
    }
    None
}

fn title_case(name: &str) -> String {
    name.split_whitespace()
        .map(|w| {
            let lower = w.to_lowercase();
            let mut c = lower.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the rules read in a paper's text.
#[derive(Debug, Default, PartialEq)]
pub struct Read {
    pub kind: Option<&'static str>,
    pub document: String,
    /// As written on the paper (« Léa Martin »).
    pub holder: String,
    pub expires: Option<NaiveDate>,
}

/// The kind, the holder and the end date of a paper, by rules, from the text the phone read.
pub fn read(text: &str, lang: &str, today: NaiveDate) -> Read {
    let (mrz_names, mrz_end) = from_mrz(text);
    let clean = scrub(text);
    let folded = fold(&clean);
    let i = super::lang_index(lang).min(3);
    let kind = KINDS.iter().find(|(_, _, words)| has_any(&folded, words));
    let lines: Vec<&str> = clean.lines().filter(|l| !l.trim().is_empty()).collect();
    let surname = labelled(&lines, SURNAMES);
    let given = labelled(&lines, GIVEN);
    let holder = match (given, surname, mrz_names) {
        (Some(g), Some(s), _) => format!("{} {}", title_case(&g), title_case(&s)),
        (_, _, Some((s, g))) => format!("{} {}", title_case(&g), title_case(&s)).trim().to_string(),
        (None, Some(s), None) => title_case(&s),
        _ => String::new(),
    };
    // the end date: after an end word (same line or the next), else the MRZ's, else the latest one still ahead
    let mut expires = None;
    for (n, line) in lines.iter().enumerate() {
        if has_any(&fold(line), ENDS) {
            let found = dates_in(line).into_iter().last().or_else(|| lines.get(n + 1).and_then(|l| dates_in(l).into_iter().last()));
            if found.is_some() {
                expires = found;
                break;
            }
        }
    }
    let expires = expires
        .or(mrz_end)
        .or_else(|| lines.iter().flat_map(|l| dates_in(l)).filter(|d| *d > today).max());
    Read { kind: kind.map(|k| k.0), document: kind.map(|k| k.1[i].to_string()).unwrap_or_default(), holder, expires }
}

/// The holder as the Carnet knows them: the person page that matches (« Lea Martin » → « Léa Martin »), else as read.
pub fn holder_page(config: &Config, holder: &str) -> String {
    if holder.trim().is_empty() {
        return String::new();
    }
    let found = super::candidates(config, &[Part::Main], holder, "person");
    match found.as_slice() {
        [one] => one.page.title.clone(),
        _ => holder.to_string(),
    }
}

pub fn dir(config: &Config) -> PathBuf {
    super::root(config).join("papers")
}

fn photo_path(config: &Config, id: &str) -> Option<PathBuf> {
    if !id.starts_with('r') || !id[1..].chars().all(|c| c.is_ascii_hexdigit()) || id.len() > 20 {
        return None;
    }
    ["jpg", "png"].iter().map(|e| dir(config).join(format!("{id}.{e}"))).find(|p| p.is_file())
}

/// The photo of paper record `id` and its type.
pub fn photo(config: &Config, id: &str) -> Option<(Vec<u8>, &'static str)> {
    let path = photo_path(config, id)?;
    let bytes = std::fs::read(&path).ok()?;
    let kind = if path.extension().is_some_and(|e| e == "png") { "image/png" } else { "image/jpeg" };
    Some((bytes, kind))
}

fn keep_photo(config: &Config, id: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let ext = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        "jpg"
    } else {
        return Err(super::NotebookError("the photo is a JPEG or a PNG".into()).into());
    };
    let folder = dir(config);
    std::fs::create_dir_all(&folder)?;
    py::chmod(&folder.to_string_lossy(), 0o700)?;
    for old in ["jpg", "png"] {
        let _ = std::fs::remove_file(folder.join(format!("{id}.{old}")));
    }
    py::write_with_mode(&folder.join(format!("{id}.{ext}")).to_string_lossy(), bytes, 0o600)?;
    Ok(())
}

/// Photos whose record is gone (« Oublie ça », a record removed) taken out. Returns how many.
pub fn sweep(config: &Config) -> usize {
    let Some(t) = records::kind("paper") else { return 0 };
    let kept: std::collections::HashSet<String> = records::records(config, t)
        .into_iter()
        .chain(records::archived_records(config, t))
        .map(|r| tk::str_or_empty(r.get("id")))
        .collect();
    let Ok(entries) = std::fs::read_dir(dir(config)) else { return 0 };
    let mut gone = 0;
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let id = name.split('.').next().unwrap_or("").to_string();
        if !kept.contains(&id) && std::fs::remove_file(e.path()).is_ok() {
            gone += 1;
        }
    }
    gone
}

struct Texts {
    end: &'static str,
    reminders: &'static str,
    and: &'static str,
    fix: &'static str,
    ok: &'static str,
    photo: &'static str,
    early: &'static str,
    renew: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { end: "Fin : {d}", reminders: "Rappels : {d}", and: "et", fix: "Corriger", ok: "OK", photo: "Voir la photo",
                        early: "Penser à renouveler {p}", renew: "Renouveler {p}" },
        "de" => Texts { end: "Ende: {d}", reminders: "Erinnerungen: {d}", and: "und", fix: "Korrigieren", ok: "OK", photo: "Foto ansehen",
                        early: "An die Erneuerung denken: {p}", renew: "{p} erneuern" },
        "es" => Texts { end: "Fin: {d}", reminders: "Recordatorios: {d}", and: "y", fix: "Corregir", ok: "OK", photo: "Ver la foto",
                        early: "Pensar en renovar {p}", renew: "Renovar {p}" },
        _ => Texts { end: "Ends: {d}", reminders: "Reminders: {d}", and: "and", fix: "Fix", ok: "OK", photo: "See the photo",
                     early: "Think about renewing {p}", renew: "Renew {p}" },
    }
}

/// « 12 mars 2031 ».
fn long_day(d: NaiveDate, lang: &str) -> String {
    format!("{} {}", super::offline::day(d, lang), d.year())
}

fn first_name(holder: &str) -> String {
    holder.split_whitespace().next().unwrap_or(holder).to_string()
}

/// The two reminders' days: 3 months and 1 month before the end (those still ahead; today when both are past).
pub fn reminder_days(expires: NaiveDate, today: NaiveDate) -> Vec<NaiveDate> {
    let days: Vec<NaiveDate> = [3, 1]
        .iter()
        .filter_map(|m| expires.checked_sub_months(Months::new(*m)))
        .filter(|d| *d >= today)
        .collect();
    if days.is_empty() {
        vec![today]
    } else {
        days
    }
}

/// The two tasks of a paper record (once per record). Returns their ids.
pub fn remind(config: &Config, record: &Dict, who: &Who, today: NaiveDate) -> anyhow::Result<Vec<String>> {
    let id = tk::str_or_empty(record.get("id"));
    let Ok(expires) = NaiveDate::parse_from_str(&tk::str_or_empty(record.get("expires")), "%Y-%m-%d") else { return Ok(Vec::new()) };
    let state = super::read_state(config);
    let made = super::strings(state.get("papers").and_then(|p| p.get(&id)));
    if !made.is_empty() {
        return Ok(made);
    }
    let lang = super::language(config);
    let t = texts(&lang);
    let what = [tk::str_or_empty(record.get("document")), first_name(&tk::str_or_empty(record.get("holder")))]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let days = reminder_days(expires, today);
    let mut ids = Vec::new();
    for (n, day) in days.iter().enumerate() {
        let title = if n + 1 < days.len() { t.early } else { t.renew }.replace("{p}", &what);
        let added = super::add_task(config, Part::Main, &title, &day.format("%Y-%m-%d").to_string(), "", "asked", "", who)?;
        ids.push(added.task.id);
    }
    let _guard = super::lock();
    super::update_state(config, |s| {
        let mut all = s.get("papers").and_then(Value::as_object).cloned().unwrap_or_default();
        all.insert(id.clone(), json!(ids));
        s.insert("papers".into(), Value::Object(all));
    })?;
    Ok(ids)
}

/// An agent's paper record (`notebook_record type=paper`, from a photo sent in a conversation): its fields cleaned as
/// the phone's path cleans them, the holder matched to their page.
pub fn clean_fields(config: &Config, fields: &mut Dict) {
    fields.remove("number");
    for (k, v) in fields.iter_mut() {
        if let Value::String(text) = v {
            *text = scrub(text).replace('\n', " ").replace("•••", "").trim().to_string();
            if k == "holder" {
                *text = holder_page(config, text);
            }
        }
    }
}

/// Why a paper was not filed.
#[derive(Debug, PartialEq)]
pub enum Unfiled {
    /// Neither a kind of paper nor a date.
    Unreadable,
}

/// `POST /v1/notebook/paper`: the paper filed from the text the phone read (and its photo). Ready to show.
pub fn file(config: &Config, text: &str, image: Option<&[u8]>, lang: &str, today: NaiveDate) -> anyhow::Result<Result<Value, Unfiled>> {
    let carnet_lang = super::language(config);
    let read = read(text, &carnet_lang, today);
    if read.kind.is_none() && read.expires.is_none() {
        return Ok(Err(Unfiled::Unreadable));
    }
    let Some(t) = records::kind("paper") else { return Ok(Err(Unfiled::Unreadable)) };
    let holder = holder_page(config, &read.holder);
    let mut fields = Dict::new();
    let document = if read.document.is_empty() { texts_document(&carnet_lang) } else { read.document.clone() };
    fields.insert("document".into(), json!(document));
    if !holder.is_empty() {
        fields.insert("holder".into(), json!(holder));
    }
    if let Some(d) = read.expires {
        fields.insert("expires".into(), json!(d.format("%Y-%m-%d").to_string()));
    }
    let who = Who::you(config);
    let (record, _) = records::upsert(config, t, "", &fields, false, &who)?;
    let id = tk::str_or_empty(record.get("id"));
    if let Some(bytes) = image {
        keep_photo(config, &id, bytes)?;
    }
    let tasks = remind(config, &record, &who, today)?;
    let shown = texts(lang);
    let mut title = document.clone();
    if !holder.is_empty() {
        title.push_str(&format!(" · {}", first_name(&holder)));
    }
    let mut lines = Vec::new();
    let mut days_shown = Vec::new();
    if let Some(d) = read.expires {
        lines.push(shown.end.replace("{d}", &long_day(d, lang)));
        days_shown = reminder_days(d, today).into_iter().map(|d| long_day(d, lang)).collect();
    }
    if !days_shown.is_empty() {
        lines.push(shown.reminders.replace("{d}", &days_shown.join(&format!(" {} ", shown.and))));
    }
    Ok(Ok(json!({
        "record": {"id": id, "type": "paper", "document": document, "holder": holder,
                   "expires": read.expires.map(|d| d.format("%Y-%m-%d").to_string())},
        "card": {"title": title, "lines": lines, "buttons": [{"id": "fix", "text": shown.fix}, {"id": "ok", "text": shown.ok}]},
        "tasks": tasks,
        "photo": if image.is_some() { json!(shown.photo) } else { Value::Null },
    })))
}

/// A paper of an unknown kind (a date was read): « Papier ».
fn texts_document(lang: &str) -> String {
    ["Papier", "Paper", "Dokument", "Documento"][super::lang_index(lang).min(3)].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic French passport (ICAO's test MRZ, made-up person), as a phone reads it.
    pub const PASSPORT: &str = "RÉPUBLIQUE FRANÇAISE\nPASSEPORT / PASSPORT\nType P  Code FRA  Passeport n° 19AB12345\n\
Nom / Surname\nMARTIN\nPrénoms / Given names\nLÉA, MARIE\nDate de naissance 03.04.1990\n\
Date de délivrance / Date of issue\n12.03.2021\nDate d'expiration / Date of expiry\n12 MARS/MAR 2031\n\
P<FRAMARTIN<<LEA<MARIE<<<<<<<<<<<<<<<<<<<<<<\n19AB123458FRA9004035F3103128<<<<<<<<<<<<<<04\n";

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let config = super::super::tests_config(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        super::super::note(&config, Part::Main, "person", "Léa Martin", "who", &["Sœur".into()], &[], "", "", &Who::you(&config)).unwrap();
        (dir, config)
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 9).unwrap()
    }

    #[test]
    fn the_number_never_stays() {
        let clean = scrub(PASSPORT);
        assert!(!clean.contains("19AB12345") && !clean.contains("<<") && !clean.contains("123458"), "{clean}");
        assert!(clean.contains("MARTIN") && clean.contains("12 MARS/MAR 2031"));
    }

    #[test]
    fn a_passport_read_by_rules() {
        let r = read(PASSPORT, "fr", today());
        assert_eq!(r.kind, Some("passport"));
        assert_eq!(r.document, "Passeport");
        assert_eq!(r.holder, "Léa Martin");
        assert_eq!(r.expires, NaiveDate::from_ymd_opt(2031, 3, 12));
        // without the labels: the MRZ's names and end date
        let mrz_only = "PASSPORT\nP<FRAMARTIN<<LEA<<<<<<<<<<<<<<<<<<<<<<<<<<<<\n19AB123458FRA9004035F3103128<<<<<<<<<<<<<<04";
        let r = read(mrz_only, "en", today());
        assert_eq!((r.holder.as_str(), r.expires), ("Lea Martin", NaiveDate::from_ymd_opt(2031, 3, 12)));
        // a driving licence in German, a date only
        let r = read("FÜHRERSCHEIN\n1. Name: Becker\n2. Vorname: Jonas\n4b. gültig bis 01.07.2033\n5. B072RRE2I55", "de", today());
        assert_eq!((r.kind, r.holder.as_str(), r.expires), (Some("licence"), "Jonas Becker", NaiveDate::from_ymd_opt(2033, 7, 1)));
        assert_eq!(read("bonjour", "fr", today()), Read::default());
    }

    #[test]
    fn filed_with_its_photo_and_two_tasks() {
        let (_d, config) = setup();
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 1, 2, 3];
        let out = file(&config, PASSPORT, Some(&jpeg), "fr", today()).unwrap().unwrap();
        assert_eq!(out["record"]["holder"], "Léa Martin");
        assert_eq!(out["card"]["title"], "Passeport · Léa");
        assert_eq!(out["card"]["lines"], json!(["Fin : 12 mars 2031", "Rappels : 12 déc. 2030 et 12 févr. 2031"]));
        assert_eq!(out["photo"], "Voir la photo");
        assert_eq!(out["tasks"].as_array().unwrap().len(), 2);
        let id = out["record"]["id"].as_str().unwrap();
        let (bytes, kind) = photo(&config, id).unwrap();
        assert_eq!((bytes.as_slice(), kind), (&jpeg[..], "image/jpeg"));
        // the same paper again: the same record, no new tasks
        let again = file(&config, PASSPORT, None, "fr", today()).unwrap().unwrap();
        assert_eq!(again["record"]["id"], out["record"]["id"]);
        assert_eq!(again["tasks"], out["tasks"]);
        // no number anywhere in the Carnet's folder
        for entry in walkdir(&super::super::root(&config)) {
            let text = String::from_utf8_lossy(&std::fs::read(&entry).unwrap()).to_string();
            assert!(!text.contains("19AB12345") && !text.contains("123458"), "{}", entry.display());
        }
        // a second holder: a second record
        let marc = PASSPORT.replace("MARTIN\nPrénoms / Given names\nLÉA, MARIE", "DUPONT\nPrénoms / Given names\nMARC");
        let other = file(&config, &marc, None, "fr", today()).unwrap().unwrap();
        assert_ne!(other["record"]["id"], out["record"]["id"]);
        assert_eq!(file(&config, "bonjour", None, "fr", today()).unwrap(), Err(Unfiled::Unreadable));
        // the record gone: its photo too
        records::remove(&config, records::kind("paper").unwrap(), id, &Who::you(&config)).unwrap();
        assert_eq!(sweep(&config), 1);
        assert!(photo(&config, id).is_none());
    }

    fn walkdir(root: &std::path::Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walkdir(&p));
            } else {
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn the_reminders_days() {
        let end = NaiveDate::from_ymd_opt(2027, 1, 20).unwrap();
        assert_eq!(reminder_days(end, today()), vec![NaiveDate::from_ymd_opt(2026, 12, 20).unwrap()]);
        assert_eq!(reminder_days(NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(), today()), vec![today()]);
    }
}
