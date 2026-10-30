//! Spare agents (2026-10-30, Alex: « créer un agent devrait prendre ~2 s, comme Grok »).
//!
//! A place keeps a few blank profiles already made in its hub (`hermes profile create`, their own key, the bridge's
//! tools' entry, their browser and uploads), served by the gateway and with their tools listed: the slow part of a
//! creation (the `hermes` commands, the gateway's pick-up, a restart for the tools) is done ahead, while nobody waits.
//! Creating an agent (the app, a guide card, a team, a project agent) then takes one and writes files only: its
//! personality, its description, Bot Mode, its section in bridge.toml, its tools' entry (agents_admin.rs
//! `from_spare`). The pool is refilled in the background once the hub is calm (one batch, one restart at most).
//!
//! Spares live in `spare-agents.json` next to bridge.toml, never in `[agents]`: they are not in the apps' list, the
//! place's agents at most, the shared memory's circle, the Carnet's news, nor the backups (installer moving.py)
//! until one is taken. Their ids are `agent_<8 hex>`: `souls::profile_name` never makes an `_`, so an agent named by
//! the person never gets one.
//!
//! Hermes reads a routed profile's `.env` and config.yaml at each turn (multiplex gateway, `_profile_runtime_scope`;
//! internal/notes/instant-agents.md): the model and key a spare was cloned with are brought in line with the hub's
//! when it is taken (`follow_hub_model`), without a restart.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};

use crate::agents_admin::pyyaml::Yaml;
use crate::agents_admin::Run;
use crate::config::{AgentConfig, Config, SharedConfig};

/// Spares kept on a place of the all-included offer when bridge.toml says nothing (`[place] spare_agents`).
pub const OFFER_SPARES: usize = 3;
pub const POOL_FILE: &str = "spare-agents.json";
pub const SPARE_PREFIX: &str = "agent_";

/// How many spares the place keeps: `[place] spare_agents` when written, else `OFFER_SPARES` on a place of the
/// offer (a server the app created) and none on a self-install.
pub fn wanted(config: &Config) -> usize {
    if !config.hermes.enabled() {
        return 0;
    }
    match config.place_spare_agents {
        Some(n) => n,
        None if config.cloud => OFFER_SPARES,
        None => 0,
    }
}

/// A spare's id (`agent_` and 8 lowercase hex digits).
pub fn is_spare_id(id: &str) -> bool {
    id.strip_prefix(SPARE_PREFIX)
        .is_some_and(|rest| rest.len() == 8 && rest.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()))
}

pub fn new_id() -> String {
    format!("{SPARE_PREFIX}{}", crate::py::token_hex(4))
}

/// One spare, as kept in the pool file: what `agents_admin::hub_agent` can't make again (its key, its browser's and
/// uploads' settings).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Spare {
    pub id: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_vnc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_vnc_password_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_dir_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_dir_container: Option<String>,
    #[serde(default)]
    pub sandboxed: bool,
}

impl Spare {
    pub fn of(agent: &AgentConfig) -> Self {
        Spare {
            id: agent.name.clone(),
            key: agent.hermes_key.clone(),
            desktop_vnc: agent.desktop_vnc.clone(),
            desktop_vnc_password_file: agent.desktop_vnc_password_file.clone(),
            upload_dir_host: agent.upload_dir_host.clone(),
            upload_dir_container: agent.upload_dir_container.clone(),
            sandboxed: agent.sandboxed,
        }
    }

    /// The agent it becomes, named `display_name`.
    pub fn agent(&self, config: &Config, display_name: &str) -> AgentConfig {
        let mut agent = crate::agents_admin::hub_agent(config, &self.id, display_name, &self.key);
        if self.upload_dir_host.is_some() {
            agent.upload_dir_host = self.upload_dir_host.clone();
        }
        if self.upload_dir_container.is_some() {
            agent.upload_dir_container = self.upload_dir_container.clone();
        }
        agent.desktop_vnc = self.desktop_vnc.clone();
        agent.desktop_vnc_password_file = self.desktop_vnc_password_file.clone();
        agent.sandboxed = self.sandboxed;
        agent
    }
}

