//! `bipbridge sandbox adopt --agent X [--dry-run] [--undo]`: an agent set up by hand before the bridge isolated the
//! sandboxes (owner decision 2026-10-22) still mounts folders of this machine in its sandbox (`terminal.docker_volumes`
//! entries the bridge didn't write), so Hermes keeps asking before risky commands and its sandbox is not isolated
//! (sandbox.rs). This replaces each such mount by what the bridge can keep up itself, at the same path in the sandbox
//! so the agent's habits and scripts keep working:
//!
//! - **code it runs** (a read-only folder of its own: skills, bin…): a named volume, read-only, refreshed from the
//!   folder at each bridge start (the person or Hermes update it there). Hermes mounts the profile's `skills` itself
//!   too (tools/credential_files.py, at `/root/.hermes/skills`), but not at the path the person chose, so the copy stays.
//! - **its working folders** (a writable folder of its own: scripts, data, output…, or one it writes in elsewhere in
//!   the person's home, like a folder of notes its cron reads): a named volume (`bipagents-<agent>-<name>`), the
//!   content copied once and checked (files and bytes); the folder stays untouched as a backup, never deleted.
//! - **Hermes' media folder** (`~/.hermes/media`): dropped, the bridge delivers files itself (`/envoi`, sent_files.rs).
//! - **a connector's credentials** (`~/.garmin-mcp`, `*token*`…): MCP servers run on Hermes' side, outside the sandbox,
//!   so it is dropped, unless one of the agent's own files (its scripts, skills…) names it: then it stays (the agent
//!   would break) and the plan says which file.
//! - anything else (outside the person's home, a single file, someone else's folder read-only): kept, and said.
//!
//! The new `docker_volumes` are set as sandbox.rs sets them (`hermes config set`), each change logged under
//! `bipbridge::sandbox`, the gateway restarted and the agent's old sandbox containers removed (they still mount the
//! folders). `<data>/sandbox.json` (`adopted`) keeps the mounts as they were before, for `--undo`, and the volumes to
//! refresh. Nothing here reads a file's content but to look for a folder's name in the agent's own files, and nothing
//! of it is printed or logged.

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::agents_admin::{default_run, Run};
use crate::config::{AgentConfig, Config};
use crate::{py, sandbox, shares, Dict};

const USAGE: &str = "usage: bipbridge sandbox adopt --agent AGENT [--dry-run] [--undo] [--config CONFIG]\n";
/// Files larger than this are not searched for a folder's name.
const SEARCHED_MAX: u64 = 2 * 1024 * 1024;
/// At most this many files searched in the agent's folders.
const SEARCHED_FILES: usize = 20_000;
/// Files named in the plan, a mount at most.
const NAMED_MAX: usize = 5;

// --- walking a folder ----------------------------------------------------------------------------------------------

/// What a folder holds, never through a symbolic link.
#[derive(Debug, Clone, PartialEq)]
enum Entry {
    Dir(u32),
    File {
        size: u64,
        mode: u32,
        mtime: i64,
    },
    Link(PathBuf),
    /// A pipe, a socket, a device: never copied.
    Other,
}

/// Files, bytes, links and what can't be copied, in a folder.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Tally {
    pub files: usize,
    pub bytes: u64,
    pub links: usize,
    pub skipped: usize,
}

fn walk(root: &Path) -> std::io::Result<BTreeMap<PathBuf, Entry>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(relative) = stack.pop() {
        for entry in std::fs::read_dir(root.join(&relative))? {
            let entry = entry?;
            let path = relative.join(entry.file_name());
            let meta = entry.path().symlink_metadata()?;
            let kind = meta.file_type();
            let found = if kind.is_symlink() {
                Entry::Link(std::fs::read_link(entry.path())?)
            } else if kind.is_dir() {
                stack.push(path.clone());
                Entry::Dir(meta.mode() & 0o7777)
            } else if kind.is_file() {
                Entry::File { size: meta.len(), mode: meta.mode() & 0o7777, mtime: meta.mtime() }
            } else {
                Entry::Other
            };
            out.insert(path, found);
        }
    }
    Ok(out)
}

