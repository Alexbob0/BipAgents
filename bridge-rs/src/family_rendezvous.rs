//! Two households linked wherever they are, through Kerfy Studio's relay (relay/ family.rs, the push relay's
//! rendezvous): two installs at home on different networks (neither can be reached), or a place of the offer that
//! scans a home's invitation. Both bridges connect out; nothing to open at home. What passes is sealed between the
//! two bridges (family_seal.rs): the relay sees which mailbox calls which, when, how many bytes, never what.
//!
//! - **A mailbox.** This bridge takes one at the relay when it needs it (an invitation, a link made through it): a
//!   random id the relay gives, and its token, kept in `family-relay.json` next to households.json (0600). The id
//!   travels in the invitation (`relay`) and in the link's exchange; the token never leaves this bridge and the relay.
//! - **The link** (peers.rs `join`): the scanning bridge first tries the inviter's address as always; if nothing
//!   answers there (or a place scans a home), it introduces itself through the relay, sealed with the invitation's
//!   code ([`accept`]). The two then hold a key of the link (`seal` in each entry) and say who reaches whom:
//!   - a home and a place: the home fetches the place's calls from it (family_relay.rs, as when the home scans), the
//!     relay is not used again;
//!   - otherwise, whoever can't reach the other calls it through the relay (`via: "relay"`, a loopback `route`), and
//!     whoever is called that way keeps a long poll on its mailbox (`relay_in`).
//! - **A call** to such a household goes, as every family call, to the address [`crate::family_relay::reach_url`]
//!   gives: this bridge's loopback listener, which seals it and sends it to their mailbox ([`call`]); the answer comes
//!   back streamed, opened frame by frame. The callers (peers.rs, family_news.rs, agent_shares.rs…) are unchanged.
//! - **Their calls to us**: [`poll_forever`] keeps one long poll while some household calls us this way (or an
//!   invitation waits); each call is opened with the key of the household its mailbox is, refused when it doesn't
//!   open, is stale or seen already, then run through this bridge's own routes as any household call (only
//!   `/family/…`, with that household's own key: family_relay.rs `allowed`), its answer sealed back.
//!
//! The relay down: those links fail as a household that doesn't answer (502, at once); everything else (direct links,
//! homes fetching from places, links made before) never depends on it.

use std::sync::OnceLock;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use serde_json::{json, Value};
use sha2::Digest as _;

use crate::app::AppState;
use crate::config::Config;
use crate::family_seal::{self, AnswerOpener, Broken, Seen, Stale, Way, SALT_LEN};
use crate::web::HttpError;
use crate::Dict;

/// A mailbox token is renewed when it has less than this left (the relay's last 400 days).
const RENEW_BEFORE: f64 = 200.0 * 86400.0;
/// The most a sealed request may weigh (the relay's own limit).
pub const REQUEST_BYTES: usize = 16 << 20;
/// How long a call waits for the other bridge to take it, then for its answer (the relay's `pickup` and
/// `answer_start`, plus a margin).
const CALL_TIMEOUT: Duration = Duration::from_secs(15 + 660 + 60);
/// One long poll (the relay's is 25 s).
const POLL_TIMEOUT: Duration = Duration::from_secs(45);

/// `[family]` of bridge.toml.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Settings {
    /// `relay_url`: Kerfy Studio's relay (`https://push.bipagents.com`, the push relay's address by default);
    /// `""` or `"off"`: no relay (two homes can't link then).
    pub relay_url: Option<String>,
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        Settings { relay_url: table.get("relay_url").and_then(Value::as_str).map(|u| u.trim().to_string()) }
    }
}

/// The relay this bridge uses for families, if any.
pub fn base(config: &Config) -> Option<String> {
    match config.family.relay_url.as_deref() {
        Some("") | Some("off") => None,
        Some(url) => Some(crate::py::rstrip_slash(url)),
        None => config.push_service_url.clone().filter(|u| !u.is_empty()),
    }
}

