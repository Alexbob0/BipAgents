//! Each agent's model, chosen in its settings, on an install of the person's own (their computer or VPS; Alex
//! 2026-10-31). Never on the all-included offer, whose model is Kerfy's (offer-model-fixed): there the settings carry
//! no `model_choice` and these routes answer 404.
//!
//! - **default**: the install's model, the one every agent is cloned from (the hub's). « Back to the default »
//!   copies the hub's model and key again.
//! - **a provider of the whitelist** ([`PRESETS`]; the apps hold the same list and show only what both know): the
//!   connectors Hermes reaches with an API key (OpenRouter, OpenAI, Anthropic, Mistral…), never one Hermes adds later
//!   by itself. Subscription sign-ins (Claude Pro/Max, ChatGPT through Codex) are not offered: an agent already set
//!   that way on Hermes is shown only as « set on your Hermes » ([`hermes_only`]), without its name or a setting.
//! - **custom**: an OpenAI-compatible server, its address, the model's id and an optional key.
//!
//! A model is tried with one short call before it is used. Its key goes where Hermes keeps every model key, the
//! profile's `.env` (owner-only), in a variable of its own ([`OWN_KEY_ENV`], or Hermes' `ANTHROPIC_API_KEY`) so the
//! install's own key never overwrites it; the apps never get it back (only « …abcd »), no log holds it, and choosing
//! the default again deletes it. The choice is kept next to the profile ([`CHOICE_FILE`], no secret in it).
//! A turn that fails on that model ends with one plain sentence, never an empty turn ([`failure_line`]).
//!
//! No Python counterpart.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents_admin::pyyaml::Yaml;
use crate::agents_admin::{editable, profile_folder};
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};

/// Next to a profile's config: the person's choice when it is not the default ([`Chosen`]); no secret in it.
pub const CHOICE_FILE: &str = ".bipagents-model.json";
/// The variable an agent's own key is kept in (its profile's `.env`): never the hub's, so the install's key can't
/// overwrite it.
pub const OWN_KEY_ENV: &str = "BIPAGENTS_AGENT_MODEL_KEY";
/// Hermes reads Anthropic's key there.
const ANTHROPIC_ENV: &str = "ANTHROPIC_API_KEY";

/// The whitelist: the connectors the apps may show (Hermes' providers reached with an API key), and « custom ».
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    pub base_url: &'static str,
    /// needs a key (custom: optional)
    pub key: bool,
}

pub const PRESETS: [Preset; 9] = [
    Preset { id: "openrouter", base_url: "https://openrouter.ai/api/v1", key: true },
    Preset { id: "openai", base_url: "https://api.openai.com/v1", key: true },
    Preset { id: "anthropic", base_url: "https://api.anthropic.com/v1", key: true },
    Preset { id: "mistral", base_url: "https://api.mistral.ai/v1", key: true },
    Preset { id: "eurouter", base_url: "https://api.eurouter.ai/api/v1", key: true },
    Preset { id: "deepseek", base_url: "https://api.deepseek.com/v1", key: true },
    Preset { id: "gemini", base_url: "https://generativelanguage.googleapis.com/v1beta/openai", key: true },
    Preset { id: "xai", base_url: "https://api.x.ai/v1", key: true },
    Preset { id: "custom", base_url: "", key: false },
];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The person's choice when it is not the install's default. No secret: the key is in the profile's `.env`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Chosen {
    /// a preset's id (« custom » included)
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// the key's last four characters
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hint: Option<String>,
    #[serde(default)]
    pub vision: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
}

pub fn chosen_in(folder: &Path) -> Option<Chosen> {
    let text = std::fs::read_to_string(folder.join(CHOICE_FILE)).ok()?;
    serde_json::from_str::<Chosen>(&text).ok().filter(|c| !c.provider.is_empty())
}

pub fn chosen(config: &Config, agent: &AgentConfig) -> Option<Chosen> {
    chosen_in(&profile_folder(config, agent))
}

/// Whether this install offers the choice: an install of the person's own, not a place of the offer.
pub fn offered() -> bool {
    !crate::handover::on_offer()
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".tmp");
    let temporary = path.with_file_name(name);
    let _ = std::fs::remove_file(&temporary);
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
        file.write_all(text.as_bytes())?;
    }
    std::fs::rename(&temporary, path)
}

