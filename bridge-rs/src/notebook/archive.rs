//! « Archives » (the owner, 2026-10-23): what is past leaves the Carnet's lists for an « Archives » folder
//! (Archive, Archiv, Archivo), Obsidian-readable, by the nightly pass (and once at the bridge's start), never deleted:
//!
//!     Archives/Voyages/Lisbonne — nov. 2025.md     a trip once its last day is over (the page as it was)
//!     Archives/Fiches/Réservations.md               the same table as Fiches/…: a booking a few days after its date,
//!     Archives/Fiches/Achats.md                     a purchase whose warranty is over (or two years old without one),
//!     Archives/Fiches/Abonnements.md                a subscription that says it ended (or a year past its renewal),
//!     Archives/Fiches/Contrats et assurances.md     a contract three months past its end, a paper replaced by a newer
//!     Archives/Fiches/Papiers.md                    one (an expired one that is not replaced stays, with its proposal)
//!
//! Links keep working: Obsidian finds a page by its name wherever it is, the bridge by its title, aliases or id; a
//! record keeps its id. The agents still find what is archived (the search and the records' list mark it). Every move
//! is in the Carnet's history and undone there; what the person brings back stays back (a trip restored from the
//! history, a record put back in the live table or with the tool's « unarchive »: `archive_kept`, `records_kept`).
//! The archived tasks stay where they are (under « Archivées » in the tasks' page); the apps count them in the archives.
//!
//! The same pass puts each trip in its folder (`Voyages`, apart from the subjects: the migration of the pages made
//! among them before), and runs the trips' and purchases' migrations of 2026-10-23 (trip_auto.rs, records.rs).

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::records::{self, Kind, Table};
use super::{fold, Page, Part, Who};
use crate::config::Config;
use crate::tools_knowledge as tk;
use crate::Dict;

/// A booking is archived this many days after its date (a stay lasts a few nights).
pub const BOOKING_DAYS: i64 = 3;
/// A subscription a year past its renewal date with no newer one has ended.
pub const SUBSCRIPTION_DAYS: i64 = 365;
/// A contract this many days past its end.
pub const CONTRACT_DAYS: i64 = 90;
/// A purchase without a warranty this old.
pub const PURCHASE_MONTHS: u32 = 24;

/// What a pass did.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Done {
    pub classes: usize,
    pub trips: super::trip_auto::Synced,
    pub placed: usize,
    pub archived_trips: usize,
    pub archived_records: usize,
}

impl Done {
    pub fn view(&self) -> Dict {
        let mut out = Dict::new();
        for (k, v) in [
            ("classes", self.classes),
            ("trips_created", self.trips.created),
            ("trips_renamed", self.trips.renamed),
            ("trips_merged", self.trips.merged),
            ("trips_to_records", self.trips.demoted),
            ("trips_placed", self.placed),
            ("archived_trips", self.archived_trips),
            ("archived_records", self.archived_records),
        ] {
            if v > 0 {
                out.insert(k.into(), json!(v));
            }
        }
        out
    }
}

/// The whole pass (free, no model): purchases' classes, the trips from bookings, each trip in its folder, what is past
/// archived. Idempotent.
pub fn run(config: &Config, today: NaiveDate) -> anyhow::Result<Done> {
    let mut done = Done { classes: records::settle_classes(config)?, ..Done::default() };
    done.trips = super::trip_auto::sync(config, &Who::tidy(config), today)?;
    let _guard = super::lock();
    let (placed, archived) = place_trips_locked(config, today)?;
    done.placed = placed;
    done.archived_trips = archived;
    done.archived_records = archive_records_locked(config, today)?;
    Ok(done)
}

fn date_of(text: &str) -> Option<NaiveDate> {
    let (m, d, y) = super::date_in(text)?;
    NaiveDate::from_ymd_opt(y?, m, d)
}

// -- trips -----------------------------------------------------------------------------------------------------------

/// Each trip page in its folder: `Voyages`, `Archives/Voyages` once over (a trip the person brought back stays).
/// Returns (moved to `Voyages`, archived).
pub fn place_trips_locked(config: &Config, today: NaiveDate) -> anyhow::Result<(usize, usize)> {
    let who = Who::tidy(config);
    let (mut placed, mut archived) = (0, 0);
    for (path, page) in super::all_pages(config, Part::Main) {
        match place_one_locked(config, &path, &page, today, &who)? {
            Some((_, true)) => archived += 1,
            Some((_, false)) => placed += 1,
            None => {}
        }
    }
    Ok((placed, archived))
}

