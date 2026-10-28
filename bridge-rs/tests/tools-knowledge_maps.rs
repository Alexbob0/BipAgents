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
    let geocoder = spawn_fake(Router::new().route(
        "/search",
        get(|Query(q): Query<std::collections::HashMap<String, String>>| async move {
            let found = match q.get("q").map(String::as_str) {
                Some("Gare de Lyon, Paris") => json!([{"lat": "48.8443", "lon": "2.3743", "name": "Gare de Lyon",
                                                        "address": {"country_code": "fr", "country": "France"}}]),
                _ => json!([]),
            };
            axum::Json(found)
        }),
    ))
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
    let id = said.split("Map ready: ").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    assert!(id.starts_with("m-"), "{said}");
    assert!(
        said.contains(&format!("<img src=\"map:{id}\""))
            && said.contains(">Départ</li>")
            && said.contains(">Tour Eiffel</li>")
    );
    assert!(said.starts_with(&format!("MEDIA:/bipagents/maps/wellness/{id}.png\n")), "the MEDIA line first");
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
