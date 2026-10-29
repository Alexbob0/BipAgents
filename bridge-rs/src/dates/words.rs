//! The words of the standard dates in the apps' four languages (fr, en, de, es): countries, weekdays and months,
//! clock changes, holidays and school holidays, the travel question about the routines.

use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};

/// The countries the offer is sold in (EU members, then the others), with their names in fr, en, de, es.
pub const COUNTRIES: [(&str, [&str; 4]); 32] = [
    ("AT", ["Autriche", "Austria", "Österreich", "Austria"]),
    ("BE", ["Belgique", "Belgium", "Belgien", "Bélgica"]),
    ("BG", ["Bulgarie", "Bulgaria", "Bulgarien", "Bulgaria"]),
    ("HR", ["Croatie", "Croatia", "Kroatien", "Croacia"]),
    ("CY", ["Chypre", "Cyprus", "Zypern", "Chipre"]),
    ("CZ", ["Tchéquie", "Czechia", "Tschechien", "Chequia"]),
    ("DK", ["Danemark", "Denmark", "Dänemark", "Dinamarca"]),
    ("EE", ["Estonie", "Estonia", "Estland", "Estonia"]),
    ("FI", ["Finlande", "Finland", "Finnland", "Finlandia"]),
    ("FR", ["France", "France", "Frankreich", "Francia"]),
    ("DE", ["Allemagne", "Germany", "Deutschland", "Alemania"]),
    ("GR", ["Grèce", "Greece", "Griechenland", "Grecia"]),
    ("HU", ["Hongrie", "Hungary", "Ungarn", "Hungría"]),
    ("IE", ["Irlande", "Ireland", "Irland", "Irlanda"]),
    ("IT", ["Italie", "Italy", "Italien", "Italia"]),
    ("LV", ["Lettonie", "Latvia", "Lettland", "Letonia"]),
    ("LT", ["Lituanie", "Lithuania", "Litauen", "Lituania"]),
    ("LU", ["Luxembourg", "Luxembourg", "Luxemburg", "Luxemburgo"]),
    ("MT", ["Malte", "Malta", "Malta", "Malta"]),
    ("NL", ["Pays-Bas", "Netherlands", "Niederlande", "Países Bajos"]),
    ("PL", ["Pologne", "Poland", "Polen", "Polonia"]),
    ("PT", ["Portugal", "Portugal", "Portugal", "Portugal"]),
    ("RO", ["Roumanie", "Romania", "Rumänien", "Rumanía"]),
    ("SK", ["Slovaquie", "Slovakia", "Slowakei", "Eslovaquia"]),
    ("SI", ["Slovénie", "Slovenia", "Slowenien", "Eslovenia"]),
    ("ES", ["Espagne", "Spain", "Spanien", "España"]),
    ("SE", ["Suède", "Sweden", "Schweden", "Suecia"]),
    ("US", ["États-Unis", "United States", "Vereinigte Staaten", "Estados Unidos"]),
    ("GB", ["Royaume-Uni", "United Kingdom", "Vereinigtes Königreich", "Reino Unido"]),
    ("CA", ["Canada", "Canada", "Kanada", "Canadá"]),
    ("IL", ["Israël", "Israel", "Israel", "Israel"]),
    ("SG", ["Singapour", "Singapore", "Singapur", "Singapur"]),
];

/// The French school holidays' zones (the « Calendrier scolaire »'s), in their order.
pub const SCHOOL_ZONES: [&str; 4] = ["A", "B", "C", "Corse"];

/// « fr », « en », « de » or « es » (from « fr-FR », « de_DE »…); English otherwise.
pub fn lang(code: &str) -> &'static str {
    let code = code.trim().to_lowercase();
    for known in ["fr", "de", "es", "en"] {
        if code.starts_with(known) {
            return known;
        }
    }
    "en"
}

fn index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

pub fn country_name(code: &str, lang: &str) -> Option<&'static str> {
    COUNTRIES.iter().find(|(c, _)| *c == code).map(|(_, names)| names[index(lang)])
}

const WEEKDAYS: [[&str; 7]; 4] = [
    ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"],
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"],
    ["Montag", "Dienstag", "Mittwoch", "Donnerstag", "Freitag", "Samstag", "Sonntag"],
    ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"],
];

const MONTHS: [[&str; 12]; 4] = [
    [
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
    ],
    [
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
    ],
    [
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
    ],
    [
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
    ],
];

