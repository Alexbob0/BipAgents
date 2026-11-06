//! Routines on a clock, owned by the bridge (2026-11-03, dette-technique §5: « Hermes cron only a clock, or replaced
//! by the bridge's scheduler »). Hermes' cron no longer runs them: it ran each in a session of its own with another
//! prompt than the agent's conversations (no `clarify`, its « scheduled cron job » hint in the system prompt: 0 % of
//! the prompt cache, cache audit #2), and the bridge only learnt of a run after the fact by reading Hermes' cron
//! internals. Now the bridge keeps them, decides when one is due, and runs it as one background turn
//! (background_turn.rs) with the conversations' own prompt, its rules in a hidden note of its message.
//!
//! A routine: a name, what the agent does (its prompt), when (a cron expression on the routines' wall clock,
//! `dates` `routines_zone`; an interval; once at a date), maybe a number of runs, the conversation it was made in
//! (its report goes there, else to the Boîte of the main conversation), whether it is muted. Each run is a new
//! hidden conversation (« <name> · 3/11 07:00 », a `routine` thread); its report is a Boîte message (pushed),
//! `[SILENT]` keeps it quiet. Muted (Alex 5/11, the apps' « Afficher dans la discussion » off): it still runs and its
//! report stays in that run's hidden conversation (`last_session`, opened from the routine's page), but no Boîte
//! message and no push. ONE control: the « Me prévenir » that only stopped the push (`notify`) is folded into it. While the month's AI credit is used up a due run is skipped (one note per agent, credit.rs).
//!
//! The agents propose them with the bridge's tool `routine_schedule` (Hermes' `cronjob` toolset is turned off in their
//! profiles, tool_diet.rs): a proposal is off until the person's OK on its card (`bip-rout-<card>`), where they also
//! choose « M'avertir à chaque fois » or « En silence » (Alex 6/11, below). The apps make them with these routes (the
//! bridge key), in Hermes' job shape so their models stay; their form asks the same choice (`muted`):
//!
//!     GET    /v1/agents/{agent}/routines          {jobs}   (proposals waiting for their card left out)
//!     POST   /v1/agents/{agent}/routines          {name, schedule, prompt, muted?}  -> {job}   (no muted: not silent)
//!     PATCH  /v1/routines/{id}                    {name?, schedule?, prompt?, muted?} -> {job}
//!     DELETE /v1/routines/{id}
//!     POST   /v1/routines/{id}/pause | resume | run      (409 for a proposal waiting for its card)
//!     GET    /v1/cron-jobs[?agent=]  PUT /v1/cron-jobs/{agent}/{id} {notify}   the Boîte's « stop notifying »:
//!                                                 notify false = muted
//!
//! Migration: the first time the bridge sees an agent's Hermes, its jobs are read once (`GET /api/jobs`), kept here
//! with their schedule, state, conversation and mute, and deleted from Hermes.
//!
//!     <data>/routines.json (0600)   {routines: [...], migrated: [agent...]}
//!
//! No Python counterpart.

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get as get_route, patch, post, put};
use axum::Router;
use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::background_turn::{is_silent, Delivery, Kind, Pipeline, Turn, SILENT_RULE};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "routine_schedule";
/// Nothing new to tell: no message.
pub use crate::background_turn::SILENT;
/// How often the clock looks, and how long one run may take.
const TICK_SECONDS: u64 = 20;
const TURN_MAX_SECONDS: f64 = 1800.0;
/// A run missed by more than this (the bridge was down) is skipped, not caught up.
const LATE_MAX_SECONDS: f64 = 2.0 * 3600.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Schedule {
    Cron { expr: String },
    Interval { minutes: u32 },
    Once { at: f64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Routine {
    pub id: String,
    pub agent: String,
    pub name: String,
    pub prompt: String,
    pub schedule: Schedule,
    /// Skills it loads first (a model's routine, a Hermes job's).
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Its reports kept in their runs' hidden conversations: no Boîte message, no push.
    #[serde(default)]
    pub muted: bool,
    /// Read only: the « Me prévenir » off before 5/11, read as muted (`load`).
    #[serde(default = "yes", skip_serializing)]
    pub notify: bool,
    /// The conversation it was made in (its report goes there).
    #[serde(default)]
    pub origin: String,
    /// Runs left (None: for ever).
    #[serde(default)]
    pub repeat: Option<u32>,
    #[serde(default)]
    pub next_run: Option<f64>,
    #[serde(default)]
    pub last_run: Option<f64>,
    #[serde(default)]
    pub last_status: String,
    /// The last run's conversation (hidden), where its report is even when muted.
    #[serde(default)]
    pub last_session: String,
    #[serde(default)]
    pub created: f64,
    /// Proposed by its agent: waits for the person's OK on a card (`bip-rout-<card>`), off and not listed until then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<Card>,
    /// The agent's suggestion on its card (true: silent, false: tell each time); None: it gave none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_silent: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub at: f64,
}

impl Routine {
    /// Proposed by its agent, the person's answer still to come.
    pub fn waiting(&self) -> bool {
        self.card.is_some() && !self.enabled
    }
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub routines: Vec<Routine>,
    #[serde(default)]
    pub migrated: Vec<String>,
    /// The agents whose « Routines » tile is off: no `routine_schedule` for them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub off: Vec<String>,
}

static LOCK: LazyLock<Mutex<()>> = LazyLock::new(Default::default);
static RUNNING: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(Default::default);
/// When each agent's Hermes was last asked for its jobs (`migrate`).
static TRIED: LazyLock<Mutex<std::collections::HashMap<String, f64>>> = LazyLock::new(Default::default);
const MIGRATE_EVERY_SECONDS: f64 = 60.0;

fn path(config: &Config) -> std::path::PathBuf {
    std::path::Path::new(&config.data_dir()).join("routines.json")
}

pub fn load(config: &Config) -> Book {
    let mut book: Book =
        std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    for r in book.routines.iter_mut().filter(|r| !r.notify) {
        (r.muted, r.notify) = (true, true);
    }
    book
}

/// The apps' « Routines » tile, shown as Hermes' `cronjob` toolset (which the bridge keeps off for every agent):
/// whether `agent` is offered `routine_schedule`. On unless switched off.
pub fn enabled(config: &Config, agent: &str) -> bool {
    !load(config).off.iter().any(|a| a == agent)
}

pub fn set_enabled(config: &Config, agent: &str, on: bool) {
    change(config, |b| {
        b.off.retain(|a| a != agent);
        if !on {
            b.off.push(agent.to_string());
        }
    })
}

/// Read, change, write (0600), under one lock.
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Book) -> T) -> T {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut book = load(config);
    let out = f(&mut book);
    let file = path(config);
    let body = serde_json::to_string_pretty(&book).unwrap_or_default();
    if let Err(e) = crate::profile_files::write_atomic_with(&file, body.as_bytes(), 0o600) {
        tracing::warn!(file = %file.display(), error = %e, "routines not saved");
    }
    out
}

