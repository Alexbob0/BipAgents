//! Records (« Fiches », the owner 2026-10-23): useful facts kept precisely instead of in vague summaries, the structure
//! the extraction from the person's archive will fill later. Five types, one page each in the vault's « Fiches »
//! folder (Records, Unterlagen, Registros), a Markdown table Obsidian shows as it is:
//!
//!     Fiches/Achats.md                    purchases: date, item, store, price, end of warranty, source
//!     Fiches/Abonnements.md               subscriptions: service, price, period, next renewal, how to cancel, source
//!     Fiches/Contrats et assurances.md    contracts and insurance: type, provider, number, end, source
//!     Fiches/Papiers.md                   papers: document, expiry, source
//!     Fiches/Réservations.md              bookings: date, what, with whom, reference, trip, source (they feed trips)
//!
//! Each row has a stable id (`r` and 6 hex digits, its last column). The agents add and update rows through
//! notebook_record; the same record given twice is merged on its obvious key (the item and its date, the service, the
//! contract's number, the document, the booking's reference). A purchase has a class (`class`: durable goods, a service,
//! a ticket or registration, food, digital content, travel, a fee, other — given by the agent or the extraction's
//! model, else guessed from its words and price by `classify`). Only durable goods get the EU's two-year legal
//! guarantee when their end of warranty is unknown, marked so (the owner, 2026-10-23: no « garantie légale » on a 3 €
//! registration); only goods from `[notebook] warranty_min_eur` (50 € by default) get its reminder and count among the
//! « important » purchases the apps show first. `source` holds the reference of the document a record comes from (the archive's,
//! an opaque text), shown as a link when the apps get a route for it. Text the person writes around the table in
//! Obsidian is kept.
//!
//! The nightly pass proposes reminders (never adds them by itself): a warranty ending within 30 days, a renewal within
//! 7, a contract's end within 30, a paper expiring within 60 — as Carnet task proposals (mod.rs `propose`, within its
//! weekly budget), each record's date once.
//!
//! What is past moves to the same table in the archives (`Archives/Fiches/Achats.md`, archive.rs decides when): the
//! row as it was, its id kept. `records` reads the live table, `archived_records` the other; the tool's list shows both.

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::{fold, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

pub struct Field {
    pub id: &'static str,
    pub titles: [&'static str; 4],
}

pub struct Kind {
    pub id: &'static str,
    /// Its page's title (fr, en, de, es).
    pub titles: [&'static str; 4],
    /// Other names an agent may give it (folded).
    pub aliases: &'static [&'static str],
    pub fields: &'static [Field],
    /// The field holding its next date (the one sorted on and reminded of).
    pub next: &'static str,
    /// The keys a record is the same one on, tried in order (the first whose fields the new record all has).
    pub keys: &'static [&'static [&'static str]],
    /// A reminder is proposed this many days before its next date (0: never).
    pub window: i64,
    /// The proposal's kind (mod.rs PROPOSAL_KINDS).
    pub proposal: &'static str,
}

const SOURCE: Field = Field { id: "source", titles: ["Source", "Source", "Quelle", "Fuente"] };
const CLASS: Field = Field { id: "class", titles: ["Catégorie", "Kind", "Art", "Categoría"] };
const PRICE: Field = Field { id: "price", titles: ["Prix", "Price", "Preis", "Precio"] };

pub const TYPES: [Kind; 5] = [
    Kind {
        id: "purchase",
        titles: ["Achats", "Purchases", "Käufe", "Compras"],
        aliases: &["achat", "purchases", "kauf", "compra", "garantie", "garanties", "warranty", "warranties"],
        fields: &[
            Field { id: "date", titles: ["Date", "Date", "Datum", "Fecha"] },
            Field { id: "item", titles: ["Objet", "Item", "Gegenstand", "Artículo"] },
            Field { id: "store", titles: ["Magasin", "Store", "Geschäft", "Tienda"] },
            PRICE,
            CLASS,
            Field {
                id: "warranty_end",
                titles: ["Fin de garantie", "Warranty ends", "Garantie bis", "Fin de garantía"],
            },
            SOURCE,
        ],
        next: "warranty_end",
        keys: &[&["item", "date"], &["item", "store"], &["item"]],
        window: 30,
        proposal: "warranty",
    },
    Kind {
        id: "subscription",
        titles: ["Abonnements", "Subscriptions", "Abonnements", "Suscripciones"],
        aliases: &["abonnement", "subscriptions", "abo", "abos", "suscripcion"],
        fields: &[
            Field { id: "service", titles: ["Service", "Service", "Dienst", "Servicio"] },
            PRICE,
            Field { id: "period", titles: ["Période", "Period", "Zeitraum", "Periodo"] },
            Field {
                id: "renewal",
                titles: ["Prochain renouvellement", "Next renewal", "Nächste Verlängerung", "Próxima renovación"],
            },
            Field { id: "cancel", titles: ["Comment résilier", "How to cancel", "Kündigung", "Cómo cancelar"] },
            SOURCE,
        ],
        next: "renewal",
        keys: &[&["service"]],
        window: 7,
        proposal: "renewal",
    },
    Kind {
        id: "contract",
        titles: [
            "Contrats et assurances",
            "Contracts and insurance",
            "Verträge und Versicherungen",
            "Contratos y seguros",
        ],
        aliases: &[
            "contrat",
            "contrats",
            "assurance",
            "assurances",
            "contracts",
            "insurance",
            "vertrag",
            "vertrage",
            "versicherung",
            "versicherungen",
            "contrato",
            "contratos",
            "seguro",
            "seguros",
        ],
        fields: &[
            Field { id: "what", titles: ["Type", "Type", "Art", "Tipo"] },
            Field { id: "provider", titles: ["Organisme", "Provider", "Anbieter", "Entidad"] },
            Field { id: "number", titles: ["Numéro", "Number", "Nummer", "Número"] },
            Field { id: "end", titles: ["Échéance", "Ends", "Ablauf", "Vencimiento"] },
            SOURCE,
        ],
        next: "end",
        keys: &[&["number"], &["what", "provider"]],
        window: 30,
        proposal: "deadline",
    },
    Kind {
        id: "paper",
        titles: ["Papiers", "Papers", "Ausweise und Papiere", "Documentos personales"],
        aliases: &[
            "papier",
            "papers",
            "documents d identite",
            "identity documents",
            "ausweis",
            "ausweise",
            "papiere",
            "documentos",
            "papeles",
        ],
        fields: &[
            Field { id: "document", titles: ["Document", "Document", "Dokument", "Documento"] },
            Field { id: "expires", titles: ["Expiration", "Expires", "Gültig bis", "Caduca"] },
            SOURCE,
        ],
        next: "expires",
        keys: &[&["document"]],
        window: 60,
        proposal: "expiry",
    },
    Kind {
        id: "booking",
        titles: ["Réservations", "Bookings", "Buchungen", "Reservas"],
        aliases: &["reservation", "bookings", "buchung", "reserva", "reservaciones"],
        fields: &[
            Field { id: "date", titles: ["Date", "Date", "Datum", "Fecha"] },
            Field { id: "what", titles: ["Quoi", "What", "Was", "Qué"] },
            Field { id: "provider", titles: ["Chez", "With", "Bei", "Con"] },
            Field { id: "reference", titles: ["Référence", "Reference", "Buchungsnummer", "Localizador"] },
            Field { id: "trip", titles: ["Voyage", "Trip", "Reise", "Viaje"] },
            SOURCE,
        ],
        next: "date",
        keys: &[&["reference"], &["what", "date"]],
        window: 0,
        proposal: "deadline",
    },
];

/// Every field id of every type (the tool's schema).
pub fn all_fields() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for t in &TYPES {
        for f in t.fields {
            if !out.contains(&f.id) {
                out.push(f.id);
            }
        }
    }
    out
}

/// The dates among the fields: written as YYYY-MM-DD when they can be read.
const DATES: [&str; 5] = ["date", "warranty_end", "renewal", "end", "expires"];

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

/// A type by its id, its title in any language or another name.
pub fn kind(name: &str) -> Option<&'static Kind> {
    let wanted = fold(name);
    TYPES.iter().find(|t| {
        fold(t.id) == wanted || t.titles.iter().any(|x| fold(x) == wanted) || t.aliases.contains(&wanted.as_str())
    })
}

fn kind_of_page(page: &Page) -> Option<&'static Kind> {
    if page.kind() != "records" {
        return None;
    }
    kind(&tk::str_or_empty(page.front.get("records")))
}

