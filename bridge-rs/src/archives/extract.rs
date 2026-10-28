//! A document's text, title, sender and date, from the files an archive holds: plain text, Markdown (its YAML
//! frontmatter and « From: / Subject: / Date: » head lines, as e-mail notes have), HTML (stripped), `.eml` messages,
//! `.mbox` files (each message a document) and PDFs when `pdftotext` (poppler) is installed (else a PDF is found by its
//! name only). No model, nothing sent anywhere.

use std::io::{BufRead, Read};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::tools_knowledge as tk;

/// Characters of a document's text put in the index (the rest is still read by `archive_read`).
pub const MAX_TEXT: usize = 200_000;
/// A file larger than this is found by its name only (an mbox is read message by message, whatever its size).
pub const MAX_FILE: u64 = 20 * 1024 * 1024;
/// A message of an mbox, at most (the rest of a huge one, attachments mostly, is skipped).
pub const MAX_MESSAGE: usize = 2 * 1024 * 1024;
/// What a hit shows of its document.
pub const PREVIEW_CHARS: usize = 200;
const PDF_TIMEOUT: Duration = Duration::from_secs(20);

/// The kinds of files read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Markdown,
    Html,
    Eml,
    Mbox,
    Pdf,
}

impl Format {
    pub fn kind(self) -> &'static str {
        match self {
            Format::Text => "text",
            Format::Markdown => "note",
            Format::Html => "page",
            Format::Eml | Format::Mbox => "mail",
            Format::Pdf => "pdf",
        }
    }
}

/// What the archives read, for the apps (« .txt, .md, … »).
pub fn supported() -> Vec<&'static str> {
    let mut out = vec![".txt", ".md", ".html", ".eml", ".mbox"];
    if pdftotext().is_some() {
        out.push(".pdf");
    }
    out
}

/// A file's format from its name.
pub fn format_of(name: &str) -> Option<Format> {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e)?;
    Some(match ext {
        "txt" | "text" | "log" | "csv" => Format::Text,
        "md" | "markdown" | "mdown" => Format::Markdown,
        "html" | "htm" | "xhtml" => Format::Html,
        "eml" => Format::Eml,
        "mbox" | "mbx" => Format::Mbox,
        "pdf" => Format::Pdf,
        _ => return None,
    })
}

/// One document as the index keeps it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Doc {
    pub title: String,
    pub sender: String,
    /// YYYY-MM-DD, or empty.
    pub date: String,
    pub text: String,
}

/// Bytes as text: UTF-8, else Windows-1252 (what old mail and notes are in).
pub fn decode(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

/// `text` cut to `n` characters.
pub fn cut(text: &str, n: usize) -> String {
    tk::head(text, n).to_string()
}

fn squeeze_lines(text: &str) -> String {
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t\u{a0}]+").expect("valid regex"));
    static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n+").expect("valid regex"));
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    BLANKS.replace_all(&SPACES.replace_all(&text, " "), "\n\n").trim().to_string()
}

// -- dates ---------------------------------------------------------------------------------------------------------

static ISO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\D)((?:19|20)\d{2})[-/._]?(0[1-9]|1[0-2])[-/._]?(0[1-9]|[12]\d|3[01])(?:\D|$)")
        .expect("valid regex")
});
static DMY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\D)(0?[1-9]|[12]\d|3[01])[/.\-](0?[1-9]|1[0-2])[/.\-]((?:19|20)\d{2})(?:\D|$)")
        .expect("valid regex")
});
static WORDY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\W)(\d{1,2})(?:er)?\.? ([a-zéèûô]{3,9})\.? ((?:19|20)\d{2})(?:\W|$)").expect("valid regex")
});

