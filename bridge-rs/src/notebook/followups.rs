//! Follow-ups, only for what matters (Carnet v2, decision 14 of internal/notes/carnet-v2/spec.md, contract.md §4): a
//! task is followed up once, and only when everything holds — the person asked for it or committed to it (origin
//! `asked` or `own`, never an agent's proposal), they insisted (`important`), it is due within 7 days, still open, and
//! no phone reminder is linked to it (the reminder does the job). One a week at most, in the day's Carnet card
//! (ask.rs: one Carnet card a day), never at quiet hours, never in a routine nor while the agent is busy. It shows at
//! the head of the next answer of the agent that wrote the task, as a Carnet card: no Boîte, no notification, and an
//! answer runs no agent. Its sentence is made without a model: « Tu voulais » + the task's title (in the infinitive,
//! as notebook_task asks) + « aujourd'hui » / « demain » / « cette semaine ».
//!
//!     POST /v1/notebook/follow/{id}   {answer: done | tomorrow | drop | never}   → the card folded (api.rs)
//!     notebook/state.json             followups {last_at, done[] (task ids), off}, cards [{card: follow…}]

use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;
use serde_json::{json, Value};

use super::ask;
use super::{Part, Task, Who};
use crate::config::Config;
use crate::{py, tools_knowledge as tk, Dict};

/// One follow-up a week at most.
const WEEK: f64 = 7.0 * 86400.0;
/// Due within this many days.
pub const WITHIN_DAYS: i64 = 7;
/// The answers of a card.
pub const ANSWERS: [&str; 4] = ["done", "tomorrow", "drop", "never"];

pub struct Texts {
    /// « Tu voulais {task} {when}. »
    pub line: &'static str,
    pub today: &'static str,
    pub tomorrow: &'static str,
    pub week: &'static str,
    pub done: &'static str,
    pub moved: &'static str,
    pub drop: &'static str,
    pub dropped: &'static str,
    pub never: &'static str,
}

