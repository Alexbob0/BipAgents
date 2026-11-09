//! The week's review (Carnet v2, decision 15 of internal/notes/carnet-v2/spec.md, contract.md §3.6): « Ta semaine dans
//! le Carnet », on the Carnet's home, only for someone who really lives in it — and then almost never. Everything must
//! hold (`due`):
//!
//! 1. the app opened at least 4 days out of 7, in each of the last 3 weeks (`active_days`, ask.rs `app_seen`);
//! 2. at least 12 person or subject pages with an « En bref », and « Toi » filled in at least 4 sections;
//! 3. at least 8 new facts written by agents (not by the person) in the last 7 days;
//! 4. no review done in the last 7 days; 2 reviews left in a row → 4 weeks off; « Ne plus proposer » → never.
//!
//! Shown from Saturday evening (18:00) and on Sunday, never at quiet hours. At most 10 facts, written by agents, never a
//! health one (lexicon::is_health), each still on its page. No model, no conversation slot (it lives on the home).
//!
//!     GET  /v1/notebook/review?lang=   204, or {card, facts, done}
//!     POST /v1/notebook/review/done    {off?}   → 204
//!     notebook/state.json              review {offered (week), done (week), last_at, ignored_in_row, paused_until, off}

use chrono::Datelike;
use serde_json::{json, Value};

use super::ask;
use super::{lexicon, Part};
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

pub const FACTS_MAX: usize = 10;
pub const DAYS_A_WEEK: u32 = 4;
pub const PAGES_MIN: usize = 12;
pub const SELF_SECTIONS_MIN: usize = 4;
pub const NEW_FACTS_MIN: usize = 8;
const DAY: f64 = 86400.0;
const PAUSE_DAYS: f64 = 28.0;
/// The actions of `log.jsonl` that add facts.
const WRITES: [&str; 2] = ["note", "create"];

struct Texts {
    title: &'static str,
    subtitle: &'static str,
    button: &'static str,
    off: &'static str,
    done: &'static str,
}

fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { title: "Ta semaine dans le Carnet", subtitle: "{n} nouveautés · 2 min", button: "Voir", off: "Ne plus proposer", done: "Tout est bon" },
        "de" => Texts { title: "Deine Woche im Notizbuch", subtitle: "{n} Neuigkeiten · 2 Min.", button: "Ansehen", off: "Nicht mehr vorschlagen", done: "Alles stimmt" },
        "es" => Texts { title: "Tu semana en el Cuaderno", subtitle: "{n} novedades · 2 min", button: "Ver", off: "No volver a proponer", done: "Todo está bien" },
        _ => Texts { title: "Your week in the Notebook", subtitle: "{n} new things · 2 min", button: "See", off: "Don't suggest again", done: "All good" },
    }
}

/// Condition 1: at least 4 days out of 7 in each of the last 3 weeks.
pub fn active_enough(state: &Dict, today: chrono::NaiveDate) -> bool {
    let bits = ask::active_bits(state, today);
    (0..3).all(|week| ((bits >> (week * 7)) & 0x7f).count_ones() >= DAYS_A_WEEK)
}

/// Condition 2: the Carnet really filled in.
pub fn filled_enough(config: &Config) -> bool {
    let lang = super::language(config);
    let w = super::words(&lang);
    let mut pages = 0;
    let mut me_sections = 0;
    for (_, page) in super::all_pages(config, Part::Main) {
        let kind = page.kind();
        let body = super::in_outline(config, &page);
        if (kind == "person" || kind == "subject") && !super::summary_of(&body, w.brief).trim().is_empty() {
            pages += 1;
        }
        if kind == "self" {
            me_sections = filled_sections(&body, w.brief);
        }
    }
    pages >= PAGES_MIN && me_sections >= SELF_SECTIONS_MIN
}

