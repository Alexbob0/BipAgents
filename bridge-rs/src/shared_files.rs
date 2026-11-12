//! Files sent to a shared agent, the family's included (Alex, 11/11-12/11; internal/notes/shared-agent-files.md). ONE
//! rule for every agent that more than one person uses: the files and photos sent to it are visible to everyone who
//! uses it (its owner included), with who sent them, and the agent may use them in anyone's turn; the messages stay
//! in each sender's own conversation.
//!
//! - **The pool**: each shared agent has one, on its owner's place: `<data>/shared-files/<agent>/<id12>-<name>`
//!   (folders 0700, files 0600, an on-device OCR text beside a scanned PDF as `<id12>.txt`), listed in
//!   `<data>/shared-files.json` `{agents: {<agent>: {<id12>: {file, name, mime, size, kind, at, sent, household,
//!   person, conversation?, own_copy?}}}}` (`household` "" is the owner). Never in the agent's uploads, sandbox or
//!   Hermes profile.
//! - **A relative's files**: her bridge holds a document her app stored first (`POST /v1/files` with
//!   `agent=<agent>__<household>`: `<data>/shared-held/<id>/<id12>-<name>`, answered as `/bipagents/held/…`) and, with
//!   her message, sends each file to the owner's bridge, then the message naming them (agent_shares.rs `relay_send`);
//!   the held copy goes once the message went, or after a day.
//! - **The owner's files** to his shared agent go to his own conversation as to any of his agents (its uploads, its own
//!   tools read them) and, when his message goes, into the pool too ([`share_own_files`]: a hard link, else a copy).
//! - **A turn** gets the sender's photos as image parts and each document as one line
//!   `[Pièce jointe : name (mime, size) → /famille/<id12>-<name>]`. The agent reads any file of the pool with
//!   [`TOOL_NAME`] in its owner's or any relative's turn (never another household's agent's or a discussion's): a
//!   document's words (doc_text.rs: PDF, Word, Excel, PowerPoint, OpenDocument, text; a scan's OCR text), a photo as an
//!   image; with no path, the list of the pool (who sent what, when).
//! - **The apps** show the pool on the agent's page, « Fichiers », each with who sent it; the sender may delete hers.
//!   Her timeline keeps her own files with a url on her bridge.
//! - **They go** when their sender deletes them, when the agent stops being shared with that household (or with
//!   anyone, or the agent goes), when she stops using it, with a household unlinked, after the owner's `kept_days`
//!   (files_kept.rs, 30 days by default; photos 30 days), and a file no message took after an hour ([`purge`]). A
//!   conversation deleted leaves the files (they are everyone's).
//!
//! The same limits as files to one's own agents: `[limits] upload_max_mb` per file, ten a message, the place's room.
//!
//! Between the bridges (the household key):
//!
//!     POST   /family/s/{agent}/conversations/{cid}/files?name&mime&kind   raw body → {id, name, mime, size, kind, path}
//!     DELETE /family/s/{agent}/conversations/{cid}/files/{id}             {ok}  one no message took (taken back)
//!     GET    /family/s/{agent}/files                                      {files: [item]}  the pool
//!     GET    /family/s/{agent}/files/{id}                                 the file
//!     DELETE /family/s/{agent}/files/{id}                                 {ok}  one of hers
//!     DELETE /family/s/{agent}/files                                      {ok, removed}  all of hers
//!
//!     item  {id, name, mime, size, kind, at, from, mine, url}
//!
//! For the apps (under /v1, the bridge key): the owner's `GET /v1/agents/{agent}/shared-files`, `GET|DELETE
//! /v1/agents/{agent}/shared-files/{id}`; the relative's `GET /v1/shared-agents/{id}/files`, `GET|DELETE
//! /v1/shared-agents/{id}/files/{file}`, relayed.
//!
//! No Python counterpart.

use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
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
use crate::config::{AgentConfig, Config};
use crate::doc_text::Read;
use crate::threads::send::Attachment;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The bridge tool reading a file of the agent's pool.
pub const TOOL_NAME: &str = "read_sent_file";
/// Where the agent sees a file of its pool: `/famille/<id12>-<name>` (no such folder anywhere: the tool reads it).
pub const SEEN_PREFIX: &str = "/famille/";
/// What her bridge answers her app for a document it holds until her message goes.
pub const HELD_PREFIX: &str = "/bipagents/held/";
/// Files one message carries at most (threads/send.rs `ATTACHMENTS_MAX`).
pub const FILES_MAX: usize = 10;
/// The sender `household` of the agent's owner.
pub const OWNER: &str = "";
/// A file sent to the host that no message took (her message refused, her bridge gone meanwhile) goes after this.
const UNCLAIMED_SECONDS: f64 = 3600.0;
/// A held file not sent goes after this.
const HELD_SECONDS: f64 = 86_400.0;
/// Characters of a document the tool gives at once.
const TEXT_MAX: usize = 60_000;
/// Characters of an on-device OCR text kept.
const OCR_MAX: usize = 400_000;
/// A photo the tool shows at most (larger: named only).
const IMAGE_MAX: u64 = 8 * 1024 * 1024;
/// Files the tool lists at most (the latest).
const LIST_MAX: usize = 60;
const DIR_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;

static LOCK: Mutex<()> = Mutex::new(());
static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{12}\z").expect("valid regex"));
/// An agent's name or a shared agent's id on her side (`cuisine__alex`), as a folder name.
static FOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,140}\z").expect("valid regex"));

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

