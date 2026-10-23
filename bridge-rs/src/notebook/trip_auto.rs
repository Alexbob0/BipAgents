//! Trips made from bookings (the owner, 2026-10-23: « Sujets is a mess »: city and airport names, past trips, a
//! single train or a dinner booking made into « trips »). The extraction from an archive (archives/records_fill.rs)
//! writes bookings as records; this turns them into trip pages, strictly:
//!
//! - **What a trip is**: time away. Bookings are grouped by destination and overlapping or adjacent dates (± 2 days;
//!   a return leg from the destination joins its outbound; two cities linked by a leg make one trip). A group is a
//!   trip when it has a place to stay, or an outbound and a return at least one night apart, or a booking whose trip
//!   name says it is one (« Voyage à Lisbonne », « vacances », « week-end »…). A single commute train, a restaurant or
//!   an event, an isolated flight leg stay records only.
//! - **Its name**: a human one, « Lisbonne — nov. 2025 », « Week-end à Lyon — mars 2026 » (three nights at most
//!   starting on a Thursday, Friday or Saturday): the destination city (an airport's code or name made its city by a
//!   small table, a city's name in the Carnet's language when the table has it), and the month. Never an airport.
//! - **Where**: `Voyages/` (trip.rs), a past one straight in `Archives/Voyages/` (archive.rs).
//!
//! Each run (after an extraction, and at every nightly pass: the migration of the pages made before these rules) looks
//! again at the trips it made itself — `origin: archive` in their frontmatter, or, for those made before, a page whose
//! every line links a document of an archive — and nothing else:
//! renamed by the rule while their title is still the one it gave (`auto_title`; the old name kept as an alias, the
//! links to it in other pages pointed to the new one, the bookings' « trip » too), merged when two are the same trip,
//! completed with the trip's other bookings, and turned back into plain records when they fail the rule (their lines
//! kept under the bookings' table, « Réservations », nothing dropped). A page made before is handled once (its id in
//! `trips_auto_done`): one the person restored from the history stays as they left it. Every change is in the
//! Carnet's history and can be undone there.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate, Weekday};
use regex::Regex;
use serde_json::json;

