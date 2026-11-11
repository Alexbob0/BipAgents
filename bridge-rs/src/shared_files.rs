//! Files sent to a shared agent, the family's included (Alex, 11/11; internal/notes/shared-agent-files.md). A file a
//! relative sends stays in her own conversation with the agent:
//!
//! - **her bridge** holds a document her app stored first (`POST /v1/files` with `agent=<agent>__<household>`:
//!   `<data>/shared-held/<id>/<id12>-<name>`, answered as `/bipagents/held/<id12>-<name>`), and with her message sends
//!   each file to the host, then the message naming them (agent_shares.rs `relay_send`); the held copy goes once the
//!   host took the message, or after a day;
//! - **the host** keeps it at `<data>/shared-files/<agent>/<cid>/<id12>-<name>` (folders 0700, files 0600), listed in
//!   that conversation's entry of `agent-shares.json` (`files: {<id12>: {file, name, mime, size, kind, at, sent}}`),
//!   never in the agent's uploads, its sandbox or its Hermes profile: the owner's own turns can't reach it;
//! - **her turn** gets her photos as image parts and each document as one line
//!   `[Pièce jointe : name (mime, size) → /proche/<cid>/<id12>-<name>]`; the agent reads a document with
//!   [`TOOL_NAME`], which answers only in the turn of the relative of that very conversation (the run hub sees her
//!   run inside the tool);
//! - **her timeline** keeps her conversation's files (`/proche/<cid>/…`), each with a url on her bridge
//!   (`/v1/shared-agents/<id>/files/<cid>/<id12>`) relayed to the host's `GET …/files/<id12>`;
//! - **they go** with the conversation, when the owner stops sharing with her, when she stops using the agent
//!   (`DELETE /family/s/{agent}/files`), with a household unlinked, after the host's `kept_days` (photos 30 days), and a
//!   file no message claimed after an hour ([`purge`]).
//!
//! The same limits as files to one's own agents: `[limits] upload_max_mb` per file, ten a message, the place's room.
//!
//! Between the bridges (the household key):
//!
//!     POST   /family/s/{agent}/conversations/{cid}/files?name&mime&kind   raw body → {id, name, mime, size, kind, path}
//!     GET    /family/s/{agent}/conversations/{cid}/files/{id}             the file (her conversation's only)
//!     DELETE /family/s/{agent}/conversations/{cid}/files/{id}             {ok}  one no message claimed (taken back)
//!     DELETE /family/s/{agent}/files                                      {ok, removed}  all of her household's
//!
//! For her app (under /v1, the bridge key): `GET /v1/shared-agents/{id}/files/{cid}/{file}`, relayed.
//!
//! No Python counterpart.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use futures::StreamExt;
use regex::Regex;
use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;

use crate::agent_shares as shares;
use crate::app::AppState;
use crate::config::Config;
use crate::threads::send::Attachment;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The bridge tool reading a file a relative sent, in her own turn only.
pub const TOOL_NAME: &str = "read_sent_file";
/// Where the agent sees a relative's file: `/proche/<cid>/<id12>-<name>` (no such folder anywhere: the tool reads it).
pub const SEEN_PREFIX: &str = "/proche/";
/// What her bridge answers her app for a document it holds until her message goes.
pub const HELD_PREFIX: &str = "/bipagents/held/";
/// Files one message carries at most (threads/send.rs `ATTACHMENTS_MAX`).
pub const FILES_MAX: usize = 10;
/// A file sent to the host that no message claimed (her message refused, her bridge gone meanwhile) goes after this.
const UNCLAIMED_SECONDS: f64 = 3600.0;
/// A held file not sent goes after this.
const HELD_SECONDS: f64 = 86_400.0;
/// Characters of a document the tool gives at once.
const TEXT_MAX: usize = 60_000;
/// A text file read at most.
const TEXT_FILE_MAX: u64 = 2 * 1024 * 1024;
const PDF_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the tool waits for Hermes' event of its own call (whose turn it is).
const WHO_WAIT: Duration = Duration::from_secs(3);
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

static CONVERSATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^c[0-9a-f]{12}\z").expect("valid regex"));
static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{12}\z").expect("valid regex"));
/// A shared agent's id on her side (`cuisine__alex`), as a folder name.
static SHARED_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,140}\z").expect("valid regex"));

fn now() -> f64 {
    py::time_time()
}

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

// -- where they are ----------------------------------------------------------------------------------------------

fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-files")
}

/// The folder of one relative's conversation with `agent` on the host.
pub fn conversation_dir(config: &Config, agent: &str, cid: &str) -> PathBuf {
    root(config).join(agent).join(cid)
}

fn held_dir(config: &Config, id: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-held").join(id)
}

/// The path the agent is given for a file of conversation `cid`.
pub fn seen_path(cid: &str, file: &str) -> String {
    format!("{SEEN_PREFIX}{cid}/{file}")
}

/// `(cid, file, key)` of a seen path; None for anything else.
pub fn parse_seen(path: &str) -> Option<(String, String, String)> {
    let (cid, file) = path.strip_prefix(SEEN_PREFIX)?.split_once('/')?;
    let key = stored_key(file)?;
    CONVERSATION.is_match(cid).then(|| (cid.to_string(), file.to_string(), key))
}

