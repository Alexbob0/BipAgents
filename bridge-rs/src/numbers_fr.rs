//! French numbers in words, for speech: Pocket TTS reads digits one by one ("91" -> "neuf un").
//!
//! `spell_numbers` rewrites integers, decimals, times, ordinals, percentages, money, temperatures and a few
//! common units ("91" -> "quatre-vingt-onze", "14h30" -> "quatorze heures trente", "3,5 %" -> "trois virgule
//! cinq pour cent", "1er" -> "premier"). Anything it does not recognize is left untouched.
//!
//! Port of `bridge/bipbridge/numbers_fr.py` (owner group: voice-speech). Numbers are handled as digit strings, so
//! any length spells as Python's unbounded ints do; the patterns run on `voice_speech::pyre` (Python's `re`).

use std::sync::LazyLock;

use crate::voice_speech::pyre::{Match, Regex, IGNORECASE};
use crate::voice_speech::pystr;

// Python's `_X` names lose their leading underscore here (`_CAMEL` is `CAMEL`); speech_intl.rs uses them.

const UNITS: [&str; 17] = [
    "zéro", "un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit", "neuf", "dix", "onze", "douze", "treize",
    "quatorze", "quinze", "seize",
];

fn tens_word(tens: u32) -> &'static str {
    match tens {
        2 => "vingt",
        3 => "trente",
        4 => "quarante",
        5 => "cinquante",
        _ => "soixante",
    }
}

fn below_100(n: u32) -> String {
    if n <= 16 {
        return UNITS[n as usize].to_string();
    }
    if n < 20 {
        return format!("dix-{}", UNITS[(n - 10) as usize]);
    }
    let (tens, unit) = (n / 10, n % 10);
    if tens == 7 {
        return if n == 71 { "soixante et onze".to_string() } else { format!("soixante-{}", below_100(n - 60)) };
    }
    if tens == 8 {
        return if unit == 0 { "quatre-vingts".to_string() } else { format!("quatre-vingt-{}", UNITS[unit as usize]) };
    }
    if tens == 9 {
        return format!("quatre-vingt-{}", below_100(n - 80));
    }
    let word = tens_word(tens);
    match unit {
        0 => word.to_string(),
        1 => format!("{word} et un"),
        _ => format!("{word}-{}", UNITS[unit as usize]),
    }
}

fn below_1000(n: u32) -> String {
    let (hundreds, rest) = (n / 100, n % 100);
    if hundreds == 0 {
        return below_100(rest);
    }
    let head = if hundreds == 1 { "cent".to_string() } else { format!("{} cent", UNITS[hundreds as usize]) };
    if rest == 0 {
        return head + if hundreds > 1 { "s" } else { "" };
    }
    format!("{head} {}", below_100(rest))
}

/// A digit string without leading zeros ("0" for zero): `str(int(digits))`.
fn canonical(digits: &str) -> String {
    pystr::int_str(digits)
}

/// (n // 10**k, n % 10**k) of a canonical digit string, both canonical.
fn split_power(n: &str, k: usize) -> (String, String) {
    if n.len() <= k {
        return ("0".to_string(), n.to_string());
    }
    let (high, low) = n.split_at(n.len() - k);
    (canonical(high), canonical(low))
}

/// `spell_int(int(digits))`: any size, in words ("quatre-vingt-onze", "deux mille vingt-six").
pub fn spell_int_digits(digits: &str) -> String {
    let n = canonical(digits);
    if n.len() <= 3 {
        return below_1000(n.parse().unwrap_or(0));
    }
    let mut parts = Vec::new();
    let mut rest = n;
    for (k, singular, plural) in [(9, "milliard", "milliards"), (6, "million", "millions")] {
        let (count, low) = split_power(&rest, k);
        rest = low;
        if count != "0" {
            parts.push(format!("{} {}", spell_int_digits(&count), if count == "1" { singular } else { plural }));
        }
    }
    let (thousands, low) = split_power(&rest, 3);
    let thousands: u32 = thousands.parse().unwrap_or(0);
    let low: u32 = low.parse().unwrap_or(0);
    if thousands != 0 {
        let mut words = below_1000(thousands);
        if words.ends_with("vingts") || words.ends_with("cents") {
            // "quatre-vingt mille", "deux cent mille"
            words.pop();
        }
        parts.push(if thousands == 1 { "mille".to_string() } else { format!("{words} mille") });
    }
    if low != 0 {
        parts.push(below_1000(low));
    }
    parts.join(" ")
}