/// « Dimanche 25 octobre », « Sunday, October 25 », « Sonntag, 25. Oktober », « Domingo 25 de octubre ».
pub fn day(date: NaiveDate, lang: &str) -> String {
    let i = index(lang);
    let weekday = WEEKDAYS[i][date.weekday().num_days_from_monday() as usize];
    let month = MONTHS[i][date.month0() as usize];
    let text = match lang {
        "fr" if date.day() == 1 => format!("{weekday} 1er {month}"),
        "fr" => format!("{weekday} {} {month}", date.day()),
        "de" => format!("{weekday}, {}. {month}", date.day()),
        "es" => format!("{weekday} {} de {month}", date.day()),
        _ => format!("{weekday}, {month} {}", date.day()),
    };
    capitalized(&text)
}

/// The current time as the agents read it (dates/now.rs), 24-hour clock: « jeudi 29 octobre 2026, 08:40 »,
/// « Thursday, October 29, 2026, 08:40 », « Donnerstag, 29. Oktober 2026, 08:40 », « jueves 29 de octubre de 2026,
/// 08:40 ».
pub fn now_text(at: chrono::NaiveDateTime, lang: &str) -> String {
    let (date, i) = (at.date(), index(lang));
    let weekday = WEEKDAYS[i][date.weekday().num_days_from_monday() as usize];
    let month = MONTHS[i][date.month0() as usize];
    let (d, y, clock) = (date.day(), date.year(), format!("{:02}:{:02}", at.hour(), at.minute()));
    match lang {
        "fr" if d == 1 => format!("{weekday} 1er {month} {y}, {clock}"),
        "fr" => format!("{weekday} {d} {month} {y}, {clock}"),
        "de" => format!("{weekday}, {d}. {month} {y}, {clock}"),
        "es" => format!("{weekday} {d} de {month} de {y}, {clock}"),
        _ => format!("{weekday}, {month} {d}, {y}, {clock}"),
    }
}

pub fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// A wall-clock time as people say it: « 3 h », « 2 h 30 »; « 3:00 AM »; « 3 Uhr »; « las 3:00 ».
pub fn clock(time: NaiveTime, lang: &str) -> String {
    let (h, m) = (time.hour(), time.minute());
    match lang {
        "fr" if m == 0 => format!("{h} h"),
        "fr" => format!("{h} h {m:02}"),
        "de" if m == 0 => format!("{h} Uhr"),
        "de" => format!("{h}:{m:02} Uhr"),
        "es" => format!("las {h}:{m:02}"),
        _ => {
            let (twelve, half) = match h {
                0 => (12, "AM"),
                1..=11 => (h, "AM"),
                12 => (12, "PM"),
                _ => (h - 12, "PM"),
            };
            format!("{twelve}:{m:02} {half}")
        }
    }
}

/// « une heure », « 30 min »… (a clock change is an hour almost everywhere).
fn amount(seconds: i32, lang: &str) -> String {
    let minutes = seconds.unsigned_abs() / 60;
    if minutes == 60 {
        return match lang {
            "fr" => "d'une heure".into(),
            "de" => "eine Stunde".into(),
            "es" => "una hora".into(),
            _ => "one hour".into(),
        };
    }
    match lang {
        "fr" => format!("de {minutes} min"),
        _ => format!("{minutes} min"),
    }
}

/// The clock change at `at` (the old wall clock): forward (`delta` > 0, summer time starts) or back.
pub fn clock_change(delta: i32, at: NaiveTime, lang: &str) -> String {
    let (when, by) = (clock(at, lang), amount(delta, lang));
    match (lang, delta > 0) {
        ("fr", true) => format!("passage à l'heure d'été, on avance {by} à {when}"),
        ("fr", false) => format!("passage à l'heure d'hiver, on recule {by} à {when}"),
        ("de", true) => format!("Beginn der Sommerzeit, die Uhren werden um {when} {by} vorgestellt"),
        ("de", false) => format!("Ende der Sommerzeit, die Uhren werden um {when} {by} zurückgestellt"),
        ("es", true) => format!("comienza el horario de verano, a {when} se adelanta el reloj {by}"),
        ("es", false) => format!("termina el horario de verano, a {when} se retrasa el reloj {by}"),
        (_, true) => format!("daylight saving time starts, clocks go forward {by} at {when}"),
        (_, false) => format!("daylight saving time ends, clocks go back {by} at {when}"),
    }
}

/// « jour férié : Toussaint », « jour férié (Moselle) : Vendredi saint ».
pub fn holiday(name: &str, region: Option<&str>, lang: &str) -> String {
    let label = match lang {
        "fr" => "jour férié",
        "de" => "Feiertag",
        "es" => "festivo",
        _ => "public holiday",
    };
    let colon = if lang == "fr" { " :" } else { ":" };
    match region {
        Some(region) => format!("{label} ({region}){colon} {name}"),
        None => format!("{label}{colon} {name}"),
    }
}