/// A stored name's key (its 12 hex before `-`), when it is one the bridge wrote.
fn stored_key(file: &str) -> Option<String> {
    let ok = crate::files::STORED_NAME.is_match(file)
        && !file.contains(['/', '\\', '\0', '\n'])
        && file.chars().count() <= 140;
    ok.then(|| file[..12].to_string())
}

/// Whether `path` is a file of conversation `cid` (the attachments her timeline keeps).
pub fn in_conversation(path: &str, cid: &str) -> bool {
    parse_seen(path).is_some_and(|(c, _, _)| c == cid)
}

fn make_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().recursive(true).mode(DIR_MODE).create(dir)
}

fn clean_mime(given: Option<&str>) -> String {
    let mime = given.unwrap_or("").trim().to_ascii_lowercase();
    let ok = mime.len() <= 100
        && mime.split_once('/').is_some_and(|(a, b)| !a.is_empty() && !b.is_empty())
        && mime.chars().all(|c| c.is_ascii_alphanumeric() || "/.+-_".contains(c));
    if ok {
        mime
    } else {
        "application/octet-stream".into()
    }
}

fn kind_of(given: Option<&str>, mime: &str) -> &'static str {
    match given {
        Some("image") => "image",
        Some(_) => "document",
        None if mime.starts_with("image/") => "image",
        None => "document",
    }
}

// -- the host: what her conversation holds ------------------------------------------------------------------------

/// A change of conversation `cid`'s files (None: no such conversation any more).
fn with_files<R>(config: &Config, cid: &str, change: impl FnOnce(&mut Map<String, Value>) -> R) -> anyhow::Result<Option<R>> {
    shares::update(&shares::owner_path(config), |data| {
        let Some(Value::Object(c)) = shares::object_mut(data, "conversations").get_mut(cid) else { return None };
        if !matches!(c.get("files"), Some(Value::Object(_))) {
            c.insert("files".into(), json!({}));
        }
        match c.get_mut("files") {
            Some(Value::Object(files)) => Some(change(files)),
            _ => None,
        }
    })
}

/// The file `key` of conversation `c`, when it lists one.
fn entry_of(c: &Value, key: &str) -> Option<Value> {
    c.get("files").and_then(|f| f.get(key)).filter(|e| stored_key(&str_of(e, "file")).as_deref() == Some(key)).cloned()
}

/// A request body written to `path`, `max` bytes at most (413 past them, 422 empty); nothing left on a failure.
async fn write_stream(dir: &Path, path: &Path, body: Body, max: u64) -> ApiResult<u64> {
    make_dir(dir).map_err(anyhow::Error::from)?;
    let mut out = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(path)
        .await
        .map_err(anyhow::Error::from)?;
    let mut stream = body.into_data_stream();
    let written: ApiResult<u64> = async {
        let mut size: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                if crate::body_limit::is_too_large(&e) {
                    HttpError::new(413, format!("file too large (max {max} bytes)"))
                } else {
                    HttpError::new(400, "the file did not arrive whole")
                }
            })?;
            size += chunk.len() as u64;
            if size > max {
                return Err(HttpError::new(413, format!("file too large (max {max} bytes)")));
            }
            out.write_all(&chunk).await.map_err(anyhow::Error::from)?;
        }
        out.flush().await.map_err(anyhow::Error::from)?;
        if size == 0 {
            return Err(HttpError::new(422, "an empty file"));
        }
        Ok(size)
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(path).await;
    }
    written
}

/// `POST /family/s/{agent}/conversations/{cid}/files?name&mime&kind`: a file of her coming message, stored in her
/// conversation (not claimed yet: her message names it).
async fn served_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
    q: web::QueryMap,
    body: Body,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    shares::conversation(&config, &household, &agent.name, &cid)?;
    if let Err(full) = crate::place_backup::refuse_when_full().await {
        // the host's place full: as for a file to its own agents (storage_watch.rs), its person told
        let c = config.clone();
        let _ = tokio::task::spawn_blocking(move || crate::storage_watch::upload_refused(&c, None)).await;
        return Err(full);
    }
    let name = crate::files::sanitize_filename(q.get("name").unwrap_or(""));
    let mime = clean_mime(q.get("mime"));
    let kind = kind_of(q.get("kind"), &mime);
    let key: String = py::uuid4_hex().chars().take(12).collect();
    let file = format!("{key}-{name}");
    let dir = conversation_dir(&config, &agent.name, &cid);
    let path = dir.join(&file);
    let size = write_stream(&dir, &path, body, config.limits.upload_max_bytes).await?;
    let entry = json!({"file": file, "name": name, "mime": mime, "size": size, "kind": kind, "at": now(), "sent": false});
    let kept = with_files(&config, &cid, |files| {
        files.insert(key.clone(), entry.clone());
    })?;
    if kept.is_none() {
        let _ = std::fs::remove_file(&path); // her conversation ended meanwhile
        return Err(HttpError::new(404, "no such conversation"));
    }
    tracing::info!(agent = %agent.name, household = %household, size = size, kind = kind, "shared agent: a file of a relative kept");
    Ok(web::ok(json!({"id": key, "name": name, "mime": mime, "size": size, "kind": kind, "path": seen_path(&cid, &file)})))
}

