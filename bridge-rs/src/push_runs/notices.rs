//! Every notification the bridge sends, as data: what one `PushService::notify_*` is given (`Notice`) and the pure
//! function that turns it into the push (`build`). `PushService` adds what needs the bridge's state (the agent's
//! shown name, the previews setting, its language, the « Me prévenir » switch, the kept reply texts) and sends it.
//!
//! The contract: `contracts/notifications/types/<type>.json` at the repository's root holds, for each type, inputs
//! of `build` and the exact payload it must give (tests/notifications_contract.rs), read by the apps' tests and the
//! relay's too. A new `Notice` needs its contract file: the test fails without it.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    approval_line, approval_payload, email_held_line, given, login_line, message_payload, preview, question_payload, reply_payload, silent_payload, CARD_BODY, EMAIL_HELD_BODY, FAMILY_BODY, LOGIN_BODY, MEET_BODY,
    REPLY_BODY, SITE_BODY, SUBTASK_BODY,
};
use crate::Dict;

/// What the bridge knows of the agent when it builds a push.
#[derive(Clone, Debug, Deserialize)]
pub struct Context {
    /// The agent's shown name (« Vie »), the notification's title.
    pub title: String,
    /// `[push] previews`: the words travel in the alert (else a generic body the phones translate).
    pub previews: bool,
    /// The agent's language (« fr », « en », « de », « es »), for the lines written here.
    pub lang: String,
}

/// One notification, as the `notify_*` producing it is called (the reply text ids already kept by `remember`).
/// Its serde names are the `notify_*` method names: the contract files say `{"fn": "notify_reply", "args": {…}}`.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "fn", content = "args", deny_unknown_fields)]
pub enum Notice {
    /// A Boîte message (outbox): routine report, watcher, hand-back outside Bot Chat, credit, storage, imports.
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
    /// A run's reply nobody saw (also Bot Chat's autonomous turns and hand-backs: `run_id` « session:<sid> »).
    #[serde(rename = "notify_reply")]
    Reply { run_id: String, text: String, session_id: Option<String>, reply_id: Option<String> },
    /// An agent's reply in a group discussion.
    #[serde(rename = "notify_room")]
    Room { room_id: String, room_title: String, text: String, reply_id: Option<String> },
    /// A sub-task's report Hermes left in the conversation.
    #[serde(rename = "notify_subtask")]
    Subtask { session_id: String, line: Option<String>, reply_id: Option<String> },
    /// A step of a mailbox scan.
    #[serde(rename = "notify_scan")]
    Scan { line: String },
    /// A background task ended, paused, was proposed or rescued (`body`: its generic line).
    #[serde(rename = "notify_job")]
    Job { session_id: Option<String>, line: String, job_id: String, body: String },
    /// The agent asks something mid-run (clarify).
    #[serde(rename = "notify_question")]
    Question {
        run_id: String,
        request_id: Option<String>,
        question: Option<String>,
        session_id: Option<String>,
        reply_id: Option<String>,
    },
    /// Someone of the family passed something on to this agent.
    #[serde(rename = "notify_family")]
    Family { line: String, note_id: String, reply_id: Option<String> },
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
    /// A silent push (`run_finished`, `reminders`, `calendar`, `contacts`).
    #[serde(rename = "notify_silent")]
    Silent { reason: String, #[serde(default)] extra: Dict },
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

    /// Writes a line in the agent's language (read from its files: only then).
    pub fn uses_lang(&self, previews: bool) -> bool {
        match self {
            Notice::Approval { command, .. } => previews && opt(command).is_some(),
            Notice::EmailHeld { .. } | Notice::Login { .. } => true,
            _ => false,
        }
    }

    /// Silenced by the agent's « Me prévenir quand il a fini ou qu'il t'attend » switch (agent_notify.rs) when off.
    /// Approvals, accord cards, family notes, Boîte messages and silent pushes are not.
    pub fn follows_switch(&self) -> bool {
        matches!(
            self,
            Notice::Reply { .. }
                | Notice::Room { .. }
                | Notice::Subtask { .. }
                | Notice::Scan { .. }
                | Notice::Job { .. }
                | Notice::Question { .. }
        )
    }
}

fn opt(value: &Option<String>) -> Option<&str> {
    given(value.as_deref())
}

fn shown<'a>(context: &Context, line: &'a str) -> Option<&'a str> {
    if context.previews {
        Some(line)
    } else {
        None
    }
}

fn alert_push(payload: Dict, collapse_id: Option<String>) -> Built {
    Built { payload, push_type: "alert", priority: 10, collapse_id }
}

