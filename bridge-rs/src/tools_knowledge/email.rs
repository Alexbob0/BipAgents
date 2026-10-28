//! Python's `email` package as mailbox.py and mail_scan.py use it with `policy.default`: header values read as its
//! header registry gives them (unfolded, RFC 2047 words decoded, addresses and dates written again its way),
//! `email.header.decode_header` + `make_header`, `utils.parsedate_to_datetime` / `format_datetime` / `formatdate` /
//! `make_msgid`, `imaplib.Time2Internaldate`, the walk over a message's parts (mail-parser underneath), and the draft
//! an `EmailMessage` with `set_content` writes (`as_bytes(policy=SMTP)`).
//!
//! Not a Python module: the standard library parts the group needs.

use std::sync::LazyLock;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Timelike};
use mail_parser::{Address, MessageParser, MimeHeaders, PartType};
use regex::Regex;

use super::{is_py_space, split_ws};

// -- dates -----------------------------------------------------------------------------------------

const MONTHNAMES: [&str; 24] = [
    "jan",
    "feb",
    "mar",
    "apr",
    "may",
    "jun",
    "jul",
    "aug",
    "sep",
    "oct",
    "nov",
    "dec",
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];
const DAYNAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const TIMEZONES: [(&str, i64); 14] = [
    ("UT", 0),
    ("UTC", 0),
    ("GMT", 0),
    ("Z", 0),
    ("AST", -400),
    ("ADT", -300),
    ("EST", -500),
    ("EDT", -400),
    ("CST", -600),
    ("CDT", -500),
    ("MST", -700),
    ("MDT", -600),
    ("PST", -800),
    ("PDT", -700),
];
const MONTHS_TITLE: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAYS_TITLE: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Python's `int(text)` on a date's piece (sign, digits, underscores between digits).
fn py_int(text: &str) -> Option<i64> {
    let t = text.trim_matches(is_py_space);
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
    clean.parse::<i64>().ok().map(|n| sign * n)
}

