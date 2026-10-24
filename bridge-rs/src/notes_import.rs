//! The person's notes brought into an agent's knowledge (decided 2026-10-20): Google Keep has no API for personal
//! accounts, Apple Notes none at all, so the app sends files the person exported — Google Takeout's archive of Keep (its
//! JSON notes), or text and Markdown files (Apple Notes saved by a Shortcut, an Obsidian folder…). Each note becomes a page
//! `knowledge/notes/<source>/<slug>.md`, and `knowledge/notes/index.md` lists them all; the agent reads them with
//! knowledge_read and knowledge_search, and the person sees and edits them in « Sa mémoire ».
//!
//!     POST /v1/agents/{id}/memory/import   {name, data (base64), source?}  ->  {notes, skipped}
//!
//! Port of `bridge/bipbridge/notes_import.py` (owner group: tools-knowledge).

use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents_admin::{editable, profile_folder};
use crate::app::AppState;
use crate::mail_scan::slug;
use crate::mailbox::html_text;
use crate::tools_knowledge as tk;
use crate::web::{self, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

pub const MAX_BYTES: usize = 60 * (1 << 20);
pub const MAX_NOTES: usize = 5000;
pub const NOTE_CHARS: usize = 50_000;
pub const TEXT_SUFFIXES: [&str; 3] = [".txt", ".md", ".markdown"];

static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n+").expect("valid regex"));
static BASE64: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9+/]*={0,2}$").expect("valid regex"));

#[derive(Debug, Deserialize)]
pub struct Upload {
    pub name: String,
    pub data: String,
    #[serde(default)]
    pub source: Option<String>,
}

impl Validate for Upload {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "name", Some(&self.name), 1);
        check_max_len(errors, "name", Some(&self.name), 300);
        check_max_len(errors, "data", Some(&self.data), MAX_BYTES * 4 / 3 + 16);
        check_max_len(errors, "source", self.source.as_deref(), 40);
    }
}

/// A note: its title, its Markdown, its date (`YYYY-MM-DD` or "").
pub type Note = (String, String, String);

/// A Google Keep note from Takeout (title, Markdown, date), None when it is trashed or empty. Err: Python's
/// ValueError (a timestamp that is not a number).
fn keep(data: &Dict) -> Result<Option<Note>, String> {
    let get = |k: &str| data.get(k).cloned().unwrap_or(Value::Null);
    if py::truthy(&get("isTrashed")) {
        return Ok(None);
    }
    let title = tk::strip(&tk::str_or_empty(data.get("title"))).to_string();
    let mut lines: Vec<String> = Vec::new();
    if py::truthy(&get("textContent")) {
        lines.push(tk::strip(&py::py_str(&get("textContent"))).to_string());
    }
    if let Value::Array(items) = get("listContent") {
        for item in items {
            if let Value::Object(item) = item {
                let text = item.get("text").cloned().unwrap_or(Value::Null);
                if py::truthy(&text) {
                    let checked = item.get("isChecked").map(py::truthy).unwrap_or(false);
                    lines.push(format!("- [{}] {}", if checked { "x" } else { " " }, py::py_str(&text)));
                }
            }
        }
    }
    let mut labels = Vec::new();
    if let Value::Array(items) = get("labels") {
        for label in items {
            if let Value::Object(label) = label {
                let name = label.get("name").cloned().unwrap_or(Value::Null);
                if py::truthy(&name) {
                    labels.push(py::py_str(&name));
                }
            }
        }
    }
    if !labels.is_empty() {
        lines.push(format!("\nÉtiquettes : {}", labels.join(", ")));
    }
    if py::truthy(&get("isArchived")) {
        lines.push("_(archivée dans Keep)_".into());
    }
    let body = tk::strip(&lines.join("\n")).to_string();
    if title.is_empty() && body.is_empty() {
        return Ok(None);
    }
    let micros = match get("userEditedTimestampUsec") {
        v if py::truthy(&v) => v,
        _ => get("createdTimestampUsec"),
    };
    let day = if py::truthy(&micros) {
        let micros = py::py_int(&micros)?;
        let seconds = micros as f64 / 1e6;
        chrono::DateTime::from_timestamp(seconds.floor() as i64, 0)
            .map(|d| d.date_naive().format("%Y-%m-%d").to_string())
            .ok_or_else(|| "year is out of range".to_string())?
    } else {
        String::new()
    };
    let title = if title.is_empty() {
        let first = crate::hermes::splitlines(&body).into_iter().next().unwrap_or_default();
        tk::head(&first, 60).to_string()
    } else {
        title
    };
    Ok(Some((title, body, day)))
}

