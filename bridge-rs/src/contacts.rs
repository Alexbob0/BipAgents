//! The person's address book, for the agents they choose (decided 2026-10-22), shared the way the calendar is
//! (calendar.rs): no server reaches it (the iPhone's Contacts, Android's ContactsContract, where the Google contacts
//! synced on the phone are too), so the app sends a copy here when it opens (and when woken by a silent push) and
//! applies what the agents asked for. The person shares the whole address book or only some of its groups (labels),
//! with or without the contacts' notes (off unless ticked): the choice is kept here, per server (`scope`), the phone
//! sends only what it covers and the bridge keeps only that too. No photos.
//!
//!     contacts_search       contacts by name, nickname, company, e-mail, phone or group
//!     contacts_birthdays    the birthdays of the next N days
//!     contacts_list_group   a group's contacts (or the groups, without one)
//!     contacts_add          a new contact, done by the phone
//!     contacts_update       a contact changed (its name, company, birthday…; phones, e-mails, an address added)
//!
//! Nothing is deleted from here. Like the calendar, an agent's request needs no approval of its own: the person chose
//! the agents that may use the address book. The contacts are other people: the tools tell the agents never to write
//! to them without the person's approval.
//! The tools are listed only for the agents allowed (as `health` and the calendar): the tools' entry of an agent
//! changes when it is allowed or not, so its Hermes fetches the list again.
//!
//! App routes (the bridge key):
//!
//!     GET  /v1/contacts                  {agents, at, scope, groups: n, contacts: n, pending}
//!     PUT  /v1/contacts                  {groups: [{id, title}], contacts: [{id, name, nickname?, phones?, emails?,
//!                                        addresses?: [{label?, value}], birthday?, organization?, job?, notes?,
//!                                        relations?: [{label?, name}], groups?: [group ids]}]}
//!     PUT  /v1/contacts/scope            {all, groups: [group ids], notes}: what the phone shares with this server
//!     PUT  /v1/contacts/agents           {agents}
//!     GET  /v1/contacts/queue            the agents' requests waiting for the phone
//!     POST /v1/contacts/queue/{id}       {done: bool, error?}
//!
//! `birthday`: YYYY-MM-DD, or --MM-DD when the year isn't known.
//!
//!     data_dir/contacts.json   (chmod 600: the copy, the choice, the agents allowed, the requests)
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{DateTime, Datelike, Duration, NaiveDate};
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
/// Contacts one search answers with at most (in full).
pub const MAX_FOUND: usize = 30;
/// Contacts a group's list shows at most (one line each).
pub const MAX_LISTED: usize = 500;
/// The address book's size the phone may send.
pub const MAX_CONTACTS: usize = 20000;
/// `contacts_birthdays`' period by default.
pub const DEFAULT_DAYS: i64 = 30;
pub const TOOL_NAMES: [&str; 5] =
    ["contacts_search", "contacts_birthdays", "contacts_list_group", "contacts_add", "contacts_update"];

/// The contacts are third parties: said in each tool an agent reads them with.
const THIRD_PARTIES: &str = "These are other people: never write to, call or share a contact's details with anyone \
without the person's explicit approval.";

