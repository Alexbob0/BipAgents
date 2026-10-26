//! Port of bridge/tests/test_agent_tools.py: the agents' MCP tools (audio, podcasts, registration in the profiles,
//! tokens, purge) and the knowledge tools of test_mail_scan.py.

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use bipbridge::agent_tools::{self, AudioMaker};
use bipbridge::agents_admin::Run;
use bipbridge::config::{parse_config, AgentConfig, SharedConfig};
use bipbridge::Dict;
use common::fakes::FakeBackend;
use common::{auth, config, config_dict, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

type Commands = Arc<Mutex<Vec<(Vec<String>, BTreeMap<String, String>)>>>;

fn recording_run(commands: &Commands) -> Run {
    let commands = commands.clone();
    Arc::new(move |args, env| {
        commands.lock().unwrap().push((args, env));
        Box::pin(async { Ok(String::new()) })
    })
}

#[tokio::test]
async fn each_agent_of_the_install_gets_the_tools_with_a_token_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let hub = dir.path().join(".hermes");
    std::fs::create_dir_all(hub.join("profiles").join("vie")).unwrap();
    std::fs::write(hub.join("profiles/vie/config.yaml"), "model:\n  default: x\n").unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": hub.to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["Vie"] = json!({"hermes_url": "http://127.0.0.1:8642/p/vie", "hermes_key": "k".repeat(32)});
    let config = parse_config(&dict, None).unwrap();
    let commands: Commands = Default::default();
    let run = recording_run(&commands);
    agent_tools::register_all(SharedConfig::new(config.clone()), run.clone()).await;
    let seen = commands.lock().unwrap().clone();
    // Only the agents this install manages (Wellness is set up by hand): its Hermes, its profile, its HERMES_HOME.
    assert!(!seen.is_empty() && seen.iter().all(|(args, _)| args[2] == "vie" && args[0] == "/opt/hermes"));
    assert!(seen
        .iter()
        .all(|(_, env)| env.get("HERMES_HOME").map(String::as_str) == Some(&*hub.to_string_lossy()) && env.len() == 1));
    let values: BTreeMap<String, String> = seen.iter().map(|(a, _)| (a[5].clone(), a[6].clone())).collect();
    assert_eq!(
        values["mcp_servers.bipagents.url"],
        // the Carnet's tools are on by default: the install's tools and those
        format!(
            "http://127.0.0.1:{}/v1/agent-tools/mcp?v={}",
            config.port,
            agent_tools::version_of(&agent_tools::tools_for(&config, config.agent(Some("vie")).unwrap()))
        )
    );
    assert_eq!(values["mcp_servers.bipagents.timeout"], agent_tools::TOOL_TIMEOUT.to_string());
    let token = values["mcp_servers.bipagents.headers.Authorization"].strip_prefix("Bearer ").unwrap().to_string();
    assert_eq!(agent_tools::agent_for(&config, &token).unwrap().name, "vie");
    assert!(agent_tools::agent_for(&config, "nope").is_none());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(agent_tools::store_path(&config)).unwrap().permissions().mode() & 0o777, 0o600);
    // Once Hermes holds it: nothing to do at the next start, and the same token.
    std::fs::write(
        hub.join("profiles/vie/config.yaml"),
        format!(
            "mcp_servers:\n  bipagents:\n    url: {}\n    headers:\n      Authorization: Bearer {token}\n    timeout: {}\n\
             \x20 bipagents_checkout:\n    url: {}\n    headers:\n      Authorization: Bearer {token}\n    timeout: {}\n",
            values["mcp_servers.bipagents.url"],
            agent_tools::TOOL_TIMEOUT,
            // the checkout tool every agent gets (handover.rs), same token
            bipbridge::handover::tools_url(&config),
            bipbridge::handover::TOOL_TIMEOUT
        ),
    )
    .unwrap();
    commands.lock().unwrap().clear();
    agent_tools::register_all(SharedConfig::new(config.clone()), run.clone()).await;
    assert!(commands.lock().unwrap().is_empty());
    // New tools in a later version: a new entry, so Hermes fetches the list again (the token stays).
    let vie = config.agent(Some("vie")).unwrap().clone();
    let later = agent_tools::url_with(&config, "later12345");
    assert!(agent_tools::register_at(&config, &vie, &run, &later).await.unwrap());
    let changed: BTreeMap<String, String> =
        commands.lock().unwrap().iter().map(|(a, _)| (a[5].clone(), a[6].clone())).collect();
    assert_ne!(changed["mcp_servers.bipagents.url"], values["mcp_servers.bipagents.url"]);
    assert_eq!(changed["mcp_servers.bipagents.headers.Authorization"], format!("Bearer {token}"));
    // A deleted agent's token no longer opens anything.
    agent_tools::forget(&config, "vie");
    assert!(agent_tools::agent_for(&config, &token).is_none());
}