/// « …abcd »: what the apps show of a key.
pub fn hint(key: &str) -> String {
    let chars: Vec<char> = key.trim().chars().collect();
    let tail: String = chars[chars.len().saturating_sub(4)..].iter().collect();
    format!("…{tail}")
}

fn env_has(path: &Path, key: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines().any(|line| {
            let name = line.split('=').next().unwrap_or("").trim();
            name.strip_prefix("export ").map(str::trim).unwrap_or(name) == key
        })
    })
}

fn env_value(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rev().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        let name = name.trim();
        (name.strip_prefix("export ").map(str::trim).unwrap_or(name) == key).then(|| value.trim().to_string())
    })
}

fn unset_env(path: &Path, key: &str) -> std::io::Result<()> {
    if env_has(path, key) {
        crate::services::set_env(path, key, None)?;
    }
    Ok(())
}

/// The profile's `model` section rewritten by `change`, then its routing (EUrouter's providers when it is the
/// router's address, none otherwise: model_route.rs).
fn change_model(config: &Config, folder: &Path, change: impl FnOnce(&mut Yaml)) -> anyhow::Result<()> {
    let path = folder.join("config.yaml");
    let mut data = crate::agents_admin::load_or_empty(&path)?;
    if !data.is_map() {
        data = Yaml::map();
    }
    let mut section = data.get("model").filter(|m| m.is_map()).cloned().unwrap_or_else(Yaml::map);
    change(&mut section);
    data.insert("model", section);
    let level = crate::agents_admin::chosen_thinking(folder);
    if let Some(routed) = crate::model_route::with_routing(&data, &config.model_provider_order, level.as_deref()) {
        data = routed;
    }
    if path.exists() {
        crate::agents_admin::replace_yaml(&path, &data)?;
    } else {
        std::fs::create_dir_all(folder)?;
        std::fs::write(&path, crate::agents_admin::pyyaml::dump(&data))?;
    }
    Ok(())
}

/// The hub's model (the install's default), its id.
pub fn default_model(config: &Config) -> Option<String> {
    let home = PathBuf::from(crate::agents_admin::install_home(config));
    let hub = crate::agents_admin::load_or_empty(&home.join("config.yaml")).ok()?;
    hub.get("model").and_then(|m| m.get("default")).filter(|d| d.truthy()).map(Yaml::py_str)
}

/// Hermes' providers the apps may show: the others (a subscription sign-in, Codex, Nous…) are « set on your Hermes ».
const SHOWN_PROVIDERS: [&str; 4] = ["custom", "openrouter", "lmstudio", "anthropic"];

/// Whether a profile's model is set on Hermes in a mode the apps don't offer: a provider outside
/// [`SHOWN_PROVIDERS`], or Anthropic signed in with a Claude subscription (Hermes' `.anthropic_oauth.json`, an OAuth
/// token in its `.env`) rather than with a key.
pub fn hermes_only_in(folder: &Path) -> bool {
    let Ok(data) = crate::agents_admin::load_or_empty(&folder.join("config.yaml")) else { return false };
    let Some(provider) = data.get("model").and_then(|m| m.get("provider")).and_then(Yaml::as_str).map(str::to_string) else {
        return false;
    };
    if !SHOWN_PROVIDERS.contains(&provider.as_str()) {
        return true;
    }
    provider == "anthropic"
        && chosen_in(folder).is_none()
        && (folder.join(".anthropic_oauth.json").exists()
            || env_has(&folder.join(".env"), "CLAUDE_CODE_OAUTH_TOKEN")
            || env_has(&folder.join(".env"), "ANTHROPIC_TOKEN"))
}

pub fn hermes_only(config: &Config, agent: &AgentConfig) -> bool {
    hermes_only_in(&profile_folder(config, agent))
}

// -- choices -------------------------------------------------------------------------------------------------------

