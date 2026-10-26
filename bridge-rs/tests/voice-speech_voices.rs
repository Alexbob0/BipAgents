//! Port of tests/test_voices.py: the person's own voice server, which voice speaks for what, and the routes the app
//! changes it with.

mod common;

use bipbridge::config::{parse_config, Config};
use bipbridge::voices::{set_section_keys, voice_for};
use common::fakes::FakeBackend;
use common::{auth, config_dict, TestApp};
use serde_json::{json, Value};

fn server_config(dir: &std::path::Path, backend: &FakeBackend) -> Config {
    let path = dir.join("bridge.toml");
    std::fs::write(
        &path,
        "bridge_key = \"x\"\n\n[voice_server]\nurl = \"http://gpu.test:8880\"\n\n[agents.wellness]\ndisplay_name = \"Wellness\"\nntfy_topic = \"t\"\n",
    )
    .unwrap();
    let mut dict = config_dict(dir, &backend.urls());
    // nothing listens there: the voices come from the config
    dict["voice_server"] = json!({"url": "http://127.0.0.1:9", "voices": ["af_bella", "am_adam"], "live": false});
    dict["agents"]["Wellness"]["server_voice"] = json!("server:fr/af_bella");
    parse_config(&dict, Some(path.to_str().unwrap())).unwrap()
}

#[test]
fn each_use_speaks_with_the_voice_it_goes_to() {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Default::default());
    dict["voice_server"] = json!({"url": "http://127.0.0.1:9", "voices": ["af_bella", "am_adam"], "live": false});
    dict["agents"]["Wellness"]["server_voice"] = json!("server:fr/af_bella");
    let config = parse_config(&dict, None).unwrap();
    let mut agent = config.agent(Some("wellness")).unwrap().clone();
    assert_eq!(voice_for(&config, Some(&agent), Some("replies")), "server:fr/af_bella");
    assert_eq!(voice_for(&config, Some(&agent), Some("reading")), "server:fr/af_bella");
    assert_eq!(voice_for(&config, Some(&agent), Some("live")), "4193"); // the Live stays on its Bip
    assert_eq!(voice_for(&config, Some(&agent), None), "4193");
    agent.server_voice = None;
    assert_eq!(voice_for(&config, Some(&agent), Some("replies")), "4193"); // no voice picked on the server yet
    assert_eq!(voice_for(&config, None, Some("replies")), "5476");
}

#[test]
fn section_keys_are_replaced_added_or_removed_and_the_rest_stays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    std::fs::write(
        &path,
        "[voice_server]\nurl = \"http://gpu:8880\"\nlive = true\n# a comment\n\n[agents.vie]\nvoice = \"a\"\n",
    )
    .unwrap();
    let p = path.to_str().unwrap();
    set_section_keys(p, "voice_server", &[("live".into(), Some(json!(false))), ("reading".into(), Some(json!(true)))])
        .unwrap();
    set_section_keys(p, "agents.vie", &[("voice".into(), None), ("server_voice".into(), Some(json!("server:fr/x")))])
        .unwrap();
    set_section_keys(p, "pronunciations.fr", &[("Garmin".into(), Some(json!("Garmine")))]).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[voice_server]\nurl = \"http://gpu:8880\"\nlive = false\n# a comment\nreading = true\n\n\
         [agents.vie]\nserver_voice = \"server:fr/x\"\n\n[pronunciations.fr]\nGarmin = \"Garmine\"\n"
    );
}

#[tokio::test]
async fn the_app_lists_the_server_voices_and_changes_the_uses() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let config = server_config(dir.path(), &backend);
    let source = config.source_path.clone().unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let body = app.get("/v1/voices", &[(k, &v)]).await.json();
    assert_eq!(
        body,
        json!({"pocket": true, "server": {"voices": ["af_bella", "am_adam"], "uses": {"live": false, "replies": true, "reading": true}}})
    );
    let resp = app.send_json("PUT", "/v1/voices/uses", &[(k, &v)], &json!({"live": true, "reading": false})).await;
    assert_eq!(resp.json(), json!({"live": true, "replies": true, "reading": false}));
    let text = std::fs::read_to_string(&source).unwrap();
    assert!(text.contains("live = true") && text.contains("reading = false"));
    backend.release.set();
}