/// 0 <= n, in words ("quatre-vingt-onze", "deux mille vingt-six"); negative: "moins …".
pub fn spell_int(n: i64) -> String {
    if n < 0 {
        return format!("moins {}", spell_int_digits(&n.unsigned_abs().to_string()));
    }
    spell_int_digits(&n.to_string())
}

/// `_feminine`: "un" -> "une" at the end ("vingt et une heures").
fn feminine(words: &str) -> String {
    if words == "un" || words.ends_with(" un") || words.ends_with("-un") {
        format!("{}une", &words[..words.len() - 2])
    } else {
        words.to_string()
    }
}

fn drop_last_char(text: &str) -> &str {
    match text.char_indices().last() {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// `spell_ordinal(int(digits), feminine)`.
pub fn spell_ordinal_digits(digits: &str, feminine: bool) -> String {
    let n = canonical(digits);
    if n == "1" {
        return if feminine { "première" } else { "premier" }.to_string();
    }
    let mut words = spell_int_digits(&n);
    if words.ends_with("cinq") {
        return words + "uième";
    }
    if words.ends_with("neuf") {
        return format!("{}vième", drop_last_char(&words));
    }
    if words.ends_with('s') {
        // quatre-vingts, deux cents
        words.pop();
    }
    if words.ends_with('e') {
        return format!("{}ième", drop_last_char(&words));
    }
    words + "ième"
}

pub fn spell_ordinal(n: u64, feminine: bool) -> String {
    spell_ordinal_digits(&n.to_string(), feminine)
}

/// A digit string: leading zeros are read ("06" -> "zéro six"), huge numbers digit by digit.
fn spell_digits(digits: &str) -> String {
    let count = pystr::len(digits);
    if count > 1 && digits.starts_with('0') {
        let stripped = digits.trim_start_matches('0');
        let zeros = count - pystr::len(stripped);
        let mut words = vec!["zéro".to_string(); zeros];
        if !digits.trim_matches('0').is_empty() {
            words.push(spell_int_digits(digits));
        }
        return words.join(" ");
    }
    let value = canonical(digits);
    if value.len() > 12 {
        // >= 10**12
        return digits
            .chars()
            .map(|d| UNITS[pystr::decimal_value(d).unwrap_or(0) as usize])
            .collect::<Vec<_>>()
            .join(" ");
    }
    spell_int_digits(&value)
}

fn decimal(whole: &str, fraction: &str) -> String {
    format!("{} virgule {}", spell_digits(whole), spell_digits(fraction))
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern)
}

const NUM: &str = r"\d+(?:[,.]\d+)?";
static GROUPED: LazyLock<Regex> =
    LazyLock::new(|| re("(?<![\\d,.])(\\d{1,3}(?:[ \u{a0}\u{202f}]\\d{3})+)(?![\\d]|[,.]\\d)"));
static GROUP_SPACES: LazyLock<Regex> = LazyLock::new(|| re("[ \u{a0}\u{202f}]"));
static ORDINAL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)(er|re|ère|ème|eme|e)\b"));
static TIME_H: LazyLock<Regex> = LazyLock::new(|| re(r"\b([01]?\d|2[0-3]) ?[hH](?: ?([0-5]\d))?\b"));
static TIME_COLON: LazyLock<Regex> = LazyLock::new(|| re(r"\b([01]?\d|2[0-3]):([0-5]\d)\b"));
/// `_ALONE`: a pattern fragment (not compiled). A unit of one letter only when it stands alone (« 3 l'ont », « 5
/// m'a » aren't litres or metres); the longer ones first.
pub const ALONE: &str = r"(?![\w'’])";
const UNIT_WORDS: &[(&str, &str)] = &[
    (r"%", "pour cent"),
    (r"€|EUR\b|euros?\b", "euros"),
    (r"\$|USD\b|dollars?\b", "dollars"),
    (r"°C\b|° ?C\b|°", "degrés"),
    (r"km/h\b", "kilomètres heure"),
    (r"km²", "kilomètres carrés"),
    (r"km\b", "kilomètres"),
    (r"kg\b", "kilos"),
    (r"kWh\b", "kilowattheures"),
    (r"kW\b", "kilowatts"),
    (r"cm\b", "centimètres"),
    (r"mm\b", "millimètres"),
    (r"m²", "mètres carrés"),
    (r"m³", "mètres cubes"),
    (r"ms\b", "millisecondes"),
    (r"mg\b", "milligrammes"),
    (r"m[lL]\b", "millilitres"),
    (r"min\b", "minutes"),
    (r"bpm\b", "battements par minute"),
    (r"c[lL]\b", "centilitres"),
    (r"[TG]o\b|[TG]B\b", "gigaoctets"),
    (r"[MK]o\b|[MK]B\b", "mégaoctets"),
    (r"g(?![\w'’])", "grammes"),
    (r"[lL](?![\w'’])", "litres"),
    (r"m(?![\w'’])", "mètres"),
    (r"W(?![\w'’])", "watts"),
];
const BYTES: [(&str, &str); 4] =
    [("To", "téraoctets"), ("TB", "téraoctets"), ("Ko", "kilooctets"), ("KB", "kilooctets")];