/// What a bad upload answers (Python's HTTPException 400).
#[derive(Debug, thiserror::Error)]
#[error("not a zip archive")]
pub struct NotAZip;

/// (title, Markdown, date) of each note in an upload, and how many files were left out.
pub fn notes_of(name: &str, raw: &[u8]) -> anyhow::Result<(Vec<Note>, usize)> {
    let mut found: Vec<Note> = Vec::new();
    let mut skipped = 0;
    let one = |path: &str, content: &[u8], names: &[String], found: &mut Vec<Note>, skipped: &mut usize| {
        let low = path.to_lowercase();
        if low.ends_with(".json") {
            let parsed = std::str::from_utf8(content).ok().and_then(|t| serde_json::from_str::<Value>(t).ok());
            match parsed {
                Some(Value::Object(data)) => match keep(&data) {
                    Ok(Some(note)) => found.push(note),
                    Ok(None) | Err(_) => *skipped += 1,
                },
                // not a JSON object: Python fails on it (a list's .get) or skips it (bad JSON); left out here
                _ => *skipped += 1,
            }
        } else if TEXT_SUFFIXES.iter().any(|s| low.ends_with(s)) {
            let text = String::from_utf8_lossy(content);
            let text = tk::strip(&text);
            if !text.is_empty() {
                found.push((tk::stem(path).to_string(), text.to_string(), String::new()));
            }
        } else if low.ends_with(".html") || low.ends_with(".htm") {
            let keep_json = format!("{}.json", tk::head(&low, tk::chars(&low).saturating_sub(5)));
            if !names.iter().any(|p| p.to_lowercase() == keep_json) {
                let text = BLANK_LINES.replace_all(&html_text(&String::from_utf8_lossy(content)), "\n\n").into_owned();
                let text = tk::strip(&text);
                if !text.is_empty() {
                    found.push((tk::stem(path).to_string(), text.to_string(), String::new()));
                }
            }
        } else {
            *skipped += 1;
        }
    };
    if name.to_lowercase().ends_with(".zip") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(raw)).map_err(|_| NotAZip)?;
        let mut names = Vec::new();
        for index in 0..archive.len() {
            let file = archive.by_index_raw(index)?;
            if !file.is_dir() {
                names.push(file.name().to_string());
            }
        }
        for index in 0..archive.len() {
            let (is_dir, size, filename) = {
                let file = archive.by_index_raw(index)?;
                (file.is_dir(), file.size(), file.name().to_string())
            };
            if is_dir || size > 5 * (1 << 20) || found.len() >= MAX_NOTES {
                continue;
            }
            let mut content = Vec::new();
            archive.by_index(index)?.read_to_end(&mut content)?;
            one(&filename, &content, &names, &mut found, &mut skipped);
        }
    } else {
        let names = vec![name.to_string()];
        one(name, raw, &names, &mut found, &mut skipped);
    }
    Ok((found, skipped))
}

