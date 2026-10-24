//! The person's time and their standard dates (decided 2026-10-24, to sell in the US with servers in the EU).
//!
//! **Time zone.** The apps report the phone's IANA zone (at launch and when it changes); with « Fuseau horaire
//! automatique » (on by default) it becomes the person's zone, else the one they picked. Each agent's Hermes profile
//! gets it as `timezone` (prompts' dates, `execute_code`, and with Hermes patch 0008 the terminal's `TZ`), so the
//! agent's « now » follows at once. Routines are another matter (Alex, 2026-10-24): someone who flies from Paris to
//! Tokyo does not want their 7 h routine silently moved to 7 h in Tokyo, nor kept without being asked. The routines
//! keep their zone (`cron.timezone`, patch 0008) and the person is asked once, in the app: « Tu es maintenant à l'heure
//! de Tokyo (+8 h). Tes routines restent à l'heure de Paris : … » → « Les passer à l'heure de Tokyo » / « Les garder à
//! l'heure de Paris » (« Revenir comme avant » when the new zone is the one they had before). The answer holds until
//! the zone changes again. No routine at all: nothing to ask, they follow. Without patch 0008 Hermes ignores
//! `cron.timezone` (routines follow `timezone`) and caches the zone: its gateway is restarted after a change.
//!
//! **Standard dates** (standard.rs): clock changes, public holidays of the person's country and region, French school
//! holidays of their zone, computed, never stored. Agents get them with the `upcoming_dates` tool, and only when one is
//! within 7 days, ONE short line in their personality's context (`<!-- bipagents:dates -->`, refreshed hourly). The
//! Carnet's « Dates à venir » shows them as computed entries.
//!
//!     GET   /v1/time                 the view: zone, auto, the phone's, the routines', the question, country, region,
//!                                    school zone, the lists to pick from (`?lang=` fr, en, de, es)
//!     PUT   /v1/time/device          {time_zone}   the phone's zone (at launch, when it changes)
//!     PATCH /v1/time                 {auto?, time_zone?, country?, region?, school_zone?, postal_code?}
//!     POST  /v1/time/routines        {choice: "move" | "keep"}   the answer to the question
//!     GET   /v1/dates?days=          the standard dates for the person (default 60, at most 366)
//!     data_dir/time.json             the person's settings (0600)
//!
//! No Python counterpart.

pub mod standard;
pub mod words;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use chrono::{NaiveDate, NaiveTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents_admin::pyyaml::Yaml;
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::{py, Dict};

pub use standard::{For, Item, Kind};

static LOCK: Mutex<()> = Mutex::new(());

pub const DATES_START: &str = "<!-- bipagents:dates -->";
pub const DATES_END: &str = "<!-- /bipagents:dates -->";
/// The agents' tool.
pub const TOOL_NAME: &str = "upcoming_dates";
pub const MAX_DAYS: i64 = 366;
/// How many routines the question names.
const NAMED_ROUTINES: usize = 4;

fn yes() -> bool {
    true
}

/// The question waiting for the person: their routines run at `from`'s time, the phone is now at `to`'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub at: f64,
}

/// What `data_dir/time.json` keeps: three small fields for the dates, the zones for the time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Person {
    /// The agents' zone now (empty: not known yet, the server's).
    #[serde(default)]
    pub time_zone: String,
    /// « Fuseau horaire automatique »: the phone's zone is followed.
    #[serde(default = "yes")]
    pub auto: bool,
    /// The zone the phone reported last.
    #[serde(default)]
    pub device: String,
    /// The zone the routines run at.
    #[serde(default)]
    pub routines: String,
    /// The routines' zone before they were last moved (« Revenir comme avant »).
    #[serde(default)]
    pub before: String,
    #[serde(default)]
    pub question: Option<Pending>,
    /// ISO 3166 code (empty: guessed from the zone).
    #[serde(default)]
    pub country: String,
    /// The `holidays` library's subdivision code (« 57 », « BY », « NY »…).
    #[serde(default)]
    pub region: String,
    /// « A », « B », « C » or « Corse » (France).
    #[serde(default)]
    pub school_zone: String,
}

impl Default for Person {
    fn default() -> Self {
        Person {
            time_zone: String::new(),
            auto: true,
            device: String::new(),
            routines: String::new(),
            before: String::new(),
            question: None,
            country: String::new(),
            region: String::new(),
            school_zone: String::new(),
        }
    }
}

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("time.json")
}

