//! The way through a walk's or a trip's stops by the streets (owner-approved 2026-11-01, « le vrai trajet par les
//! rues »): an itinerary map (`map_image` with `line`, its markers in their order) is joined along the streets, paths
//! and roads rather than by a straight line across blocks and water, with its length and time on the card. A map of
//! places that do not follow each other (no `line`) is never routed.
//!
//! The routing source, chosen 2026-11-01: **FOSSGIS e.V.'s OSRM servers** (`routing.openstreetmap.de`, profiles
//! `routed-foot`, `routed-bike`, `routed-car`): free, no key (nothing to keep out of the code and the logs),
//! OpenStreetMap's data like the rest of the maps. Their terms (fossgis.de/arbeitsgruppen/osm-server/nutzungsbedingungen,
//! read that day) and how they are kept here:
//! - a User-Agent naming the application, never a library's (`weather::user_agent`: « BipAgents/<version> <site> »);
//! - at most one request a second (`TURN`, `GAP` apart) and one connection at a time (the same `TURN`);
//! - the attribution « © OpenStreetMap contributors » with a link to openstreetmap.org/fixthemap (`ATTRIBUTION`,
//!   `FIX_URL`: the apps show it under a routed map, the picture carries « © OpenStreetMap contributors »);
//! - commercial use only when the service is not a substantial part of the offer (their example: directions sketches
//!   on a website; here a line on a map card), no high-traffic website (each place's bridge asks for its own few
//!   routes a day, and keeps them: `CACHE_DAYS`), no mass downloads;
//! - « the URLs should not be hard-coded in the app »: the apps never call it, the address is the bridge's config
//!   (`[maps] streets_url`), and `[maps] streets = "off"` turns it off (the dashed straight line then).
//!
//! Not chosen: OpenRouteService (an account and a key per install, 2 000 directions a day: it would be optional config
//! for nothing better), Valhalla on valhalla1.openstreetmap.de (the same terms and operator, a heavier API), Google's
//! Routes API (the `route` tool's paid key, its results may not be kept).
//!
//! Privacy: the request goes from the bridge, never from the phone, and carries only the stops' coordinates (5
//! decimals, about a metre) and the profile: never a place's name, a note, the map's title or the person's words. The
//! stops keep the agent's order; they are reordered only when the agent asks (`reorder`: OSRM's trip service, first and
//! last kept). When routing is off, fails, takes longer than `[maps] streets_timeout` (6 s), or the stops are too far
//! apart for the way of travel, the map keeps the straight line, dashed, and the agent is told why.
//!
//! Not a Python module: Rust bridge only (owner group: tools-knowledge).

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::config::Config;
use crate::maps::km;
use crate::tools_knowledge as tk;

pub const FOSSGIS_URL: &str = "https://routing.openstreetmap.de";
/// What the apps show under a map routed along the streets (FOSSGIS's terms: the data's attribution, the router's).
pub const ATTRIBUTION: &str = "Route © OpenStreetMap contributors · OSRM";
/// Where the apps' attribution leads (FOSSGIS's terms: a way for the person to report the map's mistakes).
pub const FIX_URL: &str = "https://www.openstreetmap.org/fixthemap";
/// The ways of travel the streets are routed for (the map's `MODES` words); « transit » is not: a straight line.
pub const TRAVELS: [&str; 3] = ["walking", "bicycling", "driving"];
/// Between two requests (FOSSGIS: one a second at most).
const GAP: Duration = Duration::from_millis(1100);
const CACHE_DAYS: u64 = 30;
/// The most positions a route keeps once simplified (about 9 KB in the map's data).
pub const MAX_PATH: usize = 400;
/// Without a way of travel from the agent: on foot under this many km in all, or for a walk in a town (every hop under
/// `WALK_KM`, `TOWN_WALK_KM` in all: Belém and the Baixa in one day); else by car.
pub const WALK_KM: f64 = 3.0;
pub const TOWN_WALK_KM: f64 = 10.0;
static TURN: LazyLock<tokio::sync::Mutex<Option<Instant>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

/// The way along the streets: its positions (lat, lon, simplified), its length (m) and time (s), as the router gave.
#[derive(Debug, Clone, PartialEq)]
pub struct Way {
    pub path: Vec<(f64, f64)>,
    pub meters: f64,
    pub seconds: f64,
}

