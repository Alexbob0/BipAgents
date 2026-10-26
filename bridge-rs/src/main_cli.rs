//! `python -m bipbridge [serve|check|genkey|qr|peer-token|browser]` → `bipbridge <the same>`.
//!
//! Port of `bridge/bipbridge/__main__.py` (core group): the same commands, options, messages and exit codes; `serve`
//! runs the main listener (and the LAN door over HTTPS when it is on) until SIGTERM / Ctrl-C, then stops within
//! five seconds (uvicorn's `timeout_graceful_shutdown=5`).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::Router;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::app::{create_app, AppOptions};
use crate::config::{config_path, load_config, Config, SharedConfig};
use crate::logs::setup_logging;
use crate::py;

const COMMANDS: [&str; 12] = [
    "serve",
    "check",
    "genkey",
    "qr",
    "peer-token",
    "browser",
    "shares-mounts",
    "console",
    "spaces",
    "adopt",
    "sandbox",
    "agent-tools",
];

const ACTIONS: [&str; 3] = ["add", "remove", "start"];

/// The parsed command line (Python's argparse namespace).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub command: String,
    pub action: Option<String>,
    pub config: Option<String>,
    pub agent: Option<String>,
    pub png: Option<String>,
    pub json: bool,
    pub all: bool,
}

const USAGE: &str = "usage: bipbridge [-h] [--config CONFIG] [--agent AGENT] [--png PNG] [--json]
                 [--all]
                 [{serve,check,genkey,qr,peer-token,browser,shares-mounts,console,spaces,adopt,sandbox,agent-tools}]
                 [{add,remove,start}]
";

const HELP: &str = "
BipAgents bridge for Hermes agents

positional arguments:
  {serve,check,genkey,qr,peer-token,browser,shares-mounts,console,spaces,adopt,sandbox,agent-tools}
                        serve (default), check the config, print a new random
                        bridge key, show an agent's pairing QR code for the
                        app, (peer-token --agent X) give an agent set up by
                        hand its token for messaging the others, or (browser
                        add|remove --agent X|--all) set up or remove agents'
                        visible browsers, or (shares-mounts --agent X) print
                        the `-v` mounts an agent set up by hand needs for its
                        shared folders, or (console) give a one-time link that
                        opens the web console in a browser, or (spaces start)
                        start again the agents' own spaces after an install
                        was moved here, or (adopt) serve the agents an export
                        brought to a place as its own (bipinstall import-
                        place), or (sandbox) isolate the agents' sandboxes
                        now and print what each one got, or (agent-tools
                        --agent X) give an agent set up by hand the bridge's
                        tools (files to other agents, voices, pages...)
  {add,remove,start}    browser: add or remove; spaces: start

options:
  -h, --help            show this help message and exit
  --config CONFIG       config path (default: $BRIDGE_CONFIG or
                        ~/.config/hermes-ios/bridge.toml)
  --agent AGENT         qr: one agent's own code (default: the install's code,
                        which adds every agent)
  --png PNG             qr: also save the QR code as a PNG (one agent)
  --json                qr: print the payload instead of the QR code
  --all                 browser add: every agent this install manages
";

/// What argparse does instead of returning: print the help (exit 0) or an error (usage on stderr, exit 2).
#[derive(Debug, Clone, PartialEq)]
pub enum ArgsExit {
    Help,
    Error(String),
}

fn negative_number(arg: &str) -> bool {
    let Some(rest) = arg.strip_prefix('-') else { return false };
    let digits = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    digits(rest) || rest.split_once('.').map(|(a, b)| (a.is_empty() || digits(a)) && digits(b)).unwrap_or(false)
}

fn looks_like_option(arg: &str) -> bool {
    arg.starts_with('-') && arg.len() > 1 && !negative_number(arg)
}

