//! Each agent's visible browser: a real Chrome on a screen of its own, in a container of its own
//! (`browser/Containerfile`, image built by the installer).
//!
//! For an agent, this sets up:
//!
//! - its container, `bipagents-browser-<agent>`, run by a systemd user unit: its Chrome profile (cookies and logins) in
//!   `<browser root>/<agent>/profile`, nothing else of the host; a page it opens (file:// included) only ever sees this
//!   container. Started when the agent (or the person, in « Bureau ») uses it, stopped after `idle_minutes` unused
//!   (browser_mcp.py): a machine with several agents only holds the browsers in use; `idle_minutes = 0` keeps it running;
//! - two ports on 127.0.0.1, never published: Chrome's DevTools (Hermes' `browser.cdp_url`, so the vault fills in this
//!   browser), behind a secret of its own (`CDP_SECRET`), and the screen's VNC server, with a password of its own, which
//!   the bridge logs in to and relays to the app (« Bureau », behind Face ID; see desktop.py);
//! - in the agent's Hermes profile, the `stealth` MCP server (`podman exec -i <container> /opt/stealth/mcp.sh`: the
//!   server runs in the container, next to Chrome; behind browser_mcp.py, which starts it when used) and the skill that
//!   tells the agent how to use it.
//!
//! With `idle_minutes`, the container's unit and its Chrome profile folder are only written at the browser's first
//! use (bip-browser-mcp's first start, or the person's « Bureau »): an agent that never browses costs nothing, and
//! creating one is a few file writes.
//!
//! Linux only: the screen is an X server in the container.
//!
//! Port of `bridge/bipbridge/browsers.py` (owner group: browsers-places).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::agents_admin::pyyaml::{self, Yaml};
use crate::agents_admin::Run;
use crate::config::{AgentConfig, Config};

/// Python behaviours the browsers-places modules share (`str.strip()`, `shlex.quote`, `shutil.which`…).
#[path = "browsers_places/pyx.rs"]
pub mod pyx;

pub const GPU_FLAGS: [&str; 3] =
    ["--use-gl=angle", "--use-angle=vulkan", "--enable-features=Vulkan,VulkanFromANGLE,DefaultANGLEVulkan"];
/// browser/hold_gate.py: while it is there, the agent's browser tools wait.
pub const HOLD_FLAG: &str = "/tmp/bipagents-hold";
/// browser/entrypoint.sh: its screen is up.
pub const SCREEN: &str = "/tmp/.X11-unix/X99";
/// The MCP proxy Hermes runs for an agent's browser (Python: browser_mcp.py): the `bip-browser-mcp` binary.
pub const PROXY_BINARY: &str = "bip-browser-mcp";

/// The skill an agent with a browser gets (`skills/visible-browser/SKILL.md`, embedded).
pub fn skill() -> &'static str {
    crate::assets::text(&crate::assets::SKILLS, "visible-browser/SKILL.md").unwrap_or("")
}

/// Python's `BrowserError` (a `RuntimeError`): returned inside `anyhow::Error`, found with `downcast_ref`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct BrowserError(pub String);

/// The agent's browser container's name.
pub fn container(agent: &str) -> String {
    format!("bipagents-browser-{agent}")
}

/// A command run for its exit code only (stdout and stderr to /dev/null); -1 when it could not start.
pub type Exec = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, i32> + Send + Sync>;

/// `asyncio.create_subprocess_exec(*args, stdout=DEVNULL, stderr=DEVNULL)` then `wait()`.
pub fn default_exec() -> Exec {
    Arc::new(|args: Vec<String>| {
        Box::pin(async move {
            let Some((program, rest)) = args.split_first() else { return -1 };
            let status = tokio::process::Command::new(program)
                .args(rest)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await;
            match status {
                Ok(s) => s.code().unwrap_or(-1),
                Err(_) => -1,
            }
        })
    })
}

/// What browsers.py asks of the machine (Python's tests replace these: `platform.system`, `runtime`, `unit_path`,
/// `_free`, `asyncio.create_subprocess_exec`).
#[derive(Clone)]
pub struct Host {
    /// `platform.system()`: "Linux", "Darwin"…
    pub system: String,
    /// `runtime(config)`: podman or docker, found.
    pub runtime: Option<String>,
    /// Where the systemd user units go (`~/.config/systemd/user`).
    pub unit_dir: PathBuf,
    pub is_free: Arc<dyn Fn(u16) -> bool + Send + Sync>,
    pub exec: Exec,
}

