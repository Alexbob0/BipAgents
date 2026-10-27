//! The place's space encrypted at rest (« level 1 », internal/space-encryption.md, private): everything personal of this account
//! (Hermes' home, the bridge's config and data, the backup key, the browsers' profiles…) lives in one gocryptfs
//! folder. Its key is never on this server's disk: it is two shares put together (XOR), one kept here (useless alone),
//! the other kept by Kerfy's key service at another provider (keys-rs/), which hands it to this place only, with the
//! place's own token, and logs each time. After a reboot the bridge starts « locked »: its door and /health answer, the
//! rest says 423; it fetches the other share, opens the folder, starts the agents' services, and runs itself again
//! (exec) as the normal bridge. Nobody has anything to do.
//!
//! What it protects against, and what not (the design note says it in full): a copy of the disk (snapshots, images,
//! a disk taken out, backups of the volume) read away from this server opens nothing. Kerfy Studio, which runs the key
//! service, could put both shares together; anyone reading the running server's memory, or booting it with its own
//! code and asking the key service from its address, too (that last one is logged and refused from another address).
//!
//!     ~/.local/share/bipagents/space/       outside the encrypted folder ($BIPAGENTS_SPACE_DIR)
//!         space.json        Settings: on or off, the folders, the door, the services to start (0600)
//!         share             this server's share, base64url (0600)
//!         keyservice.json   {url, token, grant, signature}: where the other share is and the place's proofs (0600)
//!         locked.json       {since, reason, tries} while locked (the seat agent tells the guichet « locked »)
//!         cipher/           the encrypted folder (gocryptfs)
//!         open/             where it is opened: open/.bipagents-space shows it is
//!         rollback/         the unencrypted originals, only during `space enable` (or with --keep-rollback)
//!
//!     bipbridge space status | enable | unlock | lock | rollback | finish     (`bipagents encrypt` runs these)

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::py;

/// The file inside the opened folder that shows it is open (the agents' services wait for it: their drop-in's
/// `ConditionPathExists`).
pub const MARKER: &str = ".bipagents-space";
pub const DIR_ENV: &str = "BIPAGENTS_SPACE_DIR";
/// Tests only: the « fake » backend (no encryption) is refused unless this is set.
pub const FAKE_ENV: &str = "BIPAGENTS_SPACE_FAKE";
/// What stays outside in the bridge's data folder: its binaries, the voices, the door's certificate, this folder.
pub const KEEP_OUT: [&str; 5] = ["bridge", "voices", "lan", "space", "migrations.json"];
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::URL_SAFE_NO_PAD;
const SHARE_LEN: usize = 32;
/// Waits between tries to reach the key service while locked (then every five minutes).
const BACKOFF: [u64; 6] = [5, 10, 20, 40, 80, 160];
const BACKOFF_MAX: u64 = 300;

pub fn default_dir() -> PathBuf {
    match std::env::var(DIR_ENV) {
        Ok(d) if !d.is_empty() => PathBuf::from(py::expand(&d)),
        _ => PathBuf::from(py::expanduser("~/.local/share/bipagents/space")),
    }
}

fn now() -> f64 {
    py::time_time()
}

// -- settings --------------------------------------------------------------------------------------

/// The door while locked (the normal bridge's own, from its config at `space enable`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Door {
    pub host: String,
    pub port: u16,
    pub lan: Option<LanDoor>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LanDoor {
    pub host: String,
    pub port: u16,
    pub cert: String,
    pub key: String,
}

/// A folder moved into the encrypted one: `path` is now a link to `open/<name>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Item {
    pub name: String,
    pub path: String,
}

/// The bridge's data folder: its entries (but [`KEEP_OUT`]) moved to `open/data`, the config's `data_dir` pointed
/// there, and a link left for each folder among them (the installer knows some of them by their old path).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DataMove {
    pub old: String,
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub v: u32,
    pub enabled: bool,
    /// « gocryptfs » (« fake » in tests only).
    pub backend: String,
    /// gocryptfs' `-allow_other` (the agents' browser containers run as other user ids; needs `user_allow_other` in
    /// /etc/fuse.conf). gocryptfs then also turns on `default_permissions`: the files' own modes are checked.
    pub allow_other: bool,
    /// Started once the folder is open (`systemctl --user start …`), stopped before it is closed.
    pub units: Vec<String>,
    pub door: Door,
    pub items: Vec<Item>,
    pub data: Option<DataMove>,
    /// The unencrypted originals, kept until the first unlock worked (`space enable` checks it at once).
    pub rollback: Option<String>,
    pub enabled_at: f64,
}

/// Where the other share is, and how this place proves itself: its own token (the guichet's place token, the one its
/// backups use) and, the first time, the guichet's signed grant for it.
#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct KeyService {
    pub url: String,
    pub token: String,
    pub grant: Option<String>,
    pub signature: Option<String>,
}

impl std::fmt::Debug for KeyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyService").field("url", &self.url).finish_non_exhaustive()
    }
}

pub struct Space {
    pub dir: PathBuf,
    pub settings: Settings,
}

fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    py::write_with_mode(&tmp.to_string_lossy(), data, 0o600)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Space {
    pub fn new(dir: PathBuf) -> Space {
        Space { dir, settings: Settings { v: 1, backend: "gocryptfs".into(), ..Settings::default() } }
    }

    /// None when this account has no space settings (never encrypted).
    pub fn load(dir: &Path) -> Result<Option<Space>> {
        let path = dir.join("space.json");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
        };
        let settings: Settings =
            serde_json::from_str(&text).with_context(|| format!("{} is not valid", path.display()))?;
        Ok(Some(Space { dir: dir.to_path_buf(), settings }))
    }

    pub fn save(&self) -> Result<()> {
        py::makedirs(&self.dir.to_string_lossy(), 0o700)?;
        write_private(&self.dir.join("space.json"), serde_json::to_string_pretty(&self.settings)?.as_bytes())
    }

    pub fn cipher(&self) -> PathBuf {
        self.dir.join("cipher")
    }

    pub fn mount(&self) -> PathBuf {
        self.dir.join("open")
    }

    pub fn rollback_dir(&self) -> PathBuf {
        self.dir.join("rollback")
    }

    /// The folder is open (its marker is only there then: the mount point is empty otherwise).
    pub fn is_open(&self) -> bool {
        self.mount().join(MARKER).exists()
    }

    pub fn locked(&self) -> Option<Value> {
        std::fs::read_to_string(self.dir.join("locked.json")).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    pub fn set_locked(&self, reason: &str, tries: u64, since: f64) {
        let body = json!({"since": since, "reason": reason, "tries": tries});
        if let Err(e) = write_private(&self.dir.join("locked.json"), body.to_string().as_bytes()) {
            tracing::warn!(error = %e, "space: locked state not written");
        }
    }

    pub fn clear_locked(&self) {
        let _ = std::fs::remove_file(self.dir.join("locked.json"));
    }

    pub fn server_share(&self) -> Result<Zeroizing<Vec<u8>>> {
        let text =
            Zeroizing::new(std::fs::read_to_string(self.dir.join("share")).context("this server's share is missing")?);
        let share =
            Zeroizing::new(B64.decode(text.trim()).map_err(|_| anyhow!("this server's share is not base64url"))?);
        if share.len() != SHARE_LEN {
            bail!("this server's share has the wrong length");
        }
        Ok(share)
    }

    pub fn key_service(&self) -> Result<KeyService> {
        let text = Zeroizing::new(
            std::fs::read_to_string(self.dir.join("keyservice.json"))
                .context("the key service's settings are missing")?,
        );
        Ok(serde_json::from_str(&text)?)
    }

    /// What `space status` prints and /health tells.
    pub fn status(&self) -> Value {
        let state = if !self.settings.enabled {
            "off"
        } else if self.is_open() {
            "on"
        } else {
            "locked"
        };
        json!({
            "state": state, "backend": self.settings.backend, "rollback": self.settings.rollback.is_some(),
            "locked": self.locked(), "units": self.settings.units,
        })
    }
}