pub fn load(config: &Config) -> Person {
    let text = std::fs::read_to_string(path(config)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

fn save(config: &Config, person: &Person) -> std::io::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string_pretty(person).map_err(std::io::Error::other)?;
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

/// The person's settings changed under the lock, saved; the new ones.
pub fn change(config: &Config, edit: impl FnOnce(&mut Person)) -> std::io::Result<Person> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut person = load(config);
    edit(&mut person);
    save(config, &person)?;
    Ok(person)
}

pub fn parse_zone(name: &str) -> Option<Tz> {
    name.trim().parse::<Tz>().ok()
}

/// This machine's zone (what an agent got before the person's was known).
pub fn server_zone() -> String {
    static ZONE: LazyLock<String> = LazyLock::new(|| {
        if let Ok(tz) = std::env::var("TZ") {
            let tz = tz.trim_start_matches(':');
            if parse_zone(tz).is_some() {
                return tz.to_string();
            }
        }
        if let Ok(target) = std::fs::read_link("/etc/localtime") {
            let text = target.to_string_lossy();
            if let Some((_, name)) = text.split_once("zoneinfo/") {
                if parse_zone(name).is_some() {
                    return name.to_string();
                }
            }
        }
        if let Ok(text) = std::fs::read_to_string("/etc/timezone") {
            if parse_zone(text.trim()).is_some() {
                return text.trim().to_string();
            }
        }
        "UTC".to_string()
    });
    ZONE.clone()
}

impl Person {
    /// The agents' zone (the server's until the person's is known).
    pub fn zone_name(&self) -> String {
        if parse_zone(&self.time_zone).is_some() {
            self.time_zone.clone()
        } else {
            server_zone()
        }
    }

    pub fn zone(&self) -> Tz {
        parse_zone(&self.zone_name()).unwrap_or(Tz::UTC)
    }

    /// The routines' zone.
    pub fn routines_zone(&self) -> String {
        if parse_zone(&self.routines).is_some() {
            self.routines.clone()
        } else {
            self.zone_name()
        }
    }

    /// The country: the person's choice, else guessed from the zone.
    pub fn country_code(&self) -> Option<String> {
        if !self.country.is_empty() {
            return Some(self.country.clone());
        }
        guess_country(&self.zone_name()).map(str::to_string)
    }

    pub fn dates_for(&self) -> For {
        let country = self.country_code();
        For {
            zone: self.zone(),
            region: Some(self.region.clone()).filter(|r| !r.is_empty()),
            school_zone: Some(self.school_zone.clone()).filter(|z| !z.is_empty() && country.as_deref() == Some("FR")),
            country,
        }
    }

    /// The agents' zone set to `zone` (the phone's, or the person's pick). The routines follow when nothing runs at a
    /// set time (`has_routines` false) or on the first zone ever known; else they keep theirs and the person is asked.
    /// True when the agents' zone changed.
    pub fn set_zone(&mut self, zone: &str, has_routines: bool) -> bool {
        if self.time_zone == zone {
            return false;
        }
        let first = parse_zone(&self.time_zone).is_none() || parse_zone(&self.routines).is_none();
        self.time_zone = zone.to_string();
        if first || !has_routines {
            self.routines = zone.to_string();
            self.question = None;
        } else if self.routines == zone {
            self.question = None; // back where the routines are
        } else {
            self.question = Some(Pending { from: self.routines.clone(), to: zone.to_string(), at: py::time_time() });
        }
        true
    }

    /// The phone reported its zone.
    pub fn device_zone(&mut self, zone: &str, has_routines: bool) -> bool {
        self.device = zone.to_string();
        self.auto && self.set_zone(zone, has_routines)
    }

    /// The answer to the question: the routines moved to the new zone, or kept. False: no question waiting.
    pub fn answer(&mut self, move_them: bool) -> bool {
        let Some(question) = self.question.take() else { return false };
        if move_them {
            self.before = question.from.clone();
            self.routines = question.to;
        }
        true
    }
}

/// The country a zone is in, for the countries sold (a guess the person can change).
pub fn guess_country(zone: &str) -> Option<&'static str> {
    const EUROPE: [(&str, &str); 30] = [
        ("Vienna", "AT"),
        ("Brussels", "BE"),
        ("Sofia", "BG"),
        ("Zagreb", "HR"),
        ("Nicosia", "CY"),
        ("Prague", "CZ"),
        ("Copenhagen", "DK"),
        ("Tallinn", "EE"),
        ("Helsinki", "FI"),
        ("Paris", "FR"),
        ("Berlin", "DE"),
        ("Busingen", "DE"),
        ("Athens", "GR"),
        ("Budapest", "HU"),
        ("Dublin", "IE"),
        ("Rome", "IT"),
        ("Riga", "LV"),
        ("Vilnius", "LT"),
        ("Luxembourg", "LU"),
        ("Malta", "MT"),
        ("Amsterdam", "NL"),
        ("Warsaw", "PL"),
        ("Lisbon", "PT"),
        ("Bucharest", "RO"),
        ("Bratislava", "SK"),
        ("Ljubljana", "SI"),
        ("Madrid", "ES"),
        ("Stockholm", "SE"),
        ("London", "GB"),
        ("Belfast", "GB"),
    ];
    const OTHERS: [(&str, &str); 14] = [
        ("Atlantic/Canary", "ES"),
        ("Africa/Ceuta", "ES"),
        ("Atlantic/Madeira", "PT"),
        ("Atlantic/Azores", "PT"),
        ("Asia/Nicosia", "CY"),
        ("Asia/Famagusta", "CY"),
        ("Asia/Jerusalem", "IL"),
        ("Asia/Tel_Aviv", "IL"),
        ("Asia/Singapore", "SG"),
        ("Singapore", "SG"),
        ("Pacific/Honolulu", "US"),
        ("America/St_Johns", "CA"),
        ("Indian/Reunion", "FR"),
        ("Indian/Mayotte", "FR"),
    ];
    const CANADA: [&str; 16] = [
        "Toronto",
        "Vancouver",
        "Edmonton",
        "Winnipeg",
        "Halifax",
        "Regina",
        "Montreal",
        "Moncton",
        "Glace_Bay",
        "Goose_Bay",
        "Whitehorse",
        "Dawson_Creek",
        "Iqaluit",
        "Yellowknife",
        "Swift_Current",
        "Thunder_Bay",
    ];
    const FRANCE_OVERSEAS: [&str; 4] = ["Guadeloupe", "Martinique", "Cayenne", "Miquelon"];
    if let Some((_, code)) = OTHERS.iter().find(|(z, _)| *z == zone) {
        return Some(code);
    }
    if let Some(city) = zone.strip_prefix("Europe/") {
        return EUROPE.iter().find(|(c, _)| *c == city).map(|(_, code)| *code);
    }
    if let Some(city) = zone.strip_prefix("America/") {
        if CANADA.contains(&city) {
            return Some("CA");
        }
        if FRANCE_OVERSEAS.contains(&city) {
            return Some("FR");
        }
        const US: [&str; 10] = [
            "New_York",
            "Chicago",
            "Denver",
            "Los_Angeles",
            "Phoenix",
            "Anchorage",
            "Detroit",
            "Boise",
            "Juneau",
            "Adak",
        ];
        if US.contains(&city)
            || city.starts_with("Indiana/")
            || city.starts_with("Kentucky/")
            || city.starts_with("North_Dakota/")
        {
            return Some("US");
        }
    }
    if zone.starts_with("US/") {
        return Some("US");
    }
    if zone.starts_with("Canada/") {
        return Some("CA");
    }
    None
}