/// The routines (a proposal still waiting for the person's OK left out: it shows as its card only).
pub fn list(config: &Config, agent: Option<&str>) -> Vec<Routine> {
    all(config, agent).into_iter().filter(|r| !r.waiting()).collect()
}

/// Every routine, the proposals waiting for the person's OK too.
pub fn all(config: &Config, agent: Option<&str>) -> Vec<Routine> {
    load(config).routines.into_iter().filter(|r| agent.is_none_or(|a| r.agent == a)).collect()
}

pub fn get(config: &Config, id: &str) -> Option<Routine> {
    load(config).routines.into_iter().find(|r| r.id == id)
}

/// When each routine last ran (ISO, UTC): sleep.rs keeps a place with routines up.
pub fn last_runs(config: &Config) -> Vec<String> {
    list(config, None).iter().filter_map(|r| iso(r.last_run.or(r.next_run), Tz::UTC).as_str().map(str::to_string)).collect()
}

/// The routines' wall clock.
pub fn zone(config: &Config) -> Tz {
    crate::dates::parse_zone(&crate::dates::load(config).routines_zone()).unwrap_or(Tz::UTC)
}

// -- when -------------------------------------------------------------------------------------------------------------

const DAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Whether `value` is in a cron field (`*`, `5`, `1-5`, `*/15`, `MON-FRI`, lists of those).
fn in_field(spec: &str, value: u32, low: u32, high: u32, names: &[&str]) -> Option<bool> {
    let num = |s: &str| -> Option<u32> {
        let s = s.to_lowercase();
        s.parse().ok().or_else(|| names.iter().position(|n| s.starts_with(n)).map(|i| i as u32 + low))
    };
    let mut any = false;
    for part in spec.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => (r, s.parse::<u32>().ok().filter(|s| *s > 0)?),
            None => (part, 1),
        };
        let (from, to) = match range {
            "*" => (low, high),
            r => match r.split_once('-') {
                Some((a, b)) => (num(a)?, num(b)?),
                None if step > 1 => (num(r)?, high),
                None => (num(r)?, num(r)?),
            },
        };
        // Sunday is 0 or 7
        let v = if high == 7 && value == 0 { [0, 7] } else { [value, value] };
        any |= v.iter().any(|v| *v >= from && *v <= to && (*v - from) % step == 0);
    }
    Some(any)
}

/// Whether a cron expression (minute hour day month weekday) matches a wall-clock minute.
pub fn cron_matches(expr: &str, at: &NaiveDateTime) -> Option<bool> {
    let f: Vec<&str> = expr.split_whitespace().collect();
    if f.len() < 5 {
        return None;
    }
    let minute = in_field(f[0], at.minute(), 0, 59, &[])?;
    let hour = in_field(f[1], at.hour(), 0, 23, &[])?;
    let dom = in_field(f[2], at.day(), 1, 31, &[])?;
    let month = in_field(f[3], at.month(), 1, 12, &MONTHS)?;
    let dow = in_field(f[4], at.weekday().num_days_from_sunday(), 0, 7, &DAYS)?;
    // like cron: with both day fields restricted, either one matches
    let days = match (f[2] == "*", f[4] == "*") {
        (false, false) => dom || dow,
        _ => dom && dow,
    };
    Some(minute && hour && month && days)
}

/// The next run strictly after `after` (Unix seconds), None when there is none.
pub fn next_after(schedule: &Schedule, after: f64, zone: Tz) -> Option<f64> {
    match schedule {
        Schedule::Once { at } => (*at > after).then_some(*at),
        Schedule::Interval { minutes } => Some(after + f64::from(*minutes.max(&1)) * 60.0),
        Schedule::Cron { expr } => {
            let start = (after / 60.0).floor() as i64 * 60 + 60;
            // a year of minutes at most (« 0 0 29 2 * » waits for a leap year: none then)
            (0..366 * 24 * 60).find_map(|i| {
                let t = start + i * 60;
                let local = zone.timestamp_opt(t, 0).single()?.naive_local();
                cron_matches(expr, &local)?.then_some(t as f64)
            })
        }
    }
}

fn clock(text: &str) -> Option<(u32, u32)> {
    let t = text.trim().to_lowercase();
    let (t, pm) = match (t.strip_suffix("pm"), t.strip_suffix("am")) {
        (Some(t), _) => (t.trim().to_string(), Some(true)),
        (_, Some(t)) => (t.trim().to_string(), Some(false)),
        _ => (t, None),
    };
    let (h, m) = match t.split_once([':', 'h']) {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, if m.trim().is_empty() { 0 } else { m.trim().parse().ok()? }),
        None => (t.parse::<u32>().ok()?, 0),
    };
    let h = match pm {
        Some(true) if h < 12 => h + 12,
        Some(false) if h == 12 => 0,
        _ => h,
    };
    (h < 24 && m < 60).then_some((h, m))
}

/// What the person or the agent wrote, as Hermes took it: a cron expression, `every 2h` / `30m` (an interval),
/// `every day at 7:30` / `weekdays at 9am` / `every monday 18h`, or a date and time (once).
pub fn parse(text: &str, zone: Tz) -> Result<Schedule, String> {
    let raw = text.trim();
    let lower = raw.to_lowercase();
    let fields: Vec<&str> = raw.split_whitespace().collect();
    if fields.len() == 5 && cron_matches(raw, &NaiveDateTime::default()).is_some() {
        return Ok(Schedule::Cron { expr: raw.to_string() });
    }
    let rest = lower.strip_prefix("every ").unwrap_or(&lower).trim();
    let (days, time) = match rest.split_once(" at ").or_else(|| rest.rsplit_once(' ')) {
        Some((d, t)) if clock(t).is_some() => (d.trim(), Some(clock(t).unwrap_or_default())),
        _ => (rest, None),
    };
    if let Some((h, m)) = time {
        let dow = match days {
            "day" | "daily" | "" => Some("*".to_string()),
            "weekday" | "weekdays" => Some("1-5".into()),
            "weekend" | "weekends" => Some("0,6".into()),
            d => DAYS.iter().position(|n| d.starts_with(n)).map(|i| i.to_string()),
        };
        if let Some(dow) = dow {
            return Ok(Schedule::Cron { expr: format!("{m} {h} * * {dow}") });
        }
    }
    let unit = |s: &str| -> Option<u32> {
        let s = s.trim();
        let (n, mult) = [("min", 1), ("m", 1), ("h", 60), ("d", 1440)]
            .iter()
            .find_map(|(u, k)| s.strip_suffix(u).map(|n| (n.trim(), *k)))?;
        n.parse::<u32>().ok().filter(|n| *n > 0).map(|n| n * mult)
    };
    if let Some(minutes) = unit(rest) {
        return Ok(Schedule::Interval { minutes });
    }
    let at = chrono::DateTime::parse_from_rfc3339(raw)
        .map(|d| d.timestamp() as f64)
        .ok()
        .or_else(|| {
            ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M"].iter().find_map(|f| {
                NaiveDateTime::parse_from_str(raw, f)
                    .ok()
                    .and_then(|n| zone.from_local_datetime(&n).earliest())
                    .map(|d| d.timestamp() as f64)
            })
        });
    at.map(|at| Schedule::Once { at })
        .ok_or_else(|| format!("« {raw} » is not a schedule: give a cron expression (0 8 * * 1-5), « every 2h », « every day at 7:30 » or a date (2026-11-04T18:00)"))
}