pub fn path_of(lang: &str, t: &Kind) -> String {
    format!("{}/{}.md", super::words(lang).records, t.titles[lang_index(lang)])
}

/// The same table in the archives: `Archives/Fiches/Achats.md`.
pub fn archive_path_of(lang: &str, t: &Kind) -> String {
    let w = super::words(lang);
    format!("{}/{}/{}.md", w.archives, w.records, t.titles[lang_index(lang)])
}

fn field_of(t: &Kind, header: &str) -> Option<&'static str> {
    let wanted = fold(header);
    t.fields
        .iter()
        .find(|f| fold(f.id) == wanted || f.titles.iter().any(|x| fold(x) == wanted))
        .map(|f| f.id)
        .or(if wanted == "id" { Some("id") } else { None })
}

// -- a purchase's class ----------------------------------------------------------------------------------------------

/// The classes of a purchase (`class`): its id and its word in the table (fr, en, de, es). Only `goods` (durable goods
/// bought from a professional) have the legal guarantee.
pub const CLASSES: [(&str, [&str; 4]); 8] = [
    ("goods", ["bien durable", "durable goods", "Gebrauchsgut", "bien duradero"]),
    ("service", ["service", "service", "Dienstleistung", "servicio"]),
    ("ticket", ["billet ou inscription", "ticket or registration", "Ticket oder Anmeldung", "entrada o inscripción"]),
    ("food", ["alimentation", "food", "Lebensmittel", "comida"]),
    ("digital", ["numérique", "digital", "digital", "digital"]),
    ("travel", ["voyage", "travel", "Reise", "viaje"]),
    ("fee", ["frais", "fee", "Gebühr", "tasa"]),
    ("other", ["autre", "other", "Sonstiges", "otro"]),
];

/// A class's id from its id or its word in any language (None: not a class).
pub fn class_id(text: &str) -> Option<&'static str> {
    let wanted = fold(text);
    if wanted.is_empty() {
        return None;
    }
    CLASSES.iter().find(|(id, words)| fold(id) == wanted || words.iter().any(|w| fold(w) == wanted)).map(|(id, _)| *id)
}

fn class_word(id: &str, lang: &str) -> String {
    CLASSES.iter().find(|(c, _)| *c == id).map(|(_, w)| w[lang_index(lang)].to_string()).unwrap_or_else(|| id.into())
}

