//! The agents watch the place's space and ask what to keep (Alex, 2026-10-28: « un agent qui produit quelque chose tous
//! les jours doit prévenir et demander quoi en faire »; « ça ne devrait pas servir d'espace de stockage en vrai »). The
//! place is a working space for the agents, not storage: 5 GB on the all-included offer (place_backup.rs).
//!
//! - **What takes the space**, by agent and by kind, measured every 30 minutes and after big writes (`mark_dirty`),
//!   cached: `audio` (podcasts, agent_tools.rs), `pages` (pages.rs, maps.rs), `received` (uploads, files.rs), `made`
//!   (the files it sent, sent_files.rs), `space` (its sandbox folders and shared folder), `browser` (its Chrome profile),
//!   `archives` (what was put aside here), `connectors` (a hook: `<data>/connector-data/<agent>`, filled by the
//!   connectors when they keep data). The place's total is the hourly backup's measure (everything personal), or the
//!   sum when larger; `other` is the rest (conversations, memory, Carnet).
//! - **80 % and 95 %**: once per threshold per month, ONE card (`bip-space-<id>`, with the approvals) for the agent
//!   using the most space, about its biggest kind, and a note joined to its next turn with the person (the
//!   `/v1/live/recaps/take` path: never in a routine's own run): it tells them in plain words. Choices: keep the last N
//!   (also kept as a rule), move them to the Archives (audio made lighter), send them to the phone (« Te l'envoyer »:
//!   a Boîte message with the files, then a second card « efface-les d'ici ? »), delete them, later. The bridge applies
//!   the choice, logs it (approvals_log.rs, topic « space ») and tells the agent.
//! - **Rules** per agent and kind (`keep_last`), set from the app, the card or the agent's `space_estimate` tool once
//!   the person chose; applied at once and by the purge (app.rs `purge_loop`).
//! - **Never silent when full**: an upload or a file the bridge makes refused for space gets a plain tool result and
//!   the person a card (level 100).
//! - **Pause** (Alex, 2026-10-28: a sandbox volume has no quota and the server is shared): when the place passes its
//!   allowance, or on the offer when the server's free disk falls under 10 %, the biggest agent's sandbox containers
//!   are paused (`podman pause`), its bridge tools answer « ton espace est plein », the person gets the card; resumed
//!   (`podman unpause`) once back under. Each pause and resume is logged.
//!
//! The routes (bridge key):
//!
//!     GET   /v1/storage            {used, allowance, percent, measured_at, by_agent, other, kept_rules, rule_kinds,
//!                                   files_kept_days, paused}
//!     POST  /v1/storage/measure    the same, measured now (and the thresholds checked)
//!     PATCH /v1/storage/rules      {agent, kind, keep_last} (null/0 removes it) → the same
//!     data_dir/storage.json        rules, thresholds told, cards, notes, pauses, history
//!
//! No Python counterpart.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, FieldError, HttpError, Json, Validate};
use crate::{py, Dict};

pub const RUN_PREFIX: &str = "bip-space-";
pub const TOOL_NAME: &str = "space_estimate";
/// The thresholds told, in percent of the allowance.
pub const LEVELS: [u64; 2] = [80, 95];
/// Measured again after this long (or sooner, after a big write).
pub const MEASURE_EVERY: f64 = 1800.0;
pub const KINDS: [&str; 8] = ["audio", "pages", "received", "made", "space", "browser", "archives", "connectors"];
/// The kinds a rule (keep the last N) applies to.
pub const RULE_KINDS: [&str; 4] = ["audio", "pages", "made", "archives"];
/// The server's free disk under which, on the offer, the biggest agent's sandbox pauses.
pub const DISK_MARGIN: f64 = 0.10;
/// « Te l'envoyer »: the biggest files, at most this many, from this size.
pub const SEND_MAX: usize = 10;
pub const SEND_MIN_BYTES: u64 = 1_000_000;
/// The served folder of the files sent to the phone (until the person says they saved them).
pub const SEND_PREFIX: &str = "/bipagents/space-out";
const KEEP_MAX: u32 = 1000;
const HISTORY_MAX: usize = 200;
/// A new « full » card for the same agent at most this often once one was answered.
const FULL_EVERY: f64 = 86400.0;

static LOCK: Mutex<()> = Mutex::new(());
static DIRTY: AtomicBool = AtomicBool::new(true);
static CACHE: LazyLock<Mutex<HashMap<String, Usage>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A big write happened (a podcast, an upload): measured again at the next tick.
pub fn mark_dirty() {
    DIRTY.store(true, Ordering::Relaxed);
}

// --- the store -------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Rule {
    pub keep_last: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Card {
    pub id: String,
    pub agent: String,
    pub level: u64,
    /// « ask », or « sent » (files sent to the phone, waiting for « efface-les d'ici »).
    pub stage: String,
    pub kind: String,
    pub kind_bytes: u64,
    pub count: usize,
    pub keep_last: usize,
    pub send_count: usize,
    pub send_bytes: u64,
    /// Stage « sent »: (where it was, where it is now) for each file sent.
    #[serde(default)]
    pub moved: Vec<(String, String)>,
    pub used: u64,
    pub allowance: Option<u64>,
    #[serde(default)]
    pub paused: bool,
    pub at: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Paused {
    pub since: f64,
    /// « place » (over its allowance) or « disk » (the server's free disk under the margin).
    pub why: String,
    #[serde(default)]
    pub containers: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    rules: BTreeMap<String, BTreeMap<String, Rule>>,
    /// « 2026-10:80 » → when told.
    #[serde(default)]
    notified: BTreeMap<String, f64>,
    #[serde(default)]
    cards: Vec<Card>,
    #[serde(default)]
    notes: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    paused: BTreeMap<String, Paused>,
    #[serde(default)]
    full_at: BTreeMap<String, f64>,
    #[serde(default)]
    history: Vec<Value>,
}

fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("storage.json")
}