fn iso(at: Option<f64>, zone: Tz) -> Value {
    match at.and_then(|t| zone.timestamp_opt(t as i64, 0).single()) {
        Some(d) => json!(d.to_rfc3339()),
        None => Value::Null,
    }
}

/// As Hermes showed a job (the apps' Routine models read this shape).
pub fn view(r: &Routine, zone: Tz) -> Value {
    let (schedule, display) = match &r.schedule {
        Schedule::Cron { expr } => (json!({"kind": "cron", "expr": expr, "display": expr}), expr.clone()),
        Schedule::Interval { minutes } => {
            let d = format!("every {minutes}m");
            (json!({"kind": "interval", "minutes": minutes, "display": d}), d)
        }
        Schedule::Once { at } => {
            let d = iso(Some(*at), zone);
            (json!({"kind": "once", "run_at": d, "display": d}), d.as_str().unwrap_or("").to_string())
        }
    };
    let state = if RUNNING.lock().unwrap_or_else(|e| e.into_inner()).contains(&r.id) {
        "running"
    } else if r.waiting() {
        "waiting"
    } else if !r.enabled {
        "paused"
    } else if r.next_run.is_none() {
        "completed"
    } else {
        "scheduled"
    };
    json!({"id": r.id, "agent": r.agent, "name": r.name, "prompt": r.prompt, "schedule": schedule,
           "schedule_display": display, "enabled": r.enabled, "state": state, "muted": r.muted,
           "next_run_at": iso(r.next_run, zone), "last_run_at": iso(r.last_run, zone),
           "last_status": if r.last_status.is_empty() { Value::Null } else { json!(r.last_status) },
           "origin_session_id": if r.origin.is_empty() { Value::Null } else { json!(r.origin) },
           "last_session_id": if r.last_session.is_empty() { Value::Null } else { json!(r.last_session) },
           "repeat": r.repeat, "skills": r.skills, "deliver": "local"})
}

// -- making and changing ------------------------------------------------------------------------------------------------

/// Who makes a routine, and how its reports go.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Maker {
    /// The person (in the app): on at once, silent or not as they chose.
    Person { muted: bool },
    /// Its agent (`routine_schedule`): off until the person's OK on its card, where they choose silent or not (the
    /// agent's suggestion pre-selected).
    Agent { suggested_silent: Option<bool> },
}

pub fn create(
    config: &Config,
    agent: &str,
    name: &str,
    schedule: &str,
    prompt: &str,
    origin: Option<&str>,
    by: Maker,
) -> Result<Routine, String> {
    let (name, prompt) = (name.trim(), prompt.trim());
    if prompt.is_empty() {
        return Err("a routine needs what to do (its prompt)".into());
    }
    let zone = zone(config);
    let schedule = parse(schedule, zone)?;
    let now = py::time_time();
    let proposed = matches!(by, Maker::Agent { .. });
    let next_run = if proposed { None } else { next_after(&schedule, now, zone) };
    let routine = Routine {
        id: py::uuid4_hex()[..12].to_string(),
        agent: agent.to_string(),
        name: if name.is_empty() { prompt.chars().take(40).collect() } else { name.to_string() },
        prompt: prompt.to_string(),
        repeat: matches!(schedule, Schedule::Once { .. }).then_some(1),
        schedule,
        skills: Vec::new(),
        enabled: !proposed,
        // a proposal is not silent until the person says so on its card (an older app's « Oui »: as before 6/11)
        muted: matches!(by, Maker::Person { muted: true }),
        notify: true,
        origin: origin.unwrap_or("").to_string(),
        next_run,
        last_run: None,
        last_status: String::new(),
        last_session: String::new(),
        created: now,
        card: proposed.then(|| Card { id: py::token_hex(6), at: now }),
        suggested_silent: match by {
            Maker::Agent { suggested_silent } => suggested_silent,
            Maker::Person { .. } => None,
        },
    };
    change(config, |b| b.routines.push(routine.clone()));
    Ok(routine)
}

/// Changes one routine (None: unknown); its next run computed again when its schedule or state changes.
pub fn update(config: &Config, id: &str, edit: impl FnOnce(&mut Routine) -> Result<(), String>) -> Result<Option<Routine>, String> {
    let zone = zone(config);
    change(config, |b| {
        let Some(r) = b.routines.iter_mut().find(|r| r.id == id) else { return Ok(None) };
        let before = (r.schedule.clone(), r.enabled);
        edit(r)?;
        if (r.schedule.clone(), r.enabled) != before {
            r.next_run = if r.enabled { next_after(&r.schedule, py::time_time(), zone) } else { None };
        }
        Ok(Some(r.clone()))
    })
}

pub fn delete(config: &Config, id: &str) -> bool {
    change(config, |b| {
        let n = b.routines.len();
        b.routines.retain(|r| r.id != id);
        b.routines.len() != n
    })
}

// -- the card of a routine its agent proposed ---------------------------------------------------------------------------
//
// Alex, 6/11: « Pour les routines muettes ou non, il faut demander lorsque c'est mis en place par l'agent ou
// l'utilisateur. » The card says what and when, and asks how its reports come: « M'avertir à chaque fois » or « En
// silence » (results only on the routine's page), the agent's suggestion pre-selected in the apps. Its answers
// (`POST /v1/approve`, run id `bip-rout-<card>`):
//
//     notify          on, each report in the conversation and pushed
//     silent          on, muted
//     once (« Oui »)  on, as it is: not muted, unless the app set `muted` first (PATCH /v1/routines/{id}); an app
//                     from before 6/11 shows « Oui / Non » only, and its « Oui » keeps what routines did before
//     deny            the proposal forgotten
//
// `choices` stays `["once", "deny"]` (what every app knows); `bip_choices` and `bip_routine` are for the apps that
// show the two ways.

/// The approval card of a routine an agent proposed (`/v1/approvals`, `/v1/approve`).
pub const RUN_PREFIX: &str = "bip-rout-";
pub const NOTIFY: &str = "notify";
pub const SILENT_CHOICE: &str = "silent";

fn shown_agent(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.into())
}

/// When, in French (autonomous.rs' words: the card's sentence; the apps rebuild it in theirs from `when`).
fn when_of(r: &Routine, zone: Tz) -> Value {
    let job = view(r, zone).as_object().cloned().unwrap_or_default();
    crate::autonomous::routine_when(&job)
}

