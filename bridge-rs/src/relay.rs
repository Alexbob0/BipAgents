//! Notifications for installs without an APNs key: another person's bridge on the same server sends its pushes to
//! the administrator's bridge (`POST /v1/relay/push`, its own key), which forwards them to APNs with the team key. The
//! key never leaves the administrator's install; each relaying install has its own key and an hourly quota.
//!
//! Port of `bridge/bipbridge/relay.py` (owner group: push-runs).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};

use crate::apns::{ApnsResult, SendOptions};
use crate::app::AppState;
use crate::auth::{extract_bearer, key_matches};
use crate::logs::redact6;
use crate::push::pyx::{self, PyError};
use crate::push::Sender;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};

pub static DEVICE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-f]{32,512}$").expect("valid regex"));
/// A token from the publisher's push relay (relay/): kept as it is (case-sensitive). `r2.` and later end after 90
/// days; the first format (`r1.`, without an end) is taken only until [`R1_UNTIL`], see [`relay_token_taken`].
pub static RELAY_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^r[1-9]\.[A-Za-z0-9_-]{40,8192}$").expect("valid regex"));
/// The end of the transition from the relay's first token format: 2026-12-15, 00:00 UTC (the relay's
/// `RELAY_R1_UNTIL` default). Until then apps from before 2026-10-28 keep giving `r1.` tokens, which are taken and
/// used, and the bridge asks the app for a new one (`renew`); after it the relay answers 410 for them anyway.
pub const R1_UNTIL: u64 = 1_797_292_800;

/// Whether a relay token is of the first format.
pub fn is_first_format(token: &str) -> bool {
    token.starts_with("r1.")
}

/// Whether this bridge takes a relay token at `now` (Unix seconds).
pub fn relay_token_taken(token: &str, now: u64) -> bool {
    RELAY_TOKEN.is_match(token) && (!is_first_format(token) || now < R1_UNTIL)
}

/// Now, in Unix seconds.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
/// APNs' own limit for an alert.
pub const MAX_PAYLOAD_BYTES: usize = 4096;

/// Sends through another bridge's relay (`[push] relay_url`), or the publisher's relay (`[push] service_url`).
pub struct RelayClient {
    pub url: String,
    pub key: String,
    pub client: reqwest::Client,
}

impl RelayClient {
    pub fn new(url: &str, key: &str, client: reqwest::Client) -> Self {
        RelayClient { url: url.trim_end_matches('/').to_string(), key: key.to_string(), client }
    }

    pub async fn send(&self, device_token: &str, payload: &Value, options: &SendOptions) -> ApnsResult {
        match self.try_send(device_token, payload, options).await {
            Ok(result) => result,
            Err(exc) => {
                tracing::warn!(error = pyx::error_name(&exc), "apns send crashed");
                ApnsResult { status: 0, reason: Some("NetworkError".into()), apns_id: None }
            }
        }
    }

    /// `send`, with what Python lets through as an exception (a 200 answer that is not JSON…).
    pub async fn try_send(
        &self,
        device_token: &str,
        payload: &Value,
        options: &SendOptions,
    ) -> anyhow::Result<ApnsResult> {
        let body = json!({
            "token": device_token, "environment": options.environment, "payload": payload, "push_type": options.push_type,
            "priority": options.priority, "collapse_id": options.collapse_id,
        });
        let mut request =
            self.client.post(format!("{}/v1/relay/push", self.url)).json(&body).timeout(Duration::from_secs(15));
        if !self.key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.key));
        }
        let unreachable = |exc: &reqwest::Error| {
            tracing::warn!(error = crate::hermes::httpx_error_name(exc), "relay unreachable");
            ApnsResult { status: 503, reason: Some("RelayUnreachable".into()), apns_id: None }
        };
        let resp = match request.send().await {
            Ok(resp) => resp,
            Err(exc) => return Ok(unreachable(&exc)),
        };
        let status = resp.status().as_u16();
        let text = match resp.bytes().await {
            Ok(bytes) => bytes,
            Err(exc) => return Ok(unreachable(&exc)),
        };
        if status != 200 {
            if status == 403 {
                // the publisher blocked this device for abuse (terms): said in the log
                let detail = match serde_json::from_slice::<Value>(&text) {
                    Ok(Value::Object(map)) => map.get("detail").map(crate::py::py_str).unwrap_or_default(),
                    Ok(_) => return Err(PyError::new("AttributeError", "the relay's answer is not an object").into()),
                    Err(_) => String::from_utf8_lossy(&text).into_owned(),
                };
                tracing::warn!(detail = %pyx::head(&detail, 200), "relay refused this device");
            }
            return Ok(ApnsResult { status, reason: Some(format!("Relay{status}")), apns_id: None });
        }
        let data: Value = serde_json::from_slice(&text).map_err(|e| PyError::new("JSONDecodeError", e.to_string()))?;
        let Value::Object(data) = data else {
            return Err(PyError::new("AttributeError", "the relay's answer is not an object").into());
        };
        let status =
            crate::py::py_int(data.get("status").unwrap_or(&json!(0))).map_err(|e| PyError::new("TypeError", e))?;
        let text_of = |k: &str| data.get(k).filter(|v| !v.is_null()).map(crate::py::py_str);
        Ok(ApnsResult {
            status: status.clamp(0, u16::MAX as i64) as u16,
            reason: text_of("reason"),
            apns_id: text_of("apns_id"),
        })
    }
}

