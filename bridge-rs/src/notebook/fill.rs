//! « Remplir le Carnet » (decided 2026-10-22, after an agent filled the Carnet inline for a long time): an agent goes
//! through its memory and its past conversations and files everything durable in the Carnet, as a background task
//! (jobs.rs) with the direct budget, never in the conversation.
//!
//!     notebook_fill              the agent's MCP tool: the task started for this agent, when the person asks for it
//!     POST   /v1/notebook/fill   {agents?}   the Carnet's settings button: the task for each agent, one after another
//!     GET    /v1/notebook/fill   where that is: {state, total, done, current, agents: [{agent, state, job}]}
//!     DELETE /v1/notebook/fill   no further agent started (the one at work goes on, paused from « Tâches de fond »)
//!
//! The sequence is kept in `jobs/fill.json` and taken up again after a restart. An agent already on another task is
//! waited for (up to three hours, then skipped).

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::jobs::{self, JobError, Jobs};
use crate::tools_knowledge::{self as tk, tool_text};
use crate::{py, Dict};

pub const TOOL_NAME: &str = "notebook_fill";
pub const TEMPLATE: &str = "notebook_fill";
/// Seconds an agent busy with another task is waited for before it is skipped.
pub const WAIT_MAX: f64 = 3.0 * 3600.0;

/// What the agent is asked to do (in English, like the other instructions; it reports in its person's language).
pub const PROMPT: &str = "Fill your person's Carnet from what you already know about them, on your own:
1. Read your memory: what you keep about your person and their life.
2. Go through your past conversations with your person with session_search: search the main topics (their family \
and the people they mention, work, home, health, money, projects and plans, tastes and habits) and the names that \
come up, and read what matters.
3. File every durable fact in the Carnet with the notebook tools, following your Carnet rules: notebook_search \
first, then notebook_note under the right section of the page's outline (their « Toi » page for facts about \
themself), notebook_edit to correct what changed. Skip what is already there, small talk, one-off questions, \
passwords or codes. Add no task unless your person clearly committed to something with a date.
Don't ask questions. End with a short report in your person's language: how many pages you made or completed, \
which ones, and what you weren't sure about.";

/// The task's title in the Carnet's language.
pub fn title(config: &Config) -> &'static str {
    match super::language(config).as_str() {
        "fr" => "Remplir le Carnet",
        "de" => "Notizbuch füllen",
        "es" => "Rellenar el Cuaderno",
        _ => "Fill the Notebook",
    }
}

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Start « Remplir le Carnet » in the background: you go through your memory and your past \
    conversations with your person and file everything durable in their Carnet, on your own, within the default budget; \
    they get your report when it ends. Use it when your person asks you to fill, complete or update their Carnet from \
    what you know: never do that here in the conversation. Then tell them in one plain sentence that you're on it in \
    the background, and stop.",
        "inputSchema": {"type": "object", "properties": {}},
    })
}

/// The task for `agent` (its tool, or one step of the settings' sequence; `by`: who started it).
pub async fn start(jobs: &Arc<Jobs>, agent: &AgentConfig, by: &str) -> anyhow::Result<Dict> {
    let config = jobs.config.get();
    let budget = jobs::direct_budget(&config);
    jobs.create_by(agent, title(&config), PROMPT, budget, Some(TEMPLATE), by).await
}

/// The `notebook_fill` MCP tool.
pub async fn tool_call(jobs: Option<&Arc<Jobs>>, agent: &AgentConfig) -> anyhow::Result<Dict> {
    let Some(jobs) = jobs else {
        return Ok(tk::tool_failure(
            "background tasks aren't available here",
            "I can't fill your Carnet in the background here.",
        ));
    };
    let config = jobs.config.get();
    if !super::enabled(&config, &agent.name) {
        return Ok(tool_text("Your Carnet tools are off: the person turns them on in your page of the app.", true));
    }
    jobs::started(start(jobs, agent, "asked").await, &config, agent)
}

// -- the sequence (the settings' button) ----------------------------------------------------------------------------

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("jobs").join("fill.json")
}

