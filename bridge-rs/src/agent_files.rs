//! Files from one agent to another of the same person (owner's test 2026-10-22: Vie could not pass three PDFs to
//! Kerfy Studio). Hermes' `message_agent` carries text only, and a path in it means nothing to the other agent: each
//! agent sees its own files (its sandbox, its container), never the other's.
//!
//! `send_files_to_agent` (MCP, agent_tools.rs; offered to an agent that may write to another one) takes files the
//! sending agent has, where the bridge can reach them:
//!
//! - `/envoi/<name>`: its outbox. An isolated sandbox's own volume (sandbox.rs); for an agent set up by hand
//!   (`shares = "external"`), `<data>/envoi/<agent>`, which `shares-mounts` lists for its container;
//! - the files it was sent (its uploads as it sees them: `/fichiers/…` in a sandbox), and the shared files it fetched;
//! - `/partage/…`, its shared folder, where its sandbox mounts it;
//! - the audio files the bridge made for it (`/bipagents/audio/<agent>/…`);
//! - or a short text the tool writes as a file.
//!
//! It checks the link the person allowed (peers.rs: the same as for messages; never another household's agent:
//! family_news.rs stays the only way there), then stores each file in the recipient's uploads as an upload from the
//! app would (its « Fichiers », retention, the copy in its isolated sandbox), and gives back where the recipient
//! reads them, for the message the sender writes it next: the path its tools really see (`seen_root`), readable at
//! once, also in a sandbox started before its profile mounted that folder (`into_running_sandboxes`); a recipient
//! that can't open files gets none, and the sender is told why. No approval: both agents are the person's own, and
//! the files stay theirs. Every path is opened by the bridge without following a link, so a link made in a sandbox
//! never leads to this machine's files.
//!
//! No Python counterpart.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::sandbox::{plain_relative, OUTBOX};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "send_files_to_agent";
/// Files in one call.
pub const MAX_FILES: usize = 20;
/// The largest `text`.
pub const MAX_TEXT: usize = crate::sandbox::MAX_CONTENT;

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": format!("Give files to another of your person's agents (one you may write to with \
    message_agent): the way it gets a file of yours. It sees none of your folders, your shared folder /partage \
    included, and a path of yours means nothing to it. This tool puts the files with those it received and tells you \
    the path it reads them at: then write to it with message_agent, saying what they are, with those paths. Give a \
    file you made by copying it right into {OUTBOX} first (no sub-folder), or a file you were sent as you see it, or a \
    short text as `text`."),
        "inputSchema": {"type": "object", "properties": {
            "agent": {"type": "string", "description": "the other agent, as in your contacts (e.g. bip/kerfy-studio) or by its name"},
            "files": {"type": "array", "items": {"type": "string"}, "maxItems": MAX_FILES,
                      "description": format!("your files' paths, e.g. {OUTBOX}/devis.pdf")},
            "text": {"type": "string", "description": "or a short text to give as a file"},
            "text_name": {"type": "string", "description": "that file's name, e.g. resume.md"},
        }, "required": ["agent"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
    })
}

/// Whether `agent` gets the tool: the person let it write to another agent of this place.
pub fn offered(config: &Config, agent: &str) -> bool {
    targets(config, agent).next().is_some()
}

/// An error's text (an HttpError's detail).
fn detail(exc: &crate::web::HttpError) -> String {
    crate::py::py_str(&exc.detail)
}

pub(crate) fn links(config: &Config) -> Vec<(String, String)> {
    let data = crate::peers::Links::new(&crate::py::join(&config.data_dir(), "peers.json")).load();
    crate::peers::links_of(&data)
}

/// The agents of this place `agent` may write to.
pub(crate) fn targets<'a>(config: &'a Config, agent: &'a str) -> impl Iterator<Item = String> + 'a {
    links(config)
        .into_iter()
        .filter(move |(from, to)| {
            from == agent && crate::households::split(to).is_none() && config.agent(Some(to)).is_some()
        })
        .map(|(_, to)| to)
}

/// The outbox of an agent set up by hand: mounted at /envoi by its administrator (`shares-mounts`).
pub fn external_outbox(config: &Config, agent: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("envoi").join(agent)
}

