//! Names, acronyms and English words written the way a French, German, Spanish or Dutch voice can read them.
//!
//! Pocket TTS has one grapheme model per language and no phonemes. Its 3.3.0 voices read « Spotify » with French
//! spelling rules; the 4.x ones (retrained fr/de/es weights) say brands and anglicisms right from the raw text, and
//! respellings made for 3.3.0 bring nothing there or make things worse (listening test, 2026-10-25). So there are two
//! ways (`Spelling`):
//!
//! - `Raw` (Pocket ≥ 4, the default, and every other engine): only the rules about reading: the short exception
//!   list (`always` in the lexicon: words 4.x still says wrong, filled from listening tests only: « Garmin » ->
//!   « Garmine », « deadline » -> « dèd-laïne »), letters glued to digits cut, acronyms spelled (« PDF », « GPS »:
//!   4.x misreads them in German and Spanish too), unless the lexicon says them as a word (NASA, PIN, OK). Names
//!   and English words are left as written, CamelCase included.
//! - `Respelled` (Pocket 3.3.0: the server's `[pocket] engine = "3.3.0"`, a phone that sends its old package's
//!   version): everything below.
//!
//! In `Respelled`, before numbers are spelled, each word of the sentence goes through, in this order:
//!
//! 0. the exception list, as in `Raw`;
//! 1. the lexicon (`assets/pronunciation/<lang>.json`, the same files the apps can use offline): whole words or
//!    phrases, any case (`words`), or case kept (`exact`: « Notion » the app, not « une notion »); a plural adds « s »
//!    to a listed singular (« meetings ») unless the word is a frequent one of the language's own (« pays »);
//! 2. letters glued to digits are cut (« PS5 » -> « P S 5 » read « pé èsse cinq », « iPhone16 »); digits first
//!    (« 14h30 », « 1er », « 7j ») are the numbers' business;
//! 3. CamelCase names not in the lexicon are cut and each part read on its own (« DuckDuckGo » -> « Deuk Deuk Go »);
//! 4. acronyms in capitals are spelled with the language's letter names (« SNCF » -> « èsse ène cé èffe »), unless
//!    the lexicon reads them as a word (NASA, OTAN) or they are a word of the language in capitals (« MERCI »);
//! 5. the detector: a word that is not one of the language's own (`common_<lang>.txt`) but an English one
//!    (`english.txt`, from CMUdict) and looks English (« th », « w », « oo », « -ing »...) is respelled from its
//!    English pronunciation with a small table per language (`francise`). Only when both lists agree: a word of the
//!    sentence's language is never touched.
//!
//! Units after a number (« 2 GB », « 5 km »), e-mail addresses, links and @handles are left to the rules after this
//! one. English (`en`) only gets the lexicon and the CamelCase cut: its voice reads English. Dutch (`nl`) only ever
//! had Pocket 4 voices: always `Raw` (its lexicon holds letter names, acronyms said as words and exceptions).

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

const SOURCES: [(&str, &str); 5] = [
    ("fr", include_str!("../assets/pronunciation/fr.json")),
    ("nl", include_str!("../assets/pronunciation/nl.json")),
    ("de", include_str!("../assets/pronunciation/de.json")),
    ("es", include_str!("../assets/pronunciation/es.json")),
    ("en", include_str!("../assets/pronunciation/en.json")),
];
const ENGLISH_TXT: &str = include_str!("../assets/pronunciation/english.txt");
const COMMON_TXT: [(&str, &str); 3] = [
    ("fr", include_str!("../assets/pronunciation/common_fr.txt")),
    ("de", include_str!("../assets/pronunciation/common_de.txt")),
    ("es", include_str!("../assets/pronunciation/common_es.txt")),
];

/// The lexicon file of a language as it is embedded (for the apps' offline copy).
pub fn lexicon_json(lang: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(l, _)| *l == lang).map(|(_, json)| *json)
}

#[derive(Deserialize)]
struct LexiconFile {
    #[serde(default)]
    letters: HashMap<String, String>,
    #[serde(default)]
    exact: HashMap<String, String>,
    #[serde(default)]
    words: HashMap<String, String>,
    #[serde(default)]
    always: HashMap<String, String>,
}

/// How much of the text is rewritten for the voice (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
    /// Pocket ≥ 4 and the other engines: reading rules and the exception list only.
    Raw,
    /// Pocket 3.3.0: names and English words respelled too.
    Respelled,
}

impl Spelling {
    /// From a Pocket version (« 3.3.0 », « 4.0.1 », « 4 »): `Respelled` below 4, `Raw` from 4 on and when unknown
    /// (no version given, or not a number).
    pub fn for_engine(version: Option<&str>) -> Spelling {
        let major =
            version.and_then(|v| v.trim().trim_start_matches(['v', 'V']).split('.').next()?.parse::<u32>().ok());
        match major {
            Some(major) if major < 4 => Spelling::Respelled,
            _ => Spelling::Raw,
        }
    }
}

struct Lexicon {
    letters: HashMap<char, String>,
    exact: HashMap<String, String>,
    /// lower-case keys; words of a phrase separated by one space or hyphen
    words: HashMap<String, String>,
    /// words the current engine (Pocket 4.x) still says wrong, applied in both ways; lower-case keys
    always: HashMap<String, String>,
    /// the most words in a key
    longest: usize,
}

static LEXICONS: LazyLock<HashMap<&'static str, Lexicon>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|(lang, json)| {
            let file: LexiconFile = serde_json::from_str(json).expect("pronunciation lexicon");
            let words: HashMap<String, String> = file.words.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
            let longest = words.keys().map(|k| k.split([' ', '-']).count()).max().unwrap_or(1);
            let letters = file.letters.into_iter().filter_map(|(k, v)| Some((k.chars().next()?, v))).collect();
            let always = file.always.into_iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
            (*lang, Lexicon { letters, exact: file.exact, words, always, longest })
        })
        .collect()
});