/// The notes as pages of `knowledge/notes/<source>/`, then the index of every source written again.
pub fn write_notes(folder: &Path, source: &str, notes: &[Note]) -> anyhow::Result<usize> {
    let root = folder.join("knowledge").join("notes");
    let target = root.join(slug(source));
    std::fs::create_dir_all(&target)?;
    let mut used: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (title, body, day) in notes {
        let key = slug(title);
        let count = used.entry(key.clone()).or_insert(0);
        *count += 1;
        let name = if *count == 1 { key.clone() } else { format!("{key}-{count}") };
        let text = format!(
            "# {title}\n\n{}{}\n",
            if day.is_empty() { String::new() } else { format!("_{day}_\n\n") },
            tk::head(body, NOTE_CHARS)
        );
        let path = target.join(format!("{name}.md"));
        tk::replace_with(&target.join(format!("{name}.md.bipagents")), &path, text.as_bytes())?;
    }
    let mut lines: Vec<String> = vec![
        "# Notes".into(),
        String::new(),
        "Les notes importées par la personne, par source. Chaque ligne mène à une note (knowledge_read).".into(),
        String::new(),
    ];
    let mut sources: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&root)?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    sources.sort();
    for (source_name, source_dir) in sources {
        let mut pages: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&source_dir)?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(n, _)| n.ends_with(".md"))
            .collect();
        pages.sort();
        if pages.is_empty() {
            continue;
        }
        lines.push(format!("## {source_name} ({})", pages.len()));
        for (page_name, page) in pages {
            let size = std::fs::metadata(&page)?.len();
            let first = if size > 0 {
                let text = tk::read_text(&page)?;
                let line = crate::hermes::splitlines(&text).into_iter().next().unwrap_or_default();
                tk::strip(line.trim_start_matches(['#', ' '])).to_string()
            } else {
                tk::stem(&page_name).to_string()
            };
            lines.push(format!("- [{first}]({source_name}/{page_name})"));
        }
        lines.push(String::new());
    }
    std::fs::write(root.join("index.md"), format!("{}\n", tk::rstrip(&lines.join("\n"))))?;
    Ok(notes.len())
}

/// Python's `base64.b64decode(data, validate=True)`.
pub(crate) fn b64decode(data: &str) -> Option<Vec<u8>> {
    if !BASE64.is_match(data) {
        return None;
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireCanonical),
    );
    engine.decode(data).ok()
}

async fn import_notes(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<Upload>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| HttpError::new(404, "not an agent this install manages"))?;
    crate::place_backup::refuse_when_full().await?; // an offer's place with its 5 GB used up
    let raw = b64decode(&body.data).ok_or_else(|| HttpError::new(400, "not base64"))?;
    if raw.len() > MAX_BYTES {
        return Err(HttpError::new(413, "60 MB at most"));
    }
    let name = body.name.clone();
    let (found, skipped) =
        match tokio::task::spawn_blocking(move || notes_of(&name, &raw)).await.map_err(anyhow::Error::from)? {
            Ok(result) => result,
            Err(e) if e.downcast_ref::<NotAZip>().is_some() => return Err(HttpError::new(400, "not a zip archive")),
            Err(e) => return Err(e.into()),
        };
    if found.is_empty() {
        return Err(HttpError::new(422, "no note found in this file"));
    }
    let lowered = body.name.to_lowercase();
    let source = match body.source.filter(|s| !s.is_empty()) {
        Some(s) => s,
        None if lowered.starts_with("takeout") || lowered.ends_with(".json") => "keep".into(),
        None => Some(tk::stem(&body.name).to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "notes".into()),
    };
    let folder = profile_folder(&config, &agent);
    let written = tokio::task::spawn_blocking(move || write_notes(&folder, &source, &found))
        .await
        .map_err(anyhow::Error::from)??;
    Ok(web::ok(json!({"notes": written, "skipped": skipped})))
}

/// `notes_router()` (mounted under /v1 with the bridge key). `PUT …/memory/import` is memory_edit's
/// `…/memory/{which}` in Python (it answers « notes or user »): the same here.
pub fn router() -> Router<AppState> {
    Router::new().route("/agents/{agent_id}/memory/import", post(import_notes).put(put_import))
}

async fn put_import(
    state: State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    body: Json<crate::memory_edit::Entries>,
) -> ApiResult<Response> {
    crate::memory_edit::put_entries(state, UrlPath((agent_id, "import".to_string())), body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_as_python_validates_it() {
        assert_eq!(b64decode("QQ==").unwrap(), b"A");
        assert_eq!(b64decode("").unwrap(), b"");
        assert!(b64decode("QQ").is_none());
        assert!(b64decode("Q Q==").is_none());
        assert!(b64decode("QR==").is_some()); // trailing bits allowed
    }

    #[test]
    fn a_keep_note() {
        let data: Dict =
            serde_json::from_value(json!({"title": "", "textContent": "Offrir un vélo\nà Léa", "isArchived": true,
                                                       "createdTimestampUsec": "1760000000000000"}))
            .unwrap();
        let (title, body, day) = keep(&data).unwrap().unwrap();
        assert_eq!(title, "Offrir un vélo");
        assert_eq!(body, "Offrir un vélo\nà Léa\n_(archivée dans Keep)_");
        assert_eq!(day, "2025-10-09");
    }
}
