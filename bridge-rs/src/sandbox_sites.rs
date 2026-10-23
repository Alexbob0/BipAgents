//! The sites an agent's sandbox may reach besides the package registries (owner decision 2026-10-23, « Ok on fait
//! ça »). An isolated sandbox (sandbox.rs) reaches the internet only through the proxy `bipagents-egress`
//! (assets/sandbox/egress_proxy.py); each one gets its own proxy credentials (`http://<agent>:<token>@…` in its
//! proxy variables), so the proxy knows which agent asks and lets it through to the registries and to the hosts its
//! person allowed it (HTTPS only; exact names, or `*.domain` added by hand in the app).
//!
//! - **Refused → the person decides**: the proxy appends each refusal to a known agent to a reports file the bridge
//!   reads every few seconds (`watch`), and the bridge raises ONE card for the person: « <Agent> veut joindre
//!   data.gouv.fr depuis ses programmes — Autoriser toujours / Refuser ». It is listed with Hermes' own approvals
//!   (`GET /v1/approvals`, run id `bip-site-<id>`, choices `always`/`deny`, the host in `bip_site`), a notification
//!   says so, and the answer comes back here (`POST /v1/agents/{agent}/sandbox/sites/requests/{id}`, or `/v1/approve`)
//!   and is kept with the person's other decisions (approvals_log.rs). One card per agent and host at a time, at most
//!   `MAX_PENDING` per agent; a refused host is not asked about again for `QUIET_AFTER_NO` (7 days) unless the agent
//!   asks itself (`sandbox_sites_request`, once a day at most).
//! - **The agent asks first**: `sandbox_sites_request` (host + reason) raises the same card; `sandbox_sites` lists
//!   what it may reach (both on the `bipagents_files` MCP entry of an isolated agent, sandbox.rs).
//! - **The list in the apps**: « Sites autorisés » in each agent's options (the routes below).
//! - Never this machine: a name of a local network (`localhost`, `.local`, `.internal`…) or an address is refused here,
//!   and the proxy refuses any name that resolves to a private, loopback or Tailscale address.
//! - Isolation off, or no podman: the list is kept and shown, with no effect (`effective: false`).
//!
//! `<data>/sandbox-sites.json` (0600) keeps each agent's token, sites, refusals and the cards waiting;
//! `<data>/sandbox-proxy/policy/sites.json` is what the proxy reads (the tokens' SHA-256 and the hosts, mounted
//! read-only), `<data>/sandbox-proxy/reports/` what it writes.
//!
//! The routes (bridge key):
//!
//!     GET    /v1/agents/{agent}/sandbox/sites                    {registries, sites, pending, effective, why}
//!     POST   /v1/agents/{agent}/sandbox/sites          {host}    added by the person (`*.domain` allowed)
//!     DELETE /v1/agents/{agent}/sandbox/sites/{host}             removed
//!     POST   /v1/agents/{agent}/sandbox/sites/requests/{id} {allow}   a card answered

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

/// The run id of a card (the apps tell them from Hermes' approvals by it).
pub const RUN_PREFIX: &str = "bip-site-";
/// Cards waiting at once, per agent.
pub const MAX_PENDING: usize = 5;
/// Sites per agent.
pub const MAX_SITES: usize = 200;
/// A refused host: not asked about again for so long when the proxy reports it.
pub const QUIET_AFTER_NO: f64 = 7.0 * 86400.0;
/// …nor when the agent asks itself, for so long.
pub const QUIET_EXPLICIT: f64 = 86400.0;
/// A card nobody answered goes after so long.
pub const PENDING_TTL: f64 = 14.0 * 86400.0;
const WATCH_EVERY: Duration = Duration::from_secs(3);
const REPORTS_MAX: u64 = 512 * 1024;