/// The card's sentence: « Coach voudrait une routine « Archive du soir » : tous les jours à 23:00. »
pub fn card_sentence(config: &Config, r: &Routine) -> String {
    let when = when_of(r, zone(config));
    let when = when["text"].as_str().unwrap_or("").to_lowercase();
    let when = if when.is_empty() { String::new() } else { format!(" : {when}") };
    format!("{} voudrait une routine « {} »{when}.", shown_agent(config, &r.agent), r.name)
}

pub fn card_item(config: &Config, r: &Routine, card: &Card) -> Dict {
    let summary = card_sentence(config, r);
    let zone = zone(config);
    let suggested = match r.suggested_silent {
        Some(true) => json!(SILENT_CHOICE),
        Some(false) => json!(NOTIFY),
        None => Value::Null,
    };
    let item = json!({
        "agent": r.agent, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
        "command": format!("{TOOL_NAME} · {}", r.name), "description": summary,
        "choices": ["once", "deny"], "bip_choices": [NOTIFY, SILENT_CHOICE, "deny"],
        "created_at": card.at, "session_id": null,
        "bip_summary": summary, "bip_detail": crate::tools_knowledge::head(&r.prompt, 600),
        "bip_routine": {"id": r.id, "name": r.name, "prompt": crate::tools_knowledge::head(&r.prompt, 600),
                        "schedule_display": view(r, zone)["schedule_display"], "when": when_of(r, zone),
                        "suggested": suggested, "muted": r.muted},
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The cards waiting, oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut waiting: Vec<Routine> = all(config, agent).into_iter().filter(Routine::waiting).collect();
    waiting.sort_by(|a, b| a.card.as_ref().map(|c| c.at).unwrap_or(0.0).total_cmp(&b.card.as_ref().map(|c| c.at).unwrap_or(0.0)));
    waiting.iter().filter_map(|r| r.card.as_ref().map(|c| card_item(config, r, c))).collect()
}

/// The person answered card `card_id` with `choice` (above). The routine as it is now (on, or forgotten for `deny`),
/// None: no such card.
pub fn answer(config: &Config, agent: &str, card_id: &str, choice: &str) -> Option<Routine> {
    let zone = zone(config);
    let choice = choice.trim().to_lowercase();
    let found = change(config, |b| {
        let i = b.routines.iter().position(|r| r.agent == agent && r.waiting() && r.card.as_ref().is_some_and(|c| c.id == card_id))?;
        if !crate::card_place::allowed(&choice) {
            return Some(b.routines.remove(i));
        }
        let r = &mut b.routines[i];
        match choice.as_str() {
            SILENT_CHOICE => r.muted = true,
            NOTIFY => r.muted = false,
            _ => {} // « Oui »: as it is (not muted, unless the app said so first)
        }
        r.card = None;
        r.enabled = true;
        r.next_run = next_after(&r.schedule, py::time_time(), zone);
        Some(r.clone())
    });
    if let Some(r) = &found {
        tracing::info!(target: "bipbridge::routines", agent, card = card_id, choice = %choice, muted = r.muted, "routine card answered");
    }
    found
}

// -- the clock ----------------------------------------------------------------------------------------------------------

/// The hidden note a run reads after the routine's words: its rules travel in the message, never as run
/// instructions (the system prompt stays the conversations', and with it the prompt cache).
pub fn note(r: &Routine) -> String {
    let skills = match r.skills.as_slice() {
        [] => String::new(),
        names => format!(" Load your skill{} « {} » first (skill_view).", if names.len() > 1 { "s" } else { "" }, names.join(" », « ")),
    };
    format!(
        "{}\n\n<!--bip:routine\nThis is your routine « {} », started by its schedule: your person is not in this \
conversation. Do it on your own with your tools, then end with a short message for them in their language, written \
like a background task's report (what you did, what you found or made). If there is nothing new worth telling, \
{SILENT_RULE}.{skills}\n-->",
        r.prompt, r.name
    )
}

/// Looks for due routines every `TICK_SECONDS`; each runs apart (one at a time per routine). The first look migrates
/// the agents' Hermes jobs.
pub async fn scheduler(state: AppState) {
    loop {
        let config = state.config();
        let migrated = load(&config).migrated;
        for agent in config.agents.values().filter(|a| !migrated.contains(&a.name)) {
            // a Hermes that doesn't answer is asked again every few minutes, not at every tick
            let last = TRIED.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.name.clone(), py::time_time());
            if last.is_none_or(|at| py::time_time() - at > MIGRATE_EVERY_SECONDS) {
                migrate(&state, agent).await;
            } else {
                TRIED.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.name.clone(), last.unwrap_or_default());
            }
        }
        if crate::place_move::moving(&config) {
            // the place moving: nothing runs here; the runs missed are caught up on the new server (place_move.rs)
            tokio::time::sleep(Duration::from_secs(TICK_SECONDS)).await;
            continue;
        }
        let now = py::time_time();
        let credit = crate::credit::now_at(None).paused();
        let zone = zone(&config);
        for r in load(&config).routines {
            let due = r.enabled && r.next_run.is_some_and(|t| t <= now) && config.agent(Some(&r.agent)).is_some();
            if !due {
                continue;
            }
            let late = now - r.next_run.unwrap_or(now) > LATE_MAX_SECONDS;
            if credit || late {
                // the month's credit used, or missed while the bridge was down: skipped, the next one kept
                let next = next_after(&r.schedule, now, zone);
                let why = if credit { crate::credit::REASON } else { "missed" };
                let _ = update(&config, &r.id, |x| {
                    x.next_run = next;
                    x.last_status = why.into();
                    Ok(())
                });
                continue;
            }
            if RUNNING.lock().unwrap_or_else(|e| e.into_inner()).insert(r.id.clone()) {
                let state = state.clone();
                tokio::spawn(async move {
                    fire(&state, &r.id).await;
                    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).remove(&r.id);
                });
            }
        }
        tokio::time::sleep(Duration::from_secs(TICK_SECONDS)).await;
    }
}

