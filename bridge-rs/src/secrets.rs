//! Secrets an agent needs for a connector it made (owner decision 2026-10-28, internal/next/agent-connectors.md): a
//! password, an access token or a one-time code, asked of the person in ONE card and kept by the bridge, never by the
//! agent, its model or the conversation.
//!
//! - **The card** (`ask_secret`, connectors.rs): « Vie a besoin de ton accès Garmin », listed with the approvals
//!   (`GET /v1/approvals`, run id `bip-secret-<id>`), with the identifier (its label from the agent, prefilled only when
//!   the agent gave one, editable) and a secure field. The app answers `POST /v1/vault/secrets/requests/{id}` with the
//!   value: it never travels through Hermes, `/v1/approve` only refuses (a notification can't carry a secret).
//! - **Kept**: a password or a token in the vault (vault.rs, kind « secret », Fernet like the rest), tied to the agent
//!   that asked; the app lists it (name, site, identifier, kind, used by, created, last used), never its value. A code
//!   (MFA) is held in memory for `CODE_TTL`, given to the next run of a connector that uses it, then wiped: never
//!   written anywhere.
//! - **The agent** gets « saved as <name> » (the identifier only when it passed it itself), or a refusal / no answer in
//!   plain words. Names are unique per agent; cards are limited per agent (`MAX_WAITING` at once, `MAX_PER_HOUR`).
//! - **Blanking** (`Blanker`): every value of the agent's secrets, raw, URL-encoded, JSON-escaped, hex and base64
//!   (padded, unpadded, URL-safe, and the aligned cores of a value inside a longer base64 stream, as in a Basic
//!   header), is replaced by « [secret] » in whatever a connector sends back before it reaches Hermes, the model, the
//!   history or a log line.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The vault's kind for these items.
pub const VAULT_KIND: &str = "secret";
pub const KINDS: [&str; 3] = ["password", "token", "code"];
/// The run id of a card (the apps tell them from Hermes' approvals by it).
pub const RUN_PREFIX: &str = "bip-secret-";
/// How long the agent's call waits for the person.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(330);
/// A card the person opened from the app (« Modifier le mot de passe »): gone after so long unanswered.
pub const EDIT_TTL: f64 = 3600.0;
/// A one-time code waits so long for the run that uses it.
pub const CODE_TTL: Duration = Duration::from_secs(600);
/// Cards waiting at once, per agent.
pub const MAX_WAITING: usize = 2;
/// Cards an agent may raise per hour.
pub const MAX_PER_HOUR: usize = 10;
pub const MAX_VALUE: usize = 4096;
pub const MAX_IDENTIFIER: usize = 200;
/// The refusal's code when an agent other people talk to gets a secret without the person's confirmation.
pub const CONFIRM_OTHERS: &str = "confirm_others";
/// Its words, for an app that doesn't know the code.
pub const CONFIRM_OTHERS_TEXT: &str = "Mets à jour l’app pour donner cet accès : d’autres personnes parlent à cet agent, \
l’app doit d’abord te le demander.";
/// What a secret's value becomes.
pub const MARK: &str = "[secret]";

/// `garmin`, `ovh_api`: lower-case letters, digits and `_`, 2-32 characters, a letter first.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// The environment variables a connector gets for secret `name` of `kind`: (identifier's, value's).
pub fn env_names(name: &str, kind: &str) -> (String, String) {
    let base = name.to_uppercase();
    let value = match kind {
        "token" => "TOKEN",
        "code" => "CODE",
        _ => "PASSWORD",
    };
    (format!("{base}_USERNAME"), format!("{base}_{value}"))
}

/// What the person reads for a kind.
pub fn kind_words(kind: &str) -> &'static str {
    match kind {
        "token" => "clé d'accès",
        "code" => "code à usage unique",
        _ => "mot de passe",
    }
}

// --- blanking ------------------------------------------------------------------------------------------------------

fn percent(value: &str, keep: fn(u8) -> bool, upper: bool, plus: bool) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if keep(b) {
            out.push(b as char);
        } else if plus && b == b' ' {
            out.push('+');
        } else if upper {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// RFC 3986's unreserved characters (Python's `quote(safe="")`, most clients).
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.~".contains(&b)
}

