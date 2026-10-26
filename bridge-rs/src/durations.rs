//! « m:ss » and « h:mm:ss » read as durations when they are one, not as clock hours.
//!
//! The Wellness agent writes a running pace « 5:10 par km »: the clock rule would say « cinq heures dix ». Before the
//! numbers are spelled, a colon time is rewritten as minutes and seconds when:
//! - a pace unit follows (« /km », « par km », « au km », « du km », « min/km », « /mi », « per mile », « pro km »,
//!   « por km », « el km », « /100 m » swimming, « /500 m » rowing): « 5:10 par km » -> « cinq minutes dix par
//!   kilomètre », « 1:52/100 m » -> « une minute cinquante-deux aux 100 mètres »;
//! - it has three groups (« 3:25:12 », a marathon time) -> « trois heures vingt-cinq minutes douze », unless a clock
//!   word comes before (« à 14:05:00 » stays a clock);
//! - a duration word comes before (« en 25:30 », « temps de », « chrono », « durée », « record », « meilleur temps »,
//!   « tour en », « moyenne de », « allure », « rythme », « pace », « ritmo », « Bestzeit »...) or « min » after;
//! - its first part is above 23 (« 47:12 » can't be a clock), or another colon time of the same line is a duration
//!   (a list of splits « 5:10, 5:05, 4:58 »);
//! - it is written the runner's way: « 5'10 », « 5'10" » (always a duration).
//!
//! A clock word before keeps the hours (« à 5:10 », « vers 17:30 », « rendez-vous 9:15 », « dès 7:00 », « at »,
//! « um », « a las »), and so does a bare « 17:30 » without a cue. French is written in words here (« une minute »
//! needs the feminine); the other languages keep digits with their unit words for their number rules
//! (« 5 Minuten 10 pro Kilometer », « 1 minute » -> « one minute »).

use std::sync::LazyLock;

use regex::Regex;

use crate::numbers_fr;

/// A colon time, or the runner's « 5'10" ».
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(\d{1,2}):([0-5]\d)(?::([0-5]\d))?|(\d{1,2})['’′]([0-5]\d)(?:["”″]|'')?"#).unwrap());

/// What may follow: a pace unit (« par km », « /100 m », « min/km »), `min` alone, or nothing of the sort.
static PACE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s?(?:min(?:utes?|uten?|utos?)?\.?\s?)?(?:/\s?|(?:par|au|du|per|pro|por|el|a|al|je)\s+(?:cada\s+)?)(100\s?m|500\s?m|km|kilom[eèé]t(?:er|re|ro)s?|kilometers?|mi|miles?|meilen?|milla|mijl(?:en)?)\b",
    )
    .unwrap()
});
static MIN_AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^\s?min(?:utes?|uten?|utos?)?\b").unwrap());

