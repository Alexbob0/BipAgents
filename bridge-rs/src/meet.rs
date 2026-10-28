//! « Mon agent voit avec le tien » (Alex's pick, 2026-10-28): two linked households (households.rs) find a date for
//! something together, a dinner for example, over the same household link as peers.rs and family_news.rs.
//!
//! **What crosses**: only the slots each person ticked. The calendar never leaves its place: the free slots are
//! computed here, from the phone's copy (calendar.rs), and the other side only sees the slots offered. Each step is a
//! card the person answers (the Boîte); nothing is booked until both accepted the same slot.
//!
//! 1. Alex (the app's « Trouver une date avec… », or one of his agents with `find_date`) asks for a date with Léa:
//!    a title, a length, a period and a part of the day. His bridge computes his free slots: a **draft** card.
//!    Nothing leaves yet.
//! 2. Alex keeps the slots he wants to offer and sends them (`send`). Léa's bridge keeps them: an **offered** card,
//!    with the slots her own calendar has free already ticked (computed on her side, never sent back).
//! 3. Léa ticks what suits her and answers (`answer`, maybe none): only those slots go back.
//! 4. Alex's card shows the slots both have: he picks one (`choose`), which counts as his yes. Léa's card asks her.
//! 5. Léa accepts (`accept`): **booked** on both sides, and the event is asked to each phone's calendar (its queue,
//!    as an agent's `calendar_add`). Either may say no at any step (`decline`): the other side is told, nothing kept
//!    open.
//!
//! **Safety**: what the other household sends is data, shown to the person as text, never given to an agent as
//! instructions (no agent reads it; `find_date` only makes a draft, and an agent learns nothing back). Every message
//! between places is checked: the household key, its size (`BODY_MAX`), its fields (an id, a title of `TITLE_MAX`
//! characters on one line, at most `SLOTS_MAX` slots, each a time ahead within `AHEAD_DAYS` days), the step it may
//! come at, and a rate (`RATE_MAX` messages an hour per household, `OPEN_MAX` open requests per household).
//!
//! The app's routes (the bridge key):
//!
//!     GET    /v1/meet                     {meets: [meet]}   the open ones first, the ended ones for 30 days
//!     POST   /v1/meet                     {household, title, minutes?, from?, to?, part?}  → meet (a draft)
//!     POST   /v1/meet/{id}/send           {slots}    the draft's slots kept, sent      → meet
//!     POST   /v1/meet/{id}/answer         {slots}    the offered slots that suit (maybe none)  → meet
//!     POST   /v1/meet/{id}/choose         {slot}     one of the common slots            → meet
//!     POST   /v1/meet/{id}/accept                    the slot chosen by the other side  → meet
//!     POST   /v1/meet/{id}/decline                   no (any open step)                 → meet
//!     DELETE /v1/meet/{id}                           an ended one forgotten
//!
//!     meet   {id, role: "host"|"guest", household, household_name, title, minutes, part, state, created, updated,
//!             offered: [slot], free: [slot] (guest: free in her calendar, local only), answer: [slot],
//!             common: [slot] (host), chosen: slot|null, booked: bool (asked to the phone's calendar), agent|null}
//!     state  host: draft → sent → answered → confirmed → booked;  guest: offered → answered → asked → booked;
//!            declined (by this person) | refused (by the other side) | failed (their side could not be reached)
//!     slot   an ISO date-time with its offset (« 2026-11-06T19:30:00+01:00 »); `minutes` long
//!
//! Between the bridges (the household key):
//!
//!     POST /family/meet                  {id, title, minutes, slots}
//!     POST /family/meet/{id}/answer      {slots}
//!     POST /family/meet/{id}/confirm     {slot}
//!     POST /family/meet/{id}/accept      {}
//!     POST /family/meet/{id}/decline     {}
//!
//! `<data>/meet.json` (0600).
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use chrono::{DateTime, Duration as Days, FixedOffset, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::calendar::Moment;
use crate::config::{AgentConfig, Config};
use crate::households::Households;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "find_date";
pub const TITLE_MAX: usize = 80;
pub const SLOTS_MAX: usize = 8;
pub const MINUTES_MIN: i64 = 15;
pub const MINUTES_MAX: i64 = 720;
pub const DEFAULT_MINUTES: i64 = 120;
/// The period looked at by default, from tomorrow, and at most.
pub const DEFAULT_DAYS: i64 = 14;
pub const PERIOD_MAX_DAYS: i64 = 62;
/// A slot sent between places is at most this far ahead.
pub const AHEAD_DAYS: i64 = 120;
/// A message between places, at most (bytes).
pub const BODY_MAX: usize = 4096;
/// Messages a household may send here in an hour.
pub const RATE_MAX: usize = 30;
/// Open requests from one household, at most.
pub const OPEN_MAX: usize = 5;
/// Ended requests are kept this long.
pub const ENDED_DAYS: f64 = 30.0;
pub const PARTS: [&str; 5] = ["evening", "lunch", "morning", "afternoon", "day"];
const OPEN_STATES: [&str; 6] = ["draft", "sent", "answered", "confirmed", "offered", "asked"];

static LOCK: Mutex<()> = Mutex::new(());
static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^m[0-9a-f]{12}$").expect("valid regex"));
/// Messages received per household (their times), for the rate.
static RATE: LazyLock<Mutex<HashMap<String, Vec<f64>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn now() -> f64 {
    py::time_time()
}

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("meet.json")
}