static UNIT_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> =
    LazyLock::new(|| UNIT_WORDS.iter().map(|(p, w)| (re(p), *w)).collect());
static WITH_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    let units: Vec<&str> = UNIT_WORDS.iter().map(|(p, _)| *p).collect();
    re(&format!(r"(?<![\w,.])(-?{NUM}) ?({})", units.join("|")))
});
static DECIMAL: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])(\d+)[,.](\d+)(?![\d.,]*\d)"));
static INTEGER: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])(\d+)(?![\w]|[.,]\d)"));
static NEGATIVE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w-])-(?=\d)"));
static NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"(-?)(\d+)(?:[,.](\d+))?"));

/// "12" or "3,5" / "3.5" in words.
fn number(text: &str) -> String {
    let Some(groups) = NUMBER.fullmatch(text) else {
        return text.to_string();
    };
    let sign = groups[1].clone().unwrap_or_default();
    let whole = groups[2].clone().unwrap_or_default();
    let words = match &groups[3] {
        Some(fraction) if !fraction.is_empty() => decimal(&whole, fraction),
        _ => spell_digits(&whole),
    };
    format!("{}{words}", if sign.is_empty() { "" } else { "moins " })
}

fn unit_word(symbol: &str) -> String {
    if let Some((_, word)) = BYTES.iter().find(|(s, _)| *s == symbol) {
        return word.to_string();
    }
    for (pattern, word) in UNIT_PATTERNS.iter() {
        if pattern.fullmatch(symbol).is_some() {
            return word.to_string();
        }
    }
    symbol.to_string()
}

fn with_unit(m: &Match) -> String {
    let (value, symbol) = (m.group(1), m.group(2));
    let mut word = unit_word(&symbol);
    let mut spoken = number(&value);
    let bare = value.trim_start_matches('-');
    if bare == "0" || bare == "1" {
        // « un kilomètre », « une minute », « zéro degré »
        word = pystr::split_ws(&word)
            .into_iter()
            .map(|w| if w.ends_with('s') && w != "pour" { drop_last_char(w) } else { w })
            .collect::<Vec<_>>()
            .join(" ");
        let first = pystr::split_ws(&word).first().map(|s| s.to_string()).unwrap_or_default();
        if (first == "minute" || first == "milliseconde") && bare == "1" {
            spoken = feminine(&spoken);
        }
    }
    format!("{spoken} {word}")
}

fn time(hours: &str, minutes: Option<&str>) -> String {
    let h = canonical(hours);
    let mut words = feminine(&spell_int_digits(&h)) + if h == "0" || h == "1" { " heure" } else { " heures" };
    if let Some(minutes) = minutes.filter(|m| !m.is_empty()) {
        let m = canonical(minutes);
        if m != "0" {
            words += " ";
            words += &feminine(&spell_int_digits(&m));
        }
    }
    words
}

const MONTHS: [&str; 12] = [
    "janvier",
    "février",
    "mars",
    "avril",
    "mai",
    "juin",
    "juillet",
    "août",
    "septembre",
    "octobre",
    "novembre",
    "décembre",
];
const FRACTIONS: [((u32, u32), &str); 5] =
    [((1, 2), "un demi"), ((1, 3), "un tiers"), ((2, 3), "deux tiers"), ((1, 4), "un quart"), ((3, 4), "trois quarts")];
