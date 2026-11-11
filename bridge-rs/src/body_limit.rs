//! How much a request may send (audit 2026-10-27 #10). One limit for the whole app, set before routing so that it
//! holds for every way a route reads its body (axum's extractors, `to_bytes`, a stream): 4 MB, raised only on the
//! few routes that take large bodies, each behind its own check:
//!
//! - `POST /v1/files`: `[limits] upload_max_mb`, by its own middleware (files.rs `upload_size_limit`);
//! - `PUT /v1/import/archive`: a whole install sent from a computer, streamed to the disk, up to the size it announced
//!   with its proofs (place_import.rs);
//! - `/family/relay/answer/…`: a home's answer to a call its place kept for it, a shared file in it (64 MB);
//! - the imports the app sends in one body (notes, WhatsApp, another assistant: 60 MB base64-encoded) and the health
//!   days, behind the bridge key.
//!
//! A body announced larger is refused with 413 before it is read; a body that grows past its limit while it is read
//! fails there (413 from the extractors and `web::read_body`).

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Every route but the ones below.
pub const DEFAULT_BYTES: u64 = 4 << 20;
/// A home's answer relayed by its place (family_relay.rs).
pub const RELAY_ANSWER_BYTES: u64 = 64 << 20;
/// The imports sent in one body: 60 MB of files, base64-encoded in JSON (notes_import.rs, whatsapp_import, assistant_import).
pub const IMPORT_BYTES: u64 = 96 << 20;
/// The health days the phone sends (health.rs).
pub const HEALTH_BYTES: u64 = 16 << 20;

/// The limit for `path`, None when the route keeps its own.
pub fn limit_for(path: &str) -> Option<u64> {
    if path == crate::files::UPLOAD_PATH || path == "/v1/import/archive" {
        return None;
    }
    // a home's answer to a call its place kept for it (family_relay.rs): a shared file can be in it
    if path.starts_with("/family/relay/answer/") {
        return Some(RELAY_ANSWER_BYTES);
    }
    let import = path.starts_with("/v1/import/whatsapp/")
        || path.starts_with("/v1/assistant-import/")
        || (path.starts_with("/v1/agents/") && path.ends_with("/memory/import"));
    if import {
        return Some(IMPORT_BYTES);
    }
    // a paper's photo (notebook/papers.rs), in the JSON
    if path == "/v1/health-data/days" || path == "/v1/notebook/paper" {
        return Some(HEALTH_BYTES);
    }
    Some(DEFAULT_BYTES)
}

/// 413 with FastAPI's `{"detail"}`; the connection closed (the rest of the body is not read).
pub fn too_large_response() -> Response {
    let body = "{\"detail\":\"request body too large\"}";
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("content-length", HeaderValue::from(body.len()));
    headers.insert("connection", HeaderValue::from_static("close"));
    response
}

/// True when reading a body failed because it went past its limit.
pub fn is_too_large(err: &axum::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        if e.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        source = e.source();
    }
    false
}

/// The middleware (app.rs): by Content-Length first, then while the body is read.
pub async fn body_limit(request: Request, next: Next) -> Response {
    let Some(limit) = limit_for(request.uri().path()) else {
        return next.run(request).await;
    };
    let announced = request
        .headers()
        .get_all("content-length")
        .iter()
        .filter_map(|v| v.to_str().ok().and_then(|s| s.trim().parse::<u64>().ok()))
        .max();
    if announced.is_some_and(|n| n > limit) {
        return too_large_response();
    }
    let (parts, body) = request.into_parts();
    let body = Body::new(http_body_util::Limited::new(body, limit as usize));
    next.run(Request::from_parts(parts, body)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_by_route() {
        assert_eq!(limit_for("/v1/files"), None);
        assert_eq!(limit_for("/v1/import/archive"), None);
        assert_eq!(limit_for("/family/accept"), Some(DEFAULT_BYTES));
        assert_eq!(limit_for("/v1/agents/vie/memory/import"), Some(IMPORT_BYTES));
        assert_eq!(limit_for("/v1/agents/vie/memory"), Some(DEFAULT_BYTES));
        assert_eq!(limit_for("/v1/import/whatsapp/parse"), Some(IMPORT_BYTES));
        assert_eq!(limit_for("/family/relay/answer/x"), Some(RELAY_ANSWER_BYTES));
        assert_eq!(limit_for("/hermes/vie/v1/runs"), Some(DEFAULT_BYTES));
    }
}
