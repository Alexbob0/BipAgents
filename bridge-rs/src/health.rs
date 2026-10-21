//! The person's health data, for the agents they choose (decided 2026-10-21): their phone reads it with the system's
//! permission (Apple Health on the iPhone, where the Apple Watch's data lands too; Health Connect on Android) and
//! sends daily summaries here — sleep, steps, active energy, exercise, resting heart rate, heart rate variability,
//! weight, workouts. It stays in this place (one file per day, at most `KEPT_DAYS`), never goes anywhere else, and goes
//! when the last agent's « Santé » switch is turned off. An agent with the switch on gets the `health` tool and a
//! rule in its instructions (souls::HEALTH_RULE): help with sleep, activity and training, never diagnose.
//!
//!     health    the person's days for a period: summaries by default, or one line per day
//!
//! App routes (the bridge key):
//!
//!     GET /v1/health-data         {agents, days, first, last, at}: the agents with health on, what is kept
//!     PUT /v1/health-data/days    {days: [{date, sleep, steps, active_kcal, exercise_min, resting_hr, hrv_ms,
//!                                 weight_kg, workouts}]}: the days that changed (409 when no agent has it on)
//!
//! The switch itself is an agent setting (`PATCH /v1/agents/{id}/settings {"health": bool}`, agents_admin.rs).
//!
//!     data_dir/health/agents.json          (chmod 600: the agents allowed, the last upload)
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
pub const METRICS: [&str; 7] = ["sleep", "steps", "energy", "exercise", "heart", "weight", "workouts"];

/// The `health` tool's definition (listed only for an agent whose switch is on).
pub fn health_tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "The person's own health data, as their phone shares it each day (Apple Health with their Apple \
    Watch, or Health Connect): sleep, steps, active energy, exercise minutes, resting heart rate, heart rate \
    variability, weight and workouts. Use it to help them with their sleep, activity and training; never to \
    diagnose: when something looks worrying, suggest they see a doctor. By default: a summary of the last 7 days. \
    from/to: dates (YYYY-MM-DD), or days: how many days back; metrics: some of sleep, steps, energy, exercise, \
    heart, weight, workouts; detail: \"days\" for one line per day.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "from": {"type": "string", "description": "first day, YYYY-MM-DD"},
                "to": {"type": "string", "description": "last day, YYYY-MM-DD (default: the last day shared)"},
                "days": {"type": "integer", "description": "how many days back from `to` (default 7, at most 400)"},
                "metrics": {"type": "array", "items": {"type": "string", "enum": METRICS}},
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

/// The agents with health on.
pub fn agents(config: &Config) -> Vec<String> {
    agents_of(&read_state(config))
}

/// Whether `agent` has its « Santé » switch on.
pub fn enabled(config: &Config, agent: &str) -> bool {
    agents(config).iter().any(|a| a == agent)
}

/// The switch of `agent`. Turning off the last one deletes the days kept. Returns whether it changed.
pub fn set_enabled(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = read_state(config);
    let mut names = agents_of(&state);
    let before = names.len();
    names.retain(|a| a != agent);
    if on {
        names.push(agent.to_string());
    }
    let changed = names.len() != before;
    state.insert("agents".into(), json!(names));
    write_private(&root(config).join("agents.json"), &Value::Object(state))?;
    if names.is_empty() {
        remove_days(config)?;
    }
    Ok(changed)
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

/// One day as the phone sums it up (its local calendar day; the night that ends on it).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct Day {
    pub date: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep: Option<Sleep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_kcal: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercise_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resting_hr: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hrv_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight_kg: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workouts: Vec<Workout>,
}

/// A value kept only when it is plausible: one odd reading (a weight typed in pounds, a sensor glitch) is dropped,
/// it never blocks the phone's uploads.
fn within(value: Option<f64>, low: f64, high: f64) -> Option<f64> {
    value.filter(|v| v.is_finite() && *v >= low && *v <= high).map(|v| (v * 100.0).round() / 100.0)
}

fn short(text: Option<String>, max: usize) -> Option<String> {
    text.map(|t| t.chars().take(max).collect::<String>()).filter(|t| !t.is_empty())
}

