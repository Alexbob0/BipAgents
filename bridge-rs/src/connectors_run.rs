//! Running a connector an agent made (connectors.rs): its files pinned, one short-lived container per call in the
//! agent's isolated sandbox, its secrets as environment variables for that run only, its network its approved sites,
//! and every secret's value blanked out of what comes back.
//!
//! - **Pinned files**: approving copies the connector's folder (in the agent's sandbox, `/workspace/…`) into the
//!   bridge's data, where no agent writes, and pins a SHA-256 over it (sorted paths, each file's content hash and
//!   executable bit; links and special files refused; `MAX_FILES` / `MAX_BYTES`). Before each run both are checked
//!   again: the copy (it is what runs, read-only) and the agent's folder (a change there means « à réapprouver »: no
//!   run until the person approves the new files).
//! - **The container**: `podman run --rm -i` on `bipagents-connectors` (no route out; the egress proxy is strict
//!   there: this connector's own proxy credentials, its approved hosts on HTTPS, never the registries), no DNS, all
//!   capabilities dropped, read-only root, a small `/tmp`, memory and process limits, the pinned copy read-only at
//!   `/connector`. The secrets are passed as `-e NAME` (the value comes from podman's environment, never its command
//!   line): `<NAME>_USERNAME` and `<NAME>_PASSWORD` / `_TOKEN` / `_CODE` (secrets.rs `env_names`). A code is taken
//!   from memory for this run and gone.
//! - **MCP** (`kind = "mcp"`, the default): newline-delimited JSON-RPC on stdin/stdout: `initialize`,
//!   `notifications/initialized`, then the one request (`tools/list` at approval, `tools/call` for each call). A
//!   **script** (`kind = "script"`): its one tool `run`, the arguments as JSON on stdin, its standard output the result.
//! - **Blanking**: the result (every string of it), the error text and the standard error's tail kept as the last
//!   run's message go through secrets.rs' `Blanker` (all the agent's secret values, the codes too) before anything is
//!   returned, stored or logged.
//! - **Schedule**: `daily HH:MM`, `weekly mon HH:MM`, `hourly`, `every N minutes` (≥ 15), in the person's zone. A
//!   mechanical one (no model call) is run by the bridge (`scheduler`); else the agent puts the call in one of its
//!   routines.
//! - **Icon**: the site's apple-touch-icon or favicon, fetched from inside the same kind of container (so through the
//!   strict proxy, the connector's sites only), ≤ 256 KB, kept next to the pinned copy.
//!
//! No Python counterpart.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use chrono::{Datelike, TimeZone, Timelike};
use chrono_tz::Tz;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use zeroize::Zeroizing;

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::connectors::{self, Connector};
use crate::secrets::{self, Blanker};
use crate::{py, Dict};

pub const MAX_FILES: usize = 3000;
pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// A call's limit (Hermes waits a bit more: connectors.rs `ENTRY_TIMEOUT`).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_LIMIT: u64 = 8 * 1024 * 1024;
const STDERR_KEPT: usize = 16 * 1024;
pub const ICON_MAX: usize = 256 * 1024;
const MOUNT: &str = "/connector";
/// The connector's private state folder in its runs (its library's session tokens, caches).
pub const STATE: &str = "/state";

/// The variables that point a run's home and caches at its private state (libraries keep their tokens there:
/// garth/garminconnect's `GARMINTOKENS`, anything under `~` or the XDG folders).
pub fn state_env() -> [(&'static str, String); 6] {
    [
        ("HOME", STATE.to_string()),
        ("BIP_STATE", STATE.to_string()),
        ("XDG_CONFIG_HOME", format!("{STATE}/.config")),
        ("XDG_CACHE_HOME", format!("{STATE}/.cache")),
        ("XDG_DATA_HOME", format!("{STATE}/.local/share")),
        ("GARMINTOKENS", format!("{STATE}/garmin")),
    ]
}

// --- pinned files --------------------------------------------------------------------------------------------------

fn walk(root: &Path, relative: &Path, out: &mut Vec<(String, PathBuf, bool)>, bytes: &mut u64) -> Result<(), String> {
    let folder = root.join(relative);
    let mut entries: Vec<_> = std::fs::read_dir(&folder)
        .map_err(|e| format!("{} can't be read ({e})", relative.display()))?
        .flatten()
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name();
        let rel = relative.join(&name);
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        let shown = rel.to_string_lossy().into_owned();
        if meta.file_type().is_symlink() {
            return Err(format!("{shown} is a link: a connector's folder holds plain files only"));
        }
        if meta.is_dir() {
            walk(root, &rel, out, bytes)?;
        } else if meta.is_file() {
            use std::os::unix::fs::PermissionsExt;
            *bytes += meta.len();
            if out.len() >= MAX_FILES || *bytes > MAX_BYTES {
                return Err(format!(
                    "the folder is too big (at most {MAX_FILES} files and {} MB)",
                    MAX_BYTES / 1024 / 1024
                ));
            }
            out.push((shown, entry.path(), meta.permissions().mode() & 0o111 != 0));
        } else {
            return Err(format!("{shown} is not a plain file"));
        }
    }
    Ok(())
}