/// The push for `notice` from `agent` (its bridge key): payload, type, priority and collapse id, as `notify_*`
/// sends them.
pub fn build(agent: &str, context: &Context, notice: &Notice) -> Built {
    let title = context.title.as_str();
    let previews = context.previews;
    match notice {
        Notice::Message { outbox_id, text, session_id } => {
            let short = previews.then(|| preview(text, 160));
            alert_push(message_payload(agent, title, outbox_id, opt(session_id), short.as_deref()), None)
        }
        Notice::Approval { run_id, request_id, choices, command, session_id, room_id } => {
            let mut line = None;
            if previews {
                if let Some(command) = opt(command) {
                    line = Some(approval_line(&context.lang, command));
                }
            }
            let mut payload =
                approval_payload(agent, title, run_id, opt(request_id), choices.as_deref(), line.as_deref(), opt(session_id));
            if let Some(room) = opt(room_id) {
                payload.insert("room_id".into(), json!(room)); // tapping opens the room, where its card is
            }
            let collapse = opt(request_id).unwrap_or(run_id).to_string();
            alert_push(payload, Some(collapse))
        }
        Notice::Reply { run_id, text, session_id, reply_id } => {
            let short = previews.then(|| preview(text, 160));
            let payload =
                reply_payload(agent, title, run_id, opt(session_id), short.as_deref(), opt(reply_id), REPLY_BODY);
            alert_push(payload, Some(run_id.clone()))
        }
        Notice::Room { room_id, room_title, text, reply_id } => {
            let short = previews.then(|| preview(text, 160));
            let title = if room_title.is_empty() { title.to_string() } else { format!("{title} · {room_title}") };
            let run_id = format!("room:{room_id}");
            let mut payload = reply_payload(agent, &title, &run_id, None, short.as_deref(), opt(reply_id), REPLY_BODY);
            payload.insert("room_id".into(), json!(room_id));
            if let Some(Value::Object(aps)) = payload.get_mut("aps") {
                aps.insert("thread-id".into(), json!(run_id));
            }
            alert_push(payload, Some(run_id))
        }
        Notice::Subtask { session_id, line, reply_id } => {
            let line = opt(line);
            let short = if previews { line } else { None };
            let payload = reply_payload(
                agent,
                title,
                &format!("session:{session_id}"),
                Some(session_id),
                short,
                opt(reply_id),
                SUBTASK_BODY,
            );
            alert_push(payload, Some(format!("subtask:{session_id}")))
        }
        Notice::Scan { line } => {
            let payload = reply_payload(agent, title, "session:scan", None, shown(context, line), None, SUBTASK_BODY);
            alert_push(payload, Some(format!("scan:{agent}")))
        }
        Notice::Job { session_id, line, job_id, body } => {
            let run_id = format!("session:{}", opt(session_id).unwrap_or("job"));
            let mut payload =
                reply_payload(agent, title, &run_id, session_id.as_deref(), shown(context, line), None, body);
            payload.insert("job_id".into(), json!(job_id));
            alert_push(payload, Some(format!("job:{job_id}")))
        }
        Notice::Question { run_id, request_id, question, session_id, reply_id } => {
            let short = if previews { opt(question).map(|q| preview(q, 160)) } else { None };
            let payload = question_payload(
                agent,
                title,
                run_id,
                opt(request_id),
                opt(session_id),
                short.as_deref(),
                opt(reply_id),
            );
            let collapse = opt(request_id).unwrap_or(run_id).to_string();
            alert_push(payload, Some(collapse))
        }
        Notice::Family { line, note_id, reply_id } => {
            let short = preview(line, 160);
            let mut payload =
                reply_payload(agent, title, "session:family", None, shown(context, &short), opt(reply_id), FAMILY_BODY);
            payload.insert("family".into(), json!(note_id));
            alert_push(payload, Some(format!("family:{note_id}")))
        }
        Notice::EmailHeld { to, held_id } => {
            let line = email_held_line(&context.lang, to);
            let mut payload =
                reply_payload(agent, title, "session:email", None, shown(context, &line), None, EMAIL_HELD_BODY);
            payload.insert("email_held".into(), json!(held_id));
            alert_push(payload, Some(format!("email:{held_id}")))
        }
        Notice::Site { line, id } => {
            let mut payload = reply_payload(agent, title, "session:sites", None, shown(context, line), None, SITE_BODY);
            payload.insert("site_request".into(), json!(id));
            alert_push(payload, Some(format!("site:{id}")))
        }
        Notice::Login { host, id } => {
            let line = login_line(&context.lang, host);
            let mut payload = reply_payload(agent, title, "session:vault", None, shown(context, &line), None, LOGIN_BODY);
            payload.insert("login_request".into(), json!(id));
            alert_push(payload, Some(format!("login:{id}")))
        }
        Notice::Card { line, key, id } => {
            let mut payload = reply_payload(agent, title, "session:vault", None, shown(context, line), None, CARD_BODY);
            payload.insert(key.clone(), json!(id));
            alert_push(payload, Some(format!("{key}:{id}")))
        }
        Notice::Meet { line, id } => {
            let mut payload = reply_payload(agent, title, "session:meet", None, shown(context, line), None, MEET_BODY);
            payload.insert("meet".into(), json!(id));
            alert_push(payload, Some(format!("meet:{id}")))
        }
        Notice::Silent { reason, extra } => Built {
            payload: silent_payload(agent, reason, extra),
            push_type: "background",
            priority: 5,
            collapse_id: None,
        },
    }
}