/// `email._parseaddr._parsedate_tz`: (year, month, day, hour, minute, second) and the offset in seconds (None for
/// `-0000` and unknown zones).
pub fn parsedate_tz(data: &str) -> Option<([i64; 6], Option<i64>)> {
    let mut data: Vec<String> = split_ws(data).into_iter().map(str::to_string).collect();
    if data.is_empty() {
        return None;
    }
    if data[0].ends_with(',') || DAYNAMES.contains(&data[0].to_lowercase().as_str()) {
        data.remove(0);
    } else if let Some(i) = data[0].rfind(',') {
        data[0] = data[0][i + 1..].to_string();
    }
    if data.len() == 3 {
        let stuff: Vec<String> = data[0].split('-').map(str::to_string).collect();
        if stuff.len() == 3 {
            let mut joined = stuff;
            joined.extend(data[1..].iter().cloned());
            data = joined;
        }
    }
    if data.len() == 4 {
        let s = data[3].clone();
        let i = s.find('+').or_else(|| s.find('-'));
        match i {
            Some(i) if i > 0 => {
                data.truncate(3);
                data.push(s[..i].to_string());
                data.push(s[i..].to_string());
            }
            _ => data.push(String::new()),
        }
    }
    if data.len() < 5 {
        return None;
    }
    data.truncate(5);
    let (mut dd, mut mm, mut yy, mut tm, mut tz) =
        (data[0].clone(), data[1].clone(), data[2].clone(), data[3].clone(), data[4].clone());
    if dd.is_empty() || mm.is_empty() || yy.is_empty() {
        return None;
    }
    mm = mm.to_lowercase();
    if !MONTHNAMES.contains(&mm.as_str()) {
        let swapped = dd.to_lowercase();
        dd = mm;
        mm = swapped;
        if !MONTHNAMES.contains(&mm.as_str()) {
            return None;
        }
    }
    let mut month = MONTHNAMES.iter().position(|m| *m == mm).unwrap_or(0) as i64 + 1;
    if month > 12 {
        month -= 12;
    }
    if dd.ends_with(',') {
        dd.pop();
    }
    if let Some(i) = yy.find(':') {
        if i > 0 {
            std::mem::swap(&mut yy, &mut tm);
        }
    }
    if yy.ends_with(',') {
        yy.pop();
        if yy.is_empty() {
            return None;
        }
    }
    if !yy.chars().next().map(|c| c.is_numeric()).unwrap_or(false) {
        std::mem::swap(&mut yy, &mut tz);
    }
    if tm.ends_with(',') {
        tm.pop();
    }
    let parts: Vec<&str> = tm.split(':').collect();
    let (thh, tmm, tss) = match parts.len() {
        2 => (parts[0].to_string(), parts[1].to_string(), "0".to_string()),
        3 => (parts[0].to_string(), parts[1].to_string(), parts[2].to_string()),
        1 if parts[0].contains('.') => {
            let dotted: Vec<&str> = parts[0].split('.').collect();
            match dotted.len() {
                2 => (dotted[0].to_string(), dotted[1].to_string(), "0".to_string()),
                3 => (dotted[0].to_string(), dotted[1].to_string(), dotted[2].to_string()),
                _ => return None,
            }
        }
        _ => return None,
    };
    let (mut year, day, hour, minute, second) =
        (py_int(&yy)?, py_int(&dd)?, py_int(&thh)?, py_int(&tmm)?, py_int(&tss)?);
    if year < 100 {
        year += if year > 68 { 1900 } else { 2000 };
    }
    let tz = tz.to_uppercase();
    let mut offset = match TIMEZONES.iter().find(|(name, _)| *name == tz) {
        Some((_, value)) => Some(*value),
        None => {
            let value = py_int(&tz);
            if value == Some(0) && tz.starts_with('-') {
                None
            } else {
                value
            }
        }
    };
    if let Some(value) = offset.filter(|v| *v != 0) {
        let (sign, value) = if value < 0 { (-1, -value) } else { (1, value) };
        offset = Some(sign * ((value / 100) * 3600 + (value % 100) * 60));
    }
    Some(([year, month, day, hour, minute, second], offset))
}

/// A `datetime` from `parsedate_to_datetime`: aware (its offset in seconds) or naive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PyDateTime {
    pub naive: NaiveDateTime,
    pub offset: Option<i32>,
}

impl PyDateTime {
    /// `utils.format_datetime(dt)`: « Sun, 18 Oct 2026 10:00:00 +0200 », `-0000` for a naive one.
    pub fn format(&self) -> String {
        let n = self.naive;
        let zone = match self.offset {
            None => "-0000".to_string(),
            Some(seconds) => {
                let sign = if seconds < 0 { '-' } else { '+' };
                let s = seconds.abs();
                if s % 60 != 0 {
                    format!("{sign}{:02}{:02}{:02}", s / 3600, (s % 3600) / 60, s % 60)
                } else {
                    format!("{sign}{:02}{:02}", s / 3600, (s % 3600) / 60)
                }
            }
        };
        format!(
            "{}, {:02} {} {:04} {:02}:{:02}:{:02} {zone}",
            DAYS_TITLE[n.weekday().num_days_from_monday() as usize],
            n.day(),
            MONTHS_TITLE[n.month0() as usize],
            n.year(),
            n.hour(),
            n.minute(),
            n.second()
        )
    }

    /// `dt.date().isoformat()`.
    pub fn date_iso(&self) -> String {
        self.naive.format("%Y-%m-%d").to_string()
    }

    /// `dt.strftime("%Y-%m")`.
    pub fn month(&self) -> String {
        self.naive.format("%Y-%m").to_string()
    }
}

