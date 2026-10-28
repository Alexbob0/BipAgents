//! Connectors an agent makes on request (owner decision 2026-10-28, internal/next/agent-connectors.md; the apps' API:
//! internal/next/agent-connectors-api.md). « Crée-moi un connecteur Garmin »: the agent builds a small MCP server (or a
//! script) in its own sandbox, asks for the password with `ask_secret` (secrets.rs: a card, the vault), then registers
//! it with `connector_register`: ONE approval card says what it does, the sites it reaches, the secrets it uses and
//! when it runs. Approved, it appears in the agent's tools in the apps (icon, switch, detail page) and in its Hermes as
//! an MCP entry of its own, `bipconn_<slug>`, served by the bridge, which runs the connector in the agent's sandbox for
//! each call (connectors_run.rs).
//!
//! What holds « l'agent ne voit pas ton mot de passe », all three together:
//! - **the fingerprint**: approval pins a SHA-256 over the connector's folder and runs a copy of exactly those files;
//!   any change (`connector_update`, or files changed in its folder, found before each run) means « à réapprouver »
//!   and no run until the person approves again;
//! - **the blanking**: whatever the connector sends back has every secret value (raw, URL-encoded, base64, hex)
//!   replaced by « [secret] » before Hermes, the model, the history or a log line sees it;
//! - **the allowed sites**: its container reaches only the hosts on its card, through the egress proxy's strict side.
//!
//! Tools of the agents (MCP entry `bipagents_connectors`, same token as `bipagents`), for agents that may hold
//! credentials and whose sandbox is isolated — never the family's agent nor a project agent: `ask_secret`,
//! `connector_register`, `connector_update`, `connector_remove`, `connector_list`.
//!
//! `<data>/connectors/connectors.json` (0600): per agent, its connectors (and each one's proxy token);
//! `<data>/connectors/<agent>/<id>/code/` the approved copy, `…/icon` its site's icon.
//!
//! No Python counterpart.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get as get_route, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Notify;
use unicode_normalization::UnicodeNormalization;

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::card_pay::{Asker, ToolServer};
use crate::config::{AgentConfig, Config};
use crate::connectors_run as runner;
use crate::secrets;
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const SERVER_NAME: &str = "bipagents_connectors";
pub const PATH: &str = "/v1/agent-tools/connectors";
/// Each approved connector's own MCP entry (`<base>/v1/agent-tools/connector/<id>`).
pub const CONNECTOR_PATH: &str = "/v1/agent-tools/connector";
pub const ENTRY_PREFIX: &str = "bipconn_";
pub const RUN_PREFIX: &str = "bip-conn-";
pub const ASK_SECRET: &str = "ask_secret";
pub const REGISTER: &str = "connector_register";
pub const UPDATE: &str = "connector_update";
pub const REMOVE: &str = "connector_remove";
pub const LIST: &str = "connector_list";
/// Seconds Hermes waits for these tools (the person may take a while to answer a card).
pub const TOOL_TIMEOUT: u32 = 600;
/// Seconds Hermes waits for a connector's call.
pub const ENTRY_TIMEOUT: u32 = 180;
/// How long the agent's call waits for the person's approval before saying it still waits.
const ASK_WAIT: Duration = Duration::from_secs(330);
pub const MAX_CONNECTORS: usize = 20;
pub const MAX_SITES: usize = 10;
pub const MAX_SECRETS: usize = 6;

pub const PENDING: &str = "pending";
pub const APPROVED: &str = "approved";
pub const REAPPROVE: &str = "reapprove";

/// The symbols an agent picks from, shared with the apps (`sf` iOS, `material` Android).
pub const ICONS_JSON: &str = include_str!("../assets/connectors/icons.json");

static ICONS: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(ICONS_JSON).unwrap_or(json!({"icons": []})));

/// The icon entry of `key` (the default's when unknown).
pub fn icon_of(key: &str) -> Value {
    let icons = ICONS.get("icons").and_then(Value::as_array).cloned().unwrap_or_default();
    let found = icons.iter().find(|i| i.get("key").and_then(Value::as_str) == Some(key));
    let default = ICONS.get("default").and_then(Value::as_str).unwrap_or("plug");
    found.or_else(|| icons.iter().find(|i| i.get("key").and_then(Value::as_str) == Some(default))).cloned().unwrap_or(json!({}))
}

pub fn icon_keys() -> Vec<String> {
    ICONS
        .get("icons")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|i| i.get("key").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default()
}

// --- the store -------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Schedule {
    pub when: String,
    pub mechanical: bool,
    pub tool: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CardInfo {
    pub id: String,
    pub at: f64,
    /// `new`, `update` or `changed`.
    pub change: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LastRun {
    pub at: String,
    pub ok: bool,
    pub message: String,
    pub via: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: String,
    pub icon: String,
    pub sites: Vec<String>,
    /// Its folder in the agent's sandbox (`/workspace/connecteurs/garmin`).
    pub folder: String,
    pub entry: String,
    /// `mcp` or `script`.
    pub kind: String,
    pub declared_tools: Vec<ToolInfo>,
    /// Its server's `tools/list` (blanked), once approved.
    pub tools: Vec<Value>,
    pub secrets: Vec<String>,
    pub schedule: Option<Schedule>,
    pub unofficial: bool,
    pub status: String,
    pub enabled: bool,
    /// The approved files' SHA-256.
    pub fingerprint: String,
    /// The files on the card waiting.
    pub pending_fingerprint: String,
    pub card: Option<CardInfo>,
    pub proxy_token: String,
    pub created_at: String,
    pub approved_at: Option<String>,
    pub updated_at: String,
    pub last_run: Option<LastRun>,
    /// When the schedule last ran it (Unix seconds; set at approval so it doesn't run at once).
    pub last_scheduled: f64,
    pub icon_type: Option<String>,
    /// Came from a model (« Partager comme modèle », templates.rs): approved by this person only, never by its creator.
    pub from_template: bool,
    /// The secrets' kind and identifier label a model gave (none of the creator's values): the secret card uses them.
    pub secret_hints: Vec<SecretHint>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SecretHint {
    pub name: String,
    pub kind: String,
    pub identifier_label: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    pub agents: BTreeMap<String, Vec<Connector>>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("connectors")
}

fn store_path(config: &Config) -> PathBuf {
    folder(config).join("connectors.json")
}

pub fn code_dir(config: &Config, agent: &str, id: &str) -> PathBuf {
    folder(config).join(agent).join(id).join("code")
}

/// The connector's private state (0700, under the bridge's data, which no agent reaches): mounted at `/state` in its
/// runs only. Made when missing.
pub fn state_dir(config: &Config, agent: &str, id: &str) -> PathBuf {
    let path = folder(config).join(agent).join(id).join("state");
    if !path.is_dir() {
        let _ = std::fs::create_dir_all(&path);
    }
    let _ = py::chmod(&path.to_string_lossy(), 0o700);
    path
}

fn icon_path(config: &Config, agent: &str, id: &str) -> PathBuf {
    folder(config).join(agent).join(id).join("icon")
}

pub fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// A change to the store, saved (0600), then the proxy's policy (the connectors' credentials and sites).
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let out = {
        let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = load(config);
        let out = f(&mut store);
        let body = serde_json::to_vec_pretty(&store).map_err(std::io::Error::other)?;
        let path = store_path(config);
        std::fs::create_dir_all(folder(config))?;
        py::chmod(&folder(config).to_string_lossy(), 0o700)?;
        let tmp = path.with_extension("part");
        py::write_with_mode(&tmp.to_string_lossy(), &body, 0o600)?;
        std::fs::rename(&tmp, &path)?;
        out
    };
    if let Err(exc) = crate::sandbox_sites::ensure_policy(config) {
        tracing::warn!(target: "bipbridge::connectors", error = %exc, "proxy policy not written");
    }
    Ok(out)
}

pub fn get(config: &Config, agent: &str, id: &str) -> Option<Connector> {
    load(config).agents.get(agent)?.iter().find(|c| c.id == id).cloned()
}

fn find<'a>(store: &'a mut Store, agent: &str, id: &str) -> Option<&'a mut Connector> {
    store.agents.get_mut(agent)?.iter_mut().find(|c| c.id == id)
}

