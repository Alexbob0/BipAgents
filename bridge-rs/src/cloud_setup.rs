//! « No computer »: a server of the all-included offer, from the guichet's reserve (guichet/), set up at first boot by
//! the installer's `--cloud` mode, then paired with the person's app without a QR code.
//!
//! The guichet made a one-time secret (`pair`, 32 random bytes), put it in the server's first-boot setup and hands it to
//! the app that bought the server; nothing else secret is there, no model key, since the provider keeps that setup. The bridge's door is on the Internet with its own self-signed
//! certificate (lan.py). First the app learns that this certificate is really its server's: the bridge proves it knows
//! `pair` with an HMAC over the app's nonce and the certificate's fingerprint, which nobody in between can forge, and the
//! app pins it. Then the app claims the server over that pinned connection: it proves it knows `pair` too (an HMAC over
//! the bridge's one-time challenge and the exact request), sends the model key and the agents to create, and receives the
//! bridge key. The secret is then deleted: any later claim is refused, but its claimer's own retry while the bridge
//! key was never used, within a day (« a claim whose answer was lost » below, internal/next/claim-recovery.md).
//!
//!     GET  /v1/setup?nonce=<hex>   {state: installing | ready | restoring | claimed, step, total, label, error?,
//!                                   fingerprint, challenge, proof, prepared, claimed?}
//!     POST /v1/setup/claim         {challenge, mac, request: "<JSON>"}  ->  {bridgeURL, bridgeKey, fingerprint, agents,
//!                                                                            recovered?}
//!     request (a claim): {model, agents, language, claim_id?, timezone?, country?}
//!
//! The claim's language, the phone's time zone (IANA) and country (ISO 3166 alpha-2) are the place's from then on
//! (`apply_locale`): every agent's personality, the Carnet's language and later agents' default, Hermes' `timezone`
//! (dates/), the country the agents are told when it isn't the zone's. Older apps send no zone nor country: the
//! server's zone stays until the app reports the phone's (`PUT /v1/time/device`).
//!
//! A place whose person recovered their account without any of their devices (guichet/bipguichet/accounts.py) gets a
//! new secret from the guichet's signed order and is claimed with `{"reopen": {"name": …}}`: the bridge key changes and
//! every device key goes (device_keys.py); the device gets a key of its own.
//!
//! A place given to someone whose server died (the guichet moved them, guichet/bipguichet/watch.py) is claimed with
//! `{"restore": {"key": …}}` instead: the backup key the person's phone kept. The installer's `restore` puts their last
//! backup back (installer/bipinstall/backup.py) in a unit of its own, its steps shown as `restoring`, then restarts the
//! bridge with their own keys and certificate: the app then reaches it as before, at this new address.
//!
//! Port of `bridge/bipbridge/cloud_setup.py` (owner group: browsers-places).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::Engine as _;
use futures::future::BoxFuture;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::auth::ClientAddr;
use crate::browsers::pyx;
use crate::config::{Config, SharedConfig};
use crate::logs::J;
use crate::py::Clock;
use crate::web::{self, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::Dict;

pub const SETUP_CONTEXT: &str = "bipagents-setup-v1";
pub const CLAIM_CONTEXT: &str = "bipagents-claim-v1";
pub const CHALLENGE_SECONDS: f64 = 600.0;
pub const MODEL_KEYS: [(&str, &str); 2] = [("openrouter", "OPENROUTER_API_KEY"), ("custom", "CUSTOM_MODEL_API_KEY")];

/// HMAC-SHA256 of the parts, one per line, lowercase hex (the apps compute the same).
pub fn proof(secret: &[u8], parts: &[&str]) -> String {
    pyx::hmac_hex(secret, parts)
}

fn read_json(path: Option<&str>) -> Dict {
    let Some(path) = path.filter(|p| !p.is_empty()) else { return Dict::new() };
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

/// The pairing secret while the server waits for its app; None once claimed (the file is gone).
pub fn secret_of(config: &Config) -> Option<Vec<u8>> {
    let data = read_json(config.setup_file.as_deref());
    decode_pair(data.get("pair")?.as_str()?)
}

/// The guichet's `pair` (urlsafe base64) as bytes; None when it is not a usable secret.
fn decode_pair(value: &str) -> Option<Vec<u8>> {
    // Python's urlsafe_b64decode (non-strict): characters outside the alphabet dropped, the padding added.
    let clean: String = value.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    if clean.len() % 4 == 1 {
        return None; // binascii.Error: Incorrect padding / invalid length
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        base64::engine::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
            .with_decode_allow_trailing_bits(true),
    );
    let secret = engine.decode(clean.as_bytes()).ok()?;
    (secret.len() >= 16).then_some(secret)
}

/// What the installer wrote at each step (`--cloud`, or a restore).
#[derive(Debug, Clone)]
pub struct Progress {
    pub step: i64,
    pub total: i64,
    pub label: String,
    pub done: bool,
    pub restoring: bool,
    /// `data.get("error") or None`
    pub error: Option<Value>,
}

/// What the installer wrote at each step (`--cloud`, or a restore): step, total, label, done, restoring, error.
pub fn progress_of(config: &Config) -> anyhow::Result<Progress> {
    let data = read_json(config.setup_progress.as_deref());
    let int = |k: &str| -> anyhow::Result<i64> {
        match data.get(k).filter(|v| crate::py::truthy(v)) {
            None => Ok(0),
            Some(v) => crate::py::py_int(v).map_err(|e| anyhow::anyhow!(e)),
        }
    };
    let truthy = |k: &str| data.get(k).map(crate::py::truthy).unwrap_or(false);
    Ok(Progress {
        step: int("step")?,
        total: int("total")?,
        label: data.get("label").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default(),
        done: truthy("done"),
        restoring: truthy("restoring"),
        error: data.get("error").filter(|v| crate::py::truthy(v)).cloned(),
    })
}

fn setup_folder(config: &Config) -> PathBuf {
    let file = PathBuf::from(config.setup_file.clone().unwrap_or_default());
    file.parent().map(Path::to_path_buf).unwrap_or_default()
}

pub fn restoring(config: &Config) -> bool {
    config.setup_file.as_deref().map(|f| !f.is_empty()).unwrap_or(false)
        && setup_folder(config).join("restoring.json").exists()
}

/// The installer's restore, in a systemd unit of its own (restarting the bridge must not stop it); the key in a
/// file only this account reads, deleted once read.
pub fn start_restore(config: &Config, key: &str) -> std::io::Result<()> {
    let key_file = setup_folder(config).join("restore-key");
    crate::py::write_with_mode(&key_file.to_string_lossy(), key.as_bytes(), 0o600)?;
    let mut command = crate::place_import::installer_command("bipagents-restore");
    command.args(["restore", "--key-file", &key_file.to_string_lossy()]);
    crate::place_import::spawn_detached(command)
}

/// The person recovered their account without any of their devices (the guichet's signed order put a new pairing
/// secret here): every device key and web console goes, the bridge key changes (nobody has the new one), and the
/// device that proved the secret gets a key of its own. Returns it.
pub fn reopen(state: &AppState, name: &str) -> anyhow::Result<String> {
    let config = state.config();
    let keys = crate::device_keys::of(&config);
    keys.clear();
    let (_, key) = keys.mint(name)?;
    rotate_bridge_key(state)?;
    // the web consoles opened from the old devices too
    for session in state.console.devices() {
        if let Some(id) = session.get("id") {
            state.console.revoke(&crate::py::py_str(id));
        }
    }
    Ok(key)
}

/// A new bridge key, in bridge.toml and in the running config: the old one opens nothing any more. Returns it.
fn rotate_bridge_key(state: &AppState) -> anyhow::Result<String> {
    static BRIDGE_KEY_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^bridge_key\s*=.*$").expect("valid regex"));
    let config = state.config();
    let new = crate::py::token_urlsafe(32);
    if let Some(source) = config.source_path.as_deref().filter(|p| !p.is_empty()) {
        let line = format!("bridge_key = {}", crate::py::dumps(&json!(new)));
        std::fs::metadata(source)?;
        crate::config::update_source(source, |text| {
            Ok(if BRIDGE_KEY_LINE.is_match(text) {
                BRIDGE_KEY_LINE.replacen(text, 1, regex::NoExpand(&line)).into_owned()
            } else {
                format!("{line}\n{text}")
            })
        })?;
    }
    state.config.update(|c| c.bridge_key = new.clone());
    Ok(new)
}

// -- a claim whose answer was lost -------------------------------------------------------------------
//
// The claim's answer carries the bridge key, and nothing else ever gives it. An app that loses it (the person left
// the screen, the network dropped, the app was killed while the gateway restarted) used to find a place that says
// `claimed` to it as to anyone: stuck for its rightful owner. So the bridge keeps, next to where the secret was, a
// record of the claim (`claim-recovery.json`, chmod 600): the pairing secret, when, the claim id's and the bridge
// key's SHA-256. While it holds, `GET /v1/setup` still proves the certificate with the secret and says `ready` (with
// `claimed: true`), so an app retrying from the start reaches the claim again, and a claim proved with the same
// secret gets the place back: a fresh bridge key (the lost one revoked: whoever saw it holds nothing), the same
// agents, the new AI key written if the app brought another one (the guichet deletes the previous one when it makes
// a new one). Its answer has the first one's shape, plus `recovered: true`.
//
// The record goes, and `claimed` is the only answer again, at the first of:
// - the bridge key used once (any request authenticated with it, auth.rs): the app has its answer, nobody may take
//   the place from it any more;
// - 24 hours after the claim;
// - the bridge key changed by something else (a reopen), or a new pairing secret written (setup.json).
//
// Who can recover: only a holder of the pairing secret, as for the first claim (the app and the guichet that made
// it: the guichet could have claimed first anyway). The proof is the same HMAC over a fresh one-time challenge, and
// wrong proofs count towards the address' block as before. When the first claim carried a `claim_id` (a random id
// the app made, kept on the phone before sending, never given to the guichet), the retry must carry the same one:
// then even a holder of the secret cannot recover in the owner's place. Apps that send none (the ones out before
// this) recover with the secret alone, within the bounds above. Only a plain claim is recovered: a reopen or a
// restore after a claim gets `410` as before.
//
// The secret on disk for at most a day after the claim is no more than what setup.json held before it (the
// backups, installer/bipinstall/backup.py, start once setup.json is gone, and bridge.toml with the key itself is in
// them anyway).

/// How long after its claim a place can be recovered by its claimer (if its bridge key was never used).
pub const RECOVERY_SECONDS: f64 = 24.0 * 3600.0;

fn recovery_path(config: &Config) -> PathBuf {
    setup_folder(config).join("claim-recovery.json")
}

fn sha256_hex(text: &str) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(text.as_bytes()))
}