/// `GET /family/s/{agent}/conversations/{cid}/files/{id}`: one of her conversation's files, for her bridge.
async fn served_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid, key)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    let c = shares::conversation(&config, &household, &agent.name, &cid)?;
    let entry = KEY.is_match(&key).then(|| entry_of(&c, &key)).flatten().ok_or_else(HttpError::not_found)?;
    let path = conversation_dir(&config, &agent.name, &cid).join(str_of(&entry, "file"));
    let regular = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
    if !regular {
        return Err(HttpError::not_found());
    }
    let extra = [("x-content-type-options", "nosniff".to_string()),
                 ("content-security-policy", "sandbox; default-src 'none'".to_string())];
    let name = str_of(&entry, "name");
    Ok(web::file_response(&path.to_string_lossy(), &str_of(&entry, "mime"), Some(&name), &extra, &headers).await)
}

/// `DELETE /family/s/{agent}/conversations/{cid}/files/{id}`: a file her bridge sent for a message that didn't go
/// (taken back; one a message holds stays).
async fn served_take_back(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid, key)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    shares::conversation(&config, &household, &agent.name, &cid)?;
    let removed = with_files(&config, &cid, |files| {
        let unclaimed = files.get(&key).is_some_and(|e| e.get("sent") != Some(&Value::Bool(true)));
        if unclaimed {
            files.shift_remove(&key)
        } else {
            None
        }
    })?
    .flatten();
    let Some(entry) = removed else { return Err(HttpError::new(409, "not a file waiting for its message")) };
    let _ = std::fs::remove_file(conversation_dir(&config, &agent.name, &cid).join(str_of(&entry, "file")));
    Ok(web::ok(json!({"ok": true})))
}

/// `DELETE /family/s/{agent}/files`: she stopped using the agent: every file of her household with it goes.
async fn served_forget(State(state): State<AppState>, headers: HeaderMap, UrlPath(agent): UrlPath<String>) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    let cids: Vec<String> = shares::object(&shares::read(&shares::owner_path(&config)), "conversations")
        .map(|all| {
            all.iter()
                .filter(|(_, c)| str_of(c, "household") == household && str_of(c, "agent") == agent.name)
                .map(|(cid, _)| cid.clone())
                .collect()
        })
        .unwrap_or_default();
    let mut removed = 0;
    for cid in &cids {
        removed += with_files(&config, cid, |files| std::mem::take(files).len())?.unwrap_or(0);
        forget_conversation(&config, &agent.name, cid);
    }
    tracing::info!(agent = %agent.name, household = %household, removed = removed, "shared agent: a relative's files erased");
    Ok(web::ok(json!({"ok": true, "removed": removed})))
}

/// What a household's bridge may call for files (its key). Mounted with agent_shares' routes.
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/s/{agent}/conversations/{cid}/files", post(served_upload))
        .route("/family/s/{agent}/conversations/{cid}/files/{key}", get(served_file).delete(served_take_back))
        .route("/family/s/{agent}/files", delete(served_forget))
}

/// Whether `path` takes a file's body (body_limit.rs: the handler holds it to `upload_max_mb`).
pub fn is_upload_path(path: &str) -> bool {
    path.starts_with("/family/s/") && path.ends_with("/files") && path.contains("/conversations/")
}

/// Her message's files (`ids`, sent first): what her turn carries — photos with their bytes (image parts), documents
/// by the path the agent sees. 422 for a file her conversation doesn't hold or a message already has.
pub fn claim(config: &Config, agent: &str, cid: &str, ids: &[String]) -> ApiResult<Vec<Attachment>> {
    if ids.len() > FILES_MAX {
        return Err(HttpError::new(422, format!("{FILES_MAX} files a message at most")));
    }
    let data = shares::read(&shares::owner_path(config));
    let c = shares::object(&data, "conversations").and_then(|all| all.get(cid)).cloned().unwrap_or(Value::Null);
    let dir = conversation_dir(config, agent, cid);
    let mut out = Vec::new();
    for key in ids {
        let entry = KEY
            .is_match(key)
            .then(|| entry_of(&c, key))
            .flatten()
            .filter(|e| e.get("sent") != Some(&Value::Bool(true)))
            .ok_or_else(|| HttpError::new(422, "a file this conversation does not hold"))?;
        let file = str_of(&entry, "file");
        let image = str_of(&entry, "kind") == "image";
        let data = if image {
            use base64::Engine;
            let bytes = std::fs::read(dir.join(&file)).map_err(|_| HttpError::new(422, "a file this conversation does not hold"))?;
            Some(base64::engine::general_purpose::STANDARD.encode(bytes))
        } else {
            None
        };
        out.push(Attachment {
            kind: if image { "image" } else { "document" }.into(),
            name: Some(str_of(&entry, "name")),
            mime: Some(str_of(&entry, "mime")),
            size: entry.get("size").and_then(Value::as_u64),
            path: Some(seen_path(cid, &file)),
            data,
        });
    }
    Ok(out)
}