fn load(config: &Config) -> Option<Dict> {
    match serde_json::from_str::<Value>(&tk::read_text(&path(config)).ok()?).ok()? {
        Value::Object(state) => Some(state),
        _ => None,
    }
}

fn save(config: &Config, state: &Dict) -> std::io::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    }
    let temporary = file.with_extension("tmp");
    std::fs::write(&temporary, py::Dumps::new().unicode().dumps(&Value::Object(state.clone())))?;
    std::fs::rename(temporary, file)
}

fn entries(state: &Dict) -> Vec<Dict> {
    match state.get("agents") {
        Some(Value::Array(items)) => items.iter().filter_map(|v| v.as_object().cloned()).collect(),
        _ => Vec::new(),
    }
}

fn entry_state(entry: &Dict) -> String {
    tk::str_or_empty(entry.get("state"))
}

/// Where the sequence is, for the app.
pub fn view(config: &Config) -> Dict {
    let Some(state) = load(config) else {
        let mut out = Dict::new();
        for (key, value) in
            [("state", json!("idle")), ("total", json!(0)), ("done", json!(0)), ("current", Value::Null)]
        {
            out.insert(key.into(), value);
        }
        out.insert("agents".into(), json!([]));
        return out;
    };
    let items = entries(&state);
    let done = items.iter().filter(|e| !["waiting", "running"].contains(&entry_state(e).as_str())).count();
    let current = items.iter().find(|e| entry_state(e) == "running").and_then(|e| e.get("agent").cloned());
    let mut out = Dict::new();
    out.insert("state".into(), state.get("state").cloned().unwrap_or(json!("idle")));
    out.insert("total".into(), json!(items.len()));
    out.insert("done".into(), json!(done));
    out.insert("current".into(), current.unwrap_or(Value::Null));
    out.insert("agents".into(), Value::Array(items.into_iter().map(Value::Object).collect()));
    for key in ["started", "finished"] {
        out.insert(key.into(), state.get(key).cloned().unwrap_or(Value::Null));
    }
    out
}

/// The agents a « Remplir mon Carnet » goes through: those with the Carnet's tools on, in the install's order.
pub fn default_agents(config: &Config) -> Vec<String> {
    config.agents.values().filter(|a| super::enabled(config, &a.name)).map(|a| a.name.clone()).collect()
}

/// The sequence started (`agents`: ids, or every agent with the Carnet on).
pub fn start_all(jobs: &Arc<Jobs>, agents: Option<Vec<String>>) -> Result<Dict, JobError> {
    let config = jobs.config.get();
    if load(&config).is_some_and(|s| tk::str_or_empty(s.get("state")) == "running") {
        return Err(JobError("the Carnet is being filled already".into()));
    }
    let names: Vec<String> = match agents {
        Some(ids) => {
            let mut names = Vec::new();
            for id in ids {
                let Some(agent) = config.agent(Some(&id)) else { return Err(JobError("unknown agent".into())) };
                if !names.contains(&agent.name) {
                    names.push(agent.name.clone());
                }
            }
            names
        }
        None => default_agents(&config),
    };
    if names.is_empty() {
        return Err(JobError("no agent keeps the Carnet".into()));
    }
    let items: Vec<Value> = names.iter().map(|n| json!({"agent": n, "state": "waiting", "job": null})).collect();
    let mut state = Dict::new();
    state.insert("state".into(), json!("running"));
    state.insert("agents".into(), Value::Array(items));
    state.insert("started".into(), json!(py::time_time()));
    state.insert("finished".into(), Value::Null);
    save(&config, &state).map_err(|e| JobError(format!("the Carnet's filling can't be kept: {e}")))?;
    tracing::info!(agents = names.len(), "notebook fill started");
    follow(jobs);
    Ok(view(&config))
}

