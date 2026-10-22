//! tests/test_browsers.py: each agent's visible browser (its container, ports, MCP server, on/off switch).
//!
//! Python's `save_agent` / `delete_agent` halves (agents_admin) are checked in the agents group's tests; here the
//! browser side of a deletion is `browsers::remove`.

mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bipbridge::agents_admin::pyyaml;
use bipbridge::agents_admin::Run;
use bipbridge::browsers::{self, BrowserError, Exec, Host};
use bipbridge::config::{parse_config, Config};
use serde_json::{json, Value};

type Commands = Arc<Mutex<Vec<(Vec<String>, BTreeMap<String, String>)>>>;

struct Setup {
    config: Config,
    hub: PathBuf,
    run: Run,
    commands: Commands,
    tmp: PathBuf,
    host: Host,
    _dir: tempfile::TempDir,
}

fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let tmp = dir.path().to_path_buf();
    let hub = tmp.join(".hermes");
    std::fs::create_dir_all(hub.join("profiles").join("budget")).unwrap();
    let config_path = tmp.join("bridge.toml");
    std::fs::write(&config_path, "# bridge\n").unwrap();
    let mut data = common::config_dict(&tmp, &common::Urls::default());
    data["hermes"] = json!({"bin": "/opt/hermes", "home": hub.to_string_lossy(), "hub_url": "http://127.0.0.1:8642",
                            "isolated": true});
    data["browser"] = json!({"enabled": true, "runtime": "podman", "root": tmp.join("browsers").to_string_lossy(),
                             "first_port": 19600, "language": "fr-FR", "timezone": "Europe/Paris", "gpu": "none"});
    data["agents"]["Budget"] = json!({"hermes_url": "http://127.0.0.1:8642/p/budget", "hermes_key": "b".repeat(32)});
    let config = parse_config(&data, Some(&config_path.to_string_lossy())).unwrap();
    let commands: Commands = Arc::default();
    let seen = commands.clone();
    let run: Run = Arc::new(move |args: Vec<String>, env: BTreeMap<String, String>| {
        seen.lock().unwrap().push((args, env));
        Box::pin(async { Ok(String::new()) })
    });
    let host = Host {
        system: "Linux".into(),
        runtime: Some("/usr/bin/podman".into()),
        unit_dir: tmp.join("units"),
        is_free: Arc::new(|_| true),
        exec: Arc::new(|_| Box::pin(async { 0 })),
    };
    Setup { config, hub, run, commands, tmp, host, _dir: dir }
}

impl Setup {
    fn ran(&self) -> Vec<Vec<String>> {
        self.commands.lock().unwrap().iter().map(|c| c.0.clone()).collect()
    }
    fn clear(&self) {
        self.commands.lock().unwrap().clear();
    }
    fn home(&self) -> String {
        self.hub.to_string_lossy().into_owned()
    }
    fn host_free(&self, free: impl Fn(u16) -> bool + Send + Sync + 'static) -> Host {
        Host { is_free: Arc::new(free), ..self.host.clone() }
    }
    async fn setup_budget(&self, host: &Host) -> bipbridge::config::AgentConfig {
        let agent = self.config.agent(Some("budget")).unwrap().clone();
        browsers::setup_with(host, &self.config, &agent, &self.run, &self.home()).await.unwrap()
    }
}