fn households(config: &Config) -> Households {
    Households::new(&crate::households::path_for(config))
}

fn load(config: &Config) -> Vec<Value> {
    let text = tk::read_text(&path(config)).unwrap_or_default();
    let mut all = match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map.get("meets").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    let cutoff = now() - ENDED_DAYS * 86400.0;
    all.retain(|m| is_open(m) || m.get("updated").and_then(Value::as_f64).unwrap_or(0.0) > cutoff);
    all
}

fn save(config: &Config, meets: &[Value]) -> anyhow::Result<()> {
    let target = path(config);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = target.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!({"meets": meets})).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &target)?;
    Ok(())
}

/// Changes the requests under the lock.
fn update<R>(config: &Config, change: impl FnOnce(&mut Vec<Value>) -> R) -> anyhow::Result<R> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = load(config);
    let out = change(&mut all);
    save(config, &all)?;
    Ok(out)
}

fn is_open(m: &Value) -> bool {
    OPEN_STATES.contains(&str_of(m, "state").as_str())
}

fn slots_of(m: &Value, key: &str) -> Vec<String> {
    m.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

// -- the slots ------------------------------------------------------------------------------------------------------

/// A part of the day: when a slot may start, and by when it ends.
fn window(part: &str) -> (NaiveTime, NaiveTime) {
    let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap_or_default();
    match part {
        "lunch" => (t(12, 0), t(14, 30)),
        "morning" => (t(9, 0), t(12, 30)),
        "afternoon" => (t(14, 0), t(18, 30)),
        "day" => (t(9, 0), t(19, 0)),
        _ => (t(19, 0), t(23, 30)),
    }
}

/// The phone's timed events as wall-clock spans (the all-day ones don't take a slot: birthdays, holidays…).
fn busy(config: &Config) -> Vec<(NaiveDateTime, NaiveDateTime)> {
    let data = crate::calendar::read(config);
    let events = data.get("events").and_then(Value::as_array).cloned().unwrap_or_default();
    events
        .iter()
        .filter(|e| !e.get("all_day").map(py::truthy).unwrap_or(false))
        .filter_map(|e| {
            let start = Moment::parse(&str_of(e, "start"))?.wall();
            let end = Moment::parse(&str_of(e, "end")).map(|m| m.wall()).unwrap_or(start + Days::hours(1));
            Some((start, end.max(start)))
        })
        .collect()
}

fn overlaps(busy: &[(NaiveDateTime, NaiveDateTime)], start: NaiveDateTime, end: NaiveDateTime) -> bool {
    busy.iter().any(|(s, e)| *s < end && start < *e)
}

/// The first free start of each day of the period, in the part of the day asked, `SLOTS_MAX` at most.
pub fn free_slots(
    busy: &[(NaiveDateTime, NaiveDateTime)],
    from: NaiveDate,
    to: NaiveDate,
    part: &str,
    minutes: i64,
    after: NaiveDateTime,
) -> Vec<NaiveDateTime> {
    let (first, last) = window(part);
    let mut out = Vec::new();
    let mut day = from;
    while day <= to && out.len() < SLOTS_MAX {
        let mut start = day.and_time(first);
        let latest = day.and_time(last) - Days::minutes(minutes);
        while start <= latest {
            let end = start + Days::minutes(minutes);
            if start > after && !overlaps(busy, start, end) {
                out.push(start);
                break;
            }
            start += Days::minutes(30);
        }
        day += Days::days(1);
    }
    out
}

/// A local wall-clock time with this machine's offset then.
fn zoned(local: NaiveDateTime) -> String {
    match Local.from_local_datetime(&local).earliest() {
        Some(at) => at.fixed_offset().to_rfc3339(),
        None => local.format("%Y-%m-%dT%H:%M:%S").to_string(),
    }
}

fn parse_slot(slot: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(slot.trim()).ok()
}

/// A slot as this machine's wall clock.
fn wall_of(slot: &str) -> Option<NaiveDateTime> {
    parse_slot(slot).map(|at| at.with_timezone(&Local).naive_local())
}

/// Slots sent by another place: valid times ahead, within `AHEAD_DAYS`, each once, `SLOTS_MAX` at most. Err: why not.
pub fn checked_slots(given: &Value, at: f64) -> Result<Vec<String>, String> {
    let Some(list) = given.as_array() else { return Err("slots: a list".into()) };
    if list.len() > SLOTS_MAX {
        return Err(format!("at most {SLOTS_MAX} slots"));
    }
    let mut out: Vec<String> = Vec::new();
    for slot in list {
        let text = slot.as_str().unwrap_or("");
        let Some(when) = parse_slot(text).filter(|_| text.len() <= 40) else {
            return Err("a slot: an ISO date-time with its offset".into());
        };
        let seconds = when.timestamp() as f64;
        if seconds < at - 3600.0 || seconds > at + (AHEAD_DAYS * 86400) as f64 {
            return Err("a slot in the coming months".into());
        }
        let kept = when.to_rfc3339();
        if !within(&kept, &out) {
            out.push(kept);
        }
    }
    Ok(out)
}

/// Whether two slots are the same moment (their offsets may differ).
fn same(a: &str, b: &str) -> bool {
    matches!((parse_slot(a), parse_slot(b)), (Some(x), Some(y)) if x == y)
}

fn within(slot: &str, list: &[String]) -> bool {
    list.iter().any(|s| same(s, slot))
}

/// The offered slots free in this person's calendar.
fn free_among(config: &Config, offered: &[String], minutes: i64) -> Vec<String> {
    let busy = busy(config);
    offered
        .iter()
        .filter(|s| wall_of(s).is_some_and(|start| !overlaps(&busy, start, start + Days::minutes(minutes))))
        .cloned()
        .collect()
}

// -- the requests ---------------------------------------------------------------------------------------------------

fn household_name(config: &Config, household: &str) -> String {
    households(config)
        .get(household)
        .and_then(|h| h.get("name").map(py::py_str))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| household.to_string())
}