/// Her message went: its files are its own now (never taken back, read in her turns).
pub fn mark_sent(config: &Config, cid: &str, ids: &[String]) -> anyhow::Result<()> {
    with_files(config, cid, |files| {
        for key in ids {
            if let Some(Value::Object(e)) = files.get_mut(key) {
                e.insert("sent".into(), Value::Bool(true));
            }
        }
    })?;
    Ok(())
}

/// A conversation's folder gone (its entry is removed with it by its caller).
pub fn forget_conversation(config: &Config, agent: &str, cid: &str) {
    if !CONVERSATION.is_match(cid) {
        return;
    }
    let dir = conversation_dir(config, agent, cid);
    if dir.exists() {
        if let Err(err) = std::fs::remove_dir_all(&dir) {
            tracing::warn!(agent = %agent, error = %err, "shared agent: a conversation's files not erased");
        }
    }
}

/// Hermes keeps an inline photo of an API turn as a file of the agent's profile (`native-inputs/…`, named in the row
/// as `[Image attached at: <path>]`): for her photos, that copy would be within reach of the owner's own turns (file
/// tools). Once her turn is kept, the copies her conversation's rows name are erased (her own copy, on the bridge's
/// side, is what her timeline shows). Only files inside the agent's profile folder; returns how many.
pub async fn forget_hermes_copies(state: &AppState, agent: &crate::config::AgentConfig, session: &str) -> usize {
    let Ok(rows) = state.hermes.session_messages(agent, session).await else { return 0 };
    let mut paths: Vec<String> = Vec::new();
    for row in rows.iter().filter(|r| r.get("role").and_then(Value::as_str) == Some("user")) {
        let text = match row.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => {
                parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")
            }
            _ => continue,
        };
        for caps in crate::threads::timeline::IMAGE_AT.captures_iter(&text) {
            paths.push(caps[1].trim().to_string());
        }
    }
    if paths.is_empty() {
        return 0;
    }
    let profile = crate::agents_admin::profile_folder(&state.config(), agent);
    let marker = format!("/profiles/{}/", agent.name);
    let erased = tokio::task::spawn_blocking(move || {
        let Ok(root) = std::fs::canonicalize(&profile) else { return 0 };
        let mut erased = 0;
        for seen in paths {
            // the path as Hermes sees it (its container's home, maybe): the same file under the profile folder here
            let Some((_, rest)) = seen.split_once(&marker) else { continue };
            if rest.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
                continue;
            }
            let Ok(file) = std::fs::canonicalize(profile.join(rest)) else { continue };
            if file.starts_with(&root) && file.is_file() && std::fs::remove_file(&file).is_ok() {
                erased += 1;
                if let Some(dir) = file.parent().filter(|d| *d != root) {
                    let _ = std::fs::remove_dir(dir); // its folder, when empty
                }
            }
        }
        erased
    })
    .await
    .unwrap_or(0);
    if erased > 0 {
        tracing::info!(agent = %agent.name, erased = erased, "shared agent: Hermes' copies of a relative's photos erased");
    }
    erased
}

// -- the tool -----------------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Read a file the person talking to you sent in this conversation (a relative or a family \
member): give the path of their « [Pièce jointe : … → /proche/…] » line. Gives a PDF's or a text file's words; their \
photos come with their message itself. Only for the files of the conversation you are in.",
        "inputSchema": {"type": "object", "properties": {
            "path": {"type": "string", "description": "the path in their « Pièce jointe » line, e.g. /proche/c0123456789ab/0123456789ab-bulletin.pdf"}},
            "required": ["path"]},
    })
}

/// The relative whose turn this call is, for sure: the hub sees exactly one run inside the tool and it is the turn
/// of a relative's conversation with `agent` (waiting a moment for Hermes' event of the call). None: the owner's turn,
/// several turns at once, or a call the hub can't place.
async fn asker_of(state: &AppState, agent: &str) -> Option<crate::shared_pages::Asker> {
    let deadline = tokio::time::Instant::now() + WHO_WAIT;
    loop {
        let going = shares::going_turns(&state.config(), agent);
        if going.is_empty() {
            return None;
        }
        let inside = state.hub.runs_in_tool(agent, TOOL_NAME);
        match inside.as_slice() {
            [run] => return going.into_iter().find(|g| g.run == *run),
            [] if tokio::time::Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(100)).await,
            _ => return None,
        }
    }
}

