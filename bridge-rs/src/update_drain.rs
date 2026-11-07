//! A BipAgents update gentle with the turns going on (2026-11-06: right after a release, `bipagents update` restarted
//! the bridge in the middle of a turn; its tool calls failed, Hermes' MCP circuit breaker paused all the bridge's tools
//! for a minute, three errors in a row, and the agent told Alex « j'ai un souci d'appel d'outil »).
//!
//!     POST /v1/update/drain  {seconds?, what?}  the updater (installer/bipinstall/drain.py), just before it restarts
//!                            the bridge (`what: "bridge"`, the default) or the agents' Hermes (`"gateways"`): answers
//!                            once no turn of the person's works (conversations, rooms, Live calls: the run hub; one
//!                            waiting for the person's answer loses nothing; for the gateways, whose restart ends
//!                            every run: every run, background tasks' too, and what each Hermes counts itself), or
//!                            after `seconds` (`CEILING` by default, at most `MAX_SECONDS`):
//!                            `{drained, turns, waited}`. Then « closing »: the agents' tool calls are answered
//!                            « Mise à jour en cours : réessaie dans une minute » (in the agent's language) until the
//!                            bridge stops, `done`, or `CLOSING_SECONDS`.
//!     POST /v1/update/drain  {done: true}  the update restarted something else than this bridge: back to normal
//!     GET  /v1/update/drain  {state: idle|draining|closing, since, until, turns}
//!
//! The answer while closing is a plain result, not an error (`isError` false): Hermes (v0.22.5,
//! tools/mcp_tool_handlers.py `_record_call_outcome`) counts a strike for its breaker only for a result whose JSON has
//! an `error` key, which is what an `isError` result becomes; a plain one even closes the breaker. The agent then tells
//! the person it goes on in a minute, instead of a failure.
//!
//! The bridge also closes when it is asked to stop (SIGTERM: main_cli.rs), for the requests already under way.
//!
//! Nothing is written to disk: a bridge that restarts never starts « closing », and one whose updater went away goes
//! back to normal by itself (a drain past its `until`, a closing past `CLOSING_SECONDS`).

use std::sync::Mutex;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use crate::app::AppState;
use crate::web::{self, ApiResult};
use crate::{py, Dict};

/// The updater's wait when it says none (seconds): 5 minutes.
pub const CEILING: f64 = 300.0;
/// No drain longer than this, whatever is asked.
pub const MAX_SECONDS: f64 = 1800.0;
/// Closing lasts at most this long (the updater restarts the bridge within seconds of the drain's answer).
pub const CLOSING_SECONDS: f64 = 120.0;
/// The run hub looked at this often while draining; each Hermes asked (gateways) every few looks.
const LOOK: Duration = Duration::from_millis(500);
const HERMES_EVERY: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum What {
    /// The bridge restarts: the person's turns lose their tools for a few seconds.
    Bridge,
    /// The agents' Hermes restart: every run ends.
    Gateways,
}

impl What {
    pub fn parse(text: Option<&str>) -> What {
        match text {
            Some("gateways") => What::Gateways,
            _ => What::Bridge,
        }
    }
    fn name(self) -> &'static str {
        match self {
            What::Bridge => "bridge",
            What::Gateways => "gateways",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Idle,
    Draining { since: f64, until: f64 },
    Closing { since: f64, until: f64 },
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);

fn set(phase: Phase) {
    *PHASE.lock().unwrap_or_else(|e| e.into_inner()) = phase;
}

/// The phase at `now`: a drain or a closing past its time is over (an updater killed, or one that never restarted us).
pub fn phase_at(phase: Phase, now: f64) -> Phase {
    match phase {
        // the drain's own loop ends it at `until`; a little later, its request is gone
        Phase::Draining { until, .. } if now > until + 5.0 => Phase::Idle,
        Phase::Closing { until, .. } if now > until => Phase::Idle,
        other => other,
    }
}

pub fn phase() -> Phase {
    phase_at(*PHASE.lock().unwrap_or_else(|e| e.into_inner()), py::time_time())
}

/// The agents' tool calls get `busy_text` now.
pub fn closing() -> bool {
    matches!(phase(), Phase::Closing { .. })
}

/// The bridge is asked to stop (main_cli.rs): the requests still under way are told the same.
pub fn stopping() {
    let now = py::time_time();
    set(Phase::Closing { since: now, until: now + CLOSING_SECONDS });
}

/// The seconds asked for, bounded (none, or not a number: `CEILING`).
pub fn seconds_of(asked: &Dict) -> f64 {
    asked.get("seconds").and_then(Value::as_f64).filter(|s| s.is_finite()).unwrap_or(CEILING).clamp(0.0, MAX_SECONDS)
}

/// A tool call's answer while the bridge closes, in the agent's language: not an error (Hermes' breaker doesn't count
/// it), and what to tell the person.
pub fn busy_text(lang: &str) -> &'static str {
    match lang {
        "fr" => BUSY_FR,
        "de" => BUSY_DE,
        "es" => BUSY_ES,
        _ => BUSY_EN,
    }
}

