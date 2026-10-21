//! The person's phone calendar, for the agents they choose (decided 2026-10-21), shared the way the Reminders are
//! (reminders.rs): no server reaches it (the iPhone's EventKit, Android's CalendarContract, where the Google calendars
//! synced on the phone are too), so the app sends a copy here when it opens (and when woken by a silent push) — its
//! calendars and their events from a week back to two months ahead — and applies what the agents asked for. An agent
//! reads the last copy and asks for an event to be added, changed or deleted; the request waits here until the phone
//! has done it. Like the reminders, an agent's request needs no approval of its own: the person chose the agents that
//! may use the calendar.
//!
//!     calendar_list     the events of a period, as the phone last shared them (and when)
//!     calendar_add      a new event (a title, a start, an end or a duration, a calendar…), done by the phone
//!     calendar_update   an event changed (its title, times, place or notes), the same way
//!     calendar_delete   an event deleted, the same way
//!
//! Repeating events are listed (each occurrence) but not changed or deleted here: the person does it on the phone.
//! The calendars of the agent's mailboxes, read by CalDAV (caldav.rs), come in the same tools: listed with their
//! source, and written there directly when `calendar_add` names one of them.
//! The tools are listed only for the agents allowed (as `health`): the tools' entry of an agent changes when it is
//! allowed or not, so its Hermes fetches the list again.
//!
//! App routes (the bridge key):
//!
//!     GET  /v1/calendar                  {agents, at, from, to, calendars: n, events: n, pending}
//!     PUT  /v1/calendar                  {from, to, calendars: [{id, title, color?}],
//!                                        events: [{id, calendar, title, start, end, all_day, location?, notes?,
//!                                        recurring?}]}
//!     PUT  /v1/calendar/agents           {agents}
//!     GET  /v1/calendar/queue            the agents' requests waiting for the phone
//!     POST /v1/calendar/queue/{id}       {done: bool, error?}
//!
//! Times: `start` and `end` as the phone writes them, an ISO date-time with its offset, or a date (YYYY-MM-DD) for an
//! all-day event (`end` its last day). An agent may write a local date-time without offset: the phone's own time.
//!
//!     data_dir/calendar.json   (chmod 600: the copy, the agents allowed, the requests)
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

static LOCK: Mutex<()> = Mutex::new(());
/// Done or failed requests are kept this long (the pending ones until the phone answers).
pub const QUEUE_DAYS: i64 = 14;
/// The period `calendar_list` reads by default, from today.
pub const DEFAULT_DAYS: i64 = 7;
/// Events one answer lists at most.
pub const MAX_LISTED: usize = 200;
/// An event added without an end nor a duration.
pub const DEFAULT_MINUTES: i64 = 60;
pub const TOOL_NAMES: [&str; 4] = ["calendar_list", "calendar_add", "calendar_update", "calendar_delete"];

/// The tools' definitions (listed only for an agent allowed).
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "calendar_list",
            "description": "The events of the person's phone calendar (its own and the accounts synced on it, Google's \
        included) as their phone last shared them: by default the next 7 days. from/to: dates (YYYY-MM-DD); calendar: a \
        calendar's name. The copy covers about a week back to two months ahead.",
            "inputSchema": {"type": "object", "properties": {
                "from": {"type": "string", "description": "first day, YYYY-MM-DD (default: today)"},
                "to": {"type": "string", "description": "last day, YYYY-MM-DD (default: 7 days after from)"},
                "calendar": {"type": "string"}}},
        }),
        json!({
            "name": "calendar_add",
            "description": "Add an event to the person's phone calendar (it appears when their phone next syncs, usually \
        within minutes). start: an ISO date-time in the person's local time (2026-10-30T10:00), or a date for an all-day \
        event; end, or duration_minutes (default 60); calendar: a calendar's name (default: the phone's default one).",
            "inputSchema": {"type": "object", "properties": {
                "title": {"type": "string"}, "start": {"type": "string"}, "end": {"type": "string"},
                "duration_minutes": {"type": "integer"}, "calendar": {"type": "string"},
                "location": {"type": "string"}, "notes": {"type": "string"}},
                "required": ["title", "start"]},
        }),
        json!({
            "name": "calendar_update",
            "description": "Change one of the person's events (its id from calendar_list): only the fields given. Not for \
        a repeating event (the person changes those on their phone).",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"}, "title": {"type": "string"}, "start": {"type": "string"},
                "end": {"type": "string"}, "location": {"type": "string"}, "notes": {"type": "string"}},
                "required": ["id"]},
        }),
        json!({
            "name": "calendar_delete",
            "description": "Delete one of the person's events (its id from calendar_list). Only when the person asked for \
        it. Not for a repeating event.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
        }),
    ]
}