fn load(config: &Config) -> Store {
    std::fs::read_to_string(store_path(config)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(config: &Config, store: &Store) -> std::io::Result<()> {
    let file = store_path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    let text = serde_json::to_string(store).map_err(std::io::Error::other)?;
    py::write_with_mode(&temporary.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&temporary, &file)
}

fn change<T>(config: &Config, f: impl FnOnce(&mut Store) -> T) -> std::io::Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = load(config);
    let out = f(&mut store);
    if store.history.len() > HISTORY_MAX {
        let extra = store.history.len() - HISTORY_MAX;
        store.history.drain(..extra);
    }
    save(config, &store)?;
    Ok(out)
}

fn event(store: &mut Store, what: &str, agent: &str, detail: Value) {
    store.history.push(json!({"at": py::time_time(), "event": what, "agent": agent, "detail": detail}));
}

pub fn rules(config: &Config) -> BTreeMap<String, BTreeMap<String, Rule>> {
    load(config).rules
}

/// A rule set (`Some(n)`) or removed (`None`), then applied at once. Err: an unknown kind or a number out of range.
pub fn set_rule(config: &Config, agent: &str, kind: &str, keep_last: Option<u32>) -> anyhow::Result<usize> {
    if !RULE_KINDS.contains(&kind) {
        anyhow::bail!("kind: one of {}", RULE_KINDS.join(", "));
    }
    if keep_last.is_some_and(|n| n > KEEP_MAX) {
        anyhow::bail!("keep_last: at most {KEEP_MAX}");
    }
    change(config, |store| {
        let entry = store.rules.entry(agent.to_string()).or_default();
        match keep_last.filter(|n| *n > 0) {
            Some(n) => {
                entry.insert(kind.to_string(), Rule { keep_last: n });
            }
            None => {
                entry.remove(kind);
            }
        }
        if entry.is_empty() {
            store.rules.remove(agent);
        }
        event(store, "rule", agent, json!({"kind": kind, "keep_last": keep_last}));
    })?;
    tracing::info!(target: "bipbridge::storage", agent, kind, keep_last = ?keep_last, "space rule set");
    let removed = apply_rules(config);
    mark_dirty();
    Ok(removed)
}

// --- measuring -------------------------------------------------------------------------------------------------------

fn size_of(path: &Path) -> u64 {
    let Ok(meta) = path.symlink_metadata() else { return 0 };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0; // links are not followed
    }
    std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| size_of(&e.path())).sum()
}

fn archives_root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("space-archives")
}

pub fn send_root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("space-out")
}

/// The connectors' data of an agent (a hook: the connectors keep their downloads there, counted here).
pub fn connectors_dir(config: &Config, agent: &str) -> PathBuf {
    PathBuf::from(config.data_dir()).join("connector-data").join(agent)
}

#[derive(Debug, Clone, Default)]
pub struct AgentUsage {
    pub agent: String,
    pub name: String,
    pub kinds: BTreeMap<String, u64>,
}

impl AgentUsage {
    pub fn total(&self) -> u64 {
        self.kinds.values().sum()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub used: u64,
    pub allowance: Option<u64>,
    pub measured_at: f64,
    /// Biggest first.
    pub agents: Vec<AgentUsage>,
    pub other: u64,
}

impl Usage {
    pub fn percent(&self) -> Option<u64> {
        self.allowance.filter(|a| *a > 0).map(|a| self.used * 100 / a)
    }
}

/// What the place's agents take now, by kind (a few folder walks).
pub fn measure(config: &Config) -> Usage {
    let mut names: Vec<&AgentConfig> = config.agents.values().collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    let mut seen_uploads: Vec<String> = Vec::new();
    let mut agents = Vec::new();
    for agent in names {
        let mut kinds: BTreeMap<String, u64> = KINDS.iter().map(|k| (k.to_string(), 0)).collect();
        let name = agent.name.as_str();
        kinds.insert("audio".into(), size_of(&crate::agent_tools::audio_root(config).join(name)));
        kinds.insert(
            "pages".into(),
            size_of(&crate::pages::root(config).join(name)) + size_of(&crate::maps::media_root(config).join(name)),
        );
        if let Some(root) = agent.upload_dir_host.as_deref().filter(|r| !r.is_empty()) {
            // a folder several agents share counts once, for the first
            if !seen_uploads.iter().any(|r| r == root) {
                seen_uploads.push(root.to_string());
                kinds.insert("received".into(), size_of(Path::new(root)));
            }
        }
        kinds.insert("made".into(), size_of(&crate::sent_files::root(config).join(name)));
        let mut space = size_of(&PathBuf::from(config.data_dir()).join("envoi").join(name));
        if let Some(profile) = crate::shares::profile_dir(config, agent) {
            space += size_of(&profile.join("sandboxes"));
        }
        let share = crate::shares::share_dir(config, name);
        if share.is_dir() {
            space += size_of(&share);
        }
        kinds.insert("space".into(), space);
        if !config.browser.root.is_empty() {
            kinds.insert("browser".into(), size_of(&Path::new(&config.browser.root).join(name)));
        }
        kinds.insert(
            "archives".into(),
            size_of(&archives_root(config).join(name)) + size_of(&send_root(config).join(name)),
        );
        kinds.insert("connectors".into(), size_of(&connectors_dir(config, name)));
        let shown = if agent.display_name.is_empty() { name.to_string() } else { agent.display_name.clone() };
        agents.push(AgentUsage { agent: name.to_string(), name: shown, kinds });
    }
    agents.sort_by_key(|a| std::cmp::Reverse(a.total()));
    let sum: u64 = agents.iter().map(AgentUsage::total).sum();
    let backup = crate::place_backup::status(&crate::place_backup::home());
    let offer = backup.get("enabled").map(py::truthy).unwrap_or(false);
    let measured = backup.get("used_bytes").and_then(Value::as_u64).unwrap_or(0);
    let used = measured.max(sum);
    Usage {
        used,
        allowance: offer.then_some(crate::place_backup::ALLOWANCE_BYTES as u64),
        measured_at: py::time_time(),
        agents,
        other: used - sum,
    }
}

/// The last measure (measured now when there is none, or it is stale).
pub fn usage(config: &Config, fresh: bool) -> Usage {
    let key = config.data_dir();
    if !fresh {
        let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(found) = cache.get(&key).filter(|u| py::time_time() - u.measured_at < MEASURE_EVERY) {
            return found.clone();
        }
    }
    let measured = measure(config);
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).insert(key, measured.clone());
    measured
}

pub fn body(config: &Config, usage: &Usage) -> Value {
    let store = load(config);
    let by_agent: Vec<Value> = usage
        .agents
        .iter()
        .map(|a| json!({"agent": a.agent, "name": a.name, "total": a.total(), "kinds": a.kinds}))
        .collect();
    json!({
        "used": usage.used,
        "allowance": usage.allowance,
        "percent": usage.percent(),
        "measured_at": usage.measured_at,
        "by_agent": by_agent,
        "other": usage.other,
        "kept_rules": store.rules,
        "rule_kinds": RULE_KINDS,
        "files_kept_days": crate::files_kept::kept_days(config),
        "paused": store.paused.keys().collect::<Vec<_>>(),
    })
}

// --- an agent's files of a kind ------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Item {
    pub path: PathBuf,
    pub bytes: u64,
    pub mtime: f64,
    /// A page's id (pages are removed through pages.rs).
    pub page: Option<String>,
}

fn mtime(path: &Path) -> f64 {
    tk::mtime(path).unwrap_or(0.0)
}

fn files_in(dir: &Path, deep: bool, out: &mut Vec<Item>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else { continue };
        if meta.is_file() && !entry.file_name().to_string_lossy().starts_with('.') {
            out.push(Item { bytes: meta.len(), mtime: mtime(&path), path, page: None });
        } else if meta.is_dir() && deep {
            files_in(&path, true, out);
        }
    }
}