/// JavaScript's `encodeURIComponent`.
fn component(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b)
}

/// The forms a secret's value may take in a connector's output, the longest first.
pub fn variants(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if value.chars().count() < 3 {
        return out;
    }
    out.push(value.to_string());
    if value.len() >= 4 {
        for keep in [alnum as fn(u8) -> bool, unreserved, component] {
            for upper in [true, false] {
                for plus in [false, true] {
                    out.push(percent(value, keep, upper, plus));
                }
            }
        }
        if let Ok(quoted) = serde_json::to_string(value) {
            out.push(quoted.trim_matches('"').to_string());
        }
        out.push(hex::encode(value.as_bytes()));
        out.push(hex::encode_upper(value.as_bytes()));
        let n = value.len();
        for engine in [base64::engine::general_purpose::STANDARD_NO_PAD, base64::engine::general_purpose::URL_SAFE_NO_PAD]
        {
            for k in 0..3usize {
                let mut bytes = vec![0u8; k];
                bytes.extend_from_slice(value.as_bytes());
                let encoded = engine.encode(&bytes);
                // the characters that depend on the value's bytes only, wherever it sits in a longer stream
                let start = (8 * k).div_ceil(6);
                let end = (8 * (k + n)) / 6;
                if end > start + 5 && end <= encoded.len() {
                    out.push(encoded[start..end].to_string());
                }
            }
        }
        out.push(base64::engine::general_purpose::STANDARD.encode(value.as_bytes()));
        out.push(base64::engine::general_purpose::URL_SAFE.encode(value.as_bytes()));
    }
    out.retain(|v| v.chars().count() >= 3);
    out.sort_by_key(|v| std::cmp::Reverse(v.len()));
    out.dedup();
    out
}

/// Replaces every known form of the given values by « [secret ] ».
#[derive(Default, Clone)]
pub struct Blanker {
    forms: Vec<Zeroizing<String>>,
}

impl Blanker {
    pub fn new<S: AsRef<str>>(values: &[S]) -> Self {
        let mut forms: Vec<Zeroizing<String>> = Vec::new();
        for value in values {
            for form in variants(value.as_ref()) {
                if !forms.iter().any(|f| f.as_str() == form) {
                    forms.push(Zeroizing::new(form));
                }
            }
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        Blanker { forms }
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for form in &self.forms {
            if out.contains(form.as_str()) {
                out = out.replace(form.as_str(), MARK);
            }
        }
        out
    }

    /// Every string of a JSON value (keys too).
    pub fn value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(&s)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (self.text(&k), self.value(v))).collect()),
            other => other,
        }
    }
}

/// The blanker for everything an agent's connectors send back: all its secrets' values, the codes waiting too.
pub fn blanker_for(state: &AppState, agent: &str, extra: &[Zeroizing<String>]) -> Blanker {
    let mut values: Vec<Zeroizing<String>> =
        state.vault.secret_values(agent).unwrap_or_default().into_iter().map(|(_, _, v)| v).collect();
    values.extend(codes_of(agent));
    values.extend(extra.iter().cloned());
    Blanker::new(&values.iter().map(|v| v.as_str()).collect::<Vec<_>>())
}

// --- one-time codes (memory only) -------------------------------------------------------------------------------------

type CodeTable = HashMap<(String, String), (Zeroizing<String>, Instant)>;

static CODES: LazyLock<Mutex<CodeTable>> = LazyLock::new(Default::default);

fn codes() -> std::sync::MutexGuard<'static, CodeTable> {
    let mut table = CODES.lock().unwrap_or_else(|e| e.into_inner());
    table.retain(|_, (_, at)| at.elapsed() < CODE_TTL);
    table
}

/// The code given for `name`, taken: the next run uses it, nothing else ever sees it again.
pub fn take_code(agent: &str, name: &str) -> Option<Zeroizing<String>> {
    codes().remove(&(agent.to_string(), name.to_string())).map(|(code, _)| code)
}

pub fn code_waiting(agent: &str, name: &str) -> bool {
    codes().contains_key(&(agent.to_string(), name.to_string()))
}

