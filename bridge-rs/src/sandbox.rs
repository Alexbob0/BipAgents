//! Each agent's sandbox, isolated (owner decision 2026-10-22): no folder of this machine inside it, and no internet
//! but the official package registries. Hermes then runs the agent's harmless work there (`python -c`, a heredoc,
//! `execute_code`, `pip install`) without asking the person each time.
//!
//! Hermes' rule (tools/approval.py `_should_skip_container_guards`, tools/terminal_tool.py `_docker_has_host_access`,
//! at the commit the installer pins): a `docker` sandbox skips the approval prompts unless one of its
//! `terminal.docker_volumes` starts with `/`, `~`, `./`, `../` or a drive letter (a host path, read-only or not), or
//! `docker_mount_cwd_to_workspace` mounts the working folder. A named volume (`name:/path`) does not count. Inside an
//! isolated sandbox only the person's own `approvals.deny` rules still apply (not even Hermes' hardline blocks): what
//! stands between the agent and harm is the sandbox itself, hence no host folder and no open internet.
//!
//! What the bridge sets up, for an agent whose tools run in Hermes' docker sandbox on this machine (podman):
//!
//! - **the files the person sends it**: still stored in its uploads folder (backups, retention, the « Fichiers »
//!   list unchanged), and copied into a named volume of its own, `bipagents-files-<agent>`, mounted read-only at
//!   `/fichiers`: the same paths as before (`/fichiers/2026-10/…`). The copy follows the folder: a new file, a file
//!   removed after the time the person chose.
//! - **shared folders** (`shares.rs`: what an agent shares, what it may read): no longer mounted. The agent reaches them
//!   through the tools of a second MCP entry, `bipagents_files` (trust « untrusted »: Hermes asks the person before each
//!   write or delete, the reads are marked read-only): `shared_files_list`, `shared_files_get` (copies a file into
//!   `/fichiers/partages/<agent>/…`), `shared_files_write` (from `/envoi`, a second named volume it writes in, or a
//!   short text), `shared_files_delete`. The links' permissions are checked at each call.
//! - **the network**: the sandbox joins `bipagents-sandbox`, a podman network without a route out (`--internal`), with
//!   no DNS server. Its only way out is a small proxy (`bipagents-egress`, `assets/sandbox/egress_proxy.py`, in the
//!   sandbox image), on that network and the ordinary one, which lets through the registries of `[sandbox]
//!   registries` and nothing else; pip, npm, uv, cargo, go and apt find it through the usual proxy variables. When that
//!   can't be set up, the sandbox gets no network at all (`docker_network: false`), never the open internet, and the
//!   log says why; it is tried again every 10 minutes.
//!
//! Linux with podman only (`[sandbox] isolate = "auto"`): a podman volume is then a folder this account writes in. On
//! macOS (podman in a VM, or no sandbox at all: the installer turns the disk tools off there) and with docker, the
//! sandboxes keep mounting the folders as before (`agents_admin::mount_uploads`, `shares::apply`), and Hermes keeps
//! asking.
//!
//! `<data>/sandbox.json` keeps each agent's volumes and the network's state; each change is logged
//! (`bipbridge::sandbox`), and `bipbridge sandbox` applies it all and prints it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use serde_json::{json, Value};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::config::{AgentConfig, Config, SharedConfig};
use crate::tools_knowledge::tool_text;
use crate::web::ApiResult;
use crate::{py, shares, Dict};

/// The registries a sandbox reaches by default: Python, JavaScript, Rust, Go packages, Debian's. Only hosts that serve
/// packages: PyPI takes uploads on upload.pypi.org and crates.io's API publishes, so neither is listed, and RubyGems
/// (installs and pushes on one host) is left out; npm's registry does both, a risk accepted (2026-10-22: the agent's
/// visible browser is an easier way out anyway).
pub const REGISTRIES: [&str; 10] = [
    "pypi.org",
    "files.pythonhosted.org",
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "index.crates.io",
    "static.crates.io",
    "proxy.golang.org",
    "sum.golang.org",
    "deb.debian.org",
    "security.debian.org",
];
pub const NETWORK: &str = "bipagents-sandbox";
pub const PROXY: &str = "bipagents-egress";
pub const PROXY_PORT: u16 = 3128;
/// Hermes' default sandbox image (hermes_cli/config_defaults.py), with its registry (podman won't guess in a service).
pub const DEFAULT_IMAGE: &str = "docker.io/nousresearch/hermes-sandbox:desktop";
/// Where the agent finds the files it was sent, and the shared files it fetched (read-only).
pub const RECEIVED: &str = crate::files::SANDBOX_UPLOADS;
/// Where it puts a file to share (its own, read-write).
pub const OUTBOX: &str = "/envoi";
/// Under RECEIVED: the copies of shared files.
pub const COPIES: &str = "partages";
/// The MCP entry of the file tools, and where the bridge serves them.
pub const SERVER_NAME: &str = "bipagents_files";
pub const PATH: &str = "/v1/agent-tools/files";
/// The largest text `shared_files_write` takes as `content`.
pub const MAX_CONTENT: usize = 1024 * 1024;
const MAX_LISTED: usize = 400;
const RETRY: Duration = Duration::from_secs(600);
const SANDBOX_DIR_MODE: u32 = 0o755;
const SANDBOX_FILE_MODE: u32 = 0o644;

/// The sandbox's container arguments the bridge owns (`terminal.docker_extra_args`): its network, and no DNS server
/// (names are resolved by the proxy; nothing leaks through DNS queries).
pub fn own_args() -> [String; 2] {
    [format!("--network={NETWORK}"), "--dns=127.0.0.1".to_string()]
}