/// The browser's first use (bip-browser-mcp's first start): its container's unit and Chrome profile written.
fn first_use(s: &Setup) -> bool {
    let settings = read_json(&s.tmp.join("browsers/budget/mcp.json"));
    bipbridge::browser_mcp::write_unit(settings.as_object().unwrap()).unwrap()
}

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn an_install_hands_out_only_its_own_range_and_moves_a_browser_from_outside_it() {
    let mut s = setup();
    s.config.browser.ports = 4;
    let place = s.tmp.join("browsers").join("budget");
    std::fs::create_dir_all(place.join("profile")).unwrap();
    std::fs::write(place.join("profile").join("Cookies"), "kept").unwrap();
    std::fs::write(place.join("ports.json"), json!({"cdp": 9600, "vnc": 9601}).to_string()).unwrap();
    let outside: indexmap::IndexMap<String, u16> =
        [("cdp".to_string(), 9600), ("vnc".to_string(), 9601)].into_iter().collect();
    assert!(!browsers::in_range(&s.config, &outside));
    let agent = s.setup_budget(&s.host.clone()).await;
    assert_eq!(read_json(&place.join("ports.json")), json!({"cdp": 19600, "vnc": 19601}));
    assert_eq!(agent.desktop_vnc.as_deref(), Some("tcp://127.0.0.1:19601"));
    assert_eq!(std::fs::read_to_string(place.join("profile").join("Cookies")).unwrap(), "kept");
    assert!(s.ran().contains(&v(&[
        "/opt/hermes",
        "-p",
        "budget",
        "config",
        "set",
        "browser.cdp_url",
        "http://127.0.0.1:19600"
    ])));
    std::fs::create_dir_all(s.tmp.join("browsers").join("other")).unwrap();
    std::fs::write(
        s.tmp.join("browsers").join("other").join("ports.json"),
        json!({"cdp": 19602, "vnc": 19603}).to_string(),
    )
    .unwrap();
    // 19600-19603 all handed out: the range is full
    assert!(browsers::pick_ports(&s.config, &|_| true).is_err());
}

