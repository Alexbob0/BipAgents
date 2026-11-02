//! What each agent may do with what the phone shares (Alex, 2026-11-01: « l'accès à l'appareil » in detail, under a
//! small chevron on each row of an agent's tools). The person chooses per agent, per source:
//!
//!     reminders.rs   read (reminders_list) · write (reminders_add, reminders_complete)
//!     calendar.rs    read (calendar_list and meet.rs's free slots) · write (calendar_add, _update, _delete), for the
//!                    phone's calendar (a mailbox's calendar follows its mailbox, caldav.rs)
//!     contacts.rs    phones · emails · birthdays · addresses (shown, and searched, only when given) · write
//!                    (contacts_add, contacts_update)
//!
//! Each share's file keeps them under `rights` ({agent: {right: bool}}). A right nobody chose is given: a file from
//! before the rights holds none, and its agents keep doing what they did then (nothing to migrate). A right taken
//! back is enforced by the bridge at each tool call (the tool says so to the agent), and a right to change taken back
//! cancels that agent's requests still waiting for the phone. Health keeps its own categories (health.rs).
//!
//! App routes (the bridge key), one per share:
//!
//!     PUT /v1/{reminders,calendar,contacts}/rights   {agent, rights: {right: bool}}  → {agent, rights}
//!
//! and its status (`GET /v1/reminders`…) gives `rights` for the agents allowed and those with a choice kept.
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::web::{FieldError, Validate};
use crate::{py, Dict};

pub const REMINDERS: [&str; 2] = [READ, WRITE];
pub const CALENDAR: [&str; 2] = [READ, WRITE];
pub const CONTACTS: [&str; 5] = [PHONES, EMAILS, BIRTHDAYS, ADDRESSES, WRITE];
/// The contacts' details an agent may be shown: what the phone sends only when some agent here reads it.
pub const CONTACT_FIELDS: [&str; 4] = [PHONES, EMAILS, BIRTHDAYS, ADDRESSES];

pub const READ: &str = "read";
pub const WRITE: &str = "write";
pub const PHONES: &str = "phones";
pub const EMAILS: &str = "emails";
pub const BIRTHDAYS: &str = "birthdays";
pub const ADDRESSES: &str = "addresses";