/// `platform.system()`.
pub fn system() -> String {
    match std::env::consts::OS {
        "linux" => "Linux".into(),
        "macos" => "Darwin".into(),
        "windows" => "Windows".into(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// podman or docker (`[browser] runtime`, else the one found).
pub fn runtime(config: &Config) -> Option<String> {
    if !config.browser.runtime.is_empty() {
        return pyx::which(&config.browser.runtime);
    }
    pyx::which("podman").or_else(|| pyx::which("docker"))
}

impl Host {
    /// This machine.
    pub fn real(config: &Config) -> Host {
        Host {
            system: system(),
            runtime: runtime(config),
            unit_dir: pyx::home().join(".config").join("systemd").join("user"),
            is_free: Arc::new(free),
            exec: default_exec(),
        }
    }

    pub fn unit_path(&self, agent: &str) -> PathBuf {
        self.unit_dir.join(format!("{}.service", container(agent)))
    }
}

pub fn folder(config: &Config, agent: &str) -> PathBuf {
    PathBuf::from(&config.browser.root).join(agent)
}

pub fn on_demand(config: &Config) -> bool {
    config.browser.idle_minutes > 0
}

/// `Path.touch()`: made if missing, its time set to now.
fn touch(path: &Path) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.set_modified(std::time::SystemTime::now())
}

/// Someone uses the agent's browser (the person watching its desktop): not stopped for `idle_minutes` from now.
pub fn busy(config: &Config, agent: &str) {
    let _ = touch(&folder(config, agent).join("busy"));
}

/// A run that used the agent's browser ended (completed or failed) and no other run of the agent still uses it: its
/// browser stops `browser_mcp::AFTER_RUN_SECONDS` from now unless used again (bip-browser-mcp reads `ended`). Only
/// for an agent whose browser starts on demand and was set up (its folder there).
pub fn run_ended(config: &Config, agent: &str) {
    let place = folder(config, agent);
    if on_demand(config) && place.is_dir() {
        let _ = touch(&place.join("ended"));
    }
}

fn has_ports(config: &Config, agent: &str) -> bool {
    ports_of(config, agent).map(|p| !p.is_empty()).unwrap_or(false)
}

/// The agent's browser container, started if it was stopped, its screen up. Returns whether this agent has one.
pub async fn wake(config: &Config, agent: &str, wait: f64) -> bool {
    wake_with(&Host::real(config), config, agent, wait).await
}

pub async fn wake_with(host: &Host, config: &Config, agent: &str, wait: f64) -> bool {
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) {
        return false;
    }
    if !on_demand(config) {
        return true;
    }
    busy(config, agent);
    install_unit(host, config, agent).await;
    let screen_up = || {
        (host.exec)(vec![engine.clone(), "exec".into(), container(agent), "test".into(), "-e".into(), SCREEN.into()])
    };
    if screen_up().await == 0 {
        return true;
    }
    (host.exec)(vec!["systemctl".into(), "--user".into(), "start".into(), container(agent)]).await;
    let deadline = crate::py::monotonic() + wait;
    loop {
        if screen_up().await == 0 {
            tracing::info!(agent = %agent, "browser woken");
            break;
        }
        if crate::py::monotonic() > deadline {
            tracing::warn!(agent = %agent, "browser did not wake");
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    true
}

/// The container's unit, written now if it was left for the browser's first use (see `setup`).
async fn install_unit(host: &Host, config: &Config, agent: &str) {
    let path = folder(config, agent).join("mcp.json");
    let Some(Value::Object(settings)) =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return;
    };
    match crate::browser_mcp::write_unit(&settings) {
        Ok(true) => {
            (host.exec)(vec!["systemctl".into(), "--user".into(), "daemon-reload".into()]).await;
            tracing::info!(agent = %agent, "browser container set up");
        }
        Ok(false) => {}
        Err(exc) => tracing::warn!(agent = %agent, error = %exc, "browser container not set up"),
    }
}

/// The person takes over the agent's browser (or hands it back): its MCP tools and its DevTools wait meanwhile.
/// Returns whether this agent has such a browser (its container, set up here).
pub async fn hold(config: &Config, agent: &str, on: bool) -> bool {
    hold_with(&Host::real(config), config, agent, on).await
}

pub async fn hold_with(host: &Host, config: &Config, agent: &str, on: bool) -> bool {
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) {
        return false;
    }
    if on {
        wake_with(host, config, agent, 60.0).await;
    }
    // The person's hold carries a token of its own: the bridge's connections showing it (`X-Bip-Hold`) still reach
    // Chrome's DevTools (devtools_relay.py), to show the page on the phone and pass on the person's taps and keys.
    let token_file = folder(config, agent).join(HOLD_TOKEN);
    let command = if on {
        let token = crate::py::token_hex(16);
        if crate::py::write_with_mode(&token_file.to_string_lossy(), token.as_bytes(), 0o600).is_err() {
            tracing::warn!(agent = %agent, "browser hold token not kept");
        }
        format!("umask 077; printf 'human %s' {token} > {HOLD_FLAG}")
    } else {
        let _ = std::fs::remove_file(&token_file);
        format!("rm -f {HOLD_FLAG}")
    };
    let code = (host.exec)(vec![engine, "exec".into(), container(agent), "sh".into(), "-c".into(), command]).await;
    if code != 0 {
        tracing::warn!(agent = %agent, on = on, "browser hold not set");
    }
    true
}

/// Where the person's hold keeps its token (`<browser root>/<agent>/hold.token`, the bridge's alone).
pub const HOLD_TOKEN: &str = "hold.token";

/// The token of the person's hold on the agent's browser, while they hold it.
pub fn hold_token(config: &Config, agent: &str) -> Option<String> {
    let text = std::fs::read_to_string(folder(config, agent).join(HOLD_TOKEN)).ok()?;
    let token = text.trim();
    (!token.is_empty() && token.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| token.to_string())
}

/// The person's « Ouvrir le navigateur »: its container woken, then Chrome started on its screen with its profile
/// when none runs (browser/open_browser.sh, detached; the agent's spawn_browser connects to it later). An image
/// built before that script has none: the screen is up, without a browser. Returns whether this agent has one.
pub async fn open_browser(config: &Config, agent: &str) -> bool {
    let host = Host::real(config);
    let Some(engine) = host.runtime.clone() else { return false };
    if !wake_with(&host, config, agent, 90.0).await {
        return false;
    }
    let code = (host.exec)(vec![engine, "exec".into(), "-d".into(), container(agent), OPEN_BROWSER.into()]).await;
    if code != 0 {
        tracing::warn!(agent = %agent, code = code, "browser not opened");
    }
    true
}

/// browser/open_browser.sh in the container.
pub const OPEN_BROWSER: &str = "/opt/stealth/open_browser.sh";

/// A card payment holds the agent's browser (card_pay.rs): its MCP tools and its DevTools wait as for the person's
/// hold, except the bridge's own DevTools connections, which show `token` (`X-Bip-Card`, browser/devtools_relay.py).
/// `Some(token)`: taken, unless the person holds it already (false). `None`: given back, only if a payment held it.
pub async fn hold_for_payment(config: &Config, agent: &str, token: Option<&str>) -> bool {
    let host = Host::real(config);
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) {
        return false;
    }
    let command = match token {
        Some(token) if token.bytes().all(|b| b.is_ascii_alphanumeric()) => {
            format!("[ -e {HOLD_FLAG} ] && exit 3; umask 077; printf 'card %s' {token} > {HOLD_FLAG}")
        }
        Some(_) => return false,
        None => format!("[ \"$(head -c 4 {HOLD_FLAG} 2>/dev/null)\" = card ] && rm -f {HOLD_FLAG}; exit 0"),
    };
    let code = (host.exec)(vec![engine, "exec".into(), container(agent), "sh".into(), "-c".into(), command]).await;
    if code != 0 {
        tracing::warn!(agent = %agent, on = token.is_some(), code = code, "payment hold not set");
    }
    code == 0
}

