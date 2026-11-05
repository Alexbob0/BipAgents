//! Sending through the bridge (docs/conversations.md §6.7): every message of the person goes to the bridge with the
//! app's own id (`client_id`), and the bridge decides what becomes of it, in one place:
//!
//! ```text
//! POST /v1/threads/{thread}/messages {agent?, client_id, text, attachments?, notes?, kinds?, after?, instructions?}
//!      thread: an id, `main`, or `new` (a new conversation)                     → {client_id, thread, state, run_id?}
//! POST /v1/threads/{thread}/stop {agent?}                                       → {stopped, run_id?}
//! ```
//!
//! - nothing runs in the conversation: a turn starts (`state: started`, its `run_id` followed as before through
//!   `/v1/runs/{id}/events`);
//! - a turn goes on: words only go into it (Hermes' steer, `steered`); with files, with `after` (« Après »), or when
//!   the turn takes no more words, the message waits in the agent's queue (msg_queue.rs, `queued`) and starts next;
//! - the same `client_id` again (a retry after a lost answer) changes nothing (`known`).
//!
//! The bridge keeps each message in `thread_sends` until Hermes has stored its row; the timeline then gives that item
//! the `client_id` (and, before, the message itself as an item `s_<client_id>` at the end), so an app replaces its
//! own bubble by id, never by words or time. Photos come inline (`data`, base64) and go to Hermes as image parts;
//! documents come stored by `POST /v1/files` (`path`) and go as one reference line each.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{text, threads_of, timeline::Item, unavailable, Param, Thread, Threads};
use crate::app::AppState;
use crate::config::AgentConfig;
use crate::msg_queue::{queues_of, Sent};
use crate::store::{iso, utcnow};
use crate::web::{check_max_len, ApiResult, FieldError, HttpError, Validate};

/// An app's id for a message (a UUID).
pub static CLIENT_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{8,64}\z").expect("valid regex"));
/// Files one message carries at most.
const ATTACHMENTS_MAX: usize = 10;
/// A message Hermes never stored (its turn failed before) stops showing after this long.
const SENDS_KEPT_SECONDS: i64 = 2 * 86_400;
/// Hermes' clock and the bridge's may differ a little (an existing Hermes on another machine).
const CLOCK_SLACK: f64 = 120.0;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Attachment {
    /// `image` (a photo, `data`) or `document` (`path`, from `POST /v1/files`).
    pub kind: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mime: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default, skip_serializing)]
    pub data: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NewMessage {
    #[serde(default)]
    pub agent: Option<String>,
    pub client_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    /// The app's notes for the agent only (its 👍 / 👎 since the last message): before the words, never shown.
    #[serde(default)]
    pub notes: Option<String>,
    /// The message's kinds (`voice`, `image`, `pdf`…), for a feature to propose (feature_tips.rs, never kept).
    #[serde(default)]
    pub kinds: Vec<String>,
    /// « Après »: waits for the turn going on to end, even with words only.
    #[serde(default)]
    pub after: bool,
    /// This turn's instructions (a read-only share's, read_shares.rs).
    #[serde(default)]
    pub instructions: Option<String>,
}

impl Validate for NewMessage {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "text", Some(&self.text), crate::msg_queue::TEXT_MAX);
        check_max_len(errors, "notes", self.notes.as_deref(), crate::msg_queue::TEXT_MAX);
        check_max_len(errors, "instructions", self.instructions.as_deref(), crate::msg_queue::TEXT_MAX);
        if !CLIENT_ID.is_match(&self.client_id) {
            errors.push(field("client_id", "8 to 64 letters, digits, - or _"));
        }
        if self.text.trim().is_empty() && self.attachments.is_empty() {
            errors.push(field("text", "words or a file"));
        }
        if self.attachments.len() > ATTACHMENTS_MAX
            || self.attachments.iter().any(|a| match a.kind.as_str() {
                "image" => a.data.is_none(),
                "document" => a.path.as_deref().is_none_or(str::is_empty),
                _ => true,
            })
        {
            errors.push(field("attachments", "photos with data, documents with a path, ten at most"));
        }
    }
}

