//! Link Agent Wallet (Stripe), for people in the United States and Canada (decided 2026-10-26, reports/Paiements par
//! les agents.md): what Grok Bot uses. For each purchase Link makes a one-time virtual card, capped to the amount and
//! tied to the merchant; the person approves it in our app AND in Link's own app, then the bridge types that card
//! into the agent's browser as it types a card of the vault (card_pay.rs). The person's real card stays at Link;
//! the one-time card never reaches the model, its conversation or a log.
//!
//! Off unless `[link] enabled = true` and Kerfy Studio's OAuth client is configured (`client_id`,
//! `client_secret_file`, `publishable_key`): Stripe gives that client on request (Link Agent Wallet's registration
//! form), with the redirect address registered exactly (`redirect_uri`, `bipagents://oauth/callback` by default,
//! the apps' own scheme). The person's Link account is linked from the app like the other services of « Se connecter
//! avec… » (connections.rs routes, service `link`, listed only when this is on; the apps show it only for an App
//! Store / Google Play country of the United States or Canada, the row says `countries`). Its tokens are kept
//! encrypted like those connections' (`data_dir/link/`), renewed by the bridge; the agents never get them.
//!
//! A purchase, step by step (`pay_with_link`, on the agents' MCP entry `bipagents_link`, for the agents the person
//! chose):
//!
//! 1. the agent opens the site's payment step (the page with the card fields) and calls `pay_with_link` with the
//!    total, the merchant and what it buys; the bridge checks the page's real address (Chrome's own) is the
//!    merchant's, on https, with a card number field;
//! 2. our approval card (an MCP elicitation through Hermes' approvals, `approval_message`): « Léa veut payer 34,90 $
//!    sur shop.example avec Link »;
//! 3. on « yes », a spend request at Link (`link-cli spend-request create … --request-approval`, the access token in
//!    its environment): Link asks the person in its own app; the bridge waits for the answer (9 minutes at most);
//! 4. approved: the card is fetched into a file of the bridge's own (`--output-file`, 0600, in a 0700 folder), read,
//!    deleted at once, typed into the held page (card_pay::pay_with_values), then wiped from memory;
//! 5. the agent presses the pay button with `confirm_payment` (or gives up with `cancel_payment`), as with the vault.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::card_fill::CardValues;
use crate::card_pay::{Asker, ToolServer};
use crate::cards;
use crate::config::{AgentConfig, Config};
use crate::connections::{Client, ConnectError, Connection, Server, Store, Tokens};
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const SERVICE: &str = "link";
pub const SERVER_NAME: &str = "bipagents_link";
pub const PATH: &str = "/v1/agent-tools/link";
pub const PAY: &str = "pay_with_link";
pub const CONFIRM: &str = crate::card_pay::CONFIRM;
pub const CANCEL: &str = crate::card_pay::CANCEL;
/// Seconds Hermes waits for a call: our approval (5 minutes at most), then Link's (9 at most).
pub const TOOL_TIMEOUT: u32 = 1000;
/// The countries Link Agent Wallet serves for agent payments (Stripe's docs, « Disponibilité »).
pub const COUNTRIES: [&str; 2] = ["US", "CA"];
pub const SCOPE: &str = "payment_methods.agentic userinfo:read";
const FLOW_SECONDS: u64 = 600;
const LINK_WAIT_SECONDS: u32 = 540;
const RENEW_MARGIN: f64 = 300.0;

// -- settings ---------------------------------------------------------------------------------------------------------

