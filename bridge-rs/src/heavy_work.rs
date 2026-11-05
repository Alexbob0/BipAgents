//! A green light before heavy work (Alex, 2026-10-30): on a shared server of the offer, an agent installed Flutter and
//! the Android SDK and built in its terminal; the server's disk and memory filled and every place on it went down.
//! The offer's own agents now ask before anything big (`heavy_work`: an install, a build, a download, a data job) and
//! the bridge answers from the place's room, never shown to the person and never in figures:
//!
//! - `go` with a short lease id: one heavy job at a time per place, kept 30 minutes (asked again by the same agent:
//!   renewed), freed by `heavy_work_done`;
//! - `later`: another agent of the place holds the lease, or the place's memory is pressed (its user slice's
//!   `memory.current` near `memory.high` with mostly the programs' own memory in it, not files the kernel keeps in
//!   cache, or the kernel's PSI `memory.pressure` « some avg10 » high). Never « ask again in N s »: GLM then slept in
//!   its terminal and held the person's turn (2 × 120 s, real-server test 2026-10-31). In a person's turn the agent
//!   hands the work to a background task, which starts by itself once there is room; asked from its own background
//!   task, that task is paused and goes on by itself the same way (`call_with`);
//! - `too_big`: the estimate (+ a margin) is more than the place's free disk (statvfs of its home), or the seat agent
//!   says its disk is full (storage_watch.rs, `disk.json`). The agent says so plainly and proposes another way (an
//!   online service, GitHub Actions; a bigger plan is coming — no price).
//!
//! Per place only: the seat agent (installer/cloud/seat-agent.py) writes each place's disk level and sets its slice's
//! limits, but exposes nothing to share a lease between places; each slice's MemoryHigh keeps a place to its share.
//!
//! The backstop needs no model: on the offer, a place held at its slice's `memory.high` for three minutes, or whose
//! disk passes 90 % (its reserve left out: seat-agent.py) or that the seat agent says is full, gets its running
//! background tasks paused (jobs.rs, reason « space »; nothing is killed) and a note to its agents; they go on by
//! themselves once there is room again. « Held at » is what the kernel does: it keeps the slice just under its
//! `memory.high` (1 547 of 1 549 MB for seven minutes on the real server, 2026-10-31: waiting to be over it never
//! fired), so it counts as held when `memory.current` is within 3 % of it or the kernel's `high` count in
//! `memory.events` (each time it had to hold the slice back) grew since the last sample, with mostly programs' memory
//! in it or the slice stalled on memory (PSI).
//!
//! Background tasks pass the same check before each start (jobs.rs `launch`, `task_gate`): a place pressed, full, or
//! with another agent's heavy job on gets the task queued (paused, reason « space »), started by itself once the check
//! clears (`backstop`, each minute).
//!
//! Self-installs: the tools aren't offered, and a call answers `go`. Family, project and shared agents never get them
//! (agent_tools.rs `tools_for`; a relative's or shared turn closes them as any other own tool).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::storage_watch::SeatDisk;
use crate::tools_knowledge::tool_text;
use crate::{py, Dict};

pub const HEAVY_WORK: &str = "heavy_work";
pub const HEAVY_WORK_DONE: &str = "heavy_work_done";
pub const TOOL_NAMES: [&str; 2] = [HEAVY_WORK, HEAVY_WORK_DONE];
pub const KINDS: [&str; 4] = ["install", "build", "download", "data"];
/// A lease lasts this long unless renewed.
pub const LEASE_SECONDS: f64 = 1800.0;
/// Free room asked on top of the estimate: a quarter more, and this much.
pub const MARGIN_BYTES: u64 = 512 * 1024 * 1024;
/// The slice's memory this close to its `memory.high`: pressed.
pub const PRESSED_RATIO: f64 = 0.9;
/// The slice's memory this close to its `memory.high`: the kernel holds it there (the backstop).
pub const HELD_RATIO: f64 = 0.97;
/// At least this share of `memory.high` is the programs' own memory (anon), not files kept in cache, for « near » to
/// count: a slice reading many files fills to its `memory.high` with cache the kernel drops at no cost.
pub const ANON_RATIO: f64 = 0.6;
/// PSI « some avg10 » at which a slice held at its `memory.high` counts for the backstop.
pub const PSI_HELD: f64 = 10.0;
/// PSI « some avg10 » (percent of time stalled on memory) at which the place is pressed.
pub const PSI_PRESSED: f64 = 20.0;
/// Seconds before it is worth asking again when the place is pressed (structured only: never told to the model).
pub const WAIT_PRESSED: u64 = 120;
/// The backstop: disk used past this, or the slice held at its `memory.high` for `OVER_FOR` seconds.
pub const DISK_STOP: f64 = 0.9;
pub const OVER_FOR: f64 = 180.0;
/// Root's reserve in the place's own disk (seat-agent.py): not the place's to use, left out of its levels.
pub const RESERVE_FILE: &str = ".bipagents-reserve";
/// Lifted under these.
pub const DISK_LIFT: f64 = 0.85;
pub const MEMORY_LIFT: f64 = 0.9;
pub const CGROUP_USERS: &str = "/sys/fs/cgroup/user.slice";

