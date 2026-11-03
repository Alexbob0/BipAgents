//! Routines on a clock, owned by the bridge (2026-11-03, dette-technique §5: « Hermes cron only a clock, or replaced
//! by the bridge's scheduler »). Hermes' cron no longer runs them: it ran each in a session of its own with another
//! prompt than the agent's conversations (no `clarify`, its « scheduled cron job » hint in the system prompt: 0 % of
//! the prompt cache, cache audit #2), and the bridge only learnt of a run after the fact by reading Hermes' cron
//! internals. Now the bridge keeps them, decides when one is due, and runs it as one background turn
//! (background_turn.rs) with the conversations' own prompt, its rules in a hidden note of its message.
//!
//! A routine: a name, what the agent does (its prompt), when (a cron expression on the routines' wall clock,
//! `dates` `routines_zone`; an interval; once at a date), maybe a number of runs, the conversation it was made in
//! (its report goes there, else to the Boîte of the main conversation), whether it notifies. Each run is a new
//! hidden conversation (« <name> · 3/11 07:00 », a `routine` thread); its report is a Boîte message, `[SILENT]`
//! keeps it quiet. While the month's AI credit is used up a due run is skipped (one note per agent, credit.rs).
//!
//! The agents make them with the bridge's tool `routine_schedule` (Hermes' `cronjob` toolset is turned off in their
//! profiles, tool_diet.rs); the apps with these routes (the bridge key), in Hermes' job shape so their models stay:
//!
//!     GET    /v1/agents/{agent}/routines          {jobs}
//!     POST   /v1/agents/{agent}/routines          {name, schedule, prompt}  -> {job}
//!     PATCH  /v1/routines/{id}                    {name?, schedule?, prompt?} -> {job}
//!     DELETE /v1/routines/{id}
//!     POST   /v1/routines/{id}/pause | resume | run
//!     GET    /v1/cron-jobs[?agent=]  PUT /v1/cron-jobs/{agent}/{id} {notify}   the « Me prévenir » of each one
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
use crate::background_turn::{Delivery, Kind, Pipeline, Turn};
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "routine_schedule";
/// Nothing new to tell: no message.
pub const SILENT: &str = "[SILENT]";
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
    #[serde(default = "yes")]
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
    #[serde(default)]
    pub created: f64,
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
}

static LOCK: LazyLock<Mutex<()>> = LazyLock::new(Default::default);
static RUNNING: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(Default::default);

fn path(config: &Config) -> std::path::PathBuf {
    std::path::Path::new(&config.data_dir()).join("routines.json")
}