/// Its DevTools and VNC ports (`{"cdp": …, "vnc": …}`), when it has a browser. Its DevTools secret, if any, is then
/// known by its port (`devtools_secret`): every caller finds the port here.
pub fn ports_of(config: &Config, agent: &str) -> Option<IndexMap<String, u16>> {
    let place = folder(config, agent);
    let text = std::fs::read_to_string(place.join("ports.json")).ok()?;
    let ports: IndexMap<String, u16> = match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| v.as_u64().and_then(|p| u16::try_from(p).ok()).map(|p| (k, p)))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    if let Some(&cdp) = ports.get("cdp") {
        let mut known = SECRETS.lock().unwrap_or_else(|e| e.into_inner());
        match read_secret(&place.join(CDP_SECRET)) {
            Some(secret) => known.insert(cdp, secret),
            None => known.remove(&cdp),
        };
    }
    Some(ports)
}

/// The browser's DevTools secret (security audit 2026-10-27, 11 and 21): `<browser root>/<agent>/cdp.token`, the
/// bridge's alone (0600), mounted read-only in its container, where the relay (browser/devtools_relay.py) lets nothing
/// through without it: not another account of a shared server reaching 127.0.0.1:<port>, not a page in the browser.
/// The bridge shows it as a header (`CDP_HEADER`), Hermes as the start of its `browser.cdp_url`'s path (`cdp_url`).
pub const CDP_SECRET: &str = "cdp.token";
pub const CDP_SECRET_MOUNT: &str = "/run/bipagents/cdp.token";
pub const CDP_HEADER: &str = "x-bip-cdp";

static SECRETS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<u16, String>>> =
    std::sync::LazyLock::new(Default::default);

fn read_secret(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let secret = text.trim();
    (secret.len() >= 16 && secret.bytes().all(|b| b.is_ascii_alphanumeric())).then(|| secret.to_string())
}

/// The secret of the browser whose DevTools are on 127.0.0.1:`port` (its folder's, as `ports_of` last read it).
pub fn devtools_secret(port: u16) -> Option<String> {
    SECRETS.lock().unwrap_or_else(|e| e.into_inner()).get(&port).cloned()
}

/// The agent's browser's secret, made now when it has none (kept as it is otherwise: a running container has it).
pub fn ensure_cdp_secret(place: &Path) -> std::io::Result<String> {
    let path = place.join(CDP_SECRET);
    let secret = match read_secret(&path) {
        Some(secret) => secret,
        None => {
            let secret = crate::py::token_hex(32);
            crate::py::write_with_mode(&path.to_string_lossy(), format!("{secret}\n").as_bytes(), 0o600)?;
            secret
        }
    };
    crate::py::chmod(&path.to_string_lossy(), 0o600)?;
    Ok(secret)
}

/// Hermes' `browser.cdp_url`: its relay, the secret in front of the path (Hermes takes a URL, no header); the relay
/// takes it off and puts it back in the WebSocket address its /json/version names.
pub fn cdp_url(port: u16, secret: &str) -> String {
    format!("http://127.0.0.1:{port}/t/{secret}")
}

