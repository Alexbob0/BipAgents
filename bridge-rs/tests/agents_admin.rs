//! tests/test_agents_admin.py: agents created, claimed, edited and deleted through the hermes CLI and their profile
//! files (SOUL.md, profile.yaml, config.yaml), and the bridge's config file rewritten.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::Request;
use axum::routing::any;
use axum::Router;
use bipbridge::agents_admin::pyyaml;
use bipbridge::agents_admin::{
    self, claim_profile, create_agent, delete_agent, editable, heal_souls, heal_thinking, managed, read_model,
    read_settings, read_soul, read_thinking, save_agent, set_language, set_model, set_thinking, unclaimed_profiles,
    write_settings, AgentSettings, NewAgent, Run,
};
use bipbridge::config::{load_config, parse_config, Config, SharedConfig};
use bipbridge::souls;
use common::{auth, config_dict, TestApp, Urls};
use serde_json::{json, Value};

type Commands = Arc<Mutex<Vec<Vec<String>>>>;

/// A `run` that records each command and succeeds.
fn recorder() -> (Run, Commands) {
    let commands: Commands = Arc::default();
    let seen = commands.clone();
    let run: Run = Arc::new(move |args: Vec<String>, _env: BTreeMap<String, String>| {
        seen.lock().unwrap().push(args);
        Box::pin(async { Ok(String::new()) })
    });
    (run, commands)
}

type Sessions = Arc<Mutex<Vec<(String, Value)>>>;

/// The hub's gateway: `/v1/capabilities` answers, `/api/sessions` records the « Bot Chat », `/v1/toolsets` lists one.
async fn fake_gateway() -> (String, Sessions) {
    let sessions: Sessions = Arc::default();
    let seen = sessions.clone();
    let url = common::spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = seen.clone();
        async move {
            let path = request.uri().path().to_string();
            let body = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
            if path.ends_with("/v1/capabilities") {
                return (axum::http::StatusCode::OK, axum::Json(json!({"runs": true})));
            }
            if path.ends_with("/v1/toolsets") {
                return (
                    axum::http::StatusCode::OK,
                    axum::Json(
                        json!({"data": [{"name": "web", "label": "Web", "description": "Search", "enabled": true,
                                                 "configured": true, "tools": ["web_search"]}]}),
                    ),
                );
            }
            if path.ends_with("/api/sessions") {
                seen.lock().unwrap().push((path, serde_json::from_slice(&body).unwrap()));
                return (axum::http::StatusCode::CREATED, axum::Json(json!({"id": "api_1", "title": "Bot Chat"})));
            }
            (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({})))
        }
    })))
    .await;
    (url, sessions)
}

/// The `managed` fixture: a bridge managing the person's Hermes (its hub at `hub`), with a config file.
fn managed_config(dir: &Path, hub: &str) -> (SharedConfig, PathBuf) {
    let path = dir.join("bridge.toml");
    std::fs::write(&path, "# bridge config\n").unwrap();
    let mut d = config_dict(dir, &Urls::default());
    d["hermes"] = json!({"bin": "/opt/hermes", "home": dir.join(".hermes").to_string_lossy(),
                         "hub_url": hub, "public_hub_url": "https://mini.ts.net:8642"});
    let config = parse_config(&d, Some(&path.to_string_lossy())).unwrap();
    (SharedConfig::new(config), path)
}

fn new_agent(value: Value) -> NewAgent {
    serde_json::from_value(value).unwrap()
}

fn settings(value: Value) -> AgentSettings {
    serde_json::from_value(value).unwrap()
}

fn profiles(dir: &Path) -> PathBuf {
    dir.join(".hermes").join("profiles")
}

fn reread(path: &Path) -> Config {
    let mut data =
        bipbridge::config::toml_to_json(std::fs::read_to_string(path).unwrap().parse::<toml::Value>().unwrap());
    data["bridge_key"] = json!(common::BRIDGE_KEY);
    data["pocket"] = json!({"url": "http://p"});
    parse_config(&data, None).unwrap()
}

#[tokio::test]
async fn create_custom_agent() {
    let (hub, sessions) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = managed_config(dir.path(), &hub);
    let (run, commands) = recorder();
    let body = new_agent(
        json!({"name": "Fougère", "custom": {"role": "suit mes plantes", "tone": "drôle", "category": "home"}}),
    );
    let agent = create_agent(&config, &body, &reqwest::Client::new(), &run).await.unwrap();
    assert_eq!((agent.name.as_str(), agent.display_name.as_str()), ("fougere", "Fougère"));
    assert_eq!(agent.hermes_url, format!("{hub}/p/fougere"));
    assert_eq!(agent.public_url.as_deref(), Some("https://mini.ts.net:8642/p/fougere"));
    let ran = commands.lock().unwrap().clone();
    assert_eq!(ran[0][..6], ["/opt/hermes", "profile", "create", "fougere", "--clone-from", "default"]);
    assert_eq!(ran[1][..6], ["/opt/hermes", "-p", "fougere", "config", "set", "API_SERVER_KEY"]);
    // Nothing says this install's agents are isolated from the bridge: no terminal, files or code until turned on.
    assert_eq!(
        ran[2],
        [
            "/opt/hermes",
            "-p",
            "fougere",
            "tools",
            "disable",
            "terminal",
            "file",
            "code_execution",
            "browser",
            "--platform",
            "api_server"
        ]
    );
    let soul = std::fs::read_to_string(profiles(dir.path()).join("fougere/SOUL.md")).unwrap();
    assert!(soul.starts_with("Tu es Fougère") && soul.contains("suit mes plantes") && soul.contains("drôle"));
    assert!(std::fs::read_to_string(profiles(dir.path()).join("fougere/profile.yaml"))
        .unwrap()
        .contains("hermes-bots"));
    assert_eq!(*sessions.lock().unwrap(), vec![("/p/fougere/api/sessions".to_string(), json!({"title": "Bot Chat"}))]);
    assert_eq!(config.get().agent(Some("fougere")), Some(&agent));
    // survives a restart
    assert_eq!(reread(&path).agent(Some("fougere")).unwrap().hermes_key, agent.hermes_key);
}

#[tokio::test]
async fn an_agent_created_in_english_thinks_in_english() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &hub);
    let (run, _) = recorder();
    let http = reqwest::Client::new();
    let soul = |name: &str| std::fs::read_to_string(profiles(dir.path()).join(name).join("SOUL.md")).unwrap();
    create_agent(&config, &new_agent(json!({"name": "Max", "kind": "finance", "lang": "en"})), &http, &run)
        .await
        .unwrap();
    assert!(soul("max").starts_with("You are Max, your user's money agent"));
    create_agent(&config, &new_agent(json!({"name": "Hans", "kind": "daily", "lang": "de"})), &http, &run)
        .await
        .unwrap();
    assert!(soul("hans").starts_with("You are Hans"));
    create_agent(
        &config,
        &new_agent(json!({"name": "Ivy", "custom": {"role": "tracks my plants", "category": "home"}, "lang": "en"})),
        &http,
        &run,
    )
    .await
    .unwrap();
    let ivy = soul("ivy");
    assert!(ivy.starts_with("You are Ivy") && ivy.contains("tracks my plants") && ivy.contains("write in English"));
    create_agent(&config, &new_agent(json!({"name": "Léo", "kind": "daily"})), &http, &run).await.unwrap(); // an app from before
    assert!(soul("leo").starts_with("Tu es Léo"));
}

#[tokio::test]
async fn kinds_in_the_apps_language() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let english = app.get("/v1/agent-kinds?lang=en", &[(k, &v)]).await.json();
    let finance = |kinds: &Value| {
        kinds["kinds"].as_array().unwrap().iter().find(|k| k["id"] == "finance").unwrap()["label"].clone()
    };
    assert_eq!(finance(&english), "Money");
    let french = app.get("/v1/agent-kinds", &[(k, &v)]).await.json();
    assert_eq!(finance(&french), "Finances");
}

