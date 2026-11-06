//! The hop cap between agents (Hermes 0.23 study §4.4-3, Alex's yes 6/11): agents writing to each other with
//! `message_agent` (agent_messages.rs) stop after `MAX_HOPS` messages since their person last spoke, and the person
//! decides whether they go on.
//!
//! Upstream Hermes caps bot-to-bot hops at 4 since 0.23 (`gateway.bot_loop_guard.max_hops`, PR #148903, after two
//! gateways relaunched each other for 17 h). Our `message_agent` is the bridge's own (Hermes patch 0016 leaves Hermes'
//! out), so upstream's guard never sees it: this module is ours.
//!
//! - **A chain** is everything agents write to each other from one turn nobody of them started: the person's message
//!   (any conversation), a room's turn, a routine's, a background task the person asked for. Its id is that turn's run
//!   (the run the hub follows, runs.rs `turn_of`); a turn the hub doesn't follow counts per agent (`untracked`), until
//!   it stays quiet `UNTRACKED_IDLE` seconds.
//! - **A hop** is one message from an agent to another one (one `message_agent` call; the answer comes back in that
//!   call). The turn the target answers in is a run started for the chain (agent_pairs.rs): its own messages are hops
//!   of the same chain, so A → B → A → B… counts up, as do A asking B again and again in one turn. A background task an
//!   agent of a chain starts (jobs.rs `start_task`) carries the chain on.
//! - **At the cap** the call is refused at once, with a clear error for the agent (« Limite d'échanges entre agents
//!   atteinte… »: answer your person, or ask them whether to go on), and the person gets ONE card in the conversation
//!   the chain started from (a bridge card, `bip-hops-<id>`: « Quotidien et Coach ont échangé 4 messages entre eux.
//!   Continuer l'échange ? »). « Oui » resets the chain (its turns still going may write again) and the agent goes on
//!   (card_place.rs `settle`: a turn of its own with the answer, a new chain); « Non » leaves it there.
//!
//! The chains live in memory (a restart ends their turns too); the cards in `<data>/agent-hops.json` (0600), the last
//! `CARDS_KEPT`, until answered or `CARD_DAYS` old.
//!
//! No Python counterpart.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::{py, Dict};

/// Messages between agents per chain (upstream's `max_hops`).
pub const MAX_HOPS: u32 = 4;
/// The person's card (`/v1/approvals`, `/v1/approve`).
pub const RUN_PREFIX: &str = "bip-hops-";
/// A turn the hub doesn't follow: its agent's messages count together until it is quiet this long (seconds).
pub const UNTRACKED_IDLE: f64 = 600.0;
/// A chain is forgotten after this long without a message (seconds).
const CHAIN_KEEP: f64 = 6.0 * 3600.0;
/// Cards kept, at most, and for how long.
const CARDS_KEPT: usize = 50;
const CARD_DAYS: f64 = 3.0;
/// A chain's card the person answered « Non »: no other card for it (its agents are told the cap, nothing more).
const DENIED: &str = "denied";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chain {
    pub id: String,
    /// Messages between agents since the person last spoke (or tapped « Continuer »).
    pub hops: u32,
    /// The agent whose turn started it, and that turn (None: one the hub doesn't follow).
    pub root_agent: String,
    pub root_run: Option<String>,
    /// The agents that wrote or were written to, in order (the card names them).
    pub agents: Vec<String>,
    /// Its card waiting for the person, if any.
    pub card: Option<String>,
    pub last: f64,
}

#[derive(Default)]
struct Book {
    chains: HashMap<String, Chain>,
    /// Runs started for a chain (a target's answering turn, a task) -> the chain.
    runs: HashMap<String, String>,
}

static BOOK: LazyLock<Mutex<Book>> = LazyLock::new(|| Mutex::new(Book::default()));

fn book() -> std::sync::MutexGuard<'static, Book> {
    BOOK.lock().unwrap_or_else(|e| e.into_inner())
}

/// The chain id of a turn: the chain a run was started for, else the run itself (a root), else the agent's untracked
/// one.
fn key_of(book: &Book, agent: &str, run: Option<&str>) -> String {
    match run {
        Some(run) => book.runs.get(run).cloned().unwrap_or_else(|| run.to_string()),
        None => untracked(agent),
    }
}

fn untracked(agent: &str) -> String {
    format!("agent:{agent}")
}

/// What `step` decided.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The message may go: its chain, and its hop (1 for the first).
    Go { chain: String, hop: u32 },
    /// The cap is reached: the chain as it is (its card made by the caller when it has none yet).
    Capped(Chain),
}