/// `[link]` of bridge.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub client_id: String,
    /// A file holding Kerfy Studio's OAuth client secret (0600), never the secret itself in bridge.toml.
    pub client_secret_file: String,
    /// Kerfy Studio's Stripe publishable key (`pk_live_…`), which Link's sign-in and token endpoints ask for.
    pub publishable_key: String,
    pub redirect_uri: String,
    /// Link's sign-in server.
    pub login_url: String,
    /// Stripe's `link-cli` (npm `@stripe/link-cli`).
    pub cli: String,
    /// Spend requests in Link's test mode (test cards, nothing charged).
    pub test: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            client_id: String::new(),
            client_secret_file: String::new(),
            publishable_key: String::new(),
            redirect_uri: crate::connections::APP_REDIRECT.to_string(),
            login_url: "https://login.link.com".to_string(),
            cli: "link-cli".to_string(),
            test: false,
        }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let mut out = Settings::default();
        let text = |key: &str| table.get(key).and_then(Value::as_str).map(|s| s.trim().to_string());
        out.enabled = table.get("enabled").and_then(Value::as_bool).unwrap_or(false);
        out.test = table.get("test").and_then(Value::as_bool).unwrap_or(false);
        if let Some(v) = text("client_id") {
            out.client_id = v;
        }
        if let Some(v) = text("client_secret_file") {
            out.client_secret_file = v;
        }
        if let Some(v) = text("publishable_key") {
            out.publishable_key = v;
        }
        for (key, slot) in
            [("redirect_uri", &mut out.redirect_uri), ("login_url", &mut out.login_url), ("cli", &mut out.cli)]
        {
            if let Some(v) = text(key).filter(|v| !v.is_empty()) {
                *slot = v.trim_end_matches('/').to_string();
            }
        }
        out
    }

    /// Switched on and configured.
    pub fn ready(&self) -> bool {
        self.enabled
            && !self.client_id.is_empty()
            && !self.client_secret_file.is_empty()
            && self.publishable_key.starts_with("pk_")
    }

    fn secret(&self) -> Option<Zeroizing<String>> {
        let text = std::fs::read_to_string(&self.client_secret_file).ok()?;
        let text = Zeroizing::new(text.trim().to_string());
        (!text.is_empty()).then_some(text)
    }

    fn server(&self) -> Server {
        Server {
            issuer: self.login_url.clone(),
            authorization_endpoint: format!("{}/auth", self.login_url),
            token_endpoint: format!("{}/auth/token", self.login_url),
            registration_endpoint: None,
            revocation_endpoint: Some(format!("{}/auth/revoke", self.login_url)),
            userinfo_endpoint: None,
            resource: String::new(),
            mcp_url: String::new(),
        }
    }
}

/// Link is offered here: switched on and configured.
pub fn available(config: &Config) -> bool {
    config.link.ready()
}

fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("link")
}

fn store(config: &Config) -> Store {
    Store::new(&folder(config))
}

fn connection(config: &Config) -> Option<Connection> {
    store(config).read().ok()?.connections.get(SERVICE).cloned()
}

/// One change at a time (sign-in, renewal, agents, disconnect).
static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

// -- what the apps see (connections.rs's listing shape) ---------------------------------------------------------------

fn texts() -> Value {
    json!({
        "fr": {"label": "Link",
               "description": "Pour chaque achat, une carte à usage unique plafonnée au montant. Tu valides dans BipAgents, puis dans l’app Link.",
               "privacy": "Tes vrais numéros de carte restent chez Link (Stripe). Ton agent ne voit jamais la carte à usage unique."},
        "en": {"label": "Link",
               "description": "For each purchase, a one-time card capped to the amount. You approve in BipAgents, then in the Link app.",
               "privacy": "Your real card numbers stay with Link (Stripe). Your agent never sees the one-time card."},
        "de": {"label": "Link",
               "description": "Für jeden Kauf eine Einmalkarte, begrenzt auf den Betrag. Du bestätigst in BipAgents, dann in der Link-App.",
               "privacy": "Deine echten Kartennummern bleiben bei Link (Stripe). Dein Agent sieht die Einmalkarte nie."},
        "es": {"label": "Link",
               "description": "Para cada compra, una tarjeta de un solo uso limitada al importe. Apruebas en BipAgents y luego en la app de Link.",
               "privacy": "Los números reales de tu tarjeta se quedan en Link (Stripe). Tu agente nunca ve la tarjeta de un solo uso."},
    })
}

/// Its row in `GET /v1/connections`, None when Link is off here.
pub fn listing_row(config: &Config) -> Option<Value> {
    if !available(config) {
        return None;
    }
    let conn = connection(config);
    let conn = conn.as_ref();
    Some(json!({
        "service": SERVICE,
        "label": "Link",
        "icon": "creditcard",
        "read_only": false,
        "texts": texts(),
        "countries": COUNTRIES,
        "connected": conn.is_some_and(|c| c.state == "connected"),
        "state": conn.map(|c| c.state.as_str()).unwrap_or("not_connected"),
        "account": conn.and_then(|c| c.account.clone()),
        "agents": conn.map(|c| c.agents.clone()).unwrap_or_default(),
        "connected_at": conn.map(|c| c.connected_at),
    }))
}

fn unavailable() -> ConnectError {
    ConnectError { status: 404, code: "unavailable", message: "Link isn't offered here".into() }
}

