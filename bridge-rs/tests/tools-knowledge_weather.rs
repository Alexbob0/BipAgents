//! The `weather` and `route` tools (weather.rs, routes.rs) against fakes of MET Norway, Nominatim and Google's Routes
//! API: the switch, the geocoder's cache and pace, MET's caching rules, the text the agent reads, the link and the
//! travel time with a key.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bipbridge::config::{parse_config, Config};
use bipbridge::{agent_tools, routes, weather, Dict};
use chrono::{DateTime, Duration, Utc};
use common::{config_dict, spawn_fake, TestApp, Urls};
use indexmap::IndexMap;
use serde_json::{json, Value};

const TOKEN: &str = "weather-agent-token";

/// What a fake was asked: the path and query, and the headers that matter here.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<(String, HeaderMap, Value)>>>);

impl Seen {
    fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
    fn last(&self) -> (String, HeaderMap, Value) {
        self.0.lock().unwrap().last().cloned().unwrap()
    }
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
}

/// A MET Locationforecast answer from `start`: hourly for 48 hours, then every 6 hours for a week. Rain from 15:00 to
/// 17:00 UTC on the first day, a hot and stormy third day.
fn forecast_body(start: DateTime<Utc>) -> Value {
    let mut series = Vec::new();
    for h in 0..48 {
        let time = start + Duration::hours(h);
        let rainy = (5..7).contains(&h);
        let stormy = (30..36).contains(&h);
        series.push(json!({
            "time": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "data": {
                "instant": {"details": {"air_temperature": 14.0 + (h % 24) as f64 / 4.0, "wind_speed": 3.0,
                                        "wind_speed_of_gust": 8.0, "relative_humidity": 71.0}},
                "next_1_hours": {"summary": {"symbol_code": if rainy { "rain" } else if stormy { "rainandthunder" } else { "partlycloudy_day" }},
                                 "details": {"precipitation_amount": if rainy { 1.2 } else { 0.0 },
                                             "probability_of_precipitation": if rainy { 80.0 } else { 5.0 },
                                             "probability_of_thunder": if stormy { 40.0 } else { 0.0 }}},
                "next_6_hours": {"summary": {"symbol_code": "cloudy"}, "details": {"precipitation_amount": 2.4}},
            },
        }));
    }
    for k in 0..28 {
        let time = start + Duration::hours(48 + 6 * k);
        let hot = (8..12).contains(&k);
        series.push(json!({
            "time": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "data": {
                "instant": {"details": {"air_temperature": if hot { 29.0 } else { 12.0 }, "wind_speed": 5.0}},
                "next_6_hours": {"summary": {"symbol_code": if hot { "fair_day" } else { "lightrain" }},
                                 "details": {"precipitation_amount": if hot { 0.0 } else { 0.5 },
                                             "air_temperature_max": if hot { 32.0 } else { 13.0 },
                                             "air_temperature_min": if hot { 21.0 } else { 9.0 }}},
            },
        }));
    }
    json!({"type": "Feature", "geometry": {"type": "Point", "coordinates": [4.83, 45.76, 170]},
           "properties": {"meta": {"updated_at": start.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                                   "units": {"air_temperature": "celsius"}},
                          "timeseries": series}})
}

async fn met(State((seen, start)): State<(Seen, DateTime<Utc>)>, uri: Uri, headers: HeaderMap) -> Response {
    seen.0.lock().unwrap().push((uri.to_string(), headers.clone(), Value::Null));
    let expires = (start + Duration::hours(1)).format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    let modified = start.format("%a, %d %b %Y %H:%M:%S GMT").to_string();
    if headers.get("if-modified-since").and_then(|v| v.to_str().ok()) == Some(modified.as_str()) {
        return (StatusCode::NOT_MODIFIED, [("expires", expires)]).into_response();
    }
    ([("expires", expires), ("last-modified", modified)], Json(forecast_body(start))).into_response()
}

