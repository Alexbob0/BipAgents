//! An agent's memory, seen and edited by its person in the app: what Hermes keeps across conversations
//! (`memories/MEMORY.md`, its notes, and `USER.md`, what it knows of the person), and its knowledge pages
//! (`knowledge/…`, written by long jobs such as a mailbox scan). Hermes has no API for them: the bridge reads and writes
//! the files as Hermes does — entries separated by « \n§\n », the same limits, the same lock — and Hermes takes the
//! change into the agent's next conversation (a conversation starts from a snapshot).
//!
//!     GET    /v1/agents/{id}/memory                   {notes: {entries, limit, used}, user: {…}, pages: [{path, size, updated}]}
//!     PUT    /v1/agents/{id}/memory/{notes|user}      {entries: [...]}
//!     GET    /v1/agents/{id}/memory/pages/{path}      {path, text}
//!     PUT    /v1/agents/{id}/memory/pages/{path}      {text}
//!     DELETE /v1/agents/{id}/memory/pages/{path}
//!
//! Only an agent this install manages (agents_admin.editable).
//!
//! Port of `bridge/bipbridge/memory_edit.py` (owner group: tools-knowledge).

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents_admin::{editable, profile_folder};
use crate::app::AppState;
use crate::tools_knowledge as tk;
use crate::web::{self, check_max_items, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::Dict;

pub const DELIMITER: &str = "\n§\n";
/// Hermes' defaults (memory_char_limit…): which -> (file, limit).
pub const FILES: [(&str, &str, usize); 2] = [("notes", "MEMORY.md", 2200), ("user", "USER.md", 1375)];
pub const LIMIT_KEYS: [(&str, &str); 2] = [("notes", "memory_char_limit"), ("user", "user_char_limit")];
pub const PAGE_MAX: usize = 200_000;
pub const PAGES_MAX: usize = 2000;
pub const PAGE_SUFFIXES: [&str; 3] = [".md", ".json", ".txt"];

static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)\n§\n|^§$").expect("valid regex"));

#[derive(Debug, Deserialize)]
pub struct Entries {
    #[serde(default)]
    pub entries: Vec<String>,
}

impl Validate for Entries {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "entries", Some(&self.entries), 200);
    }
}

#[derive(Debug, Deserialize)]
pub struct Page {
    pub text: String,
}

impl Validate for Page {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "text", Some(&self.text), PAGE_MAX);
    }
}

fn file_of(which: &str) -> Option<(&'static str, usize)> {
    FILES.iter().find(|(w, _, _)| *w == which).map(|(_, f, l)| (*f, *l))
}

/// The profile's own limit when its config sets one, else Hermes' default.
fn limit(folder: &Path, which: &str) -> usize {
    let default = file_of(which).map(|(_, l)| l).unwrap_or(0);
    let key = LIMIT_KEYS.iter().find(|(w, _)| *w == which).map(|(_, k)| *k).unwrap_or("");
    let Ok(text) = tk::read_text(&folder.join("config.yaml")) else { return default };
    let Ok(data) = serde_yaml::from_str::<Value>(&text) else { return default };
    let memory = match &data {
        Value::Object(map) => map.get("memory").cloned().unwrap_or(Value::Null),
        Value::Null => Value::Null,
        _ => return default, // data.get: AttributeError
    };
    let value = match &memory {
        Value::Object(map) => map.get(key).cloned(),
        v if !crate::py::truthy(v) => None,
        _ => return default,
    };
    match value {
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) if i > 0 => i as usize,
            Some(_) => default,
            None => n.as_u64().map(|u| u as usize).unwrap_or(default),
        },
        Some(Value::Bool(true)) => 1, // isinstance(True, int)
        _ => default,
    }
}

/// Hermes' own lock on a memory file (`<file>.lock`, flock): never a write in the middle of its.
struct Locked {
    file: std::fs::File,
}