fn field(name: &str, msg: &str) -> FieldError {
    FieldError::new("value_error", vec![json!("body"), json!(name)], msg, Value::Null)
}

#[derive(Debug, Default, Deserialize)]
pub struct Stop {
    #[serde(default)]
    pub agent: Option<String>,
}

impl Validate for Stop {}

/// What Hermes gets: the notes, the person's words and one reference line per document; photos as image parts of one
/// user message (`/v1/runs` reads a multimodal input as a list of messages).
pub fn input_of(notes: Option<&str>, words: &str, attachments: &[Attachment]) -> Value {
    let lines: Vec<String> = attachments
        .iter()
        .filter(|a| a.kind == "document")
        .map(|a| {
            format!(
                "[Pièce jointe : {} ({}, {}) → {}]",
                a.name.as_deref().unwrap_or("document"),
                a.mime.as_deref().unwrap_or("application/octet-stream"),
                crate::storage_watch::size_fr(a.size.unwrap_or(0)),
                a.path.as_deref().unwrap_or("")
            )
        })
        .collect();
    let said: Vec<&str> = [notes.unwrap_or("").trim(), words.trim()].into_iter().filter(|p| !p.is_empty()).collect();
    let text = [said.join("\n"), lines.join("\n")].into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
    let images: Vec<Value> = attachments
        .iter()
        .filter_map(|a| {
            let data = a.data.as_deref().filter(|_| a.kind == "image")?;
            let url = format!("data:{};base64,{data}", a.mime.as_deref().unwrap_or("image/jpeg"));
            Some(json!({"type": "image_url", "image_url": {"url": url}}))
        })
        .collect();
    if images.is_empty() {
        return Value::String(text);
    }
    let mut parts = if text.is_empty() { Vec::new() } else { vec![json!({"type": "text", "text": text})] };
    parts.extend(images);
    json!([{"role": "user", "content": parts}])
}

impl Threads {
    /// The thread's messages sent through the bridge: the items Hermes stored get their `client_id` (and the ones just
    /// stored are matched once, in order: the first person's message since the send holding its words), the others
    /// come as items `s_<client_id>` at the end — except those waiting in the queue (`pending.queue` shows them).
    pub async fn with_sends(&self, agent: &AgentConfig, thread: &Thread, items: &mut Vec<Item>, queued: &[String]) -> anyhow::Result<()> {
        let rows = self
            .store
            .run(
                "SELECT client_id, text, attachments, message_id, created_at FROM thread_sends \
                 WHERE agent = ? AND thread_id = ? ORDER BY created_at",
                vec![text(&agent.name), text(&thread.id)],
            )
            .await?;
        if rows.is_empty() {
            return Ok(());
        }
        let s = |row: &crate::store::Row, k: &str| row.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut claimed: std::collections::HashMap<String, String> =
            rows.iter().filter(|r| !s(r, "message_id").is_empty()).map(|r| (s(r, "message_id"), s(r, "client_id"))).collect();
        let since = |row: &crate::store::Row| {
            crate::store::parse_since(&s(row, "created_at")).map(|d| d.timestamp_micros() as f64 / 1e6).unwrap_or(0.0)
        };
        let now = utcnow().timestamp() as f64;
        let mut from = 0;
        let mut waiting = Vec::new();
        for row in rows.iter().filter(|r| s(r, "message_id").is_empty()) {
            let (client_id, words, at) = (s(row, "client_id"), s(row, "text"), since(row));
            let words = words.trim();
            let found = items.iter().enumerate().skip(from).find(|(_, i)| {
                i.kind() == "message"
                    && i.body.get("role") == Some(&json!("person"))
                    && !claimed.contains_key(&i.id)
                    && (i.at == 0.0 || i.at >= at - CLOCK_SLACK) // a row without its time (an older store): order only
                    && (if words.is_empty() { i.body.contains_key("attachments") } else { i.text().contains(words) })
            });
            if let Some((n, item)) = found {
                from = n + 1;
                claimed.insert(item.id.clone(), client_id.clone());
                self.store
                    .run(
                        "UPDATE thread_sends SET message_id = ? WHERE agent = ? AND client_id = ?",
                        vec![text(&item.id), text(&agent.name), text(&client_id)],
                    )
                    .await?;
            } else if now - at < SENDS_KEPT_SECONDS as f64 && !queued.contains(&client_id) {
                waiting.push((client_id, words.to_string(), at, s(row, "attachments")));
            }
        }
        for item in items.iter_mut() {
            if let Some(c) = claimed.get(&item.id) {
                item.body.insert("client_id".into(), json!(c));
            }
        }
        let last = items.last().map(|i| i.pos).unwrap_or((0, 0, 0));
        for (n, (client_id, words, at, attachments)) in waiting.into_iter().enumerate() {
            let id = format!("s_{client_id}");
            let files: Vec<Value> = serde_json::from_str::<Vec<Value>>(&attachments)
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(k, mut a)| {
                    a["id"] = json!(format!("{id}.a{k}"));
                    a
                })
                .collect();
            let mut body = serde_json::Map::new();
            body.insert("kind".into(), json!("message"));
            body.insert("role".into(), json!("person"));
            body.insert("text".into(), json!(words));
            body.insert("client_id".into(), json!(client_id));
            body.insert("sending".into(), json!(true));
            if !files.is_empty() {
                body.insert("attachments".into(), Value::Array(files));
            }
            // after every row there is now: what Hermes stores next reads after it (`after=` brings it)
            let pos = (last.0, last.1, last.2 + 1 + n as u32);
            items.push(Item { id, pos, at, turn: String::new(), segment: String::new(), row: 0, body });
        }
        Ok(())
    }
}

