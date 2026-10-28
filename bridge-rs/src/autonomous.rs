//! « Ce que tes agents font tout seuls » (Alex, 2026-10-28): ONE list of everything an agent does without being asked,
//! per agent and across agents, instead of separate words and screens (routines, « Tâches de fond », mail scans,
//! connectors on a schedule). The apps show it in three groups:
//!
//!     schedule    « Chaque jour / chaque semaine »: the agents' routines (Hermes' scheduled tasks, `/api/jobs`) and the
//!                 connectors' mechanical schedules (connectors_run.rs, no model call)
//!     on_change   « Quand quelque chose change »: watchers that run often and wake the agent only on a change
//!                 (planned, not built: the kind `watcher` is reserved, the group stays empty for now)
//!     now         « En cours »: background tasks and mailbox scans (jobs.rs), proposed, running, paused, or done in
//!                 the last three days
//!
//! Each item: `id` (`routine:<agent>:<job>`, `task:<id>`, `mail_scan:<agent>`, `connector:<agent>:<id>`), `kind`,
//! `group`, `agent`, `ref` (the id its own routes take), `title`, `when` ({kind, hour, minute, weekday 1 = Monday, day,
//! hours, minutes, at, text}: the apps put it in their language; `text` is the French one), `state` (`active`,
//! `paused`, `waiting` for the person, `running`, `done`), `raw_state`, `last_run` ({at, ok, error} or null),
//! `next_run` (Unix seconds or null), `cost` (null for a routine; `{ai: false}` for a connector's schedule; a task's or
//! scan's `tokens`, `max_tokens`, `eur`, `budget_eur`, which the apps turn into a share of the month's credit on the
//! offer, tokens on a self-install, as « Tâches de fond » does), `actions` and `main_action`.
//!
//! The actions go through the existing routes, by kind:
//!
//!     routine     pause / resume / run / delete   the agent's Hermes: POST /api/jobs/{ref}/pause|resume|run, DELETE
//!     task, scan  pause / resume / start / delete POST /v1/jobs/{ref}/pause|resume|start, DELETE /v1/jobs/{ref}
//!     connector   pause / resume / delete         PATCH /v1/agents/{agent}/connectors/{ref} {enabled}, DELETE
//!                 run                             POST /v1/agents/{agent}/connectors/{ref}/run (here: its schedule's
//!                                                 tool now, in the background)
//!
//!     GET  /v1/autonomous[?agent=]    {items, unreachable, budgets}   `unreachable`: agents whose Hermes didn't
//!                                                                    answer (their routines as last seen, if any)
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::TimeZone;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::AgentConfig;
use crate::connectors::{self, Connector};
use crate::connectors_run::{self as runner, When};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// A finished task or scan stays in the list this long (seconds).
pub const RECENT_SECONDS: f64 = 3.0 * 86400.0;
/// How long the list waits for one agent's Hermes.
pub const HERMES_WAIT: Duration = Duration::from_secs(6);
pub const GROUPS: [&str; 3] = ["schedule", "on_change", "now"];

const DAYS_FR: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];

/// Each agent's routines as last read, shown when its Hermes doesn't answer (asleep, restarting).
static LAST_ROUTINES: LazyLock<Mutex<HashMap<String, Vec<Dict>>>> = LazyLock::new(Default::default);

// --- when ------------------------------------------------------------------------------------------------------------

fn clock(h: u32, m: u32) -> String {
    format!("{h}:{m:02}")
}

/// The French words for a `when` (the apps use their own, from the same fields).
pub fn when_text(when: &Value) -> String {
    let int = |k: &str| when.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    match when.get("kind").and_then(Value::as_str).unwrap_or("") {
        "daily" => format!("Chaque jour à {}", clock(int("hour"), int("minute"))),
        "weekdays" => format!("En semaine à {}", clock(int("hour"), int("minute"))),
        "weekly" => {
            let day = DAYS_FR.get((int("weekday").max(1) - 1) as usize).copied().unwrap_or("lundi");
            format!("Chaque {day} à {}", clock(int("hour"), int("minute")))
        }
        "monthly" => format!("Le {} de chaque mois à {}", int("day"), clock(int("hour"), int("minute"))),
        "every_hours" if int("hours") <= 1 => "Toutes les heures".into(),
        "every_hours" => format!("Toutes les {} h", int("hours")),
        "every_minutes" => format!("Toutes les {} minutes", int("minutes")),
        "once" => "Une seule fois".into(),
        "on_change" => "Quand quelque chose change".into(),
        "now" => "En cours".into(),
        _ => when.get("raw").and_then(Value::as_str).unwrap_or("").to_string(),
    }
}

