//! The sites an agent may SEND data to (owner decisions 2026-10-23, then 2026-10-28: « il faudrait pas bloquer le site
//! mais bloquer l'envoi d'informations sans validation », internal/next/send-guard.md). An isolated sandbox
//! (sandbox.rs) reaches the internet only through the proxy `bipagents-egress` (assets/sandbox/egress_proxy.py); each
//! one gets its own proxy credentials (`http://<agent>:<token>@…` in its proxy variables), and the agent's visible
//! browser is known by its address (send_guard.rs), so the proxy knows which agent asks.
//!
//! - **Reading is open**: every agent's programs and browser reach any public HTTPS site, with no card. This machine
//!   and private or Tailscale addresses stay refused (here for the names, in the proxy for what they resolve to).
//! - **Sending is guarded, for every agent** (whatever it reads can be written to a file and uploaded): the proxy
//!   counts what each tunnel sends a site this list doesn't hold; past a small volume it cuts the tunnel and reports it
//!   (`send_guard.rs` has the thresholds and why). The bridge reads the reports every few seconds (`watch`) and raises
//!   ONE card per agent: « <Agent> veut envoyer des données à exemple.com (environ 2 Mo) — Autoriser / Refuser ». It is
//!   listed with Hermes' own approvals (`GET /v1/approvals`, run id `bip-site-<id>`, choices `always`/`deny`, the host
//!   in `bip_site`, the size in `bip_site_send_bytes`, `bip_site_kind: "send"`), a notification says so, and the answer
//!   comes back here (`POST /v1/agents/{agent}/sandbox/sites/requests/{id}`, or `/v1/approve`) and is kept with the
//!   person's other decisions (approvals_log.rs). « Autoriser » validates that site for sending, for that agent, until
//!   removed in « Sites autorisés », and clears the proxy's counts for it (`reset`). Hosts reported within
//!   `GROUP_WINDOW` gather on one card; at most `MAX_PENDING` cards per agent; a refused host is not asked about again
//!   for `QUIET_AFTER_NO` (7 days) unless the agent asks itself (`sandbox_sites_request`, once a day at most).
//! - **Older apps** read `bip_site` (the first host) and the French `description`, and answer `{allow}` (that host
//!   only, the others wait on a card of their own) or « Tous les sites » (`all_sites`: every host on the card).
//! - **Connectors**: the sites on a connector's approval card are validated for sending, for the connector and for its
//!   agent (connectors.rs).
//! - **Migration** (2026-10-28): the lists kept from the « reading » cards are the agents' validated sites; the « Tous
//!   les sites » switch and the visited sites are gone (the PATCH an older app sends is accepted and does nothing).
//! - Isolation off, or no podman: the list is kept and shown, with no effect (`effective: false`).
//!
//! `<data>/sandbox-sites.json` (0600) keeps each agent's token, sites, refusals and the cards waiting;
//! `<data>/sandbox-proxy/policy/sites.json` is what the proxy reads (the tokens' SHA-256, the hosts, the limits, the
//! browsers' addresses; mounted read-only), `<data>/sandbox-proxy/reports/` what it writes (`sends.jsonl`, and
//! `usage.json`: today's counts).
//!
//! The routes (bridge key):
//!
//!     GET    /v1/agents/{agent}/sandbox/sites                    {registries, sites, pending, effective, why,
//!                                                                  reading_open, today}
//!     POST   /v1/agents/{agent}/sandbox/sites          {host}    added by the person (`*.domain` allowed)
//!     DELETE /v1/agents/{agent}/sandbox/sites/{host}             removed
//!     POST   /v1/agents/{agent}/sandbox/sites/requests/{id} {allow, hosts?}   a card answered

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
/// Hosts reported within so long gather on one card.
pub const GROUP_WINDOW: f64 = 120.0;
/// Hosts on one card.
pub const MAX_HOSTS: usize = 8;
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
    /// The sites validated for sending.
    #[serde(default)]
    pub sites: Vec<Site>,
    /// Host → when the person said no (Unix seconds); with when the agent last asked again.
    #[serde(default)]
    pub refused: BTreeMap<String, f64>,
    /// When the person last allowed a site (the proxy clears the agent's counts then).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub reset: f64,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

fn no_bytes(value: &u64) -> bool {
    *value == 0
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
    /// The first host (what older apps show).
    pub host: String,
    /// All the hosts on the card, the first one included (empty on cards made before 2026-10-25: `host` only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    #[serde(default)]
    pub reason: String,
    /// « proxy » (a send cut) or « agent » (asked first).
    pub via: String,
    pub at: f64,
    /// About how much the agent sent or tried to send (bytes; 0: not measured, the agent asked first).
    #[serde(default, skip_serializing_if = "no_bytes")]
    pub bytes: u64,
}