/// Nothing listens on 127.0.0.1:<port>.
pub fn free(port: u16) -> bool {
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err()
}

/// The ports are this install's own (first_port and the `ports` after it): another install on the machine never
/// hands them out, even while its browsers are stopped.
pub fn in_range(config: &Config, ports: &IndexMap<String, u16>) -> bool {
    let first = config.browser.first_port as u32;
    ports.values().all(|&p| first <= p as u32 && (p as u32) < first + config.browser.ports as u32)
}

/// Two ports in a row, in this install's range, that no other agent's browser has and nothing listens on.
pub fn pick_ports(config: &Config, is_free: &dyn Fn(u16) -> bool) -> Result<IndexMap<String, u16>, BrowserError> {
    let mut taken: HashSet<u32> = HashSet::new();
    let root = PathBuf::from(&config.browser.root);
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            taken.extend(ports_of(config, &name).unwrap_or_default().values().map(|&p| p as u32));
        }
    }
    let first = config.browser.first_port as u32;
    let end = first + config.browser.ports as u32;
    let mut port = first;
    while port + 1 < end {
        if port < u16::MAX as u32
            && !taken.contains(&port)
            && !taken.contains(&(port + 1))
            && is_free(port as u16)
            && is_free((port + 1) as u16)
        {
            let mut out = IndexMap::new();
            out.insert("cdp".to_string(), port as u16);
            out.insert("vnc".to_string(), (port + 1) as u16);
            return Ok(out);
        }
        port += 2;
    }
    Err(BrowserError("no free port for a new browser".into()))
}

/// `locale.getlocale()[0]`: the locale the environment sets for character types.
fn getlocale() -> Option<String> {
    for name in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                let base = value.split('.').next().unwrap_or("").split('@').next().unwrap_or("").to_string();
                return if base == "C" || base == "POSIX" || base.is_empty() { None } else { Some(base) };
            }
        }
    }
    None
}

fn language(config: &Config) -> String {
    if !config.browser.language.is_empty() {
        return config.browser.language.clone();
    }
    let found =
        std::env::var("LANG").ok().filter(|l| !l.is_empty()).or_else(getlocale).unwrap_or_else(|| "en_US".into());
    let found = found.split('.').next().unwrap_or("").to_string();
    if found != "C" && found != "POSIX" {
        found.replace('_', "-")
    } else {
        "en-US".into()
    }
}

fn timezone(config: &Config) -> String {
    if !config.browser.timezone.is_empty() {
        return config.browser.timezone.clone();
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime") {
        if let Some((_, zone)) = link.to_string_lossy().split_once("zoneinfo/") {
            return zone.to_string();
        }
    }
    std::env::var("TZ").unwrap_or_else(|_| "UTC".into())
}

fn first_part(lang: &str) -> &str {
    lang.split('-').next().unwrap_or(lang)
}

/// Chrome's WebRTC kept to proxied traffic: no UDP of its own, so an in-page torrent client (WebTorrent) reaches no
/// peer, and no address of this machine leaks to a page (send-guard.md, 2026-10-28).
pub const WEBRTC_FLAG: &str = "--force-webrtc-ip-handling-policy=disable_non_proxied_udp";

/// `podman run` (or docker) for the agent's container. `egress`: its network and Chrome's proxy flag when it goes
/// through the agents' proxy (send_guard.rs `browser_egress`).
pub fn run_args(
    config: &Config,
    agent: &str,
    ports: &IndexMap<String, u16>,
    engine: &str,
    gpu: bool,
    egress: Option<&(String, String)>,
) -> Vec<String> {
    let place = folder(config, agent);
    let lang = language(config);
    let mut flags = vec![format!("--accept-lang={lang},{}", first_part(&lang)), WEBRTC_FLAG.to_string()];
    if let Some((_, proxy)) = egress {
        flags.push(proxy.clone());
    }
    if gpu {
        flags.extend(GPU_FLAGS.iter().map(|f| f.to_string()));
    }
    // --test-type and --hide-crash-restore-bubble: no warning bar about --no-sandbox (the container is the sandbox)
    // and no « Restore pages? » after the container was stopped, over what the person watches.
    // Maximized on the 1366×768 screen, its panel kept: as on a laptop (never full screen, which sites read as a robot).
    let base = [
        "--start-maximized",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-dev-shm-usage",
        "--ignore-gpu-blocklist",
        "--test-type",
        "--hide-crash-restore-bubble",
    ];
    let browser_args: Vec<String> = base.iter().map(|s| s.to_string()).chain(flags).collect();
    let tz = timezone(config);
    let port = |name: &str| ports.get(name).copied().unwrap_or(0);
    let mut args: Vec<String> = vec![
        engine.into(),
        "run".into(),
        "--rm".into(),
        // the installer's own image, never looked for on a registry (a missing one fails at once, not after a search)
        "--pull=never".into(),
        "--name".into(),
        container(agent),
        "--hostname".into(),
        "desktop".into(),
        "--cap-drop".into(),
        "ALL".into(),
        "--security-opt".into(),
        "no-new-privileges".into(),
        "--memory".into(),
        "3g".into(),
        "--shm-size".into(),
        "1g".into(),
        "-v".into(),
        format!("{}:/home/bip/profile:Z", place.join("profile").to_string_lossy()),
        "-v".into(),
        format!("{}:/run/bipagents/vnc.pass:ro,Z", place.join("vnc.pass").to_string_lossy()),
        "-v".into(),
        format!("{}:{CDP_SECRET_MOUNT}:ro,Z", place.join(CDP_SECRET).to_string_lossy()),
        "-p".into(),
        format!("127.0.0.1:{}:9223", port("cdp")),
        "-p".into(),
        format!("127.0.0.1:{}:5900", port("vnc")),
        "-e".into(),
        format!("TZ={tz}"),
        "-e".into(),
        format!("STEALTH_TIMEZONE={tz}"),
        "-e".into(),
        format!("LANG={}.UTF-8", lang.replace('-', "_")),
        "-e".into(),
        format!("LANGUAGE={}:{}", lang.replace('-', "_"), first_part(&lang)),
        "-e".into(),
        format!("STEALTH_BROWSER_ARGS={}", browser_args.join(";")),
    ];
    let podman = Path::new(engine).file_name().map(|n| n == "podman").unwrap_or(false);
    if podman {
        // The agent's files stay the person's on the host (uid 1000 in the container is them).
        args.splice(2..2, ["--userns=keep-id:uid=1000,gid=1000".to_string()]);
    } else {
        // SAFETY: getuid/getgid cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        args.splice(2..2, ["--user".to_string(), format!("{uid}:{gid}"), "-e".to_string(), "HOME=/tmp".to_string()]);
    }
    if gpu {
        args.extend(["--device".to_string(), "/dev/dri".to_string()]);
        if podman {
            args.extend(["--group-add".to_string(), "keep-groups".to_string()]);
        }
    }
    if let Some((network, _)) = egress {
        args.extend(["--network".to_string(), network.clone()]);
    }
    args.push(config.browser.image.clone());
    args
}

