//! Agents created from the app (« + »): on an install made by the installer, the bridge manages the person's own Hermes
//! (`[hermes]` in its config) and creates a profile for each new agent — the kind or the « Autre » description the person
//! chose, the name they gave — then serves it at once, without a restart. Everything runs as the person's own account.
//!
//! Routes (bridge key):
//! - `GET /v1/agent-kinds`: the kinds of agents offered, and whether this install can create agents.
//! - `POST /v1/agents`: `{"name", "kind", "lang"}` or `{"name", "custom": {"role", "tone", "rules", "category"}, "lang"}`
//!   → the new agent as `GET /v1/agents` lists it (id, name, tailnet url, key).
//! - `PUT /v1/agents/{id}/language`: `{"lang"}` — the language it writes first in (its SOUL.md's language block).
//!
//! Port of `bridge/bipbridge/agents_admin.py` (owner group: agents).

#[path = "agents/pyyaml.rs"]
pub mod pyyaml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Request, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use futures::future::BoxFuture;
use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config, SharedConfig};
use crate::web::{
    self, body_loc, check_max_len, check_min_len, ApiResult, FieldError, HttpError, Json, QueryMap, Validate,
};
use crate::Dict;
use pyyaml::Yaml;

/// How the bridge runs the `hermes` CLI (and other commands): `run(args, env)` → stdout; a failure (non-zero exit,
/// 120 s timeout) is Python's `RuntimeError`, its message « <args[:4]> failed: <stderr tail> ». Replaceable in
/// tests, as Python's `run=` parameters.
pub type Run =
    Arc<dyn Fn(Vec<String>, BTreeMap<String, String>) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>;

/// `run_hermes` as a [`Run`].
pub fn default_run() -> Run {
    Arc::new(|args, env| Box::pin(run_hermes(args, env)))
}

/// Python's `LookupError` (`managed`, `editable`: not an agent the bridge may change).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct LookupError(pub String);

/// Python's `ValueError` of `create_agent` and `write_settings` (safe to show: the routes answer 400 with it).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct AgentError(pub String);

/// Python's `asyncio.TimeoutError` of `run_hermes` (the command ran for 120 s).
#[derive(Debug, Clone, thiserror::Error)]
#[error("")]
pub struct CommandTimeout;

/// Python's `httpx.HTTPError` of `read_settings` (the agent's gateway did not answer, or not with a 2xx).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct GatewayError(pub String);

/// Python's `OSError` (a file or a process that could not be opened) inside an `anyhow::Error`: the routes let
/// those through as a 500, where a `RuntimeError` (a command that failed) is their 502.
pub fn is_os_error(exc: &anyhow::Error) -> bool {
    exc.chain().any(|e| e.downcast_ref::<std::io::Error>().is_some())
}

/// Python's `text[-n:]` (characters).
pub fn tail(text: &str, n: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(n)).collect()
}

/// Python's `text[:n]` (characters).
fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn hermes_env(config: &Config) -> BTreeMap<String, String> {
    if config.hermes.home.is_empty() {
        BTreeMap::new()
    } else {
        env_of(&[("HERMES_HOME", &config.hermes.home)])
    }
}

/// `install.home or Path.home() / ".hermes"`.
pub fn install_home(config: &Config) -> String {
    if config.hermes.home.is_empty() {
        crate::py::join(&crate::py::expanduser("~"), ".hermes")
    } else {
        config.hermes.home.clone()
    }
}

/// Python's `str.title()`.
fn title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_cased = false;
    for c in text.chars() {
        if c.is_alphabetic() {
            if previous_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            previous_cased = true;
        } else {
            out.push(c);
            previous_cased = false;
        }
    }
    out
}

/// A dict's bool values with pydantic's lax coercion (`Dict[str, bool]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LaxBool(pub bool);

impl<'de> Deserialize<'de> for LaxBool {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        web::lax::bool(d).map(LaxBool)
    }
}

fn lax_map(map: &Option<IndexMap<String, LaxBool>>) -> IndexMap<String, bool> {
    map.as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.0)).collect()).unwrap_or_default()
}

/// Errors of a nested model (`custom`), with its field under `["body", "custom", …]`.
fn nested(errors: &mut Vec<FieldError>, parent: &str, check: impl FnOnce(&mut Vec<FieldError>)) {
    let mut inner = Vec::new();
    check(&mut inner);
    for mut e in inner {
        e.loc.insert(1, json!(parent));
        errors.push(e);
    }
}

/// `Field(pattern=…)` on a str.
fn check_pattern(errors: &mut Vec<FieldError>, field: &str, value: &str, pattern: &str, re: &Regex) {
    if !re.is_match(value) {
        errors.push(
            FieldError::new(
                "string_pattern_mismatch",
                body_loc(field),
                format!("String should match pattern '{pattern}'"),
                json!(value),
            )
            .with_ctx(json!({"pattern": pattern})),
        );
    }
}

/// « Autre »: the role, tone and rules the person wrote.
#[derive(Debug, Clone, Deserialize)]
pub struct CustomAgent {
    pub role: String,
    #[serde(default)]
    pub tone: String,
    #[serde(default)]
    pub rules: String,
    /// Older apps: its language by name (« Français »).
    #[serde(default)]
    pub language: String,
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_category() -> String {
    "tech".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct NewAgent {
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    /// The agent's language (« en », « fr-FR »…): its personality is written in it, and it writes first in it.
    /// Older apps send none: French, as before.
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub custom: Option<CustomAgent>,
    /// Its group on the home screen (agent_groups.rs), none when empty.
    #[serde(default)]
    pub group: Option<String>,
}

impl Validate for NewAgent {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "name", Some(&self.name), 1);
        check_max_len(errors, "name", Some(&self.name), 40);
        check_max_len(errors, "group", self.group.as_deref().map(str::trim), crate::agent_groups::MAX_LEN);
        if let Some(c) = &self.custom {
            nested(errors, "custom", |errors| {
                check_min_len(errors, "role", Some(&c.role), 3);
                check_max_len(errors, "role", Some(&c.role), 600);
                check_max_len(errors, "tone", Some(&c.tone), 200);
                check_max_len(errors, "rules", Some(&c.rules), 600);
                check_max_len(errors, "language", Some(&c.language), 40);
            });
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentLanguage {
    pub lang: String,
}

impl Validate for AgentLanguage {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "lang", Some(&self.lang), 2);
        check_max_len(errors, "lang", Some(&self.lang), 20);
    }
}

const THINKING_PATTERN: &str = "^(none|low|medium|high|xhigh)$";
static THINKING_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(THINKING_PATTERN).expect("valid regex"));
const MODEL_PATTERN: &str = r"^[A-Za-z0-9._:/@+-]{1,120}$";
static MODEL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(MODEL_PATTERN).expect("valid regex"));

#[derive(Debug, Clone, Deserialize)]
pub struct AgentThinking {
    pub level: String,
}

impl Validate for AgentThinking {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_pattern(errors, "level", &self.level, THINKING_PATTERN, &THINKING_RE);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentModel {
    /// The router's id of the model.
    pub name: String,
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub vision: Option<bool>,
}

impl Validate for AgentModel {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_pattern(errors, "name", &self.name, MODEL_PATTERN, &MODEL_RE);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Claim {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
}

impl Validate for Claim {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "id", Some(&self.id), 64);
        check_max_len(errors, "name", self.name.as_deref(), 60);
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentSettings {
    #[serde(default)]
    pub soul: Option<String>,
    #[serde(default)]
    pub toolsets: Option<IndexMap<String, LaxBool>>,
    /// The agent's own skills (own_skills.rs), on or off.
    #[serde(default)]
    pub skills: Option<IndexMap<String, LaxBool>>,
    /// Its own browser (browsers::switch): set up the first time.
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub browser: Option<bool>,
    /// « Santé »: the person's health data and the `health` tool (health.rs).
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub health: Option<bool>,
    /// What it reads of that data (health::CATEGORIES), its switch on (set after `health`).
    #[serde(default)]
    pub health_categories: Option<Vec<String>>,
    /// « Météo »: the `weather` tool (weather.rs).
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub weather: Option<bool>,
}

impl Validate for AgentSettings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "soul", self.soul.as_deref(), 8000);
        for (n, name) in self.health_categories.iter().flatten().enumerate() {
            if !crate::health::CATEGORIES.contains(&name.as_str()) {
                errors.push(FieldError::new(
                    "value_error",
                    vec![json!("body"), json!("health_categories"), json!(n)],
                    format!("Value error, one of {}", crate::health::CATEGORIES.join(", ")),
                    json!(name),
                ));
            }
        }
    }
}

