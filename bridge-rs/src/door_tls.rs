//! HTTPS between two bridges (households.rs): which certificate this bridge's address serves, and the client that
//! checks the other side's.
//!
//! A place of the all-included offer is reached at its server's IP address, on the door (lan.rs) with a self-signed
//! certificate: the phone apps pin its SHA-256, learnt at pairing. A self-hosted install is reached at its tailnet
//! name (`tailscale serve`, a Let's Encrypt certificate) or at a domain of the person's: publicly trusted certificates.
//!
//! So a household link keeps the other side's `fingerprint` when its address is a door: the invitation (QR code or
//! sealed through the guichet) carries the inviter's, the joining bridge sends its own with `/family/accept`. A call
//! to that household then accepts exactly that certificate, whatever the address's name, and nothing else; a link
//! without one is checked as any website (WebPKI).
//!
//! Links made before (no fingerprint stored) heal on their own: the other bridge announces its fingerprint at start
//! (`POST /family/door`, with the key we gave it, over a connection to us that our own certificate secures); until
//! then a link to an IP address without a fingerprint is shown as « to be redone » (`relink` in `/v1/family`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::Dict;

/// How often a bridge with a door tells its linked households its certificate (start, then this).
pub const ANNOUNCE_EVERY: Duration = Duration::from_secs(6 * 3600);

/// A SHA-256 in lowercase hex (what the apps pin).
pub fn valid(fingerprint: &str) -> bool {
    fingerprint.len() == 64 && fingerprint.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `Some("aa:BB…")` → `Some("aabb…")` when it is a SHA-256; None otherwise.
pub fn normalize(given: &str) -> Option<String> {
    let plain: String = given.trim().chars().filter(|c| *c != ':').collect::<String>().to_ascii_lowercase();
    valid(&plain).then_some(plain)
}

/// The certificate this bridge's own address (`public_url`) serves, when it is the door's self-signed one: the
/// fingerprint the other households must pin. None when the address is a tailnet name or a domain (WebPKI).
pub fn door_fingerprint(config: &Config) -> Option<String> {
    if !config.lan.enabled {
        return None;
    }
    let public = url::Url::parse(config.public_url.as_deref()?).ok()?;
    let door = crate::lan::lan_info(config)
        .and_then(|l| l.get("url").and_then(Value::as_str).and_then(|u| url::Url::parse(u).ok()));
    let same = door.is_some_and(|d| {
        d.host_str() == public.host_str() && d.port_or_known_default() == public.port_or_known_default()
    });
    if !(config.cloud || same) {
        return None;
    }
    crate::lan::fingerprint(config)
}

/// An HTTPS address whose host is an IP literal: no public certificate there in practice (a door).
pub fn door_address(url: &str) -> bool {
    url::Url::parse(url)
        .ok()
        .is_some_and(|u| u.scheme() == "https" && matches!(u.host(), Some(url::Host::Ipv4(_) | url::Host::Ipv6(_))))
}

/// A link that cannot work until it is made again (or the other bridge announces its certificate): a door's
/// address, no fingerprint to check it with.
pub fn needs_relink(entry: &Dict) -> bool {
    let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
    let pinned = entry.get("fingerprint").and_then(Value::as_str).is_some_and(valid);
    !pinned && door_address(url)
}

/// Accepts exactly one certificate (its SHA-256), whatever the name asked for; the handshake's signatures are still
/// checked, so only the holder of that certificate's key gets through.
#[derive(Debug)]
struct Pinned {
    pin: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let seen = Sha256::digest(end_entity.as_ref());
        if crate::auth::constant_time_eq(seen.as_slice(), &self.pin) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn pinned_client(fingerprint: &str) -> anyhow::Result<reqwest::Client> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let pin = hex::decode(fingerprint)?;
    let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { pin, provider }))
        .with_no_client_auth();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(reqwest::Client::builder()
        .use_preconfigured_tls(tls)
        .connect_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(4)
        .build()?)
}

/// A client that trusts nothing (a fingerprint that could not be used must not fall back to WebPKI).
fn refusing() -> reqwest::Client {
    reqwest::Client::builder().tls_built_in_root_certs(false).https_only(true).build().unwrap_or_default()
}

static CLIENTS: LazyLock<Mutex<HashMap<String, reqwest::Client>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The client for a bridge serving `fingerprint` (one per certificate, kept), or `base` (WebPKI) without one.
pub fn client_for(base: &reqwest::Client, fingerprint: Option<&str>) -> reqwest::Client {
    let Some(fingerprint) = fingerprint.and_then(normalize) else { return base.clone() };
    let mut clients = CLIENTS.lock().unwrap_or_else(|e| e.into_inner());
    clients
        .entry(fingerprint.clone())
        .or_insert_with(|| {
            pinned_client(&fingerprint).unwrap_or_else(|e| {
                tracing::error!(error = %e, "pinned client");
                refusing()
            })
        })
        .clone()
}

/// The client for one linked household (its entry in households.json).
pub fn household_client(base: &reqwest::Client, entry: &Dict) -> reqwest::Client {
    client_for(base, entry.get("fingerprint").and_then(Value::as_str))
}

/// Tells each linked household this bridge's door certificate (`POST /family/door`), so that links made before
/// fingerprints travelled with invitations work again. Nothing to say when this bridge's address is not a door.
pub async fn announce(http: &reqwest::Client, config: &Config, households: &crate::households::Households) {
    let Some(own) = door_fingerprint(config) else { return };
    let linked = households.load();
    let Some(entries) = linked.get("households").and_then(Value::as_object) else { return };
    for (id, entry) in entries {
        let Some(entry) = entry.as_object() else { continue };
        let url = entry.get("url").and_then(Value::as_str).unwrap_or_default();
        let key = entry.get("key_out").and_then(Value::as_str).unwrap_or_default();
        let answer = household_client(http, entry)
            .post(format!("{url}/family/door"))
            .timeout(Duration::from_secs(10))
            .bearer_auth(key)
            .json(&json!({"fingerprint": own, "url": config.public_url}))
            .send()
            .await;
        match answer {
            Ok(resp) if resp.status().is_success() => {}
            Ok(resp) => tracing::info!(household = %id, status = resp.status().as_u16(), "door certificate not taken"),
            Err(_) => tracing::info!(household = %id, "door certificate not told (their bridge does not answer)"),
        }
    }
}

/// [`announce`] at start, then every [`ANNOUNCE_EVERY`].
pub async fn announce_forever(state: crate::app::AppState) {
    loop {
        announce(&state.http, &state.config(), &state.households).await;
        tokio::time::sleep(ANNOUNCE_EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_and_addresses() {
        let fp = "ab".repeat(32);
        assert!(valid(&fp) && !valid(&fp.to_uppercase()) && !valid("abc"));
        assert_eq!(normalize(&"AB:".repeat(31).chars().chain("AB".chars()).collect::<String>()), Some(fp.clone()));
        assert!(door_address("https://203.0.113.4") && door_address("https://[2001:db8::1]:8443"));
        assert!(!door_address("https://alex.tail1234.ts.net:8443") && !door_address("http://100.64.0.1:8642"));
        let entry = |url: &str, fp: Option<&str>| {
            let mut d = Dict::new();
            d.insert("url".into(), json!(url));
            if let Some(fp) = fp {
                d.insert("fingerprint".into(), json!(fp));
            }
            d
        };
        assert!(needs_relink(&entry("https://203.0.113.4", None)));
        assert!(!needs_relink(&entry("https://203.0.113.4", Some(&fp))));
        assert!(!needs_relink(&entry("https://alex.tail1234.ts.net:8443", None)));
    }
}
