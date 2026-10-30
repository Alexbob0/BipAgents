//! Group discussions (rooms.rs), 2026-10-30: an agent's file reaches the person in the room as in a one-to-one
//! conversation (a zip could not be handed), and back in its one-to-one conversation the agent is told what is new
//! in its rooms, once (rooms_memory.rs), nothing when nothing is new.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bipbridge::config::parse_config;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

#[derive(Clone, Default)]
struct Fake {
    runs: Arc<Mutex<Vec<Value>>>,
}

async fn hermes(State(fake): State<Fake>, request: Request) -> Response {
    let method = request.method().as_str().to_string();
    let path = request.uri().path().to_string();
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let out = |v: Value| axum::Json(v).into_response();
    if method == "POST" && path.ends_with("/api/sessions") {
        return out(json!({"id": format!("s-{}", path.replace('/', "_"))}));
    }
    if method == "POST" && path.ends_with("/v1/runs") {
        let mut runs = fake.runs.lock().unwrap();
        runs.push(body);
        return out(json!({"run_id": format!("run-{}", runs.len())}));
    }
    if method == "GET" && path.contains("/v1/runs/") {
        let output = if path.starts_with("/p/dev") {
            "Voici le site du club, prêt à mettre en ligne.\nMEDIA:/envoi/site.zip"
        } else {
            "Je relis les textes."
        };
        return out(json!({"status": "completed", "output": output, "usage": {"input_tokens": 300, "output_tokens": 20}}));
    }
    out(json!({"ok": true}))
}

#[tokio::test]
async fn a_rooms_file_reaches_the_person_and_one_to_one_hears_of_the_room() {
    let fake = Fake::default();
    let hermes_url = common::spawn_fake(Router::new().fallback(hermes).with_state(fake.clone())).await;
    let dir = tempfile::tempdir().unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["outbox"] = json!({"db_path": dir.path().join("data/bridge.db").to_string_lossy()});
    let mut table = serde_json::Map::new();
    for a in ["dev", "bip"] {
        table.insert(
            a.to_string(),
            json!({"hermes_url": format!("{hermes_url}/p/{a}"), "hermes_key": format!("{a}-key-{}", "x".repeat(24)),
                   "shares": "external"}),
        );
    }
    d["agents"] = Value::Object(table);
    let app = TestApp {
        app: bipbridge::app::create_app(parse_config(&d, None).unwrap(), bipbridge::app::AppOptions { start_background: false })
            .unwrap(),
        peer: "198.51.100.7:50000".parse().unwrap(),
    };
    // the zip the agent made in its outbox (an agent set up by hand: <data>/envoi/<agent>)
    let outbox = dir.path().join("data/envoi/dev");
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(outbox.join("site.zip"), b"PK\x03\x04zip").unwrap();
    let (k, v) = auth();
    let h = [(k, v.as_str())];

    // nothing said in any room yet: no note joins the one-to-one message (0 token more)
    let take = json!({"agent": "dev", "session": "s1", "text": "Salut"});
    let recap = app.send_json("POST", "/v1/live/recaps/take", &h, &take).await.json();
    assert!(recap["recap"].as_str().is_none_or(|r| !r.contains("bip:rooms")), "{recap}");

    let made = app.send_json("POST", "/v1/rooms", &h, &json!({"participants": ["dev", "bip"], "title": "Site du club"})).await;
    assert_eq!(made.status, 200, "{}", made.text());
    let id = made.json()["id"].as_str().unwrap().to_string();
    let path = format!("/v1/rooms/{id}/messages");
    let sent = app.send_json("POST", &path, &h, &json!({"text": "@dev fais le site du club de foot"})).await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    let mut messages = Value::Null;
    for _ in 0..200 {
        messages = app.get(&path, &h).await.json();
        if messages["answering"] == json!([]) && messages["messages"].as_array().unwrap().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let reply = &messages["messages"][1];
    assert_eq!(reply["from"], "dev", "{messages}");
    assert_eq!(reply["files"], json!(["/envoi/site.zip"]), "{messages}");
    assert!(!reply["text"].as_str().unwrap().contains("MEDIA:"));
    // the run was told files may be handed, the vault never
    let runs = fake.runs.lock().unwrap().clone();
    let rules = runs[0]["instructions"].as_str().unwrap_or_default().to_string();
    assert!(rules.contains("MEDIA:<path> line on its own line") && !rules.contains("never put files"), "{rules}");
    // the file kept by the bridge at the turn's end, served for the room's message as for a conversation's
    let media = app.get("/v1/media?path=/envoi/site.zip&agent=dev", &h).await;
    assert_eq!(media.status, 200);
    std::fs::remove_file(outbox.join("site.zip")).unwrap(); // the sandbox reset: the copy stays
    assert_eq!(app.get("/v1/media?path=/envoi/site.zip&agent=dev", &h).await.status, 200);

    // back in the one-to-one conversation: what's new in the room, once
    let recap = app.send_json("POST", "/v1/live/recaps/take", &h, &take).await.json();
    let note = recap["recap"].as_str().unwrap_or_default().to_string();
    assert!(note.contains("<!--bip:rooms\n") && note.contains("From the room « Site du club »:"), "{note}");
    assert!(note.contains("- You: Voici le site du club, prêt à mettre en ligne. [file: site.zip]"), "{note}");
    assert!(note.contains("- Your person: @dev fais le site du club de foot"), "{note}");
    let again = app.send_json("POST", "/v1/live/recaps/take", &h, &take).await.json();
    assert!(again["recap"].as_str().is_none_or(|r| !r.contains("bip:rooms")), "{again}");
}