/// The pool's folder of `agent` on its owner's place.
pub fn pool_dir(config: &Config, agent: &str) -> PathBuf {
    root(config).join(agent)
}

fn index_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-files.json")
}

fn held_dir(config: &Config, id: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-held").join(id)
}

/// The path the agent is given for a file of its pool.
pub fn seen_path(file: &str) -> String {
    format!("{SEEN_PREFIX}{file}")
}

/// `(file, key)` of a seen path; None for anything else.
pub fn parse_seen(path: &str) -> Option<(String, String)> {
    let file = path.strip_prefix(SEEN_PREFIX)?;
    stored_key(file).map(|key| (file.to_string(), key))
}

/// A stored name's key (its 12 hex before `-`), when it is one the bridge wrote.
fn stored_key(file: &str) -> Option<String> {
    let ok = crate::files::STORED_NAME.is_match(file)
        && !file.contains(['/', '\\', '\0', '\n'])
        && file.chars().count() <= 140;
    ok.then(|| file[..12].to_string())
}

/// Whether `path` is a file of a pool (the attachments a relative's timeline keeps).
pub fn is_pool_path(path: &str) -> bool {
    parse_seen(path).is_some()
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

// -- the index ----------------------------------------------------------------------------------------------------

fn load(path: &Path) -> Map<String, Value> {
    let data: Value =
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    data.get("agents").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn save(path: &Path, agents: &Map<String, Value>) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    py::write_with_mode(
        &tmp.to_string_lossy(),
        py::Dumps::new().unicode().dumps(&json!({"agents": agents})).as_bytes(),
        0o600,
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A change of `agent`'s pool, under the lock.
fn update<R>(config: &Config, agent: &str, change: impl FnOnce(&mut Map<String, Value>) -> R) -> anyhow::Result<R> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = index_path(config);
    let mut agents = load(&path);
    let entry = agents.entry(agent.to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    let out = match entry {
        Value::Object(files) => change(files),
        _ => unreachable!("an object was just put there"),
    };
    if agents.get(agent).and_then(Value::as_object).is_some_and(Map::is_empty) {
        agents.shift_remove(agent);
    }
    save(&path, &agents)?;
    Ok(out)
}

/// `agent`'s pool: key → entry.
pub fn files_of(config: &Config, agent: &str) -> Map<String, Value> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(&index_path(config)).get(agent).and_then(Value::as_object).cloned().unwrap_or_default()
}

fn remove_files(config: &Config, agent: &str, entries: &[Value]) {
    let dir = pool_dir(config, agent);
    for e in entries {
        let file = str_of(e, "file");
        if stored_key(&file).is_some() {
            let _ = std::fs::remove_file(dir.join(&file));
            let _ = std::fs::remove_file(dir.join(format!("{}.txt", &file[..12])));
        }
    }
}

/// The files of `agent`'s pool that `keep` refuses, gone (index and disk). Returns how many.
fn forget_where(config: &Config, agent: &str, keep: impl Fn(&Value) -> bool) -> usize {
    let gone = update(config, agent, |files| {
        let gone: Vec<Value> = files.values().filter(|e| !keep(e)).cloned().collect();
        files.retain(|_, e| keep(e));
        gone
    })
    .unwrap_or_default();
    remove_files(config, agent, &gone);
    gone.len()
}

/// Every file of `agent`'s pool gone: shared with nobody any more, or the agent itself gone.
pub fn forget_agent(config: &Config, agent: &str) {
    if !FOLDER.is_match(agent) {
        return;
    }
    let _ = update(config, agent, |files| files.clear());
    let _ = std::fs::remove_dir_all(pool_dir(config, agent));
}

/// The households `agent` is no longer shared with: their files gone.
pub fn forget_households(config: &Config, agent: &str, gone: &[String]) {
    if !gone.is_empty() {
        forget_where(config, agent, |e| !gone.contains(&str_of(e, "household")));
    }
}

/// Whether `agent` keeps a pool: shared with at least one household (the family's agent always).
pub fn pooled(config: &Config, agent: &str) -> bool {
    !shares::shared_with(config, agent).is_empty()
}

// -- writing ------------------------------------------------------------------------------------------------------

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

/// A scan's text recognised on the phone, kept beside its file.
fn keep_ocr(config: &Config, agent: &str, key: &str, text: Option<&str>) {
    let Some(text) = text.map(str::trim).filter(|t| !t.is_empty()) else { return };
    let text: String = text.chars().take(OCR_MAX).collect();
    let path = pool_dir(config, agent).join(format!("{key}.txt"));
    if let Err(err) = py::write_with_mode(&path.to_string_lossy(), text.as_bytes(), FILE_MODE) {
        tracing::warn!(agent = %agent, error = %err, "shared agent: a scan's text not kept");
    }
}

/// `POST /family/s/{agent}/conversations/{cid}/files?name&mime&kind`: a file of her coming message, into the agent's
/// pool (not taken yet: her message names it).
async fn served_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
    q: web::QueryMap,
    body: Body,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    let c = if crate::family_thread::is_common(&config, &agent.name) {
        // a family agent's one conversation (family_thread.rs): her entry made with her first file if need be
        let key = crate::family_thread::entry(&config, &agent.name, &household, None)?;
        shares::object(&shares::read(&shares::owner_path(&config)), "conversations")
            .and_then(|all| all.get(&key))
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        shares::conversation(&config, &household, &agent.name, &cid)?
    };
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
    let dir = pool_dir(&config, &agent.name);
    let size = write_stream(&dir, &dir.join(&file), body, config.limits.upload_max_bytes).await?;
    let person = Some(str_of(&c, "person"))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| shares::shown_household(&config, &household));
    let entry = json!({"file": file, "name": name, "mime": mime, "size": size, "kind": kind, "at": now(), "sent": false,
                       "household": household, "person": person, "conversation": cid});
    update(&config, &agent.name, |files| files.insert(key.clone(), entry))?;
    tracing::info!(agent = %agent.name, household = %household, size = size, kind = kind, "shared agent: a relative's file kept");
    Ok(web::ok(json!({"id": key, "name": name, "mime": mime, "size": size, "kind": kind, "path": seen_path(&file)})))
}