/// `utils.parsedate_to_datetime(data)` (None: Python's TypeError or ValueError).
pub fn parsedate_to_datetime(data: Option<&str>) -> Option<PyDateTime> {
    let data = data?;
    if data.is_empty() {
        return None;
    }
    let ([year, month, day, hour, minute, second], offset) = parsedate_tz(data)?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year as i32, u32::try_from(month).ok()?, u32::try_from(day).ok()?)?;
    if !(0..24).contains(&hour) || !(0..60).contains(&minute) || !(0..60).contains(&second) {
        return None;
    }
    let naive = date.and_hms_opt(hour as u32, minute as u32, second as u32)?;
    if let Some(seconds) = offset {
        if seconds.abs() >= 86400 {
            return None;
        }
    }
    Some(PyDateTime { naive, offset: offset.map(|s| s as i32) })
}

/// `utils.formatdate(localtime=True)`.
pub fn formatdate_local() -> String {
    let now = Local::now();
    PyDateTime { naive: now.naive_local(), offset: Some(now.offset().local_minus_utc()) }.format()
}

/// `imaplib.Time2Internaldate(time.time())`: « "21-Oct-2026 10:00:00 +0200" ».
pub fn internaldate_now() -> String {
    let now = Local::now();
    format!("\"{}-{}-{}\"", now.format("%d"), MONTHS_TITLE[now.month0() as usize], now.format("%Y %H:%M:%S %z"))
}

/// `utils.make_msgid()`: « <time*100.pid.random@host> ».
pub fn make_msgid() -> String {
    let timeval = (crate::py::time_time() * 100.0) as i64;
    format!("<{timeval}.{}.{}@{}>", std::process::id(), rand::random::<u64>(), hostname())
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into buf.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return "localhost".into();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

/// `datetime.strftime("%d-%b-%Y")` (C locale) of a date.
pub fn imap_date(date: NaiveDate) -> String {
    format!("{:02}-{}-{}", date.day(), MONTHS_TITLE[date.month0() as usize], date.year())
}

// -- RFC 2047 --------------------------------------------------------------------------------------

static ENCODED_WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"=\?([^?\s]+)\?([QqBb])\?([^?]*?)\?=").expect("valid regex"));

fn decode_payload(encoding: &str, text: &str) -> Option<Vec<u8>> {
    if encoding.eq_ignore_ascii_case("b") {
        use base64::Engine;
        let mut clean: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        while !clean.len().is_multiple_of(4) {
            clean.push('=');
        }
        let engine = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new()
                .with_decode_allow_trailing_bits(true)
                .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
        );
        engine.decode(clean.trim_end_matches('=').as_bytes()).ok()
    } else {
        let bytes = text.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'_' => out.push(b' '),
                b'=' if i + 3 <= bytes.len()
                    && bytes[i + 1].is_ascii_hexdigit()
                    && bytes[i + 2].is_ascii_hexdigit() =>
                {
                    let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("00");
                    out.push(u8::from_str_radix(hex, 16).unwrap_or(0));
                    i += 3;
                    continue;
                }
                b => out.push(b),
            }
            i += 1;
        }
        Some(out)
    }
}

/// Bytes in a charset (`*lang` dropped): None for a charset Python doesn't know.
pub fn decode_charset(charset: &str, bytes: &[u8]) -> Option<String> {
    let charset = charset.split('*').next().unwrap_or(charset).trim().to_lowercase();
    match charset.as_str() {
        "utf-8" | "utf8" => Some(String::from_utf8_lossy(bytes).into_owned()),
        "us-ascii" | "ascii" => Some(bytes.iter().map(|b| if *b < 128 { *b as char } else { '\u{fffd}' }).collect()),
        "iso-8859-1" | "latin-1" | "latin1" | "l1" => Some(bytes.iter().map(|b| *b as char).collect()),
        other => encoding_rs::Encoding::for_label(other.as_bytes())
            .map(|e| e.decode_without_bom_handling(bytes).0.into_owned()),
    }
}