/// Generic second levels under which a `*.` would open anybody's domain.
const OPEN_SECOND_LEVELS: [&str; 10] = ["co", "com", "net", "org", "edu", "ac", "ltd", "plc", "me", "nom"];
/// Hosting where anybody gets a subdomain: never under a `*.`.
const SHARED_HOSTING: [&str; 24] = [
    "github.io",
    "githubusercontent.com",
    "gitlab.io",
    "herokuapp.com",
    "amazonaws.com",
    "cloudfront.net",
    "azurewebsites.net",
    "blob.core.windows.net",
    "appspot.com",
    "web.app",
    "firebaseapp.com",
    "googleusercontent.com",
    "storage.googleapis.com",
    "blogspot.com",
    "workers.dev",
    "pages.dev",
    "vercel.app",
    "netlify.app",
    "onrender.com",
    "fly.dev",
    "ngrok.io",
    "ngrok-free.app",
    "glitch.me",
    "repl.co",
];
/// Names of this machine or its networks.
const LOCAL_SUFFIXES: [&str; 9] =
    ["localhost", "local", "internal", "lan", "home", "arpa", "test", "invalid", "localdomain"];

static LOCK: Mutex<()> = Mutex::new(());

// --- the store -----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub agents: BTreeMap<String, AgentSites>,
    #[serde(default)]
    pub pending: Vec<Pending>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentSites {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub sites: Vec<Site>,
    /// Host → when the person said no (Unix seconds); with when the agent last asked again.
    #[serde(default)]
    pub refused: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Site {
    pub host: String,
    pub at: String,
    /// « person » (added in the app), « request » (a card answered), « adopt » (`sandbox adopt --allow-sites`).
    pub by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Pending {
    pub id: String,
    pub agent: String,
    pub host: String,
    #[serde(default)]
    pub reason: String,
    /// « proxy » (refused) or « agent » (asked first).
    pub via: String,
    pub at: f64,
}

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("sandbox-sites.json")
}

/// The proxy's folder: `policy/` (read-only in its container) and `reports/`.
pub fn proxy_dir(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("sandbox-proxy")
}

pub fn policy_dir(config: &Config) -> PathBuf {
    proxy_dir(config).join("policy")
}

pub fn reports_dir(config: &Config) -> PathBuf {
    proxy_dir(config).join("reports")
}

pub const POLICY_FILE: &str = "sites.json";
pub const REPORTS_FILE: &str = "blocked.jsonl";

pub fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_private(path: &Path, body: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("part");
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp)?;
    out.write_all(body)?;
    drop(out);
    std::fs::rename(&tmp, path)
}

/// The proxy's view: per agent, its token's SHA-256 and its hosts (nothing secret).
pub fn policy_of(store: &Store) -> Value {
    let mut agents = Dict::new();
    for (name, entry) in &store.agents {
        if entry.token.is_empty() {
            continue;
        }
        let hosts: Vec<&str> = entry.sites.iter().map(|s| s.host.as_str()).collect();
        agents.insert(name.clone(), json!({"key": crate::agent_tools::digest(&entry.token), "hosts": hosts}));
    }
    json!({"agents": agents})
}

/// The proxy's file written again (the proxy reads it again when it changed).
pub fn write_policy(config: &Config, store: &Store) -> std::io::Result<()> {
    let dir = policy_dir(config);
    std::fs::create_dir_all(&dir)?;
    py::chmod(&proxy_dir(config).to_string_lossy(), 0o700)?;
    std::fs::create_dir_all(reports_dir(config))?;
    write_private(&dir.join(POLICY_FILE), format!("{}\n", policy_of(store)).as_bytes(), 0o600)
}

/// A change to the store, saved with the proxy's file.
pub fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    let body = serde_json::to_vec_pretty(&store).map_err(std::io::Error::other)?;
    write_private(&store_path(config), &body, 0o600)?;
    write_policy(config, &store)?;
    Ok(out)
}

// --- hosts ---------------------------------------------------------------------------------------------------------