const TOKEN: &str = "agent-token-for-wellness-0123456789";

async fn app_with_token(dir: &std::path::Path, backend: &FakeBackend) -> TestApp {
    let config = config(dir, &backend.urls());
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    TestApp::new(config)
}

async fn rpc(
    app: &TestApp,
    token: &str,
    method: &str,
    params: Value,
    request_id: Option<i64>,
    accept: &str,
) -> common::TestResponse {
    let mut body = json!({"jsonrpc": "2.0", "method": method, "params": params});
    if let Some(id) = request_id {
        body["id"] = json!(id);
    }
    let bearer = format!("Bearer {token}");
    app.send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", accept)], &body).await
}

async fn call(app: &TestApp, name: &str, arguments: Value) -> Value {
    rpc(app, TOKEN, "tools/call", json!({"name": name, "arguments": arguments}), Some(1), "application/json")
        .await
        .json()["result"]
        .clone()
}

fn text_of(result: &Value) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn an_agent_turns_a_text_into_audio_with_its_voice() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app_with_token(dir.path(), &backend).await;
    assert_eq!(rpc(&app, "wrong", "initialize", json!({}), Some(1), "application/json").await.status, 401);
    let init = rpc(&app, TOKEN, "initialize", json!({"protocolVersion": "2025-06-18"}), Some(1), "application/json")
        .await
        .json();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert!(!init["result"]["capabilities"]["tools"].is_null());
    assert_eq!(rpc(&app, TOKEN, "notifications/initialized", json!({}), None, "application/json").await.status, 202);
    let tools = rpc(&app, TOKEN, "tools/list", json!({}), Some(1), "application/json").await.json();
    let names: Vec<&str> =
        tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(&names[..3], &["create_audio", "create_podcast", "list_voices"]);

    let text = "Bonjour et bienvenue dans ce podcast.\n\nVoici les nouvelles du jour.";
    let result = call(&app, "create_audio", json!({"text": text, "title": "Podcast du matin"})).await;
    assert_eq!(result["isError"], false, "{result}");
    let media = text_of(&result).split("MEDIA:").nth(1).unwrap().trim().to_string();
    assert!(media.starts_with("/bipagents/audio/wellness/") && media.contains("podcast-du-matin"), "{media}");
    // Read with the agent's reading voice, one paragraph at a time, in the background queue.
    let voices: Vec<Value> = backend.with(|s| s.kyutai_inputs.iter().map(|b| b["voice"].clone()).collect());
    assert_eq!(voices, vec![json!("4193"), json!("4193")]);
    // The app gets the file like any MEDIA: file.
    let (k, v) = auth();
    let audio = app.get(&format!("/v1/media?path={}", urlencoding(&media)), &[(k, &v)]).await;
    assert!(audio.status == 200 && audio.body.len() > 44, "{}", audio.status);
    assert_eq!(app.get("/v1/media?path=/bipagents/audio/../bridge.db", &[(k, &v)]).await.status, 404);
}

