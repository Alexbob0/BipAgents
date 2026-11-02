//! Push notifications to the agent's registered devices.
//!
//! Content travels on the tailnet, not through Apple: by default alerts carry a generic body and the
//! Notification Service Extension (`mutable-content: 1`) fetches the full text from the bridge.
//! Set `[push] previews = true` to put a short preview in the alert body instead.
//!
//! Port of `bridge/bipbridge/push.py` (owner group: push-runs).

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

/// Every notification as data and its pure payload builder (contracts/notifications/, phase 0).
#[path = "push_runs/notices.rs"]
pub mod notices;

/// Sealed notifications: the words for each device's own key, a neutral line in clear (refonte phase 5).
#[path = "push_runs/seal.rs"]
pub mod seal;

pub const MESSAGE_BODY: &str = "Nouveau message";
pub const REPLY_BODY: &str = "Ta réponse est prête.";
pub const QUESTION_BODY: &str = "Une question pour toi";
pub const APPROVAL_BODY: &str = "Approbation requise";
pub const SUBTASK_BODY: &str = "Sous-tâche terminée";
pub const FAMILY_BODY: &str = "Message de ta famille";
pub const EMAIL_HELD_BODY: &str = "Veut écrire à quelqu'un de nouveau : ton accord est demandé";
/// A site the agent's programs want to reach (sandbox_sites.rs), without the site when previews are off.
pub const SITE_BODY: &str = "Veut envoyer des données à un site : ton accord est demandé";
/// An account to create with a password of the vault (vault_logins.rs), without the site when previews are off.
pub const LOGIN_BODY: &str = "Veut créer un compte sur un site : ton accord est demandé";
/// A secret or a connector to approve (secrets.rs, connectors.rs), when previews are off.
pub const CARD_BODY: &str = "Un connecteur attend ton accord";
pub const MEET_BODY: &str = "Une date à trouver ensemble";