pub fn systemd_unit(agent: &str, engine: &str, args: &[String]) -> String {
    let name = container(agent);
    [
        "[Unit]".to_string(),
        format!("Description=Visible browser of the agent {agent} (BipAgents)"),
        String::new(),
        "[Service]".to_string(),
        format!("ExecStartPre=-{}", pyx::shlex_join(&[engine, "rm", "-f", &name])),
        format!("ExecStart={}", pyx::shlex_join(args)),
        format!("ExecStop={}", pyx::shlex_join(&[engine, "stop", "-t", "10", &name])),
        "Restart=always".to_string(),
        "RestartSec=10".to_string(),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// VNC Authentication only uses the first 8 characters; the port only listens on 127.0.0.1.
pub fn vnc_password() -> String {
    use rand::Rng;
    const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..8).map(|_| LETTERS[rand::rngs::OsRng.gen_range(0..LETTERS.len())] as char).collect()
}

pub fn gpu_available(config: &Config) -> bool {
    if config.browser.gpu != "auto" {
        return false;
    }
    let readable = std::ffi::CString::new("/dev/dri")
        .map(|p| unsafe { libc::access(p.as_ptr(), libc::R_OK) } == 0)
        .unwrap_or(false);
    readable
        && std::fs::read_dir("/dev/dri")
            .map(|entries| entries.flatten().any(|e| e.file_name().to_string_lossy().starts_with("renderD")))
            .unwrap_or(false)
}

/// The `bip-browser-mcp` binary next to this program (or, for a test binary in `deps/`, one folder up).
pub fn proxy_binary() -> String {
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..2 {
            let Some(d) = dir else { break };
            let candidate = d.join(PROXY_BINARY);
            if candidate.exists() {
                return candidate.to_string_lossy().into_owned();
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    pyx::which(PROXY_BINARY).unwrap_or_else(|| PROXY_BINARY.to_string())
}

/// Hermes' `stealth` MCP server: the one in the container, or (`proxy`: its bip-browser-mcp config) the one that
/// starts it when used.
pub fn mcp_server(engine: &str, agent: &str, proxy: Option<&Path>) -> Value {
    // A call that hangs gives up after two minutes, not Hermes' five; get_instance_state hangs on some pages (the agent
    // has get_active_tab and list_tabs).
    let command: Vec<String> = match proxy {
        Some(p) => vec![proxy_binary(), p.to_string_lossy().into_owned()],
        None => vec![engine.into(), "exec".into(), "-i".into(), container(agent), "/opt/stealth/mcp.sh".into()],
    };
    json!({"command": command[0], "args": command[1..], "connect_timeout": 90, "timeout": 120,
           "tools": {"exclude": ["get_instance_state"]}})
}

/// bip-browser-mcp's settings for the agent.
pub fn proxy_config(config: &Config, agent: &str, engine: &str, ports: &IndexMap<String, u16>) -> Value {
    let place = folder(config, agent);
    let unit = container(agent);
    let systemctl = pyx::which("systemctl").unwrap_or_else(|| "systemctl".into()); // Hermes may start it with a PATH of its own
    json!({
        "start": [systemctl, "--user", "start", unit],
        "ready": [engine, "exec", container(agent), "test", "-e", SCREEN],
        "active": [systemctl, "--user", "is-active", "--quiet", unit],
        "server": [engine, "exec", "-i", container(agent), "/opt/stealth/mcp.sh"],
        "stop": [systemctl, "--user", "stop", unit],
        "cache": place.join("mcp-cache.json").to_string_lossy(), "busy": place.join("busy").to_string_lossy(),
        "cdp_port": ports.get("cdp").copied().unwrap_or(0),
        "idle": config.browser.idle_minutes as u64 * 60,
        "ended": place.join("ended").to_string_lossy(), "after_run": crate::browser_mcp::AFTER_RUN_SECONDS,
        "shared_cache": PathBuf::from(&config.browser.root).join("mcp-cache.json").to_string_lossy(),
        "ready_seconds": crate::browser_mcp::READY_SECONDS,
    })
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn no_env() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn hermes_env(home: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("HERMES_HOME".to_string(), home.to_string())])
}

/// The agent's browser, started; its Hermes profile pointed at it. `home` is the HERMES_HOME of the gateway that
/// serves the agent. Returns the agent with its `desktop_vnc` set (the caller saves it).
pub async fn setup(config: &Config, agent: &AgentConfig, run: &Run, home: &str) -> anyhow::Result<AgentConfig> {
    setup_with(&Host::real(config), config, agent, run, home).await
}

pub async fn setup_with(
    host: &Host,
    config: &Config,
    agent: &AgentConfig,
    run: &Run,
    home: &str,
) -> anyhow::Result<AgentConfig> {
    if host.system != "Linux" {
        return Err(BrowserError("the visible browser needs Linux".into()).into());
    }
    let Some(engine) = host.runtime.clone() else {
        return Err(BrowserError("the visible browser needs podman (or docker)".into()).into());
    };
    let name = agent.name.as_str();
    let place = folder(config, name);
    let ports = match ports_of(config, name) {
        // none yet, or from before this install had a range of its own
        Some(p) if !p.is_empty() && in_range(config, &p) => p,
        _ => pick_ports(config, &*host.is_free)?,
    };
    let unit = host.unit_path(name);
    // With `idle_minutes`, a container never set up is left for its first use (bip-browser-mcp writes its unit).
    let deferred = on_demand(config) && !unit.exists();
    std::fs::create_dir_all(if deferred { place.clone() } else { place.join("profile") })?;
    crate::py::chmod(&place.to_string_lossy(), 0o700)?;
    let password = place.join("vnc.pass");
    if !password.exists() {
        std::fs::write(&password, format!("{}\n", vnc_password()))?;
    }
    crate::py::chmod(&password.to_string_lossy(), 0o600)?;
    let secret = ensure_cdp_secret(&place)?;
    let ports_value = Value::Object(ports.iter().map(|(k, v)| (k.clone(), json!(v))).collect());
    std::fs::write(place.join("ports.json"), format!("{}\n", crate::py::dumps(&ports_value)))?;

    // through the agents' proxy where the sandboxes are isolated (send_guard.rs): its network and the proxy up first,
    // else straight out as before
    let cdp_port = ports.get("cdp").copied().unwrap_or(0);
    let egress = match crate::send_guard::browser_egress(config, &engine, cdp_port) {
        Some(found) if crate::sandbox::network(config, run).await.is_ok() => {
            crate::sandbox_sites::ensure_agent(config, name)?;
            Some(found)
        }
        _ => None,
    };
    let gpu = gpu_available(config);
    let text = systemd_unit(name, &engine, &run_args(config, name, &ports, &engine, gpu, egress.as_ref()));
    let changed = unit.exists() && std::fs::read_to_string(&unit).map(|t| t != text).unwrap_or(true);
    if !deferred {
        if let Some(parent) = unit.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&unit, &text)?;
        run(args(&["systemctl", "--user", "daemon-reload"]), no_env()).await?;
    }
    let stem = container(name);
    let mut proxy = None;
    if on_demand(config) {
        // Started by its first use (bip-browser-mcp), not with the session; one running now is stopped once unused.
        let path = place.join("mcp.json");
        let mut settings = proxy_config(config, name, &engine, &ports);
        let systemctl = pyx::which("systemctl").unwrap_or_else(|| "systemctl".into());
        settings["unit"] = json!({"path": unit.to_string_lossy(), "text": text});
        settings["profile"] = json!(place.join("profile").to_string_lossy());
        settings["reload"] = json!([systemctl, "--user", "daemon-reload"]);
        std::fs::write(&path, format!("{}\n", crate::py::Dumps::new().indent(2).dumps(&settings)))?;
        if !deferred {
            run(args(&["systemctl", "--user", "disable", &stem]), no_env()).await?;
        }
        if changed {
            // set up before with other flags: started again with these if it runs (its profile stays)
            run(args(&["systemctl", "--user", "try-restart", &stem]), no_env()).await?;
        }
        proxy = Some(path);
    } else {
        run(args(&["systemctl", "--user", "enable", "--now", &stem]), no_env()).await?;
        if changed {
            // set up before with other flags: its container starts again with these (its profile stays)
            run(args(&["systemctl", "--user", "restart", &stem]), no_env()).await?;
        }
    }

    let env = hermes_env(home);
    let hermes = config.hermes.bin.as_str();
    let cdp = ports.get("cdp").copied().unwrap_or(0);
    let vnc = ports.get("vnc").copied().unwrap_or(0);
    run(
        args(&[hermes, "-p", name, "config", "set", "browser.cdp_url", &cdp_url(cdp, &secret)]),
        env.clone(),
    )
    .await?;
    let server = crate::py::dumps(&mcp_server(&engine, name, proxy.as_deref()));
    run(args(&[hermes, "-p", name, "config", "set", "mcp_servers.stealth", &server]), env.clone()).await?;
    // The vault's tools ride with Hermes' browser toolset; they act on this Chrome through its DevTools port. Of that
    // toolset the agent keeps only them (`browser.vault_only`, hermes/patches 0007): it browses with its `stealth`
    // tools, in the browser the person watches, never with Hermes' own.
    run(args(&[hermes, "-p", name, "tools", "enable", "browser", "--platform", "api_server"]), env.clone()).await?;
    set_vault_only(home, name)?;
    let skill_path =
        Path::new(home).join("profiles").join(name).join("skills").join("visible-browser").join("SKILL.md");
    if let Some(parent) = skill_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&skill_path, skill())?;

    let mut agent = agent.clone();
    agent.desktop_vnc = Some(format!("tcp://127.0.0.1:{vnc}"));
    agent.desktop_vnc_password_file = Some(password.to_string_lossy().into_owned());
    tracing::info!(agent = %name, cdp = cdp, vnc = vnc, "browser set up");
    Ok(agent)
}