static PER: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+) ?([hj]) ?/ ?(\d+)\b"));
static DATE_FULL: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,2})/(\d{1,2})/(\d{4}|\d{2})\b"));
// Dates with dashes (« 12-10-2026 », ISO « 2026-10-12 ») and ranges (« 42-46 % », « 9-12h », « 2025–2026 »):
// a range left as « quarante-deux-quarante-six » is one long hyphenated word the voice stumbles on (it repeated
// « quarante-deux » in a loop), so the dash between two numbers is read « à ».
static DATE_DASHED: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{1,2})-(\d{1,2})-(\d{4})\b"));
static DATE_ISO: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{4})-(\d{2})-(\d{2})\b"));
pub static RANGE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=\d)\s?[-–—]\s?(?=\d)"));
static DATE_SHORT: LazyLock<Regex> = LazyLock::new(|| re(r"\b(0[1-9]|[12]\d|3[01])/(0[1-9]|1[0-2])\b"));
static FRACTION: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w/])(\d+) ?/ ?(\d+)(?![\w/])"));

/// A small number of a digit string (`int(x)` compared with small bounds); None beyond u32.
fn small(digits: &str) -> Option<u32> {
    canonical(digits).parse().ok()
}

fn date(day: &str, month: &str, year: Option<&str>) -> Option<String> {
    let (d, m) = (small(day)?, small(month)?);
    if !((1..=31).contains(&d) && (1..=12).contains(&m)) {
        return None;
    }
    let mut words = format!(
        "{} {}",
        if d == 1 { "premier".to_string() } else { spell_int_digits(&d.to_string()) },
        MONTHS[m as usize - 1]
    );
    if let Some(year) = year.filter(|y| !y.is_empty()) {
        let mut y: u64 = canonical(year).parse().unwrap_or(0);
        if pystr::len(year) == 2 {
            y += 2000;
        }
        words += " ";
        words += &spell_int_digits(&y.to_string());
    }
    Some(words)
}

fn per(m: &Match) -> String {
    let unit = if m.group(2) == "h" { "heures" } else { "jours" };
    format!("{} {unit} sur {}", spell_int_digits(&m.group(1)), spell_int_digits(&m.group(3)))
}

fn fraction(m: &Match) -> String {
    let (n, d) = (small(&m.group(1)), small(&m.group(2)));
    if let (Some(n), Some(d)) = (n, d) {
        if let Some((_, words)) = FRACTIONS.iter().find(|(key, _)| *key == (n, d)) {
            return words.to_string();
        }
    }
    format!("{} sur {}", spell_digits(&m.group(1)), spell_digits(&m.group(2)))
}

pub fn spell_numbers(text: &str) -> String {
    if !pystr::has_digit(text) {
        return text.to_string();
    }
    let text = GROUPED.replace_with(text, |m| GROUP_SPACES.replace(&m.group(1), ""));
    let text = PER.replace_with(&text, per);
    let text = DATE_DASHED
        .replace_with(&text, |m| date(&m.group(1), &m.group(2), Some(&m.group(3))).unwrap_or_else(|| m.whole()));
    let text = DATE_ISO
        .replace_with(&text, |m| date(&m.group(3), &m.group(2), Some(&m.group(1))).unwrap_or_else(|| m.whole()));
    let text = RANGE.replace(&text, " à ");
    let text = DATE_FULL
        .replace_with(&text, |m| date(&m.group(1), &m.group(2), Some(&m.group(3))).unwrap_or_else(|| m.whole()));
    let text = DATE_SHORT.replace_with(&text, |m| date(&m.group(1), &m.group(2), None).unwrap_or_else(|| m.whole()));
    let text = FRACTION.replace_with(&text, fraction);
    let text = ORDINAL.replace_with(&text, |m| {
        let suffix = m.group(2);
        spell_ordinal_digits(&m.group(1), suffix == "re" || suffix == "ère")
    });
    let text = TIME_COLON.replace_with(&text, |m| time(&m.group(1), m.get(2).as_deref()));
    let text = TIME_H.replace_with(&text, |m| time(&m.group(1), m.get(2).as_deref()));
    let text = WITH_UNIT.replace_with(&text, with_unit);
    let text = DECIMAL.replace_with(&text, |m| decimal(&m.group(1), &m.group(2)));
    let text = NEGATIVE.replace(&text, "moins ");
    INTEGER.replace_with(&text, |m| spell_digits(&m.group(1)))
}

