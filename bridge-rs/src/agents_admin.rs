//! Agents created from the app (« + »): on an install made by the installer, the bridge manages the person's own Hermes
//! (`[hermes]` in its config) and creates a profile for each new agent — the kind or the « Autre » description the person
//! chose, the name they gave — then serves it at once, without a restart. Everything runs as the person's own account.
//!
//! Routes (bridge key):
//! - `GET /v1/agent-kinds`: the kinds of agents offered, and whether this install can create agents.
//! - `POST /v1/agents`: `{"name", "kind", "lang"}` or `{"name", "custom": {"role", "tone", "rules", "category"}, "lang"}`
//!   → the new agent as `GET /v1/agents` lists it (id, name, tailnet url, key).
//! - `PUT /v1/agents/{id}/language`: `{"lang"}` — the language it writes first in (its SOUL.md's language block).

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
use futures::StreamExt;
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

/// `run_hermes` as a [`Run`], the commands that write a profile under its lock (profile_files::locked).
pub fn default_run() -> Run {
    crate::profile_files::locked(Arc::new(|args, env| Box::pin(run_hermes(args, env))), run_hermes_blocking)
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

/// The agents of a place of the all-included offer at most, when its bridge.toml says nothing (`[place] agents_max`).
/// Alex, 2026-10-30: « Mettons une limite de nombre d'agents, sans le dire. Pour éviter les abus. Au-delà de 20 ça n'a
/// aucun sens dans tous les cas. » Never shown nor told (no counter, no number in the apps, the souls or the tools'
/// descriptions): only a creation past it is refused.
pub const OFFER_AGENTS_MAX: usize = 20;

/// The code of the refusal (409 `{"detail": {"code": "too_many_agents"}}`): the apps say it in plain words, no number.
pub const TOO_MANY_AGENTS: &str = "too_many_agents";

/// What a model's tool says when the place is full (a tool error, nothing in the souls: no word on every call).
pub const TOO_MANY_AGENTS_TOOL: &str =
    "This place already has many agents: suggest reusing or removing one the person no longer uses; don't create another.";

/// A new agent refused: the place already has its agents at most (`agents_max`). Routes answer 409 with
/// `TOO_MANY_AGENTS` (web.rs turns it into that answer wherever it ends up).
#[derive(Debug, Clone, thiserror::Error)]
#[error("this place already has many agents")]
pub struct TooManyAgents;

/// The place's agents at most (None: no limit): `[place] agents_max` when written (0: none), else
/// `OFFER_AGENTS_MAX` on a place of the offer (its guichet token, handover::on_offer) and no limit on a self-install.
pub fn agents_max(config: &Config) -> Option<usize> {
    agents_max_on(config, crate::handover::on_offer())
}

/// `agents_max`, with whether this is a place of the offer given.
pub fn agents_max_on(config: &Config, offer: bool) -> Option<usize> {
    match config.place_agents_max {
        Some(0) => None,
        Some(most) => Some(most),
        None => offer.then_some(OFFER_AGENTS_MAX),
    }
}

/// Every agent the place runs counts: the person's own, the guide, the project agents, the family's agent, and the
/// one prepared at install (it becomes the first agent). Agents of relatives shared with this person live on their
/// places, not in this bridge's list, so they don't.
pub fn agents_count(config: &Config) -> usize {
    config.agents.len()
}

/// Whether `more` new agents fit on the place. Agents already above the limit (none expected) are kept: only new
/// ones are refused.
pub fn room_for(config: &Config, more: usize) -> bool {
    room_for_on(config, more, crate::handover::on_offer())
}

pub fn room_for_on(config: &Config, more: usize, offer: bool) -> bool {
    agents_max_on(config, offer).is_none_or(|most| agents_count(config) + more <= most)
}

/// Err(TooManyAgents) when one more agent doesn't fit.
pub fn check_room(config: &Config) -> Result<(), TooManyAgents> {
    if room_for(config, 1) {
        Ok(())
    } else {
        tracing::info!(target: "bipbridge::agents", agents = agents_count(config), "new agent refused: the place is full");
        Err(TooManyAgents)
    }
}

impl From<TooManyAgents> for HttpError {
    fn from(refused: TooManyAgents) -> Self {
        anyhow::Error::from(refused).into()
    }
}

/// Whether a creation failed for that reason.
pub fn too_many(exc: &anyhow::Error) -> bool {
    exc.downcast_ref::<TooManyAgents>().is_some()
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
    /// Empty: the agent asks on its first open, then proposes one (guide.rs).
    #[serde(default)]
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

#[derive(Debug, Clone, Default, Deserialize)]
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
    /// A kind's agent: what the person expects from it and its tone, both optional (added to its personality).
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tone: Option<String>,
}

impl Validate for NewAgent {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "name", Some(&self.name), 1);
        check_max_len(errors, "name", Some(&self.name), 40);
        check_max_len(errors, "group", self.group.as_deref().map(str::trim), crate::agent_groups::MAX_LEN);
        check_max_len(errors, "role", self.role.as_deref(), 600);
        check_max_len(errors, "tone", self.tone.as_deref(), 200);
        if let Some(c) = &self.custom {
            nested(errors, "custom", |errors| {
                if !c.role.is_empty() {
                    // optional (the agent asks), but never a word or two
                    check_min_len(errors, "role", Some(&c.role), 3);
                }
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

const THINKING_PATTERN: &str = "^(auto|none|low|medium|high|xhigh)$";
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
    /// Its name, renamed in an app (guide::rename: its section and its personality); the apps keep no copy of it.
    #[serde(default)]
    pub name: Option<String>,
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
    /// « Carnet »: the notebook tools and their rule (notebook/), on by default.
    #[serde(default, deserialize_with = "web::lax::opt_bool")]
    pub notebook: Option<bool>,
    /// How it answers (answer_style.rs): short, balanced, detailed or custom, and the person's own style.
    #[serde(default)]
    pub answer_style: Option<String>,
    #[serde(default)]
    pub answer_style_custom: Option<String>,
}

impl Validate for AgentSettings {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "soul", self.soul.as_deref(), 8000);
        check_max_len(errors, "name", self.name.as_deref(), 40);
        check_max_len(errors, "answer_style_custom", self.answer_style_custom.as_deref(), crate::souls::CUSTOM_STYLE_MAX);
        if let Some(style) = self.answer_style.as_deref().filter(|s| !crate::souls::ANSWER_STYLES.contains(s)) {
            errors.push(FieldError::new(
                "value_error",
                web::body_loc("answer_style"),
                format!("Value error, one of {}", crate::souls::ANSWER_STYLES.join(", ")),
                json!(style),
            ));
        }
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
        anyhow::bail!("{shown} failed: {}", why_failed(&output.stdout, &output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// What a failed command said: its error output, else the error lines of its output (Hermes' CLI prints its
/// « Error: … » there, e.g. `hermes profile delete`: the move of 2026-11-07 on the token lab logged « failed: » and
/// nothing else). Only those lines: the rest of a command's output may hold what it was given (a key).
pub fn why_failed(stdout: &[u8], stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    if !err.trim().is_empty() {
        return tail(err.trim(), 300);
    }
    let out = String::from_utf8_lossy(stdout);
    let errors: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("Error") || l.starts_with('⚠') || l.starts_with("✗"))
        .collect();
    if errors.is_empty() {
        "(no error output)".to_string()
    } else {
        tail(&errors.join(" "), 300)
    }
}

/// [`run_hermes`] on a blocking thread (the commands that write a profile, under its lock: profile_files::hermes).
pub fn run_hermes_blocking(args: &[String], env: &BTreeMap<String, String>) -> anyhow::Result<String> {
    use std::io::Read;
    let Some(program) = args.first() else { anyhow::bail!("no command") };
    let mut child = std::process::Command::new(program)
        .args(&args[1..])
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let pipe = |mut from: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = from.read_to_end(&mut bytes);
            bytes
        })
    };
    let stdout = pipe(Box::new(child.stdout.take().expect("piped")));
    let stderr = pipe(Box::new(child.stderr.take().expect("piped")));
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CommandTimeout.into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (stdout, stderr) = (stdout.join().unwrap_or_default(), stderr.join().unwrap_or_default());
    if !status.success() {
        let shown = args.iter().take(4).cloned().collect::<Vec<_>>().join(" ");
        anyhow::bail!("{shown} failed: {}", why_failed(&stdout, &stderr));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

/// Tools that read this machine's files: off by default where the agents are not isolated from the bridge. The
/// browser too: Hermes' local Chromium opens file:// pages (checked: it read a file next to the bridge's keys).
pub const DISK_TOOLS: [&str; 4] = ["terminal", "file", "code_execution", "browser"];

fn load_yaml_file(path: &Path) -> anyhow::Result<Yaml> {
    let text = std::fs::read_to_string(path)?;
    Ok(pyyaml::load(&text)?)
}

/// Hermes' Bot Mode mark on a profile (`ui_meta.hermes-bots`): only for an agent that writes to another household
/// (peers.rs `mark_bots`).
pub fn mark_as_bot(folder: &Path) -> anyhow::Result<()> {
    crate::profile_files::try_edit(&folder.join("profile.yaml"), |data| {
        let meta = data.entry_map("ui_meta").filter(|m| m.is_map());
        match meta.and_then(|m| m.entry_map("hermes-bots")) {
            Some(_) => Ok(()),
            None => anyhow::bail!("AttributeError: 'ui_meta' is not a dict"),
        }
    })?;
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
/// section to keep (what the bridge does not set itself: its voice, shares…).
pub fn append_agent(config_path: &str, agent: &AgentConfig, kept: &[String]) -> std::io::Result<()> {
    crate::config::update_source(config_path, |text| Ok(with_agent(text, agent, kept)))
}

/// The config's text with the agent's section added at its end (the whole file written again by the caller, never
/// appended in place: a reader never sees half a section).
fn with_agent(before: &str, agent: &AgentConfig, kept: &[String]) -> String {
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
    before.to_string() + &lines.join("\n") + "\n"
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
            // the place's language when set (its person's, from the claim: cloud_setup.rs), else French as before
            None => crate::notebook::place_language(config).unwrap_or_else(|| "fr".into()),
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
        let soul = crate::souls::with_expectations(&soul, body.role.as_deref(), body.tone.as_deref(), Some(&lang));
        let description = match body.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(role) => head(role, 120),
            None => found.get("description").map(crate::py::py_str).unwrap_or_default(),
        };
        (soul, description)
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
    check_room(&config)?;
    let mut planned = plan_agent(&config, body)?;
    // a spare made ahead (agent_pool.rs): files only, no `hermes` command, no restart
    let spare = spare_for(&config, &planned.profile);
    if let Some(spare) = &spare {
        planned.profile = spare.id.clone();
    }
    let roleless = body.custom.as_ref().is_some_and(|c| c.role.trim().is_empty());
    if roleless {
        // before its tools are given: `propose_profile` is listed for it
        crate::guide::mark_roleless(&config, &planned.profile)?;
    }
    // no Carnet news before its first turn: what was learnt before reaches it through the common memory
    crate::notebook::news::start(&config, &planned.profile, crate::py::time_time());
    let took = spare.is_some();
    let made = match spare {
        Some(spare) => from_spare(shared, &planned, spare, http, run).await,
        None => {
            let agent = hub_agent(&config, &planned.profile, &planned.name, &crate::py::token_urlsafe(32));
            make_profile(shared, &planned, agent, http, run, false).await
        }
    };
    if took {
        crate::agent_pool::refill_soon(shared, http, run);
    }
    let agent = match made {
        Ok(agent) => agent,
        Err(exc) => {
            if roleless {
                crate::guide::forget(&shared.get(), &planned.profile);
            }
            crate::notebook::news::forget(&shared.get(), &planned.profile);
            return Err(exc);
        }
    };
    if let Some(group) = body.group.as_deref().filter(|g| !g.trim().is_empty()) {
        crate::agent_groups::set_group(&config, &agent.name, group)?;
    }
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, kind = %body.kind.as_deref().filter(|k| !k.is_empty()).unwrap_or("custom"), "agent created");
    Ok(agent)
}

/// Several new agents at once (a team card, guide_team.rs): planned one after the other (distinct profiles), made as
/// one batch (`make_profiles`: the gateways' waits side by side, one restart at most for their tools). One result per
/// body, in order: a failed agent never undoes the others.
pub async fn create_agents(
    shared: &SharedConfig,
    bodies: &[NewAgent],
    http: &reqwest::Client,
    run: &Run,
) -> Vec<anyhow::Result<AgentConfig>> {
    let mut planned_config: Config = Config::clone(&shared.get());
    let mut out: Vec<Option<anyhow::Result<AgentConfig>>> = Vec::new();
    let mut batch: Vec<(usize, PlannedAgent, AgentConfig, bool)> = Vec::new();
    // as many members as there are spares take one (files only); the others are made as one batch
    let mut spared: Vec<(usize, PlannedAgent, crate::agent_pool::Spare, bool)> = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        let mut planned = match plan_agent(&planned_config, body) {
            Ok(p) => p,
            Err(exc) => {
                out.push(Some(Err(exc)));
                continue;
            }
        };
        let spare = spare_for(&planned_config, &planned.profile);
        if let Some(spare) = &spare {
            planned.profile = spare.id.clone();
        }
        let roleless = body.custom.as_ref().is_some_and(|c| c.role.trim().is_empty());
        if roleless {
            if let Err(exc) = crate::guide::mark_roleless(&planned_config, &planned.profile) {
                out.push(Some(Err(exc)));
                continue;
            }
        }
        let agent = match &spare {
            Some(spare) => spare.agent(&planned_config, &planned.name),
            None => hub_agent(&planned_config, &planned.profile, &planned.name, &crate::py::token_urlsafe(32)),
        };
        // the next body's plan sees this one's name and profile as taken
        planned_config.agents.insert(planned.profile.clone(), agent.clone());
        match spare {
            Some(spare) => spared.push((i, planned, spare, roleless)),
            None => batch.push((i, planned, agent, roleless)),
        }
        out.push(None);
    }
    let mut results: Vec<(usize, PlannedAgent, bool, anyhow::Result<AgentConfig>)> = Vec::new();
    let took = !spared.is_empty();
    for (i, planned, spare, roleless) in spared {
        let made = from_spare(shared, &planned, spare, http, run).await;
        results.push((i, planned, roleless, made));
    }
    let items: Vec<(PlannedAgent, AgentConfig)> = batch.iter().map(|(_, p, a, _)| (p.clone(), a.clone())).collect();
    let made = make_profiles(shared, items, http, run).await;
    results.extend(batch.into_iter().zip(made).map(|((i, planned, _, roleless), made)| (i, planned, roleless, made)));
    if took {
        crate::agent_pool::refill_soon(shared, http, run);
    }
    for (i, planned, roleless, result) in results {
        let result = match result {
            Ok(agent) => {
                if let Some(group) = bodies[i].group.as_deref().filter(|g| !g.trim().is_empty()) {
                    if let Err(exc) = crate::agent_groups::set_group(&shared.get(), &agent.name, group) {
                        tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %exc, "group not set");
                    }
                }
                tracing::info!(target: "bipbridge::agents", agent = %agent.name, kind = %bodies[i].kind.as_deref().filter(|k| !k.is_empty()).unwrap_or("custom"), "agent created");
                Ok(agent)
            }
            Err(exc) => {
                if roleless {
                    crate::guide::forget(&shared.get(), &planned.profile);
                }
                Err(exc)
            }
        };
        out[i] = Some(result);
    }
    out.into_iter().map(|r| r.unwrap_or_else(|| Err(anyhow::anyhow!("not made")))).collect()
}

