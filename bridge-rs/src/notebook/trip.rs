//! « Mode voyage » (the owner, 2026-10-23): a trip's Carnet page is no longer a generic subject. A page of kind `trip`
//! (« Voyage », Trip, Reise, Viaje) lives among the subjects with its own outline (template.rs `TRIP`: Dates,
//! Itinéraire, Transports, Hébergements, Documents, À emporter, Budget, Sur place, Notes) and its own layout in the apps,
//! built from what `view` reads in it: the trip's dates (its frontmatter's `start`/`end`, else the dates it holds), a
//! day-by-day timeline, the bookings (time, flight or train number, booking reference), the packing list with its ticks
//! (`- [ ] Passeport` / `- [x] Passeport`, Obsidian's checkboxes), its places, and the agents' page with a live map
//! linked to it when there is one.
//!
//! A subject becomes a trip by the agent's judgement (notebook_note with kind trip on it) or, at night, when its words
//! clearly say so (`looks_like_trip`: a date, and a trip word with a booking, or both a way of getting there and a
//! place to stay), once per page. Nothing is dropped: each line moves to the trip's section its heading or its words
//! point to, the rest to « Notes » with its old heading in front.

use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use regex::Regex;
use serde_json::{json, Value};

use super::{fold, template, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;

/// A trip is shown in the Carnet's home from this many days before it starts until it ends.
pub const HOME_DAYS: i64 = 30;

static BOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([ xX])\]\s*").expect("valid regex"));

/// A packing line's text without its checkbox.
pub fn without_box(text: &str) -> &str {
    match BOX.find(text.trim_start()) {
        Some(m) => &text.trim_start()[m.end()..],
        None => text,
    }
}

/// The line as a thing to pack, not ticked yet (its box kept when it has one).
pub fn unticked(text: &str) -> String {
    if BOX.is_match(text.trim_start()) {
        text.trim().to_string()
    } else {
        format!("[ ] {}", text.trim())
    }
}

/// (ticked, text) of a packing line.
pub fn checkbox(text: &str) -> (bool, String) {
    match BOX.captures(text.trim_start()) {
        Some(c) => (&c[1] != " ", without_box(text).trim().to_string()),
        None => (false, text.trim().to_string()),
    }
}

// -- is it a trip? ----------------------------------------------------------------------------------------------------

const TRIP_WORDS: &[&str] = &[
    "voyage",
    "vacances",
    "sejour",
    "road trip",
    "week end a",
    "trip",
    "travel",
    "holiday",
    "holidays",
    "vacation",
    "reise",
    "urlaub",
    "ferien",
    "viaje",
    "vacaciones",
    "escapada",
];
const GETTING_THERE: &[&str] = &[
    "vol",
    "vols",
    "avion",
    "aeroport",
    "tgv",
    "gare",
    "ferry",
    "flight",
    "flights",
    "airport",
    "boarding",
    "flug",
    "flughafen",
    "bahnhof",
    "vuelo",
    "vuelos",
    "aeropuerto",
];
const STAYING: &[&str] = &[
    "hotel",
    "airbnb",
    "gite",
    "camping",
    "auberge",
    "hostel",
    "unterkunft",
    "ferienwohnung",
    "alojamiento",
    "hostal",
];
const BOOKED: &[&str] = &[
    "reservation",
    "reserve",
    "reservee",
    "reserves",
    "billet",
    "billets",
    "booking",
    "booked",
    "ticket",
    "tickets",
    "buchung",
    "gebucht",
    "reserva",
    "reservado",
    "check in",
    "pnr",
];

fn has_any(folded: &str, words: &[&str]) -> bool {
    let spaced = format!(" {folded} ");
    words.iter().any(|w| spaced.contains(&format!(" {w} ")))
}

/// Whether a subject is clearly a trip by its words (no model): a date, and a trip word with a booking, a flight or a
/// place to stay — or both a way of getting there and a place to stay.
pub fn looks_like_trip(page: &Page) -> bool {
    let text = format!("{}\n{}\n{}", page.title, page.aliases().join("\n"), page.body);
    let folded = fold(&text);
    let dated = page.body.lines().any(|l| super::date_in(l).is_some());
    if !dated {
        return false;
    }
    let trip = has_any(&folded, TRIP_WORDS);
    let there = has_any(&folded, GETTING_THERE);
    let stay = has_any(&folded, STAYING);
    let booked = has_any(&folded, BOOKED);
    (trip && (booked || there || stay)) || (there && stay)
}

