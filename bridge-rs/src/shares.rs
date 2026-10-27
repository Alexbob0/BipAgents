//! Read-only data between agents: « Vie peut lire ce que Wellness partage ».
//!
//! Each agent whose tools run in a sandbox (Hermes' `docker` terminal backend) can have a folder it shares, seen in
//! its sandbox as `/partage` (it writes there what others may read). A link reader ← source mounts the source's folder
//! in the reader's sandbox, read-only, as `/lecture/<source>`. Nothing else of either agent is visible to the other:
//! not its memory, conversations, keys or other files.
//!
//! The mounts are Hermes' own `terminal.docker_volumes` of each profile: entries the person added by hand are kept.
//! `<data>/shares.json` holds the links. The sandboxes pick the new mounts up when they start again (they are recreated
//! when their mounts change).
//!
//! Where the folders live:
//!
//! - **an install that keeps them in volumes** (Linux with podman, `[sandbox] isolate`, sandbox.rs): a podman volume
//!   per source, `bipagents-share-<agent>`. A named volume, so Hermes still counts an isolated sandbox as isolated
//!   (sandbox.rs header); mounted read-write at `/partage` in the source's sandbox and read-only at `/lecture/<agent>`
//!   in its readers'. A routine writes there at night with nobody to approve anything (owner's report 2026-10-23:
//!   the isolation of 2026-10-22 mounted no shared folder at all, so a nightly export stopped and the morning routine
//!   reading it found nothing). `<data>/shares/<agent>/` stays, kept in step with the volume (`mirror_all`: every
//!   minute, and after each write of the file tools): backups and `bipagents export` take it, agents set up by hand
//!   mount it, and it is what the sandboxes mount again when isolation is turned off (`revert_volumes`). The first
//!   time (an update from the host folders, or a fresh volume after a move or a restore: `settle`), that folder's
//!   content is copied into the volume and the folder itself is kept as a backup, renamed `<agent>.before-volume`
//!   (its files may belong to the sandbox's users, which this account can't rewrite); the log says so.
//! - **elsewhere** (macOS, docker, `isolate = "off"`, no podman): `<data>/shares/<agent>/`, mounted as it is.
//! - **an agent set up by hand** (`shares = "external"`, its own container): its administrator mounts what `mounts()`
//!   lists (`bipbridge shares-mounts`), host folders: what it reads is the folder kept in step with the source's
//!   volume; what it shares is its folder, copied into its volume for the readers the bridge sandboxes.
//!
//! The `shared_files_*` tools (sandbox.rs) and the files an agent hands out (sent_files.rs, agent_files.rs) reach the
//! same folders through `share_dir`.
//!
//! Port of `bridge/bipbridge/shares.py` (owner group: agents).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};

use crate::agents_admin::pyyaml;
use crate::agents_admin::Run;
use crate::beneath;
use crate::config::{AgentConfig, Config, SharedConfig};
use crate::Dict;

/// Python's `ValueError` of `apply`: an unknown agent, or one that cannot share (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct InvalidShare(pub String);

/// Shared folders are open to the sandboxes' own users: in a rootless sandbox this account is « root », the agent
/// another user, which a 700 folder or a 644 file shuts out (its own agent couldn't change what the bridge copied).
/// Nothing else reaches them: the person's home is closed to other accounts, and only these sandboxes mount them.
const DIR_MODE: u32 = 0o777;
const FILE_MODE: u32 = 0o666;
/// How often the volumes and their folders are put in step.
const MIRROR_EVERY: Duration = Duration::from_secs(60);
/// The suffix of the folder kept as a backup when its content moved into a volume.
pub const BACKUP_SUFFIX: &str = ".before-volume";

/// The skill an agent gets when it shares or reads (`skills/shared-data/SKILL.md`, embedded).
pub fn skill() -> &'static str {
    crate::assets::text(&crate::assets::SKILLS, "shared-data/SKILL.md").unwrap_or("")
}

pub fn shares_root(config: &Config) -> PathBuf {
    PathBuf::from(crate::py::join(&config.data_dir(), "shares"))
}

/// The podman volume holding what `agent` shares.
pub fn volume_name(agent: &str) -> String {
    format!("bipagents-share-{agent}")
}

/// Whether this install keeps the shared folders in podman volumes (it isolates its sandboxes: sandbox.rs `engine`).
pub fn in_volumes(config: &Config) -> bool {
    crate::sandbox::engine(config).is_some()
}