/// `agent`, in its turn `run` (None: one the hub doesn't follow), writes to `target` at `now`: counted, or refused at
/// the cap.
pub fn step(agent: &str, run: Option<&str>, target: &str, now: f64) -> Step {
    let mut book = book();
    book.chains.retain(|_, c| now - c.last < CHAIN_KEEP);
    let live: std::collections::HashSet<String> = book.chains.keys().cloned().collect();
    book.runs.retain(|_, chain| live.contains(chain));
    let id = key_of(&book, agent, run);
    let chain = book.chains.entry(id.clone()).or_insert_with(|| Chain {
        id: id.clone(),
        root_agent: agent.to_string(),
        root_run: run.map(str::to_string),
        last: now,
        ..Chain::default()
    });
    if run.is_none() && chain.root_run.is_none() && now - chain.last >= UNTRACKED_IDLE {
        // an untracked turn: quiet long enough, the person spoke in between
        chain.hops = 0;
        chain.agents.clear();
    }
    chain.last = now;
    if chain.hops >= MAX_HOPS {
        return Step::Capped(chain.clone());
    }
    chain.hops += 1;
    for name in [agent, target] {
        if !chain.agents.iter().any(|a| a == name) {
            chain.agents.push(name.to_string());
        }
    }
    Step::Go { chain: id, hop: chain.hops }
}

/// Run `run_id` was started for chain `chain` (the target's answering turn, a task started in the chain).
pub fn follow(run_id: &str, chain: &str) {
    let mut book = book();
    if book.chains.contains_key(chain) {
        book.runs.insert(run_id.to_string(), chain.to_string());
    }
}

/// Run `run_id` is over: what it writes next can't be its chain's any more.
pub fn forget_run(run_id: &str) {
    book().runs.remove(run_id);
}

/// The chain the turn `run` of `agent` belongs to, when it already wrote to an agent or was started for one (a task
/// started there carries it on).
pub fn chain_of_turn(agent: &str, run: Option<&str>) -> Option<String> {
    let book = book();
    let id = key_of(&book, agent, run);
    book.chains.get(&id).filter(|c| c.hops > 0).map(|c| c.id.clone())
}

pub fn chain(id: &str) -> Option<Chain> {
    book().chains.get(id).cloned()
}

/// The person said go on: the chain counts from zero again.
pub fn reset(id: &str) {
    if let Some(chain) = book().chains.get_mut(id) {
        chain.hops = 0;
        chain.card = None;
    }
}

fn set_card(id: &str, card: &str) {
    if let Some(chain) = book().chains.get_mut(id) {
        chain.card = Some(card.to_string());
    }
}

// -- the person's card -----------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: String,
    /// The agent whose conversation shows it (the chain's root).
    pub agent: String,
    pub chain: String,
    pub agents: Vec<String>,
    pub hops: u32,
    pub at: f64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cards {
    #[serde(default)]
    cards: Vec<Card>,
}

static FILE_LOCK: Mutex<()> = Mutex::new(());

fn path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-hops.json")
}

fn load(config: &Config) -> Cards {
    std::fs::read(path(config)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn change<T>(config: &Config, edit: impl FnOnce(&mut Cards) -> T) -> T {
    let _held = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cards = load(config);
    let now = py::time_time();
    cards.cards.retain(|c| now - c.at < CARD_DAYS * 86400.0);
    let out = edit(&mut cards);
    if cards.cards.len() > CARDS_KEPT {
        let extra = cards.cards.len() - CARDS_KEPT;
        cards.cards.drain(..extra);
    }
    let file = path(config);
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(body) = serde_json::to_vec(&cards) {
        let tmp = file.with_extension("part");
        if py::write_with_mode(&tmp.to_string_lossy(), &body, 0o600).is_ok() {
            let _ = std::fs::rename(&tmp, &file);
        }
    }
    out
}

/// The card for a chain at its cap: made once (None: it has one waiting already). Its run id.
pub fn card_for(config: &Config, chain: &Chain) -> Option<String> {
    if chain.card.as_deref().is_some_and(|c| c == DENIED || waiting(config, c)) {
        return None;
    }
    let id = py::uuid4_hex()[..16].to_string();
    let card = Card {
        id: id.clone(),
        agent: chain.root_agent.clone(),
        chain: chain.id.clone(),
        agents: chain.agents.clone(),
        hops: chain.hops,
        at: py::time_time(),
    };
    change(config, |cards| cards.cards.push(card));
    let run_id = format!("{RUN_PREFIX}{id}");
    set_card(&chain.id, &run_id);
    Some(run_id)
}

fn waiting(config: &Config, run_id: &str) -> bool {
    let id = run_id.strip_prefix(RUN_PREFIX).unwrap_or(run_id);
    load(config).cards.iter().any(|c| c.id == id)
}

fn names(config: &Config, agents: &[String]) -> Vec<String> {
    agents.iter().map(|a| crate::mail_tidy::shown_agent(config, a)).collect()
}

/// « Quotidien et Coach », « Quotidien, Coach et Budget ».
fn joined(lang: &str, names: &[String]) -> String {
    let and = match lang {
        "fr" => "et",
        "de" => "und",
        "es" => "y",
        _ => "and",
    };
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., last] => format!("{} {and} {last}", head.join(", ")),
    }
}

