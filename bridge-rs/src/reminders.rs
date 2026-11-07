//! The person's iPhone Reminders, for the agents they choose (decided 2026-10-20). No server reaches them (Apple has no
//! API for them), so the iPhone app shares them: it sends its lists and reminders here when it opens (and when woken by
//! a silent push), and applies what the agents asked for, through EventKit. An agent reads the last copy and asks for
//! a reminder to be added or ticked off; the request waits here until the iPhone has done it. When no agent may use
//! them any more, the copy is deleted (as the address book's).
//!
//!     reminders_list       the reminders as the iPhone last sent them (not done, and done in the last 30 days)
//!     reminders_add        a new reminder (a title, a date, a list, or a place: « when I get to… »), done by the phone
//!                          at its next sync (an Android phone does only those with a place, as its own « rappels par
//!                          lieu »: Android has no Reminders app)
//!     reminders_complete   a reminder ticked off, the same way
//!
//! App routes (the bridge key):
//!
//!     GET  /v1/reminders                 {agents, rights: {agent: {read, write}}, at, lists: n, items: n}
//!     PUT  /v1/reminders                 {lists: [{id, title}], items: [{id, list, title, notes, due, completed}]}
//!     PUT  /v1/reminders/agents          {agents}
//!     PUT  /v1/reminders/rights          {agent, rights: {read?, write?}}: what that agent may do (phone_rights.rs)
//!     PUT  /v1/reminders/android         {calendar, items: [{id, title, due}]}: the Android app's reminders at a time,
//!                                        events in its calendar, and whether it may write there
//!     GET  /v1/reminders/queue           the agents' requests waiting for the iPhone (`?phone=android`: for the
//!                                        Android app: those marked `phone` « android », and the reminders by place)
//!     POST /v1/reminders/queue/{id}      {done: bool, error?} (error « unknown request », from an app older than the
//!                                        request's op: it stays waiting for the app's update)
//!
//!     data_dir/reminders.json   (chmod 600: the copy, the agents allowed, the requests)

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::calendar::Moment;
use crate::config::Config;
use crate::phone_rights::{self, Rights};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

/// Python's `LOCK` (a thread lock: the routes and the tools run in threads).
static LOCK: Mutex<()> = Mutex::new(());
pub const KEPT_DONE: usize = 200;
pub const QUEUE_DAYS: i64 = 14;
pub const REMINDER_TOOL_NAMES: [&str; 3] = ["reminders_list", "reminders_add", "reminders_complete"];

/// `REMINDER_TOOLS`: the MCP tool definitions (reminders_list, reminders_add, reminders_complete).
pub fn reminder_tools() -> Vec<Value> {
    vec![
        json!({
            "name": "reminders_list",
            "description": "The person's reminders as their iPhone last shared them (its Reminders app): open ones, and \
        those done in the last 30 days, with the ones you asked for that it hasn't added yet. Optional: a list's name, or \
        include_done.",
            "inputSchema": {"type": "object", "properties": {"list": {"type": "string"}, "include_done": {"type": "boolean"}}},
        }),
        json!({
            "name": "reminders_add",
            "description": "Add a reminder for the person, done by their phone at its next sync (usually within minutes): \
        at a time or on a day (due), or by place (« when I get to the supermarket… »). The date and time always go in \
        due, never only in the title or the notes. A reminder at a time or on a day goes into an iPhone's Reminders app; \
        a reminder by place is done by an iPhone or an Android phone. This tool's answer says what happened: tell the \
        person that, never steps to do it by hand on a phone. list: a list's name, optional. By place: place, the \
        place's name or address as the person says it (their phone finds it), latitude and longitude only when you \
        know them, when: arriving (default) or leaving, radius in metres (default 150).",
            "inputSchema": {"type": "object", "properties": {
                "title": {"type": "string", "description": "what to remind them of, without the date or time"},
                "due": {"type": "string", "description": "when, in the person's local time, no time zone: \
        2026-11-07T14:00 for a time (they are alerted then), 2026-11-07 for a day without a time"},
                "list": {"type": "string"}, "notes": {"type": "string"},
                "place": {"type": "string"}, "latitude": {"type": "number"},
                "longitude": {"type": "number"}, "radius": {"type": "number"},
                "when": {"type": "string", "enum": ["arriving", "leaving"],
                         "description": "for a reminder by place only: on arriving at it or on leaving it"}},
                "required": ["title"], "additionalProperties": false},
        }),
        json!({
            "name": "reminders_complete",
            "description": "Tick off one of the person's reminders (its id from reminders_list).",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]},
        }),
    ]
}

fn path_of(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("reminders.json")
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

/// `_age(seconds)`: « 12 min », « 3 h », « 2 days ».
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

/// `wake()`: asks the phone to sync (a silent push).
pub type Wake = std::sync::Arc<dyn Fn() + Send + Sync>;

fn list_of(data: &Dict, key: &str) -> Vec<Value> {
    match data.get(key) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn field<'a>(item: &'a Value, key: &str) -> Option<&'a Value> {
    item.as_object().and_then(|m| m.get(key))
}

