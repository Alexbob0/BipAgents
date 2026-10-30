//! « L'agent de projet » (decided by Alex on 2026-10-28): an agent made for one occasion — a trip, a party, a move —
//! whose whole thread the person shows read-only to up to 20 friends (read_shares.rs, a share of the agent's thread).
//! People use one continuous thread per agent, and their usual agent's thread holds private things: what they share is
//! a project agent, never their usual thread. Design: internal/project-agent.md.
//!
//! **What it reaches.** Like the family's agent (family_agent.rs), it starts clean and closed: for ALL its turns the
//! bridge's MCP tools answer only `TOOLS` (its own memory pages, weather, routes, maps, create_page), everything else
//! is closed (`closed_tool`), `tools/list` gives only those (`tools`), Hermes' disk and browser tools are turned off
//! when it is made (agents_admin::make_profile), its visible browser is not set up, and the settings refuse to turn
//! them, Santé or the Carnet back on. Web search stays. Its personality carries a block saying friends see its thread
//! (`with_project`). Its memory: what is said in it, and the summary its person approved when it came from another
//! agent (guide.rs' `project` card; written as information, not instructions).
//!
//! **Its end.** It has an end date (`ends`). Every minute, a live project past its end has its share stopped (deleted
//! at the guichet, so in the friends' apps) and is archived: kept with its thread, off the home screen, listed under
//! « Projets terminés », restorable (a new end date, not shared again until the person invites again) or deletable
//! (the usual agent deletion).
//!
//! `<data>/project-agents.json` (0600): `{"projects": {agent: {created, ends, archived, from, lang}}}`.
//!
//!     GET  /v1/project-agents                   {projects: [view]}
//!     POST /v1/project-agents                   {name, ends, lang?, category?, color?, summary?}  → 201 view
//!          (category + color: its Bip, kept as with PUT /v1/agents/{id}/appearance; the hand-over card's accept
//!          takes the same two, guide.rs)
//!     POST /v1/project-agents/{agent}/ends      {ends} → view
//!     POST /v1/project-agents/{agent}/archive   → view
//!     POST /v1/project-agents/{agent}/restore   {ends?} → view
//!
//!     view {agent, name, category, color, ends, created, archived, from, share: read-share view | null}
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The bridge's MCP tools a project agent may use, in every turn.
pub const TOOLS: [&str; 7] = [
    crate::agent_tools::KNOWLEDGE_READ,
    crate::agent_tools::KNOWLEDGE_SEARCH,
    crate::weather::TOOL_NAME,
    crate::routes::TOOL_NAME,
    crate::maps::TOOL_NAME,
    crate::agent_tools::CREATE_PAGE,
    crate::friends_cards::TOOL_NAME,
];
/// The default length of a project (and of a restored one).
pub const DEFAULT_DAYS: f64 = 30.0;
/// The furthest end date (the guichet keeps a share at most a year).
pub const MAX_DAYS: f64 = 366.0;
/// How often ended projects are looked for.
pub const CHECK_SECONDS: u64 = 60;
pub const SUMMARY_MAX: usize = 1200;

pub const BLOCK_START: &str = "<!-- bipagents:project -->";
pub const BLOCK_END: &str = "<!-- /bipagents:project -->";
const RULE: &str = "You are a project agent: your person made you for one occasion (a trip, a party, a move…). \
Friends your person invited may follow this whole thread read-only: everything you write here and every page you \
make, they see. Keep to the project. You reach nobody's mail, calendar, contacts, health data, notebook, archives, \
files, vault or logins, not even your person's: if asked, say kindly that a project agent has no access to them, and \
never write private details about anyone here. Your memory is only what is said here. Web search, weather, routes, \
maps and pages are open to you: show plans, budgets, lists and places as pages (create_page, with map_image for \
places), so the friends see them too. Friends can't write here, but they can tap your friends_card cards: use a \
vote when the group has to choose, a checklist to share tasks; you hear of their taps at your next turn.";

static LOCK: Mutex<()> = Mutex::new(());