/// No further agent started.
pub fn stop_all(jobs: &Arc<Jobs>) -> Dict {
    let config = jobs.config.get();
    if let Some(mut state) = load(&config).filter(|s| tk::str_or_empty(s.get("state")) == "running") {
        let items: Vec<Value> = entries(&state)
            .into_iter()
            .map(|mut e| {
                if entry_state(&e) == "waiting" {
                    e.insert("state".into(), json!("skipped"));
                }
                Value::Object(e)
            })
            .collect();
        state.insert("agents".into(), Value::Array(items));
        state.insert("state".into(), json!("stopped"));
        state.insert("finished".into(), json!(py::time_time()));
        let _ = save(&config, &state);
    }
    if let Some(task) = jobs.filler.lock().unwrap_or_else(|e| e.into_inner()).take() {
        task.abort();
    }
    view(&config)
}

/// The sequence followed (again, after a restart).
pub fn follow(jobs: &Arc<Jobs>) {
    let config = jobs.config.get();
    if !load(&config).is_some_and(|s| tk::str_or_empty(s.get("state")) == "running") {
        return;
    }
    let mut filler = jobs.filler.lock().unwrap_or_else(|e| e.into_inner());
    if filler.as_ref().is_none_or(|t| t.is_finished()) {
        let this = jobs.clone();
        *filler = Some(tokio::spawn(async move { run(this).await }));
    }
}

/// One agent after the other: its task started once it may run, then followed until it ends.
async fn run(jobs: Arc<Jobs>) {
    loop {
        let config = jobs.config.get();
        let Some(mut state) = load(&config) else { return };
        if tk::str_or_empty(state.get("state")) != "running" {
            return;
        }
        let mut items = entries(&state);
        let Some(index) = items.iter().position(|e| ["waiting", "running"].contains(&entry_state(e).as_str())) else {
            state.insert("state".into(), json!("done"));
            state.insert("finished".into(), json!(py::time_time()));
            let _ = save(&config, &state);
            tracing::info!("notebook fill done");
            return;
        };
        let entry = &mut items[index];
        let name = tk::str_or_empty(entry.get("agent"));
        let mut wait = true;
        if entry_state(entry) == "running" {
            let job = jobs::load(&config, &tk::str_or_empty(entry.get("job")));
            let job_state = job.as_ref().map(|j| tk::str_or_empty(j.get("state"))).unwrap_or_else(|| "failed".into());
            if !["running", "stopping"].contains(&job_state.as_str()) {
                entry.insert("state".into(), json!(job_state));
                wait = false;
            }
        } else {
            match config.agent(Some(&name)).cloned().filter(|a| super::enabled(&config, &a.name)) {
                None => {
                    entry.insert("state".into(), json!("skipped"));
                    wait = false;
                }
                Some(agent) if jobs.may_run(&agent.name).is_ok() => match start(&jobs, &agent, "person").await {
                    Ok(job) => {
                        entry.insert("state".into(), json!("running"));
                        entry.insert("job".into(), job.get("id").cloned().unwrap_or(Value::Null));
                    }
                    Err(err) => {
                        tracing::info!(agent = %agent.name, error = %err, "notebook fill: task not started");
                        entry.insert("state".into(), json!("failed"));
                        entry.insert("error".into(), json!(tk::head(&err.to_string(), 300)));
                        wait = false;
                    }
                },
                Some(_) => {
                    // another task of this agent first (or the install's limit): waited for, then skipped
                    let now = py::time_time();
                    let since = entry.get("waiting_since").and_then(Value::as_f64).unwrap_or(now);
                    entry.insert("waiting_since".into(), json!(since));
                    if now - since >= WAIT_MAX {
                        entry.insert("state".into(), json!("skipped"));
                        wait = false;
                    }
                }
            }
        }
        state.insert("agents".into(), Value::Array(items.into_iter().map(Value::Object).collect()));
        // the person stopped the sequence meanwhile: their change stands
        if load(&config).is_some_and(|s| tk::str_or_empty(s.get("state")) != "running") {
            return;
        }
        let _ = save(&config, &state);
        if wait {
            tokio::time::sleep(Duration::from_secs_f64(jobs.poll)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_view() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("bridge.db").to_string_lossy().into_owned();
        let view = view(&config);
        assert_eq!(view.get("state"), Some(&json!("idle")));
        assert_eq!(view.get("total"), Some(&json!(0)));
    }
}