/// `path` under `prefix` (a folder): the rest, without its leading `/`.
pub(crate) fn under<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return None;
    }
    path.strip_prefix(prefix)?.strip_prefix('/').filter(|rest| !rest.is_empty())
}

/// Where this agent can put a file it made so that the tool reaches it (for the error the agent gets).
fn where_to_put(config: &Config, agent: &AgentConfig) -> String {
    let mut places: Vec<String> = Vec::new();
    if crate::sandbox::isolates(config, agent) || agent.shares == "external" {
        places.push(format!("copy it right into {OUTBOX} (no sub-folder)"));
    }
    let shared = crate::shares::load(config).iter().any(|(_, source)| source == &agent.name);
    if crate::shares::why_not(config, agent).is_none() && (shared || !crate::sandbox::isolates(config, agent)) {
        places.push("put it in /partage".into());
    }
    if places.is_empty() {
        "Only files you were sent, the audio you made, or a short text (`text`) can be given from here.".into()
    } else {
        format!("To give a file you made, {} and give that path.", places.join(", or "))
    }
}

/// The file behind a path as the agent sees it: its folder on this machine and the path in it. Err: why not, for
/// the agent.
fn locate(config: &Config, agent: &AgentConfig, raw: &str) -> Result<(PathBuf, PathBuf), String> {
    let path = raw.trim().trim_start_matches("MEDIA:").trim();
    let path = path.strip_prefix("file://").unwrap_or(path);
    let isolated =
        if crate::sandbox::isolates(config, agent) { crate::sandbox::volumes_of(config, &agent.name) } else { None };
    let mut found: Option<(PathBuf, &str, bool)> = None; // (folder, rest, a single name only)
    if let Some(rest) = under(path, OUTBOX) {
        let outbox = match &isolated {
            Some((_, outbox)) => Some(outbox.clone()),
            None if agent.shares == "external" => Some(external_outbox(config, &agent.name)),
            None => None,
        };
        if let Some(outbox) = outbox {
            found = Some((outbox, rest, true));
        }
    }
    if found.is_none() {
        if let (Some((files, _)), Some(rest)) = (&isolated, under(path, crate::sandbox::RECEIVED)) {
            found = Some((files.clone(), rest, false));
        }
    }
    if found.is_none() {
        let host = agent.upload_dir_host.as_deref().filter(|h| !h.is_empty());
        let seen = agent.upload_dir_container.as_deref().filter(|c| !c.is_empty()).or(host);
        if let (Some(host), Some(rest)) = (host, seen.and_then(|seen| under(path, seen))) {
            found = Some((PathBuf::from(host), rest, false));
        }
    }
    if found.is_none() && crate::shares::why_not(config, agent).is_none() {
        if let Some(rest) = under(path, "/partage") {
            found = Some((crate::shares::share_dir(config, &agent.name), rest, false));
        }
    }
    if found.is_none() {
        let audio = format!("{}/{}", crate::agent_tools::MEDIA_PREFIX, agent.name);
        if let Some(rest) = under(path, &audio) {
            found = Some((crate::agent_tools::audio_root(config).join(&agent.name), rest, true));
        }
    }
    let Some((folder, rest, single)) = found else {
        return Err(format!("{raw}: not a file I can reach. {}", where_to_put(config, agent)));
    };
    let relative = plain_relative(rest).filter(|r| !single || r.components().count() == 1);
    let Some(relative) = relative else {
        return Err(format!("{raw}: not a file I can reach. {}", where_to_put(config, agent)));
    };
    Ok((folder, relative))
}

/// `root/relative` opened one name at a time, none of them a link (beneath.rs: `openat2(RESOLVE_BENEATH |
/// RESOLVE_NO_SYMLINKS)` on Linux, else `openat` with O_NOFOLLOW): a folder or file the agent swaps for a link
/// meanwhile fails here instead of leading to this machine's files. A regular file only.
pub fn open_beneath(root: &Path, relative: &Path) -> std::io::Result<std::fs::File> {
    crate::beneath::open_file(root, relative)
}

