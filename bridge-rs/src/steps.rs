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

use serde_json::{json, Value};

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

/// One `steps` item per turn: the turn's last list (an emptied list: none), id `<turn>.steps`, where its call is.
pub fn fold(items: &mut Vec<Item>) {
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
}