#[tokio::test]
async fn kinds_route_on_a_bridge_without_hermes() {
    let dir = tempfile::tempdir().unwrap();
    let app = TestApp::new(common::config(dir.path(), &Urls::default()));
    let (k, v) = auth();
    let kinds = app.get("/v1/agent-kinds", &[(k, &v)]).await.json();
    assert_eq!(kinds["can_create"], false); // the test bridge does not manage a Hermes
    let ids: Vec<&str> = kinds["kinds"].as_array().unwrap().iter().map(|k| k["id"].as_str().unwrap()).collect();
    assert!(["daily", "finance", "custom"].iter().all(|id| ids.contains(id)));
    assert_eq!(kinds["kinds"].as_array().unwrap().iter().find(|k| k["id"] == "custom").unwrap()["custom"], true);
    let body = json!({"name": "Léa", "kind": "daily"});
    assert_eq!(app.send_json("POST", "/v1/agents", &[(k, &v)], &body).await.status, 409);
    assert_eq!(app.send_json("POST", "/v1/agents", &[], &body).await.status, 401);
    // pydantic's errors, nested model included
    let bad = app.send_json("POST", "/v1/agents", &[(k, &v)], &json!({"name": "", "custom": {"role": "x"}})).await;
    assert_eq!(bad.status, 422);
    let locs: Vec<Value> = bad.json()["detail"].as_array().unwrap().iter().map(|e| e["loc"].clone()).collect();
    assert_eq!(locs, vec![json!(["body", "name"]), json!(["body", "custom", "role"])]);
}

#[tokio::test]
async fn hermes_in_a_container() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &hub);
    config.update(|c| c.hermes.container_home = "/home/hermes/.hermes".into());
    let (run, _) = recorder();
    let agent =
        create_agent(&config, &new_agent(json!({"name": "Budget", "kind": "finance"})), &reqwest::Client::new(), &run)
            .await
            .unwrap();
    assert_eq!(agent.upload_dir_host.as_deref(), Some(&*profiles(dir.path()).join("budget/uploads").to_string_lossy()));
    assert_eq!(agent.upload_dir_container.as_deref(), Some("/home/hermes/.hermes/profiles/budget/uploads"));
}

#[tokio::test]
async fn refusals() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &hub);
    let (run, _) = recorder();
    let http = reqwest::Client::new();
    for body in [
        json!({"name": "X", "kind": "nope"}),
        json!({"name": "X"}),
        json!({"name": "X", "custom": {"role": "abc", "category": "zzz"}}),
    ] {
        let err = create_agent(&config, &new_agent(body), &http, &run).await.unwrap_err();
        assert!(err.downcast_ref::<agents_admin::AgentError>().is_some());
    }
    let taken =
        create_agent(&config, &new_agent(json!({"name": "Wellness", "kind": "wellness"})), &http, &run).await.unwrap();
    assert_eq!(taken.name, "wellness-2"); // « wellness » already exists in this bridge
}

#[tokio::test]
async fn settings_and_delete() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = managed_config(dir.path(), &hub);
    let (run, commands) = recorder();
    let http = reqwest::Client::new();
    let agent =
        create_agent(&config, &new_agent(json!({"name": "Budget", "kind": "finance"})), &http, &run).await.unwrap();
    let read = read_settings(&config.get(), &agent, &http).await.unwrap();
    write_settings(
        &config,
        &agent,
        &settings(json!({"soul": "Tu es Budget, sobre.", "toolsets": {"web": false, "terminal": true}})),
        &run,
    )
    .await
    .unwrap();
    let err =
        write_settings(&config, &agent, &settings(json!({"toolsets": {"web; rm -rf": true}})), &run).await.unwrap_err();
    assert_eq!(err.downcast_ref::<agents_admin::AgentError>().unwrap().0, "invalid toolset name");
    assert!(read["soul"].as_str().unwrap().starts_with("Tu es Budget") && read["toolsets"][0]["name"] == "web");
    assert_eq!(
        read["toolsets"][0],
        json!({"name": "web", "label": "Web", "description": "Search", "enabled": true, "configured": true})
    );
    let written = std::fs::read_to_string(profiles(dir.path()).join("budget/SOUL.md")).unwrap();
    assert!(written.starts_with("Tu es Budget, sobre.\n") && written.contains("write in French.")); // its language block kept
    let ran = commands.lock().unwrap().clone();
    let cmd = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(ran.contains(&cmd(&[
        "/opt/hermes",
        "-p",
        "budget",
        "tools",
        "enable",
        "terminal",
        "--platform",
        "api_server"
    ])));
    assert!(ran.contains(&cmd(&[
        "/opt/hermes",
        "-p",
        "budget",
        "tools",
        "disable",
        "web",
        "--platform",
        "api_server"
    ])));
    assert!(managed(&config.get(), "wellness").is_err()); // set up by hand: never edited or deleted from the app
    assert!(std::fs::read_to_string(&path).unwrap().contains("[agents.budget]"));
    delete_agent(&config, &agent, &run).await.unwrap();
    let ran = commands.lock().unwrap().clone();
    assert!(ran.contains(&cmd(&["/opt/hermes", "profile", "delete", "budget", "--yes"])));
    assert!(
        !std::fs::read_to_string(&path).unwrap().contains("[agents.budget]")
            && config.get().agent(Some("budget")).is_none()
    );
}

#[tokio::test]
async fn an_isolated_install_keeps_the_agents_tools() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &hub);
    config.update(|c| c.hermes.isolated = true);
    let (run, commands) = recorder();
    create_agent(&config, &new_agent(json!({"name": "Budget", "kind": "finance"})), &reqwest::Client::new(), &run)
        .await
        .unwrap();
    assert!(!commands.lock().unwrap().iter().any(|c| c.iter().any(|a| a == "disable")));
}

#[tokio::test]
async fn profiles_created_on_the_server_can_be_claimed() {
    let (hub, sessions) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = managed_config(dir.path(), &hub);
    let root = profiles(dir.path());
    std::fs::create_dir_all(root.join("cuisine")).unwrap();
    std::fs::write(root.join("cuisine/profile.yaml"), "description: Recettes et menus\n").unwrap();
    std::fs::create_dir(root.join("jardin")).unwrap();
    std::fs::write(root.join("jardin/SOUL.md"), "Tu es Jardin.").unwrap();
    std::fs::write(root.join("jardin/.env"), "API_SERVER_KEY=own-key\n").unwrap();
    std::fs::create_dir(root.join("ancien")).unwrap();
    std::fs::write(root.join("ancien/config.yaml"), "{}").unwrap();
    std::fs::create_dir(root.join(".deleted")).unwrap();
    std::fs::write(root.join(".deleted/ancien"), "deleted").unwrap(); // Hermes' tombstone
    std::fs::create_dir(root.join("logs-only")).unwrap(); // not a profile
    assert_eq!(
        Value::from(unclaimed_profiles(&config.get()).into_iter().map(Value::Object).collect::<Vec<_>>()),
        json!([{"id": "cuisine", "name": "Cuisine", "description": "Recettes et menus"}, {"id": "jardin", "name": "Jardin", "description": ""}])
    );
    let (run, commands) = recorder();
    let http = reqwest::Client::new();
    let jardin = claim_profile(&config, "jardin", Some("Mon jardin"), &http, &run).await.unwrap();
    // Its key kept; the only thing written is the bridge's tools (create_audio) in its profile.
    assert_eq!(jardin.hermes_key, "own-key");
    let keys: std::collections::BTreeSet<String> = commands.lock().unwrap().iter().map(|a| a[5].clone()).collect();
    assert_eq!(
        keys,
        ["mcp_servers.bipagents.url", "mcp_servers.bipagents.headers.Authorization", "mcp_servers.bipagents.timeout"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    );
    commands.lock().unwrap().clear();
    assert_eq!(jardin.display_name, "Mon jardin");
    assert_eq!(jardin.public_url.as_deref(), Some("https://mini.ts.net:8642/p/jardin"));
    let cuisine = claim_profile(&config, "cuisine", None, &http, &run).await.unwrap();
    let ran = commands.lock().unwrap().clone();
    // `--` before the key: a key starting with « - » was read as an option (lab, 2026-10-29)
    assert_eq!(ran[0], ["/opt/hermes", "-p", "cuisine", "config", "set", "API_SERVER_KEY", "--", &cuisine.hermes_key]);
    assert!(ran[1..].iter().all(|a| a[5].starts_with("mcp_servers.bipagents.")));
    let urls: Vec<String> = sessions.lock().unwrap().iter().map(|(u, _)| u.clone()).collect();
    assert_eq!(urls, ["/p/jardin/api/sessions", "/p/cuisine/api/sessions"]); // their « Discussion »
    assert!(std::fs::read_to_string(root.join("cuisine/profile.yaml")).unwrap().contains("hermes-bots")); // Bot Mode
    assert!(
        unclaimed_profiles(&config.get()).is_empty()
            && std::fs::read_to_string(&path).unwrap().contains("[agents.cuisine]")
    );
    let err = claim_profile(&config, "ancien", None, &http, &run).await.unwrap_err();
    assert!(err.downcast_ref::<agents_admin::LookupError>().is_some());
}

#[tokio::test]
async fn agents_route_lists_unclaimed_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    config.hermes.bin = "/opt/hermes".into();
    config.hermes.hub_url = "http://127.0.0.1:8642".into();
    config.hermes.home = dir.path().join(".hermes").to_string_lossy().into_owned();
    std::fs::create_dir_all(dir.path().join(".hermes/profiles/cuisine")).unwrap();
    std::fs::write(dir.path().join(".hermes/profiles/cuisine/SOUL.md"), "Tu es Cuisine.").unwrap();
    let app = TestApp::new(config);
    let (k, v) = auth();
    let listing = app.get("/v1/agents", &[(k, &v)]).await.json();
    assert_eq!(listing["unclaimed"], json!([{"id": "cuisine", "name": "Cuisine", "description": ""}]));
    assert_eq!(app.send_json("POST", "/v1/agents/claim", &[(k, &v)], &json!({"id": "nope"})).await.status, 404);
}

