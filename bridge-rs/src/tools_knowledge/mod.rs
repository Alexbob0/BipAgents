//! Private helpers of the tools-knowledge group (agent_tools, pages, mailbox, mail_scan, memory_edit, notes_import,
//! reminders, jobs, favicons): Python's string and file behaviours these modules rely on (`str.strip()`'s whitespace,
//! `read_text`'s universal newlines, NFKD slugs), its HTML parser (`html.parser`, `html.unescape`), the `email`
//! package's header and date handling, and an `imaplib`-like IMAP client.
//!
//! Not a Python module: plumbing shared by the group's modules.

pub mod email;
pub mod entities;
pub mod html;
pub mod imap;

use std::path::Path;

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Python's `str.isspace()` for one character.
pub fn is_py_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}

/// `s.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_py_space)
}

/// `s.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_py_space)
}

/// `s.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_py_space)
}

/// `s.split()`.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_py_space).filter(|p| !p.is_empty()).collect()
}

/// `" ".join(s.split())`.
pub fn squeeze(s: &str) -> String {
    split_ws(s).join(" ")
}

/// Text from elsewhere (a family message, a sender's name, a background report) put inside a `<!--bip:… -->` note for
/// the agent: its own `<!--` and `-->` get non-breaking hyphens (U+2011, they look the same), so it can neither end
/// the note early nor open a fake one (audit 2026-10-27 #15). The apps' and live_brief's strip, lazy up to the first
/// `-->`, then removes exactly the note.
pub fn in_note(s: &str) -> String {
    let mut out = s.to_string();
    loop {
        let next = out.replace("-->", "\u{2011}\u{2011}>").replace("<!--", "<!\u{2011}\u{2011}");
        if next == out {
            return out;
        }
        out = next;
    }
}

/// `in_note` on one line: control characters (new lines included) become spaces, runs of spaces one.
pub fn in_note_line(s: &str) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    in_note(&squeeze(&flat))
}

/// `s[:n]` (in characters).
pub fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// `s[n:]` (in characters).
pub fn tail_from(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[i..],
        None => "",
    }
}

/// `len(s)` (in characters).
pub fn chars(s: &str) -> usize {
    s.chars().count()
}

/// `s.capitalize()`: the first character in title case, the rest lower case.
pub fn capitalize(s: &str) -> String {
    let mut it = s.chars();
    match it.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(it.as_str().to_lowercase().chars()).collect(),
    }
}

/// `str.isdigit()` on a non-empty string (ASCII digits: the ids it checks are IMAP uids).
pub fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

/// `unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode()`.
pub fn ascii_fold(text: &str) -> String {
    text.nfkd().filter(char::is_ascii).collect()
}

/// Punctuation that the ASCII folding would drop (typographic apostrophes and quotes, dashes) made spaces, and the
/// ligatures and letters it would lose spelled out (« œ » → « oe », « ß » → « ss »): what is left keeps its words
/// apart (« l’enquête » → « l enquête », not « lenquête »).
pub fn spaced_punctuation(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'œ' => out.push_str("oe"),
            'Œ' => out.push_str("OE"),
            'æ' => out.push_str("ae"),
            'Æ' => out.push_str("AE"),
            'ß' => out.push_str("ss"),
            'ø' => out.push('o'),
            'Ø' => out.push('O'),
            'ł' => out.push('l'),
            'Ł' => out.push('L'),
            'đ' => out.push('d'),
            c if c.is_alphanumeric() || c.is_ascii() => out.push(c),
            c if c.is_alphabetic() => out.push(c),
            // a combining accent stays with its letter
            c if unicode_normalization::char::is_combining_mark(c) => out.push(c),
            _ => out.push(' '),
        }
    }
    out
}

/// `re.sub(r"[^a-z0-9]+", "-", text)` on an ASCII lower-case text.
pub fn dashes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

/// `path.read_text(encoding="utf-8")`: strict UTF-8 (an error is `InvalidData`, Python's UnicodeDecodeError), and
/// universal newlines (`\r\n` and `\r` read as `\n`).
pub fn read_text(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(universal_newlines(text))
}

/// Python's text-mode reading: `\r\n` and lone `\r` become `\n`.
pub fn universal_newlines(text: String) -> String {
    if !text.contains('\r') {
        return text;
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `path.write_text(text, encoding="utf-8")` then `os.replace(temporary, path)`.
pub fn replace_with(temporary: &Path, path: &Path, data: &[u8]) -> std::io::Result<()> {
    std::fs::write(temporary, data)?;
    std::fs::rename(temporary, path)
}

/// `path.stat().st_mtime` (seconds, as Python computes it from the nanoseconds).
pub fn mtime(path: &Path) -> Option<f64> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(meta.mtime() as f64 + meta.mtime_nsec() as f64 * 1e-9)
}

/// `path.stem` of a file name.
pub fn stem(name: &str) -> &str {
    let base = name.rsplit('/').next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 && i < base.len() - 1 => &base[..i],
        _ => base,
    }
}

/// `needle in container` for a JSON value (a list's items, a dict's keys, a string's substrings).
pub fn py_in(needle: &str, container: &Value) -> bool {
    match container {
        Value::Array(items) => items.iter().any(|v| v.as_str() == Some(needle)),
        Value::Object(map) => map.contains_key(needle),
        Value::String(s) => s.contains(needle),
        _ => false,
    }
}

/// `{"content": [{"type": "text", "text": text}], "isError": error}`: an MCP tool's result.
pub fn tool_text(text: &str, error: bool) -> crate::Dict {
    let mut out = crate::Dict::new();
    out.insert("content".into(), serde_json::json!([{"type": "text", "text": text}]));
    out.insert("isError".into(), Value::Bool(error));
    out
}

/// A failure the agent tends to repeat to the person word for word: what went wrong (for the agent), then a plain
/// sentence it can pass on instead (PLAIN_RULE, souls.rs).
pub fn tool_failure(what: &str, plain: &str) -> crate::Dict {
    tool_text(&format!("{what}\nTell your person simply, in their language: « {plain} »"), true)
}

/// `isinstance(value, int)` (a bool is one in Python): its value.
pub fn as_py_int(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::Bool(b) => Some(*b as i64),
        Value::Number(n) if n.is_i64() => n.as_i64(),
        Value::Number(n) if n.is_u64() => n.as_u64().map(|u| u.min(i64::MAX as u64) as i64),
        _ => None,
    }
}

/// `str(value or "")`.
pub fn str_or_empty(value: Option<&Value>) -> String {
    match value {
        Some(v) if crate::py::truthy(v) => crate::py::py_str(v),
        _ => String::new(),
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn now_iso_seconds() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_string_helpers() {
        assert_eq!(strip("\u{1c} a b \u{a0}"), "a b");
        assert_eq!(squeeze("  Mon   <b>titre</b>\n "), "Mon <b>titre</b>");
        assert_eq!(head("Élodie", 2), "Él");
        assert_eq!(capitalize("suivi-de-sante"), "Suivi-de-sante");
        assert_eq!(dashes(&ascii_fold("Prises de sang à jeûn").to_lowercase()), "prises-de-sang-a-jeun");
        assert_eq!(stem("Takeout/Keep/Courses.json"), "Courses");
        assert_eq!(stem("a.tar.gz"), "a.tar");
        assert_eq!(stem(".bashrc"), ".bashrc");
        assert_eq!(universal_newlines("a\r\nb\rc".into()), "a\nb\nc");
    }
}