/// `claim_id` in a claim's request: 16 to 128 characters of [A-Za-z0-9_-]; Ok(None) when absent.
fn claim_id_of(asked: &Value) -> Result<Option<String>, String> {
    check_str(asked.get("claim_id"), &Str { min: Some(16), max: Some(128), pattern: Some(r"^[A-Za-z0-9_\-]+$") }, true)
}

/// The record a plain claim leaves (written before setup.json goes).
fn write_recovery(config: &Config, claim_id: Option<&str>, model_key: &str, agents: &[Value]) -> anyhow::Result<()> {
    let data = read_json(config.setup_file.as_deref());
    let Some(pair) = data.get("pair").and_then(Value::as_str) else { return Ok(()) };
    let record = json!({
        "pair": pair, "at": crate::py::time_time(), "claim_id": claim_id.map(sha256_hex),
        "key": sha256_hex(&config.bridge_key), "model_key": sha256_hex(model_key), "agents": agents,
    });
    let path = recovery_path(config);
    crate::py::write_with_mode(&path.to_string_lossy(), crate::py::dumps(&record).as_bytes(), 0o600)?;
    Ok(())
}

/// The record of a claim still recoverable, with its secret; an expired or stale one is deleted.
fn recovery_of(config: &Config) -> Option<(Vec<u8>, Dict)> {
    if config.setup_file.as_deref().map(str::is_empty).unwrap_or(true) {
        return None;
    }
    let path = recovery_path(config);
    let data = read_json(Some(&path.to_string_lossy()));
    if data.is_empty() {
        return None;
    }
    let at = data.get("at").and_then(Value::as_f64).unwrap_or(0.0);
    let fresh = (0.0..RECOVERY_SECONDS).contains(&(crate::py::time_time() - at));
    let same_key = data.get("key").and_then(Value::as_str) == Some(sha256_hex(&config.bridge_key).as_str());
    let secret = data.get("pair").and_then(Value::as_str).and_then(decode_pair);
    match secret {
        Some(secret) if fresh && same_key => Some((secret, data)),
        _ => {
            let _ = std::fs::remove_file(&path);
            None
        }
    }
}

/// The bridge key was used (auth.rs): its app has the claim's answer, the claim can't be recovered any more.
pub fn bridge_key_used(config: &Config) {
    if config.setup_file.as_deref().map(str::is_empty).unwrap_or(true) {
        return;
    }
    let path = recovery_path(config);
    if path.exists() && std::fs::remove_file(&path).is_ok() {
        tracing::info!("the claim's bridge key used: no recovery any more");
    }
}

/// The bridge's one-time challenges for a claim: each one used once, within ten minutes.
pub struct Challenges {
    pub clock: Clock,
    pub items: Mutex<HashMap<String, f64>>,
}

impl Default for Challenges {
    fn default() -> Self {
        Challenges::new(crate::py::monotonic_clock())
    }
}

impl Challenges {
    pub fn new(clock: Clock) -> Self {
        Challenges { clock, items: Mutex::new(HashMap::new()) }
    }

    pub fn issue(&self) -> String {
        let now = (self.clock)();
        let mut open = self.items.lock().unwrap_or_else(|e| e.into_inner());
        open.retain(|_, t| now - *t < CHALLENGE_SECONDS);
        if open.len() > 64 {
            // never more than a handful in flight: the oldest go
            let mut by_age: Vec<(String, f64)> = open.iter().map(|(c, t)| (c.clone(), *t)).collect();
            by_age.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            for (old, _) in by_age.into_iter().take(32) {
                open.remove(&old);
            }
        }
        let challenge = crate::py::token_hex(16);
        open.insert(challenge.clone(), now);
        challenge
    }

    pub fn take(&self, challenge: &str) -> bool {
        let issued = self.items.lock().unwrap_or_else(|e| e.into_inner()).remove(challenge);
        match issued {
            Some(t) => (self.clock)() - t < CHALLENGE_SECONDS,
            None => false,
        }
    }
}

// -- the requests ------------------------------------------------------------------------------------

/// `ModelChoice`: custom is any OpenAI-compatible API (EUrouter…).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    pub provider: String,
    pub name: String,
    pub base_url: Option<String>,
    pub key: String,
    pub vision: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentChoice {
    pub name: String,
    pub kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimRequest {
    pub model: ModelChoice,
    pub agents: Vec<AgentChoice>,
    pub language: Option<String>,
    /// The phone's IANA time zone (« America/New_York »); one this bridge doesn't know is left out.
    pub timezone: Option<String>,
    /// The phone's region (« US »), upper-cased.
    pub country: Option<String>,
}

/// A str field's constraints, checked as pydantic does (type, then min, max, pattern).
struct Str {
    min: Option<usize>,
    max: Option<usize>,
    pattern: Option<&'static str>,
}

static PATTERNS: LazyLock<Mutex<HashMap<&'static str, Regex>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn matches(pattern: &'static str, value: &str) -> bool {
    let mut cache = PATTERNS.lock().unwrap_or_else(|e| e.into_inner());
    cache.entry(pattern).or_insert_with(|| Regex::new(pattern).expect("valid pattern")).is_match(value)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// pydantic's first error on a str field (JSON mode), as `exc.errors()[0]["msg"]`.
fn check_str(value: Option<&Value>, rules: &Str, optional: bool) -> Result<Option<String>, String> {
    let value = match value {
        None if optional => return Ok(None),
        None => return Err("Field required".into()),
        Some(Value::Null) if optional => return Ok(None),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err("Input should be a valid string".into()),
    };
    let n = value.chars().count();
    if let Some(min) = rules.min {
        if n < min {
            return Err(format!("String should have at least {min} character{}", plural(min)));
        }
    }
    if let Some(max) = rules.max {
        if n > max {
            return Err(format!("String should have at most {max} character{}", plural(max)));
        }
    }
    if let Some(pattern) = rules.pattern {
        if !matches(pattern, &value) {
            return Err(format!("String should match pattern '{pattern}'"));
        }
    }
    Ok(Some(value))
}

/// pydantic's lax bool (JSON mode).
fn check_bool(value: Option<&Value>, default: bool) -> Result<bool, String> {
    const UNABLE: &str = "Input should be a valid boolean, unable to interpret input";
    match value {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                match i {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => Err(UNABLE.into()),
                }
            } else {
                match n.as_f64() {
                    Some(0.0) => Ok(false),
                    Some(1.0) => Ok(true),
                    _ => Err("Input should be a valid boolean".into()),
                }
            }
        }
        Some(Value::String(s)) => match s.to_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err(UNABLE.into()),
        },
        Some(_) => Err("Input should be a valid boolean".into()),
    }
}