/// The agent on a provider of the whitelist (or a custom server), from its next message. `key`: None keeps the one
/// it has, "" means none.
pub fn use_model(
    config: &Config,
    agent: &AgentConfig,
    preset: &Preset,
    base_url: &str,
    model: &str,
    key: Option<&str>,
    vision: bool,
) -> anyhow::Result<()> {
    let folder = profile_folder(config, agent);
    let before = chosen_in(&folder);
    let env = folder.join(".env");
    let anthropic = preset.id == "anthropic";
    let var = if anthropic { ANTHROPIC_ENV } else { OWN_KEY_ENV };
    let kept = match key.map(str::trim) {
        None => before.as_ref().and_then(|c| c.key_hint.clone()).and_then(|h| {
            // the key it had, moved to this provider's variable when it changes
            let old_var = if before.as_ref().is_some_and(|c| c.provider == "anthropic") { ANTHROPIC_ENV } else { OWN_KEY_ENV };
            env_value(&env, old_var).map(|k| (k, h))
        }),
        Some("") => None,
        Some(k) => Some((k.to_string(), hint(k))),
    };
    for name in [OWN_KEY_ENV, ANTHROPIC_ENV, "ANTHROPIC_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"] {
        unset_env(&env, name)?; // only this agent's own key stays
    }
    if let Some((key, _)) = &kept {
        crate::services::set_env(&env, var, Some(key))?;
    }
    let has_key = kept.is_some();
    change_model(config, &folder, |m| {
        m.insert("default", Yaml::str(model));
        if anthropic {
            m.insert("provider", Yaml::str("anthropic"));
            m.remove("base_url");
            m.remove("key_env");
        } else {
            m.insert("provider", Yaml::str("custom"));
            m.insert("base_url", Yaml::str(base_url));
            if has_key {
                m.insert("key_env", Yaml::str(OWN_KEY_ENV));
            } else {
                m.remove("key_env");
            }
        }
        m.insert("supports_vision", Yaml::Bool(vision));
    })?;
    let chosen = Chosen {
        provider: preset.id.into(),
        model: model.into(),
        base_url: (!anthropic).then(|| base_url.to_string()),
        key_hint: kept.map(|(_, h)| h),
        vision,
        since: Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
    };
    write_private(&folder.join(CHOICE_FILE), &serde_json::to_string_pretty(&chosen).unwrap_or_default())?;
    Ok(())
}

/// Back to the install's default: its own key deleted, the hub's model and key copied again, with the routing and
/// thinking floor any agent of the install gets.
pub fn back_to_default(config: &Config, agent: &AgentConfig) -> anyhow::Result<()> {
    let folder = profile_folder(config, agent);
    let env = folder.join(".env");
    unset_env(&env, OWN_KEY_ENV)?;
    if chosen_in(&folder).is_some_and(|c| c.provider == "anthropic") {
        unset_env(&env, ANTHROPIC_ENV)?; // a hub on Anthropic with a key: copied again below
    }
    crate::agent_pool::follow_hub_model(config, &folder)?;
    change_model(config, &folder, |_| {})?;
    match std::fs::remove_file(folder.join(CHOICE_FILE)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
        _ => {}
    }
    crate::agents_admin::heal_thinking_of(config, agent);
    Ok(())
}

// -- what the app sees ---------------------------------------------------------------------------------------------

/// The agent's model as the apps show it (`GET /v1/agents/{id}/model-choice`, and `model_choice` in its settings:
/// the field tells the apps this bridge offers the choice). None on the offer. Never a key: « …abcd ».
pub fn view(config: &Config, agent: &AgentConfig) -> Option<Value> {
    if !offered() || agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) {
        return None;
    }
    if hermes_only(config, agent) {
        return Some(json!({"choice": "hermes"})); // « Modèle réglé sur ton Hermes »: no name, no setting
    }
    let chosen = chosen(config, agent);
    Some(json!({
        "choice": if chosen.is_some() { "own" } else { "default" },
        "provider": chosen.as_ref().map(|c| c.provider.clone()),
        "model": chosen.as_ref().map(|c| c.model.clone()).or_else(|| default_model(config)),
        "base_url": chosen.as_ref().and_then(|c| c.base_url.clone()),
        "key_hint": chosen.as_ref().and_then(|c| c.key_hint.clone()),
        "default_model": default_model(config),
        "providers": PRESETS.iter().map(|p| p.id).collect::<Vec<_>>(),
    }))
}