/// English words -> their CMUdict pronunciation (ARPAbet, stress digits kept).
static ENGLISH: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| ENGLISH_TXT.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(' ')).collect());

/// Per language: its own frequent words -> their rank (1 = the most frequent).
static COMMON: LazyLock<HashMap<&'static str, HashMap<&'static str, u32>>> = LazyLock::new(|| {
    COMMON_TXT
        .iter()
        .map(|(lang, txt)| {
            let ranks = txt.lines().filter(|l| !l.starts_with('#')).zip(1u32..).collect();
            (*lang, ranks)
        })
        .collect()
});

/// Unit symbols the numbers' rules read after a number: left as they are there (« 2 GB », « 1 kWh »).
const UNITS: &[&str] = &[
    "EUR", "USD", "GBP", "km", "kg", "kWh", "kW", "cm", "mm", "ms", "mg", "ml", "mL", "min", "bpm", "lpm", "cl", "cL",
    "To", "Go", "Mo", "Ko", "TB", "GB", "MB", "KB", "g", "l", "L", "m", "W", "h", "H", "mph", "lb", "lbs", "C", "F",
];

/// A language's own word (not an English one): `rank` at most `top` when given.
fn native(lang: &str, lower: &str, top: Option<u32>) -> bool {
    match COMMON.get(lang).and_then(|ranks| ranks.get(lower)) {
        Some(rank) => top.is_none_or(|top| *rank <= top),
        None => false,
    }
}

struct Token {
    start: usize,
    end: usize,
    /// part of an e-mail address, a link, a file name or a handle: never touched
    protected: bool,
}

/// A whitespace-delimited chunk that is an address, a link, a handle, code (« `sleep_score` ») or a dotted name
/// (« example.com »).
fn protected_chunk(chunk: &str) -> bool {
    if chunk.contains(['@', '_', '`']) || chunk.contains("://") || chunk.to_lowercase().starts_with("www.") {
        return true;
    }
    let chars: Vec<char> = chunk.chars().collect();
    chars.windows(3).any(|w| w[0].is_alphanumeric() && w[1] == '.' && w[2].is_alphabetic())
}

fn tokens(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut chunk_protected = false;
    let mut start: Option<usize> = None;
    let mut new_chunk = true;
    for (i, c) in text.char_indices() {
        if new_chunk && !c.is_whitespace() {
            let end = text[i..].find(char::is_whitespace).map_or(text.len(), |e| i + e);
            chunk_protected = protected_chunk(&text[i..end]);
            new_chunk = false;
        }
        if c.is_alphanumeric() {
            start.get_or_insert(i);
        } else {
            if let Some(s) = start.take() {
                out.push(Token { start: s, end: i, protected: chunk_protected });
            }
            if c.is_whitespace() {
                new_chunk = true;
            }
        }
    }
    if let Some(s) = start {
        out.push(Token { start: s, end: text.len(), protected: chunk_protected });
    }
    out
}

/// The capital of the word kept on its respelling (« Meeting à 10 h » -> « Mitingue »).
fn follow_case(word: &str, spoken: &str) -> String {
    let upper = word.chars().next().is_some_and(char::is_uppercase);
    let mut chars = spoken.chars();
    match chars.next() {
        Some(first) if upper && first.is_lowercase() => first.to_uppercase().chain(chars).collect(),
        _ => spoken.to_string(),
    }
}

/// Where the sentence starts, a capital says nothing about the word.
fn at_sentence_start(before: &str) -> bool {
    match before.trim_end_matches([' ', '\t', '«', '"', '“', '(', '\u{a0}']).chars().last() {
        None => true,
        Some(c) => matches!(c, '.' | '!' | '?' | ':' | '\n' | '…' | '-' | '—' | '–' | '•' | '*'),
    }
}

/// Small Roman numerals (« Louis XIV », « tome II »): not acronyms.
fn roman(word: &str) -> bool {
    word.chars().all(|c| matches!(c, 'I' | 'V' | 'X'))
}