/// The RFC 2047 words of a header decoded (white space between two of them dropped). `strict`: None when a word's
/// charset is unknown (`make_header` raises LookupError), else that word read as ASCII.
pub fn decode_words(value: &str, strict: bool) -> Option<String> {
    let mut out = String::new();
    let mut last_end = 0;
    let mut previous_was_word = false;
    for caps in ENCODED_WORD.captures_iter(value) {
        let whole = caps.get(0).expect("match");
        let between = &value[last_end..whole.start()];
        if !(previous_was_word
            && !between.is_empty()
            && between.chars().all(|c| c == ' ' || c == '\t' || c == '\r' || c == '\n'))
        {
            out.push_str(between);
        }
        let decoded = decode_payload(&caps[2], &caps[3]).and_then(|bytes| match decode_charset(&caps[1], &bytes) {
            Some(text) => Some(text),
            None if strict => None,
            None => Some(bytes.iter().map(|b| if *b < 128 { *b as char } else { '\u{fffd}' }).collect()),
        });
        match decoded {
            Some(text) => out.push_str(&text),
            None if strict => return None,
            None => out.push_str(whole.as_str()),
        }
        previous_was_word = true;
        last_end = whole.end();
    }
    out.push_str(&value[last_end..]);
    Some(out)
}

/// `str(make_header(decode_header(value)))`, the value itself when that fails (mailbox.py's `_decode`).
pub fn decode_header(value: &str) -> String {
    decode_words(value, true).unwrap_or_else(|| value.to_string())
}

// -- headers ---------------------------------------------------------------------------------------

/// A message's headers in order: the name as written, the value unfolded (`policy.default`'s source parse).
#[derive(Debug, Clone, Default)]
pub struct Headers {
    pub fields: Vec<(String, String)>,
}

static HEADER_NAME: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| regex::bytes::Regex::new(r"^[\x21-\x39\x3b-\x7e]+:").expect("valid regex"));

impl Headers {
    /// The header block of a raw message (up to the first empty line).
    pub fn parse(raw: &[u8]) -> Headers {
        let mut fields: Vec<(String, Vec<u8>)> = Vec::new();
        let mut rest = raw;
        let mut first = true;
        while !rest.is_empty() {
            let end = rest.iter().position(|b| *b == b'\n').map(|p| p + 1).unwrap_or(rest.len());
            let line = &rest[..end];
            rest = &rest[end..];
            let bare = line.strip_suffix(b"\n").unwrap_or(line);
            let bare = bare.strip_suffix(b"\r").unwrap_or(bare);
            if bare.is_empty() {
                break;
            }
            if line[0] == b' ' || line[0] == b'\t' {
                if let Some(last) = fields.last_mut() {
                    last.1.extend_from_slice(line);
                }
                continue;
            }
            if first && line.starts_with(b"From ") {
                first = false;
                continue;
            }
            first = false;
            if HEADER_NAME.is_match(line) {
                let colon = line.iter().position(|b| *b == b':').unwrap_or(0);
                let name = String::from_utf8_lossy(&line[..colon]).into_owned();
                let mut value = &line[colon + 1..];
                while let Some((b' ' | b'\t', tail)) = value.split_first() {
                    value = tail;
                }
                fields.push((name, value.to_vec()));
            } else {
                break; // the body starts (a missing header/body separator)
            }
        }
        let fields = fields
            .into_iter()
            .map(|(name, value)| {
                let text = String::from_utf8_lossy(&value).into_owned();
                (name, text.replace(['\r', '\n'], ""))
            })
            .collect();
        Headers { fields }
    }