/// An itinerary planned: its way of travel, the stops' new order when the agent asked for one, the way along the
/// streets, or why there is none (said to the agent; the map draws the straight line, dashed).
#[derive(Debug, Clone, PartialEq)]
pub struct Planned {
    pub travel: &'static str,
    pub order: Option<Vec<usize>>,
    pub way: Option<Way>,
    pub why: Option<String>,
}

/// How the itinerary goes: the agent's `travel`, else its `directions` when it is one of `TRAVELS` (or « transit »),
/// else guessed from the distance (`guess`).
pub fn travel_of(travel: Option<&str>, directions: Option<&str>, stops: &[(f64, f64)]) -> &'static str {
    let said = |m: Option<&str>| {
        m.map(str::trim).and_then(|m| TRAVELS.iter().chain(["transit"].iter()).find(|t| **t == m).copied())
    };
    said(travel).or_else(|| said(directions)).unwrap_or_else(|| guess(stops))
}

/// On foot for a short walk or a walk in a town (stop to stop under `WALK_KM`), else by car.
pub fn guess(stops: &[(f64, f64)]) -> &'static str {
    let legs: Vec<f64> = stops.windows(2).map(|p| km(p[0], p[1])).collect();
    let total: f64 = legs.iter().sum();
    let longest = legs.iter().copied().fold(0.0, f64::max);
    if total <= WALK_KM || (longest <= WALK_KM && total <= TOWN_WALK_KM) {
        "walking"
    } else {
        "driving"
    }
}

/// The straight line through the stops, km.
pub fn straight_km(stops: &[(f64, f64)]) -> f64 {
    stops.windows(2).map(|p| km(p[0], p[1])).sum()
}

/// Why the stops are too far apart to be routed that way (a hop, or the whole, beyond what a walk, a ride or a drive
/// takes): None when they are not.
pub fn too_far(travel: &str, stops: &[(f64, f64)]) -> Option<String> {
    let (hop, all) = match travel {
        "walking" => (15.0, 40.0),
        "bicycling" => (60.0, 200.0),
        _ => (800.0, 2500.0),
    };
    let longest = stops.windows(2).map(|p| km(p[0], p[1])).fold(0.0, f64::max);
    let total = straight_km(stops);
    if longest > hop {
        Some(format!("a hop of {longest:.0} km is too far to go {}", words(travel)))
    } else if total > all {
        Some(format!("{total:.0} km in all is too far to go {}", words(travel)))
    } else {
        None
    }
}

/// « on foot », « by bike », « by car » (to the agent).
pub fn words(travel: &str) -> &'static str {
    match travel {
        "walking" => "on foot",
        "bicycling" => "by bike",
        "transit" => "by public transport",
        _ => "by car",
    }
}

/// How far (m) the line may stray from the street's own shape once simplified: a walk keeps its corners.
fn tolerance(travel: &str) -> f64 {
    match travel {
        "walking" => 4.0,
        "bicycling" => 6.0,
        _ => 15.0,
    }
}

/// The positions that keep the line's shape within `meters` of it (Douglas-Peucker on a flat projection around the
/// first position: good to a few cm over a town, a few m over a country), the first and last kept.
pub fn simplify(path: &[(f64, f64)], meters: f64) -> Vec<(f64, f64)> {
    if path.len() < 3 {
        return path.to_vec();
    }
    let lat0 = path[0].0.to_radians().cos();
    let flat: Vec<(f64, f64)> = path.iter().map(|(la, lo)| (lo * lat0 * 111_320.0, la * 110_574.0)).collect();
    let mut keep = vec![false; path.len()];
    keep[0] = true;
    keep[path.len() - 1] = true;
    let mut spans = vec![(0, path.len() - 1)];
    while let Some((a, b)) = spans.pop() {
        if b <= a + 1 {
            continue;
        }
        let ((ax, ay), (bx, by)) = (flat[a], flat[b]);
        let (dx, dy) = (bx - ax, by - ay);
        let length = (dx * dx + dy * dy).sqrt();
        let (mut far, mut at) = (0.0, a);
        for (i, (px, py)) in flat.iter().enumerate().take(b).skip(a + 1) {
            let d = if length < 1e-9 {
                ((px - ax).powi(2) + (py - ay).powi(2)).sqrt()
            } else {
                ((px - ax) * dy - (py - ay) * dx).abs() / length
            };
            if d > far {
                (far, at) = (d, i);
            }
        }
        if far > meters {
            keep[at] = true;
            spans.push((a, at));
            spans.push((at, b));
        }
    }
    path.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

/// The route's positions small enough for the map's data: simplified for its way of travel, then more coarsely until at
/// most `MAX_PATH` remain, each rounded to 5 decimals (about a metre).
pub fn small(path: &[(f64, f64)], travel: &str) -> Vec<(f64, f64)> {
    let mut meters = tolerance(travel);
    let mut kept = simplify(path, meters);
    while kept.len() > MAX_PATH {
        meters *= 2.0;
        kept = simplify(path, meters);
    }
    let round = |v: f64| (v * 1e5).round() / 1e5;
    kept.into_iter().map(|(la, lo)| (round(la), round(lo))).collect()
}

fn profile(travel: &str) -> &'static str {
    match travel {
        "walking" => "routed-foot",
        "bicycling" => "routed-bike",
        _ => "routed-car",
    }
}