/// The tools' definitions (listed only for an agent allowed).
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": "contacts_search",
            "description": format!("Find people in the person's address book (as their phone last shared it) by name, \
        nickname, company, e-mail or phone number: their phones, e-mails, birthday, addresses, company and the groups \
        they are in. group: only in that group. {THIRD_PARTIES}"),
            "inputSchema": {"type": "object", "properties": {
                "query": {"type": "string", "description": "a name, nickname, company, e-mail or phone number"},
                "group": {"type": "string"}}},
        }),
        json!({
            "name": "contacts_birthdays",
            "description": format!("The birthdays of the person's contacts in the next days (default 30, up to 366), \
        soonest first, with the age they turn when the year is known. group: only that group's. {THIRD_PARTIES}"),
            "inputSchema": {"type": "object", "properties": {
                "days": {"type": "integer"}, "group": {"type": "string"}}},
        }),
        json!({
            "name": "contacts_list_group",
            "description": format!("The contacts of one group (label) of the person's address book, one line each \
        (phones and e-mails), e.g. to prepare a list of people to invite; without a group, the groups and their size. \
        {THIRD_PARTIES}"),
            "inputSchema": {"type": "object", "properties": {"group": {"type": "string"}}},
        }),
        json!({
            "name": "contacts_add",
            "description": "Add a contact to the person's address book (it appears when their phone next syncs, \
        usually within minutes). Only when the person asked for it. phones, emails: lists; birthday: YYYY-MM-DD or \
        --MM-DD; group: a group's name.",
            "inputSchema": {"type": "object", "properties": {
                "name": {"type": "string"}, "nickname": {"type": "string"},
                "phones": {"type": "array", "items": {"type": "string"}},
                "emails": {"type": "array", "items": {"type": "string"}},
                "birthday": {"type": "string"}, "address": {"type": "string"},
                "organization": {"type": "string"}, "job": {"type": "string"}, "notes": {"type": "string"},
                "group": {"type": "string"}},
                "required": ["name"]},
        }),
        json!({
            "name": "contacts_update",
            "description": "Change one of the person's contacts (its id from contacts_search): only the fields given. \
        Only when the person asked for it. add_phones, add_emails, add_address are added to what it has (nothing is \
        removed); add_to_group: a group's name. Contacts are never deleted from here.",
            "inputSchema": {"type": "object", "properties": {
                "id": {"type": "string"}, "name": {"type": "string"}, "nickname": {"type": "string"},
                "birthday": {"type": "string"}, "organization": {"type": "string"}, "job": {"type": "string"},
                "notes": {"type": "string"},
                "add_phones": {"type": "array", "items": {"type": "string"}},
                "add_emails": {"type": "array", "items": {"type": "string"}},
                "add_address": {"type": "string"}, "add_to_group": {"type": "string"}},
                "required": ["id"]},
        }),
    ]
}

fn path_of(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("contacts.json")
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

/// Whether `agent` may use the address book (and gets its tools).
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

fn items(item: &Value, key: &str) -> Vec<Value> {
    item.get(key).and_then(Value::as_array).cloned().unwrap_or_default()
}

/// Lower case, without accents: « Hélène » is found by « helene ».
fn folded(value: &str) -> String {
    tk::ascii_fold(&value.to_lowercase())
}

/// A phone number's digits, its last 9 (« +33 6 12 34 56 78 » and « 06 12 34 56 78 » the same).
fn phone_key(value: &str) -> String {
    let digits: Vec<char> = value.chars().filter(char::is_ascii_digit).collect();
    digits[digits.len().saturating_sub(9)..].iter().collect()
}

/// A birthday as the phone or an agent writes it: (year if known, month, day). « 1990-05-12 », « --05-12 », « 05-12 ».
pub fn birthday(value: &str) -> Option<(Option<i32>, u32, u32)> {
    let value = value.trim();
    if let Ok(day) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        return Some((Some(day.year()), day.month(), day.day()));
    }
    let rest = value.strip_prefix("--").unwrap_or(value);
    let (month, day) = rest.split_once('-')?;
    let (month, day) = (month.parse::<u32>().ok()?, day.parse::<u32>().ok()?);
    // 2000 is a leap year: 02-29 is a day.
    NaiveDate::from_ymd_opt(2000, month, day).map(|_| (None, month, day))
}

/// A birthday as stored: YYYY-MM-DD, or --MM-DD without its year.
fn birthday_text((year, month, day): (Option<i32>, u32, u32)) -> String {
    match year {
        Some(year) => format!("{year:04}-{month:02}-{day:02}"),
        None => format!("--{month:02}-{day:02}"),
    }
}

