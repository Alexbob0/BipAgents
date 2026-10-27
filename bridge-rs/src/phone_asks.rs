//! What the agents asked the person's phone to add (calendar.rs, reminders.rs, contacts.rs) and the phone has not shown
//! back yet. The phone does the requests at its next sync and sends a new copy later: until then, the copy an agent
//! lists doesn't have what it just added. A model that checks its work by listing finds nothing and adds again — on
//! 2026-10-27 (token lab, GLM-5.3 Flash) one turn added the same event 255 times. So the lists show the agent's own
//! adds still unseen, marked as such, and an add identical to one asked a moment ago is not asked again.
//!
//! No Python counterpart (owner group: tools-knowledge).

use serde_json::Value;

use crate::tools_knowledge as tk;

/// An add identical to one asked this recently (or still waiting, however old) is not asked again.
pub const TWIN_SECONDS: f64 = 30.0 * 60.0;

fn text(item: &Value, key: &str) -> String {
    item.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// Lower case, without accents nor surrounding spaces: « Dentiste » and « dentiste » are the same title.
pub fn folded(value: &str) -> String {
    tk::ascii_fold(&value.trim().to_lowercase())
}

/// Still waiting for the phone.
pub fn waiting(op: &Value) -> bool {
    text(op, "state") == "pending"
}

/// `agent`'s « add » requests the phone's copy doesn't show yet: waiting, or done after the copy was sent (`at`).
pub fn unseen_adds<'a>(queue: &'a [Value], agent: &str, at: Option<f64>) -> Vec<&'a Value> {
    queue
        .iter()
        .filter(|op| text(op, "op") == "add" && text(op, "agent") == agent)
        .filter(|op| {
            waiting(op)
                || (text(op, "state") == "done"
                    && match (op.get("finished").and_then(Value::as_f64), at) {
                        (Some(finished), Some(at)) => finished > at,
                        (_, None) => true,
                        (None, Some(_)) => false,
                    })
        })
        .collect()
}

/// `agent`'s « add » request whose args `same` matches, still waiting or asked in the last `TWIN_SECONDS` (not one
/// that failed or was cancelled).
pub fn twin<'a>(queue: &'a [Value], agent: &str, now: f64, same: impl Fn(&Value) -> bool) -> Option<&'a Value> {
    queue.iter().rev().find(|op| {
        text(op, "op") == "add"
            && text(op, "agent") == agent
            && (waiting(op)
                || (text(op, "state") == "done"
                    && op.get("created").and_then(Value::as_f64).is_some_and(|c| now - c <= TWIN_SECONDS)))
            && op.get("args").is_some_and(&same)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn op(id: &str, agent: &str, state: &str, created: f64, finished: Option<f64>, title: &str) -> Value {
        json!({"id": id, "op": "add", "agent": agent, "state": state, "created": created, "finished": finished,
               "args": {"title": title}})
    }

    #[test]
    fn unseen_until_the_phone_shares_again() {
        let queue = vec![
            op("a", "coach", "pending", 100.0, None, "Banque"),
            op("b", "coach", "done", 100.0, Some(150.0), "Dentiste"),
            op("c", "coach", "done", 100.0, Some(110.0), "Salon"),
            op("d", "coach", "failed", 100.0, Some(150.0), "Raté"),
            op("e", "other", "pending", 100.0, None, "Pas à moi"),
            json!({"id": "f", "op": "delete", "agent": "coach", "state": "pending", "args": {"id": "x"}}),
        ];
        let ids = |at| unseen_adds(&queue, "coach", at).iter().map(|o| text(o, "id")).collect::<Vec<_>>();
        assert_eq!(ids(Some(120.0)), ["a", "b"]);
        assert_eq!(ids(None), ["a", "b", "c"]);
    }

    #[test]
    fn the_same_add_is_found_while_waiting_or_recent() {
        let queue = vec![
            op("old", "coach", "done", 0.0, Some(10.0), "Test"),
            op("new", "coach", "pending", 0.0, None, "Test"),
            op("x", "coach", "cancelled", 9_000.0, Some(9_001.0), "Autre"),
        ];
        let same = |title: &'static str| move |args: &Value| folded(&text(args, "title")) == folded(title);
        assert_eq!(twin(&queue, "coach", 9_000.0, same(" test ")).map(|o| text(o, "id")), Some("new".into()));
        assert!(twin(&queue, "other", 9_000.0, same("Test")).is_none());
        assert!(twin(&queue, "coach", 9_000.0, same("Autre")).is_none(), "a cancelled one is no twin");
        let done_only = vec![op("old", "coach", "done", 0.0, Some(10.0), "Test")];
        assert!(twin(&done_only, "coach", 60.0, same("Test")).is_some());
        assert!(twin(&done_only, "coach", TWIN_SECONDS + 1.0, same("Test")).is_none());
    }
}