#[tokio::test]
async fn the_language_is_changed_without_touching_the_personality() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &hub);
    let (run, _) = recorder();
    let agent = create_agent(
        &config,
        &new_agent(json!({"name": "Max", "kind": "daily", "lang": "en"})),
        &reqwest::Client::new(),
        &run,
    )
    .await
    .unwrap();
    let c = config.get();
    assert!(read_soul(&c, &agent).unwrap().contains("write in English."));
    assert_eq!(set_language(&c, &agent, "es-ES").unwrap(), "es");
    let soul = read_soul(&c, &agent).unwrap();
    assert!(
        soul.contains("write in Spanish.")
            && soul.starts_with("You are Max")
            && soul.matches(souls::BLOCK_START).count() == 1
    );
    write_settings(&config, &agent, &settings(json!({"soul": "You are Max, blunt and funny."})), &run).await.unwrap();
    let soul = read_soul(&c, &agent).unwrap();
    assert!(soul.starts_with("You are Max, blunt and funny.") && soul.contains("write in Spanish."));
    let (body, lang) = souls::split_language(&soul);
    assert_eq!(lang.as_deref(), Some("es"));
    assert!(!body.contains(souls::BLOCK_START));
}

fn load(path: &Path) -> Value {
    pyyaml::to_json(&pyyaml::load(&std::fs::read_to_string(path).unwrap()).unwrap())
}

#[tokio::test]
async fn thinking_level_is_set_in_the_agents_config() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let c = config.get();
    let agent = c.agents.values().next().unwrap().clone();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.yaml"), "model:\n  provider: openrouter\n  default: x\n").unwrap();
    assert_eq!(read_thinking(&c, &agent), None);
    assert!(set_thinking(&c, &agent, "high").unwrap_err().downcast_ref::<agents_admin::LookupError>().is_some());
    std::fs::write(
        folder.join("config.yaml"),
        "model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\nagent:\n  max_turns: 60\n",
    )
    .unwrap();
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("medium"));
    assert_eq!(set_thinking(&c, &agent, "xhigh").unwrap(), "xhigh");
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("xhigh"));
    let data = load(&folder.join("config.yaml"));
    assert!(data.get("providers").is_none()); // never enable_thinking on: it would win over a turn asking for none (Live)
    assert_eq!(data["agent"], json!({"max_turns": 60, "reasoning_effort": "xhigh"}));
    assert_eq!(data["model"]["default"], "glm");
    // the same bytes as PyYAML's safe_dump(allow_unicode=True, sort_keys=False)
    assert_eq!(
        std::fs::read_to_string(folder.join("config.yaml")).unwrap(),
        "model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\nagent:\n  max_turns: 60\n  reasoning_effort: xhigh\n"
    );
    set_thinking(&c, &agent, "none").unwrap();
    let data = load(&folder.join("config.yaml"));
    assert_eq!(data["agent"]["reasoning_effort"], "none");
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none"));
    assert_eq!(
        data["providers"]["bipagents-thinking"],
        json!({"api": "http://gpu:8888/v1", "extra_body": {"chat_template_kwargs": {"enable_thinking": false}}})
    );
    set_thinking(&c, &agent, "medium").unwrap();
    assert!(load(&folder.join("config.yaml")).get("providers").is_none());
    assert!(set_thinking(&c, &agent, "ultra").is_err());
}

#[tokio::test]
async fn an_agent_set_up_by_hand_gets_its_personality_and_thinking_changed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home-vie");
    std::fs::create_dir_all(home.join("profiles/vie")).unwrap();
    std::fs::write(home.join("profiles/vie/SOUL.md"), "Tu es Vie.\n").unwrap();
    std::fs::write(
        home.join("profiles/vie/config.yaml"),
        "model:\n  provider: custom\n  default: local-model\n  base_url: http://gpu:8888/v1\nagent:\n  reasoning_effort: medium\n",
    )
    .unwrap();
    let mut d = config_dict(dir.path(), &Urls::default());
    d["agents"]["vie"] = json!({"display_name": "Vie", "hermes_url": "http://127.0.0.1:8644", "hermes_key": "k".repeat(32),
                                "isolated": true, "host_home": home.to_string_lossy()});
    let config = SharedConfig::new(parse_config(&d, None).unwrap());
    let c = config.get();
    let agent = editable(&c, "vie").unwrap();
    assert!(managed(&c, "vie").is_err()); // no deleting, no space, no services from the app
    assert!(editable(&c, "wellness").is_err()); // set up by hand without its home: nothing changes
                                                // its gateway isn't asked for its tools (nothing listens there)
    let read = read_settings(&c, &agent, &reqwest::Client::new()).await.unwrap();
    assert_eq!(read["soul"], "Tu es Vie.");
    assert_eq!(read["by_hand"], true);
    assert_eq!(read["thinking"], "medium");
    set_language(&c, &agent, "fr").unwrap();
    let (run, _) = recorder();
    write_settings(&config, &agent, &settings(json!({"soul": "Tu es Vie, brève."})), &run).await.unwrap();
    let soul = read_soul(&c, &agent).unwrap();
    assert!(soul.starts_with("Tu es Vie, brève.") && soul.contains("French"));
    assert!(write_settings(&config, &agent, &settings(json!({"toolsets": {"web": false}})), &run).await.is_err());
    assert_eq!(set_thinking(&c, &agent, "high").unwrap(), "high");
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("high"));
}