/// The folder's password: both shares put together (XOR), base64url. Each share alone says nothing of it.
pub fn password(server: &[u8], service: &[u8]) -> Result<Zeroizing<String>> {
    if server.len() != SHARE_LEN || service.len() != SHARE_LEN {
        bail!("a share has the wrong length");
    }
    let mut key = Zeroizing::new([0u8; SHARE_LEN]);
    for i in 0..SHARE_LEN {
        key[i] = server[i] ^ service[i];
    }
    Ok(Zeroizing::new(B64.encode(key.as_slice())))
}

pub fn new_share() -> Zeroizing<Vec<u8>> {
    let mut share = Zeroizing::new(vec![0u8; SHARE_LEN]);
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, share.as_mut_slice());
    share
}

// -- the other share -------------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    /// No answer (network, the service down): tried again.
    #[error("key service unreachable: {0}")]
    Unreachable(String),
    /// The service said no (stopped for this place, another address, too many tries): tried again, more slowly.
    #[error("key service refused: {0}")]
    Refused(String),
}

#[async_trait::async_trait]
pub trait ShareSource: Send + Sync {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError>;
}

/// Kerfy's key service (keys-rs/): `POST {url}/v1/share` with the place's token, the grant the first time.
pub struct HttpShare {
    pub service: KeyService,
    pub client: reqwest::Client,
}

impl HttpShare {
    pub fn new(service: KeyService) -> Result<HttpShare> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .https_only(!service.url.starts_with("http://127.0.0.1") && !service.url.starts_with("http://localhost"))
            .build()?;
        Ok(HttpShare { service, client })
    }
}

#[async_trait::async_trait]
impl ShareSource for HttpShare {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        let url = format!("{}/v1/share", self.service.url.trim_end_matches('/'));
        let body = json!({"grant": self.service.grant, "signature": self.service.signature});
        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.service.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ShareError::Unreachable(e.without_url().to_string()))?;
        let status = response.status();
        let text =
            Zeroizing::new(response.text().await.map_err(|e| ShareError::Unreachable(e.without_url().to_string()))?);
        if status.is_server_error() {
            return Err(ShareError::Unreachable(format!("HTTP {}", status.as_u16())));
        }
        let data: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            let detail = data.get("detail").and_then(Value::as_str).unwrap_or("no reason given");
            return Err(ShareError::Refused(format!("HTTP {}: {detail}", status.as_u16())));
        }
        let share = data
            .get("share")
            .and_then(Value::as_str)
            .and_then(|s| B64.decode(s).ok())
            .filter(|s| s.len() == SHARE_LEN)
            .ok_or_else(|| ShareError::Refused("the answer holds no share".into()))?;
        Ok(Zeroizing::new(share))
    }
}

// -- the encrypted folder --------------------------------------------------------------------------

#[async_trait::async_trait]
pub trait Mounter: Send + Sync {
    /// What it needs is installed (else a sentence saying what to install).
    fn check(&self) -> Result<()>;
    async fn init(&self, cipher: &Path, password: &str) -> Result<()>;
    async fn mount(&self, cipher: &Path, mount: &Path, password: &str, allow_other: bool) -> Result<()>;
    async fn unmount(&self, mount: &Path) -> Result<()>;
}

pub fn mounter(backend: &str) -> Result<Box<dyn Mounter>> {
    match backend {
        "gocryptfs" | "" => Ok(Box::new(Gocryptfs)),
        "fake" if std::env::var(FAKE_ENV).map(|v| v == "1").unwrap_or(false) => Ok(Box::new(FakeMounter)),
        other => bail!("unknown space backend {other:?}"),
    }
}

fn which(name: &str) -> Option<String> {
    crate::spaces::which(name).map(|p| p.to_string_lossy().into_owned())
}

/// gocryptfs (FUSE, no root): any filesystem (ext4 on the offer's Ubuntu, btrfs on Fedora), file contents and names
/// encrypted (AES-256-GCM, EME), sizes and the tree's shape visible. Its master key is in gocryptfs.conf, sealed with
/// the password (scrypt): the password is a 256-bit key, so scrypt's cost is kept low.
///
/// Opened under systemd (a transient unit of its own, `bipagents-space-open`), so restarting the bridge leaves it
/// open; the password goes through a FIFO in $XDG_RUNTIME_DIR (memory), never a file or a command line.
pub struct Gocryptfs;

async fn run_with_stdin(program: &str, args: &[&str], input: &str) -> Result<std::process::Output> {
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("can't run {program}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let line = Zeroizing::new(format!("{input}\n"));
        stdin.write_all(line.as_bytes()).await?;
        drop(stdin);
    }
    let output = tokio::time::timeout(Duration::from_secs(120), child.wait_with_output())
        .await
        .map_err(|_| anyhow!("{program} took too long"))??;
    Ok(output)
}

fn tail(output: &std::process::Output) -> String {
    let text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let text = if text.is_empty() { String::from_utf8_lossy(&output.stdout).trim().to_string() } else { text };
    text.chars().rev().take(300).collect::<Vec<_>>().into_iter().rev().collect()
}