/// The name the recipient sees: a stored upload's `<id12>-` left out.
fn shown_name(file: &Path) -> String {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if crate::files::STORED_NAME.is_match(&name) {
        name.chars().skip(13).collect()
    } else {
        name
    }
}

/// The recipient named as the agent may have written it: `bip/kerfy-studio`, `@kerfy-studio`, « Kerfy Studio ».
pub(crate) fn recipient<'a>(config: &'a Config, wanted: &str) -> Option<&'a AgentConfig> {
    let name = wanted.trim();
    let name = name.strip_prefix(&format!("{}/", crate::peers::PEER_NAME)).unwrap_or(name).trim_start_matches('@');
    config.agent(Some(name)).or_else(|| {
        let low = name.to_lowercase();
        config.agents.values().find(|a| !a.display_name.is_empty() && a.display_name.to_lowercase() == low)
    })
}

pub(crate) fn shown(agent: &AgentConfig) -> &str {
    if agent.display_name.is_empty() {
        &agent.name
    } else {
        &agent.display_name
    }
}

/// A file given: its name, where it is on this machine (the recipient's uploads), where the recipient reads it.
struct Given {
    name: String,
    host: PathBuf,
    seen: String,
}

/// The folder where `target`'s tools see the files it is given (its uploads), from what its Hermes really runs, not
/// only from what the bridge's config says: Err, why it can't open them (said plainly to the sender instead of a
/// path). Owner's report 2026-11-07 (Vie → Wellness, a PDF of lab results): Wellness found neither the file nor
/// `/fichiers` at the path it was given. The cases:
///
/// - its tools in Hermes' docker sandbox, isolated by the bridge (sandbox.rs): `/fichiers`, its files' volume, filled
///   by `sync_agent` — whatever `upload_dir_container` says: an agent just moved into its own space still had its
///   space's folder there, which nothing of this machine's is mounted in its sandbox (spaces.rs, until the bridge's
///   next start);
/// - in a sandbox the bridge doesn't isolate: `/fichiers` where its profile mounts its uploads
///   (`agents_admin::mount_uploads`), else nowhere yet;
/// - on this machine (Hermes' `local` backend): the folder as the bridge was told (the hub's container home), but
///   never `/fichiers`, a sandbox's path left from before (its folder on this machine then);
/// - set up by hand, or not a profile of the bridge's: as the bridge was told (its administrator mounts it there);
/// - its profile without file tools (`terminal`, `file`, `code_execution` all off): nowhere.
pub(crate) fn seen_root(config: &Config, target: &AgentConfig) -> Result<String, String> {
    use crate::files::SANDBOX_UPLOADS;
    let host = target.upload_dir_host.as_deref().filter(|h| !h.is_empty());
    let told = target.upload_dir_container.as_deref().filter(|c| !c.is_empty());
    let Some(host) = host else { return Err("it has no folder for received files".into()) };
    let folder = crate::shares::profile_dir(config, target).filter(|f| f.is_dir());
    let Some(folder) = folder.filter(|_| target.shares != "external") else {
        return told.map(str::to_string).ok_or_else(|| "it has no folder for received files".into());
    };
    if !reads_files(&folder) {
        return Err("its file tools are turned off, so it can't open files".into());
    }
    match crate::shares::terminal_backend(&folder).as_deref() {
        Some("docker") if crate::sandbox::isolates(config, target) => {
            match crate::sandbox::volumes_of(config, &target.name) {
                Some(_) => Ok(crate::sandbox::RECEIVED.to_string()),
                None => Err("its sandbox isn't set up for files yet".into()),
            }
        }
        Some("docker") => {
            let mounted = crate::shares::docker_volumes(&folder).iter().any(|v| {
                let mut parts = v.split(':');
                parts.next().is_some_and(|h| h.trim_end_matches('/') == host.trim_end_matches('/'))
                    && parts.next() == Some(SANDBOX_UPLOADS)
            });
            if mounted {
                Ok(SANDBOX_UPLOADS.to_string())
            } else {
                Err("its sandbox doesn't mount its received files yet".into())
            }
        }
        None | Some("local") => Ok(told.filter(|c| *c != SANDBOX_UPLOADS).unwrap_or(host).to_string()),
        Some(_) => told.map(str::to_string).ok_or_else(|| "it has no folder for received files".into()),
    }
}

