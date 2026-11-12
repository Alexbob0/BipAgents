//! One pipeline for every turn an agent takes without the person (2026-11-03, dette-technique §5): a background task
//! (jobs.rs), a routine started by an event (triggers.rs), a watcher's wake-up (watchers.rs), the hand-back of finished
//! work (handback.rs: a task's report, a sub-agent's, the guide's new agent introducing itself). Each of them is only
//! a *starter* that decides **when**; this module does the rest, the same way for all:
//!
//! - **where**: the turn runs in a thread (docs/conversations.md), at its tip: the session Hermes goes on in after a
//!   compression, never a closed segment. A starter names the thread by any of its sessions, its id or `main`;
//!   [`Pipeline::work_session`] opens a work conversation of its own (a task's « 🧩 … », a trigger's of the day);
//! - **whether**: the month's AI credit used up holds every starter except the ones reporting work already done
//!   ([`Kind::needs_credit`]); a conversation (or, for a trigger, the agent) busy with another run is waited for;
//! - **the same prompt**: never run instructions (Hermes adds them to the system prompt, which then differs from the
//!   agent's conversations and loses the model server's prompt cache: cache audit #2, 2026-11-03); what a kind of
//!   turn must know (« this is a background task… ») travels in the turn's own message, as a hidden note;
//! - **how**: one Hermes run, followed by the run hub like a foreground turn (brake, approvals and questions pushed,
//!   file hand-over, thread events, Live Activities), its budget capped by the hub before a model call past it
//!   (`limit_tokens`), stopped after `max_seconds`;
//! - **what lands**: [`Pipeline::deliver`]: the agent's own message stays where it wrote it, one journal row pointing
//!   at it (pushed by the « Me prévenir » rules unless on screen), the files of the work attached to it as items
//!   (`Threads::attach`); a message the bridge writes instead (a template, a watcher's words) is a Boîte message in
//!   that thread.
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::{HermesClient, HermesError};
use crate::live_brief::Briefs;
use crate::outbox::{BoiteMessage, OutboxService};
use crate::push::PushService;
use crate::runs::RunHub;
use crate::tools_knowledge as tk;
use crate::{py, Dict};

/// The code word a turn without the person answers when there is nothing worth telling: no message, no push.
pub const SILENT: &str = "[SILENT]";

/// The rule a hidden note gives for it: the same words every run (the prompt cache), and a code word the model must
/// not translate (Alex 4/11: a French routine answered « [PAS UN MOT] », delivered as a report).
pub const SILENT_RULE: &str = "answer exactly [SILENT]: a code word for the app, never translated, kept exactly as \
written (in English, in brackets, nothing else), whatever the conversation's language";

/// A reply that tells the person nothing: empty, the code word (with anything after it), « SILENT » or « NO_REPLY »
/// without brackets (as Hermes' cron reads them), or only one short bracketed marker in any language (« [PAS UN MOT] »,
/// « [RIEN] », « [SILENCIO] »: the code word translated). A report that starts with a bracketed tag and goes on is not
/// silent.
pub fn is_silent(text: &str) -> bool {
    static NOTES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"<!--[\s\S]*?(?:-->|$)").expect("valid regex"));
    let bare = NOTES.replace_all(text, "");
    let t = bare.trim().trim_matches(|c: char| matches!(c, '`' | '*' | '_' | '"' | '«' | '»' | '“' | '”')).trim();
    if t.is_empty() || bare.trim_start().starts_with(SILENT) {
        return true;
    }
    let t = t.trim_end_matches(['.', '!']).trim();
    if ["SILENT", "NO_REPLY", "NO REPLY"].iter().any(|w| t.eq_ignore_ascii_case(w)) {
        return true;
    }
    let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) else { return false };
    let inner = inner.trim();
    inner.chars().count() <= 32
        && inner.chars().any(char::is_alphabetic)
        && inner.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.' | '!' | '\'' | '’'))
}

