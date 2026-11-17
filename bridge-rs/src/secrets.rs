//! Secrets an agent needs for a connector it made (owner decision 2026-10-28, internal/next/agent-connectors.md): a
//! password, an access token or a one-time code, asked of the person in ONE card and kept by the bridge, never by the
//! agent, its model or the conversation.
//!
//! - **The card** (`ask_secret`, connectors.rs): « Vie a besoin de ton accès Garmin », listed with the approvals
//!   (`GET /v1/approvals`, run id `bip-secret-<id>`), with the identifier (its label from the agent, prefilled only when
//!   the agent gave one, editable) and a secure field. The app answers `POST /v1/vault/secrets/requests/{id}` with the
//!   value: it never travels through Hermes, `/v1/approve` only refuses (a notification can't carry a secret).
//! - **Kept**: a password or a token in the vault (vault.rs, kind « secret », Fernet like the rest), tied to the agent
//!   that asked; the app lists it (name, site, identifier, kind, used by, created, last used), never its value. A code
//!   (MFA) is held in memory for `CODE_TTL`, given to the next run of a connector that uses it, then wiped: never
//!   written anywhere.
//! - **The agent** gets « saved as <name> » (the identifier only when it passed it itself), or a refusal / no answer in
//!   plain words. Names are unique per agent; cards are limited per agent (`MAX_WAITING` at once, `MAX_PER_HOUR`).
//! - **A site's login** (`use: browser`; le coffre rassure, 17/11, passation-pro briefs/design-coffre-rassurer.md §4):
//!   the card says so (`bip_secret_use`) and whether the person may type it themselves in the agent's browser
//!   (`bip_secret_self`: asked by the agent, a visible browser of its own to take over). « Je le taperai moi-même »
//!   (`{"allow": false, "way": "self"}`, kept as the choice `self`) keeps nothing: the agent's call waits until the
//!   person has taken its browser and handed it back (`HAND_WAIT`), then reads the page (`HANDED_BACK`); still held
//!   then, it says it will go on once handed back and stops (`NOT_BACK`), and goes on once, in a later turn, when the
//!   hand is back (card_place.rs `settle`, `self_hand`, `resume_words`). Nobody took the browser while it waited, or
//!   `SELF_KEEP` passed: the answer is closed, a take afterwards resumes nothing (review of 17/11). On an agent other
//!   people talk to, only with the person's confirmation (`confirmed_others`), as « Enregistrer ».
//! - **Blanking** (`Blanker`): every value of the agent's secrets, raw, URL-encoded, JSON-escaped, hex and base64
//!   (padded, unpadded, URL-safe, and the aligned cores of a value inside a longer base64 stream, as in a Basic
//!   header), is replaced by « [secret] » in whatever a connector sends back before it reaches Hermes, the model, the
//!   history or a log line.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use base64::Engine;
use indexmap::IndexMap;
use serde_json::{json, Value};
use tokio::sync::Notify;
use zeroize::{Zeroize, Zeroizing};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

/// The vault's kind for these items.
pub const VAULT_KIND: &str = "secret";
pub const KINDS: [&str; 3] = ["password", "token", "code"];
/// The run id of a card (the apps tell them from Hermes' approvals by it).
pub const RUN_PREFIX: &str = "bip-secret-";
/// How long the agent's call waits for the person.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(330);
/// A card the person opened from the app (« Modifier le mot de passe »): gone after so long unanswered.
pub const EDIT_TTL: f64 = 3600.0;
/// A one-time code waits so long for the run that uses it.
pub const CODE_TTL: Duration = Duration::from_secs(600);
/// Cards waiting at once, per agent.
pub const MAX_WAITING: usize = 2;
/// Cards an agent may raise per hour.
pub const MAX_PER_HOUR: usize = 10;
pub const MAX_VALUE: usize = 4096;
pub const MAX_IDENTIFIER: usize = 200;
/// The refusal's code when an agent other people talk to gets a secret without the person's confirmation.
pub const CONFIRM_OTHERS: &str = "confirm_others";
/// Its words, for an app that doesn't know the code.
pub const CONFIRM_OTHERS_TEXT: &str = "Mets à jour l’app pour donner cet accès : d’autres personnes parlent à cet agent, \
l’app doit d’abord te le demander.";
/// What a secret's value becomes.
pub const MARK: &str = "[secret]";

/// `garmin`, `ovh_api`: lower-case letters, digits and `_`, 2-32 characters, a letter first.
pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (2..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'_')
}

/// The environment variables a connector gets for secret `name` of `kind`: (identifier's, value's).
pub fn env_names(name: &str, kind: &str) -> (String, String) {
    let base = name.to_uppercase();
    let value = match kind {
        "token" => "TOKEN",
        "code" => "CODE",
        _ => "PASSWORD",
    };
    (format!("{base}_USERNAME"), format!("{base}_{value}"))
}

/// What the person reads for a kind.
pub fn kind_words(kind: &str) -> &'static str {
    match kind {
        "token" => "clé d'accès",
        "code" => "code à usage unique",
        _ => "mot de passe",
    }
}

// --- blanking ------------------------------------------------------------------------------------------------------

fn percent(value: &str, keep: fn(u8) -> bool, upper: bool, plus: bool) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if keep(b) {
            out.push(b as char);
        } else if plus && b == b' ' {
            out.push('+');
        } else if upper {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push_str(&format!("%{b:02x}"));
        }
    }
    out
}

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}

/// RFC 3986's unreserved characters (Python's `quote(safe="")`, most clients).
fn unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.~".contains(&b)
}

/// JavaScript's `encodeURIComponent`.
fn component(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b)
}

/// The forms a secret's value may take in a connector's output, the longest first.
pub fn variants(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if value.chars().count() < 3 {
        return out;
    }
    out.push(value.to_string());
    if value.len() >= 4 {
        for keep in [alnum as fn(u8) -> bool, unreserved, component] {
            for upper in [true, false] {
                for plus in [false, true] {
                    out.push(percent(value, keep, upper, plus));
                }
            }
        }
        if let Ok(quoted) = serde_json::to_string(value) {
            out.push(quoted.trim_matches('"').to_string());
        }
        out.push(hex::encode(value.as_bytes()));
        out.push(hex::encode_upper(value.as_bytes()));
        let n = value.len();
        for engine in [base64::engine::general_purpose::STANDARD_NO_PAD, base64::engine::general_purpose::URL_SAFE_NO_PAD]
        {
            for k in 0..3usize {
                let mut bytes = vec![0u8; k];
                bytes.extend_from_slice(value.as_bytes());
                let encoded = engine.encode(&bytes);
                // the characters that depend on the value's bytes only, wherever it sits in a longer stream
                let start = (8 * k).div_ceil(6);
                let end = (8 * (k + n)) / 6;
                if end > start + 5 && end <= encoded.len() {
                    out.push(encoded[start..end].to_string());
                }
            }
        }
        out.push(base64::engine::general_purpose::STANDARD.encode(value.as_bytes()));
        out.push(base64::engine::general_purpose::URL_SAFE.encode(value.as_bytes()));
    }
    out.retain(|v| v.chars().count() >= 3);
    out.sort_by_key(|v| std::cmp::Reverse(v.len()));
    out.dedup();
    out
}