/// `DELETE /family/s/{agent}/conversations/{cid}/files/{id}`: a file her bridge sent for a message that didn't go
/// (taken back; one a message took stays).
async fn served_take_back(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid, key)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let config = state.config();
    let unclaimed = |e: &Value| {
        str_of(e, "household") == household
            && str_of(e, "conversation") == cid
            && e.get("sent") != Some(&Value::Bool(true))
    };
    let taken = update(&config, &agent.name, |files| match files.get(&key) {
        Some(e) if unclaimed(e) => files.shift_remove(&key),
        _ => None,
    })?;
    let Some(entry) = taken else { return Err(HttpError::new(409, "not a file waiting for its message")) };
    remove_files(&config, &agent.name, &[entry]);
    Ok(web::ok(json!({"ok": true})))
}

/// Her message's files (`ids`, sent first into conversation `cid`, an OCR text maybe): what her turn carries — photos
/// with their bytes (image parts), documents by the path the agent sees. 422 for a file she didn't send for this
/// conversation, or one a message already took.
pub fn claim(
    config: &Config,
    agent: &str,
    household: &str,
    cid: &str,
    ids: &[(String, Option<String>)],
) -> ApiResult<Vec<Attachment>> {
    if ids.len() > FILES_MAX {
        return Err(HttpError::new(422, format!("{FILES_MAX} files a message at most")));
    }
    let files = files_of(config, agent);
    let dir = pool_dir(config, agent);
    let mut out = Vec::new();
    for (key, text) in ids {
        let refused = || HttpError::new(422, "a file this conversation did not send");
        let entry = Some(key)
            .filter(|k| KEY.is_match(k))
            .and_then(|k| files.get(k.as_str()))
            .filter(|e| str_of(e, "household") == household && str_of(e, "conversation") == cid)
            .filter(|e| e.get("sent") != Some(&Value::Bool(true)))
            .ok_or_else(refused)?;
        let file = str_of(entry, "file");
        let image = str_of(entry, "kind") == "image";
        let data = if image {
            use base64::Engine;
            let bytes = std::fs::read(dir.join(&file)).map_err(|_| refused())?;
            Some(base64::engine::general_purpose::STANDARD.encode(bytes))
        } else {
            keep_ocr(config, agent, key, text.as_deref());
            None
        };
        out.push(Attachment {
            kind: if image { "image" } else { "document" }.into(),
            name: Some(str_of(entry, "name")),
            mime: Some(str_of(entry, "mime")),
            size: entry.get("size").and_then(Value::as_u64),
            path: Some(seen_path(&file)),
            data,
            text: None,
        });
    }
    Ok(out)
}

/// Her message went: its files are in the pool now, for everyone who uses the agent.
pub fn mark_sent(config: &Config, agent: &str, ids: &[String]) -> anyhow::Result<()> {
    update(config, agent, |files| {
        for key in ids {
            if let Some(Value::Object(e)) = files.get_mut(key) {
                e.insert("sent".into(), Value::Bool(true));
            }
        }
    })
}

/// The owner's message to his shared agent went with files (stored for it as for any of his agents: its uploads, a
/// photo kept by threads/send.rs `keep_photos`): each also into the pool, a hard link of the upload (else a copy), so
/// everyone who uses the agent sees it and the agent reads it in anyone's turn. `person`: the owner's first name.
pub fn share_own_files(config: &Config, agent: &AgentConfig, attachments: &[Attachment], person: &str) {
    if attachments.iter().all(|a| a.path.is_none()) || !pooled(config, &agent.name) {
        return;
    }
    let (Some(host), Some(seen)) = (
        agent.upload_dir_host.as_deref().filter(|h| !h.is_empty()),
        agent.upload_dir_container.as_deref().filter(|c| !c.is_empty()),
    ) else {
        return;
    };
    let seen_root = format!("{}/", seen.trim_end_matches('/'));
    let dir = pool_dir(config, &agent.name);
    if make_dir(&dir).is_err() {
        return;
    }
    for a in attachments {
        let Some(path) = a.path.as_deref() else { continue };
        let Some(rest) = path.strip_prefix(&seen_root).filter(|r| !r.split('/').any(|p| p.is_empty() || p == ".."))
        else {
            continue;
        };
        let source = Path::new(host).join(rest);
        let regular = std::fs::symlink_metadata(&source).is_ok_and(|m| m.file_type().is_file());
        if !regular {
            continue;
        }
        let name = crate::files::sanitize_filename(
            a.name.as_deref().unwrap_or_else(|| rest.rsplit('/').next().unwrap_or("fichier")),
        );
        let key: String = py::uuid4_hex().chars().take(12).collect();
        let file = format!("{key}-{name}");
        let target = dir.join(&file);
        if std::fs::hard_link(&source, &target).is_err() && std::fs::copy(&source, &target).is_err() {
            tracing::warn!(agent = %agent.name, "shared agent: an owner's file not put in the pool");
            continue;
        }
        let _ = std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(FILE_MODE));
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        let mime = clean_mime(a.mime.as_deref());
        let kind = if a.kind == "image" { "image" } else { "document" };
        let entry = json!({"file": file, "name": name, "mime": mime, "size": size, "kind": kind, "at": now(), "sent": true,
                           "household": OWNER, "person": person, "own_copy": path});
        if update(config, &agent.name, |files| files.insert(key.clone(), entry)).is_ok() && kind == "document" {
            keep_ocr(config, &agent.name, &key, a.text.as_deref());
        }
    }
}

