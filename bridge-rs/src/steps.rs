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
// what the agent really did: one step per kind of action (consecutive calls of a kind are one step, with a count),
// the call running the current step. Only from the 2nd step (a quick answer never gets one), at most `AUTO_VISIBLE`
// lines (older done steps folded into « +N »). Labels are the bridge's words for the tool's kind, a count and at most a
// site's domain: never an argument (a mail's subject, an address, a file's words). The agent's own list wins.

/// Lines an automatic list shows at most (the oldest done steps folded into a first « +N » line).
pub const AUTO_VISIBLE: usize = 6;

/// One call the agent made, for the automatic list.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub tool: String,
    /// The site a browsing call is on (its domain only).
    pub site: Option<String>,
    pub running: bool,
    pub call_id: Option<String>,
}

/// The kind of step a tool makes (None: a call that is no step of the work: the tool search, memory, a question…).
fn kind_of(tool: &str) -> Option<&'static str> {
    let bare = crate::loop_brake::bare(tool);
    let bare = bare.strip_prefix("mcp__stealth__").or_else(|| bare.strip_prefix("mcp_stealth_")).map_or(bare, |_| "site");
    Some(match bare {
        "site" | "browser_navigate" | "browser_click" | "browser_type" | "browser_snapshot" | "browser_scroll"
        | "browser_exec" | "web_extract" => "site",
        "mail_search" | "mail_overview" | "mail_list" => "mail_search",
        "mail_read" => "mail_read",
        "mail_draft" | "mail_send" | "mail_reply" => "mail_write",
        "mail_bulk" | "mail_tidy" | "mail_rules" | "mail_move" => "mail_tidy",
        "web_search" => "web_search",
        "notebook_search" | "notebook_read" | "notebook_list" => "notebook_read",
        "notebook_note" | "notebook_fill" | "notebook_write" => "notebook_write",
        "calendar_list" => "calendar_read",
        "calendar_add" | "calendar_update" => "calendar_write",
        "reminders_list" => "reminders_read",
        "reminders_add" => "reminders_write",
        "contacts_search" => "contacts",
        "create_page" => "page",
        "map_image" => "map",
        "weather" => "weather",
        "read_file" | "search_files" | "list_files" => "files_read",
        "write_file" | "patch" | "give_file" => "files_write",
        "terminal" | "execute_code" => "compute",
        "vision_analyze" => "image",
        "start_task" | "propose_task" => "task",
        "ask_agent" | "message_agent" => "agent",
        "create_audio" | "create_podcast" => "audio",
        "tool_search" | "tool_describe" | "tool_call" | "skill_view" | "skills_list" | "memory" | "clarify"
        | "session_search" | "todo_list" | "todo" | "after_call" => return None,
        _ => "other",
    })
}

