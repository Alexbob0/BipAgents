//! An agent's Hermes restarted when there is a reason, never systematically (Alex, 2026-10-28). Reasons:
//!
//! - **memory**: its gateway's unit above `hermes_mb`, or above `hermes_growth` times what it took 10 minutes after it
//!   started; its browser container's unit above `chrome_mb`. Acted on at night only: from the agent's quiet hour (the
//!   hour of `window_start..window_end` with the least activity over the last 14 days in the bridge's records, `hour`
//!   when set; a few minutes of jitter per agent) until `window_end`, at most once in 12 hours;
//! - **unhealthy**: its Hermes did not answer `unhealthy_pings` pings in a row (one every 5 minutes). Acted on soon;
//! - **requested**: other code called [`request_restart`] (e.g. tools changed and this Hermes did not list them again).
//!   Acted on soon;
//! - **the bridge's programs**: its Hermes runs a program of an older bridge (the browser proxy `bip-browser-mcp`,
//!   browsers::stale_proxies), checked each tick: after an update the gateway keeps the binary it started with
//!   (9/11, the release's browser fixes never reached people). Acted on soon, whatever `gap_minutes`.
//!
//! « Soon » reasons: at most once per `gap_minutes` (an hour) per agent, except a new agent's tools
//! ([`NEW_AGENT_TOOLS`]: agents_admin.rs `tools_loaded` restarts the hub's gateway itself as soon as it is calm, and
//! hands it here only when it stays busy). Without a reason: no restart.
//!
//! Only when the agent does nothing: no turn going on (conversations, rooms, Live calls: the run hub, and its Hermes'
//! own count, `GET /health/detailed`: a bridge that restarted doesn't follow the turns started before it), no Live
//! call in the last 15 minutes, no background task or archive extraction (jobs.rs), no mail scan (mail_scan.rs), no
//! watcher check (watchers.rs), no Hermes routine running or due within `routine_margin_minutes`. Busy: tried again in
//! 10 minutes (2 for a soon reason); a night's memory restart not done by `window_end` waits for the next night. A soon
//! reason held only by turns waits at most `turn_wait_seconds` (10 minutes), then the restart is done all the same (a
//! turn that long is stuck, or waits for an answer that may never come). 2026-11-06: right after a release, the bridge
//! restarted by `bipagents update` asked for « tools » restarts while a turn it no longer followed went on.
//!
//! Reasons asked for while one waits (tools, a new agent's tools, unhealthy…) make one restart, its record naming them
//! all (`; `).
//!
//! The restart is the bridge's own (`agents_admin::restart_gateway`, the path settings changes take; agents sharing the
//! hub's gateway restart together, so all of them must be idle), then the agent's browser container with
//! `systemctl --user try-restart` (only if it runs; its profile is a volume). Hermes keeps its sessions, memory and
//! routines on disk (HERMES_HOME); a restart loses no conversation.
//!
//! Off for an existing Hermes the person brought (`existing-hermes.json` with `preexisting` next to the bridge's
//! config): that is their machine, only the bridge's programs are followed there (its own tools).
//! `[agent_restart] enabled` overrides (false: none at all).
//!
//!     data_dir/agent_restarts.json      {agent: {at, reason}}: the last restart of each agent (diagnostics, /v1/status)

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use chrono::{Datelike, TimeZone, Timelike};
use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::py;

/// `[agent_restart]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// None: on, except for an existing Hermes the person brought.
    pub enabled: Option<bool>,
    /// The night's hour for memory restarts; None: the quietest hour of the window.
    pub hour: Option<u32>,
    pub window_start: u32,
    pub window_end: u32,
    /// A routine due within this many minutes (or running): not now.
    pub routine_margin_minutes: u32,
    /// Hermes' gateway above this (MB): restarted at night.
    pub hermes_mb: f64,
    /// … or above this many times its size 10 minutes after it started.
    pub hermes_growth: f64,
    /// The browser container above this (MB).
    pub chrome_mb: f64,
    /// Pings failed in a row before « unhealthy ».
    pub unhealthy_pings: u32,
    /// At most one soon restart (unhealthy, requested) per agent this often.
    pub gap_minutes: u32,
    pub tick_seconds: f64,
    /// Its Hermes pinged this often; its size measured this often.
    pub ping_seconds: f64,
    pub measure_seconds: f64,
    /// The per-agent jitter of the night's hour stays below this many minutes (0: none).
    pub jitter_minutes: u32,
    /// Busy: tried again after this many seconds (a soon reason: a fifth of it).
    pub retry_seconds: f64,
    /// A soon restart held only by turns going on waits at most this long, then is done all the same.
    pub turn_wait_seconds: f64,
    /// An agent whose tools changed and whose Hermes hasn't listed them again after this long: restarted once idle.
    pub stale_tools_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: None,
            hour: None,
            window_start: 1,
            window_end: 6,
            routine_margin_minutes: 30,
            hermes_mb: 900.0,
            hermes_growth: 2.5,
            chrome_mb: 1536.0,
            unhealthy_pings: 3,
            gap_minutes: 60,
            tick_seconds: 60.0,
            ping_seconds: 300.0,
            measure_seconds: 600.0,
            jitter_minutes: 20,
            retry_seconds: 600.0,
            turn_wait_seconds: TURN_WAIT_SECONDS,
            stale_tools_seconds: STALE_TOOLS_SECONDS,
        }
    }
}