#[async_trait::async_trait]
impl Mounter for Gocryptfs {
    fn check(&self) -> Result<()> {
        if which("gocryptfs").is_none() {
            bail!("gocryptfs is not installed (Ubuntu/Debian: sudo apt install gocryptfs; Fedora: sudo dnf install gocryptfs)");
        }
        if which("fusermount3").is_none() && which("fusermount").is_none() {
            bail!("FUSE is not installed (Ubuntu/Debian: sudo apt install fuse3; Fedora: sudo dnf install fuse3)");
        }
        Ok(())
    }

    async fn init(&self, cipher: &Path, password: &str) -> Result<()> {
        let cipher_s = cipher.to_string_lossy();
        let output = run_with_stdin("gocryptfs", &["-init", "-q", "-scryptn", "10", &cipher_s], password).await?;
        if !output.status.success() {
            bail!("gocryptfs -init failed: {}", tail(&output));
        }
        Ok(())
    }

    async fn mount(&self, cipher: &Path, mount: &Path, password: &str, allow_other: bool) -> Result<()> {
        let (cipher_s, mount_s) = (cipher.to_string_lossy().to_string(), mount.to_string_lossy().to_string());
        let mut options = vec!["-q".to_string()];
        if allow_other {
            options.push("-allow_other".into());
        }
        let runtime = std::env::var("XDG_RUNTIME_DIR").ok().filter(|d| Path::new(d).is_dir());
        if let (Some(runtime), Some(systemd_run)) = (runtime, which("systemd-run")) {
            // A unit of its own: the bridge restarting (an update) leaves the folder open.
            let fifo = Path::new(&runtime).join(format!("bipagents-space-{}.fifo", std::process::id()));
            let _ = std::fs::remove_file(&fifo);
            let c_path = std::ffi::CString::new(fifo.to_string_lossy().as_bytes())?;
            // SAFETY: a valid C string; mkfifo only creates the node.
            if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } != 0 {
                bail!("can't make the password's pipe: {}", std::io::Error::last_os_error());
            }
            let _ = tokio::process::Command::new("systemctl")
                .args(["--user", "reset-failed", "bipagents-space-open.service"])
                .output()
                .await;
            let fifo_s = fifo.to_string_lossy().to_string();
            let mut args: Vec<String> = vec![
                "--user".into(),
                "--quiet".into(),
                "--collect".into(),
                "--unit=bipagents-space-open".into(),
                "--description=BipAgents: the encrypted space, open".into(),
                "--".into(),
                which("gocryptfs").unwrap_or_else(|| "gocryptfs".into()),
                "-fg".into(),
                "-passfile".into(),
                fifo_s.clone(),
            ];
            args.extend(options);
            args.push(cipher_s);
            args.push(mount_s);
            let started = tokio::process::Command::new(&systemd_run).args(&args).output().await;
            let result = async {
                let started = started.context("can't run systemd-run")?;
                if !started.status.success() {
                    bail!("systemd-run failed: {}", tail(&started));
                }
                // The reader is there now (or soon): the password, then the pipe goes.
                let pass = Zeroizing::new(format!("{password}\n"));
                let fifo_w = fifo.clone();
                tokio::time::timeout(
                    Duration::from_secs(20),
                    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                        use std::io::Write;
                        let mut fh = std::fs::OpenOptions::new().write(true).open(&fifo_w)?;
                        fh.write_all(pass.as_bytes())
                    }),
                )
                .await
                .map_err(|_| anyhow!("gocryptfs did not read its password"))???;
                Ok(())
            }
            .await;
            let _ = std::fs::remove_file(&fifo);
            result?;
        } else {
            let mut args: Vec<&str> = options.iter().map(String::as_str).collect();
            args.push(&cipher_s);
            args.push(&mount_s);
            let output = run_with_stdin("gocryptfs", &args, password).await?;
            if !output.status.success() {
                bail!("gocryptfs failed (code {:?}): {}", output.status.code(), tail(&output));
            }
        }
        // Under systemd, gocryptfs mounts once it has read the password: waited for here.
        for _ in 0..100 {
            if mount.join(MARKER).exists() || mount_point_active(mount) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("the folder did not open (wrong key, or gocryptfs stopped: journalctl --user -u bipagents-space-open)")
    }

    async fn unmount(&self, mount: &Path) -> Result<()> {
        let program = which("fusermount3").or_else(|| which("fusermount")).unwrap_or_else(|| "fusermount3".into());
        let output = tokio::process::Command::new(&program).arg("-u").arg(mount).output().await?;
        if !output.status.success() && mount.join(MARKER).exists() {
            bail!("can't close the folder (still in use?): {}", tail(&output));
        }
        Ok(())
    }
}

/// Tests only (no encryption at all): the « cipher » folder holds the files under `store/` and the password's
/// SHA-256; opening moves `store/` to the mount point, closing moves it back.
pub struct FakeMounter;

fn fake_check(password: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(password.as_bytes()))
}

#[async_trait::async_trait]
impl Mounter for FakeMounter {
    fn check(&self) -> Result<()> {
        Ok(())
    }

    async fn init(&self, cipher: &Path, password: &str) -> Result<()> {
        std::fs::create_dir_all(cipher.join("store"))?;
        std::fs::write(cipher.join("fake.conf"), fake_check(password))?;
        Ok(())
    }

    async fn mount(&self, cipher: &Path, mount: &Path, password: &str, _allow_other: bool) -> Result<()> {
        if std::fs::read_to_string(cipher.join("fake.conf"))? != fake_check(password) {
            bail!("wrong key");
        }
        if mount.exists() {
            std::fs::remove_dir(mount).context("the mount point is not empty")?;
        }
        std::fs::rename(cipher.join("store"), mount)?;
        Ok(())
    }

    async fn unmount(&self, mount: &Path) -> Result<()> {
        let cipher = mount.parent().ok_or_else(|| anyhow!("no parent"))?.join("cipher");
        std::fs::rename(mount, cipher.join("store"))?;
        std::fs::create_dir_all(mount)?;
        Ok(())
    }
}

// -- unlock, lock ----------------------------------------------------------------------------------

/// Both shares, then the folder opened. Nothing written to disk.
pub async fn unlock(space: &Space, source: &dyn ShareSource, mounter: &dyn Mounter) -> Result<()> {
    if space.is_open() {
        return Ok(());
    }
    let server = space.server_share()?;
    let service = source.share().await?;
    let pass = password(&server, &service)?;
    std::fs::create_dir_all(space.mount())?;
    mounter.mount(&space.cipher(), &space.mount(), &pass, space.settings.allow_other).await?;
    if !space.is_open() {
        let _ = mounter.unmount(&space.mount()).await;
        bail!("the folder opened without its marker");
    }
    Ok(())
}

/// Runs a command (systemctl), replaceable in tests.
pub type Exec = Arc<dyn Fn(Vec<String>) -> futures::future::BoxFuture<'static, Result<()>> + Send + Sync>;