/// One reader/writer of the pool file at a time.
static FILE_LOCK: Mutex<()> = Mutex::new(());

fn pool_path(config: &Config) -> Option<PathBuf> {
    let source = config.source_path.as_deref().filter(|p| !p.is_empty())?;
    Some(Path::new(source).parent()?.join(POOL_FILE))
}

fn read_raw(config: &Config) -> Vec<Spare> {
    let Some(path) = pool_path(config) else { return Vec::new() };
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_raw(config: &Config, spares: &[Spare]) -> anyhow::Result<()> {
    let Some(path) = pool_path(config) else { return Ok(()) };
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), serde_json::to_string(spares)?.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn profile_folder(config: &Config, id: &str) -> PathBuf {
    PathBuf::from(crate::agents_admin::install_home(config)).join("profiles").join(id)
}

/// Whether a kept spare can still be taken: its profile is there and no agent has its id.
fn usable(config: &Config, spare: &Spare) -> bool {
    is_spare_id(&spare.id) && !config.agents.contains_key(&spare.id) && profile_folder(config, &spare.id).join("config.yaml").is_file()
}

/// The spares ready to be taken.
pub fn spares(config: &Config) -> Vec<Spare> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    read_raw(config).into_iter().filter(|s| usable(config, s)).collect()
}

/// A spare of the pool (the bridge's MCP endpoint knows its token: its Hermes lists the tools before it is taken).
pub fn spare(config: &Config, id: &str) -> Option<Spare> {
    if !is_spare_id(id) {
        return None;
    }
    spares(config).into_iter().find(|s| s.id == id)
}

/// The agent a spare is served as before it is taken (no name).
pub fn agent(config: &Config, id: &str) -> Option<AgentConfig> {
    spare(config, id).map(|s| s.agent(config, ""))
}

/// The first spare, out of the pool (None: none left). The file is rewritten without it (and without those that
/// can't be used any more).
pub fn take(config: &Config) -> Option<Spare> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut kept: Vec<Spare> = read_raw(config).into_iter().filter(|s| usable(config, s)).collect();
    if kept.is_empty() {
        return None;
    }
    let taken = kept.remove(0);
    if let Err(exc) = write_raw(config, &kept) {
        tracing::warn!(error = %exc, "spare agents' file not written: the spare is not taken");
        return None;
    }
    Some(taken)
}

/// Spares made ready, added to the pool.
pub fn add(config: &Config, made: &[Spare]) -> anyhow::Result<()> {
    let _lock = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut kept: Vec<Spare> = read_raw(config).into_iter().filter(|s| usable(config, s)).collect();
    for spare in made {
        if !kept.iter().any(|s| s.id == spare.id) {
            kept.push(spare.clone());
        }
    }
    write_raw(config, &kept)
}

/// The variables a model's key may be kept in (cloud_setup.rs `MODEL_KEYS`, and Hermes' other providers).
const KEY_VARIABLES: [&str; 5] =
    ["CUSTOM_MODEL_API_KEY", "OPENROUTER_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "LM_API_KEY"];

fn env_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rev().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        let name = name.trim();
        let name = name.strip_prefix("export ").map(str::trim).unwrap_or(name);
        (name == key && !line.trim_start().starts_with('#')).then(|| value.trim().to_string())
    })
}