/// A host as the person or the agent gave it (`https://data.gouv.fr/fr/`, `Data.Gouv.FR`) made plain; `*.domain`
/// only when `wildcard`. Err: why not, in plain words.
pub fn clean_host(raw: &str, wildcard: bool) -> Result<String, String> {
    let mut host = raw.trim().to_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = host.strip_prefix(scheme) {
            host = rest.to_string();
        }
    }
    let host = host.split(['/', '?', '#']).next().unwrap_or("").to_string();
    let host = host.rsplit_once('@').map(|(_, h)| h.to_string()).unwrap_or(host);
    let host = match host.rsplit_once(':') {
        Some((h, port)) if port.chars().all(|c| c.is_ascii_digit()) => h.to_string(),
        _ => host,
    };
    let host = host.trim_end_matches('.').to_string();
    if host.is_empty() {
        return Err("give a site's name, like data.gouv.fr".into());
    }
    let (star, base) = match host.strip_prefix("*.") {
        Some(base) => (true, base.to_string()),
        None => (false, host.clone()),
    };
    if star && !wildcard {
        return Err("give one site's exact name (no *)".into());
    }
    if base.len() > 253 || base.contains('*') {
        return Err("not a site's name".into());
    }
    let labels: Vec<&str> = base.split('.').collect();
    if labels.len() < 2 {
        return Err("not a site's name (like data.gouv.fr)".into());
    }
    for label in &labels {
        let ok = !label.is_empty()
            && label.len() <= 63
            && label.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && !label.starts_with('-')
            && !label.ends_with('-');
        if !ok {
            return Err("not a site's name (letters, digits and dashes only)".into());
        }
    }
    let last = labels[labels.len() - 1];
    if !(last.chars().all(|c| c.is_ascii_lowercase()) || last.starts_with("xn--")) {
        return Err("an address, not a site's name: only names are allowed".into());
    }
    if LOCAL_SUFFIXES.contains(&last) || base == "home.arpa" {
        return Err("a name of this machine or its network: never allowed".into());
    }
    if star {
        let two_level_open = labels.len() == 2 && OPEN_SECOND_LEVELS.contains(&labels[0]);
        if SHARED_HOSTING.iter().any(|s| base == *s || base.ends_with(&format!(".{s}"))) || two_level_open {
            return Err(format!("*.{base} would open sites anybody can make: give the exact names instead"));
        }
    }
    Ok(host)
}

/// The registries every sandbox reaches.
pub fn registries(config: &Config) -> Vec<String> {
    config.sandbox.registries.clone()
}

fn covered(sites: &[Site], host: &str) -> bool {
    sites.iter().any(|s| match s.host.strip_prefix("*.") {
        Some(base) => host == s.host || host.ends_with(&format!(".{base}")),
        None => s.host == host,
    })
}

/// Whether the agent's sandbox may reach `host` already (a registry, or one of its sites).
pub fn reachable(config: &Config, agent: &str, host: &str) -> bool {
    registries(config).iter().any(|r| r == host)
        || load(config).agents.get(agent).is_some_and(|a| covered(&a.sites, host))
}

// --- an agent's token ----------------------------------------------------------------------------------------------

/// The agent's proxy URL: the sandboxes' proxy (`http://10.233.80.2:3128`) with its own credentials, its token made
/// the first time (and the proxy's file written).
pub fn proxy_url(config: &Config, agent: &str, base: &str) -> std::io::Result<String> {
    let token = {
        let known = load(config).agents.get(agent).map(|a| a.token.clone()).unwrap_or_default();
        if known.is_empty() {
            change(config, |store| {
                let entry = store.agents.entry(agent.to_string()).or_default();
                if entry.token.is_empty() {
                    entry.token = py::token_urlsafe(24);
                }
                entry.token.clone()
            })?
        } else {
            known
        }
    };
    let rest = base.strip_prefix("http://").unwrap_or(base);
    Ok(format!("http://{agent}:{token}@{rest}"))
}

/// The proxy's file in step with the store (at each start, before the proxy).
pub fn ensure_policy(config: &Config) -> std::io::Result<()> {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    write_policy(config, &load(config))
}

// --- the list ------------------------------------------------------------------------------------------------------