/// The French school zone and region of a postal code (« 75011 » → C; « 57000 » → B, Moselle; « 20000 » → Corse).
pub fn from_postal_code(code: &str) -> Option<(&'static str, Option<String>)> {
    let code = code.trim();
    if code.len() != 5 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if code.starts_with("97") {
        let region = &code[..3];
        return matches!(region, "971" | "972" | "973" | "974" | "976").then(|| ("", Some(region.to_string())));
    }
    let department = &code[..2];
    if department == "20" {
        return Some(("Corse", None));
    }
    const A: [&str; 32] = [
        "25", "39", "70", "90", "24", "33", "40", "47", "64", "03", "15", "43", "63", "21", "58", "71", "89", "07",
        "26", "38", "73", "74", "19", "23", "87", "01", "42", "69", "16", "17", "79", "86",
    ];
    const B: [&str; 41] = [
        "04", "05", "13", "84", "02", "60", "80", "59", "62", "54", "55", "57", "88", "44", "49", "53", "72", "85",
        "06", "83", "14", "27", "50", "61", "76", "18", "28", "36", "37", "41", "45", "08", "10", "51", "52", "22",
        "29", "35", "56", "67", "68",
    ];
    const C: [&str; 21] = [
        "77", "93", "94", "11", "30", "34", "48", "66", "75", "09", "12", "31", "32", "46", "65", "81", "82", "78",
        "91", "92", "95",
    ];
    let zone = if A.contains(&department) {
        "A"
    } else if B.contains(&department) {
        "B"
    } else if C.contains(&department) {
        "C"
    } else {
        return None;
    };
    let region = match department {
        "57" => Some("57".to_string()),
        "67" | "68" => Some("6AE".to_string()),
        _ => None,
    };
    Some((zone, region))
}

// -- the agents' Hermes ---------------------------------------------------------------------------------------------

fn config_path(config: &Config, agent: &AgentConfig) -> PathBuf {
    crate::agents_admin::profile_folder(config, agent).join("config.yaml")
}

/// `timezone` and `cron.timezone` in a profile's config: true when it was rewritten.
pub fn write_profile(path: &Path, zone: &str, routines: &str) -> anyhow::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut data = crate::agents_admin::load_or_empty(path)?;
    let mut changed = false;
    if data.get("timezone").and_then(Yaml::as_str) != Some(zone) {
        data.insert("timezone", Yaml::str(zone));
        changed = true;
    }
    if !data.get("cron").is_some_and(Yaml::is_map) {
        data.insert("cron", Yaml::map());
    }
    if let Some(cron) = data.get_mut("cron") {
        if cron.get("timezone").and_then(Yaml::as_str) != Some(routines) {
            cron.insert("timezone", Yaml::str(routines));
            changed = true;
        }
    }
    if changed {
        crate::agents_admin::replace_yaml(path, &data)?;
    }
    Ok(changed)
}

/// Every agent's profile given the person's zones; the agents whose profile changed. Nothing before the person's zone
/// is known (the agents keep the server's, as before).
pub fn write_profiles(config: &Config, person: &Person) -> Vec<AgentConfig> {
    if parse_zone(&person.time_zone).is_none() {
        return Vec::new();
    }
    let (zone, routines) = (person.zone_name(), person.routines_zone());
    let mut changed = Vec::new();
    for agent in config.agents.values() {
        match write_profile(&config_path(config, agent), &zone, &routines) {
            Ok(true) => changed.push(agent.clone()),
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(target: "bipbridge::dates", agent = %agent.name, error = %err, "time zone not written")
            }
        }
    }
    if !changed.is_empty() {
        let names: Vec<&str> = changed.iter().map(|a| a.name.as_str()).collect();
        tracing::info!(target: "bipbridge::dates", zone = %zone, routines = %routines, agents = %names.join(","), "agents' time zone set");
    }
    changed
}

