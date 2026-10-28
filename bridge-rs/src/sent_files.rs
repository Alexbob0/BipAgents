//! Files an agent hands the person with a `MEDIA:<path>` line from its own places (owner's report 2026-10-22: since
//! the sandboxes are isolated, Wellness' « infographie » showed « Fichier indisponible »: a sandbox writes in its
//! `/envoi` volume or its workspace, and none of them is a declared `[media] roots`).
//!
//! The path is the agent's own: `/envoi/courses.xlsx`, `/workspace/out/graph.png`, `/root/notes.md`,
//! `/fichiers/2026-10/…`, or a relative `graph.png` (its sandbox's working folder: `/root`, then `/workspace`). Its
//! places, for that agent only (never another agent's):
//!
//! - `/envoi`: its isolated sandbox's outbox volume (`<data>/sandbox.json`), or `<data>/envoi/<agent>` for an agent
//!   set up by hand (agent_files.rs);
//! - `/partage` and `/lecture/<agent>`: its shared folder and the ones the person lets it read (shares.rs
//!   `share_dir`: their podman volumes, or their folders on this machine);
//! - `/workspace` and `/root`: Hermes' persistent docker sandbox folders under its profile,
//!   `<profile>/sandboxes/docker/<task>/workspace` and `…/home` (tools/environments/docker.py at the pinned commit:
//!   `container_persistent`, Hermes' default, which nothing of ours changes);
//! - its own Hermes profile folder, by its path on this machine (`/home/alex/.hermes/profiles/wellness/data/x.png`:
//!   owner's report 2026-10-22, an agent whose scripts write there);
//! - the folders mounted by hand into its sandbox (`terminal.docker_volumes`, `host:container`), by either path; never
//!   one inside another agent's profile or the bridge's own data; a podman volume named for the agent
//!   (`bipagents-<agent>-data:/home/hermes/.hermes/profiles/<agent>/data`) by its path in the sandbox;
//! - the files it was sent (its uploads, as it sees them: `/fichiers/…` in a sandbox);
//! - anything else (`/tmp/infographie.png`: a tmpfs of the container) is copied out of its Hermes sandbox with
//!   `podman cp <container>:<path> -`: a container labelled `hermes-agent=1` and `hermes-profile=<agent>` by Hermes,
//!   and only when its mounts are this agent's (its `/root` or `/workspace` under the agent's profile, or its
//!   `/envoi`). The tar stream is read here: one regular file under the size limit, never a link, a device, a pipe
//!   or a folder.
//!
//! Hidden and secret-looking files are never served from any of them (`.env`, `auth.json`, `*.pem`, `*.key`,
//! `credentials*`, `token*`, `config.yaml`, anything under a dot-folder or a `vault`): with .json/.txt/.md servable,
//! a `MEDIA:` line must not bring a profile's secrets into the conversation.
//!
//! The bridge copies the file into a store of its own when the run that names it ends (runs.rs) or when the app first
//! asks for it (`GET /v1/media?path=…&agent=…`): `<data>/sent-files/<agent>/<16 hex of the path>/<name>`, kept as
//! long as the person's files (files_kept.rs), and serves that copy, so the file stays when the sandbox is reset. A
//! newer file at the same path replaces the copy. Each path is opened one name at a time without following a link
//! (`agent_files::open_beneath`): a link made in a sandbox never leads to this machine's files.
//!
//! No Python counterpart.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::agent_files::{open_beneath, under};
use crate::config::{AgentConfig, Config};
use crate::sandbox::{plain_relative, OUTBOX};

/// Where Hermes' persistent docker sandboxes keep their folders, under a profile.
const SANDBOXES: &str = "sandboxes/docker";

/// How long one podman command may take.
const PODMAN_TIMEOUT: Duration = Duration::from_secs(60);

pub fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("sent-files")
}