impl Locked {
    fn take(path: &Path) -> std::io::Result<Locked> {
        use std::os::unix::io::AsRawFd;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let name = format!("{}.lock", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        let file = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path.with_file_name(name))?;
        // SAFETY: flock on a file descriptor this struct owns.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Locked { file })
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        // SAFETY: the descriptor is still open (closed after this drop).
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn read_entries(folder: &Path, which: &str) -> anyhow::Result<Dict> {
    let (file, _) = file_of(which).ok_or_else(|| anyhow::anyhow!("KeyError: '{which}'"))?;
    let path = folder.join("memories").join(file);
    let text = match tk::read_text(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            return Err(anyhow::anyhow!("'utf-8' codec can't decode {}", path.display()))
        }
        Err(_) => String::new(),
    };
    let entries: Vec<String> =
        text.split(DELIMITER).map(|e| tk::strip(e).to_string()).filter(|e| !e.is_empty()).collect();
    let used = tk::chars(&entries.join(DELIMITER));
    let mut out = Dict::new();
    out.insert("entries".into(), json!(entries));
    out.insert("limit".into(), json!(limit(folder, which)));
    out.insert("used".into(), json!(used));
    Ok(out)
}

/// The entries written as Hermes writes them. Err: an `HttpError` 413 when they are too long (Python's
/// HTTPException), or a file error.
pub fn write_entries(folder: &Path, which: &str, entries: &[String]) -> anyhow::Result<Dict> {
    let (file, _) = file_of(which).ok_or_else(|| anyhow::anyhow!("KeyError: '{which}'"))?;
    let cleaned: Vec<String> = entries
        .iter()
        .map(|e| tk::strip(&SEPARATORS.replace_all(tk::strip(e), "")).to_string())
        .filter(|e| !e.is_empty())
        .collect();
    let text = cleaned.join(DELIMITER);
    let most = limit(folder, which);
    if tk::chars(&text) > most {
        return Err(HttpError::new(413, format!("{} characters for {most} at most", tk::chars(&text))).into());
    }
    let path = folder.join("memories").join(file);
    {
        let _lock = Locked::take(&path)?;
        tk::replace_with(&path.with_file_name(format!("{file}.bipagents")), &path, text.as_bytes())?;
    }
    read_entries(folder, which)
}

/// A path's part still to resolve.
enum Part {
    Parent,
    Name(std::ffi::OsString),
}

fn parts_of(path: &Path) -> Vec<Part> {
    path.components()
        .filter_map(|c| match c {
            Component::ParentDir => Some(Part::Parent),
            Component::Normal(n) => Some(Part::Name(n.to_os_string())),
            _ => None,
        })
        .collect()
}

/// `Path.resolve()` (not strict): absolute, symlinks of the existing part followed, `..` applied after them.
pub fn resolve(path: &Path) -> PathBuf {
    let absolute =
        if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut pending: VecDeque<Part> = parts_of(&absolute).into();
    let mut result = PathBuf::from("/");
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        match part {
            Part::Parent => {
                result.pop();
            }
            Part::Name(name) => {
                let candidate = result.join(&name);
                let is_link =
                    std::fs::symlink_metadata(&candidate).map(|m| m.file_type().is_symlink()).unwrap_or(false);
                match std::fs::read_link(&candidate) {
                    Ok(target) if is_link && links < 40 => {
                        links += 1;
                        if target.is_absolute() {
                            result = PathBuf::from("/");
                        }
                        for p in parts_of(&target).into_iter().rev() {
                            pending.push_front(p);
                        }
                    }
                    _ => result = candidate,
                }
            }
        }
    }
    result
}

/// `_page_path`: a knowledge page's file, never outside the folder (Err: Python's HTTPException 404, an `HttpError`).
pub fn page_path(folder: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    let root = resolve(&folder.join("knowledge"));
    let path = resolve(&root.join(relative));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !(path.starts_with(&root) && path != root) || !PAGE_SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return Err(HttpError::new(404, "not a knowledge page").into());
    }
    Ok(path)
}