/// The word respelled, or None to leave it as it is.
fn respell_word(
    word: &str,
    lang: &str,
    lexicon: &Lexicon,
    spelling: Spelling,
    after_number: bool,
    sentence_start: bool,
) -> Option<String> {
    let first = word.chars().next()?;
    if first.is_numeric() || word.chars().any(|c| c.is_numeric() && !c.is_ascii_digit()) {
        return None; // « 14h30 », « 1er », « 4G »: the numbers' rules
    }
    if after_number && UNITS.contains(&word) {
        return None;
    }
    let lower = word.to_lowercase();
    let plural_stem =
        lower.strip_suffix('s').filter(|stem| stem.chars().count() >= 3 && !native(lang, &lower, Some(5000)));
    let lookup = |table: &HashMap<String, String>| {
        if let Some(spoken) = table.get(&lower) {
            return Some(follow_case(word, spoken));
        }
        plural_stem.and_then(|stem| table.get(stem)).map(|spoken| follow_case(word, spoken) + "s")
    };
    if let Some(spoken) = lookup(&lexicon.always) {
        return Some(spoken);
    }
    let respelled = spelling == Spelling::Respelled;
    let capitals = word.chars().count() >= 2 && word.chars().all(char::is_uppercase);
    if let Some(spoken) = lexicon.exact.get(word) {
        // raw: an acronym the lexicon says as a word (« PIN ») is left to the voice, not spelled
        return if respelled { Some(spoken.clone()) } else { None };
    }
    if let Some(spoken) = lookup(&lexicon.words) {
        // raw: the voice says it from the written word, « NASA » or « OK » included (not spelled)
        return respelled.then_some(spoken);
    }
    // letters glued to digits: « PS5 », « A320 », « iPhone16 »
    if word.chars().any(|c| c.is_ascii_digit()) {
        let mut parts: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut digits = false;
        for c in word.chars() {
            if !current.is_empty() && c.is_ascii_digit() != digits {
                parts.push(std::mem::take(&mut current));
            }
            digits = c.is_ascii_digit();
            current.push(c);
        }
        parts.push(current);
        let spoken: Vec<String> = parts
            .iter()
            .map(|part| {
                if part.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                    part.clone()
                } else {
                    respell_word(part, lang, lexicon, spelling, false, sentence_start).unwrap_or_else(|| part.clone())
                }
            })
            .collect();
        return Some(spoken.join(" "));
    }
    let parts = if respelled { camel_parts(word) } else { vec![word] };
    if parts.len() > 1 {
        let spoken: Vec<String> = parts
            .iter()
            .map(|part| {
                respell_word(part, lang, lexicon, spelling, false, sentence_start).unwrap_or_else(|| part.to_string())
            })
            .collect();
        return Some(spoken.join(" "));
    }
    let count = word.chars().count();
    if capitals {
        if lexicon.letters.is_empty() || roman(word) {
            return None;
        }
        // a word in capitals (« MERCI », « OUI ») is not an acronym; short ones only when very common
        if native(lang, &lower, if count >= 4 { None } else { Some(1000) }) {
            return None;
        }
        let letters: Vec<String> =
            lower.chars().map(|c| lexicon.letters.get(&c).cloned().unwrap_or_else(|| c.to_string())).collect();
        return Some(letters.join(" "));
    }
    if lang == "en" || !respelled {
        return None;
    }
    let capital = first.is_uppercase() && !sentence_start;
    let phones = detect(&lower, lang, capital)?;
    Some(follow_case(word, &francise(phones, &lower, lang)))
}

/// « DuckDuckGo » -> Duck, Duck, Go; « OpenAI » -> Open, AI; « HTMLParser » -> HTML, Parser. Only names that start
/// with a capital (« iPhone », « eBay » are the lexicon's).
fn camel_parts(word: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = word.char_indices().collect();
    if !chars.first().is_some_and(|(_, c)| c.is_uppercase()) || !chars.iter().any(|(_, c)| c.is_lowercase()) {
        return vec![word];
    }
    let mut cuts = Vec::new();
    for k in 1..chars.len() {
        let (prev, here) = (chars[k - 1].1, chars[k].1);
        let next_lower = chars.get(k + 1).is_some_and(|(_, c)| c.is_lowercase());
        if (prev.is_lowercase() && here.is_uppercase()) || (prev.is_uppercase() && here.is_uppercase() && next_lower) {
            cuts.push(chars[k].0);
        }
    }
    if cuts.is_empty() {
        return vec![word];
    }
    let mut parts = Vec::new();
    let mut from = 0;
    for cut in cuts {
        parts.push(&word[from..cut]);
        from = cut;
    }
    parts.push(&word[from..]);
    parts
}

const CONSONANTS: &str = "bcdfghjklmnpqrstvwxz";

/// How English the word's letters look for a speaker of `lang` (each clue 1 or 0.5).
pub fn english_score(lower: &str, lang: &str) -> f32 {
    let has = |p: &str| lower.contains(p);
    let ends_consonant_y = {
        let mut rev = lower.chars().rev();
        rev.next() == Some('y') && rev.next().is_some_and(|c| CONSONANTS.contains(c))
    };
    let mut score: f32 = 0.0;
    for pattern in ["th", "oo", "ea", "igh", "ow", "oa"] {
        if has(pattern) {
            score += 1.0;
        }
    }
    if ends_consonant_y {
        score += 1.0;
    }
    match lang {
        "fr" => {
            for pattern in ["w", "ee", "sh", "ck"] {
                if has(pattern) {
                    score += 1.0;
                }
            }
            if lower.ends_with("ing") {
                score += 1.0;
            }
            for pattern in ["k", "ay"] {
                if has(pattern) {
                    score += 0.5;
                }
            }
        }
        "de" => {
            if lower.replace("sch", "").contains("sh") {
                score += 1.0;
            }
            if lower.chars().zip(lower.chars().skip(1).chain([' '])).any(|(c, n)| c == 'c' && n != 'h' && n != 'k') {
                score += 1.0; // German writes « c » only in « ch », « ck » (and loanwords)
            }
            for pattern in ["ee", "w", "j", "ay"] {
                if has(pattern) {
                    score += 0.5;
                }
            }
            if lower.ends_with("ing") {
                score += 0.5;
            }
        }
        _ => {
            for pattern in ["w", "ee", "sh", "ck", "k"] {
                if has(pattern) {
                    score += 1.0;
                }
            }
            if lower.ends_with("ing") {
                score += 1.0;
            }
            if ["ss", "tt", "ff", "pp", "dd", "gg", "bb", "mm", "zz"].iter().any(|p| has(p)) {
                score += 1.0; // Spanish doubles only c, l, r (and n)
            }
            if lower.chars().last().is_some_and(|c| "tkpbgmfvc".contains(c)) {
                score += 0.5; // Spanish words end in a vowel, n, s, r, l, d, z, j or y
            }
        }
    }
    score
}

/// The English pronunciation of a word that is English and not the language's own, when it looks English enough:
/// one clue for a lower-case word, more for a capitalized one in the middle of a sentence (more often a person's
/// name: « Kevin », « William »). German capitalizes every noun, so a capital says nothing there.
fn detect(lower: &str, lang: &str, capital: bool) -> Option<&'static str> {
    if lower.chars().count() < 3 || !lower.chars().all(|c| c.is_ascii_lowercase()) || native(lang, lower, None) {
        return None;
    }
    let phones = ENGLISH.get(lower)?;
    let needed = if capital && lang != "de" { 1.5 } else { 1.0 };
    (english_score(lower, lang) >= needed).then_some(*phones)
}