pub static CAMEL: LazyLock<Regex> = LazyLock::new(|| re(r"\b[A-Z][a-z]+(?:[A-Z][a-z]+|[A-Z]{2,}\b)+\b"));
pub static OPEN: LazyLock<Regex> = LazyLock::new(|| re(r"\s*[(\[]\s*"));
pub static CLOSE: LazyLock<Regex> = LazyLock::new(|| re(r"\s*[)\]]\s*"));
pub static DASH: LazyLock<Regex> = LazyLock::new(|| re(r"\s+[—–]\s+"));
pub static COMMA_RUNS: LazyLock<Regex> = LazyLock::new(|| re(r"\s*,(?:\s*,)+\s*"));
pub static COMMA_BEFORE_END: LazyLock<Regex> = LazyLock::new(|| re(r"\s*,\s*([.!?…:;])"));
static CAMEL_JOIN: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=[a-z])(?=[A-Z])"));
pub static SPACE_RUNS: LazyLock<Regex> = LazyLock::new(|| re(r" {2,}"));

/// "DuckDuckGo" -> "Duck Duck Go", "OpenAI" -> "Open AI" (each part is then read as a word); "WhatsApp" stays.
pub fn split_camel(word: &str) -> String {
    if KEEP_WHOLE.contains(&pystr::lower(word).as_str()) {
        return word.to_string();
    }
    CAMEL_JOIN.replace(word, " ")
}

/// A pattern fragment (not compiled). « @ » outside an e-mail address (« RDV @ 14h », « prix@10 € »): a word of the
/// language; before a name, a mention (« avec @Léa »): not read; an address stays as it is.
pub const MENTION_AT: &str = r"(?<![\w.+-])@(?=[^\W\d_])";
/// A pattern fragment (not compiled).
pub const NOT_EMAIL_AT: &str = r"\s*(?:(?<![\w.+-])@|@(?![\w-]+(?:\.[\w-]+)+))\s*";
/// Followed by a capitalized name.
const CAP: &str = r"(?=[ -]?[A-ZÀ-Ý])";
/// English words common in the agents' replies, respelled so a French voice says them right
/// (singular, plural). Tuned by ear: adjust the spelling when one still sounds off.
const ENGLISH: &[(&str, &str)] = &[
    ("run", "reune"),
    ("runs", "reunes"),
    ("cron", "crone"),
    ("crons", "crones"),
    ("live", "laïve"),
    ("lives", "laïves"),
    ("push", "pouche"),
    ("bug", "beug"),
    ("bugs", "beugs"),
    ("mail", "mèle"),
    ("mails", "mèles"),
    ("skill", "skile"),
    ("skills", "skiles"),
    ("prompt", "prompte"),
    ("prompts", "promptes"),
    ("token", "tokène"),
    ("tokens", "tokènes"),
    ("update", "eupdète"),
    ("updates", "eupdètes"),
    ("cloud", "claoude"),
    ("bridge", "bridje"),
    ("wellness", "ouèlnesse"),
    ("trainline", "trène laïne"),
    ("feedback", "fidbak"),
    ("workflow", "weurkflo"),
    ("workflows", "weurkflos"),
    ("check", "tchèque"),
    ("checks", "tchèques"),
    // Brands, picked by ear on 2026-10-15 (voices/ab-test, Galet): the others sound right as they are written.
    ("garmin", "Garmine"),
    ("oura", "Oûra"),
    ("spotify", "Spotifaï"),
    ("github", "Guite-heub"),
    ("tailscale", "Téil-skèil"),
    ("bluetooth", "Blou-tousse"),
    ("icloud", "Aïe-claoude"),
    ("chatgpt", "Tchat G P T"),
    ("uber", "Oubeur"),
    ("netflix", "Net-flix"),
];
/// Brands said right as they are written, which the CamelCase split would cut (« Whats App »).
const KEEP_WHOLE: [&str; 4] = ["whatsapp", "youtube", "airpods", "linkedin"];