/// Replaces every known form of the given values by « [secret ] ».
#[derive(Default, Clone)]
pub struct Blanker {
    forms: Vec<Zeroizing<String>>,
}

impl Blanker {
    pub fn new<S: AsRef<str>>(values: &[S]) -> Self {
        let mut forms: Vec<Zeroizing<String>> = Vec::new();
        for value in values {
            for form in variants(value.as_ref()) {
                if !forms.iter().any(|f| f.as_str() == form) {
                    forms.push(Zeroizing::new(form));
                }
            }
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        Blanker { forms }
    }

    pub fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_string();
        for form in &self.forms {
            if out.contains(form.as_str()) {
                out = out.replace(form.as_str(), MARK);
            }
        }
        out
    }

    /// Every string of a JSON value (keys too).
    pub fn value(&self, value: Value) -> Value {
        match value {
            Value::String(s) => Value::String(self.text(&s)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (self.text(&k), self.value(v))).collect()),
            other => other,
        }
    }
}

/// The blanker for everything an agent's connectors send back: all its secrets' values, the codes waiting too.
pub fn blanker_for(state: &AppState, agent: &str, extra: &[Zeroizing<String>]) -> Blanker {
    let mut values: Vec<Zeroizing<String>> =
        state.vault.secret_values(agent).unwrap_or_default().into_iter().map(|(_, _, v)| v).collect();
    values.extend(codes_of(agent));
    values.extend(extra.iter().cloned());
    Blanker::new(&values.iter().map(|v| v.as_str()).collect::<Vec<_>>())
}

// --- one-time codes (memory only) -------------------------------------------------------------------------------------

type CodeTable = HashMap<(String, String), (Zeroizing<String>, Instant)>;

static CODES: LazyLock<Mutex<CodeTable>> = LazyLock::new(Default::default);

fn codes() -> std::sync::MutexGuard<'static, CodeTable> {
    let mut table = CODES.lock().unwrap_or_else(|e| e.into_inner());
    table.retain(|_, (_, at)| at.elapsed() < CODE_TTL);
    table
}

/// The code given for `name`, taken: the next run uses it, nothing else ever sees it again.
pub fn take_code(agent: &str, name: &str) -> Option<Zeroizing<String>> {
    codes().remove(&(agent.to_string(), name.to_string())).map(|(code, _)| code)
}

pub fn code_waiting(agent: &str, name: &str) -> bool {
    codes().contains_key(&(agent.to_string(), name.to_string()))
}

fn codes_of(agent: &str) -> Vec<Zeroizing<String>> {
    codes().iter().filter(|((a, _), _)| a == agent).map(|(_, (code, _))| code.clone()).collect()
}

// --- the cards ---------------------------------------------------------------------------------------------------------

/// What a card asks for.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub name: String,
    /// `https://connect.garmin.com` (empty: no site).
    pub origin: String,
    pub kind: String,
    pub identifier_label: String,
    pub identifier: String,
    pub reason: String,
    /// Opened by the person from a connector's page (no agent waits for it).
    pub edit: bool,
    pub connector: Option<String>,
    /// A login for the agent's visible browser on `origin` (a supplier's portal): kept as a vault login the agent
    /// logs in with (`vault_login`), not as a connector's secret. Same card.
    pub browser_login: bool,
    /// Such a login's own login page, when the site the agent gave says more than its origin (vault.rs `login_url_of`).
    pub login_url: Option<String>,
}

impl Request {
    pub fn host(&self) -> String {
        self.origin.split("://").nth(1).unwrap_or(&self.origin).to_string()
    }

    /// For a password the identifier is asked too (always); for the others when the agent gave one.
    pub fn identifier_required(&self) -> bool {
        self.kind == "password"
    }
}

/// The person's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Kept (a code: held for the next run), with the identifier the person confirmed.
    Saved { identifier: String },
    Refused,
    /// « Je le taperai moi-même »: nothing kept, the person signs in themselves in the agent's browser.
    TypedByPerson,
}

/// The choice kept for « Je le taperai moi-même » (approvals_log.rs, the answered card).
pub const SELF: &str = "self";
/// How long the agent's call waits for the person to take its browser and hand it back (as « Payer dans le
/// navigateur de l'agent », handover.rs).
pub const HAND_WAIT: Duration = Duration::from_secs(600);
/// What the agent reads once the person signed in themselves and handed its browser back (design §1.5).
pub const HANDED_BACK: &str = "The person signed in themselves in your browser and handed it back. Read the page: signed \
in, go on; the sign-in page still there, tell them in one sentence what you see. Nothing was kept in the vault: don't \
ask for this site again in this conversation.";
/// What it reads when they still hold it after `HAND_WAIT`, or never took it: it goes on in a later turn once they
/// hand it back (card_place.rs `settle`, `resume_words`).
pub const NOT_BACK: &str = "The person chose to sign in themselves in your browser and hasn't handed it back yet. Don't \
use your browser: say you'll go on once they hand it back, then stop.";

/// `HAND_WAIT`, only ever shorter in the black-box tests (`BIP_TEST_HAND_WAIT_SECONDS`, 1 to 600, said in the log).
pub fn hand_wait() -> Duration {
    let Some(seconds) = std::env::var("BIP_TEST_HAND_WAIT_SECONDS").ok().and_then(|s| s.trim().parse::<u64>().ok())
    else {
        return HAND_WAIT;
    };
    let wait = Duration::from_secs(seconds.clamp(1, HAND_WAIT.as_secs()));
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| tracing::warn!(seconds = wait.as_secs(), "secret card: the person's hand waited for at a test interval"));
    wait
}

/// Where the person's hand is, for an answer « Je le taperai moi-même » not told to the agent yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    /// Not taken yet, or held now: wait.
    Wait,
    /// Held now, seen for the first time: note it, and wait.
    Taken,
    /// Taken, then handed back: the agent goes on.
    Back,
    /// Never taken by the time the agent stopped waiting, or kept too long: forgotten, nothing told.
    Closed,
}