pub fn of_agent(config: &Config, agent: &str) -> Vec<Connector> {
    load(config).agents.get(agent).cloned().unwrap_or_default()
}

pub fn all(config: &Config) -> Vec<(String, Connector)> {
    load(config).agents.into_iter().flat_map(|(a, list)| list.into_iter().map(move |c| (a.clone(), c))).collect()
}

/// The name a connector gives the proxy.
pub fn proxy_name(id: &str) -> String {
    format!("conn-{id}")
}

/// The proxy's entries for the connectors that may run: their credentials' hash, their hosts, strict.
pub fn proxy_identities(config: &Config) -> Vec<(String, Value)> {
    all(config)
        .into_iter()
        .filter(|(_, c)| c.status == APPROVED && c.enabled && !c.proxy_token.is_empty())
        .map(|(_, c)| {
            (
                proxy_name(&c.id),
                json!({"key": crate::agent_tools::digest(&c.proxy_token), "hosts": c.sites, "connector": true}),
            )
        })
        .collect()
}

/// The connectors that use secret `name` of `agent` (the vault's listing).
pub fn used_by(config: &Config, agent: &str, name: &str) -> Vec<Value> {
    of_agent(config, agent)
        .into_iter()
        .filter(|c| c.secrets.iter().any(|s| s == name))
        .map(|c| json!({"id": c.id, "name": c.name}))
        .collect()
}

pub fn set_last_run(config: &Config, agent: &str, id: &str, ok: bool, message: &str, via: &str) {
    let _ = change(config, |store| {
        if let Some(c) = find(store, agent, id) {
            c.last_run =
                Some(LastRun { at: crate::vault::now(), ok, message: crate::browsers::pyx::head(message, 300).to_string(), via: via.into() });
        }
    });
}

pub fn set_scheduled(config: &Config, agent: &str, id: &str, at: f64) {
    let _ = change(config, |store| {
        if let Some(c) = find(store, agent, id) {
            c.last_scheduled = at;
        }
    });
}

pub fn save_icon(config: &Config, agent: &str, id: &str, data: &[u8]) {
    let path = icon_path(config, agent, id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if py::write_with_mode(&path.to_string_lossy(), data, 0o600).is_ok() {
        let kind = crate::favicons::image_type(data).to_string();
        let _ = change(config, |store| {
            if let Some(c) = find(store, agent, id) {
                c.icon_type = Some(kind);
            }
        });
    }
}

// --- who gets them ---------------------------------------------------------------------------------------------------

/// Whether `agent` may make connectors: it may hold credentials, its sandbox is isolated, and it is neither the
/// family's agent nor a project agent (both closed).
pub fn offered(config: &Config, agent: &AgentConfig) -> bool {
    crate::vault::holder(config, agent).is_none()
        && !crate::family_agent::is_family(config, &agent.name)
        && !crate::project_agent::is_project(config, &agent.name)
        && crate::sandbox::isolates(config, agent)
}

fn base_url(config: &Config) -> String {
    let base = crate::agent_tools::url_with(config, "");
    base.split('?').next().unwrap_or("").trim_end_matches(crate::agent_tools::PATH).to_string()
}

pub fn tools_url(config: &Config) -> String {
    format!("{}{PATH}?v={}", base_url(config), crate::agent_tools::version_of(&tools()))
}

pub fn entry_name(slug: &str) -> String {
    format!("{ENTRY_PREFIX}{slug}")
}

fn connector_url(config: &Config, conn: &Connector) -> String {
    format!("{}{CONNECTOR_PATH}/{}?v={}", base_url(config), conn.id, &conn.fingerprint[..conn.fingerprint.len().min(12)])
}

/// The connectors' tools kept out of Hermes' tool search (tool_diet.rs): an approved connector is called like any tool.
pub fn direct_entries(config: &Config, agent: &AgentConfig) -> Vec<String> {
    of_agent(config, &agent.name)
        .iter()
        .filter(|c| c.status == APPROVED)
        .flat_map(|c| {
            c.tools
                .iter()
                .filter_map(|t| t.get("name").and_then(Value::as_str))
                .map(|t| format!("!mcp__{}__{t}", entry_name(&c.slug)))
                .collect::<Vec<_>>()
        })
        .take(24)
        .collect()
}

/// The `bipconn_*` entries taken out of the agent's profile when they're not wanted any more.
fn remove_entries(config: &Config, agent: &AgentConfig, keep: &[String]) -> bool {
    use crate::agents_admin::pyyaml;
    let Some(folder) = crate::shares::profile_dir(config, agent) else { return false };
    let path = folder.join("config.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else { return false };
    let Ok(mut data) = pyyaml::load(&text) else { return false };
    let Some(servers) = data.get_mut("mcp_servers").filter(|s| s.is_map()) else { return false };
    let names: Vec<String> = match &*servers {
        pyyaml::Yaml::Map(pairs) => pairs
            .iter()
            .filter_map(|(k, _)| k.as_str().map(str::to_string))
            .filter(|k| k.starts_with(ENTRY_PREFIX) && !keep.contains(k))
            .collect(),
        _ => Vec::new(),
    };
    if names.is_empty() {
        return false;
    }
    for name in &names {
        servers.remove(name);
    }
    let written = std::fs::write(&path, pyyaml::dump(&data)).is_ok();
    if written {
        tracing::info!(target: "bipbridge::connectors", agent = %agent.name, entries = %names.join(","), "connector entries removed");
    }
    written
}

/// The agent's Hermes knows the connectors' tools and each approved connector (entries next to `bipagents`, its
/// token); entries of connectors gone are taken out. Returns whether its profile changed.
pub async fn register(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<bool> {
    if !offered(config, agent) {
        return Ok(remove_entries(config, agent, &[]));
    }
    let mut changed =
        crate::card_pay::register_entry(config, agent, run, SERVER_NAME, &tools_url(config), TOOL_TIMEOUT).await?;
    let mut keep = Vec::new();
    for conn in of_agent(config, &agent.name).iter().filter(|c| c.status == APPROVED) {
        let name = entry_name(&conn.slug);
        changed |= crate::card_pay::register_entry(config, agent, run, &name, &connector_url(config, conn), ENTRY_TIMEOUT)
            .await?;
        keep.push(name);
    }
    changed |= remove_entries(config, agent, &keep);
    Ok(changed)
}

async fn register_now(state: &AppState, agent: &AgentConfig) {
    let config = state.config();
    if let Err(exc) = register(&config, agent, &crate::agents_admin::default_run()).await {
        tracing::warn!(target: "bipbridge::connectors", agent = %agent.name, error = %crate::agents_admin::tail(&exc.to_string(), 200), "connector entries not written");
    }
    let _ = crate::tool_diet::apply(&config, agent);
}

// --- the cards -------------------------------------------------------------------------------------------------------

static WAITERS: LazyLock<Mutex<BTreeMap<String, (Arc<Notify>, Option<bool>)>>> = LazyLock::new(Default::default);

fn waiters() -> std::sync::MutexGuard<'static, BTreeMap<String, (Arc<Notify>, Option<bool>)>> {
    WAITERS.lock().unwrap_or_else(|e| e.into_inner())
}

fn shown_name(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(agent.to_string())
}

pub fn schedule_text(schedule: &Option<Schedule>) -> String {
    match schedule.as_ref().and_then(|s| runner::parse_when(&s.when).ok()) {
        Some(when) => runner::when_text(&when),
        None => "Sur demande".into(),
    }
}

/// The card's sentence.
pub fn sentence(shown: &str, conn: &Connector, change: &str) -> String {
    match change {
        "new" => format!("{shown} a créé le connecteur {}", conn.name),
        "update" => format!("{shown} a modifié le connecteur {}", conn.name),
        "template" => format!("{shown} : le connecteur {} vient d'un modèle, à approuver", conn.name),
        _ => format!("Le connecteur {} de {shown} a changé : à réapprouver", conn.name),
    }
}

fn secrets_view(state: Option<&AppState>, agent: &str, conn: &Connector) -> Vec<Value> {
    let items = state.map(|s| s.vault.secret_items(agent)).unwrap_or_default();
    let values: Vec<String> = state
        .and_then(|s| s.vault.secret_values(agent).ok())
        .map(|v| v.into_iter().map(|(n, _, _)| n).collect())
        .unwrap_or_default();
    conn.secrets
        .iter()
        .map(|name| {
            let item = items.iter().find(|i| &i.label == name);
            let info = item.and_then(|i| i.secret_info.clone()).unwrap_or(json!({}));
            let kind = info.get("kind").and_then(Value::as_str).unwrap_or("password").to_string();
            let saved = if kind == "code" { secrets::code_waiting(agent, name) } else { values.contains(name) };
            json!({"name": name, "kind": kind, "identifier": item.and_then(|i| i.identifier.clone()),
                   "identifier_label": info.get("identifier_label").cloned().unwrap_or(Value::Null),
                   "saved": saved, "last_used_at": info.get("last_used_at").cloned().unwrap_or(Value::Null)})
        })
        .collect()
}

fn icon_url(agent: &str, conn: &Connector) -> Value {
    match conn.icon_type {
        Some(_) => json!(format!("/v1/agents/{agent}/connectors/{}/icon", conn.id)),
        None => Value::Null,
    }
}

fn tool_names(conn: &Connector) -> Vec<String> {
    let from_server: Vec<String> =
        conn.tools.iter().filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string)).collect();
    if from_server.is_empty() {
        conn.declared_tools.iter().map(|t| t.name.clone()).collect()
    } else {
        from_server
    }
}

