//! Every notification the bridge sends, as data: what one `PushService::notify_*` is given (`Notice`), the event it
//! makes (`Notice::event`, what `PushService::emit` journals and pushes) and the pure function that turns it into the
//! push (`build`). `PushService` adds what needs the bridge's state (the agent's shown name and language, the thread
//! and message the event points at, the journal id and badge, the « Me prévenir » switch) and sends it.
//!
//! The contract: `contracts/notifications/types/<type>.json` at the repository's root holds, for each type, inputs
//! of `build` and the exact payload it must give (tests/notifications_contract.rs), read by the apps' tests and the
//! relay's too. A new `Notice` needs its contract file: the test fails without it.

use serde::Deserialize;
use serde_json::{json, Value};

use super::seal::Sealed;
use super::{approval_line, email_held_line, fnv, given, login_line, payload, silent_payload, Event};
use crate::notifications::Kind;
use crate::Dict;

/// What the bridge knows of the agent when it builds a push.
#[derive(Clone, Debug, Deserialize)]
pub struct Context {
    /// The agent's shown name (« Vie »), the sealed notification's title.
    pub title: String,
    /// The agent's language (« fr », « en », « de », « es »), for the lines written here.
    pub lang: String,
}

/// Where the event points once `emit` resolved it, and its journal row: the thread of its session (or the main
/// thread for a Boîte message), the stored message a reply waited for, the journal id, the unread total.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Pointer {
    pub n: String,
    #[serde(default)]
    pub thread: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub badge: i64,
}

/// One notification, as the `notify_*` producing it is called.
/// Its serde names are the `notify_*` method names: the contract files say `{"fn": "notify_reply", "args": {…}}`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "fn", content = "args", deny_unknown_fields)]
pub enum Notice {
    /// A Boîte message (outbox): routine report, watcher, hand-back outside the main thread, credit, storage, imports.
    #[serde(rename = "notify_message")]
    Message { outbox_id: String, text: String, session_id: Option<String> },
    /// An approval Hermes waits for (`room_id`: a group discussion's turn).
    #[serde(rename = "notify_approval_values")]
    Approval {
        run_id: String,
        request_id: Option<String>,
        choices: Option<Vec<Value>>,
        command: Option<String>,
        session_id: Option<String>,
        room_id: Option<String>,
    },
    /// A run's reply nobody saw (also the main thread's autonomous turns and hand-backs: `run_id` « session:<sid> »).
    #[serde(rename = "notify_reply")]
    Reply { run_id: String, text: String, session_id: Option<String> },
    /// An agent's reply in a group discussion.
    #[serde(rename = "notify_room")]
    Room { room_id: String, room_title: String, text: String },
    /// A sub-task's report Hermes left in the conversation.
    #[serde(rename = "notify_subtask")]
    Subtask { session_id: String, line: Option<String> },
    /// A step of a mailbox scan.
    #[serde(rename = "notify_scan")]
    Scan { line: String },
    /// A background task ended, paused, was proposed or rescued (`body`: its generic line).
    #[serde(rename = "notify_job")]
    Job { session_id: Option<String>, line: String, job_id: String, body: String },
    /// The agent asks something mid-run (clarify).
    #[serde(rename = "notify_question")]
    Question { run_id: String, request_id: Option<String>, question: Option<String>, session_id: Option<String> },
    /// Someone of the family passed something on to this agent.
    #[serde(rename = "notify_family")]
    Family { line: String, note_id: String },
    /// An e-mail to an address not allowed yet waits for the person.
    #[serde(rename = "notify_email_held")]
    EmailHeld { to: String, held_id: String },
    /// The agent's programs want to send data to a site.
    #[serde(rename = "notify_site_request")]
    Site { line: String, id: String },
    /// An account to create with a password of the vault.
    #[serde(rename = "notify_login_request")]
    Login { host: String, id: String },
    /// A connector's, a secret's or a mail connector's card (`key`: `connector_request`, `secret_request`,
    /// `mail_request`).
    #[serde(rename = "notify_bridge_card")]
    Card { line: String, key: String, id: String },
    /// A date to find with a relative's household.
    #[serde(rename = "notify_meet")]
    Meet { line: String, id: String },
    /// A silent push (`run_finished`, `reminders`, `calendar`, `contacts`, `read`).
    #[serde(rename = "notify_silent")]
    Silent {
        reason: String,
        #[serde(default)]
        extra: Dict,
    },
}