#[test]
fn rewriting_an_agents_section_keeps_what_the_bridge_does_not_set() {
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("vie.key");
    std::fs::write(&key_file, "k".repeat(32)).unwrap();
    let path = dir.path().join("bridge.toml");
    std::fs::write(
        &path,
        format!(
            "bridge_key = \"{}\"\n\n[pocket]\nurl = \"http://p\"\n\n[agents.vie]\ndisplay_name = \"Vie\"\nhermes_url = \"http://127.0.0.1:9212/p/vie\"\nhermes_key_file = \"{}\"\n# her voice\nvoice = \"pocket:colibri\"\nntfy_topic = \"vie-out\"\nshares = \"external\"\n\n[agents.wellness]\ndisplay_name = \"Wellness\"\nhermes_url = \"http://127.0.0.1:9212/p/wellness\"\nhermes_key = \"{}\"\n",
            "b".repeat(40),
            key_file.to_string_lossy(),
            "w".repeat(32)
        ),
    )
    .unwrap();
    let config = SharedConfig::new(load_config(Some(&path.to_string_lossy())).unwrap());
    let mut moved = config.get().agent(Some("vie")).unwrap().clone();
    moved.hermes_url = "http://127.0.0.1:8660/p/vie".into();
    moved.space = Some("/s/vie".into());
    moved.isolated = true;
    save_agent(&config, &moved).unwrap();
    let again = load_config(Some(&path.to_string_lossy())).unwrap();
    let vie = again.agent(Some("vie")).unwrap();
    assert_eq!(vie.voice.as_deref(), Some("pocket:colibri"));
    assert_eq!(vie.ntfy_topic.as_deref(), Some("vie-out"));
    assert_eq!(vie.shares, "external");
    assert!(
        vie.space.as_deref() == Some("/s/vie") && vie.isolated && vie.hermes_url.starts_with("http://127.0.0.1:8660")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(vie.hermes_key, "k".repeat(32));
    assert!(!text.split("[agents.vie]").nth(1).unwrap().split("[agents").next().unwrap().contains("hermes_key = "));
    assert_eq!(text.matches("hermes_url").count(), 2);
    assert_eq!(again.agent(Some("wellness")).unwrap().hermes_key, "w".repeat(32));
}

/// The first agent, served by the hub (the bridge manages it).
fn hub_agent(config: &SharedConfig) -> bipbridge::config::AgentConfig {
    let mut agent = config.get().agents.values().next().unwrap().clone();
    agent.hermes_url = format!("http://127.0.0.1:8642/p/{}", agent.name);
    config.put_agent(agent.clone());
    agent
}

#[test]
fn thinking_forced_on_by_an_earlier_bridge_is_healed() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(
        folder.join("config.yaml"),
        "model:\n  provider: custom\n  base_url: http://gpu:8888/v1\nagent:\n  reasoning_effort: xhigh\nproviders:\n  bipagents-thinking:\n    api: http://gpu:8888/v1\n    extra_body:\n      chat_template_kwargs:\n        enable_thinking: true\n",
    )
    .unwrap();
    heal_thinking(&config.get());
    assert!(load(&folder.join("config.yaml")).get("providers").is_none());
    assert_eq!(read_thinking(&config.get(), &agent).as_deref(), Some("xhigh"));
}

const GLM_EU: &str =
    "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\nagent:\n  max_turns: 60\n";

#[test]
fn glm_through_eurouter_thinks_low_and_never_none() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let c = config.get();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("config.yaml");
    let mark = folder.join(agents_admin::THINKING_OFF_MARK);
    // nothing chosen: « low » shown, and written at start
    std::fs::write(&path, GLM_EU).unwrap();
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("low"));
    heal_thinking(&c);
    let data = load(&path);
    assert_eq!(data["agent"], json!({"max_turns": 60, "reasoning_effort": "low"}));
    assert_eq!(data["providers"]["bipagents-route"]["extra_body"]["provider"]["allow_fallbacks"], true);
    // « none »: written « low », the person's choice kept for the app, no enable_thinking entry
    assert_eq!(agents_admin::choose_thinking(&c, &agent, "none").unwrap(), "none");
    let data = load(&path);
    assert_eq!(data["agent"]["reasoning_effort"], "low");
    assert!(data["providers"].get("bipagents-thinking").is_none());
    assert!(mark.exists());
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none"));
    assert_eq!(agents_admin::quick_turn_options(&c, &agent), json!({"reasoning": {"enabled": true, "effort": "low"}}));
    assert_eq!(
        agents_admin::task_turn_options(&c, &agent),
        Some(json!({"reasoning": {"enabled": true, "effort": "medium"}}))
    );
    // a level the person picks: theirs, kept at start; tasks follow it from « medium » up
    assert_eq!(agents_admin::choose_thinking(&c, &agent, "medium").unwrap(), "medium");
    assert!(!mark.exists());
    heal_thinking(&c);
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "medium");
    assert_eq!(agents_admin::task_turn_options(&c, &agent), None);
    // switched to DeepSeek with « none » chosen: really off there (DeepSeek follows it), and back to GLM: « low »
    agents_admin::choose_thinking(&c, &agent, "none").unwrap();
    set_model(&c, &agent, "deepseek/deepseek-v4.1-flash", None).unwrap();
    let data = load(&path);
    assert_eq!(data["agent"]["reasoning_effort"], "none");
    assert!(!mark.exists());
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none"));
    assert_eq!(agents_admin::quick_turn_options(&c, &agent), json!({"reasoning": {"enabled": false}}));
    assert_eq!(agents_admin::task_turn_options(&c, &agent), None);
    set_model(&c, &agent, "glm-5.3-flash", None).unwrap();
    let data = load(&path);
    assert_eq!(data["agent"]["reasoning_effort"], "low");
    assert!(data["providers"].get("bipagents-thinking").is_none());
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none"));
}

/// Real-day bench 2026-10-28: Hermes' config template writes `reasoning_effort: medium` in every new profile, which
/// the bridge took for the person's choice; and « Prof », set to « low » while a claim was still making it, was back
/// to « medium » at task time (Hermes' own `config set` commands rewrite the file meanwhile).
#[test]
fn hermes_template_medium_is_no_choice_and_a_lost_choice_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let c = config.get();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("config.yaml");
    let template = format!("{GLM_EU}  reasoning_effort: medium\n");
    // a fresh profile (or an existing place, the migration): « low », shown and written
    std::fs::write(&path, &template).unwrap();
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("low"));
    assert!(agents_admin::heal_thinking_of(&c, &agent));
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "low");
    assert!(!agents_admin::heal_thinking_of(&c, &agent)); // nothing more to do
    // the person sets « low », then a rewrite brings the template back: put back, and at each start too
    agents_admin::choose_thinking(&c, &agent, "low").unwrap();
    assert_eq!(std::fs::read_to_string(folder.join(agents_admin::THINKING_CHOSEN_MARK)).unwrap().trim(), "low");
    std::fs::write(&path, &template).unwrap();
    assert!(agents_admin::heal_thinking_of(&c, &agent));
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "low");
    // « medium » the person chose stays, and comes back after a rewrite to « low »; background tasks follow it
    agents_admin::choose_thinking(&c, &agent, "medium").unwrap();
    heal_thinking(&c);
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "medium");
    std::fs::write(&path, format!("{GLM_EU}  reasoning_effort: low\n")).unwrap();
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("medium"));
    heal_thinking(&c);
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "medium");
    // the routing written again (each time the bridge gives the tools) keeps the person's level
    std::fs::write(&path, template.replace("medium", "low")).unwrap();
    bipbridge::model_route::write(&path, &[]).unwrap();
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "medium");
    // another model: the person's level put back too
    std::fs::write(&path, "model:\n  provider: custom\n  default: local\n  base_url: http://gpu:8888/v1\n").unwrap();
    agents_admin::choose_thinking(&c, &agent, "high").unwrap();
    std::fs::write(&path, "model:\n  provider: custom\n  default: local\n  base_url: http://gpu:8888/v1\nagent:\n  reasoning_effort: medium\n").unwrap();
    assert!(agents_admin::heal_thinking_of(&c, &agent));
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "high");
}