/// A `self` answer's longest wait for the hand, taken in time (review of 17/11: never days later).
pub const SELF_KEEP: Duration = Duration::from_secs(2 * 3600);

/// For an answer kept until the agent knows it (card_place.rs `settle`), `age` seconds after it was given: `hand_now`,
/// or `Hand::Closed` when nobody took the browser by the time the agent stopped waiting, or `SELF_KEEP` passed.
pub fn hand_after(holds: bool, held: bool, age: f64) -> Hand {
    // the agent's call waits `hand_wait`, checking each second; a minute more for an answer it never saw
    let gave_up = hand_wait().as_secs_f64() + 60.0;
    if age > SELF_KEEP.as_secs_f64() || (!held && age > gave_up) {
        return Hand::Closed;
    }
    hand_now(holds, held)
}

/// `holds`: the person holds the agent's browser now; `held`: they were seen holding it since their answer.
pub fn hand_now(holds: bool, held: bool) -> Hand {
    match (holds, held) {
        (true, false) => Hand::Taken,
        (false, true) => Hand::Back,
        _ => Hand::Wait,
    }
}

/// When the person last handed each agent's browser back (desktop.rs `control`): a take and a hand-back between two
/// looks of a wait (a second apart) still count.
static GIVEN_BACK: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);

/// The person handed `agent`'s browser back, now.
pub fn hand_given_back(agent: &str) {
    GIVEN_BACK.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string(), py::time_time());
}

/// Handed back at `since` (Unix seconds) or later.
pub fn given_back_since(agent: &str, since: f64) -> bool {
    GIVEN_BACK.lock().unwrap_or_else(|e| e.into_inner()).get(agent).is_some_and(|at| *at >= since)
}

/// The person holds the agent's browser in the app: its desktop taken over (desktop.rs `control`), or Hermes' lease.
pub fn person_holds(state: &AppState, agent: &str) -> bool {
    if state.desktop_control.lock().unwrap_or_else(|e| e.into_inner()).get(agent).copied().unwrap_or(false) {
        return true;
    }
    let config = state.config();
    let folder = config.agent(Some(agent)).and_then(|a| crate::desktop::desktop_dir(&config, a));
    folder.filter(|f| f.is_dir()).is_some_and(|f| crate::desktop::app_holds(&f))
}

/// How the agent's wait for the person's hand ended (`wait_for_hand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandEnd {
    /// Taken, then handed back.
    Back,
    /// Taken, still held when the wait ended: the agent goes on once it is back (card_place.rs).
    StillHeld,
    /// Never taken while the agent waited: nothing more to wait for.
    NeverTaken,
}

