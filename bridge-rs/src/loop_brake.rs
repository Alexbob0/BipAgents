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
//!     same_tool = 10          tool_calls = 60        same_call = 3        same_lookup = 5      writes = 10
//!     browser_calls = 40      page_reads = 15        discovery_same = 5   discovery_calls = 8
//!     task_same_tool = 40     task_tool_calls = 120  task_same_call = 5   task_same_lookup = 8 task_writes = 40
//!     task_browser_calls = 80 task_page_reads = 30   task_discovery_same = 5 task_discovery_calls = 15
//!     routine_tool_calls = 40 idle_seconds = 120
//!     rescue = true           rescue_tool_calls = 5  rescue_seconds = 180
//!
//! Searching its own tools (2026-10-28, real-day bench: 9 `tool_search` and 2 `tool_describe`, all different, in 3
//! minutes, unheld): Hermes' `tool_search` and `tool_describe` count apart. The same query (`discovery_query`: its
//! words, whatever the options) is told at 3 and stops the turn at `discovery_same` (5); different queries count
//! toward `discovery_calls` (8 in all, told at 5). The usual plain line follows (« je cherchais sans fin un outil
//! pour le faire »), then the rescue turn, which may not search its tools again.
//!
//! Read-only lookups (2026-10-28, Alex's home server: Vie diagnosing a nightly job was stopped after 5 calls, 3 of
//! them the same `session_search`): searching past conversations, the Carnet, the web, files, mails or the memory
//! the same way again is how a diagnosis goes, so it takes `same_lookup` identical calls (5), not `same_call` (3,
//! for writes and actions). A call made again right after the identical call failed (`tool.completed` with an
//! error) is a retry, never counted as « the same call ».
//!
//! The rescue turn (2026-10-28, Alex: a bare « I stopped » helps nobody): a person's conversation turn the brake
//! stopped is followed by ONE short turn of the same agent in the same conversation (runs.rs `loop_rescue`), told
//! out of sight (`rescue_input`, its row hidden by the apps: `[bipagents:loop_rescue]`) why it was stopped, not to
//! call that tool again, and to answer with what it has: what it tried, what it found, what blocked it, one or two
//! ways forward. That turn has tight limits (`rescue_limits`: `rescue_tool_calls` calls, the stopped tool refused at
//! the bridge's MCP and stopping the turn at its second call) and is never rescued itself. The person reads the
//! brake's short line (`notice_head`), then the agent's own reply; when the rescue fails too, the full plain line.
//! Background tasks and routines get no rescue turn.
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
    /// The same call (tool and arguments) this many times stops it…
    pub same_call: u32,
    /// … but a read-only lookup's (`lookup_tool`: past conversations, the Carnet, the web, files…) this many times.
    pub same_lookup: u32,
    /// The bridge's write tools refuse past this many writes.
    pub writes: u32,
    /// The visible browser: more calls than this stops the turn…
    pub browser_calls: u32,
    /// … and more page reads (its content, a screenshot, a snapshot) than this.
    pub page_reads: u32,
    /// Searching its own tools (Hermes' `tool_search`, `tool_describe`): the same query this many times stops the
    /// turn…
    pub discovery_same: u32,
    /// … and more discovery calls than this in all, whatever the queries.
    pub discovery_calls: u32,
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
    /// A limit getting close (`soft_limit`, about two thirds) is told to the agent once, before any stop.
    pub warn: bool,
    /// A stopped conversation turn is followed by one short turn that explains (runs.rs `loop_rescue`)…
    pub rescue: bool,
    /// … with at most this many tool calls…
    pub rescue_tool_calls: u32,
    /// … and this long at most before it is stopped too.
    pub rescue_seconds: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            conversation: Limits {
                same_tool: 10,
                tool_calls: 60,
                same_call: 3,
                same_lookup: 5,
                writes: 10,
                browser_calls: 40,
                page_reads: 15,
                discovery_same: 5,
                discovery_calls: 8,
            },
            // under Hermes' own cap of 150 model calls (tool_diet.rs), so the brake speaks first
            task: Limits {
                same_tool: 40,
                tool_calls: 120,
                same_call: 5,
                same_lookup: 8,
                writes: 40,
                browser_calls: 80,
                page_reads: 30,
                discovery_same: 5,
                discovery_calls: 15,
            },
            routine_tool_calls: 40,
            idle_seconds: 120.0,
            warn: true,
            rescue: true,
            rescue_tool_calls: 5,
            rescue_seconds: 180.0,
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
            same_lookup: n(&format!("{prefix}same_lookup"), d.same_lookup, 2),
            writes: n(&format!("{prefix}writes"), d.writes, 1),
            browser_calls: n(&format!("{prefix}browser_calls"), d.browser_calls, 5),
            page_reads: n(&format!("{prefix}page_reads"), d.page_reads, 2),
            discovery_same: n(&format!("{prefix}discovery_same"), d.discovery_same, 2),
            discovery_calls: n(&format!("{prefix}discovery_calls"), d.discovery_calls, 2),
        };
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(d.enabled),
            conversation: limits("", d.conversation),
            task: limits("task_", d.task),
            routine_tool_calls: n("routine_tool_calls", d.routine_tool_calls, 5),
            idle_seconds: table.get("idle_seconds").and_then(Value::as_f64).unwrap_or(d.idle_seconds).clamp(10.0, 86_400.0),
            warn: table.get("warn").and_then(Value::as_bool).unwrap_or(d.warn),
            rescue: table.get("rescue").and_then(Value::as_bool).unwrap_or(d.rescue),
            rescue_tool_calls: n("rescue_tool_calls", d.rescue_tool_calls, 1),
            rescue_seconds: table
                .get("rescue_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(d.rescue_seconds)
                .clamp(20.0, 1_800.0),
        }
    }

    /// The rescue turn's limits: a few calls to answer, never a new round of work.
    pub fn rescue_limits(&self) -> Limits {
        let calls = self.rescue_tool_calls;
        Limits {
            same_tool: 3,
            tool_calls: calls,
            same_call: 2,
            same_lookup: 2,
            writes: 2,
            browser_calls: calls,
            page_reads: 2,
            discovery_same: 2,
            discovery_calls: 2,
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

/// Read-only lookups: searching or reading what is there (Hermes' past conversations, the Carnet, the web, files,
/// mails, the calendar, contacts, the agent's pages, skills, its memory read), held to `same_lookup`.
const LOOKUP_TOOLS: [&str; 14] = [
    "session_search",
    "web_search",
    "web_extract",
    "read_file",
    "search_files",
    "notebook_search",
    "notebook_read",
    "read_page",
    "list_pages",
    "skill_view",
    "skills_list",
    "tool_search",
    "tool_describe",
    "message_search",
];

/// `signature`: the call's arguments or preview (Hermes' `memory` both reads and writes: a read only by its action).
pub fn lookup_tool(tool: &str, signature: Option<&str>) -> bool {
    let name = bare(tool);
    if write_tool(name) {
        return false;
    }
    if name == "memory" {
        let s = signature.unwrap_or("").to_ascii_lowercase();
        return ["\"read\"", "\"view\"", "\"search\"", "\"list\""].iter().any(|a| s.contains(a))
            || s.starts_with("read")
            || s.starts_with("search");
    }
    LOOKUP_TOOLS.contains(&name)
        || name.starts_with("knowledge_")
        || ["_search", "_list", "_read", "_get"].iter().any(|end| name.ends_with(end))
}

/// Hermes' tool search (tool_diet.rs: the tools behind it): looking for a tool, not doing the work. The real-day
/// bench of 2026-10-28 (GLM « medium », « ask Coach » with no message_agent in view) made 9 `tool_search` and 2
/// `tool_describe` in 3 minutes, all different, and nothing held it: each stayed under `same_tool` and `same_lookup`.
pub const DISCOVERY_TOOLS: [&str; 2] = ["tool_search", "tool_describe"];

pub fn discovery_tool(tool: &str) -> bool {
    DISCOVERY_TOOLS.contains(&bare(tool))
}

/// What a discovery call looked for (its query or the tool it describes, in lower case, spaces folded): the same
/// query again with other options is the same search. A signature that isn't JSON (a preview, a hash) is used whole.
pub fn discovery_query(signature: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(signature).ok();
    let found = parsed.as_ref().and_then(|v| {
        ["query", "q", "name", "tool", "tool_name", "names"]
            .iter()
            .find_map(|k| v.get(*k))
            .map(|q| q.as_str().map(str::to_string).unwrap_or_else(|| q.to_string()))
    });
    let text = found.unwrap_or_else(|| signature.to_string()).to_lowercase();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tool's answer that says it failed (`tool.completed`: Hermes' `error`, or an error in its preview).
pub fn completed_with_error(data: &serde_json::Map<String, Value>) -> bool {
    if data.get("error").and_then(Value::as_bool).unwrap_or(false) {
        return true;
    }
    let preview = data.get("preview").and_then(Value::as_str).unwrap_or("").trim_start();
    let head = preview.chars().take(80).collect::<String>().to_ascii_lowercase();
    head.starts_with("error")
        || head.starts_with("{\"error\"")
        || head.contains("\"success\": false")
        || head.contains("\"success\":false")
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
    /// Searching its own tools again and again (the same query, or too many searches).
    Discovery,
}

/// A turn's tool calls so far.
#[derive(Debug, Default, Clone)]
pub struct TurnCount {
    pub total: u32,
    pub browser: u32,
    pub page_reads: u32,
    per_tool: HashMap<String, u32>,
    per_call: HashMap<String, u32>,
    /// Discovery calls (`DISCOVERY_TOOLS`) in all, and by query.
    pub discovery: u32,
    per_query: HashMap<String, u32>,
    /// The turn's last call (tool and arguments), and whether it answered with an error: the identical call right
    /// after is a retry, not « the same call again ».
    last_call: Option<String>,
    last_tool: String,
    last_failed: bool,
    /// A rescue turn's stopped tool (bare name): its second call stops the turn (the first is refused at the MCP for
    /// the bridge's tools, and the rescue's instruction says not to call it).
    pub banned: Option<String>,
    banned_calls: u32,
    /// The limits already brought close in this turn (told once each), and the one the last call brought close.
    warned: HashSet<String>,
    warning: Option<Warning>,
}

/// About two thirds of a hard limit: where the agent is told it gets close (same call 3 of 5 for a lookup, 2 of 3
/// for a write, 40 of 60 calls, 7 of 10 for one tool).
pub fn soft_limit(hard: u32) -> u32 {
    (2 * hard + 1) / 3
}

/// A limit the turn gets close to: the agent is told, in a steer (runs.rs `brake`), before being stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    /// The same call (tool, how many times, the hard limit it is stopped at).
    SameCall(String, u32, u32),
    /// One tool many times (tool, how many times, the hard limit it is stopped past).
    SameTool(String, u32, u32),
    /// Many calls in all, many page reads, many browser calls (how many, the hard limit).
    TooMany(u32, u32),
    PageReads(u32, u32),
    Browser(u32, u32),
    /// Tool searches: the same query (how many times, the hard limit it is stopped at)…
    DiscoverySame(u32, u32),
    /// … or many in all (how many, the hard limit it is stopped past).
    Discovery(u32, u32),
}

/// The steers the bridge's brake gives start with this (the apps hide such rows: `[bipagents:`).
pub const WARN_MARK: &str = "[bipagents:brake]";

impl Warning {
    fn key(&self) -> String {
        match self {
            Warning::SameCall(tool, _, _) => format!("call:{tool}"),
            Warning::SameTool(tool, _, _) => format!("tool:{tool}"),
            Warning::TooMany(..) => "total".into(),
            Warning::PageReads(..) => "page_reads".into(),
            Warning::Browser(..) => "browser".into(),
            Warning::DiscoverySame(..) => "discovery_same".into(),
            Warning::Discovery(..) => "discovery".into(),
        }
    }

    /// For the logs.
    pub fn reason(&self) -> String {
        match self {
            Warning::SameCall(tool, n, _) => format!("same call {n} times ({tool})"),
            Warning::SameTool(tool, n, _) => format!("same tool {n} times ({tool})"),
            Warning::TooMany(n, _) => format!("{n} tool calls"),
            Warning::PageReads(n, _) => format!("the browser read the page {n} times"),
            Warning::Browser(n, _) => format!("{n} browser calls"),
            Warning::DiscoverySame(n, _) => format!("the same tool search {n} times"),
            Warning::Discovery(n, _) => format!("{n} tool searches"),
        }
    }

    /// The note the agent gets (a steer, after its current tool calls), in English, out of the person's sight.
    pub fn text(&self) -> String {
        let what = match self {
            Warning::SameCall(tool, n, hard) => {
                format!("you have made exactly the same {tool} call {n} times in this turn; at {hard} the turn is stopped")
            }
            Warning::SameTool(tool, count, hard) => {
                format!("you have called {tool} {count} times in this turn; past {hard} the turn is stopped")
            }
            Warning::TooMany(count, hard) => {
                format!("you have made {count} tool calls in this turn; past {hard} the turn is stopped")
            }
            Warning::PageReads(count, hard) => {
                format!("you have read the page {count} times in this turn; past {hard} the turn is stopped")
            }
            Warning::Browser(count, hard) => {
                format!("you have made {count} browser steps in this turn; past {hard} the turn is stopped")
            }
            Warning::DiscoverySame(count, hard) => format!(
                "you have searched your tools for the same thing {count} times in this turn; at {hard} the turn is \
stopped. If no tool does it, say so to the person"
            ),
            Warning::Discovery(count, hard) => format!(
                "you have searched your tools {count} times in this turn; past {hard} the turn is stopped. Use a tool \
you found, or tell the person plainly what you can't do"
            ),
        };
        format!(
            "{WARN_MARK} A note from the bridge, not from your person: {what}. Change approach (other words, another \
source, another tool), or answer the person now with what you have and say what is missing."
        )
    }
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
        let key = signature.filter(|s| !s.trim().is_empty()).map(|s| format!("{name}\u{1f}{s}"));
        let retry = self.last_failed && key.is_some() && self.last_call == key;
        self.last_call = key;
        self.last_tool = name.clone();
        self.last_failed = false;
        if self.banned.as_deref() == Some(name.as_str()) {
            self.banned_calls += 1;
            if self.banned_calls > 1 {
                return Some(Over::SameCall(name));
            }
        }
        // the first limit getting close: told once before the stop (`warning`)
        let mut close: Option<Warning> = None;
        let mut near = |count: u32, hard: u32, at_hard: bool, warning: &dyn Fn() -> Warning| {
            // `at_hard`: the limit is reached at `hard` (else passed after it)
            let soft = soft_limit(hard);
            if close.is_none() && soft >= 2 && soft < hard + u32::from(!at_hard) && count == soft {
                close = Some(warning());
            }
        };
        if browser {
            self.browser += 1;
            if page_read_tool(&name) {
                self.page_reads += 1;
                if self.page_reads > limits.page_reads {
                    return Some(Over::PageReads);
                }
                near(self.page_reads, limits.page_reads, false, &|| Warning::PageReads(self.page_reads, limits.page_reads));
            }
            if self.browser > limits.browser_calls {
                return Some(Over::Browser);
            }
            near(self.browser, limits.browser_calls, false, &|| Warning::Browser(self.browser, limits.browser_calls));
        }
        if discovery_tool(&name) {
            self.discovery += 1;
            let discovery = self.discovery;
            if let Some(query) = signature.filter(|s| !s.trim().is_empty()).map(discovery_query) {
                let again = self.per_query.entry(query).or_insert(0);
                *again += 1;
                let again = *again;
                if again >= limits.discovery_same {
                    return Some(Over::Discovery);
                }
                near(again, limits.discovery_same, true, &|| Warning::DiscoverySame(again, limits.discovery_same));
            }
            if discovery > limits.discovery_calls {
                return Some(Over::Discovery);
            }
            near(discovery, limits.discovery_calls, false, &|| Warning::Discovery(discovery, limits.discovery_calls));
        }
        let work = browser || WORK_TOOLS.contains(&name.as_str());
        let same = self.per_tool.entry(name.clone()).or_insert(0);
        *same += 1;
        let same = *same;
        if !work && same > limits.same_tool {
            return Some(Over::SameTool(name));
        }
        if !work {
            near(same, limits.same_tool, false, &|| Warning::SameTool(name.clone(), same, limits.same_tool));
        }
        if let Some(signature) = signature.filter(|s| !s.trim().is_empty()) {
            if !browser && !PREVIEW_ONLY_TOOLS.contains(&name.as_str()) && !retry {
                let again = self.per_call.entry(format!("{name}\u{1f}{signature}")).or_insert(0);
                *again += 1;
                let again = *again;
                let limit = if lookup_tool(&name, Some(signature)) { limits.same_lookup } else { limits.same_call };
                if again >= limit {
                    return Some(Over::SameCall(name));
                }
                near(again, limit, true, &|| Warning::SameCall(name.clone(), again, limit));
            }
        }
        if self.total > limits.tool_calls {
            return Some(Over::TooMany);
        }
        near(self.total, limits.tool_calls, false, &|| Warning::TooMany(self.total, limits.tool_calls));
        if let Some(warning) = close {
            if self.warned.insert(warning.key()) {
                self.warning = Some(warning);
            }
        }
        None
    }

    /// The limit the last call brought close (once per kind and tool in a turn), for the note the agent gets.
    pub fn take_warning(&mut self) -> Option<Warning> {
        self.warning.take()
    }

    /// The turn's call of `tool` answered (`tool.completed`): `failed` when with an error.
    pub fn completed(&mut self, tool: &str, failed: bool) {
        if bare(tool) == self.last_tool {
            self.last_failed = failed;
        }
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

/// A read-only lookup's purpose in plain words, as the apps word it (HermesKit ToolKind, the app's ToolKindText:
/// « Je cherche dans nos anciennes discussions… »): what the turn kept doing again, never a tool's name.
fn lookup_phrase(tool: &str, lang: &str) -> Option<&'static str> {
    let tool = bare(tool);
    let pick = |fr, en, de, es| Some(match lang { "fr" => fr, "de" => de, "es" => es, _ => en });
    if tool == "session_search" || tool == "message_search" {
        pick(
            "la même recherche dans nos anciennes discussions",
            "the same search in our past conversations",
            "dieselbe Suche in unseren früheren Unterhaltungen",
            "la misma búsqueda en nuestras conversaciones anteriores",
        )
    } else if tool.starts_with("notebook_") {
        pick("la même recherche dans le Carnet", "the same search in the Notebook", "dieselbe Suche im Notizbuch", "la misma búsqueda en el Cuaderno")
    } else if tool == "web_search" {
        pick("la même recherche sur le web", "the same web search", "dieselbe Websuche", "la misma búsqueda en la web")
    } else if tool == "web_extract" {
        pick(
            "la même lecture d'une page web",
            "the same reading of a web page",
            "dasselbe Lesen einer Webseite",
            "la misma lectura de una página web",
        )
    } else if tool == "read_file" || tool == "search_files" {
        pick("la même lecture de fichier", "the same file reading", "dasselbe Lesen einer Datei", "la misma lectura de un archivo")
    } else if tool.starts_with("mail_") {
        pick("la même recherche dans les e-mails", "the same search in the e-mails", "dieselbe Suche in den E-Mails", "la misma búsqueda en los correos")
    } else if tool.starts_with("calendar_") {
        pick("la même consultation de l'agenda", "the same look at the calendar", "denselben Blick in den Kalender", "la misma consulta de la agenda")
    } else if tool.starts_with("contacts_") {
        pick("la même recherche dans les contacts", "the same search in the contacts", "dieselbe Suche in den Kontakten", "la misma búsqueda en los contactos")
    } else if tool.starts_with("reminders_") {
        pick("la même consultation des rappels", "the same look at the reminders", "denselben Blick in die Erinnerungen", "la misma consulta de los recordatorios")
    } else if tool == "memory" || tool.starts_with("knowledge_") {
        pick("la même lecture de ma mémoire", "the same look into my memory", "denselben Blick in mein Gedächtnis", "la misma consulta de mi memoria")
    } else if tool == "read_page" || tool == "list_pages" {
        pick("la même lecture de mes pages", "the same look at my pages", "denselben Blick in meine Seiten", "la misma consulta de mis páginas")
    } else {
        None
    }
}

fn language(lang: &str) -> String {
    lang.split(['-', '_']).next().unwrap_or("en").to_lowercase()
}

/// « the same call again » of a lookup: said as what it was for (« je relançais la même recherche dans nos anciennes
/// discussions »).
fn repeated_lookup(over: &Over, lang: &str) -> Option<&'static str> {
    match over {
        Over::SameCall(tool) if lookup_tool(tool, None) => lookup_phrase(tool, lang),
        _ => None,
    }
}