fn card_item(state: Option<&AppState>, config: &Config, agent: &str, conn: &Connector, card: &CardInfo) -> Dict {
    let item = json!({
        "agent": agent,
        "run_id": format!("{RUN_PREFIX}{}", card.id),
        "request_id": card.id,
        "command": format!("{} · {}", conn.name, conn.sites.join(", ")),
        "description": sentence(&shown_name(config, agent), conn, &card.change),
        "choices": ["once", "deny"],
        "created_at": card.at,
        "session_id": null,
        "bip_connector_id": conn.id,
        "bip_connector_name": conn.name,
        "bip_connector_description": conn.description,
        "bip_connector_sites": conn.sites,
        "bip_connector_secrets": secrets_view(state, agent, conn),
        "bip_connector_schedule": schedule_text(&conn.schedule),
        "bip_connector_tools": tool_names(conn),
        "bip_connector_icon": {"symbol": icon_of(&conn.icon).get("key").cloned().unwrap_or(Value::Null),
                               "image": icon_url(agent, conn)},
        "bip_connector_unofficial": conn.unofficial,
        "bip_connector_private_state": true,
        "bip_connector_change": card.change,
        "bip_connector_from_template": conn.from_template,
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The connector cards waiting (`GET /v1/approvals`).
pub fn pending_items(state: &AppState, config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut out = Vec::new();
    for (name, list) in load(config).agents {
        if agent.is_some_and(|a| a != name) {
            continue;
        }
        for conn in &list {
            if let Some(card) = &conn.card {
                out.push(card_item(Some(state), config, &name, conn, card));
            }
        }
    }
    out.sort_by(|a, b| {
        let at = |d: &Dict| d.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
        at(a).total_cmp(&at(b))
    });
    out
}

pub(crate) fn new_card(change: &str) -> CardInfo {
    CardInfo { id: format!("c{}", &py::uuid4_hex()[..12]), at: py::time_time(), change: change.into() }
}

pub(crate) async fn announce(state: &AppState, agent: &str, conn: &Connector, card: &CardInfo) {
    let config = state.config();
    let line = sentence(&shown_name(&config, agent), conn, &card.change);
    tracing::info!(target: "bipbridge::connectors", agent, connector = %conn.id, change = %card.change, card = %card.id, "connector card raised");
    waiters().insert(card.id.clone(), (Arc::new(Notify::new()), None));
    state.push.notify_bridge_card(agent, &line, "connector_request", &card.id).await;
}

/// The connector's files changed since approved (found before a run): « à réapprouver », a card for the person.
pub async fn mark_changed(state: &AppState, agent: &AgentConfig, id: &str, now: &str) {
    let config = state.config();
    let raised = change(&config, |store| {
        let conn = find(store, &agent.name, id)?;
        conn.status = REAPPROVE.into();
        conn.pending_fingerprint = now.to_string();
        conn.updated_at = crate::vault::now();
        if conn.card.is_some() {
            return None;
        }
        let card = new_card("changed");
        conn.card = Some(card.clone());
        Some((conn.clone(), card))
    })
    .ok()
    .flatten();
    tracing::warn!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, "connector's files changed since approval: not run until approved again");
    if let Some((conn, card)) = raised {
        announce(state, &agent.name, &conn, &card).await;
    }
}

/// How a card's answer went.
#[derive(Debug, PartialEq)]
pub enum Answered {
    Approved,
    Refused,
    NotFound,
    /// Its files changed between the card and the answer: a new card replaces it.
    Changed,
}

fn record(config: &Config, agent: &str, conn: &Connector, card: &CardInfo, allow: bool, via: &str) {
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
        "command": format!("{} · {}", conn.name, conn.sites.join(", ")),
        "description": sentence(&shown_name(config, agent), conn, &card.change),
        "topic": "connector", "choice": if allow { "once" } else { "deny" }, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(agent, error = %exc, "connector decision not kept");
        }
    }
}

fn wake(card: &str, approved: bool) {
    if let Some((notify, answer)) = waiters().get_mut(card) {
        *answer = Some(approved);
        notify.notify_waiters();
    }
}

