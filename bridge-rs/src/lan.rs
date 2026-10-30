//! The local-network door: when Tailscale is down (no Internet at home, the VPN off), the app reaches the bridge
//! on the LAN, over HTTPS with a self-signed certificate it pins (the SHA-256 of the certificate travels in the pairing
//! QR code), and Hermes through the bridge under `/hermes/<agent>/…`. Off unless `[lan] enabled = true`.
//!
//! Also builds the pairing payload (what the QR code carries) and the LAN details the app refreshes on its own
//! (`GET /v1/pairing`) so a new IP address needs no new QR code while the tailnet works.

use std::net::{IpAddr, UdpSocket};

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use axum::response::Response;
use axum::routing::on;
use axum::Router;
use futures::StreamExt;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::auth::ClientAddr;
use crate::config::{AgentConfig, Config};
use crate::py;
use crate::web::{ApiResult, HttpError};

pub const CERT_NAME: &str = "bridge-lan.pem";
pub const KEY_NAME: &str = "bridge-lan.key";
/// Headers that belong to one connection, not to the request being relayed.
pub const HOP: [&str; 10] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

pub fn certificate_paths(config: &Config) -> (String, String) {
    (py::join(&config.lan.cert_dir, CERT_NAME), py::join(&config.lan.cert_dir, KEY_NAME))
}

/// The LAN certificate and key, created once (ECDSA P-256, 20 years: the app pins it, no CA involved).
pub fn ensure_certificate(config: &Config) -> anyhow::Result<(String, String)> {
    let (cert_path, key_path) = certificate_paths(config);
    if std::path::Path::new(&cert_path).exists() && std::path::Path::new(&key_path).exists() {
        return Ok((cert_path, key_path));
    }
    py::makedirs(&config.lan.cert_dir, 0o700)?;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "BipAgents bridge (LAN)");
    params.distinguished_name = name;
    let now = chrono::Utc::now();
    let start = now - chrono::Duration::days(1);
    let end = now + chrono::Duration::days(365 * 20);
    use chrono::Datelike;
    params.not_before = rcgen::date_time_ymd(start.year(), start.month() as u8, start.day() as u8);
    params.not_after = rcgen::date_time_ymd(end.year(), end.month() as u8, end.day() as u8);
    // x509.random_serial_number(): 159 random bits, positive.
    let mut serial = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut serial);
    serial[0] &= 0x7f;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    params.key_identifier_method = rcgen::KeyIdMethod::Sha256;
    let cert = params.self_signed(&key)?;
    py::write_with_mode(&key_path, key.serialize_pem().as_bytes(), 0o600)?;
    std::fs::write(&cert_path, cert.pem())?;
    tracing::info!(path = %cert_path, "LAN certificate created");
    Ok((cert_path, key_path))
}

/// SHA-256 of the certificate (DER), lowercase hex: what the app pins.
pub fn fingerprint(config: &Config) -> Option<String> {
    let (cert_path, _) = certificate_paths(config);
    let text = std::fs::read(&cert_path).ok()?;
    let block = pem::parse_many(&text).ok()?.into_iter().find(|p| p.tag() == "CERTIFICATE")?;
    Some(hex::encode(Sha256::digest(block.contents())))
}

/// This machine's address on its main network (no packet is sent: a UDP « connect » only picks the route).
pub fn detect_address() -> Option<String> {
    for target in ["192.168.0.1", "10.0.0.1", "172.16.0.1"] {
        let Ok(probe) = UdpSocket::bind("0.0.0.0:0") else { continue };
        if probe.connect((target, 9)).is_err() {
            continue;
        }
        let Ok(local) = probe.local_addr() else { continue };
        let ip = local.ip();
        let address = ip.to_string();
        let private = match ip {
            IpAddr::V4(v4) => v4.is_private(),
            IpAddr::V6(_) => false,
        };
        // 100.x: the tailnet itself
        if private && !ip.is_loopback() && !address.starts_with("100.") {
            return Some(address);
        }
    }
    None
}