pub fn texts(lang: &str) -> Texts {
    match lang {
        "fr" => Texts { line: "Tu voulais {task} {when}.", today: "aujourd'hui", tomorrow: "demain", week: "cette semaine",
                        done: "C'est fait", moved: "Demain", drop: "Laisse tomber", dropped: "Laissé tombé", never: "Ne plus me relancer" },
        "de" => Texts { line: "Du wolltest {when} {task}.", today: "heute", tomorrow: "morgen", week: "diese Woche",
                        done: "Erledigt", moved: "Morgen", drop: "Lass es", dropped: "Fallen gelassen", never: "Nicht mehr erinnern" },
        "es" => Texts { line: "Querías {task} {when}.", today: "hoy", tomorrow: "mañana", week: "esta semana",
                        done: "Hecho", moved: "Mañana", drop: "Déjalo", dropped: "Descartado", never: "No volver a recordármelo" },
        _ => Texts { line: "You wanted to {task} {when}.", today: "today", tomorrow: "tomorrow", week: "this week",
                     done: "Done", moved: "Tomorrow", drop: "Drop it", dropped: "Dropped", never: "Don't remind me again" },
    }
}

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\[([^\]|#]+)(?:[#|]([^\]]*))?\]\]").expect("valid regex"));

/// German words a task's title may start with that stay in lower case in a sentence (its nouns keep their capital).
const DE_LOWER: &[&str] = &["Den", "Die", "Das", "Dem", "Der", "Ein", "Eine", "Einen", "Mit", "Zum", "Zur", "Beim", "Am", "Im"];

/// The card's sentence (no model): « Tu voulais rappeler le plombier cette semaine. »
pub fn sentence(lang: &str, title: &str, due: &str, today: NaiveDate) -> String {
    let t = texts(lang);
    let plain = LINK.replace_all(title, |c: &regex::Captures| c.get(2).map_or(&c[1], |m| m.as_str()).to_string());
    let plain = plain.trim().trim_end_matches('.');
    let mut words = plain.splitn(2, ' ');
    let first = words.next().unwrap_or("");
    let rest = words.next().map(|r| format!(" {r}")).unwrap_or_default();
    let first = if lang == "de" && !DE_LOWER.contains(&first) {
        first.to_string()
    } else {
        let mut c = first.chars();
        c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
    };
    let when = match NaiveDate::parse_from_str(due, "%Y-%m-%d").map(|d| (d - today).num_days()) {
        Ok(0) => t.today,
        Ok(1) => t.tomorrow,
        _ => t.week,
    };
    t.line.replace("{task}", &format!("{first}{rest}")).replace("{when}", when)
}

/// Whether `task` may be followed up for `agent` on `today` (everything but the week, the slot and the hour).
fn eligible(config: &Config, state: &Dict, agent: &str, task: &Task, today: NaiveDate) -> bool {
    let shown = config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or_default();
    let by_agent = task.who == agent || (!shown.is_empty() && task.who == shown);
    let due = NaiveDate::parse_from_str(&task.due, "%Y-%m-%d").map(|d| (d - today).num_days());
    let followed = super::strings(state.get("followups").and_then(|f| f.get("done"))).contains(&task.id);
    by_agent
        && !task.done
        && !task.archived
        && task.part == Part::Main
        && !task.id.is_empty()
        && ["asked", "own"].contains(&task.origin.as_str())
        && matches!(due, Ok(d) if (0..=WITHIN_DAYS).contains(&d))
        && super::is_important(config, &task.id)
        && !super::reminder_linked(state, &task.id)
        && !followed
}

/// The person wrote to `agent` in `session` (live_brief.rs `message_notes`): a follow-up card put at the head of its
/// answer when one is due and the day allows it. `busy`: a run of the agent goes on, or one of its background tasks.
pub fn on_message(config: &Config, agent: &str, session: &str, busy: bool, now: f64) -> anyhow::Result<bool> {
    if busy || session.is_empty() || session.starts_with("cron_") || !super::root(config).is_dir() {
        return Ok(false);
    }
    let state = super::read_state(config);
    let f = state.get("followups").cloned().unwrap_or(json!({}));
    let recent = f.get("last_at").and_then(Value::as_f64).is_some_and(|t| now - t < WEEK);
    if f.get("off").map(py::truthy).unwrap_or(false) || recent || !ask::slot_free(&state, now) || ask::quiet_in(&state, now) {
        return Ok(false);
    }
    let today = ask::local(now).date_naive();
    let mut tasks: Vec<Task> =
        super::tasks(config, Part::Main).into_iter().filter(|t| eligible(config, &state, agent, t, today)).collect();
    tasks.sort_by(|a, b| a.due.cmp(&b.due));
    let Some(task) = tasks.into_iter().next() else { return Ok(false) };
    let id = format!("bip-follow-{}", py::token_hex(4));
    let card = json!({"id": id, "card": "follow", "at": now, "day": ask::day_of(now), "agent": agent, "session": session,
                      "state": "open", "task": task.id, "title": task.title, "due": task.due});
    let _guard = super::lock();
    super::update_state(config, |s| {
        ask::take_slot(s, "follow", now);
        let mut f = s.get("followups").and_then(Value::as_object).cloned().unwrap_or_default();
        f.insert("last_at".into(), json!(now));
        let mut done = super::strings(f.get("done"));
        done.push(task.id.clone());
        let over = done.len().saturating_sub(500);
        done.drain(..over);
        f.insert("done".into(), json!(done));
        s.insert("followups".into(), Value::Object(f));
        ask::push_card(s, card);
    })?;
    tracing::info!(target: "bipbridge::notebook", agent, "follow-up shown");
    Ok(true)
}

/// The open or folded view of a `follow` card (contract.md §4); None once « Ne plus me relancer ».
pub fn view(card: &Dict, lang: &str) -> Option<Value> {
    let t = texts(lang);
    let state = tk::str_or_empty(card.get("state"));
    let mut body = json!({"kind": "carnet_card", "card": "follow", "id": card.get("id"), "state": state});
    match state.as_str() {
        "open" => {
            let day = NaiveDate::parse_from_str(&tk::str_or_empty(card.get("day")), "%Y-%m-%d")
                .unwrap_or_else(|_| chrono::Local::now().date_naive());
            body["top"] = json!(ask::texts(lang).top);
            body["agent"] = card.get("agent").cloned().unwrap_or(Value::Null);
            body["text"] = json!(sentence(lang, &tk::str_or_empty(card.get("title")), &tk::str_or_empty(card.get("due")), day));
            body["chips"] = json!([{"id": "done", "text": t.done}, {"id": "tomorrow", "text": t.moved}, {"id": "drop", "text": t.drop}]);
            body["menu"] = json!([{"id": "never", "text": t.never}]);
        }
        "done" => body["text"] = json!(t.done),
        "moved" => body["text"] = json!(t.moved),
        "dropped" => body["text"] = json!(t.dropped),
        _ => return None,
    }
    Some(body)
}

/// The person's answer on card `id` (`POST /v1/notebook/follow/{id}`): « C'est fait » ticks the task, « Demain » moves
/// it a day, « Laisse tomber » archives it, « Ne plus me relancer » turns the follow-ups off. No agent runs.
pub fn answer(config: &Config, id: &str, choice: &str, lang: &str) -> anyhow::Result<Result<Value, ask::Refused>> {
    let state = super::read_state(config);
    let Some(card) = state
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id) && tk::str_or_empty(c.get("card")) == "follow"))
        .and_then(Value::as_object)
        .cloned()
    else {
        return Ok(Err(ask::Refused::Missing));
    };
    if tk::str_or_empty(card.get("state")) != "open" {
        return Ok(Err(ask::Refused::Gone(view(&card, lang).unwrap_or(Value::Null))));
    }
    let task = tk::str_or_empty(card.get("task"));
    let who = Who::you(config);
    let to = match choice {
        "done" => {
            super::set_task_done(config, &[Part::Main], &task, true, &who, false)?;
            "done"
        }
        "tomorrow" => {
            let today = chrono::Local::now().date_naive();
            let from = NaiveDate::parse_from_str(&tk::str_or_empty(card.get("due")), "%Y-%m-%d").unwrap_or(today).max(today);
            let due = (from + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();
            super::set_task_due(config, &task, &due, &who)?;
            "moved"
        }
        "drop" => {
            super::archive_task(config, &task, &who)?;
            "dropped"
        }
        _ => "never",
    };
    let _guard = super::lock();
    let after = super::update_state(config, |s| {
        if let Some(c) = ask::card_mut(s, id) {
            c.insert("state".into(), json!(to));
            // answered: what it said is not kept
            c.remove("title");
        }
        if to == "never" {
            let mut f = s.get("followups").and_then(Value::as_object).cloned().unwrap_or_default();
            f.insert("off".into(), json!(true));
            s.insert("followups".into(), Value::Object(f));
        }
    })?;
    let card = after
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find(|c| c.get("id").and_then(Value::as_str) == Some(id)))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(Ok(view(&card, lang).unwrap_or_else(|| json!({"kind": "carnet_card", "card": "follow", "id": id, "state": "never"}))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = super::super::tests_config(dir.path());
        let agent = crate::config::AgentConfig { name: "maison".into(), display_name: "Maison".into(), ..Default::default() };
        config.agents.insert("maison".into(), agent);
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        (dir, config)
    }

    fn at(days: i64, hour: u32) -> f64 {
        use chrono::TimeZone;
        let d = chrono::Local::now().date_naive() + chrono::Duration::days(days);
        chrono::Local.from_local_datetime(&d.and_hms_opt(hour, 0, 0).unwrap()).single().unwrap().timestamp() as f64
    }

    fn add(config: &Config, title: &str, days: i64, origin: &str, important: bool) -> String {
        let due = (chrono::Local::now().date_naive() + chrono::Duration::days(days)).format("%Y-%m-%d").to_string();
        let added = super::super::add_task(config, Part::Main, title, &due, "", origin, "", &Who::agent(config, "maison")).unwrap();
        if important {
            super::super::set_important(config, &added.task.id, true).unwrap();
        }
        added.task.id
    }

    fn last_card(config: &Config) -> Dict {
        let s = super::super::read_state(config);
        s["cards"].as_array().unwrap().last().unwrap().as_object().unwrap().clone()
    }

    #[test]
    fn the_sentence_without_a_model() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 9).unwrap();
        assert_eq!(sentence("fr", "Rappeler le [[Plombier]]", "2026-11-12", today), "Tu voulais rappeler le Plombier cette semaine.");
        assert_eq!(sentence("fr", "Rappeler le plombier", "2026-11-10", today), "Tu voulais rappeler le plombier demain.");
        assert_eq!(sentence("en", "Call the plumber", "2026-11-09", today), "You wanted to call the plumber today.");
        assert_eq!(sentence("de", "Den Klempner anrufen", "2026-11-12", today), "Du wolltest diese Woche den Klempner anrufen.");
        assert_eq!(sentence("es", "Llamar al fontanero", "2026-11-10", today), "Querías llamar al fontanero mañana.");
    }

    #[test]
    fn only_what_matters_once_a_week() {
        let (_d, config) = setup();
        let now = at(0, 14);
        // a task not important, one too far: nothing
        add(&config, "Acheter du pain", 2, "asked", false);
        add(&config, "Renouveler le passeport", 20, "asked", true);
        assert!(!on_message(&config, "maison", "s1", false, now).unwrap());
        let id = add(&config, "Rappeler le plombier", 3, "own", true);
        // busy, a routine, quiet hours: not now
        assert!(!on_message(&config, "maison", "s1", true, now).unwrap());
        assert!(!on_message(&config, "maison", "cron_1", false, now).unwrap());
        assert!(!on_message(&config, "maison", "s1", false, at(0, 23)).unwrap());
        // another agent: not its task
        assert!(!on_message(&config, "quotidien", "s1", false, now).unwrap());
        assert!(on_message(&config, "maison", "s1", false, now).unwrap());
        let card = last_card(&config);
        assert_eq!(card["task"].as_str(), Some(id.as_str()));
        let shown = view(&card, "fr").unwrap();
        assert_eq!(shown["text"], "Tu voulais rappeler le plombier cette semaine.");
        assert_eq!(shown["chips"][2]["text"], "Laisse tomber");
        // a second important task: not this week (and the day's card is taken)
        add(&config, "Payer la cantine", 2, "asked", true);
        assert!(!on_message(&config, "maison", "s1", false, at(1, 14)).unwrap());
        assert!(!on_message(&config, "maison", "s1", false, at(6, 14)).unwrap());
    }

    #[test]
    fn a_phone_reminder_does_the_job() {
        let (_d, config) = setup();
        let id = add(&config, "Rappeler le plombier", 3, "asked", true);
        super::super::update_state(&config, |s| {
            s.insert("reminders".into(), json!({id.clone(): {"op": "x", "part": "main", "complete": false}}));
        })
        .unwrap();
        assert!(!on_message(&config, "maison", "s1", false, at(0, 14)).unwrap());
    }

    #[test]
    fn the_answers() {
        let (_d, config) = setup();
        let id = add(&config, "Rappeler le plombier", 3, "asked", true);
        on_message(&config, "maison", "s1", false, at(0, 14)).unwrap();
        let card = tk::str_or_empty(last_card(&config).get("id"));
        let moved = answer(&config, &card, "tomorrow", "fr").unwrap().unwrap();
        assert_eq!((moved["state"].as_str(), moved["text"].as_str()), (Some("moved"), Some("Demain")));
        let task = super::super::tasks(&config, Part::Main).into_iter().find(|t| t.id == id).unwrap();
        let want = (chrono::Local::now().date_naive() + chrono::Duration::days(4)).format("%Y-%m-%d").to_string();
        assert_eq!(task.due, want);
        assert!(!last_card(&config).contains_key("title"), "answered: nothing of it kept");
        assert!(matches!(answer(&config, &card, "done", "fr").unwrap(), Err(ask::Refused::Gone(_))));
        // « Laisse tomber »: archived; « Ne plus me relancer »: off
        let other = add(&config, "Payer la cantine", 2, "asked", true);
        super::super::update_state(&config, |s| {
            s.remove("slot");
            s.remove("followups");
        })
        .unwrap();
        on_message(&config, "maison", "s2", false, at(0, 15)).unwrap();
        let card = tk::str_or_empty(last_card(&config).get("id"));
        assert_eq!(answer(&config, &card, "drop", "fr").unwrap().unwrap()["text"], "Laissé tombé");
        let task = super::super::tasks(&config, Part::Main).into_iter().find(|t| t.id == other).unwrap();
        assert!(task.archived && !task.done);
        super::super::update_state(&config, |s| {
            s.remove("slot");
            s.remove("followups");
        })
        .unwrap();
        add(&config, "Appeler la banque", 1, "asked", true);
        on_message(&config, "maison", "s3", false, at(0, 16)).unwrap();
        let card = tk::str_or_empty(last_card(&config).get("id"));
        assert_eq!(answer(&config, &card, "never", "fr").unwrap().unwrap()["state"], "never");
        add(&config, "Appeler la mairie", 1, "asked", true);
        super::super::update_state(&config, |s| {
            s.remove("slot");
        })
        .unwrap();
        assert!(!on_message(&config, "maison", "s4", false, at(8, 14)).unwrap());
    }
}
