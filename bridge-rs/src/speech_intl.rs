//! English, Spanish and German text preparation for speech (the French one is `numbers_fr`).
//!
//! Pocket TTS reads digits one by one and skips most symbols, so numbers, money, times, units and common
//! abbreviations are spelled out in the voice's language ("$5" -> "five dollars", "14:30" -> "vierzehn Uhr
//! dreißig", "3,5 %" -> "tres coma cinco por ciento"). Pauses (parentheses, dashes) and CamelCase names are
//! handled as in French. Anything not recognized is left untouched.
//!
//! Port of `bridge/bipbridge/speech_intl.py` (owner group: voice-speech). num2words (Python, LGPL, optional there)
//! is `voice_speech::num2words` here, always present; the patterns run on `voice_speech::pyre` (Python's `re`).

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::numbers_fr::{self as fr, ALONE, MENTION_AT, NOT_EMAIL_AT};
use crate::voice_speech::num2words::num2words;
use crate::voice_speech::pyre::{Match, Regex};
use crate::voice_speech::pystr;

pub const LANGUAGES: [&str; 3] = ["en", "es", "de"];

/// `spell(int(digits), lang, to)`: num2words' words (commas removed); the digits themselves from 10**12 on.
fn spell_digits(digits: &str, lang: &str, to: &str) -> String {
    let n = pystr::int_str(digits);
    if n.len() > 12 {
        return n;
    }
    match n.parse::<u64>().ok().and_then(|value| num2words(value, lang, to)) {
        Some(words) => words.replace(',', ""),
        None => n,
    }
}

fn spell(n: u64, lang: &str, to: &str) -> String {
    spell_digits(&n.to_string(), lang, to)
}

fn zero_word(lang: &str) -> &'static str {
    match lang {
        "en" => "zero",
        "es" => "cero",
        _ => "null",
    }
}

/// "0042" keeps its leading zeros, read one by one.
fn digits_words(digits: &str, lang: &str) -> String {
    let stripped = digits.trim_start_matches('0');
    let zeros = pystr::len(digits) - pystr::len(stripped);
    let zero = zero_word(lang);
    let mut words = vec![zero.to_string(); zeros];
    if !stripped.is_empty() {
        words.push(spell_digits(stripped, lang, "cardinal"));
    }
    let joined = words.join(" ");
    if joined.is_empty() {
        zero.to_string()
    } else {
        joined
    }
}

type Unit = (String, &'static str, &'static str);

