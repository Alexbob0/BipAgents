//! The agents' vault: credentials given from the app, used by the agents without them ever reading one.
//!
//! The bridge keeps the master copy, encrypted (Fernet) in its data folder, which no agent can reach: agents
//! run in their own container or sandbox. Each item is then written into the Hermes vault of the agents the
//! person allowed, and only theirs. Hermes fills the login form itself on the item's exact origin, and the
//! password never reaches the model, the conversation or the logs (`browser_vault_fill`).
//!
//! Write-only: the app never reads a secret back. It replaces or deletes it. One exception: a login an agent created
//! with a password the vault made (vault_logins.rs), which the person never typed, is shown to them on request.
//!
//! An agent holds items in its own space (`isolated`), or, since 2026-10-28 (Alex), in the shared hub when only the
//! person's own agents live there (`holds_in_hub`): in a shared gateway an agent with a terminal could read
//! another's vault, which is accepted between one person's agents, never with an agent other people talk to
//! (`others_facing`). While the hub hosts one, its agents move into their own space at their first secret
//! (own_space.rs), as before. Trade-off: one agent tricked by what it read could reach the person's other agents'
//! passwords, in exchange for no extra container per agent.
//!
//! Port of `bridge/bipbridge/vault.py` (owner group: browsers-places).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, patch};
use axum::Router;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::browsers::pyx;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};
use crate::Dict;

pub const KINDS: [&str; 4] = ["login", "payment", "address", crate::cards::KIND];
/// A card's handle for an agent allowed to use it: nothing is written into the agent's own vault.
pub const CARD_HANDLE: &str = "bridge";
pub const IDENTIFIER_TYPES: [&str; 3] = ["email", "phone", "username"];
pub const PAYMENT_FIELDS: [&str; 6] =
    ["card_number", "cardholder_name", "exp_month", "exp_year", "cvc", "billing_postal_code"];
pub const ADDRESS_FIELDS: [&str; 6] = ["address_line1", "address_line2", "city", "state", "postal_code", "country"];

/// Runs inside the agent's Hermes (its Python, its HERMES_HOME): the request, secret included, comes on stdin,
/// never on the command line. Prints JSON only.
pub const HERMES_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
req = json.load(sys.stdin)
if req.get("root"):  # Hermes' own Python with its code folder and dependencies, as its launcher sets them up
    sys.path.insert(0, req["root"])
    import hermes_bootstrap
from agent.vault_store import VaultStore
store = VaultStore(base_dir=Path(req["home"]) / "vault")
if req["op"] == "add":
    meta = store.add_item(req["kind"], req["label"], req["secret"], origin=req.get("origin"))
    print(json.dumps({"id": meta.id}))
elif req["op"] == "remove":
    print(json.dumps({"removed": store.remove_item(req["id"])}))
else:
    print(json.dumps({"items": [m.to_dict() for m in store.list_items()]}))
"#;

/// Python's `VaultError` and its kinds.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VaultError {
    #[error("{0}")]
    Invalid(String),
    /// `CannotHold(reasons)`: agent id → why it cannot hold credentials.
    #[error("some agents cannot hold credentials")]
    CannotHold(IndexMap<String, String>),
    /// `Unprotected`.
    #[error("this server does not isolate its agents: no credentials are kept")]
    Unprotected,
    /// Payment cards are off on this install (`[vault] cards`).
    #[error("payment cards are off on this server")]
    CardsOff,
    /// `ConfirmOthers(agents)`: other people talk to these agents and the person did not confirm (`confirmed_others`).
    #[error("other people talk to these agents: the person must confirm")]
    ConfirmOthers(Vec<String>),
}

/// A card found for an agent (`Vault::card_for`): its numbers stay in the bridge.
pub struct CardFound {
    pub id: String,
    pub label: String,
    /// Its public part (cards.rs).
    pub meta: Dict,
    /// number, exp_month, exp_year, csc, name, postal_code.
    pub secret: Dict,
}

/// Python's `KeyError(item_id)`: no item with this id.
#[derive(Debug, Clone, thiserror::Error)]
#[error("'{0}'")]
pub struct NoSuchItem(pub String);

fn invalid(text: &str) -> VaultError {
    VaultError::Invalid(text.to_string())
}

/// An item's metadata (never its secret).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub origin: Option<String>,
    pub identifier_type: Option<String>,
    pub identifier: Option<String>,
    pub has_otp: bool,
    /// agent id -> its handle in that agent's Hermes vault
    pub agents: IndexMap<String, String>,
    pub created_at: String,
    pub updated_at: String,
    /// A payment card's public part (cards.rs): last 4 digits, expiry, flags, sites, payments. Never its numbers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card: Option<Value>,
    /// A login an agent created on a website with a password of the vault (vault_logins.rs): that agent's id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Such a login until the agent (or the person) confirms the site accepted the sign-up: « en attente de
    /// confirmation ».
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed_at: Option<String>,
    /// A secret an agent asked for (secrets.rs): its kind (password, token, code), the identifier's label, when last
    /// used. Never its value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_info: Option<Value>,
}

impl Item {
    /// What the app sees: the agents' ids, sorted, without their handles.
    pub fn public(&self) -> Value {
        let mut agents: Vec<&String> = self.agents.keys().collect();
        agents.sort();
        let mut out = json!({"id": self.id, "kind": self.kind, "label": self.label, "origin": self.origin,
               "identifier_type": self.identifier_type, "identifier": self.identifier, "has_otp": self.has_otp,
               "agents": agents, "created_at": self.created_at, "updated_at": self.updated_at});
        if let (Some(card), Value::Object(map)) = (&self.card, &mut out) {
            map.insert("card".into(), crate::cards::public(card));
        }
        if let (Some(info), Value::Object(map)) = (&self.secret_info, &mut out) {
            map.insert("created_by".into(), json!(self.created_by));
            map.insert("secret".into(), info.clone());
            return out;
        }
        if let (Some(agent), Value::Object(map)) = (&self.created_by, &mut out) {
            map.insert("created_by".into(), json!(agent));
            map.insert("pending".into(), json!(self.pending));
            map.insert("confirmed_at".into(), json!(self.confirmed_at));
        }
        out
    }
}

/// `datetime.now(timezone.utc).isoformat(timespec="seconds")`.
pub fn now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

/// « sncf-connect.com/login » → « https://sncf-connect.com »: the origin of what was typed, nothing guessed
/// (no « www. » added). Hermes fills only on this exact origin.
pub fn normalize_origin(url: &str) -> Result<String, VaultError> {
    let raw = pyx::strip(url).to_string();
    if raw.is_empty() {
        return Err(invalid("site is required"));
    }
    let raw = if raw.contains("://") { raw } else { format!("https://{raw}") };
    let parts = pyx::urlsplit(&raw).map_err(VaultError::Invalid)?;
    let hostname = parts.hostname();
    if !(parts.scheme == "https" || parts.scheme == "http") || hostname.is_none() {
        return Err(invalid("site must be a web address"));
    }
    let port = parts.port().map_err(VaultError::Invalid)?;
    let default = if parts.scheme == "https" { 443 } else { 80 };
    let port = match port {
        Some(p) if p != 0 && p != default => format!(":{p}"),
        _ => String::new(),
    };
    Ok(format!("{}://{}{port}", parts.scheme, hostname.unwrap_or_default().to_lowercase()))
}