fn tally_of(entries: &BTreeMap<PathBuf, Entry>) -> Tally {
    let mut tally = Tally::default();
    for entry in entries.values() {
        match entry {
            Entry::File { size, .. } => {
                tally.files += 1;
                tally.bytes += size;
            }
            Entry::Link(_) => tally.links += 1,
            Entry::Other => tally.skipped += 1,
            Entry::Dir(_) => {}
        }
    }
    tally
}

pub fn tally(root: &Path) -> std::io::Result<Tally> {
    walk(root).map(|e| tally_of(&e))
}

fn copy_one(source: &Path, target: &Path, mode: u32, mtime: i64) -> std::io::Result<()> {
    let tmp = target.with_file_name(format!(
        ".{}.bip-part",
        target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
    ));
    let result = (|| {
        let mut input = sandbox::open_plain(source)?;
        let mut out = std::fs::File::create(&tmp)?;
        std::io::copy(&mut input, &mut out)?;
        out.set_permissions(std::fs::Permissions::from_mode(mode))?;
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime.max(0) as u64);
        out.set_modified(when)?;
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// `target` made to hold what `source` holds (files with their modes and times, links as links, never pipes or
/// devices); `exact`: what `source` doesn't hold any more removed. Returns the entries written or removed.
pub fn mirror(source: &Path, target: &Path, exact: bool) -> std::io::Result<usize> {
    let wanted = walk(source)?;
    std::fs::create_dir_all(target)?;
    let present = walk(target)?;
    let mut changed = 0;
    for (relative, entry) in &wanted {
        let path = target.join(relative);
        let now = present.get(relative);
        match entry {
            Entry::Dir(_) => {
                if !matches!(now, Some(Entry::Dir(_))) {
                    if now.is_some() {
                        remove_any(&path)?;
                    }
                    std::fs::create_dir(&path)?;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
                    changed += 1;
                }
            }
            Entry::File { size, mode, mtime } => {
                if let Some(Entry::File { size: s, mtime: m, mode: md }) = now {
                    if s == size && m == mtime {
                        if md != mode {
                            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode))?;
                        }
                        continue;
                    }
                }
                if matches!(now, Some(Entry::Dir(_)) | Some(Entry::Link(_)) | Some(Entry::Other)) {
                    remove_any(&path)?;
                }
                copy_one(&source.join(relative), &path, *mode, *mtime)?;
                changed += 1;
            }
            Entry::Link(to) => {
                if now == Some(entry) {
                    continue;
                }
                if now.is_some() {
                    remove_any(&path)?;
                }
                std::os::unix::fs::symlink(to, &path)?;
                changed += 1;
            }
            Entry::Other => {}
        }
    }
    if exact {
        // children before their folder (reverse order), each gone once
        for relative in present.keys().rev() {
            let path = target.join(relative);
            if !wanted.contains_key(relative) && path.symlink_metadata().is_ok() {
                remove_any(&path)?;
                changed += 1;
            }
        }
    }
    // the folders' own modes last (a read-only folder copied into would refuse the files)
    for (relative, entry) in wanted.iter().rev() {
        if let Entry::Dir(mode) = entry {
            std::fs::set_permissions(target.join(relative), std::fs::Permissions::from_mode(*mode | 0o700))?;
        }
    }
    Ok(changed)
}

// --- the plan ------------------------------------------------------------------------------------------------------

/// What becomes of a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Read-only code: a read-only volume refreshed at each bridge start.
    Code,
    /// A working folder: a volume, the content copied once.
    Work,
    Drop,
    Keep,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub spec: String,
    pub host: PathBuf,
    pub target: String,
    pub read_only: bool,
    pub action: Action,
    pub why: String,
    pub volume: Option<String>,
    pub tally: Tally,
}

#[derive(Debug, Clone)]
pub struct Plan {
    pub agent: String,
    pub folder: PathBuf,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub items: Vec<Item>,
}

