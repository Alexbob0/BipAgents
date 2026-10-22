//! The `weather` tool (decided 2026-10-22): the forecast for a place — the conditions now, the next 24 hours hour by
//! hour, the next days, and heads-ups (heavy rain, heat, thunder, strong wind, snow) — for « do I need an umbrella this
//! morning? » in a reply or a routine. Switched per agent (« Météo » in the app, on at creation for the everyday and
//! home agents): `PATCH /v1/agents/{id}/settings {"weather": bool}` (agents_admin.rs).
//!
//! Providers, both free and without a key, both reached from this place's server with nothing but the place:
//!
//! - **MET Norway's Locationforecast 2.0** (the Norwegian Meteorological Institute; global, ECMWF-based outside the
//!   Nordics). Free, commercial use allowed under CC BY 4.0 / NLOD with attribution. Its terms: a User-Agent naming
//!   the application and a contact (`USER_AGENT`), at most 4 decimals in the coordinates (we send 2: about 1 km, which
//!   also lets nearby requests share the cache), honour `Expires` (no new request before it) and send
//!   `If-Modified-Since` after it, slow down on 429. Open-Meteo is free for non-commercial use only; Météo-France's
//!   API needs an account and a token per application on its portal, so MET Norway is used everywhere.
//! - **OpenStreetMap's Nominatim** for place names. Its usage policy: at most 1 request per second (one at a time
//!   here, `GEOCODER_GAP` apart), a User-Agent identifying the application, results cached (`PLACES`), no bulk or
//!   autocomplete use, « © OpenStreetMap contributors » (ODbL). Each place's server makes its own few requests; an
//!   install that outgrows the public service points `[weather] geocoder_url` at its own or a paid Nominatim.
//!   Coordinates (« 48.85,2.35 ») skip the geocoder.
//!
//! MET gives UTC times; the place's local time comes from its country (`utc_offset`: the European, North American and
//! Israeli summer-time rules), else the times stay in UTC and the tool says so.
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Offset, Timelike, Utc, Weekday};
use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "weather";
/// The days the tool gives at most (MET's forecast goes about 9 days ahead).
pub const MAX_DAYS: i64 = 9;
pub const DEFAULT_DAYS: i64 = 4;
/// Between two requests to the geocoder (Nominatim: at most 1 per second).
pub const GEOCODER_GAP: StdDuration = StdDuration::from_millis(1100);
/// Places kept, and for how long (names rarely move).
const PLACES_KEPT: usize = 500;
const PLACE_DAYS: u64 = 30;
/// Forecasts kept (one per place asked recently).
const FORECASTS_KEPT: usize = 200;
pub const ATTRIBUTION: &str = "Source: MET Norway (Norwegian Meteorological Institute), CC BY 4.0.";
pub const OSM_ATTRIBUTION: &str = "Place search © OpenStreetMap contributors.";

/// What MET Norway and Nominatim are told about the caller: the application and where to reach its publisher.
pub fn user_agent() -> String {
    format!("BipAgents/{} https://bipagents.com", crate::VERSION)
}

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(user_agent())
        .connect_timeout(StdDuration::from_secs(10))
        .timeout(StdDuration::from_secs(20))
        .build()
        .unwrap_or_default()
});

/// The client for the tools' outside services (weather, geocoding, routes), with `user_agent()`.
pub fn http() -> &'static reqwest::Client {
    &HTTP
}

// --- the switch ---------------------------------------------------------------------------------------------------

static LOCK: Mutex<()> = Mutex::new(());

fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("weather")
}

