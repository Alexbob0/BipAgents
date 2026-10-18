//! Google: Firebase Cloud Messaging HTTP v1, with an access token from the service account (renewed every 50
//! minutes, or at once when Google refuses it).

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

use crate::payload::fcm_data;
use crate::Outcome;

const RENEW_AFTER: Duration = Duration::from_secs(50 * 60);
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const SEND_BASE: &str = "https://fcm.googleapis.com";
/// Errors after which Google will never accept the token again: told to the bridge as Apple's 410.
const DEAD: &[&str] = &["UNREGISTERED", "INVALID_ARGUMENT", "SENDER_ID_MISMATCH"];

#[derive(Deserialize)]
pub struct ServiceAccount {
    pub project_id: String,
    pub client_email: String,
    pub private_key: String,
    #[serde(default = "default_token_uri")]
    pub token_uri: String,
}

fn default_token_uri() -> String {
    "https://oauth2.googleapis.com/token".into()
}

pub struct Fcm {
    http: reqwest::Client,
    project: String,
    email: String,
    key: EncodingKey,
    token_uri: String,
    send_base: String,
    access: Mutex<Option<(String, Instant)>>,
}

#[derive(serde::Serialize)]
struct Claims<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
}

impl Fcm {
    pub fn new(account: ServiceAccount, http: reqwest::Client, send_base: Option<String>) -> Result<Self, String> {
        let key = EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|e| format!("FCM key: {e}"))?;
        Ok(Self {
            http,
            project: account.project_id,
            email: account.client_email,
            key,
            token_uri: account.token_uri,
            send_base: send_base.unwrap_or_else(|| SEND_BASE.into()),
            access: Mutex::new(None),
        })
    }

    async fn access_token(&self, renew: bool) -> Result<String, String> {
        let mut access = self.access.lock().await;
        if !renew {
            if let Some((token, made)) = access.as_ref() {
                if made.elapsed() < RENEW_AFTER {
                    return Ok(token.clone());
                }
            }
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let claims = Claims { iss: &self.email, scope: SCOPE, aud: &self.token_uri, iat: now, exp: now + 3600 };
        let assertion = jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &self.key)
            .map_err(|e| format!("signing: {e}"))?;
        let response = self
            .http
            .post(&self.token_uri)
            .form(&[("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"), ("assertion", &assertion)])
            .send()
            .await
            .map_err(|e| format!("token request: {}", e.without_url()))?;
        let body: Value = response.json().await.map_err(|_| "token response is not JSON".to_string())?;
        let token = body.get("access_token").and_then(Value::as_str).ok_or("no access_token")?.to_string();
        *access = Some((token.clone(), Instant::now()));
        Ok(token)
    }

    pub async fn send(&self, device: &str, payload: &Value, priority: u8, collapse_id: Option<&str>) -> Outcome {
        let mut android = json!({"priority": if priority >= 10 { "HIGH" } else { "NORMAL" }});
        if let Some(id) = collapse_id {
            android["collapse_key"] = Value::String(id.chars().take(64).collect());
        }
        let message = json!({"message": {"token": device, "data": fcm_data(payload), "android": android}});
        let mut outcome = self.post(&message, false).await;
        if outcome.status == 401 {
            outcome = self.post(&message, true).await;
        }
        outcome
    }

    async fn post(&self, message: &Value, renew: bool) -> Outcome {
        let token = match self.access_token(renew).await {
            Ok(token) => token,
            Err(error) => {
                tracing::warn!(%error, "fcm access token");
                return Outcome { status: 503, reason: Some("FcmAuthFailed".into()), id: None };
            }
        };
        let url = format!("{}/v1/projects/{}/messages:send", self.send_base, self.project);
        let response = match self.http.post(url).bearer_auth(token).json(message).send().await {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error.without_url(), "fcm unreachable");
                return Outcome { status: 503, reason: Some("FcmUnreachable".into()), id: None };
            }
        };
        let status = response.status().as_u16();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status == 200 {
            return Outcome { status, reason: None, id: body.get("name").and_then(Value::as_str).map(String::from) };
        }
        let error = body.get("error");
        let detail = error
            .and_then(|e| e.get("details"))
            .and_then(Value::as_array)
            .and_then(|items| items.iter().find_map(|d| d.get("errorCode").and_then(Value::as_str)));
        let reason = detail.or_else(|| error.and_then(|e| e.get("status")).and_then(Value::as_str)).map(String::from);
        let dead = matches!(status, 400 | 404) && reason.as_deref().is_some_and(|r| DEAD.contains(&r));
        Outcome { status: if dead { 410 } else { status }, reason, id: None }
    }
}