/// The proxy variables the bridge owns (`terminal.docker_env`).
pub const PROXY_ENV: [&str; 8] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "http_proxy",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
    "npm_config_proxy",
    "npm_config_https_proxy",
];

static PROXY_SCRIPT: &str = include_str!("../assets/sandbox/egress_proxy.py");

/// The tools' skill for an agent that shares or reads (`skills/shared-data/SKILL.tools.md`, embedded).
pub fn skill() -> &'static str {
    crate::assets::text(&crate::assets::SKILLS, "shared-data/SKILL.tools.md").unwrap_or("")
}

/// `10.233.80.0/24` → `10.233.80.2`: the proxy's address on the sandboxes' network.
pub fn proxy_ip(subnet: &str) -> Option<String> {
    let (network, bits) = subnet.trim().split_once('/')?;
    if bits != "24" {
        return None;
    }
    let parts: Vec<u8> = network.split('.').map(|p| p.parse::<u8>()).collect::<Result<_, _>>().ok()?;
    if parts.len() != 4 || parts[3] != 0 {
        return None;
    }
    Some(format!("{}.{}.{}.2", parts[0], parts[1], parts[2]))
}

pub fn files_volume(agent: &str) -> String {
    format!("bipagents-files-{agent}")
}

pub fn outbox_volume(agent: &str) -> String {
    format!("bipagents-envoi-{agent}")
}

/// podman, when this install isolates the sandboxes (`[sandbox] isolate`).
pub fn engine(config: &Config) -> Option<String> {
    let wanted = match config.sandbox.isolate.as_str() {
        "off" => false,
        "on" => true,
        _ => crate::browsers::system() == "Linux" && config.browser.runtime != "docker",
    };
    if !wanted {
        return None;
    }
    crate::browsers::pyx::which("podman")
}

/// Whether the bridge isolates this agent's sandbox: an agent it manages, whose tools run in Hermes' docker sandbox,
/// on an install that isolates. Agents set up by hand (`shares = "external"`) keep their own mounts.
pub fn isolates(config: &Config, agent: &AgentConfig) -> bool {
    if agent.shares == "external" || engine(config).is_none() {
        return false;
    }
    let Some(folder) = shares::profile_dir(config, agent) else { return false };
    folder.is_dir()
        && shares::terminal_backend(&folder).as_deref() == Some("docker")
        && !desktop_in_sandbox(config, &folder)
}

// --- state ---------------------------------------------------------------------------------------------------------

fn state_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("sandbox.json")
}

pub(crate) fn load_state(config: &Config) -> Dict {
    let Ok(text) = std::fs::read_to_string(state_path(config)) else { return Dict::new() };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

pub(crate) fn save_state(config: &Config, change: impl FnOnce(&mut Dict)) -> std::io::Result<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut state = load_state(config);
    change(&mut state);
    let path = state_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, format!("{}\n", py::Dumps::new().indent(1).dumps(&Value::Object(state))))?;
    std::fs::rename(&tmp, &path)
}

/// The host folders of an agent's volumes (`files`, `outbox`), once set up.
pub fn volumes_of(config: &Config, agent: &str) -> Option<(PathBuf, PathBuf)> {
    let state = load_state(config);
    let entry = state.get("agents")?.get(agent)?;
    let files = entry.get("files")?.as_str().filter(|s| !s.is_empty())?;
    let outbox = entry.get("outbox")?.as_str().filter(|s| !s.is_empty())?;
    Some((PathBuf::from(files), PathBuf::from(outbox)))
}

fn forget_state(config: &Config, agent: &str) {
    let _ = save_state(config, |state| {
        if let Some(Value::Object(agents)) = state.get_mut("agents") {
            agents.remove(agent);
        }
    });
}

// --- the network ---------------------------------------------------------------------------------------------------

/// The proxy's URL, or why the sandboxes have no network; and when that was found.
type NetState = Option<(Instant, Result<String, String>)>;

/// The sandboxes' network as last set up.
static NET: LazyLock<tokio::sync::Mutex<NetState>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The image the proxy runs in: `[sandbox] image`, else the hub's sandbox image, else Hermes' default.
fn proxy_image(config: &Config) -> String {
    if !config.sandbox.image.is_empty() {
        return config.sandbox.image.clone();
    }
    let home = if config.hermes.home.is_empty() {
        py::join(&py::expanduser("~"), ".hermes")
    } else {
        config.hermes.home.clone()
    };
    let hub = shares::profile_config(Path::new(&home));
    match hub.get("terminal").and_then(|t| t.get("docker_image")).and_then(Value::as_str) {
        Some(image) if !image.trim().is_empty() => {
            let image = image.trim();
            // podman refuses a short name in a service: Hermes' own images come from Docker Hub
            if image.starts_with("nousresearch/") || image.starts_with("nikolaik/") {
                format!("docker.io/{image}")
            } else {
                image.to_string()
            }
        }
        _ => DEFAULT_IMAGE.to_string(),
    }
}