/// One of the reminder tools, for `agent`. Err: a file that cannot be written (Python's OSError).
pub fn call(
    config: &Config,
    agent: &str,
    name: &str,
    arguments: &Dict,
    wake: Option<&Wake>,
    now: Option<f64>,
) -> anyhow::Result<Dict> {
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(py::time_time);
    let said = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(config);
        if !tk::py_in(agent, data.get("agents").unwrap_or(&json!([]))) {
            // The same words for both phones: the turn may come from either, and the « Rappels » tool is on the agent's
            // page in both apps (7/11, Android: the agent gave iPhone steps from « on an iPhone: Settings › Rappels »).
            return Ok(tool_text(
                "The person hasn't shared their reminders with you, so nothing was done. They can turn on the « Rappels » \
                 tool on your page in the BipAgents app: tell them so in one sentence, without steps for a particular \
                 phone.",
                true,
            ));
        }
        // What the person lets this agent do (phone_rights.rs): reading them, adding and ticking them off.
        let (right, what) = if name == "reminders_list" {
            (crate::phone_rights::READ, "read their reminders")
        } else {
            (crate::phone_rights::WRITE, "add or tick off their reminders")
        };
        if REMINDER_TOOL_NAMES.contains(&name) && !crate::phone_rights::allows(&data, agent, right) {
            return Ok(tool_text(&crate::phone_rights::refused(what), true));
        }
        let lists: Vec<(Value, Value)> = list_of(&data, "lists")
            .iter()
            .filter_map(|l| Some((field(l, "id")?.clone(), field(l, "title")?.clone())))
            .collect();
        let list_title = |id: Option<&Value>| -> Option<Value> {
            let id = id?;
            lists.iter().rev().find(|(k, _)| k == id).map(|(_, t)| t.clone())
        };
        let items = list_of(&data, "items");
        // The Android phone's own: its reminders at a time, events in its calendar (`PUT /v1/reminders/android`).
        let android = android_items(&data);
        if name == "reminders_list" {
            let wanted = tk::str_or_empty(arguments.get("list")).to_lowercase();
            let done = arguments.get("include_done").map(py::truthy).unwrap_or(false);
            let rows: Vec<&Value> = items
                .iter()
                .filter(|i| {
                    (done || !field(i, "completed").map(py::truthy).unwrap_or(false))
                        && (wanted.is_empty()
                            || list_title(field(i, "list")).map(|t| py::py_str(&t)).unwrap_or_default().to_lowercase()
                                == wanted)
                })
                .collect();
            let at = data.get("at").cloned().unwrap_or(Value::Null);
            // What this agent asked to add and the copy doesn't show yet (phone_asks.rs): listed, marked.
            let queue = list_of(&data, "queue");
            let asked: Vec<String> = crate::phone_asks::unseen_adds(&queue, agent, at.as_f64())
                .into_iter()
                .filter_map(|op| {
                    let args = field(op, "args")?;
                    let text = |k: &str| field(args, k).map(py::py_str).unwrap_or_default();
                    let list = text("list");
                    if !wanted.is_empty() && list.to_lowercase() != wanted {
                        return None;
                    }
                    let list = if list.is_empty() { "default list".to_string() } else { list };
                    let mut line = format!("- id {} · {list} · {}", field(op, "id").map(py::py_str)?, text("title"));
                    if !text("due").is_empty() {
                        line += &format!(" · due {}", text("due"));
                    }
                    let phone = doer(op);
                    line += &if crate::phone_asks::waiting(op) {
                        format!(" · asked by you, not on the {phone} yet (added at its next sync)")
                    } else {
                        format!(" · added by the {phone} just now (not in this copy yet)")
                    };
                    Some(line)
                })
                .collect();
            let in_calendar: Vec<String> = if wanted.is_empty() {
                android
                    .iter()
                    .take(200)
                    .map(|i| {
                        let text = |k: &str| field(i, k).map(py::py_str).unwrap_or_default();
                        let mut line = format!("- id {} · {}", text("id"), text("title"));
                        if !text("due").is_empty() {
                            line += &format!(" · due {}", text("due"));
                        }
                        line
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let in_calendar = if in_calendar.is_empty() {
                String::new()
            } else {
                format!("\nIn the Android phone's calendar (the reminders you added there, to come):\n{}", in_calendar.join("\n"))
            };
            if rows.is_empty() && !asked.is_empty() {
                return Ok(tool_text(&format!("Not in the phone's copy yet:\n{}{in_calendar}", asked.join("\n")), false));
            }
            let asked = if asked.is_empty() { String::new() } else { format!("\n{}", asked.join("\n")) };
            if rows.is_empty() && !in_calendar.is_empty() {
                return Ok(tool_text(in_calendar.trim_start(), false));
            }
            if rows.is_empty() {
                let since = if py::truthy(&at) {
                    format!(" (as of {} ago)", age(now - py::py_float(&at).unwrap_or(now)))
                } else {
                    String::new()
                };
                return Ok(tool_text(&format!("No reminder.{since}"), false));
            }
            let lines: Vec<String> = rows
                .iter()
                .take(200)
                .map(|i| {
                    let get = |k: &str| field(i, k).cloned().unwrap_or(Value::Null);
                    let list = list_title(field(i, "list")).map(|t| py::py_str(&t)).unwrap_or_else(|| "?".into());
                    let mut line = format!("- id {} · {list} · {}", py::py_str(&get("id")), py::py_str(&get("title")));
                    if py::truthy(&get("due")) {
                        line += &format!(" · due {}", py::py_str(&get("due")));
                    }
                    if py::truthy(&get("completed")) {
                        line += " · done";
                    }
                    if py::truthy(&get("notes")) {
                        line += &format!("\n  {}", tk::head(&py::py_str(&get("notes")), 200));
                    }
                    line
                })
                .collect();
            let shared = if py::truthy(&at) { py::py_float(&at).unwrap_or(now) } else { now };
            return Ok(tool_text(
                &format!("As the iPhone shared them {} ago:\n{}{asked}{in_calendar}", age(now - shared), lines.join("\n")),
                false,
            ));
        }
        let (op, said) = if name == "reminders_add" {
            // An argument the schema doesn't have is refused, not dropped: a time given as `time` or `date` was lost
            // (the reminder then came without it).
            let unknown: Vec<&str> =
                arguments.keys().map(String::as_str).filter(|k| !ADD_ARGUMENTS.contains(k)).collect();
            if !unknown.is_empty() {
                return Ok(tool_text(
                    &format!(
                        "Not added: unknown argument {}. The date and time go together in due (2026-11-07T14:00, \
                         or 2026-11-07 for a day); the others are {}.",
                        unknown.join(", "),
                        ADD_ARGUMENTS.join(", ")
                    ),
                    true,
                ));
            }
            let title = tk::strip(&tk::str_or_empty(arguments.get("title"))).to_string();
            if title.is_empty() {
                return Ok(tool_text("a title is needed", true));
            }
            let due = match due_arg(&tk::str_or_empty(arguments.get("due"))) {
                Ok(due) => due,
                Err(why) => return Ok(tool_text(why, true)),
            };
            let mut args = json!({"title": tk::head(&title, 300), "due": due.map(|d| d.write()).unwrap_or_default(),
                                  "list": tk::head(&tk::str_or_empty(arguments.get("list")), 120),
                                  "notes": tk::head(&tk::str_or_empty(arguments.get("notes")), 2000)});
            let by_place = match place_args(arguments) {
                Ok(Some(Value::Object(place))) => {
                    if let Some(args) = args.as_object_mut() {
                        args.extend(place);
                    }
                    true
                }
                Ok(_) => false,
                Err(why) => return Ok(tool_text(why, true)),
            };
            // A reminder at a time or on a day: in an iPhone's Reminders app when one shares them here, else in the
            // Android phone's calendar (Alex, 7/11: « Dans l'agenda »), never both. With neither, it would wait forever
            // while the agent told the person « c'est noté pour 14 h » (7/11): not asked, and the agent told why.
            let phone = if by_place { Phone::Any } else { phone_for(&data, now) };
            if phone == Phone::None {
                return Ok(tool_text(NO_PHONE, true));
            }
            if due.is_none() && matches!(phone, Phone::Android | Phone::AndroidWaiting) {
                return Ok(tool_text(
                    "Not added: on the person's Android phone a reminder goes in the calendar, so it needs a date (due, \
                     with the time if any). Ask them when, or make it a reminder by place.",
                    true,
                ));
            }
            // The same reminder (its title and date) open in the copy, or asked a moment ago: not asked again.
            let text = |v: &Value, k: &str| field(v, k).map(py::py_str).unwrap_or_default();
            let same = |v: &Value| {
                crate::phone_asks::folded(&text(v, "title")) == crate::phone_asks::folded(&title)
                    && same_due(&text(v, "due"), &text(&args, "due"))
            };
            if let Some(open) =
                items.iter().chain(&android).find(|i| same(i) && !field(i, "completed").map(py::truthy).unwrap_or(false))
            {
                return Ok(tool_text(
                    &format!("Already in the reminders: id {} · {}. Not added again.", text(open, "id"), text(open, "title")),
                    false,
                ));
            }
            let queue = list_of(&data, "queue");
            if let Some(twin) = crate::phone_asks::twin(&queue, agent, now, same) {
                return Ok(tool_text(
                    &format!(
                        "Already asked: id {} · {} ({}). Not added again: nothing more to do for this reminder.",
                        text(twin, "id"),
                        title,
                        if crate::phone_asks::waiting(twin) {
                            format!("the {} adds it at its next sync", doer(twin))
                        } else {
                            format!("the {} added it a moment ago", doer(twin))
                        }
                    ),
                    false,
                ));
            }
            let said = asked_add(due, phone, &text(&args, "place"), text(&args, "when") == "leaving");
            let mut op =
                json!({"id": py::token_hex(6), "op": "add", "agent": agent, "created": now, "state": "pending", "args": args});
            if phone == Phone::Android || phone == Phone::AndroidWaiting {
                op["phone"] = json!(ANDROID);
            }
            (op, said)
        } else if name == "reminders_complete" {
            let reminder = tk::str_or_empty(arguments.get("id"));
            let is = |i: &Value| field(i, "id").and_then(Value::as_str) == Some(reminder.as_str());
            let mut op = json!({"id": py::token_hex(6), "op": "complete", "agent": agent, "created": now,
                                "state": "pending", "args": {"id": reminder}});
            if items.iter().any(is) {
                (op, ASKED.to_string())
            } else if android.iter().any(is) {
                // In the Android phone's calendar: the event deleted while it is to come (an event past keeps its
                // place in the calendar, its alert taken off).
                op["phone"] = json!(ANDROID);
                (op, "Asked: the Android phone takes it off its calendar at its next sync (usually within minutes).".into())
            } else {
                return Ok(tool_text("unknown reminder id (see reminders_list)", true));
            }
        } else {
            return Ok(tool_text(&format!("unknown tool: {name}"), true));
        };
        let mut queue = match data.get("queue") {
            Some(Value::Array(q)) => q.clone(),
            _ => Vec::new(),
        };
        queue.push(op);
        data.insert("queue".into(), Value::Array(queue));
        write(config, &data)?;
        said
    };
    if let Some(wake) = wake {
        wake();
    }
    Ok(tool_text(&said, false))
}

/// `reminders_add`'s arguments (its schema's): any other is refused.
pub const ADD_ARGUMENTS: [&str; 9] = ["title", "due", "list", "notes", "place", "latitude", "longitude", "radius", "when"];

/// What a request asked of the phone answers when nothing more precise is known.
const ASKED: &str = "Asked: the person's phone does it at its next sync (usually within minutes, or when the app opens).";

/// The answer to a reminder at a time or on a day when no phone can do it: no iPhone shares its Reminders here, and
/// no Android app (from 7/11 on) said it takes them. Plainly not done, and nothing about another phone's steps.
pub const NO_PHONE: &str = "Not added: none of the person's phones takes reminders at a time from you yet (no iPhone \
    shares its Reminders with you, and BipAgents on their Android phone hasn't synced them: an update of the app puts \
    them in its calendar). Tell the person plainly that this reminder isn't set; don't say it is, and don't give them \
    steps for a phone. If they want, you can offer to write to them at that time with a one-time routine \
    (routine_schedule), or a reminder by place.";

/// The request's `phone` for the Android app (`GET /v1/reminders/queue?phone=android`): one only it does (an iPhone
/// never sees it), at a time or on a day in its calendar, or ticked off there.
pub const ANDROID: &str = "android";

/// The phone that does a reminder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phone {
    /// An iPhone, in its Reminders app (every request without `phone`).
    Iphone,
    /// The Android phone, in its calendar.
    Android,
    /// The Android phone, once the person lets BipAgents use its calendar.
    AndroidWaiting,
    /// A reminder by place: whichever phone takes it.
    Any,
    /// None can.
    None,
}

/// Who does a reminder at a time or on a day: an iPhone sharing its Reminders here, else the Android phone that said
/// it puts them in its calendar (`PUT /v1/reminders/android`, less than `IPHONE_DAYS` old), else none.
pub fn phone_for(data: &Dict, now: f64) -> Phone {
    if iphone_syncs(data, now) {
        return Phone::Iphone;
    }
    let android = data.get(ANDROID);
    let at = android.and_then(|a| a.get("at")).and_then(Value::as_f64);
    if !at.is_some_and(|at| now - at < IPHONE_DAYS * 86400.0) {
        return Phone::None;
    }
    if android.and_then(|a| a.get("calendar")).and_then(Value::as_bool) == Some(true) {
        Phone::Android
    } else {
        Phone::AndroidWaiting
    }
}

/// The Android phone's reminders: the events it added to its calendar for the agents, to come.
fn android_items(data: &Dict) -> Vec<Value> {
    match data.get(ANDROID).and_then(|a| a.get("items")) {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// How long an iPhone's copy shows that it does the reminders (it shares it each time the app opens, and when a
/// silent push wakes it): past that, it is taken as gone (the person moved to another phone).
pub const IPHONE_DAYS: f64 = 30.0;

/// Whether an iPhone shares its Reminders here (its copy, `PUT /v1/reminders`, less than `IPHONE_DAYS` old): the only
/// phone that does a reminder at a time or on a day.
pub fn iphone_syncs(data: &Dict, now: f64) -> bool {
    data.get("at").and_then(Value::as_f64).is_some_and(|at| now - at < IPHONE_DAYS * 86400.0)
}

/// `due` as an agent wrote it, checked and written as the phones read it: a day (2026-11-07), a local time
/// (2026-11-07T14:00:00, the person's own: the iPhone puts it in its time zone) or one with its offset. None: no due.
pub fn due_arg(due: &str) -> Result<Option<Moment>, &'static str> {
    let due = due.trim();
    if due.is_empty() {
        return Ok(None);
    }
    Moment::parse(due).map(Some).ok_or(
        "due: a date and time in the person's local time (2026-11-07T14:00) or a day (2026-11-07); not added",
    )
}

/// Two dues the same moment (the copy writes it with the phone's offset, a request without one).
fn same_due(a: &str, b: &str) -> bool {
    match (Moment::parse(a), Moment::parse(b)) {
        (Some(a), Some(b)) => a.wall() == b.wall() && matches!(a, Moment::Day(_)) == matches!(b, Moment::Day(_)),
        _ => a.trim() == b.trim(),
    }
}

/// What the agent is told of a reminder asked: where it lands, and when it alerts.
fn asked_add(due: Option<Moment>, phone: Phone, place: &str, leaving: bool) -> String {
    let at = due.map(|due| match due {
        Moment::Day(day) => (day.format("%Y-%m-%d").to_string(), false),
        moment => (moment.wall().format("%Y-%m-%d %H:%M").to_string(), true),
    });
    let next = "at its next sync (usually within minutes, or when the app opens)";
    match phone {
        Phone::Any | Phone::None => {
            let side = if leaving { "leaving" } else { "arriving at" };
            let place = if place.is_empty() { "the place".to_string() } else { format!("« {place} »") };
            let when = at.map(|(at, _)| format!(", due {at}")).unwrap_or_default();
            format!("Asked: the person's phone reminds them on {side} {place}{when}; it takes the request {next}.")
        }
        Phone::Iphone => {
            let when = match at {
                Some((at, true)) => format!(" for {at}, with an alert at that time"),
                Some((day, false)) => format!(" for {day} (a day, no time)"),
                None => String::new(),
            };
            format!("Asked: it goes into the Reminders app of their iPhone{when}, {next}.")
        }
        Phone::Android => {
            let when = match at {
                Some((at, true)) => format!(" at {at}, with an alert at that time"),
                Some((day, false)) => format!(" on {day}, all day, with an alert at 9:00"),
                None => String::new(),
            };
            format!(
                "Asked: their Android phone puts it in their calendar{when}, {next}. Tell them it is in their \
                 calendar (« dans ton agenda »), with an alert."
            )
        }
        Phone::AndroidWaiting => format!(
            "Asked, but not set yet: their Android phone puts reminders in its calendar, and BipAgents isn't allowed \
             to use the calendar there. It waits until they allow it: the app asks them (its « Rappels » page). Tell \
             them plainly it isn't in their calendar yet and why, without steps for another phone{}.",
            at.map(|(at, _)| format!("; then it goes in at {at}")).unwrap_or_default()
        ),
    }
}

/// Which of the person's phones does a request, as the agent is told: a reminder by place, whichever they have (both
/// do those); one for the Android phone's calendar, it; the rest, their iPhone (7/11: the agent was told « the
/// person's iPhone » for a reminder by place made on an Android phone).
fn doer(op: &Value) -> &'static str {
    if field(op, "args").and_then(|a| field(a, "place")).is_some() {
        "phone"
    } else if field(op, "phone").and_then(Value::as_str) == Some(ANDROID) {
        "Android phone"
    } else {
        "iPhone"
    }
}

/// A request for the phone added by the bridge itself (the Carnet's dated tasks, notebook/mod.rs): `op` « add »,
/// « complete » or « remove » (args: the reminder's `id` when known, and the task's `mark`: the phone removes only a
/// reminder whose notes carry it), its args as the phone reads them. Returns the request's id. The caller may hold the notebook's lock
/// (taken before this one, never after).
pub fn enqueue(config: &Config, agent: &str, op: &str, args: Value) -> std::io::Result<String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = read(config);
    let id = py::token_hex(6);
    let mut queue = list_of(&data, "queue");
    queue.push(
        json!({"id": id, "op": op, "agent": agent, "created": py::time_time(), "state": "pending", "args": args}),
    );
    data.insert("queue".into(), Value::Array(queue));
    write(config, &data)?;
    Ok(id)
}