fn external(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some_and(|a| a.shares == "external")
}

/// The volumes' folders on this machine, by source, as `settle` last found them (`<data>/sandbox.json`, `shares`).
pub fn volume_folders(config: &Config) -> BTreeMap<String, PathBuf> {
    let state = crate::sandbox::load_state(config);
    let Some(Value::Object(shares)) = state.get("shares") else { return BTreeMap::new() };
    shares
        .iter()
        .filter_map(|(name, entry)| {
            let folder = entry.get("folder")?.as_str().filter(|f| f.starts_with('/'))?;
            Some((name.clone(), PathBuf::from(folder)))
        })
        .collect()
}

/// Where the bridge reads and writes what `agent` shares: its volume's folder once settled (an agent the bridge sets
/// up, on an install that keeps them in volumes), else `<data>/shares/<agent>`.
pub fn share_dir(config: &Config, agent: &str) -> PathBuf {
    if !external(config, agent) && in_volumes(config) {
        if let Some(folder) = volume_folders(config).remove(agent).filter(|f| f.is_dir()) {
            return folder;
        }
    }
    shares_root(config).join(agent)
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

/// A `docker_volumes` entry of a shared folder, as this bridge sets them (today's volumes or the host folders).
pub fn share_entry(config: &Config, entry: &str) -> bool {
    let root = shares_root(config).to_string_lossy().into_owned();
    let target = entry.split(':').nth(1).unwrap_or("");
    entry.starts_with(&format!("{root}/"))
        || entry.starts_with("bipagents-share-")
        || target == "/partage"
        || target.starts_with("/lecture/")
}

/// The shared folders' mounts of `agent`: its own at /partage when another agent reads it, and what it may read at
/// /lecture/<source>, read-only. `named`: the podman volumes, else the folders of this machine.
pub fn share_mounts(config: &Config, agent: &str, reads: &[(String, String)], named: bool) -> Vec<String> {
    let root = shares_root(config).to_string_lossy().into_owned();
    let mut volumes = Vec::new();
    if reads.iter().any(|(_, source)| source == agent) {
        // `z`: a label every container may read (SELinux, as on Fedora: without it the sandbox can't open the files);
        // a named volume is labelled by podman itself.
        volumes.push(if named {
            format!("{}:/partage", volume_name(agent))
        } else {
            format!("{root}/{agent}:/partage:z")
        });
    }
    let mut sorted: Vec<&(String, String)> = reads.iter().collect();
    sorted.sort();
    sorted.dedup();
    for (reader, source) in sorted {
        if reader == agent {
            volumes.push(if named {
                format!("{}:/lecture/{source}:ro", volume_name(source))
            } else {
                format!("{root}/{source}:/lecture/{source}:ro,z")
            });
        }
    }
    volumes
}

/// The agent's docker_volumes (an agent the bridge sets up, whose sandbox it doesn't isolate): the person's own
/// entries, then its shared folder (when another agent reads it) and what it may read.
pub fn volumes_for(config: &Config, agent: &str, reads: &[(String, String)], current: &[String]) -> Vec<String> {
    let mut volumes: Vec<String> = current.iter().filter(|v| !share_entry(config, v)).cloned().collect();
    volumes.extend(share_mounts(config, agent, reads, in_volumes(config) && !external(config, agent)));
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

/// `terminal.docker_volumes` of the profile set to `volumes`.
async fn set_volumes(config: &Config, name: &str, folder: &Path, volumes: &[String], run: &Run) -> anyhow::Result<()> {
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let listed = crate::py::dumps(&json!(volumes));
    run(
        args(&[&config.hermes.bin, "-p", name, "config", "set", "terminal.docker_volumes", &listed]),
        BTreeMap::from([("HERMES_HOME".to_string(), home)]),
    )
    .await?;
    Ok(())
}

fn write_skill(folder: &Path, linked: bool, text: &str) -> std::io::Result<()> {
    let skill_dir = folder.join("skills").join("shared-data");
    if linked {
        std::fs::create_dir_all(&skill_dir)?;
        std::fs::write(skill_dir.join("SKILL.md"), text)?;
    } else {
        let _ = std::fs::remove_dir_all(&skill_dir);
    }
    Ok(())
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
        crate::py::chmod(&share.to_string_lossy(), DIR_MODE)?;
    }
    // The volumes first: the sandboxes mount them by name.
    ensure_volumes(config, run, wanted).await;
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
        let linked = wanted.iter().any(|(a, b)| a == name || b == name);
        if crate::sandbox::isolates(config, agent) {
            // Its volumes, beside the ones of its isolated sandbox; the file tools' skill. What it may no longer read
            // goes from the copies the tools made.
            let current = docker_volumes(&folder);
            let volumes = crate::sandbox::wanted_volumes(config, name, &current, wanted);
            if volumes != current {
                set_volumes(config, name, &folder, &volumes, run).await?;
            }
            write_skill(&folder, linked, crate::sandbox::skill())?;
            crate::sandbox::drop_stale_copies(config, name, wanted);
            continue;
        }
        let volumes = volumes_for(config, name, wanted, &docker_volumes(&folder));
        set_volumes(config, name, &folder, &volumes, run).await?;
        write_skill(&folder, linked, skill())?;
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

/// At start: the shared folders' mounts of the agents the bridge sets up without isolating them (an install that
/// stepped back from isolation, a sandbox holding its desktop), where they differ from what the links want.
pub async fn heal_mounts(config: &Config, run: &Run) {
    let reads = load(config);
    let concerned: BTreeSet<String> = reads.iter().flat_map(|(a, b)| [a.clone(), b.clone()]).collect();
    for name in concerned {
        let Some(agent) = config.agent(Some(&name)) else { continue };
        if agent.shares == "external" || crate::sandbox::isolates(config, agent) {
            continue;
        }
        let Some(folder) = profile_dir(config, agent).filter(|f| f.is_dir()) else { continue };
        if terminal_backend(&folder).as_deref() != Some("docker") {
            continue;
        }
        let current = docker_volumes(&folder);
        let volumes = volumes_for(config, &name, &reads, &current);
        if volumes != current {
            match set_volumes(config, &name, &folder, &volumes, run).await {
                Ok(()) => {
                    tracing::info!(target: "bipbridge::sandbox", agent = %name, mounts = %volumes.join(" "), "shared folders mounted again")
                }
                Err(exc) => {
                    tracing::warn!(target: "bipbridge::sandbox", agent = %name, error = %crate::agents_admin::tail(&exc.to_string(), 200), "shared folders not mounted")
                }
            }
        }
        if let Err(exc) = write_skill(&folder, true, skill()) {
            tracing::warn!(agent = %name, error = %exc, "shared-data skill not written");
        }
    }
}

/// What an agent set up by hand mounts (`podman run -v …`): its shared folder when another agent reads it, and what
/// it may read, read-only (folders of this machine: the ones kept in step with the volumes, where there are);
/// `bipbridge shares-mounts --agent <name>` prints them.
pub fn mounts(config: &Config, agent: &str, reads: Option<&[(String, String)]>) -> Vec<String> {
    let mut volumes = match reads {
        Some(reads) => share_mounts(config, agent, reads, false),
        None => share_mounts(config, agent, &load(config), false),
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

/// A deleted agent (already out of the config): the others stop reading it, and its shared folder goes (its volume,
/// the folder kept in step with it, the backup of before the volume).
pub async fn forget(config: &Config, agent: &str, run: &Run) -> anyhow::Result<()> {
    let reads = load(config);
    if reads.iter().any(|(a, b)| a == agent || b == agent) {
        let kept: Vec<(String, String)> = reads.into_iter().filter(|(a, b)| a != agent && b != agent).collect();
        apply(config, &kept, run).await?;
    }
    if volume_folders(config).contains_key(agent) {
        if let Some(podman) = crate::sandbox::engine(config) {
            if let Err(exc) = run(args(&[&podman, "volume", "rm", "-f", &volume_name(agent)]), BTreeMap::new()).await {
                tracing::warn!(target: "bipbridge::sandbox", agent, error = %crate::agents_admin::tail(&exc.to_string(), 200), "shared folder's volume not removed");
            }
        }
        forget_volume(config, agent);
    }
    let root = shares_root(config);
    let _ = std::fs::remove_dir_all(root.join(agent));
    if let Ok(entries) = std::fs::read_dir(&root) {
        let backup = format!("{agent}{BACKUP_SUFFIX}");
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(&backup) {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    Ok(())
}

// --- the volumes ---------------------------------------------------------------------------------------------------

fn remember_volume(config: &Config, agent: &str, folder: &Path) -> std::io::Result<()> {
    crate::sandbox::save_state(config, |state| {
        let shares = state.entry("shares").or_insert_with(|| json!({}));
        if !shares.is_object() {
            *shares = json!({});
        }
        shares[agent] = json!({"folder": folder.to_string_lossy()});
    })
}

fn forget_volume(config: &Config, agent: &str) {
    let _ = crate::sandbox::save_state(config, |state| {
        if let Some(Value::Object(shares)) = state.get_mut("shares") {
            shares.remove(agent);
        }
    });
}

/// The volume of each source of `reads`, made and settled (`settle`). Returns what was done, for the command line.
pub async fn ensure_volumes(config: &Config, run: &Run, reads: &[(String, String)]) -> Vec<String> {
    let Some(podman) = crate::sandbox::engine(config) else { return Vec::new() };
    let sources: BTreeSet<String> =
        reads.iter().map(|(_, s)| s.clone()).filter(|s| config.agent(Some(s)).is_some()).collect();
    let mut said = Vec::new();
    for source in sources {
        let volume = volume_name(&source);
        let folder = match crate::sandbox::mountpoint(run, &podman, &volume).await {
            Ok(folder) => folder,
            Err(exc) => {
                let why = crate::agents_admin::tail(&exc.to_string(), 200);
                tracing::warn!(target: "bipbridge::sandbox", agent = %source, volume = %volume, error = %why, "shared folder's volume not made");
                said.push(format!("{source}'s shared folder: no volume ({why})"));
                continue;
            }
        };
        let (c, s, p) = (config.clone(), source.clone(), podman.clone());
        match tokio::task::spawn_blocking(move || settle(&c, &s, &folder, Some(&p))).await {
            Ok(Ok(line)) => said.push(line),
            Ok(Err(exc)) => {
                tracing::warn!(target: "bipbridge::sandbox", agent = %source, volume = %volume, error = %exc, "shared folder's volume not filled");
                said.push(format!("{source}'s shared folder: {volume} not filled ({exc})"));
            }
            Err(exc) => said.push(format!("{source}'s shared folder: {exc}")),
        }
    }
    said
}

fn is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut d| d.next().is_none()).unwrap_or(true)
}

/// `<root>/<agent>.before-volume`, or with the time after it when one is already there.
fn backup_path(root: &Path, agent: &str) -> PathBuf {
    let plain = root.join(format!("{agent}{BACKUP_SUFFIX}"));
    if plain.symlink_metadata().is_err() {
        return plain;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    root.join(format!("{agent}{BACKUP_SUFFIX}-{now}"))
}

/// The source's volume (at `folder`) put in step with its folder of this machine, and remembered. Which one holds
/// the truth: the folder for an agent set up by hand (it writes there); the volume once remembered at this place,
/// unless it is empty while the folder is not (a volume made again: a move, a restore, `podman volume rm`); else the
/// folder, the first time: its content is copied into the volume and the folder kept as a backup.
pub fn settle(config: &Config, source: &str, folder: &Path, podman: Option<&str>) -> std::io::Result<String> {
    let root = shares_root(config);
    let host = root.join(source);
    let volume = volume_name(source);
    // Best effort: podman gives a volume to the sandbox's user the first time a sandbox mounts it.
    let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(DIR_MODE));
    let known = volume_folders(config).remove(source);
    let said = if external(config, source) {
        let (copied, removed) = mirror(&host, folder, podman)?;
        format!("{source}'s shared folder: {} copied into {volume} ({copied} new, {removed} gone)", host.display())
    } else if known.as_deref() == Some(folder) && !(is_empty(folder) && !is_empty(&host)) {
        let (copied, removed) = mirror(folder, &host, podman)?;
        format!("{source}'s shared folder: in {volume} ({copied} new, {removed} gone in {})", host.display())
    } else if known.is_some() && !is_empty(&host) {
        // The folder is the one this bridge kept in step: the volume was made again.
        let (copied, _) = mirror(&host, folder, podman)?;
        tracing::info!(target: "bipbridge::sandbox", agent = %source, volume = %volume, files = copied, "shared folder's volume made again: filled from the folder kept in step with it");
        format!("{source}'s shared folder: {volume} made again, {copied} file(s) copied back into it")
    } else if !is_empty(&host) {
        let (copied, _) = mirror(&host, folder, podman)?;
        let backup = backup_path(&root, source);
        let kept = match std::fs::rename(&host, &backup) {
            Ok(()) => backup,
            Err(exc) => {
                tracing::warn!(target: "bipbridge::sandbox", agent = %source, error = %exc, "the shared folder could not be renamed: it stays where it is");
                host.clone()
            }
        };
        mirror(folder, &host, podman)?;
        tracing::info!(
            target: "bipbridge::sandbox",
            agent = %source,
            volume = %volume,
            files = copied,
            backup = %kept.display(),
            "shared folder moved into its podman volume, mounted at /partage and /lecture/<agent> again; the old folder is kept as a backup"
        );
        format!(
            "{source}'s shared folder: {copied} file(s) copied into {volume}; the old folder kept as a backup at {}",
            kept.display()
        )
    } else {
        mirror(folder, &host, podman)?;
        format!("{source}'s shared folder: in {volume}")
    };
    remember_volume(config, source, folder)?;
    Ok(said)
}

/// Every volume and its folder put in step (blocking): the folder follows the volume, or the volume the folder of an
/// agent set up by hand. Where the install no longer keeps them in volumes, nothing.
pub fn mirror_all(config: &Config) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    if !in_volumes(config) {
        return;
    }
    let podman = crate::sandbox::engine(config);
    for (source, folder) in volume_folders(config) {
        if config.agent(Some(&source)).is_none() || !folder.is_dir() {
            continue;
        }
        let host = shares_root(config).join(&source);
        let result = if external(config, &source) {
            mirror(&host, &folder, podman.as_deref())
        } else {
            mirror(&folder, &host, podman.as_deref())
        };
        let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
        let warned = warned.get_or_insert_with(HashSet::new);
        match result {
            Ok(_) => {
                warned.remove(&source);
            }
            Err(exc) => {
                if warned.insert(source.clone()) {
                    tracing::warn!(target: "bipbridge::sandbox", agent = %source, error = %exc, "shared folder and its volume not in step (backups miss the latest changes)");
                }
            }
        }
    }
}

/// One source's volume and folder put in step now (after a write of the file tools).
pub fn mirror_one(config: &Config, source: &str) {
    if !in_volumes(config) {
        return;
    }
    let Some(folder) = volume_folders(config).remove(source).filter(|f| f.is_dir()) else { return };
    let host = shares_root(config).join(source);
    let podman = crate::sandbox::engine(config);
    let result = if external(config, source) {
        mirror(&host, &folder, podman.as_deref())
    } else {
        mirror(&folder, &host, podman.as_deref())
    };
    if let Err(exc) = result {
        tracing::warn!(target: "bipbridge::sandbox", agent = %source, error = %exc, "shared folder and its volume not in step");
    }
}

/// Every minute, the volumes and their folders put in step (`mirror_all`).
pub async fn mirror_forever(config: SharedConfig) {
    loop {
        tokio::time::sleep(MIRROR_EVERY).await;
        let current = config.get();
        let _ = tokio::task::spawn_blocking(move || mirror_all(&current)).await;
    }
}

/// Isolation turned off (`[sandbox] isolate = "off"`, or no podman any more): each volume's content back in its
/// folder of this machine (when its folder is still there), which the sandboxes mount again (`heal_mounts`); the
/// volumes stay. Blocking.
pub fn revert_volumes(config: &Config) {
    if in_volumes(config) {
        return;
    }
    let folders = volume_folders(config);
    for (source, folder) in folders {
        if folder.is_dir() && !external(config, &source) {
            let host = shares_root(config).join(&source);
            let podman = crate::browsers::pyx::which("podman");
            match mirror(&folder, &host, podman.as_deref()) {
                Ok(_) => {
                    tracing::info!(target: "bipbridge::sandbox", agent = %source, folder = %host.display(), "sandboxes not isolated any more: the shared folder is back on this machine's folder (its volume kept)")
                }
                Err(exc) => {
                    tracing::warn!(target: "bipbridge::sandbox", agent = %source, error = %exc, "the shared folder's volume could not be copied back")
                }
            }
        }
        forget_volume(config, &source);
    }
}

// --- copying a folder ----------------------------------------------------------------------------------------------

fn temporary(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    name.starts_with(".bip-") && name.ends_with(".part")
}

static MIRRORING: Mutex<()> = Mutex::new(());

/// `dst` made the same as `src` (`sync_tree`); when this account may not change some of it (files a sandbox's user
/// made), the same done as the sandboxes' « root » (`podman unshare`, by this binary: beneath.rs `as_root`).
pub fn mirror(src: &Path, dst: &Path, podman: Option<&str>) -> std::io::Result<(usize, usize)> {
    let _held = MIRRORING.lock().unwrap_or_else(|e| e.into_inner());
    match (sync_tree(src, dst), podman) {
        (Err(exc), Some(podman)) if exc.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::debug!(target: "bipbridge::sandbox", from = %src.display(), to = %dst.display(), "shared folder copied as the sandboxes' root (files of a sandbox's user)");
            mirror_as_root(podman, src, dst)
        }
        (other, _) => other,
    }
}

/// `sync_tree` run as the sandboxes' « root »: the same walk through folder descriptors, never a shell on paths (a
/// `cp -R`/`chmod -R` there would follow a folder the sandbox swaps for a link meanwhile).
fn mirror_as_root(podman: &str, src: &Path, dst: &Path) -> std::io::Result<(usize, usize)> {
    let out = beneath::as_root(podman, &[OsStr::new("mirror"), src.as_os_str(), dst.as_os_str()])?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        let mut counts = text.split_whitespace().map(|n| n.parse::<usize>().unwrap_or(0));
        return Ok((counts.next().unwrap_or(0), counts.next().unwrap_or(0)));
    }
    let error = String::from_utf8_lossy(&out.stderr);
    Err(std::io::Error::other(crate::agents_admin::tail(error.trim(), 200)))
}

/// `dst` made the same as `src` (a missing `src` is an empty one): its folders and regular files copied where their
/// size or time differ, what is not in `src` removed; links, pipes and devices never copied. Folders open to all
/// (DIR_MODE), files readable and writable by all (FILE_MODE): see DIR_MODE. Returns (copied, removed).
///
/// Either side may be a volume a sandbox writes in (the source's /partage): both are walked through folder
/// descriptors (beneath.rs), one name at a time, never through a link, the copies written under a temporary name and
/// renamed in their folder's descriptor (security audit 2026-10-27, H1). A name the sandbox swaps or removes meanwhile
/// is skipped.
pub fn sync_tree(src: &Path, dst: &Path) -> std::io::Result<(usize, usize)> {
    let to = beneath::Dir::open_or_make(dst)?;
    open_dir(&to);
    let from = match beneath::Dir::open(src) {
        Ok(from) => Some(from),
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => None,
        Err(exc) => return Err(exc),
    };
    let mut counts = (0, 0);
    sync_dir(from.as_ref(), &to, &mut counts, 0)?;
    Ok(counts)
}

/// A folder open to all (one this account may not change, a sandbox's user's, stays as it is: writing in it then
/// fails, and `mirror` does it as the sandboxes' « root »).
fn open_dir(dir: &beneath::Dir) {
    if dir.stat_self().is_ok_and(|st| st.mode != DIR_MODE) {
        let _ = dir.chmod(DIR_MODE);
    }
}

/// A name swapped for a link, or gone, since its folder was read: None (skipped); any other error stays.
fn unless_swapped<T>(result: std::io::Result<T>) -> std::io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound || beneath::refused(&exc) => Ok(None),
        Err(exc) => Err(exc),
    }
}