/// The `notify_*` names `Notice` stands for, one per variant: each must have a contract file.
pub const BUILDERS: [&str; 15] = [
    "notify_message",
    "notify_approval_values",
    "notify_reply",
    "notify_room",
    "notify_subtask",
    "notify_scan",
    "notify_job",
    "notify_question",
    "notify_family",
    "notify_email_held",
    "notify_site_request",
    "notify_login_request",
    "notify_bridge_card",
    "notify_meet",
    "notify_silent",
];

/// A push ready to fan out.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    pub payload: Dict,
    /// APNs `apns-push-type`: « alert » or « background ».
    pub push_type: &'static str,
    pub priority: u8,
    /// APNs collapse id (FCM collapse key): a later push with the same one replaces it.
    pub collapse_id: Option<String>,
}

impl Notice {
    /// Its `notify_*` name (`BUILDERS`).
    pub fn builder(&self) -> &'static str {
        match self {
            Notice::Message { .. } => "notify_message",
            Notice::Approval { .. } => "notify_approval_values",
            Notice::Reply { .. } => "notify_reply",
            Notice::Room { .. } => "notify_room",
            Notice::Subtask { .. } => "notify_subtask",
            Notice::Scan { .. } => "notify_scan",
            Notice::Job { .. } => "notify_job",
            Notice::Question { .. } => "notify_question",
            Notice::Family { .. } => "notify_family",
            Notice::EmailHeld { .. } => "notify_email_held",
            Notice::Site { .. } => "notify_site_request",
            Notice::Login { .. } => "notify_login_request",
            Notice::Card { .. } => "notify_bridge_card",
            Notice::Meet { .. } => "notify_meet",
            Notice::Silent { .. } => "notify_silent",
        }
    }

    /// Silenced by the agent's « Me prévenir quand il a fini ou qu'il t'attend » switch (agent_notify.rs) when off
    /// (the rules table, notifications.rs; a Boîte message's own rule depends on whose it is, decided by `emit`).
    pub fn follows_switch(&self) -> bool {
        self.event("", &Context { title: String::new(), lang: "fr".into() })
            .is_some_and(|e| e.kind.follows_switch(e.from))
    }

    /// The event `PushService::emit` journals and pushes (None for a silent push, which is neither journaled nor
    /// pointed).
    pub fn event(&self, agent: &str, context: &Context) -> Option<Event> {
        let event = match self {
            Notice::Message { outbox_id, text, session_id } => {
                let mut event =
                    Event::new(agent, Kind::Boite, outbox_id).session(opt(session_id)).reference(outbox_id);
                event.shown = Some(text.clone());
                event.message = Some(format!("o_{outbox_id}"));
                event
            }
            Notice::Approval { run_id, request_id, choices, command, session_id, room_id } => {
                let request = opt(request_id).unwrap_or(run_id).to_string();
                let choices = choices.as_ref().filter(|c| !c.is_empty()).map(|c| Value::Array(c.clone()));
                let mut event = Event::new(agent, Kind::Approval, &request)
                    .session(opt(session_id))
                    .reference(&request)
                    .collapse(&request)
                    .with("run_id", json!(run_id))
                    .with("request_id", json!(opt(request_id)))
                    .with("choices", choices.unwrap_or(Value::Null));
                event.text = opt(command).map(|c| approval_line(&context.lang, c));
                event.room = opt(room_id).map(str::to_string);
                event
            }
            Notice::Reply { run_id, text, session_id } => {
                let mut event = Event::new(agent, Kind::Reply, run_id)
                    .session(opt(session_id))
                    .reference(run_id)
                    .text(Some(text));
                event.wait = true;
                // a run's own replies replace each other (one run, one notification); an autonomous turn's
                // (« session:… ») never replaces an earlier one
                if !run_id.starts_with("session:") {
                    event.collapse = Some(run_id.clone());
                }
                event
            }
            Notice::Room { room_id, room_title, text } => {
                let key = format!("{room_id}:{:x}", fnv(text));
                let mut event = Event::new(agent, Kind::RoomReply, key)
                    .reference(room_id)
                    .text(Some(text))
                    .collapse(format!("room:{room_id}"));
                if !room_title.is_empty() {
                    event.title = Some(format!("{} · {}", context.title, room_title));
                }
                event.room = Some(room_id.clone());
                event
            }
            Notice::Subtask { session_id, line } => {
                let line = opt(line);
                let key = format!("{session_id}:{:x}", fnv(line.unwrap_or("")));
                Event::new(agent, Kind::Subtask, key).session(Some(session_id)).text(line)
            }
            Notice::Scan { line } => {
                let key = format!("{:x}:{}", fnv(line), crate::store::iso(crate::store::utcnow()));
                Event::new(agent, Kind::Scan, key).text(Some(line)).collapse(format!("scan:{agent}"))
            }
            Notice::Job { session_id, line, job_id, body } => {
                let mut event = Event::new(agent, Kind::Task, format!("{job_id}:{body}"))
                    .session(opt(session_id))
                    .reference(job_id)
                    .text(Some(line))
                    .collapse(format!("job:{job_id}"));
                event.shown = Some(body.clone());
                event
            }
            Notice::Question { run_id, request_id, question, session_id } => {
                let request = opt(request_id).unwrap_or(run_id).to_string();
                Event::new(agent, Kind::Question, &request)
                    .session(opt(session_id))
                    .reference(&request)
                    .text(opt(question))
                    .collapse(&request)
                    .with("run_id", json!(run_id))
                    .with("request_id", json!(opt(request_id)))
            }
            Notice::Family { line, note_id } => {
                Event::new(agent, Kind::Family, note_id).reference(note_id).text(Some(line))
            }
            Notice::EmailHeld { to, held_id } => {
                card(agent, Kind::EmailHeld, &email_held_line(&context.lang, to), held_id)
            }
            Notice::Site { line, id } => card(agent, Kind::Site, line, id),
            Notice::Login { host, id } => card(agent, Kind::Login, &login_line(&context.lang, host), id),
            Notice::Card { line, key, id } => card(agent, Kind::of_card(key), line, id),
            Notice::Meet { line, id } => card(agent, Kind::Meet, line, id),
            Notice::Silent { .. } => return None,
        };
        Some(event)
    }
}