/// One trip page in its folder: Some((its new path, archived)) when it moved.
pub fn place_one_locked(
    config: &Config,
    path: &str,
    page: &Page,
    today: NaiveDate,
    who: &Who,
) -> anyhow::Result<Option<(String, bool)>> {
    if page.kind() != "trip" {
        return Ok(None);
    }
    let lang = super::language(config);
    let w = super::words(&lang);
    let kept = super::strings(super::read_state(config).get("archive_kept"));
    let folder = path.rsplit_once('/').map(|(f, _)| f.to_string()).unwrap_or_default();
    let end = super::trip::dates_of(page).1;
    let past = end.is_some_and(|e| e < today) && !kept.contains(&page.id());
    let wanted = if past { w.past_trips() } else { w.trips.to_string() };
    // a trip archived by hand (in Obsidian) or kept by the person where it is: left
    if folder == wanted || (folder == w.past_trips() && !end.is_some_and(|e| e >= today)) {
        return Ok(None);
    }
    let Some(text) = super::read_page_text(config, Part::Main, path) else { return Ok(None) };
    let to = super::free_path(config, Part::Main, &wanted, &page.title, path);
    let (action, summary) = if past {
        ("archive", format!("{} → {}", page.title, w.past_trips()))
    } else {
        ("move", format!("{} → {}", page.title, w.trips))
    };
    super::move_page_locked(config, Part::Main, path, &to, &text, who, action, &summary)?;
    Ok(Some((to, past)))
}

// -- records ---------------------------------------------------------------------------------------------------------

const ENDED: &[&str] = &[
    "resilie",
    "resiliee",
    "annule",
    "annulee",
    "termine",
    "terminee",
    "fini",
    "cancelled",
    "canceled",
    "ended",
    "expired",
    "gekundigt",
    "beendet",
    "abgelaufen",
    "cancelado",
    "cancelada",
    "finalizado",
    "terminado",
];

fn says_ended(record: &Dict, fields: &[&str]) -> bool {
    fields.iter().any(|f| {
        let folded = format!(" {} ", fold(&tk::str_or_empty(record.get(*f))));
        ENDED.iter().any(|w| folded.contains(&format!(" {w} ")))
    })
}

/// Whether a record is past (archived at night). `rows`: its type's live records (a paper replaced by a newer one).
pub fn is_past(t: &Kind, record: &Dict, rows: &[Dict], today: NaiveDate) -> bool {
    let day = |k: &str| date_of(&tk::str_or_empty(record.get(k)));
    match t.id {
        "booking" => day("date").is_some_and(|d| (today - d).num_days() > BOOKING_DAYS),
        "purchase" => match day("warranty_end") {
            Some(end) => end < today,
            None => day("date").is_some_and(|d| {
                d.checked_add_months(chrono::Months::new(PURCHASE_MONTHS)).is_some_and(|until| until < today)
            }),
        },
        "subscription" => {
            says_ended(record, &["renewal", "period"])
                || day("renewal").is_some_and(|d| (today - d).num_days() > SUBSCRIPTION_DAYS)
        }
        "contract" => {
            says_ended(record, &["end"]) || day("end").is_some_and(|d| (today - d).num_days() > CONTRACT_DAYS)
        }
        "paper" => {
            // replaced: the same document with a later expiry (an expired one alone stays, with its proposal)
            let name = fold(&tk::str_or_empty(record.get("document")));
            let mine = day("expires");
            !name.is_empty()
                && mine.is_some()
                && rows.iter().any(|r| {
                    tk::str_or_empty(r.get("id")) != tk::str_or_empty(record.get("id"))
                        && fold(&tk::str_or_empty(r.get("document"))) == name
                        && date_of(&tk::str_or_empty(r.get("expires"))).is_some_and(|e| Some(e) > mine)
                })
        }
        _ => false,
    }
}

