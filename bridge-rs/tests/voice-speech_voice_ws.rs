//! Port of the voice parts of tests/test_voice_and_approvals.py: `WS /v1/voice` follow / cancel / errors / resume,
//! over a real socket.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use bipbridge::numbers_fr::prepare_for_synthesis;
use common::fakes::{pcm_for, sse, FakeBackend, Flag, RunItem};
use common::{config, BRIDGE_KEY, HERMES_KEY};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Served {
    _dir: tempfile::TempDir,
    backend: FakeBackend,
    url: String,
}

async fn serve() -> Served {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = bipbridge::app::create_app(
        config(dir.path(), &backend.urls()),
        bipbridge::app::AppOptions { start_background: false },
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app.router.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await;
        drop(app);
    });
    Served { _dir: dir, backend, url: format!("ws://{addr}/v1/voice") }
}

async fn connect(url: &str, key: Option<&str>) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    let mut request = url.into_client_request().unwrap();
    if let Some(key) = key {
        request.headers_mut().insert("authorization", format!("Bearer {key}").parse().unwrap());
    }
    connect_with(request).await
}

async fn connect_with(
    request: tokio_tungstenite::tungstenite::handshake::client::Request,
) -> Result<Ws, tokio_tungstenite::tungstenite::Error> {
    tokio_tungstenite::connect_async(request).await.map(|(ws, _)| ws)
}

enum Frame {
    Json(Value),
    Bin(Vec<u8>),
}

async fn receive(ws: &mut Ws) -> Frame {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(20), ws.next())
            .await
            .expect("frame in time")
            .expect("open")
            .expect("frame");
        match msg {
            Message::Text(t) => return Frame::Json(serde_json::from_str(t.as_str()).unwrap()),
            Message::Binary(b) => return Frame::Bin(b.to_vec()),
            _ => continue,
        }
    }
}

async fn receive_json(ws: &mut Ws) -> Value {
    match receive(ws).await {
        Frame::Json(v) => v,
        Frame::Bin(_) => panic!("binary frame"),
    }
}

async fn receive_frames(ws: &mut Ws, until_type: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    for _ in 0..50 {
        let frame = receive(ws).await;
        let stop = matches!(&frame, Frame::Json(v) if v["type"] == until_type);
        frames.push(frame);
        if stop {
            break;
        }
    }
    frames
}

async fn send(ws: &mut Ws, value: Value) {
    ws.send(Message::Text(serde_json::to_string(&value).unwrap().into())).await.unwrap();
}

fn run(backend: &FakeBackend, id: &str, items: Vec<RunItem>) {
    backend.with(|s| s.runs.insert(id.to_string(), items));
}