impl Plan {
    /// Host paths still mounted once the plan is applied.
    pub fn left(&self) -> Vec<String> {
        sandbox::host_mounts(&self.after)
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn inside(path: &Path, root: &Path) -> bool {
    let (path, root) = (canonical(path), canonical(root));
    path != root && path.starts_with(&root)
}

/// A connector's credentials, by the folder's name.
pub fn credential_like(name: &str) -> bool {
    let n = name.trim_start_matches('.').to_lowercase();
    n.ends_with("-mcp")
        || n.ends_with("_mcp")
        || n.starts_with("mcp-")
        || n == "ssh"
        || ["token", "credential", "secret", "oauth", "gnupg", "keyring"].iter().any(|w| n.contains(w))
}

/// Hermes' media folder (`~/.hermes/media`), where agents used to put files for the person.
fn media_folder(host: &Path, target: &str) -> bool {
    let host = host.to_string_lossy();
    let host = host.trim_end_matches('/');
    let target = target.trim_end_matches('/');
    host.ends_with("/.hermes/media") || target.ends_with("/.hermes/media")
}

/// `bipagents-<agent>-<folder's name>`, in podman's characters, not one of `taken`.
pub fn volume_name(agent: &str, host: &Path, taken: &[String]) -> String {
    let base = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let slug: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "dossier".to_string() } else { slug };
    let first = format!("bipagents-{agent}-{slug}");
    let mut name = first.clone();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{first}-{n}");
        n += 1;
    }
    name
}

/// The MCP servers of the agent's config whose command, arguments or environment name this folder.
fn mcp_users(folder: &Path, host: &Path) -> Vec<String> {
    let base = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let full = host.to_string_lossy().into_owned();
    let home = folder.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let mut users = Vec::new();
    for dir in [folder.to_path_buf(), home] {
        if let Some(Value::Object(servers)) = shares::profile_config(&dir).get("mcp_servers") {
            for (name, entry) in servers {
                let text = py::dumps(entry);
                if (text.contains(&full) || (base.len() >= 4 && text.contains(&base))) && !users.contains(name) {
                    users.push(name.clone());
                }
            }
        }
    }
    users
}

fn searchable(path: &Path, size: u64) -> bool {
    size <= SEARCHED_MAX && path.file_name().is_some_and(|n| !n.to_string_lossy().starts_with(".bip-"))
}

/// The agent's own files (in `roots`) that name `needles`: « <folder>/<relative path> », a few at most. Only names
/// are returned, never content.
fn naming(roots: &[(PathBuf, String)], needles: &[String]) -> Vec<String> {
    let needles: Vec<&[u8]> = needles.iter().filter(|n| n.len() >= 4).map(|n| n.as_bytes()).collect();
    let mut found = Vec::new();
    let mut searched = 0;
    for (root, shown) in roots {
        let Ok(entries) = walk(root) else { continue };
        for (relative, entry) in entries {
            let Entry::File { size, .. } = entry else { continue };
            let path = root.join(&relative);
            if !searchable(&path, size) || searched >= SEARCHED_FILES {
                continue;
            }
            searched += 1;
            let Ok(mut file) = sandbox::open_plain(&path) else { continue };
            let mut bytes = Vec::new();
            if std::io::Read::read_to_end(&mut file, &mut bytes).is_err() || bytes.iter().take(8192).any(|b| *b == 0) {
                continue;
            }
            if needles.iter().any(|n| bytes.windows(n.len()).any(|w| w == *n)) {
                found.push(format!("{shown}/{}", relative.to_string_lossy()));
                if found.len() >= NAMED_MAX {
                    return found;
                }
            }
        }
    }
    found
}

/// A `docker_volumes` entry that mounts a folder of this machine: (host, target, options).
fn host_entry(spec: &str) -> Option<(String, String, String)> {
    let spec = spec.trim();
    if sandbox::host_mounts(&[spec.to_string()]).is_empty() {
        return None;
    }
    let mut parts = spec.splitn(3, ':');
    let host = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    Some((host, target, parts.next().unwrap_or("").to_string()))
}