fn cues(lang: &str) -> (&'static [&'static str], &'static [&'static str]) {
    // (duration words that may come just before, clock words that may come just before), lower case
    match lang {
        "fr" => (
            &[
                "en",
                "temps de",
                "temps",
                "chrono",
                "chrono de",
                "durée",
                "durée de",
                "record",
                "record de",
                "meilleur temps",
                "tour en",
                "moyenne de",
                "moyenne",
                "allure",
                "allure de",
                "rythme",
                "rythme de",
                "pace",
                "split",
                "fini en",
                "bouclé en",
                "couru en",
            ],
            &[
                "à",
                "a",
                "vers",
                "dès",
                "rendez-vous",
                "rdv",
                "jusqu'à",
                "avant",
                "après",
                "de",
                "entre",
                "et",
                "pour",
                "réveil",
                "réveil à",
                "depuis",
                "le",
                "ce soir",
                "ce matin",
                "demain",
            ],
        ),
        "de" => (
            &[
                "in",
                "zeit",
                "zeit von",
                "pace",
                "tempo",
                "rekord",
                "bestzeit",
                "durchschnitt",
                "schnitt",
                "dauer",
                "runde in",
                "split",
            ],
            &["um", "ab", "bis", "gegen", "vor", "nach", "seit", "von", "zwischen", "und"],
        ),
        "es" => (
            &[
                "en",
                "tiempo de",
                "tiempo",
                "ritmo",
                "ritmo de",
                "récord",
                "récord de",
                "mejor tiempo",
                "media de",
                "promedio de",
                "duración",
                "duración de",
                "vuelta en",
                "pace",
                "crono",
                "split",
            ],
            &["a las", "a la", "las", "la", "desde las", "hasta las", "sobre las", "hacia las", "de", "y", "entre"],
        ),
        "nl" => (
            &[
                "in",
                "tijd",
                "tijd van",
                "tempo",
                "tempo van",
                "pace",
                "record",
                "beste tijd",
                "gemiddeld",
                "gemiddelde",
                "gemiddelde van",
                "duur",
                "ronde in",
                "split",
                "gelopen in",
                "klaar in",
            ],
            &["om", "vanaf", "tot", "rond", "tegen", "voor", "na", "sinds", "van", "tussen", "en"],
        ),
        _ => (
            &[
                "in",
                "time of",
                "time",
                "pace",
                "pace of",
                "split",
                "record",
                "best",
                "best time",
                "average",
                "average of",
                "lap",
                "lap in",
                "finished in",
                "ran",
                "chrono",
                "duration",
            ],
            &["at", "by", "until", "from", "around", "before", "after", "since", "to", "and", "between"],
        ),
    }
}

/// The text just before ends with one of `words` as whole words (« … temps de »).
fn ends_with_word(before: &str, words: &[&str]) -> bool {
    let tail: String = before.trim_end().chars().rev().take(24).collect::<Vec<_>>().into_iter().rev().collect();
    let tail = tail.to_lowercase();
    words
        .iter()
        .any(|w| tail.strip_suffix(w).is_some_and(|rest| rest.chars().last().is_none_or(|c| !c.is_alphanumeric())))
}

struct Found {
    start: usize,
    end: usize,
    h: Option<u32>,
    m: u32,
    s: u32,
    /// the pace unit after it, as said in the language, and where it ends
    pace: Option<(String, usize)>,
    duration: bool,
    clock: bool,
}

fn pace_words(lang: &str, unit: &str) -> String {
    let unit = unit.to_lowercase().replace(' ', "");
    let kind = if unit.starts_with("100") {
        "100"
    } else if unit.starts_with("500") {
        "500"
    } else if unit.starts_with('k') {
        "km"
    } else {
        "mi"
    };
    match (lang, kind) {
        ("fr", "km") => "par kilomètre",
        ("fr", "mi") => "par mile",
        ("fr", "100") => "aux 100 mètres",
        ("fr", "500") => "aux 500 mètres",
        ("de", "km") => "pro Kilometer",
        ("de", "mi") => "pro Meile",
        ("de", "100") => "pro 100 Meter",
        ("de", "500") => "pro 500 Meter",
        ("es", "km") => "por kilómetro",
        ("es", "mi") => "por milla",
        ("es", "100") => "por cada 100 metros",
        ("es", "500") => "por cada 500 metros",
        ("nl", "km") => "per kilometer",
        ("nl", "mi") => "per mijl",
        ("nl", "100") => "per 100 meter",
        ("nl", "500") => "per 500 meter",
        (_, "km") => "per kilometer",
        (_, "mi") => "per mile",
        (_, "100") => "per 100 meters",
        _ => "per 500 meters",
    }
    .to_string()
}

