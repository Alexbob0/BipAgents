//! Files the agents produce and point to with a `MEDIA:<path>` line (the podcast mp3, a chart…).
//!
//! Hermes runs in a container: the path is the agent's (`/home/hermes/simplex-files/x.mp3`). `[media]
//! roots` maps agent-side prefixes to host directories; only files under a declared root, with a known
//! media or document extension, are ever served (symlinks and `..` resolved first). A file in the agent's own places
//! (its isolated sandbox's `/envoi`, its workspace) is served from the bridge's copy: sent_files.rs.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::hermes::SSEEvent;

/// `^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$` (multiline).
pub const MEDIA_LINE: &str = r"(?m)^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$";

pub const CONTENT_TYPES: &[(&str, &str)] = &[
    (".mp3", "audio/mpeg"),
    (".m4a", "audio/mp4"),
    (".aac", "audio/aac"),
    (".wav", "audio/wav"),
    (".ogg", "audio/ogg"),
    (".opus", "audio/ogg"),
    (".png", "image/png"),
    (".jpg", "image/jpeg"),
    (".jpeg", "image/jpeg"),
    (".gif", "image/gif"),
    (".webp", "image/webp"),
    (".heic", "image/heic"),
    (".pdf", "application/pdf"),
    (".mp4", "video/mp4"),
    (".m4v", "video/x-m4v"),
    (".mov", "video/quicktime"),
    (".webm", "video/webm"),
    (".html", "text/html; charset=utf-8"), // a page an agent made (pages.py): the apps show it locked
];

/// Documents (2026-10-22): always a download (`attachment`, `nosniff`, a `sandbox` policy), opened by Quick Look or
/// the phone's apps; none is ever rendered as a page of the bridge's origin.
pub const DOCUMENT_TYPES: &[(&str, &str)] = &[
    (".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    (".xls", "application/vnd.ms-excel"),
    (".csv", "text/csv"),
    (".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    (".doc", "application/msword"),
    (".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    (".ppt", "application/vnd.ms-powerpoint"),
    (".odt", "application/vnd.oasis.opendocument.text"),
    (".ods", "application/vnd.oasis.opendocument.spreadsheet"),
    (".odp", "application/vnd.oasis.opendocument.presentation"),
    (".txt", "text/plain"),
    (".md", "text/markdown"),
    (".json", "application/json"),
    (".zip", "application/zip"),
    (".rtf", "application/rtf"),
    (".key", "application/vnd.apple.keynote"),
    (".pages", "application/vnd.apple.pages"),
    (".numbers", "application/vnd.apple.numbers"),
    (".svg", "image/svg+xml"), // an infographic: never rendered as the bridge's page (its scripts stay locked)
];

/// One of the documents.
pub fn is_document(path: &str) -> bool {
    let ext = extension(path);
    DOCUMENT_TYPES.iter().any(|(e, _)| *e == ext)
}

static MEDIA_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(MEDIA_LINE).expect("valid regex"));
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("valid regex"));

/// `(text without its MEDIA: lines, [paths])`.
pub fn extract_media(text: &str) -> (String, Vec<String>) {
    let paths: Vec<String> = MEDIA_RE.captures_iter(text).map(|c| c[1].to_string()).collect();
    if paths.is_empty() {
        return (text.to_string(), Vec::new());
    }
    let without = MEDIA_RE.replace_all(text, "");
    let cleaned = BLANK_LINES.replace_all(&without, "\n\n");
    (crate::push::pyx::strip(&cleaned).to_string(), paths)
}

/// The text without the `MEDIA:` lines whose path `drop` picks (the others stay where they are).
pub fn without_media_lines(text: &str, drop: impl Fn(&str) -> bool) -> String {
    let without =
        MEDIA_RE.replace_all(text, |c: &regex::Captures| if drop(&c[1]) { String::new() } else { c[0].to_string() });
    let cleaned = BLANK_LINES.replace_all(&without, "\n\n");
    crate::push::pyx::strip(&cleaned).to_string()
}

/// `os.path.splitext(path)[1].lower()`: the last component's extension (leading dots are not one).
fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem_start = name.len() - name.trim_start_matches('.').len();
    match name[stem_start..].rfind('.') {
        Some(i) => name[stem_start + i..].to_lowercase(),
        None => String::new(),
    }
}

