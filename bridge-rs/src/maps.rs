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
        "description": "Make a map (OpenStreetMap) with numbered markers, for a page or a reply. Give the places as names \
    (« Gare de Lyon, Paris ») or « lat, lon ». The result gives a <figure class=\"map-card\"> to paste as it is in \
    create_page's HTML: in the app it is a live map (move, zoom, a tap on a line of its list goes to its marker, an \
    « Open in Maps » button per place), elsewhere its picture. line: draw a line through the markers in order (a walk, \
    a tour). directions: a button for the way from the first marker to the last (walking, driving, transit, \
    bicycling). Also a MEDIA: line to show the picture alone in a reply.",
        "inputSchema": {"type": "object", "properties": {
            "markers": {"type": "array", "maxItems": MAX_MARKERS, "items": {"type": "object", "properties": {
                "place": {"type": "string", "description": "a name or « lat, lon »"},
                "label": {"type": "string", "description": "what the list says for it"},
                "color": {"type": "string", "description": "red, blue, green, orange, purple, teal, yellow, pink or #rrggbb"}},
                "required": ["place"]}},
            "center": {"type": "string", "description": "without markers: the place the map shows"},
            "zoom": {"type": "integer", "minimum": 3, "maximum": 18, "description": "default: what fits the markers"},
            "line": {"type": "boolean", "description": "a line through the markers, in their order"},
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
    lat: f64,
    lon: f64,
    color: [u8; 3],
}

async fn locate(config: &Config, place: &str) -> Result<(f64, f64, String), String> {
    if let Some((lat, lon)) = crate::weather::parse_coordinates(place) {
        return Ok((lat, lon, place.trim().to_string()));
    }
    match crate::weather::geocode(config, place).await? {
        Some(found) => Ok((found.lat, found.lon, found.name)),
        None => Err(format!("no place found for « {place} »")),
    }
}

/// A marker as the card lists it: its label, where it is on the picture (% of its width and height), where it is on
/// Earth, its colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub lat: f64,
    pub lon: f64,
    pub color: [u8; 3],
}

/// The map drawn: its PNG, its markers, and its center when it has no marker.
pub struct Drawn {
    pub png: Vec<u8>,
    pub places: Vec<Placed>,
    pub center: Option<(f64, f64)>,
    pub zoom: u32,
}