/// A spare for a planned agent (agent_pool.rs), out of the pool: never for the family's agent nor a project agent
/// (marked before they are made: their tools differ from their creation on).
fn spare_for(config: &Config, planned_profile: &str) -> Option<crate::agent_pool::Spare> {
    if crate::agent_pool::wanted(config) == 0
        || crate::family_agent::is_family(config, planned_profile)
        || crate::project_agent::is_project(config, planned_profile)
    {
        return None;
    }
    crate::agent_pool::take(config)
}

/// A spare becomes the planned agent, with file writes only: its model as the hub's, its personality and
/// description, Bot Mode and its « Bot Chat » (one request to its gateway, which serves it already), its section in
/// the bridge's config, the guide's blocks, its tools' entry (written into its config.yaml, no `hermes` command).
/// Its Hermes listed the bridge's tools when it was made: a change of them reaches it by `tools/list_changed`.
pub async fn from_spare(
    shared: &SharedConfig,
    planned: &PlannedAgent,
    spare: crate::agent_pool::Spare,
    http: &reqwest::Client,
    run: &Run,
) -> anyhow::Result<AgentConfig> {
    let started = std::time::Instant::now();
    let config = shared.get();
    let folder = PathBuf::from(install_home(&config)).join("profiles").join(&spare.id);
    crate::agent_pool::follow_hub_model(&config, &folder)?;
    crate::profile_files::new_soul(&folder, &planned.soul)?;
    // what `profile create --description` writes
    crate::profile_files::edit(&folder.join("profile.yaml"), |data| {
        data.insert("description", Yaml::str(&planned.description))
    })?;
    let agent = spare.agent(&config, &planned.name);
    wait_served(http, &agent, true).await?;
    reserve_agent(shared, &agent)?;
    crate::guide::refresh(&shared.get()); // its blocks, and the guide's list of agents
    give_tools(&shared.get(), &agent, run).await?;
    tools_loaded(shared, std::slice::from_ref(&agent), http, run).await;
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, ms = started.elapsed().as_millis() as u64, "spare agent taken");
    Ok(agent)
}