fn now() -> f64 {
    py::time_time()
}

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

// -- the file ---------------------------------------------------------------------------------------------------------

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("project-agents.json")
}

fn load(path: &PathBuf) -> Dict {
    let mut data: Dict = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    if !matches!(data.get("projects"), Some(Value::Object(_))) {
        data.insert("projects".into(), json!({}));
    }
    data
}

fn save(path: &PathBuf, data: &Dict) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    py::write_with_mode(&tmp.to_string_lossy(), Value::Object(data.clone()).to_string().as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The project entry of `agent`, changed under the lock (None: it goes). Returns it as kept.
fn update(
    config: &Config,
    agent: &str,
    change: impl FnOnce(Option<Value>) -> Option<Value>,
) -> anyhow::Result<Option<Value>> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let file = path(config);
    let mut data = load(&file);
    let projects = data.get_mut("projects").and_then(Value::as_object_mut).expect("an object");
    let out = change(projects.get(agent).cloned());
    match &out {
        Some(v) => projects.insert(agent.to_string(), v.clone()),
        None => projects.remove(agent),
    };
    save(&file, &data)?;
    Ok(out)
}

fn all(config: &Config) -> serde_json::Map<String, Value> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(&path(config)).get("projects").and_then(Value::as_object).cloned().unwrap_or_default()
}

/// The project entry of `agent`, if it is a project agent.
pub fn entry(config: &Config, agent: &str) -> Option<Value> {
    if agent.is_empty() {
        return None;
    }
    all(config).get(agent).cloned()
}

/// Whether `agent` (an agent of this place) is a project agent.
pub fn is_project(config: &Config, agent: &str) -> bool {
    entry(config, agent).is_some()
}

/// What the agents' listing says of it: `{ends, archived}`, or null.
pub fn listing_mark(config: &Config, agent: &str) -> Value {
    match entry(config, agent) {
        Some(e) => json!({"ends": e["ends"], "archived": e["archived"]}),
        None => Value::Null,
    }
}

/// Its creation starts: marked before the profile is made (its tools are set up closed from the start).
fn mark(config: &Config, agent: &str, ends: f64, lang: &str, from: Option<&str>) -> anyhow::Result<()> {
    update(config, agent, |_| {
        Some(json!({"created": now(), "ends": ends, "archived": null, "lang": lang, "from": from}))
    })?;
    Ok(())
}

/// A project's end date changed (its share's too, by the caller).
pub fn set_ends_of(config: &Config, agent: &str, ends: f64) -> anyhow::Result<()> {
    update(config, agent, |e| {
        e.map(|mut e| {
            e["ends"] = json!(ends);
            e
        })
    })?;
    Ok(())
}

/// The agent deleted (or its creation failed): its entry goes.
pub fn forget(config: &Config, agent: &str) {
    if is_project(config, agent) {
        let _ = update(config, agent, |_| None);
        tracing::info!(agent = %agent, "project agent: forgotten");
    }
}

// -- what it reaches ------------------------------------------------------------------------------------------------

/// The answer a closed tool gives, in every turn of a project agent.
pub fn closed_tool(name: &str) -> Option<Dict> {
    if TOOLS.contains(&name) {
        return None;
    }
    Some(tk::tool_text(
        "You are a project agent shown to friends: nobody's mail, calendar, contacts, health, notebook, archives, \
         files or tasks are open to you, not even your person's. Answer from what is said here, the web, weather, \
         routes, maps and pages.",
        true,
    ))
}

/// Its `tools/list`: only `TOOLS`.
pub fn tools(config: &Config) -> Vec<Value> {
    let mut out: Vec<Value> = crate::agent_tools::install_tools(config)
        .iter()
        .filter(|t| t.get("name").and_then(Value::as_str).is_some_and(|n| TOOLS.contains(&n)))
        .cloned()
        .collect();
    if !out.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(crate::weather::TOOL_NAME)) {
        out.push(crate::weather::weather_tool());
    }
    out.push(crate::friends_cards::tool()); // the friends' votes and checklists (friends_cards.rs)
    out
}

