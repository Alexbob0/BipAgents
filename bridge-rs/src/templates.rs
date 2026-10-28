//! « Partager comme modèle » (approved by Alex 2026-10-28; the design and the apps' contract:
//! internal/next/agent-templates.md). An agent shared as a model: its look, its personality, its tool choices, its own
//! skills, its routines' definitions and its connectors' code travel; nothing of the person's does (memory, the
//! Carnet, conversations, files, pages, the vault, secrets, connectors' state, mailboxes, health, approvals, links).
//!
//! - Export (`POST /v1/agents/{agent}/template`): the bundle is built from an allow-list of fields, never by copying
//!   a folder or a record; the person's review choices (`leave_out`) come with each call (stateless); the scrubber
//!   (templates_scrub.rs) flags what looks personal or secret. The app then publishes it at the guichet itself, with
//!   its account (as it makes read-only shares there).
//! - Import (`POST /v1/templates/import`): a new agent made from a bundle (untrusted: every name, path, size, host
//!   and schedule checked again). Its connectors land « à approuver » with a card each, their files in the bridge's
//!   own copy folder (the approval pins them from there); its routines land paused until the person turns them on
//!   (`/v1/agents/{agent}/template-routines`); its skills are marked « importé ». Health stays off.
//! - `find_template`: the guide agent's search of Kerfy's ready-made models at the guichet (empty for now).
//!
//! `<data>/templates/routines.json` (0600): per agent, the routines an import paused (only those are switched here).
//!
//! No Python counterpart.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post, put};
use axum::Router;
use base64::Engine;
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::agents_admin::{self, AgentSettings, CustomAgent, LaxBool, NewAgent};
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::connectors::{self, Connector, Schedule, SecretHint, ToolInfo};
use crate::connectors_run as runner;
use crate::templates_scrub as scrub;
use crate::tools_knowledge::tool_text;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const FORMAT: &str = "bipagents-agent-template";
pub const VERSION: i64 = 1;
/// The bundle's JSON at most (the guichet takes 1 000 000).
pub const MAX_BYTES: usize = 900_000;
pub const MAX_FILE: usize = 256 * 1024;
pub const MAX_SKILL_FILES: usize = 64;
pub const MAX_CONNECTOR_FILES: usize = 200;
pub const MAX_SKILLS: usize = 40;
pub const MAX_ROUTINES: usize = 30;
pub const MAX_INSTRUCTIONS: usize = 8000;
pub const MAX_INTRO: usize = 400;
pub const MAX_NAME: usize = 40;
pub const MAX_PROMPT: usize = 4000;
/// Hermes' `paused_reason` of a routine a model brought, until the person turns it on.
pub const PAUSE_REASON: &str = "bipagents-template: turned on by the person in the app";
pub const FIND_TOOL: &str = "find_template";
pub const LINK_BASE: &str = "https://bipagents.com/m/";
pub const DEFAULT_GUICHET: &str = "https://guichet.bipagents.com";
/// A bundle's files decoded: (relative path, executable, content).
type Files = Vec<(PathBuf, bool, Vec<u8>)>;
const STD: base64::engine::general_purpose::GeneralPurpose = base64::engine::general_purpose::STANDARD;

/// What never travels, in the person's words (shown on the review screen).
pub const NEVER: [&str; 6] = [
    "Sa mémoire et son Carnet",
    "Les conversations",
    "Les fichiers, archives et pages",
    "Les mots de passe, le coffre et l'état des connecteurs",
    "Les boîtes mail, l'agenda, les contacts et la santé",
    "Les liens avec ta famille et tes autres agents",
];

fn bad(detail: impl Into<String>) -> HttpError {
    HttpError::new(400, detail.into())
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn head(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

// -- files ------------------------------------------------------------------------------------------------------------

/// The plain files under `root` (sorted; links, hidden files and folders left out), as the bundle carries them.
fn files_of(root: &Path, max_files: usize) -> Result<Vec<Value>, String> {
    fn walk(root: &Path, rel: &Path, out: &mut Vec<(String, PathBuf, bool)>, max: usize) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(root.join(rel)).map_err(|e| e.to_string())?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
            let here = rel.join(&name);
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                walk(root, &here, out, max)?;
            } else if meta.is_file() {
                use std::os::unix::fs::PermissionsExt;
                if meta.len() as usize > MAX_FILE {
                    return Err(format!("{} is over {} KB", here.display(), MAX_FILE / 1024));
                }
                out.push((here.to_string_lossy().into_owned(), entry.path(), meta.permissions().mode() & 0o111 != 0));
                if out.len() > max {
                    return Err(format!("more than {max} files"));
                }
            }
        }
        Ok(())
    }
    let mut found = Vec::new();
    walk(root, Path::new(""), &mut found, max_files)?;
    let mut out = Vec::new();
    for (path, file, exec) in found {
        let data = std::fs::read(&file).map_err(|e| e.to_string())?;
        out.push(match String::from_utf8(data) {
            Ok(text) => json!({"path": path, "exec": exec, "text": text}),
            Err(e) => json!({"path": path, "exec": exec, "base64": STD.encode(e.into_bytes())}),
        });
    }
    Ok(out)
}