/// The offer (model fixed, nothing chosen): a claim's model files give GLM « low » in the hub every agent is cloned
/// from and in the agent made before the claim.
#[test]
fn a_claims_model_files_give_glm_low_over_the_template() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.yaml"), "agent:\n  reasoning_effort: medium\n  max_turns: 60\n").unwrap();
    let model = bipbridge::cloud_setup::ModelChoice {
        provider: "custom".into(),
        name: "glm-5.3-flash".into(),
        base_url: Some("https://api.eurouter.ai/api/v1".into()),
        key: "k".into(),
        vision: true,
    };
    bipbridge::cloud_setup::write_model_files(dir.path(), &model, &[]).unwrap();
    assert_eq!(load(&dir.path().join("config.yaml"))["agent"]["reasoning_effort"], "low");
}

#[test]
fn glm_none_written_by_an_earlier_bridge_is_healed_to_low() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let c = config.get();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join("config.yaml");
    std::fs::write(
        &path,
        format!("{GLM_EU}  reasoning_effort: none\nproviders:\n  bipagents-thinking:\n    api: https://api.eurouter.ai/api/v1\n    extra_body:\n      chat_template_kwargs:\n        enable_thinking: false\n"),
    )
    .unwrap();
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none"));
    heal_thinking(&c);
    let data = load(&path);
    assert_eq!(data["agent"]["reasoning_effort"], "low");
    assert!(data["providers"].get("bipagents-thinking").is_none());
    assert_eq!(read_thinking(&c, &agent).as_deref(), Some("none")); // the app still shows the person's choice
    // a self-hosted server keeps « none » and its enable_thinking entry
    std::fs::write(&path, "model:\n  provider: custom\n  default: glm-5.3\n  base_url: http://sparks:8000/v1\nagent:\n  reasoning_effort: none\n").unwrap();
    std::fs::remove_file(folder.join(agents_admin::THINKING_OFF_MARK)).unwrap();
    heal_thinking(&c);
    assert_eq!(load(&path)["agent"]["reasoning_effort"], "none");
    assert_eq!(agents_admin::quick_turn_options(&c, &agent), json!({"reasoning": {"enabled": false}}));
}

#[test]
fn an_earlier_bridge_block_gets_the_rule_about_what_agents_read() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let c = config.get();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    let old = format!(
        "Tu es Vie, ma personnalité à moi.\n\n{}\nLanguage: answer in the language your person writes in. When you write first (a routine, a report, a reminder), write in French.\n{}\n",
        souls::BLOCK_START,
        souls::BLOCK_END
    );
    std::fs::write(folder.join("SOUL.md"), old).unwrap();
    heal_souls(&c);
    let soul = read_soul(&c, &agent).unwrap();
    assert!(soul.starts_with("Tu es Vie, ma personnalité à moi.") && soul.contains(souls::CONTENT_RULE));
    assert!(soul.contains("write in French.") && soul.matches(souls::BLOCK_START).count() == 1);
    assert_eq!(soul.matches(souls::STYLE_RULE).count(), 1); // short answers (2026-10-30), without recreating it
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), soul); // once
                                                      // A personality written by hand gets the block too (owner 2026-10-22: every agent talks plainly), in the place's
                                                      // language; the person's words are kept.
    std::fs::write(folder.join("SOUL.md"), "Fait à la main, sans bloc.\n").unwrap();
    heal_souls(&c);
    let healed = read_soul(&c, &agent).unwrap();
    assert!(healed.starts_with("Fait à la main, sans bloc.\n\n") && healed.contains(souls::PLAIN_RULE));
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), healed);
    std::fs::write(folder.join("SOUL.md"), "").unwrap();
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), ""); // none at all: Hermes' own stays
}

#[test]
fn healing_tells_an_agent_whether_its_memory_is_shared() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let agent = hub_agent(&config);
    let c = config.get();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("SOUL.md"), format!("Tu es Vie.\n\n{}\n", souls::language_block(Some("fr")))).unwrap();
    // in the circle (the Carnet on by default): what it remembers of the person is common, and it knows it
    heal_souls(&c);
    let soul = read_soul(&c, &agent).unwrap();
    assert_eq!(soul.matches(souls::SHARED_MEMORY_RULE).count(), 1, "{soul}");
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), soul, "once");
    assert!(!agents_admin::editable_soul(&c, &agent).unwrap().contains(souls::SHARED_MEMORY_START), "never shown");
    // the health given to it: the variant (what it learns stays its own)
    bipbridge::health::set_enabled(&c, &agent.name, true).unwrap();
    heal_souls(&c);
    let soul = read_soul(&c, &agent).unwrap();
    assert!(soul.contains(souls::SHARED_MEMORY_HEALTH_RULE) && !soul.contains(souls::SHARED_MEMORY_RULE));
    // the Carnet off: out of the circle, the block goes
    bipbridge::notebook::set_enabled(&c, &agent.name, false).unwrap();
    heal_souls(&c);
    let soul = read_soul(&c, &agent).unwrap();
    assert!(!soul.contains(souls::SHARED_MEMORY_START) && soul.starts_with("Tu es Vie.\n\n"), "{soul}");
}

#[test]
fn the_agents_model_is_switched_in_its_config() {
    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), "http://127.0.0.1:8642");
    let c = config.get();
    let agent = c.agents.values().next().unwrap().clone();
    let folder = profiles(dir.path()).join(&agent.name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.yaml"), "model:\n  provider: anthropic\n").unwrap();
    assert_eq!(read_model(&c, &agent), None);
    assert!(set_model(&c, &agent, "glm-5.3-flash", Some(true)).is_err());
    std::fs::write(
        folder.join("config.yaml"),
        "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n  supports_vision: true\nagent:\n  max_turns: 60\n",
    )
    .unwrap();
    assert_eq!(read_model(&c, &agent), Some(json!({"name": "glm-5.3-flash", "vision": true})));
    assert_eq!(
        set_model(&c, &agent, "deepseek-v4-flash-0731", Some(false)).unwrap(),
        json!({"name": "deepseek-v4-flash-0731", "vision": false})
    );
    let data = load(&folder.join("config.yaml"));
    assert_eq!(data["model"]["base_url"], "https://api.eurouter.ai/api/v1");
    assert_eq!(data["agent"], json!({"max_turns": 60}));
    assert_eq!(set_model(&c, &agent, "minimax-m3", None).unwrap()["vision"], false);
    // vision unsaid: kept
}