/// The person answered connector card `card_id` (the app or a notification).
pub async fn answer(state: &AppState, card_id: &str, allow: bool, via: &str) -> Answered {
    let config = state.config();
    let Some((agent, conn)) = all(&config).into_iter().find(|(_, c)| c.card.as_ref().is_some_and(|k| k.id == card_id))
    else {
        return Answered::NotFound;
    };
    let Some(agent_cfg) = config.agent(Some(&agent)).cloned() else { return Answered::NotFound };
    let card = conn.card.clone().unwrap_or_default();
    if !allow {
        record(&config, &agent, &conn, &card, false, via);
        let first = conn.approved_at.is_none();
        let _ = change(&config, |store| {
            if first {
                if let Some(list) = store.agents.get_mut(&agent) {
                    list.retain(|c| c.id != conn.id);
                }
            } else if let Some(c) = find(store, &agent, &conn.id) {
                c.card = None;
            }
        });
        if first {
            let _ = std::fs::remove_dir_all(folder(&config).join(&agent).join(&conn.id));
        }
        tracing::info!(target: "bipbridge::connectors", agent = %agent, connector = %conn.id, "connector refused by the person");
        wake(card_id, false);
        return Answered::Refused;
    }
    // The files approved are the ones on the card: checked again, then copied and checked once more.
    let code = code_dir(&config, &agent, &conn.id);
    let source = runner::workspace(&config, &agent_cfg, &conn.folder);
    let wanted = conn.pending_fingerprint.clone();
    let pinned = tokio::task::spawn_blocking(move || -> Result<String, String> {
        match source {
            Some(source) => {
                let now = runner::fingerprint(&source)?;
                if now != wanted {
                    return Ok(now);
                }
                runner::snapshot(&source, &code)
            }
            // its folder is gone (the sandbox reset): the copy kept, if it is the files on the card
            None => runner::fingerprint(&code),
        }
    })
    .await
    .ok()
    .and_then(Result::ok);
    let Some(pinned) = pinned.filter(|p| *p == conn.pending_fingerprint) else {
        // changed meanwhile: a new card with what is there now
        let fresh = runner::workspace_fingerprint(&config, &agent_cfg, &conn.folder).await.and_then(Result::ok);
        let replaced = change(&config, |store| {
            let c = find(store, &agent, &conn.id)?;
            c.pending_fingerprint = fresh.clone().unwrap_or_default();
            c.status = if c.approved_at.is_some() { REAPPROVE.into() } else { PENDING.into() };
            let card = new_card("changed");
            c.card = Some(card.clone());
            Some((c.clone(), card))
        })
        .ok()
        .flatten();
        if let Some((c, card)) = replaced {
            announce(state, &agent, &c, &card).await;
        }
        return Answered::Changed;
    };
    record(&config, &agent, &conn, &card, true, via);
    let _ = change(&config, |store| {
        if let Some(c) = find(store, &agent, &conn.id) {
            c.fingerprint = pinned.clone();
            c.status = APPROVED.into();
            c.enabled = true;
            c.card = None;
            c.approved_at = Some(crate::vault::now());
            c.updated_at = crate::vault::now();
            c.last_scheduled = py::time_time();
        }
    });
    tracing::info!(target: "bipbridge::connectors", agent = %agent, connector = %conn.id, fingerprint = %&pinned[..12], "connector approved by the person");
    // its tools asked of its server, then its entry in the agent's Hermes; its icon in the background
    if let Some(tools) = runner::discover(state, &agent_cfg, &conn.id).await {
        let _ = change(&config, |store| {
            if let Some(c) = find(store, &agent, &conn.id) {
                c.tools = tools;
            }
        });
    } else if conn.tools.is_empty() {
        let declared: Vec<Value> = conn
            .declared_tools
            .iter()
            .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": {"type": "object"}}))
            .collect();
        let _ = change(&config, |store| {
            if let Some(c) = find(store, &agent, &conn.id) {
                c.tools = declared;
            }
        });
    }
    register_now(state, &agent_cfg).await;
    let (s, a, id) = (state.clone(), agent_cfg.clone(), conn.id.clone());
    tokio::spawn(async move { runner::fetch_icon(&s, &a, &id).await });
    wake(card_id, true);
    Answered::Approved
}

/// The agent's call waits for the person's answer to `card` (None: not yet).
async fn wait_answer(card: &str) -> Option<bool> {
    let Some(notify) = waiters().get(card).map(|(n, _)| n.clone()) else { return None };
    let deadline = tokio::time::Instant::now() + ASK_WAIT;
    let outcome = loop {
        let wakeup = notify.notified();
        if let Some(answer) = waiters().get(card).and_then(|(_, a)| *a) {
            break Some(answer);
        }
        if tokio::time::timeout_at(deadline, wakeup).await.is_err() {
            break None;
        }
    };
    waiters().remove(card);
    outcome
}

// --- the agent's tools -----------------------------------------------------------------------------------------------

const BUILD: &str = "How to build one: write it in its own folder under /workspace/connecteurs/<name> (a small MCP \
server on stdio, Python or Node, with its tests), its dependencies vendored INSIDE that folder (e.g. pip install \
--target ./vendor; you install them yourself, never ask the person to run a command): it runs without the package \
registries. It reads its secrets from environment variables (secret « garmin » of kind password: GARMIN_USERNAME and \
GARMIN_PASSWORD; token: GARMIN_TOKEN; code: GARMIN_CODE) and never prints them, never reads a local config file for \
them. Session tokens and caches a library keeps go in its private state folder, $BIP_STATE (also its HOME and XDG \
folders; GARMINTOKENS points there): kept between its runs, never in your space, never in its own folder. Technical \
details go in a README.md in its folder; to the person, say what it does in plain words. Before testing a service, list EVERY host its library needs (Garmin: sso.garmin.com, \
connect.garmin.com, connectapi.garmin.com…) and ask for them together with sandbox_sites_request, one call right \
after the other, so the person gets ONE card; the connector's sites must be exactly those. Heavy work may go to a \
background task, but a secret never goes in a task's prompt or the conversation: only ask_secret.";

const SECRET_RULE: &str = "Never ask for a password, key or code in the conversation, and never write one in a \
message, a file, a task or a log: always ask_secret. You never see the value; the person types it in a card.";