/// The records that are past moved to their archived table; a record found in both (the person restored the live
/// table from its history) stays live and is kept from now on. Returns how many moved.
pub fn archive_records_locked(config: &Config, today: NaiveDate) -> anyhow::Result<usize> {
    let lang = super::language(config);
    let w = super::words(&lang);
    let who = Who::tidy(config);
    let mut kept = super::strings(super::read_state(config).get("records_kept"));
    let kept_before = kept.len();
    let mut moved = 0;
    for t in records::TYPES.iter() {
        let (path, before, mut page, mut table) = records::read_at(config, t, false);
        let (apath, abefore, mut apage, mut atable) = records::read_at(config, t, true);
        // brought back by the person: out of the archived table, kept live
        let live: Vec<String> = table.rows.iter().map(|r| tk::str_or_empty(r.get("id"))).collect();
        let doubled: Vec<String> = atable
            .rows
            .iter()
            .map(|r| tk::str_or_empty(r.get("id")))
            .filter(|id| !id.is_empty() && live.contains(id))
            .collect();
        let mut archive_changed = false;
        if !doubled.is_empty() {
            atable.rows.retain(|r| !doubled.contains(&tk::str_or_empty(r.get("id"))));
            for id in &doubled {
                if !kept.contains(id) {
                    kept.push(id.clone());
                }
            }
            archive_changed = true;
        }
        let rows = table.rows.clone();
        let (going, staying): (Vec<Dict>, Vec<Dict>) = table
            .rows
            .drain(..)
            .partition(|r| !kept.contains(&tk::str_or_empty(r.get("id"))) && is_past(t, r, &rows, today));
        table.rows = staying;
        if !going.is_empty() {
            atable.rows.extend(going.iter().cloned());
            moved += going.len();
            archive_changed = true;
        }
        if archive_changed {
            let summary = if going.is_empty() {
                doubled.join(" · ")
            } else {
                going.iter().map(|r| records::summary_head(t, r)).collect::<Vec<_>>().join(" · ")
            };
            apage.body = records::render_body(
                t,
                &lang,
                &Table { before: atable.before, rows: atable.rows, after: atable.after },
            );
            apage.front.insert("archived".into(), json!(true));
            apage.touch();
            super::commit(
                config,
                Part::Main,
                &apath,
                abefore.as_deref(),
                Some(&apage.render()),
                &who,
                "archive",
                &summary,
            )?;
        }
        if !going.is_empty() && before.is_some() {
            let summary = format!(
                "→ {} : {}",
                w.archives,
                going.iter().map(|r| records::summary_head(t, r)).collect::<Vec<_>>().join(" · ")
            );
            page.body = records::render_body(t, &lang, &table);
            page.touch();
            super::commit(
                config,
                Part::Main,
                &path,
                before.as_deref(),
                Some(&page.render()),
                &who,
                "archive",
                &summary,
            )?;
        }
    }
    if kept.len() != kept_before {
        super::update_state(config, |s| {
            s.insert("records_kept".into(), json!(kept));
        })?;
    }
    Ok(moved)
}

/// A record moved by hand (the tool's « archive » / « unarchive », the app): Ok(None) when there is no such id where
/// it would come from. Unarchived, it stays live from then on; archived, it may come back the same way.
pub fn set_record_archived(
    config: &Config,
    t: &'static Kind,
    id: &str,
    archived: bool,
    who: &Who,
) -> anyhow::Result<Option<Dict>> {
    let _guard = super::lock();
    let lang = super::language(config);
    let id = id.trim().to_string();
    let (fpath, fbefore, mut fpage, mut ftable) = records::read_at(config, t, !archived);
    let Some(i) = ftable.rows.iter().position(|r| tk::str_or_empty(r.get("id")) == id) else { return Ok(None) };
    let record = ftable.rows.remove(i);
    let (tpath, tbefore, mut tpage, mut ttable) = records::read_at(config, t, archived);
    ttable.rows.push(record.clone());
    let w = super::words(&lang);
    let summary =
        format!("{} {} : {}", if archived { "→" } else { "←" }, w.archives, records::summary_head(t, &record));
    tpage.body = records::render_body(t, &lang, &ttable);
    if archived {
        tpage.front.insert("archived".into(), json!(true));
    }
    tpage.touch();
    let action = if archived { "archive" } else { "unarchive" };
    super::commit(config, Part::Main, &tpath, tbefore.as_deref(), Some(&tpage.render()), who, action, &summary)?;
    fpage.body = records::render_body(t, &lang, &ftable);
    fpage.touch();
    super::commit(config, Part::Main, &fpath, fbefore.as_deref(), Some(&fpage.render()), who, action, &summary)?;
    super::update_state(config, |s| {
        let mut kept = super::strings(s.get("records_kept"));
        kept.retain(|k| *k != id);
        if !archived {
            kept.push(id.clone());
        }
        s.insert("records_kept".into(), json!(kept));
    })?;
    Ok(Some(record))
}

/// The archives' counts for the apps' tile: past trips, each type's archived records, archived tasks.
pub fn counts(config: &Config) -> Value {
    let lang = super::language(config);
    let w = super::words(&lang);
    let trips =
        super::all_pages(config, Part::Main).iter().filter(|(path, p)| p.kind() == "trip" && w.archived(path)).count();
    let mut out = Dict::new();
    out.insert("trip".into(), json!(trips));
    let mut total = trips;
    for t in records::TYPES.iter() {
        let n = records::archived_records(config, t).len();
        total += n;
        out.insert(t.id.into(), json!(n));
    }
    let tasks = super::tasks(config, Part::Main).iter().filter(|t| t.archived && !t.done).count();
    total += tasks;
    out.insert("tasks".into(), json!(tasks));
    json!({"title": w.archives, "total": total, "counts": out, "trips_path": w.past_trips()})
}