/// The page made a trip: its kind, its content in the trip's outline (nothing dropped), the packing lines with their
/// boxes, its dates in its frontmatter when they can be read.
pub fn make_trip(page: &mut Page, lang: &str) {
    page.front.insert("type".into(), json!("trip"));
    let brief = super::words(lang).brief;
    let shaped = template::normalize("trip", lang, brief, &page.body).body;
    let packing = fold(&template::title("trip", "packing", lang));
    let mut in_packing = false;
    let lines: Vec<String> = shaped
        .lines()
        .map(|l| {
            if let Some(name) = l.strip_prefix("## ") {
                in_packing = fold(name) == packing;
                return l.to_string();
            }
            match l.strip_prefix("- ") {
                Some(item) if in_packing => format!("- {}", unticked(item)),
                _ => l.to_string(),
            }
        })
        .collect();
    page.body = lines.join("\n") + "\n";
    let (start, end) = dates_of(page);
    for (key, date) in [("start", start), ("end", end)] {
        if let Some(d) = date {
            if !page.front.contains_key(key) {
                page.front.insert(key.into(), json!(d.format("%Y-%m-%d").to_string()));
            }
        }
    }
}

/// The subjects that are clearly trips turned into trips (the nightly pass, free), each page once: one the person
/// turned back into a subject stays one. Returns how many.
pub fn detect(config: &Config) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let lang = super::language(config);
    let seen = super::strings(super::read_state(config).get("trips_seen"));
    let who = Who::tidy(config);
    let mut done: Vec<String> = Vec::new();
    for (path, mut page) in super::all_pages(config, Part::Main) {
        if page.kind() != "subject" || page.id().is_empty() || seen.contains(&page.id()) || !looks_like_trip(&page) {
            continue;
        }
        let before = page.render();
        make_trip(&mut page, &lang);
        page.touch();
        let summary = format!("{} : {}", super::words(&lang).tidy, trip_word(&lang));
        super::commit(config, Part::Main, &path, Some(&before), Some(&page.render()), &who, "trip", &summary)?;
        super::archive::place_one_locked(config, &path, &page, chrono::Local::now().date_naive(), &who)?;
        done.push(page.id());
    }
    if !done.is_empty() {
        super::update_state(config, |s| {
            let mut all = super::strings(s.get("trips_seen"));
            all.extend(done.iter().cloned());
            s.insert("trips_seen".into(), json!(all));
        })?;
    }
    Ok(done.len())
}

/// A subject made a trip because the nightly pass's model says it is one (its « trip » line). The caller holds the
/// lock. Ok(false): no such subject.
pub fn convert_locked(config: &Config, query: &str, who: &Who) -> anyhow::Result<bool> {
    let Some(found) = super::find(config, &[Part::Main], query) else { return Ok(false) };
    if found.page.kind() != "subject" {
        return Ok(false);
    }
    let lang = super::language(config);
    let mut page = found.page;
    let before = page.render();
    page.ensure_id();
    make_trip(&mut page, &lang);
    page.touch();
    let summary = format!("{} : {}", super::words(&lang).tidy, trip_word(&lang));
    super::commit(config, Part::Main, &found.path, Some(&before), Some(&page.render()), who, "trip", &summary)?;
    super::archive::place_one_locked(config, &found.path, &page, chrono::Local::now().date_naive(), who)?;
    let id = page.id();
    super::update_state(config, |s| {
        let mut all = super::strings(s.get("trips_seen"));
        all.push(id.clone());
        s.insert("trips_seen".into(), json!(all));
    })?;
    Ok(true)
}

fn trip_word(lang: &str) -> &'static str {
    match lang {
        "fr" => "voyage",
        "de" => "Reise",
        "es" => "viaje",
        _ => "trip",
    }
}

/// A trip's dates and destination set by an agent (notebook_note's start, end, destination): in its frontmatter.
pub fn set_details(
    config: &Config,
    part: Part,
    path: &str,
    start: &str,
    end: &str,
    destination: &str,
    who: &Who,
) -> anyhow::Result<bool> {
    let _guard = super::lock();
    let Some(before) = super::read_page_text(config, part, path) else { return Ok(false) };
    let mut page = Page::parse(&before, "");
    if page.kind() != "trip" {
        return Ok(false);
    }
    for (key, value) in [("start", start), ("end", end)] {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let date = super::due_date(value)?;
        page.front.insert(key.into(), json!(date));
    }
    let destination = tk::head(&tk::squeeze(destination.trim()), 120).to_string();
    if !destination.is_empty() {
        page.front.insert("destination".into(), json!(destination));
    }
    let after = page.render();
    if after == before {
        return Ok(false);
    }
    let summary =
        [start, end, destination.as_str()].iter().filter(|v| !v.trim().is_empty()).cloned().collect::<Vec<_>>();
    super::commit(config, part, path, Some(&before), Some(&after), who, "trip", &summary.join(" → "))?;
    Ok(true)
}