const GB: f64 = 1_000_000_000.0;

pub const GO_TEXT: &str = "go. When it's finished, call heavy_work_done with this lease; past 30 minutes, ask again.";
pub const WAIT_BUSY: &str = "later: another big job is running in this space. Don't wait for it in this turn (no \
sleep, no retry loop).";
pub const WAIT_PRESSED_TEXT: &str = "later: this space is busy right now. Don't wait for it in this turn (no sleep, no \
retry loop).";
/// After `later`, in a person's turn.
pub const LATER_TURN: &str = " Tell your person in one plain sentence that it will be done a bit later, and hand it \
to a background task (start_task): it starts by itself once there is room.";
/// After `later`, asked from the agent's own background task (paused by `call_with`).
pub const LATER_TASK: &str = " This background task is paused now and goes on by itself once there is room: end \
this step with one line saying where you stopped.";
pub const TOO_BIG_TEXT: &str = "too_big: this doesn't fit in this space. Don't start it. Tell your person so simply \
and suggest another way: a free online service, or GitHub Actions for builds; a bigger plan is coming.";
/// The backstop's note (storage_watch.rs notes, read on the agents' next turn).
pub const BACKSTOP_NOTE: &str = "Your person's space was too full: your background tasks are paused and go on by \
themselves once there is room. Start nothing big for now; nothing was deleted.";

/// The tools' schemas (behind Hermes' tool search: found by their keywords).
pub fn tools() -> Vec<Value> {
    vec![
        json!({
            "name": HEAVY_WORK,
            "description": "Ask before anything big: installing (SDK, toolchain, packages), building, compiling, \
        downloading or crunching data. Answers go (with a lease), later or too_big. Keywords: install, build, compile, \
        download, SDK, installer, compiler, télécharger, installieren, kompilieren, herunterladen, instalar, compilar, descargar.",
            "inputSchema": {"type": "object", "properties": {
                "kind": {"type": "string", "enum": KINDS},
                "what": {"type": "string", "maxLength": 120},
                "estimate_gb": {"type": "number", "description": "disk it may take"}},
                "required": ["kind", "what"]},
        }),
        json!({
            "name": HEAVY_WORK_DONE,
            "description": "The big job heavy_work allowed is finished: frees its lease.",
            "inputSchema": {"type": "object", "properties": {"lease": {"type": "string"}}, "required": ["lease"]},
        }),
    ]
}

/// Whether `agent` gets the tools: an own agent of an offer place (not one set up by hand).
pub fn offered(agent: &AgentConfig, offer: bool) -> bool {
    offer && !agent.host_home.as_deref().is_some_and(|h| !h.is_empty())
}

// --- the place's room --------------------------------------------------------------------------------------------

/// The place's slice memory, in bytes (None: not readable or « max »).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Memory {
    pub current: Option<u64>,
    pub high: Option<u64>,
    /// PSI « some avg10 »
    pub stalled: Option<f64>,
    /// `memory.events` « high »: how many times the kernel held the slice back at its `memory.high`
    pub high_events: Option<u64>,
    /// `memory.stat` « anon »: the programs' own memory (not files kept in cache)
    pub anon: Option<u64>,
}

impl Memory {
    fn near(&self, ratio: f64) -> bool {
        matches!((self.current, self.high), (Some(c), Some(h)) if h > 0 && c as f64 >= h as f64 * ratio)
    }

    /// Mostly the programs' own memory (unknown: counted as such).
    fn programs(&self) -> bool {
        match (self.anon, self.high) {
            (Some(a), Some(h)) if h > 0 => a as f64 >= h as f64 * ANON_RATIO,
            _ => true,
        }
    }

    /// Close to its ceiling with programs' memory, or stalled on memory.
    pub fn pressed(&self) -> bool {
        (self.near(PRESSED_RATIO) && self.programs()) || self.stalled.is_some_and(|s| s >= PSI_PRESSED)
    }

    /// Held at its `memory.high` by the kernel (within 3 %, or its `high` count grew since the last sample: `grew`),
    /// with programs' memory or stalled.
    pub fn held(&self, grew: bool) -> bool {
        (self.near(HELD_RATIO) || grew) && (self.programs() || self.stalled.is_some_and(|s| s >= PSI_HELD))
    }
}

/// What the bridge knows of the place's room.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Room {
    pub disk_total: Option<u64>,
    pub disk_free: Option<u64>,
    /// root's reserve in it (counted as used by the disk, not the place's)
    pub reserve: u64,
    pub seat: SeatDisk,
    pub memory: Memory,
}