fn view(m: &Value) -> Value {
    let mut out = m.as_object().cloned().unwrap_or_default();
    if str_of(m, "role") == "host" {
        let offered = slots_of(m, "offered");
        let common: Vec<String> = slots_of(m, "answer").into_iter().filter(|s| within(s, &offered)).collect();
        out.insert("common".into(), json!(common));
    } else {
        out.insert("common".into(), json!([]));
    }
    for key in ["chosen", "agent"] {
        out.entry(key).or_insert(Value::Null);
    }
    out.entry("booked").or_insert(json!(false));
    Value::Object(out)
}

pub fn listing(config: &Config) -> Value {
    let mut all = load(config);
    all.sort_by(|a, b| {
        is_open(b).cmp(&is_open(a)).then(
            b.get("updated").and_then(Value::as_f64).unwrap_or(0.0).total_cmp(&a.get("updated").and_then(Value::as_f64).unwrap_or(0.0)),
        )
    });
    json!({"meets": all.iter().map(view).collect::<Vec<_>>()})
}

/// What a draft asks for.
#[derive(Debug, Default, Clone)]
pub struct Ask {
    pub household: String,
    pub title: String,
    pub minutes: Option<i64>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub part: Option<String>,
    pub agent: Option<String>,
}

/// The household named `wanted` (its id or its name, any case).
fn find_household(config: &Config, wanted: &str) -> Option<String> {
    let wanted = tk::squeeze(wanted).to_lowercase();
    households(config).public().iter().find_map(|h| {
        let id = h.get("id").map(py::py_str).unwrap_or_default();
        let name = h.get("name").map(py::py_str).unwrap_or_default();
        (id == wanted || name.to_lowercase() == wanted).then_some(id)
    })
}