fn files_of(folder: &Path) -> Result<Vec<(String, PathBuf, bool)>, String> {
    if !folder.is_dir() {
        return Err("the folder isn't there".into());
    }
    let mut out = Vec::new();
    let mut bytes = 0;
    walk(folder, Path::new(""), &mut out, &mut bytes)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The folder's SHA-256: its sorted paths, each with its executable bit and its content's hash.
pub fn fingerprint(folder: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for (path, file, exec) in files_of(folder)? {
        let content = std::fs::read(&file).map_err(|e| format!("{path} can't be read ({e})"))?;
        hasher.update(path.as_bytes());
        hasher.update([0, u8::from(exec)]);
        hasher.update(Sha256::digest(&content));
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

/// `source` copied to `target` (replaced), readable by the container: the copy's fingerprint.
pub fn snapshot(source: &Path, target: &Path) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let files = files_of(source)?;
    let staging = target.with_extension("part");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    for (path, file, exec) in &files {
        let to = staging.join(path);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let content = std::fs::read(file).map_err(|e| format!("{path} can't be read ({e})"))?;
        std::fs::write(&to, content).map_err(|e| e.to_string())?;
        let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(if *exec { 0o755 } else { 0o644 }));
    }
    let _ = std::fs::remove_dir_all(target);
    std::fs::rename(&staging, target).map_err(|e| e.to_string())?;
    fingerprint(target)
}

/// The connector's folder in the agent's sandbox, on this machine (None: not there now).
pub fn workspace(config: &Config, agent: &AgentConfig, folder: &str) -> Option<PathBuf> {
    crate::sent_files::sandbox_folder(config, agent, folder)
}

/// The agent's folder's fingerprint now (None: the folder is gone; Err: it can't be pinned).
pub async fn workspace_fingerprint(config: &Config, agent: &AgentConfig, folder: &str) -> Option<Result<String, String>> {
    let path = workspace(config, agent, folder)?;
    tokio::task::spawn_blocking(move || fingerprint(&path)).await.ok()
}

// --- schedule ------------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum When {
    Hourly,
    Every(u32),
    Daily(u32, u32),
    Weekly(chrono::Weekday, u32, u32),
}

const DAYS: [(&str, chrono::Weekday, &str); 7] = [
    ("mon", chrono::Weekday::Mon, "lundis"),
    ("tue", chrono::Weekday::Tue, "mardis"),
    ("wed", chrono::Weekday::Wed, "mercredis"),
    ("thu", chrono::Weekday::Thu, "jeudis"),
    ("fri", chrono::Weekday::Fri, "vendredis"),
    ("sat", chrono::Weekday::Sat, "samedis"),
    ("sun", chrono::Weekday::Sun, "dimanches"),
];

fn clock(text: &str) -> Result<(u32, u32), String> {
    let (h, m) = text.split_once(':').ok_or("a time is written HH:MM")?;
    let (h, m) = (h.parse::<u32>().map_err(|_| "HH:MM")?, m.parse::<u32>().map_err(|_| "HH:MM")?);
    if h > 23 || m > 59 {
        return Err("a time is written HH:MM, 00:00 to 23:59".into());
    }
    Ok((h, m))
}

/// `daily 07:00`, `weekly mon 07:00`, `hourly`, `every 30 minutes`.
pub fn parse_when(text: &str) -> Result<When, String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["hourly"] => Ok(When::Hourly),
        ["daily", at] => clock(at).map(|(h, m)| When::Daily(h, m)),
        ["weekly", day, at] => {
            let day = DAYS.iter().find(|(k, _, _)| day.starts_with(k)).ok_or("a day is mon, tue, wed… sun")?.1;
            clock(at).map(|(h, m)| When::Weekly(day, h, m))
        }
        ["every", n, unit] if unit.starts_with("min") => match n.parse::<u32>() {
            Ok(n) if (15..=1440).contains(&n) => Ok(When::Every(n)),
            _ => Err("every N minutes: N from 15 to 1440".into()),
        },
        _ => Err("schedule.when: « daily 07:00 », « weekly mon 07:00 », « hourly » or « every 30 minutes »".into()),
    }
}

/// What the person reads.
pub fn when_text(when: &When) -> String {
    let at = |h: u32, m: u32| format!("{h}:{m:02}");
    match when {
        When::Hourly => "Toutes les heures".into(),
        When::Every(n) => format!("Toutes les {n} minutes"),
        When::Daily(h, m) => format!("Tous les jours à {}", at(*h, *m)),
        When::Weekly(day, h, m) => {
            let name = DAYS.iter().find(|(_, d, _)| d == day).map(|(_, _, n)| *n).unwrap_or("");
            format!("Tous les {name} à {}", at(*h, *m))
        }
    }
}