fn path_of(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("calendar.json")
}

pub fn read(config: &Config) -> Dict {
    match tk::read_text(&path_of(config)).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

pub fn write(config: &Config, data: &Dict) -> std::io::Result<()> {
    let path = path_of(config);
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&Value::Object(data.clone())).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &path)
}

/// The agents allowed.
pub fn agents(config: &Config) -> Vec<String> {
    match read(config).get("agents") {
        Some(Value::Array(names)) => names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// Whether `agent` may use the calendar (and gets its tools).
pub fn enabled(config: &Config, agent: &str) -> bool {
    agents(config).iter().any(|a| a == agent)
}

/// « 12 min », « 3 h », « 2 days ».
fn age(seconds: f64) -> String {
    let minutes = (seconds / 60.0).floor() as i64;
    if minutes < 120 {
        format!("{minutes} min")
    } else if minutes < 2880 {
        format!("{} h", minutes.div_euclid(60))
    } else {
        format!("{} days", minutes.div_euclid(1440))
    }
}

fn list_of(data: &Dict, key: &str) -> Vec<Value> {
    match data.get(key) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn text(item: &Value, key: &str) -> String {
    item.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A moment as an agent or the phone writes it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Moment {
    Day(NaiveDate),
    Local(NaiveDateTime),
    Zoned(DateTime<FixedOffset>),
}

impl Moment {
    pub fn parse(value: &str) -> Option<Moment> {
        let value = value.trim();
        if let Ok(day) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            return Some(Moment::Day(day));
        }
        if let Ok(zoned) = DateTime::parse_from_rfc3339(value) {
            return Some(Moment::Zoned(zoned));
        }
        for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
            if let Ok(local) = NaiveDateTime::parse_from_str(value, format) {
                return Some(Moment::Local(local));
            }
        }
        let value = match value.strip_suffix('Z') {
            Some(utc) => format!("{utc}+00:00"),
            None => value.to_string(),
        };
        for format in ["%Y-%m-%dT%H:%M%:z", "%Y-%m-%dT%H:%M%z"] {
            if let Ok(zoned) = DateTime::parse_from_str(&value, format) {
                return Some(Moment::Zoned(zoned));
            }
        }
        None
    }

    pub fn plus(self, minutes: i64) -> Moment {
        match self {
            Moment::Day(day) => Moment::Day(day + Duration::days((minutes / 1440).max(0))),
            Moment::Local(local) => Moment::Local(local + Duration::minutes(minutes)),
            Moment::Zoned(zoned) => Moment::Zoned(zoned + Duration::minutes(minutes)),
        }
    }

    pub fn write(self) -> String {
        match self {
            Moment::Day(day) => day.format("%Y-%m-%d").to_string(),
            Moment::Local(local) => local.format("%Y-%m-%dT%H:%M:%S").to_string(),
            Moment::Zoned(zoned) => zoned.to_rfc3339(),
        }
    }

    /// Comparable across the kinds as wall-clock times (an offset as the phone's own is assumed).
    pub fn wall(self) -> NaiveDateTime {
        match self {
            Moment::Day(day) => day.and_hms_opt(0, 0, 0).unwrap_or_default(),
            Moment::Local(local) => local,
            Moment::Zoned(zoned) => zoned.naive_local(),
        }
    }
}

/// The day an event's moment falls on, as the phone wrote it (its first 10 characters: the phone's own date).
fn day_of(value: &str) -> Option<NaiveDate> {
    value.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// « 2026-10-22 10:00–11:00 », « 2026-10-22 all day », « 2026-10-22 09:00 → 2026-10-24 18:00 ».
fn when(event: &Value) -> String {
    let (start, end) = (text(event, "start"), text(event, "end"));
    let all_day = event.get("all_day").map(py::truthy).unwrap_or(false);
    let clock = |v: &str| v.get(11..16).unwrap_or("").to_string();
    let (first, last) = (start.get(..10).unwrap_or(&start), end.get(..10).unwrap_or(&end));
    if all_day {
        return if last.is_empty() || last == first {
            format!("{first} all day")
        } else {
            format!("{first} → {last} all day")
        };
    }
    if last.is_empty() {
        return format!("{first} {}", clock(&start));
    }
    if last == first {
        format!("{first} {}–{}", clock(&start), clock(&end))
    } else {
        format!("{first} {} → {last} {}", clock(&start), clock(&end))
    }
}

fn now_or(now: Option<f64>) -> f64 {
    now.filter(|n| *n != 0.0).unwrap_or_else(py::time_time)
}

fn today(now: f64) -> NaiveDate {
    DateTime::from_timestamp(now as i64, 0).map(|d| d.with_timezone(&chrono::Local).date_naive()).unwrap_or_default()
}

/// The period `calendar_list` reads: from/to (dates), by default today and the next 7 days. Err: why not.
pub fn range(arguments: &Dict, now: Option<f64>) -> Result<(NaiveDate, NaiveDate), String> {
    let arg = |k: &str| tk::strip(&tk::str_or_empty(arguments.get(k))).to_string();
    let from = match arg("from") {
        f if f.is_empty() => today(now_or(now)),
        f => day_of(&f).ok_or("from: a date, YYYY-MM-DD")?,
    };
    let to = match arg("to") {
        t if t.is_empty() => from + Duration::days(DEFAULT_DAYS),
        t => day_of(&t).ok_or("to: a date, YYYY-MM-DD")?,
    };
    Ok((from, to))
}

/// `calendar_add`'s arguments checked: {title, start, end, all_day, calendar, location, notes}. Err: why not.
pub fn new_event(arguments: &Dict) -> Result<Value, String> {
    let arg = |k: &str| tk::strip(&tk::str_or_empty(arguments.get(k))).to_string();
    let title = arg("title");
    if title.is_empty() {
        return Err("a title is needed".into());
    }
    let start =
        Moment::parse(&arg("start")).ok_or("start: an ISO date-time (2026-10-30T10:00) or a date (2026-10-30)")?;
    let end = match arg("end") {
        e if e.is_empty() => {
            let minutes = arguments.get("duration_minutes").and_then(|v| py::py_float(v).ok()).map(|m| m as i64);
            match (start, minutes) {
                (Moment::Day(_), None) => start,
                (_, Some(m)) if m <= 0 || m > 60 * 24 * 31 => return Err("duration_minutes: from 1 to 44640".into()),
                (_, m) => start.plus(m.unwrap_or(DEFAULT_MINUTES)),
            }
        }
        e => Moment::parse(&e).ok_or("end: an ISO date-time or a date")?,
    };
    if end.wall() < start.wall() || (end.wall() == start.wall() && !matches!(start, Moment::Day(_))) {
        return Err("the end must come after the start".into());
    }
    Ok(json!({"title": tk::head(&title, 300), "start": start.write(), "end": end.write(),
              "all_day": matches!(start, Moment::Day(_)),
              "calendar": tk::head(&arg("calendar"), 120), "location": tk::head(&arg("location"), 300),
              "notes": tk::head(&arg("notes"), 2000)}))
}

/// One of the calendar tools, for `agent`: the phone's copy (when the person shared it with the agent) and its
/// mailboxes' calendars (`remote`, read by caldav.rs). The phone's requests go to its queue. Err: a file that cannot
/// be written.
pub fn call(
    config: &Config,
    agent: &str,
    name: &str,
    arguments: &Dict,
    now: Option<f64>,
    remote: &crate::caldav::Remote,
) -> anyhow::Result<Dict> {
    let now = now_or(now);
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = read(config);
    let phone = tk::py_in(agent, data.get("agents").unwrap_or(&json!([])));
    if !phone && !remote.any {
        return Ok(tool_text(
            "The person hasn't shared their phone calendar with you: they switch « Calendrier » on in your page of the \
BipAgents app.",
            true,
        ));
    }
    let mut calendars: Vec<Value> = if phone {
        list_of(&data, "calendars")
            .into_iter()
            .map(|mut c| {
                c["source"] = json!("phone");
                c
            })
            .collect()
    } else {
        Vec::new()
    };
    calendars.extend(remote.calendars.iter().cloned());
    let title_of = |id: &str| -> (String, String) {
        calendars
            .iter()
            .rev()
            .find(|c| text(c, "id") == id)
            .map(|c| (text(c, "title"), text(c, "source")))
            .unwrap_or_else(|| ("?".into(), String::new()))
    };
    let shown = |id: &str| -> String {
        match title_of(id) {
            (title, source) if source.is_empty() => title,
            (title, source) => format!("{title} ({source})"),
        }
    };
    let events = if phone { list_of(&data, "events") } else { Vec::new() };
    let arg = |k: &str| tk::strip(&tk::str_or_empty(arguments.get(k))).to_string();
    if name == "calendar_list" {
        let (from, to) = match range(arguments, Some(now)) {
            Ok(range) => range,
            Err(why) => return Ok(tool_text(&why, true)),
        };
        let wanted = arg("calendar").to_lowercase();
        let mut rows: Vec<&Value> = events
            .iter()
            .chain(remote.events.iter())
            .filter(|e| {
                let Some(start) = day_of(&text(e, "start")) else { return false };
                let end = day_of(&text(e, "end")).unwrap_or(start).max(start);
                let calendar = text(e, "calendar");
                start <= to
                    && end >= from
                    && (wanted.is_empty()
                        || title_of(&calendar).0.to_lowercase() == wanted
                        || shown(&calendar).to_lowercase() == wanted)
            })
            .collect();
        rows.sort_by_key(|e| Moment::parse(&text(e, "start")).map(Moment::wall));
        let mut sources = Vec::new();
        if phone {
            let at = data.get("at").and_then(Value::as_f64);
            let held = match (data.get("from").and_then(Value::as_str), data.get("to").and_then(Value::as_str)) {
                (Some(f), Some(t)) if !f.is_empty() && !t.is_empty() => format!(", for {f} to {t}"),
                _ => String::new(),
            };
            sources.push(match at {
                Some(at) => format!("as the phone shared it {} ago{held}", age(now - at)),
                None => "the phone hasn't shared it yet".to_string(),
            });
        }
        let mut labels: Vec<String> =
            remote.calendars.iter().map(|c| text(c, "source")).filter(|s| !s.is_empty()).collect();
        labels.dedup();
        if !labels.is_empty() {
            sources.push(format!("read now from {}", labels.join(", ")));
        }
        let since = sources.join("; ");
        let failed = if remote.failed.is_empty() {
            String::new()
        } else {
            format!("\n(Not reached: {}.)", remote.failed.join("; "))
        };
        if rows.is_empty() {
            return Ok(tool_text(&format!("No event from {from} to {to} ({since}).{failed}"), false));
        }
        let lines: Vec<String> = rows
            .iter()
            .take(MAX_LISTED)
            .map(|e| {
                let mut line = format!(
                    "- id {} · {} · {} · {}",
                    text(e, "id"),
                    shown(&text(e, "calendar")),
                    when(e),
                    text(e, "title")
                );
                if !text(e, "location").is_empty() {
                    line += &format!(" · at {}", text(e, "location"));
                }
                if e.get("recurring").map(py::truthy).unwrap_or(false) {
                    line += " · repeats";
                }
                if !text(e, "notes").is_empty() {
                    line += &format!("\n  {}", tk::head(&text(e, "notes"), 200));
                }
                line
            })
            .collect();
        let more = if rows.len() > MAX_LISTED {
            format!("\n({} more: ask for a shorter period)", rows.len() - MAX_LISTED)
        } else {
            String::new()
        };
        return Ok(tool_text(&format!("From {from} to {to}, {since}:\n{}{more}{failed}", lines.join("\n")), false));
    }
    if !phone {
        // The mailboxes' calendars are written by caldav.rs: what reaches here is not theirs.
        return Ok(match name {
            "calendar_add" | "calendar_update" | "calendar_delete" => {
                tool_text("unknown event id (see calendar_list)", true)
            }
            _ => tool_text(&format!("unknown tool: {name}"), true),
        });
    }
    let known = |id: &str| events.iter().find(|e| text(e, "id") == id);
    let op = match name {
        "calendar_add" => match new_event(arguments) {
            Ok(event) => event,
            Err(why) => return Ok(tool_text(&why, true)),
        },
        "calendar_update" | "calendar_delete" => {
            let id = arg("id");
            let Some(event) = known(&id) else {
                return Ok(tool_text("unknown event id (see calendar_list)", true));
            };
            if event.get("recurring").map(py::truthy).unwrap_or(false) {
                return Ok(tool_text(
                    "a repeating event: the person changes or deletes it on their phone (tell them)",
                    true,
                ));
            }
            if name == "calendar_delete" {
                json!({"id": id})
            } else {
                let mut args = Dict::new();
                args.insert("id".into(), json!(id));
                for (key, max) in [("title", 300), ("location", 300), ("notes", 2000)] {
                    if arguments.contains_key(key) {
                        args.insert(key.into(), json!(tk::head(&tk::str_or_empty(arguments.get(key)), max)));
                    }
                }
                for key in ["start", "end"] {
                    let value = arg(key);
                    if value.is_empty() {
                        continue;
                    }
                    let Some(moment) = Moment::parse(&value) else {
                        return Ok(tool_text(&format!("{key}: an ISO date-time or a date"), true));
                    };
                    args.insert(key.into(), json!(moment.write()));
                }
                let start = args
                    .get("start")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| text(event, "start"));
                let end =
                    args.get("end").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| text(event, "end"));
                if let (Some(s), Some(e)) = (Moment::parse(&start), Moment::parse(&end)) {
                    if e.wall() < s.wall() {
                        return Ok(tool_text("the end must come after the start", true));
                    }
                }
                if args.len() == 1 {
                    return Ok(tool_text("nothing to change", true));
                }
                Value::Object(args)
            }
        }
        _ => return Ok(tool_text(&format!("unknown tool: {name}"), true)),
    };
    let kind = name.trim_start_matches("calendar_");
    let mut queue = list_of(&data, "queue");
    queue.push(json!({"id": py::token_hex(6), "op": kind, "agent": agent, "created": now, "state": "pending",
                      "args": op}));
    data.insert("queue".into(), Value::Array(queue));
    write(config, &data)?;
    Ok(tool_text(ASKED, false))
}

/// What a request queued for the phone answers (the phone is woken when it starts so).
pub const ASKED: &str =
    "Asked: the person's phone does it at its next sync (usually within minutes, or when the app opens).";

// -- the app's routes --------------------------------------------------------------------------------------------------

fn nested_max_len(errors: &mut Vec<FieldError>, loc: Vec<Value>, value: &str, max: usize) {
    if value.chars().count() > max {
        errors.push(
            FieldError::new(
                "string_too_long",
                loc,
                format!("String should have at most {max} characters"),
                json!(value),
            )
            .with_ctx(json!({"max_length": max})),
        );
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Event {
    pub id: String,
    #[serde(default)]
    pub calendar: String,
    #[serde(default)]
    pub title: String,
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub all_day: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub location: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, deserialize_with = "web::lax::bool", skip_serializing_if = "std::ops::Not::not")]
    pub recurring: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CalendarInfo {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub calendars: Vec<CalendarInfo>,
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Validate for Snapshot {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "from", Some(&self.from), 40);
        web::check_max_len(errors, "to", Some(&self.to), 40);
        check_max_items(errors, "calendars", Some(&self.calendars), 200);
        check_max_items(errors, "events", Some(&self.events), 5000);
        for (n, c) in self.calendars.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("calendars"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &c.id, 300);
            nested_max_len(errors, loc("title"), &c.title, 200);
            nested_max_len(errors, loc("color"), c.color.as_deref().unwrap_or(""), 20);
        }
        for (n, e) in self.events.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("events"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &e.id, 300);
            nested_max_len(errors, loc("calendar"), &e.calendar, 300);
            nested_max_len(errors, loc("title"), &e.title, 1000);
            nested_max_len(errors, loc("start"), &e.start, 40);
            nested_max_len(errors, loc("end"), &e.end, 40);
            nested_max_len(errors, loc("location"), &e.location, 500);
            nested_max_len(errors, loc("notes"), &e.notes, 4000);
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Agents {
    #[serde(default)]
    pub agents: Vec<String>,
}

impl Validate for Agents {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(&self.agents), 50);
    }
}