/// One run of routine `id`: its turn in a new hidden conversation, its report in the Boîte of the conversation it was
/// made in (else the main one).
pub async fn fire(state: &AppState, id: &str) {
    let config = state.config();
    let Some(r) = get(&config, id) else { return };
    let Some(agent) = config.agent(Some(&r.agent)).cloned() else { return };
    let zone = zone(&config);
    let now = py::time_time();
    // taken now: a run that fails is not tried again in a loop
    let next = match r.repeat {
        Some(n) if n <= 1 => None,
        _ => next_after(&r.schedule, now, zone),
    };
    let _ = update(&config, id, |x| {
        x.next_run = next;
        x.last_run = Some(now);
        x.repeat = x.repeat.map(|n| n.saturating_sub(1));
        Ok(())
    });
    let pipeline = Pipeline::of(state);
    let when = zone.timestamp_opt(now as i64, 0).single().map(|d| d.format("%-d/%-m %H:%M").to_string());
    let title = format!("{} · {}", r.name, when.unwrap_or_default());
    let mut ran_in = String::new();
    let status = match pipeline.work_session(&agent, &title, "").await {
        Err(e) => format!("no conversation ({e})"),
        Ok(session) => {
            ran_in = session.clone();
            // the run in its own hidden thread, never at the tip of the conversation the routine was made in
            let origin = json!({"kind": "routine", "routine": r.id});
            let thread = crate::threads::threads_of(state).adopt(&agent, &session, &title, "routine", origin).await;
            let mut turn = Turn::new(&agent, Kind::Routine, thread.as_deref().unwrap_or(&session), note(&r));
            turn.max_seconds = TURN_MAX_SECONDS;
            match pipeline.run(&turn).await {
                Err(why) => why.to_string(),
                Ok(ran) if !ran.answered() => format!("ended {}", ran.status),
                Ok(ran) if is_silent(&ran.text) => "silent".into(),
                // muted: the report stays in the run's own conversation, nothing in the person's, no push
                Ok(_) if r.muted => "muted".into(),
                Ok(ran) => deliver(state, &pipeline, &agent, &r, &ran).await,
            }
        }
    };
    tracing::info!(target: "bipbridge::routines", agent = %agent.name, routine = %r.id, status = %status, "routine ran");
    crate::beta_measures::routine_delivered(&config, py::time_time());
    let _ = update(&config, id, |x| {
        x.last_status = status;
        if !ran_in.is_empty() {
            x.last_session = ran_in;
        }
        Ok(())
    });
}

/// The report where the routine was made (that conversation still there), else under the main conversation; the
/// agent finds what it said there with the person's next message.
async fn deliver(state: &AppState, pipeline: &Pipeline, agent: &AgentConfig, r: &Routine, ran: &crate::background_turn::Ran) -> String {
    let origin = match r.origin.as_str() {
        "" => None,
        o => state.hermes.session(agent, o).await.ok().map(|_| o.to_string()),
    };
    // no conversation of its own (an app's routine, a Hermes job from before 1/11): the main one, so its push opens
    // the person's conversation on the report, not the run's hidden thread (Alex 4/11 took that one for his main one)
    let home = match &origin {
        Some(o) => o.clone(),
        None => match pipeline.tip(agent, "main").await {
            tip if tip == "main" || tip.is_empty() => ran.session.clone(),
            tip => tip,
        },
    };
    let delivery = Delivery {
        agent: agent.clone(),
        session: home,
        key: format!("routine:{}:{}", r.id, ran.run_id),
        title: r.name.clone(),
        text: ran.text.clone(),
        as_written: false,
        run_id: Some(ran.run_id.clone()),
        notify: true,
        files: crate::made_files::MADE.in_run(&agent.name, &ran.run_id),
    };
    if !pipeline.deliver(&delivery).await {
        return "not delivered".into();
    }
    if let Some(origin) = origin {
        let recap = crate::handback::routine_recap(&r.name, &ran.text);
        let _ = state.briefs.keep_recap(&agent.name, &origin, &recap);
    }
    "sent".into()
}

/// The agent's Hermes jobs, read once, kept here, and deleted from Hermes (it no longer runs them). An unreachable
/// Hermes is tried again at the next look.
pub async fn migrate(state: &AppState, agent: &AgentConfig) {
    let config = state.config();
    let jobs = match state.hermes.routines(agent).await {
        Ok(jobs) => jobs,
        // a Hermes without jobs (no such route): nothing to move
        Err(crate::hermes::HermesError::Http { status: 404, .. }) => Vec::new(),
        Err(_) => return,
    };
    let zone = zone(&config);
    let muted: Vec<String> = state
        .store
        .cron_jobs(Some(&agent.name))
        .await
        .unwrap_or_default()
        .iter()
        .filter(|row| row.get("notify").and_then(Value::as_i64) == Some(0))
        .filter_map(|row| row.get("job").and_then(Value::as_str).map(str::to_string))
        .collect();
    let mut kept = Vec::new();
    for job in &jobs {
        let text = |k: &str| job.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let s = job.get("schedule").cloned().unwrap_or(Value::Null);
        let schedule = match s.get("kind").and_then(Value::as_str) {
            Some("cron") => s.get("expr").and_then(Value::as_str).map(|e| Schedule::Cron { expr: e.into() }),
            Some("interval") => s.get("minutes").and_then(Value::as_u64).map(|m| Schedule::Interval { minutes: m as u32 }),
            Some("once") => s
                .get("run_at")
                .and_then(Value::as_str)
                .and_then(|d| DateTime::parse_from_rfc3339(d).ok())
                .map(|d| Schedule::Once { at: d.timestamp() as f64 }),
            _ => s.as_str().and_then(|t| parse(t, zone).ok()),
        };
        let (Some(schedule), false) = (schedule, text("prompt").is_empty()) else {
            tracing::warn!(target: "bipbridge::routines", agent = %agent.name, job = %text("id"), "a Hermes job without a schedule or prompt: left there");
            continue;
        };
        let enabled = job.get("enabled").and_then(Value::as_bool).unwrap_or(true)
            && !matches!(text("state").as_str(), "paused" | "completed");
        let times = job.get("repeat").and_then(|r| r.get("times")).and_then(Value::as_u64);
        let done = job.get("repeat").and_then(|r| r.get("completed")).and_then(Value::as_u64).unwrap_or(0);
        let now = py::time_time();
        kept.push(Routine {
            id: text("id"),
            agent: agent.name.clone(),
            name: text("name"),
            prompt: text("prompt"),
            next_run: if enabled { next_after(&schedule, now, zone) } else { None },
            schedule,
            skills: job.get("skills").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            enabled,
            muted: muted.contains(&text("id")),
            notify: true,
            // where it was made: patch 0019's session, else an older job's api_server chat (the main conversation)
            origin: match text("origin_session_id") {
                o if o.is_empty() => job
                    .get("origin")
                    .filter(|o| o.get("platform").and_then(Value::as_str) == Some("api_server"))
                    .and_then(|o| o.get("chat_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                o => o,
            },
            repeat: times.map(|t| t.saturating_sub(done) as u32),
            last_run: DateTime::parse_from_rfc3339(&text("last_run_at")).ok().map(|d| d.timestamp() as f64),
            last_status: text("last_status"),
            last_session: String::new(),
            created: now,
            card: None,
            suggested_silent: None,
        });
    }
    let ids: Vec<String> = kept.iter().map(|r| r.id.clone()).collect();
    change(&config, |b| {
        b.routines.retain(|r| !(r.agent == agent.name && ids.contains(&r.id)));
        b.routines.extend(kept);
        b.migrated.push(agent.name.clone());
    });
    for id in &ids {
        if let Err(e) = state.hermes.delete_routine(agent, id).await {
            tracing::warn!(target: "bipbridge::routines", agent = %agent.name, job = %id, error = %e, "Hermes job not deleted: it may run once more");
        }
    }
    tracing::info!(target: "bipbridge::routines", agent = %agent.name, routines = ids.len(), "routines moved from Hermes to the bridge");
}

// -- the agent's tool ---------------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Your routines on a clock: things you do by yourself at set times (each morning, every \
weekday at 9, every 2 hours, once tomorrow at 18:00). `create` (name, schedule, prompt: what to do each time, in your \
person's words), `list`, `update` (name: the routine's; schedule and/or prompt), `pause`, `resume`, `run` (now, \
once), `delete`. A schedule: a cron expression (`30 7 * * 1-5`), `every 2h`, `every day at 7:30`, `weekdays at 9am`, \
`every monday 18h`, or a date and time (`2026-11-04T18:00`). `create` proposes it: your person turns it on with a \
card in the app, where they also choose whether each report comes to them (in the conversation, with a notification) \
or stays silent (only on the routine's page). Suggest one with `silent`: true for a background chore whose result \
they don't need to read each time (a nightly archive, a clean-up, a sync), false for what they want to read (a \
morning brief, a reminder, a digest). For something that should react to an e-mail or a link, use routine_trigger \
instead.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["create", "list", "update", "pause", "resume", "run", "delete"]},
            "name": {"type": "string", "description": "a few words, in the person's language"},
            "schedule": {"type": "string"},
            "prompt": {"type": "string"},
            "silent": {"type": "boolean", "description": "create: your suggestion, pre-selected on the card (the person \
confirms): true = results only on the routine's page, false = tell them each time"}},
            "required": ["action"]},
    })
}

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).map(py::py_str).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// What a `routine_schedule` call leaves the caller to do: a routine to start now, a card to place and push.
#[derive(Debug, Default, PartialEq)]
pub struct Called {
    pub result: Dict,
    pub run: Option<String>,
    /// (its sentence, its card id: the push's reference, as routine_trigger's)
    pub card: Option<(String, String)>,
}

