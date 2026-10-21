//! Firebase Cloud Messaging (HTTP v1) for the Android app, with the Firebase project's service account.
//!
//! Same contract as ApnsClient (`send` returns an ApnsResult), so PushService and the relay treat both alike. The
//! APNs-style payload is turned into an FCM *data* message: the app's messaging service builds the notification itself,
//! fetching the text over the tailnet as the iPhone's notification extension does; nothing readable goes through Google
//! unless `[push] previews` is on.
//!
//! Port of `bridge/bipbridge/fcm.py` (owner group: push-runs).

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::apns::{encode_jwt, ApnsResult, SendOptions};
use crate::logs::{redact6, J};
use crate::push::pyx::{self, PyError};
use crate::py::Clock;

pub const TOKEN_URL: &str = crate::config::FCM_TOKEN_URL;
pub const BASE_URL: &str = crate::config::FCM_BASE_URL;
pub const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
pub const ACCESS_TTL_SECONDS: f64 = 50.0 * 60.0;
/// FCM errors meaning the token will never be accepted again.
pub const DEAD: [&str; 3] = ["UNREGISTERED", "INVALID_ARGUMENT", "SENDER_ID_MISMATCH"];

/// An FCM registration token (case-sensitive).
pub static FCM_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_:\-]{20,4096}$").expect("valid regex"));

/// An APNs payload as FCM data (strings only): the alert's title and body, its category, and the ids.
pub fn data_message(payload: &Value) -> Map<String, Value> {
    let empty = Value::Object(Map::new());
    let aps = payload.get("aps").filter(|a| crate::py::truthy(a)).unwrap_or(&empty);
    let alert = aps.get("alert").filter(|a| crate::py::truthy(a)).unwrap_or(&empty);
    let mut data = Map::new();
    if let Value::Object(alert) = alert {
        for key in ["title", "body"] {
            if let Some(v) = alert.get(key).filter(|v| crate::py::truthy(v)) {
                data.insert(key.into(), json!(crate::py::py_str(v)));
            }
        }
    }
    if let Some(category) = aps.get("category").filter(|c| crate::py::truthy(c)) {
        data.insert("category".into(), json!(crate::py::py_str(category)));
    }
    if aps.get("content-available").map(crate::py::truthy).unwrap_or(false) && !crate::py::truthy(alert) {
        data.insert("silent".into(), json!("1"));
    }
    if let Value::Object(map) = payload {
        for (key, value) in map {
            if key == "aps" || value.is_null() {
                continue;
            }
            let text = match value {
                Value::String(s) => s.clone(),
                other => crate::py::Dumps::new().unicode().dumps(other),
            };
            data.insert(key.clone(), json!(text));
        }
    }
    data
}

pub struct FcmClient {
    pub project: String,
    pub email: String,
    pub key: String,
    pub token_uri: String,
    pub base_url: String,
    pub client: reqwest::Client,
    pub clock: Clock,
    access: Mutex<Option<(String, f64)>>,
}