#[derive(Debug, Deserialize)]
pub struct Done {
    #[serde(deserialize_with = "web::lax::bool")]
    pub done: bool,
    #[serde(default)]
    pub error: Option<String>,
}

impl Validate for Done {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "error", self.error.as_deref(), 300);
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> ApiResult<T> {
    Ok(tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)??)
}

fn pending(op: &Value) -> bool {
    op.get("state").and_then(Value::as_str) == Some("pending")
}

async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let data = blocking(move || Ok(read(&config))).await?;
    let get = |k: &str| data.get(k).cloned().unwrap_or(Value::Null);
    Ok(web::ok(json!({
        "agents": data.get("agents").cloned().unwrap_or(json!([])), "at": get("at"), "from": get("from"), "to": get("to"),
        "calendars": list_of(&data, "calendars").len(), "events": list_of(&data, "events").len(),
        "pending": list_of(&data, "queue").iter().filter(|op| pending(op)).count(),
    })))
}

async fn snapshot(State(state): State<AppState>, Json(body): Json<Snapshot>) -> ApiResult<Response> {
    let config = state.config();
    let events = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        data.insert("calendars".into(), Value::Array(body.calendars.iter().map(dump).collect()));
        data.insert("events".into(), Value::Array(body.events.iter().map(dump).collect()));
        data.insert("from".into(), json!(body.from));
        data.insert("to".into(), json!(body.to));
        data.insert("at".into(), json!(py::time_time()));
        let cutoff = py::time_time() - (QUEUE_DAYS * 86400) as f64;
        let queue: Vec<Value> = list_of(&data, "queue")
            .into_iter()
            .filter(|op| pending(op) || op.get("created").and_then(Value::as_f64).map(|c| c > cutoff).unwrap_or(false))
            .collect();
        data.insert("queue".into(), Value::Array(queue));
        write(&config, &data)?;
        Ok(body.events.len())
    })
    .await?;
    Ok(web::ok(json!({"events": events})))
}