/// Symbols and abbreviations a TTS engine would read letter by letter or skip. Add entries here as they come up.
static LEXICON: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    let source: Vec<(String, &str)> = vec![
        // An arrow is as often « then » or « so » as « to »: a pause, and nothing at the start of a line (a bullet).
        (r"(?m)^[ \t]*(?:→|->|⟶|⇒|=>)[ \t]*".into(), ""),
        (r"\s*(?:→|->|⟶|⇒|=>)\s*".into(), ", "),
        (r"\s*(?:←|<-)\s*".into(), ", "),
        (r"\bSt(?=[- ][A-ZÀ-Ý])".into(), "Saint"),
        (r"\bSte(?=[- ][A-ZÀ-Ý])".into(), "Sainte"),
        (r"\bM\.(?= [A-ZÀ-Ý])".into(), "Monsieur"),
        (r"\bMM\.(?= [A-ZÀ-Ý])".into(), "Messieurs"),
        (r"\bMme\b\.?".into(), "Madame"),
        (r"\bMmes\b\.?".into(), "Mesdames"),
        (r"\bMlle\b\.?".into(), "Mademoiselle"),
        (format!(r"\bDr\b\.?{CAP}"), "Docteur"),
        (format!(r"\bPr\b\.?{CAP}"), "Professeur"),
        (r"\bBd\b\.?".into(), "boulevard"),
        (r"\bav\.(?= )".into(), "avenue"),
        (r"\benv\.".into(), "environ"),
        (r"\bapprox\.".into(), "environ"),
        (r"\bp\. ?ex\.".into(), "par exemple"),
        (r"\bc\.?-à-d\.?".into(), "c'est-à-dire"),
        (r"\betc\.(?=\s*\S)".into(), "et cetera,"),
        (r"\betc\.".into(), "et cetera."),
        (r"\bcf\.".into(), "voir"),
        (r"\bvs\.?(?= )".into(), "contre"),
        (r"\b[nN]°\s?".into(), "numéro "),
        (r"\bRDV\b|\brdv\b".into(), "rendez-vous"),
        (r"\bsvp\b|\bSVP\b".into(), "s'il vous plaît"),
        (r"\btél\.".into(), "téléphone"),
        (r"\s+&\s+".into(), " et "),
        (r"\s+\+\s+".into(), " plus "),
        (r"(?<=\d)\s?[~∼]\s?(?=\d)".into(), " à "),
        (r"\s*(?<!~)[~∼≈≃](?!~)\s*".into(), " environ "),
        (r"(?<!\w)≥\s?".into(), "au moins "),
        (r"(?<!\w)≤\s?".into(), "au plus "),
        (r"(?<=\d)\s?×\s?(?=\d)".into(), " fois "),
        (MENTION_AT.into(), ""),
        (NOT_EMAIL_AT.into(), " à "),
    ];
    let mut rules: Vec<(Regex, &'static str)> = source.into_iter().map(|(p, w)| (re(&p), w)).collect();
    for (word, spoken) in ENGLISH {
        rules.push((Regex::with_flags(&format!(r"\b{word}\b"), IGNORECASE), *spoken));
    }
    rules.push((re(r"\bNotion\b"), "Nôcheune")); // the app; « une notion » stays French
    rules
});

static SLASH_WORDS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::with_flags(r"\bet ?/ ?ou\b", IGNORECASE), "et ou"),
        (Regex::with_flags(r"\baller ?/ ?retour\b", IGNORECASE), "aller-retour"),
        (re(r"\bA/R\b"), "aller-retour"),
        (re(r"\bkm/h\b"), "kilomètres heure"),
    ]
});
static SLASH: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=\w) ?/ ?(?=\w)"));

/// Last touch before a TTS engine: symbols and abbreviations expanded, numbers in words, slashes read ("ou", "sur",
/// dates), CamelCase names split, and parentheses or dashes read as pauses ("le train (TGV) part" -> "le train, TGV,
/// part").
pub fn prepare_for_synthesis(text: &str) -> String {
    let mut text = text.to_string();
    for (pattern, words) in LEXICON.iter() {
        text = pattern.replace(&text, words);
    }
    text = spell_numbers(&text);
    for (pattern, words) in SLASH_WORDS.iter() {
        text = pattern.replace(&text, words);
    }
    text = SLASH.replace(&text, " ou "); // "Garmin/Oura" -> "Garmin ou Oura"
    pauses(&text)
}