/// A file a family agent made (an audio, a podcast: `/bipagents/audio/<agent>/<file>`) into its pool, a hard link
/// (else a copy): the family's, so every member's bridge serves it from the common conversation (family_thread.rs)
/// and it shows in its « Fichiers », sent by the agent (household `OWNER`, its name). Kept the pool's time, as any.
pub fn share_made(config: &Config, agent: &str, path: &str, title: Option<&str>) {
    let prefix = format!("{}/{agent}/", crate::agent_tools::MEDIA_PREFIX);
    let Some(file_name) = path.strip_prefix(&prefix).filter(|f| !f.is_empty() && !f.contains('/') && !f.starts_with('.'))
    else {
        return;
    };
    let source = crate::agent_tools::audio_root(config).join(agent).join(file_name);
    if !std::fs::symlink_metadata(&source).is_ok_and(|m| m.file_type().is_file()) {
        return;
    }
    let dir = pool_dir(config, agent);
    if make_dir(&dir).is_err() {
        return;
    }
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let name = crate::files::sanitize_filename(&match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t}.{ext}"),
        None => file_name.to_string(),
    });
    let key: String = py::uuid4_hex().chars().take(12).collect();
    let file = format!("{key}-{name}");
    let target = dir.join(&file);
    if std::fs::hard_link(&source, &target).is_err() && std::fs::copy(&source, &target).is_err() {
        tracing::warn!(agent = %agent, "family agent: a made file not put in its pool");
        return;
    }
    let _ = std::fs::set_permissions(&target, std::os::unix::fs::PermissionsExt::from_mode(FILE_MODE));
    let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
    let mime = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        _ => "application/octet-stream",
    };
    let shown = config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.to_string());
    let entry = json!({"file": file, "name": name, "mime": mime, "size": size, "kind": "document", "at": now(),
                       "sent": true, "household": OWNER, "person": shown, "own_copy": path, "made": true});
    if update(config, agent, |files| files.insert(key, entry)).is_err() {
        tracing::warn!(agent = %agent, "family agent: a made file not noted in its pool");
    }
}

// -- the pool, as the apps see it ---------------------------------------------------------------------------------

/// The pool's files that went (the latest first), as the apps list them: `mine` for those `household` sent, `url`
/// made by `url`.
fn listed(config: &Config, agent: &str, household: &str, url: impl Fn(&str) -> String) -> Vec<Value> {
    let mut files: Vec<(String, Value)> =
        files_of(config, agent).into_iter().filter(|(_, e)| e.get("sent") == Some(&Value::Bool(true))).collect();
    files.sort_by(|a, b| {
        let at = |e: &Value| e.get("at").and_then(Value::as_f64).unwrap_or(0.0);
        at(&b.1).total_cmp(&at(&a.1))
    });
    files
        .into_iter()
        .map(|(key, e)| {
            json!({"id": key, "name": e.get("name"), "mime": e.get("mime"), "size": e.get("size"), "kind": e.get("kind"),
                   "at": e.get("at"), "from": e.get("person"), "mine": str_of(&e, "household") == household,
                   "url": url(&key)})
        })
        .collect()
}

/// One sent file of the pool, as a response (its name, its type, the apps' cache marks).
async fn file_response(config: &Config, agent: &str, key: &str, headers: &HeaderMap) -> ApiResult<Response> {
    let entry = KEY
        .is_match(key)
        .then(|| files_of(config, agent).get(key).cloned())
        .flatten()
        .filter(|e| e.get("sent") == Some(&Value::Bool(true)))
        .ok_or_else(HttpError::not_found)?;
    let path = pool_dir(config, agent).join(str_of(&entry, "file"));
    let regular = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
    if !regular {
        return Err(HttpError::not_found());
    }
    let extra = [
        ("x-content-type-options", "nosniff".to_string()),
        ("content-security-policy", "sandbox; default-src 'none'".to_string()),
    ];
    let name = str_of(&entry, "name");
    Ok(web::file_response(&path.to_string_lossy(), &str_of(&entry, "mime"), Some(&name), &extra, headers).await)
}

/// Deleting one of the pool's files: only by the household that sent it.
fn delete_one(config: &Config, agent: &str, household: &str, key: &str) -> ApiResult<()> {
    let removed = update(config, agent, |files| match files.get(key) {
        Some(e) if str_of(e, "household") == household => files.shift_remove(key),
        _ => None,
    })?;
    let entry = removed.ok_or_else(|| HttpError::new(404, "no such file of yours"))?;
    remove_files(config, agent, &[entry]);
    tracing::info!(agent = %agent, "shared agent: a file deleted by its sender");
    Ok(())
}