fn dump<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

async fn set_agents(State(state): State<AppState>, Json(body): Json<Agents>) -> ApiResult<Response> {
    let config = state.config();
    let mut names: Vec<String> = Vec::new();
    for a in &body.agents {
        match config.agent(Some(a)) {
            Some(agent) => {
                if !names.contains(&agent.name) {
                    names.push(agent.name.clone())
                }
            }
            None => return Err(HttpError::new(400, "unknown agent")),
        }
    }
    let config_c = config.clone();
    let (before, agents) = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config_c);
        let before = match data.get("agents") {
            Some(Value::Array(v)) => v.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
            _ => Vec::<String>::new(),
        };
        data.insert("agents".into(), json!(names));
        write(&config_c, &data)?;
        Ok((before, names))
    })
    .await?;
    // The agents allowed or no longer: their tools' entry changes (the calendar's tools listed or not).
    let run = crate::agents_admin::default_run();
    for name in agents.iter().filter(|a| !before.contains(a)).chain(before.iter().filter(|a| !agents.contains(a))) {
        if let Some(agent) = config.agent(Some(name)) {
            if let Err(exc) = crate::agent_tools::register(&config, agent, &run).await {
                tracing::warn!(agent = %agent.name, error = %exc, "agent tools not registered");
            }
        }
    }
    Ok(web::ok(json!({"agents": agents})))
}

