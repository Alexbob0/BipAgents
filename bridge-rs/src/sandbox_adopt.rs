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
//! **Move and link** (2026-10-23): a folder that programs OUTSIDE the sandbox keep writing (a connector of the agent's,
//! an MCP server on Hermes' side, writing its databases or refreshing its tokens there) can't be copied once: they
//! would go on writing the old folder, and the agent would read a stale copy. Such a folder is « linked »: copied
//! into its volume (`bipagents-<agent>-<name>`, checked), renamed `<name>.before-adopt` (a backup, never deleted),
//! and a symbolic link left at its path, to the volume's folder (`podman volume inspect`'s Mountpoint): the sandbox
//! mounts the named volume (isolated, by Hermes' rule), the programs outside keep writing the same path. Chosen when
//!
//! - the person says so (`--link <path>`, the folder's path on this machine or in the sandbox);
//! - the agent's MCP servers name it (their command, arguments, environment or working folder, in its config.yaml);
//! - files in it changed since the agent's sandbox last stopped (podman knows when), or a process of this machine
//!   that is not a container's has a file of it open (Linux's /proc);
//! - a connector's credentials that the agent's own files use (they were kept before, the agent not isolated): its
//!   own scripts then call the service from the sandbox, so the plan lists the sites they will need (from the
//!   service's name and the addresses written in those files) and `--allow-sites` allows them in the same run
//!   (sandbox_sites.rs), never silently.
//!
//! A linked volume is open to the sandbox's user and this account alike (folders 0777, files 0666, executables 0777:
//! in a rootless sandbox this account is « root », the agent's user another one), and the bridge keeps it so every few
//! minutes (`tend_links`); a symbolic link made in it that leads out of it is removed (programs of this machine
//! write through it: the sandbox must not point them elsewhere). `--undo` takes the link away and puts the folder
//! back, with the volume's latest content. Backups and exports take a linked volume like a working one
//! (installer/bipinstall/volumes.py), and the link as a link.
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

const USAGE: &str = "usage: bipbridge sandbox adopt --agent AGENT [--dry-run] [--undo] [--link PATH]... \
                     [--allow-sites] [--config CONFIG]\n";
/// The original folder of a linked one, kept as a backup.
pub const BACKUP_SUFFIX: &str = ".before-adopt";
/// Folders and files of a linked volume: open to the sandbox's user and this account alike.
const LINK_DIR_MODE: u32 = 0o777;
const LINK_FILE_MODE: u32 = 0o666;
/// The services' sites a connector's credentials are used with, by a word of the folder's name.
const SERVICE_SITES: [(&str, &[&str]); 8] = [
    ("garmin", &["connectapi.garmin.com", "connect.garmin.com", "sso.garmin.com", "thegarth.s3.amazonaws.com"]),
    ("oura", &["api.ouraring.com", "cloud.ouraring.com"]),
    ("strava", &["www.strava.com"]),
    ("withings", &["wbsapi.withings.net", "account.withings.com"]),
    ("fitbit", &["api.fitbit.com"]),
    ("whoop", &["api.prod.whoop.com"]),
    ("polar", &["www.polaraccesslink.com", "polarremote.com"]),
    ("google", &["oauth2.googleapis.com", "www.googleapis.com"]),
];
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
    /// A folder programs outside the sandbox keep writing: a volume, the folder renamed as a backup and a link to
    /// the volume left at its path.
    Link,
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
    /// The sites the agent's own files will need from the sandbox (a connector's credentials linked).
    pub sites: Vec<String>,
}

