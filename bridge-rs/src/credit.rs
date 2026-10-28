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
//! - routines: Hermes' cron jobs of every agent paused in their `cron/jobs.json` (under Hermes' own `.jobs.lock`),
//!   marked `paused_reason` = [`PAUSE_REASON`] so that only those are resumed; the first time, one short note in each
//!   agent's Boîte (never one per run);
//! - the agent's browser: its tools refused with a plain message ([`BROWSER_REFUSED`], browser_mcp.rs);
//! - background tasks: new ones refused ([`JOBS_REFUSED`]); a running task stops after its current step (jobs.rs),
//!   a mail scan after its current batch (mail_scan.rs), an archive's extraction after its current document
//!   (archives/records_fill.rs);
//! - watchers: their checks go on, but a change waits to wake its agent (watchers.rs).
//!
//! Back to `ok` or `low` (a pack bought, the new month): the routines paused here resume, the tasks and mail scans
//! paused for the credit go on ([`run_forever`]).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::Config;

/// The file, under the place's home.
pub const FILE: &str = ".local/state/bipagents/credit.json";
/// What marks the routines paused here (Hermes' `paused_reason`).
pub const PAUSE_REASON: &str = "bipagents-credit";
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

/// Every `cron/jobs.json` of the place: each agent's profile's (with the agent's name), and Hermes' default home's.
pub fn routine_files(config: &Config) -> Vec<(Option<String>, PathBuf)> {
    let mut files: Vec<(Option<String>, PathBuf)> = config
        .agents
        .values()
        .map(|a| (Some(a.name.clone()), crate::agents_admin::profile_folder(config, a).join("cron").join("jobs.json")))
        .collect();
    if let Some(agent) = config.agents.values().next() {
        let root = PathBuf::from(crate::agents_admin::home_root(config, agent)).join("cron").join("jobs.json");
        if !files.iter().any(|(_, f)| *f == root) {
            files.push((None, root));
        }
    }
    files
}

/// Hermes' cross-process lock on a cron folder (`<cron dir>/.jobs.lock`, its `_jobs_lock`), held while it lives.
pub(crate) struct JobsLock(std::fs::File);

impl JobsLock {
    pub(crate) fn take(folder: &Path) -> std::io::Result<JobsLock> {
        use std::os::fd::AsRawFd;
        let file = std::fs::OpenOptions::new().create(true).append(true).open(folder.join(".jobs.lock"))?;
        for _ in 0..50 {
            // SAFETY: flock on a descriptor this File owns.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(JobsLock(file));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "Hermes holds its cron lock"))
    }
}

impl Drop for JobsLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: as above.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn jobs_of(data: &mut Value) -> Option<&mut Vec<Value>> {
    match data {
        Value::Array(items) => Some(items),
        Value::Object(map) => map.get_mut("jobs").and_then(Value::as_array_mut),
        _ => None,
    }
}

/// Changes a jobs file with `change` (each job, true when changed), under Hermes' lock, written whole in place of the
/// old one (its mode kept). How many jobs changed.
fn change_jobs(file: &Path, change: impl Fn(&mut Map<String, Value>) -> bool) -> std::io::Result<usize> {
    let Some(folder) = file.parent().filter(|f| f.is_dir()) else { return Ok(0) };
    if !file.is_file() {
        return Ok(0);
    }
    let _lock = JobsLock::take(folder)?;
    let text = std::fs::read_to_string(file)?;
    let Ok(mut data) = serde_json::from_str::<Value>(&text) else { return Ok(0) };
    let mut changed = 0;
    if let Some(jobs) = jobs_of(&mut data) {
        for job in jobs.iter_mut() {
            if let Value::Object(job) = job {
                if change(job) {
                    changed += 1;
                }
            }
        }
    }
    if changed > 0 {
        let mode = std::fs::metadata(file)?.permissions().mode();
        let tmp = folder.join(format!(".jobs.json.bipagents-{}", std::process::id()));
        {
            let mut out = std::fs::File::create(&tmp)?;
            out.write_all(serde_json::to_string_pretty(&data).unwrap_or_default().as_bytes())?;
            out.sync_all()?;
        }
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o7777))?;
        std::fs::rename(&tmp, file)?;
    }
    Ok(changed)
}