/// Words that tell a purchase's class (folded, whole words or phrases), tried in this order: the first class whose
/// words are in the item or the store wins.
const CLASS_WORDS: [(&str, &[&str]); 7] = [
    (
        "fee",
        &[
            "frais",
            "fee",
            "fees",
            "commission",
            "amende",
            "penalite",
            "timbre fiscal",
            "taxe",
            "gebuhr",
            "gebuhren",
            "tasa",
            "tasas",
            "comision",
            "frais de port",
            "frais de dossier",
            "shipping",
            "porto",
            "envio",
        ],
    ),
    (
        "ticket",
        &[
            "billet",
            "billets",
            "ticket",
            "tickets",
            "entree",
            "entrees",
            "inscription",
            "inscriptions",
            "registration",
            "concert",
            "spectacle",
            "festival",
            "match",
            "cinema",
            "musee",
            "evenement",
            "event",
            "conference",
            "seminaire",
            "webinar",
            "webinaire",
            "atelier",
            "workshop",
            "salon",
            "exposition",
            "eintritt",
            "eintrittskarte",
            "konzert",
            "anmeldung",
            "veranstaltung",
            "entrada",
            "entradas",
            "inscripcion",
            "evento",
            "museo",
            "theatre",
            "theater",
            "teatro",
            "place de concert",
            "pass",
        ],
    ),
    (
        "travel",
        &[
            "vol",
            "vols",
            "flight",
            "train",
            "tgv",
            "sncf",
            "ouigo",
            "hotel",
            "airbnb",
            "taxi",
            "uber",
            "bus",
            "peage",
            "parking",
            "ferry",
            "sejour",
            "location de voiture",
            "car rental",
            "bahn",
            "flug",
            "zug",
            "vuelo",
            "renfe",
            "ryanair",
            "easyjet",
            "air france",
            "transavia",
            "vueling",
            "blablacar",
            "eurostar",
            "navigo",
            "nuitee",
            "nuitees",
            "hebergement",
            "booking com",
        ],
    ),
    (
        "food",
        &[
            "restaurant",
            "repas",
            "menu",
            "pizza",
            "sushi",
            "burger",
            "deliveroo",
            "uber eats",
            "ubereats",
            "just eat",
            "boulangerie",
            "traiteur",
            "epicerie",
            "supermarche",
            "courses",
            "alimentation",
            "dejeuner",
            "diner",
            "brunch",
            "lebensmittel",
            "essen",
            "comida",
            "cena",
            "almuerzo",
            "groceries",
            "grocery",
            "takeaway",
            "frichti",
            "picard",
        ],
    ),
    (
        "digital",
        &[
            "application",
            "app",
            "apps",
            "ebook",
            "e book",
            "kindle",
            "app store",
            "google play",
            "steam",
            "logiciel",
            "software",
            "licence",
            "license",
            "telechargement",
            "download",
            "vod",
            "itunes",
            "spotify",
            "netflix",
            "icloud",
            "stockage en ligne",
            "in app",
            "credits",
            "dlc",
            "abonnement numerique",
            "jeu video en ligne",
            "lizenz",
            "aplicacion",
        ],
    ),
    (
        "service",
        &[
            "abonnement",
            "subscription",
            "forfait",
            "reparation",
            "repair",
            "entretien",
            "revision",
            "cours",
            "lecon",
            "formation",
            "coiffeur",
            "menage",
            "nettoyage",
            "pressing",
            "assurance",
            "cotisation",
            "adhesion",
            "membership",
            "consultation",
            "seance",
            "massage",
            "installation",
            "intervention",
            "plombier",
            "electricien",
            "garde",
            "location",
            "rental",
            "livraison",
            "delivery",
            "service",
            "services",
            "dienstleistung",
            "reparatur",
            "wartung",
            "kurs",
            "servicio",
            "reparacion",
            "clase",
            "mutuelle",
            "loyer",
            "facture",
            "electricite",
            "gaz",
            "internet",
            "mobile",
        ],
    ),
    (
        "goods",
        &[
            "lave linge",
            "lave vaisselle",
            "seche linge",
            "machine",
            "frigo",
            "refrigerateur",
            "congelateur",
            "televiseur",
            "tv",
            "television",
            "ordinateur",
            "laptop",
            "macbook",
            "iphone",
            "smartphone",
            "telephone",
            "tablette",
            "ipad",
            "casque",
            "ecouteurs",
            "airpods",
            "aspirateur",
            "four",
            "micro ondes",
            "velo",
            "trottinette",
            "montre",
            "appareil photo",
            "camera",
            "console",
            "playstation",
            "xbox",
            "nintendo",
            "imprimante",
            "ecran",
            "moniteur",
            "clavier",
            "souris",
            "meuble",
            "canape",
            "lit",
            "matelas",
            "chaise",
            "table",
            "bureau",
            "lampe",
            "chaussures",
            "baskets",
            "vetement",
            "vetements",
            "manteau",
            "veste",
            "jean",
            "sac",
            "valise",
            "jouet",
            "perceuse",
            "outil",
            "outils",
            "poussette",
            "batterie",
            "chargeur",
            "enceinte",
            "routeur",
            "drone",
            "cafetiere",
            "bouilloire",
            "grille pain",
            "blender",
            "mixeur",
            "robot",
            "poele",
            "casserole",
            "pneu",
            "pneus",
            "lunettes",
            "bijou",
            "bague",
            "washing machine",
            "dishwasher",
            "fridge",
            "freezer",
            "computer",
            "phone",
            "headphones",
            "vacuum",
            "bike",
            "bicycle",
            "watch",
            "chair",
            "sofa",
            "couch",
            "bed",
            "mattress",
            "desk",
            "shoes",
            "jacket",
            "printer",
            "monitor",
            "keyboard",
            "speaker",
            "router",
            "waschmaschine",
            "spulmaschine",
            "kuhlschrank",
            "fernseher",
            "handy",
            "kopfhorer",
            "staubsauger",
            "fahrrad",
            "uhr",
            "drucker",
            "lavadora",
            "lavavajillas",
            "nevera",
            "frigorifico",
            "televisor",
            "portatil",
            "movil",
            "auriculares",
            "aspiradora",
            "bicicleta",
            "reloj",
            "impresora",
        ],
    ),
];

/// A price's amount (« 1 299,00 € », « $45.99 », « 12 EUR »), when it has one.
pub fn price_value(text: &str) -> Option<f64> {
    static NUMBER: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\d{1,3}(?:[ \u{a0}\u{202f}.,]\d{3})+(?:[.,]\d{1,2})?|\d+(?:[.,]\d{1,2})?")
            .expect("valid regex")
    });
    let found = NUMBER.find(text)?.as_str();
    let plain: String = found.chars().filter(|c| !c.is_whitespace()).collect();
    // the last « , » or « . » followed by one or two digits is the decimal mark; the others group thousands
    let (whole, cents) = match plain.rfind([',', '.']) {
        Some(i) if plain.len() - i - 1 <= 2 => (&plain[..i], &plain[i + 1..]),
        _ => (plain.as_str(), ""),
    };
    let whole: String = whole.chars().filter(char::is_ascii_digit).collect();
    format!("{whole}.{}", if cents.is_empty() { "0" } else { cents }).parse().ok()
}