/// The `podman run` of the registries' proxy (replacing the one before: its allow-list may have changed).
pub fn proxy_command(config: &Config, podman: &str) -> Vec<String> {
    let ip = proxy_ip(&config.sandbox.subnet).unwrap_or_default();
    let allow = config.sandbox.registries.join(",");
    let mut command = args(&[
        podman,
        "run",
        "-d",
        "--replace",
        "--name",
        PROXY,
        "--restart=always",
        "--network",
        "podman",
        "--network",
        &format!("{NETWORK}:ip={ip}"),
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--memory",
        "96m",
        "--pids-limit",
        "64",
        "--label",
        "bipagents.role=egress",
        "-e",
        &format!("BIP_ALLOW={allow}"),
        "-e",
        &format!("BIP_PORT={PROXY_PORT}"),
        "--entrypoint",
        "python3",
    ]);
    command.push(proxy_image(config));
    command.extend(args(&["-u", "-c", PROXY_SCRIPT]));
    command
}

async fn set_up_network(config: &Config, run: &Run, podman: &str) -> Result<String, String> {
    let ip = proxy_ip(&config.sandbox.subnet).ok_or("no valid [sandbox] subnet")?;
    let none = BTreeMap::new();
    if run(args(&[podman, "network", "exists", NETWORK]), none.clone()).await.is_err() {
        run(
            args(&[podman, "network", "create", "--internal", "--subnet", &config.sandbox.subnet, NETWORK]),
            none.clone(),
        )
        .await
        .map_err(|e| format!("network not created: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
        tracing::info!(target: "bipbridge::sandbox", network = NETWORK, subnet = %config.sandbox.subnet, "sandboxes' network created (no route out)");
    }
    run(proxy_command(config, podman), none)
        .await
        .map_err(|e| format!("registries' proxy not started: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
    tracing::info!(target: "bipbridge::sandbox", proxy = PROXY, registries = %config.sandbox.registries.join(","), "registries' proxy started");
    Ok(format!("http://{ip}:{PROXY_PORT}"))
}

/// The proxy's URL for the sandboxes, setting the network and the proxy up once per start (again after a failure,
/// once RETRY has passed); Err: why the sandboxes get no network.
pub async fn network(config: &Config, run: &Run) -> Result<String, String> {
    let Some(podman) = engine(config) else { return Err("no podman".into()) };
    let mut held = NET.lock().await;
    if let Some((at, result)) = held.as_ref() {
        if result.is_ok() || at.elapsed() < RETRY {
            return result.clone();
        }
    }
    let result = set_up_network(config, run, &podman).await;
    if let Err(why) = &result {
        tracing::warn!(target: "bipbridge::sandbox", reason = %why, "sandboxes without network: no package can be installed (tried again in 10 minutes)");
    }
    *held = Some((Instant::now(), result.clone()));
    result
}

/// Tests: the network set up again at the next call.
pub async fn reset_network() {
    *NET.lock().await = None;
}

// --- an agent ------------------------------------------------------------------------------------------------------

/// A `docker_volumes` entry the bridge owns (today's or the mounts it used to set): its uploads, shared folders and
/// volumes.
pub(crate) fn owned_volume(config: &Config, entry: &str) -> bool {
    let root = shares::shares_root(config).to_string_lossy().into_owned();
    let target = entry.split(':').nth(1).unwrap_or("");
    entry.starts_with(&format!("{root}/"))
        || [RECEIVED, OUTBOX, "/partage"].contains(&target)
        || target.starts_with("/lecture/")
}

/// What a sandbox reaches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Net<'a> {
    /// The registries only, through the proxy at this URL.
    Registries(&'a str),
    /// Nothing (the proxy couldn't be set up).
    Closed,
}

/// Hermes' Bot Desktop runs in the agent's sandbox: an install without the visible browser's containers
/// (`[browser] enabled = false`) whose agents browse with the desktop Hermes starts in their sandbox
/// (`bot_desktop.auto_start`, the installer's setup_desktop). Its browser needs the internet, so that sandbox is not
/// isolated at all (owner decision 2026-10-22): its folders stay mounted as before and Hermes keeps asking the person
/// before risky commands, rather than running code with the open internet and no question asked.
pub fn desktop_in_sandbox(config: &Config, folder: &Path) -> bool {
    if config.browser.enabled {
        return false;
    }
    let home = folder.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    for dir in [folder.to_path_buf(), home] {
        if let Some(Value::Object(desktop)) = shares::profile_config(&dir).get("bot_desktop") {
            if desktop.get("placement").and_then(Value::as_str) == Some("gateway") {
                return false;
            }
            if let Some(on) = desktop.get("auto_start") {
                return py::truthy(on);
            }
        }
    }
    false
}

/// The terminal settings an isolated sandbox gets, from the profile's current ones: the person's own entries kept.
pub fn wanted_terminal(config: &Config, agent: &str, current: &Dict, net: Net) -> Dict {
    let list = |key: &str| -> Vec<String> {
        match current.get(key) {
            Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
            _ => Vec::new(),
        }
    };
    let mut volumes: Vec<String> = list("docker_volumes").into_iter().filter(|v| !owned_volume(config, v)).collect();
    volumes.push(format!("{}:{RECEIVED}:ro", files_volume(agent)));
    volumes.push(format!("{}:{OUTBOX}", outbox_volume(agent)));
    let ours = own_args();
    let mut extra: Vec<String> = list("docker_extra_args").into_iter().filter(|a| !ours.contains(a)).collect();
    let mut env: Dict = match current.get("docker_env") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    env.retain(|k, _| !PROXY_ENV.contains(&k.as_str()));
    if let Net::Registries(url) = net {
        extra.extend(ours);
        for key in PROXY_ENV {
            let value =
                if key.to_lowercase() == "no_proxy" { "localhost,127.0.0.1".to_string() } else { url.to_string() };
            env.insert(key.to_string(), json!(value));
        }
    }
    let mut out = Dict::new();
    out.insert("docker_volumes".into(), json!(volumes));
    out.insert("docker_extra_args".into(), json!(extra));
    out.insert("docker_env".into(), Value::Object(env));
    out.insert("docker_network".into(), json!(net != Net::Closed));
    out
}

/// Host paths still mounted in the sandbox (entries the person added by hand): Hermes keeps asking because of them.
pub fn host_mounts(volumes: &[String]) -> Vec<String> {
    volumes
        .iter()
        .filter(|v| {
            let v = v.trim();
            v.starts_with('/') || v.starts_with('~') || v.starts_with("./") || v.starts_with("../")
        })
        .cloned()
        .collect()
}

pub(crate) async fn mountpoint(run: &Run, podman: &str, volume: &str) -> anyhow::Result<PathBuf> {
    let none = BTreeMap::new();
    run(args(&[podman, "volume", "create", "--ignore", volume]), none.clone()).await?;
    let out = run(args(&[podman, "volume", "inspect", "--format", "{{.Mountpoint}}", volume]), none).await?;
    let path = out.trim();
    if path.is_empty() || !path.starts_with('/') {
        anyhow::bail!("podman gave no folder for the volume {volume}");
    }
    Ok(PathBuf::from(path))
}

/// The agent's sandbox isolated: its volumes, its uploads copied in, its terminal settings, the file tools' skill when
/// it shares or reads. Returns the agent as the bridge now serves it (uploads seen at /fichiers). Err: a podman or
/// `hermes config set` that failed (the agent then keeps what it had).
pub async fn isolate(shared: &SharedConfig, agent: &AgentConfig, run: &Run, save: bool) -> anyhow::Result<AgentConfig> {
    let config = shared.get();
    let Some(podman) = engine(&config) else { return Ok(agent.clone()) };
    let Some(folder) = shares::profile_dir(&config, agent) else { return Ok(agent.clone()) };
    let (files, outbox) = (
        mountpoint(run, &podman, &files_volume(&agent.name)).await?,
        mountpoint(run, &podman, &outbox_volume(&agent.name)).await?,
    );
    save_state(&config, |state| {
        let agents = state.entry("agents").or_insert_with(|| json!({}));
        if !agents.is_object() {
            *agents = json!({});
        }
        agents[agent.name.as_str()] = json!({"files": files.to_string_lossy(), "outbox": outbox.to_string_lossy()});
    })?;
    let (c, name) = (config.clone(), agent.name.clone());
    let (added, removed) = tokio::task::spawn_blocking(move || sync_agent(&c, &name)).await??;
    // The code an adopted agent runs (its skills, its bin…), refreshed from the folders the person keeps it in.
    let (c, name) = (config.clone(), agent.name.clone());
    tokio::task::spawn_blocking(move || crate::sandbox_adopt::refresh(&c, &name)).await?;
    let proxy = network(&config, run).await.ok();
    let net = match &proxy {
        Some(url) => Net::Registries(url),
        None => Net::Closed,
    };
    let current = match shares::profile_config(&folder).get("terminal") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    let wanted = wanted_terminal(&config, &agent.name, &current, net);
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env = BTreeMap::from([("HERMES_HOME".to_string(), home)]);
    let mut changed: Vec<String> = Vec::new();
    for (key, value) in &wanted {
        let now = current.get(key).cloned().unwrap_or(Value::Null);
        let same = match (key.as_str(), &now) {
            ("docker_network", Value::Null) => value == &json!(true), // Hermes' default
            _ => &now == value,
        };
        if same {
            continue;
        }
        let shown = match value {
            Value::Bool(b) => b.to_string(),
            other => py::dumps(other),
        };
        run(
            args(&[&config.hermes.bin, "-p", &agent.name, "config", "set", &format!("terminal.{key}"), &shown]),
            env.clone(),
        )
        .await?;
        changed.push(key.clone());
    }
    if !changed.is_empty() {
        let before: Vec<String> = match current.get("docker_volumes") {
            Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
            _ => Vec::new(),
        };
        tracing::info!(
            target: "bipbridge::sandbox",
            agent = %agent.name,
            changed = %changed.join(","),
            mounts_before = %before.join(" "),
            network = match net {
                Net::Registries(_) => "registries only",
                Net::Closed => "none",
            },
            "sandbox isolated: no folder of this machine inside, the files through the bridge's tools"
        );
    }
    let kept = host_mounts(&match wanted.get("docker_volumes") {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    if !kept.is_empty() {
        tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, mounts = %kept.join(" "), "the sandbox still mounts folders added by hand: Hermes keeps asking before risky commands");
    }
    if added + removed > 0 {
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, added, removed, "received files copied into the sandbox");
    }
    let reads = shares::load(&config);
    let (c, name) = (config.clone(), agent.name.clone());
    let linked = reads.iter().any(|(a, b)| a == &agent.name || b == &agent.name);
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        drop_stale_copies(&c, &name, &reads);
        let skill_dir = folder.join("skills").join("shared-data");
        if linked {
            std::fs::create_dir_all(&skill_dir)?;
            std::fs::write(skill_dir.join("SKILL.md"), skill())?;
        }
        Ok(())
    })
    .await??;
    let mut agent = agent.clone();
    if agent.upload_dir_container.as_deref() != Some(RECEIVED) {
        agent.upload_dir_container = Some(RECEIVED.to_string());
        if save {
            crate::agents_admin::save_agent(shared, &agent)?;
        }
    }
    Ok(agent)
}

/// At start: every sandboxed agent isolated (or its uploads mounted as before where the install doesn't isolate);
/// while the network can't be set up, tried again every 10 minutes.
pub async fn isolate_all(config: SharedConfig, run: Run) {
    crate::agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
    let current = config.get();
    let any = current.agents.values().any(|a| isolates(&current, a));
    if !any {
        return;
    }
    while network(&config.get(), &run).await.is_err() {
        tokio::time::sleep(RETRY + Duration::from_secs(5)).await;
        if network(&config.get(), &run).await.is_ok() {
            tracing::info!(target: "bipbridge::sandbox", "sandboxes' network up: package registries reachable again");
            crate::agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
            break;
        }
    }
}

/// A deleted agent: its volumes go (the bridge kept the originals of what they held).
pub async fn forget(config: &Config, agent: &str, run: &Run) {
    let Some(podman) = engine(config) else { return };
    if volumes_of(config, agent).is_none() {
        return;
    }
    for volume in [files_volume(agent), outbox_volume(agent)] {
        if let Err(exc) = run(args(&[&podman, "volume", "rm", "-f", &volume]), BTreeMap::new()).await {
            tracing::warn!(target: "bipbridge::sandbox", agent, error = %crate::agents_admin::tail(&exc.to_string(), 200), "sandbox volume not removed");
        }
    }
    forget_state(config, agent);
}

// --- the files in the volume ---------------------------------------------------------------------------------------

/// A regular file opened for reading, never through a symbolic link (O_NOFOLLOW: a link swapped in after the checks
/// fails here), nor a pipe or a device.
pub fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn copy_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let tmp = target.with_file_name(format!(
        ".{}.part",
        target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let result = (|| {
        let mut input = open_plain(source)?;
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut input, &mut out)?;
        out.set_permissions(std::fs::Permissions::from_mode(SANDBOX_FILE_MODE))?;
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn make_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SANDBOX_DIR_MODE))
}

/// The bridge's stored uploads (`<month>/<id12>-<name>`), as relative paths.
fn stored_files(root: &Path) -> BTreeMap<PathBuf, u64> {
    use crate::files::{MONTH_DIR, STORED_NAME};
    let mut found = BTreeMap::new();
    let Ok(months) = std::fs::read_dir(root) else { return found };
    for month in months.flatten() {
        let name = month.file_name().to_string_lossy().into_owned();
        let Ok(kind) = month.file_type() else { continue };
        if !MONTH_DIR.is_match(&name) || !kind.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(month.path()) else { continue };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if STORED_NAME.is_match(&file) && meta.is_file() {
                found.insert(PathBuf::from(&name).join(&file), meta.len());
            }
        }
    }
    found
}

/// The volume holds the agent's uploads, no more no less (the shared files' copies left alone): (added, removed).
pub fn sync_uploads(uploads: &Path, volume: &Path) -> std::io::Result<(usize, usize)> {
    let wanted = stored_files(uploads);
    let present = stored_files(volume);
    let (mut added, mut removed) = (0, 0);
    for (relative, size) in &wanted {
        if present.get(relative) == Some(size) {
            continue;
        }
        let target = volume.join(relative);
        if let Some(parent) = target.parent() {
            make_dir(parent)?;
        }
        copy_file(&uploads.join(relative), &target)?;
        added += 1;
    }
    for relative in present.keys() {
        if !wanted.contains_key(relative) && std::fs::remove_file(volume.join(relative)).is_ok() {
            removed += 1;
        }
    }
    let months: BTreeSet<PathBuf> = present.keys().filter_map(|p| p.parent().map(Path::to_path_buf)).collect();
    for month in months {
        let dir = volume.join(month);
        if std::fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&dir);
        }
    }
    Ok((added, removed))
}