/// Given to a request run in-process for an invitation answered through the relay (peers.rs `family_accept`): an
/// extension no outside request can carry.
#[derive(Clone, Debug)]
pub struct ThroughRelay {
    /// The mailbox it came from.
    pub mailbox: String,
}

/// 22 base64url characters (the relay's mailbox ids).
pub fn valid_mailbox(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

// -- this bridge's mailbox and its invitations (family-relay.json) -------------------------------------------------

fn state_path(config: &Config) -> String {
    crate::py::join(&config.data_dir(), "family-relay.json")
}

fn load(config: &Config) -> Dict {
    std::fs::read_to_string(state_path(config))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn save(config: &Config, data: &Dict) -> anyhow::Result<()> {
    let path = state_path(config);
    if let Some(parent) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = format!("{path}.tmp");
    crate::py::write_with_mode(&tmp, crate::py::dumps(&Value::Object(data.clone())).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// One change at a time to family-relay.json.
static FILE: std::sync::Mutex<()> = std::sync::Mutex::new(());
/// One registration at a time.
static REGISTERING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn edit(config: &Config, change: impl FnOnce(&mut Dict)) -> anyhow::Result<()> {
    let _held = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = load(config);
    change(&mut data);
    save(config, &data)
}

/// This bridge's mailbox id, if it has one (no call to the relay).
pub fn own_mailbox(config: &Config) -> Option<String> {
    load(config).get("mailbox")?.get("id")?.as_str().filter(|id| valid_mailbox(id)).map(str::to_string)
}

/// This bridge's mailbox (id, token): the one kept, renewed when it ends within [`RENEW_BEFORE`], else a new one.
pub async fn mailbox(state: &AppState) -> anyhow::Result<(String, String)> {
    let config = state.config();
    let base = base(&config).ok_or_else(|| anyhow::anyhow!("no family relay set up"))?;
    let _one = REGISTERING.lock().await;
    let kept = load(&config).get("mailbox").and_then(Value::as_object).cloned().unwrap_or_default();
    let field = |k: &str| kept.get(k).and_then(Value::as_str).map(str::to_string);
    let expires = kept.get("expires").and_then(Value::as_f64).unwrap_or(0.0);
    let now = crate::py::time_time();
    if let (Some(id), Some(token)) = (field("id"), field("token")) {
        if expires - now > RENEW_BEFORE {
            return Ok((id, token));
        }
    }
    let old = field("token");
    let answer = state
        .http
        .post(format!("{base}/v1/family/mailbox"))
        .timeout(Duration::from_secs(10))
        .json(&json!({"token": old}))
        .send()
        .await;
    let fresh = match answer {
        Ok(resp) if resp.status().is_success() => resp.json::<Value>().await.ok(),
        Ok(resp) => {
            tracing::warn!(status = resp.status().as_u16(), "family relay: no mailbox given");
            None
        }
        Err(e) => {
            tracing::info!(error = %e, "family relay: not reached for a mailbox");
            None
        }
    };
    let Some(fresh) = fresh else {
        // the relay out of reach while renewing: the token kept still works until its end
        return match (field("id"), field("token")) {
            (Some(id), Some(token)) if expires > now => Ok((id, token)),
            _ => Err(anyhow::anyhow!("Kerfy Studio's relay can't be reached")),
        };
    };
    let id = fresh.get("mailbox").and_then(Value::as_str).unwrap_or_default().to_string();
    let token = fresh.get("token").and_then(Value::as_str).unwrap_or_default().to_string();
    if !valid_mailbox(&id) || token.is_empty() || field("id").is_some_and(|kept| kept != id) {
        anyhow::bail!("the relay gave no usable mailbox");
    }
    let expires = fresh.get("expires").and_then(Value::as_f64).unwrap_or(now + 400.0 * 86400.0);
    edit(&config, |data| {
        data.insert("mailbox".into(), json!({"id": id, "token": token, "expires": expires}));
    })?;
    Ok((id, token))
}

/// An invitation made with our mailbox in it: its exchange through the relay will be sealed with its code's key.
pub fn remember_invite(config: &Config, code: &str, until: f64) -> anyhow::Result<()> {
    let digest = hex::encode(sha2::Sha256::digest(code.as_bytes()));
    let key = family_seal::encode_key(&family_seal::invite_key(code));
    let now = crate::py::time_time();
    edit(config, |data| {
        let mut invites = data.get("invites").and_then(Value::as_object).cloned().unwrap_or_default();
        invites.retain(|_, v| v.get("until").and_then(Value::as_f64).is_some_and(|t| t > now));
        invites.insert(digest, json!({"key": key, "until": until}));
        data.insert("invites".into(), Value::Object(invites));
    })
}

/// The invitations still waiting (their digest, their exchange's key).
fn invite_keys(config: &Config) -> Vec<(String, [u8; 32])> {
    let now = crate::py::time_time();
    let data = load(config);
    let Some(invites) = data.get("invites").and_then(Value::as_object) else { return Vec::new() };
    invites
        .iter()
        .filter(|(_, v)| v.get("until").and_then(Value::as_f64).is_some_and(|t| t > now))
        .filter_map(|(d, v)| Some((d.clone(), family_seal::decode_key(v.get("key")?.as_str()?)?)))
        .collect()
}

fn forget_invite(config: &Config, digest: &str) {
    let _ = edit(config, |data| {
        if let Some(Value::Object(invites)) = data.get_mut("invites") {
            invites.shift_remove(digest);
        }
    });
}

// -- a link's fields in its households.json entry ---------------------------------------------------------------

/// What a household entry holds of its link through the relay.
pub struct Link {
    pub key: [u8; 32],
    /// Ours: `inviter` or `joiner`.
    pub role: String,
    /// Theirs, at the relay.
    pub mailbox: Option<String>,
}

impl Link {
    pub fn of(entry: &Dict) -> Option<Link> {
        let key = family_seal::decode_key(entry.get("seal")?.as_str()?)?;
        let role = entry.get("role")?.as_str()?.to_string();
        Way::out(&role)?;
        let mailbox = entry.get("mailbox").and_then(Value::as_str).filter(|m| valid_mailbox(m)).map(str::to_string);
        Some(Link { key, role, mailbox })
    }
}

/// The loopback token of a household we call through the relay.
pub fn route_of(entry: &Dict) -> Option<&str> {
    if entry.get("via").and_then(Value::as_str) != Some("relay") {
        return None;
    }
    entry.get("route").and_then(Value::as_str).filter(|t| t.len() >= 16)
}

/// The household whose loopback token this is (a call of ours to go through the relay).
pub fn household_of_route(state: &AppState, token: &str) -> Option<(String, Dict)> {
    let data = state.households.load();
    let households = data.get("households")?.as_object()?;
    households.iter().find_map(|(id, v)| {
        let entry = v.as_object()?;
        let route = route_of(entry)?;
        crate::auth::constant_time_eq(route.as_bytes(), token.as_bytes()).then(|| (id.clone(), entry.clone()))
    })
}

/// Whether this bridge must keep a long poll on its mailbox: a household calls us through the relay, or an
/// invitation with our mailbox waits.
fn needs_poll(state: &AppState) -> bool {
    let config = state.config();
    if base(&config).is_none() || own_mailbox(&config).is_none() {
        return false;
    }
    let data = state.households.load();
    let called = data.get("households").and_then(Value::as_object).is_some_and(|h| {
        h.values().any(|v| v.get("relay_in").and_then(Value::as_bool) == Some(true) && v.get("seal").is_some())
    });
    called || !invite_keys(&config).is_empty()
}

// -- calling a household through the relay ------------------------------------------------------------------------

fn failed(status: u16, code: &str, message: &str) -> Response {
    HttpError::json(status, json!({"code": code, "message": message})).into_response()
}

fn relay_down() -> Response {
    failed(502, "relay_unreachable", "Kerfy Studio's relay can't be reached right now")
}

fn not_answering() -> Response {
    failed(502, "their_side_unreachable", "their side does not answer right now")
}

/// Why a call through the relay got no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallFailure {
    /// The relay can't be reached (or gave us no mailbox).
    RelayDown,
    /// Their bridge is not connected to it, or did not take the call.
    Away,
    /// Their answer doesn't open (not theirs, changed, cut), or the relay refused the call.
    Broken,
}

/// One sealed call to `mailbox`, its answer's head opened: (status, headers, the rest of the answer as it comes).
async fn send(
    state: &AppState,
    mailbox: &str,
    kind: &str,
    key: &[u8; 32],
    way: Way,
    head: Value,
    body: &[u8],
) -> Result<(u16, Vec<(String, String)>, Body), CallFailure> {
    let config = state.config();
    let base = base(&config).ok_or(CallFailure::RelayDown)?;
    let (_, token) = mailbox_token(state).await?;
    let (wire, salt) = family_seal::seal_request(key, way, &head, body);
    let resp = state
        .http
        .post(format!("{base}/v1/family/call/{mailbox}"))
        .timeout(CALL_TIMEOUT)
        .bearer_auth(token)
        .header("x-bip-kind", kind)
        .header("content-type", "application/octet-stream")
        .body(wire)
        .send()
        .await
        .map_err(|_| CallFailure::RelayDown)?;
    match resp.status().as_u16() {
        200 => {}
        503 | 504 => return Err(CallFailure::Away),
        502 => return Err(CallFailure::RelayDown),
        status => {
            tracing::info!(status, "family relay: call refused by the relay");
            return Err(CallFailure::Broken);
        }
    }
    let mut opener = AnswerOpener::new(key, &salt, way);
    let mut stream = Box::pin(resp.bytes_stream());
    let (head, last) = loop {
        match opener.next_frame() {
            Ok(Some(frame)) => break frame,
            Ok(None) => {}
            Err(Broken) => return Err(CallFailure::Broken),
        }
        match stream.next().await {
            Some(Ok(chunk)) => opener.push(&chunk),
            _ => return Err(CallFailure::Broken), // ended (or cut) before its head: refused, or not theirs
        }
    };
    let head: Value = serde_json::from_slice(&head).map_err(|_| CallFailure::Broken)?;
    let status = head.get("s").and_then(Value::as_u64).and_then(|s| u16::try_from(s).ok()).unwrap_or(502);
    let headers: Vec<(String, String)> =
        head.get("h").cloned().and_then(|h| serde_json::from_value(h).ok()).unwrap_or_default();
    if last {
        return Ok((status, headers, Body::empty()));
    }
    // the rest, frame by frame; an error if a frame doesn't open or the stream ends before the last one
    let rest = futures::stream::unfold(Some((opener, stream)), |held| async move {
        let (mut opener, mut stream) = held?;
        loop {
            match opener.next_frame() {
                Ok(Some((data, last))) => {
                    let next = if last { None } else { Some((opener, stream)) };
                    return Some((Ok::<Bytes, std::io::Error>(Bytes::from(data)), next));
                }
                Ok(None) => {}
                Err(Broken) => return Some((Err(std::io::Error::other("a family answer does not open")), None)),
            }
            match stream.next().await {
                Some(Ok(chunk)) => opener.push(&chunk),
                _ => return Some((Err(std::io::Error::other("a family answer was cut")), None)),
            }
        }
    });
    Ok((status, headers, Body::from_stream(rest)))
}

async fn mailbox_token(state: &AppState) -> Result<(String, String), CallFailure> {
    mailbox(state).await.map_err(|_| CallFailure::RelayDown)
}

/// A call of ours to a household reached through the relay (the loopback listener, family_relay.rs `relay_in`):
/// sealed with the link's key, sent to their mailbox, their answer opened as it comes.
pub async fn call(
    state: AppState,
    household: String,
    entry: Dict,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Bytes,
) -> Response {
    let (Some(link), true) = (Link::of(&entry), base(&state.config()).is_some()) else {
        tracing::info!(household = %household, "family relay: no key or no relay for this link");
        return not_answering();
    };
    let Some(mailbox) = link.mailbox.as_deref() else { return not_answering() };
    let way = Way::out(&link.role).unwrap_or(Way::Invite);
    let head = json!({"m": method, "p": path, "h": headers, "t": crate::py::time_time()});
    match send(&state, mailbox, "call", &link.key, way, head, &body).await {
        Ok((status, headers, body)) => {
            let mut response = Response::new(body);
            *response.status_mut() =
                StatusCode::from_u16(status).ok().filter(|s| !s.is_informational()).unwrap_or(StatusCode::BAD_GATEWAY);
            crate::family_relay::put_headers(response.headers_mut(), &headers);
            response
        }
        Err(CallFailure::RelayDown) => {
            tracing::info!(household = %household, "family relay: the relay can't be reached");
            relay_down()
        }
        Err(CallFailure::Away) => not_answering(),
        Err(CallFailure::Broken) => {
            tracing::warn!(household = %household, "family relay: their answer does not open (changed, cut, or not theirs)");
            not_answering()
        }
    }
}

/// The scanning bridge's `POST /family/accept` through the relay, to the inviter's mailbox, sealed with the
/// invitation's code: (status, body). `current`: our key for them when we link again.
pub async fn accept(
    state: &AppState,
    mailbox: &str,
    code: &str,
    payload: &Value,
    current: Option<&str>,
) -> Result<(u16, Bytes), CallFailure> {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(current) = current {
        headers.push(("authorization".into(), format!("Bearer {current}")));
    }
    let key = family_seal::invite_key(code);
    // the inviter's bridge may be (re)connecting its poll a moment after showing the invitation: a few tries
    let mut tries = 0;
    let (status, body) = loop {
        let head = json!({"m": "POST", "p": "/family/accept", "h": headers, "t": crate::py::time_time()});
        match send(state, mailbox, "invite", &key, Way::Invite, head, payload.to_string().as_bytes()).await {
            Err(CallFailure::Away) if tries < 4 => {
                tries += 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Err(failure) => return Err(failure),
            Ok((status, _, body)) => break (status, body),
        }
    };
    let bytes = axum::body::to_bytes(body, 1 << 20).await.map_err(|_| CallFailure::Broken)?;
    Ok((status, bytes))
}

// -- their calls to us ------------------------------------------------------------------------------------------

/// Rings when this bridge may have to poll now (an invitation made, a link through the relay): no wait for the next
/// look.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub fn wake() {
    WAKE.notify_one();
}

/// What this bridge saw (family_seal.rs `Seen`), made once.
fn seen(config: &Config) -> &'static Seen {
    static SEEN: OnceLock<Seen> = OnceLock::new();
    SEEN.get_or_init(|| Seen::new(&config.data_dir()))
}

/// Keeps one long poll on this bridge's mailbox while a household calls us through the relay or an invitation waits.
pub async fn poll_forever(state: AppState) {
    let mut pause = Duration::from_secs(1);
    loop {
        if !needs_poll(&state) {
            let _ = tokio::time::timeout(Duration::from_secs(3), WAKE.notified()).await;
            continue;
        }
        let config = state.config();
        let (Some(base), Ok((_, token))) = (base(&config), mailbox(&state).await) else {
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(15));
            continue;
        };
        let answer =
            state.http.get(format!("{base}/v1/family/next")).timeout(POLL_TIMEOUT).bearer_auth(&token).send().await;
        match answer {
            Ok(resp) if resp.status() == StatusCode::OK => {
                pause = Duration::from_secs(1);
                let header =
                    |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                let (id, from, kind) = (header("x-bip-call"), header("x-bip-from"), header("x-bip-kind"));
                if let Ok(wire) = resp.bytes().await {
                    tokio::spawn(handle(state.clone(), base, token, id, from, kind, wire));
                }
            }
            Ok(resp) if resp.status() == StatusCode::NO_CONTENT => pause = Duration::from_secs(1),
            Ok(resp) => {
                tracing::warn!(status = resp.status().as_u16(), "family relay: poll refused");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(15));
            }
        }
    }
}