impl Room {
    /// The share of the disk the place uses, its reserve left out.
    pub fn disk_used_ratio(&self) -> Option<f64> {
        match (self.disk_total, self.disk_free) {
            (Some(t), Some(f)) if t > 0 => Some(1.0 - (f + self.reserve).min(t) as f64 / t as f64),
            _ => None,
        }
    }
}

/// The room root's reserve holds in the place's own disk (0: none, or not root's).
pub fn reserve_of(home: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(home.join(RESERVE_FILE)) {
        Ok(m) if m.is_file() && m.uid() == 0 => m.blocks() * 512,
        _ => 0,
    }
}

fn statvfs(path: &Path) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    // SAFETY: statvfs fills the struct it is given; the path is a valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let unit = stat.f_frsize as u64;
    Some((stat.f_blocks as u64 * unit, stat.f_bavail as u64 * unit))
}

fn number(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// « some avg10=… » of a PSI file.
pub fn psi_some(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.starts_with("some "))?;
    line.split_whitespace().find_map(|f| f.strip_prefix("avg10=")).and_then(|v| v.parse().ok())
}

/// The slice of this account (systemd's user-<uid>.slice).
pub fn slice_dir() -> PathBuf {
    // SAFETY: getuid never fails.
    let uid = unsafe { libc::getuid() };
    Path::new(CGROUP_USERS).join(format!("user-{uid}.slice"))
}

/// The value of `key` in a cgroup's « key value » lines (memory.events, memory.stat).
pub fn keyed(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(' ')?.trim().parse().ok())
}

pub fn memory_of(slice: &Path) -> Memory {
    let read = |name: &str| std::fs::read_to_string(slice.join(name)).ok();
    Memory {
        current: number(&slice.join("memory.current")),
        high: number(&slice.join("memory.high")),
        stalled: read("memory.pressure").as_deref().and_then(psi_some),
        high_events: read("memory.events").as_deref().and_then(|t| keyed(t, "high")),
        anon: read("memory.stat").as_deref().and_then(|t| keyed(t, "anon")),
    }
}

pub fn measure(home: &Path, slice: &Path, now: f64) -> Room {
    let disk = statvfs(home);
    Room {
        disk_total: disk.map(|d| d.0),
        disk_free: disk.map(|d| d.1),
        reserve: reserve_of(home),
        seat: crate::storage_watch::seat_disk(home, now),
        memory: memory_of(slice),
    }
}

/// The place's room now (its home, its slice).
pub fn room_now(now: f64) -> Room {
    measure(&crate::place_backup::home(), &slice_dir(), now)
}

// --- the lease ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Lease {
    pub id: String,
    pub agent: String,
    pub kind: String,
    pub what: String,
    pub until: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    lease: Option<Lease>,
}

static LOCK: Mutex<()> = Mutex::new(());

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("heavy_work.json")
}

fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(config: &Config, store: &Store) -> std::io::Result<()> {
    let file = store_path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string(store).map_err(std::io::Error::other)?;
    py::write_with_mode(&file.to_string_lossy(), text.as_bytes(), 0o600)
}

/// The lease held now (an expired one counts as none).
pub fn held(config: &Config, now: f64) -> Option<Lease> {
    load(config).lease.filter(|l| l.until > now)
}

// --- the decision ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// the lease to hold (kept when the agent already had it)
    Go(Option<String>),
    /// another agent's lease (true) or memory pressed (false), and seconds to wait
    Wait(bool, u64),
    TooBig,
}

/// The estimate when the agent gave none, by kind.
pub fn default_gb(kind: &str) -> f64 {
    match kind {
        "install" => 2.0,
        "build" => 1.5,
        "download" => 1.0,
        _ => 0.5,
    }
}

/// The answer for `agent`, from the place's room and the lease held now.
pub fn decide(
    room: &Room,
    held: Option<&Lease>,
    agent: &str,
    kind: &str,
    estimate_gb: Option<f64>,
    now: f64,
) -> Decision {
    let own = held.filter(|l| l.until > now && l.agent == agent).map(|l| l.id.clone());
    if let Some(other) = held.filter(|l| l.until > now && l.agent != agent) {
        return Decision::Wait(true, ((other.until - now).ceil() as u64).clamp(60, 600));
    }
    if room.seat.pause || room.seat.state == "full" {
        return Decision::TooBig;
    }
    let estimate = estimate_gb.filter(|g| g.is_finite() && *g > 0.0).unwrap_or_else(|| default_gb(kind));
    let need = (estimate * GB * 1.25) as u64 + MARGIN_BYTES;
    if room.disk_free.is_some_and(|free| free < need) {
        return Decision::TooBig;
    }
    if room.memory.pressed() {
        return Decision::Wait(false, WAIT_PRESSED);
    }
    Decision::Go(own)
}

