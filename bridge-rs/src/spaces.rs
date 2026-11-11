//! An agent's own space: its own Hermes, apart from the shared hub, so it may hold credentials.
//!
//! In the hub, every agent runs in one gateway under one account: an agent with a terminal could read another's
//! files, vault included. Moving an agent gives it:
//!
//! - its own HERMES_HOME (`<spaces root>/<agent>`) and gateway, on a local port of its own; the app reaches it
//!   through the bridge's routes, so no new port is published on the tailnet;
//! - with podman, its terminal, files, code and browser in a sandbox container (Hermes' `docker` terminal backend,
//!   which runs podman), and attachments limited to its own files (`gateway.strict`);
//! - its conversations, memory, routines, personality and settings, copied from the hub (the database through
//!   SQLite's backup, consistent while the hub keeps running), then the hub forgets it.
//!
//! The service that runs the space's gateway is a systemd user unit (Linux) or a launchd agent (macOS), or the
//! administrator's own commands (`[spaces] start` / `stop`), e.g. a container per space on a server where Hermes
//! itself runs in containers.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::agents_admin::pyyaml::Yaml;
use crate::agents_admin::Run;
use crate::browsers::pyx::shlex_quote;
use crate::config::{AgentConfig, Config};

/// Runtime files of a running gateway or sandbox: never copied (the space starts its own).
pub const SKIP: [&str; 9] = [
    "sandboxes",
    "bot-desktop",
    "gateway.pid",
    "gateway.lock",
    "gateway.sock",
    "gateway_state.json",
    "state.db",
    "state.db-wal",
    "state.db-shm",
];

/// Python's `SpaceError` (a `RuntimeError`, safe to show): returned inside `anyhow::Error`, found with `downcast_ref`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct SpaceError(pub String);

/// How many times the hub's copy of a moved agent is deleted again after a failure (`Spaces::forget_later`).
const FORGET_TRIES: usize = 4;

pub fn unit_name(agent: &str) -> String {
    format!("bipagents-space-{agent}")
}

fn home_dir() -> String {
    crate::py::expanduser("~")
}

/// Python's `template.format(**values)` for an administrator's command (`{agent}`, `{home}`; `{{`/`}}` braces).
pub fn format_keys(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = template.replace("{{", "\u{0}").replace("}}", "\u{1}");
    for (key, value) in values {
        out = out.replace(&format!("{{{key}}}"), value);
    }
    out.replace('\u{0}', "{").replace('\u{1}', "}")
}

pub fn systemd_unit(agent: &str, hermes: &str, home: &str) -> String {
    let env = [
        ("HERMES_HOME", home.to_string()),
        ("PYTHONUNBUFFERED", "1".to_string()),
        ("PATH", format!("{}/.local/bin:/usr/local/bin:/usr/bin:/bin", home_dir())),
    ];
    let exec = [hermes, "gateway", "run"].iter().map(|a| shlex_quote(a)).collect::<Vec<_>>().join(" ");
    let mut lines = vec![
        "[Unit]".to_string(),
        format!("Description=BipAgents space of {agent} (Hermes gateway)"),
        String::new(),
        "[Service]".to_string(),
        format!("ExecStart={exec}"),
        format!("WorkingDirectory={home}"),
        "Restart=on-failure".to_string(),
        "RestartSec=5".to_string(),
    ];
    lines.extend(env.iter().map(|(k, v)| format!("Environment={k}={v}")));
    lines.extend(["".to_string(), "[Install]".to_string(), "WantedBy=default.target".to_string(), String::new()]);
    lines.join("\n")
}