/// A call that opened: whose, and how its answer is sealed.
struct Opened {
    /// The household calling (None: an invitation's exchange).
    household: Option<String>,
    invite: Option<String>,
    key: [u8; 32],
    way: Way,
    salt: [u8; SALT_LEN],
    head: Value,
    body: Vec<u8>,
}

/// A link call from `from`: opened with the key of a household at that mailbox, in the way it calls us.
fn open_call(state: &AppState, from: &str, wire: &[u8]) -> Option<Opened> {
    let data = state.households.load();
    let households = data.get("households")?.as_object()?;
    households.iter().find_map(|(id, v)| {
        let link = Link::of(v.as_object()?)?;
        if link.mailbox.as_deref() != Some(from) {
            return None;
        }
        let way = Way::incoming(&link.role)?;
        let (salt, head, body) = family_seal::open_request(&link.key, way, wire).ok()?;
        Some(Opened { household: Some(id.clone()), invite: None, key: link.key, way, salt, head, body })
    })
}

/// An invitation's exchange: opened with the key of one of ours still waiting.
fn open_invite(config: &Config, wire: &[u8]) -> Option<Opened> {
    invite_keys(config).into_iter().find_map(|(digest, key)| {
        let (salt, head, body) = family_seal::open_request(&key, Way::Invite, wire).ok()?;
        Some(Opened { household: None, invite: Some(digest), key, way: Way::Invite, salt, head, body })
    })
}