/// The line's first sentence: what happened. Alone above the rescue turn's reply (runs.rs `loop_rescue`).
pub fn notice_head(over: &Over, lang: &str) -> String {
    let lang = language(lang);
    let lang = lang.as_str();
    if let Some(what) = repeated_lookup(over, lang) {
        return match lang {
            "fr" => format!("Je me suis arrêté : je relançais {what}."),
            "de" => format!("Ich habe aufgehört: Ich habe immer wieder {what} gestartet."),
            "es" => format!("Me he detenido: repetía {what}."),
            _ => format!("I stopped: I kept repeating {what}."),
        };
    }
    match over {
        Over::SameTool(tool) | Over::SameCall(tool) => match (area(tool, lang), lang) {
            (Some(on), "fr") => format!("Je me suis arrêté : je tournais en rond sur {on}."),
            (None, "fr") => "Je me suis arrêté : je refaisais sans cesse la même étape.".to_string(),
            (Some(on), "de") => format!("Ich habe aufgehört: Ich habe mich bei {on} im Kreis gedreht."),
            (None, "de") => "Ich habe aufgehört: Ich habe immer wieder denselben Schritt gemacht.".to_string(),
            (Some(on), "es") => format!("Me he detenido: estaba dando vueltas con {on}."),
            (None, "es") => "Me he detenido: repetía el mismo paso una y otra vez.".to_string(),
            (Some(on), _) => format!("I stopped: I was going round in circles with {on}."),
            (None, _) => "I stopped: I kept repeating the same step.".to_string(),
        },
        Over::TooMany => match lang {
            "fr" => "Je me suis arrêté : cette demande me prenait beaucoup trop d'étapes d'un coup.",
            "de" => "Ich habe aufgehört: Diese Anfrage brauchte viel zu viele Schritte auf einmal.",
            "es" => "Me he detenido: esta petición me llevaba demasiados pasos de una vez.",
            _ => "I stopped: this request was taking far too many steps at once.",
        }
        .to_string(),
        Over::PageReads => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à lire cette page.",
            "de" => "Ich habe aufgehört: Ich konnte diese Seite nicht lesen.",
            "es" => "Me he detenido: no conseguía leer esta página.",
            _ => "I stopped: I couldn't read this page.",
        }
        .to_string(),
        Over::Browser => match lang {
            "fr" => "Je me suis arrêté : je n'arrivais pas à avancer dans le navigateur.",
            "de" => "Ich habe aufgehört: Ich bin im Browser nicht weitergekommen.",
            "es" => "Me he detenido: no conseguía avanzar en el navegador.",
            _ => "I stopped: I couldn't get anywhere in the browser.",
        }
        .to_string(),
        Over::Discovery => match lang {
            "fr" => "Je me suis arrêté : je cherchais sans fin un outil pour le faire.",
            "de" => "Ich habe aufgehört: Ich habe endlos nach einem Werkzeug dafür gesucht.",
            "es" => "Me he detenido: buscaba sin parar una herramienta para hacerlo.",
            _ => "I stopped: I kept looking for a tool to do it.",
        }
        .to_string(),
    }
}