fn local(zone: Tz, date: chrono::NaiveDate, h: u32, m: u32) -> Option<chrono::DateTime<Tz>> {
    zone.from_local_datetime(&date.and_hms_opt(h, m, 0)?).earliest()
}

/// The first time after `after` it runs.
pub fn next_after(when: &When, after: chrono::DateTime<Tz>) -> chrono::DateTime<Tz> {
    let zone = after.timezone();
    match when {
        When::Every(n) => after + chrono::Duration::minutes(i64::from(*n)),
        When::Hourly => {
            let top = after.with_minute(0).and_then(|t| t.with_second(0)).and_then(|t| t.with_nanosecond(0));
            top.unwrap_or(after) + chrono::Duration::hours(1)
        }
        When::Daily(h, m) | When::Weekly(_, h, m) => {
            for days in 0..9 {
                let date = after.date_naive() + chrono::Duration::days(days);
                if let When::Weekly(day, _, _) = when {
                    if date.weekday() != *day {
                        continue;
                    }
                }
                if let Some(at) = local(zone, date, *h, *m).filter(|at| *at > after) {
                    return at;
                }
            }
            after + chrono::Duration::days(1)
        }
    }
}

// --- the container -------------------------------------------------------------------------------------------------

/// Why a call didn't run, or failed.
#[derive(Debug, Clone, PartialEq)]
pub enum RunError {
    Unknown,
    NotApproved,
    Off,
    Changed,
    NoSandbox(String),
    MissingSecret(String),
    Timeout,
    /// Blanked already.
    Failed(String),
}

impl RunError {
    /// For the agent (the tool's result).
    pub fn agent_text(&self) -> String {
        match self {
            RunError::Unknown => "This connector doesn't exist any more.".into(),
            RunError::NotApproved => "This connector waits for the person's approval in the app: it doesn't run yet.".into(),
            RunError::Off => "The person turned this connector off in the app.".into(),
            RunError::Changed => "This connector's files changed since the person approved them: it doesn't run until \
                                  they approve again (a card was raised; call connector_update if you meant to change it)."
                .into(),
            RunError::NoSandbox(why) => format!("Connectors run only in your isolated space, unavailable now ({why})."),
            RunError::MissingSecret(name) => format!(
                "The secret « {name} » isn't in the vault: ask for it with ask_secret (the person types it in the app)."
            ),
            RunError::Timeout => format!("The connector didn't answer within {} seconds.", CALL_TIMEOUT.as_secs()),
            RunError::Failed(why) => format!("The connector failed: {why}"),
        }
    }

    /// For the person (the connector's last run).
    pub fn person_text(&self) -> String {
        match self {
            RunError::Unknown => "Connecteur introuvable.".into(),
            RunError::NotApproved => "En attente de ton accord.".into(),
            RunError::Off => "Coupé.".into(),
            RunError::Changed => "Modifié depuis ton accord : à réapprouver.".into(),
            RunError::NoSandbox(_) => "Son espace isolé n'est pas disponible pour le moment.".into(),
            RunError::MissingSecret(name) => format!("Le secret « {name} » n'est pas dans le coffre."),
            RunError::Timeout => "Le service n'a pas répondu à temps.".into(),
            RunError::Failed(why) => format!("Erreur : {}", crate::browsers::pyx::head(why, 200)),
        }
    }
}

/// The image the connector runs in: the agent's sandbox image, else the sandboxes' default.
pub fn image_for(config: &Config, agent: &AgentConfig) -> String {
    let own = crate::shares::profile_dir(config, agent)
        .map(|folder| crate::shares::profile_config(&folder))
        .and_then(|p| p.get("terminal")?.get("docker_image")?.as_str().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty());
    match own {
        Some(image) if image.starts_with("nousresearch/") || image.starts_with("nikolaik/") => format!("docker.io/{image}"),
        Some(image) => image,
        None => crate::sandbox::proxy_image(config),
    }
}

/// The variables a run gets: (name, value). Never logged.
pub type Env = Vec<(String, Zeroizing<String>)>;

