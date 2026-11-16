//! What the agent does now, for the apps (« ce que fait l'agent », Alex 16/11).
//!
//! « Parfois on attend 5-6 min et on sait pas trop ce que l'agent essaye de faire. » The run hub (runs.rs) sees every
//! call, every model call's end, every question to the person; this module turns them into one `turn.status` event per
//! change of what the agent does, and one more when a wait grows abnormal (`slow`), so the apps' working line can say
//! « Sur sncf-connect.com : la page met du temps · 25 s » instead of a film.
//!
//! ```json
//! {"event": "turn.status", "run_id": "…", "v": 1, "state": "tool", "slow": true, "stuck": false,
//!  "since": 1794818000.0, "started": 1794817860.0, "now": 1794818025.0,
//!  "kind": "browser", "action": "fill", "site": "sncf-connect.com", "limit_s": 120,
//!  "step": {"index": 3, "total": 4, "text": "Choisir la place"},
//!  "text": "Sur sncf-connect.com : la page met du temps"}
//! ```
//!
//! - `state`: `thinking` (nothing yet), `model` (the model has the hand: after a call, between calls), `tool`,
//!   `waiting_agent` (another agent's answer, `message_agent`), `waiting_person` (an approval, a question),
//!   `unreachable` (the bridge lost Hermes; it keeps trying). Later: `looping` (the brake's two thirds), `retrying` and
//!   `compacting` (with a Hermes patch, D6 B: not now).
//! - `since`: when this state began; `started`: the turn's start; `now`: the bridge's clock (the apps count from it,
//!   never from their own clock). `slow`: the wait passed its threshold (`[status]`); `stuck`: the model's, far past it.
//! - Never a call's arguments: a kind of action, the bare domain of a browsing call (`live_activities::site`), the
//!   agent's own step from its list (already shown in « Étapes »), numbers. `text` is the bridge's words in the agent's
//!   language (fr, en, de, es), for a state an app doesn't know; the apps word the ones they know.
//!
//! Sent only to an app that asks (`GET /v1/runs/{id}/events?…&status=1`): an iPhone app from before it takes any unknown
//! event for « not words » and drops its working line during a pause in the reply (ConversationModel.watchWordsPause).
//! Off by default (`[status] enabled = false`) until both apps read it.
//!
//! No Python counterpart (owner group: push-runs).

use serde_json::{json, Map, Value};

/// The run stream's event.
pub const EVENT: &str = "turn.status";
/// The event's shape.
pub const VERSION: u32 = 1;
/// How often the hub looks at its runs' waits (`RunHub::status_tick`).
pub const TICK_SECONDS: f64 = 2.0;
/// The browser's own limit on one call (browsers.rs: the MCP's timeout), said once its call is past half of it.
pub const BROWSER_LIMIT_SECONDS: f64 = 120.0;
/// An MCP call's limit (Hermes' default), for another agent's answer.
pub const MCP_LIMIT_SECONDS: f64 = 300.0;

/// Whether an event of the run's stream goes to a listener: a status only to one that asked for them (`status=1`).
pub fn crosses(kind: &str, asked: bool) -> bool {
    asked || kind != EVENT
}

/// `[status]` in bridge.toml (every value optional). The thresholds come from the token lab's measures
/// (the token lab, 16/11): a model call's p90 is 5 s (max 14.5 s), a heavy page's p90 14 s.
///
///     enabled = false     model_slow = 20     model_stuck = 90     page_slow = 15     tool_slow = 20
///     agent_slow = 15     local_factor = 3
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub model_slow: f64,
    pub model_stuck: f64,
    pub page_slow: f64,
    pub tool_slow: f64,
    pub agent_slow: f64,
    /// A model on the person's own machine is slower (141 s to a first word seen): its thresholds × this.
    pub local_factor: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: false,
            model_slow: 20.0,
            model_stuck: 90.0,
            page_slow: 15.0,
            tool_slow: 20.0,
            agent_slow: 15.0,
            local_factor: 3.0,
        }
    }
}

impl Settings {
    pub fn from_table(table: &Value) -> Settings {
        let d = Settings::default();
        let n = |key: &str, default: f64| table.get(key).and_then(Value::as_f64).unwrap_or(default).clamp(1.0, 3600.0);
        Settings {
            enabled: table.get("enabled").and_then(Value::as_bool).unwrap_or(d.enabled),
            model_slow: n("model_slow", d.model_slow),
            model_stuck: n("model_stuck", d.model_stuck),
            page_slow: n("page_slow", d.page_slow),
            tool_slow: n("tool_slow", d.tool_slow),
            agent_slow: n("agent_slow", d.agent_slow),
            local_factor: table.get("local_factor").and_then(Value::as_f64).unwrap_or(d.local_factor).clamp(1.0, 20.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Thinking,
    Model,
    Tool,
    WaitingAgent,
    WaitingPerson,
    Unreachable,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Thinking => "thinking",
            State::Model => "model",
            State::Tool => "tool",
            State::WaitingAgent => "waiting_agent",
            State::WaitingPerson => "waiting_person",
            State::Unreachable => "unreachable",
        }
    }
}

/// A call running.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    call_id: Option<String>,
    tool: String,
    kind: &'static str,
    site: Option<String>,
    service: Option<String>,
    since: f64,
}