#[tokio::test]
async fn voice_follow_happy_path() {
    let s = serve().await;
    run(
        &s.backend,
        "run_1",
        vec![
            RunItem::Text(": keepalive\n\n".into()),
            RunItem::Text(sse("message.delta", json!({"run_id": "run_1", "delta": "Bonjour à tous. Il fait 3."}))),
            RunItem::Text(sse("message.delta", json!({"run_id": "run_1", "delta": "5 degrés"}))),
            RunItem::Text(sse("tool.started", json!({"run_id": "run_1", "tool": "weather", "preview": "{}"}))),
            RunItem::Text(sse("message.delta", json!({"run_id": "run_1", "delta": " dehors ! **Fin**"}))),
            RunItem::Text(sse("run.completed", json!({"run_id": "run_1"}))),
        ],
    );
    let mut ws = connect(&s.url, Some(BRIDGE_KEY)).await.unwrap();
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "run_1"})).await;
    let frames = receive_frames(&mut ws, "done").await;
    let Frame::Json(following) = &frames[0] else { panic!() };
    assert_eq!(
        *following,
        json!({"type": "following", "run_id": "run_1", "agent": "wellness", "format": "pcm16", "sample_rate": 24000, "channels": 1})
    );
    let sentences = ["Bonjour à tous.", "Il fait 3.5 degrés.", "dehors !", "Fin."];
    let body = &frames[1..frames.len() - 1];
    assert_eq!(body.len(), 2 * sentences.len());
    for (i, text) in sentences.iter().enumerate() {
        let Frame::Json(header) = &body[2 * i] else { panic!("header") };
        assert_eq!(header["type"], "sentence");
        assert_eq!(header["seq"], i);
        assert_eq!(header["text"], *text);
        assert_eq!(header["sample_rate"], 24000);
        assert_eq!(header["channels"], 1);
        let Frame::Bin(audio) = &body[2 * i + 1] else { panic!("audio") };
        assert_eq!(*audio, pcm_for(&prepare_for_synthesis(text)));
        assert_eq!(header["bytes"], audio.len());
    }
    let Frame::Json(done) = frames.last().unwrap() else { panic!() };
    assert_eq!(*done, json!({"type": "done", "run_id": "run_1", "reason": "completed"}));
    s.backend.with(|st| {
        assert_eq!(st.run_requests[0].header("authorization").unwrap(), format!("Bearer {HERMES_KEY}"));
        let inputs: Vec<Value> = st.kyutai_inputs.iter().map(|b| b["input"].clone()).collect();
        let expected: Vec<Value> = sentences.iter().map(|t| json!(prepare_for_synthesis(t))).collect();
        assert_eq!(inputs, expected);
        assert!(st.kyutai_inputs.iter().all(|b| b["voice"] == "4193"));
    });
    s.backend.release.set();
}

#[tokio::test]
async fn voice_cancel_stops_everything() {
    let s = serve().await;
    let gate = Flag::new();
    run(
        &s.backend,
        "run_2",
        vec![
            RunItem::Text(sse("message.delta", json!({"run_id": "run_2", "delta": "Première phrase. "}))),
            RunItem::Text(sse("message.delta", json!({"run_id": "run_2", "delta": "Deuxième"}))),
            RunItem::Wait(gate.clone()), // Hermes is still generating...
            RunItem::Text(sse("message.delta", json!({"run_id": "run_2", "delta": " phrase jamais dite."}))),
            RunItem::Text(sse("run.completed", json!({"run_id": "run_2"}))),
        ],
    );
    let mut ws = connect(&s.url, Some(BRIDGE_KEY)).await.unwrap();
    send(&mut ws, json!({"type": "follow", "agent": "Wellness", "run_id": "run_2", "voice": "5207"})).await;
    let frames = receive_frames(&mut ws, "sentence").await;
    let Frame::Json(last) = frames.last().unwrap() else { panic!() };
    assert_eq!(last["text"], "Première phrase.");
    let Frame::Bin(audio) = receive(&mut ws).await else { panic!() };
    assert_eq!(audio, pcm_for("Première phrase."));
    send(&mut ws, json!({"type": "cancel"})).await;
    assert_eq!(receive_json(&mut ws).await, json!({"type": "done", "run_id": null, "reason": "cancelled"}));
    gate.set();
    send(&mut ws, json!({"type": "ping"})).await;
    assert_eq!(receive_json(&mut ws).await, json!({"type": "pong"})); // nothing else was queued
    s.backend.with(|st| {
        let inputs: Vec<Value> = st.kyutai_inputs.iter().map(|b| b["input"].clone()).collect();
        assert_eq!(inputs, vec![json!("Première phrase.")]);
        assert_eq!(st.kyutai_inputs[0]["voice"], "5207");
    });
    s.backend.release.set();
}

