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
//! - `clarify` in `platform_toolsets.api_server` (`with_clarify`): the app's question card needs it, Hermes 0.22.5's
//!   API toolset leaves it out.
//!
//! - `tools.order_first` ([`COMMON_FIRST`]): the tools every agent has, first in the model's tool list, in one fixed
//!   order (Hermes patch 0022), then the agent's own sorted. The list is fixed per kind of agent: nothing in it
//!   follows the person's history (cache audit 2026-11-03, #1: each agent's first call read nothing from the cache,
//!   its list differing from the others' at the 2nd tool, and from another person's by what their model called lately).
//!
//! The kind: the guide, else the kind whose starting personality the agent's SOUL.md still has, else the category of
//! its Bip in the apps, else « custom ». Applied whenever the bridge gives an agent its tools (`agent_tools::register`:
//! at start for every agent — the migration of existing ones — and when a tool is shared or turned on), and when the
//! vault's items change (its tools come out of the search only for an agent that holds one).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agents_admin::pyyaml::Yaml;
use crate::config::{AgentConfig, Config};

/// Hermes 0.22.5's `tools.tool_search.defer` default (hermes_cli/config_defaults.py): kept, since an explicit list
/// replaces it wholesale. Its `browser_vault` is a `VAULT_TOOLS` entry here.
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
    "set_theme",
    "read_terminal",
    "read_window_below",
    "focus_pane",
];

/// Marks a defer list as written by this bridge (a `!name` that is no tool).
pub const MARK: &str = "!bipagents";

/// Deferred for every kind: sub-agents (the bridge's background tasks do long work: `start_task`), Hermes' own
/// speech (the bridge's `create_audio` speaks with the agent's voice).
pub const ALWAYS_DEFERRED: [&str; 2] = ["delegate_task", "text_to_speech"];

/// Hermes' vault tool (one `browser_vault` with an `action`, patch 0007): direct only for an agent that holds an item
/// of the vault.
pub const VAULT_TOOLS: [&str; 1] = ["browser_vault"];

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

/// A bridge tool kept direct brings the one it leads to (first, then), when the agent has it. Behind the search,
/// `mail_read` cost the real-day bench's loops (2026-10-29, GLM-5.3 Flash): after `tool_describe` on it the model
/// called `mail_search` again with the same arguments 14 to 17 times, never `tool_call`, until the brake refused.
pub const DIRECT_PAIRS: [(&str, &str); 1] = [("mail_search", "mail_read")];

/// The guide's own tools in view, besides its daily regime (beyond `MAX_DIRECT`). Its earlier regime of its own (these
/// and `notebook_search` only, mail, notes, calendar, reminders, pages and tasks behind the search) cost the real-day
/// bench of 2026-10-30 (GLM-5.3 Flash, the offer's first agent made the guide): 11/16 and two loops on
/// `notebook_search`, against 13/16 for the same agent on the daily regime.
pub const GUIDE_DIRECT: [&str; 3] = ["propose_agent", "propose_team", "suggest_switch"];

/// Hermes' iteration cap (`agent.max_turns`) written in a profile that has none: model calls in one turn.
pub const MAX_TURNS: u32 = 150;