fn codes_of(agent: &str) -> Vec<Zeroizing<String>> {
    codes().iter().filter(|((a, _), _)| a == agent).map(|(_, (code, _))| code.clone()).collect()
}

// --- the cards ---------------------------------------------------------------------------------------------------------

/// What a card asks for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub name: String,
    /// `https://connect.garmin.com` (empty: no site).
    pub origin: String,
    pub kind: String,
    pub identifier_label: String,
    pub identifier: String,
    pub reason: String,
    /// Opened by the person from a connector's page (no agent waits for it).
    pub edit: bool,
    pub connector: Option<String>,
}

impl Request {
    pub fn host(&self) -> String {
        self.origin.split("://").nth(1).unwrap_or(&self.origin).to_string()
    }

    /// For a password the identifier is asked too (always); for the others when the agent gave one.
    pub fn identifier_required(&self) -> bool {
        self.kind == "password"
    }
}

/// The person's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Kept (a code: held for the next run), with the identifier the person confirmed.
    Saved { identifier: String },
    Refused,
}

struct Card {
    agent: String,
    request: Request,
    at: f64,
    answer: Option<Outcome>,
    notify: Arc<Notify>,
}

static CARDS: LazyLock<Mutex<IndexMap<String, Card>>> = LazyLock::new(Default::default);
static RAISED: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(Default::default);

fn cards() -> std::sync::MutexGuard<'static, IndexMap<String, Card>> {
    let mut table = CARDS.lock().unwrap_or_else(|e| e.into_inner());
    let now = py::time_time();
    table.retain(|_, c| !(c.request.edit && now - c.at > EDIT_TTL));
    table
}

fn shown_name(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(agent.to_string())
}

/// « Garmin » from `connect.garmin.com`, else the secret's name.
pub fn service_of(request: &Request) -> String {
    let host = request.host();
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    let word = if parts.len() >= 2 { parts[parts.len() - 2] } else { request.name.as_str() };
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => request.name.clone(),
    }
}

/// The card's sentence.
pub fn sentence(shown: &str, request: &Request) -> String {
    let host = request.host();
    let what = match request.kind.as_str() {
        "code" => "du code reçu pour",
        "token" => "de ta clé d'accès",
        _ => "de ton accès",
    };
    if host.is_empty() {
        format!("{shown} a besoin {what} {}", service_of(request))
    } else {
        format!("{shown} a besoin {what} {} ({host})", service_of(request))
    }
}

/// A new card, or why not (the agent is told). `edit`: opened by the person, no rate limit.
pub fn open(agent: &str, request: Request) -> Result<String, String> {
    let mut table = cards();
    if table.values().any(|c| c.agent == agent && c.request.name == request.name && c.answer.is_none()) {
        return Err(format!("A card for « {} » is already waiting for the person.", request.name));
    }
    if !request.edit {
        let waiting = table.values().filter(|c| c.agent == agent && c.answer.is_none() && !c.request.edit).count();
        if waiting >= MAX_WAITING {
            return Err("Several cards already wait for the person: wait for their answers first.".into());
        }
        let mut raised = RAISED.lock().unwrap_or_else(|e| e.into_inner());
        let recent = raised.entry(agent.to_string()).or_default();
        recent.retain(|at| at.elapsed() < Duration::from_secs(3600));
        if recent.len() >= MAX_PER_HOUR {
            return Err("Too many secrets asked within the hour: wait, and ask the person what they want.".into());
        }
        recent.push(Instant::now());
    }
    let id = py::uuid4_hex()[..12].to_string();
    tracing::info!(agent, secret = %request.name, kind = %request.kind, edit = request.edit, "secret card raised");
    table.insert(
        id.clone(),
        Card { agent: agent.to_string(), request, at: py::time_time(), answer: None, notify: Arc::new(Notify::new()) },
    );
    drop(table);
    Ok(id)
}