/// The next time a birthday comes, from `today` (a 29 February on the 28th in other years).
fn next_birthday(today: NaiveDate, month: u32, day: u32) -> Option<NaiveDate> {
    let on =
        |year: i32| NaiveDate::from_ymd_opt(year, month, day).or_else(|| NaiveDate::from_ymd_opt(year, month, day - 1));
    let this_year = on(today.year())?;
    if this_year >= today {
        Some(this_year)
    } else {
        on(today.year() + 1)
    }
}

fn now_or(now: Option<f64>) -> f64 {
    now.filter(|n| *n != 0.0).unwrap_or_else(py::time_time)
}

fn today(now: f64) -> NaiveDate {
    DateTime::from_timestamp(now as i64, 0).map(|d| d.with_timezone(&chrono::Local).date_naive()).unwrap_or_default()
}

/// « mobile: +33 6 12 34 56 78 » (the label when there is one).
fn labelled(entry: &Value, key: &str) -> String {
    match (text(entry, "label"), text(entry, key)) {
        (label, value) if label.is_empty() => value,
        (label, value) => format!("{label}: {value}"),
    }
}

fn joined(contact: &Value, key: &str, value: &str) -> String {
    items(contact, key).iter().map(|e| labelled(e, value)).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("; ")
}

/// One contact in full, for `contacts_search`.
fn shown(contact: &Value, group_title: &dyn Fn(&str) -> Option<String>) -> String {
    let mut line = format!("- id {} · {}", text(contact, "id"), text(contact, "name"));
    if !text(contact, "nickname").is_empty() {
        line += &format!(" (« {} »)", text(contact, "nickname"));
    }
    let work: Vec<String> =
        [text(contact, "organization"), text(contact, "job")].into_iter().filter(|s| !s.is_empty()).collect();
    if !work.is_empty() {
        line += &format!(" · {}", work.join(", "));
    }
    let mut more = Vec::new();
    for (key, value, title) in
        [("phones", "value", "phones"), ("emails", "value", "e-mails"), ("addresses", "value", "address")]
    {
        let all = joined(contact, key, value);
        if !all.is_empty() {
            more.push(format!("{title}: {all}"));
        }
    }
    if let Some(day) = birthday(&text(contact, "birthday")) {
        more.push(format!("birthday: {}", birthday_text(day)));
    }
    let relations = joined(contact, "relations", "name");
    if !relations.is_empty() {
        more.push(format!("relations: {relations}"));
    }
    let groups: Vec<String> =
        items(contact, "groups").iter().filter_map(|g| g.as_str().and_then(group_title)).collect();
    if !groups.is_empty() {
        more.push(format!("groups: {}", groups.join(", ")));
    }
    if !text(contact, "notes").is_empty() {
        more.push(format!("notes: {}", tk::head(&text(contact, "notes"), 300)));
    }
    for extra in more {
        line += &format!("\n  {extra}");
    }
    line
}

/// One contact on one line, for a group's list: its name, first phone and first e-mail.
fn short(contact: &Value) -> String {
    let first = |key: &str| items(contact, key).first().map(|e| text(e, "value")).unwrap_or_default();
    let mut line = format!("- id {} · {}", text(contact, "id"), text(contact, "name"));
    for value in [first("phones"), first("emails")] {
        if !value.is_empty() {
            line += &format!(" · {value}");
        }
    }
    line
}

/// The strings of an argument given as a list (or as one string).
fn strings(arguments: &Dict, key: &str, max: usize) -> Vec<String> {
    let values = match arguments.get(key) {
        Some(Value::Array(values)) => values.iter().map(|v| tk::str_or_empty(Some(v))).collect(),
        Some(Value::String(one)) => vec![one.clone()],
        _ => Vec::new(),
    };
    values.iter().map(|v| tk::head(tk::strip(v), max).to_string()).filter(|v| !v.is_empty()).take(10).collect()
}