async fn queue(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let data = blocking(move || Ok(read(&config))).await?;
    let waiting: Vec<Value> = list_of(&data, "queue")
        .iter()
        .filter(|op| pending(op))
        .map(|op| {
            let get = |k: &str| op.get(k).cloned().unwrap_or(Value::Null);
            json!({"id": get("id"), "op": get("op"), "args": get("args"), "agent": get("agent"), "created": get("created")})
        })
        .collect();
    Ok(web::ok(json!({"queue": waiting})))
}

async fn done(State(state): State<AppState>, Path(op_id): Path<String>, Json(body): Json<Done>) -> ApiResult<Response> {
    let config = state.config();
    let found = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let mut queue = list_of(&data, "queue");
        let Some(op) = queue.iter_mut().find(|op| op.get("id").and_then(Value::as_str) == Some(op_id.as_str())) else {
            return Ok(false);
        };
        if let Some(map) = op.as_object_mut() {
            map.insert("state".into(), json!(if body.done { "done" } else { "failed" }));
            map.insert("error".into(), json!(body.error));
            map.insert("finished".into(), json!(py::time_time()));
        }
        data.insert("queue".into(), Value::Array(queue));
        write(&config, &data)?;
        Ok(true)
    })
    .await?;
    if !found {
        return Err(HttpError::new(404, "no such request"));
    }
    Ok(web::ok(json!({"ok": true})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/calendar", get(status).put(snapshot))
        .route("/calendar/agents", put(set_agents))
        .route("/calendar/queue", get(queue))
        .route("/calendar/queue/{op_id}", post(done))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_as_agents_write_them() {
        assert_eq!(Moment::parse("2026-10-30").map(|m| m.write()).as_deref(), Some("2026-10-30"));
        assert_eq!(
            Moment::parse("2026-10-30T10:00").map(|m| m.plus(90).write()).as_deref(),
            Some("2026-10-30T11:30:00")
        );
        assert_eq!(
            Moment::parse("2026-10-30T10:00:00+02:00").map(|m| m.plus(60).write()).as_deref(),
            Some("2026-10-30T11:00:00+02:00")
        );
        assert_eq!(Moment::parse("2026-10-30T23:30Z").map(|m| m.write()).as_deref(), Some("2026-10-30T23:30:00+00:00"));
        assert!(Moment::parse("demain").is_none());
    }

    #[test]
    fn when_in_short() {
        let timed = json!({"start": "2026-10-22T10:00:00+02:00", "end": "2026-10-22T11:00:00+02:00"});
        assert_eq!(when(&timed), "2026-10-22 10:00–11:00");
        let days = json!({"start": "2026-10-22", "end": "2026-10-24", "all_day": true});
        assert_eq!(when(&days), "2026-10-22 → 2026-10-24 all day");
        assert_eq!(when(&json!({"start": "2026-10-22", "end": "2026-10-22", "all_day": true})), "2026-10-22 all day");
    }
}