/// A draft: the free slots computed here, nothing sent. Err: (status, why).
pub fn draft(config: &Config, ask: &Ask, at: f64) -> Result<Value, (u16, String)> {
    let household = find_household(config, &ask.household).ok_or((404, "no such household linked".to_string()))?;
    let title = tk::head(&tk::in_note_line(&ask.title), TITLE_MAX).to_string();
    if title.is_empty() {
        return Err((422, "a title".into()));
    }
    let minutes = ask.minutes.unwrap_or(DEFAULT_MINUTES);
    if !(MINUTES_MIN..=MINUTES_MAX).contains(&minutes) {
        return Err((422, format!("minutes: {MINUTES_MIN} to {MINUTES_MAX}")));
    }
    let part = ask.part.clone().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).unwrap_or("evening".into());
    if !PARTS.contains(&part.as_str()) {
        return Err((422, format!("part: one of {}", PARTS.join(", "))));
    }
    let today = DateTime::from_timestamp(at as i64, 0).map(|d| d.with_timezone(&Local)).unwrap_or_else(Local::now);
    let day = |given: &Option<String>, default: NaiveDate| -> Result<NaiveDate, (u16, String)> {
        match given.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            None => Ok(default),
            Some(d) => NaiveDate::parse_from_str(d.get(..10).unwrap_or(d), "%Y-%m-%d")
                .map_err(|_| (422, "from / to: a date, YYYY-MM-DD".to_string())),
        }
    };
    let from = day(&ask.from, today.date_naive() + Days::days(1))?.max(today.date_naive());
    let to = day(&ask.to, from + Days::days(DEFAULT_DAYS - 1))?;
    if to < from || (to - from).num_days() > PERIOD_MAX_DAYS {
        return Err((422, format!("a period of {PERIOD_MAX_DAYS} days at most")));
    }
    let slots: Vec<String> =
        free_slots(&busy(config), from, to, &part, minutes, today.naive_local()).into_iter().map(zoned).collect();
    if slots.is_empty() {
        return Err((409, "no free slot in that period".into()));
    }
    let meet = json!({
        "id": format!("m{}", py::token_hex(6)),
        "role": "host",
        "household": household,
        "household_name": household_name(config, &household),
        "title": title,
        "minutes": minutes,
        "part": part,
        "state": "draft",
        "created": at,
        "updated": at,
        "offered": slots,
        "free": [],
        "answer": [],
        "agent": ask.agent,
    });
    update(config, |all| {
        // a draft not sent is replaced by the next one for the same household
        all.retain(|m| !(str_of(m, "role") == "host" && str_of(m, "state") == "draft" && str_of(m, "household") == household));
        all.push(meet.clone());
    })
    .map_err(|e| (500, e.to_string()))?;
    Ok(view(&meet))
}

fn find(all: &[Value], id: &str) -> Option<usize> {
    all.iter().position(|m| str_of(m, "id") == id)
}

fn set(m: &mut Value, key: &str, value: Value) {
    if let Value::Object(map) = m {
        map.insert(key.into(), value);
        map.insert("updated".into(), json!(now()));
    }
}

/// The event asked to this person's phone calendar (calendar.rs' queue): « Dîner avec Léa ».
fn book(config: &Config, m: &Value) -> bool {
    let chosen = str_of(m, "chosen");
    let Some(start) = parse_slot(&chosen) else { return false };
    let end = start + Days::minutes(m.get("minutes").and_then(Value::as_i64).unwrap_or(DEFAULT_MINUTES));
    let title = format!("{} ({})", str_of(m, "title"), str_of(m, "household_name"));
    let event = json!({"title": tk::head(&title, 300), "start": start.to_rfc3339(), "end": end.to_rfc3339(),
                       "all_day": false, "calendar": "", "location": "", "notes": ""});
    crate::calendar::queue_event(config, "bip", &event).is_ok()
}

// -- between the places ---------------------------------------------------------------------------------------------

/// A call to the other household's bridge. Err: it could not be reached, or said no (its words).
async fn tell(state: &AppState, household: &str, path: &str, body: Value) -> Result<(), (u16, String)> {
    let entry = state.households.get(household).ok_or((404, "no such household linked".to_string()))?;
    let url = format!("{}/family/meet{path}", entry.get("url").map(py::py_str).unwrap_or_default());
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let answer = crate::door_tls::household_client(&state.http, &entry)
        .post(url)
        .timeout(Duration::from_secs(10))
        .bearer_auth(key)
        .json(&body)
        .send()
        .await
        .map_err(|_| (502, "their side does not answer right now: try again later".to_string()))?;
    let status = answer.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(());
    }
    let detail = answer.json::<Value>().await.ok().map(|v| str_of(&v, "detail")).unwrap_or_default();
    Err((if status == 429 { 429 } else { 409 }, tk::head(&detail, 200).to_string()))
}

fn refused((status, why): (u16, String)) -> HttpError {
    HttpError::new(status, why)
}

/// The agent this person's pushes go to: the first that reads the calendar, else the first.
fn push_agent(config: &Config) -> Option<String> {
    let mut names: Vec<&String> = config.agents.keys().collect();
    names.sort();
    names
        .iter()
        .find(|n| crate::calendar::enabled(config, n))
        .or(names.first())
        .map(|n| n.to_string())
}