/// The agent's files of `kind`, the newest first (the kinds a choice or a rule acts on).
pub fn items(config: &Config, agent: &str, kind: &str) -> Vec<Item> {
    let mut out = Vec::new();
    match kind {
        "audio" => files_in(&crate::agent_tools::audio_root(config).join(agent), false, &mut out),
        "made" => files_in(&crate::sent_files::root(config).join(agent), true, &mut out),
        "archives" => files_in(&archives_root(config).join(agent), true, &mut out),
        "pages" => {
            let folder = crate::pages::root(config).join(agent);
            for page in crate::pages::list_pages(config, agent) {
                let id = tk::str_or_empty(page.get("page"));
                if id.is_empty() {
                    continue;
                }
                let parts = ["html", "source", "json"].map(|e| folder.join(format!("{id}.{e}")));
                let bytes = parts.iter().map(|p| size_of(p)).sum();
                out.push(Item { path: parts[0].clone(), bytes, mtime: mtime(&parts[2]), page: Some(id) });
            }
        }
        _ => {}
    }
    out.sort_by(|a, b| b.mtime.partial_cmp(&a.mtime).unwrap_or(std::cmp::Ordering::Equal));
    out
}

fn remove_item(config: &Config, agent: &str, item: &Item) -> bool {
    match &item.page {
        Some(id) => crate::pages::delete(config, agent, id),
        None => std::fs::remove_file(&item.path).is_ok(),
    }
}

/// Every one but the newest `keep` removed. Returns (removed, freed bytes).
fn keep_newest(config: &Config, agent: &str, kind: &str, keep: usize) -> (usize, u64) {
    let (mut removed, mut freed) = (0, 0);
    for item in items(config, agent, kind).iter().skip(keep) {
        if remove_item(config, agent, item) {
            removed += 1;
            freed += item.bytes;
        }
    }
    (removed, freed)
}

/// The rules applied (the purge's and a new rule's). Returns how many files went.
pub fn apply_rules(config: &Config) -> usize {
    let mut removed = 0;
    for (agent, kinds) in rules(config) {
        if config.agent(Some(&agent)).is_none() {
            continue;
        }
        for (kind, rule) in kinds {
            let (n, freed) = keep_newest(config, &agent, &kind, rule.keep_last as usize);
            if n > 0 {
                tracing::info!(target: "bipbridge::storage", agent = %agent, kind = %kind, removed = n, freed, "space rule applied");
                removed += n;
            }
        }
    }
    removed
}

fn unique(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    (2..).map(|n| dir.join(format!("{n}-{name}"))).find(|p| !p.exists()).unwrap_or(first)
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_err() {
        std::fs::copy(from, to)?;
        std::fs::remove_file(from)?;
    }
    Ok(())
}

/// An audio file made lighter for the archives (speech quality, mono): kept only when smaller.
fn lighter_audio(from: &Path, to: &Path) -> bool {
    let Some(ffmpeg) = crate::spaces::which("ffmpeg") else { return false };
    let target = to.with_extension("light.m4a");
    let done = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-y", "-loglevel", "error", "-i"])
        .arg(from)
        .args(["-c:a", "aac", "-b:a", "24k", "-ac", "1"])
        .arg(&target)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let smaller = done && size_of(&target) > 0 && size_of(&target) < size_of(from);
    if smaller && std::fs::rename(&target, to.with_extension("m4a")).is_ok() {
        let _ = std::fs::remove_file(from);
        return true;
    }
    let _ = std::fs::remove_file(&target);
    false
}

/// Every one but the newest `keep` moved to the archives (audio made lighter). Returns (moved, freed bytes).
fn archive_older(config: &Config, agent: &str, kind: &str, keep: usize) -> (usize, u64) {
    let dir = archives_root(config).join(agent).join(kind);
    let (mut moved, mut freed) = (0, 0);
    for item in items(config, agent, kind).iter().skip(keep) {
        let name = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let to = unique(&dir, &name);
        let _ = std::fs::create_dir_all(&dir);
        let done = match &item.page {
            Some(id) => std::fs::copy(&item.path, &to).is_ok() && crate::pages::delete(config, agent, id),
            None if kind == "audio" && lighter_audio(&item.path, &to) => true,
            None => move_file(&item.path, &to).is_ok(),
        };
        if done {
            moved += 1;
            let now = if to.exists() { size_of(&to) } else { size_of(&to.with_extension("m4a")) };
            freed += item.bytes.saturating_sub(now);
        }
    }
    (moved, freed)
}

/// The big files « Te l'envoyer » sends: the biggest, from SEND_MIN_BYTES, at most SEND_MAX.
fn to_send(config: &Config, agent: &str, kind: &str) -> Vec<Item> {
    if kind == "pages" {
        return Vec::new();
    }
    let mut found: Vec<Item> = items(config, agent, kind).into_iter().filter(|i| i.bytes >= SEND_MIN_BYTES).collect();
    found.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    found.truncate(SEND_MAX);
    found
}

// --- cards and notes -----------------------------------------------------------------------------------------------

fn kind_fr(kind: &str) -> &'static str {
    match kind {
        "audio" => "les podcasts et audios",
        "pages" => "les pages",
        "received" => "les fichiers reçus",
        "made" => "les fichiers faits",
        "space" => "l'espace de travail",
        "browser" => "le navigateur",
        "archives" => "les archives",
        _ => "les données des connexions",
    }
}

fn kind_en(kind: &str) -> &'static str {
    match kind {
        "audio" => "podcasts and audio",
        "pages" => "pages",
        "received" => "files your person sent you",
        "made" => "files you made and sent",
        "space" => "working space (sandbox files)",
        "browser" => "browser",
        "archives" => "archived files",
        _ => "connection data",
    }
}

/// « 300 Mo », « 1,2 Go », « 45 Ko ».
pub fn size_fr(bytes: u64) -> String {
    let b = bytes as f64;
    let (value, unit) = if b >= 1e9 {
        (b / 1e9, "Go")
    } else if b >= 1e6 {
        (b / 1e6, "Mo")
    } else {
        ((b / 1e3).max(if bytes > 0 { 1.0 } else { 0.0 }), "Ko")
    };
    let text = if value < 10.0 && unit != "Ko" { format!("{value:.1}") } else { format!("{}", value.round() as u64) };
    format!("{} {unit}", text.trim_end_matches(".0").replace('.', ","))
}

fn size_en(bytes: u64) -> String {
    size_fr(bytes).replace(',', ".").replace("Go", "GB").replace("Mo", "MB").replace("Ko", "KB")
}

const WORKING_SPACE: &str = "Ton espace sert au travail de tes agents, pas à tout garder.";

fn shown(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(agent.to_string())
}

