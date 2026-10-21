//! Python's `html.parser.HTMLParser` (3.12, `convert_charrefs=True`, one `feed` and no `close`, as mailbox.py's
//! `html_text` uses it) and `html.unescape`: the same tags, the same data, the same text held back at the end.
//!
//! Not a Python module: a port of the standard library parts the group needs.

use std::sync::LazyLock;

use regex::Regex;

use super::entities::ENTITIES;

/// What the parser reports (Python's `handle_starttag`, `handle_endtag`, `handle_data`).
pub trait Handler {
    fn starttag(&mut self, tag: &str);
    fn endtag(&mut self, tag: &str);
    fn data(&mut self, data: &str);
}

const CDATA_CONTENT_ELEMENTS: [&str; 6] = ["script", "style", "xmp", "iframe", "noembed", "noframes"];
const RCDATA_CONTENT_ELEMENTS: [&str; 2] = ["textarea", "title"];

fn is_tag_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{0c}' | ' ')
}

fn char_at(s: &str, i: usize) -> Option<char> {
    s.get(i..).and_then(|r| r.chars().next())
}

fn char_before(s: &str, i: usize) -> Option<char> {
    s.get(..i).and_then(|r| r.chars().next_back())
}

/// One `feed(data)` of a fresh `HTMLParser` (no `close()`): what it reports.
pub fn feed(rawdata: &str, handler: &mut dyn Handler) {
    let mut parser = Parser { cdata_elem: None, escapable: true };
    parser.goahead(rawdata, false, handler);
}

struct Parser {
    cdata_elem: Option<String>,
    escapable: bool,
}

impl Parser {
    fn set_cdata_mode(&mut self, elem: &str, escapable: bool) {
        self.cdata_elem = Some(elem.to_lowercase());
        self.escapable = escapable;
    }

    fn clear_cdata_mode(&mut self) {
        self.cdata_elem = None;
        self.escapable = true;
    }

    /// `self.interesting.search(rawdata, i)` in CDATA mode: `</<elem>(?=[\t\n\r\f />])`, ignoring case (or the
    /// end of the text for `plaintext`).
    fn interesting(&self, rawdata: &str, i: usize) -> Option<usize> {
        let elem = self.cdata_elem.as_deref()?;
        if elem == "plaintext" {
            return Some(rawdata.len());
        }
        let bytes = rawdata.as_bytes();
        let mut from = i;
        while let Some(found) = rawdata[from..].find("</") {
            let start = from + found;
            let name_start = start + 2;
            let name_end = name_start + elem.len();
            if name_end < bytes.len()
                && bytes[name_start..name_end].eq_ignore_ascii_case(elem.as_bytes())
                && matches!(bytes[name_end], b'\t' | b'\n' | b'\r' | 0x0c | b' ' | b'/' | b'>')
            {
                return Some(start);
            }
            from = start + 2;
        }
        None
    }

    fn goahead(&mut self, rawdata: &str, end: bool, handler: &mut dyn Handler) -> usize {
        let n = rawdata.len();
        let mut i = 0;
        while i < n {
            let j;
            if self.cdata_elem.is_none() {
                match rawdata[i..].find('<') {
                    Some(found) => j = i + found,
                    None => {
                        // A charref maybe cut in half at the end: Python waits for more text.
                        let floor = last_chars_start(rawdata, 34).max(i);
                        if let Some(amppos) = rawdata[floor..].rfind('&').map(|p| p + floor) {
                            if !rawdata[amppos..].contains(['\t', '\n', '\r', '\u{0c}', ' ', ';']) {
                                break;
                            }
                        }
                        j = n;
                    }
                }
            } else {
                match self.interesting(rawdata, i) {
                    Some(found) => j = found,
                    None => break,
                }
            }
            if i < j {
                if self.escapable {
                    handler.data(&unescape(&rawdata[i..j]));
                } else {
                    handler.data(&rawdata[i..j]);
                }
            }
            i = j;
            if i == n {
                break;
            }
            if rawdata[i..].starts_with('<') {
                let k: isize = if starttagopen(rawdata, i) {
                    self.parse_starttag(rawdata, i, handler)
                } else if rawdata[i..].starts_with("</") {
                    self.parse_endtag(rawdata, i, handler)
                } else if rawdata[i..].starts_with("<!--") {
                    parse_comment(rawdata, i)
                } else if rawdata[i..].starts_with("<?") {
                    rawdata[i + 2..].find('>').map(|p| (i + 2 + p + 1) as isize).unwrap_or(-1)
                } else if rawdata[i..].starts_with("<!") {
                    parse_html_declaration(rawdata, i)
                } else if i + 1 < n || end {
                    handler.data("<");
                    (i + 1) as isize
                } else {
                    break;
                };
                if k < 0 {
                    // Not terminated: Python waits for more text (it never comes here: no close()).
                    break;
                }
                i = k as usize;
            } else {
                // `&` branches: unreachable with convert_charrefs (the text up to `<` was taken above).
                break;
            }
        }
        i
    }

