//! A WhatsApp export read back into messages, with no model: the text WhatsApp writes with « Exporter la discussion »
//! › « Sans médias » (a `.zip` holding `_chat.txt` on iPhone, a `.txt` on Android), in any of its date and time
//! layouts:
//!
//!     iPhone    [25/10/2026 14:03:12] Léa Martin: Salut          (FR, ES: day first; DE: 25.10.26, 14:03:12)
//!               [10/25/26, 2:03:12 PM] Lea Martin: Hi            (EN-US: month first, 12-hour clock)
//!     Android   25/10/2026 14:03 - Léa Martin: Salut
//!               10/25/26, 2:03 PM - Lea Martin: Hi               (a narrow no-break space before PM, often)
//!               25.10.26, 14:03 - Léa Martin: Hallo
//!               25/10/26 2:03 p. m. - Léa Martin: Hola
//!
//! The day and month order is found from the whole file (a first number above 12 is a day, a second above 12 a day
//! too; else dots and the 24-hour clock mean day first, a 12-hour clock month first). Lines without a date continue the
//! message before (multi-line messages). WhatsApp's own lines are left out: « Les messages et les appels sont chiffrés
//! de bout en bout… », someone joined or left, a group renamed (on iPhone those start with a left-to-right mark), and
//! so are media placeholders (« <Médias omis> », « image omitted »), deleted messages and « <Ce message a été
//! modifié> » marks. The direction marks WhatsApp scatters (U+200E…) are taken out everywhere.

use std::io::Read;
use std::sync::LazyLock;

use chrono::{NaiveDate, NaiveDateTime};
use regex::Regex;
use sha2::{Digest, Sha256};

/// The largest export read (a chat « sans médias » of 40,000 messages is a few MB).
pub const MAX_TEXT_BYTES: u64 = 40 * 1024 * 1024;

/// One message of the chat.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    /// Local time as the export shows it, `YYYY-MM-DD HH:MM:SS`.
    pub at: String,
    pub sender: String,
    pub text: String,
}

impl Message {
    pub fn when(&self) -> Option<NaiveDateTime> {
        NaiveDateTime::parse_from_str(&self.at, "%Y-%m-%d %H:%M:%S").ok()
    }

    /// The month it belongs to (`YYYY-MM`), the archive's file.
    pub fn month(&self) -> &str {
        self.at.get(..7).unwrap_or("0000-00")
    }

    /// The same message in any export of the chat: its time, its sender, its text (16 hex).
    pub fn fingerprint(&self) -> String {
        let digest = Sha256::digest(format!("{}\u{1f}{}\u{1f}{}", self.at, self.sender, self.text).as_bytes());
        hex::encode(digest)[..16].to_string()
    }

    /// As the archive keeps it: `[2026-10-25 14:03] Léa Martin: Salut`, the next lines indented.
    pub fn line(&self) -> String {
        let at = self.at.get(..16).unwrap_or(&self.at);
        let text = self.text.replace('\n', "\n  ");
        format!("[{at}] {}: {text}", self.sender)
    }
}

/// A chat read from an export.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Chat {
    /// From the file's name (« WhatsApp Chat - Léa Martin.zip » → « Léa Martin »), else empty.
    pub title: String,
    /// The senders, the most active first.
    pub participants: Vec<String>,
    pub messages: Vec<Message>,
    /// Media placeholders left out (photos, voice notes, documents…).
    pub media: usize,
    /// WhatsApp's own lines and deleted messages left out.
    pub system: usize,
}

impl Chat {
    pub fn first(&self) -> Option<&Message> {
        self.messages.first()
    }

    pub fn last(&self) -> Option<&Message> {
        self.messages.last()
    }

    /// The name shown: the title, else the participants (« Léa, Marc et Sophie »).
    pub fn name(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        let people: Vec<&str> = self.participants.iter().take(4).map(String::as_str).collect();
        if people.is_empty() {
            "WhatsApp".into()
        } else {
            people.join(", ")
        }
    }
}

/// Why a file can't be read as an export (a message for the person).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct NotAnExport(pub String);

