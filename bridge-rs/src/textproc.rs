//! Text normalization for speech and incremental sentence splitting.
//!
//! The splitter works on the *raw* streamed markdown (deltas from Hermes) and only cuts where the
//! boundary is certain: end punctuation followed by whitespace (so `3.5` is never cut, since the
//! digit after the dot is seen before deciding), a line break, or a soft cut near `max_chars` at a
//! word boundary. Each emitted segment is then normalized (markdown removed, links reduced to their
//! label, list items ended with a period so they are read naturally).
//!
//! Port of `bridge/bipbridge/textproc.py` (owner group: voice-speech). Its regular expressions run on
//! `voice_speech::pyre` (Python's `re` semantics and character classes).

use std::sync::LazyLock;

use unicode_normalization::UnicodeNormalization;

use crate::voice_speech::pyre::{Regex, DOTALL};
use crate::voice_speech::pystr;

pub const END_PUNCT: &str = ".!?…";
/// Characters that may close a sentence after its punctuation: quotes, brackets, emphasis markers.
pub const CLOSERS: &str = "\"'»”’)]*_";
pub const SPACES: &str = " \u{a0}\u{202f}";
/// Words after which a period does not end a sentence (compared lowercase, without the dot).
pub const ABBREVIATIONS: &[&str] = &[
    "m", "mm", "mme", "mmes", "mlle", "mlles", "dr", "pr", "st", "ste", "cf", "ex", "p", "pp", "vs", "env", "mr",
    "mrs", "ms", "jr", "sr", "fig", "chap", "av", "bd", "n°", "etc", "e.g", "i.e", "approx", "tél", "réf", "al",
];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern)
}

// Python defines `_FENCE` twice: the second definition (below `split_sentences`) is the one every function sees
// at call time; the first (`^\s{0,3}(```|~~~)`) is never used. Kept as Python runs it: a fence line starts with ```.
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::with_flags(r"```.*?(?:```|$)", DOTALL));
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| re(r"</?[A-Za-z][^>\n]*>"));
static IMAGE: LazyLock<Regex> = LazyLock::new(|| re(r"!\[([^\]]*)\]\([^)]*\)"));
static LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]]+)\]\([^)]*\)"));
static REF_LINK: LazyLock<Regex> = LazyLock::new(|| re(r"\[([^\]]+)\]\[[^\]]*\]"));
static AUTOLINK: LazyLock<Regex> = LazyLock::new(|| re(r"<(?:https?|mailto):[^>\s]+>"));
static BARE_URL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(?:https?://|www\.)[^\s)\]>]+"));
static INLINE_CODE: LazyLock<Regex> = LazyLock::new(|| re(r"`+([^`]*)`+"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{0,3}#{1,6}\s*"));
static QUOTE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s{0,3}(>\s?)+"));
static BULLET: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*[-*+•▪◦]\s+"));
static NUMBERED: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\d{1,3}[.)]\s+"));
static HRULE: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*([-*_=]\s*){3,}$"));
static TABLE_SEP: LazyLock<Regex> = LazyLock::new(|| re(r"^\s*\|?[\s:|-]+\|[\s:|-]*$"));
static BOLD: LazyLock<Regex> = LazyLock::new(|| re(r"(\*\*|__)(.+?)\1"));
static ITALIC_STAR: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])"));
static ITALIC_UNDERSCORE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w_])_(?!\s)(.+?)(?<!\s)_(?![\w_])"));
static STRIKE: LazyLock<Regex> = LazyLock::new(|| re(r"~~(.+?)~~"));
static STRAY_MARKS: LazyLock<Regex> = LazyLock::new(|| re(r"[*`#]+|(?<!\w)_+|_+(?!\w)"));
static SPACES_RE: LazyLock<Regex> = LazyLock::new(|| re(r"\s+"));
static SPACE_BEFORE_COMMA: LazyLock<Regex> = LazyLock::new(|| re(r"\s+([,.])"));
static ALNUM: LazyLock<Regex> = LazyLock::new(|| re(r"\w"));
static BLOCKS: LazyLock<Regex> = LazyLock::new(|| re(r"\n\s*\n|\n(?=[ \t]*(?:[-*•+]|\d{1,2}[.)])[ \t])"));