pub fn tools() -> Vec<Value> {
    let icons = icon_keys().join(", ");
    let schedule = json!({"type": "object", "description": "when it runs by itself (leave out: on request only)", "properties": {
        "when": {"type": "string", "description": "« daily 07:00 », « weekly mon 07:00 », « hourly » or « every 30 minutes » (the person's time)"},
        "mechanical": {"type": "boolean", "description": "true: the bridge calls the tool itself, no model call (cheapest; for a plain sync). false: you put the call in one of your routines"},
        "tool": {"type": "string", "description": "the connector's tool to call"},
        "arguments": {"type": "object"},
    }});
    let fields = json!({
        "name": {"type": "string", "description": "its name for the person, e.g. « Garmin »"},
        "sites": {"type": "array", "items": {"type": "string"}, "description": "every host it reaches, e.g. [\"sso.garmin.com\", \"connect.garmin.com\"]"},
        "description": {"type": "string", "description": "what it does, one line the person understands"},
        "icon": {"type": "string", "description": format!("one of: {icons}")},
        "folder": {"type": "string", "description": "its folder in your space, e.g. /workspace/connecteurs/garmin"},
        "entry": {"type": "string", "description": "the command that starts it, run in its folder, e.g. « python3 server.py »"},
        "kind": {"type": "string", "enum": ["mcp", "script"], "description": "mcp (default): an MCP server on stdio; script: reads JSON arguments on stdin, prints its result"},
        "tools": {"type": "array", "items": {"type": "object", "properties": {"name": {"type": "string"}, "description": {"type": "string"}}}, "description": "its tools, as you wrote them"},
        "secrets": {"type": "array", "items": {"type": "string"}, "description": "the names of the secrets it uses (see ask_secret)"},
        "schedule": schedule,
        "unofficial": {"type": "boolean", "description": "true when it uses a service's unofficial API (the person is told it may be blocked)"},
    });
    let mut update_fields = fields.clone();
    if let Some(map) = update_fields.as_object_mut() {
        map.insert("new_name".into(), json!({"type": "string", "description": "a new name, if it changes"}));
    }
    vec![
        json!({
            "name": ASK_SECRET,
            "description": format!("Ask the person for a password, an access token or a one-time code a connector \
        needs. ONE card in the app asks for the identifier (for a password) and the secret together; it goes to the \
        vault, never to you nor the conversation. You get back only « saved as <name> ». A code (MFA) is used once by \
        the connector's next run and never kept. {SECRET_RULE}"),
            "inputSchema": {"type": "object", "properties": {
                "name": {"type": "string", "description": "its name: lower-case letters, digits, _ (e.g. garmin); the connector reads GARMIN_USERNAME / GARMIN_PASSWORD"},
                "site": {"type": "string", "description": "the service's site, e.g. connect.garmin.com"},
                "kind": {"type": "string", "enum": ["password", "token", "code"]},
                "identifier_label": {"type": "string", "description": "the label of the identifier field, in the person's language, e.g. « E-mail de ton compte Garmin »"},
                "identifier": {"type": "string", "description": "the identifier, only if the person already gave it (prefilled; they can change it)"},
                "reason": {"type": "string", "description": "why, in a few words the person understands"},
                "connector": {"type": "string", "description": "the connector's name, if it exists already"},
            }, "required": ["name", "site", "kind", "reason"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
        json!({
            "name": REGISTER,
            "description": format!("Register a connector you built at the person's request, so it appears in their \
        tools: ONE approval card shows what it does, its sites, its secrets and when it runs. Once approved, its tools \
        are yours (mcp__bipconn_<name>__<tool>); it runs in your isolated space with its secrets and only its sites. \
        Any later change to its files needs the person's approval again. {BUILD}"),
            "inputSchema": {"type": "object", "properties": fields,
                            "required": ["name", "sites", "description", "icon", "folder", "entry"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
        json!({
            "name": UPDATE,
            "description": "Change a connector you registered (its files, sites, secrets, schedule…): the person \
        approves it again before it runs. Give its current name and only what changes.",
            "inputSchema": {"type": "object", "properties": update_fields, "required": ["name"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
        json!({
            "name": REMOVE,
            "description": "Remove a connector you registered (its tools go; its secrets stay in the vault).",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": false},
        }),
        json!({
            "name": LIST,
            "description": "Your connectors: their state (approved, waiting, to approve again, off), tools, sites, \
        secrets (names only) and last run.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false},
        }),
    ]
}

fn text(message: &str) -> Dict {
    tool_text(message, false)
}

fn refusal(message: &str) -> Dict {
    tool_text(message, true)
}

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).filter(|v| !v.is_null()).map(py::py_str).unwrap_or_default().trim().to_string()
}

/// « Météo du Grand-Est » → `meteo_du_grand_est`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.nfkd().filter(|c| c.is_ascii()) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    let out = out.trim_end_matches('_').to_string();
    crate::browsers::pyx::head(&out, 24).trim_end_matches('_').to_string()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(py::py_str).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect(),
        _ => Vec::new(),
    }
}

/// The fields of a connector the agent gave, applied over `base` (an update): Err is the refusal's text.
fn apply_fields(base: &mut Connector, arguments: &Dict, creating: bool) -> Result<(), String> {
    let given = |k: &str| arguments.get(k).is_some_and(|v| !v.is_null());
    let name_key = if creating { "name" } else { "new_name" };
    if given(name_key) || creating {
        let name = crate::cards::quoted(&arg(arguments, name_key), 40);
        let slug = slug(&name);
        if name.is_empty() || slug.len() < 2 {
            return Err("Give it a name the person understands, e.g. « Garmin ».".into());
        }
        base.name = name;
        base.slug = slug;
    }
    if given("sites") || creating {
        let mut sites = Vec::new();
        for raw in strings(arguments.get("sites")) {
            let host = crate::sandbox_sites::clean_host(&raw, true).map_err(|why| format!("sites: {why}"))?;
            if !sites.contains(&host) {
                sites.push(host);
            }
        }
        if sites.is_empty() || sites.len() > MAX_SITES {
            return Err(format!("sites: 1 to {MAX_SITES} hosts it reaches, e.g. [\"connect.garmin.com\"]."));
        }
        base.sites = sites;
    }
    if given("description") || creating {
        base.description = crate::cards::quoted(&arg(arguments, "description"), 200);
        if base.description.is_empty() {
            return Err("description: what it does, in one line the person understands.".into());
        }
    }
    if given("icon") || creating {
        let icon = arg(arguments, "icon").to_lowercase();
        if !icon_keys().contains(&icon) {
            return Err(format!("icon: one of {}.", icon_keys().join(", ")));
        }
        base.icon = icon;
    }
    if given("folder") || creating {
        let folder = arg(arguments, "folder");
        let ok = (folder.starts_with("/workspace/") || folder.starts_with("/root/"))
            && crate::sandbox::plain_relative(folder.trim_start_matches('/')).is_some();
        if !ok {
            return Err("folder: its own folder in your space, under /workspace (e.g. /workspace/connecteurs/garmin).".into());
        }
        base.folder = folder.trim_end_matches('/').to_string();
    }
    if given("entry") || creating {
        base.entry = crate::browsers::pyx::head(&arg(arguments, "entry"), 300).to_string();
        if runner::entry_argv(base).is_err() {
            return Err("entry: the command that starts it in its folder, e.g. « python3 server.py ».".into());
        }
    }
    if given("kind") || creating {
        let kind = arg(arguments, "kind").to_lowercase();
        base.kind = if kind.is_empty() { "mcp".into() } else { kind };
        if !["mcp", "script"].contains(&base.kind.as_str()) {
            return Err("kind: mcp or script.".into());
        }
    }
    if given("tools") {
        base.declared_tools = match arguments.get("tools") {
            Some(Value::Array(items)) => items
                .iter()
                .take(64)
                .filter_map(|t| {
                    let name = t.get("name").map(py::py_str).unwrap_or_default();
                    (!name.trim().is_empty()).then(|| ToolInfo {
                        name: crate::browsers::pyx::head(name.trim(), 64).to_string(),
                        description: crate::browsers::pyx::head(&t.get("description").map(py::py_str).unwrap_or_default(), 300).to_string(),
                    })
                })
                .collect(),
            _ => Vec::new(),
        };
    }
    if given("secrets") {
        let names = strings(arguments.get("secrets"));
        if let Some(bad) = names.iter().find(|n| !secrets::valid_name(n)) {
            return Err(format!("secrets: « {bad} » is not a secret's name (lower-case letters, digits, _)."));
        }
        if names.len() > MAX_SECRETS {
            return Err(format!("secrets: {MAX_SECRETS} at most."));
        }
        base.secrets = names;
    }
    if given("schedule") {
        base.schedule = match arguments.get("schedule") {
            Some(Value::Object(map)) => {
                let when = map.get("when").map(py::py_str).unwrap_or_default();
                runner::parse_when(&when)?;
                let mechanical = map.get("mechanical").map(py::truthy).unwrap_or(false);
                let tool = map.get("tool").map(py::py_str).unwrap_or_default().trim().to_string();
                if mechanical && tool.is_empty() {
                    return Err("schedule.tool: the tool the bridge calls by itself.".into());
                }
                Some(Schedule { when, mechanical, tool, arguments: map.get("arguments").cloned().unwrap_or(json!({})) })
            }
            _ => None,
        };
    }
    if given("unofficial") {
        base.unofficial = arguments.get("unofficial").map(py::truthy).unwrap_or(false);
    }
    Ok(())
}

async fn waited(state: &AppState, card: &str, name: &str, schedule: &Option<Schedule>) -> Dict {
    let _ = state;
    match wait_answer(card).await {
        Some(true) => {
            let routine = match schedule {
                Some(s) if !s.mechanical => format!(
                    " Its schedule ({}) is yours to keep: create a routine that calls its tool {} then.",
                    s.when, s.tool
                ),
                _ => String::new(),
            };
            text(&format!(
                "The person approved « {name} »: its tools are available now (in a new conversation if you don't see \
                 them yet).{routine}"
            ))
        }
        Some(false) => refusal(&format!(
            "The person refused « {name} »: it doesn't run. Don't register it again unless they ask you to."
        )),
        None => text(&format!(
            "« {name} » waits for the person's approval in the app. Tell them it's ready to approve; don't ask again."
        )),
    }
}

async fn register_tool(state: &AppState, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let config = state.config();
    let mut conn = Connector::default();
    if let Err(why) = apply_fields(&mut conn, arguments, true) {
        return Ok(refusal(&why));
    }
    if arguments.get("tools").is_none_or(Value::is_null) {
        conn.declared_tools = Vec::new();
    }
    let existing = of_agent(&config, &agent.name);
    if existing.iter().any(|c| c.slug == conn.slug) {
        return Ok(refusal(&format!("A connector « {} » exists already: change it with {UPDATE}.", conn.name)));
    }
    if existing.len() >= MAX_CONNECTORS {
        return Ok(refusal(&format!("You have {MAX_CONNECTORS} connectors already: remove one first.")));
    }
    let fp = match runner::workspace_fingerprint(&config, agent, &conn.folder).await {
        Some(Ok(fp)) => fp,
        Some(Err(why)) => return Ok(refusal(&format!("folder: {why}."))),
        None => return Ok(refusal(&format!("folder: {} isn't in your space (write the connector there first).", conn.folder))),
    };
    conn.id = format!("c_{}", &py::uuid4_hex()[..12]);
    conn.status = PENDING.into();
    conn.pending_fingerprint = fp;
    conn.proxy_token = py::token_urlsafe(24);
    conn.created_at = crate::vault::now();
    conn.updated_at = conn.created_at.clone();
    let card = new_card("new");
    conn.card = Some(card.clone());
    change(&config, |store| store.agents.entry(agent.name.clone()).or_default().push(conn.clone()))?;
    announce(state, &agent.name, &conn, &card).await;
    Ok(waited(state, &card.id, &conn.name, &conn.schedule).await)
}

fn named(config: &Config, agent: &str, name: &str) -> Option<Connector> {
    let wanted = slug(name);
    of_agent(config, agent).into_iter().find(|c| c.slug == wanted || c.id == name)
}

async fn update_tool(state: &AppState, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let config = state.config();
    let Some(before) = named(&config, &agent.name, &arg(arguments, "name")) else {
        return Ok(refusal("You have no connector of that name (connector_list shows them)."));
    };
    let mut conn = before.clone();
    if let Err(why) = apply_fields(&mut conn, arguments, false) {
        return Ok(refusal(&why));
    }
    if conn.slug != before.slug && of_agent(&config, &agent.name).iter().any(|c| c.slug == conn.slug) {
        return Ok(refusal("Another connector has that name."));
    }
    let fp = match runner::workspace_fingerprint(&config, agent, &conn.folder).await {
        Some(Ok(fp)) => fp,
        Some(Err(why)) => return Ok(refusal(&format!("folder: {why}."))),
        None => return Ok(refusal(&format!("folder: {} isn't in your space.", conn.folder))),
    };
    if conn == before && fp == before.fingerprint && before.status == APPROVED {
        return Ok(text("Nothing changed: it stays approved."));
    }
    if before.slug != conn.slug {
        // its entry in Hermes changes name: the old one goes now
        let _ = change(&config, |store| {
            if let Some(c) = find(store, &agent.name, &before.id) {
                c.status = REAPPROVE.into();
            }
        });
        register_now(state, agent).await;
    }
    let card = new_card("update");
    conn.card = Some(card.clone());
    conn.pending_fingerprint = fp;
    conn.status = if before.approved_at.is_some() { REAPPROVE.into() } else { PENDING.into() };
    conn.updated_at = crate::vault::now();
    let stored = conn.clone();
    change(&config, |store| {
        if let Some(c) = find(store, &agent.name, &before.id) {
            *c = stored;
        }
    })?;
    if let Some(old) = &before.card {
        waiters().remove(&old.id);
    }
    announce(state, &agent.name, &conn, &card).await;
    Ok(waited(state, &card.id, &conn.name, &conn.schedule).await)
}

/// A connector removed: its entry, its files, its icon (its secrets stay in the vault).
pub async fn remove(state: &AppState, agent: &AgentConfig, id: &str) -> bool {
    let config = state.config();
    let removed = change(&config, |store| {
        let list = store.agents.entry(agent.name.clone()).or_default();
        let before = list.len();
        list.retain(|c| c.id != id);
        before != list.len()
    })
    .unwrap_or(false);
    if removed {
        let _ = std::fs::remove_dir_all(folder(&config).join(&agent.name).join(id));
        register_now(state, agent).await;
        tracing::info!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, "connector removed");
    }
    removed
}

fn listing_for_agent(state: &AppState, agent: &AgentConfig) -> Dict {
    let config = state.config();
    let list = of_agent(&config, &agent.name);
    if list.is_empty() {
        return text("You have no connector yet.");
    }
    let rows: Vec<Value> = list
        .iter()
        .map(|c| {
            json!({"name": c.name, "state": status_text(c), "tools": tool_names(c), "sites": c.sites,
                   "secrets": c.secrets, "schedule": schedule_text(&c.schedule), "folder": c.folder,
                   "last_run": c.last_run.as_ref().map(|r| json!({"at": r.at, "ok": r.ok, "message": r.message}))})
        })
        .collect();
    text(&py::dumps(&json!({"connectors": rows})))
}

async fn ask_secret_tool(state: &AppState, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let config = state.config();
    let name = arg(arguments, "name").to_lowercase();
    if !secrets::valid_name(&name) {
        return Ok(refusal("name: lower-case letters, digits and _, 2 to 32 characters, a letter first (e.g. garmin)."));
    }
    let kind = arg(arguments, "kind").to_lowercase();
    if !secrets::KINDS.contains(&kind.as_str()) {
        return Ok(refusal("kind: password, token or code."));
    }
    let site = arg(arguments, "site");
    let origin = if site.is_empty() {
        String::new()
    } else {
        match crate::vault::normalize_origin(&site) {
            Ok(o) => o,
            Err(exc) => return Ok(refusal(&format!("site: {exc}"))),
        }
    };
    let reason = crate::cards::quoted(&arg(arguments, "reason"), 300);
    if reason.is_empty() {
        return Ok(refusal("reason: why you need it, in a few words the person understands."));
    }
    // a name already kept is not asked again (the person changes it from the connector's page)
    let known = state.vault.secret_items(&agent.name);
    if kind != "code" && known.iter().any(|i| i.label == name && i.secret_info.as_ref().and_then(|s| s.get("kind")).and_then(Value::as_str) != Some("code")) {
        return Ok(refusal(&format!(
            "« {name} » is already in the vault: use it as it is. If it's wrong, the person changes it from the \
             connector's page (« Modifier le mot de passe »); pick another name only for another account."
        )));
    }
    let identifier_label = crate::cards::quoted(&arg(arguments, "identifier_label"), 80);
    let identifier = crate::cards::quoted(&arg(arguments, "identifier"), 200);
    let gave_identifier = !identifier.is_empty();
    let connector = Some(arg(arguments, "connector")).filter(|c| !c.is_empty());
    let request = secrets::Request {
        name: name.clone(),
        origin,
        kind: kind.clone(),
        identifier_label: if identifier_label.is_empty() && kind == "password" { "Identifiant".into() } else { identifier_label },
        identifier,
        reason,
        edit: false,
        connector,
    };
    let line = secrets::sentence(&shown_name(&config, &agent.name), &request);
    let id = match secrets::open(&agent.name, request) {
        Ok(id) => id,
        Err(why) => return Ok(refusal(&why)),
    };
    state.push.notify_bridge_card(&agent.name, &line, "secret_request", &id).await;
    Ok(match secrets::wait(&id).await {
        Some(secrets::Outcome::Saved { identifier }) => {
            let with = if gave_identifier { format!(" (identifier {identifier})") } else { String::new() };
            let what = if kind == "code" { "held for the connector's next run, then gone" } else { "kept in the vault" };
            text(&format!("Saved as « {name} »{with}: {what}. You never see it; your connector reads it from its environment."))
        }
        Some(secrets::Outcome::Refused) => refusal(&format!(
            "The person refused to give « {name} »: don't ask again unless they tell you to."
        )),
        None => refusal(&format!(
            "The person did not answer in time: nothing was kept. Tell them a card waits for « {name} » in the app \
             when they're ready, and call again only when they ask."
        )),
    })
}

pub async fn call(state: &AppState, agent: &AgentConfig, name: &str, arguments: &Dict, _asker: &Asker) -> anyhow::Result<Dict> {
    if !offered(&state.config(), agent) {
        return Ok(refusal(
            "Connectors need your own isolated space, where you may keep secrets: the person moves you there in the app first.",
        ));
    }
    match name {
        ASK_SECRET => ask_secret_tool(state, agent, arguments).await,
        REGISTER => register_tool(state, agent, arguments).await,
        UPDATE => update_tool(state, agent, arguments).await,
        REMOVE => {
            let config = state.config();
            match named(&config, &agent.name, &arg(arguments, "name")) {
                Some(conn) if remove(state, agent, &conn.id).await => Ok(text(&format!("« {} » is removed.", conn.name))),
                _ => Ok(refusal("You have no connector of that name.")),
            }
        }
        LIST => Ok(listing_for_agent(state, agent)),
        other => Ok(refusal(&format!("unknown tool: {other}"))),
    }
}

const INSTRUCTIONS: &str = "BipAgents connectors: build a connector the person asks for in your space, ask its \
secrets with ask_secret (never in the conversation), then connector_register: the person approves it once and it \
appears in their tools.";

fn call_boxed<'a>(
    state: &'a AppState,
    agent: &'a AgentConfig,
    name: &'a str,
    arguments: &'a Dict,
    asker: &'a Asker,
) -> futures::future::BoxFuture<'a, anyhow::Result<Dict>> {
    Box::pin(call(state, agent, name, arguments, asker))
}

static SERVER: ToolServer = ToolServer { name: SERVER_NAME, instructions: INSTRUCTIONS, tools, call: call_boxed };

async fn mcp(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    crate::card_pay::serve(state, headers, body, &SERVER).await
}

// --- each connector's MCP entry (Hermes → the bridge → its container) --------------------------------------------------

fn rpc(id: &Value, result: Value) -> Response {
    web::json_response(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

async fn connector_mcp(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();
    let config = state.config();
    let agent = if token.is_empty() { None } else { crate::agent_tools::agent_for(&config, token) };
    let Some(agent) = agent else { return Err(HttpError::new(401, "unknown agent")) };
    // only its own connectors: another agent's id is unknown here
    let Some(conn) = get(&config, &agent.name, &id) else { return Err(HttpError::new(404, "no such connector")) };
    let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&body) else {
        return Ok(web::json_response(400, &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}})));
    };
    let rid = message.get("id").cloned().unwrap_or(Value::Null);
    if rid.is_null() {
        let mut response = Response::new(Body::empty());
        *response.status_mut() = axum::http::StatusCode::ACCEPTED;
        return Ok(response);
    }
    let params = message.get("params").cloned().unwrap_or(json!({}));
    match message.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            let version = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18").to_string();
            Ok(rpc(&rid, json!({"protocolVersion": version, "capabilities": {"tools": {"listChanged": false}},
                                "serverInfo": {"name": entry_name(&conn.slug), "version": crate::VERSION},
                                "instructions": format!("{} (connecteur fait par {}, approuvé par la personne)", conn.description, agent.display_name)})))
        }
        "ping" => Ok(rpc(&rid, json!({}))),
        "tools/list" => Ok(rpc(&rid, json!({"tools": conn.tools}))),
        "tools/call" => {
            let tool = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let arguments = params.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default();
            let result = match runner::call_tool(&state, &agent, &id, &tool, arguments, "agent").await {
                Ok(result) => result,
                Err(err) => json!({"content": [{"type": "text", "text": err.agent_text()}], "isError": true}),
            };
            Ok(rpc(&rid, result))
        }
        other => Ok(web::json_response(
            200,
            &json!({"jsonrpc": "2.0", "id": rid, "error": {"code": -32601, "message": format!("method not found: {other}")}}),
        )),
    }
}

/// The agents' routes, at the root next to their other tools (same tokens).
pub fn router() -> Router<AppState> {
    Router::new().route(PATH, post(mcp)).route(&format!("{CONNECTOR_PATH}/{{id}}"), post(connector_mcp))
}

// --- the apps' routes (bridge key) -----------------------------------------------------------------------------------

pub fn status_text(conn: &Connector) -> &'static str {
    match conn.status.as_str() {
        APPROVED if conn.enabled => "Autorisé",
        APPROVED => "Coupé",
        REAPPROVE => "À réapprouver",
        _ => "En attente de ton accord",
    }
}

