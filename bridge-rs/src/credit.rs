//! The plan's AI credit as the guichet tells this place (a server of the offer). Decided by Alex on 2026-10-26: no
//! wall when the month's credit is used.
//!
//!     ~/.local/state/bipagents/credit.json     {"state": "ok" | "low" | "full" | "stopped", "resumes": <Unix s>}
//!
//! The guichet answers each server beat with its places' states (its credit_pool.rs); the seat agent
//! (installer/cloud/seat-agent.py, root) writes this file for the place. No file (a home install, a guichet that
//! hasn't said yet) is `ok`, and so is a file past its `resumes` (the new month): a stale file never keeps anything
//! paused.
//!
//! `full`: the month's credit is used and the place's AI key runs on a grace margin; `stopped`: the grace is used too
//! (the router refuses the key; the apps offer credit packs). On both, the conversations are never blocked here, but
//! what runs by itself pauses:
//!
//! - routines: their runs skipped while it lasts (routines.rs); the first time, one short note in each agent's Boîte
//!   (never one per run);
//! - the agent's browser: its tools refused with a plain message ([`BROWSER_REFUSED`], browser_mcp.rs);
//! - background tasks: new ones refused ([`JOBS_REFUSED`]); a running task stops after its current step (jobs.rs),
//!   a mail scan after its current batch (mail_scan.rs), an archive's extraction after its current document
//!   (archives/records_fill.rs);
//! - watchers: their checks go on, but a change waits to wake its agent (watchers.rs).
//!
//! Back to `ok` or `low` (a pack bought, the new month): the routines run again, the tasks and mail scans paused for
//! the credit go on ([`run_forever`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::app::AppState;

/// The file, under the place's home.
pub const FILE: &str = ".local/state/bipagents/credit.json";
/// A task or a mail scan paused for the credit (its `reason` / `error`).
pub const REASON: &str = "credit";
/// How often the place looks at its credit.
pub const EVERY_SECONDS: u64 = 60;

/// The agent's browser tools while the credit is used.
pub const BROWSER_REFUSED: &str = "This month's AI credit is used up: the browser is paused until next month or \
until credit is added in the app (conversations go on).\nTell your person simply, in their language: « This month's \
credit is used up, so I can't open web pages for now; it comes back next month or as soon as you add credit in the \
app. »";
/// A background task asked while the credit is used.
pub const JOBS_REFUSED: &str = "this month's AI credit is used up: background tasks start again next month or as soon \
as credit is added in the app";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Ok,
    Low,
    Full,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Credit {
    pub state: State,
    /// when the new month's credit comes (Unix seconds)
    pub resumes: Option<f64>,
}

impl Credit {
    pub const OK: Credit = Credit { state: State::Ok, resumes: None };

    /// Whether what runs by itself pauses (routines, the browser, background tasks).
    pub fn paused(&self) -> bool {
        matches!(self.state, State::Full | State::Stopped)
    }
}

/// The place's home (`$HOME`).
pub fn home() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::py::expanduser("~")))
}

