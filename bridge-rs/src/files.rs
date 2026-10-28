//! `POST /v1/files`: store a document where the agent's container can read it.
//!
//! Stored at `{upload_dir_host}/{YYYY-MM}/{id12}-{name}`; the response gives the same file as seen
//! from inside the Hermes container: `{upload_dir_container}/{YYYY-MM}/{id12}-{name}`. An agent whose tools run in a
//! sandbox sees its folder there at `/fichiers`, read-only (`agents_admin.mount_uploads`).
//!
//! Port of `bridge/bipbridge/files.py` (owner group: agents).

use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use futures::StreamExt;
use regex::Regex;
use serde_json::{json, Value};
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

use crate::app::AppState;
use crate::config::{AgentConfig, Limits};
use crate::web::{ApiResult, HttpError};

pub const MAX_NAME_CHARS: usize = 120;
pub const CHUNK: usize = 1024 * 1024;
/// Where a sandboxed agent finds the files it was sent. The sandbox's user is not this account (rootless podman maps
/// it elsewhere): its folder and files are readable by others, which only that sandbox reaches (the home is closed).
pub const SANDBOX_UPLOADS: &str = "/fichiers";
pub const SANDBOX_DIR_MODE: u32 = 0o755;
pub const SANDBOX_FILE_MODE: u32 = 0o644;
/// The path the size limit watches.
pub const UPLOAD_PATH: &str = "/v1/files";
/// Starlette's form limits for this route (`request.form(max_files=1, max_fields=8)`) and its per-field size.
const MAX_FILES: usize = 1;
const MAX_FIELDS: usize = 8;
const MAX_PART_SIZE: usize = 1024 * 1024;

pub static STORED_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{12}-[^\n]").expect("valid regex"));
pub static MONTH_DIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}\n?\z").expect("valid regex"));

/// `BodyTooLarge`: an HTTPException 413.
fn body_too_large(limit: u64) -> HttpError {
    HttpError::new(413, format!("file too large (max {limit} bytes)"))
}

/// Python's `str.isalnum()` for one character (letters and numbers of any script).
fn is_alnum(c: char) -> bool {
    use GeneralCategory::*;
    matches!(
        get_general_category(c),
        UppercaseLetter
            | LowercaseLetter
            | TitlecaseLetter
            | ModifierLetter
            | OtherLetter
            | DecimalNumber
            | LetterNumber
            | OtherNumber
    )
}

/// Python's `str.isalnum()` (False for an empty string).
pub fn py_isalnum(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_alnum)
}

/// Kept by `[^\w.\-+@()\[\],]+` (Python's Unicode `\w`).
fn is_safe(c: char) -> bool {
    c == '_' || is_alnum(c) || ".-+@()[],".contains(c)
}

fn is_control_like(c: char) -> bool {
    use GeneralCategory::*;
    matches!(get_general_category(c), Control | Format | Surrogate | PrivateUse | Unassigned)
}

/// Basename only, no control chars, shell-friendly (spaces -> `_`), no leading dot, ≤120 chars.
pub fn sanitize_filename(raw: &str) -> String {
    let name: String = raw.nfc().collect();
    let name = name.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or("").to_string();
    let name: String = name.chars().filter(|c| !is_control_like(*c)).collect();
    // `_UNSAFE.sub("_", name.strip())`
    let mut safe = String::new();
    let mut in_unsafe = false;
    for c in name.trim().chars() {
        if is_safe(c) {
            safe.push(c);
            in_unsafe = false;
        } else if !in_unsafe {
            safe.push('_');
            in_unsafe = true;
        }
    }
    // `_REPEAT.sub("_", name).strip("._-")`
    let mut collapsed = String::new();
    for c in safe.chars() {
        if c == '_' && collapsed.ends_with('_') {
            continue;
        }
        collapsed.push(c);
    }
    let mut name = collapsed.trim_matches(|c| c == '.' || c == '_' || c == '-').to_string();
    if name.is_empty() || name == "." || name == ".." {
        name = "fichier".to_string();
    }
    let chars: Vec<char> = name.chars().collect();
    if chars.len() > MAX_NAME_CHARS {
        let dot = chars.iter().rposition(|c| *c == '.');
        match dot {
            Some(at) if chars.len() - at - 1 > 0 && chars.len() - at - 1 <= 16 => {
                let ext: String = chars[at + 1..].iter().collect();
                let keep = MAX_NAME_CHARS.saturating_sub(ext.chars().count() + 1);
                let stem: String = chars[..at].iter().take(keep).collect();
                name = format!("{}.{ext}", stem.trim_end_matches(['.', '_', '-']));
            }
            _ => name = chars[..MAX_NAME_CHARS].iter().collect(),
        }
    }
    name
}