/// Per language: units (pattern, plural, singular).
fn units(lang: &str) -> Vec<Unit> {
    let alone = ALONE;
    let list: Vec<(String, &str, &str)> = match lang {
        "en" => vec![
            (r"%".into(), "percent", "percent"),
            (r"€|EUR\b|euros?\b".into(), "euros", "euro"),
            (r"\$|USD\b|dollars?\b".into(), "dollars", "dollar"),
            (r"£|GBP\b".into(), "pounds", "pound"),
            (r"°C\b|° ?C\b".into(), "degrees Celsius", "degree Celsius"),
            (r"°F\b|° ?F\b".into(), "degrees Fahrenheit", "degree Fahrenheit"),
            (r"°".into(), "degrees", "degree"),
            (r"km/h\b".into(), "kilometers per hour", "kilometer per hour"),
            (r"mph\b".into(), "miles per hour", "mile per hour"),
            (r"km\b".into(), "kilometers", "kilometer"),
            (r"kg\b".into(), "kilograms", "kilogram"),
            (r"lbs?\b".into(), "pounds", "pound"),
            (r"cm\b".into(), "centimeters", "centimeter"),
            (r"mm\b".into(), "millimeters", "millimeter"),
            (r"m²".into(), "square meters", "square meter"),
            (r"km²".into(), "square kilometers", "square kilometer"),
            (r"mg\b".into(), "milligrams", "milligram"),
            (r"m[lL]\b".into(), "milliliters", "milliliter"),
            (r"kWh\b".into(), "kilowatt hours", "kilowatt hour"),
            (r"TB\b".into(), "terabytes", "terabyte"),
            (r"GB\b".into(), "gigabytes", "gigabyte"),
            (r"MB\b".into(), "megabytes", "megabyte"),
            (format!("g{alone}"), "grams", "gram"),
            (format!("L{alone}"), "liters", "liter"),
            (r"ms\b".into(), "milliseconds", "millisecond"),
            (r"min\b|minutes?\b".into(), "minutes", "minute"),
            (r"h\b|hours?\b".into(), "hours", "hour"),
            (r"bpm\b".into(), "beats per minute", "beat per minute"),
        ],
        "es" => vec![
            (r"%".into(), "por ciento", "por ciento"),
            (r"€|EUR\b|euros?\b".into(), "euros", "euro"),
            (r"\$|USD\b|dólar(?:es)?\b".into(), "dólares", "dólar"),
            (r"°C\b|° ?C\b|°".into(), "grados", "grado"),
            (r"km/h\b".into(), "kilómetros por hora", "kilómetro por hora"),
            (r"km²".into(), "kilómetros cuadrados", "kilómetro cuadrado"),
            (r"km\b".into(), "kilómetros", "kilómetro"),
            (r"kg\b".into(), "kilos", "kilo"),
            (r"kWh\b".into(), "kilovatios hora", "kilovatio hora"),
            (r"cm\b".into(), "centímetros", "centímetro"),
            (r"mm\b".into(), "milímetros", "milímetro"),
            (r"m²".into(), "metros cuadrados", "metro cuadrado"),
            (r"mg\b".into(), "miligramos", "miligramo"),
            (r"m[lL]\b".into(), "mililitros", "mililitro"),
            (r"TB\b".into(), "terabytes", "terabyte"),
            (r"GB\b".into(), "gigas", "giga"),
            (r"MB\b".into(), "megas", "mega"),
            (r"ms\b".into(), "milisegundos", "milisegundo"),
            (r"min\b|minutos?\b".into(), "minutos", "minuto"),
            (r"h\b|horas?\b".into(), "horas", "hora"),
            (r"lpm\b|bpm\b".into(), "pulsaciones por minuto", "pulsación por minuto"),
            (format!("g{alone}"), "gramos", "gramo"),
            (format!("[lL]{alone}"), "litros", "litro"),
            (format!("m{alone}"), "metros", "metro"),
        ],
        "de" => vec![
            (r"%".into(), "Prozent", "Prozent"),
            (r"€|EUR\b|Euro\b".into(), "Euro", "Euro"),
            (r"\$|USD\b|Dollar\b".into(), "Dollar", "Dollar"),
            (r"°C\b|° ?C\b|°".into(), "Grad", "Grad"),
            (r"km/h\b".into(), "Kilometer pro Stunde", "Kilometer pro Stunde"),
            (r"km²".into(), "Quadratkilometer", "Quadratkilometer"),
            (r"km\b".into(), "Kilometer", "Kilometer"),
            (r"kg\b".into(), "Kilogramm", "Kilogramm"),
            (r"kWh\b".into(), "Kilowattstunden", "Kilowattstunde"),
            (r"cm\b".into(), "Zentimeter", "Zentimeter"),
            (r"mm\b".into(), "Millimeter", "Millimeter"),
            (r"m²".into(), "Quadratmeter", "Quadratmeter"),
            (r"mg\b".into(), "Milligramm", "Milligramm"),
            (r"m[lL]\b".into(), "Milliliter", "Milliliter"),
            (r"TB\b".into(), "Terabyte", "Terabyte"),
            (r"GB\b".into(), "Gigabyte", "Gigabyte"),
            (r"MB\b".into(), "Megabyte", "Megabyte"),
            (r"ms\b".into(), "Millisekunden", "Millisekunde"),
            (r"min\b|Minuten?\b".into(), "Minuten", "Minute"),
            (r"h\b|Stunden?\b".into(), "Stunden", "Stunde"),
            (r"bpm\b".into(), "Schläge pro Minute", "Schlag pro Minute"),
            (format!("g{alone}"), "Gramm", "Gramm"),
            (format!("[lL]{alone}"), "Liter", "Liter"),
            (format!("m{alone}"), "Meter", "Meter"),
        ],
        _ => Vec::new(),
    };
    list
}

/// A language's units: each pattern compiled (for `re.fullmatch`) and the whole alternation after a number.
struct Units {
    each: Vec<(Regex, &'static str, &'static str)>,
    with_number: Regex,
}

