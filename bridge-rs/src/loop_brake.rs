//! The brake on a turn going round in circles (2026-10-27). In the token lab, GLM-5.3 Flash answered « open this
//! Wikipedia page in your browser » with 255 `calendar_add` and 7 `calendar_list` in one turn, for 15 minutes, until
//! the bench's timeout: 267 model calls, 10.5 M tokens. Its words kept saying it would stop. Nothing stopped it: Hermes
//! has no iteration cap by default (`agent.max_turns` unset is unlimited), and the long-work steer (runs.rs `handoff`)
//! is only advice.
//!
//! Two parts, independent of the model:
//!
//! - the run hub (runs.rs) counts the tool calls of every run it follows (`tool.started`). A turn that calls one tool
//!   more than `same_tool` times, makes more than `tool_calls` calls in all, or makes the same call (tool and
//!   arguments) `same_call` times is stopped (Hermes' `POST /v1/runs/{id}/stop`), and the conversation gets a short
//!   plain line (`bridge.notice`, code `loop_stopped`, in the agent's language): « Je me suis arrêté : je tournais en
//!   rond sur l'agenda. Dis-moi si tu veux que je réessaie. » The work tools a turn legitimately repeats (the terminal,
//!   code, files, web search, the visible browser) are not held to `same_tool`, only to the total, and a run that
//!   browses gets the background limits for its total (the person watches it live). Calls a sub-agent makes while it
//!   works don't count (as for the steer);
//! - the bridge's own write tools (calendar, reminders, contacts, Carnet, mail drafts, pages, audio, family news) are
//!   refused past `writes` writes in a turn, and the very same write (same tool, same arguments) twice in a turn is
//!   refused at once, with a plain answer the model can act on (« already done »). A « turn » here: the agent's runs
//!   the hub follows at that moment; with none (a routine, a run nobody follows), the agent's calls until it has been
//!   quiet `idle_seconds`, and then only the cap applies (the person may have asked for the same thing again).
//!
//! Background tasks (jobs.rs, « Tâches de fond »: work with a budget, many calls by design) have their own, higher
//! limits (`task_*`); they are stopped the same way, the task then ends as stopped. The mail scan (mail_scan.rs) makes
//! no tool calls at all (the agent's model reads mails with no tools), so nothing here concerns it.
//!
//! `[brake]` in bridge.toml (every value optional):
//!
//!     enabled = true
//!     same_tool = 10          tool_calls = 60        same_call = 3        writes = 10
//!     task_same_tool = 40     task_tool_calls = 150  task_same_call = 5   task_writes = 40
//!     idle_seconds = 600
//!
//! No Python counterpart (owner group: push-runs).

use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use serde_json::{json, Value};

use crate::tools_knowledge::tool_text;
use crate::Dict;

/// One kind of turn's limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// One tool called more than this many times stops the turn.
    pub same_tool: u32,
    /// More calls than this in all stops it.
    pub tool_calls: u32,
    /// The same call (tool and arguments) this many times stops it.
    pub same_call: u32,
    /// The bridge's write tools refuse past this many writes.
    pub writes: u32,
}

/// `[brake]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub conversation: Limits,
    pub task: Limits,
    /// With no run followed: the agent's calls form one turn until it has been quiet this long.
    pub idle_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            conversation: Limits { same_tool: 10, tool_calls: 60, same_call: 3, writes: 10 },
            task: Limits { same_tool: 40, tool_calls: 150, same_call: 5, writes: 40 },
            idle_seconds: 600.0,
        }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let d = Settings::default();
        let n = |key: &str, default: u32, min: u32| -> u32 {
            match table.get(key).and_then(Value::as_i64) {
                Some(v) => v.clamp(min as i64, 100_000) as u32,
                None => default,
            }
        };
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(d.enabled),
            conversation: Limits {
                same_tool: n("same_tool", d.conversation.same_tool, 2),
                tool_calls: n("tool_calls", d.conversation.tool_calls, 5),
                same_call: n("same_call", d.conversation.same_call, 2),
                writes: n("writes", d.conversation.writes, 1),
            },
            task: Limits {
                same_tool: n("task_same_tool", d.task.same_tool, 2),
                tool_calls: n("task_tool_calls", d.task.tool_calls, 5),
                same_call: n("task_same_call", d.task.same_call, 2),
                writes: n("task_writes", d.task.writes, 1),
            },
            idle_seconds: table.get("idle_seconds").and_then(Value::as_f64).unwrap_or(d.idle_seconds).clamp(10.0, 86_400.0),
        }
    }

    pub fn limits(&self, background: bool) -> Limits {
        if background {
            self.task
        } else {
            self.conversation
        }
    }
}

