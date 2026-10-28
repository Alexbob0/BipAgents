//! Token diet, phase 2 (2026-10-26, « ranger, pas retirer »): what each agent's Hermes shows the model on every call,
//! by kind of agent. Nothing is taken away: a tool the kind rarely needs goes behind Hermes' tool search
//! (`tool_search` → `tool_describe` → `tool_call`) instead of costing its whole schema on each call, and the bridge
//! tools the kind uses often come out of it, so the agent stops spending a discovery round-trip (a full call each)
//! before using them. Measured on 2026-10-24 (scripts/token-lab): ≈ 19.3k input tokens per call, 11.8k of them the
//! schemas of 22 Hermes core tools, and 40 of 126 tool calls were discovery.
//!
//! Written in each agent's profile (`config.yaml`):
//! - `tools.tool_search.defer`: Hermes' own default list (an explicit list replaces it), the core tools the kind
//!   defers, and `!mcp__bipagents__<tool>` for the bridge tools kept direct (Hermes patch 0009; without it those
//!   entries match nothing and the bridge tools simply stay behind the search, as before). `!bipagents` marks the
//!   list as this bridge's: a list the person wrote is left alone.
//! - `skills.compact_categories`: Hermes' bundled skill categories the kind rarely needs, listed by name only in the
//!   system prompt (patch 0009); they still load with `skill_view`.
//! - the visible browser's MCP server without its resource and prompt helpers (4 entries of the search's catalog the
//!   agents never use).
//!
//! - every tool the agent's model called directly in its conversations of the last `CALLED_DAYS` days stays direct
//!   (a bridge or other MCP tool gets its `!` entry, a core tool leaves the list), whatever its kind's plan says. A
//!   model imitates its history: DeepSeek-V4 on the person's own server (Wellness, 2026-10-27), with many direct calls
//!   to a tool the diet had since deferred, kept calling it directly in its native markup; the server parses calls to
//!   the request's tools only, so the call came back as text (or nothing) and the turn ended empty.
//!
//! The kind: the guide, else the kind whose starting personality the agent's SOUL.md still has, else the category of
//! its Bip in the apps, else « custom ». Applied whenever the bridge gives an agent its tools (`agent_tools::register`:
//! at start for every agent — the migration of existing ones — and when a tool is shared or turned on), and when the
//! vault's items change (its tools come out of the search only for an agent that holds one).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents_admin::pyyaml::Yaml;
use crate::config::{AgentConfig, Config};

/// Hermes 517b5e10f's `tools.tool_search.defer` default (hermes_cli/config_defaults.py): kept, since an explicit list
/// replaces it wholesale.
pub const HERMES_DEFAULT_DEFERRED: [&str; 18] = [
    "computer_use",
    "session_search",
    "image_generate",
    "todo_list",
    "process_manage",
    "cronjob_manage",
    "drive_preview",
    "gui_tour",
    "desktop_preview",
    "annotate_preview",
    "manage_catalog",
    "show_tip",
    "desktop_project",
    "close_terminal",
    "apply_layout",
    "read_terminal",
    "read_window_below",
    "focus_pane",
];

/// Marks a defer list as written by this bridge (a `!name` that is no tool).
pub const MARK: &str = "!bipagents";

/// Deferred for every kind: sub-agents (the bridge's background tasks do long work: `start_task`), Hermes' own
/// speech (the bridge's `create_audio` speaks with the agent's voice).
pub const ALWAYS_DEFERRED: [&str; 2] = ["delegate_task", "text_to_speech"];

/// Hermes' vault tools (patch 0007): direct only for an agent that holds an item of the vault.
pub const VAULT_TOOLS: [&str; 5] = [
    "browser_vault_list",
    "browser_vault_unlock",
    "browser_vault_fill",
    "browser_vault_save_login",
    "browser_vault_enter_code",
];

/// Writing skills: direct only for an agent that has written some (it keeps doing so).
pub const SKILL_WRITING: &str = "skill_manage";

/// At most this many bridge tools out of the search.
pub const MAX_DIRECT: usize = 12;

