//! Interactive maps in the agents' pages (owner-approved 2026-10-23): pan, pinch, numbered markers, drawn by MapLibre
//! GL JS in the page with OpenFreeMap's vector tiles (OpenStreetMap data, free, no key, commercial use allowed). The
//! phone never talks to OpenFreeMap: the bridge fetches and keeps everything a map needs (its style, rewritten so that
//! every address in it is `bipmap://map/…`, the tiles, the fonts' glyphs, the sprite) and the apps serve `bipmap:`
//! to the page themselves from these routes, with their usual key. The library itself ships in the apps.
//!
//! - `GET /v1/maps/style.json` -> the style, its sources, glyphs and sprite pointing at `bipmap://map/…`
//! - `GET /v1/maps/tiles/{source}/{z}/{x}/{y}.pbf` -> a vector tile (z up to the source's maxzoom, at most 15)
//! - `GET /v1/maps/fonts/{fontstack}/{start}-{end}.pbf` -> glyphs, only the style's own fonts and whole 256 ranges
//! - `GET /v1/maps/sprite/{sprite|sprite@2x}.{json|png}` -> the style's icons
//!
//! Bounded on purpose: a page could be talked into asking tiles to spell something. Only these four kinds of address
//! exist, every number is checked (a tile inside the world at a zoom the source has, a glyph range the font has), every
//! upstream address comes from the configured provider's own style (never from the page), and misses spend a budget
//! (`BURST` at once, `REFILL` a second, `PARALLEL` at a time): past it the answer is 429 and nothing is asked upstream.
//! The answers are kept on disk (`CACHE_DAYS`, at most `MAX_CACHE_BYTES`), so a map seen once opens again offline.
//! `[maps] vector = "openfreemap"` (or « off »), `vector_url` (tests: a fake), `vector_style` (« positron »).
//!
//! Not a Python module: Rust bridge only (owner group: tools-knowledge).

use std::collections::BTreeSet;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::Config;
use crate::web::{ApiResult, HttpError};

/// What the pages and the apps call the bridge's map files.
pub const SCHEME_BASE: &str = "bipmap://map/";
pub const ATTRIBUTION: &str = "© OpenFreeMap © OpenMapTiles © OpenStreetMap contributors";
pub const CACHE_DAYS: u64 = 30;
/// The style and the sources' TileJSON are asked again after a day.
const STYLE_SECONDS: u64 = 86400;
pub const MAX_ZOOM: u32 = 15;
const MAX_CACHE_BYTES: u64 = 512 * 1024 * 1024;
/// Misses at once, then per second (OpenFreeMap has no hard limit; this keeps a household far below anything rude).
pub const BURST: f64 = 400.0;
pub const REFILL: f64 = 8.0;
const PARALLEL: usize = 6;
const MAX_BYTES: usize = 4 * 1024 * 1024;

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9_-]{1,40}$").expect("valid regex"));
static RANGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,5})-(\d{1,5})\.pbf$").expect("valid regex"));
static SPRITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^sprite(@2x)?\.(json|png)$").expect("valid regex"));

/// The style as the pages get it, with what the bridge needs to fetch for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rewritten {
    pub style: Value,
    /// source name -> (its upstream tile address with {z}/{x}/{y}, its min and max zoom)
    pub tiles: Vec<(String, String, u32, u32)>,
    pub glyphs: Option<String>,
    pub sprite: Option<String>,
    pub fonts: BTreeSet<String>,
}

struct Known {
    key: String,
    at: Instant,
    rewritten: Rewritten,
}

static KNOWN: LazyLock<tokio::sync::Mutex<Option<Known>>> = LazyLock::new(|| tokio::sync::Mutex::new(None));
static PARALLEL_TURNS: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(PARALLEL));
static BUDGET: LazyLock<std::sync::Mutex<(f64, Instant)>> =
    LazyLock::new(|| std::sync::Mutex::new((BURST, Instant::now())));
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// Upstream answers come as they are (gzip kept: smaller on disk and to the phone).
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(crate::weather::user_agent())
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default()
});

fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("map-vector")
}

/// `scheme://host[:port]` of an address.
fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{}://{}", scheme.to_lowercase(), host.to_lowercase()))
}

/// Upstream addresses are followed only on the configured provider's own host.
fn allowed(config: &Config, url: &str) -> bool {
    origin(url).is_some() && origin(url) == origin(&config.maps.vector_url)
}

fn short_hash(text: &str) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()))[..12].to_string()
}

/// One upstream miss, if the budget allows it.
fn spend() -> bool {
    let mut budget = BUDGET.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let refilled = (budget.0 + now.duration_since(budget.1).as_secs_f64() * REFILL).min(BURST);
    if refilled < 1.0 {
        *budget = (refilled, now);
        return false;
    }
    *budget = (refilled - 1.0, now);
    true
}

fn fresh(path: &FsPath, seconds: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(seconds))
}

fn keep(path: &FsPath, data: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
        let temporary = path.with_extension(format!("part{}", rand::random::<u32>()));
        if crate::tools_knowledge::replace_with(&temporary, path, data).is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
    }
}

/// A file from upstream, kept under `path` for `seconds` (an older copy when upstream fails). `None`: upstream has
/// nothing there (404/204). `Err`: unreachable or over budget, with the status to answer.
async fn fetch(config: &Config, url: &str, path: &FsPath, seconds: u64) -> Result<Option<Vec<u8>>, u16> {
    if fresh(path, seconds) {
        if let Ok(data) = tokio::fs::read(path).await {
            return Ok(Some(data));
        }
    }
    let stale = || std::fs::read(path).ok();
    if !allowed(config, url) {
        return Err(502);
    }
    if !spend() {
        return stale().map(Some).ok_or(429);
    }
    let Ok(_turn) = PARALLEL_TURNS.acquire().await else { return Err(503) };
    let answer = HTTP.get(url).header("accept-encoding", "gzip").send().await;
    let response = match answer {
        Ok(r) => r,
        Err(_) => return stale().map(Some).ok_or(502),
    };
    let status = response.status().as_u16();
    if status == 404 || status == 204 {
        return Ok(None);
    }
    if status != 200 {
        return stale().map(Some).ok_or(502);
    }
    let gzipped = response
        .headers()
        .get("content-encoding")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("gzip"));
    let encoding = response.headers().get("content-encoding").and_then(|v| v.to_str().ok()).map(str::to_string);
    if encoding.is_some() && !gzipped {
        return stale().map(Some).ok_or(502);
    }
    let Ok(body) = response.bytes().await else { return stale().map(Some).ok_or(502) };
    if body.len() > MAX_BYTES {
        return Err(502);
    }
    let data = body.to_vec();
    keep(path, &data);
    if WRITES.fetch_add(1, Ordering::Relaxed) % 256 == 255 {
        let root = folder(config);
        tokio::task::spawn_blocking(move || prune(&root, MAX_CACHE_BYTES));
    }
    Ok(Some(data))
}