fn notify(state: &AppState, line: String, id: &str) {
    let config = state.config();
    let Some(agent) = push_agent(&config) else { return };
    let (push, id) = (state.push.clone(), id.to_string());
    state.spawn(async move {
        push.notify_meet(&agent, &line, &id).await;
    });
}

fn household_of(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();
    let household = if token.is_empty() { None } else { state.households.caller(token) };
    household.ok_or_else(|| HttpError::new(401, "unknown household"))
}

/// Whether `household` may send one more message now (and counts it).
pub fn within_rate(household: &str, at: f64) -> bool {
    let mut rate = RATE.lock().unwrap_or_else(|e| e.into_inner());
    let times = rate.entry(household.to_string()).or_default();
    times.retain(|t| *t > at - 3600.0);
    if times.len() >= RATE_MAX {
        return false;
    }
    times.push(at);
    true
}

/// A message from another place: its household, its body as an object (size and rate checked).
fn incoming(state: &AppState, headers: &HeaderMap, body: &[u8]) -> ApiResult<(String, Map<String, Value>)> {
    let household = household_of(state, headers)?;
    if body.len() > BODY_MAX {
        return Err(HttpError::new(413, "too long"));
    }
    if !within_rate(&household, now()) {
        tracing::info!(household = %household, "meet: too many messages, refused");
        return Err(HttpError::new(429, "too many requests: later"));
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => Ok((household, map)),
        Ok(Value::Null) | Err(_) if body.is_empty() => Ok((household, Map::new())),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

fn checked_id(id: &str) -> ApiResult<()> {
    if ID.is_match(id) {
        Ok(())
    } else {
        Err(HttpError::new(422, "a request id"))
    }
}

/// The request `id` of `household` with this role, at one of `states`.
fn theirs(all: &[Value], household: &str, id: &str, role: &str, states: &[&str]) -> ApiResult<usize> {
    let at = all
        .iter()
        .position(|m| str_of(m, "id") == id && str_of(m, "household") == household && str_of(m, "role") == role)
        .ok_or_else(HttpError::not_found)?;
    if !states.contains(&str_of(&all[at], "state").as_str()) {
        return Err(HttpError::new(409, "not at that step"));
    }
    Ok(at)
}

async fn their_request(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let (household, body) = incoming(&state, &headers, &body)?;
    let body = Value::Object(body);
    let id = str_of(&body, "id");
    checked_id(&id)?;
    let title = tk::head(&tk::in_note_line(&str_of(&body, "title")), TITLE_MAX).to_string();
    if title.is_empty() {
        return Err(HttpError::new(422, "a title"));
    }
    let minutes = body.get("minutes").and_then(Value::as_i64).unwrap_or(0);
    if !(MINUTES_MIN..=MINUTES_MAX).contains(&minutes) {
        return Err(HttpError::new(422, "minutes"));
    }
    let at = now();
    let slots = checked_slots(body.get("slots").unwrap_or(&Value::Null), at).map_err(|e| HttpError::new(422, e))?;
    if slots.is_empty() {
        return Err(HttpError::new(422, "at least one slot"));
    }
    let config = state.config();
    let name = household_name(&config, &household);
    let free = free_among(&config, &slots, minutes);
    let meet = json!({
        "id": id, "role": "guest", "household": household, "household_name": name, "title": title,
        "minutes": minutes, "part": Value::Null, "state": "offered", "created": at, "updated": at,
        "offered": slots, "free": free, "answer": [],
    });
    let added = update(&config, |all| {
        if all.iter().any(|m| str_of(m, "id") == id && str_of(m, "household") == household) {
            return Err(HttpError::new(409, "already received"));
        }
        let open = all.iter().filter(|m| str_of(m, "household") == household && is_open(m)).count();
        if open >= OPEN_MAX {
            return Err(HttpError::new(429, "too many open requests"));
        }
        all.push(meet.clone());
        Ok(())
    })?;
    added?;
    tracing::info!(household = %household, slots = slots.len(), "meet: a request received");
    notify(&state, format!("{name} propose une date : {title}"), &id);
    Ok(web::ok(json!({"ok": true})))
}

async fn their_answer(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, body) = incoming(&state, &headers, &body)?;
    checked_id(&id)?;
    let slots = checked_slots(body.get("slots").unwrap_or(&json!([])), now()).map_err(|e| HttpError::new(422, e))?;
    let config = state.config();
    let line = update(&config, |all| -> ApiResult<String> {
        let at = theirs(all, &household, &id, "host", &["sent"])?;
        let offered = slots_of(&all[at], "offered");
        // only what was offered counts
        let kept: Vec<String> = slots.iter().filter(|s| within(s, &offered)).cloned().collect();
        let m = &mut all[at];
        set(m, "answer", json!(kept));
        set(m, "state", json!("answered"));
        Ok(if kept.is_empty() {
            format!("{} : aucun créneau ne va pour {}", str_of(m, "household_name"), str_of(m, "title"))
        } else {
            format!("{} a répondu pour {} : choisis la date", str_of(m, "household_name"), str_of(m, "title"))
        })
    })??;
    notify(&state, line, &id);
    Ok(web::ok(json!({"ok": true})))
}

async fn their_confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, body) = incoming(&state, &headers, &body)?;
    checked_id(&id)?;
    let slot = str_of(&Value::Object(body), "slot");
    let config = state.config();
    let line = update(&config, |all| -> ApiResult<String> {
        let at = theirs(all, &household, &id, "guest", &["answered"])?;
        if !within(&slot, &slots_of(&all[at], "answer")) {
            return Err(HttpError::new(422, "a slot this person accepted"));
        }
        let m = &mut all[at];
        set(m, "chosen", json!(slot));
        set(m, "state", json!("asked"));
        Ok(format!("{} propose de garder une date pour {} : d'accord ?", str_of(m, "household_name"), str_of(m, "title")))
    })??;
    notify(&state, line, &id);
    Ok(web::ok(json!({"ok": true})))
}