/// `<root>/<agent>/<16 hex of the path>/<name>`: where the copy of `path` lives (None for a path with no plain name).
fn copy_path(config: &Config, agent: &str, path: &str) -> Option<PathBuf> {
    let name = path.rsplit('/').next().filter(|n| plain_relative(n).is_some_and(|r| r.components().count() == 1))?;
    let key = hex::encode(Sha256::digest(path.as_bytes()))[..16].to_string();
    plain_relative(agent).filter(|r| r.components().count() == 1)?;
    Some(root(config).join(agent).join(key).join(name))
}

/// A hidden or secret-looking file (or one under a hidden folder, or a vault): never handed out.
pub(crate) fn secret_looking(relative: &Path) -> bool {
    let parts: Vec<String> = relative.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    let Some(name) = parts.last() else { return true };
    if parts.iter().any(|p| p.starts_with('.') || p == "vault" || p == "secrets") {
        return true;
    }
    let stem = name.split('.').next().unwrap_or_default();
    ["credential", "credentials", "token", "tokens", "secret", "auth", "id_rsa", "id_ed25519", "id_ecdsa"]
        .contains(&stem)
        || name.starts_with("credentials")
        || name.starts_with("token")
        || [".key", ".pem", ".p12", ".pfx", ".keystore", ".kdbx"].iter().any(|e| name.ends_with(e))
        || ["config.yaml", "config.yml", "config.toml", "config.json"].contains(&name.as_str())
}

/// The agent's own mounts (`host:container[:options]` in its profile's `terminal.docker_volumes`): (host folder, path
/// in the sandbox, whether a named volume). A volume counts when named for this agent (`bipagents-<agent>-…`, its
/// folder from podman); the whole machine, folders of another agent's profile or of the bridge's own data never do.
fn hand_mounts(config: &Config, agent: &AgentConfig, profile: &Path) -> Vec<(PathBuf, String, bool)> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut forbidden: Vec<PathBuf> = config
        .agents
        .values()
        .filter(|other| other.name != agent.name)
        .filter_map(|other| crate::shares::profile_dir(config, other))
        .map(|p| canonical(&p))
        .collect();
    forbidden.push(canonical(Path::new(&config.data_dir())));
    let mut out = Vec::new();
    for spec in crate::shares::docker_volumes(profile) {
        let mut parts = spec.split(':');
        let (Some(host), Some(seen)) = (parts.next(), parts.next()) else { continue };
        let seen = seen.trim_end_matches('/');
        // the /envoi volume (known from sandbox.json), `/`, a path going up
        if !seen.starts_with('/')
            || seen == OUTBOX
            || plain_relative(seen).is_none()
            || seen.split('/').any(|p| p == "..")
        {
            continue;
        }
        let (host, named) = if host.starts_with('/') {
            if plain_relative(host).is_none() || host.split('/').any(|p| p == "..") {
                continue;
            }
            (PathBuf::from(host), false)
        } else if own_volume(config, agent, host) {
            let Some(mountpoint) = volume_mountpoint(host) else { continue };
            (mountpoint, true)
        } else {
            continue; // another agent's volume, or one the bridge didn't name
        };
        let real = canonical(&host);
        if forbidden.iter().any(|f| real.starts_with(f) || f.starts_with(&real)) {
            continue;
        }
        out.push((host, seen.to_string(), named));
    }
    out
}

/// A podman volume named for this agent (`bipagents-<agent>-<what>`), and not for another agent whose name starts
/// like this one's (`bipagents-vie-pro-…` is never `vie`'s).
pub(crate) fn own_volume(config: &Config, agent: &AgentConfig, volume: &str) -> bool {
    let named_for = |name: &str| {
        volume.strip_prefix(&format!("bipagents-{name}-")).is_some_and(|rest| {
            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
        })
    };
    named_for(&agent.name)
        && !config
            .agents
            .values()
            .any(|o| o.name.len() > agent.name.len() && o.name != agent.name && named_for(&o.name))
}

