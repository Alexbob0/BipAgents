//! The relay's three routes.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::apns::Apns;
use crate::config::Config;
use crate::fcm::Fcm;
use crate::limits::Quota;
use crate::payload::scrub;
use crate::seal::{Device, Platform, Sealer, PREFIX};

pub struct Relay {
    pub sealer: Sealer,
    pub apns: Option<Apns>,
    pub fcm: Option<Fcm>,
    per_device: Quota,
    per_ip_push: Quota,
    per_ip_register: Quota,
    trust_forwarded_for: bool,
    blocked: std::collections::HashSet<String>,
}

impl Relay {
    pub fn new(config: Config, http: reqwest::Client) -> Result<Self, String> {
        Ok(Self {
            sealer: Sealer::from_base64_list(&config.seal_keys)?,
            apns: config.apns.map(|c| Apns::new(c, http.clone())).transpose()?,
            fcm: config.fcm.map(|a| Fcm::new(a, http.clone(), config.fcm_send_base.clone())).transpose()?,
            per_device: Quota::new(config.pushes_per_device_hour),
            per_ip_push: Quota::new(config.pushes_per_ip_hour),
            per_ip_register: Quota::new(config.registrations_per_ip_hour),
            trust_forwarded_for: config.trust_forwarded_for,
            blocked: config.blocked.into_iter().collect(),
        })
    }

    /// Every minute: the quotas forget what is about to be an hour old.
    pub fn sweep(&self) {
        self.per_device.sweep();
        self.per_ip_push.sweep();
        self.per_ip_register.sweep();
    }

    fn is_blocked(&self, token: &str) -> bool {
        !self.blocked.is_empty() && self.blocked.contains(&fingerprint(token))
    }

    fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> String {
        if self.trust_forwarded_for {
            if let Some(first) = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                return first.to_string();
            }
        }
        peer.ip().to_string()
    }
}

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/register", post(register))
        .route("/v1/relay/push", post(push))
        .route("/v1/push", post(push))
        .layer(DefaultBodyLimit::max(16 * 1024))
        .with_state(relay)
}

fn refuse(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail": detail}))).into_response()
}

/// A short fingerprint for the logs: never the token itself.
fn fingerprint(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

async fn health(State(relay): State<Arc<Relay>>) -> Json<Value> {
    Json(json!({"status": "ok", "apns": relay.apns.is_some(), "fcm": relay.fcm.is_some()}))
}

#[derive(Deserialize)]
struct Register {
    platform: Platform,
    token: String,
    #[serde(default)]
    environment: Option<String>,
}

fn apple_token(raw: &str) -> Option<String> {
    let token: String = raw.chars().filter(|c| !matches!(c, ' ' | '<' | '>')).collect::<String>().to_lowercase();
    (32..=512).contains(&token.len()).then_some(()).filter(|_| token.chars().all(|c| c.is_ascii_hexdigit()))?;
    Some(token)
}

fn google_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    let valid = (20..=4096).contains(&token.len())
        && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'));
    valid.then(|| token.to_string())
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Register>,
) -> Response {
    if !relay.per_ip_register.allow(relay.client_ip(peer, &headers).as_bytes()) {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "too many registrations");
    }
    let device = match body.platform {
        Platform::Apns => {
            if relay.apns.is_none() {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Apple notifications are not set up on this relay");
            }
            let environment = body.environment.unwrap_or_else(|| "production".into());
            let Some(token) =
                apple_token(&body.token).filter(|_| matches!(environment.as_str(), "sandbox" | "production"))
            else {
                return refuse(StatusCode::BAD_REQUEST, "invalid Apple token or environment");
            };
            Device { platform: Platform::Apns, environment, token }
        }
        Platform::Fcm => {
            if relay.fcm.is_none() {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Android notifications are not set up on this relay");
            }
            let Some(token) = google_token(&body.token) else {
                return refuse(StatusCode::BAD_REQUEST, "invalid Firebase token");
            };
            Device { platform: Platform::Fcm, environment: "fcm".into(), token }
        }
    };
    if relay.is_blocked(&device.token) {
        return refuse(StatusCode::FORBIDDEN, "this device is blocked on the relay: write to support@kerfystudio.com");
    }
    tracing::info!(platform = ?device.platform, device = %fingerprint(&device.token), "device registered");
    Json(json!({"relay_token": relay.sealer.seal(&device)})).into_response()
}

fn alert() -> String {
    "alert".into()
}

fn high() -> u8 {
    10
}

#[derive(Deserialize)]
struct Push {
    token: String,
    payload: Value,
    #[serde(default = "alert")]
    push_type: String,
    #[serde(default = "high")]
    priority: u8,
    #[serde(default)]
    collapse_id: Option<String>,
}

async fn push(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Push>,
) -> Response {
    if !body.token.starts_with(PREFIX) {
        // A device token sent as such (a bridge set up for direct delivery): not a dead token, just the wrong relay.
        return refuse(StatusCode::BAD_REQUEST, "not a relay token: the app registers with /v1/register first");
    }
    let Some(device) = relay.sealer.open(&body.token) else {
        // Sealed by a key no longer in use, or altered: the bridge forgets it and the app registers again.
        return Json(json!({"status": 410, "reason": "BadRelayToken", "apns_id": null})).into_response();
    };
    if !matches!(body.push_type.as_str(), "alert" | "background")
        || !matches!(body.priority, 5 | 10)
        || body.collapse_id.as_ref().is_some_and(|c| c.len() > 64)
    {
        return refuse(StatusCode::BAD_REQUEST, "invalid push");
    }
    if relay.is_blocked(&device.token) {
        tracing::info!(device = %fingerprint(&device.token), "push refused: device blocked");
        return refuse(StatusCode::FORBIDDEN, "this device is blocked on the relay: write to support@kerfystudio.com");
    }
    if !relay.per_ip_push.allow(relay.client_ip(peer, &headers).as_bytes())
        || !relay.per_device.allow(device.token.as_bytes())
    {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "quota exceeded");
    }
    let payload = match scrub(&body.payload) {
        Ok(payload) => payload,
        Err("payload too large") => return refuse(StatusCode::PAYLOAD_TOO_LARGE, "payload too large"),
        Err(reason) => return refuse(StatusCode::BAD_REQUEST, reason),
    };
    let collapse = body.collapse_id.as_deref();
    let outcome = match device.platform {
        Platform::Apns => match &relay.apns {
            Some(apns) => {
                apns.send(&device.token, &device.environment, &payload, &body.push_type, body.priority, collapse).await
            }
            None => return refuse(StatusCode::SERVICE_UNAVAILABLE, "Apple notifications are not set up on this relay"),
        },
        Platform::Fcm => match &relay.fcm {
            Some(fcm) => fcm.send(&device.token, &payload, body.priority, collapse).await,
            None => {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Android notifications are not set up on this relay")
            }
        },
    };
    tracing::info!(platform = ?device.platform, device = %fingerprint(&device.token), status = outcome.status,
                   reason = outcome.reason.as_deref().unwrap_or(""), "push relayed");
    Json(json!({"status": outcome.status, "reason": outcome.reason, "apns_id": outcome.id})).into_response()
}
