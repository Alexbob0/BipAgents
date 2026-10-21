//! Small Python behaviours the browsers-places modules rely on (not a Python module of their own): `str.strip()`
//! with Python's whitespace, `str.isprintable()`, `shlex.quote`, `shutil.which`, `hmac.new(...).hexdigest()`,
//! `urllib.parse.urlsplit`'s host and port, `json.loads`' error text, and `Path.home()`.

use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

/// `str.isspace()` for one character.
pub fn is_space(c: char) -> bool {
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

/// `text.strip()`.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `text[:n]` (code points).
pub fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `text[-n:]` (code points).
pub fn tail(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(n)).collect()
}

/// `str.isprintable()`: no character of the categories Cc, Cf, Cs, Co, Cn, Zl, Zp, Zs (the space excepted).
pub fn isprintable(text: &str) -> bool {
    use unicode_general_category::{get_general_category, GeneralCategory as G};
    text.chars().all(|c| {
        c == ' '
            || !matches!(
                get_general_category(c),
                G::Control
                    | G::Format
                    | G::Surrogate
                    | G::PrivateUse
                    | G::Unassigned
                    | G::LineSeparator
                    | G::ParagraphSeparator
                    | G::SpaceSeparator
            )
    })
}

/// `shlex.quote(s)`.
pub fn shlex_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    let safe = s.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// `shlex.join(args)`.
pub fn shlex_join<S: AsRef<str>>(args: &[S]) -> String {
    args.iter().map(|a| shlex_quote(a.as_ref())).collect::<Vec<_>>().join(" ")
}

fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

