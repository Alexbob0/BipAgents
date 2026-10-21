//! The shared fakes (tests/common/fakes.rs) answer as conftest's do.

mod common;

use std::time::Duration;

use common::fakes::*;
use common::*;
use futures::StreamExt;
use serde_json::json;

#[tokio::test]
async fn backend_fakes_answer_like_conftest() {
    let backend = FakeBackend::start().await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{}/v1/audio/speech", backend.kyutai_url))
        .json(&json!({"input": "ab", "response_format": "mp3"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"ID3ab");
    let r = http
        .post(format!("{}/v1/audio/stream", backend.kyutai_url))
        .json(&json!({"input": "abc"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.bytes().await.unwrap().to_vec(), pcm_for("abc"));
    let r = http
        .post(format!("{}/v1/audio/speech", backend.pocket_url))
        .json(&json!({"input": "ab", "response_format": "wav"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.bytes().await.unwrap().to_vec(), wav(&pcm_for("ba")));
    assert_eq!(backend.with(|s| s.kyutai_inputs.len()), 2);

    backend.with(|s| {
        s.runs.insert(
            "run_1".into(),
            vec![
                RunItem::Text(sse("message.delta", json!({"delta": "Hi"}))),
                RunItem::Text(sse("run.completed", json!({}))),
            ],
        );
    });
    let agent = bipbridge::config::AgentConfig {
        name: "w".into(),
        hermes_url: backend.hermes_url.clone(),
        hermes_key: HERMES_KEY.into(),
        ..Default::default()
    };
    let hermes = bipbridge::hermes::HermesClient::new(http.clone());
    let mut events = hermes.run_events(&agent, "run_1").await.unwrap();
    assert_eq!(events.next().await.unwrap().unwrap().text(), "Hi");
    assert_eq!(events.next().await.unwrap().unwrap().r#type, "run.completed");
    backend.release.set();
    assert!(events.next().await.is_none());
    assert_eq!(hermes.run_events(&agent, "nope").await.err().unwrap().status(), Some(404));
    assert_eq!(hermes.session(&agent, "s").await.unwrap()["message_count"], 7);

    backend.with(|s| s.ntfy_lines.insert("topic".into(), vec!["{\"id\": \"m1\"}".into()]));
    let lines: Vec<String> = bipbridge::hermes::lines_of(
        http.get(format!("{}/topic/json", backend.ntfy_url)).send().await.unwrap().bytes_stream(),
    )
    .map(|l| l.unwrap())
    .collect()
    .await;
    assert_eq!(
        lines,
        vec![
            "{\"id\": \"open1\", \"event\": \"open\", \"topic\": \"topic\"}".to_string(),
            "{\"id\": \"m1\"}".to_string()
        ]
    );

    let apns = FakeApns::start().await;
    apns.responses.lock().unwrap().insert("dead".into(), (410, Some("Unregistered".into())));
    let r = http.post(format!("{}/3/device/live", apns.url)).json(&json!({"aps": {}})).send().await.unwrap();
    assert_eq!(r.headers()["apns-id"], "abc");
    let r = http.post(format!("{}/3/device/dead", apns.url)).json(&json!({})).send().await.unwrap();
    assert_eq!(r.status(), 410);
    assert_eq!(apns.payloads(), vec![json!({"aps": {}}), json!({})]);
    assert!(wait_until(|| true, Duration::from_millis(10)).await);
}
