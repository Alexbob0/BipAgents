//! The publisher's push relay (relay/), seen from the bridge: its device tokens (an app with no Apple key of the
//! install's own registers there) and the client that sends through it (`[push] service_url`).

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

use crate::apns::{ApnsResult, SendOptions};
use crate::push::pyx::{self, PyError};

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

/// Sends through the publisher's relay (`[push] service_url`, relay/).
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
