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
//!   code, files, web search, the visible browser) are not held to `same_tool`, only to the total. The visible
//!   browser has its own, lower limits (`browser_calls` calls, `page_reads` page reads or screenshots in a turn),
//!   well under Hermes' own cap of 150 (tool_diet.rs), so a page the agent can't read ends on « Je me suis arrêté : je
//!   n'arrivais pas à lire cette page… », not on a silent failure; a browsing turn's total is `tool_calls` +
//!   `browser_calls`. Calls a sub-agent makes while it works don't count (as for the steer);
//! - the bridge's own tools are counted at its MCP endpoint, per turn: the run the hub follows for the agent then
//!   (the one that just called this tool when several go on), keyed by its run id, never per agent across
//!   conversations. Its write tools (calendar, reminders, contacts, Carnet, mail drafts, pages, audio, family news)
//!   are refused past `writes` writes in that turn, and the very same write (same tool, same arguments) twice in it is
//!   refused at once with a plain answer the model can act on (« already done »). A turn no run of the hub carries
//!   (a routine, Hermes' cron) is the agent's calls until it has been quiet `idle_seconds`: there, the cap applies,
//!   one bridge tool more than `same_tool` times or more than `routine_tool_calls` bridge calls in all are refused
//!   with « stop and answer with what you have » (the bridge can't stop a cron turn: Hermes has no stop for it).
//!
//! Every run the bridge starts or relays is followed by the hub, whoever looks at it (2026-10-27 evening, the token
//! lab's bench): the app's runs through the bridge's `GET /v1/runs/{id}/events`, but also those started and followed
//! through the door's Hermes relay (`/hermes/<agent>/v1/runs`, lan.rs: the lab, the apps' direct fallbacks), peers'
//! messages (peers.rs), Live, rooms, relatives, background tasks. Before, the relay's runs were invisible to the hub:
//! no stop at all, and their writes counted per agent across conversations (a reminder refused at the first write).
//!
//! Background tasks (jobs.rs, « Tâches de fond »: work with a budget, many calls by design) have their own, higher
//! limits (`task_*`); they are stopped the same way, the task then ends as stopped. The mail scan (mail_scan.rs) makes
//! no tool calls at all (the agent's model reads mails with no tools), so nothing here concerns it.
//!
//! `[brake]` in bridge.toml (every value optional):
//!
//!     enabled = true
//!     same_tool = 10          tool_calls = 60        same_call = 3        writes = 10
//!     browser_calls = 40      page_reads = 15
//!     task_same_tool = 40     task_tool_calls = 120  task_same_call = 5   task_writes = 40
//!     task_browser_calls = 80 task_page_reads = 30
//!     routine_tool_calls = 30 idle_seconds = 120
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
    /// The visible browser: more calls than this stops the turn…
    pub browser_calls: u32,
    /// … and more page reads (its content, a screenshot, a snapshot) than this.
    pub page_reads: u32,
}

/// `[brake]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub conversation: Limits,
    pub task: Limits,
    /// A turn no followed run carries (a routine): more bridge tool calls than this are refused.
    pub routine_tool_calls: u32,
    /// With no run followed: the agent's calls form one turn until it has been quiet this long.
    pub idle_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            conversation: Limits {
                same_tool: 10,
                tool_calls: 60,
                same_call: 3,
                writes: 10,
                browser_calls: 40,
                page_reads: 15,
            },
            // under Hermes' own cap of 150 model calls (tool_diet.rs), so the brake speaks first
            task: Limits { same_tool: 40, tool_calls: 120, same_call: 5, writes: 40, browser_calls: 80, page_reads: 30 },
            routine_tool_calls: 30,
            idle_seconds: 120.0,
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
        let limits = |prefix: &str, d: Limits| Limits {
            same_tool: n(&format!("{prefix}same_tool"), d.same_tool, 2),
            tool_calls: n(&format!("{prefix}tool_calls"), d.tool_calls, 5),
            same_call: n(&format!("{prefix}same_call"), d.same_call, 2),
            writes: n(&format!("{prefix}writes"), d.writes, 1),
            browser_calls: n(&format!("{prefix}browser_calls"), d.browser_calls, 5),
            page_reads: n(&format!("{prefix}page_reads"), d.page_reads, 2),
        };
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(d.enabled),
            conversation: limits("", d.conversation),
            task: limits("task_", d.task),
            routine_tool_calls: n("routine_tool_calls", d.routine_tool_calls, 5),
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

