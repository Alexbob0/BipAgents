//! The bridge's share of an agent's prompt, exactly as the bridge writes it (« token diet », 2026-10-24): every
//! SOUL.md a kind of agent starts with (with its language, notebook, archives and health blocks), the bridge's skills,
//! and the tool lists of its MCP servers (`bipagents`, `bipagents_pay`, `bipagents_vault`, `bipagents_files`), as
//! Hermes receives them. Written as JSON for scripts/token-anatomy.py to count. Ignored by default:
//!
//!     TOKEN_ANATOMY_OUT=/some/dir cargo test --test token_anatomy -- --ignored

mod common;

use bipbridge::{
    agent_files, agent_tools, archives, calendar, card_pay, contacts, family_news, health, notebook, sandbox,
    sandbox_sites, souls, vault_logins, weather,
};
use common::{config, Urls};
use serde_json::{json, Value};

#[test]
#[ignore]
fn dump_the_bridge_share_of_a_prompt() {
    let out = std::env::var("TOKEN_ANATOMY_OUT").expect("TOKEN_ANATOMY_OUT");
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());

    let mut souls_out = serde_json::Map::new();
    for lang in ["fr", "en"] {
        for category in souls::CATEGORIES {
            let base = souls::soul_for("Léa", category, Some(lang)).unwrap();
            // the Carnet on: in the person's circle, its memory of them common (shared_memory.rs)
            let with_notebook = souls::with_shared_memory(&souls::with_notebook(&base, true), Some(true));
            let with_all = souls::with_health(&souls::with_archives(&with_notebook, true), true);
            let with_all = souls::with_shared_memory(&with_all, Some(false)); // the health agent's variant
            souls_out.insert(
                format!("{lang}/{category}"),
                json!({"base": base, "notebook": with_notebook, "all": with_all}),
            );
        }
    }
    let blocks = json!({
        "language_block_en": souls::language_block(Some("en")),
        "CONTENT_RULE": souls::CONTENT_RULE, "AI_RULE": souls::AI_RULE, "CARE_RULE": souls::CARE_RULE,
        "FAMILY_RULE": souls::FAMILY_RULE, "DRAFT_RULE": souls::DRAFT_RULE, "PLAIN_RULE": souls::PLAIN_RULE,
        "FILES_RULE": souls::FILES_RULE, "LONG_WORK_RULE": souls::LONG_WORK_RULE, "VISUAL_RULE": souls::VISUAL_RULE,
        "NOTEBOOK_RULE": souls::NOTEBOOK_RULE, "ARCHIVES_RULE": souls::ARCHIVES_RULE, "HEALTH_RULE": souls::HEALTH_RULE,
        "SHARED_MEMORY_RULE": souls::SHARED_MEMORY_RULE, "SHARED_MEMORY_HEALTH_RULE": souls::SHARED_MEMORY_HEALTH_RULE,
        "prototype/language_block_compact_en": souls::language_block_compact(Some("en")),
        "prototype/COMPACT_NOTEBOOK_RULE": souls::COMPACT_NOTEBOOK_RULE,
    });

    let tools = json!({
        "bipagents/base": agent_tools::tools(),
        "bipagents/family": family_news::tools().to_vec(),
        "bipagents/notebook": notebook::tools::tools(false),
        "bipagents/notebook_health_agent": notebook::tools::tools(true),
        "bipagents/notebook_fill": [notebook::fill::tool()],
        "bipagents/weather": [weather::weather_tool()],
        "bipagents/health": [health::health_tool(&health::CATEGORIES)],
        "bipagents/calendar": calendar::tools(),
        "bipagents/contacts": contacts::tools(),
        "bipagents/agent_files": [agent_files::tool()],
        "bipagents/archives": archives::tools(&config, "wellness"),
        "bipagents_pay": card_pay::tools(),
        "bipagents_vault": vault_logins::tools(),
        "bipagents_files": sandbox::tools(),
        "bipagents_files/sites": sandbox_sites::tools(),
    });

    let all = json!({"souls": souls_out, "blocks": blocks, "tools": tools});
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(format!("{out}/bridge-share.json"), serde_json::to_string_pretty(&all).unwrap()).unwrap();
    let _: &Value = &all;
}