/// `enqueue`, unless the same request (same op, same args) is already waiting or was done: the cleanups that ask for
/// it may run again before the phone shares its reminders anew. Returns the new request's id, None when not asked.
pub fn enqueue_once(config: &Config, agent: &str, op: &str, args: Value) -> std::io::Result<Option<String>> {
    {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let asked = list_of(&read(config), "queue").iter().any(|o| {
            field(o, "op").and_then(Value::as_str) == Some(op)
                && field(o, "args") == Some(&args)
                && matches!(field(o, "state").and_then(Value::as_str), Some("pending" | "done"))
        });
        if asked {
            return Ok(None);
        }
    }
    enqueue(config, agent, op, args).map(Some)
}

/// What an app older than a request's op answers (`RemindersSync.apply`): the request stays waiting, done once the
/// person has updated the app.
pub const UNKNOWN_REQUEST: &str = "unknown request";

/// A request still waiting for the phone, cancelled (it never reaches it). Returns whether it was waiting.
pub fn cancel_pending(config: &Config, op_id: &str) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = read(config);
    let mut queue = list_of(&data, "queue");
    let mut found = false;
    for op in queue.iter_mut() {
        if field(op, "id").and_then(Value::as_str) == Some(op_id)
            && field(op, "state").and_then(Value::as_str) == Some("pending")
        {
            if let Some(map) = op.as_object_mut() {
                map.insert("state".into(), json!("cancelled"));
                map.insert("finished".into(), json!(py::time_time()));
            }
            found = true;
        }
    }
    if found {
        data.insert("queue".into(), Value::Array(queue));
        write(config, &data)?;
    }
    Ok(found)
}

