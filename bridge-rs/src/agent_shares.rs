//! Agents shared with relatives (linked households, households.rs). Two kinds:
//! - **a personal agent** the owner shares (Alex, 12/11: « La personne avec qui tu as partagé ne peut pas écrire ni
//!   faire de demande mais voit tout ce que tu lui mets / demandes »): the relative READS the owner's own conversations
//!   with it, read-only, from the share on or with the ones before (the owner's choice per household); she never
//!   writes, asks, answers its cards or calls it. The feed: agent_feed.rs (internal/notes/personal-share-readonly.md);
//! - **the family's agent** (family_agent.rs): each member talks to it in her own conversation (« relative's turns »
//!   below, guest turns of Hermes patch 0018). Only the family's agent has them now.
//!
//! **The owner's side** (Alex shares Cuisine with Marie):
//!
//!     GET  /v1/agents/{agent_id}/shares   share view (below)
//!     PUT  /v1/agents/{agent_id}/shares   {households: [id], history?: {<household>: "from_now"|"all"},
//!                                          confirmed?: bool}  → share view
//!          409 {detail, warning} when a household is added, or one goes to "all", without `confirmed: true`
//!
//!     share view  {agent, households: [{id, name, shared, history, since}], warning: {private_tools: [name],
//!                  confirm: true}, told: {household: bool} (PUT only: whether their bridge got the news)}
//!                 the family's agent: also `conversations` and `tokens` (its members' turns)
//!
//! Only linked households can be chosen. `history` (`from_now`, the default, or `all`) and `since` (when it was
//! shared) say what of the owner's conversations the relative reads. `private_tools` lists what the agent reaches of
//! the owner's data (health, mail, calendar, contacts, notebook, archives, files): what it read or wrote there in his
//! conversations is in them, so the app names those tools in the confirmation, asked at every share.
//!
//! **Between the bridges** (the household key, as peers.rs and family_news.rs):
//!
//!     POST   /family/shared                                  {agent, name, category, color, shared}  (to the relative)
//!     GET    /family/s/{agent}/feed…                         the owner's conversations, read-only (agent_feed.rs)
//!
//! and, for the family's agent only (a personal agent answers them 403 `shared_read_only`):
//!
//!     GET    /family/s/{agent}/conversations                 {conversations: [conv]}       (this relative's only)
//!     POST   /family/s/{agent}/conversations                 {title?}  → conv
//!     GET    /family/s/{agent}/conversations/{cid}/timeline  a timeline page (docs/conversations.md §6.6)
//!     GET    /family/s/{agent}/conversations/{cid}/events    SSE: `thread` {thread: cid, version} at each change
//!     GET    /family/s/{agent}/conversations/{cid}/runs/{run}/events   SSE: the turn's words as they come (text,
//!                                                            approvals and the end only: no tool step crosses)
//!     POST   /family/s/{agent}/conversations/{cid}/messages  {text, person?, client_id?, files?}  → {run_id}  (409
//!                                                            while a turn goes on; `files`: ids sent first,
//!                                                            shared_files.rs)
//!     POST   /family/s/{agent}/conversations/{cid}/stop      {ok}
//!     DELETE /family/s/{agent}/conversations/{cid}           {ok}
//!     POST   /family/s/{agent}/approval                      {run_id, choice | answer, request_id?}  → Hermes' answer
//!
//!     conv    {id, title, created, updated, messages, tokens: {input, output}}
//!
//! **The relative's side** (Marie's app, her bridge's key; `id` is `<agent>__<household>`, e.g. `cuisine__alex`):
//!
//!     GET    /v1/shared-agents                    {agents: [{id, agent, household, owner, name, category, color,
//!                                                  state: "offered"|"added", since, family?, read_only, new}]}
//!     POST   /v1/shared-agents/{id}/add           {ok}   « Ajouter » (the card in her Boîte)
//!     DELETE /v1/shared-agents/{id}               {ok}   « Non merci » / « Ne plus l'utiliser »
//!
//! A shared agent's conversations are threads of her own bridge's routes with `agent=<id>` (threads/mod.rs), relayed
//! to the owner's bridge: a personal agent's through agent_feed.rs (read only: her writes answer 403
//! `shared_read_only`), the family's agent's here: `GET /v1/threads` (her list), `…/{cid}/timeline`,
//! `GET /v1/threads/events?thread=<cid>` (on screen while open: no push meanwhile), `POST …/{cid}/messages` (`new`: a
//! conversation opened by its first message), `…/{cid}/stop`, `DELETE …/{cid}`, `GET /v1/runs/{run}/events?agent=<id>
//! &thread=<cid>` (the words live). Her pending approvals are also in her `GET /v1/approvals`, and `POST /v1/approve`
//! with that agent relays her answer; while her app isn't looking, her bridge follows the turn and pushes the approval
//! and the reply to her devices.
//!
//! **Safety of a member's turn with the family's agent** (on the owner's bridge):
//! - Her words reach the agent framed as a relative's message, never as the owner's instructions, with run
//!   instructions saying so (family_agent.rs `member_instructions`, souls.rs `CONTENT_RULE`).
//! - The owner's private data stays closed, by permissions (internal/next/relative-turns.md): the bridge's MCP tools
//!   answer only the family's (agent_tools.rs); Hermes' own tools: the run is marked (`guest_turn`, patch 0018,
//!   `guest_tools`): Hermes offers it only those and `web_search`, and refuses every other tool a model names anyway;
//!   it leaves the agent's USER.md out. A Hermes without patch 0018 ignores the mark, so on such a Hermes no member's
//!   turn runs (`guest_turns_closed`, audit 11/11 BA-2).
//! - Approvals belong to the person talking: hers are left out of the owner's Boîte and `/v1/approve` refuses them.
//! - Only text crosses: `MEDIA:` lines are taken out of the replies (rooms.rs `room_text`); her files go into the
//!   agent's pool (shared_files.rs).
//! - Credit: the owner's key; the tokens are counted per member here (`tokens`).
//!
//! `<data>/agent-shares.json` (0600): what is shared with whom (`shared`), each share's choice (`history`: {agent:
//! {household: {mode, since}}}), the family's members' conversations and their tokens; `<data>/shared-with-me.json`
//! (0600): the agents relatives share with this person (what of each she has opened: `seen`).
//!
//! No Python counterpart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{delete, get, post};
use axum::Router;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{split, Households, SEPARATOR};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const TEXT_MAX: usize = 8000;
pub const TITLE_MAX: usize = 80;
pub const SESSION_TITLE: &str = "BipAgents · proche";
/// A relative's turn is no longer « going on » after this (a restart lost its follower): the tools open again.
pub const TURN_MAX_SECONDS: f64 = 3600.0;
/// Conversations a relative keeps with one agent.
pub const CONVERSATIONS_MAX: usize = 50;
/// The bridge's MCP tools a relative's turn may use: the agent's own memory pages (common, decision 3A) and tools
/// reading nothing of the owner's.
/// Not `dates`: it reads the birthdays of the owner's Carnet (2026-10-27).
pub const RELATIVE_TOOLS: [&str; 5] = [
    crate::agent_tools::KNOWLEDGE_READ,
    crate::agent_tools::KNOWLEDGE_SEARCH,
    crate::weather::TOOL_NAME,
    crate::routes::TOOL_NAME,
    crate::maps::TOOL_NAME,
];
/// Hermes' own tools a guest turn keeps (patch 0018 accepts only these besides MCP tools): it reads nothing of anyone.
pub const GUEST_NATIVE_TOOLS: [&str; 1] = ["web_search"];

/// The tools a guest turn may use, as Hermes names them: the bridge's `names` (`mcp__bipagents__<name>`) and
/// `GUEST_NATIVE_TOOLS`.
pub fn guest_tools(names: &[&str]) -> Vec<String> {
    names
        .iter()
        .map(|n| format!("{}{n}", crate::tool_diet::BRIDGE_PREFIX))
        .chain(GUEST_NATIVE_TOOLS.iter().map(|n| n.to_string()))
        .collect()
}

/// The file Hermes patch 0018 adds: without it Hermes ignores the `guest_turn` mark, and a relative's turn runs open
/// (the owner's terminal, files, `session_search`, USER.md).
pub const GUEST_TURN_FILE: &str = "agent/guest_turn.py";

/// A file the patch changes (tracked in Hermes' checkout) and what it adds there: `GUEST_TURN_FILE` is new, hence
/// untracked, and outlives a `git reset --hard` or a stash that takes the patch's other hunks off.
pub const GUEST_TURN_WIRING: (&str, &str) = ("gateway/platforms/api_server_runs.py", "agent.guest_turn");

/// Whether this install's Hermes closes a relative's turn (patch 0018), checked on disk, never by sending a turn (an
/// unpatched Hermes would simply run it) (audit 11/11, BA-2). The offer's places run the shared Hermes built with
/// every patch (a failed one stops the build): yes. A self-install: only when the code the agent runs (its own
/// space's, else the hub's `[hermes] root`, else `<home>/hermes-agent`, the official install's folder) has both the
/// patch's file and its wiring in the run API: a person who declined the fixes, runs another Hermes version, or ran
/// a plain `hermes update` has not. A Hermes whose code folder the bridge cannot find (in a container, set up by
/// hand) counts as open: a relative getting the owner's tools is worse than a share refused (owner's decision 11/11).
pub fn guest_turns_closed(config: &Config, agent: Option<&AgentConfig>) -> bool {
    if config.cloud {
        return true;
    }
    let own = agent.and_then(|a| a.hermes_root.clone()).filter(|r| !r.is_empty());
    let root = own
        .or_else(|| Some(config.hermes.root.clone()).filter(|r| !r.is_empty()))
        .or_else(|| Some(config.hermes.home.clone()).filter(|h| !h.is_empty()).map(|h| py::join(&h, "hermes-agent")));
    let Some(root) = root else {
        tracing::warn!("shared agent: this Hermes' code folder is unknown ([hermes] root): relatives' turns refused");
        return false;
    };
    let root = std::path::Path::new(&root);
    let (wired, mark) = GUEST_TURN_WIRING;
    root.join(GUEST_TURN_FILE).is_file()
        && std::fs::read_to_string(root.join(wired)).is_ok_and(|text| text.contains(mark))
}

/// The refusal while `guest_turns_closed` is false, in the place's language: a plain sentence both apps show
/// (`detail`), and `code` for an app that learns to say it in its own words.
pub fn guest_turns_refused(config: &Config, status: u16) -> Response {
    let text = match crate::notebook::place_language(config).as_deref().unwrap_or("fr") {
        "en" => {
            "Your agents can't be shared with your family until your Hermes is up to date: run bipagents \
hermes-update --fixes-only on your computer."
        }
        "de" => {
            "Deine Agenten können erst mit deiner Familie geteilt werden, wenn dein Hermes aktuell ist: Führe auf \
deinem Computer „bipagents hermes-update --fixes-only“ aus."
        }
        "es" => {
            "Tus agentes no se pueden compartir con tu familia hasta que tu Hermes esté al día: ejecuta \
«bipagents hermes-update --fixes-only» en tu ordenador."
        }
        _ => {
            "Tes agents ne peuvent pas être partagés avec tes proches tant que ton Hermes n’est pas à jour : lance \
« bipagents hermes-update --fixes-only » sur ton ordinateur."
        }
    };
    tracing::warn!("shared agent: refused, this Hermes lacks patch 0018 (relatives' turns would run open)");
    web::json_response(status, &json!({"detail": text, "code": GUEST_TURNS_OPEN}))
}

