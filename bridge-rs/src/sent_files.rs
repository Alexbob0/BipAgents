//! Files an agent hands the person with a `MEDIA:<path>` line from its own places (owner's report 2026-10-22: since
//! the sandboxes are isolated, Wellness' « infographie » showed « Fichier indisponible »: a sandbox writes in its
//! `/envoi` volume or its workspace, and none of them is a declared `[media] roots`).
//!
//! The path is the agent's own: `/envoi/courses.xlsx`, `/workspace/out/graph.png`, `/root/notes.md`,
//! `/fichiers/2026-10/…`. Its places, for that agent only (never another agent's):
//!
//! - `/envoi`: its isolated sandbox's outbox volume (`<data>/sandbox.json`), or `<data>/envoi/<agent>` for an agent
//!   set up by hand (agent_files.rs);
//! - `/workspace` and `/root`: Hermes' persistent docker sandbox folders under its profile,
//!   `<profile>/sandboxes/docker/<task>/workspace` and `…/home` (tools/environments/docker.py at the pinned commit:
//!   `container_persistent`, Hermes' default). `/tmp` is a tmpfs nothing outside the sandbox reaches;
//! - the files it was sent (its uploads, as it sees them: `/fichiers/…` in a sandbox).
//!
//! The bridge copies the file into a store of its own when the run that names it ends (runs.rs) or when the app first
//! asks for it (`GET /v1/media?path=…&agent=…`): `<data>/sent-files/<agent>/<16 hex of the path>/<name>`, kept as
//! long as the person's files (files_kept.rs), and serves that copy, so the file stays when the sandbox is reset. A
//! newer file at the same path replaces the copy. Each path is opened one name at a time without following a link
//! (`agent_files::open_beneath`): a link made in a sandbox never leads to this machine's files.
//!
//! No Python counterpart.

use std::io::Read;
use std::path::PathBuf;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::agent_files::{open_beneath, under};
use crate::config::{AgentConfig, Config};
use crate::sandbox::{plain_relative, OUTBOX};

/// Where Hermes' persistent docker sandboxes keep their folders, under a profile.
const SANDBOXES: &str = "sandboxes/docker";

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

fn modified(file: &std::fs::File) -> SystemTime {
    file.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// The agent's file behind `path`, opened (the newest when several sandboxes have one). Err: why not, for the log.
fn open_source(config: &Config, agent: &AgentConfig, path: &str) -> Result<std::fs::File, &'static str> {
    for (prefix, folders) in places(config, agent) {
        let Some(rest) = under(path, &prefix) else { continue };
        let Some(relative) = plain_relative(rest) else { return Err("not a plain path") };
        let newest = folders.iter().filter_map(|folder| open_beneath(folder, &relative).ok()).max_by_key(modified);
        return newest.ok_or("no such file in the agent's places (or a link)");
    }
    Err("not under one of the agent's places (/envoi, /workspace, /root, its received files)")
}

/// The copy of the agent's file behind `path`, made or refreshed now: Err(why) when there is none.
pub fn capture(config: &Config, agent: &AgentConfig, path: &str) -> Result<PathBuf, String> {
    if crate::media::content_type_of(path).is_none() {
        return Err("not a type of file the apps open".into());
    }
    let target = copy_path(config, &agent.name, path).ok_or("not a plain path")?;
    let existing = std::fs::metadata(&target).ok().filter(|m| m.is_file());
    let source = match open_source(config, agent, path) {
        Ok(file) => file,
        Err(_) if existing.is_some() => return Ok(target), // the sandbox reset since: the copy stays
        Err(why) => return Err(why.into()),
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
    let folder = target.parent().ok_or("no folder")?;
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let tmp = folder.join(format!(".{}.part", crate::py::token_urlsafe(6)));
    let result = (|| -> std::io::Result<()> {
        let mut out = std::fs::File::create(&tmp)?;
        let copied = std::io::copy(&mut source.take(limit + 1), &mut out)?;
        if copied > limit {
            return Err(std::io::Error::other("too large"));
        }
        std::fs::rename(&tmp, &target)
    })();
    if let Err(exc) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(exc.to_string());
    }
    Ok(target)
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
        match capture(config, agent, &path) {
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
    fn the_received_files_and_the_persistent_workspace() {
        let (dir, config, agent) = setup();
        std::fs::write(dir.path().join("uploads/2026-10/0123456789ab-facture.pdf"), b"%PDF").unwrap();
        assert!(capture(&config, &agent, "/fichiers/2026-10/0123456789ab-facture.pdf").is_ok());
        assert!(capture(&config, &agent, "/workspace/x.png").is_err()); // no Hermes profile: no workspace
    }
}
