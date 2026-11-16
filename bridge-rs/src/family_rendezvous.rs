//! Two households linked wherever they are, through Kerfy Studio's relay (relay/ family.rs, the push relay's
//! rendezvous): two installs at home on different networks (neither can be reached), or a place of the offer that
//! scans a home's invitation. Both bridges connect out; nothing to open at home. What passes is sealed between the
//! two bridges (family_seal.rs): the relay sees which mailbox calls which, when, how many bytes, never what.
//!
//! - **A mailbox.** This bridge takes one at the relay when a link needs it: a random id the relay gives, and its
//!   token, kept in `family-relay.json` next to households.json (0600). The token never leaves this bridge and the
//!   relay; the id goes only to the households linked through the relay.
//! - **Passes.** Only the households it linked with may call that mailbox: at the link, each bridge asks the relay for
//!   a pass to its own mailbox, for the other household's mailbox only ([`new_pass`]), and gives it to the other,
//!   sealed in their exchange (the joiner's goes once it knows the inviter's mailbox: peers.rs `give_pass`); the other
//!   presents it with each call (`pass` in its entry). The bridge says which of the passes it gave it still accepts
//!   (`given_pass`: with each poll, and at once when a link is removed, [`announce_passes`]); the relay keeps the
//!   oldest generation among them (a pass's id carries when it was made). Removing a link, the bridge first gives the
//!   households it keeps passes of a newer generation (peers.rs `unlink`), so the one withdrawn, older, is refused
//!   whatever the relay remembers: a household unlinked, or whoever holds a copy of a pass, can't call it, use up its
//!   quotas, or tell whether the home's computer is on. Every 30 days each household gets a fresh pass (peers.rs
//!   `renew_passes`): passes sealed with a key the relay retires keep working.
//! - **An invitation** has a mailbox of its own, ending with it ([`invite_mailbox`]): the QR code carries its id
//!   (`relay`), never the household's. The scanning bridge introduces itself there, sealed with the invitation's code
//!   ([`accept`]) — or first at the inviter's address, sealed the same way (peers.rs `join`): only a bridge holding the
//!   invitation can open that and answer it, so nothing that happens to answer at that address on the scanner's
//!   network learns the code or takes the inviter's place.
//! - **The link** then holds a key of its own (`seal` in each entry) and says who reaches whom:
//!   - a home and a place: the home fetches the place's calls from it (family_relay.rs, as when the home scans), the
//!     relay is not used again;
//!   - otherwise, whoever can't reach the other calls it through the relay (`via: "relay"`, a loopback `route`), and
//!     whoever is called that way keeps a long poll on its mailbox (`relay_in`).
//! - **A call** to such a household goes, as every family call, to the address [`crate::family_relay::reach_url`]
//!   gives: this bridge's loopback listener, which seals it and sends it to their mailbox ([`call`]); the answer comes
//!   back streamed, opened frame by frame. The callers (peers.rs, family_news.rs, agent_shares.rs…) are unchanged.
//! - **Their calls to us**: [`poll_forever`] keeps one long poll per mailbox needed (ours while a household calls us
//!   this way, each invitation's while it waits); each call is read up to [`REQUEST_BYTES`], opened with the key of the
//!   household its mailbox is (or the invitation's), refused when it doesn't open, is stale, seen already or can't be
//!   written down as seen, then run through this bridge's own routes as any household call (only `/family/…`, with that
//!   household's own key: family_relay.rs `allowed`), its answer sealed back. [`HANDLING`] calls at a time at most.
//!
//! The relay down: those links fail as a household that doesn't answer (502, at once); everything else (direct links,
//! homes fetching from places, links made before) never depends on it.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, OnceLock};
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
/// An invitation's exchange, sealed (the relay's limit for it).
pub const INVITE_BYTES: usize = 64 * 1024;
/// How long a call waits for the other bridge to take it, then for its answer (the relay's `pickup` and
/// `answer_start`, plus a margin).
const CALL_TIMEOUT: Duration = Duration::from_secs(15 + 660 + 60);
/// One long poll (the relay's is 25 s).
const POLL_TIMEOUT: Duration = Duration::from_secs(45);
/// Calls handled at once, every mailbox together: the next poll waits for one to end.
pub const HANDLING: usize = 16;

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