/// The agents with « Météo » on.
pub fn agents(config: &Config) -> Vec<String> {
    let text = tk::read_text(&root(config).join("agents.json")).unwrap_or_default();
    match serde_json::from_str::<Value>(&text).ok().as_ref().and_then(|v| v.get("agents")) {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

/// Whether `agent` has the `weather` tool.
pub fn enabled(config: &Config, agent: &str) -> bool {
    agents(config).iter().any(|a| a == agent)
}

/// The switch of `agent`. Returns whether it changed.
pub fn set_enabled(config: &Config, agent: &str, on: bool) -> std::io::Result<bool> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut names = agents(config);
    let before = names.clone();
    names.retain(|a| a != agent);
    if on {
        names.push(agent.to_string());
    }
    if names == before {
        return Ok(false);
    }
    let path = root(config).join("agents.json");
    std::fs::create_dir_all(root(config))?;
    let temporary = path.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), py::dumps(&json!({"agents": names})).as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &path)?;
    Ok(true)
}

/// A deleted agent: off.
pub fn forget(config: &Config, agent: &str) {
    if enabled(config, agent) {
        if let Err(err) = set_enabled(config, agent, false) {
            tracing::warn!(agent = %agent, error = %err, "weather switch not removed");
        }
    }
}

/// The tool's definition (listed only for an agent whose switch is on).
pub fn weather_tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "The weather forecast for a place: the conditions now, the next 24 hours hour by hour, the next \
    days, and heads-ups (heavy rain, heat, thunderstorms, strong wind, snow). Metric units (°C, km/h, mm). When the \
    person doesn't name a place, use their home town if you know it from your memory; else ask them. The heads-ups \
    come from the forecast, they are not official warnings. Mention the source (MET Norway) when you show a forecast \
    in a page.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "place": {"type": "string", "maxLength": 200,
                          "description": "a town, an address or « latitude,longitude », e.g. « Lyon, France »"},
                "days": {"type": "integer", "minimum": 1, "maximum": MAX_DAYS,
                         "description": "how many days, today included (default 4)"},
            },
            "required": ["place"],
        },
    })
}

// --- places --------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// ISO 3166-1 alpha-2, lower case; empty when unknown (coordinates given).
    pub country: String,
}

/// A name looked up: the place found (or none), and when.
type Looked = (Option<Place>, Instant);

static PLACES: LazyLock<Mutex<IndexMap<String, Looked>>> = LazyLock::new(|| Mutex::new(IndexMap::new()));
/// The last request to the geocoder: one at a time, `GEOCODER_GAP` apart.
static GEOCODER_TURN: LazyLock<tokio::sync::Mutex<Option<Instant>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

/// « 48.8566, 2.3522 » → (48.8566, 2.3522).
pub fn parse_coordinates(text: &str) -> Option<(f64, f64)> {
    let (lat, lon) = text.split_once(',')?;
    let lat: f64 = lat.trim().parse().ok()?;
    let lon: f64 = lon.trim().parse().ok()?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)).then_some((lat, lon))
}

fn num(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The first result of a Nominatim search (`format=jsonv2&addressdetails=1`).
pub fn place_of(answer: &Value) -> Option<Place> {
    let first = answer.as_array()?.first()?;
    let lat = num(first.get("lat"))?;
    let lon = num(first.get("lon"))?;
    let address = first.get("address");
    let field = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").trim().to_string();
    let country = field(address.and_then(|a| a.get("country_code"))).to_lowercase();
    let display = field(first.get("display_name"));
    let mut name = field(first.get("name"));
    if name.is_empty() {
        name = display.split(',').next().unwrap_or("").trim().to_string();
    }
    let region = field(address.and_then(|a| a.get("state")));
    let land = field(address.and_then(|a| a.get("country")));
    let mut parts = vec![name];
    for part in [region, land] {
        if !part.is_empty() && !parts.contains(&part) {
            parts.push(part);
        }
    }
    Some(Place { name: parts.join(", "), lat, lon, country })
}

/// The place a name stands for, from the cache or the geocoder. Ok(None): nothing found. Err: the geocoder failed.
pub async fn geocode(config: &Config, query: &str) -> Result<Option<Place>, String> {
    let key = query.trim().to_lowercase();
    {
        let places = PLACES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((place, at)) = places.get(&key) {
            if at.elapsed() < StdDuration::from_secs(PLACE_DAYS * 86400) {
                return Ok(place.clone());
            }
        }
    }
    let mut turn = GEOCODER_TURN.lock().await;
    if let Some(last) = *turn {
        let since = last.elapsed();
        if since < GEOCODER_GAP {
            tokio::time::sleep(GEOCODER_GAP - since).await;
        }
    }
    let answer = HTTP
        .get(&config.weather.geocoder_url)
        .query(&[("q", query.trim()), ("format", "jsonv2"), ("limit", "1"), ("addressdetails", "1")])
        .send()
        .await;
    *turn = Some(Instant::now());
    drop(turn);
    let response = answer.map_err(|e| format!("the place search did not answer ({})", short_error(&e)))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("the place search answered {status}"));
    }
    let answer: Value =
        response.json().await.map_err(|_| "the place search answered something unreadable".to_string())?;
    let place = place_of(&answer);
    let mut places = PLACES.lock().unwrap_or_else(|e| e.into_inner());
    places.shift_remove(&key);
    places.insert(key, (place.clone(), Instant::now()));
    while places.len() > PLACES_KEPT {
        places.shift_remove_index(0);
    }
    Ok(place)
}