/// The spare's model and its key as the hub's now (the claim wrote them there after the spare was cloned, or a key
/// changed since): config.yaml's `model` and the key's `.env` line. Files only; Hermes reads them at the next turn.
pub fn follow_hub_model(config: &Config, folder: &Path) -> anyhow::Result<bool> {
    let home = PathBuf::from(crate::agents_admin::install_home(config));
    let hub = crate::agents_admin::load_or_empty(&home.join("config.yaml"))?;
    let Some(model) = hub.get("model").filter(|m| m.is_map()).cloned() else { return Ok(false) };
    let path = folder.join("config.yaml");
    let mut data = crate::agents_admin::load_or_empty(&path)?;
    if !data.is_map() {
        data = Yaml::map();
    }
    let mut changed = false;
    // what the bridge sets per agent (thinking floor, routing) is set again by `agent_tools::register`
    let mut section = data.get("model").filter(|m| m.is_map()).cloned().unwrap_or_else(Yaml::map);
    for key in ["provider", "default", "base_url", "key_env", "supports_vision"] {
        let wanted = model.get(key).cloned();
        if section.get(key).cloned() != wanted {
            match wanted {
                Some(value) => section.insert(key, value),
                None => {
                    section.remove(key);
                }
            }
            changed = true;
        }
    }
    if changed {
        data.insert("model", section);
        crate::agents_admin::replace_yaml(&path, &data)?;
    }
    let mut names: Vec<String> = KEY_VARIABLES.iter().map(|s| s.to_string()).collect();
    if let Some(own) = model.get("key_env").and_then(Yaml::as_str) {
        names.push(own.to_string());
    }
    for name in names {
        let Some(value) = env_value(&home.join(".env"), &name) else { continue };
        if env_value(&folder.join(".env"), &name).as_deref() != Some(value.as_str()) {
            crate::cloud_setup::set_env(&folder.join(".env"), &name, &value)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// `hermes -p <profile> config set mcp_servers.<…> <value>` written straight into the profile's config.yaml, as
/// Hermes would (a number or a boolean as such, else a string): taking a spare runs no Python CLI (≈ 1 s each).
/// Every other command goes to `inner`.
pub fn files_run(inner: Run) -> Run {
    Arc::new(move |args: Vec<String>, env: BTreeMap<String, String>| {
        let inner = inner.clone();
        Box::pin(async move {
            match written_directly(&args, &env) {
                Some(Ok(())) => Ok(String::new()),
                Some(Err(exc)) => Err(exc),
                None => inner(args, env).await,
            }
        })
    })
}

fn written_directly(args: &[String], env: &BTreeMap<String, String>) -> Option<anyhow::Result<()>> {
    // [bin, -p, profile, config, set, key, (--)? value]
    if args.len() < 7 || args[1] != "-p" || args[3] != "config" || args[4] != "set" {
        return None;
    }
    let key = &args[5];
    let value = match (args.len(), args[6].as_str()) {
        (7, _) => &args[6],
        (8, "--") => &args[7],
        _ => return None,
    };
    if !key.starts_with("mcp_servers.") || key.split('.').any(str::is_empty) {
        return None;
    }
    let home = env.get("HERMES_HOME").filter(|h| !h.is_empty())?;
    let path = Path::new(home).join("profiles").join(&args[2]).join("config.yaml");
    if !path.is_file() {
        return None;
    }
    Some(set_dotted(&path, key, value))
}

fn set_dotted(path: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    let mut data = crate::agents_admin::load_or_empty(path)?;
    if !data.is_map() {
        data = Yaml::map();
    }
    let parts: Vec<&str> = key.split('.').collect();
    let mut node = &mut data;
    for part in &parts[..parts.len() - 1] {
        node = node.entry_map(part).ok_or_else(|| anyhow::anyhow!("{key}: {part} is not a mapping"))?;
    }
    let parsed = if let Ok(n) = value.parse::<i64>() {
        crate::agents_admin::pyyaml::from_json(&serde_json::json!(n))
    } else if value == "true" || value == "false" {
        Yaml::Bool(value == "true")
    } else {
        Yaml::str(value)
    };
    node.insert(parts[parts.len() - 1], parsed);
    crate::agents_admin::replace_yaml(path, &data)
}

/// A refill running (one at a time).
static REFILLING: AtomicBool = AtomicBool::new(false);

/// How long a refill waits for the hub to be calm before making its profiles anyway (its restart, if one is needed,
/// still waits for calm).
const CALM_WAIT: std::time::Duration = std::time::Duration::from_secs(600);
const CALM_CHECK: std::time::Duration = std::time::Duration::from_secs(3);

/// The pool refilled in the background when spares are missing, once the hub's agents are all idle.
pub fn refill_soon(shared: &SharedConfig, http: &reqwest::Client, run: &Run) {
    let config = shared.get();
    if wanted(&config) <= spares(&config).len() || REFILLING.swap(true, Ordering::SeqCst) {
        return;
    }
    let (shared, http, run) = (shared.clone(), http.clone(), run.clone());
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        while started.elapsed() < CALM_WAIT && !calm(&shared.get()).await {
            tokio::time::sleep(CALM_CHECK).await;
        }
        let made = refill(&shared, &http, &run).await;
        REFILLING.store(false, Ordering::SeqCst);
        if !made.is_empty() {
            tracing::info!(spares = %made.join(","), "spare agents ready");
        }
    });
}

/// Every agent of the hub idle (or the restart loop not running: tests, a bridge starting).
async fn calm(config: &Config) -> bool {
    let Some(first) = config.agents.values().find(|a| a.space.as_deref().is_none_or(str::is_empty)) else {
        return true;
    };
    if !crate::agent_restart::loop_running() {
        return true;
    }
    crate::agent_restart::group_calm(config, first).await.is_some()
}

/// Spares made now up to `wanted` (one batch). Returns their ids.
pub async fn refill(shared: &SharedConfig, http: &reqwest::Client, run: &Run) -> Vec<String> {
    let config = shared.get();
    let missing = wanted(&config).saturating_sub(spares(&config).len());
    if missing == 0 {
        return Vec::new();
    }
    let made = crate::agents_admin::make_spares(shared, missing, http, run).await;
    let ids: Vec<String> = made.iter().map(|s| s.id.clone()).collect();
    if let Err(exc) = add(&shared.get(), &made) {
        tracing::warn!(error = %exc, "spare agents' file not written");
        return Vec::new();
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spare_ids_never_clash_with_named_agents() {
        assert!(is_spare_id("agent_0a1b2c3d"));
        assert!(is_spare_id(&new_id()));
        for name in ["agent", "agent-2", "agent_xyz", "agent_0A1B2C3D", "agent_0a1b2c3", "lea"] {
            assert!(!is_spare_id(name), "{name}");
        }
        assert!(!is_spare_id(&crate::souls::profile_name("Agent 0a1b2c3d", &[])));
        assert!(!is_spare_id(&crate::souls::profile_name("agent_0a1b2c3d", &[])));
    }

    #[test]
    fn mcp_settings_are_written_into_the_profile_without_hermes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("profiles").join("agent_00000001");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("config.yaml"), "model:\n  default: glm\n").unwrap();
        let env: BTreeMap<String, String> =
            [("HERMES_HOME".to_string(), dir.path().to_string_lossy().into_owned())].into_iter().collect();
        let set = |key: &str, value: &str| {
            let args: Vec<String> =
                ["/opt/hermes", "-p", "agent_00000001", "config", "set", key, value].iter().map(|s| s.to_string()).collect();
            written_directly(&args, &env).unwrap().unwrap();
        };
        set("mcp_servers.bipagents.url", "http://127.0.0.1:8643/v1/agent-tools/mcp?v=2");
        set("mcp_servers.bipagents.headers.Authorization", "Bearer abc");
        set("mcp_servers.bipagents.timeout", "120");
        let text = std::fs::read_to_string(folder.join("config.yaml")).unwrap();
        let data = crate::agents_admin::pyyaml::to_json(&crate::agents_admin::pyyaml::load(&text).unwrap());
        assert_eq!(data["model"]["default"], "glm");
        assert_eq!(
            data["mcp_servers"]["bipagents"],
            serde_json::json!({"url": "http://127.0.0.1:8643/v1/agent-tools/mcp?v=2",
                               "headers": {"Authorization": "Bearer abc"}, "timeout": 120})
        );
        // anything else goes to Hermes
        let other: Vec<String> =
            ["/opt/hermes", "-p", "agent_00000001", "config", "set", "API_SERVER_KEY", "k"].iter().map(|s| s.to_string()).collect();
        assert!(written_directly(&other, &env).is_none());
    }
}