/// Whether a settings change would open something private to it (the family's agent's rule).
pub fn refused_setting(toolsets: &indexmap::IndexMap<String, bool>, browser: Option<bool>, private: bool) -> bool {
    crate::family_agent::refused_setting(toolsets, browser, private)
}

/// Its personality's block.
pub fn with_project(soul: &str, on: bool) -> String {
    let (rest, _) = split_project(soul);
    if !on {
        return rest;
    }
    format!("{}\n\n{BLOCK_START}\n{RULE}\n{BLOCK_END}\n", rest.trim_end())
}

pub fn split_project(soul: &str) -> (String, bool) {
    let (Some(start), Some(end)) = (soul.find(BLOCK_START), soul.find(BLOCK_END)) else {
        return (soul.to_string(), false);
    };
    if end < start {
        return (soul.to_string(), false);
    }
    let rest = format!("{}{}", soul[..start].trim_end(), &soul[end + BLOCK_END.len()..]);
    (format!("{}\n", rest.trim_end()), true)
}

fn role(lang: &str, name: &str) -> String {
    match lang.get(..2).unwrap_or("fr") {
        "fr" => format!(
            "l'agent du projet « {name} » : préparer ce projet avec ton utilisateur et ses amis, garder le plan, les \
             dates, le budget, les idées et les lieux à jour, en pages et en cartes"
        ),
        "de" => format!(
            "der Agent des Projekts „{name}“: dieses Projekt mit deiner Person und ihren Freunden vorbereiten, Plan, \
             Termine, Budget, Ideen und Orte aktuell halten, als Seiten und Karten"
        ),
        "es" => format!(
            "el agente del proyecto «{name}»: preparar este proyecto con tu persona y sus amigos, mantener al día el \
             plan, las fechas, el presupuesto, las ideas y los lugares, en páginas y mapas"
        ),
        _ => format!(
            "the agent of the project “{name}”: prepare it with your person and their friends, keep the plan, dates, \
             budget, ideas and places up to date, as pages and maps"
        ),
    }
}

// -- the views --------------------------------------------------------------------------------------------------------

/// A project as the apps show it.
pub fn view(config: &Config, agent: &AgentConfig, entry: &Value) -> Value {
    let share = crate::read_shares::thread_share(config, &agent.name).map(|s| crate::read_shares::view(&s));
    let from = entry
        .get("from")
        .and_then(Value::as_str)
        .map(|f| config.agent(Some(f)).map(|a| a.display_name.clone()).unwrap_or_else(|| f.to_string()));
    json!({"agent": agent.name, "name": agent.display_name, "category": agent.category, "color": agent.color,
           "ends": entry["ends"], "created": entry["created"], "archived": entry["archived"], "from": from,
           "share": share})
}

fn view_of(config: &Config, agent: &str) -> ApiResult<Value> {
    let found = config.agent(Some(agent)).ok_or_else(|| HttpError::new(404, "no such project"))?;
    let entry = entry(config, agent).ok_or_else(|| HttpError::new(404, "no such project"))?;
    Ok(view(config, found, &entry))
}

/// An end date given by the apps: in the future, within a year.
pub fn ends_ok(ends: Option<f64>) -> ApiResult<f64> {
    let at = now();
    ends.filter(|e| *e > at && *e <= at + MAX_DAYS * 86400.0)
        .ok_or_else(|| HttpError::new(422, "ends: a date in the future, within a year"))
}

