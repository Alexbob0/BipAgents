//! The relay's routes: registering a device, relaying a push, and passing on « Report this reply » to Kerfy Studio
//! (kept nowhere here; the phone's address only counted, hashed in memory, against abuse).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::apns::Apns;
use crate::config::Config;
use crate::fcm::Fcm;
use crate::limits::Quota;
use crate::payload::{scrub, scrub_live};
use crate::seal::{now, Audience, Claims, Closed, Device, Platform, Sealer, PREFIX, RETIRED_PREFIX};

pub struct Relay {
    pub sealer: Sealer,
    pub apns: Option<Apns>,
    pub fcm: Option<Fcm>,
    per_device: Quota,
    per_ip_push: Quota,
    per_ip_register: Quota,
    per_ip_report: Quota,
    report_url: Option<String>,
    report_secret: Option<String>,
    http: reqwest::Client,
    trust_forwarded_for: bool,
    blocked: std::collections::HashSet<String>,
    /// SHA-256 of the guichet's keys: compared as digests, so the comparison tells nothing of the key.
    guichet_keys: Vec<[u8; 32]>,
    /// The transition from the first token format (see seal.rs): its end, and what still used it since the start.
    r1_until: u64,
    r1_used: AtomicU64,
    unkeyed_guichet: AtomicU64,
}

impl Relay {
    pub fn new(config: Config, http: reqwest::Client) -> Result<Self, String> {
        Ok(Self {
            sealer: Sealer::from_base64_list(&config.seal_keys)?,
            apns: config.apns.map(|c| Apns::new(c, http.clone())).transpose()?,
            fcm: config.fcm.map(|a| Fcm::new(a, http.clone(), config.fcm_send_base.clone())).transpose()?,
            per_device: Quota::new(config.pushes_per_device_hour),
            per_ip_push: Quota::new(config.pushes_per_ip_hour),
            per_ip_register: Quota::new(config.registrations_per_ip_hour),
            per_ip_report: Quota::new(config.reports_per_ip_hour),
            report_url: config.report_url,
            report_secret: config.report_secret,
            http,
            trust_forwarded_for: config.trust_forwarded_for,
            blocked: config.blocked.into_iter().collect(),
            guichet_keys: config.guichet_keys.iter().map(|k| Sha256::digest(k.as_bytes()).into()).collect(),
            r1_until: config.r1_until,
            r1_used: AtomicU64::new(0),
            unkeyed_guichet: AtomicU64::new(0),
        })
    }

    /// Whether the first token format is still taken (and sealed for older apps).
    fn in_transition(&self, now: u64) -> bool {
        now < self.r1_until
    }

    /// Whether this push comes from the guichet: `Authorization: Bearer <one of its keys>`.
    fn sent_by_guichet(&self, headers: &HeaderMap) -> bool {
        let presented = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
            .map(|k| <[u8; 32]>::from(Sha256::digest(k.trim().as_bytes())));
        presented.is_some_and(|p| self.guichet_keys.contains(&p))
    }

    /// Every minute: the quotas forget what is about to be an hour old.
    pub fn sweep(&self) {
        self.per_device.sweep();
        self.per_ip_push.sweep();
        self.per_ip_register.sweep();
        self.per_ip_report.sweep();
    }

    fn is_blocked(&self, token: &str) -> bool {
        !self.blocked.is_empty() && self.blocked.contains(&fingerprint(token))
    }

    fn client_ip(&self, peer: SocketAddr, headers: &HeaderMap) -> String {
        if self.trust_forwarded_for {
            if let Some(first) = headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(',').next())
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                return first.to_string();
            }
        }
        peer.ip().to_string()
    }
}

pub fn router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/register", post(register))
        .route("/v1/relay/push", post(push))
        .route("/v1/push", post(push))
        .route("/v1/report", post(report))
        .layer(DefaultBodyLimit::max(32 * 1024))
        .with_state(relay)
}