/// Python's `argparse` for this parser: options anywhere (`--config X`, `--config=X`, unique prefixes such as
/// `--conf`), the two optional positionals in order, the same error messages.
pub fn parse_args(argv: &[String]) -> Result<Args, ArgsExit> {
    const OPTIONS: [&str; 6] = ["--help", "--config", "--agent", "--png", "--json", "--all"];
    let mut args = Args { command: "serve".into(), ..Default::default() };
    let mut positionals: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    let mut i = 0;
    let mut only_positionals = false;
    while i < argv.len() {
        let arg = &argv[i];
        i += 1;
        if only_positionals || !looks_like_option(arg) {
            positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        if arg == "-h" {
            return Err(ArgsExit::Help);
        }
        if !arg.starts_with("--") {
            extras.push(arg.clone());
            continue;
        }
        let (name, explicit) = match arg.split_once('=') {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let matches: Vec<&str> = if OPTIONS.contains(&name.as_str()) {
            vec![OPTIONS.iter().copied().find(|o| *o == name).unwrap_or_default()]
        } else {
            OPTIONS.iter().copied().filter(|o| o.starts_with(name.as_str())).collect()
        };
        let option = match matches.as_slice() {
            [one] => *one,
            [] => {
                extras.push(arg.clone());
                continue;
            }
            many => return Err(ArgsExit::Error(format!("ambiguous option: {name} could match {}", many.join(", ")))),
        };
        match option {
            "--help" => return Err(ArgsExit::Help),
            "--json" | "--all" => {
                if let Some(value) = explicit {
                    return Err(ArgsExit::Error(format!("argument {option}: ignored explicit argument '{value}'")));
                }
                if option == "--json" {
                    args.json = true;
                } else {
                    args.all = true;
                }
            }
            _ => {
                let value = match explicit {
                    Some(v) => v,
                    None => match argv.get(i) {
                        Some(next) if !looks_like_option(next) => {
                            i += 1;
                            next.clone()
                        }
                        _ => return Err(ArgsExit::Error(format!("argument {option}: expected one argument"))),
                    },
                };
                match option {
                    "--config" => args.config = Some(value),
                    "--agent" => args.agent = Some(value),
                    _ => args.png = Some(value),
                }
            }
        }
    }
    let mut positionals = positionals.into_iter();
    if let Some(command) = positionals.next() {
        if !COMMANDS.contains(&command.as_str()) {
            return Err(ArgsExit::Error(format!(
                "argument command: invalid choice: '{command}' (choose from {})",
                COMMANDS.join(", ")
            )));
        }
        args.command = command;
    }
    if let Some(action) = positionals.next() {
        if !ACTIONS.contains(&action.as_str()) {
            return Err(ArgsExit::Error(format!(
                "argument action: invalid choice: '{action}' (choose from {})",
                ACTIONS.join(", ")
            )));
        }
        args.action = Some(action);
    }
    extras.extend(positionals);
    if !extras.is_empty() {
        return Err(ArgsExit::Error(format!("unrecognized arguments: {}", extras.join(" "))));
    }
    Ok(args)
}

/// The process's exit code.
pub fn main(argv: Vec<String>) -> i32 {
    // `bipbridge space …`: the encrypted space (space.rs), its own command line.
    if argv.first().map(String::as_str) == Some("space") {
        return crate::space::cli(&argv[1..]);
    }
    // `bipbridge sandbox adopt …`: its own command line (sandbox_adopt.rs), `--config X` before it or among its own
    // options.
    let (mut config_first, mut rest) = (Vec::new(), &argv[..]);
    while let Some(first) = rest.first() {
        if first == "--config" && rest.len() > 1 {
            config_first.extend(rest[..2].iter().cloned());
            rest = &rest[2..];
        } else if let Some(value) = first.strip_prefix("--config=") {
            config_first.extend(["--config".to_string(), value.to_string()]);
            rest = &rest[1..];
        } else {
            break;
        }
    }
    let with_config = |args: &[String]| -> Vec<String> { args.iter().cloned().chain(config_first.clone()).collect() };
    // `bipbridge archive …`: an agent's archives from the command line (archives/).
    if rest.first().map(String::as_str) == Some("archive") {
        return crate::archives::cli(&with_config(&rest[1..]));
    }
    // `bipbridge approvals [--agent X] [--last N]`: what the agents asked, from the bridge's logs (approvals_log.rs).
    if rest.first().map(String::as_str) == Some("approvals") {
        return crate::approvals_log::cli(&with_config(&rest[1..]));
    }
    if rest.first().map(String::as_str) == Some("sandbox") && rest.get(1).map(String::as_str) == Some("adopt") {
        return crate::sandbox_adopt::cli(&with_config(&rest[2..]));
    }
    let args = match parse_args(&argv) {
        Ok(a) => a,
        Err(ArgsExit::Help) => {
            print!("{USAGE}{HELP}");
            return 0;
        }
        Err(ArgsExit::Error(message)) => {
            eprint!("{USAGE}");
            eprintln!("bipbridge: error: {message}");
            return 2;
        }
    };
    if args.command == "genkey" {
        println!("{}", py::token_urlsafe(32));
        return 0;
    }
    // An encrypted space not open yet (after a reboot): the locked door until it opens, then this bridge again.
    if args.command == "serve" {
        if let Some(code) = crate::space::serve_if_locked() {
            return code;
        }
    }
    setup_logging("info");
    let config = match load_config(args.config.as_deref()) {
        Ok(c) => c,
        Err(exc) => {
            eprintln!("config error: {exc}");
            return 2;
        }
    };
    setup_logging(&config.log_level);
    // One thread, like Python's asyncio loop: tasks start in the order they are created (the TTS queue's arrival
    // order depends on it), and a person's bridge needs no more (blocking work goes to spawn_blocking's pool).
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match args.command.as_str() {
        "check" => check(&config, &args),
        "qr" => qr(&config, &args),
        "peer-token" => peer_token(&config, &args),
        "shares-mounts" => shares_mounts(&config, &args),
        "agent-tools" => agent_tools_entry(&config, &args),
        "browser" => runtime.block_on(browser(config, &args)),
        "spaces" => runtime.block_on(spaces(&config)),
        "adopt" => runtime.block_on(adopt(config)),
        "sandbox" => runtime.block_on(sandbox(config)),
        "console" => console(&config),
        _ => runtime.block_on(serve(config)),
    }
}

fn check(config: &Config, args: &Args) -> i32 {
    println!("config OK: {}", args.config.clone().unwrap_or_else(config_path));
    println!("  listen      {}:{}", config.host, config.port);
    println!("  kyutai      {}", config.kyutai_url.as_deref().unwrap_or("off"));
    println!("  pocket      {}", config.pocket_url.as_deref().unwrap_or("off"));
    println!("  voice       {} (default)", config.default_voice);
    println!("  ntfy        {}", config.ntfy_url);
    println!(
        "  apns        {} ({})",
        if config.apns.enabled() { "enabled" } else { "disabled" },
        config.apns.environment
    );
    println!("  outbox db   {}", config.outbox.db_path);
    let lan = if config.lan.enabled { crate::lan::lan_info(config) } else { None };
    let lan_text = match lan {
        Some(l) => l["url"].as_str().unwrap_or("").to_string(),
        None if config.lan.enabled => "enabled, address or certificate missing".into(),
        None => "off".into(),
    };
    println!("  lan         {lan_text}");
    for agent in config.agents.values() {
        let or_dash = |v: &Option<String>| v.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "-".into());
        println!(
            "  agent {}: hermes={} ntfy_topic={} uploads={} -> {}",
            agent.name,
            agent.hermes_url,
            or_dash(&agent.ntfy_topic),
            or_dash(&agent.upload_dir_host),
            or_dash(&agent.upload_dir_container)
        );
    }
    0
}

fn peer_token(config: &Config, args: &Args) -> i32 {
    use crate::peers::{manual_token, peer_url, Links, PEER_NAME};
    let Some(agent) = args.agent.as_deref() else {
        eprintln!("peer-token needs --agent");
        return 2;
    };
    let links = Links::new(&py::join(&config.data_dir(), "peers.json"));
    let token = match manual_token(config, &links, agent) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("# In the HERMES_HOME of the gateway serving {agent} (the bridge must be reachable from there):");
    println!("hermes config set bot_peers.{PEER_NAME}.url {}", peer_url(config));
    println!("# In that agent's profile .env (its old token, if any, stops working):");
    println!("HERMES_PEER_{}_KEY={token}", PEER_NAME.to_uppercase());
    0
}

/// The bridge's tools (agent_tools.rs) for an agent whose Hermes the bridge doesn't set up itself: a new token (its
/// old one stops working), printed once with the commands for its administrator, never stored in clear.
fn agent_tools_entry(config: &Config, args: &Args) -> i32 {
    use crate::agent_tools::{digest, load_tokens, save_tokens, tools_for, url_with, version_of, SERVER_NAME};
    let Some(agent) = args.agent.as_deref().and_then(|a| config.agent(Some(a))) else {
        eprintln!("agent-tools needs --agent X (an agent of this bridge)");
        return 2;
    };
    if crate::shares::profile_dir(config, agent).is_some() {
        println!("{} gets the bridge's tools by itself (the bridge sets up its Hermes).", agent.name);
        return 0;
    }
    let token = py::token_urlsafe(32);
    let mut tokens = load_tokens(config);
    tokens.insert(agent.name.clone(), digest(&token));
    if let Err(e) = save_tokens(config, &tokens) {
        eprintln!("{e}");
        return 1;
    }
    let url = url_with(config, &version_of(&tools_for(config, agent)));
    println!("# In the HERMES_HOME of the gateway serving {} (the bridge must be reachable from there):", agent.name);
    println!("hermes -p {} config set mcp_servers.{SERVER_NAME}.url '{url}'", agent.name);
    println!("hermes -p {} config set mcp_servers.{SERVER_NAME}.headers.Authorization 'Bearer {token}'", agent.name);
    println!(
        "hermes -p {} config set mcp_servers.{SERVER_NAME}.timeout {}",
        agent.name,
        crate::agent_tools::TOOL_TIMEOUT
    );
    println!("# then restart that gateway.");
    0
}

fn shares_mounts(config: &Config, args: &Args) -> i32 {
    // For the start script of an agent set up by hand (shares = "external"): one `-v` argument per line.
    let agent = args.agent.as_deref().and_then(|a| config.agent(Some(a)));
    let Some(agent) = agent else {
        eprintln!("shares-mounts needs --agent X (an agent of this bridge)");
        return 2;
    };
    if agent.shares == "external" {
        // Its outbox, open to its container's own user (as the shared folders are, shares.rs).
        let outbox = crate::agent_files::external_outbox(config, &agent.name);
        if std::fs::create_dir_all(&outbox).and_then(|_| py::chmod(&outbox.to_string_lossy(), 0o777)).is_err() {
            eprintln!("could not make {}", outbox.display());
            return 1;
        }
    }
    for mount in crate::shares::mounts(config, &agent.name, None) {
        println!("-v\n{mount}");
    }
    0
}

/// The agents' sandboxes isolated now (what the bridge does at start, sandbox.rs), and what each one got: run by the
/// installer's migration so the update's log says it. The running bridge reads the agents' sections at its restart.
async fn sandbox(config: Config) -> i32 {
    use crate::agents_admin::{default_run, give_tools, managed, mount_uploads};
    let shared = SharedConfig::new(config);
    let run = default_run();
    let config = shared.get();
    if crate::sandbox::engine(&config).is_none() {
        println!("sandboxes left as they are: isolating them needs Linux and podman ([sandbox] isolate)");
        return 0;
    }
    match crate::sandbox::network(&config, &run).await {
        Ok(url) => println!(
            "network: {} (no route out), registries through {url}: {}",
            crate::sandbox::NETWORK,
            config.sandbox.registries.join(", ")
        ),
        Err(why) => println!("network: none ({why}); the sandboxes get no internet at all until it works"),
    }
    // The shared folders' podman volumes (made, filled from the folders the first time), before the sandboxes.
    for line in crate::shares::ensure_volumes(&config, &run, &crate::shares::load(&config)).await {
        println!("{line}");
    }
    let mut failed = 0;
    let agents: Vec<_> = config.agents.values().cloned().collect();
    for agent in agents {
        if managed(&shared.get(), &agent.name).is_err() || !crate::sandbox::isolates(&shared.get(), &agent) {
            continue;
        }
        let result: anyhow::Result<()> = async {
            let agent = mount_uploads(&shared, &agent, &run, true).await?;
            shared.put_agent(agent.clone());
            give_tools(&shared.get(), &agent, &run).await?;
            let folder = crate::shares::profile_dir(&shared.get(), &agent).unwrap_or_default();
            let mounts = crate::shares::docker_volumes(&folder);
            let kept = crate::sandbox::host_mounts(&mounts);
            let by_hand =
                if kept.is_empty() { String::new() } else { format!("; still mounted by hand: {}", kept.join(" ")) };
            println!("{}: isolated (volumes: {}){by_hand}", agent.name, mounts.join(" "));
            Ok(())
        }
        .await;
        if let Err(exc) = result {
            eprintln!("{}: {exc}", agent.name);
            failed += 1;
        }
    }
    i32::from(failed > 0)
}

fn console(config: &Config) -> i32 {
    use crate::console::{ConsoleStore, LINK_MINUTES};
    let store = ConsoleStore::new(&py::join(&config.data_dir(), "console.json"));
    let base = config.public_url.clone().unwrap_or_else(|| format!("http://127.0.0.1:{}", config.port));
    let base = base.trim_end_matches('/');
    let code = match store.new_link() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    // The code goes after « # »: the browser keeps it, no server or proxy log ever sees it.
    println!("Open this link in your computer's browser (once, within {LINK_MINUTES} minutes):\n\n  {base}/console/#{code}\n");
    0
}

fn manages(config: &Config, name: &str) -> bool {
    crate::agents_admin::managed(config, name).is_ok()
}

/// Agents' visible browsers (browsers.py), set up from the command line: the installer, or an administrator adding
/// one to an agent that has none. Run it while the bridge is stopped, or restart the bridge after: it rewrites the
/// agents' sections of the config file.
async fn browser(config: Config, args: &Args) -> i32 {
    use crate::agents_admin::{default_run, home_root, managed, save_agent};
    let names: Vec<String> = if args.all {
        config.agents.values().filter(|a| manages(&config, &a.name)).map(|a| a.name.clone()).collect()
    } else if let Some(agent) = args.agent.clone() {
        vec![agent]
    } else {
        eprintln!("browser needs --agent X or --all");
        return 2;
    };
    let shared = SharedConfig::new(config);
    let run = default_run();
    let mut failed = 0;
    for name in names {
        let config = shared.get();
        let mut agent = match managed(&config, &name) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("{name}: not an agent this install manages");
                failed += 1;
                continue;
            }
        };
        let removing = args.action.as_deref() == Some("remove");
        let result: anyhow::Result<()> = async {
            if removing {
                crate::browsers::remove(&config, &name, &run).await?;
                agent.desktop_vnc = None;
                agent.desktop_vnc_password_file = None;
            } else {
                agent = crate::browsers::setup(&config, &agent, &run, &home_root(&config, &agent)).await?;
            }
            save_agent(&shared, &agent)?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => println!("{name}: browser {}", if removing { "removed" } else { "ready" }),
            Err(exc) => {
                eprintln!("{name}: {exc}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// After an export was put into this place (installer/bipinstall/to_place.py): each agent it brought served as
/// the place's own are (Bot Mode, its files where its sandbox sees them, the bridge's tools), and each profile no
/// agent serves yet (a Hermes alone's) claimed. Run it with the gateway up and the bridge stopped, or restart the
/// bridge after: it rewrites the agents' sections of the config file.
async fn adopt(config: Config) -> i32 {
    use crate::agents_admin::{
        claim_profile, default_run, give_tools, managed, mark_as_bot, mount_uploads, unclaimed_profiles,
    };
    let shared = SharedConfig::new(config);
    let run = default_run();
    let mut failed = 0;
    let agents: Vec<_> = shared.get().agents.values().cloned().collect();
    for agent in agents {
        if managed(&shared.get(), &agent.name).is_err() {
            continue;
        }
        let result: anyhow::Result<()> = async {
            let config = shared.get();
            if let Some(folder) = crate::shares::profile_dir(&config, &agent) {
                if folder.is_dir() {
                    mark_as_bot(&folder)?;
                }
            }
            let agent = mount_uploads(&shared, &agent, &run, true).await?;
            shared.put_agent(agent.clone());
            give_tools(&shared.get(), &agent, &run).await?;
            println!("{}: ready", agent.name);
            Ok(())
        }
        .await;
        if let Err(exc) = result {
            eprintln!("{}: {exc}", agent.name);
            failed += 1;
        }
    }
    let http = match reqwest::Client::builder().timeout(Duration::from_secs(30)).build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    for profile in unclaimed_profiles(&shared.get()) {
        let id = profile.get("id").map(py::py_str).unwrap_or_default();
        match claim_profile(&shared, &id, None, &http, &run).await {
            Ok(agent) => println!("{}: claimed", agent.name),
            Err(exc) => {
                eprintln!("{id}: {exc}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// Each agent with a space of its own (spaces.py) gets its gateway's service again: after `bipagents import`
/// brought the install to this machine (the spaces' folders came with it, their services did not).
async fn spaces(config: &Config) -> i32 {
    let spaces = crate::spaces::Spaces::new(crate::agents_admin::default_run());
    let mut failed = 0;
    for agent in config.agents.values() {
        if agent.space.as_deref().map(str::is_empty).unwrap_or(true) {
            continue;
        }
        match spaces.start(config, agent).await {
            Ok(()) => println!("{}: space started", agent.name),
            Err(exc) => {
                eprintln!("{}: {exc}", agent.name);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        1
    } else {
        0
    }
}

/// An agent's pairing QR code in the terminal (scan it in the app: Ajouter un agent › Scanner le QR code).
fn qr(config: &Config, args: &Args) -> i32 {
    use crate::lan::{ensure_certificate, install_payload, pairing_payload};
    if config.lan.enabled {
        // the fingerprint goes in the payload
        if let Err(e) = ensure_certificate(config) {
            eprintln!("{e}");
            return 1;
        }
    }
    if let Some(name) = args.agent.as_deref() {
        if config.agent(Some(name)).is_none() {
            eprintln!("unknown agent: {name}");
            return 2;
        }
    }
    let made = match args.agent.as_deref().and_then(|a| config.agent(Some(a))) {
        Some(agent) => pairing_payload(config, agent).map(|p| (agent.display_name.clone(), p)),
        None => install_payload(config).map(|p| ("BipAgents".to_string(), p)),
    };
    let (label, data) = match made {
        Ok(x) => x,
        Err(exc) => {
            eprintln!("config error: {exc}");
            return 2;
        }
    };
    let payload = py::Dumps::new().compact().unicode().dumps(&data);
    if args.json {
        println!("{payload}");
        return 0;
    }
    let code = match qrcode::QrCode::with_error_correction_level(payload.as_bytes(), qrcode::EcLevel::M) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!("\n{label} — contains keys: show it only to your own phone.\n");
    print!("{}", terminal_compact(&code, 4));
    if let Some(png) = args.png.as_deref() {
        if let Err(e) = save_png(&code, png, 8, 2) {
            eprintln!("{e}");
            return 1;
        }
        println!("saved: {png}");
    }
    0
}

fn dark_at(code: &qrcode::QrCode, border: usize, x: usize, y: usize) -> bool {
    let width = code.width();
    if x < border || y < border || x >= width + border || y >= width + border {
        return false;
    }
    code[(x - border, y - border)] == qrcode::Color::Dark
}

/// segno's `terminal(compact=True)`: two rows per line in half blocks, dark modules as the terminal's background.
pub fn terminal_compact(code: &qrcode::QrCode, border: usize) -> String {
    let size = code.width() + 2 * border;
    let mut out = String::new();
    let mut y = 0;
    while y < size {
        for x in 0..size {
            let top = dark_at(code, border, x, y);
            let bottom = if y + 1 < size { dark_at(code, border, x, y + 1) } else { true };
            out.push(match (top, bottom) {
                (true, true) => ' ',
                (false, true) => '\u{2580}',
                (true, false) => '\u{2584}',
                (false, false) => '\u{2588}',
            });
        }
        out.push('\n');
        y += 2;
    }
    out
}

/// segno's `save(path, scale, border)` as a PNG: black modules on white.
pub fn save_png(code: &qrcode::QrCode, path: &str, scale: usize, border: usize) -> anyhow::Result<()> {
    let size = (code.width() + 2 * border) * scale;
    let mut pixels = vec![255u8; size * size];
    for y in 0..size {
        for x in 0..size {
            if dark_at(code, border, x / scale, y / scale) {
                pixels[y * size + x] = 0;
            }
        }
    }
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size as u32, size as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(())
}

pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => futures::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let term = futures::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

const UVICORN: &str = "uvicorn.error";

/// uvicorn adds `server: uvicorn` to every response (`server_header=True`).
async fn server_header(mut response: axum::response::Response) -> axum::response::Response {
    if !response.headers().contains_key("server") {
        response.headers_mut().insert("server", axum::http::HeaderValue::from_static("uvicorn"));
    }
    response
}

/// `serve`: the usual listener (127.0.0.1, published by `tailscale serve`) plus, when `[lan]` is on, the LAN door
/// over HTTPS: one process and one set of services; the door starts once the main server runs, and stops with it.
/// uvicorn's own log lines, with their `color_message`, as the Python bridge writes them.
pub async fn serve(config: Config) -> i32 {
    let pid = std::process::id();
    tracing::info!(target: UVICORN, color_message = "Started server process [\u{1b}[36m%d\u{1b}[0m]", "Started server process [{pid}]");
    tracing::info!(target: UVICORN, "Waiting for application startup.");
    let lan = config.lan.clone();
    let (host, port) = (config.host.clone(), config.port);
    let tls = if lan.enabled {
        match crate::lan::ensure_certificate(&config) {
            Ok(paths) => Some(paths),
            Err(e) => {
                tracing::error!(target: UVICORN, exc = %format!("{e:#}"), "Application startup failed. Exiting.");
                return 3;
            }
        }
    } else {
        None
    };
    let app = match create_app(config, AppOptions::default()) {
        Ok(a) => a,
        Err(e) => {
            tracing::error!(target: UVICORN, exc = %format!("{e:#}"), "Application startup failed. Exiting.");
            return 3;
        }
    };
    tracing::info!(target: UVICORN, "Application startup complete.");
    let router = app.router.clone().layer(axum::middleware::map_response(server_header));
    let listener = match tokio::net::TcpListener::bind((host.as_str(), port)).await {
        Ok(l) => l,
        Err(e) => {
            let errno = e.raw_os_error().unwrap_or(0);
            let text = crate::config::os_error(&e).to_lowercase();
            tracing::error!(target: UVICORN, "[Errno {errno}] error while attempting to bind on address ('{host}', {port}): {text}");
            tracing::info!(target: UVICORN, "Waiting for application shutdown.");
            app.shutdown().await;
            tracing::info!(target: UVICORN, "Application shutdown complete.");
            return 3;
        }
    };
    tracing::info!(
        target: UVICORN,
        color_message = "Uvicorn running on \u{1b}[1m%s://%s:%d\u{1b}[0m (Press CTRL+C to quit)",
        "Uvicorn running on http://{host}:{port} (Press CTRL+C to quit)"
    );
    let stop = CancellationToken::new();
    let service = router.clone().into_make_service_with_connect_info::<SocketAddr>();
    let main_stop = stop.clone();
    let mut main_task = tokio::spawn(async move {
        axum::serve(listener, service).with_graceful_shutdown(main_stop.cancelled_owned()).await
    });
    let door = match tls {
        Some((cert, key)) => match serve_door(router.clone(), &lan.host, lan.port, &cert, &key, stop.clone()).await {
            Ok(task) => Some(task),
            Err(e) => {
                tracing::error!(target: UVICORN, "{e:#}");
                None
            }
        },
        None => None,
    };
    let ended_alone = tokio::select! {
        _ = shutdown_signal() => false,
        _ = &mut main_task => true,
    };
    tracing::info!(target: UVICORN, "Shutting down");
    stop.cancel();
    if !ended_alone && tokio::time::timeout(Duration::from_secs(5), &mut main_task).await.is_err() {
        tracing::error!(target: UVICORN, "Cancel 1 running task(s), timeout graceful shutdown exceeded");
        main_task.abort();
    }
    if let Some(door) = door {
        let _ = tokio::time::timeout(Duration::from_secs(5), door).await;
        tracing::info!(target: UVICORN, color_message = "Finished server process [\u{1b}[36m%d\u{1b}[0m]", "Finished server process [{pid}]");
    }
    tracing::info!(target: UVICORN, "Waiting for application shutdown.");
    app.shutdown().await;
    tracing::info!(target: UVICORN, "Application shutdown complete.");
    tracing::info!(target: UVICORN, color_message = "Finished server process [\u{1b}[36m%d\u{1b}[0m]", "Finished server process [{pid}]");
    0
}

/// The LAN door: the same app over HTTPS (HTTP/1.1, as uvicorn), each connection with its peer address. Its
/// uvicorn server has no lifespan: it logs its start and its address only.
pub async fn serve_door(
    router: Router,
    host: &str,
    port: u16,
    cert: &str,
    key: &str,
    stop: CancellationToken,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let acceptor = tls_acceptor(cert, key)?;
    let pid = std::process::id();
    tracing::info!(target: UVICORN, color_message = "Started server process [\u{1b}[36m%d\u{1b}[0m]", "Started server process [{pid}]");
    let listener = tokio::net::TcpListener::bind((host, port)).await?;
    tracing::info!(
        target: UVICORN,
        color_message = "Uvicorn running on \u{1b}[1m%s://%s:%d\u{1b}[0m (Press CTRL+C to quit)",
        "Uvicorn running on https://{host}:{port} (Press CTRL+C to quit)"
    );
    Ok(serve_door_on(listener, router, acceptor, stop))
}

/// The door's accept loop on a bound listener (tests bind 127.0.0.1:0).
pub fn serve_door_on(
    listener: tokio::net::TcpListener,
    router: Router,
    acceptor: tokio_rustls::TlsAcceptor,
    stop: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let (stream, peer) = tokio::select! {
                _ = stop.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok(x) => x,
                    Err(_) => continue,
                },
            };
            let (acceptor, router, stop) = (acceptor.clone(), router.clone(), stop.clone());
            tokio::spawn(async move {
                let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream)).await else {
                    return;
                };
                let service = hyper::service::service_fn(move |mut request: hyper::Request<hyper::body::Incoming>| {
                    request.extensions_mut().insert(ConnectInfo(peer));
                    request.extensions_mut().insert(crate::auth::Scheme("https"));
                    router.clone().oneshot(request.map(axum::body::Body::new))
                });
                let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
                let connection = builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(tls), service);
                tokio::select! {
                    _ = connection => {},
                    _ = stop.cancelled() => {},
                }
            });
        }
        tracing::info!(target: UVICORN, "Shutting down");
    })
}

pub fn tls_acceptor(cert: &str, key: &str) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
    let certs: Vec<CertificateDer<'static>> = pem::parse_many(std::fs::read(cert)?)?
        .into_iter()
        .filter(|p| p.tag() == "CERTIFICATE")
        .map(|p| CertificateDer::from(p.contents().to_vec()))
        .collect();
    let key_pem = pem::parse_many(std::fs::read(key)?)?
        .into_iter()
        .find(|p| p.tag() == "PRIVATE KEY")
        .ok_or_else(|| anyhow::anyhow!("no PKCS#8 private key in {key}"))?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pem.contents().to_vec()));
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, ArgsExit> {
        parse_args(&line.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn argparse_compatible() {
        let a = parse("browser --agent x add").unwrap();
        assert_eq!((a.command.as_str(), a.agent.as_deref(), a.action.as_deref()), ("browser", Some("x"), Some("add")));
        assert_eq!(parse("").unwrap().command, "serve");
        assert_eq!(parse("--conf /c check").unwrap().config.as_deref(), Some("/c"));
        assert_eq!(parse("--config=/c").unwrap().config.as_deref(), Some("/c"));
        assert_eq!(parse("--config a --config b").unwrap().config.as_deref(), Some("b"));
        assert_eq!(parse("-- check").unwrap().command, "check");
        assert_eq!(parse("--agent -5").unwrap().agent.as_deref(), Some("-5"));
        assert_eq!(parse("--agent=-x").unwrap().agent.as_deref(), Some("-x"));
        assert!(parse("--al").unwrap().all);
        assert_eq!(parse("-h"), Err(ArgsExit::Help));
        assert_eq!(parse("--he"), Err(ArgsExit::Help));
        let err = |l: &str| match parse(l) {
            Err(ArgsExit::Error(m)) => m,
            other => panic!("{l}: {other:?}"),
        };
        assert_eq!(err("add"), "argument command: invalid choice: 'add' (choose from serve, check, genkey, qr, peer-token, browser, shares-mounts, console, spaces, adopt, sandbox, agent-tools)");
        assert_eq!(err("-5"), "argument command: invalid choice: '-5' (choose from serve, check, genkey, qr, peer-token, browser, shares-mounts, console, spaces, adopt, sandbox, agent-tools)");
        assert_eq!(err("check extra"), "argument action: invalid choice: 'extra' (choose from add, remove, start)");
        assert_eq!(err("spaces start extra"), "unrecognized arguments: extra");
        assert_eq!(err("--bogus"), "unrecognized arguments: --bogus");
        assert_eq!(err("-x"), "unrecognized arguments: -x");
        assert_eq!(err("--agent"), "argument --agent: expected one argument");
        assert_eq!(err("--agent -x"), "argument --agent: expected one argument");
        assert_eq!(err("--json=1"), "argument --json: ignored explicit argument '1'");
        assert_eq!(err("--a x"), "ambiguous option: --a could match --agent, --all");
    }
}