use super::records::{self, Kind};
use super::{fold, template, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// Bookings within this many days of each other (and of the same destination) are one trip.
pub const GAP_DAYS: i64 = 2;
/// A leg from the destination this many days at most after the way there is the way back.
pub const RETURN_DAYS: i64 = 31;
/// Bookings this many days old at most start a new trip page (older ones only complete the existing ones).
pub const NEW_TRIP_DAYS: i64 = 365;

// -- places ----------------------------------------------------------------------------------------------------------

/// Common airports and cities: (IATA codes, other names folded, the city in fr, en, de, es).
const PLACES: &[(&[&str], &[&str], [&str; 4])] = &[
    (
        &["CDG", "ORY", "BVA", "PAR"],
        &["charles de gaulle", "roissy", "orly", "beauvais", "paris"],
        ["Paris", "Paris", "Paris", "París"],
    ),
    (
        &["LIS"],
        &["humberto delgado", "lisbonne", "lisbon", "lisboa", "lissabon"],
        ["Lisbonne", "Lisbon", "Lissabon", "Lisboa"],
    ),
    (&["OPO"], &["francisco sa carneiro", "porto", "oporto"], ["Porto", "Porto", "Porto", "Oporto"]),
    (
        &["LHR", "LGW", "STN", "LTN", "LCY", "LON"],
        &["heathrow", "gatwick", "stansted", "luton", "londres", "london", "st pancras"],
        ["Londres", "London", "London", "Londres"],
    ),
    (&["AMS"], &["schiphol", "amsterdam"], ["Amsterdam", "Amsterdam", "Amsterdam", "Ámsterdam"]),
    (
        &["BRU", "CRL"],
        &["zaventem", "charleroi", "bruxelles", "brussels", "brussel", "brussel"],
        ["Bruxelles", "Brussels", "Brüssel", "Bruselas"],
    ),
    (&["MAD"], &["barajas", "madrid"], ["Madrid", "Madrid", "Madrid", "Madrid"]),
    (&["BCN"], &["el prat", "barcelone", "barcelona"], ["Barcelone", "Barcelona", "Barcelona", "Barcelona"]),
    (&["SVQ"], &["seville", "sevilla", "sevilla"], ["Séville", "Seville", "Sevilla", "Sevilla"]),
    (&["VLC"], &["valence espagne", "valencia"], ["Valence", "Valencia", "Valencia", "Valencia"]),
    (
        &["PMI"],
        &["palma", "majorque", "mallorca", "mallorca"],
        ["Palma de Majorque", "Palma de Mallorca", "Palma de Mallorca", "Palma de Mallorca"],
    ),
    (&["FCO", "CIA", "ROM"], &["fiumicino", "ciampino", "rome", "roma", "rom"], ["Rome", "Rome", "Rom", "Roma"]),
    (
        &["MXP", "LIN", "BGY", "MIL"],
        &["malpensa", "linate", "bergame", "milan", "milano", "mailand"],
        ["Milan", "Milan", "Mailand", "Milán"],
    ),
    (&["VCE"], &["marco polo", "venise", "venice", "venezia", "venedig"], ["Venise", "Venice", "Venedig", "Venecia"]),
    (&["NAP"], &["naples", "napoli", "neapel"], ["Naples", "Naples", "Neapel", "Nápoles"]),
    (
        &["FLR", "PSA"],
        &["florence", "firenze", "florenz", "pise", "pisa"],
        ["Florence", "Florence", "Florenz", "Florencia"],
    ),
    (&["BER", "TXL", "SXF"], &["brandenburg", "tegel", "berlin"], ["Berlin", "Berlin", "Berlin", "Berlín"]),
    (&["MUC"], &["munich", "munchen", "muenchen", "munich"], ["Munich", "Munich", "München", "Múnich"]),
    (
        &["FRA"],
        &["francfort", "frankfurt", "francfort sur le main"],
        ["Francfort", "Frankfurt", "Frankfurt", "Fráncfort"],
    ),
    (&["HAM"], &["hambourg", "hamburg", "hamburgo"], ["Hambourg", "Hamburg", "Hamburg", "Hamburgo"]),
    (&["ZRH"], &["zurich", "zuerich", "zurich kloten"], ["Zurich", "Zurich", "Zürich", "Zúrich"]),
    (&["GVA"], &["geneve", "geneva", "genf", "ginebra", "cointrin"], ["Genève", "Geneva", "Genf", "Ginebra"]),
    (&["VIE"], &["schwechat", "vienne autriche", "vienna", "wien", "viena"], ["Vienne", "Vienna", "Wien", "Viena"]),
    (&["PRG"], &["vaclav havel", "prague", "praha", "prag", "praga"], ["Prague", "Prague", "Prag", "Praga"]),
    (&["BUD"], &["budapest"], ["Budapest", "Budapest", "Budapest", "Budapest"]),
    (
        &["CPH"],
        &["kastrup", "copenhague", "copenhagen", "kopenhagen", "copenhague"],
        ["Copenhague", "Copenhagen", "Kopenhagen", "Copenhague"],
    ),
    (&["DUB"], &["dublin"], ["Dublin", "Dublin", "Dublin", "Dublín"]),
    (&["EDI"], &["edimbourg", "edinburgh"], ["Édimbourg", "Edinburgh", "Edinburgh", "Edimburgo"]),
    (
        &["ATH"],
        &["eleftherios venizelos", "athenes", "athens", "athen", "atenas"],
        ["Athènes", "Athens", "Athen", "Atenas"],
    ),
    (&["IST", "SAW"], &["sabiha gokcen", "istanbul", "estambul"], ["Istanbul", "Istanbul", "Istanbul", "Estambul"]),
    (&["NCE"], &["cote d azur", "nice"], ["Nice", "Nice", "Nizza", "Niza"]),
    (&["LYS"], &["saint exupery", "lyon"], ["Lyon", "Lyon", "Lyon", "Lyon"]),
    (&["MRS"], &["marignane", "marseille", "marsella"], ["Marseille", "Marseille", "Marseille", "Marsella"]),
    (&["TLS"], &["blagnac", "toulouse"], ["Toulouse", "Toulouse", "Toulouse", "Toulouse"]),
    (&["BOD"], &["merignac", "bordeaux", "burdeos"], ["Bordeaux", "Bordeaux", "Bordeaux", "Burdeos"]),
    (&["NTE"], &["nantes atlantique", "nantes"], ["Nantes", "Nantes", "Nantes", "Nantes"]),
    (&["MPL"], &["montpellier"], ["Montpellier", "Montpellier", "Montpellier", "Montpellier"]),
    (
        &["SXB"],
        &["entzheim", "strasbourg", "strassburg", "estrasburgo"],
        ["Strasbourg", "Strasbourg", "Straßburg", "Estrasburgo"],
    ),
    (&["BIQ"], &["biarritz"], ["Biarritz", "Biarritz", "Biarritz", "Biarritz"]),
    (&["AJA"], &["ajaccio"], ["Ajaccio", "Ajaccio", "Ajaccio", "Ajaccio"]),
    (&["BIA"], &["bastia"], ["Bastia", "Bastia", "Bastia", "Bastia"]),
    (&["MRU"], &["maurice", "mauritius"], ["Maurice", "Mauritius", "Mauritius", "Mauricio"]),
    (
        &["RUN"],
        &["roland garros", "la reunion", "saint denis de la reunion"],
        ["La Réunion", "Réunion", "Réunion", "Reunión"],
    ),
    (&["PTP"], &["pointe a pitre", "guadeloupe"], ["Guadeloupe", "Guadeloupe", "Guadeloupe", "Guadalupe"]),
    (&["FDF"], &["fort de france", "martinique"], ["Martinique", "Martinique", "Martinique", "Martinica"]),
    (
        &["RAK"],
        &["menara", "marrakech", "marrakesch", "marrakech"],
        ["Marrakech", "Marrakesh", "Marrakesch", "Marrakech"],
    ),
    (&["CMN"], &["mohammed v", "casablanca"], ["Casablanca", "Casablanca", "Casablanca", "Casablanca"]),
    (&["TUN"], &["carthage", "tunis", "tunez"], ["Tunis", "Tunis", "Tunis", "Túnez"]),
    (
        &["JFK", "EWR", "LGA", "NYC"],
        &["john f kennedy", "newark", "la guardia", "laguardia", "new york", "nueva york"],
        ["New York", "New York", "New York", "Nueva York"],
    ),
    (&["SFO"], &["san francisco"], ["San Francisco", "San Francisco", "San Francisco", "San Francisco"]),
    (&["LAX"], &["los angeles"], ["Los Angeles", "Los Angeles", "Los Angeles", "Los Ángeles"]),
    (&["YUL"], &["trudeau", "montreal"], ["Montréal", "Montreal", "Montreal", "Montreal"]),
    (&["NRT", "HND", "TYO"], &["narita", "haneda", "tokyo", "tokio"], ["Tokyo", "Tokyo", "Tokio", "Tokio"]),
    (&["KIX", "ITM"], &["kansai", "osaka"], ["Osaka", "Osaka", "Osaka", "Osaka"]),
    (&["BKK"], &["suvarnabhumi", "bangkok"], ["Bangkok", "Bangkok", "Bangkok", "Bangkok"]),
    (&["DXB"], &["dubai", "dubaï"], ["Dubaï", "Dubai", "Dubai", "Dubái"]),
    (&["SIN"], &["changi", "singapour", "singapore", "singapur"], ["Singapour", "Singapore", "Singapur", "Singapur"]),
    (&["TLV"], &["ben gurion", "tel aviv"], ["Tel Aviv", "Tel Aviv", "Tel Aviv", "Tel Aviv"]),
];

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z]{3})\b").expect("valid regex"));

/// Words around a place's name that are not the place (« Aéroport de », « Gare de », « Terminal 2 »…), folded.
const NOT_PLACE: &[&str] = &[
    "aeroport",
    "airport",
    "aeropuerto",
    "flughafen",
    "international",
    "intl",
    "gare",
    "station",
    "estacion",
    "bahnhof",
    "hbf",
    "terminal",
    "de",
    "du",
    "des",
    "d",
    "la",
    "le",
    "les",
    "l",
    "of",
    "the",
    "del",
    "el",
    "terminal",
    "ville",
    "centre",
    "center",
    "centro",
    "zentrum",
    "st",
    "saint",
    "sainte",
    "a",
    "en",
    "in",
    "nach",
    "to",
    "vers",
    "hacia",
];

