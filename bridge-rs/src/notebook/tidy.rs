//! The Carnet's nightly pass (« rangement »), cheap by design: the agents file facts as they talk (the rule in their
//! personality); once a night (or a week, or never: the person's setting) the bridge looks for what they missed.
//!
//! 1. A free pre-filter, no model call: only the conversations with new messages since the last pass (a cursor per
//!    conversation), only the turns where the agent did not write to the Carnet itself, only those that look like they
//!    hold durable facts (a date, a name, a project, a family word, an appointment…). Nothing left and nothing « À
//!    ranger »: no model call at all.
//! 2. Otherwise one call to a small, cheap model (`[notebook] model`, through the agents' own model provider unless
//!    `[notebook] base_url` says otherwise), with the pages' names and those turns, which answers with what to file
//!    (`note`, `task`, `merge` lines); the bridge applies them like an agent's writes, attributed to the agent whose
//!    conversation it was. What it sends is capped (`max_input_chars`).
//! 3. A hard monthly cap per install (`[notebook] monthly_cap`, 0.10 € by default): a run that could pass it is cut down
//!    or skipped; once reached, the pass waits for the next month (the agents keep filing as they talk). Every run logs
//!    its tokens and its estimated cost (`notebook pass` in the log, and `GET /v1/notebook/runs`).

use std::sync::LazyLock;
use std::time::Duration;

use chrono::{Datelike, Local, Timelike};
use regex::Regex;
use serde_json::{json, Value};

use super::{Part, Who};
use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::hermes::HermesClient;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// Runs kept in the state (for the app and the owner's measures).
pub const RUNS_KEPT: usize = 60;
/// One turn's text sent at most.
pub const TURN_CHARS: usize = 700;
/// What the model may answer.
pub const MAX_OUTPUT_TOKENS: i64 = 900;
/// How often the scheduler looks at the clock.
pub const TICK: Duration = Duration::from_secs(600);

pub const INSTRUCTIONS: &str = "You tidy a person's notebook (their « Carnet »), shared by their AI agents. You get \
the names of its pages, then excerpts of today's conversations between the person and their agents that the agents \
did not file, and maybe lines « to sort » copied from the agents' memories. File only durable facts worth finding \
again: about a person (birthday, family, preferences, what was agreed), a subject or project (decisions, status, next \
steps), an area of life. A fact is not a task: a birthday or someone else's deadline is a dated fact on its page. \
Add a task only when the person asked for it (« remind me… », « I have to… »: origin asked) or stated their own \
commitment with a deadline (origin own); never invent one. Skip small talk, one-off questions, passwords and codes, and \
anything the person asked not to keep. Use an existing page's exact name when the fact is about it. Write each fact \
short (one fact an item), in the person's language. Never file health facts. Every page has fixed sections, named by \
id: person: who (how they relate, how they met, contact details), dates, family, work, tastes (likes, dislikes, food \
allergies), gifts, agreed (promises, loans, appointments), exchanges (short dated lines), remember; subject: status, \
decisions, next, people, budget, documents, history; area: matters, habits, contacts, ongoing; self (the person \
themself, kind self, their one page « Toi »): about, family, work, habits, goals, agents (how their agents should \
behave with them); other only when nothing fits. You may also get lines filed under « other »: move each to its section when one clearly fits. Answer \
with JSON lines only, one per line, nothing else:
{\"op\": \"note\", \"agent\": \"<agent id>\", \"kind\": \"person|subject|area|self\", \"page\": \"<name>\", \"section\": \"<section id>\", \"facts\": [\"...\"], \"summary\": \"<2-3 short lines, optional>\"}
{\"op\": \"refile\", \"page\": \"<name>\", \"fact\": \"<the line, exactly>\", \"section\": \"<section id>\"}
{\"op\": \"task\", \"agent\": \"<agent id>\", \"origin\": \"asked|own\", \"title\": \"...\", \"due\": \"YYYY-MM-DD\", \"page\": \"<name, optional>\"}
{\"op\": \"merge\", \"from\": \"<page name>\", \"into\": \"<page name>\"}   (two pages about the same person or subject)
Nothing worth filing: answer nothing.";

static DURABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b\d{1,2}[/.-]\d{1,2}\b|\b\d{4}-\d{2}-\d{2}\b|\b\d{1,2}(?:er)?\s+(?:janv|févr|fevr|mars|avr|mai|juin|juil|août|aout|sept|oct|nov|déc|dec|jan|feb|mar|apr|may|jun|jul|aug|sep|ene|abr|ago|dic|märz|okt|dez)",
        r"|\b(?:anniversaire|birthday|geburtstag|cumpleaños|rendez-vous|rdv|appointment|termin|cita|réunion|meeting|reunión|projet|project|projekt|proyecto|devis|quote|contrat|contract|vertrag|contrato|décid|decid|beschlossen|deadline|échéance|avant le|before|bis zum|antes del|demain|tomorrow|morgen|mañana|lundi|mardi|mercredi|jeudi|vendredi|samedi|dimanche|monday|tuesday|wednesday|thursday|friday|saturday|sunday)",
        r"|\b(?:femme|mari|fils|fille|frère|soeur|sœur|mère|père|maman|papa|wife|husband|son|daughter|brother|sister|mother|father|frau|mann|sohn|tochter|bruder|schwester|esposa|esposo|hijo|hija|hermano|hermana|madre|padre)\b",
        r"|\b(?:allergi|préf[eè]re|aime|déteste|prefer|likes|loves|hates|mag|liebt|prefiere|le gusta|odia|n'oublie|remember|vergiss|recuerda|toujours|always|jamais|never|adresse|address|déménag|moving|umzug|mudanza)"
    ))
    .expect("valid regex")
});
/// A capitalised word that does not start a sentence: a name.
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-zà-ÿ,;:]\s+\p{Lu}\p{Ll}{2,}").expect("valid regex"));

