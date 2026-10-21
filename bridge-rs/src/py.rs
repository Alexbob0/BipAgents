//! Python-compatible helpers the port relies on to behave exactly like the Python bridge: `json.dumps` with its
//! defaults (`", "` / `": "` separators, `ensure_ascii`), `os.path` expansion and normalisation, `secrets` tokens,
//! and the lenient conversions `parse_config` applies to TOML values (`str()`, `int()`, `float()`, truthiness).
//!
//! Not a Python module of its own: shared plumbing for every module (core group).

use std::fmt::Write as _;

use rand::RngCore;
use serde_json::Value;

// -- json.dumps ------------------------------------------------------------------------------------

/// Options of Python's `json.dumps` that the bridge uses.
#[derive(Clone, Debug)]
pub struct Dumps {
    pub ensure_ascii: bool,
    pub sort_keys: bool,
    /// `indent=N`: newlines and N spaces per level (item separator then `","`, as Python does).
    pub indent: Option<usize>,
    /// `separators=(item, key)`; None: Python's default (`", "`, `": "`, or `","`, `": "` with an indent).
    pub separators: Option<(&'static str, &'static str)>,
}

impl Default for Dumps {
    fn default() -> Self {
        Dumps { ensure_ascii: true, sort_keys: false, indent: None, separators: None }
    }
}

impl Dumps {
    /// `json.dumps(value)`.
    pub fn new() -> Self {
        Self::default()
    }
    /// `separators=(",", ":")`.
    pub fn compact(mut self) -> Self {
        self.separators = Some((",", ":"));
        self
    }
    /// `ensure_ascii=False`.
    pub fn unicode(mut self) -> Self {
        self.ensure_ascii = false;
        self
    }
    pub fn sort_keys(mut self) -> Self {
        self.sort_keys = true;
        self
    }
    pub fn indent(mut self, indent: usize) -> Self {
        self.indent = Some(indent);
        self
    }

    pub fn dumps(&self, value: &Value) -> String {
        let mut out = String::new();
        self.write(&mut out, value, 0);
        out
    }

    fn seps(&self) -> (&'static str, &'static str) {
        match self.separators {
            Some(s) => s,
            None if self.indent.is_some() => (",", ": "),
            None => (", ", ": "),
        }
    }

    fn newline(&self, out: &mut String, level: usize) {
        if let Some(indent) = self.indent {
            out.push('\n');
            for _ in 0..indent * level {
                out.push(' ');
            }
        }
    }

    fn write(&self, out: &mut String, value: &Value, level: usize) {
        let (item_sep, key_sep) = self.seps();
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) => {
                if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                    out.push_str(&float_repr(f));
                } else {
                    let _ = write!(out, "{n}");
                }
            }
            Value::String(s) => write_string(out, s, self.ensure_ascii),
            Value::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    self.newline(out, level + 1);
                    self.write(out, item, level + 1);
                }
                self.newline(out, level);
                out.push(']');
            }
            Value::Object(map) => {
                if map.is_empty() {
                    out.push_str("{}");
                    return;
                }
                let mut keys: Vec<&String> = map.keys().collect();
                if self.sort_keys {
                    // Python sorts str keys by code point; Rust's str ordering is by bytes (UTF-8): the same order.
                    keys.sort();
                }
                out.push('{');
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push_str(item_sep);
                    }
                    self.newline(out, level + 1);
                    write_string(out, key, self.ensure_ascii);
                    out.push_str(key_sep);
                    self.write(out, &map[*key], level + 1);
                }
                self.newline(out, level);
                out.push('}');
            }
        }
    }
}

/// `json.dumps(value)` with Python's defaults.
pub fn dumps(value: &Value) -> String {
    Dumps::default().dumps(value)
}