/// Hermes' toolsets that open no file of the agent's: a profile whose app toolsets (`platform_toolsets.api_server`)
/// are only these can't read what it is given. Any other name (`file`, `terminal`, `code_execution`, Hermes' API
/// toolset `hermes-api-server` that holds them, a toolset unknown here) counts as able to.
const NO_FILE_TOOLSETS: [&str; 15] = [
    "web",
    "search",
    "vision",
    "image_gen",
    "memory",
    "clarify",
    "session_search",
    "skills",
    "todo",
    "delegation",
    "connections",
    "tts",
    "cronjob",
    "messaging",
    "browser",
];

/// Whether the profile's Hermes tools can open a file (`platform_toolsets.api_server`, the toolsets of the app's
/// conversations; unset: Hermes' API toolset, which has them).
fn reads_files(folder: &Path) -> bool {
    let data = crate::shares::profile_config(folder);
    match data.get("platform_toolsets").and_then(|p| p.get(crate::tool_diet::API_PLATFORM)) {
        Some(Value::Array(sets)) => !sets.iter().filter_map(Value::as_str).all(|s| NO_FILE_TOOLSETS.contains(&s)),
        _ => true,
    }
}

/// Why a sandbox restart was asked for: its running sandbox doesn't mount its received files.
pub const NO_RECEIVED_MOUNT: &str = "its sandbox doesn't mount its received files";

/// The files given, readable right now at their `/fichiers` path in each running sandbox of the recipient's that
/// doesn't mount that folder: Hermes keeps a sandbox's mounts for as long as it is used (its container is
/// recreated with the profile's new mounts only after `terminal.lifetime_seconds` idle, or a gateway restart), so one
/// started before its profile mounted `/fichiers` doesn't see the volume `sync_agent` filled (token lab of
/// 2026-11-07: `ls /fichiers`, no such folder). Copied in with `podman cp`, and its gateway restarted soon, once it is
/// idle, so that its next sandbox mounts the folder. Ok when nothing was missing or every copy was made.
fn into_running_sandboxes(config: &Config, target: &AgentConfig, given: &[Given]) -> Result<(), String> {
    let Some(podman) = crate::browsers::pyx::which("podman").or_else(|| crate::browsers::pyx::which("docker")) else {
        return Ok(()); // no container here: nothing running to look at
    };
    let Ok(sandboxes) = crate::sent_files::sandboxes(config, target, &podman) else { return Ok(()) };
    let missing: Vec<_> =
        sandboxes.iter().filter(|c| c.running && !c.mounts.iter().any(|m| m == crate::sandbox::RECEIVED)).collect();
    if missing.is_empty() {
        return Ok(());
    }
    crate::agent_restart::request_restart(&target.name, NO_RECEIVED_MOUNT);
    for sandbox in missing {
        for file in given {
            crate::sent_files::copy_into(&podman, &sandbox.id, &file.host, &file.seen)?;
        }
        tracing::info!(target: "bipbridge::agents", recipient = %target.name, container = %sandbox.id, files = given.len(), "given files put into a running sandbox that doesn't mount its received files");
    }
    Ok(())
}