/// The sections of `body` (« En bref » apart) with at least one line.
fn filled_sections(body: &str, brief: &str) -> usize {
    let mut count = 0;
    let mut current: Option<String> = None;
    let mut filled = false;
    for line in body.lines().chain(std::iter::once("## ")) {
        if let Some(name) = line.strip_prefix("## ") {
            if filled && current.as_deref().is_some_and(|c| super::fold(c) != super::fold(brief)) {
                count += 1;
            }
            current = Some(name.to_string());
            filled = false;
        } else if current.is_some() && !line.trim().is_empty() {
            filled = true;
        }
    }
    count
}

/// The facts agents wrote in the last 7 days, newest first, still on their page, never health: (log entry, fact).
fn new_facts(config: &Config, now: f64) -> Vec<(Dict, String)> {
    let mut out = Vec::new();
    let mut texts: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for entry in super::history(config, Some(Part::Main), None, usize::MAX) {
        let at = chrono::DateTime::parse_from_rfc3339(&tk::str_or_empty(entry.get("at"))).map(|d| d.timestamp() as f64).unwrap_or(0.0);
        if now - at > 7.0 * DAY {
            break;
        }
        let by = tk::str_or_empty(entry.get("by"));
        let action = tk::str_or_empty(entry.get("action"));
        if by == "you" || by == "nightly" || entry.get("agent").is_none_or(Value::is_null) || !WRITES.contains(&action.as_str()) {
            continue;
        }
        let path = tk::str_or_empty(entry.get("path"));
        let text = texts.entry(path.clone()).or_insert_with(|| super::read_page_text(config, Part::Main, &path).unwrap_or_default());
        for fact in tk::str_or_empty(entry.get("summary")).split(" · ").map(str::trim).filter(|f| !f.is_empty()) {
            if lexicon::is_health(fact) || !text.lines().any(|l| l.trim_start_matches("- ").starts_with(fact)) {
                continue;
            }
            out.push((entry.clone(), fact.to_string()));
        }
    }
    out
}

/// Whether the review is due now (everything holds); its facts when it is.
pub fn due(config: &Config, now: f64) -> Option<Vec<(Dict, String)>> {
    let state = super::read_state(config);
    let r = state.get("review").cloned().unwrap_or(json!({}));
    let today = ask::local(now).date_naive();
    let evening = (ask::weekday(now) == 6 && ask::hour(now) >= 18) || ask::weekday(now) == 7;
    let paused = r.get("paused_until").and_then(Value::as_f64).is_some_and(|t| now < t);
    let recent = r.get("last_at").and_then(Value::as_f64).is_some_and(|t| now - t < 7.0 * DAY);
    if !evening
        || ask::quiet_in(&state, now)
        || r.get("off").map(py::truthy).unwrap_or(false)
        || paused
        || recent
        || !active_enough(&state, today)
        || !filled_enough(config)
    {
        return None;
    }
    let facts = new_facts(config, now);
    (facts.len() >= NEW_FACTS_MIN).then_some(facts)
}

fn week_of(now: f64) -> String {
    let w = ask::local(now).date_naive().iso_week();
    format!("{}-W{:02}", w.year(), w.week())
}