static UNITS: LazyLock<HashMap<&'static str, Units>> = LazyLock::new(|| {
    LANGUAGES
        .iter()
        .map(|lang| {
            let list = units(lang);
            let alternation: Vec<&str> = list.iter().map(|(p, _, _)| p.as_str()).collect();
            let with_number = Regex::new(&format!(r"(?<![\w,.])(-?\d+(?:[,.]\d+)?) ?({})", alternation.join("|")));
            let each = list.iter().map(|(p, plural, singular)| (Regex::new(p), *plural, *singular)).collect();
            (*lang, Units { each, with_number })
        })
        .collect()
});

/// "1" before a singular unit: "one hour", "una hora", "eine Stunde".
fn one(lang: &str, word: &str) -> &'static str {
    match lang {
        "en" => "one",
        "es" => {
            if word == "hora" || word == "pulsación por minuto" {
                "una"
            } else {
                "un"
            }
        }
        _ => {
            if ["Stunde", "Minute", "Millisekunde", "Kilowattstunde"].contains(&word) {
                "eine"
            } else {
                "ein"
            }
        }
    }
}

const CURRENCIES: [&str; 6] = ["euros", "dollars", "pounds", "dólares", "Euro", "Dollar"];

/// Per language: decimal word, minus, "or", "to".
fn word(lang: &str, key: &str) -> &'static str {
    match (lang, key) {
        ("en", "point") => "point",
        ("en", "minus") => "minus",
        ("en", "or") => "or",
        ("en", "to") => "to",
        ("es", "point") => "coma",
        ("es", "minus") => "menos",
        ("es", "or") => "o",
        ("es", "to") => "a",
        ("de", "point") => "Komma",
        ("de", "minus") => "minus",
        ("de", "or") => "oder",
        ("de", "to") => "bis",
        _ => "",
    }
}

fn lexicon_source(lang: &str) -> Vec<(String, &'static str)> {
    let common = |and: &'static str,
                  plus: &'static str,
                  to: &'static str,
                  about: &'static str,
                  least: &'static str,
                  most: &'static str,
                  times: &'static str,
                  at: &'static str| {
        vec![
            (r"(?m)^[ \t]*(?:→|->|⟶|⇒|=>)[ \t]*".to_string(), ""),
            (r"\s*(?:→|->|⟶|⇒|=>)\s*".to_string(), ", "),
            (r"\s+&\s+".to_string(), and),
            (r"\s+\+\s+".to_string(), plus),
            (r"(?<=\d)\s?[~∼]\s?(?=\d)".to_string(), to),
            (r"\s*(?<!~)[~∼≈≃](?!~)\s*".to_string(), about),
            (r"(?<!\w)≥\s?".to_string(), least),
            (r"(?<!\w)≤\s?".to_string(), most),
            (r"(?<=\d)\s?×\s?(?=\d)".to_string(), times),
            (MENTION_AT.to_string(), ""),
            (NOT_EMAIL_AT.to_string(), at),
        ]
    };
    let mut rules = match lang {
        "en" => common(" and ", " plus ", " to ", " about ", "at least ", "at most ", " times ", " at "),
        "es" => common(" y ", " más ", " a ", " unos ", "al menos ", "como mucho ", " por ", " a "),
        "de" => common(" und ", " plus ", " bis ", " etwa ", "mindestens ", "höchstens ", " mal ", " at "),
        _ => return Vec::new(),
    };
    let own: &[(&str, &'static str)] = match lang {
        "en" => &[
            (r"\be\.g\.,?", "for example"),
            (r"\bi\.e\.,?", "that is"),
            (r"\betc\.(?=\s*\S)", "et cetera,"),
            (r"\betc\.", "et cetera."),
            (r"\bvs\.?(?= )", "versus"),
            (r"\bapprox\.", "approximately"),
            (r"\bDr\.(?= [A-Z])", "Doctor"),
            (r"\bMr\.(?= [A-Z])", "Mister"),
            (r"\bMrs\.(?= [A-Z])", "Missus"),
            (r"\bMs\.(?= [A-Z])", "Miz"),
            (r"\bNo\.(?= ?\d)", "number"),
            (r"#(?=\d)", "number "),
        ],
        "es" => &[
            (r"\bp\. ?ej\.", "por ejemplo"),
            (r"\betc\.(?=\s*\S)", "etcétera,"),
            (r"\betc\.", "etcétera."),
            (r"\baprox\.", "aproximadamente"),
            (r"\bSr\.(?= [A-ZÁÉÍÓÚÑ])", "señor"),
            (r"\bSra\.(?= [A-ZÁÉÍÓÚÑ])", "señora"),
            (r"\bDr\.(?= [A-ZÁÉÍÓÚÑ])", "doctor"),
            (r"\bDra\.(?= [A-ZÁÉÍÓÚÑ])", "doctora"),
            (r"\bUd\.", "usted"),
            (r"\bUds\.", "ustedes"),
            (r"\bpág\.", "página"),
            (r"\bn\.?º\s?", "número "),
        ],
        _ => &[
            (r"\bz\. ?B\.", "zum Beispiel"),
            (r"\bd\. ?h\.", "das heißt"),
            (r"\bu\. ?a\.", "unter anderem"),
            (r"\busw\.", "und so weiter"),
            (r"\bbzw\.", "beziehungsweise"),
            (r"\bggf\.", "gegebenenfalls"),
            (r"\bca\.", "circa"),
            (r"\bevtl\.", "eventuell"),
            (r"\binkl\.", "inklusive"),
            (r"\bvs\.?(?= )", "gegen"),
            (r"\bDr\.(?= [A-ZÄÖÜ])", "Doktor"),
            (r"\bNr\.(?= ?\d)", "Nummer"),
            (r"\bStr\.", "Straße"),
        ],
    };
    rules.extend(own.iter().map(|(p, w)| (p.to_string(), *w)));
    rules
}