async fn their_accept(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, _) = incoming(&state, &headers, &body)?;
    checked_id(&id)?;
    let config = state.config();
    let line = update(&config, |all| -> ApiResult<String> {
        let at = theirs(all, &household, &id, "host", &["confirmed"])?;
        let booked = book(&config, &all[at]);
        let m = &mut all[at];
        set(m, "state", json!("booked"));
        set(m, "booked", json!(booked));
        Ok(format!("C'est d'accord avec {} : {}", str_of(m, "household_name"), str_of(m, "title")))
    })??;
    notify(&state, line, &id);
    Ok(web::ok(json!({"ok": true})))
}

async fn their_decline(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, _) = incoming(&state, &headers, &body)?;
    checked_id(&id)?;
    let config = state.config();
    let line = update(&config, |all| -> ApiResult<String> {
        let at = all
            .iter()
            .position(|m| str_of(m, "id") == id && str_of(m, "household") == household && is_open(m))
            .ok_or_else(HttpError::not_found)?;
        let m = &mut all[at];
        set(m, "state", json!("refused"));
        Ok(format!("{} ne peut pas pour {}", str_of(m, "household_name"), str_of(m, "title")))
    })??;
    notify(&state, line, &id);
    Ok(web::ok(json!({"ok": true})))
}

/// What a linked household's bridge may call (mounted at the root, its key checked).
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/meet", post(their_request))
        .route("/family/meet/{id}/answer", post(their_answer))
        .route("/family/meet/{id}/confirm", post(their_confirm))
        .route("/family/meet/{id}/accept", post(their_accept))
        .route("/family/meet/{id}/decline", post(their_decline))
}

// -- the app's routes -----------------------------------------------------------------------------------------------

fn body_of(bytes: &[u8]) -> ApiResult<Value> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

fn mine(config: &Config, id: &str) -> ApiResult<Value> {
    load(config).into_iter().find(|m| str_of(m, "id") == id).ok_or_else(HttpError::not_found)
}

fn at_step(m: &Value, role: &str, states: &[&str]) -> ApiResult<()> {
    if str_of(m, "role") != role || !states.contains(&str_of(m, "state").as_str()) {
        return Err(HttpError::new(409, "not at that step"));
    }
    Ok(())
}

/// Changes one request and gives its view.
fn changed(config: &Config, id: &str, edit: impl FnOnce(&mut Value)) -> ApiResult<Response> {
    let out = update(config, |all| {
        let at = find(all, id)?;
        edit(&mut all[at]);
        Some(view(&all[at]))
    })?;
    Ok(web::ok(out.ok_or_else(HttpError::not_found)?))
}

/// The person's slots, among `allowed` (each once). Err: one not allowed.
fn picked(body: &Value, allowed: &[String]) -> ApiResult<Vec<String>> {
    let given = checked_slots(body.get("slots").unwrap_or(&json!([])), now()).map_err(|e| HttpError::new(422, e))?;
    if let Some(bad) = given.iter().find(|s| !within(s, allowed)) {
        return Err(HttpError::new(422, format!("not a slot of this request: {bad}")));
    }
    Ok(allowed.iter().filter(|s| within(s, &given)).cloned().collect())
}

async fn list_route(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(listing(&state.config())))
}