/// The line's second sentence: what the person can do.
pub fn notice_tail(over: &Over, lang: &str) -> &'static str {
    let lang = language(lang);
    let lang = lang.as_str();
    if repeated_lookup(over, lang).is_some() {
        return match lang {
            "fr" => "Dis-moi si tu veux que je réessaie autrement.",
            "de" => "Sag mir, ob ich es anders versuchen soll.",
            "es" => "Dime si quieres que lo intente de otra manera.",
            _ => "Tell me if you want me to try another way.",
        };
    }
    match over {
        Over::TooMany => match lang {
            "fr" => "Dis-moi si tu veux que je continue, ou que j'en fasse une tâche de fond.",
            "de" => "Sag mir, ob ich weitermachen oder eine Hintergrundaufgabe daraus machen soll.",
            "es" => "Dime si quieres que siga, o que la convierta en una tarea en segundo plano.",
            _ => "Tell me if you want me to go on, or to make it a background task.",
        },
        Over::PageReads => match lang {
            "fr" => "Dis-moi si tu veux que je réessaie, ou donne-moi un autre lien.",
            "de" => "Sag mir, ob ich es noch einmal versuchen soll, oder gib mir einen anderen Link.",
            "es" => "Dime si quieres que lo intente de nuevo, o dame otro enlace.",
            _ => "Tell me if you want me to try again, or give me another link.",
        },
        _ => match lang {
            "fr" => "Dis-moi si tu veux que je réessaie.",
            "de" => "Sag mir, ob ich es noch einmal versuchen soll.",
            "es" => "Dime si quieres que lo intente de nuevo.",
            _ => "Tell me if you want me to try again.",
        },
    }
}