fn object(value: &Value) -> Result<&Dict, String> {
    value.as_object().ok_or_else(|| "Input should be an object".to_string())
}

impl ClaimRequest {
    /// `ClaimRequest.model_validate_json(text)`; Err: the first error's message.
    pub fn from_json(text: &str) -> Result<ClaimRequest, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("Invalid JSON: {e}"))?;
        Self::from_value(&value)
    }

    pub fn from_value(value: &Value) -> Result<ClaimRequest, String> {
        let root = object(value)?;
        let model = match root.get("model") {
            None => return Err("Field required".into()),
            Some(m) => object(m)?,
        };
        let provider = check_str(
            model.get("provider"),
            &Str { min: None, max: None, pattern: Some(r"^(openrouter|custom)$") },
            false,
        )?;
        let name = check_str(model.get("name"), &Str { min: Some(1), max: Some(200), pattern: None }, false)?;
        let base_url = check_str(
            model.get("base_url"),
            &Str { min: None, max: Some(300), pattern: Some(r"^https://[^\s]+$") },
            true,
        )?;
        let key =
            check_str(model.get("key"), &Str { min: Some(8), max: Some(400), pattern: Some(r"^[^\s]+$") }, false)?;
        let vision = check_bool(model.get("vision"), false)?;
        let mut agents = Vec::new();
        match root.get("agents") {
            None => {}
            Some(Value::Array(items)) => {
                for item in items {
                    let item = object(item)?;
                    let name = check_str(item.get("name"), &Str { min: Some(1), max: Some(40), pattern: None }, false)?;
                    let kind = check_str(item.get("kind"), &Str { min: None, max: Some(30), pattern: None }, true)?;
                    agents.push(AgentChoice { name: name.unwrap_or_default(), kind });
                }
                if items.len() > 8 {
                    return Err(format!("List should have at most 8 items after validation, not {}", items.len()));
                }
            }
            Some(_) => return Err("Input should be a valid array".into()),
        }
        let language = check_str(root.get("language"), &Str { min: None, max: Some(20), pattern: None }, true)?;
        let timezone = check_str(
            root.get("timezone"),
            &Str { min: Some(1), max: Some(64), pattern: Some(r"^[A-Za-z0-9_+\-]+(/[A-Za-z0-9_+\-]+)*$") },
            true,
        )?
        .filter(|zone| {
            // a zone newer than this bridge's database: the claim goes on, the server's zone stays
            let known = crate::dates::parse_zone(zone).is_some();
            if !known {
                tracing::warn!(zone = %zone, "claim: time zone unknown here, left out");
            }
            known
        });
        let country = check_str(root.get("country"), &Str { min: None, max: Some(2), pattern: Some(r"^[A-Za-z]{2}$") }, true)?
            .map(|c| c.to_uppercase());
        Ok(ClaimRequest {
            model: ModelChoice {
                provider: provider.unwrap_or_default(),
                name: name.unwrap_or_default(),
                base_url,
                key: key.unwrap_or_default(),
                vision,
            },
            agents,
            language,
            timezone,
            country,
        })
    }
}

/// `RestoreRequest.model_validate_json(text).restore.key`.
fn restore_key(asked: &Value) -> Result<String, String> {
    let restore = object(asked.get("restore").ok_or("Field required")?)?;
    Ok(check_str(
        restore.get("key"),
        &Str { min: Some(16), max: Some(200), pattern: Some(r"^[A-Za-z0-9_\-]+$") },
        false,
    )?
    .unwrap_or_default())
}

/// `ReopenRequest.model_validate_json(text).reopen.name`.
fn reopen_name(asked: &Value) -> Result<String, String> {
    let reopen = object(asked.get("reopen").ok_or("Field required")?)?;
    Ok(check_str(reopen.get("name"), &Str { min: Some(1), max: Some(60), pattern: None }, false)?.unwrap_or_default())
}

#[derive(Debug, Deserialize)]
pub struct Claim {
    pub challenge: String,
    pub mac: String,
    /// the JSON the mac covers, exactly as the app sent it
    pub request: String,
}

impl Validate for Claim {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        for (field, value, min, max) in
            [("challenge", &self.challenge, 16, 128), ("mac", &self.mac, 64, 64), ("request", &self.request, 2, 20_000)]
        {
            let before = errors.len();
            check_min_len(errors, field, Some(value), min);
            if errors.len() == before {
                check_max_len(errors, field, Some(value), max);
            }
        }
    }
}

/// The variable holding the model's key.
fn key_env(model: &ModelChoice) -> &'static str {
    MODEL_KEYS.iter().find(|(p, _)| *p == model.provider).map(|(_, k)| *k).unwrap_or("CUSTOM_MODEL_API_KEY")
}

fn model_base_url(model: &ModelChoice) -> Option<String> {
    model
        .base_url
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| (model.provider == "openrouter").then(|| "https://openrouter.ai/api/v1".to_string()))
}

/// The hub profile's model (every agent is cloned from it), as the installer sets it (hermes.set_model).
pub fn model_commands(config: &Config, model: &ModelChoice) -> Vec<Vec<String>> {
    // the hub is the default profile: no -p
    let setting = |key: &str, value: &str| {
        vec![config.hermes.bin.clone(), "config".into(), "set".into(), key.to_string(), value.to_string()]
    };
    let key_env = key_env(model);
    let mut commands = vec![setting("model.provider", &model.provider), setting("model.default", &model.name)];
    if let Some(url) = model_base_url(model) {
        commands.push(setting("model.base_url", &url));
    }
    if model.provider == "custom" {
        commands.push(setting("model.key_env", key_env));
    }
    commands.push(setting(key_env, &model.key));
    commands.push(setting("model.supports_vision", if model.vision { "true" } else { "false" }));
    commands
}

/// « Model unreachable » in the app: the bridge checks the model's /models with its key (`[model]`).
pub fn write_model_health(config: &SharedConfig, base_url: &str, key: &str) -> anyhow::Result<()> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    config.update(|c| {
        c.model_health_url = Some(url.clone());
        c.model_health_key = Some(key.to_string());
    });
    let Some(source) = config.get().source_path.clone().filter(|p| !p.is_empty()) else { return Ok(()) };
    std::fs::metadata(&source)?;
    crate::config::update_source(&source, |text| {
        let mut inside = false;
        let mut kept: Vec<String> = Vec::new();
        for line in crate::hermes::splitlines(text) {
            let stripped = pyx::strip(&line);
            if stripped.starts_with('[') && stripped.ends_with(']') {
                inside = stripped == "[model]";
            }
            if !inside {
                kept.push(line.clone());
            }
        }
        Ok(format!(
            "{}\n\n[model]\nhealth_url = {}\nhealth_key = {}\n",
            kept.join("\n").trim_end_matches('\n'),
            crate::py::dumps(&json!(url)),
            crate::py::dumps(&json!(key))
        ))
    })?;
    Ok(())
}

/// What a claim already did, kept with the secret: a claim that failed half-way is tried again without redoing it.
fn remember(config: &Config, values: &[(&str, Value)]) -> anyhow::Result<()> {
    let mut data = read_json(config.setup_file.as_deref());
    for (k, v) in values {
        data.insert(k.to_string(), v.clone());
    }
    let path = PathBuf::from(config.setup_file.clone().unwrap_or_default());
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), crate::py::dumps(&Value::Object(data)).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The person's language, time zone and country, from the claim, made the place's (before its agents are named):
/// - the Carnet's language, while it has no page (a place's fallback for every text and later agent: notebook.rs);
/// - the phone's zone as the person's (dates/: « Fuseau horaire automatique »), written in the hub's Hermes config
///   (agents made later are cloned from it) and every profile's (`timezone`, `cron.timezone`);
/// - the phone's country, when it is one the offer is sold in (holidays, and the agents' bip:now line).
///
/// Wife's place, 2026-10-30: the claim's « en » named the agents, but nothing else of the place knew it.
pub fn apply_locale(config: &Config, request: &ClaimRequest) -> anyhow::Result<()> {
    if let Some(lang) = request.language.as_deref().filter(|l| !l.is_empty()) {
        let code = crate::souls::agent_language(Some(lang));
        let known = crate::notebook::WORDS.iter().any(|w| w.lang == code);
        let _guard = crate::notebook::lock();
        let empty = crate::notebook::all_pages(config, crate::notebook::Part::Main).is_empty()
            && crate::notebook::all_pages(config, crate::notebook::Part::Health).is_empty();
        if known && empty {
            crate::notebook::update_state(config, |s| {
                s.insert("language".into(), json!(code));
            })?;
        }
    }
    let zone = request.timezone.clone().filter(|z| crate::dates::parse_zone(z).is_some());
    let country =
        request.country.clone().filter(|c| crate::dates::words::country_name(c, "en").is_some());
    if zone.is_none() && country.is_none() {
        return Ok(()); // an older app: the server's zone until the app reports the phone's
    }
    let person = crate::dates::change(config, |p| {
        if let Some(zone) = &zone {
            p.device_zone(zone, false);
        }
        if let Some(country) = &country {
            if *country != p.country {
                p.region.clear();
                p.school_zone.clear();
            }
            p.country = country.clone();
        }
    })?;
    if crate::dates::parse_zone(&person.time_zone).is_some() {
        let hub = PathBuf::from(crate::agents_admin::install_home(config)).join("config.yaml");
        crate::dates::write_profile(&hub, &person.zone_name(), &person.routines_zone())?;
        crate::dates::write_profiles(config, &person);
    }
    tracing::info!(zone = %person.zone_name(), country = %person.country, "claim: the person's time zone and country");
    Ok(())
}