fn write_string(out: &mut String, s: &str, ensure_ascii: bool) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{:04x}", unit);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(float)`: shortest round-trip digits, `1.0`, `1e+16`, `1.5e-07`, `inf`/`nan` as JSON writes them.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    // Rust's `{:e}` gives the shortest round-trip digits: "1.2345e3", "-5e-7".
    let sci = format!("{:e}", f);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let negative = mantissa.starts_with('-');
    let digits: String = mantissa.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if negative { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let (head, tail) = digits.split_at(1);
        let mant = if tail.is_empty() { head.to_string() } else { format!("{head}.{tail}") };
        let esign = if exp < 0 { '-' } else { '+' };
        return format!("{sign}{mant}e{esign}{:02}", exp.abs());
    }
    let n = digits.len() as i32;
    let point = exp + 1; // digits before the decimal point
    let body = if point <= 0 {
        format!("0.{}{}", "0".repeat((-point) as usize), digits)
    } else if point >= n {
        format!("{}{}.0", digits, "0".repeat((point - n) as usize))
    } else {
        let (a, b) = digits.split_at(point as usize);
        format!("{a}.{b}")
    };
    format!("{sign}{body}")
}

// -- os.path ---------------------------------------------------------------------------------------

/// `os.path.abspath(os.path.expanduser(os.path.expandvars(path)))`: what config.py's `_expand` does.
pub fn expand(path: &str) -> String {
    abspath(&expanduser(&expandvars(path)))
}

/// `os.path.expandvars`: `$NAME` and `${NAME}` replaced when NAME is set, left as they are otherwise.
pub fn expandvars(path: &str) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let re = regex::Regex::new(r"\$([A-Za-z0-9_]+|\{[^}]*\})").expect("valid regex");
    let mut out = String::new();
    let mut last = 0;
    for m in re.find_iter(path) {
        let mut name = &path[m.start() + 1..m.end()];
        if name.starts_with('{') && name.ends_with('}') {
            name = &name[1..name.len() - 1];
        }
        out.push_str(&path[last..m.start()]);
        match std::env::var(name) {
            Ok(value) => out.push_str(&value),
            Err(_) => out.push_str(m.as_str()),
        }
        last = m.end();
    }
    out.push_str(&path[last..]);
    out
}

/// `os.path.expanduser`: `~` (HOME, else the account's home) and `~user`.
pub fn expanduser(path: &str) -> String {
    if !path.starts_with('~') {
        return path.to_string();
    }
    let i = path[1..].find('/').map(|p| p + 1).unwrap_or(path.len());
    let home = if i == 1 {
        match std::env::var("HOME") {
            Ok(h) => Some(h),
            Err(_) => home_of_uid(),
        }
    } else {
        home_of_user(&path[1..i])
    };
    let Some(home) = home else { return path.to_string() };
    let home = home.trim_end_matches('/');
    let joined = format!("{home}{}", &path[i..]);
    if joined.is_empty() {
        "/".into()
    } else {
        joined
    }
}

fn home_of_uid() -> Option<String> {
    // SAFETY: getpwuid returns a pointer into static storage (or null); read at once, never kept.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

fn home_of_user(name: &str) -> Option<String> {
    let cname = std::ffi::CString::new(name).ok()?;
    // SAFETY: as above.
    unsafe {
        let pw = libc::getpwnam(cname.as_ptr());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())
    }
}