/// A browsing call's action, by its tool's name (never its arguments).
fn action_of(tool: &str) -> Option<&'static str> {
    let name = tool.rsplit(['_', '.']).next().unwrap_or(tool).to_ascii_lowercase();
    let tool = tool.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| tool.contains(w));
    if has(&["navigate", "open_url", "goto", "spawn_browser", "new_tab"]) {
        Some("open")
    } else if has(&["click", "press", "tap"]) {
        Some("click")
    } else if has(&["type", "fill", "select_option", "input", "upload"]) {
        Some("fill")
    } else if has(&["page_content", "get_text", "read", "extract", "snapshot", "find_element", "query"]) {
        Some("read")
    } else if has(&["screenshot", "vision"]) {
        Some("look")
    } else if has(&["scroll"]) {
        Some("scroll")
    } else if name.is_empty() {
        None
    } else {
        Some("browse")
    }
}

/// The agent's bookkeeping (its list, its memory, the tool search…): no change of what it does for the person.
fn bookkeeping(kind: &str) -> bool {
    crate::tool_kinds::BOOKKEEPING.contains(&kind)
}

/// One run's status, as its events come (the hub's run state keeps it: `SubscriptionState::status`).
#[derive(Debug, Clone)]
pub struct Tracker {
    lang: String,
    local: bool,
    started: f64,
    state: State,
    since: f64,
    /// The last sign of life in this state (words, reasoning): a model writing is not a slow model.
    progress: f64,
    open: Vec<Open>,
    waiting_person: u32,
    unreachable: bool,
    slow: bool,
    stuck: bool,
    /// The agent's own list (or the automatic one): the step in progress, its rank and the list's length.
    step: Option<(u64, u64, Option<String>)>,
    /// The site the last finished browsing call was on: a call naming no page stays there.
    last_done_site: Option<String>,
    /// The key of the last status sent: nothing twice.
    sent: Option<String>,
    ended: bool,
}