// -- reading a trip -----------------------------------------------------------------------------------------------------

static RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(\d{1,2})(?:er)?\s*(?:au|-|–|to|bis|al|y el)\s*(\d{1,2})(?:er|st|nd|rd|th|\.)?\s+([a-z]{3,10})\b(?:\s+(\d{4}))?")
        .expect("valid regex")
});
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([01]?\d|2[0-3])[:hH]([0-5]\d)\b").expect("valid regex"));
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:r[ée]f(?:[ée]rence)?|ref|booking|confirmation|pnr|dossier|n°|nr|num[ée]ro|number|buchung(?:snummer)?|localizador|reserva|code)\.?\s*[:#°]?\s*([A-Za-z0-9][A-Za-z0-9-]{3,19})",
    )
    .expect("valid regex")
});
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z0-9]{6})\b").expect("valid regex"));
static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b((?:TGV|ICE|AVE|EC|IC|[A-Z]{2}|[A-Z]\d|\d[A-Z])\s?\d{2,4})\b").expect("valid regex")
});

/// A date in a line, with the year when it has none: the one that puts it within a year from `around`.
pub fn date_of(text: &str, around: NaiveDate) -> Option<NaiveDate> {
    let (month, day, year) = super::date_in(text)?;
    match year {
        Some(y) => NaiveDate::from_ymd_opt(y, month, day),
        None => {
            let this = NaiveDate::from_ymd_opt(around.year(), month, day)?;
            if (around - this).num_days() > 60 {
                NaiveDate::from_ymd_opt(around.year() + 1, month, day)
            } else {
                Some(this)
            }
        }
    }
}

fn front_date(page: &Page, key: &str) -> Option<NaiveDate> {
    let text = tk::str_or_empty(page.front.get(key));
    NaiveDate::parse_from_str(tk::head(text.trim(), 10), "%Y-%m-%d").ok()
}

fn section_facts(page: &Page, lang: &str, id: &str) -> Vec<super::Fact> {
    let title = fold(&template::title("trip", id, lang));
    page.sections().into_iter().filter(|(name, _)| fold(name) == title).flat_map(|(_, facts)| facts).collect()
}

fn section_facts_any(page: &Page, id: &str) -> Vec<super::Fact> {
    page.sections()
        .into_iter()
        .filter(|(name, _)| template::resolve("trip", name) == Some(id))
        .flat_map(|(_, facts)| facts)
        .collect()
}

