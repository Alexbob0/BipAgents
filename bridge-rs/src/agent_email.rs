//! An agent's own e-mail address (decided 2026-10-22), off by default: the person turns it on in the app.
//!
//! - On a place of the all-included offer: an address of Kerfy Studio's (`lea.k7m2@a.bipagents.com`), asked of the
//!   guichet by this bridge itself with the place's own token (`~/.config/bipagents/backup.json`, the backup token of
//!   its setup, as for its backups and voices). The guichet answers the mailbox's password once and doesn't keep it;
//!   the phone never sees it.
//! - Anywhere: a mailbox the person already has (its IMAP/SMTP servers and an app password).
//!
//! Hermes' own e-mail gateway (plugins/platforms/email) reads the mailbox: the bridge writes its `EMAIL_*` settings
//! into the agent's profile `.env` (chmod 600; the hub's agents' tools run in their sandbox and never see it) and
//! restarts its gateway. The rules:
//!
//! - the agent reads and answers only the allowed addresses (`EMAIL_ALLOWED_USERS`: the person's own by default,
//!   more added by them), and only mail whose sender is proved (SPF/DKIM/DMARC, `EMAIL_AUTHSERV_ID`): Hermes drops
//!   the rest without a word;
//! - it sends through this bridge's guard (agent_email_smtp.rs), never straight to the server: to allowed addresses
//!   only, at most `daily_limit` mails a day (20 unless the person says otherwise); a mail to a new address waits for
//!   the person's « Envoyer » in the app, which then allows that address;
//! - what a mail says is someone else's text: the agents' SOUL content rule (souls.rs) covers it, as for pages.
//!
//! Kerfy Studio's mailboxes keep `retention_days` (the guichet's setting, 90): older messages in INBOX and Sent are
//! deleted here every day. A mailbox of the person's own is never purged.
//!
//!     GET    /v1/agents/{agent}/email                 the view (below)
//!     POST   /v1/agents/{agent}/email                 {kind: bipagents|own, start_again?, …}   turned on
//!     PATCH  /v1/agents/{agent}/email                 {allowed?, daily_limit?}
//!     DELETE /v1/agents/{agent}/email                 turned off (a Kerfy Studio address deleted with its mail)
//!     POST   /v1/agents/{agent}/email/held/{id}       {send: bool}                   a held mail sent or dropped
//!
//!     data_dir/agent-email/<agent>.json (chmod 600), data_dir/agent-email/held/<agent>/<id>.eml|.json,
//!     data_dir/agent-email/given/<agent>.json (chmod 600: the guichet's last password, until the address is deleted)
//!
//! The guichet gives an address it already made a new password only with its current one (audit 2026-10-27, 17):
//! when it answers « exists », the bridge asks again with the password it holds (its record's, else the kept one).
//! When this place no longer holds it (an old restore), the answer is 409 « … it can't be taken back here » (the
//! apps know it by these words) and nothing changes. The person can then start again with a new address
//! (Alex 2026-10-29): the same POST with `start_again: true`, sent only after their explicit OK in the app, deletes
//! the old address at the guichet (`DELETE /v1/mailboxes/{agent}`, with the place's token: the address and all its
//! mail; it stays reserved until the mail server deleted it) and asks a new one, kept like any new address. Without
//! that flag nothing is ever deleted. `DELETE …/email` also asks the guichet to delete the address on a place of the
//! offer when this place has no record of it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Extension, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, check_max_items, check_max_len, check_range, ApiResult, FieldError, HttpError, Json, Validate};

pub const DEFAULT_DAILY_LIMIT: u32 = 20;
pub const MAX_DAILY_LIMIT: u32 = 200;
pub const MAX_ALLOWED: usize = 50;
pub const MAX_HELD: usize = 20;
/// Hermes polls the mailbox this often (seconds): a minute is plenty for mail.
pub const POLL_SECONDS: u32 = 60;
/// The keys the bridge writes into the agent's profile `.env`, and removes when turned off.
pub const ENV_KEYS: [&str; 12] = [
    "EMAIL_ADDRESS",
    "EMAIL_PASSWORD",
    "EMAIL_IMAP_HOST",
    "EMAIL_IMAP_PORT",
    "EMAIL_IMAP_SECURITY",
    "EMAIL_SMTP_HOST",
    "EMAIL_SMTP_PORT",
    "EMAIL_SMTP_SECURITY",
    "EMAIL_ALLOWED_USERS",
    "EMAIL_AUTHSERV_ID",
    "EMAIL_POLL_INTERVAL",
    "EMAIL_ALLOW_ALL_USERS",
];

/// Writes to the records, the held mails and the profiles' `.env` one at a time.
pub static LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// An agent's address as this bridge keeps it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub agent: String,
    /// « bipagents » (Kerfy Studio's) or « own »
    pub kind: String,
    pub address: String,
    /// its server's login, when it isn't the address
    pub username: Option<String>,
    pub password: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// tls (from the start), starttls or plain
    pub smtp_security: String,
    /// the receiving server's Authentication-Results id: senders are proved by it
    pub authserv_id: String,
    pub allowed: Vec<String>,
    pub daily_limit: u32,
    /// UTC day (days since 1970) of `sent_today`
    pub sent_day: String,
    pub sent_today: u32,
    pub retention_days: Option<i64>,
    pub purged_day: String,
    pub last_error: Option<String>,
}