pub fn in_hub(config: &Config, agent: &AgentConfig) -> bool {
    config.hermes.enabled()
        && agent.space.as_deref().map(str::is_empty).unwrap_or(true)
        && agent.hermes_url.starts_with(&format!("{}/p/", config.hermes.hub_url))
}

/// The agents of this place that people other than its person talk to (Alex, 2026-10-28): the family's agent,
/// project agents (with guests), agents shared with a relative (agent_shares.rs), and any agent another household's
/// agents may write to or be written to by (a link of « Qui peut écrire à qui » with a relative's agent at the other
/// end, peers.rs; rooms_remote.rs uses the same links). Each file read once.
pub fn others_facing_agents(config: &Config) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if let Some(family) = crate::family_agent::own(config) {
        out.insert(family);
    }
    let links = crate::peers::links_of(&crate::peers::Links::new(&crate::py::join(&config.data_dir(), "peers.json")).load());
    let remote = |id: &str| config.agent(Some(id)).is_none() && crate::households::split(id).is_some();
    for (source, target) in &links {
        if remote(source) {
            out.insert(target.to_lowercase());
        }
        if remote(target) {
            out.insert(source.to_lowercase());
        }
    }
    for name in config.agents.keys() {
        if crate::project_agent::is_project(config, name) || !crate::agent_shares::shared_with(config, name).is_empty() {
            out.insert(name.clone());
        }
    }
    out
}

/// Whether people other than this place's person talk to `agent` (`others_facing_agents`).
pub fn others_facing(config: &Config, agent: &str) -> bool {
    others_facing_agents(config).contains(&agent.trim().to_lowercase())
}

/// Who else talks to `agent`, by name, for the confirmation before it gets a credential (secrets.rs): the linked
/// households of the family's agent, the relatives it is shared with, the households whose agents are linked with it.
/// Empty when none is known by name (the apps then say « d'autres personnes »). Project guests have no names here: a
/// project agent never gets credentials.
pub fn others_names(config: &Config, agent: &str) -> Vec<String> {
    let agent = agent.trim().to_lowercase();
    let linked = crate::households::Households::new(&crate::households::path_for(config));
    let mut households: Vec<String> = crate::agent_shares::shared_with(config, &agent);
    let links = crate::peers::links_of(&crate::peers::Links::new(&crate::py::join(&config.data_dir(), "peers.json")).load());
    for (source, target) in &links {
        let other = if target.to_lowercase() == agent {
            source
        } else if source.to_lowercase() == agent {
            target
        } else {
            continue;
        };
        if config.agent(Some(other)).is_none() {
            if let Some((_, household)) = crate::households::split(other) {
                households.push(household);
            }
        }
    }
    let mut names: Vec<String> = Vec::new();
    for household in households {
        let name = linked
            .get(&household)
            .and_then(|h| h.get("name").map(crate::py::py_str))
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_default();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Whether `agent`, of the shared hub, holds credentials right there, with no move (Alex, 2026-10-28: a person's own
/// agents are no longer separated from each other): the install protects the vault, the hub's Python can write its
/// items, the agent's tools run in a sandbox of their own (sandbox.rs: connectors need one, the move gives it one),
/// the agent is not others-facing and the hub hosts none. `holder` asks at each secret's use, not only when the tools
/// are offered: once an others-facing agent appears in the hub, a new secret moves its agent as before.
pub fn holds_in_hub(config: &Config, agent: &AgentConfig) -> bool {
    hub_may_hold(config, agent, &others_facing_agents(config))
        && crate::sandbox::isolates(config, agent)
}

/// `holds_in_hub` but the sandbox (unit tests).
fn hub_may_hold(config: &Config, agent: &AgentConfig, facing: &std::collections::HashSet<String>) -> bool {
    config.hermes.isolated
        && !agent.isolated
        && in_hub(config, agent)
        && !config.hermes.python.is_empty()
        && !facing.contains(&agent.name)
        && !config.agents.values().any(|a| in_hub(config, a) && facing.contains(&a.name))
}

/// The hub's agents holding credentials although the hub now hosts an others-facing agent (logged at start and when
/// an agent becomes others-facing: their items stay in their profile's vault until they move or are taken back).
pub fn hub_holders_at_risk(config: &Config, records: &[Dict]) -> Vec<String> {
    let facing = others_facing_agents(config);
    if !config.agents.values().any(|a| in_hub(config, a) && facing.contains(&a.name)) {
        return Vec::new();
    }
    let mut out: Vec<String> = config
        .agents
        .values()
        .filter(|a| in_hub(config, a) && !a.isolated)
        .filter(|a| {
            records.iter().any(|r| {
                r.get("agents").and_then(Value::as_object).and_then(|m| m.get(&a.name)).and_then(Value::as_str)
                    .is_some_and(|h| h != CARD_HANDLE && !h.is_empty())
            })
        })
        .map(|a| a.name.clone())
        .collect();
    out.sort();
    out
}

/// The agent as the vault reaches its Hermes: an agent holding credentials in the hub gets the hub's Python and its
/// profile's HERMES_HOME (as that Python sees it), the others are as they are.
pub fn vault_target(config: &Config, agent: &AgentConfig) -> AgentConfig {
    if agent.isolated || !in_hub(config, agent) || config.hermes.python.is_empty() {
        return agent.clone();
    }
    let mut target = agent.clone();
    let home = if !config.hermes.container_home.is_empty() {
        crate::py::join(&crate::py::join(&config.hermes.container_home, "profiles"), &agent.name)
    } else {
        crate::spaces::hub_profile(config, &agent.name).to_string_lossy().into_owned()
    };
    target.python = config.hermes.python.clone();
    target.hermes_home = Some(home);
    target.hermes_root = Some(config.hermes.root.clone()).filter(|r| !r.is_empty());
    target
}

/// None when the agent can hold credentials, else why not.
pub fn holder(config: &Config, agent: &AgentConfig) -> Option<String> {
    if !config.hermes.isolated {
        return Some("unprotected_install".into()); // the agents could read the bridge's own copy: no credentials at all
    }
    if !agent.isolated {
        if holds_in_hub(config, agent) {
            return None; // one of the person's own agents, in a hub only they use
        }
        // An agent of the shared hub can move into its own space from the app; one set up by hand is up to its
        // administrator (its own container, then `isolated`, `python` and `hermes_home` in this file).
        return Some(if in_hub(config, agent) { "needs_own_space" } else { "not_configured" }.into());
    }
    if agent.space.as_deref().map(|s| !s.is_empty()).unwrap_or(false) && !agent.sandboxed {
        // Its own gateway, but under the same account as the hub's agents, without a sandbox: they could still
        // read its vault on disk.
        return Some("needs_sandbox".into());
    }
    if agent.python.is_empty() || agent.hermes_home.as_deref().map(str::is_empty).unwrap_or(true) {
        return Some("not_configured".into());
    }
    None
}

/// How the vault reaches an agent's Hermes vault (tests give their own, as Python's `run=`).
pub type VaultRun = Arc<dyn Fn(AgentConfig, Dict) -> BoxFuture<'static, anyhow::Result<Dict>> + Send + Sync>;

/// The request on the agent's own Python, its answer the last line it prints.
pub async fn run_in_hermes(agent: AgentConfig, request: Dict) -> anyhow::Result<Dict> {
    use tokio::io::AsyncWriteExt;
    let Some((program, rest)) = agent.python.split_first() else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no python for this agent").into());
    };
    let mut child = tokio::process::Command::new(program)
        .args(rest)
        .args(["-c", HERMES_SCRIPT])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut payload = Dict::new();
    payload.insert("home".into(), json!(agent.hermes_home));
    payload.insert("root".into(), json!(agent.hermes_root));
    payload.extend(request);
    let payload = crate::py::dumps(&Value::Object(payload));
    let mut stdin = child.stdin.take();
    let writing = async move {
        if let Some(stdin) = stdin.as_mut() {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }
        drop(stdin);
    };
    let (_, output) =
        tokio::time::timeout(Duration::from_secs(60), async { tokio::join!(writing, child.wait_with_output()) })
            .await
            .map_err(|_| anyhow::anyhow!("TimeoutError"))?;
    let output = output?;
    if !output.status.success() {
        // Hermes' VaultError messages never contain secrets; keep only the last line.
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        let lines = crate::hermes::splitlines(pyx::strip(&err));
        let last = lines.last().cloned().unwrap_or_else(|| "?".into());
        return Err(VaultError::Invalid(format!(
            "{}: Hermes refused the item ({})",
            agent.name,
            pyx::tail(&last, 200)
        ))
        .into());
    }
    let unexpected = || VaultError::Invalid(format!("{}: unexpected answer from Hermes", agent.name));
    let out = String::from_utf8(output.stdout).map_err(|_| unexpected())?;
    let last = crate::hermes::splitlines(pyx::strip(&out)).last().cloned().ok_or_else(unexpected)?;
    match serde_json::from_str::<Value>(&last).map_err(|_| unexpected())? {
        Value::Object(map) => Ok(map),
        other => Err(anyhow::anyhow!("TypeError: {} is not a dict", crate::py::py_repr(&other))),
    }
}

/// `[str(a) for a in value]` (a str gives its characters, a dict its keys; a number cannot be iterated).
fn strs_of(value: &Value) -> anyhow::Result<Vec<String>> {
    match value {
        Value::Array(items) => Ok(items.iter().map(crate::py::py_str).collect()),
        Value::String(s) => Ok(s.chars().map(|c| c.to_string()).collect()),
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        other => Err(anyhow::anyhow!("TypeError: {} object is not iterable", crate::py::py_repr(other))),
    }
}

fn given<'a>(body: &'a Dict, key: &str) -> Option<&'a Value> {
    body.get(key).filter(|v| !v.is_null())
}