/// `lon,lat;lon,lat…` (OSRM's order), 5 decimals: all the router is told.
fn coordinates(stops: &[(f64, f64)]) -> String {
    stops.iter().map(|(la, lo)| format!("{lo:.5},{la:.5}")).collect::<Vec<_>>().join(";")
}

fn cache_file(config: &Config, kind: &str, travel: &str, stops: &[(f64, f64)]) -> PathBuf {
    let key = format!("{kind}|{travel}|{}|{}", config.maps.streets_url, coordinates(stops));
    let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(key.as_bytes()));
    PathBuf::from(config.data_dir()).join("map-routes").join(format!("{}.json", &hash[..24]))
}

fn cached(path: &Path) -> Option<Value> {
    let age = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let fresh = SystemTime::now().duration_since(age).ok()? < Duration::from_secs(CACHE_DAYS * 86400);
    fresh.then(|| std::fs::read_to_string(path).ok()).flatten().and_then(|t| serde_json::from_str(&t).ok())
}

fn keep(path: &Path, value: &Value) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = tk::replace_with(&path.with_extension("part"), path, value.to_string().as_bytes());
    }
}

/// One request to the router (`route` or `trip`), within its terms: one at a time, `GAP` apart, the application's
/// User-Agent, `[maps] streets_timeout` at most. Its answer when OSRM says « Ok ».
async fn ask(config: &Config, service: &str, travel: &str, stops: &[(f64, f64)], query: &str) -> Result<Value, String> {
    // OSRM's own path has a profile word too; FOSSGIS picks the profile by the first part (`routed-foot`)
    let url =
        format!("{}/{}/{service}/v1/driving/{}?{query}", config.maps.streets_url, profile(travel), coordinates(stops));
    let mut turn = TURN.lock().await;
    if let Some(last) = *turn {
        if last.elapsed() < GAP {
            tokio::time::sleep(GAP - last.elapsed()).await;
        }
    }
    let answer = crate::weather::http().get(&url).timeout(config.maps.streets_timeout).send().await;
    let answer = match answer {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = tokio::time::timeout(config.maps.streets_timeout, response.json::<Value>()).await;
            match body {
                Ok(Ok(value)) => Ok((status, value)),
                Ok(Err(_)) => Err(format!("the street routing answered {status}, unreadable")),
                Err(_) => Err("the street routing did not answer in time".to_string()),
            }
        }
        Err(e) if e.is_timeout() => Err("the street routing did not answer in time".to_string()),
        Err(_) => Err("the street routing did not answer".to_string()),
    };
    *turn = Some(Instant::now());
    drop(turn);
    let (status, value) = answer?;
    match value["code"].as_str() {
        Some("Ok") if status == 200 => Ok(value),
        Some("NoRoute") | Some("NoTrips") => Err("the streets do not join these stops".into()),
        Some("NoSegment") => Err("a stop is too far from any street".into()),
        _ => Err(format!("the street routing answered {status}")),
    }
}

