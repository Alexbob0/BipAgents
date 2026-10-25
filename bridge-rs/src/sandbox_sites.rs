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
//! - **Fewer cards (owner, 2026-10-25: « il y a trop de demandes d'accord »)**:
//!   - *what the person's request opened already*: a site the same agent opened itself (a `browser_navigate` or a
//!     `web_extract` its run reported, a tab of its own Chrome) is open to its programs for `VISIT_TTL` (7 days), with
//!     its registrable domain and that domain's usual subdomains (`www.`, `static.`, `cdn.`…; `registrable`);
//!   - *one card per agent*: the hosts refused within `GROUP_WINDOW` gather on one card (« Vie veut joindre 3 sites
//!     depuis ses programmes : a, b, c »), choices `always` (these sites), `all_sites` (« Tous les sites pour Vie »)
//!     and `deny`. Older apps read `bip_site` (the first host) and answer `{allow}`: that host only, the others wait
//!     on a card of their own;
//!   - *« Tous les sites »*, a switch per agent (`all_sites`): its programs reach any public site on HTTPS, with no
//!     card (the proxy's `"any"`). Off by default for an agent that holds something sensitive (`sensitive`: vault
//!     items, a mailbox, health data, the money category), on for the others. Local and private addresses stay
//!     refused either way.
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
//!     GET    /v1/agents/{agent}/sandbox/sites                    {registries, sites, pending, effective, why,
//!                                                                  all_sites, all_sites_default, sensitive, visited}
//!     PATCH  /v1/agents/{agent}/sandbox/sites          {all_sites}   the « Tous les sites » switch
//!     POST   /v1/agents/{agent}/sandbox/sites          {host}    added by the person (`*.domain` allowed)
//!     DELETE /v1/agents/{agent}/sandbox/sites/{host}             removed
//!     POST   /v1/agents/{agent}/sandbox/sites/requests/{id} {allow, hosts?, all_sites?}   a card answered

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
/// Hosts refused within so long gather on one card.
pub const GROUP_WINDOW: f64 = 120.0;
/// Hosts on one card.
pub const MAX_HOSTS: usize = 8;
/// A site the agent opened itself (browser, web tools): open to its programs for so long.
pub const VISIT_TTL: f64 = 7.0 * 86400.0;
/// Sites opened kept per agent.
const MAX_VISITS: usize = 300;
/// A visit noted again only after so long (the file is not written at each page).
const VISIT_AGAIN: f64 = 3600.0;
/// The subdomains a site usually loads its parts from, opened with a site the agent visited.
const USUAL_SUBDOMAINS: [&str; 12] =
    ["www", "static", "cdn", "api", "assets", "img", "images", "media", "fonts", "files", "download", "m"];
/// Second levels under which a registrable domain has three labels (`gouv.fr`, `co.uk`).
const SECOND_LEVELS: [&str; 20] = [
    "co", "com", "net", "org", "edu", "ac", "gov", "gouv", "ltd", "plc", "me", "nom", "or", "ne", "go", "gob", "mil",
    "govt", "nic", "asso",
];
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
    /// « Tous les sites »: the person's choice (None: the default, `all_sites_default`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_sites: Option<bool>,
    /// Host → when the agent last opened it itself (browser, web tools).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub visited: BTreeMap<String, f64>,
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
    /// « proxy » (refused) or « agent » (asked first).
    pub via: String,
    pub at: f64,
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

