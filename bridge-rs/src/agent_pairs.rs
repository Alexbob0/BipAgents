//! Two agents' own conversation, « A ⇄ B » (Alex, 2026-10-29: « Les échanges entre 2 bots dans une conversation à
//! part, qui s'ouvre quand on clique dessus, au lieu de l'avoir dans notre conversation au milieu »).
//!
//! Until then an agent asking another one (`message_agent`, agent_messages.rs) wrote into the target's « Bot Chat »,
//! the person's main conversation with it: long requests and answers between agents in the middle of their own
//! messages. Now each pair of agents of this install has one conversation, kept over time, whoever asks:
//!
//! - **The target answers in a conversation of its own for that pair** (a hidden Hermes session titled
//!   `SESSION_TITLE <pair id>`, one per agent of the pair, made the first time it is asked), with its memory and its
//!   tools as usual. The ask is signed as Hermes signs a peer's message (« Message from Quotidien (@quotidien): … »,
//!   peers.rs `SIGNATURE`) after a short note for the agent (`<!--bip:pair …-->`, hidden by the apps). Its turn is
//!   a run the run hub follows (runs.rs): an approval or a question nobody follows is pushed and waits in the Boîte,
//!   which shows inside the person's main conversation with that agent, as for the rooms. The asker gets the answer in
//!   the same call; a turn waiting for the person past `agent_messages::ANSWER_SECONDS` goes on, and its answer is
//!   kept here (the asker is told so).
//! - **The bridge keeps the pair's thread** (`<data>/agent-pairs.json`, 0600): the asks and the answers, with who and
//!   when, the last `MESSAGES_KEPT`; the person reads it in the app, read-only, from a compact line in the main
//!   conversation of either agent (« Pub & Acquisition ⇄ Bot Kerfy · 3 messages · Voir l'échange »), placed at the
//!   time of each `exchange` (messages less than `EXCHANGE_GAP` apart), and from both agents' « Historique ».
//! - Only this install's agents, along the links of « Qui peut écrire à qui » (checked by agent_messages.rs). Another
//!   household's agents never get here (family_news.rs stays their way), a relative's turn has no `message_agent`
//!   (agent_shares.rs `RELATIVE_TOOLS`), and these routes are the person's app's only: a relative's bridge never
//!   lists them.
//!
//!     GET /v1/agent-pairs?agent=<id>   {pairs: [pair]}   the pairs this agent is in, the most recent first
//!     GET /v1/agent-pairs/{id}         pair + {thread: [message]}   the last MESSAGES_KEPT, oldest first
//!
//!     pair      {id, agents: [{id, name, category, color, gone}] (sorted by id), messages: <count, all time>,
//!                created, updated, last: message|null, exchanges: [{id, at, last, from, count}] (the last 50)}
//!     message   {id, at, from, to, kind: "ask"|"answer"|"notice", text, exchange}
//!
//! No Python counterpart.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, State};
use axum::response::Response;
use axum::routing::get as get_route;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::hermes::{HermesClient, HermesError};
use crate::runs::RunHub;
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError, QueryMap};
use crate::{py, Dict};

/// The hidden Hermes conversation where an agent answers the other one of a pair: `<this> <pair id>` (Hermes wants
/// titles unique per agent).
pub const SESSION_TITLE: &str = "BipAgents · ⇄";
/// Messages kept per pair (the count shown stays the all-time one).
pub const MESSAGES_KEPT: usize = 400;
/// Exchanges kept per pair (the lines in the main conversations).
pub const EXCHANGES_KEPT: usize = 200;
/// Exchanges given in a pair's summary.
pub const EXCHANGES_LISTED: usize = 50;
/// Messages closer than this (seconds) belong to one exchange: one line in the main conversation.
pub const EXCHANGE_GAP: f64 = 1800.0;
/// A turn waiting for the person's approval (or answer) is followed this long.
pub const APPROVAL_WAIT: f64 = 1800.0;
/// How often the target's run is asked about.
const POLL_SECONDS: f64 = 1.0;
/// An answer kept, at most (characters).
pub const TEXT_MAX: usize = 16000;

