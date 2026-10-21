//! A place nobody uses goes to sleep (a server of the all-included offer only): no request from an app for
//! `IDLE_DAYS` and no scheduled task run for a week, its Hermes stops, which gives the server's others about 250 MB of
//! memory back. The bridge keeps answering; the next request from an app wakes Hermes and waits for it (a few seconds).
//! Nothing is lost: only Hermes' process stops. A place with routines never sleeps (they run in Hermes).
//!
//!     data_dir/sleep.json                      {last, asleep}: the last request from an app, and whether Hermes sleeps
//!     ~/.local/state/bipagents/asleep          while it sleeps: the seat agent tells the guichet (not a stopped Hermes)
//!
//! Port of `bridge/bipbridge/sleep.py` (owner group: push-runs).

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde_json::{json, Value};

use crate::config::SharedConfig;
use crate::py::Clock;

pub const IDLE_DAYS: i64 = 14;
/// A scheduled task run since then: the place has routines, it stays up.
pub const ROUTINE_DAYS: i64 = 8;
pub const CHECK_SECONDS: u64 = 1800;
pub const WAKE_SECONDS: u64 = 90;
/// `~/.local/state/bipagents/asleep`.
pub const MARKER: &str = ".local/state/bipagents/asleep";
/// Hermes' own service, or the installer's.
pub const UNITS: [&str; 2] = ["hermes-gateway", "bipagents-hermes"];