pub fn systemctl() -> Exec {
    Arc::new(|args: Vec<String>| {
        Box::pin(async move {
            let output = tokio::process::Command::new("systemctl").args(&args).output().await?;
            if !output.status.success() {
                bail!("systemctl {}: {}", args.join(" "), tail(&output));
            }
            Ok(())
        })
    })
}

pub async fn start_units(space: &Space, exec: &Exec) -> Result<()> {
    if space.settings.units.is_empty() {
        return Ok(());
    }
    let mut args = vec!["--user".to_string(), "start".to_string(), "--no-block".to_string()];
    args.extend(space.settings.units.iter().cloned());
    exec(args).await
}

pub async fn stop_units(space: &Space, exec: &Exec) -> Result<()> {
    if space.settings.units.is_empty() {
        return Ok(());
    }
    let mut args = vec!["--user".to_string(), "stop".to_string()];
    args.extend(space.settings.units.iter().cloned());
    exec(args).await
}

/// The agents' services stopped, the folder closed (`space lock`; also before a rollback).
pub async fn lock(space: &Space, mounter: &dyn Mounter, exec: &Exec) -> Result<()> {
    stop_units(space, exec).await?;
    if space.is_open() {
        mounter.unmount(&space.mount()).await?;
    }
    space.set_locked("closed by hand", 0, now());
    Ok(())
}

// -- the locked bridge -----------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LockedState {
    pub since: f64,
    pub reason: String,
    pub tries: u64,
}

/// What a locked bridge answers: /health says so, everything else 423 (the apps keep their data and wait).
pub fn locked_router(state: Arc<Mutex<LockedState>>) -> Router {
    let health_state = state.clone();
    Router::new()
        .route(
            "/health",
            get(move || {
                let s = health_state.lock().map(|s| s.clone()).unwrap_or_default();
                async move {
                    Json(json!({
                        "ok": false, "version": crate::VERSION, "space": "locked", "reason": s.reason,
                        "since": s.since, "features": ["space"],
                    }))
                }
            }),
        )
        .fallback(|_request: Request| async {
            (StatusCode::LOCKED, Json(json!({"detail": "space locked", "space": "locked"}))).into_response()
        })
}

/// Tests replace the share source, the backend, systemctl and what happens once open (the exec).
pub struct LockedDeps {
    pub source: Box<dyn ShareSource>,
    pub mounter: Box<dyn Mounter>,
    pub exec: Exec,
    /// The unit of [`BACKOFF`]'s waits: a second (tests: a few milliseconds).
    pub pace: Duration,
}

impl LockedDeps {
    pub fn real(space: &Space) -> Result<LockedDeps> {
        Ok(LockedDeps {
            source: Box::new(HttpShare::new(space.key_service()?)?),
            mounter: mounter(&space.settings.backend)?,
            exec: systemctl(),
            pace: Duration::from_secs(1),
        })
    }
}

/// Tries until the folder is open: the key service first (a few seconds after a reboot the network may not be up),
/// then the waits of [`BACKOFF`]. The state for /health and the seat agent at each try. Returns false when stopped.
pub async fn unlock_loop(
    space: &Space,
    deps: &LockedDeps,
    state: &Arc<Mutex<LockedState>>,
    stop: &CancellationToken,
) -> bool {
    let since = now();
    let mut tries = 0u64;
    loop {
        tries += 1;
        match unlock(space, deps.source.as_ref(), deps.mounter.as_ref()).await {
            Ok(()) => {
                space.clear_locked();
                tracing::info!(tries, "space: open");
                if let Err(e) = start_units(space, &deps.exec).await {
                    tracing::warn!(error = %format!("{e:#}"), "space: services not started");
                }
                return true;
            }
            Err(e) => {
                let reason = match e.downcast_ref::<ShareError>() {
                    Some(ShareError::Unreachable(_)) => "key service unreachable".to_string(),
                    Some(ShareError::Refused(why)) => format!("key service refused: {why}"),
                    None => format!("{e:#}"),
                };
                tracing::warn!(tries, reason = %reason, "space: still locked");
                space.set_locked(&reason, tries, since);
                if let Ok(mut s) = state.lock() {
                    *s = LockedState { since, reason, tries };
                }
            }
        }
        let wait = BACKOFF.get(tries as usize - 1).copied().unwrap_or(BACKOFF_MAX);
        tokio::select! {
            _ = stop.cancelled() => return false,
            _ = tokio::time::sleep(deps.pace * wait as u32) => {}
        }
    }
}

/// `bipbridge serve` on an encrypted space that is not open: the locked door until the folder opens, then the
/// normal bridge (this same program, run again). None: not encrypted, or open (the normal bridge starts).
pub fn serve_if_locked() -> Option<i32> {
    let dir = default_dir();
    let space = match Space::load(&dir) {
        Ok(Some(s)) if s.settings.enabled => s,
        Ok(_) => return None,
        Err(e) => {
            eprintln!("space settings: {e:#}");
            return Some(2);
        }
    };
    if space.is_open() {
        space.clear_locked();
        // The bridge started again while the folder stayed open (an update): its services may be waiting.
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
            let _ = runtime.block_on(start_units(&space, &systemctl()));
        }
        return None;
    }
    crate::logs::setup_logging("info");
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return Some(1);
        }
    };
    let deps = match LockedDeps::real(&space) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "space: can't unlock");
            space.set_locked(&format!("{e:#}"), 0, now());
            // Still serve the locked door: the guichet sees « locked » and why, not an outage.
            LockedDeps {
                source: Box::new(Never),
                mounter: Box::new(FakeMounterRefused),
                exec: systemctl(),
                pace: Duration::from_secs(1),
            }
        }
    };
    let code = runtime.block_on(serve_locked(space, deps));
    if code != 0 {
        return Some(code);
    }
    Some(run_again())
}

struct Never;

#[async_trait::async_trait]
impl ShareSource for Never {
    async fn share(&self) -> Result<Zeroizing<Vec<u8>>, ShareError> {
        Err(ShareError::Refused("this place's key service settings can't be read".into()))
    }
}

struct FakeMounterRefused;

#[async_trait::async_trait]
impl Mounter for FakeMounterRefused {
    fn check(&self) -> Result<()> {
        bail!("no backend")
    }
    async fn init(&self, _: &Path, _: &str) -> Result<()> {
        bail!("no backend")
    }
    async fn mount(&self, _: &Path, _: &Path, _: &str, _: bool) -> Result<()> {
        bail!("no backend")
    }
    async fn unmount(&self, _: &Path) -> Result<()> {
        Ok(())
    }
}