const MONTHS: [(&str, u32); 40] = [
    ("jan", 1),
    ("janv", 1),
    ("janvier", 1),
    ("january", 1),
    ("januar", 1),
    ("enero", 1),
    ("feb", 2),
    ("fev", 2),
    ("fevr", 2),
    ("fevrier", 2),
    ("february", 2),
    ("februar", 2),
    ("febrero", 2),
    ("mar", 3),
    ("mars", 3),
    ("march", 3),
    ("marz", 3),
    ("marzo", 3),
    ("apr", 4),
    ("avr", 4),
    ("avril", 4),
    ("april", 4),
    ("abril", 4),
    ("mai", 5),
    ("may", 5),
    ("mayo", 5),
    ("juin", 6),
    ("june", 6),
    ("juni", 6),
    ("junio", 6),
    ("juil", 7),
    ("juillet", 7),
    ("july", 7),
    ("juli", 7),
    ("julio", 7),
    ("aout", 8),
    ("august", 8),
    ("agosto", 8),
    ("aug", 8),
    ("ago", 8),
];
const MONTHS_LATE: [(&str, u32); 17] = [
    ("sep", 9),
    ("sept", 9),
    ("septembre", 9),
    ("september", 9),
    ("septiembre", 9),
    ("oct", 10),
    ("octobre", 10),
    ("october", 10),
    ("oktober", 10),
    ("octubre", 10),
    ("nov", 11),
    ("novembre", 11),
    ("november", 11),
    ("noviembre", 11),
    ("dec", 12),
    ("decembre", 12),
    ("december", 12),
];

fn month_named(word: &str) -> Option<u32> {
    let word = fold(word);
    MONTHS.iter().chain(MONTHS_LATE.iter()).find(|(name, _)| *name == word).map(|(_, m)| *m).or(match word.as_str() {
        "dezember" | "diciembre" | "dic" | "dez" => Some(12),
        _ => None,
    })
}

fn valid(year: i32, month: u32, day: u32) -> Option<String> {
    chrono::NaiveDate::from_ymd_opt(year, month, day).map(|d| d.format("%Y-%m-%d").to_string())
}

/// A date written in any usual way (RFC 2822 as in mail, ISO, 12/03/2024, « 12 mars 2024 ») as YYYY-MM-DD.
pub fn date_of(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(d) = crate::tools_knowledge::email::parsedate_to_datetime(Some(text)) {
        return Some(d.naive.format("%Y-%m-%d").to_string());
    }
    if let Some(c) = ISO.captures(text) {
        if let Some(d) = valid(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?) {
            return Some(d);
        }
    }
    if let Some(c) = DMY.captures(text) {
        if let Some(d) = valid(c[3].parse().ok()?, c[2].parse().ok()?, c[1].parse().ok()?) {
            return Some(d);
        }
    }
    let c = WORDY.captures(text)?;
    valid(c[3].parse().ok()?, month_named(&c[2])?, c[1].parse().ok()?)
}

/// A date in a file's name (« 2024-03-12 facture.md », « 20240312_123.eml »).
pub fn date_in_name(name: &str) -> Option<String> {
    let c = ISO.captures(name)?;
    valid(c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?)
}

// -- formats -------------------------------------------------------------------------------------------------------

/// Lower case, accents taken off, character by character (the same length in characters as the text).
pub fn fold_char(c: char) -> char {
    let base = c.nfd().next().unwrap_or(c);
    base.to_lowercase().next().unwrap_or(base)
}

pub fn fold(text: &str) -> String {
    text.chars().map(fold_char).collect()
}

const TITLE_KEYS: [&str; 6] = ["title", "subject", "sujet", "objet", "titre", "betreff"];
const SENDER_KEYS: [&str; 7] = ["from", "de", "sender", "expediteur", "author", "auteur", "von"];
const DATE_KEYS: [&str; 6] = ["date", "sent", "envoye", "received", "recu", "datum"];

fn key_of(line: &str) -> Option<(String, String)> {
    let plain = line.trim().trim_start_matches(['-', '>', '*', '#', ' ']).replace("**", "").replace("__", "");
    let (key, value) = plain.split_once(':')?;
    let key = fold(key.trim());
    if key.is_empty() || key.chars().count() > 12 || !key.chars().all(|c| c.is_alphabetic() || c == ' ' || c == '-') {
        return None;
    }
    Some((key.trim().to_string(), value.trim().trim_matches('"').trim().to_string()))
}

