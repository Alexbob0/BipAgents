//! The web console (`/console/`): the person's agents from a computer's browser, on the tailnet (or the LAN door), served
//! by this bridge. The page (`console/`) is static; it uses the same routes as the iPhone app, with a session instead of
//! the bridge key:
//!
//! - **Logging in**: a one-time link (`python -m bipbridge console`, valid 10 minutes, its code in the link's `#` so it
//!   never reaches a log) or a code the console shows and the person approves in the iPhone app
//!   (`POST /v1/console/pairings/{code}`). Either gives a session cookie: HttpOnly (no script reads it), Secure, SameSite
//!   Strict, 30 days, listed and revocable (`/v1/console/devices`).
//! - **Calling the bridge**: the cookie plus an `X-Bip-Console: 1` header (a page from elsewhere cannot send it without the
//!   browser asking first) stands for the bridge key (`auth.require_bridge_key`).
//! - **Calling an agent's Hermes**: through `/hermes/<agent>/…`, the bridge adding the agent's key itself: no Hermes key
//!   ever reaches the browser (`/v1/agents` leaves them out for a console).
//!
//! Port of `bridge/bipbridge/console.py` (owner group: browsers-places).

use std::path::PathBuf;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::Dict;

pub const COOKIE: &str = "bip_console";
pub const HEADER: &str = "x-bip-console";
pub const SESSION_DAYS: i64 = 30;
pub const LINK_MINUTES: i64 = 10;
pub const PAIRING_MINUTES: i64 = 5;

/// The page runs only its own scripts and styles, talks only to this bridge, and is never framed.
pub const SECURITY_HEADERS: [(&str, &str); 4] = [
    (
        "Content-Security-Policy",
        "default-src 'self'; img-src 'self' data: blob:; media-src 'self' blob:; connect-src 'self'; style-src 'self'; \
         script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
    ),
    ("X-Content-Type-Options", "nosniff"),
    ("Referrer-Policy", "no-referrer"),
    ("Cache-Control", "no-store"),
];

fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn as_f64(value: Option<&Value>) -> f64 {
    value.and_then(Value::as_f64).unwrap_or(0.0)
}

/// Sessions, one-time links and pairings, in `<data>/console.json` (0600): digests only, never a token.
pub struct ConsoleStore {
    pub path: PathBuf,
    /// Python's methods run one at a time on its event loop: here, a read-change-write is never interleaved.
    lock: Mutex<()>,
}

impl ConsoleStore {
    pub fn new(path: &str) -> Self {
        ConsoleStore { path: PathBuf::from(path), lock: Mutex::new(()) }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn load(&self) -> Dict {
        let data: Value = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| Value::Object(Map::new()));
        let now = crate::py::time_time();
        let section = |name: &str| -> Map<String, Value> {
            match data.get(name) {
                Some(Value::Object(map)) => map.clone(),
                _ => Map::new(),
            }
        };
        let sessions: Map<String, Value> =
            section("sessions").into_iter().filter(|(_, v)| as_f64(v.get("expires")) > now).collect();
        let links: Map<String, Value> =
            section("links").into_iter().filter(|(_, v)| v.as_f64().unwrap_or(0.0) > now).collect();
        let pairings: Map<String, Value> =
            section("pairings").into_iter().filter(|(_, v)| as_f64(v.get("expires")) > now).collect();
        let mut out = Dict::new();
        out.insert("sessions".into(), Value::Object(sessions));
        out.insert("links".into(), Value::Object(links));
        out.insert("pairings".into(), Value::Object(pairings));
        out
    }

