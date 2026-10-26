//! Messages between agents, through the bridge, only where the person allowed them.
//!
//! Hermes' Bot Mode lets an agent message another one (`message_agent`) on a « peer »: an address and a key in
//! its config (`bot_peers`). Giving an agent another agent's key would let it do anything with that agent. So
//! the bridge is the only peer: each gateway gets a token of its own, which says who is calling and nothing more,
//! and the bridge forwards to the target with the target's key, but only along a link the person turned on in
//! the app (« Qui peut écrire à qui »). The target answers in its permanent « Bot Chat », as with any peer.
//!
//! A peer only reaches the target's « Bot Chat »: finding or opening it, writing in it, following or stopping a run
//! there (`only_bot_chat`). Its other conversations, its approvals and its runs elsewhere stay out of reach, whatever
//! an agent tricked by what it read would try with its token.
//!
//! Hermes delivers these messages as the gateway (`hermes -p default peer dm`): the token says which gateway
//! calls, and the message's signature (« Message from … (@agent): ») which of its agents. An agent in its own
//! space is alone in its gateway, so it is known for sure; in the shared hub, an agent could sign as another hub
//! agent (same person, nothing secret there). Agents of the hub can also message each other directly.
//!
//! Port of `bridge/bipbridge/peers.py` (owner group: agents).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post, put};
use axum::Router;
use futures::TryStreamExt;
use regex::Regex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{own_url, split, HouseholdError, Households};
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::Dict;

pub use crate::households::SEPARATOR;

/// The permanent conversation where an agent gets its peers' messages (Hermes' Bot Mode).
pub const BOT_CHAT_TITLE: &str = "Bot Chat";
/// What agents see in their roster: message_agent target "bip/<agent>".
pub const PEER_NAME: &str = "bip";
/// Headers never forwarded (hop by hop, or the caller's own key).
const HOP: [&str; 7] =
    ["host", "content-length", "connection", "keep-alive", "transfer-encoding", "authorization", "upgrade"];
const ALLOWED_PATHS: [&str; 3] = ["api/sessions", "v1/runs", "v1/capabilities"];

fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Python's `ValueError` of `apply` and `manual_token` (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct InvalidLink(pub String);

/// The links the person allowed between agents, and the tokens given to their gateways (`peers.json`).
pub struct Links {
    pub path: PathBuf,
}