/// One of the contact tools, for `agent`. The phone's requests go to its queue. Err: a file that cannot be written.
pub fn call(config: &Config, agent: &str, name: &str, arguments: &Dict, now: Option<f64>) -> anyhow::Result<Dict> {
    let now = now_or(now);
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = read(config);
    if !tk::py_in(agent, data.get("agents").unwrap_or(&json!([]))) {
        return Ok(tool_text(
            "The person hasn't shared their address book with you: they switch « Contacts » on in your page of the \
BipAgents app.",
            true,
        ));
    }
    let groups = list_of(&data, "groups");
    let group_title = |id: &str| groups.iter().rev().find(|g| text(g, "id") == id).map(|g| text(g, "title"));
    // A group's id from its name (or its id).
    let group_id = |wanted: &str| -> Option<String> {
        let wanted = folded(wanted);
        groups.iter().find(|g| folded(&text(g, "title")) == wanted || text(g, "id") == wanted).map(|g| text(g, "id"))
    };
    let contacts = list_of(&data, "contacts");
    let arg = |k: &str| tk::strip(&tk::str_or_empty(arguments.get(k))).to_string();
    let since = match data.get("at").and_then(Value::as_f64) {
        Some(at) => format!("as the phone shared it {} ago", age(now - at)),
        None => "the phone hasn't shared it yet".to_string(),
    };
    // The contacts of the group asked for (all without one). Err: no such group.
    let in_group = |wanted: &str| -> Result<Vec<&Value>, String> {
        if wanted.is_empty() {
            return Ok(contacts.iter().collect());
        }
        let Some(id) = group_id(wanted) else {
            let names: Vec<String> = groups.iter().map(|g| text(g, "title")).collect();
            return Err(if names.is_empty() {
                "no group shared (the person shares their address book without groups)".to_string()
            } else {
                format!("no such group; the groups: {}", names.join(", "))
            });
        };
        Ok(contacts.iter().filter(|c| items(c, "groups").iter().any(|g| g.as_str() == Some(id.as_str()))).collect())
    };
    match name {
        "contacts_search" => {
            let query = arg("query");
            let rows = match in_group(&arg("group")) {
                Ok(rows) => rows,
                Err(why) => return Ok(tool_text(&why, true)),
            };
            if query.is_empty() && arg("group").is_empty() {
                return Ok(tool_text("a query is needed (a name, an e-mail, a phone number…)", true));
            }
            let wanted = folded(&query);
            let digits = phone_key(&query);
            let found: Vec<&&Value> = rows
                .iter()
                .filter(|c| {
                    let words = ["name", "nickname", "organization", "job"]
                        .iter()
                        .any(|k| folded(&text(c, k)).contains(&wanted));
                    let mail = items(c, "emails").iter().any(|e| folded(&text(e, "value")).contains(&wanted));
                    let phone = digits.len() >= 4
                        && items(c, "phones").iter().any(|p| phone_key(&text(p, "value")).contains(&digits));
                    words || mail || phone
                })
                .collect();
            if found.is_empty() {
                return Ok(tool_text(&format!("No contact matches ({since})."), false));
            }
            let lines: Vec<String> = found.iter().take(MAX_FOUND).map(|c| shown(c, &group_title)).collect();
            let more = if found.len() > MAX_FOUND {
                format!("\n({} more: ask more precisely)", found.len() - MAX_FOUND)
            } else {
                String::new()
            };
            Ok(tool_text(&format!("{} found, {since}:\n{}{more}", found.len(), lines.join("\n")), false))
        }
        "contacts_birthdays" => {
            let days = match arguments.get("days").map(|v| py::py_float(v).map(|d| d as i64)) {
                None | Some(Ok(0)) => DEFAULT_DAYS,
                Some(Ok(d)) if (1..=366).contains(&d) => d,
                _ => return Ok(tool_text("days: from 1 to 366", true)),
            };
            let rows = match in_group(&arg("group")) {
                Ok(rows) => rows,
                Err(why) => return Ok(tool_text(&why, true)),
            };
            let today = today(now);
            let last = today + Duration::days(days);
            let mut coming: Vec<(NaiveDate, String)> = rows
                .iter()
                .filter_map(|c| {
                    let (year, month, day) = birthday(&text(c, "birthday"))?;
                    let on = next_birthday(today, month, day).filter(|d| *d <= last)?;
                    let when = match (on - today).num_days() {
                        0 => "today".to_string(),
                        1 => "tomorrow".to_string(),
                        n => format!("in {n} days"),
                    };
                    let turns = year.map(|y| format!(" · turns {}", on.year() - y)).unwrap_or_default();
                    Some((on, format!("- {on} ({when}) · {}{turns} · id {}", text(c, "name"), text(c, "id"))))
                })
                .collect();
            coming.sort();
            if coming.is_empty() {
                return Ok(tool_text(&format!("No birthday in the next {days} days ({since})."), false));
            }
            let lines: Vec<String> = coming.into_iter().map(|(_, line)| line).collect();
            Ok(tool_text(&format!("The next {days} days, {since}:\n{}", lines.join("\n")), false))
        }
        "contacts_list_group" => {
            let wanted = arg("group");
            if wanted.is_empty() {
                if groups.is_empty() {
                    return Ok(tool_text(&format!("No group shared; {} contacts ({since}).", contacts.len()), false));
                }
                let lines: Vec<String> = groups
                    .iter()
                    .map(|g| {
                        let id = text(g, "id");
                        let size = contacts
                            .iter()
                            .filter(|c| items(c, "groups").iter().any(|x| x.as_str() == Some(id.as_str())))
                            .count();
                        format!("- {} ({size})", text(g, "title"))
                    })
                    .collect();
                return Ok(tool_text(
                    &format!("The groups, {since} ({} contacts in all):\n{}", contacts.len(), lines.join("\n")),
                    false,
                ));
            }
            let rows = match in_group(&wanted) {
                Ok(rows) => rows,
                Err(why) => return Ok(tool_text(&why, true)),
            };
            if rows.is_empty() {
                return Ok(tool_text(&format!("Nobody in « {wanted} » ({since})."), false));
            }
            let mut rows = rows;
            rows.sort_by_key(|c| folded(&text(c, "name")));
            let lines: Vec<String> = rows.iter().take(MAX_LISTED).map(|c| short(c)).collect();
            let more =
                if rows.len() > MAX_LISTED { format!("\n({} more)", rows.len() - MAX_LISTED) } else { String::new() };
            Ok(tool_text(
                &format!("« {wanted} », {} contacts, {since}:\n{}{more}", rows.len(), lines.join("\n")),
                false,
            ))
        }
        "contacts_add" | "contacts_update" => {
            let mut args = Dict::new();
            let adding = name == "contacts_add";
            if !adding {
                let id = arg("id");
                if !contacts.iter().any(|c| text(c, "id") == id) {
                    return Ok(tool_text("unknown contact id (see contacts_search)", true));
                }
                args.insert("id".into(), json!(id));
            } else if arg("name").is_empty() {
                return Ok(tool_text("a name is needed", true));
            }
            for (key, max) in [("name", 300), ("nickname", 200), ("organization", 300), ("job", 300), ("notes", 4000)] {
                if arguments.contains_key(key) {
                    args.insert(key.into(), json!(tk::head(&arg(key), max)));
                }
            }
            if !adding && args.contains_key("notes") && !notes_shared(&data) {
                // Its notes aren't shared: written over without being seen, they would be lost.
                return Ok(tool_text("the person doesn't share their contacts' notes: notes can't be changed", true));
            }
            if !adding && args.get("name").and_then(Value::as_str) == Some("") {
                return Ok(tool_text("a contact keeps a name", true));
            }
            let value = arg("birthday");
            if !value.is_empty() {
                let Some(day) = birthday(&value) else {
                    return Ok(tool_text("birthday: YYYY-MM-DD, or --MM-DD without the year", true));
                };
                args.insert("birthday".into(), json!(birthday_text(day)));
            }
            let (phones, emails, address, group) = if adding {
                ("phones", "emails", "address", "group")
            } else {
                ("add_phones", "add_emails", "add_address", "add_to_group")
            };
            for (key, field, max) in [(phones, "phones", 100), (emails, "emails", 300)] {
                let values = strings(arguments, key, max);
                if !values.is_empty() {
                    args.insert(field.into(), json!(values));
                }
            }
            let value = arg(address);
            if !value.is_empty() {
                args.insert("address".into(), json!(tk::head(&value, 500)));
            }
            let value = arg(group);
            if !value.is_empty() {
                let Some(id) = group_id(&value) else {
                    return Ok(tool_text("no such group (see contacts_list_group)", true));
                };
                args.insert("group".into(), json!(id));
            }
            if !adding && args.len() == 1 {
                return Ok(tool_text("nothing to change", true));
            }
            let mut queue = list_of(&data, "queue");
            let args_v = Value::Object(args.clone());
            let twin = if adding {
                crate::phone_asks::twin(&queue, agent, now, |a| a == &args_v)
            } else {
                queue.iter().find(|op| crate::phone_asks::waiting(op) && op.get("args") == Some(&args_v))
            };
            if let Some(twin) = twin {
                // phone_asks.rs: the same request again (a model checking its work in a copy that doesn't show it yet)
                return Ok(tool_text(
                    &format!(
                        "Already asked (request {}): not asked again. The contact shows in contacts_search once the \
phone has done it and shared the address book again.",
                        text(twin, "id")
                    ),
                    false,
                ));
            }
            queue.push(json!({"id": py::token_hex(6), "op": if adding { "add" } else { "update" }, "agent": agent,
                              "created": now, "state": "pending", "args": args}));
            data.insert("queue".into(), Value::Array(queue));
            write(config, &data)?;
            Ok(tool_text(ASKED, false))
        }
        _ => Ok(tool_text(&format!("unknown tool: {name}"), true)),
    }
}