/// What the person or this machine tells the plan.
#[derive(Debug, Clone, Default)]
pub struct Hints {
    /// `--link <path>`: on this machine or in the sandbox.
    pub link: Vec<String>,
    /// When the agent's sandbox last stopped (Unix seconds); None: unknown, or one runs now.
    pub last_run: Option<i64>,
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

/// `path` in `text`, as a whole path (followed by `/`, a quote, a space or the end: not `/x/database` for `/x/data`).
fn names_path(text: &str, path: &str) -> bool {
    !path.is_empty()
        && text.match_indices(path).any(|(at, _)| {
            matches!(text[at + path.len()..].chars().next(), None | Some('/' | '"' | '\'' | ' ' | ':' | ','))
        })
}

/// The MCP servers of the agent's config that name this working folder by its path (a folder's bare name, like
/// `data`, says too little): its full path, `~/…`, `<parent>/<name>`, or its parent's path with `<name>/` beside it
/// (a working folder and a relative path).
fn mcp_writers(folder: &Path, host: &Path) -> Vec<String> {
    let base = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let full = host.to_string_lossy().into_owned();
    let parent = host.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let parent_name = host.parent().and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned());
    let person_home = crate::place_backup::home();
    let tilde = host.strip_prefix(&person_home).ok().map(|r| format!("~/{}", r.to_string_lossy()));
    let home = folder.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    let mut users = Vec::new();
    for dir in [folder.to_path_buf(), home] {
        if let Some(Value::Object(servers)) = shares::profile_config(&dir).get("mcp_servers") {
            for (name, entry) in servers {
                let text = py::dumps(entry);
                let named = names_path(&text, &full)
                    || tilde.as_ref().is_some_and(|t| names_path(&text, t))
                    || parent_name.as_ref().is_some_and(|p| names_path(&text, &format!("{p}/{base}")))
                    || (!parent.is_empty()
                        && text.contains(&parent)
                        && (text.contains(&format!("{base}/")) || text.contains(&format!("\"{base}\""))));
                if named && !users.contains(name) {
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
#[cfg(test)]
fn naming(roots: &[(PathBuf, String)], needles: &[String]) -> Vec<String> {
    naming_found(roots, needles).into_iter().map(|(shown, _)| shown).collect()
}

/// `naming`, with each file's path.
fn naming_found(roots: &[(PathBuf, String)], needles: &[String]) -> Vec<(String, PathBuf)> {
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
                found.push((format!("{shown}/{}", relative.to_string_lossy()), path));
                if found.len() >= NAMED_MAX {
                    return found;
                }
            }
        }
    }
    found
}

/// Files in `root` changed after `since` (Unix seconds), never through a link.
fn changed_since(root: &Path, since: i64) -> usize {
    walk(root)
        .map(|entries| entries.values().filter(|e| matches!(e, Entry::File { mtime, .. } if *mtime > since)).count())
        .unwrap_or(0)
}

/// The processes of this machine (not a container's) with a file of `root` open: their names (Linux's /proc; none
/// elsewhere). Only the names, never their arguments.
fn open_by_host(root: &Path) -> Vec<String> {
    let root = canonical(root);
    let mut names: Vec<String> = Vec::new();
    let Ok(procs) = std::fs::read_dir("/proc") else { return names };
    for proc in procs.flatten() {
        let pid = proc.file_name().to_string_lossy().into_owned();
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let dir = proc.path();
        let Ok(fds) = std::fs::read_dir(dir.join("fd")) else { continue };
        let holds = fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t.starts_with(&root)));
        if !holds {
            continue;
        }
        let cgroup = std::fs::read_to_string(dir.join("cgroup")).unwrap_or_default();
        if cgroup.contains("libpod") || cgroup.contains("podman") || cgroup.contains("docker") {
            continue; // a sandbox's own
        }
        let name = std::fs::read_to_string(dir.join("comm")).unwrap_or_default().trim().to_string();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// The sites a connector's credentials are used with from the sandbox: its service's (by a word of the folder's name)
/// and the addresses written in the agent's files that use them. Registries left out.
fn needed_sites(config: &Config, name: &str, files: &[PathBuf]) -> Vec<String> {
    let lower = name.to_lowercase();
    let mut sites: Vec<String> = Vec::new();
    for (word, hosts) in SERVICE_SITES {
        if lower.contains(word) {
            sites.extend(hosts.iter().map(|h| h.to_string()));
        }
    }
    static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"https?://([A-Za-z0-9][A-Za-z0-9.-]{0,250}\.[A-Za-z]{2,24})").expect("valid regex")
    });
    for file in files {
        let Ok(mut f) = sandbox::open_plain(file) else { continue };
        let mut text = String::new();
        if std::io::Read::read_to_string(&mut std::io::Read::take(&mut f, SEARCHED_MAX), &mut text).is_err() {
            continue;
        }
        for found in URL.captures_iter(&text) {
            if let Ok(host) = crate::sandbox_sites::clean_host(&found[1], false) {
                if !sites.contains(&host) && sites.len() < 20 {
                    sites.push(host);
                }
            }
        }
    }
    let registries = crate::sandbox_sites::registries(config);
    sites.retain(|s| !registries.contains(s));
    sites.dedup();
    sites
}