/// `n` spares made as one batch (agent_pool.rs `refill`): their profiles a few at a time, served, their browser and
/// uploads, the bridge's tools; one wait for their tools to be listed and at most one restart of the hub's gateway,
/// only when all its agents are idle. Those made are returned (a failed one is left out, its folder hidden).
pub async fn make_spares(
    shared: &SharedConfig,
    n: usize,
    http: &reqwest::Client,
    run: &Run,
) -> Vec<crate::agent_pool::Spare> {
    let config = shared.get();
    let started = std::time::Instant::now();
    // replaced when it is taken; the guide's daily personality meanwhile
    let soul = crate::souls::soul_for(crate::guide::GUIDE_NAME, "daily", None).unwrap_or_default();
    let items: Vec<(PlannedAgent, AgentConfig)> = (0..n)
        .map(|_| {
            let id = crate::agent_pool::new_id();
            let planned =
                PlannedAgent { profile: id.clone(), name: String::new(), soul: soul.clone(), description: String::new() };
            let agent = hub_agent(&config, &id, "", &crate::py::token_urlsafe(32));
            (planned, agent)
        })
        .collect();
    let mut preparing: Vec<BoxFuture<'_, anyhow::Result<()>>> = Vec::new();
    for (planned, agent) in &items {
        preparing.push(Box::pin(prepare_profile(shared, planned, agent, run, false)));
    }
    let prepared: Vec<anyhow::Result<()>> =
        futures::stream::iter(preparing).buffered(PREPARED_TOGETHER).collect().await;
    let served = futures::future::join_all(items.iter().zip(prepared).map(|((_, agent), ready)| async move {
        ready?;
        wait_served(http, agent, false).await
    }))
    .await;
    let mut made: Vec<AgentConfig> = Vec::new();
    for ((_, agent), served) in items.into_iter().zip(served) {
        let result: anyhow::Result<AgentConfig> = async {
            served?;
            let agent = with_browser(&config, agent, run).await;
            let agent = mount_uploads(shared, &agent, run, false).await?;
            give_tools(&config, &agent, run).await?;
            Ok(agent)
        }
        .await;
        match result {
            Ok(agent) => made.push(agent),
            Err(exc) => {
                tracing::warn!(target: "bipbridge::agents", error = %tail(&format!("{exc:#}"), 200), "spare agent not made")
            }
        }
    }
    // Their tools listed before anyone takes them: one wait for the batch, one restart at most (all agents idle).
    let sessions = &crate::mcp_sessions::SESSIONS;
    if made.first().is_some_and(|first| sessions.others_listed(&first.name)) {
        let waited = std::time::Instant::now();
        while made.iter().any(|a| !sessions.has_listed(&a.name)) && waited.elapsed().as_secs_f64() < TOOLS_LISTED_SECONDS
        {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let missing: Vec<AgentConfig> = made.iter().filter(|a| !sessions.has_listed(&a.name)).cloned().collect();
        if !missing.is_empty() && !restart_hub_when_calm(shared, &missing, http, run).await {
            // one taken before a restart: its first turn waits for its tools (`tools_loaded`)
            tracing::info!(target: "bipbridge::agents", spares = missing.len(), "spare agents without their tools yet");
        }
    }
    tracing::info!(target: "bipbridge::agents", spares = made.len(), seconds = format!("{:.1}", started.elapsed().as_secs_f64()), "spare agents made");
    made.iter().map(crate::agent_pool::Spare::of).collect()
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
    prepare_profile(shared, planned, &agent, run, resume).await?;
    // The gateway picks new profiles up within 30 s; its permanent « Bot Chat » is opened as soon as it answers.
    wait_served(http, &agent, true).await?;
    let agent = finish_profile(shared, planned, agent, run).await?;
    tools_loaded(shared, std::slice::from_ref(&agent), http, run).await;
    Ok(agent)
}

/// Profiles prepared at the same time in a batch.
const PREPARED_TOGETHER: usize = 3;

/// Several planned agents made as one batch (a team): their profiles a few at a time (`PREPARED_TOGETHER`), then the gateway's pick-up of all of them waited for side by side (up to 30 s each when
/// waited one after the other), then their saving, and one wait for their tools (at most one restart of the hub's
/// gateway for all of them, `tools_loaded`). One result per item, in order.
pub async fn make_profiles(
    shared: &SharedConfig,
    items: Vec<(PlannedAgent, AgentConfig)>,
    http: &reqwest::Client,
    run: &Run,
) -> Vec<anyhow::Result<AgentConfig>> {
    // the `hermes` commands of a few profiles side by side (each its own folder; `--no-alias` writes no shared file)
    let mut preparing: Vec<BoxFuture<'_, anyhow::Result<()>>> = Vec::new();
    for (planned, agent) in &items {
        preparing.push(Box::pin(prepare_profile(shared, planned, agent, run, false)));
    }
    let prepared: Vec<anyhow::Result<()>> =
        futures::stream::iter(preparing).buffered(PREPARED_TOGETHER).collect().await;
    let mut serving: Vec<BoxFuture<'_, anyhow::Result<()>>> = Vec::new();
    for ((_, agent), ready) in items.iter().zip(prepared) {
        serving.push(Box::pin(async move {
            ready?;
            wait_served(http, agent, true).await
        }));
    }
    let served = futures::future::join_all(serving).await;
    let mut out = Vec::new();
    for ((planned, agent), served) in items.into_iter().zip(served) {
        out.push(match served {
            Ok(()) => finish_profile(shared, &planned, agent, run).await,
            Err(exc) => Err(exc),
        });
    }
    let made: Vec<AgentConfig> = out.iter().filter_map(|r| r.as_ref().ok().cloned()).collect();
    tools_loaded(shared, &made, http, run).await;
    out
}

/// A profile made for `agent` (`resume`: kept when an attempt cut short made it), its key, its tools' entry and its
/// personality written: ready for the gateway to pick up.
async fn prepare_profile(
    shared: &SharedConfig,
    planned: &PlannedAgent,
    agent: &AgentConfig,
    run: &Run,
    resume: bool,
) -> anyhow::Result<()> {
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
    // Its key straight into the profile's `.env` (0600, atomic), where `hermes config set` would put it: never on a
    // command line, which every account of a shared server reads in /proc (audit 11/11, BV-6).
    std::fs::create_dir_all(&folder)?;
    crate::cloud_setup::set_env(&folder.join(".env"), "API_SERVER_KEY", &agent.hermes_key)?;
    // The family's agent (family_agent.rs): never anything that reads the place's disk or drives a browser with
    // someone's logins, sandboxed or not.
    // A project agent (project_agent.rs) the same: shown to friends, it reaches nobody's files.
    let family =
        crate::family_agent::is_family(&config, &profile) || crate::project_agent::is_project(&config, &profile);
    if !install.isolated || family {
        // Same account as this bridge and no sandbox: nothing that reads the disk until the person turns it on.
        let mut args = strings(&[&hermes, "-p", &profile, "tools", "disable"]);
        args.extend(strings(&DISK_TOOLS));
        args.extend(strings(&["--platform", "api_server"]));
        run(args, env.clone()).await?;
    }
    std::fs::create_dir_all(&folder)?;
    // The bridge's tools in its config.yaml before the gateway loads the profile: Hermes connects a profile's MCP
    // servers when it loads it, and an entry written after that is only read at its next restart (2026-10-30 bench:
    // a new agent answered its first messages with Hermes' tools only, and wrote « call me Max » into the shared
    // memory instead of using propose_profile). Given again below, once it is saved (its guide blocks, its browser).
    give_tools(&shared.get(), agent, run).await?;
    crate::profile_files::new_soul(&folder, &planned.soul)?;
    Ok(())
}

/// A served profile finished: its browser, uploads, section in the bridge's config, guide blocks and tools.
async fn finish_profile(
    shared: &SharedConfig,
    planned: &PlannedAgent,
    agent: AgentConfig,
    run: &Run,
) -> anyhow::Result<AgentConfig> {
    let config = shared.get();
    let profile = planned.profile.clone();
    let family =
        crate::family_agent::is_family(&config, &profile) || crate::project_agent::is_project(&config, &profile);
    let mut agent = if family { agent } else { with_browser(&config, agent, run).await };
    agent = mount_uploads(shared, &agent, run, false).await?;
    if shared.get().agents.contains_key(&profile) {
        save_agent(shared, &agent)?;
    } else {
        reserve_agent(shared, &agent)?;
    }
    crate::guide::refresh(&shared.get()); // its blocks, and the guide's list of agents
    give_tools(&shared.get(), &agent, run).await?;
    Ok(agent)
}

/// How long a new agent's Hermes gets to list the bridge's tools before its gateway is restarted for them.
pub const TOOLS_LISTED_SECONDS: f64 = 20.0;

/// A new agent's first turn has the bridge's tools (propose_profile, create_podcast…): when its Hermes hasn't listed
/// them within `TOOLS_LISTED_SECONDS` (it loaded the profile before its entry was written), its gateway is restarted:
/// - its own (its space): now, nothing else runs there and it has not spoken yet;
/// - the hub's, shared with the person's other agents: now when they are all idle (`agent_restart::group_calm`), before
///   the answer to its creation; else its first turn waits for its tools (`mcp_sessions` `first_turn`, at most
///   `FIRST_TURN_WAIT`) while the bridge restarts the gateway at the first calm moment (`CALM_WATCH`, then the
///   restart loop's request, never held back by its one-hour gap). 2026-10-30 bench: on the hub the restart was only
///   asked for, the first message came first, and a second agent made within the hour got no restart at all.
///
/// Only where other agents have listed them already: proof this bridge's Hermes connect at once (a Hermes that
/// lists lazily, or the first agent of an install, is never waited for).
async fn tools_loaded(shared: &SharedConfig, agents: &[AgentConfig], http: &reqwest::Client, run: &Run) {
    let sessions = &crate::mcp_sessions::SESSIONS;
    let Some(first) = agents.first() else { return };
    if !sessions.others_listed(&first.name) {
        return;
    }
    // one wait for the whole batch
    let started = std::time::Instant::now();
    while agents.iter().any(|a| !sessions.has_listed(&a.name)) && started.elapsed().as_secs_f64() < TOOLS_LISTED_SECONDS {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let missing: Vec<&AgentConfig> = agents.iter().filter(|a| !sessions.has_listed(&a.name)).collect();
    let config = shared.get();
    let (spaced, hub): (Vec<&AgentConfig>, Vec<&AgentConfig>) =
        missing.into_iter().partition(|a| a.space.as_deref().is_some_and(|s| !s.is_empty()));
    for agent in spaced {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, "new agent without its tools: its gateway restarted");
        match restart_gateway(&config, agent, run).await {
            // served again before the app's first message
            Ok(()) => {
                let _ = wait_served(http, agent, false).await;
            }
            Err(exc) => {
                tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %tail(&exc.to_string(), 200), "gateway not restarted");
                crate::agent_restart::request_restart(&agent.name, crate::agent_restart::NEW_AGENT_TOOLS);
            }
        }
    }
    let hub: Vec<AgentConfig> = hub.into_iter().cloned().collect();
    if hub.is_empty() {
        return;
    }
    hub.iter().for_each(|a| sessions.hold_first_turn(&a.name));
    if restart_hub_when_calm(shared, &hub, http, run).await {
        return;
    }
    if !crate::agent_restart::enabled(&config) {
        return; // an existing Hermes the person brought: their first turns wait a little, then go
    }
    let names: Vec<&str> = hub.iter().map(|a| a.name.as_str()).collect();
    tracing::info!(target: "bipbridge::agents", agents = %names.join(","), "new agents without their tools: first turns wait, gateway restarted once calm");
    let (shared, http, run) = (shared.clone(), http.clone(), run.clone());
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        loop {
            tokio::time::sleep(CALM_CHECK).await;
            let config = shared.get();
            let waiting: Vec<AgentConfig> = hub
                .iter()
                .filter(|a| config.agents.contains_key(&a.name) && !sessions.has_listed(&a.name))
                .cloned()
                .collect();
            hub.iter().filter(|a| !waiting.iter().any(|w| w.name == a.name)).for_each(|a| sessions.release_first_turn(&a.name));
            if waiting.is_empty() || restart_hub_when_calm(&shared, &waiting, &http, &run).await {
                return;
            }
            if started.elapsed() >= CALM_WATCH {
                for agent in &waiting {
                    crate::agent_restart::request_restart(&agent.name, crate::agent_restart::NEW_AGENT_TOOLS);
                }
                return;
            }
        }
    });
}

