//! « Se connecter avec… »: services with their own remote MCP server and their own sign-in (OAuth 2.1), connected
//! from the app without a key to paste (decided 2026-10-25). Notion first, then Finary (read only). The catalog is
//! `assets/connections.yaml`; adding a service with standard MCP authorization is one entry there.
//!
//! The person taps « Connecter » in ⊞+, the app asks the bridge for a sign-in address, opens it in an in-app web auth
//! session (ASWebAuthenticationSession on iOS, a Custom Tab on Android), the person signs in at the service and
//! chooses what to allow, the service sends the browser back to `bipagents://oauth/callback?code=…&state=…`, and the
//! app hands that address to the bridge, which finishes the exchange.
//!
//! App routes (the bridge key):
//!
//!     GET    /v1/connections
//!            → {connections: [{service, label, icon, read_only, texts, connected, state, account, agents,
//!                              connected_at}]}
//!            state: "not_connected" | "connected" | "expired" (the service refused a renewal: connect again)
//!     POST   /v1/connections/{service}/start   {agents: ["bip", …], return_to?: "app" | "bridge"}
//!            → {authorize_url, state, redirect_uri, expires_in}     (open authorize_url; callback scheme "bipagents")
//!            409 {detail: {code: "cannot_hold", agents: {name: reason}}}  an agent without its own protected space
//!            409 {detail: {code: "name_taken", agents: [names]}}          the agent already has a server of that name
//!            502 {detail: {code: "discovery_failed" | "registration_failed", message}}
//!     POST   /v1/connections/callback          {url: "bipagents://oauth/callback?code=…&state=…"}
//!            → the connection, as in the listing
//!            400 {detail: {code: "denied" | "unknown_state" | "wrong_issuer" | "bad_callback"}}
//!            502 {detail: {code: "exchange_failed", message}}
//!     PUT    /v1/connections/{service}/agents  {agents: [...]}   which agents use it (no new sign-in; [] = none)
//!     DELETE /v1/connections/{service}         → {ok: true, revoked: bool}  revoked at the service where it allows it
//!
//! And without the key, for `return_to: "bridge"`:
//!
//!     GET    /oauth/callback?code=…&state=…    finishes the same way, then sends the browser to
//!                                              `bipagents://oauth/done?service=…&status=connected|error[&code=…]`
//!
//! The redirect: `bipagents://oauth/callback` by default. It never touches the network, so it works the same for a
//! bridge reached through the relay, Tailscale only, or the LAN; the services refuse plain-http redirects that
//! aren't loopback (a Tailscale `http://100.x…` address), and an https one needs a domain and certificate the
//! install may not have. PKCE keeps it safe: an app that caught the code can't use it without the verifier, which
//! never leaves the bridge. `return_to: "bridge"` (the bridge's `public_url`, or `[connections] callback_url`) is
//! there for the web console and installs with a public https address.
//!
//! How it works: discovery as the MCP authorization spec says (protected resource metadata, RFC 9728, then the
//! authorization server's metadata, RFC 8414, whose issuer must be the catalog's), dynamic client registration
//! (RFC 7591, one client per service and redirect, kept), authorization code with PKCE S256 and the `resource`
//! parameter (RFC 8707), the `iss` of the answer checked (RFC 9207).
//!
//! The keys: the bridge keeps the refresh token, encrypted like the mailboxes (`data_dir/connections/
//! connections.json.enc` + `connections.key`, 0600), and renews the access token itself before it ends (every
//! `[connections] refresh_seconds`, 60 by default). Each chosen agent gets only the short-lived access token, in
//! Hermes' own OAuth files of its profile (`mcp-tokens/<name>.json` and `.client.json`, 0600, no refresh token), and
//! an `mcp_servers.<name>` entry (`url`, `auth: oauth`, `trust: untrusted`) written with Hermes' CLI as services.rs
//! does. Hermes reloads those files when their mtime changes (`mcp_oauth_manager.invalidate_if_disk_changed`), so a
//! renewal reaches a running agent without a restart, and since Hermes never holds a refresh token, two agents can't
//! burn each other's (single-use refresh tokens). No Hermes patch is needed. The app never sees a token, nor does the
//! model: they're not in config.yaml, not on a command line, not in any answer.
//!
//! Safety: `trust: untrusted` makes Hermes ask the person before each tool without `readOnlyHint` (writes, as for
//! the other services), the souls' CONTENT_RULE keeps what a page says as data, and an agent needs its own protected
//! space (vault::holder, as the services with a token) to hold the keys.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::header::HOST;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Router;
use base64::Engine;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Digest;

use crate::agents_admin::{default_run, Run};
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::py;
use crate::web::{self, check_max_items, ApiResult, FieldError, HttpError, Json, Validate};

/// The app catches this scheme (ASWebAuthenticationSession `callbackURLScheme`, Android's intent filter).
pub const APP_REDIRECT: &str = "bipagents://oauth/callback";
pub const APP_DONE: &str = "bipagents://oauth/done";
/// How long a started sign-in may wait for its callback.
pub const FLOW_SECONDS: u64 = 900;
pub const FLOWS_MAX: usize = 32;
/// The access token is renewed when it has less than this left.
pub const RENEW_MARGIN: f64 = 300.0;
pub const AGENTS_MAX: usize = 32;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const CLIENT_NAME: &str = "BipAgents";
pub const CATALOG_YAML: &str = include_str!("../assets/connections.yaml");

// -- settings and catalog ----------------------------------------------------------------------------------------------

/// `[connections]` of bridge.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// service → its MCP address instead of the catalog's (tests, a proxy); the issuer check is then the metadata's own.
    pub urls: IndexMap<String, String>,
    /// How often the renewals are looked at.
    pub refresh_seconds: f64,
    /// The base of `/oauth/callback` for `return_to: "bridge"` (else the bridge's `public_url`, else the request's host).
    pub callback_url: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { urls: IndexMap::new(), refresh_seconds: 60.0, callback_url: None }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let mut out = Settings::default();
        if let Some(Value::Object(urls)) = table.get("urls") {
            for (k, v) in urls {
                if let Some(v) = v.as_str() {
                    out.urls.insert(k.clone(), v.trim_end_matches('/').to_string());
                }
            }
        }
        if let Some(s) = table.get("refresh_seconds").and_then(Value::as_f64) {
            out.refresh_seconds = s.max(1.0);
        }
        out.callback_url = table
            .get("callback_url")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').into());
        out
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Texts {
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub privacy: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Service {
    pub id: String,
    pub url: String,
    pub issuer: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub icon: String,
    pub texts: IndexMap<String, Texts>,
}

static CATALOG: LazyLock<Vec<Service>> = LazyLock::new(|| serde_yaml::from_str(CATALOG_YAML).unwrap_or_default());

pub fn catalog() -> &'static [Service] {
    &CATALOG
}