async fn nominatim(State(seen): State<Seen>, uri: Uri, headers: HeaderMap) -> Json<Value> {
    seen.0.lock().unwrap().push((uri.to_string(), headers, Value::Null));
    if uri.query().unwrap_or("").contains("Nowhere") {
        return Json(json!([]));
    }
    Json(json!([{"lat": "45.7578137", "lon": "4.8320114", "name": "Lyon",
                 "display_name": "Lyon, Métropole de Lyon, Rhône, Auvergne-Rhône-Alpes, France métropolitaine, France",
                 "address": {"city": "Lyon", "state": "Auvergne-Rhône-Alpes", "country": "France",
                             "country_code": "fr"}}]))
}

async fn google(State(seen): State<Seen>, uri: Uri, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    seen.0.lock().unwrap().push((uri.to_string(), headers.clone(), body));
    if headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) != Some("test-maps-key") {
        return (StatusCode::FORBIDDEN, Json(json!({"error": {"status": "PERMISSION_DENIED"}}))).into_response();
    }
    Json(json!({"routes": [{"duration": "1534s", "staticDuration": "1180s", "distanceMeters": 18234}]})).into_response()
}

struct Fakes {
    met: Seen,
    geocoder: Seen,
    google: Seen,
    urls: (String, String, String),
}

async fn fakes(start: DateTime<Utc>) -> Fakes {
    let (met_seen, geo_seen, google_seen) = (Seen::default(), Seen::default(), Seen::default());
    let met_url = spawn_fake(Router::new().route("/complete", get(met)).with_state((met_seen.clone(), start))).await;
    let geo_url = spawn_fake(Router::new().route("/search", get(nominatim)).with_state(geo_seen.clone())).await;
    let google_url =
        spawn_fake(Router::new().route("/directions/v2:computeRoutes", post(google)).with_state(google_seen.clone()))
            .await;
    Fakes {
        met: met_seen,
        geocoder: geo_seen,
        google: google_seen,
        urls: (
            format!("{met_url}/complete"),
            format!("{geo_url}/search"),
            format!("{google_url}/directions/v2:computeRoutes"),
        ),
    }
}

fn configured(dir: &std::path::Path, fakes: &Fakes, key: bool) -> Config {
    let mut dict = config_dict(dir, &Urls::default());
    dict["weather"] = json!({"met_url": fakes.urls.0, "geocoder_url": fakes.urls.1});
    dict["maps"] = json!({"routes_url": fakes.urls.2});
    if key {
        dict["maps"]["google_key"] = json!("test-maps-key");
    }
    let config = parse_config(&dict, None).unwrap();
    let mut tokens = IndexMap::new();
    tokens.insert("wellness".to_string(), agent_tools::digest(TOKEN));
    agent_tools::save_tokens(&config, &tokens).unwrap();
    config
}

fn args(value: Value) -> Dict {
    value.as_object().unwrap().clone()
}

fn text_of(result: &Dict) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

async fn rpc(app: &TestApp, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "method": method, "params": params, "id": 1});
    let bearer = format!("Bearer {TOKEN}");
    let answer = app
        .send_json("POST", "/v1/agent-tools/mcp", &[("authorization", &bearer), ("accept", "application/json")], &body)
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json()["result"].clone()
}