impl FcmClient {
    /// Reads the Firebase project's service account (JSON). `base_url` / `token_url`: `[fcm]`'s test overrides.
    pub fn new(
        service_account_path: &str,
        client: reqwest::Client,
        base_url: &str,
        token_url: Option<&str>,
    ) -> anyhow::Result<Self> {
        let account: Value = serde_json::from_str(&std::fs::read_to_string(service_account_path)?)?;
        let field = |k: &str| -> anyhow::Result<String> {
            account.get(k).and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow::anyhow!("KeyError: '{k}'"))
        };
        let token_uri = token_url
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| account.get("token_uri").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string))
            .unwrap_or_else(|| TOKEN_URL.to_string());
        Ok(FcmClient {
            project: field("project_id")?,
            email: field("client_email")?,
            key: field("private_key")?,
            token_uri,
            base_url: if base_url.is_empty() {
                BASE_URL.to_string()
            } else {
                base_url.trim_end_matches('/').to_string()
            },
            client,
            clock: crate::py::wall_clock(),
            access: Mutex::new(None),
        })
    }

    /// An OAuth access token for FCM (a JWT grant signed with the service account's key), kept until near expiry.
    /// Errors: a `reqwest::Error` is Python's `httpx.HTTPError` (an HTTP error status included, as
    /// `raise_for_status`); anything else is an exception Python lets through.
    pub async fn access_token(&self, force: bool) -> anyhow::Result<String> {
        let now = (self.clock)();
        let cached = self.access.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some((token, at)) = &cached {
            if !force && now - at < ACCESS_TTL_SECONDS {
                return Ok(token.clone());
            }
        }
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(self.key.as_bytes())
            .map_err(|e| PyError::new("ValueError", e.to_string()))?;
        let iat = now.floor() as i64;
        let claims = json!({"iss": self.email, "scope": SCOPE, "aud": self.token_uri, "iat": iat, "exp": iat + 3600});
        let assertion =
            encode_jwt(&json!({"typ": "JWT", "alg": "RS256"}), &claims, &key, jsonwebtoken::Algorithm::RS256)?;
        let resp = self
            .client
            .post(&self.token_uri)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", assertion.as_str())])
            .timeout(Duration::from_secs(15))
            .send()
            .await?;
        let resp = resp.error_for_status()?;
        let body = resp.bytes().await?;
        let data: Value = serde_json::from_slice(&body).map_err(|e| PyError::new("JSONDecodeError", e.to_string()))?;
        let token = match data.get("access_token") {
            Some(v) if data.is_object() => crate::py::py_str(v),
            _ => return Err(PyError::new("KeyError", "'access_token'").into()),
        };
        *self.access.lock().unwrap_or_else(|e| e.into_inner()) = Some((token.clone(), now));
        Ok(token)
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

    /// `send`, with what Python lets through as an exception (a token answer without `access_token`…).
    pub async fn try_send(
        &self,
        device_token: &str,
        payload: &Value,
        options: &SendOptions,
    ) -> anyhow::Result<ApnsResult> {
        let mut android = Map::new();
        android.insert("priority".into(), json!(if options.priority >= 10 { "HIGH" } else { "NORMAL" }));
        if let Some(collapse) = options.collapse_id.as_deref().filter(|c| !c.is_empty()) {
            android.insert("collapse_key".into(), json!(pyx::head(collapse, 64)));
        }
        let message = json!({"token": device_token, "data": data_message(payload), "android": android});
        let mut result = self.post(&message, false).await?;
        if result.status == 401 {
            result = self.post(&message, true).await?;
        }
        let (token, status, reason) = (redact6(device_token), result.status, J(&result.reason));
        if result.ok() {
            tracing::info!(token = %token, status = status, reason = ?reason, "fcm push");
        } else {
            tracing::warn!(token = %token, status = status, reason = ?reason, "fcm push");
        }
        Ok(result)
    }

    async fn post(&self, message: &Value, force: bool) -> anyhow::Result<ApnsResult> {
        let attempt: anyhow::Result<reqwest::Response> = async {
            let token = self.access_token(force).await?;
            Ok(self
                .client
                .post(format!("{}/v1/projects/{}/messages:send", self.base_url, self.project))
                .json(&json!({"message": message}))
                .timeout(Duration::from_secs(15))
                .header("Authorization", format!("Bearer {token}"))
                .send()
                .await?)
        }
        .await;
        let unreachable = |name: &str| {
            tracing::warn!(error = name, "fcm unreachable");
            ApnsResult { status: 0, reason: Some("NetworkError".into()), apns_id: None }
        };
        let resp = match attempt {
            Ok(resp) => resp,
            Err(exc) => match exc.downcast_ref::<reqwest::Error>() {
                Some(e) if e.is_status() => return Ok(unreachable("HTTPStatusError")),
                Some(e) => return Ok(unreachable(crate::hermes::httpx_error_name(e))),
                None => return Err(exc),
            },
        };
        let status = resp.status().as_u16();
        let body = match resp.bytes().await {
            Ok(body) => body,
            Err(e) => return Ok(unreachable(crate::hermes::httpx_error_name(&e))),
        };
        let parsed = serde_json::from_slice::<Value>(&body);
        if status == 200 {
            let data = parsed.map_err(|e| PyError::new("JSONDecodeError", e.to_string()))?;
            let Value::Object(data) = data else {
                return Err(PyError::new("AttributeError", "the FCM answer is not an object").into());
            };
            let name = data.get("name").filter(|n| !n.is_null()).map(crate::py::py_str);
            return Ok(ApnsResult { status: 200, reason: None, apns_id: name });
        }
        let mut reason = None;
        if let Ok(data) = parsed {
            let Value::Object(data) = data else {
                return Err(PyError::new("AttributeError", "the FCM answer is not an object").into());
            };
            let empty = Value::Object(Map::new());
            let error = data.get("error").unwrap_or(&empty);
            let Value::Object(error) = error else {
                return Err(PyError::new("AttributeError", "the FCM error is not an object").into());
            };
            let details: Vec<Value> = match error.get("details") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(|d| d.as_object())
                    .map(|d| d.get("errorCode").cloned().unwrap_or(Value::Null))
                    .collect(),
                Some(_) => return Err(PyError::new("TypeError", "the FCM error details are not a list").into()),
            };
            let first = details.into_iter().find(crate::py::truthy);
            let chosen = first.or_else(|| error.get("status").cloned()).filter(|v| !v.is_null());
            reason = chosen.map(|v| crate::py::py_str(&v));
        }
        // A token FCM will never accept again reads as APNs' 410, so PushService drops the device.
        let dead = reason.as_deref().map(|r| DEAD.contains(&r)).unwrap_or(false);
        let status = if dead && (status == 400 || status == 404) { 410 } else { status };
        Ok(ApnsResult { status, reason, apns_id: None })
    }
}