pub fn service(id: &str) -> Option<&'static Service> {
    CATALOG.iter().find(|s| s.id == id)
}

impl Service {
    /// The `mcp_servers` name in the agents' profiles (the id: lowercase, no spaces).
    pub fn server_name(&self) -> &str {
        &self.id
    }

    fn label(&self) -> String {
        self.texts.get("fr").or_else(|| self.texts.values().next()).map(|t| t.label.clone()).unwrap_or(self.id.clone())
    }

    /// What the app's catalog shows (also `connections` of `/v1/tool-catalog`).
    pub fn public(&self) -> Value {
        let fr = self.texts.get("fr").or_else(|| self.texts.values().next());
        json!({
            "id": self.id,
            "label": self.label(),
            "description": fr.map(|t| t.description.clone()).unwrap_or_default(),
            "icon": self.icon,
            "read_only": self.read_only,
            "texts": self.texts,
        })
    }
}

/// The address to reach, and whether it was given in bridge.toml (then http and any issuer are accepted).
fn mcp_url(config: &Config, svc: &Service) -> (String, bool) {
    match config.connections.urls.get(&svc.id) {
        Some(u) => (u.clone(), true),
        None => (svc.url.clone(), false),
    }
}

// -- what is kept ------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Client {
    pub client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// none | client_secret_post | client_secret_basic
    pub auth_method: String,
    pub redirect_uri: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Server {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub userinfo_endpoint: Option<String>,
    /// The protected resource (RFC 8707 `resource`).
    pub resource: String,
    pub mcp_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// Unix time; None: the service gave no lifetime.
    #[serde(default)]
    pub expires_at: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Connection {
    pub service: String,
    pub client: Client,
    pub server: Server,
    pub tokens: Tokens,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub agents: Vec<String>,
    pub connected_at: f64,
    #[serde(default)]
    pub refreshed_at: Option<f64>,
    /// connected | expired
    pub state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Stored {
    #[serde(default)]
    pub connections: BTreeMap<String, Connection>,
    /// Registered clients, by `service redirect_uri issuer`: a new sign-in doesn't register again.
    #[serde(default)]
    pub clients: BTreeMap<String, Client>,
}

/// The encrypted file (Fernet, as mailbox.rs) and its key.
pub struct Store {
    folder: PathBuf,
}

impl Store {
    pub fn new(folder: &Path) -> Store {
        Store { folder: folder.to_path_buf() }
    }

    fn fernet(&self) -> anyhow::Result<fernet::Fernet> {
        py::makedirs(&self.folder.to_string_lossy(), 0o700)?;
        let key_path = self.folder.join("connections.key");
        if !key_path.exists() {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut fh = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&key_path)?;
            fh.write_all(fernet::Fernet::generate_key().as_bytes())?;
        }
        let key = std::fs::read_to_string(&key_path)?;
        fernet::Fernet::new(key.trim()).ok_or_else(|| anyhow::anyhow!("the connections' key is not a Fernet key"))
    }

    pub fn read(&self) -> anyhow::Result<Stored> {
        let path = self.folder.join("connections.json.enc");
        if !path.exists() {
            return Ok(Stored::default());
        }
        let token = std::fs::read_to_string(&path)?;
        let plain = self
            .fernet()?
            .decrypt(token.trim())
            .map_err(|_| anyhow::anyhow!("the connections can't be decrypted with their key"))?;
        Ok(serde_json::from_slice(&plain)?)
    }

    pub fn write(&self, stored: &Stored) -> anyhow::Result<()> {
        let blob = self.fernet()?.encrypt(&serde_json::to_vec(stored)?);
        let path = self.folder.join("connections.json.enc");
        let tmp = self.folder.join(format!(".connections.json.enc.{}.tmp", std::process::id()));
        py::write_with_mode(&tmp.to_string_lossy(), blob.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

pub fn store_of(config: &Config) -> Store {
    Store::new(&PathBuf::from(config.data_dir()).join("connections"))
}

/// One change at a time (the routes and the renewals), across the HTTP calls they make.
static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

// -- sign-ins on their way ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Flow {
    service: String,
    verifier: String,
    agents: Vec<String>,
    client: Client,
    server: Server,
    to_bridge: bool,
    started: std::time::Instant,
}

static FLOWS: LazyLock<std::sync::Mutex<HashMap<String, Flow>>> = LazyLock::new(Default::default);

fn keep_flow(state: &str, flow: Flow) {
    let mut flows = FLOWS.lock().unwrap_or_else(|e| e.into_inner());
    flows.retain(|_, f| f.started.elapsed().as_secs() < FLOW_SECONDS);
    if flows.len() >= FLOWS_MAX {
        if let Some(oldest) = flows.iter().min_by_key(|(_, f)| f.started).map(|(k, _)| k.clone()) {
            flows.remove(&oldest);
        }
    }
    flows.insert(state.to_string(), flow);
}

/// The flow of this `state`, once (a second callback with it finds nothing).
fn take_flow(state: &str) -> Option<Flow> {
    let mut flows = FLOWS.lock().unwrap_or_else(|e| e.into_inner());
    flows.remove(state).filter(|f| f.started.elapsed().as_secs() < FLOW_SECONDS)
}

// -- OAuth -------------------------------------------------------------------------------------------------------------

/// A failure the app can tell apart (`code`), with a short message for the logs (never a token).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ConnectError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

fn fail(status: u16, code: &'static str, message: impl Into<String>) -> ConnectError {
    ConnectError { status, code, message: message.into() }
}

impl From<ConnectError> for HttpError {
    fn from(e: ConnectError) -> Self {
        HttpError::json(e.status, json!({"code": e.code, "message": e.message}))
    }
}

pub fn pkce_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// RFC 9728 / RFC 8414 well-known addresses: the suffix goes between the host and the path.
pub fn well_known(base: &str, suffix: &str) -> Vec<String> {
    let Ok(url) = url::Url::parse(base) else { return Vec::new() };
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_end_matches('/');
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(format!("{origin}/.well-known/{suffix}{path}"));
    }
    out.push(format!("{origin}/.well-known/{suffix}"));
    out
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("BipAgents-bridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

async fn get_json(client: &reqwest::Client, url: &str) -> Option<serde_json::Map<String, Value>> {
    let resp = client.get(url).header("accept", "application/json").send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    match resp.json::<Value>().await.ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn s(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string)
}

fn acceptable(url: &str, allow_http: bool) -> bool {
    match url::Url::parse(url) {
        Ok(u) => u.scheme() == "https" || (allow_http && u.scheme() == "http"),
        Err(_) => false,
    }
}

/// Where the service signs people in, found from its MCP address.
pub async fn discover(
    client: &reqwest::Client,
    svc: &Service,
    url: &str,
    overridden: bool,
) -> Result<Server, ConnectError> {
    let mut resource_meta = None;
    for candidate in well_known(url, "oauth-protected-resource") {
        if let Some(m) = get_json(client, &candidate).await {
            resource_meta = Some(m);
            break;
        }
    }
    let resource_meta = resource_meta.ok_or_else(|| fail(502, "discovery_failed", "no protected resource metadata"))?;
    let issuer = resource_meta
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .map(|s| s.trim_end_matches('/').to_string())
        .ok_or_else(|| fail(502, "discovery_failed", "no authorization server"))?;
    if !overridden && issuer != svc.issuer.trim_end_matches('/') {
        return Err(fail(502, "discovery_failed", format!("unexpected authorization server {issuer}")));
    }
    let mut meta = None;
    for suffix in ["oauth-authorization-server", "openid-configuration"] {
        for candidate in well_known(&issuer, suffix) {
            if let Some(m) = get_json(client, &candidate).await {
                meta = Some(m);
                break;
            }
        }
        if meta.is_some() {
            break;
        }
    }
    let meta = meta.ok_or_else(|| fail(502, "discovery_failed", "no authorization server metadata"))?;
    if s(&meta, "issuer").map(|i| i.trim_end_matches('/').to_string()) != Some(issuer.clone()) {
        return Err(fail(502, "discovery_failed", "the metadata names another issuer"));
    }
    if let Some(methods) = meta.get("code_challenge_methods_supported").and_then(Value::as_array) {
        if !methods.iter().any(|m| m == "S256") {
            return Err(fail(502, "discovery_failed", "no PKCE S256"));
        }
    }
    let mut userinfo = s(&meta, "userinfo_endpoint");
    if userinfo.is_none() && svc.scope.split_whitespace().any(|w| w == "openid") {
        for candidate in well_known(&issuer, "openid-configuration") {
            if let Some(m) = get_json(client, &candidate).await {
                userinfo = s(&m, "userinfo_endpoint");
                break;
            }
        }
    }
    let server = Server {
        issuer,
        authorization_endpoint: s(&meta, "authorization_endpoint").unwrap_or_default(),
        token_endpoint: s(&meta, "token_endpoint").unwrap_or_default(),
        registration_endpoint: s(&meta, "registration_endpoint"),
        revocation_endpoint: s(&meta, "revocation_endpoint"),
        userinfo_endpoint: userinfo,
        resource: s(&resource_meta, "resource").unwrap_or_else(|| url.to_string()),
        mcp_url: url.to_string(),
    };
    let endpoints = [Some(&server.authorization_endpoint), Some(&server.token_endpoint)];
    if endpoints.iter().flatten().any(|e| !acceptable(e, overridden))
        || [&server.registration_endpoint, &server.revocation_endpoint, &server.userinfo_endpoint]
            .iter()
            .any(|e| e.as_ref().is_some_and(|e| !acceptable(e, overridden)))
    {
        return Err(fail(502, "discovery_failed", "an endpoint is not https"));
    }
    Ok(server)
}

/// RFC 7591: a public client of this redirect (a secret is kept if the service gives one).
pub async fn register(
    client: &reqwest::Client,
    server: &Server,
    redirect_uri: &str,
    scope: &str,
) -> Result<Client, ConnectError> {
    let endpoint = server
        .registration_endpoint
        .as_ref()
        .ok_or_else(|| fail(502, "registration_failed", "the service doesn't register clients"))?;
    let mut body = json!({
        "client_name": CLIENT_NAME,
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "application_type": "native",
    });
    if !scope.is_empty() {
        body["scope"] = json!(scope);
    }
    let resp = client
        .post(endpoint)
        .json(&body)
        .send()
        .await
        .map_err(|e| fail(502, "registration_failed", format!("unreachable ({})", e.without_url())))?;
    let status = resp.status();
    let answer: Value = resp.json().await.unwrap_or(Value::Null);
    let Some(map) = answer.as_object().filter(|_| status.is_success()) else {
        let why = answer.get("error_description").or(answer.get("error")).and_then(Value::as_str).unwrap_or("");
        return Err(fail(
            502,
            "registration_failed",
            format!("HTTP {} {}", status.as_u16(), crate::tools_knowledge::head(why, 200)),
        ));
    };
    let client_id = s(map, "client_id").ok_or_else(|| fail(502, "registration_failed", "no client_id"))?;
    let client_secret = s(map, "client_secret");
    let auth_method = s(map, "token_endpoint_auth_method")
        .unwrap_or_else(|| if client_secret.is_some() { "client_secret_post" } else { "none" }.to_string());
    Ok(Client { client_id, client_secret, auth_method, redirect_uri: redirect_uri.to_string() })
}

pub fn authorize_url(server: &Server, client: &Client, state: &str, challenge: &str, scope: &str) -> String {
    let mut url = match url::Url::parse(&server.authorization_endpoint) {
        Ok(u) => u,
        Err(_) => return String::new(),
    };
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code")
            .append_pair("client_id", &client.client_id)
            .append_pair("redirect_uri", &client.redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", &server.resource);
        if !scope.is_empty() {
            q.append_pair("scope", scope);
            if scope.split_whitespace().any(|w| w == "offline_access") {
                q.append_pair("prompt", "consent"); // OpenID servers give a refresh token only with consent asked
            }
        }
    }
    url.to_string()
}

/// The token endpoint's form, with the client's authentication.
async fn token_request(
    http: &reqwest::Client,
    server: &Server,
    client: &Client,
    mut form: Vec<(&str, String)>,
) -> Result<serde_json::Map<String, Value>, (u16, String)> {
    let mut request = http.post(&server.token_endpoint).header("accept", "application/json");
    match (client.auth_method.as_str(), &client.client_secret) {
        ("client_secret_basic", Some(secret)) => request = request.basic_auth(&client.client_id, Some(secret)),
        (_, Some(secret)) => {
            form.push(("client_id", client.client_id.clone()));
            form.push(("client_secret", secret.clone()));
        }
        (_, None) => form.push(("client_id", client.client_id.clone())),
    }
    let resp = request.form(&form).send().await.map_err(|e| (0, format!("unreachable ({})", e.without_url())))?;
    let status = resp.status().as_u16();
    let answer: Value = resp.json().await.unwrap_or(Value::Null);
    match answer {
        Value::Object(map) if (200..300).contains(&status) && s(&map, "access_token").is_some() => Ok(map),
        other => {
            let why = other.get("error").and_then(Value::as_str).unwrap_or("no access token").to_string();
            Err((status, why))
        }
    }
}

fn tokens_of(answer: &serde_json::Map<String, Value>, previous: Option<&Tokens>) -> Tokens {
    let expires_in = answer.get("expires_in").and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()));
    Tokens {
        access_token: s(answer, "access_token").unwrap_or_default(),
        // a renewal without a new refresh token keeps the old one (RFC 6749 §6)
        refresh_token: s(answer, "refresh_token").or_else(|| previous.and_then(|p| p.refresh_token.clone())),
        scope: s(answer, "scope").or_else(|| previous.and_then(|p| p.scope.clone())),
        expires_at: expires_in.map(|e| py::time_time() + e),
    }
}

/// The account's name for the app: the token answer's, an ID token's claims, or the userinfo endpoint's.
pub fn account_of(answer: &serde_json::Map<String, Value>) -> Option<String> {
    for key in ["workspace_name", "email", "name"] {
        if let Some(v) = s(answer, key) {
            return Some(v);
        }
    }
    let id_token = s(answer, "id_token")?;
    let payload = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: serde_json::Map<String, Value> = serde_json::from_slice(&bytes).ok()?;
    s(&claims, "email").or_else(|| s(&claims, "name")).or_else(|| s(&claims, "preferred_username"))
}

async fn userinfo(http: &reqwest::Client, server: &Server, access_token: &str) -> Option<String> {
    let endpoint = server.userinfo_endpoint.as_ref()?;
    let resp = http.get(endpoint).bearer_auth(access_token).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let map: serde_json::Map<String, Value> = resp.json().await.ok()?;
    s(&map, "email").or_else(|| s(&map, "name"))
}

/// The query of a callback address (`bipagents://oauth/callback?…` or `…/oauth/callback?…`).
pub fn callback_query(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or(url);
    let query = query.split('#').next().unwrap_or("");
    url::form_urlencoded::parse(query.as_bytes()).into_owned().collect()
}

// -- the agents' profiles ----------------------------------------------------------------------------------------------

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn hermes_env(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([("HERMES_HOME".to_string(), home.to_string_lossy().into_owned())])
}

fn tokens_dir(config: &Config, agent: &AgentConfig) -> PathBuf {
    crate::services::profile_home(config, agent).join("profiles").join(&agent.name).join("mcp-tokens")
}

/// Hermes' own OAuth files (tools/mcp_oauth.py `HermesTokenStorage`): the access token only, never the refresh one.
pub fn write_token_files(dir: &Path, name: &str, conn: &Connection) -> std::io::Result<()> {
    py::makedirs(&dir.to_string_lossy(), 0o700)?;
    py::chmod(&dir.to_string_lossy(), 0o700)?;
    let now = py::time_time();
    let mut tokens = json!({"access_token": conn.tokens.access_token, "token_type": "Bearer"});
    if let Some(at) = conn.tokens.expires_at {
        tokens["expires_in"] = json!(((at - now).max(0.0)) as i64);
        tokens["expires_at"] = json!(at);
    }
    if let Some(scope) = &conn.tokens.scope {
        tokens["scope"] = json!(scope);
    }
    let client = json!({
        "client_id": conn.client.client_id,
        "client_name": CLIENT_NAME,
        "redirect_uris": [conn.client.redirect_uri],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": conn.client.auth_method,
    });
    for (suffix, value) in [(".client.json", client), (".json", tokens)] {
        let path = dir.join(format!("{name}{suffix}"));
        let tmp = dir.join(format!(".{name}{suffix}.{}.tmp", std::process::id()));
        py::write_with_mode(&tmp.to_string_lossy(), serde_json::to_string_pretty(&value)?.as_bytes(), 0o600)?;
        std::fs::rename(&tmp, &path)?;
    }
    let _ = std::fs::remove_file(dir.join(format!("{name}.meta.json"))); // Hermes finds it again
    Ok(())
}

pub fn remove_token_files(dir: &Path, name: &str) {
    for suffix in [".json", ".client.json", ".meta.json", ".cimd-off", ".json.refresh.lock"] {
        let _ = std::fs::remove_file(dir.join(format!("{name}{suffix}")));
    }
}

async fn wire(config: &Config, agent: &AgentConfig, conn: &Connection, run: &Run) -> anyhow::Result<()> {
    let name = conn.service.as_str();
    write_token_files(&tokens_dir(config, agent), name, conn)?;
    let env = hermes_env(&crate::services::profile_home(config, agent));
    let hermes = config.hermes.bin.as_str();
    for (key, value) in [("url", conn.server.mcp_url.as_str()), ("auth", "oauth"), ("trust", "untrusted")] {
        run(
            args(&[hermes, "-p", &agent.name, "config", "set", &format!("mcp_servers.{name}.{key}"), value]),
            env.clone(),
        )
        .await?;
    }
    tracing::info!(agent = %agent.name, service = %name, "connections: wired");
    Ok(())
}

async fn unwire(config: &Config, agent: &AgentConfig, name: &str, run: &Run) -> anyhow::Result<()> {
    let env = hermes_env(&crate::services::profile_home(config, agent));
    run(args(&[&config.hermes.bin, "-p", &agent.name, "config", "unset", &format!("mcp_servers.{name}")]), env).await?;
    remove_token_files(&tokens_dir(config, agent), name);
    tracing::info!(agent = %agent.name, service = %name, "connections: unwired");
    Ok(())
}

/// The agents asked for, each one this install manages and able to hold the keys; none with a server of that name
/// that isn't this connection's (one at the service's own address is: left over, or set up by hand the same way).
fn agents_of(config: &Config, given: &[String], svc: &Service, ours: &[String]) -> ApiResult<Vec<AgentConfig>> {
    let name = svc.server_name();
    let (our_url, _) = mcp_url(config, svc);
    let mut out: Vec<AgentConfig> = Vec::new();
    let mut cannot: IndexMap<String, String> = IndexMap::new();
    let mut taken: Vec<String> = Vec::new();
    for id in given {
        let agent = crate::agents_admin::managed(config, id)
            .map_err(|_| HttpError::new(404, format!("not an agent this install manages: {id}")))?;
        if out.iter().any(|a| a.name == agent.name) {
            continue;
        }
        if let Some(why) = crate::vault::holder(config, &agent) {
            cannot.insert(agent.name.clone(), why);
        }
        if !ours.contains(&agent.name)
            && crate::services::listing(config, &agent).iter().any(|s| {
                s.get("name").and_then(Value::as_str) == Some(name)
                    && s.get("url").and_then(Value::as_str) != Some(our_url.as_str())
            })
        {
            taken.push(agent.name.clone());
        }
        out.push(agent);
    }
    if !cannot.is_empty() {
        return Err(HttpError::json(409, json!({"code": "cannot_hold", "agents": cannot})));
    }
    if !taken.is_empty() {
        return Err(HttpError::json(409, json!({"code": "name_taken", "agents": taken})));
    }
    Ok(out)
}

// -- views -------------------------------------------------------------------------------------------------------------

pub fn view(svc: &Service, conn: Option<&Connection>) -> Value {
    let mut out = svc.public();
    let id = out["id"].clone();
    if let Value::Object(map) = &mut out {
        map.remove("id");
        map.shift_insert(0, "service".into(), id);
        map.insert("connected".into(), json!(conn.is_some_and(|c| c.state == "connected")));
        map.insert("state".into(), json!(conn.map(|c| c.state.as_str()).unwrap_or("not_connected")));
        map.insert("account".into(), json!(conn.and_then(|c| c.account.clone())));
        map.insert("agents".into(), json!(conn.map(|c| c.agents.clone()).unwrap_or_default()));
        map.insert("connected_at".into(), json!(conn.map(|c| c.connected_at)));
    }
    out
}

// -- the work ----------------------------------------------------------------------------------------------------------

pub struct Started {
    pub authorize_url: String,
    pub state: String,
    pub redirect_uri: String,
}

pub async fn start(
    config: &Config,
    svc: &Service,
    agent_ids: &[String],
    redirect_uri: &str,
    to_bridge: bool,
) -> ApiResult<Started> {
    let _guard = LOCK.lock().await;
    let store = store_of(config);
    let mut stored = store.read()?;
    let ours = stored.connections.get(&svc.id).map(|c| c.agents.clone()).unwrap_or_default();
    let agents = agents_of(config, agent_ids, svc, &ours)?;
    if agents.is_empty() {
        return Err(HttpError::new(422, "choose at least one agent"));
    }
    let (url, overridden) = mcp_url(config, svc);
    let http = http();
    let server = discover(&http, svc, &url, overridden).await?;
    let key = format!("{} {} {}", svc.id, redirect_uri, server.issuer);
    let client = match stored.clients.get(&key) {
        Some(c) => c.clone(),
        None => {
            let c = register(&http, &server, redirect_uri, &svc.scope).await?;
            stored.clients.insert(key, c.clone());
            store.write(&stored)?;
            c
        }
    };
    let state = py::token_urlsafe(24);
    let verifier = py::token_urlsafe(48);
    let authorize = authorize_url(&server, &client, &state, &pkce_challenge(&verifier), &svc.scope);
    keep_flow(
        &state,
        Flow {
            service: svc.id.clone(),
            verifier,
            agents: agents.iter().map(|a| a.name.clone()).collect(),
            client,
            server,
            to_bridge,
            started: std::time::Instant::now(),
        },
    );
    tracing::info!(service = %svc.id, agents = ?agent_ids, "connections: sign-in started");
    Ok(Started { authorize_url: authorize, state, redirect_uri: redirect_uri.to_string() })
}

/// The callback's query: the code exchanged, the keys kept, the agents wired. Ok: (service, whether to send the
/// browser back to the app).
pub async fn finish(
    config: &Config,
    query: &HashMap<String, String>,
    run: &Run,
) -> Result<(Value, bool), (ConnectError, bool)> {
    let state = query.get("state").cloned().unwrap_or_default();
    let flow = take_flow(&state).ok_or((fail(400, "unknown_state", "no sign-in is waiting for this answer"), false))?;
    let to_bridge = flow.to_bridge;
    let err = |e: ConnectError| (e, to_bridge);
    if let Some(error) = query.get("error") {
        let code = if error == "access_denied" { "denied" } else { "bad_callback" };
        return Err(err(fail(400, code, crate::tools_knowledge::head(error, 80))));
    }
    if let Some(iss) = query.get("iss") {
        if iss.trim_end_matches('/') != flow.server.issuer {
            return Err(err(fail(400, "wrong_issuer", "the answer comes from another server")));
        }
    }
    let code = query.get("code").filter(|c| !c.is_empty()).ok_or_else(|| err(fail(400, "bad_callback", "no code")))?;
    let http = http();
    let form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.clone()),
        ("redirect_uri", flow.client.redirect_uri.clone()),
        ("code_verifier", flow.verifier.clone()),
        ("resource", flow.server.resource.clone()),
    ];
    let answer = token_request(&http, &flow.server, &flow.client, form).await.map_err(|(status, why)| {
        err(fail(502, "exchange_failed", format!("HTTP {status} {}", crate::tools_knowledge::head(&why, 80))))
    })?;
    let tokens = tokens_of(&answer, None);
    let account = match account_of(&answer) {
        Some(a) => Some(a),
        None => userinfo(&http, &flow.server, &tokens.access_token).await,
    };
    let svc = service(&flow.service).ok_or_else(|| err(fail(400, "unknown_state", "no such service")))?;
    let _guard = LOCK.lock().await;
    let store = store_of(config);
    let mut stored = store.read().map_err(|e| err(fail(500, "store_failed", e.to_string())))?;
    let previous = stored.connections.get(&flow.service).map(|c| c.agents.clone()).unwrap_or_default();
    let conn = Connection {
        service: flow.service.clone(),
        client: flow.client.clone(),
        server: flow.server.clone(),
        tokens,
        account,
        agents: flow.agents.clone(),
        connected_at: py::time_time(),
        refreshed_at: None,
        state: "connected".into(),
    };
    stored.connections.insert(flow.service.clone(), conn.clone());
    store.write(&stored).map_err(|e| err(fail(500, "store_failed", e.to_string())))?;
    for name in previous.iter().filter(|a| !conn.agents.contains(a)) {
        if let Some(agent) = config.agent(Some(name)) {
            let _ = unwire(config, agent, &flow.service, run).await;
        }
    }
    for name in &conn.agents {
        if let Some(agent) = config.agent(Some(name)) {
            wire(config, agent, &conn, run).await.map_err(|e| err(fail(502, "wiring_failed", e.to_string())))?;
        }
    }
    tracing::info!(service = %flow.service, agents = ?conn.agents, "connections: connected");
    Ok((view(svc, Some(&conn)), to_bridge))
}