/// `read_sent_file`: a file of the conversation whose relative's turn this is.
pub async fn read_call(state: &AppState, agent: &str, arguments: &Dict) -> Dict {
    let given = tk::str_or_empty(arguments.get("path"));
    let given = given.trim().trim_start_matches("MEDIA:").trim();
    let Some((cid, file, key)) = parse_seen(given) else {
        return tk::tool_text("Give the path written in their « [Pièce jointe : … → /proche/…] » line.", true);
    };
    let closed = || {
        tk::tool_text(
            "This file isn't open here: only the person who sent it can have it read, in their own conversation with you.",
            true,
        )
    };
    let Some(asker) = asker_of(state, agent).await else {
        tracing::info!(agent = %agent, "read_sent_file: not a relative's own turn: refused");
        return closed();
    };
    if asker.conversation != cid {
        tracing::info!(agent = %agent, household = %asker.household, "read_sent_file: a file of another conversation: refused");
        return closed();
    }
    let config = state.config();
    let data = shares::read(&shares::owner_path(&config));
    let c = shares::object(&data, "conversations").and_then(|all| all.get(&cid)).cloned().unwrap_or(Value::Null);
    let entry = Some(c)
        .filter(|c| str_of(c, "agent") == agent && str_of(c, "household") == asker.household)
        .and_then(|c| entry_of(&c, &key))
        .filter(|e| str_of(e, "file") == file && e.get("sent") == Some(&Value::Bool(true)));
    let Some(entry) = entry else {
        return tk::tool_text("No such file in this conversation (erased, or never sent).", true);
    };
    let (name, mime) = (str_of(&entry, "name"), str_of(&entry, "mime"));
    if str_of(&entry, "kind") == "image" {
        return tk::tool_text(
            &format!(
                "« {name} » is a photo: it came with their message (what you said of it is in this conversation). To \
                 look at it again, ask them to send it once more."
            ),
            false,
        );
    }
    let path = conversation_dir(&config, agent, &cid).join(&file);
    let (n, m) = (name.clone(), mime.clone());
    let read = tokio::task::spawn_blocking(move || document_text(&path, &n, &m)).await.unwrap_or(Read::Unreadable);
    match read {
        Read::Text(text) => {
            let shown: String = text.chars().take(TEXT_MAX).collect();
            let more = text.chars().count().saturating_sub(TEXT_MAX);
            let tail = if more > 0 { format!("\n[… {more} characters more not shown]") } else { String::new() };
            tk::tool_text(&format!("« {name} » ({mime}):\n{shown}{tail}"), false)
        }
        Read::NoText => tk::tool_text(
            &format!("« {name} » has no text to read (a scan?): ask them to send a photo of it, which you can look at."),
            false,
        ),
        Read::Unreadable => tk::tool_text(
            &format!("« {name} » ({mime}) can't be read here: a PDF, a text file or a photo can. Ask them for one of those."),
            false,
        ),
    }
}

enum Read {
    Text(String),
    NoText,
    Unreadable,
}

fn extension(name: &str) -> String {
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

/// A document's words: a PDF's (pdf-extract, its own thread, `PDF_TIMEOUT` at most), a text file's.
fn document_text(path: &Path, name: &str, mime: &str) -> Read {
    let ext = extension(name);
    if ext == "pdf" || mime == "application/pdf" {
        let Ok(bytes) = std::fs::read(path) else { return Read::Unreadable };
        return match pdf_text(bytes) {
            Some(text) if !text.trim().is_empty() => Read::Text(text),
            Some(_) => Read::NoText,
            None => Read::Unreadable,
        };
    }
    let text_like = mime.starts_with("text/")
        || ["txt", "md", "markdown", "csv", "tsv", "json", "xml", "html", "htm", "eml", "ics", "vcf", "log"].contains(&ext.as_str());
    if !text_like {
        return Read::Unreadable;
    }
    let Ok(file) = std::fs::File::open(path) else { return Read::Unreadable };
    let mut bytes = Vec::new();
    if std::io::Read::read_to_end(&mut std::io::Read::take(file, TEXT_FILE_MAX), &mut bytes).is_err() {
        return Read::Unreadable;
    }
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let text = if ["html", "htm"].contains(&ext.as_str()) || mime == "text/html" { crate::mailbox::html_text(&text) } else { text };
    if text.trim().is_empty() {
        Read::NoText
    } else {
        Read::Text(squeeze(&text))
    }
}

/// A PDF's text, None when it can't be read (a broken file, a panic of the reader, too long).
fn pdf_text(bytes: Vec<u8>) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let read = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes).ok()).ok().flatten();
        let _ = tx.send(read);
    });
    rx.recv_timeout(PDF_TIMEOUT).ok().flatten().map(|t| squeeze(&t))
}

