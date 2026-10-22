//! The `route` tool (decided 2026-10-22): a route to a place as a link that opens the person's maps app with it
//! prefilled — Google Maps' universal directions link, which the iPhone app opens in Apple Maps and the Android app
//! in Google Maps, both as an « Ouvrir l'itinéraire » button under the reply. Every agent has it (nothing leaves the
//! server for the link alone).
//!
//! With a Google Maps Platform key (`[maps] google_key` or `google_key_file` in the bridge's config, never shown to
//! the agents) the tool also gives the travel time and distance from Google's Routes API (`computeRoutes`): with
//! the traffic for a drive, the timetables for public transport. Then the origin, the destination, the mode and the
//! times go to Google; nothing is kept (Google's terms forbid caching the results). Off without a key.
//!
//! No Python counterpart (owner group: tools-knowledge).

use std::time::Duration as StdDuration;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};

use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "route";
pub const MODES: [&str; 4] = ["driving", "walking", "transit", "bicycling"];
pub const MAPS_DIR_URL: &str = "https://www.google.com/maps/dir/";

/// The tool's definition (every agent).
pub fn route_tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "A route to a place, as a link that opens the person's maps app with the route ready (Apple \
    Maps on an iPhone, Google Maps elsewhere): put the link alone on a line in your reply, the app shows it as a \
    button. When this server has a routing service, it also gives the travel time (with the traffic, or by the \
    timetables) and the distance. Without an origin the maps app starts from where the person is; for a travel \
    time, give an origin (their home address from your memory, if it fits).",
        "inputSchema": {
            "type": "object",
            "properties": {
                "destination": {"type": "string", "maxLength": 300, "description": "an address or a place's name"},
                "origin": {"type": "string", "maxLength": 300,
                           "description": "where from (default: where the person is when they open the link)"},
                "mode": {"type": "string", "enum": MODES, "description": "default: driving"},
                "depart_at": {"type": "string",
                              "description": "optional departure time, YYYY-MM-DDTHH:MM (the person's time) or with an offset"},
                "arrive_by": {"type": "string",
                              "description": "optional arrival time for public transport, same format"},
            },
            "required": ["destination"],
        },
    })
}

/// Google Maps' universal directions link (https://developers.google.com/maps/documentation/urls/get-started).
pub fn maps_link(origin: Option<&str>, destination: &str, mode: &str) -> String {
    let mut params: Vec<(&str, &str)> = vec![("api", "1")];
    if let Some(origin) = origin {
        params.push(("origin", origin));
    }
    params.push(("destination", destination));
    params.push(("travelmode", mode));
    // Spaces as %20 rather than « + »: every reader of the link decodes them the same way (a « + » is %2B).
    reqwest::Url::parse_with_params(MAPS_DIR_URL, &params)
        .map(|u| String::from(u).replace('+', "%20"))
        .unwrap_or_default()
}