#[tokio::test]
async fn voice_errors() {
    let s = serve().await;
    s.backend.with(|st| st.run_status.insert("missing".into(), 404));
    let mut ws = connect(&s.url, Some(BRIDGE_KEY)).await.unwrap();
    send(&mut ws, json!({"type": "follow", "agent": "ghost", "run_id": "r"})).await;
    assert_eq!(
        receive_json(&mut ws).await,
        json!({"type": "error", "code": "unknown_agent", "message": "agent inconnu"})
    );
    ws.send(Message::Text("not json".into())).await.unwrap();
    assert_eq!(receive_json(&mut ws).await["code"], "invalid_json");
    ws.send(Message::Text("[1]".into())).await.unwrap();
    assert_eq!(receive_json(&mut ws).await["code"], "invalid_json");
    ws.send(Message::Binary(vec![1u8, 2].into())).await.unwrap();
    assert_eq!(
        receive_json(&mut ws).await,
        json!({"type": "error", "code": "binary_not_supported", "message": "JSON attendu"})
    );
    send(&mut ws, json!({"type": "dance"})).await;
    assert_eq!(receive_json(&mut ws).await, json!({"type": "error", "code": "unknown_message", "message": "dance"}));
    send(&mut ws, json!({})).await;
    assert_eq!(receive_json(&mut ws).await["message"], "None");
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "a/b"})).await;
    assert_eq!(
        receive_json(&mut ws).await,
        json!({"type": "error", "code": "invalid_run_id", "message": "run_id invalide"})
    );
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "r", "format": "mp3"})).await;
    assert_eq!(receive_json(&mut ws).await, json!({"type": "error", "code": "invalid_format", "message": "mp3"}));
    send(&mut ws, json!({"type": "cancel"})).await; // nothing followed: no answer
    send(&mut ws, json!({"type": "ping"})).await;
    assert_eq!(receive_json(&mut ws).await, json!({"type": "pong"}));
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "missing"})).await;
    let frames = receive_frames(&mut ws, "done").await;
    let values: Vec<Value> =
        frames.into_iter().filter_map(|f| if let Frame::Json(v) = f { Some(v) } else { None }).collect();
    assert_eq!(values[0]["type"], "following");
    assert_eq!(values[1], json!({"type": "error", "code": "run_not_found", "message": "flux Hermes indisponible"}));
    assert_eq!(values[2]["reason"], "error");
    s.backend.release.set();
}

#[tokio::test]
async fn voice_resume_from_seq_and_final_text_fallback() {
    let s = serve().await;
    run(
        &s.backend,
        "run_3",
        vec![RunItem::Text(sse("run.completed", json!({"run_id": "run_3", "output": "Un. Deux. Trois."})))],
    );
    let mut ws = connect(&s.url, Some(BRIDGE_KEY)).await.unwrap();
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "run_3", "from_seq": 1})).await;
    let frames = receive_frames(&mut ws, "done").await;
    let sentences: Vec<&Value> = frames
        .iter()
        .filter_map(|f| if let Frame::Json(v) = f { Some(v) } else { None })
        .filter(|v| v["type"] == "sentence")
        .collect();
    let texts: Vec<&Value> = sentences.iter().map(|v| &v["text"]).collect();
    let seqs: Vec<&Value> = sentences.iter().map(|v| &v["seq"]).collect();
    assert_eq!(texts, [&json!("Deux."), &json!("Trois.")]);
    assert_eq!(seqs, [&json!(1), &json!(2)]);
    s.backend.release.set();
}

#[tokio::test]
async fn voice_reports_unreachable_hermes() {
    let s = serve().await;
    let mut ws = connect(&s.url, Some(BRIDGE_KEY)).await.unwrap();
    send(&mut ws, json!({"type": "follow", "agent": "wellness", "run_id": "down_1"})).await;
    let frames = receive_frames(&mut ws, "done").await;
    let values: Vec<Value> =
        frames.into_iter().filter_map(|f| if let Frame::Json(v) = f { Some(v) } else { None }).collect();
    assert_eq!(values[1]["code"], "hermes_unreachable");
    assert_eq!(*values.last().unwrap(), json!({"type": "done", "run_id": "down_1", "reason": "error"}));
    assert_eq!(s.backend.with(|st| st.run_requests.len()), 3);
}

#[tokio::test]
async fn a_wrong_key_is_refused_before_the_upgrade() {
    let s = serve().await;
    match connect(&s.url, Some("wrong")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
        other => panic!("expected 403, got {:?}", other.map(|_| ())),
    }
    assert!(connect(&s.url, None).await.is_err());
}