const BUSY_FR: &str = "Mise à jour en cours : réessaie dans une minute. BipAgents redémarre pour se mettre à jour, \
rien n'est cassé. Si ta personne attend, dis-lui simplement que tu reprends dans une minute.";
const BUSY_EN: &str = "Update in progress: try again in a minute. BipAgents is restarting for an update, nothing is \
broken. If your person is waiting, just tell them you'll pick this up in a minute.";
const BUSY_DE: &str = "Update läuft: versuch es in einer Minute noch einmal. BipAgents startet für ein Update neu, \
nichts ist kaputt. Wartet deine Person, sag ihr einfach, dass du in einer Minute weitermachst.";
const BUSY_ES: &str = "Actualización en curso: vuelve a intentarlo dentro de un minuto. BipAgents se reinicia para \
actualizarse, no hay ninguna avería. Si tu persona espera, dile simplemente que sigues dentro de un minuto.";

/// `busy_text` as an MCP tool result (the bridge's other MCP servers: files, card payments, connectors).
pub fn busy_result(lang: &str) -> Value {
    json!({"content": [{"type": "text", "text": busy_text(lang)}], "isError": false})
}

/// The hub's runs a restart of `what` would cut, now. The bridge's: those that work (not background tasks', whose
/// tool calls only wait a few seconds, nor those waiting for their person: nothing lost). The gateways': every run
/// it follows (their restart ends them all).
fn hub_turns(state: &AppState, what: What) -> usize {
    match what {
        What::Bridge => {
            let task_runs: Vec<String> = crate::jobs::tasks(&state.config())
                .into_iter()
                .filter_map(|j| j.get("run_id").and_then(Value::as_str).map(str::to_string))
                .collect();
            state.hub.working(&task_runs)
        }
        What::Gateways => state.hub.running(&[]),
    }
}

/// The agents whose Hermes says it works now, by its own count (a turn no bridge follows, a background task).
async fn hermes_turns(state: &AppState) -> usize {
    let mut busy = 0;
    for agent in state.config().agents.values() {
        if crate::agent_restart::hermes_busy(state, agent).await == Some(true) {
            busy += 1;
        }
    }
    busy
}

/// The drain: answers once no turn goes on, or at `seconds`; closing from then on.
pub async fn drain(state: &AppState, what: What, seconds: f64) -> Dict {
    let started = py::time_time();
    let until = started + seconds;
    set(Phase::Draining { since: started, until });
    let mut looks = 0u32;
    let mut last = usize::MAX;
    // the gateways: each Hermes asked at the first look and every few after, its count kept in between
    let mut hermes = 0;
    let turns = loop {
        if what == What::Gateways && looks.is_multiple_of(HERMES_EVERY) {
            hermes = hermes_turns(state).await;
        }
        looks += 1;
        let turns = hub_turns(state, what).max(hermes);
        if turns != last && turns > 0 {
            tracing::info!(target: "bipbridge::update", what = what.name(), turns, "update waits for the turns going on");
        }
        last = turns;
        if turns == 0 || py::time_time() >= until {
            break turns;
        }
        tokio::time::sleep(LOOK).await;
    };
    let now = py::time_time();
    set(Phase::Closing { since: now, until: now + CLOSING_SECONDS });
    let waited = (now - started).max(0.0);
    if turns == 0 {
        tracing::info!(target: "bipbridge::update", what = what.name(), waited = waited.round(), "no turn going on: ready for the update");
    } else {
        tracing::warn!(target: "bipbridge::update", what = what.name(), turns, waited = waited.round(),
            "turns still going on at the drain's ceiling: the update goes on");
    }
    let mut out = Dict::new();
    out.insert("drained".into(), json!(turns == 0));
    out.insert("turns".into(), json!(turns));
    out.insert("waited".into(), json!((waited * 10.0).round() / 10.0));
    out.insert("closing_seconds".into(), json!(CLOSING_SECONDS));
    out
}

