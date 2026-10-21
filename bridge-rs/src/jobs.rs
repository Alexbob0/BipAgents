//! Background jobs (« Tâches de fond », decided 2026-10-21): long work an agent does apart from the person's
//! conversations, with its cost shown before (a budget the person chooses) and after (the tokens it took), pause, resume
//! and cancel, and a notification when it ends. Two kinds:
//!
//!     mail_scan   a mailbox read through by steps (mail_scan.py): listed and paused here, run there
//!     task        a long task in the agent's own Hermes, with all its tools, in a conversation of its own (« 🧩 <title> »):
//!                 the bridge starts the run, reads the conversation's token counters every few seconds and stops the run
//!                 once the budget is reached (paused: the person may give more and resume) or after three hours; the
//!                 agent ends with a report, which the notification opens
//!
//! An agent may propose a task (MCP `propose_task` in agent_tools.py, for work too long for a reply): it waits, budget and all, until the
//! person starts it in the app. An agent never starts paid work by itself.
//!
//!     GET    /v1/jobs                        every job of every agent: proposals first, then the newest
//!     POST   /v1/agents/{id}/jobs            {title, prompt, max_tokens, template?}   a task started now
//!     POST   /v1/jobs/{id}/start             {max_tokens?}       a proposed task started
//!     POST   /v1/jobs/{id}/pause
//!     POST   /v1/jobs/{id}/resume            {max_tokens?, message?}   a bigger budget, the words that resume it
//!     DELETE /v1/jobs/{id}                   stopped if it runs, then forgotten (its conversation and pages stay)
//!
//! Mail scans keep their own routes for their steps (/v1/agents/{id}/mail-scan…); their job id here is
//! `mail-scan:<agent>`.
//!
//! Port of `bridge/bipbridge/jobs.py` (owner group: tools-knowledge).

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use futures::future::BoxFuture;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::{HermesClient, HermesError};
use crate::mail_scan::{self, ScanError, Scans};
use crate::push::PushService;
use crate::runs::RunHub;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::web::{
    self, body_loc, check_max_len, check_min_len, check_range, ApiResult, FieldError, HttpError, Json, OptionalJson,
    Validate,
};
use crate::{py, Dict};

pub const POLL_SECONDS: f64 = 10.0;
/// A run going on longer is paused: the person resumes it if it was worth it.
pub const MAX_STEP_SECONDS: f64 = 3.0 * 3600.0;
/// Tasks running at once on this install (one per agent).
pub const RUNNING_MAX: usize = 3;
/// An agent's proposals waiting for the person.
pub const PROPOSALS_MAX: usize = 5;
/// Seconds a stop may take before the task is counted as paused anyway.
pub const STOP_GRACE: f64 = 120.0;
/// Finished tasks kept in the list, per agent.
pub const KEPT_PER_AGENT: usize = 40;
pub const REPORT_CHARS: usize = 4000;
/// An agent's proposal: tokens.
pub const BUDGETS: [(&str, i64); 3] = [("small", 300_000), ("medium", 1_000_000), ("large", 3_000_000)];
pub const TERMINAL: [&str; 5] = ["completed", "failed", "cancelled", "canceled", "interrupted"];
pub const TITLE_MARK: &str = "🧩 ";
pub const INSTRUCTIONS: &str =
    "This is a background task the person gave you from the app (« Tâches de fond »): they are not following
this conversation. Work through it on your own, step by step, with your tools. Don't ask questions unless you truly
cannot go on: choose sensible defaults and say which in your report. Keep what you make where the person finds it: a
page (create_page) for a table, a comparison or a dashboard, a file in your workspace for a longer document. Your work
has a budget; if you are told to continue, pick up where you stopped instead of starting again. End with a short
report in the person's language: what you did, what you found or made and where it is, what is left or uncertain.";
pub const CONTINUE: &str = "Continue the task where you stopped (don't start again), then end with your report.";
pub const DONE_BODY: &str = "Tâche de fond terminée";
pub const PAUSED_BODY: &str = "Tâche de fond en pause";
pub const PROPOSED_BODY: &str = "Tâche de fond proposée";

/// Python's `JobError` (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct JobError(pub String);

fn job_error(message: &str) -> anyhow::Error {
    JobError(message.into()).into()
}

fn budget_of(name: &str) -> Option<i64> {
    BUDGETS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

fn budgets() -> Value {
    Value::Object(BUDGETS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect())
}

fn folder(config: &crate::config::Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("jobs")
}

static JOB_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^t[0-9a-f]{12}$").expect("valid regex"));

fn path_of(config: &crate::config::Config, job_id: &str) -> Result<PathBuf, JobError> {
    if !JOB_ID.is_match(job_id) {
        return Err(JobError("unknown job".into()));
    }
    Ok(folder(config).join(format!("{job_id}.json")))
}

pub fn load(config: &crate::config::Config, job_id: &str) -> Option<Dict> {
    let text = tk::read_text(&path_of(config, job_id).ok()?).ok()?;
    match serde_json::from_str::<Value>(&text).ok()? {
        Value::Object(job) => Some(job),
        _ => None,
    }
}

pub fn save(config: &crate::config::Config, job: &Dict) -> anyhow::Result<()> {
    let path = path_of(config, &tk::str_or_empty(job.get("id")))?;
    let parent = folder(config);
    if !parent.is_dir() {
        if let Some(grand) = parent.parent() {
            std::fs::create_dir_all(grand)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(&parent) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e.into()),
            _ => {}
        }
    }
    let temporary = path.with_extension("tmp");
    let mut fh = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&temporary)?;
    fh.write_all(py::Dumps::new().unicode().dumps(&Value::Object(job.clone())).as_bytes())?;
    drop(fh);
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