/// The school holidays' names of the « Calendrier scolaire » (French) in the other languages.
const SCHOOL_NAMES: [(&str, [&str; 3]); 6] = [
    ("toussaint", ["All Saints' school holidays", "Herbstferien (Allerheiligen)", "vacaciones de Todos los Santos"]),
    ("noël", ["Christmas school holidays", "Weihnachtsferien", "vacaciones de Navidad"]),
    ("hiver", ["winter school holidays", "Winterferien", "vacaciones de invierno"]),
    ("printemps", ["spring school holidays", "Frühjahrsferien", "vacaciones de primavera"]),
    ("été", ["summer holidays", "Sommerferien", "vacaciones de verano"]),
    ("ascension", ["Ascension long weekend", "Brückentag Christi Himmelfahrt", "puente de la Ascensión"]),
];

/// « Vacances de la Toussaint » in `lang` (lower case but its proper nouns).
fn school_name(name: &str, lang: &str) -> String {
    let low = name.to_lowercase();
    if lang == "fr" {
        let mut out = low;
        for proper in ["Toussaint", "Noël", "Ascension"] {
            out = out.replace(&proper.to_lowercase(), proper);
        }
        return out;
    }
    let i = index(lang).saturating_sub(1).min(2);
    SCHOOL_NAMES.iter().find(|(key, _)| low.contains(key)).map(|(_, names)| names[i].to_string()).unwrap_or(low)
}

/// What a school holiday's day is: its first day off (`Start`), the day classes resume (`End`), or a single day off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchoolDay {
    Start,
    End,
    Single,
}

/// « début des vacances de la Toussaint (zone C) », « rentrée après les vacances de Noël (zone A) »…
pub fn school(name: &str, which: SchoolDay, zone: &str, lang: &str) -> String {
    let named = school_name(name, lang);
    let zone_label = match (lang, zone) {
        (_, "Corse") => "Corse".to_string(),
        ("fr", z) => format!("zone {z}"),
        ("de", z) => format!("Zone {z}"),
        (_, z) => format!("zone {z}"),
    };
    let plural = named.starts_with("vacances");
    let text = match (lang, which) {
        ("fr", SchoolDay::Start) if plural => format!("début des {named}"),
        ("fr", SchoolDay::Start) => format!("début du {named}"),
        ("fr", SchoolDay::End) if plural => format!("rentrée après les {named}"),
        ("fr", SchoolDay::End) => format!("rentrée après le {named}"),
        ("fr", SchoolDay::Single) => named,
        ("de", SchoolDay::Start) => format!("Beginn: {named}"),
        ("de", SchoolDay::End) => format!("Schulbeginn nach: {named}"),
        ("es", SchoolDay::Start) => format!("empiezan las {named}"),
        ("es", SchoolDay::End) => format!("vuelta a clase tras las {named}"),
        (_, SchoolDay::Start) => format!("{named} begin"),
        (_, SchoolDay::End) => format!("back to school after the {named}"),
        (_, SchoolDay::Single) => named,
    };
    let where_ = match lang {
        "fr" => format!(" ({zone_label}, France)"),
        _ => format!(" (France, {zone_label})"),
    };
    format!("{text}{where_}")
}

/// One line of dates: « Dimanche 25 octobre : passage à l'heure d'hiver… ».
pub fn dated_line(date: NaiveDate, text: &str, lang: &str) -> String {
    let colon = if lang == "fr" { " :" } else { ":" };
    format!("{}{colon} {text}", day(date, lang))
}

pub fn separator(lang: &str) -> &'static str {
    if lang == "fr" {
        " ; "
    } else {
        "; "
    }
}

/// « Tokyo » from « Asia/Tokyo », « New York » from « America/New_York ».
pub fn city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// « +8 h », « −5 h 30 » (the difference between two zones' clocks now); None when they show the same time.
pub fn offset_difference(seconds: i32, lang: &str) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    let sign = if seconds > 0 { "+" } else { "−" };
    let minutes = seconds.unsigned_abs() / 60;
    let (h, m) = (minutes / 60, minutes % 60);
    Some(match (lang, m) {
        ("fr", 0) => format!("{sign}{h} h"),
        ("fr", m) => format!("{sign}{h} h {m:02}"),
        (_, 0) => format!("{sign}{h} h"),
        (_, m) => format!("{sign}{h}:{m:02} h"),
    })
}

/// The question when the phone's time zone changed and the routines still run at the old zone's time.
pub struct Question {
    pub text: String,
    pub move_label: String,
    pub keep_label: String,
}