/// `--link <path>` names this mount (its folder on this machine, or its path in the sandbox).
fn asked_link(hints: &Hints, host: &Path, target: &str) -> bool {
    hints.link.iter().any(|given| {
        let given = given.trim().trim_end_matches('/');
        let path = PathBuf::from(if given.starts_with('~') { py::expanduser(given) } else { given.to_string() });
        canonical(&path) == canonical(host) || given == target.trim_end_matches('/')
    })
}

/// Why programs outside the sandbox keep writing this folder (empty: nothing says so).
fn host_writers(folder: &Path, host: &Path, hints: &Hints) -> Vec<String> {
    let mut why = Vec::new();
    let users = mcp_writers(folder, host);
    if !users.is_empty() {
        why.push(format!("the MCP server « {} » uses it, outside the sandbox", users.join(" », « ")));
    }
    if let Some(since) = hints.last_run {
        let n = changed_since(host, since);
        if n > 0 {
            why.push(format!("{n} file(s) changed since its sandbox last ran"));
        }
    }
    let open = open_by_host(host);
    if !open.is_empty() {
        why.push(format!("open in {} on this machine", open.join(", ")));
    }
    why
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
    plan_with(config, agent, &Hints::default())
}

/// `plan`, with what the person or this machine said (`--link`, the sandbox's last run).
pub fn plan_with(config: &Config, agent: &AgentConfig, hints: &Hints) -> anyhow::Result<Plan> {
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
            sites: Vec::new(),
        };
        let asked = asked_link(hints, &host, &target);
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
        } else if let Some(archive) = crate::archives::covering(config, &agent.name, &host) {
            // a folder its archive reads where it is (archives/): no copy that would go stale
            item.action = Action::Drop;
            item.why = format!("reached through archive search: « {} »", archive.name);
        } else if own_roots.iter().any(|r| inside(&host, r)) || (!read_only && inside(&host, &person_home)) || asked {
            // written by programs outside the sandbox too: linked, not copied once
            let writers = if read_only || asked { Vec::new() } else { host_writers(&folder, &host, hints) };
            item.action = if asked || !writers.is_empty() {
                Action::Link
            } else if read_only {
                Action::Code
            } else {
                Action::Work
            };
            if item.action == Action::Link {
                item.why = if asked { "you asked for it (--link)".to_string() } else { writers.join("; ") };
            }
            item.volume = Some(volume_name(&agent.name, &host, &taken));
            taken.push(item.volume.clone().unwrap_or_default());
            item.tally = tally(&host)?;
        } else {
            item.why = "a folder outside the agent's own: not recognised".into();
        }
        items.push(item);
    }
    // credentials: linked when one of the agent's own files names them (they were kept before: the agent not isolated)
    // (its folders moved now, and those an earlier adopt moved: their volumes, where the agent works since)
    let shown = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut roots: Vec<(PathBuf, String)> = items
        .iter()
        .filter(|i| matches!(i.action, Action::Code | Action::Work | Action::Link))
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
        let found = naming_found(&roots, &[name.clone(), item.target.clone()]);
        let named: Vec<String> = found.iter().map(|(s, _)| s.clone()).collect();
        let users = mcp_users(&folder, &item.host);
        let asked = asked_link(hints, &item.host, &item.target);
        if (!named.is_empty() || asked) && item.host.is_dir() {
            item.action = Action::Link;
            let writers = if users.is_empty() {
                "whatever refreshes them on this machine keeps doing so".to_string()
            } else {
                format!("the MCP server « {} » keeps refreshing them on this machine", users.join(" », « "))
            };
            item.why = if named.is_empty() {
                format!("you asked for it (--link); {writers}")
            } else {
                format!("a connector's credentials that the agent's own files use ({}); {writers}", named.join(", "))
            };
            item.volume = Some(volume_name(&agent.name, &item.host, &taken));
            taken.push(item.volume.clone().unwrap_or_default());
            item.tally = tally(&item.host)?;
            if !named.is_empty() {
                let files: Vec<PathBuf> = found.into_iter().map(|(_, p)| p).collect();
                item.sites = needed_sites(config, &name, &files);
            }
        } else if !named.is_empty() {
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
                Action::Link => Some(format!(
                    "{}:{}{}",
                    item.volume.clone().unwrap_or_default(),
                    item.target,
                    if item.read_only { ":ro" } else { "" }
                )),
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

/// A working folder this big is better connected as an archive (searched where it is) than copied once.
pub const ARCHIVE_HINT_FILES: usize = 500;
pub const ARCHIVE_HINT_BYTES: u64 = 200 * 1024 * 1024;

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
            Action::Link => format!(
                "  link  {host} -> volume {volume} at {}{} (programs outside the sandbox keep writing it: {}; copied \
                 and checked, {}; the folder renamed {}{BACKUP_SUFFIX} as a backup, and a link to the volume left \
                 at its path)",
                item.target,
                if item.read_only { " (read-only)" } else { "" },
                item.why,
                counted(&item.tally),
                host
            ),
            Action::Drop => format!("  drop  {host} ({})", item.why),
            Action::Keep => format!("  keep  {host} ({})", item.why),
        });
        if item.action == Action::Work
            && (item.tally.files >= ARCHIVE_HINT_FILES || item.tally.bytes >= ARCHIVE_HINT_BYTES)
        {
            lines.push(format!(
                "        hint: if something outside the sandbox keeps filling this folder, a copy goes stale: connect it \
                 as an archive instead (searched where it is, the mount then dropped): bipagents archive add {} {host}",
                plan.agent
            ));
        }
        if !item.sites.is_empty() {
            lines.push(format!(
                "        the agent's own files use them from the sandbox: it will need these sites allowed: {} \
                 (--allow-sites allows them now; else it asks you when it tries)",
                item.sites.join(", ")
            ));
        }
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
    apply_with(config, agent, run, &Hints::default(), false).await
}

