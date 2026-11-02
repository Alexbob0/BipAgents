//! Maps for the agents' pages and replies (owner-approved 2026-10-22, OpenStreetMap only): the pages stay offline (their
//! policy has no network), so the bridge draws the map. `map_image` finds the places through the weather tool's
//! Nominatim client (its User-Agent and its one-request-per-second turn), fetches the OpenStreetMap raster tiles itself
//! within the tile usage policy (a User-Agent naming the application, at most 2 requests a second, a disk cache kept
//! 30 days under the data dir, only the tiles one map needs, never more than `MAX_TILES`), stitches them, draws numbered
//! markers and the required « © OpenStreetMap contributors » on the picture, and keeps it as a PNG: a `MEDIA:` file
//! for a reply, or `src="map:<id>"` in a page (pages.rs puts it inline as a data: URI when the page is kept).
//! `[maps] tiles = "osm"` (the only provider for now; a paid one can come later) and `tile_url` (tests: a fake).
//!
//! Since 2026-10-23 the card it gives also carries each place's position (`data-lat`, `data-lon`), an optional line
//! through them and the travel mode for directions: inside the apps the page kit turns the picture into a live map
//! (map_tiles.rs) with « Open in Maps » links; anywhere else (an export, a browser) the picture stays.
//!
//! Not a Python module: Rust bridge only (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::config::Config;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "map_image";
/// How the app asks for them (`GET /v1/media?path=…`).
pub const PREFIX: &str = "/bipagents/maps";
pub const ATTRIBUTION: &str = "© OpenStreetMap contributors";
pub const MAX_TILES: usize = 30;
const TILE: f64 = 256.0;
/// Between two tile requests (the policy: no heavy use; 2 per second at most here).
const TILE_GAP: Duration = Duration::from_millis(500);
const TILE_DAYS: u64 = 30;
const MAX_MARKERS: usize = 12;
/// How the directions link travels (the apps turn it into Apple Maps' or Google Maps' own words).
pub const MODES: [&str; 4] = ["walking", "driving", "transit", "bicycling"];
static TILE_TURN: LazyLock<tokio::sync::Mutex<Option<Instant>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "A live map in the app (move, zoom, « Open in Maps » per place): use it whenever the person asks \
    for a map, a « carte », places to see, a tour or a way somewhere; never generate an image of a map, never draw one \
    in a page. Give each place by its NAME (in the local language when you know it: « Castelo de São Jorge ») or its \
    ADDRESS (« 12 rue de Rivoli »), and area (the town): \
    the bridge finds the exact spots itself. Never write coordinates from memory (they land in the sea); « lat, lon » \
    only when the person or a tool gave them (they are checked). The result gives a MEDIA: line (the map, shown in your \
    reply) and a short card to put in a bigger create_page page. line: a walk, a ride or a trip: the markers are its \
    stops in your order, joined by the way along the streets with its length and time (travel: how; reorder: only \
    when the person wants the shortest order). Not for places that do not follow each other. directions: a button for \
    the way from the first marker to the last. To move a place, call again.",
        "inputSchema": {"type": "object", "properties": {
            "markers": {"type": "array", "maxItems": MAX_MARKERS, "items": {"type": "object", "properties": {
                "place": {"type": "string", "description": "its name or address as typed in a map search; not coordinates"},
                "label": {"type": "string", "description": "what the list says for it (default: the name found)"},
                "note": {"type": "string", "description": "a short line under it (hours, why go)"},
                "color": {"type": "string", "description": "red, blue, green, orange, purple, teal, yellow, pink or #rrggbb"}},
                "required": ["place"]}},
            "area": {"type": "string", "description": "the town or region all the places are in (« Lisbonne, Portugal »): the search stays there"},
            "title": {"type": "string", "description": "the map's title (« Lisbonne en 5 monuments »)"},
            "center": {"type": "string", "description": "without markers: the place the map shows"},
            "zoom": {"type": "integer", "minimum": 3, "maximum": 18, "description": "default: what fits the markers"},
            "line": {"type": "boolean", "description": "an itinerary: the way through the markers along the streets, in \
    their order"},
            "travel": {"type": "string", "enum": crate::map_route::TRAVELS, "description": "with line: on foot \
    (walking: the default for a walk in a town), by bike or by car (default: on foot under ~3 km, else by car)"},
            "reorder": {"type": "boolean", "description": "with line: put the stops between the first and the last in \
    the shortest order (only when asked; default: your order)"},
            "directions": {"type": "string", "enum": MODES, "description": "a directions button, first marker to last"},
            "width": {"type": "integer", "minimum": 200, "maximum": 800},
            "height": {"type": "integer", "minimum": 150, "maximum": 800}}},
    })
}

fn root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-maps")
}

fn tiles_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("map-tiles")
}

/// The kept map `id` of `agent` (a PNG), if any.
pub fn file(config: &Config, agent: &str, id: &str) -> Option<PathBuf> {
    let ok = !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let path = root(config).join(agent).join(format!("{id}.png"));
    (ok && path.is_file()).then_some(path)
}

pub fn media_root(config: &Config) -> PathBuf {
    root(config)
}

/// A deleted agent: its maps go with it.
pub fn forget(config: &Config, agent: &str) {
    let _ = std::fs::remove_dir_all(root(config).join(agent));
}

// --- the projection -------------------------------------------------------------------------------------------------

/// World pixel coordinates at zoom `z` (Web Mercator, 256 px tiles).
pub fn pixel(lat: f64, lon: f64, z: u32) -> (f64, f64) {
    let scale = TILE * 2f64.powi(z as i32);
    let lat = lat.clamp(-85.0511, 85.0511).to_radians();
    let x = (lon + 180.0) / 360.0 * scale;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * scale;
    (x, y)
}

/// The highest zoom (≤ 17) at which every point fits in `w` × `h` with a margin.
pub fn fit(points: &[(f64, f64)], w: u32, h: u32) -> u32 {
    if points.len() < 2 {
        return 15;
    }
    for z in (3..=17).rev() {
        let px: Vec<(f64, f64)> = points.iter().map(|(la, lo)| pixel(*la, *lo, z)).collect();
        let (min_x, max_x) = px.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
        let (min_y, max_y) = px.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
        if max_x - min_x <= w as f64 - 80.0 && max_y - min_y <= h as f64 - 80.0 {
            return z;
        }
    }
    3
}

// --- tiles ----------------------------------------------------------------------------------------------------------

async fn tile(config: &Config, z: u32, x: i64, y: i64) -> Result<Vec<u8>, String> {
    let n = 1i64 << z;
    let x = x.rem_euclid(n);
    if !(0..n).contains(&y) {
        return Ok(Vec::new()); // beyond the poles: left blank
    }
    let path = tiles_dir(config).join(format!("{z}/{x}/{y}.png"));
    let fresh = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(TILE_DAYS * 86400));
    if fresh {
        if let Ok(data) = std::fs::read(&path) {
            return Ok(data);
        }
    }
    let url = config
        .maps
        .tile_url
        .replace("{z}", &z.to_string())
        .replace("{x}", &x.to_string())
        .replace("{y}", &y.to_string());
    let mut turn = TILE_TURN.lock().await;
    if let Some(last) = *turn {
        if last.elapsed() < TILE_GAP {
            tokio::time::sleep(TILE_GAP - last.elapsed()).await;
        }
    }
    let answer = crate::weather::http().get(&url).send().await;
    *turn = Some(Instant::now());
    drop(turn);
    let response = answer.map_err(|_| "the map tiles did not answer".to_string())?;
    if response.status().as_u16() != 200 {
        // an old copy is better than nothing
        return std::fs::read(&path).map_err(|_| format!("the map tiles answered {}", response.status().as_u16()));
    }
    let data = response.bytes().await.map_err(|_| "the map tiles answered something unreadable".to_string())?.to_vec();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        let _ = tk::replace_with(&path.with_extension("part"), &path, &data);
    }
    Ok(data)
}

/// A tile's pixels as RGBA, 256 × 256 (None: unreadable).
fn decode(data: &[u8]) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buffer).ok()?;
    if info.width != 256 || info.height != 256 {
        return None;
    }
    let pixels = &buffer[..info.buffer_size()];
    Some(match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => pixels.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|g| [*g, *g, *g, 255]).collect(),
        png::ColorType::Indexed => return None,
    })
}

