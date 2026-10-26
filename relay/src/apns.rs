//! Apple: one HTTP/2 client kept open, a provider token (ES256 JWT) renewed every 40 minutes.

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::Value;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Outcome;

const RENEW_AFTER: Duration = Duration::from_secs(40 * 60);
const PRODUCTION: &str = "https://api.push.apple.com";
const SANDBOX: &str = "https://api.sandbox.push.apple.com";

pub struct ApnsConfig {
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    pub topic: String,
    /// Tests only: one base URL for both environments.
    pub url_override: Option<String>,
}

pub struct Apns {
    http: reqwest::Client,
    key: EncodingKey,
    key_id: String,
    team_id: String,
    topic: String,
    url_override: Option<String>,
    token: RwLock<Option<(String, Instant)>>,
}

#[derive(serde::Serialize)]
struct Claims<'a> {
    iss: &'a str,
    iat: u64,
}

impl Apns {
    pub fn new(config: ApnsConfig, http: reqwest::Client) -> Result<Self, String> {
        let key = EncodingKey::from_ec_pem(config.key_pem.as_bytes()).map_err(|e| format!("APNs key: {e}"))?;
        Ok(Self {
            http,
            key,
            key_id: config.key_id,
            team_id: config.team_id,
            topic: config.topic,
            url_override: config.url_override,
            token: RwLock::new(None),
        })
    }

    fn provider_token(&self, renew: bool) -> String {
        if !renew {
            if let Some((token, made)) = self.token.read().expect("apns token lock").as_ref() {
                if made.elapsed() < RENEW_AFTER {
                    return token.clone();
                }
            }
        }
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let iat = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let token = jsonwebtoken::encode(&header, &Claims { iss: &self.team_id, iat }, &self.key)
            .expect("signing with a loaded key does not fail");
        *self.token.write().expect("apns token lock") = Some((token.clone(), Instant::now()));
        token
    }

    pub async fn send(
        &self,
        device: &str,
        environment: &str,
        payload: &Value,
        push_type: &str,
        priority: u8,
        collapse_id: Option<&str>,
    ) -> Outcome {
        let mut outcome = self.post(device, environment, payload, push_type, priority, collapse_id, false).await;
        if outcome.status == 403 && outcome.reason.as_deref() == Some("ExpiredProviderToken") {
            outcome = self.post(device, environment, payload, push_type, priority, collapse_id, true).await;
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    async fn post(
        &self,
        device: &str,
        environment: &str,
        payload: &Value,
        push_type: &str,
        priority: u8,
        collapse_id: Option<&str>,
        renew: bool,
    ) -> Outcome {
        let base = self.url_override.as_deref().unwrap_or(if environment == "sandbox" { SANDBOX } else { PRODUCTION });
        let mut request = self
            .http
            .post(format!("{base}/3/device/{device}"))
            .header("authorization", format!("bearer {}", self.provider_token(renew)))
            // a Live Activity's updates go to the app's topic with Apple's suffix
            .header(
                "apns-topic",
                if push_type == "liveactivity" {
                    format!("{}.push-type.liveactivity", self.topic)
                } else {
                    self.topic.clone()
                },
            )
            .header("apns-push-type", push_type)
            .header("apns-priority", priority.to_string())
            .json(payload);
        if let Some(id) = collapse_id {
            request = request.header("apns-collapse-id", id);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                let id = response.headers().get("apns-id").and_then(|v| v.to_str().ok()).map(String::from);
                if status == 200 {
                    return Outcome { status, reason: None, id };
                }
                let reason = response
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|body| body.get("reason").and_then(Value::as_str).map(String::from));
                Outcome { status, reason, id }
            }
            Err(error) => {
                tracing::warn!(error = %error.without_url(), "apns unreachable");
                Outcome { status: 503, reason: Some("ApnsUnreachable".into()), id: None }
            }
        }
    }
}