/// `GET /v1/notebook/review`: the review, or None (204). Offering a new week counts the last one left unanswered:
/// 2 in a row → 4 weeks off.
pub fn view(config: &Config, lang: &str, now: f64) -> anyhow::Result<Option<Value>> {
    let Some(facts) = due(config, now) else { return Ok(None) };
    let week = week_of(now);
    let _guard = super::lock();
    let state = super::read_state(config);
    let r = state.get("review").and_then(Value::as_object).cloned().unwrap_or_default();
    let offered = tk::str_or_empty(r.get("offered"));
    if offered != week {
        let ignored = !offered.is_empty() && tk::str_or_empty(r.get("done")) != offered;
        let row = if ignored { r.get("ignored_in_row").and_then(Value::as_i64).unwrap_or(0) + 1 } else { 0 };
        let pause = row >= 2;
        super::update_state(config, |s| {
            let mut r = s.get("review").and_then(Value::as_object).cloned().unwrap_or_default();
            r.insert("offered".into(), json!(week));
            r.insert("ignored_in_row".into(), json!(if pause { 0 } else { row }));
            if pause {
                r.insert("paused_until".into(), json!(now + PAUSE_DAYS * DAY));
            }
            s.insert("review".into(), Value::Object(r));
        })?;
        if pause {
            return Ok(None);
        }
    }
    let t = texts(lang);
    let names = |agent: &str| config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or_else(|| agent.to_string());
    let shown: Vec<Value> = facts
        .iter()
        .take(FACTS_MAX)
        .map(|(entry, fact)| {
            let path = tk::str_or_empty(entry.get("path"));
            let section = super::read_page_text(config, Part::Main, &path)
                .and_then(|text| section_of(config, &text, &path, fact))
                .unwrap_or_default();
            let at = chrono::DateTime::parse_from_rfc3339(&tk::str_or_empty(entry.get("at")))
                .map(|d| d.with_timezone(&chrono::Local).date_naive())
                .unwrap_or_else(|_| chrono::Local::now().date_naive());
            json!({"change_id": entry.get("id"), "text": fact, "page": entry.get("title"), "path": path, "section": section,
                   "by": names(&tk::str_or_empty(entry.get("agent"))), "when": super::offline::day(at, lang)})
        })
        .collect();
    Ok(Some(json!({
        "card": {"title": t.title, "subtitle": t.subtitle.replace("{n}", &facts.len().to_string()), "button": t.button,
                 "menu": [{"id": "off", "text": t.off}]},
        "facts": shown, "done": t.done,
    })))
}

/// The section id of the page holding `fact`.
fn section_of(config: &Config, text: &str, path: &str, fact: &str) -> Option<String> {
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let page = super::Page::parse(text, &stem);
    let kind = page.kind();
    let lang = super::language(config);
    let mut current: Option<String> = None;
    for line in super::in_outline(config, &page).lines() {
        if let Some(name) = line.strip_prefix("## ") {
            current = Some(name.to_string());
        } else if line.trim_start_matches("- ").starts_with(fact) {
            let title = current.clone()?;
            return super::template::ids(&kind)
                .into_iter()
                .find(|id| super::fold(&super::template::title(&kind, id, &lang)) == super::fold(&title))
                .map(str::to_string);
        }
    }
    None
}