    fn parse_starttag(&mut self, rawdata: &str, i: usize, handler: &mut dyn Handler) -> isize {
        let endpos = locatetagend(rawdata, i + 1);
        if rawdata.as_bytes()[endpos - 1] != b'>' {
            return -1;
        }
        let (name_end, mut k) = tagfind_tolerant(rawdata, i + 1);
        let tag = rawdata[i + 1..name_end].to_lowercase();
        while k < endpos {
            match attrfind_tolerant(rawdata, k) {
                Some(next) => k = next,
                None => break,
            }
        }
        let rest = super::strip(&rawdata[k.min(endpos)..endpos]);
        if rest != ">" && rest != "/>" {
            handler.data(&rawdata[i..endpos]);
            return endpos as isize;
        }
        if rest.ends_with("/>") {
            handler.starttag(&tag);
            handler.endtag(&tag);
        } else {
            handler.starttag(&tag);
            if CDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) || tag == "plaintext" {
                self.set_cdata_mode(&tag, false);
            } else if RCDATA_CONTENT_ELEMENTS.contains(&tag.as_str()) {
                self.set_cdata_mode(&tag, true);
            }
        }
        endpos as isize
    }

    fn parse_endtag(&mut self, rawdata: &str, i: usize, handler: &mut dyn Handler) -> isize {
        if !rawdata[i + 2..].contains('>') {
            return -1;
        }
        if !endtagopen(rawdata, i) {
            if rawdata[i + 2..].starts_with('>') {
                return (i + 3) as isize; // </> is ignored
            }
            return parse_bogus_comment(rawdata, i);
        }
        let j = locatetagend(rawdata, i + 2);
        if rawdata.as_bytes()[j - 1] != b'>' {
            return -1;
        }
        let (name_end, _) = tagfind_tolerant(rawdata, i + 2);
        let tag = rawdata[i + 2..name_end].to_lowercase();
        handler.endtag(&tag);
        self.clear_cdata_mode();
        j as isize
    }
}

/// The byte offset where the last `count` characters start (`n - count` in characters, at least 0).
fn last_chars_start(s: &str, count: usize) -> usize {
    s.char_indices().rev().nth(count - 1).map(|(i, _)| i).unwrap_or(0)
}

/// `<[a-zA-Z]` at `i`.
fn starttagopen(s: &str, i: usize) -> bool {
    let b = s.as_bytes();
    b.get(i) == Some(&b'<') && b.get(i + 1).map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
}

/// `</[a-zA-Z]` at `i`.
fn endtagopen(s: &str, i: usize) -> bool {
    let b = s.as_bytes();
    s[i..].starts_with("</") && b.get(i + 2).map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
}

fn parse_comment(s: &str, i: usize) -> isize {
    // commentclose `--!?>` searched, else commentabruptclose `-?>` matched at i+4
    let from = i + 4;
    let rest = &s[from..];
    let mut search = 0;
    while let Some(p) = rest[search..].find("--") {
        let at = search + p;
        let after = &rest[at + 2..];
        if after.starts_with('>') {
            return (from + at + 3) as isize;
        }
        if after.starts_with("!>") {
            return (from + at + 4) as isize;
        }
        search = at + 1;
    }
    if rest.starts_with("->") {
        return (from + 2) as isize;
    }
    if rest.starts_with('>') {
        return (from + 1) as isize;
    }
    -1
}

fn parse_bogus_comment(s: &str, i: usize) -> isize {
    s[i + 2..].find('>').map(|p| (i + 2 + p + 1) as isize).unwrap_or(-1)
}

fn parse_html_declaration(s: &str, i: usize) -> isize {
    let rest = &s[i..];
    if rest.starts_with("<!--") {
        return parse_comment(s, i);
    }
    if rest.starts_with("<![CDATA[") {
        return s[i + 9..].find("]]>").map(|p| (i + 9 + p + 3) as isize).unwrap_or(-1);
    }
    if super::head(rest, 9).to_lowercase() == "<!doctype" {
        let from = i + super::head(rest, 9).len();
        return s[from..].find('>').map(|p| (from + p + 1) as isize).unwrap_or(-1);
    }
    if rest.starts_with("<![") {
        return s[i + 3..].find('>').map(|p| (i + 3 + p + 1) as isize).unwrap_or(-1);
    }
    parse_bogus_comment(s, i)
}