/// What `adopt` would do for this agent, from its profile's mounts now.
pub fn plan(config: &Config, agent: &AgentConfig) -> anyhow::Result<Plan> {
    let Some(folder) = shares::profile_dir(config, agent) else {
        anyhow::bail!("{} is not an agent this install manages", agent.name);
    };
    let before = shares::docker_volumes(&folder);
    let mut own_roots = vec![folder.clone()];
    if let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) {
        own_roots.push(PathBuf::from(space));
    }
    // a folder the agent writes in elsewhere in the person's home (a folder of notes it keeps, read by its cron) is a
    // working folder too; anything outside the home, or read-only and not its own, stays as it is
    let person_home = crate::place_backup::home();
    let mut items: Vec<Item> = Vec::new();
    let mut taken: Vec<String> = before.iter().map(|v| v.split(':').next().unwrap_or("").to_string()).collect();
    for spec in &before {
        if sandbox::owned_volume(config, spec) {
            continue; // the bridge's own (its uploads, shared folders): sandbox.rs replaces them
        }
        let Some((raw, target, options)) = host_entry(spec) else { continue };
        let host = PathBuf::from(if raw.starts_with('~') { py::expanduser(&raw) } else { raw.clone() });
        let read_only = options.split(',').any(|o| o.trim() == "ro");
        let name = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut item = Item {
            spec: spec.clone(),
            host: host.clone(),
            target: target.clone(),
            read_only,
            action: Action::Keep,
            why: String::new(),
            volume: None,
            tally: Tally::default(),
        };
        if !host.is_absolute() {
            item.why = "a relative path: not recognised".into();
        } else if media_folder(&host, &target) {
            item.action = Action::Drop;
            item.why = "Hermes' media folder: the bridge delivers files itself now (/envoi)".into();
        } else if credential_like(&name) {
            item.action = Action::Drop; // decided below, once the agent's own folders are known
        } else if !host.exists() {
            item.action = Action::Drop;
            item.why = "missing on this machine".into();
        } else if !host.is_dir() {
            item.why = "a single file, not a folder: not recognised".into();
        } else if own_roots.iter().any(|r| inside(&host, r)) || (!read_only && inside(&host, &person_home)) {
            item.action = if read_only { Action::Code } else { Action::Work };
            item.volume = Some(volume_name(&agent.name, &host, &taken));
            taken.push(item.volume.clone().unwrap_or_default());
            item.tally = tally(&host)?;
        } else {
            item.why = "a folder outside the agent's own: not recognised".into();
        }
        items.push(item);
    }
    // credentials: kept only when one of the agent's own files names them
    // (its folders moved now, and those an earlier adopt moved: their volumes, where the agent works since)
    let shown = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut roots: Vec<(PathBuf, String)> = items
        .iter()
        .filter(|i| matches!(i.action, Action::Code | Action::Work))
        .map(|i| (i.host.clone(), shown(&i.host)))
        .collect();
    if let Some(entry) = adopted(config, &agent.name) {
        for volume in entry.get("volumes").and_then(Value::as_array).into_iter().flatten() {
            let name = py::py_str(volume.get("name").unwrap_or(&Value::Null));
            let path = PathBuf::from(py::py_str(volume.get("mountpoint").unwrap_or(&Value::Null)));
            if path.is_dir() && !roots.iter().any(|(p, _)| *p == path) {
                roots.push((path, format!("volume {name}")));
            }
        }
    }
    for item in items.iter_mut().filter(|i| i.action == Action::Drop && i.why.is_empty()) {
        let name = item.host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let named = naming(&roots, &[name, item.target.clone()]);
        let users = mcp_users(&folder, &item.host);
        if !named.is_empty() {
            item.action = Action::Keep;
            item.why = format!(
                "a connector's credentials that the agent's own files use ({}): without them they would break",
                named.join(", ")
            );
        } else if !users.is_empty() {
            item.why = format!(
                "a connector's credentials, used by the MCP server « {} », which runs outside the sandbox",
                users.join(" », « ")
            );
        } else {
            item.why = "a connector's credentials that nothing in the sandbox uses".into();
        }
    }
    let after: Vec<String> = before
        .iter()
        .filter_map(|spec| match items.iter().find(|i| &i.spec == spec) {
            None => Some(spec.clone()),
            Some(item) => match item.action {
                Action::Code => Some(format!("{}:{}:ro", item.volume.clone().unwrap_or_default(), item.target)),
                Action::Work => Some(format!("{}:{}", item.volume.clone().unwrap_or_default(), item.target)),
                Action::Drop => None,
                Action::Keep => Some(spec.clone()),
            },
        })
        .collect();
    Ok(Plan { agent: agent.name.clone(), folder, before, after, items })
}

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