/// A city of the table named in a text (« Airbnb Lisbonne » → « Lisbonne »), in the Carnet's language.
pub fn known_city(text: &str, lang: &str) -> Option<String> {
    let i = lang_index(lang);
    for c in CODE.captures_iter(text) {
        if let Some(p) = PLACES.iter().find(|p| p.0.contains(&&c[1])) {
            return Some(p.2[i].to_string());
        }
    }
    let folded = format!(" {} ", fold(text));
    PLACES
        .iter()
        .find(|p| {
            p.1.iter().any(|n| folded.contains(&format!(" {n} ")))
                || p.2.iter().any(|n| folded.contains(&format!(" {} ", fold(n))))
        })
        .map(|p| p.2[i].to_string())
}

/// The city a place's text means: an IATA code, an airport's or a city's name known to the table (in the Carnet's
/// language), else the text without « Aéroport de », « Gare de »… (None: nothing left, or only a code it doesn't know).
pub fn city(text: &str, lang: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(known) = known_city(text, lang) {
        return Some(known);
    }
    // the words of the place only: « Gare de Lyon Part-Dieu » stays as written, « Aéroport de Faro (FAO) » → « Faro »
    let cleaned = Regex::new(r"\([^)]*\)").expect("valid regex").replace_all(text, " ").to_string();
    let words: Vec<&str> = cleaned.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).collect();
    let kept: Vec<&str> = words
        .iter()
        .copied()
        .skip_while(|w| NOT_PLACE.contains(&fold(w).as_str()) || fold(w).starts_with("aeroport"))
        .take_while(|w| !["terminal", "hall"].contains(&fold(w).as_str()))
        .collect();
    let name = kept.join(" ").trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let name = tk::head(&name, 40).trim().to_string();
    if name.chars().filter(|c| c.is_alphabetic()).count() < 3 || name.chars().all(|c| !c.is_lowercase()) {
        return None;
    }
    Some(name)
}

static ARROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*(?:→|->|⇒|➜|>|–>|—>|\s-\s|\s–\s)\s*").expect("valid regex"));
static FROM_TO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:de|from|von|desde)\s+(.+?)\s+(?:à|a|vers|to|nach|hacia|pour)\s+(.+)$").expect("valid regex")
});
static AT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\s(?:à|a|au|aux|en|in|im|nach|to|pour|for|vers|hacia|towards)\s+(.+)$").expect("valid regex")
});

/// A leg's ends (« Vol Paris → Lisbonne », « Train de Lyon à Paris », « CDG-LIS »): (from, to), as written.
pub fn ends(what: &str) -> (Option<String>, Option<String>) {
    let plain = what.trim();
    let codes: Vec<String> = CODE.captures_iter(plain).map(|c| c[1].to_string()).collect();
    if let Some(c) = Regex::new(r"\b([A-Z]{3})\s*[-–→>/]+\s*([A-Z]{3})\b").expect("valid regex").captures(plain) {
        return (Some(c[1].to_string()), Some(c[2].to_string()));
    }
    let parts: Vec<&str> = ARROW.split(plain).collect();
    if parts.len() >= 2 {
        // « Vol AF1234 Paris » → « Paris »: the last words before the arrow
        let first = parts[0].split_whitespace().last().unwrap_or("").to_string();
        let from = strip_lead(parts[0]).unwrap_or(first);
        let to = parts[parts.len() - 1].split([',', '(', '·']).next().unwrap_or("").trim().to_string();
        return (Some(from), Some(to).filter(|t| !t.is_empty()));
    }
    if let Some(c) = FROM_TO.captures(plain) {
        return (
            Some(c[1].trim().to_string()),
            Some(c[2].split([',', '(', '·']).next().unwrap_or("").trim().to_string()),
        );
    }
    if codes.len() == 2 {
        return (Some(codes[0].clone()), Some(codes[1].clone()));
    }
    (None, None)
}

/// A leg's start without its kind and number (« Vol AF1234 Paris CDG » → « Paris CDG »).
fn strip_lead(text: &str) -> Option<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let kept: Vec<&str> = words
        .iter()
        .copied()
        .skip_while(|w| {
            let f = fold(w);
            LEG_WORDS.contains(&f.as_str()) || w.chars().any(|c| c.is_ascii_digit()) || f.is_empty()
        })
        .collect();
    let out = kept.join(" ");
    (!out.trim().is_empty()).then(|| out.trim().to_string())
}

const LEG_WORDS: &[&str] = &[
    "vol",
    "flight",
    "flug",
    "vuelo",
    "train",
    "tgv",
    "ouigo",
    "ter",
    "intercites",
    "ice",
    "zug",
    "tren",
    "ave",
    "bus",
    "car",
    "ferry",
    "bateau",
    "eurostar",
    "thalys",
    "aller",
    "retour",
    "outbound",
    "return",
    "billet",
    "e billet",
    "ticket",
    "de",
    "from",
    "von",
    "desde",
    "reservation",
    "booking",
    "aller simple",
    "aller retour",
];

const LODGING: &[&str] = &[
    "hotel",
    "hotels",
    "airbnb",
    "gite",
    "camping",
    "chambre d hotes",
    "chambres d hotes",
    "appartement",
    "logement",
    "nuit",
    "nuits",
    "nuitee",
    "nuitees",
    "hostel",
    "auberge",
    "lodging",
    "accommodation",
    "stay",
    "apartment",
    "unterkunft",
    "ferienwohnung",
    "pension",
    "alojamiento",
    "hostal",
    "apartamento",
    "booking com",
    "abritel",
    "residence",
    "villa",
    "chalet",
    "bed and breakfast",
    "b b",
    "riad",
    "ryokan",
    "motel",
    "resort",
];
const TRANSPORT: &[&str] = &[
    "vol",
    "vols",
    "flight",
    "flights",
    "avion",
    "train",
    "tgv",
    "ouigo",
    "ter",
    "intercites",
    "eurostar",
    "thalys",
    "ferry",
    "bateau",
    "bus",
    "flixbus",
    "blablacar",
    "car",
    "covoiturage",
    "flug",
    "zug",
    "bahn",
    "ice",
    "vuelo",
    "tren",
    "ave",
    "renfe",
    "ryanair",
    "easyjet",
    "air france",
    "transavia",
    "vueling",
    "lufthansa",
    "klm",
    "iberia",
    "sncf",
    "trenitalia",
    "sbb",
    "db",
];
const EVENT: &[&str] = &[
    "restaurant",
    "table",
    "diner",
    "dejeuner",
    "brunch",
    "concert",
    "spectacle",
    "theatre",
    "cinema",
    "musee",
    "visite",
    "match",
    "festival",
    "billet",
    "entree",
    "soiree",
    "dinner",
    "lunch",
    "show",
    "museum",
    "tour",
    "excursion",
    "konzert",
    "restaurante",
    "cena",
    "comida",
    "entrada",
    "visita",
    "atelier",
    "cours",
    "spa",
    "massage",
    "golf",
    "parking",
    "location de voiture",
    "car rental",
    "voiture",
    "mietwagen",
    "alquiler",
];
const TRIP_NAME: &[&str] = &[
    "voyage",
    "vacances",
    "sejour",
    "week end",
    "weekend",
    "road trip",
    "trip",
    "holiday",
    "holidays",
    "vacation",
    "getaway",
    "reise",
    "urlaub",
    "ferien",
    "wochenende",
    "kurzurlaub",
    "viaje",
    "vacaciones",
    "escapada",
    "fin de semana",
    "circuit",
    "croisiere",
    "cruise",
];