pub fn launchd_plist(agent: &str, hermes: &str, home: &str, log_path: &str) -> String {
    let env = [
        ("HERMES_HOME", home.to_string()),
        ("PYTHONUNBUFFERED", "1".to_string()),
        ("PATH", format!("{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin", home_dir())),
    ];
    let args: String = [hermes, "gateway", "run"].iter().map(|a| format!("<string>{a}</string>")).collect();
    let envs: String = env.iter().map(|(k, v)| format!("<key>{k}</key><string>{v}</string>")).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\
         <key>Label</key><string>io.github.bipagents.space.{agent}</string>\
         <key>ProgramArguments</key><array>{args}</array>\
         <key>EnvironmentVariables</key><dict>{envs}</dict>\
         <key>WorkingDirectory</key><string>{home}</string>\
         <key>RunAtLoad</key><true/><key>KeepAlive</key><true/>\
         <key>StandardOutPath</key><string>{log_path}</string><key>StandardErrorPath</key><string>{log_path}</string>\
         </dict></plist>\n"
    )
}

/// A local port no agent uses and nothing listens on, from `[spaces] first_port`.
pub fn free_port(config: &Config) -> Result<u16, SpaceError> {
    let taken: HashSet<u16> =
        config.agents.values().filter_map(|a| url::Url::parse(&a.hermes_url).ok().and_then(|u| u.port())).collect();
    let first = config.spaces.first_port as u32;
    let mut port = first;
    while port < first + 200 {
        if port <= u16::MAX as u32 && !taken.contains(&(port as u16)) {
            let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
            if std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_err() {
                return Ok(port as u16);
            }
        }
        port += 1;
    }
    Err(SpaceError("no free port for a new space".into()))
}

/// KEY=value lines of a .env: the given ones replaced or added, the others kept.
pub fn set_env(path: &Path, values: &[(&str, String)]) -> std::io::Result<()> {
    let text = if path.exists() { std::fs::read_to_string(path)? } else { String::new() };
    let mut lines: Vec<String> = crate::ssh_keys::splitlines(&text)
        .into_iter()
        .filter(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            !values.iter().any(|(k, _)| *k == key)
        })
        .map(str::to_string)
        .collect();
    lines.extend(values.iter().map(|(k, v)| format!("{k}={v}")));
    std::fs::write(path, format!("{}\n", lines.join("\n")))?;
    crate::py::chmod(&path.to_string_lossy(), 0o600)
}

/// `shutil.copystat`: the mode and the times of `source` on `target` (links themselves: their times only).
fn copy_stat(source: &Path, target: &Path, link: bool) -> std::io::Result<()> {
    let meta = if link { std::fs::symlink_metadata(source)? } else { std::fs::metadata(source)? };
    if !link {
        std::fs::set_permissions(target, meta.permissions())?;
        if let (Ok(accessed), Ok(modified)) = (meta.accessed(), meta.modified()) {
            let times = std::fs::FileTimes::new().set_accessed(accessed).set_modified(modified);
            if let Ok(file) = std::fs::File::open(target) {
                let _ = file.set_times(times);
            }
        }
    }
    Ok(())
}

/// `shutil.copytree(source, target, symlinks=True, ignore=…)`: links stay links (even dangling ones), files are
/// copied with their mode and times.
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if SKIP.contains(&name.as_str()) || name.ends_with(".sock") {
            continue;
        }
        let (from, to) = (entry.path(), target.join(&name));
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(&from)?, &to)?;
        } else if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
            copy_stat(&from, &to, false)?;
        }
    }
    copy_stat(source, target, false)
}

/// The agent's folder, its database included (SQLite backup: consistent while the hub gateway writes).
pub fn copy_profile(source: &Path, target: &Path) -> anyhow::Result<()> {
    copy_tree(source, target)?;
    let database = source.join("state.db");
    if database.exists() {
        let src = rusqlite::Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut dst = rusqlite::Connection::open(target.join("state.db"))?;
        {
            let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
            // Python's `src.backup(dst)`: every page in one step (-1), 250 ms between tries when busy or locked.
            loop {
                match backup.step(-1)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => {}
                    _ => std::thread::sleep(Duration::from_millis(250)),
                }
            }
        }
    }
    Ok(())
}