static LEXICON: LazyLock<HashMap<&'static str, Vec<(Regex, &'static str)>>> = LazyLock::new(|| {
    LANGUAGES
        .iter()
        .map(|lang| (*lang, lexicon_source(lang).into_iter().map(|(p, w)| (Regex::new(&p), w)).collect()))
        .collect()
});

const MONTHS_DE: [&str; 12] = [
    "Januar",
    "Februar",
    "März",
    "April",
    "Mai",
    "Juni",
    "Juli",
    "August",
    "September",
    "Oktober",
    "November",
    "Dezember",
];
const MONTHS_EN: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_ES: [&str; 12] = [
    "enero",
    "febrero",
    "marzo",
    "abril",
    "mayo",
    "junio",
    "julio",
    "agosto",
    "septiembre",
    "octubre",
    "noviembre",
    "diciembre",
];

fn months(lang: &str) -> &'static [&'static str; 12] {
    match lang {
        "en" => &MONTHS_EN,
        "es" => &MONTHS_ES,
        _ => &MONTHS_DE,
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern)
}

static YEAR_CONTEXT_EN: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"\b(in|since|by|from|until|before|after|of|year|January|February|March|April|May|June|July|August|September|October|November|December) (1[1-9]\d\d|20\d\d)\b",
    )
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| re(r"(-?)(\d+)(?:[,.](\d+))?"));
static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d{4})-(\d{2})-(\d{2})\b"));
static TIME_COLON: LazyLock<Regex> = LazyLock::new(|| re(r"\b([01]?\d|2[0-3]):([0-5]\d)\b"));
static TIME_COLON_DE: LazyLock<Regex> = LazyLock::new(|| re(r"\b([01]?\d|2[0-3])[:.]([0-5]\d)(?: ?Uhr\b)?"));
// Python's class holds three plain spaces here (numbers_fr's holds the no-break ones too): kept as it is.
static GROUPED_SPACE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\d,.])(\d{1,3}(?:[   ]\d{3})+)(?![\d]|[,.]\d)"));
static GROUP_SPACES: LazyLock<Regex> = LazyLock::new(|| re(r"[   ]"));
static GROUPED_COMMA: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\d,.])(\d{1,3}(?:,\d{3})+)(?![\d]|[,.]\d)"));
static GROUPED_DOT: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\d,.])(\d{1,3}(?:\.\d{3})+)(?![\d]|[,.]\d)"));
static GROUP_MARKS: LazyLock<Regex> = LazyLock::new(|| re(r"[,.]"));
static CURRENCY_FIRST: LazyLock<Regex> = LazyLock::new(|| re(r"([$€£])\s?(-?\d+(?:[.,]\d+)?)"));
static DECIMAL: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])(-?\d+[,.]\d+)(?![\d.,]*\d)"));
static INTEGER: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])(\d+)(?![\w]|[.,]\d)"));
static NEGATIVE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w-])-(?=\d)"));
static ONE_BEFORE_NOUN: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])1 (?=([^\W\d]+))"));
static ORDINAL_EN: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)(st|nd|rd|th)\b"));
static ORDINAL_ES: LazyLock<Regex> = LazyLock::new(|| re(r"\b(\d+)\.?([ºª])"));
static ORDINAL_DE_DATE: LazyLock<Regex> =
    LazyLock::new(|| re(&format!(r"\b([1-9]|[12]\d|3[01])\. (?={})", MONTHS_DE.join("|"))));