/// Emoji and pictographs: a voice reads them out (« visage souriant ») or stumbles on them (Pocket, Alex). The same
/// ranges as VoiceKit's SpeechNormalizer and Android's Emoji.
const EMOJI_PATTERN: &str =
    "[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{231A}\u{231B}\u{2328}\u{23CF}\u{23E9}-\u{23F3}\u{23F8}-\u{23FA}\
\u{2B05}-\u{2B07}\u{2B1B}\u{2B1C}\u{2B50}\u{2B55}\u{25AA}\u{25AB}\u{25B6}\u{25C0}\u{25FB}-\u{25FE}\u{2934}\u{2935}\
\u{3030}\u{303D}\u{3297}\u{3299}\u{FE0E}\u{FE0F}\u{200D}\u{20E3}\u{E0020}-\u{E007F}]+";
static EMOJI: LazyLock<Regex> = LazyLock::new(|| re(EMOJI_PATTERN));
static EMOJI_BEFORE_STOP: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"[ \t]*{EMOJI_PATTERN}(?=[.,])")));
static TABS_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"[ \t]{2,}"));

/// Turn one markdown segment (a sentence or a line) into plain speakable text.
pub fn normalize_segment(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let text: String = text.nfc().collect();
    let mut lines = Vec::new();
    for line in pystr::splitlines(&text) {
        if HRULE.matches_at_start(&line) || TABLE_SEP.matches_at_start(&line) || FENCE.matches_at_start(&line) {
            continue;
        }
        let mut line = HEADING.replace(&line, "");
        line = QUOTE.replace(&line, "");
        line = BULLET.replace(&line, "");
        line = NUMBERED.replace(&line, "");
        if line.matches('|').count() >= 2 {
            // table row -> comma separated cells
            let cells: Vec<&str> = pystr::strip_chars(pystr::strip(&line), "|").split('|').map(pystr::strip).collect();
            line = cells.into_iter().filter(|c| !c.is_empty()).collect::<Vec<_>>().join(", ");
        }
        lines.push(line);
    }
    let mut text = lines.join(" ");
    text = HTML_TAG.replace(&text, "");
    text = IMAGE.replace(&text, r"\1");
    text = LINK.replace(&text, r"\1");
    text = REF_LINK.replace(&text, r"\1");
    text = AUTOLINK.replace(&text, "");
    text = BARE_URL.replace(&text, "");
    text = INLINE_CODE.replace(&text, r"\1");
    for _ in 0..2 {
        // nested emphasis such as ***x***
        text = BOLD.replace(&text, r"\2");
        text = ITALIC_STAR.replace(&text, r"\1");
        text = ITALIC_UNDERSCORE.replace(&text, r"\1");
    }
    text = STRIKE.replace(&text, r"\1");
    text = STRAY_MARKS.replace(&text, "");
    text = pystr::strip(&SPACES_RE.replace(&text, " ")).to_string();
    text = SPACE_BEFORE_COMMA.replace(&text, r"\1");
    let mut text = pystr::strip_chars(&text, " ,;").to_string();
    if !ALNUM.is_match(&text) {
        return String::new();
    }
    let last = text.chars().last().unwrap_or(' ');
    if !".!?…:;,\"'»”)".contains(last) {
        text.push('.');
    }
    text
}

/// `buf.rfind(c, 0, end)` for one character.
fn rfind_char(buf: &[char], c: char, end: usize) -> Option<usize> {
    buf[..end.min(buf.len())].iter().rposition(|&x| x == c)
}