/// What a request queued for the phone answers (the phone is woken when it starts so).
pub const ASKED: &str =
    "Asked: the person's phone does it at its next sync (usually within minutes, or when the app opens).";

/// Whether the person shares their contacts' notes with this server.
fn notes_shared(data: &Dict) -> bool {
    data.get("scope").and_then(|s| s.get("notes")).map(py::truthy).unwrap_or(false)
}

// -- what is kept: the person's choice ------------------------------------------------------------------------------

/// The person's choice for this server: the whole address book or some groups, the notes or not. None of it set: the
/// whole address book, without the notes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Scope {
    #[serde(default = "yes", deserialize_with = "web::lax::bool")]
    pub all: bool,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, deserialize_with = "web::lax::bool")]
    pub notes: bool,
}

fn yes() -> bool {
    true
}

impl Default for Scope {
    fn default() -> Self {
        Scope { all: true, groups: Vec::new(), notes: false }
    }
}

impl Validate for Scope {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "groups", Some(&self.groups), 200);
        for (n, g) in self.groups.iter().enumerate() {
            nested_max_len(errors, vec![json!("body"), json!("groups"), json!(n)], g, 300);
        }
    }
}

fn scope_of(data: &Dict) -> Scope {
    data.get("scope").and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default()
}

/// Only what the choice covers: the groups chosen (and their contacts), without the notes unless ticked.
fn narrowed(scope: &Scope, groups: Vec<Value>, contacts: Vec<Value>) -> (Vec<Value>, Vec<Value>) {
    let chosen = |id: &str| scope.all || scope.groups.iter().any(|g| g == id);
    let groups: Vec<Value> = groups.into_iter().filter(|g| chosen(&text(g, "id"))).collect();
    let contacts = contacts
        .into_iter()
        .filter(|c| scope.all || items(c, "groups").iter().any(|g| g.as_str().map(chosen).unwrap_or(false)))
        .map(|mut c| {
            if let Some(map) = c.as_object_mut() {
                if !scope.notes {
                    map.remove("notes");
                }
                if !scope.all {
                    // The groups it is in among the chosen ones (the others' names stay on the phone).
                    let kept: Vec<Value> = items(&Value::Object(map.clone()), "groups")
                        .into_iter()
                        .filter(|g| g.as_str().map(chosen).unwrap_or(false))
                        .collect();
                    map.insert("groups".into(), Value::Array(kept));
                }
            }
            c
        })
        .collect();
    (groups, contacts)
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

fn nested_max_items(errors: &mut Vec<FieldError>, loc: Vec<Value>, count: usize, max: usize) {
    if count > max {
        errors.push(
            FieldError::new("too_long", loc, format!("List should have at most {max} items"), json!(count))
                .with_ctx(json!({"max_length": max})),
        );
    }
}

/// A phone, an e-mail or an address, with its label (« mobile », « home »…).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Entry {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub value: String,
}