static YEAR_DE: LazyLock<Regex> = LazyLock::new(|| re(r"(?<![\w.,])(1[1-9]\d\d)(?![\w]|[.,]\d)"));
static FINAL_O: LazyLock<Regex> = LazyLock::new(|| re(r"o$"));
static FIRST_THIRD_ES: LazyLock<Regex> = LazyLock::new(|| re(r"\b(prim|terc)ero (?=[^\W\d])"));
static FEMININE_ES: LazyLock<Regex> =
    LazyLock::new(|| re(r"^(?!(?:día|mapa|problema|programa|sistema|tema|idioma|clima|planeta)$).*a$"));
static FEMININE_DE: LazyLock<Regex> = LazyLock::new(|| re(r"e$"));
static MONEY_SPLIT: LazyLock<Regex> = LazyLock::new(|| re(r"(.*) (.*)"));
static SLASH: LazyLock<Regex> = LazyLock::new(|| re(r"(?<=[^\W\d]) ?/ ?(?=[^\W\d])"));

/// "12", "3.5" or "3,5" in words (the decimal separator is accepted either way).
fn number(value: &str, lang: &str) -> String {
    let Some(groups) = NUMBER.fullmatch(value) else {
        return value.to_string();
    };
    let sign = groups[1].clone().unwrap_or_default();
    let whole = groups[2].clone().unwrap_or_default();
    let mut words = spell_digits(&whole, lang, "cardinal");
    if let Some(fraction) = groups[3].as_deref().filter(|f| !f.is_empty()) {
        if lang == "es" && pystr::len(fraction) <= 2 && !fraction.starts_with('0') {
            words += &format!(" coma {}", spell_digits(fraction, lang, "cardinal"));
        } else {
            let digits: Vec<String> =
                fraction.chars().map(|d| spell_digits(&d.to_string(), lang, "cardinal")).collect();
            words += &format!(" {} {}", word(lang, "point"), digits.join(" "));
        }
    }
    if sign.is_empty() {
        words
    } else {
        format!("{} {words}", word(lang, "minus"))
    }
}

fn with_unit(lang: &str, value: &str, symbol: &str) -> String {
    let units = &UNITS[lang];
    let Some((_, plural, singular)) = units.each.iter().find(|(pattern, _, _)| pattern.fullmatch(symbol).is_some())
    else {
        // Python's next() would raise StopIteration here; every symbol the alternation matched has its unit
        return format!("{value} {symbol}");
    };
    let normalized = value.replace(',', ".");
    let (whole, cents) = match normalized.split_once('.') {
        Some((w, c)) => (w.to_string(), c.to_string()),
        None => (normalized.clone(), String::new()),
    };
    if CURRENCIES.contains(plural) && pystr::len(&cents) == 2 {
        let joined = format!("{whole} {symbol}");
        let parts = MONEY_SPLIT.captures(&joined).unwrap_or_default();
        let (w, s) =
            (parts.get(1).cloned().flatten().unwrap_or_default(), parts.get(2).cloned().flatten().unwrap_or_default());
        let mut money = with_unit(lang, &w, &s);
        if pystr::int_str(&cents) != "0" {
            let spoken = spell_digits(&cents, lang, "cardinal");
            money += &match lang {
                "en" => format!(" and {spoken} cents"),
                "es" => format!(" con {spoken}"),
                _ => format!(" {spoken}"),
            };
        }
        return money;
    }
    if value.trim_start_matches('-') == "1" {
        return format!("{} {singular}", one(lang, singular));
    }
    format!("{} {plural}", number(value, lang))
}