/// The file under a home.
pub fn file_of(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The credit the file says now (`ok` when there is none, it can't be read, or its new month has come).
pub fn read(path: &Path, now: f64) -> Credit {
    let Ok(text) = std::fs::read_to_string(path) else { return Credit::OK };
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&text) else { return Credit::OK };
    let resumes = data.get("resumes").and_then(Value::as_f64);
    if resumes.is_some_and(|r| now >= r) {
        return Credit::OK;
    }
    let state = match data.get("state").and_then(Value::as_str) {
        Some("low") => State::Low,
        Some("full") => State::Full,
        Some("stopped") => State::Stopped,
        _ => State::Ok,
    };
    Credit { state, resumes }
}

/// The credit now, from `path` or the place's own file.
pub fn now_at(path: Option<&Path>) -> Credit {
    let own;
    let path = match path {
        Some(p) => p,
        None => {
            own = file_of(&home());
            &own
        }
    };
    read(path, crate::py::time_time())
}

// -- routines -----------------------------------------------------------------------------------------------------


/// The note an agent's Boîte gets the first time its routines pause, in its language.
pub fn routines_note(lang: &str) -> &'static str {
    match lang {
        "en" => {
            "This month's AI credit is used up: your routines are paused. Our conversations go on. They start \
again next month, or as soon as you add credit in the app."
        }
        "de" => {
            "Das KI-Guthaben dieses Monats ist aufgebraucht: Deine Routinen pausieren. Unsere Gespräche gehen \
weiter. Sie laufen nächsten Monat wieder, oder sobald du in der App Guthaben hinzufügst."
        }
        "es" => {
            "El crédito de IA de este mes se ha agotado: tus rutinas están en pausa. Nuestras conversaciones \
siguen. Vuelven el mes que viene, o en cuanto añadas crédito en la app."
        }
        "nl" => {
            "Het AI-tegoed van deze maand is op: je routines staan op pauze. Onze gesprekken gaan door. Ze \
starten weer volgende maand, of zodra je tegoed toevoegt in de app."
        }
        _ => {
            "Le crédit d’IA du mois est utilisé : tes routines sont en pause. Nos discussions continuent. Elles \
reprennent le mois prochain, ou dès que tu ajoutes du crédit dans l’app."
        }
    }
}

/// One look at the credit. Used up: the routines skip their runs (routines.rs) and each agent that has some gets one
/// note in its Boîte, once per pause and month; back: the tasks and mail scans paused for the credit go on, when it
/// was paused at the last look (`was_paused`).
pub async fn check(state: &AppState, credit: Credit, was_paused: Option<bool>) {
    let config = state.config();
    if credit.paused() {
        for agent in config.agents.values() {
            if !crate::routines::list(&config, Some(&agent.name)).iter().any(|r| r.enabled) {
                continue;
            }
            // the same id is stored once (a restart, a second round: nothing again)
            let lang = crate::handback::language_of(&config, agent);
            let id = format!("credit-pause:{}:{}", agent.name, credit.resumes.unwrap_or(0.0) as i64);
            let note = crate::outbox::BoiteMessage { id: Some(id), ..crate::outbox::BoiteMessage::new(routines_note(&lang)) };
            if let Err(e) = state.outbox.ingest(agent, note, true).await {
                tracing::warn!(agent = %agent.name, error = %format!("{e:#}"), "credit note not stored");
            }
        }
        return;
    }
    if was_paused == Some(true) {
        crate::jobs::jobs_of(state).resume_after_credit(&config).await;
    }
}

/// Every minute, for ever: the credit's file read, what runs by itself paused or resumed.
pub async fn run_forever(state: AppState) {
    let mut was: Option<bool> = None;
    loop {
        let credit = now_at(None);
        check(&state, credit, was).await;
        was = Some(credit.paused());
        tokio::time::sleep(Duration::from_secs(EVERY_SECONDS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_says_the_state_until_its_new_month() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credit.json");
        assert_eq!(read(&path, 100.0), Credit::OK); // none: ok
        std::fs::write(&path, r#"{"state": "full", "resumes": 200}"#).unwrap();
        assert!(read(&path, 100.0).paused());
        assert_eq!(read(&path, 100.0).resumes, Some(200.0));
        assert_eq!(read(&path, 200.0), Credit::OK); // the new month came: a stale file pauses nothing
        std::fs::write(&path, r#"{"state": "stopped"}"#).unwrap();
        assert!(read(&path, 100.0).paused());
        std::fs::write(&path, r#"{"state": "low", "resumes": 200}"#).unwrap();
        assert!(!read(&path, 100.0).paused());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read(&path, 100.0), Credit::OK);
    }

    #[test]
    fn the_note_is_in_the_agent_s_language() {
        assert!(routines_note("fr").contains("routines sont en pause"));
        assert!(routines_note("nl").contains("pauze"));
        assert!(routines_note("xx").starts_with("Le crédit"));
    }
}