/// `podman run` for connector `conn` of `agent`: the variables named, never their values. `mount`: its pinned files.
pub fn container_args(
    config: &Config,
    agent: &AgentConfig,
    conn: &Connector,
    podman: &str,
    env: &Env,
    mount: bool,
    command: &[String],
) -> Vec<String> {
    let name = format!("bipconn-{}-{}", conn.id.trim_start_matches("c_"), &py::uuid4_hex()[..8]);
    let mut args: Vec<String> = [
        podman,
        "run",
        "--rm",
        "-i",
        "--name",
        &name,
        "--label",
        "bipagents.role=connector",
        "--label",
        &format!("bipagents.agent={}", agent.name),
        "--network",
        crate::sandbox::CONNECTOR_NETWORK,
        "--dns=127.0.0.1",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,nosuid,nodev,size=64m",
        "--memory",
        "256m",
        "--pids-limit",
        "128",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if mount {
        let code = connectors::code_dir(config, &agent.name, &conn.id);
        args.extend(["-v".into(), format!("{}:{MOUNT}:ro,z", code.display()), "-w".into(), MOUNT.into()]);
        // its private state (a library's session tokens…): mounted for its runs only, never in the agent's space
        let state = connectors::state_dir(config, &agent.name, &conn.id);
        args.extend(["-v".into(), format!("{}:{STATE}:rw,z", state.display())]);
        for (key, value) in state_env() {
            args.extend(["-e".into(), format!("{key}={value}")]);
        }
    } else {
        args.extend(["-e".into(), "HOME=/tmp".into()]);
    }
    for (key, _) in env {
        args.extend(["-e".into(), key.clone()]);
    }
    args.push(image_for(config, agent));
    args.extend(command.iter().cloned());
    args
}

/// The proxy variables of a run (its own credentials on the strict side of the proxy).
fn proxy_env(config: &Config, conn: &Connector) -> Result<Env, RunError> {
    let ip = crate::sandbox::connector_proxy_ip(&config.sandbox.subnet)
        .ok_or_else(|| RunError::NoSandbox("no valid [sandbox] subnet".into()))?;
    let url = Zeroizing::new(format!(
        "http://{}:{}@{ip}:{}",
        connectors::proxy_name(&conn.id),
        conn.proxy_token,
        crate::sandbox::PROXY_PORT
    ));
    let mut env: Env = Vec::new();
    for key in ["HTTPS_PROXY", "HTTP_PROXY", "https_proxy", "http_proxy"] {
        env.push((key.into(), url.clone()));
    }
    for key in ["NO_PROXY", "no_proxy"] {
        env.push((key.into(), Zeroizing::new("localhost,127.0.0.1".into())));
    }
    Ok(env)
}

/// A child process stopped for good: its stdin closed, a moment to end, then its container removed and it killed.
async fn stop(child: &mut tokio::process::Child, podman: &str, args: &[String]) {
    if tokio::time::timeout(Duration::from_secs(3), child.wait()).await.is_err() {
        if let Some(at) = args.iter().position(|a| a == "--name") {
            if let Some(name) = args.get(at + 1) {
                let _ = tokio::process::Command::new(podman)
                    .args(["rm", "-f", "--ignore", name])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
            }
        }
        let _ = child.kill().await;
    }
}

fn spawn(args: &[String], env: &Env) -> Result<tokio::process::Child, RunError> {
    let mut command = tokio::process::Command::new(&args[0]);
    command.args(&args[1..]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for (key, value) in env {
        command.env(key, value.as_str());
    }
    command.spawn().map_err(|e| RunError::NoSandbox(format!("podman can't be started: {e}")))
}

fn stderr_task(child: &mut tokio::process::Child) -> tokio::task::JoinHandle<String> {
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        let Some(mut stderr) = stderr else { return String::new() };
        let mut kept: Vec<u8> = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > STDERR_KEPT * 2 {
                kept.drain(..kept.len() - STDERR_KEPT);
            }
        }
        String::from_utf8_lossy(&kept).into_owned()
    })
}

/// The last lines of the standard error, blanked.
fn tail_of(stderr: &str, blank: &Blanker) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(3);
    crate::browsers::pyx::tail(&blank.text(&lines[start..].join(" / ")), 300).to_string()
}

async fn write_line(stdin: &mut tokio::process::ChildStdin, message: &Value) -> Result<(), RunError> {
    let line = format!("{message}\n");
    stdin.write_all(line.as_bytes()).await.map_err(|_| RunError::Failed("it closed its input".into()))?;
    stdin.flush().await.map_err(|_| RunError::Failed("it closed its input".into()))
}

async fn read_response<R: tokio::io::AsyncBufRead + Unpin>(lines: &mut tokio::io::Lines<R>, id: i64) -> Result<Value, RunError> {
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else { continue };
                if message.get("id").and_then(Value::as_i64) == Some(id) && message.get("method").is_none() {
                    return Ok(message);
                }
            }
            Ok(None) => return Err(RunError::Failed("it stopped before answering".into())),
            Err(_) => return Err(RunError::Failed("its answer can't be read".into())),
        }
    }
}