/// The model set, the gateway restarted to read its key, the agents created. Returns the agents (id, name).
pub async fn claim_server(
    config: &SharedConfig,
    request: &ClaimRequest,
    http: &reqwest::Client,
    run: Option<Run>,
) -> anyhow::Result<Vec<Value>> {
    use crate::agents_admin::{create_agent, default_run, NewAgent};
    let run = run.unwrap_or_else(default_run);
    let _preparing = PREPARING.lock().await; // a first agent being prepared is waited for
    if let Err(exc) = apply_locale(&config.get(), request) {
        tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "claim: language, time zone or country not kept");
    }
    if let Some(prepared) = prepared_of(&config.get()).filter(|_| !request.agents.is_empty()) {
        return claim_prepared(config, request, http, run, &prepared).await;
    }
    let snapshot = config.get();
    let mut env = BTreeMap::new();
    if !snapshot.hermes.home.is_empty() {
        env.insert("HERMES_HOME".to_string(), snapshot.hermes.home.clone());
    }
    let state = read_json(snapshot.setup_file.as_deref());
    if !state.get("model_set").map(crate::py::truthy).unwrap_or(false) {
        for command in model_commands(&snapshot, &request.model) {
            run(command, env.clone()).await?;
        }
        // the providers EUrouter tries first (model_route.rs): in the hub's config, which every agent is cloned from
        let hub_config = PathBuf::from(crate::agents_admin::install_home(&snapshot)).join("config.yaml");
        crate::model_route::write(&hub_config, &snapshot.model_provider_order)?;
        run(vec![snapshot.hermes.bin.clone(), "gateway".into(), "restart".into()], env.clone()).await?;
        let base_url = request
            .model
            .base_url
            .clone()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| "https://openrouter.ai/api/v1".into());
        write_model_health(config, &base_url, &request.model.key)?;
        remember(&config.get(), &[("model_set", json!(true))])?;
    }
    // the app's index -> the profile made for it
    let mut created: Dict = match state.get("created") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    for (index, choice) in request.agents.iter().enumerate() {
        if created.contains_key(&index.to_string()) {
            continue;
        }
        let kind = choice.kind.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| "daily".into());
        let name = if index == 0 { first_agent_name(&choice.name) } else { choice.name.clone() };
        let body = NewAgent {
            name,
            kind: Some(kind.clone()),
            lang: request.language.clone().filter(|l| !l.is_empty()),
            custom: None,
            group: None,
            ..Default::default()
        };
        let agent = create_agent(config, &body, http, &run).await?;
        let lang = request.language.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| crate::notebook::place_language(&config.get()).unwrap_or_default());
        if index == 0 {
            become_guide(config, &agent.name, &lang)?;
        } else {
            crate::welcome::wait_for(&config.get(), &agent, &kind, &lang);
        }
        created.insert(index.to_string(), json!(agent.name));
        remember(&config.get(), &[("created", Value::Object(created.clone()))])?;
    }
    let now = config.get();
    Ok((0..request.agents.len())
        .filter_map(|i| {
            let id = created.get(&i.to_string()).and_then(Value::as_str)?;
            let agent = now.agents.get(id)?;
            Some(json!({"id": id, "name": agent.display_name}))
        })
        .collect())
}

// -- a place prepared before its claim (Rust only) ---------------------------------------------------
//
// A place of the reserve waits for its app with its first agent already made (its profile, key, uploads, tools and
// browser settings: the 40-odd seconds of `hermes` commands). The claim then only writes files (the model and its
// key, the agent's name, personality and language), answers the app, and restarts the gateway once, directly; the
// app's requests through the door wait meanwhile (`READINESS`). Other agents asked for are listed at once and made
// right after, in the background (`claim-pending.json` next to the secret, finished at the next start if need be).

/// The name of the agent made before the claim: « Bip », the guide that is always there (Alex, 2026-10-30;
/// internal/next/bip-default-agent.md). Its profile id stays neutral (`PREPARED_ID`): a claim may rename it.
pub const PREPARED_NAME: &str = crate::guide::GUIDE_NAME;
pub const PREPARED_ID: &str = "agent";
pub const PREPARED_KIND: &str = "daily";
/// The name the apps send for the place's first agent (OfferFlow.firstAgents, Android FIRST_AGENT), and the one
/// places prepared before 2026-10-30 still carry: it means « Bip » at the claim, and it marks an agent still
/// waiting for the first choice on an older place (guide.rs).
pub const PLACEHOLDER_NAME: &str = "Agent";

/// The name the place's first agent gets at the claim: the app's, or « Bip » for the placeholder (or none).
pub fn first_agent_name(asked: &str) -> String {
    let asked = asked.trim();
    if asked.is_empty() || asked.eq_ignore_ascii_case(PLACEHOLDER_NAME) || asked.eq_ignore_ascii_case(PREPARED_NAME) {
        PREPARED_NAME.to_string()
    } else {
        asked.to_string()
    }
}
/// How long a request through the door waits for an agent the claim is still making ready.
pub const READY_WAIT: std::time::Duration = std::time::Duration::from_secs(45);

/// One preparation or claim at a time: a claim arriving while the place prepares its first agent waits for it.
static PREPARING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// What is not ready yet after a claim: the gateway restarting (`GATEWAY`), agents still being made.
#[derive(Default)]
pub struct Readiness {
    waiting: Mutex<std::collections::HashSet<String>>,
    changed: tokio::sync::Notify,
}

/// The hub's gateway, in `Readiness`.
pub const GATEWAY: &str = "";

pub static READINESS: LazyLock<Readiness> = LazyLock::new(Readiness::default);

impl Readiness {
    pub fn hold(&self, name: &str) {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(name.to_string());
    }

    pub fn release(&self, name: &str) {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        self.changed.notify_waiters();
    }

    fn blocked(&self, agent: &str) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(|e| e.into_inner());
        waiting.contains(GATEWAY) || waiting.contains(agent)
    }

    /// Returns once the agent can be reached (or after `limit`: the request then goes, and fails as it would).
    pub async fn wait(&self, agent: &str, limit: std::time::Duration) {
        let deadline = tokio::time::Instant::now() + limit;
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.blocked(agent) {
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return;
            }
        }
    }
}

/// The agent made before the claim, when there is one.
pub fn prepared_of(config: &Config) -> Option<String> {
    let id = read_json(config.setup_file.as_deref()).get("prepared")?.as_str()?.to_string();
    config.agents.contains_key(&id).then_some(id)
}

/// A place of the reserve that can be prepared: one the app has not claimed, with nothing done for a claim yet.
fn preparable(config: &Config) -> bool {
    let state = read_json(config.setup_file.as_deref());
    config.cloud
        && config.hermes.enabled()
        && config.agents.is_empty()
        && secret_of(config).is_some()
        && !restoring(config)
        && !state.contains_key("model_set")
        && !state.contains_key("created")
}

/// The place's first agent, made before the claim. Returns its id (None: nothing to prepare).
pub async fn prepare_place(
    config: &SharedConfig,
    http: &reqwest::Client,
    run: Option<Run>,
) -> anyhow::Result<Option<String>> {
    use crate::agents_admin::{hub_agent, install_home, make_profile, plan_agent, NewAgent};
    let run = run.unwrap_or_else(crate::agents_admin::default_run);
    let _preparing = PREPARING.lock().await;
    let snapshot = config.get();
    if let Some(id) = prepared_of(&snapshot) {
        return Ok(Some(id));
    }
    if !preparable(&snapshot) {
        return Ok(None);
    }
    let started = crate::py::monotonic();
    let body = NewAgent {
        name: PREPARED_NAME.into(),
        kind: Some(PREPARED_KIND.into()),
        lang: None,
        custom: None,
        group: None,
        ..Default::default()
    };
    let mut planned = plan_agent(&snapshot, &body)?;
    // a profile left by a preparation cut short (the bridge restarted by the installer): another id
    let profiles = PathBuf::from(install_home(&snapshot)).join("profiles");
    let mut taken: Vec<String> = snapshot.agents.keys().cloned().collect();
    if let Ok(entries) = std::fs::read_dir(&profiles) {
        taken.extend(entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()));
    }
    planned.profile = crate::souls::profile_name(PREPARED_ID, &taken);
    let agent = hub_agent(&snapshot, &planned.profile, &planned.name, &crate::py::token_urlsafe(32));
    let agent = make_profile(config, &planned, agent, http, &run, false).await?;
    if let Err(exc) = warm_browser(&config.get(), &agent.name, &run).await {
        tracing::warn!(agent = %agent.name, error = %pyx::tail(&format!("{exc:#}"), 300), "browser not warmed up");
    }
    remember(&config.get(), &[("prepared", json!(agent.name))])?;
    tracing::info!(agent = %agent.name, seconds = format!("{:.1}", crate::py::monotonic() - started), "place prepared for its claim");
    Ok(Some(agent.name))
}