fn sync_dir(
    src: Option<&beneath::Dir>,
    dst: &beneath::Dir,
    counts: &mut (usize, usize),
    depth: usize,
) -> std::io::Result<()> {
    use beneath::Kind;
    let mut kept: HashSet<OsString> = HashSet::new();
    if let Some(src) = src.filter(|_| depth <= 32) {
        for (name, meta) in src.entries()? {
            if temporary(&name) {
                continue;
            }
            let there = dst.stat(&name).ok();
            match meta.kind {
                Kind::Dir => {
                    let Some(from) = unless_swapped(src.sub_one(&name))? else { continue };
                    if there.is_some_and(|t| t.kind != Kind::Dir) {
                        dst.remove(&name)?;
                    }
                    let to = match dst.sub_one(&name) {
                        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => {
                            dst.mkdir(&name, DIR_MODE)?;
                            dst.sub_one(&name)?
                        }
                        other => other?,
                    };
                    open_dir(&to);
                    sync_dir(Some(&from), &to, counts, depth + 1)?;
                }
                Kind::File => {
                    let same = there.is_some_and(|t| {
                        t.kind == Kind::File
                            && t.size == meta.size
                            && (t.mtime, t.mtime_nsec) == (meta.mtime, meta.mtime_nsec)
                    });
                    if !same {
                        let Some(mut input) = unless_swapped(src.open_file(Path::new(&name)))? else { continue };
                        if there.is_some_and(|t| t.kind == Kind::Dir) {
                            dst.remove(&name)?;
                        }
                        dst.write_file(&name, FILE_MODE, Some(meta.modified()), |out| {
                            std::io::copy(&mut input, out).map(|_| ())
                        })?;
                        counts.0 += 1;
                    }
                }
                _ => continue,
            }
            kept.insert(name);
        }
    }
    for name in dst.entries()?.into_keys() {
        if kept.contains(&name) || temporary(&name) {
            continue;
        }
        dst.remove(&name)?;
        counts.1 += 1;
    }
    Ok(())
}

