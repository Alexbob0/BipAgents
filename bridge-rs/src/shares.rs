//! Read-only data between agents: « Vie peut lire ce que Wellness partage ».
//!
//! Each agent whose tools run in a sandbox (Hermes' `docker` terminal backend) can have a folder it shares,
//! `<data>/shares/<agent>/`, seen in its sandbox as `/partage` (it writes there what others may read). A link
//! reader ← source mounts the source's folder in the reader's sandbox, read-only, as `/lecture/<source>`. Nothing else
//! of either agent is visible to the other: not its memory, conversations, keys or other files.
//!
//! The mounts are Hermes' own `terminal.docker_volumes` of each profile: entries the person added by hand are kept.
//! `<data>/shares.json` holds the links. The sandboxes pick the new mounts up when they start again (they are recreated
//! when their mounts change).
//!
//! Where the bridge isolates the sandboxes (Linux with podman, sandbox.rs), nothing is mounted: the same folders are
//! reached through the agent's file tools (`shared_files_*`), which check these links at each call; writes and deletes
//! are approved by the person. Agents set up by hand (`shares = "external"`) still mount what `mounts()` lists.
//!
//! Port of `bridge/bipbridge/shares.py` (owner group: agents).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agents_admin::pyyaml;
use crate::agents_admin::Run;
use crate::config::{AgentConfig, Config};
use crate::Dict;

/// Python's `ValueError` of `apply`: an unknown agent, or one that cannot share (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct InvalidShare(pub String);

/// The skill an agent gets when it shares or reads (`skills/shared-data/SKILL.md`, embedded).
pub fn skill() -> &'static str {
    crate::assets::text(&crate::assets::SKILLS, "shared-data/SKILL.md").unwrap_or("")
}

pub fn shares_root(config: &Config) -> PathBuf {
    PathBuf::from(crate::py::join(&config.data_dir(), "shares"))
}

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(crate::py::join(&config.data_dir(), "shares.json"))
}

/// The person's read links: (reader, shared agent).
pub fn load(config: &Config) -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(store_path(config)) else { return Vec::new() };
    let Ok(data) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    let reads = data.get("reads").and_then(Value::as_array).cloned().unwrap_or_default();
    reads
        .iter()
        .filter_map(|x| match x.as_array() {
            Some(pair) if pair.len() == 2 => Some((crate::py::py_str(&pair[0]), crate::py::py_str(&pair[1]))),
            _ => None,
        })
        .collect()
}

fn save(config: &Config, reads: &[(String, String)]) -> std::io::Result<()> {
    let path = store_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let unique: BTreeSet<&(String, String)> = reads.iter().collect();
    let listed: Vec<Value> = unique.into_iter().map(|(r, s)| json!([r, s])).collect();
    std::fs::write(&path, format!("{}\n", crate::py::dumps(&json!({"reads": listed}))))
}

/// The agent's Hermes profile, when this install manages it (its hub, or its own space).
pub fn profile_dir(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    let space = agent.space.as_deref().filter(|s| !s.is_empty());
    let hub = &config.hermes.hub_url;
    if !config.hermes.enabled() || !(space.is_some() || agent.hermes_url.starts_with(&format!("{hub}/p/"))) {
        return None;
    }
    let home = match space {
        Some(s) => s.to_string(),
        None if !config.hermes.home.is_empty() => config.hermes.home.clone(),
        None => crate::py::join(&crate::py::expanduser("~"), ".hermes"),
    };
    Some(PathBuf::from(home).join("profiles").join(&agent.name))
}

