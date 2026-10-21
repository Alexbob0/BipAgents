//! Files the agents produce and point to with a `MEDIA:<path>` line (the podcast mp3, a chart…).
//!
//! Hermes runs in a container: the path is the agent's (`/home/hermes/simplex-files/x.mp3`). `[media]
//! roots` maps agent-side prefixes to host directories; only files under a declared root, with a known
//! media extension, are ever served (symlinks and `..` resolved first).
//!
//! Port of `bridge/bipbridge/media.py` (owner group: push-runs).

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

/// `^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$` (multiline).
pub const MEDIA_LINE: &str = r"(?m)^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$";

pub const CONTENT_TYPES: [(&str, &str); 18] = [
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

/// `os.path.splitext(path)[1].lower()`: the last component's extension (leading dots are not one).
fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem_start = name.len() - name.trim_start_matches('.').len();
    match name[stem_start..].rfind('.') {
        Some(i) => name[stem_start + i..].to_lowercase(),
        None => String::new(),
    }
}

/// The host file for an agent-side path, or `None` if it is outside every root, missing, or not media.
pub fn resolve(path: &str, roots: &IndexMap<String, String>) -> Option<String> {
    let ext = extension(path);
    if !CONTENT_TYPES.iter().any(|(e, _)| *e == ext) {
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
    let ext = extension(path);
    CONTENT_TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, t)| t.to_string())
        .unwrap_or_else(|| "application/octet-stream".into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }
}
