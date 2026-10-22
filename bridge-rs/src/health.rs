//! The person's health data, for the agents they choose (decided 2026-10-21): their phone reads it with the system's
//! permission (Apple Health on the iPhone, where the Apple Watch's data lands too; Health Connect on Android) and
//! sends daily summaries here. It stays in this place (one file per day, at most `KEPT_DAYS`), never goes anywhere
//! else, and goes when the last agent's « Santé » switch is turned off. An agent with the switch on gets the `health`
//! tool and a rule in its instructions (souls::HEALTH_RULE): help with sleep, activity and training, never diagnose.
//!
//! What each agent reads is its own choice of categories (2026-10-22, `CATEGORIES`): activity, sleep, heart, body,
//! food and water, well-being, and the more sensitive blood glucose and cycle, off unless the person turns them on.
//! A new switch starts with `DEFAULT_CATEGORIES`. The phone sends, for this place, the figures of the categories some
//! agent here reads (`shared` in the status), never more; the bridge keeps only those (a category no agent reads any
//! more is wiped from the days kept) and serves each agent only its own categories: the tool reads each day through
//! `Day::restricted`, whatever else the file holds.
//!
//!     health    the person's days for a period: summaries by default, or one line per day
//!
//! App routes (the bridge key):
//!
//!     GET /v1/health-data         {agents, categories: {agent: [..]}, shared: [..], days, first, last, at}
//!     PUT /v1/health-data/days    {days: [Day]}: the days that changed (409 when no agent has it on)
//!
//! The switch and the categories are agent settings (`PATCH /v1/agents/{id}/settings {"health": bool,
//! "health_categories": [..]}`, agents_admin.rs).
//!
//!     data_dir/health/agents.json          (chmod 600: the agents allowed, their categories, the last upload)
//!     data_dir/health/days/<date>.json     (chmod 600: one day)
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use chrono::{Duration, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

static LOCK: Mutex<()> = Mutex::new(());
/// Days kept (a little more than a year, to compare with the same season).
pub const KEPT_DAYS: usize = 400;
/// Days one upload may carry.
pub const MAX_UPLOAD: usize = 400;
pub const MAX_WORKOUTS: usize = 50;
/// A period the tool reads at most, and by default.
pub const DEFAULT_DAYS: i64 = 7;
pub const TOOL_NAME: &str = "health";

/// What an agent may read, in the apps' order. The apps show the same list (« Activité », « Sommeil », « Cœur »,
/// « Corps », « Alimentation et eau », « Bien-être », « Glycémie », « Cycle »).
pub const CATEGORIES: [&str; 8] = ["activity", "sleep", "heart", "body", "food", "wellbeing", "glucose", "cycle"];
/// Off unless the person turns them on, one by one (« Tout partager » leaves them out).
pub const SENSITIVE: [&str; 2] = ["glucose", "cycle"];
/// A switch turned on: these.
pub const DEFAULT_CATEGORIES: [&str; 3] = ["activity", "sleep", "heart"];
/// An agent whose switch was on before the categories (2026-10-22): what it read then (weight included).
const EARLIER_CATEGORIES: [&str; 4] = ["activity", "sleep", "heart", "body"];
/// The tool's former `metrics`, still understood: the category each one is in.
const OLD_METRICS: [(&str, &str); 7] = [
    ("sleep", "sleep"),
    ("steps", "activity"),
    ("energy", "activity"),
    ("exercise", "activity"),
    ("workouts", "activity"),
    ("heart", "heart"),
    ("weight", "body"),
];

/// What each category holds, as the tool's description says it.
fn category_words(category: &str) -> &'static str {
    match category {
        "activity" => {
            "activity (steps, active energy, exercise minutes, workouts, walking and running distance, flights \
climbed, stand hours)"
        }
        "sleep" => "sleep (time asleep and in bed, sleep stages, breathing rate and wrist temperature at night)",
        "heart" => "heart (resting heart rate, heart rate variability, the day's heart rate, blood oxygen, VO2 max)",
        "body" => "body (weight, body fat, BMI, blood pressure)",
        "food" => "food and water (food energy eaten, water, caffeine)",
        "wellbeing" => "well-being (mindful minutes, mood logs)",
        "glucose" => "blood glucose (mmol/L)",
        "cycle" => "menstrual cycle (flow, period starts)",
        _ => "",
    }
}

/// The known categories among `names`, once each, in `CATEGORIES`' order.
pub fn known_categories<S: AsRef<str>>(names: &[S]) -> Vec<&'static str> {
    CATEGORIES.iter().copied().filter(|c| names.iter().any(|n| n.as_ref() == *c)).collect()
}

/// The `health` tool's definition for an agent reading `categories` (listed only for an agent whose switch is on).
pub fn health_tool(categories: &[&str]) -> Value {
    let shared = if categories.is_empty() {
        "none for now (the person chooses them in the app)".to_string()
    } else {
        categories.iter().map(|c| category_words(c)).collect::<Vec<_>>().join("; ")
    };
    json!({
        "name": TOOL_NAME,
        "description": format!("The person's own health data, as their phone shares it each day (Apple Health with \
    their Apple Watch, or Health Connect), in the categories they chose to share with you: {shared}. Use it to help them \
    with their sleep, activity, training and habits; never to diagnose: when something looks worrying, suggest they see a \
    doctor. By default: a summary of the last 7 days. from/to: dates (YYYY-MM-DD), or days: how many days back; \
    categories: some of yours; detail: \"days\" for one line per day."),
        "inputSchema": {
            "type": "object",
            "properties": {
                "from": {"type": "string", "description": "first day, YYYY-MM-DD"},
                "to": {"type": "string", "description": "last day, YYYY-MM-DD (default: the last day shared)"},
                "days": {"type": "integer", "description": "how many days back from `to` (default 7, at most 400)"},
                "categories": {"type": "array", "items": {"type": "string", "enum": categories}},
                "detail": {"type": "string", "enum": ["summary", "days"]},
            },
        },
    })
}

fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("health")
}

fn days_dir(config: &Config) -> PathBuf {
    root(config).join("days")
}

fn write_private(path: &std::path::Path, data: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(data).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, path)
}

fn read_state(config: &Config) -> Dict {
    match tk::read_text(&root(config).join("agents.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

fn agents_of(state: &Dict) -> Vec<String> {
    match state.get("agents") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// The categories of `agent` in `state` (none when its switch is off).
fn categories_in(state: &Dict, agent: &str) -> Vec<&'static str> {
    if !agents_of(state).iter().any(|a| a == agent) {
        return Vec::new();
    }
    match state.get("categories").and_then(|c| c.get(agent)) {
        Some(Value::Array(items)) => known_categories(&items.iter().filter_map(Value::as_str).collect::<Vec<_>>()),
        _ => EARLIER_CATEGORIES.to_vec(),
    }
}

/// The categories some agent reads: what the phone sends, what the days keep.
fn shared_in(state: &Dict) -> Vec<&'static str> {
    let all: Vec<&str> = agents_of(state).iter().flat_map(|a| categories_in(state, a)).collect();
    known_categories(&all)
}

/// The agents with health on.
pub fn agents(config: &Config) -> Vec<String> {
    agents_of(&read_state(config))
}

/// Whether `agent` has its « Santé » switch on.
pub fn enabled(config: &Config, agent: &str) -> bool {
    agents(config).iter().any(|a| a == agent)
}

/// The categories `agent` reads (none when its switch is off).
pub fn categories(config: &Config, agent: &str) -> Vec<&'static str> {
    categories_in(&read_state(config), agent)
}

fn categories_map(state: &Dict) -> Dict {
    agents_of(state).iter().map(|a| (a.clone(), json!(categories_in(state, a)))).collect()
}

/// `state` written with these agents' categories (only the agents with the switch on), then the days kept cut down
/// to what some agent still reads.
fn save_state(config: &Config, mut state: Dict, names: Vec<String>, categories: Dict) -> std::io::Result<()> {
    let categories: Dict = categories.into_iter().filter(|(a, _)| names.contains(a)).collect();
    state.insert("agents".into(), json!(names));
    state.insert("categories".into(), Value::Object(categories));
    write_private(&root(config).join("agents.json"), &Value::Object(state.clone()))?;
    if names.is_empty() {
        remove_days(config)
    } else {
        prune_days(config, &shared_in(&state))
    }
}

/// The switch of `agent`; turned on, it reads `DEFAULT_CATEGORIES`. Turning off the last one deletes the days kept.
/// Returns whether it changed.
pub fn set_enabled(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let state = read_state(config);
    let mut categories = categories_map(&state);
    let mut names = agents_of(&state);
    let before = names.len();
    names.retain(|a| a != agent);
    categories.remove(agent);
    if on {
        names.push(agent.to_string());
        let kept = categories_in(&state, agent);
        categories.insert(agent.into(), json!(if kept.is_empty() { DEFAULT_CATEGORIES.to_vec() } else { kept }));
    }
    let changed = names.len() != before;
    save_state(config, state, names, categories)?;
    Ok(changed)
}

/// The categories `agent` reads (its switch on; unknown names ignored). What no agent reads any more leaves the days
/// kept. Returns the categories now, or None when its switch is off.
pub fn set_categories<S: AsRef<str>>(
    config: &Config,
    agent: &str,
    wanted: &[S],
) -> std::io::Result<Option<Vec<&'static str>>> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let state = read_state(config);
    let names = agents_of(&state);
    if !names.iter().any(|a| a == agent) {
        return Ok(None);
    }
    let chosen = known_categories(wanted);
    let mut categories = categories_map(&state);
    categories.insert(agent.into(), json!(chosen));
    save_state(config, state, names, categories)?;
    Ok(Some(chosen))
}

/// A deleted agent: off (and the days with it when it was the last one).
pub fn forget(config: &Config, agent: &str) {
    if enabled(config, agent) {
        if let Err(err) = set_enabled(config, agent, false) {
            tracing::warn!(agent = %agent, error = %err, "health switch not removed");
        }
    }
}

fn remove_days(config: &Config) -> std::io::Result<()> {
    match std::fs::remove_dir_all(days_dir(config)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

/// Each day kept, without the figures of the categories outside `shared` (rewritten only when it changes).
fn prune_days(config: &Config, shared: &[&str]) -> std::io::Result<()> {
    for date in kept_dates(config) {
        let Some(day) = read_day(config, date) else { continue };
        let cut = day.clone().restricted(shared);
        if cut != day {
            write_private(&day_path(config, date), &serde_json::to_value(&cut).unwrap_or(Value::Null))?;
        }
    }
    Ok(())
}

// -- the days ----------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Sleep {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asleep_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_bed_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// The stages, when the watch knows them (minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deep_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rem_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub core_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awake_min: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Workout {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kcal: Option<f64>,
}

/// One day as the phone sums it up (its local calendar day; the night that ends on it). Each figure belongs to one
/// category (`restricted`); the unit is in its name.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Day {
    pub date: String,
    // activity
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_kcal: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercise_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_km: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flights: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stand_hours: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workouts: Vec<Workout>,
    // sleep
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep: Option<Sleep>,
    /// Breaths a minute, while asleep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resp_rate: Option<f64>,
    /// The wrist's temperature while asleep (°C; the tool shows how far it is from the period's average).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrist_temp_c: Option<f64>,
    // heart
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resting_hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hrv_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr_avg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hr_max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spo2_pct: Option<f64>,
    /// ml/kg/min.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vo2max: Option<f64>,
    // body
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_kg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_fat_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bmi: Option<f64>,
    /// mmHg, the day's last reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bp_sys: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bp_dia: Option<f64>,
    // food
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kcal_eaten: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub water_l: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caffeine_mg: Option<f64>,
    // wellbeing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mindful_min: Option<f64>,
    /// The day's mood logs (iPhone's « État d'esprit »): their average, from −1 (very unpleasant) to 1 (very pleasant).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood_valence: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood_logs: Option<f64>,
    // glucose (mmol/L: the unit in France and most of Europe; mg/dL = mmol/L × 18)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glucose_mmol: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glucose_min_mmol: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glucose_max_mmol: Option<f64>,
    // cycle
    /// « none », « light », « medium », « heavy » or « unspecified ».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub menstrual_flow: Option<String>,
    /// The first day of a period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_start: Option<bool>,
}