/// A reminder by place's arguments, as the phone reads them (strings, like the rest of a request's args): `place` (a
/// name the phone looks up when there are no coordinates), `latitude`/`longitude`, `radius` in metres and `when`
/// (« arriving » or « leaving »). None without a place; Err: why the agent's arguments don't make one.
pub fn place_args(arguments: &Dict) -> Result<Option<Value>, &'static str> {
    const NUMBERS: &str = "latitude, longitude and radius are numbers";
    let number = |key: &str| -> Result<Option<f64>, &'static str> {
        match arguments.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Number(n)) => Ok(n.as_f64()),
            Some(Value::String(t)) if t.trim().is_empty() => Ok(None),
            Some(Value::String(t)) => t.trim().parse::<f64>().map(Some).map_err(|_| NUMBERS),
            Some(_) => Err(NUMBERS),
        }
    };
    let name = tk::strip(&tk::str_or_empty(arguments.get("place"))).to_string();
    let (latitude, longitude) = (number("latitude")?, number("longitude")?);
    let when = tk::strip(&tk::str_or_empty(arguments.get("when"))).to_lowercase();
    if name.is_empty() && latitude.is_none() && longitude.is_none() {
        return if when.is_empty() { Ok(None) } else { Err("a reminder by place needs a place (its name or address)") };
    }
    if latitude.is_some() != longitude.is_some() {
        return Err("give both latitude and longitude, or only the place's name");
    }
    if let (Some(lat), Some(lon)) = (latitude, longitude) {
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return Err("latitude is between -90 and 90, longitude between -180 and 180");
        }
    }
    let when = match when.as_str() {
        "" | "arriving" | "arrive" | "enter" => "arriving",
        "leaving" | "leave" | "exit" => "leaving",
        _ => return Err("when is « arriving » or « leaving »"),
    };
    // The phones watch a circle: under 100 m it is easily missed, over 5 km it means little.
    let radius = number("radius")?.unwrap_or(150.0).clamp(100.0, 5000.0);
    let mut place =
        json!({"place": tk::head(&name, 200), "when": when, "radius": format!("{}", radius.round() as i64)});
    if let (Some(lat), Some(lon), Some(map)) = (latitude, longitude, place.as_object_mut()) {
        map.insert("latitude".into(), json!(format!("{lat:.6}")));
        map.insert("longitude".into(), json!(format!("{lon:.6}")));
    }
    Ok(Some(place))
}

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
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub list: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub due: String,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub completed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReminderList {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub lists: Vec<ReminderList>,
    #[serde(default)]
    pub items: Vec<Item>,
}