pub async fn run_hermes(args: Vec<String>, env: BTreeMap<String, String>) -> anyhow::Result<String> {
    let Some(program) = args.first() else { anyhow::bail!("no command") };
    let child = tokio::process::Command::new(program)
        .args(&args[1..])
        .envs(&env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let output = match tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await {
        Ok(output) => output?,
        Err(_) => return Err(CommandTimeout.into()),
    };
    if !output.status.success() {
        let shown = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
        let err = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{shown} failed: {}", tail(err.trim(), 300));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Tools that read this machine's files: off by default where the agents are not isolated from the bridge. The
/// browser too: Hermes' local Chromium opens file:// pages (checked: it read a file next to the bridge's keys).
pub const DISK_TOOLS: [&str; 4] = ["terminal", "file", "code_execution", "browser"];

fn load_yaml_file(path: &Path) -> anyhow::Result<Yaml> {
    let text = std::fs::read_to_string(path)?;
    Ok(pyyaml::load(&text)?)
}

/// Bot Mode: the agents of an install can message each other in their « Bot Chat ».
pub fn mark_as_bot(folder: &Path) -> anyhow::Result<()> {
    let path = folder.join("profile.yaml");
    let data = if path.exists() { load_yaml_file(&path)? } else { Yaml::Null };
    let mut data = if data.is_map() { data } else { Yaml::map() };
    let meta = data.entry_map("ui_meta").ok_or_else(|| anyhow::anyhow!("AttributeError: 'ui_meta' is not a dict"))?;
    if !meta.is_map() || meta.entry_map("hermes-bots").is_none() {
        anyhow::bail!("AttributeError: 'ui_meta' is not a dict");
    }
    std::fs::write(&path, pyyaml::dump(&data))?;
    Ok(())
}

fn line_key(line: &str) -> String {
    if line.contains('=') && !line.trim_start().starts_with('#') {
        line.split('=').next().unwrap_or("").trim().to_string()
    } else {
        String::new()
    }
}

/// A TOML basic string.
fn q(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The new agent in the bridge's config file, so it is still there after a restart. `kept`: other lines of its
/// section to keep (what the bridge does not set itself: its voice, ntfy, shares…).
pub fn append_agent(config_path: &str, agent: &AgentConfig, kept: &[String]) -> std::io::Result<()> {
    let mut lines = vec![
        String::new(),
        format!("[agents.{}]", agent.name),
        format!("display_name = {}", q(&agent.display_name)),
        format!("hermes_url = {}", q(&agent.hermes_url)),
    ];
    if !kept.iter().any(|k| line_key(k) == "hermes_key_file") {
        lines.push(format!("hermes_key = {}", q(&agent.hermes_key)));
    }
    if let Some(url) = agent.public_url.as_deref().filter(|u| !u.is_empty()) {
        lines.push(format!("public_url = {}", q(url)));
    }
    if let Some(host) = agent.upload_dir_host.as_deref().filter(|u| !u.is_empty()) {
        lines.push(format!("upload_dir_host = {}", q(host)));
        lines.push(format!("upload_dir_container = {}", q(agent.upload_dir_container.as_deref().unwrap_or(""))));
    }
    if agent.isolated {
        lines.push("isolated = true".into());
    }
    if agent.sandboxed {
        lines.push("sandboxed = true".into());
    }
    if !agent.python.is_empty() {
        lines.push(format!("python = [{}]", agent.python.iter().map(|a| q(a)).collect::<Vec<_>>().join(", ")));
    }
    let optional = [
        ("hermes_home", &agent.hermes_home),
        ("hermes_root", &agent.hermes_root),
        ("space", &agent.space),
        ("desktop_url", &agent.desktop_url),
        ("desktop_vnc", &agent.desktop_vnc),
        ("desktop_vnc_password_file", &agent.desktop_vnc_password_file),
    ];
    for (key, value) in optional {
        if let Some(value) = value.as_deref().filter(|v| !v.is_empty()) {
            lines.push(format!("{key} = {}", q(value)));
        }
    }
    let mut written: std::collections::HashSet<String> = lines.iter().map(|l| line_key(l)).collect();
    written.extend(MANAGED_KEYS.iter().map(|k| k.to_string()));
    lines.extend(kept.iter().filter(|line| !written.contains(&line_key(line))).cloned());
    use std::io::Write;
    let mut fh = std::fs::OpenOptions::new().append(true).create(true).open(config_path)?;
    fh.write_all((lines.join("\n") + "\n").as_bytes())
}

/// What a new agent will be (nothing written yet): its profile id, the name the person gave, its personality and its
/// profile's description.
#[derive(Debug, Clone)]
pub struct PlannedAgent {
    pub profile: String,
    pub name: String,
    pub soul: String,
    pub description: String,
}

pub fn plan_agent(config: &Config, body: &NewAgent) -> anyhow::Result<PlannedAgent> {
    let name = body.name.trim().to_string();
    let lang = match body.lang.as_deref().filter(|l| !l.is_empty()) {
        Some(lang) => lang.to_string(),
        None => match body.custom.as_ref().filter(|c| !c.language.is_empty()) {
            Some(c) => c.language.clone(),
            None => "fr".to_string(),
        },
    };
    let (soul, description) = if let Some(custom) = &body.custom {
        if !crate::souls::CATEGORIES.contains(&custom.category.as_str()) {
            return Err(AgentError("unknown category".into()).into());
        }
        let soul = crate::souls::custom_soul(&name, &custom.role, &custom.tone, &custom.rules, Some(&lang));
        (soul, head(custom.role.trim(), 120))
    } else if let Some(kind) = body.kind.as_deref().filter(|k| !k.is_empty()) {
        let found = crate::souls::template(kind, Some(&lang));
        let Some(found) = found.filter(|t| t.get("soul").map(crate::py::truthy).unwrap_or(false)) else {
            return Err(AgentError("unknown kind".into()).into());
        };
        let soul = crate::souls::soul_for(&name, kind, Some(&lang)).map_err(|e| AgentError(e.to_string()))?;
        (soul, found.get("description").map(crate::py::py_str).unwrap_or_default())
    } else {
        return Err(AgentError("a kind or a custom description is needed".into()).into());
    };
    let taken: Vec<String> = config.agents.keys().cloned().collect();
    let profile = crate::souls::profile_name(&name, &taken);
    Ok(PlannedAgent { profile, name, soul, description })
}

/// The agent as this bridge serves it, in the hub, with its key (its profile may not exist yet).
pub fn hub_agent(config: &Config, profile: &str, display_name: &str, key: &str) -> AgentConfig {
    let install = &config.hermes;
    let folder = PathBuf::from(install_home(config)).join("profiles").join(profile);
    let uploads = folder.join("uploads").to_string_lossy().into_owned();
    let seen_by_agent = if install.container_home.is_empty() {
        uploads.clone()
    } else {
        format!("{}/profiles/{profile}/uploads", install.container_home)
    };
    AgentConfig {
        name: profile.to_string(),
        display_name: display_name.to_string(),
        hermes_url: format!("{}/p/{profile}", install.hub_url),
        hermes_key: key.to_string(),
        upload_dir_host: Some(uploads),
        upload_dir_container: Some(seen_by_agent),
        public_url: public_agent_url(config, profile),
        ..AgentConfig::default()
    }
}

/// An agent listed (in the bridge's config, so the app gets it) before its profile is made (`make_profile`).
pub fn reserve_agent(shared: &SharedConfig, agent: &AgentConfig) -> anyhow::Result<()> {
    if let Some(path) = shared.get().source_path.as_deref() {
        append_agent(path, agent, &[])?;
    }
    shared.put_agent(agent.clone());
    Ok(())
}

/// A new agent in the person's hub (its profile, key, personality, tools); added to the configuration.
pub async fn create_agent(
    shared: &SharedConfig,
    body: &NewAgent,
    http: &reqwest::Client,
    run: &Run,
) -> anyhow::Result<AgentConfig> {
    let config = shared.get();
    let planned = plan_agent(&config, body)?;
    let agent = hub_agent(&config, &planned.profile, &planned.name, &crate::py::token_urlsafe(32));
    let agent = make_profile(shared, &planned, agent, http, run, false).await?;
    if let Some(group) = body.group.as_deref().filter(|g| !g.trim().is_empty()) {
        crate::agent_groups::set_group(&config, &agent.name, group)?;
    }
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, kind = %body.kind.as_deref().filter(|k| !k.is_empty()).unwrap_or("custom"), "agent created");
    Ok(agent)
}

/// The hub profile of a planned agent: made, given its key and personality, served, then saved in the bridge's
/// config (its section rewritten when it was reserved there first). `resume`: a profile already made by an attempt
/// cut short is kept, not made again.
pub async fn make_profile(
    shared: &SharedConfig,
    planned: &PlannedAgent,
    agent: AgentConfig,
    http: &reqwest::Client,
    run: &Run,
    resume: bool,
) -> anyhow::Result<AgentConfig> {
    let config = shared.get();
    let install = &config.hermes;
    let profile = planned.profile.clone();
    let env = hermes_env(&config);
    let hermes = install.bin.clone();
    let folder = PathBuf::from(install_home(&config)).join("profiles").join(&profile);
    let made = resume && folder.join("config.yaml").is_file();
    // The hub's (default profile's) model and settings, never another agent's: cloning the active profile would copy
    // its MCP servers and peer keys. Its own key, its personality.
    if !made {
        run(
            strings(&[
                &hermes,
                "profile",
                "create",
                &profile,
                "--clone-from",
                "default",
                "--no-alias",
                "--description",
                &planned.description,
            ]),
            env.clone(),
        )
        .await?;
    }
    run(strings(&[&hermes, "-p", &profile, "config", "set", "API_SERVER_KEY", &agent.hermes_key]), env.clone()).await?;
    if !install.isolated {
        // Same account as this bridge and no sandbox: nothing that reads the disk until the person turns it on.
        let mut args = strings(&[&hermes, "-p", &profile, "tools", "disable"]);
        args.extend(strings(&DISK_TOOLS));
        args.extend(strings(&["--platform", "api_server"]));
        run(args, env.clone()).await?;
    }
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join("SOUL.md"), &planned.soul)?;
    mark_as_bot(&folder)?;
    // The gateway picks new profiles up within 30 s; its permanent « Bot Chat » is opened as soon as it answers.
    wait_served(http, &agent, true).await?;
    let mut agent = with_browser(&config, agent, run).await;
    agent = mount_uploads(shared, &agent, run, false).await?;
    if shared.get().agents.contains_key(&profile) {
        save_agent(shared, &agent)?;
    } else {
        reserve_agent(shared, &agent)?;
    }
    give_tools(&shared.get(), &agent, run).await?;
    Ok(agent)
}

/// Its visible browser when the install gives one; the agent works without it (`bipbridge browser add <agent>` sets
/// it up later).
async fn with_browser(config: &Config, agent: AgentConfig, run: &Run) -> AgentConfig {
    if !config.browser.enabled {
        return agent;
    }
    match crate::browsers::setup(config, &agent, run, &install_home(config)).await {
        Ok(changed) => changed,
        Err(exc) => {
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %tail(&exc.to_string(), 200), "browser not set up");
            agent
        }
    }
}

/// Where the app reaches a new agent: through the bridge's door on a server the app created (`cloud`), else its
/// Hermes on the tailnet.
pub fn public_agent_url(config: &Config, profile: &str) -> Option<String> {
    if let Some(url) = config.public_url.as_deref().filter(|u| config.cloud && !u.is_empty()) {
        return Some(format!("{url}/hermes/{profile}"));
    }
    let hub = &config.hermes.public_hub_url;
    if hub.is_empty() {
        None
    } else {
        Some(format!("{hub}/p/{profile}"))
    }
}

pub async fn wait_served(http: &reqwest::Client, agent: &AgentConfig, bot_chat: bool) -> anyhow::Result<()> {
    let auth = format!("Bearer {}", agent.hermes_key);
    for _ in 0..90 {
        // a new profile is served at once; its key is picked up within ~30 s
        let resp = http
            .get(format!("{}/v1/capabilities", agent.hermes_url))
            .header("Authorization", &auth)
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        if let Ok(resp) = resp {
            if resp.status().as_u16() == 200 {
                if !bot_chat {
                    return Ok(());
                }
                let posted = http
                    .post(format!("{}/api/sessions", agent.hermes_url))
                    .header("Authorization", &auth)
                    .timeout(Duration::from_secs(10))
                    .json(&json!({"title": "Bot Chat"}))
                    .send()
                    .await;
                if posted.is_ok() {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    anyhow::bail!("the gateway did not pick the agent up")
}

pub static PROFILE_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").expect("valid regex"));

/// Profiles of this install's Hermes that the app cannot reach yet: created on the server (`hermes profile
/// create`, another tool) rather than from the app. Never the hub's own profile, a deleted one (Hermes leaves a
/// tombstone in profiles/.deleted/) or a folder that is not a profile.
pub fn unclaimed_profiles(config: &Config) -> Vec<Dict> {
    if !config.hermes.enabled() {
        return Vec::new();
    }
    let root = PathBuf::from(install_home(config)).join("profiles");
    let mut folders: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(entries) => entries.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => Vec::new(),
    };
    folders.sort();
    let mut found = Vec::new();
    for folder in folders {
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if !folder.is_dir()
            || !PROFILE_ID.is_match(&name)
            || config.agents.contains_key(&name)
            || root.join(".deleted").join(&name).exists()
            || !["config.yaml", "SOUL.md", "profile.yaml"].iter().any(|m| folder.join(m).is_file())
        {
            continue;
        }
        let meta = load_yaml_file(&folder.join("profile.yaml")).unwrap_or(Yaml::Null);
        let description = match meta.get("description") {
            Some(d) if d.truthy() => d.py_str(),
            _ => String::new(),
        };
        let mut entry = Dict::new();
        entry.insert("id".into(), json!(name));
        entry.insert("name".into(), json!(title(&name.replace(['-', '_'], " "))));
        entry.insert("description".into(), json!(description));
        found.push(entry);
    }
    found
}

pub fn profile_key(folder: &Path) -> Option<String> {
    let text = std::fs::read_to_string(folder.join(".env")).ok()?;
    for line in text.lines() {
        let (key, value) = line.split_once('=').unwrap_or((line, ""));
        if key.trim() == "API_SERVER_KEY" && !value.trim().is_empty() {
            return Some(value.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

/// A profile found on the server, served to the app like an agent made there: its own key (kept when it has
/// one), Bot Mode and its permanent « Bot Chat » (the app's « Discussion », where it can write to the other agents),
/// its place in this bridge's config, its visible browser when the install gives one. Its personality, tools and
/// memory stay as they are.
pub async fn claim_profile(
    shared: &SharedConfig,
    name: &str,
    display_name: Option<&str>,
    http: &reqwest::Client,
    run: &Run,
) -> anyhow::Result<AgentConfig> {
    let config = shared.get();
    if !unclaimed_profiles(&config).iter().any(|p| p.get("id").and_then(Value::as_str) == Some(name)) {
        return Err(LookupError(name.to_string()).into());
    }
    let install = &config.hermes;
    let env = hermes_env(&config);
    let folder = PathBuf::from(install_home(&config)).join("profiles").join(name);
    let key = match profile_key(&folder) {
        Some(key) if !key.is_empty() => key,
        _ => {
            let key = crate::py::token_urlsafe(32);
            run(strings(&[&install.bin, "-p", name, "config", "set", "API_SERVER_KEY", &key]), env.clone()).await?;
            key
        }
    };
    let uploads = folder.join("uploads").to_string_lossy().into_owned();
    let seen_by_agent = if install.container_home.is_empty() {
        uploads.clone()
    } else {
        format!("{}/profiles/{name}/uploads", install.container_home)
    };
    let given = display_name.unwrap_or("").trim();
    let title = if given.is_empty() { title(&name.replace(['-', '_'], " ")) } else { given.to_string() };
    let mut agent = AgentConfig {
        name: name.to_string(),
        display_name: title,
        hermes_url: format!("{}/p/{name}", install.hub_url),
        hermes_key: key,
        upload_dir_host: Some(uploads),
        upload_dir_container: Some(seen_by_agent),
        public_url: public_agent_url(&config, name),
        ..AgentConfig::default()
    };
    mark_as_bot(&folder)?;
    wait_served(http, &agent, true).await?;
    agent = with_browser(&config, agent, run).await;
    agent = mount_uploads(shared, &agent, run, false).await?;
    if let Some(path) = config.source_path.as_deref() {
        append_agent(path, &agent, &[])?;
    }
    shared.put_agent(agent.clone());
    give_tools(&shared.get(), &agent, run).await?;
    tracing::info!(target: "bipbridge::agents", agent = %name, "agent claimed");
    Ok(agent)
}

/// An agent this bridge created: in the person's hub, or moved into its own space (never one set up by hand,
/// like an agent in a container of its own).
pub fn managed(config: &Config, agent_id: &str) -> Result<AgentConfig, LookupError> {
    let agent = config.agent(Some(agent_id));
    let hub = &config.hermes.hub_url;
    match agent {
        Some(agent)
            if config.hermes.enabled()
                && (agent.space.as_deref().is_some_and(|s| !s.is_empty())
                    || agent.hermes_url.starts_with(&format!("{hub}/p/"))) =>
        {
            Ok(agent.clone())
        }
        _ => Err(LookupError(agent_id.to_string())),
    }
}

/// An agent whose personality, language and thinking the app may change: one this bridge created, or one set
/// up by hand whose HERMES_HOME it was given (`host_home`).
pub fn editable(config: &Config, agent_id: &str) -> Result<AgentConfig, LookupError> {
    if let Some(agent) = config.agent(Some(agent_id)) {
        if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
            return Ok(agent.clone());
        }
    }
    managed(config, agent_id)
}

/// The HERMES_HOME of the gateway that serves the agent: its space's, the one given for an agent set up by
/// hand, or the hub's.
pub fn home_root(config: &Config, agent: &AgentConfig) -> String {
    if let Some(space) = agent.space.as_deref().filter(|s| !s.is_empty()) {
        return space.to_string();
    }
    if let Some(home) = agent.host_home.as_deref().filter(|s| !s.is_empty()) {
        return home.to_string();
    }
    if !config.hermes.home.is_empty() {
        return config.hermes.home.clone();
    }
    crate::py::join(&crate::py::expanduser("~"), ".hermes")
}

pub fn profile_folder(config: &Config, agent: &AgentConfig) -> PathBuf {
    PathBuf::from(home_root(config, agent)).join("profiles").join(&agent.name)
}

/// What the bridge writes in an agent's section: on a rewrite (a move, a browser), these are replaced, any other
/// line of the section is kept.
pub const MANAGED_KEYS: [&str; 15] = [
    "display_name",
    "hermes_url",
    "hermes_key",
    "public_url",
    "upload_dir_host",
    "upload_dir_container",
    "isolated",
    "sandboxed",
    "python",
    "hermes_home",
    "hermes_root",
    "space",
    "desktop_url",
    "desktop_vnc",
    "desktop_vnc_password_file",
];

/// The lines of `[agents.<name>]` in the bridge's config file, its header and blank lines left out.
pub fn agent_section_lines(config_path: &str, name: &str) -> std::io::Result<Vec<String>> {
    let mut lines = Vec::new();
    let mut inside = false;
    let wanted = format!("[agents.{name}]");
    for line in std::fs::read_to_string(config_path)?.lines() {
        let header = line.trim();
        if header.starts_with('[') && header.ends_with(']') {
            inside = header == wanted;
            continue;
        }
        if inside && !header.is_empty() {
            lines.push(line.to_string());
        }
    }
    Ok(lines)
}

/// The files an agent is sent, where its tools can open them: a sandbox sees nothing of this machine but what its
/// profile mounts (`terminal.docker_volumes`), so its uploads folder is mounted there, read-only, at /fichiers, and the
/// bridge names them by that path. Agents whose tools run here, or set up by hand, are left as they are. Returns the
/// agent as the bridge now serves it (`save`: its section of the config rewritten, for an agent already there).
pub async fn mount_uploads(
    shared: &SharedConfig,
    agent: &AgentConfig,
    run: &Run,
    save: bool,
) -> anyhow::Result<AgentConfig> {
    use crate::files::{open_to_sandbox, SANDBOX_UPLOADS};
    let config = shared.get();
    let folder = crate::shares::profile_dir(&config, agent);
    let host = agent.upload_dir_host.clone().unwrap_or_default();
    let Some(folder) = folder else { return Ok(agent.clone()) };
    if agent.shares == "external"
        || host.is_empty()
        || !folder.is_dir()
        || crate::shares::terminal_backend(&folder).as_deref() != Some("docker")
    {
        return Ok(agent.clone());
    }
    let root = host.clone();
    tokio::task::spawn_blocking(move || open_to_sandbox(&root)).await??;
    let terminal = crate::shares::profile_config(&folder).get("terminal").cloned().unwrap_or(Value::Null);
    let current: Vec<String> = match terminal.get("docker_volumes") {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).collect(),
        _ => Vec::new(),
    };
    // `z`: readable by the container (SELinux)
    let volume = format!("{host}:{SANDBOX_UPLOADS}:ro,z");
    let mut wanted: Vec<String> =
        current.iter().filter(|v| v.split(':').nth(1) != Some(SANDBOX_UPLOADS)).cloned().collect();
    wanted.push(volume);
    if wanted != current {
        let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let value = crate::py::dumps(&json!(wanted));
        run(
            strings(&[&config.hermes.bin, "-p", &agent.name, "config", "set", "terminal.docker_volumes", &value]),
            env_of(&[("HERMES_HOME", &home)]),
        )
        .await?;
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, "uploads mounted in the sandbox");
    }
    let mut agent = agent.clone();
    if agent.upload_dir_container.as_deref() != Some(SANDBOX_UPLOADS) {
        agent.upload_dir_container = Some(SANDBOX_UPLOADS.to_string());
        if save {
            save_agent(shared, &agent)?;
        }
    }
    Ok(agent)
}

/// The bridge's tools (`create_audio`…) in the agent's Hermes; it works without them if that fails.
pub async fn give_tools(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<()> {
    if let Err(exc) = crate::agent_tools::register(config, agent, run).await {
        tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %tail(&exc.to_string(), 200), "agent tools not registered");
    }
    Ok(())
}

/// At start: every sandboxed agent's uploads mounted (agents made before this, or moved since).
pub async fn mount_all_uploads(config: SharedConfig, run: Run) {
    let agents: Vec<AgentConfig> = config.get().agents.values().cloned().collect();
    for agent in agents {
        if let Err(exc) = mount_uploads(&config, &agent, &run, true).await {
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %tail(&exc.to_string(), 200), "uploads not mounted");
        }
    }
}

/// The agent's section of the bridge's config file rewritten (after a move, a browser): what the bridge sets is
/// replaced, the rest of the section (voice, ntfy, shares, a key kept in a file…) stays. The running
/// configuration gets the agent too.
pub fn save_agent(config: &SharedConfig, agent: &AgentConfig) -> anyhow::Result<()> {
    if let Some(path) = config.get().source_path.as_deref() {
        let kept = agent_section_lines(path, &agent.name)?;
        remove_agent_section(path, &agent.name)?;
        append_agent(path, agent, &kept)?;
    }
    config.put_agent(agent.clone());
    Ok(())
}

/// Drops `[agents.<name>]` from the bridge's config file (its lines, up to the next table).
pub fn remove_agent_section(config_path: &str, name: &str) -> std::io::Result<()> {
    let text = std::fs::read_to_string(config_path)?;
    let wanted = format!("[agents.{name}]");
    let mut kept = String::new();
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let header = line.trim();
        if header.starts_with('[') && header.ends_with(']') {
            skipping = header == wanted;
        }
        if !skipping {
            kept.push_str(line);
        }
    }
    std::fs::write(config_path, kept.trim_end_matches('\n').to_string() + "\n")
}

pub fn read_soul(config: &Config, agent: &AgentConfig) -> std::io::Result<String> {
    let path = profile_folder(config, agent).join("SOUL.md");
    if path.exists() {
        std::fs::read_to_string(path)
    } else {
        Ok(String::new())
    }
}

/// The agent's language block rewritten (the rest of its personality untouched); returns the language kept.
pub fn set_language(config: &Config, agent: &AgentConfig, lang: &str) -> std::io::Result<String> {
    let code = crate::souls::agent_language(Some(lang));
    let soul = read_soul(config, agent)?;
    std::fs::write(profile_folder(config, agent).join("SOUL.md"), crate::souls::with_language(&soul, Some(&code)))?;
    Ok(code)
}

pub const THINKING_PROVIDER: &str = "bipagents-thinking";
pub const THINKING_LEVELS: [&str; 5] = ["none", "low", "medium", "high", "xhigh"];
/// What Hermes asks a custom server for when nothing is set.
pub const DEFAULT_THINKING: &str = "medium";

fn profile_config_path(config: &Config, agent: &AgentConfig) -> PathBuf {
    profile_folder(config, agent).join("config.yaml")
}

/// `yaml.safe_load(path.read_text()) or {}`.
pub fn load_or_empty(path: &Path) -> anyhow::Result<Yaml> {
    let data = load_yaml_file(path)?;
    Ok(if data.truthy() { data } else { Yaml::map() })
}

/// Written next to it then moved over it: never a half-written config for a gateway reading it.
pub fn replace_yaml(path: &Path, data: &Yaml) -> anyhow::Result<()> {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    std::fs::write(&temporary, pyyaml::dump(data))?;
    std::fs::set_permissions(&temporary, std::fs::metadata(path)?.permissions())?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// How much the agent thinks before answering (none … xhigh), or None when its model is not one the bridge can
/// set (an OpenAI-compatible server of the person's: provider `custom` with an address).
pub fn read_thinking(config: &Config, agent: &AgentConfig) -> Option<String> {
    let data = load_or_empty(&profile_config_path(config, agent)).ok()?;
    let model = data.get("model").filter(|m| m.is_map())?;
    if model.get("provider").and_then(Yaml::as_str) != Some("custom")
        || !model.get("base_url").is_some_and(Yaml::truthy)
    {
        return None;
    }
    let effort = data.get("agent").filter(|a| a.is_map()).and_then(|a| a.get("reasoning_effort"));
    let effort = match effort {
        None | Some(Yaml::Null) => return Some(DEFAULT_THINKING.into()),
        Some(Yaml::Str(s)) if s.is_empty() => return Some(DEFAULT_THINKING.into()),
        Some(value) => value.py_str().trim().to_lowercase(),
    };
    if ["none", "false", "disabled", "off"].contains(&effort.as_str()) {
        // YAML false reads « false »
        return Some("none".into());
    }
    Some(if THINKING_LEVELS.contains(&effort.as_str()) { effort } else { DEFAULT_THINKING.into() })
}

/// The agent's thinking level, read by Hermes from the next message (it reloads a profile's config when it
/// changes): `reasoning_effort`, which GLM-like servers follow (the model still answers an easy question at once).
/// « none » also turns the chat template's `enable_thinking` off (bound to the model's address), for servers that
/// only follow that one. Never on: it would win over a turn that asks for no thinking (Live).
pub fn set_thinking(config: &Config, agent: &AgentConfig, level: &str) -> anyhow::Result<String> {
    if !THINKING_LEVELS.contains(&level) || read_thinking(config, agent).is_none() {
        return Err(LookupError(agent.name.clone()).into());
    }
    let path = profile_config_path(config, agent);
    let mut data = load_or_empty(&path)?;
    let mut providers = data.get("providers").filter(|p| p.is_map()).cloned().unwrap_or_else(Yaml::map);
    if level == "none" {
        let api = data.get("model").and_then(|m| m.get("base_url")).cloned().unwrap_or(Yaml::Null);
        let mut kwargs = Yaml::map();
        kwargs.insert("enable_thinking", Yaml::Bool(false));
        let mut extra = Yaml::map();
        extra.insert("chat_template_kwargs", kwargs);
        let mut entry = Yaml::map();
        entry.insert("api", api);
        entry.insert("extra_body", extra);
        providers.insert(THINKING_PROVIDER, entry);
    } else {
        providers.remove(THINKING_PROVIDER);
    }
    if providers.truthy() {
        data.insert("providers", providers);
    } else {
        data.remove("providers");
    }
    let mut settings = data.get("agent").filter(|a| a.is_map()).cloned().unwrap_or_else(Yaml::map);
    settings.insert("reasoning_effort", Yaml::str(level));
    data.insert("agent", settings);
    replace_yaml(&path, &data)?;
    Ok(level.to_string())
}

/// The agent's model (its id at the router, whether it sees images), or None when the bridge can't switch it
/// (an agent set up by hand, a provider without a model id).
pub fn read_model(config: &Config, agent: &AgentConfig) -> Option<Value> {
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        return None;
    }
    let data = load_or_empty(&profile_config_path(config, agent)).ok()?;
    let model = data.get("model").filter(|m| m.is_map())?;
    let provider = model.get("provider").and_then(Yaml::as_str);
    if !matches!(provider, Some("custom") | Some("openrouter")) {
        return None;
    }
    let default = model.get("default").filter(|d| d.truthy())?;
    let vision = model.get("supports_vision").map(Yaml::truthy).unwrap_or(false);
    Some(json!({"name": default.py_str(), "vision": vision}))
}

/// Another model for this agent, from its next message, with the same router and key (the all-included offer's
/// key works with any model of its router; the person's own key with any of theirs).
pub fn set_model(config: &Config, agent: &AgentConfig, name: &str, vision: Option<bool>) -> anyhow::Result<Value> {
    if read_model(config, agent).is_none() {
        return Err(LookupError(agent.name.clone()).into());
    }
    let path = profile_config_path(config, agent);
    let mut data = load_or_empty(&path)?;
    if let Some(model) = data.get_mut("model") {
        model.insert("default", Yaml::str(name));
        if let Some(vision) = vision {
            model.insert("supports_vision", Yaml::Bool(vision));
        }
    }
    replace_yaml(&path, &data)?;
    Ok(read_model(config, agent).unwrap_or_else(|| json!({})))
}

/// Agents whose bridge-owned block (language, what they read) an earlier bridge wrote: brought up to date, the
/// personality the person wrote untouched.
pub fn heal_souls(config: &Config) {
    for name in config.agents.keys() {
        let Ok(agent) = editable(config, name) else { continue };
        let Ok(soul) = read_soul(config, &agent) else { continue };
        let (_, lang) = crate::souls::split_language(&soul);
        if let Some(lang) = lang {
            if !soul.contains(&crate::souls::language_block(Some(&lang))) {
                let _ = std::fs::write(
                    profile_folder(config, &agent).join("SOUL.md"),
                    crate::souls::with_language(&soul, Some(&lang)),
                );
            }
        }
    }
}

/// Profiles set by an earlier bridge with the chat template's thinking forced on: that wins over a turn asking for
/// none (Live), so it goes (their reasoning_effort stays).
pub fn heal_thinking(config: &Config) {
    for name in config.agents.keys() {
        let Ok(agent) = editable(config, name) else { continue };
        let Some(level) = read_thinking(config, &agent).filter(|l| l != "none") else { continue };
        let Ok(data) = load_or_empty(&profile_config_path(config, &agent)) else { continue };
        if data.get("providers").and_then(|p| p.get(THINKING_PROVIDER)).is_some() {
            let _ = set_thinking(config, &agent, &level);
        }
    }
}

/// The settings page of an agent. `GatewayError` (Python's `httpx.HTTPError`) when its gateway does not answer.
pub async fn read_settings(config: &Config, agent: &AgentConfig, http: &reqwest::Client) -> anyhow::Result<Value> {
    // The language block is the bridge's: shown and changed as « language », never in the editable personality.
    let (soul, lang) = crate::souls::split_language(&read_soul(config, agent)?);
    let (soul, _) = crate::souls::split_health(&soul);
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        // set up by hand: its tools are its administrator's
        return Ok(json!({"id": agent.name, "name": agent.display_name, "soul": soul.trim(), "language": lang,
                         "thinking": read_thinking(config, agent), "model": null, "by_hand": true, "toolsets": [],
                         "isolated": agent.isolated,
                         "protected": true, "services": []}));
    }
    let resp = http
        .get(format!("{}/v1/toolsets", agent.hermes_url))
        .header("Authorization", format!("Bearer {}", agent.hermes_key))
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| GatewayError(e.to_string()))?;
    if !resp.status().is_success() {
        return Err(GatewayError(format!("status {}", resp.status().as_u16())).into());
    }
    let bytes = resp.bytes().await.map_err(|e| GatewayError(e.to_string()))?;
    let answer: Value = serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("JSONDecodeError: {e}"))?;
    let Value::Object(answer) = answer else { anyhow::bail!("AttributeError: 'list' object has no attribute 'get'") };
    let toolsets: Vec<Value> = match answer.get("data") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_object)
            .filter(|t| t.get("name").is_some_and(crate::py::truthy))
            .map(|t| {
                let mut out = Dict::new();
                for k in ["name", "label", "description", "enabled", "configured"] {
                    out.insert(k.into(), t.get(k).cloned().unwrap_or(Value::Null));
                }
                Value::Object(out)
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(json!({"id": agent.name, "name": agent.display_name, "soul": soul.trim(), "language": lang,
              "thinking": read_thinking(config, agent), "model": read_model(config, agent), "toolsets": toolsets,
              "isolated": agent.isolated,
              "protected": config.hermes.isolated,
              "services": crate::services::listing(config, agent), "skills": crate::own_skills::listing(&profile_folder(config, agent)),
              "browser": browser_settings(config, agent),
              "health": {"on": crate::health::enabled(config, agent.name.as_str()),
                         "categories": crate::health::categories(config, agent.name.as_str())},
              "weather": {"on": crate::weather::enabled(config, agent.name.as_str())},
              "group": crate::agent_groups::group_of(config, &agent.name)}))
}

/// « Naviguer à ta place »: its own browser, on or not (`ready`: set up before, turning it on is instant); None
/// when this install can't give it one (the app shows Hermes' own browser tool instead).
pub fn browser_settings(config: &Config, agent: &AgentConfig) -> Option<Value> {
    let on = crate::browsers::state(&home_root(config, agent), &agent.name);
    if on.is_none() && !crate::browsers::available(config) {
        return None;
    }
    Some(json!({"on": on == Some(true), "ready": on.is_some()}))
}

pub async fn write_settings(
    shared: &SharedConfig,
    agent: &AgentConfig,
    body: &AgentSettings,
    run: &Run,
) -> anyhow::Result<()> {
    let config = shared.get();
    let toolsets = lax_map(&body.toolsets);
    let skills = lax_map(&body.skills);
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty())
        && (!toolsets.is_empty()
            || !skills.is_empty()
            || body.browser.is_some()
            || body.health.is_some()
            || body.health_categories.is_some()
            || body.weather.is_some())
    {
        return Err(AgentError("the tools of an agent set up by hand are changed on its server".into()).into());
    }
    if !skills.is_empty() {
        if let Err(exc) = crate::own_skills::set_enabled(&profile_folder(&config, agent), &skills) {
            return Err(match exc.downcast_ref::<LookupError>() {
                Some(e) => AgentError(format!("not one of the agent's own skills: {e}")).into(),
                None => exc,
            });
        }
    }
    if let Some(on) = body.browser {
        let changed = match crate::browsers::switch(&config, agent, on, run, &home_root(&config, agent)).await {
            Ok(changed) => changed,
            Err(exc) => {
                return Err(match exc.downcast_ref::<crate::browsers::BrowserError>() {
                    Some(e) => AgentError(e.to_string()).into(),
                    None => exc,
                })
            }
        };
        if on {
            save_agent(shared, &changed)?; // set up the first time: its desktop in its section
        }
    }
    if let Some(soul) = &body.soul {
        // The personality the person edited, with the language block it had (they never see it).
        let (_, lang) = crate::souls::split_language(&read_soul(&config, agent)?);
        let text = format!("{}\n", soul.trim());
        let text = match lang.as_deref() {
            Some(lang) if !lang.is_empty() => crate::souls::with_language(&text, Some(lang)),
            _ => text,
        };
        let text = crate::souls::with_health(&text, crate::health::enabled(&config, &agent.name));
        std::fs::write(profile_folder(&config, agent).join("SOUL.md"), text)?;
    }
    if let Some(on) = body.health {
        set_health(&config, agent, on, run).await?;
    }
    if let Some(categories) = &body.health_categories {
        set_health_categories(&config, agent, categories, run).await?;
    }
    if let Some(on) = body.weather {
        if crate::weather::set_enabled(&config, &agent.name, on)? {
            // Its tools' entry changes (`weather` listed or not): its Hermes fetches the list again.
            if let Err(exc) = crate::agent_tools::register(&config, agent, run).await {
                let error = tail(&exc.to_string(), 200);
                tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %error, "agent tools not registered");
            }
        }
    }
    let env = env_of(&[("HERMES_HOME", &home_root(&config, agent))]);
    for action in ["enable", "disable"] {
        let mut names: Vec<&String> =
            toolsets.iter().filter(|(_, on)| **on == (action == "enable")).map(|(n, _)| n).collect();
        names.sort();
        if names.iter().any(|n| !crate::files::py_isalnum(&n.replace(['-', '_'], ""))) {
            return Err(AgentError("invalid toolset name".into()).into());
        }
        if !names.is_empty() {
            let mut args = strings(&[&config.hermes.bin, "-p", &agent.name, "tools", action]);
            args.extend(names.into_iter().cloned());
            args.extend(strings(&["--platform", "api_server"]));
            run(args, env.clone()).await?;
        }
    }
    Ok(())
}