/// One MCP request to the connector's server, in its container: the response's `result` (not blanked yet).
async fn mcp_session(args: &[String], env: &Env, method: &str, params: Value) -> Result<(Value, String), RunError> {
    let mut child = spawn(args, env)?;
    let errors = stderr_task(&mut child);
    let mut stdin = child.stdin.take().ok_or_else(|| RunError::Failed("no input".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| RunError::Failed("no output".into()))?;
    let mut lines = BufReader::new(stdout.take(OUTPUT_LIMIT)).lines();
    let talk = async {
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "bipagents-bridge", "version": crate::VERSION}}});
        write_line(&mut stdin, &init).await?;
        let first = read_response(&mut lines, 1).await?;
        if let Some(error) = first.get("error") {
            return Err(RunError::Failed(format!("it refused to start ({})", error.get("message").map(py::py_str).unwrap_or_default())));
        }
        write_line(&mut stdin, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        write_line(&mut stdin, &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params})).await?;
        let answer = read_response(&mut lines, 2).await?;
        if let Some(error) = answer.get("error") {
            return Err(RunError::Failed(error.get("message").map(py::py_str).unwrap_or_else(|| "an error".into())));
        }
        Ok(answer.get("result").cloned().unwrap_or(json!({})))
    };
    let outcome = tokio::time::timeout(CALL_TIMEOUT, talk).await;
    drop(stdin);
    stop(&mut child, &args[0], args).await;
    let stderr = errors.await.unwrap_or_default();
    match outcome {
        Ok(Ok(result)) => Ok((result, stderr)),
        Ok(Err(RunError::Failed(why))) => Err(RunError::Failed(format!("{why}{}", if stderr.trim().is_empty() { String::new() } else { format!(" — {}", stderr.trim()) }))),
        Ok(Err(other)) => Err(other),
        Err(_) => Err(RunError::Timeout),
    }
}

/// A script connector's run: the arguments on stdin, its output the result.
async fn script_session(args: &[String], env: &Env, input: &Value) -> Result<(Value, String), RunError> {
    let mut child = spawn(args, env)?;
    let errors = stderr_task(&mut child);
    let mut stdin = child.stdin.take().ok_or_else(|| RunError::Failed("no input".into()))?;
    let stdout = child.stdout.take().ok_or_else(|| RunError::Failed("no output".into()))?;
    let work = async {
        let _ = stdin.write_all(format!("{input}\n").as_bytes()).await;
        drop(stdin);
        let mut out = Vec::new();
        let _ = stdout.take(256 * 1024).read_to_end(&mut out).await;
        let status = child.wait().await.ok();
        (out, status)
    };
    let outcome = tokio::time::timeout(CALL_TIMEOUT, work).await;
    let stderr = match &outcome {
        Ok(_) => errors.await.unwrap_or_default(),
        Err(_) => {
            stop(&mut child, &args[0], args).await;
            errors.await.unwrap_or_default()
        }
    };
    let Ok((out, status)) = outcome else { return Err(RunError::Timeout) };
    let failed = !status.is_some_and(|s| s.success());
    let text = String::from_utf8_lossy(&out).into_owned();
    let text = if text.trim().is_empty() && failed { stderr.clone() } else { text };
    Ok((json!({"content": [{"type": "text", "text": text}], "isError": failed}), stderr))
}

/// A run's secrets: its variables, the names used, and the codes taken (for blanking).
type RunSecrets = (Env, Vec<String>, Vec<Zeroizing<String>>);

/// The secrets' variables of a run, the names used, and the codes taken (for blanking).
fn secrets_env(state: &AppState, agent: &str, conn: &Connector) -> Result<RunSecrets, RunError> {
    let kept = state.vault.secret_values(agent).map_err(|_| RunError::Failed("the vault can't be read".into()))?;
    let items = state.vault.secret_items(agent);
    let mut env: Env = Vec::new();
    let mut used = Vec::new();
    let mut codes = Vec::new();
    for name in &conn.secrets {
        let item = items.iter().find(|i| &i.label == name);
        let kind = item
            .and_then(|i| i.secret_info.as_ref())
            .and_then(|s| s.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("password")
            .to_string();
        let (user_var, value_var) = secrets::env_names(name, &kind);
        if let Some(identifier) = item.and_then(|i| i.identifier.clone()).filter(|i| !i.is_empty()) {
            env.push((user_var, Zeroizing::new(identifier)));
        }
        if kind == "code" {
            if let Some(code) = secrets::take_code(agent, name) {
                env.push((value_var, code.clone()));
                codes.push(code);
            }
            continue;
        }
        let Some((_, _, value)) = kept.iter().find(|(n, _, _)| n == name) else {
            return Err(RunError::MissingSecret(name.clone()));
        };
        env.push((value_var, value.clone()));
        used.push(name.clone());
    }
    Ok((env, used, codes))
}

/// Everything checked before a run: the connector, its approval, its files (the copy and the agent's folder), the
/// sandbox. The connector and podman's path.
async fn ready(state: &AppState, agent: &AgentConfig, id: &str, need_on: bool) -> Result<(Connector, String), RunError> {
    let config = state.config();
    let conn = connectors::get(&config, &agent.name, id).ok_or(RunError::Unknown)?;
    if conn.status != connectors::APPROVED {
        return Err(RunError::NotApproved);
    }
    if need_on && !conn.enabled {
        return Err(RunError::Off);
    }
    if let Some(Ok(now)) = workspace_fingerprint(&config, agent, &conn.folder).await {
        if now != conn.fingerprint {
            connectors::mark_changed(state, agent, id, &now).await;
            return Err(RunError::Changed);
        }
    }
    let code = connectors::code_dir(&config, &agent.name, id);
    let pinned = tokio::task::spawn_blocking(move || fingerprint(&code)).await.ok().and_then(Result::ok);
    if pinned.as_deref() != Some(conn.fingerprint.as_str()) {
        connectors::mark_changed(state, agent, id, &conn.fingerprint).await;
        return Err(RunError::Changed);
    }
    if !crate::sandbox::isolates(&config, agent) {
        return Err(RunError::NoSandbox("the sandbox isn't isolated on this server".into()));
    }
    let podman = crate::sandbox::engine(&config).ok_or_else(|| RunError::NoSandbox("no podman".into()))?;
    crate::sandbox::network(&config, &crate::agents_admin::default_run()).await.map_err(RunError::NoSandbox)?;
    Ok((conn, podman))
}

/// A tool of connector `id` called (by the agent through its MCP entry, or by the schedule): its MCP result, blanked.
pub async fn call_tool(
    state: &AppState,
    agent: &AgentConfig,
    id: &str,
    tool: &str,
    arguments: Dict,
    via: &str,
) -> Result<Value, RunError> {
    let outcome = run(state, agent, id, tool, arguments).await;
    let config = state.config();
    let (ok, message) = match &outcome {
        Ok(result) => {
            let failed = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
            let first = result
                .get("content")
                .and_then(Value::as_array)
                .and_then(|c| c.iter().find_map(|i| i.get("text").and_then(Value::as_str)))
                .unwrap_or("")
                .trim()
                .to_string();
            if failed {
                (false, format!("Erreur : {}", crate::browsers::pyx::head(&first, 200)))
            } else {
                (true, "OK".to_string())
            }
        }
        Err(err) => (false, err.person_text()),
    };
    if !matches!(outcome, Err(RunError::Unknown)) {
        connectors::set_last_run(&config, &agent.name, id, ok, &message, via);
    }
    tracing::info!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, tool = %tool, via = %via, ok, "connector called");
    outcome
}