/// `routines`: « Point du matin à 7 h »… (at most a few); `back`: the new zone is where the routines were before.
pub fn question(
    from: &str,
    to: &str,
    difference: Option<String>,
    routines: &[String],
    back: bool,
    lang: &str,
) -> Question {
    let (from_city, to_city) = (city(from), city(to));
    let diff = difference.map(|d| format!(" ({d})")).unwrap_or_default();
    let list = routines.join(", ");
    let (text, move_label, keep_label) = match lang {
        "fr" => (
            format!("Tu es maintenant à l'heure de {to_city}{diff}. Tes routines restent à l'heure de {from_city} : {list}."),
            if back { format!("Revenir comme avant (heure de {to_city})") } else { format!("Les passer à l'heure de {to_city}") },
            format!("Les garder à l'heure de {from_city}"),
        ),
        "de" => (
            format!("Du bist jetzt in der Zeit von {to_city}{diff}. Deine Routinen laufen weiter nach der Zeit von {from_city}: {list}."),
            if back { format!("Wie vorher (Zeit von {to_city})") } else { format!("Auf die Zeit von {to_city} umstellen") },
            format!("Bei der Zeit von {from_city} bleiben"),
        ),
        "es" => (
            format!("Ahora estás en la hora de {to_city}{diff}. Tus rutinas siguen en la hora de {from_city}: {list}."),
            if back { format!("Volver como antes (hora de {to_city})") } else { format!("Pasarlas a la hora de {to_city}") },
            format!("Mantenerlas en la hora de {from_city}"),
        ),
        _ => (
            format!("You're now on {to_city} time{diff}. Your routines still run on {from_city} time: {list}."),
            if back { format!("Back to how it was ({to_city} time)") } else { format!("Move them to {to_city} time") },
            format!("Keep them on {from_city} time"),
        ),
    };
    Question { text, move_label, keep_label }
}

/// « Point du matin à 7 h », « Morning check at 7:00 AM »…
pub fn routine_at(name: &str, at: Option<NaiveTime>, lang: &str) -> String {
    match at {
        None => name.to_string(),
        Some(time) => match lang {
            "fr" => format!("{name} à {}", clock(time, lang)),
            "de" => format!("{name} um {}", clock(time, lang)),
            "es" => format!("{name} a {}", clock(time, lang)),
            _ => format!("{name} at {}", clock(time, lang)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_and_clocks_in_four_languages() {
        let sunday = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        assert_eq!(day(sunday, "fr"), "Dimanche 25 octobre");
        assert_eq!(day(sunday, "en"), "Sunday, October 25");
        assert_eq!(day(sunday, "de"), "Sonntag, 25. Oktober");
        assert_eq!(day(sunday, "es"), "Domingo 25 de octubre");
        let three = NaiveTime::from_hms_opt(3, 0, 0).unwrap();
        assert_eq!(clock(three, "fr"), "3 h");
        assert_eq!(clock(NaiveTime::from_hms_opt(14, 30, 0).unwrap(), "en"), "2:30 PM");
        assert_eq!(clock_change(-3600, three, "fr"), "passage à l'heure d'hiver, on recule d'une heure à 3 h");
    }

    #[test]
    fn school_holidays_read_well() {
        assert_eq!(
            school("Vacances de la Toussaint", SchoolDay::Start, "C", "fr"),
            "début des vacances de la Toussaint (zone C, France)"
        );
        assert_eq!(
            school("Vacances d'Hiver", SchoolDay::End, "A", "fr"),
            "rentrée après les vacances d'hiver (zone A, France)"
        );
        assert_eq!(school("Pont de l'Ascension", SchoolDay::Single, "B", "fr"), "pont de l'Ascension (zone B, France)");
        assert_eq!(
            school("Vacances de Noël", SchoolDay::Start, "B", "en"),
            "Christmas school holidays begin (France, zone B)"
        );
    }

    #[test]
    fn the_travel_question() {
        let q = question(
            "Europe/Paris",
            "Asia/Tokyo",
            offset_difference(8 * 3600, "fr"),
            &["Point du matin à 7 h".into(), "Bilan du soir à 21 h".into()],
            false,
            "fr",
        );
        assert_eq!(
            q.text,
            "Tu es maintenant à l'heure de Tokyo (+8 h). Tes routines restent à l'heure de Paris : Point du matin à 7 h, \
Bilan du soir à 21 h."
        );
        assert_eq!(q.move_label, "Les passer à l'heure de Tokyo");
        assert_eq!(q.keep_label, "Les garder à l'heure de Paris");
        assert_eq!(offset_difference(-(5 * 3600 + 1800), "en").as_deref(), Some("−5:30 h"));
    }
}