#[tokio::test]
async fn an_agent_gets_its_own_browser_container_and_desktop() {
    let s = setup();
    let agent = s.setup_budget(&s.host_free(|p| p != 19600)).await;
    // Its two local ports (19600 is in use), its VNC password and its Chrome profile, kept apart.
    let place = s.tmp.join("browsers").join("budget");
    let ports = read_json(&place.join("ports.json"));
    assert_eq!(ports, json!({"cdp": 19602, "vnc": 19603}));
    let password = place.join("vnc.pass");
    assert_eq!(std::fs::read_to_string(&password).unwrap().trim().len(), 8);
    assert_eq!(std::fs::metadata(&password).unwrap().permissions().mode() & 0o777, 0o600);
    // Its container and Chrome profile are only set up at its first use (bip-browser-mcp), not with the agent.
    let unit_file = s.tmp.join("units").join("bipagents-browser-budget.service");
    assert!(!place.join("profile").exists() && !unit_file.exists());
    assert!(!s.ran().iter().any(|c| c[0] == "systemctl"));
    assert!(first_use(&s) && !first_use(&s)); // written once
    assert!(place.join("profile").is_dir());
    // The container: nothing of the host but its profile and password, ports on 127.0.0.1 only.
    let unit = std::fs::read_to_string(&unit_file).unwrap();
    let start = unit.lines().find(|l| l.starts_with("ExecStart=")).unwrap();
    assert!(start.contains("--name bipagents-browser-budget") && start.contains("--cap-drop ALL"));
    assert!(start.contains(&format!("-v {}/browsers/budget/profile:/home/bip/profile:Z", s.tmp.display())));
    assert!(start.contains("-p 127.0.0.1:19602:9223 -p 127.0.0.1:19603:5900"));
    assert!(start.contains("--userns=keep-id:uid=1000,gid=1000") && !start.contains("--device"));
    assert!(start.contains("--accept-lang=fr-FR,fr") && start.contains("TZ=Europe/Paris"));
    assert!(start.ends_with(" bipagents-browser"));
    // Not started with the session: the agent's Hermes pointed at it, its MCP server behind bip-browser-mcp.
    let ran = s.ran();
    assert!(!ran
        .iter()
        .any(|c| c[..3] == v(&["systemctl", "--user", "enable"])[..] || c.contains(&"start".to_string())));
    assert!(ran.contains(&v(&[
        "/opt/hermes",
        "-p",
        "budget",
        "config",
        "set",
        "browser.cdp_url",
        "http://127.0.0.1:19602"
    ])));
    let mcp = ran.iter().find(|c| c.len() > 6 && c[4..6] == v(&["set", "mcp_servers.stealth"])[..]).unwrap();
    let mcp: Value = serde_json::from_str(&mcp[6]).unwrap();
    assert_eq!(
        mcp,
        json!({"command": browsers::proxy_binary(), "connect_timeout": 90, "timeout": 120,
                            "args": [place.join("mcp.json").to_string_lossy()],
                            "tools": {"exclude": ["get_instance_state"]}})
    );
    assert!(mcp["command"].as_str().unwrap().ends_with("bip-browser-mcp"));
    let proxy = read_json(&place.join("mcp.json"));
    assert_eq!(
        proxy["server"],
        json!(["/usr/bin/podman", "exec", "-i", "bipagents-browser-budget", "/opt/stealth/mcp.sh"])
    );
    assert_eq!(
        proxy["start"].as_array().unwrap()[1..],
        json!(["--user", "start", "bipagents-browser-budget"]).as_array().unwrap()[..]
    );
    assert_eq!(
        proxy["stop"].as_array().unwrap()[1..],
        json!(["--user", "stop", "bipagents-browser-budget"]).as_array().unwrap()[..]
    );
    assert_eq!(proxy["idle"], 600);
    assert_eq!(proxy["cdp_port"], 19602);
    assert_eq!(proxy["busy"], json!(place.join("busy").to_string_lossy()));
    assert_eq!(proxy["unit"]["path"], json!(unit_file.to_string_lossy()));
    assert_eq!(proxy["unit"]["text"], json!(unit));
    assert_eq!(proxy["reload"].as_array().unwrap()[1..], json!(["--user", "daemon-reload"]).as_array().unwrap()[..]);
    assert_eq!(proxy["shared_cache"], json!(s.tmp.join("browsers/mcp-cache.json").to_string_lossy()));
    for (args, env) in s.commands.lock().unwrap().iter() {
        if args[0] == "/opt/hermes" {
            assert_eq!(env, &BTreeMap::from([("HERMES_HOME".to_string(), s.home())]));
        }
    }
    let skill = s.hub.join("profiles/budget/skills/visible-browser/SKILL.md");
    assert!(std::fs::read_to_string(skill).unwrap().starts_with("---"));
    // Of Hermes' own browser tools only the vault's stay: it browses in the browser « Bureau » shows.
    assert!(browsers::vault_only(&s.home(), "budget"));
    // The app's « Bureau »: the bridge logs in to its VNC server.
    assert_eq!(agent.desktop_vnc.as_deref(), Some("tcp://127.0.0.1:19603"));
    assert_eq!(agent.desktop_vnc_password_file.as_deref(), Some(&*password.to_string_lossy()));
    // A second agent takes the next ports; setting one up again keeps its own.
    let next = browsers::pick_ports(&s.config, &|_| true).unwrap();
    assert_eq!((next["cdp"], next["vnc"]), (19600, 19601));
    s.clear();
    s.setup_budget(&s.host).await;
    assert_eq!(read_json(&place.join("ports.json")), ports);
    assert!(s.ran().contains(&v(&["systemctl", "--user", "disable", "bipagents-browser-budget"])));
    assert!(!s.ran().contains(&v(&["systemctl", "--user", "restart", "bipagents-browser-budget"])));
    // A unit written by an earlier bridge (a 1600×900 window): rewritten, and its container restarted with these flags.
    std::fs::write(
        &unit_file,
        std::fs::read_to_string(&unit_file).unwrap().replace("--start-maximized", "--window-size=1600,900"),
    )
    .unwrap();
    s.clear();
    s.setup_budget(&s.host).await;
    let text = std::fs::read_to_string(&unit_file).unwrap();
    assert!(text.contains("--start-maximized") && !text.contains("--window-size"));
    assert!(s.ran().contains(&v(&["systemctl", "--user", "try-restart", "bipagents-browser-budget"])));
}

#[tokio::test]
async fn a_browser_kept_running() {
    let mut s = setup();
    s.config.browser.idle_minutes = 0;
    s.setup_budget(&s.host.clone()).await;
    let ran = s.ran();
    assert!(ran.contains(&v(&["systemctl", "--user", "enable", "--now", "bipagents-browser-budget"])));
    let mcp = ran.iter().find(|c| c.len() > 6 && c[4..6] == v(&["set", "mcp_servers.stealth"])[..]).unwrap();
    let mcp: Value = serde_json::from_str(&mcp[6]).unwrap();
    assert_eq!(mcp["command"], "/usr/bin/podman");
    assert_eq!(mcp["args"], json!(["exec", "-i", "bipagents-browser-budget", "/opt/stealth/mcp.sh"]));
}