/// The bridge's tools as Hermes names them (`mcp__bipagents__calendar_add`, `mcp_bipagents_calendar_add`): the bare
/// name.
pub fn bare(tool: &str) -> &str {
    for prefix in ["mcp__bipagents__", "mcp_bipagents_"] {
        if let Some(rest) = tool.strip_prefix(prefix) {
            return rest;
        }
    }
    tool
}

/// Tools a turn legitimately calls many times: held to the total only, never to `same_tool` (nor to `same_call`
/// for the first ones, whose short preview repeats while the work differs: a test run again after a fix).
const WORK_TOOLS: [&str; 10] = [
    "terminal",
    "execute_code",
    "process",
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "web_search",
    "web_extract",
    "delegate_task",
];
const PREVIEW_ONLY_TOOLS: [&str; 3] = ["terminal", "execute_code", "process"];

/// The bridge's tools that change something for the person.
pub const WRITE_TOOLS: [&str; 18] = [
    "calendar_add",
    "calendar_update",
    "calendar_delete",
    "reminders_add",
    "reminders_complete",
    "contacts_add",
    "contacts_update",
    "notebook_note",
    "notebook_edit",
    "notebook_task",
    "notebook_record",
    "mail_draft",
    "create_page",
    "delete_page",
    "create_audio",
    "create_podcast",
    "tell_family",
    "send_files_to_agent",
];

pub fn write_tool(tool: &str) -> bool {
    WRITE_TOOLS.contains(&bare(tool))
}

/// Why a turn was stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Over {
    /// One tool, too many times (its bare name).
    SameTool(String),
    /// Too many calls in all.
    TooMany,
    /// The same call again and again (its bare tool name).
    SameCall(String),
}

/// A turn's tool calls so far.
#[derive(Debug, Default, Clone)]
pub struct TurnCount {
    pub total: u32,
    per_tool: HashMap<String, u32>,
    per_call: HashMap<String, u32>,
}