/// One call, for `agent` (blocking: files only).
pub fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    let wanted = arguments.get("agent").map(crate::py::py_str).unwrap_or_default();
    if crate::households::split(wanted.trim().trim_start_matches("bip/")).is_some() {
        return tool_failure(
            "Files only go to your person's own agents here, never to another household's.",
            "I can only pass files on to your own agents.",
        );
    }
    let Some(target) = recipient(config, &wanted).cloned() else {
        let mine: Vec<String> = targets(config, &agent.name).map(|t| format!("bip/{t}")).collect();
        return tool_text(
            &format!(
                "No agent {wanted:?} here. {}",
                if mine.is_empty() {
                    "You may give files to none yet.".to_string()
                } else {
                    format!("You may give files to: {}.", mine.join(", "))
                }
            ),
            true,
        );
    };
    if target.name == agent.name {
        return tool_text("That is you: name another agent.", true);
    }
    if !links(config).iter().any(|(from, to)| *from == agent.name && *to == target.name) {
        return tool_failure(
            &format!("The person hasn't let you write to {}.", shown(&target)),
            &format!(
                "I can't pass things on to {} yet: you can allow it in the app, under « Qui peut écrire à qui ».",
                shown(&target)
            ),
        );
    }
    let seen = match seen_root(config, &target) {
        Ok(root) => root,
        Err(why) => {
            return tool_failure(
                &format!("{} can't receive files here: {why}. Nothing was given.", shown(&target)),
                &format!("{} can't receive files for now.", shown(&target)),
            )
        }
    };
    let paths: Vec<String> = match arguments.get("files") {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).filter(|p| !p.trim().is_empty()).collect(),
        Some(Value::String(one)) if !one.trim().is_empty() => vec![one.clone()],
        _ => Vec::new(),
    };
    let text = arguments.get("text").and_then(Value::as_str).filter(|t| !t.trim().is_empty());
    if paths.is_empty() && text.is_none() {
        return tool_text(&format!("Give `files` (e.g. {OUTBOX}/devis.pdf) or a `text`."), true);
    }
    if paths.len() > MAX_FILES {
        return tool_text(&format!("At most {MAX_FILES} files at a time."), true);
    }
    if text.is_some_and(|t| t.len() > MAX_TEXT) {
        return tool_text(&format!("This text is too long: put it in a file in {OUTBOX} and give that."), true);
    }
    // Every file found before any is given: all or nothing.
    let limit = config.limits.upload_max_bytes;
    let mut sources: Vec<(Option<std::fs::File>, String)> = Vec::new();
    for raw in &paths {
        let (folder, relative) = match locate(config, agent, raw) {
            Ok(found) => found,
            Err(why) => return tool_text(&why, true),
        };
        let opened = open_beneath(&folder, &relative);
        match opened.as_ref().map(|f| f.metadata().map(|m| m.len()).unwrap_or(u64::MAX)) {
            Ok(size) if size > limit => {
                return tool_failure(
                    &format!("{raw} is too large to give (max {} MB).", limit / (1024 * 1024)),
                    "This file is too large to pass on.",
                )
            }
            Ok(_) => sources.push((opened.ok(), shown_name(&relative))),
            Err(_) => return tool_text(&format!("{raw}: no such file. {}", where_to_put(config, agent)), true),
        }
    }
    let mut given: Vec<Given> = Vec::new();
    let mut store =
        |open: &mut dyn FnMut() -> std::io::Result<Box<dyn Read>>, name: &str| match crate::files::store_upload_from(
            &target,
            open,
            name,
            None,
            &config.limits,
            None,
        ) {
            Ok(stored) => {
                // `<month>/<id>-<name>`, under the recipient's uploads folder and under the folder its tools see
                let path = stored.get("path").map(crate::py::py_str).unwrap_or_default();
                let mut parts = path.rsplitn(3, '/');
                let (file, month) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
                let host = target.upload_dir_host.as_deref().unwrap_or_default();
                given.push(Given {
                    name: stored.get("filename").map(crate::py::py_str).unwrap_or_else(|| name.to_string()),
                    host: Path::new(host).join(month).join(file),
                    seen: format!("{}/{month}/{file}", seen.trim_end_matches('/')),
                });
                Ok(())
            }
            Err(exc) => Err(exc),
        };
    for (file, name) in &mut sources {
        // The file as it was checked, already open: nothing swapped in between.
        let mut open = || {
            file.take()
                .map(|f| Box::new(f.take(limit + 1)) as Box<dyn Read>)
                .ok_or_else(|| std::io::Error::other("already read"))
        };
        if let Err(exc) = store(&mut open, name) {
            tracing::warn!(target: "bipbridge::agents", sender = %agent.name, recipient = %target.name, error = %detail(&exc), "file not given");
            return tool_failure(
                &format!("{name} could not be given ({}).", detail(&exc)),
                "I couldn't pass this file on; I'll try again.",
            );
        }
    }
    if let Some(text) = text {
        let name = arguments.get("text_name").map(crate::py::py_str).filter(|n| !n.trim().is_empty());
        let name = name.unwrap_or_else(|| "note.txt".into());
        let bytes = text.as_bytes().to_vec();
        let mut open = || Ok(Box::new(std::io::Cursor::new(bytes.clone())) as Box<dyn Read>);
        if let Err(exc) = store(&mut open, &name) {
            return tool_text(&format!("The text could not be given ({}).", detail(&exc)), true);
        }
    }
    if let Err(exc) = crate::sandbox::sync_agent(config, &target.name) {
        tracing::warn!(target: "bipbridge::agents", recipient = %target.name, error = %exc, "given files not copied into the sandbox");
        return tool_failure(
            &format!("The files could not be put where {} reads them ({exc}).", shown(&target)),
            "I couldn't pass this file on; I'll try again.",
        );
    }
    if seen == crate::sandbox::RECEIVED {
        if let Err(why) = into_running_sandboxes(config, &target, &given) {
            tracing::warn!(target: "bipbridge::agents", recipient = %target.name, error = %why, "given files not put into its running sandbox");
            return tool_failure(
                &format!(
                    "{who} can't open them right now: its sandbox doesn't show its received files yet ({why}). They \
are kept in its files; it sees them once its sandbox restarts, soon. Tell {who} so with message_agent, or try again \
later.",
                    who = shown(&target)
                ),
                &format!("{} will see this file a bit later.", shown(&target)),
            );
        }
    }
    tracing::info!(target: "bipbridge::agents", sender = %agent.name, recipient = %target.name, files = given.len(), seen = %seen, "files given to another agent");
    let lines: Vec<String> = given.iter().map(|g| format!("- {}: {}", g.name, g.seen)).collect();
    tool_text(
        &format!(
            "Given to {who}: {n} file(s). {who} finds them here (read-only):\n{}\nIt doesn't know yet: write to it now \
with message_agent (target bip/{target}), saying what these files are and what you'd like, with these paths. To your \
person, just say the files were passed on to {who}.",
            lines.join("\n"),
            who = shown(&target),
            n = given.len(),
            target = target.name,
        ),
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_under_a_folder() {
        assert_eq!(under("/envoi/a.pdf", "/envoi"), Some("a.pdf"));
        assert_eq!(under("/envoi/", "/envoi"), None);
        assert_eq!(under("/envoie/a.pdf", "/envoi"), None);
        assert_eq!(under("/x/uploads/2026-10/a", "/x/uploads/"), Some("2026-10/a"));
        assert_eq!(under("/a", ""), None);
    }

    #[test]
    fn never_through_a_link() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("envoi");
        std::fs::create_dir_all(root.join("2026-10")).unwrap();
        std::fs::write(root.join("2026-10/a.pdf"), b"pdf").unwrap();
        std::fs::write(dir.path().join("secret"), b"key").unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("up")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("secret"), root.join("s")).unwrap();
        let mut read = String::new();
        open_beneath(&root, Path::new("2026-10/a.pdf")).unwrap().read_to_string(&mut read).unwrap();
        assert_eq!(read, "pdf");
        assert!(open_beneath(&root, Path::new("up/secret")).is_err());
        assert!(open_beneath(&root, Path::new("s")).is_err());
        assert!(open_beneath(&root, Path::new("2026-10")).is_err()); // a folder
        assert!(open_beneath(&root, Path::new("")).is_err());
    }

    #[test]
    fn file_tools_off_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        assert!(reads_files(dir.path())); // no config: Hermes' API toolset, which has them
        let write = |sets: &str| {
            std::fs::write(dir.path().join("config.yaml"), format!("platform_toolsets:\n  api_server: {sets}\n"))
                .unwrap()
        };
        write("[web, memory, terminal]");
        assert!(reads_files(dir.path()));
        write("[hermes-api-server, clarify]"); // what the bridge writes (tool_diet.rs `with_clarify`)
        assert!(reads_files(dir.path()));
        write("[web, memory, vision]");
        assert!(!reads_files(dir.path()));
    }

    #[test]
    fn stored_names_lose_their_id() {
        assert_eq!(shown_name(Path::new("/f/2026-10/0123456789ab-devis.pdf")), "devis.pdf");
        assert_eq!(shown_name(Path::new("/envoi/devis.pdf")), "devis.pdf");
    }
}