/// `turn_wait_seconds` by default: 10 minutes.
pub const TURN_WAIT_SECONDS: f64 = 600.0;

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let d = Settings::default();
        let f = |key: &str, default: f64, min: f64, max: f64| -> f64 {
            table.get(key).and_then(Value::as_f64).map(|v| v.clamp(min, max)).unwrap_or(default)
        };
        let hour = |key: &str| table.get(key).and_then(Value::as_f64).map(|v| v.clamp(0.0, 23.0) as u32);
        let window_start = hour("window_start").unwrap_or(d.window_start);
        let window_end = table
            .get("window_end")
            .and_then(Value::as_f64)
            .map(|v| (v.clamp(1.0, 24.0) as u32).max(window_start + 1))
            .unwrap_or(d.window_end.max(window_start + 1));
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool),
            hour: hour("hour"),
            window_start,
            window_end,
            routine_margin_minutes: f("routine_margin_minutes", d.routine_margin_minutes as f64, 0.0, 720.0) as u32,
            hermes_mb: f("hermes_mb", d.hermes_mb, 50.0, 1_000_000.0),
            hermes_growth: f("hermes_growth", d.hermes_growth, 1.1, 100.0),
            chrome_mb: f("chrome_mb", d.chrome_mb, 50.0, 1_000_000.0),
            unhealthy_pings: f("unhealthy_pings", d.unhealthy_pings as f64, 1.0, 100.0) as u32,
            gap_minutes: f("gap_minutes", d.gap_minutes as f64, 0.0, 10_080.0) as u32,
            tick_seconds: f("tick_seconds", d.tick_seconds, 0.2, 3600.0),
            ping_seconds: f("ping_seconds", d.ping_seconds, 0.2, 86_400.0),
            measure_seconds: f("measure_seconds", d.measure_seconds, 0.2, 86_400.0),
            jitter_minutes: f("jitter_minutes", d.jitter_minutes as f64, 0.0, 59.0) as u32,
            retry_seconds: f("retry_seconds", d.retry_seconds, 0.2, 86_400.0),
            turn_wait_seconds: f("turn_wait_seconds", d.turn_wait_seconds, 0.0, 86_400.0),
            stale_tools_seconds: f("stale_tools_seconds", d.stale_tools_seconds, 0.2, 86_400.0),
        }
    }
}

/// An existing Hermes the person brought (installer/bipinstall/existing.py writes the record next to the config).
pub fn existing_hermes(config: &Config) -> bool {
    let Some(source) = config.source_path.as_deref() else { return false };
    let record = Path::new(source).parent().map(|p| p.join("existing-hermes.json"));
    record
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|v| v.get("preexisting").and_then(Value::as_bool).unwrap_or(false))
}

pub fn enabled(config: &Config) -> bool {
    config.agent_restart.enabled.unwrap_or_else(|| !existing_hermes(config))
}

// --- requests from other code ----------------------------------------------------------------------------------------

/// Each agent's reasons asked for, in order, each once: one restart does them all.
static REQUESTS: LazyLock<Mutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);
static LIVE: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
/// A Live call this recent: the agent is not idle.
pub const LIVE_SECONDS: f64 = 900.0;

/// A new agent on the hub's gateway whose Hermes did not load the bridge's tools: never held back by `gap_minutes`
/// (2026-10-30 bench: two agents made within the hour, the second never restarted, its first turns without tools).
pub const NEW_AGENT_TOOLS: &str = "new agent without its tools";

/// Its Hermes runs a program of an older bridge (the browser proxy, browsers::stale_proxies): the bridge was updated,
/// the gateway kept the binary it started with (9/11). Never held back by `gap_minutes` either (the bridge's own
/// « tools » restart right after an update must not keep the old proxy for an hour), and done for an existing Hermes
/// the person brought too: these are the bridge's own tools.
pub const BRIDGE_PROGRAMS: &str = "the bridge's programs changed";

/// The bridge's state, for [`group_calm`] (set when the loop starts; None in tests and before).
static APP: std::sync::OnceLock<AppState> = std::sync::OnceLock::new();

/// Whether the restart loop runs (false in tests and before it starts): [`group_calm`] can then answer.
pub fn loop_running() -> bool {
    APP.get().is_some()
}

/// Asks for the agent's Hermes to be restarted soon, once it is idle (at most once per `gap_minutes`). A reason asked
/// for again while it waits is not asked twice; another one joins it (one restart).
pub fn request_restart(agent: &str, reason: &str) {
    let mut all = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    let reasons = all.entry(agent.to_string()).or_default();
    if !reasons.iter().any(|r| r == reason) {
        tracing::info!(target: "bipbridge::restart", agent, reason, "restart requested");
        reasons.push(reason.to_string());
    }
}

/// The agent's reasons waiting (none: empty).
fn pending(agent: &str) -> Vec<String> {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).get(agent).cloned().unwrap_or_default()
}

/// Several agents' reasons, each once, in order (the agents sharing one gateway: one restart).
pub fn merged<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for reason in lists.into_iter().flatten() {
        if !out.contains(reason) {
            out.push(reason.clone());
        }
    }
    out
}

#[cfg(test)]
fn requested(agent: &str) -> Option<String> {
    let reasons = pending(agent);
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

fn clear_request(agent: &str) {
    REQUESTS.lock().unwrap_or_else(|e| e.into_inner()).remove(agent);
}

/// The reasons a restart did, taken off (one asked for while it ran stays for the next).
fn clear_done(agent: &str, done: &[String]) {
    let mut all = REQUESTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(reasons) = all.get_mut(agent) {
        reasons.retain(|r| !done.contains(r));
        if reasons.is_empty() {
            all.remove(agent);
        }
    }
}

/// A Live call of the agent going on now (live_brief.rs).
pub fn note_live(agent: &str) {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string(), py::time_time());
}

fn live_recently(agent: &str, now: f64) -> bool {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).get(agent).is_some_and(|at| now - at < LIVE_SECONDS)
}

// --- the rules (pure) ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Memory(String),
    Unhealthy,
    Requested(String),
}