/// `POST /v1/notebook/review/done`: « Tout est bon » (or « Ne plus proposer », `off`).
pub fn done(config: &Config, off: bool, now: f64) -> std::io::Result<()> {
    let _guard = super::lock();
    super::update_state(config, |s| {
        let mut r = s.get("review").and_then(Value::as_object).cloned().unwrap_or_default();
        let offered = r.get("offered").cloned().unwrap_or(json!(week_of(now)));
        r.insert("done".into(), offered);
        r.insert("last_at".into(), json!(now));
        r.insert("ignored_in_row".into(), json!(0));
        if off {
            r.insert("off".into(), json!(true));
        }
        s.insert("review".into(), Value::Object(r));
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::Who;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        let agent = crate::config::AgentConfig { name: "quotidien".into(), display_name: "Quotidien".into(), ..Default::default() };
        config.agents.insert("quotidien".into(), agent);
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    /// The coming Sunday at `hour`, `weeks` later.
    fn sunday(weeks: i64, hour: u32) -> f64 {
        use chrono::TimeZone;
        let today = chrono::Local::now().date_naive();
        let d = today + chrono::Duration::days(7 - today.weekday().num_days_from_monday() as i64 - 1 + 7 * weeks);
        chrono::Local.from_local_datetime(&d.and_hms_opt(hour, 0, 0).unwrap()).single().unwrap().timestamp() as f64
    }

    fn active(config: &Config, now: f64, bits: u32) {
        super::super::update_state(config, |s| {
            s.insert("active_days".into(), json!({"day": ask::day_of(now), "bits": bits}));
        })
        .unwrap();
    }

    fn fill(config: &Config, pages: usize, facts_each: usize) {
        let who = Who::agent(config, "quotidien");
        for i in 0..pages {
            let facts: Vec<String> = (0..facts_each).map(|n| format!("Aime la couleur {}", ["bleu", "vert", "rouge"][n % 3])).collect();
            super::super::note(config, Part::Main, "person", &format!("Ami {}", ["Alain", "Bruno", "Chloé", "Denis", "Emma", "Fanny", "Gilles", "Hugo", "Inès", "Jules", "Karim", "Laura", "Marc"][i]), "tastes", &facts, &[], "", "Un ami proche", &who).unwrap();
        }
        for (section, fact) in [("about", "Habite à Lyon"), ("family", "Deux enfants"), ("work", "Ingénieur"), ("habits", "Court le dimanche")] {
            super::super::note(config, Part::Main, "self", "Toi", section, &[fact.into()], &[], "", "", &who).unwrap();
        }
    }

    #[test]
    fn only_for_a_carnet_lived_in() {
        let (_d, config) = setup();
        let now = sunday(0, 11);
        // occasional use: never
        fill(&config, 13, 1);
        active(&config, now, 0b0000011_0000011_0000011);
        assert!(view(&config, "fr", now).unwrap().is_none());
        // 4 days a week for 3 weeks: yes
        active(&config, now, 0b0001111_0001111_0001111);
        let shown = view(&config, "fr", now).unwrap().expect("due");
        assert_eq!(shown["card"]["title"], "Ta semaine dans le Carnet");
        assert!(shown["card"]["subtitle"].as_str().unwrap().ends_with(" nouveautés · 2 min"));
        assert_eq!(shown["facts"].as_array().unwrap().len(), FACTS_MAX);
        assert_eq!(shown["facts"][0]["by"], "Quotidien");
        // not on a Wednesday, not at quiet hours
        assert!(view(&config, "fr", now - 4.0 * DAY).unwrap().is_none());
        assert!(view(&config, "fr", sunday(0, 23)).unwrap().is_none());
        // done: not for 7 days
        done(&config, false, now).unwrap();
        assert!(view(&config, "fr", now + 3600.0).unwrap().is_none());
    }

    #[test]
    fn each_condition_alone() {
        let (_d, config) = setup();
        let now = sunday(0, 11);
        active(&config, now, 0b0001111_0001111_0001111);
        // too few pages
        fill(&config, 5, 2);
        assert!(view(&config, "fr", now).unwrap().is_none());
        // health is never shown, the person's own facts don't count
        let (_d2, config2) = setup();
        active(&config2, now, 0b0001111_0001111_0001111);
        fill(&config2, 13, 1);
        super::super::note(&config2, Part::Main, "person", "Ami Alain", "tastes", &["Aime le jazz".into()], &[], "", "", &Who::you(&config2)).unwrap();
        let facts = new_facts(&config2, now);
        assert!(facts.iter().all(|(_, f)| f != "Aime le jazz"));
    }

    #[test]
    fn two_left_in_a_row_four_weeks_off() {
        let (_d, config) = setup();
        fill(&config, 13, 1);
        let first = sunday(0, 11);
        active(&config, first, 0b0001111_0001111_0001111);
        assert!(view(&config, "fr", first).unwrap().is_some());
        // left; a week later shown again (the facts are a week old: rewritten)
        super::super::update_state(&config, |s| {
            let mut r = s["review"].as_object().unwrap().clone();
            r.insert("offered".into(), json!("2000-W01"));
            s.insert("review".into(), Value::Object(r));
        })
        .unwrap();
        assert!(view(&config, "fr", first).unwrap().is_some());
        super::super::update_state(&config, |s| {
            let mut r = s["review"].as_object().unwrap().clone();
            r.insert("offered".into(), json!("2000-W02"));
            s.insert("review".into(), Value::Object(r));
        })
        .unwrap();
        assert!(view(&config, "fr", first).unwrap().is_none(), "2 left in a row: 4 weeks off");
        let r = &super::super::read_state(&config)["review"];
        assert!(r["paused_until"].as_f64().unwrap() > first + 27.0 * DAY);
        // « Ne plus proposer »
        done(&config, true, first).unwrap();
        assert!(super::super::read_state(&config)["review"]["off"].as_bool().unwrap());
    }

    #[test]
    fn sections_filled() {
        assert_eq!(filled_sections("## En bref\n- x\n## A\n- y\n## B\n\n## C\n- z\n", "En bref"), 2);
    }
}