/// The card's sentence, in the agent's language.
pub fn sentence(lang: &str, names: &[String], hops: u32) -> String {
    let who = joined(lang, names);
    match lang {
        "fr" => format!("{who} ont échangé {hops} messages entre eux sans toi. Continuer l'échange ?"),
        "de" => format!("{who} haben {hops} Nachrichten untereinander ausgetauscht. Weitermachen lassen?"),
        "es" => format!("{who} han intercambiado {hops} mensajes entre ellos. ¿Seguir con el intercambio?"),
        _ => format!("{who} have exchanged {hops} messages between themselves. Let them go on?"),
    }
}

pub fn card_item(config: &Config, card: &Card) -> Dict {
    let lang = crate::mail_tidy::language(config, &card.agent);
    let summary = sentence(&lang, &names(config, &card.agents), card.hops);
    let item = json!({
        "agent": card.agent, "run_id": format!("{RUN_PREFIX}{}", card.id), "request_id": card.id,
        "command": format!("message_agent · {} hops · {}", card.hops, card.agents.join(" ⇄ ")),
        "description": summary, "choices": ["once", "deny"], "created_at": card.at, "session_id": null,
        "bip_summary": summary,
        "bip_hops": {"agents": card.agents, "hops": card.hops, "max": MAX_HOPS},
    });
    match item {
        Value::Object(map) => map,
        _ => Dict::new(),
    }
}

pub fn pending_items(config: &Config, agent: Option<&str>) -> Vec<Dict> {
    let mut cards: Vec<Card> = load(config).cards.into_iter().filter(|c| agent.is_none_or(|a| a == c.agent)).collect();
    cards.sort_by(|a, b| a.at.total_cmp(&b.at));
    cards.iter().map(|c| card_item(config, c)).collect()
}