/// Fills `doc`'s title, sender and date from `key: value` lines (a frontmatter, a note's head), first one wins.
fn take_keys<'a>(doc: &mut Doc, lines: impl Iterator<Item = &'a str>) {
    for line in lines {
        let Some((key, value)) = key_of(line) else { continue };
        if value.is_empty() {
            continue;
        }
        if doc.title.is_empty() && TITLE_KEYS.contains(&key.as_str()) {
            doc.title = cut(&value, 200);
        } else if doc.sender.is_empty() && SENDER_KEYS.contains(&key.as_str()) {
            doc.sender = cut(&value, 200);
        } else if doc.date.is_empty() && DATE_KEYS.contains(&key.as_str()) {
            doc.date = date_of(&value).unwrap_or_default();
        }
    }
}

/// A Markdown or text note: its frontmatter, its head lines, its first heading.
pub fn note(name: &str, text: &str) -> Doc {
    let mut doc = Doc::default();
    let mut body = text.trim_start_matches('\u{feff}');
    if let Some(rest) = body.strip_prefix("---\n").or_else(|| body.strip_prefix("---\r\n")) {
        if let Some(end) = rest.find("\n---") {
            take_keys(&mut doc, rest[..end].lines());
            body = rest[end + 4..].trim_start_matches(['-', '\r', '\n']);
        }
    }
    take_keys(&mut doc, body.lines().take(15));
    if doc.title.is_empty() {
        doc.title = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("# ").map(|t| t.trim().to_string()))
            .map(|t| cut(&t, 200))
            .unwrap_or_default();
    }
    if doc.title.is_empty() {
        doc.title = stem(name);
    }
    if doc.date.is_empty() {
        doc.date = date_in_name(name).unwrap_or_default();
    }
    doc.text = cut(&squeeze_lines(body), MAX_TEXT);
    doc
}

fn stem(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
    stem.replace(['_', '-'], " ").trim().to_string()
}

/// An HTML page: its `<title>` and its words.
pub fn html(name: &str, text: &str) -> Doc {
    static TITLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").expect("valid regex"));
    let mut doc = Doc {
        title: TITLE
            .captures(text)
            .map(|c| cut(crate::tools_knowledge::html::unescape(c[1].trim()).trim(), 200))
            .unwrap_or_default(),
        ..Doc::default()
    };
    let body = squeeze_lines(&crate::mailbox::html_text(text));
    take_keys(&mut doc, body.lines().take(15));
    if doc.title.is_empty() {
        doc.title = stem(name);
    }
    if doc.date.is_empty() {
        doc.date = date_in_name(name).unwrap_or_default();
    }
    doc.text = cut(&body, MAX_TEXT);
    doc
}

/// An e-mail (`.eml`, a message of an mbox, a message fetched from a mailbox): its subject, sender, date and text
/// (plain part, else its HTML stripped); the attachments' names, not their content.
pub fn mail(raw: &[u8]) -> Doc {
    let (message, text, files) = crate::mailbox::message_text(raw);
    let decoded = |name: &str| crate::mailbox::decode(message.get(name).as_deref());
    let mut text = cut(&text, MAX_TEXT);
    if !files.is_empty() {
        text.push_str(&format!("\n\n[Attachments: {}]", files.join(", ")));
    }
    Doc {
        title: cut(&decoded("Subject"), 200),
        sender: cut(&decoded("From"), 200),
        date: message.get("Date").and_then(|d| date_of(&d)).unwrap_or_default(),
        text,
    }
}

/// The messages of an mbox: (byte offset of its `From ` line, the message without it), read as a stream.
pub fn mbox_messages(reader: impl Read, mut each: impl FnMut(u64, &[u8]) -> bool) -> std::io::Result<()> {
    let mut reader = std::io::BufReader::new(reader);
    let mut line = Vec::new();
    let (mut offset, mut start): (u64, Option<u64>) = (0, None);
    let mut message: Vec<u8> = Vec::new();
    let mut previous_blank = true;
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if previous_blank && line.starts_with(b"From ") {
            if let Some(at) = start {
                if !each(at, &message) {
                    return Ok(());
                }
            }
            start = Some(offset);
            message.clear();
        } else if start.is_some() && message.len() < MAX_MESSAGE {
            // `>From ` lines are a quoted `From ` (mboxrd)
            let body: &[u8] = if line.starts_with(b">From ") { &line[1..] } else { &line };
            message.extend_from_slice(body);
        }
        previous_blank = line.iter().all(u8::is_ascii_whitespace);
        offset += n as u64;
    }
    if let Some(at) = start {
        each(at, &message);
    }
    Ok(())
}