/// « Santé » on or off: the switch, the rule in its personality, and its tools' entry (the `health` tool listed or
/// not: a changed entry makes its Hermes fetch the list again).
pub async fn set_health(config: &Config, agent: &AgentConfig, on: bool, run: &Run) -> anyhow::Result<()> {
    crate::health::set_enabled(config, &agent.name, on)?;
    let soul = read_soul(config, agent)?;
    let changed = crate::souls::with_health(&soul, on);
    if changed != soul {
        std::fs::write(profile_folder(config, agent).join("SOUL.md"), changed)?;
    }
    if let Err(exc) = crate::agent_tools::register(config, agent, run).await {
        let error = tail(&exc.to_string(), 200);
        tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %error, "agent tools not registered");
    }
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, on = on, "health switched");
    Ok(())
}

/// What `agent` reads of the person's health data (its switch on): its tools' entry changes with it (the tool's
/// description and choices list the categories).
pub async fn set_health_categories(
    config: &Config,
    agent: &AgentConfig,
    categories: &[String],
    run: &Run,
) -> anyhow::Result<()> {
    let Some(now) = crate::health::set_categories(config, &agent.name, categories)? else {
        return Err(AgentError("« Santé » is off for this agent".into()).into());
    };
    if let Err(exc) = crate::agent_tools::register(config, agent, run).await {
        let error = tail(&exc.to_string(), 200);
        tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %error, "agent tools not registered");
    }
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, categories = %now.join(","), "health categories set");
    Ok(())
}