/// The agent's uploads copied into its volume (after an upload, a purge, at start). Nothing for an agent the bridge
/// doesn't isolate.
pub fn sync_agent(config: &Config, agent: &str) -> std::io::Result<(usize, usize)> {
    let Some((files, _)) = volumes_of(config, agent) else { return Ok((0, 0)) };
    let Some(uploads) = config.agent(Some(agent)).and_then(|a| a.upload_dir_host.clone()).filter(|u| !u.is_empty())
    else {
        return Ok((0, 0));
    };
    if !files.is_dir() {
        return Ok((0, 0));
    }
    sync_uploads(Path::new(&uploads), &files)
}

/// Copies of what the agent may no longer read, removed from its volume.
pub fn drop_stale_copies(config: &Config, agent: &str, reads: &[(String, String)]) {
    let Some((files, _)) = volumes_of(config, agent) else { return };
    let Ok(entries) = std::fs::read_dir(files.join(COPIES)) else { return };
    for entry in entries.flatten() {
        let source = entry.file_name().to_string_lossy().into_owned();
        if !may_read(agent, &source, reads) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

// --- the file tools ------------------------------------------------------------------------------------------------

pub const LIST: &str = "shared_files_list";
pub const GET: &str = "shared_files_get";
pub const WRITE: &str = "shared_files_write";
pub const DELETE: &str = "shared_files_delete";

fn may_read(agent: &str, source: &str, reads: &[(String, String)]) -> bool {
    source == agent || reads.iter().any(|(r, s)| r == agent && s == source)
}

/// The tools' definitions. Reads are marked read-only: on this « untrusted » entry, Hermes asks the person before the
/// others.
pub fn tools() -> Vec<Value> {
    let read_only = json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false});
    vec![
        json!({
            "name": LIST,
            "description": "List the shared folders you can use: yours (the agents the person allowed read it) and \
        those other agents share with you. The files the person sends you are already in /fichiers (read-only).",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": read_only,
        }),
        json!({
            "name": GET,
            "description": format!("Copy a file from a shared folder into your sandbox, at {RECEIVED}/{COPIES}/<agent>/<path> \
        (read-only: copy it to your workspace to change it)."),
            "inputSchema": {"type": "object", "properties": {
                "agent": {"type": "string", "description": "whose shared folder (yours if left out), as shared_files_list names it"},
                "path": {"type": "string", "description": "the file's path in that folder"},
            }, "required": ["path"]},
            "annotations": read_only,
        }),
        json!({
            "name": WRITE,
            "description": format!("Put a file in your shared folder, for the agents the person allowed (they only \
        read it). Copy the file right into {OUTBOX} (no folder) and give its path as `from`, or give a short text as `content`. The person \
        approves it."),
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "where in your shared folder, e.g. resume/semaine.md"},
                "from": {"type": "string", "description": format!("{OUTBOX}/<name>: a file you copied there")},
                "content": {"type": "string", "description": "or the text itself"},
            }, "required": ["path"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
        json!({
            "name": DELETE,
            "description": "Delete a file or folder from your shared folder. The person approves it.",
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "its path in your shared folder"},
            }, "required": ["path"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false},
        }),
    ]
}