fn truthy(body: &Dict, key: &str) -> bool {
    body.get(key).map(crate::py::truthy).unwrap_or(false)
}

/// The person's credentials (Fernet-encrypted `vault.json.enc`, its key `vault.key`), given to the agents' own
/// Hermes vaults.
pub struct Vault {
    pub folder: PathBuf,
    pub path: PathBuf,
    pub key_path: PathBuf,
    pub run: VaultRun,
    lock: tokio::sync::Mutex<()>,
}

impl Vault {
    pub fn new(folder: &str) -> Self {
        Self::with_run(folder, Arc::new(|agent, request| Box::pin(run_in_hermes(agent, request))))
    }

    pub fn with_run(folder: &str, run: VaultRun) -> Self {
        let folder = PathBuf::from(folder);
        Vault {
            path: folder.join("vault.json.enc"),
            key_path: folder.join("vault.key"),
            folder,
            run,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    // -- storage ---------------------------------------------------------------------------------------

    fn fernet(&self) -> anyhow::Result<fernet::Fernet> {
        crate::py::makedirs(&self.folder.to_string_lossy(), 0o700)?;
        if !self.key_path.exists() {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut fh = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&self.key_path)?;
            fh.write_all(fernet::Fernet::generate_key().as_bytes())?;
        }
        let key = std::fs::read_to_string(&self.key_path)?;
        fernet::Fernet::new(pyx::strip(&key))
            .ok_or_else(|| VaultError::Invalid("Fernet key must be 32 url-safe base64-encoded bytes.".into()).into())
    }

/// Logs the hub's agents that hold credentials while the hub hosts an others-facing agent (`hub_holders_at_risk`):
    /// their items stay in their profile's vault (nothing is taken back by itself), new secrets move them.
    pub fn warn_hub_holders(&self, config: &Config) {
        let at_risk = hub_holders_at_risk(config, &self.read().unwrap_or_default());
        if !at_risk.is_empty() {
            tracing::warn!(target: "bipbridge::vault", agents = ?at_risk, "the hub hosts an agent other people talk to, \
                and these agents of the hub hold credentials there: they stay until the agent moves or the person \
                takes them back; a new secret moves the agent into its own space");
        }
    }

    fn read(&self) -> anyhow::Result<Vec<Dict>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let token = std::fs::read_to_string(&self.path)?;
        let plain = self
            .fernet()?
            .decrypt(pyx::strip(&token))
            .map_err(|_| invalid("vault cannot be decrypted with its key"))?;
        match serde_json::from_slice::<Value>(&plain) {
            Ok(Value::Array(rows)) => Ok(rows.into_iter().filter_map(|r| r.as_object().cloned()).collect()),
            Ok(_) => Ok(Vec::new()),
            Err(_) => Err(VaultError::Invalid(pyx::json_error(&plain).unwrap_or_default()).into()),
        }
    }