/// `code` of that refusal.
pub const GUEST_TURNS_OPEN: &str = "hermes_guest_turns_open";

/// What a relative's turn has besides `RELATIVE_TOOLS`: a page, from what is said in her conversation, seen only by
/// her, marked as made at her request, a few a day (shared_pages.rs, Alex 2026-11-09); the files she sent in that
/// conversation, read in her own turns only (shared_files.rs, Alex 11/11).
pub const FRIEND_TOOLS: [&str; 2] = [crate::agent_tools::CREATE_PAGE, crate::shared_files::TOOL_NAME];

/// A member's turn with the family's agent: its family tools (family_agent.rs `TOOLS`).
pub fn relative_guest_tools() -> Vec<String> {
    guest_tools(&crate::family_agent::TOOLS)
}

/// What a relative's bridge answers, and the owner's, to anything but reading a personal agent shared with her.
pub const READ_ONLY: &str = "shared_read_only";

/// The refusal of a write to a personal shared agent: 403 `{detail, code}`.
pub fn read_only_refused() -> HttpError {
    HttpError::with_code(403, "this agent is shared with you to read only", READ_ONLY)
}

/// The first line of a relative's message as the agent gets it (taken off again for the thread).
const FRAME_MARK: &str = "a relative of your person";

static LOCK: Mutex<()> = Mutex::new(());
/// Approvals waiting for this person in agents shared with her: (shared id, run_id, request_id) → the approval.
type PendingKey = (String, String, String);
static PENDING: LazyLock<Mutex<HashMap<PendingKey, Dict>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn now() -> f64 {
    py::time_time()
}

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

// -- the files ------------------------------------------------------------------------------------------------------

pub(crate) fn owner_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-shares.json")
}

fn mine_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("shared-with-me.json")
}

fn load(path: &PathBuf) -> Dict {
    let data: Value =
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
    let mut data = data.as_object().cloned().unwrap_or_default();
    for key in ["shared", "conversations", "tokens", "agents"] {
        if !matches!(data.get(key), Some(Value::Object(_))) {
            data.insert(key.into(), json!({}));
        }
    }
    data
}

fn save(path: &PathBuf, data: &Dict) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    py::write_with_mode(
        &tmp.to_string_lossy(),
        py::Dumps::new().unicode().dumps(&Value::Object(data.clone())).as_bytes(),
        0o600,
    )?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub(crate) fn update<R>(path: &PathBuf, change: impl FnOnce(&mut Dict) -> R) -> anyhow::Result<R> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = load(path);
    let out = change(&mut data);
    save(path, &data)?;
    Ok(out)
}

pub(crate) fn read(path: &PathBuf) -> Dict {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(path)
}

pub(crate) fn object_mut<'a>(data: &'a mut Dict, key: &str) -> &'a mut Map<String, Value> {
    if !matches!(data.get(key), Some(Value::Object(_))) {
        data.insert(key.into(), json!({}));
    }
    match data.get_mut(key) {
        Some(Value::Object(map)) => map,
        _ => unreachable!("an object was just put there"),
    }
}

pub(crate) fn object<'a>(data: &'a Dict, key: &str) -> Option<&'a Map<String, Value>> {
    data.get(key).and_then(Value::as_object)
}

fn households(config: &Config) -> Households {
    Households::new(&crate::households::path_for(config))
}

fn household_name(linked: &Households, household: &str) -> String {
    linked
        .get(household)
        .and_then(|h| h.get("name").map(py::py_str))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| household.to_string())
}

// -- the owner's side: who may use which agent ----------------------------------------------------------------------

/// The linked households `agent` is shared with (unlinked ones left out).
pub fn shared_with(config: &Config, agent: &str) -> Vec<String> {
    let linked = households(config);
    if crate::family_agent::is_family(config, agent) {
        // the family's agent: every linked household, new ones included (family_agent.rs)
        return linked.public().iter().map(|h| h.get("id").map(py::py_str).unwrap_or_default()).collect();
    }
    let data = read(&owner_path(config));
    object(&data, "shared")
        .and_then(|s| s.get(agent))
        .and_then(Value::as_array)
        .map(|items| items.iter().map(py::py_str).filter(|h| linked.get(h).is_some()).collect())
        .unwrap_or_default()
}

/// What of the owner's data the agent reaches (its tools turned on), for the extra confirmation.
pub fn private_tools(config: &Config, agent: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    if crate::health::enabled(config, agent) {
        out.push("health");
    }
    if crate::mailbox::of(config).of_agent(agent).map(|a| !a.is_empty()).unwrap_or(false) {
        out.push("mail");
    }
    if crate::agent_tools::calendar_on(config, agent) {
        out.push("calendar");
    }
    if crate::contacts::enabled(config, agent) {
        out.push("contacts");
    }
    if crate::notebook::enabled(config, agent) {
        out.push("notebook");
    }
    if crate::archives::enabled(config, agent) {
        out.push("archives");
    }
    if crate::agent_files::offered(config, agent) {
        out.push("files");
    }
    out
}

/// The confirmation before sharing (the 409's `detail`, in the place's language; the apps say the same in theirs):
/// what the relatives will read, that they can't write, and what of the owner's private data may be in it.
pub fn confirm_text(config: &Config, agent: &AgentConfig, private: &[&str]) -> String {
    let lang = crate::notebook::place_language(config).unwrap_or_else(|| "fr".into());
    let name = Some(agent.display_name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| agent.name.clone());
    let word = |tool: &str| -> &'static str {
        let words: [&str; 4] = match tool {
            "health" => ["ta santé", "your health data", "deine Gesundheitsdaten", "tu salud"],
            "mail" => ["ta messagerie", "your mail", "deine E-Mails", "tu correo"],
            "calendar" => ["ton agenda", "your calendar", "deinen Kalender", "tu calendario"],
            "contacts" => ["tes contacts", "your contacts", "deine Kontakte", "tus contactos"],
            "notebook" => ["ton Carnet", "your Notebook", "dein Notizbuch", "tu Cuaderno"],
            "archives" => ["tes archives", "your archives", "deine Archive", "tus documentos archivados"],
            _ => ["tes fichiers", "your files", "deine Dateien", "tus archivos"],
        };
        words[match lang.as_str() { "en" => 1, "de" => 2, "es" => 3, _ => 0 }]
    };
    let and = match lang.as_str() { "en" => " and ", "de" => " und ", "es" => " y ", _ => " et " };
    let words: Vec<&str> = private.iter().map(|t| word(t)).collect();
    let list = match words.split_last() {
        Some((last, [])) => last.to_string(),
        Some((last, rest)) => format!("{}{and}{last}", rest.join(", ")),
        None => String::new(),
    };
    let (shared, tools) = match lang.as_str() {
        "en" => (
            format!("Your relatives will be able to read your conversations with {name}: your messages, its replies, \
the photos, files and pages. They won't be able to write to it or ask it anything."),
            format!(" {name} has access to {list}: whatever it reads or writes there in your conversations, they will \
see too."),
        ),
        "de" => (
            format!("Deine Angehörigen können deine Unterhaltungen mit {name} lesen: deine Nachrichten, seine \
Antworten, die Fotos, Dateien und Seiten. Sie können ihm weder schreiben noch etwas fragen."),
            format!(" {name} hat Zugriff auf {list}: Was er dort in deinen Unterhaltungen liest oder schreibt, sehen \
sie auch."),
        ),
        "es" => (
            format!("Tus familiares podrán leer tus conversaciones con {name}: tus mensajes, sus respuestas, las fotos, \
los archivos y las páginas. No podrán escribirle ni pedirle nada."),
            format!(" {name} tiene acceso a {list}: lo que lea o escriba ahí en tus conversaciones, también lo verán."),
        ),
        _ => (
            format!("Tes proches pourront lire tes conversations avec {name} : tes messages, ses réponses, les photos, \
fichiers et pages. Ils ne pourront ni lui écrire ni lui demander quoi que ce soit."),
            format!(" {name} a accès à {list} : ce qu’il y lit ou y écrit dans tes conversations, ils le verront aussi."),
        ),
    };
    if private.is_empty() {
        shared
    } else {
        shared + &tools
    }
}

fn tokens_value(input: i64, output: i64) -> Value {
    json!({"input": input, "output": output})
}

fn n(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn conversation_view(id: &str, c: &Value) -> Value {
    json!({"id": id, "title": c.get("title"), "created": c.get("created"), "updated": c.get("updated"),
           "messages": n(c, "messages"), "tokens": c.get("tokens").cloned().unwrap_or_else(|| tokens_value(0, 0))})
}

/// What the relative of `household` reads of `agent`'s conversations: from `since` on (`from_now`), or all of them
/// (`all`, since 0). None: not shared with that household.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub all: bool,
    pub since: f64,
}

impl Window {
    /// From when on she reads (0: everything).
    pub fn from(&self) -> f64 {
        if self.all {
            0.0
        } else {
            self.since
        }
    }
}

pub const FROM_NOW: &str = "from_now";
pub const ALL: &str = "all";

fn window_of(data: &Dict, agent: &str, household: &str) -> Window {
    let entry = object(data, "history").and_then(|h| h.get(agent)).and_then(|a| a.get(household));
    Window {
        all: entry.is_some_and(|e| str_of(e, "mode") == ALL),
        since: entry.and_then(|e| e.get("since")).and_then(Value::as_f64).unwrap_or(0.0),
    }
}

/// The owner's choice for `household` on `agent` (a personal agent shared with it), None when it isn't.
pub fn window(config: &Config, agent: &str, household: &str) -> Option<Window> {
    if crate::family_agent::is_family(config, agent) || !shared_with(config, agent).iter().any(|h| h == household) {
        return None;
    }
    Some(window_of(&read(&owner_path(config)), agent, household))
}

/// The owner's view of one agent's sharing.
pub fn share_view(config: &Config, agent: &AgentConfig) -> Value {
    let linked = households(config);
    let shared: HashSet<String> = shared_with(config, &agent.name).into_iter().collect();
    let data = read(&owner_path(config));
    let listed: Vec<Value> = linked
        .public()
        .iter()
        .map(|h| {
            let id = h.get("id").map(py::py_str).unwrap_or_default();
            let on = shared.contains(&id);
            let w = window_of(&data, &agent.name, &id);
            json!({"id": id, "name": h.get("name"), "shared": on,
                   "history": on.then_some(if w.all { ALL } else { FROM_NOW }),
                   "since": (on && w.since > 0.0).then_some(w.since)})
        })
        .collect();
    let mut view = json!({
        "agent": agent.name,
        "households": listed,
        "warning": {"private_tools": private_tools(config, &agent.name), "confirm": true},
    });
    if crate::family_agent::is_family(config, &agent.name) {
        view["conversations"] = json!(member_conversations(&data, &linked, &agent.name));
        view["tokens"] = json!(member_tokens(&data, &linked, &agent.name));
    }
    view
}