/// Before the claim: the prepared agent's browser started once through its bip-browser-mcp and asked what it serves.
/// The claim's gateway restart then finds those answers cached (nothing starts for Hermes to connect), and the
/// account's first container (rootless podman sets its storage up then: ~40 s measured on a loaded server) is made
/// while no one waits. The browser is stopped after.
async fn warm_browser(config: &Config, agent: &str, run: &Run) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let settings = crate::browsers::folder(config, agent).join("mcp.json");
    if !config.browser.enabled || !settings.is_file() {
        return Ok(());
    }
    let started = crate::py::monotonic();
    let mut child = tokio::process::Command::new(crate::browsers::proxy_binary())
        .arg(&settings)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
    let mut lines = BufReader::new(child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?).lines();
    let mut messages = vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18",
               "capabilities": {}, "clientInfo": {"name": "bipbridge-warm-up", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    for (id, method) in crate::browser_mcp::LISTS.iter().enumerate() {
        messages.push(json!({"jsonrpc": "2.0", "id": id + 2, "method": method}));
    }
    for message in &messages {
        stdin.write_all(format!("{message}\n").as_bytes()).await?;
    }
    stdin.flush().await?;
    let wanted = crate::browser_mcp::LISTS.len() + 1;
    let answered = tokio::time::timeout(std::time::Duration::from_secs(240), async {
        let mut seen = 0;
        while let Ok(Some(line)) = lines.next_line().await {
            if serde_json::from_str::<Value>(&line).ok().is_some_and(|v| v.get("id").is_some_and(|i| !i.is_null())) {
                seen += 1;
                if seen == wanted {
                    break;
                }
            }
        }
        seen
    })
    .await
    .unwrap_or(0);
    drop(stdin); // its Hermes gone: the helper stops its server and leaves
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), child.wait()).await;
    let _ = run(
        vec!["systemctl".into(), "--user".into(), "stop".into(), crate::browsers::container(agent)],
        BTreeMap::new(),
    )
    .await;
    tracing::info!(agent = %agent, answers = answered, seconds = format!("{:.1}", crate::py::monotonic() - started),
                   "browser warmed up for the claim");
    Ok(())
}