async fn create_route(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let ask = Ask {
        household: str_of(&body, "household"),
        title: str_of(&body, "title"),
        minutes: body.get("minutes").and_then(Value::as_i64),
        from: body.get("from").and_then(Value::as_str).map(str::to_string),
        to: body.get("to").and_then(Value::as_str).map(str::to_string),
        part: body.get("part").and_then(Value::as_str).map(str::to_string),
        agent: None,
    };
    Ok(web::ok(draft(&state.config(), &ask, now()).map_err(refused)?))
}

async fn send_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let config = state.config();
    let m = mine(&config, &id)?;
    at_step(&m, "host", &["draft", "failed"])?;
    let slots = picked(&body, &slots_of(&m, "offered"))?;
    if slots.is_empty() {
        return Err(HttpError::new(422, "at least one slot"));
    }
    let household = str_of(&m, "household");
    let message = json!({"id": id, "title": str_of(&m, "title"), "minutes": m["minutes"], "slots": slots});
    let sent = tell(&state, &household, "", message).await;
    changed(&config, &id, |m| {
        set(m, "offered", json!(slots));
        set(m, "state", json!(if sent.is_ok() { "sent" } else { "failed" }));
    })?;
    sent.map_err(refused)?;
    Ok(web::ok(view(&mine(&config, &id)?)))
}

async fn answer_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let config = state.config();
    let m = mine(&config, &id)?;
    at_step(&m, "guest", &["offered"])?;
    let slots = picked(&body, &slots_of(&m, "offered"))?;
    tell(&state, &str_of(&m, "household"), &format!("/{id}/answer"), json!({"slots": slots})).await.map_err(refused)?;
    changed(&config, &id, |m| {
        set(m, "answer", json!(slots));
        set(m, "state", json!("answered"));
    })
}

async fn choose_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_of(&body)?;
    let config = state.config();
    let m = mine(&config, &id)?;
    at_step(&m, "host", &["answered"])?;
    let slot = str_of(&body, "slot");
    let common = slots_of(&view(&m), "common");
    let Some(slot) = common.iter().find(|s| same(s, &slot)).cloned() else {
        return Err(HttpError::new(422, "one of the slots you both have"));
    };
    tell(&state, &str_of(&m, "household"), &format!("/{id}/confirm"), json!({"slot": slot})).await.map_err(refused)?;
    changed(&config, &id, |m| {
        set(m, "chosen", json!(slot));
        set(m, "state", json!("confirmed"));
    })
}

async fn accept_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let m = mine(&config, &id)?;
    at_step(&m, "guest", &["asked"])?;
    tell(&state, &str_of(&m, "household"), &format!("/{id}/accept"), json!({})).await.map_err(refused)?;
    let booked = book(&config, &m);
    changed(&config, &id, |m| {
        set(m, "state", json!("booked"));
        set(m, "booked", json!(booked));
    })
}

async fn decline_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let m = mine(&config, &id)?;
    if !is_open(&m) && str_of(&m, "state") != "failed" {
        return Err(HttpError::new(409, "already ended"));
    }
    // a draft never left: nobody to tell
    if !["draft", "failed"].contains(&str_of(&m, "state").as_str()) {
        // told when it can be; no is no, here, either way
        let _ = tell(&state, &str_of(&m, "household"), &format!("/{id}/decline"), json!({})).await;
    }
    changed(&config, &id, |m| set(m, "state", json!("declined")))
}

async fn forget_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let m = mine(&config, &id)?;
    if is_open(&m) {
        return Err(HttpError::new(409, "say no first"));
    }
    update(&config, |all| all.retain(|x| str_of(x, "id") != id))?;
    Ok(web::ok(json!({"ok": true})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/meet", get(list_route).post(create_route))
        .route("/meet/{id}", delete(forget_route))
        .route("/meet/{id}/send", post(send_route))
        .route("/meet/{id}/answer", post(answer_route))
        .route("/meet/{id}/choose", post(choose_route))
        .route("/meet/{id}/accept", post(accept_route))
        .route("/meet/{id}/decline", post(decline_route))
}

// -- the agents' tool -----------------------------------------------------------------------------------------------

/// Listed for an agent that reads the phone's calendar, on an install with a linked household.
pub fn offered(config: &Config, agent: &str) -> bool {
    crate::calendar::enabled(config, agent) && crate::family_news::has_family(config)
}

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Propose finding a date with a relative's household (a dinner, a visit): your person gets a \
card with their free slots, computed from their calendar, and chooses what to offer. Nothing is sent without them, \
and you learn nothing of the other side.",
        "inputSchema": {"type": "object", "properties": {
            "household": {"type": "string", "description": "the relative's household, as family_members names it"},
            "title": {"type": "string", "description": "what for, short, in their language (« Dîner »)"},
            "minutes": {"type": "integer", "description": "how long, default 120"},
            "from": {"type": "string", "description": "first day, YYYY-MM-DD (default tomorrow)"},
            "to": {"type": "string", "description": "last day, YYYY-MM-DD (default two weeks)"},
            "part": {"type": "string", "enum": PARTS}},
            "required": ["household", "title"]},
    })
}