/// This install can give an agent its own browser from the app: Linux, a container runtime, and the image the
/// installer built (`browser.enabled`).
pub fn available(config: &Config) -> bool {
    available_with(&Host::real(config), config)
}

pub fn available_with(host: &Host, config: &Config) -> bool {
    config.browser.enabled && host.system == "Linux" && host.runtime.is_some()
}

fn profile_config(home: &str, agent: &str) -> PathBuf {
    Path::new(home).join("profiles").join(agent).join("config.yaml")
}

/// Whether the agent's browser is on (its MCP server enabled in its Hermes profile); None: it has none.
pub fn state(home: &str, agent: &str) -> Option<bool> {
    let text = std::fs::read_to_string(profile_config(home, agent)).ok()?;
    let mut data = pyyaml::load(&text).ok()?;
    if !data.truthy() {
        data = Yaml::map();
    }
    if !data.is_map() {
        return None;
    }
    let server = data.get("mcp_servers").filter(|m| m.truthy())?.get("stealth")?;
    if !server.is_map() {
        return None;
    }
    let enabled = server.get("enabled").map(Yaml::py_str).unwrap_or_else(|| "True".into());
    Some(!matches!(pyx::strip(&enabled).to_lowercase().as_str(), "false" | "0" | "no" | "off"))
}

