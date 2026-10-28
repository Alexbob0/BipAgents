//! BipAgents bridge: TTS by sentence, voice WebSocket, file drop, APNs push and ntfy outbox
//! for Hermes agents (see ../README.md).
//!
//! Rust port of `bridge/bipbridge` (Python): the same routes, JSON, files on disk, config and CLI. One Rust module
//! per Python module, same name.

pub const VERSION: &str = "0.1.0";

/// A function not ported yet that the core calls on its normal path (startup, the auth middleware): it does
/// nothing instead of panicking, so the core runs and its tests pass. Its owner replaces the call with the port.
#[macro_export]
macro_rules! not_ported {
    ($what:expr) => {
        tracing::trace!(what = $what, "not ported yet")
    };
}

/// A dict as Python's `Dict[str, Any]` (insertion order kept).
pub type Dict = serde_json::Map<String, serde_json::Value>;

// -- core (plumbing that is not a Python module) ---------------------------------------------------
pub mod assets;
pub mod py;
pub mod web;

// -- core ------------------------------------------------------------------------------------------
pub mod app;
pub mod auth;
pub mod config;
pub mod guard;
pub mod hermes;
pub mod lan;
pub mod logs;
pub mod loop_brake;
pub mod main_cli;
pub mod storage_watch;
pub mod feature_tips;
pub mod store;

// -- push-runs -------------------------------------------------------------------------------------
pub mod apns;
pub mod approval_text;
pub mod approvals_log;
pub mod made_files;
pub mod cited_files;
pub mod credit;
pub mod cronwatch;
pub mod no_reply;
pub mod fcm;
pub mod media;
pub mod ntfy;
pub mod live_activities;
pub mod push;
pub mod relay;
pub mod runs;
pub mod sleep;

// -- voice-speech ----------------------------------------------------------------------------------
pub mod durations;
pub mod live_brief;
pub mod numbers_fr;
pub mod pronounce;
pub mod pronunciations;
pub mod speech_intl;
pub mod textproc;
pub mod tts;
/// Private helpers of the voice-speech modules (Python's `re`/`str` semantics, num2words).
pub mod voice_speech;
pub mod voice_ws;
pub mod voices;

// -- agents ----------------------------------------------------------------------------------------
pub mod agent_groups;
pub mod agent_shares;
pub mod read_shares;
pub mod templates;
pub mod templates_scrub;
pub mod agents_admin;
/// The person's time zone and standard dates (no Python counterpart).
pub mod dates;
pub mod device_keys;
pub mod door_tls;
pub mod family_agent;
/// « L'agent de projet »: an agent for one occasion whose thread friends follow (no Python counterpart).
pub mod project_agent;
pub mod family_news;
pub mod body_limit;
pub mod files;
pub mod files_kept;
pub mod households;
pub mod own_skills;
pub mod peers;
pub mod beneath;
pub mod sandbox;
pub mod sandbox_adopt;
pub mod sandbox_sites;
pub mod send_guard;
pub mod sent_files;
pub mod services;
pub mod shares;
pub mod souls;
pub mod space;
pub mod spaces;
pub mod ssh_keys;
/// What each kind of agent keeps in view of Hermes' tools (token diet, phase 2).
pub mod tool_diet;
/// The providers EUrouter tries first, for its prompt cache.
pub mod model_route;

// -- tools-knowledge -------------------------------------------------------------------------------
pub mod agent_email;
pub mod agent_email_smtp;
pub mod agent_files;
pub mod agent_tools;
pub mod archives;
pub mod whatsapp_import;
pub mod assistant_import;
pub mod favicons;
pub mod guide;
pub mod handback;
pub mod health;
pub mod jobs;
pub mod mail_scan;
pub mod mailbox;
pub mod map_tiles;
pub mod maps;
pub mod memory_edit;
pub mod memory_language;
pub mod message_search;
pub mod notebook;
pub mod notes_import;
pub mod pages;
pub mod phone_asks;
pub mod reminders;
pub mod rooms;
pub mod rooms_remote;
pub mod routes;
#[doc(hidden)]
pub mod tools_knowledge; // the group's private helpers (pub for its golden tests)
pub mod weather;

// -- browsers-places -------------------------------------------------------------------------------
pub mod browser_mcp;
pub mod browsers;
pub mod caldav;
pub mod connections;
pub mod calendar;
pub mod card_fill;
pub mod card_pay;
pub mod handover;
pub mod link_wallet;
pub mod cards;
pub mod cloud_setup;
pub mod console;
pub mod contacts;
pub mod desktop;
pub mod desktop_page;
pub mod desktop_read;
pub mod login_fill;
pub mod place_backup;
pub mod place_move;
pub mod place_import;
pub mod vault;
pub mod vault_logins;
// connectors an agent makes on request (2026-10-28): their secrets, their store and approval, their runs
pub mod connectors;
pub mod connectors_run;
pub mod secrets;

pub use app::AppState;