#[tokio::test]
async fn deleting_the_agent_removes_its_browser() {
    let s = setup();
    s.setup_budget(&s.host).await;
    first_use(&s);
    s.clear();
    browsers::remove_with(&s.host, &s.config, "budget", &s.run).await.unwrap();
    let ran = s.ran();
    assert!(ran.contains(&v(&["systemctl", "--user", "disable", "--now", "bipagents-browser-budget"])));
    assert!(ran.contains(&v(&["/usr/bin/podman", "rm", "-f", "bipagents-browser-budget"])));
    assert!(!s.tmp.join("browsers").join("budget").exists());
    assert!(!s.tmp.join("units").join("bipagents-browser-budget.service").exists());
}

#[test]
fn with_a_gpu_and_docker() {
    let s = setup();
    let ports = [("cdp".to_string(), 1), ("vnc".to_string(), 2)].into_iter().collect();
    let args = browsers::run_args(&s.config, "budget", &ports, "/usr/bin/docker", true);
    assert!(args.contains(&"--device".to_string()) && !args.contains(&"keep-groups".to_string()));
    assert!(!args.contains(&"--userns=keep-id:uid=1000,gid=1000".to_string()));
    assert!(args.iter().any(|a| a.contains("--use-angle=vulkan")));
}

#[tokio::test]
async fn only_on_linux() {
    let s = setup();
    let host = Host { system: "Darwin".into(), ..s.host.clone() };
    let agent = s.config.agent(Some("budget")).unwrap().clone();
    let err = browsers::setup_with(&host, &s.config, &agent, &s.run, &s.home()).await.unwrap_err();
    assert!(err.downcast_ref::<BrowserError>().is_some());
    assert!(s.ran().is_empty());
}

type Execs = Arc<Mutex<Vec<Vec<String>>>>;

fn recording_exec(screen_up: Arc<Mutex<bool>>) -> (Exec, Execs) {
    let ran: Execs = Arc::default();
    let seen = ran.clone();
    let exec: Exec = Arc::new(move |args: Vec<String>| {
        seen.lock().unwrap().push(args.clone());
        let mut up = screen_up.lock().unwrap();
        if args[..3] == ["systemctl", "--user", "start"] {
            *up = true;
        }
        let code = if *up || args[0] == "systemctl" { 0 } else { 1 };
        Box::pin(async move { code })
    });
    (exec, ran)
}