/// The person's answer to card `id` (None: no answer in time). The card goes either way.
pub async fn wait(id: &str) -> Option<Outcome> {
    let Some(notify) = cards().get(id).map(|c| c.notify.clone()) else { return Some(Outcome::Refused) };
    let deadline = tokio::time::Instant::now() + ASK_TIMEOUT;
    let outcome = loop {
        let wake = notify.notified();
        match cards().get(id).map(|c| c.answer.clone()) {
            Some(Some(answer)) => break Some(answer),
            Some(None) => {}
            None => break Some(Outcome::Refused),
        }
        if tokio::time::timeout_at(deadline, wake).await.is_err() {
            break None;
        }
    };
    cards().shift_remove(id);
    outcome
}

/// The card waiting with this id, as the apps show it.
fn item_of(config: &Config, id: &str, card: &Card) -> Dict {
    let r = &card.request;
    let item = json!({
        "agent": card.agent,
        "run_id": format!("{RUN_PREFIX}{id}"),
        "request_id": id,
        "command": if r.host().is_empty() { r.name.clone() } else { format!("{} · {}", r.name, r.host()) },
        "description": sentence(&shown_name(config, &card.agent), r),
        "choices": ["once", "deny"],
        "created_at": card.at,
        "session_id": null,
        "bip_secret_name": r.name,
        "bip_secret_site": r.host(),
        "bip_secret_kind": r.kind,
        "bip_secret_identifier_label": r.identifier_label,
        "bip_secret_identifier": r.identifier,
        "bip_secret_identifier_required": r.identifier_required(),
        "bip_secret_reason": r.reason,
        "bip_secret_edit": r.edit,
        "bip_secret_connector": r.connector,
    });
    let mut item = item.as_object().cloned().unwrap_or_default();
    if crate::vault::others_facing(config, &card.agent) {
        // other people talk to it: the app asks one confirmation first (`give`'s `confirmed_others`)
        item.insert("bip_secret_others".into(), json!(true));
        item.insert("bip_secret_others_names".into(), json!(crate::vault::others_names(config, &card.agent)));
    }
    item
}

/// The cards waiting (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    cards()
        .iter()
        .filter(|(_, c)| c.answer.is_none() && agent.is_none_or(|a| a == c.agent))
        .map(|(id, c)| item_of(config, id, c))
        .collect()
}

/// One card as the apps show it (the app shows a card it opened at once).
pub fn item(config: &Config, id: &str) -> Option<Dict> {
    cards().get(id).map(|c| item_of(config, id, c))
}

fn record(config: &Config, agent: &str, id: &str, request: &Request, allow: bool, via: &str) {
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id,
        "command": if request.host().is_empty() { request.name.clone() } else { format!("{} · {}", request.name, request.host()) },
        "description": sentence(&shown_name(config, agent), request),
        "topic": "secret", "choice": if allow { "once" } else { "deny" }, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(agent, error = %exc, "secret decision not kept");
        }
    }
}

/// Refused (the app, or a notification's action). False: no such card waiting.
pub fn deny(config: &Config, agent: Option<&str>, id: &str, via: &str) -> bool {
    let (agent, request) = {
        let mut table = cards();
        let Some(card) = table.get_mut(id).filter(|c| c.answer.is_none() && agent.is_none_or(|a| a == c.agent)) else {
            return false;
        };
        card.answer = Some(Outcome::Refused);
        card.notify.notify_waiters();
        let out = (card.agent.clone(), card.request.clone());
        if card.request.edit {
            table.shift_remove(id);
        }
        out
    };
    record(config, &agent, id, &request, false, via);
    tracing::info!(agent = %agent, secret = %request.name, "secret card refused");
    true
}

/// Why a value can't be taken.
#[derive(Debug)]
pub enum GiveError {
    NoCard,
    /// The agent talks with other people too, and the person did not confirm (an app from before the confirmation).
    ConfirmOthers,
    Invalid(String),
    Failed(anyhow::Error),
}