/// A value kept only when it is plausible: one odd reading (a weight typed in pounds, a sensor glitch) is dropped,
/// it never blocks the phone's uploads.
fn within(value: Option<f64>, low: f64, high: f64) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v >= low && *v <= high).map(|v| (v * 100.0).round() / 100.0)
}

fn short(text: Option<String>, max: usize) -> Option<String> {
    text.map(|t| t.chars().take(max).collect::<String>()).filter(|t| !t.is_empty())
}

const FLOWS: [&str; 5] = ["none", "light", "medium", "heavy", "unspecified"];

impl Day {
    /// The day as it is kept: implausible values out, strings cut short.
    fn cleaned(self) -> Day {
        let sleep = self.sleep.map(|s| Sleep {
            asleep_min: within(s.asleep_min, 0.0, 1440.0),
            in_bed_min: within(s.in_bed_min, 0.0, 1440.0),
            start: short(s.start, 40),
            end: short(s.end, 40),
            deep_min: within(s.deep_min, 0.0, 1440.0),
            rem_min: within(s.rem_min, 0.0, 1440.0),
            core_min: within(s.core_min, 0.0, 1440.0),
            awake_min: within(s.awake_min, 0.0, 1440.0),
        });
        let workouts = self
            .workouts
            .into_iter()
            .take(MAX_WORKOUTS)
            .map(|w| Workout {
                kind: w.kind.chars().take(40).collect::<String>().to_lowercase(),
                start: short(w.start, 40),
                duration_min: within(w.duration_min, 0.0, 1440.0),
                distance_m: within(w.distance_m, 0.0, 1_000_000.0),
                kcal: within(w.kcal, 0.0, 20000.0),
            })
            .collect();
        let flow = self.menstrual_flow.map(|f| f.to_lowercase()).filter(|f| FLOWS.contains(&f.as_str()));
        Day {
            date: self.date,
            steps: within(self.steps, 0.0, 300_000.0).map(f64::round),
            active_kcal: within(self.active_kcal, 0.0, 20000.0),
            exercise_min: within(self.exercise_min, 0.0, 1440.0),
            distance_km: within(self.distance_km, 0.0, 1000.0),
            flights: within(self.flights, 0.0, 5000.0).map(f64::round),
            stand_hours: within(self.stand_hours, 0.0, 24.0).map(f64::round),
            workouts,
            sleep: sleep.filter(|s| *s != Sleep::default()),
            resp_rate: within(self.resp_rate, 3.0, 60.0),
            wrist_temp_c: within(self.wrist_temp_c, 25.0, 45.0),
            resting_hr: within(self.resting_hr, 20.0, 250.0),
            hrv_ms: within(self.hrv_ms, 0.0, 500.0),
            hr_avg: within(self.hr_avg, 20.0, 250.0),
            hr_min: within(self.hr_min, 20.0, 250.0),
            hr_max: within(self.hr_max, 20.0, 250.0),
            spo2_pct: within(self.spo2_pct, 50.0, 100.0),
            vo2max: within(self.vo2max, 5.0, 100.0),
            weight_kg: within(self.weight_kg, 1.0, 500.0),
            body_fat_pct: within(self.body_fat_pct, 1.0, 80.0),
            bmi: within(self.bmi, 5.0, 100.0),
            bp_sys: within(self.bp_sys, 50.0, 260.0),
            bp_dia: within(self.bp_dia, 30.0, 200.0),
            kcal_eaten: within(self.kcal_eaten, 0.0, 20000.0),
            water_l: within(self.water_l, 0.0, 20.0),
            caffeine_mg: within(self.caffeine_mg, 0.0, 5000.0),
            mindful_min: within(self.mindful_min, 0.0, 1440.0),
            mood_valence: within(self.mood_valence, -1.0, 1.0),
            mood_logs: within(self.mood_logs, 0.0, 1000.0).map(f64::round),
            glucose_mmol: within(self.glucose_mmol, 1.0, 40.0),
            glucose_min_mmol: within(self.glucose_min_mmol, 1.0, 40.0),
            glucose_max_mmol: within(self.glucose_max_mmol, 1.0, 40.0),
            menstrual_flow: flow,
            cycle_start: self.cycle_start.filter(|s| *s),
        }
    }

    /// Only the figures of `allowed` categories: what an agent reads, what the place keeps.
    pub fn restricted(mut self, allowed: &[&str]) -> Day {
        let has = |c: &str| allowed.contains(&c);
        if !has("activity") {
            (self.steps, self.active_kcal, self.exercise_min) = (None, None, None);
            (self.distance_km, self.flights, self.stand_hours) = (None, None, None);
            self.workouts.clear();
        }
        if !has("sleep") {
            (self.sleep, self.resp_rate, self.wrist_temp_c) = (None, None, None);
        }
        if !has("heart") {
            (self.resting_hr, self.hrv_ms, self.hr_avg, self.hr_min) = (None, None, None, None);
            (self.hr_max, self.spo2_pct, self.vo2max) = (None, None, None);
        }
        if !has("body") {
            (self.weight_kg, self.body_fat_pct, self.bmi, self.bp_sys, self.bp_dia) = (None, None, None, None, None);
        }
        if !has("food") {
            (self.kcal_eaten, self.water_l, self.caffeine_mg) = (None, None, None);
        }
        if !has("wellbeing") {
            (self.mindful_min, self.mood_valence, self.mood_logs) = (None, None, None);
        }
        if !has("glucose") {
            (self.glucose_mmol, self.glucose_min_mmol, self.glucose_max_mmol) = (None, None, None);
        }
        if !has("cycle") {
            (self.menstrual_flow, self.cycle_start) = (None, None);
        }
        self
    }
}