/// An accord card (always notified), its id in `r`.
fn card(agent: &str, kind: Kind, line: &str, id: &str) -> Event {
    Event::new(agent, kind, id).reference(id).text(Some(line)).collapse(format!("{}:{id}", kind.as_str()))
}

fn opt(value: &Option<String>) -> Option<&str> {
    given(value.as_deref())
}

impl Event {
    /// The conversation and the message it points at: a room's own, else the thread `emit` resolved for its session
    /// (the main thread for a Boîte message), and the message it names or the stored reply it waited for.
    pub fn target(&self, pointer: &Pointer) -> (Option<String>, Option<String>) {
        match &self.room {
            Some(room) => (Some(format!("room:{room}")), self.message.clone()),
            None => {
                let message = self.message.clone().or_else(|| pointer.message.clone().filter(|_| self.wait));
                (pointer.thread.clone(), message)
            }
        }
    }

    /// The title the push shows: the agent's name, or a group discussion's « <agent> · <room> ».
    pub fn shown_title(&self, context: &Context) -> String {
        self.title.clone().unwrap_or_else(|| context.title.clone())
    }

    /// What a phone with a key gets sealed (seal.rs): the title the push shows, the whole words (they never travel
    /// in clear).
    pub fn sealed(&self, context: &Context) -> Sealed {
        let words = self.text.as_deref().or(self.shown.as_deref()).unwrap_or("");
        Sealed::new(self.kind.as_str(), &self.shown_title(context), words)
    }

    /// Its push (`emit` sends exactly this to a phone without a key; `sealed` added for one with a key): the neutral
    /// line, the pointer, the buttons' fields.
    pub fn push(&self, pointer: &Pointer) -> Built {
        let (thread, message) = self.target(pointer);
        let payload = payload(
            self.kind,
            &self.agent,
            &pointer.n,
            thread.as_deref(),
            message.as_deref(),
            self.ref_id.as_deref(),
            pointer.badge,
            &self.extra,
        );
        Built { payload, push_type: "alert", priority: 10, collapse_id: self.collapse.clone() }
    }
}

/// What `notice` says, sealed for each phone with a key; None for a silent push.
pub fn sealed(agent: &str, context: &Context, notice: &Notice) -> Option<Sealed> {
    notice.event(agent, context).map(|e| e.sealed(context))
}

/// The push for `notice` from `agent` (its bridge key) once pointed (`pointer`): payload, type, priority and collapse
/// id, as `emit` sends them.
pub fn build(agent: &str, context: &Context, notice: &Notice, pointer: &Pointer) -> Built {
    match notice.event(agent, context) {
        Some(event) => event.push(pointer),
        None => {
            let Notice::Silent { reason, extra } = notice else { unreachable!("only a silent notice has no event") };
            Built { payload: silent_payload(agent, reason, extra), push_type: "background", priority: 5, collapse_id: None }
        }
    }
}