/// Every file under `root`, sorted as Python sorts `Path`s (by their parts).
fn walk(root: &Path, out: &mut Vec<(Vec<String>, PathBuf)>, parts: &[String], depth: usize) {
    if depth > 40 {
        return;
    }
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let mut these = parts.to_vec();
        these.push(name);
        let path = entry.path();
        // rglob does not descend into symlinked folders (Python 3.12)
        let is_link = entry.file_type().map(|t| t.is_symlink()).unwrap_or(false);
        if path.is_dir() && !is_link {
            walk(&path, out, &these, depth + 1);
        }
        out.push((these, path));
    }
}

pub fn pages(folder: &Path) -> Vec<Dict> {
    let root = folder.join("knowledge");
    let mut found = Vec::new();
    if !root.is_dir() {
        return found;
    }
    let mut all = Vec::new();
    walk(&root, &mut all, &[], 0);
    all.sort_by(|a, b| a.0.cmp(&b.0));
    for (parts, path) in all {
        let name = parts.last().cloned().unwrap_or_default();
        let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
        if path.is_file() && PAGE_SUFFIXES.iter().any(|s| name.ends_with(s)) && !is_link {
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let mut item = Dict::new();
            item.insert("path".into(), json!(parts.join("/")));
            item.insert("size".into(), json!(meta.len()));
            item.insert("updated".into(), json!(tk::mtime(&path).unwrap_or(0.0)));
            found.push(item);
            if found.len() >= PAGES_MAX {
                break;
            }
        }
    }
    found
}

/// `folder_of(request, agent_id)`: the agent's profile folder, 404 for an agent this install doesn't manage.
pub fn folder_of(state: &AppState, agent_id: &str) -> ApiResult<PathBuf> {
    let config = state.config();
    let agent = editable(&config, agent_id).map_err(|_| HttpError::new(404, "not an agent this install manages"))?;
    Ok(profile_folder(&config, &agent))
}

/// An error of this module's functions as the route answers it (its HttpError, else a 500).
pub fn http_error(err: anyhow::Error) -> HttpError {
    match err.downcast::<HttpError>() {
        Ok(http) => http,
        Err(other) => other.into(),
    }
}

async fn memory(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let folder = folder_of(&state, &agent_id)?;
    let out = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        Ok(json!({"notes": read_entries(&folder, "notes")?, "user": read_entries(&folder, "user")?, "pages": pages(&folder)}))
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(http_error)?;
    Ok(web::ok(out))
}

/// `PUT /agents/{agent_id}/memory/{which}` (also what `PUT …/memory/import` reaches in Python: notes_import.rs).
pub async fn put_entries(
    State(state): State<AppState>,
    UrlPath((agent_id, which)): UrlPath<(String, String)>,
    Json(body): Json<Entries>,
) -> ApiResult<Response> {
    if file_of(&which).is_none() {
        return Err(HttpError::new(404, "notes or user"));
    }
    let folder = folder_of(&state, &agent_id)?;
    let out = tokio::task::spawn_blocking(move || write_entries(&folder, &which, &body.entries))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(http_error)?;
    Ok(web::ok(Value::Object(out)))
}

async fn get_page(
    State(state): State<AppState>,
    UrlPath((agent_id, path)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let target = page_path(&folder_of(&state, &agent_id)?, &path).map_err(http_error)?;
    let text = tokio::task::spawn_blocking(move || tk::read_text(&target)).await.map_err(anyhow::Error::from)?;
    match text {
        Ok(text) => Ok(web::ok(json!({"path": path, "text": text}))),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            Err(anyhow::anyhow!("'utf-8' codec can't decode").into())
        }
        Err(_) => Err(HttpError::new(404, "no such page")),
    }
}

