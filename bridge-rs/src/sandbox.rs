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
//! - **shared folders** (`shares.rs`: what an agent shares, what it may read): podman volumes too,
//!   `bipagents-share-<agent>`, mounted read-write at `/partage` in the source's sandbox and read-only at
//!   `/lecture/<agent>` in the sandboxes of the readers the person linked (2026-10-23: from 2026-10-22 they were not
//!   mounted at all, and a routine sharing at night had nobody to approve its writes). The same folders through the
//!   tools of a second MCP entry, `bipagents_files` (trust « untrusted »: Hermes asks the person before a tool not
//!   marked read-only): `shared_files_list`, `shared_files_get` (copies a file into `/fichiers/partages/<agent>/…`),
//!   `shared_files_write` (from `/envoi`, a second named volume it writes in, or a short text; into its own shared
//!   folder only, which it writes directly anyway: marked read-only, so never asked), `shared_files_delete` (asked).
//!   The links' permissions are checked at each call. Every file of these folders is reached through folder
//!   descriptors, one name at a time and never through a link (beneath.rs; security audit 2026-10-27, H1): the
//!   sandbox writes in them and can swap a folder for a link at any moment.
//! - **the network**: the sandbox joins `bipagents-sandbox`, a podman network without a route out (`--internal`), with
//!   no DNS server. Its only way out is a small proxy (`bipagents-egress`, `assets/sandbox/egress_proxy.py`, in the
//!   sandbox image), on that network and the ordinary one, which lets through the registries of `[sandbox]
//!   registries` and, since 2026-10-23, the sites the person allowed that agent (sandbox_sites.rs: each sandbox's
//!   proxy variables carry its own credentials, `http://<agent>:<token>@…`, so the proxy knows who asks; a site it
//!   refuses becomes a card for the person); pip, npm, uv, cargo, go and apt find it through the usual proxy
//!   variables. When that
//!   can't be set up, the sandbox gets no network at all (`docker_network: false`), never the open internet, and the
//!   log says why; it is tried again every 10 minutes.
//!
//! Linux with podman only (`[sandbox] isolate = "auto"`): a podman volume is then a folder this account writes in. On
//! macOS (podman in a VM, or no sandbox at all: the installer turns the disk tools off there) and with docker, the
//! sandboxes keep mounting the folders as before (`agents_admin::mount_uploads`, `shares::apply`), and Hermes keeps
//! asking.
//!
//! Stepping back (`isolate = "off"`, or podman gone): at start, each sandbox the bridge had isolated gets back what it
//! had before (`revert`): its uploads folder mounted, its shared folders mounted from this machine's folders (each
//! volume's content copied back there first, `shares::revert_volumes`), and the bridge's network, proxy variables,
//! `docker_network: false` and volumes taken out of its settings; the registries' proxy is stopped. The volumes
//! themselves stay (turning isolation on again finds them).
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
use crate::{beneath, py, shares, Dict};

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
/// The connectors' own network (connectors_run.rs): no route out either, and on it the proxy is strict — only a
/// connector's own credentials, only its approved sites, never the registries (`BIP_STRICT`).
pub const CONNECTOR_NETWORK: &str = "bipagents-connectors";
pub const PROXY: &str = "bipagents-egress";
pub const PROXY_PORT: u16 = 3128;

/// `10.233.80.0/24` → `10.233.81.0/24`: the connectors' network, next to the sandboxes'.
pub fn connector_subnet(subnet: &str) -> Option<String> {
    proxy_ip(subnet)?;
    let parts: Vec<u8> = subnet.split('/').next()?.split('.').map(|p| p.parse::<u8>()).collect::<Result<_, _>>().ok()?;
    let third = parts[2].checked_add(1)?;
    Some(format!("{}.{}.{third}.0/24", parts[0], parts[1]))
}

/// The proxy's address on the connectors' network.
pub fn connector_proxy_ip(subnet: &str) -> Option<String> {
    proxy_ip(&connector_subnet(subnet)?)
}
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

/// The sandbox's container arguments the bridge owns (`terminal.docker_extra_args`): its network, no DNS server
/// (names are resolved by the proxy; nothing leaks through DNS queries), and on a shared server its share of the
/// processors (`--cpus`, send_guard.rs).
pub fn own_args(config: &Config) -> Vec<String> {
    let mut out = vec![format!("--network={NETWORK}"), "--dns=127.0.0.1".to_string()];
    if let Some(cpus) = crate::send_guard::cpus(config) {
        out.push(format!("--cpus={cpus}"));
    }
    out
}