/// A deleted agent: its profile (or space), browser, shares and tokens gone; out of the configuration. (Its vault
/// handles go in the route, as in Python.)
pub async fn delete_agent(shared: &SharedConfig, agent: &AgentConfig, run: &Run) -> anyhow::Result<()> {
    let config = shared.get();
    if agent.space.as_deref().is_some_and(|s| !s.is_empty()) {
        crate::spaces::Spaces::new(run.clone()).remove(&config, agent).await?;
    } else {
        run(strings(&[&config.hermes.bin, "profile", "delete", &agent.name, "--yes"]), hermes_env(&config)).await?;
    }
    if crate::browsers::folder(&config, &agent.name).exists() {
        crate::browsers::remove(&config, &agent.name, run).await?;
    }
    if let Some(path) = config.source_path.as_deref() {
        remove_agent_section(path, &agent.name)?;
    }
    shared.remove_agent(&agent.name);
    let config = shared.get();
    crate::shares::forget(&config, &agent.name, run).await?;
    crate::agent_tools::forget(&config, &agent.name);
    crate::health::forget(&config, &agent.name);
    crate::weather::forget(&config, &agent.name);
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, "agent deleted");
    Ok(())
}

// -- routes ---------------------------------------------------------------------------------------------------------

/// One creation (or change) at a time: profile names, the config file.
static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// A failure of Hermes (Python's `RuntimeError`, `asyncio.TimeoutError`) → `status` with `detail`; an `OSError`
/// (which the Python does not catch) → 500.
fn hermes_failed(exc: anyhow::Error, status: u16, detail: &str) -> HttpError {
    if is_os_error(&exc) {
        HttpError::from(exc)
    } else {
        HttpError::new(status, detail)
    }
}

