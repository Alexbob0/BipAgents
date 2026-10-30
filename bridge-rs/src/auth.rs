//! Bearer authentication: the bridge key, or a key of one of the person's devices (device_keys.py); constant-time
//! comparisons.
//!
//! In axum: [`require_bridge_key`] is the middleware app.rs puts on every router Python mounts with
//! `Depends(require_bridge_key)`; it leaves a [`Caller`] in the request's extensions (Python's
//! `request.state.console` / `request.state.device`). [`ClientAddr`] is Python's `request.client.host`, with
//! uvicorn's `proxy_headers` rule (an `X-Forwarded-For` from 127.0.0.1, `tailscale serve`, is believed).

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use subtle::ConstantTimeEq;

use crate::app::AppState;
use crate::web::HttpError;

/// uvicorn's `forwarded_allow_ips="127.0.0.1"`.
pub const TRUSTED_PROXIES: [&str; 1] = ["127.0.0.1"];

/// `hmac.compare_digest` on bytes.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// The token of an `Authorization: Bearer <token>` header (any case for « Bearer »).
pub fn extract_bearer(header: Option<&str>) -> Option<String> {
    let header = header.filter(|h| !h.is_empty())?;
    let (scheme, token) = match header.split_once(' ') {
        Some((s, t)) => (s, t),
        None => (header, ""),
    };
    if scheme.to_lowercase() != "bearer" {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// `Authorization`'s bearer token in a request's headers.
pub fn bearer_of(headers: &HeaderMap) -> Option<String> {
    extract_bearer(headers.get("authorization").and_then(|v| v.to_str().ok()))
}

pub fn key_matches(expected: &str, presented: Option<&str>) -> bool {
    match presented {
        None | Some("") => {
            // Still run a comparison so timing does not reveal "missing" vs "wrong".
            let zeros = vec![0u8; expected.len()];
            let _ = constant_time_eq(expected.as_bytes(), &zeros);
            false
        }
        Some(p) => constant_time_eq(expected.as_bytes(), p.as_bytes()),
    }
}

/// uvicorn's `_parse_host_port`: a forwarded value's host and port (0 when none or malformed).
pub fn parse_host_port(value: &str) -> (String, u16) {
    if let Some(rest) = value.strip_prefix('[') {
        let Some(end) = rest.find(']') else { return (value.to_string(), 0) };
        let host = &rest[..end];
        let remainder = &rest[end + 1..];
        if remainder.is_empty() {
            return (host.to_string(), 0);
        }
        let Some(port) = remainder.strip_prefix(':') else { return (value.to_string(), 0) };
        return match port.trim().parse::<i64>() {
            Ok(p) => (host.to_string(), p.clamp(0, u16::MAX as i64) as u16),
            Err(_) => (host.to_string(), 0),
        };
    }
    if value.matches(':').count() == 1 {
        let (host, port) = value.rsplit_once(':').unwrap_or((value, ""));
        return match port.trim().parse::<i64>() {
            Ok(p) => (host.to_string(), p.clamp(0, u16::MAX as i64) as u16),
            Err(_) => (value.to_string(), 0),
        };
    }
    (value.to_string(), 0)
}

fn trusted(host: &str) -> bool {
    match host.parse::<std::net::IpAddr>() {
        Ok(ip) => TRUSTED_PROXIES.iter().any(|t| t.parse::<std::net::IpAddr>().map(|t| t == ip).unwrap_or(false)),
        Err(_) => TRUSTED_PROXIES.contains(&host),
    }
}

/// uvicorn's `_TrustedHosts.get_trusted_client_address`: from the right, the first address that is not a trusted
/// proxy; all trusted: the leftmost.
pub fn trusted_client_address(x_forwarded_for: &str) -> (String, u16) {
    let hosts: Vec<&str> = x_forwarded_for.split(',').map(str::trim).collect();
    for host_port in hosts.iter().rev() {
        let (host, port) = parse_host_port(host_port);
        if host.is_empty() || !trusted(&host) {
            return (host, port);
        }
    }
    parse_host_port(hosts[0])
}

/// uvicorn's `ProxyHeadersMiddleware` (`proxy_headers=True`, `forwarded_allow_ips="127.0.0.1"`): the client's
/// address, the `X-Forwarded-For` one when the peer is a trusted proxy (`tailscale serve`).
pub fn client_host(peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<String> {
    let peer = peer?;
    let host = peer.ip().to_string();
    if trusted(&host) {
        let values: Vec<String> = headers
            .get_all("x-forwarded-for")
            .iter()
            .map(|v| v.as_bytes().iter().map(|b| *b as char).collect())
            .collect();
        if !values.is_empty() {
            let (forwarded, _port) = trusted_client_address(&values.join(", "));
            if !forwarded.is_empty() {
                return Some(forwarded);
            }
        }
    }
    Some(host)
}

/// The listener's scheme ("https" on the LAN door), put in the request's extensions by the listener.
#[derive(Debug, Clone, Copy)]
pub struct Scheme(pub &'static str);

/// `request.url.scheme`: the listener's, or `X-Forwarded-Proto` from a trusted proxy (uvicorn's proxy headers).
pub fn scheme_of(parts: &Parts) -> String {
    let own = parts.extensions.get::<Scheme>().map(|s| s.0).unwrap_or("http");
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    if let Some(peer) = peer {
        if trusted(&peer.ip().to_string()) {
            let forwarded = parts
                .headers
                .get_all("x-forwarded-proto")
                .iter()
                .next_back()
                .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_string());
            if let Some(proto) = forwarded.filter(|p| ["http", "https", "ws", "wss"].contains(&p.as_str())) {
                return proto;
            }
        }
    }
    own.to_string()
}

/// `address_of(request)` / `request.client.host`: None when the server gave no peer address (tests).
#[derive(Debug, Clone, Default)]
pub struct ClientAddr(pub Option<String>);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientAddr(address_of(parts)))
    }
}