/// One of the arguments the bridge owns (an earlier `--cpus` included, while the bridge sets one).
fn owned_arg(config: &Config, arg: &str) -> bool {
    own_args(config).iter().any(|a| a == arg)
        || (arg.starts_with("--cpus=") && crate::send_guard::cpus(config).is_some())
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

/// Where this machine's resolvers are written: the usual file, then systemd-resolved's list of the real ones when the
/// usual file only names its local stub (Fedora, Ubuntu: `127.0.0.53`).
pub const RESOLV_CONFS: [&str; 2] = ["/etc/resolv.conf", "/run/systemd/resolve/resolv.conf"];

/// The name servers the proxy asks (`--dns`), from the first file that names one reachable from a container: never a
/// loopback stub, nor a link-local address bound to an interface (`fe80::1%eth0`). Without them podman would give the
/// proxy only the sandboxes' network's own resolver, which never answers for the internet on an internal network
/// (seen on aibox 2026-10-28: every site « name not found », the registries included).
pub fn nameservers(files: &[&str]) -> Vec<String> {
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        let found: Vec<String> = text
            .lines()
            .filter_map(|line| line.trim().strip_prefix("nameserver"))
            .map(|rest| rest.trim().to_string())
            .filter(|ns| {
                !ns.contains('%')
                    && ns.parse::<std::net::IpAddr>().is_ok_and(|ip| !ip.is_loopback() && !ip.is_unspecified())
            })
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
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
pub(crate) fn proxy_image(config: &Config) -> String {
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

/// The proxy's own resolv.conf: this machine's real name servers only.
pub fn proxy_resolv(config: &Config) -> PathBuf {
    crate::sandbox_sites::proxy_dir(config).join("resolv.conf")
}

/// The file's text: at most three servers (the resolver reads no more), a short timeout.
pub fn resolv_text(servers: &[String]) -> String {
    let mut text: String = servers.iter().take(3).map(|ns| format!("nameserver {ns}\n")).collect();
    text.push_str("options timeout:2 attempts:2\n");
    text
}

/// Writes the proxy's resolv.conf from this machine's resolvers, or removes it when none is found (podman's default).
pub fn write_proxy_resolv(config: &Config, files: &[&str]) -> std::io::Result<()> {
    let path = proxy_resolv(config);
    let servers = nameservers(files);
    if servers.is_empty() {
        tracing::warn!(target: "bipbridge::sandbox", "no name server found for the sandboxes' proxy: podman's default");
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, resolv_text(&servers))
}

/// The `podman run` of the registries' proxy (replacing the one before: its allow-list may have changed).
pub fn proxy_command(config: &Config, podman: &str) -> Vec<String> {
    let ip = proxy_ip(&config.sandbox.subnet).unwrap_or_default();
    let strict = connector_proxy_ip(&config.sandbox.subnet).unwrap_or_default();
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
        // the connectors' network (connectors_run.rs): strict there
        "--network",
        &format!("{CONNECTOR_NETWORK}:ip={strict}"),
        "-e",
        &format!("BIP_STRICT={strict}"),
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
        // the agents' sites (sandbox_sites.rs): read-only here; and where it reports what it refused. Root of the
        // rootless container: this account, which owns both folders (no capability needed).
        "--user",
        "0:0",
        "-v",
        &format!("{}:/bip/policy:ro,z", crate::sandbox_sites::policy_dir(config).display()),
        "-v",
        &format!("{}:/bip/reports:z", crate::sandbox_sites::reports_dir(config).display()),
        "-e",
        &format!("BIP_POLICY=/bip/policy/{}", crate::sandbox_sites::POLICY_FILE),
        "-e",
        &format!("BIP_REPORTS=/bip/reports/{}", crate::sandbox_sites::REPORTS_FILE),
        "-e",
        &format!("BIP_USAGE=/bip/reports/{}", crate::send_guard::USAGE_FILE),
        // the offer's piracy/tracker list (send_guard.rs), when the bridge keeps one
        "-e",
        &format!("BIP_BLOCKLIST=/bip/policy/{}", crate::send_guard::BLOCKLIST_FILE),
    ]);
    // the agents' visible browsers (send_guard.rs): known by their address on their own network
    if let (true, Some(browsers)) =
        (crate::send_guard::browser_routed(config), crate::send_guard::browser_proxy_ip(&config.sandbox.subnet))
    {
        command.extend(args(&[
            "--network",
            &format!("{}:ip={browsers}", crate::send_guard::BROWSER_NETWORK),
            "-e",
            &format!("BIP_BROWSERS={browsers}"),
        ]));
    }
    // its own resolv.conf (set_up_network writes it): podman would put the internal networks' resolvers first,
    // which never answer for the internet (aibox 2026-10-28); a file mounted there is left as it is
    let resolv = proxy_resolv(config);
    if resolv.is_file() {
        command.extend(["-v".to_string(), format!("{}:/etc/resolv.conf:ro,z", resolv.display())]);
    }
    command.extend(args(&["--entrypoint", "python3"]));
    command.push(proxy_image(config));
    command.extend(args(&["-u", "-c", PROXY_SCRIPT]));
    command
}

async fn set_up_network(config: &Config, run: &Run, podman: &str) -> Result<String, String> {
    let ip = proxy_ip(&config.sandbox.subnet).ok_or("no valid [sandbox] subnet")?;
    let none = BTreeMap::new();
    if run(args(&[podman, "network", "exists", NETWORK]), none.clone()).await.is_err() {
        run(
            // no resolver of its own: the sandboxes have none (`--dns=127.0.0.1`), the proxy gets this machine's
            args(&[podman, "network", "create", "--internal", "--disable-dns", "--subnet", &config.sandbox.subnet, NETWORK]),
            none.clone(),
        )
        .await
        .map_err(|e| format!("network not created: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
        tracing::info!(target: "bipbridge::sandbox", network = NETWORK, subnet = %config.sandbox.subnet, "sandboxes' network created (no route out)");
    }
    let connectors = connector_subnet(&config.sandbox.subnet).ok_or("no valid [sandbox] subnet")?;
    if run(args(&[podman, "network", "exists", CONNECTOR_NETWORK]), none.clone()).await.is_err() {
        run(args(&[podman, "network", "create", "--internal", "--disable-dns", "--subnet", &connectors, CONNECTOR_NETWORK]), none.clone())
            .await
            .map_err(|e| format!("connectors' network not created: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
        tracing::info!(target: "bipbridge::sandbox", network = CONNECTOR_NETWORK, subnet = %connectors, "connectors' network created (no route out)");
    }
    if crate::send_guard::browser_routed(config) {
        ensure_browser_network(config, run, podman).await?;
    }
    crate::sandbox_sites::ensure_policy(config).map_err(|e| format!("the agents' sites not written: {e}"))?;
    write_proxy_resolv(config, &RESOLV_CONFS).map_err(|e| format!("the proxy's name servers not written: {e}"))?;
    // the offer's blocklist fetched now if it is due (the proxy reads it when it appears; never waited for)
    let c = config.clone();
    tokio::spawn(async move { crate::send_guard::refresh_blocklist(&c).await });
    let command = proxy_command(config, podman);
    if let Err(first) = run(command.clone(), none.clone()).await {
        // The proxy's monitor (conmon) lives in the bridge's unit: a stop that times out kills it, and the next start
        // can't `--replace` a container left half-stopped (« could not be stopped… conmon exited prematurely », lab
        // 2026-10-29: no network for the sandboxes until the retry, 10 minutes later). Removed, then started again.
        tracing::info!(target: "bipbridge::sandbox", error = %crate::agents_admin::tail(&first.to_string(), 200), "registries' proxy not replaced: removed and started again");
        let _ = run(args(&[podman, "rm", "-f", "--ignore", PROXY]), none.clone()).await;
        run(command, none)
            .await
            .map_err(|e| format!("registries' proxy not started: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
    }
    tracing::info!(target: "bipbridge::sandbox", proxy = PROXY, registries = %config.sandbox.registries.join(","), "registries' proxy started");
    Ok(format!("http://{ip}:{PROXY_PORT}"))
}

/// The agents' browsers' network (send_guard.rs). Not `--internal`: podman publishes no port of a container on an
/// internal network, and the bridge reaches each Chrome's DevTools and screen through 127.0.0.1. Chrome itself goes
/// through the proxy (`--proxy-server`), which knows each browser by its address here.
pub async fn ensure_browser_network(config: &Config, run: &Run, podman: &str) -> Result<(), String> {
    let none = BTreeMap::new();
    let network = crate::send_guard::BROWSER_NETWORK;
    if run(args(&[podman, "network", "exists", network]), none.clone()).await.is_ok() {
        return Ok(());
    }
    let subnet = crate::send_guard::browser_subnet(&config.sandbox.subnet).ok_or("no valid [sandbox] subnet")?;
    run(args(&[podman, "network", "create", "--subnet", &subnet, network]), none)
        .await
        .map_err(|e| format!("browsers' network not created: {}", crate::agents_admin::tail(&e.to_string(), 200)))?;
    tracing::info!(target: "bipbridge::sandbox", network, subnet = %subnet, "browsers' network created (through the agents' proxy)");
    Ok(())
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
    let target = entry.split(':').nth(1).unwrap_or("");
    shares::share_entry(config, entry) || [RECEIVED, OUTBOX].contains(&target)
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

/// The volumes an isolated sandbox mounts: the person's own entries, its files, its outbox, its shared folders.
pub fn wanted_volumes(config: &Config, agent: &str, current: &[String], reads: &[(String, String)]) -> Vec<String> {
    let mut volumes: Vec<String> = current.iter().filter(|v| !owned_volume(config, v)).cloned().collect();
    volumes.push(format!("{}:{RECEIVED}:ro", files_volume(agent)));
    volumes.push(format!("{}:{OUTBOX}", outbox_volume(agent)));
    volumes.extend(shares::share_mounts(config, agent, reads, true));
    volumes
}

/// The terminal settings an isolated sandbox gets, from the profile's current ones: the person's own entries kept.
pub fn wanted_terminal(config: &Config, agent: &str, current: &Dict, net: Net, reads: &[(String, String)]) -> Dict {
    let list = |key: &str| -> Vec<String> {
        match current.get(key) {
            Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
            _ => Vec::new(),
        }
    };
    let volumes = wanted_volumes(config, agent, &list("docker_volumes"), reads);
    let ours = own_args(config);
    let mut extra: Vec<String> = list("docker_extra_args").into_iter().filter(|a| !owned_arg(config, a)).collect();
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

/// Hermes' approval keys for « run code the agent wrote » (tools/approval.py, tools/approval_detection.py at the
/// commit the installer pins): `execute_code`'s whole-script gate and the interpreter / shell flags (`python -c`, a
/// heredoc, `bash -c`). An isolated sandbox lets them through (owner decision 2026-10-26): its agent reads and computes
/// in its own space without asking. They go in the profile's `command_allowlist`, Hermes' own permanent approvals.
///
/// Why it stays safe, prompt injection being the threat:
/// - these four gates are about the *form* of the code, not what it does: the same agent may already write a script
///   with `write_file` and run it with `python3 script.py`, which no gate catches. They never were a wall against an
///   agent steered by a page or an e-mail; they only made it ask before honest work (« ma journée de demain »);
/// - Hermes checks its specific patterns first (`detect_dangerous_command`): a command that deletes recursively, drops
///   a table, edits a credential file, pipes a download into a shell or uploads a secret with curl still asks, even
///   inside `python -c "…"` or `bash -c "…"` (the patterns read the whole text); its hardline blocks and the person's
///   `approvals.deny` rules are untouched, and `approvals.mode` stays « manual »;
/// - only where the sandbox's network is the bridge's (`isolates`): no route out but the package registries and the
///   sites the person allowed that agent (a new one is a card, sandbox_sites.rs), so what the code reads cannot be
///   sent away; nothing of this machine is mounted but its own volumes (and folders the person mounted by hand, the
///   agent's own working folders, which `sandbox adopt` moves into volumes), and its shared folder is read by the
///   person's own agents;
/// - what leaves its space still asks, through other gates: sending (mail drafts only, messages to other agents go
///   through the bridge's links), payments (each one, with the card's code), deleting a shared file
///   (`shared_files_delete`), the tools of services the person added (`trust: untrusted`), a new site.
///
/// On an install that doesn't isolate (macOS, docker, `isolate = "off"`), Hermes asks as before (`revert` takes them
/// out).
pub const CODE_KEYS: [&str; 4] =
    ["execute_code", "script execution via -e/-c flag", "script execution via heredoc", "shell command via -c/-lc flag"];

/// The profile's `command_allowlist` with `CODE_KEYS` added (`isolated`) or taken out, the person's own entries kept in
/// their order. None: nothing to change.
pub fn wanted_allowlist(current: &Value, isolated: bool) -> Option<Vec<String>> {
    let items: Vec<String> = match current {
        Value::Array(items) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    };
    let mut wanted: Vec<String> = items.iter().filter(|k| !CODE_KEYS.contains(&k.as_str())).cloned().collect();
    if isolated {
        wanted.extend(CODE_KEYS.iter().map(|k| k.to_string()));
    }
    let same = wanted.len() == items.len() && wanted.iter().all(|k| items.contains(k));
    (!same).then_some(wanted)
}

/// `command_allowlist` set as `wanted_allowlist` says (Hermes reads it when the gateway starts: the update restarts
/// it, migrations.py). Returns whether it changed.
async fn set_allowlist(config: &Config, agent: &AgentConfig, folder: &Path, run: &Run, isolated: bool) -> anyhow::Result<bool> {
    let current = shares::profile_config(folder).get("command_allowlist").cloned().unwrap_or(Value::Null);
    let Some(wanted) = wanted_allowlist(&current, isolated) else { return Ok(false) };
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env = BTreeMap::from([("HERMES_HOME".to_string(), home)]);
    let value = py::dumps(&json!(wanted));
    run(args(&[&config.hermes.bin, "-p", &agent.name, "config", "set", "command_allowlist", &value]), env).await?;
    tracing::info!(
        target: "bipbridge::sandbox",
        agent = %agent.name,
        "{}",
        if isolated {
            "code the agent writes runs in its isolated sandbox without asking (execute_code, python -c, heredocs); \
             risky commands still ask"
        } else {
            "code the agent writes asks again before it runs (sandbox not isolated)"
        }
    );
    Ok(true)
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
    // its own credentials on the proxy (sandbox_sites.rs): the proxy knows it, and lets it reach its sites
    let proxy = match network(&config, run).await {
        Ok(base) => Some(crate::sandbox_sites::proxy_url(&config, &agent.name, &base)?),
        Err(_) => None,
    };
    let net = match &proxy {
        Some(url) => Net::Registries(url),
        None => Net::Closed,
    };
    let current = match shares::profile_config(&folder).get("terminal") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    let reads = shares::load(&config);
    let wanted = wanted_terminal(&config, &agent.name, &current, net, &reads);
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
            "sandbox isolated: no folder of this machine inside, its files and shared folders in podman volumes"
        );
    }
    let kept = host_mounts(&match wanted.get("docker_volumes") {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect::<Vec<_>>(),
        _ => Vec::new(),
    });
    if !kept.is_empty() {
        tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, mounts = %kept.join(" "), "the sandbox still mounts folders added by hand: Hermes keeps asking before risky commands");
    }
    // its own code runs without asking: the network is the bridge's (CODE_KEYS says why it stays safe)
    set_allowlist(&config, agent, &folder, run, true).await?;
    if added + removed > 0 {
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, added, removed, "received files copied into the sandbox");
    }
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

/// At start: every sandboxed agent isolated (or its uploads mounted as before where the install doesn't isolate, and
/// what isolation set taken back: `revert`); while the network can't be set up, tried again every 10 minutes.
pub async fn isolate_all(config: SharedConfig, run: Run) {
    let current = config.get();
    if engine(&current).is_none() {
        let state = load_state(&current);
        let had = state.get("agents").and_then(Value::as_object).is_some_and(|a| !a.is_empty())
            || !shares::volume_folders(&current).is_empty();
        let c = current.clone();
        let _ = tokio::task::spawn_blocking(move || shares::revert_volumes(&c)).await;
        crate::agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
        shares::heal_mounts(&config.get(), &run).await;
        if had {
            stop_proxy(&run).await;
        }
        return;
    }
    // The shared folders' volumes before the sandboxes that mount them.
    shares::ensure_volumes(&current, &run, &shares::load(&current)).await;
    crate::agents_admin::mount_all_uploads(config.clone(), run.clone()).await;
    shares::heal_mounts(&config.get(), &run).await;
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

/// The registries' proxy stopped, once the install no longer isolates (it would restart with the machine otherwise).
async fn stop_proxy(run: &Run) {
    let Some(podman) = crate::browsers::pyx::which("podman") else { return };
    match run(args(&[&podman, "rm", "-f", "--ignore", PROXY]), BTreeMap::new()).await {
        Ok(_) => {
            tracing::info!(target: "bipbridge::sandbox", proxy = PROXY, "sandboxes not isolated any more: registries' proxy stopped")
        }
        Err(exc) => {
            tracing::warn!(target: "bipbridge::sandbox", error = %crate::agents_admin::tail(&exc.to_string(), 200), "registries' proxy not stopped")
        }
    }
}

/// An agent this bridge had isolated whose sandbox it no longer isolates (`[sandbox] isolate = "off"`, podman gone,
/// its desktop now in its sandbox): what isolation set taken out of its terminal settings (its volumes, the
/// sandboxes' network and DNS, the proxy variables, `docker_network: false`), the person's own entries kept. Its
/// uploads and shared folders are then mounted as before (`agents_admin::mount_uploads`, `shares::heal_mounts`).
pub async fn revert(config: &Config, agent: &AgentConfig, folder: &Path, run: &Run) -> anyhow::Result<()> {
    if load_state(config).get("agents").and_then(|a| a.get(&agent.name)).is_none() {
        return Ok(());
    }
    let current = match shares::profile_config(folder).get("terminal") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    let list = |key: &str| -> Vec<String> {
        match current.get(key) {
            Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
            _ => Vec::new(),
        }
    };
    let named = [files_volume(&agent.name), outbox_volume(&agent.name)];
    let volumes: Vec<String> = list("docker_volumes")
        .into_iter()
        .filter(|v| {
            let name = v.split(':').next().unwrap_or("");
            !named.iter().any(|n| n == name) && !name.starts_with("bipagents-share-")
        })
        .collect();
    let extra: Vec<String> = list("docker_extra_args")
        .into_iter()
        .filter(|a| !owned_arg(config, a))
        .collect();
    let mut env: Dict = match current.get("docker_env") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    env.retain(|k, _| !PROXY_ENV.contains(&k.as_str()));
    let mut wanted = Dict::new();
    wanted.insert("docker_volumes".into(), json!(volumes));
    wanted.insert("docker_extra_args".into(), json!(extra));
    wanted.insert("docker_env".into(), Value::Object(env));
    if current.get("docker_network") == Some(&json!(false)) {
        wanted.insert("docker_network".into(), json!(true));
    }
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env = BTreeMap::from([("HERMES_HOME".to_string(), home)]);
    let mut changed: Vec<String> = Vec::new();
    for (key, value) in &wanted {
        let now = current.get(key).cloned().unwrap_or(Value::Null);
        let empty =
            matches!(value, Value::Array(a) if a.is_empty()) || matches!(value, Value::Object(o) if o.is_empty());
        if &now == value || (now.is_null() && empty) {
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
    if set_allowlist(config, agent, folder, run, false).await? {
        changed.push("command_allowlist".into());
    }
    tracing::info!(
        target: "bipbridge::sandbox",
        agent = %agent.name,
        changed = %changed.join(","),
        "sandbox not isolated any more: the bridge's network, proxy and volumes taken out, its folders mounted as before"
    );
    forget_state(config, &agent.name);
    Ok(())
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
    crate::sandbox_sites::forget(config, agent);
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

/// By path: only for the uploads (the bridge's own folder) copied into the files volume, which the sandbox mounts
/// read-only (/fichiers), so neither side can hold a link the sandbox made. Folders a sandbox writes in go through
/// beneath.rs.
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
        // Only into the agent's own shared folder, which its sandbox mounts read-write anyway (/partage): marked
        // read-only so that Hermes doesn't ask the person (a routine shares at night, 2026-10-23).
        json!({
            "name": WRITE,
            "description": format!("Put a file in your shared folder (/partage), for the agents the person allowed \
        (they only read it). Copy the file right into {OUTBOX} (no folder) and give its path as `from`, or give a short text as \
        `content`. No approval needed."),
            "inputSchema": {"type": "object", "properties": {
                "path": {"type": "string", "description": "where in your shared folder, e.g. resume/semaine.md"},
                "from": {"type": "string", "description": format!("{OUTBOX}/<name>: a file you copied there")},
                "content": {"type": "string", "description": "or the text itself"},
            }, "required": ["path"]},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false},
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
    .into_iter()
    .chain(crate::sandbox_sites::tools())
    .collect()
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

/// A plain relative path's folder and last name (`plain_relative` gives one).
fn split_last(relative: &Path) -> (PathBuf, std::ffi::OsString) {
    let folder = relative.parent().map(Path::to_path_buf).unwrap_or_default();
    (folder, relative.file_name().map(|n| n.to_os_string()).unwrap_or_default())
}

fn human(size: u64) -> String {
    match size {
        s if s >= 1024 * 1024 => format!("{:.1} MB", s as f64 / (1024.0 * 1024.0)),
        s if s >= 1024 => format!("{:.1} KB", s as f64 / 1024.0),
        s => format!("{s} B"),
    }
}

/// The files of a shared folder, read through folder descriptors (a folder swapped for a link meanwhile is not
/// listed: never this machine's files' names).
fn listing(root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let Ok(top) = beneath::Dir::open(root) else { return lines };
    let mut stack = vec![PathBuf::new()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = top.sub(&dir).and_then(|d| d.entries()) else { continue };
        for (name, stat) in entries {
            let relative = dir.join(&name);
            match stat.kind {
                beneath::Kind::Dir if relative.components().count() < beneath::MAX_DEPTH => stack.push(relative),
                beneath::Kind::File => {
                    lines.push(format!("- {} ({})", relative.to_string_lossy(), human(stat.size)));
                }
                _ => {}
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
    match name {
        LIST => {
            let mut out = vec![format!("Files the person sent you: in {RECEIVED} (read-only).")];
            let readers: Vec<&str> = reads.iter().filter(|(_, s)| s == &agent.name).map(|(r, _)| r.as_str()).collect();
            let own = listing(&shares::share_dir(config, &agent.name));
            out.push(format!(
                "\nYour shared folder (agent `{}`){}: {}",
                agent.name,
                if readers.is_empty() {
                    ", read by no agent yet".to_string()
                } else {
                    format!(", at /partage, read by {}", readers.join(", "))
                },
                if own.is_empty() { "empty".to_string() } else { format!("{} file(s)", own.len()) }
            ));
            out.extend(own);
            for (_, source) in reads.iter().filter(|(r, _)| r == &agent.name) {
                let shown =
                    config.agent(Some(source)).map(|a| a.display_name.clone()).unwrap_or_else(|| source.clone());
                let files = listing(&shares::share_dir(config, source));
                out.push(format!(
                    "\nShared with you by {shown} (agent `{source}`, at /lecture/{source}, read-only): {}",
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
            // Opened through the folder's descriptor, one name at a time, never through a link: what is read is in
            // the shared folder, whatever the sandbox swaps meanwhile.
            let Ok(mut input) = beneath::open_file(&shares::share_dir(config, &source), &relative) else {
                return error("no such file (give a file, as shared_files_list shows it)");
            };
            let Some((files, _)) = volumes_of(config, &agent.name) else {
                return error("your space isn't ready for files yet; try again in a moment");
            };
            let done = (|| -> std::io::Result<()> {
                let (folder, name) = split_last(&relative);
                let into = beneath::Dir::open(&files)?
                    .make_dirs(&Path::new(COPIES).join(&source).join(folder), Some(SANDBOX_DIR_MODE))?;
                into.write_file(&name, SANDBOX_FILE_MODE, None, |out| std::io::copy(&mut input, out).map(|_| ()))
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
            let own = match beneath::Dir::open_or_make(&shares::share_dir(config, &agent.name)) {
                Ok(own) => own,
                Err(exc) => return error(&format!("your shared folder can't be opened ({exc})")),
            };
            if name == DELETE {
                // In its folder's descriptor (`unlinkat`), a folder emptied the same way: never through a link.
                let gone = own.parent_of(&relative).and_then(|(parent, name)| parent.remove(&name));
                return match gone {
                    Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => {
                        error("no such file in your shared folder")
                    }
                    Err(exc) if beneath::refused(&exc) => error("not a path in your shared folder"),
                    Ok(_) => {
                        shares::mirror_one(config, &agent.name);
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
                    return error("your space isn't ready for files yet; try again in a moment");
                };
                // Right in /envoi, no folder on the way: nothing the sandbox could swap for a link meanwhile.
                let name = plain_relative(rest).filter(|r| r.components().count() == 1);
                let Some(source) = name else {
                    return error(&format!("put the file right in {OUTBOX} (no folder), then give {OUTBOX}/<name>"));
                };
                let limit = config.limits.upload_max_bytes;
                let input = match beneath::open_file(&outbox, &source) {
                    Ok(file) if file.metadata().map(|m| m.len() <= limit).unwrap_or(false) => file,
                    Ok(_) => return error("this file is too large to share"),
                    Err(_) => return error(&format!("no such file in {OUTBOX}")),
                };
                write_into(&own, &relative, |out| {
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
                write_into(&own, &relative, |out| out.write_all(content.as_bytes()))
            } else {
                return error(&format!("give `from` (a file under {OUTBOX}) or `content`"));
            };
            match written {
                Ok(()) => {
                    shares::mirror_one(config, &agent.name);
                    tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, path = %relative.to_string_lossy(), "shared file written");
                    tool_text(&format!("In your shared folder: {}", relative.to_string_lossy()), false)
                }
                Err(exc) => error(&format!("could not write it: {exc}")),
            }
        }
        _ => error(&format!("unknown tool: {name}")),
    }
}

/// A file written into the agent's shared folder, open to the sandboxes' users like the rest of it (its agent may
/// change it from /partage). Every step in a folder's descriptor (beneath.rs): the folders on the way made with
/// `mkdirat` and opened without following a link, their mode set with `fchmod` (a folder its agent made is its
/// agent's: left as it is), the file written under a fresh temporary name (`O_CREAT|O_EXCL|O_NOFOLLOW`) then renamed
/// in that same folder.
fn write_into(
    own: &beneath::Dir,
    relative: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let (dir_mode, file_mode) = shares::open_modes();
    let (folder, name) = split_last(relative);
    own.make_dirs(&folder, Some(dir_mode))?.write_file(&name, file_mode, None, fill)
}

const INSTRUCTIONS: &str = "BipAgents files: the files your person sends you are in /fichiers (read-only). Shared \
folders between agents, where the person set them up: yours at /partage (write there directly, no approval), the \
ones shared with you at /lecture/<agent> (read-only). shared_files_list, shared_files_get and shared_files_write reach \
them too; the person approves each shared_files_delete. To give files to one of your \
other agents, put them in /envoi then use send_files_to_agent (bipagents tools). To give a file to your person, \
put it in /envoi and end your reply with a line MEDIA:/envoi/<name>; a page goes through create_page. Your \
sandbox reaches the package registries (pip, npm…) and the sites your person allowed (sandbox_sites lists them); \
another site your programs need: sandbox_sites_request asks your person (a site refused to your programs is asked \
about by itself). Read web pages with your web tools or your browser, never curl or a script; programs only for an \
API or data your person asked for.";

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
            if [crate::sandbox_sites::LIST, crate::sandbox_sites::REQUEST].contains(&name.as_str()) {
                let (c, a) = (config.clone(), agent.clone());
                let (text, is_error, card) =
                    tokio::task::spawn_blocking(move || crate::sandbox_sites::call(&c, &a, &name, &arguments))
                        .await
                        .map_err(anyhow::Error::from)?;
                if let Some(card) = card {
                    crate::sandbox_sites::notify(&state, &agent, &card).await;
                }
                return Ok(reply(Value::Object(tool_text(&text, is_error))));
            }
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
    fn code_keys_join_and_leave_the_persons_own_approvals() {
        let added = wanted_allowlist(&json!(["podman ps"]), true).unwrap();
        assert_eq!(added[0], "podman ps");
        assert!(CODE_KEYS.iter().all(|k| added.contains(&k.to_string())) && added.len() == 5);
        assert_eq!(wanted_allowlist(&json!(added.clone()), true), None); // already there
        assert_eq!(wanted_allowlist(&Value::Null, false), None);
        assert_eq!(wanted_allowlist(&json!(added), false), Some(vec!["podman ps".to_string()]));
        // a specific pattern the person allowed for good stays; only the four code keys move
        let kept = wanted_allowlist(&json!(["recursive delete"]), true).unwrap();
        assert_eq!(kept.iter().filter(|k| k.as_str() == "recursive delete").count(), 1);
        assert!(!CODE_KEYS.contains(&"recursive delete"));
    }

    #[test]
    fn the_proxy_takes_the_second_address() {
        assert_eq!(proxy_ip("10.233.80.0/24").as_deref(), Some("10.233.80.2"));
        assert_eq!(proxy_ip("10.233.80.0/16"), None);
        assert_eq!(proxy_ip("10.233.80.1/24"), None);
        assert_eq!(proxy_ip("nonsense"), None);
    }

    #[test]
    fn the_proxy_asks_this_machines_real_resolvers() {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("resolv.conf");
        let real = dir.path().join("resolved.conf");
        // aibox, 2026-10-28: the usual file names only systemd-resolved's stub, the real one is the box
        std::fs::write(&stub, "nameserver 127.0.0.53\noptions edns0 trust-ad\nsearch lan\n").unwrap();
        std::fs::write(&real, "# resolved\nnameserver 192.168.1.1\nnameserver fe80::1%eth0\nnameserver 2a01:cb00::1\n").unwrap();
        let files = [stub.to_str().unwrap(), real.to_str().unwrap()];
        assert_eq!(nameservers(&files), vec!["192.168.1.1", "2a01:cb00::1"]);
        // a usual file with a reachable server is enough; none anywhere: podman's default
        std::fs::write(&stub, "nameserver 100.100.100.100\n").unwrap();
        assert_eq!(nameservers(&files), vec!["100.100.100.100"]);
        assert!(nameservers(&[dir.path().join("none").to_str().unwrap()]).is_empty());
        assert_eq!(resolv_text(&["192.168.1.1".into()]), "nameserver 192.168.1.1\noptions timeout:2 attempts:2\n");
        // the proxy gets the file, mounted over podman's own (whose first servers are the internal networks')
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.path().join("data/bridge.db").to_string_lossy().into_owned();
        config.sandbox.subnet = "10.233.80.0/24".into();
        std::fs::write(&stub, "nameserver 127.0.0.53\n").unwrap();
        write_proxy_resolv(&config, &files).unwrap();
        assert_eq!(std::fs::read_to_string(proxy_resolv(&config)).unwrap(), resolv_text(&["192.168.1.1".into(), "2a01:cb00::1".into()]));
        let command = proxy_command(&config, "podman");
        let mount = format!("{}:/etc/resolv.conf:ro,z", proxy_resolv(&config).display());
        assert!(command.contains(&mount) && !command.iter().any(|a| a.starts_with("--dns=")));
        // none found: no file, podman's default
        write_proxy_resolv(&config, &[dir.path().join("none").to_str().unwrap()]).unwrap();
        assert!(!proxy_resolv(&config).exists() && !proxy_command(&config, "podman").contains(&mount));
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

    /// An agent whose shared folder is `<data>/shares/vie`, its files volume and outbox in the temporary folder, and
    /// « home »: what the person's account holds outside (a secret, closed folder).
    fn tools_setup() -> (tempfile::TempDir, Config, AgentConfig, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.path().join("data/bridge.db").to_string_lossy().into_owned();
        config.sandbox.isolate = "off".into();
        let agent = AgentConfig { name: "vie".into(), shares: "external".into(), ..AgentConfig::default() };
        config.agents.insert(agent.name.clone(), agent.clone());
        let (files, outbox) = (dir.path().join("files"), dir.path().join("envoi"));
        std::fs::create_dir_all(&files).unwrap();
        std::fs::create_dir_all(&outbox).unwrap();
        save_state(&config, |state| {
            state.insert(
                "agents".into(),
                json!({"vie": {"files": files.to_string_lossy(), "outbox": outbox.to_string_lossy()}}),
            );
        })
        .unwrap();
        let share = shares::share_dir(&config, "vie");
        std::fs::create_dir_all(&share).unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".config")).unwrap();
        std::fs::write(home.join("secret.txt"), "home secret").unwrap();
        std::fs::set_permissions(home.join("secret.txt"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, config, agent, share, home)
    }

    fn tool(config: &Config, agent: &AgentConfig, name: &str, arguments: Value) -> (bool, String) {
        let Value::Object(arguments) = arguments else { panic!() };
        let out = call(config, agent, name, &arguments);
        let text = out["content"][0]["text"].as_str().unwrap_or_default().to_string();
        (out.get("isError").and_then(Value::as_bool).unwrap_or(false), text)
    }

    /// Home as it was: the same names, the secret unchanged, the modes untouched.
    fn home_untouched(home: &Path) {
        let mut names: Vec<_> = std::fs::read_dir(home).unwrap().flatten().map(|e| e.file_name()).collect();
        names.sort();
        assert_eq!(names, [std::ffi::OsString::from(".config"), "secret.txt".into()], "something written in home");
        assert!(std::fs::read_dir(home.join(".config")).unwrap().next().is_none(), "something written in .config");
        assert_eq!(std::fs::read_to_string(home.join("secret.txt")).unwrap(), "home secret");
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777;
        assert_eq!((mode(home), mode(&home.join("secret.txt"))), (0o700, 0o600), "home's modes changed");
    }

    /// No copy the agent can read holds home's secret.
    fn nothing_leaked(files: &Path) {
        let mut stack = vec![files.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let meta = entry.path().symlink_metadata().unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else if meta.is_file() {
                    let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
                    assert!(!text.contains("home secret"), "{} holds home's secret", entry.path().display());
                }
            }
        }
    }

    /// A link the sandbox planted at each place a tool goes through: refused, nothing outside the shared folder read,
    /// written, made, chmodded or removed (security audit 2026-10-27, H1).
    #[test]
    fn the_share_tools_never_go_through_a_planted_link() {
        let (dir, config, agent, share, home) = tools_setup();
        let (files, outbox) = (dir.path().join("files"), dir.path().join("envoi"));
        // what works: a file written, listed, fetched, deleted
        let (failed, said) = tool(&config, &agent, WRITE, json!({"path": "a/b/c.txt", "content": "ok"}));
        assert!(!failed, "{said}");
        assert_eq!(std::fs::read_to_string(share.join("a/b/c.txt")).unwrap(), "ok");
        let mode = |p: &Path| std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&share.join("a/b")), mode(&share.join("a/b/c.txt"))), (0o777, 0o666));
        assert!(tool(&config, &agent, LIST, json!({})).1.contains("- a/b/c.txt (2 B)"));
        let (failed, said) = tool(&config, &agent, GET, json!({"path": "a/b/c.txt"}));
        assert!(!failed, "{said}");
        assert_eq!(std::fs::read_to_string(files.join("partages/vie/a/b/c.txt")).unwrap(), "ok");
        std::fs::write(outbox.join("graphe.png"), "PNG").unwrap();
        let (failed, said) = tool(&config, &agent, WRITE, json!({"path": "g.png", "from": "/envoi/graphe.png"}));
        assert!(!failed, "{said}");
        assert_eq!(std::fs::read_to_string(share.join("g.png")).unwrap(), "PNG");
        assert!(!tool(&config, &agent, DELETE, json!({"path": "a"})).0);
        assert!(!share.join("a").exists());

        // a folder of the shared folder is a link to home
        std::os::unix::fs::symlink(&home, share.join("d")).unwrap();
        assert!(tool(&config, &agent, GET, json!({"path": "d/secret.txt"})).0);
        assert!(tool(&config, &agent, WRITE, json!({"path": "d/x.txt", "content": "x"})).0);
        assert!(tool(&config, &agent, WRITE, json!({"path": "d/.config/systemd/user/x.service", "content": "x"})).0);
        assert!(tool(&config, &agent, WRITE, json!({"path": "d/x.txt", "from": "/envoi/graphe.png"})).0);
        assert!(tool(&config, &agent, DELETE, json!({"path": "d/secret.txt"})).0);
        assert!(!tool(&config, &agent, LIST, json!({})).1.contains("secret"));
        home_untouched(&home);
        // removing the link removes the link
        assert!(!tool(&config, &agent, DELETE, json!({"path": "d"})).0);
        assert!(share.join("d").symlink_metadata().is_err());
        home_untouched(&home);

        // the file itself is a link to home's secret
        std::os::unix::fs::symlink(home.join("secret.txt"), share.join("s.txt")).unwrap();
        assert!(tool(&config, &agent, GET, json!({"path": "s.txt"})).0);
        // written: the link replaced by the file, never written through
        assert!(!tool(&config, &agent, WRITE, json!({"path": "s.txt", "content": "new"})).0);
        assert_eq!(std::fs::read_to_string(share.join("s.txt")).unwrap(), "new");
        home_untouched(&home);

        // its outbox's file is a link to home's secret
        std::os::unix::fs::symlink(home.join("secret.txt"), outbox.join("s.txt")).unwrap();
        assert!(tool(&config, &agent, WRITE, json!({"path": "t.txt", "from": "/envoi/s.txt"})).0);

        // where GET copies to (its files volume) holds a link to home
        std::fs::write(share.join("ok.txt"), "ok").unwrap();
        std::fs::remove_dir_all(files.join("partages")).unwrap();
        std::os::unix::fs::symlink(&home, files.join("partages")).unwrap();
        assert!(tool(&config, &agent, GET, json!({"path": "ok.txt"})).0);
        std::fs::remove_file(files.join("partages")).unwrap();
        std::fs::create_dir_all(files.join("partages")).unwrap();
        std::os::unix::fs::symlink(&home, files.join("partages/vie")).unwrap();
        assert!(tool(&config, &agent, GET, json!({"path": "ok.txt"})).0);
        home_untouched(&home);
        nothing_leaked(&files);
    }

    /// The race itself: a thread swaps a folder of the shared folder for a link to home and back, in one step each
    /// (`renameat2(RENAME_EXCHANGE)` / `renamex_np(RENAME_SWAP)`), as fast as it can, while the agent's tools read,
    /// write and delete through it again and again. Some calls get through (the folder was there), none reaches
    /// home.
    #[test]
    fn the_share_tools_win_the_race_against_a_swapped_folder() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (dir, config, agent, share, home) = tools_setup();
        let files = dir.path().join("files");
        std::fs::create_dir_all(share.join("d")).unwrap();
        std::fs::write(share.join("d/secret.txt"), "share").unwrap();
        std::os::unix::fs::symlink(&home, share.join("d.link")).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper = {
            let (share, stop) = (share.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    beneath::exchange(&share.join("d"), &share.join("d.link")).unwrap();
                    std::thread::yield_now();
                    beneath::exchange(&share.join("d"), &share.join("d.link")).unwrap();
                }
            })
        };
        let (mut got, mut wrote) = (0, 0);
        for i in 0..1500 {
            if !tool(&config, &agent, GET, json!({"path": "d/secret.txt"})).0 {
                got += 1;
            }
            let path = format!("d/w{}.txt", i % 2);
            if !tool(&config, &agent, WRITE, json!({"path": path, "content": "from the agent"})).0 {
                wrote += 1;
            }
            let _ = tool(&config, &agent, WRITE, json!({"path": "d/.config/x.txt", "content": "x"}));
            let _ = tool(&config, &agent, DELETE, json!({"path": path}));
            let _ = tool(&config, &agent, DELETE, json!({"path": "d/secret.txt/.."}));
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        assert!(got > 0 && wrote > 0, "the tools never got through: {got} {wrote}");
        home_untouched(&home);
        nothing_leaked(&files);
        assert_eq!(std::fs::read_to_string(files.join("partages/vie/d/secret.txt")).unwrap(), "share");
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
        let reads = vec![("vie".to_string(), "w".to_string()), ("budget".to_string(), "vie".to_string())];
        let wanted = wanted_terminal(&config, "vie", &current, Net::Registries("http://10.233.80.2:3128"), &reads);
        assert_eq!(
            wanted["docker_volumes"],
            json!([
                "/home/x/photos:/photos:ro",
                "bipagents-files-vie:/fichiers:ro",
                "bipagents-envoi-vie:/envoi",
                "bipagents-share-vie:/partage",
                "bipagents-share-w:/lecture/w:ro"
            ])
        );
        assert!(host_mounts(&wanted_volumes(&config, "vie", &[], &reads)).is_empty());
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
        let closed = wanted_terminal(&config, "vie", &current, Net::Closed, &[]);
        assert_eq!(closed["docker_extra_args"], json!(["--shm-size=2g"]));
        assert_eq!(closed["docker_env"], json!({"LANG": "fr_FR.UTF-8"}));
        assert_eq!(closed["docker_network"], json!(false));
    }
}