/// What a starter is (its credit rule, the brake's limits, the logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Finished work told where it was asked (a task's report, a sub-agent's).
    Handback,
    /// A background task (jobs.rs).
    Task,
    /// A routine started by an e-mail or a link (triggers.rs).
    Trigger,
    /// A watcher that saw a change (watchers.rs).
    Watcher,
    /// A routine on a clock (routines.rs).
    Routine,
    /// What the agent noted in a voice call to make once it ended (live_calls.rs `after_call`).
    AfterCall,
    /// A card answered that the turn which asked it ended without knowing (card_place.rs `settle`): the agent goes on.
    Card,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Handback => "handback",
            Kind::Task => "task",
            Kind::Trigger => "trigger",
            Kind::Watcher => "watcher",
            Kind::Routine => "routine",
            Kind::AfterCall => "after_call",
            Kind::Card => "card",
        }
    }

    /// Work the person did not just ask for stops with the month's credit; telling what is done never does.
    pub fn needs_credit(self) -> bool {
        !matches!(self, Kind::Handback)
    }

    /// A routine started by an event waits while the agent runs anything else (triggers.rs: one at a time).
    fn waits_for_agent(self) -> bool {
        self == Kind::Trigger
    }

    /// The brake's higher limits for long work (loop_brake.rs).
    fn long(self) -> bool {
        matches!(self, Kind::Task | Kind::Trigger | Kind::Routine | Kind::AfterCall)
    }
}

/// One turn to take.
#[derive(Debug, Clone)]
pub struct Turn {
    pub agent: AgentConfig,
    pub kind: Kind,
    /// What the agent reads (often a hidden `<!--bip:… -->` note the timeline leaves out).
    pub prompt: String,
    /// The destination: a thread id, `main`, or any session of a thread (the turn runs at its tip).
    pub thread: String,
    /// Tokens this turn may take (the hub stops it before a model call past them); None: no limit of its own.
    pub budget: Option<i64>,
    pub options: Option<Value>,
    pub max_seconds: f64,
    /// How long a busy conversation (or a trigger's agent) is waited for; 0: not looked at (a task in its own conversation).
    pub busy_wait: f64,
}

impl Turn {
    pub fn new(agent: &AgentConfig, kind: Kind, thread: &str, prompt: impl Into<String>) -> Turn {
        Turn {
            agent: agent.clone(),
            kind,
            prompt: prompt.into(),
            thread: thread.to_string(),
            budget: None,
            options: None,
            max_seconds: 360.0,
            busy_wait: 600.0,
        }
    }
}

/// A run's end as Hermes tells it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct End {
    /// `completed`, `failed`, `cancelled`, `interrupted`, or `gone` (Hermes forgot the run: its conversation's last
    /// reply is the output).
    pub status: String,
    pub output: String,
    pub error: String,
    pub usage: Option<Value>,
}

/// How a turn went.
#[derive(Debug, Clone, PartialEq)]
pub struct Ran {
    pub run_id: String,
    /// The session it ran in (the thread's tip).
    pub session: String,
    /// `completed`, `failed`, `cancelled`, `interrupted`, `gone` (Hermes forgot the run: its last reply read), `timeout`.
    pub status: String,
    /// Its final reply.
    pub text: String,
    pub tokens: i64,
}

impl Ran {
    /// Ended with words of the agent's.
    pub fn answered(&self) -> bool {
        (self.status == "completed" || self.status == "gone") && !self.text.is_empty()
    }
}

/// Why a turn did not run.
#[derive(Debug)]
pub enum NotRun {
    /// The month's AI credit is used up.
    Credit,
    /// The conversation (or the agent) stayed busy.
    Busy,
    /// Hermes no longer has the conversation.
    Gone,
    Refused(HermesError),
}

impl std::fmt::Display for NotRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotRun::Credit => write!(f, "the month's AI credit is used up"),
            NotRun::Busy => write!(f, "the conversation stayed busy"),
            NotRun::Gone => write!(f, "the conversation is gone"),
            NotRun::Refused(why) => write!(f, "the agent refused ({why})"),
        }
    }
}

