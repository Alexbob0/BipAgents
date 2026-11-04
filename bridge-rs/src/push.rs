//! Push notifications to the agent's registered devices.
//!
//! What the agent said never travels in clear: a push carries the pointer (docs/conversations.md §9.3) and a neutral
//! line (`notif.<type>`, the phones translate it); a phone that gave its key (`devices.push_key`) gets the words sealed
//! for it in `enc` (seal.rs), opened by its Notification Service Extension. A phone without a key gets the neutral
//! line only.

use std::sync::Arc;

use serde_json::{json, Map, Value};

use crate::apns::{ApnsClient, ApnsResult, SendOptions};
use crate::config::SharedConfig;
use crate::fcm::FcmClient;
use crate::logs::{redact6, J};
use crate::notifications::{BoiteFrom, Kind};
use crate::relay::RelayClient;
use crate::store::{Row, Store};
use crate::Dict;

/// Python-compatible text helpers shared by the push-runs modules (whitespace, slicing, exception names).
#[path = "push_runs/pyx.rs"]
pub mod pyx;

/// Every notification as data and its pure builder (the contract's function, contracts/notifications/).
#[path = "push_runs/notices.rs"]
pub mod notices;
use notices::{Context, Notice, Pointer};

/// Sealed notifications: the words for each device's own key, a neutral line in clear (refonte phase 5).
#[path = "push_runs/seal.rs"]
pub mod seal;

/// The lines a sealed push shows, written here: in the language of the agent it comes from (« fr », « en », « de »,
/// « es »; Alex, 2026-11-01: « Veut écrire à … » reached an English person).
pub fn approval_line(lang: &str, command: &str) -> String {
    use crate::approval_text::{colon, pick};
    let said = pick(lang, "Approbation requise", "Approval needed", "Freigabe nötig", "Se necesita tu permiso");
    format!("{said}{}{}", colon(lang), preview(command, 120))
}

/// « Veut écrire à x@y » (an e-mail held for a new address, agent_email.rs).
pub fn email_held_line(lang: &str, to: &str) -> String {
    match lang {
        "fr" => format!("Veut écrire à {to}"),
        "de" => format!("Will {to} schreiben"),
        "es" => format!("Quiere escribir a {to}"),
        _ => format!("Wants to write to {to}"),
    }
}

/// « Veut créer un compte sur x avec un mot de passe du coffre » (vault_logins.rs).
pub fn login_line(lang: &str, host: &str) -> String {
    let n = crate::names::names(lang);
    match lang {
        "fr" => format!("Veut créer un compte sur {host} avec un mot de passe du coffre"),
        "de" => format!("Will ein Konto auf {host} mit einem Passwort aus dem {} erstellen", n.vault),
        "es" => format!("Quiere crear una cuenta en {host} con una contraseña de la {}", n.vault.to_lowercase()),
        _ => format!("Wants to create an account on {host} with a password from the {}", n.vault.to_lowercase()),
    }
}