/// Whether `word` would be respelled by the detector (not the lexicon), for the tests and the bench.
pub fn detected(word: &str, lang: &str) -> bool {
    detect(&word.to_lowercase(), lang, false).is_some()
}

const VOWELS: [&str; 15] = ["AA", "AE", "AH", "AO", "AW", "AY", "EH", "ER", "EY", "IH", "IY", "OW", "OY", "UH", "UW"];

/// ARPAbet phoneme -> letters a speaker of `lang` reads as that sound (before the context rules of `francise`).
fn grapheme(lang: &str, phone: &str) -> &'static str {
    match lang {
        "fr" => match phone {
            "AA" | "AE" => "a",
            "AH" => "eu",
            "AO" | "OW" => "o",
            "AW" => "aou",
            "AY" => "aï",
            "EH" | "EY" => "è",
            "ER" => "eur",
            "IH" | "IY" => "i",
            "OY" => "oï",
            "UH" | "UW" | "W" => "ou",
            "CH" => "tch",
            "DH" => "z",
            "JH" => "dj",
            "NG" => "ng",
            "SH" => "ch",
            "TH" => "s",
            "ZH" => "j",
            "HH" => "h",
            _ => consonant(phone),
        },
        "de" => match phone {
            "AA" | "AH" => "a",
            "AE" => "ä",
            "AO" => "o",
            "OW" => "ou",
            "AW" => "au",
            "AY" => "ei",
            "EH" => "e",
            "EY" => "ej",
            "ER" => "ör",
            "IH" => "i",
            "IY" => "ie",
            "OY" => "eu",
            "UH" => "u",
            "UW" => "uh",
            "CH" => "tsch",
            "DH" => "d",
            "JH" => "dsch",
            "NG" => "ng",
            "SH" | "ZH" => "sch",
            "TH" | "Z" => "s",
            "V" => "w",
            "W" => "w",
            "Y" => "j",
            "HH" => "h",
            _ => consonant(phone),
        },
        _ => match phone {
            "AA" | "AE" | "AH" => "a",
            "AO" => "o",
            "OW" => "ou",
            "AW" => "au",
            "AY" => "ai",
            "EH" => "e",
            "EY" => "ei",
            "ER" => "er",
            "IH" | "IY" => "i",
            "OY" => "oi",
            "UH" | "UW" | "W" => "u",
            "CH" => "ch",
            "DH" => "d",
            "JH" | "ZH" => "y",
            "NG" => "n",
            "SH" => "sh",
            "TH" => "z",
            "V" => "b",
            "Z" => "s",
            "HH" => "j",
            _ => consonant(phone),
        },
    }
}

fn consonant(phone: &str) -> &'static str {
    match phone {
        "B" => "b",
        "D" => "d",
        "F" => "f",
        "G" => "g",
        "K" => "k",
        "L" => "l",
        "M" => "m",
        "N" => "n",
        "P" => "p",
        "R" => "r",
        "S" => "s",
        "T" => "t",
        "V" => "v",
        "Y" => "y",
        "Z" => "z",
        _ => "",
    }
}

/// An English pronunciation (CMUdict ARPAbet) written so the language's voice says it close to English:
/// « bluetooth » B L UW1 T UW2 TH -> « bloutousse » in French, « bluhtuhs » in German, « blutuz » in Spanish.
///
/// One grapheme per phoneme (`grapheme`), then a few context rules per language:
/// - all: an English /ɑ/ written « o » (« boss », « shopping ») stays « o », as Europeans say it;
/// - fr: a schwa is « e », « è » in the last syllable before a consonant (« tokène »), « a » at the very end; a
///   final consonant French would leave silent gets an « e » (« claoude », « reune », « -sse », « -ngue »); « n »/« m »
///   doubled after a vowel before a consonant so no nasal vowel is read (« ouikènnde »); « s » between vowels is
///   « ss »; « g » before e/i is « gu »; a final /eɪ/ is « é »;
/// - de: « s » before « t »/« p » at the start is « ss » (else « scht »: « Sstriem »); a final /i/ is « i »; a final
///   unstressed /ɚ/ is « er »;
/// - es: « s » + consonant at the start takes an « e » (« Espotifai »); /k/ is « qu » before e/i, else « c »; « g »
///   before e/i is « gu »; /w/ before a at the start is « gu » (« guasap »); a final schwa is « a ».
pub fn francise(phones: &str, spelling: &str, lang: &str) -> String {
    let parsed: Vec<(&str, char)> = phones
        .split_whitespace()
        .map(|p| {
            let stress = p.chars().last().filter(char::is_ascii_digit).unwrap_or(' ');
            (p.trim_end_matches(|c: char| c.is_ascii_digit()), stress)
        })
        .collect();
    let is_vowel = |p: Option<&str>| p.is_some_and(|p| VOWELS.contains(&p));
    let written_o = spelling.contains('o') && !spelling.contains('a');
    let mut out = String::new();
    let n = parsed.len();
    for (i, (phone, stress)) in parsed.iter().enumerate() {
        let (phone, stress) = (*phone, *stress);
        let prev = if i > 0 { Some(parsed[i - 1].0) } else { None };
        let next = parsed.get(i + 1).map(|(p, _)| *p);
        let last = i + 1 == n;
        let only_consonants_after = parsed[i + 1..].iter().all(|(p, _)| !VOWELS.contains(p));
        let mut g: String = grapheme(lang, phone).to_string();
        if phone == "AA" && written_o {
            g = "o".into();
        }
        match lang {
            "fr" => {
                if phone == "AH" && stress == '0' {
                    g = if last {
                        "a".into()
                    } else if only_consonants_after {
                        "è".into()
                    } else {
                        "e".into()
                    };
                }
                if phone == "EY" && last {
                    g = "é".into();
                }
                if phone == "S" && is_vowel(prev) && is_vowel(next) {
                    g = "ss".into();
                }
                if phone == "G" && matches!(next, Some("IH" | "IY" | "EH" | "EY" | "AH" | "ER" | "Y")) {
                    g = "gu".into();
                }
                if matches!(phone, "N" | "M") && is_vowel(prev) && next.is_some() && !is_vowel(next) {
                    g = g.repeat(2);
                }
                if last && !matches!(phone, "K" | "G" | "F" | "R" | "B" | "Y" | "W") && !VOWELS.contains(&phone) {
                    g = match phone {
                        "S" | "TH" => "sse".into(),
                        "NG" => "ngue".into(),
                        _ => g + "e",
                    };
                }
            }
            "de" => {
                if phone == "S" && i == 0 && matches!(next, Some("T" | "P")) {
                    g = "ss".into();
                }
                if phone == "S" && is_vowel(prev) && is_vowel(next) {
                    g = "ss".into();
                }
                if phone == "IY" && last {
                    g = "i".into();
                }
                if phone == "ER" && last && stress == '0' {
                    g = "er".into();
                }
            }
            _ => {
                if phone == "AH" && stress == '0' && last {
                    g = "a".into();
                }
                if phone == "S" && i == 0 && next.is_some() && !is_vowel(next) {
                    g = "es".into();
                }
                if phone == "K" {
                    g = if matches!(next, Some("IH" | "IY" | "EH" | "EY")) { "qu".into() } else { "c".into() };
                }
                if phone == "G" && matches!(next, Some("IH" | "IY" | "EH" | "EY")) {
                    g = "gu".into();
                }
                if phone == "W" && i == 0 && matches!(next, Some("AA" | "AE" | "AH" | "AO")) {
                    g = "gu".into();
                }
            }
        }
        out.push_str(&g);
    }
    out
}

