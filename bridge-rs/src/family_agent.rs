//! « L'agent de la famille » (decided by Alex on 2026-10-27): one agent common to every member of a family, not only
//! rooms where each member's own agents talk together.
//!
//! **Where it lives.** A member creates it (the Famille screen, or « + Créer un agent » → « Pour toute la famille »);
//! it is a Hermes profile on that member's place, marked in `<data>/family-agent.json`. Every linked household
//! (households.rs: one install per person) gets it at once: it is shared with all of them, always, through the
//! machinery of the agents shared with relatives (agent_shares.rs: `shared_with` lists every linked household for it,
//! new ones included), and their bridges keep it already « added » (`family: true`, no « Ajouter » step; `new` until
//! the person opened it, for the app's « Nouveau : l'agent de la famille » card). A member's bridge also asks its
//! linked households (`GET /family/fa`, at most every `REFRESH_SECONDS`) so a member who joins later, or missed the
//! news, finds it. At most one per family to start: creating one is refused while a linked household has one.
//!
//! **What it reaches.** The family's data, never its creator's. For ALL its turns (its creator's own conversations
//! included) the bridge's MCP tools answer only `TOOLS`: its own memory pages, weather, routes, maps and the family
//! Carnet; mail, calendar, contacts, health, the personal Carnet, archives, files, pages, tasks, family news, cards are
//! closed (`closed_tool`), and `tools/list` gives only those (`tools`). Hermes' own tools that read the place's disk or
//! drive a browser with someone's logins (`agents_admin::DISK_TOOLS`: terminal, file, code_execution, browser) are
//! turned off when it is made, isolated install or not, its visible browser is not set up, and the settings refuse to
//! turn them (or Santé, the Carnet, the browser) back on: with them, the agent could read its creator's files on a
//! shared machine. A member's turn is moreover marked for Hermes (patch 0018, agent_shares.rs Safety): only `TOOLS`
//! and web search, no `session_search` over the creator's own conversations with it, no memory writes, no USER.md.
//! Web search stays (it reads nothing of anyone). Its personality carries a block saying all this (`souls`-style,
//! `with_family`).
//!
//! **The family Carnet** (`<data>/family-notebook.json` on the creator's place): pages, lists (« Courses »), shared
//! tasks and the family's dates (members add the ones they choose to share), readable and editable from every member's
//! app (`/v1/family-agent/notebook`, relayed to the creator's place by the members' bridges) and by the agent
//! (`family_notebook_read`, `family_notebook_write`). Health never goes there (the agent's rule, the app's footer).
//!
//! **Conversations and credit.** The creator talks to it as to any of their agents; the other members through the
//! shared-agent routes (`/v1/shared-agents/{agent}__{household}/…`), each their own conversations, private to them.
//! Its memory is common (the apps say it). The turns run on the creator's place, its model and key: with a Famille
//! plan that key draws from the family's pool. Tokens are counted per member (`/v1/family-agent/usage`).
//!
//! **Its creator leaves.** When the creator's bridge unlinks a household while it holds the family agent, it hands
//! that household a copy of what is the family's (`POST /family/fa/handover`: the agent's name, personality and the
//! family Carnet). That member's app then says « L'agent de la famille était chez Alex, qui a quitté la famille » and
//! offers « Le reprendre » (`POST /v1/family-agent {take_over: true}`): a new family agent on their place, with the same
//! name, personality and Carnet. Nothing is lost silently; the first member who takes it over owns it.
//!
//! Routes for the apps (under /v1, bridge key):
//!
//!     GET  /v1/family-agent              {agent: view|null, can_create, handover: {from, name, at}|null}
//!     POST /v1/family-agent              {name, lang?, tone?, take_over?}  → 201 view   (409: the family has one)
//!     POST /v1/family-agent/seen         {ok}
//!     GET  /v1/family-agent/notebook     notebook view
//!     POST /v1/family-agent/notebook     {op, …} → notebook view
//!     GET  /v1/family-agent/usage        {members: [{id, name, input, output, conversations}]}
//!
//!     view {id, agent, name, category, color, mine, owner, new, since}
//!
//! Between the bridges (the household key):
//!
//!     GET  /family/fa                    {agent, name, category, color, owner}   404: none here
//!     GET  /family/fa/notebook           notebook view
//!     POST /family/fa/notebook           {op, …, person?} → notebook view
//!     GET  /family/fa/usage              usage
//!     POST /family/fa/handover           {from, name, soul, lang, notebook}  the creator left: what is the family's
//!
//! No Python counterpart.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde_json::{json, Map, Value};

use crate::app::AppState;
use crate::config::{AgentConfig, Config};
use crate::households::{Households, SEPARATOR};
use crate::tools_knowledge as tk;
use crate::web::{self, ApiResult, HttpError};
use crate::{py, Dict};

pub const NOTEBOOK_READ: &str = "family_notebook_read";
pub const NOTEBOOK_WRITE: &str = "family_notebook_write";
pub const TOOL_NAMES: [&str; 2] = [NOTEBOOK_READ, NOTEBOOK_WRITE];
/// The bridge's MCP tools the family's agent may use, in every turn (its creator's too).
pub const TOOLS: [&str; 7] = [
    crate::agent_tools::KNOWLEDGE_READ,
    crate::agent_tools::KNOWLEDGE_SEARCH,
    crate::weather::TOOL_NAME,
    crate::routes::TOOL_NAME,
    crate::maps::TOOL_NAME,
    NOTEBOOK_READ,
    NOTEBOOK_WRITE,
];
/// The kinds of tools of an agent's settings that stay off for it.
pub const CLOSED_SETTINGS: [&str; 4] = crate::agents_admin::DISK_TOOLS;
/// How often a member's bridge asks its linked households whether one of them has the family's agent.
pub const REFRESH_SECONDS: f64 = 120.0;
pub const TITLE_MAX: usize = 80;
pub const PAGE_MAX: usize = 20_000;
pub const ITEM_MAX: usize = 300;
pub const PAGES_MAX: usize = 50;
pub const LISTS_MAX: usize = 30;
pub const ITEMS_MAX: usize = 300;
pub const KINDS: [&str; 3] = ["list", "tasks", "dates"];