// --- drawing --------------------------------------------------------------------------------------------------------

struct Canvas {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Canvas { w, h, rgba: [0xE8, 0xE6, 0xE1, 0xFF].repeat((w * h) as usize) }
    }

    fn blend(&mut self, x: i64, y: i64, c: [u8; 3], alpha: f64) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        for (dot, ink) in self.rgba[i..i + 3].iter_mut().zip(c) {
            *dot = (*dot as f64 * (1.0 - alpha) + ink as f64 * alpha).round() as u8;
        }
    }

    fn disc(&mut self, cx: f64, cy: f64, r: f64, c: [u8; 3]) {
        for y in (cy - r - 1.0).floor() as i64..=(cy + r + 1.0).ceil() as i64 {
            for x in (cx - r - 1.0).floor() as i64..=(cx + r + 1.0).ceil() as i64 {
                let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
                let a = (r + 0.5 - d).clamp(0.0, 1.0);
                if a > 0.0 {
                    self.blend(x, y, c, a);
                }
            }
        }
    }

    /// A thick line from one point to another (discs along it; good enough for a few segments).
    fn segment(&mut self, from: (f64, f64), to: (f64, f64), r: f64, c: [u8; 3]) {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let steps = (length / (r / 2.0)).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            self.disc(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t, r, c);
        }
    }

    /// The same, in dashes (`dash` px drawn, `gap` px left) carried on from `phase` (where the last segment's dash
    /// stopped); the new phase.
    fn dashed(
        &mut self,
        from: (f64, f64),
        to: (f64, f64),
        r: f64,
        c: [u8; 3],
        (dash, gap): (f64, f64),
        phase: f64,
    ) -> f64 {
        let length = ((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt();
        let mut at = 0.0;
        let mut phase = phase % (dash + gap);
        while at < length {
            let (on, left) = if phase < dash { (true, dash - phase) } else { (false, dash + gap - phase) };
            let end = (at + left).min(length);
            if on {
                let point = |t: f64| (from.0 + (to.0 - from.0) * t / length, from.1 + (to.1 - from.1) * t / length);
                self.segment(point(at), point(end), r, c);
            }
            phase = (phase + end - at) % (dash + gap);
            at = end;
        }
        phase
    }

    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, c: [u8; 3], alpha: f64) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.blend(xx, yy, c, alpha);
            }
        }
    }

    /// Text in the small bitmap font, `scale` pixels per dot; its width.
    fn text(&mut self, x: i64, y: i64, text: &str, scale: i64, c: [u8; 3]) -> i64 {
        let mut pen = x;
        for ch in text.chars() {
            if let Some(rows) = glyph(ch) {
                for (r, bits) in rows.iter().enumerate() {
                    for col in 0..5 {
                        if bits & (0x10 >> col) != 0 {
                            self.rect(pen + col * scale, y + r as i64 * scale, scale, scale, c, 1.0);
                        }
                    }
                }
            }
            pen += 6 * scale;
        }
        pen - x
    }

    fn png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.w, self.h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            if let Ok(mut writer) = encoder.write_header() {
                let _ = writer.write_image_data(&self.rgba);
            }
        }
        out
    }
}

/// 5 × 7 dots: the digits and the letters of the attribution.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        'b' => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E],
        'c' => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],
        'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        'i' => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],
        'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        'p' => [0x00, 0x00, 0x1E, 0x11, 0x1E, 0x10, 0x10],
        'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        's' => [0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E],
        't' => [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],
        'u' => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],
        '©' => [0x0E, 0x11, 0x17, 0x19, 0x17, 0x11, 0x0E],
        _ => return None,
    })
}

const PALETTE: [(&str, [u8; 3]); 8] = [
    ("red", [0xE5, 0x48, 0x4D]),
    ("blue", [0x4F, 0x7C, 0xFF]),
    ("green", [0x1D, 0x8A, 0x4F]),
    ("orange", [0xFF, 0x7B, 0x5C]),
    ("purple", [0x8E, 0x6C, 0xF5]),
    ("teal", [0x0D, 0x7F, 0x8B]),
    ("yellow", [0xD8, 0x9A, 0x00]),
    ("pink", [0xC2, 0x37, 0x74]),
];

fn color(name: &str, index: usize) -> [u8; 3] {
    let name = name.trim().to_lowercase();
    if let Some(hex) = name.strip_prefix('#').filter(|h| h.len() == 6) {
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            return [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        }
    }
    PALETTE.iter().find(|(n, _)| *n == name).map(|(_, c)| *c).unwrap_or(PALETTE[index % PALETTE.len()].1)
}

// --- the tool -------------------------------------------------------------------------------------------------------

struct Marker {
    label: String,
    address: String,
    area: String,
    source: &'static str,
    note: String,
    lat: f64,
    lon: f64,
    color: [u8; 3],
    /// A street (or found as one): left out rather than drawn far from the others.
    street: bool,
}

// --- finding the places ---------------------------------------------------------------------------------------------
//
// The model never places a marker from memory (2026-10-31: GLM-5.3 Flash wrote Lisbon's monuments as coordinates of
// its own and they fell in the sea, twice). Names and addresses are looked up by the bridge (Nominatim, its policy:
// weather::nominatim), inside the `area` when one is given. Coordinates are only taken once checked: inside the area,
// or on land (a reverse lookup finds something there); else the other order (« lon, lat », GeoJSON's) when that one
// passes; else the marker's label is looked up. Last, a map's places are kept together (`strays`): a place far from all
// the others is looked up again near them, and a street still far from them is left out and said.

/// A place the geocoder knows.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    pub address: String,
    pub lat: f64,
    pub lon: f64,
    /// south, north, west, east
    pub bbox: Option<[f64; 4]>,
    /// Its town and country (« Lisboa, Portugal »), when the answer had its address's parts.
    pub area: String,
    /// A street (OpenStreetMap's `highway`): its name comes back in many towns and districts.
    pub street: bool,
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The first place of a Nominatim answer (`search`: a list; `reverse`: one object, or `{"error": …}` over the sea).
pub fn found_of(answer: &Value) -> Option<Found> {
    let item = match answer {
        Value::Array(items) => items.first()?,
        other => other,
    };
    let lat = number(item.get("lat"))?;
    let lon = number(item.get("lon"))?;
    if !((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
        return None;
    }
    let address = item.get("display_name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut name = item.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if name.is_empty() {
        name = address.split(',').next().unwrap_or("").trim().to_string();
    }
    let bbox = item.get("boundingbox").and_then(Value::as_array).and_then(|b| {
        let v: Vec<f64> = b.iter().filter_map(|x| number(Some(x))).collect();
        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
    });
    let part = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| item["address"][*k].as_str().map(str::trim).filter(|v| !v.is_empty()))
            .unwrap_or("")
            .to_string()
    };
    let town = part(&["city", "town", "village", "municipality", "suburb", "county", "state"]);
    // « Sintra (Santa Maria e São Miguel, …) »: the town alone
    let town = town.split(" (").next().unwrap_or("").trim().to_string();
    let land = part(&["country"]);
    let area = [town, land].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(", ");
    // `jsonv2` says `category`, `json` says `class`
    let category = item.get("category").or_else(|| item.get("class")).and_then(Value::as_str);
    let street = category == Some("highway") || item.get("addresstype").and_then(Value::as_str) == Some("road");
    Some(Found { name, address, lat, lon, bbox, area, street })
}

/// Great-circle distance, km.
pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, la2) = (a.0.to_radians(), b.0.to_radians());
    let (dla, dlo) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().min(1.0).asin()
}

/// Where the places must be: a box (south, north, west, east), widened a little around the area asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub name: String,
    pub bbox: [f64; 4],
}

impl Area {
    /// The area around a place the geocoder found: its own box (or 10 km around it), widened by a quarter of its
    /// size and at least ~5 km (a city's box leaves out its outskirts and its shore).
    pub fn around(name: &str, found: &Found) -> Area {
        let [s, n, w, e] = found.bbox.unwrap_or([found.lat - 0.1, found.lat + 0.1, found.lon - 0.1, found.lon + 0.1]);
        let (dy, dx) = (((n - s) / 4.0).max(0.05), ((e - w) / 4.0).max(0.05));
        Area { name: name.trim().to_string(), bbox: [s - dy, n + dy, w - dx, e + dx] }
    }

