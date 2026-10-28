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
//! - `RELAY_GUICHET_KEY` (or `_FILE`): the key the guichet sends its pushes with (tokens sealed for the guichet)
//! - `RELAY_R1_UNTIL` (`2026-12-15`, UTC midnight): until then the first token format (`r1.`) is still taken and
//!   still sealed for older apps, and a guichet token sent without the guichet's key is let through (an older
//!   guichet); each such use is logged and `/health` warns. `0` closes the transition now.

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
    /// Devices refused (terms of use: abuse), by the 8-hex fingerprint the logs show, or single relay tokens by their
    /// 16-hex id (a token that leaked): `RELAY_BLOCKED` (separated by commas or spaces) or `RELAY_BLOCKED_FILE` (one
    /// per line, `#` comments).
    pub blocked: Vec<String>,
    /// The guichet's keys (`RELAY_GUICHET_KEY` or `_FILE`, comma-separated to change it without a gap): a token
    /// sealed for the guichet is sent with one of them, nothing else; none set: such tokens are refused.
    pub guichet_keys: Vec<String>,
    /// The end of the transition from the first token format (Unix seconds; 0: over).
    pub r1_until: u64,
}

/// The default end of the transition: 2026-12-15, 00:00 UTC.
pub const R1_UNTIL_DEFAULT: &str = "2026-12-15";

/// `YYYY-MM-DD` as Unix seconds at 00:00 UTC; `0` as 0.
pub fn parse_day(day: &str) -> Option<u64> {
    if day == "0" {
        return Some(0);
    }
    let mut parts = day.split('-');
    let (y, m, d): (i64, i64, i64) =
        (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    if parts.next().is_some() || !(1970..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days from civil (H. Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400).ok()
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
                .filter(|item| (item.len() == 8 || item.len() == 16) && item.chars().all(|c| c.is_ascii_hexdigit()))
                .collect(),
            guichet_keys: inline_or_file("RELAY_GUICHET_KEY")?
                .unwrap_or_default()
                .split(',')
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect(),
            r1_until: parse_day(&var("RELAY_R1_UNTIL").unwrap_or_else(|| R1_UNTIL_DEFAULT.into()))
                .ok_or("RELAY_R1_UNTIL must be a day (YYYY-MM-DD) or 0")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_reads_as_its_utc_midnight() {
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(parse_day("2000-03-01"), Some(951_868_800));
        assert_eq!(parse_day(R1_UNTIL_DEFAULT), Some(1_797_292_800));
        assert_eq!(parse_day("0"), Some(0));
        for bad in ["2026-13-01", "2026-12", "15/12/2026", "2026-12-15-1", ""] {
            assert_eq!(parse_day(bad), None, "{bad}");
        }
    }
}