fn body_json(bytes: &[u8]) -> ApiResult<Dict> {
    if bytes.is_empty() {
        return Ok(Dict::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

// -- creating ---------------------------------------------------------------------------------------------------------

/// What a new project needs.
pub struct NewProject {
    pub name: String,
    pub ends: f64,
    pub lang: String,
    /// Its Bip: one of `souls::CATEGORIES` (kept as its look when given), and a colour `#RRGGBB` (else the category's).
    pub category: String,
    pub color: Option<String>,
    /// What its person approved to hand over (guide.rs' card), kept in its memory as information.
    pub summary: String,
    /// The agent it came from (its id), if any.
    pub from: Option<AgentConfig>,
}

/// A Bip colour as the apps send it (`#RRGGBB`), upper-cased; None for anything else.
pub fn color_ok(color: Option<&str>) -> Option<String> {
    let c = color?.trim();
    let hex = c.strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|h| h.is_ascii_hexdigit())).then(|| c.to_uppercase())
}

/// A project agent made: closed tools from the start, its block, the summary in its memory.
pub async fn create(state: &AppState, new: NewProject) -> ApiResult<(AgentConfig, Value)> {
    let config = state.config();
    if !config.hermes.enabled() {
        return Err(HttpError::new(409, "this install does not manage its agents (set up by hand)"));
    }
    let name = tk::squeeze(&new.name);
    if name.is_empty() || name.chars().count() > 40 {
        return Err(HttpError::new(422, "name: 1 to 40 characters"));
    }
    let picked = crate::souls::CATEGORIES.contains(&new.category.as_str());
    let category = if picked { new.category.clone() } else { "daily".into() };
    let request = crate::agents_admin::NewAgent {
        name: name.clone(),
        lang: Some(new.lang.clone()),
        custom: Some(crate::agents_admin::CustomAgent {
            role: role(&new.lang, &name),
            tone: String::new(),
            rules: String::new(),
            language: String::new(),
            category: category.clone(),
        }),
        ..Default::default()
    };
    let from = new.from.as_ref().map(|a| a.name.clone());
    let _guard = crate::agents_admin::LOCK.lock().await;
    crate::agents_admin::check_room(&config)?;
    let planned = crate::agents_admin::plan_agent(&config, &request).map_err(|e| HttpError::new(400, e.to_string()))?;
    mark(&config, &planned.profile, new.ends, &new.lang, from.as_deref())?;
    let made =
        crate::agents_admin::create_agent(&state.config, &request, &state.http, &crate::agents_admin::default_run())
            .await;
    let agent = match made {
        Ok(agent) => {
            if agent.name != planned.profile {
                // another name was taken meanwhile: the mark follows the agent made
                forget(&state.config(), &planned.profile);
                mark(&state.config(), &agent.name, new.ends, &new.lang, from.as_deref())?;
            }
            agent
        }
        Err(exc) => {
            forget(&state.config(), &planned.profile);
            if crate::agents_admin::too_many(&exc) {
                return Err(exc.into());
            }
            tracing::warn!(error = %format!("{exc:#}"), "project agent: creation failed");
            return Err(HttpError::new(502, "the agent could not be created: try again in a moment"));
        }
    };
    // its own Bip, picked by the person (« Un projet à partager » or the hand-over card): a new agent, its own face
    let agent = if picked {
        let (outfit, costumes) = (agent.outfit.clone(), agent.costumes);
        match crate::voices::keep_appearance(state, agent.clone(), &category, new.color.clone(), outfit, costumes)
            .await
        {
            Ok(kept) => kept,
            Err(_) => {
                tracing::warn!(agent = %agent.name, "project agent: its look was not kept");
                agent
            }
        }
    } else {
        agent
    };
    let config = state.config();
    // the weather is one of its open tools (off by default for other agents)
    if let Err(err) = crate::weather::set_enabled(&config, &agent.name, true) {
        tracing::warn!(error = %err, "project agent: the weather not switched on");
    }
    // its personality with its block
    let soul = crate::agents_admin::editable_soul(&config, &agent).unwrap_or_default();
    if let Err(err) = crate::agents_admin::write_soul(&config, &agent, &soul) {
        tracing::warn!(error = %err, "project agent: its personality was not written");
    }
    let summary: String = tk::squeeze(&new.summary).chars().take(SUMMARY_MAX).collect();
    if !summary.is_empty() {
        let by = new.from.as_ref().map(|a| a.display_name.clone()).unwrap_or_else(|| name.clone());
        if let Err(err) = crate::guide::hand_over(&config, &agent, &by, &summary, &new.lang) {
            tracing::warn!(error = %err, "project agent: the summary was not kept");
        }
    }
    tracing::info!(agent = %agent.name, "project agent: created");
    crate::own_space::apart_when_idle(state, &agent.name); // its guests: out of the person's hub (own_space.rs)
    let entry = entry(&config, &agent.name).unwrap_or(Value::Null);
    let shown = view(&config, &agent, &entry);
    Ok((agent, shown))
}

// -- the end ----------------------------------------------------------------------------------------------------------

/// Archived: its share stopped (deleted at the guichet), then marked. Err: the guichet couldn't be told (the share
/// stays; tried again later).
pub async fn archive(state: &AppState, agent: &str) -> ApiResult<()> {
    let config = state.config();
    if let Some(share) = crate::read_shares::thread_share(&config, agent) {
        crate::read_shares::stop_share(state, &str_of(&share, "id")).await?;
    }
    update(&state.config(), agent, |e| {
        e.map(|mut e| {
            if e["archived"].is_null() {
                e["archived"] = json!(now());
            }
            e
        })
    })?;
    tracing::info!(agent = %agent, "project agent: archived");
    Ok(())
}

/// The projects whose end has come, archived (every `CHECK_SECONDS`).
pub async fn run_forever(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(CHECK_SECONDS)).await;
        let at = now();
        let ended: Vec<String> = all(&state.config())
            .into_iter()
            .filter(|(_, e)| e["archived"].is_null() && e["ends"].as_f64().unwrap_or(f64::MAX) <= at)
            .map(|(agent, _)| agent)
            .collect();
        for agent in ended {
            if let Err(e) = archive(&state, &agent).await {
                tracing::debug!(agent = %agent, error = ?e.detail, "project agent not archived yet");
            }
        }
    }
}