fn not_managed() -> HttpError {
    HttpError::new(404, "not an agent this install manages")
}

/// `await request.json()`: a body that is not JSON is an uncaught error (500).
async fn request_json(request: Request) -> ApiResult<Value> {
    let bytes = axum::body::to_bytes(request.into_body(), usize::MAX)
        .await
        .map_err(|_| HttpError::new(400, "There was an error parsing the body"))?;
    serde_json::from_slice(&bytes).map_err(|e| HttpError::from(anyhow::anyhow!("JSONDecodeError: {e}")))
}

/// `[(str(x.get(a, "")).lower(), str(x.get(b, "")).lower()) for x in body.get(list_key, []) if isinstance(x, dict)]`.
fn pairs_of(body: &Value, list_key: &str, a: &str, b: &str) -> ApiResult<Vec<(String, String)>> {
    let Value::Object(body) = body else {
        return Err(HttpError::from(anyhow::anyhow!("AttributeError: object has no attribute 'get'")));
    };
    let items = match body.get(list_key) {
        None => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(Value::String(_)) | Some(Value::Object(_)) => return Ok(Vec::new()),
        Some(_) => return Err(HttpError::from(anyhow::anyhow!("TypeError: object is not iterable"))),
    };
    let text = |x: &Dict, k: &str| x.get(k).map(crate::py::py_str).unwrap_or_default().to_lowercase();
    Ok(items.iter().filter_map(Value::as_object).map(|x| (text(x, a), text(x, b))).collect())
}

