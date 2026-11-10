//! A Live call's turns go through the bridge (3/11): the apps no longer call Hermes for them. Each turn runs in the
//! call's own short session (made by `POST /v1/live/calls/start`, live_brief.rs) on Hermes' `chat/stream`, whose SSE
//! answer is passed on as it comes (one request, nothing waited for: the first words as early as before), with the
//! agent's quick-turn thinking (`quick_turn_options`: off, or the least its model allows) and the media and long tools
//! refused (`LIVE_CLOSED_TOOLS`, `live_turn_options`).
//!
//!     POST /v1/live/turns       {agent, live_session, text, attachments?}   → text/event-stream (Hermes' events)
//!     POST /v1/live/turns/stop  {agent, run_id}                             → {stopped}
//!
//! The first turn's text starts with the call's prelude (the time, the call's rules, its brief, the greeting said: a
//! hidden note the apps write, LiveLines.prelude); photos come inline and documents by path, as in a written message
//! (threads/send.rs). Approvals and questions are answered through `POST /v1/approve`.
//!
//! A turn the phone's voice decider sent to be filed, not answered (`noter`, 9/11), starts with a hidden
//! `<!--bip:note … -->`: whatever the app wrote in it (the 9/11 apps named a Carnet v1 tool, `notebook_note`, that is
//! gone), the bridge puts its own words there (`NOTE`), so the tools' names live here only.

use axum::extract::State;
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use futures::StreamExt;
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

/// The tools a Live turn may not run (4/11: asked for help with a pronunciation in a call, an agent started making an
/// audio file while the person waited on the line): what makes media or long outputs (audio and podcasts, Hermes'
/// speech, images and videos, pages, files written, sub-agents, background and heavy work started, the browsers).
/// The call's rules (the apps' prelude, LiveLines `callRules`) say to speak it instead and note what is to be made with
/// `after_call`, made once the call ends (live_calls.rs). Quick ones stay: web search, the Carnet, calendar and
/// reminders, mail, maps, messages to other agents, a background task proposed (`propose_task`), `after_call`.
/// Hermes patch 0024 (`model_options.closed_tools`, a trailing `*` closes a prefix) refuses them in its dispatcher
/// (`tool_call` included) with « say it instead, offer it for after the call »; the request still lists every tool, so
/// a call's prompt is its conversations' one in the model server's cache. A Hermes without the patch ignores the field:
/// the rules still say it.
pub const LIVE_CLOSED_TOOLS: [&str; 17] = [
    "browser_*",
    "computer_use",
    "delegate_task",
    "image_generate",
    "mcp__bipagents__create_audio",
    "mcp__bipagents__create_page",
    "mcp__bipagents__create_podcast",
    "mcp__bipagents__heavy_work",
    "mcp__bipagents__start_task",
    "mcp__bipagents_files__shared_files_write",
    "mcp__stealth__*",
    "patch",
    "text_to_speech",
    "video_generate",
    "write_file",
    "xai_video_edit",
    "xai_video_extend",
];

/// The `model_options` of a Live call's turns and of its warm-up (live_brief.rs `warm_up`): the agent's quick-turn
/// thinking, `LIVE_CLOSED_TOOLS` closed.
pub fn live_turn_options(config: &crate::config::Config, agent: &crate::config::AgentConfig) -> Value {
    let mut options = crate::agents_admin::quick_turn_options(config, agent);
    options["closed_tools"] = json!(LIVE_CLOSED_TOOLS);
    options
}

/// What the agent reads for a turn to file (`with_note`): one notebook_write per page, no search first, the label
/// said again replacing the old fact, never a code; then two words.
pub const NOTE: &str = "<!--bip:note\nYour person said this to have it filed, not answered. File it now with notebook_write, \
one call per page (its name, a first name is enough, no search first; a place or subject is not a person: give its \
kind): « Label : value », the same label replaces the \
old fact, a correction too. Never a code or password: say in a few words you don't keep those. Then answer only « OK, \
noté » in your language.\n-->";

/// `text` with its `<!--bip:note … -->` (wherever it is: the first turn has the call's prelude before it) in the
/// bridge's words; other hidden notes (`<!--bip:notes-->`…) and a text without one unchanged.
pub fn with_note(text: &str) -> std::borrow::Cow<'_, str> {
    const OPEN: &str = "<!--bip:note";
    let mut from = 0;
    while let Some(at) = text[from..].find(OPEN).map(|i| i + from) {
        let after = &text[at + OPEN.len()..];
        if after.starts_with(char::is_whitespace) || after.starts_with("-->") {
            let end = after.find("-->").map_or(text.len(), |i| at + OPEN.len() + i + 3);
            return format!("{}{NOTE}{}", &text[..at], &text[end..]).into();
        }
        from = at + OPEN.len();
    }
    text.into()
}

const UNREACHABLE: &str = "your agent isn't answering right now: try again in a moment";

async fn turn(State(state): State<AppState>, Json(body): Json<Turn>) -> ApiResult<Response> {
    let config = state.config();
    let agent = crate::app::agent_or_404(&config, Some(&body.agent))?;
    // `chat/stream` takes the parts of one message (photos), not a list of messages as `/v1/runs` does
    let input = match input_of(None, &with_note(&body.text), &body.attachments) {
        Value::Array(mut messages) => messages.swap_remove(0)["content"].take(),
        words => words,
    };
    let request = json!({"input": input, "model_options": live_turn_options(&config, &agent)});
    let upstream = state.hermes.chat_stream(&agent, &body.live_session, request).await.map_err(|exc| match exc.status() {
        Some(status @ (400..=499)) => HttpError::new(status, "your agent refused this message"),
        _ => HttpError::new(502, UNREACHABLE),
    })?;
    let mut response = Response::new(axum::body::Body::from_stream(without_media(upstream.bytes_stream())));
    for (name, value) in [("content-type", "text/event-stream; charset=utf-8"), ("cache-control", "no-store"), ("x-accel-buffering", "no")] {
        response.headers_mut().insert(name, axum::http::HeaderValue::from_static(value));
    }
    Ok(response)
}