    /// The box around a point, `radius` km from it each way (a search near a map's other places).
    pub fn within_km((lat, lon): (f64, f64), radius: f64) -> Area {
        let dy = radius / 111.2;
        let dx = radius / (111.2 * lat.to_radians().cos().max(0.01));
        Area { name: String::new(), bbox: [lat - dy, lat + dy, lon - dx, lon + dx] }
    }

    /// The part of this box inside the map's area (this box when there is none, or when they do not meet).
    pub fn inside(self, area: Option<&Area>) -> Area {
        let Some(area) = area else { return self };
        let ([s, n, w, e], [s2, n2, w2, e2]) = (self.bbox, area.bbox);
        let bbox = [s.max(s2), n.min(n2), w.max(w2), e.min(e2)];
        if bbox[0] < bbox[1] && bbox[2] < bbox[3] {
            Area { name: self.name, bbox }
        } else {
            self
        }
    }

    pub fn middle(&self) -> (f64, f64) {
        let [s, n, w, e] = self.bbox;
        ((s + n) / 2.0, (w + e) / 2.0)
    }

    pub fn holds(&self, (lat, lon): (f64, f64)) -> bool {
        let [s, n, w, e] = self.bbox;
        (s..=n).contains(&lat) && (w..=e).contains(&lon)
    }

    /// Nominatim's `viewbox`: west, north, east, south.
    fn viewbox(&self) -> String {
        let [s, n, w, e] = self.bbox;
        format!("{w:.5},{n:.5},{e:.5},{s:.5}")
    }
}

/// A name or an address looked up (inside `area` only, when given).
async fn search(config: &Config, query: &str, area: Option<&Area>) -> Result<Option<Found>, String> {
    let mut params = vec![
        ("q", query.trim().to_string()),
        ("format", "jsonv2".into()),
        ("limit", "1".into()),
        ("addressdetails", "1".into()),
    ];
    if let Some(area) = area {
        params.push(("viewbox", area.viewbox()));
        params.push(("bounded", "1".into()));
    }
    let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let found = found_of(&crate::weather::nominatim(config, "search", &params).await?);
    Ok(found.filter(|f| area.is_none_or(|a| a.holds((f.lat, f.lon)))))
}

/// What is at a point (None: nothing, the open sea).
async fn reverse(config: &Config, (lat, lon): (f64, f64)) -> Result<Option<Found>, String> {
    let (lat, lon) = (format!("{lat:.5}"), format!("{lon:.5}"));
    let params =
        [("lat", lat.as_str()), ("lon", lon.as_str()), ("format", "jsonv2"), ("zoom", "14"), ("addressdetails", "1")];
    Ok(found_of(&crate::weather::nominatim(config, "reverse", &params).await?))
}

/// A name inside the area: bounded to it, as given then its first part alone (« Torre de Belém, Avenida da Índia »:
/// the address was wrong), then with the area's name added (still inside it); last, anywhere within `AROUND_KM` of
/// the area (a day out of town: Sintra from Lisbon), said in the result. Some((place, a word on it)).
async fn search_in(
    config: &Config,
    name: &str,
    area: Option<&Area>,
) -> Result<Option<(Found, Option<String>)>, String> {
    let Some(area) = area else { return Ok(search(config, name, None).await?.map(|f| (f, None))) };
    if let Some(found) = search(config, name, Some(area)).await? {
        return Ok(Some((found, None)));
    }
    let first = name.split(',').next().unwrap_or("").trim();
    if first.len() > 2 && first != name.trim() {
        if let Some(found) = search(config, first, Some(area)).await? {
            return Ok(Some((found, None)));
        }
    }
    let town = area.name.split(',').next().unwrap_or("").trim().to_lowercase();
    if !town.is_empty() && !name.to_lowercase().contains(&town) {
        let found = search(config, &format!("{name}, {}", area.name), None).await?;
        if let Some(found) = found.filter(|f| area.holds((f.lat, f.lon))) {
            return Ok(Some((found, None)));
        }
    }
    let Some(found) = search(config, name, None).await? else { return Ok(None) };
    let far = km(area.middle(), (found.lat, found.lon));
    if far > AROUND_KM {
        return Ok(None);
    }
    let said = format!("« {name} » is outside « {} » ({far:.0} km from it)", area.name);
    Ok(Some((found, Some(said))))
}

/// How far from the area a place found outside it is still taken.
const AROUND_KM: f64 = 60.0;

/// Coordinates the model gave, checked: as given, then the other way round. Some((point, swapped, its town)).
async fn check_given(
    config: &Config,
    (lat, lon): (f64, f64),
    area: Option<&Area>,
) -> Result<Option<((f64, f64), bool, String)>, String> {
    for (point, swapped) in [((lat, lon), false), ((lon, lat), true)] {
        if swapped && (lon.abs() > 90.0 || (lat - lon).abs() < 1e-9) {
            continue;
        }
        let good = match area {
            Some(area) => area.holds(point).then(|| area.name.clone()),
            None => match reverse(config, point).await {
                Ok(found) => found.map(|f| f.area),
                // no way to check them now: taken as given
                Err(_) => return Ok(Some(((lat, lon), false, String::new()))),
            },
        };
        if let Some(town) = good {
            return Ok(Some((point, swapped, town)));
        }
    }
    Ok(None)
}

/// Where the coordinates came from (the map's data, `source`): looked up in OpenStreetMap, or given by the model
/// (checked), or none (a place not found: the apps may look it up themselves).
pub const FROM_OSM: &str = "osm";
pub const FROM_MODEL: &str = "model";
pub const FROM_NONE: &str = "none";

/// A marker looked up: its place, a word on it, where its coordinates come from; or why it was not found.
type Located = Result<(Found, Option<String>, &'static str), String>;

/// Where one marker goes, a word for the person when it was not as given, and where its coordinates come from. Err:
/// not found (said in the result).
async fn locate(config: &Config, place: &str, label: &str, area: Option<&Area>) -> Located {
    if let Some(given) = crate::weather::parse_coordinates(place) {
        let plain = |(lat, lon): (f64, f64), town: String| Found {
            name: place.trim().to_string(),
            address: String::new(),
            lat,
            lon,
            bbox: None,
            area: town,
            street: false,
        };
        match check_given(config, given, area).await? {
            Some((point, false, town)) => return Ok((plain(point, town), None, FROM_MODEL)),
            Some((point, true, town)) => {
                let said = format!("« {place} » was read as « lon, lat » (else in the sea)");
                return Ok((plain(point, town), Some(said), FROM_MODEL));
            }
            None if !label.trim().is_empty() => {
                return match search_in(config, label, area).await? {
                    Some((found, also)) => {
                        let said = format!("« {place} » was not on land: « {label} » looked up instead");
                        Ok((found, Some(also.map(|a| format!("{said}; {a}")).unwrap_or(said)), FROM_OSM))
                    }
                    None => Err(format!("« {place} » is not on land{} and « {label} » was not found", within(area))),
                }
            }
            None => {
                return Err(format!("« {place} » is not on land{}: give the place's name or address", within(area)))
            }
        }
    }
    match search_in(config, place, area).await? {
        Some((found, said)) => Ok((found, said, FROM_OSM)),
        None => Err(format!(
            "no place found for « {place} »{} (try its name in the local language, or its address)",
            within(area)
        )),
    }
}

/// A place looked up, or not found (said in the result); Err: the search itself is down, no use asking for the others.
fn unless_down<T>(done: Result<T, String>) -> Result<Result<T, String>, String> {
    match done {
        Err(why) if why.starts_with("the place search") => Err(why),
        done => Ok(done),
    }
}

fn within(area: Option<&Area>) -> String {
    match area {
        Some(a) if !a.name.is_empty() => format!(" in « {} »", a.name),
        Some(_) => " near the others".into(),
        None => String::new(),
    }
}

/// The middle of some places (the median of their latitudes, of their longitudes) and how far the farthest of them is
/// from it, km.
pub fn cluster(points: &[(f64, f64)]) -> ((f64, f64), f64) {
    let median = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        match v.len() {
            0 => 0.0,
            n if n % 2 == 1 => v[n / 2],
            n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
        }
    };
    let middle = (median(points.iter().map(|p| p.0).collect()), median(points.iter().map(|p| p.1).collect()));
    (middle, points.iter().map(|p| km(*p, middle)).fold(0.0, f64::max))
}