/// `shutil.which(cmd)`: a command with a folder in it is checked as it is, else looked up in `$PATH`.
pub fn which(cmd: &str) -> Option<String> {
    if cmd.contains('/') {
        return executable(Path::new(cmd)).then(|| cmd.to_string());
    }
    let path = std::env::var("PATH").unwrap_or_else(|_| "/bin:/usr/bin".into());
    for dir in path.split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(cmd);
        if executable(&candidate) {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// `hmac.new(key, "\n".join(parts).encode(), hashlib.sha256).hexdigest()`.
pub fn hmac_hex(key: &[u8], parts: &[&str]) -> String {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key");
    mac.update(parts.join("\n").as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// `hmac.compare_digest(a, b)` on two str (Python refuses non-ASCII ones: a TypeError, here None).
pub fn compare_digest(a: &str, b: &str) -> Option<bool> {
    if !a.is_ascii() || !b.is_ascii() {
        return None;
    }
    Some(crate::auth::constant_time_eq(a.as_bytes(), b.as_bytes()))
}

/// `Path(os.environ.get("HOME") or Path.home())`.
pub fn home() -> PathBuf {
    match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => PathBuf::from(crate::py::expanduser("~")),
    }
}

/// `str(exc)` of the `json.JSONDecodeError` that `json.loads(text)` raises, None when it parses.
pub fn json_error(bytes: &[u8]) -> Option<String> {
    let err = serde_json::from_slice::<Value>(bytes).err()?;
    let text = String::from_utf8_lossy(bytes).into_owned();
    // serde's line/column (1-based) → a character offset, as Python counts.
    let (line, column) = (err.line(), err.column());
    let mut pos = 0usize;
    for (i, l) in text.split('\n').enumerate() {
        if i + 1 == line {
            pos += if err.is_eof() { column } else { column.saturating_sub(1) };
            break;
        }
        pos += l.chars().count() + 1;
    }
    let message = err.to_string();
    let starts = |p: &str| message.starts_with(p);
    let last = text.trim_end().chars().last();
    let what = if starts("EOF while parsing a value") || starts("expected value") || starts("expected ident") {
        "Expecting value"
    } else if starts("EOF while parsing an object") && matches!(last, Some('{') | Some(',')) {
        "Expecting property name enclosed in double quotes"
    } else if (starts("EOF while parsing an object") && last == Some(':'))
        || (starts("EOF while parsing a list") && matches!(last, Some('[') | Some(',')))
    {
        "Expecting value"
    } else if starts("EOF while parsing an object") || starts("EOF while parsing a list") || starts("expected `,` or") {
        "Expecting ',' delimiter"
    } else if starts("expected `:`") {
        "Expecting ':' delimiter"
    } else if starts("key must be a string") || starts("EOF while parsing an object key") {
        "Expecting property name enclosed in double quotes"
    } else if starts("EOF while parsing a string") {
        "Unterminated string starting at"
    } else if starts("trailing comma") {
        if text.chars().nth(pos) == Some('}') {
            "Expecting property name enclosed in double quotes"
        } else {
            "Expecting value"
        }
    } else if starts("trailing characters") {
        "Extra data"
    } else if starts("control character") {
        "Invalid control character at"
    } else if starts("invalid escape") || starts("invalid unicode code point") {
        "Invalid \\escape"
    } else {
        "Expecting value"
    };
    if what == "Unterminated string starting at" {
        // Python points at the opening quote
        let chars: Vec<char> = text.chars().collect();
        let mut at = pos.min(chars.len());
        while at > 0 {
            at -= 1;
            if chars[at] == '"' {
                break;
            }
        }
        pos = at;
    }
    let chars: Vec<char> = text.chars().collect();
    let before: String = chars[..pos.min(chars.len())].iter().collect();
    let lineno = before.matches('\n').count() + 1;
    let colno = match before.rfind('\n') {
        Some(i) => before[i + 1..].chars().count() + 1,
        None => pos + 1,
    };
    Some(format!("{what}: line {lineno} column {colno} (char {pos})"))
}

/// What `urllib.parse.urlsplit(url)` gives for the parts the bridge reads: (scheme, hostname, port). Its errors
/// (`ValueError`) as their messages.
pub struct Split {
    pub scheme: String,
    pub netloc: String,
}

const SCHEME_CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+-.";

pub fn urlsplit(url: &str) -> Result<Split, String> {
    let url: String = url.chars().filter(|c| !matches!(c, '\t' | '\r' | '\n')).collect();
    let url = url.trim_start_matches(|c: char| (c as u32) <= 0x20);
    let mut scheme = String::new();
    let mut rest = url;
    if let Some(i) = url.find(':') {
        let first = url.chars().next().unwrap_or(' ');
        if i > 0 && first.is_ascii_alphabetic() && url[..i].chars().all(|c| SCHEME_CHARS.contains(c)) {
            scheme = url[..i].to_ascii_lowercase();
            rest = &url[i + 1..];
        }
    }
    let mut netloc = String::new();
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = after[..end].to_string();
        if (netloc.contains('[') && !netloc.contains(']')) || (netloc.contains(']') && !netloc.contains('[')) {
            return Err("Invalid IPv6 URL".into());
        }
    }
    Ok(Split { scheme, netloc })
}

impl Split {
    fn hostinfo(&self) -> (String, Option<String>) {
        let hostinfo = self.netloc.rsplit_once('@').map(|(_, h)| h).unwrap_or(&self.netloc);
        let (hostname, port) = if let Some((_, bracketed)) = hostinfo.split_once('[') {
            let (host, after) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            let port = after.split_once(':').map(|(_, p)| p).unwrap_or("");
            (host.to_string(), port.to_string())
        } else {
            let (host, port) = hostinfo.split_once(':').unwrap_or((hostinfo, ""));
            (host.to_string(), port.to_string())
        };
        (hostname, if port.is_empty() { None } else { Some(port) })
    }

    /// `.hostname`: lowercased, None when empty.
    pub fn hostname(&self) -> Option<String> {
        let (host, _) = self.hostinfo();
        if host.is_empty() {
            return None;
        }
        Some(match host.split_once('%') {
            Some((h, zone)) => format!("{}%{zone}", h.to_lowercase()),
            None => host.to_lowercase(),
        })
    }

    /// `.port`, or its `ValueError`.
    pub fn port(&self) -> Result<Option<u32>, String> {
        let (_, port) = self.hostinfo();
        let Some(port) = port else { return Ok(None) };
        if !(port.chars().all(|c| c.is_ascii_digit())) {
            return Err(format!(
                "Port could not be cast to integer value as {}",
                crate::py::py_repr(&Value::String(port))
            ));
        }
        match port.parse::<u64>() {
            Ok(p) if p <= 65535 => Ok(Some(p as u32)),
            _ => Err("Port out of range 0-65535".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_behaviours() {
        assert_eq!(shlex_quote("a-b/c.d"), "a-b/c.d");
        assert_eq!(shlex_quote("a;b"), "'a;b'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote(""), "''");
        assert_eq!(strip("\u{1c} x \n"), "x");
        assert_eq!(json_error(b"").as_deref(), Some("Expecting value: line 1 column 1 (char 0)"));
        assert_eq!(
            json_error(b"{").as_deref(),
            Some("Expecting property name enclosed in double quotes: line 1 column 2 (char 1)")
        );
        assert_eq!(json_error(b"{\"a\" 1}").as_deref(), Some("Expecting ':' delimiter: line 1 column 6 (char 5)"));
        assert_eq!(json_error(b"{\"a\": 1}x").as_deref(), Some("Extra data: line 1 column 9 (char 8)"));
        assert_eq!(json_error(b"\"abc").as_deref(), Some("Unterminated string starting at: line 1 column 1 (char 0)"));
        assert_eq!(json_error(b"{}"), None);
        let s = urlsplit("HTTPS://User@Example.COM:8443/x").unwrap();
        assert_eq!(
            (s.scheme.as_str(), s.hostname().as_deref(), s.port()),
            ("https", Some("example.com"), Ok(Some(8443)))
        );
        assert_eq!(
            urlsplit("https://x:ab").unwrap().port(),
            Err("Port could not be cast to integer value as 'ab'".into())
        );
        assert_eq!(urlsplit("https://x:99999").unwrap().port(), Err("Port out of range 0-65535".into()));
        assert!(urlsplit("https://[::1").is_err());
        assert_eq!(hmac_hex(b"k", &["a", "b"]).len(), 64);
    }
}