/// The visible browser's tools that read the page (`mcp__stealth__get_page_content`, a screenshot, Hermes'
/// `browser_snapshot`…): the bench's loop read the same page 141 and 145 times.
pub fn page_read_tool(tool: &str) -> bool {
    let tool = tool.to_ascii_lowercase();
    ["page_content", "get_page", "page_text", "read_page", "screenshot", "snapshot", "browser_vision", "get_content"]
        .iter()
        .any(|t| tool.contains(t))
}

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
    /// The visible browser read the page too many times.
    PageReads,
    /// The visible browser, too many calls.
    Browser,
}

/// A turn's tool calls so far.
#[derive(Debug, Default, Clone)]
pub struct TurnCount {
    pub total: u32,
    pub browser: u32,
    pub page_reads: u32,
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
        if browser {
            self.browser += 1;
            if page_read_tool(&name) {
                self.page_reads += 1;
                if self.page_reads > limits.page_reads {
                    return Some(Over::PageReads);
                }
            }
            if self.browser > limits.browser_calls {
                return Some(Over::Browser);
            }
        }
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
        Over::PageReads => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à lire cette page. Dis-moi si tu veux que je réessaie, ou \
donne-moi un autre lien."
                .to_string(),
            "de" => "Ich habe aufgehört: Ich konnte diese Seite nicht lesen. Sag mir, ob ich es noch einmal versuchen \
soll, oder gib mir einen anderen Link."
                .to_string(),
            "es" => "Me he detenido: no conseguía leer esta página. Dime si quieres que lo intente de nuevo, o dame \
otro enlace."
                .to_string(),
            _ => "I stopped: I couldn't read this page. Tell me if you want me to try again, or give me another link."
                .to_string(),
        },
        Over::Browser => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à avancer dans le navigateur. Dis-moi si tu veux que je \
réessaie."
                .to_string(),
            "de" => "Ich habe aufgehört: Ich bin im Browser nicht weitergekommen. Sag mir, ob ich es noch einmal \
versuchen soll."
                .to_string(),
            "es" => "Me he detenido: no conseguía avanzar en el navegador. Dime si quieres que lo intente de nuevo."
                .to_string(),
            _ => "I stopped: I couldn't get anywhere in the browser. Tell me if you want me to try again.".to_string(),
        },
    }
}

/// For the logs.
pub fn reason(over: &Over) -> String {
    match over {
        Over::SameTool(tool) => format!("same tool too many times ({tool})"),
        Over::TooMany => "too many tool calls".to_string(),
        Over::SameCall(tool) => format!("same call repeated ({tool})"),
        Over::PageReads => "the browser read the page too many times".to_string(),
        Over::Browser => "too many browser calls".to_string(),
    }
}

// -- the bridge's own tools, at its MCP endpoint ---------------------------------------------------------------------

/// The turn a call of the bridge's tools belongs to: the run the hub follows (`RunHub::turn_of`), or none (a routine).
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub run_id: String,
    /// A background task's run (jobs.rs): the task's limits.
    pub background: bool,
}

#[derive(Debug, Default)]
struct BridgeTurn {
    writes: u32,
    seen: HashSet<String>,
    /// A turn with no run (a routine): all its bridge calls, by tool and in all.
    calls: u32,
    per_tool: HashMap<String, u32>,
    last: f64,
}

/// Each turn's calls of the bridge's tools: by `agent` + run id, or `agent` alone for the turn no run carries.
#[derive(Debug, Default)]
pub struct Turns {
    turns: Mutex<HashMap<String, BridgeTurn>>,
}