/// `apply`, with the person's `--link`s, and `--allow-sites`: the sites the linked credentials need allowed now.
pub async fn apply_with(
    config: &Config,
    agent: &AgentConfig,
    run: &Run,
    hints: &Hints,
    allow_sites: bool,
) -> anyhow::Result<Vec<String>> {
    let plan = plan_with(config, agent, hints)?;
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
            check_copy(&host, &target)?;
            Ok(format!("copied and checked: {}", counted(&tally(&target)?)))
        })
        .await??;
        lines.push(format!("  {name}: {message}"));
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, folder = %item.host.display(), volume = %name, path = %item.target, kind = if item.action == Action::Code { "code" } else { "work" }, "hand-made mount replaced by a volume");
        volumes.push(json!({"name": name, "host": item.host.to_string_lossy(), "target": item.target,
                            "kind": if item.action == Action::Code { "code" } else { "work" },
                            "mountpoint": mountpoint.to_string_lossy()}));
    }
    for item in plan.items.iter().filter(|i| i.action == Action::Link) {
        let name = item.volume.clone().unwrap_or_default();
        let mountpoint = sandbox::mountpoint(run, &podman, &name).await?;
        let (host, target) = (item.host.clone(), mountpoint.clone());
        let message = tokio::task::spawn_blocking(move || link_folder(&host, &target)).await??;
        lines.push(format!("  {name}: {message}"));
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, folder = %item.host.display(), volume = %name, path = %item.target, reason = %item.why, "hand-made mount replaced by a volume, the folder linked to it");
        volumes.push(json!({"name": name, "host": item.host.to_string_lossy(), "target": item.target, "kind": "link",
                            "mountpoint": mountpoint.to_string_lossy(),
                            "backup": backup_of(&item.host).to_string_lossy()}));
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
    let linked: Vec<String> = plan
        .items
        .iter()
        .filter(|i| i.action == Action::Link)
        .map(|i| format!("{} (backup: {})", i.host.display(), backup_of(&i.host).display()))
        .collect();
    if !linked.is_empty() {
        lines.push(format!(
            "linked to their volumes, the programs of this machine keep writing them there: {}",
            linked.join(", ")
        ));
    }
    let mut sites: Vec<String> = Vec::new();
    for item in &plan.items {
        for site in &item.sites {
            if !sites.contains(site) {
                sites.push(site.clone());
            }
        }
    }
    if !sites.is_empty() {
        if allow_sites {
            let mut allowed = Vec::new();
            for site in &sites {
                match crate::sandbox_sites::add(config, &agent.name, site, "adopt") {
                    Ok(_) => allowed.push(site.clone()),
                    Err(why) => lines.push(format!("  {site}: not allowed ({why})")),
                }
            }
            lines.push(format!(
                "sites allowed for its programs (you see and change them in the app, « Sites autorisés »): {}",
                allowed.join(", ")
            ));
        } else {
            lines.push(format!(
                "sites its programs will need, not allowed: {} (it asks you in the app when it tries; or run again \
                 with --allow-sites)",
                sites.join(", ")
            ));
        }
    }
    lines.push(format!("undo: bipbridge sandbox adopt --agent {} --undo", agent.name));
    Ok(lines)
}