impl Validate for Snapshot {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "lists", Some(&self.lists), 200);
        check_max_items(errors, "items", Some(&self.items), 5000);
        for (n, l) in self.lists.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("lists"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &l.id, 200);
            nested_max_len(errors, loc("title"), &l.title, 200);
        }
        for (n, i) in self.items.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("items"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &i.id, 200);
            nested_max_len(errors, loc("list"), &i.list, 200);
            nested_max_len(errors, loc("title"), &i.title, 1000);
            nested_max_len(errors, loc("notes"), &i.notes, 4000);
            nested_max_len(errors, loc("due"), &i.due, 40);
        }
    }
}

/// One of the Android phone's reminders: an event it added to its calendar (`due` with the phone's offset, or a day).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AndroidItem {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub due: String,
}

/// `PUT /v1/reminders/android`: what the Android app does with reminders at a time. `calendar`: whether it may write
/// to a calendar (the permission, and a calendar synced with an account); `items`: its reminders to come.
#[derive(Debug, Deserialize)]
pub struct AndroidCopy {
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub calendar: bool,
    #[serde(default)]
    pub items: Vec<AndroidItem>,
}

impl Validate for AndroidCopy {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "items", Some(&self.items), 1000);
        for (n, i) in self.items.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("items"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &i.id, 200);
            nested_max_len(errors, loc("title"), &i.title, 1000);
            nested_max_len(errors, loc("due"), &i.due, 40);
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

async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let data = blocking(move || Ok(read(&config))).await?;
    let count = |k: &str| list_of(&data, k).len();
    let pending = list_of(&data, "queue")
        .iter()
        .filter(|op| field(op, "state").and_then(Value::as_str) == Some("pending"))
        .count();
    Ok(web::ok(json!({
        "agents": data.get("agents").cloned().unwrap_or(json!([])), "at": data.get("at").cloned().unwrap_or(Value::Null),
        "rights": crate::phone_rights::listed(&data, &crate::phone_rights::REMINDERS),
        "lists": count("lists"), "items": count("items"), "pending": pending,
    })))
}

async fn snapshot(State(state): State<AppState>, Json(body): Json<Snapshot>) -> ApiResult<Response> {
    let config = state.config();
    let items = blocking(move || {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let dump = |i: &Item| serde_json::to_value(i).unwrap_or(Value::Null);
        let done: Vec<Value> = body.items.iter().filter(|i| i.completed).take(KEPT_DONE).map(dump).collect();
        let mut kept: Vec<Value> = body.items.iter().filter(|i| !i.completed).map(dump).collect();
        kept.extend(done);
        let lists: Vec<Value> = body.lists.iter().map(|l| serde_json::to_value(l).unwrap_or(Value::Null)).collect();
        data.insert("lists".into(), Value::Array(lists));
        data.insert("items".into(), Value::Array(kept));
        data.insert("at".into(), json!(py::time_time()));
        let cutoff = py::time_time() - (QUEUE_DAYS * 86400) as f64;
        let queue: Vec<Value> = list_of(&data, "queue")
            .into_iter()
            .filter(|op| {
                field(op, "state").and_then(Value::as_str) == Some("pending")
                    || field(op, "created").and_then(Value::as_f64).map(|c| c > cutoff).unwrap_or(false)
            })
            .collect();
        data.insert("queue".into(), Value::Array(queue));
        write(&config, &data)?;
        drop(guard);
        // The Carnet's tasks follow their reminders ticked on the phone (and the other way round).
        let items = list_of(&data, "items");
        crate::notebook::reminders_synced(&config, &items);
        Ok(items.len())
    })
    .await?;
    Ok(web::ok(json!({"items": items})))
}