/// The modes a file the tools write in a shared folder gets (see DIR_MODE).
pub fn open_modes() -> (u32, u32) {
    (DIR_MODE, FILE_MODE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_folder_follows_another() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("volume"), dir.path().join("host"));
        std::fs::create_dir_all(src.join("resume")).unwrap();
        std::fs::write(src.join("resume/semaine.md"), "7 h").unwrap();
        std::fs::write(src.join("graphe.png"), "PNG").unwrap();
        std::os::unix::fs::symlink("/etc/hosts", src.join("hosts")).unwrap();
        std::fs::write(src.join(".bip-x.part"), "half").unwrap();
        std::fs::create_dir_all(dst.join("old")).unwrap();
        std::fs::write(dst.join("old/gone.md"), "x").unwrap();
        std::fs::write(dst.join("graphe.png"), "older and longer").unwrap();
        assert_eq!(sync_tree(&src, &dst).unwrap(), (2, 1));
        assert_eq!(std::fs::read_to_string(dst.join("resume/semaine.md")).unwrap(), "7 h");
        assert_eq!(std::fs::read_to_string(dst.join("graphe.png")).unwrap(), "PNG");
        assert!(!dst.join("hosts").exists() && dst.join("hosts").symlink_metadata().is_err());
        assert!(!dst.join("old").exists() && !dst.join(".bip-x.part").exists());
        assert_eq!((mode(&dst), mode(&dst.join("resume")), mode(&dst.join("graphe.png"))), (0o777, 0o777, 0o666));
        // Nothing changed: nothing copied.
        assert_eq!(sync_tree(&src, &dst).unwrap(), (0, 0));
        // A missing source is an empty one.
        assert_eq!(sync_tree(&dir.path().join("none"), &dst).unwrap(), (0, 2));
        assert!(is_empty(&dst));
    }

    /// The volume (the sandbox writes in it) mirrored while a thread swaps one of its folders for a link to « home »
    /// and back, in one step each, and with a link planted where the copy goes: nothing of home is copied into the
    /// folder kept in step, nothing written, chmodded or removed in home (security audit 2026-10-27, H1).
    #[test]
    fn a_mirror_never_follows_a_swapped_folder() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let (volume, host, home) = (dir.path().join("volume"), dir.path().join("host"), dir.path().join("home"));
        std::fs::create_dir_all(volume.join("d")).unwrap();
        std::fs::write(volume.join("d/notes.md"), "share").unwrap();
        std::fs::create_dir_all(home.join("keep")).unwrap();
        std::fs::write(home.join("notes.md"), "home secret").unwrap();
        std::fs::set_permissions(home.join("notes.md"), std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&home, volume.join("d.link")).unwrap();
        let untouched = |home: &Path| {
            let mut names: Vec<_> = std::fs::read_dir(home).unwrap().flatten().map(|e| e.file_name()).collect();
            names.sort();
            assert_eq!(names, [OsString::from("keep"), "notes.md".into()]);
            assert_eq!(std::fs::read_to_string(home.join("notes.md")).unwrap(), "home secret");
            assert_eq!((mode(home), mode(&home.join("notes.md")), mode(&home.join("keep"))), (0o700, 0o600, 0o755));
        };
        std::fs::set_permissions(home.join("keep"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper = {
            let (volume, stop) = (volume.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    beneath::exchange(&volume.join("d"), &volume.join("d.link")).unwrap();
                    std::thread::yield_now();
                    beneath::exchange(&volume.join("d"), &volume.join("d.link")).unwrap();
                }
            })
        };
        let mut copied = 0;
        for _ in 0..400 {
            let _ = std::fs::remove_dir_all(&host);
            if sync_tree(&volume, &host).is_ok() && host.join("d/notes.md").exists() {
                copied += 1;
                assert_eq!(std::fs::read_to_string(host.join("d/notes.md")).unwrap(), "share");
            }
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        assert!(copied > 0, "the mirror never got through");
        untouched(&home);
        // a link planted in the folder kept in step (where the copy is written)
        std::fs::remove_dir_all(&host).unwrap();
        std::fs::create_dir_all(&host).unwrap();
        std::os::unix::fs::symlink(&home, host.join("d")).unwrap();
        std::fs::remove_file(volume.join("d.link")).unwrap();
        sync_tree(&volume, &host).unwrap();
        assert!(host.join("d").symlink_metadata().unwrap().is_dir(), "the link replaced by a folder");
        untouched(&home);
    }

    #[test]
    fn the_mounts_name_the_volumes_or_the_folders() {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = "/data/bridge.db".into();
        let reads = vec![("vie".to_string(), "wellness".to_string())];
        assert_eq!(share_mounts(&config, "wellness", &reads, true), ["bipagents-share-wellness:/partage"]);
        assert_eq!(share_mounts(&config, "vie", &reads, true), ["bipagents-share-wellness:/lecture/wellness:ro"]);
        let root = shares_root(&config).to_string_lossy().into_owned();
        assert_eq!(share_mounts(&config, "vie", &reads, false), [format!("{root}/wellness:/lecture/wellness:ro,z")]);
        assert!(share_entry(&config, "bipagents-share-wellness:/lecture/wellness:ro"));
        assert!(share_entry(&config, &format!("{root}/wellness:/partage:z")));
        assert!(!share_entry(&config, "bipagents-files-vie:/fichiers:ro") && !share_entry(&config, "/x:/photos:ro"));
    }

    #[test]
    fn the_first_time_the_folder_goes_into_the_volume_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.path().join("data/bridge.db").to_string_lossy().into_owned();
        config.sandbox.isolate = "off".into();
        let host = shares_root(&config).join("wellness");
        std::fs::create_dir_all(&host).unwrap();
        std::fs::write(host.join("sommeil.csv"), "nuit;7h").unwrap();
        let volume = dir.path().join("volume");
        std::fs::create_dir_all(&volume).unwrap();
        let said = settle(&config, "wellness", &volume, None).unwrap();
        assert!(said.contains("1 file(s) copied into bipagents-share-wellness"), "{said}");
        assert_eq!(std::fs::read_to_string(volume.join("sommeil.csv")).unwrap(), "nuit;7h");
        let backup = shares_root(&config).join("wellness.before-volume");
        assert_eq!(std::fs::read_to_string(backup.join("sommeil.csv")).unwrap(), "nuit;7h");
        assert_eq!(std::fs::read_to_string(host.join("sommeil.csv")).unwrap(), "nuit;7h"); // kept in step
        assert_eq!(mode(&volume), 0o777);
        assert_eq!(volume_folders(&config).get("wellness"), Some(&volume));
        // Then the volume holds the truth: the agent's changes reach the folder, never the other way.
        std::fs::write(volume.join("semaine.md"), "ok").unwrap();
        std::fs::remove_file(volume.join("sommeil.csv")).unwrap();
        settle(&config, "wellness", &volume, None).unwrap();
        assert!(host.join("semaine.md").exists() && !host.join("sommeil.csv").exists());
        assert!(backup.join("sommeil.csv").exists());
        // A volume made again (a move, a restore): filled from the folder, no new backup of the same content.
        let fresh = dir.path().join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        assert!(settle(&config, "wellness", &fresh, None).unwrap().contains("made again"));
        assert_eq!(std::fs::read_to_string(fresh.join("semaine.md")).unwrap(), "ok");
        assert!(host.join("semaine.md").exists());
        assert_eq!(std::fs::read_dir(shares_root(&config)).unwrap().count(), 2);
        // Isolation turned off: nothing to bring back here (no podman in tests), the volume forgotten.
        revert_volumes(&config);
        assert!(volume_folders(&config).is_empty() && host.join("semaine.md").exists());
    }
}