/// The kind's frequent bridge tools in view. First measure (2026-10-26, eight cheap ones in view): the model (GLM-5.3
/// Flash via EUrouter, tool names constrained to the request's) also called the deferred ones directly, each call
/// landing on the in-view tool sharing the longest prefix with the other tool's arguments (`notebook_note` →
/// `notebook_read`, `map_image` → `mail_search`): 14 misrouted calls in 22 tasks. Since then the frequent tools are all
/// in view (fewer deferred ones to mistake) and the bridge runs the tool a call's arguments fit (`reroute`).
pub const DIRECT_BRIDGE_TOOLS: bool = true;

/// The bridge's tools as Hermes names them.
pub const BRIDGE_PREFIX: &str = "mcp__bipagents__";

/// The visible browser's MCP entry (browsers.rs).
pub const BROWSER_SERVER: &str = "stealth";

/// The visible browser's tools kept out of the search for an agent that has it (its MCP server in the profile). Behind
/// the search, a fresh agent asked to open a page had no browser tool in view: on 2026-10-27 (token lab, GLM-5.3 Flash,
/// names constrained to the request's tools) its call ended on `calendar_add`, and the turn went round in circles on
/// the calendar (loop_brake.rs). Its other tools (tabs, scripts, closing…) stay behind the search.
pub const BROWSER_DIRECT: [&str; 6] =
    ["spawn_browser", "navigate", "get_page_content", "click_element", "type_text", "take_screenshot"];

/// Hermes' iteration cap (`agent.max_turns`) written in a profile that has none: model calls in one turn.
pub const MAX_TURNS: u32 = 150;

/// How far back the tools an agent's model called directly stay in view.
pub const CALLED_DAYS: f64 = 30.0;

/// Hermes' tool search tools: calls through them are not direct calls of a deferred tool.
const SEARCH_TOOLS: [&str; 3] = ["tool_search", "tool_describe", "tool_call"];

/// What a kind of agent keeps in view.
#[derive(Debug, Clone, PartialEq)]
pub struct KindDiet {
    /// Core tools behind the search for this kind (besides `ALWAYS_DEFERRED`, the vault's and `skill_manage`).
    pub deferred: &'static [&'static str],
    /// Bridge tools out of the search, most used first (those the agent has, up to `MAX_DIRECT`).
    pub direct: &'static [&'static str],
    /// Hermes' bundled skill categories listed by name only.
    pub compact_skills: &'static [&'static str],
}

const CODE_AND_EDITS: &[&str] = &["execute_code", "patch", "search_files"];
const NO_FILE_WRITES: &[&str] = &["execute_code", "write_file", "patch", "search_files"];
const NOT_CODING: &[&str] = &["autonomous-ai-agents", "software-development", "social-media"];