/// A relative path made of plain names (no `..`, no root, no hidden `.`/`..`): None otherwise.
pub fn plain_relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in Path::new(path.trim()).components() {
        match part {
            Component::Normal(name) => out.push(name),
            Component::RootDir | Component::CurDir => continue,
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty() && out.components().count() <= 16).then_some(out)
}

/// `root/relative`, when no part of it on the way is a symbolic link (a link made in the sandbox would otherwise
/// lead the bridge to this machine's files).
pub fn inside(root: &Path, relative: &Path) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        path.push(part);
        match path.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => return None,
            Ok(_) => {}
            Err(_) => {} // not there yet (a file to write): the rest can't be a link either
        }
    }
    Some(path)
}

fn human(size: u64) -> String {
    match size {
        s if s >= 1024 * 1024 => format!("{:.1} MB", s as f64 / (1024.0 * 1024.0)),
        s if s >= 1024 => format!("{:.1} KB", s as f64 / 1024.0),
        s => format!("{s} B"),
    }
}

fn listing(root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(root.join(&dir)) else { continue };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let relative = dir.join(entry.file_name());
            let Ok(meta) = entry.path().symlink_metadata() else { continue };
            if meta.is_dir() {
                stack.push(relative);
            } else if meta.is_file() {
                lines.push(format!("- {} ({})", relative.to_string_lossy(), human(meta.len())));
            }
            if lines.len() >= MAX_LISTED {
                lines.push("- … (more)".into());
                return lines;
            }
        }
    }
    lines
}