fn time(lang: &str, m: &Match) -> String {
    let h = pystr::int_value(&m.group(1)).unwrap_or(0);
    let minutes = pystr::int_value(&m.group(2)).unwrap_or(0);
    let hours = if lang == "de" && h == 1 { "ein".to_string() } else { spell(h, lang, "cardinal") };
    match lang {
        "en" => {
            if minutes == 0 {
                return if h <= 12 { format!("{hours} o'clock") } else { format!("{hours} hundred") };
            }
            let spoken = spell(minutes, lang, "cardinal");
            if minutes < 10 {
                format!("{hours} oh {spoken}")
            } else {
                format!("{hours} {spoken}")
            }
        }
        "es" => {
            if minutes == 0 {
                format!("{hours} en punto")
            } else {
                format!("{hours} y {}", spell(minutes, lang, "cardinal"))
            }
        }
        _ => {
            if minutes == 0 {
                format!("{hours} Uhr")
            } else {
                format!("{hours} Uhr {}", spell(minutes, lang, "cardinal"))
            }
        }
    }
}

/// "2026-10-12" -> "October twelfth, twenty twenty-six", "12 de octubre de 2026", "12. Oktober 2026" (the later
/// steps spell the Spanish and German numbers).
fn iso_date(m: &Match, lang: &str) -> String {
    let year = pystr::int_value(&m.group(1)).unwrap_or(0);
    let month = pystr::int_value(&m.group(2)).unwrap_or(0);
    let day = pystr::int_value(&m.group(3)).unwrap_or(0);
    if !((1..=12).contains(&month) && (1..=31).contains(&day)) {
        return m.whole();
    }
    let name = months(lang)[month as usize - 1];
    match lang {
        "en" => format!("{name} {}, {}", spell(day, lang, "ordinal"), spell(year, lang, "year")),
        "es" => format!("{day} de {name} de {year}"),
        _ => format!("{day}. {name} {}", spell(year, lang, "year")),
    }
}

pub fn spell_numbers(text: &str, lang: &str) -> String {
    if !LANGUAGES.contains(&lang) || !pystr::has_digit(text) {
        return text.to_string();
    }
    let mut text = text.to_string();
    if lang == "de" {
        text = YEAR_DE.replace_with(&text, |m| spell_digits(&m.group(1), lang, "year"));
    }
    text = GROUPED_SPACE.replace_with(&text, |m| GROUP_SPACES.replace(&m.group(1), ""));
    let grouped = if lang == "en" { &*GROUPED_COMMA } else { &*GROUPED_DOT };
    text = grouped.replace_with(&text, |m| GROUP_MARKS.replace(&m.group(1), ""));
    text = CURRENCY_FIRST.replace_with(&text, |m| format!("{} {}", m.group(2), m.group(1))); // "$5" -> "5 $" -> "five dollars"
    text = ISO_DATE.replace_with(&text, |m| iso_date(m, lang)); // "2026-10-12": a date, not a range
    text = fr::RANGE.replace(&text, &format!(" {} ", word(lang, "to"))); // "42-46 %" -> "42 to 46 %", not one hyphenated word
    if lang == "en" {
        text = ORDINAL_EN.replace_with(&text, |m| spell_digits(&m.group(1), lang, "ordinal"));
        text = YEAR_CONTEXT_EN
            .replace_with(&text, |m| format!("{} {}", m.group(1), spell_digits(&m.group(2), lang, "year")));
    } else if lang == "es" {
        text = ORDINAL_ES.replace_with(&text, |m| {
            let ordinal = spell_digits(&m.group(1), lang, "ordinal");
            if m.group(2) == "º" {
                ordinal
            } else {
                FINAL_O.replace(&ordinal, "a")
            }
        });
        text = FIRST_THIRD_ES.replace(&text, r"\1er "); // "tercer día"
    } else {
        text = ORDINAL_DE_DATE.replace_with(&text, |m| spell_digits(&m.group(1), lang, "ordinal") + "n ");
    }
    let times = if lang == "de" { &*TIME_COLON_DE } else { &*TIME_COLON };
    text = times.replace_with(&text, |m| time(lang, m));
    text = UNITS[lang].with_number.replace_with(&text, |m| with_unit(lang, &m.group(1), &m.group(2)));
    text = DECIMAL.replace_with(&text, |m| number(&m.group(1), lang));
    text = NEGATIVE.replace(&text, &format!("{} ", word(lang, "minus")));
    if lang == "es" || lang == "de" {
        // "1 día" -> "un día", "1 Woche" -> "eine Woche" (gender guessed from the ending)
        text = ONE_BEFORE_NOUN.replace_with(&text, |m| {
            let noun = m.group(1);
            let article = if lang == "es" {
                one(lang, if FEMININE_ES.is_match(&noun) { "hora" } else { "" })
            } else if FEMININE_DE.is_match(&noun) {
                "eine"
            } else {
                "ein"
            };
            format!("{article} ")
        });
    }
    INTEGER.replace_with(&text, |m| {
        let digits = m.group(1);
        if digits.starts_with('0') && pystr::len(&digits) > 1 {
            digits_words(&digits, lang)
        } else {
            spell_digits(&digits, lang, "cardinal")
        }
    })
}