fn links_json(links: &Dict) -> Vec<Value> {
    match links.get("links") {
        Some(Value::Array(items)) => items
            .iter()
            .map(|pair| json!({"from": pair.get(0).cloned().unwrap_or(Value::Null), "to": pair.get(1).cloned().unwrap_or(Value::Null)}))
            .collect(),
        _ => Vec::new(),
    }
}

fn created(config: &Config, agent: &AgentConfig) -> Value {
    let has_url = agent.public_url.as_deref().is_some_and(|u| !u.is_empty());
    json!({"id": agent.name, "name": agent.display_name, "url": agent.public_url,
           "key": if has_url { Some(agent.hermes_key.clone()) } else { None },
           "group": crate::agent_groups::group_of(config, &agent.name)})
}

async fn kinds(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let lang = web::query_str(&q, "lang", false, None)?.filter(|l| !l.is_empty()).unwrap_or_else(|| "fr".into());
    let kinds: Vec<Value> = crate::souls::templates(Some(&lang))
        .iter()
        .map(|t| {
            json!({"id": t.get("category"), "label": t.get("label"), "description": t.get("description"),
                   "custom": !t.get("soul").is_some_and(crate::py::truthy)})
        })
        .collect();
    Ok(web::ok(json!({"can_create": config.hermes.enabled(), "kinds": kinds})))
}