/// How far from the middle of a map's places another of its places may be: `NEAR_TIMES` as far as they spread
/// themselves, never less than `NEAR_MIN_KM`. Their spread is their farthest one, not a median: a Lisbon walk is often
/// three places in Belém and two downtown, 6 km apart, and a median would make the downtown ones strays.
pub fn near_limit(reach: f64) -> f64 {
    (reach * NEAR_TIMES).max(NEAR_MIN_KM)
}

/// The markers that do not go with the map's others: farther from the others' middle than `near_limit` of their own
/// spread (Le Moulleau, 2026-11-01: three stops within 400 m and « Avenue Gambetta » 3 km away, downtown Arcachon),
/// or far (> `FAR_KM`) from where most of them are (more than half within `NEAR_KM` of their middle: « Belém » found in
/// Brazil, the four others in Lisbon). A trip far apart on purpose spreads itself: Lisbon, Coimbra, Porto stay. Under
/// three places there is nothing to judge.
pub fn strays(points: &[(f64, f64)]) -> Vec<usize> {
    if points.len() < 3 {
        return Vec::new();
    }
    let (middle, _) = cluster(points);
    let most_near = points.iter().filter(|p| km(**p, middle) <= NEAR_KM).count() * 2 > points.len();
    (0..points.len())
        .filter(|i| {
            if most_near && km(points[*i], middle) > FAR_KM {
                return true;
            }
            let others: Vec<(f64, f64)> = points.iter().enumerate().filter(|(j, _)| j != i).map(|(_, p)| *p).collect();
            let (middle, reach) = cluster(&others);
            km(points[*i], middle) > near_limit(reach)
        })
        .collect()
}

const NEAR_KM: f64 = 60.0;
const FAR_KM: f64 = 150.0;
const NEAR_TIMES: f64 = 5.0;
const NEAR_MIN_KM: f64 = 2.0;

/// A street, or an address on one, rather than a place's own name: the same street names come back in many towns
/// and districts (« Avenue Gambetta », « Rua das Flores »), so it is looked for near the map's other places first.
pub fn street_like(place: &str) -> bool {
    if crate::weather::parse_coordinates(place).is_some() {
        return false;
    }
    let first = place.split(',').next().unwrap_or("").trim().to_lowercase();
    if first.starts_with(|c: char| c.is_ascii_digit()) {
        return true;
    }
    // the first word of a street's name, the last one, the end of a compound word
    const STARTS: &str =
        "rue avenue av. bd boulevard allée allee chemin impasse quai cours route sentier calle avenida \
paseo carrer rua travessa via viale corso vicolo street road strada";
    const ENDS: &str = "street st st. road rd avenue ave lane drive";
    const COMPOUNDS: [&str; 5] = ["straße", "strasse", "straat", "gasse", "laan"];
    let word = first.split([' ', '\'', '’']).next().unwrap_or("");
    let last = first.split_whitespace().last().unwrap_or("");
    STARTS.split(' ').any(|w| w == word)
        || ENDS.split(' ').any(|w| w == last)
        || COMPOUNDS.iter().any(|c| last.ends_with(c))
}

/// An address with its number (« Rua Augusta 100 », « 12 rue de Rivoli »): the agent says exactly where.
pub fn numbered(place: &str) -> bool {
    let first = place.split(',').next().unwrap_or("");
    first.split_whitespace().any(|w| w.len() <= 6 && w.starts_with(|c: char| c.is_ascii_digit()))
}

/// The town of « Arcachon, France » (lowercase).
fn town(area: &str) -> String {
    area.split(',').next().unwrap_or("").trim().to_lowercase()
}

fn distance(far: f64) -> String {
    if far < 10.0 {
        format!("{far:.1} km")
    } else {
        format!("{far:.0} km")
    }
}

/// A marker as the card lists it: its label, where it is on the picture (% of its width and height), where it is on
/// Earth, its colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub label: String,
    /// What the geocoder calls it in full (empty for checked coordinates).
    pub address: String,
    /// Its town and country, or the map's area.
    pub area: String,
    /// `FROM_OSM` or `FROM_MODEL`.
    pub source: &'static str,
    pub note: String,
    pub x: f64,
    pub y: f64,
    pub lat: f64,
    pub lon: f64,
    pub color: [u8; 3],
}

/// The map drawn: its PNG, its markers, and its center when it has no marker; what the person should hear about the
/// places (looked up again, not found).
pub struct Drawn {
    pub png: Vec<u8>,
    pub places: Vec<Placed>,
    pub center: Option<(f64, f64)>,
    pub zoom: u32,
    pub notes: Vec<String>,
    /// The places not found (their names), for the apps to look up themselves; the map's area.
    pub unfound: Vec<String>,
    pub area: String,
    /// An itinerary's way (`line`, map_route.rs, 2026-11-01): along the streets, or why it is the straight line.
    pub route: Option<crate::map_route::Planned>,
}