/// After « Je le taperai moi-même »: until the person has taken the agent's browser and handed it back, or
/// `hand_wait` passed, as « Payer dans le navigateur de l'agent » (handover.rs).
pub async fn wait_for_hand(state: &AppState, agent: &str) -> HandEnd {
    let started = Instant::now();
    let since = py::time_time();
    let wait = hand_wait();
    let mut held = false;
    loop {
        let holds = person_holds(state, agent);
        held |= given_back_since(agent, since);
        match hand_now(holds, held) {
            Hand::Back => return HandEnd::Back,
            Hand::Taken => held = true,
            Hand::Wait | Hand::Closed => {}
        }
        if started.elapsed() >= wait {
            return if held { HandEnd::StillHeld } else { HandEnd::NeverTaken };
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// For an answer card_place.rs keeps until the agent knows it: where the person's hand is when it was « Je le taperai
/// moi-même » to a secret card, `age` seconds after (None: another answer, or another card). `held`: seen holding
/// since the answer.
pub fn self_hand(state: &AppState, card: &str, agent: &str, choice: &str, held: bool, age: f64) -> Option<Hand> {
    if !card.starts_with(RUN_PREFIX) || choice != SELF {
        return None;
    }
    let held = held || given_back_since(agent, py::time_time() - age);
    Some(hand_after(person_holds(state, agent), held, age))
}

/// Whether the person may type it themselves in the agent's browser: a site's login for its visible browser, asked
/// by the agent (not opened from the app), and a browser they can take over.
pub fn may_type_self(config: &Config, agent: &str, request: &Request) -> bool {
    request.browser_login
        && request.kind == "password"
        && !request.edit
        && !request.origin.is_empty()
        && crate::browsers::ports_of(config, agent).is_some()
}

/// Why « Je le taperai moi-même » can't be taken.
#[derive(Debug, PartialEq, Eq)]
pub enum SelfError {
    /// No such card waiting (answered already, gone).
    NoCard,
    /// Not a site's login, or no browser to take over (`bip_secret_self` false).
    NotHere,
    /// Other people talk to the agent and the person did not confirm (`confirmed_others`), as for « Enregistrer »:
    /// their session would stay in that agent's browser (review R1, 17/11).
    ConfirmOthers,
}

/// « Je le taperai moi-même » (`{"allow": false, "way": "self"}`): nothing kept, the agent's call waits for the
/// browser handed back (`wait_for_hand`). For an agent other people talk to, only once the person confirmed it
/// (`confirmed_others`), as `give` asks: the session they open stays in that agent's browser.
pub fn choose_self(
    config: &Config,
    agent: Option<&str>,
    id: &str,
    confirmed_others: bool,
    via: &str,
) -> Result<(), SelfError> {
    let (agent, request) = {
        let mut table = cards();
        let Some(card) = table.get_mut(id).filter(|c| c.answer.is_none() && agent.is_none_or(|a| a == c.agent)) else {
            return Err(SelfError::NoCard);
        };
        if !may_type_self(config, &card.agent, &card.request) {
            return Err(SelfError::NotHere);
        }
        if !confirmed_others && crate::vault::others_facing(config, &card.agent) {
            tracing::info!(agent = %card.agent, secret = %card.request.name,
                "secret card: not typed by the person, other people talk to the agent, not confirmed");
            return Err(SelfError::ConfirmOthers);
        }
        card.answer = Some(Outcome::TypedByPerson);
        card.notify.notify_waiters();
        let out = (card.agent.clone(), card.request.clone());
        if card.left {
            table.shift_remove(id); // nobody waits for it: the agent goes on once the hand is back (card_place.rs)
        }
        out
    };
    record(config, &agent, id, &request, SELF, via);
    tracing::info!(agent = %agent, secret = %request.name, "secret card: the person signs in themselves");
    Ok(())
}

/// What the agent reads of an answer it learns in a later turn (card_place.rs `resume`): « self » only.
pub fn resume_words(card: &str, choice: &str, summary: &str) -> Option<String> {
    if !card.starts_with(RUN_PREFIX) || choice != SELF {
        return None;
    }
    let what = if summary.is_empty() { "your sign-in" } else { summary };
    Some(format!("« {what} » — {}", HANDED_BACK.trim_end_matches('.')))
}

struct Card {
    agent: String,
    request: Request,
    at: f64,
    answer: Option<Outcome>,
    notify: Arc<Notify>,
    /// The agent's call stopped waiting (`ASK_TIMEOUT`): the card stays for the person until `LEFT_TTL`, gone once
    /// answered; the agent goes on then (card_place.rs `resume`).
    left: bool,
}

/// A card the agent stopped waiting for: still answerable for so long.
pub const LEFT_TTL: f64 = 3.0 * 86400.0;

static CARDS: LazyLock<Mutex<IndexMap<String, Card>>> = LazyLock::new(Default::default);
static RAISED: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(Default::default);

fn cards() -> std::sync::MutexGuard<'static, IndexMap<String, Card>> {
    let mut table = CARDS.lock().unwrap_or_else(|e| e.into_inner());
    let now = py::time_time();
    table.retain(|_, c| !(c.request.edit && now - c.at > EDIT_TTL));
    table
}

fn shown_name(config: &Config, agent: &str) -> String {
    config.agent(Some(agent)).map(|a| a.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or(agent.to_string())
}

/// « Garmin » from `connect.garmin.com`, else the secret's name.
pub fn service_of(request: &Request) -> String {
    let host = request.host();
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    let word = if parts.len() >= 2 { parts[parts.len() - 2] } else { request.name.as_str() };
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => request.name.clone(),
    }
}

/// The card's sentence.
pub fn sentence(shown: &str, request: &Request) -> String {
    let host = request.host();
    let what = match request.kind.as_str() {
        "code" => "du code reçu pour",
        "token" => "de ta clé d'accès",
        _ => "de ton accès",
    };
    if host.is_empty() {
        format!("{shown} a besoin {what} {}", service_of(request))
    } else {
        format!("{shown} a besoin {what} {} ({host})", service_of(request))
    }
}

/// The card's sentence in `lang`, for its push (the card's own is French: the apps rebuild it in theirs; Alex,
/// 2026-11-01: « Vie a besoin de ton accès » reached an English person).
pub fn sentence_in(lang: &str, shown: &str, request: &Request) -> String {
    let lang = crate::names::names(lang).lang;
    if lang == "fr" {
        return sentence(shown, request);
    }
    let service = service_of(request);
    let host = request.host();
    let site = if host.is_empty() { String::new() } else { format!(" ({host})") };
    match (lang, request.kind.as_str()) {
        ("de", "code") => format!("{shown} braucht den Code, den du für {service} erhalten hast{site}"),
        ("de", "token") => format!("{shown} braucht deinen Zugangsschlüssel für {service}{site}"),
        ("de", _) => format!("{shown} braucht deinen Zugang zu {service}{site}"),
        ("es", "code") => format!("{shown} necesita el código recibido para {service}{site}"),
        ("es", "token") => format!("{shown} necesita tu clave de acceso de {service}{site}"),
        ("es", _) => format!("{shown} necesita tu acceso a {service}{site}"),
        (_, "code") => format!("{shown} needs the code you received for {service}{site}"),
        (_, "token") => format!("{shown} needs your access key for {service}{site}"),
        _ => format!("{shown} needs your access to {service}{site}"),
    }
}

/// A new card, or why not (the agent is told). `edit`: opened by the person, no rate limit.
pub fn open(agent: &str, request: Request) -> Result<String, String> {
    let mut table = cards();
    // left by the agent's call: gone after LEFT_TTL, or replaced by the same request asked again
    let now = py::time_time();
    table.retain(|_, c| !c.left || (now - c.at < LEFT_TTL && !(c.agent == agent && c.request.name == request.name)));
    if table.values().any(|c| c.agent == agent && c.request.name == request.name && c.answer.is_none()) {
        return Err(format!("A card for « {} » is already waiting for the person.", request.name));
    }
    if !request.edit {
        let waiting = table.values().filter(|c| c.agent == agent && c.answer.is_none() && !c.request.edit).count();
        if waiting >= MAX_WAITING {
            return Err("Several cards already wait for the person: wait for their answers first.".into());
        }
        let mut raised = RAISED.lock().unwrap_or_else(|e| e.into_inner());
        let recent = raised.entry(agent.to_string()).or_default();
        recent.retain(|at| at.elapsed() < Duration::from_secs(3600));
        if recent.len() >= MAX_PER_HOUR {
            return Err("Too many secrets asked within the hour: wait, and ask the person what they want.".into());
        }
        recent.push(Instant::now());
    }
    let id = py::uuid4_hex()[..12].to_string();
    tracing::info!(agent, secret = %request.name, kind = %request.kind, edit = request.edit, "secret card raised");
    table.insert(
        id.clone(),
        Card { agent: agent.to_string(), request, at: py::time_time(), answer: None, notify: Arc::new(Notify::new()), left: false },
    );
    drop(table);
    Ok(id)
}

/// The person's answer to card `id` (None: no answer in time). The card goes either way.
pub async fn wait(id: &str) -> Option<Outcome> {
    let Some(notify) = cards().get(id).map(|c| c.notify.clone()) else { return Some(Outcome::Refused) };
    let deadline = tokio::time::Instant::now() + ASK_TIMEOUT;
    let outcome = loop {
        let wake = notify.notified();
        match cards().get(id).map(|c| c.answer.clone()) {
            Some(Some(answer)) => break Some(answer),
            Some(None) => {}
            None => break Some(Outcome::Refused),
        }
        if tokio::time::timeout_at(deadline, wake).await.is_err() {
            break None;
        }
    };
    let mut table = cards();
    match (&outcome, table.get_mut(id)) {
        // not answered in time: left for the person (Alex 5/11: answered late, the agent goes on)
        (None, Some(card)) => card.left = true,
        _ => {
            table.shift_remove(id);
        }
    }
    outcome
}

/// The card waiting with this id, as the apps show it.
fn item_of(config: &Config, id: &str, card: &Card) -> Dict {
    let r = &card.request;
    let item = json!({
        "agent": card.agent,
        "run_id": format!("{RUN_PREFIX}{id}"),
        "request_id": id,
        "command": if r.host().is_empty() { r.name.clone() } else { format!("{} · {}", r.name, r.host()) },
        "description": sentence(&shown_name(config, &card.agent), r),
        "choices": ["once", "deny"],
        "created_at": card.at,
        "session_id": null,
        "bip_secret_name": r.name,
        "bip_secret_site": r.host(),
        "bip_secret_kind": r.kind,
        "bip_secret_identifier_label": r.identifier_label,
        "bip_secret_identifier": r.identifier,
        "bip_secret_identifier_required": r.identifier_required(),
        "bip_secret_reason": r.reason,
        "bip_secret_edit": r.edit,
        "bip_secret_connector": r.connector,
        // le coffre rassure (17/11): a site's login or a connector's secret, and « Je le taperai moi-même »
        "bip_secret_use": if r.browser_login { "browser" } else { "connector" },
        "bip_secret_self": may_type_self(config, &card.agent, r),
    });
    let mut item = item.as_object().cloned().unwrap_or_default();
    if crate::vault::others_facing(config, &card.agent) {
        // other people talk to it: the app asks one confirmation first (`give`'s `confirmed_others`)
        item.insert("bip_secret_others".into(), json!(true));
        item.insert("bip_secret_others_names".into(), json!(crate::vault::others_names(config, &card.agent)));
    }
    item
}

/// The cards waiting (`GET /v1/approvals`), oldest first.
pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let now = py::time_time();
    cards()
        .iter()
        .filter(|(_, c)| c.answer.is_none() && agent.is_none_or(|a| a == c.agent) && !(c.left && now - c.at >= LEFT_TTL))
        .map(|(id, c)| item_of(config, id, c))
        .collect()
}

/// One card as the apps show it (the app shows a card it opened at once).
pub fn item(config: &Config, id: &str) -> Option<Dict> {
    cards().get(id).map(|c| item_of(config, id, c))
}

fn record(config: &Config, agent: &str, id: &str, request: &Request, choice: &str, via: &str) {
    // the conversation the card showed in (card_place.rs): its answered line there (« ✋ Tapé par toi dans le
    // Bureau », 17/11); none when it was answered before it was placed
    let run_id = format!("{RUN_PREFIX}{id}");
    let shown = crate::card_place::of(config, &run_id).and_then(|p| p.report.or(p.session));
    let fields = json!({
        "session_id": shown, "run_id": run_id, "request_id": id,
        "command": if request.host().is_empty() { request.name.clone() } else { format!("{} · {}", request.name, request.host()) },
        "description": sentence(&shown_name(config, agent), request),
        "topic": "secret", "choice": choice, "via": via,
    });
    if let Some(map) = fields.as_object() {
        if let Err(exc) = crate::approvals_log::record(config, agent, map) {
            tracing::warn!(agent, error = %exc, "secret decision not kept");
        }
    }
}

/// Refused (the app, or a notification's action). False: no such card waiting.
pub fn deny(config: &Config, agent: Option<&str>, id: &str, via: &str) -> bool {
    let (agent, request) = {
        let mut table = cards();
        let Some(card) = table.get_mut(id).filter(|c| c.answer.is_none() && agent.is_none_or(|a| a == c.agent)) else {
            return false;
        };
        card.answer = Some(Outcome::Refused);
        card.notify.notify_waiters();
        let out = (card.agent.clone(), card.request.clone());
        if card.request.edit || card.left {
            table.shift_remove(id);
        }
        out
    };
    record(config, &agent, id, &request, "deny", via);
    tracing::info!(agent = %agent, secret = %request.name, "secret card refused");
    true
}

/// Why a value can't be taken.
#[derive(Debug)]
pub enum GiveError {
    NoCard,
    /// The agent talks with other people too, and the person did not confirm (an app from before the confirmation).
    ConfirmOthers,
    Invalid(String),
    Failed(anyhow::Error),
}

/// The person gave the value (and the identifier): kept in the vault, or held for the next run (a code). Never logged.
/// For an agent other people talk to (vault.rs `others_facing`), only once the person confirmed it
/// (`confirmed_others`, the apps' « … parle aussi avec … Continuer ? »): an app that doesn't ask can't skip it.
pub async fn give(
    state: &AppState,
    id: &str,
    value: Zeroizing<String>,
    identifier: Option<String>,
    confirmed_others: bool,
    via: &str,
) -> Result<String, GiveError> {
    let config = state.config();
    let (agent, request) = match cards().get(id).filter(|c| c.answer.is_none()) {
        Some(card) => (card.agent.clone(), card.request.clone()),
        None => return Err(GiveError::NoCard),
    };
    if value.is_empty() || value.len() > MAX_VALUE || value.contains('\0') {
        return Err(GiveError::Invalid(format!("the value must be 1 to {MAX_VALUE} characters")));
    }
    let identifier = identifier.map(|i| i.trim().to_string()).unwrap_or_else(|| request.identifier.clone());
    if identifier.chars().count() > MAX_IDENTIFIER {
        return Err(GiveError::Invalid(format!("the identifier is {MAX_IDENTIFIER} characters at most")));
    }
    if request.identifier_required() && identifier.is_empty() {
        return Err(GiveError::Invalid("the identifier is required".into()));
    }
    // card-guards, 14/11: a card's number never goes through a secure-field card (nor back to the agent with its
    // identifier), whatever it was asked for; a social security number may (UATP's look: `shows_card_number`)
    if crate::card_details::shows_card_number(&value) || crate::card_details::shows_card_number(&identifier) {
        tracing::info!(agent = %agent, secret = %request.name, "secret not kept: a payment card's number");
        return Err(GiveError::Invalid(crate::card_details::GIVEN_REFUSED.into()));
    }
    if !confirmed_others && crate::vault::others_facing(&config, &agent) {
        tracing::info!(agent = %agent, secret = %request.name, "secret not kept: other people talk to the agent, not confirmed");
        return Err(GiveError::ConfirmOthers);
    }
    if request.kind == "code" {
        codes().insert((agent.clone(), request.name.clone()), (value, Instant::now()));
        // the code's name is known (its connector lists it), never its value
        let Some(holder) = config.agent(Some(&agent)).cloned() else { return Err(GiveError::NoCard) };
        let new = crate::vault::NewSecret {
            name: &request.name,
            origin: &request.origin,
            kind: &request.kind,
            identifier_label: &request.identifier_label,
            identifier: &identifier,
            value: None,
        };
        state.vault.save_secret(&config, &holder, &new).await.map_err(GiveError::Failed)?;
    } else if request.browser_login && request.kind == "password" && !request.origin.is_empty() {
        // A site login for its browser: the vault's own login item (filled on that exact site by `browser_vault`),
        // given to this agent only, as the person's « Coffre » would.
        let Some(holder) = config.agent(Some(&agent)).cloned() else { return Err(GiveError::NoCard) };
        let mut body = Dict::new();
        body.insert("kind".into(), json!("login"));
        body.insert("label".into(), json!(request.host()));
        // review R8: the site is the origin the person's card named; the login page only beside it (vault.rs keeps it
        // on that origin only)
        body.insert("site".into(), json!(request.origin));
        if let Some(page) = &request.login_url {
            body.insert("login_url".into(), json!(page));
        }
        body.insert("identifier_type".into(), json!(crate::vault::identifier_type_of(&identifier)));
        body.insert("identifier".into(), json!(identifier));
        body.insert("password".into(), json!(value.as_str()));
        body.insert("agents".into(), json!([holder.name]));
        if confirmed_others {
            body.insert("confirmed_others".into(), json!(true));
        }
        let made = state.vault.create(&config, &body).await;
        if let Some(Value::String(p)) = body.get_mut("password") {
            p.zeroize();
        }
        made.map_err(GiveError::Failed)?;
    } else {
        let Some(holder) = config.agent(Some(&agent)).cloned() else { return Err(GiveError::NoCard) };
        let new = crate::vault::NewSecret {
            name: &request.name,
            origin: &request.origin,
            kind: &request.kind,
            identifier_label: &request.identifier_label,
            identifier: &identifier,
            value: Some(value.as_str()),
        };
        state.vault.save_secret(&config, &holder, &new).await.map_err(GiveError::Failed)?;
    }
    {
        let mut table = cards();
        if let Some(card) = table.get_mut(id) {
            card.answer = Some(Outcome::Saved { identifier: identifier.clone() });
            card.notify.notify_waiters();
            if card.request.edit || card.left {
                table.shift_remove(id);
            }
        }
    }
    record(&config, &agent, id, &request, "once", via);
    tracing::info!(agent = %agent, secret = %request.name, kind = %request.kind, "secret given by the person");
    Ok(request.name)
}

// --- the app's route (bridge key) --------------------------------------------------------------------------------------

/// A secret card answered in the app: `{"allow": true, "value": "…", "identifier": "…"}`, `{"allow": false}`, or
/// « Je le taperai moi-même » `{"allow": false, "way": "self"}` (a card with `bip_secret_self` only; with
/// `"confirmed_others": true` for an agent other people talk to, as « Enregistrer »).
async fn answer_route(State(state): State<AppState>, Path(id): Path<String>, body: Bytes) -> ApiResult<Response> {
    let body: Value = serde_json::from_slice(&body).map_err(|_| HttpError::new(400, "invalid JSON"))?;
    let Some(allow) = body.get("allow").and_then(Value::as_bool) else {
        return Err(HttpError::new(400, "allow must be true or false"));
    };
    let way = body.get("way").filter(|w| !w.is_null());
    let mut response = if let Some(way) = way {
        if allow || way.as_str() != Some(SELF) {
            return Err(HttpError::new(400, "way: only « self », with allow false"));
        }
        let confirmed = body.get("confirmed_others").and_then(Value::as_bool) == Some(true);
        match choose_self(&state.config(), None, &id, confirmed, "app") {
            Ok(()) => web::ok(json!({"ok": true, "choice": SELF})),
            Err(SelfError::NoCard) => return Err(HttpError::new(404, "no such request (answered already?)")),
            Err(SelfError::ConfirmOthers) => {
                return Err(HttpError::json(409, json!({"code": CONFIRM_OTHERS, "message": CONFIRM_OTHERS_TEXT})))
            }
            Err(SelfError::NotHere) => {
                return Err(HttpError::new(400, "not a site's login with a browser of the agent's to take over"))
            }
        }
    } else if !allow {
        if !deny(&state.config(), None, &id, "app") {
            return Err(HttpError::new(404, "no such request (answered already?)"));
        }
        web::ok(json!({"ok": true, "choice": "deny"}))
    } else {
        let value = Zeroizing::new(body.get("value").and_then(Value::as_str).unwrap_or("").to_string());
        let identifier = body.get("identifier").and_then(Value::as_str).map(str::to_string);
        let confirmed = body.get("confirmed_others").and_then(Value::as_bool) == Some(true);
        match give(&state, &id, value, identifier, confirmed, "app").await {
            Ok(name) => web::ok(json!({"ok": true, "name": name})),
            Err(GiveError::NoCard) => return Err(HttpError::new(404, "no such request (answered already?)")),
            Err(GiveError::ConfirmOthers) => return Err(HttpError::json(409, json!({"code": CONFIRM_OTHERS,
                "message": CONFIRM_OTHERS_TEXT}))),
            Err(GiveError::Invalid(why)) => return Err(HttpError::new(400, why)),
            Err(GiveError::Failed(exc)) => {
                tracing::warn!(request = %id, error = %crate::agents_admin::tail(&exc.to_string(), 200), "secret not kept");
                // The agent's vault out of reach is the server's to fix, not « not possible for this agent ».
                if let Some(crate::vault::VaultError::Unavailable(agent)) = exc.downcast_ref::<crate::vault::VaultError>() {
                    return Err(HttpError::json(503, json!({"code": "vault_unavailable", "agents": [agent]})));
                }
                return Err(HttpError::new(409, "the secret could not be kept"));
            }
        }
    };
    response.headers_mut().insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    Ok(response)
}

/// `/v1/vault/secrets/requests/{id}` (mounted under /v1 with the bridge key).
pub fn app_router() -> Router<AppState> {
    Router::new().route("/vault/secrets/requests/{id}", post(answer_route))
}

/// The agent's identity for a card (tests and connectors.rs).
pub fn agent_of_card(id: &str) -> Option<String> {
    cards().get(id).map(|c| c.agent.clone())
}

/// Cards of a deleted agent go.
pub fn forget(agent: &AgentConfig) {
    cards().retain(|_, c| c.agent != agent.name);
    codes().retain(|(a, _), _| a != &agent.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "Tr0ub4dor&3 x/y+z";

    #[test]
    fn names_and_variables() {
        assert!(valid_name("garmin") && valid_name("ovh_api2"));
        assert!(!valid_name("G") && !valid_name("1abc") && !valid_name("ab-c") && !valid_name(&"a".repeat(33)));
        assert_eq!(env_names("garmin", "password"), ("GARMIN_USERNAME".into(), "GARMIN_PASSWORD".into()));
        assert_eq!(env_names("ovh_api", "token").1, "OVH_API_TOKEN");
        assert_eq!(env_names("garmin_mfa", "code").1, "GARMIN_MFA_CODE");
    }

    #[test]
    fn every_encoding_is_blanked() {
        let b = Blanker::new(&[SECRET]);
        let std = base64::engine::general_purpose::STANDARD;
        let basic = std.encode(format!("alex@example.org:{SECRET}"));
        let cases = [
            SECRET.to_string(),
            percent(SECRET, unreserved, true, false),
            percent(SECRET, alnum, false, true),
            percent(SECRET, component, true, false),
            std.encode(SECRET),
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(SECRET),
            hex::encode(SECRET),
            serde_json::to_string(SECRET).unwrap(),
            format!("Authorization: Basic {basic}"),
        ];
        for case in cases {
            let out = b.text(&format!("before {case} after"));
            assert!(out.contains(MARK), "{case} -> {out}");
            assert!(!out.contains(SECRET), "{out}");
        }
        // inside a longer base64 stream (a Basic header): the value's own characters are gone
        let out = b.text(&basic);
        let core = &std.encode(format!("alex@example.org:{SECRET}"))[24..32];
        assert!(!out.contains(core), "{out}");
        let json = b.value(json!({"ok": [SECRET, {"k": format!("x{SECRET}")}], SECRET: 1}));
        assert!(!json.to_string().contains(SECRET));
        assert_eq!(Blanker::new(&["ab"]).text("ab cd"), "ab cd"); // too short to blank
    }

    #[test]
    fn cards_are_limited_and_unique() {
        let request = |name: &str| Request { name: name.into(), kind: "password".into(), ..Default::default() };
        let a = open("unit-a", request("garmin")).unwrap();
        assert!(open("unit-a", request("garmin")).unwrap_err().contains("already waiting"));
        let _b = open("unit-a", request("strava")).unwrap();
        assert!(open("unit-a", request("ovh")).unwrap_err().contains("Several cards"));
        assert!(cards().get(&a).unwrap().request.identifier_required());
        assert_eq!(cards().values().filter(|c| c.agent == "unit-a").count(), 2);
        cards().retain(|_, c| c.agent != "unit-a");
    }

    /// Alex 5/11: answered late, the agent goes on. A card whose call stopped waiting stays for the person, is gone
    /// once answered, replaced when the agent asks the same again, and gone after LEFT_TTL.
    #[test]
    fn a_card_the_agent_stopped_waiting_for_stays_answerable() {
        let dir = tempfile::tempdir().unwrap();
        let config = crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}),
            None,
        )
        .unwrap();
        let request = |name: &str| Request { name: name.into(), kind: "token".into(), ..Default::default() };
        let left = |id: &str| cards().get_mut(id).unwrap().left = true;
        let mine = || pending_items(&config, Some("unit-left")).len();
        let a = open("unit-left", request("meteo")).unwrap();
        left(&a);
        assert_eq!(mine(), 1);
        // asked again: the new card replaces it
        let b = open("unit-left", request("meteo")).unwrap();
        assert!(cards().get(&a).is_none() && mine() == 1);
        // answered late: kept as decided, then gone (nobody waits to take it away)
        left(&b);
        assert!(deny(&config, Some("unit-left"), &b, "app"));
        assert!(cards().get(&b).is_none() && mine() == 0);
        assert_eq!(crate::approvals_log::list(&config, "unit-left", None)[0]["choice"], json!("deny"));
        // never answered: gone from the list after LEFT_TTL
        let c = open("unit-left", request("strava")).unwrap();
        left(&c);
        cards().get_mut(&c).unwrap().at -= LEFT_TTL;
        assert_eq!(mine(), 0);
        cards().retain(|_, c| c.agent != "unit-left");
    }

    fn config_in(dir: &tempfile::TempDir) -> Config {
        crate::config::parse_config(&json!({"bridge_key": "k".repeat(32), "data_dir": dir.path().to_string_lossy()}), None)
            .unwrap()
    }

    fn site_login(name: &str) -> Request {
        Request {
            name: name.into(),
            origin: "https://www.fournisseur.example".into(),
            kind: "password".into(),
            identifier_label: "Identifiant".into(),
            reason: "Pour tes commandes".into(),
            browser_login: true,
            ..Default::default()
        }
    }

    fn with_browser(config: &Config, agent: &str) {
        let folder = crate::browsers::folder(config, agent);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("ports.json"), r#"{"cdp": 9600, "vnc": 9601}"#).unwrap();
    }

    /// design-coffre-rassurer §4: `bip_secret_use` (browser | connector) and `bip_secret_self` (the person may type it
    /// in the agent's browser: a site's login, asked by the agent, a browser to take over). The card's other fields
    /// stay as an older app reads them.
    #[test]
    fn a_card_says_what_it_is_for_and_whether_the_person_may_type_it_themselves() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let login = open("unit-use", site_login("fournisseur")).unwrap();
        let connector =
            open("unit-use", Request { name: "garmin".into(), kind: "password".into(), ..Default::default() }).unwrap();
        let card = |id: &str| item(&config, id).unwrap();
        assert_eq!((card(&login)["bip_secret_use"].clone(), card(&login)["bip_secret_self"].clone()), (json!("browser"), json!(false)));
        with_browser(&config, "unit-use");
        assert_eq!(card(&login)["bip_secret_self"], json!(true));
        assert_eq!((card(&connector)["bip_secret_use"].clone(), card(&connector)["bip_secret_self"].clone()), (json!("connector"), json!(false)));
        // what an older app reads, unchanged
        let shown = card(&login);
        assert_eq!(shown["choices"], json!(["once", "deny"]));
        assert_eq!(shown["bip_secret_kind"], json!("password"));
        assert_eq!(shown["bip_secret_site"], json!("www.fournisseur.example"));
        // a token, a code, a card the person opened from the app: never
        let token = Request { kind: "token".into(), browser_login: false, ..site_login("ovh") };
        assert!(!may_type_self(&config, "unit-use", &token));
        assert!(!may_type_self(&config, "unit-use", &Request { edit: true, ..site_login("x1") }));
        assert!(may_type_self(&config, "unit-use", &site_login("x2")));
        assert!(!may_type_self(&config, "unit-other", &site_login("x3")), "no browser of its own");
        cards().retain(|_, c| c.agent != "unit-use");
    }

    #[test]
    fn the_person_types_it_themselves_only_where_they_can_take_the_browser() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        let connector =
            open("unit-self", Request { name: "garmin".into(), kind: "password".into(), ..Default::default() }).unwrap();
        let login = open("unit-self", site_login("fournisseur")).unwrap();
        assert_eq!(choose_self(&config, None, &login, false, "app"), Err(SelfError::NotHere), "no browser yet");
        with_browser(&config, "unit-self");
        assert_eq!(choose_self(&config, None, &connector, false, "app"), Err(SelfError::NotHere));
        assert_eq!(choose_self(&config, Some("someone-else"), &login, false, "app"), Err(SelfError::NoCard));
        assert_eq!(choose_self(&config, None, "nope", false, "app"), Err(SelfError::NoCard));
        assert_eq!(choose_self(&config, None, &login, false, "app"), Ok(()));
        assert_eq!(cards().get(&login).unwrap().answer, Some(Outcome::TypedByPerson));
        assert_eq!(choose_self(&config, None, &login, false, "app"), Err(SelfError::NoCard), "answered already");
        assert!(!deny(&config, None, &login, "app"));
        // kept as the person's choice, nothing in the vault
        let kept = crate::approvals_log::list(&config, "unit-self", None);
        assert_eq!(kept.last().unwrap()["choice"], json!(SELF));
        assert_eq!(kept.last().unwrap()["run_id"], json!(format!("{RUN_PREFIX}{login}")));
        // no longer waiting for the person
        assert!(pending_items(&config, Some("unit-self")).iter().all(|c| c["request_id"] != json!(login)));
        cards().retain(|_, c| c.agent != "unit-self");
    }

    #[tokio::test]
    async fn the_agents_call_learns_the_person_types_it() {
        let dir = tempfile::tempdir().unwrap();
        let config = config_in(&dir);
        with_browser(&config, "unit-wait");
        let id = open("unit-wait", site_login("fournisseur")).unwrap();
        let waiting = tokio::spawn({
            let id = id.clone();
            async move { wait(&id).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        choose_self(&config, None, &id, false, "app").unwrap();
        assert_eq!(waiting.await.unwrap(), Some(Outcome::TypedByPerson));
        assert!(cards().get(&id).is_none());
    }

    /// design §1.5: the hand given back, or not yet after 10 minutes.
    #[test]
    fn the_agent_reads_where_the_person_is() {
        assert_eq!(
            HANDED_BACK,
            "The person signed in themselves in your browser and handed it back. Read the page: signed in, go on; the \
             sign-in page still there, tell them in one sentence what you see. Nothing was kept in the vault: don't \
             ask for this site again in this conversation."
        );
        assert_eq!(
            NOT_BACK,
            "The person chose to sign in themselves in your browser and hasn't handed it back yet. Don't use your \
             browser: say you'll go on once they hand it back, then stop."
        );
        // told in a later turn, once the hand is back (card_place.rs)
        let late = resume_words("bip-secret-abc", SELF, "Vie a besoin de ton accès Fournisseur (fournisseur.example)").unwrap();
        assert!(late.starts_with("« Vie a besoin de ton accès Fournisseur (fournisseur.example) » — "), "{late}");
        assert!(late.contains("signed in themselves in your browser and handed it back"), "{late}");
        assert!(late.contains("don't ask for this site again"), "{late}");
        assert_eq!(resume_words("bip-secret-abc", "once", "x"), None);
        assert_eq!(resume_words("bip-site-abc", SELF, "x"), None);
        // the hand: taken, then given back; never « back » before it was taken
        assert_eq!(hand_now(false, false), Hand::Wait);
        assert_eq!(hand_now(true, false), Hand::Taken);
        assert_eq!(hand_now(true, true), Hand::Wait);
        assert_eq!(hand_now(false, true), Hand::Back);
        // Hermes waits for the call long enough: the card's answer, then the hand
        let waits = ASK_TIMEOUT + HAND_WAIT + Duration::from_secs(30);
        assert!(u64::from(crate::connectors::TOOL_TIMEOUT) >= waits.as_secs(), "connectors.rs TOOL_TIMEOUT");
        assert_eq!(hand_wait(), HAND_WAIT);
        // the tool says where to ask from
        let tools = crate::connectors::tools();
        let ask = tools.iter().find(|t| t["name"] == "ask_secret").unwrap();
        let use_ = ask["inputSchema"]["properties"]["use"]["description"].as_str().unwrap();
        assert!(use_.contains("from the site's login page") && use_.contains("sign in themselves"), "{use_}");
    }

    /// Review of 17/11: a `self` answer is closed when the hand comes back (told once), when nobody took the browser
    /// by the time the agent stopped waiting, and `SELF_KEEP` after it at most: a take days later resumes nothing.
    #[test]
    fn a_self_answer_never_waits_for_days() {
        let gave_up = HAND_WAIT.as_secs_f64() + 61.0;
        let kept = SELF_KEEP.as_secs_f64() + 1.0;
        assert_eq!(hand_after(false, false, 10.0), Hand::Wait);
        assert_eq!(hand_after(true, false, 10.0), Hand::Taken);
        assert_eq!(hand_after(true, true, 10.0), Hand::Wait);
        assert_eq!(hand_after(false, true, 10.0), Hand::Back);
        // nobody took it while the agent waited: closed, a take afterwards is another matter
        assert_eq!(hand_after(false, false, gave_up), Hand::Closed);
        assert_eq!(hand_after(true, false, gave_up), Hand::Closed);
        // taken in time: back later goes on, never past SELF_KEEP
        assert_eq!(hand_after(true, true, gave_up), Hand::Wait);
        assert_eq!(hand_after(false, true, gave_up), Hand::Back);
        assert_eq!(hand_after(true, true, kept), Hand::Closed);
        assert_eq!(hand_after(false, true, kept), Hand::Closed);
        assert!(SELF_KEEP > HAND_WAIT && SELF_KEEP.as_secs() <= 6 * 3600);
    }

    /// A take and a hand-back between two looks of a wait (a second apart) still count.
    #[test]
    fn a_hand_given_back_is_remembered() {
        let before = py::time_time() - 1.0;
        assert!(!given_back_since("unit-hand", before));
        hand_given_back("unit-hand");
        assert!(given_back_since("unit-hand", before));
        assert!(!given_back_since("unit-hand", py::time_time() + 5.0), "only since the answer");
        assert!(!given_back_since("unit-other", before));
    }

    #[test]
    fn sentences() {
        let r = Request {
            name: "garmin".into(),
            origin: "https://connect.garmin.com".into(),
            kind: "password".into(),
            ..Default::default()
        };
        assert_eq!(sentence("Vie", &r), "Vie a besoin de ton accès Garmin (connect.garmin.com)");
        assert_eq!(sentence_in("fr", "Vie", &r), sentence("Vie", &r));
        assert_eq!(sentence_in("en", "Vie", &r), "Vie needs your access to Garmin (connect.garmin.com)");
        assert_eq!(sentence_in("de", "Vie", &r), "Vie braucht deinen Zugang zu Garmin (connect.garmin.com)");
        let code = Request { name: "bank".into(), kind: "code".into(), ..Default::default() };
        assert_eq!(sentence_in("es", "Vie", &code), "Vie necesita el código recibido para Bank");
    }
}