/// Every task kept on this install, in the order of their files.
pub fn tasks(config: &crate::config::Config) -> Vec<Dict> {
    let mut names: Vec<String> = std::fs::read_dir(folder(config))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('t') && n.ends_with(".json"))
        .collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let Ok(text) = tk::read_text(&folder(config).join(&name)) else { continue };
        if let Ok(Value::Object(job)) = serde_json::from_str::<Value>(&text) {
            out.push(job);
        }
    }
    out
}

/// The conversation's token counters (Hermes keeps them per session, updated after each model call).
pub fn counters(session: &Dict) -> Dict {
    let number = |key: &str| -> i64 {
        match session.get(key) {
            Some(Value::Number(n)) => n.as_i64().unwrap_or_else(|| n.as_f64().map(|f| f.trunc() as i64).unwrap_or(0)),
            _ => 0,
        }
    };
    let mut out = Dict::new();
    out.insert("input".into(), json!(number("input_tokens")));
    out.insert("output".into(), json!(number("output_tokens")));
    out.insert("cached".into(), json!(number("cache_read_tokens")));
    out
}

fn int_of(value: Option<&Value>) -> i64 {
    value.map(|v| py::py_int(v).unwrap_or(0)).unwrap_or(0)
}

pub fn spent(job: &Dict) -> i64 {
    let tokens = job.get("tokens");
    int_of(tokens.and_then(|t| t.get("input"))) + int_of(tokens.and_then(|t| t.get("output")))
}