// -- a turn that failed --------------------------------------------------------------------------------------------

/// The plain sentence a failed turn ends with, in the agent's language.
pub fn failure_sentence(agent: &str, lang: &str) -> String {
    match lang {
        "en" => format!("{agent}'s model didn't answer (key refused, server out of reach, or no funds left). Check it in {agent}'s settings (Model), or go back to the default model."),
        "de" => format!("Das Modell von {agent} hat nicht geantwortet (Schlüssel abgelehnt, Server nicht erreichbar oder kein Guthaben). Prüf es in den Einstellungen von {agent} (Modell), oder nimm wieder das Standardmodell."),
        "es" => format!("El modelo de {agent} no ha respondido (clave rechazada, servidor inaccesible o sin saldo). Revísalo en los ajustes de {agent} (Modelo), o vuelve al modelo por defecto."),
        _ => format!("Le modèle de {agent} n’a pas répondu (clé refusée, serveur injoignable ou plus de fonds). Vérifie-le dans les réglages de {agent} (Modèle), ou reviens au modèle par défaut."),
    }
}

/// A failed turn of an agent on a model chosen here: its plain sentence (the apps show it as it is). None for any
/// other agent (the apps' own words).
pub fn failure_line(config: &Config, agent: &AgentConfig) -> Option<String> {
    let chosen = chosen(config, agent)?;
    tracing::warn!(agent = %agent.name, provider = %chosen.provider, "turn failed on the agent's own model");
    let lang = crate::handback::language_of(config, agent);
    let name = if agent.display_name.trim().is_empty() { &agent.name } else { &agent.display_name };
    Some(failure_sentence(name, &lang))
}

// -- a model checked -----------------------------------------------------------------------------------------------

/// Why a model can't be used: `key_refused` (401/403), `unreachable`, `model` (anything else), with the status.
fn status_problem(status: reqwest::StatusCode) -> &'static str {
    match status.as_u16() {
        401 | 403 => "key_refused",
        _ => "model",
    }
}

/// One short call: does it answer with this key; does it read images (OpenAI-compatible: a tiny image; Anthropic: yes).
async fn check_model(http: &reqwest::Client, preset: &Preset, base_url: &str, model: &str, key: Option<&str>) -> Result<bool, Value> {
    let key = key.map(str::trim).filter(|k| !k.is_empty());
    let hello = "Réponds juste : OK";
    let failed = |why: &str, status: Option<u16>| json!({"error": why, "status": status});
    if preset.id == "anthropic" {
        let resp = http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", key.unwrap_or(""))
            .header("anthropic-version", "2023-06-01")
            .json(&json!({"model": model, "max_tokens": 8, "messages": [{"role": "user", "content": hello}]}))
            .timeout(Duration::from_secs(55))
            .send()
            .await
            .map_err(|_| failed("unreachable", None))?;
        return if resp.status().is_success() {
            Ok(true)
        } else {
            Err(failed(status_problem(resp.status()), Some(resp.status().as_u16())))
        };
    }
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    // OpenAI's own models take max_completion_tokens (its reasoning models refuse max_tokens)
    let (limit, tokens) = if preset.id == "openai" { ("max_completion_tokens", 64) } else { ("max_tokens", 8) };
    let send = |content: Value| {
        let mut body = json!({"model": model, "messages": [{"role": "user", "content": content}]});
        body[limit] = json!(tokens);
        let mut request = http.post(&url).json(&body).timeout(Duration::from_secs(55));
        if let Some(key) = key {
            request = request.bearer_auth(key);
        }
        request.send()
    };
    let resp = send(json!(hello)).await.map_err(|_| failed("unreachable", None))?;
    if !resp.status().is_success() {
        return Err(failed(status_problem(resp.status()), Some(resp.status().as_u16())));
    }
    // a tiny red square: a model that refuses images answers 400 (installer/bipinstall/models.py's check)
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEklEQVR4nGP4z8CAFWEXHbQSACj/P8Fu7N9hAAAAAElFTkSuQmCC";
    let image = json!([{"type": "text", "text": "Quelle couleur ? Un mot."},
                       {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{png}")}}]);
    Ok(send(image).await.is_ok_and(|r| r.status().is_success()))
}

// -- routes --------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct ChoiceBody {
    /// « default », or a provider of the whitelist (« custom » included)
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    /// absent: the key it has; "": none
    #[serde(default)]
    pub key: Option<String>,
}

impl Validate for ChoiceBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "provider", Some(&self.provider), 32);
        check_max_len(errors, "model", self.model.as_deref(), 200);
        check_max_len(errors, "base_url", self.base_url.as_deref(), 300);
        check_max_len(errors, "key", self.key.as_deref(), 400);
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsBody {
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub key: Option<String>,
}

impl Validate for ModelsBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "provider", Some(&self.provider), 32);
        check_max_len(errors, "base_url", self.base_url.as_deref(), 300);
        check_max_len(errors, "key", self.key.as_deref(), 400);
    }
}