/// Whether this Hermes reads a rewritten zone without a restart and knows `cron.timezone` (patch 0008).
pub fn hermes_follows(config: &Config, agent: &AgentConfig) -> bool {
    let root = agent.hermes_root.as_deref().filter(|r| !r.is_empty()).unwrap_or(&config.hermes.root);
    if root.is_empty() {
        return false;
    }
    std::fs::read_to_string(Path::new(root).join("hermes_time.py")).is_ok_and(|t| t.contains("def get_cron_timezone"))
}

/// The gateways of the agents whose zone changed restarted when their Hermes would not read it otherwise (each
/// gateway once: the hub serves several agents).
pub async fn restart_where_needed(config: &Config, changed: &[AgentConfig]) {
    if !config.hermes.enabled() {
        return;
    }
    let run = crate::agents_admin::default_run();
    let mut done: BTreeSet<String> = BTreeSet::new();
    for agent in changed {
        if hermes_follows(config, agent) {
            continue;
        }
        let home = crate::agents_admin::home_root(config, agent);
        if !done.insert(home) {
            continue;
        }
        if let Err(err) = crate::agents_admin::restart_gateway(config, agent, &run).await {
            tracing::warn!(target: "bipbridge::dates", agent = %agent.name, error = %err, "gateway not restarted for the time zone");
        }
    }
}

// -- routines ---------------------------------------------------------------------------------------------------------

/// A routine of an agent that runs at set times: its name, and the time of day when it is a daily-like one.
#[derive(Debug, Clone, PartialEq)]
pub struct Routine {
    pub agent: String,
    pub name: String,
    pub at: Option<NaiveTime>,
}

/// « 30 7 * * 1-5 » → 7:30; anything else (every hour, several times) → None.
pub fn time_of(expr: &str) -> Option<NaiveTime> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() < 5 {
        return None;
    }
    let minute: u32 = fields[0].parse().ok()?;
    let hour: u32 = fields[1].parse().ok()?;
    NaiveTime::from_hms_opt(hour, minute, 0)
}

/// The routines (enabled cron jobs) of every agent: their `cron/jobs.json` in each profile.
pub fn routines(config: &Config) -> Vec<Routine> {
    let mut out = Vec::new();
    for agent in config.agents.values() {
        let file = crate::agents_admin::profile_folder(config, agent).join("cron").join("jobs.json");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let Ok(data) = serde_json::from_str::<Value>(&text) else { continue };
        let jobs = match &data {
            Value::Array(items) => items.clone(),
            Value::Object(map) => map.get("jobs").and_then(Value::as_array).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        for job in jobs {
            let schedule = job.get("schedule").cloned().unwrap_or(Value::Null);
            if schedule.get("kind").and_then(Value::as_str) != Some("cron") || job.get("enabled") == Some(&json!(false))
            {
                continue;
            }
            let state = job.get("state").and_then(Value::as_str).unwrap_or("scheduled");
            if matches!(state, "paused" | "completed" | "deleted") {
                continue;
            }
            let name = job.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let at = schedule.get("expr").and_then(Value::as_str).and_then(time_of);
            out.push(Routine {
                agent: agent.name.clone(),
                name: if name.is_empty() { "Routine".into() } else { name },
                at,
            });
        }
    }
    out
}

fn offset_now(zone: Tz) -> i32 {
    zone.offset_from_utc_datetime(&Utc::now().naive_utc()).fix().local_minus_utc()
}

/// The question as the app shows it, in `lang`.
pub fn question_view(config: &Config, person: &Person, lang: &str) -> Value {
    let Some(q) = &person.question else { return Value::Null };
    let (Some(from), Some(to)) = (parse_zone(&q.from), parse_zone(&q.to)) else { return Value::Null };
    let all = routines(config);
    let mut named: Vec<String> =
        all.iter().take(NAMED_ROUTINES).map(|r| words::routine_at(&r.name, r.at, lang)).collect();
    if all.len() > NAMED_ROUTINES {
        named.push("…".into());
    }
    let difference = words::offset_difference(offset_now(to) - offset_now(from), lang);
    let back = !person.before.is_empty() && person.before == q.to;
    let words::Question { text, move_label, keep_label } =
        words::question(&q.from, &q.to, difference, &named, back, lang);
    let listed: Vec<Value> = all
        .iter()
        .map(|r| {
            json!({"agent": r.agent, "name": r.name,
                   "time": r.at.map(|t| t.format("%H:%M").to_string()),
                   "there": r.at.and_then(|t| standard::same_instant(t, from, to, Utc::now().with_timezone(&from).date_naive()))
                       .map(|t| t.format("%H:%M").to_string())})
        })
        .collect();
    json!({"from": q.from, "to": q.to, "text": text, "move": move_label, "keep": keep_label, "back": back,
           "routines": listed, "asked_at": q.at})
}

// -- the view ---------------------------------------------------------------------------------------------------------

/// The person's language for these texts: the app's (`?lang=`), else the Carnet's.
fn language(config: &Config, q: Option<&QueryMap>) -> &'static str {
    match q.and_then(|q| q.get("lang")).filter(|l| !l.is_empty()) {
        Some(lang) => words::lang(lang),
        None => words::lang(&crate::notebook::language(config)),
    }
}