/// The copy checked: as many files, bytes and links as the folder.
fn check_copy(host: &Path, target: &Path) -> anyhow::Result<()> {
    let (wanted, copied) = (tally(host)?, tally(target)?);
    if (wanted.files, wanted.bytes, wanted.links) != (copied.files, copied.bytes, copied.links) {
        anyhow::bail!(
            "the copy of {} is not whole ({} in the folder, {} in the volume): nothing changed",
            host.display(),
            counted(&wanted),
            counted(&copied)
        );
    }
    Ok(())
}

/// `<folder>.before-adopt`, beside the folder.
pub fn backup_of(host: &Path) -> PathBuf {
    let name = host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    host.with_file_name(format!("{name}{BACKUP_SUFFIX}"))
}

/// A folder moved into its volume and linked: copied, checked, renamed as a backup, what was written meanwhile
/// copied too, the link left at its path. On a failure after the rename, the folder is put back.
pub fn link_folder(host: &Path, volume: &Path) -> anyhow::Result<String> {
    let backup = backup_of(host);
    if let Ok(meta) = host.symlink_metadata() {
        if meta.file_type().is_symlink() {
            if std::fs::read_link(host).ok().as_deref() == Some(volume) {
                return Ok("linked already".into());
            }
            anyhow::bail!("{} is a link already, not to its volume: nothing changed", host.display());
        }
    }
    if backup.symlink_metadata().is_ok() {
        anyhow::bail!("{} exists already: move it away first; nothing changed", backup.display());
    }
    let present = tally(volume)?;
    // empty: the first time; else adopted before and undone: the folder holds the truth since
    mirror(host, volume, present.files + present.links > 0)?;
    check_copy(host, volume)?;
    std::fs::rename(host, &backup)?;
    let switched = (|| -> std::io::Result<()> {
        mirror(&backup, volume, false)?; // what was written meanwhile
        std::os::unix::fs::symlink(volume, host)
    })();
    if let Err(exc) = switched {
        let _ = std::fs::remove_file(host);
        let _ = std::fs::rename(&backup, host);
        anyhow::bail!("{} not linked ({exc}): the folder is back as it was", host.display());
    }
    let tended = open_modes(volume);
    let mut said = format!(
        "copied and checked: {}; the folder renamed {} and linked to the volume",
        counted(&tally(volume)?),
        backup.display()
    );
    if tended.removed_links > 0 {
        said.push_str(&format!(
            "; {} link(s) leading out of the folder left out (still in the backup)",
            tended.removed_links
        ));
    }
    Ok(said)
}

/// What `open_modes` did.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Tended {
    pub opened: usize,
    pub removed_links: usize,
    /// Entries of a sandbox's user, not open to this account.
    pub closed: usize,
}

/// `path` lexically inside `root` (no `..` out of it), for a link's target.
fn leads_inside(root: &Path, link: &Path, to: &Path) -> bool {
    let mut path = if to.is_absolute() { PathBuf::new() } else { link.parent().unwrap_or(root).to_path_buf() };
    for part in to.components() {
        match part {
            std::path::Component::ParentDir => {
                path.pop();
            }
            std::path::Component::CurDir => {}
            other => path.push(other.as_os_str()),
        }
    }
    path.starts_with(root)
}