/// The agent, on an install that offers the choice (404 on the offer: no such route there).
fn agent_of(state: &AppState, id: &str) -> ApiResult<AgentConfig> {
    if !offered() {
        return Err(HttpError::not_found());
    }
    let config = state.config();
    let agent = editable(&config, id).map_err(|_| HttpError::new(404, "not an agent this install manages"))?;
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) || hermes_only(&config, &agent) {
        return Err(HttpError::json(409, json!({"error": "set_on_hermes"})));
    }
    Ok(agent)
}

/// The address: the preset's, or the custom server's (http/https).
fn address(preset: &Preset, given: Option<&str>) -> ApiResult<String> {
    if preset.id != "custom" {
        return Ok(preset.base_url.into());
    }
    let url = given.map(str::trim).unwrap_or("").trim_end_matches('/').to_string();
    let parsed = url::Url::parse(&url).map_err(|_| HttpError::new(422, "the server's address isn't valid"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(HttpError::new(422, "the server's address isn't valid"));
    }
    Ok(url)
}

/// The key to try a model with: the one typed, or the one the agent has for the same provider and address.
fn key_to_try(config: &Config, agent: &AgentConfig, preset: &Preset, base_url: &str, typed: Option<&str>) -> Option<String> {
    if let Some(k) = typed.map(str::trim).filter(|k| !k.is_empty()) {
        return Some(k.to_string());
    }
    let before = chosen(config, agent).filter(|c| c.provider == preset.id && c.key_hint.is_some())?;
    if preset.id == "custom" && before.base_url.as_deref() != Some(base_url) {
        return None;
    }
    let var = if preset.id == "anthropic" { ANTHROPIC_ENV } else { OWN_KEY_ENV };
    env_value(&profile_folder(config, agent).join(".env"), var)
}

async fn get_choice(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    if !offered() {
        return Err(HttpError::not_found());
    }
    let config = state.config();
    let agent = editable(&config, &agent_id).map_err(|_| HttpError::new(404, "not an agent this install manages"))?;
    Ok(web::ok(view(&config, &agent).unwrap_or(Value::Null)))
}

async fn put_choice(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<ChoiceBody>,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    let config = state.config();
    if body.provider == "default" {
        let _guard = crate::agents_admin::LOCK.lock().await;
        back_to_default(&config, &agent)?;
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, "agent back on the install's model");
        return Ok(web::ok(view(&config, &agent).unwrap_or(Value::Null)));
    }
    let preset = preset(&body.provider).ok_or_else(|| HttpError::new(422, "not a provider offered here"))?;
    let model = body.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
    let model = model.ok_or_else(|| HttpError::new(422, "a model is needed"))?.to_string();
    let base_url = address(preset, body.base_url.as_deref())?;
    let key = key_to_try(&config, &agent, preset, &base_url, body.key.as_deref());
    if preset.key && key.is_none() {
        return Err(HttpError::new(422, "this provider needs a key"));
    }
    let vision = check_model(&state.http, preset, &base_url, &model, key.as_deref()).await.map_err(|why| HttpError::json(400, why))?;
    let given = match body.key.as_deref().map(str::trim) {
        Some(k) if !k.is_empty() => Some(k.to_string()),
        Some(_) => Some(String::new()),
        None if key.is_some() => Some(key.clone().unwrap_or_default()),
        None => Some(String::new()),
    };
    {
        let _guard = crate::agents_admin::LOCK.lock().await;
        use_model(&config, &agent, preset, &base_url, &model, given.as_deref(), vision)?;
    }
    tracing::info!(target: "bipbridge::agents", agent = %agent.name, provider = preset.id, model = %model, "agent model chosen");
    Ok(web::ok(view(&config, &agent).unwrap_or(Value::Null)))
}