/// The direction and invisible marks WhatsApp puts around names and system lines.
fn is_mark(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{feff}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}')
}

fn unmarked(text: &str) -> String {
    text.chars().filter(|c| !is_mark(*c)).collect()
}

/// A date and time at the start of a line: day/month/year in some order, then the time.
const STAMP: &str = r"(\d{1,4})[./-](\d{1,2})[./-](\d{1,4}),?\s+(\d{1,2})[:.](\d{2})(?:[:.](\d{2}))?(?:[\s\u{202f}\u{a0}]*([AaPp])\.?[\s\u{202f}\u{a0}]?[Mm]\.?)?";

static IOS: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\[{STAMP}\]\s?(.*)$")).expect("valid regex"));
static ANDROID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^{STAMP}\s+[-–]\s(.*)$")).expect("valid regex"));

/// A media placeholder: « <Médias omis> », « <Media omitted> », « image omitted », « IMG-….jpg (fichier joint) ».
static MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(<[^<>]{3,40}>|.*\b(omitted|omis|omise|omises|absente?s?|weggelassen|ausgeschlossen|omitid[oa]s?)|.*\((file attached|fichier joint|datei angehängt|archivo adjunto)\)|null)$",
    )
    .expect("valid regex")
});

/// WhatsApp's own lines and deleted messages (any language WhatsApp writes them in, folded).
const SYSTEM_TEXTS: [&str; 16] = [
    "this message was deleted",
    "you deleted this message",
    "ce message a ete supprime",
    "vous avez supprime ce message",
    "tu as supprime ce message",
    "diese nachricht wurde geloscht",
    "du hast diese nachricht geloscht",
    "se elimino este mensaje",
    "eliminaste este mensaje",
    "end-to-end encrypted",
    "chiffres de bout en bout",
    "ende-zu-ende-verschlusselt",
    "cifrados de extremo a extremo",
    "waiting for this message",
    "en attente de ce message",
    "missed voice call",
];

/// « <This message was edited> » and its translations, at a message's end.
static EDITED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s*<(this message was edited|ce message a été modifié|diese nachricht wurde bearbeitet|se editó este mensaje)>\s*$")
        .expect("valid regex")
});

fn fold(text: &str) -> String {
    crate::tools_knowledge::ascii_fold(&text.to_lowercase())
}

fn is_system(text: &str) -> bool {
    let folded = fold(text);
    SYSTEM_TEXTS.iter().any(|t| folded.contains(t))
}

/// One stamped line as read: the numbers, the 12-hour mark, the rest.
struct Raw {
    a: u32,
    b: u32,
    c: u32,
    a_len: usize,
    hour: u32,
    minute: u32,
    second: u32,
    pm: Option<bool>,
    dots: bool,
    rest: String,
    /// The line started with a direction mark (an iPhone system or media line).
    marked: bool,
}

