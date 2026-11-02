//! map_image (maps.rs): places found through the geocoder, OpenStreetMap tiles fetched once and kept, markers and the
//! attribution drawn, the picture put inline in a page. Against a fake Nominatim and a fake tile server.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use bipbridge::config::parse_config;
use bipbridge::{maps, pages};
use common::{config_dict, spawn_fake, Urls};
use serde_json::{json, Value};

fn tile_png(shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, 256, 256);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&vec![shade; 256 * 256 * 3]).unwrap();
    }
    out
}

fn text(result: &bipbridge::Dict) -> String {
    result["content"][0]["text"].as_str().unwrap_or("").to_string()
}

#[tokio::test]
async fn a_map_is_drawn_from_osm_tiles_kept_on_disk_and_put_inline_in_a_page() {
    let tiles = Arc::new(AtomicUsize::new(0));
    let agents = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (t, a) = (tiles.clone(), agents.clone());
    let tile_server = spawn_fake(Router::new().route(
        "/{z}/{x}/{y}",
        get(move |Path((_z, _x, _y)): Path<(u32, u32, String)>, headers: axum::http::HeaderMap| {
            let (t, a) = (t.clone(), a.clone());
            async move {
                t.fetch_add(1, Ordering::SeqCst);
                a.lock()
                    .unwrap()
                    .push(headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string());
                ([("content-type", "image/png")], tile_png(0xEE)).into_response()
            }
        }),
    ))
    .await;
    let geocoder = spawn_fake(
        Router::new()
            .route(
                "/search",
                get(|Query(q): Query<std::collections::HashMap<String, String>>| async move {
                    let found = match q.get("q").map(String::as_str) {
                        Some("Gare de Lyon, Paris") => {
                            json!([{"lat": "48.8443", "lon": "2.3743", "name": "Gare de Lyon",
                                                        "address": {"country_code": "fr", "country": "France"}}])
                        }
                        _ => json!([]),
                    };
                    axum::Json(found)
                }),
            )
            .route(
                "/reverse",
                get(|| async { axum::Json(json!({"lat": "48.8584", "lon": "2.2945", "display_name": "Paris"})) }),
            ),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["weather"] = json!({"geocoder_url": format!("{geocoder}/search")});
    data["maps"] = json!({"tiles": "osm", "tile_url": format!("{tile_server}/{{z}}/{{x}}/{{y}}.png")});
    let config = parse_config(&data, None).unwrap();
    let arguments = json!({"markers": [
        {"place": "Gare de Lyon, Paris", "label": "Départ", "color": "green"},
        {"place": "48.8584, 2.2945", "label": "Tour Eiffel"}], "width": 480, "height": 320});
    let result = maps::call(&config, "wellness", arguments.as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    let id = said.split("map:").nth(1).unwrap().split('"').next().unwrap().to_string();
    assert!(id.starts_with("m-"), "{said}");
    assert!(said.contains("1. Départ") && said.contains("2. Tour Eiffel"), "{said}");
    let map = maps::kept(&config, "wellness", &id).unwrap();
    assert_eq!(map["points"][1]["name"], "Tour Eiffel");
    assert_eq!((map["points"][1]["lat"].as_f64(), map["points"][1]["lon"].as_f64()), (Some(48.8584), Some(2.2945)));
    assert!(said.starts_with("MEDIA:/bipagents/pages/wellness/carte-"), "the map's page first: {said}");
    // one picture of the asked size, the tiles it needed fetched once, with the application's name
    let file = maps::file(&config, "wellness", &id).unwrap();
    let decoder = png::Decoder::new(std::io::Cursor::new(std::fs::read(&file).unwrap()));
    let info = decoder.read_info().unwrap().info().clone();
    assert_eq!((info.width, info.height), (480, 320));
    let fetched = tiles.load(Ordering::SeqCst);
    assert!((2..=maps::MAX_TILES).contains(&fetched), "{fetched}");
    assert!(agents.lock().unwrap().iter().all(|ua| ua.starts_with("BipAgents/")));
    // again: from the disk cache, nothing asked
    maps::call(&config, "wellness", arguments.as_object().unwrap()).await;
    assert_eq!(tiles.load(Ordering::SeqCst), fetched);
    // in a page, the picture goes inline: the page stays offline
    let source =
        format!("<h1>Trajet</h1><figure class=\"map-card\"><img src=\"map:{id}\"><ol><li>Départ</li></ol></figure>");
    pages::save(&config, "wellness", &source, "Trajet", None).unwrap();
    let kept = std::fs::read_to_string(pages::root(&config).join("wellness/trajet.html")).unwrap();
    assert!(kept.contains("<img src=\"data:image/png;base64,iVBOR") && !kept.contains("map:m-"));
    // an unknown place, a provider not supported, nothing asked: said plainly
    let none =
        maps::call(&config, "wellness", json!({"markers": [{"place": "Nulle part"}]}).as_object().unwrap()).await;
    assert!(none["isError"] == json!(true) && text(&none).contains("no place found for « Nulle part »"));
    let empty = maps::call(&config, "wellness", json!({}).as_object().unwrap()).await;
    assert_eq!(text(&empty), "No map: give markers or a center.");
    let mut other = data.clone();
    other["maps"]["tiles"] = Value::from("google");
    let refused = maps::call(&parse_config(&other, None).unwrap(), "wellness", arguments.as_object().unwrap()).await;
    assert!(text(&refused).contains("unknown map tiles « google »"));
}

/// A fake Nominatim knowing Lisbon and Paris: `search` (with `viewbox`/`bounded`) and `reverse` (nothing over the sea).
async fn fake_nominatim(asked: Arc<std::sync::Mutex<Vec<String>>>) -> String {
    use std::collections::HashMap;
    fn spot(lat: f64, lon: f64, name: &str, address: &str) -> Value {
        json!({"lat": lat.to_string(), "lon": lon.to_string(), "name": name, "display_name": address})
    }
    let a = asked.clone();
    let search = move |Query(q): Query<HashMap<String, String>>| {
        let a = a.clone();
        async move {
            let query = q.get("q").cloned().unwrap_or_default();
            a.lock().unwrap().push(format!("search {query} {}", q.get("viewbox").cloned().unwrap_or_default()));
            let bounded = q.get("bounded").is_some_and(|b| b == "1");
            let found = match query.as_str() {
                "Lisboa, Portugal" => json!([{"lat": "38.7077507", "lon": "-9.1365919", "name": "Lisboa",
                    "display_name": "Lisboa, Portugal",
                    "boundingbox": ["38.6913994", "38.7967584", "-9.2298356", "-9.0863328"]}]),
                "Torre de Belém" => {
                    json!([spot(38.6915837, -9.2159767, "Torre de Belém", "Torre de Belém, Belém, Lisboa, Portugal")])
                }
                "Mosteiro dos Jerónimos" => json!([spot(
                    38.6978,
                    -9.2067,
                    "Mosteiro dos Jerónimos",
                    "Mosteiro dos Jerónimos, Lisboa, Portugal"
                )]),
                "Castelo de São Jorge" => {
                    json!([spot(38.7139, -9.1334, "Castelo de São Jorge", "Castelo de São Jorge, Lisboa, Portugal")])
                }
                "Praça do Comércio" => {
                    json!([spot(38.7075, -9.1364, "Praça do Comércio", "Praça do Comércio, Lisboa, Portugal")])
                }
                // the same name elsewhere: only bounded to Lisbon does it come out there
                "Belém" if bounded => json!([spot(38.6976, -9.2063, "Belém", "Belém, Lisboa, Portugal")]),
                "Belém" => json!([spot(-1.4558, -48.5044, "Belém", "Belém, Pará, Brasil")]),
                "Tour Eiffel" => json!([spot(48.8582599, 2.2945006, "Tour Eiffel", "Tour Eiffel, Paris, France")]),
                // an address: Nominatim names it by its number
                "12 rue de Rivoli, Paris" => {
                    json!([spot(48.8556009, 2.3599124, "12", "12, Rue de Rivoli, Paris 4e, France")])
                }
                // a day out of Lisbon (25 km)
                "Palácio da Pena, Sintra" => {
                    json!([spot(38.7875680, -9.3905650, "Palácio da Pena", "Palácio da Pena, Sintra, Portugal")])
                }
                "Monte Saint-Michel" => {
                    json!([spot(48.6359, -1.5115, "Mont-Saint-Michel", "Mont-Saint-Michel, France")])
                }
                _ => json!([]),
            };
            axum::Json(found)
        }
    };
    let a = asked.clone();
    let reverse = move |Query(q): Query<HashMap<String, String>>| {
        let a = a.clone();
        async move {
            let lat: f64 = q.get("lat").and_then(|v| v.parse().ok()).unwrap_or(0.0);
            let lon: f64 = q.get("lon").and_then(|v| v.parse().ok()).unwrap_or(0.0);
            a.lock().unwrap().push(format!("reverse {lat} {lon}"));
            // land only around Paris and Lisbon here: everything else is the sea
            let land = (48.0..49.5).contains(&lat) && (1.5..3.5).contains(&lon)
                || (38.5..39.0).contains(&lat) && (-9.5..-9.0).contains(&lon);
            axum::Json(if land {
                json!({"lat": lat.to_string(), "lon": lon.to_string(), "name": "", "display_name": "Somewhere, on land"})
            } else {
                json!({"error": "Unable to geocode"})
            })
        }
    };
    spawn_fake(Router::new().route("/search", get(search)).route("/reverse", get(reverse))).await
}

async fn map_config(dir: &std::path::Path, geocoder: &str) -> bipbridge::config::Config {
    let tile_server = spawn_fake(
        Router::new()
            .route("/{z}/{x}/{y}", get(|| async { ([("content-type", "image/png")], tile_png(0xEE)).into_response() })),
    )
    .await;
    let mut data = config_dict(dir, &Urls::default());
    data["weather"] = json!({"geocoder_url": format!("{geocoder}/search")});
    // no street routing here (its own tests below, against a fake router): never the public one from the tests
    data["maps"] =
        json!({"tiles": "osm", "tile_url": format!("{tile_server}/{{z}}/{{x}}/{{y}}.png"), "streets": "off"});
    parse_config(&data, None).unwrap()
}

fn kept_map(config: &bipbridge::config::Config, agent: &str, said: &str) -> Value {
    let id = said.split("map:").nth(1).unwrap().split('"').next().unwrap();
    maps::kept(config, agent, id).expect("the map's data kept beside its picture")
}

#[tokio::test]
async fn places_are_found_by_the_bridge_inside_the_area_and_shown_as_a_live_map_page() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_nominatim(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    let arguments = json!({"title": "Lisbonne en 5 monuments", "area": "Lisboa, Portugal", "markers": [
        {"place": "Torre de Belém", "note": "10 h – 18 h"}, {"place": "Mosteiro dos Jerónimos"},
        {"place": "Belém", "label": "Quartier de Belém"}, {"place": "Castelo de São Jorge"},
        {"place": "Praça do Comércio"}], "line": true, "directions": "walking"});
    let result = maps::call(&config, "daily", arguments.as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    // the map shown at once as a page (a live map in the app), its short card for a bigger page
    let media = said.lines().next().unwrap();
    assert!(media.starts_with("MEDIA:/bipagents/pages/daily/carte-") && media.ends_with(".html"), "{said}");
    assert!(said.contains("1. Torre de Belém — Torre de Belém, Belém, Lisboa, Portugal"), "{said}");
    let map = kept_map(&config, "daily", &said);
    assert_eq!(map["type"], "map");
    assert_eq!(map["title"], "Lisbonne en 5 monuments");
    let points = map["points"].as_array().unwrap();
    assert_eq!(points.len(), 5);
    for p in points {
        let (lat, lon) = (p["lat"].as_f64().unwrap(), p["lon"].as_f64().unwrap());
        assert!((38.6..38.8).contains(&lat) && (-9.3..-9.0).contains(&lon), "{p} in Lisbon, lat then lon");
    }
    assert_eq!(points[2]["name"], "Quartier de Belém", "Belém bounded to Lisbon, not Brazil's");
    assert_eq!(points[0]["note"], "10 h – 18 h");
    assert!(points.iter().all(|p| p["source"] == "osm" && p["area"] == "Lisboa, Portugal"), "{points:?}");
    assert_eq!(map["area"], "Lisboa, Portugal");
    // routing off: the straight line, dashed, its length as the crow flies
    assert_eq!(map["route"]["mode"], "walking");
    assert_eq!((map["route"]["travel"].as_str(), map["route"]["streets"].as_bool()), (Some("walking"), Some(false)));
    assert!(map["route"].get("path").is_none() && map["route"]["meters"].as_u64().unwrap() > 6000);
    assert!(said.contains("Way: a dashed straight line, not along the streets (no street routing on this server)"));
    // every search after the area's stayed in its box
    let searches: Vec<String> = asked.lock().unwrap().iter().filter(|s| s.starts_with("search ")).cloned().collect();
    assert!(searches[0].starts_with("search Lisboa, Portugal"));
    assert!(searches[1..].iter().all(|s| s.ends_with("38.64140")), "{searches:?}");
    // the page holds the whole card, rebuilt from the data, with the places as the geocoder gave them
    let page = media.trim_start_matches("MEDIA:/bipagents/pages/daily/").trim_end_matches(".html");
    let kept = std::fs::read_to_string(pages::root(&config).join(format!("daily/{page}.html"))).unwrap();
    assert!(kept.contains("data-lat=\"38.691584\" data-lon=\"-9.215977\""), "Torre de Belém");
    assert!(kept.contains("class=\"map-data\"") && kept.contains("data:image/png;base64,"));
}

#[tokio::test]
async fn coordinates_from_the_model_are_checked_swapped_or_looked_up_by_name() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_nominatim(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    // GeoJSON's order (lon, lat) for the Eiffel tower: in the sea as given, on land the other way round
    let arguments = json!({"markers": [{"place": "2.2945, 48.8583", "label": "Tour Eiffel"},
        {"place": "48.8606, 2.3376", "label": "Louvre"},
        // coordinates « from memory » in the Atlantic: the label is looked up instead
        {"place": "38.30, -9.80", "label": "Torre de Belém"}]});
    let result = maps::call(&config, "daily", arguments.as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    let map = kept_map(&config, "daily", &said);
    let at = |i: usize| (map["points"][i]["lat"].as_f64().unwrap(), map["points"][i]["lon"].as_f64().unwrap());
    assert_eq!(at(0), (48.8583, 2.2945), "swapped back to lat, lon");
    assert_eq!(at(1), (48.8606, 2.3376), "on land as given: kept");
    assert_eq!(at(2), (38.691584, -9.215977), "looked up by its name");
    let sources: Vec<&str> = (0..3).map(|i| map["points"][i]["source"].as_str().unwrap()).collect();
    assert_eq!(sources, ["model", "model", "osm"]);
    assert!(said.contains("Check: ") && said.contains("was read as « lon, lat »") && said.contains("not on land"));
    // in the sea both ways and nothing to look up: said, not drawn
    let sea = maps::call(&config, "daily", json!({"markers": [{"place": "40.0, -30.0"}]}).as_object().unwrap()).await;
    assert!(sea["isError"] == json!(true) && text(&sea).contains("is not on land"), "{}", text(&sea));
    // inside an area, coordinates outside it are not taken either
    let off = maps::call(
        &config,
        "daily",
        json!({"area": "Lisboa, Portugal", "markers": [{"place": "-9.1393, 38.7223", "label": "Rossio"},
            {"place": "Praça do Comércio"}]})
        .as_object()
        .unwrap(),
    )
    .await;
    let map = kept_map(&config, "daily", &text(&off));
    assert_eq!((map["points"][0]["lat"].as_f64(), map["points"][0]["lon"].as_f64()), (Some(38.7223), Some(-9.1393)));
}

#[tokio::test]
async fn a_place_found_far_from_the_others_is_looked_up_again_near_them() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_nominatim(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    // no area: « Belém » alone is in Brazil, the four others in Lisbon
    let arguments = json!({"markers": [{"place": "Torre de Belém"}, {"place": "Mosteiro dos Jerónimos"},
        {"place": "Castelo de São Jorge"}, {"place": "Praça do Comércio"}, {"place": "Belém"}]});
    let result = maps::call(&config, "daily", arguments.as_object().unwrap()).await;
    let said = text(&result);
    let map = kept_map(&config, "daily", &said);
    assert_eq!((map["points"][4]["lat"].as_f64(), map["points"][4]["lon"].as_f64()), (Some(38.6976), Some(-9.2063)));
    assert!(said.contains("« Belém » was") && said.contains("looked up again near them"), "{said}");
}

#[tokio::test]
async fn a_page_keeps_the_maps_as_map_image_made_them_and_draws_none_of_its_own() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_nominatim(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    let result = maps::call(
        &config,
        "home",
        json!({"area": "Lisboa, Portugal", "markers": [{"place": "Torre de Belém"}, {"place": "Castelo de São Jorge"}]})
            .as_object()
            .unwrap(),
    )
    .await;
    let said = text(&result);
    let id = said.split("map:").nth(1).unwrap().split('"').next().unwrap().to_string();
    // « fix the map »: the model rewrites the card with positions of its own (in the sea): the bridge puts back its own
    let edited = format!(
        "<h1>Lisbonne</h1><figure class=\"map-card\"><img src=\"map:{id}\"><ol><li data-x=\"1\" data-y=\"1\" \
data-lat=\"-9.2160\" data-lon=\"38.6916\">Torre de Belém</li></ol></figure><p>Bonne visite</p>"
    );
    pages::save(&config, "home", &edited, "Lisbonne", None).unwrap();
    let kept = std::fs::read_to_string(pages::root(&config).join("home/lisbonne.html")).unwrap();
    assert!(!kept.contains("data-lat=\"-9.2160\""), "the model's positions dropped");
    assert!(kept.contains("data-lat=\"38.691584\" data-lon=\"-9.215977\"") && kept.contains("Castelo de São Jorge"));
    assert!(kept.contains("<p>Bonne visite</p>"), "the rest of the page as written");
    // the agent's own source is kept as written (read_page)
    assert!(std::fs::read_to_string(pages::root(&config).join("home/lisbonne.source")).unwrap().contains("-9.2160"));
    // a map of its own (a map library, or a card no map_image made): refused, with what to do
    for page in [
        "<div id=\"m\"></div><script>var map = L.map('m').setView([38.7, -9.1], 13);</script>".to_string(),
        "<script src=\"bipmap://map/lib.js\"></script><script>new maplibregl.Map({})</script>".to_string(),
        "<figure class=\"map-card\"><img src=\"map:m-0000000000\"><ol><li data-lat=\"1\" data-lon=\"2\">X</li></ol></figure>"
            .to_string(),
        "<figure class=\"map-card\"><ol><li data-lat=\"1\" data-lon=\"2\">X</li></ol></figure>".to_string(),
    ] {
        let refused = pages::save(&config, "home", &page, "Carte", None).unwrap_err().to_string();
        assert!(refused.contains("Call map_image with the places' names"), "{refused}");
    }
    // a page merely speaking of a leaflet is not a map
    pages::save(&config, "home", "<p>Le leaflet du musée</p>", "Musée", None).unwrap();
}

#[tokio::test]
async fn names_inside_an_area_are_found_by_their_first_part_a_day_out_and_addresses_keep_their_words() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_nominatim(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    let arguments = json!({"area": "Lisboa, Portugal", "markers": [
        {"place": "Torre de Belém, Avenida da Índia, 1400-206 Lisboa"}, // a wrong address after the name
        {"place": "Palácio da Pena, Sintra"},                            // outside the town, 25 km away
        {"place": "Monte Saint-Michel"}]}); // far away: left out, said
    let result = maps::call(&config, "daily", arguments.as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    let map = kept_map(&config, "daily", &said);
    let points = map["points"].as_array().unwrap();
    assert_eq!((points[0]["lat"].as_f64(), points[0]["source"].as_str()), (Some(38.691584), Some("osm")));
    assert_eq!((points[1]["lat"].as_f64(), points[1]["lon"].as_f64()), (Some(38.787568), Some(-9.390565)));
    assert!(said.contains("« Palácio da Pena, Sintra » is outside « Lisboa, Portugal » (2"), "{said}");
    // a place not found: in the data without coordinates (the apps may look it up), said to the agent
    assert_eq!(points[2], json!({"name": "Monte Saint-Michel", "source": "none", "area": "Lisboa, Portugal"}));
    assert!(said.contains("left out: no place found for « Monte Saint-Michel » in « Lisboa, Portugal »"), "{said}");
    // an address keeps its words, not the number the geocoder names it by
    let paris = maps::call(
        &config,
        "daily",
        json!({"markers": [{"place": "12 rue de Rivoli, Paris"}, {"place": "Tour Eiffel"}]}).as_object().unwrap(),
    )
    .await;
    let map = kept_map(&config, "daily", &text(&paris));
    assert_eq!(map["points"][0]["name"], "12 rue de Rivoli");
    assert_eq!(map["points"][1]["name"], "Tour Eiffel");
}

/// A fake Nominatim for one map's places kept together: several answers per name, in the geocoder's order, and
/// `viewbox` + `bounded=1` honoured (only what is inside the box). Coordinates as OpenStreetMap had them on 2026-11-01.
async fn fake_towns(asked: Arc<std::sync::Mutex<Vec<String>>>) -> String {
    use std::collections::HashMap;
    fn spot(lat: f64, lon: f64, name: &str, kind: &str, town: &str) -> Value {
        json!({"lat": lat.to_string(), "lon": lon.to_string(), "name": name, "category": kind,
               "display_name": format!("{name}, {town}"), "address": {"town": town, "country": "France"}})
    }
    let search = move |Query(q): Query<HashMap<String, String>>| {
        let a = asked.clone();
        async move {
            let query = q.get("q").cloned().unwrap_or_default();
            let viewbox = q.get("viewbox").cloned().unwrap_or_default();
            a.lock().unwrap().push(format!("search {query} {viewbox}"));
            let all = match query.as_str() {
                "Arcachon" => json!([{"lat": "44.6616428", "lon": "-1.1700018", "name": "Arcachon",
                    "display_name": "Arcachon, Gironde, France", "address": {"town": "Arcachon", "country": "France"},
                    "boundingbox": ["44.6387675", "44.6646925", "-1.2051148", "-1.1390026"]}]),
                "Église Notre-Dame-des-Passes" => {
                    json!([spot(44.6412126, -1.1990896, "Église Notre-Dame-des-Passes", "amenity", "Arcachon")])
                }
                "Jetée du Moulleau" => {
                    json!([spot(44.6417176, -1.2043897, "Jetée du Moulleau", "man_made", "Arcachon")])
                }
                "Plage du Moulleau" => json!([spot(44.6432, -1.2020, "Plage du Moulleau", "natural", "Arcachon")]),
                // the only one in Arcachon: downtown, 3 km from Le Moulleau
                "Avenue Gambetta, Camicas, Arcachon" => {
                    json!([spot(44.6577791, -1.1681525, "Avenue Gambetta", "highway", "Arcachon")])
                }
                "8 Avenue Gambetta, Arcachon" => json!([spot(44.6580, -1.1682, "8", "place", "Arcachon")]),
                // one downtown (the first answer), one in Le Moulleau
                "Rue des Pins" => json!([
                    spot(44.6605, -1.1750, "Rue des Pins", "highway", "Arcachon"),
                    spot(44.6401, -1.1979, "Rue des Pins", "highway", "Arcachon")
                ]),
                "Torre de Belém" => json!([spot(38.6915837, -9.2159767, "Torre de Belém", "man_made", "Lisboa")]),
                "Mosteiro dos Jerónimos" => {
                    json!([spot(38.6978, -9.2067, "Mosteiro dos Jerónimos", "amenity", "Lisboa")])
                }
                "Castelo de São Jorge" => {
                    json!([spot(38.7139, -9.1334, "Castelo de São Jorge", "historic", "Lisboa")])
                }
                "Universidade de Coimbra" => {
                    json!([spot(40.2073, -8.4260, "Universidade de Coimbra", "amenity", "Coimbra")])
                }
                "Torre dos Clérigos" => json!([spot(41.1458, -8.6139, "Torre dos Clérigos", "man_made", "Porto")]),
                "Rua das Flores, Porto" => json!([spot(41.1435, -8.6130, "Rua das Flores", "highway", "Porto")]),
                // in Coimbra first, in Lisbon too
                "Rua da Prata" => json!([
                    spot(40.2100, -8.4290, "Rua da Prata", "highway", "Coimbra"),
                    spot(38.7105, -9.1370, "Rua da Prata", "highway", "Lisboa")
                ]),
                _ => json!([]),
            };
            let bounded = q.get("bounded").is_some_and(|b| b == "1");
            // west, north, east, south
            let b: Vec<f64> = viewbox.split(',').filter_map(|v| v.parse().ok()).collect();
            let inside = |p: &Value| {
                let (lat, lon): (f64, f64) =
                    (p["lat"].as_str().unwrap().parse().unwrap(), p["lon"].as_str().unwrap().parse().unwrap());
                !bounded || (b.len() == 4 && (b[0]..=b[2]).contains(&lon) && (b[3]..=b[1]).contains(&lat))
            };
            axum::Json(Value::Array(all.as_array().unwrap().iter().filter(|p| inside(p)).cloned().collect()))
        }
    };
    spawn_fake(Router::new().route("/search", get(search))).await
}

/// The searches the fake was asked since the last time (and forgets them).
fn searches(asked: &Arc<std::sync::Mutex<Vec<String>>>) -> Vec<String> {
    std::mem::take(&mut *asked.lock().unwrap())
}

#[tokio::test]
async fn a_street_far_from_the_map_s_other_places_is_left_out_and_said_not_drawn_across_town() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_towns(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    // Alex, 2026-11-01: « Le Moulleau, le tour complet », its 4th stop downtown Arcachon, the line across the town
    let stops = |last: &str| {
        json!({"title": "Le Moulleau, le tour complet", "area": "Arcachon", "line": true, "markers": [
            {"place": "Église Notre-Dame-des-Passes"}, {"place": "Jetée du Moulleau"}, {"place": "Plage du Moulleau"},
            {"place": last}]})
    };
    let result = maps::call(&config, "daily", stops("Avenue Gambetta, Camicas, Arcachon").as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    assert!(said.contains("Map ready (3 markers"), "{said}");
    assert!(
        said.contains(
            "left out: could not place « Avenue Gambetta, Camicas, Arcachon » near the other places: the one found is 3."
        ),
        "{said}"
    );
    let map = kept_map(&config, "daily", &said);
    let points = map["points"].as_array().unwrap();
    for p in &points[..3] {
        assert!(p["lon"].as_f64().unwrap() < -1.19, "{p} in Le Moulleau");
    }
    assert_eq!(points[3], json!({"name": "Avenue Gambetta, Camicas, Arcachon", "source": "none", "area": "Arcachon"}));
    assert_eq!(map["route"]["line"], json!(true), "the line through the three stops only");
    // few requests: the area, the three names, the street near them then in the town (no second look near them)
    let done = searches(&asked);
    assert_eq!(done.len(), 6, "{done:?}");
    // the same without an area: the same answer
    let mut alone = stops("Avenue Gambetta, Camicas, Arcachon");
    alone.as_object_mut().unwrap().remove("area");
    let said = text(&maps::call(&config, "daily", alone.as_object().unwrap()).await);
    assert!(said.contains("Map ready (3 markers") && said.contains("left out: could not place"), "{said}");
    // the agent insists with the address's number: drawn where it is, with a word
    let said = text(&maps::call(&config, "daily", stops("8 Avenue Gambetta, Arcachon").as_object().unwrap()).await);
    assert!(said.contains("Map ready (4 markers") && !said.contains("left out"), "{said}");
    assert!(said.contains("« 8 Avenue Gambetta, Arcachon » is 3.") && said.contains("check it"), "{said}");
}

#[tokio::test]
async fn an_ambiguous_street_on_a_tour_is_taken_near_the_tour_s_other_places() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_towns(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    // « Rue des Pins »: the geocoder's first is downtown, inside the town's box; the one of the tour is in Le Moulleau
    let arguments = json!({"area": "Arcachon", "markers": [{"place": "Église Notre-Dame-des-Passes"},
        {"place": "Rue des Pins", "label": "Déjeuner rue des Pins"}, {"place": "Jetée du Moulleau"}]});
    let said = text(&maps::call(&config, "daily", arguments.as_object().unwrap()).await);
    assert!(said.contains("Map ready (3 markers") && !said.contains("Check:"), "{said}");
    let map = kept_map(&config, "daily", &said);
    assert_eq!((map["points"][1]["lat"].as_f64(), map["points"][1]["lon"].as_f64()), (Some(44.6401), Some(-1.1979)));
    assert_eq!(map["points"][1]["name"], "Déjeuner rue des Pins");
    // a Lisbon walk without an area, « Rua da Prata » first found in Coimbra
    let arguments = json!({"markers": [{"place": "Torre de Belém"}, {"place": "Castelo de São Jorge"},
        {"place": "Rua da Prata"}]});
    let said = text(&maps::call(&config, "daily", arguments.as_object().unwrap()).await);
    let map = kept_map(&config, "daily", &said);
    assert_eq!((map["points"][2]["lat"].as_f64(), map["points"][2]["lon"].as_f64()), (Some(38.7105), Some(-9.137)));
}

#[tokio::test]
async fn a_trip_far_apart_on_purpose_and_a_single_place_stay_as_found() {
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let geocoder = fake_towns(asked.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = map_config(dir.path(), &geocoder).await;
    // Lisbon → Coimbra → Porto, a street in Porto named with its town
    let arguments = json!({"line": true, "directions": "driving", "markers": [{"place": "Torre de Belém"},
        {"place": "Mosteiro dos Jerónimos"}, {"place": "Universidade de Coimbra"}, {"place": "Torre dos Clérigos"},
        {"place": "Rua das Flores, Porto"}]});
    let said = text(&maps::call(&config, "daily", arguments.as_object().unwrap()).await);
    assert!(said.contains("Map ready (5 markers") && !said.contains("Check:"), "{said}");
    let map = kept_map(&config, "daily", &said);
    assert_eq!((map["points"][4]["lat"].as_f64(), map["points"][4]["lon"].as_f64()), (Some(41.1435), Some(-8.613)));
    assert_eq!((map["points"][2]["lat"].as_f64(), map["points"][2]["lon"].as_f64()), (Some(40.2073), Some(-8.426)));
    // one place: as found, one request
    searches(&asked);
    let one = json!({"markers": [{"place": "Avenue Gambetta, Camicas, Arcachon"}]});
    let said = text(&maps::call(&config, "daily", one.as_object().unwrap()).await);
    assert!(said.contains("Map ready (1 markers") && !said.contains("Check:"), "{said}");
    let map = kept_map(&config, "daily", &said);
    assert_eq!((map["points"][0]["lat"].as_f64(), map["points"][0]["source"].as_str()), (Some(44.657779), Some("osm")));
    assert_eq!(searches(&asked).len(), 1);
}

// --- the way along the streets (map_route.rs, 2026-11-01) -----------------------------------------------------------

/// FOSSGIS's public router, by hand only (`cargo test --test tools-knowledge_maps -- --ignored live_`): Le Moulleau's
/// walk and a Lisbon walk, one request each, to check the answer's shape. Prints only the stops' coordinates and the
/// distance; `MAP_ROUTE_OUT=<dir>` keeps each way's data there (the iPhone's design screen, MapRouteDesign.swift).
#[tokio::test]
#[ignore]
async fn live_fossgis_walks_in_le_moulleau_and_lisbon() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = config_dict(dir.path(), &Urls::default());
    data["maps"] = json!({"streets": "osrm", "streets_timeout": 10});
    let config = parse_config(&data, None).unwrap();
    let walks = [
        ("moulleau", vec![(44.6412126, -1.1990896), (44.6417176, -1.2043897), (44.6432, -1.2020), (44.6401, -1.1979)]),
        ("lisbon", vec![(38.7139, -9.1334), (38.71182, -9.13001), (38.70763, -9.13652), (38.707, -9.1459)]),
    ];
    for (name, stops) in walks {
        let way = bipbridge::map_route::way(&config, "walking", &stops).await.unwrap();
        let straight = bipbridge::map_route::straight_km(&stops) * 1000.0;
        println!("{name}: stops {stops:?} → {:.0} m along the streets ({straight:.0} m straight)", way.meters);
        assert!(way.path.len() >= 2 && way.path.len() <= bipbridge::map_route::MAX_PATH, "{}", way.path.len());
        assert!(way.meters > straight && way.meters < straight * 3.0, "{} m", way.meters);
        // the way starts and ends by the first and last stops (within a street's width of them)
        assert!(maps::km(way.path[0], stops[0]) < 0.1 && maps::km(way.path[way.path.len() - 1], stops[3]) < 0.1);
        if let Ok(out) = std::env::var("MAP_ROUTE_OUT") {
            let kept = json!({"stops": stops, "path": way.path, "meters": way.meters, "seconds": way.seconds});
            std::fs::write(format!("{out}/{name}.json"), kept.to_string()).unwrap();
        }
    }
}

/// What the fake router was asked: each request's address and User-Agent.
type Requests = Arc<std::sync::Mutex<Vec<(String, String)>>>;

/// A fake OSRM like FOSSGIS's (`/<profile>/route|trip/v1/driving/<lon,lat;…>`): the way from stop to stop goes north
/// or south then west or east (an L: one corner a hop), a point every ~5 m wiggling by less than a metre, 1.3 times
/// as long as the straight line at 4.5 km/h; `trip` reverses the stops between the first and the last. `delay` before
/// answering.
async fn fake_osrm(asked: Requests, delay: std::time::Duration) -> String {
    let answer = move |Path((profile, service, p, coords)): Path<(String, String, String, String)>,
                       axum::extract::RawQuery(query): axum::extract::RawQuery,
                       headers: axum::http::HeaderMap| {
        let a = asked.clone();
        async move {
            let ua = headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            a.lock().unwrap().push((format!("/{profile}/{service}/v1/{p}/{coords}?{}", query.unwrap_or_default()), ua));
            tokio::time::sleep(delay).await;
            let stops: Vec<(f64, f64)> = coords
                .split(';')
                .map(|c| {
                    let (lon, lat) = c.split_once(',').unwrap();
                    (lat.parse().unwrap(), lon.parse().unwrap())
                })
                .collect();
            if service == "trip" {
                let n = stops.len();
                let places: Vec<Value> = (0..n)
                    .map(|i| json!({"waypoint_index": if i == 0 || i == n - 1 { i } else { n - 1 - i }}))
                    .collect();
                return axum::Json(json!({"code": "Ok", "waypoints": places, "trips": [{}]}));
            }
            let mut line: Vec<[f64; 2]> = Vec::new();
            for pair in stops.windows(2) {
                let ((la1, lo1), (la2, lo2)) = (pair[0], pair[1]);
                let corner = (la2, lo1);
                for (from, to) in [((la1, lo1), corner), (corner, (la2, lo2))] {
                    let steps = (maps::km(from, to) * 200.0).ceil().max(1.0) as usize;
                    for k in 0..steps {
                        let t = k as f64 / steps as f64;
                        let wiggle = if k % 2 == 0 { 0.0 } else { 0.000004 };
                        line.push([from.1 + (to.1 - from.1) * t + wiggle, from.0 + (to.0 - from.0) * t]);
                    }
                }
            }
            let last = stops[stops.len() - 1];
            line.push([last.1, last.0]);
            let straight: f64 = stops.windows(2).map(|p| maps::km(p[0], p[1])).sum::<f64>() * 1000.0;
            let meters = straight * 1.3;
            axum::Json(json!({"code": "Ok", "routes": [{"distance": meters, "duration": meters / 1.25,
                "geometry": {"type": "LineString", "coordinates": line}}], "waypoints": []}))
        }
    };
    spawn_fake(Router::new().route("/{profile}/{service}/v1/{p}/{coords}", get(answer))).await
}

async fn route_config(dir: &std::path::Path, geocoder: &str, osrm: &str, timeout: f64) -> bipbridge::config::Config {
    let tile_server = spawn_fake(
        Router::new()
            .route("/{z}/{x}/{y}", get(|| async { ([("content-type", "image/png")], tile_png(0xEE)).into_response() })),
    )
    .await;
    let mut data = config_dict(dir, &Urls::default());
    data["weather"] = json!({"geocoder_url": format!("{geocoder}/search")});
    data["maps"] = json!({"tiles": "osm", "tile_url": format!("{tile_server}/{{z}}/{{x}}/{{y}}.png"),
                          "streets": "osrm", "streets_url": osrm, "streets_timeout": timeout});
    parse_config(&data, None).unwrap()
}

/// Le Moulleau's walk (Alex, 2026-11-01): the church, the pier, the beach, lunch rue des Pins.
fn moulleau(extra: Value) -> Value {
    let mut arguments = json!({"title": "Le Moulleau à pied", "area": "Arcachon", "line": true, "markers": [
        {"place": "Église Notre-Dame-des-Passes"}, {"place": "Jetée du Moulleau"}, {"place": "Plage du Moulleau"},
        {"place": "Rue des Pins", "label": "Déjeuner rue des Pins"}]});
    for (k, v) in extra.as_object().unwrap() {
        arguments[k] = v.clone();
    }
    arguments
}

#[tokio::test]
async fn a_walk_is_drawn_along_the_streets_with_its_length_and_time_asking_only_coordinates() {
    let geocoder = fake_towns(Arc::new(std::sync::Mutex::new(Vec::new()))).await;
    let asked: Requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let osrm = fake_osrm(asked.clone(), std::time::Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let config = route_config(dir.path(), &geocoder, &osrm, 3.0).await;
    let result = maps::call(&config, "daily", moulleau(json!({})).as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "{said}");
    // under 3 km and no way said: on foot; the stops in the agent's order
    assert!(
        said.contains("Way: ") && said.contains("on foot, about") && said.contains("stops in your order"),
        "{said}"
    );
    let map = kept_map(&config, "daily", &said);
    let route = &map["route"];
    assert_eq!((route["travel"].as_str(), route["streets"].as_bool()), (Some("walking"), Some(true)), "{route}");
    assert_eq!(route["attribution"], bipbridge::map_route::ATTRIBUTION);
    let names: Vec<&str> = map["points"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["Église Notre-Dame-des-Passes", "Jetée du Moulleau", "Plage du Moulleau", "Déjeuner rue des Pins"]
    );
    // the wiggles gone, the corners kept: each hop is two straight bits (the stops, the corners)
    let path = route["path"].as_array().unwrap();
    assert_eq!(path.len(), 7, "{path:?}");
    assert_eq!(path[0], json!([44.64121, -1.19909]), "lat first, from the first stop");
    let (meters, seconds) = (route["meters"].as_u64().unwrap(), route["seconds"].as_u64().unwrap());
    assert!((1000..2500).contains(&meters) && seconds.abs_diff((meters as f64 / 1.25) as u64) <= 1, "{route}");
    // one request, from the bridge with the application's name, holding the stops' coordinates and nothing else
    let requests = asked.lock().unwrap().clone();
    assert_eq!(requests.len(), 1, "{requests:?}");
    let (url, ua) = &requests[0];
    assert!(ua.starts_with("BipAgents/"), "{ua}");
    assert!(url.starts_with("/routed-foot/route/v1/driving/-1.19909,44.64121;-1.20439,44.64172;"), "{url}");
    assert!(url.ends_with("?overview=full&geometries=geojson&steps=false&alternatives=false"), "{url}");
    let coords = url.split("/driving/").nth(1).unwrap().split('?').next().unwrap();
    assert!(coords.chars().all(|c| c.is_ascii_digit() || ".,;-".contains(c)), "only numbers: {coords}");
    // the same walk again: from the bridge's cache, nothing asked
    maps::call(&config, "daily", moulleau(json!({})).as_object().unwrap()).await;
    assert_eq!(asked.lock().unwrap().len(), 1);
    // by bike, said by the agent: its own profile
    let said = text(&maps::call(&config, "daily", moulleau(json!({"travel": "bicycling"})).as_object().unwrap()).await);
    assert!(said.contains("by bike, about"), "{said}");
    assert!(asked.lock().unwrap()[1].0.starts_with("/routed-bike/route/v1/"));
    // places that do not follow each other (no line): never routed
    let said = text(&maps::call(&config, "daily", moulleau(json!({"line": false})).as_object().unwrap()).await);
    assert!(!said.contains("Way:") && kept_map(&config, "daily", &said)["route"].is_null(), "{said}");
    assert_eq!(asked.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_router_too_slow_or_stops_too_far_apart_leave_the_straight_line_dashed() {
    let geocoder = fake_towns(Arc::new(std::sync::Mutex::new(Vec::new()))).await;
    let asked: Requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    // a router that would answer in a minute: the map is not kept waiting (the geocoder's turns make the whole call's
    // time no measure of it, its words are)
    let osrm = fake_osrm(asked.clone(), std::time::Duration::from_secs(60)).await;
    let dir = tempfile::tempdir().unwrap();
    let config = route_config(dir.path(), &geocoder, &osrm, 0.3).await;
    let result = maps::call(&config, "daily", moulleau(json!({})).as_object().unwrap()).await;
    let said = text(&result);
    assert_eq!(result["isError"], json!(false), "the map all the same: {said}");
    assert!(
        said.contains("Way: a dashed straight line, not along the streets (the street routing did not answer in time)"),
        "{said}"
    );
    let route = kept_map(&config, "daily", &said)["route"].clone();
    assert_eq!(
        (route["streets"].as_bool(), route.get("path"), route.get("seconds")),
        (Some(false), None, None),
        "{route}"
    );
    assert!((500..1500).contains(&route["meters"].as_u64().unwrap()), "as the crow flies: {route}");
    assert_eq!(asked.lock().unwrap().len(), 1);
    // Lisbon to Porto on foot: too far, nothing asked
    let trip = json!({"line": true, "travel": "walking", "markers": [{"place": "Torre de Belém"},
        {"place": "Torre dos Clérigos"}]});
    let said = text(&maps::call(&config, "daily", trip.as_object().unwrap()).await);
    assert!(said.contains("is too far to go on foot)"), "{said}");
    assert_eq!(kept_map(&config, "daily", &said)["route"]["streets"], json!(false));
    // public transport: not routed either
    let said =
        text(&maps::call(&config, "daily", moulleau(json!({"directions": "transit"})).as_object().unwrap()).await);
    assert!(said.contains("public transport is not routed here"), "{said}");
    assert_eq!(asked.lock().unwrap().len(), 1, "nothing more asked");
}

#[tokio::test]
async fn the_stops_are_reordered_only_when_the_agent_asks() {
    let geocoder = fake_towns(Arc::new(std::sync::Mutex::new(Vec::new()))).await;
    let asked: Requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let osrm = fake_osrm(asked.clone(), std::time::Duration::ZERO).await;
    let dir = tempfile::tempdir().unwrap();
    let config = route_config(dir.path(), &geocoder, &osrm, 3.0).await;
    let said = text(&maps::call(&config, "daily", moulleau(json!({"reorder": true})).as_object().unwrap()).await);
    assert!(said.contains("stops reordered for the shortest way, first and last kept"), "{said}");
    // the fake's shortest order: the two in between swapped; the numbers follow it
    assert!(said.contains("2. Plage du Moulleau") && said.contains("3. Jetée du Moulleau"), "{said}");
    let map = kept_map(&config, "daily", &said);
    assert_eq!(map["points"][1]["name"], "Plage du Moulleau");
    assert_eq!(map["route"]["path"][0], json!([44.64121, -1.19909]));
    let requests: Vec<String> = asked.lock().unwrap().iter().map(|(u, _)| u.clone()).collect();
    assert!(requests[0].starts_with("/routed-foot/trip/v1/driving/"), "{requests:?}");
    assert!(requests[0].contains("source=first&destination=last&roundtrip=false"), "{requests:?}");
    // the way asked in the new order: the beach second
    assert!(
        requests[1].starts_with("/routed-foot/route/v1/driving/-1.19909,44.64121;-1.20200,44.64320;"),
        "{requests:?}"
    );
}