fn urlencoding(text: &str) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_./".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn a_long_call_is_streamed_and_errors_are_the_tools_not_the_protocols() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app_with_token(dir.path(), &backend).await;
    let streamed = rpc(
        &app,
        TOKEN,
        "tools/call",
        json!({"name": "create_audio", "arguments": {"text": "Une phrase."}}),
        Some(1),
        "application/json, text/event-stream",
    )
    .await;
    assert!(streamed.header("content-type").unwrap().starts_with("text/event-stream"));
    let text = streamed.text();
    let event = text.lines().rfind(|l| l.starts_with("data: ")).unwrap();
    let parsed: Value = serde_json::from_str(&event["data: ".len()..]).unwrap();
    assert_eq!(parsed["result"]["isError"], false);
    let too_long = call(&app, "create_audio", json!({"text": "x".repeat(agent_tools::MAX_CHARS + 1)})).await;
    assert!(too_long["isError"] == true && text_of(&too_long).contains("too long"));
    backend.with(|s| s.kyutai_status = 500);
    let failed = call(&app, "create_audio", json!({"text": "Autre phrase."})).await;
    assert_eq!(failed["isError"], true);
    let unknown = rpc(&app, TOKEN, "resources/list", json!({}), Some(1), "application/json").await.json();
    assert_eq!(unknown["error"]["code"], -32601);
    // No server stream; the session's end is acknowledged.
    let bearer = format!("Bearer {TOKEN}");
    let get = app.get("/v1/agent-tools/mcp", &[("authorization", &bearer)]).await;
    assert_eq!((get.status.as_u16(), get.header("allow")), (405, Some("POST".to_string())));
    assert_eq!(app.request("DELETE", "/v1/agent-tools/mcp", &[("authorization", &bearer)], None).await.status, 200);
    let bad = app.request("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer)], Some("{".into())).await;
    assert_eq!(bad.status, 400);
    assert_eq!(bad.json()["error"]["code"], -32700);
}

#[test]
fn old_audio_files_are_purged() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let folder = agent_tools::audio_root(&config).join("wellness");
    std::fs::create_dir_all(&folder).unwrap();
    let (old, new) = (folder.join("old.m4a"), folder.join("new.m4a"));
    std::fs::write(&old, b"x").unwrap();
    std::fs::write(&new, b"y").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    assert_eq!(agent_tools::purge_audio(&config, 30, None), 1);
    assert!(!old.exists() && new.exists());
}

#[tokio::test]
async fn a_podcast_gives_each_host_a_voice_of_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let backend = FakeBackend::start().await;
    let app = app_with_token(dir.path(), &backend).await;
    let listed = call(&app, "list_voices", json!({})).await;
    let lines: Vec<String> = text_of(&listed).lines().map(str::to_string).collect();
    assert_eq!(&lines[..2], &["4193 (your voice)", "pocket:colibri (Bip)"]);
    let turns = json!([{"speaker": "Max", "text": "Bonjour à tous, bienvenue."},
                       {"speaker": "Léa", "text": "Salut Max ! Alors, quoi de neuf ?"},
                       {"speaker": "Max", "text": "Trois nouvelles aujourd'hui."}]);
    let result = call(&app, "create_podcast", json!({"title": "Le point", "turns": turns})).await;
    assert_eq!(result["isError"], false, "{result}");
    assert!(text_of(&result).contains("Max: 4193") && text_of(&result).contains("Léa: pocket:colibri"));
    // The first host with the agent's voice, the other with another one; turns read in order.
    let kyutai: Vec<Value> = backend.with(|s| s.kyutai_inputs.iter().map(|b| b["voice"].clone()).collect());
    let pocket: Vec<Value> = backend.with(|s| s.pocket_inputs.iter().map(|b| b["voice"].clone()).collect());
    assert_eq!(kyutai, vec![json!("4193"), json!("4193")]);
    assert_eq!(pocket, vec![json!("colibri")]);
    // A voice picked by the agent, from list_voices; an unknown one is refused.
    let picked = call(&app, "create_podcast", json!({"turns": turns, "voices": {"Léa": "pocket:galet"}})).await;
    assert_eq!(picked["isError"], false);
    assert_eq!(backend.with(|s| s.pocket_inputs.last().unwrap()["voice"].clone()), "galet");
    let bad = call(&app, "create_podcast", json!({"turns": turns, "voices": {"Léa": "pocket:dragon"}})).await;
    assert!(bad["isError"] == true && text_of(&bad).contains("unknown voice"));
    let empty = call(&app, "create_podcast", json!({"turns": [{"speaker": "Max"}]})).await;
    assert_eq!(empty["isError"], true);
}

fn voice(v: &str, kind: &str) -> Dict {
    serde_json::from_value(json!({"voice": v, "kind": kind})).unwrap()
}

