//! The one writer of an agent's Hermes profile files (docs/agent-settings.md): `config.yaml`, `SOUL.md`, the
//! bridge's settings record, and Hermes' own commands that write the profile.
//!
//! Every write holds the profile's lock (`flock` on [`LOCK_FILE`], from the read to the rename: other processes and
//! this bridge's threads alike), changes the whole document, writes nothing when it comes out equal (the bytes on
//! disk, Hermes' formatting and comments included, stay as they are), and goes to a temporary file renamed over the
//! old one with its mode (a gateway never reads half a file).
//!
//! [`reconcile`] is the single pass that renders what the bridge decides in `config.yaml` (thinking level, router
//! order, tool diet, browser proxy path): at start for every agent, whenever an agent gets its tools, after a vault
//! change. Idempotent: a second pass writes nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agents_admin::pyyaml::{self, Yaml};
use crate::agents_admin::Run;
use crate::config::{AgentConfig, Config};

/// The lock file in each profile folder.
pub const LOCK_FILE: &str = ".bipagents-profile.lock";
/// The bridge's record of the agent's settings, in its profile folder (moves, exports and backups carry it).
pub const SETTINGS_FILE: &str = "bipagents-settings.json";
/// The record's version this bridge writes.
pub const SETTINGS_VERSION: u32 = 1;
/// How long a writer waits for another's lock (a write takes milliseconds, a Hermes command about a second).
const LOCK_WAIT: Duration = Duration::from_secs(30);

// -- the lock -------------------------------------------------------------------------------------------------------

/// A profile's lock, released when dropped (none for a folder not made yet: nothing else writes there).
pub struct ProfileLock(#[allow(dead_code)] Option<std::fs::File>);

/// The lock of the profile in `folder`, waiting up to [`LOCK_WAIT`].
pub fn lock(folder: &Path) -> std::io::Result<ProfileLock> {
    if !folder.is_dir() {
        return Ok(ProfileLock(None));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(folder.join(LOCK_FILE))?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ProfileLock(Some(file))),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{}: another writer kept the profile locked", folder.display()),
                ));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

fn folder_of(path: &Path) -> PathBuf {
    path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

// -- writes ---------------------------------------------------------------------------------------------------------

/// `bytes` in `path` through a temporary file beside it, with the old file's mode (0600 for a new one).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(folder_of(path))?;
    let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o600);
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(".bipagents-tmp");
    let temporary = path.with_file_name(name);
    std::fs::write(&temporary, bytes)?;
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))?;
    std::fs::rename(&temporary, path)
}

/// `yaml.safe_load(text) or {}` of a config file (missing: empty); Err for a file that doesn't load (never
/// overwritten: what Hermes or the person wrote stays).
pub fn load(path: &Path) -> anyhow::Result<Yaml> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Yaml::map()),
        Err(e) => return Err(e.into()),
    };
    let data = pyyaml::load(&text)?;
    Ok(if data.truthy() { data } else { Yaml::map() })
}

/// A YAML config file (`config.yaml`, `profile.yaml`) changed by `change` under the profile's lock; whether it was
/// written. A file that isn't a mapping is changed as an empty one.
pub fn edit(path: &Path, change: impl FnOnce(&mut Yaml)) -> anyhow::Result<bool> {
    try_edit(path, |data| {
        change(data);
        Ok(())
    })
}

/// [`edit`] whose change may refuse (nothing written then).
pub fn try_edit(path: &Path, change: impl FnOnce(&mut Yaml) -> anyhow::Result<()>) -> anyhow::Result<bool> {
    let _lock = lock(&folder_of(path))?;
    let before = load(path)?;
    let mut data = if before.is_map() { before.clone() } else { Yaml::map() };
    change(&mut data)?;
    if data == before {
        return Ok(false);
    }
    write_atomic(path, pyyaml::dump(&data).as_bytes())?;
    Ok(true)
}