/// A file's path in a bundle, checked: relative, plain parts, no hidden part.
fn safe_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.len() > 200 || path.starts_with('/') || path.contains('\\') || path.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.starts_with('.') {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// A bundle's files checked and decoded: (path, exec, bytes).
fn decode_files(files: Option<&Value>, max: usize) -> Result<Files, String> {
    let Some(Value::Array(items)) = files else { return Err("files: a list".into()) };
    if items.is_empty() || items.len() > max {
        return Err(format!("files: 1 to {max}"));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for item in items {
        let path = s(item, "path");
        let Some(rel) = safe_path(&path) else {
            return Err(format!("files: « {} » is not a plain relative path", head(&path, 60)));
        };
        if !seen.insert(rel.clone()) {
            return Err(format!("files: « {path} » twice"));
        }
        let data = match (item.get("text").and_then(Value::as_str), item.get("base64").and_then(Value::as_str)) {
            (Some(text), None) => text.as_bytes().to_vec(),
            (None, Some(b64)) => STD.decode(b64).map_err(|_| format!("files: « {path} » is not base64"))?,
            _ => return Err(format!("files: « {path} » needs text or base64")),
        };
        if data.len() > MAX_FILE {
            return Err(format!("files: « {path} » is over {} KB", MAX_FILE / 1024));
        }
        out.push((rel, item.get("exec").and_then(Value::as_bool).unwrap_or(false), data));
    }
    Ok(out)
}

fn write_files(root: &Path, files: &[(PathBuf, bool, Vec<u8>)]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for (rel, exec, data) in files {
        let to = root.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&to, data)?;
        std::fs::set_permissions(&to, std::fs::Permissions::from_mode(if *exec { 0o755 } else { 0o644 }))?;
    }
    Ok(())
}

// -- routines (Hermes' cron/jobs.json) ---------------------------------------------------------------------------------

fn jobs_file(config: &Config, agent: &AgentConfig) -> PathBuf {
    agents_admin::profile_folder(config, agent).join("cron").join("jobs.json")
}

fn read_jobs(file: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(file) else { return Vec::new() };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Array(items)) => items,
        Ok(Value::Object(map)) => map.get("jobs").and_then(Value::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Changes the jobs file under Hermes' own lock (made when missing), written whole (0600).
fn change_jobs<T>(file: &Path, f: impl FnOnce(&mut Vec<Value>) -> T) -> std::io::Result<T> {
    let folder = file.parent().ok_or_else(|| std::io::Error::other("no folder"))?;
    std::fs::create_dir_all(folder)?;
    let _lock = crate::credit::JobsLock::take(folder)?;
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut data: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"jobs": []}));
    if !data.is_array() && !data.get("jobs").is_some_and(Value::is_array) {
        data = json!({"jobs": []});
    }
    let out = {
        let jobs = match &mut data {
            Value::Array(items) => items,
            Value::Object(map) => map.get_mut("jobs").and_then(Value::as_array_mut).expect("a list"),
            _ => unreachable!(),
        };
        f(jobs)
    };
    if let Value::Object(map) = &mut data {
        map.insert("updated_at".into(), json!(crate::vault::now()));
    }
    let tmp = folder.join(format!(".jobs.json.bipagents-{}", std::process::id()));
    py::write_with_mode(&tmp.to_string_lossy(), serde_json::to_string_pretty(&data)?.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, file)?;
    Ok(out)
}

/// A job a model may carry: a prompt on a recurring schedule, nothing that points at the creator's files or moment.
fn shareable_job(job: &Value) -> Option<Value> {
    let schedule = job.get("schedule")?;
    let kind = s(schedule, "kind");
    let prompt = s(job, "prompt").trim().to_string();
    let script = job.get("script").is_some_and(|v| !v.is_null() && v != "");
    let monitor =
        ["monitor_script", "monitor_url"].iter().any(|k| job.get(*k).is_some_and(|v| !v.is_null() && v != ""));
    if prompt.is_empty() || script || monitor || job.get("no_agent").and_then(Value::as_bool).unwrap_or(false) {
        return None;
    }
    let display = job
        .get("schedule_display")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| s(schedule, "display"));
    let schedule = match kind.as_str() {
        "cron" => json!({"kind": "cron", "expr": s(schedule, "expr"), "display": display}),
        "interval" => {
            json!({"kind": "interval", "minutes": schedule.get("minutes").and_then(Value::as_i64)?, "display": display})
        }
        _ => return None,
    };
    let skills: Vec<Value> = job.get("skills").and_then(Value::as_array).cloned().unwrap_or_default();
    Some(json!({"name": head(s(job, "name").trim(), 80), "prompt": head(&prompt, MAX_PROMPT), "schedule": schedule,
                "skills": skills}))
}

fn cron_ok(expr: &str) -> bool {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    fields.len() == 5
        && fields.iter().all(|f| f.len() <= 40 && f.chars().all(|c| c.is_ascii_alphanumeric() || ",-*/".contains(c)))
}

static LOCK: Mutex<()> = Mutex::new(());

fn routines_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("templates").join("routines.json")
}