fn short_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timed out"
    } else if err.is_connect() {
        "unreachable"
    } else {
        "network error"
    }
}

// --- the forecast --------------------------------------------------------------------------------------------------

#[derive(Clone)]
struct Cached {
    body: Value,
    last_modified: Option<String>,
    /// MET's `Expires`: no new request before it.
    expires: Option<DateTime<Utc>>,
}

static FORECASTS: LazyLock<Mutex<IndexMap<String, Cached>>> = LazyLock::new(|| Mutex::new(IndexMap::new()));

fn http_date(value: Option<&reqwest::header::HeaderValue>) -> Option<DateTime<Utc>> {
    let text = value?.to_str().ok()?;
    DateTime::parse_from_rfc2822(text).ok().map(|d| d.with_timezone(&Utc))
}

/// MET's forecast for (lat, lon), rounded to 2 decimals: from the cache until it expires, then asked again with
/// `If-Modified-Since`. Err: no forecast at all (the service failed and nothing is kept). The bool: a kept forecast
/// given because the service failed.
pub async fn forecast(config: &Config, lat: f64, lon: f64, now: DateTime<Utc>) -> Result<(Value, bool), String> {
    let (lat, lon) = (format!("{lat:.2}"), format!("{lon:.2}"));
    let key = format!("{}|{lat},{lon}", config.weather.met_url);
    let kept = FORECASTS.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned();
    if let Some(cached) = &kept {
        if cached.expires.is_some_and(|e| now < e) {
            return Ok((cached.body.clone(), false));
        }
    }
    let mut request = HTTP.get(&config.weather.met_url).query(&[("lat", lat.as_str()), ("lon", lon.as_str())]);
    if let Some(since) = kept.as_ref().and_then(|c| c.last_modified.as_deref()) {
        request = request.header("If-Modified-Since", since);
    }
    let failed = |why: String| match &kept {
        Some(cached) => Ok((cached.body.clone(), true)),
        None => Err(why),
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => return failed(format!("the weather service did not answer ({})", short_error(&err))),
    };
    let status = response.status().as_u16();
    let expires = http_date(response.headers().get("expires"));
    let last_modified = response.headers().get("last-modified").and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = match status {
        304 => match &kept {
            Some(cached) => cached.body.clone(),
            None => return Err("the weather service answered 304 without a kept forecast".into()),
        },
        200 | 203 => {
            if status == 203 {
                tracing::warn!("MET Norway: this Locationforecast version is deprecated (203)");
            }
            match response.json::<Value>().await {
                Ok(body) => body,
                Err(_) => return failed("the weather service answered something unreadable".into()),
            }
        }
        429 => return failed("the weather service asks to slow down; try again in a while".into()),
        _ => return failed(format!("the weather service answered {status}")),
    };
    let mut forecasts = FORECASTS.lock().unwrap_or_else(|e| e.into_inner());
    let last_modified = last_modified.or_else(|| kept.and_then(|c| c.last_modified));
    forecasts.shift_remove(&key);
    forecasts.insert(key, Cached { body: body.clone(), last_modified, expires });
    while forecasts.len() > FORECASTS_KEPT {
        forecasts.shift_remove_index(0);
    }
    Ok((body, false))
}

