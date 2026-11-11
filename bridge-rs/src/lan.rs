//! The local-network door: when Tailscale is down (no Internet at home, the VPN off), the app reaches the bridge
//! on the LAN, over HTTPS with a self-signed certificate it pins (the SHA-256 of the certificate travels in the pairing
//! QR code). Off unless `[lan] enabled = true`.
//!
//! Also builds the pairing payload (what the QR code carries) and the LAN details the app refreshes on its own
//! (`GET /v1/pairing`) so a new IP address needs no new QR code while the tailnet works.

use std::net::{IpAddr, UdpSocket};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::config::{AgentConfig, Config};
use crate::py;

pub const CERT_NAME: &str = "bridge-lan.pem";
pub const KEY_NAME: &str = "bridge-lan.key";

pub fn certificate_paths(config: &Config) -> (String, String) {
    (py::join(&config.lan.cert_dir, CERT_NAME), py::join(&config.lan.cert_dir, KEY_NAME))
}

/// The LAN certificate and key, created once (ECDSA P-256, 20 years: the app pins it, no CA involved).
pub fn ensure_certificate(config: &Config) -> anyhow::Result<(String, String)> {
    let (cert_path, key_path) = certificate_paths(config);
    if std::path::Path::new(&cert_path).exists() && std::path::Path::new(&key_path).exists() {
        return Ok((cert_path, key_path));
    }
    py::makedirs(&config.lan.cert_dir, 0o700)?;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new())?;
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, "BipAgents bridge (LAN)");
    params.distinguished_name = name;
    let now = chrono::Utc::now();
    let start = now - chrono::Duration::days(1);
    let end = now + chrono::Duration::days(365 * 20);
    use chrono::Datelike;
    params.not_before = rcgen::date_time_ymd(start.year(), start.month() as u8, start.day() as u8);
    params.not_after = rcgen::date_time_ymd(end.year(), end.month() as u8, end.day() as u8);
    // x509.random_serial_number(): 159 random bits, positive.
    let mut serial = [0u8; 20];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut serial);
    serial[0] &= 0x7f;
    params.serial_number = Some(rcgen::SerialNumber::from_slice(&serial));
    params.key_identifier_method = rcgen::KeyIdMethod::Sha256;
    let cert = params.self_signed(&key)?;
    py::write_with_mode(&key_path, key.serialize_pem().as_bytes(), 0o600)?;
    std::fs::write(&cert_path, cert.pem())?;
    tracing::info!(path = %cert_path, "LAN certificate created");
    Ok((cert_path, key_path))
}

/// SHA-256 of the certificate (DER), lowercase hex: what the app pins.
pub fn fingerprint(config: &Config) -> Option<String> {
    let (cert_path, _) = certificate_paths(config);
    let text = std::fs::read(&cert_path).ok()?;
    let block = pem::parse_many(&text).ok()?.into_iter().find(|p| p.tag() == "CERTIFICATE")?;
    Some(hex::encode(Sha256::digest(block.contents())))
}

/// This machine's address on its main network (no packet is sent: a UDP « connect » only picks the route).
pub fn detect_address() -> Option<String> {
    for target in ["192.168.0.1", "10.0.0.1", "172.16.0.1"] {
        let Ok(probe) = UdpSocket::bind("0.0.0.0:0") else { continue };
        if probe.connect((target, 9)).is_err() {
            continue;
        }
        let Ok(local) = probe.local_addr() else { continue };
        let ip = local.ip();
        let address = ip.to_string();
        let private = match ip {
            IpAddr::V4(v4) => v4.is_private(),
            IpAddr::V6(_) => false,
        };
        // 100.x: the tailnet itself
        if private && !ip.is_loopback() && !address.starts_with("100.") {
            return Some(address);
        }
    }
    None
}

/// {"url", "fingerprint"} for the app, or None when the door is off or not ready. A place of the offer holding its
/// bipagents.com certificate (place_cert.rs) adds `host` and `host_expires` (unix seconds): the same door reached as
/// `https://<host>:<port>` is trusted by any client, iPhones included, without pinning.
pub fn lan_info(config: &Config) -> Option<Value> {
    if !config.lan.enabled {
        return None;
    }
    let address = config.lan.address.clone().filter(|a| !a.is_empty()).or_else(detect_address)?;
    let print = fingerprint(config)?;
    let mut info = json!({"url": format!("https://{address}:{}", config.lan.port), "fingerprint": print});
    if let Some(held) = crate::place_cert::held_for(config) {
        info["host"] = json!(held.host);
        info["host_expires"] = json!(held.expires);
    }
    Some(info)
}