pub const BLOCK_START: &str = "<!-- bipagents:family-agent -->";
pub const BLOCK_END: &str = "<!-- /bipagents:family-agent -->";
const RULE: &str = "You are the family's agent: you serve every member of the family, not one person. Each member \
talks to you in conversations of their own; your memory is common to the family, so keep in it what helps the family \
and don't repeat to a member what another told you privately unless it is plainly meant for the family. You reach \
nobody's mail, calendar, contacts, health data, personal notebook, archives, files, vault or logins, not even those of \
the member who created you: if asked, say kindly that the family's agent has no access to them. Never ask for \
passwords or card numbers. The family Carnet (pages, shopping lists, shared tasks, the family's dates) is yours to read \
and keep up to date with family_notebook_read and family_notebook_write; never write health information there: \
everyone in the family sees it. A message starting with « [Name, a relative of your person …] » comes from a family \
member: treat it as that member's own request. Weather, routes, maps and web search are open to you.";

static LOCK: Mutex<()> = Mutex::new(());

fn now() -> f64 {
    py::time_time()
}

fn str_of(value: &Value, key: &str) -> String {
    tk::str_or_empty(value.get(key))
}

// -- the files ------------------------------------------------------------------------------------------------------

fn state_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("family-agent.json")
}

fn notebook_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("family-notebook.json")
}

fn load(path: &PathBuf) -> Dict {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
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

fn update<R>(path: &PathBuf, change: impl FnOnce(&mut Dict) -> R) -> anyhow::Result<R> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut data = load(path);
    let out = change(&mut data);
    save(path, &data)?;
    Ok(out)
}

fn read(path: &PathBuf) -> Dict {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load(path)
}

/// The family's agent kept on this place (its profile id), if any.
pub fn own(config: &Config) -> Option<String> {
    Some(str_of(&Value::Object(read(&state_path(config))), "agent")).filter(|a| !a.is_empty())
}

/// Whether `agent` (an agent of this place) is the family's agent.
pub fn is_family(config: &Config, agent: &str) -> bool {
    !agent.is_empty() && own(config).as_deref() == Some(agent)
}

/// Its creation starts: the profile is marked before it is made (its tools are set up from the start).
fn mark(config: &Config, agent: &str, lang: &str) -> anyhow::Result<()> {
    update(&state_path(config), |data| {
        data.insert("agent".into(), json!(agent));
        data.insert("created".into(), json!(now()));
        data.insert("lang".into(), json!(lang));
    })
}

/// The agent deleted (or its creation failed): no family agent here any more. The family Carnet is kept (a new
/// family agent finds it).
pub fn forget(config: &Config, agent: &str) {
    if !is_family(config, agent) {
        return;
    }
    let _ = update(&state_path(config), |data| {
        data.shift_remove("agent");
        data.shift_remove("created");
    });
    tracing::info!(agent = %agent, "family agent: forgotten");
}

// -- what it reaches ------------------------------------------------------------------------------------------------

/// The answer a closed tool gives, in every turn of the family's agent.
pub fn closed_tool(name: &str) -> Option<Dict> {
    if TOOLS.contains(&name) {
        return None;
    }
    Some(tk::tool_text(
        "You are the family's agent: nobody's mail, calendar, contacts, health, personal notebook, archives, files, \
         pages or tasks are open to you, not even your creator's. Answer from what you know, the family Carnet, \
         weather, routes, maps and the web.",
        true,
    ))
}

/// Its `tools/list`: only `TOOLS`.
pub fn tools(config: &Config) -> Vec<Value> {
    let mut out: Vec<Value> = crate::agent_tools::install_tools(config)
        .iter()
        .filter(|t| t.get("name").and_then(Value::as_str).is_some_and(|n| TOOLS.contains(&n)))
        .cloned()
        .collect();
    out.push(crate::weather::weather_tool());
    out.extend([read_tool(), write_tool()]);
    out
}

/// Whether a settings change would open something private to it.
pub fn refused_setting(toolsets: &indexmap::IndexMap<String, bool>, browser: Option<bool>, private: bool) -> bool {
    private
        || browser == Some(true)
        || toolsets.iter().any(|(name, on)| *on && CLOSED_SETTINGS.contains(&name.as_str()))
}

/// Its personality's block.
pub fn with_family(soul: &str, on: bool) -> String {
    let (rest, _) = split_family(soul);
    if !on {
        return rest;
    }
    format!("{}\n\n{BLOCK_START}\n{RULE}\n{BLOCK_END}\n", rest.trim_end())
}

pub fn split_family(soul: &str) -> (String, bool) {
    let (Some(start), Some(end)) = (soul.find(BLOCK_START), soul.find(BLOCK_END)) else {
        return (soul.to_string(), false);
    };
    if end < start {
        return (soul.to_string(), false);
    }
    let rest = format!("{}{}", soul[..start].trim_end(), &soul[end + BLOCK_END.len()..]);
    (format!("{}\n", rest.trim_end()), true)
}