#[test]
fn with_a_voice_model_the_hosts_get_its_natural_voices() {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["voice_server"] = json!({"url": "http://voices.test"});
    dict["agents"]["Wellness"]["voice"] = json!("pocket:mousse");
    let mut config = parse_config(&dict, None).unwrap();
    let available = vec![
        voice("pocket:mousse", "your voice"),
        voice("server:fr/fr-mascotte-s17", "voice model"),
        voice("server:fr/fr-naturelle-s15", "voice model"),
        voice("server:fr/fr-naturelle-s21", "voice model"),
        voice("pocket:galet", "Bip"),
    ];
    let speakers = vec!["Max".to_string(), "Léa".to_string()];
    let cast = agent_tools::cast_from(&config, &available, &speakers, &IndexMap::new()).unwrap();
    // The agent speaks with a Bip, but a podcast takes the model's natural voices, two different ones.
    assert_eq!(cast["Max"], "server:fr/fr-naturelle-s15");
    assert_eq!(cast["Léa"], "server:fr/fr-naturelle-s21");
    // the person keeps the voice model away from reading: Bips
    config.voice_server.as_mut().unwrap().uses.insert("reading".into(), false);
    let cast = agent_tools::cast_from(&config, &available, &speakers, &IndexMap::new()).unwrap();
    assert_eq!((cast["Max"].as_str(), cast["Léa"].as_str()), ("pocket:mousse", "pocket:galet"));
}

#[tokio::test]
async fn the_agent_reads_and_searches_its_knowledge_pages() {
    let dir = tempfile::tempdir().unwrap();
    let mut dict = config_dict(dir.path(), &Urls::default());
    dict["hermes"] = json!({"bin": "/opt/hermes", "home": dir.path().join(".hermes").to_string_lossy(), "hub_url": "http://127.0.0.1:8642"});
    dict["agents"]["coach"] =
        json!({"display_name": "Coach", "hermes_url": "http://127.0.0.1:8642/p/coach", "hermes_key": "k"});
    let config = parse_config(&dict, None).unwrap();
    let agent: AgentConfig = config.agent(Some("coach")).unwrap().clone();
    let profile = dir.path().join(".hermes/profiles/coach");
    std::fs::create_dir_all(profile.join("knowledge/mail/people")).unwrap();
    std::fs::write(profile.join("knowledge/mail/index.md"), "# Index\n- [Sam](people/sam.md)\n").unwrap();
    std::fs::write(profile.join("knowledge/mail/people/sam.md"), "# Sam\n- 2026-10-18 — Repas chez Sam dimanche\n")
        .unwrap();
    let args = |v: Value| -> Dict { serde_json::from_value(v).unwrap() };
    let text = |d: Dict| Value::Object(d)["content"][0]["text"].as_str().unwrap().to_string();
    let listed = text(agent_tools::knowledge_tool(&config, &agent, "knowledge_read", &args(json!({}))).unwrap());
    assert_eq!(listed.lines().collect::<Vec<_>>(), vec!["mail/index.md", "mail/people/sam.md"]);
    let read =
        agent_tools::knowledge_tool(&config, &agent, "knowledge_read", &args(json!({"path": "mail/people/sam.md"})))
            .unwrap();
    assert!(text(read).contains("Repas"));
    let found =
        agent_tools::knowledge_tool(&config, &agent, "knowledge_search", &args(json!({"words": "repas dimanche"})))
            .unwrap();
    assert_eq!(text(found), "mail/people/sam.md: - 2026-10-18 — Repas chez Sam dimanche");
    let outside =
        agent_tools::knowledge_tool(&config, &agent, "knowledge_read", &args(json!({"path": "../../.env"}))).unwrap();
    assert_eq!(outside["isError"], true);
    let _ = AudioMaker::new; // the type is public for the app
}

#[test]
fn the_tools_version_is_the_pythons() {
    // tools_version() of bridge/bipbridge/agent_tools.py (tests/golden/agent_tools_version.txt), until the Rust bridge's
    // own tools changed: reminders_add's place and `route`, create_page for an infographic and its link to the Carnet
    // (2026-10-22, the Python bridge doesn't have them), start_task, the page kit and map_image (2026-10-22), map_image's
    // live map, line and directions (2026-10-23), upcoming_dates (2026-10-24).
    let expected = include_str!("golden/agent_tools_version.txt").trim();
    assert_eq!(agent_tools::tools_version(), expected);
}
