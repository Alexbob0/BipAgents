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
    data["maps"] = json!({"tiles": "osm", "tile_url": format!("{tile_server}/{{z}}/{{x}}/{{y}}.png")});
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
    assert_eq!(map["route"], json!({"line": true, "mode": "walking"}));
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
