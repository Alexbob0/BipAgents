//! Small Python behaviours the push-runs modules rely on (not a Python module of their own): `str.isspace()`,
//! `str.strip()` / `str.split()` with Python's whitespace (it includes `\x1c`-`\x1f`, Rust's does not), slicing
//! by code points, and an error that carries the name of the Python exception it stands for (what a log line
//! shows as `type(exc).__name__`).

use std::fmt;

pub use crate::voice_speech::pystr::{is_space, len, rstrip, split_ws as split, strip};

/// `" ".join(text.split())`.
pub fn squash(text: &str) -> String {
    split(text).join(" ")
}

/// `text[:n]` (code points).
pub fn head(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// An exception Python lets through (a `KeyError`, a `ValueError`…): its class name, for the log lines that show
/// `type(exc).__name__`, and its message.
#[derive(Debug)]
pub struct PyError {
    pub name: &'static str,
    pub message: String,
}

impl PyError {
    pub fn new(name: &'static str, message: impl Into<String>) -> Self {
        PyError { name, message: message.into() }
    }
}

impl fmt::Display for PyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PyError {}

/// `type(exc).__name__` for an error that went through `anyhow`.
pub fn error_name(err: &anyhow::Error) -> &'static str {
    if let Some(e) = err.downcast_ref::<PyError>() {
        return e.name;
    }
    if let Some(e) = err.downcast_ref::<crate::hermes::HermesError>() {
        return e.type_name();
    }
    if let Some(e) = err.downcast_ref::<reqwest::Error>() {
        return crate::hermes::httpx_error_name(e);
    }
    if let Some(e) = err.downcast_ref::<std::io::Error>() {
        return match e.kind() {
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            _ => "OSError",
        };
    }
    if err.downcast_ref::<rusqlite::Error>().is_some() {
        return "OperationalError";
    }
    if err.downcast_ref::<serde_json::Error>().is_some() {
        return "JSONDecodeError";
    }
    "Exception"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_like_python() {
        assert_eq!(strip("\u{1c} a b \u{3000}"), "a b");
        assert_eq!(squash(" a\n\tb\u{1f}c  "), "a b c");
        assert_eq!(rstrip("ab \n"), "ab");
        assert_eq!(head("éàü", 2), "éà");
        assert_eq!(head("ab", 5), "ab");
        assert_eq!(len("é😀"), 2);
        assert!(!is_space('\u{200b}'));
    }
}