/// Added (true), or already there (false). Err: why not.
pub fn add(config: &Config, agent: &str, raw: &str, by: &str) -> Result<bool, String> {
    let host = clean_host(raw, by == "person")?;
    if registries(config).contains(&host) {
        return Ok(false);
    }
    change(config, |store| {
        let entry = store.agents.entry(agent.to_string()).or_default();
        if entry.sites.iter().any(|s| s.host == host) {
            return Ok(false);
        }
        if entry.sites.len() >= MAX_SITES {
            return Err(format!("at most {MAX_SITES} sites: remove some first"));
        }
        entry.sites.push(Site { host: host.clone(), at: crate::tools_knowledge::now_iso_seconds(), by: by.into() });
        entry.refused.remove(&host);
        store.pending.retain(|p| !(p.agent == agent && p.host == host));
        Ok(true)
    })
    .map_err(|e| e.to_string())?
    .inspect(|added| {
        if *added {
            tracing::info!(target: "bipbridge::sandbox", agent, host = %host, by, "site allowed in the sandbox");
        }
    })
}

pub fn remove(config: &Config, agent: &str, host: &str) -> std::io::Result<bool> {
    let host = host.trim().to_lowercase();
    let removed = change(config, |store| {
        let Some(entry) = store.agents.get_mut(agent) else { return false };
        let before = entry.sites.len();
        entry.sites.retain(|s| s.host != host);
        before != entry.sites.len()
    })?;
    if removed {
        tracing::info!(target: "bipbridge::sandbox", agent, host = %host, "site no longer allowed in the sandbox");
    }
    Ok(removed)
}

/// A deleted agent: its token, sites and cards go.
pub fn forget(config: &Config, agent: &str) {
    if !load(config).agents.contains_key(agent) && !load(config).pending.iter().any(|p| p.agent == agent) {
        return;
    }
    let _ = change(config, |store| {
        store.agents.remove(agent);
        store.pending.retain(|p| p.agent != agent);
    });
}

/// Whether the list does anything on this install (the agent's sandbox isolated), and why not.
pub fn effect(config: &Config, agent: &AgentConfig) -> (bool, Option<&'static str>) {
    if crate::sandbox::isolates(config, agent) {
        return (true, None);
    }
    if crate::sandbox::engine(config).is_none() {
        return (false, Some("isolation"));
    }
    (false, Some("agent"))
}

/// What the app shows.
pub fn view(config: &Config, agent: &AgentConfig) -> Value {
    let store = load(config);
    let entry = store.agents.get(&agent.name).cloned().unwrap_or_default();
    let pending: Vec<Value> = store
        .pending
        .iter()
        .filter(|p| p.agent == agent.name)
        .map(|p| json!({"id": p.id, "host": p.host, "reason": p.reason, "via": p.via, "at": iso(p.at)}))
        .collect();
    let (effective, why) = effect(config, agent);
    json!({
        "registries": registries(config),
        "sites": entry.sites,
        "pending": pending,
        "effective": effective,
        "why": why,
    })
}