fn refuse(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail": detail}))).into_response()
}

/// A short fingerprint for the logs: never the token itself.
fn fingerprint(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// `YYYY-MM-DD` of Unix seconds (UTC).
fn day_of(secs: u64) -> String {
    // civil from days (H. Hinnant)
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

async fn health(State(relay): State<Arc<Relay>>) -> Json<Value> {
    let mut health = json!({"status": "ok", "apns": relay.apns.is_some(), "fcm": relay.fcm.is_some()});
    if relay.in_transition(now()) {
        let (r1, unkeyed) = (relay.r1_used.load(Ordering::Relaxed), relay.unkeyed_guichet.load(Ordering::Relaxed));
        health["r1_until"] = json!(day_of(relay.r1_until));
        health["r1_used"] = json!(r1);
        health["guichet_without_key"] = json!(unkeyed);
        health["warning"] = json!(format!(
            "first-format relay tokens (r1) are still taken until {} ({r1} uses since start), and guichet tokens \
             without the guichet's key ({unkeyed}): update the apps, bridges and guichet before then",
            day_of(relay.r1_until)
        ));
    }
    Json(health)
}

#[derive(Deserialize)]
struct Register {
    platform: Platform,
    token: String,
    #[serde(default)]
    environment: Option<String>,
    /// Who will send with it: `bridge` (the default) or `guichet`. Not said: an app from before 2026-10-28, which
    /// during the transition gets a token of the first format (its bridges take nothing else).
    #[serde(default)]
    audience: Option<Audience>,
}

fn apple_token(raw: &str) -> Option<String> {
    let token: String = raw.chars().filter(|c| !matches!(c, ' ' | '<' | '>')).collect::<String>().to_lowercase();
    (32..=512).contains(&token.len()).then_some(()).filter(|_| token.chars().all(|c| c.is_ascii_hexdigit()))?;
    Some(token)
}

fn google_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    let valid = (20..=4096).contains(&token.len())
        && token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-'));
    valid.then(|| token.to_string())
}

async fn register(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Register>,
) -> Response {
    if !relay.per_ip_register.allow(relay.client_ip(peer, &headers).as_bytes()) {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "too many registrations");
    }
    let device = match body.platform {
        Platform::Apns => {
            if relay.apns.is_none() {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Apple notifications are not set up on this relay");
            }
            let environment = body.environment.unwrap_or_else(|| "production".into());
            let Some(token) =
                apple_token(&body.token).filter(|_| matches!(environment.as_str(), "sandbox" | "production"))
            else {
                return refuse(StatusCode::BAD_REQUEST, "invalid Apple token or environment");
            };
            Device { platform: Platform::Apns, environment, token }
        }
        Platform::Fcm => {
            if relay.fcm.is_none() {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Android notifications are not set up on this relay");
            }
            let Some(token) = google_token(&body.token) else {
                return refuse(StatusCode::BAD_REQUEST, "invalid Firebase token");
            };
            Device { platform: Platform::Fcm, environment: "fcm".into(), token }
        }
    };
    if relay.is_blocked(&device.token) {
        return refuse(StatusCode::FORBIDDEN, "this device is blocked on the relay: write to support@bipagents.com");
    }
    tracing::info!(platform = ?device.platform, device = %fingerprint(&device.token), audience = ?body.audience,
                   "device registered");
    let issued = now();
    let transition = relay.in_transition(issued);
    if body.audience.is_none() && transition {
        relay.r1_used.fetch_add(1, Ordering::Relaxed);
        tracing::warn!(device = %fingerprint(&device.token), until = %day_of(relay.r1_until),
                       "older app: first-format (r1) token sealed");
        return Json(json!({"relay_token": relay.sealer.seal_retired(&device)})).into_response();
    }
    let audience = body.audience.unwrap_or_default();
    let mut answer = json!({"relay_token": relay.sealer.seal(&device, audience, issued),
                            "expires": issued + crate::seal::LIFETIME_SECS});
    if transition && audience == Audience::Bridge {
        // For a bridge from before 2026-10-28, which refuses r2: the app falls back to this one there.
        answer["legacy_token"] = json!(relay.sealer.seal_retired(&device));
        answer["legacy_until"] = json!(relay.r1_until);
    }
    Json(answer).into_response()
}