/// Same last touch as the French one, in `lang` (`en`, `es` or `de`).
pub fn prepare_for_synthesis(text: &str, lang: &str) -> String {
    let mut text = text.to_string();
    if let Some(rules) = LEXICON.get(lang) {
        for (pattern, words) in rules {
            text = pattern.replace(&text, words);
        }
    }
    text = spell_numbers(&text, lang);
    if LANGUAGES.contains(&lang) {
        text = SLASH.replace(&text, &format!(" {} ", word(lang, "or")));
    }
    fr::pauses(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_for_synthesis_cases() {
        for (lang, text, spoken) in [
            ("en", "Bed at 11:15, 64 °F.", "Bed at eleven fifteen, sixty-four degrees Fahrenheit."),
            (
                "en",
                "About 1 mm, 250 g of flour, 1 L of water, 2 GB, the 1990s. Rain -> cancel.",
                "About one millimeter, two hundred and fifty grams of flour, one liter of water, two gigabytes, the 1990s. Rain, cancel.",
            ),
            (
                "es",
                "3 m de alto, 250 g de harina, 1 l de agua. Lluvia -> cancelado.",
                "tres metros de alto, doscientos cincuenta gramos de harina, un litro de agua. Lluvia, cancelado.",
            ),
            ("de", "Etwa 1 mm, 250 g Mehl, 1 kWh.\n-> Regen", "Etwa ein Millimeter, zweihundertfünfzig Gramm Mehl, eine Kilowattstunde.\nRegen"),
            ("en", "Meet @ the station with @Bob, mail bob@example.com", "Meet at the station with Bob, mail bob@example.com"),
            (
                "en",
                "You spent $420.50 (12% less) in 2026.",
                "You spent four hundred and twenty dollars and fifty cents, twelve percent less, in twenty twenty-six.",
            ),
            ("en", "1,250 steps vs 1 h on the 3rd day.", "one thousand two hundred and fifty steps versus one hour on the third day."),
            ("en", "Garmin/Oura e.g. OpenAI", "Garmin or Oura for example Open AI"),
            ("es", "A las 23:15, 18 °C.", "A las veintitrés y quince, dieciocho grados."),
            ("es", "Gastaste 420,50 € el 3º día.", "Gastaste cuatrocientos veinte euros con cincuenta el tercer día."),
            ("es", "1 día, 1 semana, 1.250 pasos, 3,5 km.", "un día, una semana, mil doscientos cincuenta pasos, tres coma cinco kilómetros."),
            ("de", "Um 23:15 Uhr, 18 °C.", "Um dreiundzwanzig Uhr fünfzehn, achtzehn Grad."),
            ("de", "420,50 € am 3. Oktober 2026.", "vierhundertzwanzig Euro fünfzig am dritten Oktober zweitausendsechsundzwanzig."),
            ("en", "Due 2026-10-12.", "Due October twelfth, twenty twenty-six."),
            ("es", "Para el 2026-10-12.", "Para el doce de octubre de dos mil veintiséis."),
            ("de", "Bis 2026-10-12.", "Bis zwölften Oktober zweitausendsechsundzwanzig."),
            (
                "de",
                "1 Tag, 1 Woche, 1.250 Schritte, z. B. 1999",
                "ein Tag, eine Woche, eintausendzweihundertfünfzig Schritte, zum Beispiel neunzehnhundertneunundneunzig",
            ),
        ] {
            assert_eq!(prepare_for_synthesis(text, lang), spoken, "{lang}: {text}");
        }
    }
}