/// A call's arguments in a stable form (keys sorted), for telling the same call.
pub fn canonical(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for k in keys {
                    out.insert(k.clone(), sorted(&map[k]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string(&sorted(value)).unwrap_or_default()
}

impl TurnCount {
    /// One more call of `tool` (Hermes' name), with its arguments as the run's event tells them (`signature`: the
    /// arguments, else the event's short preview; None: nothing to tell calls apart). `browser`: one of the visible
    /// browser's tools. The first limit passed, if any.
    pub fn note(&mut self, tool: &str, signature: Option<&str>, browser: bool, limits: &Limits) -> Option<Over> {
        let name = bare(tool).to_string();
        self.total += 1;
        let work = browser || WORK_TOOLS.contains(&name.as_str());
        let same = self.per_tool.entry(name.clone()).or_insert(0);
        *same += 1;
        if !work && *same > limits.same_tool {
            return Some(Over::SameTool(name));
        }
        if let Some(signature) = signature.filter(|s| !s.trim().is_empty()) {
            if !browser && !PREVIEW_ONLY_TOOLS.contains(&name.as_str()) {
                let again = self.per_call.entry(format!("{name}\u{1f}{signature}")).or_insert(0);
                *again += 1;
                if *again >= limits.same_call {
                    return Some(Over::SameCall(name));
                }
            }
        }
        if self.total > limits.tool_calls {
            return Some(Over::TooMany);
        }
        None
    }
}

/// What the person's subject was, in plain words: the area of the tool the turn kept calling.
fn area(tool: &str, lang: &str) -> Option<&'static str> {
    let tool = bare(tool);
    let pick = |fr, en, de, es| Some(match lang { "fr" => fr, "de" => de, "es" => es, _ => en });
    if tool.starts_with("calendar_") {
        pick("l'agenda", "the calendar", "dem Kalender", "la agenda")
    } else if tool.starts_with("reminders_") {
        pick("les rappels", "the reminders", "den Erinnerungen", "los recordatorios")
    } else if tool.starts_with("contacts_") {
        pick("les contacts", "the contacts", "den Kontakten", "los contactos")
    } else if tool.starts_with("notebook_") {
        pick("le Carnet", "the Notebook", "dem Notizbuch", "el Cuaderno")
    } else if tool.starts_with("mail_") {
        pick("les e-mails", "the e-mails", "den E-Mails", "los correos")
    } else if tool.ends_with("_page") || tool == "list_pages" {
        pick("une page", "a page", "einer Seite", "una página")
    } else if crate::runs::browser_tool(tool) {
        pick("le navigateur", "the browser", "dem Browser", "el navegador")
    } else if tool == "web_search" || tool == "web_extract" {
        pick("la recherche web", "the web search", "der Websuche", "la búsqueda web")
    } else {
        None
    }
}

/// The line the person reads in the conversation, in the agent's language (fr, en, de, es; else English).
pub fn notice_text(over: &Over, lang: &str) -> String {
    let lang = lang.split(['-', '_']).next().unwrap_or("en").to_lowercase();
    let lang = lang.as_str();
    match over {
        Over::SameTool(tool) | Over::SameCall(tool) => match (area(tool, lang), lang) {
            (Some(on), "fr") => format!("Je me suis arrêté : je tournais en rond sur {on}. Dis-moi si tu veux que je réessaie."),
            (None, "fr") => "Je me suis arrêté : je refaisais sans cesse la même étape. Dis-moi si tu veux que je réessaie."
                .to_string(),
            (Some(on), "de") => format!(
                "Ich habe aufgehört: Ich habe mich bei {on} im Kreis gedreht. Sag mir, ob ich es noch einmal versuchen soll."
            ),
            (None, "de") => "Ich habe aufgehört: Ich habe immer wieder denselben Schritt gemacht. Sag mir, ob ich es noch \
einmal versuchen soll."
                .to_string(),
            (Some(on), "es") => format!("Me he detenido: estaba dando vueltas con {on}. Dime si quieres que lo intente de nuevo."),
            (None, "es") => "Me he detenido: repetía el mismo paso una y otra vez. Dime si quieres que lo intente de nuevo."
                .to_string(),
            (Some(on), _) => format!("I stopped: I was going round in circles with {on}. Tell me if you want me to try again."),
            (None, _) => "I stopped: I kept repeating the same step. Tell me if you want me to try again.".to_string(),
        },
        Over::TooMany => match lang {
            "fr" => "Je me suis arrêté : cette demande me prenait beaucoup trop d'étapes d'un coup. Dis-moi si tu veux que \
je continue, ou que j'en fasse une tâche de fond."
                .to_string(),
            "de" => "Ich habe aufgehört: Diese Anfrage brauchte viel zu viele Schritte auf einmal. Sag mir, ob ich \
weitermachen oder eine Hintergrundaufgabe daraus machen soll."
                .to_string(),
            "es" => "Me he detenido: esta petición me llevaba demasiados pasos de una vez. Dime si quieres que siga, o que \
la convierta en una tarea en segundo plano."
                .to_string(),
            _ => "I stopped: this request was taking far too many steps at once. Tell me if you want me to go on, or to \
make it a background task."
                .to_string(),
        },
    }
}

/// For the logs.
pub fn reason(over: &Over) -> String {
    match over {
        Over::SameTool(tool) => format!("same tool too many times ({tool})"),
        Over::TooMany => "too many tool calls".to_string(),
        Over::SameCall(tool) => format!("same call repeated ({tool})"),
    }
}

// -- the bridge's write tools ---------------------------------------------------------------------------------------

#[derive(Debug, Default)]
struct WriteTurn {
    /// The runs the turn is (sorted ids; empty: none followed).
    runs: String,
    writes: u32,
    seen: HashSet<String>,
    last: f64,
}

/// Each agent's writes in its current turn.
#[derive(Debug, Default)]
pub struct Writes {
    turns: Mutex<HashMap<String, WriteTurn>>,
}

pub static WRITES: LazyLock<Writes> = LazyLock::new(Writes::default);

impl Writes {
    /// A write tool's call by `agent`, whose runs going on are `runs` (`(run id, background)`, from the hub): None when
    /// it may go on, else the refusal the model gets. Other tools always go on.
    pub fn check(
        &self,
        settings: &Settings,
        agent: &str,
        runs: &[(String, bool)],
        tool: &str,
        arguments: &Dict,
        now: f64,
    ) -> Option<Dict> {
        if !settings.enabled || !write_tool(tool) {
            return None;
        }
        let mut ids: Vec<&str> = runs.iter().map(|(id, _)| id.as_str()).collect();
        ids.sort();
        let key = ids.join(",");
        let background = runs.iter().any(|(_, background)| *background);
        let limits = settings.limits(background);
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let turn = turns.entry(agent.to_string()).or_default();
        if turn.runs != key || (key.is_empty() && now - turn.last > settings.idle_seconds) {
            *turn = WriteTurn { runs: key, ..WriteTurn::default() };
        }
        turn.last = now;
        let signature = format!("{}\u{1f}{}", bare(tool), canonical(&Value::Object(arguments.clone())));
        // Only within a run the hub follows: without one, the « turn » is a guess, and the person may well have asked
        // for the same thing again since (« refais le podcast »).
        if !turn.runs.is_empty() && turn.seen.contains(&signature) {
            tracing::info!(agent = %agent, tool = %tool, "the same write again in this turn: refused");
            return Some(tool_text(
                "Already done in this turn with exactly these arguments: not done again. There is nothing more to do for \
it; tell the person what you did.",
                false,
            ));
        }
        if turn.writes >= limits.writes {
            tracing::info!(agent = %agent, tool = %tool, writes = turn.writes, "too many writes in this turn: refused");
            return Some(tool_text(
                &format!(
                    "Refused: you already made {} changes for the person in this turn, the most allowed at once. Stop \
here: tell the person what you did and ask them before doing more.",
                    turn.writes
                ),
                true,
            ));
        }
        None
    }

    /// A write tool's call that `check` let through went well: counted in the agent's turn (a refused or failed one
    /// is not: the model may correct its arguments and call again).
    pub fn record(&self, agent: &str, tool: &str, arguments: &Dict) {
        if !write_tool(tool) {
            return;
        }
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let turn = turns.entry(agent.to_string()).or_default();
        turn.writes += 1;
        turn.seen.insert(format!("{}\u{1f}{}", bare(tool), canonical(&Value::Object(arguments.clone()))));
    }
}

/// The run event's tool name (Hermes writes `tool`, `tool_name` or `name`).
pub fn event_tool(data: &serde_json::Map<String, Value>) -> String {
    ["tool", "tool_name", "name"]
        .iter()
        .filter_map(|k| data.get(*k).and_then(Value::as_str))
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_string()
}

/// The run event's call signature: its arguments when it has them, else its preview.
pub fn event_signature(data: &serde_json::Map<String, Value>) -> Option<String> {
    match data.get("args").or_else(|| data.get("arguments")) {
        Some(args @ Value::Object(map)) if !map.is_empty() => return Some(canonical(args)),
        Some(Value::String(text)) if !text.trim().is_empty() => return Some(text.clone()),
        _ => {}
    }
    data.get("preview").and_then(Value::as_str).filter(|p| !p.trim().is_empty()).map(str::to_string)
}

/// The notice event's data (`bridge.notice`, code `loop_stopped`).
pub fn notice(over: &Over, lang: &str) -> serde_json::Map<String, Value> {
    let mut data = serde_json::Map::new();
    data.insert("type".into(), json!(crate::runs::NOTICE));
    data.insert("code".into(), json!("loop_stopped"));
    data.insert("reason".into(), json!(reason(over)));
    data.insert("text".into(), json!(notice_text(over, lang)));
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> Dict {
        pairs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect()
    }

    #[test]
    fn the_lab_loop_is_stopped_at_the_eleventh_add() {
        // 2026-10-27: calendar_add and calendar_list in turns, the adds' arguments unknown (no preview)
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let mut stopped = None;
        for i in 0..300 {
            let tool = if i % 2 == 0 { "mcp__bipagents__calendar_add" } else { "mcp__bipagents__calendar_list" };
            if let Some(over) = turn.note(tool, None, false, &limits) {
                stopped = Some((i, over));
                break;
            }
        }
        assert_eq!(stopped, Some((20, Over::SameTool("calendar_add".into()))));
    }

    #[test]
    fn the_same_call_three_times_stops() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let sig = canonical(&json!({"title": "Test", "start": "2026-10-28T10:00"}));
        let other = canonical(&json!({"start": "2026-10-28T10:00", "title": "Test"}));
        assert_eq!(sig, other, "keys sorted");
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits), None);
        assert_eq!(turn.note("skill_view", Some("visible-browser"), false, &limits), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits), None);
        assert_eq!(
            turn.note("mcp_bipagents_calendar_add", Some(&sig), false, &limits),
            Some(Over::SameCall("calendar_add".into()))
        );
    }

    #[test]
    fn work_tools_and_the_browser_only_count_for_the_total() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        for i in 0..29 {
            assert_eq!(turn.note("terminal", Some("cargo test"), false, &limits), None, "{i}");
            assert_eq!(turn.note("mcp__stealth__click_element", Some("{\"x\":1}"), true, &limits), None, "{i}");
        }
        assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), None);
        assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), None);
        assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), Some(Over::SameCall("web_search".into())));
        let mut turn = TurnCount::default();
        let stopped = (0..100).find_map(|i| turn.note("terminal", Some(&format!("ls {i}")), false, &limits));
        assert_eq!(stopped, Some(Over::TooMany));
        assert_eq!(turn.total, 61);
    }

    #[test]
    fn background_tasks_go_further() {
        let settings = Settings::default();
        let mut turn = TurnCount::default();
        let limits = settings.limits(true);
        for _ in 0..40 {
            assert_eq!(turn.note("mcp_bipagents_mail_read", None, false, &limits), None);
        }
        assert_eq!(turn.note("mcp_bipagents_mail_read", None, false, &limits), Some(Over::SameTool("mail_read".into())));
    }

    #[test]
    fn settings_from_the_config() {
        let s = Settings::from_table(&json!({"same_tool": 5, "task_writes": 0, "enabled": false, "idle_seconds": 1}));
        assert!(!s.enabled);
        assert_eq!(s.conversation.same_tool, 5);
        assert_eq!(s.task.writes, 1, "clamped");
        assert_eq!(s.idle_seconds, 10.0);
        assert_eq!(Settings::from_table(&Value::Null), Settings::default());
    }

    #[test]
    fn plain_words_in_four_languages() {
        let looping = Over::SameTool("calendar_add".into());
        assert_eq!(
            notice_text(&looping, "fr"),
            "Je me suis arrêté : je tournais en rond sur l'agenda. Dis-moi si tu veux que je réessaie."
        );
        assert!(notice_text(&looping, "en").contains("the calendar"));
        assert!(notice_text(&looping, "de-DE").contains("Kalender"));
        assert!(notice_text(&looping, "es").contains("la agenda"));
        assert!(notice_text(&Over::SameCall("memory".into()), "fr").contains("la même étape"));
        assert!(notice_text(&Over::TooMany, "it").starts_with("I stopped"));
    }

    #[test]
    fn writes_capped_and_the_same_write_refused() {
        let writes = Writes::default();
        let settings = Settings::default();
        let runs = vec![("run_1".to_string(), false)];
        let add = |n: usize| args(&[("title", &format!("Event {n}")), ("start", "2026-10-28")]);
        // a call let through and done (as the MCP route does)
        let call = |settings: &Settings, agent: &str, runs: &[(String, bool)], tool: &str, a: &Dict, now: f64| {
            let refused = writes.check(settings, agent, runs, tool, a, now);
            if refused.is_none() {
                writes.record(agent, tool, a);
            }
            refused
        };
        assert!(call(&settings, "coach", &runs, "calendar_list", &Dict::new(), 0.0).is_none());
        assert!(call(&settings, "coach", &runs, "calendar_add", &add(0), 0.0).is_none());
        let again = call(&settings, "coach", &runs, "mcp__bipagents__calendar_add", &add(0), 1.0).unwrap();
        assert_eq!(again["isError"], json!(false));
        // a call checked but failed (not recorded) may come again
        assert!(writes.check(&settings, "coach", &runs, "calendar_add", &add(50), 1.0).is_none());
        for n in 1..10 {
            assert!(call(&settings, "coach", &runs, "calendar_add", &add(n), 2.0).is_none(), "{n}");
        }
        let refused = call(&settings, "coach", &runs, "notebook_note", &add(99), 3.0).unwrap();
        assert_eq!(refused["isError"], json!(true));
        // another agent, or the next turn: counted afresh
        assert!(call(&settings, "other", &runs, "calendar_add", &add(0), 3.0).is_none());
        let next = vec![("run_2".to_string(), false)];
        assert!(call(&settings, "coach", &next, "calendar_add", &add(0), 4.0).is_none());
        // no run followed: one turn until quiet for idle_seconds
        let none: Vec<(String, bool)> = Vec::new();
        for n in 0..10 {
            assert!(call(&settings, "solo", &none, "create_audio", &add(0), n as f64).is_none(), "the same, no run: {n}");
        }
        assert!(call(&settings, "solo", &none, "calendar_add", &add(1), 100.0).is_some());
        assert!(call(&settings, "solo", &none, "calendar_add", &add(1), 100.0 + 601.0).is_none());
        // off
        let off = Settings { enabled: false, ..Settings::default() };
        assert!(call(&off, "coach", &next, "calendar_add", &add(0), 5.0).is_none());
    }

    #[test]
    fn signatures_from_the_events() {
        let data = |v: Value| v.as_object().cloned().unwrap();
        assert_eq!(event_tool(&data(json!({"tool_name": "x"}))), "x");
        assert_eq!(event_tool(&data(json!({"tool": "", "name": "y"}))), "y");
        assert_eq!(event_signature(&data(json!({"args": {"b": 1, "a": 2}, "preview": "p"}))).unwrap(), "{\"a\":2,\"b\":1}");
        assert_eq!(event_signature(&data(json!({"args": {}, "preview": "p"}))).unwrap(), "p");
        assert_eq!(event_signature(&data(json!({"preview": " "}))), None);
    }
}