/// A text file of the profile (`SOUL.md`…) changed by `change` (given None for a missing file; returning None leaves
/// the file as it is); whether it was written.
pub fn edit_text(path: &Path, change: impl FnOnce(Option<&str>) -> Option<String>) -> anyhow::Result<bool> {
    let _lock = lock(&folder_of(path))?;
    let before = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let Some(after) = change(before.as_deref()) else { return Ok(false) };
    if before.as_deref() == Some(after.as_str()) {
        return Ok(false);
    }
    write_atomic(path, after.as_bytes())?;
    Ok(true)
}

/// Hermes' own command on the profile in `folder` (`config set` of a key with side effects: `terminal.*` mirrored
/// in `.env`, a credential written there; `tools enable/disable`), run under the profile's lock so no edit of this
/// bridge reads the file meanwhile. A profile not made yet runs it without the lock.
pub async fn hermes(
    folder: &Path,
    run: &Run,
    args: Vec<String>,
    env: BTreeMap<String, String>,
) -> anyhow::Result<String> {
    let folder = folder.to_path_buf();
    let held = tokio::task::spawn_blocking(move || lock(&folder)).await??;
    let out = run(args, env).await;
    drop(held);
    out
}

/// The profile folder a Hermes command writes, for the commands that write one: `[bin, (-p <profile>)?, config
/// set|unset …]`, `[…, tools enable|disable …]`. Its home: `HERMES_HOME` given, else this process', else `~/.hermes`.
pub fn written_by(args: &[String], env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let (profile, rest) = match args.get(1).map(String::as_str) {
        Some("-p") => (Some(args.get(2)?), args.get(3..)?),
        _ => (None, args.get(1..)?),
    };
    let writes = matches!(
        (rest.first().map(String::as_str), rest.get(1).map(String::as_str)),
        (Some("config"), Some("set" | "unset")) | (Some("tools"), Some("enable" | "disable"))
    );
    if !writes {
        return None;
    }
    let home = env
        .get("HERMES_HOME")
        .cloned()
        .or_else(|| std::env::var("HERMES_HOME").ok())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| crate::py::join(&crate::py::expanduser("~"), ".hermes"));
    let home = PathBuf::from(home);
    Some(match profile {
        Some(name) => home.join("profiles").join(name),
        None => home,
    })
}

/// `inner` with every Hermes command that writes a profile run under that profile's lock ([`hermes`]): Hermes' own
/// `config set` (keys with side effects: `terminal.*` mirrored in `.env`, credentials) and `tools enable/disable`
/// never interleave with this bridge's edits. The bridge's [`Run`] (agents_admin::default_run).
pub fn locked(inner: Run) -> Run {
    std::sync::Arc::new(move |args: Vec<String>, env: BTreeMap<String, String>| {
        let inner = inner.clone();
        Box::pin(async move {
            match written_by(&args, &env) {
                Some(folder) => hermes(&folder, &inner, args, env).await,
                None => inner(args, env).await,
            }
        })
    })
}

/// `mcp_servers.<name>` set to `entry`'s keys (kept: the entry's other keys, e.g. `enabled`), as `hermes config set
/// mcp_servers.<name>.<key> <value>` writes each; `entry` None removes the server (`config unset`).
pub fn set_server(data: &mut Yaml, name: &str, entry: Option<&[(&str, Yaml)]>) {
    let Some(entry) = entry else {
        if let Some(servers) = data.get_mut("mcp_servers").filter(|s| s.is_map()) {
            servers.remove(name);
        }
        return;
    };
    let Some(servers) = data.entry_map("mcp_servers") else { return };
    let Some(server) = servers.entry_map(name) else { return };
    for (key, value) in entry {
        let parts: Vec<&str> = key.split('.').collect();
        let mut node = &mut *server;
        for part in &parts[..parts.len() - 1] {
            match node.entry_map(part) {
                Some(next) => node = next,
                None => return,
            }
        }
        node.insert(parts[parts.len() - 1], value.clone());
    }
}

// -- the settings record --------------------------------------------------------------------------------------------

