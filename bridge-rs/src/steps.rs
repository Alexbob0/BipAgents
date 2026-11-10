//! « Étapes » (Alex 5/11, built 6/11): the task list an agent keeps for work of several steps (Hermes' `todo_list`
//! tool, its soul's `STEPS_RULE`), shown to the person as one compact card per turn: live while the turn runs, folded
//! after (« 5/5 étapes »). docs/conversations.md §6.3b.
//!
//! - **Stored**: the timeline gives one `steps` item per turn, the list as the turn's last `todo_list` call left it
//!   (the call's result is always the whole list), where that call is; the calls themselves are never `tool` items
//!   (threads/timeline.rs makes one `steps` item per call, [`fold`] keeps the turn's last). From state.db, so it needs
//!   nothing of Hermes but its rows: with or without patch 0026 the card is there once the turn's rows are.
//! - **Live**: Hermes' `todo.updated` run event (patch 0026, the TUI gateway's event backported to the Runs API)
//!   becomes the bridge's `steps.updated` in the run's stream (runs.rs), the open screens' `thread` hint
//!   (`reason: steps`, version not bumped), a background task's `activity.steps` (jobs.rs) and the Live Activity's
//!   step (live_activities.rs).

use std::collections::HashMap;

use serde_json::{json, Map, Value};

use crate::threads::timeline::Item;

/// The bridge's run event carrying a turn's list (made from Hermes' `todo.updated`).
pub const EVENT: &str = "steps.updated";
/// Hermes' run event (patch 0026; the same name and payload as its TUI gateway's and v0.23's).
pub const HERMES_EVENT: &str = "todo.updated";
/// A step's words at most (Hermes allows 4,000: a card line, not a page).
pub const TEXT_MAX: usize = 200;
/// Steps at most in a card (Hermes allows 256).
pub const STEPS_MAX: usize = 30;
/// Sub-steps deeper than this are drawn at this depth.
const DEPTH_MAX: u64 = 3;

/// Hermes' Todo tool: its name, and the one it had before (old conversations replay it).
pub fn is_todo_tool(name: &str) -> bool {
    matches!(crate::loop_brake::bare(name), "todo_list" | "todo")
}

/// A call of the Todo tool: directly, or through Hermes' tool search (`tool_call` wrapping exactly that one call,
/// `{"calls": [{"name": "todo_list", …}]}` or the older `{"name": "todo_list", …}`).
pub fn is_todo_call(name: &str, arguments: &str) -> bool {
    if is_todo_tool(name) {
        return true;
    }
    if name != "tool_call" || !arguments.contains("todo") {
        return false;
    }
    let Ok(args) = serde_json::from_str::<Value>(arguments) else { return false };
    let mut calls = match args.get("calls") {
        Some(Value::Array(calls)) => calls.clone(),
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(calls)) => calls,
            Ok(one @ Value::Object(_)) => vec![one],
            _ => return false,
        },
        Some(one @ Value::Object(_)) => vec![one.clone()],
        None => vec![args.clone()],
        Some(_) => return false,
    };
    calls.len() == 1 && calls.pop().and_then(|c| c.get("name").and_then(Value::as_str).map(is_todo_tool)) == Some(true)
}