/// `find_date`: a draft card for the person.
pub fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    if !offered(config, &agent.name) {
        return tool_text("Your person hasn't shared their calendar with you, or has no relative linked.", true);
    }
    let arg = |k: &str| arguments.get(k).and_then(Value::as_str).map(str::to_string);
    let ask = Ask {
        household: arg("household").unwrap_or_default(),
        title: arg("title").unwrap_or_default(),
        minutes: arguments.get("minutes").and_then(|v| py::py_float(v).ok()).map(|m| m as i64),
        from: arg("from"),
        to: arg("to"),
        part: arg("part"),
        agent: Some(agent.name.clone()),
    };
    match draft(config, &ask, now()) {
        Ok(m) => tool_text(
            &format!(
                "A card with {} free slots is in your person's Boîte: they choose what to offer to {}. Say so in one \
sentence; nothing is sent until they do.",
                slots_of(&m, "offered").len(),
                str_of(&m, "household_name")
            ),
            false,
        ),
        Err((_, why)) => tool_text(&why, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn free_slots_skip_what_is_taken_and_keep_one_a_day() {
        let busy = vec![(at("2026-11-02 18:30"), at("2026-11-02 20:00")), (at("2026-11-03 19:00"), at("2026-11-03 23:00"))];
        let day = |d: &str| NaiveDate::parse_from_str(d, "%Y-%m-%d").unwrap();
        let slots = free_slots(&busy, day("2026-11-02"), day("2026-11-04"), "evening", 120, at("2026-11-01 08:00"));
        assert_eq!(slots, vec![at("2026-11-02 20:00"), at("2026-11-04 19:00")]);
        // nothing before now
        let later = free_slots(&[], day("2026-11-02"), day("2026-11-02"), "lunch", 60, at("2026-11-02 12:10"));
        assert_eq!(later, vec![at("2026-11-02 12:30")]);
        // a day too long for the window: none
        assert!(free_slots(&[], day("2026-11-02"), day("2026-11-02"), "lunch", 300, at("2026-11-01 08:00")).is_empty());
        // at most SLOTS_MAX
        let many = free_slots(&[], day("2026-11-01"), day("2026-11-30"), "day", 60, at("2026-10-01 08:00"));
        assert_eq!(many.len(), SLOTS_MAX);
    }

    #[test]
    fn slots_from_another_place_are_checked() {
        let now = DateTime::parse_from_rfc3339("2026-11-01T10:00:00+01:00").unwrap().timestamp() as f64;
        let ok = checked_slots(&json!(["2026-11-06T19:30:00+01:00", "2026-11-06T18:30:00Z"]), now).unwrap();
        assert_eq!(ok, vec!["2026-11-06T19:30:00+01:00"]); // the same moment once
        assert!(checked_slots(&json!(["2027-11-06T19:30:00+01:00"]), now).is_err()); // too far
        assert!(checked_slots(&json!(["2026-10-01T19:30:00+01:00"]), now).is_err()); // past
        assert!(checked_slots(&json!(["demain soir"]), now).is_err());
        assert!(checked_slots(&json!("2026-11-06T19:30:00+01:00"), now).is_err());
        let nine: Vec<String> = (1..=9).map(|d| format!("2026-11-{d:02}T20:00:00+01:00")).collect();
        assert!(checked_slots(&json!(nine), now).is_err());
    }

    #[test]
    fn the_rate_stops_a_flood() {
        let t = 1_000_000.0;
        for _ in 0..RATE_MAX {
            assert!(within_rate("flood-test", t));
        }
        assert!(!within_rate("flood-test", t + 10.0));
        assert!(within_rate("flood-test", t + 3601.0));
        assert!(within_rate("other-test", t));
    }

    #[test]
    fn the_common_slots_are_what_both_have() {
        let m = json!({"role": "host", "offered": ["2026-11-06T19:30:00+01:00", "2026-11-07T19:30:00+01:00"],
                       "answer": ["2026-11-07T18:30:00Z", "2026-11-09T19:30:00+01:00"]});
        assert_eq!(view(&m)["common"], json!(["2026-11-07T18:30:00Z"]));
        assert_eq!(view(&json!({"role": "guest"}))["common"], json!([]));
    }
}