/// The hub's copy of the conversations again into the space (SQLite backup), after a turn that went on in the hub
/// while the agent moved: its end is kept too. Its API conversations are then the space's (`rebind_api_sessions`).
pub fn resync_database(source: &Path, target: &Path, profile_id: &str) -> anyhow::Result<()> {
    let database = source.join("state.db");
    if !database.exists() {
        return Ok(());
    }
    let src = rusqlite::Connection::open_with_flags(&database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut dst = rusqlite::Connection::open(target.join("state.db"))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        loop {
            match backup.step(-1)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => {}
                _ => std::thread::sleep(Duration::from_millis(250)),
            }
        }
    }
    drop(dst);
    rebind_api_sessions(target, profile_id)?;
    Ok(())
}

/// Hermes' record of an API conversation's owner (`state_meta`, gateway/session_api.py `_BINDING_PREFIX`, v0.22.5):
/// `{"profile_id": <the serving profile's home, resolved>, "session_id", "route", "entry"}`.
pub const API_BINDING_PREFIX: &str = "gateway.api.binding.v1.";

/// The profile's home as the space's Hermes names it (its `profile_id`, gateway/run_runtime.py: the resolved home):
/// inside its container when the space runs in one (`[spaces] container_home`).
pub fn hermes_profile_id(config: &Config, root: &Path, agent: &str) -> String {
    if !config.spaces.container_home.is_empty() {
        return format!("{}/profiles/{agent}", config.spaces.container_home);
    }
    let home = root.join("profiles").join(agent);
    std::fs::canonicalize(&home).unwrap_or(home).to_string_lossy().into_owned()
}

/// The API conversations of a profile's database (copied from the hub) made the space's: Hermes refuses a run in a
/// conversation another profile home owns (`restore_api_session`: HTTP 409 `profile_mismatch`), and every
/// conversation made in the hub names the hub's home. Seen on the token lab (2026-11-07): after its move, an agent
/// answered no message of its pair conversation with another agent (agent_pairs.rs), « Coach didn't answer ».
/// Returns how many were rewritten. A database without Hermes' table is left alone.
pub fn rebind_api_sessions(profile: &Path, profile_id: &str) -> rusqlite::Result<usize> {
    let database = profile.join("state.db");
    if !database.exists() {
        return Ok(0);
    }
    let conn = rusqlite::Connection::open(&database)?;
    let has_table: bool = conn
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='state_meta'", [], |_| Ok(true))
        .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(false) } else { Err(e) })?;
    if !has_table {
        return Ok(0);
    }
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT key, value FROM state_meta WHERE substr(key, 1, ?1) = ?2")?;
        let found = stmt.query_map(rusqlite::params![API_BINDING_PREFIX.len() as i64, API_BINDING_PREFIX], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        found.collect::<rusqlite::Result<_>>()?
    };
    let mut changed = 0;
    for (key, value) in rows {
        let Ok(serde_json::Value::Object(mut receipt)) = serde_json::from_str::<serde_json::Value>(&value) else {
            continue;
        };
        if receipt.get("profile_id").and_then(serde_json::Value::as_str) == Some(profile_id) {
            continue;
        }
        receipt.insert("profile_id".into(), serde_json::Value::String(profile_id.to_string()));
        let text = serde_json::to_string(&receipt).unwrap_or(value);
        changed += conn.execute("UPDATE state_meta SET value = ?1 WHERE key = ?2", rusqlite::params![text, key])?;
    }
    Ok(changed)
}

/// The hub's folder of an agent's profile.
pub fn hub_profile(config: &Config, agent: &str) -> PathBuf {
    let hub = PathBuf::from(if config.hermes.home.is_empty() {
        crate::py::join(&home_dir(), ".hermes")
    } else {
        config.hermes.home.clone()
    });
    hub.join("profiles").join(agent)
}