/// « 2026-10-23T08:30 » (the server's time zone) or an RFC 3339 time → RFC 3339 in UTC, as Google wants it.
pub fn parse_time(text: &str) -> Option<String> {
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Some(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    }
    let naive = ["%Y-%m-%dT%H:%M", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(text, format).ok())?;
    let at = Local.from_local_datetime(&naive).earliest()?;
    Some(at.with_timezone(&Utc).to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// « 1534s » → 1534.
fn seconds(value: Option<&Value>) -> Option<i64> {
    value?.as_str()?.trim_end_matches('s').parse::<f64>().ok().map(|s| s.round() as i64)
}

/// 1534 s → « 26 min »; 4500 → « 1 h 15 min ».
pub fn duration_words(seconds: i64) -> String {
    let minutes = (seconds + 30) / 60;
    if minutes < 60 {
        format!("{} min", minutes.max(1))
    } else if minutes % 60 == 0 {
        format!("{} h", minutes / 60)
    } else {
        format!("{} h {:02} min", minutes / 60, minutes % 60)
    }
}

fn distance_words(meters: i64) -> String {
    if meters < 1000 {
        format!("{meters} m")
    } else {
        format!("{:.1} km", meters as f64 / 1000.0)
    }
}

/// A route asked for: the places, the mode (one of `MODES`), the times in RFC 3339.
#[derive(Debug, Clone, Copy)]
pub struct Trip<'a> {
    pub origin: &'a str,
    pub destination: &'a str,
    pub mode: &'a str,
    pub depart_at: Option<&'a str>,
    pub arrive_by: Option<&'a str>,
}

/// The travel time and distance from Google's Routes API: Ok(text) for the agent, Err(why) when unavailable.
pub async fn travel(http: &reqwest::Client, config: &Config, key: &str, trip: &Trip<'_>) -> Result<String, String> {
    let Trip { origin, destination, mode, depart_at, arrive_by } = *trip;
    let travel_mode = match mode {
        "walking" => "WALK",
        "transit" => "TRANSIT",
        "bicycling" => "BICYCLE",
        _ => "DRIVE",
    };
    let mut body = json!({
        "origin": {"address": origin},
        "destination": {"address": destination},
        "travelMode": travel_mode,
        "units": "METRIC",
    });
    if travel_mode == "DRIVE" {
        body["routingPreference"] = json!("TRAFFIC_AWARE");
    }
    match (arrive_by, depart_at) {
        (Some(arrive), _) if travel_mode == "TRANSIT" => body["arrivalTime"] = json!(arrive),
        (_, Some(depart)) => body["departureTime"] = json!(depart),
        _ => {}
    }
    let response = http
        .post(&config.maps.routes_url)
        .header("X-Goog-Api-Key", key)
        .header("X-Goog-FieldMask", "routes.duration,routes.staticDuration,routes.distanceMeters")
        .timeout(StdDuration::from_secs(20))
        .json(&body)
        .send()
        .await
        .map_err(|_| "the routing service did not answer".to_string())?;
    let status = response.status().as_u16();
    if status != 200 {
        tracing::warn!(status = status, "Google Routes API refused a route");
        return Err(format!("the routing service answered {status}"));
    }
    let answer: Value = response.json().await.map_err(|_| "the routing service answered something unreadable")?;
    let Some(route) = answer.get("routes").and_then(Value::as_array).and_then(|r| r.first()) else {
        return Err("no route found between these places".into());
    };
    let Some(duration) = seconds(route.get("duration")) else { return Err("no travel time in the answer".into()) };
    let mut line = format!("Travel time: {}", duration_words(duration));
    if travel_mode == "DRIVE" {
        match seconds(route.get("staticDuration")) {
            Some(free) if duration - free >= 120 => {
                line.push_str(&format!(" with the traffic ({} without)", duration_words(free)))
            }
            _ => line.push_str(" with the traffic"),
        }
    }
    if let Some(meters) = route.get("distanceMeters").and_then(Value::as_i64) {
        line.push_str(&format!(", {}", distance_words(meters)));
    }
    line.push_str(" (Google Maps).");
    Ok(line)
}

/// One call of the tool.
pub async fn call(http: &reqwest::Client, config: &Config, arguments: &Dict) -> Dict {
    let text = |key: &str| {
        let value = tk::strip(&tk::str_or_empty(arguments.get(key))).to_string();
        (!value.is_empty()).then_some(value)
    };
    let Some(destination) = text("destination") else { return tool_text("give a destination", true) };
    let origin = text("origin");
    if destination.chars().count() > 300 || origin.as_ref().is_some_and(|o| o.chars().count() > 300) {
        return tool_text("the places are too long: an address or a place's name each", true);
    }
    let mode = text("mode").unwrap_or_else(|| "driving".into()).to_lowercase();
    if !MODES.contains(&mode.as_str()) {
        return tool_text(&format!("mode: one of {}", MODES.join(", ")), true);
    }
    let mut times: Vec<Option<String>> = Vec::new();
    for key in ["depart_at", "arrive_by"] {
        match text(key) {
            Some(raw) => match parse_time(&raw) {
                Some(at) => times.push(Some(at)),
                None => return tool_text(&format!("{key}: a time like 2026-10-23T08:30"), true),
            },
            None => times.push(None),
        }
    }
    let how = match mode.as_str() {
        "walking" => "on foot",
        "transit" => "by public transport",
        "bicycling" => "by bike",
        _ => "by car",
    };
    let mut lines = vec![format!(
        "Route to {destination} from {} {how}.",
        origin.as_deref().unwrap_or("where the person is when they open it")
    )];
    match (config.maps.google_key.as_deref(), origin.as_deref()) {
        (Some(key), Some(from)) => {
            let trip = Trip {
                origin: from,
                destination: &destination,
                mode: &mode,
                depart_at: times[0].as_deref(),
                arrive_by: times[1].as_deref(),
            };
            match travel(http, config, key, &trip).await {
                Ok(line) => lines.push(line),
                Err(why) => lines.push(format!("Travel time: not available ({why}); the maps app shows it.")),
            }
        }
        (Some(_), None) => {
            lines.push("Travel time: give an origin to get it here; the maps app shows it anyway.".into())
        }
        (None, _) => lines.push("Travel time: the maps app shows it (no routing service on this server).".into()),
    }
    lines.push(
        "For the person: put this link alone on a line in your reply; the app shows it as a button that opens their \
maps app with the route:"
            .into(),
    );
    lines.push(maps_link(origin.as_deref(), &destination, &mode));
    tool_text(&lines.join("\n"), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_is_google_maps_universal_one() {
        assert_eq!(
            maps_link(Some("10 rue de la Paix, Paris"), "Gare de Lyon", "transit"),
            "https://www.google.com/maps/dir/?api=1&origin=10%20rue%20de%20la%20Paix%2C%20Paris&destination=Gare%20de%20Lyon&travelmode=transit"
        );
        assert_eq!(
            maps_link(None, "Lyon", "driving"),
            "https://www.google.com/maps/dir/?api=1&destination=Lyon&travelmode=driving"
        );
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(duration_words(1534), "26 min");
        assert_eq!(duration_words(4500), "1 h 15 min");
        assert_eq!(duration_words(7200), "2 h");
        assert_eq!(parse_time("2026-10-23T08:30:00+02:00").as_deref(), Some("2026-10-23T06:30:00Z"));
        assert!(parse_time("2026-10-23T08:30").is_some());
        assert_eq!(parse_time("tomorrow"), None);
    }
}