/// The person answered card `card_id`: « Oui » resets its chain (« Continuer »), « Non » leaves it at its cap. None:
/// no such card.
pub fn answer(config: &Config, agent: &str, card_id: &str, allow: bool) -> Option<bool> {
    let card = change(config, |cards| {
        let i = cards.cards.iter().position(|c| c.agent == agent && c.id == card_id)?;
        Some(cards.cards.remove(i))
    })?;
    if allow {
        reset(&card.chain);
    } else if let Some(chain) = book().chains.get_mut(&card.chain) {
        chain.card = Some(DENIED.to_string());
    }
    tracing::info!(agent, card = card_id, allow, "agent hops card answered");
    Some(allow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh(tag: &str) -> (String, String) {
        (format!("a-{tag}-{}", py::uuid4_hex()), format!("b-{tag}-{}", py::uuid4_hex()))
    }

    #[test]
    fn a_ping_pong_stops_at_the_cap_and_continuer_resets_it() {
        let (a, b) = fresh("pp");
        let root = format!("run-{}", py::uuid4_hex());
        // A's turn asks B (hop 1); B answers in a run started for the chain and writes back to A (hop 2)…
        let Step::Go { chain, hop } = step(&a, Some(&root), &b, 100.0) else { panic!("refused") };
        assert_eq!((chain.as_str(), hop), (root.as_str(), 1));
        follow("run-b-1", &chain);
        assert_eq!(step(&b, Some("run-b-1"), &a, 101.0), Step::Go { chain: root.clone(), hop: 2 });
        follow("run-a-2", &chain);
        assert_eq!(step(&a, Some("run-a-2"), &b, 102.0), Step::Go { chain: root.clone(), hop: 3 });
        assert_eq!(step(&a, Some(&root), &b, 103.0), Step::Go { chain: root.clone(), hop: 4 });
        // the fifth: refused, from any turn of the chain
        let Step::Capped(capped) = step(&b, Some("run-b-1"), &a, 104.0) else { panic!("not capped") };
        assert_eq!((capped.hops, capped.root_agent.as_str(), capped.root_run.as_deref()), (MAX_HOPS, a.as_str(), Some(root.as_str())));
        assert_eq!(capped.agents, vec![a.clone(), b.clone()]);
        assert!(matches!(step(&a, Some(&root), &b, 105.0), Step::Capped(_)));
        // another turn of the person: a chain of its own
        assert!(matches!(step(&a, Some("another-root"), &b, 106.0), Step::Go { hop: 1, .. }));
        // « Continuer »
        reset(&root);
        assert_eq!(step(&b, Some("run-b-1"), &a, 107.0), Step::Go { chain: root.clone(), hop: 1 });
        // a run over is its chain's no more
        forget_run("run-b-1");
        assert!(matches!(step(&b, Some("run-b-1"), &a, 108.0), Step::Go { hop: 1, .. }));
    }

    #[test]
    fn an_untracked_turn_counts_per_agent_until_it_is_quiet() {
        let (a, b) = fresh("un");
        for hop in 1..=MAX_HOPS {
            assert!(matches!(step(&a, None, &b, 10.0 + hop as f64), Step::Go { hop: h, .. } if h == hop));
        }
        assert!(matches!(step(&a, None, &b, 20.0), Step::Capped(_)));
        assert!(matches!(step(&a, None, &b, 20.0 + UNTRACKED_IDLE), Step::Go { hop: 1, .. }));
        assert_eq!(chain_of_turn(&a, None), Some(untracked(&a)));
        assert_eq!(chain_of_turn(&b, None), None);
    }

    #[test]
    fn the_card_names_the_agents_in_the_persons_language() {
        let names = vec!["Quotidien".to_string(), "Coach".to_string()];
        assert_eq!(sentence("fr", &names, 4), "Quotidien et Coach ont échangé 4 messages entre eux sans toi. Continuer l'échange ?");
        let three = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(joined("en", &three), "A, B and C");
    }

    #[test]
    fn one_card_per_chain_until_answered() {
        let dir = tempfile::tempdir().unwrap();
        let d = json!({"bridge_key": "k".repeat(40), "outbox": {"db_path": dir.path().join("bridge.db").to_string_lossy()},
                       "agents": {"Quotidien": {"hermes_url": "http://127.0.0.1:9", "hermes_key": "k".repeat(32)},
                                  "Coach": {"hermes_url": "http://127.0.0.1:9", "hermes_key": "k".repeat(32)}}});
        let config = crate::config::parse_config(&d, None).unwrap();
        let root = format!("run-{}", py::uuid4_hex());
        for n in 0..MAX_HOPS {
            step("quotidien", Some(&root), "coach", 1.0 + n as f64);
        }
        let Step::Capped(chain) = step("quotidien", Some(&root), "coach", 9.0) else { panic!("not capped") };
        let card = card_for(&config, &chain).expect("a card");
        assert!(card.starts_with(RUN_PREFIX));
        let Step::Capped(again) = step("quotidien", Some(&root), "coach", 10.0) else { panic!("not capped") };
        assert_eq!(card_for(&config, &again), None, "one card per chain");
        let items = pending_items(&config, Some("quotidien"));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["run_id"], json!(card));
        assert_eq!(items[0]["choices"], json!(["once", "deny"]));
        assert!(items[0]["bip_summary"].as_str().unwrap().starts_with("Quotidien et Coach ont échangé 4 messages"));
        assert!(pending_items(&config, Some("coach")).is_empty());
        let id = card.strip_prefix(RUN_PREFIX).unwrap();
        assert_eq!(answer(&config, "coach", id, true), None, "another agent's card");
        assert_eq!(answer(&config, "quotidien", id, true), Some(true));
        assert!(pending_items(&config, None).is_empty());
        assert!(matches!(step("quotidien", Some(&root), "coach", 11.0), Step::Go { hop: 1, .. }));
        assert_eq!(answer(&config, "quotidien", id, true), None, "answered already");
        // « Non »: at the cap it stays, without another card
        for n in 0..MAX_HOPS {
            step("quotidien", Some(&root), "coach", 12.0 + n as f64);
        }
        let Step::Capped(chain) = step("quotidien", Some(&root), "coach", 20.0) else { panic!("not capped") };
        let card = card_for(&config, &chain).expect("a card");
        assert_eq!(answer(&config, "quotidien", card.strip_prefix(RUN_PREFIX).unwrap(), false), Some(false));
        let Step::Capped(chain) = step("quotidien", Some(&root), "coach", 21.0) else { panic!("not capped") };
        assert_eq!(card_for(&config, &chain), None);
    }
}