impl Reason {
    pub fn text(&self) -> String {
        match self {
            Reason::Memory(why) => format!("memory: {why}"),
            Reason::Unhealthy => "unhealthy: Hermes does not answer".into(),
            Reason::Requested(why) => format!("requested: {why}"),
        }
    }
    /// Acted on within minutes (else: at the night's quiet hour).
    pub fn soon(&self) -> bool {
        !matches!(self, Reason::Memory(_))
    }
}

/// The hour of `start..end` with the fewest activity marks (local hours); ties: the one closest to 04:00, then the
/// earliest. No marks at all: 04:00 when in the window.
pub fn quiet_hour(hours: &[u32], start: u32, end: u32) -> u32 {
    let end = end.max(start + 1).min(24);
    (start..end)
        .min_by_key(|h| (hours.iter().filter(|x| *x == h).count(), h.abs_diff(4), *h))
        .unwrap_or(4)
}

/// A few minutes per agent (0..`max`), the same every night: a shared server doesn't restart everyone at once.
pub fn jitter_minutes(agent: &str, max: u32) -> u32 {
    if max == 0 {
        return 0;
    }
    let mut hash: u32 = 2_166_136_261;
    for byte in agent.bytes() {
        hash = (hash ^ byte as u32).wrapping_mul(16_777_619);
    }
    hash % max
}

/// The night's slot of a memory restart: from the quiet hour (plus the jitter) until the window's end.
pub fn in_night_slot(minute_of_day: u32, hour: u32, jitter: u32, window_end: u32) -> bool {
    minute_of_day >= hour * 60 + jitter && minute_of_day < window_end * 60
}

/// What the agent is doing now (each a reason not to restart it).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Activity {
    pub turn: bool,
    /// Its Hermes says it works (a turn the hub doesn't follow: one from before the bridge restarted).
    pub hermes: bool,
    pub live: bool,
    pub job: bool,
    pub scan: bool,
    pub watcher: bool,
    pub routine: bool,
}

impl Activity {
    /// Why not now, or None when idle.
    pub fn busy(&self) -> Option<&'static str> {
        [
            (self.turn, "a turn is going on"),
            (self.hermes, "its Hermes runs a turn"),
            (self.live, "a Live call"),
            (self.job, "a background task runs"),
            (self.scan, "a mail scan runs"),
            (self.watcher, "a watcher checks"),
            (self.routine, "a routine runs or is due"),
        ]
        .into_iter()
        .find(|(on, _)| *on)
        .map(|(_, why)| why)
    }

    /// Busy with turns only (the ones a soon restart waits for at most `turn_wait_seconds`).
    pub fn only_turns(&self) -> bool {
        !(self.live || self.job || self.scan || self.watcher || self.routine)
    }
}

/// Hermes' own count of what its gateway does now (`GET /health/detailed`, v0.22.5): `gateway_busy`, `active_agents`
/// (turns, API runs, cron jobs), the API server's runs (`readiness.checks.background_queues.active_api_runs`). None:
/// none of them said (an older Hermes, or no answer).
pub fn hermes_busy_of(data: &Value) -> Option<bool> {
    let count = |v: Option<&Value>| v.and_then(|v| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()));
    let flag = data.get("gateway_busy").and_then(Value::as_bool);
    let active = count(data.get("active_agents"));
    let runs = count(data.pointer("/readiness/checks/background_queues/active_api_runs"));
    if flag.is_none() && active.is_none() && runs.is_none() {
        return None;
    }
    Some(flag == Some(true) || active.is_some_and(|n| n > 0.0) || runs.is_some_and(|n| n > 0.0))
}

/// Whether the agent's Hermes works now, by its own count (None: it didn't say within a few seconds).
pub async fn hermes_busy(state: &AppState, agent: &AgentConfig) -> Option<bool> {
    let answer = tokio::time::timeout(Duration::from_secs(5), state.hermes.get_value(agent, "/health/detailed")).await;
    match answer {
        Ok(Ok(data)) => hermes_busy_of(&data),
        _ => None,
    }
}

/// A routine (autonomous.rs `routine_item`) running, or due from 5 minutes ago to `margin_minutes` ahead.
pub fn routine_busy(items: &[Value], now: f64, margin_minutes: u32) -> bool {
    items.iter().any(|item| {
        item["state"] == "running"
            || item["next_run"]
                .as_f64()
                .is_some_and(|next| next >= now - 300.0 && next <= now + margin_minutes as f64 * 60.0)
    })
}

/// Sizes in MB: the gateway now and 10 minutes after it started, the browser container now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sizes {
    pub hermes: Option<f64>,
    pub hermes_start: Option<f64>,
    pub chrome: Option<f64>,
}

pub fn memory_reason(sizes: &Sizes, s: &Settings) -> Option<String> {
    if let Some(now) = sizes.hermes {
        if now > s.hermes_mb {
            return Some(format!("Hermes uses {now:.0} MB (over {:.0})", s.hermes_mb));
        }
        if let Some(start) = sizes.hermes_start.filter(|v| *v > 0.0) {
            if now > start * s.hermes_growth {
                return Some(format!("Hermes uses {now:.0} MB, {:.1}x its {start:.0} MB after start", now / start));
            }
        }
    }
    match sizes.chrome {
        Some(chrome) if chrome > s.chrome_mb => Some(format!("its browser uses {chrome:.0} MB (over {:.0})", s.chrome_mb)),
        _ => None,
    }
}