    pub fn save(&self, data: &Dict) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = self.path.with_file_name(format!("{name}.tmp"));
        std::fs::write(&tmp, crate::py::Dumps::new().indent(1).dumps(&Value::Object(data.clone())))?;
        crate::py::chmod(&tmp.to_string_lossy(), 0o600)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn section<'a>(data: &'a mut Dict, name: &str) -> &'a mut Map<String, Value> {
        if !matches!(data.get(name), Some(Value::Object(_))) {
            data.insert(name.to_string(), Value::Object(Map::new()));
        }
        match data.get_mut(name) {
            Some(Value::Object(map)) => map,
            _ => unreachable!("just made"),
        }
    }

    // -- one-time links (`bipbridge console`)

    /// A one-time link code (`bipbridge console`), valid LINK_MINUTES.
    pub fn new_link(&self) -> anyhow::Result<String> {
        let _lock = self.guard();
        let code = crate::py::token_urlsafe(24);
        let mut data = self.load();
        Self::section(&mut data, "links")
            .insert(digest(&code), json!(crate::py::time_time() + (LINK_MINUTES * 60) as f64));
        self.save(&data)?;
        Ok(code)
    }

    pub fn use_link(&self, code: &str) -> bool {
        let _lock = self.guard();
        let mut data = self.load();
        if Self::section(&mut data, "links").shift_remove(&digest(code)).is_none() {
            return false;
        }
        let _ = self.save(&data);
        true
    }

    // -- pairings approved from the iPhone app

    /// A short code to read on the console, and the secret the console waits with.
    pub fn new_pairing(&self) -> anyhow::Result<Dict> {
        use rand::Rng;
        let _lock = self.guard();
        let code = format!("{:06}", rand::rngs::OsRng.gen_range(0..1_000_000u32));
        let wait = crate::py::token_urlsafe(24);
        let mut data = self.load();
        Self::section(&mut data, "pairings").insert(
            code.clone(),
            json!({"wait": digest(&wait), "expires": crate::py::time_time() + (PAIRING_MINUTES * 60) as f64, "approved": null}),
        );
        self.save(&data)?;
        let mut out = Dict::new();
        out.insert("code".into(), json!(code));
        out.insert("wait".into(), json!(wait));
        Ok(out)
    }

    pub fn approve(&self, code: &str, name: &str) -> bool {
        let _lock = self.guard();
        let mut data = self.load();
        let pairings = Self::section(&mut data, "pairings");
        let Some(Value::Object(pairing)) = pairings.get_mut(code) else { return false };
        if pairing.get("approved").map(crate::py::truthy).unwrap_or(false) {
            return false;
        }
        pairing.insert("approved".into(), json!(if name.is_empty() { "Navigateur" } else { name }));
        let _ = self.save(&data);
        true
    }

    /// The name the person gave once they approved it (the pairing is then used up), else None.
    pub fn take_pairing(&self, code: &str, wait: &str) -> Option<String> {
        let _lock = self.guard();
        let mut data = self.load();
        let pairings = Self::section(&mut data, "pairings");
        let pairing = pairings.get(code)?.as_object()?.clone();
        let stored = pairing.get("wait").map(crate::py::py_str).unwrap_or_default();
        if !crate::auth::constant_time_eq(stored.as_bytes(), digest(wait).as_bytes()) {
            return None;
        }
        let approved = pairing.get("approved").cloned().unwrap_or(Value::Null);
        if !crate::py::truthy(&approved) {
            return None;
        }
        pairings.shift_remove(code);
        let _ = self.save(&data);
        Some(crate::py::py_str(&approved))
    }

    // -- sessions

    pub fn new_session(&self, name: &str) -> anyhow::Result<String> {
        let _lock = self.guard();
        let token = crate::py::token_urlsafe(32);
        let mut data = self.load();
        let now = crate::py::time_time();
        let name: String = name.chars().take(60).collect();
        Self::section(&mut data, "sessions").insert(
            digest(&token),
            json!({"id": crate::py::token_hex(6), "name": name, "created": now, "seen": now,
                   "expires": now + (SESSION_DAYS * 86400) as f64}),
        );
        self.save(&data)?;
        Ok(token)
    }

    pub fn valid(&self, token: Option<&str>) -> bool {
        let Some(token) = token.filter(|t| !t.is_empty()) else { return false };
        let _lock = self.guard();
        let mut data = self.load();
        let sessions = Self::section(&mut data, "sessions");
        let Some(session) = sessions.get_mut(&digest(token)) else { return false };
        let now = crate::py::time_time();
        if now - as_f64(session.get("seen")) > 3600.0 {
            // last use, kept to the hour
            if let Value::Object(map) = session {
                map.insert("seen".into(), json!(now));
            }
            let _ = self.save(&data);
        }
        true
    }

    pub fn end(&self, token: Option<&str>) {
        let Some(token) = token.filter(|t| !t.is_empty()) else { return };
        let _lock = self.guard();
        let mut data = self.load();
        if Self::section(&mut data, "sessions").shift_remove(&digest(token)).is_some() {
            let _ = self.save(&data);
        }
    }

    pub fn devices(&self) -> Vec<Dict> {
        let _lock = self.guard();
        let mut data = self.load();
        Self::section(&mut data, "sessions")
            .values()
            .map(|s| match s {
                Value::Object(map) => {
                    map.iter().filter(|(k, _)| k.as_str() != "expires").map(|(k, v)| (k.clone(), v.clone())).collect()
                }
                _ => Dict::new(),
            })
            .collect()
    }

    pub fn revoke(&self, device: &str) -> bool {
        let _lock = self.guard();
        let mut data = self.load();
        let sessions = Self::section(&mut data, "sessions");
        let found: Vec<String> = sessions
            .iter()
            .filter(|(_, s)| s.get("id").and_then(Value::as_str) == Some(device))
            .map(|(k, _)| k.clone())
            .collect();
        for key in &found {
            sessions.shift_remove(key);
        }
        if !found.is_empty() {
            let _ = self.save(&data);
        }
        !found.is_empty()
    }
}