pub fn view(config: &Config, person: &Person, lang: &str) -> Value {
    let zone = person.zone();
    let now = Utc::now().with_timezone(&zone);
    let country = person.country_code();
    let countries: Vec<Value> = words::COUNTRIES
        .iter()
        .map(|(code, _)| json!({"code": code, "name": words::country_name(code, lang)}))
        .collect();
    let regions: Vec<Value> = country
        .as_deref()
        .and_then(|c| standard::HOLIDAYS.countries.get(c))
        .map(|c| c.subdivisions.iter().map(|(code, name)| json!({"code": code, "name": name})).collect())
        .unwrap_or_default();
    let school_zones: Vec<&str> =
        if country.as_deref() == Some("FR") { words::SCHOOL_ZONES.to_vec() } else { Vec::new() };
    json!({
        "time_zone": person.zone_name(),
        "known": parse_zone(&person.time_zone).is_some(),
        "auto": person.auto,
        "device": person.device,
        "routines_zone": person.routines_zone(),
        "now": now.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "question": question_view(config, person, lang),
        "country": country,
        "country_guessed": person.country.is_empty() && country.is_some(),
        "region": person.region,
        "school_zone": person.school_zone,
        "countries": countries,
        "regions": regions,
        "school_zones": school_zones,
    })
}

/// The standard dates of the person from today, for `days` days.
pub fn upcoming(person: &Person, days: i64, lang: &str) -> (NaiveDate, Vec<Item>) {
    let today = Utc::now().with_timezone(&person.zone()).date_naive();
    (today, standard::upcoming(&person.dates_for(), today, days.clamp(1, MAX_DAYS), lang))
}

/// The same as the Carnet's « Dates à venir » shows them: computed entries (`computed: true`), never saved.
pub fn notebook_entries(config: &Config, days: i64) -> Vec<Value> {
    let person = load(config);
    let lang = words::lang(&crate::notebook::language(config));
    let (today, items) = upcoming(&person, days, lang);
    items
        .iter()
        .map(|item| {
            let mut entry = item.view(today, lang);
            if let Some(map) = entry.as_object_mut() {
                map.insert("computed".into(), json!(true));
                map.insert("fact".into(), json!(item.text));
                map.insert("yearly".into(), json!(false));
            }
            entry
        })
        .collect()
}

// -- the agents' context line ---------------------------------------------------------------------------------------

/// The personality without the dates line (the person never sees it), and whether it had one.
pub fn split_dates(soul: &str) -> (String, bool) {
    let (Some(start), Some(end)) = (soul.find(DATES_START), soul.find(DATES_END)) else {
        return (soul.to_string(), false);
    };
    if end < start {
        return (soul.to_string(), false);
    }
    (format!("{}{}", soul[..start].trim_end(), &soul[end + DATES_END.len()..]), true)
}

/// The personality with the dates line (after the rest), without any when `line` is None.
pub fn with_dates(soul: &str, line: Option<&str>) -> String {
    let (body, _) = split_dates(soul);
    match line {
        None => body,
        Some(line) => format!("{}\n\n{DATES_START}\n{line}\n{DATES_END}\n", body.trim_end()),
    }
}

/// The line for an agent writing in `lang` (None: nothing within 7 days).
pub fn line_for(person: &Person, lang: &str) -> Option<String> {
    let today = Utc::now().with_timezone(&person.zone()).date_naive();
    standard::context_line(&person.dates_for(), today, words::lang(lang))
}

/// The line of the agent (its language: its personality's language block's, else the Carnet's).
pub fn agent_line(config: &Config, person: &Person, soul: &str) -> Option<String> {
    let lang = crate::souls::split_language(soul).1.unwrap_or_else(|| crate::notebook::language(config));
    line_for(person, &lang)
}

/// Each agent's personality carries today's line, or none (agents set up by hand are left as they are).
pub fn refresh_souls(config: &Config) {
    let person = load(config);
    for agent in config.agents.values() {
        if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
            continue;
        }
        let file = crate::agents_admin::profile_folder(config, agent).join("SOUL.md");
        let Ok(soul) = std::fs::read_to_string(&file) else { continue };
        let changed = with_dates(&soul, agent_line(config, &person, &soul).as_deref());
        if changed != soul {
            if let Err(err) = std::fs::write(&file, changed) {
                tracing::warn!(target: "bipbridge::dates", agent = %agent.name, error = %err, "dates line not written");
            }
        }
    }
}