// -- routes -----------------------------------------------------------------------------------------------------------

async fn list(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let projects: Vec<Value> = all(&config)
        .into_iter()
        .filter_map(|(agent, e)| config.agent(Some(&agent)).map(|a| view(&config, a, &e)))
        .collect();
    Ok(web::ok(json!({"projects": projects})))
}

async fn create_route(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let body = body_json(&body)?;
    let ends = ends_ok(body.get("ends").and_then(Value::as_f64))?;
    let lang = Some(tk::str_or_empty(body.get("lang"))).filter(|l| !l.is_empty()).unwrap_or_else(|| "fr".into());
    let new = NewProject {
        name: tk::str_or_empty(body.get("name")),
        ends,
        lang: lang.chars().take(10).collect(),
        category: tk::str_or_empty(body.get("category")),
        color: color_ok(body.get("color").and_then(Value::as_str)),
        summary: tk::str_or_empty(body.get("summary")),
        from: None,
    };
    let (_, shown) = create(&state, new).await?;
    Ok(web::json_response(201, &shown))
}

fn project_of(config: &Config, agent: &str) -> ApiResult<Value> {
    config.agent(Some(agent)).ok_or_else(|| HttpError::new(404, "no such project"))?;
    entry(config, agent).ok_or_else(|| HttpError::new(404, "no such project"))
}

async fn set_ends(State(state): State<AppState>, UrlPath(agent): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_json(&body)?;
    let config = state.config();
    let project = project_of(&config, &agent)?;
    if !project["archived"].is_null() {
        return Err(HttpError::new(409, "this project has ended: restore it first"));
    }
    let ends = ends_ok(body.get("ends").and_then(Value::as_f64))?;
    if let Some(share) = crate::read_shares::thread_share(&config, &agent) {
        crate::read_shares::change_ends(&state, &str_of(&share, "id"), ends).await?;
    }
    set_ends_of(&config, &agent, ends)?;
    Ok(web::ok(view_of(&state.config(), &agent)?))
}