/// Whether the person's words look like they hold a durable fact (the free pre-filter).
pub fn looks_durable(text: &str) -> bool {
    tk::chars(text) >= 15 && (DURABLE.is_match(text) || NAME.is_match(text))
}

fn content(message: &Dict) -> String {
    match message.get("content") {
        Some(Value::Array(parts)) => {
            parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" ")
        }
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn wrote_notebook(message: &Dict) -> bool {
    match message.get("tool_calls") {
        Some(Value::Array(calls)) => calls.iter().any(|c| {
            let name = c
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| c.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            name.contains("notebook_note") || name.contains("notebook_edit") || name.contains("notebook_task")
        }),
        _ => false,
    }
}

/// A turn: what the person said and what the agent answered; `covered` when the agent wrote to the Carnet in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub person: String,
    pub reply: String,
    pub covered: bool,
}

/// The turns of `messages` (oldest first).
pub fn turns(messages: &[Dict]) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        if message.get("display_kind").and_then(Value::as_str) == Some("hidden") {
            continue;
        }
        match role {
            "user" => out.push(Turn { person: tk::squeeze(&content(message)), reply: String::new(), covered: false }),
            "assistant" => {
                if let Some(turn) = out.last_mut() {
                    if wrote_notebook(message) {
                        turn.covered = true;
                    }
                    let text = tk::squeeze(&content(message));
                    if !text.is_empty() {
                        turn.reply = text;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The turns kept by the pre-filter.
pub fn keep(turns: Vec<Turn>) -> Vec<Turn> {
    turns.into_iter().filter(|t| !t.covered && looks_durable(&t.person)).collect()
}

// -- state ---------------------------------------------------------------------------------------------------------

fn tidy_state(state: &Dict) -> Dict {
    match state.get("tidy") {
        Some(Value::Object(m)) => m.clone(),
        _ => Dict::new(),
    }
}

fn month() -> String {
    Local::now().format("%Y-%m").to_string()
}

/// This month's spending (`eur`, tokens), reset each month.
pub fn spending(config: &Config) -> Dict {
    let tidy = tidy_state(&super::read_state(config));
    match tidy.get("spend") {
        Some(Value::Object(m)) if m.get("month").and_then(Value::as_str) == Some(month().as_str()) => m.clone(),
        _ => {
            let mut m = Dict::new();
            m.insert("month".into(), json!(month()));
            m.insert("eur".into(), json!(0.0));
            m.insert("tokens_in".into(), json!(0));
            m.insert("tokens_out".into(), json!(0));
            m.insert("model_calls".into(), json!(0));
            m
        }
    }
}

pub fn runs(config: &Config) -> Vec<Value> {
    match tidy_state(&super::read_state(config)).get("runs") {
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

/// Whether the pass is due now: its setting, the hour, once a day (or a week), and the cap not reached.
pub fn due(config: &Config, now: chrono::DateTime<Local>) -> bool {
    let every = super::nightly(config);
    if every == "never" || !super::root(config).exists() && config.agents.is_empty() {
        return false;
    }
    if now.hour() < config.notebook.hour {
        return false;
    }
    let last = tk::str_or_empty(tidy_state(&super::read_state(config)).get("last_day"));
    let today = now.format("%Y-%m-%d").to_string();
    if last == today {
        return false;
    }
    if every == "weekly" {
        if let Ok(day) = chrono::NaiveDate::parse_from_str(&last, "%Y-%m-%d") {
            if (now.date_naive() - day).num_days() < 7 {
                return false;
            }
        }
    }
    let _ = now.weekday();
    true
}

// -- the model -----------------------------------------------------------------------------------------------------

/// The model the pass calls: (base url, model, key). `[notebook] base_url` and `key`, else the first agent's own
/// provider and key (mail_scan::model_of) with `[notebook] model` (or the agent's model when that is empty).
pub fn model(config: &Config) -> Option<(String, String, String)> {
    let wanted = config.notebook.model.clone();
    if !config.notebook.base_url.is_empty() {
        let key = config.notebook.key.clone().unwrap_or_default();
        return Some((config.notebook.base_url.clone(), wanted, key));
    }
    for agent in config.agents.values() {
        if let Ok(found) = crate::mail_scan::model_of(config, agent) {
            let name = if wanted.is_empty() { found["model"].clone() } else { wanted };
            return Some((found["base_url"].clone(), name, found["key"].clone()));
        }
    }
    None
}

fn cost(config: &Config, tokens_in: i64, tokens_out: i64) -> f64 {
    (tokens_in as f64 * config.notebook.price_in + tokens_out as f64 * config.notebook.price_out) / 1_000_000.0
}

/// About 3.5 characters a token (French and English prose, JSON).
fn tokens_of(chars: usize) -> i64 {
    (chars as f64 / 3.5).ceil() as i64
}

// -- one pass ------------------------------------------------------------------------------------------------------

/// What one agent's conversations gave since the last pass: its turns kept, and the cursors to save.
struct Gathered {
    agent: AgentConfig,
    health: bool,
    seen: usize,
    kept: Vec<Turn>,
    cursors: Dict,
}

async fn gather(hermes: &HermesClient, config: &Config, agent: &AgentConfig, since: f64) -> anyhow::Result<Gathered> {
    let state = super::read_state(config);
    let old =
        state.get("cursors").and_then(|c| c.get(&agent.name)).and_then(Value::as_object).cloned().unwrap_or_default();
    let mut cursors = Dict::new();
    let mut seen = 0;
    let mut kept = Vec::new();
    for session in hermes.list_sessions(agent, 30).await? {
        if !crate::live_brief::is_conversation(&session) {
            continue;
        }
        let Some(id) = session.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
        let active = crate::live_brief::seconds(session.get("last_active")).unwrap_or(0.0);
        let before = old.get(&id).and_then(Value::as_u64).unwrap_or(0) as usize;
        if active < since && old.contains_key(&id) {
            cursors.insert(id, json!(before));
            continue;
        }
        if active < since && !old.contains_key(&id) && since > 0.0 {
            continue;
        }
        let messages = hermes.session_messages(agent, &id).await?;
        cursors.insert(id, json!(messages.len()));
        if messages.len() <= before {
            continue;
        }
        let all = turns(&messages[before..]);
        seen += all.len();
        kept.extend(keep(all));
    }
    Ok(Gathered { agent: agent.clone(), health: super::health_agent(config, &agent.name), seen, kept, cursors })
}

fn clip(text: &str, n: usize) -> String {
    if tk::chars(text) <= n {
        text.to_string()
    } else {
        format!("{}…", tk::head(text, n))
    }
}

/// The pass, now. `forced`: run even if not due (the app's « ranger maintenant », the owner's tests). Returns the run's
/// record (also kept in the state and logged).
pub async fn run(config: &Config, hermes: &HermesClient, http: &reqwest::Client) -> Dict {
    let started = tk::now_iso_seconds();
    let mut record = Dict::new();
    record.insert("at".into(), json!(started));
    let result = run_inner(config, hermes, http, &mut record).await;
    if let Err(err) = result {
        record.insert("error".into(), json!(clip(&format!("{err:#}"), 300)));
    }
    let spend = spending(config);
    let _guard = super::lock();
    let saved = super::update_state(config, |s| {
        let mut tidy = tidy_state(s);
        tidy.insert("last_day".into(), json!(Local::now().format("%Y-%m-%d").to_string()));
        let mut all = match tidy.get("runs") {
            Some(Value::Array(r)) => r.clone(),
            _ => Vec::new(),
        };
        all.insert(0, Value::Object(record.clone()));
        all.truncate(RUNS_KEPT);
        tidy.insert("runs".into(), Value::Array(all));
        let mut spend = spend.clone();
        let add_i = |m: &mut Dict, k: &str, v: i64| {
            let now = m.get(k).and_then(Value::as_i64).unwrap_or(0);
            m.insert(k.into(), json!(now + v));
        };
        let tokens_in = record.get("tokens_in").and_then(Value::as_i64).unwrap_or(0);
        let tokens_out = record.get("tokens_out").and_then(Value::as_i64).unwrap_or(0);
        add_i(&mut spend, "tokens_in", tokens_in);
        add_i(&mut spend, "tokens_out", tokens_out);
        if record.get("model_called").map(py::truthy).unwrap_or(false) {
            add_i(&mut spend, "model_calls", 1);
        }
        let eur = spend.get("eur").and_then(Value::as_f64).unwrap_or(0.0)
            + record.get("eur").and_then(Value::as_f64).unwrap_or(0.0);
        spend.insert("eur".into(), json!((eur * 1_000_000.0).round() / 1_000_000.0));
        tidy.insert("spend".into(), Value::Object(spend));
        s.insert("tidy".into(), Value::Object(tidy));
    });
    if let Err(err) = saved {
        tracing::warn!(error = %err, "notebook pass not recorded");
    }
    let int = |k: &str| record.get(k).and_then(serde_json::Value::as_i64).unwrap_or(0);
    let (seen, kept, tokens_in, tokens_out, applied) =
        (int("turns_seen"), int("turns_kept"), int("tokens_in"), int("tokens_out"), int("applied"));
    let eur = record.get("eur").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
    let called = record.get("model_called").map(py::truthy).unwrap_or(false);
    let reason = tk::str_or_empty(record.get("reason"));
    tracing::info!(reason = %reason, turns_seen = seen, turns_kept = kept, model_called = called, tokens_in = tokens_in,
        tokens_out = tokens_out, eur = eur, applied = applied, "notebook pass");
    record
}

async fn run_inner(
    config: &Config,
    hermes: &HermesClient,
    http: &reqwest::Client,
    record: &mut Dict,
) -> anyhow::Result<()> {
    record.insert("model_called".into(), json!(false));
    record.insert("tokens_in".into(), json!(0));
    record.insert("tokens_out".into(), json!(0));
    record.insert("eur".into(), json!(0.0));
    record.insert("applied".into(), json!(0));
    // the agents' memories, copied once (no model)
    // accepted proposals left untouched long after their date: archived (no model)
    let archived = tokio::task::spawn_blocking({
        let c = config.clone();
        move || super::archive_stale(&c, Local::now().date_naive())
    })
    .await??;
    if archived > 0 {
        record.insert("archived".into(), json!(archived));
    }
    let seeded = tokio::task::spawn_blocking({
        let c = config.clone();
        move || super::seed(&c)
    })
    .await??;
    if seeded > 0 {
        record.insert("seeded".into(), json!(seeded));
    }
    // pages from before the outlines: re-filed once by their words (no model)
    let refiled = tokio::task::spawn_blocking({
        let c = config.clone();
        move || super::refile_existing(&c)
    })
    .await??;
    if refiled > 0 {
        record.insert("refiled".into(), json!(refiled));
    }
    let unsorted = super::unsorted(config, super::REFILE_PER_NIGHT);
    record.insert("unsorted".into(), json!(unsorted.len()));
    let tidy = tidy_state(&super::read_state(config));
    let since = tidy.get("since").and_then(Value::as_f64).unwrap_or(py::time_time() - 36.0 * 3600.0);
    let now = py::time_time();
    let mut gathered: Vec<Gathered> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for agent in config.agents.values() {
        // The agents the person gave their health to are left out: their conversations may hold health data, which
        // only goes to a model the person accepted for it (an EU one with the offer), not to the pass's small model.
        if !super::enabled(config, &agent.name) || super::health_agent(config, &agent.name) {
            continue;
        }
        match gather(hermes, config, agent, since).await {
            Ok(g) => gathered.push(g),
            Err(err) => {
                tracing::warn!(agent = %agent.name, error = %clip(&err.to_string(), 200), "notebook pass: conversations not read");
                failed.push(agent.name.clone());
            }
        }
    }
    let seen: usize = gathered.iter().map(|g| g.seen).sum();
    let kept: usize = gathered.iter().map(|g| g.kept.len()).sum();
    record.insert("turns_seen".into(), json!(seen));
    record.insert("turns_kept".into(), json!(kept));
    let (_, inbox_lines) = super::inbox(config);
    record.insert("inbox".into(), json!(inbox_lines.len()));
    let save_cursors = |gathered: &[Gathered]| -> anyhow::Result<()> {
        let _guard = super::lock();
        super::update_state(config, |s| {
            let mut cursors = match s.get("cursors") {
                Some(Value::Object(m)) => m.clone(),
                _ => Dict::new(),
            };
            for g in gathered {
                cursors.insert(g.agent.name.clone(), Value::Object(g.cursors.clone()));
            }
            s.insert("cursors".into(), Value::Object(cursors));
            let mut tidy = tidy_state(s);
            tidy.insert("since".into(), json!(now));
            s.insert("tidy".into(), Value::Object(tidy));
        })?;
        Ok(())
    };
    if kept == 0 && inbox_lines.is_empty() && unsorted.is_empty() {
        record.insert("reason".into(), json!(if seen == 0 { "no conversation" } else { "nothing to file" }));
        save_cursors(&gathered)?;
        return Ok(());
    }
    let cap = config.notebook.monthly_cap;
    let spent = spending(config).get("eur").and_then(Value::as_f64).unwrap_or(0.0);
    let left = cap - spent;
    // the input: the pages' names, the turns (the newest kept when cut), the lines to sort
    let pages: Vec<String> = super::all_pages(config, Part::Main)
        .into_iter()
        .filter(|(_, p)| super::KINDS.contains(&p.kind().as_str()))
        .map(|(_, p)| {
            let aliases = p.aliases();
            if aliases.is_empty() {
                format!("- {} ({})", p.title, p.kind())
            } else {
                format!("- {} ({}; also {})", p.title, p.kind(), aliases.join(", "))
            }
        })
        .collect();
    let mut index = format!(
        "Today: {}. Pages of the Carnet:\n{}",
        Local::now().format("%Y-%m-%d"),
        if pages.is_empty() { "(none yet)".into() } else { pages.join("\n") }
    );
    index = clip(&index, 3000);
    let mut blocks: Vec<String> = Vec::new();
    for g in &gathered {
        for turn in &g.kept {
            blocks.push(format!(
                "[agent {}]\nPerson: {}\nAgent: {}",
                g.agent.name,
                clip(&turn.person, TURN_CHARS),
                clip(&turn.reply, TURN_CHARS / 2)
            ));
        }
    }
    let budget_chars = config.notebook.max_input_chars;
    // the most chars this run may send without passing the cap (its answer counted at its maximum)
    let affordable = if config.notebook.price_in > 0.0 {
        let euros =
            left - cost(config, 0, MAX_OUTPUT_TOKENS) - cost(config, tokens_of(INSTRUCTIONS.len() + index.len()), 0);
        ((euros.max(0.0) * 1_000_000.0 / config.notebook.price_in) * 3.5) as usize
    } else {
        usize::MAX
    };
    let room = budget_chars.min(affordable);
    if left <= 0.0 || room < 400 {
        record.insert("reason".into(), json!("monthly cap reached"));
        // the cursors stay: those turns are looked at again next month
        return Ok(());
    }
    let mut kept_blocks: Vec<String> = Vec::new();
    let mut size = 0;
    for block in blocks.iter().rev() {
        if size + block.len() > room {
            break;
        }
        size += block.len();
        kept_blocks.insert(0, block.clone());
    }
    let mut sent_inbox: Vec<String> = Vec::new();
    for line in &inbox_lines {
        if size + line.len() > room {
            break;
        }
        size += line.len();
        sent_inbox.push(line.clone());
    }
    let mut sent_unsorted: Vec<super::Unsorted> = Vec::new();
    for line in &unsorted {
        let size_of = line.page.len() + line.fact.len() + 20;
        if size + size_of > room {
            break;
        }
        size += size_of;
        sent_unsorted.push(line.clone());
    }
    if kept_blocks.is_empty() && sent_inbox.is_empty() && sent_unsorted.is_empty() {
        record.insert("reason".into(), json!("monthly cap reached"));
        return Ok(());
    }
    let mut user = format!(
        "{index}\n\nConversations:\n{}",
        if kept_blocks.is_empty() { "(none)".into() } else { kept_blocks.join("\n\n") }
    );
    if !sent_inbox.is_empty() {
        let first = gathered
            .first()
            .map(|g| g.agent.name.clone())
            .or_else(|| config.agents.keys().next().cloned())
            .unwrap_or_default();
        user.push_str(&format!(
            "\n\nTo sort (from the agents' memories; use agent {first} unless a line names another):\n{}",
            sent_inbox.join("\n")
        ));
    }
    if !sent_unsorted.is_empty() {
        let lines: Vec<String> =
            sent_unsorted.iter().map(|u| format!("- [{}] ({}) {}", u.page, u.kind, u.fact)).collect();
        user.push_str(&format!("\n\nFiled under « other » (refile them when a section fits):\n{}", lines.join("\n")));
    }
    let Some((base, model, key)) = model(config) else {
        record.insert("reason".into(), json!("no model to call"));
        return Ok(());
    };
    record.insert("model".into(), json!(model));
    let body = json!({"model": model, "temperature": 0, "max_tokens": MAX_OUTPUT_TOKENS,
                      "messages": [{"role": "system", "content": INSTRUCTIONS}, {"role": "user", "content": user}]});
    let resp = http
        .post(format!("{base}/chat/completions"))
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .timeout(Duration::from_secs(120))
        .send()
        .await?;
    record.insert("model_called".into(), json!(true));
    let status = resp.status().as_u16();
    let raw = resp.bytes().await?;
    let data: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    let usage = data.get("usage").cloned().unwrap_or(json!({}));
    let tokens_in = usage
        .get("prompt_tokens")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| tokens_of(INSTRUCTIONS.len() + user.len()));
    let tokens_out = usage.get("completion_tokens").and_then(Value::as_i64).unwrap_or(0);
    record.insert("tokens_in".into(), json!(tokens_in));
    record.insert("tokens_out".into(), json!(tokens_out));
    record.insert("eur".into(), json!((cost(config, tokens_in, tokens_out) * 1_000_000.0).round() / 1_000_000.0));
    if status >= 400 {
        record.insert("reason".into(), json!(format!("the model answered {status}")));
        return Ok(()); // the cursors stay: tried again next night
    }
    let answer = data
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let c = config.clone();
    let agents: Vec<(String, bool)> = gathered.iter().map(|g| (g.agent.name.clone(), g.health)).collect();
    let fallback =
        agents.first().map(|a| a.0.clone()).or_else(|| config.agents.keys().next().cloned()).unwrap_or_default();
    let applied = tokio::task::spawn_blocking(move || {
        let applied = apply(&c, &answer, &agents, &fallback, &sent_inbox)?;
        let _guard = super::lock();
        super::mark_tried(&c, &sent_unsorted)?;
        anyhow::Ok(applied)
    })
    .await??;
    record.insert("applied".into(), json!(applied));
    record.insert("reason".into(), json!("filed"));
    save_cursors(&gathered)?;
    Ok(())
}

/// The model's lines applied (each checked as an agent's write would be). Returns how many changed something.
pub fn apply(
    config: &Config,
    answer: &str,
    agents: &[(String, bool)],
    fallback: &str,
    inbox: &[String],
) -> anyhow::Result<usize> {
    let _guard = super::lock();
    let mut applied = 0;
    for line in answer.lines() {
        let line = line.trim().trim_matches(',');
        if !line.starts_with('{') {
            continue;
        }
        let Ok(Value::Object(op)) = serde_json::from_str::<Value>(line) else { continue };
        let s = |k: &str| tk::str_or_empty(op.get(k)).trim().to_string();
        let mut agent = s("agent");
        if config.agent(Some(&agent)).is_none() {
            agent = fallback.to_string();
        }
        let who = Who::nightly(config, &agent);
        let private = op.get("private").map(py::truthy).unwrap_or(false);
        let w = super::words(&super::language(config));
        let page = s("page");
        if private || super::fold(&page) == super::fold(w.health) || agents.iter().any(|(a, h)| *a == agent && *h) {
            continue; // never health: that stays with the health agent, in the private part
        }
        let part = Part::Main;
        let result: anyhow::Result<bool> = match s("op").as_str() {
            "note" => {
                let facts: Vec<String> = match op.get("facts") {
                    Some(Value::Array(f)) => f.iter().map(py::py_str).collect(),
                    _ => Vec::new(),
                };
                let kind = if super::KINDS.contains(&s("kind").as_str()) { s("kind") } else { "subject".into() };
                super::note_locked(config, part, &kind, &page, &s("section"), &facts, &[], "", &s("summary"), &who)
                    .map(|w| !w.added.is_empty() || w.created)
            }
            "task" => {
                // only what the person asked or committed to (never a proposal: that is the agents', with the person)
                let origin = if s("origin") == "own" { "own" } else { "asked" };
                super::add_task_locked(config, part, &s("title"), &s("due"), &page, origin, "", &who).map(|_| true)
            }
            "refile" => super::refile_locked(config, &page, &s("fact"), &s("section"), &Who::tidy(config)),
            "merge" => super::merge_locked(config, Part::Main, &s("from"), &s("into"), &who).map(|m| m.is_some()),
            _ => Ok(false),
        };
        match result {
            Ok(true) => applied += 1,
            Ok(false) => {}
            Err(err) => tracing::debug!(error = %err, "notebook pass: a line not applied"),
        }
    }
    if !inbox.is_empty() {
        let who = Who::nightly(config, fallback);
        super::inbox_remove_locked(config, inbox, &who)?;
    }
    Ok(applied)
}

/// The scheduler: every ten minutes, the pass if it is due.
pub async fn scheduler(state: AppState) {
    tokio::time::sleep(Duration::from_secs(60)).await;
    // the pages from before the outlines put in theirs at once (no model), not at the first night
    let config = state.config();
    match tokio::task::spawn_blocking(move || super::refile_existing(&config)).await {
        Ok(Err(err)) => tracing::warn!(error = %err, "notebook pages not re-filed"),
        Err(err) => tracing::warn!(error = %err, "notebook pages not re-filed"),
        Ok(Ok(_)) => {}
    }
    loop {
        let config = state.config();
        if due(&config, Local::now()) {
            run(&config, &state.hermes, &state.http).await;
        }
        tokio::time::sleep(TICK).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(role: &str, text: &str) -> Dict {
        json!({"role": role, "content": text}).as_object().unwrap().clone()
    }

    #[test]
    fn the_pre_filter_keeps_what_looks_durable_and_not_filed() {
        assert!(looks_durable("L'anniversaire de Marc est le 14 novembre"));
        assert!(looks_durable("J'ai vu Sophie hier pour le devis"));
        assert!(looks_durable("My daughter starts school next week"));
        assert!(!looks_durable("merci !"));
        assert!(!looks_durable("quel temps fait-il aujourd'hui ?"));
        let mut filed = message("assistant", "Noté.");
        filed.insert(
            "tool_calls".into(),
            json!([{"function": {"name": "mcp_bipagents_notebook_note", "arguments": "{}"}}]),
        );
        let all = turns(&[
            message("user", "Rendez-vous chez le dentiste le 3 novembre"),
            filed,
            message("user", "Mon frère Paul arrive samedi"),
            message("assistant", "Super !"),
            message("user", "ok merci"),
            message("assistant", "Avec plaisir."),
        ]);
        assert_eq!(all.len(), 3);
        assert!(all[0].covered && !all[1].covered);
        let kept = keep(all);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].person, "Mon frère Paul arrive samedi");
    }
}
