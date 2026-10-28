//! One's own Hermes into this place (installer/bipinstall/to_place.py): the app asks for a one-time code, a computer
//! proves it knows it and sends its export, then the installer puts it in, in place of this place's agents, in a unit of
//! its own (restarting the bridge must not stop it).
//!
//!     POST   /v1/import               {state: waiting, code, address, command, command_alone, expires}   a new code
//!     GET    /v1/import               {state: none | waiting | receiving | importing | done | failed, …}
//!     DELETE /v1/import               the code withdrawn, or a finished import's result cleared
//!     GET    /v1/import/hello?nonce=  {fingerprint, challenge, proof}   the computer (no bridge key: the code instead)
//!     PUT    /v1/import/archive       the export; X-Import-Challenge, X-Import-Proof (an HMAC with the code)
//!
//! A code lasts an hour and serves once (a sending cut short may start again with it). The computer learns this place's
//! certificate from the proof (an HMAC over its nonce, the certificate's fingerprint and a challenge) and pins it before
//! sending anything. Wrong proofs count as wrong keys (guard.py). Only on a place of the all-included offer.
//!
//!     ~/.local/state/bipagents/import/    code.json, the archive and its passphrase, progress.json (the installer's)
//!
//! Port of `bridge/bipbridge/place_import.py` (owner group: browsers-places).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, put};
use axum::Router;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::auth::ClientAddr;
use crate::browsers::pyx;
use crate::cloud_setup::Challenges;
use crate::config::Config;
use crate::logs::J;
use crate::py::Clock;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::Dict;

pub const IMPORT_CONTEXT: &str = "bipagents-import-v1";
pub const UPLOAD_CONTEXT: &str = "bipagents-import-upload-v1";
/// The installer's (to_place.py).
pub const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const CODE_LENGTH: usize = 16;
pub const CODE_SECONDS: f64 = 3600.0;
/// An import still « running » after that stopped half-way (the server restarted).
pub const STALE_SECONDS: f64 = 2.0 * 3600.0;
pub const MAX_BYTES: u64 = 10 * (1 << 30);
/// Left free on the server beyond the export unpacked (about three times its size).
pub const SPARE_BYTES: u64 = 2 * (1 << 30);
pub const FOLDER: &str = ".local/state/bipagents/import";
pub const UNIT: &str = "bipagents-import";
pub const INSTALLER: &str = "https://raw.githubusercontent.com/Alexbob0/BipAgents/main/install.sh";

pub fn proof(code: &str, parts: &[&str]) -> String {
    pyx::hmac_hex(code.as_bytes(), parts)
}