/// The reason to restart now, if any. `requested`/`failures`: soon reasons; `night` (in the night slot and no memory
/// restart in the last 12 hours) with `sizes`: the memory reason. `since_last`: seconds since its last restart.
pub fn reason(
    requested: Option<&str>,
    failures: u32,
    night: bool,
    sizes: &Sizes,
    since_last: Option<f64>,
    s: &Settings,
) -> Option<Reason> {
    let gap_ok = since_last.is_none_or(|t| t >= s.gap_minutes as f64 * 60.0);
    let new_agent = requested.is_some_and(|r| r.split("; ").any(|one| one == NEW_AGENT_TOOLS || one == BRIDGE_PROGRAMS));
    if gap_ok || new_agent {
        let unhealthy = failures >= s.unhealthy_pings;
        if let Some(why) = requested {
            // both: one restart, both named
            let why = if unhealthy { format!("{why}; {}", Reason::Unhealthy.text()) } else { why.to_string() };
            return Some(Reason::Requested(why));
        }
        if unhealthy {
            return Some(Reason::Unhealthy);
        }
    }
    if night && since_last.is_none_or(|t| t >= 12.0 * 3600.0) {
        return memory_reason(sizes, s).map(Reason::Memory);
    }
    None
}

// --- records ---------------------------------------------------------------------------------------------------------

fn records_path(config: &Config) -> String {
    py::join(&config.data_dir(), "agent_restarts.json")
}

pub fn records(config: &Config) -> Map<String, Value> {
    std::fs::read_to_string(records_path(config))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// The agent's last restart `{at, reason}`, if any.
pub fn last_restart(config: &Config, agent: &str) -> Option<Value> {
    records(config).get(agent).cloned()
}

/// A restart other code did (agents_admin.rs `tools_loaded`): recorded and the agents' requests done, as the loop's.
pub fn restarted(config: &Config, agents: &[String], reason: &str) {
    record(config, agents, &format!("requested: {reason}"), py::time_time());
    agents.iter().for_each(|a| clear_request(a));
}

fn record(config: &Config, agents: &[String], reason: &str, at: f64) {
    let mut all = records(config);
    for agent in agents {
        all.insert(agent.clone(), json!({"at": at, "reason": reason}));
    }
    if let Err(exc) = std::fs::write(records_path(config), format!("{}\n", Value::Object(all))) {
        tracing::warn!(target: "bipbridge::restart", error = %exc, "restart not recorded");
    }
}

/// The gateway's size taken this long after it started (or after the bridge first saw it running).
pub const BASELINE_SECONDS: f64 = 600.0;

// --- the loop ----------------------------------------------------------------------------------------------------------

/// What the loop remembers between ticks, per gateway (agents sharing it).
#[derive(Default)]
struct Group {
    failures: u32,
    last_ping: f64,
    last_measure: f64,
    sizes: Sizes,
    /// (unit start stamp, first seen at, size taken 10 minutes after).
    start: Option<(String, f64, Option<f64>)>,
    next_try: f64,
    quiet: Option<(u32, u32)>, // (day of year, hour)
    /// Since when a soon restart waits for turns only (`turn_wait_seconds`).
    turn_wait: Option<f64>,
}

/// The agents grouped by the gateway serving them: their space, or the hub's ("").
fn groups(config: &Config) -> IndexMap<String, Vec<AgentConfig>> {
    let mut out: IndexMap<String, Vec<AgentConfig>> = IndexMap::new();
    for agent in config.agents.values() {
        let key = agent.space.clone().filter(|s| !s.is_empty()).unwrap_or_default();
        out.entry(key).or_default().push(agent.clone());
    }
    out
}

fn units_dir() -> std::path::PathBuf {
    crate::place_backup::home().join(".config").join("systemd").join("user")
}

/// The systemd unit of the agent's gateway, when it has one.
fn gateway_unit(agent: &AgentConfig) -> Option<String> {
    let units = units_dir();
    let names: Vec<String> = match agent.space.as_deref().filter(|s| !s.is_empty()) {
        Some(_) => vec![crate::spaces::unit_name(&agent.name)],
        None => crate::sleep::UNITS.iter().map(|u| u.to_string()).collect(),
    };
    names.into_iter().find(|u| units.join(format!("{u}.service")).exists())
}

/// `systemctl --user show` of a unit: (MemoryCurrent in MB, its start stamp).
async fn unit_memory(unit: &str) -> Option<(f64, String)> {
    let out = crate::sleep::systemctl(vec![
        "show".into(),
        "-p".into(),
        "MemoryCurrent".into(),
        "-p".into(),
        "ActiveEnterTimestampMonotonic".into(),
        unit.into(),
    ])
    .await
    .ok()?;
    parse_show(&out)
}

pub fn parse_show(text: &str) -> Option<(f64, String)> {
    let mut memory = None;
    let mut start = String::new();
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("MemoryCurrent=") {
            memory = v.trim().parse::<f64>().ok().map(|b| b / 1_048_576.0);
        } else if let Some(v) = line.strip_prefix("ActiveEnterTimestampMonotonic=") {
            start = v.trim().to_string();
        }
    }
    memory.filter(|m| *m > 0.0 && *m < 1.0e9).map(|m| (m, start))
}

async fn measure(config: &Config, agents: &[AgentConfig], group: &mut Group, now: f64) {
    group.sizes = Sizes::default();
    if let Some((mb, stamp)) = match gateway_unit(&agents[0]) {
        Some(unit) => unit_memory(&unit).await,
        None => None,
    } {
        let start = match group.start.take() {
            Some((s, seen, size)) if s == stamp => (s, seen, size),
            _ => (stamp, now, None),
        };
        let size = start.2.or_else(|| (now - start.1 >= BASELINE_SECONDS).then_some(mb));
        group.sizes.hermes = Some(mb);
        group.sizes.hermes_start = size;
        group.start = Some((start.0, start.1, size));
    }
    if config.browser.enabled {
        for agent in agents {
            let unit = crate::browsers::container(&agent.name);
            if !units_dir().join(format!("{unit}.service")).exists() {
                continue;
            }
            if let Some((mb, _)) = unit_memory(&unit).await {
                group.sizes.chrome = Some(group.sizes.chrome.unwrap_or(0.0).max(mb));
            }
        }
    }
}