/// The line the person reads in the conversation, in the agent's language (fr, en, de, es; else English).
pub fn notice_text(over: &Over, lang: &str) -> String {
    format!("{} {}", notice_head(over, lang), notice_tail(over, lang))
}

/// The rescue turn's input starts with this (the apps hide the row, but for its plain line: HermesKit StoppedTurn,
/// Android Hermes.kt); `person_request` skips it (`[bipagents:`).
pub const RESCUE_MARK: &str = "[bipagents:loop_rescue]";

/// The tool a rescue turn may not call again (bare name): the one the turn kept calling.
pub fn stopped_tool(over: &Over) -> Option<String> {
    match over {
        Over::SameTool(tool) | Over::SameCall(tool) => Some(tool.clone()),
        Over::Discovery => Some("tool_search".into()),
        _ => None,
    }
}

/// The rescue turn's input (runs.rs `loop_rescue`): the mark, the person's line (in their language: what the apps
/// show of this row), then the agent's instruction, in English, out of the person's sight.
pub fn rescue_input(over: &Over, lang: &str) -> String {
    let what = match over {
        Over::SameCall(tool) => match lookup_phrase(tool, "en").filter(|_| lookup_tool(tool, None)) {
            Some(phrase) => format!("repeated {phrase} ({tool}) with the same arguments again and again"),
            None => format!("made exactly the same {tool} call again and again"),
        },
        Over::SameTool(tool) => format!("called {tool} too many times in one turn"),
        Over::TooMany => "made far too many tool calls in one turn".to_string(),
        Over::PageReads => "kept reading the same page in the browser without managing to".to_string(),
        Over::Browser => "made too many browser steps without getting anywhere".to_string(),
        Over::Discovery => "kept searching your tools (tool_search, tool_describe) for one to do it".to_string(),
    };
    let dont = match over {
        Over::SameCall(tool) | Over::SameTool(tool) => format!("Don't call {tool} again in this turn."),
        Over::PageReads | Over::Browser => "Don't use the browser again in this turn.".to_string(),
        Over::TooMany => "Don't start new work in this turn.".to_string(),
        Over::Discovery => {
            "Don't search your tools again in this turn: if none you have does it, say so plainly.".to_string()
        }
    };
    format!(
        "{RESCUE_MARK} {head}\n\n(A note from the bridge, not from your person.) You were stopped because you {what}. \
{dont} With what you already have, answer the person in their language: what you were trying to do, what you found \
so far, what blocked you, and propose one or two concrete ways forward (or ask one precise question). Use another \
tool only if it is really needed, two at most.",
        head = notice_head(over, lang)
    )
}