fn has(folded: &str, words: &[&str]) -> bool {
    let spaced = format!(" {folded} ");
    words.iter().any(|w| spaced.contains(&format!(" {w} ")))
}

/// What a booking is: « lodging », « transport », « event » (a restaurant, a show, a car…) or « other ».
pub fn booking_kind(what: &str, provider: &str) -> &'static str {
    let w = fold(what);
    let p = fold(provider);
    if has(&w, LODGING) {
        "lodging"
    } else if has(&w, TRANSPORT) || (ends(what).1.is_some() && !has(&w, EVENT)) {
        "transport"
    } else if has(&w, EVENT) {
        "event"
    } else if has(&p, LODGING) {
        "lodging"
    } else if has(&p, TRANSPORT) {
        "transport"
    } else {
        "other"
    }
}

// -- bookings and their trips ----------------------------------------------------------------------------------------

/// A booking as the trips see it.
#[derive(Debug, Clone)]
pub struct Booking {
    pub date: NaiveDate,
    pub fields: Dict,
    pub source: String,
    pub kind: &'static str,
    /// The destination (a city), its key (folded), and for a leg where it comes from.
    pub to: Option<String>,
    pub from: Option<String>,
    /// Its trip name says it is a trip.
    pub named: bool,
}

impl Booking {
    pub fn new(fields: &Dict, lang: &str) -> Option<Booking> {
        let date_text = tk::str_or_empty(fields.get("date"));
        let (m, d, y) = super::date_in(&date_text)?;
        let date = NaiveDate::from_ymd_opt(y?, m, d)?;
        let what = tk::str_or_empty(fields.get("what"));
        let provider = tk::str_or_empty(fields.get("provider"));
        let trip = super::LINK
            .replace_all(&tk::str_or_empty(fields.get("trip")), |c: &regex::Captures| c[1].to_string())
            .to_string();
        let kind = booking_kind(&what, &provider);
        let (from, to) = if kind == "transport" { ends(&what) } else { (None, None) };
        let named = has(&fold(&trip), TRIP_NAME);
        // the destination: a leg's end, a place after « à », « in »… (« Hôtel à Rome », « Vol vers Rome »), a city
        // the table knows in its words (« Airbnb Lisbonne »), else the trip's name
        let place = AT.captures(&what).map(|c| c[1].split([',', '(', '·']).next().unwrap_or("").trim().to_string());
        let to = to
            .and_then(|t| city(&t, lang))
            .or_else(|| place.and_then(|p| city(&p, lang)))
            .or_else(|| known_city(&what, lang))
            .or_else(|| city(&strip_trip_words(&trip), lang));
        let from = from.and_then(|f| city(&f, lang));
        Some(Booking {
            date,
            fields: fields.clone(),
            source: tk::str_or_empty(fields.get("source")),
            kind,
            to,
            from,
            named,
        })
    }

    fn key(&self) -> Option<String> {
        self.to.as_deref().map(fold)
    }
}

/// A trip's name without its trip words (« Voyage à Lisbonne » → « Lisbonne »).
fn strip_trip_words(name: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut skipping = true;
    for w in &words {
        let f = fold(w);
        if skipping && (TRIP_NAME.iter().any(|t| t.split(' ').any(|p| p == f)) || NOT_PLACE.contains(&f.as_str())) {
            continue;
        }
        skipping = false;
        out.push(w);
    }
    out.join(" ")
}

/// Bookings grouped into the journeys they make.
#[derive(Debug, Clone)]
pub struct Group {
    /// The destinations, in order (one or two cities).
    pub places: Vec<String>,
    pub start: NaiveDate,
    pub end: NaiveDate,
    pub bookings: Vec<Booking>,
}

impl Group {
    fn keys(&self) -> Vec<String> {
        self.places.iter().map(|p| fold(p)).collect()
    }

    /// Whether it is time away: a place to stay, an outbound and a return a night apart, or a trip by its name.
    pub fn is_trip(&self) -> bool {
        let stay = self.bookings.iter().any(|b| b.kind == "lodging");
        let legs: Vec<&Booking> = self.bookings.iter().filter(|b| b.kind == "transport").collect();
        let away = legs.len() >= 2 && (self.end - self.start).num_days() >= 1;
        let named = self.bookings.iter().any(|b| b.named) && self.bookings.iter().any(|b| b.kind != "event");
        stay || away || named
    }

    /// The trip's title: « Lisbonne — nov. 2025 », « Week-end à Lyon — mars 2026 ».
    pub fn title(&self, lang: &str) -> String {
        title(&self.places, self.start, trip_end(self), lang)
    }

    pub fn sources(&self) -> HashSet<String> {
        self.bookings.iter().map(|b| b.source.clone()).filter(|s| !s.is_empty()).collect()
    }
}

const MONTHS: [[&str; 12]; 4] = [
    ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."],
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
    ["Jan.", "Feb.", "März", "Apr.", "Mai", "Juni", "Juli", "Aug.", "Sept.", "Okt.", "Nov.", "Dez."],
    ["ene.", "feb.", "mar.", "abr.", "may.", "jun.", "jul.", "ago.", "sept.", "oct.", "nov.", "dic."],
];