/// The sentence on a card (and in the answered row), in French like the bridge's other cards: the apps rebuild it in
/// their own language from `bip_space`.
pub fn sentence(config: &Config, card: &Card) -> String {
    if card.stage == "sent" {
        return format!(
            "Je t'ai envoyé {} fichier{} ({}). Une fois enregistrés sur ton téléphone, je les efface d'ici ?",
            card.send_count,
            if card.send_count > 1 { "s" } else { "" },
            size_fr(card.send_bytes)
        );
    }
    let full = if card.level >= 100 { "Il est plein".to_string() } else { format!("Il est plein à {} %", card.level) };
    let mut text = format!(
        "{WORKING_SPACE} {full} ; {} de {} prennent {}.",
        kind_fr(&card.kind),
        shown(config, &card.agent),
        size_fr(card.kind_bytes)
    );
    if card.paused {
        text.push_str(&format!(
            " Le travail de {} est en pause tant que l'espace est plein.",
            shown(config, &card.agent)
        ));
    }
    text
}

fn choices(card: &Card) -> Vec<&'static str> {
    if card.stage == "sent" {
        return vec!["delete", "deny"];
    }
    let mut out = Vec::new();
    if RULE_KINDS.contains(&card.kind.as_str()) && card.count > card.keep_last {
        out.push("keep_last");
    }
    if ["audio", "pages", "made"].contains(&card.kind.as_str()) && card.count > card.keep_last {
        out.push("archive");
    }
    if card.send_count > 0 {
        out.push("send");
    }
    if RULE_KINDS.contains(&card.kind.as_str()) && card.count > 0 {
        out.push("delete");
    }
    out.push("deny");
    out
}

fn item_of(config: &Config, card: &Card) -> Dict {
    let item = json!({
        "agent": card.agent,
        "run_id": format!("{RUN_PREFIX}{}", card.id),
        "request_id": card.id,
        "command": format!("{} · {}", card.kind, size_fr(if card.stage == "sent" { card.send_bytes } else { card.kind_bytes })),
        "description": sentence(config, card),
        "choices": choices(card),
        "created_at": card.at,
        "session_id": null,
        "bip_space": {
            "level": card.level, "stage": card.stage, "kind": card.kind, "kind_bytes": card.kind_bytes,
            "count": card.count, "keep_last": card.keep_last, "send_count": card.send_count,
            "send_bytes": card.send_bytes, "used": card.used, "allowance": card.allowance, "paused": card.paused,
        },
    });
    item.as_object().cloned().unwrap_or_default()
}

/// The cards waiting, in Hermes' approvals' shape (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    load(config).cards.iter().filter(|c| agent.is_none_or(|a| a == c.agent)).map(|c| item_of(config, c)).collect()
}

pub fn pending_cards(config: &Config) -> Vec<Card> {
    load(config).cards
}

/// The proposed N: the last 10, or half of fewer.
fn proposed_keep(count: usize) -> usize {
    if count > 10 {
        10
    } else {
        (count / 2).max(1).min(count)
    }
}

/// A card about `agent`'s biggest kind that a choice acts on (else its biggest kind).
fn new_card(config: &Config, usage: &Usage, agent: &str, level: u64, paused: bool) -> Card {
    let kinds = usage.agents.iter().find(|a| a.agent == agent).map(|a| a.kinds.clone()).unwrap_or_default();
    let mut ranked: Vec<(&String, &u64)> = kinds.iter().collect();
    ranked.sort_by_key(|(_, b)| std::cmp::Reverse(**b));
    let actionable = ranked
        .iter()
        .find(|(k, b)| **b > 0 && RULE_KINDS.contains(&k.as_str()) && !items(config, agent, k).is_empty())
        .or(ranked.first())
        .map(|(k, b)| (k.to_string(), **b))
        .unwrap_or(("audio".into(), 0));
    let (kind, kind_bytes) = actionable;
    let count = items(config, agent, &kind).len();
    let send = to_send(config, agent, &kind);
    Card {
        id: py::token_hex(6),
        agent: agent.to_string(),
        level,
        stage: "ask".into(),
        keep_last: proposed_keep(count),
        kind,
        kind_bytes,
        count,
        send_count: send.len(),
        send_bytes: send.iter().map(|i| i.bytes).sum(),
        moved: Vec::new(),
        used: usage.used,
        allowance: usage.allowance,
        paused,
        at: py::time_time(),
    }
}

fn note_for(card: &Card) -> String {
    let pct = if card.level >= 100 { "full".to_string() } else { format!("{} % full", card.level) };
    let mut text = format!(
        "Your person's space (their agents' working space, not a storage: 5 GB) is {pct}; your {} take {} ({} files). \
In your next reply, tell them in one or two plain sentences in their language, e.g. « Ton espace est plein à 80 % ; mes \
podcasts prennent 300 Mo. », and that a card with the choices waits in their approvals: keep the last {}, put them in \
the Archives, send them to their phone, or delete them. Don't touch the files yourself: the bridge applies their choice.",
        kind_en(&card.kind),
        size_en(card.kind_bytes),
        card.count,
        card.keep_last
    );
    if card.paused {
        text.push_str(
            " Your workspace (terminal, code, downloads) is paused until there is room again: say so simply if they ask \
for such work.",
        );
    }
    text
}

fn add_note(store: &mut Store, agent: &str, line: String) {
    let notes = store.notes.entry(agent.to_string()).or_default();
    notes.push(line);
    if notes.len() > 5 {
        notes.remove(0);
    }
}

/// What waits for `agent`'s next turn with the person (forgotten here), as a part the apps hide.
pub fn take_note(config: &Config, agent: &str) -> Option<String> {
    let lines = change(config, |store| store.notes.remove(agent)).ok().flatten()?;
    if lines.is_empty() {
        return None;
    }
    Some(format!("<!--bip:space\n{}\n-->", lines.join("\n")))
}

fn month(now: f64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(now as i64, 0).single().map(|t| t.format("%Y-%m").to_string()).unwrap_or_default()
}

/// The thresholds checked after a measure: ONE card and note for the agent using the most space, once per threshold
/// per month (a 95 % told counts for the 80 % too). Returns the card made.
pub fn check(config: &Config, usage: &Usage, now: f64) -> Option<Card> {
    let percent = usage.percent()?;
    let level = LEVELS.iter().rev().find(|l| percent >= **l).copied()?;
    let top = usage.agents.first().filter(|a| a.total() > 0)?;
    let this_month = month(now);
    let card = new_card(config, usage, &top.agent, level, false);
    change(config, |store| {
        if store.notified.contains_key(&format!("{this_month}:{level}")) {
            return None;
        }
        for l in LEVELS.iter().filter(|l| **l <= level) {
            store.notified.insert(format!("{this_month}:{l}"), now);
        }
        store.notified.retain(|_, at| now - *at < 100.0 * 86400.0);
        // an older card of the same agent not answered: replaced
        store.cards.retain(|c| !(c.agent == card.agent && c.stage == "ask"));
        add_note(store, &card.agent, note_for(&card));
        event(store, "threshold", &card.agent, json!({"level": level, "kind": card.kind, "used": usage.used}));
        store.cards.push(card.clone());
        Some(card)
    })
    .ok()
    .flatten()
    .inspect(|c| {
        tracing::info!(target: "bipbridge::storage", agent = %c.agent, threshold = level, kind = %c.kind, used = usage.used, "space threshold: card for the agent")
    })
}