#[tokio::test]
async fn the_app_sets_an_agents_voices() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let config = server_config(dir.path(), &backend);
    let source = config.source_path.clone().unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let resp = app
        .send_json(
            "PUT",
            "/v1/agents/wellness/voices",
            &[(k, &v)],
            &json!({"voice": "pocket:mousse", "server_voice": "server:fr/am_adam"}),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json(), json!({"voice": "pocket:mousse", "server_voice": "server:fr/am_adam"}));
    let config = app.app.state.config();
    let agent = config.agent(Some("wellness")).unwrap();
    assert_eq!(agent.voice.as_deref(), Some("pocket:mousse"));
    assert_eq!(agent.server_voice.as_deref(), Some("server:fr/am_adam"));
    let text = std::fs::read_to_string(&source).unwrap();
    assert!(text.contains("voice = \"pocket:mousse\"") && text.contains("server_voice = \"server:fr/am_adam\""));
    assert!(text.contains("ntfy_topic = \"t\""));
    assert_eq!(app.send_json("PUT", "/v1/agents/nobody/voices", &[(k, &v)], &json!({})).await.status, 404);
    let too_long = "x".repeat(301);
    let resp = app.send_json("PUT", "/v1/agents/wellness/voices", &[(k, &v)], &json!({"voice": too_long})).await;
    assert_eq!(resp.status, 422);
    assert_eq!(resp.json()["detail"][0]["type"], "string_too_long");
}

#[tokio::test]
async fn without_a_voice_server() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = TestApp::new(common::config(dir.path(), &backend.urls()));
    let (k, v) = auth();
    assert_eq!(app.get("/v1/voices", &[(k, &v)]).await.json(), json!({"pocket": true, "server": null}));
    let resp = app.send_json("PUT", "/v1/voices/uses", &[(k, &v)], &json!({"live": false})).await;
    assert_eq!(resp.status, 409);
    assert_eq!(resp.json(), json!({"detail": "no voice server"}));
    backend.release.set();
}

#[tokio::test]
async fn the_app_keeps_an_agents_look_for_the_other_apps() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let config = server_config(dir.path(), &backend);
    let source = config.source_path.clone().unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let resp = app
        .send_json(
            "PUT",
            "/v1/agents/wellness/appearance",
            &[(k, &v)],
            &json!({"category": "creative", "color": "#f2649e"}),
        )
        .await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json(), json!({"category": "creative", "color": "#F2649E", "outfit": "", "costumes": true}));
    let listed = app.get("/v1/agents", &[(k, &v)]).await.json();
    let wellness: &Value = listed["agents"].as_array().unwrap().iter().find(|a| a["id"] == "wellness").unwrap();
    assert_eq!(wellness["category"], "creative");
    assert_eq!(wellness["color"], "#F2649E");
    assert!(std::fs::read_to_string(&source).unwrap().contains("category = \"creative\""));
    let put = |body: Value| {
        let app = &app;
        let v = v.clone();
        async move { app.send_json("PUT", "/v1/agents/wellness/appearance", &[(k, &v)], &body).await }
    };
    // The Bip's costume: kept when an app leaves it out, cleared with "", events off kept as `costumes = false`
    let resp = put(json!({"category": "creative", "outfit": "chef", "costumes": false})).await;
    assert_eq!(resp.json(), json!({"category": "creative", "color": null, "outfit": "chef", "costumes": false}));
    let text = std::fs::read_to_string(&source).unwrap();
    assert!(text.contains("outfit = \"chef\"") && text.contains("costumes = false"));
    let resp = put(json!({"category": "home"})).await;
    assert_eq!(resp.json(), json!({"category": "home", "color": null, "outfit": "chef", "costumes": false}));
    let listed = app.get("/v1/agents", &[(k, &v)]).await.json();
    let wellness: &Value = listed["agents"].as_array().unwrap().iter().find(|a| a["id"] == "wellness").unwrap();
    assert_eq!((wellness["outfit"].clone(), wellness["costumes"].clone()), (json!("chef"), json!(false)));
    let resp = put(json!({"category": "home", "outfit": "", "costumes": true})).await;
    assert_eq!(resp.json(), json!({"category": "home", "color": null, "outfit": "", "costumes": true}));
    let text = std::fs::read_to_string(&source).unwrap();
    assert!(!text.contains("outfit =") && !text.contains("costumes ="));
    assert_eq!(put(json!({"category": "home", "outfit": "x".repeat(31)})).await.status, 422);
    assert_eq!(put(json!({"category": "dragon"})).await.status, 400);
    let resp = put(json!({"category": "tech", "color": "red"})).await;
    assert_eq!(resp.status, 422);
    assert_eq!(
        resp.json()["detail"][0],
        json!({"type": "string_pattern_mismatch", "loc": ["body", "color"], "msg": "String should match pattern '^#[0-9A-Fa-f]{6}$'",
               "input": "red", "ctx": {"pattern": "^#[0-9A-Fa-f]{6}$"}})
    );
    assert_eq!(put(json!({"color": "#000000"})).await.status, 422); // category is required
    let resp = app.send_json("PUT", "/v1/agents/nobody/appearance", &[(k, &v)], &json!({"category": "tech"})).await;
    assert_eq!(resp.status, 404);
    backend.release.set();
}
