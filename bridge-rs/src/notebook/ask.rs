//! What the Carnet itself says to the person (Carnet v2, socle §0 and decisions 5-6 of internal/notes/carnet-v2/spec.md,
//! contract.md §4): one Carnet card a day at most in the conversations (the question of the day here, a follow-up in
//! followups.rs), never in a notification, never at quiet hours, never in a background task, a routine or a Live call.
//! A Carnet card is not an approval: it never goes to the Boîte (card_place.rs), never pushes, and an answer writes the
//! fact (or the task) without running the agent again.
//!
//! The question of the day: `notebook_write {page, section, ask: {question, chips}}` in a turn that read or wrote that
//! very page (« ce sujet précis »), on « Toi », a person or an area, for a section still empty. The bridge decides
//! (`from_agent`), the model only words it: no other call. Refused (« No question now. ») when the day's card is taken,
//! at quiet hours, while busy, for a section declined (« Ne plus me demander ça »), a question passed less than 30 days
//! ago, after 3 passes in a row (14 days off), with « Questions pour le Carnet » off, or for health or money the
//! person's message of that turn did not talk about (`on_message` keeps only that it did, never the text).
//!
//!     POST /v1/notebook/ask/{id}   {choice} · {skip} · {never} · {undo}   → the card folded (api.rs)
//!     notebook/state.json          quiet {from, to}, questions, slot {day, kind}, ask {declined[], skipped{},
//!                                  skips_in_row, paused_until}, active_days {day, bits}, cards [{card: ask|follow…}]
//!
//! Also the place's quiet hours (one setting, read by the tips of feature_tips.rs too) and the days the app was opened
//! (`app_seen`, 28 bits: the weekly review's condition, review.rs).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::{Datelike, Timelike};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::reads::Turn;
use super::{fold, lexicon, Part, Who};
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

/// Quiet hours by default (local time): from 22:00 to 08:00.
pub const QUIET: [&str; 2] = ["22:00", "08:00"];
/// A question passed is not asked again before this long.
const SKIPPED_DAYS: f64 = 30.0;
/// After this many passes in a row, no question for `PAUSE_DAYS`.
const SKIPS_IN_ROW: i64 = 3;
const PAUSE_DAYS: f64 = 14.0;
/// The answer to the agent when the bridge says no.
pub const NO: &str = "No question now.";
/// Lengths kept.
const QUESTION_MAX: usize = 160;
const CHIP_MAX: usize = 40;
/// The cards kept in `state.json` (with the « Oublie ça » ones).
pub const CARDS_MAX: usize = 100;
const DAY: f64 = 86400.0;

/// What the Carnet's cards say, in the person's language.
pub struct Texts {
    pub top: &'static str,
    pub skip: &'static str,
    pub never: &'static str,
    pub noted: &'static str,
    pub undo: &'static str,
    pub skipped: &'static str,
}

pub fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { top: "Pour ton Carnet", skip: "Passer", never: "Ne plus me demander ça", noted: "Noté : {v}", undo: "Annuler", skipped: "Passée" },
        "de" => Texts { top: "Für dein Notizbuch", skip: "Überspringen", never: "Das nicht mehr fragen", noted: "Notiert: {v}", undo: "Rückgängig", skipped: "Übersprungen" },
        "es" => Texts { top: "Para tu Cuaderno", skip: "Saltar", never: "No volver a preguntar", noted: "Anotado: {v}", undo: "Deshacer", skipped: "Saltada" },
        _ => Texts { top: "For your Notebook", skip: "Skip", never: "Don't ask me this again", noted: "Noted: {v}", undo: "Undo", skipped: "Skipped" },
    }
}

// -- time, quiet hours, the day's slot -------------------------------------------------------------------------------

pub fn local(now: f64) -> chrono::DateTime<chrono::Local> {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(now as i64, 0).single().unwrap_or_else(chrono::Local::now)
}

pub fn day_of(now: f64) -> String {
    local(now).format("%Y-%m-%d").to_string()
}