/// Its MCP server and its skill on or off in its profile, the server's entry kept (Hermes stops or starts it within
/// a minute; a request after this one already has its tools or not).
fn mark(home: &str, agent: &str, on: bool) -> anyhow::Result<()> {
    let path = profile_config(home, agent);
    let mut data = pyyaml::load(&std::fs::read_to_string(&path)?)?;
    if !data.truthy() {
        data = Yaml::map();
    }
    let server = data
        .get_mut("mcp_servers")
        .and_then(|m| m.get_mut("stealth"))
        .filter(|s| s.is_map())
        .ok_or_else(|| anyhow::anyhow!("'mcp_servers'"))?;
    server.insert("enabled", Yaml::Bool(on));
    let mut skills = match data.get("skills") {
        Some(s) if s.is_map() => s.clone(),
        _ => Yaml::map(),
    };
    let disabled: Vec<Yaml> = match skills.get("disabled").filter(|d| d.truthy()) {
        Some(Yaml::Seq(items)) => items.clone(),
        Some(Yaml::Str(s)) => s.chars().map(|c| Yaml::str(c.to_string())).collect(),
        Some(Yaml::Map(pairs)) => pairs.iter().map(|(k, _)| k.clone()).collect(),
        _ => Vec::new(),
    };
    let mut off: Vec<Yaml> = disabled.into_iter().filter(|name| name.as_str() != Some("visible-browser")).collect();
    if !on {
        off.push(Yaml::str("visible-browser"));
    }
    skills.insert("disabled", Yaml::Seq(off));
    data.insert("skills", skills);
    std::fs::write(&path, pyyaml::dump(&data))?;
    if on {
        set_vault_only(home, agent)?;
    }
    Ok(())
}

/// Whether its profile has `browser.vault_only` (Hermes' own browser tools off, the vault's kept).
pub fn vault_only(home: &str, agent: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(profile_config(home, agent)) else { return false };
    let Ok(data) = pyyaml::load(&text) else { return false };
    data.get("browser")
        .filter(|b| b.is_map())
        .and_then(|b| b.get("vault_only"))
        .map(|v| matches!(pyx::strip(&v.py_str()).to_lowercase().as_str(), "true" | "1" | "yes" | "on"))
        .unwrap_or(false)
}

