//! The core's pure functions against the Python's own results (tests/golden/core.json, made by gen_core.py).

use bipbridge::hermes::{LineDecoder, SSEEvent, SSEParser};
use bipbridge::py;
use bipbridge::store::{iso, parse_since};
use chrono::TimeZone;
use serde_json::{json, Value};

fn golden() -> Value {
    serde_json::from_str(include_str!("golden/core.json")).unwrap()
}

fn event(e: &SSEEvent) -> Value {
    json!({"type": e.r#type, "data": e.data, "raw": e.raw, "text": e.text(), "final_text": e.final_text()})
}

#[test]
fn sse_parsing() {
    for case in golden()["sse"].as_array().unwrap() {
        let mut parser = SSEParser::new();
        let mut events = Vec::new();
        for line in case["input"].as_str().unwrap().split('\n') {
            if let Some(e) = parser.feed_line(line) {
                events.push(event(&e));
            }
        }
        if let Some(e) = parser.finish() {
            events.push(event(&e));
        }
        assert_eq!(Value::Array(events), case["output"], "input {:?}", case["input"]);
    }
}

#[test]
fn line_splitting_like_httpx() {
    for case in golden()["lines"].as_array().unwrap() {
        let mut d = LineDecoder::default();
        let mut lines = Vec::new();
        for chunk in case["input"].as_array().unwrap() {
            let chunk = chunk.as_str().unwrap();
            if !chunk.is_empty() {
                lines.extend(d.decode(chunk));
            }
        }
        lines.extend(d.flush());
        assert_eq!(json!(lines), case["output"], "input {:?}", case["input"]);
    }
}

#[test]
fn timestamps() {
    for case in golden()["iso"].as_array().unwrap() {
        let p: Vec<i64> = case["input"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        let offset = chrono::FixedOffset::east_opt((p[7] * 60) as i32).unwrap();
        let dt = offset
            .with_ymd_and_hms(p[0] as i32, p[1] as u32, p[2] as u32, p[3] as u32, p[4] as u32, p[5] as u32)
            .unwrap()
            + chrono::Duration::microseconds(p[6]);
        assert_eq!(iso(dt.with_timezone(&chrono::Utc)), case["output"].as_str().unwrap());
    }
    for case in golden()["parse_since"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        match (parse_since(input), case.get("output")) {
            (Ok(dt), Some(expected)) => assert_eq!(iso(dt), expected.as_str().unwrap(), "{input:?}"),
            (Err(_), None) => {}
            (got, expected) => panic!("{input:?}: got {got:?}, expected {expected:?}"),
        }
    }
}

#[test]
fn logs_and_auth() {
    let g = golden();
    for case in g["redact"].as_array().unwrap() {
        assert_eq!(bipbridge::logs::redact(case["input"].as_str(), 6), case["output"].as_str().unwrap());
    }
    for case in g["bearer"].as_array().unwrap() {
        assert_eq!(
            json!(bipbridge::auth::extract_bearer(case["input"].as_str())),
            case["output"],
            "{:?}",
            case["input"]
        );
    }
    for case in g["forwarded"].as_array().unwrap() {
        let (host, port) = bipbridge::auth::trusted_client_address(case["input"].as_str().unwrap());
        assert_eq!(json!([host, port]), case["output"], "{:?}", case["input"]);
    }
    let mut fields = serde_json::Map::new();
    for (k, v) in [
        ("address", json!("203.0.113.9")),
        ("what", json!("bridge key")),
        ("count", json!(3)),
        ("agents", json!(["a", "b"])),
        ("ok", json!(true)),
        ("ratio", json!(0.5)),
    ] {
        fields.insert(k.into(), v);
    }
    let line = bipbridge::logs::format_line(&tracing::Level::WARN, "bipbridge::guard", "wrong key", &fields, None);
    let mut payload: serde_json::Map<String, Value> = serde_json::from_str(&line).unwrap();
    let keys: Vec<String> = payload.keys().cloned().collect();
    assert_eq!(json!(keys), g["log_line"]["keys"]);
    payload.shift_remove("ts");
    assert_eq!(
        py::Dumps::new().unicode().dumps(&Value::Object(payload)),
        g["log_line"]["line_without_ts"].as_str().unwrap()
    );
}

#[test]
fn python_json_and_numbers() {
    let g = golden();
    for case in g["dumps"].as_array().unwrap() {
        let v = &case["input"];
        assert_eq!(py::dumps(v), case["default"].as_str().unwrap(), "{v}");
        assert_eq!(py::Dumps::new().indent(1).dumps(v), case["indent1"].as_str().unwrap(), "{v}");
        assert_eq!(py::Dumps::new().compact().unicode().dumps(v), case["compact_unicode"].as_str().unwrap(), "{v}");
        assert_eq!(py::Dumps::new().sort_keys().dumps(v), case["sorted"].as_str().unwrap(), "{v}");
    }
    for case in g["float_repr"].as_array().unwrap() {
        assert_eq!(py::float_repr(case["input"].as_f64().unwrap()), case["output"].as_str().unwrap());
    }
    for case in g["int"].as_array().unwrap() {
        let got = py::py_int(&case["input"]);
        match case.get("output") {
            Some(expected) => assert_eq!(got.as_ref().ok(), expected.as_i64().as_ref(), "{:?}", case["input"]),
            None => assert!(got.is_err(), "{:?} gave {got:?}", case["input"]),
        }
    }
    for case in g["str"].as_array().unwrap() {
        assert_eq!(py::py_str(&case["input"]), case["output"].as_str().unwrap(), "{:?}", case["input"]);
    }
}