/// The person gave the value (and the identifier): kept in the vault, or held for the next run (a code). Never logged.
/// For an agent other people talk to (vault.rs `others_facing`), only once the person confirmed it
/// (`confirmed_others`, the apps' « … parle aussi avec … Continuer ? »): an app that doesn't ask can't skip it.
pub async fn give(
    state: &AppState,
    id: &str,
    value: Zeroizing<String>,
    identifier: Option<String>,
    confirmed_others: bool,
    via: &str,
) -> Result<String, GiveError> {
    let config = state.config();
    let (agent, request) = match cards().get(id).filter(|c| c.answer.is_none()) {
        Some(card) => (card.agent.clone(), card.request.clone()),
        None => return Err(GiveError::NoCard),
    };
    if value.is_empty() || value.len() > MAX_VALUE || value.contains('\0') {
        return Err(GiveError::Invalid(format!("the value must be 1 to {MAX_VALUE} characters")));
    }
    let identifier = identifier.map(|i| i.trim().to_string()).unwrap_or_else(|| request.identifier.clone());
    if identifier.chars().count() > MAX_IDENTIFIER {
        return Err(GiveError::Invalid(format!("the identifier is {MAX_IDENTIFIER} characters at most")));
    }
    if request.identifier_required() && identifier.is_empty() {
        return Err(GiveError::Invalid("the identifier is required".into()));
    }
    if !confirmed_others && crate::vault::others_facing(&config, &agent) {
        tracing::info!(agent = %agent, secret = %request.name, "secret not kept: other people talk to the agent, not confirmed");
        return Err(GiveError::ConfirmOthers);
    }
    if request.kind == "code" {
        codes().insert((agent.clone(), request.name.clone()), (value, Instant::now()));
        // the code's name is known (its connector lists it), never its value
        let Some(holder) = config.agent(Some(&agent)).cloned() else { return Err(GiveError::NoCard) };
        let new = crate::vault::NewSecret {
            name: &request.name,
            origin: &request.origin,
            kind: &request.kind,
            identifier_label: &request.identifier_label,
            identifier: &identifier,
            value: None,
        };
        state.vault.save_secret(&config, &holder, &new).await.map_err(GiveError::Failed)?;
    } else {
        let Some(holder) = config.agent(Some(&agent)).cloned() else { return Err(GiveError::NoCard) };
        let new = crate::vault::NewSecret {
            name: &request.name,
            origin: &request.origin,
            kind: &request.kind,
            identifier_label: &request.identifier_label,
            identifier: &identifier,
            value: Some(value.as_str()),
        };
        state.vault.save_secret(&config, &holder, &new).await.map_err(GiveError::Failed)?;
    }
    {
        let mut table = cards();
        if let Some(card) = table.get_mut(id) {
            card.answer = Some(Outcome::Saved { identifier: identifier.clone() });
            card.notify.notify_waiters();
            if card.request.edit {
                table.shift_remove(id);
            }
        }
    }
    record(&config, &agent, id, &request, true, via);
    tracing::info!(agent = %agent, secret = %request.name, kind = %request.kind, "secret given by the person");
    Ok(request.name)
}

// --- the app's route (bridge key) --------------------------------------------------------------------------------------