/// What the size limit leaves in the request for `handle_upload`: whether the body went over it.
#[derive(Clone)]
struct UploadLimit {
    exceeded: Arc<AtomicBool>,
    max: u64,
}

/// Python's `int(value)` on a header (spaces around, a sign, `_` between digits).
fn header_int(value: &[u8]) -> Option<i128> {
    let text = std::str::from_utf8(value).ok()?.trim();
    let (sign, digits) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let plain: String = digits.chars().filter(|c| *c != '_').collect();
    if !plain.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    plain.parse::<i128>().ok().map(|v| sign * v)
}

/// `UploadSizeLimit`: `POST /v1/files` bodies over `[limits] upload_max_mb` refused with 413 (by Content-Length,
/// or while reading a chunked body). Applied to the whole app (app.rs), as Python's middleware.
pub async fn upload_size_limit(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.uri().path() != UPLOAD_PATH {
        return next.run(request).await;
    }
    let max = state.config().limits.upload_max_bytes;
    // Multipart framing adds a little overhead around the file itself.
    let limit = max + 64 * 1024;
    for value in request.headers().get_all("content-length") {
        if let Some(length) = header_int(value.as_bytes()) {
            if length > limit as i128 {
                let body = format!("{{\"detail\":\"file too large (max {max} bytes)\"}}");
                let mut response = Response::new(Body::from(body.clone()));
                *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
                let headers = response.headers_mut();
                headers.insert("content-type", HeaderValue::from_static("application/json"));
                headers.insert("content-length", HeaderValue::from(body.len()));
                headers.insert("connection", HeaderValue::from_static("close"));
                return response;
            }
        }
    }
    let exceeded = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicU64::new(0));
    let (mut parts, body) = request.into_parts();
    let flag = exceeded.clone();
    let stream = body.into_data_stream().map(move |chunk| match chunk {
        Ok(bytes) => {
            let total = received.fetch_add(bytes.len() as u64, Ordering::SeqCst) + bytes.len() as u64;
            if total > limit {
                flag.store(true, Ordering::SeqCst);
                Err(std::io::Error::other(format!("file too large (max {max} bytes)")))
            } else {
                Ok(bytes)
            }
        }
        Err(e) => Err(std::io::Error::other(e)),
    });
    parts.extensions.insert(UploadLimit { exceeded, max });
    next.run(Request::from_parts(parts, Body::from_stream(stream))).await
}

/// A part's file, spooled to the disk while the form is read (Starlette's `SpooledTemporaryFile`); gone when dropped.
struct Spooled {
    path: PathBuf,
}

impl Spooled {
    fn create() -> std::io::Result<(Spooled, std::fs::File)> {
        let path = std::env::temp_dir().join(format!("bipbridge-upload-{}", crate::py::uuid4_hex()));
        let file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
        Ok((Spooled { path }, file))
    }
}