// -- sign-in (OAuth authorization code with PKCE, a confidential client) ---------------------------------------------

struct Flow {
    verifier: Zeroizing<String>,
    agents: Vec<String>,
    started: Instant,
}

static FLOWS: LazyLock<Mutex<std::collections::HashMap<String, Flow>>> = LazyLock::new(Default::default);

fn flows() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Flow>> {
    FLOWS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether a callback's `state` is a Link sign-in (connections.rs hands it here).
pub fn owns_state(state: &str) -> bool {
    flows().contains_key(state)
}

/// Link's sign-in address for the agents chosen.
pub fn authorize_url(settings: &Settings, state: &str, challenge: &str) -> String {
    let Ok(mut url) = url::Url::parse(&settings.server().authorization_endpoint) else { return String::new() };
    url.query_pairs_mut()
        .append_pair("key", &settings.publishable_key)
        .append_pair("client_id", &settings.client_id)
        .append_pair("redirect_uri", &settings.redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", SCOPE)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    url.to_string()
}

pub fn start(config: &Config, agents: &[String]) -> Result<Value, ConnectError> {
    if !available(config) {
        return Err(unavailable());
    }
    for name in agents {
        if config.agent(Some(name)).is_none() {
            return Err(ConnectError { status: 404, code: "unknown_agent", message: format!("no agent {name}") });
        }
    }
    let state = py::uuid4_hex();
    let verifier = Zeroizing::new(format!("{}{}", py::uuid4_hex(), py::uuid4_hex()));
    let url = authorize_url(&config.link, &state, &crate::connections::pkce_challenge(&verifier));
    let mut table = flows();
    table.retain(|_, f| f.started.elapsed().as_secs() < FLOW_SECONDS);
    table.insert(state.clone(), Flow { verifier, agents: agents.to_vec(), started: Instant::now() });
    Ok(json!({"authorize_url": url, "state": state, "redirect_uri": config.link.redirect_uri,
              "expires_in": FLOW_SECONDS}))
}

fn http() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(20)).build().unwrap_or_default()
}

/// Link's token endpoint: the publishable key in `Authorization`, the client's id and secret in the form.
async fn token_call(settings: &Settings, mut form: Vec<(&str, String)>) -> Result<Tokens, ConnectError> {
    let secret = settings.secret().ok_or_else(|| ConnectError {
        status: 503,
        code: "not_configured",
        message: "Link's client secret file can't be read".into(),
    })?;
    form.push(("client_id", settings.client_id.clone()));
    form.push(("client_secret", secret.to_string()));
    let resp = http()
        .post(settings.server().token_endpoint)
        .bearer_auth(&settings.publishable_key)
        .header("accept", "application/json")
        .form(&form)
        .send()
        .await
        .map_err(|e| ConnectError { status: 502, code: "exchange_failed", message: e.without_url().to_string() })?;
    let status = resp.status().as_u16();
    let answer: Value = resp.json().await.unwrap_or(Value::Null);
    let access = answer.get("access_token").and_then(Value::as_str).unwrap_or("");
    if !(200..300).contains(&status) || access.is_empty() {
        let why = answer.get("error").and_then(Value::as_str).unwrap_or("no access token").to_string();
        return Err(ConnectError { status: 502, code: "exchange_failed", message: format!("{status} {why}") });
    }
    let expires_in = answer.get("expires_in").and_then(Value::as_f64).unwrap_or(3600.0);
    Ok(Tokens {
        access_token: access.to_string(),
        refresh_token: answer.get("refresh_token").and_then(Value::as_str).map(str::to_string),
        scope: answer.get("scope").and_then(Value::as_str).map(str::to_string),
        expires_at: Some(py::time_time() + expires_in),
    })
}

