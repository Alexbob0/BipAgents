//! Token diet, phase 2 (2026-10-26, « ranger, pas retirer »): what each agent's Hermes shows the model on every call,
//! by kind of agent. Nothing is taken away: a tool the kind rarely needs goes behind Hermes' tool search
//! (`tool_search` → `tool_describe` → `tool_call`) instead of costing its whole schema on each call, and the few
//! bridge tools the kind uses all the time come out of it, so the agent stops spending a discovery round-trip (a full
//! call each) before using them. Measured on 2026-10-24 (scripts/token-lab): ≈ 19.3k input tokens per call, 11.8k of
//! them the schemas of 22 Hermes core tools, and 40 of 126 tool calls were discovery.
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
pub const MAX_DIRECT: usize = 8;

/// The bridge's tools as Hermes names them.
pub const BRIDGE_PREFIX: &str = "mcp__bipagents__";

/// The visible browser's MCP entry (browsers.rs).
pub const BROWSER_SERVER: &str = "stealth";

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
                "mail_search",
                "start_task",
                "calendar_add",
                "contacts_search",
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
            direct: &["health", "notebook_search", "notebook_read", "reminders_add", "weather", "start_task"],
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
                "archive_search",
                "notebook_read",
                "create_page",
                "start_task",
                "mail_search",
                "mail_read",
            ],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "creative", "media"],
        },
        "home" => KindDiet {
            deferred: NO_FILE_WRITES,
            direct: &["notebook_search", "reminders_add", "reminders_list", "calendar_list", "weather", "start_task"],
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
                "mail_search",
                "mail_read",
                "mail_draft",
                "notebook_search",
                "calendar_add",
                "start_task",
                "contacts_search",
            ],
            compact_skills: &["autonomous-ai-agents", "social-media", "creative", "media"],
        },
        "learning" => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &[
                "notebook_search",
                "create_podcast",
                "create_audio",
                "knowledge_search",
                "list_voices",
                "start_task",
            ],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "email"],
        },
        "creative" => KindDiet {
            deferred: CODE_AND_EDITS,
            direct: &["create_page", "create_audio", "create_podcast", "list_voices", "notebook_search", "start_task"],
            compact_skills: &["autonomous-ai-agents", "software-development", "social-media", "email"],
        },
        "tech" => KindDiet {
            deferred: &[],
            direct: &["notebook_search", "start_task", "archive_search", "knowledge_search", "read_page"],
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
            direct: &["notebook_search", "start_task", "reminders_add", "calendar_list", "weather"],
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

/// The diet of an agent of `kind` that has `bridge_tools`, holds a vault item or not, has written skills or not.
pub fn diet(kind: &str, bridge_tools: &[String], holds_vault_item: bool, writes_skills: bool) -> Diet {
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
    plan.direct
        .iter()
        .filter(|name| bridge_tools.iter().any(|t| t == *name))
        .take(MAX_DIRECT)
        .for_each(|name| add(&format!("!{BRIDGE_PREFIX}{name}")));
    Diet { kind: kind.to_string(), defer, compact_skills: plan.compact_skills.iter().map(|c| c.to_string()).collect() }
}

/// Whether `agent` holds an item of the vault Hermes fills from (a card is paid through the bridge instead).
pub fn holds_vault_item(config: &Config, agent: &AgentConfig) -> bool {
    let vault = crate::vault::Vault::new(&crate::py::join(&config.data_dir(), "vault"));
    vault.items().iter().any(|item| item.kind != crate::cards::KIND && item.agents.contains_key(&agent.name))
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
    Some(diet(&kind, &tools, holds_vault_item(config, agent), writes_skills))
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
        let d = diet("daily", &tools, false, false);
        assert_eq!(d.defer[0], MARK);
        for name in HERMES_DEFAULT_DEFERRED.iter().chain(ALWAYS_DEFERRED.iter()).chain(VAULT_TOOLS.iter()) {
            assert!(d.defer.contains(&name.to_string()), "{name}");
        }
        assert!(d.defer.contains(&"skill_manage".to_string()));
        assert!(d.defer.contains(&"execute_code".to_string()));
        assert!(!d.defer.contains(&"terminal".to_string()));
        assert!(!d.defer.contains(&"memory".to_string()));
        assert!(d.defer.contains(&"!mcp__bipagents__notebook_search".to_string()));
        assert!(d.defer.contains(&"!mcp__bipagents__weather".to_string()));
        // a tool it doesn't have, or one its kind doesn't use much, stays behind the search
        assert!(!d.defer.iter().any(|n| n.ends_with("reminders_add") || n.ends_with("create_podcast")));
        // an agent holding a vault item, or writing skills, keeps those tools
        let d = diet("daily", &tools, true, true);
        assert!(!d.defer.iter().any(|n| n.starts_with("browser_vault") || n == "skill_manage"));
    }

    #[test]
    fn at_most_eight_bridge_tools_direct() {
        let all: Vec<String> = kind_diet("daily").direct.iter().map(|s| s.to_string()).collect();
        let d = diet("daily", &all, false, false);
        assert_eq!(d.defer.iter().filter(|n| n.starts_with("!mcp__")).count(), MAX_DIRECT);
    }

    #[test]
    fn written_once_and_a_persons_own_list_kept() {
        let d = diet("wellness", &names(&["health"]), false, false);
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
        // the person's own list
        let theirs = pyyaml::load("tools:\n  tool_search:\n    defer: [todo_list]\n").unwrap();
        let kept = with_diet(&theirs, &d).expect("skills still written");
        let defer = kept.get("tools").and_then(|t| t.get("tool_search")).and_then(|t| t.get("defer")).unwrap();
        assert_eq!(defer, &Yaml::Seq(vec![Yaml::str("todo_list")]));
    }
}