impl Drop for Spooled {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// One item of the form: a field's text, or a file part.
enum FormItem {
    Field(String),
    File { filename: String, content_type: Option<String>, spooled: Spooled },
}

/// Why the form could not be read.
enum FormError {
    TooLarge(u64),
    Invalid,
}

/// Starlette's `_user_safe_decode`: UTF-8, else Latin-1.
fn safe_decode(data: &[u8]) -> String {
    match std::str::from_utf8(data) {
        Ok(text) => text.to_string(),
        Err(_) => data.iter().map(|&b| b as char).collect(),
    }
}

/// `request.form(max_files=1, max_fields=8)`: (name, item) pairs in order.
async fn read_form(request: Request) -> Result<Vec<(String, FormItem)>, FormError> {
    let limit = request.extensions().get::<UploadLimit>().cloned();
    let too_large = |limit: &Option<UploadLimit>| match limit {
        Some(l) if l.exceeded.load(Ordering::SeqCst) => FormError::TooLarge(l.max),
        _ => FormError::Invalid,
    };
    let content_type = request.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let main = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let body = request.into_body();
    if main == "application/x-www-form-urlencoded" {
        let bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|_| too_large(&limit))?;
        return Ok(url::form_urlencoded::parse(&bytes)
            .map(|(k, v)| (k.into_owned(), FormItem::Field(v.into_owned())))
            .collect());
    }
    if main != "multipart/form-data" {
        return Ok(Vec::new());
    }
    let boundary = multer::parse_boundary(&content_type).map_err(|_| FormError::Invalid)?;
    let mut multipart = multer::Multipart::new(body.into_data_stream(), boundary);
    let mut items = Vec::new();
    let (mut files, mut fields) = (0usize, 0usize);
    loop {
        let mut field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return Err(too_large(&limit)),
        };
        let name = field.name().unwrap_or("").to_string();
        match field.file_name().map(str::to_string) {
            Some(filename) => {
                files += 1;
                if files > MAX_FILES {
                    return Err(FormError::Invalid);
                }
                let content_type =
                    field.headers().get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string);
                let (spooled, mut file) = Spooled::create().map_err(|_| FormError::Invalid)?;
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => file.write_all(&chunk).map_err(|_| FormError::Invalid)?,
                        Ok(None) => break,
                        Err(_) => return Err(too_large(&limit)),
                    }
                }
                items.push((name, FormItem::File { filename, content_type, spooled }));
            }
            None => {
                fields += 1;
                if fields > MAX_FIELDS {
                    return Err(FormError::Invalid);
                }
                let mut data = Vec::new();
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            data.extend_from_slice(&chunk);
                            if data.len() > MAX_PART_SIZE {
                                return Err(FormError::Invalid);
                            }
                        }
                        Ok(None) => break,
                        Err(_) => return Err(too_large(&limit)),
                    }
                }
                items.push((name, FormItem::Field(safe_decode(&data))));
            }
        }
    }
    Ok(items)
}

/// `POST /v1/files` (multipart: `agent`, `file`): the file stored in the agent's uploads, its path as the agent
/// sees it.
pub async fn handle_upload(state: AppState, request: Request) -> ApiResult<Value> {
    let config = state.config();
    if let Err(full) = crate::place_backup::refuse_when_full().await {
        // an offer's place with its 5 GB used up: refused, and a card for the person (storage_watch.rs)
        let c = config.clone();
        let _ = tokio::task::spawn_blocking(move || crate::storage_watch::upload_refused(&c, None)).await;
        return Err(full);
    }
    let form = match read_form(request).await {
        Ok(form) => form,
        Err(FormError::TooLarge(max)) => return Err(body_too_large(max)),
        Err(FormError::Invalid) => return Err(HttpError::new(400, "invalid multipart body")),
    };
    // `form.get(key)`: the last item of that name.
    let last = |key: &str| form.iter().rev().find(|(k, _)| k == key).map(|(_, item)| item);
    let agent_name = match last("agent") {
        Some(FormItem::Field(text)) => Some(text.as_str()),
        _ => None,
    };
    let Some(agent) = config.agent(agent_name).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    let Some(FormItem::File { filename, content_type, spooled }) = last("file") else {
        return Err(HttpError::new(400, "missing 'file' part"));
    };
    let source = spooled.path.clone();
    let (filename, content_type, limits) = (filename.clone(), content_type.clone(), config.limits.clone());
    let stored = tokio::task::spawn_blocking(move || {
        let stored = store_upload(&agent, &source, &filename, content_type.as_deref(), &limits, None)?;
        // An isolated sandbox sees a copy of the uploads, in its own volume (sandbox.rs).
        crate::sandbox::sync_agent(&config, &agent.name).map_err(anyhow::Error::from)?;
        Ok(stored)
    })
    .await
    .map_err(anyhow::Error::from)?;
    drop(form);
    stored
}