/// The bridge's record of one agent's settings that the profile alone used to hold (docs/agent-settings.md).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSettings {
    #[serde(default)]
    pub version: u32,
    /// The thinking level the person chose (none … xhigh), None when they never did: Hermes' template level isn't
    /// theirs (GLM gets its « low » then).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Keys of a later bridge, kept as they are.
    #[serde(flatten)]
    pub other: serde_json::Map<String, Value>,
}

/// Before the record (version 0): the level the person chose, one word, and « none » under GLM's floor.
const LEGACY_THINKING_CHOSEN: &str = ".bipagents-thinking-chosen";
const LEGACY_THINKING_OFF: &str = ".bipagents-thinking-off";

fn valid_level(level: &str) -> Option<String> {
    let level = level.trim().to_lowercase();
    crate::agents_admin::THINKING_LEVELS.contains(&level.as_str()).then_some(level)
}

/// The record of the profile in `folder`; one never written comes from the mark files earlier bridges left.
pub fn settings(folder: &Path) -> AgentSettings {
    if let Ok(text) = std::fs::read_to_string(folder.join(SETTINGS_FILE)) {
        if let Ok(mut record) = serde_json::from_str::<AgentSettings>(&text) {
            record.thinking = record.thinking.as_deref().and_then(valid_level);
            return record;
        }
    }
    let chosen = std::fs::read_to_string(folder.join(LEGACY_THINKING_CHOSEN)).ok().and_then(|t| valid_level(&t));
    let off = folder.join(LEGACY_THINKING_OFF).exists().then(|| "none".to_string());
    AgentSettings { version: 0, thinking: chosen.or(off), other: Default::default() }
}

fn write_settings(folder: &Path, record: &AgentSettings) -> anyhow::Result<()> {
    let mut text = serde_json::to_string_pretty(record)?;
    text.push('\n');
    write_atomic(&folder.join(SETTINGS_FILE), text.as_bytes())?;
    for mark in [LEGACY_THINKING_CHOSEN, LEGACY_THINKING_OFF] {
        let _ = std::fs::remove_file(folder.join(mark));
    }
    Ok(())
}

/// The record changed by `change` under the profile's lock; whether it was written. A record of a later version is
/// left alone (Err).
pub fn update_settings(folder: &Path, change: impl FnOnce(&mut AgentSettings)) -> anyhow::Result<bool> {
    let _lock = lock(folder)?;
    let before = settings(folder);
    if before.version > SETTINGS_VERSION {
        anyhow::bail!("{}: settings record of a later bridge (version {})", folder.display(), before.version);
    }
    let mut record = before.clone();
    change(&mut record);
    record.version = SETTINGS_VERSION;
    if record == before {
        return Ok(false);
    }
    write_settings(folder, &record)?;
    Ok(true)
}

/// The mark files of earlier bridges moved into the record (once); whether it was written.
pub fn migrate(folder: &Path) -> anyhow::Result<bool> {
    if folder.join(SETTINGS_FILE).exists() {
        return Ok(false);
    }
    let marks = [LEGACY_THINKING_CHOSEN, LEGACY_THINKING_OFF].iter().any(|m| folder.join(m).exists());
    if !marks {
        return Ok(false);
    }
    update_settings(folder, |_| {})
}

// -- reconcile ------------------------------------------------------------------------------------------------------

