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

/// Where the agent finds what its browser downloaded (sent_files.rs, service_files.rs): `/telechargements/<name>`.
pub const DOWNLOADS: &str = "/telechargements";
/// Chrome's download folder in the container: inside its profile, so it is on this machine (and kept between starts).
pub const DOWNLOADS_IN_CONTAINER: &str = "/home/bip/profile/Downloads";
/// Chrome's managed policies (Chrome, and Chromium on ARM): the image's own file stays, this one is added.
const POLICY_DIRS: [&str; 2] = ["/etc/opt/chrome/policies/managed", "/etc/chromium/policies/managed"];
const POLICY_NAME: &str = "bipagents-downloads.json";
/// Downloads kept so long (the agent hands them over within its task).
pub const DOWNLOADS_KEPT_DAYS: u64 = 30;

/// The agent's browser downloads on this machine.
pub fn downloads_dir(config: &Config, agent: &str) -> PathBuf {
    folder(config, agent).join("profile").join("Downloads")
}

/// The policy that sends every download to `DOWNLOADS_IN_CONTAINER` without asking where (no dialog the agent can't
/// see): outside the profile, which the container writes in, mounted read-only.
pub fn downloads_policy(place: &Path) -> PathBuf {
    place.join("policies").join(POLICY_NAME)
}