/// For the logs.
pub fn reason(over: &Over) -> String {
    match over {
        Over::SameTool(tool) => format!("same tool too many times ({tool})"),
        Over::TooMany => "too many tool calls".to_string(),
        Over::SameCall(tool) => format!("same call repeated ({tool})"),
        Over::PageReads => "the browser read the page too many times".to_string(),
        Over::Browser => "too many browser calls".to_string(),
        Over::Discovery => "too many tool searches".to_string(),
    }
}

// -- the bridge's own tools, at its MCP endpoint ---------------------------------------------------------------------

/// The turn a call of the bridge's tools belongs to: the run the hub follows (`RunHub::turn_of`), or none (a routine).
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub run_id: String,
    /// A background task's run (jobs.rs): the task's limits.
    pub background: bool,
    /// A rescue turn's stopped tool (bare name): refused here.
    pub banned: Option<String>,
}

#[derive(Debug, Default)]
struct BridgeTurn {
    writes: u32,
    seen: HashSet<String>,
    /// A turn with no run (a routine): all its bridge calls, by tool and in all.
    calls: u32,
    per_tool: HashMap<String, u32>,
    last: f64,
    /// A routine's limit getting close: the note added to the call's result (`take_note`), once per limit.
    note: Option<String>,
    warned: HashSet<String>,
}

/// `note` added to a tool's result (its first text, else a text of its own).
pub fn add_note(result: &mut Dict, note: &str) {
    if let Some(Value::Array(content)) = result.get_mut("content") {
        if let Some(Value::String(text)) = content.first_mut().and_then(|c| c.get_mut("text")) {
            text.push_str("\n\n");
            text.push_str(note);
            return;
        }
        content.push(json!({"type": "text", "text": note}));
    }
}