static LOCK: Mutex<()> = Mutex::new(());
/// One turn at a time per pair (two asks at once would meet in the same Hermes conversation).
static TURNS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: u64,
    pub at: f64,
    pub from: String,
    pub to: String,
    /// "ask", "answer", or "notice" (the target couldn't answer: why, in English, for the app to word).
    pub kind: String,
    pub text: String,
    pub exchange: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Exchange {
    pub id: u64,
    pub at: f64,
    pub last: f64,
    /// Who asked first.
    pub from: String,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pair {
    pub id: String,
    /// The two agents, sorted.
    pub agents: Vec<String>,
    /// Each agent's hidden conversation for this pair (where it answers).
    #[serde(default)]
    pub sessions: BTreeMap<String, String>,
    #[serde(default)]
    pub messages: Vec<Message>,
    #[serde(default)]
    pub exchanges: Vec<Exchange>,
    #[serde(default)]
    pub created: f64,
    #[serde(default)]
    pub updated: f64,
    /// Messages ever written (the shown count) and the last id given.
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub last_id: u64,
}

impl Pair {
    pub fn has(&self, agent: &str) -> bool {
        self.agents.iter().any(|a| a == agent)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Book {
    #[serde(default)]
    pairs: BTreeMap<String, Pair>,
}

/// The pair's id: the same whoever asks (`p` and 12 hex digits).
pub fn pair_id(a: &str, b: &str) -> String {
    let (first, second) = if a <= b { (a, b) } else { (b, a) };
    let digest = hex::encode(Sha256::digest(format!("{first}\n{second}").as_bytes()));
    format!("p{}", &digest[..12])
}

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-pairs.json")
}

fn load_book(config: &Config) -> Book {
    std::fs::read(path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_book(config: &Config, book: &Book) -> anyhow::Result<()> {
    let file = path(config);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = file.with_extension("tmp");
    py::write_with_mode(&temporary.to_string_lossy(), &serde_json::to_vec(book)?, 0o600)?;
    std::fs::rename(&temporary, &file)?;
    Ok(())
}

fn change<T>(config: &Config, edit: impl FnOnce(&mut Book) -> T) -> anyhow::Result<T> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut book = load_book(config);
    let out = edit(&mut book);
    save_book(config, &book)?;
    Ok(out)
}

pub fn get(config: &Config, id: &str) -> Option<Pair> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_book(config).pairs.remove(id)
}

/// The pairs `agent` is in, the most recently active first.
pub fn of_agent(config: &Config, agent: &str) -> Vec<Pair> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut pairs: Vec<Pair> = load_book(config).pairs.into_values().filter(|p| p.has(agent)).collect();
    pairs.sort_by(|a, b| b.updated.partial_cmp(&a.updated).unwrap_or(std::cmp::Ordering::Equal));
    pairs
}

/// Adds a message to the pair of `from` and `to` (made the first time) at `at`: its id and its exchange's.
pub fn record(config: &Config, from: &str, to: &str, kind: &str, text: &str, at: f64) -> anyhow::Result<(u64, u64)> {
    let id = pair_id(from, to);
    change(config, |book| {
        let pair = book.pairs.entry(id.clone()).or_insert_with(|| {
            let mut agents = vec![from.to_string(), to.to_string()];
            agents.sort();
            Pair { id: id.clone(), agents, created: at, ..Default::default() }
        });
        add(pair, from, to, kind, text, at)
    })
}

/// `record` on a loaded pair.
pub fn add(pair: &mut Pair, from: &str, to: &str, kind: &str, text: &str, at: f64) -> (u64, u64) {
    pair.last_id += 1;
    let message_id = pair.last_id;
    let joins = pair.exchanges.last().is_some_and(|e| at - e.last <= EXCHANGE_GAP);
    let exchange = if joins {
        let last = pair.exchanges.last_mut().expect("checked");
        last.count += 1;
        last.last = at;
        last.id
    } else {
        pair.exchanges.push(Exchange { id: message_id, at, last: at, from: from.to_string(), count: 1 });
        message_id
    };
    let text = tk::head(tk::strip(text), TEXT_MAX).to_string();
    pair.messages.push(Message {
        id: message_id,
        at,
        from: from.to_string(),
        to: to.to_string(),
        kind: kind.to_string(),
        text,
        exchange,
    });
    if pair.messages.len() > MESSAGES_KEPT {
        let extra = pair.messages.len() - MESSAGES_KEPT;
        pair.messages.drain(..extra);
    }
    if pair.exchanges.len() > EXCHANGES_KEPT {
        let extra = pair.exchanges.len() - EXCHANGES_KEPT;
        pair.exchanges.drain(..extra);
    }
    pair.total += 1;
    pair.updated = at;
    (message_id, exchange)
}

/// The note the target reads above the ask (hidden by the apps), then the ask signed as a peer's.
pub fn framed(asker: &AgentConfig, message: &str) -> String {
    let name = tk::in_note_line(crate::agent_files::shown(asker));
    format!(
        "<!--bip:pair\nThis is your own conversation with {name} (@{}), another agent of your person: your person \
doesn't read it. Answer {name} here, in text. When you need your person (a decision, something only they know), say \
so in your answer: {name} asks them. Actions that need your person's approval are asked as usual.\n-->\n{}",
        asker.name,
        crate::agent_messages::signed(asker, message)
    )
}

/// What the ask gave back.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The target's answer.
    Text(String),
    /// It answered nothing.
    Empty,
    /// Its turn waits for the person (an approval, a question) past the call's time: its answer will be kept here.
    Waiting,
    /// It couldn't be reached or its turn failed.
    Unreachable,
}