impl Pending {
    pub fn all_hosts(&self) -> Vec<String> {
        if self.hosts.is_empty() {
            vec![self.host.clone()]
        } else {
            self.hosts.clone()
        }
    }
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
pub const REPORTS_FILE: &str = "sends.jsonl";

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

/// The sites the agent's approved connectors reach (validated for sending with them).
fn connector_sites(config: &Config, agent: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in crate::connectors::of_agent(config, agent) {
        if c.status == crate::connectors::APPROVED && c.enabled {
            for site in c.sites {
                if !out.contains(&site) {
                    out.push(site);
                }
            }
        }
    }
    out
}

/// The proxy's view: per agent, its token's SHA-256, the hosts validated for sending (its person's, its connectors')
/// and when its person last allowed one; the connectors' own entries; the limits; the browsers' addresses (nothing
/// secret).
pub fn policy_of(config: &Config, store: &Store) -> Value {
    let mut agents = Dict::new();
    for (name, entry) in &store.agents {
        if entry.token.is_empty() {
            continue;
        }
        let mut hosts: Vec<String> = entry.sites.iter().map(|s| s.host.clone()).collect();
        for host in connector_sites(config, name) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        let mut item = json!({"key": crate::agent_tools::digest(&entry.token), "hosts": hosts});
        if entry.reset > 0.0 {
            item["reset"] = json!(entry.reset);
        }
        agents.insert(name.clone(), item);
    }
    // the connectors the person approved (connectors.rs): their own credentials, their sites only
    for (name, item) in crate::connectors::proxy_identities(config) {
        agents.insert(name, item);
    }
    let browsers: serde_json::Map<String, Value> = crate::send_guard::browsers_map(config)
        .into_iter()
        .filter(|(_, agent)| agent.as_str().is_some_and(|a| agents.contains_key(a)))
        .collect();
    json!({"agents": agents, "browsers": browsers, "limits": crate::send_guard::limits(config)})
}

/// The proxy's file written again when it changed (the proxy reads it again then).
pub fn write_policy(config: &Config, store: &Store) -> std::io::Result<()> {
    let dir = policy_dir(config);
    std::fs::create_dir_all(&dir)?;
    py::chmod(&proxy_dir(config).to_string_lossy(), 0o700)?;
    std::fs::create_dir_all(reports_dir(config))?;
    let body = format!("{}\n", policy_of(config, store));
    let file = dir.join(POLICY_FILE);
    if std::fs::read_to_string(&file).is_ok_and(|known| known == body) {
        return Ok(());
    }
    write_private(&file, body.as_bytes(), 0o600)
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

/// The registries every sandbox reaches (and sends to without counting).
pub fn registries(config: &Config) -> Vec<String> {
    config.sandbox.registries.clone()
}

fn covered(sites: &[String], host: &str) -> bool {
    sites.iter().any(|s| match s.strip_prefix("*.") {
        Some(base) => host == s || host.ends_with(&format!(".{base}")),
        None => s == host,
    })
}

/// Whether the agent may send to `host` without a card: a registry, one of its validated sites, a site of one of its
/// approved connectors.
pub fn validated(config: &Config, agent: &str, host: &str) -> bool {
    if registries(config).iter().any(|r| r == host) {
        return true;
    }
    let mut sites: Vec<String> =
        load(config).agents.get(agent).map(|a| a.sites.iter().map(|s| s.host.clone()).collect()).unwrap_or_default();
    sites.extend(connector_sites(config, agent));
    covered(&sites, host)
}

/// « environ 2 Mo », « environ 300 Ko ».
pub fn about(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    let (value, unit) = if b >= KB * KB * KB {
        (b / (KB * KB * KB), "Go")
    } else if b >= KB * KB {
        (b / (KB * KB), "Mo")
    } else {
        ((b / KB).max(1.0), "Ko")
    };
    let shown = if value < 10.0 && unit != "Ko" {
        format!("{:.1}", value).trim_end_matches(".0").replace('.', ",")
    } else {
        format!("{}", value.round() as u64)
    };
    format!("environ {shown} {unit}")
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

/// A browser that goes through the proxy needs an entry for its agent (the proxy knows the browser by its address,
/// and finds the agent's validated sites there): made like a sandbox's, without handing the token out.
pub fn ensure_agent(config: &Config, agent: &str) -> std::io::Result<()> {
    if load(config).agents.get(agent).is_some_and(|a| !a.token.is_empty()) {
        return ensure_policy(config);
    }
    change(config, |store| {
        let entry = store.agents.entry(agent.to_string()).or_default();
        if entry.token.is_empty() {
            entry.token = py::token_urlsafe(24);
        }
    })
}

// --- the list ------------------------------------------------------------------------------------------------------

/// Added (true), or already there (false). Err: why not.
pub fn add(config: &Config, agent: &str, raw: &str, by: &str) -> Result<bool, String> {
    let host = clean_host(raw, by == "person")?;
    if registries(config).contains(&host) {
        return Ok(false);
    }
    let now = py::time_time();
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
        entry.reset = now;
        store.pending.retain(|p| !(p.agent == agent && p.host == host));
        Ok(true)
    })
    .map_err(|e| e.to_string())?
    .inspect(|added| {
        if *added {
            tracing::info!(target: "bipbridge::sandbox", agent, host = %host, by, "site validated for sending");
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
        tracing::info!(target: "bipbridge::sandbox", agent, host = %host, "site no longer validated for sending");
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

/// What the app shows. `all_sites: true` for older apps (their « Tous les sites » switch shows on: reading is open).
pub fn view(config: &Config, agent: &AgentConfig) -> Value {
    let store = load(config);
    let entry = store.agents.get(&agent.name).cloned().unwrap_or_default();
    let pending: Vec<Value> = store
        .pending
        .iter()
        .filter(|p| p.agent == agent.name)
        .map(|p| {
            json!({"id": p.id, "host": p.host, "hosts": p.all_hosts(), "reason": p.reason, "via": p.via,
                   "at": iso(p.at), "send_bytes": if p.bytes > 0 { json!(p.bytes) } else { Value::Null }})
        })
        .collect();
    let (effective, why) = effect(config, agent);
    let connectors: Vec<String> = connector_sites(config, &agent.name);
    json!({
        "registries": registries(config),
        "sites": entry.sites,
        "connector_sites": connectors,
        "pending": pending,
        "effective": effective,
        "why": why,
        "reading_open": true,
        "all_sites": true,
        "all_sites_default": true,
        "sensitive": null,
        "today": crate::send_guard::usage(config, &agent.name),
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
    /// Validated already.
    Already,
    /// A card waits for the person (`true`: made now).
    Card(Pending, bool),
    /// The person said no on that day; not asked again before `QUIET_*`.
    Refused(String),
    /// Too many cards wait for this agent.
    TooMany,
    Invalid(String),
}

/// The person asked whether `agent` may send to `host` (`via`: « proxy », a send cut, with about how many `bytes`; or
/// « agent », asked first), unless it is validated, refused lately or asked already.
pub fn ask(config: &Config, agent: &str, raw: &str, reason: &str, via: &str, bytes: u64) -> Asked {
    let host = match clean_host(raw, false) {
        Ok(h) => h,
        Err(why) => return Asked::Invalid(why),
    };
    if validated(config, agent, &host) {
        return Asked::Already;
    }
    let reason: String = reason.trim().chars().take(300).collect();
    let now = py::time_time();
    let result = change(config, |store| {
        store.pending.retain(|p| now - p.at < PENDING_TTL);
        if let Some(p) = store.pending.iter_mut().find(|p| p.agent == agent && p.all_hosts().contains(&host)) {
            if p.reason.is_empty() && !reason.is_empty() {
                p.reason = reason.clone();
            }
            p.bytes = p.bytes.max(bytes);
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
        // a card made a moment ago takes it too: one card, not one per site
        let joining = store
            .pending
            .iter_mut()
            .rev()
            .find(|p| p.agent == agent && now - p.at < GROUP_WINDOW && p.all_hosts().len() < MAX_HOSTS);
        if let Some(p) = joining {
            let mut hosts = p.all_hosts();
            hosts.push(host.clone());
            p.hosts = hosts;
            if p.reason.is_empty() && !reason.is_empty() {
                p.reason = reason.clone();
            }
            p.bytes += bytes;
            return Asked::Card(p.clone(), false);
        }
        if store.pending.iter().filter(|p| p.agent == agent).count() >= MAX_PENDING {
            return Asked::TooMany;
        }
        let card = Pending {
            id: py::token_urlsafe(9).replace(['-', '_'], "x"),
            agent: agent.to_string(),
            host: host.clone(),
            hosts: vec![host.clone()],
            reason: reason.clone(),
            via: via.to_string(),
            at: now,
            bytes,
        };
        store.pending.push(card.clone());
        Asked::Card(card, true)
    });
    match result {
        Ok(asked) => {
            if let Asked::Card(card, true) = &asked {
                tracing::info!(target: "bipbridge::sandbox", agent, host = %card.host, via, bytes, "sending asked of the person");
            }
            asked
        }
        Err(exc) => Asked::Invalid(format!("not asked: {exc}")),
    }
}

/// The sentence on a card (and in older apps' detail).
pub fn sentence(shown: &str, hosts: &[String], bytes: u64) -> String {
    let size = if bytes > 0 { format!(" ({})", about(bytes)) } else { String::new() };
    match hosts {
        [one] => format!("{shown} veut envoyer des données à {one}{size}"),
        _ => format!("{shown} veut envoyer des données à {} sites{size} : {}", hosts.len(), hosts.join(", ")),
    }
}

fn shown_name(config: &Config, agent: &str) -> String {
    let shown = config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty());
    shown.unwrap_or_else(|| agent.to_string())
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    load(config)
        .pending
        .into_iter()
        .filter(|p| agent.is_none_or(|a| a == p.agent) && now - p.at < PENDING_TTL)
        .filter_map(|p| {
            let hosts = p.all_hosts();
            let item = json!({
                "agent": p.agent,
                "run_id": format!("{RUN_PREFIX}{}", p.id),
                "request_id": p.id,
                "command": hosts.join(", "),
                "description": sentence(&shown_name(config, &p.agent), &hosts, p.bytes),
                "choices": ["always", "deny"],
                "created_at": p.at,
                "session_id": null,
                "bip_site": p.host,
                "bip_sites": hosts,
                "bip_site_kind": "send",
                "bip_site_send_bytes": p.bytes,
                "bip_site_reason": if p.reason.is_empty() { Value::Null } else { json!(p.reason) },
                "bip_site_via": p.via,
            });
            item.as_object().cloned()
        })
        .collect()
}

/// What the person said on a card.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// « Autoriser » (the hosts shown) or « Refuser » (them all).
    Hosts(bool, Vec<String>),
    /// An older app's answer, which showed the first host only: that one; the others wait on a card of their own.
    First(bool),
    /// « Refuser », every host on the card.
    DenyAll,
    /// An older app's « Tous les sites pour <agent> »: every host on the card validated.
    AllSites,
}

/// A card answered: the hosts validated for sending, or refused (not asked again for a while). The decision is kept
/// with the person's others. None: no such card (answered already, or gone).
pub fn answer(config: &Config, agent: &str, id: &str, decision: Decision, via: &str) -> std::io::Result<Option<Pending>> {
    let now = py::time_time();
    let decided = change(config, |store| {
        let at = store.pending.iter().position(|p| p.id == id && p.agent == agent)?;
        let card = store.pending.remove(at);
        let hosts = card.all_hosts();
        let (allow, chosen): (bool, Vec<String>) = match &decision {
            Decision::Hosts(allow, shown) => (*allow, hosts.iter().filter(|h| shown.contains(h)).cloned().collect()),
            Decision::First(allow) => (*allow, hosts[..1].to_vec()),
            Decision::DenyAll => (false, hosts.clone()),
            Decision::AllSites => (true, hosts.clone()),
        };
        let left: Vec<String> = hosts.iter().filter(|h| !chosen.contains(h)).cloned().collect();
        let entry = store.agents.entry(agent.to_string()).or_default();
        for host in &chosen {
            if allow {
                if !entry.sites.iter().any(|s| &s.host == host) && entry.sites.len() < MAX_SITES {
                    entry.sites.push(Site {
                        host: host.clone(),
                        at: crate::tools_knowledge::now_iso_seconds(),
                        by: "request".into(),
                    });
                }
                entry.refused.remove(host);
            } else {
                entry.refused.insert(host.clone(), now);
            }
        }
        if allow && !chosen.is_empty() {
            entry.reset = now;
        }
        if let Some(first) = left.first() {
            // not answered: on a card of their own (no new notification)
            store.pending.push(Pending {
                id: py::token_urlsafe(9).replace(['-', '_'], "x"),
                host: first.clone(),
                hosts: left.clone(),
                at: now - GROUP_WINDOW,
                ..card.clone()
            });
        }
        let shown_hosts = if chosen.is_empty() { hosts } else { chosen };
        Some((card, shown_hosts, if allow { "always" } else { "deny" }))
    })?;
    let Some((card, hosts, choice)) = decided else { return Ok(None) };
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
        "command": hosts.join(", "), "description": sentence(&shown_name(config, agent), &hosts, card.bytes),
        "topic": "site", "choice": choice, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(target: "bipbridge::sandbox", agent, error = %exc, "site decision not kept");
        }
    }
    tracing::info!(target: "bipbridge::sandbox", agent, hosts = %hosts.join(", "), choice, "sending card answered");
    Ok(Some(card))
}

// --- the proxy's reports -------------------------------------------------------------------------------------------

/// A send the proxy cut.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub agent: String,
    pub host: String,
    pub bytes: u64,
}

/// The sends the proxy cut since the last call, the file emptied (lines of an older proxy, refusals to read, are
/// left: reading is open).
pub fn take_reports(config: &Config) -> Vec<Report> {
    let file = reports_dir(config).join(REPORTS_FILE);
    let _ = std::fs::remove_file(reports_dir(config).join("blocked.jsonl")); // the reading cards' file, before 2026-10-28
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
    let mut seen: Vec<Report> = Vec::new();
    for line in body.lines() {
        let Ok(Value::Object(row)) = serde_json::from_str::<Value>(line) else { continue };
        if row.get("kind").and_then(Value::as_str) != Some("send") {
            continue;
        }
        let (Some(agent), Some(host)) =
            (row.get("agent").and_then(Value::as_str), row.get("host").and_then(Value::as_str))
        else {
            continue;
        };
        let bytes = row.get("bytes").and_then(Value::as_u64).unwrap_or(0);
        if let Some(known) = seen.iter_mut().find(|r| r.agent == agent && r.host == host) {
            known.bytes = known.bytes.max(bytes);
        } else if seen.len() < 100 {
            seen.push(Report { agent: agent.to_string(), host: host.to_string(), bytes });
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
        for report in reports {
            let Some(found) = config.agent(Some(&report.agent)).cloned() else { continue };
            tracing::warn!(target: "bipbridge::sandbox", agent = %report.agent, host = %report.host, bytes = report.bytes, "a send to a site not validated was cut");
            let c = config.clone();
            let asked =
                tokio::task::spawn_blocking(move || ask(&c, &report.agent, &report.host, "", "proxy", report.bytes))
                    .await;
            if let Ok(Asked::Card(card, true)) = asked {
                notify(&state, &found, &card).await;
            }
        }
        if ticks.is_multiple_of(10) {
            // a connector approved, a browser started, the limits changed: the proxy's file follows within half a
            // minute (written only when it changed)
            let c = config.clone();
            let _ = tokio::task::spawn_blocking(move || ensure_policy(&c)).await;
        }
        if ticks % 100 == 1 {
            let c = config.clone();
            let _ = tokio::task::spawn_blocking(move || crate::sandbox_adopt::tend_links(&c)).await;
        }
    }
}

/// The person told (a notification that opens the app's Boîte, where the card is).
pub async fn notify(state: &AppState, agent: &AgentConfig, card: &Pending) {
    let size = if card.bytes > 0 { format!(" ({})", about(card.bytes)) } else { String::new() };
    let line = format!("Veut envoyer des données à {}{size}", card.host);
    state.push.notify_site_request(&agent.name, &line, &card.id).await;
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
    /// The hosts the card showed (an older app sends none: the first host only).
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    /// An older app's « Tous les sites pour <agent> »: every host on the card.
    #[serde(default)]
    pub all_sites: bool,
}

impl Validate for Answer {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

impl Answer {
    pub fn decision(&self) -> Decision {
        match (&self.hosts, self.all_sites) {
            (_, true) => Decision::AllSites,
            (Some(hosts), _) => {
                Decision::Hosts(self.allow, hosts.iter().take(MAX_HOSTS).map(|h| h.trim().to_lowercase()).collect())
            }
            (None, _) => Decision::First(self.allow),
        }
    }
}

/// A card answered through `/v1/approve` (a notification, or an app that answers Hermes' way): `deny` (all its
/// hosts), an older app's `all_sites` (all of them allowed), else « Autoriser » (the first host, what older apps show).
pub fn decision_of_choice(choice: &str) -> Decision {
    match choice.trim().to_lowercase().as_str() {
        "all_sites" => Decision::AllSites,
        "deny" => Decision::DenyAll,
        _ => Decision::First(true),
    }
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
    match answer(&config, &agent.name, &id, body.decision(), "app").map_err(anyhow::Error::from)? {
        Some(_) => Ok(web::ok(view(&config, &agent))),
        None => Err(HttpError::new(404, "no such request (answered already?)")),
    }
}

/// An older app's « Tous les sites » switch: reading is open to every agent now, nothing to change.
async fn switch_route(State(state): State<AppState>, UrlPath(name): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    Ok(web::ok(view(&config, &agent)))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent}/sandbox/sites", get(get_sites).post(add_site).patch(switch_route))
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
            "description": "List the sites your programs (pip, npm, curl, your scripts) and your browser may SEND data \
        to: the package registries, and the sites your person validated. Reading any public HTTPS site is open.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": read_only,
        }),
        // Marked read-only: it only shows the person a card, which is their decision (no second question first).
        json!({
            "name": REQUEST,
            "description": "Ask your person to let you send data to a site (an upload, an API you write to, a form \
        with files), when what they asked needs it: they see a card in the app. Reading sites needs no request. Give \
        the site's exact name and, in a few plain words, why. Then tell them in one sentence; retry once they allowed \
        it. A send cut by the guard (a connection reset while uploading) asks by itself: ask here only to explain why, \
        or to ask first.",
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
        let mut lines = vec![
            "Reading: every public HTTPS site is open to your programs and your browser.".to_string(),
            format!("Sending, always allowed (package registries): {}", registries(config).join(", ")),
        ];
        let mut sites: Vec<String> = entry.sites.iter().map(|s| s.host.clone()).collect();
        for site in connector_sites(config, &agent.name) {
            if !sites.contains(&site) {
                sites.push(site);
            }
        }
        if sites.is_empty() {
            lines.push("Sending, validated by your person: none yet (sandbox_sites_request asks for one).".into());
        } else {
            lines.push(format!("Sending, validated by your person: {}", sites.join(", ")));
        }
        lines.push(
            "To any other site, a few KB (a page, an API call, a login, a small form) go; an upload past that is cut \
             and your person is asked."
                .into(),
        );
        let waiting: Vec<String> =
            load(config).pending.iter().filter(|p| p.agent == agent.name).map(|p| p.host.clone()).collect();
        if !waiting.is_empty() {
            lines.push(format!("Waiting for your person's answer: {}", waiting.join(", ")));
        }
        if !on {
            lines.push("(Your sandbox is not isolated here: this list has no effect.)".into());
        }
        return (lines.join("\n"), false, None);
    }
    let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match ask(config, &agent.name, &text("host"), &text("reason"), "agent", 0) {
        Asked::Already => ("You may send to it already.".into(), false, None),
        Asked::Card(card, made) => (
            format!(
                "Your person sees a card asking to let you send data to {}{}. Tell them in one plain sentence why; \
                 retry once they allowed it.",
                card.all_hosts().join(", "),
                if made { "" } else { " (asked already, still waiting)" }
            ),
            false,
            made.then_some(card),
        ),
        Asked::Refused(when) => (
            format!(
                "Your person refused sending to this site ({when}): don't ask again today; do without it or ask them \
                 in words."
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
    fn sizes_in_plain_words() {
        assert_eq!(about(2 * 1024 * 1024 + 100_000), "environ 2,1 Mo");
        assert_eq!(about(2 * 1024 * 1024), "environ 2 Mo");
        assert_eq!(about(300 * 1024), "environ 300 Ko");
        assert_eq!(about(10), "environ 1 Ko");
        assert_eq!(about(25 * 1024 * 1024), "environ 25 Mo");
        assert_eq!(about(3 * 1024 * 1024 * 1024), "environ 3 Go");
    }

    #[test]
    fn a_send_cut_one_card_then_the_persons_word() {
        let (_dir, config) = config();
        let url = proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap();
        let token = load(&config).agents["vie"].token.clone();
        assert_eq!(url, format!("http://vie:{token}@10.233.80.2:3128"));
        assert_eq!(proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap(), url); // kept
        let policy: Value =
            serde_json::from_str(&std::fs::read_to_string(policy_dir(&config).join(POLICY_FILE)).unwrap()).unwrap();
        assert_eq!(policy["agents"]["vie"]["key"], json!(crate::agent_tools::digest(&token)));
        assert_eq!(policy["limits"]["send_site_hour"], json!(65536));
        assert!(policy["agents"]["vie"].get("any").is_none() && !policy.to_string().contains(&token));
        // the proxy cut a send: one card, not two
        let Asked::Card(card, true) = ask(&config, "vie", "upload.example.net", "", "proxy", 2 * 1024 * 1024) else {
            panic!()
        };
        assert!(matches!(ask(&config, "vie", "upload.example.net", "a backup", "agent", 0), Asked::Card(_, false)));
        assert_eq!(load(&config).pending[0].reason, "a backup");
        assert_eq!(ask(&config, "vie", "pypi.org", "", "agent", 0), Asked::Already);
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], json!(format!("bip-site-{}", card.id)));
        assert_eq!(items[0]["choices"], json!(["always", "deny"]));
        assert_eq!(items[0]["bip_site_kind"], json!("send"));
        assert_eq!(items[0]["bip_site_send_bytes"], json!(2 * 1024 * 1024));
        assert_eq!(items[0]["description"], json!("vie veut envoyer des données à upload.example.net (environ 2 Mo)"));
        // refused: quiet for a week from the proxy, a day when the agent asks itself
        assert!(answer(&config, "vie", &card.id, Decision::First(false), "app").unwrap().is_some());
        assert!(answer(&config, "vie", &card.id, Decision::First(false), "app").unwrap().is_none());
        assert!(matches!(ask(&config, "vie", "upload.example.net", "", "proxy", 100_000), Asked::Refused(_)));
        assert!(matches!(ask(&config, "vie", "upload.example.net", "", "agent", 0), Asked::Refused(_)));
        assert_eq!(crate::approvals_log::list(&config, "vie", None)[0]["choice"], json!("deny"));
        // allowed: validated, in the proxy's file, the counts reset
        let Asked::Card(card, true) = ask(&config, "vie", "api.meteo.fr", "", "agent", 0) else { panic!() };
        answer(&config, "vie", &card.id, Decision::First(true), "app").unwrap();
        assert!(validated(&config, "vie", "api.meteo.fr") && !validated(&config, "budget", "api.meteo.fr"));
        let policy = policy_of(&config, &load(&config));
        assert_eq!(policy["agents"]["vie"]["hosts"], json!(["api.meteo.fr"]));
        assert!(policy["agents"]["vie"]["reset"].as_f64().unwrap() > 0.0);
        // hosts reported a moment apart: one card (MAX_HOSTS on it), their sizes added, then another
        assert!(matches!(ask(&config, "vie", "s0.example.org", "", "proxy", 70_000), Asked::Card(_, true)));
        for i in 1..MAX_HOSTS {
            assert!(matches!(ask(&config, "vie", &format!("s{i}.example.org"), "", "proxy", 70_000), Asked::Card(_, false)));
        }
        assert_eq!(load(&config).pending[0].bytes, 70_000 * MAX_HOSTS as u64);
        assert!(matches!(ask(&config, "vie", "one-more.example.org", "", "proxy", 1), Asked::Card(_, true)));
        assert_eq!(load(&config).pending.len(), 2);
        // at most MAX_PENDING cards (each made out of the window)
        for i in 0..MAX_PENDING {
            change(&config, |s| s.pending.iter_mut().for_each(|p| p.at -= GROUP_WINDOW)).unwrap();
            let asked = ask(&config, "vie", &format!("late{i}.example.org"), "", "proxy", 1);
            assert_eq!(matches!(asked, Asked::Card(_, true)), i + 2 < MAX_PENDING, "{i}");
        }
        assert!(remove(&config, "vie", "api.meteo.fr").unwrap());
        assert!(!validated(&config, "vie", "api.meteo.fr"));
        forget(&config, "vie");
        assert!(load(&config).agents.is_empty() && load(&config).pending.is_empty());
    }

    fn with_agents(dir: &tempfile::TempDir, mut agents: Value) -> Config {
        for agent in agents.as_object_mut().unwrap().values_mut() {
            agent["hermes_key"] = json!("h".repeat(32));
        }
        crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy(), "agents": agents}),
            None,
        )
        .unwrap()
    }

    #[test]
    fn one_card_older_apps_answers() {
        let dir = tempfile::tempdir().unwrap();
        let config = with_agents(&dir, json!({"vie": {"hermes_url": "http://127.0.0.1:9300", "display_name": "Vie"}}));
        proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap();
        for host in ["a.fr", "b.fr", "c.fr"] {
            ask(&config, "vie", host, "", "proxy", 100 * 1024);
        }
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0]["description"],
            json!("Vie veut envoyer des données à 3 sites (environ 300 Ko) : a.fr, b.fr, c.fr")
        );
        assert_eq!((&items[0]["bip_site"], &items[0]["command"]), (&json!("a.fr"), &json!("a.fr, b.fr, c.fr")));
        // an older app: the first host only, the others on a card of their own
        let id = items[0]["request_id"].as_str().unwrap().to_string();
        answer(&config, "vie", &id, Decision::First(true), "app").unwrap().unwrap();
        assert!(validated(&config, "vie", "a.fr") && !validated(&config, "vie", "b.fr"));
        let card = load(&config).pending[0].clone();
        assert_eq!(card.all_hosts(), vec!["b.fr", "c.fr"]);
        // « Autoriser »
        answer(&config, "vie", &card.id, Decision::Hosts(true, vec!["b.fr".into(), "c.fr".into()]), "app").unwrap();
        assert!(validated(&config, "vie", "c.fr") && load(&config).pending.is_empty());
        // « Refuser » from a notification: all of them
        ask(&config, "vie", "d.fr", "", "proxy", 1);
        ask(&config, "vie", "e.fr", "", "proxy", 1);
        let card = load(&config).pending[0].clone();
        answer(&config, "vie", &card.id, decision_of_choice("deny"), "notification").unwrap();
        assert!(matches!(ask(&config, "vie", "e.fr", "", "proxy", 1), Asked::Refused(_)));
        // an older app's « Tous les sites pour Vie »: the hosts on the card, not every site
        ask(&config, "vie", "f.fr", "", "proxy", 1);
        let card = load(&config).pending[0].clone();
        answer(&config, "vie", &card.id, decision_of_choice("all_sites"), "notification").unwrap();
        assert!(validated(&config, "vie", "f.fr") && !validated(&config, "vie", "g.fr"));
        assert!(matches!(ask(&config, "vie", "g.fr", "", "proxy", 1), Asked::Card(_, true)));
        let kept: Vec<(Value, Value)> = crate::approvals_log::list(&config, "vie", None)
            .iter()
            .map(|k| (k["command"].clone(), k["choice"].clone()))
            .collect();
        assert!(kept.contains(&(json!("f.fr"), json!("always"))) && kept.contains(&(json!("d.fr, e.fr"), json!("deny"))));
    }

    #[test]
    fn the_old_store_keeps_its_lists_as_validated() {
        let dir = tempfile::tempdir().unwrap();
        let config = with_agents(&dir, json!({"vie": {"hermes_url": "http://127.0.0.1:9300"}}));
        let old = json!({"agents": {"vie": {"token": "t".repeat(32), "all_sites": false,
            "sites": [{"host": "data.gouv.fr", "at": "2026-10-24T10:00:00+00:00", "by": "request"}],
            "visited": {"www.lemonde.fr": 1.0e9}, "refused": {}}},
            "pending": [{"id": "abc", "agent": "vie", "host": "x.fr", "hosts": ["x.fr"], "reason": "", "via": "proxy",
                         "at": py::time_time()}]});
        std::fs::write(dir.path().join("sandbox-sites.json"), old.to_string()).unwrap();
        assert!(validated(&config, "vie", "data.gouv.fr") && !validated(&config, "vie", "www.lemonde.fr"));
        ensure_policy(&config).unwrap();
        let policy = policy_of(&config, &load(&config));
        assert_eq!(policy["agents"]["vie"]["hosts"], json!(["data.gouv.fr"]));
        assert!(policy["agents"]["vie"].get("any").is_none());
        // an old reading card still answers
        assert_eq!(pending_items(&config, Some("vie"))[0]["bip_site_send_bytes"], json!(0));
        let shown = view(&config, config.agent(Some("vie")).unwrap());
        assert_eq!((shown["reading_open"].clone(), shown["all_sites"].clone()), (json!(true), json!(true)));
    }

    #[test]
    fn the_proxys_reports_are_taken_once() {
        let (_dir, config) = config();
        ensure_policy(&config).unwrap();
        let file = reports_dir(&config).join(REPORTS_FILE);
        std::fs::write(
            &file,
            "{\"kind\":\"send\",\"agent\":\"vie\",\"host\":\"a.fr\",\"bytes\":70000}\nnot json\n\
             {\"kind\":\"send\",\"agent\":\"vie\",\"host\":\"a.fr\",\"bytes\":90000}\n{\"agent\":\"w\",\"host\":\"old.fr\"}\n\
             {\"kind\":\"send\",\"agent\":\"w\",\"host\":\"b.fr\",\"bytes\":1}\n",
        )
        .unwrap();
        assert_eq!(
            take_reports(&config),
            vec![
                Report { agent: "vie".into(), host: "a.fr".into(), bytes: 90000 },
                Report { agent: "w".into(), host: "b.fr".into(), bytes: 1 }
            ]
        );
        assert!(take_reports(&config).is_empty() && !file.exists());
    }
}