// --- local time ----------------------------------------------------------------------------------------------------

fn last_sunday(year: i32, month: u32) -> NaiveDate {
    let next =
        if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1) } else { NaiveDate::from_ymd_opt(year, month + 1, 1) };
    let mut day = next.unwrap_or_default() - Duration::days(1);
    while day.weekday() != Weekday::Sun {
        day -= Duration::days(1);
    }
    day
}

fn nth_sunday(year: i32, month: u32, n: u32) -> NaiveDate {
    let mut day = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or_default();
    while day.weekday() != Weekday::Sun {
        day += Duration::days(1);
    }
    day + Duration::weeks(i64::from(n) - 1)
}

fn utc_at(date: NaiveDate, hour: i64) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0).unwrap_or_default().and_utc() + Duration::hours(hour)
}

/// The place's offset from UTC in minutes at `at`, from its country (and longitude where a country spans several
/// zones); None when unknown. Summer time: the EU's (and the UK's), North America's, Israel's.
pub fn utc_offset(country: &str, lon: f64, at: DateTime<Utc>) -> Option<i32> {
    let year = at.year();
    let eu = |standard: i32| {
        let summer = at >= utc_at(last_sunday(year, 3), 1) && at < utc_at(last_sunday(year, 10), 1);
        standard + if summer { 60 } else { 0 }
    };
    let offset = match country {
        "gb" | "ie" | "pt" | "fo" => eu(0),
        "es" if lon < -12.0 => eu(0), // the Canaries
        "fr" | "de" | "es" | "it" | "be" | "nl" | "lu" | "ch" | "at" | "dk" | "se" | "no" | "pl" | "cz" | "sk"
        | "hu" | "si" | "hr" | "mc" | "li" | "ad" | "sm" | "va" | "mt" | "rs" | "ba" | "me" | "mk" | "al" => eu(60),
        "fi" | "ee" | "lv" | "lt" | "gr" | "ro" | "bg" | "cy" | "ua" | "md" => eu(120),
        "is" => 0,
        "sg" => 480,
        "il" => {
            // From the Friday before the last Sunday of March, 02:00, to the last Sunday of October, 02:00.
            let start = utc_at(last_sunday(year, 3) - Duration::days(2), 0);
            let end = utc_at(last_sunday(year, 10), -1);
            120 + if at >= start && at < end { 60 } else { 0 }
        }
        "us" | "ca" => {
            if lon < -150.0 && country == "us" {
                return Some(-600); // Hawaii, no summer time
            }
            let hours = ((lon / 15.0).round() as i32).clamp(-9, -4);
            let standard = hours * 60;
            // From the second Sunday of March, 02:00, to the first Sunday of November, 02:00, local time.
            let start = utc_at(nth_sunday(year, 3, 2), 2 - i64::from(hours));
            let end = utc_at(nth_sunday(year, 11, 1), 1 - i64::from(hours));
            standard + if at >= start && at < end { 60 } else { 0 }
        }
        _ => return None,
    };
    Some(offset)
}

// --- the text for the agent ----------------------------------------------------------------------------------------

/// MET's symbol code in words: « lightrainshowersandthunder_day » → « light rain showers and thunder ».
pub fn symbol_words(code: &str) -> String {
    let base = code.split('_').next().unwrap_or(code);
    const WORDS: [(&str, &str); 13] = [
        ("clearsky", "clear sky"),
        ("partlycloudy", "partly cloudy"),
        ("cloudy", "cloudy"),
        ("fair", "fair"),
        ("fog", "fog"),
        ("light", "light"),
        ("heavy", "heavy"),
        ("rain", "rain"),
        ("sleet", "sleet"),
        ("snow", "snow"),
        ("showers", "showers"),
        ("and", "and"),
        ("thunder", "thunder"),
    ];
    let mut rest = base;
    let mut words: Vec<&str> = Vec::new();
    'outer: while !rest.is_empty() {
        for (token, word) in WORDS {
            if let Some(after) = rest.strip_prefix(token) {
                words.push(word);
                rest = after;
                continue 'outer;
            }
        }
        return base.to_string();
    }
    words.join(" ")
}