/// A connector as the apps show it.
pub fn view(state: &AppState, config: &Config, agent: &AgentConfig, conn: &Connector) -> Value {
    let icon = icon_of(&conn.icon);
    let tools: Vec<Value> = if conn.tools.is_empty() {
        conn.declared_tools.iter().map(|t| json!({"name": t.name, "description": t.description})).collect()
    } else {
        conn.tools
            .iter()
            .map(|t| json!({"name": t.get("name").cloned().unwrap_or(Value::Null),
                            "description": t.get("description").cloned().unwrap_or(json!(""))}))
            .collect()
    };
    let _ = config;
    json!({
        "id": conn.id, "name": conn.name, "description": conn.description, "agent": agent.name,
        "made_by": if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() },
        "icon": {"symbol": icon.get("key").cloned().unwrap_or(Value::Null), "sf": icon.get("sf").cloned().unwrap_or(Value::Null),
                 "material": icon.get("material").cloned().unwrap_or(Value::Null), "image": icon_url(&agent.name, conn)},
        "status": conn.status, "status_text": status_text(conn), "enabled": conn.enabled,
        "runs": conn.status == APPROVED && conn.enabled,
        "sites": conn.sites, "secrets": secrets_view(Some(state), &agent.name, conn), "tools": tools,
        "schedule": conn.schedule.as_ref().map(|s| json!({"when": s.when, "text": schedule_text(&conn.schedule),
                                                          "mechanical": s.mechanical, "tool": s.tool})),
        "last_run": conn.last_run.as_ref().map(|r| json!({"at": r.at, "ok": r.ok, "message": r.message})),
        "unofficial": conn.unofficial, "from_template": conn.from_template,
        "card_run_id": conn.card.as_ref().map(|c| format!("{RUN_PREFIX}{}", c.id)),
        "created_at": conn.created_at, "approved_at": conn.approved_at, "updated_at": conn.updated_at,
        "fingerprint": &conn.fingerprint[..conn.fingerprint.len().min(12)],
    })
}