async fn run(state: &AppState, agent: &AgentConfig, id: &str, tool: &str, arguments: Dict) -> Result<Value, RunError> {
    let (conn, podman) = ready(state, agent, id, true).await?;
    let config = state.config();
    let (secret_env, used, codes) = secrets_env(state, &agent.name, &conn)?;
    let mut env = proxy_env(&config, &conn)?;
    env.extend(secret_env);
    let blank = secrets::blanker_for(state, &agent.name, &codes);
    let command = entry_argv(&conn).map_err(RunError::Failed)?;
    let args = container_args(&config, agent, &conn, &podman, &env, true, &command);
    let outcome = if conn.kind == "script" {
        script_session(&args, &env, &Value::Object(arguments)).await
    } else {
        mcp_session(&args, &env, "tools/call", json!({"name": tool, "arguments": arguments})).await
    };
    drop(env);
    let _ = state.vault.touch_secrets(&agent.name, &used).await;
    match outcome {
        Ok((result, stderr)) => {
            if !stderr.trim().is_empty() {
                tracing::debug!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, stderr = %tail_of(&stderr, &blank), "connector's messages");
            }
            Ok(blank.value(result))
        }
        Err(RunError::Failed(why)) => Err(RunError::Failed(blank.text(&why))),
        Err(other) => Err(other),
    }
}

/// The connector's command, split as a shell would (no shell runs it).
pub fn entry_argv(conn: &Connector) -> Result<Vec<String>, String> {
    let argv = shlex::split(&conn.entry).filter(|a| !a.is_empty()).ok_or("entry: a command like « python3 server.py »")?;
    Ok(argv)
}

/// Its tools, asked of its server once approved (a script: its one tool). Blanked; None when it didn't answer.
pub async fn discover(state: &AppState, agent: &AgentConfig, id: &str) -> Option<Vec<Value>> {
    let (conn, podman) = ready(state, agent, id, false).await.ok()?;
    if conn.kind == "script" {
        return Some(vec![json!({"name": "run", "description": conn.description,
                                "inputSchema": {"type": "object", "additionalProperties": true}})]);
    }
    let config = state.config();
    let (secret_env, _, codes) = secrets_env(state, &agent.name, &conn).unwrap_or_default();
    let mut env = proxy_env(&config, &conn).ok()?;
    env.extend(secret_env);
    let blank = secrets::blanker_for(state, &agent.name, &codes);
    let args = container_args(&config, agent, &conn, &podman, &env, true, &entry_argv(&conn).ok()?);
    let (result, _) = mcp_session(&args, &env, "tools/list", json!({})).await.ok()?;
    let tools = blank.value(result).get("tools")?.as_array()?.clone();
    Some(tools.into_iter().filter(|t| t.get("name").and_then(Value::as_str).is_some_and(|n| !n.is_empty())).take(64).collect())
}

// --- the schedule ----------------------------------------------------------------------------------------------------