/// Something refused for space (a file the bridge makes, an upload): the person gets a card (level 100) unless one
/// waits for that agent already, at most once a day after one was answered. `note`: the agent is told on its next turn
/// too (when the refusal did not reach it as a tool result).
pub fn full_card(config: &Config, agent: &str, note: bool) -> Option<Card> {
    let usage = usage(config, false);
    let paused = is_paused(config, agent);
    let card = new_card(config, &usage, agent, 100, paused);
    let now = py::time_time();
    change(config, |store| {
        if let Some(waiting) = store.cards.iter_mut().find(|c| c.agent == agent && c.stage == "ask") {
            waiting.paused |= paused;
            return None;
        }
        if store.full_at.get(agent).is_some_and(|at| now - at < FULL_EVERY) {
            return None;
        }
        store.full_at.insert(agent.to_string(), now);
        if note {
            add_note(store, agent, note_for(&card));
        }
        event(store, "full", agent, json!({"kind": card.kind, "used": card.used}));
        store.cards.push(card.clone());
        Some(card)
    })
    .ok()
    .flatten()
}

pub fn is_paused(config: &Config, agent: &str) -> bool {
    load(config).paused.contains_key(agent)
}

/// The plain refusal of a tool that would write, when the place is full or the agent's work paused (and the card).
pub fn refuse_tool(config: &Config, agent: &str) -> Option<Dict> {
    let paused = is_paused(config, agent);
    if !paused && !crate::place_backup::storage_full(&crate::place_backup::home()) {
        return None;
    }
    full_card(config, agent, false);
    tracing::info!(target: "bipbridge::storage", agent, paused, "tool refused: no room left");
    Some(tk::tool_failure(
        "Your person's space has no room left: nothing new can be saved for now. A card with the choices (keep the last ones, \
archive, send to the phone, delete) waits in your person's approvals.",
        "Ton espace est plein : je ne peux plus rien enregistrer pour l'instant. Je t'ai mis une carte pour choisir quoi garder.",
    ))
}

/// An upload refused for space: the card goes to the agent it was for, else the one using the most.
pub fn upload_refused(config: &Config, agent: Option<&str>) {
    let usage = usage(config, false);
    let target = agent.map(str::to_string).or_else(|| usage.agents.first().map(|a| a.agent.clone()));
    if let Some(target) = target {
        full_card(config, &target, true);
    }
}

// --- the person's choice --------------------------------------------------------------------------------------------

/// What a choice did.
#[derive(Debug, Clone, Default)]
pub struct Done {
    pub choice: String,
    pub removed: usize,
    pub freed: u64,
    /// « send »: the files' served paths, for the Boîte message (app.rs).
    pub sent: Vec<String>,
    pub sent_bytes: u64,
}

impl Done {
    pub fn json(&self) -> Value {
        let mut out = json!({"ok": true, "choice": self.choice, "removed": self.removed, "freed": self.freed});
        if self.choice == "send" {
            out["sent"] = json!(self.sent.len());
            out["sent_bytes"] = json!(self.sent_bytes);
        }
        out
    }
}

/// A card answered. Ok(None): no such card (answered already). Err(message): a choice that does not apply.
pub fn answer(config: &Config, agent: &str, id: &str, choice: &str, via: &str) -> Result<Option<Done>, String> {
    let choice = choice.trim().to_lowercase();
    let (word, number) = match choice.split_once(':') {
        Some((w, n)) => (w.to_string(), n.trim().parse::<usize>().ok().filter(|n| *n > 0 && *n <= KEEP_MAX as usize)),
        None => (choice.clone(), None),
    };
    let card = {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let store = load(config);
        let Some(card) = store.cards.iter().find(|c| c.id == id && c.agent == agent).cloned() else { return Ok(None) };
        card
    };
    let allowed = choices(&card);
    if !allowed.contains(&word.as_str()) && !(word == "deny") {
        return Err(format!("choice: one of {}", allowed.join(", ")));
    }
    let mut done = Done { choice: word.clone(), ..Done::default() };
    let mut follow: Option<Card> = None;
    let mut told = String::new();
    let kind_words = kind_en(&card.kind);
    match (card.stage.as_str(), word.as_str()) {
        ("sent", "delete") => {
            for (_, now) in &card.moved {
                let path = Path::new(now);
                done.freed += size_of(path);
                if std::fs::remove_file(path).is_ok() {
                    done.removed += 1;
                }
            }
            let _ = std::fs::remove_dir_all(send_root(config).join(agent).join(&card.id));
            told = format!(
                "Your person saved the files you sent them and they were deleted here ({}).",
                size_en(done.freed)
            );
        }
        ("sent", _) => {
            for (before, now) in &card.moved {
                let _ = move_file(Path::new(now), Path::new(before));
            }
            let _ = std::fs::remove_dir_all(send_root(config).join(agent).join(&card.id));
            told = "Your person keeps the files you sent them here too.".into();
        }
        (_, "keep_last") => {
            let keep = number.unwrap_or(card.keep_last);
            let (n, freed) = keep_newest(config, agent, &card.kind, keep);
            (done.removed, done.freed) = (n, freed);
            if RULE_KINDS.contains(&card.kind.as_str()) {
                let _ = change(config, |store| {
                    store
                        .rules
                        .entry(agent.to_string())
                        .or_default()
                        .insert(card.kind.clone(), Rule { keep_last: keep as u32 });
                });
            }
            told = format!(
                "Your person chose to keep your last {keep} {kind_words} (a rule now: older ones go by themselves); {} freed.",
                size_en(freed)
            );
        }
        (_, "archive") => {
            let (n, freed) = archive_older(config, agent, &card.kind, card.keep_last);
            (done.removed, done.freed) = (n, freed);
            told = format!(
                "Your person chose to put your older {kind_words} in the Archives ({n} moved, the last {} kept).",
                card.keep_last
            );
        }
        (_, "delete") => {
            let (n, freed) = keep_newest(config, agent, &card.kind, 0);
            (done.removed, done.freed) = (n, freed);
            told = format!("Your person chose to delete your {kind_words} ({n} files, {} freed).", size_en(freed));
        }
        (_, "send") => {
            let new_id = py::token_hex(6);
            let dir = send_root(config).join(agent).join(&new_id);
            let mut moved = Vec::new();
            for item in to_send(config, agent, &card.kind) {
                let name = item.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let to = unique(&dir, &name);
                if move_file(&item.path, &to).is_ok() {
                    let served = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    done.sent.push(format!("{SEND_PREFIX}/{agent}/{new_id}/{served}"));
                    done.sent_bytes += item.bytes;
                    moved.push((item.path.to_string_lossy().into_owned(), to.to_string_lossy().into_owned()));
                }
            }
            if !moved.is_empty() {
                follow = Some(Card {
                    id: new_id,
                    stage: "sent".into(),
                    send_count: moved.len(),
                    send_bytes: done.sent_bytes,
                    moved,
                    at: py::time_time(),
                    ..card.clone()
                });
                told = format!(
                    "Your person asked to get {} of your {kind_words} on their phone: they are in their Boîte; once they \
saved them, a card lets them delete them here.",
                    done.sent.len()
                );
            }
        }
        _ => {}
    }
    let decided = change(config, |store| {
        let before = store.cards.len();
        store.cards.retain(|c| c.id != id);
        if store.cards.len() == before {
            return false;
        }
        if let Some(next) = &follow {
            store.cards.push(next.clone());
        }
        if !told.is_empty() {
            add_note(store, agent, told.clone());
        }
        event(
            store,
            "answer",
            agent,
            json!({"kind": card.kind, "choice": choice, "removed": done.removed, "freed": done.freed}),
        );
        true
    })
    .map_err(|e| e.to_string())?;
    if !decided {
        return Ok(None);
    }
    let fields = json!({
        "session_id": null, "run_id": format!("{RUN_PREFIX}{id}"), "request_id": id,
        "command": format!("{} · {}", card.kind, if word == "keep_last" { format!("keep_last {}", number.unwrap_or(card.keep_last)) } else { word.clone() }),
        "description": sentence(config, &card), "topic": "space", "choice": word, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(target: "bipbridge::storage", agent, error = %exc, "space decision not kept");
        }
    }
    tracing::info!(target: "bipbridge::storage", agent, kind = %card.kind, choice = %choice, removed = done.removed, freed = done.freed, "space card answered");
    mark_dirty();
    Ok(Some(done))
}

