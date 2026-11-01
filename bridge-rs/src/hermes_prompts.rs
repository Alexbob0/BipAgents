//! What a run waits for from its person, as Hermes 0.22.5's « session authority » asks it (2026-11-01, the move from
//! the pin 517b5e10f; upstream api-server.md and gateway/session_pending_controls.py).
//!
//! An approval or a question is a prompt: `approval.request` / `clarify.request` carry its `prompt_id` and the
//! `execution_generation` of the turn that asked, and `approval.settled` / `clarify.settled` end it (answered, timed out
//! or gone with its turn). A question is one per prompt (the clarify tool's batch comes one prompt after the other),
//! with `question`, `choices` and `multi_select` at the top. The answer names the prompt exactly: `POST
//! /v1/runs/{id}/approval` with `{request_id, execution_generation, choice}` and `POST /v1/runs/{id}/clarify` with
//! `{request_id, execution_generation, answer}`, nothing else (Hermes refuses any other shape); a question has no
//! dismissal (an empty answer skips it). `GET /v1/runs/{id}` lists what is waiting in `pending_controls`.
//!
//! The bridge and the apps read a prompt's id as `request_id` (the approvals' log, the pushes, the Boîte): `named`
//! adds it to the events.

use serde_json::{json, Map, Value};

use crate::hermes::SSEEvent;

/// An approval answered or gone.
pub const APPROVAL_SETTLED: &str = "approval.settled";
/// A question answered or gone.
pub const CLARIFY_SETTLED: &str = "clarify.settled";

/// A prompt's `execution_generation` (an integer; Hermes refuses any other).
pub fn generation(data: &Map<String, Value>) -> Option<i64> {
    data.get("execution_generation").and_then(Value::as_i64)
}

/// A prompt's event (`*.request`, `*.settled`) with its `prompt_id` as `request_id` too, as the bridge and the apps
/// read it. Other events as they are.
pub fn named(event: SSEEvent) -> SSEEvent {
    let Some(prompt) = event.data.get("prompt_id").and_then(Value::as_str).map(str::to_string) else {
        return event;
    };
    if event.data.contains_key("request_id") {
        return event;
    }
    let mut data = event.data;
    data.insert("request_id".into(), json!(prompt));
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(event.r#type, data, raw)
}

/// The prompts a run's status (`GET /v1/runs/{id}`) shows waiting, as the events that asked them (`run_id` added): a
/// run followed again after a restart sees them so (its event stream does not replay them).
pub fn pending_requests(run_id: &str, run: &Map<String, Value>) -> Vec<SSEEvent> {
    let Some(Value::Array(controls)) = run.get("pending_controls") else { return Vec::new() };
    controls
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|control| {
            let kind = match control.get("kind").and_then(Value::as_str) {
                Some("approval") => "approval.request",
                Some("clarify") => "clarify.request",
                _ => return None, // a secret or sudo prompt stays Hermes' own
            };
            let mut data = control.clone();
            data.insert("run_id".into(), json!(run_id));
            let raw = crate::py::Dumps::new().sort_keys().dumps(&Value::Object(data.clone()));
            Some(named(SSEEvent::new(kind, data, raw)))
        })
        .collect()
}

/// The `(prompt_id, execution_generation)` of the `kind` prompt (`approval` or `clarify`) a run's status shows waiting:
/// the one `request_id` names, else the only one of that kind (several: which one is not the bridge's to guess).
pub fn pending_identity(run: &Map<String, Value>, kind: &str, request_id: Option<&str>) -> Option<(String, i64)> {
    let Some(Value::Array(controls)) = run.get("pending_controls") else { return None };
    let waiting: Vec<(String, i64)> = controls
        .iter()
        .filter_map(Value::as_object)
        .filter(|c| c.get("kind").and_then(Value::as_str) == Some(kind))
        .filter_map(|c| Some((c.get("prompt_id")?.as_str()?.to_string(), generation(c)?)))
        .collect();
    match request_id.filter(|r| !r.is_empty()) {
        Some(wanted) => waiting.into_iter().find(|(prompt, _)| prompt == wanted),
        None if waiting.len() == 1 => waiting.into_iter().next(),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn prompt_events_get_their_request_id() {
        let ask = SSEEvent::new(
            "approval.request",
            data(json!({"event": "approval.request", "run_id": "run_1", "kind": "approval", "prompt_id": "p1",
                        "execution_generation": 3, "command": "rm -rf build/", "choices": ["once", "deny"]})),
            "",
        );
        let seen = named(ask);
        assert_eq!((&seen.data["request_id"], &seen.data["execution_generation"]), (&json!("p1"), &json!(3)));
        assert!(seen.raw.contains("request_id"));
        let settled = named(SSEEvent::new(CLARIFY_SETTLED, data(json!({"prompt_id": "c1"})), ""));
        assert_eq!(settled.data["request_id"], "c1");
        let delta = SSEEvent::new("message.delta", data(json!({"delta": "Bonjour"})), "{}");
        assert_eq!(named(delta.clone()), delta);
    }

    #[test]
    fn pending_prompts_of_a_runs_status() {
        let run = data(json!({"status": "waiting_for_approval", "pending_controls": [
            {"kind": "approval", "prompt_id": "p1", "execution_generation": 2, "command": "ls", "choices": ["once", "deny"]},
            {"kind": "clarify", "prompt_id": "c1", "execution_generation": 2, "question": "Quel train ?", "choices": []},
            {"kind": "secret", "prompt_id": "s1", "execution_generation": 2},
        ]}));
        let events = pending_requests("run_2", &run);
        let kinds: Vec<&str> = events.iter().map(|e| e.r#type.as_str()).collect();
        assert_eq!(kinds, ["approval.request", "clarify.request"]);
        assert_eq!((&events[1].data["request_id"], &events[1].data["run_id"]), (&json!("c1"), &json!("run_2")));
        assert!(pending_requests("run_2", &data(json!({"status": "running"}))).is_empty());
        assert_eq!(pending_identity(&run, "approval", None), Some(("p1".to_string(), 2)));
        assert_eq!(pending_identity(&run, "approval", Some("p2")), None);
        assert_eq!(pending_identity(&run, "clarify", Some("c1")), Some(("c1".to_string(), 2)));
        let two = data(json!({"pending_controls": [
            {"kind": "approval", "prompt_id": "p1", "execution_generation": 4},
            {"kind": "approval", "prompt_id": "p2", "execution_generation": 4},
        ]}));
        assert_eq!(pending_identity(&two, "approval", None), None);
        assert_eq!(pending_identity(&two, "approval", Some("p2")), Some(("p2".to_string(), 4)));
    }
}