/// A trip's human title from its places and dates.
pub fn title(places: &[String], start: NaiveDate, end: NaiveDate, lang: &str) -> String {
    let i = lang_index(lang);
    let and = ["et", "and", "und", "y"][i];
    let place = match places {
        [] => ["Voyage", "Trip", "Reise", "Viaje"][i].to_string(),
        [one] => one.clone(),
        [a, b, ..] => format!("{a} {and} {b}"),
    };
    let month = format!("{} {}", MONTHS[i][start.month0() as usize], start.year());
    let nights = (end - start).num_days();
    let weekend = places.len() == 1
        && (1..=3).contains(&nights)
        && matches!(start.weekday(), Weekday::Thu | Weekday::Fri | Weekday::Sat);
    if weekend {
        let lead = ["Week-end à", "Weekend in", "Wochenende in", "Fin de semana en"][i];
        format!("{lead} {place} — {month}")
    } else {
        format!("{place} — {month}")
    }
}

/// Bookings grouped by destination and dates (sorted by date first).
pub fn group(mut bookings: Vec<Booking>) -> Vec<Group> {
    bookings.sort_by_key(|b| b.date);
    let gap = chrono::Duration::days(GAP_DAYS);
    let mut groups: Vec<Group> = Vec::new();
    let mut loose: Vec<Booking> = Vec::new();
    for b in bookings {
        let near = |g: &Group| b.date >= g.start - gap && b.date <= g.end + gap;
        // the way back from the destination: within a month of the way there
        let back = |g: &Group| b.date >= g.start && b.date <= g.end + chrono::Duration::days(RETURN_DAYS);
        let key = b.key();
        let from = b.from.as_deref().map(fold);
        let found = groups.iter().rposition(|g| {
            (near(g) && key.as_ref().is_some_and(|k| g.keys().contains(k)))
                || (back(g) && b.kind == "transport" && from.as_ref().is_some_and(|f| g.keys().contains(f)))
        });
        match (found, &b.to) {
            (Some(i), _) => {
                let g = &mut groups[i];
                // a leg on to another city (Lisbonne → Porto) adds it; a leg back home does not
                if let (Some(f), Some(to)) = (&from, &b.to) {
                    if g.keys().contains(f) && !g.keys().contains(&fold(to)) && b.kind == "transport" {
                        // kept only when something else happens there (else it is the way back: see below)
                        g.places.push(to.clone());
                    }
                }
                g.end = g.end.max(b.date);
                g.bookings.push(b);
            }
            (None, Some(to)) => {
                groups.push(Group { places: vec![to.clone()], start: b.date, end: b.date, bookings: vec![b] })
            }
            (None, None) => loose.push(b),
        }
    }
    // a city added by a leg that nothing else is about is the way back home: taken off
    for g in groups.iter_mut() {
        if g.places.len() > 1 {
            let first = fold(&g.places[0]);
            let bookings = g.bookings.clone();
            g.places.retain(|p| {
                fold(p) == first
                    || bookings.iter().any(|b| b.kind != "transport" && b.key().as_deref() == Some(&fold(p)))
            });
        }
    }
    // what has no place of its own (a car, a dinner) joins the journey it falls in
    for b in loose {
        if let Some(g) = groups.iter_mut().find(|g| b.date >= g.start && b.date <= g.end) {
            g.bookings.push(b);
        }
    }
    groups
}

// -- the pages -------------------------------------------------------------------------------------------------------

static ARCHIVE_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[(archive:[A-Za-z0-9_.-]+/[a-z0-9-]+/[0-9a-fA-F]{16})(?:\|[^\]]*)?\]\]").expect("valid regex")
});

/// The documents a page's lines link (`[[archive:…|voir le document]]`).
pub fn sources_in(body: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in ARCHIVE_LINK.captures_iter(body) {
        if !out.contains(&c[1].to_string()) {
            out.push(c[1].to_string());
        }
    }
    out
}

/// Whether the extraction made this trip page: `origin: archive`, or (made before that mark) every line of it links a
/// document of an archive and it has no « En bref » of its own.
pub fn made_by_extraction(page: &Page, lang: &str) -> bool {
    if page.kind() != "trip" {
        return false;
    }
    if tk::str_or_empty(page.front.get("origin")) == "archive" {
        return true;
    }
    let brief = super::words(lang).brief;
    let mut lines = 0;
    for (section, facts) in page.sections() {
        for fact in facts {
            if fold(&section) == fold(brief) || !ARCHIVE_LINK.is_match(&fact.text) {
                return false;
            }
            lines += 1;
        }
    }
    lines > 0
}

/// Every line of it links a document of an archive (nobody added their own).
fn pure(page: &Page) -> bool {
    page.sections().iter().all(|(_, facts)| facts.iter().all(|f| ARCHIVE_LINK.is_match(&f.text)))
}

/// Its title is still the one the extraction gave (renaming it is allowed).
fn auto_titled(page: &Page) -> bool {
    match page.front.get("auto_title").and_then(serde_json::Value::as_str) {
        Some(t) => t == page.title,
        None => tk::str_or_empty(page.front.get("origin")) != "archive",
    }
}

fn see_word(lang: &str) -> &'static str {
    match lang {
        "fr" => "voir le document",
        "de" => "Dokument ansehen",
        "es" => "ver el documento",
        _ => "see the document",
    }
}

/// A booking's line in a trip page (as the extraction always wrote it).
pub fn line_of(b: &Booking, lang: &str) -> String {
    let parts: Vec<String> = ["date", "what", "provider", "reference"]
        .iter()
        .map(|k| tk::str_or_empty(b.fields.get(*k)))
        .filter(|v| !v.is_empty())
        .collect();
    if b.source.is_empty() {
        parts.join(" — ")
    } else {
        format!("{} [[{}|{}]]", parts.join(" — "), b.source, see_word(lang))
    }
}

fn section_of(b: &Booking) -> &'static str {
    match b.kind {
        "lodging" => "lodging",
        "transport" => "transport",
        _ => "onsite",
    }
}