pub fn load(config: &Config) -> Book {
    std::fs::read_to_string(path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
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

pub fn list(config: &Config, agent: Option<&str>) -> Vec<Routine> {
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
    } else if !r.enabled {
        "paused"
    } else if r.next_run.is_none() {
        "completed"
    } else {
        "scheduled"
    };
    json!({"id": r.id, "agent": r.agent, "name": r.name, "prompt": r.prompt, "schedule": schedule,
           "schedule_display": display, "enabled": r.enabled, "state": state, "notify": r.notify,
           "next_run_at": iso(r.next_run, zone), "last_run_at": iso(r.last_run, zone),
           "last_status": if r.last_status.is_empty() { Value::Null } else { json!(r.last_status) },
           "origin_session_id": if r.origin.is_empty() { Value::Null } else { json!(r.origin) },
           "repeat": r.repeat, "deliver": "local"})
}

// -- making and changing ------------------------------------------------------------------------------------------------

pub fn create(
    config: &Config,
    agent: &str,
    name: &str,
    schedule: &str,
    prompt: &str,
    origin: Option<&str>,
) -> Result<Routine, String> {
    let (name, prompt) = (name.trim(), prompt.trim());
    if prompt.is_empty() {
        return Err("a routine needs what to do (its prompt)".into());
    }
    let zone = zone(config);
    let schedule = parse(schedule, zone)?;
    let now = py::time_time();
    let next_run = next_after(&schedule, now, zone);
    let routine = Routine {
        id: py::uuid4_hex()[..12].to_string(),
        agent: agent.to_string(),
        name: if name.is_empty() { prompt.chars().take(40).collect() } else { name.to_string() },
        prompt: prompt.to_string(),
        repeat: matches!(schedule, Schedule::Once { .. }).then_some(1),
        schedule,
        skills: Vec::new(),
        enabled: true,
        notify: true,
        origin: origin.unwrap_or("").to_string(),
        next_run,
        last_run: None,
        last_status: String::new(),
        created: now,
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
like a background task's report (what you did, what you found or made). If there is nothing new worth telling, answer \
exactly {SILENT}.{skills}\n-->",
        r.prompt, r.name
    )
}

/// Looks for due routines every `TICK_SECONDS`; each runs apart (one at a time per routine). The first look migrates
/// the agents' Hermes jobs.
pub async fn scheduler(state: AppState) {
    loop {
        let config = state.config();
        for agent in config.agents.values() {
            if !load(&config).migrated.contains(&agent.name) {
                migrate(&state, agent).await;
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
    let status = match pipeline.work_session(&agent, &title, "").await {
        Err(e) => format!("no conversation ({e})"),
        Ok(session) => {
            crate::threads::threads_of(state).adopt(&agent, &session, "routine", json!({"kind": "routine", "routine": r.id})).await;
            let mut turn = Turn::new(&agent, Kind::Routine, &session, note(&r));
            turn.max_seconds = TURN_MAX_SECONDS;
            match pipeline.run(&turn).await {
                Err(why) => why.to_string(),
                Ok(ran) if !ran.answered() => format!("ended {}", ran.status),
                Ok(ran) if ran.text.trim().starts_with(SILENT) => "silent".into(),
                Ok(ran) => deliver(state, &pipeline, &agent, &r, &ran).await,
            }
        }
    };
    tracing::info!(target: "bipbridge::routines", agent = %agent.name, routine = %r.id, status = %status, "routine ran");
    crate::beta_measures::routine_delivered(&config, py::time_time());
    let _ = update(&config, id, |x| {
        x.last_status = status;
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
    let delivery = Delivery {
        agent: agent.clone(),
        session: origin.clone().unwrap_or_else(|| ran.session.clone()),
        key: format!("routine:{}:{}", r.id, ran.run_id),
        title: r.name.clone(),
        text: ran.text.clone(),
        as_written: false,
        run_id: Some(ran.run_id.clone()),
        notify: r.notify,
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
    let Ok(jobs) = state.hermes.routines(agent).await else { return };
    let config = state.config();
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
            notify: !muted.contains(&text("id")),
            origin: text("origin_session_id"),
            repeat: times.map(|t| t.saturating_sub(done) as u32),
            last_run: None,
            last_status: String::new(),
            created: now,
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
`every monday 18h`, or a date and time (`2026-11-04T18:00`). Its report goes where your person asked for it. For \
something that should react to an e-mail or a link, use routine_trigger instead.",
        "inputSchema": {"type": "object", "properties": {
            "action": {"type": "string", "enum": ["create", "list", "update", "pause", "resume", "run", "delete"]},
            "name": {"type": "string", "description": "a few words, in the person's language"},
            "schedule": {"type": "string"},
            "prompt": {"type": "string"}},
            "required": ["action"]},
    })
}

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).map(py::py_str).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// `routine_schedule` (`origin`: the conversation the agent is in). `run` is started by the caller.
pub fn call(config: &Config, agent: &str, arguments: &Dict, origin: Option<&str>) -> (Dict, Option<String>) {
    let zone = zone(config);
    let mine = list(config, Some(agent));
    let name = arg(arguments, "name");
    let named = mine.iter().find(|r| r.name.eq_ignore_ascii_case(&name) || r.id == name).cloned();
    let said = |r: &Routine| {
        let v = view(r, zone);
        format!("« {} » ({}), next run {}", r.name, v["schedule_display"].as_str().unwrap_or(""), v["next_run_at"])
    };
    let action = arg(arguments, "action");
    let (text, error, run) = match (action.as_str(), named) {
        ("create", _) => match create(config, agent, &name, &arg(arguments, "schedule"), &arg(arguments, "prompt"), origin) {
            Ok(r) => (format!("Routine made: {}.", said(&r)), false, None),
            Err(e) => (e, true, None),
        },
        ("list", _) => {
            let lines: Vec<String> = mine.iter().map(|r| format!("- {}{}", said(r), if r.enabled { "" } else { ", paused" })).collect();
            (if lines.is_empty() { "No routine.".into() } else { lines.join("\n") }, false, None)
        }
        (_, None) => (format!("No routine named « {name} » (list shows yours)."), true, None),
        ("delete", Some(r)) => {
            delete(config, &r.id);
            (format!("Routine « {} » deleted.", r.name), false, None)
        }
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
    (tool_text(&text, error), run)
}

// -- routes -------------------------------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct Body {
    name: Option<String>,
    schedule: Option<String>,
    prompt: Option<String>,
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
    let r = create(&config, &agent.name, b.name.as_deref().unwrap_or(""), b.schedule.as_deref().unwrap_or(""), b.prompt.as_deref().unwrap_or(""), None)
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
        if let Some(n) = b.notify {
            r.notify = n;
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
    if delete(&state.config(), &id) {
        Ok(web::ok(json!({"ok": true})))
    } else {
        Err(HttpError::not_found())
    }
}

async fn action_route(State(state): State<AppState>, UrlPath((id, action)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
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

/// The routines' « Me prévenir », as the apps' list of scheduled tasks reads it.
async fn mutes(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let agent = web::query_str(&q, "agent", false, Some(200))?.filter(|a| !a.is_empty());
    let rows: Vec<Value> = list(&config, agent.as_deref())
        .iter()
        .map(|r| json!({"agent": r.agent, "job": r.id, "name": r.name, "notify": r.notify, "last_seen": iso(r.last_run, Tz::UTC)}))
        .collect();
    Ok(web::ok(json!({"jobs": rows})))
}

async fn set_mute(State(state): State<AppState>, UrlPath((_agent, id)): UrlPath<(String, String)>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    get(&config, &id).ok_or_else(|| HttpError::new(404, "unknown job"))?;
    let notify = parsed(&body)?.notify.ok_or_else(|| HttpError::new(422, "notify is required"))?;
    let r = update(&config, &id, |r| {
        r.notify = notify;
        Ok(())
    })
    .map_err(|e| HttpError::new(422, e))?
    .ok_or_else(|| HttpError::new(404, "unknown job"))?;
    Ok(web::ok(json!({"agent": r.agent, "job": r.id, "notify": r.notify})))
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
            notify: true,
            origin: String::new(),
            repeat: None,
            next_run: None,
            last_run: None,
            last_status: String::new(),
            created: 0.0,
        };
        let n = note(&r);
        assert!(n.starts_with("Dis-moi la météo.\n\n<!--bip:routine\n") && n.contains(SILENT) && n.ends_with("-->"));
        let v = view(&r, Tz::UTC);
        assert_eq!((v["state"].as_str(), v["schedule"]["minutes"].as_u64()), (Some("completed"), Some(60)));
    }
}