/// Hermes' todo items (`{id, content, status, parent?}`) → the card's list: `{items: [{text, status, depth?}], done,
/// total}`. `status` in the apps' words (`completed` is `done`, anything unknown `pending`); `total` leaves cancelled
/// steps out.
pub fn of_todos(todos: &[Value]) -> Value {
    let parents: HashMap<&str, &str> = todos
        .iter()
        .filter_map(|t| Some((t.get("id")?.as_str()?, t.get("parent")?.as_str()?)))
        .filter(|(id, parent)| !parent.is_empty() && id != parent)
        .collect();
    let depth = |id: Option<&str>| {
        let (mut depth, mut at) = (0u64, id);
        while let Some(parent) = at.and_then(|i| parents.get(i)) {
            depth += 1;
            if depth >= DEPTH_MAX {
                break;
            }
            at = Some(parent);
        }
        depth
    };
    let mut items = Vec::new();
    let (mut done, mut total) = (0, 0);
    for todo in todos {
        let text = todo.get("content").and_then(Value::as_str).unwrap_or("").trim();
        if text.is_empty() {
            continue;
        }
        let status = match todo.get("status").and_then(Value::as_str).unwrap_or("") {
            "completed" | "done" => "done",
            "in_progress" => "in_progress",
            "cancelled" => "cancelled",
            _ => "pending",
        };
        if status != "cancelled" {
            total += 1;
        }
        if status == "done" {
            done += 1;
        }
        if items.len() < STEPS_MAX {
            let text = if text.chars().count() > TEXT_MAX {
                format!("{}…", text.chars().take(TEXT_MAX - 1).collect::<String>())
            } else {
                text.to_string()
            };
            let mut item = json!({"text": text, "status": status});
            let level = depth(todo.get("id").and_then(Value::as_str));
            if level > 0 {
                item["depth"] = json!(level);
            }
            items.push(item);
        }
    }
    json!({"items": items, "done": done, "total": total})
}

/// A Todo call's result as state.db keeps it (the whole list, `{"todos": […], "revision", "summary"}`) → the card's
/// list; None for a failed write (`{"error": …}`) or anything else.
pub fn of_result(result: &str) -> Option<Value> {
    let data: Value = serde_json::from_str(result.trim()).ok()?;
    if data.get("error").is_some() {
        return None;
    }
    Some(of_todos(data.get("todos")?.as_array()?))
}

/// Hermes' `todo.updated` event data (`{todos, revision}`) → the card's list.
pub fn of_event(data: &serde_json::Map<String, Value>) -> Option<Value> {
    Some(of_todos(data.get("todos")?.as_array()?))
}

/// The step being done (else the next one to do): what a task's card and a Live Activity show.
pub fn current(steps: &Value) -> Option<String> {
    let items = steps.get("items")?.as_array()?;
    let with = |status: &str| {
        items.iter().find(|i| i.get("status").and_then(Value::as_str) == Some(status))?.get("text")?.as_str()
    };
    with("in_progress").or_else(|| with("pending")).map(str::to_string)
}

/// (done, total) of a card's list.
pub fn counts(steps: &Value) -> (u64, u64) {
    let n = |k: &str| steps.get(k).and_then(Value::as_u64).unwrap_or(0);
    (n("done"), n("total"))
}

// -- automatic steps --------------------------------------------------------------------------------------------------
//
// Alex 6/11: a turn of several actions without a list of the agent's own still gets its card, made by the bridge from
// what the agent really did: one step per kind of action for the whole turn (its calls counted together), the call
// running the current step. Since 7/11 (Alex: « pas très utile ») only steps the person recognises count — never the
// agent's workings (calculations, files, its notes: `backstage` in tool_kinds.rs) — and only from the 3rd useful step
// (`AUTO_MIN_STEPS`: a quick answer or a short turn never gets one); at most `AUTO_VISIBLE`
// lines (older done steps folded into « +N »). Labels are the words of the tool's kind (scripts/tool-labels.py, in
// tool_kinds.rs: every tool an agent can call has one), a count, at most a site's domain or a connector's name: never
// an argument (a mail's subject, an address, a file's words) nor a tool's own name. The agent's own list wins.

/// Lines an automatic list shows at most (the oldest done steps folded into a first « +N » line).
pub const AUTO_VISIBLE: usize = 6;

/// One call the agent made, for the automatic list.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub tool: String,
    /// The site a browsing call is on (its domain only).
    pub site: Option<String>,
    /// The connector a connector's tool belongs to, by its name (« Météo du Grand-Est »: connectors::service_of).
    pub service: Option<String>,
    pub running: bool,
    pub call_id: Option<String>,
}