/// The proxy's view: per agent, its token's SHA-256, its hosts (the person's, then those its visits opened) and
/// whether any site is open to it (nothing secret).
pub fn policy_of(config: &Config, store: &Store) -> Value {
    let now = py::time_time();
    let mut agents = Dict::new();
    for (name, entry) in &store.agents {
        if entry.token.is_empty() {
            continue;
        }
        let mut hosts: Vec<String> = entry.sites.iter().map(|s| s.host.clone()).collect();
        for host in visited_names(entry, now) {
            if !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        let mut item = json!({"key": crate::agent_tools::digest(&entry.token), "hosts": hosts});
        if all_sites_on(config, name, entry) {
            item["any"] = json!(true);
        }
        agents.insert(name.clone(), item);
    }
    json!({"agents": agents})
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

/// The registrable domain of a host, simply (`www.data.gouv.fr` → `data.gouv.fr`, `a.b.example.org` →
/// `example.org`, `www.bbc.co.uk` → `bbc.co.uk`): None under shared hosting (`x.github.io`), whose neighbours are
/// anybody's, and for a suffix itself (`co.uk`).
pub fn registrable(host: &str) -> Option<String> {
    if SHARED_HOSTING.iter().any(|s| host == *s || host.ends_with(&format!(".{s}"))) {
        return None;
    }
    let labels: Vec<&str> = host.trim_start_matches("*.").split('.').collect();
    if labels.len() < 2 {
        return None;
    }
    let second = labels[labels.len() - 2];
    let keep = if SECOND_LEVELS.contains(&second) || second.len() <= 2 { 3 } else { 2 };
    if labels.len() < keep {
        return None;
    }
    Some(labels[labels.len() - keep..].join("."))
}

/// The names a visit to `host` opens: itself, its registrable domain and that domain's usual subdomains.
pub fn visit_names(host: &str) -> Vec<String> {
    let mut names = vec![host.to_string()];
    if let Some(base) = registrable(host) {
        for name in std::iter::once(base.clone()).chain(USUAL_SUBDOMAINS.iter().map(|sub| format!("{sub}.{base}"))) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

/// The names the agent's recent visits open.
fn visited_names(entry: &AgentSites, now: f64) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (host, at) in &entry.visited {
        if now - at < VISIT_TTL {
            for name in visit_names(host) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// What the agent holds that a program on the open internet should not reach freely, if anything: « finance » (the
/// money category), « health » (its health switch), « mail » (a mailbox), « vault » (items of the vault given to it).
/// What cannot be read counts as sensitive.
pub fn sensitive(config: &Config, agent: &AgentConfig) -> Option<&'static str> {
    if agent.category.as_deref() == Some("finance") {
        return Some("finance");
    }
    if crate::health::enabled(config, &agent.name) {
        return Some("health");
    }
    match crate::mailbox::of(config).of_agent(&agent.name) {
        Ok(list) if list.is_empty() => {}
        _ => return Some("mail"),
    }
    let vault = crate::vault::Vault::new(&py::join(&config.data_dir(), "vault"));
    match vault.try_items() {
        Ok(items) if !items.iter().any(|i| i.agents.contains_key(&agent.name)) => None,
        _ => Some("vault"),
    }
}

/// « Tous les sites » when the person never chose: on, unless the agent holds something sensitive.
pub fn all_sites_default(config: &Config, agent: &str) -> bool {
    config.agent(Some(agent)).is_some_and(|a| sensitive(config, a).is_none())
}

fn all_sites_on(config: &Config, agent: &str, entry: &AgentSites) -> bool {
    entry.all_sites.unwrap_or_else(|| all_sites_default(config, agent))
}

/// Whether `host` is on the agent's list: a registry, one of its sites, or a site it opened lately.
fn listed(config: &Config, store: &Store, agent: &str, host: &str) -> bool {
    registries(config).iter().any(|r| r == host)
        || store.agents.get(agent).is_some_and(|a| {
            covered(&a.sites, host) || visited_names(a, py::time_time()).iter().any(|n| n == host)
        })
}

/// Whether the agent's sandbox may reach `host` already (on its list, or « Tous les sites »).
pub fn reachable(config: &Config, agent: &str, host: &str) -> bool {
    let store = load(config);
    let entry = store.agents.get(agent).cloned().unwrap_or_default();
    listed(config, &store, agent, host) || all_sites_on(config, agent, &entry)
}

/// The sites `text` names (`https://host/…`, a run's preview of a tool call), made plain.
pub fn hosts_in(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for (at, _) in text.match_indices("://") {
        let scheme = text[..at].rsplit(|c: char| !c.is_ascii_alphabetic()).next().unwrap_or("").to_lowercase();
        if scheme != "https" && scheme != "http" {
            continue;
        }
        let rest = &text[at + 3..];
        let end = rest.find(|c: char| c.is_whitespace() || "/?#\"'<>),]".contains(c)).unwrap_or(rest.len());
        if let Ok(host) = clean_host(&rest[..end], false) {
            if !found.contains(&host) && found.len() < 20 {
                found.push(host);
            }
        }
    }
    found
}

/// The agent opened `hosts` itself (its browser, its web tools): open to its programs for `VISIT_TTL`, and the cards
/// waiting about them answered by it. Written at most once per `VISIT_AGAIN` per host.
pub fn note_visits(config: &Config, agent: &str, hosts: &[String]) {
    let now = py::time_time();
    let known = load(config);
    let entry = known.agents.get(agent);
    let fresh: Vec<String> = hosts
        .iter()
        .filter_map(|h| clean_host(h, false).ok())
        .filter(|h| !registries(config).contains(h))
        .filter(|h| !entry.and_then(|e| e.visited.get(h)).is_some_and(|at| now - at < VISIT_AGAIN))
        .collect();
    if fresh.is_empty() {
        return;
    }
    let result = change(config, |store| {
        let entry = store.agents.entry(agent.to_string()).or_default();
        for host in &fresh {
            entry.visited.insert(host.clone(), now);
        }
        entry.visited.retain(|_, at| now - *at < VISIT_TTL);
        while entry.visited.len() > MAX_VISITS {
            let oldest = entry.visited.iter().min_by(|a, b| a.1.total_cmp(b.1)).map(|(h, _)| h.clone());
            let Some(oldest) = oldest else { break };
            entry.visited.remove(&oldest);
        }
        let opened = visited_names(entry, now);
        for card in store.pending.iter_mut().filter(|p| p.agent == agent) {
            let left: Vec<String> = card.all_hosts().into_iter().filter(|h| !opened.contains(h)).collect();
            if let Some(first) = left.first() {
                card.host = first.clone();
            }
            card.hosts = left;
        }
        store.pending.retain(|p| !(p.agent == agent && p.hosts.is_empty() && opened.contains(&p.host)));
    });
    match result {
        Ok(()) => tracing::info!(target: "bipbridge::sandbox", agent, hosts = %fresh.join(", "), "sites opened by the agent"),
        Err(exc) => tracing::warn!(target: "bipbridge::sandbox", agent, error = %exc, "sites opened not kept"),
    }
}

/// A tool call a run reported (`tool.started`: the tool and its preview): the sites a browser or a web tool opened.
/// Cheap for the others; the file is written off the caller's thread.
pub fn note_tool_call(config: &Config, agent: &str, tool: &str, preview: &str) {
    let tool = tool.to_ascii_lowercase();
    let opens = tool.ends_with("navigate") || tool.ends_with("web_extract") || tool.ends_with("web_crawl");
    if !opens || preview.is_empty() {
        return;
    }
    let mut hosts = hosts_in(preview);
    if hosts.is_empty() {
        // a navigation's preview may be the bare address
        if let Some(host) = preview.split_whitespace().next().and_then(|w| clean_host(w, false).ok()) {
            hosts.push(host);
        }
    }
    if hosts.is_empty() {
        return;
    }
    let (config, agent) = (config.clone(), agent.to_string());
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(move || note_visits(&config, &agent, &hosts));
        }
        Err(_) => note_visits(&config, &agent, &hosts),
    }
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
        .map(|p| {
            json!({"id": p.id, "host": p.host, "hosts": p.all_hosts(), "reason": p.reason, "via": p.via,
                   "at": iso(p.at)})
        })
        .collect();
    let (effective, why) = effect(config, agent);
    let now = py::time_time();
    let mut visited: Vec<(&String, &f64)> = entry.visited.iter().filter(|(_, at)| now - **at < VISIT_TTL).collect();
    visited.sort_by(|a, b| b.1.total_cmp(a.1));
    let sensitive = sensitive(config, agent);
    json!({
        "registries": registries(config),
        "sites": entry.sites,
        "pending": pending,
        "effective": effective,
        "why": why,
        "all_sites": entry.all_sites.unwrap_or(sensitive.is_none()),
        "all_sites_default": sensitive.is_none(),
        "sensitive": sensitive,
        "visited": visited.iter().map(|(h, at)| json!({"host": h, "at": iso(**at)})).collect::<Vec<_>>(),
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
        if let Some(p) = store.pending.iter_mut().find(|p| p.agent == agent && p.all_hosts().contains(&host)) {
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
pub fn sentence(shown: &str, hosts: &[String]) -> String {
    match hosts {
        [one] => format!("{shown} veut joindre {one} depuis ses programmes"),
        _ => format!("{shown} veut joindre {} sites depuis ses programmes : {}", hosts.len(), hosts.join(", ")),
    }
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
            let hosts = p.all_hosts();
            let item = json!({
                "agent": p.agent,
                "run_id": format!("{RUN_PREFIX}{}", p.id),
                "request_id": p.id,
                "command": hosts.join(", "),
                "description": sentence(&shown, &hosts),
                "choices": ["always", "all_sites", "deny"],
                "created_at": p.at,
                "session_id": null,
                "bip_site": p.host,
                "bip_sites": hosts,
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
    /// « Autoriser ces sites » (the hosts shown) or « Refuser » (them all).
    Hosts(bool, Vec<String>),
    /// An older app's answer, which showed the first host only: that one; the others wait on a card of their own.
    First(bool),
    /// « Refuser », every host on the card.
    DenyAll,
    /// « Tous les sites pour <agent> ».
    AllSites,
}

/// A card answered: the hosts allowed for good, or refused (not asked again for a while), or « Tous les sites » on.
/// The decision is kept with the person's others. None: no such card (answered already, or gone).
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
            Decision::AllSites => (true, Vec::new()),
        };
        let left: Vec<String> = match &decision {
            Decision::AllSites => Vec::new(),
            _ => hosts.iter().filter(|h| !chosen.contains(h)).cloned().collect(),
        };
        let entry = store.agents.entry(agent.to_string()).or_default();
        if decision == Decision::AllSites {
            entry.all_sites = Some(true);
        }
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
        let choice = match (&decision, allow) {
            (Decision::AllSites, _) => "all_sites",
            (_, true) => "always",
            (_, false) => "deny",
        };
        Some((card, shown_hosts, choice))
    })?;
    let Some((card, hosts, choice)) = decided else { return Ok(None) };
    {
        let shown = config.agent(Some(agent)).map(|a| a.display_name.clone()).unwrap_or_default();
        let shown = if shown.is_empty() { agent.to_string() } else { shown };
        // kept in the log's words (once/session/always/deny): « Tous les sites » is « always » for `*`
        let (command, description, kept) = if choice == "all_sites" {
            ("*".to_string(), format!("{shown} peut joindre tous les sites depuis ses programmes"), "always")
        } else {
            (hosts.join(", "), sentence(&shown, &hosts), choice)
        };
        let fields = json!({
            "session_id": null, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
            "command": command, "description": description, "topic": "site", "choice": kept, "via": via,
        });
        if let Some(map) = fields.as_object() {
            if let Err(exc) = crate::approvals_log::record(config, agent, map) {
                tracing::warn!(target: "bipbridge::sandbox", agent, error = %exc, "site decision not kept");
            }
        }
        tracing::info!(target: "bipbridge::sandbox", agent, hosts = %hosts.join(", "), choice, "site card answered");
    }
    Ok(Some(card))
}

/// The « Tous les sites » switch, set by the person.
pub fn set_all_sites(config: &Config, agent: &str, on: bool) -> std::io::Result<()> {
    change(config, |store| {
        store.agents.entry(agent.to_string()).or_default().all_sites = Some(on);
        if on {
            store.pending.retain(|p| p.agent != agent);
        }
    })?;
    tracing::info!(target: "bipbridge::sandbox", agent, on, "all sites for the agent's programs");
    Ok(())
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
        let isolated: Vec<AgentConfig> =
            config.agents.values().filter(|a| crate::sandbox::isolates(&config, a)).cloned().collect();
        let mut looked: Vec<String> = Vec::new();
        for (agent, host) in reports {
            let Some(found) = isolated.iter().find(|a| a.name == agent).cloned() else { continue };
            if !looked.contains(&agent) {
                // what its browser shows now first: a site the person's request opened asks nothing
                note_tabs(&config, &agent).await;
                looked.push(agent.clone());
            }
            let c = config.clone();
            let (a, h) = (agent.clone(), host.clone());
            let asked = tokio::task::spawn_blocking(move || ask(&c, &a, &h, "", "proxy")).await;
            if let Ok(Asked::Card(card, true)) = asked {
                notify(&state, &found, &card).await;
            }
        }
        if ticks.is_multiple_of(10) {
            for agent in isolated.iter().filter(|a| !looked.contains(&a.name)) {
                note_tabs(&config, &agent.name).await;
            }
            // visits gone stale, a default changed (a mailbox connected, the health switch): the proxy's file
            // follows within half a minute (written only when it changed)
            let c = config.clone();
            let _ = tokio::task::spawn_blocking(move || ensure_policy(&c)).await;
        }
        if ticks % 100 == 1 {
            let c = config.clone();
            let _ = tokio::task::spawn_blocking(move || crate::sandbox_adopt::tend_links(&c)).await;
        }
    }
}

/// The sites the agent's own Chrome shows (its tabs), noted as opened by it.
async fn note_tabs(config: &Config, agent: &str) {
    let Some(port) = crate::desktop_page::cdp_port(config, agent) else { return };
    let token = crate::browsers::hold_token(config, agent);
    let Ok(tabs) = crate::desktop_page::tabs(port, token.as_deref()).await else { return };
    let hosts: Vec<String> = tabs.iter().flat_map(|t| hosts_in(&t.url)).collect();
    if hosts.is_empty() {
        return;
    }
    let (c, a) = (config.clone(), agent.to_string());
    let _ = tokio::task::spawn_blocking(move || note_visits(&c, &a, &hosts)).await;
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
    /// The hosts the card showed (an older app sends none: the first host only).
    #[serde(default)]
    pub hosts: Option<Vec<String>>,
    /// « Tous les sites pour <agent> ».
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

/// A card answered through `/v1/approve` (a notification, or an app that answers Hermes' way): `all_sites`,
/// `deny` (all its hosts), else an older app's « Autoriser toujours » (the first host).
pub fn decision_of_choice(choice: &str) -> Decision {
    match choice.trim().to_lowercase().as_str() {
        "all_sites" => Decision::AllSites,
        "deny" => Decision::DenyAll,
        _ => Decision::First(true),
    }
}

#[derive(Debug, Deserialize)]
pub struct Switch {
    pub all_sites: bool,
}

impl Validate for Switch {
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
    match answer(&config, &agent.name, &id, body.decision(), "app").map_err(anyhow::Error::from)? {
        Some(_) => Ok(web::ok(view(&config, &agent))),
        None => Err(HttpError::new(404, "no such request (answered already?)")),
    }
}

async fn switch_route(
    State(state): State<AppState>,
    UrlPath(name): UrlPath<String>,
    Json(body): Json<Switch>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let (c, a) = (config.clone(), agent.name.clone());
    tokio::task::spawn_blocking(move || set_all_sites(&c, &a, body.all_sites))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(anyhow::Error::from)?;
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
            "description": "List the sites your programs (pip, npm, curl, your scripts) can reach from your sandbox: the \
        package registries, always, and the sites your person allowed you.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": read_only,
        }),
        // Marked read-only: it only shows the person a card, which is their decision (no second question first).
        json!({
            "name": REQUEST,
            "description": "Ask your person to let your programs reach a site from your sandbox (HTTPS only), when a \
        script or a tool you run needs it (an API, data they asked for): they see a card in the app. Give the site's \
        exact name and, in a few plain words, why. Then tell them in one sentence; retry once they allowed it. A site \
        your programs were refused is asked about by itself: ask here only to explain why, or to ask first.",
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
        if all_sites_on(config, &agent.name, &entry) {
            lines.push("Your person opened every public HTTPS site to your programs.".into());
        }
        let opened: Vec<String> = entry
            .visited
            .iter()
            .filter(|(_, at)| py::time_time() - **at < VISIT_TTL)
            .map(|(h, _)| h.clone())
            .collect();
        if !opened.is_empty() {
            lines.push(format!("Open for a week, as you visited them (and their www., static.…): {}", opened.join(", ")));
        }
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
        lines.push(
            "Read web pages with your web tools or your browser, never curl or a script; programs only for an API or \
             data your person asked for."
                .into(),
        );
        return (lines.join("\n"), false, None);
    }
    let text = |key: &str| arguments.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    match ask(config, &agent.name, &text("host"), &text("reason"), "agent") {
        Asked::Already => ("Your programs can reach it already.".into(), false, None),
        Asked::Card(card, made) => (
            format!(
                "Your person sees a card asking to let your programs reach {}{}. Tell them in one plain sentence why; \
                 retry once they allowed it.",
                card.all_hosts().join(", "),
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
        assert_eq!(items[0]["choices"], json!(["always", "all_sites", "deny"]));
        assert_eq!(items[0]["bip_sites"], json!(["data.gouv.fr"]));
        // refused: quiet for a week from the proxy, a day when the agent asks itself
        assert!(answer(&config, "vie", &card.id, Decision::First(false), "app").unwrap().is_some());
        assert!(answer(&config, "vie", &card.id, Decision::First(false), "app").unwrap().is_none());
        assert!(matches!(ask(&config, "vie", "data.gouv.fr", "", "proxy"), Asked::Refused(_)));
        assert!(matches!(ask(&config, "vie", "data.gouv.fr", "", "agent"), Asked::Refused(_)));
        assert_eq!(crate::approvals_log::list(&config, "vie", None)[0]["choice"], json!("deny"));
        // allowed: in the list and the proxy's file
        let Asked::Card(card, true) = ask(&config, "vie", "api.meteo.fr", "", "agent") else { panic!() };
        answer(&config, "vie", &card.id, Decision::First(true), "app").unwrap();
        assert!(reachable(&config, "vie", "api.meteo.fr") && !reachable(&config, "budget", "api.meteo.fr"));
        let policy = std::fs::read_to_string(policy_dir(&config).join(POLICY_FILE)).unwrap();
        assert!(policy.contains("api.meteo.fr"));
        // hosts asked a moment apart: one card (MAX_HOSTS on it), then another
        assert!(matches!(ask(&config, "vie", "s0.example.org", "", "proxy"), Asked::Card(_, true)));
        for i in 1..MAX_HOSTS {
            assert!(matches!(ask(&config, "vie", &format!("s{i}.example.org"), "", "proxy"), Asked::Card(_, false)));
        }
        assert!(matches!(ask(&config, "vie", "one-more.example.org", "", "proxy"), Asked::Card(_, true)));
        assert_eq!(load(&config).pending.len(), 2);
        // at most MAX_PENDING cards (each made out of the window)
        for i in 0..MAX_PENDING {
            change(&config, |s| s.pending.iter_mut().for_each(|p| p.at -= GROUP_WINDOW)).unwrap();
            let asked = ask(&config, "vie", &format!("late{i}.example.org"), "", "proxy");
            assert_eq!(matches!(asked, Asked::Card(_, true)), i + 2 < MAX_PENDING, "{i}");
        }
        assert!(remove(&config, "vie", "api.meteo.fr").unwrap());
        assert!(!reachable(&config, "vie", "api.meteo.fr"));
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
    fn a_registrable_domain_simply() {
        assert_eq!(registrable("www.data.gouv.fr").as_deref(), Some("data.gouv.fr"));
        assert_eq!(registrable("a.b.example.org").as_deref(), Some("example.org"));
        assert_eq!(registrable("example.org").as_deref(), Some("example.org"));
        assert_eq!(registrable("news.bbc.co.uk").as_deref(), Some("bbc.co.uk"));
        assert_eq!(registrable("co.uk"), None);
        assert_eq!(registrable("x.github.io"), None);
        assert_eq!(registrable("bucket.s3.amazonaws.com"), None);
        let names = visit_names("news.example.org");
        assert_eq!(&names[..3], &["news.example.org", "example.org", "www.example.org"]);
        assert!(names.contains(&"static.example.org".to_string()) && !names.contains(&"evil.example.org".to_string()));
        assert_eq!(visit_names("me.github.io"), vec!["me.github.io"]);
        assert_eq!(
            hosts_in("{\"urls\": [\"https://Www.Lemonde.fr/a?b\", \"http://x.org:8080/\"]} ftp://no.fr https://localhost/"),
            vec!["www.lemonde.fr", "x.org"]
        );
    }

    #[test]
    fn a_site_the_agent_opened_asks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = with_agents(&dir, json!({"vie": {"hermes_url": "http://127.0.0.1:9300"}}));
        proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap();
        set_all_sites(&config, "vie", false).unwrap();
        // a card waits for static.lemonde.fr; then the browser opens www.lemonde.fr: answered by it
        assert!(matches!(ask(&config, "vie", "static.lemonde.fr", "", "proxy"), Asked::Card(_, true)));
        assert!(matches!(ask(&config, "vie", "evil.example", "", "proxy"), Asked::Card(_, false)));
        note_tool_call(&config, "vie", "browser_navigate", "https://www.lemonde.fr/politique/");
        assert_eq!(load(&config).pending[0].all_hosts(), vec!["evil.example"]);
        assert_eq!(ask(&config, "vie", "lemonde.fr", "", "proxy"), Asked::Already);
        assert_eq!(ask(&config, "vie", "cdn.lemonde.fr", "", "proxy"), Asked::Already);
        assert!(matches!(ask(&config, "vie", "tracker.lemonde.fr", "", "proxy"), Asked::Card(_, false)));
        let policy = policy_of(&config, &load(&config));
        let hosts = policy["agents"]["vie"]["hosts"].as_array().unwrap();
        assert!(hosts.contains(&json!("www.lemonde.fr")) && hosts.contains(&json!("static.lemonde.fr")));
        assert!(policy["agents"]["vie"].get("any").is_none());
        // web_extract and a bare address too; local ones never
        note_tool_call(&config, "vie", "web_extract", "[\"https://data.gouv.fr/x\"]");
        note_tool_call(&config, "vie", "browser_navigate", "meteo.fr");
        note_tool_call(&config, "vie", "browser_navigate", "http://192.168.1.1/");
        note_tool_call(&config, "vie", "terminal", "curl https://other.fr");
        let visited: Vec<String> = load(&config).agents["vie"].visited.keys().cloned().collect();
        assert_eq!(visited, vec!["data.gouv.fr", "meteo.fr", "www.lemonde.fr"]);
        // a week later: closed again
        change(&config, |s| s.agents.get_mut("vie").unwrap().visited.values_mut().for_each(|at| *at -= VISIT_TTL))
            .unwrap();
        assert!(!reachable(&config, "vie", "www.lemonde.fr"));
    }

    #[test]
    fn one_card_three_choices() {
        let dir = tempfile::tempdir().unwrap();
        let config = with_agents(&dir, json!({"vie": {"hermes_url": "http://127.0.0.1:9300", "display_name": "Vie"}}));
        proxy_url(&config, "vie", "http://10.233.80.2:3128").unwrap();
        set_all_sites(&config, "vie", false).unwrap();
        for host in ["a.fr", "b.fr", "c.fr"] {
            ask(&config, "vie", host, "", "proxy");
        }
        let items = pending_items(&config, Some("vie"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["description"], json!("Vie veut joindre 3 sites depuis ses programmes : a.fr, b.fr, c.fr"));
        assert_eq!((&items[0]["bip_site"], &items[0]["command"]), (&json!("a.fr"), &json!("a.fr, b.fr, c.fr")));
        // an older app: the first host only, the others on a card of their own
        let id = items[0]["request_id"].as_str().unwrap().to_string();
        answer(&config, "vie", &id, Decision::First(true), "app").unwrap().unwrap();
        assert!(reachable(&config, "vie", "a.fr") && !reachable(&config, "vie", "b.fr"));
        let card = load(&config).pending[0].clone();
        assert_eq!(card.all_hosts(), vec!["b.fr", "c.fr"]);
        // « Autoriser ces sites »
        answer(&config, "vie", &card.id, Decision::Hosts(true, vec!["b.fr".into(), "c.fr".into()]), "app").unwrap();
        assert!(reachable(&config, "vie", "c.fr") && load(&config).pending.is_empty());
        // « Refuser » from a notification: all of them
        ask(&config, "vie", "d.fr", "", "proxy");
        ask(&config, "vie", "e.fr", "", "proxy");
        let card = load(&config).pending[0].clone();
        answer(&config, "vie", &card.id, decision_of_choice("deny"), "notification").unwrap();
        assert!(matches!(ask(&config, "vie", "e.fr", "", "proxy"), Asked::Refused(_)));
        // « Tous les sites pour Vie »
        ask(&config, "vie", "f.fr", "", "proxy");
        let card = load(&config).pending[0].clone();
        answer(&config, "vie", &card.id, decision_of_choice("all_sites"), "notification").unwrap();
        assert_eq!(ask(&config, "vie", "g.fr", "", "proxy"), Asked::Already);
        assert_eq!(policy_of(&config, &load(&config))["agents"]["vie"]["any"], json!(true));
        let kept: Vec<(Value, Value)> = crate::approvals_log::list(&config, "vie", None)
            .iter()
            .map(|k| (k["command"].clone(), k["choice"].clone()))
            .collect();
        assert!(kept.contains(&(json!("*"), json!("always"))) && kept.contains(&(json!("d.fr, e.fr"), json!("deny"))));
    }

    #[test]
    fn all_sites_by_default_unless_the_agent_holds_something_sensitive() {
        let dir = tempfile::tempdir().unwrap();
        let config = with_agents(
            &dir,
            json!({"vie": {"hermes_url": "http://127.0.0.1:9300"},
                   "budget": {"hermes_url": "http://127.0.0.1:9301", "category": "finance"}}),
        );
        assert!(all_sites_default(&config, "vie") && !all_sites_default(&config, "budget"));
        assert_eq!(sensitive(&config, config.agent(Some("budget")).unwrap()), Some("finance"));
        assert!(!all_sites_default(&config, "nobody"));
        for name in ["vie", "budget"] {
            proxy_url(&config, name, "http://10.233.80.2:3128").unwrap();
        }
        let policy = policy_of(&config, &load(&config));
        assert_eq!(policy["agents"]["vie"]["any"], json!(true));
        assert!(policy["agents"]["budget"].get("any").is_none());
        assert_eq!(ask(&config, "vie", "x.fr", "", "proxy"), Asked::Already);
        assert!(matches!(ask(&config, "budget", "x.fr", "", "proxy"), Asked::Card(_, true)));
        // switched on: the cards waiting go
        set_all_sites(&config, "budget", true).unwrap();
        assert!(load(&config).pending.is_empty());
        set_all_sites(&config, "vie", false).unwrap();
        assert!(policy_of(&config, &load(&config))["agents"]["vie"].get("any").is_none());
        let shown = view(&config, config.agent(Some("vie")).unwrap());
        assert_eq!((shown["all_sites"].clone(), shown["all_sites_default"].clone()), (json!(false), json!(true)));
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