fn imported_routines(config: &Config) -> BTreeMap<String, Vec<String>> {
    std::fs::read_to_string(routines_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn remember_routines(config: &Config, agent: &str, jobs: &[String]) -> std::io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = imported_routines(config);
    all.entry(agent.to_string()).or_default().extend(jobs.iter().cloned());
    let path = routines_path(config);
    std::fs::create_dir_all(path.parent().expect("a folder"))?;
    py::write_with_mode(&path.to_string_lossy(), serde_json::to_string(&all)?.as_bytes(), 0o600)
}

// -- export -------------------------------------------------------------------------------------------------------------

/// The agents a model can be made of: managed here, not the family's nor a project agent.
fn shareable(config: &Config, agent_id: &str) -> ApiResult<AgentConfig> {
    let Some(agent) = config.agent(Some(agent_id)).cloned() else { return Err(HttpError::new(404, "no such agent")) };
    if agent.host_home.as_deref().is_some_and(|h| !h.is_empty()) || agents_admin::editable(config, agent_id).is_err() {
        return Err(HttpError::new(409, "this agent is set up by hand: it can't be shared as a model"));
    }
    if crate::family_agent::is_family(config, &agent.name) || crate::project_agent::is_project(config, &agent.name) {
        return Err(HttpError::new(409, "the family's agent and project agents can't be shared as a model"));
    }
    Ok(agent)
}

struct Part {
    id: String,
    kind: &'static str,
    title: String,
    detail: String,
    removable: bool,
    extra: Dict,
}

fn connector_bundle(state: &AppState, config: &Config, agent: &AgentConfig, conn: &Connector) -> Result<Value, String> {
    let files = files_of(&connectors::code_dir(config, &agent.name, &conn.id), MAX_CONNECTOR_FILES)?;
    let items = state.vault.secret_items(&agent.name);
    let secrets: Vec<Value> = conn
        .secrets
        .iter()
        .map(|name| {
            let info = items.iter().find(|i| &i.label == name).and_then(|i| i.secret_info.clone()).unwrap_or(json!({}));
            json!({"name": name, "kind": info.get("kind").and_then(Value::as_str).unwrap_or("password"),
                   "identifier_label": info.get("identifier_label").and_then(Value::as_str).unwrap_or("")})
        })
        .collect();
    let tools: Vec<Value> = if conn.tools.is_empty() {
        conn.declared_tools.iter().map(|t| json!({"name": t.name, "description": t.description})).collect()
    } else {
        conn.tools
            .iter()
            .map(|t| json!({"name": s(t, "name"), "description": head(&s(t, "description"), 300)}))
            .collect()
    };
    Ok(json!({
        "name": conn.name, "description": conn.description, "icon": conn.icon, "sites": conn.sites, "kind": conn.kind,
        "entry": conn.entry, "tools": tools, "secrets": secrets,
        "schedule": conn.schedule.as_ref().map(|s| json!({"when": s.when, "mechanical": s.mechanical, "tool": s.tool,
                                                          "arguments": s.arguments})),
        "unofficial": conn.unofficial, "files": files,
    }))
}

fn tool_label(name: &str) -> String {
    match name {
        "browser" => "Navigateur".into(),
        "weather" => "Météo".into(),
        "notebook" => "Carnet".into(),
        "web" => "Recherche web".into(),
        other => other.to_string(),
    }
}

/// The whole review: the bundle with `leave_out` taken out, the parts (all of them, each `included` or not), the flags.
/// Over the size: `(413, {detail, size, max, biggest})`, the part to take out named.
pub async fn review(state: &AppState, agent: &AgentConfig, body: &Value) -> (u16, Value) {
    let config = state.config();
    let leave_out: BTreeSet<String> = body
        .get("leave_out")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let name =
        body.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(|n| head(n, MAX_NAME));
    let name = name.unwrap_or_else(|| {
        head(if agent.display_name.is_empty() { &agent.name } else { &agent.display_name }, MAX_NAME)
    });
    let intro = body.get("intro").and_then(Value::as_str).map(|t| head(t.trim(), MAX_INTRO)).unwrap_or_default();

    let mut parts: Vec<Part> = Vec::new();
    let mut flags: Vec<Value> = Vec::new();
    let mut bundle = Map::new();
    bundle.insert("format".into(), json!(FORMAT));
    bundle.insert("version".into(), json!(VERSION));
    bundle.insert("name".into(), json!(name));
    let lang = agents_admin::read_soul(&config, agent).ok().and_then(|soul| crate::souls::split_language(&soul).1);
    bundle.insert("lang".into(), json!(lang));
    // the look: always
    let category = agent
        .category
        .clone()
        .filter(|c| crate::souls::CATEGORIES.contains(&c.as_str()))
        .unwrap_or_else(|| "daily".into());
    bundle.insert("look".into(), json!({"category": category, "color": agent.color, "outfit": agent.outfit}));
    parts.push(Part {
        id: "look".into(),
        kind: "look",
        title: "Son apparence et son nom".into(),
        detail: name.clone(),
        removable: false,
        extra: Dict::new(),
    });
    if !intro.is_empty() {
        let id = "intro";
        if !leave_out.contains(id) {
            bundle.insert("intro".into(), json!(intro));
            flags.extend(scrub::flags_json(id, None, &intro));
        }
        parts.push(Part {
            id: id.into(),
            kind: "intro",
            title: "Le petit mot de présentation".into(),
            detail: head(&intro, 200),
            removable: true,
            extra: Dict::new(),
        });
    }
    // the personality, without the bridge's blocks
    let instructions =
        agents_admin::editable_soul(&config, agent).map(|t| head(t.trim(), MAX_INSTRUCTIONS)).unwrap_or_default();
    if !instructions.is_empty() {
        let id = "instructions";
        if !leave_out.contains(id) {
            bundle.insert("instructions".into(), json!(instructions));
            flags.extend(scrub::flags_json(id, None, &instructions));
        }
        parts.push(Part {
            id: id.into(),
            kind: "instructions",
            title: "Sa personnalité et ses consignes".into(),
            detail: head(&instructions, 200),
            removable: true,
            extra: Dict::new(),
        });
    }
    // the tools: Hermes' toolsets (when it answers), the browser, weather, the Carnet; never health
    let settings = agents_admin::read_settings(&config, agent, &state.http).await.unwrap_or(json!({}));
    let mut toolsets = Map::new();
    for t in settings.get("toolsets").and_then(Value::as_array).cloned().unwrap_or_default() {
        let n = s(&t, "name");
        if !n.is_empty() && n.len() <= 40 {
            toolsets.insert(n, json!(t.get("enabled").and_then(Value::as_bool).unwrap_or(false)));
        }
    }
    let browser = settings.get("browser").and_then(|b| b.get("on")).and_then(Value::as_bool);
    let weather = crate::weather::enabled(&config, &agent.name);
    let notebook = crate::notebook::enabled(&config, &agent.name);
    let mut on: Vec<String> =
        toolsets.iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| tool_label(k)).collect();
    for (flag, label) in [(browser == Some(true), "browser"), (weather, "weather"), (notebook, "notebook")] {
        if flag {
            on.push(tool_label(label));
        }
    }
    if !leave_out.contains("tools") {
        bundle.insert(
            "tools".into(),
            json!({"toolsets": toolsets, "browser": browser, "weather": weather, "notebook": notebook}),
        );
    }
    parts.push(Part {
        id: "tools".into(),
        kind: "tools",
        title: "Ses outils".into(),
        detail: head(&on.join(", "), 200),
        removable: true,
        extra: Dict::new(),
    });
    // its own skills
    let profile = agents_admin::profile_folder(&config, agent);
    let mut skills = Vec::new();
    let mut skill_names = BTreeSet::new();
    for (entry, folder) in crate::own_skills::folders(&profile).into_iter().take(MAX_SKILLS) {
        let skill = entry.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        if skill_slug(&skill).is_none() {
            continue;
        }
        let id = format!("skill:{skill}");
        let description = entry.get("description").and_then(Value::as_str).unwrap_or("").to_string();
        let files = match files_of(&folder, MAX_SKILL_FILES) {
            Ok(files) => files,
            Err(_) => continue, // too big or unreadable: not offered
        };
        if !leave_out.contains(&id) {
            for f in &files {
                if let Some(text) = f.get("text").and_then(Value::as_str) {
                    flags.extend(scrub::flags_json(&id, Some(&s(f, "path")), text));
                }
            }
            skill_names.insert(skill.clone());
            skills.push(json!({"name": skill, "description": description, "files": files}));
        }
        parts.push(Part {
            id,
            kind: "skill",
            title: format!("Savoir-faire : {skill}"),
            detail: head(&description, 200),
            removable: true,
            extra: Dict::new(),
        });
    }
    bundle.insert("skills".into(), json!(skills));
    // routines' definitions
    let mut routines = Vec::new();
    for job in read_jobs(&jobs_file(&config, agent)).iter().take(MAX_ROUTINES) {
        let Some(mut shared) = shareable_job(job) else { continue };
        let id = format!("routine:{}", s(job, "id"));
        let shown = s(&shared, "name");
        let display = shared
            .get("schedule")
            .map(|s| s.get("display").and_then(Value::as_str).unwrap_or("").to_string())
            .unwrap_or_default();
        if !leave_out.contains(&id) {
            flags.extend(scrub::flags_json(&id, None, &s(&shared, "prompt")));
            // only the skills that travel with it
            let kept: Vec<Value> = shared
                .get("skills")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|n| n.as_str().is_some_and(|n| skill_names.contains(n)))
                .collect();
            shared["skills"] = json!(kept);
            routines.push(shared);
        }
        parts.push(Part {
            id,
            kind: "routine",
            title: format!("Routine : {shown}"),
            detail: display,
            removable: true,
            extra: Dict::new(),
        });
    }
    bundle.insert("routines".into(), json!(routines));
    // connectors: the approved copy only
    let mut conns = Vec::new();
    for conn in connectors::of_agent(&config, &agent.name) {
        if conn.approved_at.is_none() || conn.fingerprint.is_empty() {
            continue;
        }
        let id = format!("connector:{}", conn.id);
        let Ok(item) = connector_bundle(state, &config, agent, &conn) else { continue };
        let count = item.get("files").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        if !leave_out.contains(&id) {
            for f in item.get("files").and_then(Value::as_array).cloned().unwrap_or_default() {
                if let Some(text) = f.get("text").and_then(Value::as_str) {
                    flags.extend(scrub::flags_json(&id, Some(&s(&f, "path")), text));
                }
            }
            flags.extend(scrub::flags_json(&id, None, &conn.description));
            if let Some(schedule) = &conn.schedule {
                flags.extend(scrub::flags_json(&id, None, &schedule.arguments.to_string()));
            }
            conns.push(item);
        }
        let mut extra = Dict::new();
        extra.insert("unofficial".into(), json!(conn.unofficial));
        extra.insert("approved".into(), json!(conn.status == connectors::APPROVED));
        parts.push(Part {
            id,
            kind: "connector",
            title: format!("Connecteur : {}", conn.name),
            detail: format!("{} · {count} fichier{}", conn.sites.join(", "), if count > 1 { "s" } else { "" }),
            removable: true,
            extra,
        });
    }
    bundle.insert("connectors".into(), json!(conns));

    let bundle = Value::Object(bundle);
    let size = bundle.to_string().len();
    let parts_json: Vec<Value> = parts
        .iter()
        .map(|p| {
            let mut item = json!({"id": p.id, "kind": p.kind, "title": p.title, "detail": p.detail, "removable": p.removable,
                                  "included": !p.removable || !leave_out.contains(&p.id)});
            if let Value::Object(map) = &mut item {
                map.extend(p.extra.clone());
            }
            item
        })
        .collect();
    if size > MAX_BYTES {
        // the biggest part still in: the one to take out
        let biggest = parts
            .iter()
            .filter(|p| p.removable && !leave_out.contains(&p.id))
            .max_by_key(|p| part_size(&bundle, &p.id))
            .map(|p| p.id.clone());
        return (
            413,
            json!({"detail": "ce modèle est trop lourd : retire une partie", "size": size, "max": MAX_BYTES,
                            "biggest": biggest, "parts": parts_json}),
        );
    }
    (
        200,
        json!({"template": bundle, "parts": parts_json, "never": NEVER, "flags": flags, "size": size, "max": MAX_BYTES}),
    )
}