/// The normal bridge, in this same process (same PID for systemd): the open folder is found and it starts.
fn run_again() -> i32 {
    use std::os::unix::process::CommandExt;
    let program = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("can't find this program to run it again: {e}");
            return 75; // systemd starts it again (Restart=on-failure)
        }
    };
    let error = std::process::Command::new(program).args(std::env::args_os().skip(1)).exec();
    eprintln!("can't run the bridge again: {error}");
    75
}

/// The locked door (the main listener, and the LAN door when the bridge has one) until the folder is open; 0 then,
/// 1 when stopped (SIGTERM) first, 3 when a door can't listen.
pub async fn serve_locked(space: Space, deps: LockedDeps) -> i32 {
    let state = Arc::new(Mutex::new(LockedState { since: now(), reason: "opening".into(), tries: 0 }));
    let stop = CancellationToken::new();
    let tasks = match open_doors(&space.settings.door, locked_router(state.clone()), stop.clone()).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "space: the locked door can't listen");
            return 3;
        }
    };
    tracing::info!("space: locked, opening it");
    let opened = tokio::select! {
        opened = unlock_loop(&space, &deps, &state, &stop) => opened,
        _ = crate::main_cli::shutdown_signal() => false,
    };
    stop.cancel();
    for task in tasks {
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }
    if opened {
        0
    } else {
        1
    }
}

/// The door's listeners with `router`, until `stop`.
pub async fn open_doors(
    door: &Door,
    router: Router,
    stop: CancellationToken,
) -> Result<Vec<tokio::task::JoinHandle<()>>> {
    let mut tasks = Vec::new();
    let listener = tokio::net::TcpListener::bind((door.host.as_str(), door.port)).await?;
    let service = router.clone().into_make_service_with_connect_info::<std::net::SocketAddr>();
    let main_stop = stop.clone();
    tasks.push(tokio::spawn(async move {
        let _ = axum::serve(listener, service).with_graceful_shutdown(main_stop.cancelled_owned()).await;
    }));
    if let Some(lan) = &door.lan {
        let acceptor = crate::main_cli::tls_acceptor(&lan.cert, &lan.key)?;
        let listener = tokio::net::TcpListener::bind((lan.host.as_str(), lan.port)).await?;
        tasks.push(crate::main_cli::serve_door_on(listener, router, acceptor, stop));
    }
    Ok(tasks)
}

// -- enable (an existing space encrypted in place) -------------------------------------------------

pub struct EnableOptions {
    pub config_path: String,
    pub home: PathBuf,
    pub key_service: KeyService,
    pub units: Vec<String>,
    pub allow_other: bool,
    pub keep_rollback: bool,
    pub backend: String,
}

#[derive(Debug, Default, Serialize)]
pub struct Tally {
    pub files: u64,
    pub bytes: u64,
}

/// Files and bytes under `path` (links not followed: a link counts as a file). The config's lock and last good copy
/// aren't counted: rewriting the config (the switch) makes them.
pub fn tally(path: &Path) -> Tally {
    let mut t = Tally::default();
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    if name.ends_with(&format!(".toml{}", crate::config::LOCK_SUFFIX))
        || name.ends_with(&format!(".toml{}", crate::config::LAST_GOOD_SUFFIX))
    {
        return t;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else { return t };
    if meta.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let sub = tally(&entry.path());
                t.files += sub.files;
                t.bytes += sub.bytes;
            }
        }
    } else {
        t.files += 1;
        if meta.is_file() {
            t.bytes += meta.len();
        }
    }
    t
}

fn free_bytes(path: &Path) -> Option<u64> {
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a zeroed statvfs to fill.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// `path`, or its nearest existing parent, resolved (links followed).
fn resolved(path: &Path) -> PathBuf {
    let mut probe = path.to_path_buf();
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&probe) {
            let mut out = real;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (probe.file_name().map(|n| n.to_os_string()), probe.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                probe = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

fn inside(path: &Path, folder: &Path) -> bool {
    resolved(path).starts_with(resolved(folder))
}

/// What moves into the folder: Hermes' home, the config's folder, ~/.local/state/bipagents, and the data folder's
/// entries (but [`KEEP_OUT`]). Refused when something needed while locked would end up inside.
pub fn plan(
    config: &crate::config::Config,
    config_path: &str,
    home: &Path,
    space_dir: &Path,
) -> Result<(Vec<Item>, Option<DataMove>)> {
    let mut items = Vec::new();
    let hermes_home = if config.hermes.home.is_empty() {
        home.join(".hermes")
    } else {
        PathBuf::from(py::expand(&config.hermes.home))
    };
    let config_dir = Path::new(&py::expand(config_path))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("no config folder"))?;
    let state_dir = home.join(".local/state/bipagents");
    for (name, path) in [("hermes", &hermes_home), ("config", &config_dir), ("state", &state_dir)] {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) if name == "state" => continue,
            Err(e) => bail!("{} can't be read: {e}", path.display()),
        };
        if !meta.is_dir() {
            bail!("{} is not a folder (already a link?)", path.display());
        }
        items.push(Item { name: name.into(), path: path.to_string_lossy().into() });
    }
    let data_dir = PathBuf::from(config.data_dir());
    let mut data = None;
    if data_dir.is_dir() {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&data_dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if KEEP_OUT.contains(&name.as_str()) || entry.path() == space_dir {
                continue;
            }
            entries.push(name);
        }
        entries.sort();
        data = Some(DataMove { old: data_dir.to_string_lossy().into(), entries });
    }
    // What the locked bridge needs stays outside; what holds someone's data can't be outside of it.
    let cert_dir = PathBuf::from(&config.lan.cert_dir);
    for item in &items {
        let p = Path::new(&item.path);
        if inside(space_dir, p) {
            bail!("the space's own folder ({}) is inside {}", space_dir.display(), p.display());
        }
        if config.lan.enabled && inside(&cert_dir, p) {
            bail!("the door's certificate ({}) is inside {}: it must stay outside", cert_dir.display(), p.display());
        }
        if inside(&data_dir, p) || inside(p, &data_dir) {
            bail!("the data folder and {} overlap: not handled", p.display());
        }
    }
    Ok((items, data))
}

/// Paths of the config that hold someone's data and resolve outside the open folder (none after a good enable).
pub fn outside_paths(config: &crate::config::Config, mount: &Path) -> Vec<String> {
    let mut paths = vec![
        config.outbox.db_path.clone(),
        config.outbox.audio_dir.clone(),
        config.spaces.root.clone(),
        config.browser.root.clone(),
    ];
    if !config.hermes.home.is_empty() {
        paths.push(py::expand(&config.hermes.home));
    }
    for (k, v) in &config.media_roots {
        paths.push(k.clone());
        paths.push(v.clone());
    }
    for agent in config.agents.values() {
        if let Some(p) = &agent.upload_dir_host {
            paths.push(p.clone());
        }
    }
    paths.into_iter().filter(|p| !p.is_empty() && !inside(Path::new(p), mount)).collect()
}

