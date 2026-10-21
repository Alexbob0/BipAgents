//! The push-runs group's pure functions against the Python's own results (tests/golden/push_runs.json, made by
//! gen_push_runs.py): push payloads and previews, FCM data messages, ntfy's cron unwrapping, MEDIA lines, the cron
//! watcher's helpers and the run hub's backlog merging. Outputs are compared as JSON text: key order counts.

use std::collections::VecDeque;

use bipbridge::hermes::SSEEvent;
use bipbridge::{cronwatch, fcm, media, ntfy, push, runs, Dict};
use serde_json::{json, Value};

fn opt(v: &Value) -> Option<&str> {
    v.as_str()
}

fn dict(v: &Value) -> Dict {
    v.as_object().cloned().unwrap_or_default()
}

fn dicts(v: &Value) -> Vec<Dict> {
    v.as_array().unwrap().iter().map(dict).collect()
}

fn run(name: &str, a: &[Value]) -> Value {
    match name {
        "message_payload" => Value::Object(push::message_payload(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_str().unwrap(),
            opt(&a[3]),
            opt(&a[4]),
        )),
        "approval_payload" => {
            let choices = a[4].as_array().map(|c| c.as_slice());
            Value::Object(push::approval_payload(
                a[0].as_str().unwrap(),
                a[1].as_str().unwrap(),
                a[2].as_str().unwrap(),
                opt(&a[3]),
                choices,
                opt(&a[5]),
                opt(&a[6]),
            ))
        }
        "reply_payload" => Value::Object(push::reply_payload(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_str().unwrap(),
            opt(&a[3]),
            opt(&a[4]),
            opt(&a[5]),
            a[6].as_str().unwrap(),
        )),
        "question_payload" => Value::Object(push::question_payload(
            a[0].as_str().unwrap(),
            a[1].as_str().unwrap(),
            a[2].as_str().unwrap(),
            opt(&a[3]),
            opt(&a[4]),
            opt(&a[5]),
            opt(&a[6]),
        )),
        "silent_payload" => {
            Value::Object(push::silent_payload(a[0].as_str().unwrap(), a[1].as_str().unwrap(), &dict(&a[2])))
        }
        "preview" => json!(push::preview(a[0].as_str().unwrap(), a[1].as_u64().unwrap() as usize)),
        "data_message" => Value::Object(fcm::data_message(&a[0])),
        "unwrap_cron" => {
            let (name, body) = ntfy::unwrap_cron(a[0].as_str().unwrap());
            json!([name, body])
        }
        "auth_header" => match ntfy::auth_header(opt(&a[0])) {
            Some((k, v)) => json!({ k: v }),
            None => json!({}),
        },
        "extract_media" => {
            let (text, paths) = media::extract_media(a[0].as_str().unwrap());
            json!([text, paths])
        }
        "is_cron_session" => json!(cronwatch::is_cron_session(&dict(&a[0]))),
        "session_started" => match cronwatch::session_started(&dict(&a[0])) {
            Some(dt) => {
                let text = dt.format("%Y-%m-%dT%H:%M:%S").to_string();
                let micros = dt.and_utc().timestamp_subsec_micros();
                if micros > 0 {
                    json!(format!("{text}.{micros:06}"))
                } else {
                    json!(text)
                }
            }
            None => Value::Null,
        },
        "job_id" => json!(cronwatch::job_id(a[0].as_str().unwrap())),
        "job_name" => json!(cronwatch::job_name(opt(&a[0]))),
        "final_reply" => json!(cronwatch::final_reply(&dicts(&a[0]))),
        "subtask_report" => json!(cronwatch::subtask_report(&dicts(&a[0]))),
        "answers_another_agent" => json!(cronwatch::answers_another_agent(&dicts(&a[0]))),
        "message_text" => json!(cronwatch::message_text(&dict(&a[0]))),
        "report_line" => json!(cronwatch::report_line(a[0].as_str().unwrap())),
        "is_bot_chat" => json!(cronwatch::is_bot_chat(&dict(&a[0]))),
        "append_to_backlog" => {
            let mut backlog = VecDeque::new();
            for e in a[0].as_array().unwrap() {
                let event = SSEEvent::new(e[0].as_str().unwrap(), dict(&e[1]), e[2].as_str().unwrap());
                runs::append_to_backlog(&mut backlog, &event);
            }
            Value::Array(backlog.iter().map(|e| json!([e.r#type, e.data, e.raw])).collect())
        }
        "clarify_id" => json!(runs::clarify_id(&dict(&a[0]))),
        other => panic!("unknown function {other}"),
    }
}

#[test]
fn push_runs_helpers_match_python() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("golden/push_runs.json")).unwrap();
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let name = case["fn"].as_str().unwrap();
        let got = run(name, case["input"].as_array().unwrap());
        let (got, expected) = (serde_json::to_string(&got).unwrap(), serde_json::to_string(&case["output"]).unwrap());
        if got != expected {
            failures.push(format!("case {i} {name}({}):\n  got      {got}\n  expected {expected}", case["input"]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