/// The tools every agent has (24 measured identical on the four agents of the real-day bench, ≈ 9.1k tokens),
/// first in each agent's tool list in this order (Hermes' sorted order of their names), so that any agent's first call
/// reads them from the cache another agent left (`tools.order_first`, Hermes patch 0022). Those of the bridge are in
/// view for every kind ([`diet`]); a tool an agent lacks is skipped (the prefix stops being common there).
pub const COMMON_FIRST: [&str; 24] = [
    "clarify",
    "manage_connections",
    "mcp__bipagents__create_page",
    "mcp__bipagents__give_file",
    "mcp__bipagents__map_image",
    "mcp__bipagents__notebook_search",
    "mcp__bipagents__propose_task",
    "mcp__bipagents__start_task",
    "mcp__stealth__click_element",
    "mcp__stealth__get_page_content",
    "mcp__stealth__navigate",
    "mcp__stealth__spawn_browser",
    "mcp__stealth__take_screenshot",
    "mcp__stealth__type_text",
    "memory",
    "read_file",
    "skill_view",
    "skills_list",
    "terminal",
    "tool_call",
    "tool_describe",
    "vision_analyze",
    "web_extract",
    "web_search",
];

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
        // the guide is also its person's everyday agent (the offer's first agent, « Bip »): the daily regime, plus
        // its own cards in view (`GUIDE_DIRECT`, in `diet`)
        "daily" | "guide" => KindDiet {
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
        // the records (subscriptions, purchases, contracts) and their corrections in view: behind the search, the
        // carnet review's lab (2026-11-03, GLM-5.3 Flash) filed seven subscriptions as notes on « person » pages named
        // « Abonnements » and « Anthropic », and wrote its corrections as facts (« Remplacer la ligne… »)
        "finance" => KindDiet {
            deferred: &["patch", "search_files"],
            direct: &[
                "notebook_search",
                "notebook_record",
                "notebook_read",
                "notebook_note",
                "notebook_edit",
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
/// written skills or not. Nothing of the person's history: the same for every agent of a kind with the same tools.
pub fn diet(kind: &str, bridge_tools: &[String], browser: bool, holds_vault_item: bool, writes_skills: bool) -> Diet {
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
        // asking another agent (agent_messages.rs), whatever the kind: behind the search, the real-day bench's agent
        // hunted for it with 9 tool searches
        if bridge_tools.iter().any(|t| t == crate::agent_messages::TOOL_NAME) {
            add(&format!("!{BRIDGE_PREFIX}{}", crate::agent_messages::TOOL_NAME));
        }
        // giving the person a file it made (give_file.rs), whatever the kind: behind the search, GLM often stopped at
        // tool_describe and made a page instead, or looped (release bench of 31/10: 2/5 files given, 4/4 before)
        if bridge_tools.iter().any(|t| t == crate::give_file::TOOL_NAME) {
            add(&format!("!{BRIDGE_PREFIX}{}", crate::give_file::TOOL_NAME));
        }
        // maps (maps.rs), whatever the kind: behind the search, asked for « une carte », the agent made a picture or a
        // page with coordinates of its own, which fell in the sea (2026-10-31)
        if bridge_tools.iter().any(|t| t == crate::maps::TOOL_NAME) {
            add(&format!("!{BRIDGE_PREFIX}{}", crate::maps::TOOL_NAME));
        }
        if kind == "guide" {
            GUIDE_DIRECT
                .iter()
                .filter(|name| bridge_tools.iter().any(|t| t == *name))
                .for_each(|name| add(&format!("!{BRIDGE_PREFIX}{name}")));
        }
    }
    if browser {
        BROWSER_DIRECT.iter().for_each(|name| add(&format!("!mcp__{BROWSER_SERVER}__{name}")));
    }
    // The common tools of the bridge, whatever the kind (`COMMON_FIRST`).
    for name in COMMON_FIRST.iter().filter_map(|n| n.strip_prefix(BRIDGE_PREFIX)) {
        if bridge_tools.iter().any(|t| t == name) {
            add(&format!("!{BRIDGE_PREFIX}{name}"));
        }
    }
    // Their companions next to them, beyond MAX_DIRECT (`DIRECT_PAIRS`).
    for (first, then) in DIRECT_PAIRS {
        let first = format!("!{BRIDGE_PREFIX}{first}");
        let then_entry = format!("!{BRIDGE_PREFIX}{then}");
        if defer.contains(&first) && bridge_tools.iter().any(|t| t == then) && !defer.contains(&then_entry) {
            defer.push(then_entry);
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

/// Tools that act or propose a card for the person: a call with another tool's arguments is never run as that other
/// tool when either is one of these (2026-10-30 bench, DeepSeek: `propose_task` written with `propose_agent`'s
/// arguments made an agent card). The call then fails on its own tool's arguments and the model calls the right one.
pub const NEVER_REROUTED: [&str; 10] = [
    "propose_agent",
    "propose_team",
    "propose_room",
    "propose_profile",
    "suggest_switch",
    "create_project_agent",
    "rename_yourself",
    "propose_task",
    "start_task",
    "tell_family",
];

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
        (Some(only), None) => match only.get("name").and_then(Value::as_str) {
            Some(other) if !NEVER_REROUTED.contains(&other) && !NEVER_REROUTED.contains(&name) => other,
            _ => name,
        },
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

/// Whether `agent` holds an item of the vault Hermes fills from (a card is paid through the bridge instead; an item
/// kept by the bridge only, handle `bridge`, is not in its Hermes vault: « Coordonnées » of an agent without its
/// browser, vault.rs `give`).
pub fn holds_vault_item(config: &Config, agent: &AgentConfig) -> bool {
    let vault = crate::vault::Vault::new(&crate::py::join(&config.data_dir(), "vault"));
    vault.items().iter().any(|item| {
        item.kind != crate::cards::KIND
            && item.kind != crate::secrets::VAULT_KIND
            && item.agents.get(&agent.name).is_some_and(|handle| handle != crate::vault::CARD_HANDLE)
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
    // skills it wrote itself: a copy another agent's library gave doesn't count (skill_library.rs)
    let writes_skills = crate::skill_library::annotate(config, &agent.name, crate::own_skills::listing(&folder))
        .iter()
        .any(|s| s.get("library").and_then(|l| l.get("made_by")).and_then(Value::as_str).is_none_or(|m| m == agent.name));
    let browser = crate::agents_admin::load_or_empty(&folder.join("config.yaml"))
        .ok()
        .is_some_and(|data| data.get("mcp_servers").and_then(|s| s.get(BROWSER_SERVER)).is_some_and(Yaml::is_map));
    let mut out = diet(&kind, &tools, browser, holds_vault_item(config, agent), writes_skills);
    // the connectors the person approved are called like any tool (connectors.rs)
    for entry in crate::connectors::direct_entries(config, agent) {
        if !out.defer.contains(&entry) {
            out.defer.push(entry);
        }
    }
    Some(out)
}

fn strings(items: &[String]) -> Yaml {
    Yaml::Seq(items.iter().map(|s| Yaml::str(s.clone())).collect())
}

/// `data` with the diet in it; None when nothing changes, or when the person wrote their own defer list.
pub fn with_diet(data: &Yaml, diet: &Diet) -> Option<Yaml> {
    let mut data = if data.is_map() { data.clone() } else { Yaml::map() };
    let before = data.clone();
    let tools = data.section("tools")?;
    let search = tools.section("tool_search")?;
    let theirs = match search.get("defer") {
        Some(Yaml::Seq(items)) => !items.iter().any(|i| i.as_str() == Some(MARK)),
        Some(Yaml::Null) | None => false,
        Some(_) => true,
    };
    if !theirs {
        search.insert("defer", strings(&diet.defer));
    }
    tools.insert("order_first", Yaml::Seq(COMMON_FIRST.iter().map(|n| Yaml::str(*n)).collect()));
    let skills = data.section("skills")?;
    skills.insert("compact_categories", strings(&diet.compact_skills));
    // Hermes has no iteration cap by default (`agent.max_turns` unset: unlimited, 267 model calls in one turn on
    // 2026-10-27): one, unless the profile has its own. The bridge's brake (loop_brake.rs) stops loops much earlier;
    // this is for the runs it doesn't follow (routines).
    let agent = data.section("agent")?;
    if agent.get("max_turns").is_none_or(|v| *v == Yaml::Null) {
        agent.insert("max_turns", Yaml::Int(MAX_TURNS as i128));
    }
    // A routine the agent makes in a conversation reports there (cronwatch.rs): Hermes (patch 0019) keeps the
    // conversation on the job and tells the model so, instead of « local-only: set deliver='telegram' », unless the
    // profile says otherwise.
    let cron = data.section("cron")?;
    if cron.get("session_reports").is_none_or(|v| *v == Yaml::Null) {
        cron.insert("session_reports", Yaml::Bool(true));
    }
    with_clarify(&mut data);
    if let Some(Yaml::Map(_)) = data.get("mcp_servers").and_then(|s| s.get(BROWSER_SERVER)) {
        let entry = data.get_mut("mcp_servers").and_then(|s| s.get_mut(BROWSER_SERVER))?;
        let filter = entry.section("tools")?;
        filter.insert("resources", Yaml::Bool(false));
        filter.insert("prompts", Yaml::Bool(false));
    }
    if data == before {
        None
    } else {
        Some(data)
    }
}

/// The app's question card (`clarify.request`, QuestionCard.swift / ChatModel.kt) needs Hermes' `clarify` tool, and
/// Hermes 0.22.5's `hermes-api-server` toolset leaves it out (« no interactive UI tools like clarify »): an agent
/// without it says it cannot ask. The apps never show that switch (AgentTools.swift `hidden`), so it is always on:
/// `platform_toolsets.api_server` gets `clarify` added to the list it has (a list `hermes tools enable/disable`
/// saved keeps every other choice), or, with none saved, the platform default plus it (`[hermes-api-server,
/// clarify]`: Hermes reads a mixed list as the composite's toolsets and the named one). Hermes reads it again for
/// each run; an existing conversation gets the new tool appended to its pinned tools[] (tools/mcp_tool_agent.py
/// `restore_agent_tool_prefix`) and a Bot Chat rebuilds its prompt once (its capability epoch covers
/// `platform_toolsets`).
pub fn with_clarify(data: &mut Yaml) {
    let Some(platforms) = data.section("platform_toolsets") else { return };
    match platforms.get_mut(API_PLATFORM) {
        Some(Yaml::Seq(items)) => {
            if !items.iter().any(|i| i.as_str() == Some(CLARIFY)) {
                items.push(Yaml::str(CLARIFY));
            }
        }
        _ => platforms.insert(API_PLATFORM, strings(&[API_DEFAULT_TOOLSET.to_string(), CLARIFY.to_string()])),
    }
}

/// Hermes' platform of the apps' agents, its default toolset (hermes_cli/platforms.py), the question tool.
pub const API_PLATFORM: &str = "api_server";
pub const API_DEFAULT_TOOLSET: &str = "hermes-api-server";
pub const CLARIFY: &str = "clarify";

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
        let d = diet("daily", &tools, false, false, false);
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
        let d = diet("daily", &tools, false, true, true);
        assert!(!d.defer.iter().any(|n| n.starts_with("browser_vault") || n == "skill_manage"));
        // Hermes 0.22.5's own deferred ones kept, its vault tool among them for an agent holding nothing
        let d = diet("daily", &tools, false, false, false);
        assert!(["set_theme", "browser_vault"].iter().all(|n| d.defer.contains(&n.to_string())));
        assert!(!d.defer.iter().any(|n| n == "apply_layout" || n.starts_with("browser_vault_")));
    }

    #[test]
    fn the_map_tool_is_in_view_for_every_kind() {
        // 2026-10-31: asked for « une carte », an agent with map_image behind the search made a picture, then a page
        // with its own coordinates (in the sea)
        let tools = names(&["notebook_search", "create_page", "map_image"]);
        for kind in crate::souls::CATEGORIES.iter().chain(["guide", "custom"].iter()) {
            let d = diet(kind, &tools, false, false, false);
            assert!(d.defer.contains(&"!mcp__bipagents__map_image".to_string()), "{kind}");
        }
        let without = diet("finance", &names(&["create_page"]), false, false, false);
        assert!(!without.defer.iter().any(|n| n.ends_with("map_image")));
        // the records and their corrections in view where money is kept (carnet review, 2026-11-03)
        let tools = names(&["notebook_record", "notebook_edit", "notebook_note"]);
        let finance = diet("finance", &tools, false, false, false, &[]);
        for tool in ["notebook_record", "notebook_edit"] {
            assert!(finance.defer.contains(&format!("!{BRIDGE_PREFIX}{tool}")), "{tool}: {:?}", finance.defer);
        }
    }

    #[test]
    fn the_guide_keeps_the_daily_regime_plus_its_cards() {
        // 2026-10-30: the offer's first agent made the guide lost mail, notes, calendar… (real day 11/16)
        let mut tools: Vec<String> = kind_diet("daily").direct.iter().map(|s| s.to_string()).collect();
        tools.retain(|t| t != "tell_family"); // an install without family: the daily kind's twelve
        tools.push("mail_read".into());
        let daily = diet("daily", &tools, false, false, false);
        tools.extend(names(&["propose_agent", "propose_team", "suggest_switch", "propose_room", "propose_profile"]));
        let guide = diet("guide", &tools, false, false, false);
        assert_eq!(guide.compact_skills, daily.compact_skills);
        let extra: Vec<&String> = guide.defer.iter().filter(|d| !daily.defer.contains(d)).collect();
        let expected: Vec<String> = GUIDE_DIRECT.iter().map(|n| format!("!{BRIDGE_PREFIX}{n}")).collect();
        assert_eq!(extra, expected.iter().collect::<Vec<_>>(), "only its cards on top of the daily regime");
        assert!(daily.defer.iter().all(|d| guide.defer.contains(d)));
        for name in ["mail_search", "mail_read", "notebook_note", "calendar_list", "reminders_add", "create_page", "start_task"] {
            assert!(guide.defer.contains(&format!("!{BRIDGE_PREFIX}{name}")), "{name}");
        }
        // the rare ones stay behind the search
        assert!(!guide.defer.iter().any(|d| d.ends_with("propose_room") || d.ends_with("propose_profile")));
        // another agent with the same tools doesn't get the cards in view
        assert!(!diet("daily", &tools, false, false, false).defer.iter().any(|d| d.ends_with("propose_agent")));
    }

    #[test]
    fn the_visible_browser_in_view_for_an_agent_that_has_it() {
        // 2026-10-27: « open this page in your browser », no browser tool in view, a call ending on calendar_add
        let tools = names(&["calendar_add", "calendar_list"]);
        let with = diet("daily", &tools, true, false, false);
        for name in BROWSER_DIRECT {
            assert!(with.defer.contains(&format!("!mcp__stealth__{name}")), "{name}");
        }
        let without = diet("daily", &tools, false, false, false);
        assert!(!without.defer.iter().any(|n| n.contains("stealth")));
    }

    #[test]
    fn mail_read_comes_with_mail_search() {
        // 2026-10-29 (real-day bench, GLM-5.3 Flash): mail_read behind the search, mail_search called again and again
        let mail = names(&["notebook_search", "mail_search", "mail_read", "mail_draft"]);
        let d = diet("daily", &mail, false, false, false);
        assert!(d.defer.contains(&"!mcp__bipagents__mail_search".to_string()));
        assert!(d.defer.contains(&"!mcp__bipagents__mail_read".to_string()));
        assert!(!d.defer.contains(&"!mcp__bipagents__mail_draft".to_string()), "the others stay behind the search");
        // no mail_search in view (a kind that doesn't use it): mail_read stays behind the search too
        let d = diet("wellness", &mail, false, false, false);
        assert!(!d.defer.iter().any(|n| n.contains("mail_")));
        // without the mail tools nothing comes
        let d = diet("daily", &names(&["mail_search"]), false, false, false);
        assert!(!d.defer.contains(&"!mcp__bipagents__mail_read".to_string()));
        // the work kind had it already: once
        let d = diet("work", &mail, false, false, false);
        assert_eq!(d.defer.iter().filter(|n| n.ends_with("mail_read")).count(), 1);
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
        // a card tool's arguments under another card tool's name: left as called, it fails on its own arguments
        let cards = vec![
            tool("propose_task", &["title", "brief", "budget"], &["title", "brief"]),
            tool("propose_agent", &["kind", "name", "role", "handover"], &["kind", "name", "role"]),
            tool("notebook_read", &["page"], &["page"]),
        ];
        assert_eq!(meant(&cards, "propose_task", &args(&["kind", "name", "role"])), "propose_task");
        assert_eq!(meant(&cards, "notebook_read", &args(&["kind", "name", "role"])), "notebook_read");
    }

    #[test]
    fn at_most_eight_bridge_tools_direct() {
        let all: Vec<String> = kind_diet("daily").direct.iter().map(|s| s.to_string()).collect();
        let d = diet("daily", &all, false, false, false);
        let expected = if DIRECT_BRIDGE_TOOLS { MAX_DIRECT } else { 0 };
        assert_eq!(d.defer.iter().filter(|n| n.starts_with("!mcp__")).count(), expected);
    }

    #[test]
    fn written_once_and_a_persons_own_list_kept() {
        let d = diet("wellness", &names(&["health"]), false, false, false);
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
        // routines report to the conversation they were made in, unless the profile says otherwise
        let reports = |data: &Yaml| data.get("cron").and_then(|c| c.get("session_reports")).cloned();
        assert_eq!(reports(&changed), Some(Yaml::Bool(true)));
        let off = pyyaml::load("cron:\n  session_reports: false\n").unwrap();
        assert_eq!(reports(&with_diet(&off, &d).unwrap()), Some(Yaml::Bool(false)), "its own choice kept");
        // the person's own list
        let theirs = pyyaml::load("tools:\n  tool_search:\n    defer: [todo_list]\n").unwrap();
        let kept = with_diet(&theirs, &d).expect("skills still written");
        let defer = kept.get("tools").and_then(|t| t.get("tool_search")).and_then(|t| t.get("defer")).unwrap();
        assert_eq!(defer, &Yaml::Seq(vec![Yaml::str("todo_list")]));
    }

    fn api_list(data: &Yaml) -> Option<Vec<String>> {
        match data.get("platform_toolsets").and_then(|p| p.get(API_PLATFORM)) {
            Some(Yaml::Seq(items)) => Some(items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()),
            _ => None,
        }
    }

    #[test]
    fn clarify_always_in_the_api_toolsets() {
        let d = diet("daily", &[], false, false, false);
        // none saved (a new profile): the platform default plus clarify
        let fresh = with_diet(&pyyaml::load("model:\n  default: x\n").unwrap(), &d).unwrap();
        assert_eq!(api_list(&fresh), Some(names(&["hermes-api-server", "clarify"])));
        // a list `hermes tools enable/disable` saved (Hermes 0.22.5 leaves clarify out): kept, clarify added
        let saved = "platform_toolsets:\n  api_server: [file, memory, web]\n  cli: [web]\n";
        let changed = with_diet(&pyyaml::load(saved).unwrap(), &d).unwrap();
        assert_eq!(api_list(&changed), Some(names(&["file", "memory", "web", "clarify"])));
        assert_eq!(
            changed.get("platform_toolsets").and_then(|p| p.get("cli")),
            Some(&Yaml::Seq(vec![Yaml::str("web")])),
            "other platforms untouched"
        );
        // idempotent: the same again changes nothing, and a list that has it is left as it is
        assert!(with_diet(&changed, &d).is_none());
        let has = pyyaml::load("platform_toolsets:\n  api_server: [clarify, web]\n").unwrap();
        assert_eq!(api_list(&with_diet(&has, &d).unwrap()), Some(names(&["clarify", "web"])));
        // a value that is no list (Hermes falls back to the default for it): the default plus clarify
        let odd = pyyaml::load("platform_toolsets:\n  api_server: null\n").unwrap();
        assert_eq!(api_list(&with_diet(&odd, &d).unwrap()), Some(names(&["hermes-api-server", "clarify"])));
    }
}