/// Renews the access tokens that end soon, and hands the new ones to the agents' files. Called by [`run_forever`].
pub async fn renew_due(config: &Config) {
    let _guard = LOCK.lock().await;
    let store = store_of(config);
    let Ok(mut stored) = store.read() else { return };
    let now = py::time_time();
    let http = http();
    let mut changed = false;
    for conn in stored.connections.values_mut() {
        let due = conn.tokens.expires_at.is_some_and(|at| at - now < RENEW_MARGIN);
        let Some(refresh) = conn.tokens.refresh_token.clone().filter(|_| due && conn.state == "connected") else {
            continue;
        };
        let form = vec![
            ("grant_type", "refresh_token".to_string()),
            ("refresh_token", refresh),
            ("resource", conn.server.resource.clone()),
        ];
        match token_request(&http, &conn.server, &conn.client, form).await {
            Ok(answer) => {
                conn.tokens = tokens_of(&answer, Some(&conn.tokens));
                conn.refreshed_at = Some(now);
                changed = true;
                for name in &conn.agents {
                    if let Some(agent) = config.agent(Some(name)) {
                        if let Err(e) = write_token_files(&tokens_dir(config, agent), &conn.service, conn) {
                            tracing::warn!(agent = %name, service = %conn.service, error = %e, "connections: token not written");
                        }
                    }
                }
                tracing::info!(service = %conn.service, "connections: renewed");
            }
            Err((status, why)) if (400..500).contains(&status) => {
                // the service refused it (revoked, or expired): the person connects again
                conn.state = "expired".into();
                changed = true;
                tracing::warn!(service = %conn.service, status, why = %why, "connections: renewal refused");
            }
            Err((status, why)) => {
                tracing::warn!(service = %conn.service, status, why = %why, "connections: renewal failed, tried again later");
            }
        }
    }
    if changed {
        if let Err(e) = store.write(&stored) {
            tracing::warn!(error = %e, "connections: not saved");
        }
    }
}