fn new_id() -> String {
    format!("hw-{}", &uuid::Uuid::new_v4().simple().to_string()[..6])
}

fn str_arg(arguments: &Dict, key: &str) -> String {
    arguments.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

/// `heavy_work`, for `agent`: `offer` false (a self-install) always goes.
pub fn ask(config: &Config, agent: &str, arguments: &Dict, offer: bool, room: &Room, now: f64) -> Dict {
    let kind = str_arg(arguments, "kind");
    if !KINDS.contains(&kind.as_str()) {
        return tool_text("kind: one of install, build, download, data.", true);
    }
    if !offer {
        return tool_text("go: nothing to hold here.", false);
    }
    let what: String = str_arg(arguments, "what").chars().take(120).collect();
    let estimate = arguments.get("estimate_gb").and_then(Value::as_f64);
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    match decide(room, store.lease.as_ref(), agent, &kind, estimate, now) {
        Decision::Go(own) => {
            let id = own.unwrap_or_else(new_id);
            store.lease = Some(Lease {
                id: id.clone(),
                agent: agent.into(),
                kind: kind.clone(),
                what: what.clone(),
                until: now + LEASE_SECONDS,
            });
            if let Err(err) = save(config, &store) {
                tracing::warn!(target: "bipbridge::heavy", agent, error = %err, "lease not kept");
            }
            tracing::info!(target: "bipbridge::heavy", agent, kind = %kind, what = %what, lease = %id, "heavy work: go");
            let mut out = tool_text(&format!("{GO_TEXT} lease: {id}"), false);
            out.insert("structuredContent".into(), json!({"answer": "go", "lease": id}));
            out
        }
        Decision::Wait(busy, seconds) => {
            tracing::info!(target: "bipbridge::heavy", agent, kind = %kind, busy, seconds, "heavy work: later");
            let mut out = tool_text(&later_text(busy, false), false);
            out.insert("structuredContent".into(), json!({"answer": "later", "busy": busy, "retry_after": seconds}));
            out
        }
        Decision::TooBig => {
            tracing::info!(target: "bipbridge::heavy", agent, kind = %kind, what = %what, "heavy work: too big");
            let mut out = tool_text(TOO_BIG_TEXT, false);
            out.insert("structuredContent".into(), json!({"answer": "too_big"}));
            out
        }
    }
}

/// `heavy_work_done`: the lease freed (only by the agent holding it).
pub fn done(config: &Config, agent: &str, arguments: &Dict) -> Dict {
    let lease = str_arg(arguments, "lease");
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    if store.lease.as_ref().is_some_and(|l| l.id == lease && l.agent == agent) {
        store.lease = None;
        let _ = save(config, &store);
        tracing::info!(target: "bipbridge::heavy", agent, lease = %lease, "heavy work: done");
    }
    tool_text("done.", false)
}

/// What `later` says: never a number of seconds (the model waited them out in its turn).
pub fn later_text(busy: bool, in_task: bool) -> String {
    format!("{}{}", if busy { WAIT_BUSY } else { WAIT_PRESSED_TEXT }, if in_task { LATER_TASK } else { LATER_TURN })
}

/// One call of either tool, measured now.
pub fn call(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> Dict {
    let now = py::time_time();
    if name == HEAVY_WORK_DONE {
        return done(config, &agent.name, arguments);
    }
    let offer = offered(agent, crate::handover::on_offer());
    let room = if offer { room_now(now) } else { Room::default() };
    ask(config, &agent.name, arguments, offer, &room, now)
}

/// `call`, and for a `later` asked from the agent's own background task (no person's turn of it going on, one of
/// its tasks running): that task paused for room (jobs.rs, reason « space »), started again by `backstop` once there
/// is, and the agent told to end its step instead of waiting.
pub async fn call_with(
    config: Arc<Config>,
    agent: AgentConfig,
    name: String,
    arguments: Dict,
    jobs: Option<Arc<crate::jobs::Jobs>>,
) -> anyhow::Result<Dict> {
    let (c, a) = (config.clone(), agent.clone());
    let mut out = tokio::task::spawn_blocking(move || call(&c, &a, &name, &arguments)).await?;
    let later = out.get("structuredContent").and_then(|v| v.get("answer")) == Some(&json!("later"));
    let Some(jobs) = jobs.filter(|_| later) else { return Ok(out) };
    let in_turn = jobs.hub.as_ref().and_then(|hub| hub.asking_session(&agent.name)).is_some();
    if in_turn || !jobs.pause_running_for_space(&agent.name).await {
        return Ok(out);
    }
    let busy = out["structuredContent"]["busy"] == json!(true);
    tracing::info!(target: "bipbridge::heavy", agent = %agent.name, "heavy work later: its background task paused");
    let structured = out.remove("structuredContent").unwrap_or(json!({}));
    out = tool_text(&later_text(busy, true), false);
    out.insert("structuredContent".into(), structured);
    Ok(out)
}

/// Whether a background task of `agent` may start now (jobs.rs `launch`): None, or why not (« space »: the place
/// full; « busy »: its memory pressed, or another agent's heavy job on). The same room `heavy_work` reads.
pub fn task_gate(config: &Config, agent: &str, room: &Room, now: f64) -> Option<&'static str> {
    if room.seat.pause || room.seat.state == "full" || room.disk_used_ratio().is_some_and(|r| r >= DISK_STOP) {
        return Some("space");
    }
    if room.memory.pressed() || held(config, now).is_some_and(|l| l.agent != agent) {
        return Some("busy");
    }
    None
}

/// `task_gate` measured now, on the offer only (a self-install's tasks always start).
pub fn task_waits(config: &Config, agent: &str) -> Option<&'static str> {
    if !crate::handover::on_offer() {
        return None;
    }
    let now = py::time_time();
    task_gate(config, agent, &room_now(now), now)
}