/// A 16-hex id for a token of the first format (it holds none): what the logs show and `RELAY_BLOCKED` may name.
fn retired_id(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

fn alert() -> String {
    "alert".into()
}

fn high() -> u8 {
    10
}

#[derive(Deserialize)]
struct Push {
    token: String,
    payload: Value,
    #[serde(default = "alert")]
    push_type: String,
    #[serde(default = "high")]
    priority: u8,
    #[serde(default)]
    collapse_id: Option<String>,
}

async fn push(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Push>,
) -> Response {
    if !body.token.starts_with(PREFIX) && !body.token.starts_with(RETIRED_PREFIX) {
        // A device token sent as such (a bridge set up for direct delivery): not a dead token, just the wrong relay.
        return refuse(StatusCode::BAD_REQUEST, "not a relay token: the app registers with /v1/register first");
    }
    // A dead token reads as Apple's 410 (as the bridges and the guichet read it): the sender forgets it and the app
    // registers again.
    let gone = |reason: &str| Json(json!({"status": 410, "reason": reason, "apns_id": null})).into_response();
    let at = now();
    let transition = relay.in_transition(at);
    // During the transition a first-format token still works as it did: any sender, the guichet's texts too (the
    // guichet holds such tokens), logged each time.
    let mut retired = false;
    let claims = if transition && body.token.starts_with(RETIRED_PREFIX) {
        let Some(device) = relay.sealer.open_retired(&body.token) else { return gone("BadRelayToken") };
        retired = true;
        relay.r1_used.fetch_add(1, Ordering::Relaxed);
        let id = retired_id(&body.token);
        tracing::warn!(token = %id, until = %day_of(relay.r1_until), "first-format (r1) token used");
        Claims { device, id, aud: Audience::Guichet, iat: 0, exp: relay.r1_until }
    } else {
        match relay.sealer.open(&body.token, at) {
            Ok(claims) => claims,
            // Sealed by a key no longer in use, or altered.
            Err(Closed::Invalid) => return gone("BadRelayToken"),
            // Ended (90 days), or of the first format (no end) after the transition.
            Err(Closed::Expired | Closed::Retired) => return gone("ExpiredRelayToken"),
        }
    };
    if !retired && claims.aud == Audience::Guichet && !relay.sent_by_guichet(&headers) {
        if transition && headers.get("authorization").is_none() {
            // A guichet from before 2026-10-28 (no key yet): let through until the transition ends, logged.
            relay.unkeyed_guichet.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(token = %claims.id, until = %day_of(relay.r1_until), "guichet token sent without the key");
        } else {
            // A token the app gave the guichet, sent by anyone else (or a relay without the guichet's key).
            tracing::info!(token = %claims.id, "push refused: not the guichet");
            return refuse(StatusCode::UNAUTHORIZED, "this token is for the guichet");
        }
    }
    let device = claims.device;
    // `liveactivity`: a Live Activity's update (an agent's long work on the iPhone's lock screen), Apple only
    let live = body.push_type == "liveactivity";
    if !matches!(body.push_type.as_str(), "alert" | "background" | "liveactivity")
        || (live && !matches!(device.platform, Platform::Apns))
        || !matches!(body.priority, 5 | 10)
        || body.collapse_id.as_ref().is_some_and(|c| c.len() > 64)
    {
        return refuse(StatusCode::BAD_REQUEST, "invalid push");
    }
    if relay.is_blocked(&device.token) || relay.blocked.contains(&claims.id) {
        tracing::info!(device = %fingerprint(&device.token), token = %claims.id, "push refused: device blocked");
        return refuse(StatusCode::FORBIDDEN, "this device is blocked on the relay: write to support@bipagents.com");
    }
    if !relay.per_ip_push.allow(relay.client_ip(peer, &headers).as_bytes())
        || !relay.per_device.allow(device.token.as_bytes())
    {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "quota exceeded");
    }
    let payload = match if live { scrub_live(&body.payload) } else { scrub(&body.payload, claims.aud) } {
        Ok(payload) => payload,
        Err("payload too large") => return refuse(StatusCode::PAYLOAD_TOO_LARGE, "payload too large"),
        Err(reason) => return refuse(StatusCode::BAD_REQUEST, reason),
    };
    let collapse = body.collapse_id.as_deref();
    let outcome = match device.platform {
        Platform::Apns => match &relay.apns {
            Some(apns) => {
                apns.send(&device.token, &device.environment, &payload, &body.push_type, body.priority, collapse).await
            }
            None => return refuse(StatusCode::SERVICE_UNAVAILABLE, "Apple notifications are not set up on this relay"),
        },
        Platform::Fcm => match &relay.fcm {
            Some(fcm) => fcm.send(&device.token, &payload, body.priority, collapse).await,
            None => {
                return refuse(StatusCode::SERVICE_UNAVAILABLE, "Android notifications are not set up on this relay")
            }
        },
    };
    tracing::info!(platform = ?device.platform, device = %fingerprint(&device.token), token = %claims.id, status = outcome.status,
                   reason = outcome.reason.as_deref().unwrap_or(""), "push relayed");
    Json(json!({"status": outcome.status, "reason": outcome.reason, "apns_id": outcome.id})).into_response()
}