/// A linked volume kept open to the sandbox's user and this account (folders 0777, files 0666, executables 0777, for
/// what this account owns), and without a link leading out of it (programs of this machine write through it).
pub fn open_modes(root: &Path) -> Tended {
    let mut tended = Tended::default();
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = walk(root) else { return tended };
    for (relative, entry) in &entries {
        let path = root.join(relative);
        match entry {
            Entry::Link(to) => {
                if !leads_inside(root, &path, to) && std::fs::remove_file(&path).is_ok() {
                    tracing::warn!(target: "bipbridge::sandbox", link = %path.display(), "a link leading out of a linked volume removed");
                    tended.removed_links += 1;
                }
            }
            Entry::Dir(mode) | Entry::File { mode, .. } => {
                let Ok(meta) = path.symlink_metadata() else { continue };
                let wanted = match entry {
                    Entry::Dir(_) => LINK_DIR_MODE,
                    _ if mode & 0o111 != 0 => LINK_DIR_MODE,
                    _ => LINK_FILE_MODE,
                };
                if meta.uid() != uid {
                    if mode & 0o006 != wanted & 0o006 {
                        tended.closed += 1;
                    }
                    continue;
                }
                if mode & 0o777 != wanted
                    && std::fs::set_permissions(&path, std::fs::Permissions::from_mode(wanted)).is_ok()
                {
                    tended.opened += 1;
                }
            }
            Entry::Other => {}
        }
    }
    if root.symlink_metadata().is_ok_and(|m| m.uid() == uid) {
        let _ = std::fs::set_permissions(root, std::fs::Permissions::from_mode(LINK_DIR_MODE));
    }
    tended
}

/// Every adopted agent's linked volumes kept open and without a link leading out (every few minutes, sandbox_sites.rs
/// `watch`); what a sandbox's user made, closed to this account, opened as the sandboxes' « root » (`podman unshare`).
pub fn tend_links(config: &Config) {
    let state = sandbox::load_state(config);
    let Some(Value::Object(all)) = state.get("adopted") else { return };
    for (agent, entry) in all {
        for volume in entry.get("volumes").and_then(Value::as_array).into_iter().flatten() {
            if volume.get("kind").and_then(Value::as_str) != Some("link") {
                continue;
            }
            let mountpoint = PathBuf::from(py::py_str(volume.get("mountpoint").unwrap_or(&Value::Null)));
            if !mountpoint.is_dir() {
                continue;
            }
            let tended = open_modes(&mountpoint);
            if tended.closed > 0 {
                if let Some(podman) = sandbox::engine(config) {
                    let done = std::process::Command::new(podman)
                        .args(["unshare", "chmod", "-R", "a+rwX"])
                        .arg(&mountpoint)
                        .stdin(std::process::Stdio::null())
                        .output();
                    if !done.is_ok_and(|o| o.status.success()) {
                        tracing::warn!(target: "bipbridge::sandbox", agent, volume = %mountpoint.display(), "files of the sandbox's user in a linked volume could not be opened to this machine's programs");
                    }
                }
            }
            let host = PathBuf::from(py::py_str(volume.get("host").unwrap_or(&Value::Null)));
            if !host.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
                tracing::warn!(target: "bipbridge::sandbox", agent, folder = %host.display(), "a linked folder is not a link any more: the programs of this machine no longer write in the agent's volume");
            }
        }
    }
}