/// A deleted agent: no longer among the agents allowed (the copy goes with the last one, `set_agents_in`).
pub fn forget(config: &Config, agent: &str) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = read(config);
    let names: Vec<String> = match data.get("agents") {
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => return,
    };
    if !names.iter().any(|n| n == agent) {
        return;
    }
    let kept: Vec<String> = names.into_iter().filter(|n| n != agent).collect();
    set_agents_in(&mut data, &kept);
    if let Err(err) = write(config, &data) {
        tracing::warn!(agent = %agent, error = %err, "reminders share not updated for a deleted agent");
    }
}

/// The agents allowed now. Shared with no agent any more, the copy goes too, as the address book's (contacts.rs): the
/// lists and reminders with their notes are not kept once nobody may read them (legal review of 2026-11-01, C4); the
/// requests stay their 14 days (the Carnet's own included). The phone stops sending it.
fn set_agents_in(data: &mut Dict, names: &[String]) {
    data.insert("agents".into(), json!(names));
    if names.is_empty() {
        for key in ["lists", "items", "at", ANDROID] {
            data.remove(key);
        }
    }
}

async fn set_agents(State(state): State<AppState>, Json(body): Json<Agents>) -> ApiResult<Response> {
    let config = state.config();
    let mut names = Vec::new();
    for a in &body.agents {
        match config.agent(Some(a)) {
            Some(agent) => names.push(agent.name.clone()),
            None => return Err(HttpError::new(400, "unknown agent")),
        }
    }
    let agents = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        set_agents_in(&mut data, &names);
        write(&config, &data)?;
        Ok(data["agents"].clone())
    })
    .await?;
    Ok(web::ok(json!({"agents": agents})))
}

/// What one agent may do with the reminders (phone_rights.rs); its requests waiting are cancelled when it may no
/// longer change them.
async fn set_rights(State(state): State<AppState>, Json(body): Json<Rights>) -> ApiResult<Response> {
    let config = state.config();
    let Some(agent) = config.agent(Some(&body.agent)).map(|a| a.name.clone()) else {
        return Err(HttpError::new(400, "unknown agent"));
    };
    let name = agent.clone();
    let rights = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let (rights, stopped) = phone_rights::set(&mut data, &name, &phone_rights::REMINDERS, &body.rights);
        if stopped {
            crate::phone_rights::cancel_requests(&mut data, &name);
        }
        write(&config, &data)?;
        Ok(rights)
    })
    .await?;
    Ok(web::ok(json!({"agent": agent, "rights": rights})))
}

async fn android_copy(State(state): State<AppState>, Json(body): Json<AndroidCopy>) -> ApiResult<Response> {
    let config = state.config();
    let items = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        // Shared with no agent: nothing kept (the copy goes with the last one, `set_agents_in`).
        if list_of(&data, "agents").is_empty() {
            return Ok(0);
        }
        let items: Vec<Value> = body.items.iter().map(|i| serde_json::to_value(i).unwrap_or(Value::Null)).collect();
        let count = items.len();
        data.insert(ANDROID.into(), json!({"at": py::time_time(), "calendar": body.calendar, "items": items}));
        write(&config, &data)?;
        Ok(count)
    })
    .await?;
    Ok(web::ok(json!({"items": items})))
}

#[derive(Debug, Deserialize)]
pub struct QueueQuery {
    #[serde(default)]
    pub phone: String,
}

/// The requests waiting for a phone: for the Android app (`?phone=android`), those it does (`phone` « android », and
/// the reminders by place); for an iPhone (and an Android app from before 7/11, which keeps those by place), all but
/// the Android phone's.
async fn queue(State(state): State<AppState>, query: axum::extract::Query<QueueQuery>) -> ApiResult<Response> {
    let config = state.config();
    let android = query.phone == ANDROID;
    let data = blocking(move || Ok(read(&config))).await?;
    let mut waiting = Vec::new();
    for op in list_of(&data, "queue") {
        if field(&op, "state").and_then(Value::as_str) != Some("pending") {
            continue;
        }
        let for_android = field(&op, "phone").and_then(Value::as_str) == Some(ANDROID);
        let by_place = field(&op, "args").and_then(|a| a.get("place")).is_some();
        if for_android != android && !(android && by_place) {
            continue;
        }
        let mut out = Dict::new();
        for k in ["id", "op", "args", "agent", "created"] {
            // Python's op[k]: a request the bridge wrote has them all
            out.insert(k.into(), field(&op, k).cloned().ok_or_else(|| anyhow::anyhow!("KeyError: '{k}'"))?);
        }
        if for_android {
            out.insert("phone".into(), json!(ANDROID));
        }
        waiting.push(Value::Object(out));
    }
    Ok(web::ok(json!({"queue": waiting})))
}