fn body_of(bytes: &Bytes) -> Dict {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(m)) => m,
        _ => Dict::new(),
    }
}

async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let (name, since, until) = match phase() {
        Phase::Idle => ("idle", None, None),
        Phase::Draining { since, until } => ("draining", Some(since), Some(until)),
        Phase::Closing { since, until } => ("closing", Some(since), Some(until)),
    };
    let turns = hub_turns(&state, What::Bridge);
    Ok(web::ok(json!({"state": name, "since": since, "until": until, "turns": turns})))
}

async fn drain_route(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let asked = body_of(&body);
    if asked.get("done").and_then(Value::as_bool) == Some(true) {
        if !matches!(phase(), Phase::Idle) {
            tracing::info!(target: "bipbridge::update", "update done: tool calls served again");
        }
        set(Phase::Idle);
        return Ok(web::ok(json!({"state": "idle"})));
    }
    let what = What::parse(asked.get("what").and_then(Value::as_str));
    Ok(web::ok(Value::Object(drain(&state, what, seconds_of(&asked)).await)))
}

/// Mounted under /v1 with the bridge key (app.rs).
pub fn router() -> Router<AppState> {
    Router::new().route("/update/drain", get(status).post(drain_route))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drain_or_a_closing_left_behind_ends_by_itself() {
        let draining = Phase::Draining { since: 100.0, until: 400.0 };
        assert_eq!(phase_at(draining, 300.0), draining);
        assert_eq!(phase_at(draining, 404.0), draining); // its own loop ends it
        assert_eq!(phase_at(draining, 406.0), Phase::Idle); // its request went away
        let closing = Phase::Closing { since: 400.0, until: 400.0 + CLOSING_SECONDS };
        assert_eq!(phase_at(closing, 450.0), closing);
        assert_eq!(phase_at(closing, 400.0 + CLOSING_SECONDS + 1.0), Phase::Idle);
        assert_eq!(phase_at(Phase::Idle, 0.0), Phase::Idle);
    }

    #[test]
    fn the_seconds_asked_are_bounded() {
        let asked = |v: Value| v.as_object().cloned().unwrap_or_default();
        assert_eq!(seconds_of(&asked(json!({}))), CEILING);
        assert_eq!(seconds_of(&asked(json!({"seconds": 20}))), 20.0);
        assert_eq!(seconds_of(&asked(json!({"seconds": -5}))), 0.0);
        assert_eq!(seconds_of(&asked(json!({"seconds": 99_999}))), MAX_SECONDS);
        assert_eq!(seconds_of(&asked(json!({"seconds": "soon"}))), CEILING);
        assert_eq!(What::parse(Some("gateways")), What::Gateways);
        assert_eq!(What::parse(Some("bridge")), What::Bridge);
        assert_eq!(What::parse(None), What::Bridge);
    }

    #[test]
    fn the_answer_while_closing_says_to_try_again_in_the_agents_language() {
        assert!(busy_text("fr").starts_with("Mise à jour en cours : réessaie dans une minute."));
        assert!(busy_text("en").starts_with("Update in progress: try again in a minute."));
        assert!(busy_text("de").starts_with("Update läuft"));
        assert!(busy_text("es").starts_with("Actualización en curso"));
        assert_eq!(busy_text("it"), busy_text("en"));
        for lang in ["fr", "en", "de", "es"] {
            // never JSON (with an `error` key Hermes would count it), and one line of plain words
            assert!(serde_json::from_str::<Value>(busy_text(lang)).is_err());
            assert!(!busy_text(lang).contains("  "), "{lang}");
        }
    }
}