impl Record {
    pub fn login(&self) -> &str {
        self.username.as_deref().filter(|u| !u.is_empty()).unwrap_or(&self.address)
    }

    pub fn allows(&self, address: &str) -> bool {
        let address = address.trim().to_lowercase();
        self.allowed.contains(&address)
    }

    pub fn roll_day(&mut self) {
        let today = today();
        if self.sent_day != today {
            self.sent_day = today;
            self.sent_today = 0;
        }
    }
}

pub fn today() -> String {
    ((crate::py::time_time() / 86_400.0).floor() as i64).to_string()
}

pub fn looks_like_address(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else { return false };
    !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && text.len() <= 254
        && !text.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, ',' | '<' | '>' | '"' | ';' | '\\'))
}

/// The allowed addresses, cleaned (lowercased, once each, in the person's order); Err: the first one that isn't.
pub fn clean_allowed(given: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for raw in given {
        let address = raw.trim().to_lowercase();
        if address.is_empty() {
            continue;
        }
        if !looks_like_address(&address) {
            return Err(raw.clone());
        }
        if !out.contains(&address) {
            out.push(address);
        }
    }
    Ok(out)
}

// -- the records ---------------------------------------------------------------------------------------------------

pub fn folder(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-email")
}

fn record_path(folder: &Path, agent: &str) -> PathBuf {
    folder.join(format!("{agent}.json"))
}

pub fn load(folder: &Path, agent: &str) -> Option<Record> {
    let text = std::fs::read_to_string(record_path(folder, agent)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(folder: &Path, record: &Record) -> anyhow::Result<()> {
    std::fs::create_dir_all(folder)?;
    crate::py::chmod(&folder.to_string_lossy(), 0o700)?;
    let path = record_path(folder, &record.agent);
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), serde_json::to_string_pretty(record)?.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn forget(folder: &Path, agent: &str) {
    let _ = std::fs::remove_file(record_path(folder, agent));
    let _ = std::fs::remove_dir_all(folder.join("held").join(agent));
}

/// The last password the guichet gave for an agent's Kerfy Studio address, kept apart from its record until the
/// address is deleted there: the guichet gives an existing address a new password only with its current one
/// (audit 2026-10-27, 17), so a record lost here (its profile not written) doesn't lose the address.
fn given_path(folder: &Path, agent: &str) -> PathBuf {
    folder.join("given").join(format!("{agent}.json"))
}

pub fn keep_given(folder: &Path, agent: &str, address: &str, password: &str) -> anyhow::Result<()> {
    let dir = folder.join("given");
    std::fs::create_dir_all(&dir)?;
    crate::py::chmod(&folder.to_string_lossy(), 0o700)?;
    crate::py::chmod(&dir.to_string_lossy(), 0o700)?;
    let path = given_path(folder, agent);
    let tmp = path.with_extension("tmp");
    let text = serde_json::to_string(&json!({"address": address, "password": password}))?;
    crate::py::write_with_mode(&tmp.to_string_lossy(), text.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The address's current password: its record's, else the one kept when the guichet gave it.
pub fn current_password(folder: &Path, agent: &str) -> Option<String> {
    let from_record = load(folder, agent).filter(|r| r.kind == "bipagents").map(|r| r.password);
    let kept = || {
        let text = std::fs::read_to_string(given_path(folder, agent)).ok()?;
        let data: Value = serde_json::from_str(&text).ok()?;
        data.get("password")?.as_str().map(str::to_string)
    };
    from_record.or_else(kept).filter(|p| !p.is_empty())
}

fn forget_given(folder: &Path, agent: &str) {
    let _ = std::fs::remove_file(given_path(folder, agent));
}

pub fn records(folder: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut out: Vec<Record> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let agent = name.strip_suffix(".json")?.to_string();
            load(folder, &agent)
        })
        .collect();
    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    out
}

/// The record Hermes logs in as at the guard (its address and its mailbox password), compared in constant time.
pub fn find_by_login(folder: &Path, user: &str, password: &str) -> Option<Record> {
    use subtle::ConstantTimeEq;
    let user = user.trim().to_lowercase();
    records(folder).into_iter().find(|r| {
        r.address.to_lowercase() == user
            && !r.password.is_empty()
            && bool::from(r.password.as_bytes().ct_eq(password.as_bytes()))
    })
}

// -- held mails ----------------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Held {
    pub id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub at: f64,
}

fn held_dir(folder: &Path, agent: &str) -> PathBuf {
    folder.join("held").join(agent)
}

fn subject_of(message: &[u8]) -> String {
    let head = String::from_utf8_lossy(&message[..message.len().min(64 * 1024)]).into_owned();
    let mut subject = String::new();
    let mut inside = false;
    for line in head.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if inside && (line.starts_with(' ') || line.starts_with('\t')) {
            subject.push_str(line.trim_start());
            continue;
        }
        inside = line.to_ascii_lowercase().starts_with("subject:");
        if inside {
            subject = line[8..].trim().to_string();
        }
    }
    crate::mailbox::decode(Some(&subject)).chars().take(200).collect()
}