async fn put_page(
    State(state): State<AppState>,
    UrlPath((agent_id, path)): UrlPath<(String, String)>,
    Json(body): Json<Page>,
) -> ApiResult<Response> {
    let target = page_path(&folder_of(&state, &agent_id)?, &path).map_err(http_error)?;
    let size = body.text.len();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        tk::replace_with(&target.with_file_name(format!("{name}.bipagents")), &target, body.text.as_bytes())
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(anyhow::Error::from)?;
    Ok(web::ok(json!({"path": path, "size": size})))
}

async fn delete_page(
    State(state): State<AppState>,
    UrlPath((agent_id, path)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let target = page_path(&folder_of(&state, &agent_id)?, &path).map_err(http_error)?;
    if !target.is_file() {
        return Err(HttpError::new(404, "no such page"));
    }
    tokio::task::spawn_blocking(move || std::fs::remove_file(&target))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
    Ok(web::ok(json!({"ok": true})))
}

// `{path:path}` also matches an empty path (`…/memory/pages/`): the same handlers with "".
async fn get_page_empty(state: State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    get_page(state, UrlPath((agent_id, String::new()))).await
}

async fn put_page_empty(
    state: State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    body: Json<Page>,
) -> ApiResult<Response> {
    put_page(state, UrlPath((agent_id, String::new())), body).await
}

async fn delete_page_empty(state: State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    delete_page(state, UrlPath((agent_id, String::new()))).await
}

/// `memory_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/memory", get(memory))
        .route("/agents/{agent_id}/memory/{which}", put(put_entries))
        .route("/agents/{agent_id}/memory/pages/{*path}", get(get_page).put(put_page).delete(delete_page))
        .route("/agents/{agent_id}/memory/pages/", get(get_page_empty).put(put_page_empty).delete(delete_page_empty))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_as_hermes_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::create_dir_all(folder.join("memories")).unwrap();
        std::fs::write(folder.join("memories/MEMORY.md"), "Alex court le dimanche.\n§\nLe chat s'appelle Pixel.")
            .unwrap();
        std::fs::write(folder.join("config.yaml"), "memory:\n  user_char_limit: 40\n").unwrap();
        let notes = read_entries(folder, "notes").unwrap();
        assert_eq!(notes["entries"], json!(["Alex court le dimanche.", "Le chat s'appelle Pixel."]));
        assert_eq!(notes["limit"], json!(2200));
        assert_eq!(
            Value::Object(read_entries(folder, "user").unwrap()),
            json!({"entries": [], "limit": 40, "used": 0})
        );
        let sneaky = write_entries(folder, "user", &["a\n§\nb".to_string()]).unwrap();
        assert_eq!(sneaky["entries"], json!(["ab"]));
        let too_long = write_entries(folder, "user", &["x".repeat(41)]).unwrap_err();
        assert_eq!(too_long.downcast_ref::<HttpError>().unwrap().status.as_u16(), 413);
        assert!(folder.join("memories/USER.md.lock").exists());
    }

    #[test]
    fn pages_stay_inside_their_folder() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::create_dir_all(folder.join("knowledge/mail/people")).unwrap();
        std::fs::write(folder.join("knowledge/mail/index.md"), "# Index\n").unwrap();
        std::fs::write(folder.join("knowledge/mail/people/sam.md"), "# Sam\n").unwrap();
        std::fs::write(folder.join("knowledge/a-b.md"), "x").unwrap();
        let listed: Vec<String> = pages(folder).iter().map(|p| p["path"].as_str().unwrap().to_string()).collect();
        assert_eq!(listed, ["a-b.md", "mail/index.md", "mail/people/sam.md"]);
        assert!(page_path(folder, "../../secret.md").is_err());
        assert!(page_path(folder, "mail/run.sh").is_err());
        assert!(page_path(folder, "").is_err());
        assert!(page_path(folder, "mail/new.md").unwrap().ends_with("knowledge/mail/new.md"));
    }
}