async fn served_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let files = listed(&state.config(), &agent.name, &household, |_| String::new());
    Ok(web::ok(json!({"files": files})))
}

async fn served_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (_, agent) = shares::shared_agent(&state, &headers, &agent)?;
    file_response(&state.config(), &agent.name, &key, &headers).await
}

async fn served_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    delete_one(&state.config(), &agent.name, &household, &key)?;
    Ok(web::ok(json!({"ok": true})))
}

/// `DELETE /family/s/{agent}/files`: she stopped using the agent: every file of her household in its pool goes.
async fn served_forget(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
) -> ApiResult<Response> {
    let (household, agent) = shares::shared_agent(&state, &headers, &agent)?;
    let removed = forget_where(&state.config(), &agent.name, |e| str_of(e, "household") != household);
    tracing::info!(agent = %agent.name, household = %household, removed = removed, "shared agent: a relative's files erased");
    Ok(web::ok(json!({"ok": true, "removed": removed})))
}

/// What a household's bridge may call for files (its key). Mounted with agent_shares' routes.
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/s/{agent}/conversations/{cid}/files", post(served_upload))
        .route("/family/s/{agent}/conversations/{cid}/files/{key}", delete(served_take_back))
        .route("/family/s/{agent}/files", get(served_list).delete(served_forget))
        .route("/family/s/{agent}/files/{key}", get(served_file).delete(served_delete))
}

/// Whether `path` takes a file's body (body_limit.rs: the handler holds it to `upload_max_mb`).
pub fn is_upload_path(path: &str) -> bool {
    path.starts_with("/family/s/") && path.ends_with("/files") && path.contains("/conversations/")
}

// -- the owner's app ----------------------------------------------------------------------------------------------

fn own_agent(state: &AppState, agent: &str) -> ApiResult<AgentConfig> {
    state.config().agent(Some(agent)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

async fn own_list(State(state): State<AppState>, UrlPath(agent): UrlPath<String>) -> ApiResult<Response> {
    let agent = own_agent(&state, &agent)?;
    let name = agent.name.clone();
    let files = listed(&state.config(), &agent.name, OWNER, |key| format!("/v1/agents/{name}/shared-files/{key}"));
    Ok(web::ok(json!({"files": files})))
}

async fn own_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = own_agent(&state, &agent)?;
    file_response(&state.config(), &agent.name, &key, &headers).await
}

async fn own_delete(
    State(state): State<AppState>,
    UrlPath((agent, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = own_agent(&state, &agent)?;
    delete_one(&state.config(), &agent.name, OWNER, &key)?;
    Ok(web::ok(json!({"ok": true})))
}

// -- the tool -----------------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "The files and photos sent to you by everyone who uses you (your person and the relatives you are \
    shared with): with a path (from a « [Pièce jointe : … → /famille/…] » line, or the list), a document's words (PDF, \
    Word, Excel, PowerPoint, OpenDocument, text; a scan's text recognised on the phone) or a photo; with no path, the list: \
    each file, who sent it, when, its path.",
        "inputSchema": {"type": "object", "properties": {
            "path": {"type": "string", "description": "e.g. /famille/0123456789ab-bulletin.pdf; none: the list"}}},
    })
}

/// The pool's files that went, as the tool lists them.
fn list_text(config: &Config, agent: &str) -> String {
    let all = listed(config, agent, "\u{0}", |_| String::new());
    if all.is_empty() {
        return "Nobody has sent you a file yet.".into();
    }
    let files = files_of(config, agent);
    let lines: Vec<String> = all
        .iter()
        .take(LIST_MAX)
        .map(|f| {
            let id = str_of(f, "id");
            let file = files.get(&id).map(|e| str_of(e, "file")).unwrap_or_default();
            let at = f.get("at").and_then(Value::as_f64).unwrap_or(0.0);
            let day = chrono::DateTime::from_timestamp(at as i64, 0)
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            format!(
                "- « {} » ({}) from {}, {day}: {}",
                str_of(f, "name"),
                str_of(f, "kind"),
                str_of(f, "from"),
                seen_path(&file)
            )
        })
        .collect();
    format!("The files sent to you ({}), the latest first:\n{}", all.len(), lines.join("\n"))
}