/// Python's truthiness of an optional str (`if x:`).
fn given(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// `_preview(text, limit=160)`: whitespace squashed, cut with « … ».
pub fn preview(text: &str, limit: usize) -> String {
    let text = pyx::squash(text);
    if pyx::len(&text) <= limit {
        return text;
    }
    format!("{}…", pyx::rstrip(pyx::head(&text, limit.saturating_sub(1))))
}

fn object(value: Value) -> Dict {
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The APNs category of a notification: approvals have « Autoriser une fois / Refuser », accord cards that may be
/// answered from the notification « Ouvrir » (never a secret's: it is typed in the app), the others « Répondre ».
pub fn category(kind: Kind) -> &'static str {
    match kind {
        Kind::Approval => "APPROVAL",
        Kind::Connector | Kind::MailCard => "CARD",
        _ => "MESSAGE",
    }
}

/// A notification's push (docs/conversations.md §9.3): the neutral line (`notif.<type>`: no title, no words; a device
/// with a key gets them sealed, `PushService::fanout`), `thread-id` per conversation (`<agent>/<c>`, the
/// agent's when it is about none), the badge (the journal's unread total), then the pointer: `v`, `n`, `t`, `agent`,
/// `c`, `m`, `r` (the run / request / card / job it is about) and an approval's or question's own fields (`run_id`,
/// `request_id`, `choices`: what its buttons answer).
#[allow(clippy::too_many_arguments)]
pub fn payload(
    kind: Kind,
    agent: &str,
    n: &str,
    thread: Option<&str>,
    message: Option<&str>,
    reference: Option<&str>,
    badge: i64,
    extra: &Dict,
) -> Dict {
    let thread_id = thread.map(|c| format!("{agent}/{c}")).unwrap_or_else(|| agent.to_string());
    let mut out = object(json!({
        "aps": {"alert": {"loc-key": kind.body_key(), "body": kind.body_key()}, "thread-id": thread_id, "mutable-content": 1,
                "category": category(kind), "sound": "default", "badge": badge},
        "v": 2, "n": n, "t": kind.as_str(), "agent": agent,
    }));
    for (key, value) in [("c", thread), ("m", message), ("r", reference)] {
        if let Some(v) = given(value) {
            out.insert(key.into(), json!(v));
        }
    }
    for (k, v) in extra {
        if !v.is_null() {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

/// `silent_payload(agent, reason, **extra)`: extras that are None are left out.
pub fn silent_payload(agent: &str, reason: &str, extra: &Dict) -> Dict {
    let mut payload = object(json!({"aps": {"content-available": 1}, "agent": agent, "reason": reason}));
    for (k, v) in extra {
        if !v.is_null() {
            payload.insert(k.clone(), v.clone());
        }
    }
    payload
}

/// Who sends a push: Apple's APNs (this bridge's .p8 key), Google's FCM (the Firebase service account), or a relay
/// (another bridge's, the publisher's). Python passes any of them where it expects « a client with `send` ».
#[derive(Clone)]
pub enum Sender {
    Apns(Arc<ApnsClient>),
    Fcm(Arc<FcmClient>),
    Relay(Arc<RelayClient>),
}

impl Sender {
    pub async fn send(&self, device_token: &str, payload: &Value, options: &SendOptions) -> ApnsResult {
        match self {
            Sender::Apns(c) => c.send(device_token, payload, options).await,
            Sender::Fcm(c) => c.send(device_token, payload, options).await,
            Sender::Relay(c) => c.send(device_token, payload, options).await,
        }
    }

    /// `send`, with what Python lets through as an exception (an unreadable key, an answer that is not JSON…).
    pub async fn try_send(
        &self,
        device_token: &str,
        payload: &Value,
        options: &SendOptions,
    ) -> anyhow::Result<ApnsResult> {
        match self {
            Sender::Apns(c) => c.try_send(device_token, payload, options).await,
            Sender::Fcm(c) => c.try_send(device_token, payload, options).await,
            Sender::Relay(c) => c.try_send(device_token, payload, options).await,
        }
    }
}

pub struct PushService {
    pub config: SharedConfig,
    pub store: Store,
    /// iPhones: APNs, or the relay of an install without an Apple key.
    pub apns: Option<Sender>,
    /// FcmClient, or the relay (RelayClient) for Android devices.
    pub android: Option<Sender>,
    /// The publisher's relay (RelayClient), for devices registered there (« relay »).
    pub service: Option<Arc<RelayClient>>,
    /// The Live Activities (and Android's progress notifications) following an agent's work (live_activities.rs).
    pub live: crate::live_activities::Registry,
    /// The conversations (threads.rs): where a notification points.
    pub threads: crate::threads::Link,
}

/// One notification to journal and push ([`PushService::emit`]).
#[derive(Clone, Debug)]
pub struct Event {
    pub agent: String,
    pub kind: Kind,
    /// The Hermes session it is about (resolved to its thread), or the room.
    pub session: Option<String>,
    pub room: Option<String>,
    /// The run / request / card / job / mail / meet id it is about.
    pub ref_id: Option<String>,
    /// Its text: kept in the journal (`GET /v1/notifications/{n}`), sealed for each phone with a key.
    pub text: Option<String>,
    /// What a sealed push shows when it isn't `text` (a Boîte message's words, a task's generic line).
    pub shown: Option<String>,
    /// Another title than the agent's name (a room's « <agent> · <room> »).
    pub title: Option<String>,
    /// The message id when the producer knows it (a Boîte message: `o_<id>`).
    pub message: Option<String>,
    /// Wait for the reply's message to be stored and point at it.
    pub wait: bool,
    /// The dedupe key without the message (`<kind>:<agent>:` is put first; a reply's is its message once found).
    pub key: String,
    pub from: Option<BoiteFrom>,
    /// What an approval's or a question's buttons need (`run_id`, `request_id`, `choices`).
    pub extra: Dict,
    /// The collapse id of a notification that replaces an earlier one (an approval's request); None for the others.
    pub collapse: Option<String>,
}

impl Event {
    pub fn new(agent: &str, kind: Kind, key: impl Into<String>) -> Event {
        Event {
            agent: agent.to_string(),
            kind,
            session: None,
            room: None,
            ref_id: None,
            text: None,
            shown: None,
            title: None,
            message: None,
            wait: false,
            key: key.into(),
            from: None,
            extra: Dict::new(),
            collapse: None,
        }
    }

    fn session(mut self, session: Option<&str>) -> Self {
        self.session = given(session).map(str::to_string);
        self
    }

    fn reference(mut self, id: &str) -> Self {
        self.ref_id = Some(id.to_string()).filter(|i| !i.is_empty());
        self
    }

    /// The text kept and sealed.
    fn text(mut self, text: Option<&str>) -> Self {
        self.text = given(text).map(str::to_string);
        self
    }

    fn collapse(mut self, id: impl Into<String>) -> Self {
        self.collapse = Some(id.into());
        self
    }

    fn with(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }
}

impl PushService {
    pub fn new(
        config: SharedConfig,
        store: Store,
        apns: Option<Sender>,
        android: Option<Sender>,
        service: Option<Arc<RelayClient>>,
    ) -> Self {
        if apns.is_none() {
            tracing::warn!("APNs not configured: iPhone push notifications disabled");
        }
        PushService { config, store, apns, android, service, live: Default::default(), threads: Default::default() }
    }

    fn title(&self, agent: &str) -> String {
        let config = self.config.get();
        match config.agent(Some(agent)) {
            Some(cfg) => cfg.display_name.clone(),
            // an agent a relative shares with this person: « Cuisine (Alex) », not its id (agent_shares.rs)
            None => crate::agent_shares::shown_name(&config, agent).unwrap_or_else(|| agent.to_string()),
        }
    }

    /// What `notices` needs of the agent: its shown name and its language.
    fn context(&self, agent: &str) -> Context {
        Context { title: self.title(agent), lang: self.lang(agent).to_string() }
    }

    /// Journals and pushes `notice` (`emit`); a silent one is only pushed.
    async fn notify(&self, agent: &str, notice: Notice) -> usize {
        match notice.event(agent, &self.context(agent)) {
            Some(event) => self.emit(event).await,
            None => {
                let built = notices::build(agent, &self.context(agent), &notice, &Pointer::default());
                self.fanout(agent, built.payload, None, built.push_type, built.priority, None).await
            }
        }
    }

    /// The language `agent` writes in (« fr », « en », « de », « es »), for the lines written here.
    fn lang(&self, agent: &str) -> &'static str {
        crate::names::names(&crate::names::agent_language(&self.config.get(), agent)).lang
    }

    /// The one entry of every notification (docs/conversations.md §9.1): the conversation and the message it is
    /// about (a reply waits until stored), one journal row per event (a second producer of the same event is
    /// dropped), the rules (« Me prévenir »: notifications.rs), then the push of the pointer. Returns the devices
    /// reached.
    pub async fn emit(&self, event: Event) -> usize {
        let config = self.config.get();
        let threads = crate::threads::upgrade(&self.threads);
        let agent = config.agent(Some(&event.agent)).cloned();
        // the thread of its session (the main thread for a Boîte message) and the stored reply it waits for
        let (mut resolved, mut found) = (None, None);
        if let (None, Some(threads), Some(agent)) = (&event.room, &threads, &agent) {
            if let (true, Some(session)) = (event.wait, &event.session) {
                let wait = config.push_message_wait_seconds;
                if let Some((t, m)) = threads.find_message(agent, session, event.text.as_deref(), wait).await {
                    resolved = Some(t);
                    found = Some(m);
                }
            }
            if resolved.is_none() {
                resolved = match &event.session {
                    Some(session) => threads.resolve(agent, session).await.ok().flatten().map(|t| t.id),
                    None if event.kind == Kind::Boite => threads.main(agent).await.ok().map(|t| t.id),
                    None => None,
                };
            }
        }
        let mut pointer = Pointer { n: crate::threads::new_id("n_"), thread: resolved, message: found, badge: 0 };
        let (thread, message) = event.target(&pointer);
        let kind = event.kind.as_str();
        let key = match (&message, event.kind) {
            (Some(m), Kind::Reply) => format!("{kind}:{}:{m}", event.agent),
            _ => format!("{kind}:{}:{}", event.agent, event.key),
        };
        let id = pointer.n.clone();
        let context = self.context(&event.agent);
        let title = event.shown_title(&context);
        let row = crate::notifications::NewRow {
            id: &id,
            agent: &event.agent,
            kind,
            thread: thread.as_deref(),
            message: message.as_deref(),
            ref_id: event.ref_id.as_deref(),
            session: event.session.as_deref(),
            title: &title,
            text: event.text.as_deref(),
            state: "open",
            dedupe: &key,
        };
        match crate::notifications::insert(&self.store, &row).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::info!(agent = %event.agent, kind = kind, key = %key, "notification already sent");
                return 0;
            }
            Err(exc) => tracing::error!(exc = %format!("{exc:#}"), "notification not journaled"),
        }
        pointer.badge = crate::notifications::unread_total(&self.store).await.unwrap_or(0);
        let built = event.push(&pointer);
        let payload = built.payload;
        let quiet = event.kind.follows_switch(event.from) && !crate::agent_notify::notifies(&config, &event.agent);
        // the conversation is on an app's screen (its event stream open): seen there, read, not pushed
        let on_screen = matches!((&threads, &thread), (Some(t), Some(c)) if t.is_on_screen(c));
        let (sent, push) = if on_screen {
            if let Err(exc) = crate::notifications::mark_read(&self.store, &id).await {
                tracing::warn!(exc = %format!("{exc:#}"), "notification not marked read");
            }
            (0, "skipped:on_screen".to_string())
        } else if quiet {
            (0, "skipped:switch".to_string())
        } else {
            // collapsed only where one notification replaces another (a run's, an approval's, a card's)
            let sealed = event.sealed(&context);
            let sent = self
                .fanout(&event.agent, payload.clone(), Some(&sealed), built.push_type, built.priority, built.collapse_id)
                .await;
            (sent, if sent > 0 { format!("sent:{sent}") } else { "skipped:no_device".into() })
        };
        if let Err(exc) = crate::notifications::set_push(&self.store, &id, &push).await {
            tracing::error!(exc = %format!("{exc:#}"), "notification not journaled");
        }
        if let Some(threads) = &threads {
            threads.publish(&event.agent, "notification",
                json!({"n": id, "agent": event.agent, "t": kind, "c": thread, "m": message, "state": "open"}));
        }
        sent
    }

    pub(crate) fn sender(&self, device: &Row) -> Option<Sender> {
        match device.get("environment").and_then(Value::as_str) {
            Some("relay") => self.service.clone().map(Sender::Relay),
            Some("fcm") => self.android.clone(),
            _ => self.apns.clone(),
        }
    }

    /// Sends `payload` to `agent`'s devices; a device that gave a key (`push_key`) gets `sealed` for it (seal.rs). A
    /// device without a key, or whose sealed push can't be made, gets the neutral line alone.
    async fn fanout(
        &self,
        agent: &str,
        payload: Dict,
        sealed: Option<&seal::Sealed>,
        push_type: &str,
        priority: u8,
        collapse_id: Option<String>,
    ) -> usize {
        let devices = match self.store.devices_for_agent(agent).await {
            Ok(devices) => devices,
            Err(exc) => {
                tracing::error!(exc = %format!("{exc:#}"), agent = %agent, "push failed");
                return 0;
            }
        };
        let devices: Vec<(Row, Sender)> = devices.into_iter().filter_map(|d| self.sender(&d).map(|s| (d, s))).collect();
        if devices.is_empty() {
            return 0;
        }
        let payloads: Vec<Value> = devices
            .iter()
            .map(|(device, _)| Value::Object(Self::payload_for(device, &payload, sealed, agent)))
            .collect();
        let sends = devices.iter().zip(&payloads).map(|((device, sender), payload)| {
            let options = SendOptions {
                environment: device.get("environment").and_then(Value::as_str).unwrap_or("").to_string(),
                push_type: push_type.to_string(),
                priority,
                collapse_id: collapse_id.clone(),
                expiration: None,
            };
            let token = device.get("token").and_then(Value::as_str).unwrap_or("").to_string();
            async move { sender.try_send(&token, payload, &options).await }
        });
        let results = futures::future::join_all(sends).await;
        let mut sent = 0;
        for ((device, _), result) in devices.iter().zip(results) {
            let token = device.get("token").and_then(Value::as_str).unwrap_or("");
            match result {
                Err(exc) => {
                    tracing::warn!(error = pyx::error_name(&exc), "apns send crashed");
                }
                Ok(result) if result.ok() => sent += 1,
                Ok(result) if result.token_is_dead() => {
                    if let Err(exc) = self.store.delete_device(token).await {
                        tracing::error!(exc = %format!("{exc:#}"), "push failed");
                        continue;
                    }
                    tracing::info!(token = %redact6(token), reason = ?J(&result.reason), "device token removed");
                }
                Ok(_) => {}
            }
        }
        sent
    }

    /// The push one device gets: sealed for its key when it gave one, else `payload` (the neutral line) as built.
    fn payload_for(device: &Row, payload: &Dict, sealed: Option<&seal::Sealed>, agent: &str) -> Dict {
        let key = device.get("push_key").and_then(Value::as_str).and_then(seal::stored_key);
        let (Some(key), Some(sealed)) = (key, sealed) else { return payload.clone() };
        seal::for_device(payload, sealed, agent, &key).unwrap_or_else(|exc| {
            tracing::warn!(exc = %format!("{exc:#}"), agent = %agent, "push not sealed: neutral line only");
            payload.clone()
        })
    }

    /// A Boîte message (outbox.rs): `from` says whose (its rule, notifications.rs).
    pub async fn notify_message(
        &self,
        agent: &str,
        outbox_id: &str,
        text: &str,
        session_id: Option<&str>,
        from: BoiteFrom,
    ) -> usize {
        let notice = Notice::Message { outbox_id: outbox_id.into(), text: text.into(), session_id: own(session_id) };
        let Some(mut event) = notice.event(agent, &self.context(agent)) else { return 0 };
        event.from = Some(from);
        self.emit(event).await
    }

    pub async fn notify_approval(
        &self,
        agent: &str,
        run_id: &str,
        request_id: Option<&str>,
        choices: Option<&[String]>,
        command: Option<&str>,
        session_id: Option<&str>,
    ) -> usize {
        let choices: Option<Vec<Value>> = choices.map(|c| c.iter().map(|s| json!(s)).collect());
        self.notify_approval_values(agent, run_id, request_id, choices.as_deref(), command, session_id, None).await
    }

    /// An approval Hermes waits for (the choices as Hermes gave them); `room_id` when the run is a group
    /// discussion's turn (tapping opens the room).
    #[allow(clippy::too_many_arguments)]
    pub async fn notify_approval_values(
        &self,
        agent: &str,
        run_id: &str,
        request_id: Option<&str>,
        choices: Option<&[Value]>,
        command: Option<&str>,
        session_id: Option<&str>,
        room_id: Option<&str>,
    ) -> usize {
        let notice = Notice::Approval {
            run_id: run_id.into(),
            request_id: own(request_id),
            choices: choices.map(<[Value]>::to_vec),
            command: own(command),
            session_id: own(session_id),
            room_id: own(room_id),
        };
        self.notify(agent, notice).await
    }

    /// A run ended while nobody followed it; also a turn the agent took on its own in its main conversation
    /// (cronwatch.rs, `run_id` « session:<id> ») and a hand-back's reply (handback.rs): the same reply from two of
    /// them is one notification (its message's id).
    pub async fn notify_reply(&self, agent: &str, run_id: &str, text: &str, session_id: Option<&str>) -> usize {
        self.notify(agent, Notice::Reply { run_id: run_id.into(), text: text.into(), session_id: own(session_id) })
            .await
    }

    /// An agent answered in a group discussion (rooms.rs): « <agent> · <room> »; tapping opens the discussion on its
    /// message (`m`, the room item's id). Collapsed per discussion.
    pub async fn notify_room(&self, agent: &str, room_id: &str, room_title: &str, text: &str, message: &str) -> usize {
        let notice = Notice::Room {
            room_id: room_id.into(),
            room_title: room_title.into(),
            text: text.into(),
            message: message.into(),
        };
        self.notify(agent, notice).await
    }

    /// A background sub-task finished and Hermes left its report in the conversation without waking the agent:
    /// « 🎧 Podcast jeudi » when it made a file, else « Sous-tâche terminée ». Tapping opens the conversation.
    pub async fn notify_subtask(&self, agent: &str, session_id: &str, line: Option<&str>) -> usize {
        self.notify(agent, Notice::Subtask { session_id: session_id.into(), line: own(line) }).await
    }

    /// A step of a mailbox scan ended (mail_scan.rs); tapping opens the agent.
    pub async fn notify_scan(&self, agent: &str, line: &str) -> usize {
        self.notify(agent, Notice::Scan { line: line.into() }).await
    }

    /// A background task ended, paused at its budget, or was proposed by the agent (jobs.rs): `body` sealed when
    /// there is no line; tapping opens the task's conversation (the Boîte for a proposal). A task's latest state replaces
    /// the earlier one.
    pub async fn notify_job(
        &self,
        agent: &str,
        session_id: Option<&str>,
        line: &str,
        job_id: &str,
        body: &str,
    ) -> usize {
        let notice =
            Notice::Job { session_id: own(session_id), line: line.into(), job_id: job_id.into(), body: body.into() };
        self.notify(agent, notice).await
    }

    /// The agent asks something mid-run (clarify) while nobody follows it: always notified (an unanswered question
    /// holds the agent like an approval; adopted 2026-11-02).
    pub async fn notify_question(
        &self,
        agent: &str,
        run_id: &str,
        request_id: Option<&str>,
        question: Option<&str>,
        session_id: Option<&str>,
    ) -> usize {
        let notice = Notice::Question {
            run_id: run_id.into(),
            request_id: own(request_id),
            question: own(question),
            session_id: own(session_id),
        };
        self.notify(agent, notice).await
    }

    /// Someone of the family passed something on to this agent (family_news.rs); tapping opens the agent.
    pub async fn notify_family(&self, agent: &str, line: &str, note_id: &str) -> usize {
        self.notify(agent, Notice::Family { line: line.into(), note_id: note_id.into() }).await
    }

    /// The agent's e-mail to an address not allowed yet waits for the person (agent_email.rs): « Veut écrire à
    /// x@y »; tapping opens the agent's e-mail screen (`r`: the held mail's id).
    pub async fn notify_email_held(&self, agent: &str, to: &str, held_id: &str) -> usize {
        self.notify(agent, Notice::EmailHeld { to: to.into(), held_id: held_id.into() }).await
    }

    /// The agent wants to send data to a site (sandbox_sites.rs): tapping opens the Boîte, where the card is.
    pub async fn notify_site_request(&self, agent: &str, line: &str, id: &str) -> usize {
        self.notify(agent, Notice::Site { line: line.into(), id: id.into() }).await
    }

    /// An agent wants to create an account on a website with a password of the vault (vault_logins.rs).
    pub async fn notify_login_request(&self, agent: &str, host: &str, id: &str) -> usize {
        self.notify(agent, Notice::Login { host: host.into(), id: id.into() }).await
    }

    /// A secret card or a connector's / mail's approval card (secrets.rs, connectors.rs, agent_tools.rs). `key`:
    /// `secret_request`, `connector_request` or `mail_request`, with the card's request id.
    pub async fn notify_bridge_card(&self, agent: &str, line: &str, key: &str, id: &str) -> usize {
        self.notify(agent, Notice::Card { line: line.into(), key: key.into(), id: id.into() }).await
    }

    /// A date to find with a relative's household (meet.rs): tapping opens the Boîte, where the card is.
    pub async fn notify_meet(&self, agent: &str, line: &str, id: &str) -> usize {
        self.notify(agent, Notice::Meet { line: line.into(), id: id.into() }).await
    }

    /// `notify_silent(agent, reason, **extra)`.
    pub async fn notify_silent(&self, agent: &str, reason: &str, extra: Dict) -> usize {
        self.notify(agent, Notice::Silent { reason: reason.into(), extra }).await
    }
}

fn own(value: Option<&str>) -> Option<String> {
    value.map(str::to_string)
}

/// A short stable digest of a text (dedupe keys).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_and_silent_payloads() {
        let extra = Map::new();
        let p = payload(Kind::Reply, "vie", "n_1", Some("t_1"), Some("m_1"), Some("r1"), 3, &extra);
        assert_eq!(
            Value::Object(p),
            json!({"aps": {"alert": {"loc-key": "notif.reply", "body": "notif.reply"}, "thread-id": "vie/t_1",
                           "mutable-content": 1, "category": "MESSAGE", "sound": "default", "badge": 3},
                   "v": 2, "n": "n_1", "t": "reply", "agent": "vie", "c": "t_1", "m": "m_1", "r": "r1"})
        );
        let p = payload(Kind::Connector, "vie", "n_2", None, None, None, 0, &extra);
        assert_eq!((p["aps"]["category"].as_str(), p["aps"]["thread-id"].as_str()), (Some("CARD"), Some("vie")));
        assert_eq!(p["aps"]["alert"], json!({"loc-key": "notif.connector", "body": "notif.connector"}));
        assert_eq!(category(Kind::MailCard), "CARD");
        assert_eq!(category(Kind::Secret), "MESSAGE"); // a secret is typed in the app, never from the notification
        assert_eq!(category(Kind::Approval), "APPROVAL");
        let mut extra = Map::new();
        extra.insert("run_id".into(), json!("r"));
        extra.insert("gone".into(), Value::Null);
        assert_eq!(
            Value::Object(silent_payload("vie", "run_finished", &extra)),
            json!({"aps": {"content-available": 1}, "agent": "vie", "reason": "run_finished", "run_id": "r"})
        );
        assert_eq!(preview("  a \n b  ", 160), "a b");
        assert_eq!(preview(&"x".repeat(200), 10), format!("{}…", "x".repeat(9)));
        assert_eq!(preview("abcd efgh", 6), "abcd…");
    }

    #[test]
    fn the_lines_in_the_agents_language() {
        // French as it was
        assert_eq!(approval_line("fr", "rm -rf  x"), "Approbation requise : rm -rf x");
        assert_eq!(email_held_line("fr", "a@b.fr"), "Veut écrire à a@b.fr");
        assert_eq!(login_line("fr", "x.com"), "Veut créer un compte sur x.com avec un mot de passe du coffre");
        assert_eq!(approval_line("en", "rm x"), "Approval needed: rm x");
        assert_eq!(email_held_line("en", "a@b.fr"), "Wants to write to a@b.fr");
        assert_eq!(login_line("en", "x.com"), "Wants to create an account on x.com with a password from the vault");
        assert_eq!(login_line("de", "x.com"), "Will ein Konto auf x.com mit einem Passwort aus dem Tresor erstellen");
        assert_eq!(login_line("es", "x.com"), "Quiere crear una cuenta en x.com con una contraseña de la caja fuerte");
        for lang in ["en", "de", "es"] {
            for line in [approval_line(lang, "ls"), email_held_line(lang, "a@b.fr"), login_line(lang, "x.com")] {
                assert!(!line.contains("Veut") && !line.contains("requise") && !line.contains("coffre"), "{line}");
            }
        }
    }
}