fn with_text(mut when: Value) -> Value {
    let text = when_text(&when);
    when["text"] = json!(text);
    when
}

/// A cron expression as the apps' words: « 0 8 * * * », « 30 7 * * 1-5 », « 0 8 * * 1 », « 0 9 3 * * ».
pub fn cron_when(expr: &str) -> Value {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if let [m, h, dom, "*", dow] = parts.as_slice() {
        if let (Ok(m), Ok(h)) = (m.parse::<u32>(), h.parse::<u32>()) {
            if m < 60 && h < 24 {
                let dow_upper = dow.to_uppercase();
                match (*dom, dow_upper.as_str()) {
                    ("*", "*") => return with_text(json!({"kind": "daily", "hour": h, "minute": m})),
                    ("*", "1-5") | ("*", "MON-FRI") => {
                        return with_text(json!({"kind": "weekdays", "hour": h, "minute": m}))
                    }
                    ("*", d) => {
                        if let Ok(d) = d.parse::<u32>() {
                            if d <= 7 {
                                let weekday = if d == 0 { 7 } else { d };
                                return with_text(
                                    json!({"kind": "weekly", "weekday": weekday, "hour": h, "minute": m}),
                                );
                            }
                        }
                    }
                    (d, "*") => {
                        if let Ok(d) = d.parse::<u32>() {
                            if (1..=31).contains(&d) {
                                return with_text(json!({"kind": "monthly", "day": d, "hour": h, "minute": m}));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    with_text(json!({"kind": "custom", "raw": expr}))
}

fn interval_when(minutes: u64) -> Value {
    if minutes >= 60 && minutes.is_multiple_of(60) {
        with_text(json!({"kind": "every_hours", "hours": minutes / 60}))
    } else {
        with_text(json!({"kind": "every_minutes", "minutes": minutes}))
    }
}

/// Hermes' `schedule` ({kind: cron, expr} / {kind: interval, minutes} / {kind: once, run_at}, or a plain string).
pub fn routine_when(job: &Dict) -> Value {
    let schedule = job.get("schedule").cloned().unwrap_or(Value::Null);
    if let Some(expr) = schedule.get("expr").and_then(Value::as_str) {
        return cron_when(expr);
    }
    if let Some(minutes) = schedule.get("minutes").and_then(Value::as_u64) {
        return interval_when(minutes);
    }
    if let Some(at) = schedule.get("run_at").and_then(Value::as_str) {
        return with_text(json!({"kind": "once", "at": epoch(at)}));
    }
    let text = schedule
        .as_str()
        .map(str::to_string)
        .or_else(|| job.get("schedule_display").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    if let Some(n) = text.strip_prefix("every ") {
        if let Some(m) = n.strip_suffix('m').and_then(|n| n.parse::<u64>().ok()) {
            return interval_when(m);
        }
        if let Some(h) = n.strip_suffix('h').and_then(|n| n.parse::<u64>().ok()) {
            return interval_when(h * 60);
        }
    }
    if text.split_whitespace().count() == 5 {
        return cron_when(&text);
    }
    if let Some(at) = epoch(&text) {
        return with_text(json!({"kind": "once", "at": at}));
    }
    with_text(json!({"kind": "custom", "raw": text}))
}

/// A connector's schedule (`daily 07:00`, `weekly mon 07:00`, `hourly`, `every 30 minutes`).
pub fn connector_when(when: &When) -> Value {
    match when {
        When::Hourly => interval_when(60),
        When::Every(n) => interval_when(u64::from(*n)),
        When::Daily(h, m) => with_text(json!({"kind": "daily", "hour": h, "minute": m})),
        When::Weekly(day, h, m) => {
            with_text(json!({"kind": "weekly", "weekday": day.number_from_monday(), "hour": h, "minute": m}))
        }
    }
}

// --- helpers ---------------------------------------------------------------------------------------------------------

/// An ISO time or Unix seconds → Unix seconds.
fn epoch(text: &str) -> Option<f64> {
    crate::place_move::epoch(text.trim())
}

fn seconds_of(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64().filter(|s| *s > 0.0),
        Value::String(s) => epoch(s),
        _ => None,
    }
}

fn text_of(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.trim().to_string(),
        _ => String::new(),
    }
}

fn head(text: &str, n: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= n {
        return text.to_string();
    }
    let cut: String = text.chars().take(n.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

fn main_action(state: &str, actions: &[&str]) -> &'static str {
    let wanted: &[&'static str] = match state {
        "running" | "active" => &["pause", "open"],
        "paused" => &["resume", "open"],
        "waiting" => &["start", "open"],
        _ => &["open"],
    };
    wanted.iter().copied().find(|a| actions.contains(a)).unwrap_or("open")
}

#[allow(clippy::too_many_arguments)]
fn item(
    id: String,
    kind: &str,
    group: &str,
    agent: &str,
    reference: &str,
    title: String,
    when: Value,
    state: &str,
    raw_state: &str,
    last_run: Value,
    next_run: Option<f64>,
    cost: Value,
    actions: &[&str],
    extra: Value,
) -> Value {
    json!({
        "id": id, "kind": kind, "group": group, "agent": agent, "ref": reference, "title": title, "when": when,
        "state": state, "raw_state": raw_state, "last_run": last_run, "next_run": next_run, "cost": cost,
        "actions": actions, "main_action": main_action(state, actions), "detail": extra,
    })
}

// --- the kinds -------------------------------------------------------------------------------------------------------

/// One of an agent's routines (Hermes' `/api/jobs` row).
pub fn routine_item(agent: &str, job: &Dict) -> Option<Value> {
    let job_id = text_of(job.get("id")).chars().take(80).collect::<String>();
    if job_id.is_empty() {
        return None;
    }
    let raw = text_of(job.get("state"));
    let enabled = job.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let state = match raw.as_str() {
        "running" => "running",
        "completed" | "done" => "done",
        "paused" => "paused",
        _ if !enabled => "paused",
        _ => "active",
    };
    let prompt = text_of(job.get("prompt"));
    let mut title = text_of(job.get("name"));
    if title.is_empty() || title == job_id {
        title = head(prompt.lines().next().unwrap_or(""), 60);
    }
    if title.is_empty() {
        title = "Routine".into();
    }
    let status = text_of(job.get("last_status")).to_lowercase();
    let last_run = match seconds_of(job.get("last_run_at")) {
        Some(at) => {
            let ok =
                status.is_empty() || ["ok", "success", "succeeded", "completed", "done"].contains(&status.as_str());
            let error = if ok {
                Value::Null
            } else {
                let why = text_of(job.get("last_error"));
                json!(if why.is_empty() { "Le dernier passage n'a pas abouti.".to_string() } else { head(&why, 200) })
            };
            json!({"at": at, "ok": ok, "error": error})
        }
        None => Value::Null,
    };
    let next_run = if state == "active" { seconds_of(job.get("next_run_at")) } else { None };
    let actions: &[&str] = match state {
        "paused" => &["resume", "run", "delete", "open"],
        "done" => &["delete", "open"],
        _ => &["pause", "run", "delete", "open"],
    };
    Some(item(
        format!("routine:{agent}:{job_id}"),
        "routine",
        "schedule",
        agent,
        &job_id,
        title,
        routine_when(job),
        state,
        &raw,
        last_run,
        next_run,
        Value::Null,
        actions,
        json!({"prompt": head(&prompt, 600)}),
    ))
}

/// A background task or a mailbox scan (jobs.rs' list), None when done longer ago than `RECENT_SECONDS`.
pub fn job_item(job: &Dict, now: f64) -> Option<Value> {
    let id = text_of(job.get("id"));
    let agent = text_of(job.get("agent"));
    if id.is_empty() || agent.is_empty() {
        return None;
    }
    let kind = if text_of(job.get("kind")) == "mail_scan" { "mail_scan" } else { "task" };
    let raw = text_of(job.get("state"));
    let (state, failed) = match raw.as_str() {
        "proposed" => ("waiting", false),
        "ready" => ("waiting", false),
        "running" | "stopping" | "counting" => ("running", false),
        "paused" => ("paused", false),
        "done" | "completed" => ("done", false),
        "failed" | "cancelled" | "canceled" | "interrupted" => ("done", true),
        _ => return None,
    };
    let finished = seconds_of(job.get("finished"));
    if state == "done" && finished.is_none_or(|at| now - at > RECENT_SECONDS) {
        return None;
    }
    let mut title = text_of(job.get("title"));
    if title.is_empty() {
        title = if kind == "mail_scan" { "Lecture de la boîte mail".into() } else { "Tâche".into() };
    }
    let last_run = match (state, finished) {
        ("done", Some(at)) => {
            let error = if failed {
                let why = text_of(job.get("error"));
                json!(if why.is_empty() { "Arrêtée avant la fin.".to_string() } else { head(&why, 200) })
            } else {
                Value::Null
            };
            json!({"at": at, "ok": !failed, "error": error})
        }
        _ => Value::Null,
    };
    let actions: &[&str] = match (kind, state) {
        (_, "running") => &["pause", "delete", "open"],
        (_, "paused") => &["resume", "delete", "open"],
        ("task", "waiting") => &["start", "delete", "open"],
        _ => &["delete", "open"],
    };
    let cost = json!({
        "tokens": job.get("tokens").cloned().unwrap_or(json!({"input": 0, "output": 0, "cached": 0})),
        "max_tokens": job.get("max_tokens").cloned().unwrap_or(Value::Null),
        "eur": job.get("eur").cloned().unwrap_or(Value::Null),
        "budget_eur": job.get("budget_eur").cloned().unwrap_or(Value::Null),
    });
    let when_text = match state {
        "waiting" => "Attend ton accord",
        "paused" => "En pause",
        "done" => "Terminé",
        _ => "En cours",
    };
    let extra = json!({
        "reason": job.get("reason").cloned().unwrap_or(Value::Null),
        "session_id": job.get("session_id").cloned().unwrap_or(Value::Null),
        "progress": job.get("progress").cloned().unwrap_or(Value::Null),
        "proposed_by": job.get("proposed_by").cloned().unwrap_or(Value::Null),
    });
    let id_key = if kind == "mail_scan" { format!("mail_scan:{agent}") } else { format!("task:{id}") };
    Some(item(
        id_key,
        kind,
        "now",
        &agent,
        &id,
        title,
        json!({"kind": "now", "text": when_text}),
        state,
        &raw,
        last_run,
        None,
        cost,
        actions,
        extra,
    ))
}

/// A connector with a mechanical schedule (the bridge runs it, no model call); None otherwise (a connector called
/// from a routine shows as that routine).
pub fn connector_item(agent: &str, conn: &Connector, zone: chrono_tz::Tz) -> Option<Value> {
    let schedule = conn.schedule.as_ref().filter(|s| s.mechanical && !s.tool.is_empty())?;
    let when = runner::parse_when(&schedule.when).ok()?;
    let state = match conn.status.as_str() {
        connectors::APPROVED if conn.enabled => "active",
        connectors::APPROVED => "paused",
        _ => "waiting",
    };
    let next_run = (state == "active" && conn.last_scheduled > 0.0)
        .then(|| chrono::Utc.timestamp_opt(conn.last_scheduled as i64, 0).single())
        .flatten()
        .map(|after| runner::next_after(&when, after.with_timezone(&zone)).timestamp() as f64);
    let last_run = conn
        .last_run
        .as_ref()
        .and_then(|r| {
            let at = epoch(&r.at)?;
            Some(json!({"at": at, "ok": r.ok, "error": if r.ok { Value::Null } else { json!(head(&r.message, 200)) }}))
        })
        .unwrap_or(Value::Null);
    let actions: &[&str] = match state {
        "active" => &["pause", "run", "delete", "open"],
        "paused" => &["resume", "delete", "open"],
        _ => &["delete", "open"],
    };
    let icon = connectors::icon_of(&conn.icon);
    Some(item(
        format!("connector:{agent}:{}", conn.id),
        "connector",
        "schedule",
        agent,
        &conn.id,
        conn.name.clone(),
        connector_when(&when),
        state,
        &conn.status,
        last_run,
        next_run,
        json!({"ai": false}),
        actions,
        json!({"description": head(&conn.description, 300), "tool": schedule.tool,
               "icon": {"sf": icon.get("sf").cloned().unwrap_or(Value::Null),
                        "material": icon.get("material").cloned().unwrap_or(Value::Null)}}),
    ))
}

// --- the list --------------------------------------------------------------------------------------------------------

fn rank(item: &Value) -> (usize, usize, i64) {
    let group = GROUPS.iter().position(|g| item["group"] == *g).unwrap_or(GROUPS.len());
    let state =
        ["running", "waiting", "active", "paused", "done"].iter().position(|s| item["state"] == *s).unwrap_or(5);
    let at = match item["state"].as_str() {
        Some("active") => item["next_run"].as_f64().map(|n| n as i64).unwrap_or(i64::MAX),
        _ => -item["last_run"]["at"].as_f64().map(|n| n as i64).unwrap_or(0),
    };
    (group, state, at)
}

/// Groups in order (schedule, on change, now), then running, waiting, active (soonest first), paused, done (newest).
pub fn sort_items(items: &mut [Value]) {
    items.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            a["title"].as_str().unwrap_or("").to_lowercase().cmp(&b["title"].as_str().unwrap_or("").to_lowercase())
        })
    });
}

/// An agent's routines: Ok from its Hermes now, Err with the last ones seen when it didn't answer.
async fn routines_of(state: &AppState, agent: &AgentConfig) -> Result<Vec<Dict>, Vec<Dict>> {
    let read = tokio::time::timeout(HERMES_WAIT, state.hermes.routines(agent)).await;
    let mut cache = LAST_ROUTINES.lock().unwrap_or_else(|e| e.into_inner());
    match read {
        Ok(Ok(jobs)) => {
            cache.insert(agent.name.clone(), jobs.clone());
            Ok(jobs)
        }
        _ => Err(cache.get(&agent.name).cloned().unwrap_or_default()),
    }
}

/// Everything that runs without being asked, for one agent or all.
pub async fn listing(state: &AppState, only: Option<&str>) -> (Vec<Value>, Vec<String>) {
    let config = state.config();
    let agents: Vec<AgentConfig> =
        config.agents.values().filter(|a| only.is_none_or(|name| a.name == name)).cloned().collect();
    let reads = futures::future::join_all(agents.iter().map(|a| routines_of(state, a))).await;
    let mut items = Vec::new();
    let mut unreachable = Vec::new();
    for (agent, read) in agents.iter().zip(reads) {
        let jobs = match read {
            Ok(jobs) => jobs,
            Err(last) => {
                unreachable.push(agent.name.clone());
                last
            }
        };
        items.extend(jobs.iter().filter_map(|job| routine_item(&agent.name, job)));
    }
    let now = py::time_time();
    items.extend(crate::jobs::jobs_of(state).list(only).iter().filter_map(|job| job_item(job, now)));
    let zone = crate::dates::load(&config).zone();
    for (agent, conn) in connectors::all(&config) {
        if only.is_none_or(|name| name == agent) && config.agent(Some(&agent)).is_some() {
            items.extend(connector_item(&agent, &conn, zone));
        }
    }
    sort_items(&mut items);
    (items, unreachable)
}

async fn list_route(State(state): State<AppState>, q: web::QueryMap) -> ApiResult<Response> {
    let only = match web::query_str(&q, "agent", false, Some(80))? {
        Some(id) if !id.is_empty() => Some(
            state
                .config()
                .agent(Some(&id))
                .map(|a| a.name.clone())
                .ok_or_else(|| HttpError::new(404, "unknown agent"))?,
        ),
        _ => None,
    };
    let (items, unreachable) = listing(&state, only.as_deref()).await;
    let budgets: Map<String, Value> = crate::jobs::BUDGETS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    Ok(web::ok(json!({"items": items, "unreachable": unreachable, "budgets": budgets, "groups": GROUPS})))
}

/// A connector's scheduled call now (« Lancer maintenant »), in the background: its last run says how it went.
async fn run_connector_route(
    State(state): State<AppState>,
    UrlPath((agent_id, id)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = config.agent(Some(&agent_id)).cloned().ok_or_else(|| HttpError::new(404, "no such agent"))?;
    let conn = connectors::get(&config, &agent.name, &id).ok_or_else(|| HttpError::new(404, "no such connector"))?;
    let Some(schedule) = conn.schedule.clone().filter(|s| s.mechanical && !s.tool.is_empty()) else {
        return Err(HttpError::json(409, json!({"code": "no_schedule"})));
    };
    if conn.status != connectors::APPROVED || !conn.enabled {
        return Err(HttpError::json(409, json!({"code": "not_running"})));
    }
    let arguments = schedule.arguments.as_object().cloned().unwrap_or_default();
    let started = state.clone();
    tokio::spawn(async move {
        let _ = runner::call_tool(&started, &agent, &id, &schedule.tool, arguments, "person").await;
    });
    Ok(web::ok(json!({"started": true})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/autonomous", get(list_route))
        .route("/agents/{agent_id}/connectors/{id}/run", post(run_connector_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn cron_expressions_in_words() {
        assert_eq!(cron_when("0 8 * * *")["text"], "Chaque jour à 8:00");
        assert_eq!(cron_when("30 7 * * 1-5")["kind"], "weekdays");
        let monday = cron_when("0 8 * * 1");
        assert_eq!((monday["kind"].as_str(), monday["weekday"].as_u64()), (Some("weekly"), Some(1)));
        assert_eq!(monday["text"], "Chaque lundi à 8:00");
        assert_eq!(cron_when("0 9 * * 0")["weekday"], 7);
        assert_eq!(cron_when("0 9 3 * *")["text"], "Le 3 de chaque mois à 9:00");
        let odd = cron_when("*/5 * * * *");
        assert_eq!((odd["kind"].as_str(), odd["text"].as_str()), (Some("custom"), Some("*/5 * * * *")));
    }

    #[test]
    fn hermes_schedules_and_intervals() {
        let job = dict(json!({"schedule": {"kind": "interval", "minutes": 120}}));
        assert_eq!(routine_when(&job)["text"], "Toutes les 2 h");
        let job = dict(json!({"schedule": {"kind": "interval", "minutes": 30}}));
        assert_eq!(routine_when(&job)["text"], "Toutes les 30 minutes");
        let job = dict(json!({"schedule": "every 1h"}));
        assert_eq!(routine_when(&job)["text"], "Toutes les heures");
        let job = dict(json!({"schedule": {"kind": "once", "run_at": "2026-10-30T08:00:00+00:00"}}));
        assert_eq!(routine_when(&job)["at"], 1_793_347_200.0);
        assert_eq!(connector_when(&When::Weekly(chrono::Weekday::Wed, 7, 5))["text"], "Chaque mercredi à 7:05");
    }

    #[test]
    fn a_routine_row() {
        let job = dict(json!({"id": "j1", "name": "Point du lundi", "prompt": "Fais le point", "state": "scheduled",
                              "enabled": true, "schedule": {"kind": "cron", "expr": "0 8 * * 1"},
                              "next_run_at": "2026-11-02T08:00:00+00:00", "last_run_at": "2026-10-26T08:00:00+00:00",
                              "last_status": "error", "last_error": "model timeout"}));
        let row = routine_item("lea", &job).unwrap();
        assert_eq!(row["id"], "routine:lea:j1");
        assert_eq!((row["group"].as_str(), row["state"].as_str()), (Some("schedule"), Some("active")));
        assert_eq!(row["main_action"], "pause");
        assert_eq!(row["last_run"]["ok"], false);
        assert_eq!(row["last_run"]["error"], "model timeout");
        assert!(row["next_run"].as_f64().is_some());
        let paused =
            routine_item("lea", &dict(json!({"id": "j2", "prompt": "Rappelle-moi l'eau\nsuite", "enabled": false})))
                .unwrap();
        assert_eq!((paused["state"].as_str(), paused["main_action"].as_str()), (Some("paused"), Some("resume")));
        assert_eq!(paused["title"], "Rappelle-moi l'eau");
        assert!(paused["next_run"].is_null());
    }

    #[test]
    fn jobs_rows_and_the_recent_window() {
        let now = 2_000_000_000.0;
        let proposed = job_item(
            &dict(json!({"id": "t000000000001", "agent": "lea", "kind": "task", "title": "Écoles",
                                              "state": "proposed", "max_tokens": 300000})),
            now,
        )
        .unwrap();
        assert_eq!((proposed["state"].as_str(), proposed["main_action"].as_str()), (Some("waiting"), Some("start")));
        assert_eq!(proposed["cost"]["max_tokens"], 300000);
        let old = dict(json!({"id": "t000000000002", "agent": "lea", "kind": "task", "state": "done",
                              "finished": now - 4.0 * 86400.0}));
        assert!(job_item(&old, now).is_none());
        let failed = job_item(
            &dict(json!({"id": "t000000000003", "agent": "lea", "kind": "task", "state": "failed",
                                           "error": "boom", "finished": now - 60.0})),
            now,
        )
        .unwrap();
        assert_eq!((failed["state"].as_str(), failed["last_run"]["ok"].as_bool()), (Some("done"), Some(false)));
        let scan = job_item(
            &dict(json!({"id": "mail-scan:lea", "agent": "lea", "kind": "mail_scan", "state": "running"})),
            now,
        )
        .unwrap();
        assert_eq!((scan["id"].as_str(), scan["ref"].as_str()), (Some("mail_scan:lea"), Some("mail-scan:lea")));
        assert_eq!(scan["main_action"], "pause");
        assert!(job_item(
            &dict(json!({"id": "mail-scan:lea", "agent": "lea", "kind": "mail_scan", "state": "none"})),
            now
        )
        .is_none());
    }

    #[test]
    fn connectors_only_with_a_mechanical_schedule() {
        let mut conn = Connector {
            id: "c1".into(),
            name: "Garmin".into(),
            status: connectors::APPROVED.into(),
            enabled: true,
            last_scheduled: 1_793_347_200.0,
            schedule: Some(connectors::Schedule {
                when: "daily 07:00".into(),
                mechanical: true,
                tool: "sync".into(),
                arguments: json!({}),
            }),
            ..Default::default()
        };
        let row = connector_item("lea", &conn, chrono_tz::UTC).unwrap();
        assert_eq!((row["kind"].as_str(), row["state"].as_str()), (Some("connector"), Some("active")));
        assert_eq!(row["when"]["text"], "Chaque jour à 7:00");
        assert_eq!(row["next_run"], 1_793_430_000.0); // the next day at 7:00
        assert_eq!(row["cost"]["ai"], false);
        conn.enabled = false;
        assert_eq!(connector_item("lea", &conn, chrono_tz::UTC).unwrap()["main_action"], "resume");
        conn.schedule.as_mut().unwrap().mechanical = false;
        assert!(connector_item("lea", &conn, chrono_tz::UTC).is_none());
    }

    #[test]
    fn the_order() {
        let mut items = vec![
            json!({"group": "now", "state": "running", "title": "b"}),
            json!({"group": "schedule", "state": "paused", "title": "a"}),
            json!({"group": "schedule", "state": "active", "next_run": 20.0, "title": "z"}),
            json!({"group": "schedule", "state": "active", "next_run": 10.0, "title": "y"}),
        ];
        sort_items(&mut items);
        let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
        assert_eq!(titles, ["y", "z", "a", "b"]);
    }
}