/// Someone the contact is related to, as the phone has it (« mother: Anne »).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Relation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Contact {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nickname: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phones: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub emails: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub addresses: Vec<Entry>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub birthday: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub organization: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub job: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<Relation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Group {
    pub id: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub contacts: Vec<Contact>,
}

impl Validate for Snapshot {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "groups", Some(&self.groups), 500);
        check_max_items(errors, "contacts", Some(&self.contacts), MAX_CONTACTS);
        for (n, g) in self.groups.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("groups"), json!(n), json!(f)];
            nested_max_len(errors, loc("id"), &g.id, 300);
            nested_max_len(errors, loc("title"), &g.title, 200);
        }
        for (n, c) in self.contacts.iter().enumerate() {
            let loc = |f: &str| vec![json!("body"), json!("contacts"), json!(n), json!(f)];
            for (field, value, max) in [
                ("id", &c.id, 300),
                ("name", &c.name, 300),
                ("nickname", &c.nickname, 200),
                ("birthday", &c.birthday, 20),
                ("organization", &c.organization, 300),
                ("job", &c.job, 300),
                ("notes", &c.notes, 4000),
            ] {
                nested_max_len(errors, loc(field), value, max);
            }
            for (field, entries, max) in
                [("phones", &c.phones, 100), ("emails", &c.emails, 300), ("addresses", &c.addresses, 500)]
            {
                nested_max_items(errors, loc(field), entries.len(), 30);
                for (m, e) in entries.iter().enumerate() {
                    let at =
                        |f: &str| vec![json!("body"), json!("contacts"), json!(n), json!(field), json!(m), json!(f)];
                    nested_max_len(errors, at("label"), &e.label, 60);
                    nested_max_len(errors, at("value"), &e.value, max);
                }
            }
            nested_max_items(errors, loc("relations"), c.relations.len(), 30);
            for (m, r) in c.relations.iter().enumerate() {
                let at =
                    |f: &str| vec![json!("body"), json!("contacts"), json!(n), json!("relations"), json!(m), json!(f)];
                nested_max_len(errors, at("label"), &r.label, 60);
                nested_max_len(errors, at("name"), &r.name, 200);
            }
            nested_max_items(errors, loc("groups"), c.groups.len(), 100);
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

fn dump<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let data = blocking(move || Ok(read(&config))).await?;
    Ok(web::ok(json!({
        "agents": data.get("agents").cloned().unwrap_or(json!([])), "at": data.get("at").cloned().unwrap_or(Value::Null),
        "scope": dump(&scope_of(&data)),
        "groups": list_of(&data, "groups").len(), "contacts": list_of(&data, "contacts").len(),
        "pending": list_of(&data, "queue").iter().filter(|op| pending(op)).count(),
    })))
}