fn minutes(hhmm: &str) -> Option<u32> {
    let (h, m) = hhmm.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The quiet hours set (`state.json` `quiet`), else 22:00 → 08:00, as « HH:MM ».
pub fn quiet_hours(state: &Dict) -> [String; 2] {
    let q = state.get("quiet");
    let at = |k: &str, default: &str| {
        let v = q.and_then(|q| q.get(k)).and_then(Value::as_str).filter(|v| minutes(v).is_some());
        v.unwrap_or(default).to_string()
    };
    [at("from", QUIET[0]), at("to", QUIET[1])]
}

/// Whether `now` is within the quiet hours of `state`.
pub fn quiet_in(state: &Dict, now: f64) -> bool {
    let [from, to] = quiet_hours(state);
    let (Some(from), Some(to)) = (minutes(&from), minutes(&to)) else { return false };
    let t = local(now);
    let at = t.hour() * 60 + t.minute();
    match from.cmp(&to) {
        std::cmp::Ordering::Equal => false,
        std::cmp::Ordering::Less => at >= from && at < to,
        std::cmp::Ordering::Greater => at >= from || at < to,
    }
}

/// The place's quiet hours now (the Carnet's cards, the weekly review, the tips of feature_tips.rs).
pub fn is_quiet(config: &Config, now: f64) -> bool {
    quiet_in(&super::read_state(config), now)
}

/// « Questions pour le Carnet » (on by default).
pub fn questions_on(state: &Dict) -> bool {
    state.get("questions").map(py::truthy).unwrap_or(true)
}

/// Whether the day's Carnet card is still free.
pub fn slot_free(state: &Dict, now: f64) -> bool {
    state.get("slot").and_then(|s| s.get("day")).and_then(Value::as_str) != Some(day_of(now).as_str())
}

pub fn take_slot(state: &mut Dict, kind: &str, now: f64) {
    state.insert("slot".into(), json!({"day": day_of(now), "kind": kind}));
}

/// A card added to `state.json` `cards` (the oldest dropped past `CARDS_MAX`).
pub fn push_card(state: &mut Dict, card: Value) {
    let list = state.entry("cards").or_insert_with(|| json!([]));
    if let Value::Array(items) = list {
        items.push(card);
        let extra = items.len().saturating_sub(CARDS_MAX);
        items.drain(..extra);
    }
}

/// Whether `agent` has Carnet cards (its timeline then gets them: threads/mod.rs).
pub fn has_cards(config: &Config, agent: &str) -> bool {
    super::read_state(config)
        .get("cards")
        .and_then(Value::as_array)
        .is_some_and(|c| c.iter().any(|c| c.get("agent").and_then(Value::as_str) == Some(agent)))
}

/// The card `id` of `state`, to change.
pub fn card_mut<'a>(state: &'a mut Dict, id: &str) -> Option<&'a mut Dict> {
    state
        .get_mut("cards")?
        .as_array_mut()?
        .iter_mut()
        .find(|c| c.get("id").and_then(Value::as_str) == Some(id))?
        .as_object_mut()
}

// -- the days the app was opened (the weekly review: review.rs) ------------------------------------------------------

static SEEN_TODAY: Mutex<Option<String>> = Mutex::new(None);

/// `bits` of `active_days` moved to `today` (bit 0: that day, bit n: n days before; 28 days kept).
pub fn active_bits(state: &Dict, today: chrono::NaiveDate) -> u32 {
    let a = state.get("active_days");
    let bits = a.and_then(|a| a.get("bits")).and_then(Value::as_u64).unwrap_or(0) as u32;
    let Some(day) = a.and_then(|a| a.get("day")).and_then(Value::as_str).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
    else {
        return 0;
    };
    let shift = (today - day).num_days();
    if !(0..28).contains(&shift) {
        return 0;
    }
    (bits << shift) & 0x0fff_ffff
}

/// An authenticated request of the app (auth.rs): today counted as a day it was opened, once a day. Nothing else is
/// kept: no time, no screen, no content.
pub fn app_seen(config: &Config, now: f64) {
    let today = day_of(now);
    // no Carnet yet: not counted, and the day stays to count at a later request (once the Carnet is there)
    if !super::root(config).is_dir() {
        return;
    }
    {
        let mut seen = SEEN_TODAY.lock().unwrap_or_else(|e| e.into_inner());
        if seen.as_deref() == Some(today.as_str()) {
            return;
        }
        *seen = Some(today.clone());
    }
    let date = local(now).date_naive();
    let _guard = super::lock();
    let done = super::update_state(config, |s| {
        let bits = active_bits(s, date) | 1;
        s.insert("active_days".into(), json!({"day": today, "bits": bits}));
    });
    if let Err(err) = done {
        tracing::warn!(error = %err, "Carnet: the app's day not counted");
    }
}