    fn write(&self, records: &[Dict]) -> anyhow::Result<()> {
        let text = crate::py::dumps(&Value::Array(records.iter().cloned().map(Value::Object).collect()));
        let blob = self.fernet()?.encrypt(text.as_bytes());
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = self.path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        crate::py::write_with_mode(&tmp.to_string_lossy(), blob.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    fn item(record: &Dict) -> Item {
        let meta: Map<String, Value> =
            record.iter().filter(|(k, _)| k.as_str() != "secret").map(|(k, v)| (k.clone(), v.clone())).collect();
        serde_json::from_value(Value::Object(meta)).unwrap_or_default()
    }

    /// The items, or what kept them from being read.
    pub fn try_items(&self) -> anyhow::Result<Vec<Item>> {
        Ok(self.read()?.iter().map(Self::item).collect())
    }

    pub fn items(&self) -> Vec<Item> {
        self.try_items().unwrap_or_default()
    }

    // -- what the app sends ------------------------------------------------------------------------------

    /// A full record (metadata + secret) from the app's fields; `existing` for an update (fields left out
    /// are kept, the secret included).
    pub fn parse(body: &Dict, existing: Option<&Dict>) -> Result<Dict, VaultError> {
        Self::parse_with(body, existing, None)
    }

    /// `parse`, with a place of the offer's rules for cards (`phone`: their numbers stay on the phone).
    pub fn parse_with(
        body: &Dict,
        existing: Option<&Dict>,
        phone: Option<crate::cards::PhoneRules>,
    ) -> Result<Dict, VaultError> {
        let mut record = existing.cloned().unwrap_or_default();
        let kind = body
            .get("kind")
            .filter(|v| crate::py::truthy(v))
            .or_else(|| record.get("kind").filter(|v| crate::py::truthy(v)))
            .map(crate::py::py_str)
            .unwrap_or_else(|| "login".into());
        if !KINDS.contains(&kind.as_str()) {
            return Err(invalid("unknown kind"));
        }
        record.insert("kind".into(), json!(kind));
        let label = match given(body, "label") {
            Some(v) => crate::py::py_str(v),
            None => record.get("label").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default(),
        };
        let label = pyx::strip(&label).to_string();
        if label.is_empty() {
            return Err(invalid("name is required"));
        }
        record.insert("label".into(), json!(pyx::head(&label, 120)));
        let mut secret: Dict = match record.get("secret") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        if kind == "login" {
            if given(body, "site").is_some() || !truthy(&record, "origin") {
                let site = body.get("site").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default();
                record.insert("origin".into(), json!(normalize_origin(&site)?));
            }
            let id_type = body
                .get("identifier_type")
                .filter(|v| crate::py::truthy(v))
                .or_else(|| record.get("identifier_type").filter(|v| crate::py::truthy(v)))
                .map(crate::py::py_str)
                .unwrap_or_else(|| "email".into());
            if !IDENTIFIER_TYPES.contains(&id_type.as_str()) {
                return Err(invalid("unknown identifier type"));
            }
            let identifier = match given(body, "identifier") {
                Some(v) => crate::py::py_str(v),
                None => {
                    record.get("identifier").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default()
                }
            };
            let identifier = pyx::strip(&identifier).to_string();
            if identifier.is_empty() {
                return Err(invalid("user name is required"));
            }
            record.insert("identifier_type".into(), json!(id_type));
            record.insert("identifier".into(), json!(pyx::head(&identifier, 200)));
            if truthy(body, "password") {
                secret.insert("password".into(), json!(crate::py::py_str(&body["password"])));
            }
            if let Some(otp) = given(body, "otp_secret") {
                let otp = crate::py::py_str(otp);
                if !pyx::strip(&otp).is_empty() {
                    secret.insert("otp_secret".into(), json!(pyx::strip(&otp)));
                } else {
                    secret.shift_remove("otp_secret");
                }
            }
            if !secret.get("password").map(crate::py::truthy).unwrap_or(false) {
                return Err(invalid("password is required"));
            }
            let has_otp = secret.get("otp_secret").map(crate::py::truthy).unwrap_or(false);
            record.insert("has_otp".into(), json!(has_otp));
        } else if kind == crate::cards::KIND {
            match phone {
                Some(rules) => crate::cards::parse_phone(body, &mut record, &mut secret, rules)?,
                None => crate::cards::parse(body, &mut record, &mut secret)?,
            }
        } else {
            let allowed = if kind == "payment" { PAYMENT_FIELDS } else { ADDRESS_FIELDS };
            for name in allowed {
                if let Some(v) = given(body, name) {
                    secret.insert(name.into(), json!(pyx::strip(&crate::py::py_str(v))));
                }
            }
            let origin = if truthy(body, "site") {
                json!(normalize_origin(&crate::py::py_str(&body["site"]))?)
            } else {
                record.get("origin").cloned().unwrap_or(Value::Null)
            };
            record.insert("origin".into(), origin);
            record.insert("identifier_type".into(), Value::Null);
            record.insert("identifier".into(), Value::Null);
            record.insert("has_otp".into(), json!(false));
        }
        record.insert("secret".into(), Value::Object(secret));
        Ok(record)
    }

    pub fn to_hermes(record: &Dict) -> Dict {
        let mut secret: Dict = match record.get("secret") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        if record.get("kind").and_then(Value::as_str) == Some("login") {
            secret.insert("identifier_type".into(), record.get("identifier_type").cloned().unwrap_or(Value::Null));
            secret.insert("identifier".into(), record.get("identifier").cloned().unwrap_or(Value::Null));
        }
        let mut out = Dict::new();
        out.insert("op".into(), json!("add"));
        out.insert("kind".into(), record.get("kind").cloned().unwrap_or(Value::Null));
        out.insert("label".into(), record.get("label").cloned().unwrap_or(Value::Null));
        out.insert("origin".into(), record.get("origin").cloned().unwrap_or(Value::Null));
        out.insert("secret".into(), Value::Object(secret));
        out
    }

    // -- operations ------------------------------------------------------------------------------------

    fn check(config: &Config, agent_ids: &[String]) -> Result<Vec<AgentConfig>, VaultError> {
        let mut agents = Vec::new();
        let mut reasons = IndexMap::new();
        for agent_id in agent_ids {
            let Some(agent) = config.agent(Some(agent_id)) else {
                return Err(VaultError::Invalid(format!("unknown agent {agent_id}")));
            };
            if let Some(why) = holder(config, agent) {
                reasons.insert(agent_id.clone(), why);
            }
            agents.push(agent.clone());
        }
        if !reasons.is_empty() {
            return Err(VaultError::CannotHold(reasons));
        }
        Ok(agents)
    }

    /// An others-facing agent (`others_facing`) gets a login only once the person confirmed it (`confirmed_others`
    /// in the body, the app's « … parle aussi avec … Continuer ? »), as for a secret card (secrets.rs `give`).
    /// A card is left out: each of its payments already asks the person (card_pay.rs).
    fn confirmed(config: &Config, body: &Dict, record: &Dict, agents: &[AgentConfig]) -> Result<(), VaultError> {
        if body.get("confirmed_others").and_then(Value::as_bool) == Some(true)
            || record.get("kind").and_then(Value::as_str) == Some(crate::cards::KIND)
        {
            return Ok(());
        }
        let others: Vec<String> =
            agents.iter().filter(|a| others_facing(config, &a.name)).map(|a| a.name.clone()).collect();
        if others.is_empty() {
            Ok(())
        } else {
            tracing::info!(agents = ?others, "vault item not given: other people talk to the agents, not confirmed");
            Err(VaultError::ConfirmOthers(others))
        }
    }

    fn handles(record: &mut Dict) -> &mut Dict {
        if !matches!(record.get("agents"), Some(Value::Object(_))) {
            record.insert("agents".into(), Value::Object(Dict::new()));
        }
        match record.get_mut("agents") {
            Some(Value::Object(map)) => map,
            _ => unreachable!("just made"),
        }
    }

    async fn give(&self, config: &Config, record: &mut Dict, agents: &[AgentConfig]) -> anyhow::Result<()> {
        let card = record.get("kind").and_then(Value::as_str) == Some(crate::cards::KIND);
        for agent in agents {
            if card {
                // A card stays in the bridge: the agent only gets the right to ask for it (card_pay.rs).
                Self::handles(record).insert(agent.name.clone(), json!(CARD_HANDLE));
                continue;
            }
            let answer = (self.run)(vault_target(config, agent), Self::to_hermes(record)).await?;
            let handle = answer.get("id").ok_or_else(|| anyhow::anyhow!("KeyError: 'id'"))?;
            Self::handles(record).insert(agent.name.clone(), json!(crate::py::py_str(handle)));
            let item = record.get("id").map(crate::py::py_str).unwrap_or_default();
            tracing::info!(agent = %agent.name, item = %item, "vault item given");
        }
        Ok(())
    }

    async fn take_back(&self, config: &Config, record: &mut Dict, agent_ids: &[String]) {
        let item = record.get("id").map(crate::py::py_str).unwrap_or_default();
        for agent_id in agent_ids {
            let handle = Self::handles(record).shift_remove(agent_id);
            let agent = config.agent(Some(agent_id)).map(|a| vault_target(config, a));
            let handle = handle.filter(|h| crate::py::truthy(h) && h.as_str() != Some(CARD_HANDLE));
            if let (Some(handle), Some(agent)) = (handle, agent) {
                if !agent.python.is_empty() && agent.hermes_home.as_deref().map(|h| !h.is_empty()).unwrap_or(false) {
                    let mut request = Dict::new();
                    request.insert("op".into(), json!("remove"));
                    request.insert("id".into(), handle);
                    if (self.run)(agent.clone(), request).await.is_err() {
                        tracing::warn!(agent = %agent_id, item = %item, "vault item not removed from agent");
                    }
                }
            }
            tracing::info!(agent = %agent_id, item = %item, "vault item taken back");
        }
    }

    pub async fn create(&self, config: &Config, body: &Dict) -> anyhow::Result<Item> {
        if !config.hermes.isolated {
            return Err(VaultError::Unprotected.into());
        }
        let mut record = Self::parse_with(body, None, crate::cards::phone_rules(config))?;
        Self::cards_allowed(config, &record, true)?;
        let wanted = match body.get("agents").filter(|v| crate::py::truthy(v)) {
            Some(v) => strs_of(v)?,
            None => Vec::new(),
        };
        let agents = Self::check(config, &wanted)?;
        Self::confirmed(config, body, &record, &agents)?;
        record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
        record.insert("agents".into(), json!({}));
        record.insert("created_at".into(), json!(now()));
        record.insert("updated_at".into(), json!(now()));
        let _lock = self.lock.lock().await;
        if let Err(exc) = self.give(config, &mut record, &agents).await {
            let given: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
            self.take_back(config, &mut record, &given).await;
            return Err(exc);
        }
        let mut records = self.read()?;
        records.push(record.clone());
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    pub async fn update(&self, config: &Config, item_id: &str, body: &Dict) -> anyhow::Result<Item> {
        if !config.hermes.isolated {
            return Err(VaultError::Unprotected.into());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(index) = records.iter().position(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let before = records[index].clone();
        let mut record = Self::parse_with(body, Some(&before), crate::cards::phone_rules(config))?;
        Self::cards_allowed(config, &record, false)?;
        let before_agents = match before.get("agents") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        record.insert("agents".into(), Value::Object(before_agents.clone()));
        let had: Vec<String> = before_agents.keys().cloned().collect();
        let wanted = match given(body, "agents") {
            Some(v) => strs_of(v)?,
            None => had.clone(),
        };
        let changed = ["label", "origin", "identifier", "identifier_type", "secret"]
            .iter()
            .any(|k| record.get(*k) != before.get(*k));
        // A changed item is written again everywhere (Hermes has no update): taken back, then given.
        let to_give: Vec<String> = wanted.iter().filter(|a| !had.contains(a) || changed).cloned().collect();
        let give = Self::check(config, &to_give)?;
        // only an agent that gets this login now: one that already holds it was confirmed when it got it
        let added: Vec<AgentConfig> = give.iter().filter(|a| !had.contains(&a.name)).cloned().collect();
        Self::confirmed(config, body, &record, &added)?;
        let to_take: Vec<String> = had.iter().filter(|a| !wanted.contains(a) || changed).cloned().collect();
        self.take_back(config, &mut record, &to_take).await;
        self.give(config, &mut record, &give).await?;
        record.insert("updated_at".into(), json!(now()));
        records[index] = record.clone();
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    pub async fn delete(&self, config: &Config, item_id: &str) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        let records = self.read()?;
        let Some(mut record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)).cloned()
        else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let all: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
        self.take_back(config, &mut record, &all).await;
        let kept: Vec<Dict> =
            records.into_iter().filter(|r| r.get("id").and_then(Value::as_str) != Some(item_id)).collect();
        self.write(&kept)
    }

    /// A card added while payments are off (or kept on an install that forbids them): refused. Hermes' own `payment`
    /// items are never kept: they would hold the security code, in the agent's own vault (cards replace them).
    fn cards_allowed(config: &Config, record: &Dict, creating: bool) -> Result<(), VaultError> {
        let kind = record.get("kind").and_then(Value::as_str).unwrap_or("");
        if kind == "payment" {
            return Err(VaultError::Invalid("payment items are not kept: add a card".into()));
        }
        let on = if creating { crate::cards::enabled(config) } else { crate::cards::allowed(config) };
        if kind == crate::cards::KIND && !on {
            return Err(VaultError::CardsOff);
        }
        Ok(())
    }

    // -- cards (card_pay.rs) ---------------------------------------------------------------------------

    /// The card `label` that `agent` may use (case and spaces ignored): its id, label, public part and numbers.
    /// Not found: the labels of the cards it may use, for the agent.
    pub fn card_for(&self, agent: &str, label: &str) -> anyhow::Result<Result<CardFound, Vec<String>>> {
        let wanted = pyx::strip(label).to_lowercase();
        let mut labels = Vec::new();
        for record in self.read()? {
            let allowed = matches!(record.get("agents"), Some(Value::Object(a)) if a.contains_key(agent));
            if record.get("kind").and_then(Value::as_str) != Some(crate::cards::KIND) || !allowed {
                continue;
            }
            let name = record.get("label").map(crate::py::py_str).unwrap_or_default();
            if pyx::strip(&name).to_lowercase() == wanted {
                let object = |key: &str| match record.get(key) {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Dict::new(),
                };
                return Ok(Ok(CardFound {
                    id: record.get("id").map(crate::py::py_str).unwrap_or_default(),
                    label: name,
                    meta: object("card"),
                    secret: object("secret"),
                }));
            }
            labels.push(name);
        }
        Ok(Err(labels))
    }

    /// The cards `agent` may use: label and public part.
    pub fn cards_of(&self, agent: &str) -> Vec<(String, Value)> {
        self.items()
            .into_iter()
            .filter(|i| i.kind == crate::cards::KIND && i.agents.contains_key(agent))
            .map(|i| (i.label.clone(), i.card.clone().unwrap_or(Value::Null)))
            .collect()
    }

    /// A payment made with card `item_id`, kept on it (a single-use card is then used).
    pub async fn record_card_payment(&self, item_id: &str, payment: Value) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let mut meta = match record.get("card") {
            Some(Value::Object(m)) => m.clone(),
            _ => Dict::new(),
        };
        crate::cards::record_payment(&mut meta, payment);
        record.insert("card".into(), Value::Object(meta));
        self.write(&records)
    }

    // -- logins created by an agent (vault_logins.rs) ---------------------------------------------------------

    /// The login kept for `origin` and `identifier` (case ignored), if any.
    pub fn login_for(&self, origin: &str, identifier: &str) -> anyhow::Result<Option<Item>> {
        let wanted = pyx::strip(identifier).to_lowercase();
        Ok(self.read()?.iter().map(Self::item).find(|i| {
            i.kind == "login"
                && i.origin.as_deref() == Some(origin)
                && i.identifier.as_deref().map(|x| pyx::strip(x).to_lowercase()).as_deref() == Some(wanted.as_str())
        }))
    }

    /// A login `agent` created with a password of the vault, kept « en attente de confirmation » and written into that
    /// agent's own Hermes vault (so it can log in with `browser_vault_fill` later). The one it created before for the
    /// same site and identifier, still waiting, is replaced (a password the site refused). Never another one.
    pub async fn save_created_login(
        &self,
        config: &Config,
        agent: &AgentConfig,
        origin: &str,
        identifier: &str,
        password: &str,
    ) -> anyhow::Result<Item> {
        if let Some(why) = holder(config, agent) {
            return Err(VaultError::CannotHold([(agent.name.clone(), why)].into_iter().collect()).into());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let wanted = pyx::strip(identifier).to_lowercase();
        let index = records.iter().position(|r| {
            r.get("kind").and_then(Value::as_str) == Some("login")
                && r.get("origin").and_then(Value::as_str) == Some(origin)
                && r.get("identifier").and_then(Value::as_str).map(|x| pyx::strip(x).to_lowercase()).as_deref()
                    == Some(wanted.as_str())
        });
        let host = origin.split("://").nth(1).unwrap_or(origin).to_string();
        let mut body = Dict::new();
        body.insert("kind".into(), json!("login"));
        body.insert("label".into(), json!(host));
        body.insert("site".into(), json!(origin));
        body.insert("identifier_type".into(), json!(identifier_type_of(identifier)));
        body.insert("identifier".into(), json!(identifier));
        body.insert("password".into(), json!(password));
        let mut record = match index {
            Some(at) => {
                let before = records[at].clone();
                let item = Self::item(&before);
                if item.created_by.as_deref() != Some(agent.name.as_str()) || !item.pending {
                    return Err(invalid("this login is already in the vault").into());
                }
                let mut record = Self::parse(&body, Some(&before))?;
                let had: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
                self.take_back(config, &mut record, &had).await;
                record
            }
            None => {
                let mut record = Self::parse(&body, None)?;
                record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
                record.insert("agents".into(), json!({}));
                record.insert("created_at".into(), json!(now()));
                record
            }
        };
        record.insert("created_by".into(), json!(agent.name));
        record.insert("pending".into(), json!(true));
        record.insert("confirmed_at".into(), Value::Null);
        record.insert("updated_at".into(), json!(now()));
        if let Err(exc) = self.give(config, &mut record, std::slice::from_ref(agent)).await {
            let given: Vec<String> = Self::handles(&mut record).keys().cloned().collect();
            self.take_back(config, &mut record, &given).await;
            return Err(exc);
        }
        match index {
            Some(at) => records[at] = record.clone(),
            None => records.push(record.clone()),
        }
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    /// The site accepted the sign-up: the login is no longer « en attente de confirmation ». `agent`: only one it
    /// created (the person confirms any).
    pub async fn confirm_login(&self, item_id: &str, agent: Option<&str>) -> anyhow::Result<Item> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let Some(record) = records.iter_mut().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let item = Self::item(record);
        if item.created_by.is_none() || agent.is_some_and(|a| item.created_by.as_deref() != Some(a)) {
            return Err(NoSuchItem(item_id.to_string()).into());
        }
        if item.pending {
            record.insert("pending".into(), json!(false));
            record.insert("confirmed_at".into(), json!(now()));
            record.insert("updated_at".into(), json!(now()));
        }
        let item = Self::item(record);
        self.write(&records)?;
        Ok(item)
    }

    /// The password of a login an agent created, for the person (the app asks for Face ID first): they never typed
    /// it, so it is the only way they can know it. Items the person entered stay write-only.
    pub fn created_password(&self, item_id: &str) -> anyhow::Result<zeroize::Zeroizing<String>> {
        let records = self.read()?;
        let Some(record) = records.iter().find(|r| r.get("id").and_then(Value::as_str) == Some(item_id)) else {
            return Err(NoSuchItem(item_id.to_string()).into());
        };
        let item = Self::item(record);
        if item.kind != "login" || item.created_by.is_none() {
            return Err(VaultError::Invalid("only a login created by an agent can be shown".into()).into());
        }
        let password = record.get("secret").and_then(|s| s.get("password")).and_then(Value::as_str).unwrap_or("");
        Ok(zeroize::Zeroizing::new(password.to_string()))
    }

    /// An agent deleted: its handles go (its own vault is deleted with it).
    pub async fn forget_agent(&self, _config: &Config, agent_id: &str) -> anyhow::Result<()> {
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        // the secrets it asked for go with it (nobody else may use them)
        records.retain(|r| !Self::is_secret_of(r, agent_id, None));
        for record in records.iter_mut() {
            Self::handles(record).shift_remove(agent_id);
        }
        self.write(&records)
    }

    // -- secrets an agent asked for (secrets.rs) --------------------------------------------------------------------

    fn is_secret_of(record: &Dict, agent: &str, name: Option<&str>) -> bool {
        record.get("kind").and_then(Value::as_str) == Some(crate::secrets::VAULT_KIND)
            && record.get("created_by").and_then(Value::as_str) == Some(agent)
            && name.is_none_or(|n| record.get("label").and_then(Value::as_str) == Some(n))
    }

    /// The secret `new.name` of `agent` kept (replacing the one of that name). `value: None` (a one-time code): its
    /// name and identifier only, never a value.
    pub async fn save_secret(&self, config: &Config, agent: &AgentConfig, new: &NewSecret<'_>) -> anyhow::Result<Item> {
        if let Some(why) = holder(config, agent) {
            return Err(VaultError::CannotHold([(agent.name.clone(), why)].into_iter().collect()).into());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let index = records.iter().position(|r| Self::is_secret_of(r, &agent.name, Some(new.name)));
        let mut record = match index {
            Some(at) => records[at].clone(),
            None => {
                let mut record = Dict::new();
                record.insert("id".into(), json!(format!("item_{}", &crate::py::uuid4_hex()[..12])));
                record.insert("created_at".into(), json!(now()));
                record
            }
        };
        record.insert("kind".into(), json!(crate::secrets::VAULT_KIND));
        record.insert("label".into(), json!(new.name));
        record.insert("origin".into(), if new.origin.is_empty() { Value::Null } else { json!(new.origin) });
        let identifier = pyx::strip(new.identifier).to_string();
        if identifier.is_empty() {
            record.insert("identifier_type".into(), Value::Null);
            record.insert("identifier".into(), Value::Null);
        } else {
            record.insert("identifier_type".into(), json!(identifier_type_of(&identifier)));
            record.insert("identifier".into(), json!(pyx::head(&identifier, 200)));
        }
        record.insert("has_otp".into(), json!(false));
        record.insert("agents".into(), json!({agent.name.as_str(): CARD_HANDLE}));
        record.insert("created_by".into(), json!(agent.name));
        let last = record.get("secret_info").and_then(|i| i.get("last_used_at")).cloned().unwrap_or(Value::Null);
        record.insert(
            "secret_info".into(),
            json!({"kind": new.kind, "identifier_label": new.identifier_label, "last_used_at": last}),
        );
        let secret = match new.value {
            Some(value) => json!({"value": value}),
            None => json!({}),
        };
        record.insert("secret".into(), secret);
        record.insert("updated_at".into(), json!(now()));
        match index {
            Some(at) => records[at] = record.clone(),
            None => records.push(record.clone()),
        }
        self.write(&records)?;
        Ok(Self::item(&record))
    }

    /// `agent`'s secrets (metadata).
    pub fn secret_items(&self, agent: &str) -> Vec<Item> {
        self.read().unwrap_or_default().iter().filter(|r| Self::is_secret_of(r, agent, None)).map(Self::item).collect()
    }

    /// `agent`'s secrets with a value: (name, item, value). Only for a connector's run and for blanking.
    pub fn secret_values(&self, agent: &str) -> anyhow::Result<Vec<(String, Item, zeroize::Zeroizing<String>)>> {
        Ok(self
            .read()?
            .iter()
            .filter(|r| Self::is_secret_of(r, agent, None))
            .filter_map(|r| {
                let value = r.get("secret").and_then(|s| s.get("value")).and_then(Value::as_str)?;
                let item = Self::item(r);
                Some((item.label.clone(), item, zeroize::Zeroizing::new(value.to_string())))
            })
            .collect())
    }

    /// When `agent`'s secrets `names` were last used (a connector's run).
    pub async fn touch_secrets(&self, agent: &str, names: &[String]) -> anyhow::Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let _lock = self.lock.lock().await;
        let mut records = self.read()?;
        let mut changed = false;
        for record in records.iter_mut() {
            let name = record.get("label").and_then(Value::as_str).unwrap_or("").to_string();
            if Self::is_secret_of(record, agent, None) && names.contains(&name) {
                if let Some(Value::Object(info)) = record.get_mut("secret_info") {
                    info.insert("last_used_at".into(), json!(now()));
                    changed = true;
                }
            }
        }
        if changed {
            self.write(&records)?;
        }
        Ok(())
    }
}

/// A secret as the person gave it (secrets.rs).
pub struct NewSecret<'a> {
    pub name: &'a str,
    /// `https://connect.garmin.com`, or empty.
    pub origin: &'a str,
    pub kind: &'a str,
    pub identifier_label: &'a str,
    pub identifier: &'a str,
    /// None for a one-time code: never kept.
    pub value: Option<&'a str>,
}

/// What an identifier looks like: an e-mail, a phone number, else a user name.
pub fn identifier_type_of(identifier: &str) -> &'static str {
    if identifier.contains('@') {
        "email"
    } else if identifier.chars().all(|c| c.is_ascii_digit() || " +-.()".contains(c))
        && identifier.chars().filter(char::is_ascii_digit).count() >= 6
    {
        "phone"
    } else {
        "username"
    }
}

// -- routes (bridge key; the app asks for Face ID before calling them) ------------------------------------

fn answer(exc: anyhow::Error) -> HttpError {
    if let Some(err) = exc.downcast_ref::<VaultError>() {
        return match err {
            VaultError::CannotHold(reasons) => HttpError::json(409, json!({"code": "cannot_hold", "agents": reasons})),
            VaultError::Unprotected => HttpError::json(409, json!({"code": "unprotected_install"})),
            VaultError::CardsOff => HttpError::json(409, json!({"code": "cards_off"})),
            VaultError::ConfirmOthers(agents) => HttpError::json(409, json!({"code": crate::secrets::CONFIRM_OTHERS,
                "agents": agents, "message": crate::secrets::CONFIRM_OTHERS_TEXT})),
            VaultError::Invalid(text) => HttpError::new(400, text.clone()),
        };
    }
    if exc.downcast_ref::<NoSuchItem>().is_some() {
        return HttpError::new(404, "no such item");
    }
    HttpError::from(exc)
}

/// `await request.json()` for a route that answers its `ValueError` with a 400.
fn json_body(body: &Bytes) -> ApiResult<Value> {
    serde_json::from_slice::<Value>(body).map_err(|_| HttpError::new(400, pyx::json_error(body).unwrap_or_default()))
}

/// `body.get(...)` on what is not a dict: Python's AttributeError, a 500.
fn as_dict(value: Value) -> ApiResult<Dict> {
    match value {
        Value::Object(map) => Ok(map),
        other => Err(anyhow::anyhow!("AttributeError: {} has no attribute 'get'", crate::py::py_repr(&other)).into()),
    }
}

async fn listing(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let items: Vec<Value> = state
        .vault
        .try_items()?
        .iter()
        .map(|item| {
            let mut out = item.public();
            if let (Some(agent), Some(info)) = (&item.created_by, out.get_mut("secret")) {
                info["used_by"] = json!(crate::connectors::used_by(&config, agent, &item.label));
            }
            out
        })
        .collect();
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| {
            let why = holder(&config, a);
            let mut row = json!({"id": a.name, "name": a.display_name, "can_hold": why.is_none(), "why": why});
            if others_facing(&config, &a.name) {
                // the app asks one confirmation before giving it a login (`confirmed_others`)
                row["others"] = json!(true);
                row["others_names"] = json!(others_names(&config, &a.name));
            }
            row
        })
        .collect();
    let mut out = json!({"protected": config.hermes.isolated, "items": items, "agents": agents,
                         "cards": crate::cards::enabled(&config), "cards_allowed": crate::cards::allowed(&config)});
    if let (Some(rules), Value::Object(map)) = (crate::cards::phone_rules(&config), &mut out) {
        // A place of the offer: cards' numbers stay on the phone, a cap is required (cards.rs).
        map.insert("cards_on_phone".into(), json!(true));
        map.insert("card_cap_max_eur".into(), json!(rules.cap_max_cents / 100));
    }
    Ok(web::ok(out))
}

async fn create(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let body = json_body(&body)?;
    if !config.hermes.isolated {
        return Err(answer(VaultError::Unprotected.into()));
    }
    let body = as_dict(body)?;
    let item = state.vault.create(&config, &body).await.map_err(answer)?;
    crate::tool_diet::apply_all(&config); // its vault tools in view for the agents that now hold an item
    Ok(web::json_response(201, &item.public()))
}

async fn update(State(state): State<AppState>, Path(item_id): Path<String>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let body = json_body(&body)?;
    if !config.hermes.isolated {
        return Err(answer(VaultError::Unprotected.into()));
    }
    let body = as_dict(body)?;
    let item = state.vault.update(&config, &item_id, &body).await.map_err(answer)?;
    crate::tool_diet::apply_all(&config);
    Ok(web::ok(item.public()))
}

async fn delete(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let config = state.config();
    state.vault.delete(&config, &item_id).await.map_err(answer)?;
    crate::tool_diet::apply_all(&config);
    Ok(web::no_content())
}

/// The person confirms a login an agent created (the site accepted it).
async fn confirm(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let item = state.vault.confirm_login(&item_id, None).await.map_err(answer)?;
    tracing::info!(item = %item_id, "created login confirmed by the person");
    Ok(web::ok(item.public()))
}

/// The password of a login an agent created, shown to the person (after Face ID in the app). Never logged.
async fn reveal(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let password = state.vault.created_password(&item_id).map_err(answer)?;
    tracing::info!(item = %item_id, "created login shown to the person");
    let mut response = web::ok(json!({"password": password.as_str()}));
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `vault_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/vault", get(listing).post(create))
        .route("/vault/{item_id}", patch(update).delete(delete))
        .route("/vault/{item_id}/confirm", axum::routing::post(confirm))
        .route("/vault/{item_id}/reveal", axum::routing::post(reveal))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A protected place whose hub serves `agents`, its data in `dir`.
    fn place(dir: &std::path::Path, agents: &[&str]) -> Config {
        let mut config = Config::with_key("k".repeat(32));
        config.outbox.db_path = dir.join("outbox.db").to_string_lossy().into_owned();
        config.hermes.isolated = true;
        config.hermes.bin = "/usr/bin/hermes".into();
        config.hermes.hub_url = "http://127.0.0.1:8642".into();
        config.hermes.home = dir.join("hermes").to_string_lossy().into_owned();
        config.hermes.python = vec!["/usr/bin/python3".into()];
        for name in agents {
            let agent = AgentConfig {
                name: name.to_string(),
                hermes_url: format!("http://127.0.0.1:8642/p/{name}"),
                ..Default::default()
            };
            config.agents.insert(name.to_string(), agent);
        }
        config
    }

    fn may(config: &Config, name: &str) -> bool {
        hub_may_hold(config, &config.agents[name], &others_facing_agents(config))
    }

    #[test]
    fn a_hub_of_the_person_s_own_agents_holds_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine"]);
        assert!(!others_facing(&config, "coach"));
        assert!(may(&config, "coach") && may(&config, "cuisine"));
        // the hub's Python and the profile's home as that Python sees it
        let target = vault_target(&config, &config.agents["coach"]);
        assert_eq!(target.python, vec!["/usr/bin/python3".to_string()]);
        assert_eq!(target.hermes_home, Some(dir.path().join("hermes/profiles/coach").to_string_lossy().into_owned()));
        let mut contained = config.clone();
        contained.hermes.container_home = "/opt/data".into();
        assert_eq!(vault_target(&contained, &config.agents["coach"]).hermes_home.as_deref(), Some("/opt/data/profiles/coach"));
        // not on an unprotected install, nor without the hub's Python
        let mut open = config.clone();
        open.hermes.isolated = false;
        assert!(!may(&open, "coach"));
        assert_eq!(holder(&open, &open.agents["coach"]).as_deref(), Some("unprotected_install"));
        let mut no_python = config.clone();
        no_python.hermes.python.clear();
        assert!(!may(&no_python, "coach"));
        assert_eq!(holder(&no_python, &no_python.agents["coach"]).as_deref(), Some("needs_own_space"));
    }

    #[test]
    fn an_others_facing_agent_in_the_hub_keeps_everyone_moving() {
        for which in ["family", "project", "peer"] {
            let dir = tempfile::tempdir().unwrap();
            let config = place(dir.path(), &["coach", "fete"]);
            let (file, text) = match which {
                "family" => ("family-agent.json", r#"{"agent": "fete"}"#),
                "project" => (
                    "project-agents.json",
                    r#"{"projects": {"fete": {"created": 1.0, "ends": 9999999999.0, "archived": null}}}"#,
                ),
                _ => ("peers.json", r#"{"links": [["cuisine__lea", "fete"]]}"#),
            };
            std::fs::write(dir.path().join(file), text).unwrap();
            assert!(others_facing(&config, "fete"), "{which}");
            assert!(!others_facing(&config, "coach"), "{which}");
            assert!(!may(&config, "fete") && !may(&config, "coach"), "{which}");
            assert_eq!(holder(&config, &config.agents["coach"]).as_deref(), Some("needs_own_space"), "{which}");
            assert_eq!(holder(&config, &config.agents["fete"]).as_deref(), Some("needs_own_space"), "{which}");
            // the start's tidy moves it out (own_space.rs), never on an unprotected install
            assert_eq!(crate::own_space::to_tidy(&config), vec!["fete".to_string()], "{which}");
            let mut open = config.clone();
            open.hermes.isolated = false;
            assert!(crate::own_space::to_tidy(&open).is_empty(), "{which}");
            // once the others-facing agent lives in its own space, the hub is private again
            let mut moved = config.clone();
            let fete = moved.agents.get_mut("fete").unwrap();
            fete.isolated = true;
            fete.space = Some("/srv/spaces/fete".into());
            fete.hermes_url = "http://127.0.0.1:8700".into();
            assert!(may(&moved, "coach"), "{which}");
            assert!(crate::own_space::to_tidy(&moved).is_empty(), "{which}");
        }
        // a link between two of the person's own agents changes nothing
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine"]);
        std::fs::write(dir.path().join("peers.json"), r#"{"links": [["coach", "cuisine"]]}"#).unwrap();
        assert!(may(&config, "coach") && !others_facing(&config, "cuisine"));
    }

    #[test]
    fn who_else_talks_to_an_agent_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "cuisine", "notes"]);
        let data = std::path::PathBuf::from(config.data_dir());
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("households.json"),
                       r#"{"households": {"lea": {"name": "Léa"}, "marc": {"name": "Marc"}, "anon": {}}}"#).unwrap();
        std::fs::write(data.join("agent-shares.json"), r#"{"shared": {"coach": ["lea", "gone"]}}"#).unwrap();
        std::fs::write(data.join("peers.json"),
                       r#"{"links": [["cuisine__marc", "cuisine"], ["cuisine", "x__anon"], ["coach", "notes"]]}"#).unwrap();
        assert!(others_facing(&config, "coach") && others_facing(&config, "cuisine"));
        assert_eq!(others_names(&config, "coach"), vec!["Léa".to_string()]);
        assert_eq!(others_names(&config, "Cuisine"), vec!["Marc".to_string()]); // a household with no name: left out
        assert!(others_names(&config, "notes").is_empty() && !others_facing(&config, "notes"));
    }

    #[test]
    fn credentials_left_in_a_hub_no_longer_private_are_listed() {
        let dir = tempfile::tempdir().unwrap();
        let config = place(dir.path(), &["coach", "fete"]);
        let records: Vec<Dict> = vec![json!({"agents": {"coach": "vault_1"}}).as_object().unwrap().clone()];
        assert!(hub_holders_at_risk(&config, &records).is_empty());
        std::fs::write(dir.path().join("family-agent.json"), r#"{"agent": "fete"}"#).unwrap();
        assert_eq!(hub_holders_at_risk(&config, &records), vec!["coach".to_string()]);
        let cards: Vec<Dict> = vec![json!({"agents": {"coach": CARD_HANDLE}}).as_object().unwrap().clone()];
        assert!(hub_holders_at_risk(&config, &cards).is_empty());
    }

    #[test]
    fn origins_as_typed() {
        assert_eq!(normalize_origin("HTTPS://Example.com:443/a?b").unwrap(), "https://example.com");
        assert_eq!(normalize_origin("example.com:8443").unwrap(), "https://example.com:8443");
        assert_eq!(normalize_origin("sncf-connect.com/login").unwrap(), "https://sncf-connect.com");
        assert!(
            matches!(normalize_origin("ftp://x"), Err(VaultError::Invalid(t)) if t == "site must be a web address")
        );
        assert!(matches!(normalize_origin(" "), Err(VaultError::Invalid(t)) if t == "site is required"));
        assert!(
            matches!(normalize_origin("x.com:abc"), Err(VaultError::Invalid(t)) if t == "Port could not be cast to integer value as 'abc'")
        );
    }

    #[test]
    fn the_record_keeps_python_s_order() {
        let body = json!({"label": "SNCF", "site": "sncf-connect.com/login", "identifier": "a@b.c", "password": "p", "otp_secret": " x "});
        let record = Vault::parse(body.as_object().unwrap(), None).unwrap();
        let keys: Vec<&String> = record.keys().collect();
        assert_eq!(keys, ["kind", "label", "origin", "identifier_type", "identifier", "has_otp", "secret"]);
        assert_eq!(record["secret"], json!({"password": "p", "otp_secret": "x"}));
        let again = Vault::parse(json!({"otp_secret": ""}).as_object().unwrap(), Some(&record)).unwrap();
        assert_eq!(again["secret"], json!({"password": "p"}));
        assert_eq!(again["has_otp"], json!(false));
    }
}