/// What lands in a thread once the work is done.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub agent: AgentConfig,
    /// The thread's session the message belongs to.
    pub session: String,
    /// Once per work: the Boîte message's id, the journal's dedupe. Its prefix says whose message it is (its rule,
    /// notifications.rs): `handback:` a finished work's, `hermes-session:` a routine's.
    pub key: String,
    pub title: String,
    pub text: String,
    /// The agent's own reply, stored by Hermes in that thread (else the bridge's words: a Boîte message).
    pub as_written: bool,
    /// The run it came from (a reply's journal reference).
    pub run_id: Option<String>,
    /// Pushed (the journal's rules and the screen still decide); false: kept for the Boîte only.
    pub notify: bool,
    /// Files and pages of the work: attachments of the message, whatever its words say.
    pub files: Vec<String>,
}

/// The services a turn goes through (the bridge's own; the tests give fewer).
#[derive(Clone)]
pub struct Pipeline {
    pub config: SharedConfig,
    pub hermes: HermesClient,
    pub hub: Option<Arc<RunHub>>,
    pub push: Option<Arc<PushService>>,
    pub outbox: Option<Arc<OutboxService>>,
    pub briefs: Option<Arc<Briefs>>,
    /// The plan's AI credit as the guichet tells the place (credit.rs); None: the place's own file.
    pub credit_file: Option<PathBuf>,
    /// Seconds between two looks at a run.
    pub poll: f64,
}

/// A run's status and reply as a starter reads them. Hermes' explanation of a turn without an answer (« ⚠️ No reply:
/// … /model », alone or after a fragment, no_reply.rs) is no report: the turn did not finish (`failed`, no words), and
/// the starter says so in its own plain words (a routine: « couldn't finish », with when it tries again). Its English
/// was delivered as a routine's report before (11/11).
fn ended_with(status: String, output: &str) -> (String, String) {
    let text = tk::strip(output);
    if crate::no_reply::is_hermes_no_reply(text) || crate::no_reply::words_before(text).is_some() {
        tracing::warn!(target: "bipbridge::background_turn", status = %status, "the turn ended without an answer: no report");
        return ("failed".into(), String::new());
    }
    (status, text.to_string())
}

fn tokens_of(session: &Dict) -> i64 {
    let c = crate::jobs::counters(session);
    let n = |k: &str| c.get(k).and_then(Value::as_i64).unwrap_or(0);
    n("input") + n("output")
}

impl Pipeline {
    pub fn new(config: SharedConfig, hermes: HermesClient) -> Self {
        Pipeline { config, hermes, hub: None, push: None, outbox: None, briefs: None, credit_file: None, poll: 2.0 }
    }

    pub fn of(state: &crate::app::AppState) -> Self {
        crate::threads::threads_of(state); // the hub and the push know the conversations from now on
        Pipeline {
            hub: Some(state.hub.clone()),
            push: Some(state.push.clone()),
            outbox: Some(state.outbox.clone()),
            briefs: Some(state.briefs.clone()),
            ..Pipeline::new(state.config.clone(), state.hermes.clone())
        }
    }

    fn threads(&self) -> Option<Arc<crate::threads::Threads>> {
        let hub = self.hub.as_ref().and_then(|h| crate::threads::upgrade(&h.threads));
        hub.or_else(|| self.push.as_ref().and_then(|p| crate::threads::upgrade(&p.threads)))
    }

    pub fn credit_used(&self) -> bool {
        crate::credit::now_at(self.credit_file.as_deref()).paused()
    }

    /// The session a turn for `thread` runs in: the thread's tip (a thread id, `main`, or one of its sessions);
    /// the name as given when the bridge has no conversations (tests) or does not know it.
    pub async fn tip(&self, agent: &AgentConfig, thread: &str) -> String {
        let Some(threads) = self.threads() else { return thread.to_string() };
        let found = if thread == "main" || thread.starts_with("t_") {
            threads.named(Some(agent), thread).await.ok().map(|(_, t)| t)
        } else {
            threads.resolve(agent, thread).await.ok().flatten()
        };
        let Some(found) = found else { return thread.to_string() };
        match threads.line(agent, &found).await.ok().and_then(|s| s.last().map(|s| s.session_id.clone())) {
            Some(tip) => tip,
            None => thread.to_string(),
        }
    }