pub fn shown(code: &str) -> String {
    let chars: Vec<char> = code.chars().collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

fn read(path: &Path) -> Dict {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

fn write_private(path: &Path, data: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        crate::py::makedirs(&parent.to_string_lossy(), 0o700)?;
    }
    let tmp = path.with_extension("tmp");
    crate::py::write_with_mode(&tmp.to_string_lossy(), data.as_bytes(), 0o600)?;
    std::fs::rename(&tmp, path)
}

fn float_of(value: Option<&Value>) -> f64 {
    match value.filter(|v| crate::py::truthy(v)) {
        None => 0.0,
        Some(v) => crate::py::py_float(v).unwrap_or(0.0),
    }
}

/// The installer's checkout (the folder with `installer/bipinstall`) and the Python that runs it: the installer's own
/// environment (`installer/.venv`, made by the installer), else the Python bridge's of an install not updated since
/// (`bridge/.venv`), else `python3`. `BIPAGENTS_REPO` says where the checkout is; else the folders above this
/// program, else `~/BipAgents`.
pub fn repo_and_python() -> (PathBuf, String) {
    let has_installer = |p: &Path| p.join("installer").join("bipinstall").is_dir();
    let mut repo = std::env::var("BIPAGENTS_REPO").ok().filter(|r| !r.is_empty()).map(PathBuf::from);
    if repo.is_none() {
        if let Ok(exe) = std::env::current_exe().and_then(|e| e.canonicalize()) {
            repo = exe.ancestors().skip(1).find(|p| has_installer(p)).map(Path::to_path_buf);
        }
    }
    let repo = repo.unwrap_or_else(|| pyx::home().join("BipAgents"));
    let python = ["installer", "bridge"]
        .iter()
        .map(|folder| repo.join(folder).join(".venv").join("bin").join("python"))
        .find(|venv| venv.exists())
        .map(|venv| venv.to_string_lossy().into_owned())
        .unwrap_or_else(|| "python3".to_string());
    (repo, python)
}

/// `systemd-run --user --unit <unit> --collect --setenv=PYTHONPATH=<repo>/installer --working-directory=<repo>
/// <python> -m bipinstall` (the caller adds the installer's command).
pub fn installer_command(unit: &str) -> std::process::Command {
    let (repo, python) = repo_and_python();
    let mut command = std::process::Command::new("systemd-run");
    command.args(["--user", "--unit", unit, "--collect"]);
    command.arg(format!("--setenv=PYTHONPATH={}", repo.join("installer").to_string_lossy()));
    command.arg(format!("--working-directory={}", repo.to_string_lossy()));
    command.args([python.as_str(), "-m", "bipinstall"]);
    command
}

/// `subprocess.Popen(command, stdout=DEVNULL, stderr=DEVNULL, start_new_session=True)`: started, not waited for
/// (reaped in the background).
pub fn spawn_detached(mut command: std::process::Command) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    command.stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid() is async-signal-safe and only affects the child.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The installer's import-place, in a systemd unit of its own.
pub fn start_import(_folder: &Path, archive: &Path, passphrase: &Path) -> std::io::Result<()> {
    let mut command = installer_command(UNIT);
    command.args([
        "import-place",
        "--archive",
        &archive.to_string_lossy(),
        "--passphrase-file",
        &passphrase.to_string_lossy(),
    ]);
    spawn_detached(command)
}

/// What starts the import once the export is in (tests give their own).
pub type StartFn = Arc<dyn Fn(&Path, &Path, &Path) -> std::io::Result<()> + Send + Sync>;

pub struct Imports {
    /// None: $HOME, read each time.
    pub home: Option<PathBuf>,
    pub clock: Clock,
    pub start: StartFn,
    pub challenges: Challenges,
    /// {"received", "size"} while an export comes in.
    receiving: Mutex<Option<(u64, u64)>>,
}

fn busy() -> HttpError {
    HttpError::new(409, "an export is being received or put in")
}

impl Imports {
    pub fn new(home: Option<PathBuf>) -> Self {
        Self::with(home, crate::py::wall_clock(), Arc::new(start_import))
    }

    pub fn with(home: Option<PathBuf>, clock: Clock, start: StartFn) -> Self {
        Imports { home, clock, start, challenges: Challenges::default(), receiving: Mutex::new(None) }
    }

    pub fn folder(&self) -> PathBuf {
        self.home.clone().unwrap_or_else(pyx::home).join(FOLDER)
    }

    fn receiving(&self) -> Option<(u64, u64)> {
        *self.receiving.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_receiving(&self, value: Option<(u64, u64)>) {
        *self.receiving.lock().unwrap_or_else(|e| e.into_inner()) = value;
    }

    pub fn code(&self) -> Option<String> {
        let data = read(&self.folder().join("code.json"));
        let code = data.get("code")?.as_str()?.to_string();
        if code.chars().count() != CODE_LENGTH || float_of(data.get("expires")) < (self.clock)() {
            return None;
        }
        Some(code)
    }

    pub fn progress(&self) -> Dict {
        read(&self.folder().join("progress.json"))
    }

    pub fn importing(&self) -> bool {
        let step = self.progress();
        let truthy = |k: &str| step.get(k).map(crate::py::truthy).unwrap_or(false);
        !step.is_empty()
            && !truthy("done")
            && !truthy("error")
            && (self.clock)() - float_of(step.get("at")) < STALE_SECONDS
    }

    pub fn new_code(&self, config: &Config) -> ApiResult<Value> {
        if self.receiving().is_some() || self.importing() {
            return Err(busy());
        }
        use rand::Rng;
        let alphabet: Vec<char> = ALPHABET.chars().collect();
        let code: String = (0..CODE_LENGTH).map(|_| alphabet[rand::rngs::OsRng.gen_range(0..alphabet.len())]).collect();
        let data = json!({"code": code, "expires": (self.clock)() + CODE_SECONDS});
        write_private(&self.folder().join("code.json"), &crate::py::dumps(&data)).map_err(anyhow::Error::from)?;
        let _ = std::fs::remove_file(self.folder().join("progress.json"));
        tracing::info!("import code given");
        Ok(self.status(config))
    }

    pub fn withdraw(&self) -> ApiResult<()> {
        if self.receiving().is_some() || self.importing() {
            return Err(busy());
        }
        let _ = std::fs::remove_file(self.folder().join("code.json"));
        let _ = std::fs::remove_file(self.folder().join("progress.json"));
        Ok(())
    }

    pub fn status(&self, config: &Config) -> Value {
        if let Some((received, size)) = self.receiving() {
            return json!({"state": "receiving", "received": received, "size": size});
        }
        let step = self.progress();
        if !step.is_empty() {
            let mut out = Dict::new();
            for k in ["step", "total", "label"] {
                out.insert(k.into(), step.get(k).cloned().unwrap_or(Value::Null));
            }
            let truthy = |k: &str| step.get(k).map(crate::py::truthy).unwrap_or(false);
            let with = |state: &str, error: Option<Value>| {
                let mut all = Dict::new();
                all.insert("state".into(), json!(state));
                if let Some(e) = error {
                    all.insert("error".into(), e);
                }
                all.extend(out.clone());
                Value::Object(all)
            };
            if truthy("error") {
                return with("failed", step.get("error").cloned());
            }
            if truthy("done") {
                return with("done", None);
            }
            if (self.clock)() - float_of(step.get("at")) >= STALE_SECONDS {
                return with("failed", Some(json!("the import stopped half-way")));
            }
            return with("importing", None);
        }
        let Some(code) = self.code() else {
            return json!({"state": "none"});
        };
        let public = config.public_url.clone().unwrap_or_default();
        let address = public.strip_prefix("https://").unwrap_or(&public).trim_end_matches('/').to_string();
        let expires = float_of(read(&self.folder().join("code.json")).get("expires"));
        json!({"state": "waiting", "code": shown(&code), "address": address, "expires": expires as i64,
               "command": format!("bipagents send {address} {}", shown(&code)),
               "command_alone": format!("curl -fsSL {INSTALLER} | bash -s -- send {address} {}", shown(&code))})
    }

    pub async fn receive(
        &self,
        state: &AppState,
        address: Option<String>,
        headers: &HeaderMap,
        body: Body,
    ) -> ApiResult<Value> {
        let Some(code) = self.code() else {
            return Err(HttpError::new(403, "no code waits: ask the app for a new one"));
        };
        if self.receiving().is_some() || self.importing() {
            return Err(busy());
        }
        let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let challenge = header("x-import-challenge");
        let mac = header("x-import-proof").to_lowercase();
        let length = header("content-length");
        if length.is_empty() || !length.chars().all(|c| c.is_ascii_digit()) {
            return Err(HttpError::new(411, "the export's size is needed"));
        }
        let size: u64 = length.parse().unwrap_or(u64::MAX);
        let proved = self.challenges.take(&challenge)
            && match pyx::compare_digest(&proof(&code, &[UPLOAD_CONTEXT, &challenge, &size.to_string()]), &mac) {
                Some(same) => same,
                None => return Err(HttpError::internal()),
            };
        if !proved {
            state.guard.fail(address.as_deref(), "import proof");
            return Err(HttpError::new(401, "wrong proof"));
        }
        if size > MAX_BYTES {
            return Err(HttpError::new(413, "an export of 10 GB at most"));
        }
        let folder = self.folder();
        crate::py::makedirs(&folder.to_string_lossy(), 0o700).map_err(anyhow::Error::from)?;
        if free_bytes(&folder).map_err(anyhow::Error::from)? < size.saturating_mul(3).saturating_add(SPARE_BYTES) {
            return Err(HttpError::new(507, "not enough room left for this export"));
        }
        let (part, archive) = (folder.join("archive.part"), folder.join("archive.bipagents"));
        self.set_receiving(Some((0, size)));
        let result = self.write_archive(&folder, &part, &archive, &code, size, body).await;
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        self.set_receiving(None);
        result?;
        tracing::info!(bytes = size, address = ?J(&address), "import received");
        Ok(json!({"started": true}))
    }

    async fn write_archive(
        &self,
        folder: &Path,
        part: &Path,
        archive: &Path,
        code: &str,
        size: u64,
        body: Body,
    ) -> ApiResult<()> {
        use std::os::unix::fs::OpenOptionsExt;
        use tokio::io::AsyncWriteExt;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(part)
            .map_err(anyhow::Error::from)?;
        let mut out = tokio::fs::File::from_std(file);
        let mut stream = body.into_data_stream();
        let mut received: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let Ok(chunk) = chunk else {
                return Err(HttpError::new(400, "the sending stopped: start it again"));
            };
            received += chunk.len() as u64;
            self.set_receiving(Some((received, size)));
            if received > size {
                return Err(HttpError::new(400, "more than announced"));
            }
            out.write_all(&chunk).await.map_err(anyhow::Error::from)?;
        }
        out.flush().await.map_err(anyhow::Error::from)?;
        drop(out);
        if received != size {
            return Err(HttpError::new(400, "the export came incomplete: send it again"));
        }
        std::fs::rename(part, archive).map_err(anyhow::Error::from)?;
        let secret = folder.join("passphrase");
        write_private(&secret, code).map_err(anyhow::Error::from)?;
        let _ = std::fs::remove_file(folder.join("code.json")); // used: never twice
        let progress =
            json!({"step": 0, "total": 4, "label": "Received", "done": false, "error": null, "at": (self.clock)()});
        write_private(&folder.join("progress.json"), &crate::py::dumps(&progress)).map_err(anyhow::Error::from)?;
        let (start, folder, archive) = (self.start.clone(), folder.to_path_buf(), archive.to_path_buf());
        tokio::task::spawn_blocking(move || start(&folder, &archive, &secret))
            .await
            .map_err(anyhow::Error::from)?
            .map_err(anyhow::Error::from)?;
        Ok(())
    }
}

/// `shutil.disk_usage(path).free`.
fn free_bytes(path: &Path) -> std::io::Result<u64> {
    let c = std::ffi::CString::new(path.to_string_lossy().as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: statvfs fills the struct it is given; the path is a valid C string.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(stat.f_bavail as u64 * stat.f_frsize as u64)
}

fn cloud(state: &AppState) -> ApiResult<std::sync::Arc<Config>> {
    let config = state.config();
    if !config.cloud {
        return Err(HttpError::new(404, "not found"));
    }
    Ok(config)
}

fn guarded(state: &AppState, address: Option<&str>) -> ApiResult<std::sync::Arc<Config>> {
    let config = cloud(state)?;
    if state.guard.blocked(address) {
        return Err(HttpError::new(429, "too many wrong proofs, try again later"));
    }
    Ok(config)
}

/// (the app's routes, behind the bridge key; the computer's, proved with the code) around one `Imports`.
pub fn routers_with(imports: Arc<Imports>) -> (Router<AppState>, Router<AppState>) {
    let (i1, i2, i3) = (imports.clone(), imports.clone(), imports.clone());
    let admin = Router::new().route(
        "/import",
        get(move |State(state): State<AppState>| async move {
            let config = cloud(&state)?;
            ApiResult::Ok(web::ok(i2.status(&config)))
        })
        .post(move |State(state): State<AppState>| async move {
            let config = cloud(&state)?;
            ApiResult::Ok(web::ok(i1.new_code(&config)?))
        })
        .delete(move |State(state): State<AppState>| async move {
            cloud(&state)?;
            i3.withdraw()?;
            ApiResult::Ok(web::no_content())
        }),
    );
    let (i4, i5) = (imports.clone(), imports);
    let open = Router::new()
        .route(
            "/import/hello",
            get(move |State(state): State<AppState>, ClientAddr(address): ClientAddr, q: QueryMap| async move {
                let nonce = crate::cloud_setup::nonce_query(&q)?;
                let config = guarded(&state, address.as_deref())?;
                let code = i4.code();
                let Some(code) = code.filter(|_| i4.receiving().is_none() && !i4.importing()) else {
                    return Err(HttpError::new(404, "not found"));
                };
                let print_fp = crate::lan::fingerprint(&config).unwrap_or_default();
                let challenge = i4.challenges.issue();
                let proof = proof(&code, &[IMPORT_CONTEXT, &nonce.to_lowercase(), &print_fp, &challenge]);
                ApiResult::Ok(web::ok(json!({"fingerprint": print_fp, "challenge": challenge, "proof": proof})))
            }),
        )
        .route(
            "/import/archive",
            put(
                move |State(state): State<AppState>,
                      ClientAddr(address): ClientAddr,
                      headers: HeaderMap,
                      body: Body| async move {
                    guarded(&state, address.as_deref())?;
                    let out = i5.receive(&state, address, &headers, body).await?;
                    ApiResult::<Response>::Ok(web::ok(out))
                },
            ),
        );
    (admin, open)
}

/// (the app's routes, behind the bridge key; the computer's, proved with the code). Both mounted under /v1.
/// `home`: the account's home folder (tests give another).
pub fn place_import_routers(home: Option<PathBuf>) -> (Router<AppState>, Router<AppState>) {
    routers_with(Arc::new(Imports::new(home)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_lasts_an_hour_and_a_stuck_import_is_told() {
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(Mutex::new(1000.0f64));
        let n = now.clone();
        let imports = Imports::with(
            Some(dir.path().to_path_buf()),
            Arc::new(move || *n.lock().unwrap()),
            Arc::new(|_, _, _| Ok(())),
        );
        let mut config = Config::with_key("k".repeat(32));
        config.cloud = true;
        config.public_url = Some("https://203.0.113.7".into());
        let given = imports.new_code(&config).unwrap();
        assert_eq!(given["address"], "203.0.113.7");
        assert_eq!(given["expires"], json!(4600));
        *now.lock().unwrap() += CODE_SECONDS + 1.0;
        assert!(imports.code().is_none());
        assert_eq!(imports.status(&config), json!({"state": "none"}));
        let at = *now.lock().unwrap();
        std::fs::write(
            dir.path().join(FOLDER).join("progress.json"),
            format!("{{\"step\": 2, \"total\": 4, \"at\": {at}}}"),
        )
        .unwrap();
        assert_eq!(imports.status(&config)["state"], "importing");
        assert!(imports.importing());
        *now.lock().unwrap() += STALE_SECONDS;
        assert_eq!(
            imports.status(&config),
            json!({"state": "failed", "error": "the import stopped half-way", "step": 2, "total": 4, "label": null})
        );
        assert!(!imports.importing());
        assert_eq!(shown("ABCDEFGHJKMNPQRS"), "ABCD-EFGH-JKMN-PQRS");
    }
}