fn dict<'de, D: Deserializer<'de>>(d: D) -> Result<Map<String, Value>, D::Error> {
    match Value::deserialize(d)? {
        Value::Object(map) => Ok(map),
        _ => Err(serde::de::Error::custom("pydantic:dict_type:Input should be a valid dictionary")),
    }
}

fn default_push_type() -> String {
    "alert".into()
}

fn default_priority() -> i64 {
    10
}

/// `RelayPush` (pydantic).
#[derive(Debug, Deserialize)]
pub struct RelayPush {
    pub token: String,
    pub environment: String,
    #[serde(deserialize_with = "dict")]
    pub payload: Map<String, Value>,
    #[serde(default = "default_push_type")]
    pub push_type: String,
    #[serde(default = "default_priority", deserialize_with = "web::lax::int")]
    pub priority: i64,
    #[serde(default)]
    pub collapse_id: Option<String>,
}

impl Validate for RelayPush {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "token", Some(&self.token), 4096);
        web::check_max_len(errors, "collapse_id", self.collapse_id.as_deref(), 64);
    }
}

/// Pushes relayed per install over the last hour.
#[derive(Default)]
struct Quota {
    sent: Mutex<HashMap<String, VecDeque<f64>>>,
}

impl Quota {
    fn allow(&self, client: &str, per_hour: i64) -> bool {
        let now = crate::py::monotonic();
        let mut sent = self.sent.lock().unwrap_or_else(|e| e.into_inner());
        let window = sent.entry(client.to_string()).or_default();
        while window.front().map(|t| *t < now - 3600.0).unwrap_or(false) {
            window.pop_front();
        }
        if window.len() as i64 >= per_hour {
            return false;
        }
        window.push_back(now);
        true
    }
}

/// `relay_router()`: the relay this bridge offers to other installs (`[relay] clients`), each with its own key
/// (checked here, not the bridge key). Mounted at the root, as in Python.
pub fn router() -> Router<AppState> {
    let quota = Arc::new(Quota::default());
    Router::new().route(
        "/v1/relay/push",
        post(move |state: State<AppState>, headers: HeaderMap, body: Json<RelayPush>| {
            relay_push(state, headers, body, quota.clone())
        }),
    )
}

async fn relay_push(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<RelayPush>,
    quota: Arc<Quota>,
) -> ApiResult<Response> {
    let config = state.config();
    let relay = &config.relay;
    if !relay.enabled() {
        return Err(HttpError::new(404, "not found"));
    }
    let presented = extract_bearer(headers.get("authorization").and_then(|v| v.to_str().ok()));
    let client =
        relay.clients.iter().find(|(key, _)| key_matches(key, presented.as_deref())).map(|(_, name)| name.clone());
    let Some(client) = client else {
        return Err(HttpError::new(401, "unauthorized").with_header("www-authenticate", "Bearer"));
    };
    let android = body.environment == "fcm";
    let sender = if android { state.fcm.clone().map(Sender::Fcm) } else { state.apns.clone().map(Sender::Apns) };
    let Some(sender) = sender else {
        return Err(HttpError::new(
            503,
            format!("{} not configured on the relay", if android { "FCM" } else { "APNs" }),
        ));
    };
    let token = if android { pyx::strip(&body.token).to_string() } else { pyx::strip(&body.token).to_lowercase() };
    let valid_token = if android { crate::fcm::FCM_TOKEN.is_match(&token) } else { DEVICE_TOKEN.is_match(&token) };
    if !valid_token
        || !["sandbox", "production", "fcm"].contains(&body.environment.as_str())
        || !["alert", "background", "liveactivity"].contains(&body.push_type.as_str())
        || (android && body.push_type == "liveactivity")
        || ![5, 10].contains(&body.priority)
    {
        return Err(HttpError::new(400, "invalid push"));
    }
    let payload = Value::Object(body.payload);
    if crate::py::Dumps::new().unicode().dumps(&payload).len() > MAX_PAYLOAD_BYTES {
        return Err(HttpError::new(413, "payload too large"));
    }
    if !quota.allow(&client, relay.per_hour) {
        return Err(HttpError::new(429, "quota exceeded"));
    }
    let options = SendOptions {
        environment: body.environment.clone(),
        push_type: body.push_type.clone(),
        priority: body.priority as u8,
        collapse_id: body.collapse_id.clone(),
        expiration: None,
    };
    let result = sender.try_send(&token, &payload, &options).await?;
    tracing::info!(client = %client, token = %redact6(&token), status = result.status, "relayed push");
    Ok(web::ok(json!({"status": result.status, "reason": result.reason, "apns_id": result.apns_id})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_format_relay_tokens_are_taken_until_the_end_of_the_transition() {
        let (r1, r2) = (format!("r1.{}", "A".repeat(60)), format!("r2.{}", "A".repeat(60)));
        for now in [R1_UNTIL - 1, R1_UNTIL, R1_UNTIL + 86_400] {
            assert!(relay_token_taken(&r2, now));
        }
        assert!(relay_token_taken(&r1, R1_UNTIL - 1));
        assert!(!relay_token_taken(&r1, R1_UNTIL));
        assert!(!relay_token_taken("r1.short", R1_UNTIL - 1));
        assert!(!relay_token_taken(&"ab".repeat(32), R1_UNTIL - 1));
        assert!(is_first_format(&r1) && !is_first_format(&r2));
    }
}
