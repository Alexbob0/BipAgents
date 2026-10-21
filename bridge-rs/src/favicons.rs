//! Site icons for the apps, fetched here rather than by the phone: the links in the agents' replies show their site's
//! icon, and DuckDuckGo's icon service (which serves them) sees this server's address, never the phone's.
//!
//! - `GET /v1/favicon/{host}` -> the icon (kept 30 days on disk), 404 when the site has none
//!
//! Port of `bridge/bipbridge/favicons.py` (owner group: tools-knowledge).

use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use regex::Regex;

use crate::app::AppState;
use crate::config::Config;
use crate::py;
use crate::tools_knowledge as tk;
use crate::web::{ApiResult, HttpError};

pub const KEEP_SECONDS: u64 = 30 * 24 * 3600;
/// A site without an icon is asked again the next day.
pub const MISSING_SECONDS: u64 = 24 * 3600;
pub const MAX_BYTES: usize = 256 * 1024;
/// DuckDuckGo's icon service (`<base>/{host}.ico`).
pub const ICON_SERVICE: &str = "https://icons.duckduckgo.com/ip3";

/// `^(?=.{1,253}$)(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{0,62}$` (the length checked apart).
static HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{0,62}$").expect("valid regex")
});
const TYPES: [(&[u8], &str); 5] = [
    (b"\x89PNG", "image/png"),
    (b"GIF8", "image/gif"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"\x00\x00\x01\x00", "image/x-icon"),
    (b"RIFF", "image/webp"),
];

pub fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("favicons")
}

pub fn image_type(data: &[u8]) -> &'static str {
    TYPES.iter().find(|(magic, _)| data.starts_with(magic)).map(|(_, kind)| *kind).unwrap_or("")
}

fn icon_response(data: Vec<u8>, media_type: &str) -> Response {
    let mut response = Response::new(Body::from(data));
    let headers = response.headers_mut();
    headers.insert("cache-control", HeaderValue::from_static("private, max-age=86400"));
    if let Ok(v) = HeaderValue::from_str(media_type) {
        headers.insert("content-type", v);
    }
    response
}

fn valid_host(host: &str) -> bool {
    let length = host.chars().count();
    (1..=253).contains(&length) && HOST.is_match(host)
}

/// Seconds since the file last changed (None: no such file).
fn file_age(path: &std::path::Path, now: f64) -> Option<f64> {
    tk::mtime(path).map(|m| now - m)
}

async fn favicon(State(state): State<AppState>, Path(host): Path<String>) -> ApiResult<Response> {
    let host = tk::strip(&host).to_lowercase().trim_end_matches('.').to_string();
    if !valid_host(&host) {
        return Err(HttpError::new(400, "not a site name"));
    }
    let cache = folder(&state.config());
    let (icon, missing) = (cache.join(format!("{host}.icon")), cache.join(format!("{host}.missing")));
    let now = py::time_time();
    if icon.exists() && file_age(&icon, now).map(|a| a < KEEP_SECONDS as f64).unwrap_or(false) {
        let data = tokio::fs::read(&icon).await.map_err(anyhow::Error::from)?;
        let kind = Some(image_type(&data)).filter(|k| !k.is_empty()).unwrap_or("image/x-icon");
        return Ok(icon_response(data, kind));
    }
    if missing.exists() && file_age(&missing, now).map(|a| a < MISSING_SECONDS as f64).unwrap_or(false) {
        return Err(HttpError::new(404, "no icon"));
    }
    let answer = state.http.get(format!("{ICON_SERVICE}/{host}.ico")).timeout(Duration::from_secs(8)).send().await;
    let data = match answer {
        Err(_) => return Err(HttpError::new(502, "icon service unreachable")),
        Ok(resp) if resp.status().as_u16() == 200 => match resp.bytes().await {
            Ok(bytes) => bytes.to_vec(),
            Err(_) => return Err(HttpError::new(502, "icon service unreachable")),
        },
        Ok(_) => Vec::new(),
    };
    tokio::fs::create_dir_all(&cache).await.map_err(anyhow::Error::from)?;
    if data.is_empty() || data.len() > MAX_BYTES || image_type(&data).is_empty() {
        // `missing.touch()`: created, or its time renewed
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&missing).map_err(anyhow::Error::from)?;
        file.set_modified(std::time::SystemTime::now()).map_err(anyhow::Error::from)?;
        return Err(HttpError::new(404, "no icon"));
    }
    let partial = cache.join(format!("{host}.part"));
    tokio::fs::write(&partial, &data).await.map_err(anyhow::Error::from)?;
    tokio::fs::rename(&partial, &icon).await.map_err(anyhow::Error::from)?;
    let _ = tokio::fs::remove_file(&missing).await;
    let kind = image_type(&data);
    Ok(icon_response(data, kind))
}

/// `favicons_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new().route("/favicon/{host}", get(favicon))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_types() {
        assert!(valid_host("example.com"));
        assert!(valid_host("a.b-c.fr"));
        assert!(!valid_host("localhost"));
        assert!(!valid_host("exa_mple.com"));
        assert!(!valid_host(&format!("{}.com", "a".repeat(260))));
        assert_eq!(image_type(b"\x89PNG\r\n"), "image/png");
        assert_eq!(image_type(b"<html>"), "");
    }
}