/// The step a tool's calls make (tool_kinds.rs, its kind's): None for a call that is no step of the work (the tool
/// search, memory, a question…), `unknown` for a connector's tool.
fn step_of(tool: &str) -> Option<&'static str> {
    let kind = crate::threads::layout::kind_of(tool);
    if kind == "unknown" {
        return Some("unknown");
    }
    crate::tool_kinds::KINDS.iter().find(|k| k.id == kind).and_then(|k| k.step)
}

/// The step that names its site (« Ouvrir le site kiabi.fr »): the browser's and a web page read.
const SITE_STEP: &str = "browser";

/// fr, en, de, es (others English).
fn lang_index(lang: &str) -> usize {
    match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    }
}

/// A step's words in the agent's language: its count, its site, its connector.
fn label(step: &str, n: usize, site: Option<&str>, service: Option<&str>, lang: &str) -> String {
    use crate::tool_kinds::{STEPS, STEP_SERVICE, STEP_SITE_MANY, STEP_SITE_ONE};
    let l = lang_index(lang);
    match (step, site, service) {
        (SITE_STEP, Some(s), _) => return (if n == 1 { STEP_SITE_ONE } else { STEP_SITE_MANY })[l].replace("{s}", s),
        ("unknown", _, Some(s)) => return STEP_SERVICE[l].replace("{s}", s),
        _ => {}
    }
    let words = STEPS.iter().find(|w| w.id == step).or_else(|| STEPS.iter().find(|w| w.id == "unknown"));
    let Some(words) = words else { return String::new() };
    if n > 1 {
        words.many[l].replace("{n}", &n.to_string())
    } else {
        words.one[l].to_string()
    }
}

/// The site of a browsing call: the domain of the first web address in its words (arguments or preview), nothing else.
pub fn site_of(tool: &str, words: &str) -> Option<String> {
    (step_of(tool) == Some(SITE_STEP)).then(|| crate::live_activities::site(words)).flatten()
}

/// A backstage step: the agent's own workings (calculations, files, its notes…: `backstage` in tool_kinds.rs), never
/// counted in an automatic list (Alex 7/11).
fn backstage(step: &str) -> bool {
    crate::tool_kinds::STEPS.iter().any(|s| s.id == step && s.backstage)
}

/// Useful steps an automatic list needs before it is shown (Alex 7/11: fewer is no plan worth a card).
pub const AUTO_MIN_STEPS: usize = 3;

/// The automatic list of these calls (None: fewer than `AUTO_MIN_STEPS` useful steps). One line per step for the
/// whole turn, in the order of its first call, its calls counted together (« Lire 3 mails » whatever came between);
/// a step with a call running is the current one (a stored turn that ended: every step done). Steps only grow within
/// a turn, so a live card, once shown, stays.
pub fn auto_steps(actions: &[Action], lang: &str) -> Option<Value> {
    struct Run<'a> {
        step: &'static str,
        n: usize,
        site: Option<String>,
        /// the same site throughout
        same: bool,
        service: Option<&'a str>,
        running: bool,
    }
    let mut steps: Vec<Run> = Vec::new();
    for action in actions {
        let Some(step) = step_of(&action.tool).filter(|s| !backstage(s)) else { continue };
        let service = action.service.as_deref();
        // a connector's calls: one step per connector
        match steps.iter_mut().find(|r| r.step == step && (step != "unknown" || r.service == service)) {
            Some(run) => {
                run.n += 1;
                if run.site.is_none() {
                    run.site = action.site.clone();
                } else if action.site.is_some() && action.site != run.site {
                    run.same = false;
                }
                run.running |= action.running;
            }
            None => {
                steps.push(Run { step, n: 1, site: action.site.clone(), same: true, service, running: action.running })
            }
        }
    }
    if steps.len() < AUTO_MIN_STEPS {
        return None;
    }
    let total = steps.len();
    let done = steps.iter().filter(|s| !s.running).count();
    let l = lang_index(lang);
    let mut items: Vec<Value> = steps
        .iter()
        .map(|run| {
            // several sites in one step: a count, no domain
            let site = run.site.as_deref().filter(|_| run.same);
            let n = if run.step == SITE_STEP && !run.same { run.n.max(2) } else { run.n };
            let text = if run.step == SITE_STEP && site.is_none() && n > 1 {
                crate::tool_kinds::STEP_SITES[l].replace("{n}", &n.to_string())
            } else {
                label(run.step, n, site, run.service, lang)
            };
            json!({"text": text, "status": if run.running { "in_progress" } else { "done" }})
        })
        .collect();
    if items.len() > AUTO_VISIBLE {
        // the oldest done steps folded into a first « +N » line
        let fold = items.len() - (AUTO_VISIBLE - 1);
        let folded = items[..fold].iter().all(|i| i["status"] == "done");
        if folded {
            let more = crate::tool_kinds::STEP_MORE[l].replace("{n}", &fold.to_string());
            items.splice(..fold, [json!({"text": more, "status": "done", "folded": fold})]);
        }
    }
    Some(json!({"items": items, "done": done, "total": total, "auto": true}))
}