/// A booking read back from a line the extraction wrote (« 2025-11-12 10:35 — Vol Paris → Lisbonne — TAP — X7K2PQ
/// [[archive:…]] »), for a page whose records are gone.
fn booking_from_line(text: &str, lang: &str) -> Option<Booking> {
    let source = sources_in(text).into_iter().next().unwrap_or_default();
    let plain = ARCHIVE_LINK.replace_all(text, "").trim().to_string();
    let parts: Vec<&str> = plain.split(" — ").map(str::trim).collect();
    let mut fields = Dict::new();
    for (k, v) in ["date", "what", "provider", "reference"].iter().zip(parts.iter()) {
        if !v.is_empty() {
            fields.insert((*k).into(), json!(v));
        }
    }
    if !source.is_empty() {
        fields.insert("source".into(), json!(source));
    }
    Booking::new(&fields, lang)
}

/// What a run did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Synced {
    pub created: usize,
    pub renamed: usize,
    pub merged: usize,
    pub demoted: usize,
    pub completed: usize,
}

impl Synced {
    pub fn total(&self) -> usize {
        self.created + self.renamed + self.merged + self.demoted + self.completed
    }
}

fn booking_records(config: &Config, t: &Kind) -> Vec<Dict> {
    let mut all = records::records(config, t);
    all.extend(records::archived_records(config, t));
    all
}

/// The trips the bookings from archives make, and the pages the extraction made before, put right (see the module's
/// doc). `who`: the agent whose extraction it is, or the nightly pass. Returns what it did.
pub fn sync(config: &Config, who: &Who, today: NaiveDate) -> anyhow::Result<Synced> {
    let _guard = super::lock();
    sync_locked(config, who, today)
}

fn sync_locked(config: &Config, who: &Who, today: NaiveDate) -> anyhow::Result<Synced> {
    let lang = super::language(config);
    let mut done = Synced::default();
    let Some(bt) = records::kind("booking") else { return Ok(done) };
    let state = super::read_state(config);
    let handled: HashSet<String> = super::strings(state.get("trips_auto_done")).into_iter().collect();
    let used: HashSet<String> = super::strings(state.get("trips_auto_sources")).into_iter().collect();
    // the bookings that came from an archive
    let mut bookings: Vec<Booking> = booking_records(config, bt)
        .iter()
        .filter(|r| tk::str_or_empty(r.get("source")).starts_with("archive:"))
        .filter_map(|r| Booking::new(r, &lang))
        .collect();
    // the extraction's pages (those the person or an agent took over are left alone)
    let pages: Vec<(String, Page)> = super::all_pages(config, Part::Main)
        .into_iter()
        .filter(|(_, p)| made_by_extraction(p, &lang))
        .filter(|(_, p)| tk::str_or_empty(p.front.get("origin")) == "archive" || !handled.contains(&p.id()))
        .collect();
    // lines of those pages whose record is gone still count
    let known: HashSet<String> = bookings.iter().map(|b| b.source.clone()).collect();
    for (_, page) in &pages {
        for (_, facts) in page.sections() {
            for fact in facts {
                if let Some(b) = booking_from_line(&fact.text, &lang) {
                    if !b.source.is_empty()
                        && !known.contains(&b.source)
                        && !bookings.iter().any(|x| x.source == b.source)
                    {
                        bookings.push(b);
                    }
                }
            }
        }
    }
    let groups: Vec<Group> = group(bookings).into_iter().filter(Group::is_trip).collect();
    let group_of = |sources: &[String]| -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (i, g) in groups.iter().enumerate() {
            let n = sources.iter().filter(|s| g.sources().contains(*s)).count();
            if n > 0 && best.is_none_or(|(_, m)| n > m) {
                best = Some((i, n));
            }
        }
        best.map(|(i, _)| i)
    };
    let mut touched: Vec<String> = Vec::new();
    let mut claimed: HashMap<usize, String> = HashMap::new(); // group → the page that is it (its path)
    for (path, page) in pages {
        let mut path = path;
        let id = page.id();
        let sources = sources_in(&page.body);
        // a page the person renamed or wrote in is theirs: only completed, never renamed, merged or taken out
        let own = auto_titled(&page) && pure(&page);
        match group_of(&sources) {
            None if !own => {}
            None => {
                // not a trip by the rule: back to plain records, its lines kept under the bookings' table
                demote_locked(config, &path, &page, who, &lang)?;
                done.demoted += 1;
            }
            Some(g) => {
                if let Some(into) = claimed.get(&g).cloned().filter(|_| own) {
                    // the same trip twice: merged into the first
                    if super::merge_locked(config, Part::Main, &path, &into, who)?.is_some() {
                        done.merged += 1;
                        complete_locked(config, &into, &groups[g], who, &lang, &mut done)?;
                    }
                } else {
                    let group = &groups[g];
                    let wanted = group.title(&lang);
                    if auto_titled(&page) && page.title != wanted {
                        path = rename_locked(config, &path, &wanted, who)?;
                        done.renamed += 1;
                    }
                    complete_locked(config, &path, group, who, &lang, &mut done)?;
                    claimed.insert(g, path.clone());
                }
            }
        }
        if !id.is_empty() {
            touched.push(id);
        }
    }
    // new trips: a group no page is about yet, with a booking no trip page took before, recent enough
    let trips: Vec<(String, Page)> =
        super::all_pages(config, Part::Main).into_iter().filter(|(_, p)| p.kind() == "trip").collect();
    let mut new_sources: Vec<String> = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        if claimed.contains_key(&i) {
            new_sources.extend(g.sources());
            continue;
        }
        let covered = trips.iter().any(|(_, p)| {
            let (s, e) = super::trip::dates_of(p);
            let place = p.front.get("destination").and_then(serde_json::Value::as_str).map(str::to_string);
            let overlaps = match (s, e) {
                (Some(s), Some(e)) => {
                    s <= g.end + chrono::Duration::days(GAP_DAYS) && e >= g.start - chrono::Duration::days(GAP_DAYS)
                }
                _ => false,
            };
            let same_place = g.keys().iter().any(|k| {
                place.as_deref().map(|d| fold(d).contains(k.as_str())).unwrap_or(false)
                    || fold(&p.title).contains(k.as_str())
            });
            (overlaps && same_place) || sources_in(&p.body).iter().any(|s| g.sources().contains(s))
        });
        let fresh = g.sources().iter().any(|s| !used.contains(s));
        if covered || !fresh || (today - g.end).num_days() > NEW_TRIP_DAYS {
            continue;
        }
        create_locked(config, g, who, &lang)?;
        new_sources.extend(g.sources());
        done.created += 1;
    }
    if !touched.is_empty() || !new_sources.is_empty() {
        super::update_state(config, |s| {
            let mut all = super::strings(s.get("trips_auto_done"));
            for id in &touched {
                if !all.contains(id) {
                    all.push(id.clone());
                }
            }
            s.insert("trips_auto_done".into(), json!(all));
            let mut used = super::strings(s.get("trips_auto_sources"));
            for src in &new_sources {
                if !used.contains(src) {
                    used.push(src.clone());
                }
            }
            let over = used.len().saturating_sub(5000);
            used.drain(..over);
            s.insert("trips_auto_sources".into(), json!(used));
        })?;
    }
    Ok(done)
}