fn part_size(bundle: &Value, id: &str) -> usize {
    let find = |list: &str, key: &str, want: &str| {
        bundle
            .get(list)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(|i| s(i, key) == want))
            .map(|v| v.to_string().len())
            .unwrap_or(0)
    };
    match id.split_once(':') {
        Some(("skill", n)) => find("skills", "name", n),
        Some(("connector", _)) | Some(("routine", _)) => {
            // ids of these aren't in the bundle: the biggest of their list stands in
            let list = if id.starts_with("connector") { "connectors" } else { "routines" };
            bundle
                .get(list)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|i| i.to_string().len()).max().unwrap_or(0))
                .unwrap_or(0)
        }
        _ => bundle.get(id).map(|v| v.to_string().len()).unwrap_or(0),
    }
}

async fn export_route(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = shareable(&config, &agent_id)?;
    let body: Value =
        if body.is_empty() { json!({}) } else { serde_json::from_slice(&body).map_err(|_| bad("a JSON body"))? };
    let (status, answer) = review(&state, &agent, &body).await;
    Ok(web::json_response(status, &answer))
}

// -- import ---------------------------------------------------------------------------------------------------------------

fn skill_slug(name: &str) -> Option<String> {
    let ok = (1..=64).contains(&name.len())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !name.starts_with(['-', '_']);
    ok.then(|| name.to_string())
}

/// A connector of a bundle, checked as `connector_register` checks the agent's (and its files).
pub fn checked_connector(item: &Value) -> Result<(Connector, Files), String> {
    let name = crate::cards::quoted(&s(item, "name"), 40);
    let slug = connectors::slug(&name);
    if name.is_empty() || slug.len() < 2 {
        return Err("a connector without a name".into());
    }
    let mut sites = Vec::new();
    for raw in item.get("sites").and_then(Value::as_array).cloned().unwrap_or_default() {
        let host = crate::sandbox_sites::clean_host(raw.as_str().unwrap_or(""), true)
            .map_err(|why| format!("{name}: site {why}"))?;
        if !sites.contains(&host) {
            sites.push(host);
        }
    }
    if sites.is_empty() || sites.len() > connectors::MAX_SITES {
        return Err(format!("{name}: 1 to {} sites", connectors::MAX_SITES));
    }
    let icon = s(item, "icon").to_lowercase();
    let icon = if connectors::icon_keys().contains(&icon) { icon } else { "plug".to_string() };
    let kind = match s(item, "kind").as_str() {
        "" | "mcp" => "mcp".to_string(),
        "script" => "script".to_string(),
        _ => return Err(format!("{name}: kind mcp or script")),
    };
    let mut hints = Vec::new();
    for secret in item.get("secrets").and_then(Value::as_array).cloned().unwrap_or_default() {
        let n = s(&secret, "name");
        if !crate::secrets::valid_name(&n) {
            return Err(format!("{name}: « {} » is not a secret's name", head(&n, 40)));
        }
        let k = match s(&secret, "kind").as_str() {
            "token" => "token",
            "code" => "code",
            _ => "password",
        };
        hints.push(SecretHint {
            name: n,
            kind: k.into(),
            identifier_label: crate::cards::quoted(&s(&secret, "identifier_label"), 80),
        });
    }
    if hints.len() > connectors::MAX_SECRETS {
        return Err(format!("{name}: {} secrets at most", connectors::MAX_SECRETS));
    }
    let schedule = match item.get("schedule") {
        Some(Value::Object(map)) => {
            let when = map.get("when").and_then(Value::as_str).unwrap_or("").to_string();
            runner::parse_when(&when).map_err(|why| format!("{name}: schedule {why}"))?;
            let mechanical = map.get("mechanical").and_then(Value::as_bool).unwrap_or(false);
            let tool = map.get("tool").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if mechanical && tool.is_empty() {
                return Err(format!("{name}: schedule.tool missing"));
            }
            Some(Schedule { when, mechanical, tool, arguments: map.get("arguments").cloned().unwrap_or(json!({})) })
        }
        _ => None,
    };
    let declared_tools = item
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .take(64)
        .filter(|t| !s(t, "name").trim().is_empty())
        .map(|t| ToolInfo { name: head(s(t, "name").trim(), 64), description: head(&s(t, "description"), 300) })
        .collect();
    let files = decode_files(item.get("files"), MAX_CONNECTOR_FILES).map_err(|why| format!("{name}: {why}"))?;
    let conn = Connector {
        name,
        folder: format!("/workspace/connecteurs/{slug}"),
        slug,
        description: crate::cards::quoted(&s(item, "description"), 200),
        icon,
        sites,
        entry: head(&s(item, "entry"), 300),
        kind,
        declared_tools,
        secrets: hints.iter().map(|h| h.name.clone()).collect(),
        secret_hints: hints,
        schedule,
        unofficial: item.get("unofficial").and_then(Value::as_bool).unwrap_or(false),
        from_template: true,
        ..Connector::default()
    };
    if conn.description.is_empty() {
        return Err(format!("{}: a description", conn.name));
    }
    runner::entry_argv(&conn).map_err(|_| format!("{}: entry is not a command", conn.name))?;
    Ok((conn, files))
}