/// A podman volume's folder on this machine (`podman volume inspect --format {{.Mountpoint}}`).
pub(crate) fn volume_mountpoint(volume: &str) -> Option<PathBuf> {
    let podman = crate::browsers::pyx::which("podman")?;
    let out =
        run(&[podman, "volume".into(), "inspect".into(), "--format".into(), "{{.Mountpoint}}".into(), volume.into()])
            .ok()?;
    let text = String::from_utf8(out).ok()?;
    let path = text.trim();
    (path.starts_with('/') && plain_relative(path).is_some()).then(|| PathBuf::from(path))
}

/// The agent's places: (prefix as it sees it, folders on this machine), the longest prefix first.
fn places(config: &Config, agent: &AgentConfig) -> Vec<(String, Vec<PathBuf>)> {
    let mut out: Vec<(String, Vec<PathBuf>)> = Vec::new();
    let isolated = crate::sandbox::volumes_of(config, &agent.name);
    if let Some((_, outbox)) = &isolated {
        out.push((OUTBOX.into(), vec![outbox.clone()]));
    } else if agent.shares == "external" {
        out.push((OUTBOX.into(), vec![crate::agent_files::external_outbox(config, &agent.name)]));
    }
    if let Some(profile) = crate::shares::profile_dir(config, agent) {
        // One folder per sandbox Hermes keyed (`profile:<name>`, `default`…): all of them this agent's.
        let tasks: Vec<PathBuf> = std::fs::read_dir(profile.join(SANDBOXES))
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        out.push(("/workspace".into(), tasks.iter().map(|t| t.join("workspace")).collect()));
        out.push(("/root".into(), tasks.iter().map(|t| t.join("home")).collect()));
        for (host, seen, named) in hand_mounts(config, agent, &profile) {
            out.push((seen, vec![host.clone()]));
            if !named {
                out.push((host.to_string_lossy().trim_end_matches('/').to_string(), vec![host]));
            }
        }
        let mut names = vec![profile.to_string_lossy().into_owned()];
        if let Ok(real) = std::fs::canonicalize(&profile) {
            names.push(real.to_string_lossy().into_owned());
        }
        names.dedup();
        for name in names {
            out.push((name.trim_end_matches('/').to_string(), vec![profile.clone()]));
        }
    }
    if crate::shares::why_not(config, agent).is_none() {
        // Its shared folder, and the ones it may read (a podman volume each, or a folder of this machine: shares.rs).
        out.push(("/partage".into(), vec![crate::shares::share_dir(config, &agent.name)]));
        for (_, source) in crate::shares::load(config).iter().filter(|(reader, _)| reader == &agent.name) {
            if plain_relative(source).is_some_and(|r| r.components().count() == 1) {
                out.push((format!("/lecture/{source}"), vec![crate::shares::share_dir(config, source)]));
            }
        }
    }
    let host = agent.upload_dir_host.as_deref().filter(|h| !h.is_empty());
    if let Some(host) = host {
        let seen = agent.upload_dir_container.as_deref().filter(|c| !c.is_empty()).unwrap_or(host);
        let mut folders = vec![PathBuf::from(host)];
        if let Some((files, _)) = &isolated {
            folders.push(files.clone()); // the shared files it fetched (/fichiers/partages/…)
        }
        out.push((seen.trim_end_matches('/').to_string(), folders));
    }
    out.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
    out
}

