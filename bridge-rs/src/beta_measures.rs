//! The beta's measures (internal/research/first-run-audit-2026-10-29.md, section 4): a few counters and times of this
//! place, without any content, that the server's seat agent joins to its heartbeat (installer/cloud/seat-agent.py,
//! `measures`) for the guichet to count totals per day, never per person.
//!
//!     guide_message_at   the person's first message to the guide (unix seconds)
//!     first_accept_at    the first guide card accepted (an agent or a team)
//!     message_hours      the hours (unix seconds, to the hour) the person wrote in, these 10 days at most
//!     team_proposed / team_accepted / team_refused / team_ticked
//!                        « Équipe proposée » cards: shown, accepted, said no to, and the agents made by those accepted
//!     routine_at         the first routine reply delivered (a morning point or any other routine)
//!     data_dir/measures.json   (0600)
//!
//! No message, no name, no agent: times rounded to the hour where they repeat, and counts. No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::py;

const HOUR: f64 = 3600.0;
/// the hours kept: day 7 after the claim is over by then
const HOURS_DAYS: f64 = 10.0;
const HOURS_MAX: usize = 240;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Measures {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_message_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_accept_at: Option<f64>,
    #[serde(default)]
    pub message_hours: Vec<i64>,
    #[serde(default)]
    pub team_proposed: u32,
    #[serde(default)]
    pub team_accepted: u32,
    #[serde(default)]
    pub team_refused: u32,
    #[serde(default)]
    pub team_ticked: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routine_at: Option<f64>,
}

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("measures.json")
}

pub fn load(config: &Config) -> Measures {
    std::fs::read(path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Changed under the lock and saved; a failure is never the caller's (a measure lost, nothing else).
fn change(config: &Config, edit: impl FnOnce(&mut Measures)) {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = load(config);
    let before = m.clone();
    edit(&mut m);
    if m == before {
        return;
    }
    let file = path(config);
    let saved = (|| -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temporary = file.with_extension("tmp");
        py::write_with_mode(
            &temporary.to_string_lossy(),
            &serde_json::to_vec(&m).map_err(std::io::Error::other)?,
            0o600,
        )?;
        std::fs::rename(&temporary, &file)
    })();
    if let Err(e) = saved {
        tracing::debug!(error = %e, "beta measures not saved");
    }
}

fn hour_of(now: f64) -> i64 {
    ((now / HOUR).floor() * HOUR) as i64
}

/// A message of the person (not a routine's run) to one of the agents; `to_guide` when it is the guide.
pub fn person_message(config: &Config, to_guide: bool, now: f64) {
    change(config, |m| {
        if to_guide && m.guide_message_at.is_none() {
            m.guide_message_at = Some(now);
        }
        let hour = hour_of(now);
        if !m.message_hours.contains(&hour) {
            m.message_hours.push(hour);
        }
        let cutoff = hour_of(now - HOURS_DAYS * 86400.0);
        m.message_hours.retain(|h| *h >= cutoff);
        m.message_hours.sort_unstable();
        let extra = m.message_hours.len().saturating_sub(HOURS_MAX);
        m.message_hours.drain(..extra);
    });
}

/// A guide card accepted (an agent, or a team with `ticked` agents made).
pub fn card_accepted(config: &Config, team: Option<usize>, now: f64) {
    change(config, |m| {
        m.first_accept_at.get_or_insert(now);
        if let Some(ticked) = team {
            m.team_accepted += 1;
            m.team_ticked += ticked.min(1000) as u32;
        }
    });
}

pub fn team_proposed(config: &Config) {
    change(config, |m| m.team_proposed += 1);
}

pub fn team_refused(config: &Config) {
    change(config, |m| m.team_refused += 1);
}

/// A routine's reply delivered to the person.
pub fn routine_delivered(config: &Config, now: f64) {
    change(config, |m| {
        m.routine_at.get_or_insert(now);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        (dir, config)
    }

    #[test]
    fn counts_without_content() {
        let (_dir, config) = config();
        let t = 1_800_000_000.0;
        person_message(&config, false, t);
        person_message(&config, true, t + 60.0);
        person_message(&config, true, t + 2.0 * HOUR);
        card_accepted(&config, None, t + 100.0);
        card_accepted(&config, Some(3), t + 200.0);
        team_proposed(&config);
        team_proposed(&config);
        team_refused(&config);
        routine_delivered(&config, t + 9.0 * HOUR);
        routine_delivered(&config, t + 30.0 * HOUR);
        let m = load(&config);
        assert_eq!(m.guide_message_at, Some(t + 60.0));
        assert_eq!(m.first_accept_at, Some(t + 100.0));
        assert_eq!(m.message_hours, vec![hour_of(t), hour_of(t + 2.0 * HOUR)]);
        assert_eq!((m.team_proposed, m.team_accepted, m.team_refused, m.team_ticked), (2, 1, 1, 3));
        assert_eq!(m.routine_at, Some(t + 9.0 * HOUR));
        // the hours older than 10 days go
        person_message(&config, false, t + 11.0 * 86400.0);
        assert_eq!(load(&config).message_hours, vec![hour_of(t + 11.0 * 86400.0)]);
    }
}