/// The kinds (souls::CATEGORIES), the guide and « custom ».
pub fn kind_diet(kind: &str) -> KindDiet {
    match kind {
        "daily" => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &[
                "notebook_search",
                "calendar_list",
                "reminders_list",
                "weather",
                "reminders_add",
                "calendar_add",
                "start_task",
                "notebook_note",
                "create_page",
                "map_image",
                "tell_family",
                "propose_task",
                "mail_search",
            ],
            compact_skills: &[
                "autonomous-ai-agents",
                "software-development",
                "social-media",
                "creative",
                "media",
                "research",
            ],
        },
        "wellness" => KindDiet {
            deferred: NO_FILE_WRITES,
            direct: &[
                "health",
                "notebook_search",
                "notebook_note",
                "notebook_read",
                "reminders_add",
                "weather",
                "start_task",
                "propose_task",
                "create_page",
                "tell_family",
            ],
            compact_skills: &[
                "autonomous-ai-agents",
                "software-development",
                "social-media",
                "creative",
                "media",
                "email",
                "productivity",
            ],
        },
        "finance" => KindDiet {
            deferred: &["patch", "search_files"],
            direct: &[
                "notebook_search",
                "notebook_read",
                "notebook_note",
                "archive_search",
                "create_page",
                "start_task",
                "propose_task",
                "mail_search",
                "tell_family",
            ],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "creative", "media"],
        },
        "home" => KindDiet {
            deferred: NO_FILE_WRITES,
            direct: &[
                "notebook_search",
                "notebook_note",
                "reminders_add",
                "reminders_list",
                "calendar_list",
                "calendar_add",
                "weather",
                "start_task",
                "propose_task",
                "create_page",
                "tell_family",
            ],
            compact_skills: &[
                "autonomous-ai-agents",
                "software-development",
                "social-media",
                "creative",
                "media",
                "research",
                "email",
            ],
        },
        "work" => KindDiet {
            deferred: &[],
            direct: &[
                "calendar_list",
                "calendar_add",
                "mail_search",
                "mail_read",
                "mail_attachment",
                "mail_draft",
                "notebook_search",
                "notebook_note",
                "start_task",
                "propose_task",
                "create_page",
                "reminders_add",
            ],
            compact_skills: &["autonomous-ai-agents", "social-media", "creative", "media"],
        },
        "learning" => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &[
                "notebook_search",
                "notebook_note",
                "create_podcast",
                "create_audio",
                "list_voices",
                "knowledge_search",
                "create_page",
                "start_task",
                "propose_task",
            ],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "email"],
        },
        "creative" => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &[
                "create_page",
                "create_audio",
                "create_podcast",
                "list_voices",
                "notebook_search",
                "notebook_note",
                "start_task",
                "propose_task",
            ],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "email"],
        },
        "tech" => KindDiet {
            deferred: &[],
            direct: &[
                "notebook_search",
                "notebook_note",
                "start_task",
                "propose_task",
                "create_page",
                "archive_search",
                "knowledge_search",
            ],
            compact_skills: &["social-media", "creative", "media"],
        },
        "guide" => KindDiet {
            deferred: &["terminal", "execute_code", "write_file", "patch", "search_files", "web_extract"],
            direct: &["propose_agent", "suggest_switch", "notebook_search"],
            compact_skills: &[
                "autonomous-ai-agents",
                "software-development",
                "social-media",
                "creative",
                "media",
                "research",
                "email",
                "productivity",
                "note-taking",
            ],
        },
        _ => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &[
                "notebook_search",
                "notebook_note",
                "start_task",
                "propose_task",
                "reminders_add",
                "calendar_list",
                "weather",
                "create_page",
            ],
            compact_skills: NOT_CODING,
        },
    }
}

/// The kind whose starting personality `soul` holds (the words right after the agent's name, in any language of the
/// templates).
pub fn kind_of_soul(soul: &str) -> Option<String> {
    for lang in crate::souls::LANGUAGES {
        for template in crate::souls::templates(Some(lang)) {
            let Some(kind) = template.get("category").and_then(Value::as_str) else { continue };
            let text = template.get("soul").map(crate::py::py_str).unwrap_or_default();
            let Some((_, after)) = text.split_once("{name}") else { continue };
            // « , l'agent du quotidien de ton utilisateur » — enough to tell the kinds apart, short enough to survive
            // the person's later edits further on.
            let words: String = after.chars().take(40).collect();
            let words = words.trim();
            if words.chars().count() >= 15 && soul.contains(words) {
                return Some(kind.to_string());
            }
        }
    }
    None
}

/// The agent's kind for the diet: « guide », a kind of souls::CATEGORIES, or « custom ».
pub fn kind_of(config: &Config, agent: &AgentConfig, soul: &str) -> String {
    if crate::guide::is_guide(config, &agent.name) {
        return "guide".into();
    }
    if let Some(kind) = kind_of_soul(soul).filter(|k| crate::souls::CATEGORIES.contains(&k.as_str())) {
        return kind;
    }
    match agent.category.as_deref() {
        Some(c) if crate::souls::CATEGORIES.contains(&c) => c.to_string(),
        _ => "custom".into(),
    }
}

/// What the agent's Hermes is told.
#[derive(Debug, Clone, PartialEq)]
pub struct Diet {
    pub kind: String,
    /// `tools.tool_search.defer`.
    pub defer: Vec<String>,
    /// `skills.compact_categories`.
    pub compact_skills: Vec<String>,
}