/// `tagfind_tolerant.match(s, i)`: `([a-zA-Z][^\t\n\r\f />]*)(?:[\t\n\r\f ]|/(?!>))*` → (end of the name, end).
fn tagfind_tolerant(s: &str, i: usize) -> (usize, usize) {
    let mut p = i;
    let mut first = true;
    while let Some(c) = char_at(s, p) {
        if first {
            first = false;
        } else if is_tag_space(c) || c == '/' || c == '>' {
            break;
        }
        p += c.len_utf8();
    }
    let name_end = p;
    while let Some(c) = char_at(s, p) {
        if is_tag_space(c) || (c == '/' && char_at(s, p + 1) != Some('>')) {
            p += c.len_utf8();
        } else {
            break;
        }
    }
    (name_end, p)
}

/// An attribute's name and optional value at `p` (the part attrfind_tolerant and locatetagend share): its end,
/// or None when no attribute starts there (`(?<=['"\t\n\r\f /])[^\t\n\r\f />]…`).
fn attribute(s: &str, p: usize) -> Option<usize> {
    let before = char_before(s, p)?;
    if !(before == '\'' || before == '"' || is_tag_space(before) || before == '/') {
        return None;
    }
    let first = char_at(s, p)?;
    if is_tag_space(first) || first == '/' || first == '>' {
        return None;
    }
    let mut q = p + first.len_utf8();
    while let Some(c) = char_at(s, q) {
        if is_tag_space(c) || c == '/' || c == '=' || c == '>' {
            break;
        }
        q += c.len_utf8();
    }
    // ([\t\n\r\f ]*=[\t\n\r\f ]*('…'|"…"|(?!['"])[^>\t\n\r\f ]*))?
    let mut r = q;
    while let Some(c) = char_at(s, r).filter(|c| is_tag_space(*c)) {
        r += c.len_utf8();
    }
    if char_at(s, r) == Some('=') {
        let after_eq = r + 1;
        let mut v = after_eq;
        while let Some(c) = char_at(s, v).filter(|c| is_tag_space(*c)) {
            v += c.len_utf8();
        }
        match value_at(s, v) {
            Some(end) => q = end,
            // backtracking: one space less after `=`, and an empty bare value before it
            None if v > after_eq => q = v - 1,
            None => {}
        }
    }
    Some(q)
}

fn value_at(s: &str, v: usize) -> Option<usize> {
    match char_at(s, v) {
        Some(quote @ ('\'' | '"')) => s[v + 1..].find(quote).map(|p| v + 1 + p + 1),
        _ => {
            let mut e = v;
            while let Some(c) = char_at(s, e) {
                if c == '>' || is_tag_space(c) {
                    break;
                }
                e += c.len_utf8();
            }
            Some(e)
        }
    }
}

/// `attrfind_tolerant.match(s, k)`: an attribute then `(?:[\t\n\r\f ]|/(?!>))*`; its end.
fn attrfind_tolerant(s: &str, k: usize) -> Option<usize> {
    let mut p = attribute(s, k)?;
    while let Some(c) = char_at(s, p) {
        if is_tag_space(c) || (c == '/' && char_at(s, p + 1) != Some('>')) {
            p += c.len_utf8();
        } else {
            break;
        }
    }
    Some(p)
}

/// `locatetagend.match(s, i).end()`: the tag name, attributes, then an optional `>`.
fn locatetagend(s: &str, i: usize) -> usize {
    let (name_end, _) = tagfind_tolerant(s, i);
    let mut p = name_end;
    let skip = |p: &mut usize| {
        while let Some(c) = char_at(s, *p) {
            if is_tag_space(c) || c == '/' {
                *p += c.len_utf8();
            } else {
                break;
            }
        }
    };
    skip(&mut p);
    while let Some(end) = attribute(s, p) {
        p = end;
        skip(&mut p);
    }
    if char_at(s, p) == Some('>') {
        p += 1;
    }
    p
}

// -- html.unescape ---------------------------------------------------------------------------------

static CHARREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(#[0-9]+;?|#[xX][0-9a-fA-F]+;?|[^\t\n\x0c <&#;]{1,32};?)").expect("valid regex"));

fn entity(name: &str) -> Option<&'static str> {
    ENTITIES.binary_search_by(|(k, _)| (*k).cmp(name)).ok().map(|i| ENTITIES[i].1)
}

