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
//! contract's number, the document, the booking's reference). A purchase without a known end of warranty gets the EU's
//! two-year legal guarantee, marked so. `source` holds the reference of the document a record comes from (the archive's,
//! an opaque text), shown as a link when the apps get a route for it. Text the person writes around the table in
//! Obsidian is kept.
//!
//! The nightly pass proposes reminders (never adds them by itself): a warranty ending within 30 days, a renewal within
//! 7, a contract's end within 30, a paper expiring within 60 — as Carnet task proposals (mod.rs `propose`, within its
//! weekly budget), each record's date once.

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

fn field_of(t: &Kind, header: &str) -> Option<&'static str> {
    let wanted = fold(header);
    t.fields
        .iter()
        .find(|f| fold(f.id) == wanted || f.titles.iter().any(|x| fold(x) == wanted))
        .map(|f| f.id)
        .or(if wanted == "id" { Some("id") } else { None })
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
    let lang = super::language(config);
    let path = path_of(&lang, t);
    let before = super::read_page_text(config, Part::Main, &path);
    let page = match &before {
        Some(text) => Page::parse(text, t.titles[lang_index(&lang)]),
        None => {
            let mut p = Page::new("records", t.titles[lang_index(&lang)]);
            p.front.remove("aliases");
            p.front.insert("records".into(), json!(t.id));
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
    let (path, before, mut page, mut table) = read(config, t);
    let new = prepare(t, fields);
    let id = id.trim();
    let index = if id.is_empty() {
        table.rows.iter().position(|r| same(t, r, &new))
    } else {
        match table.rows.iter().position(|r| tk::str_or_empty(r.get("id")) == id) {
            Some(i) => Some(i),
            None => {
                return Err(
                    super::NotebookError(format!("no {} record with id {id} (action list gives them)", t.id)).into()
                )
            }
        }
    };
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
    // the EU's legal guarantee (two years) when the end of warranty is unknown, or was computed from an older date
    if t.id == "purchase" {
        let end = tk::str_or_empty(record.get("warranty_end"));
        let computed = end.is_empty()
            || end.contains(legal_word(&lang))
            || super::WORDS.iter().any(|w| end.contains(legal_word(w.lang)));
        let given = new.contains_key("warranty_end");
        if computed && !given {
            let bought = tk::str_or_empty(record.get("date"));
            if let Some((m, d, Some(y))) = super::date_in(&bought) {
                if let Some(date) = NaiveDate::from_ymd_opt(y, m, d) {
                    let until = date.checked_add_months(chrono::Months::new(24)).unwrap_or(date);
                    record.insert(
                        "warranty_end".into(),
                        json!(format!("{} ({})", until.format("%Y-%m-%d"), legal_word(&lang))),
                    );
                }
            }
        }
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
    let (path, before, mut page, mut table) = read(config, t);
    let Some(index) = table.rows.iter().position(|r| tk::str_or_empty(r.get("id")) == id.trim()) else {
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

static ARCHIVE_REF: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"archive:([A-Za-z0-9_.-]+/[a-z0-9-]+/[0-9a-fA-F]{16})\b").expect("valid regex")
});

/// The archive document a record comes from (`archive:<agent>/<archive>/<16 hex>` in its source, written by the
/// extraction from the archives or by an agent): the apps open it read-only.
pub fn source_ref(source: &str) -> Option<String> {
    ARCHIVE_REF.captures(source).map(|c| c[1].to_lowercase())
}

fn item_view(t: &Kind, record: &Dict, today: NaiveDate) -> Value {
    let mut out = record.clone();
    let next = next_date(t, record);
    out.insert("next_date".into(), json!(next.map(|d| d.format("%Y-%m-%d").to_string())));
    out.insert("in_days".into(), json!(next.map(|d| (d - today).num_days())));
    let legal = t.id == "purchase"
        && super::WORDS.iter().any(|w| tk::str_or_empty(record.get("warranty_end")).contains(legal_word(w.lang)));
    out.insert("legal".into(), json!(legal));
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
    json!({"type": t.id, "title": t.titles[i], "path": path_of(&lang, t), "fields": fields, "next": t.next,
           "next_title": next_title, "count": items.len(),
           "items": items.iter().map(|r| item_view(t, r, today)).collect::<Vec<_>>()})
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
    records(config, t)
        .into_iter()
        .filter(|r| {
            let named = tk::str_or_empty(r.get("trip"));
            let named = super::LINK.replace_all(&named, |c: &regex::Captures| c[1].to_string()).to_string();
            if !named.trim().is_empty() {
                return trip.named(&named);
            }
            match (next_date(t, r), start, end) {
                (Some(d), Some(s), Some(e)) => d >= s && d <= e,
                _ => false,
            }
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
        let body = "Mes achats.\n\n| Date | Objet | Magasin | Prix | Fin de garantie | Source | id |\n| --- | --- | --- | --- | --- | --- | --- |\n\
| 2025-03-12 | Lave-linge | Darty | 499 € | 2027-03-12 (légale) |  | r1a2b3c |\n\nÀ revoir.\n";
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
}