/// The agent's `silent` (a boolean, or its words).
fn suggestion(arguments: &Dict) -> Option<bool> {
    match arguments.get("silent")? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "silent" | "1" => Some(true),
            "false" | "no" | "notify" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// `routine_schedule` (`origin`: the conversation the agent is in).
pub fn call(config: &Config, agent: &str, arguments: &Dict, origin: Option<&str>) -> Called {
    let zone = zone(config);
    let mine = all(config, Some(agent));
    let name = arg(arguments, "name");
    let named = mine.iter().find(|r| r.name.eq_ignore_ascii_case(&name) || r.id == name).cloned();
    let said = |r: &Routine| {
        let v = view(r, zone);
        format!("« {} » ({}), next run {}", r.name, v["schedule_display"].as_str().unwrap_or(""), v["next_run_at"])
    };
    let action = arg(arguments, "action");
    let mut card = None;
    let (text, error, run) = match (action.as_str(), named) {
        ("create", Some(r)) if r.waiting() => {
            (format!("« {} » already waits for your person's OK on its card in the app.", r.name), false, None)
        }
        ("create", _) => {
            let by = Maker::Agent { suggested_silent: suggestion(arguments) };
            match create(config, agent, &name, &arg(arguments, "schedule"), &arg(arguments, "prompt"), origin, by) {
                Ok(r) => {
                    card = r.card.as_ref().map(|c| (card_sentence(config, &r), c.id.clone()));
                    let when = view(&r, zone)["schedule_display"].as_str().unwrap_or("").to_string();
                    (format!("« {} » ({when}) waits for your person's OK: a card in the app turns it on, where they also \
choose whether its reports come to them or stay silent on its page. Tell them so, in plain words.", r.name), false, None)
                }
                Err(e) => (e, true, None),
            }
        }
        ("list", _) => {
            let lines: Vec<String> = mine
                .iter()
                .map(|r| {
                    let how = if r.waiting() {
                        ", waiting for your person's OK"
                    } else if !r.enabled {
                        ", paused"
                    } else if r.muted {
                        ", silent (reports only on its page)"
                    } else {
                        ""
                    };
                    format!("- {}{how}", said(r))
                })
                .collect();
            (if lines.is_empty() { "No routine.".into() } else { lines.join("\n") }, false, None)
        }
        (_, None) => (format!("No routine named « {name} » (list shows yours)."), true, None),
        ("delete", Some(r)) => {
            delete(config, &r.id);
            if let Some(c) = &r.card {
                crate::card_place::taken(config, &format!("{RUN_PREFIX}{}", c.id)); // its card gone with it
            }
            (format!("Routine « {} » deleted.", r.name), false, None)
        }
        (_, Some(r)) if r.waiting() => (
            format!("« {} » still waits for your person's OK on its card: nothing to {action} yet (delete it to propose \
another).", r.name),
            true,
            None,
        ),
        ("run", Some(r)) => (format!("Routine « {} » starts now.", r.name), false, Some(r.id)),
        (act @ ("pause" | "resume" | "update"), Some(r)) => {
            let (schedule, prompt) = (arg(arguments, "schedule"), arg(arguments, "prompt"));
            let done = update(config, &r.id, |x| {
                match act {
                    "pause" => x.enabled = false,
                    "resume" => x.enabled = true,
                    _ => {
                        if !schedule.is_empty() {
                            x.schedule = parse(&schedule, zone)?;
                        }
                        if !prompt.is_empty() {
                            x.prompt = prompt;
                        }
                    }
                }
                Ok(())
            });
            match done {
                Ok(Some(x)) => (format!("Routine {}.", said(&x)), false, None),
                Ok(None) => ("That routine is gone.".into(), true, None),
                Err(e) => (e, true, None),
            }
        }
        (other, _) => (format!("Unknown action « {other} »."), true, None),
    };
    Called { result: tool_text(&text, error), run, card }
}

// -- routes -------------------------------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Body {
    name: Option<String>,
    schedule: Option<String>,
    prompt: Option<String>,
    muted: Option<bool>,
    notify: Option<bool>,
}

fn parsed(body: &Bytes) -> ApiResult<Body> {
    serde_json::from_slice::<Body>(body).map_err(|e| HttpError::new(422, format!("a JSON object: {e}")))
}

fn agent_of(state: &AppState, name: &str) -> ApiResult<AgentConfig> {
    state.config().agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

fn job(config: &Config, r: &Routine) -> Response {
    web::ok(json!({"job": view(r, zone(config))}))
}

async fn listing(State(state): State<AppState>, UrlPath(agent): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&state, &agent)?;
    let zone = zone(&config);
    Ok(web::ok(json!({"jobs": list(&config, Some(&agent.name)).iter().map(|r| view(r, zone)).collect::<Vec<_>>()})))
}

async fn create_route(State(state): State<AppState>, UrlPath(agent): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&state, &agent)?;
    let b = parsed(&body)?;
    // the person chose in the form; an app from before 6/11 sends no `muted`: not silent, as before
    let by = Maker::Person { muted: b.muted.unwrap_or(false) };
    let r = create(&config, &agent.name, b.name.as_deref().unwrap_or(""), b.schedule.as_deref().unwrap_or(""), b.prompt.as_deref().unwrap_or(""), None, by)
        .map_err(|e| HttpError::new(422, e))?;
    Ok(job(&config, &r))
}