/// A cookie's value from the `Cookie` headers (Starlette's `request.cookies`).
pub fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for value in headers.get_all("cookie") {
        let Ok(text) = value.to_str() else { continue };
        for part in text.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                if k.trim() == name {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

/// A console's call: its session cookie, and the header a page from elsewhere cannot send unasked.
pub fn is_console(state: &AppState, headers: &HeaderMap) -> bool {
    headers.get(HEADER).and_then(|v| v.to_str().ok()) == Some("1")
        && state.console.valid(cookie(headers, COOKIE).as_deref())
}

/// Starlette's `response.set_cookie(COOKIE, token, max_age=…, httponly=True, secure=True, samesite="strict", path="/")`.
fn session_cookie(token: &str) -> String {
    format!("{COOKIE}={token}; HttpOnly; Max-Age={}; Path=/; SameSite=strict; Secure", SESSION_DAYS * 86400)
}

/// Starlette's `response.delete_cookie(COOKIE, path="/")`.
fn deleted_cookie() -> String {
    format!("{COOKIE}=\"\"; expires={}; Max-Age=0; Path=/; SameSite=lax", web::httpdate(crate::py::time_time() as i64))
}

fn with_cookie(mut response: Response, cookie: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(cookie) {
        response.headers_mut().append("set-cookie", v);
    }
    response
}

#[derive(Debug, Deserialize)]
pub struct LinkLogin {
    pub code: String,
    #[serde(default = "browser_name")]
    pub name: String,
}

fn browser_name() -> String {
    "Navigateur".into()
}

impl Validate for LinkLogin {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "code", Some(&self.code), 100);
        check_max_len(errors, "name", Some(&self.name), 60);
    }
}

#[derive(Debug, Deserialize)]
pub struct PairingWait {
    pub code: String,
    pub wait: String,
}

impl Validate for PairingWait {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "code", Some(&self.code), 6);
        check_max_len(errors, "wait", Some(&self.wait), 100);
    }
}

#[derive(Debug, Deserialize)]
pub struct Approval {
    #[serde(default = "browser_name")]
    pub name: String,
}

impl Validate for Approval {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "name", Some(&self.name), 60);
    }
}

/// `FileResponse(PAGE / "index.html", headers=SECURITY_HEADERS)`, the page embedded: its etag and last-modified are
/// Starlette's (`md5("<mtime>-<size>")`), with this program's own file time standing for the page's.
fn page_response() -> Response {
    use md5::{Digest as _, Md5};
    let contents = crate::assets::file(&crate::assets::CONSOLE, "index.html").unwrap_or(b"");
    let mtime = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9)
        .unwrap_or(0.0);
    let etag = format!(
        "\"{}\"",
        hex::encode(Md5::digest(format!("{}-{}", crate::py::float_repr(mtime), contents.len()).as_bytes()))
    );
    let mut response = Response::new(axum::body::Body::from(contents));
    let h = response.headers_mut();
    for (name, value) in SECURITY_HEADERS {
        if let (Ok(n), Ok(v)) =
            (axum::http::HeaderName::from_bytes(name.to_ascii_lowercase().as_bytes()), HeaderValue::from_str(value))
        {
            h.insert(n, v);
        }
    }
    h.insert("content-type", HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert("content-length", HeaderValue::from(contents.len()));
    if let Ok(v) = HeaderValue::from_str(&web::httpdate(mtime as i64)) {
        h.insert("last-modified", v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert("etag", v);
    }
    h.insert("accept-ranges", HeaderValue::from_static("bytes"));
    response
}

async fn to_page() -> Response {
    let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
    response.headers_mut().insert("location", HeaderValue::from_static("/console/"));
    response.headers_mut().insert("content-length", HeaderValue::from_static("0"));
    response
}

async fn page() -> Response {
    page_response()
}

async fn login(State(state): State<AppState>, Json(body): Json<LinkLogin>) -> ApiResult<Response> {
    if !state.console.use_link(&body.code) {
        return Err(HttpError::new(401, "this link was used or has expired: ask for a new one"));
    }
    let token = state.console.new_session(&body.name)?;
    Ok(with_cookie(web::ok(json!({"ok": true})), &session_cookie(&token)))
}

async fn pairing(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(Value::Object(state.console.new_pairing()?)))
}