fn agent_of(config: &Config, name: &str) -> ApiResult<AgentConfig> {
    config.agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "no such agent"))
}

fn connector_of(config: &Config, agent: &str, id: &str) -> ApiResult<Connector> {
    get(config, agent, id).ok_or_else(|| HttpError::new(404, "no such connector"))
}

async fn list_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let items: Vec<Value> = of_agent(&config, &agent.name).iter().map(|c| view(&state, &config, &agent, c)).collect();
    Ok(web::ok(json!({"items": items, "icons": "/v1/connectors/icons"})))
}

async fn detail_route(State(state): State<AppState>, UrlPath((agent_id, id)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let conn = connector_of(&config, &agent.name, &id)?;
    Ok(web::ok(view(&state, &config, &agent, &conn)))
}

async fn switch_route(
    State(state): State<AppState>,
    UrlPath((agent_id, id)): UrlPath<(String, String)>,
    body: Bytes,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let conn = connector_of(&config, &agent.name, &id)?;
    let body: Value = serde_json::from_slice(&body).map_err(|_| HttpError::new(400, "invalid JSON"))?;
    let Some(on) = body.get("enabled").and_then(Value::as_bool) else {
        return Err(HttpError::new(400, "enabled must be true or false"));
    };
    if on && conn.status != APPROVED {
        return Err(HttpError::json(409, json!({"code": "not_approved"})));
    }
    change(&config, |store| {
        if let Some(c) = find(store, &agent.name, &id) {
            c.enabled = on;
            c.updated_at = crate::vault::now();
        }
    })
    .map_err(anyhow::Error::from)?;
    tracing::info!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, on, "connector switched by the person");
    let conn = connector_of(&config, &agent.name, &id)?;
    Ok(web::ok(view(&state, &config, &agent, &conn)))
}