/// Local hours of the agent's activity over the last 14 days (replies kept in the outbox, runs watched).
async fn activity_hours(state: &AppState, agents: &[AgentConfig], zone: chrono_tz::Tz) -> Vec<u32> {
    let since = (chrono::Utc::now() - chrono::Duration::days(14)).format("%Y-%m-%dT%H:%M:%S").to_string();
    let mut hours = Vec::new();
    for agent in agents {
        for table in ["outbox", "watched_runs"] {
            let rows = state.store.activity_times(table, &agent.name, &since).await.unwrap_or_default();
            for row in rows {
                let at = py::py_str(&row["created_at"]);
                if let Ok(t) = chrono::DateTime::parse_from_rfc3339(&at)
                    .map(|t| t.with_timezone(&chrono::Utc))
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(at.get(..19).unwrap_or(""), "%Y-%m-%dT%H:%M:%S")
                            .map(|n| n.and_utc())
                    })
                {
                    hours.push(zone.from_utc_datetime(&t.naive_utc()).hour());
                }
            }
        }
    }
    hours
}

pub(crate) async fn activity(state: &AppState, config: &Config, agent: &AgentConfig, now: f64) -> Activity {
    let name = agent.name.as_str();
    let job = crate::jobs::tasks(config).iter().any(|j| {
        py::py_str(j.get("agent").unwrap_or(&Value::Null)) == name
            && ["running", "stopping"].contains(&py::py_str(j.get("state").unwrap_or(&Value::Null)).as_str())
    });
    let zone = crate::routines::zone(config);
    let items: Vec<Value> = crate::routines::list(config, Some(name))
        .iter()
        .filter_map(|r| crate::autonomous::routine_item(name, crate::routines::view(r, zone).as_object()?))
        .collect();
    let routine = routine_busy(&items, now, config.agent_restart.routine_margin_minutes);
    let turn = state.hub.agent_running(name);
    Activity {
        turn,
        // asked only when the hub follows none: the hub's own word is enough to wait
        hermes: !turn && hermes_busy(state, agent).await == Some(true),
        live: live_recently(name, now),
        job,
        scan: state.scans.get().is_some_and(|s| s.running(name)),
        watcher: crate::watchers::any_running(name),
        routine,
    }
}

async fn restart(config: &Config, agents: &[AgentConfig]) -> anyhow::Result<()> {
    let run = crate::agents_admin::default_run();
    crate::agents_admin::restart_gateway(config, &agents[0], &run).await?;
    if config.browser.enabled {
        for agent in agents {
            let unit = crate::browsers::container(&agent.name);
            if units_dir().join(format!("{unit}.service")).exists() {
                // only if it runs (an on-demand browser stopped stays stopped); its profile stays
                let _ = crate::sleep::systemctl(vec!["try-restart".into(), unit]).await;
            }
        }
    }
    Ok(())
}

/// The agents sharing `agent`'s gateway, when they are all idle (`activity`) and restarts are on here: Some(names).
/// None when one of them is busy, restarts are off (an existing Hermes the person brought), the place sleeps, or the
/// loop isn't running (tests).
pub async fn group_calm(config: &Config, agent: &AgentConfig) -> Option<Vec<String>> {
    let state = APP.get()?;
    if !enabled(config) || (state.sleep.enabled() && state.sleep.asleep()) {
        return None;
    }
    let key = agent.space.clone().filter(|s| !s.is_empty()).unwrap_or_default();
    let agents = groups(config).swap_remove(&key).unwrap_or_default();
    let now = py::time_time();
    for other in &agents {
        if activity(state, config, other, now).await.busy().is_some() {
            return None;
        }
    }
    Some(agents.iter().map(|a| a.name.clone()).collect())
}

/// The loop (app.rs): every `tick_seconds`, each gateway looked at.
pub async fn run_forever(state: AppState) {
    let _ = APP.set(state.clone());
    let mut memory: HashMap<String, Group> = HashMap::new();
    loop {
        let config = state.config();
        // restarts off by default (an existing Hermes the person brought): only its gateway following the bridge's
        // own programs; off by the setting: none at all
        let follow_only = !enabled(&config) && config.agent_restart.enabled.is_none();
        if (enabled(&config) || follow_only) && !(state.sleep.enabled() && state.sleep.asleep()) {
            tick(&state, &config, &mut memory, follow_only).await;
        }
        tokio::time::sleep(Duration::from_secs_f64(config.agent_restart.tick_seconds)).await;
    }
}

/// An agent whose tools changed and whose Hermes hasn't listed them again by then (it ignores `tools/list_changed`:
/// an older Hermes, or one whose stream broke when the bridge restarted) is restarted once idle (agent_tools.rs
/// `stale_agents`). `stale_tools_seconds` by default.
pub const STALE_TOOLS_SECONDS: f64 = 120.0;
/// The reason a tools change asks for.
pub const TOOLS: &str = "tools";

/// Whether to ask for a [`BRIDGE_PROGRAMS`] restart: not when the agent's last one, within `gap_minutes`, was for
/// that already (its gateway started the old program again: its profile wasn't brought up to date), else it would be
/// restarted every minute.
pub fn ask_follow(last: Option<&Value>, now: f64, s: &Settings) -> bool {
    let Some(last) = last else { return true };
    let recent = last["at"].as_f64().is_some_and(|at| now - at < s.gap_minutes as f64 * 60.0);
    !(recent && last["reason"].as_str().is_some_and(|r| r.contains(BRIDGE_PROGRAMS)))
}

