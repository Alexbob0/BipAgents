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
//! reads them, for the message the sender writes it next. No approval: both agents are the person's own, and the
//! files stay theirs. Every path is opened by the bridge without following a link, so a link made in a sandbox never
//! leads to this machine's files.
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
    message_agent). It can't see your files, nor you its files: a path in a message means nothing to it, and there is \
    no common folder to look for. This tool copies the files into the files it received, then tells you where it finds \
    them: write to it right after with message_agent, saying what they are and giving those paths. Send a file you made \
    by copying it into {OUTBOX} first (no sub-folder), or a file you were sent as you see it, or give a short text as \
    `text`."),
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

fn links(config: &Config) -> Vec<(String, String)> {
    let data = crate::peers::Links::new(&crate::py::join(&config.data_dir(), "peers.json")).load();
    crate::peers::links_of(&data)
}

/// The agents of this place `agent` may write to.
fn targets<'a>(config: &'a Config, agent: &'a str) -> impl Iterator<Item = String> + 'a {
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
fn under<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
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
    if !crate::sandbox::isolates(config, agent) && crate::shares::why_not(config, agent).is_none() {
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
    if found.is_none() && isolated.is_none() && crate::shares::why_not(config, agent).is_none() {
        if let Some(rest) = under(path, "/partage") {
            found = Some((crate::shares::shares_root(config).join(&agent.name), rest, false));
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

/// `root/relative` opened one name at a time, none of them a link (`openat` with O_NOFOLLOW): a folder or file the
/// agent swaps for a link meanwhile fails here instead of leading to this machine's files. A regular file only.
pub fn open_beneath(root: &Path, relative: &Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let parts: Vec<_> = relative.components().map(|c| c.as_os_str().to_os_string()).collect();
    let mut current = std::fs::File::open(root)?;
    for (i, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(part.as_bytes()).map_err(std::io::Error::other)?;
        let kind = if i + 1 == parts.len() { libc::O_NONBLOCK } else { libc::O_DIRECTORY };
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | kind;
        // SAFETY: a valid directory descriptor and a NUL-terminated name; the new descriptor is owned right after.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` was just opened and nothing else owns it.
        current = unsafe { std::fs::File::from_raw_fd(fd) };
    }
    if parts.is_empty() || !current.metadata()?.is_file() {
        return Err(std::io::Error::other("not a regular file"));
    }
    Ok(current)
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
fn recipient<'a>(config: &'a Config, wanted: &str) -> Option<&'a AgentConfig> {
    let name = wanted.trim();
    let name = name.strip_prefix(&format!("{}/", crate::peers::PEER_NAME)).unwrap_or(name).trim_start_matches('@');
    config.agent(Some(name)).or_else(|| {
        let low = name.to_lowercase();
        config.agents.values().find(|a| !a.display_name.is_empty() && a.display_name.to_lowercase() == low)
    })
}

fn shown(agent: &AgentConfig) -> &str {
    if agent.display_name.is_empty() {
        &agent.name
    } else {
        &agent.display_name
    }
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
    let has_uploads = |a: &AgentConfig| {
        a.upload_dir_host.as_deref().is_some_and(|h| !h.is_empty())
            && a.upload_dir_container.as_deref().is_some_and(|c| !c.is_empty())
    };
    if !has_uploads(&target) {
        return tool_failure(
            &format!("{} can't receive files on this server.", shown(&target)),
            &format!("{} can't receive files for now.", shown(&target)),
        );
    }
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
    let mut given: Vec<(String, String)> = Vec::new(); // (name, path as the recipient sees it)
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
                let path = stored.get("path").map(crate::py::py_str).unwrap_or_default();
                given.push((stored.get("filename").map(crate::py::py_str).unwrap_or_else(|| name.to_string()), path));
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
    }
    tracing::info!(target: "bipbridge::agents", sender = %agent.name, recipient = %target.name, files = given.len(), "files given to another agent");
    let lines: Vec<String> = given.iter().map(|(name, path)| format!("- {name}: {path}")).collect();
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
    fn stored_names_lose_their_id() {
        assert_eq!(shown_name(Path::new("/f/2026-10/0123456789ab-devis.pdf")), "devis.pdf");
        assert_eq!(shown_name(Path::new("/envoi/devis.pdf")), "devis.pdf");
    }
}
