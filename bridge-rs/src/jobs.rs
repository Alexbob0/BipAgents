//! Background jobs (« Tâches de fond », decided 2026-10-21): long work an agent does apart from the person's
//! conversations, with its cost shown before (a budget in euros the person chooses) and after (what it took), pause, resume
//! and cancel, and a notification when it ends. Two kinds:
//!
//!     mail_scan   a mailbox read through by steps (mail_scan.rs): listed and paused here, run there
//!     task        a long task in the agent's own Hermes, with all its tools, in a conversation of its own (« 🧩 <title> »):
//!                 the bridge starts the run, reads its tokens every few seconds (each model call's, Hermes patch 0020;
//!                 else the conversation's counters) and stops the run
//!                 once the budget is reached (paused: the person may give more and resume) or after three hours; the
//!                 agent ends with a report, which the notification opens. With patch 0020 the run hub also stops it
//!                 before a model call that would pass what is left of its budget (runs.rs `cap`): the conversation's
//!                 counters move only at a run's end, so before that a run went on to its end whatever it took (Alex's
//!                 task of 2026-10-31: about twice its budget)
//!
//! A task's budget and spend are euros counted as the month's AI credit counts them (task_cost.rs, 2026-11-04): its
//! calls' fresh input, cache reads and output at the router's prices for the agent's model (`budget_eur`, `eur`,
//! `prices` on the task). `max_tokens` in a task's view is that budget in tokens without the cache, for a home
//! install's screens (its model is the person's own: tokens, not euros, there).
//!
//! An agent may propose a task (MCP `propose_task` in agent_tools.rs, for work too long for a reply): it waits, budget and all, until the
//! person starts it in the app. Since 2026-10-22 (owner's decision: long work never inline) an agent also starts a task
//! itself (`start_task`, `notebook_fill`), but only for work the person just asked for and within a small budget
//! (`[jobs] direct_budget_eur`, ≈ 0.20 €); anything bigger,
//! or its own idea, is proposed. The task is marked `proposed_by: "asked"`.
//!
//!     GET    /v1/jobs                        every job of every agent: proposals first, then the newest
//!     GET    /v1/jobs                        … and `budgets_eur` {small, medium, large}, `budgets_tokens` (home)
//!     POST   /v1/agents/{id}/jobs            {title, prompt, budget_eur, template?}   a task started now
//!     POST   /v1/jobs/{id}/start             {budget_eur?}       a proposed task started
//!     POST   /v1/jobs/{id}/pause
//!     POST   /v1/jobs/{id}/resume            {budget_eur?, message?}   a bigger budget (its new total), the words
//!                                            that resume it (a mail scan: {max_tokens?})
//!     DELETE /v1/jobs/{id}                   stopped if it runs, then forgotten (its conversation and pages stay)
//!
//! Mail scans keep their own routes for their steps (/v1/agents/{id}/mail-scan…); their job id here is
//! `mail-scan:<agent>`.

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
use crate::background_turn::{End, Kind, NotRun, Pipeline, Turn};
use crate::hermes::{HermesClient, HermesError};
use crate::mail_scan::{self, ScanError, Scans};
use crate::push::PushService;
use crate::runs::RunHub;
use crate::task_cost::{self, Prices};
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
/// A task paused because the place was too full (heavy_work.rs).
pub const SPACE_REASON: &str = "space";
/// Seconds a stop may take before the task is counted as paused anyway.
pub const STOP_GRACE: f64 = 120.0;
/// Finished tasks kept in the list, per agent.
pub const KEPT_PER_AGENT: usize = 40;
pub const REPORT_CHARS: usize = 4000;
/// The budgets the person picks and an agent proposes, in euros of the month's credit.
pub const BUDGETS: [(&str, f64); 3] = [("small", 0.20), ("medium", 0.50), ("large", 1.50)];
pub const TERMINAL: [&str; 5] = ["completed", "failed", "cancelled", "canceled", "interrupted"];
pub const TITLE_MARK: &str = "🧩 ";
pub const INSTRUCTIONS: &str =
    "This is a background task the person gave you from the app (« Tâches de fond »): they are not following
this conversation. Work through it on your own, step by step, with your tools. Don't ask questions unless you truly
cannot go on: choose sensible defaults and say which in your report. Keep what you make where the person finds it: a
page (create_page) for a table, a comparison or a dashboard; a file you make (a deck, a Word document, a spreadsheet, a
PDF, each version or style asked) given with give_file, and the MEDIA line it gives you put in your report. Your work
has a budget; if you are told to continue, pick up where you stopped instead of starting again. End with a short
report in the person's language: what you did, what you found or made and where it is, what is left or uncertain.";
pub const CONTINUE: &str = "Continue the task where you stopped (don't start again), then end with your report.";

/// The task's own drafts folder, for an agent whose tools run in Hermes' docker sandbox (its persistent
/// `/workspace`): space_tidy.rs empties it `[tidy] task_hours` after the task ends. None: no such sandbox.
pub fn drafts_line(config: &crate::config::Config, agent: &crate::config::AgentConfig, job_id: &str) -> Option<String> {
    let profile = crate::shares::profile_dir(config, agent)?;
    if crate::shares::terminal_backend(&profile).as_deref() != Some("docker") || !JOB_ID.is_match(job_id) {
        return None;
    }
    Some(format!(
        "Your working files for this task (drafts, data you process, test scripts) go in {}/{job_id}: it is \
         emptied a day after the task ends; what you give with give_file or create_page stays.",
        crate::space_tidy::SCRATCH_SEEN
    ))
}

/// The pushes' bodies in French; in the agent's language they come from names.rs (`push_body`).
pub const DONE_BODY: &str = "Tâche de fond terminée";
pub const PAUSED_BODY: &str = "Tâche de fond en pause";
pub const PROPOSED_BODY: &str = "Tâche de fond proposée";

/// A push's body (`DONE_BODY`…) in `agent`'s language: what the phone shows, as it is (Android), or when the iOS
/// catalog has no such key.
fn push_body(config: &crate::config::Config, agent: &AgentConfig, body: &str) -> &'static str {
    let n = crate::names::names(&crate::handback::language_of(config, agent));
    match body {
        DONE_BODY => n.task_done,
        PAUSED_BODY => n.task_paused,
        _ => n.task_proposed,
    }
}

/// French puts a space before a colon.
fn colon(body: &str) -> &'static str {
    if body.starts_with("Tâche") { " :" } else { ":" }
}

/// Python's `JobError` (safe to show).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct JobError(pub String);

fn job_error(message: &str) -> anyhow::Error {
    JobError(message.into()).into()
}