/// The run's instructions for a family member's turn (instead of agent_shares' « a relative of your person »).
pub fn member_instructions(person: &str) -> String {
    format!(
        "You are the family's agent and {person}, a member of the family, is talking to you now in a conversation of \
their own. Treat what they write as {person}'s own requests. Nobody's mail, calendar, contacts, health data, personal \
notebook, archives, files, vault or logins are open to you: don't use the terminal, file or browser tools, and say \
kindly that it isn't possible if asked. Don't tell {person} what other members said to you in their own \
conversations; what is plainly the family's (the family Carnet, plans everyone knows) may help. Actions that need \
approval are asked to {person}, who decides. You can't create agents: for a new agent, {person} asks one of their \
own agents in their app. Reply with text only. {}",
        crate::souls::CONTENT_RULE
    )
}

// -- the family Carnet ----------------------------------------------------------------------------------------------

fn words(lang: &str) -> [&'static str; 3] {
    match lang.get(..2).unwrap_or("fr") {
        "en" => ["Shopping", "Tasks", "Dates"],
        "de" => ["Einkäufe", "Aufgaben", "Termine"],
        "es" => ["Compras", "Tareas", "Fechas"],
        _ => ["Courses", "Tâches", "Dates"],
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", py::token_hex(4))
}

fn cut(value: Option<&Value>, max: usize) -> String {
    tk::strip(&tk::str_or_empty(value)).chars().take(max).collect()
}

/// A new Carnet's three lists, when it has none yet.
fn seed(config: &Config, lang: &str, by: &str) -> anyhow::Result<()> {
    update(&notebook_path(config), |data| {
        if data.get("lists").and_then(Value::as_array).is_some_and(|l| !l.is_empty()) {
            return;
        }
        let at = now();
        let lists: Vec<Value> = words(lang)
            .iter()
            .zip(KINDS)
            .map(|(title, kind)| {
                json!({"id": new_id("l"), "title": title, "kind": kind, "items": [], "updated": at, "by": by})
            })
            .collect();
        data.insert("lists".into(), json!(lists));
        data.entry("pages").or_insert(json!([]));
        data.insert("updated".into(), json!(at));
    })
}

/// The Carnet as the apps get it.
pub fn notebook_view(config: &Config) -> Value {
    view_of(&read(&notebook_path(config)))
}

fn view_of(data: &Dict) -> Value {
    json!({"pages": data.get("pages").cloned().unwrap_or(json!([])),
           "lists": data.get("lists").cloned().unwrap_or(json!([])),
           "updated": data.get("updated").cloned().unwrap_or(json!(0))})
}

fn due_of(value: Option<&Value>) -> Result<Value, String> {
    let text = tk::strip(&tk::str_or_empty(value)).to_string();
    if text.is_empty() {
        return Ok(Value::Null);
    }
    chrono::NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        .map(|_| json!(text))
        .map_err(|_| "due: a date YYYY-MM-DD".to_string())
}

fn array_mut<'a>(data: &'a mut Dict, key: &str) -> &'a mut Vec<Value> {
    if !matches!(data.get(key), Some(Value::Array(_))) {
        data.insert(key.into(), json!([]));
    }
    match data.get_mut(key) {
        Some(Value::Array(items)) => items,
        _ => unreachable!("an array was just put there"),
    }
}

/// One change of the Carnet (`op`), by `by` (a member's name, or the agent's). Err: a message for the app / agent.
pub fn apply(data: &mut Dict, body: &Dict, by: &str, at: f64) -> Result<(), String> {
    let op = tk::str_or_empty(body.get("op"));
    let id = cut(body.get("id"), 40);
    let find = |items: &[Value], id: &str| items.iter().position(|v| str_of(v, "id") == id);
    match op.as_str() {
        "page_save" => {
            let title = cut(body.get("title"), TITLE_MAX);
            let text: String = tk::str_or_empty(body.get("text")).chars().take(PAGE_MAX).collect();
            let pages = array_mut(data, "pages");
            if id.is_empty() {
                if title.is_empty() {
                    return Err("title: 1 to 80 characters".into());
                }
                if pages.len() >= PAGES_MAX {
                    return Err(format!("at most {PAGES_MAX} pages: delete one first"));
                }
                pages.push(json!({"id": new_id("p"), "title": title, "text": text, "updated": at, "by": by}));
            } else {
                let at_index = find(pages, &id).ok_or("no such page")?;
                let page = &mut pages[at_index];
                if !title.is_empty() {
                    page["title"] = json!(title);
                }
                if body.contains_key("text") {
                    page["text"] = json!(text);
                }
                page["updated"] = json!(at);
                page["by"] = json!(by);
            }
        }
        "page_delete" => {
            let pages = array_mut(data, "pages");
            let at_index = find(pages, &id).ok_or("no such page")?;
            pages.remove(at_index);
        }
        "list_save" => {
            let title = cut(body.get("title"), TITLE_MAX);
            let kind = Some(tk::str_or_empty(body.get("kind"))).filter(|k| !k.is_empty());
            if kind.as_deref().is_some_and(|k| !KINDS.contains(&k)) {
                return Err("kind: list, tasks or dates".into());
            }
            let lists = array_mut(data, "lists");
            if id.is_empty() {
                if title.is_empty() {
                    return Err("title: 1 to 80 characters".into());
                }
                if lists.len() >= LISTS_MAX {
                    return Err(format!("at most {LISTS_MAX} lists: delete one first"));
                }
                lists.push(json!({"id": new_id("l"), "title": title, "kind": kind.unwrap_or("list".into()),
                                  "items": [], "updated": at, "by": by}));
            } else {
                let at_index = find(lists, &id).ok_or("no such list")?;
                let list = &mut lists[at_index];
                if !title.is_empty() {
                    list["title"] = json!(title);
                }
                if let Some(kind) = kind {
                    list["kind"] = json!(kind);
                }
                list["updated"] = json!(at);
            }
        }
        "list_delete" => {
            let lists = array_mut(data, "lists");
            let at_index = find(lists, &id).ok_or("no such list")?;
            lists.remove(at_index);
        }
        "item_add" | "item_save" | "item_delete" | "clear_done" => {
            let list_id = cut(body.get("list"), 40);
            let lists = array_mut(data, "lists");
            let at_index = find(lists, &list_id).ok_or("no such list")?;
            let list = &mut lists[at_index];
            if !matches!(list.get("items"), Some(Value::Array(_))) {
                list["items"] = json!([]);
            }
            let Some(Value::Array(items)) = list.get_mut("items") else { unreachable!("an array was just put there") };
            match op.as_str() {
                "item_add" => {
                    let text = cut(body.get("text"), ITEM_MAX);
                    if text.is_empty() {
                        return Err("text: 1 to 300 characters".into());
                    }
                    if items.len() >= ITEMS_MAX {
                        return Err(format!("at most {ITEMS_MAX} items: clear the ticked ones first"));
                    }
                    let due = due_of(body.get("due"))?;
                    items.push(json!({"id": new_id("i"), "text": text, "done": false, "due": due, "by": by,
                                      "at": at}));
                }
                "item_save" => {
                    let item_index = find(items, &id).ok_or("no such item")?;
                    let item = &mut items[item_index];
                    let text = cut(body.get("text"), ITEM_MAX);
                    if !text.is_empty() {
                        item["text"] = json!(text);
                    }
                    if let Some(done) = body.get("done") {
                        item["done"] = json!(py::truthy(done));
                    }
                    if body.contains_key("due") {
                        item["due"] = due_of(body.get("due"))?;
                    }
                    item["by"] = json!(by);
                    item["at"] = json!(at);
                }
                "item_delete" => {
                    let item_index = find(items, &id).ok_or("no such item")?;
                    items.remove(item_index);
                }
                _ => items.retain(|i| !i.get("done").is_some_and(py::truthy)),
            }
            list["updated"] = json!(at);
        }
        _ => {
            return Err("op: page_save, page_delete, list_save, list_delete, item_add, item_save, item_delete or \
                        clear_done"
                .into())
        }
    }
    data.insert("updated".into(), json!(at));
    Ok(())
}