fn error(text: &str) -> Dict {
    tool_text(text, true)
}

/// One call of a file tool, for `agent` (blocking: files only).
pub fn call(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> Dict {
    let reads = shares::load(config);
    let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let root = shares::shares_root(config);
    match name {
        LIST => {
            let mut out = vec![format!("Files the person sent you: in {RECEIVED} (read-only).")];
            let readers: Vec<&str> = reads.iter().filter(|(_, s)| s == &agent.name).map(|(r, _)| r.as_str()).collect();
            let own = listing(&root.join(&agent.name));
            out.push(format!(
                "\nYour shared folder (agent `{}`){}: {}",
                agent.name,
                if readers.is_empty() {
                    ", read by no agent yet".to_string()
                } else {
                    format!(", read by {}", readers.join(", "))
                },
                if own.is_empty() { "empty".to_string() } else { format!("{} file(s)", own.len()) }
            ));
            out.extend(own);
            for (_, source) in reads.iter().filter(|(r, _)| r == &agent.name) {
                let shown =
                    config.agent(Some(source)).map(|a| a.display_name.clone()).unwrap_or_else(|| source.clone());
                let files = listing(&root.join(source));
                out.push(format!(
                    "\nShared with you by {shown} (agent `{source}`): {}",
                    if files.is_empty() { "empty".to_string() } else { format!("{} file(s)", files.len()) }
                ));
                out.extend(files);
            }
            tool_text(&out.join("\n"), false)
        }
        GET => {
            let source = Some(text("agent")).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| agent.name.clone());
            let source = source.trim().to_lowercase();
            if !may_read(&agent.name, &source, &reads) {
                return error(&format!("{source} shares nothing with you (the person decides in the app)."));
            }
            let Some(relative) = plain_relative(&text("path")) else {
                return error("give the file's path, as shared_files_list shows it");
            };
            let Some(file) = inside(&root.join(&source), &relative) else { return error("no such file") };
            if !file.symlink_metadata().map(|m| m.is_file()).unwrap_or(false) {
                return error("no such file (give a file, as shared_files_list shows it)");
            }
            let Some((files, _)) = volumes_of(config, &agent.name) else {
                return error("your sandbox isn't ready for files yet; try again in a moment");
            };
            let copies = files.join(COPIES).join(&source);
            let Some(target) = inside(&copies, &relative) else { return error("no such file") };
            let done = (|| -> std::io::Result<()> {
                make_dir(&files.join(COPIES))?;
                if let Some(parent) = target.parent() {
                    make_dir(parent)?;
                }
                copy_file(&file, &target)
            })();
            match done {
                Ok(()) => tool_text(
                    &format!("Copied to {RECEIVED}/{COPIES}/{source}/{} (read-only).", relative.to_string_lossy()),
                    false,
                ),
                Err(exc) => error(&format!("could not copy it: {exc}")),
            }
        }
        WRITE | DELETE => {
            if let Some(why) = shares::why_not(config, agent) {
                return error(&format!("you have no shared folder here ({why})"));
            }
            let Some(relative) = plain_relative(&text("path")) else {
                return error("give a path in your shared folder");
            };
            let own = root.join(&agent.name);
            let Some(target) = inside(&own, &relative) else { return error("not a path in your shared folder") };
            if name == DELETE {
                let meta = match target.symlink_metadata() {
                    Ok(meta) => meta,
                    Err(_) => return error("no such file in your shared folder"),
                };
                let gone = if meta.is_dir() { std::fs::remove_dir_all(&target) } else { std::fs::remove_file(&target) };
                return match gone {
                    Ok(()) => {
                        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, path = %relative.to_string_lossy(), "shared file deleted");
                        tool_text(&format!("Deleted from your shared folder: {}", relative.to_string_lossy()), false)
                    }
                    Err(exc) => error(&format!("could not delete it: {exc}")),
                };
            }
            let (from, content) = (text("from"), arguments.get("content").and_then(Value::as_str));
            let written = if !from.trim().is_empty() {
                let Some(rest) = from.trim().strip_prefix(OUTBOX).filter(|r| r.starts_with('/')) else {
                    return error(&format!("`from` must be a file under {OUTBOX}: copy it there first"));
                };
                let Some((_, outbox)) = volumes_of(config, &agent.name) else {
                    return error("your sandbox isn't ready for files yet; try again in a moment");
                };
                // Right in /envoi, no folder on the way: nothing the sandbox could swap for a link meanwhile.
                let name = plain_relative(rest).filter(|r| r.components().count() == 1);
                let Some(source) = name.and_then(|r| inside(&outbox, &r)) else {
                    return error(&format!("put the file right in {OUTBOX} (no folder), then give {OUTBOX}/<name>"));
                };
                let limit = config.limits.upload_max_bytes;
                let input = match open_plain(&source) {
                    Ok(file) if file.metadata().map(|m| m.len() <= limit).unwrap_or(false) => file,
                    Ok(_) => return error("this file is too large to share"),
                    Err(_) => return error(&format!("no such file in {OUTBOX}")),
                };
                write_into(&own, &target, |out| {
                    use std::io::Read;
                    let copied = std::io::copy(&mut input.take(limit + 1), out)?;
                    if copied > limit {
                        return Err(std::io::Error::other("too large"));
                    }
                    Ok(())
                })
            } else if let Some(content) = content {
                if content.len() > MAX_CONTENT {
                    return error(&format!("too long: put it in a file under {OUTBOX} and give `from`"));
                }
                write_into(&own, &target, |out| out.write_all(content.as_bytes()))
            } else {
                return error(&format!("give `from` (a file under {OUTBOX}) or `content`"));
            };
            match written {
                Ok(()) => {
                    tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, path = %relative.to_string_lossy(), "shared file written");
                    tool_text(&format!("In your shared folder: {}", relative.to_string_lossy()), false)
                }
                Err(exc) => error(&format!("could not write it: {exc}")),
            }
        }
        _ => error(&format!("unknown tool: {name}")),
    }
}