#[derive(Debug, Clone, Default)]
struct Step {
    time: DateTime<Utc>,
    temp: Option<f64>,
    wind: Option<f64>,
    gust: Option<f64>,
    humidity: Option<f64>,
    symbol1: Option<String>,
    rain1: Option<f64>,
    chance1: Option<f64>,
    thunder1: Option<f64>,
    symbol6: Option<String>,
    rain6: Option<f64>,
    max6: Option<f64>,
    min6: Option<f64>,
}

fn steps_of(body: &Value) -> Vec<Step> {
    let Some(series) = body.pointer("/properties/timeseries").and_then(Value::as_array) else { return Vec::new() };
    let mut steps: Vec<Step> = series
        .iter()
        .filter_map(|entry| {
            let time = DateTime::parse_from_rfc3339(entry.get("time")?.as_str()?).ok()?.with_timezone(&Utc);
            let data = entry.get("data")?;
            let instant = data.pointer("/instant/details");
            let detail = |key: &str| num(instant.and_then(|d| d.get(key)));
            let symbol = |period: &str| {
                data.pointer(&format!("/{period}/summary/symbol_code")).and_then(Value::as_str).map(str::to_string)
            };
            let next = |period: &str, key: &str| num(data.pointer(&format!("/{period}/details/{key}")));
            Some(Step {
                time,
                temp: detail("air_temperature"),
                wind: detail("wind_speed"),
                gust: detail("wind_speed_of_gust"),
                humidity: detail("relative_humidity"),
                symbol1: symbol("next_1_hours"),
                rain1: next("next_1_hours", "precipitation_amount"),
                chance1: next("next_1_hours", "probability_of_precipitation"),
                thunder1: next("next_1_hours", "probability_of_thunder"),
                symbol6: symbol("next_6_hours"),
                rain6: next("next_6_hours", "precipitation_amount"),
                max6: next("next_6_hours", "air_temperature_max"),
                min6: next("next_6_hours", "air_temperature_min"),
            })
        })
        .collect();
    steps.sort_by_key(|s| s.time);
    steps
}

fn kmh(ms: f64) -> i64 {
    (ms * 3.6).round() as i64
}

fn deg(c: f64) -> String {
    format!("{} °C", c.round() as i64)
}