/// The folder a trip goes in: `Voyages`, or `Archives/Voyages` once it is over.
pub fn folder_for(w: &super::Words, end: Option<NaiveDate>, today: NaiveDate) -> String {
    match end {
        Some(e) if e < today => w.past_trips(),
        _ => w.trips.to_string(),
    }
}

fn create_locked(config: &Config, g: &Group, who: &Who, lang: &str) -> anyhow::Result<String> {
    let w = super::words(lang);
    let title = g.title(lang);
    let mut page = Page::new("trip", &title);
    page.front.insert("origin".into(), json!("archive"));
    page.front.insert("auto_title".into(), json!(title));
    page.front.insert("start".into(), json!(g.start.format("%Y-%m-%d").to_string()));
    page.front.insert("end".into(), json!(trip_end(g).format("%Y-%m-%d").to_string()));
    page.front.insert("destination".into(), json!(g.places.join(", ")));
    let mut body = String::new();
    for b in &g.bookings {
        let section = template::title("trip", section_of(b), lang);
        body = super::add_facts(&body, &section, &[line_of(b, lang)], who, &super::today()).0;
    }
    page.body = template::normalize("trip", lang, w.brief, &body).body;
    page.touch();
    let today = chrono::Local::now().date_naive();
    let path = super::free_path(config, Part::Main, &folder_for(w, Some(trip_end(g)), today), &title, "");
    super::commit(config, Part::Main, &path, None, Some(&page.render()), who, "create", &title)?;
    rename_bookings_locked(config, g, &title, who)?;
    Ok(path)
}

/// A group's last day: its last booking's, or the day after a stay that ends it.
fn trip_end(g: &Group) -> NaiveDate {
    let last = g.bookings.iter().map(|b| b.date).max().unwrap_or(g.end);
    if g.bookings.iter().any(|b| b.date == last && b.kind == "lodging") {
        last + chrono::Duration::days(1)
    } else {
        last
    }
}

/// A trip page given the bookings of its group it lacks, its dates and destination (the extraction's pages only).
fn complete_locked(
    config: &Config,
    path: &str,
    g: &Group,
    who: &Who,
    lang: &str,
    done: &mut Synced,
) -> anyhow::Result<()> {
    let Some(before) = super::read_page_text(config, Part::Main, path) else { return Ok(()) };
    let mut page = Page::parse(&before, "");
    let have = sources_in(&page.body);
    let mut body = page.body.clone();
    let mut added = 0;
    for b in &g.bookings {
        if !b.source.is_empty() && have.contains(&b.source) {
            continue;
        }
        let section = template::title("trip", section_of(b), lang);
        let (next, new) = super::add_facts(&body, &section, &[line_of(b, lang)], who, &super::today());
        body = next;
        added += new.len();
    }
    page.body = template::normalize("trip", lang, super::words(lang).brief, &body).body;
    let start = g.start.min(super::trip::dates_of(&page).0.unwrap_or(g.start));
    let end = trip_end(g).max(super::trip::dates_of(&page).1.unwrap_or(g.end));
    page.front.insert("start".into(), json!(start.format("%Y-%m-%d").to_string()));
    page.front.insert("end".into(), json!(end.format("%Y-%m-%d").to_string()));
    if page.front.get("destination").and_then(serde_json::Value::as_str).unwrap_or("").is_empty()
        || page.front.get("origin").is_none()
    {
        page.front.insert("destination".into(), json!(g.places.join(", ")));
    }
    if page.front.get("origin").is_none() {
        page.front.insert("origin".into(), json!("archive"));
        page.front.insert("auto_title".into(), json!(page.title.clone()));
    }
    let after = page.render();
    if after != before {
        page.touch();
        super::commit(config, Part::Main, path, Some(&before), Some(&page.render()), who, "trip", &page.title.clone())?;
        if added > 0 {
            done.completed += 1;
        }
    }
    rename_bookings_locked(config, g, &page.title, who)
}

/// A trip page renamed (its old name kept as an alias, the links to it pointed to the new one): its new path.
pub fn rename_locked(config: &Config, path: &str, title: &str, who: &Who) -> anyhow::Result<String> {
    let Some(before) = super::read_page_text(config, Part::Main, path) else { return Ok(path.to_string()) };
    let mut page = Page::parse(&before, "");
    let old = page.title.clone();
    let mut aliases = page.aliases();
    aliases.push(old.clone());
    page.title = title.to_string();
    page.set_aliases(&aliases);
    page.front.insert("auto_title".into(), json!(title));
    page.touch();
    let folder = path.rsplit_once('/').map(|(f, _)| f.to_string()).unwrap_or_default();
    let lang = super::language(config);
    let today = chrono::Local::now().date_naive();
    // its folder: the trips' (a page among the subjects leaves them)
    let w = super::words(&lang);
    let folder = if folder == w.past_trips() || folder == w.trips {
        folder
    } else {
        folder_for(w, super::trip::dates_of(&page).1, today)
    };
    let to = super::free_path(config, Part::Main, &folder, title, path);
    super::move_page_locked(config, Part::Main, path, &to, &page.render(), who, "rename", &format!("{old} → {title}"))?;
    relink_locked(config, &old, title, who)?;
    Ok(to)
}

/// The links to `old` in the other pages pointed to `new` (« [[Lisbonne]] » → « [[Lisbonne — nov. 2025|Lisbonne]] »).
fn relink_locked(config: &Config, old: &str, new: &str, who: &Who) -> anyhow::Result<usize> {
    let pattern = Regex::new(&format!(r"\[\[{}(\|[^\]]*)?\]\]", regex::escape(old)))?;
    let mut n = 0;
    for (path, page) in super::all_pages(config, Part::Main) {
        if page.title == new {
            continue;
        }
        let Some(before) = super::read_page_text(config, Part::Main, &path) else { continue };
        if !pattern.is_match(&before) {
            continue;
        }
        let after = pattern
            .replace_all(&before, |c: &regex::Captures| match c.get(1) {
                Some(shown) => format!("[[{new}{}]]", shown.as_str()),
                None => format!("[[{new}|{old}]]"),
            })
            .to_string();
        super::commit(config, Part::Main, &path, Some(&before), Some(&after), who, "links", &format!("{old} → {new}"))?;
        n += 1;
    }
    Ok(n)
}