/// The client's address for a request's parts.
pub fn address_of(parts: &Parts) -> Option<String> {
    let peer = parts.extensions.get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    client_host(peer, &parts.headers)
}

/// Who passed the bridge-key check (Python's `request.state.console` and `request.state.device`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Caller {
    /// A web console's session (console.py): it never holds the key.
    pub console: bool,
    /// The device whose own key was presented (device_keys.py), when it was not the bridge key.
    pub device: Option<String>,
}

impl<S: Send + Sync> FromRequestParts<S> for Caller {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Caller>().cloned().unwrap_or_default())
    }
}

/// A key one of the person's devices was given (device_keys.py): which one.
fn device(state: &AppState, presented: Option<&str>) -> Option<String> {
    let presented = presented.filter(|p| !p.is_empty())?;
    crate::device_keys::of(&state.config()).match_key(Some(presented))
}

fn too_many() -> HttpError {
    HttpError::new(429, "too many wrong keys, try again later")
}

/// The bridge key (the app), or a web console's session (console.py; `Caller.console` then says so). An address
/// that kept presenting wrong keys is refused for a while (guard.py). A request from an app wakes a sleeping place
/// first (sleep.py).
pub async fn require_bridge_key(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    let address = address_of(&parts);
    if state.guard.blocked(address.as_deref()) {
        return too_many().into_response();
    }
    let config = state.config();
    let presented = bearer_of(&parts.headers);
    let matched = key_matches(&config.bridge_key, presented.as_deref());
    let device = if matched { None } else { device(&state, presented.as_deref()) };
    if matched {
        // a place of the offer: its app has the claim's answer, the claim can't be recovered any more
        crate::cloud_setup::bridge_key_used(&config);
    }
    if matched || device.is_some() {
        parts.extensions.insert(Caller { console: false, device });
        // A request from an app: a place asleep (sleep.py) wakes its Hermes first.
        state.sleep.awake().await;
        return next.run(Request::from_parts(parts, body)).await;
    }
    if crate::console::is_console(&state, &parts.headers) {
        parts.extensions.insert(Caller { console: true, device: None });
        return next.run(Request::from_parts(parts, body)).await;
    }
    if presented.is_some() {
        state.guard.fail(address.as_deref(), "bridge key");
    }
    HttpError::new(401, "unauthorized").with_header("www-authenticate", "Bearer").into_response()
}

/// The voice WebSocket's check (before accepting it).
pub fn websocket_authorized(state: &AppState, address: Option<&str>, headers: &HeaderMap) -> bool {
    if state.guard.blocked(address) {
        return false;
    }
    let presented = bearer_of(headers);
    let config = state.config();
    if key_matches(&config.bridge_key, presented.as_deref()) {
        crate::cloud_setup::bridge_key_used(&config);
        return true;
    }
    if device(state, presented.as_deref()).is_some() {
        return true;
    }
    if presented.is_some() {
        state.guard.fail(address, "bridge key");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_parsing() {
        assert_eq!(extract_bearer(Some("Bearer abc")).as_deref(), Some("abc"));
        assert_eq!(extract_bearer(Some("bearer  abc ")).as_deref(), Some("abc"));
        assert_eq!(extract_bearer(Some("Basic abc")), None);
        assert_eq!(extract_bearer(Some("abc")), None);
        assert_eq!(extract_bearer(Some("Bearer ")), None);
        assert_eq!(extract_bearer(None), None);
        assert!(key_matches("k", Some("k")) && !key_matches("k", Some("x")) && !key_matches("k", None));
    }

    #[test]
    fn forwarded_for_from_the_local_proxy_only() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "100.64.0.7, 127.0.0.1".parse().unwrap());
        let local: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let remote: SocketAddr = "192.168.1.9:5000".parse().unwrap();
        assert_eq!(client_host(Some(local), &headers).as_deref(), Some("100.64.0.7"));
        assert_eq!(client_host(Some(remote), &headers).as_deref(), Some("192.168.1.9"));
        assert_eq!(client_host(Some(local), &HeaderMap::new()).as_deref(), Some("127.0.0.1"));
        assert_eq!(client_host(None, &headers), None);
    }
}