/// The map: its PNG and its markers.
pub async fn draw(config: &Config, arguments: &Dict) -> Result<Drawn, String> {
    if config.maps.tiles != "osm" {
        return Err(format!("unknown map tiles « {} » (only osm)", config.maps.tiles));
    }
    let w = arguments.get("width").and_then(Value::as_u64).unwrap_or(600).clamp(200, 800) as u32;
    let h = arguments.get("height").and_then(Value::as_u64).unwrap_or(400).clamp(150, 800) as u32;
    let text = |v: Option<&Value>| v.and_then(Value::as_str).map(str::trim).unwrap_or("").to_string();
    let mut notes = Vec::new();
    let area = match text(arguments.get("area")) {
        a if a.is_empty() => None,
        a => match search(config, &a, None).await? {
            Some(found) => Some(Area::around(&a, &found)),
            None => {
                notes.push(format!("the area « {a} » was not found: places looked up anywhere"));
                None
            }
        },
    };
    let area_name = area.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    // (its colour's index, what was asked, its label, the item)
    let items: Vec<(usize, String, String, &Value)> = arguments
        .get("markers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_MARKERS)
        .enumerate()
        .filter_map(|(i, item)| {
            let label = text(item.get("label"));
            let place = text(item.get("place"));
            let place = if place.is_empty() { label.clone() } else { place };
            (!place.is_empty()).then_some((i, place, label, item))
        })
        .collect();
    // the places' own names first, then the streets near them: a street's name is in many towns and districts, and
    // the geocoder gives its first one (2026-11-01, Le Moulleau)
    let mut located: Vec<Option<Located>> = vec![None; items.len()];
    for (k, (_, place, label, _)) in items.iter().enumerate().filter(|(_, it)| !street_like(&it.1)) {
        located[k] = Some(unless_down(locate(config, place, label, area.as_ref()).await)?);
    }
    let anchors: Vec<(f64, f64)> = located.iter().flatten().flatten().map(|(f, _, _)| (f.lat, f.lon)).collect();
    let mut tried_near = vec![false; items.len()];
    for k in 0..items.len() {
        if located[k].is_some() {
            continue;
        }
        let (_, place, label, _) = &items[k];
        if !anchors.is_empty() {
            let (middle, reach) = cluster(&anchors);
            let limit = near_limit(reach);
            let near = Area::within_km(middle, limit).inside(area.as_ref());
            tried_near[k] = true;
            let found = search(config, place, Some(&near)).await?;
            if let Some(found) = found.filter(|f| km(middle, (f.lat, f.lon)) <= limit) {
                located[k] = Some(Ok((found, None, FROM_OSM)));
                continue;
            }
        }
        located[k] = Some(unless_down(locate(config, place, label, area.as_ref()).await)?);
    }
    let mut markers = Vec::new();
    // what was asked for each marker, its label, whether it was looked for near the others already
    let mut asked: Vec<(String, String, bool)> = Vec::new();
    let mut missing = Vec::new();
    let mut unfound = Vec::new();
    for (k, ((i, place, label, item), done)) in items.into_iter().zip(located).enumerate() {
        let (found, said, source) = match done.unwrap_or_else(|| Err(format!("no place found for « {place} »"))) {
            Ok(done) => done,
            Err(why) => {
                missing.push(why);
                unfound.push(if label.is_empty() { place } else { label });
                continue;
            }
        };
        notes.extend(said);
        let paint = color(&text(item.get("color")), i);
        let shown = if !label.is_empty() {
            label.clone()
        } else if place.starts_with(|c: char| c.is_ascii_digit()) || !found.name.chars().any(char::is_alphabetic) {
            // an address: its own words (the geocoder names it by its number: « 12 »)
            place.split(',').next().unwrap_or(&place).trim().to_string()
        } else {
            found.name.clone()
        };
        let street = source == FROM_OSM && (street_like(&place) || found.street);
        markers.push(Marker {
            label: shown,
            address: found.address,
            area: if found.area.is_empty() {
                area.as_ref().map(|a| a.name.clone()).unwrap_or_default()
            } else {
                found.area
            },
            source,
            note: text(item.get("note")),
            lat: found.lat,
            lon: found.lon,
            color: paint,
            street,
        });
        asked.push((place, label, tried_near[k]));
    }
    // one map's places together: a place far from all the others is looked up again near them; a street still far
    // from them is not drawn there but left out and said, so the agent can give it better (a trip's stop in another
    // town it names, or an address with its number, stays where it is, with a word)
    let points: Vec<(f64, f64)> = markers.iter().map(|m| (m.lat, m.lon)).collect();
    let strays = strays(&points);
    let kept: Vec<usize> = (0..markers.len()).filter(|i| !strays.contains(i)).collect();
    let mut dropped = Vec::new();
    if !strays.is_empty() && !kept.is_empty() {
        let (middle, reach) = cluster(&kept.iter().map(|i| points[*i]).collect::<Vec<_>>());
        let limit = near_limit(reach);
        let near = Area::within_km(middle, limit);
        let towns: Vec<String> = kept.iter().map(|i| town(&markers[*i].area)).collect();
        for i in strays {
            let (place, label, tried) = &asked[i];
            // coordinates (given, or not on land): their label is what can be looked up
            let name = if crate::weather::parse_coordinates(place).is_some() { label } else { place };
            let again = if name.is_empty() || *tried {
                None
            } else {
                search(config, name, Some(&near)).await?.filter(|f| km(middle, (f.lat, f.lon)) <= limit)
            };
            let m = &mut markers[i];
            let far = distance(km((m.lat, m.lon), middle));
            // a stop in another town, named: « Rua das Flores, Porto » on a trip from Lisbon
            let elsewhere = {
                let t = town(&m.area);
                !t.is_empty() && place.to_lowercase().contains(&t) && !towns.contains(&t)
            };
            match again {
                Some(found) => {
                    notes.push(format!("« {place} » was {far} from the others: looked up again near them"));
                    (m.lat, m.lon, m.address, m.source) = (found.lat, found.lon, found.address, FROM_OSM);
                    if !found.area.is_empty() {
                        m.area = found.area;
                    }
                }
                None if m.street && !numbered(place) && !elsewhere => {
                    let at =
                        if m.address.is_empty() { String::new() } else { format!(" ({})", tk::head(&m.address, 90)) };
                    missing.push(format!(
                        "could not place « {place} » near the other places: the one found is {far} from them{at}; give \
the street of that stop near the others or a place beside it (if it really is there, its address with the number)"
                    ));
                    unfound.push(if label.is_empty() { place.clone() } else { label.clone() });
                    dropped.push(i);
                }
                None => notes.push(format!("« {place} » is {far} from the others: check it (or give area)")),
            }
        }
    }
    for i in dropped.into_iter().rev() {
        markers.remove(i);
    }
    if markers.is_empty() && !missing.is_empty() {
        return Err(missing.join("; "));
    }
    notes.extend(missing.into_iter().map(|why| format!("left out: {why}")));
    // an itinerary (2026-11-01): its stops joined along the streets, in the agent's order unless it asked for the
    // shortest; the map is framed on the way too (it may round a block beyond the stops)
    let line = arguments.get("line").and_then(Value::as_bool).unwrap_or(false);
    let route = if line && markers.len() > 1 {
        let stops: Vec<(f64, f64)> = markers.iter().map(|m| (m.lat, m.lon)).collect();
        let word = |key: &str| arguments.get(key).and_then(Value::as_str);
        let reorder = arguments.get("reorder").and_then(Value::as_bool).unwrap_or(false);
        let planned = crate::map_route::plan(config, word("travel"), word("directions"), reorder, &stops).await;
        if let Some(order) = &planned.order {
            let mut taken: Vec<Option<Marker>> = markers.into_iter().map(Some).collect();
            markers = order.iter().filter_map(|i| taken[*i].take()).collect();
        }
        Some(planned)
    } else {
        None
    };
    let way: &[(f64, f64)] = route.as_ref().and_then(|r| r.way.as_ref()).map(|w| w.path.as_slice()).unwrap_or(&[]);
    let points: Vec<(f64, f64)> = markers.iter().map(|m| (m.lat, m.lon)).chain(way.iter().copied()).collect();
    let mut asked_center = None;
    let center = match arguments.get("center").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => {
            let (found, _, _) = locate(config, c, "", area.as_ref()).await?;
            asked_center = Some((found.lat, found.lon));
            (found.lat, found.lon)
        }
        None if !points.is_empty() => {
            let z = 17;
            let px: Vec<(f64, f64)> = points.iter().map(|(la, lo)| pixel(*la, *lo, z)).collect();
            let (min_x, max_x) = px.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
            let (min_y, max_y) = px.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            unproject((min_x + max_x) / 2.0, (min_y + max_y) / 2.0, z)
        }
        None => return Err("give markers or a center".into()),
    };
    let z = arguments
        .get("zoom")
        .and_then(Value::as_u64)
        .map(|z| z.clamp(3, 18) as u32)
        .unwrap_or_else(|| fit(&points, w, h));
    let (cx, cy) = pixel(center.0, center.1, z);
    let (left, top) = (cx - w as f64 / 2.0, cy - h as f64 / 2.0);
    let (tx0, ty0) = ((left / TILE).floor() as i64, (top / TILE).floor() as i64);
    let (tx1, ty1) = (((left + w as f64) / TILE).floor() as i64, ((top + h as f64) / TILE).floor() as i64);
    let count = ((tx1 - tx0 + 1) * (ty1 - ty0 + 1)) as usize;
    if count > MAX_TILES {
        return Err("the map is too big".into());
    }
    let mut canvas = Canvas::new(w, h);
    for ty in ty0..=ty1 {
        for tx in tx0..=tx1 {
            let data = tile(config, z, tx, ty).await?;
            let Some(pixels) = decode(&data) else { continue };
            let (ox, oy) = ((tx as f64 * TILE - left).round() as i64, (ty as f64 * TILE - top).round() as i64);
            for y in 0..256i64 {
                let cy = oy + y;
                if cy < 0 || cy >= h as i64 {
                    continue;
                }
                for x in 0..256i64 {
                    let cx = ox + x;
                    if cx < 0 || cx >= w as i64 {
                        continue;
                    }
                    let s = ((y * 256 + x) * 4) as usize;
                    let d = ((cy as u32 * w + cx as u32) * 4) as usize;
                    canvas.rgba[d..d + 3].copy_from_slice(&pixels[s..s + 3]);
                }
            }
        }
    }
    if line {
        // the way along the streets when there is one; else the straight line, dashed (not a way to follow)
        let on_picture = |(lat, lon): (f64, f64)| {
            let (px, py) = pixel(lat, lon, z);
            (px - left, py - top)
        };
        let path: Vec<(f64, f64)> = if way.len() >= 2 {
            way.iter().copied().map(on_picture).collect()
        } else {
            markers.iter().map(|m| on_picture((m.lat, m.lon))).collect()
        };
        let (white, ink) = ([0xFF, 0xFF, 0xFF], [0x7C, 0x6C, 0xF2]);
        if way.len() >= 2 || route.is_none() {
            for (r, c) in [(3.5, white), (2.2, ink)] {
                for pair in path.windows(2) {
                    canvas.segment(pair[0], pair[1], r, c);
                }
            }
        } else {
            for (r, c) in [(3.5, white), (2.2, ink)] {
                let mut phase = 0.0;
                for pair in path.windows(2) {
                    phase = canvas.dashed(pair[0], pair[1], r, c, (12.0, 8.0), phase);
                }
            }
        }
    }
    let mut places = Vec::new();
    for (i, m) in markers.iter().enumerate() {
        let (px, py) = pixel(m.lat, m.lon, z);
        let (x, y) = (px - left, py - top);
        canvas.disc(x + 1.0, y + 2.0, 14.0, [0, 0, 0]); // a soft shadow, drawn light
        canvas.disc(x, y, 14.0, [255, 255, 255]);
        canvas.disc(x, y, 11.5, m.color);
        let number = (i + 1).to_string();
        let width = number.len() as i64 * 12 - 2;
        canvas.text((x - width as f64 / 2.0).round() as i64, (y - 7.0).round() as i64, &number, 2, [255, 255, 255]);
        places.push(Placed {
            label: m.label.clone(),
            address: m.address.clone(),
            area: m.area.clone(),
            source: m.source,
            note: m.note.clone(),
            x: (x / w as f64 * 1000.0).round() / 10.0,
            y: (y / h as f64 * 1000.0).round() / 10.0,
            lat: m.lat,
            lon: m.lon,
            color: m.color,
        });
    }
    // the attribution the tile policy asks for, on the picture itself
    let text_w = ATTRIBUTION.chars().count() as i64 * 12;
    let (bx, by) = (w as i64 - text_w - 14, h as i64 - 24);
    canvas.rect(bx, by, text_w + 14, 24, [255, 255, 255], 0.85);
    canvas.text(bx + 7, by + 5, ATTRIBUTION, 2, [0x33, 0x33, 0x33]);
    let center = asked_center.filter(|_| markers.is_empty());
    Ok(Drawn { png: canvas.png(), places, center, zoom: z, notes, unfound, area: area_name, route })
}