/// The mechanical schedules, run by the bridge (no model call): checked every 30 seconds.
pub async fn scheduler(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(30)).await;
        let config = state.config();
        let zone = crate::dates::load(&config).zone();
        let now = chrono::Utc::now().with_timezone(&zone);
        for (agent, conn) in connectors::all(&config) {
            let Some(schedule) = conn.schedule.as_ref().filter(|s| s.mechanical && !s.tool.is_empty()) else { continue };
            if conn.status != connectors::APPROVED || !conn.enabled {
                continue;
            }
            let Ok(when) = parse_when(&schedule.when) else { continue };
            let base = if conn.last_scheduled > 0.0 { conn.last_scheduled } else { continue };
            let Some(after) = chrono::DateTime::from_timestamp(base as i64, 0) else { continue };
            if now < next_after(&when, after.with_timezone(&zone)) {
                continue;
            }
            connectors::set_scheduled(&config, &agent, &conn.id, py::time_time());
            let Some(agent_cfg) = config.agent(Some(&agent)).cloned() else { continue };
            let arguments = schedule.arguments.as_object().cloned().unwrap_or_default();
            let (state, tool, id) = (state.clone(), schedule.tool.clone(), conn.id.clone());
            tokio::spawn(async move {
                let _ = call_tool(&state, &agent_cfg, &id, &tool, arguments, "schedule").await;
            });
        }
    }
}

// --- the icon --------------------------------------------------------------------------------------------------------

const FETCH: &str = r#"
import base64, json, sys, urllib.request
for url in sys.argv[1:]:
    try:
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (BipAgents icon)"})
        with urllib.request.urlopen(req, timeout=15) as r:
            data = r.read(524289)[:524288]
            print(json.dumps({"url": url, "status": r.status, "type": r.headers.get("content-type", ""),
                              "b64": base64.b64encode(data).decode()}), flush=True)
    except Exception as e:
        print(json.dumps({"url": url, "error": str(e)[:200]}), flush=True)
"#;

/// The icons a page names, best first: apple-touch-icon, then the others (PNG first), then the usual paths.
pub fn icon_candidates(page: &str, base: &str) -> Vec<String> {
    static LINK: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<link\b[^>]*>").expect("valid regex"));
    static ATTR: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?is)\b(rel|href|type)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#).expect("valid regex")
    });
    let Ok(base_url) = url::Url::parse(base) else { return Vec::new() };
    let mut found: Vec<(u8, String)> = Vec::new();
    for link in LINK.find_iter(page) {
        let (mut rel, mut href, mut kind) = (String::new(), String::new(), String::new());
        for cap in ATTR.captures_iter(link.as_str()) {
            let value = cap.get(2).or(cap.get(3)).or(cap.get(4)).map(|m| m.as_str().to_string()).unwrap_or_default();
            match cap[1].to_lowercase().as_str() {
                "rel" => rel = value.to_lowercase(),
                "href" => href = value,
                _ => kind = value.to_lowercase(),
            }
        }
        if href.is_empty() || href.starts_with("data:") {
            continue;
        }
        let rank = if rel.contains("apple-touch-icon") {
            0
        } else if rel.split_whitespace().any(|r| r == "icon") {
            if kind.contains("png") || href.to_lowercase().ends_with(".png") { 1 } else if kind.contains("svg") || href.ends_with(".svg") { 9 } else { 2 }
        } else {
            continue;
        };
        if rank == 9 {
            continue; // SVG: not a format the apps show as an image here
        }
        if let Ok(url) = base_url.join(&href) {
            if url.scheme() == "https" {
                found.push((rank, url.to_string()));
            }
        }
    }
    found.sort_by_key(|(rank, _)| *rank);
    let mut out: Vec<String> = found.into_iter().map(|(_, u)| u).collect();
    for path in ["/apple-touch-icon.png", "/favicon.ico"] {
        if let Ok(url) = base_url.join(path) {
            out.push(url.to_string());
        }
    }
    out.dedup();
    out.truncate(6);
    out
}

async fn fetch(config: &Config, agent: &AgentConfig, conn: &Connector, podman: &str, urls: &[String]) -> Vec<Value> {
    let Ok(env) = proxy_env(config, conn) else { return Vec::new() };
    let mut command: Vec<String> = vec!["python3".into(), "-c".into(), FETCH.into()];
    command.extend(urls.iter().cloned());
    let args = container_args(config, agent, conn, podman, &env, false, &command);
    let Ok(mut child) = spawn(&args, &env) else { return Vec::new() };
    drop(child.stdin.take());
    let Some(stdout) = child.stdout.take() else { return Vec::new() };
    let mut out = Vec::new();
    let read = async {
        let _ = stdout.take(4 * 1024 * 1024).read_to_end(&mut out).await;
    };
    let _ = tokio::time::timeout(Duration::from_secs(60), read).await;
    stop(&mut child, podman, &args).await;
    String::from_utf8_lossy(&out).lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect()
}