/// `agent`'s rights among `keys` in a share's file: each one given unless the person took it back.
pub fn of(data: &Dict, agent: &str, keys: &[&'static str]) -> BTreeMap<&'static str, bool> {
    let kept = data.get("rights").and_then(|r| r.get(agent));
    keys.iter().map(|k| (*k, kept.and_then(|r| r.get(*k)).and_then(Value::as_bool).unwrap_or(true))).collect()
}

/// Whether `agent` has `key`.
pub fn allows(data: &Dict, agent: &str, key: &'static str) -> bool {
    of(data, agent, &[key])[key]
}

fn allowed_agents(data: &Dict) -> Vec<String> {
    match data.get("agents") {
        Some(Value::Array(names)) => names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect(),
        _ => Vec::new(),
    }
}

/// The rights of the agents allowed and of those whose choice is kept (an agent turned off keeps it for next time),
/// as the status gives them.
pub fn listed(data: &Dict, keys: &[&'static str]) -> Value {
    let mut names = allowed_agents(data);
    if let Some(Value::Object(kept)) = data.get("rights") {
        names.extend(kept.keys().filter(|k| !names.contains(k)).cloned().collect::<Vec<_>>());
    }
    Value::Object(names.iter().map(|n| (n.clone(), json!(of(data, n, keys)))).collect())
}

/// What some agent allowed has: what the phone needs to send (the contacts' details nobody reads stay on it).
pub fn shared(data: &Dict, keys: &[&'static str]) -> BTreeMap<&'static str, bool> {
    let agents = allowed_agents(data);
    keys.iter().map(|k| (*k, agents.iter().any(|a| of(data, a, &[*k])[*k]))).collect()
}

/// `changes` (some of `keys`; the others ignored) applied to `agent`'s rights in `data`. Returns its rights now, and
/// whether its right to change was taken back now (then its requests waiting are cancelled, `cancel_requests`).
pub fn set(
    data: &mut Dict,
    agent: &str,
    keys: &[&'static str],
    changes: &BTreeMap<String, bool>,
) -> (BTreeMap<&'static str, bool>, bool) {
    let before = of(data, agent, keys);
    let mut now = before.clone();
    for (key, on) in changes {
        if let Some(k) = keys.iter().find(|k| **k == key.as_str()) {
            now.insert(k, *on);
        }
    }
    let mut all = match data.remove("rights") {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    };
    all.insert(agent.to_string(), json!(now));
    data.insert("rights".into(), Value::Object(all));
    let stopped = before.get(WRITE) == Some(&true) && now.get(WRITE) == Some(&false);
    (now, stopped)
}

/// `agent`'s requests still waiting for the phone, cancelled (its right to change taken back): how many.
pub fn cancel_requests(data: &mut Dict, agent: &str) -> usize {
    let Some(Value::Array(queue)) = data.get_mut("queue") else { return 0 };
    let mut cancelled = 0;
    for op in queue.iter_mut() {
        let mine = op.get("agent").and_then(Value::as_str) == Some(agent);
        if mine && op.get("state").and_then(Value::as_str) == Some("pending") {
            if let Some(map) = op.as_object_mut() {
                map.insert("state".into(), json!("cancelled"));
                map.insert("error".into(), json!("not allowed any more"));
                map.insert("finished".into(), json!(py::time_time()));
            }
            cancelled += 1;
        }
    }
    cancelled
}

/// `PUT /v1/…/rights`' body.
#[derive(Debug, Deserialize)]
pub struct Rights {
    pub agent: String,
    #[serde(default)]
    pub rights: BTreeMap<String, bool>,
}

impl Validate for Rights {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        crate::web::check_max_len(errors, "agent", Some(&self.agent), 200);
        if self.rights.len() > 10 {
            errors.push(
                FieldError::new(
                    "too_long",
                    vec![json!("body"), json!("rights")],
                    "Dictionary should have at most 10 items".to_string(),
                    json!(self.rights.len()),
                )
                .with_ctx(json!({"max_length": 10})),
            );
        }
    }
}

/// What a tool answers when the person took the right back: `what`, in the agent's words (« read their reminders »).
pub fn refused(what: &str) -> String {
    format!(
        "The person doesn't let you {what}: tell them if it's needed, they choose it in your page of the BipAgents app \
(the small arrow beside the tool)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(value: Value) -> Dict {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_file_from_before_the_rights_gives_them_all() {
        let old = data(json!({"agents": ["coach"], "queue": []}));
        assert_eq!(of(&old, "coach", &CONTACTS).values().filter(|on| **on).count(), 5);
        assert!(allows(&old, "coach", WRITE));
        assert_eq!(listed(&old, &REMINDERS), json!({"coach": {"read": true, "write": true}}));
        assert_eq!(shared(&old, &CONTACT_FIELDS).values().filter(|on| **on).count(), 4);
        // No agent allowed: nothing to send.
        assert!(shared(&data(json!({})), &CONTACT_FIELDS).values().all(|on| !on));
    }

    #[test]
    fn a_choice_kept_and_the_requests_cancelled() {
        let mut file = data(json!({"agents": ["coach", "chef"], "queue": [
            {"id": "a", "agent": "coach", "state": "pending"},
            {"id": "b", "agent": "coach", "state": "done"},
            {"id": "c", "agent": "chef", "state": "pending"}]}));
        let changes: BTreeMap<String, bool> =
            [("write".to_string(), false), ("phones".to_string(), false), ("bogus".to_string(), false)].into();
        let (now, stopped) = set(&mut file, "coach", &CONTACTS, &changes);
        assert!(stopped);
        assert_eq!(now.get(PHONES), Some(&false));
        assert_eq!(now.get(EMAILS), Some(&true));
        assert!(!now.contains_key("bogus"));
        assert_eq!(cancel_requests(&mut file, "coach"), 1);
        assert_eq!(file["queue"][0]["state"], "cancelled");
        assert_eq!(file["queue"][1]["state"], "done");
        assert_eq!(file["queue"][2]["state"], "pending");
        // The other agent still reads the phones: the phone keeps sending them.
        assert!(shared(&file, &CONTACT_FIELDS)[PHONES]);
        file.insert("agents".into(), json!(["coach"]));
        assert!(!shared(&file, &CONTACT_FIELDS)[PHONES]);
        // Turned off, the agent keeps its choice for next time.
        file.insert("agents".into(), json!([]));
        assert_eq!(listed(&file, &CONTACTS)["coach"]["phones"], false);
        // Given back: not a « stopped ».
        let (_, stopped) = set(&mut file, "coach", &CONTACTS, &[("write".to_string(), true)].into());
        assert!(!stopped);
    }
}