    /// A work conversation of its own (« 🧩 <title> », « ⚡ <name> · 2/11 », « Veilleur : … »): `kept` while Hermes
    /// still has it, else a new one titled `title` (numbered when another conversation holds that title).
    pub async fn work_session(&self, agent: &AgentConfig, title: &str, kept: &str) -> Result<String, HermesError> {
        if !kept.is_empty() && self.hermes.session(agent, kept).await.is_ok() {
            return Ok(kept.to_string());
        }
        let mut attempt = 1;
        loop {
            let named = if attempt == 1 { title.to_string() } else { format!("{title} ({attempt})") };
            match self.hermes.create_session(agent, &named).await {
                Ok(session) => return Ok(session),
                Err(HermesError::Http { status: 400 | 409, .. }) if attempt < 9 => attempt += 1,
                Err(err) => return Err(err),
            }
        }
    }

    /// The turn started: its run id and session. The hub follows it from here (its end is not pushed: `deliver`).
    pub async fn start(&self, turn: &Turn) -> Result<(String, String), NotRun> {
        if turn.kind.needs_credit() && self.credit_used() {
            return Err(NotRun::Credit);
        }
        let agent = &turn.agent;
        let session = self.wait_idle(turn).await?;
        let text = crate::dates::now::stamped(&self.config.get(), agent, &turn.prompt);
        let run_id = self.hermes.start_run_with(agent, &text, &session, None, turn.options.clone()).await.map_err(
            |e| match e {
                HermesError::Http { status: 404 | 410, .. } => NotRun::Gone,
                e => NotRun::Refused(e),
            },
        )?;
        self.follow(agent, turn.kind, &run_id, &session, turn.budget);
        tracing::info!(target: "bipbridge::background_turn", agent = %agent.name, kind = turn.kind.as_str(),
            run_id = %run_id, session = %session, "background turn started");
        Ok((run_id, session))
    }

    /// The session the turn goes to (its thread's tip) once nothing runs there any more (for a trigger: in the
    /// agent), waited for up to `busy_wait`.
    pub async fn wait_idle(&self, turn: &Turn) -> Result<String, NotRun> {
        let agent = &turn.agent;
        let session = self.tip(agent, &turn.thread).await;
        let mut waited = 0.0;
        while let Some(hub) = self.hub.as_ref().filter(|_| turn.busy_wait > 0.0) {
            let busy = if turn.kind.waits_for_agent() {
                hub.agent_running(&agent.name)
            } else {
                hub.session_busy(&agent.name, &session)
            };
            if !busy {
                break;
            }
            if waited >= turn.busy_wait {
                return Err(NotRun::Busy);
            }
            tokio::time::sleep(Duration::from_secs_f64(self.poll)).await;
            waited += self.poll;
        }
        Ok(session)
    }

    /// The run hub follows the turn's run (again after a restart): approvals and questions pushed, the brake's
    /// limits of its kind, its budget; its end is told by the starter (`deliver`).
    pub fn follow(&self, agent: &AgentConfig, kind: Kind, run_id: &str, session: &str, budget: Option<i64>) {
        let Some(hub) = &self.hub else { return };
        hub.watch(agent, run_id, Some(session).filter(|s| !s.is_empty()), false);
        // nobody started it from an app: what it asks while no app follows it waits as a card (held_approvals.rs)
        hub.mark_unattended(&agent.name, run_id);
        if kind.long() {
            hub.mark_background(&agent.name, run_id);
        }
        if let Some(budget) = budget {
            hub.limit_tokens(&agent.name, run_id, budget);
        }
    }

    /// The run's end, or None while it goes on.
    pub async fn status(&self, agent: &AgentConfig, run_id: &str, session: &str) -> Option<End> {
        match self.hermes.run(agent, run_id).await {
            Ok(run) => {
                let status = tk::str_or_empty(run.get("status"));
                let text = |k: &str| run.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                crate::jobs::TERMINAL.contains(&status.as_str()).then(|| End {
                    output: text("output"),
                    error: text("error"),
                    usage: run.get("usage").cloned(),
                    status,
                })
            }
            // Hermes restarted, or forgot the run long after it ended: its conversation's last reply
            Err(HermesError::Http { status: 404 | 410, .. }) => {
                let output = match self.hermes.session_messages(agent, session).await {
                    Ok(m) => crate::cronwatch::final_reply(&m).unwrap_or_default(),
                    Err(_) => String::new(),
                };
                Some(End { status: "gone".into(), output, ..End::default() })
            }
            Err(_) => None, // Hermes restarting: asked again next look
        }
    }