/// A bundle checked before anything is made (400s say what is wrong).
pub fn check_bundle(bundle: &Value) -> Result<(), String> {
    if !bundle.is_object() {
        return Err("template: an object".into());
    }
    if s(bundle, "format") != FORMAT || bundle.get("version").and_then(Value::as_i64) != Some(VERSION) {
        return Err("template: not a BipAgents model of a version this bridge knows".into());
    }
    let name = s(bundle, "name");
    if name.trim().is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("name: 1 to {MAX_NAME} characters"));
    }
    if s(bundle, "instructions").chars().count() > MAX_INSTRUCTIONS || s(bundle, "intro").chars().count() > MAX_INTRO {
        return Err("instructions or intro too long".into());
    }
    for (list, max) in [("skills", MAX_SKILLS), ("routines", MAX_ROUTINES), ("connectors", connectors::MAX_CONNECTORS)]
    {
        match bundle.get(list) {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) if items.len() <= max => {}
            _ => return Err(format!("{list}: a list of {max} at most")),
        }
    }
    for skill in bundle.get("skills").and_then(Value::as_array).cloned().unwrap_or_default() {
        if skill_slug(&s(&skill, "name")).is_none() {
            return Err(format!("skills: « {} » is not a skill's name", head(&s(&skill, "name"), 40)));
        }
        decode_files(skill.get("files"), MAX_SKILL_FILES)
            .map_err(|why| format!("skill {}: {why}", s(&skill, "name")))?;
        let has_md =
            skill.get("files").and_then(Value::as_array).is_some_and(|f| f.iter().any(|f| s(f, "path") == "SKILL.md"));
        if !has_md {
            return Err(format!("skill {}: SKILL.md missing", s(&skill, "name")));
        }
    }
    for routine in bundle.get("routines").and_then(Value::as_array).cloned().unwrap_or_default() {
        if s(&routine, "prompt").trim().is_empty() || s(&routine, "prompt").chars().count() > MAX_PROMPT {
            return Err("routines: each one a prompt".into());
        }
    }
    let mut slugs = BTreeSet::new();
    for item in bundle.get("connectors").and_then(Value::as_array).cloned().unwrap_or_default() {
        let (conn, _) = checked_connector(&item)?;
        if !slugs.insert(conn.slug.clone()) {
            return Err(format!("connectors: « {} » twice", conn.name));
        }
    }
    Ok(())
}

/// The routine as Hermes' `create_job` stores a paused one (`deliver: local`: no target of the creator's).
fn hermes_job(routine: &Value, skills: &BTreeSet<String>) -> Option<Value> {
    let schedule = routine.get("schedule")?;
    let display = s(schedule, "display");
    let parsed = match s(schedule, "kind").as_str() {
        "cron" if cron_ok(&s(schedule, "expr")) => {
            let expr = s(schedule, "expr");
            json!({"kind": "cron", "expr": expr, "display": if display.is_empty() { expr.clone() } else { head(&display, 80) }})
        }
        "interval" => {
            let minutes = schedule.get("minutes").and_then(Value::as_i64).filter(|m| (5..=60 * 24 * 31).contains(m))?;
            json!({"kind": "interval", "minutes": minutes, "display": if display.is_empty() { format!("every {minutes}m") } else { head(&display, 80) }})
        }
        _ => return None,
    };
    let prompt = head(s(routine, "prompt").trim(), MAX_PROMPT);
    let name = match head(s(routine, "name").trim(), 80) {
        n if n.is_empty() => head(&prompt, 50),
        n => n,
    };
    let kept: Vec<String> = routine
        .get("skills")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|n| skills.contains(*n)).map(str::to_string).collect())
        .unwrap_or_default();
    let now = crate::vault::now();
    Some(json!({
        "id": &py::uuid4_hex()[..12], "name": name, "prompt": prompt, "skills": kept, "skill": kept.first(),
        "model": null, "provider": null, "base_url": null, "script": null, "no_agent": false,
        "monitor_script": null, "monitor_url": null, "monitor_state": null, "context_from": null,
        "schedule_display": parsed["display"], "schedule": parsed,
        "repeat": {"times": null, "completed": 0},
        "enabled": false, "state": "paused", "paused_at": now, "paused_reason": PAUSE_REASON,
        "created_at": now, "next_run_at": null, "last_run_at": null, "last_status": null, "last_error": null,
        "last_delivery_error": null, "last_delivery_unverified": null, "last_failure": null, "failure_streak": 0,
        "deliver": "local", "origin": null, "enabled_toolsets": null, "workdir": null,
    }))
}