async fn handle(state: AppState, base: String, token: String, id: String, from: String, kind: String, wire: Bytes) {
    let config = state.config();
    let opened = if kind == "invite" { open_invite(&config, &wire) } else { open_call(&state, &from, &wire) };
    let refused = |reason: &str| {
        tracing::warn!(reason, kind = %kind, "family relay: call refused");
    };
    let response = match opened {
        None => {
            refused("unsealed");
            None
        }
        Some(opened) => {
            let t = opened.head.get("t").and_then(Value::as_f64).unwrap_or(f64::NAN);
            match seen(&config).take(&opened.salt, t, crate::py::time_time()) {
                Err(Stale::Replay) => {
                    refused("replay");
                    None
                }
                Err(Stale::Time) => {
                    refused("stale");
                    None
                }
                Ok(()) => Some(run(&state, opened, &from).await),
            }
        }
    };
    // a refusal is an empty answer: the caller fails at once instead of waiting for the relay's end
    let body = match response {
        None => reqwest::Body::from(Vec::new()),
        Some((key, way, salt, response)) => {
            let status = response.status().as_u16();
            let headers = crate::family_relay::header_pairs(response.headers());
            let mut framer = family_seal::answer_sealer(&key, &salt, way);
            let first = framer.seal(json!({"s": status, "h": headers}).to_string().as_bytes(), false);
            let frames = futures::stream::unfold(
                (Some(framer), response.into_body().into_data_stream(), Some(first)),
                |(framer, mut data, first)| async move {
                    if let Some(first) = first {
                        return Some((Ok::<Bytes, std::io::Error>(Bytes::from(first)), (framer, data, None)));
                    }
                    let mut framer = framer?;
                    match data.next().await {
                        Some(Ok(chunk)) => {
                            let sealed = framer.seal_all(&chunk, false);
                            Some((Ok(Bytes::from(sealed)), (Some(framer), data, None)))
                        }
                        // the end: the last frame marked (a broken body ends without it: cut, as the caller sees)
                        None => Some((Ok(Bytes::from(framer.seal(&[], true))), (None, data, None))),
                        Some(Err(e)) => Some((Err(std::io::Error::other(e)), (None, data, None))),
                    }
                },
            );
            reqwest::Body::wrap_stream(frames)
        }
    };
    let sent = state
        .http
        .post(format!("{base}/v1/family/answer/{id}"))
        .timeout(CALL_TIMEOUT)
        .bearer_auth(token)
        .header("content-type", "application/octet-stream")
        .body(body)
        .send()
        .await;
    if let Err(e) = sent {
        tracing::info!(error = %e, "family relay: answer not sent");
    }
}

