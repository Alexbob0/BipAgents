//! Settings, from the environment only (one container, nothing to mount but secrets).
//!
//! - `RELAY_SEAL_KEYS` (required): base64 keys of 32 bytes, comma-separated, the first one sealing
//!   (`bipagents-relay keygen` makes one)
//! - `APNS_KEY` or `APNS_KEY_FILE` (the team's .p8), `APNS_KEY_ID`, `APNS_TEAM_ID`, `APNS_TOPIC` (the app's bundle
//!   id): Apple; left out, Apple devices are refused
//! - `FCM_SERVICE_ACCOUNT` or `FCM_SERVICE_ACCOUNT_FILE` (the JSON key): Google; left out, Android devices are refused
//! - `PORT` (default 8080) or `RELAY_LISTEN` (`0.0.0.0:8080`)
//! - `RELAY_PUSHES_PER_DEVICE_HOUR` (300), `RELAY_PUSHES_PER_IP_HOUR` (20000), `RELAY_REGISTRATIONS_PER_IP_HOUR` (60)
//! - `RELAY_TRUST_FORWARDED_FOR=1` behind a proxy that sets `X-Forwarded-For` (the quotas then count its first address)

use std::net::SocketAddr;

use crate::apns::ApnsConfig;
use crate::fcm::ServiceAccount;

pub struct Config {
    pub listen: SocketAddr,
    pub seal_keys: String,
    pub apns: Option<ApnsConfig>,
    pub fcm: Option<ServiceAccount>,
    /// Tests only: where Google's send API is.
    pub fcm_send_base: Option<String>,
    pub pushes_per_device_hour: u32,
    pub pushes_per_ip_hour: u32,
    pub registrations_per_ip_hour: u32,
    pub trust_forwarded_for: bool,
    /// Where « Report this reply » goes on (the guichet's /v1/reports), with the secret it checks; none: refused.
    pub report_url: Option<String>,
    pub report_secret: Option<String>,
    pub reports_per_ip_hour: u32,
    /// Devices refused (terms of use: abuse), by the 8-hex fingerprint the logs show: `RELAY_BLOCKED` (separated by
    /// commas or spaces) or `RELAY_BLOCKED_FILE` (one per line, `#` comments).
    pub blocked: Vec<String>,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// `NAME`, else the contents of the file named by `NAME_FILE`.
fn inline_or_file(name: &str) -> Result<Option<String>, String> {
    if let Some(value) = var(name) {
        return Ok(Some(value));
    }
    match var(&format!("{name}_FILE")) {
        Some(path) => std::fs::read_to_string(&path).map(Some).map_err(|e| format!("{name}_FILE ({path}): {e}")),
        None => Ok(None),
    }
}

fn number(name: &str, default: u32) -> Result<u32, String> {
    var(name).map_or(Ok(default), |v| v.parse().map_err(|_| format!("{name} must be a number")))
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let listen = match var("RELAY_LISTEN") {
            Some(addr) => addr.parse().map_err(|_| "RELAY_LISTEN must be host:port".to_string())?,
            None => SocketAddr::from(([0, 0, 0, 0], number("PORT", 8080)? as u16)),
        };
        let seal_keys = var("RELAY_SEAL_KEYS").ok_or("RELAY_SEAL_KEYS is required (bipagents-relay keygen)")?;
        let apns = match inline_or_file("APNS_KEY")? {
            Some(key_pem) => Some(ApnsConfig {
                key_pem,
                key_id: var("APNS_KEY_ID").ok_or("APNS_KEY_ID is required with APNS_KEY")?,
                team_id: var("APNS_TEAM_ID").ok_or("APNS_TEAM_ID is required with APNS_KEY")?,
                topic: var("APNS_TOPIC").ok_or("APNS_TOPIC (the app's bundle id) is required with APNS_KEY")?,
                url_override: var("APNS_URL"),
            }),
            None => None,
        };
        let fcm = match inline_or_file("FCM_SERVICE_ACCOUNT")? {
            Some(json) => Some(serde_json::from_str(&json).map_err(|e| format!("FCM_SERVICE_ACCOUNT: {e}"))?),
            None => None,
        };
        Ok(Self {
            listen,
            seal_keys,
            apns,
            fcm,
            fcm_send_base: var("FCM_URL"),
            pushes_per_device_hour: number("RELAY_PUSHES_PER_DEVICE_HOUR", 300)?,
            pushes_per_ip_hour: number("RELAY_PUSHES_PER_IP_HOUR", 20000)?,
            registrations_per_ip_hour: number("RELAY_REGISTRATIONS_PER_IP_HOUR", 60)?,
            trust_forwarded_for: var("RELAY_TRUST_FORWARDED_FOR").is_some_and(|v| v == "1" || v == "true"),
            report_url: var("RELAY_REPORT_URL"),
            report_secret: inline_or_file("RELAY_REPORT_SECRET")?
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
            reports_per_ip_hour: number("RELAY_REPORTS_PER_IP_HOUR", 20)?,
            blocked: inline_or_file("RELAY_BLOCKED")?
                .unwrap_or_default()
                .lines()
                .map(|line| line.split('#').next().unwrap_or(""))
                .flat_map(|line| line.split([',', ' ', '\t']))
                .map(|item| item.trim().to_ascii_lowercase())
                .filter(|item| item.len() == 8 && item.chars().all(|c| c.is_ascii_hexdigit()))
                .collect(),
        })
    }
}
