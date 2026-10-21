//! The SSE parser the run hub reads Hermes with (Python: tests/test_sse.py).

use bipbridge::hermes::{SSEEvent, SSEParser};
use serde_json::json;

fn parse(text: &str) -> Vec<SSEEvent> {
    let mut parser = SSEParser::new();
    let mut events = Vec::new();
    for line in text.split('\n') {
        if let Some(ev) = parser.feed_line(line) {
            events.push(ev);
        }
    }
    if let Some(last) = parser.finish() {
        events.push(last);
    }
    events
}

#[test]
fn type_from_json_then_event_line() {
    let events = parse(
        ": keepalive\n\n\
         event: message.delta\ndata: {\"event\": \"message.delta\", \"run_id\": \"r1\", \"delta\": \"Bon\"}\n\n\
         data: {\"type\": \"assistant.delta\", \"delta\": \"jour\"}\n\n\
         event: run.completed\ndata: {\"run_id\": \"r1\"}\n\n",
    );
    let types: Vec<&str> = events.iter().map(|e| e.r#type.as_str()).collect();
    assert_eq!(types, ["message.delta", "assistant.delta", "run.completed"]);
    assert_eq!([events[0].text(), events[1].text()], ["Bon", "jour"]);
    assert_eq!(events[2].data["run_id"], "r1");
}

#[test]
fn keepalive_only_yields_nothing() {
    assert!(parse(": keepalive\n\n: keepalive\n\n").is_empty());
}

#[test]
fn multiline_data_and_done() {
    let events = parse("data: {\"type\": \"x\",\ndata:  \"delta\": \"a\"}\n\ndata: [DONE]\n\n");
    assert_eq!(events[0].r#type, "x");
    assert_eq!(events[0].text(), "a");
    assert_eq!(events[1].r#type, "done");
}

#[test]
fn bare_json_line_and_plain_text_data() {
    let events = parse("{\"type\": \"message.delta\", \"delta\": \"hi\"}\nevent: note\ndata: plain words\n\n");
    assert_eq!(events[0].r#type, "message.delta");
    assert_eq!(events[0].text(), "hi");
    assert_eq!(events[1].r#type, "note");
    assert_eq!(serde_json::Value::Object(events[1].data.clone()), json!({"text": "plain words"}));
}

#[test]
fn nested_delta_and_final_text() {
    let events = parse(
        "data: {\"type\": \"message.delta\", \"delta\": {\"content\": \"x\"}}\n\n\
         data: {\"type\": \"run.completed\", \"output\": \"Réponse finale.\"}\n\n",
    );
    assert_eq!(events[0].text(), "x");
    assert_eq!(events[1].final_text(), "Réponse finale.");
}