fn parse_date(text: &str) -> Option<NaiveDate> {
    if text.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// The dates kept, oldest first.
fn kept_dates(config: &Config) -> Vec<NaiveDate> {
    let Ok(entries) = std::fs::read_dir(days_dir(config)) else { return Vec::new() };
    let mut dates: Vec<NaiveDate> = entries
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).and_then(parse_date))
        .collect();
    dates.sort();
    dates
}

fn day_path(config: &Config, date: NaiveDate) -> PathBuf {
    days_dir(config).join(format!("{}.json", date.format("%Y-%m-%d")))
}

fn read_day(config: &Config, date: NaiveDate) -> Option<Day> {
    tk::read_text(&day_path(config, date)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// The days sent by the phone, kept (each replaces the one of its date) with only the categories some agent reads;
/// then only the last `KEPT_DAYS`. A date too far in the future or the past is skipped. Returns how many were stored.
pub fn store(config: &Config, days: Vec<Day>, today: NaiveDate) -> std::io::Result<usize> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = read_state(config);
    let shared = shared_in(&state);
    let oldest = today - Duration::days(KEPT_DAYS as i64);
    let latest = today + Duration::days(2); // the phone's day may be ahead of the server's
    let mut stored = 0;
    for day in days {
        let Some(date) = parse_date(&day.date).filter(|d| *d > oldest && *d <= latest) else { continue };
        let day = day.cleaned().restricted(&shared);
        write_private(&day_path(config, date), &serde_json::to_value(&day).unwrap_or(Value::Null))?;
        stored += 1;
    }
    let dates = kept_dates(config);
    for date in dates.iter().take(dates.len().saturating_sub(KEPT_DAYS)).chain(dates.iter().filter(|d| **d <= oldest)) {
        match std::fs::remove_file(day_path(config, *date)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    state.insert("at".into(), json!(py::time_time()));
    write_private(&root(config).join("agents.json"), &Value::Object(state))?;
    Ok(stored)
}

// -- the tool ----------------------------------------------------------------------------------------------------------

/// « 6 h 52 ».
fn hours(minutes: f64) -> String {
    let total = minutes.round() as i64;
    format!("{} h {:02}", total / 60, total % 60)
}

/// The clock time of an ISO date-time (« 23:12 »), as the phone wrote it (the person's local time).
fn clock(iso: Option<&str>) -> Option<String> {
    let iso = iso?;
    let time = iso.get(11..16)?;
    (time.as_bytes().get(2) == Some(&b':')).then(|| time.to_string())
}

fn average(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn lowest(values: &[f64]) -> f64 {
    values.iter().cloned().fold(f64::MAX, f64::min)
}

fn highest(values: &[f64]) -> f64 {
    values.iter().cloned().fold(f64::MIN, f64::max)
}

fn number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

fn whole(value: f64) -> i64 {
    value.round() as i64
}

fn workout_text(w: &Workout) -> String {
    let mut text = if w.kind.is_empty() { "workout".to_string() } else { w.kind.clone() };
    if let Some(minutes) = w.duration_min {
        text += &format!(" {} min", whole(minutes));
    }
    if let Some(metres) = w.distance_m.filter(|m| *m > 0.0) {
        text += &format!(" {:.1} km", metres / 1000.0);
    }
    if let Some(kcal) = w.kcal.filter(|k| *k > 0.0) {
        text += &format!(" {} kcal", whole(kcal));
    }
    text
}

fn stages_text(sleep: &Sleep) -> Option<String> {
    let parts: Vec<String> =
        [("deep", sleep.deep_min), ("REM", sleep.rem_min), ("core", sleep.core_min), ("awake", sleep.awake_min)]
            .iter()
            .filter_map(|(name, minutes)| minutes.map(|m| format!("{name} {}", hours(m))))
            .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// `text` of the value, when there is one.
fn shown(value: Option<f64>, text: impl Fn(f64) -> String) -> Option<String> {
    value.map(text)
}

fn day_line(day: &Day, wanted: &[&str]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let has = |m: &str| wanted.contains(&m);
    if has("sleep") {
        if let Some(sleep) = &day.sleep {
            let mut text = format!("sleep {}", sleep.asleep_min.map(hours).unwrap_or_else(|| "?".into()));
            if let Some(bed) = sleep.in_bed_min {
                text += &format!(" (in bed {})", hours(bed));
            }
            if let (Some(start), Some(end)) = (clock(sleep.start.as_deref()), clock(sleep.end.as_deref())) {
                text += &format!(" {start}–{end}");
            }
            if let Some(stages) = stages_text(sleep) {
                text += &format!(" [{stages}]");
            }
            parts.push(text);
        }
        parts.extend(shown(day.resp_rate, |v| format!("{} breaths/min asleep", number(v))));
        parts.extend(shown(day.wrist_temp_c, |v| format!("wrist {v:.2} °C asleep")));
    }
    if has("activity") {
        parts.extend(shown(day.steps, |v| format!("{} steps", v as i64)));
        parts.extend(shown(day.active_kcal, |v| format!("{} active kcal", whole(v))));
        parts.extend(shown(day.exercise_min, |v| format!("{} min exercise", whole(v))));
        parts.extend(shown(day.distance_km, |v| format!("{v:.1} km walked or run")));
        parts.extend(shown(day.flights, |v| format!("{} flights", whole(v))));
        parts.extend(shown(day.stand_hours, |v| format!("{} stand hours", whole(v))));
    }
    if has("heart") {
        parts.extend(shown(day.resting_hr, |v| format!("resting HR {}", whole(v))));
        parts.extend(shown(day.hrv_ms, |v| format!("HRV {} ms", whole(v))));
        let range = match (day.hr_min, day.hr_max) {
            (Some(low), Some(high)) => format!(" ({}–{})", whole(low), whole(high)),
            _ => String::new(),
        };
        parts.extend(shown(day.hr_avg, |v| format!("HR {} bpm{range}", whole(v))));
        parts.extend(shown(day.spo2_pct, |v| format!("blood oxygen {} %", number(v))));
        parts.extend(shown(day.vo2max, |v| format!("VO2 max {}", number(v))));
    }
    if has("body") {
        parts.extend(shown(day.weight_kg, |v| format!("{} kg", number(v))));
        parts.extend(shown(day.body_fat_pct, |v| format!("body fat {} %", number(v))));
        parts.extend(shown(day.bmi, |v| format!("BMI {}", number(v))));
        if let (Some(sys), Some(dia)) = (day.bp_sys, day.bp_dia) {
            parts.push(format!("blood pressure {}/{}", whole(sys), whole(dia)));
        }
    }
    if has("food") {
        parts.extend(shown(day.kcal_eaten, |v| format!("{} kcal eaten", whole(v))));
        parts.extend(shown(day.water_l, |v| format!("{v:.1} L water")));
        parts.extend(shown(day.caffeine_mg, |v| format!("{} mg caffeine", whole(v))));
    }
    if has("wellbeing") {
        parts.extend(shown(day.mindful_min, |v| format!("{} min mindfulness", whole(v))));
        parts.extend(shown(day.mood_valence, |v| format!("mood {v:+.2}")));
    }
    if has("glucose") {
        parts.extend(shown(day.glucose_mmol, |v| format!("glucose {v:.1} mmol/L")));
    }
    if has("cycle") {
        if let Some(flow) = &day.menstrual_flow {
            let start = if day.cycle_start == Some(true) { ", period start" } else { "" };
            parts.push(format!("flow {flow}{start}"));
        }
    }
    if has("activity") && !day.workouts.is_empty() {
        parts.push(day.workouts.iter().map(workout_text).collect::<Vec<_>>().join(", "));
    }
    let shown = if parts.is_empty() { "nothing recorded".to_string() } else { parts.join(" · ") };
    format!("{}: {shown}", day.date)
}

/// « Weight: 71 kg on 2026-10-14, 71.6 kg on 2026-10-20 (+0.6 kg) »: the first and last of a period.
fn trend(days: &[Day], label: &str, unit: &str, pick: impl Fn(&Day) -> Option<f64>) -> Option<String> {
    let measured: Vec<(&Day, f64)> = days.iter().filter_map(|d| pick(d).map(|v| (d, v))).collect();
    let (first, last) = (measured.first()?, measured.last()?);
    Some(if measured.len() == 1 {
        format!("{label}: {}{unit} on {}", number(last.1), last.0.date)
    } else {
        format!(
            "{label}: {}{unit} on {}, {}{unit} on {} ({:+.1}{unit})",
            number(first.1),
            first.0.date,
            number(last.1),
            last.0.date,
            last.1 - first.1
        )
    })
}

fn summary(days: &[Day], wanted: &[&str]) -> Vec<String> {
    let mut lines = Vec::new();
    let has = |m: &str| wanted.contains(&m);
    let values = |pick: &dyn Fn(&Day) -> Option<f64>| -> Vec<f64> { days.iter().filter_map(pick).collect() };
    let daily = |lines: &mut Vec<String>, pick: &dyn Fn(&Day) -> Option<f64>, text: &dyn Fn(f64) -> String| {
        if let Some(avg) = average(&values(pick)) {
            lines.push(text(avg));
        }
    };
    if has("sleep") {
        let asleep = values(&|d| d.sleep.as_ref().and_then(|s| s.asleep_min));
        if let Some(avg) = average(&asleep) {
            let mut line = format!(
                "Sleep: {} asleep on average over {} nights (shortest {}, longest {})",
                hours(avg),
                asleep.len(),
                hours(lowest(&asleep)),
                hours(highest(&asleep))
            );
            if let Some(bed) = average(&values(&|d| d.sleep.as_ref().and_then(|s| s.in_bed_min))) {
                line += &format!(", {} in bed", hours(bed));
            }
            if let Some(last) = days.iter().rev().find_map(|d| d.sleep.as_ref().map(|s| (d, s))) {
                if let (Some(start), Some(end)) = (clock(last.1.start.as_deref()), clock(last.1.end.as_deref())) {
                    line += &format!("; last night ({}) {start}–{end}", last.0.date);
                }
            }
            lines.push(line);
        }
        let stages = Sleep {
            deep_min: average(&values(&|d| d.sleep.as_ref().and_then(|s| s.deep_min))),
            rem_min: average(&values(&|d| d.sleep.as_ref().and_then(|s| s.rem_min))),
            core_min: average(&values(&|d| d.sleep.as_ref().and_then(|s| s.core_min))),
            awake_min: average(&values(&|d| d.sleep.as_ref().and_then(|s| s.awake_min))),
            ..Sleep::default()
        };
        if let Some(text) = stages_text(&stages) {
            lines.push(format!("Sleep stages on average: {text}"));
        }
        daily(&mut lines, &|d| d.resp_rate, &|v| format!("Breathing rate asleep: {v:.1} breaths/min on average"));
        let temps: Vec<(&Day, f64)> = days.iter().filter_map(|d| d.wrist_temp_c.map(|t| (d, t))).collect();
        if let (Some(avg), Some(last)) = (average(&temps.iter().map(|t| t.1).collect::<Vec<_>>()), temps.last()) {
            let mut line = format!("Wrist temperature asleep: {avg:.2} °C on average");
            if temps.len() > 1 {
                line += &format!("; last night ({}) {:+.2} °C from that", last.0.date, last.1 - avg);
            }
            lines.push(line);
        }
    }
    if has("activity") {
        let steps = values(&|d| d.steps);
        if let Some(avg) = average(&steps) {
            let best = days.iter().filter(|d| d.steps.is_some()).max_by(|a, b| a.steps.partial_cmp(&b.steps).unwrap());
            let mut line = format!("Steps: {} a day on average over {} days", whole(avg), steps.len());
            if let Some(best) = best {
                line += &format!(" (most: {} on {})", best.steps.unwrap_or(0.0) as i64, best.date);
            }
            lines.push(line);
        }
        daily(&mut lines, &|d| d.active_kcal, &|v| format!("Active energy: {} kcal a day on average", whole(v)));
        let minutes = values(&|d| d.exercise_min);
        if let Some(avg) = average(&minutes) {
            lines.push(format!(
                "Exercise: {} min a day on average, {} min in all",
                whole(avg),
                whole(minutes.iter().sum::<f64>())
            ));
        }
        daily(&mut lines, &|d| d.distance_km, &|v| format!("Walking and running: {v:.1} km a day on average"));
        daily(&mut lines, &|d| d.flights, &|v| format!("Flights climbed: {} a day on average", number(v)));
        daily(&mut lines, &|d| d.stand_hours, &|v| format!("Stand hours: {} a day on average", number(v)));
        let mut kinds: BTreeMap<String, (usize, f64, f64)> = BTreeMap::new();
        for w in days.iter().flat_map(|d| d.workouts.iter()) {
            let entry = kinds.entry(if w.kind.is_empty() { "workout".into() } else { w.kind.clone() }).or_default();
            entry.0 += 1;
            entry.1 += w.duration_min.unwrap_or(0.0);
            entry.2 += w.distance_m.unwrap_or(0.0);
        }
        if !kinds.is_empty() {
            let count: usize = kinds.values().map(|k| k.0).sum();
            let each: Vec<String> = kinds
                .iter()
                .map(|(kind, (n, minutes, metres))| {
                    let distance = if *metres > 0.0 { format!(", {:.1} km", metres / 1000.0) } else { String::new() };
                    format!("{kind} ×{n} ({} min{distance})", whole(*minutes))
                })
                .collect();
            lines.push(format!("Workouts: {count} — {}", each.join(", ")));
        }
    }
    if has("heart") {
        let hr = values(&|d| d.resting_hr);
        if let Some(avg) = average(&hr) {
            lines.push(format!("Resting heart rate: {} bpm on average ({}–{})", whole(avg), lowest(&hr), highest(&hr)));
        }
        daily(&mut lines, &|d| d.hrv_ms, &|v| format!("Heart rate variability: {} ms on average", whole(v)));
        if let Some(avg) = average(&values(&|d| d.hr_avg)) {
            let mut line = format!("Heart rate: {} bpm on average", whole(avg));
            let (low, high) = (values(&|d| d.hr_min), values(&|d| d.hr_max));
            if !low.is_empty() && !high.is_empty() {
                line += &format!(" (lowest {}, highest {})", whole(lowest(&low)), whole(highest(&high)));
            }
            lines.push(line);
        }
        let oxygen = values(&|d| d.spo2_pct);
        if let Some(avg) = average(&oxygen) {
            lines.push(format!(
                "Blood oxygen: {} % on average (lowest day {} %)",
                number(avg),
                number(lowest(&oxygen))
            ));
        }
        lines.extend(trend(days, "VO2 max", " ml/kg/min", |d| d.vo2max));
    }
    if has("body") {
        lines.extend(trend(days, "Weight", " kg", |d| d.weight_kg));
        lines.extend(trend(days, "Body fat", " %", |d| d.body_fat_pct));
        lines.extend(trend(days, "BMI", "", |d| d.bmi));
        let pressure: Vec<(&Day, f64, f64)> = days.iter().filter_map(|d| Some((d, d.bp_sys?, d.bp_dia?))).collect();
        if let Some(last) = pressure.last() {
            let sys = average(&pressure.iter().map(|p| p.1).collect::<Vec<_>>()).unwrap_or(0.0);
            let dia = average(&pressure.iter().map(|p| p.2).collect::<Vec<_>>()).unwrap_or(0.0);
            lines.push(format!(
                "Blood pressure: {}/{} mmHg on average over {} days (last {}/{} on {})",
                whole(sys),
                whole(dia),
                pressure.len(),
                whole(last.1),
                whole(last.2),
                last.0.date
            ));
        }
    }
    if has("food") {
        daily(&mut lines, &|d| d.kcal_eaten, &|v| format!("Food energy: {} kcal a day on average", whole(v)));
        daily(&mut lines, &|d| d.water_l, &|v| format!("Water: {v:.1} L a day on average"));
        daily(&mut lines, &|d| d.caffeine_mg, &|v| format!("Caffeine: {} mg a day on average", whole(v)));
    }
    if has("wellbeing") {
        let mindful = values(&|d| d.mindful_min);
        if !mindful.is_empty() {
            lines.push(format!(
                "Mindfulness: {} min in all, on {} days",
                whole(mindful.iter().sum::<f64>()),
                mindful.len()
            ));
        }
        let moods: Vec<(f64, f64)> =
            days.iter().filter_map(|d| d.mood_valence.map(|v| (v, d.mood_logs.unwrap_or(1.0).max(1.0)))).collect();
        let logs: f64 = moods.iter().map(|m| m.1).sum();
        if logs > 0.0 {
            let avg = moods.iter().map(|m| m.0 * m.1).sum::<f64>() / logs;
            lines.push(format!(
                "Mood: {avg:+.2} on average over {} logs (from −1 very unpleasant to +1 very pleasant)",
                whole(logs)
            ));
        }
    }
    if has("glucose") {
        if let Some(avg) = average(&values(&|d| d.glucose_mmol)) {
            let mut line = format!("Blood glucose: {avg:.1} mmol/L on average");
            let (low, high) = (values(&|d| d.glucose_min_mmol), values(&|d| d.glucose_max_mmol));
            if !low.is_empty() && !high.is_empty() {
                line += &format!(" (lowest {:.1}, highest {:.1})", lowest(&low), highest(&high));
            }
            lines.push(line);
        }
    }
    if has("cycle") {
        let starts: Vec<&str> = days.iter().filter(|d| d.cycle_start == Some(true)).map(|d| d.date.as_str()).collect();
        let mut flows: BTreeMap<&str, usize> = BTreeMap::new();
        for flow in days.iter().filter_map(|d| d.menstrual_flow.as_deref()).filter(|f| *f != "none") {
            *flows.entry(flow).or_default() += 1;
        }
        if !starts.is_empty() || !flows.is_empty() {
            let mut line = "Cycle:".to_string();
            if !starts.is_empty() {
                line += &format!(" period started on {};", starts.join(", "));
            }
            let each: Vec<String> = flows.iter().map(|(f, n)| format!("{f} ×{n}")).collect();
            line += &format!(" flow on {} days ({})", flows.values().sum::<usize>(), each.join(", "));
            lines.push(line);
        }
    }
    lines
}

/// The `health` tool for `agent` (whose switch the caller checked): only its categories.
pub fn call(config: &Config, agent: &str, arguments: &Dict) -> Dict {
    let allowed = categories(config, agent);
    if allowed.is_empty() {
        return tool_text(
            "The person shares no health category with you for now: they choose them in « Santé » on your page of the \
BipAgents app.",
            false,
        );
    }
    let dates = kept_dates(config);
    let Some(last_kept) = dates.last().copied() else {
        return tool_text(
            "No health data yet: the person's phone sends it when the BipAgents app opens (or in the background, \
on an iPhone).",
            false,
        );
    };
    let date_arg = |key: &str| -> Result<Option<NaiveDate>, String> {
        let text = tk::strip(&tk::str_or_empty(arguments.get(key))).to_string();
        if text.is_empty() {
            return Ok(None);
        }
        parse_date(&text).map(Some).ok_or_else(|| format!("{key}: a date as YYYY-MM-DD"))
    };
    let (from, to) = match (date_arg("from"), date_arg("to")) {
        (Ok(from), Ok(to)) => (from, to),
        (Err(e), _) | (_, Err(e)) => return tool_text(&e, true),
    };
    let to = to.unwrap_or(last_kept);
    let back = tk::as_py_int(arguments.get("days")).unwrap_or(DEFAULT_DAYS).clamp(1, KEPT_DAYS as i64);
    let from = from.unwrap_or(to - Duration::days(back - 1));
    if from > to {
        return tool_text("from is after to", true);
    }
    let asked: Vec<&str> = match (arguments.get("categories"), arguments.get("metrics")) {
        (Some(Value::Array(items)), _) if !items.is_empty() => items.iter().filter_map(Value::as_str).collect(),
        (_, Some(Value::Array(items))) if !items.is_empty() => items
            .iter()
            .filter_map(Value::as_str)
            .filter_map(|m| OLD_METRICS.iter().find(|(old, _)| *old == m).map(|(_, c)| *c))
            .collect(),
        _ => allowed.clone(),
    };
    let wanted: Vec<&str> = allowed.iter().copied().filter(|c| asked.contains(c)).collect();
    if wanted.is_empty() {
        return tool_text(&format!("categories: some of {}", allowed.join(", ")), true);
    }
    let days: Vec<Day> = dates
        .iter()
        .filter(|d| **d >= from && **d <= to)
        .filter_map(|d| read_day(config, *d))
        .map(|d| d.restricted(&allowed))
        .collect();
    let period = format!("{} to {}", from.format("%Y-%m-%d"), to.format("%Y-%m-%d"));
    if days.is_empty() {
        return tool_text(
            &format!("No health data from {period} (kept: {} to {last_kept}).", dates[0].format("%Y-%m-%d")),
            false,
        );
    }
    let by_day = tk::str_or_empty(arguments.get("detail")) == "days";
    let lines: Vec<String> =
        if by_day { days.iter().map(|d| day_line(d, &wanted)).collect() } else { summary(&days, &wanted) };
    let body = if lines.is_empty() { "Nothing recorded in these categories.".to_string() } else { lines.join("\n") };
    tool_text(
        &format!("Health, {period} ({} days with data; shared with you: {}):\n{body}", days.len(), allowed.join(", ")),
        false,
    )
}

// -- the app's routes --------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct Upload {
    #[serde(default)]
    pub days: Vec<Day>,
}

impl Validate for Upload {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "days", Some(&self.days), MAX_UPLOAD);
        for (n, day) in self.days.iter().enumerate() {
            if parse_date(&day.date).is_none() {
                errors.push(FieldError::new(
                    "value_error",
                    vec![json!("body"), json!("days"), json!(n), json!("date")],
                    "Value error, a date as YYYY-MM-DD",
                    json!(day.date),
                ));
            }
        }
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> ApiResult<T> {
    Ok(tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)??)
}

async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let out = blocking(move || {
        let data = read_state(&config);
        let dates = kept_dates(&config);
        let shown = |d: Option<&NaiveDate>| d.map(|d| json!(d.format("%Y-%m-%d").to_string())).unwrap_or(Value::Null);
        Ok(json!({"agents": agents_of(&data), "categories": categories_map(&data), "shared": shared_in(&data),
                  "days": dates.len(), "first": shown(dates.first()), "last": shown(dates.last()),
                  "at": data.get("at").cloned().unwrap_or(Value::Null)}))
    })
    .await?;
    Ok(web::ok(out))
}

async fn upload(State(state): State<AppState>, Json(body): Json<Upload>) -> ApiResult<Response> {
    let config = state.config();
    let stored = blocking(move || {
        if agents(&config).is_empty() {
            return Ok(None); // switched off meanwhile: nothing is kept
        }
        let stored = store(&config, body.days, chrono::Local::now().date_naive())?;
        Ok(Some((stored, kept_dates(&config).len())))
    })
    .await?;
    let Some((stored, total)) = stored else {
        return Err(HttpError::new(409, "health is off for every agent"));
    };
    Ok(web::ok(json!({"stored": stored, "days": total})))
}

/// `health_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/health-data", get(status)).route("/health-data/days", put(upload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implausible_values_are_dropped_not_refused() {
        let day: Day = serde_json::from_value(json!({"date": "2026-10-20", "steps": 8423.4, "weight_kg": 0,
            "resting_hr": 58, "sleep": {"asleep_min": -3}, "workouts": [{"type": "Running", "duration_min": 32}],
            "spo2_pct": 120, "glucose_mmol": 5.43, "menstrual_flow": "Medium", "cycle_start": false,
            "mood_valence": -2, "wrist_temp_c": 35.123}))
        .unwrap();
        let day = day.cleaned();
        assert_eq!((day.steps, day.weight_kg, day.resting_hr), (Some(8423.0), None, Some(58.0)));
        assert_eq!(day.sleep, None);
        assert_eq!(day.workouts[0].kind, "running");
        assert_eq!((day.spo2_pct, day.glucose_mmol, day.mood_valence), (None, Some(5.43), None));
        assert_eq!(
            (day.menstrual_flow.as_deref(), day.cycle_start, day.wrist_temp_c),
            (Some("medium"), None, Some(35.12))
        );
    }

    #[test]
    fn times_and_durations_read_plainly() {
        assert_eq!(hours(412.0), "6 h 52");
        assert_eq!(clock(Some("2026-10-19T23:12:00+02:00")).as_deref(), Some("23:12"));
        assert_eq!(clock(Some("2026-10-19")), None);
        assert_eq!(number(71.0), "71");
        assert_eq!(number(71.24), "71.2");
    }

    fn full_day() -> Day {
        serde_json::from_value(json!({"date": "2026-10-20", "steps": 9000, "distance_km": 6.4, "flights": 12,
            "stand_hours": 10, "workouts": [{"type": "running", "duration_min": 30}],
            "sleep": {"asleep_min": 420, "deep_min": 70, "rem_min": 95}, "resp_rate": 14.5, "wrist_temp_c": 35.2,
            "resting_hr": 55, "hr_avg": 72, "hr_min": 48, "hr_max": 160, "spo2_pct": 97, "vo2max": 44.1,
            "weight_kg": 70.5, "body_fat_pct": 18.2, "bmi": 22.4, "bp_sys": 121, "bp_dia": 79,
            "kcal_eaten": 2100, "water_l": 1.8, "caffeine_mg": 190, "mindful_min": 10, "mood_valence": 0.4,
            "mood_logs": 2, "glucose_mmol": 5.6, "glucose_min_mmol": 4.4, "glucose_max_mmol": 7.9,
            "menstrual_flow": "light", "cycle_start": true}))
        .unwrap()
    }

    #[test]
    fn each_category_keeps_only_its_own_figures() {
        let all = full_day().cleaned();
        assert_eq!(all.clone().restricted(&CATEGORIES), all);
        let heart = all.clone().restricted(&["heart"]);
        assert_eq!(
            heart,
            Day {
                date: "2026-10-20".into(),
                resting_hr: Some(55.0),
                hr_avg: Some(72.0),
                hr_min: Some(48.0),
                hr_max: Some(160.0),
                spo2_pct: Some(97.0),
                vo2max: Some(44.1),
                ..Day::default()
            }
        );
        assert_eq!(all.clone().restricted(&[]), Day { date: "2026-10-20".into(), ..Day::default() });
        // Every figure is in some category: the day with each category alone, put back together, is the whole day.
        let json_of = |d: &Day| serde_json::to_value(d).unwrap().as_object().unwrap().clone();
        let mut merged = Dict::new();
        for category in CATEGORIES {
            for (k, v) in json_of(&all.clone().restricted(&[category])) {
                assert!(k == "date" || !merged.contains_key(&k), "{k} in two categories");
                merged.insert(k, v);
            }
        }
        assert_eq!(merged, json_of(&all));
    }

    #[test]
    fn the_tool_speaks_of_the_new_figures() {
        let days = vec![full_day().cleaned()];
        let text = summary(&days, &CATEGORIES).join("\n");
        for expected in [
            "Sleep stages on average: deep 1 h 10, REM 1 h 35",
            "Breathing rate asleep: 14.5 breaths/min",
            "Walking and running: 6.4 km a day",
            "Flights climbed: 12 a day",
            "Heart rate: 72 bpm on average (lowest 48, highest 160)",
            "Blood oxygen: 97 %",
            "VO2 max: 44.1 ml/kg/min on 2026-10-20",
            "Body fat: 18.2 % on 2026-10-20",
            "Blood pressure: 121/79 mmHg on average over 1 days",
            "Food energy: 2100 kcal",
            "Water: 1.8 L",
            "Caffeine: 190 mg",
            "Mindfulness: 10 min in all, on 1 days",
            "Mood: +0.40 on average over 2 logs",
            "Blood glucose: 5.6 mmol/L on average (lowest 4.4, highest 7.9)",
            "Cycle: period started on 2026-10-20; flow on 1 days (light ×1)",
        ] {
            assert!(text.contains(expected), "{expected} not in:\n{text}");
        }
        let line = day_line(&days[0], &["heart", "cycle"]);
        assert!(line.contains("HR 72 bpm (48–160)") && line.contains("flow light, period start"), "{line}");
        assert!(!line.contains("steps") && !line.contains("kg"), "{line}");
    }

    #[test]
    fn the_tool_description_lists_only_the_agents_categories() {
        let tool = health_tool(&["activity", "glucose"]);
        let text = tool["description"].as_str().unwrap();
        assert!(text.contains("flights climbed") && text.contains("blood glucose (mmol/L)"));
        assert!(!text.contains("menstrual") && !text.contains("blood pressure"));
        assert_eq!(tool["inputSchema"]["properties"]["categories"]["items"]["enum"], json!(["activity", "glucose"]));
        assert_eq!(known_categories(&["cycle", "x", "activity", "cycle"]), vec!["activity", "cycle"]);
    }
}