fn stamped(line: &str) -> Option<Raw> {
    let marked = line.starts_with(is_mark);
    let clean = unmarked(line);
    let (caps, ios) = match IOS.captures(&clean) {
        Some(c) => (c, true),
        None => (ANDROID.captures(&clean)?, false),
    };
    let n = |i: usize| caps.get(i).and_then(|m| m.as_str().parse::<u32>().ok());
    let rest = caps.get(8).map(|m| m.as_str()).unwrap_or("");
    // the text after « Name: » on an iPhone line may start with the mark that says it is WhatsApp's own
    let raw_rest = if ios {
        line.find(']').map(|i| line[i + 1..].trim_start_matches([' ']).to_string()).unwrap_or_default()
    } else {
        rest.to_string()
    };
    Some(Raw {
        a: n(1)?,
        b: n(2)?,
        c: n(3)?,
        a_len: caps.get(1).map(|m| m.as_str().len()).unwrap_or(0),
        hour: n(4)?,
        minute: n(5)?,
        second: n(6).unwrap_or(0),
        pm: caps.get(7).map(|m| m.as_str().eq_ignore_ascii_case("p")),
        dots: clean.get(..12).is_some_and(|head| head.contains('.') && !head.contains('/')),
        rest: raw_rest,
        marked,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Order {
    Dmy,
    Mdy,
    Ymd,
}

/// The day and month order, from all the stamped lines.
fn order_of(raws: &[&Raw]) -> Order {
    if raws.iter().any(|r| r.a_len == 4) {
        return Order::Ymd;
    }
    if raws.iter().any(|r| r.a > 12) {
        return Order::Dmy;
    }
    if raws.iter().any(|r| r.b > 12) {
        return Order::Mdy;
    }
    if raws.iter().any(|r| r.dots) {
        return Order::Dmy;
    }
    if raws.iter().any(|r| r.pm.is_some()) {
        return Order::Mdy;
    }
    Order::Dmy
}

fn date_time(raw: &Raw, order: Order) -> Option<NaiveDateTime> {
    let (y, m, d) = match order {
        Order::Dmy => (raw.c, raw.b, raw.a),
        Order::Mdy => (raw.c, raw.a, raw.b),
        Order::Ymd => (raw.a, raw.b, raw.c),
    };
    let year = if y < 100 { 2000 + y as i32 } else { y as i32 };
    let mut hour = raw.hour;
    match raw.pm {
        Some(true) if hour < 12 => hour += 12,
        Some(false) if hour == 12 => hour = 0,
        _ => {}
    }
    NaiveDate::from_ymd_opt(year, m, d)?.and_hms_opt(hour, raw.minute, raw.second)
}

/// « Léa Martin: Salut » → (« Léa Martin », « Salut »); None for a line without a sender (Android's system lines).
fn split_sender(rest: &str) -> Option<(String, String)> {
    let at = rest.find(": ").or_else(|| rest.strip_suffix(':').map(|r| r.len()))?;
    let sender = unmarked(&rest[..at]).trim().to_string();
    if sender.is_empty() || sender.chars().count() > 60 || sender.contains(['“', '”', '«', '»', '"']) {
        return None;
    }
    let text = rest.get(at + 2..).unwrap_or("").to_string();
    Some((sender, text))
}

/// The chat in `text` (the export's whole text). `file_name`: what the person shared, for the title.
pub fn parse(text: &str, file_name: &str) -> Result<Chat, NotAnExport> {
    let lines: Vec<&str> = text.lines().collect();
    let raws: Vec<Option<Raw>> = lines.iter().map(|l| stamped(l)).collect();
    let stamped_lines: Vec<&Raw> = raws.iter().flatten().collect();
    if stamped_lines.len() < 2 && !(stamped_lines.len() == 1 && lines.len() <= 3) {
        return Err(NotAnExport("this file isn't a WhatsApp chat export".into()));
    }
    let order = order_of(&stamped_lines);
    let mut chat = Chat { title: title_of(file_name), ..Chat::default() };
    // (message, kept): a message left out still takes its continuation lines
    let mut current: Option<(Message, bool)> = None;
    let mut out: Vec<Message> = Vec::new();
    let flush = |current: &mut Option<(Message, bool)>, out: &mut Vec<Message>| {
        if let Some((mut m, keep)) = current.take() {
            m.text = EDITED.replace(m.text.trim_end(), "").trim().to_string();
            if keep && !m.text.is_empty() {
                out.push(m);
            }
        }
    };
    for (line, raw) in lines.iter().zip(&raws) {
        let Some(raw) = raw else {
            if let Some((m, _)) = current.as_mut() {
                let more = unmarked(line);
                m.text.push('\n');
                m.text.push_str(more.trim_end());
            }
            continue;
        };
        flush(&mut current, &mut out);
        let Some(at) = date_time(raw, order) else { continue };
        let at = at.format("%Y-%m-%d %H:%M:%S").to_string();
        match split_sender(&raw.rest) {
            Some((sender, text)) => {
                let marked_text = text.starts_with(is_mark) || raw.marked;
                let clean = unmarked(&text).trim().to_string();
                if MEDIA.is_match(&clean)
                    && (marked_text || clean.starts_with('<') || clean.ends_with(')') || clean == "null")
                {
                    chat.media += 1;
                    current = Some((Message { at, sender, text: String::new() }, false));
                } else if marked_text || is_system(&clean) {
                    // an iPhone system line (« ‎Léa a créé ce groupe »), a deleted message
                    chat.system += 1;
                    current = Some((Message { at, sender, text: String::new() }, false));
                } else {
                    current = Some((Message { at, sender, text: clean }, true));
                }
            }
            None => {
                chat.system += 1;
                current = Some((Message { at, sender: String::new(), text: String::new() }, false));
            }
        }
    }
    flush(&mut current, &mut out);
    if out.is_empty() && chat.media == 0 {
        return Err(NotAnExport("no message found in this WhatsApp export".into()));
    }
    // the iPhone's system lines carry the chat's name as their sender: not a participant unless it wrote
    let mut counts: Vec<(String, usize)> = Vec::new();
    for m in &out {
        match counts.iter_mut().find(|(s, _)| *s == m.sender) {
            Some((_, n)) => *n += 1,
            None => counts.push((m.sender.clone(), 1)),
        }
    }
    counts.sort_by_key(|c| std::cmp::Reverse(c.1));
    chat.participants = counts.into_iter().map(|(s, _)| s).collect();
    chat.messages = out;
    Ok(chat)
}

/// The chat's title from the shared file's name, in the languages WhatsApp names it.
pub fn title_of(file_name: &str) -> String {
    static PREFIX: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?i)^\s*(?:whatsapp[\s_-]*chat|discussion[\s_-]*whatsapp|chat[\s_-]*de[\s_-]*whatsapp|conversa[\s_-]*do[\s_-]*whatsapp|whatsapp[\s_-]*unterhaltung)\s*(?:-|–|(?:with|avec|mit|con|com)\b)?\s*",
        )
        .expect("valid regex")
    });
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = base.strip_suffix(".zip").or_else(|| base.strip_suffix(".txt")).unwrap_or(base);
    let stem = stem.strip_suffix(".txt").unwrap_or(stem);
    if !PREFIX.is_match(stem) {
        return String::new();
    }
    let title = unmarked(&PREFIX.replace(stem, "")).replace('_', " ");
    static COPY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\(\d{1,3}\)$").expect("valid regex"));
    let title = COPY.replace(title.trim(), "");
    let title = title.trim();
    crate::tools_knowledge::head(title, 80).to_string()
}

