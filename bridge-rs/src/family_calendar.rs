//! The family agent and its members' calendars (Alex, 12/11: « quand tu partages ton calendrier, tu choisis déjà les
//! agents »). Each person's phone calendar stays on HER place (calendar.rs: a copy from her phone, on her bridge); she
//! may pick the family agents she is in among the agents allowed to use it (`<agent>__<household>` on her side, off by
//! default), with the same read/write rights as her own agents (phone_rights.rs). The family agent lives on its
//! creator's place, so:
//!
//! - **reading** (`calendar_list`, in anyone's turn and its routines): the creator's own copy (when he allowed the
//!   agent) and, asked now through the household link, each member's: her bridge checks the key, that the agent is a
//!   family agent she is in, and that her calendar.json allows it, then answers exactly what calendar.rs would show that
//!   agent (`calendar::call` under its id there), never more. The events come under the person's first name (her
//!   household's name). Nothing is kept on the creator's place: each call asks again. A member's bridge that doesn't
//!   answer gives one line, never a failure.
//! - **writing** (`calendar_add`, `_update`, `_delete`): only on the calendar of a person who gave it the right to
//!   change it; `who` names her (default: the person writing in this turn, when she allows it). A member's request is
//!   queued on HER bridge under the agent's id there and her phone does it, as for her own agents (no extra approval:
//!   she granted it).
//! - **the tools** are listed for the family agent only while someone allows it: the creator's calendar.json, and for
//!   members the flags their bridges send at each change (`POST /family/fa/calendar`, below; off, she leaves or is taken
//!   out: the flag goes at once). The flags only say who allows what; each call asks her bridge, which decides.
//!
//! Between the bridges (the household key):
//!
//!     POST /family/calendar/{agent}      on a MEMBER's bridge, from the creator's: {name, arguments}
//!                                        → {allowed, read, write, result?}   (404: no family agent of the caller's
//!                                        she is in)
//!     POST /family/fa/calendar           on the CREATOR's bridge, from a member's: {agent, on, read, write}
//!                                        (what she now allows that family agent) → {ok}
//!
//! `<data>/family-agent.json` `calendars: {agent: {household: {read, write}}}` on the creator's place.
//!
//! No Python counterpart.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{split, SEPARATOR};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// How long the creator's bridge waits for a member's.
pub const ASK_SECONDS: u64 = 8;

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