/// The Boîte message carrying the files sent to the phone (« Te l'envoyer »), in the agent's language.
pub fn send_message(config: &Config, agent: &AgentConfig, paths: &[String]) -> Dict {
    let lead = match crate::handback::language_of(config, agent).as_str() {
        "en" => "Here are the files to save on your phone. Once they're saved, tell me on the card and I'll delete them from your space.",
        "de" => "Hier sind die Dateien zum Speichern auf deinem Telefon. Sobald sie gespeichert sind, sag es mir auf der Karte, dann lösche ich sie aus deinem Bereich.",
        "es" => "Aquí tienes los archivos para guardar en tu teléfono. Cuando estén guardados, dímelo en la tarjeta y los borro de tu espacio.",
        _ => "Voici les fichiers à enregistrer sur ton téléphone. Une fois enregistrés, dis-le-moi sur la carte et je les efface de ton espace.",
    };
    let mut text = lead.to_string();
    for path in paths {
        text.push_str(&format!("\nMEDIA:{path}"));
    }
    let mut message = Dict::new();
    message.insert("id".into(), json!(format!("space:{}", py::token_hex(6))));
    message.insert("message".into(), json!(text));
    message
}

// --- pausing the biggest agent's sandbox ---------------------------------------------------------------------------

fn disk_low(home: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(home.to_string_lossy().as_bytes()) else { return false };
    // SAFETY: statvfs fills the struct it is given; the path is a valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return false;
    }
    let total = stat.f_blocks as f64 * stat.f_frsize as f64;
    let free = stat.f_bavail as f64 * stat.f_frsize as f64;
    total > 0.0 && free / total < DISK_MARGIN
}