/// The folder of this machine behind a folder `path` as the agent sees it in its sandbox (`/workspace/notes`, a
/// mounted folder, its outbox…): the newest when several of its sandboxes have one. None: not one of its places, or no
/// such folder (or a link on the way). Its real path (`canonicalize`), and only when that is still inside the place: a
/// folder the sandbox swapped for a link between the walk and the resolution never leads out (security audit
/// 2026-10-27); the archives then open it only while it still leads there (beneath.rs `open_exact`).
pub(crate) fn sandbox_folder(config: &Config, agent: &AgentConfig, path: &str) -> Option<PathBuf> {
    let path = path.trim().trim_end_matches('/');
    for (prefix, folders) in places(config, agent) {
        let relative = if path == prefix.trim_end_matches('/') {
            PathBuf::new()
        } else {
            match under(path, &prefix) {
                Some(rest) => plain_relative(rest)?,
                None => continue,
            }
        };
        let found = folders
            .iter()
            .filter(|folder| folder.is_dir())
            .filter_map(|folder| {
                // no link on the way beneath the place: each part a real folder
                let mut at = folder.clone();
                for part in relative.components() {
                    at.push(part);
                    if !at.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                        return None;
                    }
                }
                let (real, place) = (std::fs::canonicalize(&at).ok()?, std::fs::canonicalize(folder).ok()?);
                real.starts_with(&place).then_some(real)
            })
            .max_by_key(|candidate| candidate.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH));
        return found;
    }
    None
}