pub static TURNS: LazyLock<Turns> = LazyLock::new(Turns::default);

fn turn_key(agent: &str, turn: Option<&Turn>) -> String {
    format!("{agent}\u{1f}{}", turn.map(|t| t.run_id.as_str()).unwrap_or(""))
}

fn signature_of(tool: &str, arguments: &Dict) -> String {
    format!("{}\u{1f}{}", bare(tool), canonical(&Value::Object(arguments.clone())))
}

impl Turns {
    /// A call of the bridge's tool `tool` by `agent` in `turn`: None when it may go on, else the refusal the model
    /// gets.
    pub fn check(
        &self,
        settings: &Settings,
        agent: &str,
        turn: Option<&Turn>,
        tool: &str,
        arguments: &Dict,
        now: f64,
    ) -> Option<Dict> {
        if !settings.enabled {
            return None;
        }
        let limits = settings.limits(turn.is_some_and(|t| t.background));
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        if turns.len() > 2_000 {
            turns.retain(|_, t| now - t.last < 86_400.0);
        }
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        if turn.is_none() && now - entry.last > settings.idle_seconds {
            *entry = BridgeTurn::default();
        }
        entry.last = now;
        if turn.is_none() {
            // a routine: Hermes can't stop it, so its calls of the bridge's tools are held here
            entry.calls += 1;
            let same = entry.per_tool.entry(bare(tool).to_string()).or_insert(0);
            *same += 1;
            let (same, calls) = (*same, entry.calls);
            if same > limits.same_tool || calls > settings.routine_tool_calls {
                tracing::info!(agent = %agent, tool = %tool, calls = calls, same = same,
                    "going round in circles in a turn no run carries: tool refused");
                return Some(tool_text(
                    &format!(
                        "Refused: {} in this turn already, too many. Stop calling tools now: answer the person with \
what you have, and say plainly what you couldn't find or do.",
                        if same > limits.same_tool {
                            format!("you called {} {} times", bare(tool), same - 1)
                        } else {
                            format!("you made {} calls", calls - 1)
                        }
                    ),
                    true,
                ));
            }
        }
        if !write_tool(tool) {
            return None;
        }
        // Only within a followed run: without one, the « turn » is a guess, and the person may well have asked for
        // the same thing again since (« refais le podcast »).
        if turn.is_some() && entry.seen.contains(&signature_of(tool, arguments)) {
            tracing::info!(agent = %agent, tool = %tool, "the same write again in this turn: refused");
            return Some(tool_text(
                "Already done in this turn with exactly these arguments: not done again. There is nothing more to do for \
it; tell the person what you did.",
                false,
            ));
        }
        if entry.writes >= limits.writes {
            tracing::info!(agent = %agent, tool = %tool, writes = entry.writes, "too many writes in this turn: refused");
            return Some(tool_text(
                &format!(
                    "Refused: you already made {} changes for the person in this turn, the most allowed at once. Stop \
here: tell the person what you did and ask them before doing more.",
                    entry.writes
                ),
                true,
            ));
        }
        None
    }