/// A new agent from a bundle (checked first). Its connectors pending, its routines paused, health off.
pub async fn import(state: &AppState, body: &Value) -> ApiResult<Value> {
    let config = state.config();
    if !config.hermes.enabled() {
        return Err(HttpError::new(409, "this install does not manage its agents (set up by hand)"));
    }
    let bundle = body.get("template").cloned().unwrap_or(Value::Null);
    if bundle.to_string().len() > MAX_BYTES {
        return Err(HttpError::new(413, "ce modèle est trop lourd"));
    }
    check_bundle(&bundle).map_err(bad)?;
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| s(&bundle, "name"));
    if name.chars().count() > MAX_NAME {
        return Err(bad(format!("name: 1 to {MAX_NAME} characters")));
    }
    let look = bundle.get("look").cloned().unwrap_or(json!({}));
    let category = Some(s(&look, "category"))
        .filter(|c| crate::souls::CATEGORIES.contains(&c.as_str()))
        .unwrap_or_else(|| "daily".into());
    let lang = Some(s(&bundle, "lang"))
        .filter(|l| (2..=20).contains(&l.len()) && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    let intro = s(&bundle, "intro");
    let role = if intro.trim().is_empty() {
        format!("Agent créé à partir du modèle « {} »", s(&bundle, "name"))
    } else {
        head(intro.trim(), 600)
    };
    let new = NewAgent {
        name: name.clone(),
        lang,
        custom: Some(CustomAgent {
            role,
            tone: String::new(),
            rules: String::new(),
            language: String::new(),
            category: category.clone(),
        }),
        ..NewAgent::default()
    };
    let mut left_out: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let run = agents_admin::default_run();
    let agent = {
        let _guard = agents_admin::LOCK.lock().await;
        let agent = match agents_admin::create_agent(&state.config, &new, &state.http, &run).await {
            Ok(agent) => agent,
            Err(exc) => {
                if let Some(e) = exc.downcast_ref::<agents_admin::AgentError>() {
                    return Err(HttpError::new(400, e.0.clone()));
                }
                tracing::warn!(target: "bipbridge::templates", error = %head(&exc.to_string(), 200), "agent from a model not created");
                return Err(HttpError::new(502, "Hermes could not create the agent"));
            }
        };
        // its personality and tools (health always off)
        let tools = bundle.get("tools").cloned().unwrap_or(json!({}));
        let toolsets: IndexMap<String, LaxBool> = tools
            .get("toolsets")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter(|(k, _)| {
                        k.len() <= 40 && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                    })
                    .filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), LaxBool(b))))
                    .collect()
            })
            .unwrap_or_default();
        let instructions = s(&bundle, "instructions");
        let settings = AgentSettings {
            soul: Some(instructions.clone()).filter(|t| !t.trim().is_empty()),
            toolsets: Some(toolsets).filter(|t| !t.is_empty()),
            weather: tools.get("weather").and_then(Value::as_bool),
            notebook: tools.get("notebook").and_then(Value::as_bool),
            ..AgentSettings::default()
        };
        if let Err(exc) = agents_admin::write_settings(&state.config, &agent, &settings, &run).await {
            tracing::warn!(target: "bipbridge::templates", agent = %agent.name, error = %head(&exc.to_string(), 200), "model's settings not all applied");
            left_out.push("une partie de ses outils n'a pas pu être réglée".into());
        }
        if tools.get("browser").and_then(Value::as_bool) == Some(true) {
            let browser = AgentSettings { browser: Some(true), ..AgentSettings::default() };
            if agents_admin::write_settings(&state.config, &agent, &browser, &run).await.is_err() {
                left_out.push("son propre navigateur (à allumer dans ses outils)".into());
            }
        }
        agent
    };
    // its look
    let color = Some(s(&look, "color"))
        .filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()));
    let outfit = Some(head(&s(&look, "outfit"), 40))
        .filter(|o| !o.is_empty() && o.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    let appearance = crate::voices::AgentAppearance { category, color, outfit, costumes: None };
    let _ =
        crate::voices::set_agent_appearance(State(state.clone()), UrlPath(agent.name.clone()), web::Json(appearance))
            .await;
    let config = state.config();
    let agent = config.agent(Some(&agent.name)).cloned().unwrap_or(agent);
    let profile = agents_admin::profile_folder(&config, &agent);

    // its skills, marked imported
    let mut skills_done = BTreeSet::new();
    for skill in bundle.get("skills").and_then(Value::as_array).cloned().unwrap_or_default() {
        let skill_name = s(&skill, "name");
        let folder = profile.join("skills").join(&skill_name);
        if folder.exists() {
            left_out.push(format!("le savoir-faire « {skill_name} » (un autre du même nom existe déjà)"));
            continue;
        }
        let Ok(files) = decode_files(skill.get("files"), MAX_SKILL_FILES) else { continue };
        if write_files(&folder, &files).is_ok() {
            skills_done.insert(skill_name);
        }
    }
    if !skills_done.is_empty() {
        let mut all = crate::own_skills::imported(&profile);
        all.extend(skills_done.iter().cloned());
        let list: Vec<&String> = all.iter().collect();
        let _ = std::fs::write(profile.join("skills").join(crate::own_skills::IMPORTED_FILE), json!(list).to_string());
    }

    // its routines, paused
    let mut routines_out = Vec::new();
    let mut jobs = Vec::new();
    for routine in bundle.get("routines").and_then(Value::as_array).cloned().unwrap_or_default() {
        match hermes_job(&routine, &skills_done) {
            Some(job) => jobs.push(job),
            None => left_out.push(format!(
                "la routine « {} » (son horaire n'est pas pris en charge)",
                head(&s(&routine, "name"), 60)
            )),
        }
    }
    if !jobs.is_empty() {
        let file = jobs_file(&config, &agent);
        let added = jobs.clone();
        match change_jobs(&file, move |list| list.extend(added)) {
            Ok(()) => {
                let ids: Vec<String> = jobs.iter().map(|j| s(j, "id")).collect();
                let _ = remember_routines(&config, &agent.name, &ids);
                for job in &jobs {
                    routines_out.push(json!({"job": s(job, "id"), "name": s(job, "name"), "display": s(job, "schedule_display"), "on": false}));
                }
            }
            Err(exc) => {
                tracing::warn!(target: "bipbridge::templates", agent = %agent.name, error = %exc, "routines not written");
                left_out.push("ses routines".into());
            }
        }
    }

    // its connectors, pending, with a card each
    let mut conns_out = Vec::new();
    let items = bundle.get("connectors").and_then(Value::as_array).cloned().unwrap_or_default();
    if !items.is_empty() && !connectors::offered(&config, &agent) {
        // they wait, unapproved, like any: they run once the agent has its own space (its settings in the app)
        notes.push(
            "Ses connecteurs tourneront une fois l'agent installé dans son propre espace (dans ses réglages).".into(),
        );
    }
    {
        for item in items {
            let Ok((mut conn, files)) = checked_connector(&item) else { continue };
            conn.id = format!("c_{}", &py::uuid4_hex()[..12]);
            let code = connectors::code_dir(&config, &agent.name, &conn.id);
            let pinned = write_files(&code, &files).map_err(|e| e.to_string()).and_then(|_| runner::fingerprint(&code));
            let Ok(fingerprint) = pinned else {
                let _ = std::fs::remove_dir_all(code.parent().unwrap_or(&code));
                left_out.push(format!("le connecteur « {} »", conn.name));
                continue;
            };
            conn.status = connectors::PENDING.into();
            conn.pending_fingerprint = fingerprint;
            conn.proxy_token = py::token_urlsafe(24);
            conn.created_at = crate::vault::now();
            conn.updated_at = conn.created_at.clone();
            let card = connectors::new_card("template");
            conn.card = Some(card.clone());
            let saved = conn.clone();
            let name = agent.name.clone();
            if connectors::change(&config, move |store| store.agents.entry(name).or_default().push(saved)).is_err() {
                left_out.push(format!("le connecteur « {} »", conn.name));
                continue;
            }
            connectors::announce(state, &agent.name, &conn, &card).await;
            conns_out.push(json!({
                "id": conn.id, "name": conn.name, "run_id": format!("{}{}", connectors::RUN_PREFIX, card.id),
                "unofficial": conn.unofficial,
                "secrets": conn.secret_hints.iter().map(|h| json!({"name": h.name, "kind": h.kind})).collect::<Vec<_>>(),
            }));
        }
    }
    tracing::info!(target: "bipbridge::templates", agent = %agent.name, source = %head(&s(body, "source"), 64),
                   connectors = conns_out.len(), routines = routines_out.len(), skills = skills_done.len(), "agent made from a model");
    let has_url = agent.public_url.as_deref().is_some_and(|u| !u.is_empty());
    let created = json!({"id": agent.name, "name": agent.display_name, "url": agent.public_url,
                         "key": if has_url { Some(agent.hermes_key.clone()) } else { None },
                         "group": crate::agent_groups::group_of(&config, &agent.name)});
    Ok(json!({"agent": created, "connectors": conns_out, "routines": routines_out,
              "skills": skills_done.into_iter().collect::<Vec<_>>(), "left_out": left_out, "notes": notes}))
}