/// The trip's first and last days: its frontmatter's `start` and `end`, else the dates its « Dates » holds (« du 12
/// au 19 novembre » too), else those of its itinerary and bookings.
pub fn dates_of(page: &Page) -> (Option<NaiveDate>, Option<NaiveDate>) {
    let today = chrono::Local::now().date_naive();
    let (mut start, mut end) = (front_date(page, "start"), front_date(page, "end"));
    if start.is_some() && end.is_some() {
        return (start, end);
    }
    let mut found: Vec<NaiveDate> = Vec::new();
    for ids in [&["dates"][..], &["itinerary", "transport", "lodging"][..]] {
        for id in ids {
            for fact in section_facts_any(page, id) {
                let folded = tk::ascii_fold(&fact.text.to_lowercase());
                if let Some(c) = RANGE.captures(&folded) {
                    let year = c.get(4).map(|y| y.as_str().to_string()).unwrap_or_default();
                    for day in [&c[1], &c[2]] {
                        if let Some(d) = date_of(&format!("{day} {} {year}", &c[3]), today) {
                            found.push(d);
                        }
                    }
                    continue;
                }
                if let Some(d) = date_of(&fact.text, today) {
                    found.push(d);
                }
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    start = start.or_else(|| found.iter().min().copied());
    end = end.or_else(|| found.iter().max().copied()).or(start);
    (start, end)
}

/// What a booking line holds: its date, time, flight or train number and booking reference.
pub fn booking_parts(text: &str, around: NaiveDate) -> Value {
    let date = date_of(text, around).map(|d| d.format("%Y-%m-%d").to_string());
    let time = TIME.captures(text).map(|c| format!("{:0>2}:{}", &c[1], &c[2]));
    let number = NUMBER.captures(text).map(|c| c[1].to_string());
    let plain_number = number.as_deref().map(|n| n.replace(' ', "")).unwrap_or_default();
    let reference = REFERENCE
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .find(|r| (r.chars().any(|c| c.is_ascii_digit()) || r.chars().all(|c| !c.is_lowercase())) && *r != plain_number)
        .or_else(|| {
            CODE.captures_iter(text).map(|c| c[1].to_string()).find(|r| {
                r.chars().any(|c| c.is_ascii_digit())
                    && r.chars().any(|c| c.is_ascii_uppercase())
                    && number.as_deref().map(|n| n.replace(' ', "") != *r).unwrap_or(true)
            })
        });
    json!({"date": date, "time": time, "number": number, "reference": reference})
}

/// A place's name in a line: what comes before its first « : », « , », « · » or « — ».
fn place_name(text: &str) -> String {
    let text = super::LINK.replace_all(text, |c: &regex::Captures| c[1].to_string()).to_string();
    let cut = text.find([':', ',', '·', '—', '(']).unwrap_or(text.len());
    tk::head(text[..cut].trim(), 80).trim().to_string()
}

/// The trip for the apps' layout: its dates and destination, the days, the bookings (its own lines and the
/// booking records that name it), the packing list, its places and the agents' page with a map linked to it.
pub fn view(config: &Config, part: Part, path: &str, page: &Page) -> Value {
    let lang = super::language(config);
    let (start, end) = dates_of(page);
    let around = start.unwrap_or_else(|| chrono::Local::now().date_naive());
    let length = match (start, end) {
        (Some(s), Some(e)) if e >= s => Some((e - s).num_days() + 1),
        _ => None,
    };
    // the days: « J2 », « Day 2 »… or a date, in order; the lines without either after them
    let mut days: Vec<(i64, Value)> = Vec::new();
    for (n, fact) in section_facts(page, &lang, "itinerary").into_iter().enumerate() {
        let mut text = fact.text.clone();
        let mut day: Option<i64> = None;
        if let Some(c) = template::DAY_MARK.captures(&fact.text) {
            day = c[1].parse().ok();
            text = fact.text[c.get(0).map(|m| m.end()).unwrap_or(0)..].to_string();
        }
        let mut date = date_of(&text, around);
        if day.is_none() {
            if let (Some(d), Some(s)) = (date, start) {
                day = Some((d - s).num_days() + 1);
            }
        }
        if date.is_none() {
            if let (Some(n), Some(s)) = (day, start) {
                date = s.checked_add_signed(chrono::Duration::days(n - 1));
            }
        }
        let title = text.trim_start_matches(|c: char| c.is_whitespace() || ":·-–—,.".contains(c)).to_string();
        // a date at its start is shown apart: « 12/11 : Tokyo » → « Tokyo »
        let title = match super::ISO_DATE.find(&title).or_else(|| super::NUMERIC.find(&title)) {
            Some(m) if m.start() == 0 => {
                title[m.end()..].trim_start_matches(|c: char| c.is_whitespace() || ":·-–—,.".contains(c)).to_string()
            }
            _ => title,
        };
        let order = day.unwrap_or(1000 + n as i64);
        days.push((
            order,
            json!({"day": day, "date": date.map(|d| d.format("%Y-%m-%d").to_string()), "text": title,
                   "line": fact.text, "place": place_name(&title)}),
        ));
    }
    days.sort_by_key(|d| d.0);
    let mut bookings: Vec<Value> = Vec::new();
    for id in ["transport", "lodging"] {
        for fact in section_facts(page, &lang, id) {
            let mut b = booking_parts(&fact.text, around);
            b["kind"] = json!(id);
            b["text"] = json!(fact.text);
            bookings.push(b);
        }
    }
    for record in super::records::bookings_for(config, page, start, end) {
        let text = ["what", "provider"]
            .iter()
            .map(|k| tk::str_or_empty(record.get(*k)))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let mut b = booking_parts(&format!("{text} {}", tk::str_or_empty(record.get("date"))), around);
        let reference = tk::str_or_empty(record.get("reference"));
        if !reference.is_empty() {
            b["reference"] = json!(reference);
        }
        b["kind"] = json!("booking");
        b["text"] = json!(text);
        b["record"] = json!(tk::str_or_empty(record.get("id")));
        b["source"] = record.get("source").cloned().unwrap_or(Value::Null);
        bookings.push(b);
    }
    bookings.sort_by_key(|b| {
        (tk::str_or_empty(b.get("date")).is_empty(), tk::str_or_empty(b.get("date")), tk::str_or_empty(b.get("time")))
    });
    let packing: Vec<Value> = section_facts(page, &lang, "packing")
        .iter()
        .map(|f| {
            let (done, text) = checkbox(&f.text);
            json!({"text": text, "done": done})
        })
        .collect();
    let mut places: Vec<Value> = Vec::new();
    let mut add_place = |name: String, line: &str, section: &str| {
        if name.chars().count() >= 2 && !places.iter().any(|p| fold(&tk::str_or_empty(p.get("name"))) == fold(&name)) {
            places.push(json!({"name": name, "line": line, "section": section}));
        }
    };
    for d in &days {
        add_place(tk::str_or_empty(d.1.get("place")), &tk::str_or_empty(d.1.get("text")), "itinerary");
    }
    for id in ["lodging", "onsite"] {
        for f in section_facts(page, &lang, id) {
            add_place(place_name(&f.text), &f.text, id);
        }
    }
    json!({"start": start.map(|d| d.format("%Y-%m-%d").to_string()),
           "end": end.map(|d| d.format("%Y-%m-%d").to_string()), "days_count": length,
           "destination": page.front.get("destination").and_then(Value::as_str),
           "days": days.into_iter().map(|d| d.1).collect::<Vec<_>>(), "bookings": bookings, "packing": packing,
           "places": places, "map_page": map_page(config, part, path)})
}

/// The agents' page with a map (the page kit's live map, maps.rs) linked to this Carnet page, if any.
fn map_page(config: &Config, part: Part, path: &str) -> Value {
    for d in super::documents(config, part) {
        if d.get("linked").and_then(|l| l.get("path")).and_then(Value::as_str) != Some(path) {
            continue;
        }
        let (agent, id) = (tk::str_or_empty(d.get("agent")), tk::str_or_empty(d.get("page")));
        if id.is_empty() {
            continue;
        }
        let Ok(source) = crate::pages::source(config, &agent, &id) else { continue };
        if source.contains("map:") || source.contains("data-lat") {
            return json!({"agent": agent, "page": id, "title": d.get("name"), "path": d.get("path")});
        }
    }
    Value::Null
}

/// The trips to show in the Carnet's home: from `HOME_DAYS` days before they start until they end, the soonest first.
pub fn upcoming(config: &Config, today: NaiveDate) -> Vec<Value> {
    let mut out: Vec<(NaiveDate, Value)> = Vec::new();
    for (path, page) in super::all_pages(config, Part::Main) {
        if page.kind() != "trip" {
            continue;
        }
        let (Some(start), end) = dates_of(&page) else { continue };
        let end = end.unwrap_or(start);
        let until = (start - today).num_days();
        if until > HOME_DAYS || end < today {
            continue;
        }
        let day = if until <= 0 { Some((today - start).num_days() + 1) } else { None };
        out.push((
            start,
            json!({"path": path, "title": page.title, "destination": page.front.get("destination").and_then(Value::as_str),
                   "start": start.format("%Y-%m-%d").to_string(), "end": end.format("%Y-%m-%d").to_string(),
                   "in_days": until.max(0), "day": day, "days_count": (end - start).num_days() + 1,
                   "icon": page.icon()}),
        ));
    }
    out.sort_by_key(|t| t.0);
    out.into_iter().map(|t| t.1).collect()
}

/// A thing to pack ticked (or not) by the person in the app. Ok(false) when the page has no such line.
pub fn tick(config: &Config, part: Part, path: &str, item: &str, done: bool, who: &Who) -> anyhow::Result<bool> {
    let _guard = super::lock();
    super::checked(path)?;
    let Some(before) = super::read_page_text(config, part, path) else { return Ok(false) };
    let mut page = Page::parse(&before, "");
    let lang = super::language(config);
    let packing = fold(&template::title("trip", "packing", &lang));
    let wanted = fold(without_box(item));
    let mut in_packing = false;
    let mut changed = false;
    let mut lines: Vec<String> = page.body.lines().map(str::to_string).collect();
    for line in lines.iter_mut() {
        if let Some(name) = line.strip_prefix("## ") {
            in_packing = fold(name) == packing;
            continue;
        }
        let Some(rest) = line.trim_start().strip_prefix("- ") else { continue };
        if !in_packing || changed {
            continue;
        }
        let (text, by) = super::split_attribution(rest);
        if fold(without_box(&text)) != wanted {
            continue;
        }
        let (was, plain) = checkbox(&text);
        if was == done {
            return Ok(true);
        }
        let attribution = by.map(|(who, date)| format!(" — *{who}, {date}*")).unwrap_or_default();
        *line = format!("- [{}] {plain}{attribution}", if done { "x" } else { " " });
        changed = true;
    }
    if !changed {
        return Ok(false);
    }
    page.body = lines.join("\n") + "\n";
    page.touch();
    super::commit(config, part, path, Some(&before), Some(&page.render()), who, "check", tk::head(item, 200))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subject_that_is_a_trip_keeps_everything() {
        let body = "## En bref\nJapon en novembre.\n\n## Où on en est\n- Tout est réservé — *Léa, 2026-10-20*\n\n\
## Décisions\n- Vol AF276 le 12/11/2026 à 10:35, réf X7K2PQ — *Léa, 2026-10-20*\n- Hôtel Gion à Kyoto, 3 nuits — *Léa, 2026-10-21*\n\n\
## Prochaines étapes\n- J2 : Kyoto, temples — *Léa, 2026-10-21*\n- Ne pas oublier l'adaptateur à emporter — *Léa, 2026-10-21*\n\n\
## Historique\n- 2026-10-19 : parlé avec Marc — *Léa, 2026-10-19*\n";
        let mut page = Page::parse(&format!("---\ntype: subject\n---\n# Voyage au Japon\n\n{body}"), "x");
        assert!(looks_like_trip(&page));
        make_trip(&mut page, "fr");
        assert_eq!(page.kind(), "trip");
        let text = page.render();
        for fact in ["Tout est réservé", "Vol AF276", "Hôtel Gion", "J2 : Kyoto", "adaptateur", "parlé avec Marc"] {
            assert!(text.contains(fact), "{fact} lost:\n{text}");
        }
        assert!(text.contains("## Transports\n- Vol AF276"), "{text}");
        assert!(text.contains("## Hébergements\n- Hôtel Gion"), "{text}");
        assert!(text.contains("## Itinéraire\n- J2 : Kyoto"), "{text}");
        assert!(text.contains("## À emporter\n- [ ] Ne pas oublier"), "{text}");
        assert!(text.contains("## Notes\n- Où on en est : Tout est réservé"), "{text}");
        assert!(text.contains("start: 2026-11-12"), "{text}");
        let b =
            booking_parts("Vol AF276 le 12/11/2026 à 10:35, réf X7K2PQ", NaiveDate::from_ymd_opt(2026, 11, 1).unwrap());
        assert_eq!(b["date"], "2026-11-12");
        assert_eq!(b["time"], "10:35");
        assert_eq!(b["number"], "AF276");
        assert_eq!(b["reference"], "X7K2PQ");
        let b = booking_parts(
            "Train TGV 6201 Paris → Lyon 08h04, dossier QWERTY",
            NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
        );
        assert_eq!(b["time"], "08:04");
        assert_eq!(b["number"], "TGV 6201");
        assert_eq!(b["reference"], "QWERTY");
    }

    #[test]
    fn a_subject_is_not_a_trip_without_clear_words() {
        let page = Page::parse("---\ntype: subject\n---\n# Cuisine\n\n## Décisions\n- Devis le 12/11 : 480 €\n", "x");
        assert!(!looks_like_trip(&page));
        let page = Page::parse("---\ntype: subject\n---\n# Vacances\n\n## Notes\n- Idées : la mer\n", "x");
        assert!(!looks_like_trip(&page), "no date");
        let page = Page::parse("---\ntype: subject\n---\n# Lisbonne\n\n## Notes\n- Vol le 3 mai, hôtel Alfama\n", "x");
        assert!(looks_like_trip(&page));
    }

    #[test]
    fn dates_come_from_a_range() {
        let page = Page::parse("---\ntype: trip\n---\n# Rome\n\n## Dates\n- Du 12 au 19 novembre 2026\n", "x");
        let (s, e) = dates_of(&page);
        assert_eq!(s, NaiveDate::from_ymd_opt(2026, 11, 12));
        assert_eq!(e, NaiveDate::from_ymd_opt(2026, 11, 19));
    }
}