/// Hermes' events as the apps get them: a reply's `MEDIA:` lines out (media.rs), the files at the end as its
/// `attachments`; its keepalives passed on.
fn without_media<S, E>(chunks: S) -> impl futures::Stream<Item = Result<bytes::Bytes, std::io::Error>> + Send
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    async_stream::stream! {
        let lines = crate::hermes::lines_of(chunks);
        futures::pin_mut!(lines);
        let (mut parser, mut media) = (crate::hermes::SSEParser::new(), crate::media::MediaFilter::default());
        let mut done = false;
        while !done {
            let event = match lines.next().await {
                Some(Ok(line)) if line.starts_with(':') => {
                    yield Ok(bytes::Bytes::from(format!("{line}\n\n")));
                    continue;
                }
                Some(Ok(line)) => parser.feed_line(&line),
                Some(Err(exc)) => {
                    yield Err(std::io::Error::other(exc));
                    return;
                }
                None => {
                    done = true;
                    parser.finish()
                }
            };
            let Some(event) = event else { continue };
            let mut shown = Vec::new();
            if crate::hermes::is_terminal(&event.r#type) || event.r#type == "assistant.completed" {
                shown = media.flush_events("assistant.delta", event.data.get("run_id").unwrap_or(&Value::Null));
            }
            shown.extend(media.event(event));
            for event in shown {
                yield Ok(frame(&event.r#type, &event.raw));
            }
        }
    }
}

fn frame(kind: &str, data: &str) -> bytes::Bytes {
    bytes::Bytes::from(format!("event: {kind}\ndata: {}\n\n", data.replace('\n', "\ndata: ")))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn closes(name: &str) -> bool {
        LIVE_CLOSED_TOOLS.iter().any(|c| match c.strip_suffix('*') {
            Some(prefix) => name.starts_with(prefix),
            None => name == *c,
        })
    }

    #[test]
    fn a_note_to_file_gets_the_bridges_words() {
        // what the 9/11 apps send (VoiceDecider noteTurn), a v1 tool's name in it
        let old = "<!--bip:note\nYour person said this to have it filed, not answered: keep it in the Carnet (notebook_note), then \
answer only « OK, noté » in your language, nothing more.\n-->\npour info le plombier vient jeudi";
        assert_eq!(with_note(old), format!("{NOTE}\npour info le plombier vient jeudi"));
        // after the call's prelude, in the first turn
        let first = "<!--bip:now x-->\n<!--bip:live\nrules <!--bip:end-->\n\n<!--bip:note\nkeep it\n-->\nnote que Léa chausse du 36";
        assert_eq!(with_note(first), format!("<!--bip:now x-->\n<!--bip:live\nrules <!--bip:end-->\n\n{NOTE}\nnote que Léa chausse du 36"));
        for same in ["bonjour", "<!--bip:notes-->[Request]", "<!--bip:notebook x-->", "je note que <!--bip:noted"] {
            assert_eq!(with_note(same), same);
        }
        assert!(NOTE.contains(crate::notebook::tools::WRITE) && !NOTE.contains("notebook_note"));
        assert!(NOTE.ends_with("-->") && NOTE.matches("-->").count() == 1);
        // a note never closed: the rest is the note's
        assert_eq!(with_note("<!--bip:note\nkeep it"), NOTE);
    }

    #[test]
    fn a_call_closes_the_media_and_long_tools_and_keeps_the_quick_ones() {
        let bridge = |name: &str| format!("{}{name}", crate::tool_diet::BRIDGE_PREFIX);
        for made in [crate::agent_tools::CREATE_AUDIO, crate::agent_tools::CREATE_PODCAST, crate::agent_tools::CREATE_PAGE,
            crate::jobs::START_TASK, crate::heavy_work::HEAVY_WORK]
        {
            assert!(closes(&bridge(made)), "{made}");
        }
        for native in ["text_to_speech", "image_generate", "write_file", "browser_navigate", "mcp__stealth__navigate"] {
            assert!(closes(native), "{native}");
        }
        for quick in [crate::agent_tools::PROPOSE_TASK, crate::agent_tools::KNOWLEDGE_SEARCH, crate::maps::TOOL_NAME,
            crate::routes::TOOL_NAME, crate::weather::TOOL_NAME, crate::agent_messages::TOOL_NAME,
            crate::notebook::tools::SEARCH, crate::notebook::tools::WRITE, "calendar_list", "reminders_add", "mail_search",
            "mail_read", crate::agent_tools::LIST_PAGES, crate::agent_tools::READ_PAGE, crate::live_calls::AFTER_CALL]
        {
            assert!(!closes(&bridge(quick)), "{quick}");
        }
        for quick in ["web_search", "web_extract", "clarify", "memory", "read_file", "terminal", "tool_search"] {
            assert!(!closes(quick), "{quick}");
        }
        // Hermes 0024 takes at most 128 names without spaces; one fixed list, in order
        assert!(LIVE_CLOSED_TOOLS.len() <= 128 && LIVE_CLOSED_TOOLS.iter().all(|n| !n.contains(char::is_whitespace)));
        assert!(LIVE_CLOSED_TOOLS.windows(2).all(|w| w[0] < w[1]));
    }
}
