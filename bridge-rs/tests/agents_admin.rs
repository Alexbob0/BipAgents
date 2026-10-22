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
    assert_eq!(ran[0], ["/opt/hermes", "-p", "cuisine", "config", "set", "API_SERVER_KEY", &cuisine.hermes_key]);
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
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), soul); // once
    std::fs::write(folder.join("SOUL.md"), "Fait à la main, sans bloc.\n").unwrap();
    heal_souls(&c);
    assert_eq!(read_soul(&c, &agent).unwrap(), "Fait à la main, sans bloc.\n"); // a hand-made personality stays as it is
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
            "group"
        ]
    );
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