/// At start: once the installer is done, a place of the reserve prepares its first agent.
pub async fn prepare_when_installed(config: SharedConfig, http: reqwest::Client) {
    loop {
        let snapshot = config.get();
        if !preparable(&snapshot) || prepared_of(&snapshot).is_some() {
            return;
        }
        match progress_of(&snapshot) {
            Ok(step) if step.done => break,
            Ok(step) if step.error.is_some() => return,
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    if let Err(exc) = prepare_place(&config, &http, None).await {
        tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "place not prepared: its claim makes its agents");
    }
}

/// The model in a Hermes home or profile folder, as `hermes config set` writes it: `model` in config.yaml, the key in
/// `.env`; with the providers EUrouter tries first (`order`, model_route.rs).
pub fn write_model_files(folder: &Path, model: &ModelChoice, order: &[String]) -> anyhow::Result<()> {
    use crate::agents_admin::pyyaml::{self, Yaml};
    let path = folder.join("config.yaml");
    let mut data = if path.exists() { crate::agents_admin::load_or_empty(&path)? } else { Yaml::map() };
    if !data.is_map() {
        data = Yaml::map();
    }
    let mut section = match data.get("model") {
        Some(m) if m.is_map() => m.clone(),
        _ => Yaml::map(),
    };
    section.insert("provider", Yaml::str(&model.provider));
    section.insert("default", Yaml::str(&model.name));
    if let Some(url) = model_base_url(model) {
        section.insert("base_url", Yaml::str(url));
    }
    if model.provider == "custom" {
        section.insert("key_env", Yaml::str(key_env(model)));
    }
    section.insert("supports_vision", Yaml::Bool(model.vision));
    data.insert("model", section);
    let chosen = crate::agents_admin::chosen_thinking(folder);
    if let Some(routed) = crate::model_route::with_routing(&data, order, chosen.as_deref()) {
        data = routed;
    }
    if path.exists() {
        crate::agents_admin::replace_yaml(&path, &data)?;
    } else {
        std::fs::create_dir_all(folder)?;
        std::fs::write(&path, pyyaml::dump(&data))?;
    }
    set_env(&folder.join(".env"), key_env(model), &model.key)
}

/// `KEY=value` in a `.env` file (replaced, or added), the file only its owner reads.
pub fn set_env(path: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = Vec::new();
    let mut found = false;
    for line in text.lines() {
        let name = line.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
        let name = name.strip_prefix("export ").map(str::trim).unwrap_or(name);
        if name == key && !line.trim_start().starts_with('#') {
            if !found {
                lines.push(format!("{key}={value}"));
            }
            found = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !found {
        lines.push(format!("{key}={value}"));
    }
    let tmp = path.with_extension("bip-tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), (lines.join("\n") + "\n").as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The hub's gateway restarted directly, by its systemd unit: `hermes gateway restart` costs a Python start and
/// systemd's RestartSec on top. Hermes' own CLI when no unit of it is found.
pub fn restart_command(config: &Config, units: &Path) -> Vec<String> {
    for unit in crate::sleep::UNITS {
        if units.join(format!("{unit}.service")).exists() {
            return ["systemctl", "--user", "restart", unit].iter().map(|s| s.to_string()).collect();
        }
    }
    vec![config.hermes.bin.clone(), "gateway".into(), "restart".into()]
}

fn pending_path(config: &Config) -> PathBuf {
    setup_folder(config).join("claim-pending.json")
}

fn read_pending(config: &Config) -> Vec<Value> {
    let path = pending_path(config);
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

fn write_pending(config: &Config, items: &[Value]) -> anyhow::Result<()> {
    let path = pending_path(config);
    if items.is_empty() {
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    crate::py::write_with_mode(&path.to_string_lossy(), crate::py::dumps(&json!(items)).as_bytes(), 0o600)?;
    Ok(())
}

/// The prepared agent becomes the person's first one: their name, its kind's personality, their language.
fn rename_prepared(config: &SharedConfig, prepared: &str, choice: &AgentChoice, lang: &str) -> anyhow::Result<()> {
    use crate::agents_admin::pyyaml::Yaml;
    let snapshot = config.get();
    let mut agent = snapshot.agents.get(prepared).cloned().ok_or_else(|| anyhow::anyhow!("no agent {prepared}"))?;
    let name = &first_agent_name(&choice.name);
    let kind = choice.kind.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| PREPARED_KIND.into());
    let (soul, kind) = match crate::souls::soul_for(name, &kind, Some(lang)) {
        Ok(soul) => (soul, kind),
        Err(_) => (crate::souls::soul_for(name, PREPARED_KIND, Some(lang))?, PREPARED_KIND.to_string()),
    };
    let folder = crate::agents_admin::profile_folder(&snapshot, &agent);
    std::fs::write(folder.join("SOUL.md"), soul)?;
    // what `hermes profile create --description` wrote
    let description = crate::souls::template(&kind, Some(lang))
        .and_then(|t| t.get("description").map(crate::py::py_str))
        .unwrap_or_default();
    let meta = folder.join("profile.yaml");
    if meta.exists() && !description.is_empty() {
        let mut data = crate::agents_admin::load_or_empty(&meta)?;
        if data.is_map() {
            data.insert("description", Yaml::str(description));
            crate::agents_admin::replace_yaml(&meta, &data)?;
        }
    }
    agent.display_name = name.to_string();
    crate::agents_admin::save_agent(config, &agent)
}

/// The place's first agent becomes the guide (whatever name the app gave it), its blocks written in its personality;
/// its first conversation opens with its welcome.
fn become_guide(config: &SharedConfig, id: &str, lang: &str) -> anyhow::Result<()> {
    let snapshot = config.get();
    let agent = snapshot.agents.get(id).ok_or_else(|| anyhow::anyhow!("no agent {id}"))?;
    crate::guide::guide_from_claim(&snapshot, agent, lang)?;
    crate::guide::sync_soul(&snapshot, agent);
    Ok(())
}

/// The claim of a prepared place: files only, then the gateway restarted and the other agents made after the answer.
async fn claim_prepared(
    config: &SharedConfig,
    request: &ClaimRequest,
    http: &reqwest::Client,
    run: Run,
    prepared: &str,
) -> anyhow::Result<Vec<Value>> {
    use crate::agents_admin::{hub_agent, install_home, plan_agent, reserve_agent, NewAgent};
    let started = crate::py::monotonic();
    let snapshot = config.get();
    let state = read_json(snapshot.setup_file.as_deref());
    let lang =
        request.language.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| crate::notebook::place_language(&snapshot).unwrap_or_else(|| "fr".into()));
    if !state.get("model_set").map(crate::py::truthy).unwrap_or(false) {
        let home = PathBuf::from(install_home(&snapshot));
        let order = &snapshot.model_provider_order;
        write_model_files(&home, &request.model, order)?; // the hub: every agent made later is cloned from it
        write_model_files(&home.join("profiles").join(prepared), &request.model, order)?; // cloned before
        let base_url = model_base_url(&request.model).unwrap_or_else(|| "https://openrouter.ai/api/v1".into());
        write_model_health(config, &base_url, &request.model.key)?;
        remember(&config.get(), &[("model_set", json!(true))])?;
    }
    let mut created: Dict = match state.get("created") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    if !created.contains_key("0") {
        rename_prepared(config, prepared, &request.agents[0], &lang)?;
        become_guide(config, prepared, &lang)?;
        created.insert("0".into(), json!(prepared));
        remember(&config.get(), &[("created", Value::Object(created.clone()))])?;
    }
    let mut pending = read_pending(&config.get());
    for (index, choice) in request.agents.iter().enumerate().skip(1) {
        if created.contains_key(&index.to_string()) {
            continue;
        }
        let kind = choice.kind.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| "daily".into());
        let body = NewAgent {
            name: choice.name.clone(),
            kind: Some(kind.clone()),
            lang: Some(lang.clone()),
            custom: None,
            group: None,
            ..Default::default()
        };
        crate::agents_admin::check_room(&config.get())?;
        let planned = plan_agent(&config.get(), &body)?;
        let agent = hub_agent(&config.get(), &planned.profile, &planned.name, &crate::py::token_urlsafe(32));
        reserve_agent(config, &agent)?;
        crate::welcome::wait_for(&config.get(), &agent, &kind, &lang);
        pending.push(json!({"id": planned.profile, "name": planned.name, "kind": kind, "lang": lang}));
        write_pending(&config.get(), &pending)?;
        created.insert(index.to_string(), json!(planned.profile));
        remember(&config.get(), &[("created", Value::Object(created.clone()))])?;
    }
    READINESS.hold(GATEWAY);
    for entry in &pending {
        READINESS.hold(entry["id"].as_str().unwrap_or(""));
    }
    let (c, h, id) = (config.clone(), http.clone(), prepared.to_string());
    tokio::spawn(async move { after_claim(c, h, run, id).await });
    tracing::info!(seconds = format!("{:.1}", crate::py::monotonic() - started), "claim written");
    let now = config.get();
    Ok((0..request.agents.len())
        .filter_map(|i| {
            let id = created.get(&i.to_string()).and_then(Value::as_str)?;
            let agent = now.agents.get(id)?;
            Some(json!({"id": id, "name": agent.display_name}))
        })
        .collect())
}

/// After the claim's answer: the gateway restarted once to read the model and its key, then the other agents made.
async fn after_claim(config: SharedConfig, http: reqwest::Client, run: Run, prepared: String) {
    let started = crate::py::monotonic();
    let snapshot = config.get();
    let mut env = BTreeMap::new();
    if !snapshot.hermes.home.is_empty() {
        env.insert("HERMES_HOME".to_string(), snapshot.hermes.home.clone());
    }
    let units = pyx::home().join(".config").join("systemd").join("user");
    if let Err(exc) = run(restart_command(&snapshot, &units), env).await {
        tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "gateway not restarted after the claim");
    }
    if let Some(agent) = snapshot.agents.get(&prepared) {
        match crate::agents_admin::wait_served(&http, agent, false).await {
            Ok(()) => {
                tracing::info!(agent = %prepared, seconds = format!("{:.1}", crate::py::monotonic() - started), "agent ready after the claim")
            }
            Err(exc) => tracing::warn!(agent = %prepared, error = %exc, "agent not ready after the claim"),
        }
    }
    READINESS.release(GATEWAY);
    finish_pending(&config, &http, &run).await;
}

/// A recovered claim that brought another AI key (the guichet deleted the first one when it made this one): the
/// model written again in the hub and in every agent's profile, the health check's key, the gateway restarted.
pub async fn rekey_model(config: &SharedConfig, model: &ModelChoice, run: Run) -> anyhow::Result<()> {
    let snapshot = config.get();
    let home = PathBuf::from(crate::agents_admin::install_home(&snapshot));
    let order = &snapshot.model_provider_order;
    write_model_files(&home, model, order)?;
    for agent in snapshot.agents.values() {
        let folder = crate::agents_admin::profile_folder(&snapshot, agent);
        if folder != home && folder.is_dir() {
            write_model_files(&folder, model, order)?;
        }
    }
    let base_url = model_base_url(model).unwrap_or_else(|| "https://openrouter.ai/api/v1".into());
    write_model_health(config, &base_url, &model.key)?;
    let mut env = BTreeMap::new();
    if !snapshot.hermes.home.is_empty() {
        env.insert("HERMES_HOME".to_string(), snapshot.hermes.home.clone());
    }
    let units = pyx::home().join(".config").join("systemd").join("user");
    run(restart_command(&snapshot, &units), env).await?;
    Ok(())
}

/// The agents a claim listed before making them: their profiles made now (after the claim, or at the next start).
pub async fn finish_pending(config: &SharedConfig, http: &reqwest::Client, run: &Run) {
    use crate::agents_admin::{make_profile, plan_agent, NewAgent};
    let mut left = read_pending(&config.get());
    for entry in left.clone() {
        let id = entry["id"].as_str().unwrap_or("").to_string();
        let started = crate::py::monotonic();
        let result: anyhow::Result<()> = async {
            let Some(agent) = config.get().agents.get(&id).cloned() else { return Ok(()) }; // removed since
            let body = NewAgent {
                name: entry["name"].as_str().unwrap_or(&id).to_string(),
                kind: entry["kind"].as_str().map(str::to_string),
                lang: entry["lang"].as_str().map(str::to_string),
                custom: None,
                group: None,
                ..Default::default()
            };
            let mut planned = plan_agent(&config.get(), &body)?;
            planned.profile = id.clone();
            make_profile(config, &planned, agent, http, run, true).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                left.retain(|e| e["id"].as_str() != Some(id.as_str()));
                if let Err(exc) = write_pending(&config.get(), &left) {
                    tracing::warn!(error = %exc, "claim-pending.json not written");
                }
                tracing::info!(agent = %id, seconds = format!("{:.1}", crate::py::monotonic() - started), "agent made after the claim");
            }
            Err(exc) => {
                tracing::warn!(agent = %id, error = %pyx::tail(&format!("{exc:#}"), 300), "agent not made after the claim: tried again at the next start")
            }
        }
        READINESS.release(&id);
    }
}

/// At start: what a claim left to do (the bridge stopped before its agents were made).
pub fn has_pending(config: &Config) -> bool {
    config.setup_file.as_deref().is_some_and(|f| !f.is_empty()) && !read_pending(config).is_empty()
}

// -- routes ------------------------------------------------------------------------------------------

/// What the routes call to claim a server and to start a restore (tests give their own, as Python's monkeypatch).
pub type ClaimFn = Arc<dyn Fn(AppState, ClaimRequest) -> BoxFuture<'static, anyhow::Result<Vec<Value>>> + Send + Sync>;
pub type RestoreFn = Arc<dyn Fn(&Config, &str) -> std::io::Result<()> + Send + Sync>;
pub type RekeyFn = Arc<dyn Fn(AppState, ModelChoice) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>;

#[derive(Clone)]
pub struct Hooks {
    pub claim: ClaimFn,
    pub restore: RestoreFn,
    /// A recovered claim's new AI key written ([`rekey_model`]).
    pub rekey: RekeyFn,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            claim: Arc::new(|state: AppState, request: ClaimRequest| {
                Box::pin(async move { claim_server(&state.config, &request, &state.http, None).await })
            }),
            restore: Arc::new(start_restore),
            rekey: Arc::new(|state: AppState, model: ModelChoice| {
                Box::pin(async move { rekey_model(&state.config, &model, crate::agents_admin::default_run()).await })
            }),
        }
    }
}