/// `read_sent_file` in a turn of the agent's owner or of a relative it is shared with (agent_tools.rs keeps it from
/// other households' agents and discussions).
pub async fn read_call(state: &AppState, agent: &str, arguments: &Dict) -> Dict {
    let config = state.config();
    let given = tk::str_or_empty(arguments.get("path"));
    let given = given.trim().trim_start_matches("MEDIA:").trim().to_string();
    if given.is_empty() {
        return tk::tool_text(&list_text(&config, agent), false);
    }
    let files = files_of(&config, agent);
    // by its pool path, or (one of its owner's) by the path of his own upload
    let found = match parse_seen(&given) {
        Some((file, key)) => files.get(&key).filter(|e| str_of(e, "file") == file).map(|e| (key, e.clone())),
        None => files.iter().find(|(_, e)| str_of(e, "own_copy") == given).map(|(k, e)| (k.clone(), e.clone())),
    };
    let Some((key, entry)) = found.filter(|(_, e)| e.get("sent") == Some(&Value::Bool(true))) else {
        return tk::tool_text(
            "No such file sent to you (erased, or never sent). Call read_sent_file with no path for the list.",
            true,
        );
    };
    let (name, mime, from) = (str_of(&entry, "name"), str_of(&entry, "mime"), str_of(&entry, "person"));
    let path = pool_dir(&config, agent).join(str_of(&entry, "file"));
    if str_of(&entry, "kind") == "image" {
        let size = entry.get("size").and_then(Value::as_u64).unwrap_or(0);
        let bytes = if size <= IMAGE_MAX { tokio::fs::read(&path).await.ok() } else { None };
        let Some(bytes) = bytes else {
            return tk::tool_text(&format!("« {name} », a photo from {from}, can't be shown now."), false);
        };
        use base64::Engine;
        let mut out = Dict::new();
        out.insert(
            "content".into(),
            json!([
            {"type": "image", "data": base64::engine::general_purpose::STANDARD.encode(bytes), "mimeType": mime},
            {"type": "text", "text": format!("« {name} », a photo from {from}.")}]),
        );
        out.insert("isError".into(), Value::Bool(false));
        return out;
    }
    let ocr = pool_dir(&config, agent).join(format!("{key}.txt"));
    let (n, m) = (name.clone(), mime.clone());
    let read = tokio::task::spawn_blocking(move || crate::doc_text::document_text(&path, &n, &m))
        .await
        .unwrap_or(Read::Unreadable);
    let shown = |text: &str, note: &str| {
        let words: String = text.chars().take(TEXT_MAX).collect();
        let more = text.chars().count().saturating_sub(TEXT_MAX);
        let tail = if more > 0 { format!("\n[… {more} characters more not shown]") } else { String::new() };
        tk::tool_text(&format!("« {name} » ({mime}), from {from}{note}:\n{words}{tail}"), false)
    };
    match read {
        Read::Text(text) => shown(&text, ""),
        // a scan: the text its sender's phone recognised, when it sent one
        _ if ocr.is_file() => match std::fs::read_to_string(&ocr) {
            Ok(text) if !text.trim().is_empty() => shown(&text, ", its text recognised on the phone"),
            _ => tk::tool_text(&format!("« {name} » is a scan whose text can't be read here: say so plainly."), false),
        },
        Read::NoText => tk::tool_text(
            &format!("« {name} » is a scan with no text to read: say so plainly, and ask for a photo of it if needed."),
            false,
        ),
        Read::Unreadable => tk::tool_text(
            &format!("« {name} » ({mime}) can't be read here: a PDF, a Word, Excel or PowerPoint file, a text or a photo can."),
            false,
        ),
    }
}

// -- her side: held until her message goes ------------------------------------------------------------------------

/// `POST /v1/files` for a shared agent (`id`): the file held here until her message takes it to the host.
pub fn hold(config: &Config, id: &str, source: &Path, filename: &str, content_type: Option<&str>) -> ApiResult<Value> {
    if !FOLDER.is_match(id) {
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
        let size =
            std::io::copy(&mut std::io::Read::take(&mut input, max + 1), &mut out).map_err(anyhow::Error::from)?;
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
    /// A scan's text recognised on her phone.
    text: Option<String>,
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
                    Ok(Outgoing { kind: "image", name, mime, bytes: Some(bytes), held: None, text: None })
                }
                "document" => {
                    let file =
                        a.path.as_deref().and_then(|p| p.strip_prefix(HELD_PREFIX)).filter(|f| stored_key(f).is_some());
                    let path = file.map(|f| held_dir(config, id).join(f)).filter(|p| p.is_file() && !p.is_symlink());
                    let path = path.ok_or_else(|| HttpError::new(422, "this file is no longer here: add it again"))?;
                    let name = a
                        .name
                        .clone()
                        .filter(|n| !n.trim().is_empty())
                        .unwrap_or_else(|| file.map(|f| f[13..].to_string()).unwrap_or_else(|| "fichier".into()));
                    let text = a.text.clone().filter(|t| !t.trim().is_empty());
                    Ok(Outgoing { kind: "document", name, mime, bytes: None, held: Some(path), text })
                }
                _ => Err(HttpError::new(422, "photos with data, documents with a path")),
            }
        })
        .collect()
}

/// Her files sent to the host into conversation `cid`, one request each: what her message names (`{id, text?}`), or
/// the host's refusal (the files it already took, taken back).
pub async fn send_files(
    state: &AppState,
    id: &str,
    cid: &str,
    files: &[Outgoing],
) -> ApiResult<Result<Vec<Value>, (u16, Value)>> {
    let mut sent: Vec<Value> = Vec::new();
    for f in files {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("name", &f.name)
            .append_pair("mime", &f.mime)
            .append_pair("kind", f.kind)
            .finish();
        let body = match (&f.bytes, &f.held) {
            (Some(bytes), _) => reqwest::Body::from(bytes.clone()),
            (None, Some(path)) => {
                let file = tokio::fs::File::open(path)
                    .await
                    .map_err(|_| HttpError::new(422, "this file is no longer here: add it again"))?;
                reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file))
            }
            _ => continue,
        };
        let request =
            shares::owner_request(state, id, reqwest::Method::POST, &format!("conversations/{cid}/files?{query}"))?;
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
                        sent.push(json!({"id": key, "text": f.text}));
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