async fn archive_route(State(state): State<AppState>, UrlPath(agent): UrlPath<String>) -> ApiResult<Response> {
    project_of(&state.config(), &agent)?;
    archive(&state, &agent).await?;
    Ok(web::ok(view_of(&state.config(), &agent)?))
}

async fn restore(State(state): State<AppState>, UrlPath(agent): UrlPath<String>, body: Bytes) -> ApiResult<Response> {
    let body = body_json(&body)?;
    let config = state.config();
    project_of(&config, &agent)?;
    let ends = match body.get("ends").and_then(Value::as_f64) {
        Some(e) => ends_ok(Some(e))?,
        None => now() + DEFAULT_DAYS * 86400.0,
    };
    update(&config, &agent, |e| {
        e.map(|mut e| {
            e["archived"] = Value::Null;
            e["ends"] = json!(ends);
            e
        })
    })?;
    tracing::info!(agent = %agent, "project agent: restored");
    Ok(web::ok(view_of(&state.config(), &agent)?))
}

/// For the apps (mounted under /v1, bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/project-agents", get(list).post(create_route))
        .route("/project-agents/{agent}/ends", post(set_ends))
        .route("/project-agents/{agent}/archive", post(archive_route))
        .route("/project-agents/{agent}/restore", post(restore))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bip_colour_is_six_hex_digits() {
        assert_eq!(color_ok(Some("#ff8a3d")).as_deref(), Some("#FF8A3D"));
        assert_eq!(color_ok(Some(" #12AB9F ")).as_deref(), Some("#12AB9F"));
        for bad in [None, Some(""), Some("ff8a3d"), Some("#ff8a3"), Some("#gg8a3d"), Some("red")] {
            assert_eq!(color_ok(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn only_the_project_s_tools_answer() {
        for open in TOOLS {
            assert!(closed_tool(open).is_none(), "{open}");
        }
        for closed in ["mail_search", "calendar_events", "health", "notebook_read", "archives_search", "agent_file"] {
            assert!(closed_tool(closed).is_some(), "{closed}");
        }
        let config = Config::with_key("k");
        let names: Vec<String> = tools(&config).iter().filter_map(|t| t["name"].as_str().map(str::to_string)).collect();
        assert!(names.iter().all(|n| TOOLS.contains(&n.as_str())), "{names:?}");
        assert!(names.contains(&"create_page".to_string()) && names.contains(&"weather".to_string()));
    }

    #[test]
    fn its_block_comes_and_goes() {
        let soul = "Tu es Lisbonne.\n";
        let with = with_project(soul, true);
        assert!(with.contains(BLOCK_START) && with.contains("read-only"));
        assert_eq!(with_project(&with, true).matches(BLOCK_START).count(), 1);
        let (rest, had) = split_project(&with);
        assert!(had && !rest.contains("bipagents:project"));
        assert_eq!(rest.trim(), "Tu es Lisbonne.");
        assert!(!with_project(&with, false).contains(BLOCK_START));
    }

    #[test]
    fn end_dates_are_in_the_future_within_a_year() {
        let at = now();
        assert!(ends_ok(Some(at + 86400.0)).is_ok());
        assert!(ends_ok(Some(at - 1.0)).is_err());
        assert!(ends_ok(Some(at + 400.0 * 86400.0)).is_err());
        assert!(ends_ok(None).is_err());
    }

    #[test]
    fn the_file_marks_and_forgets() {
        let dir = std::env::temp_dir().join(format!("project-agents-{}", std::process::id()));
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.join("outbox.db").to_string_lossy().into_owned();
        assert!(!is_project(&config, "lisbonne"));
        mark(&config, "lisbonne", 2.0e9, "fr", Some("bip")).unwrap();
        assert!(is_project(&config, "lisbonne"));
        assert_eq!(listing_mark(&config, "lisbonne"), json!({"ends": 2.0e9, "archived": null}));
        assert_eq!(listing_mark(&config, "bip"), Value::Null);
        forget(&config, "lisbonne");
        assert!(!is_project(&config, "lisbonne"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