/// `data_dir` set in the config's text (and the door's certificate folder kept where it is), other lines untouched.
pub fn rewrite_config(text: &str, data_dir: &str, cert_dir: Option<&str>) -> String {
    let quoted = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut out = Vec::new();
    let mut in_table = false;
    let mut data_done = false;
    let mut in_lan = false;
    let mut cert_done = cert_dir.is_none();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            if !in_table && !data_done {
                out.push(format!("data_dir = {}", quoted(data_dir)));
                data_done = true;
            }
            if in_lan && !cert_done {
                out.push(format!("cert_dir = {}", quoted(cert_dir.unwrap_or_default())));
                cert_done = true;
            }
            in_table = true;
            in_lan = trimmed.starts_with("[lan]");
            out.push(line.to_string());
            continue;
        }
        let key = trimmed.split('=').next().unwrap_or("").trim();
        if !in_table && key == "data_dir" {
            out.push(format!("data_dir = {}", quoted(data_dir)));
            data_done = true;
            continue;
        }
        if in_lan && key == "cert_dir" {
            out.push(format!("cert_dir = {}", quoted(cert_dir.unwrap_or_default())));
            cert_done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !data_done {
        out.insert(0, format!("data_dir = {}", quoted(data_dir)));
    }
    if in_lan && !cert_done {
        out.push(format!("cert_dir = {}", quoted(cert_dir.unwrap_or_default())));
        cert_done = true;
    }
    if !cert_done {
        out.push(String::new());
        out.push("[lan]".into());
        out.push(format!("cert_dir = {}", quoted(cert_dir.unwrap_or_default())));
    }
    out.join("\n") + "\n"
}

async fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let output =
        tokio::process::Command::new("cp").arg("-a").arg(from).arg(to).output().await.context("can't run cp")?;
    if !output.status.success() {
        bail!("copying {} failed: {}", from.display(), tail(&output));
    }
    Ok(())
}

fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).with_context(|| format!("can't link {}", link.display()))
}

/// Puts the originals back from `rollback/` (links removed, the folder closed): the space as before `enable`.
pub async fn rollback(space: &mut Space, mounter: &dyn Mounter, exec: &Exec) -> Result<()> {
    let Some(kept) = space.settings.rollback.clone() else { bail!("no unencrypted copy is kept: nothing to put back") };
    let kept = PathBuf::from(kept);
    let _ = stop_units(space, exec).await;
    for item in &space.settings.items {
        let path = Path::new(&item.path);
        let original = kept.join(&item.name);
        if !original.exists() {
            continue;
        }
        if std::fs::symlink_metadata(path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&original, path)?;
    }
    if let Some(data) = &space.settings.data {
        for entry in &data.entries {
            let path = Path::new(&data.old).join(entry);
            let original = kept.join("data").join(entry);
            if !original.exists() && std::fs::symlink_metadata(&original).is_err() {
                continue;
            }
            if std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
                std::fs::remove_file(&path)?;
            }
            std::fs::rename(&original, &path)?;
        }
    }
    if space.is_open() {
        mounter.unmount(&space.mount()).await?;
    }
    let _ = std::fs::remove_dir_all(&kept);
    let _ = std::fs::remove_dir_all(space.cipher());
    let _ = std::fs::remove_file(space.dir.join("share"));
    space.settings.enabled = false;
    space.settings.rollback = None;
    space.clear_locked();
    space.save()?;
    Ok(())
}

/// The unencrypted originals deleted (`space finish`). Deleted, not wiped: on an SSD or a provider's volume, their
/// blocks may stay readable until overwritten (the design note says so).
pub fn finish(space: &mut Space) -> Result<()> {
    if let Some(kept) = space.settings.rollback.take() {
        std::fs::remove_dir_all(&kept).or_else(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        })?;
        space.save()?;
    }
    Ok(())
}