fn unproject(x: f64, y: f64, z: u32) -> (f64, f64) {
    let scale = TILE * 2f64.powi(z as i32);
    let lon = x / scale * 360.0 - 180.0;
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y / scale;
    (n.sinh().atan().to_degrees(), lon)
}

/// The map as data, for the apps that draw maps themselves (Apple Plans, Google Maps; 2026-10-31) and to rebuild the
/// card in a page: every position from the geocoder, never from the model. Kept beside the picture (`<id>.json`).
///
/// `{"type": "map", "version": 1, "id", "title", "area"?, "points": [{"name", "address"?, "area"?, "lat"?, "lon"?,
/// "source": "osm"|"model"|"none", "note"?, "color", "x", "y"}] (no lat/lon when "none"),
/// "route": {"line": bool, "mode": "walking"|"driving"|"transit"|"bicycling"|null, and with a line since 2026-11-01
/// (map_route.rs): "travel": "walking"|"bicycling"|"driving"|"transit", "streets": bool (false: the straight line,
/// dashed), "meters" (along the streets, or straight), with the streets "path": [[lat, lon], …] (simplified),
/// "seconds", "attribution"} | null,
/// "center": {"lat", "lon", "zoom"} | null (a map without markers), "attribution"}`
pub fn data(id: &str, title: &str, drawn: &Drawn, line: bool, directions: Option<&str>) -> Value {
    let points: Vec<Value> = drawn
        .places
        .iter()
        .map(|p| {
            let mut point = json!({"name": p.label, "lat": round6(p.lat), "lon": round6(p.lon), "source": p.source,
                                   "color": format!("#{}", hex::encode(p.color)), "x": p.x, "y": p.y});
            if !p.area.is_empty() {
                point["area"] = json!(p.area);
            }
            if !p.address.is_empty() {
                point["address"] = json!(p.address);
            }
            if !p.note.is_empty() {
                point["note"] = json!(p.note);
            }
            point
        })
        .chain(drawn.unfound.iter().map(|name| {
            let mut point = json!({"name": name, "source": FROM_NONE});
            if !drawn.area.is_empty() {
                point["area"] = json!(drawn.area);
            }
            point
        }))
        .collect();
    let line = line && drawn.places.len() > 1;
    let mode = directions.filter(|m| MODES.contains(m)).filter(|_| drawn.places.len() > 1);
    let mut route = if line || mode.is_some() { json!({"line": line, "mode": mode}) } else { Value::Null };
    if let Some(planned) = drawn.route.as_ref().filter(|_| line) {
        route["travel"] = json!(planned.travel);
        match &planned.way {
            Some(way) => {
                route["streets"] = json!(true);
                route["path"] = json!(way.path.iter().map(|(a, b)| [*a, *b]).collect::<Vec<_>>());
                route["meters"] = json!(way.meters.round() as u64);
                route["seconds"] = json!(way.seconds.round() as u64);
                route["attribution"] = json!(crate::map_route::ATTRIBUTION);
            }
            None => {
                let stops: Vec<(f64, f64)> = drawn.places.iter().map(|p| (p.lat, p.lon)).collect();
                route["streets"] = json!(false);
                route["meters"] = json!((crate::map_route::straight_km(&stops) * 1000.0).round() as u64);
            }
        }
    }
    json!({
        "type": "map",
        "version": 1,
        "id": id,
        "title": title,
        "area": if drawn.area.is_empty() { Value::Null } else { json!(drawn.area) },
        "points": points,
        "route": route,
        "center": drawn.center.map(|(lat, lon)| json!({"lat": round6(lat), "lon": round6(lon), "zoom": drawn.zoom})),
        "attribution": ATTRIBUTION,
    })
}

fn round6(v: f64) -> f64 {
    (v * 1e6).round() / 1e6
}

/// The card in a page, from the map's data: the picture (`map:<id>`, put inline when the page is kept), its list
/// with each place's position and colour (the page kit makes it a live map in the apps), what the live map adds (a
/// line, directions, the center of a map without markers), and the data itself for the apps (`.map-data`).
pub fn figure_of(map: &Value) -> String {
    let id = map["id"].as_str().unwrap_or("");
    let empty = Vec::new();
    let points = map["points"].as_array().unwrap_or(&empty);
    let items: String = points
        .iter()
        .filter(|p| p["lat"].is_f64() && p["lon"].is_f64())
        .map(|p| {
            let f = |k: &str| p[k].as_f64().unwrap_or(0.0);
            let note = p["note"].as_str().filter(|n| !n.is_empty());
            format!(
                "<li data-x=\"{}\" data-y=\"{}\" data-lat=\"{:.6}\" data-lon=\"{:.6}\" data-color=\"{}\">{}{}</li>",
                f("x"),
                f("y"),
                f("lat"),
                f("lon"),
                tk::html::escape(p["color"].as_str().unwrap_or("")),
                tk::html::escape(p["name"].as_str().unwrap_or("")),
                note.map(|n| format!(" <small>{}</small>", tk::html::escape(n))).unwrap_or_default()
            )
        })
        .collect();
    let mut attributes = format!(" data-map=\"{}\"", tk::html::escape(id));
    if map["route"]["line"].as_bool() == Some(true) {
        attributes.push_str(" data-line=\"1\"");
    }
    if let Some(mode) = map["route"]["mode"].as_str().filter(|m| MODES.contains(m)) {
        attributes.push_str(&format!(" data-directions=\"{mode}\""));
    }
    if let (Some(lat), Some(lon)) = (map["center"]["lat"].as_f64(), map["center"]["lon"].as_f64()) {
        attributes.push_str(&format!(" data-center=\"{lat:.6},{lon:.6}\" data-zoom=\"{}\"", map["center"]["zoom"]));
    }
    // inside a <script>: no « </ » can end it early
    let json = serde_json::to_string(map).unwrap_or_default().replace('<', "\\u003c");
    format!(
        "<figure class=\"map-card\"{attributes}><img src=\"map:{id}\" alt=\"Carte\"><ol>{items}</ol>\
<script type=\"application/json\" class=\"map-data\">{json}</script></figure>"
    )
}