/// A step's words in the agent's language (fr, en, de, es; others English): its kind, its count, its site.
fn label(kind: &str, n: usize, site: Option<&str>, lang: &str) -> String {
    let l = match lang {
        "fr" => 0,
        "de" => 2,
        "es" => 3,
        _ => 1,
    };
    let pick = |one: [&str; 4], many: [&str; 4]| {
        if n > 1 {
            many[l].replace("{n}", &n.to_string())
        } else {
            one[l].to_string()
        }
    };
    match kind {
        "site" => match (site, n) {
            (Some(s), 1) => ["Ouvrir le site {s}", "Open {s}", "{s} öffnen", "Abrir {s}"][l].replace("{s}", s),
            (Some(s), _) => ["Naviguer sur {s}", "Browse {s}", "Auf {s} navigieren", "Navegar por {s}"][l].replace("{s}", s),
            (None, _) => pick(
                ["Ouvrir un site", "Open a site", "Eine Website öffnen", "Abrir un sitio"],
                ["Naviguer sur le web", "Browse the web", "Im Web navigieren", "Navegar por la web"],
            ),
        },
        "mail_search" => pick(
            ["Chercher dans tes mails", "Search your mail", "In deinen Mails suchen", "Buscar en tu correo"],
            ["Chercher dans tes mails", "Search your mail", "In deinen Mails suchen", "Buscar en tu correo"],
        ),
        "mail_read" => pick(
            ["Lire un mail", "Read an email", "Eine Mail lesen", "Leer un correo"],
            ["Lire {n} mails", "Read {n} emails", "{n} Mails lesen", "Leer {n} correos"],
        ),
        "mail_write" => pick(
            ["Écrire un mail", "Write an email", "Eine Mail schreiben", "Escribir un correo"],
            ["Écrire {n} mails", "Write {n} emails", "{n} Mails schreiben", "Escribir {n} correos"],
        ),
        "mail_tidy" => pick(
            ["Ranger tes mails", "Sort your mail", "Deine Mails ordnen", "Ordenar tu correo"],
            ["Ranger tes mails", "Sort your mail", "Deine Mails ordnen", "Ordenar tu correo"],
        ),
        "web_search" => pick(
            ["Chercher sur le web", "Search the web", "Im Web suchen", "Buscar en la web"],
            ["Faire {n} recherches sur le web", "Run {n} web searches", "{n} Websuchen", "Hacer {n} búsquedas en la web"],
        ),
        "notebook_read" => pick(
            ["Chercher dans ton Carnet", "Look in your Notebook", "In deinem Notizbuch suchen", "Buscar en tu Cuaderno"],
            ["Chercher dans ton Carnet", "Look in your Notebook", "In deinem Notizbuch suchen", "Buscar en tu Cuaderno"],
        ),
        "notebook_write" => pick(
            ["Noter dans ton Carnet", "Note it in your Notebook", "Im Notizbuch notieren", "Anotar en tu Cuaderno"],
            ["Noter dans ton Carnet", "Note it in your Notebook", "Im Notizbuch notieren", "Anotar en tu Cuaderno"],
        ),
        "calendar_read" => pick(
            ["Regarder ton agenda", "Check your calendar", "Deinen Kalender ansehen", "Mirar tu agenda"],
            ["Regarder ton agenda", "Check your calendar", "Deinen Kalender ansehen", "Mirar tu agenda"],
        ),
        "calendar_write" => pick(
            ["Ajouter à ton agenda", "Add to your calendar", "In den Kalender eintragen", "Añadir a tu agenda"],
            ["Ajouter {n} événements", "Add {n} events", "{n} Termine eintragen", "Añadir {n} eventos"],
        ),
        "reminders_read" => pick(
            ["Regarder tes rappels", "Check your reminders", "Deine Erinnerungen ansehen", "Mirar tus recordatorios"],
            ["Regarder tes rappels", "Check your reminders", "Deine Erinnerungen ansehen", "Mirar tus recordatorios"],
        ),
        "reminders_write" => pick(
            ["Créer un rappel", "Set a reminder", "Eine Erinnerung anlegen", "Crear un recordatorio"],
            ["Créer {n} rappels", "Set {n} reminders", "{n} Erinnerungen anlegen", "Crear {n} recordatorios"],
        ),
        "contacts" => pick(
            ["Chercher dans tes contacts", "Look in your contacts", "In deinen Kontakten suchen", "Buscar en tus contactos"],
            ["Chercher dans tes contacts", "Look in your contacts", "In deinen Kontakten suchen", "Buscar en tus contactos"],
        ),
        "page" => pick(
            ["Préparer une page", "Make a page", "Eine Seite erstellen", "Preparar una página"],
            ["Préparer {n} pages", "Make {n} pages", "{n} Seiten erstellen", "Preparar {n} páginas"],
        ),
        "map" => pick(
            ["Faire une carte", "Make a map", "Eine Karte erstellen", "Hacer un mapa"],
            ["Faire {n} cartes", "Make {n} maps", "{n} Karten erstellen", "Hacer {n} mapas"],
        ),
        "weather" => pick(
            ["Regarder la météo", "Check the weather", "Das Wetter ansehen", "Mirar el tiempo"],
            ["Regarder la météo", "Check the weather", "Das Wetter ansehen", "Mirar el tiempo"],
        ),
        "files_read" => pick(
            ["Lire un fichier", "Read a file", "Eine Datei lesen", "Leer un archivo"],
            ["Lire {n} fichiers", "Read {n} files", "{n} Dateien lesen", "Leer {n} archivos"],
        ),
        "files_write" => pick(
            ["Préparer un fichier", "Make a file", "Eine Datei erstellen", "Preparar un archivo"],
            ["Préparer {n} fichiers", "Make {n} files", "{n} Dateien erstellen", "Preparar {n} archivos"],
        ),
        "compute" => pick(
            ["Faire un calcul", "Run a calculation", "Etwas berechnen", "Hacer un cálculo"],
            ["Faire des calculs", "Run calculations", "Berechnungen durchführen", "Hacer cálculos"],
        ),
        "image" => pick(
            ["Regarder une image", "Look at an image", "Ein Bild ansehen", "Mirar una imagen"],
            ["Regarder {n} images", "Look at {n} images", "{n} Bilder ansehen", "Mirar {n} imágenes"],
        ),
        "task" => pick(
            ["Lancer une tâche de fond", "Start a background task", "Eine Hintergrundaufgabe starten", "Iniciar una tarea de fondo"],
            ["Lancer une tâche de fond", "Start a background task", "Eine Hintergrundaufgabe starten", "Iniciar una tarea de fondo"],
        ),
        "agent" => pick(
            ["Demander à un autre agent", "Ask another agent", "Einen anderen Agenten fragen", "Preguntar a otro agente"],
            ["Demander à d'autres agents", "Ask other agents", "Andere Agenten fragen", "Preguntar a otros agentes"],
        ),
        "audio" => pick(
            ["Préparer l'audio", "Make the audio", "Das Audio erstellen", "Preparar el audio"],
            ["Préparer l'audio", "Make the audio", "Das Audio erstellen", "Preparar el audio"],
        ),
        _ => pick(
            ["Utiliser un outil", "Use a tool", "Ein Werkzeug benutzen", "Usar una herramienta"],
            ["Utiliser {n} outils", "Use {n} tools", "{n} Werkzeuge benutzen", "Usar {n} herramientas"],
        ),
    }
}