/// A purchase's class from its words (item, store) and its price, when no one gave it: the first class whose words
/// are there, else durable goods from `min_eur`, else other (no warranty, no reminder).
pub fn classify(item: &str, store: &str, price: &str, min_eur: f64) -> &'static str {
    let item = format!(" {} ", fold(item));
    let store = format!(" {} ", fold(store));
    // the item's words first: « Machine à café » is goods wherever it was bought
    for text in [&item, &store] {
        for (class, words) in CLASS_WORDS.iter().rev() {
            if *class == "goods" && words.iter().any(|w| text.contains(&format!(" {w} "))) {
                return class;
            }
        }
        for (class, words) in CLASS_WORDS.iter() {
            if *class != "goods" && words.iter().any(|w| text.contains(&format!(" {w} "))) {
                return class;
            }
        }
    }
    match price_value(price) {
        Some(p) if p >= min_eur => "goods",
        _ => "other",
    }
}

/// Whether a purchase is worth showing first (and its warranty a reminder): durable goods from `min_eur`, or a
/// warranty still running that the document or the person stated (the computed legal one counts only for goods
/// worth it: a 10 € cable is not « important »).
pub fn important(record: &Dict, min_eur: f64, today: NaiveDate) -> bool {
    let class = class_id(&tk::str_or_empty(record.get("class"))).unwrap_or("other");
    let goods_worth =
        class == "goods" && price_value(&tk::str_or_empty(record.get("price"))).is_some_and(|p| p >= min_eur);
    let end = tk::str_or_empty(record.get("warranty_end"));
    let stated = !end.is_empty() && !is_legal(&end);
    let running =
        super::date_in(&end).and_then(|(m, d, y)| NaiveDate::from_ymd_opt(y?, m, d)).is_some_and(|d| d >= today);
    goods_worth || (stated && running)
}

/// Whether an end of warranty is the computed legal one (« 2027-03-12 (légale) »).
pub fn is_legal(end: &str) -> bool {
    super::WORDS.iter().any(|w| end.contains(&format!("({})", legal_word(w.lang))))
}

/// A purchase's class set (given or guessed) and its legal warranty kept only for durable goods: computed when
/// unknown, taken off (it was computed, never stated) for the others. `given`: the fields just given.
fn settle_purchase(record: &mut Dict, given: &Dict, lang: &str, min_eur: f64) {
    let class = match class_id(&tk::str_or_empty(record.get("class"))) {
        Some(c) => c,
        None => classify(
            &tk::str_or_empty(record.get("item")),
            &tk::str_or_empty(record.get("store")),
            &tk::str_or_empty(record.get("price")),
            min_eur,
        ),
    };
    record.insert("class".into(), json!(class_word(class, lang)));
    let end = tk::str_or_empty(record.get("warranty_end"));
    let computed = end.is_empty() || is_legal(&end);
    if !computed || given.contains_key("warranty_end") {
        return;
    }
    if class != "goods" {
        record.remove("warranty_end");
        return;
    }
    let bought = tk::str_or_empty(record.get("date"));
    if let Some((m, d, Some(y))) = super::date_in(&bought) {
        if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
            let until = date.checked_add_months(chrono::Months::new(24)).unwrap_or(date);
            record.insert("warranty_end".into(), json!(format!("{} ({})", until.format("%Y-%m-%d"), legal_word(lang))));
        }
    }
}

/// The legal guarantee's mark after a computed end of warranty.
fn legal_word(lang: &str) -> &'static str {
    match lang {
        "fr" => "légale",
        "de" => "gesetzlich",
        "es" => "legal",
        _ => "legal",
    }
}

/// A records page: the text before its table, its rows (field id → text, `id` included), the text after.
pub struct Table {
    pub before: Vec<String>,
    pub rows: Vec<Dict>,
    pub after: Vec<String>,
}

fn cells(line: &str) -> Vec<String> {
    let inner = line.trim().trim_start_matches('|').trim_end_matches('|');
    inner.split('|').map(|c| c.trim().to_string()).collect()
}

pub fn parse(t: &Kind, body: &str) -> Table {
    let lines: Vec<&str> = body.lines().collect();
    let Some(start) = lines.iter().position(|l| l.trim_start().starts_with('|')) else {
        return Table { before: lines.iter().map(|l| l.to_string()).collect(), rows: Vec::new(), after: Vec::new() };
    };
    let end =
        lines[start..].iter().position(|l| !l.trim_start().starts_with('|')).map(|i| start + i).unwrap_or(lines.len());
    let header: Vec<Option<&'static str>> = cells(lines[start]).iter().map(|h| field_of(t, h)).collect();
    let mut rows = Vec::new();
    for line in &lines[start + 1..end] {
        let row = cells(line);
        if row.iter().all(|c| c.chars().all(|ch| "-: ".contains(ch))) {
            continue; // the separator
        }
        let mut record = Dict::new();
        for (cell, field) in row.iter().zip(header.iter()) {
            if let Some(field) = field {
                if !cell.is_empty() {
                    record.insert((*field).into(), json!(cell));
                }
            }
        }
        if !record.is_empty() {
            rows.push(record);
        }
    }
    Table {
        before: lines[..start].iter().map(|l| l.to_string()).collect(),
        rows,
        after: lines[end..].iter().map(|l| l.to_string()).collect(),
    }
}

fn cell(text: &str) -> String {
    tk::squeeze(&text.replace(['|', '\n', '\r'], " / ")).trim().to_string()
}

pub fn render_body(t: &Kind, lang: &str, table: &Table) -> String {
    let i = lang_index(lang);
    let mut out: Vec<String> = table.before.clone();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if !out.is_empty() {
        out.push(String::new());
    }
    let mut header: Vec<&str> = t.fields.iter().map(|f| f.titles[i]).collect();
    header.push("id");
    out.push(format!("| {} |", header.join(" | ")));
    out.push(format!("|{}", " --- |".repeat(header.len())));
    for row in &table.rows {
        let mut values: Vec<String> = t.fields.iter().map(|f| cell(&tk::str_or_empty(row.get(f.id)))).collect();
        values.push(tk::str_or_empty(row.get("id")));
        out.push(format!("| {} |", values.join(" | ")));
    }
    let after: Vec<String> = table.after.iter().skip_while(|l| l.trim().is_empty()).cloned().collect();
    if !after.is_empty() {
        out.push(String::new());
        out.extend(after);
    }
    out.join("\n") + "\n"
}