/// The bookings of a trip named after it in their « trip » column (in the live table and the archived one).
fn rename_bookings_locked(config: &Config, g: &Group, title: &str, who: &Who) -> anyhow::Result<()> {
    let Some(t) = records::kind("booking") else { return Ok(()) };
    let sources = g.sources();
    let lang = super::language(config);
    for archived in [false, true] {
        let (path, before, mut page, mut table) = records::read_at(config, t, archived);
        if before.is_none() {
            continue;
        }
        let mut changed = false;
        for row in table.rows.iter_mut() {
            if sources.contains(&tk::str_or_empty(row.get("source"))) && tk::str_or_empty(row.get("trip")) != title {
                row.insert("trip".into(), json!(title));
                changed = true;
            }
        }
        if changed {
            page.body = records::render_body(t, &lang, &table);
            page.touch();
            super::commit(config, Part::Main, &path, before.as_deref(), Some(&page.render()), who, "trip", title)?;
        }
    }
    Ok(())
}

/// A page the extraction made that is no trip by the rule: its lines under the bookings' table (« Réservations »), a
/// heading with its title over them, then the page taken out (in its history: restoring it brings it back, and it is
/// then left as it is). Nothing dropped.
fn demote_locked(config: &Config, path: &str, page: &Page, who: &Who, lang: &str) -> anyhow::Result<()> {
    let Some(t) = records::kind("booking") else { return Ok(()) };
    let (rpath, before, mut rpage, table) = records::read_at(config, t, false);
    let mut lines: Vec<String> = Vec::new();
    for (section, facts) in page.sections() {
        for fact in facts {
            let line = if fact.who.is_empty() {
                fact.text.clone()
            } else {
                format!("{} — *{}, {}*", fact.text, fact.who, fact.date)
            };
            let line = if section.is_empty() { line } else { format!("{section} : {line}") };
            lines.push(format!("- {line}"));
        }
    }
    let mut after = table.after.clone();
    while after.last().is_some_and(|l| l.trim().is_empty()) {
        after.pop();
    }
    if !after.is_empty() {
        after.push(String::new());
    }
    after.push(format!("## {}", page.title));
    after.extend(lines);
    let table = records::Table { before: table.before, rows: table.rows, after };
    rpage.body = records::render_body(t, lang, &table);
    rpage.touch();
    let summary = format!("{} → {}", page.title, t.titles[lang_index(lang)]);
    super::commit(config, Part::Main, &rpath, before.as_deref(), Some(&rpage.render()), who, "demote", &summary)?;
    let text = super::read_page_text(config, Part::Main, path);
    super::commit(config, Part::Main, path, text.as_deref(), None, who, "demote", &summary)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(date: &str, what: &str, trip: &str) -> Booking {
        let mut f = Dict::new();
        f.insert("date".into(), json!(date));
        f.insert("what".into(), json!(what));
        if !trip.is_empty() {
            f.insert("trip".into(), json!(trip));
        }
        f.insert(
            "source".into(),
            json!(format!("archive:vie/mails/{:016x}", fold(&format!("{date}{what}")).len() * 7919 + date.len())),
        );
        Booking::new(&f, "fr").unwrap()
    }

    #[test]
    fn places_are_cities() {
        assert_eq!(city("LIS", "fr").as_deref(), Some("Lisbonne"));
        assert_eq!(city("Aéroport Paris-Charles de Gaulle (CDG)", "fr").as_deref(), Some("Paris"));
        assert_eq!(city("Lisbon", "fr").as_deref(), Some("Lisbonne"));
        assert_eq!(city("Aéroport de Faro", "fr").as_deref(), Some("Faro"));
        assert_eq!(city("XYZ", "fr"), None);
        assert_eq!(ends("Vol AF1234 Paris → Lisbonne"), (Some("Paris".into()), Some("Lisbonne".into())));
        assert_eq!(ends("Train de Lyon à Paris"), (Some("Lyon".into()), Some("Paris".into())));
        assert_eq!(ends("CDG-LIS"), (Some("CDG".into()), Some("LIS".into())));
        assert_eq!(booking_kind("Hôtel Alfama", ""), "lodging");
        assert_eq!(booking_kind("Table pour 4", "Le Comptoir"), "event");
        assert_eq!(booking_kind("Vol TP433", "TAP"), "transport");
    }

    #[test]
    fn only_time_away_is_a_trip() {
        // outbound + return two nights apart: one trip, named by the city and the month
        let g =
            group(vec![b("2025-11-12", "Vol Paris → Lisbonne", "LIS"), b("2025-11-16", "Vol Lisbonne → Paris", "")]);
        assert_eq!(g.len(), 1, "{g:?}");
        assert!(g[0].is_trip());
        assert_eq!(g[0].title("fr"), "Lisbonne — nov. 2025");
        // a weekend with a hotel
        let g = group(vec![b("2026-03-13", "Hôtel à Lyon", ""), b("2026-03-13", "Train Paris → Lyon", "")]);
        assert_eq!(g.len(), 1);
        assert!(g[0].is_trip());
        assert_eq!(super::title(&g[0].places, g[0].start, trip_end(&g[0]), "fr"), "Week-end à Lyon — mars 2026");
        // a single commute train, an isolated leg, a dinner: no trip
        assert!(group(vec![b("2026-02-03", "Train Paris → Lyon", "")]).iter().all(|g| !g.is_trip()));
        assert!(group(vec![b("2026-02-03", "Train Paris → Lyon", ""), b("2026-02-03", "Train Lyon → Paris", "")])
            .iter()
            .all(|g| !g.is_trip()));
        assert!(group(vec![b("2026-02-03", "Table au restaurant Septime", "Paris")]).iter().all(|g| !g.is_trip()));
        // named as a trip
        let g = group(vec![b("2026-07-01", "Vol vers Rome", "Vacances à Rome")]);
        assert!(g[0].is_trip());
        assert_eq!(g[0].title("en"), "Rome — Jul 2026");
    }
}
