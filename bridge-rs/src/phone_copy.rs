//! The phone's copies here (its calendar, address book and reminders: calendar.rs, contacts.rs, reminders.rs): when
//! the phone last sent its copy whole (`at`) and when it last checked that copy still matches (`checked`).
//!
//! 16/11 (battery, Alex's choice D3): the apps sent every copy whole each time they came to the front. Now they send it
//! only when it changed (its fingerprint, `hash`, a SHA-256 of the copy, the phone's), at most every quarter of an hour
//! unless an agent's request or a setting asks; unchanged, they only say so:
//!
//!     POST /v1/<calendar|contacts|reminders>/verified  {hash}   → {checked}: the copy here is the phone's still;
//!                                                                 409: not the one here (or none): send it whole
//!
//! The agents read « as the phone last checked it … ago », never « up to date » on the strength of an old copy: the
//! moment a copy was checked is not the moment it changed.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::web::{self, FieldError, Validate};
use crate::{py, Dict};

#[derive(Debug, Deserialize)]
pub struct Verified {
    pub hash: String,
}

impl Validate for Verified {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "hash", Some(&self.hash), 128);
    }
}

/// A copy sent whole: sent and checked now, its fingerprint kept (none from an app before 16/11: no « verified » then).
pub fn sent(data: &mut Dict, hash: Option<&str>) {
    let now = py::time_time();
    data.insert("at".into(), json!(now));
    data.insert("checked".into(), json!(now));
    match hash.filter(|h| !h.is_empty()) {
        Some(h) => {
            data.insert("hash".into(), json!(h));
        }
        None => {
            data.remove("hash");
        }
    }
}

/// The phone says its copy is still `hash`: true (checked now) when it is the one here, false when there is none or
/// another (the phone then sends it whole).
pub fn verify(data: &mut Dict, hash: &str) -> bool {
    let held = data.get("at").is_some() && !hash.is_empty() && data.get("hash").and_then(Value::as_str) == Some(hash);
    if held {
        data.insert("checked".into(), json!(py::time_time()));
    }
    held
}

/// The copy dropped (no agent may read it any more): its fingerprint and check go with it.
pub fn forget(data: &mut Dict) {
    data.remove("hash");
    data.remove("checked");
}

/// When the phone last vouched for its copy: checked, else sent (a copy from before 16/11).
pub fn seen(data: &Dict) -> Option<f64> {
    let at = data.get("at").and_then(Value::as_f64);
    let checked = data.get("checked").and_then(Value::as_f64);
    match (at, checked) {
        (Some(a), Some(c)) => Some(a.max(c)),
        (a, c) => a.or(c),
    }
}

/// How old the copy is, for the agents: « as the phone shared it 3 min ago », or once checked since its last change
/// « as the phone last checked it 3 min ago (unchanged for 2 h) ». `age` words a number of seconds.
pub fn freshness(data: &Dict, now: f64, age: impl Fn(f64) -> String) -> Option<String> {
    let at = data.get("at").and_then(Value::as_f64)?;
    let seen = seen(data).unwrap_or(at);
    Some(if seen - at >= 60.0 {
        format!("as the phone last checked it {} ago (unchanged for {})", age(now - seen), age(now - at))
    } else {
        format!("as the phone shared it {} ago", age(now - at))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn a_copy_is_checked_only_against_its_own_fingerprint() {
        let mut d = data(json!({}));
        assert!(!verify(&mut d, "h1"), "no copy here: sent whole");
        sent(&mut d, Some("h1"));
        assert!(verify(&mut d, "h1"));
        assert!(!verify(&mut d, "h2"), "another copy: sent whole");
        sent(&mut d, None);
        assert!(!verify(&mut d, "h1"), "a copy from an app before 16/11 has no fingerprint");
    }

    #[test]
    fn the_agents_read_when_it_was_checked_and_since_when_it_is_unchanged() {
        let age = |s: f64| format!("{} min", (s / 60.0).floor() as i64);
        let d = data(json!({"at": 1000.0, "checked": 1000.0}));
        assert_eq!(freshness(&d, 1300.0, age).unwrap(), "as the phone shared it 5 min ago");
        let d = data(json!({"at": 1000.0, "checked": 4600.0}));
        assert_eq!(freshness(&d, 4900.0, age).unwrap(), "as the phone last checked it 5 min ago (unchanged for 65 min)");
        assert_eq!(freshness(&data(json!({})), 4900.0, age), None);
        assert_eq!(seen(&d), Some(4600.0));
    }
}