/// A mail kept until the person decides; its id. At most `MAX_HELD` an agent: the oldest goes.
pub fn hold(folder: &Path, agent: &str, from: &str, to: &[String], message: &[u8]) -> anyhow::Result<String> {
    let dir = held_dir(folder, agent);
    std::fs::create_dir_all(&dir)?;
    crate::py::chmod(&folder.join("held").to_string_lossy(), 0o700)?;
    let mut kept = held_list(folder, agent);
    while kept.len() >= MAX_HELD {
        let oldest = kept.remove(0);
        drop_held(folder, agent, &oldest.id);
    }
    let id = crate::py::token_urlsafe(9);
    let meta = Held {
        id: id.clone(),
        from: from.into(),
        to: to.to_vec(),
        subject: subject_of(message),
        at: crate::py::time_time(),
    };
    crate::py::write_with_mode(&dir.join(format!("{id}.eml")).to_string_lossy(), message, 0o600)?;
    crate::py::write_with_mode(
        &dir.join(format!("{id}.json")).to_string_lossy(),
        serde_json::to_string(&meta)?.as_bytes(),
        0o600,
    )?;
    Ok(id)
}

pub fn held_list(folder: &Path, agent: &str) -> Vec<Held> {
    let Ok(entries) = std::fs::read_dir(held_dir(folder, agent)) else { return Vec::new() };
    let mut out: Vec<Held> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    out
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn drop_held(folder: &Path, agent: &str, id: &str) {
    let dir = held_dir(folder, agent);
    let _ = std::fs::remove_file(dir.join(format!("{id}.eml")));
    let _ = std::fs::remove_file(dir.join(format!("{id}.json")));
}

// -- the agent's profile -------------------------------------------------------------------------------------------

/// Hermes' e-mail gateway settings for this record (the guard as its SMTP server).
pub fn env_values(record: &Record, guard_port: u16) -> Vec<(&'static str, String)> {
    let mut out = vec![
        ("EMAIL_ADDRESS", record.address.clone()),
        ("EMAIL_PASSWORD", record.password.clone()),
        ("EMAIL_IMAP_HOST", record.imap_host.clone()),
        ("EMAIL_IMAP_PORT", record.imap_port.to_string()),
        ("EMAIL_IMAP_SECURITY", "tls".into()),
        ("EMAIL_SMTP_HOST", "127.0.0.1".into()),
        ("EMAIL_SMTP_PORT", guard_port.to_string()),
        ("EMAIL_SMTP_SECURITY", "plain".into()),
        ("EMAIL_ALLOWED_USERS", record.allowed.join(",")),
        ("EMAIL_POLL_INTERVAL", POLL_SECONDS.to_string()),
        // never open to anyone, whatever the gateway's defaults
        ("EMAIL_ALLOW_ALL_USERS", "false".into()),
    ];
    if !record.authserv_id.is_empty() {
        out.push(("EMAIL_AUTHSERV_ID", record.authserv_id.clone()));
    }
    out
}

/// The profile's `.env` with these keys set (None: removed), the others kept; written whole, chmod 600, in place.
pub fn write_env(path: &Path, values: &[(&str, Option<String>)]) -> std::io::Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = text
        .lines()
        .filter(|line| !values.iter().any(|(k, _)| line.split('=').next().unwrap_or("").trim() == *k))
        .map(str::to_string)
        .collect();
    for (key, value) in values {
        if let Some(v) = value {
            let v: String = v.chars().filter(|c| *c != '\n' && *c != '\r').collect();
            lines.push(format!("{key}={v}"));
        }
    }
    let tmp = path.with_extension("bip-tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), format!("{}\n", lines.join("\n")).as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)
}

fn env_path(config: &Config, agent: &AgentConfig) -> Option<PathBuf> {
    crate::shares::profile_dir(config, agent).map(|p| p.join(".env"))
}

/// The gateway serving the agent restarted (its space's, or the hub's) to read its settings again.
async fn restart(config: &Config, agent: &AgentConfig, run: &Run) {
    let mut env = BTreeMap::new();
    let command = match agent.space.as_deref().filter(|s| !s.is_empty()) {
        Some(space) => {
            env.insert("HERMES_HOME".to_string(), space.to_string());
            vec![config.hermes.bin.clone(), "gateway".into(), "restart".into()]
        }
        None => {
            if !config.hermes.home.is_empty() {
                env.insert("HERMES_HOME".to_string(), config.hermes.home.clone());
            }
            let units = crate::place_backup::home().join(".config").join("systemd").join("user");
            crate::cloud_setup::restart_command(config, &units)
        }
    };
    if let Err(e) = run(command, env).await {
        tracing::warn!(agent = %agent.name, error = %crate::agents_admin::tail(&format!("{e:#}"), 300), "gateway not restarted for its e-mail");
    }
}

async fn apply(config: &Config, agent: &AgentConfig, record: Option<&Record>, deps: &Deps) -> Result<(), HttpError> {
    let Some(path) = env_path(config, agent) else {
        return Err(HttpError::new(400, "this agent's settings aren't kept by this install"));
    };
    let values: Vec<(&str, Option<String>)> = match record {
        Some(r) => {
            let set = env_values(r, deps.guard_port);
            ENV_KEYS.iter().map(|k| (*k, set.iter().find(|(s, _)| s == k).map(|(_, v)| v.clone()))).collect()
        }
        None => ENV_KEYS.iter().map(|k| (*k, None)).collect(),
    };
    write_env(&path, &values).map_err(anyhow::Error::from)?;
    restart(config, agent, &deps.run).await;
    Ok(())
}

// -- the guichet ---------------------------------------------------------------------------------------------------

/// The guichet's address and this place's token, when this is a place of the offer.
pub fn place_token(home: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(home.join(crate::place_backup::SETTINGS)).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let url = data.get("url")?.as_str()?.trim();
    let token = data.get("token")?.as_str()?.trim();
    if url.is_empty() || token.is_empty() {
        return None;
    }
    let base = url.trim_end_matches('/').trim_end_matches("/v1/backup").trim_end_matches('/').to_string();
    Some((base, token.to_string()))
}

/// The guichet's refusal: its status and its detail.
struct Refusal {
    status: u16,
    detail: String,
}

impl Refusal {
    /// « this address exists: its current password is needed »: the guichet's 403 for an address already made.
    fn address_exists(&self) -> bool {
        self.status == 403 && self.detail.contains("exists")
    }

    fn into_http(self) -> HttpError {
        HttpError::new(if matches!(self.status, 404 | 409 | 503) { self.status } else { 502 }, self.detail)
    }
}

async fn guichet_call(
    deps: &Deps,
    method: reqwest::Method,
    agent: &str,
    body: Option<Value>,
) -> Result<Result<Value, Refusal>, HttpError> {
    let Some((base, token)) = place_token(&deps.home) else {
        return Err(HttpError::new(503, "these addresses come with the all-included offer only"));
    };
    let mut req = deps.http.request(method, format!("{base}/v1/mailboxes/{agent}")).bearer_auth(token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.map_err(|_| HttpError::new(502, "the guichet doesn't answer"))?;
    let status = res.status().as_u16();
    let data: Value = res.json().await.unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(Ok(data));
    }
    let detail = data.get("detail").and_then(Value::as_str).unwrap_or("the guichet refused").to_string();
    Ok(Err(Refusal { status, detail }))
}

async fn guichet(deps: &Deps, method: reqwest::Method, agent: &str, body: Option<Value>) -> Result<Value, HttpError> {
    guichet_call(deps, method, agent, body).await?.map_err(Refusal::into_http)
}

/// The agent's Kerfy Studio address asked of the guichet. One it already has is given a new password only with its
/// current one (audit 2026-10-27, 17): asked again with the password this place holds, when it holds one. When it
/// holds none, `start_again` (the person's explicit OK) deletes the old address at the guichet and asks a new one.
async fn ask_address(deps: &Deps, dir: &Path, agent: &AgentConfig, start_again: bool) -> Result<Value, HttpError> {
    let first = guichet_call(deps, reqwest::Method::PUT, &agent.name, Some(json!({"name": agent.display_name}))).await?;
    let refusal = match first {
        Ok(given) => return Ok(given),
        Err(r) if r.address_exists() => r,
        Err(r) => return Err(r.into_http()),
    };
    let lost = || {
        HttpError::new(
            409,
            "this agent already has an address and this place no longer has its password: it can't be taken back here",
        )
    };
    let Some(current) = current_password(dir, &agent.name) else {
        if start_again {
            return start_again_with_a_new_address(deps, dir, agent).await;
        }
        tracing::warn!(agent = %agent.name, detail = %refusal.detail, "agent e-mail address exists, no password kept");
        return Err(lost());
    };
    let body = json!({"name": agent.display_name, "current": current});
    match guichet_call(deps, reqwest::Method::PUT, &agent.name, Some(body)).await? {
        Ok(given) => {
            tracing::info!(agent = %agent.name, "agent e-mail address asked again with its current password");
            Ok(given)
        }
        Err(r) if r.address_exists() && start_again => start_again_with_a_new_address(deps, dir, agent).await,
        Err(r) if r.address_exists() => Err(lost()),
        Err(r) => Err(r.into_http()),
    }
}

/// The old address deleted at the guichet (with all its mail), then a new one asked: only on the person's request.
async fn start_again_with_a_new_address(deps: &Deps, dir: &Path, agent: &AgentConfig) -> Result<Value, HttpError> {
    tracing::warn!(agent = %agent.name, "agent e-mail: deleting the old address at the guichet, as the person asked");
    delete_at_guichet(deps, dir, &agent.name).await?;
    let given = guichet(deps, reqwest::Method::PUT, &agent.name, Some(json!({"name": agent.display_name}))).await?;
    tracing::info!(agent = %agent.name, "agent e-mail: a new address given after the old one was deleted");
    Ok(given)
}

/// The agent's address deleted at the guichet (none there: fine), and the password kept for it forgotten.
async fn delete_at_guichet(deps: &Deps, dir: &Path, agent: &str) -> Result<(), HttpError> {
    match guichet(deps, reqwest::Method::DELETE, agent, None).await {
        Ok(_) => {}
        Err(e) if e.status.as_u16() == 404 => {}
        Err(e) => return Err(e),
    }
    let _lock = LOCK.lock().await;
    forget_given(dir, agent); // the address is gone at the guichet
    Ok(())
}

// -- the view and the routes ---------------------------------------------------------------------------------------

/// What a route needs besides the state (tests give their own).
#[derive(Clone)]
pub struct Deps {
    pub run: Run,
    /// where the place's backup settings are (`$HOME`)
    pub home: PathBuf,
    pub http: reqwest::Client,
    pub guard_port: u16,
}

pub fn view(folder: &Path, agent: &str, offered: bool) -> Value {
    match load(folder, agent) {
        None => json!({
            "enabled": false, "kind": null, "address": null, "allowed": [], "daily_limit": DEFAULT_DAILY_LIMIT,
            "sent_today": 0, "offered": offered, "retention_days": null, "held": [], "last_error": null,
        }),
        Some(mut r) => {
            r.roll_day();
            let held: Vec<Value> = held_list(folder, agent)
                .into_iter()
                .map(|h| json!({"id": h.id, "to": h.to, "subject": h.subject, "at": h.at}))
                .collect();
            let mut last_error = r.last_error.clone();
            if r.authserv_id.is_empty() && last_error.is_none() {
                last_error = Some("no authserv-id: every sender is ignored until it is set".into());
            }
            json!({
                "enabled": true, "kind": r.kind, "address": r.address, "allowed": r.allowed,
                "daily_limit": r.daily_limit, "sent_today": r.sent_today, "offered": offered,
                "retention_days": r.retention_days, "held": held, "last_error": last_error,
            })
        }
    }
}

fn agent_of(config: &Config, name: &str) -> Result<AgentConfig, HttpError> {
    config.agent(Some(name)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

#[derive(Debug, Deserialize)]
pub struct TurnOn {
    pub kind: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub daily_limit: Option<i64>,
    /// kind bipagents: the address exists and this place lost its password: delete it at the guichet and ask a new
    /// one (the person's explicit OK in the app)
    #[serde(default)]
    pub start_again: bool,
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub imap_host: Option<String>,
    #[serde(default)]
    pub imap_port: Option<i64>,
    #[serde(default)]
    pub smtp_host: Option<String>,
    #[serde(default)]
    pub smtp_port: Option<i64>,
    #[serde(default)]
    pub authserv_id: Option<String>,
    /// tls, starttls, or plain (this machine only: Proton Mail Bridge, tests); default from the port
    #[serde(default)]
    pub smtp_security: Option<String>,
}

fn limits(errors: &mut Vec<FieldError>, allowed: Option<&Vec<String>>, daily: Option<i64>) {
    check_max_items(errors, "allowed", allowed.map(Vec::as_slice), MAX_ALLOWED);
    if let Some(d) = daily {
        check_range(errors, web::body_loc("daily_limit"), d as f64, Some(1.0), Some(MAX_DAILY_LIMIT as f64), json!(d));
    }
}

impl Validate for TurnOn {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "kind", Some(&self.kind), 20);
        limits(errors, Some(&self.allowed), self.daily_limit);
        for (name, value) in [
            ("address", &self.address),
            ("username", &self.username),
            ("imap_host", &self.imap_host),
            ("smtp_host", &self.smtp_host),
            ("authserv_id", &self.authserv_id),
        ] {
            check_max_len(errors, name, value.as_deref(), 254);
        }
        check_max_len(errors, "password", self.password.as_deref(), 500);
        for (name, port) in [("imap_port", self.imap_port), ("smtp_port", self.smtp_port)] {
            if let Some(p) = port {
                check_range(errors, web::body_loc(name), p as f64, Some(1.0), Some(65535.0), json!(p));
            }
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Change {
    #[serde(default)]
    pub allowed: Option<Vec<String>>,
    #[serde(default)]
    pub daily_limit: Option<i64>,
}

impl Validate for Change {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        limits(errors, self.allowed.as_ref(), self.daily_limit);
    }
}

#[derive(Debug, Deserialize)]
pub struct Decision {
    pub send: bool,
}

impl Validate for Decision {
    fn validate(&self, _errors: &mut Vec<FieldError>) {}
}

/// The receiving server's Authentication-Results id for the big providers (what the person would have to look up).
pub fn known_authserv_id(imap_host: &str) -> &'static str {
    let host = imap_host.to_lowercase();
    if host == "imap.gmail.com" || host.ends_with(".googlemail.com") {
        "mx.google.com"
    } else {
        ""
    }
}

fn clean(text: Option<&str>) -> String {
    text.unwrap_or("").trim().to_string()
}

async fn get_one(
    State(state): State<AppState>,
    Extension(deps): Extension<Deps>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    Ok(web::ok(view(&folder(&config), &agent.name, place_token(&deps.home).is_some())))
}

async fn turn_on(
    State(state): State<AppState>,
    Extension(deps): Extension<Deps>,
    UrlPath(name): UrlPath<String>,
    Json(body): Json<TurnOn>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let dir = folder(&config);
    if env_path(&config, &agent).is_none() {
        return Err(HttpError::new(400, "this agent's settings aren't kept by this install"));
    }
    if load(&dir, &agent.name).is_some() {
        return Err(HttpError::new(409, "this agent has an address already"));
    }
    let allowed =
        clean_allowed(&body.allowed).map_err(|a| HttpError::new(400, format!("not an e-mail address: {a}")))?;
    let mut record = Record {
        agent: agent.name.clone(),
        kind: body.kind.clone(),
        allowed,
        daily_limit: body.daily_limit.map(|d| d as u32).unwrap_or(DEFAULT_DAILY_LIMIT),
        sent_day: today(),
        ..Record::default()
    };
    match body.kind.as_str() {
        "bipagents" => {
            let given = ask_address(&deps, &dir, &agent, body.start_again).await?;
            let text = |v: &Value| v.as_str().unwrap_or("").to_string();
            let port = |v: &Value, default: u16| v.as_u64().map(|p| p as u16).unwrap_or(default);
            record.address = text(&given["address"]).to_lowercase();
            record.password = text(&given["password"]);
            record.imap_host = text(&given["imap"]["host"]);
            record.imap_port = port(&given["imap"]["port"], 993);
            record.smtp_host = text(&given["smtp"]["host"]);
            record.smtp_port = port(&given["smtp"]["port"], 465);
            record.smtp_security = given["smtp"]["security"].as_str().unwrap_or("tls").to_string();
            record.authserv_id = text(&given["authserv_id"]);
            record.retention_days = given["retention_days"].as_i64();
            if !record.address.is_empty() && !record.password.is_empty() {
                // kept before anything else can fail: the guichet's password for this address is this one now
                let _lock = LOCK.lock().await;
                keep_given(&dir, &agent.name, &record.address, &record.password)?;
            }
            if record.address.is_empty() || record.password.is_empty() || record.imap_host.is_empty() {
                return Err(HttpError::new(502, "the guichet's answer is incomplete"));
            }
        }
        "own" => {
            record.address = clean(body.address.as_deref()).to_lowercase();
            record.password = body.password.clone().unwrap_or_default();
            record.username = body.username.as_deref().map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
            record.imap_host = clean(body.imap_host.as_deref());
            record.smtp_host = clean(body.smtp_host.as_deref());
            if !looks_like_address(&record.address)
                || record.password.is_empty()
                || record.imap_host.is_empty()
                || record.smtp_host.is_empty()
            {
                return Err(HttpError::new(400, "the address, its password and its IMAP and SMTP servers are needed"));
            }
            record.imap_port = body.imap_port.map(|p| p as u16).unwrap_or(993);
            record.smtp_port = body.smtp_port.map(|p| p as u16).unwrap_or(465);
            record.smtp_security = match body.smtp_security.as_deref() {
                Some(s @ ("tls" | "starttls")) => s.to_string(),
                Some("plain") if ["127.0.0.1", "localhost", "::1"].contains(&record.smtp_host.as_str()) => {
                    "plain".into()
                }
                Some(_) => return Err(HttpError::new(400, "smtp_security: tls or starttls")),
                None if record.smtp_port == 465 => "tls".into(),
                None => "starttls".into(),
            };
            record.authserv_id = clean(body.authserv_id.as_deref());
            if record.authserv_id.is_empty() {
                record.authserv_id = known_authserv_id(&record.imap_host).to_string();
            }
        }
        _ => return Err(HttpError::new(400, "kind: bipagents or own")),
    }
    {
        let _lock = LOCK.lock().await;
        save(&dir, &record)?;
    }
    if let Err(e) = apply(&config, &agent, Some(&record), &deps).await {
        let _lock = LOCK.lock().await;
        forget(&dir, &agent.name);
        return Err(e);
    }
    tracing::info!(agent = %agent.name, kind = %record.kind, "agent e-mail turned on");
    let mut res = web::ok(view(&dir, &agent.name, place_token(&deps.home).is_some()));
    *res.status_mut() = axum::http::StatusCode::CREATED;
    Ok(res)
}

async fn change(
    State(state): State<AppState>,
    Extension(deps): Extension<Deps>,
    UrlPath(name): UrlPath<String>,
    Json(body): Json<Change>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let dir = folder(&config);
    let (record, allowed_changed) = {
        let _lock = LOCK.lock().await;
        let mut record = load(&dir, &agent.name).ok_or_else(|| HttpError::new(404, "no address"))?;
        let mut changed = false;
        if let Some(given) = &body.allowed {
            let allowed =
                clean_allowed(given).map_err(|a| HttpError::new(400, format!("not an e-mail address: {a}")))?;
            changed = allowed != record.allowed;
            record.allowed = allowed;
        }
        if let Some(d) = body.daily_limit {
            record.daily_limit = d as u32;
        }
        save(&dir, &record)?;
        (record, changed)
    };
    if allowed_changed {
        apply(&config, &agent, Some(&record), &deps).await?;
    }
    Ok(web::ok(view(&dir, &agent.name, place_token(&deps.home).is_some())))
}

async fn turn_off(
    State(state): State<AppState>,
    Extension(deps): Extension<Deps>,
    UrlPath(name): UrlPath<String>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let dir = folder(&config);
    let Some(record) = load(&dir, &agent.name) else {
        // no record here (an old restore): on a place of the offer the guichet may still have the agent's address,
        // deleted on the person's request all the same
        if place_token(&deps.home).is_none() {
            return Err(HttpError::new(404, "no address"));
        }
        delete_at_guichet(&deps, &dir, &agent.name).await?;
        tracing::info!(agent = %agent.name, "agent e-mail: address deleted at the guichet, none kept here");
        return Ok(web::ok(json!({"ok": true})));
    };
    if record.kind == "bipagents" {
        delete_at_guichet(&deps, &dir, &agent.name).await?;
    }
    {
        let _lock = LOCK.lock().await;
        forget(&dir, &agent.name);
    }
    apply(&config, &agent, None, &deps).await?;
    tracing::info!(agent = %agent.name, "agent e-mail turned off");
    Ok(web::ok(json!({"ok": true})))
}

async fn decide_held(
    State(state): State<AppState>,
    Extension(deps): Extension<Deps>,
    UrlPath((name, id)): UrlPath<(String, String)>,
    Json(body): Json<Decision>,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_of(&config, &name)?;
    let dir = folder(&config);
    if !valid_id(&id) {
        return Err(HttpError::new(404, "no such mail"));
    }
    let meta = held_list(&dir, &agent.name)
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(|| HttpError::new(404, "no such mail"))?;
    if !body.send {
        let _lock = LOCK.lock().await;
        drop_held(&dir, &agent.name, &id);
        return Ok(web::ok(view(&dir, &agent.name, place_token(&deps.home).is_some())));
    }
    let message = std::fs::read(held_dir(&dir, &agent.name).join(format!("{id}.eml"))).map_err(anyhow::Error::from)?;
    let mut record = load(&dir, &agent.name).ok_or_else(|| HttpError::new(404, "no address"))?;
    crate::agent_email_smtp::deliver(&record, &meta.from, &meta.to, &message)
        .await
        .map_err(|e| HttpError::new(502, format!("not sent: {}", crate::agents_admin::tail(&format!("{e:#}"), 200))))?;
    {
        let _lock = LOCK.lock().await;
        record = load(&dir, &agent.name).unwrap_or(record);
        record.roll_day();
        record.sent_today += 1;
        for to in &meta.to {
            let to = to.to_lowercase();
            if !record.allowed.contains(&to) && record.allowed.len() < MAX_ALLOWED {
                record.allowed.push(to);
            }
        }
        save(&dir, &record)?;
        drop_held(&dir, &agent.name, &id);
    }
    apply(&config, &agent, Some(&record), &deps).await?;
    Ok(web::ok(view(&dir, &agent.name, place_token(&deps.home).is_some())))
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    router_with(Deps {
        run: crate::agents_admin::default_run(),
        home: crate::place_backup::home(),
        http: reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().unwrap_or_default(),
        guard_port: crate::agent_email_smtp::guard_port(),
    })
}

pub fn router_with(deps: Deps) -> Router<AppState> {
    Router::new()
        .route("/agents/{agent}/email", get(get_one).post(turn_on).patch(change).delete(turn_off))
        .route("/agents/{agent}/email/held/{id}", post(decide_held))
        .layer(Extension(deps))
}

// -- in the background: the guard, and Kerfy Studio's mailboxes' retention ------------------------------------------

/// The guard and the daily purge (app.rs `start_background`).
pub fn start(state: &AppState) {
    let config = state.config();
    let push = state.push.clone();
    let notify: crate::agent_email_smtp::Notify = Arc::new(move |agent, to, id| {
        let push = push.clone();
        tokio::spawn(async move {
            push.notify_email_held(&agent, &to, &id).await;
        });
    });
    let guard = crate::agent_email_smtp::Guard { folder: folder(&config), notify };
    state.spawn(crate::agent_email_smtp::serve(guard, crate::agent_email_smtp::guard_port()));
    let dir = folder(&config);
    state.spawn(async move {
        loop {
            purge_due(&dir).await;
            tokio::time::sleep(std::time::Duration::from_secs(6 * 3600)).await;
        }
    });
}

/// `22-Jul-2026`: IMAP's date for SEARCH BEFORE.
pub fn imap_date(epoch: f64) -> String {
    chrono::DateTime::from_timestamp(epoch as i64, 0).map(|d| d.format("%d-%b-%Y").to_string()).unwrap_or_default()
}

/// Kerfy Studio's mailboxes: messages older than their retention deleted, once a day.
pub async fn purge_due(dir: &Path) {
    for record in records(dir) {
        let Some(days) = record.retention_days.filter(|d| *d > 0 && record.kind == "bipagents") else { continue };
        let today = today();
        if record.purged_day == today {
            continue;
        }
        let before = imap_date(crate::py::time_time() - days as f64 * 86_400.0);
        let account = json!({
            "host": record.imap_host, "port": record.imap_port, "username": record.login(),
            "password": record.password,
        });
        let Value::Object(account) = account else { continue };
        let done = tokio::task::spawn_blocking(move || purge(&account, &before)).await;
        match done {
            Ok(Ok(n)) => {
                if n > 0 {
                    tracing::info!(agent = %record.agent, messages = n, "old e-mails deleted");
                }
                let _lock = LOCK.lock().await;
                if let Some(mut now) = load(dir, &record.agent) {
                    now.purged_day = today;
                    let _ = save(dir, &now);
                }
            }
            Ok(Err(e)) => tracing::warn!(agent = %record.agent, error = %format!("{e:#}"), "old e-mails not deleted"),
            Err(e) => tracing::warn!(agent = %record.agent, error = %e, "old e-mails not deleted"),
        }
    }
}

/// INBOX and the Sent folder: messages before `before` flagged deleted and expunged; how many.
pub fn purge(account: &crate::Dict, before: &str) -> anyhow::Result<usize> {
    let mut conn = crate::mailbox::connect(account)?;
    let mut folders = vec!["INBOX".to_string()];
    if let Ok((status, rows)) = conn.list() {
        if status == "OK" {
            for (flags, name) in crate::mailbox::list_rows(&rows) {
                if flags.contains("\\Sent") && !folders.contains(&name) {
                    folders.push(name);
                }
            }
        }
    }
    let mut total = 0;
    for name in folders {
        let quoted = crate::mailbox::quote(&name);
        if conn.select(&quoted, false).map(|r| r.0 != "OK").unwrap_or(true) {
            continue;
        }
        let (status, data) = conn.uid("SEARCH", &["BEFORE", before])?;
        if status != "OK" {
            continue;
        }
        let line = match data.first() {
            Some(crate::tools_knowledge::imap::Data::Line(l)) => String::from_utf8_lossy(l).into_owned(),
            _ => String::new(),
        };
        let uids: Vec<&str> = line.split_whitespace().collect();
        if uids.is_empty() {
            continue;
        }
        let set = uids.join(",");
        conn.uid("STORE", &[&set, "+FLAGS.SILENT", "(\\Deleted)"])?;
        conn.uid("EXPUNGE", &[&set])?;
        total += uids.len();
    }
    let _ = conn.logout();
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_lists() {
        assert!(looks_like_address("lea@a.bipagents.com"));
        for bad in ["", "lea", "lea@x", "a b@x.com", "a@x.com,b@y.com", "<a@x.com>", "a@.com"] {
            assert!(!looks_like_address(bad), "{bad}");
        }
        let cleaned = clean_allowed(&[" Alex@Example.COM ".into(), "alex@example.com".into(), "".into()]).unwrap();
        assert_eq!(cleaned, ["alex@example.com"]);
        assert_eq!(clean_allowed(&["nope".into()]), Err("nope".into()));
        assert_eq!(known_authserv_id("imap.gmail.com"), "mx.google.com");
        assert_eq!(imap_date(1_790_000_000.0), "21-Sep-2026");
    }

    #[test]
    fn the_profile_gets_the_gateway_s_settings_and_nothing_else_changes() {
        let dir = tempfile::tempdir().unwrap();
        let env = dir.path().join(".env");
        std::fs::write(&env, "OPENAI_API_KEY=k\nEMAIL_PASSWORD=old\n").unwrap();
        let record = Record {
            agent: "lea".into(),
            kind: "bipagents".into(),
            address: "lea.k7m2@a.bipagents.test".into(),
            password: "pw".into(),
            imap_host: "mail.test".into(),
            imap_port: 993,
            smtp_host: "mail.test".into(),
            smtp_port: 465,
            smtp_security: "tls".into(),
            authserv_id: "mail.test".into(),
            allowed: vec!["alex@example.com".into(), "chi@example.com".into()],
            daily_limit: 20,
            ..Record::default()
        };
        let set = env_values(&record, 25_501);
        let values: Vec<(&str, Option<String>)> =
            ENV_KEYS.iter().map(|k| (*k, set.iter().find(|(s, _)| s == k).map(|(_, v)| v.clone()))).collect();
        write_env(&env, &values).unwrap();
        let text = std::fs::read_to_string(&env).unwrap();
        assert!(text.starts_with("OPENAI_API_KEY=k\n"));
        assert!(!text.contains("old"));
        for line in [
            "EMAIL_ADDRESS=lea.k7m2@a.bipagents.test",
            "EMAIL_SMTP_HOST=127.0.0.1",
            "EMAIL_SMTP_PORT=25501",
            "EMAIL_SMTP_SECURITY=plain",
            "EMAIL_IMAP_SECURITY=tls",
            "EMAIL_ALLOWED_USERS=alex@example.com,chi@example.com",
            "EMAIL_AUTHSERV_ID=mail.test",
            "EMAIL_ALLOW_ALL_USERS=false",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} in {text}");
        }
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&env).unwrap().permissions().mode() & 0o777, 0o600);
        let none: Vec<(&str, Option<String>)> = ENV_KEYS.iter().map(|k| (*k, None)).collect();
        write_env(&env, &none).unwrap();
        assert_eq!(std::fs::read_to_string(&env).unwrap(), "OPENAI_API_KEY=k\n");
    }

    #[test]
    fn the_place_token_is_the_backup_one() {
        let dir = tempfile::tempdir().unwrap();
        assert!(place_token(dir.path()).is_none());
        let path = dir.path().join(crate::place_backup::SETTINGS);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"url": "https://guichet.test/v1/backup/", "token": "t1", "password": "p"}"#).unwrap();
        assert_eq!(place_token(dir.path()), Some(("https://guichet.test".into(), "t1".into())));
    }

    #[test]
    fn the_guichet_s_password_is_kept_apart_until_the_address_goes() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("agent-email");
        assert_eq!(current_password(&folder, "lea"), None);
        keep_given(&folder, "lea", "lea.k7m2@a.bipagents.test", "pw1").unwrap();
        assert_eq!(current_password(&folder, "lea").as_deref(), Some("pw1"));
        assert!(records(&folder).is_empty(), "the kept password is no record");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(given_path(&folder, "lea")).unwrap().permissions().mode() & 0o777, 0o600);
        // a record of Kerfy Studio's address wins; one of the person's own mailbox is never sent to the guichet
        let mut record = Record { agent: "lea".into(), kind: "bipagents".into(), password: "pw2".into(), ..Record::default() };
        save(&folder, &record).unwrap();
        assert_eq!(current_password(&folder, "lea").as_deref(), Some("pw2"));
        record.kind = "own".into();
        save(&folder, &record).unwrap();
        assert_eq!(current_password(&folder, "lea").as_deref(), Some("pw1"));
        forget(&folder, "lea");
        forget_given(&folder, "lea");
        assert_eq!(current_password(&folder, "lea"), None);
    }

    #[test]
    fn held_mails_keep_their_subject() {
        let dir = tempfile::tempdir().unwrap();
        let message = b"From: a@x.test\r\nSubject: =?utf-8?q?R=C3=A9union?=\r\n demain\r\n\r\nBody\r\n";
        let id = hold(dir.path(), "lea", "a@x.test", &["b@y.test".into()], message).unwrap();
        let listed = held_list(dir.path(), "lea");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, id);
        assert!(listed[0].subject.starts_with("Réunion"), "{}", listed[0].subject);
        drop_held(dir.path(), "lea", &id);
        assert!(held_list(dir.path(), "lea").is_empty());
    }
}
