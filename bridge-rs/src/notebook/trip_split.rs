//! A page that mixes several journeys split into real trips, with the person's yes (the owner, 2026-10-29). An agent
//! filing mails from the archives put an Amsterdam flight of December 2021, an airport parking of July 2022 and a car
//! rental of October 2022 on one « Voyages » page; nothing grouped them into trips.
//!
//! - **Detection** (the nightly pass, and on demand by an agent: notebook_edit `split_trips`): a trip page, or a list
//!   of trips (« Voyages », trip.rs `is_collection`), whose dated lines and booking records (those naming it in their
//!   « trip ») form 2 or more clusters: dates more than `GAP_DAYS` apart start a new one, each with at least one
//!   booking (trip.rs `weight` 3, or a record). A refund or an invoice (weight 0) goes with the trip it follows (within
//!   `AFTER_DAYS`); a cluster of plain notes without a booking stays on the page. Each trip is named by its place when
//!   its lines or records say one (« Amsterdam, déc. 2021 »), else by its month (« Voyage de juil. 2022 »).
//! - **One proposal per page** in the Carnet's proposals (mod.rs, kind `split`, outside the tasks' weekly budget):
//!   « Séparer « Voyages » en 3 voyages : … ? », answered in the apps (Séparer / Laisser comme ça). Left as it is: not
//!   proposed again until the page changes a lot (a new trip in it, or half as many dated lines again).
//! - **Yes**: one trip page per cluster (its lines under their headings, in the trip's outline; its frontmatter's
//!   start, end and destination), its booking records pointed to it; the lines without a date stay on the page, which
//!   becomes (or stays) a subject linking to the new trips. Nothing dropped, every page written as a change of the
//!   Carnet's history (each can be undone there). A second yes does nothing more.

use chrono::{Datelike, NaiveDate};
use serde_json::{json, Value};

use super::records;
use super::trip_auto::{self, Booking};
use super::{fold, template, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// Dated lines more than this many days apart belong to two trips.
pub const GAP_DAYS: i64 = 21;
/// A refund or an invoice this many days at most after a trip goes with it.
pub const AFTER_DAYS: i64 = 120;
/// The proposal's kind (mod.rs proposals).
pub const KIND: &str = "split";

/// One dated thing of a page: a line of it (`line`, its index in the body's lines) or a booking record (`record`).
#[derive(Debug, Clone)]
pub struct Item {
    pub date: NaiveDate,
    pub weight: u32,
    pub line: Option<usize>,
    pub record: Option<String>,
    /// Where it goes (a city), and where a leg comes from.
    pub to: Option<String>,
    pub from: Option<String>,
}

/// One journey found in a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Trip {
    pub title: String,
    pub place: Option<String>,
    pub start: NaiveDate,
    pub end: NaiveDate,
    /// The body's lines that move to it (indices), and its records' ids.
    pub lines: Vec<usize>,
    pub records: Vec<String>,
}

fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

/// Whether a page may hold several journeys: a trip, or a list of trips (« Voyages »).
pub fn splittable(page: &Page) -> bool {
    let kind = tk::str_or_empty(page.front.get("type"));
    kind == "trip" || (kind == "subject" && super::trip::is_collection(&page.title))
}