/// {"url", "fingerprint"} for the app, or None when the door is off or not ready.
pub fn lan_info(config: &Config) -> Option<Value> {
    if !config.lan.enabled {
        return None;
    }
    let address = config.lan.address.clone().filter(|a| !a.is_empty()).or_else(detect_address)?;
    let print = fingerprint(config)?;
    Some(json!({"url": format!("https://{address}:{}", config.lan.port), "fingerprint": print}))
}

/// What the pairing QR code carries (the app's « Scanner le QR code »): tailnet addresses, keys, voice, and the
/// LAN door. Err (Python's ValueError) when a tailnet address is missing from the config.
pub fn pairing_payload(config: &Config, agent: &AgentConfig) -> Result<Value, String> {
    let Some(base) = agent.public_url.as_deref().filter(|u| !u.is_empty()) else {
        return Err(format!("agents.{}.public_url is missing (its Hermes address on the tailnet)", agent.name));
    };
    let Some(bridge) = config.public_url.as_deref().filter(|u| !u.is_empty()) else {
        return Err("public_url is missing (the bridge's address on the tailnet)".into());
    };
    let mut payload = Map::new();
    payload.insert("name".into(), json!(agent.display_name));
    payload.insert("agent".into(), json!(agent.name));
    payload.insert("baseURL".into(), json!(base));
    payload.insert("apiKey".into(), json!(agent.hermes_key));
    payload.insert("bridgeURL".into(), json!(bridge));
    payload.insert("bridgeKey".into(), json!(config.bridge_key));
    if let Some(voice) = agent.voice.as_deref().filter(|v| !v.is_empty()) {
        payload.insert("voice".into(), json!(voice));
    }
    if let Some(lan) = lan_info(config) {
        payload.insert("lan".into(), lan);
    }
    Ok(Value::Object(payload))
}

/// The install's QR code: the bridge's tailnet address and key (and the LAN door). The app then asks the bridge
/// for every agent (`GET /v1/agents`), so one scan adds them all, and agents created later show up too.
pub fn install_payload(config: &Config) -> Result<Value, String> {
    let Some(bridge) = config.public_url.as_deref().filter(|u| !u.is_empty()) else {
        return Err("public_url is missing (the bridge's address on the tailnet)".into());
    };
    let mut payload = Map::new();
    payload.insert("v".into(), json!(2));
    payload.insert("bridgeURL".into(), json!(bridge));
    payload.insert("bridgeKey".into(), json!(config.bridge_key));
    if let Some(lan) = lan_info(config) {
        payload.insert("lan".into(), lan);
    }
    Ok(Value::Object(payload))
}

fn is_hop(name: &str) -> bool {
    HOP.contains(&name.to_ascii_lowercase().as_str())
}

/// `/hermes/<agent>/<path>` → the agent's Hermes, streamed both ways (SSE included). Hermes checks its own key:
/// the app sends it as on the tailnet. Mounted at the root.
pub fn hermes_proxy() -> Router<AppState> {
    let methods = || {
        on(axum::routing::MethodFilter::GET, relay)
            .on(axum::routing::MethodFilter::POST, relay)
            .on(axum::routing::MethodFilter::PUT, relay)
            .on(axum::routing::MethodFilter::PATCH, relay)
            .on(axum::routing::MethodFilter::DELETE, relay)
    };
    Router::new().route("/hermes/{agent}/{*path}", methods()).route("/hermes/{agent}/", methods_empty())
}

fn methods_empty() -> axum::routing::MethodRouter<AppState> {
    let empty = |state: State<AppState>,
                 addr: ClientAddr,
                 Path(agent): Path<String>,
                 method: Method,
                 uri: Uri,
                 headers: HeaderMap,
                 body: Body| async move {
        relay(state, addr, Path((agent, String::new())), method, uri, headers, body).await
    };
    on(axum::routing::MethodFilter::GET, empty)
        .on(axum::routing::MethodFilter::POST, empty)
        .on(axum::routing::MethodFilter::PUT, empty)
        .on(axum::routing::MethodFilter::PATCH, empty)
        .on(axum::routing::MethodFilter::DELETE, empty)
}