/// An opened call run through this bridge's own routes: (key, way, salt, its response).
async fn run(state: &AppState, opened: Opened, from: &str) -> ([u8; 32], Way, [u8; SALT_LEN], Response) {
    let method = opened.head.get("m").and_then(Value::as_str).unwrap_or("GET").to_string();
    let path = opened.head.get("p").and_then(Value::as_str).unwrap_or("/").to_string();
    let pairs: Vec<(String, String)> =
        opened.head.get("h").cloned().and_then(|h| serde_json::from_value(h).ok()).unwrap_or_default();
    let response = match (&opened.household, &opened.invite) {
        (Some(household), _) => {
            crate::family_relay::run_in_process(state, household, &method, &path, &pairs, Bytes::from(opened.body))
                .await
        }
        (None, Some(digest)) if method == "POST" && path == "/family/accept" => {
            let through = ThroughRelay { mailbox: from.to_string() };
            let response = crate::family_relay::run_accept(&pairs, Bytes::from(opened.body), through).await;
            if response.status().is_success() {
                forget_invite(&state.config(), digest); // used: its key goes with it
            }
            response
        }
        _ => HttpError::not_found().into_response(),
    };
    (opened.key, opened.way, opened.salt, response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_relay_is_the_push_relay_unless_set_or_off() {
        let mut config = Config::with_key("k".repeat(40));
        assert_eq!(base(&config), None);
        config.push_service_url = Some("https://push.bipagents.com".into());
        assert_eq!(base(&config).as_deref(), Some("https://push.bipagents.com"));
        config.family = Settings::from_table(&json!({"relay_url": "http://127.0.0.1:9000/"}));
        assert_eq!(base(&config).as_deref(), Some("http://127.0.0.1:9000"));
        config.family = Settings::from_table(&json!({"relay_url": "off"}));
        assert_eq!(base(&config), None);
    }

    #[test]
    fn a_link_through_the_relay_reads_from_its_entry() {
        let key = family_seal::encode_key(&[9u8; 32]);
        let mut entry = Dict::new();
        entry.insert("url".into(), json!("http://lea.home.arpa:8642"));
        assert!(Link::of(&entry).is_none() && route_of(&entry).is_none());
        entry.insert("seal".into(), json!(key));
        entry.insert("role".into(), json!("joiner"));
        entry.insert("mailbox".into(), json!("AAAAAAAAAAAAAAAAAAAAAA"));
        let link = Link::of(&entry).unwrap();
        assert_eq!(
            (link.key, link.role.as_str(), link.mailbox.as_deref()),
            ([9u8; 32], "joiner", Some("AAAAAAAAAAAAAAAAAAAAAA"))
        );
        entry.insert("role".into(), json!("someone"));
        assert!(Link::of(&entry).is_none());
        entry.insert("route".into(), json!("r".repeat(32)));
        assert!(route_of(&entry).is_none(), "a route only with via = relay");
        entry.insert("via".into(), json!("relay"));
        assert_eq!(route_of(&entry), Some("r".repeat(32).as_str()));
        assert!(valid_mailbox("AAAAAAAAAAAAAAAAAAAAAA") && !valid_mailbox("AAA") && !valid_mailbox(&"/".repeat(22)));
    }
}