/// The body's lines with their heading (None before the first one), « En bref » left out.
fn lines_of(page: &Page, lang: &str) -> Vec<(usize, Option<String>, String)> {
    let brief = fold(super::words(lang).brief);
    let mut heading: Option<String> = None;
    let mut out = Vec::new();
    for (i, line) in page.body.lines().enumerate() {
        if let Some(name) = line.strip_prefix("## ") {
            heading = Some(name.trim().to_string());
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() || line.starts_with("# ") || heading.as_deref().is_some_and(|h| fold(h) == brief) {
            continue;
        }
        out.push((i, heading.clone(), trimmed.to_string()));
    }
    out
}

/// A line's text without its list mark and attribution (whose date is not the line's).
fn fact_text(line: &str) -> String {
    let item = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")).unwrap_or(line);
    super::split_attribution(item).0
}

/// A line's words for its place: its dates and times taken out (« Vol Paris → Amsterdam le 18/12/2021 à 07:15 »).
fn placeless(text: &str) -> String {
    let text = super::ISO_DATE.replace_all(text, " ");
    let text = super::NUMERIC.replace_all(&text, " ");
    let time = regex::Regex::new(r"\b(?:à|at|um|a las)?\s*\d{1,2}[:hH]\d{2}\b").expect("valid regex");
    let text = time.replace_all(&text, " ");
    let tail = regex::Regex::new(r"(?i)\s+(?:le|du|au|on|am|el|the)\s*$").expect("valid regex");
    let squeezed = tk::squeeze(&text);
    tail.replace_all(squeezed.trim_end_matches([',', ' ', ':']), "").to_string()
}

/// What else books a journey in a list of trips (an airport parking, a ticket), besides trip.rs's booking words.
const BOOKED: &[&str] = &[
    "parking",
    "aeroport",
    "airport",
    "flughafen",
    "aeropuerto",
    "billet",
    "billets",
    "ticket",
    "tickets",
    "reservation",
    "booking",
    "buchung",
    "reserva",
    "croisiere",
    "cruise",
];

/// A line's weight: trip.rs's (a booking 3, a refund or an invoice 0, else 1), an airport parking or a ticket a
/// booking too.
fn weight(text: &str, explicit: bool) -> u32 {
    let w = super::trip::weight(text, explicit);
    let spaced = format!(" {} ", fold(text));
    if w == 1 && BOOKED.iter().any(|b| spaced.contains(&format!(" {b} "))) {
        3
    } else {
        w
    }
}

/// The booking records that belong to a page: those naming it in their « trip » (live and archived).
pub fn records_of(config: &Config, page: &Page) -> Vec<Dict> {
    let Some(t) = records::kind("booking") else { return Vec::new() };
    let mut all = records::records(config, t);
    all.extend(records::archived_records(config, t));
    all.into_iter()
        .filter(|r| {
            let named = tk::str_or_empty(r.get("trip"));
            let named = super::LINK.replace_all(&named, |c: &regex::Captures| c[1].to_string()).to_string();
            !named.trim().is_empty() && page.named(&named)
        })
        .collect()
}

/// A page's dated things: its lines (a year-less date read around `today`) and its records.
pub fn items_of(page: &Page, records: &[Dict], lang: &str, today: NaiveDate) -> Vec<Item> {
    let mut out = Vec::new();
    for (i, heading, line) in lines_of(page, lang) {
        let text = fact_text(&line);
        let Some(date) = super::trip::date_of(&text, today) else { continue };
        let explicit = heading.as_deref().and_then(|h| template::resolve("trip", h)) == Some("dates");
        let mut fields = Dict::new();
        fields.insert("date".into(), json!(date.format("%Y-%m-%d").to_string()));
        fields.insert("what".into(), json!(placeless(&text)));
        let booking = Booking::new(&fields, lang);
        out.push(Item {
            date,
            weight: weight(&text, explicit),
            line: Some(i),
            record: None,
            to: booking.as_ref().and_then(|b| b.to.clone()),
            from: booking.and_then(|b| b.from),
        });
    }
    for r in records {
        let mut fields = r.clone();
        // the trip's name is the page's (« Voyages »): no place in it
        fields.remove("trip");
        let Some(b) = Booking::new(&fields, lang) else { continue };
        let words = format!("{} {}", tk::str_or_empty(r.get("what")), tk::str_or_empty(r.get("provider")));
        let weight = if weight(&words, false) == 0 { 0 } else { 3 };
        out.push(Item {
            date: b.date,
            weight,
            line: None,
            record: Some(tk::str_or_empty(r.get("id"))),
            to: b.to.clone(),
            from: b.from.clone(),
        });
    }
    out.sort_by_key(|i| (i.date, i.line.unwrap_or(usize::MAX)));
    out
}

/// The clusters of `items` that are trips (each with a booking), as indices into `items`, in date order: dates more
/// than `GAP_DAYS` apart start a new one; a refund or an invoice joins the trip it follows within `AFTER_DAYS`.
pub fn clusters(items: &[Item]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut last: Option<NaiveDate> = None;
    for (i, item) in items.iter().enumerate().filter(|(_, it)| it.weight > 0) {
        match (last, groups.last_mut()) {
            (Some(d), Some(g)) if (item.date - d).num_days() <= GAP_DAYS => g.push(i),
            _ => groups.push(vec![i]),
        }
        last = Some(item.date);
    }
    groups.retain(|g| g.iter().any(|i| items[*i].weight >= 3));
    for (i, item) in items.iter().enumerate().filter(|(_, it)| it.weight == 0) {
        let follows = groups
            .iter()
            .enumerate()
            .filter_map(|(n, g)| {
                let end = g.iter().map(|j| items[*j].date).max()?;
                let gap = (item.date - end).num_days();
                (0..=AFTER_DAYS).contains(&gap).then_some((n, gap))
            })
            .min_by_key(|(_, gap)| *gap);
        if let Some((n, _)) = follows {
            groups[n].push(i);
        }
    }
    groups
}

/// A trip's place: where its legs and bookings go most, not where it starts from (tie: the first one).
pub fn place_of(items: &[&Item]) -> Option<String> {
    let home = items.iter().find_map(|i| i.from.clone()).map(|h| fold(&h));
    let mut votes: Vec<(String, u32)> = Vec::new();
    for item in items.iter().filter(|i| i.weight > 0) {
        let Some(to) = item.to.as_deref() else { continue };
        if home.as_deref() == Some(fold(to).as_str()) {
            continue;
        }
        let w = if item.weight >= 3 { 2 } else { 1 };
        match votes.iter_mut().find(|(c, _)| fold(c) == fold(to)) {
            Some(v) => v.1 += w,
            None => votes.push((to.to_string(), w)),
        }
    }
    let best = votes.iter().map(|v| v.1).max()?;
    votes.into_iter().find(|v| v.1 == best).map(|v| v.0)
}

/// « Amsterdam, déc. 2021 », else « Voyage de juil. 2022 » (« Voyage d'oct. 2022 »).
pub fn title(place: Option<&str>, start: NaiveDate, lang: &str) -> String {
    let i = lang_index(lang);
    let month = format!("{} {}", trip_auto::MONTHS[i][start.month0() as usize], start.year());
    match place {
        Some(p) => format!("{p}, {month}"),
        None => match i {
            0 if month.starts_with(['a', 'o', 'e', 'é', 'i', 'u']) => format!("Voyage d'{month}"),
            0 => format!("Voyage de {month}"),
            2 => format!("Reise im {month}"),
            3 => format!("Viaje de {month}"),
            _ => format!("Trip in {month}"),
        },
    }
}

/// The journeys of a page (none unless 2 or more), with their names (each one once).
pub fn trips(items: &[Item], lang: &str) -> Vec<Trip> {
    let groups = clusters(items);
    if groups.len() < 2 {
        return Vec::new();
    }
    let mut out: Vec<Trip> = Vec::new();
    for g in groups {
        let members: Vec<&Item> = g.iter().map(|i| &items[*i]).collect();
        let dated: Vec<NaiveDate> = members.iter().filter(|m| m.weight > 0).map(|m| m.date).collect();
        let (Some(start), Some(end)) = (dated.iter().min().copied(), dated.iter().max().copied()) else { continue };
        let place = place_of(&members);
        let mut name = title(place.as_deref(), start, lang);
        let mut n = 2;
        while out.iter().any(|t| fold(&t.title) == fold(&name)) {
            name = format!("{} ({n})", title(place.as_deref(), start, lang));
            n += 1;
        }
        let mut lines: Vec<usize> = members.iter().filter_map(|m| m.line).collect();
        lines.sort();
        out.push(Trip {
            title: name,
            place,
            start,
            end,
            lines,
            records: members.iter().filter_map(|m| m.record.clone()).collect(),
        });
    }
    out
}

/// The journeys of a page of the Carnet (its records read).
pub fn plan(config: &Config, page: &Page) -> Vec<Trip> {
    if !splittable(page) {
        return Vec::new();
    }
    let lang = super::language(config);
    let items = items_of(page, &records_of(config, page), &lang, chrono::Local::now().date_naive());
    trips(&items, &lang)
}

/// The proposal's question, in the Carnet's language.
pub fn question(page_title: &str, trips: &[Trip], lang: &str) -> String {
    let names = trips.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join(" · ");
    let n = trips.len();
    match lang {
        "fr" => format!("Séparer « {page_title} » en {n} voyages : {names} ?"),
        "de" => format!("« {page_title} » in {n} Reisen aufteilen: {names}?"),
        "es" => format!("¿Separar « {page_title} » en {n} viajes: {names}?"),
        _ => format!("Split « {page_title} » into {n} trips: {names}?"),
    }
}

fn key_of(path: &str, page: &Page) -> String {
    if page.id().is_empty() {
        path.to_string()
    } else {
        page.id()
    }
}

/// How much of a page was counted: its trips, its dated things.
fn measure(config: &Config, page: &Page, trips: &[Trip]) -> (usize, usize) {
    let lang = super::language(config);
    let items = items_of(page, &records_of(config, page), &lang, chrono::Local::now().date_naive());
    (trips.len(), items.len())
}

/// What `propose_locked` did.
#[derive(Debug, Clone, PartialEq)]
pub enum Proposed {
    /// A new proposal (its id, its question).
    Made(String, String),
    /// One was already open for this page (brought up to date).
    Open(String, String),
    /// The person said to leave it as it is, and it hasn't changed much since.
    Declined,
    /// Not one journey more than one.
    Nothing,
}

fn trips_view(trips: &[Trip]) -> Value {
    json!(trips
        .iter()
        .map(|t| json!({"title": t.title, "start": t.start.format("%Y-%m-%d").to_string(),
                        "end": t.end.format("%Y-%m-%d").to_string(), "place": t.place,
                        "lines": t.lines.len(), "records": t.records.len()}))
        .collect::<Vec<_>>())
}

/// The split of one page proposed (the caller holds the lock). `asked`: an agent asked for it (the person's word), so
/// a page left as it is is proposed again.
pub fn propose_locked(config: &Config, path: &str, page: &Page, asked: bool) -> anyhow::Result<Proposed> {
    let trips = plan(config, page);
    if trips.is_empty() {
        return Ok(Proposed::Nothing);
    }
    let lang = super::language(config);
    let key = key_of(path, page);
    let (count, dated) = measure(config, page, &trips);
    let state = super::read_state(config);
    if !asked {
        if let Some(old) = state.get("split_declined").and_then(|d| d.get(&key)) {
            let (old_trips, old_items) = (
                old.get("trips").and_then(Value::as_u64).unwrap_or(0) as usize,
                old.get("items").and_then(Value::as_u64).unwrap_or(0) as usize,
            );
            if count <= old_trips && dated < old_items + (old_items / 2).max(3) {
                return Ok(Proposed::Declined);
            }
        }
    }
    let text = question(&page.title, &trips, &lang);
    let open = super::proposals(config).into_iter().find(|p| {
        tk::str_or_empty(p.get("kind")) == KIND
            && tk::str_or_empty(p.get("state")) == "open"
            && tk::str_or_empty(p.get("page_key")) == key
    });
    let view = trips_view(&trips);
    if let Some(open) = open {
        let id = tk::str_or_empty(open.get("id"));
        super::update_state(config, |s| {
            if let Some(Value::Array(items)) = s.get_mut("proposals") {
                for item in items.iter_mut().filter(|i| i.get("id").and_then(Value::as_str) == Some(id.as_str())) {
                    item["title"] = json!(text);
                    item["page"] = json!(path);
                    item["page_title"] = json!(page.title);
                    item["trips"] = view.clone();
                    item["measure"] = json!({"trips": count, "items": dated});
                }
            }
        })?;
        return Ok(Proposed::Open(id, text));
    }
    let id = format!("q{}", py::token_hex(4));
    let item = json!({"id": id, "agent": "", "kind": KIND, "title": text, "due": "", "at": tk::now_iso_seconds(),
                      "state": "open", "page": path, "page_key": key, "page_title": page.title, "trips": view,
                      "measure": {"trips": count, "items": dated}});
    super::update_state(config, |s| {
        let mut all = match s.get("proposals") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        all.push(item);
        let excess = all.len().saturating_sub(100);
        all.drain(..excess);
        s.insert("proposals".into(), Value::Array(all));
    })?;
    Ok(Proposed::Made(id, text))
}

/// The nightly pass: every page holding several journeys proposed for a split (once; free, no model). Returns how
/// many new proposals.
pub fn detect(config: &Config) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let mut made = 0;
    for (path, page) in super::all_pages(config, Part::Main) {
        if !splittable(&page) {
            continue;
        }
        if let Proposed::Made(..) = propose_locked(config, &path, &page, false)? {
            made += 1;
        }
    }
    Ok(made)
}