#[tokio::test]
async fn routes_for_settings_thinking_and_model() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    config.hermes.bin = "/opt/hermes".into();
    config.hermes.hub_url = hub.clone();
    config.hermes.home = dir.path().join(".hermes").to_string_lossy().into_owned();
    let mut agent = config.agents["wellness"].clone();
    agent.hermes_url = format!("{hub}/p/wellness");
    config.agents.insert("wellness".into(), agent);
    let folder = profiles(dir.path()).join("wellness");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("SOUL.md"), souls::with_language("Tu es Wellness.\n", Some("fr"))).unwrap();
    std::fs::write(
        folder.join("config.yaml"),
        "model:\n  provider: custom\n  default: glm\n  base_url: http://gpu/v1\n",
    )
    .unwrap();
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let got = app.get("/v1/agents/wellness/settings", &h).await;
    assert_eq!(got.status, 200);
    let keys: Vec<String> = got.json().as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        [
            "id",
            "name",
            "soul",
            "language",
            "thinking",
            "model",
            "toolsets",
            "isolated",
            "protected",
            "services",
            "skills",
            "browser",
            "health",
            "weather",
            "notebook",
            "guide",
            "answer_style",
            "group"
        ]
    );
    assert_eq!(got.json()["answer_style"], json!({"style": "short", "custom": "", "custom_max": 300}));
    assert_eq!(got.json()["soul"], "Tu es Wellness.");
    assert_eq!(got.json()["language"], "fr");
    assert_eq!(got.json()["health"], json!({"on": false, "categories": []}));
    // « Santé » on: the rule goes in its personality, which the person still sees as they wrote it.
    let on = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &json!({"health": true})).await.json();
    assert_eq!(
        (&on["health"], &on["soul"]),
        (&json!({"on": true, "categories": ["activity", "sleep", "heart"]}), &json!("Tu es Wellness."))
    );
    // Its categories: the known ones, in their order; an unknown one refused.
    let chosen = json!({"health_categories": ["glucose", "activity", "glucose"]});
    let chosen = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &chosen).await.json();
    assert_eq!(chosen["health"], json!({"on": true, "categories": ["activity", "glucose"]}));
    let odd = json!({"health_categories": ["activity", "dreams"]});
    let odd = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &odd).await;
    assert_eq!(odd.status, 422);
    assert_eq!(odd.json()["detail"][0]["loc"], json!(["body", "health_categories", 1]));
    assert!(std::fs::read_to_string(folder.join("SOUL.md")).unwrap().contains(souls::HEALTH_RULE));
    let edited = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &json!({"soul": "Tu es Vie."})).await;
    assert_eq!(edited.json()["soul"], "Tu es Vie.");
    let soul = std::fs::read_to_string(folder.join("SOUL.md")).unwrap();
    assert!(soul.contains(souls::HEALTH_RULE) && soul.contains("write in French"));
    std::fs::write(folder.join("SOUL.md"), souls::with_language("Tu es Wellness.\n", Some("fr"))).unwrap();
    bipbridge::health::set_enabled(&config, "wellness", false).unwrap();
    // « Météo »: the weather tool, nothing in its personality.
    assert_eq!(got.json()["weather"], json!({"on": false}));
    let on = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &json!({"weather": true})).await.json();
    assert_eq!(on["weather"], json!({"on": true}));
    assert!(bipbridge::weather::enabled(&config, "wellness"));
    let off = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &json!({"weather": false})).await.json();
    assert_eq!(off["weather"], json!({"on": false}));
    let off = json!({"health_categories": ["activity"]});
    let off = app.send_json("PATCH", "/v1/agents/wellness/settings", &h, &off).await;
    assert_eq!(off.status, 400, "{}", off.text()); // its switch off: nothing to choose
    assert_eq!(
        app.get("/v1/agents/nobody/settings", &h).await.json(),
        json!({"detail": "not an agent this install manages"})
    );
    assert_eq!(
        app.get("/v1/agents/wellness/thinking", &h).await.json(),
        json!({"id": "wellness", "thinking": "medium"})
    );
    let bad = app.send_json("PUT", "/v1/agents/wellness/thinking", &h, &json!({"level": "ultra"})).await;
    assert_eq!(bad.status, 422);
    assert_eq!(bad.json()["detail"][0]["type"], "string_pattern_mismatch");
    let put = app.send_json("PUT", "/v1/agents/wellness/thinking", &h, &json!({"level": "low"})).await;
    assert_eq!(put.json(), json!({"id": "wellness", "thinking": "low"}));
    let model =
        app.send_json("PUT", "/v1/agents/wellness/model", &h, &json!({"name": "minimax-m3", "vision": "yes"})).await;
    assert_eq!(model.json(), json!({"id": "wellness", "model": {"name": "minimax-m3", "vision": true}}));
    let lang = app.send_json("PUT", "/v1/agents/wellness/language", &h, &json!({"lang": "en-GB"})).await;
    assert_eq!(lang.json(), json!({"id": "wellness", "language": "en"}));
    assert_eq!(app.send_json("PUT", "/v1/agents/wellness/language", &h, &json!({"lang": "e"})).await.status, 422);
    let catalog = app.get("/v1/tool-catalog", &h).await.json();
    assert!(!catalog["services"].as_array().unwrap().is_empty());
    let shares = app.get("/v1/agent-shares", &h).await.json();
    assert_eq!(shares["agents"][0]["id"], "wellness");
    let links = app.get("/v1/agent-links", &h).await.json();
    assert_eq!(links, json!({"agents": [{"id": "wellness", "name": "Wellness", "managed": true}], "links": []}));
    assert_eq!(app.request("DELETE", "/v1/agents/claim", &h, None).await.status, 404);
}

#[tokio::test]
async fn the_answer_style_is_kept_and_switches_one_paragraph_of_the_soul() {
    // Alex 2026-10-30: short by default, balanced, detailed or the person's own style, per agent
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    config.hermes.bin = "/opt/hermes".into();
    config.hermes.hub_url = hub.clone();
    config.hermes.home = dir.path().join(".hermes").to_string_lossy().into_owned();
    let source = dir.path().join("bridge.toml");
    std::fs::write(&source, "[agents.wellness]\ndisplay_name = \"Wellness\"\n").unwrap();
    config.source_path = Some(source.to_string_lossy().into_owned());
    let mut agent = config.agents["wellness"].clone();
    agent.hermes_url = format!("{hub}/p/wellness");
    config.agents.insert("wellness".into(), agent);
    let folder = profiles(dir.path()).join("wellness");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("SOUL.md"), souls::with_language("Tu es Wellness.\n", Some("fr"))).unwrap();
    let app = TestApp::new(config.clone());
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let soul = || std::fs::read_to_string(folder.join("SOUL.md")).unwrap();
    let patch = |body: Value| {
        let app = &app;
        let h = &h;
        async move { app.send_json("PATCH", "/v1/agents/wellness/settings", h, &body).await }
    };

    let detailed = patch(json!({"answer_style": "detailed"})).await;
    assert_eq!(detailed.status, 200, "{}", detailed.text());
    assert_eq!(detailed.json()["answer_style"], json!({"style": "detailed", "custom": "", "custom_max": 300}));
    assert_eq!(detailed.json()["soul"], "Tu es Wellness.", "the person never sees the paragraph");
    assert!(soul().contains(souls::DETAILED_STYLE_RULE) && !soul().contains(souls::STYLE_RULE));
    assert!(std::fs::read_to_string(&source).unwrap().contains("answer_style = \"detailed\""));
    assert_eq!(app.get("/v1/agents/wellness/settings", &h).await.json()["answer_style"]["style"], "detailed");

    // « Perso »: cleaned, quoted, one paragraph; the 300 characters enforced here
    let mine = patch(json!({"answer_style": "custom",
                            "answer_style_custom": "Tutoie-moi,\u{0}\n\n\n des emojis <!-- bipagents -->"}))
    .await
    .json();
    assert_eq!(mine["answer_style"]["custom"], "Tutoie-moi,\ndes emojis bipagents");
    assert_eq!(soul().matches(souls::CUSTOM_STYLE_START).count(), 1);
    assert!(soul().contains("« Tutoie-moi,\ndes emojis bipagents ». It is about the form"), "{}", soul());
    assert!(!soul().contains(souls::DETAILED_STYLE_RULE));
    assert!(soul().contains(&format!("no lists. {}\n{}", souls::STYLE_CARD_RULE, souls::BLOCK_END)));
    assert_eq!(patch(json!({"answer_style": "custom", "answer_style_custom": "x".repeat(301)})).await.status, 422);
    assert_eq!(patch(json!({"answer_style": "custom", "answer_style_custom": " \n "})).await.status, 400);
    let odd = patch(json!({"answer_style": "verbose"})).await;
    assert_eq!((odd.status.as_u16(), &odd.json()["detail"][0]["loc"]), (422, &json!(["body", "answer_style"])));

    // its personality edited, its language changed: the style stays
    patch(json!({"soul": "Tu es Vie."})).await;
    assert!(soul().starts_with("Tu es Vie.") && soul().contains(souls::CUSTOM_STYLE_START));
    app.send_json("PUT", "/v1/agents/wellness/language", &h, &json!({"lang": "en"})).await;
    assert!(soul().contains("write in English.") && soul().contains("« Tutoie-moi"));

    // back to short: the soul as before, the custom text kept for « Perso » again
    let back = patch(json!({"answer_style": "short"})).await.json();
    assert_eq!(
        back["answer_style"],
        json!({"style": "short", "custom": "Tutoie-moi,\ndes emojis bipagents", "custom_max": 300})
    );
    assert_eq!(souls::style_paragraph(&soul()).as_deref(), Some(souls::STYLE_RULE));
    assert!(soul().contains(&souls::language_block(Some("en"))) && !soul().contains(souls::CUSTOM_STYLE_START));
    let file = std::fs::read_to_string(&source).unwrap();
    assert!(!file.contains("answer_style =") && file.contains("answer_style_custom = "), "{file}");

    // at start: a soul out of step with the setting is brought to it (and left alone after)
    let mut kept = app.app.state.config().agents["wellness"].clone();
    kept.answer_style = Some("balanced".into());
    app.app.state.config.put_agent(kept);
    heal_souls(&app.app.state.config());
    assert!(soul().contains(souls::BALANCED_STYLE_RULE) && !soul().contains(souls::STYLE_RULE));
    assert_eq!(soul().matches("Answer first").count(), 1);
    let healed = soul();
    heal_souls(&app.app.state.config());
    assert_eq!(soul(), healed);

    // « Sois plus détaillé »: a card, applied on the tap only
    let c = app.app.state.config();
    let agent = c.agents["wellness"].clone();
    let tools = bipbridge::guide::tools(&c, &agent);
    let style_tool = tools.iter().find(|t| t["name"] == bipbridge::guide::PROPOSE_PROFILE).expect("propose_profile");
    assert_eq!(style_tool["inputSchema"]["required"], json!(["answer_style"]));
    let args = |s: &str| json!({"answer_style": s}).as_object().unwrap().clone();
    let same = bipbridge::guide::call(&c, &agent, bipbridge::guide::PROPOSE_PROFILE, &args("balanced")).unwrap();
    assert_eq!(same["isError"], true, "already its style");
    let card = bipbridge::guide::call(&c, &agent, bipbridge::guide::PROPOSE_PROFILE, &args("detailed")).unwrap();
    assert_eq!(card["isError"], false, "{card:?}");
    assert!(soul().contains(souls::BALANCED_STYLE_RULE), "nothing before the tap");
    let view = app.get("/v1/guide", &h).await.json();
    let proposal = view["proposals"].as_array().unwrap().iter().find(|p| p["type"] == "style").unwrap().clone();
    assert_eq!((&proposal["answer_style"], &proposal["agent"]), (&json!("detailed"), &json!("wellness")));
    let id = proposal["id"].as_str().unwrap();
    let done = app.request("POST", &format!("/v1/guide/proposals/{id}/accept"), &h, None).await;
    assert_eq!(done.status, 200, "{}", done.text());
    assert_eq!(done.json()["type"], "style");
    assert_eq!(done.json()["answer_style"]["style"], "detailed");
    assert!(soul().contains(souls::DETAILED_STYLE_RULE) && !soul().contains(souls::BALANCED_STYLE_RULE));
    assert_eq!(app.app.state.config().agents["wellness"].answer_style.as_deref(), Some("detailed"));
}