/// An existing space encrypted in place, with its services stopped (`bipagents encrypt` stops them, and the bridge):
/// the other share from the key service (the place enrolled), this server's made, the folder made and opened, each
/// folder copied in and checked, the originals set aside and replaced by links, the config pointed inside; then the
/// folder closed and opened again from scratch (the key service asked again) before the originals are deleted.
/// Any failure before the end puts everything back as it was.
pub async fn enable(
    opts: &EnableOptions,
    dir: &Path,
    source: &dyn ShareSource,
    mounter: &dyn Mounter,
    exec: &Exec,
) -> Result<Value> {
    if let Some(existing) = Space::load(dir)? {
        if existing.settings.enabled {
            bail!("this space is already encrypted");
        }
    }
    mounter.check()?;
    let config = crate::config::load_config(Some(&opts.config_path)).map_err(|e| anyhow!("{e}"))?;
    if std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::new(config.host.parse().unwrap_or(std::net::IpAddr::from([127, 0, 0, 1])), config.port),
        Duration::from_millis(500),
    )
    .is_ok()
    {
        bail!("the bridge is running: stop it first (bipagents encrypt does)");
    }
    let (items, data) = plan(&config, &opts.config_path, &opts.home, dir)?;
    let mut total = Tally::default();
    for item in &items {
        let t = tally(Path::new(&item.path));
        total.files += t.files;
        total.bytes += t.bytes;
    }
    if let Some(d) = &data {
        for entry in &d.entries {
            let t = tally(&Path::new(&d.old).join(entry));
            total.files += t.files;
            total.bytes += t.bytes;
        }
    }
    py::makedirs(&dir.to_string_lossy(), 0o700)?;
    let needed = total.bytes + total.bytes / 10 + 256 * 1024 * 1024;
    if let Some(free) = free_bytes(dir) {
        if free < needed {
            bail!("not enough free disk: {} MB needed, {} MB free", needed / 1_000_000, free / 1_000_000);
        }
    }

    let mut space = Space::new(dir.to_path_buf());
    space.settings.backend = opts.backend.clone();
    space.settings.allow_other = opts.allow_other;
    space.settings.units = opts.units.clone();
    if space.cipher().exists() || space.rollback_dir().exists() {
        bail!("{} holds a cipher or rollback folder from an earlier try: look at it, then remove it", dir.display());
    }
    let service_share = source.share().await.map_err(|e| anyhow!("{e}"))?;
    let server_share = new_share();
    write_private(&dir.join("share"), B64.encode(server_share.as_slice()).as_bytes())?;
    write_private(&dir.join("keyservice.json"), Zeroizing::new(serde_json::to_string(&opts.key_service)?).as_bytes())?;
    let pass = password(&server_share, &service_share)?;
    drop(service_share);

    // 1. The folder, empty, opened.
    py::makedirs(&space.cipher().to_string_lossy(), 0o700)?;
    std::fs::create_dir_all(space.mount())?;
    let undo_cipher = |space: &Space| {
        let _ = std::fs::remove_dir_all(space.cipher());
        let _ = std::fs::remove_file(space.dir.join("share"));
    };
    if let Err(e) = mounter.init(&space.cipher(), &pass).await {
        undo_cipher(&space);
        return Err(e);
    }
    if let Err(e) = mounter.mount(&space.cipher(), &space.mount(), &pass, opts.allow_other).await {
        undo_cipher(&space);
        return Err(e);
    }
    std::fs::write(space.mount().join(MARKER), b"BipAgents: this folder is the encrypted space, open.\n")?;

    // 2. Copies, checked.
    let copied = async {
        for item in &items {
            copy_tree(Path::new(&item.path), &space.mount().join(&item.name)).await?;
            let (a, b) = (tally(Path::new(&item.path)), tally(&space.mount().join(&item.name)));
            if a.files != b.files || a.bytes != b.bytes {
                bail!(
                    "the copy of {} differs ({} files, {} bytes → {}, {})",
                    item.path,
                    a.files,
                    a.bytes,
                    b.files,
                    b.bytes
                );
            }
        }
        if let Some(d) = &data {
            std::fs::create_dir_all(space.mount().join("data"))?;
            for entry in &d.entries {
                let from = Path::new(&d.old).join(entry);
                let to = space.mount().join("data").join(entry);
                copy_tree(&from, &to).await?;
                let (a, b) = (tally(&from), tally(&to));
                if a.files != b.files || a.bytes != b.bytes {
                    bail!("the copy of {} differs", from.display());
                }
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = copied {
        let _ = mounter.unmount(&space.mount()).await;
        undo_cipher(&space);
        return Err(e);
    }

    // 3. The switch: originals aside, links in their place, the config pointed inside.
    let kept = space.rollback_dir();
    py::makedirs(&kept.to_string_lossy(), 0o700)?;
    space.settings.items = items.clone();
    space.settings.data = data.clone();
    space.settings.rollback = Some(kept.to_string_lossy().into());
    space.settings.enabled = true;
    space.settings.enabled_at = now();
    space.save()?;
    let switched = async {
        for item in &items {
            std::fs::rename(&item.path, kept.join(&item.name))?;
            symlink(&space.mount().join(&item.name), Path::new(&item.path))?;
        }
        if let Some(d) = &data {
            std::fs::create_dir_all(kept.join("data"))?;
            for entry in &d.entries {
                let path = Path::new(&d.old).join(entry);
                let is_dir = std::fs::symlink_metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
                std::fs::rename(&path, kept.join("data").join(entry))?;
                if is_dir {
                    symlink(&space.mount().join("data").join(entry), &path)?;
                }
            }
            let config_file = Path::new(&py::expand(&opts.config_path)).to_path_buf();
            std::fs::metadata(&config_file)?;
            let cert_dir = config.lan.cert_dir.clone();
            let data_dir = space.mount().join("data").to_string_lossy().into_owned();
            crate::config::update_source(&config_file.to_string_lossy(), |text| {
                Ok(rewrite_config(text, &data_dir, Some(&cert_dir)))
            })?;
        }
        let new_config = crate::config::load_config(Some(&opts.config_path)).map_err(|e| anyhow!("{e}"))?;
        let outside = outside_paths(&new_config, &space.mount());
        if !outside.is_empty() {
            bail!("some of the config's folders would stay unencrypted: {}", outside.join(", "));
        }
        let lan = if new_config.lan.enabled {
            let (cert, key) = crate::lan::certificate_paths(&new_config);
            if inside(Path::new(&cert), &space.mount()) {
                bail!("the door's certificate would be inside the encrypted folder");
            }
            Some(LanDoor { host: new_config.lan.host.clone(), port: new_config.lan.port, cert, key })
        } else {
            None
        };
        space.settings.door = Door { host: new_config.host.clone(), port: new_config.port, lan };
        space.save()?;
        Ok(())
    }
    .await;
    if let Err(e) = switched {
        let _ = rollback(&mut space, mounter, exec).await;
        return Err(e.context("encryption undone, everything is as before"));
    }

    // 4. Closed, then opened again from scratch: the key service and this server's share give the same key.
    let inside_now = || {
        let mut t = Tally::default();
        for item in &items {
            let sub = tally(&space.mount().join(&item.name));
            t.files += sub.files;
            t.bytes += sub.bytes;
        }
        if let Some(d) = &data {
            for entry in &d.entries {
                let sub = tally(&space.mount().join("data").join(entry));
                t.files += sub.files;
                t.bytes += sub.bytes;
            }
        }
        t
    };
    let checked = async {
        let before = inside_now(); // the config rewritten: a few bytes more than the originals
        if before.files != total.files {
            bail!("{} files inside instead of {}", before.files, total.files);
        }
        mounter.unmount(&space.mount()).await?;
        if space.is_open() {
            bail!("the folder did not close");
        }
        unlock(&space, source, mounter).await?;
        let again = inside_now();
        if again.files != before.files || again.bytes != before.bytes {
            bail!(
                "after opening again, {} files and {} bytes instead of {} and {}",
                again.files,
                again.bytes,
                before.files,
                before.bytes
            );
        }
        Ok(())
    }
    .await;
    if let Err(e) = checked {
        let _ = rollback(&mut space, mounter, exec).await;
        return Err(e.context("encryption undone, everything is as before"));
    }
    if !opts.keep_rollback {
        finish(&mut space)?;
    }
    space.clear_locked();
    Ok(json!({
        "state": "on", "files": total.files, "bytes": total.bytes,
        "items": items.iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
        "data": data.map(|d| d.entries), "rollback": space.settings.rollback,
    }))
}

/// A FUSE folder is mounted there (Linux: /proc/self/mountinfo); false elsewhere.
fn mount_point_active(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string("/proc/self/mountinfo") else { return false };
    let wanted = resolved(path).to_string_lossy().to_string();
    text.lines().any(|line| line.split(' ').nth(4).map(|m| m.replace("\\040", " ") == wanted).unwrap_or(false))
}

// -- the command line ------------------------------------------------------------------------------

const USAGE: &str = "usage: bipbridge space {status|enable|unlock|lock|rollback|finish} [--config CONFIG]
                       [--unit UNIT]... [--allow-other] [--keep-rollback]

  status     on, locked or off, and why it is locked (JSON)
  enable     encrypt this account's space in place (its services and the bridge stopped first); the key service
             from $BIPAGENTS_KEYS_URL, $BIPAGENTS_KEYS_TOKEN and, the first time, $BIPAGENTS_KEYS_GRANT and
             $BIPAGENTS_KEYS_SIGNATURE (never on the command line)
  unlock     open it now (the key service asked) and start its services
  lock       stop its services and close it
  rollback   put the unencrypted originals back (only while they are kept: --keep-rollback)
  finish     delete the unencrypted originals now
";

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `bipbridge space …`: the exit code.
pub fn cli(argv: &[String]) -> i32 {
    let Some(action) = argv.first().cloned() else {
        eprint!("{USAGE}");
        return 2;
    };
    let mut config = None;
    let mut units = Vec::new();
    let (mut allow_other, mut keep_rollback) = (false, false);
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--config" if i + 1 < argv.len() => {
                config = Some(argv[i + 1].clone());
                i += 1;
            }
            "--unit" if i + 1 < argv.len() => {
                units.push(argv[i + 1].clone());
                i += 1;
            }
            "--allow-other" => allow_other = true,
            "--keep-rollback" => keep_rollback = true,
            "-h" | "--help" => {
                print!("{USAGE}");
                return 0;
            }
            other => {
                eprint!("{USAGE}");
                eprintln!("bipbridge space: unknown option {other}");
                return 2;
            }
        }
        i += 1;
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let dir = default_dir();
    let result: Result<Value> = runtime.block_on(async {
        match action.as_str() {
            "status" => match Space::load(&dir)? {
                Some(space) => Ok(space.status()),
                None => Ok(json!({"state": "off"})),
            },
            "enable" => {
                let key_service = KeyService {
                    url: env("BIPAGENTS_KEYS_URL").ok_or_else(|| anyhow!("BIPAGENTS_KEYS_URL is not set"))?,
                    token: env("BIPAGENTS_KEYS_TOKEN").ok_or_else(|| anyhow!("BIPAGENTS_KEYS_TOKEN is not set"))?,
                    grant: env("BIPAGENTS_KEYS_GRANT"),
                    signature: env("BIPAGENTS_KEYS_SIGNATURE"),
                };
                let backend = env("BIPAGENTS_SPACE_BACKEND").unwrap_or_else(|| "gocryptfs".into());
                let opts = EnableOptions {
                    config_path: config.clone().unwrap_or_else(crate::config::config_path),
                    home: PathBuf::from(py::expanduser("~")),
                    key_service: key_service.clone(),
                    units: units.clone(),
                    allow_other,
                    keep_rollback,
                    backend: backend.clone(),
                };
                let source = HttpShare::new(key_service)?;
                enable(&opts, &dir, &source, mounter(&backend)?.as_ref(), &systemctl()).await
            }
            "unlock" => {
                let space = Space::load(&dir)?
                    .filter(|s| s.settings.enabled)
                    .ok_or_else(|| anyhow!("this space is not encrypted"))?;
                let deps = LockedDeps::real(&space)?;
                unlock(&space, deps.source.as_ref(), deps.mounter.as_ref()).await?;
                space.clear_locked();
                start_units(&space, &deps.exec).await?;
                Ok(space.status())
            }
            "lock" => {
                let space = Space::load(&dir)?
                    .filter(|s| s.settings.enabled)
                    .ok_or_else(|| anyhow!("this space is not encrypted"))?;
                lock(&space, mounter(&space.settings.backend)?.as_ref(), &systemctl()).await?;
                Ok(space.status())
            }
            "rollback" => {
                let mut space = Space::load(&dir)?.ok_or_else(|| anyhow!("this space is not encrypted"))?;
                let backend = space.settings.backend.clone();
                rollback(&mut space, mounter(&backend)?.as_ref(), &systemctl()).await?;
                Ok(space.status())
            }
            "finish" => {
                let mut space = Space::load(&dir)?.ok_or_else(|| anyhow!("this space is not encrypted"))?;
                finish(&mut space)?;
                Ok(space.status())
            }
            other => bail!("unknown action {other}"),
        }
    });
    match result {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("bipbridge space {action}: {e:#}");
            1
        }
    }
}