/// A record's next date (its warranty's end, its renewal, its expiry…), when it can be read.
pub fn next_date(t: &Kind, record: &Dict) -> Option<NaiveDate> {
    let text = tk::str_or_empty(record.get(t.next));
    let (m, d, y) = super::date_in(&text)?;
    NaiveDate::from_ymd_opt(y?, m, d)
}

static TIME: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"\b([01]?\d|2[0-3])[:hH]([0-5]\d)\b").expect("valid regex"));

/// A date written YYYY-MM-DD (and its time HH:MM when it has one), when it can be read; else as it was.
fn iso(text: &str) -> String {
    let text = text.trim();
    match super::date_in(text) {
        Some((m, d, Some(y))) if text.len() <= 30 => match NaiveDate::from_ymd_opt(y, m, d) {
            Some(date) => match TIME.captures(text) {
                Some(c) => format!("{} {:0>2}:{}", date.format("%Y-%m-%d"), &c[1], &c[2]),
                None => date.format("%Y-%m-%d").to_string(),
            },
            None => text.to_string(),
        },
        _ => text.to_string(),
    }
}

/// Whether `old` is the same record as `new` on its type's first key `new` fully has (an empty field of `old`
/// matching anything).
fn same(t: &Kind, old: &Dict, new: &Dict) -> bool {
    let has = |r: &Dict, k: &str| !tk::str_or_empty(r.get(k)).trim().is_empty();
    let Some(key) = t.keys.iter().find(|key| key.iter().all(|k| has(new, k))) else { return false };
    has(old, key[0])
        && key
            .iter()
            .all(|k| !has(old, k) || fold(&tk::str_or_empty(old.get(*k))) == fold(&tk::str_or_empty(new.get(*k))))
}

fn read(config: &Config, t: &Kind) -> (String, Option<String>, Page, Table) {
    read_at(config, t, false)
}

/// A type's live table, or the archived one (`archived`).
pub(crate) fn read_at(config: &Config, t: &Kind, archived: bool) -> (String, Option<String>, Page, Table) {
    let lang = super::language(config);
    let path = if archived { archive_path_of(&lang, t) } else { path_of(&lang, t) };
    let before = super::read_page_text(config, Part::Main, &path);
    let title = t.titles[lang_index(&lang)];
    let page = match &before {
        Some(text) => Page::parse(text, title),
        None => {
            let mut p = Page::new("records", title);
            p.front.remove("aliases");
            p.front.insert("records".into(), json!(t.id));
            if archived {
                p.front.insert("archived".into(), json!(true));
            }
            p
        }
    };
    let table = parse(t, &page.body);
    (path, before, page, table)
}

/// A record's fields as `upsert` keeps them: its type's fields only, one line each, dates written YYYY-MM-DD.
pub fn prepare(t: &Kind, fields: &Dict) -> Dict {
    let mut new = Dict::new();
    for f in t.fields {
        let value = tk::str_or_empty(fields.get(f.id));
        let value = tk::head(&cell(&value), 300).to_string();
        if !value.is_empty() {
            let value = if DATES.contains(&f.id) { iso(&value) } else { value };
            new.insert(f.id.into(), json!(value));
        }
    }
    new
}

/// The row of `rows` that is the same record as `new` (prepared), on its type's key.
pub fn find_same(t: &Kind, rows: &[Dict], new: &Dict) -> Option<usize> {
    rows.iter().position(|r| same(t, r, new))
}

/// Whether `value` is one of the date fields (written YYYY-MM-DD).
pub fn is_date_field(field: &str) -> bool {
    DATES.contains(&field)
}

/// What `upsert` did.
#[derive(Debug, Clone, PartialEq)]
pub enum Upserted {
    Added,
    Updated,
    Same,
}

/// A record added, or merged into the one it is (by `id`, else by its type's key); `update_only`: refused when there
/// is none. Its dates written YYYY-MM-DD, a purchase's end of warranty computed when unknown (two years, marked
/// legal). Returns the record and what was done.
pub fn upsert(
    config: &Config,
    t: &'static Kind,
    id: &str,
    fields: &Dict,
    update_only: bool,
    who: &Who,
) -> anyhow::Result<(Dict, Upserted)> {
    let _guard = super::lock();
    let lang = super::language(config);
    let new = prepare(t, fields);
    let id = id.trim();
    let position = |table: &Table| {
        if id.is_empty() {
            table.rows.iter().position(|r| same(t, r, &new))
        } else {
            table.rows.iter().position(|r| tk::str_or_empty(r.get("id")) == id)
        }
    };
    // the live table, else the archived one when the record is there (it stays there, updated)
    let (mut path, mut before, mut page, mut table) = read(config, t);
    let mut index = position(&table);
    if index.is_none() {
        let archived = read_at(config, t, true);
        if let Some(i) = position(&archived.3) {
            (path, before, page, table) = archived;
            index = Some(i);
        }
    }
    if index.is_none() && !id.is_empty() {
        return Err(super::NotebookError(format!("no {} record with id {id} (action list gives them)", t.id)).into());
    }
    if index.is_none() && update_only {
        return Err(
            super::NotebookError(format!("no such {} record: give its id (action list gives them)", t.id)).into()
        );
    }
    if index.is_none() && new.is_empty() {
        return Err(super::NotebookError("give the record's fields".into()).into());
    }
    let mut record = match index {
        Some(i) => table.rows[i].clone(),
        None => {
            let mut r = Dict::new();
            r.insert("id".into(), json!(format!("r{}", py::token_hex(3))));
            r
        }
    };
    let was = record.clone();
    for (k, v) in &new {
        // the same value written another way (« lave linge » for « Lave-linge ») keeps the record's spelling
        let kept = tk::str_or_empty(record.get(k));
        if kept.is_empty() || fold(&kept) != fold(&tk::str_or_empty(Some(v))) {
            record.insert(k.clone(), v.clone());
        }
    }
    // its class; the EU's legal guarantee (two years) for durable goods when the end of warranty is unknown, or was
    // computed from an older date
    if t.id == "purchase" {
        if new.contains_key("class") && class_id(&tk::str_or_empty(new.get("class"))).is_none() {
            record.remove("class");
        }
        settle_purchase(&mut record, &new, &lang, config.notebook.warranty_min_eur);
    }
    let done = match index {
        Some(_) if record == was => return Ok((record, Upserted::Same)),
        Some(i) => {
            table.rows[i] = record.clone();
            Upserted::Updated
        }
        None => {
            table.rows.push(record.clone());
            Upserted::Added
        }
    };
    page.body = render_body(t, &lang, &table);
    page.touch();
    let summary = summary_of(t, &record);
    let action = if done == Upserted::Added { "record_add" } else { "record_update" };
    super::commit(config, Part::Main, &path, before.as_deref(), Some(&page.render()), who, action, &summary)?;
    Ok((record, done))
}