async fn done(State(state): State<AppState>, Path(op_id): Path<String>, Json(body): Json<Done>) -> ApiResult<Response> {
    let config = state.config();
    let found = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let mut queue = list_of(&data, "queue");
        for op in queue.iter_mut() {
            if field(op, "id").and_then(Value::as_str) == Some(op_id.as_str()) {
                // an app too old for this request: kept waiting for its update (tried again at each sync)
                let later = !body.done && body.error.as_deref().map(str::trim) == Some(UNKNOWN_REQUEST);
                if let Some(map) = op.as_object_mut() {
                    if later {
                        map.insert("error".into(), json!(body.error));
                        map.insert("tried".into(), json!(py::time_time()));
                        data.insert("queue".into(), Value::Array(queue));
                        write(&config, &data)?;
                        return Ok(true);
                    }
                    map.insert("state".into(), json!(if body.done { "done" } else { "failed" }));
                    map.insert("error".into(), json!(body.error));
                    map.insert("finished".into(), json!(py::time_time()));
                }
                data.insert("queue".into(), Value::Array(queue));
                write(&config, &data)?;
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await?;
    if !found {
        return Err(HttpError::new(404, "no such request"));
    }
    Ok(web::ok(json!({"ok": true})))
}

/// `reminders_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/reminders", get(status).put(snapshot))
        .route("/reminders/agents", put(set_agents))
        .route("/reminders/rights", put(set_rights))
        .route("/reminders/android", put(android_copy))
        .route("/reminders/queue", get(queue))
        .route("/reminders/queue/{op_id}", post(done))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(arguments: Value) -> Result<Option<Value>, &'static str> {
        place_args(arguments.as_object().unwrap())
    }

    #[test]
    fn places_as_the_phones_read_them() {
        assert_eq!(place(json!({"title": "x"})), Ok(None));
        assert_eq!(
            place(json!({"place": "Carrefour Market, rue Oberkampf"})),
            Ok(Some(json!({"place": "Carrefour Market, rue Oberkampf", "when": "arriving", "radius": "150"})))
        );
        assert_eq!(
            place(json!({"latitude": 48.8566, "longitude": "2.3522", "when": "leaving", "radius": 20})),
            Ok(Some(
                json!({"place": "", "when": "leaving", "radius": "100", "latitude": "48.856600", "longitude": "2.352200"})
            ))
        );
        assert!(place(json!({"latitude": 48.8})).is_err());
        assert!(place(json!({"latitude": 98.0, "longitude": 2.0})).is_err());
        assert!(place(json!({"place": "home", "when": "tomorrow"})).is_err());
        assert!(place(json!({"when": "arriving"})).is_err());
    }

    #[test]
    fn the_copy_goes_with_the_last_agent() {
        let mut data: Dict = serde_json::from_value(json!({
            "agents": ["coach"], "lists": [{"id": "L1", "title": "Courses"}],
            "items": [{"id": "r1", "list": "L1", "title": "Lait", "notes": "demi-écrémé"}], "at": 1.0,
            "queue": [{"id": "q1", "op": "add", "state": "pending"}]})).unwrap();
        set_agents_in(&mut data, &["coach".to_string()]);
        assert_eq!(data["items"].as_array().map(Vec::len), Some(1));
        set_agents_in(&mut data, &[]);
        assert_eq!(Value::Object(data), json!({"agents": [], "queue": [{"id": "q1", "op": "add", "state": "pending"}]}));
    }

    fn shared(dir: &std::path::Path, extra: Value) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
        let mut data: Dict = serde_json::from_value(json!({"agents": ["coach"]})).unwrap();
        data.extend(serde_json::from_value::<Dict>(extra).unwrap());
        write(&config, &data).unwrap();
        config
    }

    fn add(config: &Config, arguments: Value) -> (bool, String, Option<Value>) {
        let arguments: Dict = serde_json::from_value(arguments).unwrap();
        let said = call(config, "coach", "reminders_add", &arguments, None, Some(NOW)).unwrap();
        let queue = list_of(&read(config), "queue");
        (
            said["isError"] == json!(true),
            said["content"][0]["text"].as_str().unwrap_or("").to_string(),
            queue.last().cloned(),
        )
    }

    const NOW: f64 = 1_793_000_000.0;

    /// « Rappelle-moi à 14 h » as the agents wrote it (7/11, Alex: the iPhone showed the day, not the time): the time
    /// reaches the phone, written as it reads it, and the agent is told where it lands.
    #[test]
    fn a_reminder_at_two_reaches_the_iphone_with_its_time() {
        let dir = tempfile::tempdir().unwrap();
        let config = shared(dir.path(), json!({"at": NOW - 60.0, "lists": [], "items": []}));
        for (due, written) in [
            ("2026-11-07T14:00", "2026-11-07T14:00:00"),
            ("2026-11-08 14:00", "2026-11-08T14:00:00"),
            ("2026-12-24T14:00:00", "2026-12-24T14:00:00"),
            ("2026-11-09T14:00:00+01:00", "2026-11-09T14:00:00+01:00"),
            ("2026-11-10", "2026-11-10"),
        ] {
            let (failed, said, op) = add(&config, json!({"title": format!("Appeler Paul {due}"), "due": due}));
            let op = op.unwrap();
            assert!(!failed, "{said}");
            assert_eq!(op["args"]["due"], json!(written), "{due}");
            assert_eq!(op.get("phone"), None, "an iPhone's: every app reads it");
            assert!(said.contains("the Reminders app of their iPhone"), "{said}");
            if due.len() > 10 {
                assert!(said.contains("14:00, with an alert at that time"), "{said}");
            } else {
                assert!(said.contains("2026-11-10 (a day, no time)"), "{said}");
            }
        }
        // A time the phone can't read, or put where the bridge doesn't look: refused, not dropped.
        let (failed, said, _) = add(&config, json!({"title": "x", "due": "demain 14h"}));
        assert!(failed && said.starts_with("due: a date and time"), "{said}");
        let (failed, said, _) = add(&config, json!({"title": "Appeler Paul", "due": "2026-11-07", "time": "14:00"}));
        assert!(failed && said.contains("unknown argument time") && said.contains("in due"), "{said}");
        // The same reminder, the copy writing it with the phone's offset: not asked twice.
        let mut data = read(&config);
        data.insert(
            "items".into(),
            json!([{"id": "r1", "list": "L", "title": "Banque", "due": "2026-11-12T10:00:00+01:00"}]),
        );
        write(&config, &data).unwrap();
        let (failed, said, _) = add(&config, json!({"title": "banque", "due": "2026-11-12T10:00"}));
        assert!(!failed && said.starts_with("Already in the reminders: id r1"), "{said}");
    }

    /// 7/11, Android: the agent said « c'est noté » for a reminder no phone would do, then gave iPhone steps.
    #[test]
    fn without_an_iphone_the_reminder_goes_to_the_android_calendar_or_is_plainly_not_set() {
        let dir = tempfile::tempdir().unwrap();
        // No phone said it takes them (an Android app from before 7/11): not asked, said plainly, no phone's steps.
        let config = shared(dir.path(), json!({}));
        let (failed, said, op) = add(&config, json!({"title": "Appeler Paul", "due": "2026-11-07T14:00"}));
        assert!(failed && op.is_none());
        assert!(said.starts_with("Not added") && said.contains("isn't set") && !said.contains("Settings"), "{said}");
        // A reminder by place still goes (both phones do those).
        let (failed, said, op) = add(&config, json!({"title": "Lait", "place": "Monoprix"}));
        assert!(!failed && op.unwrap().get("phone").is_none(), "{said}");
        // The Android app said it may write to a calendar: there, at the time, with an alert; for it only.
        let android = json!({"android": {"at": NOW - 60.0, "calendar": true, "items": []}});
        let config = shared(dir.path(), android);
        let (failed, said, op) = add(&config, json!({"title": "Appeler Paul", "due": "2026-11-07T14:00"}));
        let op = op.unwrap();
        assert!(!failed && op["phone"] == json!("android") && op["args"]["due"] == json!("2026-11-07T14:00:00"));
        assert!(said.contains("in their calendar at 2026-11-07 14:00, with an alert"), "{said}");
        assert!(!said.contains("iPhone"), "{said}");
        let (_, said, _) = add(&config, json!({"title": "Vaccin", "due": "2026-11-20"}));
        assert!(said.contains("on 2026-11-20, all day, with an alert at 9:00"), "{said}");
        let (failed, said, _) = add(&config, json!({"title": "Pain"}));
        assert!(failed && said.contains("needs a date"), "{said}");
        // Without the calendar allowed: it waits, and the agent says so.
        let config = shared(dir.path(), json!({"android": {"at": NOW - 60.0, "calendar": false}}));
        let (failed, said, op) = add(&config, json!({"title": "Appeler Paul", "due": "2026-11-07T14:00"}));
        assert!(!failed && op.unwrap()["phone"] == json!("android"));
        assert!(said.starts_with("Asked, but not set yet") && said.contains("allow"), "{said}");
        // Both phones: the iPhone's Reminders, never the Android calendar too.
        let both = json!({"at": NOW - 60.0, "android": {"at": NOW - 60.0, "calendar": true}});
        let config = shared(dir.path(), both);
        let (_, said, op) = add(&config, json!({"title": "Appeler Paul", "due": "2026-11-07T14:00"}));
        assert!(op.unwrap().get("phone").is_none() && said.contains("the Reminders app of their iPhone"), "{said}");
        // An iPhone not heard of for a month: taken as gone.
        let config = shared(dir.path(), json!({"at": NOW - 40.0 * 86400.0}));
        assert!(add(&config, json!({"title": "x", "due": "2026-11-07T14:00"})).0);
    }

    #[test]
    fn the_android_calendar_reminders_are_listed_and_ticked_off_there() {
        let dir = tempfile::tempdir().unwrap();
        let items = json!([{"id": "a42", "title": "Appeler Paul", "due": "2026-11-07T14:00:00+01:00"}]);
        let config = shared(dir.path(), json!({"android": {"at": NOW - 60.0, "calendar": true, "items": items}}));
        let listed = call(&config, "coach", "reminders_list", &Dict::new(), None, Some(NOW)).unwrap();
        let listed = listed["content"][0]["text"].as_str().unwrap().to_string();
        assert!(listed.contains("Android phone's calendar") && listed.contains("- id a42 · Appeler Paul · due 2026-11-07T14:00"));
        let (_, said, _) = add(&config, json!({"title": "appeler paul", "due": "2026-11-07T14:00"}));
        assert!(said.starts_with("Already in the reminders: id a42"), "{said}");
        let arguments: Dict = serde_json::from_value(json!({"id": "a42"})).unwrap();
        let done = call(&config, "coach", "reminders_complete", &arguments, None, Some(NOW)).unwrap();
        assert_eq!(done["isError"], json!(false));
        let op = list_of(&read(&config), "queue").last().cloned().unwrap();
        assert_eq!((&op["op"], &op["phone"], &op["args"]), (&json!("complete"), &json!("android"), &json!({"id": "a42"})));
    }

    #[test]
    fn the_schema_says_where_the_time_goes() {
        let tools = reminder_tools();
        let add = tools.iter().find(|t| t["name"] == "reminders_add").unwrap();
        let schema = &add["inputSchema"];
        assert_eq!(schema["additionalProperties"], json!(false));
        let properties: Vec<&str> = schema["properties"].as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(properties.len(), ADD_ARGUMENTS.len());
        assert!(ADD_ARGUMENTS.iter().all(|a| properties.contains(a)));
        assert!(schema["properties"]["due"]["description"].as_str().unwrap().contains("2026-11-07T14:00"));
        // Nothing in the words an agent reads tells it to send the person to a phone's settings.
        for tool in &tools {
            let words = tool.to_string();
            assert!(!words.contains("Settings") && !words.contains("Réglages"), "{words}");
        }
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.join("bridge.db").to_string_lossy().into_owned();
        config
    }

    /// 7/11: a reminder by place made by an Android phone, the agent told « the person's iPhone » does it. A reminder
    /// by place is the person's phone's (either kind does it), the others their iPhone's (Android has no Reminders app).
    #[test]
    fn a_reminder_by_place_is_the_phones_the_others_the_iphones() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        let mut data = Dict::new();
        data.insert("agents".into(), json!(["coach"]));
        data.insert("at".into(), json!(940.0)); // an iPhone shares its Reminders (7/11: else no phone takes « banque »)
        write(&config, &data).unwrap();
        let run = |name: &str, arguments: Value| {
            let result = call(&config, "coach", name, arguments.as_object().unwrap(), None, Some(1000.0)).unwrap();
            assert_eq!(result["isError"], json!(false), "{result:?}");
            result["content"][0]["text"].as_str().unwrap().to_string()
        };
        let bread = json!({"title": "Acheter du pain", "place": "Boulangerie Dupont"});
        let asked = run("reminders_add", bread.clone());
        assert!(asked.starts_with("Asked: the person's phone reminds them on arriving at « Boulangerie Dupont »"), "{asked}");
        assert!(!asked.contains("iPhone"));
        assert!(run("reminders_add", bread).contains("(the phone adds it at its next sync)"));
        let bank = run("reminders_add", json!({"title": "Rendez-vous banque", "due": "2026-11-09T10:00"}));
        assert!(bank.starts_with("Asked: it goes into the Reminders app of their iPhone for 2026-11-09 10:00"), "{bank}");
        let listed = run("reminders_list", json!({}));
        assert!(listed.contains("Acheter du pain · asked by you, not on the phone yet"), "{listed}");
        assert!(
            listed.contains("Rendez-vous banque · due 2026-11-09T10:00:00 · asked by you, not on the iPhone yet"),
            "{listed}"
        );
    }

    #[test]
    fn ages_as_python() {
        assert_eq!(age(59.0), "0 min");
        assert_eq!(age(7199.0), "119 min");
        assert_eq!(age(7200.0), "2 h");
        assert_eq!(age(3.0 * 86400.0), "3 days");
        assert_eq!(age(-30.0), "-1 min");
    }
}