/// Writes the downloads policy next to the profile (before any unit that mounts it is written).
pub fn write_downloads_policy(place: &Path) -> std::io::Result<()> {
    let path = downloads_policy(place);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = format!(
        "{}\n",
        // a portal's second invoice is not held back by « download multiple files? » (nobody would see it).
        // Chrome's optional components (~80 MB per agent that browses, agents-tidy.md; downloaded while the agent
        // browses): no on-device AI model, no component updates (its text-to-speech engine…; Chrome keeps updating
        // the ones its security needs, CRLSet and Safe Browsing), no translation offers. With OPTIONAL_OFF.
        json!({"DownloadDirectory": DOWNLOADS_IN_CONTAINER, "PromptForDownloadLocation": false,
               "DefaultAutomaticDownloadsSetting": 1, "GenAILocalFoundationalModelSettings": 1,
               "ComponentUpdatesEnabled": false, "TranslateEnabled": false})
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(())
}

/// The two `-v` arguments that mount the downloads policy (`run_args`).
fn policy_mounts(place: &Path) -> [String; 4] {
    let policy = downloads_policy(place).to_string_lossy().into_owned();
    [
        "-v".into(),
        format!("{policy}:{}/{POLICY_NAME}:ro,Z", POLICY_DIRS[0]),
        "-v".into(),
        format!("{policy}:{}/{POLICY_NAME}:ro,Z", POLICY_DIRS[1]),
    ]
}

/// A unit's text with the downloads policy mounted next to the DevTools secret (a unit written before the policy
/// existed); None when it has it already or has no such mount to follow.
pub fn with_policy_mounts(text: &str, place: &Path) -> Option<String> {
    if text.contains(POLICY_NAME) {
        return None;
    }
    let anchor = format!(":{CDP_SECRET_MOUNT}:ro,Z");
    let at = text.find(&anchor)? + anchor.len();
    let insert = format!(" {}", pyx::shlex_join(&policy_mounts(place)));
    Some(format!("{}{insert}{}", &text[..at], &text[at..]))
}

/// An agent's browser set up before the downloads policy: the policy written, its unit (and the one its MCP proxy
/// writes at first use) given the mounts. The running container keeps its own until it stops (unused); the next
/// start has them. Whether a unit file changed (systemd must reload).
pub fn refresh_downloads(config: &Config, agent: &str) -> bool {
    let place = folder(config, agent);
    if !place.is_dir() {
        return false;
    }
    if let Err(exc) = write_downloads_policy(&place) {
        tracing::warn!(agent = %agent, error = %exc, "browser downloads policy not written");
        return false;
    }
    let mut changed = false;
    let unit = Host::real(config).unit_path(agent);
    if let Some(text) = std::fs::read_to_string(&unit).ok().and_then(|t| with_policy_mounts(&t, &place)) {
        changed = std::fs::write(&unit, text).is_ok();
    }
    let settings_path = place.join("mcp.json");
    if let Ok(Value::Object(mut settings)) =
        std::fs::read_to_string(&settings_path).map(|t| serde_json::from_str(&t).unwrap_or(Value::Null))
    {
        let text = settings.get("unit").and_then(|u| u.get("text")).and_then(Value::as_str).map(str::to_string);
        if let Some(patched) = text.and_then(|t| with_policy_mounts(&t, &place)) {
            if let Some(Value::Object(unit)) = settings.get_mut("unit") {
                unit.insert("text".into(), json!(patched));
                let _ = std::fs::write(&settings_path, format!("{}\n", crate::py::dumps(&Value::Object(settings))));
            }
        }
    }
    if changed {
        tracing::info!(agent = %agent, "browser: downloads go to its profile from its next start");
    }
    changed
}

/// The latest finished downloads, as the agent sees them (« /telechargements/x.pdf (34 KB) »); older than
/// `DOWNLOADS_KEPT_DAYS` removed on the way. Never through a link.
pub fn recent_downloads(config: &Config, agent: &str, n: usize) -> Vec<String> {
    let dir = downloads_dir(config, agent);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let keep = Duration::from_secs(DOWNLOADS_KEPT_DAYS * 86400);
    let mut files: Vec<(std::time::SystemTime, String, u64)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !meta.is_file() || name.starts_with('.') || name.ends_with(".crdownload") {
            continue;
        }
        let at = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        if at.elapsed().is_ok_and(|age| age > keep) {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        files.push((at, name, meta.len()));
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files
        .into_iter()
        .take(n)
        .map(|(_, name, size)| format!("{DOWNLOADS}/{name} ({} KB)", size.div_ceil(1024)))
        .collect()
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

/// The brake's rescue turn (runs.rs `loop_rescue`) goes on for the agent until `deadline` (unix seconds): its browser
/// reads no page meanwhile (bip-browser-mcp reads `rescue`, refuses at once), so the rescue can't hang on the page it
/// was stopped for. Only for an agent whose browser starts on demand (bip-browser-mcp in between) and was set up (its
/// folder there). Returns whether it was written.
pub fn rescue_started(config: &Config, agent: &str, deadline: f64) -> bool {
    let place = folder(config, agent);
    on_demand(config) && place.is_dir() && std::fs::write(place.join("rescue"), format!("{deadline:.0}\n")).is_ok()
}

/// The rescue turn over: the agent's browser reads pages again.
pub fn rescue_ended(config: &Config, agent: &str) {
    let _ = std::fs::remove_file(folder(config, agent).join("rescue"));
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

/// What became of the person's take-over (or hand-back) of the agent's browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The agent has no browser of its own here.
    NoBrowser,
    Done,
    /// Not taken: the bridge holds the browser for a card payment or a vault fill (the flag reads « card »).
    Payment,
}

/// The command that sets the person's hold: never over a payment's (exit 3 then, nothing written).
pub fn human_hold_command(token: &str) -> String {
    format!(
        "[ \"$(head -c 4 {HOLD_FLAG} 2>/dev/null)\" = card ] && exit 3; umask 077; printf 'human %s' {token} > \
         {HOLD_FLAG}"
    )
}

/// The command that lifts the person's hold: a payment's flag is left alone (only the payment's own release, its
/// fields emptied and its page left, removes it).
pub fn human_release_command() -> String {
    // no flag, nothing to do (fix review 11/11, 5th check): never a `rm` that could meet a payment's hold set between
    // the read and it (the payment's own look before typing refuses then, card_pay.rs `calls_check_command`)
    format!("[ -e {HOLD_FLAG} ] || exit 0; [ \"$(head -c 4 {HOLD_FLAG} 2>/dev/null)\" = card ] || rm -f {HOLD_FLAG}")
}

/// The person takes over the agent's browser (or hands it back): its MCP tools and its DevTools wait meanwhile.
/// A card payment's hold is never overwritten nor removed here (security check 11/11, N1): taking over then answers
/// `Hold::Payment`, handing back leaves it.
pub async fn hold(config: &Config, agent: &str, on: bool) -> Hold {
    hold_with(&Host::real(config), config, agent, on).await
}

pub async fn hold_with(host: &Host, config: &Config, agent: &str, on: bool) -> Hold {
    let Some(engine) = host.runtime.clone() else { return Hold::NoBrowser };
    if !has_ports(config, agent) {
        return Hold::NoBrowser;
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
        human_hold_command(&token)
    } else {
        let _ = std::fs::remove_file(&token_file);
        human_release_command()
    };
    let code = (host.exec)(vec![engine, "exec".into(), container(agent), "sh".into(), "-c".into(), command]).await;
    if on && code == 3 {
        let _ = std::fs::remove_file(&token_file);
        tracing::info!(agent = %agent, "browser not taken over: a payment holds it");
        return Hold::Payment;
    }
    if code != 0 {
        tracing::warn!(agent = %agent, on = on, "browser hold not set");
    }
    Hold::Done
}

/// A payment's hold ended (card_pay.rs `release`, after the card's fields were emptied and the page left): the flag
/// removed, or, while the person holds the browser in the app (`person`, they took it over during the payment),
/// turned into the person's hold with a new token, so the agent's tools wait until they hand it back. Only a payment's
/// flag (« card ») is touched, and only the one this payment set (`own`: its token, fix review 11/11, 4th check).
pub async fn end_payment_hold(config: &Config, agent: &str, own: &str, person: bool) -> bool {
    if !person {
        return lift_payment_hold(config, agent, own).await;
    }
    let host = Host::real(config);
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) || !alnum(own) {
        return false;
    }
    // A token already kept (a take-over meeting this end, fix review 11/11, 3rd check, P5) is the one used: the page
    // view and the flag never disagree.
    let token = match hold_token(config, agent) {
        Some(kept) => kept,
        None => {
            let token = crate::py::token_hex(16);
            let token_file = folder(config, agent).join(HOLD_TOKEN);
            if crate::py::write_with_mode(&token_file.to_string_lossy(), token.as_bytes(), 0o600).is_err() {
                tracing::warn!(agent = %agent, "browser hold token not kept");
            }
            token
        }
    };
    let command = payment_to_person_command(own, &token);
    let code = (host.exec)(vec![engine, "exec".into(), container(agent), "sh".into(), "-c".into(), command]).await;
    if code != 0 {
        tracing::warn!(agent = %agent, code = code, "payment hold not handed to the person");
    }
    code == 0
}

fn alnum(token: &str) -> bool {
    !token.is_empty() && token.bytes().all(|b| b.is_ascii_alphanumeric())
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
/// Taken, unless anything holds it already (false). Given back by `lift_payment_hold` with the same token.
pub async fn hold_for_payment(config: &Config, agent: &str, token: &str) -> bool {
    run_hold_command(config, agent, token, payment_hold_command, "payment hold not set").await
}

/// The hold `token` set (a payment's, a vault fill's, or the sweep's re-key) given back: only that one. A late or
/// queued lift (a vault fill's guard after the sweep ended its hold and a payment took the browser, a release that
/// ran on) never ends a hold it did not set (fix review 11/11, 4th check, problem 1).
pub async fn lift_payment_hold(config: &Config, agent: &str, token: &str) -> bool {
    run_hold_command(config, agent, token, payment_release_command, "payment hold not lifted").await
}

async fn run_hold_command(
    config: &Config,
    agent: &str,
    token: &str,
    command: fn(&str) -> String,
    failed: &str,
) -> bool {
    let host = Host::real(config);
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) || !alnum(token) {
        return false;
    }
    let command = command(token);
    let code = (host.exec)(vec![engine, "exec".into(), container(agent), "sh".into(), "-c".into(), command]).await;
    if code != 0 {
        tracing::warn!(agent = %agent, code = code, "{failed}");
    }
    code == 0
}

/// The command that sets a payment's (or a vault fill's) hold: never over another hold (exit 3, nothing written).
pub fn payment_hold_command(token: &str) -> String {
    format!("[ -e {HOLD_FLAG} ] && exit 3; umask 077; printf 'card %s' {token} > {HOLD_FLAG}")
}

/// The command that lifts the hold `token` set: the flag goes only when it is exactly that one.
pub fn payment_release_command(token: &str) -> String {
    format!("[ \"$(cat {HOLD_FLAG} 2>/dev/null)\" = 'card {token}' ] && rm -f {HOLD_FLAG}; exit 0")
}

/// `STALE_CHECK`'s answer when the browser carries a payment's flag.
pub const STALE_HOLD: i32 = 7;

/// The command that looks for a payment's hold in the browser (exit `STALE_HOLD`; 0: none), changing nothing.
pub fn stale_check_command() -> String {
    format!("[ \"$(head -c 4 {HOLD_FLAG} 2>/dev/null)\" = card ] && exit {STALE_HOLD}; exit 0")
}

/// The command that gives a payment's hold `token` (still « card »: nothing changes for the agent), so the bridge can
/// reach the page to leave it; nothing written when the flag is not a payment's (exit 1).
pub fn rekey_hold_command(token: &str) -> String {
    format!(
        "[ \"$(head -c 4 {HOLD_FLAG} 2>/dev/null)\" = card ] || exit 1; umask 077; printf 'card %s' {token} > \
         {HOLD_FLAG}"
    )
}

/// At start (card_pay.rs `end_stale_holds`): whether a payment's (or a vault fill's) hold was left in the agent's
/// browser by a bridge that stopped meanwhile; it carries `token` from now on.
pub async fn stale_payment_hold(config: &Config, agent: &str, token: &str) -> bool {
    let host = Host::real(config);
    let Some(engine) = host.runtime.clone() else { return false };
    if !has_ports(config, agent) || !token.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return false;
    }
    // Each look bounded (fix review 11/11, 4th check, problem 2): a podman that blocks (its storage locked by an image
    // pull) never keeps the agent's lock, which cancel_payment, confirm_payment and « Suspendre » need.
    let run = |command: String| {
        let exec =
            (host.exec)(vec![engine.clone(), "exec".into(), container(agent), "sh".into(), "-c".into(), command]);
        async move { tokio::time::timeout(STALE_LOOK, exec).await.unwrap_or(-1) }
    };
    run(stale_check_command()).await == STALE_HOLD && run(rekey_hold_command(token)).await == 0
}

/// How long one of the sweep's looks into a browser container may take.
const STALE_LOOK: Duration = Duration::from_secs(20);

/// The agent's browser container started again (`--rm`: Chrome, its pages and the hold's flag go with the old one).
/// Whether it was.
pub async fn restart(config: &Config, agent: &str) -> bool {
    let host = Host::real(config);
    let code = (host.exec)(vec!["systemctl".into(), "--user".into(), "restart".into(), container(agent)]).await;
    tracing::info!(agent = %agent, code = code, "browser restarted");
    code == 0
}

/// The command that turns the payment `own`'s hold into the person's (`end_payment_hold`, their `token`): only that
/// payment's flag changes.
pub fn payment_to_person_command(own: &str, token: &str) -> String {
    format!(
        "[ \"$(cat {HOLD_FLAG} 2>/dev/null)\" = 'card {own}' ] && {{ umask 077; printf 'human %s' {token} > \
         {HOLD_FLAG}; }}; exit 0"
    )
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

/// Chrome's optional downloads that no policy stops (the optimization guide's models and hints, 47 MB, data files
/// the component policy exempts), off: less disk, and less traffic and CPU while the agent's first pages load.
/// Chrome only reads the last `--disable-features`: this one, after nodriver's, keeps nodriver's two (site isolation
/// off: fewer processes, less memory).
pub const OPTIONAL_OFF: &str = "--disable-features=IsolateOrigins,site-per-process,OptimizationGuideModelDownloading,\
OptimizationHintsFetching,OptimizationTargetPrediction,OptimizationHints";

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
    let mut flags =
        vec![format!("--accept-lang={lang},{}", first_part(&lang)), WEBRTC_FLAG.to_string(), OPTIONAL_OFF.to_string()];
    if let Some((_, proxy)) = egress {
        flags.push(proxy.clone());
    }
    if gpu {
        flags.extend(GPU_FLAGS.iter().map(|f| f.to_string()));
    }
    // --test-type and --hide-crash-restore-bubble: no warning bar about --no-sandbox (the container is the sandbox)
    // and no « Restore pages? » after the container was stopped, over what the person watches.
    // Maximized on the 1366×768 screen, its panel kept: as on a laptop (never full screen, which sites read as a robot).
    // No back/forward cache: a page left keeps no live copy that « back » brings again (a card payment's page, besides
    // the history card_pay.rs forgets after it; fix review 11/11, B2).
    let base = [
        "--start-maximized",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-dev-shm-usage",
        "--ignore-gpu-blocklist",
        "--test-type",
        "--hide-crash-restore-bubble",
        "--disable-back-forward-cache",
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
    // the downloads policy, right after the DevTools secret (`with_policy_mounts` patches an older unit the same way)
    let after_secret = args.iter().position(|a| a.ends_with(&format!(":{CDP_SECRET_MOUNT}:ro,Z"))).map_or(0, |i| i + 1);
    args.splice(after_secret..after_secret, policy_mounts(&place));
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
        // a command run in the container (a screenshot the real server saved there, fetched for the agent)
        "exec": [engine, "exec", container(agent)],
        "stop": [systemctl, "--user", "stop", unit],
        "cache": place.join("mcp-cache.json").to_string_lossy(), "busy": place.join("busy").to_string_lossy(),
        "cdp_port": ports.get("cdp").copied().unwrap_or(0),
        "idle": config.browser.idle_minutes as u64 * 60,
        "ended": place.join("ended").to_string_lossy(), "after_run": crate::browser_mcp::AFTER_RUN_SECONDS,
        "rescue": place.join("rescue").to_string_lossy(),
        "shared_cache": PathBuf::from(&config.browser.root).join("mcp-cache.json").to_string_lossy(),
        "ready_seconds": crate::browser_mcp::READY_SECONDS,
        // the phone exit's hub (phone_exit): why a page failed (`WHY`), a challenge page asked about (`ASK`)
        "agent": agent,
        "exit_sock": if config.phone_exit.enabled {
            crate::phone_exit::socket_path(config).to_string_lossy().to_string()
        } else {
            String::new()
        },
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
    // mounted by the unit (`run_args`): there before any unit is written, deferred or not
    write_downloads_policy(&place)?;
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
    crate::profile_files::set(
        &profile_config(home, name),
        &[
            ("browser.cdp_url", Yaml::str(cdp_url(cdp, &secret))),
            ("mcp_servers.stealth", crate::agents_admin::pyyaml::from_json(&mcp_server(&engine, name, proxy.as_deref()))),
        ],
    )?;
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
    crate::profile_files::try_edit(&profile_config(home, agent), |data| {
        let server = data
            .get_mut("mcp_servers")
            .and_then(|m| m.get_mut("stealth"))
            .filter(|s| s.is_map())
            .ok_or_else(|| anyhow::anyhow!("'mcp_servers'"))?;
        server.insert("enabled", Yaml::Bool(on));
        let skills = data.section("skills").ok_or_else(|| anyhow::anyhow!("'skills'"))?;
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
        Ok(())
    })?;
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
    crate::profile_files::edit(&profile_config(home, agent), |data| {
        if let Some(browser) = data.section("browser") {
            browser.insert("vault_only", Yaml::Bool(true));
        }
    })?;
    Ok(())
}

/// The visible browser's MCP server run by this bridge's program (`proxy`) when it names another one: an older
/// version's (each version's binaries are in a folder of their own, the one before kept), or one gone (aibox
/// 2026-10-26: Vie's and Kerfy Studio's spaces had lost their browser after an update). Hermes starts the command at
/// its gateway's next start: a gateway still running the old one is restarted once idle ([`stale_proxies`],
/// agent_restart.rs). profile_files::reconcile.
pub fn with_proxy(data: &mut Yaml, proxy: &str) {
    let Some(server) = data.get_mut("mcp_servers").and_then(|m| m.get_mut("stealth")).filter(|s| s.is_map()) else {
        return;
    };
    let Some(command) = server.get("command").and_then(|c| c.as_str()) else { return };
    if command.ends_with(PROXY_BINARY) && command != proxy {
        server.insert("command", Yaml::str(proxy));
    }
}

/// The agents whose browser tools run now through another program than this bridge's `bip-browser-mcp`: a
/// `bip-browser-mcp` of this account (Hermes' gateway starts it with the agent's `mcp.json`) whose binary isn't this
/// bridge's (an older version's folder, or replaced: « (deleted) »). 9/11: after a wave the bridge was new and the
/// gateways kept the old proxy, so the release's browser fixes never reached people. Linux only (/proc).
pub fn stale_proxies(config: &Config) -> Vec<String> {
    let Ok(proxy) = Path::new(&proxy_binary()).canonicalize() else { return Vec::new() };
    let mut running = Vec::new();
    for entry in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        if !entry.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // another account's process: its binary can't be read (and isn't this bridge's business)
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else { continue };
        let Ok(raw) = std::fs::read(entry.path().join("cmdline")) else { continue };
        let args = raw.split(|b| *b == 0).map(|a| String::from_utf8_lossy(a).into_owned()).collect();
        running.push((exe, args));
    }
    stale_of(config, &proxy, &running)
}

/// [`stale_proxies`] over the processes given: (its binary, its arguments).
pub fn stale_of(config: &Config, proxy: &Path, running: &[(PathBuf, Vec<String>)]) -> Vec<String> {
    // the agents, and the spare ones the hub's gateway serves too (agent_pool.rs: their own browser, not in `[agents]`)
    // and the abandoned ones it still serves until they are removed (a batch cut short, agent_pool.rs `abandoned`)
    let mut served: Vec<String> = config.agents.values().map(|a| a.name.clone()).collect();
    served.extend(crate::agent_pool::spares(config).into_iter().map(|s| s.id));
    served.extend(crate::agent_pool::abandoned(config));
    let mut out: Vec<String> = Vec::new();
    for (exe, args) in running {
        let shown = exe.to_string_lossy();
        let plain = shown.strip_suffix(" (deleted)").unwrap_or(&shown);
        if Path::new(plain).file_name().is_none_or(|n| n != PROXY_BINARY) || exe.as_path() == proxy {
            continue;
        }
        let Some(settings) = args.get(1) else { continue };
        let found = served.iter().find(|name| folder(config, name).join("mcp.json") == Path::new(settings));
        if let Some(name) = found.filter(|n| !out.contains(n)) {
            out.push(name.clone());
        }
    }
    out
}

/// [`with_proxy`] in each spare agent's profile (agent_pool.rs), the abandoned ones too until they are removed: not in
/// `[agents]`, so not reconciled, but served by the hub's gateway, which starts their browser proxy too (9/11 lab: five
/// spares kept the old one after the wave; 12/11 lab: one spare of a batch cut short at the install did).
pub fn spares_follow(config: &Config) {
    let proxy = proxy_binary();
    if !Path::new(&proxy).exists() {
        return;
    }
    let mut ids: Vec<String> = crate::agent_pool::spares(config).into_iter().map(|s| s.id).collect();
    ids.extend(crate::agent_pool::abandoned(config));
    let profiles = PathBuf::from(crate::agents_admin::install_home(config)).join("profiles");
    for id in ids {
        let path = profiles.join(&id).join("config.yaml");
        if !path.is_file() {
            continue;
        }
        if let Err(exc) = crate::profile_files::edit(&path, |data| with_proxy(data, &proxy)) {
            tracing::warn!(agent = %id, error = %format!("{exc:#}"), "spare's browser proxy not brought up to date");
        }
    }
}

/// At the bridge's start: every agent whose visible browser is on gets `browser.vault_only` (those set up before it
/// browsed with Hermes' own browser tools too, which could run while its container was stopped, unseen in « Bureau »).
/// Agents set up by hand on their server are left as they are.
pub async fn vault_only_all(config: &Config) {
    let mut reload = false;
    for agent in config.agents.values() {
        if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) || ports_of(config, &agent.name).is_none() {
            continue;
        }
        // Its downloads where the agent can hand them over (a browser set up before: its unit given the policy).
        reload |= refresh_downloads(config, &agent.name);
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
    if reload {
        let _ = tokio::process::Command::new("systemctl").args(["--user", "daemon-reload"]).status().await;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `command` run by a real shell against a flag file of its own (in place of the container's): its exit code and
    /// what the flag reads after it (None: no flag).
    fn run_on(flag: &Path, before: Option<&str>, command: &str) -> (i32, Option<String>) {
        match before {
            Some(text) => std::fs::write(flag, text).unwrap(),
            None => {
                let _ = std::fs::remove_file(flag);
            }
        }
        let command = command.replace(HOLD_FLAG, &flag.to_string_lossy());
        let status = std::process::Command::new("sh").args(["-c", &command]).status().unwrap();
        (status.code().unwrap_or(-1), std::fs::read_to_string(flag).ok())
    }

    #[test]
    fn neither_hold_overwrites_nor_lifts_the_other() {
        // Security check 11/11, N1: the person's take-over wrote « human » over a payment's « card » flag, and their
        // hand-back removed whatever flag was there, with the card still typed in the page.
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("hold");
        let card = || Some("card 0123abcd".to_string());
        let human = || Some("human 77aa".to_string());
        // The person's take-over: refused (3) over a payment's hold, which stays; set when nothing holds the browser.
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &human_hold_command("77aa")), (3, card()));
        assert_eq!(run_on(&flag, None, &human_hold_command("77aa")), (0, human()));
        // Their hand-back: a payment's hold stays; theirs (or an older bridge's flag without a token) goes.
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &human_release_command()), (0, card()));
        assert_eq!(run_on(&flag, Some("human 77aa"), &human_release_command()), (0, None));
        assert_eq!(run_on(&flag, Some(""), &human_release_command()), (0, None));
        assert_eq!(run_on(&flag, None, &human_release_command()).0, 0);
        // A payment's hold: never over the person's; its release lifts only its own flag.
        assert_eq!(run_on(&flag, Some("human 77aa"), &payment_hold_command("0123abcd")), (3, human()));
        assert_eq!(run_on(&flag, None, &payment_hold_command("0123abcd")), (0, card()));
        assert_eq!(run_on(&flag, Some("human 77aa"), &payment_release_command("0123abcd")), (0, human()));
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &payment_release_command("0123abcd")), (0, None));
        // Ended while the person holds the browser: theirs from then on, only in place of this payment's.
        let to_person = payment_to_person_command("0123abcd", "99ff");
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &to_person), (0, Some("human 99ff".into())));
        assert_eq!(run_on(&flag, None, &to_person), (0, None));
        assert_eq!(run_on(&flag, Some("human 77aa"), &to_person), (0, human()));
        assert_eq!(run_on(&flag, Some("card 5e5e"), &to_person), (0, Some("card 5e5e".into())));
    }

    #[test]
    fn a_late_lift_never_ends_a_hold_it_did_not_set() {
        // Fix review 11/11, 4th check, problem 1: an aborted vault fill's guard, queued behind the sweep and a
        // payment, removed whatever « card » flag was there, the payment's with the card typed in the page.
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("hold");
        let fill = payment_release_command("f111");
        // The payment's hold (or the sweep's re-key) is not the fill's: it stays.
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &fill), (0, Some("card 0123abcd".into())));
        assert_eq!(run_on(&flag, Some("card f1112"), &fill), (0, Some("card f1112".into())));
        assert_eq!(run_on(&flag, Some("card"), &fill), (0, Some("card".into())));
        assert_eq!(run_on(&flag, Some("human f111"), &fill), (0, Some("human f111".into())));
        // Its own: lifted.
        assert_eq!(run_on(&flag, Some("card f111"), &fill), (0, None));
    }

    #[test]
    fn a_payments_hold_left_behind_is_found_and_given_a_new_token() {
        // Security check 11/11, N2: after a restart the bridge knows no payment's token; it gives the flag one.
        let dir = tempfile::tempdir().unwrap();
        let flag = dir.path().join("hold");
        let card = || Some("card 0123abcd".to_string());
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &stale_check_command()), (STALE_HOLD, card()));
        assert_eq!(run_on(&flag, Some("human 77aa"), &stale_check_command()), (0, Some("human 77aa".into())));
        assert_eq!(run_on(&flag, None, &stale_check_command()), (0, None));
        let rekey = rekey_hold_command("5e5e");
        assert_eq!(run_on(&flag, Some("card 0123abcd"), &rekey), (0, Some("card 5e5e".into())));
        assert_eq!(run_on(&flag, Some("human 77aa"), &rekey), (1, Some("human 77aa".into())));
        assert_eq!(run_on(&flag, None, &rekey), (1, None));
    }
}
