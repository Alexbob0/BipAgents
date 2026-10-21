//! BipAgents bridge: TTS by sentence, voice WebSocket, file drop, APNs push and ntfy outbox
//! for Hermes agents (see ../README.md).
//!
//! Rust port of `bridge/bipbridge` (Python): the same routes, JSON, files on disk, config and CLI. One Rust module
//! per Python module, same name; see PORTING.md for the layout, the owners and the conventions.

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
pub mod main_cli;
pub mod store;

// -- push-runs -------------------------------------------------------------------------------------
pub mod apns;
pub mod cronwatch;
pub mod fcm;
pub mod media;
pub mod ntfy;
pub mod push;
pub mod relay;
pub mod runs;
pub mod sleep;

// -- voice-speech ----------------------------------------------------------------------------------
pub mod live_brief;
pub mod numbers_fr;
pub mod pronunciations;
pub mod speech_intl;
pub mod textproc;
pub mod tts;
/// Private helpers of the voice-speech modules (Python's `re`/`str` semantics, num2words).
pub mod voice_speech;
pub mod voice_ws;
pub mod voices;

// -- agents ----------------------------------------------------------------------------------------
pub mod agents_admin;
pub mod device_keys;
pub mod family_news;
pub mod files;
pub mod households;
pub mod own_skills;
pub mod peers;
pub mod services;
pub mod shares;
pub mod souls;
pub mod spaces;
pub mod ssh_keys;

// -- tools-knowledge -------------------------------------------------------------------------------
pub mod agent_tools;
pub mod favicons;
pub mod jobs;
pub mod mail_scan;
pub mod mailbox;
pub mod memory_edit;
pub mod notes_import;
pub mod pages;
pub mod reminders;
#[doc(hidden)]
pub mod tools_knowledge; // the group's private helpers (pub for its golden tests)

// -- browsers-places -------------------------------------------------------------------------------
pub mod browser_mcp;
pub mod browsers;
pub mod cloud_setup;
pub mod console;
pub mod desktop;
pub mod place_backup;
pub mod place_import;
pub mod vault;

pub use app::AppState;