async fn new_agent(State(state): State<AppState>, Json(body): Json<NewAgent>) -> ApiResult<Response> {
    if !state.config().hermes.enabled() {
        return Err(HttpError::new(409, "this install does not manage its agents (set up by hand)"));
    }
    let _guard = LOCK.lock().await;
    let agent = match create_agent(&state.config, &body, &state.http, &default_run()).await {
        Ok(agent) => agent,
        Err(exc) => {
            if let Some(e) = exc.downcast_ref::<AgentError>() {
                return Err(HttpError::new(400, e.0.clone()));
            }
            if is_os_error(&exc) {
                return Err(exc.into());
            }
            tracing::warn!(target: "bipbridge::agents", error = %head(&exc.to_string(), 200), "agent creation failed");
            return Err(HttpError::new(502, "Hermes could not create the agent"));
        }
    };
    Ok(web::json_response(201, &created(&state.config(), &agent)))
}

/// A profile created on the server (listed under `unclaimed` by GET /v1/agents), served to the app.
async fn claim(State(state): State<AppState>, Json(body): Json<Claim>) -> ApiResult<Response> {
    let _guard = LOCK.lock().await;
    let agent = match claim_profile(&state.config, &body.id, body.name.as_deref(), &state.http, &default_run()).await {
        Ok(agent) => agent,
        Err(exc) => {
            if exc.downcast_ref::<LookupError>().is_some() {
                return Err(HttpError::new(404, "no such profile waiting on the server"));
            }
            if is_os_error(&exc) {
                return Err(exc.into());
            }
            tracing::warn!(target: "bipbridge::agents", agent = %body.id, error = %tail(&exc.to_string(), 200), "claim failed");
            return Err(HttpError::new(502, "Hermes did not serve the agent"));
        }
    };
    Ok(web::json_response(201, &created(&state.config(), &agent)))
}

async fn get_settings(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    match read_settings(&config, &agent, &state.http).await {
        Ok(settings) => Ok(web::ok(settings)),
        Err(exc) if exc.downcast_ref::<GatewayError>().is_some() => Err(HttpError::new(502, "Hermes unreachable")),
        Err(exc) => Err(exc.into()),
    }
}

/// The agent's language, set in the app: it writes first in it (and answers in whatever it is written to).
async fn put_language(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentLanguage>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    let _guard = LOCK.lock().await;
    let code = set_language(&config, &agent, &body.lang).map_err(anyhow::Error::from)?;
    Ok(web::ok(json!({"id": agent.name, "language": code})))
}

/// How much the agent thinks before answering (null: its model can't be set from here).
async fn get_thinking(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    Ok(web::ok(json!({"id": agent.name, "thinking": read_thinking(&config, &agent)})))
}

/// The thinking level, from the app, between two messages.
async fn put_thinking(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentThinking>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    let _guard = LOCK.lock().await;
    match set_thinking(&config, &agent, &body.level) {
        Ok(level) => Ok(web::ok(json!({"id": agent.name, "thinking": level}))),
        Err(exc) if exc.downcast_ref::<LookupError>().is_some() => {
            Err(HttpError::new(409, "this agent's model can't be switched from here"))
        }
        Err(exc) => Err(exc.into()),
    }
}

/// Another model for the agent, from the app, between two messages.
async fn put_model(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentModel>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    let model = {
        let _guard = LOCK.lock().await;
        match set_model(&config, &agent, &body.name, body.vision) {
            Ok(model) => model,
            Err(exc) if exc.downcast_ref::<LookupError>().is_some() => {
                return Err(HttpError::new(409, "this agent's model can't be switched from here"))
            }
            Err(exc) => return Err(exc.into()),
        }
    };
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, model = %body.name, "agent model changed");
    Ok(web::ok(json!({"id": agent.name, "model": model})))
}

async fn patch_settings(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentSettings>,
) -> ApiResult<Response> {
    let agent = editable(&state.config(), &agent_id).map_err(|_| not_managed())?;
    {
        let _guard = LOCK.lock().await;
        if let Err(exc) = write_settings(&state.config, &agent, &body, &default_run()).await {
            if let Some(e) = exc.downcast_ref::<AgentError>() {
                return Err(HttpError::new(400, e.0.clone()));
            }
            return Err(hermes_failed(exc, 502, "Hermes could not change the agent"));
        }
    }
    match read_settings(&state.config(), &agent, &state.http).await {
        Ok(settings) => Ok(web::ok(settings)),
        Err(exc) if exc.downcast_ref::<GatewayError>().is_some() => {
            Ok(web::ok(json!({"id": agent.name, "name": agent.display_name})))
        }
        Err(exc) => Err(exc.into()),
    }
}

async fn agent_links(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let mut agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| json!({"id": a.name, "name": a.display_name, "managed": crate::peers::managed_home(&config, a).is_some()}))
        .collect();
    // The agents of the linked households, as their bridges list them (a household that does not answer is
    // left out for now; its links stay).
    for household in state.households.public() {
        let id = household.get("id").map(crate::py::py_str).unwrap_or_default();
        let hname = household.get("name").map(crate::py::py_str).unwrap_or_default();
        let entry = state.households.get(&id).unwrap_or_default();
        let url = entry.get("url").map(crate::py::py_str).unwrap_or_default();
        let key_out = entry.get("key_out").map(crate::py::py_str).unwrap_or_default();
        let listed: Vec<Value> = match state
            .http
            .get(format!("{url}/family/agents"))
            .timeout(Duration::from_secs(4))
            .header("Authorization", format!("Bearer {key_out}"))
            .send()
            .await
        {
            Ok(resp) if resp.status().as_u16() == 200 => {
                match resp.bytes().await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
                    Some(Value::Object(answer)) => match answer.get("agents") {
                        Some(Value::Array(items)) => items.clone(),
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        };
        for remote in listed {
            let rid = remote.get("id").map(crate::py::py_str).unwrap_or_default();
            let rname = remote.get("name").map(crate::py::py_str).unwrap_or_default();
            agents.push(
                json!({"id": format!("{rid}{}{id}", crate::peers::SEPARATOR), "name": format!("{rname} ({hname})"),
                               "managed": false, "household": id}),
            );
        }
    }
    Ok(web::ok(json!({"agents": agents, "links": links_json(&state.links.load())})))
}

async fn set_agent_links(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let wanted = pairs_of(&body, "links", "from", "to")?;
    let result = {
        let _guard = LOCK.lock().await;
        match crate::peers::apply(&state.config(), &state.links, &wanted, &default_run(), Some(&state.households)).await
        {
            Ok(result) => result,
            Err(exc) => {
                if let Some(e) = exc.downcast_ref::<crate::peers::InvalidLink>() {
                    return Err(HttpError::new(400, e.0.clone()));
                }
                return Err(hermes_failed(exc, 502, "Hermes could not be set up"));
            }
        }
    };
    // Agents set up by hand need their token added by the administrator: say which, never the token itself.
    let mut manual: Vec<String> = match result.get("manual") {
        Some(Value::Object(m)) => m.keys().cloned().collect(),
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).collect(),
        _ => Vec::new(),
    };
    manual.sort();
    Ok(web::ok(json!({"links": links_json(&state.links.load()), "needs_setup": manual})))
}