/// The oldest files out until the cache is under 80 % of `limit`.
pub fn prune(root: &FsPath, limit: u64) {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()));
            }
        }
    }
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    if total <= limit {
        return;
    }
    files.sort();
    for (_, size, path) in files {
        if total <= limit / 10 * 8 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// Plain JSON text of a file kept gzipped or not.
fn text_of(data: &[u8]) -> Option<String> {
    if data.starts_with(&[0x1f, 0x8b]) {
        use std::io::Read;
        let mut out = String::new();
        flate2::read::GzDecoder::new(data).read_to_string(&mut out).ok()?;
        Some(out)
    } else {
        String::from_utf8(data.to_vec()).ok()
    }
}

fn collect_fonts(value: &Value, into: &mut BTreeSet<String>) {
    match value {
        // a font's name (« Noto Sans Bold »), not an expression's operator (« step », « literal »)
        Value::String(s)
            if s.len() <= 60
                && s.contains(' ')
                && s.chars().all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_') =>
        {
            into.insert(s.clone());
        }
        Value::Array(items) => items.iter().for_each(|v| collect_fonts(v, into)),
        _ => {}
    }
}

/// The provider's style made for the pages: only its vector sources (each one's tiles at `bipmap://map/tiles/<name>/`),
/// the layers drawing them, its glyphs and sprite at `bipmap://map/…`, the attribution in plain words. `tilejson`
/// gives a vector source's TileJSON when the style names it by `url`.
pub fn rewrite(style: &Value, tilejson: &dyn Fn(&str) -> Option<Value>) -> Option<Rewritten> {
    let object = style.as_object()?;
    let mut out = Rewritten::default();
    let mut sources = Map::new();
    for (name, source) in object.get("sources")?.as_object()? {
        if source.get("type").and_then(Value::as_str) != Some("vector") || !NAME.is_match(name) {
            continue;
        }
        let described = match source.get("url").and_then(Value::as_str) {
            // a source whose description could not be had (elsewhere than the provider's host): left out
            Some(url) => match tilejson(url) {
                Some(described) => described,
                None => continue,
            },
            None => source.clone(),
        };
        let Some(template) =
            described.get("tiles").and_then(Value::as_array).and_then(|t| t.first()).and_then(Value::as_str)
        else {
            continue;
        };
        let minzoom = described.get("minzoom").and_then(Value::as_u64).unwrap_or(0).min(MAX_ZOOM as u64) as u32;
        let maxzoom =
            described.get("maxzoom").and_then(Value::as_u64).unwrap_or(14).clamp(minzoom as u64, MAX_ZOOM as u64)
                as u32;
        out.tiles.push((name.clone(), template.to_string(), minzoom, maxzoom));
        sources.insert(
            name.clone(),
            json!({"type": "vector", "tiles": [format!("{SCHEME_BASE}tiles/{name}/{{z}}/{{x}}/{{y}}.pbf")],
                   "minzoom": minzoom, "maxzoom": maxzoom, "attribution": ATTRIBUTION}),
        );
    }
    if sources.is_empty() {
        return None;
    }
    let mut layers = Vec::new();
    for layer in object.get("layers").and_then(Value::as_array).into_iter().flatten() {
        let source = layer.get("source").and_then(Value::as_str);
        if source.is_some_and(|s| !sources.contains_key(s)) {
            continue;
        }
        if let Some(font) = layer.get("layout").and_then(|l| l.get("text-font")) {
            collect_fonts(font, &mut out.fonts);
        }
        layers.push(layer.clone());
    }
    let mut style = Map::new();
    style.insert("version".into(), json!(8));
    style.insert("sources".into(), Value::Object(sources));
    style.insert("layers".into(), Value::Array(layers));
    if let Some(glyphs) = object.get("glyphs").and_then(Value::as_str) {
        out.glyphs = Some(glyphs.to_string());
        style.insert("glyphs".into(), json!(format!("{SCHEME_BASE}fonts/{{fontstack}}/{{range}}.pbf")));
    }
    if let Some(sprite) = object.get("sprite").and_then(Value::as_str) {
        out.sprite = Some(sprite.to_string());
        style.insert("sprite".into(), json!(format!("{SCHEME_BASE}sprite/sprite")));
    }
    out.style = Value::Object(style);
    Some(out)
}

fn absolute(base: &str, url: &str) -> String {
    if url.contains("://") {
        url.to_string()
    } else if let Some(origin) = origin(base) {
        format!("{origin}/{}", url.trim_start_matches('/'))
    } else {
        url.to_string()
    }
}

/// The current style (asked again after a day; the last one kept when the provider does not answer).
async fn current(config: &Config) -> Result<Rewritten, HttpError> {
    if config.maps.vector != "openfreemap" {
        return Err(HttpError::not_found());
    }
    let key = format!("{}|{}", config.maps.vector_url, config.maps.vector_style);
    let mut known = KNOWN.lock().await;
    if let Some(k) = known.as_ref().filter(|k| k.key == key && k.at.elapsed() < Duration::from_secs(STYLE_SECONDS)) {
        return Ok(k.rewritten.clone());
    }
    let root = folder(config);
    let base = config.maps.vector_url.trim_end_matches('/');
    let style_url = format!("{base}/styles/{}", config.maps.vector_style);
    let style_path = root.join(format!("style-{}.json", short_hash(&key)));
    let style = fetch(config, &style_url, &style_path, STYLE_SECONDS).await;
    let style = match style {
        Ok(Some(data)) => text_of(&data).and_then(|t| serde_json::from_str::<Value>(&t).ok()),
        _ => None,
    };
    let Some(style) = style else {
        return match known.as_ref().filter(|k| k.key == key) {
            Some(k) => Ok(k.rewritten.clone()),
            None => Err(HttpError::new(502, "the map style is unreachable")),
        };
    };
    // the sources' TileJSON, fetched before the rewrite (it is synchronous)
    let mut described = std::collections::HashMap::new();
    for source in style.get("sources").and_then(Value::as_object).into_iter().flat_map(|m| m.values()) {
        if let Some(url) = source.get("url").and_then(Value::as_str) {
            let url = absolute(&style_url, url);
            let path = root.join(format!("tilejson-{}.json", short_hash(&url)));
            if let Ok(Some(data)) = fetch(config, &url, &path, STYLE_SECONDS).await {
                if let Some(v) = text_of(&data).and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                    described.insert(url, v);
                }
            }
        }
    }
    let rewritten = rewrite(&style, &|url| described.get(&absolute(&style_url, url)).cloned())
        .ok_or_else(|| HttpError::new(502, "the map style has no vector source"))?;
    let mut rewritten = rewritten;
    for tile in rewritten.tiles.iter_mut() {
        tile.1 = absolute(&style_url, &tile.1);
    }
    rewritten.glyphs = rewritten.glyphs.map(|g| absolute(&style_url, &g));
    rewritten.sprite = rewritten.sprite.map(|s| absolute(&style_url, &s));
    *known = Some(Known { key, at: Instant::now(), rewritten: rewritten.clone() });
    Ok(rewritten)
}