/// The family's agent's members' conversations: who, when, how many messages (never their words).
fn member_conversations(data: &Dict, linked: &Households, agent: &str) -> Vec<Value> {
    let mut conversations: Vec<Value> = object(data, "conversations")
        .map(|all| {
            all.iter()
                .filter(|(_, c)| str_of(c, "agent") == agent)
                .map(|(id, c)| {
                    let household = str_of(c, "household");
                    let mut view = conversation_view(id, c);
                    if let Some(row) = view.as_object_mut() {
                        row.remove("title");
                    }
                    view["household"] = json!(household);
                    view["household_name"] = json!(household_name(linked, &household));
                    view
                })
                .collect()
        })
        .unwrap_or_default();
    conversations.sort_by(|a, b| {
        let at = |v: &Value| v.get("updated").and_then(Value::as_f64).unwrap_or(0.0);
        at(b).partial_cmp(&at(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    conversations
}

fn member_tokens(data: &Dict, linked: &Households, agent: &str) -> Vec<Value> {
    object(data, "tokens")
        .map(|all| {
            all.iter()
                .filter_map(|(household, per)| {
                    let t = per.get(agent)?;
                    Some(json!({"household": household, "household_name": household_name(linked, household),
                                "input": n(t, "input"), "output": n(t, "output")}))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The owner's choice: these linked households may read `agent` (`history`: a household's choice, `from_now` for a
/// new one given none). Returns the households added and removed.
pub fn set_shared(
    config: &Config,
    agent: &str,
    wanted: &[String],
    history: &HashMap<String, bool>,
) -> anyhow::Result<(Vec<String>, Vec<String>)> {
    let before: HashSet<String> = shared_with(config, agent).into_iter().collect();
    let mut kept: Vec<String> = Vec::new();
    for h in wanted {
        if !kept.contains(h) {
            kept.push(h.clone());
        }
    }
    let at = now();
    update(&owner_path(config), |data| {
        object_mut(data, "shared").insert(agent.to_string(), json!(kept));
        let gone: Vec<String> = before.iter().filter(|h| !kept.contains(h)).cloned().collect();
        let all = object_mut(data, "history");
        let per = all.entry(agent.to_string()).or_insert_with(|| json!({}));
        if !per.is_object() {
            *per = json!({});
        }
        if let Value::Object(per) = per {
            per.retain(|h, _| kept.contains(h));
            for h in &kept {
                let entry = per.entry(h.clone()).or_insert_with(|| json!({"mode": FROM_NOW, "since": at}));
                if let Some(wide) = history.get(h) {
                    entry["mode"] = json!(if *wide { ALL } else { FROM_NOW });
                }
            }
        }
        // a relative no longer shared with: her conversations go (the family's agent; hidden sessions left to Hermes)
        object_mut(data, "conversations")
            .retain(|_, c| !(str_of(c, "agent") == agent && gone.contains(&str_of(c, "household"))));
    })?;
    let added = kept.iter().filter(|h| !before.contains(*h)).cloned().collect();
    let removed: Vec<String> = before.into_iter().filter(|h| !kept.contains(h)).collect();
    crate::shared_files::forget_households(config, agent, &removed);
    Ok((added, removed))
}

// -- a relative's turn going on (the tools' gate, the Boîte's filter) -----------------------------------------------

fn turn_going(c: &Value, at: f64) -> Option<String> {
    let run = c.get("run")?;
    let started = run.get("started").and_then(Value::as_f64)?;
    (at - started < TURN_MAX_SECONDS).then(|| str_of(run, "id")).filter(|id| !id.is_empty())
}

/// The conversation reserved for a turn about to start (`run.id` a `starting_…` mark): None when a turn goes on.
fn reserve_turn(path: &PathBuf, cid: &str) -> anyhow::Result<Option<String>> {
    let mark = format!("starting_{}", crate::py::token_hex(6));
    update(path, |data| {
        let Some(Value::Object(c)) = object_mut(data, "conversations").get_mut(cid) else { return None };
        if turn_going(&Value::Object(c.clone()), now()).is_some() {
            return None;
        }
        c.insert("run".into(), json!({"id": mark, "started": now()}));
        Some(mark)
    })
}

/// The reservation `mark` replaced by the run Hermes started, or let go when none started.
fn settle_turn(path: &PathBuf, cid: &str, mark: &str, run_id: Option<&str>) -> anyhow::Result<()> {
    update(path, |data| {
        if let Some(Value::Object(c)) = object_mut(data, "conversations").get_mut(cid) {
            if c.get("run").and_then(|r| r.get("id")).and_then(Value::as_str) == Some(mark) {
                let run = run_id.map(|id| json!({"id": id, "started": now()})).unwrap_or(Value::Null);
                c.insert("run".into(), run);
            }
            if run_id.is_some() {
                c.insert("updated".into(), json!(now()));
            }
        }
    })
}

/// Whether a relative talks to `agent` right now (the owner's private tools are then closed to it).
pub fn relative_turn(config: &Config, agent: &str) -> bool {
    let data = read(&owner_path(config));
    let at = now();
    object(&data, "conversations")
        .is_some_and(|all| all.values().any(|c| str_of(c, "agent") == agent && turn_going(c, at).is_some()))
}

/// The relatives' turns going on with `agent`: who talks, in which conversation, which run (shared_pages.rs `who`).
pub fn going_turns(config: &Config, agent: &str) -> Vec<crate::shared_pages::Asker> {
    let data = read(&owner_path(config));
    let at = now();
    let linked = households(config);
    object(&data, "conversations")
        .map(|all| {
            all.iter()
                .filter(|(_, c)| str_of(c, "agent") == agent)
                .filter_map(|(cid, c)| {
                    let run = turn_going(c, at)?;
                    let household = str_of(c, "household");
                    let name = Some(str_of(c, "person")).filter(|p| !p.is_empty()).unwrap_or_else(|| household_name(&linked, &household));
                    Some(crate::shared_pages::Asker { household, conversation: cid.clone(), name, run })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `run_id` is a relative's turn (its approvals are theirs, not the owner's).
pub fn relative_run(config: &Config, run_id: &str) -> bool {
    let data = read(&owner_path(config));
    object(&data, "conversations")
        .is_some_and(|all| all.values().any(|c| c.get("run").is_some_and(|r| str_of(r, "id") == run_id)))
}

/// The answer a closed tool gives during a relative's turn (`friend`: a relative talking to a shared agent, who may
/// also have a page made, `FRIEND_TOOLS`; else another household's agent or a relative's discussion).
pub fn closed_tool(name: &str, friend: bool) -> Option<Dict> {
    if RELATIVE_TOOLS.contains(&name) || (friend && FRIEND_TOOLS.contains(&name)) {
        return None;
    }
    let pages = if friend { "" } else { "pages, " };
    let page_ok = if friend { " A page made from what they say here is fine (create_page)." } else { "" };
    Some(tk::tool_text(
        &format!(
            "A relative of your person is talking to you: your person's own data and tools (mail, calendar, contacts, \
             health, notebook, archives, files, {pages}tasks) are closed during their turn.{page_ok} Answer from what you \
             know."
        ),
        true,
    ))
}

// -- the owner's bridge serving a relative --------------------------------------------------------------------------

/// What a relative's turn is told of the files (shared_files.rs): those everyone who uses the agent sent are
/// everyone's, read with `read_sent_file`.
pub fn sent_files_rule(person: &str) -> String {
    format!(
        "The files and photos sent to you by everyone who uses you ({person}, your person, the other relatives) are \
the family's: {person}'s photos come with their message; read a document with read_sent_file (the path of its « Pièce \
jointe » line), and with no path it lists everyone's files, who sent each. Never send them outside the family."
    )
}

/// A household's name as this person's place shows it (who sent a file, shared_files.rs).
pub(crate) fn shown_household(config: &Config, household: &str) -> String {
    household_name(&households(config), household)
}

/// The relative's words as the agent gets them.
pub fn framed(person: &str, household: &str, text: &str) -> String {
    format!("[{person}, {FRAME_MARK} (household « {household} »), writes to you:]\n{text}")
}

fn unframed(text: &str) -> String {
    let shown = crate::live_brief::visible(text); // without the current time at its top (dates/now.rs)
    let text = shown.as_str();
    match text.split_once('\n') {
        Some((first, rest)) if first.starts_with('[') && first.contains(FRAME_MARK) => rest.to_string(),
        _ => text.to_string(),
    }
}

fn household_of(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();
    let household = if token.is_empty() { None } else { state.households.caller(token) };
    household.ok_or_else(|| HttpError::new(401, "unknown household"))
}

/// The agent the calling household may use (404 for an agent not shared with them: they can't tell it exists).
pub(crate) fn shared_agent(state: &AppState, headers: &HeaderMap, agent: &str) -> ApiResult<(String, AgentConfig)> {
    let household = household_of(state, headers)?;
    let config = state.config();
    let found = config.agent(Some(agent)).cloned().filter(|a| shared_with(&config, &a.name).contains(&household));
    match found {
        Some(a) => Ok((household, a)),
        None => Err(HttpError::new(404, "no such shared agent")),
    }
}

/// The agent the calling household may TALK to: the family's agent; a personal agent shared with her is read-only
/// (403 `shared_read_only`, agent_feed.rs serves what she reads).
pub(crate) fn talking_agent(state: &AppState, headers: &HeaderMap, agent: &str) -> ApiResult<(String, AgentConfig)> {
    let (household, agent) = shared_agent(state, headers, agent)?;
    if !crate::family_agent::is_family(&state.config(), &agent.name) {
        tracing::info!(agent = %agent.name, household = %household, "shared agent: a write to a read-only share refused");
        return Err(read_only_refused());
    }
    Ok((household, agent))
}

/// This household's conversation `cid` with `agent`.
pub(crate) fn conversation(config: &Config, household: &str, agent: &str, cid: &str) -> ApiResult<Value> {
    let data = read(&owner_path(config));
    object(&data, "conversations")
        .and_then(|all| all.get(cid))
        .filter(|c| str_of(c, "household") == household && str_of(c, "agent") == agent)
        .cloned()
        .ok_or_else(|| HttpError::new(404, "no such conversation"))
}

fn body_json(bytes: &[u8]) -> ApiResult<Dict> {
    if bytes.is_empty() {
        return Ok(Dict::new());
    }
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(HttpError::new(422, "a JSON object")),
    }
}

async fn served_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let data = read(&owner_path(&state.config()));
    let mut out: Vec<Value> = object(&data, "conversations")
        .map(|all| {
            all.iter()
                .filter(|(_, c)| str_of(c, "household") == household && str_of(c, "agent") == agent.name)
                .map(|(id, c)| conversation_view(id, c))
                .collect()
        })
        .unwrap_or_default();
    out.reverse();
    Ok(web::ok(json!({"conversations": out})))
}

async fn served_create(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let body = body_json(&body)?;
    let title: String = tk::squeeze(&tk::str_or_empty(body.get("title"))).chars().take(TITLE_MAX).collect();
    let config = state.config();
    let count = object(&read(&owner_path(&config)), "conversations")
        .map(|all| {
            all.values().filter(|c| str_of(c, "household") == household && str_of(c, "agent") == agent.name).count()
        })
        .unwrap_or(0);
    if count >= CONVERSATIONS_MAX {
        return Err(HttpError::new(409, "too many conversations with this agent: end one first"));
    }
    let cid = format!("c{}", py::token_hex(6));
    let session_title = format!("{SESSION_TITLE} {household} {cid}");
    let session = state
        .hermes
        .create_session(&agent, &session_title)
        .await
        .map_err(|_| HttpError::new(502, "the agent can't be reached"))?;
    let mut fields = Map::new();
    fields.insert("title".into(), json!(session_title));
    fields.insert("hidden".into(), Value::Bool(true)); // not in the owner's own list
    if state.hermes.update_session(&agent, &session, fields).await.is_err() {
        tracing::info!(agent = %agent.name, "shared agent: the conversation could not be hidden");
    }
    let at = now();
    let entry = json!({"agent": agent.name, "household": household, "session": session, "title": title,
                       "created": at, "updated": at, "messages": 0, "tokens": tokens_value(0, 0), "run": null});
    update(&owner_path(&config), |data| object_mut(data, "conversations").insert(cid.clone(), entry.clone()))?;
    tracing::info!(agent = %agent.name, household = %household, "shared agent: conversation opened");
    Ok(web::ok(conversation_view(&cid, &entry)))
}

/// The cards of her turn waiting for her: its approvals and its questions (Hermes' clarify).
fn prompts_of(state: &AppState, agent: &str, run_id: &str) -> (Vec<Value>, Vec<Value>) {
    let of_run = |list: Vec<Dict>, fields: &[&str]| -> Vec<Value> {
        list.into_iter()
            .filter(|a| a.get("run_id").and_then(Value::as_str) == Some(run_id))
            .map(|a| {
                let mut card = Map::new();
                card.insert("id".into(), json!(crate::threads::answered_id(Some(run_id), a.get("request_id").and_then(Value::as_str))));
                card.insert("run_id".into(), json!(run_id));
                for k in fields {
                    card.insert((*k).into(), a.get(*k).cloned().unwrap_or(Value::Null));
                }
                Value::Object(card)
            })
            .collect()
    };
    (
        of_run(state.hub.pending_approvals(Some(agent)), &["request_id", "command", "description", "choices"]),
        of_run(state.hub.pending_questions(agent), &["kind", "request_id", "questions", "question", "choices"]),
    )
}

/// The timeline of this household's conversation (docs/conversations.md §6.6), as `/v1/threads/{id}/timeline`
/// answers, under the conversation's id: its people's and agent's words and notices only (only text crosses: no
/// file, tool step or Boîte message of the owner's), and the pages made at this household's request
/// (shared_pages.rs), which her bridge asks for by `GET …/pages/{page}`.
async fn served_timeline(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
    q: web::QueryMap,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let c = conversation(&state.config(), &household, &agent.name, &cid)?;
    let want = crate::threads::want_of(&q)?;
    let threads = crate::threads::threads_of(&state);
    let unreachable = |_| HttpError::new(502, "the agent can't be reached");
    // read as Hermes keeps it now: a relative's turn is a guest run, whose end does not refresh the owner's copy
    threads.source(&agent).forget(&str_of(&c, "session"));
    let thread = threads.resolve(&agent, &str_of(&c, "session")).await.map_err(unreachable)?.ok_or_else(HttpError::not_found)?;
    let segments = threads.line(&agent, &thread).await.map_err(unreachable)?;
    let items: Vec<crate::threads::timeline::Item> = threads
        .items(&agent, &thread, &segments)
        .await
        .map_err(unreachable)?
        .into_iter()
        .filter(|i| matches!(i.kind(), "message" | "notice"))
        .map(|mut i| {
            let files = pool_files(i.body.remove("attachments"));
            if !files.is_empty() {
                i.body.insert("attachments".into(), Value::Array(files));
            }
            if i.body.get("role") == Some(&json!("person")) {
                let words = unframed(i.text());
                i.body.insert("text".into(), json!(words.trim()));
            }
            i
        })
        .collect();
    let page = crate::threads::timeline::page(items, &want);
    // the turn going on, as the shared screen draws it: writing, or waiting for her answer, since when
    let (run, (approvals, questions)) = match turn_going(&c, now()) {
        Some(run_id) => {
            let prompts = prompts_of(&state, &agent.name, &run_id);
            let status = if prompts.0.is_empty() && prompts.1.is_empty() { "writing" } else { "approval" };
            let started = c.get("run").and_then(|r| r.get("started")).cloned().unwrap_or(Value::Null);
            (json!({"run_id": run_id, "status": status, "started": started}), prompts)
        }
        None => (Value::Null, Default::default()),
    };
    // the timeline's own answer (its catch-up marks too: `joins`, `reset`, `replaced`): the apps read her conversation
    // in the conversation screen of their own agents, a catch-up after her turn included
    let shown = json!({"id": cid, "kind": "shared", "title": c.get("title"), "version": thread.version});
    let pending = json!({"run": run, "approvals": approvals, "questions": questions});
    Ok(web::ok(page.answer(shown, thread.version, pending, false)))
}

/// Of a message's attachments, the files of the agent's pool her messages carry (shared_files.rs), path and kind
/// only.
fn pool_files(attachments: Option<Value>) -> Vec<Value> {
    let Some(Value::Array(all)) = attachments else { return Vec::new() };
    all.into_iter()
        .filter(|a| crate::shared_files::is_pool_path(&str_of(a, "path")))
        .map(|a| {
            let mut kept = Map::new();
            for k in ["id", "kind", "name", "mime", "size", "path", "offset"] {
                if let Some(v) = a.get(k) {
                    kept.insert(k.into(), v.clone());
                }
            }
            Value::Object(kept)
        })
        .collect()
}

/// An SSE answer.
pub(crate) fn sse(stream: impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static) -> Response {
    let mut response = Response::new(axum::body::Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert("content-type", axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"));
    headers.insert("cache-control", axum::http::HeaderValue::from_static("no-store"));
    headers.insert("x-accel-buffering", axum::http::HeaderValue::from_static("no"));
    response
}

pub(crate) fn event_line(kind: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {kind}\ndata: {}\n\n", py::dumps(data)))
}

/// The household's conversation changed (a turn's end, her message stored, an approval answered): `thread` hints, as
/// `/v1/threads/events` gives them, under the conversation's id.
async fn served_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let c = conversation(&state.config(), &household, &agent.name, &cid)?;
    let threads = crate::threads::threads_of(&state);
    let thread = threads
        .resolve(&agent, &str_of(&c, "session"))
        .await
        .map_err(|_| HttpError::new(502, "the agent can't be reached"))?
        .ok_or_else(HttpError::not_found)?;
    Ok(thread_hints(&state, thread.id, cid))
}

/// `thread` hints of the owner's thread `thread`, under the id the other side knows it by (`shown`).
pub(crate) fn thread_hints(state: &AppState, thread: String, shown: String) -> Response {
    let mut events = crate::threads::threads_of(state).subscribe();
    let stream = async_stream::stream! {
        yield Ok::<_, std::io::Error>(Bytes::from_static(b": threads\n\n"));
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(crate::app::SSE_KEEPALIVE_SECONDS), events.recv()).await {
                Err(_) => yield Ok(Bytes::from_static(b": keepalive\n\n")),
                Ok(Ok((_, kind, data))) if kind == "thread" && data.get("thread") == Some(&json!(thread)) => {
                    yield Ok(event_line("thread", &json!({"thread": shown, "version": data.get("version")})));
                }
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => yield Ok(event_line("lagged", &json!({}))),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
            }
        }
    };
    sse(stream)
}

/// The household's turn as it goes (only text crosses: its words, its approvals and questions, its end; no tool step).
async fn served_run_events(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid, run_id)): UrlPath<(String, String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let c = conversation(&state.config(), &household, &agent.name, &cid)?;
    if turn_going(&c, now()).as_deref() != Some(run_id.as_str()) {
        return Err(HttpError::new(404, "no such turn"));
    }
    Ok(turn_words(&state, &agent, &run_id, true))
}

/// A turn's words as they come, only text crossing: its deltas, its end, and (`prompts`) its approvals and questions;
/// no tool step. `prompts` false: a reader who answers nothing (agent_feed.rs).
pub(crate) fn turn_words(state: &AppState, agent: &AgentConfig, run_id: &str, prompts: bool) -> Response {
    let mut listener = state.hub.subscribe(agent, run_id, "shared", false);
    let stream = async_stream::stream! {
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(crate::app::SSE_KEEPALIVE_SECONDS), listener.queue.recv()).await {
                Err(_) => yield Ok::<_, std::io::Error>(Bytes::from_static(b": keepalive\n\n")),
                Ok(None) | Ok(Some(None)) => break,
                Ok(Some(Some(event))) => {
                    let kind = event.r#type.as_str();
                    let crosses = crate::hermes::is_delta(kind) || crate::hermes::is_terminal(kind)
                        || (prompts && ["approval.request", "clarify.request", crate::hermes_prompts::APPROVAL_SETTLED,
                            crate::hermes_prompts::CLARIFY_SETTLED].contains(&kind));
                    if crosses {
                        let mut data = event.data.clone();
                        data.retain(|k, _| ["event", "run_id", "delta", "text", "request_id", "command", "description",
                                            "choices", "questions", "question", "item_id", "bip_summary", "error", "output"].contains(&k.as_str()));
                        if let Some(Value::String(output)) = data.get_mut("output") {
                            *output = crate::rooms::room_text(output).0;
                        }
                        yield Ok(event_line(kind, &Value::Object(data)));
                    }
                }
            }
        }
        listener.close();
    };
    sse(stream)
}

/// A shared agent's thread `main`: the relative's latest conversation with it (the one it opens on, as one's own
/// agent opens on its main thread), None while she has none (her first message to `main` makes it). Any other id is
/// that conversation.
async fn conversation_of(state: &AppState, id: &str, cid: &str) -> ApiResult<Option<String>> {
    if cid != "main" {
        return Ok(Some(cid.to_string()));
    }
    let (status, list) = to_owner(state, id, reqwest::Method::GET, "conversations", None).await?;
    if status != 200 {
        return Err(HttpError::new(status, "their side refused"));
    }
    let updated = |c: &Value| c.get("updated").and_then(Value::as_f64).unwrap_or(0.0);
    Ok(list
        .get("conversations")
        .and_then(Value::as_array)
        .and_then(|all| all.iter().max_by(|x, y| updated(x).total_cmp(&updated(y))))
        .map(|c| str_of(c, "id"))
        .filter(|c| !c.is_empty()))
}

/// The page of `main` before her first message: her conversation still to make, nothing in it.
fn empty_main() -> Value {
    json!({"thread": {"id": "main", "kind": "shared", "title": null, "tip": null, "version": 0}, "items": [],
           "first": null, "last": null, "more_before": false, "more_after": false, "anchor": null, "version": 0,
           "pending": {"run": null, "approvals": [], "questions": []}})
}

/// The relative's side: `/v1/threads…?agent=<agent>__<household>` relayed to the owner's bridge (the conversation's
/// id is the thread's, and its tip too: the apps key a turn by it). `main`: her latest conversation. None when `agent`
/// is not a shared agent's id.
pub async fn relay_thread(state: &AppState, agent: &str, what: &str, q: &web::QueryMap) -> Option<ApiResult<Response>> {
    if !agent.contains(SEPARATOR) {
        return None;
    }
    if read_only(&state.config(), agent) {
        return Some(crate::agent_feed::relay_thread(state, agent, what, q).await);
    }
    // a catch-up (`after`) is the latest page, the window: her conversation is words only, and its items' cursors
    // move as Hermes stores her turn (the phone's held `s_` row, then the stored one)
    let query: Vec<(String, String)> = q.pairs.iter().filter(|(k, _)| k != "agent" && k != "after").cloned().collect();
    let encoded = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(query).finish();
    Some(match what.split_once('/') {
        // a timeline page: the app is looking at it, nothing pushed meanwhile
        Some((cid, "timeline")) => async {
            let Some(cid) = conversation_of(state, agent, cid).await? else {
                return Ok(web::ok(empty_main()));
            };
            state.presence.touch(agent, &cid);
            let (status, mut page) =
                to_owner(state, agent, reqwest::Method::GET, &format!("conversations/{cid}/timeline?{encoded}"), None)
                    .await?;
            if let Some(Value::Object(thread)) = page.get_mut("thread") {
                thread.insert("tip".into(), json!(cid));
            }
            // her files with a url on this bridge (shared_files.rs)
            Ok(relayed((status, crate::shared_files::urls_under(agent, page))))
        }
        .await,
        // ended by her: its words deleted on the owner's side
        None if what.ends_with(":delete") => async {
            let Some(cid) = conversation_of(state, agent, what.trim_end_matches(":delete")).await? else {
                return Ok(web::ok(json!({"ok": true})));
            };
            to_owner(state, agent, reqwest::Method::DELETE, &format!("conversations/{cid}"), None).await.map(relayed)
        }
        .await,
        // the list: the relative's conversations as threads
        _ if what.is_empty() => to_owner(state, agent, reqwest::Method::GET, "conversations", None).await.map(|(status, list)| {
            let threads: Vec<Value> = list
                .get("conversations")
                .and_then(Value::as_array)
                .map(|all| {
                    all.iter()
                        .map(|c| json!({"id": c.get("id"), "agent": agent, "kind": "shared", "title": c.get("title"),
                                        "hidden": false, "version": 0, "last_active": c.get("updated")}))
                        .collect()
                })
                .unwrap_or_default();
            web::json_response(status, &json!({"threads": threads}))
        }),
        _ => Err(HttpError::not_found()),
    })
}

fn usage_of(run: &Dict) -> Option<(i64, i64)> {
    let usage = run.get("usage")?.as_object()?;
    let k = |key: &str| usage.get(key).map(|v| py::py_int(v).unwrap_or(0)).unwrap_or(0);
    let (input, output) = (k("input_tokens"), k("output_tokens"));
    (input + output > 0).then_some((input, output))
}

fn counters_of(session: &Dict) -> (i64, i64) {
    let counters = crate::jobs::counters(session);
    let k = |key: &str| counters.get(key).and_then(Value::as_i64).unwrap_or(0);
    (k("input"), k("output"))
}

/// The relative's turn followed to its end on the owner's bridge (a follower: the hub keeps its approvals, nothing is
/// pushed to the owner), then its tokens counted for that relative.
async fn follow_turn(
    state: AppState,
    agent: AgentConfig,
    household: String,
    cid: String,
    run_id: String,
    base: (i64, i64),
) {
    let mut listener = state.hub.subscribe(&agent, &run_id, "shared", false);
    let followed = tokio::time::timeout(Duration::from_secs_f64(TURN_MAX_SECONDS), async {
        while listener.next().await.is_some() {}
    })
    .await;
    listener.close();
    if followed.is_err() {
        let _ = state.hermes.stop_run(&agent, &run_id).await;
    }
    let config = state.config();
    let session =
        conversation(&config, &household, &agent.name, &cid).map(|c| str_of(&c, "session")).unwrap_or_default();
    let used = match state.hermes.run(&agent, &run_id).await.ok().as_ref().and_then(usage_of) {
        Some(used) => used,
        None => {
            let after = state.hermes.session(&agent, &session).await.map(|s| counters_of(&s)).unwrap_or(base);
            ((after.0 - base.0).max(0), (after.1 - base.1).max(0))
        }
    };
    let said = |m: &Dict| matches!(m.get("role").and_then(Value::as_str), Some("user" | "assistant"));
    let messages = state.hermes.session_messages(&agent, &session).await.map(|m| m.iter().filter(|m| said(m)).count()).ok();
    let kept = update(&owner_path(&config), |data| {
        if let Some(Value::Object(c)) = object_mut(data, "conversations").get_mut(&cid) {
            if c.get("run").and_then(|r| r.get("id")).and_then(Value::as_str) == Some(run_id.as_str()) {
                c.insert("run".into(), Value::Null);
            }
            let t = c.get("tokens").cloned().unwrap_or_else(|| tokens_value(0, 0));
            c.insert("tokens".into(), tokens_value(n(&t, "input") + used.0, n(&t, "output") + used.1));
            if let Some(count) = messages {
                c.insert("messages".into(), json!(count));
            }
            c.insert("updated".into(), json!(now()));
        }
        let per = object_mut(data, "tokens").entry(household.clone()).or_insert_with(|| json!({}));
        if !per.is_object() {
            *per = json!({});
        }
        if let Value::Object(per) = per {
            let t = per.get(&agent.name).cloned().unwrap_or_else(|| tokens_value(0, 0));
            per.insert(agent.name.clone(), tokens_value(n(&t, "input") + used.0, n(&t, "output") + used.1));
        }
    });
    if let Err(err) = kept {
        tracing::warn!(error = %format!("{err:#}"), "shared agent: the turn's tokens were not kept");
    }
    // her turn's reply stored: her conversation read again (its cached rows forgotten), her screen told to read it
    crate::threads::threads_of(&state).touch(&agent.name, Some(&session), "ended").await;
    tracing::info!(agent = %agent.name, household = %household, tokens = used.0 + used.1, "shared agent: turn ended");
}

async fn served_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let body = body_json(&body)?;
    let text = tk::strip(&tk::str_or_empty(body.get("text"))).to_string();
    // her files, sent first into the agent's pool for this conversation (shared_files.rs): their ids, a scan's text
    let file_ids = crate::shared_files::named(body.get("files"));
    if (text.is_empty() && file_ids.is_empty()) || text.chars().count() > TEXT_MAX {
        return Err(HttpError::new(422, format!("a message of 1 to {TEXT_MAX} characters, or files")));
    }
    let config = state.config();
    let c = conversation(&config, &household, &agent.name, &cid)?;
    if !guest_turns_closed(&config, Some(&agent)) {
        // shared before, and its Hermes updated since without the patch: no turn rather than an open one (503: her
        // app says « your relative's agents don't answer for now »)
        return Ok(guest_turns_refused(&config, 503));
    }
    let attachments = crate::shared_files::claim(&config, &agent.name, &household, &cid, &file_ids)?;
    // one run per shared conversation: reserved in the same locked write as the test, before Hermes is called
    let Some(reserved) = reserve_turn(&owner_path(&config), &cid)? else {
        return Err(HttpError::new(409, "the agent is still answering"));
    };
    let linked = households(&config);
    let shown = household_name(&linked, &household);
    let person: String = Some(tk::squeeze(&tk::str_or_empty(body.get("person"))))
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| shown.clone())
        .chars()
        .take(40)
        .collect();
    // who writes, for what is made at their request (shared_pages.rs: « Faite à la demande de Marie »)
    update(&owner_path(&config), |data| {
        if let Some(Value::Object(c)) = object_mut(data, "conversations").get_mut(&cid) {
            c.insert("person".into(), json!(person));
        }
    })?;
    let rules = crate::names::localize(
        &crate::family_agent::member_instructions(&person),
        &crate::names::agent_language(&config, &agent.name),
    );
    let session = str_of(&c, "session");
    let base = state.hermes.session(&agent, &session).await.map(|s| counters_of(&s)).unwrap_or((0, 0));
    let words = crate::dates::now::stamped(&config, &agent, &framed(&person, &shown, &text));
    // her photos as image parts, her documents as one line each (threads/send.rs, as for one's own agents)
    let input = crate::threads::send::input_of(None, &words, &attachments);
    let tools = relative_guest_tools();
    let started = state.hermes.start_guest_run(&agent, input, &session, Some(&rules), &tools).await;
    let run_id = started.as_ref().ok().cloned();
    settle_turn(&owner_path(&config), &cid, &reserved, run_id.as_deref())?;
    let run_id = run_id.ok_or_else(|| HttpError::new(502, "the agent can't be reached"))?;
    let keys: Vec<String> = file_ids.iter().map(|(k, _)| k.clone()).collect();
    crate::shared_files::mark_sent(&config, &agent.name, &keys)?;
    tracing::info!(agent = %agent.name, household = %household, files = file_ids.len(), "shared agent: a relative's turn");
    // her app's id for the message: its item carries it once Hermes has it (threads/send.rs), with its files
    let client_id = tk::str_or_empty(body.get("client_id"));
    if crate::threads::send::CLIENT_ID.is_match(&client_id) {
        let threads = crate::threads::threads_of(&state);
        if let Ok(Some(thread)) = threads.resolve(&agent, &session).await {
            crate::threads::send::keep(&threads, &agent.name, &thread.id, &client_id, &text, &attachments).await?;
            threads.bump(&thread, "sent").await?;
        }
    }
    state.spawn(follow_turn(state.clone(), agent, household, cid, run_id.clone(), base));
    Ok(web::ok(json!({"run_id": run_id})))
}

async fn served_stop(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let c = conversation(&state.config(), &household, &agent.name, &cid)?;
    if let Some(run_id) = turn_going(&c, now()) {
        let _ = state.hermes.stop_run(&agent, &run_id).await;
    }
    Ok(web::ok(json!({"ok": true})))
}

async fn end_conversation(state: &AppState, agent: &AgentConfig, cid: &str, c: &Value) -> anyhow::Result<()> {
    if let Some(run_id) = turn_going(c, now()) {
        let _ = state.hermes.stop_run(agent, &run_id).await;
    }
    if let Err(err) = state.hermes.delete_session(agent, &str_of(c, "session")).await {
        tracing::info!(agent = %agent.name, error = %err, "shared agent: a conversation was not deleted in Hermes");
    }
    update(&owner_path(&state.config()), |data| {
        object_mut(data, "conversations").shift_remove(cid);
    })?;
    Ok(()) // the files sent in it stay in the agent's pool, everyone's (shared_files.rs)
}

async fn served_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath((agent, cid)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let c = conversation(&state.config(), &household, &agent.name, &cid)?;
    end_conversation(&state, &agent, &cid, &c).await?;
    Ok(web::ok(json!({"ok": true})))
}

/// The relative answers an approval or a question (`answer`) of their own turn (never one of the owner's runs).
async fn served_approval(
    State(state): State<AppState>,
    headers: HeaderMap,
    UrlPath(agent): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let (household, agent) = talking_agent(&state, &headers, &agent)?;
    let body = body_json(&body)?;
    let run_id = tk::str_or_empty(body.get("run_id"));
    let (kind, value) = match body.get("answer").and_then(Value::as_str) {
        Some(answer) => ("clarify", answer.trim().to_string()),
        None => ("approval", tk::str_or_empty(body.get("choice")).trim().to_lowercase()),
    };
    let request_id = Some(tk::str_or_empty(body.get("request_id"))).filter(|r| !r.is_empty());
    let config = state.config();
    let theirs = object(&read(&owner_path(&config)), "conversations").is_some_and(|all| {
        all.values().any(|c| {
            str_of(c, "household") == household
                && str_of(c, "agent") == agent.name
                && c.get("run").is_some_and(|r| str_of(r, "id") == run_id)
        })
    });
    if run_id.is_empty() || !theirs {
        return Err(HttpError::new(404, "no such turn"));
    }
    let (status, payload) = state
        .hermes
        .answer_prompt(&agent, &run_id, kind, &value, request_id.as_deref())
        .await
        .map_err(|_| HttpError::new(502, "the agent can't be reached"))?;
    // answered, or a question nothing waits for any more (409): gone from every phone
    if (200..300).contains(&status) || (status == 409 && kind == "clarify" && request_id.is_some()) {
        state.hub.forget_approval(&agent.name, &run_id, request_id.as_deref());
        let session = object(&read(&owner_path(&config)), "conversations")
            .and_then(|all| all.values().find(|c| c.get("run").is_some_and(|r| str_of(r, "id") == run_id)).map(|c| str_of(c, "session")));
        crate::threads::threads_of(&state).touch(&agent.name, session.as_deref(), "answered").await;
    }
    tracing::info!(agent = %agent.name, household = %household, status = status, kind = kind, "shared agent: a relative's answer");
    Ok(web::json_response(status, &payload))
}

/// `POST /family/shared`: a relative shares one of their agents with this person (or stops).
async fn offered(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let household = household_of(&state, &headers)?;
    let body = body_json(&body)?;
    receive_offer(&state.config(), &household, &body)?;
    Ok(web::ok(json!({"ok": true})))
}

/// An offer from a linked household kept (or taken back): `{agent, name, category, color, shared, family?}`. The
/// family's agent (family_agent.rs) is « added » at once, and `new` until the person opened it.
pub fn receive_offer(config: &Config, household: &str, body: &Dict) -> ApiResult<()> {
    let agent: String = tk::str_or_empty(body.get("agent")).chars().take(64).collect();
    if agent.is_empty()
        || agent.contains(SEPARATOR)
        || !agent.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(HttpError::new(422, "an agent id"));
    }
    let id = format!("{agent}{SEPARATOR}{household}");
    let shared = body.get("shared").map(py::truthy).unwrap_or(true);
    let family = body.get("family").is_some_and(py::truthy);
    let owner = household_name(&households(config), household);
    let cut = |key: &str, max: usize| -> Value {
        let text: String = tk::str_or_empty(body.get(key)).chars().take(max).collect();
        if text.is_empty() {
            Value::Null
        } else {
            json!(text)
        }
    };
    let entry_name = cut("name", 60);
    let (category, color) = (cut("category", 30), cut("color", 30));
    update(&mine_path(config), |data| {
        let agents = object_mut(data, "agents");
        if !shared {
            agents.shift_remove(&id);
            return;
        }
        if family {
            // one family agent per household: an older one goes
            agents.retain(|k, a| {
                *k == id || !(str_of(a, "household") == household && a.get("family").is_some_and(py::truthy))
            });
        }
        let before = agents.get(&id).cloned();
        let state_word = if family {
            "added".to_string()
        } else {
            before.as_ref().map(|a| str_of(a, "state")).filter(|s| s == "added").unwrap_or("offered".into())
        };
        let since = before.as_ref().and_then(|a| a.get("since").cloned()).unwrap_or(json!(now()));
        let mut entry = json!({"agent": agent, "household": household, "owner": owner, "name": entry_name,
                               "category": category, "color": color, "state": state_word, "since": since});
        if family {
            entry["family"] = json!(true);
            entry["new"] =
                before.as_ref().map(|a| a.get("new").cloned().unwrap_or(json!(false))).unwrap_or(json!(true));
        }
        agents.insert(id.clone(), entry);
    })?;
    tracing::info!(household = %household, agent = %agent, shared = shared, family = family, "shared agent: offer received");
    Ok(())
}

/// The family's agent of `household` is gone (it answered it has none, or handed it over).
pub fn drop_family_offers(config: &Config, household: &str) {
    let _ = update(&mine_path(config), |data| {
        object_mut(data, "agents")
            .retain(|_, a| !(str_of(a, "household") == household && a.get("family").is_some_and(py::truthy)));
    });
}

/// The person opened the family's agent: no « Nouveau » card any more.
pub fn family_seen(config: &Config, id: &str) -> anyhow::Result<()> {
    update(&mine_path(config), |data| {
        if let Some(Value::Object(a)) = object_mut(data, "agents").get_mut(id) {
            a.insert("new".into(), json!(false));
        }
    })
}

/// What a linked household's bridge may call (its key). Mounted at the root.
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/shared", post(offered))
        .route("/family/s/{agent}/conversations", get(served_list).post(served_create))
        .route("/family/s/{agent}/conversations/{cid}", delete(served_delete))
        .route("/family/s/{agent}/conversations/{cid}/messages", post(served_post))
        .route("/family/s/{agent}/conversations/{cid}/timeline", get(served_timeline))
        .route("/family/s/{agent}/conversations/{cid}/events", get(served_events))
        .route("/family/s/{agent}/conversations/{cid}/runs/{run_id}/events", get(served_run_events))
        .route("/family/s/{agent}/conversations/{cid}/stop", post(served_stop))
        .route("/family/s/{agent}/approval", post(served_approval))
        .merge(crate::shared_files::household_router()) // the files she sends in her conversations
        .merge(crate::agent_feed::household_router()) // a personal agent's conversations, read-only
}

// -- the owner's app ------------------------------------------------------------------------------------------------

fn agent_or_404(config: &Config, id: &str) -> ApiResult<AgentConfig> {
    config.agent(Some(id)).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

async fn get_shares(State(state): State<AppState>, UrlPath(agent): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_or_404(&config, &agent)?;
    Ok(web::ok(share_view(&config, &agent)))
}

/// Tells a relative's bridge an agent is shared with them, or not any more.
async fn tell_relative(state: &AppState, household: &str, agent: &AgentConfig, shared: bool) -> bool {
    let Some(entry) = state.households.get(household) else { return false };
    let url = crate::family_relay::reach_url(&entry);
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let body = json!({"agent": agent.name, "name": agent.display_name, "category": agent.category,
                      "color": agent.color, "shared": shared});
    match crate::door_tls::household_client(&state.http, &entry)
        .post(format!("{url}/family/shared"))
        .timeout(Duration::from_secs(10))
        .header("Authorization", format!("Bearer {key}"))
        .json(&body)
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

async fn put_shares(
    State(state): State<AppState>,
    UrlPath(agent): UrlPath<String>,
    body: Bytes,
) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_or_404(&config, &agent)?;
    let body = body_json(&body)?;
    let wanted: Vec<String> = match body.get("households") {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
        _ => return Err(HttpError::new(422, "households: a list of linked households' ids")),
    };
    let linked = households(&config);
    if let Some(unknown) = wanted.iter().find(|h| linked.get(h).is_none()) {
        return Err(HttpError::new(400, format!("not a linked household: {unknown}")));
    }
    // each household's choice: « Aussi les conversations d'avant » (`all`) or « À partir de maintenant »
    let mut history: HashMap<String, bool> = HashMap::new();
    if let Some(Value::Object(given)) = body.get("history") {
        for (household, mode) in given {
            match mode.as_str() {
                Some(ALL) => history.insert(household.clone(), true),
                Some(FROM_NOW) => history.insert(household.clone(), false),
                _ => return Err(HttpError::new(422, "history: \"from_now\" or \"all\" per household")),
            };
        }
    }
    let before = shared_with(&config, &agent.name);
    let data = read(&owner_path(&config));
    // what makes more of the owner's words readable: a household added, one going to `all`
    let widens = wanted.iter().any(|h| {
        !before.contains(h) || (history.get(h) == Some(&true) && !window_of(&data, &agent.name, h).all)
    });
    if widens && !body.get("confirmed").map(py::truthy).unwrap_or(false) {
        let view = share_view(&config, &agent);
        let mut payload = json!({"detail": confirm_text(&config, &agent, &private_tools(&config, &agent.name))});
        payload["warning"] = view["warning"].clone();
        return Ok(web::json_response(409, &payload));
    }
    let (added, removed) = set_shared(&config, &agent.name, &wanted, &history)?;
    let mut told = Map::new();
    for (household, shared) in added.iter().map(|h| (h, true)).chain(removed.iter().map(|h| (h, false))) {
        told.insert(household.clone(), json!(tell_relative(&state, household, &agent, shared).await));
    }
    tracing::info!(agent = %agent.name, added = ?added, removed = ?removed, "shared agent: shares set");
    if !added.is_empty() {
        // others see its conversations: out of the person's hub and of the shared memory (vault.rs `others_facing`)
        crate::own_space::apart_when_idle(&state, &agent.name);
    }
    let mut view = share_view(&config, &agent);
    view["told"] = Value::Object(told);
    Ok(web::ok(view))
}

// -- the relative's app -----------------------------------------------------------------------------------------------

/// The agents relatives share with this person (still linked ones only).
pub fn shared_with_me(config: &Config) -> Vec<Value> {
    let linked = households(config);
    let data = read(&mine_path(config));
    object(&data, "agents")
        .map(|all| {
            all.iter()
                .filter(|(_, a)| linked.get(&str_of(a, "household")).is_some())
                .map(|(id, a)| {
                    let mut view = a.clone();
                    view["id"] = json!(id);
                    view
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One of them, by its id (`cuisine__alex`); `added`: only once the person took it.
fn mine(config: &Config, id: &str, added: bool) -> ApiResult<Value> {
    let found = shared_with_me(config).into_iter().find(|a| str_of(a, "id") == id);
    match found {
        Some(a) if !added || str_of(&a, "state") == "added" => Ok(a),
        Some(_) => Err(HttpError::new(409, "add this agent first")),
        None => Err(HttpError::new(404, "no such shared agent")),
    }
}

/// « Cuisine (Alex) »: how this person's notifications name an agent a relative shares with her.
pub fn shown_name(config: &Config, id: &str) -> Option<String> {
    let found = shared_with_me(config).into_iter().find(|a| str_of(a, "id") == id)?;
    let (name, owner) = (str_of(&found, "name"), str_of(&found, "owner"));
    match (name.is_empty(), owner.is_empty()) {
        (true, _) => None,
        (false, true) => Some(name),
        (false, false) => Some(format!("{name} ({owner})")),
    }
}

/// Whether `id` is an agent a relative shares with this person (and she added it).
pub fn is_shared_with_me(config: &Config, id: &str) -> bool {
    split(id).is_some() && mine(config, id, true).is_ok()
}

/// Whether `id` is a personal agent a relative shares with this person: she reads it, never writes (agent_feed.rs).
pub fn read_only(config: &Config, id: &str) -> bool {
    shared_with_me(config).iter().any(|a| str_of(a, "id") == id && !a.get("family").is_some_and(py::truthy))
}

/// The shared agent `id` as this person's bridge keeps it (agent_feed.rs: what she has opened of it).
pub(crate) fn mine_path_of(config: &Config) -> PathBuf {
    mine_path(config)
}

async fn list_mine(State(state): State<AppState>) -> ApiResult<Response> {
    let mut agents = shared_with_me(&state.config());
    for a in agents.iter_mut() {
        let read_only = !a.get("family").is_some_and(py::truthy);
        a["read_only"] = json!(read_only);
        if read_only {
            // « Nouveau »: something in its owner's conversations she hasn't opened yet
            let id = str_of(a, "id");
            a["new"] = json!(str_of(a, "state") == "added" && crate::agent_feed::has_new(&state, &id).await);
        }
        if let Some(row) = a.as_object_mut() {
            row.remove("seen");
        }
    }
    Ok(web::ok(json!({"agents": agents})))
}

async fn add_mine(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    mine(&config, &id, false)?;
    update(&mine_path(&config), |data| {
        if let Some(Value::Object(a)) = object_mut(data, "agents").get_mut(&id) {
            a.insert("state".into(), json!("added"));
        }
    })?;
    Ok(web::ok(json!({"ok": true})))
}

async fn drop_mine(State(state): State<AppState>, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    let config = state.config();
    mine(&config, &id, false)?;
    // the files she sent it go with her (shared_files.rs): asked of its owner's bridge while she still may
    crate::shared_files::forget_mine(&state, &id).await;
    update(&mine_path(&config), |data| {
        object_mut(data, "agents").shift_remove(&id);
    })?;
    Ok(web::ok(json!({"ok": true})))
}

/// A call to the owner's bridge for one of the agents shared with this person: their answer, as it is.
pub(crate) async fn to_owner(
    state: &AppState,
    id: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> ApiResult<(u16, Value)> {
    let mut request = owner_request(state, id, method, path)?;
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    let status = resp.status().as_u16();
    let payload = resp.json::<Value>().await.unwrap_or(Value::Null);
    Ok((status, payload))
}

/// A request to the owner's bridge for one of the agents shared with this person (`…/family/s/<agent>/<path>`, her
/// household's key, 20 s unless the caller sets its own).
pub(crate) fn owner_request(state: &AppState, id: &str, method: reqwest::Method, path: &str) -> ApiResult<reqwest::RequestBuilder> {
    let config = state.config();
    let shared = mine(&config, id, true)?;
    let household = str_of(&shared, "household");
    let entry = state.households.get(&household).ok_or_else(|| HttpError::new(404, "no such shared agent"))?;
    let url = format!(
        "{}/family/s/{}/{path}",
        crate::family_relay::reach_url(&entry),
        str_of(&shared, "agent")
    );
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let http = crate::door_tls::household_client(&state.http, &entry);
    Ok(http.request(method, url).timeout(Duration::from_secs(20)).bearer_auth(key))
}

pub(crate) fn relayed((status, payload): (u16, Value)) -> Response {
    web::json_response(status, &payload)
}

/// Follows the relative's turn from her side: the approvals listed in her Boîte and pushed, its questions and the
/// reply pushed, while her app isn't looking.
async fn follow_from_here(state: AppState, id: String, cid: String, run_id: String) {
    let started = now();
    let mut pushed: HashSet<String> = HashSet::new();
    loop {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let answer = to_owner(&state, &id, reqwest::Method::GET, &format!("conversations/{cid}/timeline?limit=1"), None).await;
        let Ok((200, page)) = answer else {
            if now() - started > TURN_MAX_SECONDS {
                break;
            }
            continue;
        };
        let pending = page.get("pending").cloned().unwrap_or(Value::Null);
        let going = pending.get("run").and_then(|r| r.get("run_id")).and_then(Value::as_str) == Some(run_id.as_str());
        let approvals: Vec<Value> = pending.get("approvals").and_then(Value::as_array).cloned().unwrap_or_default();
        {
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            pending.retain(|(a, r, _), _| !(*a == id && *r == run_id));
            for a in &approvals {
                let rid = str_of(a, "request_id");
                let mut item = a.as_object().cloned().unwrap_or_default();
                item.insert("agent".into(), json!(id));
                item.insert("session_id".into(), json!(cid));
                item.entry("created_at").or_insert(json!(now()));
                pending.insert((id.clone(), run_id.clone(), rid), item);
            }
        }
        let looking = state.presence.viewing(&id, &cid);
        for a in &approvals {
            let rid = str_of(a, "request_id");
            if looking || !pushed.insert(format!("a:{rid}")) {
                continue;
            }
            let choices = a.get("choices").and_then(Value::as_array).cloned();
            let command = a.get("command").and_then(Value::as_str).map(str::to_string);
            let request = Some(rid.clone()).filter(|r| !r.is_empty());
            state
                .push
                .notify_approval_values(
                    &id,
                    &run_id,
                    request.as_deref(),
                    choices.as_deref(),
                    command.as_deref(),
                    Some(&cid),
                    None,
                )
                .await;
        }
        for q in pending.get("questions").and_then(Value::as_array).into_iter().flatten() {
            let rid = str_of(q, "request_id");
            if looking || !pushed.insert(format!("q:{rid}")) {
                continue;
            }
            let first = q.get("questions").and_then(|l| l.get(0)).unwrap_or(q);
            let question = first.get("question").and_then(Value::as_str);
            let request = Some(rid.as_str()).filter(|r| !r.is_empty());
            state.push.notify_question(&id, &run_id, request, question, Some(&cid)).await;
        }
        if !going || now() - started > TURN_MAX_SECONDS {
            let reply = page
                .get("items")
                .and_then(Value::as_array)
                .and_then(|m| m.last())
                .filter(|m| str_of(m, "role") == "agent")
                .map(|m| str_of(m, "text"));
            if let Some(reply) = reply.filter(|_| !state.presence.viewing(&id, &cid)) {
                state.push.notify_reply(&id, &run_id, &reply, Some(&cid)).await;
            }
            break;
        }
    }
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|(a, r, _), _| !(*a == id && *r == run_id));
}

/// Her message (`POST /v1/threads/{cid}/messages?agent=<id>`, `cid` `new` for a new conversation): relayed to the
/// owner's bridge, and its turn followed from here for her pushes.
pub async fn relay_send(
    state: &AppState,
    agent: Option<&str>,
    cid: &str,
    body: &crate::threads::send::NewMessage,
) -> Option<ApiResult<Response>> {
    let id = agent.filter(|a| a.contains(SEPARATOR))?.to_string();
    if read_only(&state.config(), &id) {
        return Some(Err(read_only_refused()));
    }
    Some(async {
        let text = tk::strip(&body.text).to_string();
        if (text.is_empty() && body.attachments.is_empty()) || text.chars().count() > TEXT_MAX {
            return Err(HttpError::new(422, format!("a message of 1 to {TEXT_MAX} characters, or files")));
        }
        // her files (photos inline, documents held here: shared_files.rs), read before anything is made
        let files = crate::shared_files::outgoing(&state.config(), &id, &body.attachments)?;
        let cid = conversation_of(state, &id, cid).await?.unwrap_or_else(|| "new".to_string());
        let cid = match cid.as_str() {
            "new" => {
                // titled with her first words, else her first file's name
                let first = if text.is_empty() { body.attachments[0].name.clone().unwrap_or_default() } else { text.clone() };
                let title: String = first.chars().take(TITLE_MAX).collect();
                let (status, made) = to_owner(state, &id, reqwest::Method::POST, "conversations", Some(json!({"title": title}))).await?;
                if status != 200 {
                    return Ok(relayed((status, made)));
                }
                str_of(&made, "id")
            }
            cid => cid.to_string(),
        };
        // her files first, into her conversation on the owner's side; then the message naming them
        let ids = match crate::shared_files::send_files(state, &id, &cid, &files).await? {
            Ok(ids) => ids,
            Err(refused) => return Ok(relayed(refused)),
        };
        let person = state.households.name();
        let sent = json!({"text": text, "person": person, "client_id": body.client_id, "files": ids});
        let answer = to_owner(state, &id, reqwest::Method::POST, &format!("conversations/{cid}/messages"), Some(sent)).await;
        let (status, answer) = match answer {
            Ok(answer) => answer,
            Err(err) => {
                crate::shared_files::take_back(state, &id, &cid, &ids).await;
                return Err(err);
            }
        };
        let Some(run_id) = answer.get("run_id").and_then(Value::as_str).filter(|_| status == 200).map(str::to_string) else {
            // not taken (a turn still going on): the files it got taken back, the ones held here kept for a retry
            crate::shared_files::take_back(state, &id, &cid, &ids).await;
            return Ok(relayed((status, answer)));
        };
        crate::shared_files::sent(&files);
        state.spawn(follow_from_here(state.clone(), id.clone(), cid.clone(), run_id.clone()));
        Ok(web::ok(json!({"client_id": body.client_id, "thread": cid, "tip": cid, "state": "started", "run_id": run_id})))
    }
    .await)
}

pub async fn relay_stop(state: &AppState, agent: Option<&str>, cid: &str) -> Option<ApiResult<Response>> {
    let id = agent.filter(|a| a.contains(SEPARATOR))?;
    if read_only(&state.config(), id) {
        return Some(Err(read_only_refused()));
    }
    Some(
        async {
            let Some(cid) = conversation_of(state, id, cid).await? else {
                return Ok(web::ok(json!({"stopped": false})));
            };
            to_owner(state, id, reqwest::Method::POST, &format!("conversations/{cid}/stop"), None)
                .await
                .map(|(status, _)| web::json_response(status, &json!({"stopped": status == 200})))
        }
        .await,
    )
}

/// An event stream of the owner's (`events`, `runs/<run>/events` of conversation `cid`), passed on as it comes;
/// while open, the conversation is on her screen (nothing pushed from here meanwhile).
pub async fn relay_stream(state: &AppState, id: &str, cid: &str, path: &str) -> ApiResult<Response> {
    if read_only(&state.config(), id) {
        return crate::agent_feed::relay_stream(state, id, cid, path).await;
    }
    // `main`: her latest conversation (none yet: nothing to follow)
    let cid = &conversation_of(state, id, cid).await?.ok_or_else(HttpError::not_found)?;
    let config = state.config();
    let shared = mine(&config, id, true)?;
    let entry = state.households.get(&str_of(&shared, "household")).ok_or_else(|| HttpError::new(404, "no such shared agent"))?;
    let url = format!("{}/family/s/{}/conversations/{cid}/{path}", crate::family_relay::reach_url(&entry),
                      str_of(&shared, "agent"));
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let resp = crate::door_tls::household_client(&state.http, &entry)
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| HttpError::new(502, "their side does not answer right now: try again later"))?;
    if !resp.status().is_success() {
        return Err(HttpError::new(resp.status().as_u16(), "their side refused"));
    }
    let (state, id, cid) = (state.clone(), id.to_string(), cid.to_string());
    let turn = path.starts_with("runs/");
    let mut chunks = resp.bytes_stream();
    let stream = async_stream::stream! {
        use futures::StreamExt;
        state.presence.touch(&id, &cid);
        while let Some(chunk) = chunks.next().await {
            state.presence.touch(&id, &cid); // their keepalive every 10 s: still on screen
            match chunk {
                Ok(bytes) => {
                    // her turn's end: passed on once the owner's side keeps it over (its reply stored), so the page
                    // her app reads right after has the reply
                    if turn && bytes.windows(11).any(|w| w == b"event: run.") {
                        turn_kept(&state, &id, &cid).await;
                    }
                    yield Ok::<_, std::io::Error>(bytes)
                }
                Err(_) => break,
            }
        }
    };
    Ok(sse(stream))
}

/// Waits (at most 10 s) until the owner's side no longer lists her turn as going: it then kept its reply.
async fn turn_kept(state: &AppState, id: &str, cid: &str) {
    for _ in 0..20 {
        let page = to_owner(state, id, reqwest::Method::GET, &format!("conversations/{cid}/timeline?limit=1"), None).await;
        if let Ok((200, page)) = page {
            if page.get("pending").and_then(|p| p.get("run")).is_none_or(Value::is_null) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The person's answer to an approval or a question (`answer`) of her turn, relayed to the owner's bridge.
pub async fn approve_mine(
    state: &AppState,
    id: &str,
    run_id: &str,
    choice: &str,
    answer: Option<&str>,
    request_id: Option<&str>,
) -> ApiResult<Response> {
    if read_only(&state.config(), id) {
        return Err(read_only_refused());
    }
    let mut body = json!({"run_id": run_id, "choice": choice, "request_id": request_id});
    if let Some(answer) = answer {
        body["answer"] = json!(answer);
    }
    let question = answer.is_some();
    let answer = to_owner(state, id, reqwest::Method::POST, "approval", Some(body)).await?;
    // answered, or a question nothing waits for any more (409): gone
    if (200..300).contains(&answer.0) || (answer.0 == 409 && question && request_id.is_some()) {
        PENDING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(a, r, q), _| !(a == id && r == run_id && request_id.is_none_or(|rid| rid == q)));
    }
    Ok(relayed(answer))
}

pub fn pending_mine(agent: Option<&str>) -> Vec<Dict> {
    let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    pending.iter().filter(|((a, _, _), _)| agent.is_none_or(|x| x == a)).map(|(_, item)| item.clone()).collect()
}

/// For the apps (mounted under /v1, bridge key): the owner's sharing and the relative's shared agents.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/agents/{agent_id}/shares", get(get_shares).put(put_shares))
        .route("/shared-agents", get(list_mine))
        .route("/shared-agents/{id}", delete(drop_mine))
        .route("/shared-agents/{id}/add", post(add_mine))
        .merge(crate::shared_files::router()) // her conversation's files, from the owner's bridge
        .merge(crate::agent_feed::router()) // what she opens of a personal agent's conversations
}

// -- the move to read-only shares (12/11) ---------------------------------------------------------------------------

/// What `drop_relative_conversations` takes away of `data` (pure): the relatives' conversations with personal agents
/// (`(cid, agent, session)`, `gone` those already deleted in Hermes or whose agent is gone), their tokens, and gives
/// each personal share its choice (`from_now`, `at`) when it has none. Returns the conversations still to delete.
pub fn read_only_migration(
    data: &mut Dict,
    family: &dyn Fn(&str) -> bool,
    gone: &HashSet<String>,
    at: f64,
) -> Vec<(String, String, String)> {
    let mut left = Vec::new();
    object_mut(data, "conversations").retain(|cid, c| {
        let agent = str_of(c, "agent");
        if family(&agent) {
            return true;
        }
        if gone.contains(cid) {
            return false;
        }
        left.push((cid.clone(), agent, str_of(c, "session")));
        true
    });
    for per in object_mut(data, "tokens").values_mut() {
        if let Value::Object(per) = per {
            per.retain(|agent, _| family(agent));
        }
    }
    let shared: Vec<(String, Vec<String>)> = object(data, "shared")
        .map(|all| {
            all.iter()
                .filter(|(agent, _)| !family(agent))
                .map(|(agent, h)| (agent.clone(), h.as_array().map(|l| l.iter().map(py::py_str).collect()).unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let history = object_mut(data, "history");
    for (agent, households) in shared {
        let per = history.entry(agent).or_insert_with(|| json!({}));
        if let Value::Object(per) = per {
            for h in households {
                per.entry(h).or_insert_with(|| json!({"mode": FROM_NOW, "since": at}));
            }
        }
    }
    left
}

/// At the bridge's start (Alex 12/11, no compatibility window): the relatives' own conversations with personal
/// shared agents go — their `agent-shares.json` entries, their hidden Hermes sessions (else `session_search` could
/// bring them up in the owner's turns), their tokens; the shares stay, read-only from now on. The files relatives sent
/// to those agents go with the pools' purge (shared_files.rs `pooled`: the family's agent only). Retried while Hermes
/// doesn't answer. Nothing to do once done.
pub async fn drop_relative_conversations(state: AppState) {
    let mut gone: HashSet<String> = HashSet::new();
    for attempt in 0..30 {
        let config = state.config();
        let family = |agent: &str| crate::family_agent::is_family(&config, agent);
        let left = match update(&owner_path(&config), |data| read_only_migration(data, &family, &gone, now())) {
            Ok(left) => left,
            Err(err) => {
                tracing::warn!(error = %format!("{err:#}"), "shared agents: the read-only move was not written");
                return;
            }
        };
        if left.is_empty() {
            if attempt > 0 {
                tracing::info!(deleted = gone.len(), "shared agents: the relatives' conversations with personal agents deleted");
            }
            return;
        }
        let mut failed = false;
        for (cid, agent, session) in left {
            let Some(agent) = config.agent(Some(&agent)).cloned() else {
                gone.insert(cid);
                continue;
            };
            match state.hermes.delete_session(&agent, &session).await {
                Ok(()) | Err(crate::hermes::HermesError::Http { status: 404, .. }) => {
                    gone.insert(cid);
                }
                Err(err) => {
                    failed = true;
                    tracing::info!(agent = %agent.name, error = %err, "shared agents: a relative's conversation not deleted yet");
                }
            }
        }
        if failed {
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guest_turn_check_looks_for_what_patch_0018_really_writes() {
        // the repository's patch for the pinned Hermes (hermes/patches/BASE): if a rebase onto a newer Hermes moves the
        // file or its wiring, every updated self-install would refuse sharing (or a check left behind would pass)
        let patches = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../hermes/patches");
        let patch = std::fs::read_dir(&patches)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("0018-") && n.ends_with(".patch")))
            .expect("patch 0018 in hermes/patches");
        let text = std::fs::read_to_string(patch).unwrap();
        let section = |path: &str| {
            let head = format!("diff --git a/{path} b/{path}\n");
            let start = text.find(&head).unwrap_or_else(|| panic!("patch 0018 changes no {path}")) + head.len();
            let end = text[start..].find("\ndiff --git ").map_or(text.len(), |e| start + e);
            text[start..end].to_string()
        };
        assert!(section(GUEST_TURN_FILE).starts_with("new file mode"), "patch 0018 creates {GUEST_TURN_FILE}");
        let (wired, mark) = GUEST_TURN_WIRING;
        assert!(
            section(wired).lines().any(|l| l.starts_with('+') && !l.starts_with("+++") && l.contains(mark)),
            "patch 0018 adds « {mark} » to {wired}"
        );
    }

    #[test]
    fn a_relatives_words_are_framed_for_the_agent_and_shown_plain() {
        let text = framed("Marie", "Marie", "Une recette avec des poireaux ?");
        assert!(text.starts_with("[Marie, a relative of your person (household « Marie »), writes to you:]\n"));
        assert_eq!(unframed(&text), "Une recette avec des poireaux ?");
        // a person's own bracket line is kept
        assert_eq!(unframed("[note]\nhello"), "[note]\nhello");
    }

    #[test]
    fn only_harmless_tools_answer_during_a_relatives_turn() {
        for friend in [true, false] {
            assert!(closed_tool("knowledge_read", friend).is_none());
            assert!(closed_tool("weather", friend).is_none());
            for name in ["mail_search", "health_data", "calendar_events", "propose_task", "contacts_search", "read_page",
                         "list_pages", "delete_page", "notebook_search"]
            {
                assert_eq!(closed_tool(name, friend).unwrap()["isError"], true, "{name}");
            }
        }
        // a relative talking to a shared agent may have a page made (2026-11-09) and her files read (11/11); another
        // household's agent may not
        for name in ["create_page", "read_sent_file"] {
            assert!(closed_tool(name, true).is_none(), "{name}");
            assert_eq!(closed_tool(name, false).unwrap()["isError"], true, "{name}");
        }
    }

    #[test]
    fn the_move_to_read_only_deletes_the_relatives_conversations_with_personal_agents_only() {
        let mut data: Dict = json!({
            "shared": {"cuisine": ["marie"], "famille": ["marie"]},
            "conversations": {
                "c1": {"agent": "cuisine", "household": "marie", "session": "s1"},
                "c2": {"agent": "famille", "household": "marie", "session": "s2"},
                "c3": {"agent": "cuisine", "household": "paul", "session": "s3"}},
            "tokens": {"marie": {"cuisine": {"input": 1, "output": 2}, "famille": {"input": 3, "output": 4}}},
        })
        .as_object()
        .cloned()
        .unwrap();
        let family = |a: &str| a == "famille";
        let left = read_only_migration(&mut data, &family, &HashSet::new(), 500.0);
        let mut cids: Vec<&str> = left.iter().map(|(c, _, _)| c.as_str()).collect();
        cids.sort();
        assert_eq!(cids, ["c1", "c3"], "to delete in Hermes, the family's kept");
        assert_eq!(left.iter().find(|l| l.0 == "c1").unwrap().2, "s1");
        assert_eq!(data["tokens"]["marie"], json!({"famille": {"input": 3, "output": 4}}));
        assert_eq!(data["history"]["cuisine"]["marie"], json!({"mode": FROM_NOW, "since": 500.0}));
        assert!(data["history"].get("famille").is_none());
        // once deleted in Hermes, their entries go; the shares stay, their choice untouched by a later start
        let gone: HashSet<String> = ["c1".to_string(), "c3".to_string()].into();
        assert!(read_only_migration(&mut data, &family, &gone, 900.0).is_empty());
        let kept: Vec<&String> = data["conversations"].as_object().unwrap().keys().collect();
        assert_eq!(kept, ["c2"]);
        assert_eq!(data["shared"]["cuisine"], json!(["marie"]));
        assert_eq!(data["history"]["cuisine"]["marie"]["since"], 500.0);
        assert!(read_only_migration(&mut data, &family, &HashSet::new(), 900.0).is_empty(), "nothing more to do");
    }

    #[test]
    fn the_share_confirmation_says_what_she_will_read_and_the_private_tools() {
        let config = Config::with_key("k");
        let agent = AgentConfig { name: "cuisine".into(), display_name: "Cuisine".into(), ..Default::default() };
        let plain = confirm_text(&config, &agent, &[]);
        assert!(plain.starts_with("Tes proches pourront lire tes conversations avec Cuisine"), "{plain}");
        assert!(plain.contains("ni lui écrire ni lui demander") && !plain.contains("accès"));
        let private = confirm_text(&config, &agent, &["mail", "calendar", "notebook"]);
        assert!(private.contains("Cuisine a accès à ta messagerie, ton agenda et ton Carnet"), "{private}");
        assert!(private.contains("ils le verront aussi"));
    }

    #[test]
    fn a_relatives_run_is_marked_with_the_bridge_tools_it_keeps_and_web_search_only() {
        let tools = guest_tools(&RELATIVE_TOOLS);
        assert_eq!(
            tools,
            [
                "mcp__bipagents__knowledge_read",
                "mcp__bipagents__knowledge_search",
                "mcp__bipagents__weather",
                "mcp__bipagents__route",
                "mcp__bipagents__map_image",
                "web_search"
            ]
        );
        for closed in ["terminal", "read_file", "session_search", "memory", "browser_navigate", "execute_code"] {
            assert!(!tools.iter().any(|t| t == closed), "{closed}");
        }
        let family = guest_tools(&crate::family_agent::TOOLS);
        assert!(family.contains(&"mcp__bipagents__family_notebook_write".to_string()));
        assert_eq!(crate::hermes::guest_turn(&tools)["tools"][5], "web_search");
    }

    #[test]
    fn two_quick_messages_start_one_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shares.json");
        update(&path, |data| {
            object_mut(data, "conversations").insert("c1".into(), json!({"agent": "vie", "run": null}));
        })
        .unwrap();
        let first = reserve_turn(&path, "c1").unwrap().expect("free");
        assert!(reserve_turn(&path, "c1").unwrap().is_none(), "the second waits for the first");
        assert!(reserve_turn(&path, "ghost").unwrap().is_none());
        settle_turn(&path, "c1", &first, Some("run_1")).unwrap();
        assert_eq!(turn_going(&read(&path)["conversations"]["c1"], now()).as_deref(), Some("run_1"));
        assert!(reserve_turn(&path, "c1").unwrap().is_none());
        // a run that could not start lets the conversation go
        update(&path, |data| {
            object_mut(data, "conversations").insert("c2".into(), json!({"agent": "vie", "run": null}));
        })
        .unwrap();
        let mark = reserve_turn(&path, "c2").unwrap().unwrap();
        settle_turn(&path, "c2", &mark, None).unwrap();
        assert!(reserve_turn(&path, "c2").unwrap().is_some());
    }

    #[test]
    fn a_turn_counts_as_going_on_for_an_hour_at_most() {
        let c = json!({"run": {"id": "r1", "started": 100.0}});
        assert_eq!(turn_going(&c, 200.0).as_deref(), Some("r1"));
        assert_eq!(turn_going(&c, 100.0 + TURN_MAX_SECONDS + 1.0), None);
        assert_eq!(turn_going(&json!({"run": null}), 200.0), None);
    }
}