/// `follow_only`: restarts are off here (an existing Hermes the person brought), only [`BRIDGE_PROGRAMS`] is acted on.
async fn tick(state: &AppState, config: &Config, memory: &mut HashMap<String, Group>, follow_only: bool) {
    let s = &config.agent_restart;
    let now = py::time_time();
    if !follow_only {
        for (agent, since) in crate::agent_tools::stale_agents(config) {
            if now - since >= s.stale_tools_seconds && !pending(&agent).iter().any(|r| r == TOOLS) {
                request_restart(&agent, TOOLS);
            }
        }
    }
    let kept = records(config);
    if config.browser.enabled {
        for found in crate::browsers::stale_proxies(config) {
            // a spare agent (not in `[agents]`): the hub's gateway serves it, restarted through a hub agent
            let hub = || config.agents.values().find(|a| a.space.as_deref().is_none_or(str::is_empty)).map(|a| a.name.clone());
            let Some(agent) = (if config.agents.contains_key(&found) { Some(found) } else { hub() }) else { continue };
            if ask_follow(kept.get(&agent), now, s) {
                request_restart(&agent, BRIDGE_PROGRAMS);
            }
        }
    }
    let zone = crate::dates::load(config).zone();
    let local = zone.from_utc_datetime(&chrono::Utc::now().naive_utc());
    let minute = local.hour() * 60 + local.minute();
    for (key, agents) in groups(config) {
        // first ping and measure one period after the bridge starts (a starting Hermes is not unhealthy)
        let group = memory.entry(key.clone()).or_insert_with(|| Group { last_ping: now, last_measure: now, ..Group::default() });
        let names: Vec<String> = agents.iter().map(|a| a.name.clone()).collect();
        if now - group.last_ping >= s.ping_seconds {
            group.last_ping = now;
            let ping = tokio::time::timeout(Duration::from_secs(10), state.hermes.capabilities(&agents[0])).await;
            group.failures = if matches!(ping, Ok(Ok(_))) { 0 } else { group.failures + 1 };
        }
        if now - group.last_measure >= s.measure_seconds {
            group.last_measure = now;
            measure(config, &agents, group, now).await;
        }
        let hour = match (s.hour, group.quiet) {
            (Some(h), _) => h,
            (None, Some((day, h))) if day == local.ordinal() => h,
            _ => {
                let h = quiet_hour(&activity_hours(state, &agents, zone).await, s.window_start, s.window_end);
                group.quiet = Some((local.ordinal(), h));
                h
            }
        };
        let night = in_night_slot(minute, hour, jitter_minutes(&agents[0].name, s.jitter_minutes), s.window_end);
        let since_last = names
            .iter()
            .filter_map(|n| kept.get(n).and_then(|r| r["at"].as_f64()))
            .fold(None, |acc: Option<f64>, at| Some(acc.map_or(at, |a| a.max(at))))
            .map(|at| now - at);
        // every reason of the agents sharing this gateway: one restart
        let taken: Vec<(String, Vec<String>)> = names
            .iter()
            .map(|n| (n.clone(), pending(n).into_iter().filter(|r| !follow_only || r == BRIDGE_PROGRAMS).collect()))
            .collect();
        let asked = merged(taken.iter().map(|(_, r)| r.as_slice()));
        let asked = (!asked.is_empty()).then(|| asked.join("; "));
        let (failures, night) = if follow_only { (0, false) } else { (group.failures, night) };
        let Some(why) = reason(asked.as_deref(), failures, night, &group.sizes, since_last, s) else {
            group.turn_wait = None;
            continue;
        };
        if now < group.next_try {
            continue;
        }
        let mut busy = None;
        let mut only_turns = true;
        for agent in &agents {
            let doing = activity(state, config, agent, now).await;
            if let Some(what) = doing.busy() {
                busy.get_or_insert((agent.name.clone(), what));
                only_turns &= doing.only_turns();
            }
        }
        match busy {
            None => group.turn_wait = None,
            Some((agent, what)) => {
                // a soon reason held only by turns: waited for at most `turn_wait_seconds`
                let capped = why.soon() && only_turns;
                let waited = if capped { now - *group.turn_wait.get_or_insert(now) } else { 0.0 };
                if !capped {
                    group.turn_wait = None;
                }
                if !capped || waited < s.turn_wait_seconds {
                    group.next_try = now + if why.soon() { s.retry_seconds / 5.0 } else { s.retry_seconds };
                    tracing::info!(target: "bipbridge::restart", agent = %agent, reason = %why.text(), busy = what, "restart waits");
                    continue;
                }
                tracing::warn!(target: "bipbridge::restart", agent = %agent, reason = %why.text(), busy = what,
                    waited = waited.round(), "restart after waiting for the turns to end: its ceiling reached");
            }
        }
        let text = why.text();
        match restart(config, &agents).await {
            Ok(()) => {
                tracing::info!(target: "bipbridge::restart", agents = %names.join(","), reason = %text, "agent restarted");
                record(config, &names, &text, now);
                for (name, done) in &taken {
                    clear_done(name, done);
                }
                *group = Group { last_ping: now, last_measure: now, quiet: group.quiet, ..Group::default() };
            }
            Err(exc) => {
                group.next_try = now + s.retry_seconds;
                tracing::warn!(target: "bipbridge::restart", agents = %names.join(","), reason = %text,
                    error = %crate::agents_admin::tail(&format!("{exc:#}"), 300), "agent not restarted");
            }
        }
    }
}

