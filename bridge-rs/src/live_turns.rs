//! A Live call's turns go through the bridge (3/11): the apps no longer call Hermes for them. Each turn runs in the
//! call's own short session (made by `POST /v1/live/calls/start`, live_brief.rs) on Hermes' `chat/stream`, whose SSE
//! answer is passed on as it comes (one request, nothing waited for: the first words as early as before), with the
//! agent's quick-turn thinking (`quick_turn_options`: off, or the least its model allows).
//!
//!     POST /v1/live/turns       {agent, live_session, text, attachments?}   → text/event-stream (Hermes' events)
//!     POST /v1/live/turns/stop  {agent, run_id}                             → {stopped}
//!
//! The first turn's text starts with the call's prelude (the time, the call's rules, its brief, the greeting said: a
//! hidden note the apps write, LiveLines.prelude); photos come inline and documents by path, as in a written message
//! (threads/send.rs). Approvals and questions are answered through `POST /v1/approve`.

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures::TryStreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::{AppState, RUN_ID, SESSION_ID};
use crate::threads::send::{input_of, Attachment};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, Validate};

#[derive(Debug, Deserialize)]
pub struct Turn {
    pub agent: String,
    pub live_session: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

impl Validate for Turn {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "agent", Some(&self.agent), 200);
        check_max_len(errors, "text", Some(&self.text), crate::live_brief::WARM_PRELUDE_MAX + crate::msg_queue::TEXT_MAX);
        if !SESSION_ID.is_match(&self.live_session) {
            errors.push(field("live_session", "a session id"));
        }
        if self.text.trim().is_empty() && self.attachments.is_empty() {
            errors.push(field("text", "words or a file"));
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Stop {
    pub agent: String,
    pub run_id: String,
}

impl Validate for Stop {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        if !RUN_ID.is_match(&self.run_id) {
            errors.push(field("run_id", "a run id"));
        }
    }
}

fn field(name: &str, msg: &str) -> FieldError {
    FieldError::new("value_error", vec![json!("body"), json!(name)], msg, Value::Null)
}

const UNREACHABLE: &str = "your agent isn't answering right now: try again in a moment";

async fn turn(State(state): State<AppState>, Json(body): Json<Turn>) -> ApiResult<Response> {
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&body.agent))?;
    // `chat/stream` takes the parts of one message (photos), not a list of messages as `/v1/runs` does
    let input = match input_of(None, &body.text, &body.attachments) {
        Value::Array(mut messages) => messages.swap_remove(0)["content"].take(),
        words => words,
    };
    let request = json!({"input": input, "model_options": crate::agents_admin::quick_turn_options(&config, &agent)});
    let upstream = state.hermes.chat_stream(&agent, &body.live_session, request).await.map_err(|exc| match exc.status() {
        Some(status @ (400..=499)) => HttpError::new(status, "your agent refused this message"),
        _ => HttpError::new(502, UNREACHABLE),
    })?;
    let mut response = Response::new(axum::body::Body::from_stream(upstream.bytes_stream().map_err(std::io::Error::other)));
    for (name, value) in [("content-type", "text/event-stream; charset=utf-8"), ("cache-control", "no-store"), ("x-accel-buffering", "no")] {
        response.headers_mut().insert(name, axum::http::HeaderValue::from_static(value));
    }
    Ok(response)
}

async fn stop(State(state): State<AppState>, Json(body): Json<Stop>) -> ApiResult<Response> {
    let agent = crate::app::agent_or_404(&state.config(), Some(&body.agent))?;
    match state.hermes.stop_run(&agent, &body.run_id).await {
        Ok(()) => Ok(web::ok(json!({"stopped": true}))),
        Err(exc) if matches!(exc.status(), Some(404 | 409)) => Ok(web::ok(json!({"stopped": false}))),
        Err(_) => Err(HttpError::new(502, UNREACHABLE)),
    }
}

/// Mounted under /v1 with the bridge key.
pub fn router() -> Router<AppState> {
    Router::new().route("/live/turns", post(turn)).route("/live/turns/stop", post(stop))
}