/// [`app_seen`] from a request (auth.rs): written apart from it, and only the first time of the day.
pub fn app_seen_soon(config: &std::sync::Arc<Config>) {
    let now = py::time_time();
    let today = day_of(now);
    if SEEN_TODAY.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(today.as_str()) {
        return;
    }
    let config = std::sync::Arc::clone(config);
    std::thread::spawn(move || app_seen(&config, now));
}

#[cfg(test)]
pub fn forget_seen_today() {
    *SEEN_TODAY.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// -- what the person's message talked about (health, money), never the text ----------------------------------------

#[derive(Clone, Copy, Default)]
struct Mention {
    at: f64,
    health: bool,
    money: bool,
}

static MENTIONS: LazyLock<Mutex<HashMap<String, Mention>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn mention_key(agent: &str, session: &str) -> String {
    format!("{agent}\u{1f}{session}")
}

/// The person wrote to `agent` in `session` (live_brief.rs `message_notes`): whether the message talked about health
/// or money, kept in memory for its turn (a question about either needs it). The text is dropped.
pub fn on_message(agent: &str, session: &str, text: &str, now: f64) {
    if session.is_empty() {
        return;
    }
    let m = Mention { at: now, health: lexicon::is_health(text), money: lexicon::is_money(text) };
    let mut all = MENTIONS.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|_, v| now - v.at < 4.0 * 3600.0);
    all.insert(mention_key(agent, session), m);
}

fn mentioned(agent: &str, session: &str) -> Mention {
    MENTIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&mention_key(agent, session)).copied().unwrap_or_default()
}

// -- the question of the day ---------------------------------------------------------------------------------------

