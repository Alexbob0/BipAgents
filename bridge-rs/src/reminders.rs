//! The person's iPhone Reminders, for the agents they choose (decided 2026-10-20). No server reaches them (Apple has no
//! API for them), so the iPhone app shares them: it sends its lists and reminders here when it opens (and when woken by
//! a silent push), and applies what the agents asked for, through EventKit. An agent reads the last copy and asks for
//! a reminder to be added or ticked off; the request waits here until the iPhone has done it.
//!
//!     reminders_list       the reminders as the iPhone last sent them (not done, and done in the last 30 days)
//!     reminders_add        a new reminder (a title, a date, a list, or a place: « when I get to… »), done by the phone
//!                          at its next sync (an Android phone does only those with a place, as its own « rappels par
//!                          lieu »: Android has no Reminders app)
//!     reminders_complete   a reminder ticked off, the same way
//!
//! App routes (the bridge key):
//!
//!     GET  /v1/reminders                 {agents, at, lists: n, items: n}
//!     PUT  /v1/reminders                 {lists: [{id, title}], items: [{id, list, title, notes, due, completed}]}
//!     PUT  /v1/reminders/agents          {agents}
//!     GET  /v1/reminders/queue           the agents' requests waiting for the iPhone
//!     POST /v1/reminders/queue/{id}      {done: bool, error?} (error « unknown request », from an app older than the
//!                                        request's op: it stays waiting for the app's update)
//!
//!     data_dir/reminders.json   (chmod 600: the copy, the agents allowed, the requests)
//!
//! Port of `bridge/bipbridge/reminders.py` (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;
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
            "description": "The person's iPhone Reminders as their phone last shared them: open ones, and those done in the \
        last 30 days. Optional: a list's name, or include_done.",
            "inputSchema": {"type": "object", "properties": {"list": {"type": "string"}, "include_done": {"type": "boolean"}}},
        }),
        json!({
            "name": "reminders_add",
            "description": "Add a reminder to the person's iPhone Reminders (it appears when their iPhone next syncs, usually \
        within minutes). due: an ISO date or date-time, optional; list: a list's name, optional. A reminder by place \
        (« when I get to the supermarket… »): place, the place's name or address as the person says it (their phone \
        finds it), latitude and longitude only when you know them, when: arriving (default) or leaving, radius in \
        metres (default 150). On an Android phone, only reminders with a place are done.",
            "inputSchema": {"type": "object", "properties": {"title": {"type": "string"}, "due": {"type": "string"},
                                                             "list": {"type": "string"}, "notes": {"type": "string"},
                                                             "place": {"type": "string"}, "latitude": {"type": "number"},
                                                             "longitude": {"type": "number"}, "radius": {"type": "number"},
                                                             "when": {"type": "string", "enum": ["arriving", "leaving"]}},
                            "required": ["title"]},
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
    {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(config);
        if !tk::py_in(agent, data.get("agents").unwrap_or(&json!([]))) {
            return Ok(tool_text(
                "The person hasn't shared their reminders with you: they do it in the BipAgents app (on an iPhone: Settings › \
                 Rappels; on Android: your « Rappels » tool, for reminders by place).",
                true,
            ));
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
                &format!("As the iPhone shared them {} ago:\n{}", age(now - shared), lines.join("\n")),
                false,
            ));
        }
        let op = if name == "reminders_add" {
            let title = tk::strip(&tk::str_or_empty(arguments.get("title"))).to_string();
            if title.is_empty() {
                return Ok(tool_text("a title is needed", true));
            }
            let mut args = json!({"title": tk::head(&title, 300), "due": tk::head(&tk::str_or_empty(arguments.get("due")), 40),
                                  "list": tk::head(&tk::str_or_empty(arguments.get("list")), 120),
                                  "notes": tk::head(&tk::str_or_empty(arguments.get("notes")), 2000)});
            match place_args(arguments) {
                Ok(Some(Value::Object(place))) => {
                    if let Some(args) = args.as_object_mut() {
                        args.extend(place);
                    }
                }
                Ok(_) => {}
                Err(why) => return Ok(tool_text(why, true)),
            }
            json!({"id": py::token_hex(6), "op": "add", "agent": agent, "created": now, "state": "pending", "args": args})
        } else if name == "reminders_complete" {
            let reminder = tk::str_or_empty(arguments.get("id"));
            if !items.iter().any(|i| field(i, "id").and_then(Value::as_str) == Some(reminder.as_str())) {
                return Ok(tool_text("unknown reminder id (see reminders_list)", true));
            }
            json!({"id": py::token_hex(6), "op": "complete", "agent": agent, "created": now, "state": "pending",
                   "args": {"id": reminder}})
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
    }
    if let Some(wake) = wake {
        wake();
    }
    Ok(tool_text(
        "Asked: the person's iPhone does it at its next sync (usually within minutes, or when the app opens).",
        false,
    ))
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
        data.insert("agents".into(), json!(names));
        write(&config, &data)?;
        Ok(data["agents"].clone())
    })
    .await?;
    Ok(web::ok(json!({"agents": agents})))
}

async fn queue(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let data = blocking(move || Ok(read(&config))).await?;
    let mut waiting = Vec::new();
    for op in list_of(&data, "queue") {
        if field(&op, "state").and_then(Value::as_str) != Some("pending") {
            continue;
        }
        let mut out = Dict::new();
        for k in ["id", "op", "args", "agent", "created"] {
            // Python's op[k]: a request the bridge wrote has them all
            out.insert(k.into(), field(&op, k).cloned().ok_or_else(|| anyhow::anyhow!("KeyError: '{k}'"))?);
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
    fn ages_as_python() {
        assert_eq!(age(59.0), "0 min");
        assert_eq!(age(7199.0), "119 min");
        assert_eq!(age(7200.0), "2 h");
        assert_eq!(age(3.0 * 86400.0), "3 days");
        assert_eq!(age(-30.0), "-1 min");
    }
}