/// A change of the Carnet on this place; Ok(view) or Err(message).
pub fn change(config: &Config, body: &Dict, by: &str) -> anyhow::Result<Result<Value, String>> {
    update(&notebook_path(config), |data| apply(data, body, by, now()).map(|_| view_of(data)))
}

/// The Carnet as plain text for the agent.
pub fn as_text(view: &Value) -> String {
    let mut out = Vec::new();
    for list in view["lists"].as_array().into_iter().flatten() {
        out.push(format!("## {} ({}, list id {})", str_of(list, "title"), str_of(list, "kind"), str_of(list, "id")));
        let items = list["items"].as_array().cloned().unwrap_or_default();
        if items.is_empty() {
            out.push("(empty)".into());
        }
        for item in items {
            let tick = if item.get("done").is_some_and(py::truthy) { "x" } else { " " };
            let due =
                Some(str_of(&item, "due")).filter(|d| !d.is_empty()).map(|d| format!(" 📅 {d}")).unwrap_or_default();
            out.push(format!(
                "- [{tick}] {}{due} (id {}, by {})",
                str_of(&item, "text"),
                str_of(&item, "id"),
                str_of(&item, "by")
            ));
        }
    }
    for page in view["pages"].as_array().into_iter().flatten() {
        out.push(format!(
            "## Page « {} » (page id {})\n{}",
            str_of(page, "title"),
            str_of(page, "id"),
            str_of(page, "text")
        ));
    }
    if out.is_empty() {
        "The family Carnet is empty.".into()
    } else {
        out.join("\n")
    }
}

fn read_tool() -> Value {
    json!({
        "name": NOTEBOOK_READ,
        "description": "Read the family Carnet, shared by every member of the family and editable in their app: its \
    lists (shopping « Courses », shared tasks, the family's dates) with their items and ids, and its pages.",
        "inputSchema": {"type": "object", "properties": {}},
    })
}

fn write_tool() -> Value {
    json!({
        "name": NOTEBOOK_WRITE,
        "description": "Change the family Carnet (every member sees it at once in their app; never write health \
    information there). op: item_add {list, text, due?} adds to a list (due YYYY-MM-DD for a task or a date); \
    item_save {list, id, text?, done?, due?}; item_delete {list, id}; clear_done {list}; list_save {id?, title, kind?: \
    list|tasks|dates} creates (no id) or renames; list_delete {id}; page_save {id?, title, text} creates (no id) or \
    rewrites a page; page_delete {id}. Ids come from family_notebook_read.",
        "inputSchema": {"type": "object", "properties": {
            "op": {"type": "string", "enum": ["item_add", "item_save", "item_delete", "clear_done", "list_save",
                                              "list_delete", "page_save", "page_delete"]},
            "list": {"type": "string"}, "id": {"type": "string"}, "title": {"type": "string"},
            "text": {"type": "string"}, "kind": {"type": "string", "enum": KINDS},
            "done": {"type": "boolean"}, "due": {"type": "string", "description": "YYYY-MM-DD"}},
            "required": ["op"]},
    })
}