/// A mailbox scan (mail_scan.py) in the common shape.
pub fn scan_view(job: &Dict, running: bool) -> Dict {
    let mut state = job.get("state").map(py::py_str).unwrap_or_else(|| "none".into());
    if (state == "counting" || state == "running") && !running {
        state = "paused".into();
    }
    let step = job.get("step").filter(|v| py::truthy(v)).cloned().unwrap_or(json!({}));
    let last = match job.get("ranges") {
        Some(Value::Array(r)) if !r.is_empty() => r[r.len() - 1].clone(),
        _ => json!({}),
    };
    let agent = job.get("agent").cloned().unwrap_or(Value::Null);
    let has_step = step.as_object().is_some_and(|m| !m.is_empty());
    let out = json!({
        "id": format!("mail-scan:{}", py::py_str(&agent)), "kind": "mail_scan", "agent": agent,
        "title": job.get("label").filter(|v| py::truthy(v)).cloned().unwrap_or(json!("")),
        "state": if state == "counting" { "running".to_string() } else { state.clone() },
        "reason": if job.get("error") == Some(&json!("budget")) { json!("budget") } else { Value::Null },
        "error": if state == "failed" { job.get("error").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "created": job.get("created").cloned().unwrap_or(Value::Null),
        "finished": if state == "done" || state == "ready" { last.get("at").cloned().unwrap_or(Value::Null) } else { Value::Null },
        "tokens": job.get("tokens").filter(|v| py::truthy(v)).cloned().unwrap_or(json!({"input": 0, "output": 0, "cached": 0})),
        "max_tokens": step.get("max_tokens").cloned().unwrap_or(Value::Null),
        "progress": if has_step { json!({"done": step.get("processed").cloned().unwrap_or(json!(0)),
                                         "total": step.get("mails").cloned().unwrap_or(json!(0))}) } else { Value::Null },
        "processed": job.get("processed").cloned().unwrap_or(json!(0)),
        "pages": job.get("pages").cloned().unwrap_or(json!(0)),
    });
    match out {
        Value::Object(m) => m,
        _ => Dict::new(),
    }
}

pub fn task_view(job: &Dict) -> Dict {
    const KEEP: [&str; 18] = [
        "id",
        "kind",
        "agent",
        "title",
        "prompt",
        "template",
        "state",
        "reason",
        "error",
        "created",
        "started",
        "finished",
        "tokens",
        "max_tokens",
        "session_id",
        "report",
        "proposed_by",
        "runs",
    ];
    KEEP.iter().map(|k| (k.to_string(), job.get(*k).cloned().unwrap_or(Value::Null))).collect()
}

fn state_of(job: &Dict) -> String {
    tk::str_or_empty(job.get("state"))
}

fn agent_of(job: &Dict) -> String {
    py::py_str(job.get("agent").unwrap_or(&Value::Null))
}

/// `job.get(a) or job.get(b) or 0` as a number.
fn first_number(job: &Dict, keys: &[&str]) -> f64 {
    for key in keys {
        if let Some(v) = job.get(*key).filter(|v| py::truthy(v)) {
            return py::py_float(v).unwrap_or(0.0);
        }
    }
    0.0
}

/// `push.notify_job(agent, session_id, line, job_id, body)`.
pub type TellJob = Arc<dyn Fn(String, Option<String>, String, String, String) -> BoxFuture<'static, ()> + Send + Sync>;

/// Background tasks: long runs with a budget, paused and resumed, and the agents' proposals.
pub struct Jobs {
    pub config: SharedConfig,
    pub hermes: HermesClient,
    pub hub: Option<Arc<RunHub>>,
    pub push: Option<Arc<PushService>>,
    pub scans: Option<Arc<Scans>>,
    pub poll: f64,
    pub max_seconds: f64,
    /// The tasks followed now (job id → task), stopped with the bridge.
    pub followers: std::sync::Mutex<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>,
    /// How a task's end or an agent's proposal is told (`push.notify_job`; tests give their own).
    pub tell: Option<TellJob>,
}

impl Jobs {
    pub fn new(
        config: SharedConfig,
        hermes: HermesClient,
        hub: Option<Arc<RunHub>>,
        push: Option<Arc<PushService>>,
        scans: Option<Arc<Scans>>,
    ) -> Self {
        let tell: Option<TellJob> = push.clone().map(|push| {
            let tell: TellJob = Arc::new(move |agent, session, line, job_id, body| {
                let push = push.clone();
                Box::pin(async move {
                    push.notify_job(&agent, session.as_deref(), &line, &job_id, &body).await;
                })
            });
            tell
        });
        Jobs {
            config,
            hermes,
            hub,
            push,
            scans,
            poll: POLL_SECONDS,
            max_seconds: MAX_STEP_SECONDS,
            followers: Default::default(),
            tell,
        }
    }

    fn followers(&self) -> std::sync::MutexGuard<'_, HashMap<String, tokio::task::JoinHandle<()>>> {
        self.followers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Waits until the task's follower ends (tests: Python's `await board.followers[id]`).
    pub async fn wait(&self, job_id: &str) {
        loop {
            let done = self.followers().get(job_id).is_none_or(|t| t.is_finished());
            if done {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    // -- the list ------------------------------------------------------------------------------------------------------

    pub fn list(&self) -> Vec<Dict> {
        let config = self.config.get();
        let mut out: Vec<Dict> =
            tasks(&config).iter().filter(|job| config.agent(Some(&agent_of(job))).is_some()).map(task_view).collect();
        for name in config.agents.keys() {
            if let Some(scan) = mail_scan::load(&config, name) {
                let running = self.scans.as_ref().is_some_and(|s| s.running(name));
                out.push(scan_view(&scan, running));
            }
        }
        out.sort_by(|a, b| {
            let key = |j: &Dict| {
                let state = state_of(j);
                (
                    state != "proposed",
                    state != "running" && state != "stopping",
                    -first_number(j, &["started", "created"]),
                )
            };
            let (ka, kb) = (key(a), key(b));
            (ka.0, ka.1).cmp(&(kb.0, kb.1)).then(ka.2.partial_cmp(&kb.2).unwrap_or(std::cmp::Ordering::Equal))
        });
        out
    }

    pub fn view(&self, job_id: &str) -> Result<Dict, JobError> {
        load(&self.config.get(), job_id).map(|job| task_view(&job)).ok_or_else(|| JobError("unknown job".into()))
    }

    fn agent(&self, name: &str) -> Result<AgentConfig, JobError> {
        self.config
            .get()
            .agent(Some(name))
            .cloned()
            .ok_or_else(|| JobError("this agent isn't on this install any more".into()))
    }

    // -- a new task ----------------------------------------------------------------------------------------------------

    pub fn new_task(
        &self,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        max_tokens: i64,
        template: Option<&str>,
        proposed_by: &str,
    ) -> anyhow::Result<Dict> {
        let title = tk::head(&tk::squeeze(title), 80).to_string();
        let prompt = tk::strip(prompt).to_string();
        if title.is_empty() || prompt.is_empty() {
            return Err(job_error("a task needs a title and what to do"));
        }
        let job = json!({"id": format!("t{}", py::token_hex(6)), "kind": "task", "agent": agent.name, "title": title,
                         "prompt": prompt, "template": template, "state": "proposed", "reason": null, "error": null,
                         "created": py::time_time(), "started": null, "finished": null, "max_tokens": max_tokens,
                         "tokens": {"input": 0, "output": 0, "cached": 0}, "session_id": null, "run_id": null,
                         "report": null, "proposed_by": proposed_by, "runs": 0});
        let Value::Object(job) = job else { unreachable!() };
        save(&self.config.get(), &job)?;
        self.trim(&agent.name);
        Ok(job)
    }

    pub async fn create(
        self: &Arc<Self>,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        max_tokens: i64,
        template: Option<&str>,
    ) -> anyhow::Result<Dict> {
        self.may_run(&agent.name)?;
        let mut job = self.new_task(agent, title, prompt, max_tokens, template, "person")?;
        let text = format!("{}\n\n{}", tk::str_or_empty(job.get("title")), tk::str_or_empty(job.get("prompt")));
        if let Err(err) = self.launch(&mut job, &text).await {
            if err.downcast_ref::<JobError>().is_some() {
                if let Ok(path) = path_of(&self.config.get(), &tk::str_or_empty(job.get("id"))) {
                    let _ = std::fs::remove_file(path);
                }
            }
            return Err(err);
        }
        Ok(task_view(&job))
    }

    /// An agent's proposal: kept until the person starts it (or forgets it) in the app.
    pub async fn propose(&self, agent: &AgentConfig, title: &str, prompt: &str, budget: &str) -> anyhow::Result<Dict> {
        let waiting =
            tasks(&self.config.get()).iter().filter(|j| agent_of(j) == agent.name && state_of(j) == "proposed").count();
        if waiting >= PROPOSALS_MAX {
            return Err(job_error("enough of your proposals are waiting for the person already"));
        }
        let max_tokens = budget_of(budget).unwrap_or(1_000_000);
        let job = self.new_task(agent, title, prompt, max_tokens, None, "agent")?;
        if let Some(tell) = &self.tell {
            let line = format!("{PROPOSED_BODY} : {}", tk::str_or_empty(job.get("title")));
            tell(agent.name.clone(), None, line, tk::str_or_empty(job.get("id")), PROPOSED_BODY.into()).await;
        }
        Ok(job)
    }

    pub async fn start(self: &Arc<Self>, job_id: &str, max_tokens: Option<i64>) -> anyhow::Result<Dict> {
        let job = load(&self.config.get(), job_id);
        let Some(mut job) = job.filter(|j| state_of(j) == "proposed") else {
            return Err(job_error("this task isn't waiting to start"));
        };
        self.may_run(&agent_of(&job))?;
        if let Some(max) = max_tokens.filter(|m| *m != 0) {
            job.insert("max_tokens".into(), json!(max));
        }
        let text = format!("{}\n\n{}", tk::str_or_empty(job.get("title")), tk::str_or_empty(job.get("prompt")));
        self.launch(&mut job, &text).await?;
        Ok(task_view(&job))
    }

    fn may_run(&self, agent: &str) -> Result<(), JobError> {
        let running: Vec<Dict> = tasks(&self.config.get())
            .into_iter()
            .filter(|j| ["running", "stopping"].contains(&state_of(j).as_str()))
            .collect();
        if running.iter().any(|j| agent_of(j) == agent) {
            return Err(JobError("this agent is already on a background task".into()));
        }
        if running.len() >= RUNNING_MAX {
            return Err(JobError("enough background tasks are running here for now".into()));
        }
        Ok(())
    }

    fn trim(&self, agent: &str) {
        let config = self.config.get();
        let mut ended: Vec<Dict> = tasks(&config)
            .into_iter()
            .filter(|j| agent_of(j) == agent && ["done", "failed"].contains(&state_of(j).as_str()))
            .collect();
        // sorted(..., reverse=True) keeps equal keys in their order
        ended.sort_by(|a, b| {
            first_number(b, &["finished", "created"])
                .partial_cmp(&first_number(a, &["finished", "created"]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for job in ended.iter().skip(KEPT_PER_AGENT) {
            if let Ok(path) = path_of(&config, &tk::str_or_empty(job.get("id"))) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    // -- running -------------------------------------------------------------------------------------------------------

    /// A run in the task's conversation (made the first time), followed until it ends.
    async fn launch(self: &Arc<Self>, job: &mut Dict, text: &str) -> anyhow::Result<()> {
        let agent = self.agent(&agent_of(job))?;
        let refused = |err: HermesError| -> anyhow::Error {
            match &err {
                HermesError::Http { status, .. } => JobError(format!("the agent refused the task ({status})")).into(),
                HermesError::Decode(_) => err.into(),
                _ => job_error("the agent can't be reached"),
            }
        };
        if !job.get("session_id").is_some_and(py::truthy) {
            let title = format!("{TITLE_MARK}{}", tk::str_or_empty(job.get("title")));
            let session = self.hermes.create_session(&agent, &title).await.map_err(refused)?;
            job.insert("session_id".into(), json!(session));
        }
        let session_id = tk::str_or_empty(job.get("session_id"));
        let base = counters(&self.hermes.session(&agent, &session_id).await.map_err(refused)?);
        let run_id = self.hermes.start_run(&agent, text, &session_id, Some(INSTRUCTIONS)).await.map_err(refused)?;
        let now = py::time_time();
        let before = job.get("tokens").cloned().unwrap_or(json!({}));
        let started = job.get("started").filter(|v| py::truthy(v)).cloned().unwrap_or(json!(now));
        let runs = int_of(job.get("runs")) + 1;
        for (key, value) in [
            ("state", json!("running")),
            ("reason", Value::Null),
            ("error", Value::Null),
            ("run_id", json!(run_id)),
            ("base", Value::Object(base)),
            ("before", before),
            ("step_started", json!(now)),
            ("started", started),
            ("finished", Value::Null),
            ("runs", json!(runs)),
            ("stopping", Value::Null),
        ] {
            job.insert(key.into(), value);
        }
        save(&self.config.get(), job)?;
        if let Some(hub) = &self.hub {
            hub.watch(&agent, &run_id, Some(&session_id), false); // approvals and questions still pushed
        }
        let id = tk::str_or_empty(job.get("id"));
        self.follow(&id);
        tracing::info!(agent = %agent.name, job = %id, budget = int_of(job.get("max_tokens")), "job started");
        Ok(())
    }

    fn follow(self: &Arc<Self>, job_id: &str) {
        let mut followers = self.followers();
        if followers.get(job_id).is_none_or(|t| t.is_finished()) {
            let (this, id) = (self.clone(), job_id.to_string());
            followers.insert(job_id.to_string(), tokio::spawn(async move { this.watch(&id).await }));
        }
    }

    /// After a restart: the running tasks followed again.
    pub async fn resume_following(self: Arc<Self>) -> usize {
        let config = self.config.get();
        let mut count = 0;
        for job in tasks(&config) {
            let state = state_of(&job);
            if !(state == "running" || state == "stopping") {
                continue;
            }
            let Some(agent) = config.agent(Some(&agent_of(&job))) else { continue };
            let run_id = tk::str_or_empty(job.get("run_id"));
            if let Some(hub) = &self.hub {
                if !run_id.is_empty() {
                    let session = job.get("session_id").filter(|v| !v.is_null()).map(py::py_str);
                    hub.watch(agent, &run_id, session.as_deref(), false);
                }
            }
            self.follow(&tk::str_or_empty(job.get("id")));
            count += 1;
        }
        count
    }

    /// Stops following (the bridge stops).
    pub fn stop_followers(&self) {
        for (_, task) in self.followers.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            task.abort();
        }
    }

    async fn watch(&self, job_id: &str) {
        loop {
            tokio::time::sleep(Duration::from_secs_f64(self.poll)).await;
            let config = self.config.get();
            let Some(mut job) = load(&config, job_id) else { return };
            let state = state_of(&job);
            if state != "running" && state != "stopping" {
                return;
            }
            let Some(agent) = config.agent(Some(&agent_of(&job))).cloned() else {
                job.insert("state".into(), json!("failed"));
                job.insert("error".into(), json!("this agent isn't on this install any more"));
                job.insert("finished".into(), json!(py::time_time()));
                let _ = save(&config, &job);
                return;
            };
            let run_id = tk::str_or_empty(job.get("run_id"));
            let run = match self.hermes.run(&agent, &run_id).await {
                Ok(run) => run,
                Err(HermesError::Http { status, .. }) if status == 404 || status == 410 => {
                    // Hermes restarted, or forgot the run long after it ended
                    let mut gone = Dict::new();
                    gone.insert("status".into(), json!("gone"));
                    gone
                }
                Err(HermesError::Decode(err)) => {
                    tracing::error!(job = %job_id, exc = %err, "Task exception was never retrieved");
                    return;
                }
                Err(_) => continue, // Hermes restarting: asked again at the next round
            };
            let session_id = tk::str_or_empty(job.get("session_id"));
            match self.hermes.session(&agent, &session_id).await {
                Ok(session) => count(&mut job, &session, None),
                Err(HermesError::Decode(err)) => {
                    tracing::error!(job = %job_id, exc = %err, "Task exception was never retrieved");
                    return;
                }
                Err(_) => {}
            }
            let status = tk::str_or_empty(run.get("status"));
            if TERMINAL.contains(&status.as_str()) || status == "gone" {
                self.ended(&agent, &mut job, &run).await;
                return;
            }
            let now = py::time_time();
            if state_of(&job) == "running" {
                let step_started = job.get("step_started").map(|v| py::py_float(v).unwrap_or(now)).unwrap_or(now);
                if spent(&job) >= int_of(job.get("max_tokens")) {
                    self.stop(&agent, &mut job, "budget").await;
                } else if now - step_started >= self.max_seconds {
                    self.stop(&agent, &mut job, "time").await;
                }
            } else {
                let asked = job.get("stop_asked").map(|v| py::py_float(v).unwrap_or(now)).unwrap_or(now);
                if now - asked >= STOP_GRACE {
                    let mut cancelled = Dict::new();
                    cancelled.insert("status".into(), json!("cancelled"));
                    self.ended(&agent, &mut job, &cancelled).await; // Hermes never confirmed the stop
                    return;
                }
            }
            let _ = save(&config, &job);
        }
    }

    async fn stop(&self, agent: &AgentConfig, job: &mut Dict, reason: &str) {
        job.insert("state".into(), json!("stopping"));
        job.insert("stopping".into(), json!(reason));
        job.insert("stop_asked".into(), json!(py::time_time()));
        let _ = save(&self.config.get(), job);
        let id = tk::str_or_empty(job.get("id"));
        if self.hermes.stop_run(agent, &tk::str_or_empty(job.get("run_id"))).await.is_err() {
            tracing::info!(agent = %agent.name, job = %id, "job stop refused");
        }
        tracing::info!(agent = %agent.name, job = %id, reason = %reason, "job stopping");
    }

    async fn ended(&self, agent: &AgentConfig, job: &mut Dict, run: &Dict) {
        let mut status = tk::str_or_empty(run.get("status"));
        let usage = match run.get("usage") {
            Some(Value::Object(u)) => Some(u.clone()),
            _ => None,
        };
        let session_id = tk::str_or_empty(job.get("session_id"));
        let session = self.hermes.session(agent, &session_id).await.unwrap_or_default();
        count(job, &session, usage.as_ref());
        let mut output = run.get("output").and_then(Value::as_str).map(str::to_string);
        if status == "gone" {
            output = match self.hermes.session_messages(agent, &session_id).await {
                Ok(messages) => crate::cronwatch::final_reply(&messages),
                Err(_) => None,
            };
            status = if output.as_deref().is_some_and(|o| !o.is_empty()) {
                "completed".into()
            } else {
                "interrupted".into()
            };
        }
        let now = py::time_time();
        let stopping = job.get("stopping").filter(|v| py::truthy(v)).cloned();
        if stopping.is_some() || ["cancelled", "canceled", "interrupted"].contains(&status.as_str()) {
            job.insert("state".into(), json!("paused"));
            job.insert("reason".into(), stopping.unwrap_or(json!("interrupted")));
        } else if status == "completed" {
            let report = tk::head(tk::strip(output.as_deref().unwrap_or("")), REPORT_CHARS).to_string();
            job.insert("state".into(), json!("done"));
            job.insert("report".into(), if report.is_empty() { Value::Null } else { json!(report) });
            job.insert("finished".into(), json!(now));
        } else {
            let error = run.get("error").and_then(Value::as_str).unwrap_or("");
            let error = if error.is_empty() { "the task failed" } else { error };
            job.insert("state".into(), json!("failed"));
            job.insert("error".into(), json!(tk::head(error, 300)));
            job.insert("finished".into(), json!(now));
        }
        job.insert("stopping".into(), Value::Null);
        job.insert("stop_asked".into(), Value::Null);
        job.insert("run_id".into(), Value::Null);
        let _ = save(&self.config.get(), job);
        let reason = job.get("reason").cloned().unwrap_or(Value::Null);
        tracing::info!(agent = %agent.name, job = %tk::str_or_empty(job.get("id")), state = %state_of(job),
            reason = ?crate::logs::J(&reason), tokens = spent(job), "job ended");
        if state_of(job) == "done" || reason == json!("budget") || reason == json!("time") {
            self.tell_end(agent, job).await;
        }
    }

    async fn tell_end(&self, agent: &AgentConfig, job: &Dict) {
        let Some(tell) = &self.tell else { return };
        let body = if state_of(job) == "done" { DONE_BODY } else { PAUSED_BODY };
        let line = format!("{body} : {}", tk::str_or_empty(job.get("title")));
        let session = job.get("session_id").filter(|v| !v.is_null()).map(py::py_str);
        tell(agent.name.clone(), session, line, tk::str_or_empty(job.get("id")), body.into()).await;
    }

    // -- the person's controls -----------------------------------------------------------------------------------------

    pub async fn pause(&self, job_id: &str) -> anyhow::Result<Dict> {
        if job_id.starts_with("mail-scan:") {
            return self.scan(job_id, "pause", None);
        }
        let Some(mut job) = load(&self.config.get(), job_id) else { return Err(job_error("unknown job")) };
        if state_of(&job) == "running" {
            let agent = self.agent(&agent_of(&job))?;
            self.stop(&agent, &mut job, "stopped").await;
        }
        Ok(task_view(&job))
    }

    pub async fn resume(
        self: &Arc<Self>,
        job_id: &str,
        max_tokens: Option<i64>,
        message: Option<&str>,
    ) -> anyhow::Result<Dict> {
        if job_id.starts_with("mail-scan:") {
            return self.scan(job_id, "resume", max_tokens);
        }
        let job = load(&self.config.get(), job_id);
        let Some(mut job) = job.filter(|j| ["paused", "failed", "done"].contains(&state_of(j).as_str())) else {
            return Err(job_error("nothing to resume"));
        };
        self.may_run(&agent_of(&job))?;
        match max_tokens.filter(|m| *m != 0) {
            Some(max) => {
                job.insert("max_tokens".into(), json!(max.max(spent(&job) + 1000)));
            }
            None if spent(&job) >= int_of(job.get("max_tokens")) => {
                return Err(job_error("the budget is used up: give a bigger one"))
            }
            None => {}
        }
        let words = tk::head(tk::strip(message.unwrap_or("")), 2000).to_string();
        let text = if words.is_empty() { CONTINUE.to_string() } else { words };
        self.launch(&mut job, &text).await?;
        Ok(task_view(&job))
    }

    pub async fn forget(&self, job_id: &str) -> anyhow::Result<()> {
        if job_id.starts_with("mail-scan:") {
            self.scan(job_id, "forget", None)?;
            return Ok(());
        }
        let config = self.config.get();
        let Some(job) = load(&config, job_id) else { return Err(job_error("unknown job")) };
        let run_id = tk::str_or_empty(job.get("run_id"));
        if ["running", "stopping"].contains(&state_of(&job).as_str()) && !run_id.is_empty() {
            if let Ok(agent) = self.agent(&agent_of(&job)) {
                let _ = self.hermes.stop_run(&agent, &run_id).await;
            }
        }
        if let Some(follower) = self.followers().remove(job_id) {
            follower.abort();
        }
        if let Ok(path) = path_of(&config, job_id) {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    fn scan(&self, job_id: &str, action: &str, max_tokens: Option<i64>) -> anyhow::Result<Dict> {
        let name = job_id.strip_prefix("mail-scan:").unwrap_or(job_id);
        let config = self.config.get();
        let agent = config.agent(Some(name)).cloned();
        let (Some(agent), Some(scans)) = (agent, self.scans.as_ref()) else { return Err(job_error("unknown job")) };
        if mail_scan::load(&config, name).is_none() {
            return Err(job_error("unknown job"));
        }
        let as_job_error = |err: anyhow::Error| match err.downcast_ref::<ScanError>() {
            Some(exc) => JobError(exc.0.clone()).into(),
            None => err,
        };
        match action {
            "pause" => {
                scans.pause(&config, name).map_err(as_job_error)?;
            }
            "resume" => {
                scans.resume(&config, &agent, max_tokens).map_err(as_job_error)?;
            }
            _ => {
                scans.forget(&config, name).map_err(as_job_error)?;
                let mut out = Dict::new();
                out.insert("id".into(), json!(job_id));
                out.insert("state".into(), json!("none"));
                return Ok(out);
            }
        }
        let job = mail_scan::load(&config, name).unwrap_or_else(|| {
            let mut d = Dict::new();
            d.insert("agent".into(), json!(name));
            d
        });
        Ok(scan_view(&job, scans.running(name)))
    }
}

/// What the task took so far: the tokens before this run, and this run's (its exact usage when it ended, else the
/// conversation's counters since it started).
fn count(job: &mut Dict, session: &Dict, usage: Option<&Dict>) {
    let before =
        job.get("before").filter(|v| py::truthy(v)).or_else(|| job.get("tokens")).cloned().unwrap_or(json!({}));
    let base = job.get("base").filter(|v| py::truthy(v)).cloned().unwrap_or(json!({}));
    let now = counters(session);
    let exact = usage.is_some_and(|u| {
        u.get("input_tokens").is_some_and(py::truthy) || u.get("output_tokens").is_some_and(py::truthy)
    });
    let run: Dict = if exact {
        let u = usage.cloned().unwrap_or_default();
        let get = |k: &str| match u.get(k) {
            Some(v) if py::truthy(v) => py::py_int(v).unwrap_or(0),
            _ => 0,
        };
        let mut r = Dict::new();
        r.insert("input".into(), json!(get("input_tokens")));
        r.insert("output".into(), json!(get("output_tokens")));
        r.insert("cached".into(), json!(get("cache_read_tokens")));
        r
    } else {
        now.iter().map(|(k, v)| (k.clone(), json!((int_of(Some(v)) - int_of(base.get(k))).max(0)))).collect()
    };
    if !session.is_empty() || usage.is_some_and(|u| !u.is_empty()) {
        let tokens: Dict = ["input", "output", "cached"]
            .iter()
            .map(|k| (k.to_string(), json!(int_of(before.get(*k)) + int_of(run.get(*k)))))
            .collect();
        job.insert("tokens".into(), Value::Object(tokens));
    }
}

/// `f"{n:,}"`.
fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// The `propose_task` MCP tool (agent_tools.py). Err: what Python lets through (a task file that cannot be written).
pub async fn propose_tool(jobs: Option<&Jobs>, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let Some(jobs) = jobs else { return Ok(tool_text("background tasks aren't available here", true)) };
    let budget = match arguments.get("budget") {
        Some(v) if py::truthy(v) => py::py_str(v),
        _ => "medium".into(),
    };
    let prompt = tk::str_or_empty(arguments.get("prompt"));
    match jobs.propose(agent, &tk::str_or_empty(arguments.get("title")), tk::head(&prompt, 8000), &budget).await {
        Ok(job) => Ok(tool_text(
            &format!(
                "Proposed (budget about {} tokens). It starts only when the person starts it in « Tâches de fond »: tell them so in your reply.",
                thousands(int_of(job.get("max_tokens")))
            ),
            false,
        )),
        Err(err) => match err.downcast_ref::<JobError>() {
            Some(exc) => Ok(tool_text(&exc.0, true)),
            None => Err(err),
        },
    }
}

/// The bridge's background tasks (one object, made the first time).
pub fn jobs_of(state: &AppState) -> Arc<Jobs> {
    state
        .jobs
        .get_or_init(|| {
            Arc::new(Jobs::new(
                state.config.clone(),
                state.hermes.clone(),
                Some(state.hub.clone()),
                Some(state.push.clone()),
                Some(crate::mail_scan::scans_of(state)),
            ))
        })
        .clone()
}

// -- the app's routes ---------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub prompt: String,
    #[serde(deserialize_with = "web::lax::int")]
    pub max_tokens: i64,
    #[serde(default)]
    pub template: Option<String>,
}

impl Validate for NewTask {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "title", Some(&self.title), 1);
        check_max_len(errors, "title", Some(&self.title), 200);
        check_min_len(errors, "prompt", Some(&self.prompt), 1);
        check_max_len(errors, "prompt", Some(&self.prompt), 8000);
        check_range(
            errors,
            body_loc("max_tokens"),
            self.max_tokens as f64,
            Some(10_000.0),
            Some(50_000_000.0),
            json!(self.max_tokens),
        );
        check_max_len(errors, "template", self.template.as_deref(), 40);
    }
}

#[derive(Debug, Deserialize)]
pub struct Start {
    #[serde(default, deserialize_with = "web::lax::opt_int")]
    pub max_tokens: Option<i64>,
}

impl Validate for Start {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(m) = self.max_tokens {
            check_range(errors, body_loc("max_tokens"), m as f64, Some(10_000.0), Some(50_000_000.0), json!(m));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Resume {
    #[serde(default, deserialize_with = "web::lax::opt_int")]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
}

impl Validate for Resume {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(m) = self.max_tokens {
            check_range(errors, body_loc("max_tokens"), m as f64, Some(10_000.0), Some(200_000_000.0), json!(m));
        }
        check_max_len(errors, "message", self.message.as_deref(), 2000);
    }
}

/// A JobError → 404 (« unknown job ») or 409; anything else → a 500.
fn refused(err: anyhow::Error) -> HttpError {
    match err.downcast_ref::<JobError>() {
        Some(exc) => HttpError::new(if exc.0 == "unknown job" { 404 } else { 409 }, exc.0.clone()),
        None => err.into(),
    }
}

async fn listing(State(state): State<AppState>) -> ApiResult<Response> {
    let jobs = jobs_of(&state).list();
    Ok(web::ok(json!({"jobs": jobs, "budgets": budgets()})))
}

async fn create_route(
    State(state): State<AppState>,
    UrlPath(agent_id): UrlPath<String>,
    Json(body): Json<NewTask>,
) -> ApiResult<Response> {
    let Some(agent) = state.config().agent(Some(&agent_id)).cloned() else {
        return Err(HttpError::new(404, "unknown agent"));
    };
    let job = jobs_of(&state)
        .create(&agent, &body.title, &body.prompt, body.max_tokens, body.template.as_deref())
        .await
        .map_err(refused)?;
    Ok(web::ok(Value::Object(job)))
}

async fn start_route(
    State(state): State<AppState>,
    UrlPath(job_id): UrlPath<String>,
    OptionalJson(body): OptionalJson<Start>,
) -> ApiResult<Response> {
    let job = jobs_of(&state).start(&job_id, body.and_then(|b| b.max_tokens)).await.map_err(refused)?;
    Ok(web::ok(Value::Object(job)))
}

async fn pause_route(State(state): State<AppState>, UrlPath(job_id): UrlPath<String>) -> ApiResult<Response> {
    let job = jobs_of(&state).pause(&job_id).await.map_err(refused)?;
    Ok(web::ok(Value::Object(job)))
}

async fn resume_route(
    State(state): State<AppState>,
    UrlPath(job_id): UrlPath<String>,
    OptionalJson(body): OptionalJson<Resume>,
) -> ApiResult<Response> {
    let (max_tokens, message) = match body {
        Some(b) => (b.max_tokens, b.message),
        None => (None, None),
    };
    let job = jobs_of(&state).resume(&job_id, max_tokens, message.as_deref()).await.map_err(refused)?;
    Ok(web::ok(Value::Object(job)))
}

async fn forget_route(State(state): State<AppState>, UrlPath(job_id): UrlPath<String>) -> ApiResult<Response> {
    jobs_of(&state).forget(&job_id).await.map_err(refused)?;
    Ok(web::ok(json!({"ok": true})))
}

/// `jobs_router()` (mounted under /v1 with the bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/jobs", get(listing))
        .route("/agents/{agent_id}/jobs", post(create_route))
        .route("/jobs/{job_id}/start", post(start_route))
        .route("/jobs/{job_id}/pause", post(pause_route))
        .route("/jobs/{job_id}/resume", post(resume_route))
        .route("/jobs/{job_id}", delete(forget_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_counters() {
        assert_eq!(thousands(300_000), "300,000");
        assert_eq!(thousands(999), "999");
        let session: Dict =
            serde_json::from_value(json!({"input_tokens": 12.7, "output_tokens": true, "cache_read_tokens": 3}))
                .unwrap();
        assert_eq!(Value::Object(counters(&session)), json!({"input": 12, "output": 0, "cached": 3}));
    }
}
