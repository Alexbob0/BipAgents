//! The voice decider's escalation (voice_escalate.rs): the phone's model unsure, the agent's own model asked for one
//! word, cut at 2.5 s; refusals and no model answer `null` (the phone keeps its own answer).

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::routing::post;
use axum::{Json, Router};
use bipbridge::config::parse_config;
use common::{config_dict, spawn_fake, Urls};
use serde_json::{json, Value};

async fn model(reply: &'static str, delay: Duration, seen: Arc<Mutex<Vec<Value>>>) -> String {
    let router = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body);
                tokio::time::sleep(delay).await;
                Json(json!({"choices": [{"message": {"content": reply}}]}))
            }
        }),
    );
    spawn_fake(router).await
}

fn app(dir: &std::path::Path, model_url: Option<&str>) -> common::TestApp {
    let mut dict = config_dict(dir, &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["ours"] = json!({"display_name": "Ours", "hermes_url": "http://127.0.0.1:8642/p/ours", "hermes_key": "k"});
    if let Some(url) = model_url {
        let profile = dir.join(".hermes/profiles/ours");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("config.yaml"),
            format!("model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: {url}/v1\n  key_env: CUSTOM_MODEL_API_KEY\n"),
        )
        .unwrap();
        std::fs::write(profile.join(".env"), "CUSTOM_MODEL_API_KEY=sk-test-key\n").unwrap();
    }
    common::TestApp::new(parse_config(&dict, None).unwrap())
}

fn ask(text: &str) -> Value {
    json!({"agent": "ours", "mode": "live", "text": text, "before": ["I would like a cup of tea"]})
}

#[tokio::test]
async fn the_agents_model_answers_in_one_word() {
    let dir = tempfile::tempdir().unwrap();
    let seen: Arc<Mutex<Vec<Value>>> = Default::default();
    let url = model("Noter.", Duration::ZERO, seen.clone()).await;
    let app = app(dir.path(), Some(&url));
    let (k, v) = common::auth();
    let answer = app.send_json("POST", "/v1/voice/escalate", &[(k, &v)], &ask("le dentiste est jeudi")).await;
    assert_eq!(answer.status, 200);
    assert_eq!(answer.json()["decision"], "noter");
    let body = seen.lock().unwrap()[0].clone();
    assert_eq!(body["model"], "glm-5.3-flash");
    let user = body["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("Assistant's name: Ours.") && user.contains("- I would like a cup of tea"), "{user}");
    assert!(user.contains("Last chunk (just before the pause): le dentiste est jeudi"), "{user}");
}

#[tokio::test]
async fn too_slow_or_no_model_is_null() {
    let dir = tempfile::tempdir().unwrap();
    let url = model("lancer", Duration::from_secs(4), Default::default()).await;
    let app = app(dir.path(), Some(&url));
    let (k, v) = common::auth();
    let started = Instant::now();
    let answer = app.send_json("POST", "/v1/voice/escalate", &[(k, &v)], &ask("le dentiste")).await;
    assert_eq!((answer.status.as_u16(), answer.json()["decision"].clone()), (200, Value::Null));
    assert!(started.elapsed() < Duration::from_millis(3200), "cut at 2.5 s: {:?}", started.elapsed());

    let dir = tempfile::tempdir().unwrap();
    let app = self::app(dir.path(), None);
    let answer = app.send_json("POST", "/v1/voice/escalate", &[(k, &v)], &ask("le dentiste")).await;
    assert_eq!((answer.status.as_u16(), answer.json()["decision"].clone()), (200, Value::Null));
}

#[tokio::test]
async fn refused_requests() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path(), None);
    let (k, v) = common::auth();
    assert_eq!(app.send_json("POST", "/v1/voice/escalate", &[], &ask("x")).await.status, 401);
    let unknown = json!({"agent": "nobody", "text": "x"});
    assert_eq!(app.send_json("POST", "/v1/voice/escalate", &[(k, &v)], &unknown).await.status, 404);
    for bad in [json!({"agent": "ours", "text": " "}), json!({"agent": "ours", "text": "x", "mode": "radio"})] {
        assert_eq!(app.send_json("POST", "/v1/voice/escalate", &[(k, &v)], &bad).await.status, 422, "{bad}");
    }
}