async fn import_route(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    if body.len() > MAX_BYTES + 64 * 1024 {
        return Err(HttpError::new(413, "ce modèle est trop lourd"));
    }
    let body: Value = serde_json::from_slice(&body).map_err(|_| bad("a JSON body"))?;
    Ok(web::json_response(201, &import(&state, &body).await?))
}

// -- the routines a model brought ---------------------------------------------------------------------------------------

fn agent_of(config: &Config, id: &str) -> ApiResult<AgentConfig> {
    config.agent(Some(id)).cloned().ok_or_else(|| HttpError::new(404, "no such agent"))
}

async fn routines_route(State(state): State<AppState>, UrlPath(agent_id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let ours: BTreeSet<String> =
        imported_routines(&config).remove(&agent.name).unwrap_or_default().into_iter().collect();
    let routines: Vec<Value> = read_jobs(&jobs_file(&config, &agent))
        .iter()
        .filter(|j| ours.contains(&s(j, "id")))
        .map(|j| {
            json!({"job": s(j, "id"), "name": s(j, "name"), "display": s(j, "schedule_display"), "prompt": s(j, "prompt"),
                   "on": j.get("enabled").and_then(Value::as_bool).unwrap_or(false)})
        })
        .collect();
    Ok(web::ok(json!({"routines": routines})))
}

async fn routine_switch(
    State(state): State<AppState>,
    UrlPath((agent_id, job)): UrlPath<(String, String)>,
    body: Bytes,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &agent_id)?;
    let body: Value = serde_json::from_slice(&body).map_err(|_| bad("a JSON body"))?;
    let Some(on) = body.get("on").and_then(Value::as_bool) else { return Err(bad("on: true or false")) };
    let ours = imported_routines(&config).remove(&agent.name).unwrap_or_default();
    if !ours.contains(&job) {
        return Err(HttpError::new(404, "unknown job"));
    }
    let file = jobs_file(&config, &agent);
    let wanted = job.clone();
    let found = change_jobs(&file, move |jobs| {
        let Some(Value::Object(j)) = jobs.iter_mut().find(|j| s(j, "id") == wanted) else { return false };
        if on {
            j.insert("enabled".into(), json!(true));
            j.insert("state".into(), json!("scheduled"));
            j.insert("paused_at".into(), Value::Null);
            j.insert("paused_reason".into(), Value::Null);
            j.remove("next_run_at"); // Hermes' due scan computes it again
        } else {
            j.insert("enabled".into(), json!(false));
            j.insert("state".into(), json!("paused"));
            j.insert("paused_at".into(), json!(crate::vault::now()));
            j.insert("paused_reason".into(), json!(PAUSE_REASON));
        }
        true
    })
    .map_err(anyhow::Error::from)?;
    if !found {
        return Err(HttpError::new(404, "unknown job"));
    }
    Ok(web::ok(json!({"job": job, "on": on})))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/template", post(export_route))
        .route("/agents/{agent_id}/template-routines", get(routines_route))
        .route("/agents/{agent_id}/template-routines/{job}", put(routine_switch))
        .route("/templates/import", post(import_route))
}

// -- the guide's search of ready-made models -------------------------------------------------------------------------------

/// `find_template`, for the guide agent only.
pub fn tools(config: &Config, agent: &AgentConfig) -> Vec<Value> {
    if !crate::guide::is_guide(config, &agent.name) {
        return Vec::new();
    }
    vec![json!({
        "name": FIND_TOOL,
        "description": "Search Kerfy's ready-made agent models for what your person needs; each result has a link \
    they open to see it and add it (nothing is added otherwise).",
        "inputSchema": {"type": "object", "properties": {
            "need": {"type": "string", "description": "what they need, in a few words"}},
            "required": ["need"]},
    })]
}

/// The guichet: this place's backup address's origin when it is one of the offer, else Kerfy's.
fn guichet_url(config: &Config) -> String {
    let path = PathBuf::from(py::expanduser("~")).join(".config").join("bipagents").join("backup.json");
    let _ = config;
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("url").and_then(Value::as_str).and_then(|u| url::Url::parse(u).ok()))
        .filter(|u| u.scheme() == "https")
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|| DEFAULT_GUICHET.to_string())
}