// --- the backstop ------------------------------------------------------------------------------------------------

/// The backstop's state: since when the slice is held at its `memory.high`, whether the pause holds, and the
/// kernel's `high` count at the last sample.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Backstop {
    pub over_since: Option<f64>,
    pub holding: bool,
    pub high_events: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    Pause,
    Lift,
}

impl Backstop {
    /// One sample: a step when the pause starts or lifts.
    pub fn sample(&mut self, room: &Room, now: f64) -> Option<Step> {
        let grew = matches!((self.high_events, room.memory.high_events), (Some(a), Some(b)) if b > a);
        self.high_events = room.memory.high_events;
        if room.memory.held(grew) {
            self.over_since.get_or_insert(now);
        } else {
            self.over_since = None;
        }
        let disk = room.disk_used_ratio().unwrap_or(0.0);
        let full = room.seat.state == "full" || room.seat.pause;
        let memory_long = self.over_since.is_some_and(|since| now - since >= OVER_FOR);
        if !self.holding && (disk >= DISK_STOP || full || memory_long) {
            self.holding = true;
            return Some(Step::Pause);
        }
        let memory_ok = !grew
            && match (room.memory.current, room.memory.high) {
                (Some(c), Some(h)) if h > 0 => (c as f64) < h as f64 * MEMORY_LIFT,
                _ => true,
            };
        if self.holding && disk < DISK_LIFT && !full && memory_ok {
            self.holding = false;
            return Some(Step::Lift);
        }
        None
    }
}

static BACKSTOP: Mutex<Backstop> = Mutex::new(Backstop { over_since: None, holding: false, high_events: None });

/// The backstop's pause holds: no new background task (jobs.rs).
pub fn holding() -> bool {
    BACKSTOP.lock().unwrap_or_else(|e| e.into_inner()).holding
}