/// The message of an mbox that starts at `offset`.
pub fn mbox_message_at(mut file: std::fs::File, offset: u64) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Seek;
    file.seek(std::io::SeekFrom::Start(offset))?;
    let mut found = None;
    mbox_messages(file, |at, message| {
        if at == 0 {
            found = Some(message.to_vec());
        }
        false
    })?;
    Ok(found)
}

/// `pdftotext` when installed (poppler-utils).
pub fn pdftotext() -> Option<String> {
    static FOUND: LazyLock<Option<String>> = LazyLock::new(|| crate::browsers::pyx::which("pdftotext"));
    FOUND.clone()
}

/// A PDF's text (its first 50 pages), when `pdftotext` is there and answers in time. The file already open (never
/// through a link: `agent_files::open_beneath`) is given as its input (`/dev/stdin`), never its path, which a folder
/// swapped for a link meanwhile would lead elsewhere (security audit 2026-10-27).
pub fn pdf_text(file: std::fs::File) -> Option<String> {
    let program = pdftotext()?;
    let mut child = std::process::Command::new(program)
        .args(["-q", "-l", "50", "-enc", "UTF-8", "/dev/stdin", "-"])
        .stdin(Stdio::from(file))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        let _ = (&mut out).take((MAX_TEXT * 4) as u64).read_to_end(&mut text);
        text
    });
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < PDF_TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text = reader.join().ok()?;
    Some(squeeze_lines(&String::from_utf8_lossy(&text)))
}

/// A PDF: its text if it can be had, its name in any case.
pub fn pdf(name: &str, text: Option<String>) -> Doc {
    let text = text.unwrap_or_default();
    let mut doc = Doc { title: stem(name), date: date_in_name(name).unwrap_or_default(), ..Doc::default() };
    take_keys(&mut doc, text.lines().take(10));
    doc.title = if doc.title.is_empty() { stem(name) } else { doc.title };
    doc.text = cut(&text, MAX_TEXT);
    doc
}

// -- what a document is about, for the yearly overview -------------------------------------------------------------

const SUBSCRIPTION: [&str; 10] = [
    "abonnement",
    "subscription",
    "renouvellement",
    "renewal",
    "abo",
    "suscripcion",
    "membership",
    "adhesion",
    "mitgliedschaft",
    "cotisation",
];
const TRAVEL: [&str; 22] = [
    "reservation",
    "billet",
    "vol",
    "hotel",
    "voyage",
    "train",
    "booking",
    "flight",
    "trip",
    "itinerary",
    "itineraire",
    "embarquement",
    "boarding",
    "reise",
    "flug",
    "buchung",
    "viaje",
    "vuelo",
    "reserva",
    "sejour",
    "airbnb",
    "location",
];
const PURCHASE: [&str; 18] = [
    "commande",
    "facture",
    "recu",
    "achat",
    "paiement",
    "order",
    "receipt",
    "invoice",
    "purchase",
    "payment",
    "bestellung",
    "rechnung",
    "kauf",
    "pedido",
    "factura",
    "compra",
    "livraison",
    "delivery",
];

/// « purchase », « travel », « subscription » or « other », from the words of its title, sender and first lines.
pub fn category(doc: &Doc) -> &'static str {
    let head = format!("{} {} {}", doc.title, doc.sender, tk::head(&doc.text, 400));
    let words: Vec<String> = fold(&head)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 1)
        .map(|w| if w.len() > 4 { w.trim_end_matches('s').to_string() } else { w.to_string() })
        .collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    if has(&SUBSCRIPTION) {
        "subscription"
    } else if has(&TRAVEL) {
        "travel"
    } else if has(&PURCHASE) {
        "purchase"
    } else {
        "other"
    }
}

// -- snippets ------------------------------------------------------------------------------------------------------