    /// A write tool's call that `check` let through went well: counted in its turn (a refused or failed one is not:
    /// the model may correct its arguments and call again).
    pub fn record(&self, agent: &str, turn: Option<&Turn>, tool: &str, arguments: &Dict) {
        if !write_tool(tool) {
            return;
        }
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        let entry = turns.entry(turn_key(agent, turn)).or_default();
        entry.writes += 1;
        entry.seen.insert(signature_of(tool, arguments));
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
        let s = Settings::from_table(&json!({"page_reads": 8, "task_browser_calls": 60, "routine_tool_calls": 12}));
        assert_eq!((s.conversation.page_reads, s.task.browser_calls, s.routine_tool_calls), (8, 60, 12));
        assert_eq!(s.conversation.browser_calls, 40);
        // under Hermes' cap of 150 (tool_diet.rs): the brake speaks first
        let d = Settings::default();
        assert!(d.conversation.tool_calls + d.conversation.browser_calls < 150 && d.task.tool_calls < 150);
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
        let turns = Turns::default();
        let settings = Settings::default();
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false });
        let add = |n: usize| args(&[("title", &format!("Event {n}")), ("start", "2026-10-28")]);
        // a call let through and done (as the MCP route does)
        let call = |settings: &Settings, agent: &str, turn: Option<Turn>, tool: &str, a: &Dict, now: f64| {
            let refused = turns.check(settings, agent, turn.as_ref(), tool, a, now);
            if refused.is_none() {
                turns.record(agent, turn.as_ref(), tool, a);
            }
            refused
        };
        assert!(call(&settings, "coach", run("run_1"), "calendar_list", &Dict::new(), 0.0).is_none());
        assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(0), 0.0).is_none());
        let again = call(&settings, "coach", run("run_1"), "mcp__bipagents__calendar_add", &add(0), 1.0).unwrap();
        assert_eq!(again["isError"], json!(false));
        // a call checked but failed (not recorded) may come again
        assert!(turns.check(&settings, "coach", run("run_1").as_ref(), "calendar_add", &add(50), 1.0).is_none());
        for n in 1..10 {
            assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(n), 2.0).is_none(), "{n}");
        }
        let refused = call(&settings, "coach", run("run_1"), "notebook_note", &add(99), 3.0).unwrap();
        assert_eq!(refused["isError"], json!(true));
        // another agent, or the next turn: counted afresh
        assert!(call(&settings, "other", run("run_1"), "calendar_add", &add(0), 3.0).is_none());
        assert!(call(&settings, "coach", run("run_2"), "calendar_add", &add(0), 4.0).is_none());
        // and the first turn is still its own (two runs at once: each its count)
        assert!(call(&settings, "coach", run("run_1"), "calendar_add", &add(77), 4.0).is_some());
        // no run followed: one turn until quiet for idle_seconds
        for n in 0..10 {
            assert!(call(&settings, "solo", None, "create_audio", &add(0), n as f64).is_none(), "the same, no run: {n}");
        }
        assert!(call(&settings, "solo", None, "calendar_add", &add(1), 100.0).is_some());
        assert!(call(&settings, "solo", None, "calendar_add", &add(1), 100.0 + 121.0).is_none());
        // off
        let off = Settings { enabled: false, ..Settings::default() };
        assert!(call(&off, "coach", run("run_2"), "calendar_add", &add(0), 5.0).is_none());
    }

    // -- the token lab's bench of 2026-10-27 (internal/research/token-diet-step3-2026-10-27.md), its real sequences --

    /// A turn's tools as the bench saw them (`turns[].tools`), runs of the same tool.
    fn sequence(runs: &[(&str, usize)]) -> Vec<String> {
        runs.iter().flat_map(|(tool, n)| std::iter::repeat_n(tool.to_string(), *n)).collect()
    }

    /// The run hub's count fed the turn's tool.started events: where it stops (the call's index), and why.
    fn stopped(tools: &[String], signature: impl Fn(&str) -> Option<String>) -> Option<(usize, Over)> {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let mut browsing = false;
        tools.iter().enumerate().find_map(|(i, tool)| {
            let browser = crate::runs::browser_tool(tool);
            browsing |= browser;
            // runs.rs: a browsing turn's total is tool_calls + browser_calls
            let limits = if browsing { Limits { tool_calls: limits.tool_calls + limits.browser_calls, ..limits } } else { limits };
            turn.note(tool, signature(tool).as_deref(), browser, &limits).map(|over| (i, over))
        })
    }

    const NOTE: &str = "mcp__bipagents__notebook_note";
    const SEARCH: &str = "mcp__bipagents__notebook_search";
    const READ_PAGE: &str = "mcp__stealth__get_page_content";

    fn birthday() -> Vec<String> {
        // glm-all-2, birthday_lookup: 20 notebook_note (6 identical), 11 min, no answer
        sequence(&[
            (SEARCH, 1), (NOTE, 1), ("tool_search", 1), (NOTE, 2), ("mcp__bipagents__notebook_read", 1), ("terminal", 1),
            (NOTE, 1), ("tool_call", 1), ("tool_search", 1), (NOTE, 1), ("terminal", 1), (NOTE, 4), ("terminal", 1),
            (NOTE, 3), ("terminal", 1), (NOTE, 8), ("terminal", 1),
        ])
    }

    #[test]
    fn the_bench_birthday_loop_is_stopped() {
        let tools = birthday();
        assert_eq!((tools.len(), tools.iter().filter(|t| *t == NOTE).count()), (30, 20));
        // the bench's events carry no arguments: at the 11th notebook_note
        let (at, over) = stopped(&tools, |_| None).unwrap();
        assert_eq!(over, Over::SameTool("notebook_note".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == NOTE).count(), 11);
        assert_eq!(notice_text(&over, "fr"), "Je me suis arrêté : je tournais en rond sur le Carnet. Dis-moi si tu veux que je réessaie.");
        // with its arguments (the same note again): at the 3rd identical call
        let (at, over) = stopped(&tools, |t| (t == NOTE).then(|| "{\"title\":\"Julie\"}".to_string())).unwrap();
        assert_eq!(over, Over::SameCall("notebook_note".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == NOTE).count(), 3);
        // and at the MCP: the 11th write of the turn refused, the same note again answered « already done »
        let turns = Turns::default();
        let settings = Settings::default();
        let turn = Some(Turn { run_id: "run_1d523504".into(), background: false });
        let mut refused = Vec::new();
        for (i, tool) in tools.iter().enumerate().filter(|(_, t)| *t == NOTE) {
            let a = args(&[("title", "Julie"), ("text", &format!("note {i}"))]);
            match turns.check(&settings, "quotidien", turn.as_ref(), tool, &a, i as f64) {
                None => turns.record("quotidien", turn.as_ref(), tool, &a),
                Some(r) => refused.push(r),
            }
        }
        assert_eq!(refused.len(), 10);
        assert!(refused.iter().all(|r| r["isError"] == json!(true)));
        let same = args(&[("title", "Julie"), ("text", "note 1")]);
        assert_eq!(turns.check(&settings, "quotidien", turn.as_ref(), NOTE, &same, 40.0).unwrap()["isError"], json!(false));
    }

    #[test]
    fn the_bench_draft_loop_is_stopped() {
        // glm-concise-2, draft_reply: 25 notebook_search
        let tools = sequence(&[("mcp__bipagents__calendar_list", 1), (SEARCH, 2), ("tool_describe", 1), (SEARCH, 23)]);
        let (at, over) = stopped(&tools, |_| None).unwrap();
        assert_eq!(over, Over::SameTool("notebook_search".into()));
        assert_eq!(tools[..=at].iter().filter(|t| *t == SEARCH).count(), 11, "the 11th search");
    }

    #[test]
    fn the_bench_browser_loops_are_stopped_before_hermes_cap() {
        // glm-base-2 and glm-concise-2, browser_public: 141 and 145 page reads, cut by Hermes at 150, no answer
        let base = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 1), (READ_PAGE, 1),
            ("tool_search", 1), ("search_files", 1), ("mcp__stealth__navigate", 1), ("tool_search", 1), (READ_PAGE, 3),
            ("tool_describe", 1), (READ_PAGE, 5), ("mcp__stealth__execute_script", 1), (READ_PAGE, 132),
        ]);
        let concise = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 1), (READ_PAGE, 1),
            ("terminal", 1), ("tool_describe", 1), (READ_PAGE, 144),
        ]);
        for (tools, reads) in [(base, 141), (concise, 145)] {
            assert_eq!((tools.len(), tools.iter().filter(|t| *t == READ_PAGE).count()), (150, reads));
            // the bridge's events carry the page reads' arguments: the same each time, still not « same call » (the
            // browser's), but the page reads' own limit
            let (at, over) = stopped(&tools, |t| Some(format!("{t} {{}}"))).unwrap();
            assert_eq!(over, Over::PageReads);
            assert_eq!(tools[..=at].iter().filter(|t| *t == READ_PAGE).count(), 16);
            assert!(at < 30, "long before Hermes' 150: {at}");
        }
        assert_eq!(
            notice_text(&Over::PageReads, "fr"),
            "Je me suis arrêté : je n'arrivais pas à lire cette page. Dis-moi si tu veux que je réessaie, ou donne-moi un \
autre lien."
        );
        // browsing that goes on without reading the page: at its own limit, still under Hermes' cap
        let clicks = sequence(&[("mcp__stealth__click_element", 200)]);
        let (at, over) = stopped(&clicks, |_| None).unwrap();
        assert_eq!((at, over), (40, Over::Browser));
    }

    #[test]
    fn the_bench_legitimate_turns_go_on() {
        // the bench's good turns: browsing that reads a few pages, a long file delivery with the terminal
        let browsing = sequence(&[
            ("skill_view", 1), ("mcp__stealth__spawn_browser", 1), ("mcp__stealth__navigate", 6), (READ_PAGE, 6),
            ("mcp__bipagents__calendar_list", 1),
        ]);
        assert_eq!(stopped(&browsing, |_| None), None);
        let files = sequence(&[("terminal", 12), ("write_file", 10), ("read_file", 10)]);
        assert_eq!(stopped(&files, |_| None), None);
    }

    #[test]
    fn the_bench_false_positives_are_allowed() {
        // DeepSeek (reminder ×2, plan_day, map_walk) and the morning brief: refused at the first or second write of a
        // conversation, because earlier conversations' writes of the same agent were counted with them
        let turns = Turns::default();
        let settings = Settings::default();
        let call = |agent: &str, turn: Option<Turn>, tool: &str, a: &Dict, now: f64| {
            let refused = turns.check(&settings, agent, turn.as_ref(), tool, a, now);
            if refused.is_none() {
                turns.record(agent, turn.as_ref(), tool, a);
            }
            refused
        };
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false });
        // an earlier conversation of the agent wrote 20 notes (its last ones refused)
        for i in 0..20 {
            call("quotidien", run("run_birthday"), NOTE, &args(&[("text", &format!("n{i}"))]), i as f64);
        }
        // the next conversations, a minute later each
        let reminder = |text: &str| args(&[("title", text), ("due", "2026-10-28T09:00")]);
        assert!(call("quotidien", run("run_reminder"), "mcp__bipagents__reminders_add", &reminder("Pharmacie"), 60.0).is_none());
        assert!(call("quotidien", run("run_reminder"), "mcp__bipagents__reminders_add", &reminder("Banque"), 61.0).is_none());
        let page = args(&[("title", "Ma journée"), ("html", "<h1>Mardi</h1>")]);
        assert!(call("quotidien", run("run_plan_day"), "mcp__bipagents__create_page", &page, 120.0).is_none());
        assert!(call("quotidien", run("run_map_walk"), "mcp__bipagents__create_page", &page, 180.0).is_none());
        // the morning brief (a routine, no run): its page, after the bench's turns
        assert!(call("quotidien", None, "mcp__bipagents__create_page", &page, 240.0).is_none());
        // a routine's search loop (35 Carnet searches in the bench): refused from the 11th, in plain words
        let mut refused = None;
        for i in 0..35 {
            if let Some(r) = call("budget", None, SEARCH, &args(&[("query", &format!("ville {i}"))]), 1000.0 + i as f64) {
                refused = Some((i, r));
                break;
            }
        }
        let (i, r) = refused.unwrap();
        assert_eq!(i, 10);
        assert!(r["content"][0]["text"].as_str().unwrap().contains("answer the person with what you have"));
        // and a routine's calls of many tools: at routine_tool_calls
        let mut at = None;
        for i in 0..40 {
            let tool = format!("mcp__bipagents__tool_{}", i % 8);
            if call("coach", None, &tool, &Dict::new(), 5000.0 + i as f64).is_some() {
                at = Some(i);
                break;
            }
        }
        assert_eq!(at, Some(30));
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