fn mm(amount: f64) -> String {
    format!("{:.1} mm", amount)
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn day_name(date: NaiveDate) -> String {
    format!(
        "{} {} {}",
        WEEKDAYS[date.weekday().num_days_from_monday() as usize],
        date.day(),
        MONTHS[date.month0() as usize]
    )
}

#[derive(Default)]
struct Day {
    low: Option<f64>,
    high: Option<f64>,
    rain: f64,
    chance: Option<f64>,
    gust: Option<f64>,
    wind: Option<f64>,
    /// (distance of its step to local noon in hours, symbol): the day's look is the one nearest noon.
    symbol: Option<(i64, String)>,
    thunder: bool,
    snow: bool,
}

fn max_of(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

fn min_of(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The tool's answer for a forecast: what the agent reads (English, plain lines; it answers in the person's language).
pub fn describe(body: &Value, place: &Place, days: i64, now: DateTime<Utc>, stale: bool) -> String {
    let steps = steps_of(body);
    let offset = utc_offset(&place.country, place.lon, now);
    // Each time with its own offset: a forecast may cross a change of summer time.
    let local = |t: DateTime<Utc>| {
        let minutes = utc_offset(&place.country, place.lon, t).unwrap_or(0);
        t.with_timezone(&FixedOffset::east_opt(minutes * 60).unwrap_or(Utc.fix()))
    };
    let mut out: Vec<String> = Vec::new();
    let zone_line = match offset {
        Some(minutes) => {
            let sign = if minutes < 0 { '-' } else { '+' };
            format!("Times are the place's local time (UTC{sign}{:02}:{:02}).", minutes.abs() / 60, minutes.abs() % 60)
        }
        None => "Times are UTC (the place's time zone is unknown here).".to_string(),
    };
    out.push(format!("Weather for {} ({:.2}, {:.2}). {zone_line}", place.name, place.lat, place.lon));
    if let Some(updated) = body.pointer("/properties/meta/updated_at").and_then(Value::as_str) {
        if let Ok(at) = DateTime::parse_from_rfc3339(updated) {
            let note =
                if stale { " — the weather service did not answer, this is the last forecast kept" } else { "" };
            out.push(format!("Forecast updated {}{note}.", local(at.with_timezone(&Utc)).format("%Y-%m-%d %H:%M")));
        }
    }
    let Some(current) = steps.iter().find(|s| s.time + Duration::minutes(30) >= now) else {
        out.push("No forecast for now: the data is too old.".into());
        return out.join("\n");
    };

    // Now.
    let mut now_line = format!("Now ({}):", local(current.time).format("%H:%M"));
    if let Some(t) = current.temp {
        now_line.push_str(&format!(" {}", deg(t)));
    }
    if let Some(symbol) = current.symbol1.as_deref().or(current.symbol6.as_deref()) {
        now_line.push_str(&format!(", {}", symbol_words(symbol)));
    }
    if let Some(w) = current.wind {
        now_line.push_str(&format!(", wind {} km/h", kmh(w)));
        if let Some(g) = current.gust.filter(|g| kmh(*g) > kmh(w) + 10) {
            now_line.push_str(&format!(" (gusts {})", kmh(g)));
        }
    }
    if let Some(h) = current.humidity {
        now_line.push_str(&format!(", humidity {} %", h.round() as i64));
    }
    out.push(now_line);

    // The next 24 hours.
    let hours: Vec<&Step> = steps
        .iter()
        .filter(|s| s.time >= current.time && s.time < current.time + Duration::hours(24) && s.symbol1.is_some())
        .collect();
    if let Some(first) = hours.iter().find(|s| s.rain1.unwrap_or(0.0) >= 0.2) {
        let total: f64 = hours.iter().filter_map(|s| s.rain1).sum();
        out.push(format!("Rain: from {} ({} over the next 24 hours).", local(first.time).format("%H:%M"), mm(total)));
    } else if !hours.is_empty() {
        out.push("Rain: none expected in the next 24 hours.".into());
    }
    if !hours.is_empty() {
        out.push("Next 24 hours:".into());
        for s in &hours {
            let mut line = format!("{}", local(s.time).format("%H:%M"));
            if let Some(t) = s.temp {
                line.push_str(&format!(" {}", deg(t)));
            }
            if let Some(symbol) = &s.symbol1 {
                line.push_str(&format!(", {}", symbol_words(symbol)));
            }
            let rain = s.rain1.unwrap_or(0.0);
            if rain > 0.0 {
                line.push_str(&format!(", {}", mm(rain)));
            }
            if let Some(chance) = s.chance1.filter(|c| *c >= 20.0) {
                line.push_str(&format!(" ({} % chance of rain)", chance.round() as i64));
            }
            if let Some(w) = s.wind {
                line.push_str(&format!(", wind {} km/h", kmh(w)));
            }
            out.push(format!("- {line}"));
        }
    }

    // The days.
    let today = local(now).date_naive();
    let last = today + Duration::days(days.clamp(1, MAX_DAYS) - 1);
    let mut by_day: IndexMap<NaiveDate, Day> = IndexMap::new();
    let mut covered = DateTime::<Utc>::MIN_UTC;
    for s in steps.iter().filter(|s| s.time + Duration::minutes(30) >= now) {
        let date = local(s.time).date_naive();
        if date > last {
            break;
        }
        let day = by_day.entry(date).or_default();
        day.low = min_of(min_of(day.low, s.temp), s.min6);
        day.high = max_of(max_of(day.high, s.temp), s.max6);
        day.gust = max_of(day.gust, s.gust);
        day.wind = max_of(day.wind, s.wind);
        day.chance = max_of(day.chance, s.chance1);
        if s.time >= covered {
            if let Some(rain) = s.rain1 {
                day.rain += rain;
                covered = s.time + Duration::hours(1);
            } else if let Some(rain) = s.rain6 {
                day.rain += rain;
                covered = s.time + Duration::hours(6);
            }
        }
        if s.thunder1.is_some_and(|p| p >= 30.0) {
            day.thunder = true;
        }
        for symbol in [&s.symbol1, &s.symbol6].into_iter().flatten() {
            day.thunder |= symbol.contains("thunder");
            day.snow |= symbol.contains("snow") || symbol.contains("sleet");
        }
        let hour = i64::from(local(s.time).hour());
        // A 6-hour period is centred 3 hours after its step.
        let (centre, symbol) = match (&s.symbol6, &s.symbol1) {
            (Some(six), _) => (hour + 3, six.clone()),
            (None, Some(one)) => (hour, one.clone()),
            _ => continue,
        };
        let distance = (centre - 12).abs();
        if day.symbol.as_ref().is_none_or(|(d, _)| distance < *d) {
            day.symbol = Some((distance, symbol));
        }
    }
    if !by_day.is_empty() {
        out.push("Days:".into());
    }
    let mut heads_up: Vec<String> = Vec::new();
    for (date, day) in &by_day {
        let name = if *date == today { format!("Today, {}", day_name(*date)) } else { day_name(*date) };
        let mut line = name.clone();
        if let (Some(low), Some(high)) = (day.low, day.high) {
            line.push_str(&format!(": {} to {}", deg(low), deg(high)));
        }
        if let Some((_, symbol)) = &day.symbol {
            line.push_str(&format!(", {}", symbol_words(symbol)));
        }
        line.push_str(&format!(", rain {}", mm(day.rain)));
        if let Some(w) = day.wind {
            line.push_str(&format!(", wind up to {} km/h", kmh(w)));
        }
        if let Some(g) = day.gust {
            line.push_str(&format!(" (gusts {})", kmh(g)));
        }
        out.push(format!("- {line}"));
        if day.rain >= 20.0 {
            heads_up.push(format!("heavy rain {name} ({})", mm(day.rain)));
        }
        if day.high.is_some_and(|h| h >= 30.0) {
            heads_up.push(format!("heat {name} (up to {})", deg(day.high.unwrap_or_default())));
        }
        if day.low.is_some_and(|l| l <= -5.0) {
            heads_up.push(format!("hard frost {name} (down to {})", deg(day.low.unwrap_or_default())));
        }
        if day.thunder {
            heads_up.push(format!("thunderstorms possible {name}"));
        }
        if day.gust.is_some_and(|g| kmh(g) >= 70) {
            heads_up.push(format!("strong wind {name} (gusts {} km/h)", kmh(day.gust.unwrap_or_default())));
        }
        if day.snow {
            heads_up.push(format!("snow or sleet {name}"));
        }
    }
    if heads_up.is_empty() {
        out.push("Heads-ups: none in the forecast.".into());
    } else {
        out.push(format!(
            "Heads-ups (from the forecast, not official warnings — for those, the national weather service): {}.",
            heads_up.join("; ")
        ));
    }
    let mut credits = ATTRIBUTION.to_string();
    if !place.country.is_empty() {
        credits.push(' ');
        credits.push_str(OSM_ATTRIBUTION);
    }
    out.push(credits);
    out.join("\n")
}

/// One call of the tool.
pub async fn call(config: &Config, arguments: &Dict) -> Dict {
    call_at(config, arguments, Utc::now()).await
}

/// `call` at a given time (tests).
pub async fn call_at(config: &Config, arguments: &Dict, now: DateTime<Utc>) -> Dict {
    let asked = tk::strip(&tk::str_or_empty(arguments.get("place"))).to_string();
    if asked.is_empty() {
        return tool_text("give a place: the person's town, or ask them which one", true);
    }
    if asked.chars().count() > 200 {
        return tool_text("the place is too long: a town or an address", true);
    }
    let days = tk::as_py_int(arguments.get("days")).unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    let place = match parse_coordinates(&asked) {
        Some((lat, lon)) => Place { name: asked.clone(), lat, lon, country: String::new() },
        None => match geocode(config, &asked).await {
            Ok(Some(place)) => place,
            Ok(None) => {
                return tool_text(
                    &format!("no place found for « {asked} »: try the town's name with its country, or coordinates"),
                    true,
                )
            }
            Err(why) => return tool_text(&format!("{why}; try again later"), true),
        },
    };
    match forecast(config, place.lat, place.lon, now).await {
        Ok((body, stale)) => tool_text(&describe(&body, &place, days, now, stale), false),
        Err(why) => tool_text(&format!("{why}; try again later"), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn symbols_read_as_words() {
        assert_eq!(symbol_words("lightrainshowersandthunder_day"), "light rain showers and thunder");
        assert_eq!(symbol_words("partlycloudy_night"), "partly cloudy");
        assert_eq!(symbol_words("clearsky_polartwilight"), "clear sky");
        assert_eq!(symbol_words("heavysleet"), "heavy sleet");
        assert_eq!(symbol_words("mystery"), "mystery");
    }

    #[test]
    fn coordinates_are_read_and_checked() {
        assert_eq!(parse_coordinates("48.85, 2.35"), Some((48.85, 2.35)));
        assert_eq!(parse_coordinates("91,0"), None);
        assert_eq!(parse_coordinates("Lyon, France"), None);
    }

    #[test]
    fn local_time_follows_summer_time() {
        // Paris: summer time until the last Sunday of October (25 Oct 2026, 01:00 UTC).
        assert_eq!(utc_offset("fr", 2.35, utc("2026-10-24T12:00:00Z")), Some(120));
        assert_eq!(utc_offset("fr", 2.35, utc("2026-10-25T01:00:00Z")), Some(60));
        assert_eq!(utc_offset("gb", -0.1, utc("2026-07-01T12:00:00Z")), Some(60));
        assert_eq!(utc_offset("es", -15.4, utc("2026-01-10T12:00:00Z")), Some(0));
        // New York: from 8 March 2026 to 1 November 2026.
        assert_eq!(utc_offset("us", -74.0, utc("2026-10-22T12:00:00Z")), Some(-240));
        assert_eq!(utc_offset("us", -74.0, utc("2026-11-02T12:00:00Z")), Some(-300));
        assert_eq!(utc_offset("us", -122.4, utc("2026-03-01T12:00:00Z")), Some(-480));
        assert_eq!(utc_offset("sg", 103.8, utc("2026-03-01T12:00:00Z")), Some(480));
        assert_eq!(utc_offset("il", 34.8, utc("2026-03-28T12:00:00Z")), Some(180));
        assert_eq!(utc_offset("", 2.35, utc("2026-03-01T12:00:00Z")), None);
    }

    #[test]
    fn a_nominatim_answer_gives_the_place() {
        let answer = json!([{"lat": "45.7578137", "lon": "4.8320114", "name": "Lyon",
            "display_name": "Lyon, Métropole de Lyon, Rhône, Auvergne-Rhône-Alpes, France métropolitaine, France",
            "address": {"city": "Lyon", "state": "Auvergne-Rhône-Alpes", "country": "France", "country_code": "fr"}}]);
        let place = place_of(&answer).unwrap();
        assert_eq!(place.name, "Lyon, Auvergne-Rhône-Alpes, France");
        assert_eq!(place.country, "fr");
        assert!((place.lat - 45.7578).abs() < 1e-3);
        assert_eq!(place_of(&json!([])), None);
    }
}