/// A short piece of `text` around the first of `terms` it has (folded: « eleve » finds « Élève »), the words found
/// between « », else its start.
pub fn snippet(text: &str, terms: &[String], width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let folded: Vec<char> = chars.iter().map(|c| fold_char(*c)).collect();
    let mut best: Option<(usize, usize)> = None;
    for term in terms {
        let needle: Vec<char> = fold(term).chars().collect();
        if needle.is_empty() || needle.len() > folded.len() {
            continue;
        }
        if let Some(at) = (0..=folded.len() - needle.len()).find(|i| folded[*i..*i + needle.len()] == needle[..]) {
            if best.is_none_or(|(b, _)| at < b) {
                best = Some((at, needle.len()));
            }
        }
    }
    let flat = |s: &[char]| s.iter().map(|c| if c.is_whitespace() { ' ' } else { *c }).collect::<String>();
    let Some((at, len)) = best else {
        return tk::squeeze(&flat(&chars[..chars.len().min(width)]));
    };
    // the whole word found
    let mut end = at + len;
    while end < chars.len() && chars[end].is_alphanumeric() {
        end += 1;
    }
    let mut start = at;
    while start > 0 && chars[start - 1].is_alphanumeric() {
        start -= 1;
    }
    // whole words at both ends
    let mut from = start.saturating_sub(width / 2);
    while from > 0 && chars[from - 1].is_alphanumeric() {
        from -= 1;
    }
    let mut to = (end + width / 2).min(chars.len());
    while to < chars.len() && chars[to].is_alphanumeric() {
        to += 1;
    }
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.push_str(&flat(&chars[from..start]));
    out.push('«');
    out.push_str(&flat(&chars[start..end]));
    out.push('»');
    out.push_str(&flat(&chars[end..to]));
    if to < chars.len() {
        out.push('…');
    }
    tk::squeeze(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_as_people_write_them() {
        assert_eq!(date_of("Tue, 12 Mar 2024 10:00:00 +0100").as_deref(), Some("2024-03-12"));
        assert_eq!(date_of("2024-03-12T10:00").as_deref(), Some("2024-03-12"));
        assert_eq!(date_of("12/03/2024").as_deref(), Some("2024-03-12"));
        assert_eq!(date_of("le 3 février 2023 à 10h").as_deref(), Some("2023-02-03"));
        assert_eq!(date_of("demain"), None);
        assert_eq!(date_in_name("20240312_facture.md").as_deref(), Some("2024-03-12"));
        assert_eq!(date_in_name("notes.md"), None);
    }

    #[test]
    fn a_mail_note_gives_its_head() {
        let doc = note(
            "2024/x.md",
            "---\ntitle: Votre commande Vélo\ndate: 2024-05-02\n---\n**De :** Boutique <b@shop.example>\n\nMerci pour l'achat.",
        );
        assert_eq!(doc.title, "Votre commande Vélo");
        assert_eq!(doc.sender, "Boutique <b@shop.example>");
        assert_eq!(doc.date, "2024-05-02");
        assert!(doc.text.contains("Merci pour l'achat."));
        assert_eq!(category(&doc), "purchase");
        let plain = note("2023-07-01 billet.txt", "# Billet de train Paris Lyon\nVoiture 12");
        assert_eq!((plain.title.as_str(), plain.date.as_str()), ("Billet de train Paris Lyon", "2023-07-01"));
        assert_eq!(category(&plain), "travel");
    }

    #[test]
    fn an_mbox_is_cut_into_its_messages() {
        let data = b"From a@b Mon Jan  1 00:00:00 2024\nSubject: Un\n\nPremier\n>From me\n\nFrom c@d Tue Jan  2 00:00:00 2024\nSubject: Deux\n\nSecond\n";
        let mut seen = Vec::new();
        mbox_messages(&data[..], |at, m| {
            seen.push((at, String::from_utf8_lossy(m).into_owned()));
            true
        })
        .unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, 0);
        assert!(seen[0].1.contains("From me") && !seen[0].1.contains(">From"));
        assert!(seen[1].1.starts_with("Subject: Deux"));
        let doc = mail(seen[1].1.as_bytes());
        assert_eq!(doc.title, "Deux");
    }

    #[test]
    fn snippets_show_the_word_found() {
        let text = "Bonjour,\nvotre Élève a reçu son diplôme hier. Bravo !";
        assert_eq!(snippet(text, &["eleve".into()], 20), "Bonjour, votre «Élève» a reçu son…");
        assert_eq!(snippet("abc def", &["zzz".into()], 3), "abc");
    }
}