/// « 3 heures 25 minutes 12 », « 5 minutes 10 », « 45 secondes » in the language.
fn say(lang: &str, h: Option<u32>, m: u32, s: u32) -> String {
    let (hour, minute, second) = match lang {
        "fr" => (("heure", "heures"), ("minute", "minutes"), ("seconde", "secondes")),
        "de" => (("Stunde", "Stunden"), ("Minute", "Minuten"), ("Sekunde", "Sekunden")),
        "es" => (("hora", "horas"), ("minuto", "minutos"), ("segundo", "segundos")),
        "nl" => (("uur", "uur"), ("minuut", "minuten"), ("seconde", "seconden")),
        _ => (("hour", "hours"), ("minute", "minutes"), ("second", "seconds")),
    };
    let number = |n: u32| {
        if lang == "fr" {
            numbers_fr::feminine(&numbers_fr::spell_int(n as i64))
        } else {
            n.to_string()
        }
    };
    let unit = |n: u32, (one, many): (&str, &str)| format!("{} {}", number(n), if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if let Some(h) = h.filter(|h| *h > 0) {
        parts.push(unit(h, hour));
        if m > 0 {
            parts.push(unit(m, minute));
        }
        if s > 0 {
            parts.push(if m > 0 { number(s) } else { unit(s, second) });
        }
    } else if m > 0 {
        parts.push(unit(m, minute));
        if s > 0 {
            parts.push(number(s));
        }
    } else {
        parts.push(unit(s, second));
    }
    parts.join(" ")
}

/// The colon times of `text` that are durations, rewritten (see the module doc); the rest left to the clock rules.
pub fn spell_durations(text: &str, lang: &str) -> String {
    if !text.contains(':') && !text.contains('\'') && !text.contains('’') && !text.contains('′') {
        return text.to_string();
    }
    let (duration_words, clock_words) = cues(lang);
    let mut found: Vec<Found> = Vec::new();
    for c in TIME.captures_iter(text) {
        let whole = c.get(0).unwrap();
        let (start, end) = (whole.start(), whole.end());
        let before = &text[..start];
        let after = &text[end..];
        // whole numbers only: not « 12:30:45:10 », « v3:10 », a ratio inside a word
        if before.chars().last().is_some_and(|ch| ch.is_alphanumeric() || ch == ':' || ch == '.' || ch == ',')
            || after.chars().next().is_some_and(|ch| ch.is_ascii_digit() || ch == ':')
        {
            continue;
        }
        let runner = c.get(4).is_some();
        let num = |i: usize| c.get(i).map(|g| g.as_str().parse::<u32>().unwrap_or(0));
        let (h, m, s) = if runner {
            (None, num(4).unwrap_or(0), num(5).unwrap_or(0))
        } else if let Some(sec) = num(3) {
            (num(1), num(2).unwrap_or(0), sec)
        } else {
            (None, num(1).unwrap_or(0), num(2).unwrap_or(0))
        };
        let pace = PACE.captures(after).map(|p| (pace_words(lang, &p[1]), end + p.get(0).unwrap().end()));
        // « temps de 25:30 »: the duration word wins over the clock one (« de »)
        let cue = ends_with_word(before, duration_words);
        let clock = !runner && !cue && ends_with_word(before, clock_words);
        let duration = runner
            || pace.is_some()
            || (h.is_some() && !clock)
            || (!clock && (m > 23 || cue || MIN_AFTER.is_match(after)));
        found.push(Found { start, end, h, m, s, pace, duration, clock });
    }
    if found.is_empty() {
        return text.to_string();
    }
    // a list of splits on one line: one duration makes the others (without a clock word) durations too
    let line = |at: usize| text[..at].matches('\n').count();
    let duration_lines: Vec<usize> = found.iter().filter(|f| f.duration).map(|f| line(f.start)).collect();
    for f in found.iter_mut() {
        if !f.duration && !f.clock && f.h.is_none() && duration_lines.contains(&line(f.start)) {
            f.duration = true;
        }
    }
    let mut out = String::with_capacity(text.len() + 32);
    let mut copied = 0;
    for f in found.iter().filter(|f| f.duration) {
        out.push_str(&text[copied..f.start]);
        out.push_str(&say(lang, f.h, f.m, f.s));
        copied = f.end;
        if let Some((words, pace_end)) = &f.pace {
            out.push(' ');
            out.push_str(words);
            copied = *pace_end;
        } else if let Some(m) = MIN_AFTER.find(&text[f.end..]) {
            copied = f.end + m.end(); // « 4:45 min » -> « quatre minutes quarante-cinq »
        }
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn french() {
        for (text, spoken) in [
            ("Ton allure : 5:10 par km.", "Ton allure : cinq minutes dix par kilomètre."),
            (
                "Fais 4:45 min/km puis 5:00/km.",
                "Fais quatre minutes quarante-cinq par kilomètre puis cinq minutes par kilomètre.",
            ),
            ("Nage en 1:52/100 m.", "Nage en une minute cinquante-deux aux 100 mètres."),
            ("Rame à 2:05 /500 m.", "Rame à deux minutes cinq aux 500 mètres."),
            ("Marathon bouclé en 3:25:12 !", "Marathon bouclé en trois heures vingt-cinq minutes douze !"),
            (
                "Un 10 km en 47:12, meilleur temps 45:30.",
                "Un 10 km en quarante-sept minutes douze, meilleur temps quarante-cinq minutes trente.",
            ),
            (
                "Tes tours : 5:10, 5:05 et 4:58 au km.",
                "Tes tours : cinq minutes dix, cinq minutes cinq et quatre minutes cinquante-huit par kilomètre.",
            ),
            ("Le tour en 1:32, chrono 2:10.", "Le tour en une minute trente-deux, chrono deux minutes dix."),
            ("Rythme de 5'10\" puis 4'55.", "Rythme de cinq minutes dix puis quatre minutes cinquante-cinq."),
            (
                "Rendez-vous à 5:10, vers 17:30, rendez-vous 9:15, dès 7:00.",
                "Rendez-vous à 5:10, vers 17:30, rendez-vous 9:15, dès 7:00.",
            ),
            ("Départ 17:30.", "Départ 17:30."),
            ("Un temps de 25:30 et le train de 17:30.", "Un temps de vingt-cinq minutes trente et le train de 17:30."),
            ("Le réveil sonne à 14:05:00.", "Le réveil sonne à 14:05:00."),
            ("Sans heure ici, l'ami.", "Sans heure ici, l'ami."),
        ] {
            assert_eq!(spell_durations(text, "fr"), spoken, "{text}");
        }
    }

    #[test]
    fn other_languages() {
        for (lang, text, spoken) in [
            (
                "en",
                "Easy pace 8:30 per mile, then 1:45/100 m.",
                "Easy pace 8 minutes 30 per mile, then 1 minute 45 per 100 meters.",
            ),
            ("en", "Finished in 3:25:12. Meet at 5:10.", "Finished in 3 hours 25 minutes 12. Meet at 5:10."),
            (
                "de",
                "Pace 5:10 pro km, Bestzeit 47:12, um 17:30 Uhr.",
                "Pace 5 Minuten 10 pro Kilometer, Bestzeit 47 Minuten 12, um 17:30 Uhr.",
            ),
            (
                "es",
                "Ritmo de 5:10 por km y a las 9:15 el desayuno.",
                "Ritmo de 5 minutos 10 por kilómetro y a las 9:15 el desayuno.",
            ),
            ("es", "Tiempo de 1:05:00 en la media.", "Tiempo de 1 hora 5 minutos en la media."),
            (
                "nl",
                "Een tempo van 5:10 per km, beste tijd 47:12, om 17:30 uur.",
                "Een tempo van 5 minuten 10 per kilometer, beste tijd 47 minuten 12, om 17:30 uur.",
            ),
            (
                "nl",
                "Zwemmen in 1:52/100 m, marathon in 3:25:12.",
                "Zwemmen in 1 minuut 52 per 100 meter, marathon in 3 uur 25 minuten 12.",
            ),
        ] {
            assert_eq!(spell_durations(text, lang), spoken, "{lang}: {text}");
        }
    }
}