fn counted(tally: &Tally) -> String {
    let mut text = format!("{} files, {}", tally.files, size_text(tally.bytes));
    if tally.links > 0 {
        text.push_str(&format!(", {} links", tally.links));
    }
    if tally.skipped > 0 {
        text.push_str(&format!(", {} special files left out", tally.skipped));
    }
    text
}

/// The plan in plain lines.
pub fn describe(plan: &Plan) -> Vec<String> {
    let mut lines = Vec::new();
    if plan.items.is_empty() {
        lines.push(format!("{}: no folder of this machine in its sandbox, nothing to do", plan.agent));
        return lines;
    }
    lines.push(format!("{}: {} folder(s) of this machine in its sandbox", plan.agent, plan.items.len()));
    for item in &plan.items {
        let host = item.host.display();
        let volume = item.volume.clone().unwrap_or_default();
        lines.push(match item.action {
            Action::Code => format!(
                "  copy  {host} -> volume {volume} at {} (read-only, refreshed from the folder at each bridge start; {})",
                item.target,
                counted(&item.tally)
            ),
            Action::Work => format!(
                "  move  {host} -> volume {volume} at {} (copied once, {}; the folder stays as it is, as a backup)",
                item.target,
                counted(&item.tally)
            ),
            Action::Drop => format!("  drop  {host} ({})", item.why),
            Action::Keep => format!("  keep  {host} ({})", item.why),
        });
    }
    let left = plan.left();
    if left.is_empty() {
        lines.push(
            "  then: isolated, no folder of this machine in its sandbox (Hermes stops asking before harmless commands)"
                .to_string(),
        );
    } else {
        lines.push(format!("  then: still not isolated, because of: {}", left.join(" ")));
    }
    lines
}

// --- applying it ---------------------------------------------------------------------------------------------------

fn adopted(config: &Config, agent: &str) -> Option<Dict> {
    sandbox::load_state(config).get("adopted")?.get(agent)?.as_object().cloned()
}

fn strings_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
        _ => Vec::new(),
    }
}

/// The code volumes of an adopted agent refreshed from their folders (at each bridge start, sandbox.rs's
/// `isolate`). Returns the entries written or removed; a folder that went missing is left alone.
pub fn refresh(config: &Config, agent: &str) -> usize {
    let Some(entry) = adopted(config, agent) else { return 0 };
    let mut changed = 0;
    for volume in entry.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        if volume.get("kind").and_then(Value::as_str) != Some("code") {
            continue;
        }
        let host = PathBuf::from(py::py_str(volume.get("host").unwrap_or(&Value::Null)));
        let mountpoint = PathBuf::from(py::py_str(volume.get("mountpoint").unwrap_or(&Value::Null)));
        if !host.is_dir() || !mountpoint.is_dir() {
            continue;
        }
        match mirror(&host, &mountpoint, true) {
            Ok(n) => changed += n,
            Err(exc) => {
                tracing::warn!(target: "bipbridge::sandbox", agent, folder = %host.display(), error = %exc, "adopted code not refreshed in its volume")
            }
        }
    }
    if changed > 0 {
        tracing::info!(target: "bipbridge::sandbox", agent, changed, "adopted code refreshed in its volumes");
    }
    changed
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The new mounts set (as sandbox.rs sets them), the gateway restarted, the old sandbox containers removed.
async fn set_volumes(
    config: &Config,
    agent: &AgentConfig,
    folder: &Path,
    volumes: &[String],
    run: &Run,
) -> anyhow::Result<()> {
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env = BTreeMap::from([("HERMES_HOME".to_string(), home)]);
    run(
        args(&[
            &config.hermes.bin,
            "-p",
            &agent.name,
            "config",
            "set",
            "terminal.docker_volumes",
            &py::dumps(&json!(volumes)),
        ]),
        env,
    )
    .await?;
    let podman = sandbox::engine(config);
    let old: Vec<String> = match &podman {
        Some(podman) => run(
            args(&[
                podman,
                "ps",
                "-aq",
                "--filter",
                "label=hermes-agent=1",
                "--filter",
                &format!("label=hermes-profile={}", agent.name),
            ]),
            BTreeMap::new(),
        )
        .await
        .map(|out| out.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default(),
        None => Vec::new(),
    };
    if let Err(exc) = crate::agents_admin::restart_gateway(config, agent, run).await {
        tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, error = %crate::agents_admin::tail(&exc.to_string(), 300), "gateway not restarted after its mounts changed");
        anyhow::bail!("the mounts are set, but the gateway did not restart: restart it to apply them ({exc})");
    }
    if let (Some(podman), false) = (&podman, old.is_empty()) {
        let mut command = args(&[podman, "rm", "-f"]);
        command.extend(old.iter().cloned());
        if let Err(exc) = run(command, BTreeMap::new()).await {
            tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, error = %crate::agents_admin::tail(&exc.to_string(), 200), "old sandbox containers not removed");
        }
    }
    Ok(())
}