/// What the bridge decides in the agent's `config.yaml`, written in one edit: the thinking level (the person's,
/// else GLM's floor) with the router's order, the tool diet, the browser proxy's program. Whether it changed
/// (Hermes reads the file again from the next conversation). An agent whose profile isn't on this machine: false.
pub fn reconcile(config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
    // a profile this bridge serves, or the one of an agent set up by hand (its thinking level is the bridge's)
    let folder = crate::shares::profile_dir(config, agent)
        .unwrap_or_else(|| crate::agents_admin::profile_folder(config, agent));
    let path = folder.join("config.yaml");
    if !path.is_file() {
        return Ok(false);
    }
    migrate(&folder)?;
    let chosen = settings(&folder).thinking;
    let thinking = crate::agents_admin::editable(config, &agent.name).is_ok();
    let diet = crate::tool_diet::current(config, agent); // reads SOUL.md and state.db: before the lock
    let proxy = crate::browsers::proxy_binary();
    let proxy = Path::new(&proxy).exists().then_some(proxy);
    let order = &config.model_provider_order;
    let mut floored = false;
    let changed = edit(&path, |data| {
        if thinking {
            floored = crate::agents_admin::heal_thinking_in(data, chosen.as_deref(), order);
        }
        if let Some(routed) = crate::model_route::with_routing(data, order, chosen.as_deref()) {
            *data = routed;
        }
        if let Some(diet) = &diet {
            if let Some(dieted) = crate::tool_diet::with_diet(data, diet) {
                *data = dieted;
            }
        }
        if let Some(proxy) = &proxy {
            crate::browsers::with_proxy(data, proxy);
        }
    })?;
    if floored {
        crate::agents_admin::record_floored_none(&folder)?; // the person's « none » of an earlier bridge, kept
    }
    if changed {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, "profile settings brought up to date");
    }
    Ok(changed)
}