/// Its two Carnet tools.
pub fn call_tool(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> anyhow::Result<Dict> {
    if name == NOTEBOOK_READ {
        return Ok(tk::tool_text(&as_text(&notebook_view(config)), false));
    }
    let by = if agent.display_name.is_empty() { agent.name.clone() } else { agent.display_name.clone() };
    Ok(match change(config, arguments, &by)? {
        Ok(view) => tk::tool_text(&format!("Done. The Carnet now:\n{}", as_text(&view)), false),
        Err(message) => tk::tool_text(&message, true),
    })
}

// -- a member's side: the family's agent of a linked household ---------------------------------------------------------

fn households(config: &Config) -> Households {
    Households::new(&crate::households::path_for(config))
}

async fn ask(
    state: &AppState,
    household: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Option<(u16, Value)> {
    let entry = state.households.get(household)?;
    let url = format!("{}/family/fa{path}", entry.get("url").map(py::py_str).unwrap_or_default());
    let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
    let http = crate::door_tls::household_client(&state.http, &entry);
    let mut request = http.request(method, url).timeout(Duration::from_secs(15)).bearer_auth(key);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let resp = request.send().await.ok()?;
    let status = resp.status().as_u16();
    Some((status, resp.json::<Value>().await.unwrap_or(Value::Null)))
}

/// The family's agent this person uses from a linked household: (shared id, entry).
fn known(config: &Config) -> Option<(String, Value)> {
    crate::agent_shares::shared_with_me(config)
        .into_iter()
        .find(|a| a.get("family").is_some_and(py::truthy))
        .map(|a| (str_of(&a, "id"), a))
}

/// Asks the linked households whether one of them has the family's agent (at most every `REFRESH_SECONDS`, unless
/// `force`), and keeps what they say among the agents shared with this person.
pub async fn refresh(state: &AppState, force: bool) {
    let config = state.config();
    let at = now();
    let last = read(&state_path(&config)).get("checked").and_then(Value::as_f64).unwrap_or(0.0);
    if !force && at - last < REFRESH_SECONDS {
        return;
    }
    let _ = update(&state_path(&config), |data| data.insert("checked".into(), json!(at)));
    for h in households(&config).public() {
        let household = h.get("id").map(py::py_str).unwrap_or_default();
        let Some((status, answer)) = ask(state, &household, reqwest::Method::GET, "", None).await else { continue };
        let agent = str_of(&answer, "agent");
        if status == 200 && !agent.is_empty() {
            let offer = json!({"agent": agent, "name": answer.get("name"), "category": answer.get("category"),
                               "color": answer.get("color"), "shared": true, "family": true});
            let _ = crate::agent_shares::receive_offer(&config, &household, offer.as_object().unwrap_or(&Map::new()));
        } else if status == 404 {
            crate::agent_shares::drop_family_offers(&config, &household);
        }
    }
}

// -- the views --------------------------------------------------------------------------------------------------------

fn own_view(config: &Config, agent: &AgentConfig) -> Value {
    let created = read(&state_path(config)).get("created").cloned().unwrap_or(json!(0));
    json!({"id": agent.name, "agent": agent.name, "name": agent.display_name, "category": agent.category,
           "color": agent.color, "mine": true, "owner": "", "new": false, "since": created})
}

fn member_view(id: &str, entry: &Value) -> Value {
    json!({"id": id, "agent": entry.get("agent"), "name": entry.get("name"), "category": entry.get("category"),
           "color": entry.get("color"), "mine": false, "owner": entry.get("owner"),
           "new": entry.get("new").is_some_and(py::truthy), "since": entry.get("since")})
}

fn handover_view(config: &Config) -> Value {
    match read(&state_path(config)).get("handover") {
        Some(Value::Object(h)) => json!({"from": h.get("from"), "name": h.get("name"), "at": h.get("at")}),
        _ => Value::Null,
    }
}

fn own_agent(config: &Config) -> Option<AgentConfig> {
    own(config).and_then(|id| config.agent(Some(&id)).cloned())
}

async fn get_view(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    if let Some(agent) = own_agent(&config) {
        return Ok(web::ok(json!({"agent": own_view(&config, &agent), "can_create": false, "handover": null})));
    }
    refresh(&state, false).await;
    let config = state.config();
    match known(&config) {
        Some((id, entry)) => {
            Ok(web::ok(json!({"agent": member_view(&id, &entry), "can_create": false, "handover": null})))
        }
        None => Ok(web::ok(json!({"agent": null, "can_create": config.hermes.enabled(),
                                  "handover": handover_view(&config)}))),
    }
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

fn role(lang: &str) -> &'static str {
    match lang.get(..2).unwrap_or("fr") {
        "fr" => {
            "l'agent de toute la famille : les courses, les tâches partagées, les dates de la famille, les \
                 sorties, les repas, l'organisation de la maison"
        }
        _ => {
            "the whole family's agent: shopping, shared tasks, the family's dates, outings, meals, organising the home"
        }
    }
}

/// The family's agent deleted by its creator: the members' bridges are told (their apps drop it at once; else their
/// next `refresh` finds it gone).
pub fn deleted(state: &AppState, agent: &AgentConfig) {
    let (state_c, agent) = (state.clone(), agent.clone());
    state.spawn(async move { tell_all(&state_c, &agent, false).await });
}

/// Tells the linked households the family's agent is there (their apps show it at once).
async fn tell_all(state: &AppState, agent: &AgentConfig, shared: bool) {
    let config = state.config();
    for h in households(&config).public() {
        let household = h.get("id").map(py::py_str).unwrap_or_default();
        let Some(entry) = state.households.get(&household) else { continue };
        let url = entry.get("url").map(py::py_str).unwrap_or_default();
        let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
        let body = json!({"agent": agent.name, "name": agent.display_name, "category": agent.category,
                          "color": agent.color, "shared": shared, "family": true});
        let told = crate::door_tls::household_client(&state.http, &entry)
            .post(format!("{url}/family/shared"))
            .timeout(Duration::from_secs(10))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        tracing::info!(household = %household, told = told, "family agent: news sent");
    }
}

async fn create(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    if !config.hermes.enabled() {
        return Err(HttpError::new(409, "this install does not manage its agents (set up by hand)"));
    }
    let body = body_json(&body)?;
    let take_over = body.get("take_over").is_some_and(py::truthy);
    let handover = match read(&state_path(&config)).get("handover") {
        Some(Value::Object(h)) => Some(h.clone()),
        _ => None,
    };
    if take_over && handover.is_none() {
        return Err(HttpError::new(404, "nothing to take over"));
    }
    let given = tk::squeeze(&tk::str_or_empty(body.get("name")));
    let name = match (&handover, take_over) {
        (Some(h), true) if given.is_empty() => tk::str_or_empty(h.get("name")),
        _ => given,
    };
    if name.is_empty() || name.chars().count() > 40 {
        return Err(HttpError::new(422, "name: 1 to 40 characters"));
    }
    let lang = Some(tk::str_or_empty(body.get("lang")))
        .filter(|l| !l.is_empty())
        .or_else(|| handover.as_ref().map(|h| tk::str_or_empty(h.get("lang"))).filter(|l| !l.is_empty()))
        .unwrap_or_else(|| "fr".into());
    let tone: String = tk::str_or_empty(body.get("tone")).chars().take(200).collect();
    let _guard = crate::agents_admin::LOCK.lock().await;
    if own_agent(&config).is_some() {
        return Err(HttpError::new(409, "the family already has its agent"));
    }
    refresh(&state, true).await;
    if known(&state.config()).is_some() {
        return Err(HttpError::new(409, "the family already has its agent"));
    }
    let request = crate::agents_admin::NewAgent {
        name: name.clone(),
        lang: Some(lang.clone()),
        custom: Some(crate::agents_admin::CustomAgent {
            role: role(&lang).into(),
            tone,
            rules: String::new(),
            language: String::new(),
            category: "home".into(),
        }),
        ..Default::default()
    };
    let planned = crate::agents_admin::plan_agent(&config, &request).map_err(|e| HttpError::new(400, e.to_string()))?;
    mark(&config, &planned.profile, &lang)?;
    let made =
        crate::agents_admin::create_agent(&state.config, &request, &state.http, &crate::agents_admin::default_run())
            .await;
    let agent = match made {
        Ok(agent) if agent.name == planned.profile => agent,
        Ok(agent) => {
            // another name was taken meanwhile: the mark follows the agent made
            mark(&state.config(), &agent.name, &lang)?;
            agent
        }
        Err(exc) => {
            forget(&state.config(), &planned.profile);
            tracing::warn!(error = %format!("{exc:#}"), "family agent: creation failed");
            return Err(HttpError::new(502, "the agent could not be created: try again in a moment"));
        }
    };
    let config = state.config();
    let by = {
        let n = state.households.name();
        if n.is_empty() {
            name.clone()
        } else {
            n
        }
    };
    // its personality with its block (and the handed-over one when it is taken over)
    let soul = match (&handover, take_over) {
        (Some(h), true) if !tk::str_or_empty(h.get("soul")).trim().is_empty() => tk::str_or_empty(h.get("soul")),
        _ => crate::agents_admin::editable_soul(&config, &agent).unwrap_or_default(),
    };
    if let Err(err) = crate::agents_admin::write_soul(&config, &agent, &soul) {
        tracing::warn!(error = %err, "family agent: its personality was not written");
    }
    if take_over {
        if let Some(Value::Object(notebook)) = handover.as_ref().and_then(|h| h.get("notebook")) {
            let kept = Map::from_iter(
                ["pages", "lists"].iter().filter_map(|k| notebook.get(*k).map(|v| (k.to_string(), v.clone()))),
            );
            update(&notebook_path(&config), |data| {
                for (k, v) in kept {
                    data.insert(k, v);
                }
                data.insert("updated".into(), json!(now()));
            })?;
        }
        update(&state_path(&config), |data| data.shift_remove("handover"))?;
    }
    seed(&config, &lang, &by)?;
    tell_all(&state, &agent, true).await;
    tracing::info!(agent = %agent.name, take_over = take_over, "family agent: created");
    crate::own_space::apart_when_idle(&state, &agent.name); // the family talks to it: out of the person's hub
    Ok(web::json_response(201, &own_view(&config, &agent)))
}

async fn seen(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    if let Some((id, _)) = known(&config) {
        crate::agent_shares::family_seen(&config, &id)?;
    }
    Ok(web::ok(json!({"ok": true})))
}

/// The household holding the family's agent this person uses (None: it is this place's own).
fn holder(config: &Config) -> ApiResult<Option<String>> {
    if own(config).is_some() {
        return Ok(None);
    }
    match known(config) {
        Some((_, entry)) => Ok(Some(str_of(&entry, "household"))),
        None => Err(HttpError::new(404, "no family agent")),
    }
}

async fn relayed(
    state: &AppState,
    household: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> ApiResult<Response> {
    match ask(state, household, method, path, body).await {
        Some((status, payload)) => Ok(web::json_response(status, &payload)),
        None => Err(HttpError::new(502, "their side does not answer right now: try again later")),
    }
}

async fn get_notebook(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    match holder(&config)? {
        None => Ok(web::ok(notebook_view(&config))),
        Some(household) => relayed(&state, &household, reqwest::Method::GET, "/notebook", None).await,
    }
}

fn changed(config: &Config, body: &Dict, by: &str) -> ApiResult<Response> {
    match change(config, body, by)? {
        Ok(view) => Ok(web::ok(view)),
        Err(message) => Err(HttpError::new(400, message)),
    }
}

async fn post_notebook(State(state): State<AppState>, body: Bytes) -> ApiResult<Response> {
    let config = state.config();
    let mut body = body_json(&body)?;
    let person = state.households.name();
    match holder(&config)? {
        None => changed(&config, &body, if person.is_empty() { "?" } else { &person }),
        Some(household) => {
            body.insert("person".into(), json!(person));
            relayed(&state, &household, reqwest::Method::POST, "/notebook", Some(Value::Object(body))).await
        }
    }
}

/// Tokens per member: the creator's own conversations (Hermes' sessions not opened for someone else) and each
/// household's (agent_shares.rs counts them).
async fn usage_of(state: &AppState, agent: &AgentConfig) -> Value {
    let config = state.config();
    let shares = crate::agent_shares::share_view(&config, agent);
    let mut members = Vec::new();
    let sessions = state.hermes.list_sessions(agent, 200).await.unwrap_or_default();
    let mine: Vec<&Dict> =
        sessions.iter().filter(|s| !tk::str_or_empty(s.get("title")).starts_with("BipAgents · ")).collect();
    let (mut input, mut output) = (0i64, 0i64);
    for s in &mine {
        let c = crate::jobs::counters(s);
        input += c.get("input").and_then(Value::as_i64).unwrap_or(0);
        output += c.get("output").and_then(Value::as_i64).unwrap_or(0);
    }
    let name = state.households.name();
    members.push(json!({"id": "self", "name": name, "input": input, "output": output, "conversations": mine.len()}));
    let conversations = shares["conversations"].as_array().cloned().unwrap_or_default();
    for t in shares["tokens"].as_array().into_iter().flatten() {
        let household = str_of(t, "household");
        let count = conversations.iter().filter(|c| str_of(c, "household") == household).count();
        members.push(json!({"id": household, "name": t.get("household_name"), "input": t.get("input"),
                            "output": t.get("output"), "conversations": count}));
    }
    json!({"members": members})
}

async fn usage(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    match holder(&config)? {
        None => {
            let agent = own_agent(&config).ok_or_else(|| HttpError::new(404, "no family agent"))?;
            Ok(web::ok(usage_of(&state, &agent).await))
        }
        Some(household) => relayed(&state, &household, reqwest::Method::GET, "/usage", None).await,
    }
}

/// For the apps (mounted under /v1, bridge key).
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/family-agent", get(get_view).post(create))
        .route("/family-agent/seen", post(seen))
        .route("/family-agent/notebook", get(get_notebook).post(post_notebook))
        .route("/family-agent/usage", get(usage))
}

// -- between the bridges ------------------------------------------------------------------------------------------------

fn household_of(state: &AppState, headers: &HeaderMap) -> ApiResult<String> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = header.strip_prefix("Bearer ").unwrap_or(header).trim();
    let household = if token.is_empty() { None } else { state.households.caller(token) };
    household.ok_or_else(|| HttpError::new(401, "unknown household"))
}

fn served(state: &AppState, headers: &HeaderMap) -> ApiResult<(String, AgentConfig)> {
    let household = household_of(state, headers)?;
    let agent = own_agent(&state.config()).ok_or_else(|| HttpError::new(404, "no family agent here"))?;
    Ok((household, agent))
}

async fn served_view(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let (_, agent) = served(&state, &headers)?;
    Ok(web::ok(json!({"agent": agent.name, "name": agent.display_name, "category": agent.category,
                      "color": agent.color, "owner": state.households.name()})))
}

async fn served_notebook(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    served(&state, &headers)?;
    Ok(web::ok(notebook_view(&state.config())))
}

async fn served_change(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let (household, _) = served(&state, &headers)?;
    let body = body_json(&body)?;
    let config = state.config();
    let shown = households(&config)
        .get(&household)
        .and_then(|h| h.get("name").map(py::py_str))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| household.clone());
    let person: String = Some(tk::squeeze(&tk::str_or_empty(body.get("person"))))
        .filter(|p| !p.is_empty())
        .unwrap_or(shown)
        .chars()
        .take(40)
        .collect();
    changed(&config, &body, &person)
}