/// The app's callback (`bipagents://oauth/callback?code=…&state=…`): the code exchanged, the tokens kept.
pub async fn finish(
    config: &Config,
    query: &std::collections::HashMap<String, String>,
    run: &Run,
) -> Result<Value, ConnectError> {
    let state = query.get("state").cloned().unwrap_or_default();
    let flow = flows().remove(&state).filter(|f| f.started.elapsed().as_secs() < FLOW_SECONDS);
    let Some(flow) = flow else {
        return Err(ConnectError { status: 400, code: "unknown_state", message: "no such sign-in".into() });
    };
    if query.contains_key("error") {
        return Err(ConnectError {
            status: 400,
            code: "denied",
            message: "Link refused or the person declined".into(),
        });
    }
    let Some(code) = query.get("code").filter(|c| !c.is_empty()) else {
        return Err(ConnectError { status: 400, code: "bad_callback", message: "no code".into() });
    };
    let settings = &config.link;
    let tokens = token_call(
        settings,
        vec![
            ("grant_type", "authorization_code".to_string()),
            ("redirect_uri", settings.redirect_uri.clone()),
            ("code", code.clone()),
            ("code_verifier", flow.verifier.to_string()),
        ],
    )
    .await?;
    if !tokens.scope.as_deref().unwrap_or(SCOPE).split([' ', ',']).any(|s| s == "payment_methods.agentic") {
        return Err(ConnectError { status: 400, code: "denied", message: "payments were not allowed".into() });
    }
    let _guard = LOCK.lock().await;
    let store = store(config);
    let mut stored = store.read().map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
    let conn = Connection {
        service: SERVICE.into(),
        client: Client {
            client_id: settings.client_id.clone(),
            client_secret: None, // in its own file, never in the store
            auth_method: "client_secret_post".into(),
            redirect_uri: settings.redirect_uri.clone(),
        },
        server: settings.server(),
        tokens,
        account: None,
        agents: flow.agents.clone(),
        connected_at: py::time_time(),
        refreshed_at: None,
        state: "connected".into(),
    };
    stored.connections.insert(SERVICE.into(), conn.clone());
    store.write(&stored).map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
    drop(_guard);
    wire_all(config, &conn.agents, run).await;
    tracing::info!(agents = ?conn.agents, "Link connected");
    Ok(listing_row(config).unwrap_or(Value::Null))
}

/// The agents that use Link: their entry written (or removed).
pub async fn set_agents(config: &Config, agents: &[String], run: &Run) -> Result<Value, ConnectError> {
    if !available(config) {
        return Err(unavailable());
    }
    let before = {
        let _guard = LOCK.lock().await;
        let store = store(config);
        let mut stored =
            store.read().map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
        let Some(conn) = stored.connections.get_mut(SERVICE) else {
            return Err(ConnectError { status: 404, code: "not_connected", message: "not connected".into() });
        };
        let before = std::mem::replace(&mut conn.agents, agents.to_vec());
        store.write(&stored).map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
        before
    };
    let mut touched = before;
    touched.extend(agents.iter().cloned());
    wire_all(config, &touched, run).await;
    Ok(listing_row(config).unwrap_or(Value::Null))
}

/// Disconnected: the agents' entries removed, the authorization revoked at Link, the tokens deleted.
pub async fn disconnect(config: &Config, run: &Run) -> Result<Value, ConnectError> {
    let conn = {
        let _guard = LOCK.lock().await;
        let store = store(config);
        let mut stored =
            store.read().map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
        let Some(conn) = stored.connections.remove(SERVICE) else {
            return Err(ConnectError { status: 404, code: "not_connected", message: "not connected".into() });
        };
        store.write(&stored).map_err(|e| ConnectError { status: 500, code: "store", message: e.to_string() })?;
        conn
    };
    wire_all(config, &conn.agents, run).await;
    let mut revoked = false;
    if let (Some(refresh), Some(secret)) = (conn.tokens.refresh_token.as_ref(), config.link.secret()) {
        let form = [
            ("client_id", config.link.client_id.clone()),
            ("client_secret", secret.to_string()),
            ("token", refresh.clone()),
            ("token_type_hint", "refresh_token".to_string()),
        ];
        let url = format!("{}/auth/revoke", config.link.login_url);
        revoked = http()
            .post(url)
            .bearer_auth(&config.link.publishable_key)
            .form(&form)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
    }
    tracing::info!(revoked, "Link disconnected");
    Ok(json!({"ok": true, "revoked": revoked}))
}