/// A linked folder put back: the link removed, the volume's latest content copied into the backup, the backup
/// renamed to the folder. Returns what to print.
pub fn unlink_folder(config: &Config, host: &Path, volume: &Path, backup: &Path) -> anyhow::Result<String> {
    match host.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => std::fs::remove_file(host)?,
        Ok(_) => return Ok(format!("{} is a folder again already: left as it is", host.display())),
        Err(_) => {}
    }
    if !backup.is_dir() {
        std::fs::create_dir_all(backup)?;
    }
    let copied = match mirror(volume, backup, true) {
        Err(exc) if exc.kind() == std::io::ErrorKind::PermissionDenied => {
            if let Some(podman) = sandbox::engine(config) {
                let _ = std::process::Command::new(podman)
                    .args(["unshare", "chmod", "-R", "a+rwX"])
                    .arg(volume)
                    .stdin(std::process::Stdio::null())
                    .output();
            }
            mirror(volume, backup, true)
        }
        other => other,
    };
    if let Err(exc) = copied {
        let _ = std::os::unix::fs::symlink(volume, host);
        anyhow::bail!("the volume's content not copied back into {} ({exc}): the link stays", backup.display());
    }
    // the volume's open modes taken back (others never write in a folder of this machine)
    let uid = unsafe { libc::getuid() };
    for (relative, entry) in walk(backup).unwrap_or_default() {
        if let Entry::Dir(mode) | Entry::File { mode, .. } = entry {
            let path = backup.join(relative);
            if path.symlink_metadata().is_ok_and(|m| m.uid() == uid) && mode & 0o022 != 0 {
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & !0o022));
            }
        }
    }
    std::fs::rename(backup, host)?;
    Ok(format!("{} is a folder again, with the volume's latest content", host.display()))
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
    // the linked folders first: the hand-made mounts name them
    let mut restored: Vec<String> = Vec::new();
    for volume in entry.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        if volume.get("kind").and_then(Value::as_str) != Some("link") {
            continue;
        }
        let path = |key: &str| PathBuf::from(py::py_str(volume.get(key).unwrap_or(&Value::Null)));
        let (host, mountpoint) = (path("host"), path("mountpoint"));
        let backup = if volume.get("backup").is_some_and(Value::is_string) { path("backup") } else { backup_of(&host) };
        let c = config.clone();
        let said = tokio::task::spawn_blocking(move || unlink_folder(&c, &host, &mountpoint, &backup)).await??;
        tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, said = %said, "linked folder put back");
        restored.push(format!("  {said}"));
    }
    set_volumes(config, agent, &folder, &before, run).await?;
    let name = agent.name.clone();
    sandbox::save_state(config, |state| {
        if let Some(Value::Object(all)) = state.get_mut("adopted") {
            all.remove(&name);
        }
    })?;
    tracing::info!(target: "bipbridge::sandbox", agent = %agent.name, mounts = %before.join(" "), "adopt undone: the hand-made mounts are back");
    let mut lines = vec![format!("{}: mounts as before: {}", agent.name, before.join(" "))];
    lines.extend(restored);
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

/// When the agent's sandbox last stopped (podman's containers of its profile), None while one runs or when unknown.
pub async fn last_run(config: &Config, agent: &str, run: &Run) -> Option<i64> {
    let podman = sandbox::engine(config)?;
    let out = run(
        args(&[
            &podman,
            "ps",
            "-a",
            "--filter",
            "label=hermes-agent=1",
            "--filter",
            &format!("label=hermes-profile={agent}"),
            "--format",
            "{{.State}} {{.ExitedAt}}",
        ]),
        BTreeMap::new(),
    )
    .await
    .ok()?;
    let mut last: Option<i64> = None;
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(state), Some(at)) = (parts.next(), parts.next()) else { continue };
        if state.eq_ignore_ascii_case("running") {
            return None;
        }
        if let Ok(at) = at.parse::<i64>() {
            if at > 0 {
                last = Some(last.map_or(at, |l| l.max(at)));
            }
        }
    }
    last
}

// --- the command line ----------------------------------------------------------------------------------------------