/// A record taken out (Ok(None) when there is no such id).
pub fn remove(config: &Config, t: &'static Kind, id: &str, who: &Who) -> anyhow::Result<Option<Dict>> {
    let _guard = super::lock();
    let lang = super::language(config);
    let find = |table: &Table| table.rows.iter().position(|r| tk::str_or_empty(r.get("id")) == id.trim());
    let (mut path, mut before, mut page, mut table) = read(config, t);
    let mut index = find(&table);
    if index.is_none() {
        let archived = read_at(config, t, true);
        index = find(&archived.3);
        (path, before, page, table) = archived;
    }
    let Some(index) = index else {
        return Ok(None);
    };
    let record = table.rows.remove(index);
    page.body = render_body(t, &lang, &table);
    page.touch();
    super::commit(
        config,
        Part::Main,
        &path,
        before.as_deref(),
        Some(&page.render()),
        who,
        "record_remove",
        &format!("− {}", summary_of(t, &record)),
    )?;
    Ok(Some(record))
}

/// A record in one line (the history, the tool's answers): its fields in order, « · » between them.
pub fn summary_of(t: &Kind, record: &Dict) -> String {
    t.fields
        .iter()
        .map(|f| tk::str_or_empty(record.get(f.id)))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A type's records (no page yet: none).
pub fn records(config: &Config, t: &Kind) -> Vec<Dict> {
    let lang = super::language(config);
    match super::read_page_text(config, Part::Main, &path_of(&lang, t)) {
        Some(text) => parse(t, &Page::parse(&text, "").body).rows,
        None => Vec::new(),
    }
}

/// A type's archived records (archive.rs moved them: past, ended, replaced).
pub fn archived_records(config: &Config, t: &Kind) -> Vec<Dict> {
    let lang = super::language(config);
    match super::read_page_text(config, Part::Main, &archive_path_of(&lang, t)) {
        Some(text) => parse(t, &Page::parse(&text, "").body).rows,
        None => Vec::new(),
    }
}

/// Every purchase of a page given its class (guessed when it has none) and its legal warranty only if it is durable
/// goods: the migration of 2026-10-23, at each nightly pass (idempotent: nothing to do once each row has its class).
/// Returns how many rows changed.
pub fn settle_classes(config: &Config) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let lang = super::language(config);
    let Some(t) = kind("purchase") else { return Ok(0) };
    let who = Who::tidy(config);
    let mut changed = 0;
    for archived in [false, true] {
        let (path, before, mut page, mut table) = read_at(config, t, archived);
        if before.is_none() {
            continue;
        }
        let mut lines: Vec<String> = Vec::new();
        for row in table.rows.iter_mut() {
            let was = row.clone();
            settle_purchase(row, &Dict::new(), &lang, config.notebook.warranty_min_eur);
            if *row != was {
                changed += 1;
                let warranty = if was.contains_key("warranty_end") && !row.contains_key("warranty_end") {
                    format!(" (−{})", tk::str_or_empty(was.get("warranty_end")))
                } else {
                    String::new()
                };
                lines.push(format!("{} : {}{warranty}", summary_head(t, row), tk::str_or_empty(row.get("class"))));
            }
        }
        if lines.is_empty() {
            continue;
        }
        page.body = render_body(t, &lang, &table);
        page.touch();
        super::commit(
            config,
            Part::Main,
            &path,
            before.as_deref(),
            Some(&page.render()),
            &who,
            "classes",
            &lines.join(" · "),
        )?;
    }
    Ok(changed)
}

static ARCHIVE_REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"archive:([A-Za-z0-9_.-]+/[a-z0-9-]+/[0-9a-fA-F]{16})\b").expect("valid regex")
});

/// The archive document a record comes from (`archive:<agent>/<archive>/<16 hex>` in its source, written by the
/// extraction from the archives or by an agent): the apps open it read-only.
pub fn source_ref(source: &str) -> Option<String> {
    ARCHIVE_REF.captures(source).map(|c| c[1].to_lowercase())
}

fn item_view(t: &Kind, record: &Dict, today: NaiveDate, min_eur: f64, archived: bool) -> Value {
    let mut out = record.clone();
    let next = next_date(t, record);
    out.insert("next_date".into(), json!(next.map(|d| d.format("%Y-%m-%d").to_string())));
    out.insert("in_days".into(), json!(next.map(|d| (d - today).num_days())));
    let legal = t.id == "purchase" && is_legal(&tk::str_or_empty(record.get("warranty_end")));
    out.insert("legal".into(), json!(legal));
    if t.id == "purchase" {
        let class = class_id(&tk::str_or_empty(record.get("class")));
        out.insert("class_id".into(), json!(class));
        out.insert("important".into(), json!(important(record, min_eur, today)));
    }
    out.insert("archived".into(), json!(archived));
    out.insert("source_ref".into(), json!(source_ref(&tk::str_or_empty(record.get("source")))));
    for f in t.fields {
        out.entry(f.id.to_string()).or_insert(json!(""));
    }
    Value::Object(out)
}