/// The lines a push shows when previews are on, written here: in the language of the agent it comes from (« fr »,
/// « en », « de », « es »; Alex, 2026-11-01: « Veut écrire à … » reached an English person). The bodies above stay
/// French: they are keys the phones translate (iOS `loc-key`, Android BipMessagingService `pushBody`).
pub fn approval_line(lang: &str, command: &str) -> String {
    use crate::approval_text::{colon, pick};
    let said = pick(lang, APPROVAL_BODY, "Approval needed", "Freigabe nötig", "Se necesita tu permiso");
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

/// The alert: the preview when there is one, else a generic body that iOS translates (`loc-key` is looked
/// up in the app's string catalog, whose keys are these French texts; `body` is kept for older apps).
fn alert(title: &str, preview: Option<&str>, fallback: &str) -> Value {
    match given(preview) {
        Some(p) => json!({"title": title, "body": p}),
        None => json!({"title": title, "body": fallback, "loc-key": fallback}),
    }
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

pub fn message_payload(
    agent: &str,
    title: &str,
    outbox_id: &str,
    session_id: Option<&str>,
    preview: Option<&str>,
) -> Dict {
    let mut payload = object(json!({
        "aps": {
            "alert": alert(title, preview, MESSAGE_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "MESSAGE",
            "sound": "default",
        },
        "outbox_id": outbox_id,
        "agent": agent,
    }));
    if let Some(s) = given(session_id) {
        payload.insert("session_id".into(), json!(s));
    }
    payload
}

/// `approval_payload(...)`; `choices` as Hermes gave them (a JSON list).
pub fn approval_payload(
    agent: &str,
    title: &str,
    run_id: &str,
    request_id: Option<&str>,
    choices: Option<&[Value]>,
    preview: Option<&str>,
    session_id: Option<&str>,
) -> Dict {
    let mut payload = object(json!({
        "aps": {
            "alert": alert(title, preview, APPROVAL_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "APPROVAL",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
    }));
    if let Some(r) = given(request_id) {
        payload.insert("request_id".into(), json!(r));
    }
    if let Some(c) = choices.filter(|c| !c.is_empty()) {
        payload.insert("choices".into(), Value::Array(c.to_vec()));
    }
    if let Some(s) = given(session_id) {
        payload.insert("session_id".into(), json!(s)); // tapping opens that conversation, where the card is
    }
    payload
}

/// A run finished while nobody followed it: tapping opens its conversation.
pub fn reply_payload(
    agent: &str,
    title: &str,
    run_id: &str,
    session_id: Option<&str>,
    preview: Option<&str>,
    reply_id: Option<&str>,
    fallback: &str,
) -> Dict {
    let mut payload = object(json!({
        "aps": {
            "alert": alert(title, preview, fallback),
            "thread-id": agent,
            "mutable-content": 1, // the extension shows it as a message from the agent's Bip
            "category": "MESSAGE",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
        "kind": "reply",
    }));
    if let Some(s) = given(session_id) {
        payload.insert("session_id".into(), json!(s));
    }
    if let Some(r) = given(reply_id) {
        payload.insert("reply_id".into(), json!(r)); // the extension fetches the text on the device (GET /v1/replies/{id})
    }
    payload
}

/// The agent asks something mid-run (clarify) while nobody follows it: tapping opens its conversation.
pub fn question_payload(
    agent: &str,
    title: &str,
    run_id: &str,
    request_id: Option<&str>,
    session_id: Option<&str>,
    preview: Option<&str>,
    reply_id: Option<&str>,
) -> Dict {
    let mut payload = object(json!({
        "aps": {
            "alert": alert(title, preview, QUESTION_BODY),
            "thread-id": agent,
            "mutable-content": 1,
            "category": "MESSAGE",
            "sound": "default",
        },
        "agent": agent,
        "run_id": run_id,
        "kind": "question",
    }));
    if let Some(r) = given(request_id) {
        payload.insert("request_id".into(), json!(r));
    }
    if let Some(s) = given(session_id) {
        payload.insert("session_id".into(), json!(s));
    }
    if let Some(r) = given(reply_id) {
        payload.insert("reply_id".into(), json!(r));
    }
    payload
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
pub struct Event {
    pub agent: String,
    pub kind: Kind,
    /// The Hermes session it is about (resolved to its thread), or the room.
    pub session: Option<String>,
    pub room: Option<String>,
    /// The run / request / card / job / mail / meet id it is about.
    pub ref_id: Option<String>,
    /// Its text: kept in the journal (served by `GET /v1/replies/{id}` and `/v1/notifications/{id}`).
    pub text: Option<String>,
    /// The message id when the producer knows it (a Boîte message: `o_<id>`).
    pub message: Option<String>,
    /// Wait for the reply's message to be stored and point at it.
    pub wait: bool,
    /// The dedupe key without the message (`<kind>:<agent>:` is put first; a reply's is its message once found).
    pub key: String,
    pub from: Option<BoiteFrom>,
    /// The legacy payload (every field older apps read); `reply_id` is set to the journal id when `text` is.
    pub payload: Dict,
    /// The collapse id of a notification that replaces an earlier one (an approval's request); None for the others.
    pub collapse: Option<String>,
    /// What a sealed push shows when it isn't `text` (an approval's plain words, seal.rs).
    pub shown: Option<String>,
}

impl Event {
    pub fn new(agent: &str, kind: Kind, key: impl Into<String>, payload: Dict) -> Event {
        Event {
            agent: agent.to_string(),
            kind,
            session: None,
            room: None,
            ref_id: None,
            text: None,
            message: None,
            wait: false,
            key: key.into(),
            from: None,
            payload,
            collapse: None,
            shown: None,
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

    fn text(mut self, text: Option<&str>) -> Self {
        self.text = given(text).map(str::to_string);
        self
    }

    fn collapse(mut self, id: impl Into<String>) -> Self {
        self.collapse = Some(id.into());
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

    /// A notification's text (`GET /v1/replies/{id}`): the journal's row.
    pub async fn text(&self, reply_id: &str) -> Option<String> {
        crate::notifications::text_of(&self.store, reply_id).await.ok().flatten()
    }

    fn title(&self, agent: &str) -> String {
        let config = self.config.get();
        match config.agent(Some(agent)) {
            Some(cfg) => cfg.display_name.clone(),
            // an agent a relative shares with this person: « Cuisine (Alex) », not its id (agent_shares.rs)
            None => crate::agent_shares::shown_name(&config, agent).unwrap_or_else(|| agent.to_string()),
        }
    }

    fn previews(&self) -> bool {
        self.config.get().push_previews
    }

    /// The language `agent` writes in (« fr », « en », « de », « es »), for the lines written here.
    fn lang(&self, agent: &str) -> &'static str {
        crate::names::names(&crate::names::agent_language(&self.config.get(), agent)).lang
    }

    /// The one entry of every notification (docs/conversations.md §9.1): the conversation and the message it is
    /// about (a reply waits until stored), one journal row per event (a second producer of the same event is
    /// dropped), the rules (« Me prévenir »: notifications.rs), then the push of the pointer (`v`, `n`, `t`, `c`,
    /// `m`) beside every legacy field. Returns the devices reached.
    pub async fn emit(&self, event: Event) -> usize {
        let config = self.config.get();
        let threads = crate::threads::upgrade(&self.threads);
        let agent = config.agent(Some(&event.agent)).cloned();
        let mut thread = event.room.as_ref().map(|r| format!("room:{r}"));
        let mut message = event.message.clone();
        if let (None, Some(threads), Some(agent)) = (&thread, &threads, &agent) {
            if let (true, Some(session)) = (event.wait, &event.session) {
                let wait = config.push_message_wait_seconds;
                if let Some((t, m)) = threads.find_message(agent, session, event.text.as_deref(), wait).await {
                    thread = Some(t);
                    message = Some(m);
                }
            }
            if thread.is_none() {
                thread = match &event.session {
                    Some(session) => threads.resolve(agent, session).await.ok().flatten().map(|t| t.id),
                    None if event.kind == Kind::Boite => threads.main(agent).await.ok().map(|t| t.id),
                    None => None,
                };
            }
        }
        let kind = event.kind.as_str();
        let key = match (&message, event.kind) {
            (Some(m), Kind::Reply) => format!("{kind}:{}:{m}", event.agent),
            _ => format!("{kind}:{}:{}", event.agent, event.key),
        };
        let id = crate::threads::new_id("n_");
        let title = self.title(&event.agent);
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
        // what a device with a key gets sealed (seal.rs): the title the push shows (a group discussion's too), the words
        let shown_title = event.payload.get("aps").and_then(|a| a.get("alert")).and_then(|a| a.get("title"))
            .and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| title.clone());
        let words = event.text.as_deref().or(event.shown.as_deref()).unwrap_or("");
        let sealed = seal::Sealed::new(event.kind.as_str(), &shown_title, words);
        let mut payload = event.payload;
        if event.text.is_some() {
            payload.insert("reply_id".into(), json!(id)); // the extension fetches the text (GET /v1/replies/{id})
        }
        payload.insert("v".into(), json!(2));
        payload.insert("n".into(), json!(id));
        payload.insert("t".into(), json!(kind));
        if let Some(t) = &thread {
            payload.insert("c".into(), json!(t));
        }
        if let Some(m) = &message {
            payload.insert("m".into(), json!(m));
        }
        let quiet = event.kind.follows_switch(event.from) && !crate::agent_notify::notifies(&config, &event.agent);
        let (sent, push) = if quiet {
            (0, "skipped:switch".to_string())
        } else {
            // collapsed only where one notification replaces another (an approval's, a card's); a reply never
            // replaces an earlier one (each is its own journal row)
            let sent = self.fanout(&event.agent, payload.clone(), Some(&sealed), "alert", 10, event.collapse.clone()).await;
            (sent, if sent > 0 { format!("sent:{sent}") } else { "skipped:no_device".into() })
        };
        if let Err(exc) = crate::notifications::set_push(&self.store, &id, &push, &payload).await {
            tracing::error!(exc = %format!("{exc:#}"), "notification not journaled");
        }
        if let Some(threads) = &threads {
            threads.publish(&event.agent, "notification",
                json!({"id": id, "agent": event.agent, "t": kind, "c": thread, "m": message, "state": "open"}));
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

    /// Sends `payload` to `agent`'s devices; a device that gave a key (`push_key`) gets `sealed` for it instead of any
    /// words in clear (seal.rs). A device whose sealed push can't be made gets the neutral line alone, never the clear
    /// words.
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

    /// The push one device gets: sealed for its key when it gave one and the push has words, else `payload` as built.
    fn payload_for(device: &Row, payload: &Dict, sealed: Option<&seal::Sealed>, agent: &str) -> Dict {
        let key = device.get("push_key").and_then(Value::as_str).and_then(seal::stored_key);
        let (Some(key), Some(sealed)) = (key, sealed) else { return payload.clone() };
        match seal::for_device(payload, sealed, agent, &key) {
            Ok(out) => out,
            Err(exc) => {
                tracing::warn!(exc = %format!("{exc:#}"), agent = %agent, "push not sealed: neutral line only");
                let mut out = payload.clone();
                if let Some(Value::Object(aps)) = out.get_mut("aps") {
                    let code = seal::loc_key(sealed.kind);
                    aps.insert("alert".into(), json!({"loc-key": code, "body": code}));
                }
                out
            }
        }
    }

    /// A Boîte message (ntfy.rs): `from` says whose (its rule, notifications.rs).
    pub async fn notify_message(
        &self,
        agent: &str,
        outbox_id: &str,
        text: &str,
        session_id: Option<&str>,
        from: BoiteFrom,
    ) -> usize {
        let preview = if self.previews() { Some(preview(text, 160)) } else { None };
        let payload = message_payload(agent, &self.title(agent), outbox_id, session_id, preview.as_deref());
        let mut event = Event::new(agent, Kind::Boite, outbox_id, payload).session(session_id).reference(outbox_id);
        event.message = Some(format!("o_{outbox_id}"));
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

    /// `notify_approval` with the choices as Hermes gave them (RunHub's pending approvals keep them as JSON);
    /// `room_id` when the run is a group discussion's turn (tapping opens the room).
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
        let preview_text = given(command).filter(|_| self.previews()).map(|c| approval_line(self.lang(agent), c));
        let mut payload = approval_payload(
            agent,
            &self.title(agent),
            run_id,
            request_id,
            choices,
            preview_text.as_deref(),
            session_id,
        );
        if let Some(room) = given(room_id) {
            payload.insert("room_id".into(), json!(room)); // tapping opens the room, where its card is
        }
        let request = given(request_id).unwrap_or(run_id).to_string();
        let mut event =
            Event::new(agent, Kind::Approval, &request, payload).session(session_id).reference(&request).collapse(&request);
        event.room = given(room_id).map(str::to_string);
        event.shown = given(command).map(str::to_string); // the effect in plain words (runs.rs: `bip_summary`)
        self.emit(event).await
    }

    /// A run ended while nobody followed it; also a turn the agent took on its own in its main conversation
    /// (cronwatch.rs, `run_id` « session:<id> ») and a hand-back's reply (handback.rs): the same reply from two of
    /// them is one notification (its message's id).
    pub async fn notify_reply(&self, agent: &str, run_id: &str, text: &str, session_id: Option<&str>) -> usize {
        let preview = if self.previews() { Some(preview(text, 160)) } else { None };
        let payload = reply_payload(agent, &self.title(agent), run_id, session_id, preview.as_deref(), None, REPLY_BODY);
        let mut event = Event::new(agent, Kind::Reply, run_id, payload).session(session_id).reference(run_id).text(Some(text));
        event.wait = true;
        self.emit(event).await
    }

    /// An agent answered in a group discussion (rooms.rs) nobody looks at: « <agent> · <room> », its words when
    /// previews are on; tapping opens the discussion (`room_id`).
    pub async fn notify_room(&self, agent: &str, room_id: &str, room_title: &str, text: &str) -> usize {
        let preview = if self.previews() { Some(preview(text, 160)) } else { None };
        let title = if room_title.is_empty() {
            self.title(agent)
        } else {
            format!("{} · {}", self.title(agent), room_title)
        };
        let run_id = format!("room:{room_id}");
        let mut payload = reply_payload(agent, &title, &run_id, None, preview.as_deref(), None, REPLY_BODY);
        payload.insert("room_id".into(), json!(room_id));
        if let Some(Value::Object(aps)) = payload.get_mut("aps") {
            aps.insert("thread-id".into(), json!(run_id));
        }
        let key = format!("{room_id}:{:x}", fnv(text));
        let mut event = Event::new(agent, Kind::RoomReply, key, payload).reference(room_id).text(Some(text));
        event.room = Some(room_id.to_string());
        self.emit(event).await
    }

    /// A background sub-task finished and Hermes left its report in the conversation without waking the agent:
    /// « 🎧 Podcast jeudi » when it made a file, else « Sous-tâche terminée ». Tapping opens the conversation.
    pub async fn notify_subtask(&self, agent: &str, session_id: &str, line: Option<&str>) -> usize {
        let line = given(line);
        let preview = if self.previews() { line } else { None };
        let run_id = format!("session:{session_id}");
        let payload = reply_payload(agent, &self.title(agent), &run_id, Some(session_id), preview, None, SUBTASK_BODY);
        let key = format!("{session_id}:{:x}", fnv(line.unwrap_or("")));
        self.emit(Event::new(agent, Kind::Subtask, key, payload).session(Some(session_id)).text(line)).await
    }

    /// A step of a mailbox scan ended (mail_scan.py): its line when previews are on, else « Sous-tâche terminée »;
    /// tapping opens the agent.
    pub async fn notify_scan(&self, agent: &str, line: &str) -> usize {
        let preview = if self.previews() { Some(line) } else { None };
        let payload = reply_payload(agent, &self.title(agent), "session:scan", None, preview, None, SUBTASK_BODY);
        let key = format!("{:x}:{}", fnv(line), crate::store::iso(crate::store::utcnow()));
        self.emit(Event::new(agent, Kind::Scan, key, payload).text(Some(line)).collapse(format!("scan:{agent}"))).await
    }

    /// A background task ended, paused at its budget, or was proposed by the agent (jobs.py): its line when
    /// previews are on, else `body`; tapping opens the task's conversation (the agent for a proposal).
    pub async fn notify_job(
        &self,
        agent: &str,
        session_id: Option<&str>,
        line: &str,
        job_id: &str,
        body: &str,
    ) -> usize {
        let preview = if self.previews() { Some(line) } else { None };
        let run_id = format!("session:{}", given(session_id).unwrap_or("job"));
        let mut payload = reply_payload(agent, &self.title(agent), &run_id, session_id, preview, None, body);
        payload.insert("job_id".into(), json!(job_id));
        let key = format!("{job_id}:{body}");
        self.emit(Event::new(agent, Kind::Task, key, payload).session(session_id).reference(job_id).text(Some(line))).await
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
        let question = given(question);
        let preview = if self.previews() { question.map(|q| preview(q, 160)) } else { None };
        let payload =
            question_payload(agent, &self.title(agent), run_id, request_id, session_id, preview.as_deref(), None);
        let request = given(request_id).unwrap_or(run_id).to_string();
        let event = Event::new(agent, Kind::Question, &request, payload).session(session_id).reference(&request);
        self.emit(event.text(question).collapse(&request)).await
    }

    /// Someone of the family passed something on to this agent (family_news.rs): « Alex : je rentre tard ce soir »
    /// when previews are on, else « Message de ta famille »; tapping opens the agent.
    pub async fn notify_family(&self, agent: &str, line: &str, note_id: &str) -> usize {
        let short = preview(line, 160);
        let shown = if self.previews() { Some(short.as_str()) } else { None };
        let mut payload = reply_payload(agent, &self.title(agent), "session:family", None, shown, None, FAMILY_BODY);
        payload.insert("family".into(), json!(note_id));
        self.emit(Event::new(agent, Kind::Family, note_id, payload).reference(note_id).text(Some(line))).await
    }

    /// The agent's e-mail to an address not allowed yet waits for the person (agent_email.rs): « Veut écrire à
    /// x@y » when previews are on; tapping opens the agent's e-mail screen (`email_held`: the held mail's id).
    pub async fn notify_email_held(&self, agent: &str, to: &str, held_id: &str) -> usize {
        let line = email_held_line(self.lang(agent), to);
        self.card(agent, Kind::EmailHeld, "session:email", &line, EMAIL_HELD_BODY, "email_held", held_id).await
    }

    /// The agent wants to send data to a site (sandbox_sites.rs; `line`: « Veut envoyer des données à exemple.com
    /// (environ 2 Mo) »): tapping opens the app's Boîte, where the card is.
    pub async fn notify_site_request(&self, agent: &str, line: &str, id: &str) -> usize {
        self.card(agent, Kind::Site, "session:sites", line, SITE_BODY, "site_request", id).await
    }

    /// An agent wants to create an account on a website with a password of the vault (vault_logins.rs): tapping opens
    /// the app's Boîte, where the card is.
    pub async fn notify_login_request(&self, agent: &str, host: &str, id: &str) -> usize {
        let line = login_line(self.lang(agent), host);
        self.card(agent, Kind::Login, "session:vault", &line, LOGIN_BODY, "login_request", id).await
    }

    /// A secret card or a connector's approval card (secrets.rs, connectors.rs): tapping opens the app's Boîte, where
    /// the card is. `key`: `secret_request`, `connector_request` or `mail_request`, with the card's request id.
    pub async fn notify_bridge_card(&self, agent: &str, line: &str, key: &str, id: &str) -> usize {
        self.card(agent, Kind::of_card(key), "session:vault", line, CARD_BODY, key, id).await
    }

    /// A date to find with a relative's household (meet.rs): « Léa propose une date : Dîner » when previews are on;
    /// tapping opens the app's Boîte, where the card is (`meet`: the request's id).
    pub async fn notify_meet(&self, agent: &str, line: &str, id: &str) -> usize {
        self.card(agent, Kind::Meet, "session:meet", line, MEET_BODY, "meet", id).await
    }

    /// An accord card (always notified): its line when previews are on, the card's id under `key`.
    #[allow(clippy::too_many_arguments)]
    async fn card(&self, agent: &str, kind: Kind, run_id: &str, line: &str, body: &str, key: &str, id: &str) -> usize {
        let preview = if self.previews() { Some(line) } else { None };
        let mut payload = reply_payload(agent, &self.title(agent), run_id, None, preview, None, body);
        payload.insert(key.into(), json!(id));
        self.emit(Event::new(agent, kind, id, payload).reference(id).text(Some(line)).collapse(format!("{key}:{id}"))).await
    }

    /// `notify_silent(agent, reason, **extra)`.
    pub async fn notify_silent(&self, agent: &str, reason: &str, extra: Dict) -> usize {
        self.fanout(agent, silent_payload(agent, reason, &extra), None, "background", 5, None).await
    }
}

/// A short stable digest of a text (dedupe keys).
fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_and_silent_payloads() {
        assert_eq!(message_payload("vie", "Vie", "o1", None, Some("Texte"))["aps"]["alert"]["body"], "Texte");
        assert!(!message_payload("vie", "Vie", "o1", None, None).contains_key("session_id"));
        let choices = [json!("once"), json!("deny")];
        let ap = approval_payload("vie", "Vie", "run_1", Some("req_9"), Some(&choices), None, None);
        assert_eq!(ap["aps"]["category"], "APPROVAL");
        assert_eq!(ap["run_id"], "run_1");
        assert_eq!(ap["request_id"], "req_9");
        assert_eq!(ap["choices"], json!(["once", "deny"]));
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