fn budget_of(name: &str) -> Option<f64> {
    BUDGETS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// The budgets in euros (`budgets_eur`), and in tokens without the cache at the install's prices (`budgets_tokens`,
/// a home install's screens).
pub fn budgets(config: &crate::config::Config) -> (Value, Value) {
    let prices = task_cost::configured(config);
    (
        Value::Object(BUDGETS.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()),
        Value::Object(BUDGETS.iter().map(|(k, v)| (k.to_string(), json!(prices.tokens_for(*v)))).collect()),
    )
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

/// The prices the task is counted at: its own (set at each run's start), else the install's.
pub fn prices_of(config: &crate::config::Config, job: &Dict) -> Prices {
    Prices::of_job(job).unwrap_or_else(|| task_cost::configured(config))
}

/// The task's budget in euros (a task kept from before 2026-11-04: its tokens at the `[notebook]` prices).
pub fn task_budget(config: &crate::config::Config, job: &Dict) -> f64 {
    match job.get("budget_eur").and_then(Value::as_f64) {
        Some(euros) => euros,
        None => euros_for(config, int_of(job.get("max_tokens"))),
    }
}

/// What the task took so far, in euros (task_cost.rs).
pub fn eur_spent(config: &crate::config::Config, job: &Dict) -> f64 {
    prices_of(config, job).of_tokens(job.get("tokens"))
}

/// Its euros kept on it (`eur`), as the apps read them.
fn set_eur(config: &crate::config::Config, job: &mut Dict) {
    let euros = eur_spent(config, job);
    job.insert("eur".into(), json!((euros * 1e6).round() / 1e6));
}

/// What the task's current run may take, in euros: its budget less what its earlier runs took.
fn left_for_run(config: &crate::config::Config, job: &Dict) -> f64 {
    task_budget(config, job) - prices_of(config, job).of_tokens(job.get("before"))
}

/// A mailbox scan (mail_scan.rs) in the common shape.
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
    const KEEP: [&str; 20] = [
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
        "budget_eur",
        "eur",
        "session_id",
        "report",
        "proposed_by",
        "runs",
        "from_session",
    ];
    // an extraction from an archive (archives/records_fill.rs): its euros, its cap, the documents read
    const EXTRACT: [&str; 5] = ["archive", "progress", "model", "counts", "max_tokens"];
    let extract = crate::archives::records_fill::is_extract(job);
    let mut view: Dict = KEEP.iter()
        .chain(EXTRACT.iter().filter(|_| extract))
        .map(|k| (k.to_string(), job.get(*k).cloned().unwrap_or(Value::Null)))
        .collect();
    if !extract {
        // its budget in tokens without the cache, for a home install's screens (the module's doc)
        let tokens = match (Prices::of_job(job), job.get("budget_eur").and_then(Value::as_f64)) {
            (Some(prices), Some(euros)) => json!(prices.tokens_for(euros)),
            _ => job.get("max_tokens").cloned().unwrap_or(Value::Null),
        };
        view.insert("max_tokens".into(), tokens);
    }
    // its words: the files of the report are in its conversation, never as `MEDIA:` lines here (4/11)
    if let Some(Value::String(report)) = view.get_mut("report") {
        *report = crate::media::without_media_lines(report, |_| true);
    }
    view
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
    /// « Remplir mon Carnet » for every agent, one after another (notebook/fill.rs).
    pub filler: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// A finished task's result handed back to the conversation it came from (handback.rs); None: only the push.
    pub handback: Option<Arc<crate::background_turn::Pipeline>>,
    /// The plan's AI credit as the guichet tells the place (credit.rs); None: the place's own file. Used up: no new
    /// task, a running one stops after its current step.
    pub credit_file: Option<PathBuf>,
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
            filler: Default::default(),
            tell,
            handback: None,
            credit_file: None,
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

    /// Every job, or one agent's (a conversation asks for its agent's only).
    pub fn list(&self, agent: Option<&str>) -> Vec<Dict> {
        let config = self.config.get();
        let mut out: Vec<Dict> = tasks(&config)
            .iter()
            .filter(|job| {
                let owner = agent_of(job);
                config.agent(Some(&owner)).is_some() && agent.is_none_or(|name| name == owner)
            })
            .map(|job| self.view_of(job))
            .collect();
        let names: Vec<String> = match agent {
            Some(name) => vec![name.to_string()],
            None => config.agents.keys().cloned().collect(),
        };
        for name in &names {
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
        load(&self.config.get(), job_id).map(|job| self.view_of(&job)).ok_or_else(|| JobError("unknown job".into()))
    }

    fn view_of(&self, job: &Dict) -> Dict {
        let mut view = task_view(job);
        let state = state_of(job);
        if state == "running" || state == "stopping" {
            let run_id = job.get("run_id").and_then(Value::as_str);
            view.insert("activity".into(), self.activity(&agent_of(job), run_id));
        }
        view
    }

    /// What the task does right now: its run's last tool step (or sub-agent), from the events the hub follows.
    pub fn activity(&self, agent: &str, run_id: Option<&str>) -> Value {
        let (Some(hub), Some(run_id)) = (self.hub.as_ref(), run_id) else { return Value::Null };
        let Some(sub) = hub.subscription(agent, run_id) else { return Value::Null };
        let state = sub.state.lock().unwrap_or_else(|e| e.into_inner());
        // str(value or "") as Python writes it; the preview cut to 160 characters, the tool's name whole.
        let text = |value: Option<&Value>| -> String {
            match value {
                None | Some(Value::Null) | Some(Value::Bool(false)) => String::new(),
                Some(Value::String(s)) => s.clone(),
                Some(other) => py::py_str(other),
            }
        };
        let cut = |value: Option<&Value>| -> String { text(value).chars().take(160).collect() };
        let mut activity = Value::Null;
        for event in state.backlog.iter().rev() {
            match event.r#type.as_str() {
                "tool.started" => {
                    activity = json!({"tool": text(event.data.get("tool")), "preview": cut(event.data.get("preview"))});
                    break;
                }
                "subagent.start" | "subagent.started" => {
                    activity = json!({"tool": "delegate_task", "preview": cut(event.data.get("goal"))});
                    break;
                }
                _ => {}
            }
        }
        // the agent's task list (« Étapes », steps.rs): the card's list and the step being done
        if let Some(steps) = state.steps.as_ref().filter(|s| s["items"].as_array().is_some_and(|i| !i.is_empty())) {
            if activity.is_null() {
                activity = json!({});
            }
            activity["step"] = json!(crate::steps::current(steps));
            activity["steps"] = steps.clone();
        }
        activity
    }

    /// The task's run's real tokens so far, as its model calls end (Hermes patch 0020, followed by the hub), in the
    /// shape of a run's usage: its budget then counts them, not the conversation's counters, which Hermes writes only
    /// at a run's end. None from an older Hermes.
    fn live_usage(&self, agent: &AgentConfig, run_id: &str) -> Option<Dict> {
        let progress = self.hub.as_ref()?.run_progress(&agent.name, run_id)?;
        progress.usage.filter(|u| u.total() > 0).map(|u| u.as_usage())
    }

    /// The hub stopped the task's run before a model call that would have passed its budget (`limit_tokens`).
    fn capped(&self, agent: &AgentConfig, run_id: &str) -> bool {
        self.hub.as_ref().and_then(|hub| hub.run_progress(&agent.name, run_id)).is_some_and(|p| p.capped)
    }

    fn agent(&self, name: &str) -> Result<AgentConfig, JobError> {
        self.config
            .get()
            .agent(Some(name))
            .cloned()
            .ok_or_else(|| JobError("this agent isn't on this install any more".into()))
    }

    // -- a new task ----------------------------------------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn new_task(
        &self,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        budget_eur: f64,
        template: Option<&str>,
        proposed_by: &str,
        from_session: Option<&str>,
    ) -> anyhow::Result<Dict> {
        let title = tk::head(&tk::squeeze(title), 80).to_string();
        let prompt = tk::strip(prompt).to_string();
        if title.is_empty() || prompt.is_empty() {
            return Err(job_error("a task needs a title and what to do"));
        }
        let job = json!({"id": format!("t{}", py::token_hex(6)), "kind": "task", "agent": agent.name, "title": title,
                         "prompt": prompt, "template": template, "state": "proposed", "reason": null, "error": null,
                         "created": py::time_time(), "started": null, "finished": null, "budget_eur": budget_eur,
                         "eur": 0.0, "prices": task_cost::configured(&self.config.get()).to_value(),
                         "tokens": {"input": 0, "output": 0, "cached": 0}, "session_id": null, "run_id": null,
                         "report": null, "proposed_by": proposed_by, "runs": 0,
                         "from_session": from_session.filter(|s| !s.is_empty())});
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
        budget_eur: f64,
        template: Option<&str>,
    ) -> anyhow::Result<Dict> {
        self.create_by(agent, title, prompt, budget_eur, template, "person").await
    }

    /// A task started now, by the person (`person`) or by the agent at the person's request (`asked`: start_task,
    /// notebook_fill, within the direct budget).
    pub async fn create_by(
        self: &Arc<Self>,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        budget_eur: f64,
        template: Option<&str>,
        by: &str,
    ) -> anyhow::Result<Dict> {
        self.create_from(agent, title, prompt, budget_eur, template, by, None).await
    }

    /// `create_by`, from a conversation (`from_session`): the task's result is handed back there when it ends.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_from(
        self: &Arc<Self>,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        budget_eur: f64,
        template: Option<&str>,
        by: &str,
        from_session: Option<&str>,
    ) -> anyhow::Result<Dict> {
        self.may_run(&agent.name)?;
        let mut job = self.new_task(agent, title, prompt, budget_eur, template, by, from_session)?;
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
        self.propose_from(agent, title, prompt, budget, None).await
    }

    /// `propose`, from a conversation (`from_session`): once started and done, its result goes back there.
    pub async fn propose_from(
        &self,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        budget: &str,
        from_session: Option<&str>,
    ) -> anyhow::Result<Dict> {
        let job = self.propose_quiet(agent, title, prompt, budget, from_session).await?;
        self.tell_proposed(agent, &job).await;
        Ok(job)
    }

    /// The person told of a proposal (« Tâche de fond proposée »).
    pub async fn tell_proposed(&self, agent: &AgentConfig, job: &Dict) {
        if let Some(tell) = &self.tell {
            let body = push_body(&self.config.get(), agent, PROPOSED_BODY);
            let line = format!("{body}{} {}", colon(body), tk::str_or_empty(job.get("title")));
            tell(agent.name.clone(), None, line, tk::str_or_empty(job.get("id")), body.into()).await;
        }
    }

    /// `propose_from` without telling the person (approval_level.rs starts it at once, or tells them itself).
    pub async fn propose_quiet(
        &self,
        agent: &AgentConfig,
        title: &str,
        prompt: &str,
        budget: &str,
        from_session: Option<&str>,
    ) -> anyhow::Result<Dict> {
        let waiting =
            tasks(&self.config.get()).iter().filter(|j| agent_of(j) == agent.name && state_of(j) == "proposed").count();
        if waiting >= PROPOSALS_MAX {
            return Err(job_error("enough of your proposals are waiting for the person already"));
        }
        let euros = budget_of(budget).or_else(|| budget_of("medium")).unwrap_or(0.5);
        let mut job = self.new_task(agent, title, prompt, euros, None, "agent", from_session)?;
        job.insert("prices".into(), task_cost::for_agent(&self.config.get(), agent).await.to_value());
        save(&self.config.get(), &job)?;
        Ok(job)
    }

    pub async fn start(self: &Arc<Self>, job_id: &str, budget_eur: Option<f64>) -> anyhow::Result<Dict> {
        let job = load(&self.config.get(), job_id);
        let Some(mut job) = job.filter(|j| state_of(j) == "proposed") else {
            return Err(job_error("this task isn't waiting to start"));
        };
        if crate::archives::records_fill::is_extract(&job) {
            if crate::archives::records_fill::running_for(&self.config.get(), &agent_of(&job)).is_some() {
                return Err(job_error("this agent already fills its records from an archive"));
            }
            return crate::archives::records_fill::resume(self.config.get(), job_id, None, self.tell.clone());
        }
        self.may_run(&agent_of(&job))?;
        if let Some(euros) = budget_eur.filter(|e| *e > 0.0) {
            job.insert("budget_eur".into(), json!(euros));
        }
        let text = format!("{}\n\n{}", tk::str_or_empty(job.get("title")), tk::str_or_empty(job.get("prompt")));
        self.launch(&mut job, &text).await?;
        Ok(task_view(&job))
    }

    /// The turns' pipeline (background_turn.rs) with this board's services.
    fn pipeline(&self) -> Pipeline {
        Pipeline {
            hub: self.hub.clone(),
            push: self.push.clone(),
            credit_file: self.credit_file.clone(),
            ..Pipeline::new(self.config.clone(), self.hermes.clone())
        }
    }

    pub(crate) fn may_run(&self, agent: &str) -> Result<(), JobError> {
        if self.credit_used() {
            return Err(JobError(crate::credit::JOBS_REFUSED.into()));
        }
        // (a place too full or pressed doesn't refuse: the task is queued at its start, `launch`, heavy_work.rs)
        let running: Vec<Dict> = tasks(&self.config.get())
            .into_iter()
            .filter(|j| ["running", "stopping"].contains(&state_of(j).as_str()))
            .filter(|j| !crate::archives::records_fill::is_extract(j)) // no Hermes run: apart
            .collect();
        if running.iter().any(|j| agent_of(j) == agent) {
            return Err(JobError("this agent is already on a background task".into()));
        }
        if running.len() >= RUNNING_MAX {
            return Err(JobError("enough background tasks are running here for now".into()));
        }
        Ok(())
    }

    /// The month's AI credit used (credit.rs): what runs by itself pauses.
    pub fn credit_used(&self) -> bool {
        crate::credit::now_at(self.credit_file.as_deref()).paused()
    }

    /// The place too full (heavy_work.rs's backstop): every running task stopped at its next step, paused with the
    /// reason « space ». How many.
    pub async fn pause_for_space(&self) -> usize {
        let mut count = 0;
        for mut job in tasks(&self.config.get()) {
            if state_of(&job) != "running" || crate::archives::records_fill::is_extract(&job) {
                continue;
            }
            let Ok(agent) = self.agent(&agent_of(&job)) else { continue };
            self.stop(&agent, &mut job, SPACE_REASON).await;
            count += 1;
        }
        count
    }

    /// Room again: the tasks paused or queued for it go on by themselves, each once its agent's check clears
    /// (heavy_work.rs `task_gate`; checked each minute).
    pub async fn resume_after_space(self: &Arc<Self>, config: &crate::config::Config) {
        for job in tasks(config) {
            if state_of(&job) == "paused" && job.get("reason") == Some(&json!(SPACE_REASON)) {
                let (id, agent) = (tk::str_or_empty(job.get("id")), agent_of(&job));
                let c = self.config.get();
                let waits = tokio::task::spawn_blocking(move || crate::heavy_work::task_waits(&c, &agent))
                    .await
                    .unwrap_or(Some("busy"));
                if waits.is_some() {
                    continue;
                }
                match self.resume(&id, None, None).await {
                    Ok(_) => tracing::info!(job = %id, "task started again: there is room"),
                    Err(e) => tracing::debug!(job = %id, error = %format!("{e:#}"), "task not resumed after the space"),
                }
            }
        }
    }

    /// `heavy_work` answered « later » to the agent's own background task: that task stopped at its next step,
    /// paused for room (started again by `resume_after_space`). False when the agent has none running.
    pub async fn pause_running_for_space(&self, agent: &str) -> bool {
        let Some(mut job) = tasks(&self.config.get()).into_iter().find(|j| {
            state_of(j) == "running" && agent_of(j) == agent && !crate::archives::records_fill::is_extract(j)
        }) else {
            return false;
        };
        let Ok(agent) = self.agent(agent) else { return false };
        self.stop(&agent, &mut job, SPACE_REASON).await;
        true
    }

    /// The credit back (a pack, the new month): the tasks and mail scans paused for it go on by themselves.
    pub async fn resume_after_credit(self: &Arc<Self>, config: &crate::config::Config) {
        for job in tasks(config) {
            if state_of(&job) == "paused" && job.get("reason") == Some(&json!(crate::credit::REASON)) {
                let id = tk::str_or_empty(job.get("id"));
                if let Err(e) = self.resume(&id, None, None).await {
                    tracing::warn!(job = %id, error = %format!("{e:#}"), "task not resumed after the credit");
                }
            }
        }
        if let Some(scans) = &self.scans {
            for agent in config.agents.values() {
                let paused = mail_scan::load(config, &agent.name).is_some_and(|j| {
                    j.get("state") == Some(&json!("paused")) && j.get("error") == Some(&json!(crate::credit::REASON))
                });
                if paused {
                    if let Err(e) = scans.resume(config, agent, None) {
                        tracing::warn!(agent = %agent.name, error = %format!("{e:#}"), "mail scan not resumed");
                    }
                }
            }
        }
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
        if crate::place_move::moving(&self.config.get()) {
            // The place is moving (place_move.rs): queued, started on the new server with these words.
            job.insert("state".into(), json!("paused"));
            job.insert("reason".into(), json!("moving"));
            job.insert("queued_text".into(), json!(text));
            save(&self.config.get(), job)?;
            tracing::info!(agent = %agent.name, job = %tk::str_or_empty(job.get("id")), "job queued: place moving");
            return Ok(());
        }
        let (c, name) = (self.config.get(), agent.name.clone());
        if let Some(why) = tokio::task::spawn_blocking(move || crate::heavy_work::task_waits(&c, &name)).await? {
            // The place's room as heavy_work reads it (heavy_work.rs): queued, started by itself once there is room.
            job.insert("state".into(), json!("paused"));
            job.insert("reason".into(), json!(SPACE_REASON));
            job.insert("queued_text".into(), json!(text));
            save(&self.config.get(), job)?;
            tracing::info!(agent = %agent.name, job = %tk::str_or_empty(job.get("id")), why, "job queued: no room now");
            return Ok(());
        }
        let refused = |err: HermesError| -> anyhow::Error {
            match &err {
                HermesError::Http { status, .. } => JobError(format!("the agent refused the task ({status})")).into(),
                HermesError::Decode(_) => err.into(),
                _ => job_error("the agent can't be reached"),
            }
        };
        // its prices: the router's for its model now (task_cost.rs), its earlier runs counted at them too
        job.insert("prices".into(), task_cost::for_agent(&self.config.get(), &agent).await.to_value());
        let pipeline = self.pipeline();
        // Hermes refuses a title another conversation holds (a task asked again after it was cancelled): numbered
        let kept = tk::str_or_empty(job.get("session_id"));
        let title = format!("{TITLE_MARK}{}", tk::str_or_empty(job.get("title")));
        let session_id = if kept.is_empty() { pipeline.work_session(&agent, &title, "").await.map_err(refused)? } else { kept };
        job.insert("session_id".into(), json!(session_id));
        let base = counters(&self.hermes.session(&agent, &session_id).await.map_err(refused)?);
        // the task's rules in its message (a note the apps hide), never as run instructions: the system prompt stays
        // the agent's conversations' own, and so does the model server's prompt cache; « Tâches de fond » by its name
        // in the agent's language (names.rs)
        let mut rules = crate::names::localize(INSTRUCTIONS, &crate::handback::language_of(&self.config.get(), &agent));
        // a folder of its own for its working files, emptied a day after it ends (space_tidy.rs)
        if let Some(line) = drafts_line(&self.config.get(), &agent, &tk::str_or_empty(job.get("id"))) {
            rules = format!("{rules}\n{line}");
        }
        let mut turn = Turn::new(&agent, Kind::Task, &session_id, format!("{text}\n\n<!--bip:task-rules\n{rules}\n-->"));
        // GLM through EUrouter thinks at « low » in chat since 2026-10-28; tasks stay at « medium » until measured.
        turn.options = crate::agents_admin::task_turn_options(&self.config.get(), &agent);
        turn.busy_wait = 0.0; // its own conversation: a route never waits here
        let run_id = match pipeline.start(&turn).await {
            Ok((run_id, _)) => run_id,
            Err(NotRun::Credit) => return Err(JobError(crate::credit::JOBS_REFUSED.into()).into()),
            Err(NotRun::Refused(err)) => return Err(refused(err)),
            Err(NotRun::Gone) => return Err(refused(HermesError::http(404, ""))),
            Err(why) => return Err(JobError(format!("the agent refused the task ({why})")).into()),
        };
        // this run may take its budget less what the earlier runs took (its `before`, set below)
        let left = task_budget(&self.config.get(), job) - eur_spent(&self.config.get(), job);
        if let Some(hub) = &self.hub {
            hub.limit_cost(&agent.name, &run_id, prices_of(&self.config.get(), job), left);
        }
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
        let id = tk::str_or_empty(job.get("id"));
        self.follow(&id);
        tracing::info!(agent = %agent.name, job = %id, budget_eur = task_budget(&self.config.get(), job), "job started");
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
            if !(state == "running" || state == "stopping") || crate::archives::records_fill::is_extract(&job) {
                continue;
            }
            let Some(agent) = config.agent(Some(&agent_of(&job))) else { continue };
            let (run_id, session) = (tk::str_or_empty(job.get("run_id")), tk::str_or_empty(job.get("session_id")));
            if !run_id.is_empty() {
                self.pipeline().follow(agent, Kind::Task, &run_id, &session, None);
                if let Some(hub) = &self.hub {
                    hub.limit_cost(&agent.name, &run_id, prices_of(&config, &job), left_for_run(&config, &job));
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
        if let Some(task) = self.filler.lock().unwrap_or_else(|e| e.into_inner()).take() {
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
            let (run_id, session_id) = (tk::str_or_empty(job.get("run_id")), tk::str_or_empty(job.get("session_id")));
            let end = self.pipeline().status(&agent, &run_id, &session_id).await;
            match self.hermes.session(&agent, &session_id).await {
                Ok(session) => {
                    count(&mut job, &session, self.live_usage(&agent, &run_id).as_ref());
                    set_eur(&config, &mut job);
                }
                Err(HermesError::Decode(err)) => {
                    tracing::error!(job = %job_id, exc = %err, "Task exception was never retrieved");
                    return;
                }
                Err(_) => {}
            }
            if let Some(end) = end {
                self.ended(&agent, &mut job, end).await;
                return;
            }
            let now = py::time_time();
            if state_of(&job) == "running" {
                let step_started = job.get("step_started").map(|v| py::py_float(v).unwrap_or(now)).unwrap_or(now);
                if eur_spent(&config, &job) >= task_budget(&config, &job) || self.capped(&agent, &run_id) {
                    self.stop(&agent, &mut job, "budget").await;
                } else if now - step_started >= self.max_seconds {
                    self.stop(&agent, &mut job, "time").await;
                } else if self.credit_used() {
                    // the month's credit used: the run stops after its current step, paused until the credit is back
                    self.stop(&agent, &mut job, crate::credit::REASON).await;
                }
            } else {
                let asked = job.get("stop_asked").map(|v| py::py_float(v).unwrap_or(now)).unwrap_or(now);
                if now - asked >= STOP_GRACE {
                    // Hermes never confirmed the stop
                    self.ended(&agent, &mut job, End { status: "cancelled".into(), ..End::default() }).await;
                    return;
                }
            }
            // The person paused (or the task was stopped) while this round waited on Hermes: their change stands,
            // the counters come again next round.
            let changed = load(&config, job_id)
                .is_some_and(|latest| state_of(&latest) != state || latest.get("stop_asked") != job.get("stop_asked"));
            if changed && state_of(&job) == state {
                continue;
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

        async fn ended(&self, agent: &AgentConfig, job: &mut Dict, end: End) {
        let End { mut status, output, usage, error } = end;
        let usage = match usage {
            Some(Value::Object(u)) if !u.is_empty() => Some(u),
            // a run stopped (its budget, a pause) comes back without its usage, one lost in a Hermes restart with an
            // empty one (`{}`, hermes.rs `resolve_unknown`; lab 2026-11-01: its 21.9k counted as the conversation's
            // 1.4k): its calls' real tokens
            _ => self.live_usage(agent, &tk::str_or_empty(job.get("run_id"))),
        };
        let session_id = tk::str_or_empty(job.get("session_id"));
        let session = self.hermes.session(agent, &session_id).await.unwrap_or_default();
        count(job, &session, usage.as_ref());
        set_eur(&self.config.get(), job);
        if status == "gone" {
            status = if output.is_empty() { "interrupted".into() } else { "completed".into() };
        }
        let now = py::time_time();
        let mut full = String::new();
        // the hub stopped it before a call past its budget, before this watch did (a run that still ended with its
        // answer is done)
        let capped = (status != "completed" && self.capped(agent, &tk::str_or_empty(job.get("run_id"))))
            .then(|| json!("budget"));
        let stopping = job.get("stopping").filter(|v| py::truthy(v)).cloned().or(capped);
        if stopping.is_some() || ["cancelled", "canceled", "interrupted"].contains(&status.as_str()) {
            job.insert("state".into(), json!("paused"));
            job.insert("reason".into(), stopping.unwrap_or(json!("interrupted")));
        } else if status == "completed" {
            full = tk::strip(&output).to_string();
            let report = tk::head(&full, REPORT_CHARS).to_string();
            job.insert("state".into(), json!("done"));
            job.insert("report".into(), if report.is_empty() { Value::Null } else { json!(report) });
            job.insert("finished".into(), json!(now));
        } else {
            let error = if error.is_empty() { "the task failed" } else { error.as_str() };
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
            reason = ?crate::logs::J(&reason), tokens = spent(job), eur = eur_spent(&self.config.get(), job), "job ended");
        let from = job.get("from_session").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        if let (true, Some(from), Some(handback)) = (state_of(job) == "done", from, self.handback.clone()) {
            self.hand_back(&handback, agent, job, &from, &full).await;
        } else if state_of(job) == "done" || reason == json!("budget") || reason == json!("time") {
            self.tell_end(agent, job).await;
        }
    }

    /// The task's result told in the conversation it came from: the agent's own message there (or the template), the
    /// full report kept as one of its pages and linked — unless the task made a page of its own, which is then the one
    /// page shown —, the turn's tokens counted in the task's.
    async fn hand_back(
        &self,
        handback: &crate::background_turn::Pipeline,
        agent: &AgentConfig,
        job: &mut Dict,
        from: &str,
        report: &str,
    ) {
        use crate::handback as hb;
        let config = self.config.get();
        let id = tk::str_or_empty(job.get("id"));
        let title = tk::str_or_empty(job.get("title"));
        let lang = hb::language_of(&config, agent);
        let (_, mut links) = crate::media::extract_media(report);
        links.dedup();
        links.truncate(hb::LINKS_MAX - 1);
        // a task that made its own page delivers it: no « Rapport — » page beside it saying the same (2026-10-31, the
        // Pyla's starred tables shown three times in one message)
        if !tk::strip(report).is_empty() && !links.iter().any(|l| hb::is_page(l)) {
            let page_title = hb::report_title(&lang, &title);
            let html = hb::report_html(&page_title, report);
            match crate::pages::save(&config, &agent.name, &html, &page_title, Some(&format!("rapport-{id}"))) {
                Ok(page) => links.push(tk::str_or_empty(page.get("path"))),
                Err(err) => tracing::warn!(agent = %agent.name, job = %id, error = %format!("{err:#}"),
                    "handback: the report page could not be kept"),
            }
        }
        let mut request = hb::Request::new(agent, from, id.clone(), &title, hb::note("task", &title, report, &links));
        request.links = links.clone();
        // what is left of its budget, in tokens at its prices without the cache (handback.rs counts tokens)
        let (prices, left) = (prices_of(&config, job), task_budget(&config, job) - eur_spent(&config, job));
        request.budget = Some(if left >= 0.0 { prices.tokens_for(left) } else { -prices.tokens_for(-left) });
        request.fallback =
            hb::template(&lang, &title, &hb::key_points(report, hb::POINTS_MAX, &lang, hb::today(&config)), &links);
        request.recap = Some(hb::recap(&title, report));
        request.fallback_session = job.get("session_id").and_then(Value::as_str).map(str::to_string);
        let outcome = hb::deliver(handback, &request).await;
        // the turn is part of the task's cost
        let mut latest = load(&config, &id).unwrap_or_else(|| job.clone());
        if outcome.tokens > 0 {
            let mut tokens = latest.get("tokens").and_then(Value::as_object).cloned().unwrap_or_default();
            let input = int_of(tokens.get("input")) + outcome.tokens;
            tokens.insert("input".into(), json!(input));
            latest.insert("tokens".into(), Value::Object(tokens));
            set_eur(&config, &mut latest);
        }
        latest.insert(
            "handed_back".into(),
            json!({"session": from, "by_agent": outcome.by_agent, "why": outcome.why, "at": py::time_time(),
                "links": links}),
        );
        let _ = save(&config, &latest);
        *job = latest;
    }

    async fn tell_end(&self, agent: &AgentConfig, job: &Dict) {
        let Some(tell) = &self.tell else { return };
        let body = push_body(&self.config.get(), agent, if state_of(job) == "done" { DONE_BODY } else { PAUSED_BODY });
        let line = format!("{body}{} {}", colon(body), tk::str_or_empty(job.get("title")));
        let session = job.get("session_id").filter(|v| !v.is_null()).map(py::py_str);
        tell(agent.name.clone(), session, line, tk::str_or_empty(job.get("id")), body.into()).await;
    }

    // -- the person's controls -----------------------------------------------------------------------------------------

    pub async fn pause(&self, job_id: &str) -> anyhow::Result<Dict> {
        if job_id.starts_with("mail-scan:") {
            return self.scan(job_id, "pause", None);
        }
        let Some(mut job) = load(&self.config.get(), job_id) else { return Err(job_error("unknown job")) };
        if crate::archives::records_fill::is_extract(&job) {
            return crate::archives::records_fill::pause(&self.config.get(), job_id);
        }
        if state_of(&job) == "running" {
            let agent = self.agent(&agent_of(&job))?;
            self.stop(&agent, &mut job, "stopped").await;
        }
        Ok(task_view(&job))
    }

    /// A planned move (place_move.rs): a running task stopped at its next step (Hermes interrupts between tool calls),
    /// then paused with the reason « moving ». False when it wasn't running.
    pub async fn pause_for_move(&self, job_id: &str) -> anyhow::Result<bool> {
        let Some(mut job) = load(&self.config.get(), job_id) else { return Ok(false) };
        if state_of(&job) != "running" || crate::archives::records_fill::is_extract(&job) {
            return Ok(false);
        }
        let agent = self.agent(&agent_of(&job))?;
        self.stop(&agent, &mut job, "moving").await;
        Ok(true)
    }

    /// After the move: a task paused or queued for it goes on (its first words if it never started).
    pub async fn resume_after_move(self: &Arc<Self>, job_id: &str) -> anyhow::Result<Dict> {
        let Some(mut job) = load(&self.config.get(), job_id) else { return Err(job_error("unknown job")) };
        if state_of(&job) != "paused" || job.get("reason") != Some(&json!("moving")) {
            return Err(job_error("nothing to resume"));
        }
        let text = match job.remove("queued_text") {
            Some(Value::String(text)) if !text.is_empty() => text,
            _ => CONTINUE.to_string(),
        };
        self.may_run(&agent_of(&job))?;
        self.launch(&mut job, &text).await?;
        Ok(task_view(&job))
    }

    /// A task resumed, with its new budget in euros (its new total) when given.
    pub async fn resume(
        self: &Arc<Self>,
        job_id: &str,
        budget_eur: Option<f64>,
        message: Option<&str>,
    ) -> anyhow::Result<Dict> {
        self.resume_with(job_id, None, message, budget_eur).await
    }

    /// `resume`: a mail scan's new step budget in tokens (`max_tokens`), a task's or an extraction's in euros.
    pub async fn resume_with(
        self: &Arc<Self>,
        job_id: &str,
        max_tokens: Option<i64>,
        message: Option<&str>,
        budget_eur: Option<f64>,
    ) -> anyhow::Result<Dict> {
        if job_id.starts_with("mail-scan:") {
            return self.scan(job_id, "resume", max_tokens);
        }
        if load(&self.config.get(), job_id).is_some_and(|j| crate::archives::records_fill::is_extract(&j)) {
            return crate::archives::records_fill::resume(self.config.get(), job_id, budget_eur, self.tell.clone());
        }
        let job = load(&self.config.get(), job_id);
        let Some(mut job) = job.filter(|j| ["paused", "failed", "done"].contains(&state_of(j).as_str())) else {
            return Err(job_error("nothing to resume"));
        };
        self.may_run(&agent_of(&job))?;
        let config = self.config.get();
        let spent_now = eur_spent(&config, &job);
        match budget_eur.filter(|e| *e > 0.0) {
            Some(euros) => {
                job.insert("budget_eur".into(), json!(euros.max(spent_now + 0.01)));
            }
            None if spent_now >= task_budget(&config, &job) => {
                return Err(job_error("the budget is used up: give a bigger one"))
            }
            None => {}
        }
        let words = tk::head(tk::strip(message.unwrap_or("")), 2000).to_string();
        // a task queued before its first run (no room then: `launch`) starts with its own words
        let queued = match job.remove("queued_text") {
            Some(Value::String(text)) if !text.is_empty() => Some(text),
            _ => None,
        };
        let text = if !words.is_empty() { words } else { queued.unwrap_or_else(|| CONTINUE.to_string()) };
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
        // an extraction's worker stops when it finds its file gone
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

/// A task's prompt shorter than this holds no task (`propose_tool`).
pub(crate) const MIN_PROMPT: usize = 8;

/// `propose_task` called with no task in it: the tools such a call was meant for, by name.
const NOT_A_TASK: &str = "Nothing proposed: prompt must say the whole task (the goal, what the person said, what to \
make). If you meant something else, call that tool instead: a password, API key or code the person must give → \
mcp__bipagents_connectors__ask_secret; the sites a connector needs → mcp__bipagents_files__sandbox_sites_request; a \
connector you built → mcp__bipagents_connectors__connector_register (behind the tool search: tool_call with its name).";

/// The `propose_task` MCP tool (agent_tools.rs). Err: what Python lets through (a task file that cannot be written).
pub async fn propose_tool(jobs: Option<&Jobs>, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let Some(jobs) = jobs else {
        return Ok(crate::tools_knowledge::tool_failure(
            "background tasks aren't available here",
            "I can't keep working on this in the background here.",
        ));
    };
    let budget = match arguments.get("budget") {
        Some(v) if py::truthy(v) => py::py_str(v),
        _ => "medium".into(),
    };
    let prompt = tk::str_or_empty(arguments.get("prompt"));
    if prompt.trim().chars().count() < MIN_PROMPT {
        // no task in it: a call meant for another tool (5/11, token lab, GLM-5.3 Flash: asked for a connector, the
        // model ended ask_secret / sandbox_sites_request, then behind the search, on this in-view name with
        // {"prompt": "No", "title": "x"}; tasks « x » were proposed to the person and the turn went round in circles)
        return Ok(tool_text(NOT_A_TASK, true));
    }
    let from = jobs.hub.as_ref().and_then(|hub| hub.asking_session(&agent.name));
    let title = tk::str_or_empty(arguments.get("title"));
    match jobs.propose_from(agent, &title, tk::head(&prompt, 8000), &budget, from.as_deref()).await {
        Ok(job) => Ok(tool_text(
            &format!(
                "Proposed (budget about {}). It starts only when the person starts it in « Tâches de fond »: tell them so in your reply.",
                budget_text(&jobs.config.get(), task_budget(&jobs.config.get(), &job), crate::handover::on_offer())
            ),
            false,
        )),
        Err(err) => match err.downcast_ref::<JobError>() {
            Some(exc) => Ok(tool_text(&exc.0, true)),
            None => Err(err),
        },
    }
}

/// Euros per token of an agent's work at the `[notebook]` prices: mostly reading (its context again at each step), so
/// 95 % in and 5 % out, as the apps count a budget's ceiling (JobText.ceiling).
fn euros_per_token(config: &crate::config::Config) -> f64 {
    (0.95 * config.notebook.price_in + 0.05 * config.notebook.price_out).max(0.0) / 1_000_000.0
}

/// Tokens for `euros` of an agent's work (at least 10,000, the routes' smallest budget).
pub fn tokens_for(config: &crate::config::Config, euros: f64) -> i64 {
    let per_token = euros_per_token(config);
    if per_token <= 0.0 {
        return 300_000;
    }
    ((euros.max(0.0) / per_token) as i64).clamp(10_000, 50_000_000)
}

/// Euros for `tokens` of an agent's work.
pub fn euros_for(config: &crate::config::Config, tokens: i64) -> f64 {
    tokens as f64 * euros_per_token(config)
}

/// The budget a task the person asked for may take without their go (`[jobs] direct_budget_eur`), in tokens.
pub fn direct_budget(config: &crate::config::Config) -> i64 {
    tokens_for(config, config.jobs.direct_budget_eur)
}

/// « 0.20 € » (two decimals, three under ten cents).
pub fn euros_text(euros: f64) -> String {
    if euros < 0.1 {
        format!("{euros:.3} €")
    } else {
        format!("{euros:.2} €")
    }
}

pub const START_TASK: &str = "start_task";

/// `start_task`: a task the person just asked for, started at once within the direct budget.
pub fn start_task_tool() -> Value {
    json!({
        "name": START_TASK,
        "description": "Start a background task now, for long work your person just asked you for: anything that needs \
    many steps or minutes (sorting mails, a deep research, comparing many offers, going through many pages or files). \
    It runs on its own, in a conversation of its own with all your tools, within a small default budget, and your \
    person gets your report when it ends. Then tell them in one plain sentence that you're on it in the background, \
    and stop: don't do the work here as well. Only for work your person asked for: for your own ideas, or work that \
    needs more than the default budget, use propose_task. Never for something at a later time or on a schedule \
    (« dans 2 minutes », « demain à 8 h », « tous les matins »): it would run now; that is a routine (routine_schedule, \
    with its date and time).",
        "inputSchema": {"type": "object", "properties": {
            "title": {"type": "string", "description": "a few words, in the person's language"},
            "instructions": {"type": "string", "description": "everything needed to do the task without this \
    conversation: the goal, what the person said, where to look, what to make"},
            "budget_eur": {"type": "number", "description": "optional: at most the default budget (the tool says it); \
    leave it out to take the default"}},
            "required": ["title", "instructions"]},
    })
}

/// The `start_task` MCP tool. Err: what a task file that cannot be written lets through.
pub async fn start_tool(jobs: Option<&Arc<Jobs>>, agent: &AgentConfig, arguments: &Dict) -> anyhow::Result<Dict> {
    let Some(jobs) = jobs else {
        return Ok(crate::tools_knowledge::tool_failure(
            "background tasks aren't available here",
            "I can't keep working on this in the background here.",
        ));
    };
    let config = jobs.config.get();
    let default = config.jobs.direct_budget_eur;
    let euros = match arguments.get("budget_eur").filter(|v| !v.is_null()).map(|v| py::py_float(v).ok()) {
        None => default,
        Some(None) => return Ok(tool_text("budget_eur must be a number of euros", true)),
        Some(Some(euros)) if euros > config.jobs.direct_budget_eur * 1.001 => {
            return Ok(tool_text(
                &format!(
                    "That is more than the {} a task may take without your person's go. Propose it with propose_task \
instead (they start it with one tap) and tell them so in one plain sentence.",
                    if crate::handover::on_offer() {
                        euros_text(config.jobs.direct_budget_eur)
                    } else {
                        budget_text(&config, default, false)
                    }
                ),
                true,
            ));
        }
        Some(Some(euros)) => euros.max(0.01),
    };
    let prompt = tk::str_or_empty(arguments.get("instructions"));
    let title = tk::str_or_empty(arguments.get("title"));
    let from = jobs.hub.as_ref().and_then(|hub| hub.asking_session(&agent.name));
    // a task started in a chain of messages between agents carries it on (agent_hops.rs): its own messages count too
    let turn = jobs.hub.as_ref().and_then(|hub| hub.turn_of(&agent.name, START_TASK)).map(|t| t.run_id);
    let chain = crate::agent_hops::chain_of_turn(&agent.name, turn.as_deref());
    let made = jobs.create_from(agent, &title, tk::head(&prompt, 8000), euros, None, "asked", from.as_deref()).await;
    if let (Some(chain), Ok(view)) = (chain, &made) {
        let task = load(&config, &tk::str_or_empty(view.get("id")));
        if let Some(run_id) = task.as_ref().and_then(|t| t.get("run_id")).and_then(Value::as_str) {
            crate::agent_hops::follow(run_id, &chain);
        }
    }
    started(made, &config, agent)
}

/// A task's budget as the agent may tell its person: in euros on a place of the offer (« 0.20 € »), in tokens only on
/// a home install (« 363,636 tokens »: the euros at the install's prices without the cache), whose model is the
/// person's own (their provider's prices, or none: a model on their computer).
pub fn budget_text(config: &crate::config::Config, euros: f64, offer: bool) -> String {
    if offer {
        euros_text(euros)
    } else {
        format!("{} tokens", thousands(task_cost::configured(config).tokens_for(euros)))
    }
}

/// What the agent is told once its task started (or why it could not).
pub fn started(
    outcome: anyhow::Result<Dict>,
    config: &crate::config::Config,
    agent: &AgentConfig,
) -> anyhow::Result<Dict> {
    started_on(outcome, config, agent, crate::handover::on_offer())
}

/// `started`, on a place of the offer or not.
pub fn started_on(
    outcome: anyhow::Result<Dict>,
    config: &crate::config::Config,
    agent: &AgentConfig,
    offer: bool,
) -> anyhow::Result<Dict> {
    match outcome {
        Ok(job) if job.get("reason") == Some(&json!(SPACE_REASON)) => {
            tracing::info!(agent = %agent.name, job = %tk::str_or_empty(job.get("id")), "job queued by the agent");
            Ok(tool_text(
                "Queued in the background: this space is busy right now, so it starts by itself as soon as there is \
room, and your person gets your report when it ends; they follow it in « Tâches de fond ». Tell them in one plain \
sentence that it will be done a bit later in the background, and stop here: don't do the work in this turn.",
                false,
            ))
        }
        Ok(job) => {
            let euros = task_budget(config, &job);
            tracing::info!(agent = %agent.name, job = %tk::str_or_empty(job.get("id")), budget_eur = euros,
                "job started by the agent");
            Ok(tool_text(
                &format!(
                    "Started in the background (budget about {}): it works on its own and your person gets \
your report when it ends; they follow it in « Tâches de fond ». Tell them in one plain sentence that you're on it in \
the background (e.g. « Je m'en occupe en arrière-plan, je te préviens quand c'est fini. »), and stop here.",
                    budget_text(config, euros, offer)
                ),
                false,
            ))
        }
        Err(err) => match err.downcast_ref::<JobError>() {
            Some(exc) => Ok(tool_text(
                &format!("Not started: {}. Propose it with propose_task instead, and tell your person so.", exc.0),
                true,
            )),
            None => Err(err),
        },
    }
}

/// The bridge's background tasks (one object, made the first time).
pub fn jobs_of(state: &AppState) -> Arc<Jobs> {
    state
        .jobs
        .get_or_init(|| {
            let mut board = Jobs::new(
                state.config.clone(),
                state.hermes.clone(),
                Some(state.hub.clone()),
                Some(state.push.clone()),
                Some(crate::mail_scan::scans_of(state)),
            );
            board.handback = Some(Arc::new(crate::background_turn::Pipeline::of(state)));
            let jobs = Arc::new(board);
            // The safety net's rescue starts tasks for runs that stopped without handing over (runs.rs).
            let _ = state.hub.jobs.set(Arc::downgrade(&jobs));
            jobs
        })
        .clone()
}

// -- the app's routes ---------------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub prompt: String,
    pub budget_eur: f64,
    #[serde(default)]
    pub template: Option<String>,
}

impl Validate for NewTask {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_min_len(errors, "title", Some(&self.title), 1);
        check_max_len(errors, "title", Some(&self.title), 200);
        check_min_len(errors, "prompt", Some(&self.prompt), 1);
        check_max_len(errors, "prompt", Some(&self.prompt), 8000);
        check_range(errors, body_loc("budget_eur"), self.budget_eur, Some(0.01), Some(50.0), json!(self.budget_eur));
        check_max_len(errors, "template", self.template.as_deref(), 40);
    }
}

#[derive(Debug, Deserialize)]
pub struct Start {
    #[serde(default)]
    pub budget_eur: Option<f64>,
}

impl Validate for Start {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(b) = self.budget_eur {
            check_range(errors, body_loc("budget_eur"), b, Some(0.01), Some(50.0), json!(b));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Resume {
    #[serde(default, deserialize_with = "web::lax::opt_int")]
    pub max_tokens: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    /// A task's or an extraction's new budget in euros (its new total).
    #[serde(default)]
    pub budget_eur: Option<f64>,
}

impl Validate for Resume {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if let Some(m) = self.max_tokens {
            check_range(errors, body_loc("max_tokens"), m as f64, Some(10_000.0), Some(200_000_000.0), json!(m));
        }
        check_max_len(errors, "message", self.message.as_deref(), 2000);
        if let Some(b) = self.budget_eur {
            check_range(errors, body_loc("budget_eur"), b, Some(0.01), Some(500.0), json!(b));
        }
    }
}

/// A JobError → 404 (« unknown job ») or 409; anything else → a 500.
fn refused(err: anyhow::Error) -> HttpError {
    match err.downcast_ref::<JobError>() {
        Some(exc) => HttpError::new(if exc.0 == "unknown job" { 404 } else { 409 }, exc.0.clone()),
        None => err.into(),
    }
}

async fn listing(State(state): State<AppState>, q: web::QueryMap) -> ApiResult<Response> {
    let agent = web::query_str(&q, "agent", false, Some(80))?;
    let name = match agent {
        Some(id) => Some(
            state
                .config()
                .agent(Some(&id))
                .map(|a| a.name.clone())
                .ok_or_else(|| HttpError::new(404, "unknown agent"))?,
        ),
        None => None,
    };
    let jobs = jobs_of(&state).list(name.as_deref());
    let (euros, tokens) = budgets(&state.config());
    Ok(web::ok(json!({"jobs": jobs, "budgets_eur": euros, "budgets_tokens": tokens})))
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
        .create(&agent, &body.title, &body.prompt, body.budget_eur, body.template.as_deref())
        .await
        .map_err(refused)?;
    Ok(web::ok(Value::Object(job)))
}

async fn start_route(
    State(state): State<AppState>,
    UrlPath(job_id): UrlPath<String>,
    OptionalJson(body): OptionalJson<Start>,
) -> ApiResult<Response> {
    let job = jobs_of(&state).start(&job_id, body.and_then(|b| b.budget_eur)).await.map_err(refused)?;
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
    let (max_tokens, message, budget_eur) = match body {
        Some(b) => (b.max_tokens, b.message, b.budget_eur),
        None => (None, None, None),
    };
    let job =
        jobs_of(&state).resume_with(&job_id, max_tokens, message.as_deref(), budget_eur).await.map_err(refused)?;
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

    #[test]
    fn the_direct_budget_in_tokens() {
        let mut config = crate::config::Config::with_key("k");
        // 0.20 € at 0.95 × 0.50 + 0.05 × 1.50 = 0.55 € per million tokens
        assert_eq!(direct_budget(&config), 363_636);
        assert!((euros_for(&config, 363_636) - 0.20).abs() < 0.001);
        assert_eq!(euros_text(0.2), "0.20 €");
        assert_eq!(euros_text(0.05), "0.050 €");
        config.jobs.direct_budget_eur = 0.0;
        assert_eq!(direct_budget(&config), 10_000);
        config.notebook.price_in = 0.0;
        config.notebook.price_out = 0.0;
        assert_eq!(tokens_for(&config, 1.0), 300_000);
    }

    #[test]
    fn a_budget_in_euros_on_the_offer_in_tokens_at_home() {
        // 2026-10-31, Alex's home install (his own model): a task's card said « 0,071 € dépensés · au plus 0,037 € »,
        // euros nobody pays there. At home the agent hears its budget in tokens only.
        let config = crate::config::Config::with_key("k");
        assert_eq!(budget_text(&config, 0.20, true), "0.20 €");
        assert_eq!(budget_text(&config, 0.20, false), "363,636 tokens");
        let agent = AgentConfig { name: "vie".into(), ..Default::default() };
        let job = || -> anyhow::Result<Dict> {
            Ok(json!({"id": "t000000000001", "budget_eur": 0.20}).as_object().cloned().unwrap())
        };
        let said = |offer: bool| {
            let out = started_on(job(), &config, &agent, offer).unwrap();
            out["content"][0]["text"].as_str().unwrap().to_string()
        };
        assert!(said(true).starts_with("Started in the background (budget about 0.20 €):"), "{}", said(true));
        assert!(said(false).starts_with("Started in the background (budget about 363,636 tokens):"), "{}", said(false));
        assert!(!said(false).contains('€'));
    }

    #[test]
    fn a_task_counts_its_euros_as_the_credit_does() {
        // Alex, 2026-11-04: « 5 % » of the month for a task whose credit lost under 3 %. Its euros now: fresh input,
        // cache reads and output at the router's prices, as the key's spending is billed.
        let config = crate::config::Config::with_key("k");
        let glm = Prices { input: 0.10, cached: 0.02, output: 0.35 };
        let job = json!({"budget_eur": 0.20, "prices": glm.to_value(),
                         "before": {"input": 1_000_000, "output": 20_000, "cached": 900_000},
                         "tokens": {"input": 1_500_000, "output": 30_000, "cached": 1_350_000}});
        let job = job.as_object().unwrap();
        assert!((eur_spent(&config, job) - 0.0525).abs() < 1e-9, "{}", eur_spent(&config, job));
        assert!((left_for_run(&config, job) - (0.20 - 0.035)).abs() < 1e-9);
        let view = task_view(job);
        assert_eq!(view["max_tokens"], json!(glm.tokens_for(0.20)), "its budget in tokens without the cache");
        // a task kept from before: its tokens budget at the [notebook] prices
        let old = json!({"max_tokens": 363_636}).as_object().cloned().unwrap();
        assert!((task_budget(&config, &old) - 0.20).abs() < 0.001);
        assert!(prices_of(&config, &old) == task_cost::configured(&config));
    }
}