/// Sounds that are a word in none of the languages (« euh », « hmm », « äh »…), as the apps' Hesitations take them
/// out of a transcript; « hum » is an English word and « um » a German one, so each only outside that language.
static HESITATION: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"^(?:e+u+h+|h+e+u+|h+u+m+|h+m+|m+h+|m{3,}|u+h+m*|u+m+|e+r+m+|ä+h+m*|ö+h+m*|e+h+m+)$")
        .expect("hesitation pattern")
});

fn hesitation(word: &str, lang: &str) -> bool {
    let lower = word.to_lowercase();
    if !HESITATION.is_match(&lower) {
        return false;
    }
    let hum = lower.starts_with('h') && lower.contains('u');
    let um = lower.starts_with('u') && !lower.contains('h');
    !(hum && lang == "en" || um && lang != "en")
}

/// The text without the hesitations a model writes (« Hmm, voyons… », « Euh, je crois »): a voice reads them as a
/// word (« Hmm » heard « am » in a Live call, 2026-10-30). The comma or ellipsis after one goes with it, and a
/// sentence that started with one starts with a capital again.
pub fn without_hesitations(text: &str, lang: &str) -> String {
    let in_word = |c: char| c.is_alphanumeric() || matches!(c, '\'' | '’' | '-');
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    let mut capitalize = false;
    while let Some(start) = rest.find(in_word) {
        let end = rest[start..].find(|c: char| !in_word(c)).map_or(rest.len(), |e| start + e);
        let word = &rest[start..end];
        if !hesitation(word, lang) {
            let before = &rest[..start];
            out.push_str(before);
            if capitalize {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            } else {
                out.push_str(word);
            }
            capitalize = false;
            rest = &rest[end..];
            continue;
        }
        // the comma before it, and what follows it: spaces, an ellipsis, a comma (a full stop too at a sentence start)
        let before = rest[..start].trim_end_matches([' ', '\t']);
        out.push_str(before.strip_suffix(',').unwrap_or(before).trim_end_matches([' ', '\t']));
        let opens = out.trim_end().chars().last().is_none_or(|c| ".!?…:\n".contains(c));
        let mut after = rest[end..].trim_start_matches([' ', '\t']);
        after = after.strip_prefix("...").or_else(|| after.strip_prefix('…')).unwrap_or(after);
        after = after.trim_start_matches([' ', '\t']);
        after = after.strip_prefix(',').unwrap_or(after);
        if opens {
            after = after.strip_prefix(['.', '!']).unwrap_or(after);
            capitalize = true;
        }
        let after = after.trim_start_matches([' ', '\t']);
        if !out.is_empty() && !out.ends_with(char::is_whitespace) && after.starts_with(in_word) {
            out.push(' ');
        }
        rest = after;
    }
    out.push_str(rest);
    if out == text {
        return out; // nothing removed: the text as it came, its spaces and line ends included (the golden cases)
    }
    // a hesitation went: the text's own leading and trailing whitespace kept around what is left
    let lead = &text[..text.len() - text.trim_start().len()];
    let trail = &text[text.trim_end().len()..];
    format!("{lead}{}{trail}", out.trim())
}