/// The models a provider lists with this key: ids only.
async fn post_models(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<ModelsBody>,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &agent_id)?;
    let preset = preset(&body.provider).ok_or_else(|| HttpError::new(422, "not a provider offered here"))?;
    let base = address(preset, body.base_url.as_deref())?;
    let key = key_to_try(&state.config(), &agent, preset, &base, body.key.as_deref());
    let mut request = state.http.get(format!("{}/models", base.trim_end_matches('/'))).timeout(Duration::from_secs(20));
    if preset.id == "anthropic" {
        request = request.header("x-api-key", key.as_deref().unwrap_or("")).header("anthropic-version", "2023-06-01");
    } else if let Some(key) = key.as_deref() {
        request = request.bearer_auth(key);
    }
    let resp = request.send().await.map_err(|_| HttpError::json(400, json!({"error": "unreachable"})))?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Err(HttpError::json(400, json!({"error": status_problem(status), "status": status.as_u16()})));
    }
    let data: Value = resp.json().await.map_err(|_| HttpError::json(400, json!({"error": "unreachable"})))?;
    let mut ids: Vec<String> = data
        .get("data")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(|m| m.get("id").and_then(Value::as_str)).map(str::to_string).collect())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(web::ok(json!({"models": ids})))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/model-choice", get(get_choice).put(put_choice))
        .route("/agents/{agent_id}/model-choice/models", post(post_models))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_shows_only_its_end() {
        assert_eq!(hint("sk-proj-1234567890abcd"), "…abcd");
        assert_eq!(hint("ab"), "…ab");
    }

    #[test]
    fn failure_sentences_are_plain_and_never_the_credit_s() {
        for lang in ["fr", "en", "de", "es"] {
            let line = failure_sentence("Vie", lang);
            assert!(line.chars().next().unwrap().is_uppercase() && line.ends_with('.'), "{line}");
            // the apps show the plan's credit card, or their own words, on these (ConversationModel.failure)
            for word in ["402", "insufficient credit", "key limit", "limit_source", "spend limit", "http", "timeout", "{"] {
                assert!(!line.to_lowercase().contains(word), "{line}");
            }
        }
    }

    #[test]
    fn subscription_sign_ins_are_only_set_on_hermes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        let set = |text: &str| std::fs::write(folder.join("config.yaml"), text).unwrap();
        set("model:\n  provider: openai-codex\n  default: gpt-6.1-sol\n");
        assert!(hermes_only_in(folder)); // ChatGPT through Codex
        set("model:\n  provider: custom\n  base_url: http://gpu:8888/v1\n");
        assert!(!hermes_only_in(folder));
        set("model:\n  provider: anthropic\n  default: claude-sonnet-5-5\n");
        assert!(!hermes_only_in(folder)); // with a key
        std::fs::write(folder.join(".anthropic_oauth.json"), "{}").unwrap();
        assert!(hermes_only_in(folder)); // a Claude subscription signed in on Hermes
        std::fs::remove_file(folder.join(".anthropic_oauth.json")).unwrap();
        std::fs::write(folder.join(".env"), "CLAUDE_CODE_OAUTH_TOKEN=x\n").unwrap();
        assert!(hermes_only_in(folder));
    }

    #[test]
    fn the_whitelist_is_closed() {
        assert!(preset("openrouter").is_some() && preset("custom").is_some());
        for hidden in ["openai-codex", "nous", "copilot", "claude-subscription", ""] {
            assert!(preset(hidden).is_none(), "{hidden}");
        }
    }
}