/// `os.path.abspath`: joined with the current directory, then normalised.
pub fn abspath(path: &str) -> String {
    if path.starts_with('/') {
        normpath(path)
    } else {
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "/".into());
        normpath(&format!("{cwd}/{path}"))
    }
}

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let mut initial = if path.starts_with('/') { 1 } else { 0 };
    if initial == 1 && path.starts_with("//") && !path.starts_with("///") {
        initial = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".." || (initial == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let joined = comps.join("/");
    let out = format!("{}{}", "/".repeat(initial), joined);
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

/// `os.path.join(a, b)` for two parts (an absolute `b` wins, as in Python).
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') || a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `os.path.dirname`.
pub fn dirname(path: &str) -> String {
    let i = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let head = &path[..i];
    if !head.is_empty() && head != "/".repeat(head.len()) {
        head.trim_end_matches('/').to_string()
    } else {
        head.to_string()
    }
}

/// `os.path.basename`.
pub fn basename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or("").to_string()
}

// -- secrets / uuid --------------------------------------------------------------------------------

/// `secrets.token_urlsafe(nbytes)`.
pub fn token_urlsafe(nbytes: usize) -> String {
    use base64::Engine;
    let mut buf = vec![0u8; nbytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// `secrets.token_hex(nbytes)`.
pub fn token_hex(nbytes: usize) -> String {
    let mut buf = vec![0u8; nbytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// `uuid.uuid4().hex`.
pub fn uuid4_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

// -- Python conversions of config values -----------------------------------------------------------

/// Python's `str(value)` for a value read from TOML/JSON (`True`, `1.0`, `['a', 'b']`…).
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => {
            if n.is_f64() {
                let s = float_repr(n.as_f64().unwrap_or(0.0));
                match s.as_str() {
                    "NaN" => "nan".into(),
                    "Infinity" => "inf".into(),
                    "-Infinity" => "-inf".into(),
                    _ => s,
                }
            } else {
                n.to_string()
            }
        }
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python's `repr()` of a JSON-like value.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::String(s) => {
            let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
            let mut out = String::new();
            out.push(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                        let _ = write!(out, "\\x{:02x}", c as u32);
                    }
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", py_repr(&Value::String(k.clone())), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => py_str(other),
    }
}

/// Python truthiness (`bool(value)`).
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python's `int(value)`: bools, ints, floats (truncated) and numeric strings (`" 42 "`, `"1_000"`).
pub fn py_int(value: &Value) -> Result<i64, String> {
    match value {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() {
                    Ok(f.trunc() as i64)
                } else {
                    Err("cannot convert float infinity to integer".into())
                }
            } else {
                Err(format!("invalid literal for int() with base 10: '{n}'"))
            }
        }
        Value::String(s) => {
            parse_int_literal(s).ok_or_else(|| format!("invalid literal for int() with base 10: {}", py_repr(value)))
        }
        Value::Null => {
            Err("int() argument must be a string, a bytes-like object or a real number, not 'NoneType'".into())
        }
        Value::Array(_) => {
            Err("int() argument must be a string, a bytes-like object or a real number, not 'list'".into())
        }
        Value::Object(_) => {
            Err("int() argument must be a string, a bytes-like object or a real number, not 'dict'".into())
        }
    }
}

fn parse_int_literal(s: &str) -> Option<i64> {
    let t = s.trim();
    let (sign, digits) = match t.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    clean.parse::<i64>().ok().map(|v| v * sign)
}

/// Python's `float(value)`.
pub fn py_float(value: &Value) -> Result<f64, String> {
    match value {
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::Number(n) => n.as_f64().ok_or_else(|| "invalid number".to_string()),
        Value::String(s) => {
            let t = s.trim().replace('_', "");
            let lower = t.to_ascii_lowercase();
            match lower.trim_start_matches(['+', '-']) {
                "inf" | "infinity" => Ok(if lower.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY }),
                "nan" => Ok(f64::NAN),
                _ => t.parse::<f64>().map_err(|_| format!("could not convert string to float: {}", py_repr(value))),
            }
        }
        Value::Null => Err("float() argument must be a string or a real number, not 'NoneType'".into()),
        Value::Array(_) => Err("float() argument must be a string or a real number, not 'list'".into()),
        Value::Object(_) => Err("float() argument must be a string or a real number, not 'dict'".into()),
    }
}

/// `str.rstrip("/")`.
pub fn rstrip_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// `os.makedirs(path, mode=mode, exist_ok=True)`: missing parents with the default mode, the folder itself with
/// `mode` (the umask applies, as in Python); an existing folder is left as it is.
pub fn makedirs(path: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let p = std::path::Path::new(path);
    if p.is_dir() {
        return Ok(());
    }
    if let Some(parent) = p.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            std::fs::create_dir_all(parent)?;
        }
    }
    match std::fs::DirBuilder::new().mode(mode).create(p) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && p.is_dir() => Ok(()),
        other => other,
    }
}