/// The text with names, acronyms and English words written for `lang`'s voice (see the module doc). Languages
/// without a lexicon are returned as they are.
pub fn respell(text: &str, lang: &str, spelling: Spelling) -> String {
    let Some(lexicon) = LEXICONS.get(lang) else {
        return text.to_string();
    };
    // no Pocket 3.3.0 voice speaks Dutch: nothing to respell for it
    let spelling = if lang == "nl" { Spelling::Raw } else { spelling };
    let toks = tokens(text);
    let mut out = String::with_capacity(text.len() + 16);
    let mut copied = 0;
    let mut i = 0;
    'tokens: while i < toks.len() {
        let token = &toks[i];
        if token.protected {
            i += 1;
            continue;
        }
        // phrases first, the longest first (« prime video », « wi-fi »)
        for count in (2..=lexicon.longest.min(toks.len() - i)).rev() {
            let span = &toks[i..i + count];
            if span.iter().any(|t| t.protected) {
                continue;
            }
            let joined = span.windows(2).all(|w| matches!(&text[w[0].end..w[1].start], " " | "-"));
            if !joined {
                continue;
            }
            let key = text[token.start..span[count - 1].end].to_lowercase();
            let found = lexicon.always.get(&key).or_else(|| {
                if spelling == Spelling::Respelled {
                    lexicon.words.get(&key)
                } else {
                    None
                }
            });
            if let Some(spoken) = found {
                out.push_str(&text[copied..token.start]);
                out.push_str(&follow_case(&text[token.start..token.end], spoken));
                copied = span[count - 1].end;
                i += count;
                continue 'tokens;
            }
        }
        let word = &text[token.start..token.end];
        let after_number = i > 0 && {
            let prev = &toks[i - 1];
            text[prev.start..prev.end].chars().all(char::is_numeric)
                && matches!(&text[prev.end..token.start], "" | " " | "\u{a0}" | "\u{202f}")
        };
        let sentence_start = at_sentence_start(&text[..token.start]);
        if let Some(spoken) = respell_word(word, lang, lexicon, spelling, after_number, sentence_start) {
            out.push_str(&text[copied..token.start]);
            out.push_str(&spoken);
            copied = token.end;
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexicons_load_and_keys_are_lower_case() {
        for (lang, _) in SOURCES {
            let lexicon = &LEXICONS[lang];
            // Dutch has no 3.3.0 voice, so no respellings (`words`)
            assert!(lang == "nl" || lexicon.words.len() >= 20, "{lang}");
            assert!(lang == "en" || !lexicon.letters.is_empty(), "{lang}");
            let file: serde_json::Value = serde_json::from_str(lexicon_json(lang).unwrap()).unwrap();
            for key in file["words"].as_object().unwrap().keys() {
                assert_eq!(key, &key.to_lowercase(), "{lang}: {key}");
            }
        }
        assert!(ENGLISH.len() > 30000 && COMMON["fr"].len() > 15000);
    }

    #[test]
    fn french() {
        for (text, spoken) in [
            ("Envoie-le sur WhatsApp.", "Envoie-le sur WhatsApp."),
            ("Mets Spotify, puis Netflix.", "Mets Spotifaï, puis Net-flix."),
            ("Regarde ton dépôt GitHub.", "Regarde ton dépôt Guite-heub."),
            ("Un meeting à 10 h, deux meetings demain.", "Un mitingue à 10 h, deux mitingues demain."),
            ("Mon iPhone 16 et ma PS5.", "Mon Aïe-faune 16 et ma pé èsse 5."),
            ("La SNCF et la NASA, l'OTAN.", "La èsse ène cé èffe et la Nasa, l'Otan."),
            ("Le code PIN et le Wi-Fi, avec Prime Video.", "Le code pine et le Oui-fi, avec Praïme vidéo."),
            ("Note-le dans Notion, une bonne notion.", "Note-le dans Nôcheune, une bonne notion."),
            ("Cherche sur DuckDuckGo.", "Cherche sur Deuk Deuk Go."),
            ("Louis XIV et le tome II.", "Louis XIV et le tome II."),
            ("MERCI et OUI, c'est OK.", "MERCI et OUI, c'est OK."),
            ("Écris à alex@example.com ou va sur bipagents.com.", "Écris à alex@example.com ou va sur bipagents.com."),
            ("Il pèse 2 GB, soit 5 km.", "Il pèse 2 GB, soit 5 km."),
            ("Utilise `sleep_score` et sleep_time.", "Utilise `sleep_score` et sleep_time."),
            ("Les runs et les updates du weekend.", "Les reunes et les eupdètes du ouikènde."),
            ("Le pays et les mails.", "Le pays et les mèles."),
            ("Un super throwback ce soir.", "Un super srobak ce soir."),
            ("Demain matin, je pars tôt avec ma sœur.", "Demain matin, je pars tôt avec ma sœur."),
        ] {
            assert_eq!(respell(text, "fr", Spelling::Respelled), spoken, "{text}");
        }
    }

    #[test]
    fn german_and_spanish() {
        for (lang, text, spoken) in [
            ("de", "Schick es per WhatsApp.", "Schick es per WhatsApp."),
            ("de", "Mach Spotify an, das Meeting ist um 10.", "Mach Sspotifai an, das Mieting ist um 10."),
            ("de", "Die USA und der ADAC.", "Die Uh Es Ah und der Ah Deh Ah Zeh."),
            ("de", "Morgen fahren wir mit dem Zug nach Hause.", "Morgen fahren wir mit dem Zug nach Hause."),
            ("es", "Pon Spotify.", "Pon Espotifai."),
            ("es", "El iPhone 16 y el email.", "El Aifon 16 y el imeil."),
            ("es", "Mándalo por WhatsApp a la ONU.", "Mándalo por Guasap a la Onu."),
            ("es", "La DGT y el DNI.", "La de ge te y el de ene i."),
            ("es", "Mañana vamos a la playa con mi hermana.", "Mañana vamos a la playa con mi hermana."),
            (
                "en",
                "Search DuckDuckGo on your iPhone or WhatsApp, the NASA way.",
                "Search Duck Duck Go on your iPhone or WhatsApp, the NASA way.",
            ),
        ] {
            assert_eq!(respell(text, lang, Spelling::Respelled), spoken, "{lang}: {text}");
        }
    }

    /// Pocket ≥ 4: names and English words as written, only the reading rules and the exception list.
    #[test]
    fn raw_for_pocket_4() {
        for (lang, text, spoken) in [
            (
                "fr",
                "Mets Spotify, ton dépôt GitHub sur WhatsApp, un meeting sur Teams.",
                "Mets Spotify, ton dépôt GitHub sur WhatsApp, un meeting sur Teams.",
            ),
            (
                "fr",
                "Ta montre Garmin, deux deadlines et la deadline.",
                "Ta montre Garmine, deux dèd-laïnes et la dèd-laïne.",
            ),
            (
                "fr",
                "Le PDF, le GPS et la clé USB, la NASA, ton code PIN, OK.",
                "Le pé dé èffe, le gé pé èsse et la clé u èsse bé, la NASA, ton code PIN, OK.",
            ),
            ("fr", "Mon iPhone 16 et ma PS5 sur DuckDuckGo.", "Mon iPhone 16 et ma pé èsse 5 sur DuckDuckGo."),
            ("fr", "Un super throwback ce soir.", "Un super throwback ce soir."),
            (
                "de",
                "Das PDF in der iCloud, die Deadline am Weekend.",
                "Das Peh Deh Ef in der iCloud, die Deadline am Weekend.",
            ),
            ("es", "El PDF en iCloud y el GPS.", "El pe de efe en iCloud y el ge pe ese."),
            (
                "nl",
                "De PDF staat in je iCloud, zet de GPS aan, de NS en de ANWB.",
                "De pee dee ef staat in je iCloud, zet de gee pee es aan, de en es en de aa en wee bee.",
            ),
            (
                "nl",
                "Je Garmin, Spotify en WhatsApp, je PIN bij de NAVO, OK en Louis XIV.",
                "Je Garmin, Spotify en WhatsApp, je PIN bij de NAVO, OK en Louis XIV.",
            ),
            ("nl", "Mijn iPhone 16 en de PS5, 2 GB.", "Mijn iPhone 16 en de pee es 5, 2 GB."),
        ] {
            assert_eq!(respell(text, lang, Spelling::Raw), spoken, "{lang}: {text}");
        }
        // Dutch stays raw even for an old engine
        assert_eq!(respell("Zet Spotify aan.", "nl", Spelling::Respelled), "Zet Spotify aan.");
        assert_eq!(Spelling::for_engine(None), Spelling::Raw);
        assert_eq!(Spelling::for_engine(Some("4.0.1")), Spelling::Raw);
        assert_eq!(Spelling::for_engine(Some("4")), Spelling::Raw);
        assert_eq!(Spelling::for_engine(Some("3.3.0")), Spelling::Respelled);
        assert_eq!(Spelling::for_engine(Some("v3")), Spelling::Respelled);
        assert_eq!(Spelling::for_engine(Some("later")), Spelling::Raw);
    }

    #[test]
    fn francisation_tables() {
        for (lang, word, spoken) in [
            ("fr", "bluetooth", "bloutousse"),
            ("fr", "token", "tokène"),
            ("fr", "boss", "bosse"),
            ("fr", "weekend", "ouikènnde"),
            ("fr", "sweet", "souite"),
            ("de", "bluetooth", "bluhtuhs"),
            ("de", "stream", "sstriem"),
            ("de", "happy", "häpi"),
            ("es", "bluetooth", "blutuz"),
            ("es", "stream", "estrim"),
            ("es", "happy", "japi"),
        ] {
            assert_eq!(francise(ENGLISH[word], word, lang), spoken, "{lang}: {word}");
        }
    }

    /// The detector on words the lexicon doesn't list: English ones it should catch, and the language's own words
    /// that look English (« football », « parking ») or are English words too (« table », « message »), which it must
    /// never touch (a lexicon entry may keep one as it is). Precision must stay at 1 on these.
    #[test]
    fn detector_precision() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "fr",
                &[
                    "thriller",
                    "sunshine",
                    "teenager",
                    "highlight",
                    "clean",
                    "speech",
                    "downtown",
                    "throwback",
                    "keyboard",
                    "rainbow",
                    "sweet",
                    "thing",
                    "growth",
                    "happy",
                    "healthy",
                    "lifestyle",
                    "worker",
                    "showroom",
                    "wallet",
                    "earbuds",
                    "skateboard",
                    "cheap",
                    "sheet",
                    "meal",
                    "week",
                ],
                &[
                    "football",
                    "parking",
                    "camping",
                    "table",
                    "message",
                    "village",
                    "important",
                    "question",
                    "rugby",
                    "whisky",
                    "pressing",
                    "shampooing",
                    "bowling",
                    "marketing",
                    "sandwich",
                    "tennis",
                    "chocolat",
                    "orange",
                    "piano",
                    "film",
                    "cool",
                    "smoking",
                    "brushing",
                    "jogging",
                    "kiwi",
                    "taxi",
                    "toast",
                    "dossier",
                    "moto",
                    "train",
                    "sport",
                    "hockey",
                ],
            ),
            (
                "de",
                &[
                    "thriller",
                    "sunshine",
                    "highlight",
                    "throwback",
                    "rainbow",
                    "growth",
                    "healthy",
                    "lifestyle",
                    "showroom",
                    "cheap",
                    "speech",
                    "teenager",
                    "bathroom",
                    "weather",
                    "toothbrush",
                ],
                &[
                    "hand", "bank", "kind", "rock", "ring", "warm", "winter", "wind", "fisch", "schuh", "tee", "boot",
                    "hotel", "taxi", "film", "sport", "handy", "park", "stop", "test", "tisch", "kaffee",
                ],
            ),
            (
                "es",
                &[
                    "thriller",
                    "sunshine",
                    "highlight",
                    "throwback",
                    "rainbow",
                    "growth",
                    "healthy",
                    "lifestyle",
                    "showroom",
                    "cheap",
                    "speech",
                    "teenager",
                    "keyboard",
                    "wallet",
                    "sweet",
                    "happy",
                    "thing",
                    "worker",
                ],
                &[
                    "casa",
                    "mesa",
                    "hotel",
                    "taxi",
                    "piano",
                    "general",
                    "animal",
                    "doctor",
                    "radio",
                    "tenis",
                    "chocolate",
                    "club",
                    "fútbol",
                    "bar",
                    "mar",
                    "sol",
                    "capital",
                    "final",
                    "error",
                    "local",
                ],
            ),
        ];
        let mut report = Vec::new();
        for (lang, english, own) in cases {
            let caught = english.iter().filter(|w| detected(w, lang)).count();
            let missed: Vec<&&str> = english.iter().filter(|w| !detected(w, lang)).collect();
            let false_hits: Vec<&&str> = own.iter().filter(|w| respell(w, lang, Spelling::Respelled) != **w).collect();
            let precision = caught as f32 / (caught + false_hits.len()).max(1) as f32;
            report.push(format!(
                "{lang}: caught {caught}/{} English words (missed {missed:?}), {} false hits on {} own words, precision \
                 {precision:.2}",
                english.len(),
                false_hits.len(),
                own.len()
            ));
            assert!(false_hits.is_empty(), "{lang}: {false_hits:?}");
            assert!(caught * 10 >= english.len() * 6, "{lang}: recall {caught}/{}", english.len());
        }
        eprintln!("{}", report.join("\n"));
    }

    #[test]
    fn fast_enough() {
        let sentences = [
            ("fr", "Envoie le compte rendu du meeting sur WhatsApp, puis mets Spotify et ouvre ton dépôt GitHub."),
            ("fr", "La SNCF annonce un retard de 20 min sur le TGV, ton iPhone 16 a reçu la notification."),
            ("de", "Schick es per WhatsApp, das Update für dein iPhone kommt am Weekend."),
            ("es", "Pon Spotify en el iPhone 16 y manda el email por WhatsApp a la DGT."),
        ];
        for (lang, text) in sentences {
            respell(text, lang, Spelling::Respelled); // the lists load once
        }
        let start = std::time::Instant::now();
        let rounds = 200;
        for _ in 0..rounds {
            for (lang, text) in sentences {
                std::hint::black_box(respell(text, lang, Spelling::Respelled));
                std::hint::black_box(respell(text, lang, Spelling::Raw));
            }
        }
        let per_sentence = start.elapsed().as_secs_f64() * 1000.0 / (rounds * sentences.len() * 2) as f64;
        eprintln!("respell: {per_sentence:.4} ms per sentence");
        assert!(per_sentence < 5.0, "{per_sentence} ms");
    }

    /// The Live greeting and waiting lines as the phone's Bip says them (/v1/tts/prepare, Pocket 4: `Raw`). « IA »
    /// spelled « i a » was heard « aya » (2026-10-30): one hyphenated word, the letters one after the other.
    #[test]
    fn live_lines_as_spoken() {
        for spelling in [Spelling::Raw, Spelling::Respelled] {
            let fr = |text: &str| crate::numbers_fr::prepare_for_synthesis_with(text, spelling);
            let intl = |text: &str, lang: &str| crate::speech_intl::prepare_for_synthesis_with(text, lang, spelling);
            assert_eq!(fr("Coucou, c'est Vie, ton agent IA. Ça va ?"), "Coucou, c'est Vie, ton agent I-a. Ça va ?");
            assert_eq!(fr("L'IA de la maison et l'ia."), "L'I-a de la maison et l'i-a.");
            assert_eq!(intl("¡Hola! Soy Vie, tu agente de IA. ¿Qué tal?", "es"), "¡Hola! Soy Vie, tu agente de I-a. ¿Qué tal?");
            assert_eq!(intl("Hallo, hier ist Vie, dein KI-Agent.", "de"), "Hallo, hier ist Vie, dein Kah Ih-Agent.");
            assert_eq!(intl("Hoi, met Vie, je AI-agent.", "nl"), "Hoi, met Vie, je aa ie-agent.");
            assert_eq!(intl("Hi, it's Vie, your AI agent.", "en"), "Hi, it's Vie, your AI agent.");
            assert_eq!(fr("Hmm, voyons… Demain il pleut."), "Voyons… Demain il pleut.");
        }
    }

    #[test]
    fn hesitations_are_not_read() {
        for (lang, text, spoken) in [
            ("fr", "Hmm, voyons…", "Voyons…"),
            ("fr", "Hmm… je crois que oui.", "Je crois que oui."),
            ("fr", "Euh. Bon, demain.", "Bon, demain."),
            ("fr", "C'est, euh, assez bien, hum, je crois.", "C'est assez bien je crois."),
            ("fr", "Bon. Euh… on part demain ?", "Bon. On part demain ?"),
            ("fr", "Une heure d'humour avec Humbert, son album, ah !", "Une heure d'humour avec Humbert, son album, ah !"),
            ("en", "Hmm, let me see… Um, it's sunny.", "Let me see… It's sunny."),
            ("en", "Hum a tune, uh, for me.", "Hum a tune for me."),
            ("de", "Hm, mal sehen… Um 10 Uhr, ähm, kommt er.", "Mal sehen… Um 10 Uhr kommt er."),
            ("nl", "Hm, even kijken…", "Even kijken…"),
            ("es", "Ehm, mañana vamos.", "Mañana vamos."),
            ("fr", "Hmm", ""),
        ] {
            assert_eq!(without_hesitations(text, lang), spoken, "{lang}: {text}");
        }
    }
}