/// `bipbridge sandbox adopt …` (argv after `adopt`).
pub fn cli(argv: &[String]) -> i32 {
    let (mut config_path, mut agent_name) = (None, None);
    let (mut dry_run, mut undoing, mut allow_sites) = (false, false, false);
    let mut hints = Hints::default();
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
            "--link" if i + 1 < argv.len() => {
                hints.link.push(argv[i + 1].clone());
                i += 1;
            }
            _ if arg.starts_with("--link=") => hints.link.push(arg["--link=".len()..].to_string()),
            "--allow-sites" => allow_sites = true,
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
    if !undoing {
        hints.last_run = runtime.block_on(last_run(&config, &agent.name, &default_run()));
    }
    let result = if dry_run {
        plan_with(&config, &agent, &hints).map(|p| {
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
        runtime.block_on(apply_with(&config, &agent, &default_run(), &hints, allow_sites))
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
    fn a_folder_linked_then_put_back() {
        let dir = tempfile::tempdir().unwrap();
        let (host, volume) = (dir.path().join("data"), dir.path().join("volume"));
        std::fs::create_dir_all(host.join("2026")).unwrap();
        std::fs::write(host.join("garmin.db"), b"sqlite").unwrap();
        std::fs::write(host.join("2026/sleep.json"), b"{}").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", host.join("out")).unwrap();
        std::os::unix::fs::symlink("garmin.db", host.join("latest.db")).unwrap();
        std::fs::create_dir_all(&volume).unwrap();
        let said = link_folder(&host, &volume).unwrap();
        assert!(said.contains("1 link(s) leading out"), "{said}");
        assert_eq!(std::fs::read_link(&host).unwrap(), volume);
        assert!(backup_of(&host).join("out").symlink_metadata().is_ok()); // the backup keeps everything
        assert!(volume.join("out").symlink_metadata().is_err() && volume.join("latest.db").symlink_metadata().is_ok());
        assert_eq!(std::fs::metadata(volume.join("garmin.db")).unwrap().mode() & 0o777, LINK_FILE_MODE);
        assert_eq!(std::fs::metadata(volume.join("2026")).unwrap().mode() & 0o777, LINK_DIR_MODE);
        // a program of this machine writes through the link; the sandbox writes in the volume
        std::fs::write(host.join("garmin.db"), b"sqlite, later").unwrap();
        std::fs::write(volume.join("report.md"), b"by the agent").unwrap();
        assert_eq!(link_folder(&host, &volume).unwrap(), "linked already");
        // the sandbox plants a link to this machine's files: taken away
        std::os::unix::fs::symlink("../../../../etc/hosts", volume.join("2026/evil")).unwrap();
        assert_eq!(open_modes(&volume).removed_links, 1);
        let (dir2, config) = test_config();
        let said = unlink_folder(&config, &host, &volume, &backup_of(&host)).unwrap();
        drop(dir2);
        assert!(said.contains("a folder again"), "{said}");
        assert!(host.symlink_metadata().unwrap().is_dir() && !backup_of(&host).exists());
        assert_eq!(std::fs::read(host.join("garmin.db")).unwrap(), b"sqlite, later");
        assert_eq!(std::fs::read(host.join("report.md")).unwrap(), b"by the agent");
        assert_eq!(std::fs::metadata(host.join("garmin.db")).unwrap().mode() & 0o022, 0);
    }

    fn test_config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        (dir, config)
    }

    #[test]
    fn a_link_never_replaces_what_it_does_not_know() {
        let dir = tempfile::tempdir().unwrap();
        let (host, volume) = (dir.path().join("data"), dir.path().join("volume"));
        std::fs::create_dir_all(&host).unwrap();
        std::fs::create_dir_all(&volume).unwrap();
        std::fs::create_dir_all(backup_of(&host)).unwrap();
        assert!(link_folder(&host, &volume).unwrap_err().to_string().contains("exists already"));
        assert!(host.is_dir());
    }

    #[test]
    fn whole_paths_only() {
        assert!(names_path(r#"{"env": {"DB": "/h/wellness/data/garmin.db"}}"#, "/h/wellness/data"));
        assert!(names_path(r#"["/h/wellness/data"]"#, "/h/wellness/data"));
        assert!(!names_path(r#"{"env": {"DB": "/h/wellness/database"}}"#, "/h/wellness/data"));
    }

    #[test]
    fn the_sites_a_connector_needs() {
        let (_dir, config) = test_config();
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("oura.py");
        std::fs::write(
            &script,
            b"requests.get('https://api.ouraring.com/v2/usercollection/sleep')\n\
            pip('https://pypi.org/simple')\nx = 'http://localhost:8080'\n",
        )
        .unwrap();
        let sites = needed_sites(&config, ".oura-mcp", &[script]);
        assert_eq!(sites, vec!["api.ouraring.com".to_string(), "cloud.ouraring.com".to_string()]);
        assert!(needed_sites(&config, ".garmin-mcp", &[]).contains(&"connectapi.garmin.com".to_string()));
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