impl Tracker {
    /// `lang`: the agent's language; `local`: a model on the person's own machine.
    pub fn new(started: f64, lang: &str, local: bool) -> Tracker {
        Tracker {
            lang: lang.to_string(),
            local,
            started,
            state: State::Thinking,
            since: started,
            progress: started,
            open: Vec::new(),
            waiting_person: 0,
            unreachable: false,
            slow: false,
            stuck: false,
            step: None,
            last_done_site: None,
            sent: None,
            ended: false,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// The run is over (its end came, before any turn that takes over): nothing more to say for it.
    pub fn end(&mut self) {
        self.ended = true;
    }

    /// An event of the run (`kind`, its data) at `now`: the status to send when what the agent does changed.
    pub fn on_event(&mut self, kind: &str, data: &Map<String, Value>, now: f64, settings: &Settings) -> Option<Map<String, Value>> {
        if self.ended {
            return None;
        }
        match kind {
            "tool.started" | "tool.start" => {
                let tool = crate::loop_brake::event_tool(data);
                let kind = crate::threads::layout::kind_of(&tool);
                if bookkeeping(kind) || crate::steps::is_todo_tool(&tool) {
                    return None;
                }
                let preview = data.get("preview").and_then(Value::as_str).unwrap_or("");
                let site = if kind == "browser" || kind == "webRead" { crate::live_activities::site(preview) } else { None };
                let service = data.get("service").and_then(Value::as_str).map(str::to_string);
                let call_id = data.get("tool_call_id").and_then(Value::as_str).map(str::to_string);
                // a browsing call naming no page stays on the turn's last site
                let site = site.or_else(|| (kind == "browser").then(|| self.last_site()).flatten());
                self.open.push(Open { call_id, tool, kind, site, service, since: now });
            }
            "tool.completed" | "tool.complete" | "tool.failed" => {
                let tool = crate::loop_brake::event_tool(data);
                let call_id = data.get("tool_call_id").and_then(Value::as_str);
                let at = self
                    .open
                    .iter()
                    .position(|o| call_id.is_some() && o.call_id.as_deref() == call_id)
                    .or_else(|| self.open.iter().position(|o| o.tool == tool))?;
                let done = self.open.remove(at);
                self.last_done_site = done.site.or(self.last_done_site.take());
            }
            crate::runs::MODEL_USAGE => {
                // a model call ended: the next one starts (or the reply): the model's turn again, a fresh wait
                if self.open.is_empty() && self.waiting_person == 0 {
                    self.since = now;
                    self.progress = now;
                }
            }
            "approval.request" | "approval.required" | "clarify.request" | "clarify.required" => self.waiting_person += 1,
            "approval.responded" | "approval.resolved" | "clarify.responded" | "clarify.cancelled"
            | crate::hermes_prompts::APPROVAL_SETTLED | crate::hermes_prompts::CLARIFY_SETTLED => {
                self.waiting_person = self.waiting_person.saturating_sub(1);
            }
            crate::steps::EVENT => {
                self.step = data.get("steps").and_then(step_of);
            }
            "bridge.error" => {
                if data.get("code").and_then(Value::as_str) == Some("hermes_unreachable") {
                    self.unreachable = true;
                }
            }
            _ if crate::hermes::is_terminal(kind) => {
                self.ended = true;
                return None;
            }
            _ if crate::hermes::is_delta(kind) || kind.starts_with("reasoning") || kind.starts_with("thinking") => {
                self.progress = now;
                if self.unreachable {
                    self.unreachable = false;
                } else if !self.slow {
                    return None;
                }
            }
            _ => return None,
        }
        if kind != "bridge.error" && !crate::hermes::is_delta(kind) {
            self.unreachable = false;
        }
        self.settle(now);
        self.changed(now, settings)
    }

    /// A tick of the hub's clock: the status to send when the current wait just grew abnormal.
    pub fn tick(&mut self, now: f64, settings: &Settings) -> Option<Map<String, Value>> {
        if self.ended {
            return None;
        }
        self.changed(now, settings)
    }

    /// What the agent does after this event: the newest call running, a question, the model.
    fn settle(&mut self, now: f64) {
        let state = if self.unreachable {
            State::Unreachable
        } else if self.waiting_person > 0 {
            State::WaitingPerson
        } else if let Some(open) = self.open.last() {
            if open.kind == "messageAgent" { State::WaitingAgent } else { State::Tool }
        } else {
            State::Model
        };
        let since = match state {
            State::Tool | State::WaitingAgent => self.open.last().map(|o| o.since).unwrap_or(now),
            _ if state == self.state => self.since,
            _ => now,
        };
        if state != self.state || since != self.since {
            self.progress = since;
        }
        self.state = state;
        self.since = since;
    }

    /// The wait's thresholds in this state (None: no wait to judge: the person's, a lost link).
    fn thresholds(&self, settings: &Settings) -> Option<(f64, Option<f64>)> {
        let model = if self.local { settings.local_factor } else { 1.0 };
        match self.state {
            State::Thinking | State::Model => Some((settings.model_slow * model, Some(settings.model_stuck * model))),
            State::Tool if self.open.last().is_some_and(|o| o.kind == "browser") => Some((settings.page_slow, None)),
            State::Tool => Some((settings.tool_slow, None)),
            State::WaitingAgent => Some((settings.agent_slow, None)),
            State::WaitingPerson | State::Unreachable => None,
        }
    }

    fn changed(&mut self, now: f64, settings: &Settings) -> Option<Map<String, Value>> {
        // the wait since the state began or since the model last wrote something
        let waited = now - self.progress.max(self.since);
        let (slow, stuck) = self.thresholds(settings).unwrap_or((f64::INFINITY, None));
        self.slow = waited >= slow;
        self.stuck = stuck.is_some_and(|s| waited >= s);
        let status = self.status(now);
        let key = key_of(&status);
        if self.sent.as_deref() == Some(key.as_str()) {
            return None;
        }
        self.sent = Some(key);
        Some(status)
    }

    fn last_site(&self) -> Option<String> {
        self.open.iter().rev().find_map(|o| o.site.clone()).or_else(|| self.last_done_site.clone())
    }

    /// The status now (without `run_id`, which the hub adds).
    pub fn status(&self, now: f64) -> Map<String, Value> {
        let open = self.open.last().filter(|_| matches!(self.state, State::Tool | State::WaitingAgent));
        let kind = open.map(|o| o.kind);
        let action = open.filter(|o| o.kind == "browser").and_then(|o| action_of(&o.tool));
        let site = open.and_then(|o| o.site.clone());
        let limit = match (self.state, kind) {
            (State::Tool, Some("browser")) => Some(BROWSER_LIMIT_SECONDS),
            (State::WaitingAgent, _) => Some(MCP_LIMIT_SECONDS),
            _ => None,
        };
        let mut status = Map::new();
        status.insert("event".into(), json!(EVENT));
        status.insert("v".into(), json!(VERSION));
        status.insert("state".into(), json!(self.state.as_str()));
        status.insert("slow".into(), json!(self.slow));
        status.insert("stuck".into(), json!(self.stuck));
        status.insert("since".into(), json!(self.since));
        status.insert("started".into(), json!(self.started));
        status.insert("now".into(), json!(now));
        status.insert("kind".into(), json!(kind));
        status.insert("action".into(), json!(action));
        status.insert("site".into(), json!(site));
        status.insert("service".into(), json!(open.and_then(|o| o.service.clone())));
        status.insert("limit_s".into(), json!(limit));
        status.insert(
            "step".into(),
            self.step.as_ref().map_or(Value::Null, |(i, n, t)| json!({"index": i, "total": n, "text": t})),
        );
        status.insert("text".into(), json!(self.words(open, action)));
        status
    }

    /// The bridge's words for the status (the apps word the states they know themselves).
    fn words(&self, open: Option<&Open>, action: Option<&str>) -> String {
        let l = crate::steps::lang_index(&self.lang);
        match self.state {
            State::Thinking | State::Model if self.stuck => MODEL_STUCK[l].into(),
            State::Thinking | State::Model if self.slow => MODEL_SLOW[l].into(),
            State::Thinking | State::Model => THINKING[l].into(),
            State::WaitingAgent => WAITING_AGENT[l].into(),
            State::WaitingPerson => WAITING_PERSON[l].into(),
            State::Unreachable => UNREACHABLE[l].into(),
            State::Tool => {
                let Some(open) = open else { return THINKING[l].into() };
                if open.kind == "browser" {
                    let what = if self.slow { PAGE_SLOW[l] } else { action_words(action, l) };
                    return match &open.site {
                        Some(site) => ON_SITE[l].replace("{s}", site).replace("{a}", what),
                        None => IN_BROWSER[l].replace("{a}", what),
                    };
                }
                let words = crate::steps::call_words(&open.tool, open.site.as_deref(), open.service.as_deref(), &self.lang)
                    .unwrap_or_else(|| WORKING[l].to_string());
                if self.slow { TOOL_SLOW[l].replace("{w}", &words) } else { words }
            }
        }
    }
}

/// What makes two statuses the same for the person (not the clock).
fn key_of(status: &Map<String, Value>) -> String {
    ["state", "slow", "stuck", "since", "kind", "action", "site", "service", "step", "text"]
        .iter()
        .map(|k| status.get(*k).map(Value::to_string).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("|")
}

/// The step in progress of a `steps` object (steps.rs): its rank among the steps (cancelled ones left out), the list's
/// length, its words.
fn step_of(steps: &Value) -> Option<(u64, u64, Option<String>)> {
    let items = steps.get("items")?.as_array()?;
    let live: Vec<&Value> = items.iter().filter(|i| i.get("status").and_then(Value::as_str) != Some("cancelled")).collect();
    if live.is_empty() {
        return None;
    }
    let at = live.iter().position(|i| i.get("status").and_then(Value::as_str) == Some("in_progress"));
    let done = live.iter().filter(|i| i.get("status").and_then(Value::as_str) == Some("done")).count();
    let (index, text) = match at {
        Some(i) => (i + 1, live[i].get("text").and_then(Value::as_str).map(str::to_string)),
        None => ((done + 1).min(live.len()), None),
    };
    Some((index as u64, live.len() as u64, text))
}

fn action_words(action: Option<&str>, l: usize) -> &'static str {
    match action {
        Some("open") => OPEN[l],
        Some("click") => CLICK[l],
        Some("fill") => FILL[l],
        Some("read") => READ[l],
        Some("look") => LOOK[l],
        Some("scroll") => SCROLL[l],
        _ => BROWSE[l],
    }
}

// fr, en, de, es
const THINKING: [&str; 4] = ["Je réfléchis", "Thinking", "Ich denke nach", "Estoy pensando"];
const MODEL_SLOW: [&str; 4] = [
    "J'attends la réponse du modèle (lent)",
    "Waiting for the model (slow)",
    "Ich warte auf das Modell (langsam)",
    "Espero la respuesta del modelo (lento)",
];
const MODEL_STUCK: [&str; 4] =
    ["Le modèle ne répond pas", "The model isn't answering", "Das Modell antwortet nicht", "El modelo no responde"];
const WAITING_AGENT: [&str; 4] = [
    "J'attends la réponse d'un autre agent",
    "Waiting for another agent's answer",
    "Ich warte auf die Antwort eines anderen Agenten",
    "Espero la respuesta de otro agente",
];
const WAITING_PERSON: [&str; 4] = ["J'attends ta réponse", "Waiting for your answer", "Ich warte auf deine Antwort", "Espero tu respuesta"];
const UNREACHABLE: [&str; 4] = [
    "Je n'arrive plus à joindre l'agent, je réessaie",
    "Can't reach the agent, trying again",
    "Ich erreiche den Agenten nicht, neuer Versuch",
    "No consigo contactar con el agente, lo intento de nuevo",
];
const WORKING: [&str; 4] = ["Je travaille", "Working", "Ich arbeite", "Estoy trabajando"];
const TOOL_SLOW: [&str; 4] = ["{w} : ça prend du temps", "{w}: taking a while", "{w}: das dauert", "{w}: tarda un poco"];
const ON_SITE: [&str; 4] = ["Sur {s} : {a}", "On {s}: {a}", "Auf {s}: {a}", "En {s}: {a}"];
const IN_BROWSER: [&str; 4] = ["Dans le navigateur : {a}", "In the browser: {a}", "Im Browser: {a}", "En el navegador: {a}"];
const PAGE_SLOW: [&str; 4] = ["la page met du temps", "the page is slow", "die Seite lädt langsam", "la página tarda"];
const OPEN: [&str; 4] = ["j'ouvre une page", "opening a page", "ich öffne eine Seite", "abro una página"];
const CLICK: [&str; 4] = ["je clique", "clicking", "ich klicke", "hago clic"];
const FILL: [&str; 4] = ["je remplis un formulaire", "filling in a form", "ich fülle ein Formular aus", "relleno un formulario"];
const READ: [&str; 4] = ["je lis la page", "reading the page", "ich lese die Seite", "leo la página"];
const LOOK: [&str; 4] = ["je regarde la page", "looking at the page", "ich sehe mir die Seite an", "miro la página"];
const SCROLL: [&str; 4] = ["je fais défiler la page", "scrolling the page", "ich scrolle die Seite", "desplazo la página"];
const BROWSE: [&str; 4] = ["je navigue", "browsing", "ich surfe", "navego"];

#[cfg(test)]
mod tests {
    use super::*;

    fn data(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn on() -> Settings {
        Settings { enabled: true, ..Settings::default() }
    }

    fn started(t: &mut Tracker, tool: &str, id: &str, preview: &str, now: f64) -> Option<Map<String, Value>> {
        t.on_event("tool.started", &data(json!({"tool": tool, "tool_call_id": id, "preview": preview})), now, &on())
    }

    fn completed(t: &mut Tracker, tool: &str, id: &str, now: f64) -> Option<Map<String, Value>> {
        t.on_event("tool.completed", &data(json!({"tool": tool, "tool_call_id": id})), now, &on())
    }

    fn reduced_text(status: &Map<String, Value>) -> String {
        reduced(status).get("text").and_then(Value::as_str).unwrap_or_default().to_string()
    }

    fn s<'a>(status: &'a Map<String, Value>, key: &str) -> &'a Value {
        status.get(key).unwrap_or(&Value::Null)
    }

    #[test]
    fn a_page_waited_on_is_said_with_its_site_then_slow() {
        let mut t = Tracker::new(100.0, "fr", false);
        let status = started(&mut t, "mcp__stealth__navigate", "a", "https://www.sncf-connect.com/home/search?from=Paris", 101.0)
            .expect("a status as the call starts");
        assert_eq!(s(&status, "state"), "tool");
        assert_eq!(s(&status, "kind"), "browser");
        assert_eq!(s(&status, "action"), "open");
        assert_eq!(s(&status, "site"), "sncf-connect.com");
        assert_eq!(s(&status, "since"), 101.0);
        assert_eq!(s(&status, "started"), 100.0);
        assert_eq!(s(&status, "limit_s"), BROWSER_LIMIT_SECONDS);
        assert_eq!(s(&status, "slow"), false);
        assert_eq!(s(&status, "text"), "Sur sncf-connect.com : j'ouvre une page");
        // nothing new before the threshold, then once
        assert_eq!(t.tick(110.0, &on()), None);
        let slow = t.tick(116.0, &on()).expect("slow at 15 s");
        assert_eq!(s(&slow, "slow"), true);
        assert_eq!(s(&slow, "since"), 101.0, "the wait's start, unchanged");
        assert_eq!(s(&slow, "now"), 116.0);
        assert_eq!(s(&slow, "text"), "Sur sncf-connect.com : la page met du temps");
        assert_eq!(t.tick(130.0, &on()), None, "said once");
    }

    #[test]
    fn a_quiet_model_after_a_call_is_slow_at_20_s_and_stuck_at_90() {
        let mut t = Tracker::new(0.0, "fr", false);
        started(&mut t, "web_search", "a", "horaires dentiste", 1.0);
        let model = completed(&mut t, "web_search", "a", 2.0).expect("the model's turn again");
        assert_eq!(s(&model, "state"), "model");
        assert_eq!(s(&model, "since"), 2.0);
        assert_eq!(s(&model, "text"), "Je réfléchis");
        assert_eq!(t.tick(21.0, &on()), None);
        let slow = t.tick(22.5, &on()).expect("slow");
        assert_eq!((s(&slow, "slow"), s(&slow, "stuck")), (&json!(true), &json!(false)));
        assert_eq!(s(&slow, "text"), "J'attends la réponse du modèle (lent)");
        let stuck = t.tick(92.5, &on()).expect("stuck");
        assert_eq!(s(&stuck, "stuck"), true);
        assert_eq!(s(&stuck, "text"), "Le modèle ne répond pas");
        // its words come: no longer slow, said once
        let back = t.on_event("message.delta", &data(json!({"delta": "Voilà"})), 93.0, &on()).expect("not slow any more");
        assert_eq!(s(&back, "slow"), false);
        assert_eq!(t.on_event("message.delta", &data(json!({"delta": " le"})), 93.5, &on()), None);
    }

    #[test]
    fn a_model_on_the_persons_own_machine_has_more_time() {
        let mut t = Tracker::new(0.0, "fr", true);
        completed(&mut t, "x", "a", 1.0);
        t.on_event(crate::runs::MODEL_USAGE, &data(json!({"call": 1})), 1.0, &on());
        assert_eq!(t.tick(50.0, &on()).map(|st| st["slow"].clone()), None);
        assert_eq!(t.tick(61.5, &on()).map(|st| st["slow"].clone()), Some(json!(true)));
    }

    #[test]
    fn never_a_calls_arguments() {
        let secrets = [
            "4970 1012 3456 7890",
            "FR76 3000 6000 0112 3456 7890 189",
            "marie.dupont@example.org",
            "sk-live-0123456789abcdef",
            "12 rue des Lilas, 75011 Paris",
            "?token=abc123&session=xyz",
        ];
        for secret in secrets {
            let mut t = Tracker::new(0.0, "fr", false);
            let url = format!("https://www.example-shop.fr/checkout/payment?card={secret}");
            let mut seen = vec![];
            seen.extend(t.on_event("tool.started", &data(json!({"tool": "mcp__stealth__type_text",
                "tool_call_id": "a", "preview": format!("{url} {secret}"), "args": {"text": secret}})), 1.0, &on()));
            seen.extend(t.tick(30.0, &on()));
            seen.extend(t.on_event("tool.started", &data(json!({"tool": "mcp__bipagents__mail_send",
                "tool_call_id": "b", "preview": secret, "args": {"to": secret, "body": secret}})), 31.0, &on()));
            seen.extend(t.tick(60.0, &on()));
            assert!(seen.len() >= 3, "{secret}: {seen:?}");
            for status in &seen {
                let all = Value::Object(status.clone()).to_string();
                for word in secret.split([' ', ',', '?', '&', '=']).filter(|w| w.len() >= 4) {
                    assert!(!all.contains(word), "« {word} » of « {secret} » in {all}");
                }
                assert!(!all.contains("checkout"), "a path in {all}");
                let site = s(status, "site");
                assert!(site.is_null() || site == "example-shop.fr", "{site}");
            }
        }
    }

    #[test]
    fn another_agents_answer_and_the_persons_are_waits_of_their_own() {
        let mut t = Tracker::new(0.0, "fr", false);
        let agent = started(&mut t, "mcp__bipagents__message_agent", "a", "", 1.0).unwrap();
        assert_eq!(s(&agent, "state"), "waiting_agent");
        assert_eq!(s(&agent, "limit_s"), MCP_LIMIT_SECONDS);
        assert_eq!(s(&agent, "text"), "J'attends la réponse d'un autre agent");
        assert_eq!(s(&t.tick(16.5, &on()).unwrap(), "slow"), true);
        completed(&mut t, "mcp__bipagents__message_agent", "a", 40.0);
        let asked = t.on_event("approval.request", &data(json!({"request_id": "r"})), 41.0, &on()).unwrap();
        assert_eq!(s(&asked, "state"), "waiting_person");
        assert_eq!(s(&asked, "text"), "J'attends ta réponse");
        assert_eq!(t.tick(400.0, &on()), None, "the person's time is never « slow »");
        let answered = t.on_event("approval.responded", &data(json!({"request_id": "r"})), 402.0, &on()).unwrap();
        assert_eq!(s(&answered, "state"), "model");
        assert_eq!(s(&answered, "since"), 402.0);
    }

    #[test]
    fn the_agents_own_step_goes_with_the_status() {
        let mut t = Tracker::new(0.0, "fr", false);
        let steps = json!({"items": [{"text": "Trouver les trains", "status": "done"},
                                     {"text": "Annuler", "status": "cancelled"},
                                     {"text": "Comparer les prix", "status": "in_progress"},
                                     {"text": "Te proposer le meilleur", "status": "pending"}], "done": 1, "total": 3});
        let status = t.on_event(crate::steps::EVENT, &data(json!({"steps": steps})), 5.0, &on()).unwrap();
        assert_eq!(s(&status, "step"), &json!({"index": 2, "total": 3, "text": "Comparer les prix"}));
    }

    #[test]
    fn bookkeeping_and_the_list_change_nothing_and_nothing_is_said_twice() {
        let mut t = Tracker::new(0.0, "fr", false);
        assert!(started(&mut t, "web_search", "a", "x", 1.0).is_some());
        assert_eq!(started(&mut t, "memory", "m", "", 1.5), None);
        assert_eq!(started(&mut t, "todo", "t", "", 1.6), None);
        assert_eq!(completed(&mut t, "memory", "m", 1.7), None, "a call it never followed");
        assert_eq!(t.on_event("model.usage", &data(json!({})), 1.8, &on()), None, "a call still runs");
    }

    #[test]
    fn a_lost_link_is_said_and_the_next_event_clears_it() {
        let mut t = Tracker::new(0.0, "fr", false);
        let lost = t.on_event("bridge.error", &data(json!({"code": "hermes_unreachable"})), 3.0, &on()).unwrap();
        assert_eq!(s(&lost, "state"), "unreachable");
        assert_eq!(s(&lost, "text"), "Je n'arrive plus à joindre l'agent, je réessaie");
        let back = started(&mut t, "web_search", "a", "x", 9.0).unwrap();
        assert_eq!(s(&back, "state"), "tool");
    }

    #[test]
    fn the_end_of_the_turn_ends_its_statuses() {
        let mut t = Tracker::new(0.0, "en", false);
        assert_eq!(s(&started(&mut t, "web_search", "a", "x", 1.0).unwrap(), "text"), "Search the web");
        assert_eq!(t.on_event("run.completed", &data(json!({})), 2.0, &on()), None);
        assert_eq!(t.tick(60.0, &on()), None);
    }

    // -- the loop card, the steps' times, the summary, the family, a local model (16/11, phase 2) ----------------

    #[test]
    fn the_brakes_two_thirds_become_a_looping_status_with_its_buttons() {
        let mut t = Tracker::new(0.0, "fr", false);
        started(&mut t, "mcp__stealth__click_element", "a", "https://www.sncf-connect.com/checkout/payment", 1.0);
        let lp = Loop::of(&crate::loop_brake::Warning::Browser(27, 40));
        assert_eq!((lp.over, lp.count, lp.stop_at), ("browser", 27, 41));
        let status = t.looping(lp, 200.0, &on()).expect("said at once");
        assert_eq!(s(&status, "state"), "looping");
        assert_eq!(s(&status, "actions"), &json!(["continue", "stop", "help"]));
        assert_eq!(s(&status, "loop"), &json!({"over": "browser", "site": "sncf-connect.com", "count": 27, "stop_at": 41}));
        assert_eq!(s(&status, "text"), "Je tourne en rond sur sncf-connect.com (27 étapes)");
        // the person's question still comes first; once answered, the loop again
        let asked = t.on_event("approval.request", &data(json!({})), 201.0, &on()).unwrap();
        assert_eq!(s(&asked, "state"), "waiting_person");
        let back = t.on_event("approval.responded", &data(json!({})), 202.0, &on()).unwrap();
        assert_eq!(s(&back, "state"), "looping");
        // « Continuer »: what it does again, no buttons
        let again = t.continued(203.0, &on()).unwrap();
        assert_eq!(s(&again, "state"), "tool");
        assert_eq!(s(&again, "actions"), &json!([]));
        assert_eq!(s(&again, "loop"), &Value::Null);
    }

    #[test]
    fn each_warning_says_how_many_times_and_where_it_stops() {
        use crate::loop_brake::Warning;
        let cases = [
            (Warning::SameCall("calendar_add".into(), 2, 3), "same_call", 2, 3),
            (Warning::SameTool("mail_search".into(), 7, 10), "same_tool", 7, 11),
            (Warning::TooMany(40, 60), "too_many", 40, 61),
            (Warning::PageReads(10, 15), "page_reads", 10, 16),
            (Warning::DiscoverySame(3, 5), "discovery", 3, 5),
            (Warning::Discovery(5, 8), "discovery", 5, 9),
        ];
        for (w, over, count, stop_at) in cases {
            let lp = Loop::of(&w);
            assert_eq!((lp.over, lp.count, lp.stop_at), (over, count, stop_at), "{w:?}");
        }
        let mut t = Tracker::new(0.0, "fr", false);
        let said = t.looping(Loop::of(&Warning::SameCall("calendar_add".into(), 2, 3)), 5.0, &on()).unwrap();
        assert_eq!(s(&said, "text"), "Je tourne en rond (2 essais)");
        assert_eq!(s(&said, "loop")["kind"], "calendarChange");
    }

    #[test]
    fn each_step_gets_its_time_and_the_turn_where_it_went() {
        let mut t = Tracker::new(0.0, "fr", false);
        let list = |a: &str, b: &str| {
            json!({"items": [{"text": "Trouver les trains", "status": a}, {"text": "Comparer les prix", "status": b}],
                   "done": 0, "total": 2})
        };
        t.steps_with_times(&list("in_progress", "pending"), 0.0);
        started(&mut t, "mcp__stealth__navigate", "a", "https://www.sncf-connect.com/", 2.0);
        completed(&mut t, "mcp__stealth__navigate", "a", 32.0);
        let timed = t.steps_with_times(&list("done", "in_progress"), 50.0);
        assert_eq!(timed["items"][0]["secs"], 50);
        assert_eq!(timed["items"][1]["secs"], 0);
        let timed = t.steps_with_times(&list("done", "in_progress"), 80.0);
        assert_eq!(timed["items"][0]["secs"], 50, "a done step keeps its time");
        assert_eq!(timed["items"][1]["secs"], 30, "the step going on, so far");
        assert_eq!(timed["time"]["secs"], 80);
        let time = t.time(80.0);
        assert_eq!((time["secs"].clone(), time["tools"].clone(), time["model"].clone()), (json!(80), json!(30), json!(50)));
        assert_eq!((time["person"].clone(), time["agent"].clone(), time["actions"].clone()), (json!(0), json!(0), json!(1)));
        assert_eq!(time["top"], json!([{"what": "model", "secs": 50}, {"what": "site", "site": "sncf-connect.com", "secs": 30}]));
    }

    #[test]
    fn a_long_turn_ends_with_where_its_time_went_a_short_one_says_nothing() {
        let mut short = Tracker::new(0.0, "fr", false);
        started(&mut short, "web_search", "a", "x", 1.0);
        assert_eq!(short.on_event("run.completed", &data(json!({})), 59.0, &on()), None);
        let mut long = Tracker::new(0.0, "fr", false);
        assert!(long.on_event("clarify.request", &data(json!({})), 10.0, &on()).is_some());
        long.on_event("clarify.responded", &data(json!({})), 50.0, &on());
        let done = long.on_event("run.completed", &data(json!({})), 75.0, &on()).expect("a summary");
        assert_eq!(s(&done, "state"), "done");
        assert_eq!(s(&done, "took")["secs"], 75);
        assert_eq!(s(&done, "took")["person"], 40);
        assert_eq!(s(&done, "took")["top"][0], json!({"what": "person", "secs": 40}));
        assert_eq!(long.tick(200.0, &on()), None);
    }

    #[test]
    fn the_summary_goes_on_the_turns_last_reply() {
        // (segment, at, an agent's message): a person's message, a short word between calls, the reply, the next turn
        let items = [("s1", 100.0, false), ("s1", 130.0, true), ("s1", 170.0, true), ("s1", 300.0, false), ("s1", 310.0, true),
                     ("s2", 171.0, true)];
        let took = json!({"secs": 72});
        let rows = vec![("s1".to_string(), 100.0, 172.0, took.clone()), ("s1".to_string(), 400.0, 470.0, json!({"secs": 70}))];
        assert_eq!(place_took(&items, &rows), vec![(2, took)], "the reply, not the next turn's, nor another segment's");
    }

    #[test]
    fn a_family_member_reads_a_reduced_status() {
        let mut t = Tracker::new(0.0, "fr", false);
        let steps = json!({"items": [{"text": "Vérifier le taux de sucre de Marie", "status": "in_progress"}], "done": 0, "total": 1});
        t.on_event(crate::steps::EVENT, &data(json!({"steps": steps})), 1.0, &on());
        let full = started(&mut t, "mcp__stealth__navigate", "a", "https://www.pharmacie-exemple.fr/x", 2.0).unwrap();
        let reduced = reduced(&full);
        let raw = Value::Object(reduced.clone()).to_string();
        assert!(!raw.contains("pharmacie") && !raw.contains("sucre"), "{raw}");
        assert_eq!((&reduced["state"], &reduced["text"]), (&json!("tool"), &json!("Dans le navigateur : j'ouvre une page")));
        for key in ["actions", "step", "site", "loop", "service"] {
            assert!(reduced.get(key).is_none(), "{key} in {raw}");
        }
        let lp = t.looping(Loop::of(&crate::loop_brake::Warning::Browser(27, 40)), 5.0, &on()).unwrap();
        assert_eq!(reduced_text(&lp), "Je tourne en rond (27 étapes)");
    }

    #[test]
    fn a_model_on_this_machine_or_the_network_is_local() {
        assert!(local_base("http://127.0.0.1:8080/v1") && local_base("http://192.168.1.4:8000/v1"));
        assert!(local_base("http://spark.local/v1"));
        assert!(!local_base("https://api.eurouter.ai/api/v1") && !local_base(""));
    }

    #[test]
    fn only_an_app_that_asks_gets_the_statuses() {
        assert!(!crosses(EVENT, false));
        assert!(crosses(EVENT, true));
        assert!(crosses("tool.started", false) && crosses("steps.updated", false));
    }

    #[test]
    fn settings_from_the_config() {
        assert!(!Settings::default().enabled, "off until both apps read it");
        let s = Settings::from_table(&json!({"enabled": true, "model_slow": 30, "page_slow": 0.1, "local_factor": 50}));
        assert!(s.enabled);
        assert_eq!((s.model_slow, s.page_slow, s.local_factor, s.tool_slow), (30.0, 1.0, 20.0, 20.0));
    }
}