/// `browser.vault_only: true` in its profile (hermes/patches 0007): of Hermes' `browser` toolset only the vault's
/// tools stay, so every page the agent opens is in its visible browser (the `stealth` tools), the one « Bureau »
/// shows. Hermes reads it at its next request. A Hermes without the patch ignores it.
pub fn set_vault_only(home: &str, agent: &str) -> anyhow::Result<()> {
    if vault_only(home, agent) {
        return Ok(());
    }
    let path = profile_config(home, agent);
    let mut data = match std::fs::read_to_string(&path) {
        Ok(text) => pyyaml::load(&text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Yaml::map(),
        Err(e) => return Err(e.into()),
    };
    if !data.truthy() {
        data = Yaml::map();
    }
    if !data.is_map() {
        return Ok(());
    }
    let mut browser = match data.get("browser") {
        Some(b) if b.is_map() => b.clone(),
        _ => Yaml::map(),
    };
    browser.insert("vault_only", Yaml::Bool(true));
    data.insert("browser", browser);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, pyyaml::dump(&data))?;
    Ok(())
}

/// At the bridge's start: every agent whose visible browser is on gets `browser.vault_only` (those set up before it
/// browsed with Hermes' own browser tools too, which could run while its container was stopped, unseen in « Bureau »).
/// Agents set up by hand on their server are left as they are.
pub async fn vault_only_all(config: &Config) {
    for agent in config.agents.values() {
        if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) || ports_of(config, &agent.name).is_none() {
            continue;
        }
        let home = crate::agents_admin::home_root(config, agent);
        if state(&home, &agent.name) != Some(true) {
            continue;
        }
        // Its skill as this bridge has it (it tells the agent which tools to browse with).
        let skill_path =
            Path::new(&home).join("profiles").join(&agent.name).join("skills").join("visible-browser").join("SKILL.md");
        if skill_path.exists() && std::fs::read_to_string(&skill_path).map(|t| t != skill()).unwrap_or(true) {
            if let Err(exc) = std::fs::write(&skill_path, skill()) {
                tracing::warn!(agent = %agent.name, error = %exc, "browser skill not updated");
            }
        }
        if vault_only(&home, &agent.name) {
            continue;
        }
        match set_vault_only(&home, &agent.name) {
            Ok(()) => tracing::info!(agent = %agent.name, "browser: Hermes' own browser tools off, the vault's kept"),
            Err(exc) => tracing::warn!(agent = %agent.name, error = %exc, "browser.vault_only not set"),
        }
    }
}

/// The app's « Naviguer à ta place »: on sets the agent's browser up the first time (`setup`), then only turns it
/// back on; off turns its tools off and stops its Chrome, keeping its profile (the sites it is logged in to).
pub async fn switch(
    config: &Config,
    agent: &AgentConfig,
    on: bool,
    run: &Run,
    home: &str,
) -> anyhow::Result<AgentConfig> {
    switch_with(&Host::real(config), config, agent, on, run, home).await
}

pub async fn switch_with(
    host: &Host,
    config: &Config,
    agent: &AgentConfig,
    on: bool,
    run: &Run,
    home: &str,
) -> anyhow::Result<AgentConfig> {
    let env = hermes_env(home);
    let hermes = config.hermes.bin.as_str();
    let name = agent.name.as_str();
    if on {
        if state(home, name).is_none() || !folder(config, name).exists() {
            if !available_with(host, config) {
                return Err(BrowserError("this install has no browser for its agents".into()).into());
            }
            return setup_with(host, config, agent, run, home).await;
        }
        mark(home, name, true)?;
        run(args(&[hermes, "-p", name, "tools", "enable", "browser", "--platform", "api_server"]), env).await?;
        tracing::info!(agent = %name, "browser on");
        return Ok(agent.clone());
    }
    if state(home, name).is_some() {
        mark(home, name, false)?;
    }
    run(args(&[hermes, "-p", name, "tools", "disable", "browser", "--platform", "api_server"]), env).await?;
    let _ = run(args(&["systemctl", "--user", "stop", &container(name)]), no_env()).await;
    tracing::info!(agent = %name, "browser off");
    Ok(agent.clone())
}

/// The agent's browser stopped, its container and profile gone (its Hermes profile is the caller's).
pub async fn remove(config: &Config, agent: &str, run: &Run) -> anyhow::Result<()> {
    remove_with(&Host::real(config), config, agent, run).await
}

pub async fn remove_with(host: &Host, config: &Config, agent: &str, run: &Run) -> anyhow::Result<()> {
    let unit = host.unit_path(agent);
    let stem = container(agent);
    if unit.exists() {
        let _ = run(args(&["systemctl", "--user", "disable", "--now", &stem]), no_env()).await;
        let _ = std::fs::remove_file(&unit);
        let reloaded = run(args(&["systemctl", "--user", "daemon-reload"]), no_env()).await;
        if reloaded.is_ok() {
            let _ = run(args(&["systemctl", "--user", "reset-failed", &stem]), no_env()).await;
        }
    }
    if let Some(engine) = host.runtime.clone() {
        let _ = run(vec![engine, "rm".into(), "-f".into(), stem], no_env()).await;
    }
    let _ = std::fs::remove_dir_all(folder(config, agent));
    tracing::info!(agent = %agent, "browser removed");
    Ok(())
}