/// « Report this reply » from the app: the reason, the person's words, the reply's text, the agent's kind, the app's
/// version and the phone's model; never an address or a name. Passed on to Kerfy Studio as is, kept nowhere here.
#[derive(Deserialize, serde::Serialize)]
struct Report {
    reason: String,
    #[serde(default)]
    comment: String,
    text: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    app: String,
    #[serde(default)]
    model: String,
    platform: String,
    #[serde(default)]
    lang: String,
}

impl Report {
    fn valid(&self) -> bool {
        let short = |v: &str, max: usize| v.chars().count() <= max;
        !self.reason.trim().is_empty()
            && !self.text.trim().is_empty()
            && short(&self.reason, 60)
            && short(&self.comment, 2000)
            && short(&self.text, 8000)
            && short(&self.category, 30)
            && short(&self.app, 40)
            && short(&self.model, 60)
            && matches!(self.platform.as_str(), "ios" | "android")
            && short(&self.lang, 8)
    }
}

async fn report(
    State(relay): State<Arc<Relay>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Report>,
) -> Response {
    if !body.valid() {
        return refuse(StatusCode::BAD_REQUEST, "invalid report");
    }
    let (Some(url), Some(secret)) = (&relay.report_url, &relay.report_secret) else {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "reports are not set up on this relay: the app sends an e-mail",
        );
    };
    if !relay.per_ip_report.allow(relay.client_ip(peer, &headers).as_bytes()) {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "quota exceeded");
    }
    let sent = relay.http.post(url).header("x-relay-secret", secret).json(&body).send().await;
    match sent {
        Ok(resp) if resp.status().is_success() => {
            tracing::info!(platform = %body.platform, "report passed on");
            (StatusCode::ACCEPTED, Json(json!({"ok": true}))).into_response()
        }
        Ok(resp) => {
            tracing::warn!(status = resp.status().as_u16(), "report refused downstream");
            refuse(StatusCode::BAD_GATEWAY, "the report could not be passed on: the app sends an e-mail")
        }
        Err(_) => refuse(StatusCode::BAD_GATEWAY, "the report could not be passed on: the app sends an e-mail"),
    }
}