/// How often a new agent's tools wait for a calm moment of the hub's gateway, and how long before the restart loop
/// takes it over.
const CALM_CHECK: Duration = Duration::from_secs(3);
const CALM_WATCH: Duration = Duration::from_secs(600);

/// The hub's gateway restarted once for new agents' tools when every agent on it is idle: served again, their tools
/// listed (or `TOOLS_LISTED_SECONDS` passed). Whether it was restarted.
async fn restart_hub_when_calm(
    shared: &SharedConfig,
    agents: &[AgentConfig],
    http: &reqwest::Client,
    run: &Run,
) -> bool {
    let sessions = &crate::mcp_sessions::SESSIONS;
    let config = shared.get();
    let Some(first) = agents.first() else { return false };
    let Some(names) = crate::agent_restart::group_calm(&config, first).await else { return false };
    let new: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    tracing::info!(target: "bipbridge::agents", agents = %new.join(","), "new agents without their tools: the hub's gateway restarted, all calm");
    if let Err(exc) = restart_gateway(&config, first, run).await {
        tracing::warn!(target: "bipbridge::agents", agent = %first.name, error = %tail(&exc.to_string(), 200), "gateway not restarted");
        return false;
    }
    crate::agent_restart::restarted(&config, &names, crate::agent_restart::NEW_AGENT_TOOLS);
    futures::future::join_all(agents.iter().map(|a| wait_served(http, a, false))).await;
    // their first turns still wait for the listing if it comes later (mcp_sessions `first_turn`)
    let started = std::time::Instant::now();
    while agents.iter().any(|a| !sessions.has_listed(&a.name)) && started.elapsed().as_secs_f64() < TOOLS_LISTED_SECONDS {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    agents.iter().filter(|a| sessions.has_listed(&a.name)).for_each(|a| sessions.release_first_turn(&a.name));
    true
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
                    .json(&json!({"title": crate::threads::LEGACY_MAIN_TITLE}))
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
            || crate::agent_pool::is_spare_id(&name) // made ahead by the bridge (agent_pool.rs), never shown
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
    check_room(&config)?;
    let install = &config.hermes;
    let folder = PathBuf::from(install_home(&config)).join("profiles").join(name);
    let key = match profile_key(&folder) {
        Some(key) if !key.is_empty() => key,
        _ => {
            let key = crate::py::token_urlsafe(32);
            // In its `.env`, never on a command line (see prepare_profile).
            crate::cloud_setup::set_env(&folder.join(".env"), "API_SERVER_KEY", &key)?;
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
    Ok(section_lines(&std::fs::read_to_string(config_path)?, name))
}

fn section_lines(text: &str, name: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut inside = false;
    let wanted = format!("[agents.{name}]");
    for line in text.lines() {
        let header = line.trim();
        if header.starts_with('[') && header.ends_with(']') {
            inside = header == wanted;
            continue;
        }
        if inside && !header.is_empty() {
            lines.push(line.to_string());
        }
    }
    lines
}

/// The hub's sandbox written into a profile that relies on it (`terminal.backend: docker` in its own config.yaml): Hermes
/// never gives a profile the hub's backend (shares::terminal_backend), so such an agent ran its tools on this machine,
/// as this account, while the bridge treated it as sandboxed. Wellness on Alex's place (moved in by hand on 13/10):
/// « Failed to write file: Permission denied: /workspace » on 11/11, the bridge having sent it to /workspace/brouillons.
async fn sandbox_of_its_own(config: &Config, agent: &AgentConfig, folder: &Path, run: &Run) -> anyhow::Result<()> {
    if crate::shares::own_terminal_backend(folder).is_some() {
        return Ok(());
    }
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    run(
        strings(&[&config.hermes.bin, "-p", &agent.name, "config", "set", "terminal.backend", "docker"]),
        env_of(&[("HERMES_HOME", &home)]),
    )
    .await?;
    tracing::warn!(target: "bipbridge::agents", agent = %agent.name,
        "its tools ran on this machine (its profile set no sandbox of its own): now in the hub's sandbox");
    Ok(())
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
    if agent.shares == "external" || !folder.is_dir() || crate::shares::terminal_backend(&folder).as_deref() != Some("docker") {
        return Ok(agent.clone());
    }
    sandbox_of_its_own(&config, agent, &folder, run).await?;
    crate::sandbox::label_hermes_folders(&folder, run).await;
    if host.is_empty() {
        return Ok(agent.clone());
    }
    if crate::sandbox::isolates(&config, agent) {
        // Linux with podman: no folder of this machine in the sandbox, its uploads copied into a volume (sandbox.rs).
        return crate::sandbox::isolate(shared, agent, run, save).await;
    }
    // Isolated before (an install stepping back): what isolation set taken out first.
    crate::sandbox::revert(&config, agent, &folder, run).await?;
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
    wanted.push(volume.clone());
    // Already there once (other mounts after it, such as the shared folders): left where it is.
    if current.iter().filter(|v| v.split(':').nth(1) == Some(SANDBOX_UPLOADS)).collect::<Vec<_>>() == [&volume] {
        wanted = current.clone();
    }
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

/// The gateway serving the agent restarted, to read its settings again: its space's (its systemd unit when there is
/// one, else Hermes' own command in its HERMES_HOME), or the hub's (cloud_setup's `restart_command`). The hub's
/// gateway serves all its agents: they all restart.
pub async fn restart_gateway(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<()> {
    // a message that finds it down in the next moments waits in line (threads/send.rs), never « isn't answering »
    crate::agent_restart::restart_began(agent);
    let mut env = BTreeMap::new();
    let units = crate::place_backup::home().join(".config").join("systemd").join("user");
    let command = match agent.space.as_deref().filter(|s| !s.is_empty()) {
        Some(space) => {
            let unit = crate::spaces::unit_name(&agent.name);
            if units.join(format!("{unit}.service")).exists() {
                strings(&["systemctl", "--user", "restart", &unit])
            } else {
                env.insert("HERMES_HOME".to_string(), space.to_string());
                vec![config.hermes.bin.clone(), "gateway".into(), "restart".into()]
            }
        }
        None => {
            if !config.hermes.home.is_empty() {
                env.insert("HERMES_HOME".to_string(), config.hermes.home.clone());
            }
            crate::cloud_setup::restart_command(config, &units)
        }
    };
    run(command, env).await.map(|_| ())
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
/// replaced, the rest of the section (voice, shares, a key kept in a file…) stays. The running
/// configuration gets the agent too.
pub fn save_agent(config: &SharedConfig, agent: &AgentConfig) -> anyhow::Result<()> {
    if let Some(path) = config.get().source_path.as_deref() {
        if !std::path::Path::new(path).exists() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
        }
        // one rewrite under the config's locks: the section is never missing from the file, even for an instant
        crate::config::update_source(path, |text| {
            let kept = section_lines(text, &agent.name);
            Ok(with_agent(&without_agent(text, &agent.name), agent, &kept))
        })?;
    }
    let renamed = config.get().agents.get(&agent.name).is_some_and(|a| a.display_name != agent.display_name);
    config.put_agent(agent.clone());
    if renamed {
        crate::guide::refresh(&config.get()); // the guide's list of agents
    }
    Ok(())
}

/// Drops `[agents.<name>]` from the bridge's config file (its lines, up to the next table).
pub fn remove_agent_section(config_path: &str, name: &str) -> std::io::Result<()> {
    if !std::path::Path::new(config_path).exists() {
        return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
    }
    crate::config::update_source_removing(config_path, &[name], |text| Ok(without_agent(text, name)))
}

fn without_agent(text: &str, name: &str) -> String {
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
    kept.trim_end_matches('\n').to_string() + "\n"
}

pub fn read_soul(config: &Config, agent: &AgentConfig) -> std::io::Result<String> {
    let path = profile_folder(config, agent).join("SOUL.md");
    if path.exists() {
        std::fs::read_to_string(path)
    } else {
        Ok(String::new())
    }
}

/// The agent's settings record with its personality: the one in its SOUL.md taken in once when the record has none
/// (an agent made before the record, or from a template written as a file: profile_files::new_soul).
pub fn soul_record(config: &Config, agent: &AgentConfig) -> anyhow::Result<crate::profile_files::AgentSettings> {
    let folder = profile_folder(config, agent);
    let record = crate::profile_files::settings(&folder);
    if record.personality.is_some() {
        return Ok(record);
    }
    let soul = read_soul(config, agent)?;
    if soul.trim().is_empty() {
        return Ok(record); // Hermes' own personality: nothing of the bridge's
    }
    let (personality, lang) = personality_of(&soul);
    crate::profile_files::update_settings(&folder, |s| {
        s.personality = Some(personality);
        s.language = s.language.take().or(lang);
    })?;
    Ok(crate::profile_files::settings(&folder))
}

/// The agent's SOUL.md rendered from its record (docs/agent-settings.md): the personality the person wrote, then the
/// bridge's blocks from their own stores ([`soul_text`]). Written only when it changes; whether it was. An agent the
/// bridge may not write ([`editable`]) or with Hermes' own personality (none in the record) is left as it is.
pub fn render_soul(config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
    if editable(config, &agent.name).is_err() || !profile_folder(config, agent).is_dir() {
        return Ok(false);
    }
    let record = soul_record(config, agent)?;
    let Some(personality) = record.personality.as_deref() else { return Ok(false) };
    // a personality without a language yet writes first in the place's (every agent talks plainly: owner 2026-10-22),
    // kept in its record from then on
    let lang = match record.language.clone() {
        Some(lang) => lang,
        None => {
            let lang = crate::notebook::language(config);
            crate::profile_files::update_settings(&profile_folder(config, agent), |s| s.language = Some(lang.clone()))?;
            lang
        }
    };
    let text = soul_text(config, agent, personality, &lang);
    crate::profile_files::edit_text(&profile_folder(config, agent).join("SOUL.md"), |_| Some(text))
}

/// [`render_soul`] for every agent (at start: profile_files::reconcile_all); a failure is logged.
pub fn render_souls(config: &Config) {
    for agent in config.agents.values() {
        match render_soul(config, agent) {
            Ok(true) => tracing::info!(target: "bipbridge::agents", agent = %agent.name, "personality's bridge rules brought up to date"),
            Ok(false) => {}
            Err(err) => tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %format!("{err:#}"), "personality not rendered"),
        }
    }
}

/// The agent's language block rewritten (the rest of its personality untouched); returns the language kept.
pub fn set_language(config: &Config, agent: &AgentConfig, lang: &str) -> anyhow::Result<String> {
    let code = crate::souls::agent_language(Some(lang));
    soul_record(config, agent)?;
    crate::profile_files::update_settings(&profile_folder(config, agent), |s| {
        s.personality.get_or_insert_with(String::new);
        s.language = Some(code.clone());
    })?;
    render_soul(config, agent)?;
    Ok(code)
}

pub const THINKING_PROVIDER: &str = "bipagents-thinking";
pub const THINKING_LEVELS: [&str; 5] = ["none", "low", "medium", "high", "xhigh"];
/// « Automatique » (the apps' first choice, everyone's default): no level of the person's, the bridge decides each
/// turn (turn_thinking.rs). What the routes report for such an agent ([`shown_thinking`]) and the value that clears a
/// chosen level ([`choose_thinking`]).
pub const AUTO_THINKING: &str = "auto";
/// What Hermes asks a custom server for when nothing is set.
pub const DEFAULT_THINKING: &str = "medium";
/// GLM's level through EUrouter when nothing is chosen, and what « none » becomes there (see [`keeps_thinking_on`]).
pub const GLM_THINKING: &str = "low";
/// The level the person chose for the profile in `folder` (its settings record, profile_files.rs), None when they
/// never did. A level in the config without it is not the person's: Hermes' config template writes
/// `reasoning_effort: medium` in every new profile (real-day bench 2026-10-28: the offer ran GLM at « medium », 1.8×
/// the cost), so GLM through EUrouter gets its « low » there. Kept apart from config.yaml, which Hermes' own commands
/// rewrite, so a chosen level a rewrite lost is put back (profile_files::reconcile).
pub fn chosen_thinking(folder: &Path) -> Option<String> {
    crate::profile_files::settings(folder).thinking
}

/// Models whose thinking can't be switched off: GLM through EUrouter (measured 2026-10-27, internal/research/
/// token-diet-step3-2026-10-27.md). There `reasoning_effort: none` makes GLM think ~400 tokens and answer NOTHING
/// (Entrim), `chat_template_kwargs.enable_thinking` is ignored, and « low » cuts its thinking to ~1 token on an
/// easy question: −74 % cost, 3× faster, same quality as « medium » (Hermes' default). So « low » is their default
/// and their floor: « none » is sent as « low ». Melious refuses any reasoning field for GLM (400, measured 2026-10-28
/// and 2026-10-31), so it is out of the router's order (model_route.rs); Hermes also retries a turn once without the
/// field on that 400.
pub fn keeps_thinking_on(model: &str, base_url: &str) -> bool {
    model.to_lowercase().contains("glm") && crate::model_route::pins(base_url)
}

/// Whether a Hermes config's model is one of [`keeps_thinking_on`] (provider `custom`, its model id and address).
pub fn thinking_floor(data: &Yaml) -> bool {
    let Some(model) = data.get("model").filter(|m| m.is_map()) else { return false };
    model.get("provider").and_then(Yaml::as_str) == Some("custom")
        && keeps_thinking_on(
            &model.get("default").map(Yaml::py_str).unwrap_or_default(),
            model.get("base_url").and_then(Yaml::as_str).unwrap_or(""),
        )
}

/// The level written in a config (`agent.reasoning_effort`, « off »/false read as « none »), None when unset.
fn written_thinking(data: &Yaml) -> Option<String> {
    let effort = data.get("agent").filter(|a| a.is_map()).and_then(|a| a.get("reasoning_effort"));
    let effort = match effort {
        None | Some(Yaml::Null) => return None,
        Some(Yaml::Str(s)) if s.is_empty() => return None,
        Some(value) => value.py_str().trim().to_lowercase(),
    };
    if ["none", "false", "disabled", "off"].contains(&effort.as_str()) {
        // YAML false reads « false »
        return Some("none".into());
    }
    Some(effort)
}

/// A config with GLM's floor (see [`keeps_thinking_on`]), `chosen` the level the person chose ([`chosen_thinking`]):
/// - chosen: that level (« none » written « low »), also when a rewrite of the config lost it;
/// - nothing chosen: « low », over Hermes' template « medium » too (never the person's: [`chosen_thinking`]);
///   « high » or « xhigh », which only a person writes (bridges before the record didn't keep one), stay.
///
/// None when nothing changes (another model, the level already right). Applied wherever the bridge writes a model's
/// routing (model_route::with_routing): at each start for every agent, at a claim, for the hub new agents are cloned
/// from, when the model or the level changes, each time the bridge gives an agent its tools.
pub fn with_thinking_floor(data: &Yaml, chosen: Option<&str>) -> Option<Yaml> {
    if !thinking_floor(data) {
        return None;
    }
    let written = written_thinking(data);
    let wanted = match chosen {
        Some("none") | None => GLM_THINKING,
        Some(level) => level,
    };
    let kept = match chosen {
        Some(_) => written.as_deref() == Some(wanted),
        None => written.as_deref().is_some_and(|l| [GLM_THINKING, "high", "xhigh"].contains(&l)),
    };
    if kept {
        return None;
    }
    let mut out = data.clone();
    let mut settings = out.get("agent").filter(|a| a.is_map()).cloned().unwrap_or_else(Yaml::map);
    settings.insert("reasoning_effort", Yaml::str(wanted));
    out.insert("agent", settings);
    Some(out)
}

/// The `model_options` of a turn that wants an answer at once (Live calls, the Live notes): thinking off, or GLM's
/// floor where it can't be switched off (« none » would make it answer nothing). Hermes 517b5e10f reads both shapes
/// (`gateway/platforms/api_server_request_options.py`); to a custom server « enabled: false » goes out as
/// `reasoning_effort: none`, « effort: low » as `reasoning_effort: low`.
pub fn quick_turn_options(config: &Config, agent: &AgentConfig) -> Value {
    let floor = load_or_empty(&profile_config_path(config, agent)).map(|d| thinking_floor(&d)).unwrap_or(false);
    if floor {
        json!({"reasoning": {"enabled": true, "effort": GLM_THINKING}})
    } else {
        json!({"reasoning": {"enabled": false}})
    }
}

/// The `model_options` of a background task's run (jobs.rs): « medium » for GLM until « low » is measured on long
/// tasks (decided 2026-10-28 after the bench). None: the agent's own level (another model, or a level the person
/// set above « low »).
pub fn task_turn_options(config: &Config, agent: &AgentConfig) -> Option<Value> {
    let data = load_or_empty(&profile_config_path(config, agent)).ok()?;
    let level = written_thinking(&data).unwrap_or_else(|| GLM_THINKING.into());
    (thinking_floor(&data) && (level == "none" || level == GLM_THINKING))
        .then(|| json!({"reasoning": {"enabled": true, "effort": DEFAULT_THINKING}}))
}

/// An agent whose thinking the bridge decides turn by turn (turn_thinking.rs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoThinking {
    /// Its model can't think less than « low » (GLM through EUrouter, [`keeps_thinking_on`]).
    pub floor: bool,
}

/// Whether the bridge decides `agent`'s thinking turn by turn: a model it sets (provider `custom` with an address,
/// see [`read_thinking`]) and no level the person chose ([`chosen_thinking`]: a chosen level stays every turn's).
pub fn auto_thinking(config: &Config, agent: &AgentConfig) -> Option<AutoThinking> {
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        return None; // set up by hand: its level is its administrator's
    }
    let data = load_or_empty(&profile_config_path(config, agent)).ok()?;
    auto_thinking_of(&data, chosen_thinking(&profile_folder(config, agent)).as_deref())
}

/// [`auto_thinking`] of a config, `chosen` the person's level.
pub fn auto_thinking_of(data: &Yaml, chosen: Option<&str>) -> Option<AutoThinking> {
    if chosen.is_some() {
        return None;
    }
    thinking_of(data, None)?;
    // a level only a person writes, from a bridge before the record (« high », « xhigh », « none » with the chat
    // template's thinking off): theirs
    let written = written_thinking(data);
    let entry = data.get("providers").and_then(|p| p.get(THINKING_PROVIDER)).is_some();
    if matches!(written.as_deref(), Some("high" | "xhigh")) || (written.as_deref() == Some("none") && entry) {
        return None;
    }
    Some(AutoThinking { floor: thinking_floor(data) })
}

fn profile_config_path(config: &Config, agent: &AgentConfig) -> PathBuf {
    profile_folder(config, agent).join("config.yaml")
}

/// `yaml.safe_load(path.read_text()) or {}`.
pub fn load_or_empty(path: &Path) -> anyhow::Result<Yaml> {
    let data = load_yaml_file(path)?;
    Ok(if data.truthy() { data } else { Yaml::map() })
}

/// How much the agent thinks before answering (none … xhigh), or None when its model is not one the bridge can
/// set (an OpenAI-compatible server of the person's: provider `custom` with an address).
pub fn read_thinking(config: &Config, agent: &AgentConfig) -> Option<String> {
    let data = load_or_empty(&profile_config_path(config, agent)).ok()?;
    thinking_of(&data, chosen_thinking(&profile_folder(config, agent)).as_deref())
}

/// [`read_thinking`] of a config, `chosen` the person's level ([`chosen_thinking`]).
fn thinking_of(data: &Yaml, chosen: Option<&str>) -> Option<String> {
    let model = data.get("model").filter(|m| m.is_map())?;
    if model.get("provider").and_then(Yaml::as_str) != Some("custom")
        || !model.get("base_url").is_some_and(Yaml::truthy)
    {
        return None;
    }
    if let Some(chosen) = chosen {
        return Some(chosen.to_string()); // the person's, whatever a rewrite of the config left there (reconcile)
    }
    let floor = thinking_floor(data);
    let default = if floor { GLM_THINKING } else { DEFAULT_THINKING };
    let Some(effort) = written_thinking(data) else { return Some(default.into()) };
    if floor && effort == DEFAULT_THINKING {
        return Some(default.into()); // Hermes' template « medium », never the person's: GLM thinks « low »
    }
    Some(if THINKING_LEVELS.contains(&effort.as_str()) { effort } else { default.into() })
}

/// What the app shows in « Réflexion »: « auto » when the bridge decides each turn ([`auto_thinking`]), else the level
/// ([`read_thinking`]); None when its model can't be set from the app.
pub fn shown_thinking(config: &Config, agent: &AgentConfig) -> Option<String> {
    let level = read_thinking(config, agent)?;
    Some(if auto_thinking(config, agent).is_some() { AUTO_THINKING.into() } else { level })
}

/// The level the person chose from the app or the console: recorded (their settings record), then written.
/// « auto » ([`AUTO_THINKING`]): their level forgotten and the default written (no thinking entry; GLM's « low »), so
/// the bridge decides each turn again ([`auto_thinking`]).
pub fn choose_thinking(config: &Config, agent: &AgentConfig, level: &str) -> anyhow::Result<String> {
    if level == AUTO_THINKING {
        // set up by hand: its level is its administrator's, never decided turn by turn (auto_thinking)
        if read_thinking(config, agent).is_none() || agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
            return Err(LookupError(agent.name.clone()).into());
        }
        let folder = profile_folder(config, agent);
        crate::profile_files::update_settings(&folder, |s| s.thinking = None)?;
        crate::profile_files::edit(&folder.join("config.yaml"), |data| {
            let default = if thinking_floor(data) { GLM_THINKING } else { DEFAULT_THINKING };
            write_thinking(data, default, None, &config.model_provider_order);
        })?;
        return Ok(AUTO_THINKING.into());
    }
    if !THINKING_LEVELS.contains(&level) || read_thinking(config, agent).is_none() {
        return Err(LookupError(agent.name.clone()).into());
    }
    crate::profile_files::update_settings(&profile_folder(config, agent), |s| s.thinking = Some(level.into()))?;
    set_thinking(config, agent, level)
}

/// The agent's thinking level, read by Hermes from the next message (it reloads a profile's config when it
/// changes): `reasoning_effort`, which GLM-like servers follow (the model still answers an easy question at once).
/// « none » also turns the chat template's `enable_thinking` off (bound to the model's address), for servers that
/// only follow that one. Never on: it would win over a turn that asks for no thinking (Live).
/// GLM through EUrouter ([`keeps_thinking_on`]): « none » is written « low » (its floor), the person's choice kept
/// in their record so the app still shows it; no `enable_thinking` entry (ignored there).
pub fn set_thinking(config: &Config, agent: &AgentConfig, level: &str) -> anyhow::Result<String> {
    if !THINKING_LEVELS.contains(&level) || read_thinking(config, agent).is_none() {
        return Err(LookupError(agent.name.clone()).into());
    }
    let folder = profile_folder(config, agent);
    let chosen = chosen_thinking(&folder);
    let mut floored = false;
    crate::profile_files::edit(&folder.join("config.yaml"), |data| {
        floored = write_thinking(data, level, chosen.as_deref(), &config.model_provider_order);
    })?;
    keep_floored_none(&folder, floored, chosen.as_deref())?;
    Ok(level.to_string())
}

/// « none » written « low » under GLM's floor with no level recorded (a level of an earlier bridge, carried to GLM):
/// recorded, so the app still shows « none ».
fn keep_floored_none(folder: &Path, floored: bool, chosen: Option<&str>) -> anyhow::Result<()> {
    if floored && chosen.is_none() {
        crate::profile_files::update_settings(folder, |s| s.thinking = Some("none".into()))?;
    }
    Ok(())
}

/// `level` written in a config (see [`set_thinking`]), with the router's order; whether it was « none » written
/// « low » under GLM's floor.
fn write_thinking(data: &mut Yaml, level: &str, chosen: Option<&str>, order: &[String]) -> bool {
    let floored = thinking_floor(data) && level == "none";
    let written = if floored { GLM_THINKING } else { level };
    let mut providers = data.get("providers").filter(|p| p.is_map()).cloned().unwrap_or_else(Yaml::map);
    if written == "none" {
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
    settings.insert("reasoning_effort", Yaml::str(written));
    data.insert("agent", settings);
    if let Some(routed) = crate::model_route::with_routing(data, order, chosen) {
        *data = routed; // the thinking entry is the one Hermes picks for the router's address: it carries the routing
    }
    floored
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
    let folder = profile_folder(config, agent);
    let recorded = chosen_thinking(&folder);
    let mut floored = false;
    crate::profile_files::edit(&folder.join("config.yaml"), |data| {
        let floor_before = thinking_floor(data);
        // the level the person chose (« none » under GLM's floor included), carried over to the new model
        let chosen = written_thinking(data).and_then(|_| thinking_of(data, recorded.as_deref()));
        if let Some(model) = data.get_mut("model") {
            model.insert("default", Yaml::str(name));
            if let Some(vision) = vision {
                model.insert("supports_vision", Yaml::Bool(vision));
            }
        }
        let order = &config.model_provider_order;
        if let Some(routed) = crate::model_route::with_routing(data, order, recorded.as_deref()) {
            *data = routed;
        }
        if floor_before != thinking_floor(data) {
            // to or from GLM through EUrouter: « none » written as that model needs it (set_thinking)
            if let Some(level) = chosen {
                floored = write_thinking(data, &level, recorded.as_deref(), order);
            }
        }
    })?;
    keep_floored_none(&folder, floored, recorded.as_deref())?;
    Ok(read_model(config, agent).unwrap_or_else(|| json!({})))
}

/// The thinking level in a config as it should be (profile_files::reconcile), `chosen` the person's level:
/// - a config an earlier bridge wrote with the chat template's thinking forced on: that wins over a turn asking for
///   none (Live), so it goes (its reasoning_effort stays);
/// - GLM through EUrouter ([`keeps_thinking_on`], 2026-10-28): nothing chosen (Hermes' template « medium »
///   included: the migration of existing places, real-day bench 2026-10-28) becomes « low », « none » becomes
///   « low » with the person's choice kept for the app, and the ignored `enable_thinking` entry goes;
/// - a level the person chose that a rewrite of the config lost is put back: Hermes' own commands rewrite the config
///   (`hermes config set`, `tools enable`), and an agent still being made after a claim lost the level the person
///   had just set (« Prof » back to « medium », real-day bench 2026-10-28).
///
/// Whether « none » was written « low » under GLM's floor with nothing recorded (to record: [`keep_floored_none`]).
pub fn heal_thinking_in(data: &mut Yaml, chosen: Option<&str>, order: &[String]) -> bool {
    let Some(level) = thinking_of(data, chosen) else { return false };
    let entry = data.get("providers").and_then(|p| p.get(THINKING_PROVIDER)).is_some();
    let heal = if thinking_floor(data) {
        entry || with_thinking_floor(data, chosen).is_some()
    } else {
        (entry && level != "none")
            || (!entry && level == "none" && chosen.is_some())
            || chosen.is_some_and(|c| written_thinking(data).as_deref() != Some(c))
    };
    heal && write_thinking(data, &level, chosen, order) && chosen.is_none()
}

/// [`keep_floored_none`] for profile_files::reconcile.
pub fn record_floored_none(folder: &Path) -> anyhow::Result<()> {
    keep_floored_none(folder, true, None)
}

/// The settings page of an agent. `GatewayError` (Python's `httpx.HTTPError`) when its gateway does not answer.
pub async fn read_settings(config: &Config, agent: &AgentConfig, http: &reqwest::Client) -> anyhow::Result<Value> {
    // The language block is the bridge's: shown and changed as « language », never in the editable personality.
    let record = soul_record(config, agent)?;
    let (soul, lang) = (record.personality.unwrap_or_default(), record.language);
    let guide = json!({"on": crate::guide::is_guide(config, &agent.name)});
    let notify = json!({"on": crate::agent_notify::notifies(config, &agent.name)});
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        // set up by hand: its tools are its administrator's
        return Ok(json!({"id": agent.name, "name": agent.display_name, "soul": soul.trim(), "language": lang,
                         "thinking": shown_thinking(config, agent), "model": null, "by_hand": true, "toolsets": [],
                         "isolated": agent.isolated, "guide": guide, "notify": notify,
                         "answer_style": crate::answer_style::view(agent),
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
                if out["name"] == crate::tool_diet::CRON_TOOLSET {
                    // the « Routines » tile: the bridge's routines (Hermes' cron stays off)
                    out.insert("enabled".into(), Value::Bool(crate::routines::enabled(config, &agent.name)));
                }
                Value::Object(out)
            })
            .collect(),
        _ => Vec::new(),
    };
    Ok(json!({"id": agent.name, "name": agent.display_name, "soul": soul.trim(), "language": lang,
              "thinking": shown_thinking(config, agent), "model": read_model(config, agent), "toolsets": toolsets,
              "isolated": agent.isolated,
              "protected": config.hermes.isolated,
              "services": crate::services::listing(config, agent), "skills": crate::skill_library::annotate(config, &agent.name, crate::own_skills::listing(&profile_folder(config, agent))),
              "browser": browser_settings(config, agent),
              "health": {"on": crate::health::enabled(config, agent.name.as_str()),
                         "categories": crate::health::categories(config, agent.name.as_str())},
              "weather": {"on": crate::weather::enabled(config, agent.name.as_str())},
              "notebook": {"on": crate::notebook::enabled(config, agent.name.as_str())},
              "guide": guide,
              "notify": notify,
              "answer_style": crate::answer_style::view(agent),
              // the per-agent model at home (model_choice.rs): its presence tells the apps this bridge offers it
              "model_choice": crate::model_choice::view(config, agent),
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
    let mut toolsets = lax_map(&body.toolsets);
    // the « Routines » tile drives the bridge's routines, never Hermes' cron (tool_diet.rs keeps it off)
    let routines_on = toolsets.shift_remove(crate::tool_diet::CRON_TOOLSET);
    let skills = lax_map(&body.skills);
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty())
        && (!toolsets.is_empty()
            || !skills.is_empty()
            || body.browser.is_some()
            || body.health.is_some()
            || body.health_categories.is_some()
            || body.weather.is_some()
            || body.notebook.is_some())
    {
        return Err(AgentError("the tools of an agent set up by hand are changed on its server".into()).into());
    }
    if crate::family_agent::is_family(&config, &agent.name)
        && crate::family_agent::refused_setting(
            &toolsets,
            body.browser,
            body.health == Some(true) || body.notebook == Some(true),
        )
    {
        return Err(AgentError("the family's agent keeps nobody's private data: these tools stay off".into()).into());
    }
    if crate::project_agent::is_project(&config, &agent.name)
        && crate::project_agent::refused_setting(
            &toolsets,
            body.browser,
            body.health == Some(true) || body.notebook == Some(true),
        )
    {
        return Err(AgentError("a project agent is shown to friends: these tools stay off".into()).into());
    }
    if let Some(on) = routines_on {
        crate::routines::set_enabled(&config, &agent.name, on);
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
        write_soul(&config, agent, soul)?;
    }
    if body.answer_style.is_some() || body.answer_style_custom.is_some() {
        // the soul's style paragraph follows at once (an agent set up by hand too: its soul is the bridge's to write)
        let style = body.answer_style.clone().unwrap_or_else(|| crate::answer_style::style_of(agent).to_string());
        crate::answer_style::set(shared, agent, &style, body.answer_style_custom.as_deref())?;
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
    if let Some(on) = body.notebook {
        if crate::notebook::set_enabled(&config, &agent.name, on)? {
            // Its tools' entry changes (the notebook tools listed or not) and its rule with them.
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
    // last: the steps above save the section they were given (its old name)
    if let Some(name) = body.name.as_deref().filter(|n| !n.trim().is_empty()) {
        let current = shared.get().agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
        crate::guide::rename(shared, &current, name)?;
    }
    Ok(())
}

/// The personality the person edited (or imported), kept in the record and rendered with the bridge's blocks.
pub fn write_soul(config: &Config, agent: &AgentConfig, soul: &str) -> anyhow::Result<()> {
    soul_record(config, agent)?;
    crate::profile_files::update_settings(&profile_folder(config, agent), |s| s.personality = Some(soul.trim().to_string()))?;
    render_soul(config, agent).map(|_| ())
}

/// The agent's SOUL.md: `personality` (the person's text) and its language block, then the bridge's blocks in ONE
/// fixed order, the ones that change most last (cache audit 2026-11-03: the prompt's cache breaks at the first
/// changed character, and each block used to move to the end when it changed): the rules that never change (payments,
/// the offer's server), the switches' rules (Carnet, archives, shared skills, health, shared memory), the family's or
/// project's block, then the personal parts: the guide's blocks (they list the other agents) and today's line. The
/// only renderer of SOUL.md (golden: tests/golden/soul_rendered.md).
pub fn soul_text(config: &Config, agent: &AgentConfig, personality: &str, lang: &str) -> String {
    let text = format!("{}\n", personality.trim());
    let text = crate::answer_style::apply(&crate::souls::with_language(&text, Some(lang)), agent);
    // the family's agent: none of its creator's private tools, its own block instead (family_agent.rs)
    let project = crate::project_agent::is_project(config, &agent.name);
    let family = crate::family_agent::is_family(config, &agent.name);
    let closed = family || project;
    let open = !closed && !agent.host_home.as_deref().is_some_and(|h| !h.is_empty());
    let text = if open { crate::souls::with_payments(&text) } else { text };
    let text = crate::souls::with_server(&text, open && crate::handover::on_offer());
    let text = crate::souls::with_notebook(&text, !closed && crate::notebook::enabled(config, &agent.name));
    let text = crate::souls::with_archives(&text, !closed && crate::archives::enabled(config, &agent.name));
    let text = crate::skill_library::with_block(&text, !closed && crate::skill_library::holds(config, &agent.name));
    let text = crate::souls::with_health(&text, !closed && crate::health::enabled(config, &agent.name));
    let circle = if closed { None } else { crate::shared_memory::membership(config, &agent.name) };
    let text = crate::souls::with_shared_memory(&text, circle);
    let text = if closed { crate::approval_level::split_block(&text).0 } else { crate::approval_level::with_block(config, &agent.name, &text) };
    let text = crate::family_agent::with_family(&text, family);
    let text = crate::project_agent::with_project(&text, project);
    let text = crate::guide::with_blocks(config, agent, &text);
    let line = crate::dates::agent_line(config, &crate::dates::load(config), &text);
    crate::dates::with_dates(&text, line.as_deref())
}

/// A SOUL.md split into the person's personality (without any of the bridge's blocks) and its block's language.
pub fn personality_of(soul: &str) -> (String, Option<String>) {
    let (soul, lang) = crate::souls::split_language(soul);
    let (soul, _) = crate::souls::split_health(&soul);
    let (soul, _) = crate::souls::split_notebook(&soul);
    let (soul, _) = crate::souls::split_shared_memory(&soul);
    let (soul, _) = crate::approval_level::split_block(&soul);
    let (soul, _) = crate::skill_library::split_block(&soul);
    let (soul, _) = crate::souls::split_archives(&soul);
    let soul = crate::souls::split_server(&crate::souls::split_payments(&soul));
    let (soul, _) = crate::dates::split_dates(&soul);
    let (soul, _) = crate::family_agent::split_family(&soul);
    let (soul, _) = crate::project_agent::split_project(&soul);
    (crate::guide::split_blocks(&soul).trim().to_string(), lang)
}

/// The personality as the person sees and edits it: without the bridge's blocks.
pub fn editable_soul(config: &Config, agent: &AgentConfig) -> anyhow::Result<String> {
    Ok(soul_record(config, agent)?.personality.unwrap_or_default())
}

/// « Santé » on or off: the switch, the rule in its personality, and its tools' entry (the `health` tool listed or
/// not: a changed entry makes its Hermes fetch the list again).
pub async fn set_health(config: &Config, agent: &AgentConfig, on: bool, run: &Run) -> anyhow::Result<()> {
    crate::health::set_enabled(config, &agent.name, on)?;
    render_soul(config, agent)?;
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
    crate::sandbox::forget(&config, &agent.name, run).await;
    crate::sent_files::forget(&config, &agent.name);
    crate::agent_tools::forget(&config, &agent.name);
    crate::health::forget(&config, &agent.name);
    // the phone's calendar, reminders and address book: their copy goes with the last agent allowed (2026-11-01)
    crate::calendar::forget(&config, &agent.name);
    crate::reminders::forget(&config, &agent.name);
    crate::contacts::forget(&config, &agent.name);
    crate::weather::forget(&config, &agent.name);
    crate::guide::forget(&config, &agent.name);
    crate::family_agent::forget(&config, &agent.name);
    crate::project_agent::forget(&config, &agent.name);
    crate::friends_cards::forget(&config, &agent.name);
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, "agent deleted");
    Ok(())
}

// -- routes ---------------------------------------------------------------------------------------------------------

/// One creation (or change) at a time: profile names, the config file.
pub(crate) static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

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
    json!({"id": agent.name, "name": agent.display_name, "url": agent.public_url,
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
            if is_os_error(&exc) || too_many(&exc) {
                return Err(exc.into());
            }
            tracing::warn!(target: "bipbridge::agents", error = %head(&exc.to_string(), 200), "agent creation failed");
            return Err(HttpError::new(502, "the agent could not be created: try again in a moment"));
        }
    };
    // its first conversation opens with its welcome (welcome.rs): a kind's own, or the one of an agent with its role
    let kind = if body.custom.is_some() { "custom" } else { body.kind.as_deref().unwrap_or("custom") };
    let lang = body.lang.clone().filter(|l| !l.is_empty()).unwrap_or_else(|| crate::notebook::place_language(&state.config()).unwrap_or_else(|| "fr".into()));
    crate::welcome::wait_for(&state.config(), &agent, kind, &lang);
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
            if is_os_error(&exc) || too_many(&exc) {
                return Err(exc.into());
            }
            tracing::warn!(target: "bipbridge::agents", agent = %body.id, error = %tail(&exc.to_string(), 200), "claim failed");
            return Err(HttpError::new(502, "the agent could not be started: try again in a moment"));
        }
    };
    Ok(web::json_response(201, &created(&state.config(), &agent)))
}

async fn get_settings(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    match read_settings(&config, &agent, &state.http).await {
        Ok(settings) => Ok(web::ok(settings)),
        Err(exc) if exc.downcast_ref::<GatewayError>().is_some() => Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
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
    let previous = read_soul(&config, &agent).ok().and_then(|soul| crate::souls::split_language(&soul).1);
    let code = set_language(&config, &agent, &body.lang)?;
    // its memory may still be in the old language and pull its replies back to it: translated once when it clearly is
    crate::memory_language::after_change(config.clone(), state.http.clone(), agent.clone(), previous, code.clone());
    Ok(web::ok(json!({"id": agent.name, "language": code})))
}

/// How much the agent thinks before answering (null: its model can't be set from here).
async fn get_thinking(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    Ok(web::ok(json!({"id": agent.name, "thinking": shown_thinking(&config, &agent)})))
}

/// The thinking level, from the app, between two messages.
async fn put_thinking(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<AgentThinking>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| not_managed())?;
    // an agent a claim is still making: its profile first (its config is written meanwhile)
    crate::cloud_setup::READINESS.wait(&agent.name, crate::cloud_setup::READY_WAIT).await;
    let _guard = LOCK.lock().await;
    match choose_thinking(&config, &agent, &body.level) {
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
    if body.browser.is_some() {
        // its « Coordonnées » in its Hermes vault only while its browser is on (vault.rs `follow_browser`)
        let config = state.config();
        let agent = config.agent(Some(&agent.name)).cloned().unwrap_or_else(|| agent.clone());
        match state.vault.follow_browser(&config, &agent).await {
            Ok(true) => {
                let _ = crate::profile_files::reconcile(&config, &agent);
            }
            Ok(false) => {}
            Err(exc) => tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %exc,
                "contact details not matched to its browser"),
        }
    }
    // as it is now (its answer style may have changed)
    let agent = state.config().agent(Some(&agent.name)).cloned().unwrap_or(agent);
    match read_settings(&state.config(), &agent, &state.http).await {
        Ok(settings) => Ok(web::ok(settings)),
        Err(exc) if exc.downcast_ref::<GatewayError>().is_some() => {
            Ok(web::ok(json!({"id": agent.name, "name": agent.display_name,
                              "answer_style": crate::answer_style::view(&agent)})))
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
        let url = crate::family_relay::reach_url(&entry);
        let key_out = entry.get("key_out").map(crate::py::py_str).unwrap_or_default();
        let listed: Vec<Value> = match crate::door_tls::household_client(&state.http, &entry)
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
        match crate::peers::apply(&state.config(), &state.links, &wanted, Some(&state.households)) {
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
    // An agent linked with another household's: out of the person's hub once idle (own_space.rs).
    let config = state.config();
    for (from, to) in &wanted {
        let (ours, theirs) = if crate::households::split(from).is_some() { (to, from) } else { (from, to) };
        if crate::households::split(theirs).is_none() {
            continue;
        }
        if let Some(agent) = config.agent(Some(ours)).filter(|a| crate::own_space::should_leave_hub(&config, a)) {
            crate::own_space::apart_when_idle(&state, &agent.name);
        }
    }
    // Who may write to whom decides who gets send_files_to_agent (agent_files.rs): their tools' list follows.
    let run = default_run();
    for agent in config.agents.values() {
        give_tools(&config, agent, &run).await?;
    }
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
    // « Se connecter avec… » (connections.rs): signed in at the service, apart so older apps don't show a form for them
    let connections: Vec<Value> = crate::connections::catalog().iter().map(|s| s.public()).collect();
    Ok(web::ok(json!({"services": services, "connections": connections})))
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

/// The agent out of the shared hub into its own space, its conversations, memory and files with it, by itself
/// (own_space.rs: its first secret, or other people talk to it). Takes up to two minutes. `keep_hub`: the hub keeps
/// serving its profile until `Spaces::forget_in_hub` (a turn going on there ends there).
pub async fn move_into_space(state: &AppState, agent: &AgentConfig, keep_hub: bool) -> ApiResult<AgentConfig> {
    let _guard = LOCK.lock().await;
    let run = default_run();
    let spaces = crate::spaces::Spaces::new(run.clone());
    let config = state.config();
    let moving = if keep_hub {
        spaces.move_agent_keeping_hub(&config, agent, &state.http).await
    } else {
        spaces.move_agent(&config, agent, &state.http).await
    };
    let moved = match moving {
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
    // Its sandbox in the space as in the hub: its files' volume at /fichiers, its outbox, its shared folders, and the
    // bridge naming its received files by that path. Until 2026-11-07 that waited for the bridge's next start, and
    // the files another agent gave it were named by its space's folder, which its sandbox doesn't mount.
    let moved = match mount_uploads(&state.config, &moved, &run, true).await {
        Ok(mounted) => mounted,
        Err(exc) => {
            tracing::warn!(target: "bipbridge::agents", agent = %moved.name, error = %tail(&exc.to_string(), 200), "uploads not mounted in the new space's sandbox (the bridge's next start tries again)");
            moved
        }
    };
    give_tools(&state.config(), &moved, &run).await?; // its profile is a new one, in its space
    Ok(moved)
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
    let family = crate::family_agent::is_family(&state.config(), &agent.name);
    let result = async {
        delete_agent(&state.config, &agent, &default_run()).await?;
        if family {
            crate::family_agent::deleted(&state, &agent); // the members' apps drop it at once
        }
        crate::agent_groups::set_group(&state.config(), &agent.name, "")?;
        crate::connectors::forget(&state.config(), &agent.name); // its connectors (connectors.rs)
        crate::secrets::forget(&agent);
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
        .route("/agents/{agent_id}", axum::routing::delete(remove))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_command_says_why() {
        assert_eq!(why_failed(b"", b"boom\n"), "boom");
        let hermes = "Deleting coach\nError: a running gateway still owns this profile; retry the delete once it settles\n";
        assert_eq!(
            why_failed(hermes.as_bytes(), b""),
            "Error: a running gateway still owns this profile; retry the delete once it settles"
        );
        // never the rest of its output (what it was given may be there)
        assert_eq!(why_failed(b"Set API_SERVER_KEY = sk-secret\n", b""), "(no error output)");
    }

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