/// One type for the apps: its title, page, fields and records, the soonest next date first (past ones and those
/// without a date after).
pub fn type_view(config: &Config, t: &Kind, today: NaiveDate) -> Value {
    let lang = super::language(config);
    let i = lang_index(&lang);
    let mut items: Vec<Dict> = records(config, t);
    items.sort_by_key(|r| match next_date(t, r) {
        Some(d) if d >= today => (0, (d - today).num_days()),
        Some(d) => (1, (today - d).num_days()),
        None => (2, 0),
    });
    let fields: Vec<Value> = t.fields.iter().map(|f| json!({"id": f.id, "title": f.titles[i]})).collect();
    let next_title = t.fields.iter().find(|f| f.id == t.next).map(|f| f.titles[i]).unwrap_or("");
    let min_eur = config.notebook.warranty_min_eur;
    let mut archived = archived_records(config, t);
    archived.sort_by_key(|r| {
        std::cmp::Reverse(next_date(t, r).or_else(|| {
            super::date_in(&tk::str_or_empty(r.get("date"))).and_then(|(m, d, y)| NaiveDate::from_ymd_opt(y?, m, d))
        }))
    });
    let views: Vec<Value> = items.iter().map(|r| item_view(t, r, today, min_eur, false)).collect();
    let important = views.iter().filter(|v| v.get("important").map(py::truthy).unwrap_or(false)).count();
    json!({"type": t.id, "title": t.titles[i], "path": path_of(&lang, t), "fields": fields, "next": t.next,
           "next_title": next_title, "count": items.len(), "important_count": important,
           "warranty_min_eur": min_eur, "items": views,
           "archive_path": archive_path_of(&lang, t), "archived_count": archived.len(),
           "archived": archived.iter().map(|r| item_view(t, r, today, min_eur, true)).collect::<Vec<_>>()})
}

/// Every type, for `GET /v1/notebook/records`.
pub fn view(config: &Config, today: NaiveDate) -> Value {
    json!({"types": TYPES.iter().map(|t| type_view(config, t, today)).collect::<Vec<_>>()})
}

/// The home's line per type that has records: its count and its next date to come.
pub fn home(config: &Config, today: NaiveDate) -> Vec<Value> {
    let lang = super::language(config);
    let i = lang_index(&lang);
    let mut out = Vec::new();
    for t in &TYPES {
        let items = records(config, t);
        if items.is_empty() {
            continue;
        }
        let next = items
            .iter()
            .filter_map(|r| next_date(t, r).filter(|d| *d >= today).map(|d| (d, r)))
            .min_by_key(|(d, _)| *d)
            .map(|(d, r)| {
                json!({"date": d.format("%Y-%m-%d").to_string(), "in_days": (d - today).num_days(),
                                 "text": summary_head(t, r)})
            });
        out.push(
            json!({"type": t.id, "title": t.titles[i], "path": path_of(&lang, t), "count": items.len(), "next": next}),
        );
    }
    out
}

/// A record's name: its first text field that is not a date.
pub fn summary_head(t: &Kind, record: &Dict) -> String {
    t.fields
        .iter()
        .filter(|f| !DATES.contains(&f.id))
        .map(|f| tk::str_or_empty(record.get(f.id)))
        .find(|v| !v.is_empty())
        .unwrap_or_default()
}

/// The records page's view in `page_view` (its type, fields and sorted records), None for another page.
pub fn page_view(config: &Config, page: &Page, today: NaiveDate) -> Option<Value> {
    kind_of_page(page).map(|t| type_view(config, t, today))
}

/// The booking records of a trip: those naming it (its title or an alias, in their « trip »), and those naming no trip
/// whose date falls within it.
pub fn bookings_for(config: &Config, trip: &Page, start: Option<NaiveDate>, end: Option<NaiveDate>) -> Vec<Dict> {
    let Some(t) = kind("booking") else { return Vec::new() };
    let mut all = records(config, t);
    all.extend(archived_records(config, t));
    let near = |d: Option<NaiveDate>, days: i64| match (d, start, end) {
        (Some(d), Some(s), Some(e)) => d >= s - chrono::Duration::days(days) && d <= e + chrono::Duration::days(days),
        (Some(_), _, _) => days > 0,
        _ => days > 0,
    };
    all.into_iter()
        .filter(|r| {
            let named = tk::str_or_empty(r.get("trip"));
            let named = super::LINK.replace_all(&named, |c: &regex::Captures| c[1].to_string()).to_string();
            if !named.trim().is_empty() && trip.named(&named) {
                // the same name for two trips (« Lisbonne » in 2025 and 2026): the dates tell which
                return near(next_date(t, r), 3);
            }
            named.trim().is_empty() && near(next_date(t, r), 0)
        })
        .collect()
}

fn proposal_title(lang: &str, t: &Kind, record: &Dict, date: NaiveDate) -> String {
    let name = summary_head(t, record);
    let when = date.format("%Y-%m-%d");
    match (lang, t.id) {
        ("fr", "purchase") => format!("Garantie de {name} : se termine le {when}"),
        ("fr", "subscription") => format!("Renouvellement de {name} le {when} : garder ou résilier ?"),
        ("fr", "contract") => format!("Échéance {name} le {when}"),
        ("fr", _) => format!("{name} expire le {when} : le renouveler"),
        ("de", "purchase") => format!("Garantie für {name} endet am {when}"),
        ("de", "subscription") => format!("{name} verlängert sich am {when}: behalten oder kündigen?"),
        ("de", "contract") => format!("{name} läuft am {when} ab"),
        ("de", _) => format!("{name} läuft am {when} ab: erneuern"),
        ("es", "purchase") => format!("La garantía de {name} termina el {when}"),
        ("es", "subscription") => format!("{name} se renueva el {when}: ¿mantener o cancelar?"),
        ("es", "contract") => format!("{name} vence el {when}"),
        ("es", _) => format!("{name} caduca el {when}: renovarlo"),
        (_, "purchase") => format!("Warranty of {name} ends on {when}"),
        (_, "subscription") => format!("{name} renews on {when}: keep or cancel?"),
        (_, "contract") => format!("{name} ends on {when}"),
        _ => format!("{name} expires on {when}: renew it"),
    }
}