async fn tool_names(app: &TestApp) -> Vec<String> {
    rpc(app, "tools/list", json!({})).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn the_forecast_for_a_town_with_metres_cache_and_the_geocoders() {
    let start = at("2026-10-22T10:00:00Z");
    let fakes = fakes(start).await;
    let dir = tempfile::tempdir().unwrap();
    let config = configured(dir.path(), &fakes, false);

    let result = weather::call_at(&config, &args(json!({"place": "Lyon", "days": 5})), start).await;
    let text = text_of(&result);
    assert_eq!(result["isError"], false, "{text}");
    assert!(text.starts_with("Weather for Lyon, Auvergne-Rhône-Alpes, France (45.76, 4.83)."), "{text}");
    assert!(text.contains("Times are the place's local time (UTC+02:00)."), "{text}");
    assert!(text.contains("Now (12:00): 14 °C, partly cloudy, wind 11 km/h (gusts 29), humidity 71 %"), "{text}");
    assert!(text.contains("Rain: from 17:00 (2.4 mm over the next 24 hours)."), "{text}");
    assert!(text.contains("- 17:00 15 °C, rain, 1.2 mm (80 % chance of rain), wind 11 km/h"), "{text}");
    assert_eq!(text.lines().filter(|l| l.starts_with("- ") && l.as_bytes()[4] == b':').count(), 24, "{text}");
    assert!(text.contains("- Today, Thu 22 Oct: "), "{text}");
    assert!(text.contains("thunderstorms possible Fri 23 Oct"), "{text}");
    assert!(text.contains("heat Mon 26 Oct (up to 32 °C)"), "{text}");
    assert!(text.ends_with("Source: MET Norway (Norwegian Meteorological Institute), CC BY 4.0. Place search © OpenStreetMap contributors."));

    // MET was asked with 2 decimals and the application's User-Agent; so was the geocoder.
    let (query, headers, _) = fakes.met.last();
    assert_eq!(query, "/complete?lat=45.76&lon=4.83");
    let agent = headers["user-agent"].to_str().unwrap();
    assert!(agent.starts_with("BipAgents/") && agent.ends_with(" https://bipagents.com"), "{agent}");
    let (search, headers, _) = fakes.geocoder.last();
    assert_eq!(search, "/search?q=Lyon&format=jsonv2&limit=1&addressdetails=1");
    assert_eq!(headers["user-agent"].to_str().unwrap(), agent);

    // Asked again before MET's Expires: nothing sent anywhere (both answers kept).
    weather::call_at(&config, &args(json!({"place": " lyon "})), start + Duration::minutes(20)).await;
    assert_eq!((fakes.met.count(), fakes.geocoder.count()), (1, 1));
    // After it: asked again with If-Modified-Since, a 304 keeps the forecast.
    let later = weather::call_at(&config, &args(json!({"place": "Lyon"})), start + Duration::minutes(90)).await;
    assert_eq!(fakes.met.count(), 2);
    assert_eq!(fakes.met.last().1["if-modified-since"], "Thu, 22 Oct 2026 10:00:00 GMT");
    assert!(text_of(&later).contains("Now (13:00)"), "{}", text_of(&later));
    // And not again until the new Expires.
    weather::call_at(&config, &args(json!({"place": "Lyon"})), start + Duration::minutes(95)).await;
    assert_eq!(fakes.met.count(), 3, "the 304's Expires was already past here: a new conditional request");

    // Coordinates skip the geocoder; times stay in UTC there.
    let coords = weather::call_at(&config, &args(json!({"place": "45.7600, 4.8300", "days": 1})), start).await;
    assert!(text_of(&coords).contains("Times are UTC"), "{}", text_of(&coords));
    assert_eq!(fakes.geocoder.count(), 1);

    // Nothing found, nothing asked, the geocoder one request a second at most.
    let begun = Instant::now();
    let none = weather::call_at(&config, &args(json!({"place": "Nowhere at all"})), start).await;
    assert_eq!(none["isError"], true);
    assert!(text_of(&none).starts_with("no place found for « Nowhere at all »"));
    assert!(begun.elapsed() >= std::time::Duration::from_millis(1000) || fakes.geocoder.count() == 2);
    assert_eq!(weather::call_at(&config, &args(json!({})), start).await["isError"], true);
}

#[tokio::test]
async fn weather_is_listed_only_once_switched_on() {
    let start = at("2026-10-22T10:00:00Z");
    let fakes = fakes(start).await;
    let dir = tempfile::tempdir().unwrap();
    let config = configured(dir.path(), &fakes, false);
    let app = TestApp::new(config.clone());
    let names = tool_names(&app).await;
    assert!(names.contains(&"route".to_string()) && !names.contains(&"weather".to_string()), "{names:?}");
    let off = rpc(&app, "tools/call", json!({"name": "weather", "arguments": {"place": "Lyon"}})).await;
    assert_eq!(off["isError"], true);
    assert_eq!(fakes.geocoder.count(), 0);

    assert!(weather::set_enabled(&config, "wellness", true).unwrap());
    assert!(!weather::set_enabled(&config, "wellness", true).unwrap());
    assert_eq!(tool_names(&app).await.last().unwrap(), "weather");
    use std::os::unix::fs::PermissionsExt;
    let file = dir.path().join("data/weather/agents.json");
    assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
    let wellness = config.agent(Some("wellness")).unwrap();
    assert_ne!(agent_tools::version_of(&agent_tools::tools_for(&config, wellness)), agent_tools::tools_version());
    let on = rpc(&app, "tools/call", json!({"name": "weather", "arguments": {"place": "Lyon, France"}})).await;
    assert_eq!(on["isError"], false, "{on}");
    weather::forget(&config, "wellness");
    assert!(!weather::enabled(&config, "wellness"));
}

#[tokio::test]
async fn a_route_is_a_link_and_a_travel_time_with_a_key() {
    let fakes = fakes(at("2026-10-22T10:00:00Z")).await;
    let dir = tempfile::tempdir().unwrap();

    // Without a key: the link only, nothing sent.
    let config = configured(dir.path(), &fakes, false);
    let app = TestApp::new(config.clone());
    let plain = rpc(
        &app,
        "tools/call",
        json!({"name": "route", "arguments": {"destination": "Gare de Lyon, Paris",
                                                                                  "mode": "transit"}}),
    )
    .await;
    let text = plain["content"][0]["text"].as_str().unwrap();
    assert_eq!(plain["isError"], false);
    assert!(text
        .starts_with("Route to Gare de Lyon, Paris from where the person is when they open it by public transport."));
    assert!(text.contains("no routing service on this server"));
    assert!(text.ends_with(
        "\nhttps://www.google.com/maps/dir/?api=1&destination=Gare%20de%20Lyon%2C%20Paris&travelmode=transit"
    ));
    assert_eq!(fakes.google.count(), 0);

    // With a key: Google's travel time, with the traffic for a drive; the key never in the text.
    let config = configured(dir.path(), &fakes, true);
    let arguments = args(json!({"destination": "Versailles", "origin": "10 rue de la Paix, Paris",
                                "depart_at": "2026-10-23T08:30:00+02:00"}));
    let result = routes::call(weather::http(), &config, &arguments).await;
    let text = text_of(&result);
    assert!(text.contains("Travel time: 26 min with the traffic (20 min without), 18.2 km (Google Maps)."), "{text}");
    assert!(!text.contains("test-maps-key"));
    let (_, headers, body) = fakes.google.last();
    assert_eq!(headers["x-goog-fieldmask"], "routes.duration,routes.staticDuration,routes.distanceMeters");
    assert_eq!(
        body,
        json!({"origin": {"address": "10 rue de la Paix, Paris"}, "destination": {"address": "Versailles"},
               "travelMode": "DRIVE", "routingPreference": "TRAFFIC_AWARE", "units": "METRIC",
               "departureTime": "2026-10-23T06:30:00Z"})
    );
    // An arrival time for public transport; a refused key still gives the link.
    let transit = args(json!({"destination": "Versailles", "origin": "Paris", "mode": "transit",
                              "arrive_by": "2026-10-23T09:00:00Z"}));
    routes::call(weather::http(), &config, &transit).await;
    assert_eq!(fakes.google.last().2["arrivalTime"], "2026-10-23T09:00:00Z");
    assert!(fakes.google.last().2.get("routingPreference").is_none());
    let mut wrong = config.clone();
    wrong.maps.google_key = Some("revoked".into());
    let refused = text_of(&routes::call(weather::http(), &wrong, &transit).await);
    assert!(refused.contains("Travel time: not available (the routing service answered 403)"), "{refused}");
    assert!(refused.contains("https://www.google.com/maps/dir/?api=1&origin=Paris&destination=Versailles"));
    // Wrong arguments.
    let bad = routes::call(weather::http(), &config, &args(json!({"destination": "X", "mode": "plane"}))).await;
    assert_eq!(bad["isError"], true);
    let bad = routes::call(weather::http(), &config, &args(json!({"destination": "X", "depart_at": "demain"}))).await;
    assert_eq!(bad["isError"], true);
}