fn text_of(job: &Map<String, Value>, key: &str) -> String {
    job.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// Pauses the routines that would run (enabled, not paused, not over), as Hermes' `pause_job` does, marked as the
/// credit's. How many were paused.
pub fn pause_routines(file: &Path, now_iso: &str) -> std::io::Result<usize> {
    change_jobs(file, |job| {
        let enabled = job.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let state = text_of(job, "state");
        let paused = state == "paused" || job.get("paused_at").is_some_and(|v| !v.is_null() && v != "");
        if !enabled || paused || matches!(state.as_str(), "completed" | "error" | "deleted") {
            return false;
        }
        job.insert("enabled".into(), json!(false));
        job.insert("state".into(), json!("paused"));
        job.insert("paused_at".into(), json!(now_iso));
        job.insert("paused_reason".into(), json!(PAUSE_REASON));
        true
    })
}

/// Resumes the routines paused for the credit (only those), as Hermes' `resume_job` does: a run missed meanwhile stays
/// due, Hermes' catch-up rule decides (one run, or a logged skip). How many were resumed.
pub fn resume_routines(file: &Path) -> std::io::Result<usize> {
    change_jobs(file, |job| {
        if text_of(job, "paused_reason") != PAUSE_REASON {
            return false;
        }
        job.insert("enabled".into(), json!(true));
        job.insert("state".into(), json!("scheduled"));
        job.insert("paused_at".into(), Value::Null);
        job.insert("paused_reason".into(), Value::Null);
        true
    })
}

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

fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// One look at the credit: routines paused (a note the first time) or resumed; the tasks and mail scans paused for
/// the credit resumed when it was paused at the last look (`was_paused`).
pub async fn check(state: &AppState, credit: Credit, was_paused: Option<bool>) {
    let config = state.config();
    let files = routine_files(&config);
    if credit.paused() {
        let iso = iso_now();
        for (agent, file) in files {
            let paused = {
                let (file, iso) = (file.clone(), iso.clone());
                tokio::task::spawn_blocking(move || pause_routines(&file, &iso)).await
            };
            let paused = match paused {
                Ok(Ok(n)) => n,
                Ok(Err(e)) => {
                    tracing::warn!(file = %file.display(), error = %e, "routines not paused for the credit");
                    continue;
                }
                Err(_) => continue,
            };
            let Some(agent) = agent.and_then(|name| config.agent(Some(&name)).cloned()) else { continue };
            if paused == 0 {
                continue;
            }
            tracing::info!(agent = %agent.name, routines = paused, "routines paused: the month's AI credit is used");
            // once per pause and month: the same id is stored once (a restart, a second round: nothing again)
            let lang = crate::handback::language_of(&config, &agent);
            let mut note = Map::new();
            note.insert("message".into(), json!(routines_note(&lang)));
            note.insert(
                "id".into(),
                json!(format!("credit-pause:{}:{}", agent.name, credit.resumes.unwrap_or(0.0) as i64)),
            );
            if let Err(e) = state.outbox.ingest(&agent, &note, true, false).await {
                tracing::warn!(agent = %agent.name, error = %format!("{e:#}"), "credit note not stored");
            }
        }
        return;
    }
    for (_, file) in files {
        let shown = file.display().to_string();
        match tokio::task::spawn_blocking(move || resume_routines(&file)).await {
            Ok(Ok(n)) if n > 0 => tracing::info!(file = %shown, routines = n, "routines resumed: AI credit back"),
            Ok(Err(e)) => tracing::warn!(file = %shown, error = %e, "routines not resumed"),
            _ => {}
        }
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
    fn routines_pause_and_only_the_credit_s_resume() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("jobs.json");
        let jobs = json!({"jobs": [
            {"id": "a", "name": "Météo", "enabled": true, "state": "scheduled",
             "next_run_at": "2026-10-27T07:00:00+01:00"},
            {"id": "b", "name": "Mine", "enabled": false, "state": "paused", "paused_at": "x", "paused_reason": "user"},
            {"id": "c", "name": "Once", "enabled": true, "state": "completed"},
        ], "updated_at": "kept"});
        std::fs::write(&file, jobs.to_string()).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(pause_routines(&file, "2026-10-26T17:00:00Z").unwrap(), 1);
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let a = &data["jobs"][0];
        assert_eq!((a["enabled"].clone(), a["state"].clone()), (json!(false), json!("paused")));
        assert_eq!(a["paused_reason"], json!(PAUSE_REASON));
        assert_eq!(a["paused_at"], json!("2026-10-26T17:00:00Z"));
        assert_eq!(data["jobs"][2]["state"], "completed");
        assert_eq!(data["updated_at"], "kept");
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        // every minute while the credit is used: nothing more to pause
        assert_eq!(pause_routines(&file, "later").unwrap(), 0);
        // the credit back: only the credit's resume, their next run kept (Hermes' catch-up rule decides)
        assert_eq!(resume_routines(&file).unwrap(), 1);
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        let a = &data["jobs"][0];
        assert_eq!((a["enabled"].clone(), a["state"].clone()), (json!(true), json!("scheduled")));
        assert_eq!(a["paused_reason"], Value::Null);
        assert_eq!(a["next_run_at"], json!("2026-10-27T07:00:00+01:00"));
        assert_eq!(data["jobs"][1]["paused_reason"], "user"); // the person's own pause stays
        assert_eq!(resume_routines(&file).unwrap(), 0);
        // a list without its wrapper, and no file at all
        std::fs::write(&file, json!([{"id": "d", "schedule": {"kind": "cron"}}]).to_string()).unwrap();
        assert_eq!(pause_routines(&file, "now").unwrap(), 1);
        assert_eq!(pause_routines(&dir.path().join("none").join("jobs.json"), "now").unwrap(), 0);
    }

    #[test]
    fn the_note_is_in_the_agent_s_language() {
        assert!(routines_note("fr").contains("routines sont en pause"));
        assert!(routines_note("nl").contains("pauze"));
        assert!(routines_note("xx").starts_with("Le crédit"));
    }
}