/// An access token with 5 minutes left at least (renewed with the refresh token, which Link rotates).
async fn access_token(config: &Config) -> Option<Zeroizing<String>> {
    let _guard = LOCK.lock().await;
    let store = store(config);
    let mut stored = store.read().ok()?;
    let conn = stored.connections.get_mut(SERVICE)?;
    if conn.state != "connected" {
        return None;
    }
    let fresh = conn.tokens.expires_at.is_some_and(|at| at - py::time_time() > RENEW_MARGIN);
    if !fresh {
        let refresh = conn.tokens.refresh_token.clone()?;
        match token_call(&config.link, vec![("grant_type", "refresh_token".into()), ("refresh_token", refresh)]).await {
            Ok(mut tokens) => {
                if tokens.refresh_token.is_none() {
                    tokens.refresh_token = conn.tokens.refresh_token.clone();
                }
                conn.tokens = tokens;
                conn.refreshed_at = Some(py::time_time());
            }
            Err(e) => {
                tracing::warn!(error = %e, "Link: renewal refused");
                if e.message.contains("invalid_grant") {
                    conn.state = "expired".into();
                }
                let _ = store.write(&stored);
                return None;
            }
        }
        let _ = store.write(&stored);
    }
    stored.connections.get(SERVICE).map(|c| Zeroizing::new(c.tokens.access_token.clone()))
}

// -- the agents' entry ------------------------------------------------------------------------------------------------

/// Whether `agent` gets the Link tools: Link on here, connected, and this agent chosen.
pub fn offered(config: &Config, agent: &AgentConfig) -> bool {
    available(config)
        && connection(config).is_some_and(|c| c.state == "connected" && c.agents.iter().any(|a| a == &agent.name))
}

pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": PAY,
            "description": format!("Pay with the person's Link wallet (Stripe): a one-time card made for this \
        purchase, capped to its amount and tied to the shop. Open the site's payment step first (the page with the \
        card number field), note the total and the pay button's text, then call this with the total and currency \
        shown, the shop's name and address, and what you buy. The person approves in the BipAgents app, then in \
        the Link app; on « yes » the card is typed into the page for you (you never see it) and your browser is \
        paused: then call {CONFIRM} (or {CANCEL}). {}", crate::handover::RULES),
            "inputSchema": {"type": "object", "properties": {
                "amount": {"type": "string", "description": "the total to pay as the page shows it, e.g. 34.90"},
                "currency": {"type": "string", "description": "e.g. USD"},
                "merchant_name": {"type": "string", "description": "the shop's name, e.g. « Northwind Outfitters »"},
                "merchant_url": {"type": "string", "description": "the shop's https address"},
                "items": {"type": "string", "description": "what you buy, in a few words, in the person's language"},
            }, "required": ["amount", "currency", "merchant_name", "merchant_url", "items"]},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": true},
        }),
        json!({
            "name": CONFIRM,
            "description": format!("After {PAY}: press the page's pay button (you can't use your browser \
        meanwhile). Give the button's text as you read it before, e.g. « Pay $34.90 »."),
            "inputSchema": {"type": "object", "properties": {
                "button": {"type": "string", "description": "the pay button's text"},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "openWorldHint": true},
        }),
        json!({
            "name": CANCEL,
            "description": "Give up the payment in progress: the card is erased from the page and your browser is \
        yours again.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
        }),
    ]
}

pub fn tools_url(config: &Config) -> String {
    let base = crate::agent_tools::url_with(config, "");
    let base = base.split('?').next().unwrap_or("").trim_end_matches(crate::agent_tools::PATH).to_string();
    format!("{base}{PATH}?v={}", crate::agent_tools::version_of(&tools()))
}

/// The agent's Hermes knows the Link tools while it may use them, and forgets them otherwise. Returns whether its
/// profile changed.
pub async fn register(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<bool> {
    if offered(config, agent) {
        return crate::card_pay::register_entry(config, agent, run, SERVER_NAME, &tools_url(config), TOOL_TIMEOUT)
            .await;
    }
    unregister(config, agent, run).await
}

async fn unregister(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<bool> {
    let Some(folder) = crate::shares::profile_dir(config, agent) else { return Ok(false) };
    let profile = crate::shares::profile_config(&folder);
    if profile.get("mcp_servers").and_then(|s| s.get(SERVER_NAME)).is_none() {
        return Ok(false);
    }
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env: std::collections::BTreeMap<String, String> = [("HERMES_HOME".to_string(), home)].into_iter().collect();
    run(
        vec![
            config.hermes.bin.clone(),
            "-p".into(),
            agent.name.clone(),
            "config".into(),
            "unset".into(),
            format!("mcp_servers.{SERVER_NAME}"),
        ],
        env,
    )
    .await?;
    Ok(true)
}

async fn wire_all(config: &Config, names: &[String], run: &Run) {
    let mut seen = std::collections::HashSet::new();
    for name in names {
        if !seen.insert(name.clone()) {
            continue;
        }
        if let Some(agent) = config.agent(Some(name)) {
            if let Err(e) = register(config, agent, run).await {
                tracing::warn!(agent = %name, error = %e, "Link tools not (un)registered");
            }
        }
    }
}

/// What Hermes shows as our approval, read by the apps (LinkPayment.swift / .kt).
pub fn approval_message(id: &str, cents: i64, currency: &str, site: &str, items: &str) -> String {
    format!(
        "Link payment {id}: {} {currency} on {site} for \"{}\". Approve here, then in the Link app.",
        cards::format_cents(cents),
        cards::quoted(items, 120)
    )
}

/// What the person reads in Link's app (at least 100 characters, Link's rule).
pub fn context_for(agent: &str, items: &str, merchant: &str, site: &str) -> String {
    let text = format!(
        "{items}, bought from {merchant} ({site}) by {agent}, the person's BipAgents assistant, at their request. \
         Approved first in the BipAgents app."
    );
    if text.chars().count() >= 100 {
        text
    } else {
        format!("{text} One-time card for this purchase only, capped to its total.")
    }
}

fn arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).filter(|v| !v.is_null()).map(py::py_str).unwrap_or_default().trim().to_string()
}