/// Reading one given item (a mail, its attachment, a Carnet or app page): a routine reading 14 different mails is
/// working, not looping (2026-10-28, the replay's `mail_scan_busy_night`), so its limit on one tool counts the same
/// item read again, not reads of different items.
pub fn item_read(tool: &str) -> bool {
    let name = bare(tool);
    !write_tool(name) && (name.ends_with("_read") || name == "mail_attachment" || name == "read_page")
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
        if let Some(banned) = turn.and_then(|t| t.banned.as_deref()).filter(|b| *b == bare(tool)) {
            tracing::info!(agent = %agent, tool = %tool, "the tool the turn kept calling, in its rescue turn: refused");
            return Some(tool_text(
                &format!(
                    "Refused: you were stopped for calling {banned} again and again, so it isn't available in this \
turn. Answer the person now with what you already have."
                ),
                true,
            ));
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
            let key = if item_read(tool) { signature_of(tool, arguments) } else { bare(tool).to_string() };
            let same = entry.per_tool.entry(key).or_insert(0);
            *same += 1;
            let (same, calls) = (*same, entry.calls);
            // getting close (about two thirds): told in the call's result, once per limit (a routine can't be steered)
            let close = if same == soft_limit(limits.same_tool) {
                Some((format!("tool:{}", bare(tool)), format!("you have called {} {same} times", bare(tool))))
            } else if calls == soft_limit(settings.routine_tool_calls) {
                Some(("total".to_string(), format!("you have made {calls} calls")))
            } else {
                None
            };
            if let (Some((key, what)), true) = (close, settings.warn) {
                if entry.warned.insert(key) {
                    tracing::info!(agent = %agent, tool = %tool, calls = calls, same = same,
                        "brake_warned: a routine told it is going round in circles");
                    entry.note = Some(format!(
                        "{WARN_MARK} Note from the bridge: {what} in this turn, close to the limit. Change approach, or finish with what you have."
                    ));
                }
            }
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

    /// The note a routine's call gets with its result (its limits getting close), once.
    pub fn take_note(&self, agent: &str, turn: Option<&Turn>) -> Option<String> {
        let mut turns = self.turns.lock().unwrap_or_else(|e| e.into_inner());
        turns.get_mut(&turn_key(agent, turn)).and_then(|t| t.note.take())
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

/// The notice event's data (`bridge.notice`, code `loop_stopped`). `rescue`: a rescue turn follows (runs.rs
/// `loop_rescue`), so only the line's first sentence (the agent's own reply says the rest).
pub fn notice(over: &Over, lang: &str, rescue: bool) -> serde_json::Map<String, Value> {
    let mut data = serde_json::Map::new();
    data.insert("type".into(), json!(crate::runs::NOTICE));
    data.insert("code".into(), json!("loop_stopped"));
    data.insert("reason".into(), json!(reason(over)));
    let text = if rescue { notice_head(over, lang) } else { notice_text(over, lang) };
    data.insert("text".into(), json!(text));
    if rescue {
        data.insert("rescue".into(), json!(true));
    }
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

    /// Real-day bench 2026-10-28: « ask Coach » with no message_agent in view, 9 different tool_search and 2
    /// tool_describe in 3 minutes, nothing held it.
    #[test]
    fn searching_its_tools_again_and_again_is_braked() {
        let limits = Settings::default().conversation;
        // the same query (other options, other spacing): told at 3, stopped at 5
        let mut turn = TurnCount::default();
        let same = |n: u32| canonical(&json!({"query": "message  Agent", "limit": n}));
        for n in 1..=4u32 {
            assert_eq!(turn.note("tool_search", Some(&same(n)), false, &limits), None, "{n}");
            let warning = turn.take_warning();
            assert_eq!(warning.is_some(), n == 3, "{n}: {warning:?}");
            if let Some(w) = warning {
                assert_eq!(w, Warning::DiscoverySame(3, 5));
                assert!(w.text().contains("say so to the person"));
            }
        }
        assert_eq!(turn.note("tool_search", Some(&same(5)), false, &limits), Some(Over::Discovery));
        // different queries: past 8 discovery calls in all, tool_describe included
        let mut turn = TurnCount::default();
        let mut stopped = None;
        for n in 0..11u32 {
            let (tool, args) = if n % 5 == 4 { ("tool_describe", json!({"name": format!("t{n}")})) } else {
                ("tool_search", json!({"query": format!("agent {n}")}))
            };
            if let Some(over) = turn.note(tool, Some(&canonical(&args)), false, &limits) {
                stopped = Some((n, over));
                break;
            }
        }
        assert_eq!(stopped, Some((8, Over::Discovery)));
        assert_eq!(stopped_tool(&Over::Discovery).as_deref(), Some("tool_search"));
        assert_eq!(
            notice_text(&Over::Discovery, "fr"),
            "Je me suis arrêté : je cherchais sans fin un outil pour le faire. Dis-moi si tu veux que je réessaie."
        );
        assert!(rescue_input(&Over::Discovery, "fr").contains("Don't search your tools again"));
        // a normal turn: two searches, then the tool found is called
        let mut turn = TurnCount::default();
        for (tool, args) in [
            ("tool_search", json!({"query": "mail"})),
            ("tool_describe", json!({"name": "mcp__bipagents__mail_read"})),
            ("tool_call", json!({"name": "mcp__bipagents__mail_read", "arguments": {"id": 1}})),
            ("tool_call", json!({"name": "mcp__bipagents__mail_read", "arguments": {"id": 2}})),
        ] {
            assert_eq!(turn.note(tool, Some(&canonical(&args)), false, &limits), None);
            assert_eq!(turn.take_warning(), None);
        }
        assert_eq!(discovery_query("{\"query\": \"Mail  READ\", \"limit\": 3}"), "mail read");
        assert_eq!(discovery_query("44136fa355b3"), "44136fa355b3");
    }

    #[test]
    fn work_tools_and_the_browser_only_count_for_the_total() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        for i in 0..29 {
            assert_eq!(turn.note("terminal", Some("cargo test"), false, &limits), None, "{i}");
            assert_eq!(turn.note("mcp__stealth__click_element", Some("{\"x\":1}"), true, &limits), None, "{i}");
        }
        assert_eq!(turn.total, 58);
        // the same web search: a lookup, held to same_lookup (5)
        let mut turn = TurnCount::default();
        for _ in 0..4 {
            assert_eq!(turn.note("web_search", Some("tour Eiffel"), false, &limits), None);
        }
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
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false, banned: None });
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
        let turn = Some(Turn { run_id: "run_1d523504".into(), background: false, banned: None });
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
        let run = |id: &str| Some(Turn { run_id: id.to_string(), background: false, banned: None });
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
        for i in 0..50 {
            let tool = format!("mcp__bipagents__tool_{}", i % 8);
            if call("coach", None, &tool, &Dict::new(), 5000.0 + i as f64).is_some() {
                at = Some(i);
                break;
            }
        }
        assert_eq!(at, Some(40));
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

    // -- 2026-10-28 09:24, Alex's home server: Vie diagnosing the failing « scan-emails-fond-nuit » job was stopped
    // after 5 calls, « same call repeated (session_search) » --

    const SESSIONS: &str = "session_search";

    #[test]
    fn the_mornings_diagnosis_goes_on_now() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let query = "scan-emails-fond-nuit";
        let calls = [
            (SESSIONS, query),
            ("skill_view", "cronjob"),
            (SESSIONS, query),
            ("read_file", "~/.hermes/cron/jobs.json"),
            (SESSIONS, query),
        ];
        for (i, (tool, preview)) in calls.iter().enumerate() {
            assert_eq!(turn.note(tool, Some(preview), false, &limits), None, "call {i}");
        }
        assert_eq!(turn.total, 5);
        // the same lookup a 4th and a 5th time: stopped at the 5th
        assert_eq!(turn.note(SESSIONS, Some(query), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some(query), false, &limits), Some(Over::SameCall(SESSIONS.into())));
    }

    #[test]
    fn five_identical_lookups_stop_and_are_said_in_plain_words() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note(SESSIONS, Some("nuit"), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((4, Over::SameCall(SESSIONS.into()))));
        let over = Over::SameCall(SESSIONS.into());
        assert_eq!(
            notice_text(&over, "fr"),
            "Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions. Dis-moi si tu veux que je \
réessaie autrement."
        );
        assert_eq!(notice_head(&over, "fr"), "Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions.");
        assert!(notice_text(&over, "en").contains("the same search in our past conversations"));
        assert!(notice_text(&over, "de").contains("früheren Unterhaltungen"));
        assert!(notice_text(&over, "es-ES").contains("conversaciones anteriores"));
        for lang in ["fr", "en", "de", "es"] {
            assert!(!notice_text(&over, lang).contains("session_search"), "never a tool's name: {lang}");
        }
        assert!(notice_text(&Over::SameCall("mcp__bipagents__notebook_search".into()), "fr").contains("dans le Carnet"));
        assert!(notice_text(&Over::SameCall("web_search".into()), "fr").contains("la même recherche sur le web"));
        // writes and actions keep same_call (3)
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((2, Over::SameCall("calendar_add".into()))));
        // Hermes' memory: a read is a lookup, an addition is not
        assert!(lookup_tool("memory", Some("{\"action\": \"read\"}")) && !lookup_tool("memory", Some("add: le code du portail")));
        assert!(lookup_tool("mcp_bipagents_calendar_list", None) && !lookup_tool("mcp_bipagents_calendar_add", None));
    }

    #[test]
    fn a_retry_after_an_error_is_not_the_same_call() {
        let limits = Settings::default().conversation;
        let mut turn = TurnCount::default();
        let failed = |turn: &mut TurnCount| turn.completed("mcp_bipagents_calendar_add", true);
        let call = "{\"title\":\"Dentiste\"}";
        // the same write five times, each right after the identical one failed: retries
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        for _ in 0..4 {
            failed(&mut turn);
            assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        }
        // it went through, then the same again: counted (2nd, then 3rd: stopped)
        turn.completed("mcp_bipagents_calendar_add", false);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some(call), false, &limits), Some(Over::SameCall("calendar_add".into())));
        // an error, then another call in between: the identical call after is counted again
        let mut turn = TurnCount::default();
        for _ in 0..4 {
            assert_eq!(turn.note(SESSIONS, Some("nuit"), false, &limits), None);
            turn.completed(SESSIONS, false);
        }
        turn.completed(SESSIONS, true);
        assert_eq!(turn.note("skill_view", Some("cronjob"), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some("nuit"), false, &limits), Some(Over::SameCall(SESSIONS.into())));
        // what tells an error
        let data = |v: Value| v.as_object().cloned().unwrap();
        assert!(completed_with_error(&data(json!({"tool": "x", "error": true}))));
        assert!(completed_with_error(&data(json!({"tool": "x", "preview": "{\"success\": false, \"error\": \"db locked\"}"}))));
        assert!(completed_with_error(&data(json!({"tool": "x", "preview": "Error: timeout"}))));
        assert!(!completed_with_error(&data(json!({"tool": "x", "error": false, "preview": "3 sessions found"}))));
    }

    #[test]
    fn the_rescue_turn_is_short_and_may_not_call_the_tool_again() {
        let settings = Settings::default();
        let limits = settings.rescue_limits();
        let over = Over::SameCall(SESSIONS.into());
        let mut turn = TurnCount { banned: stopped_tool(&over), ..TurnCount::default() };
        // once (Hermes' own tool: it runs; the bridge's: refused at the MCP), twice: stopped
        assert_eq!(turn.note(SESSIONS, Some("autre"), false, &limits), None);
        assert_eq!(turn.note(SESSIONS, Some("encore"), false, &limits), Some(Over::SameCall(SESSIONS.into())));
        // at most 5 calls
        let mut turn = TurnCount::default();
        let stopped = (0..10).find_map(|i| turn.note("read_file", Some(&format!("f{i}")), false, &limits).map(|o| (i, o)));
        assert_eq!(stopped, Some((5, Over::TooMany)));
        // its input: the mark, the person's line, then the agent's instruction (out of sight)
        let input = rescue_input(&over, "fr");
        assert!(input.starts_with(
            "[bipagents:loop_rescue] Je me suis arrêté : je relançais la même recherche dans nos anciennes discussions.\n\n"
        ));
        assert!(input.contains("Don't call session_search again in this turn."));
        assert!(input.contains("answer the person in their language"));
        assert!(rescue_input(&Over::TooMany, "en").contains("Don't start new work"));
        // the bridge's own tool, in the rescue turn: refused, in plain words the model can act on
        let turns = Turns::default();
        let rescue = Turn { run_id: "run_r".into(), background: false, banned: Some("notebook_search".into()) };
        let refused = turns
            .check(&settings, "vie", Some(&rescue), "mcp__bipagents__notebook_search", &args(&[("words", "nuit")]), 0.0)
            .unwrap();
        assert_eq!(refused["isError"], json!(true));
        assert!(refused["content"][0]["text"].as_str().unwrap().contains("Answer the person now"));
        assert!(turns.check(&settings, "vie", Some(&rescue), "mcp__bipagents__calendar_list", &Dict::new(), 0.0).is_none());
        // the notice when a rescue follows: its first sentence only
        let data = notice(&over, "fr", true);
        assert_eq!(data["text"], json!(notice_head(&over, "fr")));
        assert_eq!(data["rescue"], json!(true));
        assert!(!notice(&over, "fr", false).contains_key("rescue"));
    }

    #[test]
    fn the_agent_is_told_before_the_stop_and_a_turn_that_changes_approach_goes_on() {
        let limits = Settings::default().conversation;
        assert_eq!((soft_limit(5), soft_limit(3), soft_limit(60), soft_limit(10)), (3, 2, 40, 7));
        let mut turn = TurnCount::default();
        // the same lookup: told at the 3rd (stopped at the 5th), once
        for i in 0..2 {
            assert_eq!(turn.note(SESSIONS, Some(NIGHT_Q), false, &limits), None);
            assert_eq!(turn.take_warning(), None, "{i}");
        }
        assert_eq!(turn.note(SESSIONS, Some(NIGHT_Q), false, &limits), None);
        let warning = turn.take_warning().unwrap();
        assert_eq!(warning, Warning::SameCall(SESSIONS.into(), 3, 5));
        assert!(warning.text().starts_with("[bipagents:brake] A note from the bridge, not from your person: you have made \
exactly the same session_search call 3 times in this turn; at 5 the turn is stopped. Change approach"));
        // it changes approach: other searches, other tools; never stopped, told once more when the total gets close
        let mut told = Vec::new();
        for i in 0..50 {
            let tool = ["web_search", "read_file", "search_files", "terminal"][i % 4];
            assert_eq!(turn.note(tool, Some(&format!("autre piste {i}")), false, &limits), None, "{i}");
            told.extend(turn.take_warning());
        }
        assert_eq!(told, vec![Warning::TooMany(40, 60)]);
        assert_eq!(turn.total, 53);
        // a write: told at the 2nd identical one (stopped at the 3rd)
        let mut turn = TurnCount::default();
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits), None);
        assert_eq!(turn.take_warning(), None);
        assert_eq!(turn.note("mcp_bipagents_calendar_add", Some("{\"t\":1}"), false, &limits), None);
        assert_eq!(turn.take_warning(), Some(Warning::SameCall("calendar_add".into(), 2, 3)));
        // one tool with different arguments: at the 7th of 10
        let mut turn = TurnCount::default();
        let told: Vec<_> = (0..7)
            .filter_map(|i| {
                turn.note("mcp_bipagents_reminders_add", Some(&format!("{i}")), false, &limits);
                turn.take_warning()
            })
            .collect();
        assert_eq!(told, vec![Warning::SameTool("reminders_add".into(), 7, 10)]);
    }

    const NIGHT_Q: &str = "scan-emails-fond-nuit";

    #[test]
    fn a_routine_reading_many_mails_goes_on_and_is_told_when_close() {
        // the replay's mail_scan_busy_night: 3 searches, 14 different mails read, 2 notes, in a routine
        let turns = Turns::default();
        let settings = Settings::default();
        let mut refused = Vec::new();
        let mut notes = Vec::new();
        let mut call = |tool: &str, a: Dict, i: usize| {
            match turns.check(&settings, "vie", None, tool, &a, i as f64) {
                Some(r) => refused.push((i, r)),
                None => turns.record("vie", None, tool, &a),
            }
            notes.extend(turns.take_note("vie", None).map(|n| (i, n)));
        };
        let mut i = 0;
        for q in ["facture", "banque", "école"] {
            call("mcp_bipagents_mail_search", args(&[("query", q)]), i);
            i += 1;
        }
        for m in 0..14 {
            call("mcp_bipagents_mail_read", args(&[("id", &format!("m{m}"))]), i);
            i += 1;
        }
        call("mcp_bipagents_mail_attachment", args(&[("id", "m3"), ("n", "1")]), i);
        i += 1;
        for n in 0..2 {
            call("mcp_bipagents_notebook_note", args(&[("text", &format!("n{n}"))]), i);
            i += 1;
        }
        assert!(refused.is_empty(), "{refused:?}");
        assert!(notes.is_empty(), "20 calls: under the total's two thirds (27 of 40): {notes:?}");
        // the same mail read again and again: told at the 7th, refused at the 11th
        let turns = Turns::default();
        let mut told = None;
        let refused_at = (0..20).find(|i| {
            let a = args(&[("id", "m1")]);
            let r = turns.check(&settings, "lea", None, "mcp_bipagents_mail_read", &a, 100.0 + *i as f64);
            if let Some(note) = turns.take_note("lea", None) {
                told.get_or_insert((*i, note));
            }
            r.is_some()
        });
        assert_eq!(refused_at, Some(10));
        let (at, note) = told.unwrap();
        assert_eq!(at, 6);
        assert!(note.starts_with("[bipagents:brake] Note from the bridge: you have called mail_read 7 times"));
        let mut result = crate::tools_knowledge::tool_text("Mail 1", false);
        add_note(&mut result, &note);
        assert!(result["content"][0]["text"].as_str().unwrap().starts_with("Mail 1\n\n[bipagents:brake]"));
        assert!(item_read("mcp_bipagents_mail_read") && item_read("mail_attachment") && !item_read("notebook_search"));
    }

    #[test]
    fn the_new_settings_from_the_config() {
        let s = Settings::from_table(&json!({"same_lookup": 7, "task_same_lookup": 12, "rescue": false,
                                             "rescue_tool_calls": 3, "rescue_seconds": 5}));
        assert_eq!((s.conversation.same_lookup, s.task.same_lookup), (7, 12));
        assert_eq!(s.conversation.same_call, 3, "writes and actions unchanged");
        assert!(!s.rescue);
        assert_eq!(s.rescue_limits().tool_calls, 3);
        assert_eq!(s.rescue_seconds, 20.0, "clamped");
        let d = Settings::default();
        assert_eq!((d.conversation.same_lookup, d.conversation.same_call, d.rescue, d.rescue_tool_calls), (5, 3, true, 5));
    }
}