/// The way along the streets through the stops in their order (from the cache when asked within `CACHE_DAYS`).
pub async fn way(config: &Config, travel: &str, stops: &[(f64, f64)]) -> Result<Way, String> {
    let file = cache_file(config, "route", travel, stops);
    let value = match cached(&file) {
        Some(value) => value,
        None => {
            let answer =
                ask(config, "route", travel, stops, "overview=full&geometries=geojson&steps=false&alternatives=false")
                    .await?;
            let route = &answer["routes"][0];
            let path: Vec<(f64, f64)> = route["geometry"]["coordinates"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| Some((p.get(1)?.as_f64()?, p.get(0)?.as_f64()?)))
                .filter(|(la, lo)| (-90.0..=90.0).contains(la) && (-180.0..=180.0).contains(lo))
                .collect();
            let (Some(meters), Some(seconds)) = (route["distance"].as_f64(), route["duration"].as_f64()) else {
                return Err("the street routing gave no way".into());
            };
            if path.len() < 2 {
                return Err("the street routing gave no way".into());
            }
            let kept = json!({"path": small(&path, travel).iter().map(|(a, b)| [*a, *b]).collect::<Vec<_>>(),
                              "meters": meters.round(), "seconds": seconds.round()});
            keep(&file, &kept);
            kept
        }
    };
    let path = value["path"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?)))
        .collect::<Vec<_>>();
    match (value["meters"].as_f64(), value["seconds"].as_f64()) {
        (Some(meters), Some(seconds)) if path.len() >= 2 => Ok(Way { path, meters, seconds }),
        _ => Err("the street routing gave no way".into()),
    }
}