fn image_of(answer: &Value) -> Option<Vec<u8>> {
    let data = base64::engine::general_purpose::STANDARD.decode(answer.get("b64")?.as_str()?).ok()?;
    (!data.is_empty() && data.len() <= ICON_MAX && !crate::favicons::image_type(&data).is_empty()).then_some(data)
}

/// The site's icon fetched and kept (nothing when it can't be had: the apps show the symbol).
pub async fn fetch_icon(state: &AppState, agent: &AgentConfig, id: &str) {
    let Ok((conn, podman)) = ready(state, agent, id, false).await else { return };
    let Some(host) = conn.sites.iter().find(|s| !s.starts_with("*.")).cloned() else { return };
    let config = state.config();
    let home = format!("https://{host}/");
    let page = fetch(&config, agent, &conn, &podman, std::slice::from_ref(&home)).await;
    let html = page
        .first()
        .and_then(|a| base64::engine::general_purpose::STANDARD.decode(a.get("b64")?.as_str()?).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let candidates = icon_candidates(&html, &home);
    for answer in fetch(&config, agent, &conn, &podman, &candidates).await {
        if let Some(data) = image_of(&answer) {
            connectors::save_icon(&config, &agent.name, id, &data);
            tracing::info!(target: "bipbridge::connectors", agent = %agent.name, connector = %id, bytes = data.len(), "connector's icon kept");
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_pinned_by_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("lib")).unwrap();
        std::fs::write(src.join("server.py"), "print(1)").unwrap();
        std::fs::write(src.join("lib/a.py"), "x = 1").unwrap();
        let first = fingerprint(&src).unwrap();
        let copy = dir.path().join("copy");
        assert_eq!(snapshot(&src, &copy).unwrap(), first);
        std::fs::write(src.join("lib/a.py"), "x = 2").unwrap();
        assert_ne!(fingerprint(&src).unwrap(), first);
        std::fs::write(src.join("lib/a.py"), "x = 1").unwrap();
        assert_eq!(fingerprint(&src).unwrap(), first);
        std::fs::rename(src.join("lib/a.py"), src.join("lib/b.py")).unwrap();
        assert_ne!(fingerprint(&src).unwrap(), first);
        std::os::unix::fs::symlink("/etc/passwd", src.join("x")).unwrap();
        assert!(fingerprint(&src).unwrap_err().contains("is a link"));
    }

    #[test]
    fn schedules() {
        let zone: Tz = "Europe/Paris".parse().unwrap();
        let at = zone.with_ymd_and_hms(2026, 10, 28, 8, 0, 0).unwrap(); // a Wednesday
        assert_eq!(next_after(&parse_when("daily 07:00").unwrap(), at), zone.with_ymd_and_hms(2026, 10, 29, 7, 0, 0).unwrap());
        assert_eq!(next_after(&parse_when("daily 09:30").unwrap(), at), zone.with_ymd_and_hms(2026, 10, 28, 9, 30, 0).unwrap());
        assert_eq!(next_after(&parse_when("weekly mon 07:00").unwrap(), at), zone.with_ymd_and_hms(2026, 11, 2, 7, 0, 0).unwrap());
        assert_eq!(next_after(&parse_when("hourly").unwrap(), at), zone.with_ymd_and_hms(2026, 10, 28, 9, 0, 0).unwrap());
        assert_eq!(next_after(&parse_when("every 30 minutes").unwrap(), at), zone.with_ymd_and_hms(2026, 10, 28, 8, 30, 0).unwrap());
        assert!(parse_when("every 5 minutes").is_err() && parse_when("daily 25:00").is_err() && parse_when("tomorrow").is_err());
        assert_eq!(when_text(&parse_when("daily 07:00").unwrap()), "Tous les jours à 7:00");
        assert_eq!(when_text(&parse_when("weekly mon 18:05").unwrap()), "Tous les lundis à 18:05");
    }

    #[test]
    fn icons_found_in_a_page() {
        let page = r#"<head><link rel="icon" href="/fav.ico"><link rel="stylesheet" href="/a.css">
            <LINK REL='apple-touch-icon' HREF='https://connect.garmin.com/touch.png'><link rel="icon" type="image/png" href="i.png"></head>"#;
        let found = icon_candidates(page, "https://connect.garmin.com/");
        assert_eq!(found[0], "https://connect.garmin.com/touch.png");
        assert_eq!(found[1], "https://connect.garmin.com/i.png");
        assert_eq!(found[2], "https://connect.garmin.com/fav.ico");
        assert!(found.contains(&"https://connect.garmin.com/favicon.ico".to_string()));
        let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\nxxxx");
        assert!(image_of(&json!({"b64": png})).is_some());
        assert!(image_of(&json!({"b64": base64::engine::general_purpose::STANDARD.encode(b"<html>")})).is_none());
    }

    #[test]
    fn plain_words_for_both() {
        assert!(RunError::Changed.person_text().contains("réapprouver"));
        assert!(RunError::MissingSecret("garmin".into()).agent_text().contains("ask_secret"));
    }
}