/// An agent asks for it (notebook_edit split_trips): the page's split proposed to the person.
pub fn propose(config: &Config, query: &str) -> anyhow::Result<Proposed> {
    let _guard = super::lock();
    let Some(found) = super::find(config, &[Part::Main], query) else {
        return super::refuse("No such page in the Carnet (notebook_search finds pages by name or words).");
    };
    propose_locked(config, &found.path, &found.page, true)
}

fn set_state(config: &Config, id: &str, to: &str, made: Option<Value>) -> std::io::Result<Dict> {
    super::update_state(config, |s| {
        if let Some(Value::Array(items)) = s.get_mut("proposals") {
            for item in items.iter_mut().filter(|i| i.get("id").and_then(Value::as_str) == Some(id)) {
                item["state"] = json!(to);
                if let Some(made) = &made {
                    item["made"] = made.clone();
                }
            }
        }
    })
}

fn find_proposal(config: &Config, id: &str) -> anyhow::Result<Dict> {
    match super::proposals(config).into_iter().find(|p| tk::str_or_empty(p.get("id")) == id.trim()) {
        Some(p) if tk::str_or_empty(p.get("kind")) == KIND => Ok(p),
        Some(_) => super::refuse("this proposal is not a split"),
        None => super::refuse("unknown proposal id"),
    }
}