/// The stops' shortest order, the first and the last kept (OSRM's trip service): `order[k]` is the stop that comes k-th.
pub async fn order(config: &Config, travel: &str, stops: &[(f64, f64)]) -> Result<Vec<usize>, String> {
    if stops.len() < 4 {
        return Ok((0..stops.len()).collect()); // nothing between the first and the last to move
    }
    let file = cache_file(config, "trip", travel, stops);
    let value = match cached(&file) {
        Some(value) => value,
        None => {
            let answer =
                ask(config, "trip", travel, stops, "source=first&destination=last&roundtrip=false&overview=false")
                    .await?;
            let places: Vec<u64> = answer["waypoints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|w| w["waypoint_index"].as_u64())
                .collect();
            let kept = json!({"places": places});
            keep(&file, &kept);
            kept
        }
    };
    let places: Vec<usize> =
        value["places"].as_array().into_iter().flatten().filter_map(|v| v.as_u64()).map(|v| v as usize).collect();
    let mut order: Vec<usize> = (0..stops.len()).collect();
    order.sort_by_key(|i| places.get(*i).copied().unwrap_or(usize::MAX));
    let mut sorted = order.clone();
    sorted.sort_unstable();
    if places.len() != stops.len() || sorted != (0..stops.len()).collect::<Vec<_>>() || order[0] != 0 {
        return Err("the street routing gave no order".into());
    }
    Ok(order)
}

/// An itinerary's way: its way of travel, the new order when asked (`reorder`), the way along the streets or why not.
pub async fn plan(
    config: &Config,
    travel: Option<&str>,
    directions: Option<&str>,
    reorder: bool,
    stops: &[(f64, f64)],
) -> Planned {
    let travel = travel_of(travel, directions, stops);
    let none = |why: String, order: Option<Vec<usize>>| Planned { travel, order, way: None, why: Some(why) };
    if config.maps.streets == "off" {
        return none("no street routing on this server".into(), None);
    }
    if travel == "transit" {
        return none("public transport is not routed here".into(), None);
    }
    if let Some(why) = too_far(travel, stops) {
        return none(why, None);
    }
    let order = if reorder {
        match order(config, travel, stops).await {
            Ok(order) if order.iter().enumerate().any(|(k, i)| k != *i) => Some(order),
            Ok(_) => None,
            Err(why) => return none(why, None),
        }
    } else {
        None
    };
    let stops: Vec<(f64, f64)> = match &order {
        Some(order) => order.iter().map(|i| stops[*i]).collect(),
        None => stops.to_vec(),
    };
    match way(config, travel, &stops).await {
        Ok(way) => Planned { travel, order, way: Some(way), why: None },
        Err(why) => none(why, order),
    }
}

/// « 2.4 km on foot, about 35 min » (to the agent).
pub fn summary(travel: &str, way: &Way) -> String {
    let distance = if way.meters < 1000.0 {
        format!("{:.0} m", (way.meters / 10.0).round() * 10.0)
    } else if way.meters < 10_000.0 {
        format!("{:.1} km", way.meters / 1000.0)
    } else {
        format!("{:.0} km", way.meters / 1000.0)
    };
    let minutes = (way.seconds / 60.0).round().max(1.0) as u64;
    let time = if minutes < 60 { format!("{minutes} min") } else { format!("{} h {:02}", minutes / 60, minutes % 60) };
    format!("{distance} {}, about {time}", words(travel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_way_of_travel_is_the_agent_s_else_guessed_from_the_distance() {
        // Le Moulleau: four stops within 600 m
        let moulleau = [(44.6412, -1.1991), (44.6417, -1.2044), (44.6432, -1.2020), (44.6401, -1.1979)];
        assert_eq!(travel_of(None, None, &moulleau), "walking");
        assert_eq!(travel_of(Some("bicycling"), Some("walking"), &moulleau), "bicycling");
        assert_eq!(travel_of(None, Some("driving"), &moulleau), "driving");
        assert_eq!(travel_of(Some("teleport"), Some("transit"), &moulleau), "transit");
        // a Lisbon walk: Belém, then the Baixa 6 km away: a hop too long to guess a walk; by car
        let lisbon = [(38.6916, -9.2160), (38.6979, -9.2068), (38.7075, -9.1364), (38.7139, -9.1334)];
        assert_eq!(guess(&lisbon), "driving");
        // the Baixa and Alfama: hops under 3 km, 4 km in all: a walk
        let baixa =
            [(38.7075, -9.1364), (38.7139, -9.1334), (38.7118, -9.1300), (38.7107, -9.1450), (38.7130, -9.1400)];
        assert_eq!(guess(&baixa), "walking");
        // Lisbon → Porto: too far on foot, fine by car
        let trip = [(38.6916, -9.2160), (41.1458, -8.6139)];
        assert!(too_far("walking", &trip).unwrap().contains("too far to go on foot"));
        assert!(too_far("driving", &trip).is_none());
        assert!(too_far("walking", &moulleau).is_none());
    }

    #[test]
    fn a_route_is_simplified_within_metres_of_its_shape_and_kept_small() {
        // a straight street with points every 10 m and a wiggle of 1 m: two ends left
        let street: Vec<(f64, f64)> =
            (0..100).map(|i| (44.64 + i as f64 * 0.00009, -1.20 + if i % 2 == 0 { 0.0 } else { 0.0000125 })).collect();
        let kept = simplify(&street, 4.0);
        assert_eq!(kept, vec![street[0], street[99]]);
        // a corner (an L): the corner kept
        let mut corner: Vec<(f64, f64)> = (0..50).map(|i| (44.64 + i as f64 * 0.0001, -1.20)).collect();
        corner.extend((1..50).map(|i| (44.6449, -1.20 + i as f64 * 0.0001)));
        let kept = simplify(&corner, 4.0);
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[1], corner[49]);
        // a long winding drive: never more than MAX_PATH, the ends kept, rounded to 5 decimals
        let winding: Vec<(f64, f64)> =
            (0..20_000).map(|i| (38.7 + i as f64 * 0.00001, -9.1 + (i as f64 / 300.0).sin() * 0.02)).collect();
        let kept = small(&winding, "driving");
        assert!(kept.len() <= MAX_PATH && kept.len() > 10, "{}", kept.len());
        assert_eq!(kept[0], (38.7, -9.1));
        assert!(kept
            .iter()
            .all(|(a, b)| ((a * 1e5).round() / 1e5 - a).abs() < 1e-12 && ((b * 1e5).round() / 1e5 - b).abs() < 1e-12));
        assert_eq!(simplify(&winding[..2], 4.0).len(), 2);
    }

    #[test]
    fn the_agent_hears_the_way_in_plain_words() {
        let way = Way { path: vec![], meters: 2412.0, seconds: 2090.0 };
        assert_eq!(summary("walking", &way), "2.4 km on foot, about 35 min");
        let way = Way { path: vec![], meters: 312_400.0, seconds: 11_400.0 };
        assert_eq!(summary("driving", &way), "312 km by car, about 3 h 10");
        let way = Way { path: vec![], meters: 914.0, seconds: 30.0 };
        assert_eq!(summary("bicycling", &way), "910 m by bike, about 1 min");
        assert_eq!(coordinates(&[(44.641213, -1.19909)]), "-1.19909,44.64121", "lon first, 5 decimals: all it is told");
    }
}