/// The plan applied: volumes made and filled, the mounts rewritten, the gateway restarted. Returns what to print.
pub async fn apply(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<Vec<String>> {
    let plan = plan(config, agent)?;
    let mut lines = describe(&plan);
    if plan.items.is_empty() || plan.after == plan.before {
        if !plan.items.is_empty() {
            lines.push("nothing changed".into());
        }
        return Ok(lines);
    }
    let Some(podman) = sandbox::engine(config) else {
        anyhow::bail!("this install doesn't isolate the sandboxes (it needs Linux and podman, [sandbox] isolate)");
    };
    let mut volumes: Vec<Value> = Vec::new();
    for item in plan.items.iter().filter(|i| matches!(i.action, Action::Code | Action::Work)) {
        let name = item.volume.clone().unwrap_or_default();
        let mountpoint = sandbox::mountpoint(run, &podman, &name).await?;
        let (host, target, code) = (item.host.clone(), mountpoint.clone(), item.action == Action::Code);
        let message = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let present = tally(&target)?;
            if code {
                mirror(&host, &target, true)?;
            } else if present.files + present.links > 0 {
                return Ok(format!("holds {} already (copied before): left as it is", counted(&present)));
            } else {
                mirror(&host, &target, false)?;
            }
            let (wanted, copied) = (tally(&host)?, tally(&target)?);
            if (wanted.files, wanted.bytes, wanted.links) != (copied.files, copied.bytes, copied.links) {
                anyhow::bail!(
                    "the copy of {} is not whole ({} in the folder, {} in the volume): nothing changed",
                    host.display(),
                    counted(&wanted),
                    counted(&copied)
                );
            }
            Ok(format!("copied and checked: {}", counted(&copied)))
        })
        .await??;
        lines.push(format!("  {name}: {message}"));
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, folder = %item.host.display(), volume = %name, path = %item.target, kind = if item.action == Action::Code { "code" } else { "work" }, "hand-made mount replaced by a volume");
        volumes.push(json!({"name": name, "host": item.host.to_string_lossy(), "target": item.target,
                            "kind": if item.action == Action::Code { "code" } else { "work" },
                            "mountpoint": mountpoint.to_string_lossy()}));
    }
    for item in &plan.items {
        match item.action {
            Action::Drop => {
                tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, folder = %item.host.display(), reason = %item.why, "hand-made mount dropped")
            }
            Action::Keep => {
                tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, folder = %item.host.display(), reason = %item.why, "hand-made mount kept")
            }
            _ => {}
        }
    }
    let (name, before, after) = (agent.name.clone(), plan.before.clone(), plan.after.clone());
    sandbox::save_state(config, |state| {
        let all = state.entry("adopted").or_insert_with(|| json!({}));
        if !all.is_object() {
            *all = json!({});
        }
        let earlier = all.get(&name).cloned();
        // a second adopt (after a Carnet import, say): the mounts of the first kept for --undo
        let first = earlier.as_ref().and_then(|e| e.get("before")).cloned().unwrap_or(json!(before));
        let mut kept: Vec<Value> =
            earlier.as_ref().and_then(|e| e.get("volumes")).and_then(Value::as_array).cloned().unwrap_or_default();
        for v in volumes {
            kept.retain(|k| k.get("name") != v.get("name"));
            kept.push(v);
        }
        all[name.as_str()] =
            json!({"at": crate::tools_knowledge::now_iso_seconds(), "before": first, "after": after, "volumes": kept});
    })?;
    set_volumes(config, agent, &plan.folder, &plan.after, run).await?;
    let now = shares::docker_volumes(&plan.folder);
    let left = sandbox::host_mounts(&now);
    if left.is_empty() {
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, "agent adopted: its sandbox mounts no folder of this machine, it is isolated");
        lines.push(format!("{}: isolated now (mounts: {})", agent.name, now.join(" ")));
    } else {
        tracing::warn!(target: "bipbridge::sandbox", agent = %agent.name, mounts = %left.join(" "), "agent adopted, still not isolated: these folders stay mounted");
        lines.push(format!("{}: still not isolated, because of: {}", agent.name, left.join(" ")));
    }
    let backups: Vec<String> =
        plan.items.iter().filter(|i| i.action == Action::Work).map(|i| i.host.to_string_lossy().into_owned()).collect();
    if !backups.is_empty() {
        lines.push(format!(
            "the folders copied stay as they were, untouched (a backup; the agent now works in the volumes): {}",
            backups.join(" ")
        ));
    }
    lines.push(format!("undo: bipbridge sandbox adopt --agent {} --undo", agent.name));
    Ok(lines)
}