fn iso(at: f64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(at as i64, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S+00:00").to_string())
        .unwrap_or_default()
}

// --- the cards -----------------------------------------------------------------------------------------------------

/// What came of asking.
#[derive(Debug, Clone, PartialEq)]
pub enum Asked {
    /// Reachable already.
    Already,
    /// A card waits for the person (`true`: made now).
    Card(Pending, bool),
    /// The person said no on that day; not asked again before `QUIET_*`.
    Refused(String),
    /// Too many cards wait for this agent.
    TooMany,
    Invalid(String),
}

/// The person asked about `host` for `agent` (`via`: « proxy » or « agent »), unless it is reachable, refused lately
/// or asked already.
pub fn ask(config: &Config, agent: &str, raw: &str, reason: &str, via: &str) -> Asked {
    let host = match clean_host(raw, false) {
        Ok(h) => h,
        Err(why) => return Asked::Invalid(why),
    };
    if reachable(config, agent, &host) {
        return Asked::Already;
    }
    let reason: String = reason.trim().chars().take(300).collect();
    let now = py::time_time();
    let result = change(config, |store| {
        store.pending.retain(|p| now - p.at < PENDING_TTL);
        if let Some(p) = store.pending.iter_mut().find(|p| p.agent == agent && p.host == host) {
            if p.reason.is_empty() && !reason.is_empty() {
                p.reason = reason.clone();
            }
            return Asked::Card(p.clone(), false);
        }
        let entry = store.agents.entry(agent.to_string()).or_default();
        if let Some(when) = entry.refused.get(&host).copied() {
            let quiet = if via == "agent" { QUIET_EXPLICIT } else { QUIET_AFTER_NO };
            if now - when < quiet {
                return Asked::Refused(iso(when));
            }
            if via == "agent" {
                entry.refused.insert(host.clone(), now); // asked again: once a day at most
            }
        }
        if store.pending.iter().filter(|p| p.agent == agent).count() >= MAX_PENDING {
            return Asked::TooMany;
        }
        let card = Pending {
            id: py::token_urlsafe(9).replace(['-', '_'], "x"),
            agent: agent.to_string(),
            host: host.clone(),
            reason: reason.clone(),
            via: via.to_string(),
            at: now,
        };
        store.pending.push(card.clone());
        Asked::Card(card, true)
    });
    match result {
        Ok(asked) => {
            if let Asked::Card(card, true) = &asked {
                tracing::info!(target: "bipbridge::sandbox", agent, host = %card.host, via, "site asked of the person");
            }
            asked
        }
        Err(exc) => Asked::Invalid(format!("not asked: {exc}")),
    }
}

/// The sentence on a card (and in older apps' detail).
pub fn sentence(shown: &str, host: &str) -> String {
    format!("{shown} veut joindre {host} depuis ses programmes")
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    load(config)
        .pending
        .into_iter()
        .filter(|p| agent.is_none_or(|a| a == p.agent) && now - p.at < PENDING_TTL)
        .filter_map(|p| {
            let shown = config.agent(Some(&p.agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty());
            let shown = shown.unwrap_or_else(|| p.agent.clone());
            let item = json!({
                "agent": p.agent,
                "run_id": format!("{RUN_PREFIX}{}", p.id),
                "request_id": p.id,
                "command": p.host,
                "description": sentence(&shown, &p.host),
                "choices": ["always", "deny"],
                "created_at": p.at,
                "session_id": null,
                "bip_site": p.host,
                "bip_site_reason": if p.reason.is_empty() { Value::Null } else { json!(p.reason) },
                "bip_site_via": p.via,
            });
            item.as_object().cloned()
        })
        .collect()
}

/// A card answered: the host allowed for good, or refused (not asked again for a while). The decision is kept with
/// the person's others. None: no such card (answered already, or gone).
pub fn answer(config: &Config, agent: &str, id: &str, allow: bool, via: &str) -> std::io::Result<Option<Pending>> {
    let now = py::time_time();
    let card = change(config, |store| {
        let at = store.pending.iter().position(|p| p.id == id && p.agent == agent)?;
        let card = store.pending.remove(at);
        let entry = store.agents.entry(agent.to_string()).or_default();
        if allow {
            if !entry.sites.iter().any(|s| s.host == card.host) && entry.sites.len() < MAX_SITES {
                entry.sites.push(Site {
                    host: card.host.clone(),
                    at: crate::tools_knowledge::now_iso_seconds(),
                    by: "request".into(),
                });
            }
            entry.refused.remove(&card.host);
        } else {
            entry.refused.insert(card.host.clone(), now);
        }
        Some(card)
    })?;
    if let Some(card) = &card {
        let shown = config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or_default();
        let fields = json!({
            "session_id": null, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
            "command": card.host, "description": sentence(if shown.is_empty() { agent } else { &shown }, &card.host),
            "topic": "site", "choice": if allow { "always" } else { "deny" }, "via": via,
        });
        if let Some(map) = fields.as_object() {
            if let Err(exc) = crate::approvals_log::record(config, agent, map) {
                tracing::warn!(target: "bipbridge::sandbox", agent, error = %exc, "site decision not kept");
            }
        }
        tracing::info!(target: "bipbridge::sandbox", agent, host = %card.host, allowed = allow, "site card answered");
    }
    Ok(card)
}

// --- the proxy's reports -------------------------------------------------------------------------------------------

/// The refusals the proxy reported since the last call: (agent, host), the file emptied.
pub fn take_reports(config: &Config) -> Vec<(String, String)> {
    let file = reports_dir(config).join(REPORTS_FILE);
    let Ok(meta) = file.symlink_metadata() else { return Vec::new() };
    let taking = reports_dir(config).join(format!("{REPORTS_FILE}.taking"));
    if !meta.is_file() || std::fs::rename(&file, &taking).is_err() {
        let _ = std::fs::remove_file(&file);
        return Vec::new();
    }
    let body = match crate::sandbox::open_plain(&taking) {
        Ok(f) => {
            use std::io::Read;
            let mut text = String::new();
            let _ = f.take(REPORTS_MAX).read_to_string(&mut text);
            text
        }
        Err(_) => String::new(),
    };
    let _ = std::fs::remove_file(&taking);
    let mut seen: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        let Ok(Value::Object(row)) = serde_json::from_str::<Value>(line) else { continue };
        let (Some(agent), Some(host)) =
            (row.get("agent").and_then(Value::as_str), row.get("host").and_then(Value::as_str))
        else {
            continue;
        };
        let pair = (agent.to_string(), host.to_string());
        if !seen.contains(&pair) && seen.len() < 100 {
            seen.push(pair);
        }
    }
    seen
}

/// The proxy's reports turned into cards (each few seconds), and the adopted « link » folders tended.
pub async fn watch(state: AppState) {
    let mut ticks: u64 = 0;
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        ticks += 1;
        let config = state.config();
        let c = config.clone();
        let reports = tokio::task::spawn_blocking(move || take_reports(&c)).await.unwrap_or_default();
        for (agent, host) in reports {
            let Some(found) = config.agent(Some(&agent)).cloned() else { continue };
            if !crate::sandbox::isolates(&config, &found) {
                continue;
            }
            let c = config.clone();
            let (a, h) = (agent.clone(), host.clone());
            let asked = tokio::task::spawn_blocking(move || ask(&c, &a, &h, "", "proxy")).await;
            if let Ok(Asked::Card(card, true)) = asked {
                notify(&state, &found, &card).await;
            }
        }
        if ticks % 100 == 1 {
            let c = config.clone();
            let _ = tokio::task::spawn_blocking(move || crate::sandbox_adopt::tend_links(&c)).await;
        }
    }
}

