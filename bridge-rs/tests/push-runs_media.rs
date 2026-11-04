//! Files the agents point to with MEDIA: lines: served under the declared roots only, and a podcast's audio kept
//! as the outbox item's audio; cron jobs' notification preference (Python: the media parts of
//! tests/test_media_and_jobs.py).

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use bipbridge::config::parse_config;
use bipbridge::media::{extract_media, resolve};
use bipbridge::outbox::BoiteMessage;
use common::fakes::*;
use common::*;
use indexmap::IndexMap;
use serde_json::{json, Value};

#[test]
fn extract_and_resolve_media() {
    let text = "MEDIA:/home/hermes/simplex-files/point.mp3\n\n🎙️ Point du matin — lundi 5 octobre";
    assert_eq!(
        extract_media(text),
        ("🎙️ Point du matin — lundi 5 octobre".to_string(), vec!["/home/hermes/simplex-files/point.mp3".to_string()])
    );
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("simplex");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("point.mp3"), b"ID3podcast").unwrap();
    std::fs::write(dir.path().join("secret.mp3"), b"no").unwrap();
    let mut roots = IndexMap::new();
    roots.insert("/home/hermes/simplex-files".to_string(), root.to_string_lossy().into_owned());
    let real = std::fs::canonicalize(root.join("point.mp3")).unwrap().to_string_lossy().into_owned();
    assert_eq!(resolve("/home/hermes/simplex-files/point.mp3", &roots), Some(real));
    assert_eq!(resolve("/home/hermes/simplex-files/../secret.mp3", &roots), None); // traversal
    assert_eq!(resolve("/home/hermes/simplex-files/missing.mp3", &roots), None);
    assert_eq!(resolve("/etc/passwd", &roots), None);
    std::fs::write(root.join("notes.sh"), "x").unwrap();
    assert_eq!(resolve("/home/hermes/simplex-files/notes.sh", &roots), None); // not media
}

#[tokio::test]
async fn media_route_and_podcast_audio_in_the_outbox() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let root = dir.path().join("simplex");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("point.mp3"), b"ID3podcast-audio").unwrap();
    let mut data = config_dict(dir.path(), &backend.urls());
    data["media"] = json!({"roots": {"/home/hermes/simplex-files": root.to_string_lossy()}});
    let client = TestApp::new(parse_config(&data, None).unwrap());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = client.get("/v1/media?path=%2Fhome%2Fhermes%2Fsimplex-files%2Fpoint.mp3", &h).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.body, b"ID3podcast-audio");
    assert_eq!(resp.header("content-type").as_deref(), Some("audio/mpeg"));
    assert_eq!(client.get("/v1/media?path=%2Fetc%2Fpasswd", &h).await.status, StatusCode::NOT_FOUND);
    let services = client.app.state.clone();
    let agent = services.config.get().agent(Some("wellness")).cloned().unwrap();
    let item = services
        .outbox
        .ingest(
            &agent,
            BoiteMessage {
                id: Some("n1".into()),
                ..BoiteMessage::new("MEDIA:/home/hermes/simplex-files/point.mp3\n\nPoint du matin")
            },
            true,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item["text"], "Point du matin");
    let audio = client.get(&format!("/v1/outbox/{}/audio", item["id"].as_str().unwrap()), &h).await;
    assert_eq!(audio.status, StatusCode::OK);
    assert_eq!(audio.body, b"ID3podcast-audio"); // the podcast, not a synthesis
    assert!(backend.with(|s| s.pocket_inputs.is_empty()));
    client.shutdown().await;
    backend.release.set();
}

#[tokio::test]
async fn cron_job_notification_preference() {
    let dir = tempfile::tempdir().unwrap();
    let client = TestApp::new(config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let config = client.app.state.config();
    let id = bipbridge::routines::create(&config, "wellness", "Scan des e-mails", "every 2h", "Scanne", None).unwrap().id;
    let jobs = client.get("/v1/cron-jobs", &h).await.json()["jobs"].clone();
    let listed: Vec<(Value, Value, Value)> =
        jobs.as_array().unwrap().iter().map(|j| (j["job"].clone(), j["name"].clone(), j["notify"].clone())).collect();
    assert_eq!(listed, [(json!(id), json!("Scan des e-mails"), json!(true))]);
    let resp = client.send_json("PUT", &format!("/v1/cron-jobs/wellness/{id}"), &h, &json!({"notify": false})).await;
    assert_eq!(resp.status, StatusCode::OK);
    assert!(!bipbridge::routines::get(&config, &id).unwrap().notify); // muted, name kept
    assert_eq!(client.get("/v1/cron-jobs?agent=wellness", &h).await.json()["jobs"][0]["name"], "Scan des e-mails");
    let resp = client.send_json("PUT", "/v1/cron-jobs/wellness/nope", &h, &json!({"notify": false})).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn muted_job_reply_is_stored_without_push() {
    let dir = tempfile::tempdir().unwrap();
    let apns = FakeApns::start().await;
    let mut data = config_dict(dir.path(), &Urls::default());
    data["apns"]["base_url"] = json!(apns.url);
    let client = TestApp::new(parse_config(&data, None).unwrap());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let resp = client
        .send_json(
            "POST",
            "/v1/devices",
            &h,
            &json!({"token": "ab".repeat(32), "environment": "sandbox", "agent_ids": ["wellness"]}),
        )
        .await;
    assert_eq!(resp.status, StatusCode::OK);
    let services = client.app.state.clone();
    let agent = services.config.get().agent(Some("wellness")).cloned().unwrap();
    let item = services
        .outbox
        .ingest(&agent, BoiteMessage { id: Some("s1".into()), ..BoiteMessage::new("Tri des e-mails fait.") }, false)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    let item = item.expect("stored");
    assert!(apns.requests.lock().unwrap().is_empty());
    let mut found = false;
    for _ in 0..30 {
        found = boite_items(&client).await.iter().any(|row| row["id"] == item["id"]); // muted: in the Boîte, not pushed
        if found {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(found);
    client.shutdown().await;
}