/// The files the host took for a message that didn't go, taken back (best effort: else its purge, after an hour).
pub async fn take_back(state: &AppState, id: &str, cid: &str, sent: &[Value]) {
    for key in sent.iter().filter_map(|s| s.get("id").and_then(Value::as_str)) {
        if let Ok(request) =
            shares::owner_request(state, id, reqwest::Method::DELETE, &format!("conversations/{cid}/files/{key}"))
        {
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

/// What a message's `files` names: ids (a relative's bridge before 12/11) or `{id, text?}`.
pub fn named(files: Option<&Value>) -> Vec<(String, Option<String>)> {
    let Some(Value::Array(all)) = files else { return Vec::new() };
    all.iter()
        .map(|f| match f {
            Value::Object(o) => {
                (tk::str_or_empty(o.get("id")), o.get("text").and_then(Value::as_str).map(str::to_string))
            }
            other => (py::py_str(other), None),
        })
        .collect()
}

/// She stops using the agent: the host asked to erase her files in its pool (best effort), the ones held here gone.
pub async fn forget_mine(state: &AppState, id: &str) {
    if let Ok(request) = shares::owner_request(state, id, reqwest::Method::DELETE, "files") {
        match request.timeout(Duration::from_secs(10)).send().await {
            Ok(resp) if resp.status().is_success() => {}
            _ => tracing::info!(agent = %id, "her files not erased on the host's side now: its purge will"),
        }
    }
    if FOLDER.is_match(id) {
        let _ = std::fs::remove_dir_all(held_dir(&state.config(), id));
    }
}

/// Her relayed timeline: each of her files (`/famille/…`) with a url on this bridge.
pub fn urls_under(id: &str, mut page: Value) -> Value {
    if let Some(Value::Array(items)) = page.get_mut("items") {
        for item in items {
            if let Some(Value::Array(atts)) = item.get_mut("attachments") {
                for a in atts {
                    if let Some((_, key)) = parse_seen(&str_of(a, "path")) {
                        a["url"] = json!(format!("/v1/shared-agents/{id}/files/{key}"));
                    }
                }
            }
        }
    }
    page
}

/// `GET /v1/shared-agents/{id}/files`: the agent's pool, from its owner's bridge (each with its url here).
async fn relayed_list(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let resp = shares::owner_request(&state, &id, reqwest::Method::GET, "files")?
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    let mut payload = resp.json::<Value>().await.unwrap_or(Value::Null);
    if let Some(Value::Array(files)) = payload.get_mut("files") {
        for f in files {
            let key = str_of(f, "id");
            f["url"] = json!(format!("/v1/shared-agents/{id}/files/{key}"));
        }
    }
    Ok(web::json_response(status, &payload))
}

/// `GET /v1/shared-agents/{id}/files/{key}`: one file of the agent's pool, from its owner's bridge.
async fn relayed_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    if !KEY.is_match(&key) {
        return Err(HttpError::not_found());
    }
    let mut request = shares::owner_request(&state, &id, reqwest::Method::GET, &format!("files/{key}"))?
        .timeout(Duration::from_secs(600));
    for name in ["if-none-match", "if-modified-since", "range"] {
        if let Some(v) = headers.get(name) {
            request = request.header(name, v.clone());
        }
    }
    let resp = request
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    if !(resp.status().is_success() || status == 304) {
        return Err(HttpError::new(if status == 404 { 404 } else { 502 }, "this file can't be had right now"));
    }
    let kept: Vec<(String, HeaderValue)> = [
        "content-type",
        "content-length",
        "content-disposition",
        "etag",
        "last-modified",
        "accept-ranges",
        "content-range",
    ]
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

/// `DELETE /v1/shared-agents/{id}/files/{key}`: one of her files, deleted on its owner's bridge.
async fn relayed_delete(
    State(state): State<AppState>,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    if !KEY.is_match(&key) {
        return Err(HttpError::not_found());
    }
    let resp = shares::owner_request(&state, &id, reqwest::Method::DELETE, &format!("files/{key}"))?
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    let payload = resp.json::<Value>().await.unwrap_or(Value::Null);
    Ok(web::json_response(status, &payload))
}

/// For the apps (under /v1, the bridge key): the owner's pool of his agent, a relative's pool of an agent shared with
/// her.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent}/shared-files", get(own_list))
        .route("/agents/{agent}/shared-files/{key}", get(own_file).delete(own_delete))
        .route("/shared-agents/{id}/files", get(relayed_list))
        .route("/shared-agents/{id}/files/{key}", get(relayed_file).delete(relayed_delete))
}

// -- the purge ----------------------------------------------------------------------------------------------------

/// When a file was last written (Unix time).
fn written_at(file: &std::fs::DirEntry) -> Option<f64> {
    let modified = file.metadata().ok()?.modified().ok()?;
    Some(modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64())
}

/// The files that should be gone, erased (app.rs `purge_loop`): held ones past a day; in the pools, those of an agent
/// gone or shared with nobody, of a household no longer shared with (unlinked), past the owner's `kept_days` (photos
/// `PHOTO_DAYS`), taken by no message within an hour, and any file a pool doesn't list. Returns how many.
pub fn purge(config: &Config, at: f64) -> usize {
    let mut removed = 0;
    // held here for a message that never went
    let held = PathBuf::from(config.data_dir()).join("shared-held");
    for dir in std::fs::read_dir(&held).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(dir.path()).into_iter().flatten().flatten() {
            let old = written_at(&file).is_some_and(|w| at - w > HELD_SECONDS);
            if old && std::fs::remove_file(file.path()).is_ok() {
                removed += 1;
            }
        }
    }
    let kept_days = crate::files_kept::kept_days(config);
    let mut agents: Vec<String> = std::fs::read_dir(root(config))
        .into_iter()
        .flatten()
        .flatten()
        .map(|d| d.file_name().to_string_lossy().into_owned())
        .collect();
    let indexed: Vec<String> = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        load(&index_path(config)).keys().cloned().collect()
    };
    for agent in indexed {
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    for agent in agents {
        if config.agent(Some(&agent)).is_none() || !pooled(config, &agent) {
            removed += files_of(config, &agent).len();
            forget_agent(config, &agent);
            continue;
        }
        let shared = shares::shared_with(config, &agent);
        let stays = |e: &Value| {
            let age = at - e.get("at").and_then(Value::as_f64).unwrap_or(0.0);
            let household = str_of(e, "household");
            (household == OWNER || shared.contains(&household))
                && !(e.get("sent") != Some(&Value::Bool(true)) && age > UNCLAIMED_SECONDS)
                && !(kept_days > 0 && age > kept_days as f64 * 86_400.0)
                && !(str_of(e, "kind") == "image" && age > crate::files_kept::PHOTO_DAYS * 86_400.0)
        };
        removed += forget_where(config, &agent, stays);
        // on the disk, what the pool doesn't list (an hour old: one arriving now is listed once it is whole)
        let listed: Vec<String> =
            files_of(config, &agent).iter().flat_map(|(key, e)| [str_of(e, "file"), format!("{key}.txt")]).collect();
        for file in std::fs::read_dir(pool_dir(config, &agent)).into_iter().flatten().flatten() {
            let name = file.file_name().to_string_lossy().into_owned();
            let old = written_at(&file).is_some_and(|w| at - w > UNCLAIMED_SECONDS);
            if listed.contains(&name) || !old {
                continue;
            }
            let path = file.path();
            let gone = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            if gone.is_ok() {
                removed += 1;
            }
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        (dir, config)
    }

    #[test]
    fn a_seen_path_names_a_stored_file_only() {
        let path = seen_path("0123456789ab-bulletin.pdf");
        assert_eq!(path, "/famille/0123456789ab-bulletin.pdf");
        assert_eq!(parse_seen(&path), Some(("0123456789ab-bulletin.pdf".into(), "0123456789ab".into())));
        for bad in [
            "/famille/../x",
            "/famille/notes.txt",
            "/fichiers/2026-11/0123456789ab-x.pdf",
            "/famille/0123456789ab-a/b",
            "/proche/c0123456789ab/0123456789ab-x.pdf",
        ] {
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
    }

    #[test]
    fn a_message_names_its_files_by_id_or_with_a_scans_text() {
        assert_eq!(named(Some(&json!(["0123456789ab"]))), vec![("0123456789ab".into(), None)]);
        assert_eq!(
            named(Some(&json!([{"id": "0123456789ab", "text": "Certificat"}]))),
            vec![("0123456789ab".into(), Some("Certificat".into()))]
        );
        assert!(named(None).is_empty());
    }

    #[test]
    fn the_pool_lists_what_went_and_only_its_sender_deletes_it() {
        let (_dir, config) = config();
        let pool = pool_dir(&config, "maison");
        make_dir(&pool).unwrap();
        for (key, household, sent) in
            [("0123456789ab", "marie", true), ("0123456789ac", OWNER, true), ("0123456789ad", "marie", false)]
        {
            std::fs::write(pool.join(format!("{key}-f.pdf")), b"%PDF").unwrap();
            update(&config, "maison", |files| {
                files.insert(
                    key.into(),
                    json!({"file": format!("{key}-f.pdf"), "name": "f.pdf", "kind": "document",
                                                "at": 1.0 + key.len() as f64, "sent": sent, "household": household,
                                                "person": if household == OWNER { "Alex" } else { "Marie" }}),
                )
            })
            .unwrap();
        }
        let list = listed(&config, "maison", "marie", |k| format!("/u/{k}"));
        assert_eq!(list.len(), 2, "the one no message took isn't listed");
        assert!(list.iter().any(|f| f["id"] == "0123456789ab"
            && f["mine"] == true
            && f["from"] == "Marie"
            && f["url"] == "/u/0123456789ab"));
        assert!(list.iter().any(|f| f["id"] == "0123456789ac" && f["mine"] == false && f["from"] == "Alex"));
        assert!(list_text(&config, "maison").contains("« f.pdf » (document) from Alex"));
        assert_eq!(delete_one(&config, "maison", "marie", "0123456789ac").unwrap_err().status.as_u16(), 404);
        delete_one(&config, "maison", "marie", "0123456789ab").unwrap();
        assert!(!pool.join("0123456789ab-f.pdf").exists() && pool.join("0123456789ac-f.pdf").exists());
        forget_households(&config, "maison", &["marie".into()]);
        assert_eq!(files_of(&config, "maison").keys().collect::<Vec<_>>(), ["0123456789ac"]);
        forget_agent(&config, "maison");
        assert!(files_of(&config, "maison").is_empty() && !pool.exists());
    }

    #[test]
    fn a_held_file_keeps_the_size_limit() {
        let (dir, mut config) = config();
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