/// [`reconcile`] for every agent (at start, after a vault change).
pub fn reconcile_all(config: &Config) {
    for agent in config.agents.values() {
        if let Err(exc) = reconcile(config, agent) {
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %format!("{exc:#}"), "profile settings not brought up to date");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("profile-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_edit_that_changes_nothing_writes_nothing() {
        let dir = folder();
        let path = dir.join("config.yaml");
        // Hermes' own formatting and comments: kept byte for byte when nothing changes
        let text = "# Hermes\nmodel:   {default: glm}\ntools:\n  x: 1\n";
        std::fs::write(&path, text).unwrap();
        assert!(!edit(&path, |d| d.insert("tools", d.get("tools").cloned().unwrap())).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        assert!(edit(&path, |d| d.insert("timezone", Yaml::str("Europe/Paris"))).unwrap());
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!edit(&path, |d| d.insert("timezone", Yaml::str("Europe/Paris"))).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[test]
    fn a_file_that_does_not_load_is_never_overwritten() {
        let dir = folder();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "a: [unclosed\n").unwrap();
        assert!(edit(&path, |d| d.insert("b", Yaml::Int(1))).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a: [unclosed\n");
    }

    #[test]
    fn the_mode_is_kept_and_no_temporary_file_stays() {
        use std::os::unix::fs::PermissionsExt;
        let dir = folder();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "a: 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(edit(&path, |d| d.insert("a", Yaml::Int(2))).unwrap());
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        let names: Vec<String> =
            std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".bipagents-tmp")), "{names:?}");
    }

    #[test]
    fn writers_at_once_lose_nothing() {
        let dir = folder();
        let path = dir.join("config.yaml");
        std::fs::write(&path, "{}\n").unwrap();
        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for j in 0..10 {
                        edit(&path, |d| d.insert(&format!("k{i}_{j}"), Yaml::Int(j))).unwrap();
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        let data = load(&path).unwrap();
        for i in 0..8 {
            for j in 0..10 {
                assert_eq!(data.get(&format!("k{i}_{j}")), Some(&Yaml::Int(j as i128)));
            }
        }
    }

    #[test]
    fn text_files_too() {
        let dir = folder();
        let path = dir.join("SOUL.md");
        assert!(!edit_text(&path, |t| t.map(str::to_string)).unwrap());
        assert!(!path.exists());
        assert!(edit_text(&path, |t| Some(format!("{}Bonjour\n", t.unwrap_or("")))).unwrap());
        assert!(!edit_text(&path, |t| t.map(str::to_string)).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Bonjour\n");
    }

    #[test]
    fn servers_set_and_removed_as_config_set_and_unset_do() {
        let mut data = pyyaml::load("mcp_servers:\n  stealth:\n    enabled: false\n").unwrap();
        let entry = [("url", Yaml::str("http://u")), ("headers.Authorization", Yaml::str("Bearer t")), ("timeout", Yaml::Int(60))];
        set_server(&mut data, "stealth", Some(&entry));
        assert_eq!(
            pyyaml::dump(&data),
            "mcp_servers:\n  stealth:\n    enabled: false\n    url: http://u\n    headers:\n      Authorization: Bearer t\n    timeout: 60\n"
        );
        set_server(&mut data, "stealth", None);
        set_server(&mut data, "absent", None);
        assert_eq!(pyyaml::dump(&data), "mcp_servers: {}\n");
    }

    #[test]
    fn hermes_commands_that_write_a_profile_are_known() {
        let args = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        let env: BTreeMap<String, String> = [("HERMES_HOME".to_string(), "/h".to_string())].into_iter().collect();
        let profile = Some(PathBuf::from("/h/profiles/maison"));
        assert_eq!(written_by(&args("hermes -p maison config set terminal.docker_volumes []"), &env), profile);
        assert_eq!(written_by(&args("hermes -p maison config unset mcp_servers.notion"), &env), profile);
        assert_eq!(written_by(&args("hermes -p maison tools disable terminal --platform api_server"), &env), profile);
        assert_eq!(written_by(&args("hermes config set bot_peers.bip.url u"), &env), Some(PathBuf::from("/h")));
        assert_eq!(written_by(&args("hermes -p maison gateway restart"), &env), None);
        assert_eq!(written_by(&args("hermes profile create maison"), &env), None);
        assert_eq!(written_by(&args("hermes -p"), &env), None);
    }

    #[tokio::test]
    async fn a_hermes_command_waits_for_an_edit() {
        let dir = folder();
        let held = lock(&dir).unwrap();
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = ran.clone();
        let run: Run = std::sync::Arc::new(move |_, _| {
            seen.store(true, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(String::new()) })
        });
        let task = tokio::spawn({
            let dir = dir.clone();
            async move { hermes(&dir, &run, vec![], BTreeMap::new()).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        drop(held);
        task.await.unwrap().unwrap();
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn the_marks_of_earlier_bridges_become_the_record() {
        let dir = folder();
        assert_eq!(settings(&dir), AgentSettings::default());
        assert!(!migrate(&dir).unwrap());
        std::fs::write(dir.join(LEGACY_THINKING_OFF), "").unwrap();
        assert_eq!(settings(&dir).thinking.as_deref(), Some("none"));
        std::fs::write(dir.join(LEGACY_THINKING_CHOSEN), "High\n").unwrap();
        assert_eq!(settings(&dir).thinking.as_deref(), Some("high")); // the chosen level wins over the old mark
        assert!(migrate(&dir).unwrap());
        assert!(!dir.join(LEGACY_THINKING_CHOSEN).exists() && !dir.join(LEGACY_THINKING_OFF).exists());
        assert_eq!(settings(&dir), AgentSettings { version: 1, thinking: Some("high".into()), other: Default::default() });
        assert!(!migrate(&dir).unwrap());
    }

    #[test]
    fn every_setting_round_trips_and_a_later_record_is_kept() {
        let dir = folder();
        for level in crate::agents_admin::THINKING_LEVELS {
            update_settings(&dir, |s| s.thinking = Some(level.to_string())).unwrap();
            assert_eq!(settings(&dir).thinking.as_deref(), Some(level));
            assert!(!update_settings(&dir, |s| s.thinking = Some(level.to_string())).unwrap());
        }
        update_settings(&dir, |s| s.thinking = None).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(SETTINGS_FILE)).unwrap(), "{\n  \"version\": 1\n}\n");
        // a later bridge's keys stay, and its record is never rewritten by this one
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"version": 1, "thinking": "low", "skills_off": ["x"]}"#).unwrap();
        update_settings(&dir, |s| s.thinking = Some("high".into())).unwrap();
        assert_eq!(settings(&dir).other.get("skills_off"), Some(&serde_json::json!(["x"])));
        std::fs::write(dir.join(SETTINGS_FILE), r#"{"version": 9, "thinking": "low"}"#).unwrap();
        assert!(update_settings(&dir, |s| s.thinking = None).is_err());
        assert_eq!(settings(&dir).thinking.as_deref(), Some("low"));
    }
}