/// The `links` of a loaded `peers.json` as pairs.
pub fn links_of(data: &Dict) -> Vec<(String, String)> {
    data.get("links")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|x| {
                    x.as_array().filter(|p| p.len() == 2).map(|p| (crate::py::py_str(&p[0]), crate::py::py_str(&p[1])))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn pairs_value(pairs: &[(String, String)]) -> Value {
    Value::Array(pairs.iter().map(|(a, b)| json!([a, b])).collect())
}

impl Links {
    pub fn new(path: &str) -> Self {
        Links { path: PathBuf::from(path) }
    }

    /// `{"links": [[from, to]…], "tokens": {gateway: digest}}`.
    pub fn load(&self) -> Dict {
        let data: Value = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        let data = data.as_object().cloned().unwrap_or_default();
        let mut out = Dict::new();
        out.insert("links".into(), pairs_value(&links_of(&data)));
        out.insert("tokens".into(), data.get("tokens").cloned().unwrap_or_else(|| json!({})));
        out
    }

    pub fn save(&self, data: &Dict) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut links = links_of(data);
        links.sort();
        let content =
            json!({"links": pairs_value(&links), "tokens": data.get("tokens").cloned().unwrap_or_else(|| json!({}))});
        let tmp = format!("{}.tmp", self.path.display());
        crate::py::write_with_mode(&tmp, crate::py::dumps(&content).as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// The gateway (see `gateway_of`) this token was given to.
    pub fn caller(&self, token: &str) -> Option<String> {
        let wanted = digest(token);
        let data = self.load();
        let tokens = data.get("tokens")?.as_object()?;
        tokens
            .iter()
            .find(|(_, d)| {
                d.as_str().map(|d| crate::auth::constant_time_eq(d.as_bytes(), wanted.as_bytes())).unwrap_or(false)
            })
            .map(|(gw, _)| gw.clone())
    }

    pub fn allowed(&self, sender: &str, target: &str) -> bool {
        links_of(&self.load()).iter().any(|(s, t)| s == sender && t == target)
    }
}

/// The HERMES_HOME of the gateway serving an agent this bridge manages (None: set up by hand).
pub fn managed_home(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    if let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(space));
    }
    if config.hermes.enabled() && agent.hermes_url.starts_with(&format!("{}/p/", config.hermes.hub_url)) {
        let home = if config.hermes.home.is_empty() {
            crate::py::join(&crate::py::expanduser("~"), ".hermes")
        } else {
            config.hermes.home.clone()
        };
        return Some(PathBuf::from(home));
    }
    None
}

/// Which gateway serves the agent: its HERMES_HOME, or « manual:<agent> » for one set up by hand.
pub fn gateway_of(config: &Config, agent: &AgentConfig) -> String {
    match managed_home(config, agent) {
        Some(home) => home.to_string_lossy().into_owned(),
        None => format!("manual:{}", agent.name),
    }
}

pub static SIGNATURE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Message from .{0,200}? \(@([A-Za-z0-9_-]{1,64})\):").expect("valid regex"));

/// `json.loads(body or b"{}")` as a dict (anything else: empty).
fn body_json(body: &[u8]) -> Option<Value> {
    if body.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_slice(body).ok()
}

/// The agent of `gateway` that signed this message (Hermes puts « Message from … (@agent): » first).
pub fn sender_of(config: &Config, gateway: &str, body: &[u8]) -> Option<String> {
    let data = body_json(body)?;
    let text = match &data {
        Value::Object(map) => {
            let value = map
                .get("message")
                .filter(|v| crate::py::truthy(v))
                .or_else(|| map.get("input").filter(|v| crate::py::truthy(v)));
            value.map(crate::py::py_str).unwrap_or_default()
        }
        _ => String::new(),
    };
    let found = SIGNATURE.captures(&text)?;
    let agent = config.agent(Some(&found[1]))?;
    if gateway_of(config, agent) == gateway {
        Some(agent.name.clone())
    } else {
        None
    }
}

/// The bridge as the agents reach it (`peer_url`, default http://127.0.0.1:<port>/peer).
pub fn peer_url(config: &Config) -> String {
    match config.peer_url.as_deref().filter(|u| !u.is_empty()) {
        Some(url) => url.to_string(),
        None => format!("http://127.0.0.1:{}/peer", config.port),
    }
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// The person's links. The gateway of each sender gets its token and the bridge as its peer; a gateway with no
/// sender left loses its token. Returns `{"manual": {agent: ""}}`: the agents set up by hand that need their token
/// from the administrator. `InvalidLink` (Python's `ValueError`) for a link that cannot be.
pub async fn apply(
    config: &Config,
    links: &Links,
    wanted: &[(String, String)],
    run: &Run,
    households: Option<&Households>,
) -> anyhow::Result<Dict> {
    let linked: HashSet<String> = match households {
        Some(h) => h
            .load()
            .get("households")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default(),
        None => HashSet::new(),
    };
    let known = |name: &str| match split(name) {
        Some((_, household)) => linked.contains(&household),
        None => config.agent(Some(name)).is_some(),
    };
    for (sender, target) in wanted {
        // Between two households, one end is ours: « vie → cuisine__lea » (ours writes) or « vie__alex → cuisine ».
        if sender == target || !known(sender) || !known(target) || (split(sender).is_some() && split(target).is_some())
        {
            return Err(InvalidLink(format!("unknown link {sender} → {target}")).into());
        }
    }
    let mut data = links.load();
    let mut sending: BTreeMap<String, AgentConfig> = BTreeMap::new();
    for (sender, _) in wanted {
        if split(sender).is_some() {
            continue;
        }
        if let Some(agent) = config.agent(Some(sender)) {
            sending.insert(gateway_of(config, agent), agent.clone());
        }
    }
    let mut tokens = data.get("tokens").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut manual = Dict::new();
    let key_env = format!("HERMES_PEER_{}_KEY", PEER_NAME.to_uppercase());
    for (gateway, agent) in &sending {
        if tokens.contains_key(gateway) {
            continue;
        }
        if gateway.starts_with("manual:") {
            manual.insert(agent.name.clone(), json!("")); // the administrator adds it (bipbridge peer-token)
            continue;
        }
        let token = crate::py::token_urlsafe(32);
        let env = BTreeMap::from([("HERMES_HOME".to_string(), gateway.clone())]);
        // The gateway's own (default) profile: Hermes reads the peer and its key there when it delivers.
        run(
            args(&[&config.hermes.bin, "config", "set", &format!("bot_peers.{PEER_NAME}.url"), &peer_url(config)]),
            env.clone(),
        )
        .await?;
        run(
            args(&[
                &config.hermes.bin,
                "config",
                "set",
                &format!("bot_peers.{PEER_NAME}.note"),
                "Tes autres agents, par BipAgents (cible : bip/<agent>)",
            ]),
            env.clone(),
        )
        .await?;
        crate::services::set_env(&Path::new(gateway).join(".env"), &key_env, Some(&token))?;
        tokens.insert(gateway.clone(), json!(digest(&token)));
    }
    // A gateway that sends nothing any more loses its token, when the bridge can give it a new one by itself. An agent
    // set up by hand keeps it: only its administrator could put a new one in, and without a link it is refused anyway.
    let stale: Vec<String> =
        tokens.keys().filter(|g| !sending.contains_key(*g) && !g.starts_with("manual:")).cloned().collect();
    for gateway in stale {
        if Path::new(&gateway).is_dir() {
            crate::services::set_env(&Path::new(&gateway).join(".env"), &key_env, None)?;
        }
        tokens.shift_remove(&gateway);
    }
    let unique: BTreeSet<(String, String)> = wanted.iter().cloned().collect();
    let pairs: Vec<(String, String)> = unique.into_iter().collect();
    data.insert("links".into(), pairs_value(&pairs));
    data.insert("tokens".into(), Value::Object(tokens));
    links.save(&data)?;
    write_contacts(config, &pairs, households);
    tracing::info!(links = pairs.len(), gateways = sending.len(), "agent links set");
    let mut out = Dict::new();
    out.insert("manual".into(), Value::Object(manual));
    Ok(out)
}

const CONTACTS: &str = "---
name: agent-contacts
description: \"The other agents you may write to with message_agent or give files to with send_files_to_agent, and how to address them.\"
version: 1.1.0
author: BipAgents
license: MIT
---

# Agents you may write to

Use `message_agent` with one of these targets (the person allowed exactly these; any other is refused):

{lines}

An answer arrives in your « Bot Chat ». A message from another household's agent is signed with its household,
for example « Message from Vie (Alex) (@vie__alex) »: answer it at `bip/vie__alex`.

Another agent never sees your files, nor a path you write to it, and there is no common folder to look for. To give
files to one of your person's agents, use `send_files_to_agent` (it says where that agent finds them), then tell it
with `message_agent`, giving those paths. Another household's agents get no files.
";

pub fn contacts_skill_path(home: &Path, agent: &str) -> PathBuf {
    home.join("profiles").join(agent).join("skills").join("agent-contacts").join("SKILL.md")
}

/// Each agent this bridge manages gets the list of agents it may write to (a skill in its profile).
pub fn write_contacts(config: &Config, links: &[(String, String)], households: Option<&Households>) {
    let names: BTreeMap<String, String> = households
        .map(|h| {
            h.public()
                .iter()
                .map(|x| {
                    (
                        x.get("id").map(crate::py::py_str).unwrap_or_default(),
                        x.get("name").map(crate::py::py_str).unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for agent in config.agents.values() {
        let Some(home) = managed_home(config, agent) else { continue };
        let skill = contacts_skill_path(&home, &agent.name);
        let mut targets: Vec<&String> = links.iter().filter(|(s, _)| *s == agent.name).map(|(_, t)| t).collect();
        targets.sort();
        if targets.is_empty() {
            if skill.exists() {
                let _ = std::fs::remove_file(&skill);
            }
            continue;
        }
        let mut lines = Vec::new();
        for target in targets {
            match split(target) {
                Some((other, household)) => {
                    let household_name = names.get(&household).cloned().unwrap_or(household);
                    lines.push(format!("- `bip/{target}`: {other}, an agent of {household_name}'s household"));
                }
                None => {
                    let shown =
                        config.agent(Some(target)).map(|a| a.display_name.clone()).unwrap_or_else(|| target.clone());
                    lines.push(format!("- `bip/{target}`: {shown}"));
                }
            }
        }
        let written = skill
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&skill, CONTACTS.replace("{lines}", &lines.join("\n"))));
        if written.is_err() {
            tracing::warn!(agent = %agent.name, "contacts not written");
        }
    }
}

/// A new token for an agent set up by hand (its old one stops working). Printed once, never stored.
pub fn manual_token(config: &Config, links: &Links, name: &str) -> anyhow::Result<String> {
    let Some(agent) = config.agent(Some(name)) else {
        return Err(InvalidLink(format!("unknown agent {name}")).into());
    };
    let token = crate::py::token_urlsafe(32);
    let mut data = links.load();
    let mut tokens = data.get("tokens").and_then(Value::as_object).cloned().unwrap_or_default();
    tokens.insert(gateway_of(config, agent), json!(digest(&token)));
    data.insert("tokens".into(), Value::Object(tokens));
    links.save(&data)?;
    Ok(token)
}

// -- the peer protocol ------------------------------------------------------------------------------------------------

/// `json.loads(body)` as a dict (`_json`): anything else is empty.
fn json_dict(body: &[u8]) -> Dict {
    match body_json(body) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

/// `urllib.parse.quote_plus`-style encoding of a query, as httpx writes `params=`.
fn encode_query(pairs: &[(String, String)]) -> String {
    const SAFE: &percent_encoding::AsciiSet =
        &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b' ');
    let enc = |s: &str| percent_encoding::utf8_percent_encode(s, SAFE).to_string().replace(' ', "+");
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

/// `urllib.parse.quote(text)` (`/` kept).
fn quote(text: &str) -> String {
    const SAFE: &percent_encoding::AsciiSet =
        &percent_encoding::NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/');
    percent_encoding::utf8_percent_encode(text, SAFE).to_string()
}

/// `urllib.parse.unquote(text)`.
fn unquote(text: &str) -> String {
    percent_encoding::percent_decode_str(text).decode_utf8_lossy().into_owned()
}

async fn from_target(state: &AppState, agent: &AgentConfig, path: &str, params: &[(String, String)]) -> Option<Value> {
    let mut url = format!("{}/{path}", agent.hermes_url);
    if !params.is_empty() {
        url = format!("{url}?{}", encode_query(params));
    }
    let resp = state
        .http
        .get(url)
        .bearer_auth(&agent.hermes_key)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;
    if resp.status().as_u16() != 200 {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The target's Bot Chat, as its own Hermes lists it (hidden sessions included).
async fn bot_chat(state: &AppState, agent: &AgentConfig) -> Option<Dict> {
    let params = [("limit", "200"), ("title", BOT_CHAT_TITLE), ("include_hidden", "1")]
        .map(|(k, v)| (k.to_string(), v.to_string()));
    let listing = from_target(state, agent, "api/sessions", &params).await?;
    let rows = listing.as_object()?.get("data")?;
    let rows = rows.as_array()?;
    rows.iter()
        .filter_map(Value::as_object)
        .find(|row| {
            row.get("title").filter(|t| crate::py::truthy(t)).map(crate::py::py_str).unwrap_or_default().trim()
                == BOT_CHAT_TITLE
        })
        .cloned()
}

fn id_of(row: &Dict) -> String {
    row.get("id").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default()
}

static CHAT_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^api/sessions/([^/]+)/chat\z").expect("valid regex"));
static RUN_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^v1/runs/([^/]+)(/stop)?\z").expect("valid regex"));

/// What a peer may do with `agent`: the calls of `hermes peer` (dm, send, status, stop), on its Bot Chat only.
/// Returns the answer when the bridge gives it itself (the Bot Chat's listing, never the others'), None to forward
/// the call; anything else is refused.
async fn only_bot_chat(
    state: &AppState,
    agent: &AgentConfig,
    method: &Method,
    path: &str,
    query: &QueryMap,
    body: &[u8],
) -> ApiResult<Option<Response>> {
    let refuse = || HttpError::new(403, format!("only {}'s Bot Chat is reachable", agent.name));
    if *method == Method::GET && path == "v1/capabilities" {
        return Ok(None);
    }
    if *method == Method::GET && path == "api/sessions" {
        if query.get("title") != Some(BOT_CHAT_TITLE) {
            return Err(refuse());
        }
        let row = bot_chat(state, agent).await;
        let data: Vec<Value> = row.into_iter().map(Value::Object).collect();
        return Ok(Some(web::ok(json!({"object": "list", "data": data}))));
    }
    if *method == Method::POST && path == "api/sessions" {
        if json_dict(body).get("title") != Some(&json!(BOT_CHAT_TITLE)) {
            return Err(refuse());
        }
        return Ok(None);
    }
    let chat = CHAT_PATH.captures(path);
    if *method == Method::POST && (chat.is_some() || path == "v1/runs") {
        let wanted = match &chat {
            Some(c) => unquote(&c[1]),
            None => json_dict(body)
                .get("session_id")
                .filter(|v| crate::py::truthy(v))
                .map(crate::py::py_str)
                .unwrap_or_default(),
        };
        let row = bot_chat(state, agent).await;
        match row {
            Some(row) if !wanted.is_empty() && wanted == id_of(&row) => return Ok(None),
            _ => return Err(refuse()),
        }
    }
    if let Some(run) = RUN_PATH.captures(path) {
        if (*method == Method::GET) == run.get(2).is_none() {
            let status = from_target(state, agent, &format!("v1/runs/{}", &run[1]), &[]).await;
            let row = bot_chat(state, agent).await;
            let session = status.as_ref().and_then(Value::as_object).map(|s| {
                s.get("session_id").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default()
            });
            match (session, row) {
                (Some(session), Some(row)) if session == id_of(&row) => return Ok(None),
                _ => return Err(refuse()),
            }
        }
    }
    Err(refuse())
}

fn is_message(method: &Method, path: &str) -> bool {
    *method == Method::POST && (path.ends_with("/chat") || path == "v1/runs")
}

/// The caller's token (`authorization` without its « Bearer » prefix).
fn bearer(headers: &HeaderMap) -> String {
    let header =
        headers.get("authorization").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
    header.strip_prefix("Bearer ").unwrap_or(&header).trim().to_string()
}

/// The caller's headers to forward: the last value of each, hop-by-hop ones (and `x-bip-*` when asked) left out.
fn forwarded_headers(headers: &HeaderMap, drop_bip: bool) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in headers {
        let lower = name.as_str();
        if HOP.contains(&lower) || (drop_bip && lower.starts_with("x-bip-")) {
            continue;
        }
        out.insert(name.clone(), value.clone());
    }
    out
}

/// The request to the target streamed back as it comes (a turn can take minutes: no read timeout).
async fn forward(
    state: &AppState,
    method: &Method,
    url: &str,
    query: &QueryMap,
    body: Bytes,
    headers: HeaderMap,
) -> ApiResult<Response> {
    forward_with(&state.http, method, url, query, body, headers).await
}

/// [`forward`] with another client (a linked household's, door_tls.rs).
async fn forward_with(
    http: &reqwest::Client,
    method: &Method,
    url: &str,
    query: &QueryMap,
    body: Bytes,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let mut target = url.to_string();
    if !query.pairs.is_empty() {
        target = format!("{target}?{}", encode_query(&query.pairs));
    }
    let mut builder = http.request(method.clone(), target).headers(headers);
    if !body.is_empty() || matches!(*method, Method::POST | Method::PUT | Method::PATCH) {
        builder = builder.body(body);
    }
    let upstream = builder.send().await.map_err(|_| HttpError::new(502, "agent unreachable"))?;
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    // httpx joins a header's values with « , ».
    let mut merged: Vec<(HeaderName, Vec<u8>)> = Vec::new();
    for (name, value) in upstream.headers() {
        let lower = name.as_str();
        if HOP.contains(&lower) || lower == "content-encoding" {
            continue;
        }
        match merged.iter_mut().find(|(n, _)| n == name) {
            Some((_, joined)) => {
                joined.extend_from_slice(b", ");
                joined.extend_from_slice(value.as_bytes());
            }
            None => merged.push((name.clone(), value.as_bytes().to_vec())),
        }
    }
    let stream = upstream.bytes_stream().map_err(std::io::Error::other);
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = status;
    for (name, value) in merged {
        if let Ok(value) = HeaderValue::from_bytes(&value) {
            response.headers_mut().insert(name, value);
        }
    }
    Ok(response)
}

/// `/peer/p/<target>/<path>`.
async fn relay(state: AppState, target: String, path: String, request: Request) -> ApiResult<Response> {
    let config = state.config();
    let token = bearer(request.headers());
    let gateway = if token.is_empty() { None } else { state.links.caller(&token) };
    let Some(gateway) = gateway else {
        return Err(HttpError::new(401, "unknown agent"));
    };
    let remote = split(&target);
    let agent = if remote.is_none() { config.agent(Some(&target)).cloned() } else { None };
    let household = remote.as_ref().and_then(|(_, h)| state.households.get(h));
    if (agent.is_none() && household.is_none()) || !ALLOWED_PATHS.iter().any(|p| path.starts_with(p)) {
        return Err(HttpError::new(404, "not found"));
    }
    let (parts, body) = request.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    let query = QueryMap::parse(parts.uri.query());
    let links = links_of(&state.links.load());
    let linked = |s: &str, t: &str| links.iter().any(|(a, b)| a == s && b == t);
    let mine: BTreeSet<String> =
        config.agents.values().filter(|a| gateway_of(&config, a) == gateway).map(|a| a.name.clone()).collect();
    if let (Some(household), Some((remote_agent, _))) = (household, remote) {
        return to_household(
            &state,
            &config,
            &gateway,
            &mine,
            &links,
            &target,
            &remote_agent,
            &household,
            &path,
            &parts.method,
            &parts.headers,
            &query,
            body,
        )
        .await;
    }
    let agent = agent.expect("a local agent");
    let (sender, allowed) = if is_message(&parts.method, &path) {
        let sender = sender_of(&config, &gateway, &body).or_else(|| {
            if mine.len() == 1 {
                mine.iter().next().cloned()
            } else {
                None
            }
        });
        let allowed = sender.as_deref().map(|s| linked(s, &agent.name)).unwrap_or(false);
        (sender, allowed)
    } else {
        // finding or opening the target's Bot Chat: some agent of this gateway may write to it
        (None, mine.iter().any(|name| linked(name, &agent.name)))
    };
    if !allowed {
        tracing::info!(sender = %sender.clone().unwrap_or_else(|| gateway.clone()), target = %agent.name, "agent message refused");
        return Err(HttpError::new(403, format!("not allowed to write to {} (ask the person)", agent.name)));
    }
    if let Some(answer) = only_bot_chat(&state, &agent, &parts.method, &path, &query, &body).await? {
        return Ok(answer);
    }
    let mut headers = forwarded_headers(&parts.headers, false);
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", agent.hermes_key)) {
        headers.insert("authorization", value);
    }
    let url = format!("{}/{path}", agent.hermes_url);
    let response = forward(&state, &parts.method, &url, &query, body, headers).await?;
    if let Some(sender) = sender {
        tracing::info!(sender = %sender, target = %agent.name, "agent message");
    }
    Ok(response)
}

/// One of our agents writes to an agent of a linked household: our link checked here, theirs on their bridge.
#[allow(clippy::too_many_arguments)]
async fn to_household(
    state: &AppState,
    config: &Config,
    gateway: &str,
    mine: &BTreeSet<String>,
    links: &[(String, String)],
    target: &str,
    agent: &str,
    household: &Dict,
    path: &str,
    method: &Method,
    request_headers: &HeaderMap,
    query: &QueryMap,
    body: Bytes,
) -> ApiResult<Response> {
    let linked = |s: &str| links.iter().any(|(a, b)| a == s && b == target);
    let (sender, allowed) = if is_message(method, path) {
        let sender =
            sender_of(config, gateway, &body)
                .or_else(|| if mine.len() == 1 { mine.iter().next().cloned() } else { None });
        let allowed = sender.as_deref().map(linked).unwrap_or(false);
        (sender, allowed)
    } else {
        let sender = mine.iter().find(|n| linked(n)).cloned();
        let allowed = sender.is_some();
        (sender, allowed)
    };
    let Some(sender) = sender.filter(|_| allowed) else {
        tracing::info!(sender = %gateway, target = %target, "household message refused");
        return Err(HttpError::new(403, format!("not allowed to write to {target} (ask the person)")));
    };
    let shown = config.agent(Some(&sender)).map(|a| a.display_name.clone()).unwrap_or_else(|| sender.clone());
    let key_out = household.get("key_out").map(crate::py::py_str).unwrap_or_default();
    let mut headers = forwarded_headers(request_headers, false);
    for (name, value) in [
        ("authorization", format!("Bearer {key_out}")),
        ("x-bip-sender", sender.clone()),
        ("x-bip-sender-name", quote(&shown)),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(name, value);
        }
    }
    tracing::info!(sender = %sender, target = %target, "household message");
    let url = format!("{}/family/p/{agent}/{path}", household.get("url").map(crate::py::py_str).unwrap_or_default());
    let http = crate::door_tls::household_client(&state.http, household);
    forward_with(&http, method, &url, query, body, headers).await
}

/// « Message from Vie (@vie): » becomes « Message from Vie (Alex) (@vie__alex): »: the agent sees who writes, from
/// which household, and the address to answer.
pub fn resign(body: &[u8], sender: &str, name: &str, household: &str) -> Vec<u8> {
    let Some(Value::Object(mut data)) = body_json(body) else { return body.to_vec() };
    for field in ["message", "input"] {
        if let Some(Value::String(text)) = data.get(field).cloned() {
            let signed = format!(
                "Message from {name} ({household}) (@{sender}{SEPARATOR}{}):",
                crate::households::slug(household)
            );
            let resigned = match SIGNATURE.find(&text) {
                Some(found) => format!("{signed}{}", &text[found.end()..]),
                None => format!("{signed} {text}"),
            };
            data.insert(field.to_string(), Value::String(resigned));
        }
    }
    crate::py::dumps(&Value::Object(data)).into_bytes()
}

async fn relay_path(
    State(state): State<AppState>,
    UrlPath((target, path)): UrlPath<(String, String)>,
    request: Request,
) -> ApiResult<Response> {
    relay(state, target, path, request).await
}

async fn relay_bare(
    State(state): State<AppState>,
    UrlPath(target): UrlPath<String>,
    request: Request,
) -> ApiResult<Response> {
    relay(state, target, String::new(), request).await
}

/// `/peer/p/<target>/…`: Hermes' peer protocol (its sessions and runs API), for the agents (their own tokens).
/// Mounted at the root.
pub fn peer_router() -> Router<AppState> {
    Router::new()
        .route("/peer/p/{target}/", get(relay_bare).post(relay_bare))
        .route("/peer/p/{target}/{*path}", get(relay_path).post(relay_path))
}

// -- between households -----------------------------------------------------------------------------------------------

/// The household whose key this request carries (401 otherwise).
fn household_caller(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let token = bearer(headers);
    let household = if token.is_empty() { None } else { state.households.caller(&token) };
    household.ok_or_else(|| HttpError::new(401, "unknown household"))
}

/// Starlette's `await request.json()`: invalid JSON is an uncaught error (500).
async fn request_json(request: Request) -> ApiResult<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    serde_json::from_slice(&bytes).map_err(|e| HttpError::from(anyhow::anyhow!("JSONDecodeError: {e}")))
}

/// `body.get(key, "")` with Python's `str()`; a body that is not a dict is an uncaught error (500).
fn body_str(body: &Value, key: &str) -> ApiResult<String> {
    match body {
        Value::Object(map) => Ok(map.get(key).map(crate::py::py_str).unwrap_or_default()),
        _ => Err(HttpError::from(anyhow::anyhow!("AttributeError: object has no attribute 'get'"))),
    }
}

/// A household linked or unlinked: the agents get the family's tools, or lose them (agent_tools.rs `tools_for`).
fn family_tools_changed(state: &AppState) {
    let config = state.config();
    if config.hermes.enabled() {
        state.spawn(crate::agent_tools::register_all(state.config.clone(), crate::agents_admin::default_run()));
    }
}

async fn family_accept(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let (code, name, url, key) =
        (body_str(&body, "code")?, body_str(&body, "name")?, body_str(&body, "url")?, body_str(&body, "key")?);
    // the certificate their address serves when it is a door (sent over ours, checked, with our one-time code)
    let fingerprint = body_str(&body, "fingerprint")?;
    let fingerprint = Some(fingerprint.as_str()).filter(|f| !f.is_empty());
    match state.households.accept(&code, &name, &url, &key, fingerprint) {
        Ok(mut answer) => {
            family_tools_changed(&state);
            if let Some(own) = crate::door_tls::door_fingerprint(&state.config()) {
                answer.insert("fingerprint".into(), json!(own));
            }
            Ok(web::ok(Value::Object(answer)))
        }
        Err(HouseholdError(message)) => Err(HttpError::new(403, message)),
    }
}

/// `POST /family/door {fingerprint}`: a linked household's bridge tells the certificate its address serves (links
/// made before invitations carried it, a certificate put back after a restore). Its key says who it is.
async fn family_door(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let household = household_caller(&state, request.headers())?;
    let body = request_json(request).await?;
    let (fingerprint, url) = (body_str(&body, "fingerprint")?, body_str(&body, "url")?);
    let url = Some(url.as_str()).filter(|u| !u.is_empty());
    match state.households.set_fingerprint(&household, &fingerprint, url) {
        Ok(changed) => {
            if changed {
                tracing::info!(household = %household, "household door certificate kept");
            }
            Ok(web::ok(json!({"ok": true})))
        }
        Err(e) if e.downcast_ref::<HouseholdError>().is_some() => {
            Err(HttpError::new(409, e.downcast_ref::<HouseholdError>().map(|h| h.0.clone()).unwrap_or_default()))
        }
        Err(_) => Err(HttpError::new(401, "unknown household")),
    }
}

async fn family_agents(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    household_caller(&state, &headers)?;
    let config = state.config();
    let agents: Vec<Value> = config.agents.values().map(|a| json!({"id": a.name, "name": a.display_name})).collect();
    Ok(web::ok(json!({"name": state.households.name(), "agents": agents})))
}

/// `/family/p/<target>/<path>`: a linked household's agent writes to one of ours.
async fn deliver(state: AppState, target: String, path: String, request: Request) -> ApiResult<Response> {
    let household = household_caller(&state, request.headers())?;
    let config = state.config();
    let Some(agent) =
        config.agent(Some(&target)).cloned().filter(|_| ALLOWED_PATHS.iter().any(|p| path.starts_with(p)))
    else {
        return Err(HttpError::new(404, "not found"));
    };
    let (parts, body) = request.into_parts();
    let sender = parts
        .headers
        .get("x-bip-sender")
        .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
        .unwrap_or_default();
    let remote = format!("{sender}{SEPARATOR}{household}");
    let links = links_of(&state.links.load());
    let allowed = if is_message(&parts.method, &path) {
        !sender.is_empty() && links.iter().any(|(s, t)| *s == remote && *t == agent.name)
    } else {
        links.iter().any(|(s, t)| s.ends_with(&format!("{SEPARATOR}{household}")) && *t == agent.name)
    };
    if !allowed {
        tracing::info!(sender = %remote, target = %agent.name, "household message refused");
        return Err(HttpError::new(403, format!("{} does not take messages from {remote}", agent.display_name)));
    }
    let mut body = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    let query = QueryMap::parse(parts.uri.query());
    if let Some(answer) = only_bot_chat(&state, &agent, &parts.method, &path, &query, &body).await? {
        return Ok(answer);
    }
    if is_message(&parts.method, &path) {
        let given = parts
            .headers
            .get("x-bip-sender-name")
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .unwrap_or_default();
        let name = Some(unquote(&given)).filter(|n| !n.is_empty()).unwrap_or_else(|| sender.clone());
        let household_name =
            state.households.get(&household).and_then(|h| h.get("name").map(crate::py::py_str)).unwrap_or_default();
        body = Bytes::from(resign(&body, &sender, &name, &household_name));
    }
    let mut headers = forwarded_headers(&parts.headers, true);
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", agent.hermes_key)) {
        headers.insert("authorization", value);
    }
    tracing::info!(sender = %remote, target = %agent.name, "household message");
    let url = format!("{}/{path}", agent.hermes_url);
    forward(&state, &parts.method, &url, &query, body, headers).await
}

async fn deliver_path(
    State(state): State<AppState>,
    UrlPath((target, path)): UrlPath<(String, String)>,
    request: Request,
) -> ApiResult<Response> {
    deliver(state, target, path, request).await
}

async fn deliver_bare(
    State(state): State<AppState>,
    UrlPath(target): UrlPath<String>,
    request: Request,
) -> ApiResult<Response> {
    deliver(state, target, String::new(), request).await
}

/// What a linked household's bridge may call (the key each was given): introduce itself with an invitation code,
/// list our agents, and hand a message over along a link this person turned on. Mounted at the root.
pub fn family_router() -> Router<AppState> {
    Router::new()
        .route("/family/accept", post(family_accept))
        .route("/family/agents", get(family_agents))
        .route("/family/door", post(family_door))
        .route("/family/p/{target}/", get(deliver_bare).post(deliver_bare))
        .route("/family/p/{target}/{*path}", get(deliver_path).post(deliver_path))
}

// -- the app's side -----------------------------------------------------------------------------------------------------

async fn family(State(state): State<AppState>) -> Response {
    let config = state.config();
    web::ok(json!({"name": state.households.name(), "url": own_url(&config), "households": state.households.public()}))
}

async fn set_name(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let name = body_str(&body, "name")?;
    if let Err(exc) = state.households.set_name(&name) {
        return match exc.downcast_ref::<HouseholdError>() {
            Some(e) => Err(HttpError::new(400, e.0.clone())),
            None => Err(exc.into()),
        };
    }
    Ok(web::ok(json!({"name": state.households.name()})))
}

/// `{days?}`: an invitation sent through the person's account (« Inviter par son compte ») waits longer than a QR
/// code shown in the same room (ten minutes): up to a week.
async fn invite(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let bytes = axum::body::to_bytes(request.into_body(), 64 * 1024)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    let days = serde_json::from_slice::<Value>(&bytes).ok().and_then(|b| b.get("days").and_then(Value::as_f64));
    let seconds = days.map(|d| d.clamp(0.0, 7.0) * 86400.0).filter(|s| *s > 0.0);
    let config = state.config();
    let Some(url) = own_url(&config).filter(|u| !u.is_empty()) else {
        return Err(HttpError::new(409, "this bridge has no address on the tailnet (public_url)"));
    };
    let code = match state.households.invite_for(seconds.unwrap_or(crate::households::INVITE_SECONDS)) {
        Ok(code) => code,
        Err(exc) => {
            return match exc.downcast_ref::<HouseholdError>() {
                Some(e) => Err(HttpError::new(409, e.0.clone())),
                None => Err(exc.into()),
            }
        }
    };
    let mut invitation = json!({"v": 1, "kind": "family", "url": url, "code": code, "name": state.households.name()});
    // our address is a door with a self-signed certificate: the other bridge pins it, as the apps do
    if let Some(fingerprint) = crate::door_tls::door_fingerprint(&config) {
        invitation["fingerprint"] = json!(fingerprint);
    }
    Ok(web::ok(invitation))
}

/// The other household's invitation, scanned in the app: this bridge introduces itself to theirs.
async fn join(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let url = body_str(&body, "url")?.trim_end_matches('/').to_string();
    let code = body_str(&body, "code")?;
    let config = state.config();
    let mine = own_url(&config).filter(|u| !u.is_empty());
    let (Some(mine), false) = (mine, state.households.name().is_empty()) else {
        return Err(HttpError::new(409, "name this household and give the bridge a tailnet address first"));
    };
    // the inviter's certificate when its address is a door (in its invitation, as the apps get it at pairing)
    let given = body_str(&body, "fingerprint")?;
    let fingerprint = match given.trim() {
        "" => None,
        given => Some(
            crate::door_tls::normalize(given).ok_or_else(|| HttpError::new(400, "this invitation is damaged"))?,
        ),
    };
    if fingerprint.is_none() && crate::door_tls::door_address(&url) {
        // a door's address without its certificate: an older app, or an invitation from an older bridge
        return Err(HttpError::new(409, "this invitation is too old: ask for a new one (and update the app)"));
    }
    let key = crate::py::token_urlsafe(32);
    let mut payload = json!({"code": code, "name": state.households.name(), "url": mine, "key": key});
    if let Some(own) = crate::door_tls::door_fingerprint(&config) {
        payload["fingerprint"] = json!(own);
    }
    let resp = crate::door_tls::client_for(&state.http, fingerprint.as_deref())
        .post(format!("{url}/family/accept"))
        .timeout(std::time::Duration::from_secs(15))
        .json(&payload)
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their bridge does not answer"))?;
    if resp.status().as_u16() != 200 {
        return Err(HttpError::new(403, "their bridge refused the invitation (expired?)"));
    }
    let bytes = resp.bytes().await.map_err(|_| HttpError::new(502, "their bridge does not answer"))?;
    let answer: Value =
        serde_json::from_slice(&bytes).map_err(|e| HttpError::from(anyhow::anyhow!("JSONDecodeError: {e}")))?;
    let name = body_str(&answer, "name")?;
    let key_in = body_str(&answer, "key")?;
    let household = match state.households.joined(&name, &url, &key, &key_in, fingerprint.as_deref()) {
        Ok(h) => {
            family_tools_changed(&state);
            h
        }
        Err(exc) => {
            return match exc.downcast_ref::<HouseholdError>() {
                Some(e) => Err(HttpError::new(409, e.0.clone())),
                None => Err(exc.into()),
            }
        }
    };
    Ok(web::ok(json!({"id": household, "name": answer.get("name").cloned().unwrap_or(Value::Null)})))
}

async fn unlink(state: &AppState, household: &str) -> ApiResult<Response> {
    if state.households.remove(household).is_err() {
        return Err(HttpError::new(404, "no such household"));
    }
    let mut data = state.links.load();
    let suffix = format!("{SEPARATOR}{household}");
    let kept: Vec<(String, String)> =
        links_of(&data).into_iter().filter(|(s, t)| !s.ends_with(&suffix) && !t.ends_with(&suffix)).collect();
    data.insert("links".into(), pairs_value(&kept));
    state.links.save(&data)?;
    family_tools_changed(state);
    Ok(web::no_content())
}

async fn unlink_route(State(state): State<AppState>, UrlPath(household): UrlPath<String>) -> ApiResult<Response> {
    unlink(&state, &household).await
}

/// Starlette tries the routes in order: `DELETE /v1/family/name` (a path with another method) falls through to
/// `DELETE /v1/family/{household}`. Other methods: 405 with this path's own methods.
async fn or_unlink(state: AppState, request: Request, literal: &str, allow: &'static str) -> Response {
    if request.method() == Method::DELETE {
        return match unlink(&state, literal).await {
            Ok(r) => r,
            Err(e) => axum::response::IntoResponse::into_response(e),
        };
    }
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    response.headers_mut().insert("allow", HeaderValue::from_static(allow));
    response
}

async fn name_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "name", "PUT").await
}

async fn invite_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "invite", "POST").await
}