/// A stored tool item as an action (a call through the tool search named by the call it wraps).
fn action_of(item: &Item) -> Option<Action> {
    let tool = item.body.get("tool")?;
    let mut name = tool.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    let args = tool.get("arguments").and_then(Value::as_str).unwrap_or("");
    if name == "tool_call" {
        let inner = serde_json::from_str::<Value>(args).ok().and_then(|a| {
            let call = a.get("calls").and_then(|c| c.get(0)).cloned().unwrap_or(a);
            call.get("name").and_then(Value::as_str).map(str::to_string)
        });
        name = inner.unwrap_or(name);
    }
    let site = site_of(&name, args);
    Some(Action {
        running: tool.get("status").and_then(Value::as_str) == Some("running"),
        tool: name,
        site,
        service: tool.get("service").and_then(Value::as_str).map(str::to_string),
        call_id: tool.get("call_id").and_then(Value::as_str).map(str::to_string),
    })
}

/// The automatic `steps` item of each turn without a list of the agent's own, after the turn's last call.
fn with_auto(items: &mut Vec<Item>, own: &std::collections::HashSet<String>, lang: &str) {
    let mut actions: HashMap<String, (Vec<Action>, usize)> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if item.kind() != "tool" || own.contains(&item.turn) {
            continue;
        }
        if let Some(action) = action_of(item) {
            let entry = actions.entry(item.turn.clone()).or_default();
            entry.0.push(action);
            entry.1 = i;
        }
    }
    let mut after: Vec<(usize, Item)> = actions
        .into_iter()
        .filter_map(|(turn, (list, last))| {
            let steps = auto_steps(&list, lang)?;
            let at = &items[last];
            let mut body = Map::new();
            body.insert("kind".into(), json!("steps"));
            body.insert("steps".into(), steps);
            Some((last, Item {
                id: format!("{turn}.steps"),
                pos: (at.pos.0, at.pos.1, 99),
                at: at.at,
                turn,
                segment: at.segment.clone(),
                row: at.row,
                body,
            }))
        })
        .collect();
    after.sort_by_key(|a| std::cmp::Reverse(a.0));
    for (last, item) in after {
        items.insert(last + 1, item);
    }
}

/// One `steps` item per turn: the turn's last list (an emptied list: none), id `<turn>.steps`, where its call is; a
/// turn of several actions without a list of the agent's own gets the automatic one, after its last call.
pub fn fold(items: &mut Vec<Item>, lang: &str) {
    let own: std::collections::HashSet<String> =
        items.iter().filter(|i| i.kind() == "steps").map(|i| i.turn.clone()).collect();
    fold_own(items);
    with_auto(items, &own, lang);
}