/// A question's fingerprint (folded text): what « passée » and « Ne plus » remember, never the text.
fn fingerprint(question: &str) -> String {
    let digest = Sha256::digest(fold(question).as_bytes());
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Words a question is made of that are not its label (folded).
const ASKING: &[&str] = &[
    "quel", "quelle", "quels", "quelles", "est", "sont", "combien", "c", "ce", "qu", "que", "quoi", "a", "il", "elle",
    "what", "which", "is", "are", "how", "many", "much", "does", "do", "his", "her", "their", "welche", "welcher",
    "welches", "was", "ist", "wie", "viel", "hat", "sein", "ihr", "ihre", "cual", "cuales", "es", "cuanto", "cuantos",
    "que", "su", "sus", "tiene",
];

/// The label a question asks about: the lexicon's (« chausse du combien » → « Pointure »), else its own words without
/// the asking ones and the page's names (« Quel est le plat préféré de Léa ? » → « Plat préféré »); 4 words at most.
pub fn label_of(question: &str, names: &[String], lang: &str) -> Option<String> {
    if let Some(id) = lexicon::query(question).1.first() {
        if let Some(l) = lexicon::LABELS.iter().find(|l| l.id == *id) {
            return Some(lexicon::name_of(l, lang).to_string());
        }
    }
    let named: Vec<String> = names.iter().flat_map(|n| fold(n).split(' ').map(str::to_string).collect::<Vec<_>>()).collect();
    let kept: Vec<String> = question
        .split(|c: char| c.is_whitespace() || c == '\'' || c == '’')
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .filter(|w| {
            let f = fold(w);
            !f.is_empty() && !ASKING.contains(&f.as_str()) && !lexicon::STOP.contains(&f.as_str()) && !named.contains(&f)
        })
        .map(str::to_string)
        .collect();
    if kept.is_empty() || kept.len() > 4 {
        return None;
    }
    let text = kept.join(" ");
    let mut chars = text.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

/// `notebook_write {ask}` by `agent` in `turn` about `page` › `section`: the card put under its answer, or « No
/// question now. ». `filled`: the section had facts before this call.
#[allow(clippy::too_many_arguments)]
pub fn from_agent(
    config: &Config,
    agent: &str,
    turn: &Turn,
    page: &super::Found,
    section: &str,
    ask: &Value,
    filled: bool,
    now: f64,
) -> anyhow::Result<String> {
    let question = tk::squeeze(ask.get("question").and_then(Value::as_str).unwrap_or("").trim());
    let chips: Vec<String> = match ask.get("chips") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|c| tk::squeeze(c.trim()))
            .filter(|c| !c.is_empty() && c.chars().count() <= CHIP_MAX)
            .collect(),
        _ => Vec::new(),
    };
    let kind = page.page.kind();
    let Some(section) = super::template::resolve(&kind, section) else { return Ok(NO.into()) };
    let session = turn.session.clone().unwrap_or_default();
    let fine = !question.is_empty()
        && question.chars().count() <= QUESTION_MAX
        && (2..=4).contains(&chips.len())
        && ["self", "person", "area"].contains(&kind.as_str())
        && page.part == Part::Main
        && !filled
        && !turn.busy
        && !session.is_empty()
        && !session.starts_with("cron_");
    if !fine {
        return Ok(NO.into());
    }
    let id = if page.page.id().is_empty() { page.path.clone() } else { page.page.id() };
    if !super::reads::touched(agent, turn, &id) {
        return Ok(NO.into());
    }
    // health or money: only when the person's message of this turn talked about it
    let said = mentioned(agent, &session);
    let all = std::iter::once(&question).chain(chips.iter());
    let (health, money) = all.fold((false, false), |(h, m), t| (h || lexicon::is_health(t), m || lexicon::is_money(t)));
    if (health && !said.health) || (money && !said.money) {
        return Ok(NO.into());
    }
    let lang = super::language(config);
    let mut names = vec![page.page.title.clone()];
    names.extend(page.page.aliases());
    let Some(label) = label_of(&question, &names, &lang) else { return Ok(NO.into()) };
    let print = fingerprint(&question);
    let declined_key = format!("{kind}:{section}");
    let _guard = super::lock();
    let state = super::read_state(config);
    let a = state.get("ask").cloned().unwrap_or(json!({}));
    let declined = super::strings(a.get("declined")).contains(&declined_key);
    let skipped_at = a.get("skipped").and_then(|s| s.get(&print)).and_then(Value::as_f64);
    let paused = a.get("paused_until").and_then(Value::as_f64).is_some_and(|t| now < t);
    let passed_unanswered = state.get("cards").and_then(Value::as_array).is_some_and(|cards| {
        cards.iter().any(|c| {
            tk::str_or_empty(c.get("card")) == "ask"
                && tk::str_or_empty(c.get("print")) == print
                && now - c.get("at").and_then(Value::as_f64).unwrap_or(0.0) < SKIPPED_DAYS * DAY
        })
    });
    let allowed = questions_on(&state)
        && slot_free(&state, now)
        && !quiet_in(&state, now)
        && !declined
        && !paused
        && skipped_at.is_none_or(|t| now - t >= SKIPPED_DAYS * DAY)
        && !passed_unanswered;
    if !allowed {
        return Ok(NO.into());
    }
    let card_id = format!("bip-ask-{}", py::token_hex(4));
    let card = json!({"id": card_id, "card": "ask", "at": now, "day": day_of(now), "agent": agent, "session": session,
                      "run": turn.run, "state": "open", "path": page.path, "title": page.page.title, "kind": kind,
                      "section": section, "label": label, "question": question, "chips": chips, "print": print});
    super::update_state(config, |s| {
        take_slot(s, "ask", now);
        push_card(s, card);
    })?;
    Ok("Question shown to the person under your answer.".into())
}