/// At start, then each hour: the agents' profiles carry the person's zones (an agent created since gets them) and
/// their personality today's line.
pub async fn run_forever(state: AppState) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    loop {
        let config = state.config();
        let c = config.clone();
        let changed = tokio::task::spawn_blocking(move || {
            refresh_souls(&c);
            write_profiles(&c, &load(&c))
        })
        .await
        .unwrap_or_default();
        restart_where_needed(&config, &changed).await;
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

// -- the agents' tool -----------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Your person's standard dates coming up, computed for them: clock changes (daylight saving time) \
    in their time zone, public holidays of their country and region, and French school holidays of their zone. Also \
    their time zone and the date and time where they are. Use it before planning around a date (a trip, a delivery, \
    a routine at night), or when they ask « c'est férié lundi ? », « quand sont les vacances ? ».",
        "inputSchema": {
            "type": "object",
            "properties": {
                "days": {"type": "integer", "minimum": 1, "maximum": MAX_DAYS,
                         "description": "how many days ahead, today included (default 30)"},
            },
        },
    })
}

/// The tool's answer for `agent` (its language for the texts).
pub fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    let days = arguments.get("days").and_then(Value::as_i64).unwrap_or(30).clamp(1, MAX_DAYS);
    let person = load(config);
    let soul = crate::agents_admin::read_soul(config, agent).unwrap_or_default();
    let lang = words::lang(&crate::souls::split_language(&soul).1.unwrap_or_else(|| crate::notebook::language(config)));
    let zone = person.zone();
    let now = Utc::now().with_timezone(&zone);
    let (today, items) = upcoming(&person, days, lang);
    let mut lines = vec![format!(
        "Time zone: {} (now {}, UTC{}).",
        person.zone_name(),
        now.format("%A %Y-%m-%d %H:%M"),
        now.format("%:z")
    )];
    let who = person.dates_for();
    let mut place: Vec<String> = Vec::new();
    match who.country.as_deref() {
        Some(code) => {
            let name = words::country_name(code, "en").unwrap_or(code);
            place.push(format!(
                "Country: {name}{}",
                if person.country.is_empty() { " (guessed from the time zone)" } else { "" }
            ));
        }
        None => place.push("Country: not set (the person picks it in the app's Settings)".into()),
    }
    if let Some(region) = &who.region {
        let shown = who
            .country
            .as_deref()
            .and_then(|c| standard::HOLIDAYS.countries.get(c))
            .and_then(|c| c.subdivisions.get(region))
            .cloned()
            .unwrap_or_else(|| region.clone());
        place.push(format!("region: {shown}"));
    }
    if let Some(zone) = &who.school_zone {
        place.push(format!("French school zone: {zone}"));
    }
    lines.push(format!("{}.", place.join(", ")));
    if items.is_empty() {
        lines.push(format!("No standard date in the next {days} days."));
    } else {
        for item in &items {
            let until = (item.date - today).num_days();
            lines.push(format!(
                "- {} ({}): {}",
                item.date.format("%Y-%m-%d"),
                words::day(item.date, lang),
                if until == 0 { format!("today, {}", item.text) } else { item.text.clone() }
            ));
        }
    }
    crate::tools_knowledge::tool_text(&lines.join("\n"), false)
}

// -- the routes -------------------------------------------------------------------------------------------------------

/// Whether any agent has a routine at set times (the question is only worth asking then).
fn has_routines(config: &Config) -> bool {
    !routines(config).is_empty()
}

/// After a change: the agents' profiles rewritten now, their gateways restarted in the background when needed, their
/// dates line refreshed (a new country brings other holidays).
fn applied(state: &AppState, config: &Config, person: &Person) {
    let changed = write_profiles(config, person);
    refresh_souls(config);
    if !changed.is_empty() {
        let config = config.clone();
        state.spawn(async move { restart_where_needed(&config, &changed).await });
    }
}

async fn blocking<T: Send + 'static>(work: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> ApiResult<T> {
    Ok(tokio::task::spawn_blocking(work).await.map_err(anyhow::Error::from)??)
}

async fn get_time(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let lang = language(&config, Some(&q));
    let out = blocking(move || Ok(view(&config, &load(&config), lang))).await?;
    Ok(web::ok(out))
}

fn zone_error(field: &str, value: &str) -> FieldError {
    FieldError::new("value_error", vec![json!("body"), json!(field)], "Value error, unknown time zone", json!(value))
}

#[derive(Debug, Deserialize)]
pub struct Device {
    pub time_zone: String,
}

impl Validate for Device {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "time_zone", Some(&self.time_zone), 64);
        if parse_zone(&self.time_zone).is_none() {
            errors.push(zone_error("time_zone", &self.time_zone));
        }
    }
}

async fn put_device(State(state): State<AppState>, q: QueryMap, Json(body): Json<Device>) -> ApiResult<Response> {
    let config = state.config();
    let lang = language(&config, Some(&q));
    let (c, s) = (config.clone(), state.clone());
    let out = blocking(move || {
        let routines = has_routines(&c);
        let zone = body.time_zone.trim().to_string();
        let mut moved = false;
        let person = change(&c, |p| moved = p.device_zone(&zone, routines))?;
        if moved {
            tracing::info!(target: "bipbridge::dates", zone = %zone, asked = person.question.is_some(), "the phone's time zone followed");
            applied(&s, &c, &person);
        }
        Ok(view(&c, &person, lang))
    })
    .await?;
    Ok(web::ok(out))
}