/// The card of a map as `map_image` gave it (for tests and old callers).
pub fn figure(id: &str, drawn: &Drawn, line: bool, directions: Option<&str>) -> String {
    figure_of(&data(id, "", drawn, line, directions))
}

/// A kept map's data (`<id>.json`), if any.
pub fn kept(config: &Config, agent: &str, id: &str) -> Option<Value> {
    let picture = file(config, agent, id)?;
    let text = std::fs::read_to_string(picture.with_extension("json")).ok()?;
    serde_json::from_str::<Value>(&text).ok().filter(|v| v["type"] == "map")
}

/// What the agent pastes in a page: short, so that nothing in it is copied wrong; the bridge puts the whole card in
/// its place when the page is kept (pages.rs).
pub fn short_card(id: &str) -> String {
    format!("<figure class=\"map-card\"><img src=\"map:{id}\" alt=\"Carte\"></figure>")
}

/// `map_image`: the map kept for the agent, with how to show it.
pub async fn call(config: &Config, agent: &str, arguments: &Dict) -> Dict {
    let drawn = match draw(config, arguments).await {
        Ok(made) => made,
        Err(why) => return tool_text(&format!("No map: {why}."), true),
    };
    let png = &drawn.png;
    let id = format!("m-{}", &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(png))[..10]);
    let folder = root(config).join(agent);
    let path = folder.join(format!("{id}.png"));
    let line = arguments.get("line").and_then(Value::as_bool).unwrap_or(false);
    let directions = arguments.get("directions").and_then(Value::as_str).map(str::trim);
    let title = arguments.get("title").and_then(Value::as_str).map(str::trim).unwrap_or("");
    let title = if title.is_empty() { "Carte" } else { tk::head(title, 80) };
    let map = data(&id, title, &drawn, line, directions);
    let json = serde_json::to_string_pretty(&map).unwrap_or_default();
    if std::fs::create_dir_all(&folder).is_err()
        || tk::replace_with(&folder.join(format!(".{id}.part")), &path, png).is_err()
        || tk::replace_with(&folder.join(format!(".{id}.json.part")), &path.with_extension("json"), json.as_bytes())
            .is_err()
    {
        return tool_text("No map: it could not be kept.", true);
    }
    // the map shown at once: a page holding only it (a live map in the app, its picture elsewhere)
    let page = format!("<h1>{}</h1>{}", tk::html::escape(title), short_card(&id));
    let shown = match crate::pages::save(config, agent, &page, title, Some(&format!("carte-{}", &id[2..]))) {
        Ok(made) => made.get("path").and_then(Value::as_str).map(str::to_string),
        Err(_) => None,
    };
    let places: Vec<String> = drawn
        .places
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let at = if p.address.is_empty() { String::new() } else { format!(" — {}", tk::head(&p.address, 90)) };
            format!("{}. {}{at}", i + 1, p.label)
        })
        .collect();
    let mut checks =
        if drawn.notes.is_empty() { String::new() } else { format!("\nCheck: {}.", drawn.notes.join("; ")) };
    // the itinerary's way, as the card shows it (2026-11-01): what the agent may say of it
    if let Some(planned) = drawn.route.as_ref().filter(|_| line && drawn.places.len() > 1) {
        let order = if planned.order.is_some() {
            "stops reordered for the shortest way, first and last kept: the list above is the new order"
        } else {
            "stops in your order"
        };
        checks.push_str(&match &planned.way {
            Some(way) => format!(
                "\nWay: {} along the streets ({order}; OpenStreetMap, OSRM). The card shows it.",
                crate::map_route::summary(planned.travel, way)
            ),
            None => format!(
                "\nWay: a dashed straight line, not along the streets ({}); give no walking or driving time for it.",
                planned.why.as_deref().unwrap_or("no route")
            ),
        });
    }
    let show = match shown {
        Some(path) => format!(
            "MEDIA:{path}\nMap ready ({} markers, placed by the bridge from the places' names):\n{}{checks}\nTo show it, put \
the MEDIA line above alone on its own line in your reply, exactly as it is: a live map in the app (move, zoom, « Open \
in Maps »). Inside a bigger create_page page, paste instead: {}\nThe positions come from OpenStreetMap's search \
of these names (a place on the water's edge, like a tower on a river, is drawn at the edge). If the person says one is \
wrong, call map_image again with a more precise name or address for it (and area); never write coordinates or a map \
of your own.",
            drawn.places.len(),
            places.join("\n"),
            short_card(&id)
        ),
        None => format!(
            "MEDIA:{PREFIX}/{agent}/{id}.png\nMap ready ({} markers, placed by the bridge):\n{}{checks}\nPut the MEDIA \
line above alone on its own line in your reply (its picture), or paste into create_page's HTML: {}",
            drawn.places.len(),
            places.join("\n"),
            short_card(&id)
        ),
    };
    tool_text(&show, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_and_fit() {
        let (x, y) = pixel(0.0, 0.0, 1);
        assert!((x - 256.0).abs() < 1e-6 && (y - 256.0).abs() < 1e-6);
        let (lat, lon) = unproject(x, y, 1);
        assert!(lat.abs() < 1e-9 && lon.abs() < 1e-9);
        let paris = [(48.8566, 2.3522), (48.8584, 2.2945)];
        let z = fit(&paris, 600, 400);
        assert!((12..=14).contains(&z), "{z}");
        assert_eq!(fit(&[(48.0, 2.0)], 600, 400), 15);
        assert_eq!(color("#102030", 0), [0x10, 0x20, 0x30]);
        assert_eq!(color("Blue", 0), [0x4F, 0x7C, 0xFF]);
        assert_eq!(color("", 1), PALETTE[1].1);
        assert!(ATTRIBUTION.chars().all(|c| c == ' ' || glyph(c).is_some()));
    }

    #[test]
    fn the_card_carries_what_the_live_map_needs() {
        let place = |label: &str, lat: f64| Placed {
            label: label.into(),
            address: String::new(),
            area: String::new(),
            source: FROM_OSM,
            note: String::new(),
            x: 10.0,
            y: 20.5,
            lat,
            lon: 4.85,
            color: [0xE5, 0x48, 0x4D],
        };
        let drawn = Drawn {
            png: Vec::new(),
            places: vec![place("Départ <A>", 45.7), place("Fin", 45.71)],
            center: None,
            zoom: 14,
            notes: Vec::new(),
            unfound: vec!["Nulle part".into()],
            area: "Lyon".into(),
            route: None,
        };
        let card = figure("m-1", &drawn, true, Some("walking"));
        assert!(card.starts_with(
            "<figure class=\"map-card\" data-map=\"m-1\" data-line=\"1\" data-directions=\"walking\"><img src=\"map:m-1\" \
alt=\"Carte\"><ol>\
<li data-x=\"10\" data-y=\"20.5\" data-lat=\"45.700000\" data-lon=\"4.850000\" data-color=\"#e5484d\">Départ &lt;A&gt;</li>\
<li data-x=\"10\" data-y=\"20.5\" data-lat=\"45.710000\" data-lon=\"4.850000\" data-color=\"#e5484d\">Fin</li></ol>\
<script type=\"application/json\" class=\"map-data\">"
        ), "{card}");
        // the data inside cannot close its script early, and reads back as the map
        assert!(!card.contains("<A>") && card.ends_with("</script></figure>"));
        let json = card.split("class=\"map-data\">").nth(1).unwrap().trim_end_matches("</script></figure>");
        let map: Value = serde_json::from_str(json).unwrap();
        assert_eq!(map["points"][0]["name"], "Départ <A>");
        assert_eq!((map["points"][1]["lat"].as_f64(), map["points"][1]["lon"].as_f64()), (Some(45.71), Some(4.85)));
        assert_eq!(map["route"], json!({"line": true, "mode": "walking"}));
        assert_eq!(map["area"], "Lyon");
        assert_eq!(map["points"][0]["source"], "osm");
        // a place not found: in the data for the apps (they may find it), not in the card's list
        assert_eq!(map["points"][2], json!({"name": "Nulle part", "source": "none", "area": "Lyon"}));
        assert_eq!(card.matches("<li ").count(), 2);
        // an unknown mode, or a single place: no directions, no line
        let alone = Drawn { places: vec![place("A", 1.0)], ..drawn };
        let card = figure("m-1", &alone, true, Some("walking"));
        assert!(!card.contains("data-line") && !card.contains("data-directions") && card.contains("\"route\":null"));
        let empty = Drawn {
            png: Vec::new(),
            places: Vec::new(),
            center: Some((45.5, 4.25)),
            zoom: 12,
            notes: Vec::new(),
            unfound: Vec::new(),
            area: String::new(),
            route: None,
        };
        assert!(figure("m-2", &empty, false, Some("teleport")).starts_with(
            "<figure class=\"map-card\" data-map=\"m-2\" data-center=\"45.500000,4.250000\" data-zoom=\"12\"><img"
        ));
    }

    #[test]
    fn an_itinerary_s_card_carries_its_way_along_the_streets_or_says_it_is_straight() {
        use crate::map_route::{Planned, Way};
        let place = |lat: f64, lon: f64| Placed {
            label: "A".into(),
            address: String::new(),
            area: String::new(),
            source: FROM_OSM,
            note: String::new(),
            x: 1.0,
            y: 2.0,
            lat,
            lon,
            color: [0, 0, 0],
        };
        let way = Way {
            path: vec![(44.64121, -1.19909), (44.6415, -1.2), (44.64172, -1.20439)],
            meters: 423.6,
            seconds: 338.8,
        };
        let mut drawn = Drawn {
            png: Vec::new(),
            places: vec![place(44.64121, -1.19909), place(44.64172, -1.20439)],
            center: None,
            zoom: 16,
            notes: Vec::new(),
            unfound: Vec::new(),
            area: String::new(),
            route: Some(Planned { travel: "walking", order: None, way: Some(way), why: None }),
        };
        let map = data("m-1", "", &drawn, true, None);
        assert_eq!(
            map["route"],
            json!({"line": true, "mode": null, "travel": "walking", "streets": true, "meters": 424, "seconds": 339,
                   "path": [[44.64121, -1.19909], [44.6415, -1.2], [44.64172, -1.20439]],
                   "attribution": crate::map_route::ATTRIBUTION})
        );
        // no way: the straight line, its length as the crow flies, no time
        drawn.route = Some(Planned { travel: "walking", order: None, way: None, why: Some("timed out".into()) });
        let map = data("m-1", "", &drawn, true, Some("walking"));
        assert_eq!(
            map["route"],
            json!({"line": true, "mode": "walking", "travel": "walking", "streets": false, "meters": 423})
        );
        // not an itinerary: nothing of a way
        assert_eq!(data("m-1", "", &drawn, false, None)["route"], Value::Null);
    }

    #[test]
    fn nominatim_answers_read_in_their_order() {
        // search: a list, lat and lon as strings; boundingbox south, north, west, east
        let lisbon = json!([{"lat": "38.7077507", "lon": "-9.1365919", "name": "Lisboa",
            "display_name": "Lisboa, Portugal", "boundingbox": ["38.6913994", "38.7967584", "-9.2298356", "-9.0863328"]}]);
        let found = found_of(&lisbon).unwrap();
        assert_eq!((found.lat, found.lon), (38.7077507, -9.1365919), "lat then lon, never swapped");
        assert_eq!(found.bbox, Some([38.6913994, 38.7967584, -9.2298356, -9.0863328]));
        // reverse: one object; over the sea an error
        assert!(found_of(&json!({"error": "Unable to geocode"})).is_none());
        assert!(found_of(&json!([])).is_none());
        let area = Area::around("Lisboa", &found);
        assert!(area.holds((38.6916, -9.2160)), "Torre de Belém, on the city's edge");
        assert!(!area.holds((-9.2160, 38.6916)), "the same, lon first: off Africa");
        assert!(!area.holds((-22.9, -43.2)), "Rio");
        assert_eq!(area.viewbox(), "-9.27984,38.84676,-9.03633,38.64140");
        assert!((km((48.8566, 2.3522), (38.7223, -9.1393)) - 1450.0).abs() < 20.0);
    }

    #[test]
    fn a_marker_far_from_the_others_is_a_stray() {
        let lisbon = [(38.6916, -9.2160), (38.6979, -9.2068), (38.7139, -9.1335), (38.7101, -9.1366)];
        assert!(strays(&lisbon).is_empty());
        let mut one_off = lisbon.to_vec();
        one_off.push((-1.4558, -48.5044)); // « Belém » found in Brazil
        assert_eq!(strays(&one_off), vec![4]);
        // two or three far apart on purpose (a trip): no majority, nothing moved
        assert!(strays(&[(48.85, 2.35), (38.72, -9.14), (41.39, 2.17)]).is_empty());
        assert!(strays(&[(48.85, 2.35), (38.72, -9.14)]).is_empty());
    }

    #[test]
    fn a_map_s_places_are_judged_by_their_own_spread() {
        // Le Moulleau (2026-11-01): the church, the pier, the beach within 400 m; « Avenue Gambetta » downtown, 3 km
        let moulleau =
            [(44.6412126, -1.1990896), (44.6417176, -1.2043897), (44.6432, -1.2020), (44.6577791, -1.1681525)];
        assert_eq!(strays(&moulleau), vec![3]);
        let (middle, reach) = cluster(&moulleau[..3]);
        assert!(reach < 0.4 && near_limit(reach) == NEAR_MIN_KM, "{reach}");
        assert!((3.0..3.5).contains(&km(moulleau[3], middle)));
        // a Lisbon walk: three places in Belém, two downtown 6 km away: no stray
        let lisbon =
            [(38.6916, -9.2160), (38.6978, -9.2067), (38.6976, -9.2063), (38.7139, -9.1335), (38.7075, -9.1364)];
        assert!(strays(&lisbon).is_empty());
        // a trip Lisbon → Coimbra → Porto: far apart on purpose
        let trip = [(38.6916, -9.2160), (38.6978, -9.2067), (40.2073, -8.4260), (41.1458, -8.6139), (41.1435, -8.6130)];
        assert!(strays(&trip).is_empty());
        // one place, two: nothing to judge
        assert!(strays(&[(44.6577791, -1.1681525)]).is_empty());
        assert!(strays(&moulleau[2..]).is_empty());
    }

    #[test]
    fn streets_and_addresses_are_told_from_places_names() {
        for street in [
            "Avenue Gambetta, Camicas, Arcachon",
            "rue des Pins",
            "12 rue de Rivoli, Paris",
            "Rua das Flores, Porto",
            "Calle Mayor",
            "Kaiserstraße",
            "Baker Street",
            "Via del Corso",
            "Allée des Mimosas",
        ] {
            assert!(street_like(street), "{street}");
        }
        for place in [
            "Église Notre-Dame-des-Passes",
            "Jetée du Moulleau",
            "Torre de Belém, Avenida da Índia",
            "Praça do Comércio",
            "Plage du Moulleau",
            "44.64, -1.20",
        ] {
            assert!(!street_like(place), "{place}");
        }
        assert!(numbered("Rua Augusta 100, Lisboa") && numbered("12 rue de Rivoli"));
        assert!(!numbered("Avenue Gambetta, 33120 Arcachon") && !numbered("Avenue Gambetta"));
        // OpenStreetMap says a street is one (`category`, or `class` in its older format)
        let road = json!([{"lat": "44.6577791", "lon": "-1.1681525", "name": "Avenue Gambetta", "category": "highway",
            "address": {"town": "Arcachon", "country": "France"}}]);
        let found = found_of(&road).unwrap();
        assert!(found.street && found.area == "Arcachon, France");
        assert!(found_of(&json!([{"lat": "1", "lon": "2", "class": "highway"}])).unwrap().street);
        assert!(!found_of(&json!([{"lat": "1", "lon": "2", "category": "amenity"}])).unwrap().street);
        // the box near the others, kept inside the map's area
        let near = Area::within_km((44.6420, -1.2018), 2.0);
        assert!(near.holds((44.6412, -1.1991)) && !near.holds((44.6578, -1.1682)));
        let arcachon = Area { name: "Arcachon".into(), bbox: [44.63, 44.67, -1.21, -1.13] };
        assert_eq!(near.clone().inside(Some(&arcachon)).bbox[2], -1.21);
        assert_eq!(near.clone().inside(None), near);
    }
}