/// The host file for an agent-side path, or `None` if it is outside every root, missing, or not media (nor a
/// document).
pub fn resolve(path: &str, roots: &IndexMap<String, String>) -> Option<String> {
    resolve_kinds(path, roots, true)
}

/// `resolve` for the bridge's own folders (its audio files, the agents' pages): media only, never the records and
/// sources kept beside them.
pub fn resolve_media(path: &str, roots: &IndexMap<String, String>) -> Option<String> {
    resolve_kinds(path, roots, false)
}

fn resolve_kinds(path: &str, roots: &IndexMap<String, String>, documents: bool) -> Option<String> {
    let ext = extension(path);
    if !CONTENT_TYPES.iter().any(|(e, _)| *e == ext) && !(documents && is_document(path)) {
        return None;
    }
    // sorted(roots.items(), key=lambda item: -len(item[0])): the longest prefix first, ties in their order
    let mut sorted: Vec<(&String, &String)> = roots.iter().collect();
    sorted.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.chars().count()));
    for (prefix, host_dir) in sorted {
        let prefix = prefix.trim_end_matches('/');
        if path != prefix && !path.starts_with(&format!("{prefix}/")) {
            continue;
        }
        // os.path.realpath: symlinks and `..` resolved (a root or a file that does not exist is never served)
        let root = std::fs::canonicalize(crate::py::expanduser(host_dir)).ok()?;
        let rest = path[prefix.len()..].trim_start_matches('/');
        let candidate = std::fs::canonicalize(root.join(rest)).ok()?;
        if !candidate.starts_with(&root) || !candidate.is_file() {
            return None;
        }
        return Some(candidate.to_string_lossy().into_owned());
    }
    None
}

pub fn content_type(path: &str) -> String {
    content_type_of(path).unwrap_or_else(|| "application/octet-stream".into())
}

/// The content type of a file the apps open, None for any other.
pub fn content_type_of(path: &str) -> Option<String> {
    let ext = extension(path);
    CONTENT_TYPES.iter().chain(DOCUMENT_TYPES).find(|(e, _)| *e == ext).map(|(_, t)| t.to_string())
}

/// The `MEDIA:` lines of a reply taken out of what the apps get (4/11): the files they name reach the apps as
/// structured items only. One per stream (a run's events, a Live turn): a delta's text loses its MEDIA lines as it
/// comes (a line start that may still become one held back until its line is whole) and each line removed is a
/// [`FILE`] event at once, where it was, so a page shows in its place while the reply is written; a whole text (an
/// interim, the end's `output`) loses them at once, the end's files added to its `attachments` (`offset`: where each
/// line was). What the model wrote is kept whole (`said`) for the bridge's own reading of the reply (made_files.rs,
/// sent_files.rs).
#[derive(Debug, Default)]
pub struct MediaFilter {
    held: String,
    mid_line: bool,
    said: String,
    /// Characters of the words passed on so far (a file event's `offset`).
    shown: usize,
}

/// A file of the reply being written, as one of the end's `attachments` (`offset`: characters of the words before it
/// in this stream).
pub const FILE: &str = "message.file";

/// What a delta becomes: words, and the files of the MEDIA lines it held, in order.
#[derive(Debug, PartialEq)]
pub enum Piece {
    Words(String),
    File(String),
}

/// The events whose whole text is the reply's (or part of it), MEDIA lines taken out.
const TEXT_EVENTS: [&str; 5] =
    ["message.interim", "assistant.commentary", "assistant.completed", "message.completed", "message.complete"];
/// The keys a whole text may be under.
const TEXT_KEYS: [&str; 6] = ["output", "final_output", "response", "text", "content", "result"];

impl MediaFilter {
    /// A delta's text without its MEDIA lines, each one a file in its place.
    pub fn text(&mut self, chunk: &str) -> Vec<Piece> {
        self.said.push_str(chunk);
        let text = std::mem::take(&mut self.held) + chunk;
        let mut out = Vec::new();
        let mut rest = text.as_str();
        loop {
            let newline = rest.find('\n');
            if self.mid_line {
                let Some(at) = newline else {
                    self.words(&mut out, rest);
                    return out;
                };
                self.words(&mut out, &rest[..=at]);
                rest = &rest[at + 1..];
                self.mid_line = false;
                continue;
            }
            match newline {
                Some(at) => {
                    match media_path(&rest[..at]) {
                        Some(path) => out.push(Piece::File(path)),
                        None => self.words(&mut out, &rest[..=at]),
                    }
                    rest = &rest[at + 1..];
                }
                None => {
                    if may_become_media(rest) {
                        self.held = rest.to_string();
                    } else {
                        self.words(&mut out, rest);
                        self.mid_line = !rest.is_empty();
                    }
                    return out;
                }
            }
        }
    }