/// Sampled every minute (storage_watch.rs `run_forever`), on the offer only.
pub async fn backstop(state: &crate::app::AppState) {
    if !crate::handover::on_offer() {
        return;
    }
    let now = py::time_time();
    let room = tokio::task::spawn_blocking(move || room_now(now)).await.unwrap_or_default();
    let step = BACKSTOP.lock().unwrap_or_else(|e| e.into_inner()).sample(&room, now);
    let jobs = crate::jobs::jobs_of(state);
    match step {
        Some(Step::Pause) => {
            let paused = jobs.pause_for_space().await;
            crate::storage_watch::note_all(&state.config(), BACKSTOP_NOTE);
            tracing::warn!(target: "bipbridge::heavy", paused, current = ?room.memory.current, high = ?room.memory.high,
                stalled = ?room.memory.stalled, disk = ?room.disk_used_ratio(), "place too full: background tasks paused");
        }
        Some(Step::Lift) => {
            jobs.resume_after_space(&state.config()).await;
            tracing::info!(target: "bipbridge::heavy", "room again: background tasks go on");
        }
        // tasks queued for room (their start waited: jobs.rs `launch`) start once the check clears for them
        None if !holding() => jobs.resume_after_space(&state.config()).await,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        (dir, config)
    }

    fn roomy() -> Room {
        Room { disk_total: Some(5_000_000_000), disk_free: Some(4_000_000_000), ..Room::default() }
    }

    fn args(kind: &str, gb: Option<f64>) -> Dict {
        let mut a = Dict::new();
        a.insert("kind".into(), json!(kind));
        a.insert("what".into(), json!("flutter"));
        if let Some(gb) = gb {
            a.insert("estimate_gb".into(), json!(gb));
        }
        a
    }

    fn answer(out: &Dict) -> String {
        out["structuredContent"]["answer"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn go_then_wait_for_another_agent_then_free() {
        let (_dir, config) = setup();
        let go = ask(&config, "dev", &args("build", Some(1.0)), true, &roomy(), 1000.0);
        assert_eq!(answer(&go), "go");
        let lease = go["structuredContent"]["lease"].as_str().unwrap().to_string();
        assert!(lease.starts_with("hw-") && lease.len() == 9);
        // another agent waits, told when to ask again
        let wait = ask(&config, "vie", &args("download", Some(0.1)), true, &roomy(), 1100.0);
        assert_eq!(answer(&wait), "later");
        assert_eq!(wait["structuredContent"]["retry_after"], json!(600));
        // the holder asks again: the same lease, renewed
        let again = ask(&config, "dev", &args("build", Some(1.0)), true, &roomy(), 1500.0);
        assert_eq!(again["structuredContent"]["lease"], json!(lease));
        assert_eq!(held(&config, 1500.0).unwrap().until, 1500.0 + LEASE_SECONDS);
        // only its holder frees it
        done(&config, "vie", &json!({"lease": lease}).as_object().unwrap().clone());
        assert!(held(&config, 1600.0).is_some());
        done(&config, "dev", &json!({"lease": lease}).as_object().unwrap().clone());
        assert!(held(&config, 1600.0).is_none());
        assert_eq!(answer(&ask(&config, "vie", &args("data", None), true, &roomy(), 1700.0)), "go");
    }

    #[test]
    fn a_lease_expires_after_thirty_minutes() {
        let (_dir, config) = setup();
        assert_eq!(answer(&ask(&config, "dev", &args("install", Some(1.0)), true, &roomy(), 0.0)), "go");
        let wait = ask(&config, "vie", &args("install", Some(1.0)), true, &roomy(), LEASE_SECONDS - 30.0);
        assert_eq!(wait["structuredContent"]["retry_after"], json!(60)); // never under a minute
        assert!(held(&config, LEASE_SECONDS + 1.0).is_none());
        assert_eq!(
            answer(&ask(&config, "vie", &args("install", Some(1.0)), true, &roomy(), LEASE_SECONDS + 1.0)),
            "go"
        );
        assert_eq!(held(&config, LEASE_SECONDS + 2.0).unwrap().agent, "vie");
    }

    #[test]
    fn too_big_on_disk_plainly_and_without_numbers() {
        let (_dir, config) = setup();
        // Flutter + Android SDK: about 10 GB in a 5 GB space
        let out = ask(&config, "dev", &args("install", Some(10.0)), true, &roomy(), 0.0);
        assert_eq!(answer(&out), "too_big");
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("doesn't fit") && text.contains("GitHub Actions") && text.contains("bigger plan"));
        assert!(!text.chars().any(|c| c.is_ascii_digit()) && !text.contains('€'));
        assert!(held(&config, 1.0).is_none());
        // the margin: 3.5 GB asked with 4 GB free doesn't fit; 2.5 GB does
        assert_eq!(decide(&roomy(), None, "dev", "install", Some(3.5), 0.0), Decision::TooBig);
        assert_eq!(decide(&roomy(), None, "dev", "install", Some(2.5), 0.0), Decision::Go(None));
        // the kind's estimate when none is given; the seat agent's « full »
        let small = Room { disk_free: Some(2_000_000_000), ..roomy() };
        assert_eq!(decide(&small, None, "dev", "install", None, 0.0), Decision::TooBig);
        assert_eq!(decide(&small, None, "dev", "data", None, 0.0), Decision::Go(None));
        let full = Room { seat: SeatDisk { state: "full".into(), pause: false }, ..roomy() };
        assert_eq!(decide(&full, None, "dev", "data", Some(0.01), 0.0), Decision::TooBig);
    }

    #[test]
    fn memory_pressure_waits() {
        let gb = 1_000_000_000;
        let near = Room {
            memory: Memory { current: Some(950 * gb / 1000), high: Some(gb), ..Memory::default() },
            ..roomy()
        };
        assert_eq!(decide(&near, None, "dev", "build", Some(0.5), 0.0), Decision::Wait(false, WAIT_PRESSED));
        let stalled = Room { memory: Memory { stalled: Some(35.0), ..Memory::default() }, ..roomy() };
        assert_eq!(decide(&stalled, None, "dev", "build", Some(0.5), 0.0), Decision::Wait(false, WAIT_PRESSED));
        let fine = Room {
            memory: Memory { current: Some(gb / 2), high: Some(gb), stalled: Some(1.0), ..Memory::default() },
            ..roomy()
        };
        // near its ceiling with files the kernel keeps in cache, not programs: not pressed
        let cache = Room {
            memory: Memory { current: Some(950 * gb / 1000), high: Some(gb), anon: Some(gb / 5), ..Memory::default() },
            ..roomy()
        };
        assert_eq!(decide(&cache, None, "dev", "build", Some(0.5), 0.0), Decision::Go(None));
        assert_eq!(decide(&fine, None, "dev", "build", Some(0.5), 0.0), Decision::Go(None));
        let (_dir, config) = setup();
        let wait = ask(&config, "dev", &args("build", None), true, &near, 0.0);
        let text = wait["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("later") && !text.to_lowercase().contains("memory") && !text.contains("RAM"));
        // never a number of seconds to wait out in the turn: a background task instead
        assert!(!text.chars().any(|c| c.is_ascii_digit()) && text.contains("start_task") && text.contains("no sleep"));
        assert!(later_text(true, true).contains("paused now") && !later_text(true, true).contains("start_task"));
        assert!(held(&config, 1.0).is_none());
    }

    #[test]
    fn a_task_waits_for_the_same_room_as_heavy_work() {
        let (_dir, config) = setup();
        assert_eq!(task_gate(&config, "dev", &roomy(), 0.0), None);
        let gb = 1_000_000_000;
        let pressed = Room {
            memory: Memory { current: Some(950 * gb / 1000), high: Some(gb), ..Memory::default() },
            ..roomy()
        };
        assert_eq!(task_gate(&config, "dev", &pressed, 0.0), Some("busy"));
        let full = Room { seat: SeatDisk { state: "full".into(), pause: false }, ..roomy() };
        assert_eq!(task_gate(&config, "dev", &full, 0.0), Some("space"));
        // another agent's heavy job: later; its own: no
        assert_eq!(answer(&ask(&config, "vie", &args("install", Some(1.0)), true, &roomy(), 0.0)), "go");
        assert_eq!(task_gate(&config, "dev", &roomy(), 10.0), Some("busy"));
        assert_eq!(task_gate(&config, "vie", &roomy(), 10.0), None);
    }

    #[test]
    fn a_self_install_always_goes() {
        let (_dir, config) = setup();
        let full = Room { disk_free: Some(0), seat: SeatDisk { state: "full".into(), pause: true }, ..roomy() };
        let out = ask(&config, "dev", &args("install", Some(50.0)), false, &full, 0.0);
        assert!(out["content"][0]["text"].as_str().unwrap().starts_with("go"));
        assert!(held(&config, 1.0).is_none());
        assert_eq!(ask(&config, "dev", &args("party", None), true, &roomy(), 0.0)["isError"], json!(true));
        let own = AgentConfig { name: "dev".into(), ..AgentConfig::default() };
        let by_hand = AgentConfig { name: "vie".into(), host_home: Some("/srv/vie".into()), ..AgentConfig::default() };
        assert!(offered(&own, true) && !offered(&own, false) && !offered(&by_hand, true));
    }

    #[test]
    fn only_the_offer_s_own_agents_get_it() {
        let (dir, mut config) = setup();
        for name in ["dev", "famille", "projet"] {
            config.agents.insert(name.into(), AgentConfig { name: name.into(), ..AgentConfig::default() });
        }
        std::fs::write(dir.path().join("family-agent.json"), r#"{"agent": "famille"}"#).unwrap();
        std::fs::write(dir.path().join("project-agents.json"), r#"{"projects": {"projet": {"ends": 0}}}"#).unwrap();
        let has = |name: &str, offer: bool| {
            crate::agent_tools::tools_for_at(&config, &config.agents[name], offer)
                .iter()
                .any(|t| t["name"] == HEAVY_WORK)
        };
        assert!(has("dev", true));
        assert!(!has("dev", false)); // a self-install
        assert!(!has("famille", true) && !has("projet", true));
        // a relative's turn or a shared conversation: closed like the person's other tools
        assert!(crate::agent_shares::closed_tool(HEAVY_WORK).is_some());
        assert!(crate::read_shares::closed_tool(HEAVY_WORK, false).is_some() && crate::read_shares::closed_tool(HEAVY_WORK, true).is_some());
        // never in view: behind the tool search for every kind
        for kind in ["daily", "custom", "guide"] {
            assert!(!crate::tool_diet::kind_diet(kind).direct.contains(&HEAVY_WORK));
        }
    }

    #[test]
    fn small_schemas_found_by_keywords() {
        let tools = tools();
        let text = serde_json::to_string(&tools).unwrap();
        assert!(text.len() < 1300, "{}", text.len());
        for word in ["install", "build", "compile", "download", "SDK", "télécharger", "kompilieren", "descargar"] {
            assert!(tools[0]["description"].as_str().unwrap().contains(word), "{word}");
        }
    }

    #[test]
    fn psi_and_slice_read() {
        let text = "some avg10=12.50 avg60=3.00 avg300=1.00 total=1\nfull avg10=2.00 avg60=0 avg300=0 total=0\n";
        assert_eq!(psi_some(text), Some(12.5));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("memory.current"), "800\n").unwrap();
        std::fs::write(dir.path().join("memory.high"), "max\n").unwrap();
        assert_eq!(memory_of(dir.path()), Memory { current: Some(800), ..Memory::default() });
        std::fs::write(dir.path().join("memory.events"), "low 0\nhigh 1234\nmax 0\noom 0\noom_kill 0\n").unwrap();
        std::fs::write(dir.path().join("memory.stat"), "anon 700\nfile 100\nanon_thp 0\n").unwrap();
        let read = memory_of(dir.path());
        assert_eq!((read.high_events, read.anon), (Some(1234), Some(700)));
    }

    #[test]
    fn the_backstop_pauses_on_full_disk_or_long_memory_and_lifts() {
        let mut b = Backstop::default();
        let gb = 1_000_000_000u64;
        let disk_full = Room { disk_total: Some(10 * gb), disk_free: Some(gb / 2), ..Room::default() };
        assert_eq!(b.sample(&disk_full, 0.0), Some(Step::Pause));
        assert_eq!(b.sample(&disk_full, 60.0), None);
        let between = Room { disk_free: Some(gb + gb / 5), ..disk_full.clone() }; // 88 %: still held
        assert_eq!(b.sample(&between, 120.0), None);
        assert_eq!(b.sample(&roomy(), 180.0), Some(Step::Lift));
        // over its memory.high: only after five minutes
        let over = Room { memory: Memory { current: Some(gb + 1), high: Some(gb), ..Memory::default() }, ..roomy() };
        assert_eq!(b.sample(&over, 1000.0), None);
        assert_eq!(b.sample(&over, 1000.0 + OVER_FOR - 1.0), None);
        assert_eq!(b.sample(&over, 1000.0 + OVER_FOR), Some(Step::Pause));
        let under = Room { memory: Memory { current: Some(gb / 2), high: Some(gb), ..Memory::default() }, ..roomy() };
        assert_eq!(b.sample(&under, 2000.0), Some(Step::Lift));
        // a dip resets the clock
        b.sample(&over, 3000.0);
        b.sample(&under, 3100.0);
        assert_eq!(b.sample(&over, 3000.0 + OVER_FOR), None);
    }

    #[test]
    fn the_backstop_fires_when_the_kernel_holds_the_slice_just_under_its_ceiling() {
        // the real server, 2026-10-31: 1 547 of 1 549 MB for seven minutes, never over it
        let mb = 1_048_576u64;
        let held = |events: u64| Room {
            memory: Memory {
                current: Some(1547 * mb),
                high: Some(1549 * mb),
                stalled: Some(30.0),
                high_events: Some(events),
                anon: Some(1400 * mb),
            },
            ..roomy()
        };
        let mut b = Backstop::default();
        assert_eq!(b.sample(&held(10), 0.0), None);
        assert_eq!(b.sample(&held(400), 60.0), None);
        assert_eq!(b.sample(&held(900), 120.0), None);
        assert_eq!(b.sample(&held(1500), OVER_FOR), Some(Step::Pause));
        // still held back by the kernel (its count grows) though a little under: not lifted
        let lower = |events: u64| Room {
            memory: Memory { current: Some(1300 * mb), high_events: Some(events), ..held(0).memory },
            ..roomy()
        };
        assert_eq!(b.sample(&lower(1600), 300.0), None);
        // let go: lifted
        let calm = Room {
            memory: Memory { current: Some(600 * mb), stalled: Some(0.0), high_events: Some(1600), ..held(0).memory },
            ..roomy()
        };
        assert_eq!(b.sample(&calm, 360.0), Some(Step::Lift));
        // the count alone, growing each minute under 97 %, holds it too
        let mut b = Backstop::default();
        for (i, events) in [100u64, 200, 300, 400, 500].iter().enumerate() {
            let step = b.sample(&lower(*events), i as f64 * 60.0);
            assert_eq!(step, if i == 4 { Some(Step::Pause) } else { None }, "{i}");
        }
        // a slice full of files kept in cache (little anon, not stalled), the count growing: not held
        let cache = |events: u64| Room {
            memory: Memory {
                current: Some(1548 * mb),
                high: Some(1549 * mb),
                stalled: Some(0.5),
                high_events: Some(events),
                anon: Some(200 * mb),
            },
            ..roomy()
        };
        let mut b = Backstop::default();
        for i in 0..10u64 {
            assert_eq!(b.sample(&cache(i * 100), i as f64 * 60.0), None);
        }
        // the seat agent's « full » pauses at once; its reserve is left out of the disk's share
        let mut b = Backstop::default();
        let full = Room { seat: SeatDisk { state: "full".into(), pause: false }, ..roomy() };
        assert_eq!(b.sample(&full, 0.0), Some(Step::Pause));
        assert_eq!(b.sample(&roomy(), 60.0), Some(Step::Lift));
        let gb = 1_000_000_000u64;
        let reserved = Room { disk_total: Some(5 * gb), disk_free: Some(gb / 5), reserve: gb / 5, ..Room::default() };
        assert!((reserved.disk_used_ratio().unwrap() - 0.92).abs() < 1e-9);
    }
}