fn refusal(text: &str) -> Dict {
    tool_text(text, true)
}

/// The output of `link-cli … --format json`, never logged (the card itself only ever goes to a file).
async fn cli(config: &Config, token: &str, args: &[String], seconds: u64) -> Result<Value, String> {
    let home = folder(config).join("cli");
    py::makedirs(&home.to_string_lossy(), 0o700).map_err(|e| e.to_string())?;
    let mut command = tokio::process::Command::new(&config.link.cli);
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()))
        .env("HOME", &home)
        .env("LINK_ACCESS_TOKEN", token)
        .env("LINK_NO_REFRESH", "1")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(seconds), command.output())
        .await
        .map_err(|_| "Link did not answer in time".to_string())?
        .map_err(|e| format!("link-cli can't run here ({e})"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str::<Value>(text.trim()).unwrap_or(Value::Null);
    if value.is_null() {
        return Err(format!("link-cli answered {}", output.status));
    }
    Ok(value)
}

/// The one-time card from its file, the file deleted at once. Not `Debug`; wiped when dropped.
fn read_card(path: &Path) -> Result<(CardValues, String), String> {
    let raw = Zeroizing::new(std::fs::read(path).map_err(|_| "the card's file is missing".to_string())?);
    let _ = std::fs::remove_file(path);
    let value: Value = serde_json::from_slice(&raw).map_err(|_| "the card's file can't be read".to_string())?;
    let card = value.get("card").unwrap_or(&value);
    let s = |k: &str| card.get(k).map(py::py_str).unwrap_or_default();
    let n = |k: &str| card.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
    let address = card.get("billing_address").cloned().unwrap_or(Value::Null);
    let values = CardValues {
        number: s("number"),
        exp_month: n("exp_month"),
        exp_year: n("exp_year"),
        csc: s("cvc"),
        name: address.get("name").map(py::py_str).unwrap_or_default(),
        postal_code: address.get("postal_code").map(py::py_str).unwrap_or_default(),
    };
    if values.number.len() < 12 || values.exp_month == 0 {
        return Err("Link gave no usable card".into());
    }
    let last4 = values.number.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    Ok((values, last4))
}

fn bare(host: &str) -> String {
    host.trim().trim_end_matches('.').trim_start_matches("www.").to_lowercase()
}

async fn pay(state: &AppState, agent: &AgentConfig, arguments: &Dict, asker: &Asker) -> anyhow::Result<Dict> {
    let config = state.config();
    if !offered(&config, agent) {
        return Ok(refusal("Link isn't connected for you: the person connects it in the app (your tools, « Link »)."));
    }
    let items = cards::quoted(&arg(arguments, "items"), 120);
    let merchant = cards::quoted(&arg(arguments, "merchant_name"), 80);
    if items.is_empty() || merchant.is_empty() {
        return Ok(refusal("Say what you buy (items) and from which shop (merchant_name)."));
    }
    let cents = match cards::amount_cents(arguments.get("amount").unwrap_or(&Value::Null)) {
        Ok(c) => c,
        Err(why) => return Ok(refusal(&why)),
    };
    let Some(currency) = cards::currency(&arg(arguments, "currency")) else {
        return Ok(refusal("Give the currency as its 3-letter code, e.g. USD."));
    };
    let merchant_url = arg(arguments, "merchant_url");
    let wanted = url::Url::parse(&merchant_url).ok().and_then(|u| u.host_str().map(bare)).unwrap_or_default();
    let (mut page, port, url, host) = match crate::card_pay::payment_page(&config, agent).await {
        Ok(found) => found,
        Err(refused) => return Ok(refused),
    };
    let got = bare(&host);
    if wanted.is_empty()
        || !(got == wanted || got.ends_with(&format!(".{wanted}")) || wanted.ends_with(&format!(".{got}")))
    {
        return Ok(refusal(&format!(
            "The page open in your browser is on {host}, not on the shop's address ({merchant_url}). Pay only on the \
             shop's own payment page."
        )));
    }
    let fields = page.fields().await.unwrap_or_default();
    drop(page);
    if !fields.iter().any(|f| f.kind == "number") {
        return Ok(refusal(&format!(
            "No card number field on this page ({host}). Go to the payment step (where the card number is asked), \
             then call {PAY} again."
        )));
    }
    let id = format!("lnk_{}", &py::uuid4_hex()[..12]);
    if !asker.ask(&approval_message(&id, cents, &currency, &host, &items)).await {
        return Ok(refusal("The person did not approve this payment: nothing was asked of Link. Don't pay."));
    }
    let Some(token) = access_token(&config).await else {
        return Ok(refusal("Link can't be reached for this person now (connect it again in the app). Don't pay."));
    };
    let mut args: Vec<String> = [
        "spend-request",
        "create",
        "--amount",
        &cents.to_string(),
        "--currency",
        &currency.to_lowercase(),
        "--merchant-name",
        &merchant,
        "--merchant-url",
        &merchant_url,
        "--context",
        &context_for(&agent.display_name, &items, &merchant, &host),
        "--line-item",
        &format!("name:{},unit_amount:{cents},quantity:1", items.replace([',', ':'], " ")),
        "--total",
        &format!("type:total,display_text:Total,amount:{cents}"),
        "--idempotency-key",
        &id,
        "--request-approval",
        "--format",
        "json",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if config.link.test {
        args.push("--test".into());
    }
    let created = match cli(&config, &token, &args, 60).await {
        Ok(v) => v,
        Err(why) => {
            tracing::warn!(agent = %agent.name, error = %why, "Link: spend request not created");
            return Ok(refusal("Link did not take the request. Don't pay; tell the person to finish on their phone."));
        }
    };
    let Some(request) =
        created.get("id").and_then(Value::as_str).filter(|i| i.starts_with("lsrq_")).map(str::to_string)
    else {
        return Ok(refusal("Link did not take the request. Don't pay; tell the person to finish on their phone."));
    };
    tracing::info!(agent = %agent.name, site = %host, request = %request, "Link: approval asked in Link's app");
    let wait = [
        "spend-request".to_string(),
        "retrieve".into(),
        request.clone(),
        "--interval".into(),
        "2".into(),
        "--timeout".into(),
        LINK_WAIT_SECONDS.to_string(),
        "--format".into(),
        "json".into(),
    ];
    let status = cli(&config, &token, &wait, LINK_WAIT_SECONDS as u64 + 30)
        .await
        .ok()
        .and_then(|v| v.get("status").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    if status != "approved" {
        let say = match status.as_str() {
            "denied" => {
                "The person declined in the Link app: nothing was paid. Don't ask again unless they tell you to."
            }
            "requires_action" => {
                "Link needs the person first (a check in the Link app or their bank): tell them to \
                                  open the Link app; nothing was paid."
            }
            _ => {
                "The person did not approve in the Link app in time: nothing was paid. Ask them if they still want it."
            }
        };
        return Ok(refusal(say));
    }
    // The card: into a file of the bridge's own, read and deleted at once.
    let dir = folder(&config).join("cards");
    py::makedirs(&dir.to_string_lossy(), 0o700)?;
    let file = dir.join(format!("{id}.json"));
    let _ = std::fs::remove_file(&file);
    let fetch = [
        "spend-request".to_string(),
        "retrieve".into(),
        request.clone(),
        "--include".into(),
        "card".into(),
        "--output-file".into(),
        file.to_string_lossy().into_owned(),
        "--format".into(),
        "json".into(),
    ];
    let fetched = cli(&config, &token, &fetch, 60).await;
    drop(token);
    let card = fetched.and_then(|_| read_card(&file));
    let _ = std::fs::remove_file(&file);
    let (values, last4) = match card {
        Ok(found) => found,
        Err(why) => {
            tracing::warn!(agent = %agent.name, error = %why, "Link: card not fetched");
            return Ok(refusal(
                "The one-time card could not be fetched from Link: nothing was typed. Tell the person.",
            ));
        }
    };
    Ok(crate::card_pay::pay_with_values(state, agent, values, "Link", &last4, cents, &currency, (url, port, host))
        .await)
}

pub async fn call(
    state: &AppState,
    agent: &AgentConfig,
    name: &str,
    arguments: &Dict,
    asker: &Asker,
) -> anyhow::Result<Dict> {
    match name {
        PAY => pay(state, agent, arguments, asker).await,
        CONFIRM => crate::card_pay::confirm_held(state, agent, arguments).await,
        CANCEL => Ok(crate::card_pay::cancel_held(state, agent).await),
        other => Ok(refusal(&format!("unknown tool: {other}"))),
    }
}

const INSTRUCTIONS: &str = "BipAgents Link: pay_with_link pays on the shop's payment page in your browser with a \
one-time card from the person's Link wallet, once they approve in the BipAgents app and in Link; then \
confirm_payment presses the pay button. Only pay what the person asked for.";

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
    let config = state.config();
    if !available(&config) {
        return Err(HttpError::new(404, "Link isn't offered here"));
    }
    crate::card_pay::serve(state, headers, body, &SERVER).await
}

/// The tools' route, at the root next to the other agent tools (same tokens).
pub fn router() -> Router<AppState> {
    Router::new().route(PATH, post(mcp))
}

/// `GET /v1/link`: whether Link is offered here and connected (the rest goes through `/v1/connections`).
async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    Ok(web::ok(json!({"available": available(&config), "countries": COUNTRIES, "connection": listing_row(&config)})))
}

pub fn app_router() -> Router<AppState> {
    Router::new().route("/link", get(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings::from_table(&json!({
            "enabled": true, "client_id": "lnkcli_test", "client_secret_file": "/nonexistent",
            "publishable_key": "pk_test_123",
        }))
    }

    #[test]
    fn off_by_default_and_until_configured() {
        assert!(!Settings::default().ready());
        assert!(!Settings::from_table(&json!({"enabled": true})).ready());
        assert!(!Settings::from_table(
            &json!({"client_id": "x", "client_secret_file": "/s", "publishable_key": "pk_x"})
        )
        .ready());
        assert!(settings().ready());
        assert_eq!(settings().redirect_uri, "bipagents://oauth/callback");
    }

    #[test]
    fn the_sign_in_asks_for_payments_with_pkce() {
        let url = authorize_url(&settings(), "st4te", "ch4llenge");
        assert!(url.starts_with("https://login.link.com/auth?key=pk_test_123&client_id=lnkcli_test"));
        assert!(url.contains("redirect_uri=bipagents%3A%2F%2Foauth%2Fcallback"));
        assert!(url.contains("scope=payment_methods.agentic+userinfo%3Aread"));
        assert!(url.contains("code_challenge=ch4llenge&code_challenge_method=S256"));
    }

    #[test]
    fn the_message_is_what_the_apps_read() {
        assert_eq!(
            approval_message("lnk_0123456789ab", 3490, "USD", "shop.example", "Trail \"shoes\""),
            "Link payment lnk_0123456789ab: 34.90 USD on shop.example for \"Trail 'shoes'\". Approve here, then in \
             the Link app."
        );
    }

    #[test]
    fn links_context_is_long_enough() {
        let text = context_for("Léa", "Gourde", "Northwind", "shop.example");
        assert!(text.chars().count() >= 100, "{text}");
    }

    #[test]
    fn the_card_file_is_read_then_gone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("c.json");
        std::fs::write(
            &path,
            r#"{"id":"lsrq_x","card":{"number":"4000009990001984","cvc":"100","exp_month":6,"exp_year":2029,
               "billing_address":{"name":"Jordan Example","postal_code":"94015"}}}"#,
        )
        .unwrap();
        let (values, last4) = read_card(&path).unwrap();
        assert_eq!(last4, "1984");
        assert_eq!((values.exp_month, values.exp_year), (6, 2029));
        assert!(!path.exists());
        assert!(read_card(&path).is_err());
    }
}