async fn patch_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let (b, zone) = (parsed(&body)?, zone(&config));
    let done = update(&config, &id, |r| {
        if let Some(s) = b.schedule.as_deref() {
            r.schedule = parse(s, zone)?;
        }
        if let Some(n) = b.name.filter(|n| !n.trim().is_empty()) {
            r.name = n.trim().into();
        }
        if let Some(p) = b.prompt.filter(|p| !p.trim().is_empty()) {
            r.prompt = p.trim().into();
        }
        if let Some(m) = b.muted {
            r.muted = m;
        }
        Ok(())
    });
    match done {
        Ok(Some(r)) => Ok(job(&config, &r)),
        Ok(None) => Err(HttpError::not_found()),
        Err(e) => Err(HttpError::new(422, e)),
    }
}

async fn delete_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    if let Some(card) = get(&config, &id).and_then(|r| r.card) {
        crate::card_place::taken(&config, &format!("{RUN_PREFIX}{}", card.id)); // a proposal: its card gone with it
    }
    if delete(&config, &id) {
        Ok(web::ok(json!({"ok": true})))
    } else {
        Err(HttpError::not_found())
    }
}

async fn action_route(State(state): State<AppState>, UrlPath((id, action)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    if get(&config, &id).is_some_and(|r| r.waiting()) {
        // a proposal: its card turns it on (where its reports' way is chosen)
        return Err(HttpError::new(409, "this routine waits for the answer on its card"));
    }
    let enabled = match action.as_str() {
        "pause" => false,
        "resume" => true,
        "run" => {
            get(&config, &id).ok_or_else(HttpError::not_found)?;
            let (state, id) = (state.clone(), id.clone());
            tokio::spawn(async move { fire(&state, &id).await });
            return Ok(web::ok(json!({"ok": true})));
        }
        _ => return Err(HttpError::not_found()),
    };
    let r = update(&config, &id, |r| {
        r.enabled = enabled;
        Ok(())
    })
    .map_err(|e| HttpError::new(422, e))?
    .ok_or_else(HttpError::not_found)?;
    Ok(job(&config, &r))
}

/// The routines as the Boîte's « stop notifying » sees them: `notify` = not muted.
async fn mutes(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let agent = web::query_str(&q, "agent", false, Some(200))?.filter(|a| !a.is_empty());
    if let Some(a) = &agent {
        agent_of(&state, a)?;
    }
    let rows: Vec<Value> = list(&config, agent.as_deref())
        .iter()
        .map(|r| json!({"agent": r.agent, "job": r.id, "name": r.name, "notify": !r.muted, "last_seen": iso(r.last_run, Tz::UTC)}))
        .collect();
    Ok(web::ok(json!({"jobs": rows})))
}

/// The Boîte's « stop notifying » on a report (`notify: false`): the routine muted, as its page's switch does.
async fn set_mute(State(state): State<AppState>, UrlPath((agent, id)): UrlPath<(String, String)>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    agent_of(&state, &agent)?;
    get(&config, &id).ok_or_else(|| HttpError::new(404, "unknown job"))?;
    let notify = parsed(&body)?.notify.ok_or_else(|| HttpError::new(422, "notify is required"))?;
    let r = update(&config, &id, |r| {
        r.muted = !notify;
        Ok(())
    })
    .map_err(|e| HttpError::new(422, e))?
    .ok_or_else(|| HttpError::new(404, "unknown job"))?;
    Ok(web::ok(json!({"agent": r.agent, "job": r.id, "notify": !r.muted})))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent}/routines", get_route(listing).post(create_route))
        .route("/routines/{id}", patch(patch_route).delete(delete_route))
        .route("/routines/{id}/{action}", post(action_route))
        .route("/cron-jobs", get_route(mutes))
        .route("/cron-jobs/{agent}/{job}", put(set_mute))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> f64 {
        Tz::Europe__Paris.from_local_datetime(&NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M").unwrap()).unwrap().timestamp() as f64
    }

    #[test]
    fn schedules_as_people_and_hermes_write_them() {
        let z = Tz::Europe__Paris;
        let cron = |e: &str| Schedule::Cron { expr: e.into() };
        assert_eq!(parse("0 8 * * 1-5", z), Ok(cron("0 8 * * 1-5")));
        assert_eq!(parse("every day at 7:30", z), Ok(cron("30 7 * * *")));
        assert_eq!(parse("weekdays at 9am", z), Ok(cron("0 9 * * 1-5")));
        assert_eq!(parse("every monday 18h", z), Ok(cron("0 18 * * 1")));
        assert_eq!(parse("every 2h", z), Ok(Schedule::Interval { minutes: 120 }));
        assert_eq!(parse("30m", z), Ok(Schedule::Interval { minutes: 30 }));
        assert_eq!(parse("2026-11-04T18:00", z), Ok(Schedule::Once { at: at("2026-11-04 18:00") }));
        assert!(parse("demain peut-être", z).is_err());
    }

    #[test]
    fn the_next_run_on_the_wall_clock() {
        let z = Tz::Europe__Paris;
        let weekdays = Schedule::Cron { expr: "30 7 * * MON-FRI".into() };
        // Friday 8:00 → Monday 7:30
        assert_eq!(next_after(&weekdays, at("2026-11-06 08:00"), z), Some(at("2026-11-09 07:30")));
        // across the change to winter time (25 October 2026): still 7:30 on the clock
        let daily = Schedule::Cron { expr: "30 7 * * *".into() };
        assert_eq!(next_after(&daily, at("2026-10-24 12:00"), z), Some(at("2026-10-25 07:30")));
        assert_eq!(next_after(&Schedule::Cron { expr: "*/15 * * * *".into() }, at("2026-11-03 10:01"), z), Some(at("2026-11-03 10:15")));
        let once = Schedule::Once { at: at("2026-11-04 18:00") };
        assert_eq!(next_after(&once, at("2026-11-03 10:00"), z), Some(at("2026-11-04 18:00")));
        assert_eq!(next_after(&once, at("2026-11-05 10:00"), z), None);
        assert_eq!(next_after(&Schedule::Interval { minutes: 90 }, 1000.0, z), Some(1000.0 + 5400.0));
        // with both day fields given, either one (as cron)
        let both = Schedule::Cron { expr: "0 9 1 * 1".into() };
        assert_eq!(next_after(&both, at("2026-11-01 10:00"), z), Some(at("2026-11-02 09:00")));
    }

    #[test]
    fn its_rules_travel_in_the_message() {
        let r = Routine {
            id: "r1".into(),
            agent: "vie".into(),
            name: "Météo".into(),
            prompt: "Dis-moi la météo.".into(),
            schedule: Schedule::Interval { minutes: 60 },
            skills: Vec::new(),
            enabled: true,
            muted: false,
            notify: true,
            origin: String::new(),
            repeat: None,
            next_run: None,
            last_run: None,
            last_status: String::new(),
            last_session: String::new(),
            created: 0.0,
            card: None,
            suggested_silent: None,
        };
        let n = note(&r);
        assert!(n.starts_with("Dis-moi la météo.\n\n<!--bip:routine\n") && n.contains(SILENT) && n.ends_with("-->"));
        let v = view(&r, Tz::UTC);
        assert_eq!((v["state"].as_str(), v["schedule"]["minutes"].as_u64()), (Some("completed"), Some(60)));
    }

    #[test]
    fn muted_is_one_switch_and_the_old_notify_off_reads_as_muted() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        // a routine whose « Me prévenir » was off before 5/11: muted, and written back without `notify`
        std::fs::write(path(&config), r#"{"routines": [{"id": "r1", "agent": "vie", "name": "Météo", "prompt": "x",
            "schedule": {"kind": "interval", "minutes": 60}, "notify": false}, {"id": "r2", "agent": "vie",
            "name": "Agenda", "prompt": "y", "schedule": {"kind": "interval", "minutes": 60}}]}"#).unwrap();
        let muted: Vec<bool> = list(&config, Some("vie")).iter().map(|r| r.muted).collect();
        assert_eq!(muted, [true, false]);
        update(&config, "r2", |r| {
            r.muted = true;
            Ok(())
        })
        .unwrap();
        let written = std::fs::read_to_string(path(&config)).unwrap();
        assert!(!written.contains("notify") && written.matches("\"muted\": true").count() == 2, "{written}");
        let v = view(&get(&config, "r1").unwrap(), Tz::UTC);
        assert_eq!((v["muted"].as_bool(), v["last_session_id"].is_null()), (Some(true), true));
    }

    fn config_in(dir: &tempfile::TempDir) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config
    }

    fn args(v: Value) -> Dict {
        v.as_object().cloned().unwrap_or_default()
    }

    fn propose(config: &Config, name: &str, silent: Option<bool>) -> (Called, Dict) {
        let mut a = json!({"action": "create", "name": name, "schedule": "every day at 23:00", "prompt": "Archive."});
        if let Some(s) = silent {
            a["silent"] = json!(s);
        }
        let called = call(config, "vie", &args(a), Some("api_c"));
        let card = pending_items(config, Some("vie")).into_iter().find(|c| c["bip_routine"]["name"] == name).unwrap();
        (called, card)
    }

    fn card_id(card: &Dict) -> String {
        card["request_id"].as_str().unwrap().to_string()
    }

    #[test]
    fn an_agents_routine_waits_for_its_card_with_both_ways_and_its_suggestion() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let (called, card) = propose(&config, "Archive du soir", Some(true));
        assert!(called.run.is_none() && called.result.get("isError") == Some(&json!(false)));
        let (_, pushed) = called.card.clone().unwrap();
        assert_eq!(pushed, card_id(&card));
        assert_eq!(card["run_id"], json!(format!("{RUN_PREFIX}{pushed}")));
        assert_eq!((card["choices"].clone(), card["bip_choices"].clone()), (json!(["once", "deny"]), json!(["notify", "silent", "deny"])));
        assert_eq!((card["bip_routine"]["suggested"].as_str(), card["bip_routine"]["muted"].as_bool()), (Some("silent"), Some(false)));
        assert_eq!((card["bip_routine"]["when"]["kind"].as_str(), card["bip_routine"]["when"]["hour"].as_u64()), (Some("daily"), Some(23)));
        assert!(card["bip_summary"].as_str().unwrap().contains("voudrait une routine « Archive du soir »"), "{card:?}");
        // off, not listed, nothing due
        let r = all(&config, Some("vie")).pop().unwrap();
        assert!(r.waiting() && r.next_run.is_none() && !r.muted && r.origin == "api_c");
        assert!(list(&config, Some("vie")).is_empty());
        assert_eq!(view(&r, Tz::UTC)["state"], "waiting");
        // the agent can't turn it on itself, nor propose it twice
        let resumed = call(&config, "vie", &args(json!({"action": "resume", "name": "Archive du soir"})), None);
        assert_eq!(resumed.result.get("isError"), Some(&json!(true)));
        let twice = call(&config, "vie", &args(json!({"action": "create", "name": "Archive du soir", "schedule": "every 2h", "prompt": "x"})), None);
        assert!(twice.card.is_none() && pending_items(&config, None).len() == 1);
        // suggestions: false, none, words
        assert_eq!(propose(&config, "Brief", Some(false)).1["bip_routine"]["suggested"], "notify");
        assert!(propose(&config, "Sans avis", None).1["bip_routine"]["suggested"].is_null());
        assert_eq!(suggestion(&args(json!({"silent": "yes"}))), Some(true));
        assert_eq!(suggestion(&args(json!({"silent": "maybe"}))), None);
    }

    #[test]
    fn each_answer_gives_its_mute() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let answered = |name: &str, silent: Option<bool>, choice: &str| {
            let (_, card) = propose(&config, name, silent);
            answer(&config, "vie", &card_id(&card), choice)
        };
        let silent = answered("A", Some(false), "silent").unwrap();
        assert!(silent.muted && silent.enabled && silent.card.is_none() && silent.next_run.is_some());
        assert!(!answered("B", Some(true), "notify").unwrap().muted);
        // an older app's « Oui »: not silent, as before 6/11, whatever the suggestion
        assert!(!answered("C", Some(true), "once").unwrap().muted);
        // the newer iPhone sets the way first, then « Oui »
        let (_, card) = propose(&config, "D", None);
        let id = card["bip_routine"]["id"].as_str().unwrap().to_string();
        update(&config, &id, |r| {
            r.muted = true;
            Ok(())
        })
        .unwrap();
        assert!(answer(&config, "vie", &card_id(&card), "once").unwrap().muted);
        // « Non »: forgotten; answered twice or by another agent: no such card
        let (_, card) = propose(&config, "E", None);
        assert_eq!(answer(&config, "autre", &card_id(&card), "once"), None);
        assert!(answer(&config, "vie", &card_id(&card), "deny").is_some());
        assert_eq!(answer(&config, "vie", &card_id(&card), "once"), None);
        let names: Vec<String> = list(&config, Some("vie")).iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, ["A", "B", "C", "D"]);
        assert!(pending_items(&config, None).is_empty());
    }

    #[test]
    fn the_persons_routine_is_on_at_once_as_they_chose() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        for muted in [true, false] {
            let r = create(&config, "vie", "Météo", "every day at 7:30", "Météo.", None, Maker::Person { muted }).unwrap();
            assert!(r.enabled && r.card.is_none() && r.next_run.is_some() && r.muted == muted);
        }
        assert!(pending_items(&config, None).is_empty());
        // the tool's description asks the agent for its suggestion
        let t = tool();
        assert_eq!(t["inputSchema"]["properties"]["silent"]["type"], "boolean");
        let d = t["description"].as_str().unwrap();
        assert!(d.contains("background chore") && d.contains("morning brief"), "{d}");
    }
}