/// The person told (a notification that opens the app's Boîte, where the card is).
pub async fn notify(state: &AppState, agent: &AgentConfig, card: &Pending) {
    state.push.notify_site_request(&agent.name, &card.host, &card.id).await;
}

// --- the routes ----------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NewSite {
    pub host: String,
}

impl Validate for NewSite {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "host", Some(&self.host), 300);
    }
}

#[derive(Debug, Deserialize)]
pub struct Answer {
    pub allow: bool,
}

impl Validate for Answer {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

fn agent_of(config: &Config, name: &str) -> Result<AgentConfig, HttpError> {
    config.agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

async fn get_sites(State(state): State<AppState>, UrlPath(name): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    Ok(web::ok(view(&config, &agent)))
}

async fn add_site(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Json(body): Json<NewSite>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let (c, a, host) = (config.clone(), agent.name.clone(), body.host.clone());
    tokio::task::spawn_blocking(move || add(&c, &a, &host, "person"))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|why| HttpError::new(422, why))?;
    Ok(web::ok(view(&config, &agent)))
}

async fn delete_site(
    State(state): State<AppState>,
    UrlPath((name, host)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    if !remove(&config, &agent.name, &host).map_err(anyhow::Error::from)? {
        return Err(HttpError::new(404, "no such site"));
    }
    Ok(web::ok(view(&config, &agent)))
}

async fn answer_route(
    State(state): State<AppState>,
    UrlPath((name, id)): UrlPath<(String, String)>,
    Json(body): Json<Answer>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    match answer(&config, &agent.name, &id, body.allow, "app").map_err(anyhow::Error::from)? {
        Some(_) => Ok(web::ok(view(&config, &agent))),
        None => Err(HttpError::new(404, "no such request (answered already?)")),
    }
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent}/sandbox/sites", get(get_sites).post(add_site))
        .route("/agents/{agent}/sandbox/sites/{host}", axum::routing::delete(delete_site))
        .route("/agents/{agent}/sandbox/sites/requests/{id}", post(answer_route))
}

// --- the agent's tools ---------------------------------------------------------------------------------------------

pub const LIST: &str = "sandbox_sites";
pub const REQUEST: &str = "sandbox_sites_request";

pub fn tools() -> Vec<Value> {
    let read_only = json!({"readOnlyHint": true, "destructiveHint": false, "openWorldHint": false});
    vec![
        json!({
            "name": LIST,
            "description": "List the sites your programs (pip, npm, curl, your scripts) can reach from your sandbox: the \
        package registries, always, and the sites your person allowed you.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": read_only,
        }),
        // Marked read-only: it only shows the person a card, which is their decision (no second question first).
        json!({
            "name": REQUEST,
            "description": "Ask your person to let your programs reach a site from your sandbox (HTTPS only), when a \
        script or a tool you run needs it: they see « Autoriser toujours / Refuser » in the app. Give the site's exact \
        name and, in a few plain words, why. Then tell them in one sentence; retry once they allowed it. A site your \
        programs were refused is asked about by itself: ask here only to explain why, or to ask first.",
            "inputSchema": {"type": "object", "properties": {
                "host": {"type": "string", "description": "the site's name, e.g. data.gouv.fr"},
                "reason": {"type": "string", "description": "why, in a few plain words in your person's language"},
            }, "required": ["host"]},
            "annotations": read_only,
        }),
    ]
}

/// One call of these tools for `agent` (blocking: files only). Returns the tool's text and whether it is an error,
/// and the card made, if any (to notify the person).
pub fn call(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> (String, bool, Option<Pending>) {
    let (on, _) = effect(config, agent);
    if name == LIST {
        let entry = load(config).agents.get(&agent.name).cloned().unwrap_or_default();
        let mut lines = vec![format!("Always open (package registries): {}", registries(config).join(", "))];
        if entry.sites.is_empty() {
            lines.push("Allowed by your person: none yet (sandbox_sites_request asks for one).".into());
        } else {
            let sites: Vec<String> = entry.sites.iter().map(|s| s.host.clone()).collect();
            lines.push(format!("Allowed by your person: {}", sites.join(", ")));
        }
        let waiting: Vec<String> =
            load(config).pending.iter().filter(|p| p.agent == agent.name).map(|p| p.host.clone()).collect();
        if !waiting.is_empty() {
            lines.push(format!("Waiting for your person's answer: {}", waiting.join(", ")));
        }
        if !on {
            lines.push("(Your sandbox is not isolated here: this list has no effect.)".into());
        }
        lines.push("Anything else: use your web tools (search, browser), not your programs.".into());
        return (lines.join("\n"), false, None);
    }
    let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match ask(config, &agent.name, &text("host"), &text("reason"), "agent") {
        Asked::Already => ("Your programs can reach it already.".into(), false, None),
        Asked::Card(card, made) => (
            format!(
                "Your person sees a card asking to let your programs reach {}{}. Tell them in one plain sentence why; \
                 retry once they allowed it.",
                card.host,
                if made { "" } else { " (asked already, still waiting)" }
            ),
            false,
            made.then_some(card),
        ),
        Asked::Refused(when) => (
            format!(
                "Your person refused this site ({when}): don't ask again today; do without it or ask them in words."
            ),
            true,
            None,
        ),
        Asked::TooMany => (
            "Several requests already wait for your person's answer: wait for them before asking more.".into(),
            true,
            None,
        ),
        Asked::Invalid(why) => (why, true, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        (dir, config)
    }

    #[test]
    fn hosts_are_names_of_the_internet() {
        assert_eq!(clean_host("https://Data.Gouv.FR/fr/datasets?x=1", false).unwrap(), "data.gouv.fr");
        assert_eq!(clean_host("api.meteo.fr:443", false).unwrap(), "api.meteo.fr");
        assert_eq!(clean_host("user@x.example.org.", false).unwrap(), "x.example.org");
        for bad in [
            "",
            "localhost",
            "10.0.0.1",
            "[::1]",
            "printer.local",
            "host.containers.internal",
            "nas.lan",
            "x",
            "a_b.fr",
            "-a.fr",
            "router.home.arpa",
            "*.example.org",
        ] {
            assert!(clean_host(bad, false).is_err(), "{bad}");
        }
        assert_eq!(clean_host("*.ouraring.com", true).unwrap(), "*.ouraring.com");
        for bad in ["*.com", "*.co.uk", "*.github.io", "*.x.amazonaws.com", "*.*.fr", "a.*.fr"] {
            assert!(clean_host(bad, true).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_card_once_then_the_persons_word() {
        let (_dir, config) = config();
        let url = proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap();
        let token = load(&config).agents["vie"].token.clone();
        assert_eq!(url, format!("http://vie:{token}@10.233.80.2:3128"));
        assert_eq!(proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap(), url); // kept
        let policy: Value =
            serde_json::from_str(&std::fs::read_to_string(policy_dir(&config).join(POLICY_FILE)).unwrap()).unwrap();
        assert_eq!(policy["agents"]["vie"]["key"], json!(crate::agent_tools::digest(&token)));
        assert!(!policy.to_string().contains(&token));
        // the proxy refused it: one card, not two
        let Asked::Card(card, true) = ask(&config, "vie", "data.gouv.fr", "", "proxy") else { panic!() };
        assert!(matches!(ask(&config, "vie", "data.gouv.fr", "open data", "agent"), Asked::Card(_, false)));
        assert_eq!(load(&config).pending[0].reason, "open data");
        assert_eq!(ask(&config, "vie", "pypi.org", "", "agent"), Asked::Already);
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], json!(format!("bip-site-{}", card.id)));
        assert_eq!(items[0]["choices"], json!(["always", "deny"]));
        // refused: quiet for a week from the proxy, a day when the agent asks itself
        assert!(answer(&config, "vie", &card.id, false, "app").unwrap().is_some());
        assert!(answer(&config, "vie", &card.id, false, "app").unwrap().is_none());
        assert!(matches!(ask(&config, "vie", "data.gouv.fr", "", "proxy"), Asked::Refused(_)));
        assert!(matches!(ask(&config, "vie", "data.gouv.fr", "", "agent"), Asked::Refused(_)));
        assert_eq!(crate::approvals_log::list(&config, "vie", None)[0]["choice"], json!("deny"));
        // allowed: in the list and the proxy's file
        let Asked::Card(card, true) = ask(&config, "vie", "api.meteo.fr", "", "agent") else { panic!() };
        answer(&config, "vie", &card.id, true, "app").unwrap();
        assert!(reachable(&config, "vie", "api.meteo.fr") && !reachable(&config, "budget", "api.meteo.fr"));
        let policy = std::fs::read_to_string(policy_dir(&config).join(POLICY_FILE)).unwrap();
        assert!(policy.contains("api.meteo.fr"));
        // at most MAX_PENDING cards
        for i in 0..MAX_PENDING {
            assert!(matches!(ask(&config, "vie", &format!("s{i}.example.org"), "", "proxy"), Asked::Card(_, true)));
        }
        assert_eq!(ask(&config, "vie", "one-more.example.org", "", "proxy"), Asked::TooMany);
        assert!(remove(&config, "vie", "api.meteo.fr").unwrap());
        assert!(!reachable(&config, "vie", "api.meteo.fr"));
        forget(&config, "vie");
        assert!(load(&config).agents.is_empty() && load(&config).pending.is_empty());
    }

    #[test]
    fn the_proxys_reports_are_taken_once() {
        let (_dir, config) = config();
        ensure_policy(&config).unwrap();
        let file = reports_dir(&config).join(REPORTS_FILE);
        std::fs::write(
            &file,
            "{\"agent\":\"vie\",\"host\":\"a.fr\"}\nnot json\n{\"agent\":\"vie\",\"host\":\"a.fr\"}\n{\"agent\":\"w\",\"host\":\"b.fr\"}\n",
        )
        .unwrap();
        assert_eq!(take_reports(&config), vec![("vie".into(), "a.fr".into()), ("w".into(), "b.fr".into())]);
        assert!(take_reports(&config).is_empty() && !file.exists());
    }
}