/// A message kept until Hermes stores it (`with_sends`): the person's words, its files' names (never their bytes).
pub async fn keep(
    threads: &Threads,
    agent: &str,
    thread: &str,
    client_id: &str,
    words: &str,
    attachments: &[Attachment],
) -> anyhow::Result<()> {
    threads
        .store
        .run(
            "INSERT OR IGNORE INTO thread_sends (agent, client_id, thread_id, text, attachments, message_id, created_at) \
             VALUES (?,?,?,?,?,NULL,?)",
            vec![
                text(agent),
                text(client_id),
                text(thread),
                text(words.trim()),
                match attachments.is_empty() {
                    true => Param::Null,
                    false => text(&serde_json::to_string(attachments).unwrap_or_default()),
                },
                text(&iso(utcnow())),
            ],
        )
        .await?;
    Ok(())
}

/// The conversation's tip, made when it has none yet (its first message): a Hermes session titled from the words.
async fn tip_of(state: &AppState, threads: &Threads, agent: &AgentConfig, thread: &Thread, words: &str) -> ApiResult<String> {
    if let Some(tip) = threads.line(agent, thread).await.map_err(unavailable)?.last() {
        return Ok(tip.session_id.clone());
    }
    let first: String = words.chars().take(60).collect();
    // Hermes refuses a title another conversation holds (400 `invalid_title`): a new conversation opened with the same
    // words as an earlier one (« Bonjour », the console's « ＋ », a relative's) failed with « isn't answering » and left
    // an empty thread (release bench of 4/11). Numbered then, as the background turns' work conversations are.
    let mut attempt = 1;
    let session = loop {
        let title = if attempt == 1 { first.clone() } else { format!("{first} ({attempt})") };
        match state.hermes.create_session(agent, &title).await {
            Ok(session) => break session,
            Err(crate::hermes::HermesError::Http { status: 400 | 409, .. }) if attempt < 9 => attempt += 1,
            Err(_) => return Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
        }
    };
    threads.link(agent, thread, &session).await?;
    Ok(session)
}