fn answer(data: Vec<u8>, kind: &'static str, seconds: u64) -> Response {
    let gzipped = data.starts_with(&[0x1f, 0x8b]);
    let mut response = Response::new(Body::from(data));
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static(kind));
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    if gzipped {
        headers.insert("content-encoding", HeaderValue::from_static("gzip"));
    }
    if let Ok(v) = HeaderValue::from_str(&format!("private, max-age={seconds}")) {
        headers.insert("cache-control", v);
    }
    response
}

fn empty(seconds: u64) -> Response {
    let mut response = answer(Vec::new(), "application/x-protobuf", seconds);
    *response.status_mut() = axum::http::StatusCode::NO_CONTENT;
    response
}

fn failed(status: u16) -> HttpError {
    match status {
        429 => HttpError::new(429, "too many map requests, try again in a moment"),
        _ => HttpError::new(502, "the map tiles are unreachable"),
    }
}

async fn style(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let rewritten = current(&config).await?;
    let text = serde_json::to_vec(&rewritten.style).map_err(anyhow::Error::from)?;
    Ok(answer(text, "application/json", 3600))
}

/// The tile `z/x/y` of a source the style has, at a zoom it has (None: no such tile).
pub fn tile_address(rewritten: &Rewritten, source: &str, z: u32, x: u64, y: u64) -> Option<String> {
    let (_, template, minzoom, maxzoom) = rewritten.tiles.iter().find(|t| t.0 == source)?;
    if z < *minzoom || z > *maxzoom || x >= 1u64 << z || y >= 1u64 << z {
        return None;
    }
    Some(template.replace("{z}", &z.to_string()).replace("{x}", &x.to_string()).replace("{y}", &y.to_string()))
}