/// The map: its PNG and its markers.
pub async fn draw(config: &Config, arguments: &Dict) -> Result<Drawn, String> {
    if config.maps.tiles != "osm" {
        return Err(format!("unknown map tiles « {} » (only osm)", config.maps.tiles));
    }
    let w = arguments.get("width").and_then(Value::as_u64).unwrap_or(600).clamp(200, 800) as u32;
    let h = arguments.get("height").and_then(Value::as_u64).unwrap_or(400).clamp(150, 800) as u32;
    let mut markers = Vec::new();
    for (i, item) in
        arguments.get("markers").and_then(Value::as_array).into_iter().flatten().take(MAX_MARKERS).enumerate()
    {
        let place = item.get("place").and_then(Value::as_str).unwrap_or("").trim();
        if place.is_empty() {
            continue;
        }
        let (lat, lon, found) = locate(config, place).await?;
        let label = item.get("label").and_then(Value::as_str).map(str::trim).filter(|l| !l.is_empty());
        let paint = color(item.get("color").and_then(Value::as_str).unwrap_or(""), i);
        markers.push(Marker { label: label.map(str::to_string).unwrap_or(found), lat, lon, color: paint });
    }
    let points: Vec<(f64, f64)> = markers.iter().map(|m| (m.lat, m.lon)).collect();
    let mut asked_center = None;
    let center = match arguments.get("center").and_then(Value::as_str).map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => {
            let (lat, lon, _) = locate(config, c).await?;
            asked_center = Some((lat, lon));
            (lat, lon)
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
    let line = arguments.get("line").and_then(Value::as_bool).unwrap_or(false);
    if line {
        let path: Vec<(f64, f64)> = markers
            .iter()
            .map(|m| {
                let (px, py) = pixel(m.lat, m.lon, z);
                (px - left, py - top)
            })
            .collect();
        for pair in path.windows(2) {
            canvas.segment(pair[0], pair[1], 3.5, [0xFF, 0xFF, 0xFF]);
        }
        for pair in path.windows(2) {
            canvas.segment(pair[0], pair[1], 2.2, [0x7C, 0x6C, 0xF2]);
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
    Ok(Drawn { png: canvas.png(), places, center: asked_center.filter(|_| markers.is_empty()), zoom: z })
}

fn unproject(x: f64, y: f64, z: u32) -> (f64, f64) {
    let scale = TILE * 2f64.powi(z as i32);
    let lon = x / scale * 360.0 - 180.0;
    let n = std::f64::consts::PI - 2.0 * std::f64::consts::PI * y / scale;
    (n.sinh().atan().to_degrees(), lon)
}

/// The card to paste in a page: the picture (`map:<id>`, put inline when the page is kept), its list with each place's
/// position and colour, and what the live map adds (a line, directions, the center of a map without markers).
pub fn figure(id: &str, drawn: &Drawn, line: bool, directions: Option<&str>) -> String {
    let items: String = drawn
        .places
        .iter()
        .map(|p| {
            format!(
                "<li data-x=\"{}\" data-y=\"{}\" data-lat=\"{:.6}\" data-lon=\"{:.6}\" data-color=\"#{}\">{}</li>",
                p.x,
                p.y,
                p.lat,
                p.lon,
                hex::encode(p.color),
                tk::html::escape(&p.label)
            )
        })
        .collect();
    let mut attributes = String::new();
    if line && drawn.places.len() > 1 {
        attributes.push_str(" data-line=\"1\"");
    }
    if let Some(mode) = directions.filter(|m| MODES.contains(m)).filter(|_| drawn.places.len() > 1) {
        attributes.push_str(&format!(" data-directions=\"{mode}\""));
    }
    if let Some((lat, lon)) = drawn.center {
        attributes.push_str(&format!(" data-center=\"{lat:.6},{lon:.6}\" data-zoom=\"{}\"", drawn.zoom));
    }
    format!("<figure class=\"map-card\"{attributes}><img src=\"map:{id}\" alt=\"Carte\"><ol>{items}</ol></figure>")
}

/// `map_image`: the map kept for the agent, with how to show it.
pub async fn call(config: &Config, agent: &str, arguments: &Dict) -> Dict {
    let drawn = match draw(config, arguments).await {
        Ok(made) => made,
        Err(why) => return tool_text(&format!("No map: {why}."), true),
    };
    let data = &drawn.png;
    let id = format!("m-{}", &hex::encode(<sha2::Sha256 as sha2::Digest>::digest(data))[..10]);
    let folder = root(config).join(agent);
    let path = folder.join(format!("{id}.png"));
    if std::fs::create_dir_all(&folder).is_err()
        || tk::replace_with(&folder.join(format!(".{id}.part")), &path, data).is_err()
    {
        return tool_text("No map: it could not be kept.", true);
    }
    let line = arguments.get("line").and_then(Value::as_bool).unwrap_or(false);
    let directions = arguments.get("directions").and_then(Value::as_str).map(str::trim);
    let card = figure(&id, &drawn, line, directions);
    tool_text(
        &format!(
            "MEDIA:{PREFIX}/{agent}/{id}.png\nMap ready: {id} ({} markers). To show the picture alone, put the MEDIA line \
above alone on its own line in your reply, exactly as it is. In a page, paste this as it is into create_page's HTML (a \
live map in the app, its picture elsewhere):\n{card}",
            drawn.places.len()
        ),
        false,
    )
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
        };
        let card = figure("m-1", &drawn, true, Some("walking"));
        assert_eq!(
            card,
            "<figure class=\"map-card\" data-line=\"1\" data-directions=\"walking\"><img src=\"map:m-1\" alt=\"Carte\"><ol>\
<li data-x=\"10\" data-y=\"20.5\" data-lat=\"45.700000\" data-lon=\"4.850000\" data-color=\"#e5484d\">Départ &lt;A&gt;</li>\
<li data-x=\"10\" data-y=\"20.5\" data-lat=\"45.710000\" data-lon=\"4.850000\" data-color=\"#e5484d\">Fin</li></ol></figure>"
        );
        // an unknown mode, or a single place: no directions, no line
        let alone = Drawn { places: vec![place("A", 1.0)], ..drawn };
        let card = figure("m-1", &alone, true, Some("walking"));
        assert!(!card.contains("data-line") && !card.contains("data-directions"));
        let empty = Drawn { png: Vec::new(), places: Vec::new(), center: Some((45.5, 4.25)), zoom: 12 };
        assert!(figure("m-2", &empty, false, Some("teleport"))
            .starts_with("<figure class=\"map-card\" data-center=\"45.500000,4.250000\" data-zoom=\"12\"><img"));
    }
}