/// The export's text from what was shared: a `.zip` (its `_chat.txt`, or its only `.txt`) or a `.txt`.
pub fn text_of(file_name: &str, raw: &[u8]) -> Result<String, NotAnExport> {
    let bytes = if raw.starts_with(b"PK\x03\x04") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(raw))
            .map_err(|_| NotAnExport("this zip can't be opened".into()))?;
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        let pick = names
            .iter()
            .find(|n| n.rsplit('/').next() == Some("_chat.txt"))
            .or_else(|| names.iter().find(|n| n.to_lowercase().ends_with(".txt") && !n.contains("__MACOSX")))
            .cloned()
            .ok_or_else(|| NotAnExport("no chat in this zip (WhatsApp puts it in _chat.txt)".into()))?;
        let mut out = Vec::new();
        archive
            .by_name(&pick)
            .map_err(|_| NotAnExport("this zip can't be opened".into()))?
            .take(MAX_TEXT_BYTES)
            .read_to_end(&mut out)
            .map_err(|_| NotAnExport("this zip can't be opened".into()))?;
        out
    } else {
        let lower = file_name.to_lowercase();
        if !(lower.ends_with(".txt") || lower.is_empty() || !lower.contains('.')) {
            return Err(NotAnExport("share the WhatsApp export (.zip or .txt)".into()));
        }
        raw.to_vec()
    };
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    // UTF-16 (an old Windows export) read too
    if bytes.len() >= 2 && (bytes[..2] == [0xff, 0xfe] || bytes[..2] == [0xfe, 0xff]) {
        let le = bytes[0] == 0xff;
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) })
            .collect();
        return Ok(String::from_utf16_lossy(&units));
    }
    Ok(String::from_utf8_lossy(bytes).replace("\r\n", "\n"))
}