/// The place's agents at most (Alex, 2026-10-30, « sans le dire »): 20 on a place of the offer unless its bridge.toml
/// says otherwise, none on a self-install; only new agents are refused, nothing is told before.
#[test]
fn the_place_limit_is_twenty_on_the_offer_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    assert_eq!(config.place_agents_max, None);
    assert_eq!(agents_admin::agents_max_on(&config, true), Some(agents_admin::OFFER_AGENTS_MAX));
    assert_eq!(agents_admin::OFFER_AGENTS_MAX, 20);
    assert_eq!(agents_admin::agents_max_on(&config, false), None); // a self-install: no limit
    assert!(agents_admin::room_for_on(&config, 1000, false));
    config.place_agents_max = Some(0); // written as 0: none, even on the offer
    assert_eq!(agents_admin::agents_max_on(&config, true), None);
    config.place_agents_max = Some(3);
    assert_eq!(
        (agents_admin::agents_max_on(&config, true), agents_admin::agents_max_on(&config, false)),
        (Some(3), Some(3))
    );
    // every agent the place runs counts (here: wellness)
    assert_eq!(agents_admin::agents_count(&config), 1);
    assert!(agents_admin::room_for_on(&config, 2, true) && !agents_admin::room_for_on(&config, 3, true));
    // at 20 on the offer: the 21st is refused
    config.place_agents_max = None;
    for n in 0..19 {
        let name = format!("a{n}");
        config.agents.insert(name.clone(), bipbridge::config::AgentConfig { name, ..Default::default() });
    }
    assert!(!agents_admin::room_for_on(&config, 1, true) && agents_admin::room_for_on(&config, 1, false));
}

fn capped(config: &SharedConfig, most: usize) {
    config.update(|c| c.place_agents_max = Some(most));
}

#[tokio::test]
async fn a_new_agent_past_the_limit_is_refused_and_nothing_is_made() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let (config, path) = managed_config(dir.path(), &hub);
    let (run, commands) = recorder();
    let http = reqwest::Client::new();
    let had = config.get().agents.len();
    capped(&config, had + 1);
    let body = new_agent(json!({"name": "Coach", "kind": "wellness"}));
    create_agent(&config, &body, &http, &run).await.unwrap();
    commands.lock().unwrap().clear();
    let err =
        create_agent(&config, &new_agent(json!({"name": "Chef", "kind": "home"})), &http, &run).await.unwrap_err();
    assert!(agents_admin::too_many(&err), "{err:#}");
    assert!(commands.lock().unwrap().is_empty(), "no profile made");
    assert_eq!(config.get().agents.len(), had + 1);
    assert!(!profiles(dir.path()).join("chef").exists() && !std::fs::read_to_string(&path).unwrap().contains("chef"));
    // a limit lowered below the agents there: none removed, new ones still refused
    capped(&config, 1);
    assert_eq!(config.get().agents.len(), had + 1);
    assert!(agents_admin::too_many(&create_agent(&config, &body, &http, &run).await.unwrap_err()));
    // a profile made on the server and claimed: an agent too
    std::fs::create_dir_all(profiles(dir.path()).join("jardin")).unwrap();
    std::fs::write(profiles(dir.path()).join("jardin/SOUL.md"), "Tu es Jardin.").unwrap();
    assert!(agents_admin::too_many(&claim_profile(&config, "jardin", None, &http, &run).await.unwrap_err()));
    // no limit (0): made again
    capped(&config, 0);
    create_agent(&config, &new_agent(json!({"name": "Chef", "kind": "home"})), &http, &run).await.unwrap();
}