/// A file written into the agent's shared folder (readable by the sandboxes of agents set up by hand that mount it).
fn write_into(
    own: &Path,
    target: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(own)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
        if parent != own {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(SANDBOX_DIR_MODE))?;
        }
    }
    if target.symlink_metadata().map(|m| m.is_dir()).unwrap_or(false) {
        return Err(std::io::Error::other("a folder has this name"));
    }
    let tmp = target.with_file_name(format!(".bip-{}.part", py::token_urlsafe(6)));
    let result = (|| {
        let mut out = std::fs::File::create(&tmp)?;
        fill(&mut out)?;
        out.set_permissions(std::fs::Permissions::from_mode(SANDBOX_FILE_MODE))?;
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

const INSTRUCTIONS: &str = "BipAgents files: the files your person sends you are in /fichiers (read-only). Shared \
folders between agents: shared_files_list, shared_files_get (copies into /fichiers/partages/), and to share a file, \
put it in /envoi then shared_files_write; the person approves writes and deletes. To give files to one of your \
other agents, put them in /envoi then use send_files_to_agent (bipagents tools). To give a file to your person, \
put it in /envoi and end your reply with a line MEDIA:/envoi/<name>; a page goes through create_page. Your \
sandbox reaches the package registries (pip, npm…) and nothing else on the internet: use your web tools for the \
rest.";

/// The file tools' MCP endpoint (same token as the bridge's other tools).
async fn mcp(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();
    let config = state.config();
    let agent = if token.is_empty() { None } else { crate::agent_tools::agent_for(&config, token) };
    let Some(agent) = agent else { return Err(crate::web::HttpError::new(401, "unknown agent")) };
    let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&body) else {
        return Ok(crate::web::json_response(
            400,
            &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}),
        ));
    };
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    if id.is_null() {
        let mut response = Response::new(axum::body::Body::empty());
        *response.status_mut() = axum::http::StatusCode::ACCEPTED;
        return Ok(response);
    }
    let reply = |result: Value| crate::web::json_response(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}));
    let params = message.get("params").cloned().unwrap_or(json!({}));
    match message.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18").to_string();
            Ok(reply(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": crate::VERSION},
                "instructions": INSTRUCTIONS,
            })))
        }
        "ping" => Ok(reply(json!({}))),
        "tools/list" => Ok(reply(json!({"tools": tools()}))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let arguments = match params.get("arguments") {
                Some(Value::Object(map)) => map.clone(),
                _ => Dict::new(),
            };
            let result = tokio::task::spawn_blocking(move || call(&config, &agent, &name, &arguments))
                .await
                .map_err(anyhow::Error::from)?;
            Ok(reply(Value::Object(result)))
        }
        other => Ok(crate::web::json_response(
            200,
            &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {other}")}}),
        )),
    }
}