/// `os.chmod(path, mode)`.
pub fn chmod(path: &str, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// `os.open(path, O_WRONLY | O_CREAT | O_TRUNC, mode)` then write: a new file is created with `mode` (umask
/// applied); an existing one keeps its mode, as in Python.
pub fn write_with_mode(path: &str, data: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut fh = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(path)?;
    fh.write_all(data)
}

// -- clocks ----------------------------------------------------------------------------------------

/// A clock in seconds, replaceable in tests (Python's `clock=time.time` / `time.monotonic` parameters).
pub type Clock = std::sync::Arc<dyn Fn() -> f64 + Send + Sync>;

/// `time.time()`.
pub fn time_time() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `time.time` as a [`Clock`].
pub fn wall_clock() -> Clock {
    std::sync::Arc::new(time_time)
}

/// `time.monotonic()`: seconds since an arbitrary start (never 0).
pub fn monotonic() -> f64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    1_000.0 + START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// `time.monotonic` as a [`Clock`].
pub fn monotonic_clock() -> Clock {
    std::sync::Arc::new(monotonic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dumps_like_python() {
        assert_eq!(dumps(&json!(["b", "a"])), r#"["b", "a"]"#);
        assert_eq!(
            dumps(&json!({"a": 1, "b": [true, null], "c": "é"})),
            "{\"a\": 1, \"b\": [true, null], \"c\": \"\\u00e9\"}"
        );
        assert_eq!(Dumps::new().compact().unicode().dumps(&json!({"x": "é😀"})), r#"{"x":"é😀"}"#);
        assert_eq!(dumps(&json!("😀")), "\"\\ud83d\\ude00\"");
        assert_eq!(
            Dumps::new().indent(1).dumps(&json!({"t": {"a": [1]}})),
            "{\n \"t\": {\n  \"a\": [\n   1\n  ]\n }\n}"
        );
        assert_eq!(dumps(&json!({})), "{}");
    }

    #[test]
    fn floats_like_python() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(0.1), "0.1");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1.5e-7), "1.5e-07");
        assert_eq!(float_repr(123456.789), "123456.789");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(-2.5), "-2.5");
        assert_eq!(float_repr(1e15), "1000000000000000.0");
    }

    #[test]
    fn paths_like_python() {
        assert_eq!(normpath("/a/./b/../c//d/"), "/a/c/d");
        assert_eq!(normpath("//a"), "//a");
        assert_eq!(normpath("///a"), "/a");
        assert_eq!(normpath("../x"), "../x");
        assert_eq!(normpath(""), ".");
        assert_eq!(dirname("/a/b/c.db"), "/a/b");
        assert_eq!(dirname("/c.db"), "/");
        assert_eq!(dirname("c.db"), "");
        std::env::set_var("BIP_PY_TEST", "v");
        assert_eq!(expandvars("$BIP_PY_TEST/${BIP_PY_TEST}/$NOPE_NOT_SET"), "v/v/$NOPE_NOT_SET");
    }

    #[test]
    fn conversions_like_python() {
        assert_eq!(py_int(&json!("1_000")), Ok(1000));
        assert_eq!(py_int(&json!(" 7 ")), Ok(7));
        assert_eq!(py_int(&json!(2.9)), Ok(2));
        assert!(py_int(&json!("x")).is_err());
        assert_eq!(py_str(&json!(true)), "True");
        assert_eq!(py_str(&json!(1.0)), "1.0");
        assert_eq!(py_str(&json!(["a", 1])), "['a', 1]");
        assert!(!truthy(&json!("")) && truthy(&json!("0")) && !truthy(&json!(0)));
    }
}