/// The space's state for the normal bridge's /health: « off » or « on » (a locked bridge answers itself).
pub fn health_state() -> &'static str {
    match Space::load(&default_dir()) {
        Ok(Some(s)) if s.settings.enabled => "on",
        _ => "off",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_make_the_password_and_say_nothing_alone() {
        let a = [7u8; 32];
        let b = [7u8; 32];
        assert_eq!(password(&a, &b).unwrap().as_str(), B64.encode([0u8; 32]));
        assert!(password(&a[..31], &b).is_err());
        let c = new_share();
        assert_eq!(c.len(), 32);
        assert_ne!(c.as_slice(), new_share().as_slice());
    }

    #[test]
    fn config_rewritten_with_its_data_dir_and_the_door_certificate() {
        let text = "# Written by the installer\nbridge_key = \"k\"\ndata_dir = \"/home/a/.local/share/bipagents\"\n\n[lan]\nenabled = true\nport = 443\n\n[agents.vie]\ndisplay_name = \"Vie\"\n";
        let out = rewrite_config(text, "/s/open/data", Some("/home/a/.local/share/bipagents/lan"));
        assert!(out.contains("data_dir = \"/s/open/data\"\n"));
        assert!(!out.contains("data_dir = \"/home"));
        assert!(out.contains(
            "[lan]\nenabled = true\nport = 443\n\ncert_dir = \"/home/a/.local/share/bipagents/lan\"\n[agents.vie]"
        ));
        let parsed: toml::Table = out.parse().unwrap();
        assert_eq!(parsed["data_dir"].as_str(), Some("/s/open/data"));
        assert_eq!(parsed["lan"]["cert_dir"].as_str(), Some("/home/a/.local/share/bipagents/lan"));
        // no data_dir and no [lan]: both added
        let out = rewrite_config("bridge_key = \"k\"\n[agents.a]\nx = 1\n", "/d", Some("/c"));
        let parsed: toml::Table = out.parse().unwrap();
        assert_eq!(parsed["data_dir"].as_str(), Some("/d"));
        assert_eq!(parsed["lan"]["cert_dir"].as_str(), Some("/c"));
        assert_eq!(parsed["agents"]["a"]["x"].as_integer(), Some(1));
    }

    #[test]
    fn tally_counts_files_and_bytes_without_following_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/f"), b"12345").unwrap();
        std::os::unix::fs::symlink("/etc", dir.path().join("a/l")).unwrap();
        let t = tally(&dir.path().join("a"));
        assert_eq!((t.files, t.bytes), (2, 5));
    }
}