/// Lines trimmed at their end, no more than one blank line in a row.
fn squeeze(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 || out.is_empty() {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

// -- her side: held until her message goes ------------------------------------------------------------------------

/// `POST /v1/files` for a shared agent (`id`): the file held here until her message takes it to the host.
pub fn hold(config: &Config, id: &str, source: &Path, filename: &str, content_type: Option<&str>) -> ApiResult<Value> {
    if !SHARED_ID.is_match(id) {
        return Err(HttpError::new(404, "unknown agent"));
    }
    let dir = held_dir(config, id);
    make_dir(&dir).map_err(anyhow::Error::from)?;
    let name = crate::files::sanitize_filename(filename);
    let key: String = py::uuid4_hex().chars().take(12).collect();
    let file = format!("{key}-{name}");
    let target = dir.join(&file);
    let max = config.limits.upload_max_bytes;
    let copied = (|| -> ApiResult<u64> {
        let mut input = std::fs::File::open(source).map_err(anyhow::Error::from)?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(FILE_MODE)
            .open(&target)
            .map_err(anyhow::Error::from)?;
        let size = std::io::copy(&mut std::io::Read::take(&mut input, max + 1), &mut out).map_err(anyhow::Error::from)?;
        if size > max {
            return Err(HttpError::new(413, format!("file too large (max {max} bytes)")));
        }
        Ok(size)
    })();
    let size = match copied {
        Ok(size) => size,
        Err(err) => {
            let _ = std::fs::remove_file(&target);
            return Err(err);
        }
    };
    tracing::info!(agent = %id, size = size, "a file for a shared agent held until its message goes");
    Ok(json!({"path": format!("{HELD_PREFIX}{file}"), "filename": name, "size": size,
              "content_type": content_type.unwrap_or("application/octet-stream")}))
}

/// One file of her message, read before anything goes.
pub struct Outgoing {
    kind: &'static str,
    name: String,
    mime: String,
    /// A photo's bytes (its app sends them inline).
    bytes: Option<Vec<u8>>,
    /// A document held here.
    held: Option<PathBuf>,
}

/// Her message's files as she sent them (photos inline, documents held): 422 for one that can't go.
pub fn outgoing(config: &Config, id: &str, attachments: &[Attachment]) -> ApiResult<Vec<Outgoing>> {
    if attachments.len() > FILES_MAX {
        return Err(HttpError::new(422, format!("{FILES_MAX} files a message at most")));
    }
    attachments
        .iter()
        .map(|a| {
            let mime = clean_mime(a.mime.as_deref());
            match a.kind.as_str() {
                "image" => {
                    let bytes = a
                        .data
                        .as_deref()
                        .and_then(crate::threads::send::photo_bytes)
                        .ok_or_else(|| HttpError::new(422, "a photo that can't be read"))?;
                    if bytes.len() as u64 > config.limits.upload_max_bytes {
                        return Err(HttpError::new(413, "file too large"));
                    }
                    let name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "photo.jpg".into());
                    let mime = if mime.starts_with("image/") { mime } else { "image/jpeg".into() };
                    Ok(Outgoing { kind: "image", name, mime, bytes: Some(bytes), held: None })
                }
                "document" => {
                    let file = a.path.as_deref().and_then(|p| p.strip_prefix(HELD_PREFIX)).filter(|f| stored_key(f).is_some());
                    let path = file.map(|f| held_dir(config, id).join(f)).filter(|p| p.is_file() && !p.is_symlink());
                    let path = path.ok_or_else(|| HttpError::new(422, "this file is no longer here: add it again"))?;
                    let name = a.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
                        file.map(|f| f[13..].to_string()).unwrap_or_else(|| "fichier".into())
                    });
                    Ok(Outgoing { kind: "document", name, mime, bytes: None, held: Some(path) })
                }
                _ => Err(HttpError::new(422, "photos with data, documents with a path")),
            }
        })
        .collect()
}

/// Her files sent to the host into conversation `cid`, one request each: their ids there, or the host's refusal (the
/// files it already took, taken back).
pub async fn send_files(state: &AppState, id: &str, cid: &str, files: &[Outgoing]) -> ApiResult<Result<Vec<String>, (u16, Value)>> {
    let mut sent: Vec<String> = Vec::new();
    for f in files {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("name", &f.name)
            .append_pair("mime", &f.mime)
            .append_pair("kind", f.kind)
            .finish();
        let body = match (&f.bytes, &f.held) {
            (Some(bytes), _) => reqwest::Body::from(bytes.clone()),
            (None, Some(path)) => {
                let file = tokio::fs::File::open(path).await.map_err(|_| HttpError::new(422, "this file is no longer here: add it again"))?;
                reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file))
            }
            _ => continue,
        };
        let request = shares::owner_request(state, id, reqwest::Method::POST, &format!("conversations/{cid}/files?{query}"))?;
        let answer = request
            .timeout(Duration::from_secs(600))
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await;
        let refused = match answer {
            Ok(resp) if resp.status().is_success() => {
                let made = resp.json::<Value>().await.unwrap_or(Value::Null);
                match made.get("id").and_then(Value::as_str) {
                    Some(key) => {
                        sent.push(key.to_string());
                        continue;
                    }
                    None => (502, json!({"detail": "their side did not keep the file"})),
                }
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                (status, resp.json::<Value>().await.unwrap_or(json!({"detail": "their side refused the file"})))
            }
            Err(_) => (502, json!({"detail": "their side does not answer right now: try again later"})),
        };
        take_back(state, id, cid, &sent).await;
        return Ok(Err(refused));
    }
    Ok(Ok(sent))
}

/// Files the host took for a message that didn't go, taken back (best effort: else its purge, after an hour).
pub async fn take_back(state: &AppState, id: &str, cid: &str, keys: &[String]) {
    for key in keys {
        if let Ok(request) = shares::owner_request(state, id, reqwest::Method::DELETE, &format!("conversations/{cid}/files/{key}")) {
            let _ = request.timeout(Duration::from_secs(10)).send().await;
        }
    }
}