#[tokio::test]
async fn taking_over_the_browser_makes_the_agents_tools_wait() {
    let s = setup();
    s.setup_budget(&s.host).await;
    let (exec, ran) = recording_exec(Arc::new(Mutex::new(true)));
    let host = Host { exec, ..s.host.clone() };
    assert!(browsers::hold_with(&host, &s.config, "budget", true).await);
    // Never used: its container set up first. Then its screen checked (a browser stopped while unused starts
    // again), then the hold.
    let done = ran.lock().unwrap().clone();
    assert_eq!(done[0], v(&["systemctl", "--user", "daemon-reload"]));
    assert!(s.tmp.join("units").join("bipagents-browser-budget.service").exists());
    assert_eq!(
        done[1],
        v(&["/usr/bin/podman", "exec", "bipagents-browser-budget", "test", "-e", "/tmp/.X11-unix/X99"])
    );
    // The person's hold, with a token only the bridge knows (its own view of the page still gets through).
    let token = browsers::hold_token(&s.config, "budget").unwrap();
    assert_eq!(token.len(), 32);
    assert_eq!(
        done.last().unwrap(),
        &v(&[
            "/usr/bin/podman",
            "exec",
            "bipagents-browser-budget",
            "sh",
            "-c",
            &format!("umask 077; printf 'human %s' {token} > /tmp/bipagents-hold")
        ])
    );
    let kept = s.tmp.join("browsers").join("budget").join("hold.token");
    assert_eq!(std::fs::metadata(&kept).unwrap().permissions().mode() & 0o777, 0o600);
    browsers::hold_with(&host, &s.config, "budget", false).await;
    assert_eq!(ran.lock().unwrap().last().unwrap().last().unwrap(), "rm -f /tmp/bipagents-hold");
    assert!(browsers::hold_token(&s.config, "budget").is_none());
    assert!(!browsers::hold_with(&host, &s.config, "nobody", true).await); // no browser of its own
    assert_eq!(ran.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn the_desktop_wakes_a_stopped_browser() {
    let s = setup();
    s.setup_budget(&s.host).await;
    let (exec, ran) = recording_exec(Arc::new(Mutex::new(false)));
    let host = Host { exec, ..s.host.clone() };
    assert!(browsers::wake_with(&host, &s.config, "budget", 60.0).await);
    assert!(ran.lock().unwrap().contains(&v(&["systemctl", "--user", "start", "bipagents-browser-budget"])));
    assert!(s.tmp.join("browsers/budget/busy").exists()); // not stopped again right away
    ran.lock().unwrap().clear();
    assert!(browsers::wake_with(&host, &s.config, "budget", 60.0).await);
    assert_eq!(ran.lock().unwrap().len(), 1); // already up: nothing started
    assert!(!browsers::wake_with(&host, &s.config, "nobody", 60.0).await);
}

#[tokio::test]
async fn the_apps_switch_turns_the_browser_off_and_on_keeping_its_profile() {
    let s = setup();
    let agent = s.config.agent(Some("budget")).unwrap().clone();
    let home = s.home();
    assert_eq!(browsers::state(&home, "budget"), None);
    assert!(browsers::available_with(&s.host, &s.config));
    // The first time: set up (its container, its MCP server in the profile).
    browsers::switch_with(&s.host, &s.config, &agent, true, &s.run, &home).await.unwrap();
    assert!(s.ran().iter().any(|c| c.len() > 6 && c[4..6] == v(&["set", "mcp_servers.stealth"])[..]));
    first_use(&s);
    let profile = s.hub.join("profiles/budget/config.yaml"); // what `hermes config set` wrote
    std::fs::write(&profile, "model:\n  provider: custom\nskills:\n  disabled:\n  - arxiv\nmcp_servers:\n  stealth:\n    command: python3\n    args:\n    - proxy\n").unwrap();
    assert_eq!(browsers::state(&home, "budget"), Some(true));
    // Off: its tools and skill off, its Chrome stopped; its profile and its MCP entry stay.
    s.clear();
    browsers::switch_with(&s.host, &s.config, &agent, false, &s.run, &home).await.unwrap();
    let data = pyyaml::to_json(&pyyaml::load(&std::fs::read_to_string(&profile).unwrap()).unwrap());
    assert_eq!(data["mcp_servers"]["stealth"], json!({"command": "python3", "args": ["proxy"], "enabled": false}));
    assert_eq!(data["skills"]["disabled"], json!(["arxiv", "visible-browser"]));
    assert_eq!(data["model"], json!({"provider": "custom"}));
    let ran = s.ran();
    assert!(ran.contains(&v(&[
        "/opt/hermes",
        "-p",
        "budget",
        "tools",
        "disable",
        "browser",
        "--platform",
        "api_server"
    ])));
    assert!(ran.contains(&v(&["systemctl", "--user", "stop", "bipagents-browser-budget"])));
    assert!(s.tmp.join("browsers/budget/profile").is_dir());
    assert_eq!(browsers::state(&home, "budget"), Some(false));
    // On again: instant, nothing set up anew.
    s.clear();
    browsers::switch_with(&s.host, &s.config, &agent, true, &s.run, &home).await.unwrap();
    let data = pyyaml::to_json(&pyyaml::load(&std::fs::read_to_string(&profile).unwrap()).unwrap());
    assert_eq!(data["mcp_servers"]["stealth"]["enabled"], true);
    assert_eq!(data["skills"]["disabled"], json!(["arxiv"]));
    assert_eq!(
        s.ran(),
        vec![v(&["/opt/hermes", "-p", "budget", "tools", "enable", "browser", "--platform", "api_server"])]
    );
}

#[tokio::test]
async fn no_browser_to_give_without_the_installers_image() {
    let mut s = setup();
    s.config.browser.enabled = false;
    assert!(!browsers::available_with(&s.host, &s.config));
    let agent = s.config.agent(Some("budget")).unwrap().clone();
    let err = browsers::switch_with(&s.host, &s.config, &agent, true, &s.run, &s.home()).await.unwrap_err();
    assert!(err.downcast_ref::<BrowserError>().is_some());
    assert!(s.ran().is_empty());
}