/// Given to an invitation's `POST /family/accept` that came sealed with its code (peers.rs `family_accept`), through
/// the relay or straight to this bridge: an extension no outside request can carry.
#[derive(Clone, Debug)]
pub struct Introduced {
    /// Through the relay (the caller's mailbox, as the relay says), or straight to our address (None).
    pub relay: Option<String>,
}

/// 22 base64url characters (the relay's mailbox ids).
pub fn valid_mailbox(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A call's id as the relay gives it (base64url, short): anything else is not put in an address.
fn valid_call(id: &str) -> bool {
    (8..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
static FILE: Mutex<()> = Mutex::new(());
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

/// `POST /v1/family/mailbox` with this body: (id, token, expires).
async fn register(state: &AppState, body: Value) -> anyhow::Result<(String, String, f64)> {
    let base = base(&state.config()).ok_or_else(|| anyhow::anyhow!("no family relay set up"))?;
    let resp = state.http.post(format!("{base}/v1/family/mailbox")).timeout(Duration::from_secs(10)).json(&body).send();
    let resp = resp.await.map_err(|e| anyhow::anyhow!("Kerfy Studio's relay can't be reached: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("the relay gave no mailbox ({})", resp.status().as_u16());
    }
    let fresh: Value = resp.json().await?;
    let id = fresh.get("mailbox").and_then(Value::as_str).unwrap_or_default().to_string();
    let token = fresh.get("token").and_then(Value::as_str).unwrap_or_default().to_string();
    if !valid_mailbox(&id) || token.is_empty() {
        anyhow::bail!("the relay gave no usable mailbox");
    }
    let expires = fresh.get("expires").and_then(Value::as_f64).unwrap_or(0.0);
    Ok((id, token, expires))
}

/// This bridge's mailbox (id, token): the one kept, renewed when it ends within [`RENEW_BEFORE`], else a new one.
pub async fn mailbox(state: &AppState) -> anyhow::Result<(String, String)> {
    let config = state.config();
    base(&config).ok_or_else(|| anyhow::anyhow!("no family relay set up"))?;
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
    let fresh = match register(state, json!({"token": field("token")})).await {
        Ok(fresh) => fresh,
        Err(e) => {
            tracing::info!(error = %e, "family relay: no mailbox (renewed or new)");
            // the relay out of reach while renewing: the token kept still works until its end
            return match (field("id"), field("token")) {
                (Some(id), Some(token)) if expires > now => Ok((id, token)),
                _ => Err(e),
            };
        }
    };
    let (id, token, expires) = fresh;
    if field("id").is_some_and(|kept| kept != id) {
        anyhow::bail!("the relay renewed another mailbox");
    }
    let expires = if expires > 0.0 { expires } else { now + 400.0 * 86400.0 };
    edit(&config, |data| {
        data.insert("mailbox".into(), json!({"id": id, "token": token, "expires": expires}));
    })?;
    Ok((id, token))
}

/// An invitation's own mailbox at the relay, until `until` (its id goes in the QR code).
pub async fn invite_mailbox(state: &AppState, until: f64) -> anyhow::Result<(String, String)> {
    let (id, token, _) = register(state, json!({"invite_until": until.ceil() as u64})).await?;
    Ok((id, token))
}

/// A pass to call this bridge's mailbox, for the household whose mailbox is `holder` (the relay seals it, and it
/// works for that mailbox only): (pass, its id, its end). Its id goes in what this bridge says it accepts
/// ([`announce_passes`]), until the link is removed.
pub async fn new_pass(state: &AppState, holder: &str) -> anyhow::Result<(String, String, f64)> {
    let base = base(&state.config()).ok_or_else(|| anyhow::anyhow!("no family relay set up"))?;
    let (_, token) = mailbox(state).await?;
    let config = state.config();
    let after = withdrawn(&config);
    let resp = state.http.post(format!("{base}/v1/family/pass")).timeout(Duration::from_secs(10)).bearer_auth(token);
    let resp = resp.json(&json!({"for": holder, "after": after})).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("the relay gave no pass ({})", resp.status().as_u16());
    }
    let pass: Value = resp.json().await?;
    let field = |k: &str| pass.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(made), Some(id)) = (field("pass"), field("id")) else { anyhow::bail!("no pass") };
    made_after(&config, &after, &id)?;
    Ok((made, id, pass.get("expires").and_then(Value::as_f64).unwrap_or(0.0)))
}

/// The passes withdrawn that every pass made from now on must be newer than (`after`, R7: the relay's clock set back
/// never makes a pass older than one withdrawn): the last withdrawn, or the last pass made since (newer than them).
fn withdrawn(config: &Config) -> Vec<String> {
    let data = load(config);
    let ids = data.get("withdrawn").and_then(Value::as_array).cloned().unwrap_or_default();
    ids.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// A link removed: its pass goes with those every pass made from now on is newer than. Written before the link is
/// removed (an error: the link stays).
pub fn withdraw_pass(config: &Config, id: &str) -> anyhow::Result<()> {
    edit(config, |data| {
        let mut ids: Vec<Value> = data.get("withdrawn").and_then(Value::as_array).cloned().unwrap_or_default();
        if !ids.iter().any(|v| v.as_str() == Some(id)) {
            ids.push(json!(id));
        }
        data.insert("withdrawn".into(), Value::Array(ids));
    })
}

/// A pass made newer than `after` (those sent): it stands for them from now on (newer than every pass withdrawn).
fn made_after(config: &Config, after: &[String], id: &str) -> anyhow::Result<()> {
    if after.is_empty() {
        return Ok(());
    }
    edit(config, |data| {
        let mut ids: Vec<Value> = data.get("withdrawn").and_then(Value::as_array).cloned().unwrap_or_default();
        ids.retain(|v| v.as_str().is_none_or(|kept| !after.iter().any(|sent| sent == kept)));
        ids.push(json!(id));
        data.insert("withdrawn".into(), Value::Array(ids));
    })
}

/// The number of what this bridge says now of the passes it accepts (R6: `x-bip-passes-seq`, `seq`), taken before
/// reading them: the relay keeps the highest, so a statement made before a withdrawal and come after it never undoes
/// it. Counted up for as long as the bridge runs, from this start's number ([`started`], shifted 22 bits: four
/// million statements a start before they reach the next millisecond's, room until 2109).
pub fn statement_number(config: &Config) -> u64 {
    static NEXT: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
    let next = NEXT.get_or_init(|| std::sync::atomic::AtomicU64::new(started(config) << 22));
    next.fetch_add(1, std::sync::atomic::Ordering::AcqRel) + 1
}

/// This start's number: one more than the last kept, and never below the time in milliseconds (R9: a
/// `family-relay.json` put back from a backup, or a count not written once, never makes a start count from a number
/// already used; milliseconds, not seconds: a bridge restarted twice within a second, then restored, would).
fn started(config: &Config) -> u64 {
    let now = (crate::py::time_time().max(0.0) * 1000.0) as u64 & ((1 << 42) - 1);
    let mut number = now;
    let _ = edit(config, |data| {
        number = (data.get("starts").and_then(Value::as_u64).unwrap_or(0) + 1).max(now);
        data.insert("starts".into(), json!(number));
    });
    number
}

/// The passes this bridge gave and still accepts: one per household linked through the relay (`given_pass`), but
/// for a household whose pass must be made again (`renew_pass`: a link removed while the relay was out of reach; its
/// old pass may be older than the one withdrawn).
fn accepted_passes(state: &AppState) -> Vec<String> {
    let data = state.households.load();
    let Some(households) = data.get("households").and_then(Value::as_object) else { return Vec::new() };
    households
        .values()
        .filter(|v| v.get("renew_pass").and_then(Value::as_bool) != Some(true))
        .filter_map(|v| v.get("given_pass").and_then(Value::as_str).map(str::to_string))
        .collect()
}

/// What goes in `x-bip-passes` with each poll of our mailbox (`-`: none).
fn passes_header(state: &AppState) -> String {
    let ids = accepted_passes(state);
    if ids.is_empty() {
        "-".to_string()
    } else {
        ids.join(",")
    }
}

/// Tells the relay at once which passes our mailbox accepts (a link made or removed): a household unlinked can't
/// reach it any more, nor tell whether this bridge is on. Our polls say it again with each poll.
pub async fn announce_passes(state: &AppState) {
    let Some(base) = base(&state.config()) else { return };
    let Some(token) = load(&state.config())
        .get("mailbox")
        .and_then(|m| m.get("token"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    // its number first, then what it says (R6: a statement numbered before a change says what came before it)
    let seq = statement_number(&state.config());
    let accept = accepted_passes(state);
    let sent = state
        .http
        .post(format!("{base}/v1/family/passes"))
        .timeout(Duration::from_secs(10))
        .bearer_auth(token)
        .json(&json!({"seq": seq, "accept": accept}))
        .send()
        .await;
    match sent {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => tracing::warn!(status = resp.status().as_u16(), "family relay: passes accepted not said"),
        Err(_) => tracing::info!("family relay: passes accepted not said (the relay can't be reached; said at the next poll)"),
    }
}

/// Our pass for this household's mailbox (it was given for ours; it lasts until they withdraw it).
fn pass_for(entry: &Dict) -> Option<String> {
    entry.get("pass").and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string)
}

/// An invitation made with its own mailbox: its exchange will be sealed with its code's key.
pub fn remember_invite(config: &Config, code: &str, until: f64, mailbox: (&str, &str)) -> anyhow::Result<()> {
    let digest = hex::encode(sha2::Sha256::digest(code.as_bytes()));
    let key = family_seal::encode_key(&family_seal::invite_key(code));
    let now = crate::py::time_time();
    edit(config, |data| {
        let mut invites = data.get("invites").and_then(Value::as_object).cloned().unwrap_or_default();
        invites.retain(|_, v| v.get("until").and_then(Value::as_f64).is_some_and(|t| t > now));
        invites.insert(digest, json!({"key": key, "until": until, "mailbox": mailbox.0, "token": mailbox.1}));
        data.insert("invites".into(), Value::Object(invites));
    })
}

/// An invitation still waiting (or used: kept until its end, so that a second introduction with it opens and is
/// answered « unknown or expired » as before, not taken for something else).
struct Invite {
    digest: String,
    key: [u8; 32],
    mailbox: Option<(String, String)>,
    used: bool,
}

fn invites(config: &Config) -> Vec<Invite> {
    let now = crate::py::time_time();
    let data = load(config);
    let Some(invites) = data.get("invites").and_then(Value::as_object) else { return Vec::new() };
    invites
        .iter()
        .filter(|(_, v)| v.get("until").and_then(Value::as_f64).is_some_and(|t| t > now))
        .filter_map(|(d, v)| {
            let key = family_seal::decode_key(v.get("key")?.as_str()?)?;
            let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
            let mailbox = field("mailbox").filter(|m| valid_mailbox(m)).zip(field("token"));
            let used = v.get("used").and_then(Value::as_bool) == Some(true);
            Some(Invite { digest: d.clone(), key, mailbox, used })
        })
        .collect()
}

/// An invitation used: its mailbox no longer polled (it ends at the relay with the invitation), its key kept until
/// its end (see [`Invite`]).
fn forget_invite(config: &Config, digest: &str) {
    let _ = edit(config, |data| {
        if let Some(Value::Object(invites)) = data.get_mut("invites") {
            if let Some(Value::Object(invite)) = invites.get_mut(digest) {
                invite.insert("used".into(), json!(true));
                invite.shift_remove("token");
            }
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

/// The mailboxes this bridge must keep a long poll on (id, token, an invitation's): ours while a household calls us
/// through the relay, each invitation's while it waits.
fn mailboxes_to_poll(state: &AppState) -> Vec<(String, String, bool)> {
    let config = state.config();
    if base(&config).is_none() {
        return Vec::new();
    }
    let mut out: Vec<(String, String, bool)> = invites(&config)
        .into_iter()
        .filter(|i| !i.used)
        .filter_map(|i| i.mailbox.map(|(id, token)| (id, token, true)))
        .collect();
    let data = state.households.load();
    let called = data.get("households").and_then(Value::as_object).is_some_and(|h| {
        h.values().any(|v| v.get("relay_in").and_then(Value::as_bool) == Some(true) && v.get("seal").is_some())
    });
    let kept = load(&config).get("mailbox").and_then(Value::as_object).cloned().unwrap_or_default();
    if let (true, Some(id), Some(token)) = (
        called,
        kept.get("id").and_then(Value::as_str).filter(|id| valid_mailbox(id)),
        kept.get("token").and_then(Value::as_str),
    ) {
        out.push((id.to_string(), token.to_string(), false));
    }
    out
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

/// How a sealed call goes out.
enum Kind<'a> {
    /// To a linked household, with the pass it gave us.
    Call(&'a str),
    /// An introduction, to an invitation's mailbox.
    Invite,
}

/// An answer's head and the rest of it as it comes, from a response whose body is a sealed answer.
async fn opened_answer(
    resp: reqwest::Response,
    key: &[u8; 32],
    salt: &[u8; SALT_LEN],
    way: Way,
) -> Result<(u16, Vec<(String, String)>, Body), CallFailure> {
    let mut opener = AnswerOpener::new(key, salt, way);
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

/// One sealed call to `mailbox`, its answer's head opened: (status, headers, the rest of the answer as it comes).
async fn send(
    state: &AppState,
    mailbox: &str,
    kind: Kind<'_>,
    key: &[u8; 32],
    way: Way,
    head: Value,
    body: &[u8],
) -> Result<(u16, Vec<(String, String)>, Body), CallFailure> {
    let config = state.config();
    let base = base(&config).ok_or(CallFailure::RelayDown)?;
    let (_, token) = mailbox_token(state).await?;
    let (wire, salt) = family_seal::seal_request(key, way, &head, body);
    let mut call = state
        .http
        .post(format!("{base}/v1/family/call/{mailbox}"))
        .timeout(CALL_TIMEOUT)
        .bearer_auth(token)
        .header("content-type", "application/octet-stream");
    call = match kind {
        Kind::Call(pass) => call.header("x-bip-kind", "call").header("x-bip-pass", pass),
        Kind::Invite => call.header("x-bip-kind", "invite"),
    };
    let resp = call.body(wire).send().await.map_err(|_| CallFailure::RelayDown)?;
    match resp.status().as_u16() {
        200 => {}
        // their bridge not connected or not taking it, the relay busy or over a quota: a household not answering
        503 | 504 | 429 | 408 => return Err(CallFailure::Away),
        502 => return Err(CallFailure::RelayDown),
        401 | 403 => {
            tracing::warn!("family relay: our mailbox is refused by the relay (blocked, or sealed with keys it dropped)");
            return Err(CallFailure::RelayDown);
        }
        status => {
            tracing::info!(status, "family relay: call refused by the relay");
            return Err(CallFailure::Broken);
        }
    }
    opened_answer(resp, key, &salt, way).await
}

async fn mailbox_token(state: &AppState) -> Result<(String, String), CallFailure> {
    mailbox(state).await.map_err(|_| CallFailure::RelayDown)
}

/// A call of ours to a household reached through the relay (the loopback listener, family_relay.rs `relay_in`):
/// sealed with the link's key, sent to their mailbox with their pass, their answer opened as it comes.
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
    let Some(pass) = pass_for(&entry) else {
        tracing::info!(household = %household, "family relay: no pass for this household's mailbox (link it again)");
        return not_answering();
    };
    let way = Way::out(&link.role).unwrap_or(Way::Invite);
    let head = json!({"m": method, "p": path, "h": headers, "t": crate::py::time_time()});
    match send(&state, mailbox, Kind::Call(&pass), &link.key, way, head, &body).await {
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

/// An invitation's `POST /family/accept`, sealed with its code: what goes, and the salt its answer opens with.
pub fn sealed_accept(code: &str, payload: &Value, current: Option<&str>) -> (Vec<u8>, [u8; SALT_LEN]) {
    let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
    if let Some(current) = current {
        headers.push(("authorization".into(), format!("Bearer {current}")));
    }
    let head = json!({"m": "POST", "p": "/family/accept", "h": headers, "t": crate::py::time_time()});
    family_seal::seal_request(&family_seal::invite_key(code), Way::Invite, &head, payload.to_string().as_bytes())
}

/// The answer to [`sealed_accept`], if it opens with the invitation's code: (status, body).
pub async fn opened_accept(resp: reqwest::Response, code: &str, salt: &[u8; SALT_LEN]) -> Option<(u16, Bytes)> {
    let key = family_seal::invite_key(code);
    let (status, _, body) = opened_answer(resp, &key, salt, Way::Invite).await.ok()?;
    let bytes = axum::body::to_bytes(body, 1 << 20).await.ok()?;
    Some((status, bytes))
}

/// The scanning bridge's `POST /family/accept` through the relay, to the invitation's mailbox, sealed with the
/// invitation's code: (status, body). `current`: our key for them when we link again.
pub async fn accept(
    state: &AppState,
    mailbox: &str,
    code: &str,
    payload: &Value,
    current: Option<&str>,
) -> Result<(u16, Bytes), CallFailure> {
    let key = family_seal::invite_key(code);
    // the inviter's bridge may be (re)connecting its poll a moment after showing the invitation: a few tries
    let mut tries = 0;
    let (status, body) = loop {
        let mut headers = vec![("content-type".to_string(), "application/json".to_string())];
        if let Some(current) = current {
            headers.push(("authorization".into(), format!("Bearer {current}")));
        }
        let head = json!({"m": "POST", "p": "/family/accept", "h": headers, "t": crate::py::time_time()});
        match send(state, mailbox, Kind::Invite, &key, Way::Invite, head, payload.to_string().as_bytes()).await {
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

/// What this bridge saw (family_seal.rs `Seen`), made once.
fn seen(config: &Config) -> &'static Seen {
    static SEEN: OnceLock<Seen> = OnceLock::new();
    SEEN.get_or_init(|| Seen::new(&config.data_dir()))
}

/// Rings when this bridge may have to poll now (an invitation made, a link through the relay): no wait for the next
/// look.
static WAKE: tokio::sync::Notify = tokio::sync::Notify::const_new();

pub fn wake() {
    WAKE.notify_waiters();
    WAKE.notify_one();
}

/// The mailboxes a long poll runs for.
static POLLING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
/// Calls handled at once.
static HANDLERS: LazyLock<std::sync::Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| std::sync::Arc::new(tokio::sync::Semaphore::new(HANDLING)));

/// Whether the passes are being looked at ([`crate::peers::renew_passes`]).
static RENEWING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How often.
const RENEW_EVERY: Duration = Duration::from_secs(60);

/// Keeps one long poll per mailbox this bridge needs (ours while a household calls us through the relay, each
/// invitation's while it waits); every minute, the passes it gives (made again, given, renewed when due).
pub async fn poll_forever(state: AppState) {
    let mut renewed: Option<std::time::Instant> = None;
    loop {
        if renewed.is_none_or(|at| at.elapsed() >= RENEW_EVERY) && base(&state.config()).is_some() {
            renewed = Some(std::time::Instant::now());
            if !RENEWING.swap(true, std::sync::atomic::Ordering::AcqRel) {
                let state = state.clone();
                tokio::spawn(async move {
                    crate::peers::renew_passes(&state).await;
                    RENEWING.store(false, std::sync::atomic::Ordering::Release);
                });
            }
        }
        for (id, _, invitation) in mailboxes_to_poll(&state) {
            let fresh = POLLING.lock().unwrap_or_else(|e| e.into_inner()).insert(id.clone());
            if fresh {
                tokio::spawn(poll_one(state.clone(), id, invitation));
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), WAKE.notified()).await;
    }
}

/// One mailbox's long poll, as long as it is needed (with its token as kept now: renewed, sealed with the relay's
/// newest key).
async fn poll_one(state: AppState, id: String, invitation: bool) {
    let mut pause = Duration::from_secs(1);
    while let Some((_, token, _)) = mailboxes_to_poll(&state).into_iter().find(|(m, _, _)| *m == id) {
        let Some(base) = base(&state.config()) else { break };
        let mut ask = state.http.get(format!("{base}/v1/family/next")).timeout(POLL_TIMEOUT).bearer_auth(&token);
        if !invitation {
            // the passes our mailbox accepts, said again with each poll (a relay restarted knows them at once)
            let seq = statement_number(&state.config());
            ask = ask.header("x-bip-passes-seq", seq.to_string()).header("x-bip-passes", passes_header(&state));
        }
        let answer = ask.send().await;
        match answer {
            Ok(resp) if resp.status() == StatusCode::OK => {
                pause = Duration::from_secs(1);
                let header =
                    |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                let (call, from, kind) = (header("x-bip-call"), header("x-bip-from"), header("x-bip-kind"));
                let limit = if invitation { INVITE_BYTES } else { REQUEST_BYTES };
                let wire = read_at_most(resp, limit).await;
                // HANDLING calls at once: past it, this mailbox's next poll waits for one to end
                let Ok(permit) = HANDLERS.clone().acquire_owned().await else { break };
                match (wire, valid_call(&call)) {
                    (Some(wire), true) => {
                        let (state, base, token, mailbox) = (state.clone(), base.clone(), token.clone(), id.clone());
                        tokio::spawn(async move {
                            handle(state, base, token, mailbox, invitation, call, from, kind, wire).await;
                            drop(permit);
                        });
                    }
                    _ => tracing::warn!("family relay: a call too large or misnamed, dropped"),
                }
            }
            Ok(resp) if resp.status() == StatusCode::NO_CONTENT => pause = Duration::from_secs(1),
            Ok(resp) => {
                tracing::warn!(status = resp.status().as_u16(), invitation, "family relay: poll refused");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Err(_) => {
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(15));
            }
        }
    }
    POLLING.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

/// A body read up to `limit` bytes (None past it, or cut).
async fn read_at_most(resp: reqwest::Response, limit: usize) -> Option<Bytes> {
    if resp.content_length().is_some_and(|n| n as usize > limit) {
        return None;
    }
    let mut stream = Box::pin(resp.bytes_stream());
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if out.len() + chunk.len() > limit {
            return None;
        }
        out.extend_from_slice(&chunk);
    }
    Some(Bytes::from(out))
}

/// A call that opened: whose, and how its answer is sealed.
pub struct Opened {
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

/// An invitation's exchange: opened with the key of the invitation whose mailbox it came to (or, straight to our
/// address, any of ours still waiting).
pub fn open_invite(config: &Config, mailbox: Option<&str>, wire: &[u8]) -> Option<Opened> {
    invites(config)
        .into_iter()
        .filter(|i| mailbox.is_none() || i.mailbox.as_ref().map(|(m, _)| m.as_str()) == mailbox)
        .find_map(|i| {
            let (salt, head, body) = family_seal::open_request(&i.key, Way::Invite, wire).ok()?;
            Some(Opened { household: None, invite: Some(i.digest), key: i.key, way: Way::Invite, salt, head, body })
        })
}

/// Why an opened call is not taken (logged as such).
fn stale(config: &Config, opened: &Opened) -> Option<&'static str> {
    let t = opened.head.get("t").and_then(Value::as_f64).unwrap_or(f64::NAN);
    match seen(config).take(&opened.salt, t, crate::py::time_time()) {
        Ok(()) => None,
        Err(Stale::Replay) => Some("replay"),
        Err(Stale::Time) => Some("stale"),
        Err(Stale::Unkept) => Some("unkept"),
    }
}

/// An answer sealed for an opened call, as a stream (the answer's own salt first, then its head, then its body).
fn sealed_answer(
    key: &[u8; 32],
    way: Way,
    salt: &[u8; SALT_LEN],
    response: Response,
) -> impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    let status = response.status().as_u16();
    let headers = crate::family_relay::header_pairs(response.headers());
    let mut framer = family_seal::answer_sealer(key, salt, way);
    let first = framer.seal(json!({"s": status, "h": headers}).to_string().as_bytes(), false);
    futures::stream::unfold(
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
    )
}

#[allow(clippy::too_many_arguments)]
async fn handle(
    state: AppState,
    base: String,
    token: String,
    mailbox: String,
    invitation: bool,
    id: String,
    from: String,
    kind: String,
    wire: Bytes,
) {
    let config = state.config();
    let opened = match (invitation, kind.as_str()) {
        (true, "invite") => open_invite(&config, Some(&mailbox), &wire),
        (false, "call") => open_call(&state, &from, &wire),
        _ => None,
    };
    let refused = |reason: &str| {
        tracing::warn!(reason, kind = %kind, "family relay: call refused");
    };
    let response = match opened {
        None => {
            refused("unsealed");
            None
        }
        Some(opened) => match stale(&config, &opened) {
            Some(reason) => {
                refused(reason);
                None
            }
            None => Some(run(&state, opened, Some(&from)).await),
        },
    };
    // a refusal is an empty answer: the caller fails at once instead of waiting for the relay's end
    let body = match response {
        None => reqwest::Body::from(Vec::new()),
        Some((key, way, salt, response)) => reqwest::Body::wrap_stream(sealed_answer(&key, way, &salt, response)),
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

/// An opened call run through this bridge's own routes: (key, way, salt, its response). `relay`: the mailbox an
/// invitation's exchange came from through the relay (None: straight to our address).
async fn run(state: &AppState, opened: Opened, relay: Option<&str>) -> ([u8; 32], Way, [u8; SALT_LEN], Response) {
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
            let introduced = Introduced { relay: relay.map(str::to_string) };
            let response = crate::family_relay::run_accept(state, &pairs, Bytes::from(opened.body), introduced).await;
            if response.status().is_success() {
                forget_invite(&state.config(), digest); // used: its mailbox no longer polled
            }
            response
        }
        _ => HttpError::not_found().into_response(),
    };
    (opened.key, opened.way, opened.salt, response)
}

/// `POST /family/accept/sealed` (open, as `/family/accept`): an invitation's exchange sealed with its code, straight
/// to our address (peers.rs `join`, before the relay). Answered sealed the same way; anything that doesn't open with
/// one of our invitations waiting gets a plain 404, so the scanning bridge goes through the relay.
pub async fn accept_sealed(state: &AppState, wire: Bytes) -> Response {
    let config = state.config();
    let Some(opened) = open_invite(&config, None, &wire) else {
        return HttpError::not_found().into_response();
    };
    if let Some(reason) = stale(&config, &opened) {
        tracing::warn!(reason, "family link: a sealed introduction refused");
        return HttpError::not_found().into_response();
    }
    let (key, way, salt, response) = run(state, opened, None).await;
    let mut answer = Response::new(Body::from_stream(sealed_answer(&key, way, &salt, response)));
    answer.headers_mut().insert("content-type", axum::http::HeaderValue::from_static("application/octet-stream"));
    answer
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
        assert!(valid_call("abcdefgh12345678") && !valid_call("../x") && !valid_call("a/b/c/d/e/f") && !valid_call(""));
    }

    /// R7 (review of R5): once a pass is withdrawn, every pass asked for says it (`after`), and the first one made
    /// then stands for it (newer than every pass withdrawn); a pass withdrawn meanwhile is kept until a pass made after
    /// it. R6: the statements' numbers grow, from one start to the next too.
    #[test]
    fn passes_are_asked_newer_than_those_withdrawn_and_statements_grow() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        assert!(withdrawn(&config).is_empty(), "nothing withdrawn: no `after`");
        made_after(&config, &[], "p0").unwrap();
        assert!(withdrawn(&config).is_empty());
        withdraw_pass(&config, "alex").unwrap();
        withdraw_pass(&config, "alex").unwrap();
        assert_eq!(withdrawn(&config), vec!["alex"]);
        let sent = withdrawn(&config);
        withdraw_pass(&config, "zoe").unwrap(); // withdrawn while that pass was being made
        made_after(&config, &sent, "paul-new").unwrap();
        assert_eq!(withdrawn(&config), vec!["zoe", "paul-new"]);
        let (first, second) = (started(&config), started(&config));
        assert!(second > first, "a later start counts from higher");
        // R9: the count put back from an old backup (or never written): the clock is the floor
        let now = (crate::py::time_time() * 1000.0) as u64;
        edit(&config, |data| {
            data.insert("starts".into(), json!(3));
        })
        .unwrap();
        assert!(started(&config) >= now, "a start after a restore counts from a number already used");
        let (a, b) = (statement_number(&config), statement_number(&config));
        assert!(b > a);
    }

    #[test]
    fn an_invitations_exchange_opens_with_its_own_invitation_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let until = crate::py::time_time() + 600.0;
        remember_invite(&config, "code-1", until, ("AAAAAAAAAAAAAAAAAAAAAA", "fm1.t1")).unwrap();
        remember_invite(&config, "code-2", until, ("BBBBBBBBBBBBBBBBBBBBBB", "fm1.t2")).unwrap();
        let (wire, _) = sealed_accept("code-1", &json!({"name": "Alex"}), None);
        assert!(open_invite(&config, None, &wire).is_some(), "straight to our address: any invitation of ours");
        assert!(open_invite(&config, Some("AAAAAAAAAAAAAAAAAAAAAA"), &wire).is_some());
        assert!(open_invite(&config, Some("BBBBBBBBBBBBBBBBBBBBBB"), &wire).is_none(), "another invitation's mailbox");
        let (other, _) = sealed_accept("code-3", &json!({}), None);
        assert!(open_invite(&config, None, &other).is_none(), "an invitation we never made");
        assert!(!String::from_utf8_lossy(&wire).contains("code-1"), "the code never travels");
    }
}