/// The folded or open view of an `ask` card (contract.md §4), None when it is gone (« Ne plus me demander ça »).
pub fn view(card: &Dict, lang: &str, now: f64) -> Option<Value> {
    let t = texts(lang);
    let id = card.get("id").cloned().unwrap_or(Value::Null);
    let mut state = tk::str_or_empty(card.get("state"));
    // unanswered: at midnight it is passed
    if state == "open" && tk::str_or_empty(card.get("day")) != day_of(now) {
        state = "skipped".into();
    }
    let mut body = json!({"kind": "carnet_card", "card": "ask", "id": id, "state": state});
    match state.as_str() {
        "open" => {
            body["top"] = json!(t.top);
            body["agent"] = card.get("agent").cloned().unwrap_or(Value::Null);
            body["question"] = card.get("question").cloned().unwrap_or(Value::Null);
            let mut chips: Vec<Value> = super::strings(card.get("chips")).iter().map(|c| json!({"id": c, "text": c})).collect();
            chips.push(json!({"id": "skip", "text": t.skip}));
            body["chips"] = json!(chips);
            body["menu"] = json!([{"id": "never", "text": t.never}]);
        }
        "done" => {
            body["text"] = json!(t.noted.replace("{v}", &tk::str_or_empty(card.get("choice"))));
            if !tk::str_or_empty(card.get("change_id")).is_empty() {
                body["undo"] = json!(t.undo);
            }
        }
        "skipped" => body["text"] = json!(t.skipped),
        _ => return None,
    }
    Some(body)
}

/// What the person tapped on an `ask` card.
pub enum Answer {
    Choice(String),
    Skip,
    Never,
    Undo,
}

/// Why an answer is refused.
#[derive(Debug, PartialEq)]
pub enum Refused {
    /// No such card.
    Missing,
    /// Answered or passed already: the card as it is.
    Gone(Value),
}

/// The person's tap on card `id` (`POST /v1/notebook/ask/{id}`): the fact written in their name (« Pointure : 36 »,
/// a label said again replaces the old one; a health answer goes where health goes), or the pass remembered. No
/// agent runs, nothing goes to the Boîte nor pushes. Returns the card folded.
pub fn answer(config: &Config, id: &str, answer: Answer, lang: &str, now: f64) -> anyhow::Result<Result<Value, Refused>> {
    let state = super::read_state(config);
    let Some(card) = state
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id) && tk::str_or_empty(c.get("card")) == "ask"))
        .and_then(Value::as_object)
        .cloned()
    else {
        return Ok(Err(Refused::Missing));
    };
    let shown = view(&card, lang, now);
    let open = shown.as_ref().and_then(|v| v.get("state")).and_then(Value::as_str) == Some("open");
    let who = Who::you(config);
    match answer {
        Answer::Undo => {
            let change = tk::str_or_empty(card.get("change_id"));
            if tk::str_or_empty(card.get("state")) != "done" {
                return Ok(Err(Refused::Gone(shown.unwrap_or(Value::Null))));
            }
            // no change id: the fact was there already, the page stays as it is
            if !change.is_empty() {
                super::undo_change(config, &change, &who)?;
            }
            let _guard = super::lock();
            let after = super::update_state(config, |s| {
                if let Some(c) = card_mut(s, id) {
                    c.insert("state".into(), json!("open"));
                    c.remove("choice");
                    c.remove("change_id");
                }
            })?;
            Ok(Ok(folded(&after, id, lang, now)))
        }
        _ if !open => Ok(Err(Refused::Gone(shown.unwrap_or(Value::Null)))),
        Answer::Choice(choice) => {
            let choice = tk::squeeze(choice.trim());
            if choice.is_empty() || lexicon::id_number(&choice) {
                return Ok(Err(Refused::Gone(shown.unwrap_or(Value::Null))));
            }
            let fact = format!("{} : {choice}", tk::str_or_empty(card.get("label")));
            let kind = tk::str_or_empty(card.get("kind"));
            let title = tk::str_or_empty(card.get("title"));
            let section = tk::str_or_empty(card.get("section"));
            // the write and its change id under one lock: « Annuler » undoes this change, never another one; a fact
            // already there wrote nothing, so there is nothing to undo
            let _guard = super::lock();
            let written = super::note_locked(config, Part::Main, &kind, &title, &section, &[fact], &[], "", "", &who)?;
            let change = if written.path.is_empty() || written.added.is_empty() {
                String::new()
            } else {
                super::history(config, None, Some(&written.path), 1)
                    .first()
                    .map(|e| tk::str_or_empty(e.get("id")))
                    .unwrap_or_default()
            };
            let after = super::update_state(config, |s| {
                if let Some(c) = card_mut(s, id) {
                    c.insert("state".into(), json!("done"));
                    c.insert("choice".into(), json!(choice));
                    c.insert("change_id".into(), json!(change));
                }
                ask_state(s, |a| {
                    a.insert("skips_in_row".into(), json!(0));
                });
            })?;
            Ok(Ok(folded(&after, id, lang, now)))
        }
        Answer::Skip => {
            let print = tk::str_or_empty(card.get("print"));
            let _guard = super::lock();
            let after = super::update_state(config, |s| {
                if let Some(c) = card_mut(s, id) {
                    c.insert("state".into(), json!("skipped"));
                }
                ask_state(s, |a| {
                    let mut skipped = a.get("skipped").and_then(Value::as_object).cloned().unwrap_or_default();
                    skipped.retain(|_, t| t.as_f64().is_some_and(|t| now - t < SKIPPED_DAYS * DAY));
                    skipped.insert(print.clone(), json!(now));
                    a.insert("skipped".into(), Value::Object(skipped));
                    let row = a.get("skips_in_row").and_then(Value::as_i64).unwrap_or(0) + 1;
                    if row >= SKIPS_IN_ROW {
                        a.insert("paused_until".into(), json!(now + PAUSE_DAYS * DAY));
                        a.insert("skips_in_row".into(), json!(0));
                    } else {
                        a.insert("skips_in_row".into(), json!(row));
                    }
                });
            })?;
            Ok(Ok(folded(&after, id, lang, now)))
        }
        Answer::Never => {
            let key = format!("{}:{}", tk::str_or_empty(card.get("kind")), tk::str_or_empty(card.get("section")));
            let _guard = super::lock();
            super::update_state(config, |s| {
                if let Some(c) = card_mut(s, id) {
                    c.insert("state".into(), json!("never"));
                }
                ask_state(s, |a| {
                    let mut declined = super::strings(a.get("declined"));
                    if !declined.contains(&key) {
                        declined.push(key.clone());
                    }
                    a.insert("declined".into(), json!(declined));
                });
            })?;
            Ok(Ok(json!({"kind": "carnet_card", "card": "ask", "id": id, "state": "never"})))
        }
    }
}