    /// Tokens a run took: Hermes' usage, else the hub's count of its calls, else what the conversation grew by.
    pub async fn tokens(
        &self,
        agent: &AgentConfig,
        run_id: &str,
        session: &str,
        usage: Option<&Value>,
        before: i64,
    ) -> i64 {
        let exact = usage.map(|u| {
            let n = |k: &str| u.get(k).map(|v| py::py_int(v).unwrap_or(0)).unwrap_or(0);
            n("input_tokens") + n("output_tokens")
        });
        if let Some(t) = exact.filter(|t| *t > 0) {
            return t;
        }
        if let Some(t) = self.hub.as_ref().and_then(|h| h.run_progress(&agent.name, run_id)).and_then(|p| p.usage) {
            return t.total();
        }
        let after = self.hermes.session(agent, session).await.map(|s| tokens_of(&s)).unwrap_or(before);
        (after - before).max(0)
    }

    /// Follows a started run to its end (stopped after `max_seconds`).
    pub async fn wait(&self, agent: &AgentConfig, run_id: &str, session: &str, max_seconds: f64, before: i64) -> Ran {
        let started = py::time_time();
        let end = loop {
            tokio::time::sleep(Duration::from_secs_f64(self.poll)).await;
            if let Some(end) = self.status(agent, run_id, session).await {
                break end;
            }
            if py::time_time() - started >= max_seconds {
                let _ = self.hermes.stop_run(agent, run_id).await;
                break End { status: "timeout".into(), ..End::default() };
            }
        };
        let (status, text) = ended_with(end.status, &end.output);
        let tokens = self.tokens(agent, run_id, session, end.usage.as_ref(), before).await;
        tracing::info!(target: "bipbridge::background_turn", agent = %agent.name, run_id = %run_id, status = %status,
            tokens = tokens, "background turn ended");
        Ran { run_id: run_id.to_string(), session: session.to_string(), status, text, tokens }
    }

    /// Starts the turn and follows it to its end.
    pub async fn run(&self, turn: &Turn) -> Result<Ran, NotRun> {
        self.run_then(turn, |_| {}).await
    }

    /// `run`, with `started` told the run's id as soon as Hermes took the turn (card_place.rs: an answer told once).
    pub async fn run_then(&self, turn: &Turn, started: impl FnOnce(&str)) -> Result<Ran, NotRun> {
        let session = self.tip(&turn.agent, &turn.thread).await;
        let before = self.hermes.session(&turn.agent, &session).await.map(|s| tokens_of(&s)).unwrap_or(0);
        let (run_id, session) = self.start(turn).await?;
        started(&run_id);
        Ok(self.wait(&turn.agent, &run_id, &session, turn.max_seconds, before).await)
    }