#[derive(Debug, Deserialize)]
pub struct Settings {
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub auto: Option<bool>,
    #[serde(default)]
    pub time_zone: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub school_zone: Option<String>,
    #[serde(default)]
    pub postal_code: Option<String>,
}

impl Validate for Settings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        for (field, value) in [
            ("time_zone", &self.time_zone),
            ("country", &self.country),
            ("region", &self.region),
            ("school_zone", &self.school_zone),
            ("postal_code", &self.postal_code),
        ] {
            check_max_len(errors, field, value.as_deref(), 64);
        }
        if let Some(zone) = self.time_zone.as_deref().filter(|z| parse_zone(z).is_none()) {
            errors.push(zone_error("time_zone", zone));
        }
        if let Some(code) = self.country.as_deref().filter(|c| !c.is_empty() && words::country_name(c, "en").is_none())
        {
            errors.push(FieldError::new(
                "value_error",
                vec![json!("body"), json!("country")],
                "Value error, not a country BipAgents is offered in",
                json!(code),
            ));
        }
        if let Some(zone) = self.school_zone.as_deref().filter(|z| !z.is_empty() && !words::SCHOOL_ZONES.contains(z)) {
            errors.push(FieldError::new(
                "literal_error",
                vec![json!("body"), json!("school_zone")],
                "Input should be 'A', 'B', 'C', 'Corse' or ''",
                json!(zone),
            ));
        }
    }
}

async fn patch_time(State(state): State<AppState>, q: QueryMap, Json(body): Json<Settings>) -> ApiResult<Response> {
    let config = state.config();
    let lang = language(&config, Some(&q));
    let (c, s) = (config.clone(), state.clone());
    let out = blocking(move || {
        let routines = has_routines(&c);
        let before = load(&c);
        let region_valid = |country: &str, region: &str| {
            region.is_empty()
                || standard::HOLIDAYS.countries.get(country).is_some_and(|c| c.subdivisions.contains_key(region))
        };
        let mut bad_region = false;
        let person = change(&c, |p| {
            if let Some(country) = &body.country {
                if *country != p.country {
                    p.region.clear(); // another country's region means nothing here
                }
                p.country = country.clone();
            }
            if let Some(code) = body.postal_code.as_deref().filter(|c| !c.trim().is_empty()) {
                if let Some((zone, region)) = from_postal_code(code) {
                    if p.country.is_empty() {
                        p.country = "FR".into();
                    }
                    if !zone.is_empty() {
                        p.school_zone = zone.into();
                    }
                    p.region = region.unwrap_or_default();
                }
            }
            if let Some(region) = &body.region {
                let country = p.country_code().unwrap_or_default();
                if region_valid(&country, region) {
                    p.region = region.clone();
                } else {
                    bad_region = true;
                }
            }
            if let Some(zone) = &body.school_zone {
                p.school_zone = zone.clone();
            }
            if let Some(auto) = body.auto {
                p.auto = auto;
                if auto && parse_zone(&p.device).is_some() {
                    let device = p.device.clone();
                    p.set_zone(&device, routines);
                }
            }
            if let Some(zone) = body.time_zone.as_deref().map(str::trim) {
                if !p.auto {
                    p.set_zone(zone, routines);
                }
            }
        })?;
        if person != before {
            applied(&s, &c, &person);
        }
        if bad_region {
            return Ok(Err(HttpError::new(422, "unknown region for this country")));
        }
        Ok(Ok(view(&c, &person, lang)))
    })
    .await??;
    Ok(web::ok(out))
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    pub choice: String,
}

impl Validate for Answer {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !matches!(self.choice.as_str(), "move" | "keep") {
            errors.push(FieldError::new(
                "literal_error",
                vec![json!("body"), json!("choice")],
                "Input should be 'move' or 'keep'",
                json!(self.choice),
            ));
        }
    }
}

async fn answer_routines(State(state): State<AppState>, q: QueryMap, Json(body): Json<Answer>) -> ApiResult<Response> {
    let config = state.config();
    let lang = language(&config, Some(&q));
    let (c, s) = (config.clone(), state.clone());
    let out = blocking(move || {
        let mut answered = false;
        let person = change(&c, |p| answered = p.answer(body.choice == "move"))?;
        if !answered {
            return Ok(Err(HttpError::new(404, "no question waiting")));
        }
        tracing::info!(target: "bipbridge::dates", choice = %body.choice, routines = %person.routines_zone(), "routines' time zone answered");
        applied(&s, &c, &person);
        Ok(Ok(view(&c, &person, lang)))
    })
    .await??;
    Ok(web::ok(out))
}