fn text_of(result: &Dict) -> String {
    result
        .get("content")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn failed(result: &Dict) -> bool {
    result.get("isError").is_some_and(py::truthy)
}

// -- a member's side ------------------------------------------------------------------------------------------------

/// Whether `id` (`<agent>__<household>`) is a family agent this person is in: her calendar may be offered to it.
pub fn is_member_agent(config: &Config, id: &str) -> bool {
    crate::family_agent::is_family_of_household(config, id)
}

/// What she allows that family agent now: (on, read, write).
fn allowed(config: &Config, id: &str) -> (bool, bool, bool) {
    let on = crate::calendar::enabled(config, id);
    (on, on && crate::calendar::reads(config, id), on && crate::calendar::writes(config, id))
}

/// Her picker changed for these agents: the creator's bridge of each family agent among them is told what she allows
/// now (in the background; its next ask corrects a missed one).
pub fn tell_owners(state: &AppState, ids: &[String]) {
    let config = state.config();
    for id in ids.iter().filter(|id| id.contains(SEPARATOR)) {
        let Some((agent, household)) = split(id) else { continue };
        let (on, read, write) = if is_member_agent(&config, id) { allowed(&config, id) } else { (false, false, false) };
        tell(state, &household, json!({"agent": agent, "on": on, "read": read, "write": write}));
    }
}

/// She no longer uses that family agent (she left it, or its creator took her out): her calendar isn't offered to it
/// any more, and its creator's bridge is told at once.
pub fn left(state: &AppState, id: &str) {
    let config = state.config();
    let was = crate::calendar::enabled(&config, id);
    crate::calendar::forget(&config, id);
    if let (true, Some((agent, household))) = (was, split(id)) {
        tell(state, &household, json!({"agent": agent, "on": false, "read": false, "write": false}));
    }
}

fn tell(state: &AppState, household: &str, body: Value) {
    let Some(entry) = state.households.get(household) else { return };
    let http = state.http.clone();
    let household = household.to_string();
    state.spawn(async move {
        let url = format!("{}/family/fa/calendar", crate::family_relay::reach_url(&entry));
        let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
        let sent = crate::door_tls::household_client(&http, &entry)
            .post(url)
            .timeout(Duration::from_secs(15))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        tracing::info!(household = %household, sent = sent, "family calendar: the creator told");
    });
}

fn body_json(bytes: &[u8]) -> ApiResult<Dict> {
    if bytes.is_empty() {
        return Ok(Dict::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

/// `POST /family/calendar/{agent}`: the creator's family agent uses this person's calendar, as her calendar tools
/// would let it under its id here (`<agent>__<caller>`): nothing when she didn't pick it.
async fn served(
    State(state): State<AppState>,
    UrlPath(agent): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let household = crate::agent_shares::household_of(&state, &headers)?;
    let config = state.config();
    let id = format!("{agent}{SEPARATOR}{household}");
    if !is_member_agent(&config, &id) {
        return Err(HttpError::new(404, "no such family agent"));
    }
    let body = body_json(&body)?;
    let name = str_of(&Value::Object(body.clone()), "name");
    if !crate::calendar::TOOL_NAMES.contains(&name.as_str()) {
        return Err(HttpError::new(422, "name: a calendar tool"));
    }
    let mut arguments = body.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
    arguments.shift_remove("who");
    let (on, read, write) = allowed(&config, &id);
    if !on {
        return Ok(web::ok(json!({"allowed": false, "read": false, "write": false})));
    }
    let (config_c, id_c, name_c) = (config.clone(), id.clone(), name.clone());
    let result = tokio::task::spawn_blocking(move || {
        crate::calendar::call(&config_c, &id_c, &name_c, &arguments, None, &crate::caldav::Remote::default())
    })
    .await
    .map_err(anyhow::Error::from)??;
    if text_of(&result).starts_with(crate::calendar::ASKED) {
        // her phone woken to do it now (a silent push; else when the app opens)
        if let Some(to) = crate::meet::push_agent(&config) {
            let push = state.push.clone();
            tokio::spawn(async move {
                push.notify_silent(&to, "calendar", Dict::new()).await;
            });
        }
    }
    tracing::info!(agent = %id, tool = %name, "family calendar: used by the family agent");
    Ok(web::ok(json!({"allowed": true, "read": read, "write": write, "result": result})))
}

// -- the creator's side ---------------------------------------------------------------------------------------------

/// A member's household and what she allows the family agent `agent` (her bridge's last word).
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub household: String,
    pub name: String,
    pub read: bool,
    pub write: bool,
}

/// The members of `agent` whose bridges said they allow it (members taken out, or unlinked, left out).
pub fn members(config: &Config, agent: &str) -> Vec<Member> {
    let flags = crate::family_agent::calendar_flags(config, agent);
    crate::agent_shares::shared_with(config, agent)
        .into_iter()
        .filter_map(|household| {
            let flag = flags.get(&household)?;
            Some(Member {
                name: crate::agent_shares::shown_household(config, &household),
                read: flag.get("read").is_some_and(py::truthy),
                write: flag.get("write").is_some_and(py::truthy),
                household,
            })
        })
        .collect()
}

/// Whether the family agent `agent` gets the calendar tools: its creator, or a member, allows it.
pub fn offered(config: &Config, agent: &str) -> bool {
    crate::calendar::enabled(config, agent) || !members(config, agent).is_empty()
}

/// `POST /family/fa/calendar`: what a member now allows one of this place's family agents.
async fn told(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let household = crate::agent_shares::household_of(&state, &headers)?;
    let body = Value::Object(body_json(&body)?);
    let config = state.config();
    let agent = str_of(&body, "agent");
    if !crate::family_agent::is_family(&config, &agent)
        || !crate::agent_shares::shared_with(&config, &agent).contains(&household)
    {
        return Err(HttpError::new(404, "no such family agent"));
    }
    let on = body.get("on").is_some_and(py::truthy);
    let flag = on.then(|| {
        json!({"read": body.get("read").is_some_and(py::truthy), "write": body.get("write").is_some_and(py::truthy)})
    });
    keep(&state, &agent, &household, flag).await;
    Ok(web::ok(json!({"ok": true})))
}

/// A member's flag kept (None: gone); the agent's tools' entry follows when the list changes.
async fn keep(state: &AppState, agent: &str, household: &str, flag: Option<Value>) {
    let config = state.config();
    let before = offered(&config, agent);
    if let Err(err) = crate::family_agent::set_calendar_flag(&config, agent, household, flag) {
        tracing::warn!(agent = %agent, error = %err, "family calendar: flag not kept");
        return;
    }
    let config = state.config();
    if offered(&config, agent) != before {
        if let Some(a) = config.agent(Some(agent)).cloned() {
            if let Err(exc) = crate::agent_tools::register(&config, &a, &crate::agents_admin::default_run()).await {
                tracing::warn!(agent = %agent, error = %exc, "agent tools not registered");
            }
        }
    }
}

/// What a member's bridge answered: (allowed, read, write, the tool's result), or None when it didn't answer.
type Answer = Option<(bool, bool, bool, Option<Dict>)>;

async fn ask(state: &AppState, household: &str, agent: &str, name: &str, arguments: &Dict) -> Answer {
    let entry = state.households.get(household)?;
    let url = format!("{}/family/calendar/{agent}", crate::family_relay::reach_url(&entry));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let resp = crate::door_tls::household_client(&state.http, &entry)
        .post(url)
        .timeout(Duration::from_secs(ASK_SECONDS))
        .bearer_auth(key)
        .json(&json!({"name": name, "arguments": arguments}))
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() == 404 {
        return Some((false, false, false, None)); // she is in no such agent there (any more)
    }
    if !resp.status().is_success() {
        return None;
    }
    let answer = resp.json::<Value>().await.ok()?;
    let truthy = |k: &str| answer.get(k).is_some_and(py::truthy);
    Some((truthy("allowed"), truthy("read"), truthy("write"), answer.get("result").and_then(Value::as_object).cloned()))
}

/// What a member's answer says she allows now: kept, so the tools' list follows her without waiting for her bridge's
/// news.
async fn heal(state: &AppState, agent: &str, member: &Member, answer: &Answer) {
    let Some((on, read, write, _)) = answer else { return };
    if (*on, *read, *write) == (true, member.read, member.write) {
        return;
    }
    let flag = on.then(|| json!({"read": read, "write": write}));
    keep(state, agent, &member.household, flag).await;
}

/// The creator's first name (his household's name), else a plain word.
fn creator_name(state: &AppState) -> String {
    Some(state.households.name()).filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "The member who made you".into())
}

fn first_word(name: &str) -> String {
    crate::phone_asks::folded(name.split_whitespace().next().unwrap_or(""))
}

/// Whether `who` (what the agent wrote) names `name`: the whole name, or its first word, any case or accent.
fn names(who: &str, name: &str) -> bool {
    let who = crate::phone_asks::folded(who.trim());
    !who.is_empty() && (who == crate::phone_asks::folded(name.trim()) || who == first_word(name))
}

/// The family's calendars for one of its agent's calendar tools. `turn`: the member whose turn it is (None: the
/// creator's, or a routine's).
pub async fn call(
    state: &AppState,
    agent: &AgentConfig,
    name: &str,
    arguments: &Dict,
    turn: Option<&crate::shared_pages::Asker>,
) -> anyhow::Result<Dict> {
    let config = state.config();
    let own = crate::calendar::enabled(&config, &agent.name);
    let own_read = own && crate::calendar::reads(&config, &agent.name);
    let own_write = own && crate::calendar::writes(&config, &agent.name);
    let creator = creator_name(state);
    let members = members(&config, &agent.name);
    let mut arguments = arguments.clone();
    let who = tk::strip(&tk::str_or_empty(arguments.shift_remove("who").as_ref())).to_string();
    if !own && members.is_empty() {
        return Ok(tool_text(NOBODY, false));
    }
    if name == "calendar_list" {
        let mut sections: Vec<String> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        if own_read {
            let (c, a, args) = (config.clone(), agent.name.clone(), arguments.clone());
            let result = tokio::task::spawn_blocking(move || {
                crate::calendar::call(&c, &a, "calendar_list", &args, None, &crate::caldav::Remote::default())
            })
            .await??;
            sections.push(format!("## {creator}\n{}", text_of(&result)));
        } else if own {
            sections.push(format!("## {creator}\n{}", CANNOT_READ));
        }
        let asked = futures::future::join_all(
            members.iter().map(|m| ask(state, &m.household, &agent.name, "calendar_list", &arguments)),
        )
        .await;
        for (member, answer) in members.iter().zip(&asked) {
            heal(state, &agent.name, member, answer).await;
            match answer {
                None => notes.push(no_answer(&member.name)),
                Some((false, ..)) => {}
                Some((true, false, ..)) => sections.push(format!("## {}\n{CANNOT_READ}", member.name)),
                Some((true, true, _, result)) => {
                    let text = result.as_ref().map(text_of).unwrap_or_default();
                    sections.push(format!("## {}\n{text}", member.name));
                }
            }
        }
        if sections.is_empty() && notes.is_empty() {
            return Ok(tool_text(NOBODY, false));
        }
        let mut out = format!("{LIST_HEAD}\n\n{}", sections.join("\n\n"));
        if !notes.is_empty() {
            out += &format!("\n\n{}", notes.join("\n"));
        }
        return Ok(tool_text(out.trim(), false));
    }
    if !crate::calendar::TOOL_NAMES.contains(&name) {
        return Ok(tool_text(&format!("unknown tool: {name}"), true));
    }
    // whose calendar: `who`, else the person writing in this turn
    enum Target<'a> {
        Creator,
        Member(&'a Member),
    }
    let writers: Vec<String> = own_write
        .then(|| creator.clone())
        .into_iter()
        .chain(members.iter().filter(|m| m.write).map(|m| m.name.clone()))
        .collect();
    let target = if who.is_empty() {
        match turn {
            Some(t) => members.iter().find(|m| m.household == t.household).map(Target::Member),
            None => own.then_some(Target::Creator),
        }
    } else if names(&who, &creator) && own {
        Some(Target::Creator)
    } else {
        members.iter().find(|m| names(&who, &m.name)).map(Target::Member)
    };
    let writable = match &target {
        Some(Target::Creator) => own_write,
        Some(Target::Member(m)) => m.write,
        None => false,
    };
    if !writable {
        if writers.is_empty() {
            return Ok(tool_text(NOBODY_WRITES, true));
        }
        let named = match &target {
            Some(Target::Creator) => Some(creator.clone()),
            Some(Target::Member(m)) => Some(m.name.clone()),
            None => None,
        };
        let why = match named {
            Some(person) if !who.is_empty() || turn.is_some() => format!(
                "{person} doesn't let you change their calendar (they choose it in Réglages › Calendrier of their app). \
Calendars you may change: {}. Say whose with who.",
                writers.join(", ")
            ),
            _ => format!("Whose calendar? Give who: {}.", writers.join(", ")),
        };
        return Ok(tool_text(&why, true));
    }
    match target {
        Some(Target::Member(member)) => {
            let answer = ask(state, &member.household, &agent.name, name, &arguments).await;
            heal(state, &agent.name, member, &answer).await;
            Ok(match answer {
                None => tool_text(&format!("{} Nothing was asked: try again later.", no_answer(&member.name)), true),
                Some((true, _, true, Some(result))) => {
                    let mut said = result.clone();
                    let text = text_of(&result).replace("the person's phone", &format!("{}'s phone", member.name));
                    said.insert("content".into(), json!([{"type": "text", "text": text}]));
                    said
                }
                _ => tool_text(
                    &format!("{} doesn't let you change their calendar any more.", member.name),
                    true,
                ),
            })
        }
        _ => {
            let (c, a, n, args) = (config.clone(), agent.name.clone(), name.to_string(), arguments.clone());
            let result = tokio::task::spawn_blocking(move || {
                crate::calendar::call(&c, &a, &n, &args, None, &crate::caldav::Remote::default())
            })
            .await??;
            if text_of(&result).starts_with(crate::calendar::ASKED) && !failed(&result) {
                // the creator's phone woken to do it now
                let (push, to) = (state.push.clone(), agent.name.clone());
                tokio::spawn(async move {
                    push.notify_silent(&to, "calendar", Dict::new()).await;
                });
            }
            Ok(result)
        }
    }
}

pub const LIST_HEAD: &str = "The calendars family members share with you, each under the person's first name (event ids \
are per person: give who with calendar_update or calendar_delete). Everyone in the family reads your replies: mention \
only what helps.";
pub const NOBODY: &str = "Nobody in the family shares their calendar with you yet: each member who wants to turns you on \
in Réglages › Calendrier of their BipAgents app. Say so plainly if asked.";
pub const NOBODY_WRITES: &str = "Nobody in the family lets you change their calendar: they can allow it in Réglages › \
Calendrier of their BipAgents app (the small arrow beside your name). Tell them what to add instead.";
const CANNOT_READ: &str = "(shared with you to add events only: you can't read it)";

fn no_answer(name: &str) -> String {
    format!("({name}'s calendar did not answer just now: their place is unreachable, try again later.)")
}

/// The calendar tools as a family agent gets them: the family's calendars, `who` on each.
pub fn tools() -> Vec<Value> {
    let who = json!({"type": "string", "description": "whose calendar: the person's first name (default: whoever \
writes to you now)"});
    crate::calendar::tools()
        .into_iter()
        .map(|mut tool| {
            let name = str_of(&tool, "name");
            let description = match name.as_str() {
                "calendar_list" => "The events of the calendars family members share with you (each under the \
person's first name), as their phones last shared them: by default the next 7 days. from/to: dates (YYYY-MM-DD). \
Asked now: a member whose place doesn't answer is named.",
                "calendar_add" => "Add an event to the calendar of a family member who lets you (who: their first \
name; default: whoever writes to you now); their phone does it at its next sync. start: an ISO date-time in local time \
(2026-10-30T10:00), or a date for an all-day event; end, or duration_minutes (default 60).",
                "calendar_update" => "Change one of a family member's events (who: whose; its id from calendar_list \
under their name): only the fields given. Not for a repeating event.",
                _ => "Delete one of a family member's events (who: whose; its id from calendar_list under their \
name). Only when that person asked for it. Not for a repeating event.",
            };
            tool["description"] = json!(description);
            if let Some(props) = tool.pointer_mut("/inputSchema/properties").and_then(Value::as_object_mut) {
                props.insert("who".into(), who.clone());
                if name == "calendar_list" {
                    props.shift_remove("calendar");
                }
            }
            tool
        })
        .collect()
}

/// Mounted at the root (the household key).
pub fn household_router() -> Router<AppState> {
    Router::new().route("/family/calendar/{agent}", post(served)).route("/family/fa/calendar", post(told))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_by_first_name_any_case_or_accent() {
        assert!(names("marie", "Marie"));
        assert!(names("Élodie", "elodie Martin"));
        assert!(names("Elodie Martin", "Élodie Martin"));
        assert!(!names("", "Marie"));
        assert!(!names("Mar", "Marie"));
    }

    #[test]
    fn the_family_tools_take_who_and_say_whose() {
        let tools = tools();
        assert_eq!(tools.len(), 4);
        for tool in &tools {
            assert!(tool.pointer("/inputSchema/properties/who").is_some(), "{tool}");
            assert!(str_of(tool, "description").contains("family member"), "{tool}");
        }
        assert!(tools[0].pointer("/inputSchema/properties/calendar").is_none());
    }
}