async fn tile(
    State(state): State<AppState>,
    Path((source, z, x, file)): Path<(String, u32, u64, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let y: u64 = file.strip_suffix(".pbf").and_then(|y| y.parse().ok()).ok_or_else(HttpError::not_found)?;
    let rewritten = current(&config).await?;
    let url = tile_address(&rewritten, &source, z, x, y).ok_or_else(HttpError::not_found)?;
    let template = rewritten.tiles.iter().find(|t| t.0 == source).map(|t| short_hash(&t.1)).unwrap_or_default();
    let path = folder(&config).join("tiles").join(template).join(format!("{z}/{x}/{y}.pbf"));
    match fetch(&config, &url, &path, CACHE_DAYS * 86400).await {
        Ok(Some(data)) => Ok(answer(data, "application/x-protobuf", 7 * 86400)),
        Ok(None) => Ok(empty(7 * 86400)),
        Err(status) => Err(failed(status)),
    }
}

/// A glyph range of fonts the style uses (`Noto Sans Regular,Noto Sans Bold`), a whole block of 256.
pub fn glyph_address(rewritten: &Rewritten, fontstack: &str, range: &str) -> Option<String> {
    let template = rewritten.glyphs.as_ref()?;
    let found = RANGE.captures(range)?;
    let (start, end): (u32, u32) = (found[1].parse().ok()?, found[2].parse().ok()?);
    if start % 256 != 0 || end != start + 255 || end > 65535 {
        return None;
    }
    let fonts: Vec<&str> = fontstack.split(',').map(str::trim).collect();
    if fonts.is_empty() || fonts.len() > 3 || fonts.iter().any(|f| !rewritten.fonts.contains(*f)) {
        return None;
    }
    let stack = fonts.join(",").replace(' ', "%20");
    Some(template.replace("{fontstack}", &stack).replace("{range}", &format!("{start}-{end}")))
}

async fn glyphs(
    State(state): State<AppState>,
    Path((fontstack, range)): Path<(String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let rewritten = current(&config).await?;
    let url = glyph_address(&rewritten, &fontstack, &range).ok_or_else(HttpError::not_found)?;
    let path = folder(&config).join("fonts").join(format!("{}.pbf", short_hash(&url)));
    match fetch(&config, &url, &path, CACHE_DAYS * 86400).await {
        Ok(Some(data)) => Ok(answer(data, "application/x-protobuf", 7 * 86400)),
        Ok(None) => Ok(empty(86400)),
        Err(status) => Err(failed(status)),
    }
}

async fn sprite(State(state): State<AppState>, Path(file): Path<String>) -> ApiResult<Response> {
    let config = state.config();
    let found = SPRITE.captures(&file).ok_or_else(HttpError::not_found)?;
    let rewritten = current(&config).await?;
    let base = rewritten.sprite.as_ref().ok_or_else(HttpError::not_found)?;
    let retina = found.get(1).map(|m| m.as_str()).unwrap_or("");
    let (extension, kind) = if &found[2] == "png" { ("png", "image/png") } else { ("json", "application/json") };
    let url = format!("{base}{retina}.{extension}");
    let path = folder(&config).join("sprite").join(format!("{}.{extension}", short_hash(&url)));
    match fetch(&config, &url, &path, CACHE_DAYS * 86400).await {
        Ok(Some(data)) => Ok(answer(data, kind, 7 * 86400)),
        Ok(None) => Err(HttpError::not_found()),
        Err(status) => Err(failed(status)),
    }
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/maps/style.json", get(style))
        .route("/maps/tiles/{source}/{z}/{x}/{file}", get(tile))
        .route("/maps/fonts/{fontstack}/{range}", get(glyphs))
        .route("/maps/sprite/{file}", get(sprite))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "version": 8,
            "sources": {
                "ne2_shaded": {"type": "raster", "tiles": ["https://t.example/ne/{z}/{x}/{y}.png"]},
                "openmaptiles": {"type": "vector", "url": "https://t.example/planet"}
            },
            "sprite": "https://t.example/sprites/ofm",
            "glyphs": "https://t.example/fonts/{fontstack}/{range}.pbf",
            "layers": [
                {"id": "background", "type": "background"},
                {"id": "relief", "type": "raster", "source": "ne2_shaded"},
                {"id": "label", "type": "symbol", "source": "openmaptiles",
                 "layout": {"text-font": ["Noto Sans Regular"]}},
                {"id": "bold", "type": "symbol", "source": "openmaptiles",
                 "layout": {"text-font": ["step", ["zoom"], ["literal", ["Noto Sans Bold"]], 10, ["literal", ["Noto Sans Italic"]]]}}
            ]
        })
    }

    #[test]
    fn the_style_points_only_at_bipmap() {
        let tilejson = |url: &str| {
            (url == "https://t.example/planet")
                .then(|| json!({"tiles": ["https://t.example/planet/v1/{z}/{x}/{y}.pbf"], "maxzoom": 14}))
        };
        let out = rewrite(&sample(), &tilejson).unwrap();
        let text = out.style.to_string();
        assert!(!text.contains("t.example"), "{text}");
        assert_eq!(out.style["sources"].as_object().unwrap().len(), 1);
        assert_eq!(out.style["sources"]["openmaptiles"]["tiles"][0], "bipmap://map/tiles/openmaptiles/{z}/{x}/{y}.pbf");
        assert_eq!(out.style["glyphs"], "bipmap://map/fonts/{fontstack}/{range}.pbf");
        assert_eq!(out.style["sprite"], "bipmap://map/sprite/sprite");
        let ids: Vec<&str> =
            out.style["layers"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["background", "label", "bold"]);
        assert_eq!(
            out.fonts.iter().map(String::as_str).collect::<Vec<_>>(),
            ["Noto Sans Bold", "Noto Sans Italic", "Noto Sans Regular"]
        );
        // tiles: only inside the world, at the source's zooms
        assert_eq!(
            tile_address(&out, "openmaptiles", 14, 8418, 5843).as_deref(),
            Some("https://t.example/planet/v1/14/8418/5843.pbf")
        );
        assert!(tile_address(&out, "openmaptiles", 15, 0, 0).is_none());
        assert!(tile_address(&out, "openmaptiles", 2, 4, 0).is_none());
        assert!(tile_address(&out, "ne2_shaded", 2, 0, 0).is_none());
        // glyphs: the style's fonts, whole blocks
        assert_eq!(
            glyph_address(&out, "Noto Sans Regular,Noto Sans Bold", "256-511.pbf").as_deref(),
            Some("https://t.example/fonts/Noto%20Sans%20Regular,Noto%20Sans%20Bold/256-511.pbf")
        );
        assert!(glyph_address(&out, "Noto Sans Regular", "0-300.pbf").is_none());
        assert!(glyph_address(&out, "Comic Sans", "0-255.pbf").is_none());
        assert!(glyph_address(&out, "Noto Sans Regular", "65536-65791.pbf").is_none());
        // a style without a vector source is no map
        assert!(rewrite(&json!({"sources": {}, "layers": []}), &tilejson).is_none());
    }

    #[test]
    fn origins_and_budget() {
        assert_eq!(origin("https://Tiles.Example:8443/styles/x").as_deref(), Some("https://tiles.example:8443"));
        assert_eq!(absolute("https://a.example/styles/x", "/planet"), "https://a.example/planet");
        assert!(SPRITE.is_match("sprite@2x.png") && !SPRITE.is_match("../x.png"));
        let mut spent = 0;
        while spend() && spent < 10_000 {
            spent += 1;
        }
        assert!((BURST as usize - 1..=BURST as usize + 50).contains(&spent), "{spent}");
    }

    #[test]
    fn prune_keeps_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            let path = dir.path().join(format!("t/{i}.pbf"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            let when = SystemTime::now() - Duration::from_secs(1000 - i * 10);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        }
        prune(dir.path(), 500);
        let left: Vec<_> = std::fs::read_dir(dir.path().join("t")).unwrap().flatten().collect();
        assert_eq!(left.len(), 4);
        assert!(dir.path().join("t/9.pbf").exists() && !dir.path().join("t/0.pbf").exists());
    }
}