/// `store_upload`: the spooled file copied into the agent's uploads (`now`: the month's date, for tests).
pub fn store_upload(
    agent: &AgentConfig,
    source: &std::path::Path,
    filename: &str,
    content_type: Option<&str>,
    limits: &Limits,
    now: Option<chrono::DateTime<chrono::Utc>>,
) -> ApiResult<Value> {
    store_upload_from(agent, &mut || std::fs::File::open(source), filename, content_type, limits, now)
}

/// `store_upload` from what `open` gives (called once the folder is ready): a file of another agent the bridge
/// opened itself, never through a link (agent_files.rs), or a text.
pub fn store_upload_from<R: Read>(
    agent: &AgentConfig,
    open: &mut dyn FnMut() -> std::io::Result<R>,
    filename: &str,
    content_type: Option<&str>,
    limits: &Limits,
    now: Option<chrono::DateTime<chrono::Utc>>,
) -> ApiResult<Value> {
    let (Some(host_root), Some(container_root)) = (
        agent.upload_dir_host.as_deref().filter(|s| !s.is_empty()),
        agent.upload_dir_container.as_deref().filter(|s| !s.is_empty()),
    ) else {
        return Err(HttpError::new(409, "uploads not configured for this agent"));
    };
    let name = sanitize_filename(filename);
    let month = now.unwrap_or_else(chrono::Utc::now).format("%Y-%m").to_string();
    let sandboxed = container_root == SANDBOX_UPLOADS;
    let dir_mode = if sandboxed { SANDBOX_DIR_MODE } else { limits.upload_dir_mode };
    let file_mode = if sandboxed { SANDBOX_FILE_MODE } else { limits.upload_file_mode };
    let host_dir = crate::py::join(host_root, &month);
    crate::py::makedirs(&host_dir, dir_mode).map_err(anyhow::Error::from)?;
    if sandboxed {
        crate::py::chmod(&host_dir, dir_mode).map_err(anyhow::Error::from)?; // ignore umask
    }
    let id: String = crate::py::uuid4_hex().chars().take(12).collect();
    let stored = format!("{id}-{name}");
    let host_path = crate::py::join(&host_dir, &stored);
    let copied = (|| -> Result<u64, HttpError> {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(file_mode)
            .open(&host_path)
            .map_err(anyhow::Error::from)?;
        out.set_permissions(std::fs::Permissions::from_mode(file_mode)).map_err(anyhow::Error::from)?; // ignore umask
        let mut input = open().map_err(anyhow::Error::from)?;
        let mut buffer = vec![0u8; CHUNK];
        let mut size: u64 = 0;
        loop {
            let n = input.read(&mut buffer).map_err(anyhow::Error::from)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            if size > limits.upload_max_bytes {
                return Err(body_too_large(limits.upload_max_bytes));
            }
            out.write_all(&buffer[..n]).map_err(anyhow::Error::from)?;
        }
        Ok(size)
    })();
    let size = match copied {
        Ok(size) => size,
        Err(exc) => {
            let _ = std::fs::remove_file(&host_path);
            return Err(exc);
        }
    };
    let container_path = crate::py::join(&crate::py::join(container_root, &month), &stored);
    tracing::info!(agent = %agent.name, size = size, month = %month, "file stored");
    Ok(json!({
        "path": container_path, "filename": name, "size": size,
        "content_type": content_type.unwrap_or("application/octet-stream"),
    }))
}

