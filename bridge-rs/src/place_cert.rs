//! A real certificate for a place of the offer: `<label>.p.bipagents.com`, from Let's Encrypt, through the guichet.
//!
//! The door (lan.rs) serves a self-signed certificate that the apps pin. iPhones refuse a self-signed certificate on a
//! public address (App Transport Security, and no exception in the app: Alex 2026-10-30), so each place of the offer
//! also gets a hostname under bipagents.com and a publicly trusted certificate for it. The guichet owns the DNS zone
//! and answers Let's Encrypt's DNS-01 challenge; the place only sends a certificate request (CSR). The private key is
//! made here and never leaves this place's disk.
//!
//!     GET  {guichet}/v1/places/host   Bearer <backup token>   ->  200 {"host"}
//!     POST {guichet}/v1/places/cert   Bearer, {"csr": PEM}    ->  202 {"retry_after"}            pending: same CSR again
//!                                                                 200 {"host", "certificate", "expires"}
//!                                                                 429 / 503 {"detail", "retry_after"}   back off
//!                                                                 404 / 401   not a place the guichet knows: stop
//!
//! Which certificate a connection gets is decided at its TLS hello ([`DoorCerts`]): the Let's Encrypt one when the
//! client names the place's host (SNI), the self-signed one otherwise (an IP address, a LAN name, no name at all), so
//! apps that pin the fingerprint see exactly what they saw before. A new certificate is swapped in without a restart.
//!
//! At start, then every day: nothing to do while the certificate held has 30 days or more left. Otherwise a new key
//! and CSR are made and kept on disk while the guichet works on it (`place-pending.*`), so a restart asks again with
//! the same CSR instead of starting a second order. Self-installs have no guichet token: nothing is asked there.
//!
//!     <lan.cert_dir>/   (next to bridge-lan.pem / bridge-lan.key, outside the encrypted space: the locked door uses it)
//!         place-cert.pem          the chain, leaf first
//!         place-key.pem           its P-256 key (0600)
//!         place-cert.json         {"host", "expires"} (unix seconds)
//!         place-pending-key.pem   while an order is pending (0600)
//!         place-pending.json      {"host", "csr"} while an order is pending

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwapOption;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

pub const CERT_NAME: &str = "place-cert.pem";
pub const KEY_NAME: &str = "place-key.pem";
pub const META_NAME: &str = "place-cert.json";
pub const PENDING_KEY_NAME: &str = "place-pending-key.pem";
pub const PENDING_NAME: &str = "place-pending.json";

/// A certificate with less than this left is renewed (Let's Encrypt's are 90 days: renewed around day 60).
pub const RENEW_BEFORE: i64 = 30 * 86_400;
/// Without guichet settings (yet), how soon they are looked for again.
const NO_GUICHET_RECHECK: Duration = Duration::from_secs(3600);

/// How the renewal paces itself. Tests shrink every wait to nothing.
#[derive(Clone, Debug)]
pub struct Pace {
    /// Between two checks once a certificate is held (or nothing could be done).
    pub check_every: Duration,
    /// A pending order is asked again after the guichet's `retry_after`, never sooner than this…
    pub pending_min: Duration,
    /// …nor later than this.
    pub pending_max: Duration,
    /// The first wait after a failure (the guichet busy, unreachable…); it doubles with each failure in a row.
    pub backoff_min: Duration,
    /// The longest wait after failures.
    pub backoff_max: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace {
            check_every: Duration::from_secs(86_400),
            pending_min: Duration::from_secs(5),
            pending_max: Duration::from_secs(3600),
            backoff_min: Duration::from_secs(60),
            backoff_max: Duration::from_secs(6 * 3600),
        }
    }
}

/// The certificate a place holds: for which host, until when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub host: String,
    pub expires: i64,
}

impl Held {
    /// `{"host", "expires"}` for the apps and the admin.
    pub fn json(&self) -> Value {
        json!({"host": self.host, "expires": self.expires})
    }
}

/// Whether a new certificate must be asked for: none held, or one with less than [`RENEW_BEFORE`] left.
pub fn needs_renewal(held: Option<&Held>, now: i64) -> bool {
    held.is_none_or(|h| h.expires - now < RENEW_BEFORE)
}

/// The wait after `failures` failures in a row (1 = the first): the guichet's `retry_after` when it asks for more,
/// else [`Pace::backoff_min`] doubling each time, never more than [`Pace::backoff_max`].
pub fn backoff(pace: &Pace, failures: u32, retry_after: Duration) -> Duration {
    let doubled = pace.backoff_min.saturating_mul(1u32 << failures.saturating_sub(1).min(20));
    doubled.max(retry_after).max(pace.backoff_min).min(pace.backoff_max)
}