/// A message sent again (its answer lost on the way, or the person's « Réessayer »): `known`, and where it is now, so
/// the phone does what the first answer would have had it do. Waiting in the queue: `queued` (« En attente » shows
/// it); its conversation's turn going on: that turn's `run_id` (followed again); else neither (its turn is over: the
/// timeline has it). Nothing is sent twice. An app from before reads `known` alone (thin apps, 5/11).
async fn known_now(state: &AppState, threads: &Threads, agent: &AgentConfig, client_id: &str, thread: &str) -> Value {
    let queued = queues_of(state).get(&agent.name).items.iter().any(|i| i.client_id.as_deref() == Some(client_id));
    let tip = match threads.get(thread).await {
        Ok(Some(t)) => threads.line(agent, &t).await.ok().and_then(|line| line.last().map(|s| s.session_id.clone())),
        _ => None,
    };
    let run = tip.as_deref().filter(|_| !queued).and_then(|t| state.hub.session_run(&agent.name, t));
    json!({"client_id": client_id, "thread": thread, "state": "known", "run_id": run, "tip": tip, "queued": queued})
}

/// `POST /v1/threads/{thread}/messages`.
pub async fn send(state: &AppState, agent: &AgentConfig, thread: Option<Thread>, body: NewMessage) -> ApiResult<Value> {
    let threads = threads_of(state);
    // the same message again (an answer lost on the way): nothing sent twice, no conversation made twice
    let known = state
        .store
        .run(
            "SELECT thread_id FROM thread_sends WHERE agent = ? AND client_id = ?",
            vec![text(&agent.name), text(&body.client_id)],
        )
        .await?;
    if let Some(row) = known.first() {
        return Ok(known_now(state, &threads, agent, &body.client_id, row["thread_id"].as_str().unwrap_or("")).await);
    }
    let thread = match thread {
        Some(t) => t,
        None => threads.create(&agent.name, "side", None, Value::Null, false).await?,
    };
    let words = body.text.trim().to_string();
    let tip = tip_of(state, &threads, agent, &thread, &words).await?;
    let tip = tip.as_str();
    let answer = |state: &str, run: Option<&str>| {
        json!({"client_id": body.client_id, "thread": thread.id, "state": state, "run_id": run, "tip": tip})
    };
    keep(&threads, &agent.name, &thread.id, &body.client_id, &words, &body.attachments).await?;
    let queues = queues_of(state);
    let running = state.hub.session_run(&agent.name, tip);
    let waits = queues.get(&agent.name).items.iter().any(|i| i.session_id == tip);
    if let Some(run) = running.as_deref().filter(|_| !body.after && !waits && body.attachments.is_empty()) {
        // words into the turn going on (Hermes takes them at its next step)
        let steered = queues
            .steer(agent, tip, Some(run), &words, Some(&body.client_id))
            .await
            .map_err(|_| HttpError::new(502, "your agent isn't answering right now: try again in a moment"))?;
        if let Some(run) = steered {
            threads.bump(&thread, "sent").await?;
            return Ok(answer("steered", Some(&run)));
        }
    }
    // what waited for this message (a Live call's recap, the time, the family's news, a feature to propose…)
    let waited = crate::live_brief::message_notes(state, agent, tip, &words, &body.kinds).await;
    let notes = [waited, body.notes.clone()].into_iter().flatten().collect::<Vec<_>>().join("\n");
    let input = input_of(Some(&notes), &words, &body.attachments);
    if running.is_some() || waits || body.after {
        // first in line when the turn just stopped taking words, else after what waits
        let front = running.is_some() && !body.after && !waits;
        let sent = Sent { client_id: Some(body.client_id.clone()), input: Some(input) };
        queues
            .add(agent, tip, &words, running.as_deref(), front, sent)
            .map_err(|_| HttpError::new(409, "too many messages waiting"))?;
        threads.bump(&thread, "sent").await?;
        return Ok(answer("queued", None));
    }
    crate::cloud_setup::READINESS.wait(&agent.name, crate::cloud_setup::READY_WAIT).await;
    crate::mcp_sessions::SESSIONS.first_turn(&agent.name, crate::mcp_sessions::FIRST_TURN_WAIT).await;
    let instructions = body.instructions.as_deref().filter(|i| !i.trim().is_empty());
    let run = state.hermes.start_run_input(agent, input, tip, instructions).await.map_err(|exc| match exc.status() {
        Some(status @ (400 | 413 | 415 | 422)) => HttpError::new(status, "your agent refused this message"),
        _ => HttpError::new(502, "your agent isn't answering right now: try again in a moment"),
    })?;
    crate::welcome::done(&state.config(), &agent.name, Some(tip));
    // followed by the hub whoever looks (the brake, « reply ready »), and the queue waits for it
    state.hub.watch(agent, &run, Some(tip), true);
    state.hub.keep(agent, &run, Some(tip));
    tracing::info!(agent = %agent.name, run_id = %run, "message sent: a turn started");
    threads.bump(&thread, "sent").await?;
    Ok(answer("started", Some(&run)))
}