/// What an agent's pairing QR code carries (the app's « Scanner le QR code »): the bridge's tailnet address and key,
/// the agent's name and voice, and the LAN door. Err when the bridge's tailnet address is missing from the config.
pub fn pairing_payload(config: &Config, agent: &AgentConfig) -> Result<Value, String> {
    let Some(bridge) = config.public_url.as_deref().filter(|u| !u.is_empty()) else {
        return Err("public_url is missing (the bridge's address on the tailnet)".into());
    };
    let mut payload = Map::new();
    payload.insert("name".into(), json!(agent.display_name));
    payload.insert("agent".into(), json!(agent.name));
    payload.insert("bridgeURL".into(), json!(bridge));
    payload.insert("bridgeKey".into(), json!(config.bridge_key));
    if let Some(voice) = agent.voice.as_deref().filter(|v| !v.is_empty()) {
        payload.insert("voice".into(), json!(voice));
    }
    if let Some(lan) = lan_info(config) {
        payload.insert("lan".into(), lan);
    }
    Ok(Value::Object(payload))
}

/// The install's QR code: the bridge's tailnet address and key (and the LAN door). The app then asks the bridge
/// for every agent (`GET /v1/agents`), so one scan adds them all, and agents created later show up too.
pub fn install_payload(config: &Config) -> Result<Value, String> {
    let Some(bridge) = config.public_url.as_deref().filter(|u| !u.is_empty()) else {
        return Err("public_url is missing (the bridge's address on the tailnet)".into());
    };
    let mut payload = Map::new();
    payload.insert("v".into(), json!(2));
    payload.insert("bridgeURL".into(), json!(bridge));
    payload.insert("bridgeKey".into(), json!(config.bridge_key));
    if let Some(lan) = lan_info(config) {
        payload.insert("lan".into(), lan);
    }
    Ok(Value::Object(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lan_config(dir: &std::path::Path) -> Config {
        let mut config = Config::with_key("k".repeat(40));
        config.lan.enabled = true;
        config.lan.address = Some("192.168.1.177".into());
        config.lan.cert_dir = dir.join("lan").to_string_lossy().into_owned();
        config
    }

    #[test]
    fn certificate_is_made_once_and_pinned_by_its_hash() {
        let dir = tempfile::tempdir().unwrap();
        let config = lan_config(dir.path());
        let (cert, key) = ensure_certificate(&config).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&key).unwrap().permissions().mode() & 0o777, 0o600);
        let first = fingerprint(&config).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(ensure_certificate(&config).unwrap(), (cert, key));
        assert_eq!(fingerprint(&config).unwrap(), first);
        assert_eq!(lan_info(&config).unwrap(), json!({"url": "https://192.168.1.177:8650", "fingerprint": first}));
    }

    #[test]
    fn pairing_payload_carries_tailnet_addresses_keys_and_lan() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = lan_config(dir.path());
        config.public_url = Some("https://box.example.ts.net:8643".into());
        let agent = AgentConfig {
            name: "wellness".into(),
            display_name: "Wellness".into(),
            hermes_url: "http://hermes-wellness.test".into(),
            hermes_key: "hermes-wellness-key-xyz".into(),
            public_url: Some("https://box.example.ts.net:8642".into()),
            ..Default::default()
        };
        ensure_certificate(&config).unwrap();
        let payload = pairing_payload(&config, &agent).unwrap();
        assert_eq!(payload["agent"], "wellness");
        assert_eq!(payload["bridgeURL"], "https://box.example.ts.net:8643");
        assert_eq!(
            payload["lan"],
            json!({"url": "https://192.168.1.177:8650", "fingerprint": fingerprint(&config).unwrap()})
        );
        let keys: Vec<&String> = payload.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["name", "agent", "bridgeURL", "bridgeKey", "lan"]);
        let bridge = config.public_url.take();
        assert!(pairing_payload(&config, &agent).unwrap_err().contains("public_url"));
        config.public_url = bridge;
        let install = install_payload(&config).unwrap();
        assert_eq!(install["v"], 2);
    }

    #[test]
    fn a_place_certificate_adds_its_host() {
        let dir = tempfile::tempdir().unwrap();
        let config = lan_config(dir.path());
        ensure_certificate(&config).unwrap();
        assert!(lan_info(&config).unwrap().get("host").is_none());
        let lan = std::path::Path::new(&config.lan.cert_dir);
        std::fs::write(lan.join(crate::place_cert::CERT_NAME), "chain").unwrap();
        std::fs::write(lan.join(crate::place_cert::KEY_NAME), "key").unwrap();
        let meta = json!({"host": "k3x9q2.p.bipagents.com", "expires": 1_900_000_000});
        std::fs::write(lan.join(crate::place_cert::META_NAME), meta.to_string()).unwrap();
        let info = lan_info(&config).unwrap();
        assert_eq!(info["host"], "k3x9q2.p.bipagents.com");
        assert_eq!(info["host_expires"], 1_900_000_000);
        assert_eq!(info["url"], "https://192.168.1.177:8650");
    }

    #[test]
    fn lan_off_by_default() {
        let config = Config::with_key("k".repeat(40));
        assert!(!config.lan.enabled);
        assert!(lan_info(&config).is_none());
    }
}