// -- serving ---------------------------------------------------------------------------------------------------------

/// The place's certificate as rustls serves it, with what it is for.
struct Placed {
    held: Held,
    key: Arc<CertifiedKey>,
}

/// The door's certificates: the self-signed one (always) and the place's Let's Encrypt one (once obtained), picked
/// per connection from the name the client asks for. [`DoorCerts::reload`] swaps the second one in while connections
/// go on; a handshake already started keeps the one it got.
pub struct DoorCerts {
    fallback: Arc<CertifiedKey>,
    place: ArcSwapOption<Placed>,
    dir: PathBuf,
}

impl std::fmt::Debug for DoorCerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DoorCerts").field("dir", &self.dir).field("place", &self.serving()).finish()
    }
}

impl ResolvesServerCert for DoorCerts {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let (Some(asked), Some(place)) = (hello.server_name(), self.place.load().as_ref()) {
            if asked.eq_ignore_ascii_case(&place.held.host) {
                return Some(place.key.clone());
            }
        }
        Some(self.fallback.clone())
    }
}

impl DoorCerts {
    /// The self-signed certificate at `cert` / `key` (bridge-lan.*), and the place's one from the same folder when
    /// it holds a good one.
    pub fn load(cert: &str, key: &str) -> anyhow::Result<Arc<DoorCerts>> {
        let fallback = Arc::new(certified(&std::fs::read(cert)?, &std::fs::read(key)?)?);
        let dir = Path::new(cert).parent().map(Path::to_path_buf).unwrap_or_default();
        let door = Arc::new(DoorCerts { fallback, place: ArcSwapOption::empty(), dir });
        door.reload();
        Ok(door)
    }

    /// The folder the place's files live in (the self-signed certificate's).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Reads the place's certificate from disk again and serves it from the next handshake on. Files missing or
    /// unusable (a key that is not the certificate's…): the self-signed certificate only, as before any order.
    pub fn reload(&self) -> Option<Held> {
        match load_placed(&self.dir) {
            Ok(Some(placed)) => {
                let held = placed.held.clone();
                self.place.store(Some(Arc::new(placed)));
                Some(held)
            }
            Ok(None) => {
                self.place.store(None);
                None
            }
            Err(e) => {
                tracing::warn!(error = %format!("{e:#}"), "place certificate unusable: serving the door's own only");
                self.place.store(None);
                None
            }
        }
    }

    /// What is served to clients naming the place's host, if anything.
    pub fn serving(&self) -> Option<Held> {
        self.place.load().as_ref().map(|p| p.held.clone())
    }
}

/// A PEM chain and its PEM PKCS#8 key as rustls serves them; Err when the key is not the leaf's.
fn certified(chain_pem: &[u8], key_pem: &[u8]) -> anyhow::Result<CertifiedKey> {
    let chain: Vec<CertificateDer<'static>> = pem::parse_many(chain_pem)?
        .into_iter()
        .filter(|p| p.tag() == "CERTIFICATE")
        .map(|p| CertificateDer::from(p.contents().to_vec()))
        .collect();
    if chain.is_empty() {
        anyhow::bail!("no certificate in the chain");
    }
    let key = pem::parse_many(key_pem)?
        .into_iter()
        .find(|p| p.tag() == "PRIVATE KEY")
        .ok_or_else(|| anyhow::anyhow!("no PKCS#8 private key"))?;
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.contents().to_vec()));
    let provider = rustls::crypto::ring::default_provider();
    let signing = provider.key_provider.load_private_key(key)?;
    let certified = CertifiedKey::new(chain, signing);
    certified.keys_match().map_err(|e| anyhow::anyhow!("the key is not the certificate's: {e}"))?;
    Ok(certified)
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// What `place-cert.json` says, when the certificate and its key are there too.
pub fn held(dir: &Path) -> Option<Held> {
    if !dir.join(CERT_NAME).exists() || !dir.join(KEY_NAME).exists() {
        return None;
    }
    let meta = read_json(&dir.join(META_NAME))?;
    let host = meta.get("host")?.as_str()?.to_string();
    let expires = meta.get("expires")?.as_i64()?;
    (!host.is_empty()).then_some(Held { host, expires })
}

fn load_placed(dir: &Path) -> anyhow::Result<Option<Placed>> {
    let Some(held) = held(dir) else { return Ok(None) };
    let key = certified(&std::fs::read(dir.join(CERT_NAME))?, &std::fs::read(dir.join(KEY_NAME))?)?;
    Ok(Some(Placed { held, key: Arc::new(key) }))
}

/// The certificate held in the folder of the door's certificates (`[lan] cert_dir`), when a place has one: what the
/// pairing and setup answers show the apps (`host`).
pub fn held_for(config: &crate::config::Config) -> Option<Held> {
    if !config.lan.enabled || config.lan.cert_dir.is_empty() {
        return None;
    }
    held(Path::new(&config.lan.cert_dir))
}