pub async fn run_forever(state: AppState) {
    loop {
        let seconds = state.config().connections.refresh_seconds;
        tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        renew_due(&state.config()).await;
    }
}

async fn revoke(conn: &Connection) -> bool {
    let Some(endpoint) = conn.server.revocation_endpoint.clone() else { return false };
    let http = http();
    let mut revoked = false;
    let tokens = [
        ("refresh_token", conn.tokens.refresh_token.clone()),
        ("access_token", Some(conn.tokens.access_token.clone())),
    ];
    for (hint, token) in tokens {
        let Some(token) = token else { continue };
        let mut form = vec![("token", token), ("token_type_hint", hint.to_string())];
        let mut request = http.post(&endpoint);
        match (conn.client.auth_method.as_str(), &conn.client.client_secret) {
            ("client_secret_basic", Some(secret)) => request = request.basic_auth(&conn.client.client_id, Some(secret)),
            (_, Some(secret)) => {
                form.push(("client_id", conn.client.client_id.clone()));
                form.push(("client_secret", secret.clone()));
            }
            (_, None) => form.push(("client_id", conn.client.client_id.clone())),
        }
        if let Ok(resp) = request.form(&form).send().await {
            revoked |= resp.status().is_success();
        }
    }
    revoked
}

// -- routes ------------------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct StartBody {
    #[serde(default)]
    pub agents: Vec<String>,
    #[serde(default)]
    pub return_to: Option<String>,
}