fn fold_own(items: &mut Vec<Item>) {
    let mut last: HashMap<String, usize> = HashMap::new();
    for (i, item) in items.iter().enumerate() {
        if item.kind() == "steps" {
            last.insert(item.turn.clone(), i);
        }
    }
    if last.is_empty() {
        return;
    }
    let mut index = 0;
    items.retain_mut(|item| {
        let i = index;
        index += 1;
        if item.kind() != "steps" {
            return true;
        }
        let empty =
            item.body.get("steps").and_then(|s| s.get("items")).and_then(Value::as_array).is_none_or(Vec::is_empty);
        if last.get(&item.turn) != Some(&i) || empty {
            return false;
        }
        item.id = format!("{}.steps", item.turn);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todos(list: Value) -> Vec<Value> {
        list.as_array().unwrap().clone()
    }

    #[test]
    fn hermes_list_becomes_the_cards() {
        let steps = of_todos(&todos(json!([
            {"id": "1", "content": "Lire les e-mails", "status": "completed"},
            {"id": "2", "content": "Trier par urgence", "status": "in_progress"},
            {"id": "3", "content": "Pièces jointes", "status": "pending", "parent": "2"},
            {"id": "4", "content": "Appeler la banque", "status": "cancelled"},
            {"id": "5", "content": "   ", "status": "pending"},
            {"id": "6", "content": "Résumer", "status": "weird"}
        ])));
        assert_eq!(
            steps,
            json!({"items": [
                {"text": "Lire les e-mails", "status": "done"},
                {"text": "Trier par urgence", "status": "in_progress"},
                {"text": "Pièces jointes", "status": "pending", "depth": 1},
                {"text": "Appeler la banque", "status": "cancelled"},
                {"text": "Résumer", "status": "pending"}
            ], "done": 1, "total": 4})
        );
        assert_eq!(current(&steps).as_deref(), Some("Trier par urgence"));
        assert_eq!(counts(&steps), (1, 4));
    }

    #[test]
    fn long_lists_and_words_are_cut_and_parent_cycles_end() {
        let many: Vec<Value> =
            (0..40).map(|i| json!({"id": format!("{i}"), "content": "x".repeat(500), "status": "pending"})).collect();
        let steps = of_todos(&many);
        assert_eq!(steps["items"].as_array().unwrap().len(), STEPS_MAX);
        assert_eq!(steps["total"], 40, "the count is the whole list's");
        assert_eq!(steps["items"][0]["text"].as_str().unwrap().chars().count(), TEXT_MAX);
        let cycle = of_todos(&todos(json!([
            {"id": "a", "content": "A", "status": "pending", "parent": "b"},
            {"id": "b", "content": "B", "status": "pending", "parent": "a"}
        ])));
        assert!(cycle["items"][0]["depth"].as_u64().unwrap() <= DEPTH_MAX);
    }

    #[test]
    fn calls_of_the_todo_tool_are_known_directly_or_through_the_search() {
        assert!(is_todo_call("todo_list", "{}"));
        assert!(is_todo_call("todo", "{}"));
        assert!(is_todo_call("tool_call", r#"{"calls": [{"name": "todo_list", "arguments": {"todos": []}}]}"#));
        assert!(is_todo_call("tool_call", r#"{"calls": "[{\"name\": \"todo_list\"}]"}"#));
        assert!(is_todo_call("tool_call", r#"{"name": "todo_list", "arguments": {}}"#));
        assert!(!is_todo_call("tool_call", r#"{"calls": [{"name": "todo_list"}, {"name": "web_search"}]}"#));
        assert!(!is_todo_call("tool_call", r#"{"calls": [{"name": "web_search", "arguments": {"q": "todo"}}]}"#));
        assert!(!is_todo_call("web_search", r#"{"q": "todo"}"#));
    }

    #[test]
    fn a_result_gives_the_list_a_failed_write_nothing() {
        let ok = r#"{"todos": [{"id": "1", "content": "A", "status": "completed"}], "revision": 2, "summary": {}}"#;
        assert_eq!(of_result(ok).unwrap()["done"], 1);
        assert!(of_result(r#"{"error": "Todo item '1' has empty or missing content."}"#).is_none());
        assert!(of_result("not json").is_none());
        let mut data = serde_json::Map::new();
        data.insert("todos".into(), json!([{"id": "1", "content": "A", "status": "in_progress"}]));
        data.insert("revision".into(), json!(1));
        assert_eq!(current(&of_event(&data).unwrap()).as_deref(), Some("A"));
    }

    fn act(tool: &str, site: Option<&str>, running: bool) -> Action {
        Action { tool: tool.into(), site: site.map(str::to_string), service: None, running, call_id: None }
    }

    fn texts(steps: &Value) -> Vec<(String, String)> {
        steps["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["text"].as_str().unwrap().to_string(), i["status"].as_str().unwrap().to_string()))
            .collect()
    }

    fn pair(text: &str, status: &str) -> (String, String) {
        (text.to_string(), status.to_string())
    }

    #[test]
    fn the_agents_calls_make_steps_by_kind_with_a_count() {
        let calls = [
            act("mcp__bipagents__mail_search", None, false),
            act("mcp__bipagents__mail_read", None, false),
            act("mcp__bipagents__mail_read", None, false),
            act("tool_search", None, false),
            act("mcp__bipagents__mail_read", None, false),
            act("mcp__stealth__navigate", Some("sncf-connect.com"), false),
            act("mcp__stealth__click_element", None, true),
        ];
        let steps = auto_steps(&calls, "fr").unwrap();
        assert_eq!(
            texts(&steps),
            [
                pair("Chercher dans tes mails", "done"),
                pair("Lire 3 mails", "done"),
                pair("Naviguer sur sncf-connect.com", "in_progress")
            ]
        );
        assert_eq!((steps["done"].as_u64(), steps["total"].as_u64()), (Some(2), Some(3)));
        assert_eq!(steps["auto"], true);
        assert_eq!(current(&steps).as_deref(), Some("Naviguer sur sncf-connect.com"));
        assert_eq!(auto_steps(&calls, "en").unwrap()["items"][1]["text"], "Read 3 emails");
    }

    #[test]
    fn a_quick_answer_gets_no_card() {
        assert!(auto_steps(&[act("weather", None, false)], "fr").is_none());
        assert!(auto_steps(&[act("web_search", None, false), act("web_search", None, false)], "fr").is_none());
        let meta = [act("tool_search", None, false), act("memory", None, false), act("weather", None, false)];
        assert!(auto_steps(&meta, "fr").is_none());
    }

    #[test]
    fn a_long_list_folds_its_oldest_done_steps() {
        let kinds = [
            "web_search",
            "create_page",
            "weather",
            "map_image",
            "calendar_list",
            "reminders_add",
            "mail_read",
            "give_file",
        ];
        let calls: Vec<Action> = kinds.iter().enumerate().map(|(i, k)| act(k, None, i == 7)).collect();
        let steps = auto_steps(&calls, "fr").unwrap();
        let shown = texts(&steps);
        assert_eq!(shown.len(), AUTO_VISIBLE);
        assert_eq!(shown[0], pair("+3 étapes", "done"));
        assert_eq!(shown[5].1, "in_progress");
        assert_eq!((steps["done"].as_u64(), steps["total"].as_u64()), (Some(7), Some(8)));
    }

    #[test]
    fn labels_never_carry_what_the_calls_were_about() {
        // only a browsing call's domain, from a web address; never a mail's subject or a search's words
        assert_eq!(site_of("mcp__stealth__navigate", "https://www.kiabi.fr/soldes?x=1").as_deref(), Some("kiabi.fr"));
        assert_eq!(site_of("mcp__bipagents__mail_read", "https://bank.example/confidential"), None);
        assert_eq!(site_of("web_search", r#"{"q": "avocat divorce https://x.fr"}"#), None);
        let calls = [
            act("mcp__bipagents__mail_search", None, false),
            act("web_search", None, false),
            act("mcp__stealth__navigate", Some("a.fr"), false),
            act("mcp__stealth__navigate", Some("b.fr"), false),
        ];
        assert_eq!(texts(&auto_steps(&calls, "fr").unwrap())[2].0, "Ouvrir 2 sites");
    }

    /// Alex 7/11: « Utiliser un outil » for a tool the bridge didn't know. Every tool's step is in its words (the
    /// table, scripts/tool-labels.py); a connector's by its connector's name, else a neutral phrase: never a tool's
    /// own name.
    #[test]
    fn every_step_has_its_words_never_a_tools_name() {
        let with = |tool: &str, service: Option<&str>| Action {
            tool: tool.into(),
            site: None,
            service: service.map(str::to_string),
            running: false,
            call_id: None,
        };
        let calls = [
            with("mcp__bipagents__routine_schedule", None),
            with("mcp__bipagents__give_file", None),
            with("mcp__bipagents__give_file", None),
            with("mcp__bipconn_meteo_du_grand_est__forecast", Some("Météo du Grand-Est")),
            with("mcp__bipconn_meteo_du_grand_est__alerts", Some("Météo du Grand-Est")),
            with("mcp__acme__fetch_invoices", None),
            with("upcoming_dates", None),
        ];
        assert_eq!(
            texts(&auto_steps(&calls, "fr").unwrap()),
            [
                pair("Mettre à jour une routine", "done"),
                pair("Donner 2 fichiers", "done"),
                pair("Utiliser Météo du Grand-Est", "done"),
                pair("Utiliser un service connecté", "done"),
                pair("Regarder les dates à venir", "done"),
            ]
        );
        let english: Vec<String> = texts(&auto_steps(&calls, "en").unwrap()).into_iter().map(|t| t.0).collect();
        assert_eq!(english[..2], ["Update a routine".to_string(), "Hand over 2 files".to_string()]);
        for lang in ["fr", "en", "de", "es"] {
            for kind in crate::tool_kinds::KINDS {
                let Some(step) = kind.step else { continue };
                for n in [1, 3] {
                    let text = label(step, n, None, None, lang);
                    assert!(!text.is_empty() && !text.contains('_') && !text.contains('{'), "{lang} {step}: {text}");
                }
            }
        }
    }

    /// Alex 7/11 on Wellness: « Faire un calcul / Utiliser un outil / Faire un calcul / Noter dans ton Carnet » —
    /// « ce genre de todo, pas très utile ». The workings don't count: one useful step left, no card.
    #[test]
    fn backstage_steps_never_make_a_card() {
        let alex = [
            act("execute_code", None, false),
            act("read_file", None, false),
            act("execute_code", None, false),
            act("mcp__bipagents__notebook_write", None, false),
        ];
        assert!(auto_steps(&alex, "fr").is_none());
        // three useful steps among calculations and files: a card of those three
        let useful = [
            act("mcp__bipagents__mail_search", None, false),
            act("execute_code", None, false),
            act("web_search", None, false),
            act("write_file", None, false),
            act("terminal", None, false),
            act("mcp__bipagents__create_page", None, true),
        ];
        assert_eq!(
            texts(&auto_steps(&useful, "fr").unwrap()),
            [
                pair("Chercher dans tes mails", "done"),
                pair("Chercher sur le web", "done"),
                pair("Préparer une page", "in_progress")
            ]
        );
        // every backstage step is one of the table's, and the web, a connector, a page made never are
        for id in ["compute", "readFile", "prepareFile", "knowledge", "spaceEstimate", "sitesList", "voices", "notebookSearch",
            "notebookNote", "notebookTask", "notebookRecord"]
        {
            assert!(backstage(id), "{id}");
        }
        for id in ["webSearch", "browser", "unknown", "createPage", "mailRead", "giveFile", "payment"] {
            assert!(!backstage(id), "{id}");
        }
    }

    #[test]
    fn a_step_is_one_line_for_the_whole_turn() {
        let calls = [
            act("mcp__bipagents__mail_read", None, false),
            act("web_search", None, false),
            act("mcp__bipagents__mail_read", None, false),
            act("mcp__bipagents__calendar_list", None, false),
            act("mcp__bipagents__mail_read", None, true),
            act("web_search", None, false),
        ];
        let steps = auto_steps(&calls, "fr").unwrap();
        assert_eq!(
            texts(&steps),
            [
                pair("Lire 3 mails", "in_progress"),
                pair("Faire 2 recherches sur le web", "done"),
                pair("Regarder ton agenda", "done")
            ]
        );
        assert_eq!((steps["done"].as_u64(), steps["total"].as_u64()), (Some(2), Some(3)));
    }

    /// Live: no card until the 3rd useful step starts; from then on it stays (steps only grow in a turn).
    #[test]
    fn the_live_card_appears_at_the_third_useful_step_and_stays() {
        let turn = [
            ("mcp__bipagents__mail_search", true),
            ("mcp__bipagents__mail_search", false),
            ("execute_code", true),
            ("execute_code", false),
            ("mcp__bipagents__mail_read", true),
            ("mcp__bipagents__mail_read", false),
            ("mcp__bipagents__mail_search", true),
            ("mcp__bipagents__mail_search", false),
            ("mcp__bipagents__reminders_add", true),
            ("mcp__bipagents__reminders_add", false),
            ("mcp__bipagents__mail_read", true),
        ];
        let mut actions: Vec<Action> = Vec::new();
        let mut shown = Vec::new();
        for (tool, started) in turn {
            if started {
                actions.push(act(tool, None, true));
            } else if let Some(open) = actions.iter_mut().find(|a| a.running && a.tool == tool) {
                open.running = false;
            }
            shown.push(auto_steps(&actions, "fr").map(|s| s["items"].as_array().unwrap().len()));
        }
        assert_eq!(shown, [None, None, None, None, None, None, None, None, Some(3), Some(3), Some(3)]);
        let last = auto_steps(&actions, "fr").unwrap();
        assert_eq!(last["items"][1], json!({"text": "Lire 2 mails", "status": "in_progress"}));
    }

    fn item_of(id: &str, turn: &str, pos: (u32, i64, u32), body: Value) -> Item {
        Item {
            id: id.into(),
            pos,
            at: pos.1 as f64,
            turn: turn.into(),
            segment: "s".into(),
            row: pos.1,
            body: body.as_object().unwrap().clone(),
        }
    }

    fn tool(id: &str, turn: &str, order: i64, name: &str, args: &str) -> Item {
        let body = json!({"kind": "tool", "tool": {"name": name, "arguments": args, "status": "done", "call_id": id}});
        item_of(id, turn, (0, order, 1), body)
    }

    #[test]
    fn a_turn_without_its_own_list_gets_the_automatic_one_and_its_own_list_wins() {
        let msg = |id: &str, turn: &str, order: i64| item_of(id, turn, (0, order, 0), json!({"kind": "message"}));
        let wrapped = r#"{"calls": [{"name": "mcp__bipagents__mail_read", "arguments": {"id": "42"}}]}"#;
        let own = of_todos(&[json!({"id": "1", "content": "Mon plan", "status": "completed"})]);
        let mut items = vec![
            msg("m1", "m1", 1),
            tool("m2.t0", "m1", 2, "mcp__bipagents__mail_search", "{}"),
            tool("m3.t0", "m1", 3, "tool_call", wrapped),
            tool("m4.t0", "m1", 4, "mcp__bipagents__create_page", "{}"),
            msg("m5", "m1", 5),
            // a turn with a list of its own: no automatic one
            msg("m6", "m6", 6),
            tool("m7.t0", "m6", 7, "web_search", "{}"),
            tool("m8.t0", "m6", 8, "create_page", "{}"),
            item_of("m9.t0", "m6", (0, 9, 1), json!({"kind": "steps", "steps": own})),
        ];
        fold(&mut items, "fr");
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m2.t0", "m3.t0", "m4.t0", "m1.steps", "m5", "m6", "m7.t0", "m8.t0", "m6.steps"]);
        assert_eq!(
            texts(&items[4].body["steps"]),
            [pair("Chercher dans tes mails", "done"), pair("Lire un mail", "done"), pair("Préparer une page", "done")]
        );
        assert_eq!(items[4].pos, (0, 4, 99), "after the turn's last call, before the reply");
        assert_eq!(items[9].body["steps"]["items"][0]["text"], "Mon plan");
    }
}