/// `buf.find(sub, start)` on characters.
fn find_from(buf: &[char], sub: &str, start: usize) -> Option<usize> {
    let sub: Vec<char> = sub.chars().collect();
    if start > buf.len() || sub.len() > buf.len() {
        return None;
    }
    (start..=buf.len() - sub.len()).find(|&i| buf[i..i + sub.len()] == sub[..])
}

/// `buf.rfind(sub)` on characters.
fn rfind(buf: &[char], sub: &str) -> Option<usize> {
    let sub: Vec<char> = sub.chars().collect();
    if sub.len() > buf.len() {
        return None;
    }
    (0..=buf.len() - sub.len()).rev().find(|&i| buf[i..i + sub.len()] == sub[..])
}

/// `1. ` at the start of a line is a numbered list marker, not a sentence end.
fn is_list_marker(buf: &[char], dot_index: usize, at_line_start: bool) -> bool {
    let line_start = rfind_char(buf, '\n', dot_index).map(|i| i + 1).unwrap_or(0);
    if line_start == 0 && !at_line_start {
        return false;
    }
    let prefix: String = buf[line_start..dot_index].iter().collect();
    let prefix = pystr::strip(&prefix);
    pystr::all_digits(prefix) && pystr::len(prefix) <= 3
}

fn is_abbreviation(buf: &[char], dot_index: usize) -> bool {
    let mut start = dot_index;
    while start > 0 && (pystr::is_alnum(buf[start - 1]) || "°.".contains(buf[start - 1])) {
        start -= 1;
    }
    let word = pystr::lower(&buf[start..dot_index].iter().collect::<String>());
    if word.is_empty() {
        return false;
    }
    if pystr::len(&word) == 1 && pystr::is_upper(buf[dot_index - 1]) {
        // initials: "J. Dupont"
        return true;
    }
    ABBREVIATIONS.contains(&word.as_str())
}

/// Incremental splitter: `feed(delta)` returns the sentences completed so far, `flush()`
/// returns the rest. Returned sentences are already normalized for speech.
pub struct SentenceSplitter {
    pub max_chars: usize,
    buf: Vec<char>,
    in_fence: bool,
    at_line_start: bool,
}

impl SentenceSplitter {
    pub fn new(max_chars: usize) -> Self {
        SentenceSplitter { max_chars: max_chars.max(40), buf: Vec::new(), in_fence: false, at_line_start: true }
    }

    pub fn feed(&mut self, delta: &str) -> Vec<String> {
        if delta.is_empty() {
            return Vec::new();
        }
        self.buf.extend(delta.replace("\r\n", "\n").replace('\r', "\n").chars());
        let mut out = Vec::new();
        while let Some(seg) = self.next_segment() {
            let norm = normalize_segment(&seg);
            if !norm.is_empty() {
                out.push(norm);
            }
        }
        out
    }

    pub fn flush(&mut self) -> Vec<String> {
        let rest: String = std::mem::take(&mut self.buf).into_iter().collect();
        let in_fence = std::mem::replace(&mut self.in_fence, false);
        self.at_line_start = true;
        if in_fence || pystr::strip(&rest).is_empty() {
            return Vec::new();
        }
        let norm = normalize_segment(&rest);
        if norm.is_empty() {
            Vec::new()
        } else {
            vec![norm]
        }
    }

    // -- internals -------------------------------------------------------------------------

    fn consume(&mut self, end: usize) -> String {
        let seg: String = self.buf[..end].iter().collect();
        let mut rest = end;
        while rest < self.buf.len() && (self.buf[rest] == ' ' || self.buf[rest] == '\t') {
            rest += 1;
        }
        if rest < self.buf.len() && self.buf[rest] == '\n' {
            rest += 1;
            self.at_line_start = true;
        } else {
            self.at_line_start = false;
        }
        self.buf.drain(..rest);
        seg
    }