// -- asking ----------------------------------------------------------------------------------------------------------

/// A new P-256 key (PEM PKCS#8) and the CSR (PEM PKCS#10) naming exactly `host`: as its common name and as its one
/// DNS name (subjectAltName), which is what the guichet checks before passing it to Let's Encrypt.
pub fn new_request(host: &str) -> anyhow::Result<(String, String)> {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = rcgen::CertificateParams::new(vec![host.to_string()])?;
    let mut name = rcgen::DistinguishedName::new();
    name.push(rcgen::DnType::CommonName, host);
    params.distinguished_name = name;
    let csr = params.serialize_request(&key)?.pem()?;
    Ok((key.serialize_pem(), csr))
}

/// Writes next to `path` then renames over it: a reader sees the old file or the new one, never half of one.
fn write_atomic(path: &Path, data: &[u8], mode: u32) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&temp);
    crate::py::write_with_mode(&temp.to_string_lossy(), data, mode)?;
    std::fs::rename(&temp, path)
}

/// The order under way: its host and CSR (its key stays in its own file).
fn pending(dir: &Path) -> Option<(String, String)> {
    if !dir.join(PENDING_KEY_NAME).exists() {
        return None;
    }
    let data = read_json(&dir.join(PENDING_NAME))?;
    let host = data.get("host")?.as_str()?.to_string();
    let csr = data.get("csr")?.as_str()?.to_string();
    (!host.is_empty() && csr.contains("CERTIFICATE REQUEST")).then_some((host, csr))
}

fn forget_pending(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(PENDING_NAME));
    let _ = std::fs::remove_file(dir.join(PENDING_KEY_NAME));
}

/// What one try with the guichet came to.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// A new certificate is on disk and served.
    Issued(Held),
    /// The guichet is on it: ask again (same CSR) after this.
    Pending(Duration),
    /// Something failed (the guichet busy or unreachable, an answer that can't be used): try again after at least
    /// this (0 = no wait asked), with what went wrong.
    Failed(Duration, String),
    /// The guichet doesn't know this place (404 / 401): nothing more until the bridge starts again.
    Unknown(String),
}

fn seconds(data: &Value, field: &str) -> Duration {
    data.get(field)
        .and_then(Value::as_f64)
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| Duration::from_secs_f64(s.min(86_400.0)))
        .unwrap_or_default()
}

fn detail(data: &Value, status: u16) -> String {
    data.get("detail").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}"))
}

/// A refusal that isn't « pending » or « done » turned into a step.
fn refused(status: u16, data: &Value) -> Step {
    match status {
        401 | 404 => Step::Unknown(detail(data, status)),
        _ => Step::Failed(seconds(data, "retry_after"), detail(data, status)),
    }
}

async fn call(req: reqwest::RequestBuilder) -> Result<(u16, Value), Step> {
    let res = req
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|_| Step::Failed(Duration::ZERO, "the guichet doesn't answer".into()))?;
    let status = res.status().as_u16();
    Ok((status, res.json().await.unwrap_or(Value::Null)))
}

/// One try: the order under way asked again, or a new one (the host, a new key and CSR kept on disk first), then the
/// certificate stored and swapped in when the guichet gives it.
pub async fn step(http: &reqwest::Client, base: &str, token: &str, door: &DoorCerts) -> Step {
    let dir = door.dir();
    let (host, csr) = match pending(dir) {
        Some(order) => order,
        None => {
            let (status, data) = match call(http.get(format!("{base}/v1/places/host")).bearer_auth(token)).await {
                Ok(answer) => answer,
                Err(step) => return step,
            };
            if status != 200 {
                return refused(status, &data);
            }
            let Some(host) = data.get("host").and_then(Value::as_str).map(|h| h.trim().to_ascii_lowercase()) else {
                return Step::Failed(Duration::ZERO, "the guichet gave no host".into());
            };
            if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
                return Step::Failed(Duration::ZERO, "the guichet gave an odd host".into());
            }
            let made = new_request(&host).and_then(|(key, csr)| {
                write_atomic(&dir.join(PENDING_KEY_NAME), key.as_bytes(), 0o600)?;
                let order = json!({"host": host, "csr": csr});
                write_atomic(&dir.join(PENDING_NAME), order.to_string().as_bytes(), 0o600)?;
                Ok(csr)
            });
            match made {
                Ok(csr) => (host, csr),
                Err(e) => return Step::Failed(Duration::ZERO, format!("can't make the request: {e:#}")),
            }
        }
    };
    let (status, data) =
        match call(http.post(format!("{base}/v1/places/cert")).bearer_auth(token).json(&json!({"csr": csr}))).await {
            Ok(answer) => answer,
            Err(step) => return step,
        };
    match status {
        202 => Step::Pending(seconds(&data, "retry_after")),
        200 => store(dir, &host, &data, door),
        429 | 503 | 401 | 404 => refused(status, &data),
        400..=499 => {
            // the guichet won't take this CSR (a host that changed meanwhile…): the next try starts a new order
            forget_pending(dir);
            refused(status, &data)
        }
        _ => refused(status, &data),
    }
}