/// Whether a proposal (by its id) is a split.
pub fn is_split(config: &Config, id: &str) -> bool {
    super::proposals(config)
        .iter()
        .any(|p| tk::str_or_empty(p.get("id")) == id.trim() && tk::str_or_empty(p.get("kind")) == KIND)
}

/// The open split proposal of a page (for its card on the page itself).
pub fn open_for(config: &Config, path: &str, page: &Page) -> Option<Dict> {
    let key = key_of(path, page);
    super::open_proposals(config).into_iter().find(|p| {
        tk::str_or_empty(p.get("kind")) == KIND
            && (tk::str_or_empty(p.get("page_key")) == key || tk::str_or_empty(p.get("page")) == path)
    })
}

/// « Laisser comme ça »: closed, and not proposed again until the page changes a lot.
pub fn decline(config: &Config, id: &str) -> anyhow::Result<()> {
    let _guard = super::lock();
    let found = find_proposal(config, id)?;
    let key = tk::str_or_empty(found.get("page_key"));
    let measure = found.get("measure").cloned().unwrap_or(json!({}));
    super::update_state(config, |s| {
        let mut declined = match s.get("split_declined") {
            Some(Value::Object(m)) => m.clone(),
            _ => Dict::new(),
        };
        declined.insert(key.clone(), measure.clone());
        s.insert("split_declined".into(), Value::Object(declined));
    })?;
    set_state(config, id.trim(), "declined", None)?;
    Ok(())
}