async fn podman(args: &[&str]) -> Option<String> {
    let bin = crate::browsers::pyx::which("podman")?;
    let out = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(bin).args(args).stdin(std::process::Stdio::null()).output(),
    )
    .await
    .ok()?
    .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn containers_of(agent: &str) -> Vec<String> {
    let filter = format!("label=hermes-profile={agent}");
    podman(&["ps", "-q", "--filter", "label=hermes-agent=1", "--filter", &filter])
        .await
        .map(|out| out.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// The place over its allowance (or, on the offer, the server's disk nearly full): the biggest agent's sandbox paused,
/// its tools refusing plainly, a card for the person; resumed once back under.
pub async fn enforce(config: &Config, usage: &Usage) {
    let over = usage.allowance.is_some_and(|a| usage.used >= a);
    let disk = config.cloud && usage.allowance.is_some() && {
        let home = crate::place_backup::home();
        tokio::task::spawn_blocking(move || disk_low(&home)).await.unwrap_or(false)
    };
    let paused: Vec<(String, Paused)> = load(config).paused.into_iter().collect();
    if over || disk {
        if !paused.is_empty() {
            return;
        }
        let Some(top) = usage.agents.first() else { return };
        let containers = containers_of(&top.agent).await;
        if !containers.is_empty() {
            let mut args = vec!["pause"];
            args.extend(containers.iter().map(String::as_str));
            if podman(&args).await.is_none() {
                tracing::warn!(target: "bipbridge::storage", agent = %top.agent, "sandbox not paused (podman refused)");
            }
        }
        let why = if over { "place" } else { "disk" };
        let agent = top.agent.clone();
        let _ = change(config, |store| {
            store.paused.insert(
                agent.clone(),
                Paused { since: py::time_time(), why: why.into(), containers: containers.clone() },
            );
            event(store, "pause", &agent, json!({"why": why, "used": usage.used, "containers": containers}));
        });
        tracing::warn!(target: "bipbridge::storage", agent = %top.agent, why, used = usage.used, "space full: the agent's sandbox paused");
        full_card(config, &top.agent, true);
        return;
    }
    for (agent, pause) in paused {
        let mut ids = containers_of(&agent).await;
        for id in pause.containers {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            let mut args = vec!["unpause"];
            args.extend(ids.iter().map(String::as_str));
            let _ = podman(&args).await;
        }
        let _ = change(config, |store| {
            store.paused.remove(&agent);
            for card in store.cards.iter_mut().filter(|c| c.agent == agent) {
                card.paused = false;
            }
            add_note(store, &agent, "There is room again in your person's space: your workspace works again.".into());
            event(store, "resume", &agent, json!({"used": usage.used}));
        });
        tracing::info!(target: "bipbridge::storage", agent = %agent, used = usage.used, "space back under: the agent's sandbox resumed");
    }
}

/// One measure, the thresholds and the pause checked.
pub async fn tick(config: std::sync::Arc<Config>) -> Usage {
    DIRTY.store(false, Ordering::Relaxed);
    let c = config.clone();
    let measured = tokio::task::spawn_blocking(move || {
        let measured = usage(&c, true);
        check(&c, &measured, py::time_time());
        measured
    })
    .await
    .unwrap_or_default();
    enforce(&config, &measured).await;
    measured
}

/// Measured every 30 minutes, or within a minute after a big write.
pub async fn run_forever(state: AppState) {
    tokio::time::sleep(Duration::from_secs(20)).await;
    let mut last = 0.0;
    loop {
        if DIRTY.load(Ordering::Relaxed) || py::time_time() - last >= MEASURE_EVERY {
            tick(state.config()).await;
            last = py::time_time();
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

// --- the agent's tool ------------------------------------------------------------------------------------------------

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Before you set up something recurring that makes files (a routine making podcasts or pages, \
    regular downloads), see the space it will take: gives the average size of yours, the space a month, the room left, \
    and a number to keep. Tell your person in one plain sentence and propose to keep the last N. Once they chose, call it \
    again with keep_last: only the last N are kept from then on.",
        "inputSchema": {"type": "object", "properties": {
            "kind": {"type": "string", "enum": ["audio", "pages", "files"]},
            "per_week": {"type": "number", "description": "how many a week"},
            "keep_last": {"type": "integer", "description": "only once your person chose: keep the last N (0: all)"}},
            "required": ["kind"]},
    })
}

pub fn call(config: &Config, agent: &str, arguments: &Dict) -> Dict {
    let kind = match tk::str_or_empty(arguments.get("kind")).as_str() {
        "audio" => "audio",
        "pages" => "pages",
        "files" | "made" => "made",
        _ => return tk::tool_text("kind: audio, pages or files", true),
    };
    if let Some(keep) = tk::as_py_int(arguments.get("keep_last"))
        .or_else(|| arguments.get("keep_last").and_then(Value::as_f64).map(|f| f as i64))
    {
        let keep = keep.clamp(0, KEEP_MAX as i64) as u32;
        return match set_rule(config, agent, kind, Some(keep)) {
            Ok(_) if keep == 0 => tk::tool_text(&format!("Saved: all your {} are kept.", kind_en(kind)), false),
            Ok(removed) => tk::tool_text(
                &format!(
                    "Saved: only your last {keep} {} are kept; older ones go by themselves ({removed} removed now).",
                    kind_en(kind)
                ),
                false,
            ),
            Err(err) => tk::tool_text(&err.to_string(), true),
        };
    }
    let per_week = arguments.get("per_week").and_then(Value::as_f64).filter(|n| *n > 0.0).unwrap_or(7.0);
    let mine = items(config, agent, kind);
    let default = match kind {
        "audio" => 5_000_000,
        "pages" => 50_000,
        _ => 1_000_000,
    };
    let average = if mine.is_empty() { default } else { mine.iter().map(|i| i.bytes).sum::<u64>() / mine.len() as u64 };
    let month = (average as f64 * per_week * 52.0 / 12.0) as u64;
    let usage = usage(config, false);
    let budget = usage.allowance.unwrap_or(crate::place_backup::ALLOWANCE_BYTES as u64) / 10;
    let fits = (budget / average.max(1)) as f64;
    let keep = fits.min(per_week * 4.0).clamp(3.0, 100.0) as u64;
    let room = match usage.allowance {
        Some(a) => {
            format!("The place: {} of {} used ({} %).", size_en(usage.used), size_en(a), usage.used * 100 / a.max(1))
        }
        None => format!("The place: {} used (no limit set).", size_en(usage.used)),
    };
    let rule = load(config).rules.get(agent).and_then(|r| r.get(kind)).map(|r| r.keep_last);
    let current = match rule {
        Some(n) => format!(" Now kept: your last {n}."),
        None => String::new(),
    };
    tk::tool_text(
        &format!(
            "One of your {} takes about {} ({}). At {per_week} a week: about {} a month. {room}{current} \
Propose to keep the last {keep} (about {}): their space is a working space, not a storage. Once your person chose, call \
space_estimate again with keep_last.",
            kind_en(kind),
            size_en(average),
            if mine.is_empty() { "a usual size".to_string() } else { format!("average of your {}", mine.len()) },
            size_en(month),
            size_en(keep * average)
        ),
        false,
    )
}

// --- routes ---------------------------------------------------------------------------------------------------------

/// `POST /v1/approve` for a `bip-space-` card (app.rs): the choice applied; « send » puts the files in the Boîte.
pub async fn approve(state: &AppState, agent: &AgentConfig, id: &str, choice: &str) -> ApiResult<Response> {
    let (config, name, id_owned, choice_owned) =
        (state.config(), agent.name.clone(), id.to_string(), choice.to_string());
    let done = tokio::task::spawn_blocking(move || answer(&config, &name, &id_owned, &choice_owned, "app"))
        .await
        .map_err(anyhow::Error::from)?
        .map_err(|message| HttpError::new(422, message))?
        .ok_or_else(|| HttpError::new(404, "no such request (answered already?)"))?;
    if !done.sent.is_empty() {
        let message = send_message(&state.config(), agent, &done.sent);
        if let Err(err) = state.outbox.ingest(agent, &message, true, false).await {
            tracing::warn!(target: "bipbridge::storage", agent = %agent.name, error = %format!("{err:#}"), "files to send: the Boîte refused them");
        }
    }
    Ok(web::ok(done.json()))
}

async fn get_storage(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let out = tokio::task::spawn_blocking(move || body(&config, &usage(&config, false)))
        .await
        .map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

async fn measure_now(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let measured = tick(config.clone()).await;
    let out = tokio::task::spawn_blocking(move || body(&config, &measured)).await.map_err(anyhow::Error::from)?;
    Ok(web::ok(out))
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleBody {
    pub agent: String,
    pub kind: String,
    #[serde(default, deserialize_with = "web::lax::opt_int")]
    pub keep_last: Option<i64>,
}

impl Validate for RuleBody {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !RULE_KINDS.contains(&self.kind.as_str()) {
            errors.push(FieldError::new(
                "literal_error",
                vec![json!("body"), json!("kind")],
                "Input should be audio, pages, made or archives",
                json!(self.kind),
            ));
        }
        if let Some(n) = self.keep_last.filter(|n| *n < 0 || *n > KEEP_MAX as i64) {
            errors.push(FieldError::new(
                "less_than_equal",
                vec![json!("body"), json!("keep_last")],
                "Input should be between 0 and 1000",
                json!(n),
            ));
        }
    }
}

async fn patch_rules(State(state): State<AppState>, Json(body): Json<RuleBody>) -> ApiResult<Response> {
    let config = state.config();
    let agent = config.agent(Some(&body.agent)).map(|a| a.name.clone()).ok_or_else(HttpError::not_found)?;
    let out = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        set_rule(&config, &agent, &body.kind, body.keep_last.map(|n| n as u32))?;
        Ok(body_after_change(&config))
    })
    .await
    .map_err(anyhow::Error::from)?
    .map_err(|e| HttpError::new(422, e.to_string()))?;
    Ok(web::ok(out))
}

fn body_after_change(config: &Config) -> Value {
    body(config, &usage(config, true))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/storage", get(get_storage))
        .route("/storage/measure", post(measure_now))
        .route("/storage/rules", patch(patch_rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Config) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        for name in ["vie", "max"] {
            let agent = AgentConfig { name: name.into(), display_name: name.to_uppercase(), ..AgentConfig::default() };
            config.agents.insert(name.into(), agent);
        }
        (dir, config)
    }

    fn audio(config: &Config, agent: &str, n: usize, bytes: usize) {
        let folder = crate::agent_tools::audio_root(config).join(agent);
        std::fs::create_dir_all(&folder).unwrap();
        for i in 0..n {
            let path = folder.join(format!("2026{i:04}-pod.m4a"));
            std::fs::write(&path, vec![0u8; bytes]).unwrap();
            let when = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000 + i as u64 * 100);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(when).unwrap();
        }
    }

    fn usage_at(config: &Config, percent: u64) -> Usage {
        let mut u = measure(config);
        u.allowance = Some(1_000_000);
        u.used = percent * 10_000;
        u
    }

    #[test]
    fn measured_by_agent_and_kind() {
        let (_dir, config) = setup();
        audio(&config, "vie", 3, 2000);
        let u = measure(&config);
        assert_eq!(u.agents[0].agent, "vie");
        assert_eq!(u.agents[0].kinds["audio"], 6000);
        assert_eq!(u.agents[1].total(), 0);
        assert_eq!(u.allowance, None); // not a place of the offer
        let b = body(&config, &u);
        assert_eq!(b["by_agent"][0]["kinds"]["audio"], json!(6000));
        assert_eq!(b["percent"], Value::Null);
    }

    #[test]
    fn one_card_per_threshold_per_month() {
        let (_dir, config) = setup();
        audio(&config, "vie", 20, 1000);
        let now = py::time_time();
        assert!(check(&config, &usage_at(&config, 50), now).is_none());
        let card = check(&config, &usage_at(&config, 81), now).unwrap();
        assert_eq!(
            (card.agent.as_str(), card.level, card.kind.as_str(), card.count, card.keep_last),
            ("vie", 80, "audio", 20, 10)
        );
        assert!(check(&config, &usage_at(&config, 85), now).is_none(), "80 % told once this month");
        let card = check(&config, &usage_at(&config, 96), now).unwrap();
        assert_eq!(card.level, 95);
        assert_eq!(pending_items(&config, None).len(), 1, "the 95 % card replaces the 80 % one");
        let note = take_note(&config, "vie").unwrap();
        assert!(note.starts_with("<!--bip:space\n") && note.contains("95 % full"));
        assert!(take_note(&config, "vie").is_none());
        let item = &pending_items(&config, Some("vie"))[0];
        assert!(item["run_id"].as_str().unwrap().starts_with(RUN_PREFIX));
        assert!(item["description"].as_str().unwrap().starts_with(WORKING_SPACE));
        assert_eq!(item["choices"], json!(["keep_last", "archive", "delete", "deny"]));
    }

    #[test]
    fn each_choice_applied() {
        let (_dir, config) = setup();
        audio(&config, "vie", 20, 1000);
        let card = check(&config, &usage_at(&config, 81), py::time_time()).unwrap();
        let done = answer(&config, "vie", &card.id, "keep_last:5", "app").unwrap().unwrap();
        assert_eq!((done.removed, done.freed), (15, 15_000));
        assert_eq!(items(&config, "vie", "audio").len(), 5);
        assert_eq!(rules(&config)["vie"]["audio"], Rule { keep_last: 5 });
        assert!(answer(&config, "vie", &card.id, "delete", "app").unwrap().is_none(), "answered already");
        let logged = crate::approvals_log::list(&config, "vie", None);
        assert_eq!(logged.last().unwrap()["choice"], json!("keep_last"));
        assert_eq!(logged.last().unwrap()["topic"], json!("space"));

        audio(&config, "vie", 20, 1000);
        let card = full_card(&config, "vie", true).unwrap();
        let done = answer(&config, "vie", &card.id, "archive", "app").unwrap().unwrap();
        assert!(done.removed > 0);
        assert_eq!(items(&config, "vie", "audio").len(), card.keep_last);
        assert_eq!(items(&config, "vie", "archives").len(), done.removed);

        let _ = change(&config, |s| s.full_at.clear());
        let card = full_card(&config, "vie", true).unwrap();
        let done = answer(&config, "vie", &card.id, "delete", "app").unwrap().unwrap();
        assert!(done.removed > 0);
        assert!(items(&config, "vie", "audio").is_empty());
    }

    #[test]
    fn sent_to_the_phone_then_deleted_or_kept() {
        let (_dir, config) = setup();
        audio(&config, "vie", 3, 2_000_000);
        let card = full_card(&config, "vie", true).unwrap();
        assert_eq!((card.send_count, card.send_bytes), (3, 6_000_000));
        let done = answer(&config, "vie", &card.id, "send", "app").unwrap().unwrap();
        assert_eq!(done.sent.len(), 3);
        assert!(done.sent[0].starts_with("/bipagents/space-out/vie/"));
        assert!(items(&config, "vie", "audio").is_empty());
        let next = pending_cards(&config);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].stage, "sent");
        assert_eq!(choices(&next[0]), vec!["delete", "deny"]);
        // kept here after all: back where they were
        answer(&config, "vie", &next[0].id, "deny", "app").unwrap().unwrap();
        assert_eq!(items(&config, "vie", "audio").len(), 3);
        let _ = change(&config, |s| s.full_at.clear());
        let card = full_card(&config, "vie", true).unwrap();
        answer(&config, "vie", &card.id, "send", "app").unwrap().unwrap();
        let next = pending_cards(&config)[0].clone();
        let done = answer(&config, "vie", &next.id, "delete", "app").unwrap().unwrap();
        assert_eq!((done.removed, done.freed), (3, 6_000_000));
        assert_eq!(measure(&config).agents[0].total(), 0);
    }

    #[test]
    fn rules_applied() {
        let (_dir, config) = setup();
        audio(&config, "vie", 8, 100);
        assert_eq!(set_rule(&config, "vie", "audio", Some(3)).unwrap(), 5);
        audio(&config, "max", 4, 100);
        assert_eq!(apply_rules(&config), 0);
        assert!(set_rule(&config, "vie", "browser", Some(3)).is_err());
        set_rule(&config, "vie", "audio", None).unwrap();
        assert!(rules(&config).is_empty());
    }

    #[test]
    fn the_tool_estimates_and_keeps() {
        let (_dir, config) = setup();
        audio(&config, "vie", 4, 4_000_000);
        let out = call(&config, "vie", &json!({"kind": "audio", "per_week": 7}).as_object().unwrap().clone());
        let text = out["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("4 MB") && text.contains("Propose to keep the last"), "{text}");
        let out = call(&config, "vie", &json!({"kind": "audio", "keep_last": 2}).as_object().unwrap().clone());
        assert_eq!(out["isError"], json!(false));
        assert_eq!(items(&config, "vie", "audio").len(), 2);
    }

    #[test]
    fn sizes_in_plain_words() {
        assert_eq!(size_fr(300_000_000), "300 Mo");
        assert_eq!(size_fr(1_200_000_000), "1,2 Go");
        assert_eq!(size_fr(45_000), "45 Ko");
        assert_eq!(size_en(1_200_000_000), "1.2 GB");
    }
}