impl Day {
    /// The day as it is kept: implausible values out, strings cut short.
    fn cleaned(self) -> Day {
        let sleep = self.sleep.map(|s| Sleep {
            asleep_min: within(s.asleep_min, 0.0, 1440.0),
            in_bed_min: within(s.in_bed_min, 0.0, 1440.0),
            start: short(s.start, 40),
            end: short(s.end, 40),
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
        Day {
            date: self.date,
            sleep: sleep.filter(|s| *s != Sleep::default()),
            steps: within(self.steps, 0.0, 300_000.0).map(f64::round),
            active_kcal: within(self.active_kcal, 0.0, 20000.0),
            exercise_min: within(self.exercise_min, 0.0, 1440.0),
            resting_hr: within(self.resting_hr, 20.0, 250.0),
            hrv_ms: within(self.hrv_ms, 0.0, 500.0),
            weight_kg: within(self.weight_kg, 1.0, 500.0),
            workouts,
        }
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

/// The days sent by the phone, kept (each replaces the one of its date); then only the last `KEPT_DAYS`. A date too
/// far in the future or the past is skipped. Returns how many were stored.
pub fn store(config: &Config, days: Vec<Day>, today: NaiveDate) -> std::io::Result<usize> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let oldest = today - Duration::days(KEPT_DAYS as i64);
    let latest = today + Duration::days(2); // the phone's day may be ahead of the server's
    let mut stored = 0;
    for day in days {
        let Some(date) = parse_date(&day.date).filter(|d| *d > oldest && *d <= latest) else { continue };
        let day = day.cleaned();
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
    let mut state = read_state(config);
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

fn number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

fn workout_text(w: &Workout) -> String {
    let mut text = if w.kind.is_empty() { "workout".to_string() } else { w.kind.clone() };
    if let Some(minutes) = w.duration_min {
        text += &format!(" {} min", minutes.round() as i64);
    }
    if let Some(metres) = w.distance_m.filter(|m| *m > 0.0) {
        text += &format!(" {:.1} km", metres / 1000.0);
    }
    if let Some(kcal) = w.kcal.filter(|k| *k > 0.0) {
        text += &format!(" {} kcal", kcal.round() as i64);
    }
    text
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
            parts.push(text);
        }
    }
    if has("steps") {
        if let Some(steps) = day.steps {
            parts.push(format!("{} steps", steps as i64));
        }
    }
    if has("energy") {
        if let Some(kcal) = day.active_kcal {
            parts.push(format!("{} active kcal", kcal.round() as i64));
        }
    }
    if has("exercise") {
        if let Some(minutes) = day.exercise_min {
            parts.push(format!("{} min exercise", minutes.round() as i64));
        }
    }
    if has("heart") {
        if let Some(hr) = day.resting_hr {
            parts.push(format!("resting HR {}", hr.round() as i64));
        }
        if let Some(hrv) = day.hrv_ms {
            parts.push(format!("HRV {} ms", hrv.round() as i64));
        }
    }
    if has("weight") {
        if let Some(kg) = day.weight_kg {
            parts.push(format!("{} kg", number(kg)));
        }
    }
    if has("workouts") && !day.workouts.is_empty() {
        parts.push(day.workouts.iter().map(workout_text).collect::<Vec<_>>().join(", "));
    }
    let shown = if parts.is_empty() { "nothing recorded".to_string() } else { parts.join(" · ") };
    format!("{}: {shown}", day.date)
}

fn summary(days: &[Day], wanted: &[&str]) -> Vec<String> {
    let mut lines = Vec::new();
    let has = |m: &str| wanted.contains(&m);
    let values = |pick: &dyn Fn(&Day) -> Option<f64>| -> Vec<f64> { days.iter().filter_map(pick).collect() };
    if has("sleep") {
        let asleep = values(&|d| d.sleep.as_ref().and_then(|s| s.asleep_min));
        if let Some(avg) = average(&asleep) {
            let low = asleep.iter().cloned().fold(f64::MAX, f64::min);
            let high = asleep.iter().cloned().fold(0.0, f64::max);
            let mut line = format!(
                "Sleep: {} asleep on average over {} nights (shortest {}, longest {})",
                hours(avg),
                asleep.len(),
                hours(low),
                hours(high)
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
    }
    if has("steps") {
        let steps = values(&|d| d.steps);
        if let Some(avg) = average(&steps) {
            let best = days.iter().filter(|d| d.steps.is_some()).max_by(|a, b| a.steps.partial_cmp(&b.steps).unwrap());
            let mut line = format!("Steps: {} a day on average over {} days", avg.round() as i64, steps.len());
            if let Some(best) = best {
                line += &format!(" (most: {} on {})", best.steps.unwrap_or(0.0) as i64, best.date);
            }
            lines.push(line);
        }
    }
    if has("energy") {
        let kcal = values(&|d| d.active_kcal);
        if let Some(avg) = average(&kcal) {
            lines.push(format!("Active energy: {} kcal a day on average", avg.round() as i64));
        }
    }
    if has("exercise") {
        let minutes = values(&|d| d.exercise_min);
        if let Some(avg) = average(&minutes) {
            lines.push(format!(
                "Exercise: {} min a day on average, {} min in all",
                avg.round() as i64,
                minutes.iter().sum::<f64>().round() as i64
            ));
        }
    }
    if has("heart") {
        let hr = values(&|d| d.resting_hr);
        if let Some(avg) = average(&hr) {
            let low = hr.iter().cloned().fold(f64::MAX, f64::min);
            let high = hr.iter().cloned().fold(0.0, f64::max);
            lines.push(format!("Resting heart rate: {} bpm on average ({}–{})", avg.round() as i64, low, high));
        }
        if let Some(avg) = average(&values(&|d| d.hrv_ms)) {
            lines.push(format!("Heart rate variability: {} ms on average", avg.round() as i64));
        }
    }
    if has("weight") {
        let weighed: Vec<&Day> = days.iter().filter(|d| d.weight_kg.is_some()).collect();
        if let (Some(first), Some(last)) = (weighed.first(), weighed.last()) {
            let (a, b) = (first.weight_kg.unwrap_or(0.0), last.weight_kg.unwrap_or(0.0));
            if weighed.len() == 1 {
                lines.push(format!("Weight: {} kg on {}", number(b), last.date));
            } else {
                lines.push(format!(
                    "Weight: {} kg on {}, {} kg on {} ({:+.1} kg)",
                    number(a),
                    first.date,
                    number(b),
                    last.date,
                    b - a
                ));
            }
        }
    }
    if has("workouts") {
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
                    format!("{kind} ×{n} ({} min{distance})", minutes.round() as i64)
                })
                .collect();
            lines.push(format!("Workouts: {count} — {}", each.join(", ")));
        }
    }
    lines
}

/// The `health` tool for `agent` (whose switch the caller checked).
pub fn call(config: &Config, arguments: &Dict) -> Dict {
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
    let wanted: Vec<&str> = match arguments.get("metrics") {
        Some(Value::Array(items)) if !items.is_empty() => {
            METRICS.iter().copied().filter(|m| items.iter().any(|i| i.as_str() == Some(m))).collect()
        }
        _ => METRICS.to_vec(),
    };
    if wanted.is_empty() {
        return tool_text(&format!("metrics: some of {}", METRICS.join(", ")), true);
    }
    let days: Vec<Day> =
        dates.iter().filter(|d| **d >= from && **d <= to).filter_map(|d| read_day(config, *d)).collect();
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
    let body = if lines.is_empty() { "Nothing recorded for these metrics.".to_string() } else { lines.join("\n") };
    tool_text(&format!("Health, {period} ({} days with data):\n{body}", days.len()), false)
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
        Ok(json!({"agents": agents_of(&data), "days": dates.len(), "first": shown(dates.first()),
                  "last": shown(dates.last()), "at": data.get("at").cloned().unwrap_or(Value::Null)}))
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
            "resting_hr": 58, "sleep": {"asleep_min": -3}, "workouts": [{"type": "Running", "duration_min": 32}]}))
        .unwrap();
        let day = day.cleaned();
        assert_eq!((day.steps, day.weight_kg, day.resting_hr), (Some(8423.0), None, Some(58.0)));
        assert_eq!(day.sleep, None);
        assert_eq!(day.workouts[0].kind, "running");
    }

    #[test]
    fn times_and_durations_read_plainly() {
        assert_eq!(hours(412.0), "6 h 52");
        assert_eq!(clock(Some("2026-10-19T23:12:00+02:00")).as_deref(), Some("23:12"));
        assert_eq!(clock(Some("2026-10-19")), None);
        assert_eq!(number(71.0), "71");
        assert_eq!(number(71.24), "71.2");
    }
}