async fn served_usage(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let (_, agent) = served(&state, &headers)?;
    Ok(web::ok(usage_of(&state, &agent).await))
}

/// The creator left: what is the family's, kept here until this person takes it over.
async fn handed_over(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> ApiResult<Response> {
    let household = household_of(&state, &headers)?;
    let body = body_json(&body)?;
    let config = state.config();
    if own(&config).is_some() {
        return Ok(web::ok(json!({"ok": true, "kept": false}))); // this person already holds a family agent
    }
    let notebook = body.get("notebook").cloned().filter(Value::is_object).unwrap_or(json!({}));
    let entry = json!({"household": household,
                       "from": tk::str_or_empty(body.get("from")).chars().take(60).collect::<String>(),
                       "name": tk::str_or_empty(body.get("name")).chars().take(40).collect::<String>(),
                       "soul": tk::str_or_empty(body.get("soul")).chars().take(20_000).collect::<String>(),
                       "lang": tk::str_or_empty(body.get("lang")).chars().take(10).collect::<String>(),
                       "notebook": notebook, "at": now()});
    update(&state_path(&config), |data| data.insert("handover".into(), entry))?;
    crate::agent_shares::drop_family_offers(&config, &household);
    tracing::info!(household = %household, "family agent: handed over to this place");
    Ok(web::ok(json!({"ok": true, "kept": true})))
}

/// What a linked household's bridge may call (its key). Mounted at the root.
pub fn household_router() -> Router<AppState> {
    Router::new()
        .route("/family/fa", get(served_view))
        .route("/family/fa/notebook", get(served_notebook).post(served_change))
        .route("/family/fa/usage", get(served_usage))
        .route("/family/fa/handover", post(handed_over))
}

/// A household unlinked by this person (peers.rs): if this place holds the family's agent, that member gets a copy of
/// what is the family's (its name, personality, Carnet) so it is never lost silently. `entry`: the household as it was.
pub fn after_unlink(state: &AppState, household: &str, entry: Option<Dict>) {
    let config = state.config();
    let (Some(agent), Some(entry)) = (own_agent(&config), entry) else { return };
    let soul = crate::agents_admin::editable_soul(&config, &agent).unwrap_or_default();
    let lang = tk::str_or_empty(read(&state_path(&config)).get("lang"));
    let body = json!({"from": state.households.name(), "name": agent.display_name, "soul": soul, "lang": lang,
                      "notebook": notebook_view(&config)});
    let http = state.http.clone();
    let household = household.to_string();
    state.spawn(async move {
        let url = entry.get("url").map(py::py_str).unwrap_or_default();
        let key = entry.get("key_out").map(py::py_str).unwrap_or_default();
        let sent = crate::door_tls::household_client(&http, &entry)
            .post(format!("{url}/family/fa/handover"))
            .timeout(Duration::from_secs(15))
            .bearer_auth(key)
            .json(&body)
            .send()
            .await
            .is_ok_and(|r| r.status().is_success());
        tracing::info!(household = %household, sent = sent, "family agent: handed over on leaving");
    });
}

/// `agent__household` is the family's agent of that linked household (rooms: no link needed for it).
pub fn is_family_of_household(config: &Config, id: &str) -> bool {
    id.contains(SEPARATOR) && known(config).is_some_and(|(known_id, _)| known_id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(v: Value) -> Dict {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn the_carnet_takes_pages_lists_and_items() {
        let mut data = Dict::new();
        apply(&mut data, &body(json!({"op": "list_save", "title": "Courses"})), "Alex", 1.0).unwrap();
        let list = str_of(&data["lists"][0], "id");
        assert_eq!(data["lists"][0]["kind"], "list");
        apply(&mut data, &body(json!({"op": "item_add", "list": list, "text": "Lait"})), "Léa", 2.0).unwrap();
        apply(
            &mut data,
            &body(json!({"op": "item_add", "list": list, "text": "Pain", "due": "2026-11-02"})),
            "Léa",
            2.0,
        )
        .unwrap();
        let item = str_of(&data["lists"][0]["items"][0], "id");
        apply(&mut data, &body(json!({"op": "item_save", "list": list, "id": item, "done": true})), "Alex", 3.0)
            .unwrap();
        assert_eq!(data["lists"][0]["items"][0]["done"], true);
        assert_eq!(data["lists"][0]["items"][1]["due"], "2026-11-02");
        apply(&mut data, &body(json!({"op": "clear_done", "list": list})), "Alex", 4.0).unwrap();
        assert_eq!(data["lists"][0]["items"].as_array().unwrap().len(), 1);
        assert!(apply(
            &mut data,
            &body(json!({"op": "item_add", "list": list, "text": "x", "due": "demain"})),
            "A",
            5.0
        )
        .is_err());
        assert!(apply(&mut data, &body(json!({"op": "list_save", "title": "X", "kind": "health"})), "A", 5.0).is_err());
        apply(&mut data, &body(json!({"op": "page_save", "title": "Vacances", "text": "Bretagne"})), "Alex", 6.0)
            .unwrap();
        let page = str_of(&data["pages"][0], "id");
        apply(&mut data, &body(json!({"op": "page_save", "id": page, "text": "Bretagne, juillet"})), "Léa", 7.0)
            .unwrap();
        assert_eq!(data["pages"][0]["title"], "Vacances");
        assert_eq!(data["pages"][0]["by"], "Léa");
        let text = as_text(&view_of(&data));
        assert!(text.contains("Pain 📅 2026-11-02") && text.contains("Bretagne, juillet"));
        apply(&mut data, &body(json!({"op": "page_delete", "id": page})), "Léa", 8.0).unwrap();
        assert!(apply(&mut data, &body(json!({"op": "nope"})), "A", 9.0).is_err());
    }

    #[test]
    fn only_the_familys_tools_answer() {
        for open in ["weather", "family_notebook_read", "family_notebook_write", "knowledge_read"] {
            assert!(closed_tool(open).is_none(), "{open}");
        }
        for name in [
            "mail_search",
            "health_data",
            "calendar_events",
            "contacts_search",
            "notebook_read",
            "create_page",
            "propose_task",
            "tell_family",
            "archives_search",
            "upcoming_dates",
        ] {
            assert_eq!(closed_tool(name).unwrap()["isError"], true, "{name}");
        }
        let mut toolsets = indexmap::IndexMap::new();
        toolsets.insert("web".to_string(), true);
        assert!(!refused_setting(&toolsets, None, false));
        toolsets.insert("terminal".to_string(), true);
        assert!(refused_setting(&toolsets, None, false));
        assert!(refused_setting(&indexmap::IndexMap::new(), Some(true), false));
    }

    #[test]
    fn its_block_comes_and_goes() {
        let soul = with_family("Tu es Famille.\n", true);
        assert!(soul.contains(BLOCK_START) && soul.contains("not even those of the member who created you"));
        let (rest, on) = split_family(&soul);
        assert!(on);
        assert_eq!(rest, "Tu es Famille.\n");
        assert_eq!(with_family(&soul, true).matches(BLOCK_START).count(), 1);
    }
}