async fn snapshot(State(state): State<AppState>, Json(body): Json<Snapshot>) -> ApiResult<Response> {
    let config = state.config();
    let kept = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let (groups, contacts) = narrowed(
            &scope_of(&data),
            body.groups.iter().map(dump).collect(),
            body.contacts.iter().map(dump).collect(),
        );
        let kept = contacts.len();
        data.insert("groups".into(), Value::Array(groups));
        data.insert("contacts".into(), Value::Array(contacts));
        data.insert("at".into(), json!(py::time_time()));
        let cutoff = py::time_time() - (QUEUE_DAYS * 86400) as f64;
        let queue: Vec<Value> = list_of(&data, "queue")
            .into_iter()
            .filter(|op| pending(op) || op.get("created").and_then(Value::as_f64).map(|c| c > cutoff).unwrap_or(false))
            .collect();
        data.insert("queue".into(), Value::Array(queue));
        write(&config, &data)?;
        Ok(kept)
    })
    .await?;
    Ok(web::ok(json!({"contacts": kept})))
}

/// The person's choice for this server; what was kept and no longer covered goes at once.
async fn set_scope(State(state): State<AppState>, Json(body): Json<Scope>) -> ApiResult<Response> {
    let config = state.config();
    let scope = blocking(move || {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut data = read(&config);
        let (groups, contacts) = narrowed(&body, list_of(&data, "groups"), list_of(&data, "contacts"));
        data.insert("groups".into(), Value::Array(groups));
        data.insert("contacts".into(), Value::Array(contacts));
        data.insert("scope".into(), dump(&body));
        write(&config, &data)?;
        Ok(body)
    })
    .await?;
    Ok(web::ok(json!({"scope": dump(&scope)})))
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
        if names.is_empty() {
            // Shared with no agent any more: the copy goes too (the phone stops sending it).
            data.remove("contacts");
            data.remove("groups");
            data.remove("at");
        }
        write(&config_c, &data)?;
        Ok((before, names))
    })
    .await?;
    // The agents allowed or no longer: their tools' entry changes (the address book's tools listed or not).
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
        .route("/contacts", get(status).put(snapshot))
        .route("/contacts/scope", put(set_scope))
        .route("/contacts/agents", put(set_agents))
        .route("/contacts/queue", get(queue))
        .route("/contacts/queue/{op_id}", post(done))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn birthdays_as_written() {
        assert_eq!(birthday("1990-05-12"), Some((Some(1990), 5, 12)));
        assert_eq!(birthday("--05-12"), Some((None, 5, 12)));
        assert_eq!(birthday("02-29"), Some((None, 2, 29)));
        assert_eq!(birthday("13-01"), None);
        assert_eq!(birthday("demain"), None);
        assert_eq!(birthday_text((None, 2, 9)), "--02-09");
        let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(next_birthday(day("2026-10-22"), 10, 22), Some(day("2026-10-22")));
        assert_eq!(next_birthday(day("2026-10-22"), 1, 3), Some(day("2027-01-03")));
        assert_eq!(next_birthday(day("2027-01-01"), 2, 29), Some(day("2027-02-28")));
    }

    #[test]
    fn phones_compared_by_their_last_digits() {
        assert_eq!(phone_key("+33 6 12 34 56 78"), phone_key("06.12.34.56.78"));
        assert_eq!(phone_key("12 34"), "1234");
    }

    #[test]
    fn the_choice_narrows_what_is_kept() {
        let groups = vec![json!({"id": "g1", "title": "Famille"}), json!({"id": "g2", "title": "Travail"})];
        let contacts = vec![
            json!({"id": "a", "name": "Anne", "groups": ["g1", "g2"], "notes": "allergique"}),
            json!({"id": "b", "name": "Bob", "groups": ["g2"]}),
            json!({"id": "c", "name": "Chloé"}),
        ];
        let some = Scope { all: false, groups: vec!["g1".into()], notes: false };
        let (g, c) = narrowed(&some, groups.clone(), contacts.clone());
        assert_eq!(g, vec![json!({"id": "g1", "title": "Famille"})]);
        assert_eq!(c, vec![json!({"id": "a", "name": "Anne", "groups": ["g1"]})]);
        let (_, c) = narrowed(&Scope { notes: true, ..Scope::default() }, groups, contacts);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0]["notes"], "allergique");
    }
}