/// The guichet's certificate checked against the pending key, written beside it (temp + rename), swapped in.
fn store(dir: &Path, host: &str, data: &Value, door: &DoorCerts) -> Step {
    let given_host = data.get("host").and_then(Value::as_str).unwrap_or(host);
    if !given_host.eq_ignore_ascii_case(host) {
        forget_pending(dir);
        return Step::Failed(Duration::ZERO, format!("the guichet's certificate is for another host ({given_host})"));
    }
    let (Some(chain), Some(expires)) =
        (data.get("certificate").and_then(Value::as_str), data.get("expires").and_then(Value::as_i64))
    else {
        return Step::Failed(Duration::ZERO, "the guichet's answer has no certificate or expiry".into());
    };
    let Ok(key) = std::fs::read(dir.join(PENDING_KEY_NAME)) else {
        return Step::Failed(Duration::ZERO, "the pending key is gone".into());
    };
    if let Err(e) = certified(chain.as_bytes(), &key) {
        // not ours (or not a certificate): a new order next time rather than the same answer again
        forget_pending(dir);
        return Step::Failed(Duration::ZERO, format!("the guichet's certificate can't be used: {e:#}"));
    }
    let held = Held { host: host.to_string(), expires };
    let written = (|| -> std::io::Result<()> {
        write_atomic(&dir.join(CERT_NAME), chain.as_bytes(), 0o644)?;
        write_atomic(&dir.join(KEY_NAME), &key, 0o600)?;
        write_atomic(&dir.join(META_NAME), held.json().to_string().as_bytes(), 0o644)
    })();
    if let Err(e) = written {
        return Step::Failed(Duration::ZERO, format!("can't store the certificate: {e}"));
    }
    forget_pending(dir);
    match door.reload() {
        Some(now) if now == held => Step::Issued(held),
        _ => Step::Failed(Duration::ZERO, "the stored certificate doesn't load".into()),
    }
}

/// At start, then every [`Pace::check_every`]: a certificate asked for when none is held or it ends within 30 days,
/// the order followed until the guichet gives it. `guichet` says where the guichet is and this place's token (read
/// each time: the backup settings may arrive after the bridge started); None = a self-install, nothing is asked.
pub async fn run_forever<G>(
    http: reqwest::Client,
    door: Arc<DoorCerts>,
    guichet: G,
    pace: Pace,
    stop: CancellationToken,
) where
    G: Fn() -> Option<(String, String)>,
{
    let mut failures = 0u32;
    loop {
        let now = chrono::Utc::now().timestamp();
        let wait = if !needs_renewal(door.serving().as_ref(), now) && pending(door.dir()).is_none() {
            pace.check_every
        } else if let Some((base, token)) = guichet() {
            match step(&http, &base, &token, &door).await {
                Step::Issued(held) => {
                    failures = 0;
                    tracing::info!(host = %held.host, expires = held.expires, "place certificate issued");
                    pace.check_every
                }
                Step::Pending(after) => {
                    failures = 0;
                    after.clamp(pace.pending_min, pace.pending_max)
                }
                Step::Failed(after, why) => {
                    failures += 1;
                    let wait = backoff(&pace, failures, after);
                    tracing::warn!(detail = %why, retry_in = wait.as_secs(), "place certificate not obtained");
                    wait
                }
                Step::Unknown(why) => {
                    tracing::warn!(detail = %why, "place certificate: the guichet doesn't know this place, not asking again");
                    return;
                }
            }
        } else {
            // no guichet settings: a self-install (its door keeps its own certificate), or a server of the offer whose
            // first boot hasn't written them yet; looked at again within the hour (one small file read, nothing sent)
            pace.check_every.min(NO_GUICHET_RECHECK)
        };
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(wait) => {}
        }
    }
}

/// [`run_forever`] for the bridge: the backup settings' guichet and token (agent_email.rs `place_token`).
pub async fn run_for_place(door: Arc<DoorCerts>, stop: CancellationToken) {
    let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).build().unwrap_or_default();
    let guichet = || crate::agent_email::place_token(&crate::place_backup::home());
    run_forever(http, door, guichet, Pace::default(), stop).await;
}

#[cfg(test)]
mod tests;