/// A sandboxed agent's folder, and what it already holds, readable from its sandbox.
pub fn open_to_sandbox(root: &str) -> anyhow::Result<()> {
    std::fs::create_dir_all(root)?;
    crate::py::chmod(root, SANDBOX_DIR_MODE)?;
    for month in std::fs::read_dir(root)? {
        let month = month?.file_name().to_string_lossy().into_owned();
        let month_dir = crate::py::join(root, &month);
        let path = std::path::Path::new(&month_dir);
        if !MONTH_DIR.is_match(&month) || !path.is_dir() || path.is_symlink() {
            continue;
        }
        crate::py::chmod(&month_dir, SANDBOX_DIR_MODE)?;
        for entry in std::fs::read_dir(&month_dir)? {
            let entry = entry?.file_name().to_string_lossy().into_owned();
            let file = crate::py::join(&month_dir, &entry);
            let p = std::path::Path::new(&file);
            if STORED_NAME.is_match(&entry) && p.is_file() && !p.is_symlink() {
                crate::py::chmod(&file, SANDBOX_FILE_MODE)?;
            }
        }
    }
    Ok(())
}

/// Delete stored uploads older than `retention_days` (only files the bridge wrote).
pub fn purge_uploads(root: &str, retention_days: i64, now: Option<f64>) -> usize {
    if root.is_empty() || !std::path::Path::new(root).is_dir() {
        return 0;
    }
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(crate::py::time_time);
    let cutoff = now - (retention_days * 86400) as f64;
    let mut removed = 0;
    let Ok(months) = std::fs::read_dir(root) else { return 0 };
    for month in months.flatten() {
        let month = month.file_name().to_string_lossy().into_owned();
        let month_dir = crate::py::join(root, &month);
        if !MONTH_DIR.is_match(&month) || !std::path::Path::new(&month_dir).is_dir() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&month_dir) {
            for entry in entries.flatten() {
                let entry = entry.file_name().to_string_lossy().into_owned();
                let path = crate::py::join(&month_dir, &entry);
                let p = std::path::Path::new(&path);
                if !STORED_NAME.is_match(&entry) || !p.is_file() || p.is_symlink() {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(p) else { continue };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(f64::MAX);
                if mtime < cutoff && std::fs::remove_file(p).is_ok() {
                    removed += 1;
                }
            }
        }
        if std::fs::read_dir(&month_dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
            let _ = std::fs::remove_dir(&month_dir);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// test_http_routes.py::test_sanitize_filename.
    #[test]
    fn sanitize_filename_as_python() {
        for (raw, expected) in [
            ("../../etc/passwd", "passwd".to_string()),
            ("C:\\Users\\alex\\notes.txt", "notes.txt".to_string()),
            (".bashrc", "bashrc".to_string()),
            ("rapport final\n2026.xlsx", "rapport_final2026.xlsx".to_string()),
            ("a;b|c$(rm).sh", "a_b_c_(rm).sh".to_string()),
            ("", "fichier".to_string()),
            ("..", "fichier".to_string()),
            ("Relevé de compte (oct).pdf", "Relevé_de_compte_(oct).pdf".to_string()),
        ] {
            assert_eq!(sanitize_filename(raw), expected, "{raw:?}");
        }
        assert_eq!(sanitize_filename(&format!("{}.pdf", "é".repeat(200))), format!("{}.pdf", "é".repeat(116)));
    }

    /// test_http_routes.py::test_purge_only_old_bridge_files.
    #[test]
    fn purge_only_old_bridge_files() {
        let dir = tempfile::tempdir().unwrap();
        let month = dir.path().join("2026-01");
        std::fs::create_dir(&month).unwrap();
        let (old, new, foreign) =
            (month.join("0123456789ab-old.pdf"), month.join("0123456789ac-new.pdf"), month.join("notes.txt"));
        for f in [&old, &new, &foreign] {
            std::fs::write(f, b"x").unwrap();
        }
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 86400);
        for f in [&old, &foreign] {
            std::fs::File::options().write(true).open(f).unwrap().set_modified(long_ago).unwrap();
        }
        assert_eq!(purge_uploads(&dir.path().to_string_lossy(), 30, None), 1);
        assert!(!old.exists() && new.exists() && foreign.exists());
    }
}