/// The mounts as they were before the first adopt, set again. The volumes stay (the agent's work since is in them).
pub async fn undo(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<Vec<String>> {
    let Some(entry) = adopted(config, &agent.name) else {
        anyhow::bail!("{} was never adopted: nothing to undo", agent.name);
    };
    let Some(folder) = shares::profile_dir(config, agent) else {
        anyhow::bail!("{} is not an agent this install manages", agent.name);
    };
    let before = strings_of(entry.get("before"));
    set_volumes(config, agent, &folder, &before, run).await?;
    let name = agent.name.clone();
    sandbox::save_state(config, |state| {
        if let Some(Value::Object(all)) = state.get_mut("adopted") {
            all.remove(&name);
        }
    })?;
    tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, mounts = %before.join(" "), "adopt undone: the hand-made mounts are back");
    let mut lines = vec![format!("{}: mounts as before: {}", agent.name, before.join(" "))];
    for volume in entry.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        let (name, mountpoint) = (
            py::py_str(volume.get("name").unwrap_or(&Value::Null)),
            py::py_str(volume.get("mountpoint").unwrap_or(&Value::Null)),
        );
        lines.push(format!(
            "  volume {name} kept ({mountpoint}): what the agent did there since is in it; remove it with podman volume rm {name}"
        ));
    }
    Ok(lines)
}

// --- the command line ----------------------------------------------------------------------------------------------