/// `/v1/status`'s `last_restart` of each agent.
pub fn status_fields(config: &Config) -> BTreeMap<String, Value> {
    records(config).into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quiet_hour_is_the_least_active_of_the_window() {
        assert_eq!(quiet_hour(&[], 1, 6), 4); // no data: 04:00
        assert_eq!(quiet_hour(&[1, 2, 3, 4, 4, 5, 5], 1, 6), 3); // 1, 2, 3 once each: the closest to 04:00
        assert_eq!(quiet_hour(&[1, 2, 4, 5, 5], 1, 6), 3);
        assert_eq!(quiet_hour(&[4, 4, 4], 1, 6), 3); // 3 and 5 tie at 0: 3 (closer to 4, then earliest)
        assert_eq!(quiet_hour(&[], 5, 6), 5); // 04:00 outside the window
        assert_eq!(quiet_hour(&[23, 0, 12], 1, 6), 4); // activity outside the window ignored
    }

    #[test]
    fn the_jitter_is_small_and_stable() {
        assert_eq!(jitter_minutes("chi", 20), jitter_minutes("chi", 20));
        assert!(jitter_minutes("chi", 20) < 20 && jitter_minutes("alex", 20) < 20);
        assert_eq!(jitter_minutes("chi", 0), 0);
        let spread: std::collections::BTreeSet<u32> =
            ["a", "b", "c", "d", "e", "f", "g"].iter().map(|n| jitter_minutes(n, 20)).collect();
        assert!(spread.len() > 1);
    }

    #[test]
    fn the_night_slot_runs_from_the_quiet_hour_to_the_window_end() {
        assert!(!in_night_slot(3 * 60 + 59, 4, 0, 6));
        assert!(in_night_slot(4 * 60, 4, 0, 6));
        assert!(!in_night_slot(4 * 60 + 5, 4, 7, 6)); // its jitter not reached yet
        assert!(in_night_slot(5 * 60 + 59, 4, 7, 6));
        assert!(!in_night_slot(6 * 60, 4, 0, 6)); // past the window: the next night
        assert!(!in_night_slot(14 * 60, 4, 0, 6));
    }

    #[test]
    fn idle_means_nothing_at_all_going_on() {
        assert_eq!(Activity::default().busy(), None);
        for (activity, why) in [
            (Activity { turn: true, ..Default::default() }, "a turn is going on"),
            (Activity { hermes: true, ..Default::default() }, "its Hermes runs a turn"),
            (Activity { live: true, ..Default::default() }, "a Live call"),
            (Activity { job: true, ..Default::default() }, "a background task runs"),
            (Activity { scan: true, ..Default::default() }, "a mail scan runs"),
            (Activity { watcher: true, ..Default::default() }, "a watcher checks"),
            (Activity { routine: true, ..Default::default() }, "a routine runs or is due"),
        ] {
            assert_eq!(activity.busy(), Some(why));
        }
    }

    #[test]
    fn a_routine_running_or_due_soon_counts() {
        let now = 100_000.0;
        assert!(!routine_busy(&[], now, 30));
        assert!(routine_busy(&[json!({"state": "running", "next_run": null})], now, 30));
        assert!(routine_busy(&[json!({"state": "active", "next_run": now + 29.0 * 60.0})], now, 30));
        assert!(!routine_busy(&[json!({"state": "active", "next_run": now + 31.0 * 60.0})], now, 30));
        assert!(routine_busy(&[json!({"state": "active", "next_run": now - 60.0})], now, 30)); // just due: running
        assert!(!routine_busy(&[json!({"state": "paused", "next_run": null})], now, 30));
    }

    #[test]
    fn memory_over_a_threshold_or_grown_too_much() {
        let s = Settings::default();
        assert_eq!(memory_reason(&Sizes::default(), &s), None);
        assert_eq!(memory_reason(&Sizes { hermes: Some(500.0), hermes_start: Some(300.0), chrome: Some(900.0) }, &s), None);
        assert!(memory_reason(&Sizes { hermes: Some(950.0), ..Default::default() }, &s).unwrap().contains("over 900"));
        assert!(memory_reason(&Sizes { hermes: Some(800.0), hermes_start: Some(300.0), ..Default::default() }, &s)
            .unwrap()
            .contains("2.7x"));
        assert!(memory_reason(&Sizes { chrome: Some(1600.0), ..Default::default() }, &s).unwrap().contains("browser"));
    }

    #[test]
    fn each_reason_and_no_reason() {
        let s = Settings::default();
        let big = Sizes { hermes: Some(1200.0), ..Default::default() };
        let small = Sizes { hermes: Some(300.0), ..Default::default() };
        // no reason: no restart, night or not
        assert_eq!(reason(None, 0, true, &small, None, &s), None);
        assert_eq!(reason(None, 0, false, &big, None, &s), None); // memory waits for the night
        // memory: at night only, at most once in 12 hours
        assert!(matches!(reason(None, 0, true, &big, None, &s), Some(Reason::Memory(_))));
        assert_eq!(reason(None, 0, true, &big, Some(3600.0), &s), None);
        // unhealthy: after the pings failed in a row, at most once an hour
        assert_eq!(reason(None, 2, false, &small, None, &s), None);
        assert_eq!(reason(None, 3, false, &small, None, &s), Some(Reason::Unhealthy));
        assert_eq!(reason(None, 3, false, &small, Some(1800.0), &s), None);
        assert_eq!(reason(None, 3, false, &small, Some(3700.0), &s), Some(Reason::Unhealthy));
        // requested: soon, at most once an hour
        assert_eq!(reason(Some("tools changed"), 0, false, &small, None, &s), Some(Reason::Requested("tools changed".into())));
        assert_eq!(reason(Some("tools changed"), 0, false, &small, Some(600.0), &s), None);
        // a new agent's tools: whatever the last restart (two agents made within the hour, 2026-10-30)
        assert_eq!(
            reason(Some(NEW_AGENT_TOOLS), 0, false, &small, Some(60.0), &s),
            Some(Reason::Requested(NEW_AGENT_TOOLS.into()))
        );
        // the bridge's programs changed (an update): whatever the last restart (its « tools » one right after)
        assert_eq!(
            reason(Some(BRIDGE_PROGRAMS), 0, false, &small, Some(60.0), &s),
            Some(Reason::Requested(BRIDGE_PROGRAMS.into()))
        );
        assert!(Reason::Unhealthy.soon() && !Reason::Memory(String::new()).soon());
        // several reasons merged: one restart naming them all; a new agent's among them still skips the gap
        assert_eq!(
            reason(Some("tools; new agent without its tools"), 0, false, &small, Some(60.0), &s),
            Some(Reason::Requested("tools; new agent without its tools".into()))
        );
        assert_eq!(
            reason(Some("tools"), 3, false, &small, None, &s),
            Some(Reason::Requested("tools; unhealthy: Hermes does not answer".into()))
        );
    }

    #[test]
    fn following_the_bridges_programs_is_asked_once() {
        let s = Settings::default();
        assert!(ask_follow(None, 10_000.0, &s));
        assert!(ask_follow(Some(&json!({"at": 9_000.0, "reason": "requested: tools"})), 10_000.0, &s));
        // restarted for it a minute ago and still the old program: not again within the gap
        let just = json!({"at": 9_940.0, "reason": format!("requested: tools; {BRIDGE_PROGRAMS}")});
        assert!(!ask_follow(Some(&just), 10_000.0, &s));
        assert!(ask_follow(Some(&just), 9_940.0 + 3_601.0, &s));
    }

    #[test]
    fn requests_are_kept_until_done() {
        request_restart("unit-test-agent", "tools changed");
        assert_eq!(requested("unit-test-agent").as_deref(), Some("tools changed"));
        clear_request("unit-test-agent");
        assert_eq!(requested("unit-test-agent"), None);
    }

    #[test]
    fn reasons_asked_while_one_waits_make_one_restart() {
        let agent = "unit-test-merge";
        request_restart(agent, TOOLS);
        request_restart(agent, TOOLS); // asked again: once
        request_restart(agent, NEW_AGENT_TOOLS);
        assert_eq!(requested(agent).as_deref(), Some("tools; new agent without its tools"));
        // the agents of one gateway: each reason once, in order
        let other = vec![NEW_AGENT_TOOLS.to_string(), "settings".to_string()];
        assert_eq!(merged([pending(agent).as_slice(), other.as_slice()]), vec![TOOLS, NEW_AGENT_TOOLS, "settings"]);
        // the restart did these two; one asked for meanwhile stays for the next
        let done = pending(agent);
        request_restart(agent, "settings");
        clear_done(agent, &done);
        assert_eq!(requested(agent).as_deref(), Some("settings"));
        clear_done(agent, &["settings".to_string()]);
        assert_eq!(requested(agent), None);
    }

    #[test]
    fn only_turns_are_waited_for_with_a_ceiling() {
        assert!(Activity { turn: true, hermes: true, ..Default::default() }.only_turns());
        for other in [
            Activity { turn: true, live: true, ..Default::default() },
            Activity { turn: true, job: true, ..Default::default() },
            Activity { scan: true, ..Default::default() },
            Activity { watcher: true, ..Default::default() },
            Activity { routine: true, ..Default::default() },
        ] {
            assert!(!other.only_turns(), "{other:?}");
        }
        assert_eq!(Settings::default().turn_wait_seconds, 600.0);
        assert_eq!(Settings::default().stale_tools_seconds, STALE_TOOLS_SECONDS);
    }

    #[test]
    fn hermes_says_whether_its_gateway_works() {
        // Hermes v0.22.5's GET /health/detailed
        let idle = json!({"status": "ok", "gateway_busy": false, "active_agents": 0,
            "readiness": {"checks": {"background_queues": {"active_api_runs": 0}}}});
        assert_eq!(hermes_busy_of(&idle), Some(false));
        assert_eq!(hermes_busy_of(&json!({"gateway_busy": true})), Some(true));
        assert_eq!(hermes_busy_of(&json!({"gateway_busy": false, "active_agents": 2})), Some(true));
        assert_eq!(hermes_busy_of(&json!({"active_agents": "1"})), Some(true));
        assert_eq!(
            hermes_busy_of(&json!({"readiness": {"checks": {"background_queues": {"active_api_runs": 1}}}})),
            Some(true)
        );
        // an older Hermes, or something else: nothing said
        assert_eq!(hermes_busy_of(&json!({"status": "ok"})), None);
        assert_eq!(hermes_busy_of(&json!({"error": "no route"})), None);
    }

    #[test]
    fn settings_read_and_bounded() {
        let s = Settings::from_table(&json!({"enabled": false, "hour": 3, "window_start": 2, "window_end": 1,
            "routine_margin_minutes": 45, "hermes_mb": 700, "chrome_mb": 2000}));
        assert_eq!(s.enabled, Some(false));
        assert_eq!(s.hour, Some(3));
        assert_eq!((s.window_start, s.window_end), (2, 3)); // the end after the start
        assert_eq!(s.routine_margin_minutes, 45);
        assert_eq!((s.hermes_mb, s.chrome_mb), (700.0, 2000.0));
        assert_eq!(Settings::from_table(&json!({})), Settings::default());
        let fast = Settings::from_table(&json!({"turn_wait_seconds": 2, "stale_tools_seconds": 0.5}));
        assert_eq!((fast.turn_wait_seconds, fast.stale_tools_seconds), (2.0, 0.5));
    }

    #[test]
    fn systemctl_show_parsed() {
        assert_eq!(
            parse_show("MemoryCurrent=524288000\nActiveEnterTimestampMonotonic=123\n"),
            Some((500.0, "123".into()))
        );
        assert_eq!(parse_show("MemoryCurrent=[not set]\nActiveEnterTimestampMonotonic=0\n"), None);
        assert_eq!(parse_show("MemoryCurrent=18446744073709551615\n"), None);
    }

    #[test]
    fn an_existing_hermes_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.source_path = Some(dir.path().join("bridge.toml").to_string_lossy().into_owned());
        assert!(enabled(&config));
        std::fs::write(dir.path().join("existing-hermes.json"), r#"{"preexisting": true}"#).unwrap();
        assert!(!enabled(&config));
        config.agent_restart.enabled = Some(true);
        assert!(enabled(&config)); // the setting overrides
    }
}