    fn next_segment(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let nl = self.buf.iter().position(|&c| c == '\n');
        // Inside a code fence: drop whole lines until the closing fence.
        if self.in_fence {
            let nl = nl?;
            let line: String = self.buf[..nl].iter().collect();
            self.buf.drain(..=nl);
            self.at_line_start = true;
            if FENCE.matches_at_start(&line) {
                self.in_fence = false;
            }
            return Some(String::new());
        }
        if self.at_line_start {
            let skip = self.buf.iter().take_while(|&&c| c == ' ').count();
            let head = &self.buf[skip..];
            if head.starts_with(&['`', '`', '`']) || head.starts_with(&['~', '~', '~']) {
                let nl = nl?;
                self.buf.drain(..=nl);
                self.in_fence = true;
                self.at_line_start = true;
                return Some(String::new());
            }
            if head.len() < 3 && nl.is_none() && head.iter().all(|c| "`~ ".contains(*c)) {
                return None; // might become a fence
            }
        }
        let limit = nl.unwrap_or(self.buf.len());
        if let Some(cut) = self.punctuation_boundary(limit) {
            return Some(self.consume(cut));
        }
        if limit > self.max_chars {
            if let Some(cut) = self.soft_cut(limit, nl.is_some()) {
                return Some(self.consume(cut));
            }
        }
        if let Some(nl) = nl {
            let seg: String = self.buf[..nl].iter().collect();
            self.buf.drain(..=nl);
            self.at_line_start = true;
            return Some(seg);
        }
        None
    }

    fn punctuation_boundary(&self, limit: usize) -> Option<usize> {
        let buf = &self.buf;
        let n = buf.len();
        let mut i = 0;
        while i < limit {
            let ch = buf[i];
            if END_PUNCT.contains(ch) {
                let mut j = i;
                while j + 1 < n && END_PUNCT.contains(buf[j + 1]) {
                    j += 1;
                }
                let mut k = j + 1;
                loop {
                    while k < n && CLOSERS.contains(buf[k]) {
                        k += 1;
                    }
                    // French spacing before a closing guillemet: "oui. »"
                    if k < n && SPACES.contains(buf[k]) && k + 1 < n && buf[k + 1] == '»' {
                        k += 2;
                        continue;
                    }
                    break;
                }
                if k >= n {
                    return None; // cannot decide yet: need the next character
                }
                if SPACES.contains(buf[k]) && k + 1 >= n {
                    let opened = buf[..k].iter().filter(|&&c| c == '«').count();
                    let closed = buf[..k].iter().filter(|&&c| c == '»').count();
                    if opened > closed {
                        return None; // an open « may still be closed by " »"
                    }
                }
                if pystr::is_space(buf[k]) {
                    if ch == '.' && j == i && (is_abbreviation(buf, i) || is_list_marker(buf, i, self.at_line_start)) {
                        i = k;
                        continue;
                    }
                    return Some(k);
                }
                i = k;
                continue;
            }
            i += 1;
        }
        None
    }

    /// Cut point near `max_chars` at a word boundary (prefer after a comma/semicolon), in `buf[:limit]`.
    fn soft_cut(&self, limit: usize, complete: bool) -> Option<usize> {
        let buf = &self.buf[..limit];
        let window = &buf[..self.max_chars.min(buf.len())];
        let floor = self.max_chars / 3;
        for sep in [", ", "; ", ": ", " – ", " — ", " - "] {
            if let Some(idx) = rfind(window, sep) {
                if idx >= floor {
                    return Some(idx + pystr::len(sep.trim_end()));
                }
            }
        }
        if let Some(idx) = rfind(window, " ") {
            if idx >= floor {
                return Some(idx);
            }
        }
        // One huge token (URL...): cut at the next whitespace, once it is known.
        if let Some(next) = find_from(buf, " ", self.max_chars) {
            if next > 0 {
                return Some(next);
            }
        }
        if complete || buf.len() > 4 * self.max_chars {
            return Some(buf.len());
        }
        None
    }
}

pub fn split_sentences(text: &str, max_chars: usize) -> Vec<String> {
    let mut splitter = SentenceSplitter::new(max_chars);
    let mut out = splitter.feed(text);
    out.extend(splitter.flush());
    out
}