/// `systemctl --user <args>` (replaceable in tests).
pub type Run = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>;
/// When each scheduled task last ran (ISO times, the store's cron_jobs).
pub type Routines = Arc<dyn Fn() -> BoxFuture<'static, Vec<String>> + Send + Sync>;
/// Whether Hermes answers.
pub type HermesUp = Arc<dyn Fn() -> BoxFuture<'static, bool> + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub async fn systemctl(args: Vec<String>) -> anyhow::Result<String> {
    let output = tokio::process::Command::new("systemctl")
        .arg("--user")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .await?;
    if !output.status.success() {
        use std::os::unix::process::ExitStatusExt;
        let code = output.status.code().unwrap_or_else(|| -output.status.signal().unwrap_or(0));
        anyhow::bail!("systemctl {}: {code}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct SleepState {
    last: f64,
    asleep: bool,
    saved: f64,
    waking: Option<Shared<BoxFuture<'static, ()>>>,
}

pub struct Sleep {
    pub config: SharedConfig,
    pub routines: Routines,
    pub hermes_up: HermesUp,
    pub run: Run,
    /// `time.time` (replaceable in tests).
    pub now: Clock,
    pub home: PathBuf,
    /// `data_dir/sleep.json`.
    pub path: PathBuf,
    state: Arc<Mutex<SleepState>>,
}

impl Sleep {
    pub fn new(config: SharedConfig, routines: Routines, hermes_up: HermesUp) -> Self {
        Self::with(
            config,
            routines,
            hermes_up,
            Arc::new(|args| Box::pin(systemctl(args))),
            crate::py::wall_clock(),
            None,
        )
    }

    /// `Sleep(config, routines, hermes_up, run=, now=, home=)`.
    pub fn with(
        config: SharedConfig,
        routines: Routines,
        hermes_up: HermesUp,
        run: Run,
        now: Clock,
        home: Option<PathBuf>,
    ) -> Self {
        let home = home.unwrap_or_else(|| {
            std::env::var("HOME")
                .ok()
                .filter(|h| !h.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(crate::py::expanduser("~")))
        });
        let db_path = config.get().outbox.db_path.clone();
        let path =
            std::path::Path::new(&db_path).parent().map(|p| p.to_path_buf()).unwrap_or_default().join("sleep.json");
        let (mut last, mut asleep) = (now(), false);
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&text) {
                let read_last = match data.get("last").filter(|v| crate::py::truthy(v)) {
                    Some(v) => crate::py::py_float(v).ok(),
                    None => Some(last),
                };
                if let Some(read_last) = read_last {
                    last = read_last;
                    asleep = data.get("asleep").map(crate::py::truthy).unwrap_or(false);
                }
            }
        }
        Sleep {
            config,
            routines,
            hermes_up,
            run,
            now,
            home,
            path,
            state: Arc::new(Mutex::new(SleepState { last, asleep, saved: 0.0, waking: None })),
        }
    }

    /// Only a server the app created (`cloud = true`) sleeps.
    pub fn enabled(&self) -> bool {
        self.config.get().cloud
    }

    /// The last request from an app (Unix seconds).
    pub fn last(&self) -> f64 {
        lock(&self.state).last
    }

    /// Whether Hermes sleeps.
    pub fn asleep(&self) -> bool {
        lock(&self.state).asleep
    }

    fn save(&self) {
        let (last, asleep) = {
            let st = lock(&self.state);
            (st.last, st.asleep)
        };
        let text = crate::py::dumps(&json!({"last": last, "asleep": asleep}));
        let written = (|| -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&self.path, text)
        })();
        if written.is_ok() {
            lock(&self.state).saved = (self.now)();
        }
    }

    fn mark(&self, asleep: bool) {
        let marker = self.home.join(MARKER);
        let _ = (|| -> std::io::Result<()> {
            if asleep {
                if let Some(parent) = marker.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&marker, format!("{}", (self.now)().trunc() as i64))
            } else {
                match std::fs::remove_file(&marker) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    other => other,
                }
            }
        })();
    }

    async fn units(&self) -> anyhow::Result<Vec<String>> {
        let args =
            ["list-units", "--all", "--type=service", "--plain", "--no-legend"].iter().map(|a| a.to_string()).collect();
        let out = (self.run)(args).await?;
        Ok(crate::hermes::splitlines(&out)
            .iter()
            .filter(|line| !crate::push::pyx::strip(line).is_empty())
            .filter_map(|line| crate::push::pyx::split(line).first().map(|n| n.to_string()))
            .filter(|name| UNITS.iter().any(|u| name.starts_with(u)))
            .collect())
    }

    /// A request from an app: noted, and Hermes woken first when it sleeps (concurrent requests wait together).
    pub async fn awake(self: &Arc<Self>) {
        if !self.enabled() {
            return;
        }
        let waking = {
            let mut st = lock(&self.state);
            st.last = (self.now)();
            if st.asleep {
                let running = st.waking.as_ref().filter(|w| w.peek().is_none()).cloned();
                Some(match running {
                    Some(task) => task,
                    None => {
                        let this = self.clone();
                        let handle = tokio::spawn(async move { this.wake().await });
                        let task = async move {
                            let _ = handle.await;
                        }
                        .boxed()
                        .shared();
                        st.waking = Some(task.clone());
                        task
                    }
                })
            } else {
                None
            }
        };
        match waking {
            Some(task) => task.await,
            None => {
                let due = {
                    let st = lock(&self.state);
                    st.last - st.saved > 600.0
                };
                if due {
                    self.save();
                }
            }
        }
    }

    async fn wake(&self) {
        let started: anyhow::Result<()> = async {
            for unit in self.units().await? {
                (self.run)(vec!["start".into(), unit]).await?;
            }
            Ok(())
        }
        .await;
        if let Err(exc) = started {
            tracing::warn!(error = %exc, "hermes not woken");
        }
        let deadline = (self.now)() + WAKE_SECONDS as f64;
        while (self.now)() < deadline && !(self.hermes_up)().await {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        lock(&self.state).asleep = false;
        self.mark(false);
        self.save();
        tracing::info!("hermes woken");
    }

    /// Puts Hermes to sleep when the place is idle. Whether it did.
    pub async fn check(&self) -> bool {
        let idle = {
            let st = lock(&self.state);
            !st.asleep && (self.now)() - st.last >= (IDLE_DAYS * 86400) as f64
        };
        if !self.enabled() || !idle {
            return false;
        }
        let since = chrono::Utc::now() - chrono::Duration::days(ROUTINE_DAYS);
        for seen in (self.routines)().await {
            let Some(when) = crate::store::fromisoformat(&seen.replace('Z', "+00:00")) else { continue };
            if when >= since {
                return false;
            }
        }
        let stopped: anyhow::Result<Vec<String>> = async {
            let units = self.units().await?;
            for unit in &units {
                (self.run)(vec!["stop".into(), unit.clone()]).await?;
            }
            Ok(units)
        }
        .await;
        let units = match stopped {
            Ok(units) => units,
            Err(exc) => {
                tracing::warn!(error = %exc, "hermes not put to sleep");
                return false;
            }
        };
        if units.is_empty() {
            return false;
        }
        let idle_days = {
            let mut st = lock(&self.state);
            st.asleep = true;
            (((self.now)() - st.last) / 86400.0).floor() as i64
        };
        self.mark(true);
        self.save();
        tracing::info!(idle_days = idle_days, "hermes asleep");
        true
    }

    pub async fn run_forever(self: Arc<Self>) {
        // the server restarted: its services came back up
        if self.asleep() && (self.hermes_up)().await {
            lock(&self.state).asleep = false;
            self.mark(false);
            self.save();
        }
        loop {
            tokio::time::sleep(Duration::from_secs(CHECK_SECONDS)).await;
            // never the bridge's end
            self.check().await;
        }
    }
}