async fn relay(
    State(state): State<AppState>,
    ClientAddr(address): ClientAddr,
    Path((agent, path)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    // the body is not read before these checks, and then streamed to Hermes (body_limit.rs caps it)
    if state.guard.blocked(address.as_deref()) {
        return Err(HttpError::new(429, "too many wrong keys, try again later"));
    }
    let config = state.config();
    let allowed = path.starts_with("v1/") || path.starts_with("api/") || path == "health" || path == "v1/capabilities";
    let target = match config.agent(Some(&agent)) {
        Some(t) if allowed => t.clone(),
        _ => return Err(HttpError::new(404, "not found")),
    };
    // Just claimed: its gateway restarting, or the agent still being made (cloud_setup.rs). Waited for, not failed.
    crate::cloud_setup::READINESS.wait(&target.name, crate::cloud_setup::READY_WAIT).await;
    // dict(request.headers): one value per name (the last), hop-by-hop ones dropped. Accept-Encoding is left to
    // the client here, which decodes the body (httpx does too: content-encoding is not passed on).
    let mut forwarded: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for (name, value) in headers.iter() {
        if is_hop(name.as_str()) || name.as_str() == "accept-encoding" {
            continue;
        }
        forwarded.retain(|(n, _)| n != name);
        forwarded.push((name.clone(), value.clone()));
    }
    if crate::console::is_console(&state, &headers) {
        // a web console never holds the key: the bridge adds it
        forwarded
            .retain(|(n, _)| !matches!(n.as_str(), "authorization" | "cookie") && n.as_str() != crate::console::HEADER);
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", target.hermes_key)) {
            forwarded.push((HeaderName::from_static("authorization"), v));
        }
    }
    // A run's events come from the run hub, which holds the single Hermes subscription (Hermes' event buffer has one
    // reader) and counts its tools for the brake (loop_brake.rs): the lab and the apps' direct fallbacks follow runs
    // here. Hermes still checks the key, and says first whether the run is there (its own 404).
    let events_of = RUN_EVENTS.captures(&path).map(|c| c[1].to_string()).filter(|r| crate::app::RUN_ID.is_match(r));
    if let (true, Some(run_id)) = (method == Method::GET, events_of) {
        let mut check = state.http.get(format!("{}/v1/runs/{run_id}", target.hermes_url));
        for (name, value) in forwarded.iter().filter(|(n, _)| !matches!(n.as_str(), "accept" | "content-length")) {
            check = check.header(name.clone(), value.clone());
        }
        let status = match check.timeout(std::time::Duration::from_secs(30)).send().await {
            Ok(r) => r.status().as_u16(),
            Err(exc) => {
                tracing::info!(agent = %target.name, error = crate::hermes::httpx_error_name(&exc), "LAN relay failed");
                return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment"));
            }
        };
        if status == 401 {
            state.guard.fail(address.as_deref(), "agent key");
        }
        if !(200..300).contains(&status) {
            return Err(HttpError::new(status, if status == 404 { "run not found" } else { "your agent refused this request" }));
        }
        let listener = state.hub.subscribe(&target, &run_id, "app", false);
        state.hub.keep(&target, &run_id, None);
        return Ok(crate::app::listener_events(listener));
    }
    let starts_run = method == Method::POST && path == "v1/runs";
    // A run started with the agent's key (or by the web console) is read whole, to follow the run it starts;
    // anything else goes to Hermes as it comes, and Hermes checks the key.
    let presented = forwarded
        .iter()
        .find(|(n, _)| n.as_str() == "authorization")
        .and_then(|(_, v)| crate::auth::extract_bearer(v.to_str().ok()));
    let keyed = crate::auth::key_matches(&target.hermes_key, presented.as_deref());
    let (request_body, body) = if starts_run && keyed {
        let bytes = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| {
            if crate::body_limit::is_too_large(&e) {
                HttpError::new(413, "request body too large")
            } else {
                HttpError::new(400, "There was an error parsing the body")
            }
        })?;
        (bytes.clone(), reqwest::Body::from(bytes))
    } else if axum::body::HttpBody::is_end_stream(&body) {
        (Bytes::new(), reqwest::Body::from(Bytes::new())) // a GET: no body at all, not an empty chunked one
    } else {
        (Bytes::new(), reqwest::Body::wrap_stream(body.into_data_stream()))
    };
    let starts_run = starts_run && keyed;
    let mut url = format!("{}/{}", target.hermes_url, path);
    if let Some(query) = uri.query().filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    let mut request = state.http.request(method, &url).body(body);
    for (name, value) in forwarded {
        request = request.header(name, value);
    }
    // SSE: no read timeout.
    let upstream = match request.send().await {
        Ok(r) => r,
        Err(exc) => {
            tracing::info!(agent = %target.name, error = crate::hermes::httpx_error_name(&exc), "LAN relay failed");
            return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment"));
        }
    };
    if upstream.status().as_u16() == 401 {
        // a wrong agent key, through the door
        state.guard.fail(address.as_deref(), "agent key");
    }
    let status = upstream.status();
    let mut response_headers = HeaderMap::new();
    for (name, value) in upstream.headers().iter() {
        if is_hop(name.as_str()) || name.as_str() == "content-encoding" {
            continue;
        }
        response_headers.insert(name.clone(), value.clone());
    }
    let stream = upstream.bytes_stream().map(|chunk| chunk.map_err(std::io::Error::other));
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    *response.headers_mut() = response_headers;
    if starts_run {
        if status.is_success() {
            // the person wrote to the agent: its welcome stays in that conversation only (welcome.rs)
            let session = serde_json::from_slice::<serde_json::Value>(&request_body)
                .ok()
                .and_then(|b| b.get("session_id").and_then(serde_json::Value::as_str).map(str::to_string));
            crate::welcome::done(&state.config(), &target.name, session.as_deref());
        }
        // the run it started followed by the hub whoever looks at it (the brake, loop_brake.rs)
        return Ok(crate::runs::keep_started(&state.hub, &target, &request_body, response).await);
    }
    Ok(response)
}