impl Validate for StartBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(self.agents.as_slice()), AGENTS_MAX);
    }
}

#[derive(Debug, Deserialize)]
pub struct CallbackBody {
    pub url: String,
}

impl Validate for CallbackBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        web::check_max_len(errors, "url", Some(self.url.as_str()), 8192);
    }
}

#[derive(Debug, Deserialize)]
pub struct AgentsBody {
    pub agents: Vec<String>,
}

impl Validate for AgentsBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_items(errors, "agents", Some(self.agents.as_slice()), AGENTS_MAX);
    }
}

fn service_or_404(id: &str) -> ApiResult<&'static Service> {
    service(id).ok_or_else(|| HttpError::new(404, "no such service"))
}

async fn listing(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let stored = store_of(&config).read()?;
    let mut items: Vec<Value> = catalog().iter().map(|svc| view(svc, stored.connections.get(&svc.id))).collect();
    // Link Agent Wallet (link_wallet.rs), only where it is on; the apps show it in the United States and Canada.
    items.extend(crate::link_wallet::listing_row(&config));
    Ok(web::ok(json!({"connections": items})))
}

async fn start_route(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<StartBody>,
) -> ApiResult<Response> {
    if id == crate::link_wallet::SERVICE {
        return Ok(web::ok(crate::link_wallet::start(&state.config(), &body.agents)?));
    }
    let svc = service_or_404(&id)?;
    let host = headers.get(HOST).and_then(|h| h.to_str().ok()).map(str::to_string);
    let config = state.config();
    let to_bridge = match body.return_to.as_deref() {
        None | Some("app") => false,
        Some("bridge") => true,
        Some(_) => return Err(HttpError::new(422, "return_to: app or bridge")),
    };
    let redirect = if to_bridge {
        let base = config
            .connections
            .callback_url
            .clone()
            .or_else(|| config.public_url.clone())
            .or_else(|| host.map(|h| format!("http://{h}")))
            .ok_or_else(|| HttpError::new(422, "no address for the bridge"))?;
        format!("{base}/oauth/callback")
    } else {
        APP_REDIRECT.to_string()
    };
    let started = start(&config, svc, &body.agents, &redirect, to_bridge).await?;
    Ok(web::ok(json!({
        "authorize_url": started.authorize_url,
        "state": started.state,
        "redirect_uri": started.redirect_uri,
        "expires_in": FLOW_SECONDS,
    })))
}