/// The day range of a trip in its link line (« 18 – 22 déc. 2021 »).
fn range(start: NaiveDate, end: NaiveDate, lang: &str) -> String {
    let i = lang_index(lang);
    let month = |d: NaiveDate| trip_auto::MONTHS[i][d.month0() as usize];
    if start == end {
        format!("{} {} {}", start.day(), month(start), start.year())
    } else if (start.month(), start.year()) == (end.month(), end.year()) {
        format!("{} – {} {} {}", start.day(), end.day(), month(end), end.year())
    } else if start.year() == end.year() {
        format!("{} {} – {} {} {}", start.day(), month(start), end.day(), month(end), end.year())
    } else {
        format!("{} {} {} – {} {} {}", start.day(), month(start), start.year(), end.day(), month(end), end.year())
    }
}

/// The body without the lines `gone` (indices), its headings left with nothing under them taken out.
fn without(body: &str, gone: &[usize]) -> String {
    let kept: Vec<&str> = body.lines().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, l)| l).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < kept.len() {
        let line = kept[i];
        if line.starts_with("## ") {
            let next = kept[i + 1..].iter().position(|l| l.starts_with("## ") || l.starts_with("# "));
            let end = next.map(|n| i + 1 + n).unwrap_or(kept.len());
            if kept[i + 1..end].iter().all(|l| l.trim().is_empty()) {
                i = end;
                continue;
            }
        }
        out.push(line.to_string());
        i += 1;
    }
    let mut text = out.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim_matches('\n').to_string() + "\n"
}

/// « Séparer »: the page's trips made (see the module's text). Ok: the trips made ({title, path}); a second yes on
/// the same proposal gives them again and changes nothing.
pub fn accept(config: &Config, id: &str, who: &Who) -> anyhow::Result<Value> {
    let _guard = super::lock();
    let found = find_proposal(config, id)?;
    match tk::str_or_empty(found.get("state")).as_str() {
        "accepted" => return Ok(json!({"trips": found.get("made").cloned().unwrap_or(json!([])), "already": true})),
        "open" => {}
        _ => return super::refuse("this proposal was already answered"),
    }
    let key = tk::str_or_empty(found.get("page_key"));
    let wanted = tk::str_or_empty(found.get("page"));
    let pages = super::all_pages(config, Part::Main);
    let Some((path, page)) = pages
        .iter()
        .find(|(_, p)| !p.id().is_empty() && p.id() == key)
        .or_else(|| pages.iter().find(|(p, _)| *p == wanted))
        .cloned()
    else {
        set_state(config, id.trim(), "stale", None)?;
        return super::refuse("the page is no longer in the Carnet: nothing to split");
    };
    let trips = plan(config, &page);
    if trips.is_empty() {
        set_state(config, id.trim(), "stale", None)?;
        return super::refuse("this page holds one journey at most now: nothing to split");
    }
    let made = split_locked(config, &path, &page, &trips, who)?;
    set_state(config, id.trim(), "accepted", Some(made.clone()))?;
    Ok(json!({"trips": made, "already": false}))
}