/// The file tools' route. Mounted at the root, next to the other agent tools.
pub fn router() -> Router<AppState> {
    Router::new().route(PATH, post(mcp))
}

/// Where the agent's Hermes reaches the file tools (the bridge's tools' base, `?v=` their version).
pub fn tools_url(config: &Config) -> String {
    let base = crate::agent_tools::url_with(config, "");
    let base = base.split('?').next().unwrap_or("").trim_end_matches(crate::agent_tools::PATH).to_string();
    format!("{base}{PATH}?v={}", crate::agent_tools::version_of(&tools()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_proxy_takes_the_second_address() {
        assert_eq!(proxy_ip("10.233.80.0/24").as_deref(), Some("10.233.80.2"));
        assert_eq!(proxy_ip("10.233.80.0/16"), None);
        assert_eq!(proxy_ip("10.233.80.1/24"), None);
        assert_eq!(proxy_ip("nonsense"), None);
    }

    #[test]
    fn only_plain_relative_paths() {
        assert_eq!(plain_relative("notes/a.md"), Some(PathBuf::from("notes/a.md")));
        assert_eq!(plain_relative("/notes/./a.md"), Some(PathBuf::from("notes/a.md")));
        assert_eq!(plain_relative("../a.md"), None);
        assert_eq!(plain_relative("a/../../b"), None);
        assert_eq!(plain_relative(""), None);
        assert_eq!(plain_relative("/"), None);
    }

    #[test]
    fn a_link_never_leads_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("envoi");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("etc")).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", root.join("sub/hosts")).unwrap();
        assert!(inside(&root, Path::new("etc/hosts")).is_none());
        assert!(inside(&root, Path::new("sub/hosts")).is_none());
        assert_eq!(inside(&root, Path::new("sub/new.txt")), Some(root.join("sub/new.txt")));
    }

    #[test]
    fn the_volume_follows_the_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let (uploads, volume) = (dir.path().join("uploads"), dir.path().join("volume"));
        std::fs::create_dir_all(uploads.join("2026-10")).unwrap();
        std::fs::write(uploads.join("2026-10/0123456789ab-a.pdf"), b"a").unwrap();
        std::fs::write(uploads.join("2026-10/notes.txt"), b"not the bridge's").unwrap();
        std::fs::create_dir_all(volume.join("2026-09")).unwrap();
        std::fs::write(volume.join("2026-09/0123456789ac-old.pdf"), b"old").unwrap();
        std::fs::create_dir_all(volume.join("partages/vie")).unwrap();
        std::fs::write(volume.join("partages/vie/x.md"), b"kept").unwrap();
        assert_eq!(sync_uploads(&uploads, &volume).unwrap(), (1, 1));
        assert_eq!(std::fs::read(volume.join("2026-10/0123456789ab-a.pdf")).unwrap(), b"a");
        assert!(!volume.join("2026-10/notes.txt").exists() && !volume.join("2026-09").exists());
        assert!(volume.join("partages/vie/x.md").exists());
        assert_eq!(sync_uploads(&uploads, &volume).unwrap(), (0, 0));
    }

    #[test]
    fn the_terminal_keeps_the_persons_own_settings() {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = "/data/bridge.db".into();
        let current: Dict = serde_json::from_value(json!({
            "docker_volumes": ["/home/x/photos:/photos:ro", "/up/vie/uploads:/fichiers:ro,z",
                               "/data/shares/vie:/partage:z", "/data/shares/w:/lecture/w:ro,z"],
            "docker_extra_args": ["--shm-size=2g", "--network=bipagents-sandbox"],
            "docker_env": {"LANG": "fr_FR.UTF-8", "HTTPS_PROXY": "http://old"},
        }))
        .unwrap();
        let wanted = wanted_terminal(&config, "vie", &current, Net::Registries("http://10.233.80.2:3128"));
        assert_eq!(
            wanted["docker_volumes"],
            json!(["/home/x/photos:/photos:ro", "bipagents-files-vie:/fichiers:ro", "bipagents-envoi-vie:/envoi"])
        );
        assert_eq!(
            wanted["docker_extra_args"],
            json!(["--shm-size=2g", "--network=bipagents-sandbox", "--dns=127.0.0.1"])
        );
        assert_eq!(wanted["docker_env"]["LANG"], json!("fr_FR.UTF-8"));
        assert_eq!(wanted["docker_env"]["HTTPS_PROXY"], json!("http://10.233.80.2:3128"));
        assert_eq!(wanted["docker_env"]["no_proxy"], json!("localhost,127.0.0.1"));
        assert_eq!(wanted["docker_network"], json!(true));
        assert_eq!(
            host_mounts(&["/home/x/photos:/photos:ro".into(), "bipagents-files-vie:/fichiers:ro".into()]).len(),
            1
        );
        // No proxy: no network at all, never the open internet.
        let closed = wanted_terminal(&config, "vie", &current, Net::Closed);
        assert_eq!(closed["docker_extra_args"], json!(["--shm-size=2g"]));
        assert_eq!(closed["docker_env"], json!({"LANG": "fr_FR.UTF-8"}));
        assert_eq!(closed["docker_network"], json!(false));
    }
}