async fn join_fallback(State(state): State<AppState>, request: Request) -> Response {
    or_unlink(state, request, "join", "POST").await
}

/// For the app: this household's name, its invitations, joining another one, the households linked (mounted under
/// /v1, bridge key).
pub fn household_admin_router() -> Router<AppState> {
    Router::new()
        .route("/family", get(family))
        .route("/family/name", put(set_name).fallback(name_fallback))
        .route("/family/invite", post(invite).fallback(invite_fallback))
        .route("/family/join", post(join).fallback(join_fallback))
        .route("/family/{household}", delete(unlink_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resigning_and_quoting() {
        let body = br#"{"session_id": "bot-1", "message": "Message from Vie (@vie): bonjour"}"#;
        let out: Value = serde_json::from_slice(&resign(body, "vie", "Vie", "Alex")).unwrap();
        assert_eq!(out["message"], "Message from Vie (Alex) (@vie__alex): bonjour");
        let out: Value = serde_json::from_slice(&resign(br#"{"input": "salut"}"#, "vie", "Vie", "Léa")).unwrap();
        assert_eq!(out["input"], "Message from Vie (Léa) (@vie__lea): salut");
        assert_eq!(quote("Vie Ö/x"), "Vie%20%C3%96/x");
        assert_eq!(
            encode_query(&[("title".into(), "Bot Chat".into()), ("x".into(), "a~b*c/d".into())]),
            "title=Bot+Chat&x=a~b%2Ac%2Fd"
        );
    }
}