/// The split itself (the caller holds the lock).
fn split_locked(config: &Config, path: &str, page: &Page, trips: &[Trip], who: &Who) -> anyhow::Result<Value> {
    let lang = super::language(config);
    let w = super::words(&lang);
    let today = chrono::Local::now().date_naive();
    let lines: Vec<(usize, Option<String>, String)> = lines_of(page, &lang);
    let heading_of = |i: usize| lines.iter().find(|(n, _, _)| *n == i).and_then(|(_, h, _)| h.clone());
    let raw: Vec<&str> = page.body.lines().collect();
    let mut made: Vec<Value> = Vec::new();
    let mut new_ids: Vec<String> = Vec::new();
    for trip in trips {
        // its lines under their headings, in the trip's outline
        let mut chunk = String::new();
        let mut last: Option<Option<String>> = None;
        let existing = super::all_pages(config, Part::Main)
            .into_iter()
            .find(|(p, pg)| p != path && pg.kind() == "trip" && pg.named(&trip.title));
        let already: Vec<String> = existing
            .as_ref()
            .map(|(_, pg)| pg.sections().into_iter().flat_map(|(_, f)| f).map(|f| fold(&f.text)).collect())
            .unwrap_or_default();
        for i in &trip.lines {
            let line = raw.get(*i).map(|l| l.trim()).unwrap_or("");
            if line.is_empty() || already.contains(&fold(&fact_text(line))) {
                continue;
            }
            let heading = heading_of(*i);
            if last.as_ref() != Some(&heading) {
                let name = heading.clone().unwrap_or_else(|| template::title("trip", "other", &lang));
                chunk.push_str(&format!("\n## {name}\n"));
                last = Some(heading);
            }
            chunk.push_str(line);
            chunk.push('\n');
        }
        let (trip_path, before, mut trip_page) = match existing {
            Some((p, pg)) => (p.clone(), super::read_page_text(config, Part::Main, &p), pg),
            None => {
                let mut pg = Page::new("trip", &trip.title);
                pg.front.insert("origin".into(), json!("split"));
                pg.front.insert("split_from".into(), json!(page.title));
                let folder = trip_auto::folder_for(w, Some(trip.end), today);
                (super::free_path(config, Part::Main, &folder, &trip.title, ""), None, pg)
            }
        };
        let body = format!("{}\n{chunk}", trip_page.body);
        trip_page.body = template::normalize("trip", &lang, w.brief, &body).body;
        for (k, d) in [("start", trip.start), ("end", trip.end)] {
            if !trip_page.front.contains_key(k) {
                trip_page.front.insert(k.into(), json!(d.format("%Y-%m-%d").to_string()));
            }
        }
        if let Some(place) = &trip.place {
            if !trip_page.front.contains_key("destination") {
                trip_page.front.insert("destination".into(), json!(place));
            }
        }
        trip_page.touch();
        let after = trip_page.render();
        if before.as_deref() != Some(after.as_str()) {
            let summary = format!("« {} » → {}", page.title, trip.title);
            super::commit(config, Part::Main, &trip_path, before.as_deref(), Some(&after), who, "split", &summary)?;
        }
        new_ids.push(trip_page.id());
        point_records_locked(config, &trip.records, &trip.title, who)?;
        made.push(json!({"title": trip.title, "path": trip_path, "start": trip.start.format("%Y-%m-%d").to_string(),
                         "end": trip.end.format("%Y-%m-%d").to_string()}));
    }
    // the page: what has no date stays, a subject linking to its trips
    let Some(before) = super::read_page_text(config, Part::Main, path) else { return Ok(json!(made)) };
    let mut rest = page.clone();
    rest.ensure_id();
    let gone: Vec<usize> = trips.iter().flat_map(|t| t.lines.iter().copied()).collect();
    rest.body = without(&page.body, &gone);
    if rest.body.trim().is_empty() {
        rest.body = String::new();
    }
    rest.front.insert("type".into(), json!("subject"));
    let links: Vec<String> =
        trips.iter().map(|t| format!("[[{}]] : {}", t.title, range(t.start, t.end, &lang))).collect();
    let status = template::title("subject", "status", &lang);
    rest.body = super::add_facts(&rest.body, &status, &links, who, &super::today()).0;
    rest.touch();
    let summary = format!("« {} » → {}", page.title, trips.iter().map(|t| t.title.as_str()).collect::<Vec<_>>().join(" · "));
    super::commit(config, Part::Main, path, Some(&before), Some(&rest.render()), who, "split", &summary)?;
    // neither made a trip again at night, nor taken by the extraction's own trips
    let id = rest.id();
    super::update_state(config, |s| {
        let mut seen = super::strings(s.get("trips_seen"));
        if !seen.contains(&id) {
            seen.push(id.clone());
        }
        s.insert("trips_seen".into(), json!(seen));
        let mut done = super::strings(s.get("trips_auto_done"));
        for n in &new_ids {
            if !n.is_empty() && !done.contains(n) {
                done.push(n.clone());
            }
        }
        s.insert("trips_auto_done".into(), json!(done));
    })?;
    Ok(json!(made))
}

