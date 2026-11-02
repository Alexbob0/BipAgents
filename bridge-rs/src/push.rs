//! Push notifications to the agent's registered devices.
//!
//! Content travels on the tailnet, not through Apple: by default alerts carry a generic body and the
//! Notification Service Extension (`mutable-content: 1`) fetches the full text from the bridge.
//! Set `[push] previews = true` to put a short preview in the alert body instead.
//!
//! Port of `bridge/bipbridge/push.py` (owner group: push-runs).

use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use serde_json::{json, Map, Value};

use crate::apns::{ApnsClient, ApnsResult, SendOptions};
use crate::config::SharedConfig;
use crate::fcm::FcmClient;
use crate::logs::{redact6, J};
use crate::relay::RelayClient;
use crate::store::{Row, Store};
use crate::Dict;

/// Python-compatible text helpers shared by the push-runs modules (whitespace, slicing, exception names).
#[path = "push_runs/pyx.rs"]
pub mod pyx;

/// Every notification as data and its pure payload builder (contracts/notifications/).
#[path = "push_runs/notices.rs"]
pub mod notices;
use notices::Notice;

pub const MESSAGE_BODY: &str = "Nouveau message";
pub const TEXTS_KEPT: usize = 200;
pub const TEXT_TTL: f64 = 24.0 * 3600.0;
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
    /// Texts of recent reply / question pushes, fetched by the Notification Service Extension over the
    /// tailnet (GET /v1/replies/{id}): the alert shows the message, which never goes through Apple.
    texts: Mutex<IndexMap<String, (f64, String)>>,
    /// The Live Activities (and Android's progress notifications) following an agent's work (live_activities.rs).
    pub live: crate::live_activities::Registry,
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
        PushService {
            config,
            store,
            apns,
            android,
            service,
            texts: Mutex::new(IndexMap::new()),
            live: Default::default(),
        }
    }

    /// Keeps a reply's text for the Notification Service Extension; returns its id.
    pub fn remember(&self, text: &str) -> String {
        let reply_id = crate::py::uuid4_hex();
        let mut texts = self.texts.lock().unwrap_or_else(|e| e.into_inner());
        texts.insert(reply_id.clone(), (crate::py::time_time(), text.to_string()));
        loop {
            let oldest_expired =
                texts.first().map(|(_, (t, _))| *t < crate::py::time_time() - TEXT_TTL).unwrap_or(false);
            if texts.len() > TEXTS_KEPT || oldest_expired {
                texts.shift_remove_index(0);
            } else {
                break;
            }
        }
        reply_id
    }

    /// A kept text (24 h), for `GET /v1/replies/{id}`.
    pub fn text(&self, reply_id: &str) -> Option<String> {
        let texts = self.texts.lock().unwrap_or_else(|e| e.into_inner());
        texts.get(reply_id).filter(|(t, _)| *t >= crate::py::time_time() - TEXT_TTL).map(|(_, text)| text.clone())
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

    /// « Me prévenir quand il a fini ou qu'il t'attend » is on for this agent (agent_notify.rs): off, its replies,
    /// ended work and questions wait in the app without a notification. Approvals and accord requests (site, login,
    /// held e-mail, connector, vault) always notify: an agent is never stuck without the person knowing (Alex,
    /// 2026-11-02).
    fn notifies(&self, agent: &str) -> bool {
        crate::agent_notify::notifies(&self.config.get(), agent)
    }

    /// The language `agent` writes in (« fr », « en », « de », « es »), for the lines written here.
    fn lang(&self, agent: &str) -> &'static str {
        crate::names::names(&crate::names::agent_language(&self.config.get(), agent)).lang
    }

    pub(crate) fn sender(&self, device: &Row) -> Option<Sender> {
        match device.get("environment").and_then(Value::as_str) {
            Some("relay") => self.service.clone().map(Sender::Relay),
            Some("fcm") => self.android.clone(),
            _ => self.apns.clone(),
        }
    }

    async fn fanout(
        &self,
        agent: &str,
        payload: Dict,
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
        let payload = Value::Object(payload);
        let sends = devices.iter().map(|(device, sender)| {
            let options = SendOptions {
                environment: device.get("environment").and_then(Value::as_str).unwrap_or("").to_string(),
                push_type: push_type.to_string(),
                priority,
                collapse_id: collapse_id.clone(),
                expiration: None,
            };
            let token = device.get("token").and_then(Value::as_str).unwrap_or("").to_string();
            let payload = &payload;
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

    /// What `build` needs to know of `agent` here (its language read from its files only when `notice` writes in it).
    fn context(&self, agent: &str, notice: &Notice) -> notices::Context {
        let previews = self.previews();
        let lang = if notice.uses_lang(previews) { self.lang(agent) } else { "fr" };
        notices::Context { title: self.title(agent), previews, lang: lang.to_string() }
    }

    /// Builds `notice`'s push (notices.rs, the contract's function) and sends it to `agent`'s devices; nothing when it
    /// follows the « Me prévenir » switch and the switch is off.
    async fn notify(&self, agent: &str, notice: Notice) -> usize {
        if notice.follows_switch() && !self.notifies(agent) {
            return 0;
        }
        let built = notices::build(agent, &self.context(agent, &notice), &notice);
        self.fanout(agent, built.payload, built.push_type, built.priority, built.collapse_id).await
    }

    fn owned(value: Option<&str>) -> Option<String> {
        value.map(str::to_string)
    }

    pub async fn notify_message(
        &self,
        agent: &str,
        outbox_id: &str,
        text: &str,
        session_id: Option<&str>,
        _title: Option<&str>,
    ) -> usize {
        let notice =
            Notice::Message { outbox_id: outbox_id.into(), text: text.into(), session_id: Self::owned(session_id) };
        self.notify(agent, notice).await
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
        let notice = Notice::Approval {
            run_id: run_id.into(),
            request_id: Self::owned(request_id),
            choices: choices.map(<[Value]>::to_vec),
            command: Self::owned(command),
            session_id: Self::owned(session_id),
            room_id: Self::owned(room_id),
        };
        self.notify(agent, notice).await
    }

    pub async fn notify_reply(&self, agent: &str, run_id: &str, text: &str, session_id: Option<&str>) -> usize {
        if !self.notifies(agent) {
            return 0;
        }
        let reply_id = self.remember(text);
        let notice = Notice::Reply {
            run_id: run_id.into(),
            text: text.into(),
            session_id: Self::owned(session_id),
            reply_id: Some(reply_id),
        };
        self.notify(agent, notice).await
    }

    /// An agent answered in a group discussion (rooms.rs) nobody looks at: « <agent> · <room> », its words when
    /// previews are on; tapping opens the discussion (`room_id`). Collapsed per discussion.
    pub async fn notify_room(&self, agent: &str, room_id: &str, room_title: &str, text: &str) -> usize {
        if !self.notifies(agent) {
            return 0;
        }
        let reply_id = self.remember(text);
        let notice = Notice::Room {
            room_id: room_id.into(),
            room_title: room_title.into(),
            text: text.into(),
            reply_id: Some(reply_id),
        };
        self.notify(agent, notice).await
    }

    /// A background sub-task finished and Hermes left its report in the conversation without waking the agent:
    /// « 🎧 Podcast jeudi » when it made a file, else « Sous-tâche terminée ». Tapping opens the conversation.
    pub async fn notify_subtask(&self, agent: &str, session_id: &str, line: Option<&str>) -> usize {
        if !self.notifies(agent) {
            return 0;
        }
        let line = given(line);
        let reply_id = line.map(|l| self.remember(l));
        let notice = Notice::Subtask { session_id: session_id.into(), line: Self::owned(line), reply_id };
        self.notify(agent, notice).await
    }

    /// A step of a mailbox scan ended (mail_scan.py): its line when previews are on, else « Sous-tâche terminée »;
    /// tapping opens the agent.
    pub async fn notify_scan(&self, agent: &str, line: &str) -> usize {
        self.notify(agent, Notice::Scan { line: line.into() }).await
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
        let notice = Notice::Job {
            session_id: Self::owned(session_id),
            line: line.into(),
            job_id: job_id.into(),
            body: body.into(),
        };
        self.notify(agent, notice).await
    }

    pub async fn notify_question(
        &self,
        agent: &str,
        run_id: &str,
        request_id: Option<&str>,
        question: Option<&str>,
        session_id: Option<&str>,
    ) -> usize {
        if !self.notifies(agent) {
            return 0;
        }
        let question = given(question);
        let reply_id = question.map(|q| self.remember(q));
        let notice = Notice::Question {
            run_id: run_id.into(),
            request_id: Self::owned(request_id),
            question: Self::owned(question),
            session_id: Self::owned(session_id),
            reply_id,
        };
        self.notify(agent, notice).await
    }

    /// Someone of the family passed something on to this agent (family_news.rs): « Alex : je rentre tard ce soir »
    /// when previews are on, else « Message de ta famille » (the extension fetches the line on the device); tapping
    /// opens the agent.
    pub async fn notify_family(&self, agent: &str, line: &str, note_id: &str) -> usize {
        let reply_id = self.remember(line);
        let notice = Notice::Family { line: line.into(), note_id: note_id.into(), reply_id: Some(reply_id) };
        self.notify(agent, notice).await
    }

    /// The agent's e-mail to an address not allowed yet waits for the person (agent_email.rs): « Veut écrire à
    /// x@y » when previews are on; tapping opens the agent's e-mail screen (`email_held`: the held mail's id).
    pub async fn notify_email_held(&self, agent: &str, to: &str, held_id: &str) -> usize {
        self.notify(agent, Notice::EmailHeld { to: to.into(), held_id: held_id.into() }).await
    }

    /// The agent wants to send data to a site (sandbox_sites.rs; `line`: « Veut envoyer des données à exemple.com
    /// (environ 2 Mo) »): tapping opens the app's Boîte, where the card is.
    pub async fn notify_site_request(&self, agent: &str, line: &str, id: &str) -> usize {
        self.notify(agent, Notice::Site { line: line.into(), id: id.into() }).await
    }

    /// An agent wants to create an account on a website with a password of the vault (vault_logins.rs): tapping opens
    /// the app's Boîte, where the card is.
    pub async fn notify_login_request(&self, agent: &str, host: &str, id: &str) -> usize {
        self.notify(agent, Notice::Login { host: host.into(), id: id.into() }).await
    }

    /// A secret card or a connector's approval card (secrets.rs, connectors.rs): tapping opens the app's Boîte, where
    /// the card is. `key`: `secret_request` or `connector_request`, with the card's request id.
    pub async fn notify_bridge_card(&self, agent: &str, line: &str, key: &str, id: &str) -> usize {
        self.notify(agent, Notice::Card { line: line.into(), key: key.into(), id: id.into() }).await
    }

    /// A date to find with a relative's household (meet.rs): « Léa propose une date : Dîner » when previews are on;
    /// tapping opens the app's Boîte, where the card is (`meet`: the request's id).
    pub async fn notify_meet(&self, agent: &str, line: &str, id: &str) -> usize {
        self.notify(agent, Notice::Meet { line: line.into(), id: id.into() }).await
    }

    /// `notify_silent(agent, reason, **extra)`.
    pub async fn notify_silent(&self, agent: &str, reason: &str, extra: Dict) -> usize {
        self.notify(agent, Notice::Silent { reason: reason.into(), extra }).await
    }
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