/// Her message went with its files: the copies held here go.
pub fn sent(files: &[Outgoing]) {
    for path in files.iter().filter_map(|f| f.held.as_ref()) {
        let _ = std::fs::remove_file(path);
    }
}

/// She stops using the agent: the host asked to erase her files with it (best effort), the ones held here gone.
pub async fn forget_mine(state: &AppState, id: &str) {
    if let Ok(request) = shares::owner_request(state, id, reqwest::Method::DELETE, "files") {
        match request.timeout(Duration::from_secs(10)).send().await {
            Ok(resp) if resp.status().is_success() => {}
            _ => tracing::info!(agent = %id, "her files not erased on the host's side now: its purge will"),
        }
    }
    if SHARED_ID.is_match(id) {
        let _ = std::fs::remove_dir_all(held_dir(&state.config(), id));
    }
}

/// Her relayed timeline: each file of her conversation (`/proche/<cid>/…`) with a url on this bridge.
pub fn urls_under(id: &str, mut page: Value) -> Value {
    if let Some(Value::Array(items)) = page.get_mut("items") {
        for item in items {
            if let Some(Value::Array(atts)) = item.get_mut("attachments") {
                for a in atts {
                    if let Some((cid, _, key)) = parse_seen(&str_of(a, "path")) {
                        a["url"] = json!(format!("/v1/shared-agents/{id}/files/{cid}/{key}"));
                    }
                }
            }
        }
    }
    page
}

/// `GET /v1/shared-agents/{id}/files/{cid}/{key}`: a file of her conversation, from the host.
async fn relayed_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((id, cid, key)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    if !CONVERSATION.is_match(&cid) || !KEY.is_match(&key) {
        return Err(HttpError::not_found());
    }
    let mut request = shares::owner_request(&state, &id, reqwest::Method::GET, &format!("conversations/{cid}/files/{key}"))?
        .timeout(Duration::from_secs(600));
    for name in ["if-none-match", "if-modified-since", "range"] {
        if let Some(v) = headers.get(name) {
            request = request.header(name, v.clone());
        }
    }
    let resp = request.send().await.map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    if !(resp.status().is_success() || status == 304) {
        return Err(HttpError::new(if status == 404 { 404 } else { 502 }, "this file can't be had right now"));
    }
    let kept: Vec<(String, HeaderValue)> = ["content-type", "content-length", "content-disposition", "etag", "last-modified",
                                            "accept-ranges", "content-range"]
        .iter()
        .filter_map(|n| resp.headers().get(*n).map(|v| (n.to_string(), v.clone())))
        .collect();
    let mut response = Response::new(Body::from_stream(resp.bytes_stream()));
    *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap_or(axum::http::StatusCode::OK);
    let h = response.headers_mut();
    for (name, value) in kept {
        if let Ok(name) = axum::http::HeaderName::from_bytes(name.as_bytes()) {
            h.insert(name, value);
        }
    }
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("content-security-policy", HeaderValue::from_static("sandbox; default-src 'none'"));
    h.insert("cache-control", HeaderValue::from_static("private, max-age=86400"));
    Ok(response)
}

/// For her app (under /v1, the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/shared-agents/{id}/files/{cid}/{key}", get(relayed_file))
}

// -- the purge ----------------------------------------------------------------------------------------------------

/// When a file was last written (Unix time).
fn written_at(file: &std::fs::DirEntry) -> Option<f64> {
    let modified = file.metadata().ok()?.modified().ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64())
}