/// The records `ids` pointed to their trip (`title` in their « trip »), in the live table and the archived one.
fn point_records_locked(config: &Config, ids: &[String], title: &str, who: &Who) -> anyhow::Result<()> {
    let Some(t) = records::kind("booking") else { return Ok(()) };
    if ids.is_empty() {
        return Ok(());
    }
    let lang = super::language(config);
    for archived in [false, true] {
        let (path, before, mut page, mut table) = records::read_at(config, t, archived);
        if before.is_none() {
            continue;
        }
        let mut changed = false;
        for row in table.rows.iter_mut() {
            if ids.contains(&tk::str_or_empty(row.get("id"))) && tk::str_or_empty(row.get("trip")) != title {
                row.insert("trip".into(), json!(title));
                changed = true;
            }
        }
        if changed {
            page.body = records::render_body(t, &lang, &table);
            page.touch();
            super::commit(config, Part::Main, &path, before.as_deref(), Some(&page.render()), who, "split", title)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The screenshot's page: travel items of three journeys filed in one « Voyages ».
    const VOYAGES: &str = "---\ntype: trip\nid: pvoyages001\n---\n# Voyages\n\n## En bref\nTous mes voyages.\n\n\
## Transports\n- Vol AF1240 Paris → Amsterdam le 18/12/2021 à 07:15 — *Vie, 2026-10-28*\n\
- Vol retour Amsterdam → Paris le 22/12/2021\n- Remboursement Air France le 31/12/2021\n\
- Parking aéroport le 22/07/2022\n- Location voiture le 01/10/2022\n- Location voiture retour le 08/10/2022\n\n\
## Notes\n- Garder les factures\n";

    #[test]
    fn the_screenshot_s_page_holds_three_trips() {
        let page = Page::parse(VOYAGES, "x");
        assert!(splittable(&page));
        let items = items_of(&page, &[], "fr", day(2026, 10, 29));
        assert_eq!(items.len(), 6, "the six dated lines, the attribution's date not one");
        let trips = trips(&items, "fr");
        let names: Vec<&str> = trips.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(names, ["Amsterdam, déc. 2021", "Voyage de juil. 2022", "Voyage d'oct. 2022"]);
        assert_eq!((trips[0].start, trips[0].end), (day(2021, 12, 18), day(2021, 12, 22)));
        assert_eq!(trips[0].lines.len(), 3, "the refund goes with the flights");
        assert_eq!((trips[2].start, trips[2].end), (day(2022, 10, 1), day(2022, 10, 8)));
        assert_eq!(
            question("Voyages", &trips, "fr"),
            "Séparer « Voyages » en 3 voyages : Amsterdam, déc. 2021 · Voyage de juil. 2022 · Voyage d'oct. 2022 ?"
        );
    }

    #[test]
    fn records_count_and_name_their_trip() {
        let page = Page::parse(VOYAGES, "x");
        let record = json!({"id": "r000001", "date": "2022-07-23", "what": "Hôtel à Lisbonne", "trip": "[[Voyages]]"});
        let items = items_of(&page, &[record.as_object().unwrap().clone()], "fr", day(2026, 10, 29));
        let trips = trips(&items, "fr");
        assert_eq!(trips[1].title, "Lisbonne, juil. 2022");
        assert_eq!(trips[1].records, ["r000001"]);
    }

    #[test]
    fn one_journey_or_notes_alone_are_no_split() {
        // the Amsterdam trip with its refund months later: one trip
        let one = "---\ntype: trip\n---\n# Amsterdam\n\n## Transports\n- Vol Paris → Amsterdam le 18/12/2021\n\
- Vol retour le 22/12/2021\n- Remboursement Air France le 15/03/2022 : 120 €\n";
        let page = Page::parse(one, "x");
        assert!(trips(&items_of(&page, &[], "fr", day(2026, 10, 29)), "fr").is_empty());
        // a plain note a month later is no second trip
        let notes = "---\ntype: trip\n---\n# Rome\n\n## Notes\n- Vol pour Rome le 03/05/2025\n- Penser aux photos le 20/06/2025\n";
        let page = Page::parse(notes, "x");
        assert!(trips(&items_of(&page, &[], "fr", day(2026, 10, 29)), "fr").is_empty());
        // a subject that is no list of trips is never split
        let subject = Page::parse("---\ntype: subject\n---\n# Cuisine\n\n## Historique\n- Vol le 01/01/2024\n", "x");
        assert!(!splittable(&subject));
    }

    #[test]
    fn titles_in_four_languages() {
        assert_eq!(title(Some("Amsterdam"), day(2021, 12, 18), "fr"), "Amsterdam, déc. 2021");
        assert_eq!(title(None, day(2022, 7, 22), "fr"), "Voyage de juil. 2022");
        assert_eq!(title(None, day(2022, 8, 2), "fr"), "Voyage d'août 2022");
        assert_eq!(title(None, day(2022, 7, 22), "en"), "Trip in Jul 2022");
        assert_eq!(title(None, day(2022, 7, 22), "de"), "Reise im Juli 2022");
        assert_eq!(title(None, day(2022, 7, 22), "es"), "Viaje de jul. 2022");
        assert_eq!(range(day(2021, 12, 18), day(2021, 12, 22), "fr"), "18 – 22 déc. 2021");
    }

    #[test]
    fn lines_out_and_their_empty_headings_with_them() {
        let body = "## Transports\n- a\n- b\n\n## Notes\n- c\n";
        assert_eq!(without(body, &[1, 2]), "## Notes\n- c\n");
        assert_eq!(without(body, &[1]), "## Transports\n- b\n\n## Notes\n- c\n");
    }

    fn config_in(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.outbox.db_path = dir.join("data/outbox.db").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn accepting_splits_once_and_keeps_everything() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        super::super::write_file(&super::super::part_root(&config, Part::Main).join("Voyages/Voyages.md"), VOYAGES)
            .unwrap();
        assert_eq!(detect(&config).unwrap(), 1);
        assert_eq!(detect(&config).unwrap(), 0, "one card per page");
        let id = tk::str_or_empty(super::super::open_proposals(&config)[0].get("id"));
        let who = Who::you(&config);
        let made = accept(&config, &id, &who).unwrap();
        assert_eq!(made["trips"].as_array().unwrap().len(), 3);
        let again = accept(&config, &id, &who).unwrap();
        assert_eq!(again["already"], true);
        let all = super::super::all_pages(&config, Part::Main);
        assert_eq!(all.iter().filter(|(_, p)| p.kind() == "trip").count(), 3);
        let rest = all.iter().find(|(p, _)| p == "Voyages/Voyages.md").unwrap().1.clone();
        assert_eq!(rest.kind(), "subject");
        let text = rest.render();
        assert!(text.contains("Garder les factures") && text.contains("[[Amsterdam, déc. 2021]]"), "{text}");
        assert!(!text.contains("Parking"), "{text}");
        let amsterdam = all.iter().find(|(_, p)| p.title == "Amsterdam, déc. 2021").unwrap();
        assert!(amsterdam.0.starts_with("Archives/"), "a past trip: {}", amsterdam.0);
        let text = amsterdam.1.render();
        assert!(text.contains("start: 2021-12-18") && text.contains("Remboursement"), "{text}");
        assert!(text.contains("— *Vie, 2026-10-28*"), "the attribution kept: {text}");
        // nothing more proposed for it
        assert_eq!(detect(&config).unwrap(), 0);
    }

    #[test]
    fn left_as_it_is_until_it_changes_a_lot() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(dir.path());
        super::super::update_state(&config, |s| {
            s.insert("language".into(), json!("fr"));
        })
        .unwrap();
        let file = super::super::part_root(&config, Part::Main).join("Voyages/Voyages.md");
        super::super::write_file(&file, VOYAGES).unwrap();
        detect(&config).unwrap();
        let id = tk::str_or_empty(super::super::open_proposals(&config)[0].get("id"));
        decline(&config, &id).unwrap();
        assert_eq!(detect(&config).unwrap(), 0);
        // a fourth journey: proposed again
        let more = VOYAGES.replace("## Notes", "- Vol Paris → Rome le 03/05/2024\n\n## Notes");
        super::super::write_file(&file, &more).unwrap();
        assert_eq!(detect(&config).unwrap(), 1);
    }
}