/// `bipbridge sandbox adopt …` (argv after `adopt`).
pub fn cli(argv: &[String]) -> i32 {
    let (mut config_path, mut agent_name) = (None, None);
    let (mut dry_run, mut undoing) = (false, false);
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        match arg {
            "--config" | "--agent" if i + 1 < argv.len() => {
                if arg == "--config" {
                    config_path = Some(argv[i + 1].clone());
                } else {
                    agent_name = Some(argv[i + 1].clone());
                }
                i += 1;
            }
            "--dry-run" => dry_run = true,
            "--undo" => undoing = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            _ if arg.starts_with("--config=") => config_path = Some(arg["--config=".len()..].to_string()),
            _ if arg.starts_with("--agent=") => agent_name = Some(arg["--agent=".len()..].to_string()),
            _ if !arg.starts_with('-') && agent_name.is_none() => agent_name = Some(arg.to_string()),
            other => {
                eprint!("{USAGE}");
                eprintln!("bipbridge sandbox adopt: unknown argument {other}");
                return 2;
            }
        }
        i += 1;
    }
    let Some(agent_name) = agent_name else {
        eprint!("{USAGE}");
        return 2;
    };
    crate::logs::setup_logging("info");
    let config = match crate::config::load_config(config_path.as_deref()) {
        Ok(c) => c,
        Err(exc) => {
            eprintln!("config error: {exc}");
            return 2;
        }
    };
    crate::logs::setup_logging(&config.log_level);
    let Ok(agent) = crate::agents_admin::managed(&config, &agent_name) else {
        eprintln!("{agent_name}: not an agent this install manages");
        return 2;
    };
    if !dry_run && !sandbox::isolates(&config, &agent) {
        eprintln!(
            "{agent_name}: the bridge doesn't isolate this agent's sandbox (this install doesn't isolate, or its tools \
             don't run in Hermes' docker sandbox, or its desktop runs there): nothing to adopt"
        );
        return 1;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let result = if dry_run {
        plan(&config, &agent).map(|p| {
            let mut lines = describe(&p);
            if undoing {
                lines = match adopted(&config, &agent.name) {
                    Some(e) => vec![format!(
                        "{}: --undo would set the mounts back to: {}",
                        agent.name,
                        strings_of(e.get("before")).join(" ")
                    )],
                    None => vec![format!("{}: never adopted, nothing to undo", agent.name)],
                };
            } else if !sandbox::isolates(&config, &agent) {
                lines.push("(this install doesn't isolate this agent's sandbox: the real run would refuse)".into());
            }
            lines.push("dry run: nothing changed".into());
            lines
        })
    } else if undoing {
        runtime.block_on(undo(&config, &agent, &default_run()))
    } else {
        runtime.block_on(apply(&config, &agent, &default_run()))
    };
    match result {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            0
        }
        Err(exc) => {
            eprintln!("{agent_name}: {exc}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_credentials() {
        for name in [".garmin-mcp", ".oura-mcp", "google_token", "credentials", ".ssh", "mcp-strava"] {
            assert!(credential_like(name), "{name}");
        }
        for name in ["scripts", "data", "bin-racine", "emails-memoire", "media"] {
            assert!(!credential_like(name), "{name}");
        }
    }

    #[test]
    fn volume_names_are_podmans_and_unique() {
        let host = Path::new("/home/a/.hermes/profiles/wellness/bin-racine");
        assert_eq!(volume_name("wellness", host, &[]), "bipagents-wellness-bin-racine");
        let taken = vec!["bipagents-wellness-bin-racine".to_string()];
        assert_eq!(volume_name("wellness", host, &taken), "bipagents-wellness-bin-racine-2");
        assert_eq!(volume_name("vie", Path::new("/x/Mes Notes_2"), &[]), "bipagents-vie-mes-notes-2");
    }

    #[test]
    fn the_media_folder_is_hermes() {
        assert!(media_folder(Path::new("/home/alex/.hermes/media"), "/home/hermes/.hermes/media"));
        assert!(!media_folder(Path::new("/home/alex/photos"), "/photos"));
    }

    #[test]
    fn a_mirror_follows_its_folder_and_keeps_modes() {
        let dir = tempfile::tempdir().unwrap();
        let (source, target) = (dir.path().join("bin"), dir.path().join("volume"));
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("run.sh"), b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(source.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(source.join("sub/a.py"), b"print(1)\n").unwrap();
        std::os::unix::fs::symlink("run.sh", source.join("link")).unwrap();
        assert_eq!(mirror(&source, &target, true).unwrap(), 4);
        assert_eq!(std::fs::metadata(target.join("run.sh")).unwrap().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_link(target.join("link")).unwrap(), PathBuf::from("run.sh"));
        assert_eq!(tally(&source).unwrap(), tally(&target).unwrap());
        assert_eq!(mirror(&source, &target, true).unwrap(), 0);
        std::fs::remove_dir_all(source.join("sub")).unwrap();
        std::fs::write(target.join("extra"), b"x").unwrap();
        mirror(&source, &target, true).unwrap();
        assert!(!target.join("sub").exists() && !target.join("extra").exists());
        // a working folder's copy: what the volume has more stays
        std::fs::write(target.join("made-by-the-agent"), b"x").unwrap();
        mirror(&source, &target, false).unwrap();
        assert!(target.join("made-by-the-agent").exists());
    }

    #[test]
    fn only_names_come_out_of_a_search() {
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(scripts.join("sync.py"), b"open('/home/hermes/.garmin-mcp/token')").unwrap();
        std::fs::write(scripts.join("other.py"), b"print('hello')").unwrap();
        std::fs::write(scripts.join("blob.bin"), b"\0\0.garmin-mcp").unwrap();
        let roots = [(scripts, "scripts".to_string())];
        assert_eq!(naming(&roots, &[".garmin-mcp".into()]), vec!["scripts/sync.py".to_string()]);
        assert!(naming(&roots, &[".oura-mcp".into()]).is_empty());
    }
}