    /// The raw (unfolded) value of the first header named `name`.
    pub fn raw(&self, name: &str) -> Option<&str> {
        self.fields.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    /// `message.get(name)` with `policy.default`: the value its header class gives (addresses and dates written
    /// again, encoded words decoded).
    pub fn get(&self, name: &str) -> Option<String> {
        let raw = self.raw(name)?;
        let lower = name.to_lowercase();
        Some(match lower.as_str() {
            "from" | "to" | "cc" | "bcc" | "reply-to" | "sender" | "resent-from" | "resent-to" | "resent-cc"
            | "resent-bcc" | "resent-sender" => addresses(raw),
            "date" | "resent-date" | "orig-date" => {
                if raw.is_empty() {
                    String::new()
                } else {
                    parsedate_to_datetime(Some(raw)).map(|d| d.format()).unwrap_or_else(|| raw.to_string())
                }
            }
            "message-id" | "content-type" | "content-disposition" | "content-transfer-encoding" | "mime-version" => {
                raw.to_string()
            }
            _ => decode_words(raw, false).unwrap_or_else(|| raw.to_string()),
        })
    }
}

const SPECIALS: &str = "()<>@,:;.\"[]";

fn quote_if_special(name: &str) -> String {
    if name.chars().any(|c| SPECIALS.contains(c)) {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        name.to_string()
    }
}

fn mailbox(name: Option<&str>, address: Option<&str>) -> String {
    let address = address.unwrap_or("");
    match name.filter(|n| !n.is_empty()) {
        Some(n) => format!("{} <{}>", quote_if_special(n), if address == "<>" { "" } else { address }),
        None if address.is_empty() => "<>".into(),
        None => address.to_string(),
    }
}

/// An address header as Python writes it back: « Name <a@b> », a name with specials in quotes, groups « G: a, b; ».
pub fn addresses(raw: &str) -> String {
    let source = format!("To: {raw}\r\n\r\n");
    let parsed = MessageParser::new().parse_headers(source.as_bytes());
    let formatted = parsed.as_ref().and_then(|m| m.to()).map(|address| match address {
        Address::List(list) => {
            list.iter().map(|a| mailbox(a.name.as_deref(), a.address.as_deref())).collect::<Vec<_>>().join(", ")
        }
        Address::Group(groups) => groups
            .iter()
            .map(|g| {
                let members: Vec<String> =
                    g.addresses.iter().map(|a| mailbox(a.name.as_deref(), a.address.as_deref())).collect();
                match g.name.as_deref() {
                    None if members.len() == 1 => members[0].clone(),
                    name => {
                        let joined = members.join(", ");
                        format!(
                            "{}:{}{};",
                            name.map(quote_if_special).unwrap_or_default(),
                            if joined.is_empty() { "" } else { " " },
                            joined
                        )
                    }
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    });
    match formatted {
        Some(text) if !text.is_empty() || raw.trim().is_empty() => text,
        _ => decode_words(raw, false).unwrap_or_else(|| raw.to_string()),
    }
}

// -- the parts -------------------------------------------------------------------------------------

/// What `message_text` takes from a message's parts (Python's `walk()`).
#[derive(Debug, Clone, Default)]
pub struct Walked {
    pub plain: Option<String>,
    pub html: Option<String>,
    pub files: Vec<String>,
}

fn walk(message: &mail_parser::Message, id: usize, out: &mut Walked, depth: usize) {
    let Some(part) = message.parts.get(id) else { return };
    match &part.body {
        PartType::Multipart(children) => {
            for child in children {
                walk(message, *child as usize, out, depth);
            }
        }
        PartType::Message(inner) if depth < 10 => walk(inner, 0, out, depth + 1),
        body => {
            if let Some(shown) = attached(part) {
                out.files.push(shown);
                return;
            }
            let content_type = match part.content_type() {
                None => "text/plain".to_string(),
                Some(ct) => match &ct.c_subtype {
                    Some(sub) => format!("{}/{}", ct.c_type, sub).to_lowercase(),
                    None => "text/plain".to_string(),
                },
            };
            let text = match body {
                PartType::Text(t) | PartType::Html(t) => Some(t.to_string()),
                _ => None,
            };
            if let Some(text) = text {
                if content_type == "text/plain" && out.plain.is_none() {
                    out.plain = Some(text);
                } else if content_type == "text/html" && out.html.is_none() {
                    out.html = Some(text);
                }
            }
        }
    }
}

/// A part's name when it is an attachment (named, or `Content-Disposition: attachment`): « ? » for a nameless one.
fn attached(part: &mail_parser::MessagePart) -> Option<String> {
    let name = part.attachment_name().map(|n| super::strip(n).to_string()).filter(|n| !n.is_empty());
    let disposition = part.content_disposition().map(|d| d.c_type.to_lowercase());
    if name.is_none() && disposition.as_deref() != Some("attachment") {
        return None;
    }
    Some(name.map(|n| decode_header(&n)).filter(|n| !n.is_empty()).unwrap_or_else(|| "?".into()))
}

/// One attachment: its name as `message_text` lists it, its type, its bytes decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

fn collect(message: &mail_parser::Message, id: usize, out: &mut Vec<Attachment>, depth: usize) {
    let Some(part) = message.parts.get(id) else { return };
    match &part.body {
        PartType::Multipart(children) => {
            for child in children {
                collect(message, *child as usize, out, depth);
            }
        }
        PartType::Message(inner) if depth < 10 => collect(inner, 0, out, depth + 1),
        _ => {
            if let Some(name) = attached(part) {
                let content_type = part
                    .content_type()
                    .map(|ct| match &ct.c_subtype {
                        Some(sub) => format!("{}/{}", ct.c_type, sub).to_lowercase(),
                        None => ct.c_type.to_lowercase(),
                    })
                    .unwrap_or_else(|| "application/octet-stream".into());
                out.push(Attachment { name, content_type, data: part.contents().to_vec() });
            }
        }
    }
}

/// The message's attachments, in the order `parse_message` lists their names.
pub fn attachments(raw: &[u8]) -> Vec<Attachment> {
    let mut out = Vec::new();
    if let Some(message) = MessageParser::new().parse(raw) {
        collect(&message, 0, &mut out, 0);
    }
    out
}

/// The message's headers and its parts' texts.
pub fn parse_message(raw: &[u8]) -> (Headers, Walked) {
    let headers = Headers::parse(raw);
    let mut walked = Walked::default();
    if let Some(message) = MessageParser::new().parse(raw) {
        walk(&message, 0, &mut walked, 0);
    }
    (headers, walked)
}

// -- a draft ---------------------------------------------------------------------------------------

const MAX_LINE: usize = 78;

fn q_word(text: &str) -> String {
    let mut out = String::new();
    for b in text.as_bytes() {
        match *b {
            b' ' => out.push('_'),
            c if c.is_ascii_alphanumeric() || b"-!*+/".contains(&c) => out.push(c as char),
            c => out.push_str(&format!("={c:02X}")),
        }
    }
    out
}

/// A header value as the SMTP policy writes it: ASCII as it is, runs of non-ASCII words as `=?utf-8?q?…?=`
/// (75 characters at most each), folded at 78.
pub fn header_line(name: &str, value: &str) -> String {
    let words: Vec<&str> = value.split(' ').collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    let flush = |run: &mut Vec<&str>, tokens: &mut Vec<String>| {
        if run.is_empty() {
            return;
        }
        let text = run.join(" ");
        // split into encoded words of at most 75 characters, on character boundaries
        let mut current = String::new();
        for c in text.chars() {
            let piece = q_word(&c.to_string());
            if 12 + current.len() + piece.len() > 75 && !current.is_empty() {
                tokens.push(format!("=?utf-8?q?{current}?="));
                current.clear();
            }
            current.push_str(&piece);
        }
        if !current.is_empty() {
            tokens.push(format!("=?utf-8?q?{current}?="));
        }
        run.clear();
    };
    for word in words {
        if word.is_ascii() {
            flush(&mut run, &mut tokens);
            tokens.push(word.to_string());
        } else {
            run.push(word);
        }
    }
    flush(&mut run, &mut tokens);
    let mut lines: Vec<String> = Vec::new();
    let mut line = format!("{name}:");
    for token in &tokens {
        if line.len() + 1 + token.len() > MAX_LINE && line.len() > name.len() + 1 {
            lines.push(line);
            line = String::new();
        }
        line.push(' ');
        line.push_str(token);
    }
    lines.push(line);
    lines.join("\r\n") + "\r\n"
}

/// `quoprimime.body_encode(body, maxlinelen)` (eol `\n`).
fn qp_body(body: &[u8], maxlinelen: usize) -> String {
    if body.is_empty() {
        return String::new();
    }
    let safe = |c: u8| (b' '..=b'~').contains(&c) && c != b'=' || c == b'\t';
    let encoded: String = body
        .iter()
        .map(|&c| if safe(c) || c == b'\r' || c == b'\n' { (c as char).to_string() } else { format!("={c:02X}") })
        .collect();
    let mut out: Vec<String> = Vec::new();
    let maxlinelen1 = maxlinelen - 1;
    for line in crate::hermes::splitlines(&encoded) {
        let chars: Vec<char> = line.chars().collect();
        let mut start: isize = 0;
        let laststart = chars.len() as isize - 1 - maxlinelen as isize;
        while start <= laststart {
            let stop = (start + maxlinelen1 as isize) as usize;
            let s = start as usize;
            if chars[stop - 2] == '=' {
                out.push(chars[s..stop - 1].iter().collect());
                start = stop as isize - 2;
            } else if chars[stop - 1] == '=' {
                out.push(chars[s..stop].iter().collect());
                start = stop as isize - 1;
            } else {
                out.push(chars[s..stop].iter().collect::<String>() + "=");
                start = stop as isize;
            }
        }
        let s = start.max(0) as usize;
        match chars.last() {
            Some(&last) if last == ' ' || last == '\t' => {
                let room = start - laststart;
                let q = if room >= 3 {
                    format!("={:02X}", last as u32)
                } else if room == 2 {
                    format!("{last}=\n")
                } else {
                    format!("=\n={:02X}", last as u32)
                };
                out.push(chars[s..chars.len() - 1].iter().collect::<String>() + &q);
            }
            _ => out.push(chars[s.min(chars.len())..].iter().collect()),
        }
    }
    if matches!(body.last(), Some(b'\r' | b'\n')) {
        out.push(String::new());
    }
    out.join("\n")
}

/// `bytes.splitlines()`.
fn byte_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'\n' => {
                lines.push(&data[start..i]);
                start = i + 1;
            }
            b'\r' => {
                lines.push(&data[start..i]);
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < data.len() {
        lines.push(&data[start..]);
    }
    lines
}

/// `set_content(body)`'s transfer encoding and body (with `\n` ends), as Python's heuristics choose them.
fn text_body(body: &str) -> (&'static str, Vec<u8>) {
    let data = body.as_bytes();
    let lines = byte_lines(data);
    let normal = || {
        let mut out = lines.join(&b"\n"[..]);
        out.push(b'\n');
        out
    };
    if lines.iter().map(|l| l.len()).max().unwrap_or(0) <= MAX_LINE {
        if data.is_ascii() {
            return ("7bit", normal());
        }
        return ("8bit", normal());
    }
    let sniff = {
        let mut s = lines.iter().take(10).copied().collect::<Vec<_>>().join(&b"\n"[..]);
        s.push(b'\n');
        s
    };
    let sniff_qp = qp_body(&sniff, MAX_LINE);
    use base64::Engine;
    let sniff_b64 = base64::engine::general_purpose::STANDARD.encode(&sniff).len() + 1;
    if sniff_qp.len() > sniff_b64 {
        let encoded: String = normal_base64(&sniff_for_all(&lines));
        ("base64", encoded.into_bytes())
    } else if lines.len() <= 10 {
        ("quoted-printable", sniff_qp.into_bytes())
    } else {
        ("quoted-printable", qp_body(&normal(), MAX_LINE).into_bytes())
    }
}

fn sniff_for_all(lines: &[&[u8]]) -> Vec<u8> {
    let mut s = lines.join(&b"\n"[..]);
    s.push(b'\n');
    s
}

/// `_encode_base64(data, 78)`: 57 bytes a line.
fn normal_base64(data: &[u8]) -> String {
    use base64::Engine;
    data.chunks(MAX_LINE / 4 * 3).map(|chunk| base64::engine::general_purpose::STANDARD.encode(chunk) + "\n").collect()
}

/// The draft `EmailMessage` mailbox.py makes, as `as_bytes(policy=SMTP)` writes it. Err: a header value with a
/// line break (Python's ValueError).
pub fn draft_bytes(from: &str, to: &str, subject: &str, in_reply_to: &str, body: &str) -> anyhow::Result<Vec<u8>> {
    let date = formatdate_local();
    let message_id = make_msgid();
    let mut fields: Vec<(&str, &str)> =
        vec![("From", from), ("To", to), ("Subject", subject), ("Date", &date), ("Message-ID", &message_id)];
    if !in_reply_to.is_empty() {
        fields.push(("In-Reply-To", in_reply_to));
        fields.push(("References", in_reply_to));
    }
    let mut out = String::new();
    for (name, value) in &fields {
        if value.contains(['\r', '\n']) {
            anyhow::bail!("Header values may not contain linefeed or carriage return characters");
        }
        out.push_str(&header_line(name, value));
    }
    let (cte, data) = text_body(body);
    out.push_str("Content-Type: text/plain; charset=\"utf-8\"\r\n");
    out.push_str(&format!("Content-Transfer-Encoding: {cte}\r\n"));
    out.push_str("MIME-Version: 1.0\r\n\r\n");
    let mut bytes = out.into_bytes();
    // the body's line ends as CRLF
    for line in byte_lines(&data) {
        bytes.extend_from_slice(line);
        bytes.extend_from_slice(b"\r\n");
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_as_python() {
        let d = parsedate_to_datetime(Some("Mon, 19 Oct 2026 09:00:00 +0200")).unwrap();
        assert_eq!(d.format(), "Mon, 19 Oct 2026 09:00:00 +0200");
        assert_eq!(
            parsedate_to_datetime(Some("19 Oct 2026 09:00 -0000")).unwrap().format(),
            "Mon, 19 Oct 2026 09:00:00 -0000"
        );
        assert_eq!(
            parsedate_to_datetime(Some("Tue, 6 Oct 26 9:00:00 EST")).unwrap().format(),
            "Tue, 06 Oct 2026 09:00:00 -0500"
        );
        assert!(parsedate_to_datetime(Some("not a date")).is_none());
        assert!(parsedate_to_datetime(Some("30 Feb 2026 10:00:00 +0000")).is_none());
        assert_eq!(imap_date(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()), "01-Jul-2026");
    }

    #[test]
    fn headers_decoded() {
        let raw = b"From: Banque <info@banque.example>\r\nSubject: =?utf-8?q?Relev=C3=A9_d=27octobre?=\r\n =?utf-8?b?IQ==?=\r\nTo: \"Sam\" <sam@example.com>, b@x.fr\r\n\r\nbody";
        let headers = Headers::parse(raw);
        assert_eq!(headers.get("subject").unwrap(), "Relevé d'octobre!");
        assert_eq!(headers.get("From").unwrap(), "Banque <info@banque.example>");
        assert_eq!(headers.get("To").unwrap(), "Sam <sam@example.com>, b@x.fr");
        assert_eq!(decode_header("=?unknown?q?x?="), "=?unknown?q?x?=");
        assert_eq!(addresses("\"Dr. Who\" <who@x.uk>"), "\"Dr. Who\" <who@x.uk>");
    }

    #[test]
    fn a_draft() {
        let raw =
            draft_bytes("alex@gmail.com", "Léa <lea@x.fr>", "Re: Repas", "<s1@example.com>", "Oui, avec plaisir !")
                .unwrap();
        let text = String::from_utf8(raw).unwrap();
        assert!(text.starts_with(
            "From: alex@gmail.com\r\nTo: =?utf-8?q?L=C3=A9a?= <lea@x.fr>\r\nSubject: Re: Repas\r\nDate: "
        ));
        assert!(text.contains("In-Reply-To: <s1@example.com>\r\nReferences: <s1@example.com>\r\n"));
        assert!(text.ends_with("Content-Transfer-Encoding: 7bit\r\nMIME-Version: 1.0\r\n\r\nOui, avec plaisir !\r\n"));
        assert_eq!(qp_body(&[b'a'; 100], 78).split('\n').next().unwrap().len(), 78);
    }
}