/// `v1/runs/<id>/events`.
static RUN_EVENTS: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^v1/runs/([^/]+)/events/?\z").expect("valid regex"));

#[cfg(test)]
mod tests {
    use super::*;

    fn lan_config(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(40));
        config.lan.enabled = true;
        config.lan.address = Some("192.168.1.177".into());
        config.lan.cert_dir = dir.join("lan").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn certificate_is_made_once_and_pinned_by_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let config = lan_config(dir.path());
        let (cert, key) = ensure_certificate(&config).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        let first = fingerprint(&config).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(ensure_certificate(&config).unwrap(), (cert, key));
        assert_eq!(fingerprint(&config).unwrap(), first);
        assert_eq!(lan_info(&config).unwrap(), json!({"url": "https://192.168.1.177:8650", "fingerprint": first}));
    }

    #[test]
    fn pairing_payload_carries_tailnet_addresses_keys_and_lan() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = lan_config(dir.path());
        config.public_url = Some("https://box.example.ts.net:8643".into());
        let mut agent = AgentConfig {
            name: "wellness".into(),
            display_name: "Wellness".into(),
            hermes_url: "http://hermes-wellness.test".into(),
            hermes_key: "hermes-wellness-key-xyz".into(),
            public_url: Some("https://box.example.ts.net:8642".into()),
            ..Default::default()
        };
        ensure_certificate(&config).unwrap();
        let payload = pairing_payload(&config, &agent).unwrap();
        assert_eq!(payload["baseURL"], "https://box.example.ts.net:8642");
        assert_eq!(payload["agent"], "wellness");
        assert_eq!(payload["apiKey"], "hermes-wellness-key-xyz");
        assert_eq!(payload["bridgeURL"], "https://box.example.ts.net:8643");
        assert_eq!(
            payload["lan"],
            json!({"url": "https://192.168.1.177:8650", "fingerprint": fingerprint(&config).unwrap()})
        );
        let keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["name", "agent", "baseURL", "apiKey", "bridgeURL", "bridgeKey", "lan"]);
        agent.public_url = None;
        assert!(pairing_payload(&config, &agent).unwrap_err().contains("public_url"));
        let install = install_payload(&config).unwrap();
        assert_eq!(install["v"], 2);
    }

    #[test]
    fn lan_off_by_default() {
        let config = Config::with_key("k".repeat(40));
        assert!(!config.lan.enabled);
        assert!(lan_info(&config).is_none());
    }
}