struct Shared {
    hooks: Hooks,
    challenges: Challenges,
    lock: tokio::sync::Mutex<()>,
}

fn guarded(state: &AppState, address: Option<&str>) -> ApiResult<Arc<Config>> {
    let config = state.config();
    if config.setup_file.as_deref().map(str::is_empty).unwrap_or(true) {
        return Err(HttpError::new(404, "not found"));
    }
    if state.guard.blocked(address) {
        return Err(HttpError::new(429, "too many wrong proofs, try again later"));
    }
    Ok(config)
}

/// `nonce: str = Query(..., min_length=32, max_length=128, pattern=r"^[0-9a-fA-F]+$")`.
pub fn nonce_query(q: &QueryMap) -> ApiResult<String> {
    let nonce = web::query_str(q, "nonce", true, Some(128))?.unwrap_or_default();
    let loc = vec![json!("query"), json!("nonce")];
    if nonce.chars().count() < 32 {
        return Err(HttpError::validation(vec![FieldError::new(
            "string_too_short",
            loc,
            "String should have at least 32 characters",
            json!(nonce),
        )
        .with_ctx(json!({"min_length": 32}))]));
    }
    if !matches(r"^[0-9a-fA-F]+$", &nonce) {
        return Err(HttpError::validation(vec![FieldError::new(
            "string_pattern_mismatch",
            loc,
            "String should match pattern '^[0-9a-fA-F]+$'",
            json!(nonce),
        )
        .with_ctx(json!({"pattern": "^[0-9a-fA-F]+$"}))]));
    }
    Ok(nonce)
}

async fn setup_state(
    shared: Arc<Shared>,
    state: AppState,
    address: Option<String>,
    q: QueryMap,
) -> ApiResult<Response> {
    let nonce = nonce_query(&q)?;
    let config = guarded(&state, address.as_deref())?;
    let Some(secret) = secret_of(&config) else {
        let Some((secret, _)) = recovery_of(&config) else {
            return Ok(web::ok(json!({"state": "claimed"})));
        };
        // claimed, but its answer may have been lost: the claimer may claim again (« a claim whose answer was lost »)
        let print_fp = crate::lan::fingerprint(&config).unwrap_or_default();
        let step = progress_of(&config)?;
        let challenge = shared.challenges.issue();
        return Ok(web::ok(json!({
            "state": "ready", "step": step.step, "total": step.total, "label": step.label,
            "fingerprint": print_fp, "challenge": challenge,
            "proof": proof(&secret, &[SETUP_CONTEXT, &nonce.to_lowercase(), &print_fp, &challenge]),
            "prepared": true, "claimed": true,
        })));
    };
    let print_fp = crate::lan::fingerprint(&config).unwrap_or_default();
    let step = progress_of(&config)?;
    let challenge = shared.challenges.issue();
    let status = if restoring(&config) {
        "restoring"
    } else if step.done || step.error.is_some() {
        "ready"
    } else {
        "installing"
    };
    let mut out = json!({
        "state": status, "step": step.step, "total": step.total, "label": step.label,
        "fingerprint": print_fp, "challenge": challenge,
        "proof": proof(&secret, &[SETUP_CONTEXT, &nonce.to_lowercase(), &print_fp, &challenge]),
        // Rust only: its first agent made, the claim is a matter of seconds
        "prepared": prepared_of(&config).is_some(),
    });
    if let Some(error) = step.error {
        out["error"] = error; // a restore that failed (a wrong key): the app may claim again
    }
    if let Some(held) = crate::place_cert::held_for(&config) {
        out["host"] = json!(held.host); // its bipagents.com name, once its certificate came (place_cert.rs)
    }
    Ok(web::ok(out))
}

async fn claim(shared: Arc<Shared>, state: AppState, address: Option<String>, body: Claim) -> ApiResult<Response> {
    let config = guarded(&state, address.as_deref())?;
    let agents = {
        let _lock = shared.lock.lock().await;
        let Some(secret) = secret_of(&config) else {
            return recover(&shared, &state, &config, address.as_deref(), &body).await;
        };
        if !shared.challenges.take(&body.challenge) {
            return Err(HttpError::new(400, "challenge unknown or expired: ask /v1/setup again"));
        }
        let expected = proof(&secret, &[CLAIM_CONTEXT, &body.challenge, &body.request]);
        match pyx::compare_digest(&expected, &body.mac.to_lowercase()) {
            None => return Err(HttpError::internal()),
            Some(false) => {
                state.guard.fail(address.as_deref(), "pairing proof");
                return Err(HttpError::new(401, "wrong pairing proof"));
            }
            Some(true) => {}
        }
        let step = progress_of(&config)?;
        if !step.done && step.error.is_none() {
            return Err(HttpError::new(409, "still installing"));
        }
        if restoring(&config) {
            return Err(HttpError::new(409, "already restoring"));
        }
        let Ok(asked) = serde_json::from_str::<Value>(&body.request) else {
            return Err(HttpError::new(400, "invalid request"));
        };
        let setup_file = config.setup_file.clone().unwrap_or_default();
        if asked.as_object().map(|o| o.contains_key("reopen")).unwrap_or(false) {
            let Ok(name) = reopen_name(&asked) else {
                return Err(HttpError::new(400, "invalid reopen request"));
            };
            let (s, n) = (state.clone(), name.clone());
            let key = tokio::task::spawn_blocking(move || reopen(&s, &n)).await.map_err(anyhow::Error::from)??;
            let _ = std::fs::remove_file(&setup_file);
            let _ = std::fs::remove_file(recovery_path(&config)); // an earlier claim's: nothing to recover any more
            tracing::info!(address = ?J(&address), "pairing reopened");
            let now = state.config();
            let agents: Vec<Value> =
                now.agents.values().map(|a| json!({"id": a.name, "name": a.display_name})).collect();
            return Ok(web::ok(
                json!({"bridgeURL": now.public_url, "bridgeKey": key, "fingerprint": crate::lan::fingerprint(&now),
                                      "agents": agents, "reopened": true}),
            ));
        }
        if asked.as_object().map(|o| o.contains_key("restore")).unwrap_or(false) {
            let Ok(key) = restore_key(&asked) else {
                return Err(HttpError::new(400, "invalid restore request"));
            };
            let marker = setup_folder(&config).join("restoring.json");
            std::fs::write(&marker, crate::py::dumps(&json!({"since": crate::py::time_time()})))
                .map_err(anyhow::Error::from)?;
            let (restore, c) = (shared.hooks.restore.clone(), config.clone());
            let started = tokio::task::spawn_blocking(move || restore(&c, &key)).await.map_err(anyhow::Error::from)?;
            if started.is_err() {
                let _ = std::fs::remove_file(&marker);
                return Err(HttpError::new(502, "the restore could not start: try again"));
            }
            tracing::info!(address = ?J(&address), "restore started");
            return Ok(web::ok(
                json!({"restoring": true, "bridgeURL": config.public_url, "fingerprint": crate::lan::fingerprint(&config)}),
            ));
        }
        let wanted = match ClaimRequest::from_value(&asked) {
            Ok(w) => w,
            Err(msg) => return Err(HttpError::new(400, format!("invalid request: {msg}"))),
        };
        if wanted.model.provider == "custom" && wanted.model.base_url.as_deref().map(str::is_empty).unwrap_or(true) {
            return Err(HttpError::new(400, "a custom model needs its base_url"));
        }
        let claim_id = match claim_id_of(&asked) {
            Ok(id) => id,
            Err(msg) => return Err(HttpError::new(400, format!("invalid request: {msg}"))),
        };
        let model_key = wanted.model.key.clone();
        let agents = match (shared.hooks.claim)(state.clone(), wanted).await {
            Ok(a) => a,
            Err(exc) => {
                tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "claim failed");
                return Err(HttpError::new(502, "the setup could not finish: try again"));
            }
        };
        if let Err(exc) = write_recovery(&state.config(), claim_id.as_deref(), &model_key, &agents) {
            tracing::warn!(error = %exc, "claim-recovery.json not written: a lost answer can't be recovered");
        }
        let _ = std::fs::remove_file(&setup_file); // paired: the secret is gone (but for its claimer's recovery)
        tracing::info!(agents = agents.len(), address = ?J(&address), "server claimed");
        agents
    };
    let config = state.config();
    Ok(web::ok(json!({"bridgeURL": config.public_url, "bridgeKey": config.bridge_key,
                      "fingerprint": crate::lan::fingerprint(&config), "agents": agents})))
}