/// The profile's config.yaml as a dict (empty when missing, unreadable or not a dict).
pub fn profile_config(folder: &Path) -> Dict {
    let Ok(text) = std::fs::read_to_string(folder.join("config.yaml")) else { return Dict::new() };
    match pyyaml::load(&text).map(|d| pyyaml::to_json(&d)) {
        Ok(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

/// None when the agent can share and read; otherwise why not: `not_managed` (set up by hand: its administrator
/// mounts folders), `no_sandbox` (its tools run on this machine, where a read-only mount means nothing). An agent set
/// up by hand with `shares = "external"` takes part: its administrator mounts what `mounts()` lists when it starts.
pub fn why_not(config: &Config, agent: &AgentConfig) -> Option<String> {
    if agent.shares == "external" {
        return None;
    }
    let Some(folder) = profile_dir(config, agent) else { return Some("not_managed".into()) };
    if terminal_backend(&folder).as_deref() != Some("docker") {
        return Some("no_sandbox".into());
    }
    None
}

fn env_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in crate::ssh_keys::splitlines(&text) {
        let line = line.trim();
        if let Some((name, value)) = line.split_once('=') {
            if name.trim() == key {
                let value = value.trim().trim_matches(['\'', '"']);
                return if value.is_empty() { None } else { Some(value.to_string()) };
            }
        }
    }
    None
}

/// Where the agent's tools run, as its gateway sees it: the profile's own setting (config.yaml, then its .env's
/// TERMINAL_ENV), else what it gets from the gateway's home (`hermes profile create` without --clone copies none of
/// it: such a profile runs in the hub's sandbox through the hub's environment).
pub fn terminal_backend(folder: &Path) -> Option<String> {
    let home = folder.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    for config_dir in [folder.to_path_buf(), home] {
        if let Some(Value::Object(terminal)) = profile_config(&config_dir).get("terminal") {
            if let Some(backend) = terminal.get("backend").filter(|b| crate::py::truthy(b)) {
                return Some(crate::py::py_str(backend));
            }
        }
        if let Some(env) = env_value(&config_dir.join(".env"), "TERMINAL_ENV") {
            return Some(env);
        }
    }
    None
}

/// The profile's `terminal.docker_volumes`, as strings (none when it is not a list).
pub fn docker_volumes(folder: &Path) -> Vec<String> {
    let data = profile_config(folder);
    let current = match data.get("terminal") {
        Some(Value::Object(terminal)) => terminal.get("docker_volumes").cloned(),
        _ => None,
    };
    match current {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).collect(),
        _ => Vec::new(),
    }
}

/// The agent's docker_volumes: the person's own entries, then its shared folder (when another agent reads it) and
/// what it may read.
pub fn volumes_for(config: &Config, agent: &str, reads: &[(String, String)], current: &[String]) -> Vec<String> {
    let root = shares_root(config).to_string_lossy().into_owned();
    let mut volumes: Vec<String> = current.iter().filter(|v| !v.starts_with(&format!("{root}/"))).cloned().collect();
    if reads.iter().any(|(_, source)| source == agent) {
        // `z`: a label every container may read (SELinux, as on Fedora: without it the sandbox can't open the files).
        volumes.push(format!("{root}/{agent}:/partage:z"));
    }
    let mut sorted: Vec<&(String, String)> = reads.iter().collect();
    sorted.sort();
    for (reader, source) in sorted {
        if reader == agent {
            volumes.push(format!("{root}/{source}:/lecture/{source}:ro,z"));
        }
    }
    volumes
}

/// Python's `template.format(agent=name)` for an administrator's command.
pub fn format_agent(template: &str, agent: &str) -> String {
    template
        .replace("{{", "\u{0}")
        .replace("}}", "\u{1}")
        .replace("{agent}", agent)
        .replace('\u{0}', "{")
        .replace('\u{1}', "}")
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The person's read links, set up in every agent concerned (now or before). `InvalidShare` (Python's
/// `ValueError`) for an unknown agent, or one that cannot share (see `why_not`).
pub async fn apply(config: &Config, wanted: &[(String, String)], run: &Run) -> anyhow::Result<Vec<(String, String)>> {
    for (reader, source) in wanted {
        if reader == source {
            return Err(InvalidShare("an agent always reads its own files".into()).into());
        }
        for name in [reader, source] {
            let Some(agent) = config.agent(Some(name)) else {
                return Err(InvalidShare(format!("unknown agent {name}")).into());
            };
            if let Some(reason) = why_not(config, agent) {
                return Err(InvalidShare(format!("{name}: {reason}")).into());
            }
        }
    }
    let before = load(config);
    let concerned: BTreeSet<String> =
        before.iter().chain(wanted.iter()).flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
    let sources: BTreeSet<&String> = wanted.iter().map(|(_, s)| s).collect();
    for source in sources {
        let share = shares_root(config).join(source);
        std::fs::create_dir_all(&share)?;
        // Open to the sandboxes' own user: in a rootless sandbox this account is « root », the agent another user,
        // which a 700 folder shuts out (its own agent can't write there, its readers can't read). Nothing else
        // reaches it: the person's home is closed to other accounts, and only these sandboxes mount it.
        crate::py::chmod(&share.to_string_lossy(), 0o777)?;
    }
    let mut changed_external: Vec<AgentConfig> = Vec::new();
    for name in &concerned {
        let agent = config.agent(Some(name));
        if let Some(agent) = agent.filter(|a| a.shares == "external") {
            if mounts(config, name, Some(&before)) != mounts(config, name, Some(wanted)) {
                changed_external.push(agent.clone());
            }
            continue;
        }
        let Some(agent) = agent else { continue };
        let Some(folder) = profile_dir(config, agent).filter(|f| f.is_dir()) else { continue };
        if crate::sandbox::isolates(config, agent) {
            // An isolated sandbox mounts nothing: the agent reaches the folders through its file tools, which check
            // these links at each call (sandbox.rs). What it may no longer read goes from its copies.
            let skill_dir = folder.join("skills").join("shared-data");
            if wanted.iter().any(|(a, b)| a == name || b == name) {
                std::fs::create_dir_all(&skill_dir)?;
                std::fs::write(skill_dir.join("SKILL.md"), crate::sandbox::skill())?;
            } else {
                let _ = std::fs::remove_dir_all(&skill_dir);
            }
            crate::sandbox::drop_stale_copies(config, name, wanted);
            continue;
        }
        let volumes = volumes_for(config, name, wanted, &docker_volumes(&folder));
        let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let listed = crate::py::dumps(&json!(volumes));
        run(
            args(&[&config.hermes.bin, "-p", name, "config", "set", "terminal.docker_volumes", &listed]),
            BTreeMap::from([("HERMES_HOME".to_string(), home)]),
        )
        .await?;
        let skill_dir = folder.join("skills").join("shared-data");
        if wanted.iter().any(|(a, b)| a == name || b == name) {
            std::fs::create_dir_all(&skill_dir)?;
            std::fs::write(skill_dir.join("SKILL.md"), skill())?;
        } else {
            let _ = std::fs::remove_dir_all(&skill_dir);
        }
    }
    save(config, wanted)?;
    for agent in changed_external {
        // Its administrator's command (e.g. restart its container when it is idle): what it mounts changed.
        if let Some(command) = agent.shares_apply.as_deref().filter(|c| !c.is_empty()) {
            if let Err(exc) = run(args(&["sh", "-c", &format_agent(command, &agent.name)]), BTreeMap::new()).await {
                if !crate::agents_admin::is_os_error(&exc) {
                    tracing::warn!(agent = %agent.name, error = %crate::agents_admin::tail(&exc.to_string(), 200), "shares not applied");
                } else {
                    return Err(exc);
                }
            }
        }
    }
    tracing::info!(reads = wanted.len(), "agent shares set");
    let unique: BTreeSet<(String, String)> = wanted.iter().cloned().collect();
    Ok(unique.into_iter().collect())
}

/// What an agent set up by hand mounts (`podman run -v …`): its shared folder when another agent reads it, and what
/// it may read, read-only. `bipbridge shares-mounts --agent <name>` prints them.
pub fn mounts(config: &Config, agent: &str, reads: Option<&[(String, String)]>) -> Vec<String> {
    let mut volumes = match reads {
        Some(reads) => volumes_for(config, agent, reads, &[]),
        None => volumes_for(config, agent, &load(config), &[]),
    };
    // Its outbox: what it puts there, send_files_to_agent gives to another agent (agent_files.rs).
    if config.agent(Some(agent)).is_some_and(|a| a.shares == "external") {
        volumes.push(format!(
            "{}:{}:z",
            crate::agent_files::external_outbox(config, agent).display(),
            crate::sandbox::OUTBOX
        ));
    }
    volumes
}

/// At start: each agent's `shared-data` skill, where it has one, as this bridge writes it (an agent set up by hand
/// too, when the bridge was given its HERMES_HOME): its words may have changed with an update.
pub fn heal_skills(config: &Config) {
    for name in config.agents.keys() {
        let Ok(agent) = crate::agents_admin::editable(config, name) else { continue };
        let path =
            crate::agents_admin::profile_folder(config, &agent).join("skills").join("shared-data").join("SKILL.md");
        let Ok(current) = std::fs::read_to_string(&path) else { continue };
        let wanted = if crate::sandbox::isolates(config, &agent) { crate::sandbox::skill() } else { skill() };
        if current != wanted && std::fs::write(&path, wanted).is_err() {
            tracing::warn!(agent = %agent.name, "shared-data skill not updated");
        }
    }
}

/// A deleted agent (already out of the config): the others stop reading it, and its shared folder goes.
pub async fn forget(config: &Config, agent: &str, run: &Run) -> anyhow::Result<()> {
    let reads = load(config);
    if reads.iter().any(|(a, b)| a == agent || b == agent) {
        let kept: Vec<(String, String)> = reads.into_iter().filter(|(a, b)| a != agent && b != agent).collect();
        apply(config, &kept, run).await?;
    }
    let _ = std::fs::remove_dir_all(shares_root(config).join(agent));
    Ok(())
}