/// The reminders worth proposing now (nightly, free): each record whose next date comes within its type's window,
/// proposed once for that date, within the proposals' weekly budget (the rest waits for a later night while it is
/// still ahead). Returns how many were proposed.
pub fn propose_due(config: &Config, today: NaiveDate) -> anyhow::Result<usize> {
    let lang = super::language(config);
    let done = super::strings(super::read_state(config).get("records_proposed"));
    let mut made = 0;
    for t in TYPES.iter().filter(|t| t.window > 0) {
        for record in records(config, t) {
            // a warranty's end: only for goods worth it (not a 3 € registration)
            if t.id == "purchase" && !important(&record, config.notebook.warranty_min_eur, today) {
                continue;
            }
            let Some(date) = next_date(t, &record) else { continue };
            let until = (date - today).num_days();
            if until < 0 || until > t.window {
                continue;
            }
            let key = format!("{}:{}", tk::str_or_empty(record.get("id")), date.format("%Y-%m-%d"));
            if done.contains(&key) {
                continue;
            }
            // the task's day: a little before the date, never in the past
            let before = match t.id {
                "subscription" => 2,
                "paper" => 30,
                _ => 7,
            };
            let due = (date - chrono::Duration::days(before)).max(today);
            let title = proposal_title(&lang, t, &record, date);
            let page = t.titles[lang_index(&lang)];
            match super::propose_for(config, "", t.proposal, &title, &due.format("%Y-%m-%d").to_string(), page) {
                Ok(_) => {
                    made += 1;
                    let _guard = super::lock();
                    super::update_state(config, |s| {
                        let mut all = super::strings(s.get("records_proposed"));
                        all.push(key.clone());
                        let over = all.len().saturating_sub(500);
                        all.drain(..over);
                        s.insert("records_proposed".into(), json!(all));
                    })?;
                }
                Err(err) if err.downcast_ref::<super::NotebookError>().is_some() => return Ok(made),
                Err(err) => return Err(err),
            }
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_reads_back_and_keeps_the_text_around() {
        let t = kind("Achats").unwrap();
        let body = "Mes achats.\n\n| Date | Objet | Magasin | Prix | Catégorie | Fin de garantie | Source | id |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n\
| 2025-03-12 | Lave-linge | Darty | 499 € | bien durable | 2027-03-12 (légale) |  | r1a2b3c |\n\nÀ revoir.\n";
        let table = parse(t, body);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0]["item"], "Lave-linge");
        assert_eq!(table.rows[0]["id"], "r1a2b3c");
        assert_eq!(render_body(t, "fr", &table), body);
        assert_eq!(next_date(t, &table.rows[0]), NaiveDate::from_ymd_opt(2027, 3, 12));
        assert_eq!(kind("warranty").unwrap().id, "purchase");
        assert_eq!(kind("Contrats et assurances").unwrap().id, "contract");
        assert_eq!(kind("Bookings").unwrap().id, "booking");
    }

    #[test]
    fn the_same_record_is_found_on_its_key() {
        let t = kind("purchase").unwrap();
        let old: Dict =
            serde_json::from_value(json!({"item": "Lave-linge", "date": "2025-03-12", "id": "r1"})).unwrap();
        let again: Dict = serde_json::from_value(json!({"item": "lave linge", "date": "2025-03-12"})).unwrap();
        let other: Dict = serde_json::from_value(json!({"item": "Lave-linge", "date": "2026-01-02"})).unwrap();
        assert!(same(t, &old, &again));
        assert!(!same(t, &old, &other));
        let s = kind("subscription").unwrap();
        let netflix: Dict = serde_json::from_value(json!({"service": "Netflix", "id": "r2"})).unwrap();
        let price: Dict = serde_json::from_value(json!({"service": "NETFLIX", "price": "13,49 €"})).unwrap();
        assert!(same(s, &netflix, &price));
    }

    #[test]
    fn only_durable_goods_get_the_legal_warranty() {
        assert_eq!(price_value("1 299,00 €"), Some(1299.0));
        assert_eq!(price_value("$45.99"), Some(45.99));
        assert_eq!(price_value("1,299.50 USD"), Some(1299.5));
        assert_eq!(price_value("3 €"), Some(3.0));
        assert_eq!(classify("Inscription au trail des Monts", "Njuko", "3 €", 50.0), "ticket");
        assert_eq!(classify("Machine à café l’Expresso", "Darty", "189 €", 50.0), "goods");
        assert_eq!(classify("Pizza 4 fromages", "Deliveroo", "14 €", 50.0), "food");
        assert_eq!(classify("Frais de port", "Amazon", "4,99 €", 50.0), "fee");
        assert_eq!(classify("Premium Individual", "Spotify", "10,99 €", 50.0), "digital");
        assert_eq!(classify("Philips Hue E27", "Boulanger", "79 €", 50.0), "goods");
        assert_eq!(classify("Philips Hue E27", "Boulanger", "19 €", 50.0), "other");
        assert_eq!(class_id("billet ou inscription"), Some("ticket"));
        assert_eq!(class_id("Gebrauchsgut"), Some("goods"));
        let mut ticket: Dict = serde_json::from_value(
            json!({"date": "2026-03-01", "item": "Inscription au trail", "price": "3 €", "warranty_end": "2028-03-01 (légale)"}),
        )
        .unwrap();
        settle_purchase(&mut ticket, &Dict::new(), "fr", 50.0);
        assert_eq!(ticket["class"], "billet ou inscription");
        assert!(!ticket.contains_key("warranty_end"));
        let mut tv: Dict =
            serde_json::from_value(json!({"date": "2026-03-01", "item": "Téléviseur", "price": "499 €"})).unwrap();
        settle_purchase(&mut tv, &Dict::new(), "fr", 50.0);
        assert_eq!(tv["warranty_end"], "2028-03-01 (légale)");
        let today = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        assert!(important(&tv, 50.0, today));
        let mut cable: Dict =
            serde_json::from_value(json!({"date": "2026-03-01", "item": "Chargeur", "price": "12 €"})).unwrap();
        settle_purchase(&mut cable, &Dict::new(), "fr", 50.0);
        assert!(!important(&cable, 50.0, today));
        // a warranty stated by the document stays, whatever the class
        let mut repair: Dict = serde_json::from_value(
            json!({"date": "2026-03-01", "item": "Réparation vélo", "price": "40 €", "warranty_end": "2027-03-01"}),
        )
        .unwrap();
        settle_purchase(&mut repair, &Dict::new(), "fr", 50.0);
        assert_eq!(repair["warranty_end"], "2027-03-01");
        assert!(important(&repair, 50.0, today));
    }
}