fn ask_state(state: &mut Dict, change: impl FnOnce(&mut Dict)) {
    let mut a = state.get("ask").and_then(Value::as_object).cloned().unwrap_or_default();
    change(&mut a);
    state.insert("ask".into(), Value::Object(a));
}

fn folded(state: &Dict, id: &str, lang: &str, now: f64) -> Value {
    let card = state
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id)))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    view(&card, lang, now).unwrap_or(Value::Null)
}

/// Monday = 1 … Sunday = 7, at `now`.
pub fn weekday(now: f64) -> u32 {
    local(now).weekday().number_from_monday()
}

pub fn hour(now: f64) -> u32 {
    local(now).hour()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::reads;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        let agent = crate::config::AgentConfig { name: "quotidien".into(), display_name: "Quotidien".into(), ..Default::default() };
        config.agents.insert("quotidien".into(), agent);
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        super::super::note(&config, Part::Main, "person", "Léa Martin", "who", &["Sœur".into()], &["Léa".into()], "", "", &Who::agent(&config, "quotidien")).unwrap();
        (dir, config)
    }

    /// 14:00 local on a day `days` after 2026-11-02 (a Monday).
    fn at(days: i64, hour: u32) -> f64 {
        use chrono::TimeZone;
        let d = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap() + chrono::Duration::days(days);
        chrono::Local.from_local_datetime(&d.and_hms_opt(hour, 0, 0).unwrap()).single().unwrap().timestamp() as f64
    }

    fn turn(run: &str) -> Turn {
        Turn { run: Some(run.into()), session: Some("s1".into()), ..Default::default() }
    }

    fn lea(config: &Config) -> super::super::Found {
        super::super::candidates(config, &[Part::Main], "Léa", "").into_iter().next().unwrap()
    }

    fn ask_in(config: &Config, run: &str, question: &str, chips: &[&str], now: f64) -> String {
        let t = turn(run);
        let page = lea(config);
        reads::may_read("quotidien", &t, &page.page.id());
        from_agent(config, "quotidien", &t, &page, "tastes", &json!({"question": question, "chips": chips}), false, now).unwrap()
    }

    fn card_id(config: &Config) -> String {
        let state = super::super::read_state(config);
        tk::str_or_empty(state["cards"].as_array().unwrap().last().unwrap().get("id"))
    }

    #[test]
    fn one_question_a_day_on_the_page_of_the_turn() {
        let (_d, config) = setup();
        let now = at(0, 14);
        // a page never read in the turn: no
        let page = lea(&config);
        let ask = json!({"question": "Léa chausse du combien ?", "chips": ["35", "36", "37"]});
        assert_eq!(from_agent(&config, "quotidien", &turn("r0"), &page, "tastes", &ask, false, now).unwrap(), NO);
        assert_eq!(ask_in(&config, "r1", "Léa chausse du combien ?", &["35", "36", "37"], now), "Question shown to the person under your answer.");
        let state = super::super::read_state(&config);
        let card = state["cards"][0].as_object().unwrap();
        assert_eq!((card["label"].as_str(), card["section"].as_str()), (Some("Pointure"), Some("tastes")));
        // a second the same day: no
        assert_eq!(ask_in(&config, "r2", "Quel est le plat préféré de Léa ?", &["Pizza", "Pâtes"], now + 60.0), NO);
        // a section already filled, a busy turn, a routine: no
        let next = at(1, 14);
        let t = turn("r3");
        reads::may_read("quotidien", &t, &page.page.id());
        assert_eq!(from_agent(&config, "quotidien", &t, &page, "tastes", &ask, true, next).unwrap(), NO);
        let busy = Turn { busy: true, ..turn("r4") };
        reads::may_read("quotidien", &busy, &page.page.id());
        assert_eq!(from_agent(&config, "quotidien", &busy, &page, "tastes", &ask, false, next).unwrap(), NO);
        // at quiet hours: no
        assert_eq!(ask_in(&config, "r5", "Quel est le plat préféré de Léa ?", &["Pizza", "Pâtes"], at(1, 23)), NO);
        assert_eq!(ask_in(&config, "r6", "Quel est le plat préféré de Léa ?", &["Pizza", "Pâtes"], next), "Question shown to the person under your answer.");
        let state = super::super::read_state(&config);
        assert_eq!(state["cards"][1]["label"].as_str(), Some("Plat préféré"));
    }

    #[test]
    fn health_and_money_only_when_the_person_talked_about_it() {
        let (_d, config) = setup();
        let now = at(0, 14);
        on_message("quotidien", "s1", "On parle cadeaux pour Léa", now);
        assert_eq!(ask_in(&config, "r1", "Léa a-t-elle des allergies ?", &["Oui", "Non"], now), NO);
        on_message("quotidien", "s1", "Léa est allergique, fais attention au dîner", now);
        assert_eq!(ask_in(&config, "r2", "Léa a-t-elle des allergies ?", &["Oui", "Non"], now), "Question shown to the person under your answer.");
    }

    #[test]
    fn a_tap_writes_the_fact_and_undo_takes_it_back() {
        let (_d, config) = setup();
        let now = at(0, 14);
        ask_in(&config, "r1", "Léa chausse du combien ?", &["35", "36", "37"], now);
        let id = card_id(&config);
        let open = folded(&super::super::read_state(&config), &id, "fr", now);
        assert_eq!(open["top"], "Pour ton Carnet");
        assert_eq!(open["chips"].as_array().unwrap().last().unwrap(), &json!({"id": "skip", "text": "Passer"}));
        assert_eq!(open["menu"][0]["text"], "Ne plus me demander ça");
        let done = answer(&config, &id, Answer::Choice("36".into()), "fr", now + 5.0).unwrap().unwrap();
        assert_eq!((done["state"].as_str(), done["text"].as_str(), done["undo"].as_str()), (Some("done"), Some("Noté : 36"), Some("Annuler")));
        let text = std::fs::read_to_string(super::super::root(&config).join("carnet/Personnes/Léa Martin.md")).unwrap();
        assert!(text.contains("- Pointure : 36 — *toi,"), "{text}");
        // answered: a second tap is refused with the card as it is
        assert!(matches!(answer(&config, &id, Answer::Skip, "fr", now + 6.0).unwrap(), Err(Refused::Gone(_))));
        let undone = answer(&config, &id, Answer::Undo, "fr", now + 7.0).unwrap().unwrap();
        assert_eq!(undone["state"], "open");
        let text = std::fs::read_to_string(super::super::root(&config).join("carnet/Personnes/Léa Martin.md")).unwrap();
        assert!(!text.contains("Pointure"), "{text}");
        // the next day, unanswered: « Passée »
        let later = folded(&super::super::read_state(&config), &id, "fr", at(1, 9));
        assert_eq!((later["state"].as_str(), later["text"].as_str()), (Some("skipped"), Some("Passée")));
    }

    #[test]
    fn undo_of_a_fact_already_there_reverts_nothing_else() {
        let (_d, config) = setup();
        let now = at(0, 14);
        ask_in(&config, "r1", "Léa chausse du combien ?", &["35", "36", "37"], now);
        let id = card_id(&config);
        // an agent files the same fact before the tap
        let agent = Who::agent(&config, "quotidien");
        super::super::note(&config, Part::Main, "person", "Léa Martin", "tastes", &["Pointure : 36".into()], &[], "", "", &agent).unwrap();
        let done = answer(&config, &id, Answer::Choice("36".into()), "fr", now + 5.0).unwrap().unwrap();
        assert_eq!(done["state"], "done");
        let undone = answer(&config, &id, Answer::Undo, "fr", now + 7.0).unwrap().unwrap();
        assert_eq!(undone["state"], "open");
        let text = std::fs::read_to_string(super::super::root(&config).join("carnet/Personnes/Léa Martin.md")).unwrap();
        assert!(text.contains("Pointure : 36"), "the agent's change was reverted: {text}");
    }

    #[test]
    fn passes_never_and_the_pause() {
        let (_d, config) = setup();
        let q = |n: i64| format!("Question {n} sur le jardin de Léa ?");
        for day in 0..3 {
            let now = at(day, 14);
            assert_ne!(ask_in(&config, &format!("r{day}"), &q(day), &["Oui", "Non"], now), NO, "day {day}");
            answer(&config, &card_id(&config), Answer::Skip, "fr", now + 1.0).unwrap().unwrap();
        }
        // 3 passes in a row: 14 days off
        assert_eq!(ask_in(&config, "r3", &q(3), &["Oui", "Non"], at(3, 14)), NO);
        assert_eq!(ask_in(&config, "r4", &q(4), &["Oui", "Non"], at(16, 14)), NO);
        // the same question passed: not before 30 days
        assert_eq!(ask_in(&config, "r5", &q(0), &["Oui", "Non"], at(17, 14)), NO);
        assert_ne!(ask_in(&config, "r6", &q(6), &["Oui", "Non"], at(17, 14)), NO);
        // « Ne plus me demander ça »: never again for that section of a person
        answer(&config, &card_id(&config), Answer::Never, "fr", at(17, 15)).unwrap().unwrap();
        assert_eq!(ask_in(&config, "r7", &q(7), &["Oui", "Non"], at(18, 14)), NO);
        let state = super::super::read_state(&config);
        assert_eq!(super::super::strings(state["ask"].get("declined")), vec!["person:tastes".to_string()]);
        // the setting off: never
        super::super::update_state(&config, |s| {
            s.insert("ask".into(), json!({}));
            s.insert("questions".into(), json!(false));
        })
        .unwrap();
        assert_eq!(ask_in(&config, "r8", &q(8), &["Oui", "Non"], at(19, 14)), NO);
    }

    #[test]
    fn quiet_hours_and_the_days_seen() {
        let mut state = Dict::new();
        assert!(quiet_in(&state, at(0, 23)) && quiet_in(&state, at(0, 7)) && !quiet_in(&state, at(0, 8)));
        state.insert("quiet".into(), json!({"from": "13:00", "to": "15:00"}));
        assert!(quiet_in(&state, at(0, 14)) && !quiet_in(&state, at(0, 23)));
        let (_d, config) = setup();
        forget_seen_today();
        app_seen(&config, at(0, 10));
        forget_seen_today();
        app_seen(&config, at(2, 10));
        let s = super::super::read_state(&config);
        let today = local(at(2, 10)).date_naive();
        assert_eq!(active_bits(&s, today), 0b101);
        assert_eq!(active_bits(&s, today + chrono::Duration::days(30)), 0);
    }
}