/// The end shared with speech_intl: CamelCase split, dashes and parentheses as pauses, commas tidied.
pub(crate) fn pauses(text: &str) -> String {
    let text = CAMEL.replace_with(text, |m| split_camel(&m.whole()));
    let text = DASH.replace(&text, ", ");
    let text = OPEN.replace(&text, ", ");
    let text = CLOSE.replace(&text, ", ");
    let text = COMMA_RUNS.replace(&text, ", ");
    let text = COMMA_BEFORE_END.replace(&text, r"\1");
    let text = SPACE_RUNS.replace(&text, " ");
    pystr::strip_chars(&text, " ,").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spell_int_cases() {
        for (n, words) in [
            (0, "zéro"),
            (1, "un"),
            (16, "seize"),
            (17, "dix-sept"),
            (21, "vingt et un"),
            (22, "vingt-deux"),
            (70, "soixante-dix"),
            (71, "soixante et onze"),
            (77, "soixante-dix-sept"),
            (80, "quatre-vingts"),
            (81, "quatre-vingt-un"),
            (91, "quatre-vingt-onze"),
            (99, "quatre-vingt-dix-neuf"),
            (100, "cent"),
            (101, "cent un"),
            (200, "deux cents"),
            (201, "deux cent un"),
            (1000, "mille"),
            (1990, "mille neuf cent quatre-vingt-dix"),
            (2026, "deux mille vingt-six"),
            (80000, "quatre-vingt mille"),
            (200000, "deux cent mille"),
            (1000000, "un million"),
            (2500000, "deux millions cinq cent mille"),
            (80000000, "quatre-vingts millions"),
        ] {
            assert_eq!(spell_int(n), words, "{n}");
        }
    }

    #[test]
    fn spell_ordinal_cases() {
        for (n, words) in [
            (1, "premier"),
            (2, "deuxième"),
            (5, "cinquième"),
            (9, "neuvième"),
            (21, "vingt et unième"),
            (80, "quatre-vingtième"),
            (1000, "millième"),
        ] {
            assert_eq!(spell_ordinal(n, false), words, "{n}");
        }
    }

    #[test]
    fn spell_numbers_cases() {
        for (text, spoken) in [
            ("Ta fréquence moyenne était de 91.", "Ta fréquence moyenne était de quatre-vingt-onze."),
            ("Rendez-vous à 14h30, puis à 9h.", "Rendez-vous à quatorze heures trente, puis à neuf heures."),
            ("Réveil à 07:05 et 1h de marche.", "Réveil à sept heures cinq et une heure de marche."),
            ("Ça fait 3,5 % de plus.", "Ça fait trois virgule cinq pour cent de plus."),
            ("Le billet coûte 1 200 € ou 1 € de frais.", "Le billet coûte mille deux cents euros ou un euro de frais."),
            ("Il fera 18 °C demain, -2° la nuit.", "Il fera dix-huit degrés demain, moins deux degrés la nuit."),
            (
                "Le 1er octobre, la 1re séance, le 21e jour.",
                "Le premier octobre, la première séance, le vingt et unième jour.",
            ),
            (
                "Tu as couru 10 km en 52 min, à 145 bpm.",
                "Tu as couru dix kilomètres en cinquante-deux minutes, à cent quarante-cinq battements par minute.",
            ),
            ("Ta VFC est de 45 ms.", "Ta VFC est de quarante-cinq millisecondes."),
            ("Appelle le 06 12 34.", "Appelle le zéro six douze trente-quatre."),
            ("Version 3.3.0 sans chiffres lus.", "Version 3.3.0 sans chiffres lus."),
            ("Entre 10-12 personnes.", "Entre dix à douze personnes."),
            ("Rien à lire ici.", "Rien à lire ici."),
        ] {
            assert_eq!(spell_numbers(text), spoken, "{text}");
        }
    }

    #[test]
    fn prepare_for_synthesis_cases() {
        for (text, spoken) in [
            ("Cherche sur DuckDuckGo ou YouTube.", "Cherche sur Duck Duck Go ou YouTube."),
            ("Selon OpenAI, iPhone et SNCF restent tels quels.", "Selon Open AI, iPhone et SNCF restent tels quels."),
            ("Le train (TGV) part à 9h.", "Le train, TGV, part à neuf heures."),
            ("Deux options (la seconde est moins chère).", "Deux options, la seconde est moins chère."),
            ("Prends le bus — ou marche [si beau temps].", "Prends le bus, ou marche, si beau temps."),
            ("Sommeil noté 8/10, 3/4 du trajet, 1/2 litre.", "Sommeil noté huit sur dix, trois quarts du trajet, un demi litre."),
            ("Ouvert 7j/7 et 24h/24.", "Ouvert sept jours sur sept et vingt-quatre heures sur vingt-quatre."),
            (
                "Rendez-vous le 04/10/2026, rappel le 01/11.",
                "Rendez-vous le quatre octobre deux mille vingt-six, rappel le premier novembre.",
            ),
            (
                "Données Garmin/Oura, billet aller/retour, A/R, et/ou à 50 km/h.",
                "Données Garmine ou Oûra, billet aller-retour, aller-retour, et ou à cinquante kilomètres heure.",
            ),
            (
                "Bordeaux St-Jean → Paris Montparnasse le 03/02/2027 : 19 €.",
                "Bordeaux Saint-Jean, Paris Montparnasse le trois février deux mille vingt-sept : dix-neuf euros.",
            ),
            ("Pluie -> annulation => on reste au chaud.\n→ Premier point", "Pluie, annulation, on reste au chaud.\nPremier point"),
            (
                "Environ 1 mm, 3 m de haut, 250 g de farine, 1 L d'eau, 50 cl, 2 Go, 500 Mo, 1 kWh, 12 m².",
                "Environ un millimètre, trois mètres de haut, deux cent cinquante grammes de farine, un litre d'eau, cinquante \
                 centilitres, deux gigaoctets, cinq cents mégaoctets, un kilowattheure, douze mètres carrés.",
            ),
            ("Ils sont 3 l'ont vu, 5 m'a dit.", "Ils sont trois l'ont vu, cinq m'a dit."),
            (
                "RDV @ 14h avec @Léa @ la maison, prix@10 €, écris à alex.martin+bip@example.co.uk.",
                "rendez-vous à quatorze heures avec Léa à la maison, prix à dix euros, écris à alex.martin+bip@example.co.uk.",
            ),
            (
                "RDV avec Mme Martin et M. Durand, Dr Lopez absent.",
                "rendez-vous avec Madame Martin et Monsieur Durand, Docteur Lopez absent.",
            ),
            ("Comptez env. 2 h, p. ex. le matin, c.-à-d. tôt.", "Comptez environ deux heures, par exemple le matin, c'est-à-dire tôt."),
            ("Pommes, poires, etc. Puis le dessert.", "Pommes, poires, et cetera, Puis le dessert."),
            (
                "Ligne n°4, Sainte-Anne & Ste Marie, ≈ 30 min, ≥ 5 km.",
                "Ligne numéro quatre, Sainte-Anne et Sainte Marie, environ trente minutes, au moins cinq kilomètres.",
            ),
            ("Wellness + Vie, 3 × 10 reps, PSG vs OM.", "ouèlnesse plus Vie, trois fois dix reps, PSG contre OM."),
            (
                "Ça prend ~15 min, soit ~ une heure en tout, ou 2~3 jours.",
                "Ça prend environ quinze minutes, soit environ une heure en tout, ou deux à trois jours.",
            ),
            ("Les runs Hermes et le cron du matin.", "Les reunes Hermes et le crone du matin."),
            ("Lance un Live avec Wellness, puis check tes emails.", "Lance un laïve avec ouèlnesse, puis tchèque tes emails."),
            ("Rien d'anglais dans « il court » ni « livret ».", "Rien d'anglais dans « il court » ni « livret »."),
            ("Entre 42-46% des cas.", "Entre quarante-deux à quarante-six pour cent des cas."),
            ("Entre 42–46 % des cas.", "Entre quarante-deux à quarante-six pour cent des cas."),
            ("De 9-12h", "De neuf à douze heures"),
            ("le 12-10-2026", "le douze octobre deux mille vingt-six"),
            ("le 2026-10-12", "le douze octobre deux mille vingt-six"),
            ("Il fait -5 °C", "Il fait moins cinq degrés"),
        ] {
            assert_eq!(prepare_for_synthesis(text), spoken, "{text}");
        }
    }
}