/// The diet of an agent of `kind` that has `bridge_tools`, its visible browser or not, holds a vault item or not, has
/// written skills or not, and whose model `called` these tools directly lately (Hermes' names:
/// `mcp__bipagents__health`, `execute_code`…).
pub fn diet(
    kind: &str,
    bridge_tools: &[String],
    browser: bool,
    holds_vault_item: bool,
    writes_skills: bool,
    called: &[String],
) -> Diet {
    let plan = kind_diet(kind);
    let mut defer: Vec<String> = vec![MARK.to_string()];
    let mut add = |name: &str| {
        if !defer.iter().any(|d| d == name) {
            defer.push(name.to_string());
        }
    };
    HERMES_DEFAULT_DEFERRED.iter().for_each(|n| add(n));
    ALWAYS_DEFERRED.iter().for_each(|n| add(n));
    plan.deferred.iter().for_each(|n| add(n));
    if !holds_vault_item {
        VAULT_TOOLS.iter().for_each(|n| add(n));
    }
    if !writes_skills {
        add(SKILL_WRITING);
    }
    if DIRECT_BRIDGE_TOOLS {
        plan.direct
            .iter()
            .filter(|name| bridge_tools.iter().any(|t| t == *name))
            .take(MAX_DIRECT)
            .for_each(|name| add(&format!("!{BRIDGE_PREFIX}{name}")));
    }
    if browser {
        BROWSER_DIRECT.iter().for_each(|name| add(&format!("!mcp__{BROWSER_SERVER}__{name}")));
    }
    // What the model has been calling directly stays direct (module doc), beyond MAX_DIRECT too.
    for name in called.iter().filter(|n| !n.is_empty() && !SEARCH_TOOLS.contains(&n.as_str())) {
        if name.starts_with("mcp__") {
            let gone = name.strip_prefix(BRIDGE_PREFIX).is_some_and(|tool| !bridge_tools.iter().any(|t| t == tool));
            let entry = format!("!{name}");
            if !gone && !defer.contains(&entry) {
                defer.push(entry);
            }
        } else {
            defer.retain(|d| d != name);
        }
    }
    Diet { kind: kind.to_string(), defer, compact_skills: plan.compact_skills.iter().map(|c| c.to_string()).collect() }
}

/// Whether `arguments` fit `tool`'s schema: no argument it doesn't know, none of its required ones missing.
pub fn fits(tool: &Value, arguments: &crate::Dict) -> bool {
    let schema = tool.get("inputSchema");
    let known = schema.and_then(|s| s.get("properties")).and_then(Value::as_object);
    let required = schema.and_then(|s| s.get("required")).and_then(Value::as_array);
    let all_known = arguments.keys().all(|k| known.is_some_and(|p| p.contains_key(k)));
    let none_missing = required.is_none_or(|r| r.iter().filter_map(Value::as_str).all(|k| arguments.contains_key(k)));
    all_known && none_missing
}

/// The tool a call was meant for, among `tools`: `name` when its arguments fit it (or fit nothing better), else the
/// only other tool they fit. With some bridge tools in view and the others behind the search, a model whose tool names
/// are constrained to the request's ends a deferred tool's name on the in-view one sharing its longest prefix
/// (`notebook_note` → `notebook_read`), its arguments still the deferred tool's: measured on 2026-10-26.
pub fn meant<'a>(tools: &'a [Value], name: &'a str, arguments: &crate::Dict) -> &'a str {
    let called = tools.iter().find(|t| t.get("name").and_then(Value::as_str) == Some(name));
    if arguments.is_empty() || called.is_none_or(|t| fits(t, arguments)) {
        return name;
    }
    let mut others = tools.iter().filter(|t| t.get("name").and_then(Value::as_str) != Some(name) && fits(t, arguments));
    match (others.next(), others.next()) {
        (Some(only), None) => only.get("name").and_then(Value::as_str).unwrap_or(name),
        _ => name,
    }
}