    fn words(&mut self, out: &mut Vec<Piece>, words: &str) {
        if words.is_empty() {
            return;
        }
        self.shown += words.chars().count();
        match out.last_mut() {
            Some(Piece::Words(before)) => before.push_str(words),
            _ => out.push(Piece::Words(words.to_string())),
        }
    }

    /// What is held back, given at the reply's end: words, or the file of a last MEDIA line.
    pub fn flush(&mut self) -> Vec<Piece> {
        let held = std::mem::take(&mut self.held);
        if let Some(path) = media_path(&held) {
            return vec![Piece::File(path)];
        }
        let mut out = Vec::new();
        self.words(&mut out, &held);
        self.mid_line = !held.is_empty();
        out
    }

    /// [`Self::flush`] as events: `kind` the words' delta type.
    pub fn flush_events(&mut self, kind: &str, run_id: &Value) -> Vec<SSEEvent> {
        let delta = json!({"event": kind, "run_id": run_id, "delta": ""});
        let pieces = self.flush();
        self.events(pieces, kind, delta.as_object().cloned().unwrap_or_default(), "delta")
    }

    /// Everything the deltas said, MEDIA lines in (the reply as Hermes keeps it).
    pub fn said(&self) -> &str {
        &self.said
    }

    /// An event as the apps get it: a delta as its words and files (none: all held back), any other one alone.
    pub fn event(&mut self, event: SSEEvent) -> Vec<SSEEvent> {
        if !crate::hermes::is_delta(&event.r#type) {
            return vec![self.whole(event)];
        }
        let Some((key, Value::String(text))) =
            ["delta", "text", "content"].iter().find_map(|k| event.data.get(*k).map(|v| (*k, v.clone())))
        else {
            return vec![event];
        };
        let pieces = self.text(&text);
        if pieces == [Piece::Words(text)] {
            return vec![event];
        }
        self.events(pieces, &event.r#type, event.data, key)
    }

    fn events(&self, pieces: Vec<Piece>, kind: &str, delta: Map<String, Value>, key: &str) -> Vec<SSEEvent> {
        let mut shown = self.shown
            - pieces.iter().map(|p| if let Piece::Words(w) = p { w.chars().count() } else { 0 }).sum::<usize>();
        pieces
            .into_iter()
            .map(|piece| match piece {
                Piece::Words(words) => {
                    shown += words.chars().count();
                    let mut data = delta.clone();
                    data.insert(key.into(), Value::String(words));
                    rebuilt(kind.to_string(), data)
                }
                Piece::File(path) => {
                    let mut data = Map::new();
                    data.insert("event".into(), json!(FILE));
                    data.insert("run_id".into(), delta.get("run_id").cloned().unwrap_or(Value::Null));
                    if let Value::Object(file) = crate::threads::timeline::file_attachment(&path, Some(shown)) {
                        data.extend(file);
                    }
                    rebuilt(FILE.to_string(), data)
                }
            })
            .collect()
    }

    /// A whole text without its MEDIA lines; at the end, their files added to its `attachments`.
    fn whole(&self, event: SSEEvent) -> SSEEvent {
        let kind = event.r#type.as_str();
        let end = crate::hermes::is_terminal(kind) || kind == "assistant.completed";
        if !end && !TEXT_EVENTS.contains(&kind) {
            return event;
        }
        let mut data = event.data;
        let mut changed = false;
        let mut placed: Option<Vec<Value>> = None;
        for key in TEXT_KEYS {
            let Some(Value::String(text)) = data.get(key).cloned() else { continue };
            if !MEDIA_RE.is_match(&text) {
                continue;
            }
            let (shown, files) = crate::threads::timeline::place_media(&text);
            data.insert(key.into(), Value::String(shown));
            placed.get_or_insert(files);
            changed = true;
        }
        if let (true, Some(files)) = (end, placed) {
            let stale: Vec<String> = data
                .get("stale")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let given: Vec<Value> = data.get("attachments").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut all: Vec<Value> = Vec::new();
            for file in files.into_iter().chain(given) {
                let path = file["path"].as_str().unwrap_or_default();
                if stale.iter().any(|s| s == path) || all.iter().any(|a| a["path"].as_str() == Some(path)) {
                    continue;
                }
                all.push(file);
            }
            data.insert("attachments".into(), Value::Array(all));
        }
        if changed {
            rebuilt(event.r#type, data)
        } else {
            SSEEvent::new(event.r#type, data, event.raw)
        }
    }
}

fn rebuilt(kind: String, data: Map<String, Value>) -> SSEEvent {
    let raw = crate::py::Dumps::new().unicode().dumps(&Value::Object(data.clone()));
    SSEEvent::new(kind, data, raw)
}

static MEDIA_WHOLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$").expect("valid regex"));

/// The path of a whole MEDIA line.
fn media_path(line: &str) -> Option<String> {
    MEDIA_WHOLE.captures(line).map(|c| c[1].to_string())
}

/// A line start that may still be a MEDIA line once whole.
fn may_become_media(start: &str) -> bool {
    let start = start.trim_start_matches([' ', '\t']);
    if start.len() >= "MEDIA:".len() {
        start.starts_with("MEDIA:") && !start["MEDIA:".len()..].trim_matches([' ', '\t']).contains(char::is_whitespace)
    } else {
        "MEDIA:".starts_with(start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words, a file as « [path] ».
    fn read(pieces: Vec<Piece>) -> String {
        pieces
            .into_iter()
            .map(|p| match p {
                Piece::Words(w) => w,
                Piece::File(f) => format!("[{f}]"),
            })
            .collect()
    }

    fn shown(chunks: &[&str]) -> String {
        let mut filter = MediaFilter::default();
        let mut out: String = chunks.iter().map(|c| read(filter.text(c))).collect();
        out.push_str(&read(filter.flush()));
        out
    }

    #[test]
    fn a_reply_streams_its_media_lines_as_files_in_their_places() {
        assert_eq!(shown(&["Bonjour", " toi"]), "Bonjour toi");
        assert_eq!(shown(&["A :\n\nMEDIA:/p/x.html\n\nB"]), "A :\n\n[/p/x.html]\nB");
        // a line cut anywhere across deltas, spaces around, the last line without its newline
        assert_eq!(shown(&["A\n", "M", "ED", "IA: /x.mp3 ", "\nB\nMEDIA:/y.png"]), "A\n[/x.mp3]B\n[/y.png]");
        // only a whole line is one: mid-line, with words after, or another word starting with M
        assert_eq!(
            shown(&["Voir MEDIA:/x.png\n", "MEDIA:/a b\n", "Merci\n", "MED"]),
            "Voir MEDIA:/x.png\nMEDIA:/a b\nMerci\nMED"
        );
        // words go out at once, a line start that could still be one waits for its next characters
        let mut filter = MediaFilter::default();
        assert_eq!(read(filter.text("Le plan\nM")), "Le plan\n");
        assert_eq!(read(filter.text("ais")), "Mais");
        assert_eq!(filter.said(), "Le plan\nMais");
    }

    #[test]
    fn a_delta_becomes_its_words_and_file_events() {
        let delta = |text: &str| {
            SSEEvent::new(
                "message.delta",
                json!({"event": "message.delta", "run_id": "r1", "delta": text}).as_object().cloned().unwrap(),
                "{}",
            )
        };
        let mut filter = MediaFilter::default();
        let out = filter.event(delta("Voici 🏃 :\nMEDIA:/bipagents/pages/a/p.html\nLa suite"));
        let kinds: Vec<&str> = out.iter().map(|e| e.r#type.as_str()).collect();
        assert_eq!(kinds, ["message.delta", FILE, "message.delta"]);
        assert_eq!((out[0].text(), out[2].text()), ("Voici 🏃 :\n".to_string(), "La suite".to_string()));
        let file = &out[1].data;
        assert_eq!(
            (file["path"].as_str(), file["kind"].as_str(), file["offset"].as_i64(), file["run_id"].as_str()),
            (Some("/bipagents/pages/a/p.html"), Some("page"), Some(10), Some("r1"))
        );
        assert!(out.iter().all(|e| !e.raw.contains("MEDIA:")));
        // a delta all MEDIA line: its file only; the last line at the end, as a file
        let only = filter.event(delta("\nMEDIA:/z.png\nMEDIA:/y.png"));
        assert_eq!(only.iter().map(|e| e.r#type.as_str()).collect::<Vec<_>>(), ["message.delta", FILE]);
        let end = filter.flush_events("message.delta", &json!("r1"));
        assert_eq!(
            (end.len(), end[0].data["path"].as_str(), end[0].data["offset"].as_i64()),
            (1, Some("/y.png"), Some(19))
        );
    }

    #[test]
    fn a_whole_text_and_the_end_lose_their_media_lines_for_attachments() {
        let event = |kind: &str, data: serde_json::Value| SSEEvent::new(kind, data.as_object().cloned().unwrap(), "{}");
        let mut filter = MediaFilter::default();
        let interim = filter.event(event("message.interim", json!({"text": "Je regarde.\nMEDIA:/x.png"}))).remove(0);
        assert_eq!(interim.data["text"], "Je regarde.");
        assert!(!interim.data.contains_key("attachments") && !interim.raw.contains("MEDIA"));
        let end = filter
            .event(event(
                "run.completed",
                json!({
            "output": "Voilà :\nMEDIA:/old.mp3\nMEDIA:/new.mp3", "stale": ["/old.mp3"],
            "attachments": [{"path": "/new.mp3"}, {"path": "/made.html"}]}),
            ))
            .remove(0);
        assert_eq!(end.data["output"], "Voilà :");
        let paths: Vec<&str> =
            end.data["attachments"].as_array().unwrap().iter().filter_map(|a| a["path"].as_str()).collect();
        assert_eq!(paths, ["/new.mp3", "/made.html"], "stale ones out, each file once");
        // nothing to take out: the event as it came
        let plain = filter
            .event(SSEEvent::new("run.completed", json!({"output": "Ok"}).as_object().cloned().unwrap(), "RAW"))
            .remove(0);
        assert_eq!(plain.raw, "RAW");
    }

    #[test]
    fn extract_and_resolve_media() {
        let text = "MEDIA:/home/hermes/simplex-files/point.mp3\n\n🎙️ Point du matin — lundi 5 octobre";
        assert_eq!(
            extract_media(text),
            (
                "🎙️ Point du matin — lundi 5 octobre".to_string(),
                vec!["/home/hermes/simplex-files/point.mp3".to_string()]
            )
        );
        assert_eq!(extract_media("plain"), ("plain".to_string(), vec![]));
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("simplex");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("point.mp3"), b"ID3podcast").unwrap();
        std::fs::write(dir.path().join("secret.mp3"), b"no").unwrap();
        let mut roots = IndexMap::new();
        roots.insert("/home/hermes/simplex-files".to_string(), root.to_string_lossy().into_owned());
        let real = std::fs::canonicalize(root.join("point.mp3")).unwrap().to_string_lossy().into_owned();
        assert_eq!(resolve("/home/hermes/simplex-files/point.mp3", &roots), Some(real));
        assert_eq!(resolve("/home/hermes/simplex-files/../secret.mp3", &roots), None); // traversal
        assert_eq!(resolve("/home/hermes/simplex-files/missing.mp3", &roots), None);
        assert_eq!(resolve("/etc/passwd", &roots), None);
        std::fs::write(root.join("notes.sh"), "x").unwrap();
        assert_eq!(resolve("/home/hermes/simplex-files/notes.sh", &roots), None); // not media
        std::fs::write(root.join("courses.xlsx"), "PK").unwrap();
        assert!(resolve("/home/hermes/simplex-files/courses.xlsx", &roots).is_some()); // a document
                                                                                       // a symlink out of the root is not served
        std::os::unix::fs::symlink(dir.path().join("secret.mp3"), root.join("link.mp3")).unwrap();
        assert_eq!(resolve("/home/hermes/simplex-files/link.mp3", &roots), None);
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type("/a/b.MP3"), "audio/mpeg");
        assert_eq!(content_type("/a/page.html"), "text/html; charset=utf-8");
        assert_eq!(content_type("/a/.mp3"), "application/octet-stream");
        assert_eq!(content_type("/a.b/c"), "application/octet-stream");
        assert_eq!(content_type("x.tar.gz"), "application/octet-stream");
        assert_eq!(
            content_type("/envoi/Courses.XLSX"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(content_type("/envoi/notes.md"), "text/markdown");
        assert!(is_document("/envoi/a.docx") && is_document("/envoi/a.numbers") && !is_document("/envoi/a.html"));
        assert!(!is_document("/envoi/a.pdf") && !is_document("/envoi/a.sh"));
    }
}