/// Who may read what another agent shares (read-only), and which agents can take part.
async fn agent_shares(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| json!({"id": a.name, "name": a.display_name, "why_not": crate::shares::why_not(&config, a), "external": a.shares == "external"}))
        .collect();
    let reads: Vec<Value> =
        crate::shares::load(&config).into_iter().map(|(r, s)| json!({"reader": r, "source": s})).collect();
    Ok(web::ok(json!({"agents": agents, "reads": reads})))
}

async fn set_agent_shares(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    let body = request_json(request).await?;
    let wanted = pairs_of(&body, "reads", "reader", "source")?;
    let _guard = LOCK.lock().await;
    match crate::shares::apply(&state.config(), &wanted, &default_run()).await {
        Ok(reads) => Ok(web::ok(
            json!({"reads": reads.into_iter().map(|(r, s)| json!({"reader": r, "source": s})).collect::<Vec<_>>()}),
        )),
        Err(exc) => {
            if let Some(e) = exc.downcast_ref::<crate::shares::InvalidShare>() {
                return Err(HttpError::new(400, e.0.clone()));
            }
            Err(hermes_failed(exc, 502, "Hermes could not be set up"))
        }
    }
}

async fn tool_catalog() -> ApiResult<Response> {
    let services: Vec<Value> = crate::services::catalog()
        .iter()
        .map(|item| {
            let mut out = Dict::new();
            for k in ["id", "label", "description", "icon", "personal", "fields"] {
                out.insert(k.into(), item.get(k).cloned().unwrap_or(Value::Null));
            }
            Value::Object(out)
        })
        .collect();
    Ok(web::ok(json!({"services": services})))
}

fn managed_or_404(state: &AppState, agent_id: &str) -> ApiResult<AgentConfig> {
    managed(&state.config(), agent_id).map_err(|_| not_managed())
}

async fn connect_service(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    request: Request,
) -> ApiResult<Response> {
    let agent = managed_or_404(&state, &agent_id)?;
    let body = request_json(request).await?;
    let Value::Object(body) = body else {
        return Err(HttpError::from(anyhow::anyhow!("AttributeError: object has no attribute 'get'")));
    };
    match crate::services::add(&state.config(), &agent, &body, &default_run()).await {
        Ok(result) => Ok(web::json_response(201, &Value::Object(result))),
        Err(exc) => {
            if let Some(crate::vault::VaultError::CannotHold(reasons)) = exc.downcast_ref::<crate::vault::VaultError>()
            {
                return Err(HttpError::json(409, json!({"code": "cannot_hold", "agents": reasons})));
            }
            if let Some(e) = exc.downcast_ref::<crate::services::ServiceError>() {
                return Err(HttpError::new(400, e.0.clone()));
            }
            Err(hermes_failed(exc, 502, "Hermes could not add the service"))
        }
    }
}

async fn disconnect_service(
    State(state): State<AppState>,
    UrlPath((agent_id, name)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = managed_or_404(&state, &agent_id)?;
    match crate::services::remove(&state.config(), &agent, &name, &default_run()).await {
        Ok(()) => Ok(web::no_content()),
        Err(exc) if exc.downcast_ref::<crate::services::UnknownService>().is_some() => {
            Err(HttpError::new(404, "no such service"))
        }
        Err(exc) => Err(hermes_failed(exc, 502, "Hermes could not remove the service")),
    }
}

async fn test_service(
    State(state): State<AppState>,
    UrlPath((agent_id, name)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let agent = managed_or_404(&state, &agent_id)?;
    match crate::services::test(&state.config(), &agent, &name, &default_run()).await {
        Ok(result) => Ok(web::ok(Value::Object(result))),
        Err(exc) if exc.downcast_ref::<crate::services::UnknownService>().is_some() => {
            Err(HttpError::new(404, "no such service"))
        }
        Err(exc) if is_os_error(&exc) => Err(exc.into()),
        Err(exc) => Ok(web::ok(json!({"ok": false, "message": tail(&exc.to_string(), 300), "tools": null}))),
    }
}

/// Moves an agent of the shared hub into its own space (so it may hold credentials). Takes up to two minutes.
async fn own_space(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let agent = managed_or_404(&state, &agent_id)?;
    let _guard = LOCK.lock().await;
    let run = default_run();
    let moved = match crate::spaces::Spaces::new(run.clone()).move_agent(&state.config(), &agent, &state.http).await {
        Ok(moved) => moved,
        Err(exc) => {
            if let Some(e) = exc.downcast_ref::<crate::spaces::SpaceError>() {
                return Err(HttpError::new(409, e.0.clone()));
            }
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %tail(&exc.to_string(), 200), "move into a space failed");
            return Err(HttpError::new(502, "the space could not be created"));
        }
    };
    save_agent(&state.config, &moved)?;
    give_tools(&state.config(), &moved, &run).await?; // its profile is a new one, in its space
    let has_url = moved.public_url.as_deref().is_some_and(|u| !u.is_empty());
    Ok(web::ok(json!({"id": moved.name, "name": moved.display_name, "url": moved.public_url,
                      "key": if has_url { Some(moved.hermes_key.clone()) } else { None }, "isolated": true})))
}

async fn remove(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    remove_agent(state, agent_id).await
}

/// `DELETE /agents/claim`: Starlette falls through `POST /agents/claim` to `DELETE /agents/{agent_id}`.
async fn remove_claim(State(state): State<AppState>) -> ApiResult<Response> {
    remove_agent(state, "claim".into()).await
}

async fn remove_agent(state: AppState, agent_id: String) -> ApiResult<Response> {
    let agent = managed_or_404(&state, &agent_id)?;
    let _guard = LOCK.lock().await;
    let result = async {
        delete_agent(&state.config, &agent, &default_run()).await?;
        crate::agent_groups::set_group(&state.config(), &agent.name, "")?;
        state.vault.forget_agent(&state.config(), &agent.name).await
    }
    .await;
    match result {
        Ok(()) => Ok(web::no_content()),
        Err(exc) => Err(hermes_failed(exc, 502, "Hermes could not delete the agent")),
    }
}

/// `agents_admin_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agent-kinds", get(kinds))
        .route("/agents", post(new_agent))
        .route("/agents/claim", post(claim).delete(remove_claim))
        .route("/agents/{agent_id}/settings", get(get_settings).patch(patch_settings))
        .route("/agents/{agent_id}/language", put(put_language))
        .route("/agents/{agent_id}/thinking", get(get_thinking).put(put_thinking))
        .route("/agents/{agent_id}/model", put(put_model))
        .route("/agent-links", get(agent_links).put(set_agent_links))
        .route("/agent-shares", get(agent_shares).put(set_agent_shares))
        .route("/tool-catalog", get(tool_catalog))
        .route("/agents/{agent_id}/services", post(connect_service))
        .route("/agents/{agent_id}/services/{name}", axum::routing::delete(disconnect_service))
        .route("/agents/{agent_id}/services/{name}/test", post(test_service))
        .route("/agents/{agent_id}/space", post(own_space))
        .route("/agents/{agent_id}", axum::routing::delete(remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_tails() {
        assert_eq!(title("mon jardin"), "Mon Jardin");
        assert_eq!(title("a1b cÉ"), "A1B Cé");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
        assert_eq!(line_key("  voice = \"x\""), "voice");
        assert_eq!(line_key("# a = b"), "");
        assert_eq!(q("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
