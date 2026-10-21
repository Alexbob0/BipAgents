//! The Python package's data files, embedded from `../bridge/bipbridge` (one source of truth): the web console's
//! page and scripts, noVNC and the vendored Chart.js, the agents' skills, and the YAML catalogs. Plus
//! [`static_router`], Starlette's `StaticFiles` for an embedded folder.
//!
//! Not a Python module: shared plumbing (core group).

use axum::extract::{Path, Request};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;
use include_dir::{include_dir, Dir};
use sha2::{Digest, Sha256};

use crate::app::AppState;

/// `bipbridge/console/` (index.html, static/…).
pub static CONSOLE: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../bridge/bipbridge/console");
/// `bipbridge/static/` (novnc/, vendor/chart.umd.min.js).
pub static STATIC: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../bridge/bipbridge/static");
/// `bipbridge/skills/` (visible-browser/SKILL.md, shared-data/SKILL.md).
pub static SKILLS: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../bridge/bipbridge/skills");
/// `bipbridge/agent_templates.yaml` (souls.py).
pub const AGENT_TEMPLATES_YAML: &str = include_str!("../../bridge/bipbridge/agent_templates.yaml");
/// `bipbridge/tool_catalog.yaml` (services.py).
pub const TOOL_CATALOG_YAML: &str = include_str!("../../bridge/bipbridge/tool_catalog.yaml");

/// A file of an embedded folder (`"core/rfb.js"`), as bytes.
pub fn file<'a>(dir: &'a Dir<'static>, path: &str) -> Option<&'a [u8]> {
    dir.get_file(path.trim_start_matches('/')).map(|f| f.contents())
}

/// A text file of an embedded folder.
pub fn text<'a>(dir: &'a Dir<'static>, path: &str) -> Option<&'a str> {
    dir.get_file(path.trim_start_matches('/')).and_then(|f| f.contents_utf8())
}

/// Python's `mimetypes.guess_type` for the files served here, with Starlette's `; charset=utf-8` on text.
pub fn guess_type(path: &str) -> String {
    let mime = mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream");
    let mime = match path.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("js") | Some("mjs") => "text/javascript",
        _ => mime,
    };
    if mime.starts_with("text/") {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    }
}

/// StaticFiles raises HTTPException(404/405) inside the FastAPI app: FastAPI's JSON answer, no `Allow`.
fn plain(status: StatusCode, text: &'static str) -> Response {
    crate::web::HttpError::new(status.as_u16(), text).into_response()
}

/// Starlette's `StaticFiles(directory=...)` over an embedded folder: GET/HEAD, 404 « Not Found » and 405 « Method
/// Not Allowed » as FastAPI's JSON, an ETag and `If-None-Match` → 304. Mount it with `Router::nest(prefix, ...)`.
pub fn static_router(dir: &'static Dir<'static>) -> Router<AppState> {
    let serve = move |path: Option<Path<String>>, request: Request| async move {
        let path = path.map(|Path(p)| p).unwrap_or_default();
        serve_file(dir, &path, &request)
    };
    Router::new().route("/{*path}", any(serve)).route("/", any(serve)).fallback(|request: Request| async move {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            plain(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed")
        } else {
            plain(StatusCode::NOT_FOUND, "Not Found")
        }
    })
}

fn serve_file(dir: &'static Dir<'static>, path: &str, request: &Request) -> Response {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
    }
    let clean = path.trim_start_matches('/');
    if clean.split('/').any(|part| part == "..") {
        return plain(StatusCode::NOT_FOUND, "Not Found");
    }
    // Entry paths are relative to the embedded root, whichever folder `dir` is.
    let Some(file) = dir.get_file(dir.path().join(clean)) else {
        return plain(StatusCode::NOT_FOUND, "Not Found");
    };
    let contents = file.contents();
    let etag = format!("\"{}\"", hex::encode(&Sha256::digest(contents)[..16]));
    if let Some(inm) = request.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if inm.split(',').any(|t| t.trim().trim_start_matches("W/") == etag) {
            let mut r = StatusCode::NOT_MODIFIED.into_response();
            r.headers_mut()
                .insert(header::ETAG, HeaderValue::from_str(&etag).unwrap_or(HeaderValue::from_static("\"\"")));
            return r;
        }
    }
    let mut response = if request.method() == Method::HEAD {
        Response::new(axum::body::Body::empty())
    } else {
        Response::new(axum::body::Body::from(contents))
    };
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&guess_type(clean)) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(contents.len()));
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, v);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_data_is_embedded() {
        assert!(text(&CONSOLE, "index.html").is_some());
        assert!(file(&STATIC, "vendor/chart.umd.min.js").is_some());
        assert!(text(&SKILLS, "visible-browser/SKILL.md").is_some());
        assert!(AGENT_TEMPLATES_YAML.contains("daily") || !AGENT_TEMPLATES_YAML.is_empty());
        assert!(!TOOL_CATALOG_YAML.is_empty());
        assert_eq!(guess_type("a/b.css"), "text/css; charset=utf-8");
        assert_eq!(guess_type("x.js"), "text/javascript; charset=utf-8");
    }
}