fn modified(file: &std::fs::File) -> SystemTime {
    file.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Why a file could not be had from this machine's folders.
enum NotHere {
    /// Never handed out, from anywhere (a hidden or secret file, a path going up).
    Refused(&'static str),
    /// Not in the agent's places on this machine: its sandbox may still have it.
    Missing(&'static str),
}

/// The agent's file behind `path`, opened (the newest when several sandboxes have one; the longest place first).
fn open_source(config: &Config, agent: &AgentConfig, path: &str) -> Result<std::fs::File, NotHere> {
    let mut why = NotHere::Missing(
        "not under one of the agent's places (/envoi, /workspace, /root, its profile, its mounted folders, its \
         received files)",
    );
    for (prefix, folders) in places(config, agent) {
        let Some(rest) = under(path, &prefix) else { continue };
        let Some(relative) = plain_relative(rest) else { return Err(NotHere::Refused("not a plain path")) };
        if secret_looking(&relative) {
            return Err(NotHere::Refused("a hidden or secret-looking file (never sent)"));
        }
        let newest = folders.iter().filter_map(|folder| open_beneath(folder, &relative).ok()).max_by_key(modified);
        match newest {
            Some(file) => return Ok(file),
            None => why = NotHere::Missing("no such file in the agent's places (or a link)"),
        }
    }
    Err(why)
}

/// The paths the agent may mean: a relative one is in its sandbox's working folder (`/root` by default in Hermes'
/// docker sandbox, `/workspace` when it mounts one).
fn meant(path: &str) -> Vec<String> {
    if path.starts_with('/') {
        vec![path.to_string()]
    } else {
        vec![format!("/root/{path}"), format!("/workspace/{path}")]
    }
}

/// The copy of the agent's file behind `path`, made or refreshed now: Err(why) when there is none.
pub fn capture(config: &Config, agent: &AgentConfig, path: &str) -> Result<PathBuf, String> {
    capture_with(config, agent, path, false)
}

/// `capture`; `fresh`: at the end of a run, the sandbox is asked again even when a copy exists.
fn capture_with(config: &Config, agent: &AgentConfig, path: &str, fresh: bool) -> Result<PathBuf, String> {
    if crate::media::content_type_of(path).is_none() {
        return Err("not a type of file the apps open".into());
    }
    let target = copy_path(config, &agent.name, path).ok_or("not a plain path")?;
    let existing = std::fs::metadata(&target).ok().filter(|m| m.is_file());
    let mut why = "not a plain path";
    let mut source = None;
    for candidate in meant(path) {
        match open_source(config, agent, &candidate) {
            Ok(file) => {
                source = Some(file);
                break;
            }
            Err(NotHere::Refused(reason)) => return Err(reason.into()),
            Err(NotHere::Missing(reason)) => why = reason,
        }
    }
    let Some(source) = source else {
        if existing.is_some() && !fresh {
            return Ok(target); // the sandbox reset since: the copy stays
        }
        let mut sandbox_why = String::from("not tried");
        for candidate in meant(path) {
            match from_sandbox(config, agent, &candidate, &target) {
                Ok(container) => {
                    tracing::info!(
                        target: "bipbridge::media",
                        agent = %agent.name,
                        path = %path,
                        container = %container,
                        "file copied out of the agent's sandbox (podman cp)"
                    );
                    return Ok(target);
                }
                Err(reason) => sandbox_why = reason,
            }
        }
        if existing.is_some() {
            return Ok(target);
        }
        return Err(format!("{why}; its sandbox: {sandbox_why}"));
    };
    let meta = source.metadata().map_err(|e| e.to_string())?;
    if let Some(copy) = &existing {
        let copied = copy.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if copy.len() == meta.len() && meta.modified().unwrap_or(SystemTime::UNIX_EPOCH) <= copied {
            return Ok(target);
        }
    }
    let limit = config.limits.upload_max_bytes;
    if meta.len() > limit {
        return Err(format!("too large ({} bytes, at most {limit})", meta.len()));
    }
    store(&target, |out| {
        let copied = std::io::copy(&mut source.take(limit + 1), out)?;
        if copied > limit {
            return Err(std::io::Error::other("too large"));
        }
        Ok(())
    })
}

/// `target` written through a temporary file beside it (removed on any error).
fn store(target: &Path, write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>) -> Result<PathBuf, String> {
    let folder = target.parent().ok_or("no folder")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let tmp = folder.join(format!(".{}.part", crate::py::token_urlsafe(6)));
    let result = (|| -> std::io::Result<()> {
        let mut out = std::fs::File::create(&tmp)?;
        write(&mut out)?;
        out.flush()?;
        std::fs::rename(&tmp, target)
    })();
    if let Err(exc) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(exc.to_string());
    }
    Ok(target.to_path_buf())
}

// --- out of the agent's sandbox (podman cp) ------------------------------------------------------------------------

/// Hermes' label value for a name (`_sanitize_label_value`): `[A-Za-z0-9_.-]`, at most 63 characters.
fn label_value(name: &str) -> String {
    let value: String =
        name.chars().map(|c| if c.is_ascii_alphanumeric() || "_.-".contains(c) { c } else { '_' }).take(63).collect();
    if value.is_empty() {
        "unknown".into()
    } else {
        value
    }
}

/// The command started, its output piped; a watchdog kills it after PODMAN_TIMEOUT.
fn spawn(command: &[String]) -> Result<std::process::Child, String> {
    let child = std::process::Command::new(&command[0])
        .args(&command[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let pid = child.id() as libc::pid_t;
    std::thread::spawn(move || {
        std::thread::sleep(PODMAN_TIMEOUT);
        // SAFETY: a signal to the child's pid only; reaped long before in the usual case (then ESRCH, or at worst a
        // recycled pid of this user's, which a minute makes unlikely).
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    Ok(child)
}

/// `command`'s output (Err when it fails).
fn run(command: &[String]) -> Result<Vec<u8>, String> {
    let mut child = spawn(command)?;
    let mut out = Vec::new();
    let read = child.stdout.take().map(|mut s| s.read_to_end(&mut out));
    let status = child.wait().map_err(|e| e.to_string())?;
    match read {
        Some(Ok(_)) if status.success() => Ok(out),
        _ => Err(format!("podman {} failed ({status})", command.get(1).map(String::as_str).unwrap_or_default())),
    }
}

/// The agent's own Hermes sandbox containers, the running ones first: labelled for its profile by Hermes, and holding
/// its folders (its `/root` or `/workspace` under its profile, or its `/envoi`). Never another agent's.
fn sandbox_containers(config: &Config, agent: &AgentConfig, podman: &str) -> Result<Vec<String>, String> {
    let profile = crate::shares::profile_dir(config, agent).ok_or("no Hermes profile of the bridge's")?;
    let label = label_value(&agent.name);
    let listed = run(&[
        podman.into(),
        "ps".into(),
        "-a".into(),
        "--filter".into(),
        "label=hermes-agent=1".into(),
        "--filter".into(),
        format!("label=hermes-profile={label}"),
        "--format".into(),
        "json".into(),
    ])?;
    let listed: Value = serde_json::from_slice(&listed).map_err(|_| "podman ps: not JSON")?;
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let sandboxes = canonical(&profile.join(SANDBOXES));
    let outbox = crate::sandbox::volumes_of(config, &agent.name).map(|(_, o)| canonical(&o));
    let mut found: Vec<(bool, String)> = Vec::new();
    for item in listed.as_array().into_iter().flatten() {
        let id = item.get("Id").and_then(Value::as_str).unwrap_or_default();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let Ok(inspected) = run(&[podman.into(), "container".into(), "inspect".into(), id.into()]) else { continue };
        let inspected: Value = serde_json::from_slice(&inspected).unwrap_or(Value::Null);
        let Some(c) = inspected.as_array().and_then(|a| a.first()) else { continue };
        let labels = c.pointer("/Config/Labels");
        let label_of = |key: &str| labels.and_then(|l| l.get(key)).and_then(Value::as_str);
        if label_of("hermes-agent") != Some("1") || label_of("hermes-profile") != Some(label.as_str()) {
            continue;
        }
        let ours = c.get("Mounts").and_then(Value::as_array).into_iter().flatten().any(|m| {
            let source = m.get("Source").and_then(Value::as_str).map(|s| canonical(Path::new(s)));
            match (m.get("Destination").and_then(Value::as_str), source) {
                (Some("/root" | "/workspace"), Some(s)) => s.starts_with(&sandboxes),
                (Some(OUTBOX), Some(s)) => outbox.as_ref() == Some(&s),
                _ => false,
            }
        });
        if ours {
            let running = c.pointer("/State/Running").and_then(Value::as_bool).unwrap_or(false);
            found.push((!running, id.to_string()));
        }
    }
    found.sort_by_key(|(stopped, _)| *stopped);
    Ok(found.into_iter().map(|(_, id)| id).collect())
}

/// The agent's file at `path` in its sandbox, copied to `target` (`podman cp <container>:<path> -`, a tar stream
/// read here). Ok: the container it came from.
fn from_sandbox(config: &Config, agent: &AgentConfig, path: &str, target: &Path) -> Result<String, String> {
    if path.split('/').any(|p| p == "..") {
        return Err("not a plain path".into());
    }
    let relative = plain_relative(path).ok_or("not a plain path")?;
    if secret_looking(&relative) {
        return Err("a hidden or secret-looking file (never sent)".into());
    }
    let podman = crate::browsers::pyx::which("podman").ok_or("no podman")?;
    let containers = sandbox_containers(config, agent, &podman)?;
    if containers.is_empty() {
        return Err("no Hermes sandbox container of this agent's".into());
    }
    let seen = format!("/{}", relative.to_string_lossy());
    let limit = config.limits.upload_max_bytes;
    let mut why = String::new();
    for id in containers {
        let mut child = spawn(&[podman.clone(), "cp".into(), format!("{id}:{seen}"), "-".into()])?;
        let Some(stdout) = child.stdout.take() else { continue };
        let result = store(target, |out| untar_one(stdout, out, limit));
        let _ = child.kill();
        let _ = child.wait();
        let short: String = id.chars().take(12).collect();
        match result {
            Ok(_) => return Ok(short),
            Err(reason) => why = format!("{short}: {reason}"),
        }
    }
    Err(why)
}

/// The one regular file of a tar stream, written to `out`: anything else (a link, a folder, a device, a pipe) is
/// refused, and more than `limit` bytes.
fn untar_one(mut tar: impl Read, out: &mut impl Write, limit: u64) -> std::io::Result<()> {
    let refuse = |why: String| Err(std::io::Error::other(why));
    let mut header = [0u8; 512];
    loop {
        if tar.read_exact(&mut header).is_err() || header.iter().all(|b| *b == 0) {
            return refuse("no such file in the sandbox".into());
        }
        let size_field = &header[124..136];
        if size_field[0] & 0x80 != 0 {
            return refuse("too large".into());
        }
        let text: String = size_field.iter().take_while(|b| **b != 0 && **b != b' ').map(|b| *b as char).collect();
        let size = u64::from_str_radix(text.trim(), 8).map_err(|_| std::io::Error::other("not a tar stream"))?;
        match header[156] {
            // PAX and GNU long-name records: what comes before the entry itself
            b'x' | b'g' | b'L' | b'K' => {
                if size > 1 << 20 {
                    return refuse("not a tar stream".into());
                }
                std::io::copy(&mut (&mut tar).take(size.div_ceil(512) * 512), &mut std::io::sink())?;
            }
            b'0' | 0 | b'7' => {
                if size > limit {
                    return refuse(format!("too large ({size} bytes, at most {limit})"));
                }
                if std::io::copy(&mut (&mut tar).take(size), out)? != size {
                    return refuse("cut short".into());
                }
                return Ok(());
            }
            b'1' | b'2' => return refuse("a link (never followed)".into()),
            b'5' => return refuse("a folder".into()),
            _ => return refuse("not a regular file (a device or a pipe)".into()),
        }
    }
}

/// For an app that names no agent (versions before 2026-10-22): the copy of `path`, when exactly one agent has one.
pub fn find_unique(config: &Config, path: &str) -> Option<PathBuf> {
    let mut found = config.agents.values().filter_map(|a| copy_path(config, &a.name, path)).filter(|p| p.is_file());
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

/// A reply's `MEDIA:` files kept now (at the end of the run), those the bridge serves from elsewhere left alone. A
/// file the person can't get is logged: the agent said « voilà », the app shows « Fichier introuvable sur le serveur ».
pub fn capture_reply(config: &Config, agent: &AgentConfig, text: &str) {
    if !text.contains("MEDIA:") {
        return;
    }
    for path in crate::media::extract_media(text).1 {
        if crate::agent_tools::served_media(config, &path).is_some() {
            continue;
        }
        match capture_with(config, agent, &path, true) {
            Ok(_) => {
                tracing::info!(target: "bipbridge::media", agent = %agent.name, path = %path, "file the agent sent kept")
            }
            Err(why) => tracing::warn!(
                target: "bipbridge::media",
                agent = %agent.name,
                path = %path,
                why = %why,
                "the agent pointed to a file the person can't get (MEDIA: line)"
            ),
        }
    }
}

/// Copies older than `retention_days` removed (by when they were made). Returns how many.
pub fn purge(config: &Config, retention_days: i64, now: Option<f64>) -> usize {
    let now = now.unwrap_or_else(crate::py::time_time);
    let cutoff = now - (retention_days * 86400) as f64;
    let mut removed = 0;
    let Ok(agents) = std::fs::read_dir(root(config)) else { return 0 };
    for agent in agents.flatten() {
        let Ok(keys) = std::fs::read_dir(agent.path()) else { continue };
        for key in keys.flatten() {
            let Ok(files) = std::fs::read_dir(key.path()) else { continue };
            for file in files.flatten() {
                let Ok(meta) = file.path().symlink_metadata() else { continue };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(f64::MAX);
                if meta.is_file() && mtime < cutoff && std::fs::remove_file(file.path()).is_ok() {
                    removed += 1;
                }
            }
            let _ = std::fs::remove_dir(key.path()); // only when empty
        }
        let _ = std::fs::remove_dir(agent.path());
    }
    removed
}

/// A deleted agent: its copies go.
pub fn forget(config: &Config, agent: &str) {
    if plain_relative(agent).is_some_and(|r| r.components().count() == 1) {
        let _ = std::fs::remove_dir_all(root(config).join(agent));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Config, AgentConfig) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        let mut agent = AgentConfig { name: "wellness".into(), ..AgentConfig::default() };
        let uploads = dir.path().join("uploads");
        std::fs::create_dir_all(uploads.join("2026-10")).unwrap();
        agent.upload_dir_host = Some(uploads.to_string_lossy().into_owned());
        agent.upload_dir_container = Some("/fichiers".into());
        agent.shares = "external".into();
        config.agents.insert(agent.name.clone(), agent.clone());
        (dir, config, agent)
    }

    #[test]
    fn copies_from_the_agents_own_places_only() {
        let (dir, config, agent) = setup();
        let outbox = crate::agent_files::external_outbox(&config, "wellness");
        std::fs::create_dir_all(outbox.join("sub")).unwrap();
        std::fs::write(outbox.join("courses.xlsx"), b"PK-xlsx").unwrap();
        std::fs::write(outbox.join("sub/graph.png"), b"PNG").unwrap();
        std::fs::write(dir.path().join("secret.txt"), b"no").unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), outbox.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(dir.path(), outbox.join("up")).unwrap();
        let copy = capture(&config, &agent, "/envoi/courses.xlsx").unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"PK-xlsx");
        assert!(copy.starts_with(root(&config).join("wellness")));
        assert!(capture(&config, &agent, "/envoi/sub/graph.png").is_ok());
        for refused in ["/envoi/link.txt", "/envoi/up/secret.txt", "/envoi/../secret.txt", "/etc/hosts", "/envoi/x.sh"]
        {
            assert!(capture(&config, &agent, refused).is_err(), "{refused}");
        }
        // The sandbox reset: the copy stays; a newer file replaces it.
        std::fs::remove_file(outbox.join("courses.xlsx")).unwrap();
        assert_eq!(std::fs::read(capture(&config, &agent, "/envoi/courses.xlsx").unwrap()).unwrap(), b"PK-xlsx");
        std::fs::write(outbox.join("courses.xlsx"), b"PK-xlsx-2").unwrap();
        assert_eq!(std::fs::read(capture(&config, &agent, "/envoi/courses.xlsx").unwrap()).unwrap(), b"PK-xlsx-2");
        // Another agent never gets it; an app naming no agent does while only one agent has that path.
        let other = AgentConfig { name: "vie".into(), shares: "external".into(), ..AgentConfig::default() };
        assert!(capture(&config, &other, "/envoi/courses.xlsx").is_err());
        assert_eq!(find_unique(&config, "/envoi/courses.xlsx"), Some(copy.clone()));
        // Kept as long as the person's files.
        assert_eq!(purge(&config, 30, None), 0);
        assert_eq!(purge(&config, 30, Some(crate::py::time_time() + 31.0 * 86400.0)), 2);
        assert!(!copy.exists());
    }

    #[test]
    fn secrets_and_hidden_files_never_go() {
        for path in [
            "auth.json",
            ".env",
            "a/.git/x.png",
            "tokens.json",
            "token_cache.json",
            "cle.pem",
            "srv.key",
            "credentials.json",
            "config.yaml",
            "vault/x.pdf",
            "Secret.txt",
        ] {
            assert!(secret_looking(Path::new(path)), "{path}");
        }
        for path in ["runs_infographic_23.png", "progress_photos/semaine.pdf", "notes.md", "plan.json", "graph.svg"] {
            assert!(!secret_looking(Path::new(path)), "{path}");
        }
        assert_eq!(meant("graph.png"), ["/root/graph.png", "/workspace/graph.png"]);
        assert_eq!(label_value("ma vie"), "ma_vie");
    }

    #[test]
    fn the_received_files_and_the_persistent_workspace() {
        let (dir, config, agent) = setup();
        std::fs::write(dir.path().join("uploads/2026-10/0123456789ab-facture.pdf"), b"%PDF").unwrap();
        assert!(capture(&config, &agent, "/fichiers/2026-10/0123456789ab-facture.pdf").is_ok());
        assert!(capture(&config, &agent, "/workspace/x.png").is_err()); // no Hermes profile: no workspace
    }
}