/// Without emoji (« Bravo 🎉 ! » → « Bravo ! »), the spaces they leave tidied.
pub fn strip_emoji(text: &str) -> String {
    if !EMOJI.is_match(text) {
        return text.to_string();
    }
    let text = EMOJI_BEFORE_STOP.replace(text, ""); // « Super 👍. » → « Super. »
    let text = EMOJI.replace(&text, " ");
    let text = TABS_SPACES.replace(&text, " ");
    text.split('\n').map(pystr::strip).collect::<Vec<_>>().join("\n")
}

/// Whole-text normalization (used for outbox audio): sentences joined by spaces. `paragraphs`: one line
/// per paragraph or list item, so the reader (Pocket) breathes between them instead of running on.
pub fn normalize_for_speech(text: &str, max_chars: usize, paragraphs: bool) -> String {
    if !paragraphs {
        return split_sentences(text, max_chars).join(" ");
    }
    let blocks = BLOCKS.split(&FENCE.replace(&text.replace("\r\n", "\n"), ""));
    blocks
        .iter()
        .map(|block| split_sentences(block, max_chars).join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(text: &str, step: usize, max_chars: usize) -> Vec<String> {
        let mut sp = SentenceSplitter::new(max_chars);
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        for piece in chars.chunks(step) {
            out.extend(sp.feed(&piece.iter().collect::<String>()));
        }
        out.extend(sp.flush());
        out
    }

    fn split(text: &str) -> Vec<String> {
        split_sentences(text, 180)
    }

    #[test]
    fn basic_punctuation_and_decimals() {
        let text = "Bonjour à tous. Il fait 3.5 degrés dehors ! Et toi ? Bon… À demain";
        let expected = ["Bonjour à tous.", "Il fait 3.5 degrés dehors !", "Et toi ?", "Bon…", "À demain."];
        assert_eq!(split(text), expected);
        // Same result whatever the delta boundaries (character by character, small chunks).
        assert_eq!(stream(text, 1, 180), expected);
        assert_eq!(stream(text, 3, 180), expected);
    }

    #[test]
    fn decimal_split_across_deltas_is_not_cut() {
        let mut sp = SentenceSplitter::new(180);
        assert!(sp.feed("La dose est de 3.").is_empty()); // undecided: next char unknown
        assert_eq!(sp.feed("5 mg. Ensuite"), ["La dose est de 3.5 mg."]);
        assert_eq!(sp.flush(), ["Ensuite."]);
    }

    #[test]
    fn abbreviations_and_initials() {
        assert_eq!(
            split("M. Dupont arrive, etc. et puis J. Martin aussi. Fin."),
            ["M. Dupont arrive, etc. et puis J. Martin aussi.", "Fin."]
        );
    }

    #[test]
    fn closing_quotes_and_emphasis_stay_with_sentence() {
        assert_eq!(
            split("Il a dit « oui. » Puis **c'est fait.** Voilà"),
            ["Il a dit « oui. »", "Puis c'est fait.", "Voilà."]
        );
    }

    #[test]
    fn markdown_lists_headings_links_code() {
        let text =
            "## Plan du soir\n\n- **Coucher** à 22 h\n- Lire [cet article](https://example.com/a.b) ou https://x.y/z\n\
                    1. Respirer\n2. Dormir\n```bash\nrm -rf /tmp/x. y\n```\nUtilise `sleep_score` demain.\n";
        assert_eq!(
            split(text),
            [
                "Plan du soir.",
                "Coucher à 22 h.",
                "Lire cet article ou.",
                "Respirer.",
                "Dormir.",
                "Utilise sleep_score demain."
            ]
        );
    }

    #[test]
    fn table_rows_read_as_lists() {
        assert_eq!(split("| Jour | Heures |\n|---|---|\n| Lundi | 7 |\n"), ["Jour, Heures.", "Lundi, 7."]);
    }

    #[test]
    fn long_text_cut_at_word_boundary_near_max() {
        let words = "mot ".repeat(100).trim().to_string();
        let parts = split_sentences(&words, 60);
        assert!(parts.len() > 1);
        for part in &parts {
            assert!(part.chars().count() <= 61); // 60 + final period
            assert!(!part.starts_with(' '));
        }
        let joined: Vec<String> = parts.iter().map(|p| p.trim_end_matches('.').to_string()).collect();
        assert_eq!(
            joined.join(" ").split_whitespace().collect::<Vec<_>>(),
            words.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn long_text_prefers_comma() {
        let text = format!("Premier morceau assez long pour dépasser la limite fixée, {}", "suite ".repeat(20));
        assert_eq!(split_sentences(&text, 80)[0], "Premier morceau assez long pour dépasser la limite fixée.");
    }

    #[test]
    fn streamed_long_text_waits_for_word_end() {
        let mut sp = SentenceSplitter::new(50);
        let out = sp.feed(&format!("{} {}", "a".repeat(10), "b".repeat(45))); // 56 chars, no space after the long word yet
        assert!(out == vec![format!("{}.", "a".repeat(10))] || out.is_empty());
        let mut rest = sp.feed("bbb fin");
        rest.extend(sp.flush());
        let all: String = out.iter().chain(rest.iter()).cloned().collect::<String>().replace(['.', ' '], "");
        assert_eq!(all, format!("{}{}fin", "a".repeat(10), "b".repeat(48)));
    }

    #[test]
    fn normalize_segment_drops_empty_and_symbols() {
        assert_eq!(normalize_segment("---"), "");
        assert_eq!(normalize_segment("**"), "");
        assert_eq!(normalize_segment("<b>Gras</b> et _italique_"), "Gras et italique.");
        assert_eq!(normalize_segment("snake_case_name reste"), "snake_case_name reste.");
    }

    #[test]
    fn normalize_for_speech_joins() {
        assert_eq!(
            normalize_for_speech("# Titre\nBonjour **toi**.\n\nÇa va ?", 180, false),
            "Titre. Bonjour toi. Ça va ?"
        );
    }

    #[test]
    fn trailing_space_after_sentence_is_emitted_immediately() {
        let mut sp = SentenceSplitter::new(180);
        assert_eq!(sp.feed("Je dois demander. "), ["Je dois demander."]);
        assert!(sp.feed("Il dit « oui. ").is_empty()); // waits: the guillemet may close
        assert_eq!(sp.feed("» Ensuite"), ["Il dit « oui. »"]);
    }

    #[test]
    fn paragraphs_are_kept_for_a_phrasing_reader() {
        let text = "Intro **courte** :\n\n- Premier point\n- Second point.\n\n```sh\nrm -rf x\n\nls\n```\nFin ?";
        assert_eq!(
            normalize_for_speech(text, 180, true).split('\n').collect::<Vec<_>>(),
            ["Intro courte :", "Premier point.", "Second point.", "Fin ?"]
        ); // items get their full stop
        assert!(!normalize_for_speech(text, 180, false).contains('\n'));
    }

    #[test]
    fn emoji_are_never_read() {
        for (text, spoken) in [
            ("Bravo 🎉 !", "Bravo !"),
            ("Super 👍.", "Super."),
            ("🎧 Podcast du matin", "Podcast du matin"),
            ("Ta nuit : 7 h 😴 bien dormi ✅", "Ta nuit : 7 h bien dormi"), // French spacing before « : » kept
            ("Famille 👨\u{200d}👩\u{200d}👧 ok", "Famille ok"),            // a whole ZWJ sequence
            ("Note 1\u{fe0f}\u{20e3} ici", "Note 1 ici"),
            ("Rien à enlever, 3 → 4.", "Rien à enlever, 3 → 4."),
        ] {
            assert_eq!(strip_emoji(text), spoken, "{text}");
        }
    }
}