async fn get_dates(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let lang = language(&config, Some(&q));
    let days = q.get("days").and_then(|d| d.parse::<i64>().ok()).unwrap_or(60).clamp(1, MAX_DAYS);
    let out = blocking(move || {
        let person = load(&config);
        let (today, items) = upcoming(&person, days, lang);
        let dates: Vec<Value> = items.iter().map(|i| i.view(today, lang)).collect();
        Ok(json!({"today": today.format("%Y-%m-%d").to_string(), "time_zone": person.zone_name(), "dates": dates}))
    })
    .await?;
    Ok(web::ok(out))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/time", get(get_time).patch(patch_time))
        .route("/time/device", put(put_device))
        .route("/time/routines", post(answer_routines))
        .route("/dates", get(get_dates))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phone_moves_the_agents_and_asks_for_the_routines() {
        let mut p = Person::default();
        // first zone ever: everything follows, nothing to ask
        assert!(p.device_zone("Europe/Paris", true));
        assert_eq!((p.time_zone.as_str(), p.routines.as_str()), ("Europe/Paris", "Europe/Paris"));
        assert!(p.question.is_none());
        // to Tokyo: the agents follow at once, the routines wait for the answer
        assert!(p.device_zone("Asia/Tokyo", true));
        assert_eq!(p.time_zone, "Asia/Tokyo");
        assert_eq!(p.routines_zone(), "Europe/Paris");
        assert_eq!(p.question.as_ref().map(|q| (q.from.as_str(), q.to.as_str())), Some(("Europe/Paris", "Asia/Tokyo")));
        // kept: remembered until the zone changes again
        assert!(p.answer(false));
        assert!(!p.answer(false));
        assert!(!p.device_zone("Asia/Tokyo", true));
        assert!(p.question.is_none());
        // back home: the routines are already there, nothing to ask
        assert!(p.device_zone("Europe/Paris", true));
        assert!(p.question.is_none());
        // to New York, moved; then home again: asked, « Revenir comme avant »
        p.device_zone("America/New_York", true);
        assert!(p.answer(true));
        assert_eq!(p.routines, "America/New_York");
        assert_eq!(p.before, "Europe/Paris");
        p.device_zone("Europe/Paris", true);
        assert_eq!(p.question.as_ref().map(|q| q.to.as_str()), Some("Europe/Paris"));
    }

    #[test]
    fn without_routines_or_with_the_switch_off() {
        let mut p = Person::default();
        p.device_zone("Europe/Paris", false);
        p.device_zone("Asia/Tokyo", false);
        assert_eq!(p.routines, "Asia/Tokyo");
        assert!(p.question.is_none());
        p.auto = false;
        assert!(!p.device_zone("America/Chicago", true));
        assert_eq!(p.time_zone, "Asia/Tokyo");
        assert_eq!(p.device, "America/Chicago");
    }

    #[test]
    fn countries_guessed_and_postal_codes() {
        assert_eq!(guess_country("Europe/Paris"), Some("FR"));
        assert_eq!(guess_country("America/Los_Angeles"), Some("US"));
        assert_eq!(guess_country("America/Indiana/Indianapolis"), Some("US"));
        assert_eq!(guess_country("America/Toronto"), Some("CA"));
        assert_eq!(guess_country("Europe/London"), Some("GB"));
        assert_eq!(guess_country("Asia/Jerusalem"), Some("IL"));
        assert_eq!(guess_country("Asia/Tokyo"), None);
        assert_eq!(from_postal_code("75011"), Some(("C", None)));
        assert_eq!(from_postal_code("69003"), Some(("A", None)));
        assert_eq!(from_postal_code("57000"), Some(("B", Some("57".into()))));
        assert_eq!(from_postal_code("67000"), Some(("B", Some("6AE".into()))));
        assert_eq!(from_postal_code("20000"), Some(("Corse", None)));
        assert_eq!(from_postal_code("97400"), Some(("", Some("974".into()))));
        assert_eq!(from_postal_code("1234"), None);
    }

    #[test]
    fn routine_times() {
        assert_eq!(time_of("0 7 * * *"), NaiveTime::from_hms_opt(7, 0, 0));
        assert_eq!(time_of("30 21 * * 1-5"), NaiveTime::from_hms_opt(21, 30, 0));
        assert_eq!(time_of("*/15 * * * *"), None);
        assert_eq!(time_of("0 7,19 * * *"), None);
    }

    #[test]
    fn the_profile_gets_both_zones_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.yaml");
        std::fs::write(&file, "model:\n  default: m\ncron:\n  catch_up_missed: true\n").unwrap();
        assert!(write_profile(&file, "Asia/Tokyo", "Europe/Paris").unwrap());
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("timezone: Asia/Tokyo"), "{text}");
        assert!(text.contains("catch_up_missed: true") && text.contains("  timezone: Europe/Paris"), "{text}");
        assert!(!write_profile(&file, "Asia/Tokyo", "Europe/Paris").unwrap());
        assert!(!write_profile(&dir.path().join("none.yaml"), "Asia/Tokyo", "Asia/Tokyo").unwrap());
    }

    #[test]
    fn the_dates_line_comes_and_goes() {
        let soul = "Tu es Bip.\n\n<!-- bipagents:language -->\nLanguage…\n<!-- /bipagents:language -->\n";
        let with = with_dates(soul, Some("Dimanche 25 octobre : passage à l'heure d'hiver"));
        assert!(with.ends_with(
            "<!-- bipagents:dates -->\nDimanche 25 octobre : passage à l'heure d'hiver\n<!-- /bipagents:dates -->\n"
        ));
        assert_eq!(split_dates(&with), (soul.trim_end().to_string() + "\n", true));
        assert_eq!(with_dates(&with, None), soul.trim_end().to_string() + "\n");
        assert_eq!(crate::souls::split_language(&with).1.as_deref(), Some("en"));
    }
}