/// The files that should be gone, erased (app.rs `purge_loop`): held ones past a day; on the host, the folders of
/// conversations gone or no longer shared, files past the host's `kept_days` (photos `PHOTO_DAYS`), files no message
/// claimed within an hour, and any file its conversation doesn't list. Returns how many.
pub fn purge(config: &Config, at: f64) -> usize {
    let mut removed = 0;
    // held here for a message that never went
    let held = PathBuf::from(config.data_dir()).join("shared-held");
    for dir in std::fs::read_dir(&held).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let old = written_at(&file).is_some_and(|t| at - t > HELD_SECONDS);
            if old && std::fs::remove_file(file.path()).is_ok() {
                removed += 1;
            }
        }
    }
    let base = root(config);
    let Ok(agents) = std::fs::read_dir(&base) else { return removed };
    let kept_days = crate::files_kept::kept_days(config);
    let expired = |e: &Value| {
        let age = at - e.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        (e.get("sent") != Some(&Value::Bool(true)) && age > UNCLAIMED_SECONDS)
            || (kept_days > 0 && age > kept_days as f64 * 86_400.0)
            || (str_of(e, "kind") == "image" && age > crate::files_kept::PHOTO_DAYS * 86_400.0)
    };
    for agent_dir in agents.flatten() {
        let agent = agent_dir.file_name().to_string_lossy().into_owned();
        let shared: Vec<String> = shares::shared_with(config, &agent);
        let data = shares::read(&shares::owner_path(config));
        let conversations = shares::object(&data, "conversations").cloned().unwrap_or_default();
        for conv_dir in std::fs::read_dir(agent_dir.path()).into_iter().flatten().flatten() {
            let cid = conv_dir.file_name().to_string_lossy().into_owned();
            let c = conversations.get(&cid).filter(|c| str_of(c, "agent") == agent && shared.contains(&str_of(c, "household")));
            let Some(c) = c else {
                // its conversation gone, or its household no longer shared with (unlinked)
                removed += std::fs::read_dir(conv_dir.path()).map(|d| d.count()).unwrap_or(0);
                let _ = std::fs::remove_dir_all(conv_dir.path());
                continue;
            };
            let listed: Map<String, Value> = c.get("files").and_then(Value::as_object).cloned().unwrap_or_default();
            let gone: Vec<String> = listed.iter().filter(|(_, e)| expired(e)).map(|(k, _)| k.clone()).collect();
            if !gone.is_empty() {
                let _ = with_files(config, &cid, |files| {
                    for key in &gone {
                        files.shift_remove(key);
                    }
                });
            }
            let keep: Vec<String> =
                listed.iter().filter(|(k, _)| !gone.contains(k)).map(|(_, e)| str_of(e, "file")).collect();
            for file in std::fs::read_dir(conv_dir.path()).into_iter().flatten().flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                // one it doesn't list: an expired one, or one an hour old that never got listed (one arriving now
                // is listed once it is whole)
                let listed_once = listed.values().any(|e| str_of(e, "file") == name);
                let stale = listed_once || written_at(&file).is_some_and(|w| at - w > UNCLAIMED_SECONDS);
                if !keep.contains(&name) && stale && std::fs::remove_file(file.path()).is_ok() {
                    removed += 1;
                }
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seen_path_names_its_conversation_and_a_stored_file_only() {
        let path = seen_path("c0123456789ab", "0123456789ab-bulletin.pdf");
        assert_eq!(path, "/proche/c0123456789ab/0123456789ab-bulletin.pdf");
        assert_eq!(
            parse_seen(&path),
            Some(("c0123456789ab".into(), "0123456789ab-bulletin.pdf".into(), "0123456789ab".into()))
        );
        assert!(in_conversation(&path, "c0123456789ab") && !in_conversation(&path, "c0123456789ac"));
        for bad in ["/proche/c0123456789ab/../x", "/proche/c01/0123456789ab-x.pdf", "/proche/c0123456789ab/notes.txt",
                    "/fichiers/2026-11/0123456789ab-x.pdf", "/proche/c0123456789ab/0123456789ab-a/b"]
        {
            assert_eq!(parse_seen(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_kind_and_a_mime_are_taken_as_safe_words_only() {
        assert_eq!(clean_mime(Some("Application/PDF")), "application/pdf");
        assert_eq!(clean_mime(Some("text/html; charset=x")), "application/octet-stream");
        assert_eq!(clean_mime(None), "application/octet-stream");
        assert_eq!(kind_of(None, "image/jpeg"), "image");
        assert_eq!(kind_of(Some("document"), "image/jpeg"), "document");
        assert_eq!(kind_of(Some("x"), "application/pdf"), "document");
    }

    #[test]
    fn a_text_file_reads_and_an_unknown_kind_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("a.txt");
        std::fs::write(&txt, "Inscription au foot\n\n\n\nSamedi 10 h   \n").unwrap();
        assert!(matches!(document_text(&txt, "inscription.txt", "text/plain"), Read::Text(t) if t == "Inscription au foot\n\nSamedi 10 h"));
        let doc = dir.path().join("b.docx");
        std::fs::write(&doc, b"PK\x03\x04").unwrap();
        assert!(matches!(document_text(&doc, "fiche.docx", "application/octet-stream"), Read::Unreadable));
        let broken = dir.path().join("c.pdf");
        std::fs::write(&broken, b"%PDF-1.4 nothing").unwrap();
        assert!(matches!(document_text(&broken, "c.pdf", "application/pdf"), Read::Unreadable | Read::NoText));
    }

    #[test]
    fn a_held_file_keeps_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        config.limits.upload_max_bytes = 10;
        let source = dir.path().join("in");
        std::fs::write(&source, b"0123456789").unwrap();
        let held = hold(&config, "maison__alex", &source, "bulletin trimestre.pdf", Some("application/pdf")).unwrap();
        let path = held["path"].as_str().unwrap();
        assert!(path.starts_with(HELD_PREFIX) && path.ends_with("-bulletin_trimestre.pdf"), "{path}");
        std::fs::write(&source, b"0123456789+").unwrap();
        assert_eq!(hold(&config, "maison__alex", &source, "x.pdf", None).unwrap_err().status.as_u16(), 413);
        assert_eq!(hold(&config, "../x", &source, "x.pdf", None).unwrap_err().status.as_u16(), 404);
        let files: Vec<_> = std::fs::read_dir(held_dir(&config, "maison__alex")).unwrap().collect();
        assert_eq!(files.len(), 1, "the refused one left nothing");
    }
}