async fn callback_route(State(state): State<AppState>, Json(body): Json<CallbackBody>) -> ApiResult<Response> {
    let query = callback_query(&body.url);
    if query.get("state").is_some_and(|s| crate::link_wallet::owns_state(s)) {
        return Ok(web::ok(crate::link_wallet::finish(&state.config(), &query, &default_run()).await?));
    }
    match finish(&state.config(), &query, &default_run()).await {
        Ok((connection, _)) => Ok(web::ok(connection)),
        Err((e, _)) => Err(e.into()),
    }
}

/// The browser's way back with `return_to: "bridge"`: no bridge key, the single-use `state` is the proof.
async fn browser_callback(State(state): State<AppState>, Query(query): Query<HashMap<String, String>>) -> Response {
    let service = query.get("state").and_then(|s| FLOWS.lock().ok()?.get(s).map(|f| f.service.clone()));
    let result = finish(&state.config(), &query, &default_run()).await;
    let (to_bridge, done) = match &result {
        Ok((_, to_bridge)) => (*to_bridge, "status=connected".to_string()),
        Err((e, to_bridge)) => (*to_bridge, format!("status=error&code={}", e.code)),
    };
    if to_bridge {
        let service = service.unwrap_or_default();
        let location = format!(
            "{APP_DONE}?service={}&{done}",
            url::form_urlencoded::byte_serialize(service.as_bytes()).collect::<String>()
        );
        return (axum::http::StatusCode::FOUND, [(axum::http::header::LOCATION, location)]).into_response();
    }
    let (status, text) = match result {
        Ok(_) => (axum::http::StatusCode::OK, "C'est connecté. Tu peux revenir dans l'app."),
        Err(_) => (axum::http::StatusCode::BAD_REQUEST, "La connexion n'a pas abouti. Recommence depuis l'app."),
    };
    let page = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>BipAgents</title>\
         <p style=\"font:17px -apple-system,sans-serif;margin:2em\">{text}</p>"
    );
    (status, [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

async fn set_agents(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<AgentsBody>,
) -> ApiResult<Response> {
    if id == crate::link_wallet::SERVICE {
        return Ok(web::ok(crate::link_wallet::set_agents(&state.config(), &body.agents, &default_run()).await?));
    }
    let svc = service_or_404(&id)?;
    let config = state.config();
    let run = default_run();
    let _guard = LOCK.lock().await;
    let store = store_of(&config);
    let mut stored = store.read()?;
    let Some(conn) = stored.connections.get(&svc.id).cloned() else {
        return Err(HttpError::new(404, "not connected"));
    };
    let agents = agents_of(&config, &body.agents, svc, &conn.agents)?;
    let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
    for name in conn.agents.iter().filter(|a| !names.contains(a)) {
        if let Some(agent) = config.agent(Some(name)) {
            unwire(&config, agent, &svc.id, &run).await.map_err(|e| HttpError::new(502, format!("Hermes: {e}")))?;
        }
    }
    let mut conn = conn;
    conn.agents = names;
    stored.connections.insert(svc.id.clone(), conn.clone());
    store.write(&stored)?;
    if conn.state == "connected" {
        for agent in &agents {
            wire(&config, agent, &conn, &run).await.map_err(|e| HttpError::new(502, format!("Hermes: {e}")))?;
        }
    }
    Ok(web::ok(view(svc, Some(&conn))))
}

async fn disconnect(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    if id == crate::link_wallet::SERVICE {
        return Ok(web::ok(crate::link_wallet::disconnect(&state.config(), &default_run()).await?));
    }
    let svc = service_or_404(&id)?;
    let config = state.config();
    let run = default_run();
    let _guard = LOCK.lock().await;
    let store = store_of(&config);
    let mut stored = store.read()?;
    let Some(conn) = stored.connections.remove(&svc.id) else {
        return Err(HttpError::new(404, "not connected"));
    };
    for name in &conn.agents {
        if let Some(agent) = config.agent(Some(name)) {
            if let Err(e) = unwire(&config, agent, &svc.id, &run).await {
                tracing::warn!(agent = %name, service = %svc.id, error = %e, "connections: not removed from the profile");
            }
        }
    }
    store.write(&stored)?;
    let revoked = revoke(&conn).await;
    tracing::info!(service = %svc.id, revoked, "connections: disconnected");
    Ok(web::ok(json!({"ok": true, "revoked": revoked})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/connections", get(listing))
        .route("/connections/callback", post(callback_route))
        .route("/connections/{service}/start", post(start_route))
        .route("/connections/{service}/agents", put(set_agents))
        .route("/connections/{service}", axum::routing::delete(disconnect))
}

/// Without the key: the browser coming back (`return_to: "bridge"`).
pub fn callback_router() -> Router<AppState> {
    Router::new().route("/oauth/callback", get(browser_callback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalog_has_notion_then_finary_in_four_languages() {
        let ids: Vec<&str> = catalog().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["notion", "finary"]);
        for svc in catalog() {
            for lang in ["fr", "en", "de", "es"] {
                let t = &svc.texts[lang];
                assert!(!t.label.is_empty() && !t.description.is_empty() && !t.privacy.is_empty(), "{} {lang}", svc.id);
            }
            assert!(svc.url.starts_with("https://") && svc.issuer.starts_with("https://"));
        }
        assert!(service("finary").unwrap().read_only && !service("notion").unwrap().read_only);
        assert!(service("finary").unwrap().scope.contains("offline_access"));
    }

    #[test]
    fn pkce_is_base64url_sha256() {
        // computed apart with Python's hashlib and base64.urlsafe_b64encode, padding removed
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mJ92IZiE7oTFv8T5KJu1Zk8pK5rDZq"),
            "j-ZZuS0GQldBWvk-M_Y9NjvJ9JGrgF1auBdHL88wnFc"
        );
    }

    #[test]
    fn well_known_addresses_put_the_suffix_before_the_path() {
        assert_eq!(
            well_known("https://public-api.finary.com/mcp", "oauth-protected-resource"),
            [
                "https://public-api.finary.com/.well-known/oauth-protected-resource/mcp",
                "https://public-api.finary.com/.well-known/oauth-protected-resource"
            ]
        );
        assert_eq!(
            well_known("https://clerk.finary.com", "oauth-authorization-server"),
            ["https://clerk.finary.com/.well-known/oauth-authorization-server"]
        );
    }

    #[test]
    fn the_callback_query_from_the_apps_address() {
        let q = callback_query("bipagents://oauth/callback?code=a%2Fb&state=xyz&iss=https%3A%2F%2Fmcp.notion.com");
        assert_eq!(q["code"], "a/b");
        assert_eq!(q["state"], "xyz");
        assert_eq!(q["iss"], "https://mcp.notion.com");
        assert_eq!(callback_query("code=1&state=2")["state"], "2");
    }

    #[test]
    fn the_account_from_the_answer_or_its_id_token() {
        let mut answer = serde_json::Map::new();
        answer.insert("workspace_name".into(), json!("Maison"));
        assert_eq!(account_of(&answer).as_deref(), Some("Maison"));
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"email":"a@example.org"}"#);
        let mut answer = serde_json::Map::new();
        answer.insert("id_token".into(), json!(format!("h.{claims}.s")));
        assert_eq!(account_of(&answer).as_deref(), Some("a@example.org"));
        assert_eq!(account_of(&serde_json::Map::new()), None);
    }

    #[test]
    fn the_authorize_address_carries_pkce_resource_and_state() {
        let server = Server {
            authorization_endpoint: "https://as.example/authorize".into(),
            resource: "https://mcp.example".into(),
            ..Default::default()
        };
        let client = Client {
            client_id: "c1".into(),
            redirect_uri: APP_REDIRECT.into(),
            auth_method: "none".into(),
            ..Default::default()
        };
        let url = authorize_url(&server, &client, "st", "ch", "openid offline_access");
        let q = callback_query(&url);
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], APP_REDIRECT);
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["resource"], "https://mcp.example");
        assert_eq!(q["prompt"], "consent");
        assert!(!callback_query(&authorize_url(&server, &client, "st", "ch", "")).contains_key("scope"));
    }

    #[test]
    fn a_renewal_without_a_new_refresh_token_keeps_the_old_one() {
        let old = Tokens { access_token: "a".into(), refresh_token: Some("r1".into()), ..Default::default() };
        let mut answer = serde_json::Map::new();
        answer.insert("access_token".into(), json!("b"));
        answer.insert("expires_in".into(), json!(3600));
        let new = tokens_of(&answer, Some(&old));
        assert_eq!(new.refresh_token.as_deref(), Some("r1"));
        assert!(new.expires_at.unwrap() > py::time_time() + 3500.0);
    }

    #[test]
    fn hermes_gets_the_access_token_only() {
        let dir = std::env::temp_dir().join(format!("bip-conn-{}", py::token_hex(4)));
        let conn = Connection {
            service: "notion".into(),
            client: Client {
                client_id: "c1".into(),
                client_secret: Some("s".into()),
                auth_method: "client_secret_post".into(),
                redirect_uri: APP_REDIRECT.into(),
            },
            tokens: Tokens {
                access_token: "AT".into(),
                refresh_token: Some("RT".into()),
                scope: None,
                expires_at: Some(py::time_time() + 100.0),
            },
            ..Default::default()
        };
        write_token_files(&dir, "notion", &conn).unwrap();
        let tokens = std::fs::read_to_string(dir.join("notion.json")).unwrap();
        let client = std::fs::read_to_string(dir.join("notion.client.json")).unwrap();
        assert!(tokens.contains("\"AT\"") && !tokens.contains("RT") && tokens.contains("expires_at"));
        assert!(client.contains("c1") && !client.contains("\"s\""));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("notion.json")).unwrap().permissions().mode() & 0o777, 0o600);
        remove_token_files(&dir, "notion");
        assert!(!dir.join("notion.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_store_is_encrypted() {
        let dir = std::env::temp_dir().join(format!("bip-store-{}", py::token_hex(4)));
        let store = Store::new(&dir);
        let mut stored = Stored::default();
        stored.connections.insert(
            "notion".into(),
            Connection {
                service: "notion".into(),
                tokens: Tokens { access_token: "SECRET-AT".into(), ..Default::default() },
                ..Default::default()
            },
        );
        store.write(&stored).unwrap();
        let raw = std::fs::read_to_string(dir.join("connections.json.enc")).unwrap();
        assert!(!raw.contains("SECRET-AT"));
        assert_eq!(store.read().unwrap().connections["notion"].tokens.access_token, "SECRET-AT");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