/// Each way an agent is made answers the same plain refusal (409, a code, no number); the model's tools refuse
/// before any card.
#[tokio::test]
async fn every_way_to_make_an_agent_refuses_past_the_limit() {
    let (hub, _) = fake_gateway().await;
    let dir = tempfile::tempdir().unwrap();
    let mut config = common::config(dir.path(), &Urls::default());
    config.hermes.bin = "/opt/hermes".into();
    config.hermes.hub_url = hub.clone();
    config.hermes.home = dir.path().join(".hermes").to_string_lossy().into_owned();
    let mut wellness = config.agents["wellness"].clone();
    wellness.hermes_url = format!("{hub}/p/wellness");
    wellness.host_home = Some(dir.path().to_string_lossy().into_owned());
    config.agents.insert("wellness".into(), wellness);
    config.place_agents_max = Some(1); // wellness is there: full
    let app = TestApp::new(config);
    let (k, v) = auth();
    let h = [(k, v.as_str())];
    let refused = json!({"detail": {"code": agents_admin::TOO_MANY_AGENTS}});
    std::fs::create_dir_all(profiles(dir.path()).join("jardin")).unwrap();
    std::fs::write(profiles(dir.path()).join("jardin/SOUL.md"), "Tu es Jardin.").unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let in_ten_days = now + 10.0 * 86400.0;
    let routes: [(&str, Value); 5] = [
        ("/v1/agents", json!({"name": "Chef", "kind": "home"})),
        ("/v1/agents/claim", json!({"id": "jardin"})),
        ("/v1/project-agents", json!({"name": "Voyage", "summary": "Le voyage à Rome", "ends": in_ten_days})),
        ("/v1/family-agent", json!({"name": "Famille"})),
        ("/v1/assistant-import/apply", json!({"agent": "", "name": "Grok", "soul": "Tu aides.", "source": "Grok"})),
    ];
    for (path, body) in routes {
        let got = app.send_json("POST", path, &h, &body).await;
        assert_eq!((got.status.as_u16(), got.json()), (409, refused.clone()), "{path}: {}", got.text());
    }
    assert_eq!(app.app.state.config().agents.len(), 1);

    // the model's tools: an error for it, and no card
    let c = app.app.state.config();
    let agent = c.agents["wellness"].clone();
    bipbridge::guide::change(&c, |s| s.guide = Some("wellness".into())).unwrap();
    let args = |v: Value| v.as_object().unwrap().clone();
    let member = |name: &str| {
        json!({"name": name, "kind": "work", "role": "Suit les ventes", "brief": "Le plan vise 200 abonnés.",
               "actions": ["Lister les prospects", "Préparer un mail"]})
    };
    let team = json!({"subject": "Plan", "agents": [member("Ventes"), member("Compta")]});
    let coach = json!({"kind": "wellness", "name": "Coach", "role": "Suit mes séances"});
    let project = json!({"name": "Voyage", "summary": "Rome en mai"});
    for (tool, given) in [
        (bipbridge::guide::PROPOSE_AGENT, coach.clone()),
        (bipbridge::guide_team::PROPOSE_TEAM, team.clone()),
        (bipbridge::guide::CREATE_PROJECT, project.clone()),
    ] {
        let result = bipbridge::guide::call(&c, &agent, tool, &args(given)).unwrap();
        assert_eq!(result["isError"], true, "{tool}");
        assert_eq!(result["content"][0]["text"], agents_admin::TOO_MANY_AGENTS_TOOL, "{tool}");
    }
    assert!(bipbridge::guide::load(&c).proposals.is_empty(), "no card past the limit");

    // cards made while there was room, tapped once the place is full: refused, the cards kept
    app.app.state.config.update(|c| c.place_agents_max = Some(3));
    let c = app.app.state.config();
    for (tool, given) in [
        (bipbridge::guide::PROPOSE_AGENT, coach),
        (bipbridge::guide_team::PROPOSE_TEAM, team),
        (bipbridge::guide::CREATE_PROJECT, project),
    ] {
        let result = bipbridge::guide::call(&c, &agent, tool, &args(given)).unwrap();
        assert_eq!(result["isError"], false, "{tool}: {result:?}");
    }
    app.app.state.config.update(|c| c.place_agents_max = Some(1));
    let cards = bipbridge::guide::load(&app.app.state.config()).proposals;
    assert_eq!(cards.len(), 3);
    for card in &cards {
        let id = card["id"].as_str().unwrap();
        let got = app.request("POST", &format!("/v1/guide/proposals/{id}/accept"), &h, None).await;
        assert_eq!((got.status.as_u16(), got.json()), (409, refused.clone()), "{}: {}", card["type"], got.text());
    }
    assert_eq!(bipbridge::guide::load(&app.app.state.config()).proposals.len(), 3);
    assert_eq!(app.app.state.config().agents.len(), 1);
}

/// A hub's gateway that serves a new profile only `pickup` after its first poll (the real one picks new profiles up
/// within 30 s).
async fn slow_gateway(pickup: std::time::Duration) -> String {
    let seen: Arc<Mutex<BTreeMap<String, std::time::Instant>>> = Arc::default();
    common::spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = seen.clone();
        async move {
            let path = request.uri().path().to_string();
            let profile = path.split('/').nth(2).unwrap_or("").to_string();
            let first = *seen.lock().unwrap().entry(profile).or_insert_with(std::time::Instant::now);
            if first.elapsed() < pickup {
                return (axum::http::StatusCode::NOT_FOUND, axum::Json(json!({})));
            }
            if path.ends_with("/api/sessions") {
                return (axum::http::StatusCode::CREATED, axum::Json(json!({"id": "api_1", "title": "Bot Chat"})));
            }
            (axum::http::StatusCode::OK, axum::Json(json!({"runs": true})))
        }
    })))
    .await
}

static SLOW_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A `run` whose every `hermes` command takes `command` (the CLI's start-up).
fn slow_run(command: std::time::Duration) -> Run {
    Arc::new(move |_args: Vec<String>, _env: BTreeMap<String, String>| {
        SLOW_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async move {
            tokio::time::sleep(command).await;
            Ok(String::new())
        })
    })
}

#[tokio::test]
async fn a_team_is_made_as_one_batch() {
    // 2026-10-30: a 6-agent team made one agent after the other took minutes, past the app's wait. Scaled down: the
    // gateway serves a profile 1.2 s after its first poll (up to 30 s for real), each `hermes` command takes 40 ms.
    use std::time::{Duration, Instant};
    let team: Vec<NewAgent> = ["Courses", "Jardin", "Budget", "Sport", "Voyages", "Devoirs"]
        .iter()
        .map(|n| new_agent(json!({"name": n, "kind": "daily", "lang": "en"})))
        .collect();
    let http = reqwest::Client::new();
    let (pickup, command) = (Duration::from_millis(1200), Duration::from_millis(40));

    let dir = tempfile::tempdir().unwrap();
    let (config, _) = managed_config(dir.path(), &slow_gateway(pickup).await);
    let started = Instant::now();
    for body in &team[..3] {
        create_agent(&config, body, &http, &slow_run(command)).await.unwrap();
    }
    let three_one_by_one = started.elapsed();
    for body in &team[3..] {
        create_agent(&config, body, &http, &slow_run(command)).await.unwrap();
    }
    let one_by_one = started.elapsed();

    let dir = tempfile::tempdir().unwrap();
    let (config, path) = managed_config(dir.path(), &slow_gateway(pickup).await);
    let started = Instant::now();
    let made = agents_admin::create_agents(&config, &team[..3], &http, &slow_run(command)).await;
    let three_batch = started.elapsed();
    assert!(made.iter().all(Result::is_ok));
    let made = agents_admin::create_agents(&config, &team[3..], &http, &slow_run(command)).await;
    assert!(made.iter().all(Result::is_ok));
    let dir6 = tempfile::tempdir().unwrap();
    let (config6, path6) = managed_config(dir6.path(), &slow_gateway(pickup).await);
    let started6 = Instant::now();
    let made6 = agents_admin::create_agents(&config6, &team, &http, &slow_run(command)).await;
    let batch = started6.elapsed();
    eprintln!(
        "team timing: 3 agents one by one {three_one_by_one:?}, as one batch {three_batch:?}; \
         6 agents one by one {one_by_one:?}, as one batch {batch:?}; {} commands in all",
        SLOW_RUNS.load(std::sync::atomic::Ordering::Relaxed)
    );
    let names: Vec<String> = made6.into_iter().map(|r| r.unwrap().name).collect();
    assert_eq!(names, ["courses", "jardin", "budget", "sport", "voyages", "devoirs"]);
    let kept = reread(&path6);
    assert!(names.iter().all(|n| kept.agent(Some(n)).is_some()), "all six in the bridge's config");
    assert!(reread(&path).agent(Some("devoirs")).is_some());
    assert!(one_by_one >= pickup * 6);
    assert!(batch * 3 < one_by_one * 2, "the gateway's pick-ups waited for side by side: {batch:?}");
    // the same name twice in one batch: two distinct profiles
    let twice = [new_agent(json!({"name": "Léo", "kind": "daily"})), new_agent(json!({"name": "Léo", "kind": "home"}))];
    let made = agents_admin::create_agents(&config6, &twice, &http, &slow_run(Duration::ZERO)).await;
    let ids: Vec<String> = made.iter().map(|r| r.as_ref().map(|a| a.name.clone()).unwrap_or_default()).collect();
    assert!(!ids[0].is_empty() && ids[0] != ids[1], "{ids:?}");
}