fn invalid_charref(num: u32) -> Option<&'static str> {
    Some(match num {
        0x00 => "\u{fffd}",
        0x0d => "\r",
        0x80 => "\u{20ac}",
        0x81 => "\u{81}",
        0x82 => "\u{201a}",
        0x83 => "\u{0192}",
        0x84 => "\u{201e}",
        0x85 => "\u{2026}",
        0x86 => "\u{2020}",
        0x87 => "\u{2021}",
        0x88 => "\u{02c6}",
        0x89 => "\u{2030}",
        0x8a => "\u{0160}",
        0x8b => "\u{2039}",
        0x8c => "\u{0152}",
        0x8d => "\u{8d}",
        0x8e => "\u{017d}",
        0x8f => "\u{8f}",
        0x90 => "\u{90}",
        0x91 => "\u{2018}",
        0x92 => "\u{2019}",
        0x93 => "\u{201c}",
        0x94 => "\u{201d}",
        0x95 => "\u{2022}",
        0x96 => "\u{2013}",
        0x97 => "\u{2014}",
        0x98 => "\u{02dc}",
        0x99 => "\u{2122}",
        0x9a => "\u{0161}",
        0x9b => "\u{203a}",
        0x9c => "\u{0153}",
        0x9d => "\u{9d}",
        0x9e => "\u{017e}",
        0x9f => "\u{0178}",
        _ => return None,
    })
}

fn invalid_codepoint(num: u32) -> bool {
    matches!(num, 0x1..=0x8 | 0xe..=0x1f | 0x7f..=0x9f | 0xfdd0..=0xfdef | 0xb)
        || (num & 0xfffe == 0xfffe && num <= 0x10ffff)
}

fn replace_charref(s: &str) -> String {
    if let Some(number) = s.strip_prefix('#') {
        let (digits, radix) = match number.strip_prefix(['x', 'X']) {
            Some(hex) => (hex.trim_end_matches(';'), 16),
            None => (number.trim_end_matches(';'), 10),
        };
        let num = u64::from_str_radix(digits.trim_start_matches('0'), radix)
            .unwrap_or(if digits.trim_start_matches('0').is_empty() { 0 } else { u64::MAX });
        if num <= u32::MAX as u64 {
            if let Some(text) = invalid_charref(num as u32) {
                return text.to_string();
            }
        }
        if (0xD800..=0xDFFF).contains(&num) || num > 0x10FFFF {
            return "\u{fffd}".into();
        }
        if invalid_codepoint(num as u32) {
            return String::new();
        }
        return char::from_u32(num as u32).map(String::from).unwrap_or_default();
    }
    if let Some(text) = entity(s) {
        return text.to_string();
    }
    // the longest matching name (as defined by the standard)
    let bounds: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    for x in (2..bounds.len()).rev() {
        if let Some(text) = entity(&s[..bounds[x]]) {
            return format!("{text}{}", &s[bounds[x]..]);
        }
    }
    format!("&{s}")
}

/// `html.unescape(s)`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    CHARREF.replace_all(s, |caps: &regex::Captures| replace_charref(&caps[1])).into_owned()
}

/// `html.escape(s)` (quotes too).
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Log(Vec<String>);

    impl Handler for Log {
        fn starttag(&mut self, tag: &str) {
            self.0.push(format!("<{tag}>"));
        }
        fn endtag(&mut self, tag: &str) {
            self.0.push(format!("</{tag}>"));
        }
        fn data(&mut self, data: &str) {
            self.0.push(data.to_string());
        }
    }

    fn events(html: &str) -> Vec<String> {
        let mut log = Log(Vec::new());
        feed(html, &mut log);
        log.0
    }

    #[test]
    fn tags_data_and_held_back_text() {
        assert_eq!(events("<p class='x'>Caf&eacute; &amp; th&eacute;</p>"), ["<p>", "Café & thé", "</p>"]);
        assert_eq!(events("<script>if (a<b) x()</script>ok"), ["<script>", "if (a<b) x()", "</script>", "ok"]);
        assert_eq!(events("<br/>AT&T"), ["<br>", "</br>"]); // `&T` at the end: Python waits for more
        assert_eq!(events("a <b"), ["a "]);
        assert_eq!(events("<!-- c --><!DOCTYPE html>x"), ["x"]);
    }

    #[test]
    fn unescape_as_python() {
        assert_eq!(unescape("&lt;&#62;&#x41;&eacute&notit;&#128;&#0;&#xD800;"), "<>Aé¬it;€\u{fffd}\u{fffd}");
        assert_eq!(unescape("&unknown; &"), "&unknown; &");
    }
}