/// A claim on a place already claimed: its claimer whose answer was lost gets the place back (« a claim whose answer
/// was lost »), anyone else `410`. Called with the claims' lock held.
async fn recover(
    shared: &Shared,
    state: &AppState,
    config: &Config,
    address: Option<&str>,
    body: &Claim,
) -> ApiResult<Response> {
    let gone = || HttpError::new(410, "these agents are already linked to an app");
    let Some((secret, mut record)) = recovery_of(config) else { return Err(gone()) };
    if !shared.challenges.take(&body.challenge) {
        return Err(HttpError::new(400, "challenge unknown or expired: ask /v1/setup again"));
    }
    let expected = proof(&secret, &[CLAIM_CONTEXT, &body.challenge, &body.request]);
    match pyx::compare_digest(&expected, &body.mac.to_lowercase()) {
        None => return Err(HttpError::internal()),
        Some(false) => {
            state.guard.fail(address, "pairing proof");
            return Err(HttpError::new(401, "wrong pairing proof"));
        }
        Some(true) => {}
    }
    let Ok(asked) = serde_json::from_str::<Value>(&body.request) else {
        return Err(HttpError::new(400, "invalid request"));
    };
    if asked.as_object().is_some_and(|o| o.contains_key("reopen") || o.contains_key("restore")) {
        return Err(gone()); // only a plain claim is recovered
    }
    let wanted = match ClaimRequest::from_value(&asked) {
        Ok(w) => w,
        Err(msg) => return Err(HttpError::new(400, format!("invalid request: {msg}"))),
    };
    if wanted.model.provider == "custom" && wanted.model.base_url.as_deref().map(str::is_empty).unwrap_or(true) {
        return Err(HttpError::new(400, "a custom model needs its base_url"));
    }
    let claim_id = match claim_id_of(&asked) {
        Ok(id) => id.map(|id| sha256_hex(&id)),
        Err(msg) => return Err(HttpError::new(400, format!("invalid request: {msg}"))),
    };
    if let Some(first) = record.get("claim_id").and_then(Value::as_str) {
        if claim_id.as_deref() != Some(first) {
            tracing::warn!(address = ?J(&address), "claim recovery refused: not the first claim's id");
            return Err(gone());
        }
    }
    let model_key = sha256_hex(&wanted.model.key);
    if record.get("model_key").and_then(Value::as_str) != Some(model_key.as_str()) {
        if let Err(exc) = (shared.hooks.rekey)(state.clone(), wanted.model.clone()).await {
            tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "recovered claim's AI key not written");
            return Err(HttpError::new(502, "the setup could not finish: try again"));
        }
    }
    let s = state.clone();
    let key = tokio::task::spawn_blocking(move || rotate_bridge_key(&s)).await.map_err(anyhow::Error::from)??;
    record.insert("key".into(), json!(sha256_hex(&key)));
    record.insert("model_key".into(), json!(model_key));
    if let Some(id) = claim_id {
        record.insert("claim_id".into(), json!(id)); // a retry that brought one: the next ones need it too
    }
    let path = recovery_path(config);
    crate::py::write_with_mode(
        &path.to_string_lossy(),
        crate::py::dumps(&Value::Object(record.clone())).as_bytes(),
        0o600,
    )
    .map_err(anyhow::Error::from)?;
    let now = state.config();
    let agents: Vec<Value> = match record.get("agents") {
        Some(Value::Array(items)) => items
            .iter()
            // the first answer's agents (none can have been removed: that takes the bridge key), names as they are now
            .map(|a| match a.get("id").and_then(Value::as_str).and_then(|id| now.agents.get(id)) {
                Some(agent) => json!({"id": agent.name, "name": agent.display_name}),
                None => a.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    tracing::info!(agents = agents.len(), address = ?J(&address), "claim recovered: a new bridge key for its claimer");
    Ok(web::ok(json!({"bridgeURL": now.public_url, "bridgeKey": key, "fingerprint": crate::lan::fingerprint(&now),
                      "agents": agents, "recovered": true})))
}

/// `cloud_setup_router()` with the routes' own claim and restore (tests).
pub fn router_with(hooks: Hooks) -> Router<AppState> {
    let shared = Arc::new(Shared { hooks, challenges: Challenges::default(), lock: tokio::sync::Mutex::new(()) });
    let (a, b) = (shared.clone(), shared);
    Router::new()
        .route(
            "/setup",
            get(move |State(state): State<AppState>, ClientAddr(address): ClientAddr, q: QueryMap| {
                setup_state(a.clone(), state, address, q)
            }),
        )
        .route(
            "/setup/claim",
            post(move |State(state): State<AppState>, ClientAddr(address): ClientAddr, Json(body): Json<Claim>| {
                claim(b.clone(), state, address, body)
            }),
        )
}

/// `cloud_setup_router()`: a server the app created, until the app claims it (its own proofs, no bridge key).
/// Mounted under /v1 without the bridge key.
pub fn router() -> Router<AppState> {
    router_with(Hooks::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gateway_is_restarted_by_its_unit_when_there_is_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k".repeat(40));
        config.hermes.bin = "/opt/hermes".into();
        assert_eq!(restart_command(&config, dir.path()), vec!["/opt/hermes", "gateway", "restart"]);
        std::fs::write(dir.path().join("bipagents-hermes.service"), "").unwrap();
        assert_eq!(restart_command(&config, dir.path()), vec!["systemctl", "--user", "restart", "bipagents-hermes"]);
        std::fs::write(dir.path().join("hermes-gateway.service"), "").unwrap();
        assert_eq!(restart_command(&config, dir.path()), vec!["systemctl", "--user", "restart", "hermes-gateway"]);
    }

    #[test]
    fn a_key_in_env_files_is_replaced_or_added() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".env");
        set_env(&path, "OPENROUTER_API_KEY", "sk-1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "OPENROUTER_API_KEY=sk-1\n");
        std::fs::write(&path, "# keys\nAPI_SERVER_KEY=abc\nOPENROUTER_API_KEY=old\n").unwrap();
        set_env(&path, "OPENROUTER_API_KEY", "sk-2").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "# keys\nAPI_SERVER_KEY=abc\nOPENROUTER_API_KEY=sk-2\n");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn challenges_serve_once_and_expire() {
        let now = Arc::new(Mutex::new(0.0f64));
        let n = now.clone();
        let challenges = Challenges::new(Arc::new(move || *n.lock().unwrap()));
        let c = challenges.issue();
        assert_eq!(c.len(), 32);
        assert!(challenges.take(&c) && !challenges.take(&c));
        let c = challenges.issue();
        *now.lock().unwrap() = CHALLENGE_SECONDS;
        assert!(!challenges.take(&c));
        for _ in 0..70 {
            challenges.issue();
        }
        assert!(challenges.items.lock().unwrap().len() <= 65);
    }

    #[test]
    fn claim_requests_as_pydantic_reads_them() {
        let ok = ClaimRequest::from_json(r#"{"model": {"provider": "custom", "name": "glm", "key": "sk-0123456789", "base_url": "https://x/v1", "vision": "yes"}, "agents": [{"name": "Q"}], "language": "fr"}"#).unwrap();
        assert!(ok.model.vision && ok.agents[0].kind.is_none());
        assert_eq!(ClaimRequest::from_json("[]").unwrap_err(), "Input should be an object");
        assert_eq!(
            ClaimRequest::from_json(r#"{"model": {"provider": "x"}}"#).unwrap_err(),
            "String should match pattern '^(openrouter|custom)$'"
        );
        assert_eq!(
            ClaimRequest::from_json(r#"{"model": {"provider": "custom", "name": "a", "key": "short"}}"#).unwrap_err(),
            "String should have at least 8 characters"
        );
        assert_eq!(
            ClaimRequest::from_json(
                r#"{"model": {"provider": "custom", "name": "a", "key": "kkkkkkkkk", "vision": 2}}"#
            )
            .unwrap_err(),
            "Input should be a valid boolean, unable to interpret input"
        );
    }

    #[test]
    fn model_commands_as_the_installer_sets_them() {
        let mut config = Config::with_key("k");
        config.hermes.bin = "/opt/hermes".into();
        let model = ModelChoice {
            provider: "custom".into(),
            name: "glm".into(),
            base_url: Some("https://e/v1".into()),
            key: "sk".into(),
            vision: false,
        };
        let commands = model_commands(&config, &model);
        assert_eq!(commands[2], vec!["/opt/hermes", "config", "set", "model.base_url", "https://e/v1"]);
        assert_eq!(commands[3], vec!["/opt/hermes", "config", "set", "model.key_env", "CUSTOM_MODEL_API_KEY"]);
        assert_eq!(commands[5], vec!["/opt/hermes", "config", "set", "model.supports_vision", "false"]);
    }
}
