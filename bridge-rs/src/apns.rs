//! APNs HTTP/2 client with token-based (ES256 JWT) authentication.
//!
//! Port of `bridge/bipbridge/apns.py` (owner group: push-runs).

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use crate::config::ApnsConfig;
use crate::logs::{redact6, J};
use crate::push::pyx::{self, PyError};
use crate::py::Clock;

/// Apple's hosts, by environment (see `ApnsConfig::host`, which also applies `[apns] base_url` / `sandbox_url`).
pub use crate::config::APNS_HOSTS as HOSTS;

/// Apple rejects provider tokens older than 60 min.
pub const TOKEN_TTL_SECONDS: f64 = 50.0 * 60.0;

/// APNs reasons for a token that will never work again (the device is forgotten).
pub const DEAD_TOKEN_REASONS: [&str; 3] = ["BadDeviceToken", "Unregistered", "DeviceTokenNotForTopic"];

/// What a push service answered (APNs, FCM and the relays answer with the same shape).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApnsResult {
    pub status: u16,
    pub reason: Option<String>,
    pub apns_id: Option<String>,
}

impl ApnsResult {
    pub fn ok(&self) -> bool {
        self.status == 200
    }

    pub fn token_is_dead(&self) -> bool {
        self.status == 410
            || (self.status == 400 && self.reason.as_deref().map(|r| DEAD_TOKEN_REASONS.contains(&r)).unwrap_or(false))
    }
}

/// `send(..., *, environment, push_type="alert", priority=10, collapse_id=None, expiration=None)`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendOptions {
    pub environment: String,
    pub push_type: String,
    pub priority: u8,
    pub collapse_id: Option<String>,
    pub expiration: Option<i64>,
}

impl SendOptions {
    pub fn new(environment: &str) -> Self {
        SendOptions {
            environment: environment.into(),
            push_type: "alert".into(),
            priority: 10,
            collapse_id: None,
            expiration: None,
        }
    }
}

/// PyJWT's `jwt.encode(claims, key, algorithm)`: the header's keys sorted (`alg`, `kid`, `typ`), compact JSON, the
/// claims in their order; base64url without padding.
pub fn encode_jwt(
    header: &Value,
    claims: &Value,
    key: &jsonwebtoken::EncodingKey,
    algorithm: jsonwebtoken::Algorithm,
) -> anyhow::Result<String> {
    let b64 = |v: &Value, sort: bool| {
        let dumps = crate::py::Dumps::new().compact();
        let text = if sort { dumps.sort_keys().dumps(v) } else { dumps.dumps(v) };
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(text.as_bytes())
    };
    let message = format!("{}.{}", b64(header, true), b64(claims, false));
    let signature = jsonwebtoken::crypto::sign(message.as_bytes(), key, algorithm)
        .map_err(|e| PyError::new("ValueError", e.to_string()))?;
    Ok(format!("{message}.{signature}"))
}

pub struct ApnsClient {
    pub config: ApnsConfig,
    /// An HTTP/2 client (APNs speaks HTTP/2 only).
    pub client: reqwest::Client,
    pub clock: Clock,
    state: Mutex<TokenState>,
}

#[derive(Default)]
struct TokenState {
    key: Option<String>,
    token: Option<String>,
    token_iat: f64,
}

impl ApnsClient {
    pub fn new(config: ApnsConfig, client: reqwest::Client) -> Self {
        Self::with_clock(config, client, crate::py::wall_clock())
    }

    pub fn with_clock(config: ApnsConfig, client: reqwest::Client, clock: Clock) -> Self {
        ApnsClient { config, client, clock, state: Mutex::new(TokenState::default()) }
    }

