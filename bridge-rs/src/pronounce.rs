//! Names, acronyms and letters written the way a French, German, Spanish or Dutch voice reads them.
//!
//! Pocket TTS 4.x (every server and phone pack since 2026-10-25) says brands and anglicisms right from the raw text:
//! only the rules about reading are applied here, before numbers are spelled:
//!
//! - the short exception list (`always` in the lexicon `assets/pronunciation/<lang>.json`: words 4.x still says
//!   wrong, filled from listening tests only: « Garmin » -> « Garmine », « deadline » -> « dèd-laïne »); a plural
//!   adds « s » to a listed singular unless the word is a frequent one of the language's own (« pays »);
//! - letters glued to digits are cut (« PS5 » -> « P S 5 » read « pé èsse cinq »); digits first (« 14h30 », « 1er »)
//!   are the numbers' business;
//! - acronyms in capitals of at most 5 letters are spelled with the language's letter names (« PDF » -> « pé dé
//!   èffe »: 4.x misreads them in German and Spanish too), unless the lexicon says them as a word (NASA, PIN, OK) or
//!   they are a word of the language in capitals (« MERCI », `common_<lang>.txt`: its words of 5 letters or fewer);
//!   a longer word in capitals is read as a word (« ATTENTION », « UNICEF »).
//!
//! Names and English words are left as written, CamelCase included. Units after a number (« 2 GB », « 5 km »),
//! e-mail addresses, links and @handles are left to the rules after this one. The lexicon's `words` and `exact`
//! (respellings for the old 3.3.0 voices) are the apps' offline copy's business, not read here.

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

/// Per language: its own frequent words of at most [`SPELLED_MAX`] letters -> their rank (1 = the most frequent) in the
/// whole list (« merci 85 »).
static COMMON: LazyLock<HashMap<&'static str, HashMap<&'static str, u32>>> = LazyLock::new(|| {
    COMMON_TXT
        .iter()
        .map(|(lang, txt)| {
            let ranks = txt
                .lines()
                .filter(|l| !l.starts_with('#'))
                .filter_map(|l| l.split_once(' ').and_then(|(word, rank)| Some((word, rank.parse().ok()?))))
                .collect();
            (*lang, ranks)
        })
        .collect()
});

/// The longest word in capitals spelled as an acronym (« ANSSI »); a longer one is read as a word.
const SPELLED_MAX: usize = 5;

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

/// Small Roman numerals (« Louis XIV », « tome II »): not acronyms.
fn roman(word: &str) -> bool {
    word.chars().all(|c| matches!(c, 'I' | 'V' | 'X'))
}

/// The word respelled, or None to leave it as it is.
fn respell_word(word: &str, lang: &str, lexicon: &Lexicon, after_number: bool) -> Option<String> {
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
    let capitals = word.chars().count() >= 2 && word.chars().all(char::is_uppercase);
    if lexicon.exact.contains_key(word) || lookup(&lexicon.words).is_some() {
        // the voice says it from the written word (« PIN », « NASA », « OK »): not spelled
        return None;
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
                    respell_word(part, lang, lexicon, false).unwrap_or_else(|| part.clone())
                }
            })
            .collect();
        return Some(spoken.join(" "));
    }
    let count = word.chars().count();
    if capitals {
        if lexicon.letters.is_empty() || roman(word) || count > SPELLED_MAX {
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
    None
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

/// The text with exceptions and acronyms written for `lang`'s voice (see the module doc). Languages without a
/// lexicon are returned as they are.
pub fn respell(text: &str, lang: &str) -> String {
    let Some(lexicon) = LEXICONS.get(lang) else {
        return text.to_string();
    };
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
            if let Some(spoken) = lexicon.always.get(&key) {
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
        if let Some(spoken) = respell_word(word, lang, lexicon, after_number) {
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
        assert!(COMMON["fr"].len() > 5000 && COMMON["fr"]["merci"] == 85);
        assert!(COMMON.values().flat_map(|ranks| ranks.keys()).all(|w| w.chars().count() <= SPELLED_MAX));
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
            // 5 letters at most are spelled, a word of the language in capitals never; longer ones read as words
            (
                "fr",
                "MERCI ! La SNCF et l'ANSSI, ATTENTION au CHANGEMENT, l'UNICEF.",
                "MERCI ! La èsse ène cé èffe et l'a ène èsse èsse i, ATTENTION au CHANGEMENT, l'UNICEF.",
            ),
            (
                "de",
                "DANKE! Die DSGVO gilt, ACHTUNG, auch für UNICEF.",
                "DANKE! Die Deh Es Geh Fau Oh gilt, ACHTUNG, auch für UNICEF.",
            ),
            ("es", "GRACIAS. La DGT y UNICEF.", "GRACIAS. La de ge te y UNICEF."),
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
            assert_eq!(respell(text, lang), spoken, "{lang}: {text}");
        }
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
            respell(text, lang); // the lists load once
        }
        let start = std::time::Instant::now();
        let rounds = 200;
        for _ in 0..rounds {
            for (lang, text) in sentences {
                std::hint::black_box(respell(text, lang));
            }
        }
        let per_sentence = start.elapsed().as_secs_f64() * 1000.0 / (rounds * sentences.len()) as f64;
        eprintln!("respell: {per_sentence:.4} ms per sentence");
        assert!(per_sentence < 5.0, "{per_sentence} ms");
    }

    /// The Live greeting and waiting lines as the phone's Bip says them (/v1/tts/prepare, Pocket 4: `Raw`). « IA »
    /// spelled « i a » was heard « aya » (2026-10-30): one hyphenated word, the letters one after the other.
    #[test]
    fn live_lines_as_spoken() {
        {
            let fr = crate::numbers_fr::prepare_for_synthesis;
            let intl = crate::speech_intl::prepare_for_synthesis;
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
