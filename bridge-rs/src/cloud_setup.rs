//! « No computer »: a server of the all-included offer, from the guichet's reserve (guichet/), set up at first boot by
//! the installer's `--cloud` mode, then paired with the person's app without a QR code.
//!
//! The guichet made a one-time secret (`pair`, 32 random bytes), put it in the server's first-boot setup and hands it to
//! the app that bought the server; nothing else secret is there, no model key, since the provider keeps that setup. The bridge's door is on the Internet with its own self-signed
//! certificate (lan.py). First the app learns that this certificate is really its server's: the bridge proves it knows
//! `pair` with an HMAC over the app's nonce and the certificate's fingerprint, which nobody in between can forge, and the
//! app pins it. Then the app claims the server over that pinned connection: it proves it knows `pair` too (an HMAC over
//! the bridge's one-time challenge and the exact request), sends the model key and the agents to create, and receives the
//! bridge key. The secret is then deleted: any later claim is refused.
//!
//!     GET  /v1/setup?nonce=<hex>   {state: installing | ready | restoring | claimed, step, total, label, error?,
//!                                   fingerprint, challenge, proof}
//!     POST /v1/setup/claim         {challenge, mac, request: "<JSON>"}  ->  {bridgeURL, bridgeKey, fingerprint, agents}
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
    let value = data.get("pair")?.as_str()?;
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
    static BRIDGE_KEY_LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^bridge_key\s*=.*$").expect("valid regex"));
    let config = state.config();
    let keys = crate::device_keys::of(&config);
    keys.clear();
    let (_, key) = keys.mint(name)?;
    let new = crate::py::token_urlsafe(32);
    if let Some(source) = config.source_path.as_deref().filter(|p| !p.is_empty()) {
        let line = format!("bridge_key = {}", crate::py::dumps(&json!(new)));
        let text = std::fs::read_to_string(source)?;
        let out = if BRIDGE_KEY_LINE.is_match(&text) {
            BRIDGE_KEY_LINE.replacen(&text, 1, regex::NoExpand(&line)).into_owned()
        } else {
            format!("{line}\n{text}")
        };
        std::fs::write(source, out)?;
        crate::py::chmod(source, 0o600)?;
    }
    state.config.update(|c| c.bridge_key = new.clone());
    // the web consoles opened from the old devices too
    for session in state.console.devices() {
        if let Some(id) = session.get("id") {
            state.console.revoke(&crate::py::py_str(id));
        }
    }
    Ok(key)
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

/// The hub profile's model (every agent is cloned from it), as the installer sets it (hermes.set_model).
pub fn model_commands(config: &Config, model: &ModelChoice) -> Vec<Vec<String>> {
    // the hub is the default profile: no -p
    let setting = |key: &str, value: &str| {
        vec![config.hermes.bin.clone(), "config".into(), "set".into(), key.to_string(), value.to_string()]
    };
    let key_env =
        MODEL_KEYS.iter().find(|(p, _)| *p == model.provider).map(|(_, k)| *k).unwrap_or("CUSTOM_MODEL_API_KEY");
    let mut commands = vec![setting("model.provider", &model.provider), setting("model.default", &model.name)];
    let base_url = model
        .base_url
        .clone()
        .filter(|u| !u.is_empty())
        .or_else(|| (model.provider == "openrouter").then(|| "https://openrouter.ai/api/v1".to_string()));
    if let Some(url) = base_url {
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
    let text = std::fs::read_to_string(&source)?;
    let mut inside = false;
    let mut kept: Vec<String> = Vec::new();
    for line in crate::hermes::splitlines(&text) {
        let stripped = pyx::strip(&line);
        if stripped.starts_with('[') && stripped.ends_with(']') {
            inside = stripped == "[model]";
        }
        if !inside {
            kept.push(line.clone());
        }
    }
    let text = format!(
        "{}\n\n[model]\nhealth_url = {}\nhealth_key = {}\n",
        kept.join("\n").trim_end_matches('\n'),
        crate::py::dumps(&json!(url)),
        crate::py::dumps(&json!(key))
    );
    std::fs::write(&source, text)?;
    crate::py::chmod(&source, 0o600)?;
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

/// The model set, the gateway restarted to read its key, the agents created. Returns the agents (id, name).
pub async fn claim_server(
    config: &SharedConfig,
    request: &ClaimRequest,
    http: &reqwest::Client,
    run: Option<Run>,
) -> anyhow::Result<Vec<Value>> {
    use crate::agents_admin::{create_agent, default_run, NewAgent};
    let run = run.unwrap_or_else(default_run);
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
        let body = NewAgent {
            name: choice.name.clone(),
            kind: Some(choice.kind.clone().filter(|k| !k.is_empty()).unwrap_or_else(|| "daily".into())),
            lang: request.language.clone(),
            custom: None,
            group: None,
        };
        let agent = create_agent(config, &body, http, &run).await?;
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

// -- routes ------------------------------------------------------------------------------------------

/// What the routes call to claim a server and to start a restore (tests give their own, as Python's monkeypatch).
pub type ClaimFn = Arc<dyn Fn(AppState, ClaimRequest) -> BoxFuture<'static, anyhow::Result<Vec<Value>>> + Send + Sync>;
pub type RestoreFn = Arc<dyn Fn(&Config, &str) -> std::io::Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct Hooks {
    pub claim: ClaimFn,
    pub restore: RestoreFn,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks {
            claim: Arc::new(|state: AppState, request: ClaimRequest| {
                Box::pin(async move { claim_server(&state.config, &request, &state.http, None).await })
            }),
            restore: Arc::new(start_restore),
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
        return Ok(web::ok(json!({"state": "claimed"})));
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
    });
    if let Some(error) = step.error {
        out["error"] = error; // a restore that failed (a wrong key): the app may claim again
    }
    Ok(web::ok(out))
}

async fn claim(shared: Arc<Shared>, state: AppState, address: Option<String>, body: Claim) -> ApiResult<Response> {
    let config = guarded(&state, address.as_deref())?;
    let agents = {
        let _lock = shared.lock.lock().await;
        let Some(secret) = secret_of(&config) else {
            return Err(HttpError::new(410, "this server is already paired"));
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
        let agents = match (shared.hooks.claim)(state.clone(), wanted).await {
            Ok(a) => a,
            Err(exc) => {
                tracing::warn!(error = %pyx::tail(&format!("{exc:#}"), 300), "claim failed");
                return Err(HttpError::new(502, "the server could not finish: try again"));
            }
        };
        let _ = std::fs::remove_file(&setup_file); // paired: the secret is gone for good
        tracing::info!(agents = agents.len(), address = ?J(&address), "server claimed");
        agents
    };
    let config = state.config();
    Ok(web::ok(json!({"bridgeURL": config.public_url, "bridgeKey": config.bridge_key,
                      "fingerprint": crate::lan::fingerprint(&config), "agents": agents})))
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