/// A secret card answered in the app: `{"allow": true, "value": "…", "identifier": "…"}` or `{"allow": false}`.
async fn answer_route(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> ApiResult<Response> {
    let body: Value = serde_json::from_slice(&body).map_err(|_| HttpError::new(400, "invalid JSON"))?;
    let Some(allow) = body.get("allow").and_then(Value::as_bool) else {
        return Err(HttpError::new(400, "allow must be true or false"));
    };
    let mut response = if !allow {
        if !deny(&state.config(), None, &id, "app") {
            return Err(HttpError::new(404, "no such request (answered already?)"));
        }
        web::ok(json!({"ok": true, "choice": "deny"}))
    } else {
        let value = Zeroizing::new(body.get("value").and_then(Value::as_str).unwrap_or("").to_string());
        let identifier = body.get("identifier").and_then(Value::as_str).map(str::to_string);
        let confirmed = body.get("confirmed_others").and_then(Value::as_bool) == Some(true);
        match give(&state, &id, value, identifier, confirmed, "app").await {
            Ok(name) => web::ok(json!({"ok": true, "name": name})),
            Err(GiveError::NoCard) => return Err(HttpError::new(404, "no such request (answered already?)")),
            Err(GiveError::ConfirmOthers) => return Err(HttpError::json(409, json!({"code": CONFIRM_OTHERS,
                "message": CONFIRM_OTHERS_TEXT}))),
            Err(GiveError::Invalid(why)) => return Err(HttpError::new(400, why)),
            Err(GiveError::Failed(exc)) => {
                tracing::warn!(request = %id, error = %crate::agents_admin::tail(&exc.to_string(), 200), "secret not kept");
                return Err(HttpError::new(409, "the secret could not be kept"));
            }
        }
    };
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `/v1/vault/secrets/requests/{id}` (mounted under /v1 with the bridge key).
pub fn app_router() -> Router<AppState> {
    Router::new().route("/vault/secrets/requests/{id}", post(answer_route))
}

/// The agent's identity for a card (tests and connectors.rs).
pub fn agent_of_card(id: &str) -> Option<String> {
    cards().get(id).map(|c| c.agent.clone())
}

/// Cards of a deleted agent go.
pub fn forget(agent: &AgentConfig) {
    cards().retain(|_, c| c.agent != agent.name);
    codes().retain(|(a, _), _| a != &agent.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "Tr0ub4dor&3 x/y+z";

    #[test]
    fn names_and_variables() {
        assert!(valid_name("garmin") && valid_name("ovh_api2"));
        assert!(!valid_name("G") && !valid_name("1abc") && !valid_name("ab-c") && !valid_name(&"a".repeat(33)));
        assert_eq!(env_names("garmin", "password"), ("GARMIN_USERNAME".into(), "GARMIN_PASSWORD".into()));
        assert_eq!(env_names("ovh_api", "token").1, "OVH_API_TOKEN");
        assert_eq!(env_names("garmin_mfa", "code").1, "GARMIN_MFA_CODE");
    }

    #[test]
    fn every_encoding_is_blanked() {
        let b = Blanker::new(&[SECRET]);
        let std = base64::engine::general_purpose::STANDARD;
        let basic = std.encode(format!("alex@example.org:{SECRET}"));
        let cases = [
            SECRET.to_string(),
            percent(SECRET, unreserved, true, false),
            percent(SECRET, alnum, false, true),
            percent(SECRET, component, true, false),
            std.encode(SECRET),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(SECRET),
            hex::encode(SECRET),
            serde_json::to_string(SECRET).unwrap(),
            format!("Authorization: Basic {basic}"),
        ];
        for case in cases {
            let out = b.text(&format!("before {case} after"));
            assert!(out.contains(MARK), "{case} -> {out}");
            assert!(!out.contains(SECRET), "{out}");
        }
        // inside a longer base64 stream (a Basic header): the value's own characters are gone
        let out = b.text(&basic);
        let core = &std.encode(format!("alex@example.org:{SECRET}"))[24..32];
        assert!(!out.contains(core), "{out}");
        let json = b.value(json!({"ok": [SECRET, {"k": format!("x{SECRET}")}], SECRET: 1}));
        assert!(!json.to_string().contains(SECRET));
        assert_eq!(Blanker::new(&["ab"]).text("ab cd"), "ab cd"); // too short to blank
    }

    #[test]
    fn cards_are_limited_and_unique() {
        let request = |name: &str| Request { name: name.into(), kind: "password".into(), ..Default::default() };
        let a = open("unit-a", request("garmin")).unwrap();
        assert!(open("unit-a", request("garmin")).unwrap_err().contains("already waiting"));
        let _b = open("unit-a", request("strava")).unwrap();
        assert!(open("unit-a", request("ovh")).unwrap_err().contains("Several cards"));
        assert!(cards().get(&a).unwrap().request.identifier_required());
        assert_eq!(cards().values().filter(|c| c.agent == "unit-a").count(), 2);
        cards().retain(|_, c| c.agent != "unit-a");
    }

    #[test]
    fn sentences() {
        let r = Request {
            name: "garmin".into(),
            origin: "https://connect.garmin.com".into(),
            kind: "password".into(),
            ..Default::default()
        };
        assert_eq!(sentence("Vie", &r), "Vie a besoin de ton accès Garmin (connect.garmin.com)");
    }
}