async fn delete_route(State(state): State<AppState>, UrlPath((agent_id, id)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    connector_of(&config, &agent.name, &id)?;
    remove(&state, &agent, &id).await;
    Ok(web::no_content())
}

async fn reapprove_route(State(state): State<AppState>, UrlPath((agent_id, id)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let conn = connector_of(&config, &agent.name, &id)?;
    if let Some(card) = &conn.card {
        return Ok(web::ok(json!({"run_id": format!("{RUN_PREFIX}{}", card.id)})));
    }
    let fp = match runner::workspace_fingerprint(&config, &agent, &conn.folder).await {
        Some(Ok(fp)) => fp,
        _ => conn.fingerprint.clone(),
    };
    let card = new_card("changed");
    let raised = change(&config, |store| {
        let c = find(store, &agent.name, &id)?;
        c.card = Some(card.clone());
        c.pending_fingerprint = fp.clone();
        if c.status == APPROVED && fp != c.fingerprint {
            c.status = REAPPROVE.into();
        }
        Some(c.clone())
    })
    .map_err(anyhow::Error::from)?;
    if let Some(conn) = raised {
        announce(&state, &agent.name, &conn, &card).await;
    }
    Ok(web::ok(json!({"run_id": format!("{RUN_PREFIX}{}", card.id)})))
}

/// « Modifier le mot de passe »: the same secret card, opened by the person, the identifier prefilled.
async fn edit_secret_route(
    State(state): State<AppState>,
    UrlPath((agent_id, id, name)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let conn = connector_of(&config, &agent.name, &id)?;
    if !conn.secrets.contains(&name) {
        return Err(HttpError::new(404, "this connector uses no such secret"));
    }
    let item = state.vault.secret_items(&agent.name).into_iter().find(|i| i.label == name);
    // a model's connector: the kind and label it gave, until the person's own secret is kept
    let hint = conn.secret_hints.iter().find(|h| h.name == name);
    let hinted = hint.map(|h| json!({"kind": h.kind, "identifier_label": h.identifier_label})).unwrap_or(json!({}));
    let info = item.as_ref().and_then(|i| i.secret_info.clone()).unwrap_or(hinted);
    let kind = info.get("kind").and_then(Value::as_str).unwrap_or("password").to_string();
    let origin = item
        .as_ref()
        .and_then(|i| i.origin.clone())
        .or_else(|| conn.sites.iter().find(|s| !s.starts_with("*.")).map(|s| format!("https://{s}")))
        .unwrap_or_default();
    let label = info.get("identifier_label").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("Identifiant");
    let request = secrets::Request {
        name: name.clone(),
        origin,
        kind,
        identifier_label: label.to_string(),
        identifier: item.and_then(|i| i.identifier).unwrap_or_default(),
        reason: format!("Pour le connecteur {}", conn.name),
        edit: true,
        connector: Some(conn.name.clone()),
    };
    let id = secrets::open(&agent.name, request).map_err(|why| HttpError::new(409, why))?;
    let card = secrets::item(&config, &id).unwrap_or_default();
    Ok(web::ok(json!({"run_id": format!("{}{id}", secrets::RUN_PREFIX), "card": card})))
}

async fn icon_route(State(state): State<AppState>, UrlPath((agent_id, id)): UrlPath<(String, String)>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    connector_of(&config, &agent.name, &id)?;
    let data = std::fs::read(icon_path(&config, &agent.name, &id)).map_err(|_| HttpError::new(404, "no icon"))?;
    let kind = crate::favicons::image_type(&data);
    if kind.is_empty() {
        return Err(HttpError::new(404, "no icon"));
    }
    let mut response = Response::new(Body::from(data));
    response.headers_mut().insert("content-type", axum::http::HeaderValue::from_static(kind));
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("private, max-age=86400"));
    Ok(response)
}

async fn icons_route() -> ApiResult<Response> {
    Ok(web::ok(ICONS.clone()))
}

async fn answer_route(State(state): State<AppState>, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body: Value = serde_json::from_slice(&body).map_err(|_| HttpError::new(400, "invalid JSON"))?;
    let Some(allow) = body.get("allow").and_then(Value::as_bool) else {
        return Err(HttpError::new(400, "allow must be true or false"));
    };
    answered_response(answer(&state, &id, allow, "app").await, allow)
}

/// The HTTP answer to a card's answer (the app's route and `/v1/approve`).
pub fn answered_response(outcome: Answered, allow: bool) -> ApiResult<Response> {
    match outcome {
        Answered::NotFound => Err(HttpError::new(404, "no such request (answered already?)")),
        Answered::Changed => Err(HttpError::json(409, json!({"code": "changed"}))),
        _ => Ok(web::ok(json!({"ok": true, "choice": if allow { "once" } else { "deny" }}))),
    }
}

/// The apps' routes (mounted under /v1 with the bridge key).
pub fn app_router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/connectors", get_route(list_route))
        .route("/agents/{agent_id}/connectors/{id}", get_route(detail_route).patch(switch_route).delete(delete_route))
        .route("/agents/{agent_id}/connectors/{id}/reapprove", post(reapprove_route))
        .route("/agents/{agent_id}/connectors/{id}/secrets/{name}/edit", post(edit_secret_route))
        .route("/agents/{agent_id}/connectors/{id}/icon", get_route(icon_route))
        .route("/connectors/requests/{id}", post(answer_route))
        .route("/connectors/icons", get_route(icons_route))
}

/// An agent deleted: its connectors and their files go.
pub fn forget(config: &Config, agent: &str) {
    let _ = change(config, |store| store.agents.remove(agent));
    let _ = std::fs::remove_dir_all(folder(config).join(agent));
}

/// The connectors' entries in the agent's settings (`GET /v1/agents/{id}/settings`): the mosaic's tiles.
pub fn settings_listing(state: &AppState, config: &Config, agent: &AgentConfig) -> Vec<Value> {
    of_agent(config, &agent.name).iter().map(|c| view(state, config, agent, c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_icons() {
        assert_eq!(slug("Garmin"), "garmin");
        assert_eq!(slug("Météo du Grand-Est !"), "meteo_du_grand_est");
        assert_eq!(slug("   "), "");
        assert!(icon_keys().len() >= 40);
        assert_eq!(icon_of("fitness")["sf"], "figure.run");
        assert_eq!(icon_of("nope")["key"], "plug");
        assert!(icon_keys().iter().all(|k| icon_of(k)["material"].is_string()));
    }

    fn args(value: Value) -> Dict {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn fields_are_checked() {
        let good = json!({"name": "Garmin", "sites": ["connect.garmin.com", "https://sso.garmin.com/x"],
                          "description": "Envoie tes séances", "icon": "fitness",
                          "folder": "/workspace/connecteurs/garmin", "entry": "python3 server.py",
                          "secrets": ["garmin"], "schedule": {"when": "daily 07:00", "mechanical": true, "tool": "sync"}});
        let mut conn = Connector::default();
        apply_fields(&mut conn, &args(good.clone()), true).unwrap();
        assert_eq!(conn.sites, ["connect.garmin.com", "sso.garmin.com"]);
        assert_eq!(conn.kind, "mcp");
        assert_eq!(schedule_text(&conn.schedule), "Tous les jours à 7:00");
        let bad = |patch: Value| {
            let mut v = good.clone();
            for (k, x) in patch.as_object().unwrap() {
                v[k] = x.clone();
            }
            let mut c = Connector::default();
            apply_fields(&mut c, &args(v), true).unwrap_err()
        };
        assert!(bad(json!({"folder": "/etc/x"})).contains("folder"));
        assert!(bad(json!({"folder": "/workspace/../etc"})).contains("folder"));
        assert!(bad(json!({"icon": "unicorn"})).contains("icon"));
        assert!(bad(json!({"sites": []})).contains("sites"));
        assert!(bad(json!({"secrets": ["Bad-Name"]})).contains("secrets"));
        assert!(bad(json!({"schedule": {"when": "daily 07:00", "mechanical": true}})).contains("schedule.tool"));
        assert!(bad(json!({"entry": ""})).contains("entry"));
    }

    #[test]
    fn the_tools_say_the_rules() {
        let tools = tools();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, [ASK_SECRET, REGISTER, UPDATE, REMOVE, LIST]);
        assert!(tools[0]["description"].as_str().unwrap().contains("Never ask for a password"));
        assert!(tools[1]["description"].as_str().unwrap().contains("sandbox_sites_request"));
        assert!(tools[1]["description"].as_str().unwrap().contains("never goes in a task's prompt"));
    }
}