static HTTP: LazyLock<reqwest::Client> =
    LazyLock::new(|| reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap_or_default());

/// An id the guichet makes: url-safe base64, 16 to 64 signs.
pub fn id_ok(id: &str) -> bool {
    (16..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The catalogue's answer in a few lines for the agent.
pub fn catalogue_text(answer: &Value) -> String {
    let items = answer.get("templates").and_then(Value::as_array).cloned().unwrap_or_default();
    if items.is_empty() {
        return "No ready-made model fits yet: propose making the agent yourself (propose_agent).".into();
    }
    let lines: Vec<String> = items
        .iter()
        .take(5)
        .filter(|t| id_ok(&s(t, "id")))
        .map(|t| format!("- {} — {} — {LINK_BASE}{}", head(&s(t, "name"), 40), head(&s(t, "intro"), 160), s(t, "id")))
        .collect();
    format!(
        "Ready-made models (give the person the link of the one that fits; they see it and add it themselves):\n{}",
        lines.join("\n")
    )
}

pub async fn find_call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    if !crate::guide::is_guide(config, &agent.name) {
        return tool_text("This tool is the guide's.", true);
    }
    let need = arguments.get("need").and_then(Value::as_str).map(|n| head(n.trim(), 200)).unwrap_or_default();
    let url = format!("{}/v1/templates/catalogue", guichet_url(config));
    let answer = HTTP.get(url).query(&[("q", need.as_str())]).send().await;
    let answer = match answer {
        Ok(resp) if resp.status().is_success() => resp.json::<Value>().await.unwrap_or(json!({})),
        _ => return tool_text("The models' catalogue doesn't answer now: propose making the agent yourself.", false),
    };
    tool_text(&catalogue_text(&answer), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connector_item() -> Value {
        json!({"name": "Garmin", "description": "Envoie tes séances", "icon": "fitness", "sites": ["connect.garmin.example"],
               "kind": "mcp", "entry": "python3 server.py", "tools": [{"name": "push", "description": "Pushes"}],
               "secrets": [{"name": "garmin", "kind": "password", "identifier_label": "E-mail Garmin"}],
               "schedule": {"when": "daily 07:00", "mechanical": true, "tool": "push", "arguments": {}},
               "unofficial": true, "files": [{"path": "server.py", "exec": false, "text": "print(1)"}]})
    }

    fn bundle() -> Value {
        json!({"format": FORMAT, "version": 1, "name": "Coach", "look": {"category": "wellness"},
               "instructions": "Tu es un coach.",
               "skills": [{"name": "plan", "files": [{"path": "SKILL.md", "text": "---\nname: plan\n---\n"}]}],
               "routines": [{"name": "Matin", "prompt": "Prépare", "schedule": {"kind": "cron", "expr": "0 7 * * *"}}],
               "connectors": [connector_item()]})
    }

    #[test]
    fn a_good_bundle_passes_and_bad_ones_say_why() {
        check_bundle(&bundle()).unwrap();
        let mut b = bundle();
        b["version"] = json!(2);
        assert!(check_bundle(&b).is_err());
        for path in ["../x", "/etc/passwd", ".hidden", "a/../../b", "a//b"] {
            let mut b = bundle();
            b["connectors"][0]["files"][0]["path"] = json!(path);
            assert!(check_bundle(&b).unwrap_err().contains("plain relative path"), "{path}");
        }
        let mut b = bundle();
        b["connectors"][0]["sites"] = json!(["not a host!"]);
        assert!(check_bundle(&b).is_err());
        let mut b = bundle();
        b["connectors"][0]["secrets"] = json!([{"name": "Bad Name"}]);
        assert!(check_bundle(&b).is_err());
        let mut b = bundle();
        b["skills"][0]["name"] = json!("../evil");
        assert!(check_bundle(&b).is_err());
        let mut b = bundle();
        b["connectors"] = json!([connector_item(), connector_item()]);
        assert!(check_bundle(&b).unwrap_err().contains("twice"));
    }

    #[test]
    fn an_imported_connector_carries_hints_and_no_state() {
        let (conn, files) = checked_connector(&connector_item()).unwrap();
        assert!(
            conn.from_template && conn.status.is_empty() && conn.fingerprint.is_empty() && conn.proxy_token.is_empty()
        );
        assert_eq!(conn.secret_hints[0].identifier_label, "E-mail Garmin");
        assert_eq!(conn.folder, "/workspace/connecteurs/garmin");
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn routines_land_paused_without_the_creators_targets() {
        let skills: BTreeSet<String> = ["plan".to_string()].into_iter().collect();
        let job = hermes_job(
            &json!({"name": "Matin", "prompt": "Prépare", "skills": ["plan", "other"],
                                     "schedule": {"kind": "cron", "expr": "0 7 * * *"}}),
            &skills,
        )
        .unwrap();
        assert_eq!(
            (job["enabled"].clone(), job["state"].clone(), job["deliver"].clone()),
            (json!(false), json!("paused"), json!("local"))
        );
        assert_eq!(job["skills"], json!(["plan"]));
        assert_eq!(job["origin"], Value::Null);
        assert!(hermes_job(&json!({"prompt": "x", "schedule": {"kind": "once", "run_at": "2026-01-01"}}), &skills)
            .is_none());
        assert!(
            hermes_job(&json!({"prompt": "x", "schedule": {"kind": "cron", "expr": "rm -rf /"}}), &skills).is_none()
        );
        // what a job may carry: never a script, a monitor or a delivery
        let shared = shareable_job(
            &json!({"id": "a", "name": "n", "prompt": "p", "deliver": "telegram:123", "origin": {"chat": 1},
                                           "schedule": {"kind": "cron", "expr": "0 7 * * *", "display": "0 7 * * *"}}),
        )
        .unwrap();
        assert_eq!(
            shared.as_object().unwrap().keys().cloned().collect::<Vec<_>>(),
            ["name", "prompt", "schedule", "skills"]
        );
        assert!(shareable_job(
            &json!({"prompt": "p", "script": "x.py", "schedule": {"kind": "cron", "expr": "0 7 * * *"}})
        )
        .is_none());
    }

    #[test]
    fn the_catalogue_reads_as_links() {
        assert!(catalogue_text(&json!({"templates": []})).starts_with("No ready-made model"));
        let text = catalogue_text(
            &json!({"templates": [{"id": "AbCdEfGhIjKlMnOpQrStUv", "name": "Coach", "intro": "Court"}]}),
        );
        assert!(text.contains("https://bipagents.com/m/AbCdEfGhIjKlMnOpQrStUv"), "{text}");
    }
}