/// The site of a browsing call: the domain of the first web address in its words (arguments or preview), nothing else.
pub fn site_of(tool: &str, words: &str) -> Option<String> {
    (kind_of(tool) == Some("site")).then(|| crate::live_activities::site(words)).flatten()
}

/// The automatic list of these calls (None: fewer than 2 steps). `running`: whether a running call is the current
/// step (a stored turn that ended: every step done).
pub fn auto_steps(actions: &[Action], lang: &str) -> Option<Value> {
    // (kind, count, site, same site throughout, running)
    let mut steps: Vec<(&'static str, usize, Option<String>, bool, bool)> = Vec::new();
    for action in actions {
        let Some(kind) = kind_of(&action.tool) else { continue };
        match steps.last_mut() {
            Some(last) if last.0 == kind => {
                last.1 += 1;
                if last.2.is_none() {
                    last.2 = action.site.clone();
                } else if action.site.is_some() && action.site != last.2 {
                    last.3 = false;
                }
                last.4 |= action.running;
            }
            _ => steps.push((kind, 1, action.site.clone(), true, action.running)),
        }
    }
    if steps.len() < 2 {
        return None;
    }
    let total = steps.len();
    let done = steps.iter().filter(|s| !s.4).count();
    let mut items: Vec<Value> = steps
        .iter()
        .map(|(kind, n, site, same, running)| {
            // several sites in one step: a count, no domain
            let site = site.as_deref().filter(|_| *same);
            let n = if *kind == "site" && !*same { (*n).max(2) } else { *n };
            let text = if *kind == "site" && site.is_none() && n > 1 {
                ["Ouvrir {n} sites", "Open {n} sites", "{n} Websites öffnen", "Abrir {n} sitios"]
                    [match lang { "fr" => 0, "de" => 2, "es" => 3, _ => 1 }]
                    .replace("{n}", &n.to_string())
            } else {
                label(kind, n, site, lang)
            };
            json!({"text": text, "status": if *running { "in_progress" } else { "done" }})
        })
        .collect();
    if items.len() > AUTO_VISIBLE {
        // the oldest done steps folded into a first « +N » line
        let fold = items.len() - (AUTO_VISIBLE - 1);
        let folded = items[..fold].iter().all(|i| i["status"] == "done");
        if folded {
            let more = ["+{n} étapes", "+{n} steps", "+{n} Schritte", "+{n} pasos"]
                [match lang { "fr" => 0, "de" => 2, "es" => 3, _ => 1 }]
                .replace("{n}", &fold.to_string());
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
    after.sort_by(|a, b| b.0.cmp(&a.0));
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
        Action { tool: tool.into(), site: site.map(str::to_string), running, call_id: None }
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
            "notebook_note",
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