    /// The provider JWT (ES256 with the .p8 key), renewed every 50 minutes or when `force`d.
    pub fn provider_token(&self, force: bool) -> anyhow::Result<String> {
        let now = (self.clock)();
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if force || state.token.is_none() || now - state.token_iat >= TOKEN_TTL_SECONDS {
            if state.key.is_none() {
                state.key = Some(std::fs::read_to_string(&self.config.p8_path)?);
            }
            let pem = state.key.as_deref().unwrap_or_default();
            let key = jsonwebtoken::EncodingKey::from_ec_pem(pem.as_bytes())
                .map_err(|e| PyError::new("ValueError", e.to_string()))?;
            let iat = now.floor();
            let header = json!({"typ": "JWT", "alg": "ES256", "kid": self.config.key_id});
            let claims = json!({"iss": self.config.team_id, "iat": iat as i64});
            state.token = Some(encode_jwt(&header, &claims, &key, jsonwebtoken::Algorithm::ES256)?);
            state.token_iat = iat;
        }
        Ok(state.token.clone().unwrap_or_default())
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

    /// `send`, with what Python lets through as an exception (an unreadable key…).
    pub async fn try_send(
        &self,
        device_token: &str,
        payload: &Value,
        options: &SendOptions,
    ) -> anyhow::Result<ApnsResult> {
        let host = self.config.host(&options.environment);
        let body = crate::py::Dumps::new().compact().unicode().dumps(payload).into_bytes();
        let mut result = self.post(&host, device_token, &body, options, false).await?;
        if result.status == 403
            && matches!(result.reason.as_deref(), Some("ExpiredProviderToken") | Some("InvalidProviderToken"))
        {
            result = self.post(&host, device_token, &body, options, true).await?;
        }
        let (token, env, kind, status, reason) =
            (redact6(device_token), &options.environment, &options.push_type, result.status, J(&result.reason));
        if result.ok() {
            tracing::info!(token = %token, env = %env, r#type = %kind, status = status, reason = ?reason, "apns push");
        } else {
            tracing::warn!(token = %token, env = %env, r#type = %kind, status = status, reason = ?reason, "apns push");
        }
        Ok(result)
    }

    async fn post(
        &self,
        host: &str,
        device_token: &str,
        body: &[u8],
        options: &SendOptions,
        force_token: bool,
    ) -> anyhow::Result<ApnsResult> {
        let mut request = self
            .client
            .post(format!("{host}/3/device/{device_token}"))
            .header("authorization", format!("bearer {}", self.provider_token(force_token)?))
            .header("apns-topic", &self.config.bundle_id)
            .header("apns-push-type", &options.push_type)
            .header("apns-priority", options.priority.to_string())
            .header("content-type", "application/json");
        if let Some(collapse) = options.collapse_id.as_deref().filter(|c| !c.is_empty()) {
            request = request.header("apns-collapse-id", pyx::head(collapse, 64));
        }
        if let Some(expiration) = options.expiration {
            request = request.header("apns-expiration", expiration.to_string());
        }
        let response = request.body(body.to_vec()).timeout(Duration::from_secs(15)).send().await;
        let unreachable = |exc: &reqwest::Error| {
            tracing::warn!(error = crate::hermes::httpx_error_name(exc), "apns unreachable");
            ApnsResult { status: 0, reason: Some("NetworkError".into()), apns_id: None }
        };
        let resp = match response {
            Ok(resp) => resp,
            Err(exc) => return Ok(unreachable(&exc)),
        };
        let status = resp.status().as_u16();
        let apns_id = resp.headers().get("apns-id").and_then(|v| v.to_str().ok()).map(str::to_string);
        let mut reason = None;
        if status != 200 {
            let text = match resp.bytes().await {
                Ok(bytes) => bytes,
                Err(exc) => return Ok(unreachable(&exc)),
            };
            if let Ok(data) = serde_json::from_slice::<Value>(&text) {
                match data {
                    Value::Object(map) => reason = map.get("reason").filter(|r| !r.is_null()).map(crate::py::py_str),
                    _ => return Err(PyError::new("AttributeError", "the APNs answer is not an object").into()),
                }
            }
        }
        Ok(ApnsResult { status, reason, apns_id })
    }
}