/// `shutil.which(name)`: an executable of that name on the PATH.
pub fn which(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| {
        candidate.is_file()
            && std::fs::metadata(candidate).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    })
}

pub fn sandbox_available(config: &Config) -> bool {
    if config.spaces.sandbox == "none" {
        return false;
    }
    which("podman").is_some() || which("docker").is_some()
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

pub struct Spaces {
    /// `run(args, env)` → stdout; a failure is Python's `RuntimeError`.
    pub run: Run,
}

impl Spaces {
    pub fn new(run: Run) -> Self {
        Spaces { run }
    }

    async fn run(&self, items: &[&str], env: BTreeMap<String, String>) -> anyhow::Result<String> {
        (self.run)(args(items), env).await
    }

    async fn service(&self, config: &Config, agent: &str, home: &str, start: bool) -> anyhow::Result<()> {
        let spaces = &config.spaces;
        let quoted = shlex_quote(home);
        if let Some(command) = spaces.start.as_deref().filter(|c| !c.is_empty() && start) {
            self.run(&["sh", "-c", &format_keys(command, &[("agent", agent), ("home", &quoted)])], BTreeMap::new())
                .await?;
            return Ok(());
        }
        if let Some(command) = spaces.stop.as_deref().filter(|c| !c.is_empty() && !start) {
            self.run(&["sh", "-c", &format_keys(command, &[("agent", agent), ("home", &quoted)])], BTreeMap::new())
                .await?;
            return Ok(());
        }
        if cfg!(target_os = "macos") {
            let path = PathBuf::from(home_dir())
                .join("Library")
                .join("LaunchAgents")
                .join(format!("io.github.bipagents.space.{agent}.plist"));
            // SAFETY: getuid has no preconditions.
            let domain = format!("gui/{}", unsafe { libc::getuid() });
            if start {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let log =
                    PathBuf::from(home_dir()).join("Library").join("Logs").join(format!("bipagents-space-{agent}.log"));
                std::fs::write(&path, launchd_plist(agent, &config.hermes.bin, home, &log.to_string_lossy()))?;
                self.run(&["launchctl", "bootstrap", &domain, &path.to_string_lossy()], BTreeMap::new()).await?;
            } else {
                self.run(
                    &["launchctl", "bootout", &format!("{domain}/io.github.bipagents.space.{agent}")],
                    BTreeMap::new(),
                )
                .await?;
                let _ = std::fs::remove_file(&path);
            }
            return Ok(());
        }
        let unit = unit_name(agent);
        let path =
            PathBuf::from(home_dir()).join(".config").join("systemd").join("user").join(format!("{unit}.service"));
        if start {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, systemd_unit(agent, &config.hermes.bin, home))?;
            self.run(&["systemctl", "--user", "daemon-reload"], BTreeMap::new()).await?;
            self.run(&["systemctl", "--user", "enable", "--now", &unit], BTreeMap::new()).await?;
        } else {
            self.run(&["systemctl", "--user", "disable", "--now", &unit], BTreeMap::new()).await?;
            let _ = std::fs::remove_file(&path);
            self.run(&["systemctl", "--user", "daemon-reload"], BTreeMap::new()).await?;
            // A stopped gateway may exit non-zero: no « failed » line left in the user's units.
            if let Err(exc) = self.run(&["systemctl", "--user", "reset-failed", &unit], BTreeMap::new()).await {
                if crate::agents_admin::is_os_error(&exc) {
                    return Err(exc);
                }
            }
        }
        Ok(())
    }

    /// The service of an agent that already has its space (after `bipagents import`, on another machine).
    pub async fn start(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<()> {
        let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) else {
            return Err(SpaceError(format!("{} has no space of its own", agent.name)).into());
        };
        self.service(config, &agent.name, space, true).await
    }

    /// `move`: the agent, from the hub into its own space. Returns its new configuration (the caller saves it).
    pub async fn move_agent(
        &self,
        config: &Config,
        agent: &AgentConfig,
        http: &reqwest::Client,
    ) -> anyhow::Result<AgentConfig> {
        let moved = self.move_agent_keeping_hub(config, agent, http).await?;
        // Its space serves it now: the move stands. Until 2026-11-07 a failed delete here failed the whole move with
        // the space left running and the bridge still pointing at the hub (whose copy Hermes had already marked
        // deleted), and each try again then met « a space with this name already exists » (token lab).
        if let Err(exc) = self.forget_in_hub(config, &agent.name).await {
            tracing::warn!(agent = %agent.name, error = %exc, "the hub's copy of the moved agent not deleted yet: tried again later");
            self.forget_later(config, &agent.name);
        }
        Ok(moved)
    }

    /// The hub's copy of a moved agent deleted later, a few times (`[spaces] forget_again`, then 4 times longer each
    /// time): what kept it (« a running gateway still owns this profile ») settles. Done once it is gone.
    pub fn forget_later(&self, config: &Config, agent: &str) {
        let (spaces, config, agent) = (Spaces::new(self.run.clone()), config.clone(), agent.to_string());
        tokio::spawn(async move {
            let mut wait = config.spaces.forget_again;
            for _ in 0..FORGET_TRIES {
                tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                if !hub_profile(&config, &agent).is_dir() {
                    return;
                }
                match spaces.forget_in_hub(&config, &agent).await {
                    Ok(()) => {
                        tracing::info!(agent = %agent, "the hub forgot the moved agent");
                        return;
                    }
                    Err(exc) => {
                        tracing::warn!(agent = %agent, error = %exc, "the hub's copy of the moved agent still not deleted")
                    }
                }
                wait *= 4.0;
            }
            tracing::warn!(agent = %agent, "the hub keeps a copy of the moved agent: `hermes profile delete <agent> --yes` removes it");
        });
    }

    /// The move without its last step: the hub still serves the profile, so a turn going on there ends there (the
    /// move an agent's own call starts, own_space.rs); `forget_in_hub` once it has.
    pub async fn move_agent_keeping_hub(
        &self,
        config: &Config,
        agent: &AgentConfig,
        http: &reqwest::Client,
    ) -> anyhow::Result<AgentConfig> {
        let hub = PathBuf::from(if config.hermes.home.is_empty() {
            crate::py::join(&home_dir(), ".hermes")
        } else {
            config.hermes.home.clone()
        });
        let source = hub.join("profiles").join(&agent.name);
        if agent.isolated || !source.is_dir() {
            return Err(SpaceError("this agent is not in the shared hub".into()).into());
        }
        let root = PathBuf::from(&config.spaces.root).join(&agent.name);
        if root.exists() {
            // Not its space (the bridge serves it from the hub): what a move that didn't finish left, a copy never
            // used. Its gateway stopped and the folder removed, the move made again from the hub.
            if agent.space.as_deref().is_some_and(|s| Path::new(s) == root) {
                return Err(SpaceError("a space with this name already exists".into()).into());
            }
            tracing::warn!(agent = %agent.name, space = %root.display(), "a space left by a move that didn't finish: removed, the move made again");
            if let Err(stop) = self.service(config, &agent.name, &root.to_string_lossy(), false).await {
                tracing::debug!(error = %stop, "the left space's service not stopped");
            }
            std::fs::remove_dir_all(&root)?;
        }
        let port = free_port(config)?;
        let space_key = crate::py::token_urlsafe(32);
        if let Some(parent) = root.parent() {
            std::fs::create_dir_all(parent)?;
        }
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        }
        let result: anyhow::Result<(AgentConfig, bool)> = async {
            // The hub's settings (model, providers), its own listener.
            for name in ["config.yaml", ".env"] {
                if hub.join(name).exists() {
                    std::fs::copy(hub.join(name), root.join(name))?;
                    copy_stat(&hub.join(name), &root.join(name), false)?;
                }
            }
            set_env(
                &root.join(".env"),
                &[
                    ("API_SERVER_ENABLED", "true".to_string()),
                    ("API_SERVER_HOST", "127.0.0.1".to_string()),
                    ("API_SERVER_PORT", port.to_string()),
                    ("API_SERVER_KEY", space_key.clone()),
                ],
            )?;
            let profile = root.join("profiles").join(&agent.name);
            copy_profile(&source, &profile)?;
            let rebound = rebind_api_sessions(&profile, &hermes_profile_id(config, &root, &agent.name))?;
            if rebound > 0 {
                tracing::info!(agent = %agent.name, conversations = rebound, "its API conversations made its space's");
            }
            let env = BTreeMap::from([("HERMES_HOME".to_string(), root.to_string_lossy().into_owned())]);
            let has_start = config.spaces.start.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
            let sandboxed = has_start || sandbox_available(config);
            let bin = config.hermes.bin.as_str();
            // Attachments only from its own files (a routed profile reads this from its own config). Hermes' own
            // browser tools: « Browser Use » puts a socket under the agent's folder, too long a path here. (0.22.5
            // still reads it, « off » = the built-in tools; 0.23.0 drops `browser.backend` with the built-in browser
            // tools, config v52: there, turn the `browser` toolset off for this profile instead.)
            crate::profile_files::set(
                &root.join("profiles").join(&agent.name).join("config.yaml"),
                &[("gateway.strict", Yaml::Bool(true)), ("browser.backend", Yaml::str("off"))],
            )?;
            if !has_start && sandbox_available(config) {
                // Terminal, files, code and the browser's desktop in a container; nothing of the host mounted.
                self.run(&[bin, "-p", &agent.name, "config", "set", "terminal.backend", "docker"], env.clone()).await?;
            }
            let root_text = root.to_string_lossy().into_owned();
            self.service(config, &agent.name, &root_text, true).await?;
            let mut moved = agent.clone();
            moved.hermes_url = format!("http://127.0.0.1:{port}/p/{}", agent.name);
            moved.public_url =
                config.public_url.as_deref().filter(|u| !u.is_empty()).map(|u| format!("{u}/hermes/{}", agent.name));
            moved.isolated = true;
            moved.sandboxed = sandboxed;
            moved.space = Some(root_text.clone());
            moved.python = config.hermes.python.clone();
            moved.hermes_root = Some(config.hermes.root.clone()).filter(|r| !r.is_empty());
            let profile = root.join("profiles").join(&agent.name);
            moved.hermes_home = Some(profile.to_string_lossy().into_owned());
            moved.upload_dir_host = Some(profile.join("uploads").to_string_lossy().into_owned());
            // Where the agent finds them: inside its container, its HERMES_HOME may sit elsewhere.
            let seen = if config.spaces.container_home.is_empty() {
                root_text.clone()
            } else {
                config.spaces.container_home.clone()
            };
            moved.upload_dir_container = Some(format!("{seen}/profiles/{}/uploads", agent.name));
            let mut started = false;
            for _ in 0..120 {
                let answer = http
                    .get(format!("{}/v1/capabilities", moved.hermes_url))
                    .bearer_auth(&agent.hermes_key)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await;
                if matches!(&answer, Ok(resp) if resp.status().as_u16() == 200) {
                    started = true;
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            if !started {
                return Err(SpaceError("the new space did not start".into()).into());
            }
            Ok((moved, sandboxed))
        }
        .await;
        let (moved, _) = match result {
            Ok(done) => done,
            Err(exc) => {
                if let Err(stop) = self.service(config, &agent.name, &root.to_string_lossy(), false).await {
                    tracing::debug!(error = %stop, "space service not stopped after a failed move");
                }
                let _ = std::fs::remove_dir_all(&root);
                return Err(exc);
            }
        };
        tracing::info!(agent = %agent.name, port = port, sandbox = sandbox_available(config), "agent moved into its own space");
        Ok(moved)
    }

    /// Served by its own gateway now: the hub forgets it.
    pub async fn forget_in_hub(&self, config: &Config, agent: &str) -> anyhow::Result<()> {
        let hub_env = if config.hermes.home.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([("HERMES_HOME".to_string(), config.hermes.home.clone())])
        };
        self.run(&[&config.hermes.bin, "profile", "delete", agent, "--yes"], hub_env).await?;
        Ok(())
    }

    /// A deleted agent's space: its gateway stopped, its folder gone.
    pub async fn remove(&self, config: &Config, agent: &AgentConfig) -> anyhow::Result<()> {
        let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) else { return Ok(()) };
        if self.service(config, &agent.name, space, false).await.is_err() {
            tracing::warn!(agent = %agent.name, "space service not stopped");
        }
        let _ = std::fs::remove_dir_all(space);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hubs_api_conversations_become_the_spaces() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(rebind_api_sessions(dir.path(), "/space/profiles/coach").unwrap(), 0); // no database
        let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
        conn.execute_batch("CREATE TABLE sessions (id TEXT)").unwrap();
        drop(conn);
        assert_eq!(rebind_api_sessions(dir.path(), "/space/profiles/coach").unwrap(), 0); // no Hermes table
        let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
        conn.execute_batch("CREATE TABLE state_meta (key TEXT PRIMARY KEY, value TEXT)").unwrap();
        let receipt = |id: &str, home: &str| {
            serde_json::json!({"profile_id": home, "session_id": id, "route": "agent:main:api:dm",
                               "entry": {"session_id": id, "origin": {"profile": "coach"}}})
            .to_string()
        };
        let put = |key: String, value: String| {
            conn.execute("INSERT INTO state_meta VALUES (?1, ?2)", rusqlite::params![key, value]).unwrap();
        };
        put(format!("{API_BINDING_PREFIX}api_1"), receipt("api_1", "/hub/profiles/coach"));
        put(format!("{API_BINDING_PREFIX}api_2"), receipt("api_2", "/space/profiles/coach"));
        put("gateway.local_policy.v1:local-1".into(), receipt("local-1", "/hub/profiles/coach"));
        put(format!("{API_BINDING_PREFIX}api_3"), "not json".into());
        drop(conn);
        assert_eq!(rebind_api_sessions(dir.path(), "/space/profiles/coach").unwrap(), 1);
        let conn = rusqlite::Connection::open(dir.path().join("state.db")).unwrap();
        let value = |key: &str| -> serde_json::Value {
            let text: String =
                conn.query_row("SELECT value FROM state_meta WHERE key = ?1", [key], |r| r.get(0)).unwrap();
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
        };
        let moved = value(&format!("{API_BINDING_PREFIX}api_1"));
        assert_eq!(moved["profile_id"], "/space/profiles/coach");
        assert_eq!(moved["entry"]["origin"]["profile"], "coach"); // the rest as it was
        assert_eq!(value("gateway.local_policy.v1:local-1")["profile_id"], "/hub/profiles/coach"); // not an API one
        assert_eq!(rebind_api_sessions(dir.path(), "/space/profiles/coach").unwrap(), 0); // once
    }

    #[test]
    fn quoting_and_commands() {
        assert_eq!(shlex_quote("/opt/hermes"), "/opt/hermes");
        assert_eq!(shlex_quote("a b"), "'a b'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote(""), "''");
        assert_eq!(
            format_keys("start {agent} {home} {{x}}", &[("agent", "vie"), ("home", "'/a b'")]),
            "start vie '/a b' {x}"
        );
        let unit = systemd_unit("vie", "/opt/hermes", "/s/vie");
        assert!(unit.starts_with("[Unit]\nDescription=BipAgents space of vie (Hermes gateway)\n\n[Service]\nExecStart=/opt/hermes gateway run\n"));
        assert!(unit.ends_with("[Install]\nWantedBy=default.target\n"));
    }
}