async fn pairing_wait(State(state): State<AppState>, Json(body): Json<PairingWait>) -> ApiResult<Response> {
    let Some(name) = state.console.take_pairing(&body.code, &body.wait) else {
        return Ok(web::ok(json!({"ok": false})));
    };
    let token = state.console.new_session(&name)?;
    Ok(with_cookie(web::ok(json!({"ok": true})), &session_cookie(&token)))
}

async fn me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    web::ok(json!({"logged_in": is_console(&state, &headers)}))
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    state.console.end(cookie(&headers, COOKIE).as_deref());
    with_cookie(web::ok(json!({"ok": true})), &deleted_cookie())
}

/// The page and its login, without the bridge key (`/console/…`). Mounted at the root (not on a cloud server).
pub fn console_router() -> Router<AppState> {
    Router::new()
        .route("/console", get(to_page))
        .route("/console/", get(page))
        .route("/console/api/login", post(login))
        .route("/console/api/pairing", post(pairing))
        .route("/console/api/pairing/wait", post(pairing_wait))
        .route("/console/api/me", get(me))
        .route("/console/api/logout", post(logout))
}

async fn approve(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Json(body): Json<Approval>,
) -> ApiResult<Response> {
    if !state.console.approve(&code, &body.name) {
        return Err(HttpError::new(404, "no console waits with this code"));
    }
    Ok(web::ok(json!({"ok": true})))
}

async fn devices(State(state): State<AppState>) -> Response {
    let devices: Vec<Value> = state.console.devices().into_iter().map(Value::Object).collect();
    web::ok(json!({"devices": devices}))
}

async fn revoke(State(state): State<AppState>, Path(device): Path<String>) -> ApiResult<Response> {
    if !state.console.revoke(&device) {
        return Err(HttpError::new(404, "unknown device"));
    }
    Ok(web::no_content())
}

/// For the app (bridge key): approve a console, list and revoke them (`/v1/console/…`).
pub fn console_admin_router() -> Router<AppState> {
    Router::new()
        .route("/console/pairings/{code}", post(approve))
        .route("/console/devices", get(devices))
        .route("/console/devices/{device}", delete(revoke))
}

/// The console's scripts and styles (`console/static`, embedded), mounted at /console/static.
pub fn console_files() -> Router<AppState> {
    let dir: &'static include_dir::Dir<'static> =
        crate::assets::CONSOLE.get_dir("static").expect("bipbridge/console/static is embedded");
    crate::assets::static_router(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_pairings_and_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConsoleStore::new(&dir.path().join("console.json").to_string_lossy());
        let code = store.new_link().unwrap();
        assert!(store.use_link(&code) && !store.use_link(&code));
        let pairing = store.new_pairing().unwrap();
        let code = pairing["code"].as_str().unwrap().to_string();
        assert_eq!(code.len(), 6);
        let wait = pairing["wait"].as_str().unwrap().to_string();
        assert_eq!(store.take_pairing(&code, &wait), None);
        assert!(store.approve(&code, "") && !store.approve(&code, "Mac"));
        assert_eq!(store.take_pairing(&code, "guess"), None);
        assert_eq!(store.take_pairing(&code, &wait).as_deref(), Some("Navigateur"));
        let token = store.new_session(&"x".repeat(80)).unwrap();
        assert!(store.valid(Some(&token)) && !store.valid(Some("nope")) && !store.valid(None));
        let devices = store.devices();
        assert_eq!(devices[0]["name"].as_str().unwrap().len(), 60);
        assert!(!devices[0].contains_key("expires"));
        assert!(store.revoke(devices[0]["id"].as_str().unwrap()) && !store.valid(Some(&token)));
        let text = std::fs::read_to_string(dir.path().join("console.json")).unwrap();
        assert!(!text.contains(&token) && text.starts_with("{\n \"sessions\": {}"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.path().join("console.json")).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn cookies_as_starlette_writes_them() {
        assert_eq!(session_cookie("T"), "bip_console=T; HttpOnly; Max-Age=2592000; Path=/; SameSite=strict; Secure");
        let deleted = deleted_cookie();
        assert!(
            deleted.starts_with("bip_console=\"\"; expires=")
                && deleted.ends_with(" GMT; Max-Age=0; Path=/; SameSite=lax")
        );
    }
}