/// `meant` for a call the bridge received from `agent`'s Hermes, logged when it changes the tool.
pub fn reroute(config: &Config, agent: &AgentConfig, name: &str, arguments: &crate::Dict) -> String {
    let tools = crate::agent_tools::tools_for(config, agent);
    let found = meant(&tools, name, arguments).to_string();
    if found != name {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, called = %name, meant = %found,
                       "a call's arguments were another tool's: that tool run");
    }
    found
}

/// The tools the model called directly (not through Hermes' `tool_call`) in the conversations of `state_db` (a
/// profile's Hermes sessions) since `since` (Unix time), most called first. Empty when the store can't be read.
pub fn called_directly(state_db: &Path, since: f64) -> Vec<String> {
    if !state_db.is_file() {
        return Vec::new();
    }
    let read = || -> rusqlite::Result<Vec<String>> {
        let conn = rusqlite::Connection::open_with_flags(
            state_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_millis(300))?;
        let mut stmt = conn.prepare(
            "SELECT tool_calls FROM messages WHERE role = 'assistant' AND tool_calls IS NOT NULL AND timestamp >= ?1 \
             ORDER BY id DESC LIMIT 5000",
        )?;
        let rows = stmt.query_map([since], |row| row.get::<_, String>(0))?;
        let mut counts: Vec<(String, usize)> = Vec::new();
        for raw in rows.flatten() {
            let Ok(Value::Array(calls)) = serde_json::from_str::<Value>(&raw) else { continue };
            for call in calls {
                let Some(name) = call.pointer("/function/name").and_then(Value::as_str) else { continue };
                match counts.iter_mut().find(|(n, _)| n == name) {
                    Some((_, count)) => *count += 1,
                    None => counts.push((name.to_string(), 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(counts.into_iter().map(|(name, _)| name).collect())
    };
    read().unwrap_or_default()
}

/// Whether `agent` holds an item of the vault Hermes fills from (a card is paid through the bridge instead).
pub fn holds_vault_item(config: &Config, agent: &AgentConfig) -> bool {
    let vault = crate::vault::Vault::new(&crate::py::join(&config.data_dir(), "vault"));
    vault.items().iter().any(|item| {
        item.kind != crate::cards::KIND && item.kind != crate::secrets::VAULT_KIND && item.agents.contains_key(&agent.name)
    })
}

fn profile_folder(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    crate::shares::profile_dir(config, agent).filter(|f| f.join("config.yaml").is_file())
}

/// The diet of `agent` as things are now (None: an agent whose profile this bridge doesn't reach).
pub fn current(config: &Config, agent: &AgentConfig) -> Option<Diet> {
    let folder = profile_folder(config, agent)?;
    let soul = std::fs::read_to_string(folder.join("SOUL.md")).unwrap_or_default();
    let kind = kind_of(config, agent, &soul);
    let tools: Vec<String> = crate::agent_tools::tools_for(config, agent)
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
    let writes_skills = !crate::own_skills::listing(&folder).is_empty();
    let called = called_directly(&folder.join("state.db"), crate::py::time_time() - CALLED_DAYS * 86_400.0);
    let browser = crate::agents_admin::load_or_empty(&folder.join("config.yaml"))
        .ok()
        .is_some_and(|data| data.get("mcp_servers").and_then(|s| s.get(BROWSER_SERVER)).is_some_and(Yaml::is_map));
    let mut out = diet(&kind, &tools, browser, holds_vault_item(config, agent), writes_skills, &called);
    // the connectors the person approved are called like any tool (connectors.rs)
    for entry in crate::connectors::direct_entries(config, agent) {
        if !out.defer.contains(&entry) {
            out.defer.push(entry);
        }
    }
    Some(out)
}

/// `parent[key]`, made a dict when missing or not one.
fn map_at<'a>(parent: &'a mut Yaml, key: &str) -> Option<&'a mut Yaml> {
    if !parent.get(key).is_some_and(Yaml::is_map) {
        parent.insert(key, Yaml::map());
    }
    parent.get_mut(key)
}

fn strings(items: &[String]) -> Yaml {
    Yaml::Seq(items.iter().map(|s| Yaml::str(s.clone())).collect())
}

/// `data` with the diet in it; None when nothing changes, or when the person wrote their own defer list.
pub fn with_diet(data: &Yaml, diet: &Diet) -> Option<Yaml> {
    let mut data = if data.is_map() { data.clone() } else { Yaml::map() };
    let before = data.clone();
    let tools = map_at(&mut data, "tools")?;
    let search = map_at(tools, "tool_search")?;
    let theirs = match search.get("defer") {
        Some(Yaml::Seq(items)) => !items.iter().any(|i| i.as_str() == Some(MARK)),
        Some(Yaml::Null) | None => false,
        Some(_) => true,
    };
    if !theirs {
        search.insert("defer", strings(&diet.defer));
    }
    let skills = map_at(&mut data, "skills")?;
    skills.insert("compact_categories", strings(&diet.compact_skills));
    // Hermes has no iteration cap by default (`agent.max_turns` unset: unlimited, 267 model calls in one turn on
    // 2026-10-27): one, unless the profile has its own. The bridge's brake (loop_brake.rs) stops loops much earlier;
    // this is for the runs it doesn't follow (routines).
    let agent = map_at(&mut data, "agent")?;
    if agent.get("max_turns").is_none_or(|v| *v == Yaml::Null) {
        agent.insert("max_turns", Yaml::Int(MAX_TURNS as i128));
    }
    if let Some(Yaml::Map(_)) = data.get("mcp_servers").and_then(|s| s.get(BROWSER_SERVER)) {
        let entry = data.get_mut("mcp_servers").and_then(|s| s.get_mut(BROWSER_SERVER))?;
        let filter = map_at(entry, "tools")?;
        filter.insert("resources", Yaml::Bool(false));
        filter.insert("prompts", Yaml::Bool(false));
    }
    if data == before {
        None
    } else {
        Some(data)
    }
}

fn write(path: &Path, diet: &Diet) -> anyhow::Result<bool> {
    let data = crate::agents_admin::load_or_empty(path)?;
    match with_diet(&data, diet) {
        Some(changed) => {
            crate::agents_admin::replace_yaml(path, &changed)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The agent's diet written in its profile; whether it changed (Hermes reads it again from the next conversation).
pub fn apply(config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
    let Some(folder) = profile_folder(config, agent) else { return Ok(false) };
    let Some(diet) = current(config, agent) else { return Ok(false) };
    let changed = write(&folder.join("config.yaml"), &diet)?;
    if changed {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, kind = %diet.kind, "tools in view set for its kind");
    }
    Ok(changed)
}

/// Every agent's (the vault's items changed: its tools follow who holds one).
pub fn apply_all(config: &Config) {
    for agent in config.agents.values() {
        if let Err(exc) = apply(config, agent) {
            tracing::warn!(target: "bipbridge::agents", agent = %agent.name, error = %exc, "tools in view not set");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_admin::pyyaml;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_kind_and_the_guide_have_a_diet() {
        for kind in crate::souls::CATEGORIES.iter().chain(["guide", "custom"].iter()) {
            let d = kind_diet(kind);
            assert!(!d.direct.is_empty(), "{kind}");
            assert!(d.direct.len() <= MAX_DIRECT + 2, "{kind}");
        }
    }

    #[test]
    fn the_templates_souls_tell_their_kind() {
        for lang in crate::souls::LANGUAGES {
            for kind in crate::souls::CATEGORIES {
                let soul = crate::souls::soul_for("Camille", kind, Some(lang)).unwrap();
                assert_eq!(kind_of_soul(&soul).as_deref(), Some(kind), "{kind} {lang}");
            }
        }
        assert_eq!(kind_of_soul("Tu es Léo, un agent personnel de ton utilisateur."), None);
    }

    #[test]
    fn deferred_not_removed_and_bridge_tools_out_of_the_search() {
        let tools = names(&["notebook_search", "calendar_list", "create_podcast", "weather"]);
        let d = diet("daily", &tools, false, false, false, &[]);
        assert_eq!(d.defer[0], MARK);
        for name in HERMES_DEFAULT_DEFERRED.iter().chain(ALWAYS_DEFERRED.iter()).chain(VAULT_TOOLS.iter()) {
            assert!(d.defer.contains(&name.to_string()), "{name}");
        }
        assert!(d.defer.contains(&"skill_manage".to_string()));
        assert!(d.defer.contains(&"execute_code".to_string()));
        assert!(!d.defer.contains(&"terminal".to_string()));
        assert!(!d.defer.contains(&"memory".to_string()));
        assert_eq!(d.defer.contains(&"!mcp__bipagents__notebook_search".to_string()), DIRECT_BRIDGE_TOOLS);
        assert_eq!(d.defer.contains(&"!mcp__bipagents__weather".to_string()), DIRECT_BRIDGE_TOOLS);
        // a tool it doesn't have, or one its kind doesn't use much, stays behind the search
        assert!(!d.defer.iter().any(|n| n.ends_with("reminders_add") || n.ends_with("create_podcast")));
        // an agent holding a vault item, or writing skills, keeps those tools
        let d = diet("daily", &tools, false, true, true, &[]);
        assert!(!d.defer.iter().any(|n| n.starts_with("browser_vault") || n == "skill_manage"));
    }

    #[test]
    fn the_visible_browser_in_view_for_an_agent_that_has_it() {
        // 2026-10-27: « open this page in your browser », no browser tool in view, a call ending on calendar_add
        let tools = names(&["calendar_add", "calendar_list"]);
        let with = diet("daily", &tools, true, false, false, &[]);
        for name in BROWSER_DIRECT {
            assert!(with.defer.contains(&format!("!mcp__stealth__{name}")), "{name}");
        }
        let without = diet("daily", &tools, false, false, false, &[]);
        assert!(!without.defer.iter().any(|n| n.contains("stealth")));
    }

    #[test]
    fn tools_the_model_has_been_calling_stay_direct() {
        // Wellness, 2026-10-27: its history called a tool directly that its kind's plan leaves behind the search.
        let tools = names(&["health", "notebook_read", "mail_search"]);
        let called = names(&[
            "mcp__bipagents__notebook_read",
            "mcp__bipagents__gone_tool",
            "mcp__stealth__navigate",
            "execute_code",
            "tool_call",
        ]);
        let d = diet("wellness", &tools, false, false, false, &called);
        assert!(d.defer.contains(&"!mcp__bipagents__notebook_read".to_string()));
        assert!(d.defer.contains(&"!mcp__stealth__navigate".to_string()));
        assert!(!d.defer.contains(&"execute_code".to_string()), "a core tool it calls is not deferred");
        assert!(!d.defer.iter().any(|n| n.contains("gone_tool") || n == "!tool_call"));
        assert!(d.defer.contains(&"!mcp__bipagents__health".to_string()));
        assert!(!d.defer.iter().any(|n| n.ends_with("mail_search")), "not called, not in the plan: searched");
    }

    #[test]
    fn called_directly_reads_the_profiles_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, \
             tool_calls TEXT, timestamp REAL);",
        )
        .unwrap();
        let call = |name: &str| format!(r#"[{{"id":"c","type":"function","function":{{"name":"{name}","arguments":"{{}}"}}}}]"#);
        for (name, ts) in [("mcp__bipagents__health", 100.0), ("mcp__bipagents__health", 101.0), ("weather", 102.0), ("old", 1.0)] {
            conn.execute(
                "INSERT INTO messages (session_id, role, tool_calls, timestamp) VALUES ('s', 'assistant', ?1, ?2)",
                rusqlite::params![call(name), ts],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO messages (session_id, role, tool_calls, timestamp) VALUES ('s', 'assistant', 'oops', 103)", [])
            .unwrap();
        drop(conn);
        assert_eq!(called_directly(&path, 50.0), names(&["mcp__bipagents__health", "weather"]));
        assert!(called_directly(&dir.path().join("none.db"), 0.0).is_empty());
    }

    #[test]
    fn a_call_runs_the_tool_its_arguments_fit() {
        let tool = |name: &str, props: &[&str], required: &[&str]| {
            let properties: serde_json::Map<String, Value> =
                props.iter().map(|p| (p.to_string(), serde_json::json!({"type": "string"}))).collect();
            serde_json::json!({"name": name, "inputSchema": {"type": "object", "properties": properties, "required": required}})
        };
        let tools = vec![
            tool("notebook_search", &["words", "kind"], &[]),
            tool("notebook_read", &["page"], &["page"]),
            tool("notebook_note", &["page", "facts", "section"], &["page", "facts"]),
            tool("mail_search", &["words", "days"], &[]),
            tool("map_image", &["markers", "title", "line", "color"], &["markers"]),
        ];
        let args =
            |pairs: &[&str]| -> crate::Dict { pairs.iter().map(|k| (k.to_string(), Value::from("x"))).collect() };
        // its own arguments: itself
        assert_eq!(meant(&tools, "notebook_read", &args(&["page"])), "notebook_read");
        // the 2026-10-26 lab's misroutes
        assert_eq!(meant(&tools, "notebook_read", &args(&["page", "facts", "section"])), "notebook_note");
        assert_eq!(meant(&tools, "mail_search", &args(&["markers", "line", "color"])), "map_image");
        assert_eq!(meant(&tools, "notebook_search", &args(&["page"])), "notebook_read");
        // nothing else fits, two others fit, no arguments, or an unknown tool: left as called
        assert_eq!(meant(&tools, "mail_search", &args(&["zzz"])), "mail_search");
        assert_eq!(meant(&tools, "map_image", &args(&["words"])), "map_image");
        assert_eq!(meant(&tools, "start_task", &crate::Dict::new()), "start_task");
        assert_eq!(meant(&tools, "other", &args(&["page"])), "other");
    }

    #[test]
    fn at_most_eight_bridge_tools_direct() {
        let all: Vec<String> = kind_diet("daily").direct.iter().map(|s| s.to_string()).collect();
        let d = diet("daily", &all, false, false, false, &[]);
        let expected = if DIRECT_BRIDGE_TOOLS { MAX_DIRECT } else { 0 };
        assert_eq!(d.defer.iter().filter(|n| n.starts_with("!mcp__")).count(), expected);
    }

    #[test]
    fn written_once_and_a_persons_own_list_kept() {
        let d = diet("wellness", &names(&["health"]), false, false, false, &[]);
        let data = pyyaml::load("model:\n  default: x\nmcp_servers:\n  stealth:\n    url: http://a\n").unwrap();
        let changed = with_diet(&data, &d).expect("changed");
        assert_eq!(
            changed
                .get("mcp_servers")
                .and_then(|s| s.get("stealth"))
                .and_then(|s| s.get("tools"))
                .and_then(|t| t.get("prompts")),
            Some(&Yaml::Bool(false))
        );
        let written = changed.get("tools").and_then(|t| t.get("tool_search")).and_then(|t| t.get("defer")).unwrap();
        assert!(matches!(written, Yaml::Seq(items) if items.len() == d.defer.len()));
        assert_eq!(changed.get("model").and_then(|m| m.get("default")).and_then(Yaml::as_str), Some("x"));
        assert!(with_diet(&changed, &d).is_none(), "the same again changes nothing");
        let cap = |data: &Yaml| data.get("agent").and_then(|a| a.get("max_turns")).cloned();
        assert_eq!(cap(&changed), Some(Yaml::Int(MAX_TURNS as i128)));
        let own = pyyaml::load("agent:
  max_turns: 40
").unwrap();
        assert_eq!(cap(&with_diet(&own, &d).unwrap()), Some(Yaml::Int(40)), "its own cap kept");
        // the person's own list
        let theirs = pyyaml::load("tools:\n  tool_search:\n    defer: [todo_list]\n").unwrap();
        let kept = with_diet(&theirs, &d).expect("skills still written");
        let defer = kept.get("tools").and_then(|t| t.get("tool_search")).and_then(|t| t.get("defer")).unwrap();
        assert_eq!(defer, &Yaml::Seq(vec![Yaml::str("todo_list")]));
    }
}