fn turn_lock(id: &str) -> Arc<tokio::sync::Mutex<()>> {
    TURNS.lock().unwrap_or_else(|e| e.into_inner()).entry(id.to_string()).or_default().clone()
}

/// `asker` asks `target` in their pair's conversation: the target's answer within `wait` seconds (its turn goes on
/// past them when it waits for the person, and its answer is kept in the pair's thread).
pub async fn ask(
    http: &reqwest::Client,
    hub: Option<Arc<RunHub>>,
    config: &Config,
    asker: &AgentConfig,
    target: &AgentConfig,
    message: &str,
    wait: f64,
) -> Answer {
    let id = pair_id(&asker.name, &target.name);
    if let Err(exc) = record(config, &asker.name, &target.name, "ask", message, py::time_time()) {
        tracing::warn!(agent = %asker.name, target = %target.name, error = %exc, "agent pair: the ask not kept");
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<Answer>();
    let (http, config, asker, target, message) =
        (http.clone(), config.clone(), asker.clone(), target.clone(), message.to_string());
    tokio::spawn(async move {
        let lock = turn_lock(&id);
        let _turn = lock.lock().await;
        let answer = turn(&http, hub, &config, &id, &asker, &target, &message, wait).await;
        let at = py::time_time();
        let kept = match &answer {
            Answer::Text(text) => record(&config, &target.name, &asker.name, "answer", text, at),
            Answer::Empty => record(&config, &target.name, &asker.name, "notice", "answered nothing", at),
            Answer::Unreachable => record(&config, &target.name, &asker.name, "notice", "couldn't answer", at),
            Answer::Waiting => Ok((0, 0)),
        };
        if let Err(exc) = kept {
            tracing::warn!(agent = %asker.name, target = %target.name, error = %exc, "agent pair: the answer not kept");
        }
        let _ = tx.send(answer);
    });
    // a turn waiting for the person goes on past `wait` (the task keeps its answer): the asker is told so
    match tokio::time::timeout(Duration::from_secs_f64(wait + 5.0), rx).await {
        Ok(Ok(answer)) => answer,
        _ => Answer::Waiting,
    }
}

/// The target's hidden conversation for this pair (made, and hidden, the first time).
async fn session_for(hermes: &HermesClient, config: &Config, id: &str, target: &AgentConfig) -> Result<String, HermesError> {
    if let Some(session) = get(config, id).and_then(|p| p.sessions.get(&target.name).cloned()) {
        return Ok(session);
    }
    let title = format!("{SESSION_TITLE} {id}");
    let session = hermes.create_session(target, &title).await?;
    let mut fields = serde_json::Map::new();
    fields.insert("title".into(), json!(title));
    fields.insert("hidden".into(), Value::Bool(true));
    if hermes.update_session(target, &session, fields).await.is_err() {
        tracing::info!(agent = %target.name, pair = %id, "agent pair: the conversation could not be hidden");
    }
    let _ = change(config, |book| {
        if let Some(pair) = book.pairs.get_mut(id) {
            pair.sessions.insert(target.name.clone(), session.clone());
        }
    });
    Ok(session)
}

fn forget_session(config: &Config, id: &str, agent: &str) {
    let _ = change(config, |book| {
        if let Some(pair) = book.pairs.get_mut(id) {
            pair.sessions.remove(agent);
        }
    });
}

/// One turn of `target`, followed until it ends: stopped after `wait` seconds, or `APPROVAL_WAIT` while it waits for
/// the person.
#[allow(clippy::too_many_arguments)]
async fn turn(
    http: &reqwest::Client,
    hub: Option<Arc<RunHub>>,
    config: &Config,
    id: &str,
    asker: &AgentConfig,
    target: &AgentConfig,
    message: &str,
    wait: f64,
) -> Answer {
    let hermes = HermesClient::new(http.clone());
    let input = crate::dates::now::stamped(config, target, &framed(asker, message));
    let mut started = None;
    for _ in 0..2 {
        let session = match session_for(&hermes, config, id, target).await {
            Ok(session) => session,
            Err(exc) => {
                tracing::warn!(agent = %asker.name, target = %target.name, error = %exc, "agent pair: no conversation");
                return Answer::Unreachable;
            }
        };
        match hermes.start_run(target, &input, &session, None).await {
            Ok(run_id) => {
                started = Some((run_id, session));
                break;
            }
            // the conversation is gone (deleted from Hermes): a new one
            Err(HermesError::Http { status: 404, .. }) => forget_session(config, id, &target.name),
            Err(exc) => {
                tracing::warn!(agent = %asker.name, target = %target.name, error = %exc, "agent pair: no turn");
                return Answer::Unreachable;
            }
        }
    }
    let Some((run_id, session)) = started else { return Answer::Unreachable };
    if let Some(hub) = &hub {
        // approvals and questions nobody follows are pushed (the Boîte); the answer goes back to the asker
        hub.watch(target, &run_id, Some(&session), false);
    }
    let since = py::time_time();
    loop {
        tokio::time::sleep(Duration::from_secs_f64(POLL_SECONDS)).await;
        match hermes.run(target, &run_id).await {
            Ok(run) => {
                let status = tk::str_or_empty(run.get("status"));
                if crate::jobs::TERMINAL.contains(&status.as_str()) {
                    if status != "completed" {
                        return Answer::Unreachable;
                    }
                    let output = run.get("output").and_then(Value::as_str).unwrap_or("");
                    return answer_of(output);
                }
            }
            Err(HermesError::Http { status: 404 | 410, .. }) => {
                // Hermes restarted, or forgot the run: its last reply in the conversation
                let output = match hermes.session_messages(target, &session).await {
                    Ok(messages) => crate::cronwatch::final_reply(&messages).unwrap_or_default(),
                    Err(_) => String::new(),
                };
                return answer_of(&output);
            }
            Err(_) => {} // Hermes restarting: asked again next round
        }
        let waiting = hub.as_ref().is_some_and(|hub| {
            hub.pending_approvals(Some(&target.name))
                .iter()
                .any(|a| a.get("run_id").and_then(Value::as_str) == Some(run_id.as_str()))
        });
        let elapsed = py::time_time() - since;
        let limit = if waiting { APPROVAL_WAIT.max(wait) } else { wait };
        if elapsed >= limit {
            let _ = hermes.stop_run(target, &run_id).await;
            return Answer::Unreachable;
        }
    }
}

fn answer_of(output: &str) -> Answer {
    let text = tk::strip(output);
    if text.is_empty() {
        Answer::Empty
    } else {
        Answer::Text(text.to_string())
    }
}

// -- what the app reads ----------------------------------------------------------------------------------------------

fn agent_view(config: &Config, id: &str) -> Value {
    match config.agent(Some(id)) {
        Some(agent) => json!({"id": agent.name, "name": agent.display_name, "category": agent.category,
                              "color": agent.color, "gone": false}),
        None => json!({"id": id, "name": id, "category": null, "color": null, "gone": true}),
    }
}

pub fn pair_view(config: &Config, pair: &Pair) -> Dict {
    let exchanges = &pair.exchanges[pair.exchanges.len().saturating_sub(EXCHANGES_LISTED)..];
    let view = json!({
        "id": pair.id,
        "agents": pair.agents.iter().map(|a| agent_view(config, a)).collect::<Vec<_>>(),
        "messages": pair.total, "created": pair.created, "updated": pair.updated,
        "last": pair.messages.last(), "exchanges": exchanges,
    });
    match view {
        Value::Object(map) => map,
        _ => Dict::new(),
    }
}

async fn listing(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let config = state.config();
    let agent = config.agent(Some(&agent)).ok_or_else(HttpError::not_found)?.name.clone();
    let pairs: Vec<Value> = of_agent(&config, &agent).iter().map(|p| Value::Object(pair_view(&config, p))).collect();
    Ok(web::ok(json!({"pairs": pairs})))
}

async fn thread(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let pair = get(&config, &id).ok_or_else(HttpError::not_found)?;
    let mut view = pair_view(&config, &pair);
    view.insert("thread".into(), json!(pair.messages));
    Ok(web::ok(Value::Object(view)))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/agent-pairs", get_route(listing)).route("/agent-pairs/{pair_id}", get_route(thread))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pair_is_the_same_whoever_asks() {
        assert_eq!(pair_id("quotidien", "coach"), pair_id("coach", "quotidien"));
        assert_ne!(pair_id("quotidien", "coach"), pair_id("quotidien", "budget"));
        let id = pair_id("a", "b");
        assert!(id.starts_with('p') && id.len() == 13);
    }

    #[test]
    fn close_messages_make_one_exchange_and_the_thread_is_capped() {
        let mut pair = Pair { id: pair_id("a", "b"), agents: vec!["a".into(), "b".into()], ..Default::default() };
        assert_eq!(add(&mut pair, "a", "b", "ask", "Combien ?", 1000.0), (1, 1));
        assert_eq!(add(&mut pair, "b", "a", "answer", "Sept.", 1010.0), (2, 1));
        assert_eq!(add(&mut pair, "a", "b", "ask", "Et hier ?", 1020.0), (3, 1));
        assert_eq!(pair.exchanges, vec![Exchange { id: 1, at: 1000.0, last: 1020.0, from: "a".into(), count: 3 }]);
        // later: a new exchange, started by the other one
        assert_eq!(add(&mut pair, "b", "a", "ask", "Tu as le menu ?", 1020.0 + EXCHANGE_GAP + 1.0), (4, 4));
        assert_eq!(pair.exchanges.len(), 2);
        assert_eq!(pair.exchanges[1].from, "b");
        for n in 0..MESSAGES_KEPT + 5 {
            add(&mut pair, "a", "b", "ask", "x", 5000.0 + n as f64);
        }
        assert_eq!(pair.messages.len(), MESSAGES_KEPT);
        assert_eq!(pair.total, MESSAGES_KEPT + 9);
        assert_eq!(pair.updated, 5000.0 + (MESSAGES_KEPT + 4) as f64);
    }

    #[test]
    fn the_target_reads_a_note_then_the_signed_ask() {
        let asker = AgentConfig { name: "quotidien".into(), display_name: "Quotidien --> x".into(), ..Default::default() };
        let text = framed(&asker, " Combien d'heures ? ");
        assert!(text.starts_with("<!--bip:pair\nThis is your own conversation with Quotidien \u{2011}\u{2011}> x (@quotidien)"));
        let (note, ask) = text.split_once("\n-->\n").unwrap();
        assert!(!note.contains("-->"));
        assert_eq!(ask, "Message from Quotidien --> x (@quotidien): Combien d'heures ?");
        assert_eq!(&crate::peers::SIGNATURE.captures(ask).unwrap()[1], "quotidien");
    }
}