    /// The result in its thread (see the module's doc). False when nothing could be put there.
    pub async fn deliver(&self, d: &Delivery) -> bool {
        let agent = &d.agent;
        if d.as_written {
            if let (Some(threads), false) = (self.threads(), d.files.is_empty()) {
                let wait = self.config.get().push_message_wait_seconds;
                match threads.find_message(agent, &d.session, Some(&d.text), wait).await {
                    Some((thread, message)) => threads.attach(agent, &thread, &message, &d.files).await,
                    None => tracing::warn!(target: "bipbridge::background_turn", agent = %agent.name,
                        session = %d.session, "the work's files not attached: its message not found"),
                }
            }
            if let (true, Some(push)) = (d.notify, &self.push) {
                let run = d.run_id.clone().unwrap_or_else(|| format!("session:{}", d.session));
                push.notify_reply(&agent.name, &run, &d.text, Some(&d.session)).await;
            } else if let Some(threads) = self.threads() {
                threads.touch(&agent.name, Some(&d.session), "run").await;
            }
            return true;
        }
        let Some(outbox) = &self.outbox else { return false };
        let message = BoiteMessage {
            id: Some(d.key.clone()),
            text: d.text.clone(),
            files: d.files.clone(),
            title: Some(d.title.clone()),
            session_id: Some(d.session.clone()),
        };
        match outbox.ingest(agent, message, d.notify).await {
            Ok(_) => true,
            Err(err) => {
                tracing::warn!(target: "bipbridge::background_turn", agent = %agent.name, key = %d.key,
                    error = %format!("{err:#}"), "the Boîte refused the message");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn who_needs_the_credit_and_who_waits() {
        assert!(Kind::Task.needs_credit() && Kind::Trigger.needs_credit() && Kind::Watcher.needs_credit());
        assert!(!Kind::Handback.needs_credit());
        assert!(Kind::Trigger.waits_for_agent() && !Kind::Handback.waits_for_agent());
        assert!(Kind::Task.long() && !Kind::Watcher.long());
        assert!(Kind::AfterCall.needs_credit() && Kind::AfterCall.long() && !Kind::AfterCall.waits_for_agent());
    }

    #[test]
    fn silent_replies_in_any_language_and_only_those() {
        for quiet in ["", "  \n", "[SILENT]", " [SILENT]\n", "[SILENT] rien de neuf", "`[SILENT]`", "**[SILENT]**",
            "SILENT", "silent.", "no_reply", "[PAS UN MOT]", "[SILENCE]", "[RIEN]", "[SILENCIO]", "[STILL]", "[ Rien. ]",
            "« [RIEN] »", "[nichts_zu_melden]", "[AUCUN-MESSAGE]", "<!--bip:x-->[RIEN]", "[l’heure du silence]"] {
            assert!(is_silent(quiet), "{quiet:?} should be silent");
        }
        for said in ["[Météo] Soleil toute la journée.", "[RIEN] sauf un e-mail de la banque.", "Rien de neuf.",
            "SILENT NIGHT est passé à la radio", "[voir le rapport](https://example.com/r)",
            "[Pièce jointe : devis.pdf (application/pdf, 12 Ko) → /home/x/devis.pdf]",
            "[ce message est bien trop long pour être un simple marqueur]", "[42]", "[RIEN]\n[RIEN]", "Pas un mot."] {
            assert!(!is_silent(said), "{said:?} is a report");
        }
        // the note's words: the code word itself, the same text every run (the prompt cache)
        assert!(SILENT_RULE.contains(SILENT) && SILENT_RULE.contains("never translated"));
    }

    #[test]
    fn a_turn_answered_only_with_words() {
        let ran = |status: &str, text: &str| Ran {
            run_id: "r".into(),
            session: "s".into(),
            status: status.into(),
            text: text.into(),
            tokens: 0,
        };
        assert!(ran("completed", "ok").answered() && ran("gone", "ok").answered());
        assert!(!ran("completed", "").answered() && !ran("timeout", "").answered() && !ran("failed", "x").answered());
        assert_eq!(NotRun::Gone.to_string(), "the conversation is gone");
    }

    #[test]
    fn hermes_explanation_of_no_answer_is_no_report() {
        let end = |output: &str| ended_with("completed".into(), output);
        let failed = ("failed".to_string(), String::new());
        assert_eq!(end("⚠️ No reply: GLM didn't produce a reply this time, even after retries. Send `continue` to try \
                        again, or switch models with /model."), failed);
        assert_eq!(end("Je regarde\n\n⚠️ Pas de réponse : le flux s'est arrêté trop tôt."), failed);
        assert_eq!(end("  Soleil toute la journée.\n"), ("completed".into(), "Soleil toute la journée.".into()));
        assert_eq!(end("⚠️ Alerte orages ce soir."), ("completed".into(), "⚠️ Alerte orages ce soir.".into()));
        assert_eq!(ended_with("timeout".into(), ""), ("timeout".into(), String::new()));
    }
}