/// `POST /v1/threads/{thread}/stop`: the turn going on in the conversation stopped (Hermes then ends it cancelled).
pub async fn stop(state: &AppState, agent: &AgentConfig, thread: &Thread) -> ApiResult<Value> {
    let threads = threads_of(state);
    let tip = threads.line(agent, thread).await.map_err(unavailable)?.last().map(|s| s.session_id.clone());
    let Some(run) = tip.as_deref().and_then(|t| state.hub.session_run(&agent.name, t)) else {
        return Ok(json!({"stopped": false}));
    };
    match state.hermes.stop_run(agent, &run).await {
        Ok(()) => Ok(json!({"stopped": true, "run_id": run})),
        Err(exc) if matches!(exc.status(), Some(404 | 409)) => Ok(json!({"stopped": false, "run_id": run})),
        Err(_) => Err(HttpError::new(502, "your agent isn't answering right now: try again in a moment")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo() -> Attachment {
        Attachment { kind: "image".into(), name: None, mime: Some("image/png".into()), size: None, path: None, data: Some("AAAA".into()) }
    }

    #[test]
    fn words_go_as_a_string_photos_as_one_user_message() {
        assert_eq!(input_of(None, " Bonjour ", &[]), json!("Bonjour"));
        assert_eq!(input_of(Some("<!--bip:now 9 h-->"), "Bonjour", &[]), json!("<!--bip:now 9 h-->\nBonjour"));
        let input = input_of(None, "Et ça ?", &[photo()]);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0], json!({"type": "text", "text": "Et ça ?"}));
        assert_eq!(input[0]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        // a photo alone: no empty text part
        assert_eq!(input_of(None, "", &[photo()])[0]["content"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_document_goes_as_its_reference_line() {
        let doc = Attachment { kind: "document".into(), name: Some("devis.pdf".into()), mime: Some("application/pdf".into()),
                               size: Some(12_000), path: Some("/uploads/2026-11/abc-devis.pdf".into()), data: None };
        let input = input_of(None, "Regarde", &[doc]);
        assert_eq!(input, json!("Regarde\n\n[Pièce jointe : devis.pdf (application/pdf, 12 Ko) → /uploads/2026-11/abc-devis.pdf]"));
        // the timeline reads it back as the message's attachment, the words alone shown
        let (words, files) = super::super::timeline::visible(input.as_str().unwrap(), "m_x", 0);
        assert_eq!(words.trim(), "Regarde");
        assert_eq!(files[0]["name"], "devis.pdf");
    }

    #[test]
    fn a_message_needs_words_or_a_file_and_a_client_id() {
        let mut errors = Vec::new();
        let body: NewMessage = serde_json::from_value(json!({"client_id": "x", "text": " "})).unwrap();
        body.validate(&mut errors);
        assert_eq!(errors.len(), 2);
        let mut errors = Vec::new();
        let body: NewMessage =
            serde_json::from_value(json!({"client_id": "0F0E-1A2B-3C4D", "attachments": [{"kind": "image", "data": "AA"}]})).unwrap();
        body.validate(&mut errors);
        assert!(errors.is_empty());
    }
}
