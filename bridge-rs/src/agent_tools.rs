//! Tools the bridge gives the agents, over MCP (`/v1/agent-tools/mcp`): `create_audio` reads a text aloud with the
//! voice the person chose for the agent in the app (its server voice for reading — Drama 3, Kokoro… — else its Bip);
//! `create_podcast` makes a conversation between hosts, each with a voice of their own (the person's voice model when
//! it reads, else Bips); `list_voices` lists the voices the agent may give them; `create_page` and its kin keep the
//! small web pages the agent makes for the person (pages.py); `mail_search`, `mail_read`, `mail_attachment` and
//! `mail_draft` reach the mailboxes the person connected for it (mailbox.py); `propose_task` proposes a background
//! task the person starts in the app (jobs.py); `tell_family` proposes to pass something on to the family's agents, which the person sends
//! or not from the app (family_news.rs, only for an install with a linked household).
//!
//! The agent's own Hermes calls it (an MCP server entry `bipagents` in its profile, with a token of its own), not its
//! sandbox: nothing is opened to the containers, and the voice server stays on this machine's loopback. The text is cut
//! into paragraphs and short groups of sentences (a voice server skips or repeats lines in a long block), read with the
//! person's pronunciations, one piece after the other in the background queue, so a Live started meanwhile is not held
//! up. The file lands in the bridge's own folder, served to the app like any `MEDIA:` file under `/bipagents/audio`.
//!
//! Port of `bridge/bipbridge/agent_tools.py` (owner group: tools-knowledge).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::Infallible;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::post;
use axum::Router;
use indexmap::IndexMap;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agents_admin::Run;
use crate::app::AppState;
use crate::config::{AgentConfig, Config, SharedConfig};
use crate::jobs::Jobs;
use crate::push::PushService;
use crate::tools_knowledge::{self as tk, tool_text};
use crate::tts::{mono, voice_language, Resampler, SynthOptions, TtsError, TtsService, BACKGROUND, OUTPUT_RATE};
use crate::voices::{relay_for, voice_for};
use crate::web::ApiResult;
use crate::{mailbox, pages, py, reminders, shares, Dict};

/// The entry in the agents' `mcp_servers`.
pub const SERVER_NAME: &str = "bipagents";
pub const PATH: &str = "/v1/agent-tools/mcp";
/// How the app asks for the files (`GET /v1/media?path=…`).
pub const MEDIA_PREFIX: &str = "/bipagents/audio";
/// About 25 minutes of audio.
pub const MAX_CHARS: usize = 20000;
pub const CHUNK_CHARS: usize = 350;
pub const MAX_TURNS: usize = 400;
pub const MAX_SPEAKERS: usize = 4;
pub const PAUSE_TURN_MS: u32 = 350;
pub const PAUSE_SENTENCES_MS: u32 = 150;
pub const PAUSE_PARAGRAPH_MS: u32 = 500;
/// Seconds Hermes waits for a call (its default is 300: too short for a podcast).
pub const TOOL_TIMEOUT: u32 = 1800;
/// An SSE comment while it reads, so the client's read timeout never fires.
pub const KEEPALIVE_SECONDS: f64 = 10.0;
pub const AI_SOURCE: &str = "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia";

/// Python's `ValueError` here: a message for the agent (`isError`), not a protocol error.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ValueError(pub String);

fn value_error(message: impl Into<String>) -> anyhow::Error {
    ValueError(message.into()).into()
}

// --- the tools ---------------------------------------------------------------------------------------------------

pub const CREATE_AUDIO: &str = "create_audio";
pub const CREATE_PODCAST: &str = "create_podcast";
pub const LIST_VOICES: &str = "list_voices";
pub const CREATE_PAGE: &str = "create_page";
pub const LIST_PAGES: &str = "list_pages";
pub const READ_PAGE: &str = "read_page";
pub const DELETE_PAGE: &str = "delete_page";
pub const KNOWLEDGE_READ: &str = "knowledge_read";
pub const KNOWLEDGE_SEARCH: &str = "knowledge_search";
pub const PROPOSE_TASK: &str = "propose_task";
pub const KNOWLEDGE_TOOLS: [&str; 2] = [KNOWLEDGE_READ, KNOWLEDGE_SEARCH];
pub const PAGE_TOOLS: [&str; 4] = [CREATE_PAGE, LIST_PAGES, READ_PAGE, DELETE_PAGE];
/// The tools that make files in the place: refused plainly when it is full (storage_watch.rs).
pub const WRITING_TOOLS: [&str; 3] = [CREATE_AUDIO, CREATE_PODCAST, CREATE_PAGE];

fn create_audio_tool() -> Value {
    json!({
        "name": "create_audio",
        "description": "Turn a text into an audio file read aloud with your voice — the one the person chose for you in the \
    BipAgents app — for example a podcast, a story or a long message to listen to. It takes about as long as the \
    audio lasts. The result gives a MEDIA: line: put it alone on a line in your reply, and the person gets the \
    audio in the app. Write the text as it should be heard: no markdown, no lists, no links.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "What to read aloud.", "maxLength": MAX_CHARS},
                "title": {"type": "string", "description": "A short title, used for the file name.", "maxLength": 80},
            },
            "required": ["text"],
        },
    })
}

fn create_podcast_tool() -> Value {
    json!({
        "name": "create_podcast",
        "description": "Make a podcast where two or more hosts talk with each other, each with their own voice — like a radio show: \
    the first host speaks with your voice (or the best one of the person's voice model), the others get \
    different voices (or the ones you pick with list_voices). Write a real conversation: short turns, reactions, \
    questions, a little humour, each host with a name and a personality; no markdown, no lists, no links. It \
    takes about as long as the audio lasts. The result gives a MEDIA: line: put it alone on a line in your reply.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "A short title, used for the file name.", "maxLength": 80},
                "turns": {
                    "type": "array", "maxItems": MAX_TURNS,
                    "description": "The conversation, in order: who speaks and what they say.",
                    "items": {"type": "object", "required": ["speaker", "text"], "properties": {
                        "speaker": {"type": "string", "description": "The host's name, the same in every turn."},
                        "text": {"type": "string", "description": "What they say in this turn."}}},
                },
                "voices": {"type": "object", "additionalProperties": {"type": "string"},
                           "description": "Optional: a voice per host name, from list_voices."},
            },
            "required": ["turns"],
        },
    })
}

fn list_voices_tool() -> Value {
    json!({
        "name": "list_voices",
        "description": "The voices create_podcast may give its hosts: yours first, then the person's voice model's, then \
    the Bips (the app's mascots), in your language.",
        "inputSchema": {"type": "object", "properties": {}},
    })
}

/// create_page's description: what a page is for, its locks, and the page kit with short examples to copy (the
/// quality lever: a small model writes a polished page in a few lines of HTML; pages.rs puts the kit inline).
pub const CREATE_PAGE_DESCRIPTION: &str = r##"Make a small live web page for the person, shown INSIDE your reply where you put its MEDIA: line (and kept in their Carnet): a dashboard, a comparison, a plan, a progression, a budget, a routine, a diagram, a calculator, a quiz, a checklist. Always use this for a page (never an .html file of your own: it would not open). Write ONE HTML file (or just its body): the data inline in a <script>. It runs locked, with no network at all: no CDN, no fetch, no external images or fonts (emoji, inline SVG, CSS or data: URIs). Nothing typed in it is kept. Phone first, a clear title and the date or source of the data in a .sub line. To change it later, call again with its page id. Link it to the Carnet page of its subject (notebook; search the Carnet first).
THE KIT is always there (no link needed), in the app's look, light and dark, in your colour; use it rather than your own CSS:
- text: h1, .sub (date, source), h2, .muted, .good/.warn/.bad; .note (a tinted tip), .chip (.good .warn .bad .plain)
- .card (a rounded block); .grid (tiles side by side; .grid.two: two columns); .row (.between)
- .kpi tiles: <div class="kpi"><span class="l">Label</span><span class="v">42<small>km</small></span><span class="d up">+12 %</span></div> (.kpi.main: the one that matters; .d.down)
- .bar progress: <div class="bar" style="--v:64%"></div> (.good .warn .bad); .big (a huge number)
- <table class="sort"> (a tap on a header sorts; td.n right-aligned, tr.best highlighted), wrapped in <div class="table-wrap">
- <ol class="timeline"><li class="done"><b>Step</b> text<span class="when">date</span></li> (li.now: current); <ul class="check"><li class="done">
- diagrams: <div class="flow"><div>A</div><div>B</div></div> (arrows; .flow.down vertical), or <pre class="diagram">A -> B
B -> C: label</pre> (boxes and arrows drawn for you)
- charts in SVG: <div id="c"></div><script>Bip.bars("#c",{labels:["L","M"],values:[5,8],unit:"km",goal:6})</script>; Bip.line(el,{labels,values|series:[{name,values}],unit,goal}); Bip.donut(el,{labels,values,unit,center:"1 748 €"}); Bip.spark(el,[3,5,4]); colours var(--c1)…var(--c6)
- what-ifs: <label class="field">Budget <output for="b" data-unit="€"></output></label><input type="range" id="b" min="0" max="500" value="200">, <label class="toggle">Bio <input type="checkbox" id="bio"></label>, then Bip.whatIf(v => Bip.set("#total", Bip.fmt(v.b*12,0,"€"))); .tabs: <div class="tabs"><button>Week</button><button>Month</button></div><section class="tab">…</section><section class="tab">…</section>
- maps: call map_image first (places as names or « lat, lon », numbered markers; line: true for a walk or a tour, directions: "walking" for a way there), then paste the <figure class="map-card"> it gives as it is: a live map in the app (move, zoom, « Open in Maps » on each place), its picture elsewhere
- Bip.fmt(n, digits, unit) formats in the person's language. Chart.js 4 too if you need more: <script src="chart.js"></script> (already in the app's colours).
EXAMPLES (whole pages):
1 dashboard: <h1>🏃 Tes sorties</h1><p class="sub">Santé · 4 dernières semaines</p><div class="grid"><div class="kpi main"><span class="l">Distance</span><span class="v">86<small>km</small></span><span class="d up">+14 %</span></div><div class="kpi"><span class="l">Allure</span><span class="v">5:42<small>/km</small></span></div></div><div class="card"><h3>Km par semaine</h3><div id="w"></div></div><script>Bip.bars("#w",{labels:["S1","S2","S3","S4"],values:[18,21,22,25],unit:"km"})</script>
2 comparison: <h1>Mutuelles</h1><p class="sub">Devis du 12/10</p><div class="table-wrap"><table class="sort"><thead><tr><th>Offre</th><th class="n">€/mois</th><th class="n">Optique</th></tr></thead><tbody><tr class="best"><td>Alan</td><td class="n">42</td><td class="n">200 €</td></tr><tr><td>MGEN</td><td class="n">48</td><td class="n">150 €</td></tr></tbody></table></div><p class="note">Alan : le moins cher et le mieux remboursé.</p>
3 plan: <h1>Plan 10 km</h1><p class="sub">6 semaines</p><ol class="timeline"><li class="done"><b>Semaine 1</b> 3 × 5 km<span class="when">3 oct.</span></li><li class="now"><b>Semaine 2</b> fractionné<span class="when">10 oct.</span></li><li><b>Semaine 6</b> 10 km 🎉</li></ol>
4 diagram: <h1>Le matin</h1><pre class="diagram">Réveil -> Petit-déj
Petit-déj -> École: 8 h 10
Petit-déj -> Travail</pre>
The result gives a MEDIA: line: put it alone on its own line where the page belongs in your reply."##;

fn create_page_tool() -> Value {
    json!({
        "name": "create_page",
        "description": CREATE_PAGE_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "html": {"type": "string", "description": "The whole page: one HTML file with everything inline."},
                "title": {"type": "string", "description": "A short title, shown on the page's card.", "maxLength": 80},
                "page": {"type": "string", "description": "To update a page: its id (from a previous result or list_pages)."},
                "notebook": {"type": "string", "description": "The Carnet page it is about: its name, its path (« Domaines/Santé ») or its id. Kept when you update the page without it."},
            },
            "required": ["html", "title"],
        },
    })
}

fn list_pages_tool() -> Value {
    json!({
        "name": "list_pages",
        "description": "The pages you made for the person (id, title, last change), the last changed first.",
        "inputSchema": {"type": "object", "properties": {}},
    })
}

fn read_page_tool() -> Value {
    json!({
        "name": "read_page",
        "description": "The HTML of one of your pages as you wrote it, to update it with create_page.",
        "inputSchema": {"type": "object", "properties": {"page": {"type": "string", "description": "The page id."}},
                        "required": ["page"]},
    })
}

fn delete_page_tool() -> Value {
    json!({
        "name": "delete_page",
        "description": "Delete one of your pages, when the person asks for it.",
        "inputSchema": {"type": "object", "properties": {"page": {"type": "string", "description": "The page id."}},
                        "required": ["page"]},
    })
}

fn knowledge_read_tool() -> Value {
    json!({
        "name": "knowledge_read",
        "description": "Read one of your knowledge pages (for instance mail/index.md: what the scan of the person's mailbox \
    wrote, then the pages it links to). Without a path: the list of your pages.",
        "inputSchema": {"type": "object", "properties": {"path": {"type": "string", "description": "e.g. mail/index.md"}}},
    })
}

fn knowledge_search_tool() -> Value {
    json!({
        "name": "knowledge_search",
        "description": "Search your knowledge pages for words; returns the matching lines with their page.",
        "inputSchema": {"type": "object", "properties": {"words": {"type": "string"}}, "required": ["words"]},
    })
}

// The mail tools are listed for every agent (Hermes reads the list once, when its gateway starts): an agent the person
// gave no mailbox is told so when it tries one.
fn propose_task_tool() -> Value {
    json!({
        "name": "propose_task",
        "description": "Propose a background task to the person: long work (a deep research, a file to prepare, many pages or \
    mails to go through) that would take far more than a reply. It does not start: the person sees it in the \
    app's « Tâches de fond » with its cost and starts it, then you do it in a conversation of its own and your \
    report is sent to them. Use it when the person asks for something that big; then tell them in your reply \
    that it waits for their go in « Tâches de fond ».",
        "inputSchema": {"type": "object", "properties": {
            "title": {"type": "string", "description": "a few words, in the person's language"},
            "prompt": {"type": "string", "description": "everything needed to do the task without this conversation: \
    the goal, what the person said, where to look, what to make"},
            "budget": {"type": "string", "enum": ["small", "medium", "large"],
                       "description": "small: a few searches; medium: a real research or file; large: hundreds of \
    pages or documents"}},
            "required": ["title", "prompt"]},
    })
}

static TOOL_LIST: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let mut tools = vec![
        create_audio_tool(),
        create_podcast_tool(),
        list_voices_tool(),
        create_page_tool(),
        list_pages_tool(),
        read_page_tool(),
        delete_page_tool(),
    ];
    tools.extend(mailbox::mail_tools());
    tools.extend([knowledge_read_tool(), knowledge_search_tool()]);
    tools.extend(reminders::reminder_tools());
    tools.push(propose_task_tool());
    tools.push(crate::jobs::start_task_tool());
    tools.push(crate::routes::route_tool());
    tools.push(crate::maps::tool());
    tools.push(crate::dates::tool());
    tools.push(crate::storage_watch::tool()); // the space a recurring production takes, and what to keep
    tools
});

/// `TOOLS`: every tool's definition, in Python's order.
pub fn tools() -> &'static [Value] {
    &TOOL_LIST
}

/// The tools with the family's (`tell_family`, `family_members`: only for an install with a linked household).
static FAMILY_TOOL_LIST: LazyLock<Vec<Value>> = LazyLock::new(|| {
    let mut tools = TOOL_LIST.clone();
    tools.extend(crate::family_news::tools());
    tools
});

/// The tools every agent of this install gets (the family's once a household is linked).
pub fn install_tools(config: &Config) -> &'static [Value] {
    if crate::family_news::has_family(config) {
        &FAMILY_TOOL_LIST
    } else {
        &TOOL_LIST
    }
}

/// An agent's own knowledge pages (memory_edit.py: knowledge/ in its profile, its person sees and edits them).
/// Err: a page that is not UTF-8 (Python's UnicodeDecodeError, which it lets through).
pub fn knowledge_tool(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> anyhow::Result<Dict> {
    use crate::memory_edit::{page_path, pages as listed};
    let folder = crate::agents_admin::profile_folder(config, agent);
    // read_text: a missing page or one outside the folder is caught (HTTPException, OSError), bad UTF-8 is not
    let read = |relative: &str| -> anyhow::Result<Option<String>> {
        let Ok(path) = page_path(&folder, relative) else { return Ok(None) };
        match tk::read_text(&path) {
            Ok(text) => Ok(Some(text)),
            Err(err) if err.kind() == std::io::ErrorKind::InvalidData => Err(err.into()),
            Err(_) => Ok(None),
        }
    };
    if name == KNOWLEDGE_READ {
        let path = tk::strip(&tk::str_or_empty(arguments.get("path"))).trim_start_matches('/').to_string();
        if path.is_empty() {
            let found = listed(&folder);
            if found.is_empty() {
                return Ok(tool_text("No knowledge page yet.", false));
            }
            let lines: Vec<String> = found.iter().map(|p| tk::str_or_empty(p.get("path"))).collect();
            return Ok(tool_text(&lines.join("\n"), false));
        }
        return Ok(match read(&path)? {
            Some(text) => tool_text(tk::head(&text, 20000), false),
            None => tool_text(&format!("no page {path} (knowledge_read without a path lists them)"), true),
        });
    }
    let words: Vec<String> = tk::split_ws(&tk::str_or_empty(arguments.get("words")))
        .into_iter()
        .filter(|w| tk::chars(w) > 1)
        .map(str::to_lowercase)
        .take(8)
        .collect();
    if words.is_empty() {
        return Ok(tool_text("give some words to search", true));
    }
    let mut hits: Vec<String> = Vec::new();
    'pages: for page in listed(&folder) {
        let page_name = tk::str_or_empty(page.get("path"));
        let Some(text) = read(&page_name)? else { continue };
        for line in crate::hermes::splitlines(&text) {
            let low = line.to_lowercase();
            if words.iter().all(|w| low.contains(w.as_str())) {
                hits.push(format!("{page_name}: {}", tk::head(tk::strip(&line), 240)));
                if hits.len() >= 40 {
                    break 'pages;
                }
            }
        }
    }
    if hits.is_empty() {
        Ok(tool_text("Nothing found.", false))
    } else {
        Ok(tool_text(&hits.join("\n"), false))
    }
}

/// `_digest`: what the store keeps of a token.
pub fn digest(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

pub fn store_path(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-tools.json")
}

pub fn audio_root(config: &Config) -> PathBuf {
    PathBuf::from(config.data_dir()).join("agent-audio")
}

/// The agents' audio files and pages (the bridge's own folders).
pub fn media_roots(config: &Config) -> IndexMap<String, String> {
    let mut roots = IndexMap::new();
    roots.insert(MEDIA_PREFIX.into(), audio_root(config).to_string_lossy().into_owned());
    roots.insert(pages::PREFIX.into(), pages::root(config).to_string_lossy().into_owned());
    roots.insert(crate::maps::PREFIX.into(), crate::maps::media_root(config).to_string_lossy().into_owned());
    roots
}

/// The files sent to the phone from a space card (storage_watch.rs): documents too, until the person saved them.
fn send_roots(config: &Config) -> IndexMap<String, String> {
    let root = crate::storage_watch::send_root(config).to_string_lossy().into_owned();
    IndexMap::from([(crate::storage_watch::SEND_PREFIX.to_string(), root)])
}

/// The host file of a `MEDIA:` path under `[media] roots` (media and documents) or the bridge's own folders (media
/// only: a page's record and source beside it are never served).
pub fn served_media(config: &Config, path: &str) -> Option<String> {
    crate::media::resolve(path, &config.media_roots)
        .or_else(|| crate::media::resolve_media(path, &media_roots(config)))
        .or_else(|| crate::media::resolve(path, &send_roots(config)))
}

/// `tools_version()` of a tool list: the first 10 hex digits of the SHA-256 of `json.dumps(tools, sort_keys=True)`.
pub fn version_of(tools: &[Value]) -> String {
    let text = py::Dumps::new().sort_keys().dumps(&Value::Array(tools.to_vec()));
    hex::encode(Sha256::digest(text.as_bytes()))[..10].to_string()
}

/// The tools of `agent`: the install's, the Carnet's (on by default), `health` when the person gave it their health data, the calendar's when they
/// shared their calendar with it, the address book's when they shared that, `weather` when its « Météo » is on.
pub fn tools_for(config: &Config, agent: &AgentConfig) -> Vec<Value> {
    if crate::family_agent::is_family(config, &agent.name) {
        return crate::family_agent::tools(config); // nobody's private data (family_agent.rs)
    }
    if crate::project_agent::is_project(config, &agent.name) {
        return crate::project_agent::tools(config); // shown to friends: nobody's private data (project_agent.rs)
    }
    let mut tools = install_tools(config).to_vec();
    if crate::notebook::enabled(config, &agent.name) {
        tools.extend(crate::notebook::tools::tools(crate::notebook::health_agent(config, &agent.name)));
        tools.push(crate::notebook::fill::tool());
    }
    if crate::weather::enabled(config, &agent.name) {
        tools.push(crate::weather::weather_tool());
    }
    if crate::health::enabled(config, &agent.name) {
        tools.push(crate::health::health_tool(&crate::health::categories(config, &agent.name)));
    }
    if calendar_on(config, &agent.name) {
        tools.extend(crate::calendar::tools());
    }
    if crate::meet::offered(config, &agent.name) {
        tools.push(crate::meet::tool()); // a date to find with a relative's household (meet.rs)
    }
    if crate::contacts::enabled(config, &agent.name) {
        tools.extend(crate::contacts::tools());
    }
    if crate::agent_files::offered(config, &agent.name) {
        tools.push(crate::agent_files::tool());
    }
    if crate::agent_messages::offered(config, &agent.name) {
        tools.push(crate::agent_messages::tool()); // in any conversation, not only its Bot Chat (agent_messages.rs)
    }
    if crate::archives::enabled(config, &agent.name) {
        tools.extend(crate::archives::tools(config, &agent.name));
    }
    // the guide's (propose_agent, suggest_switch), or propose_profile while it has no role or a generic name
    tools.extend(crate::guide::tools(config, agent));
    // the guide's search of ready-made models (templates.rs)
    tools.extend(crate::templates::tools(config, agent));
    tools
}

/// The agents whose tools now differ from what their latest MCP session listed, each with since when (Unix time):
/// their Hermes hasn't fetched the new list yet (mcp_sessions.rs). An agent with no session isn't stale.
pub fn stale_agents(config: &Config) -> Vec<(String, f64)> {
    crate::mcp_sessions::SESSIONS.stale(
        |name| config.agent(Some(name)).map(|agent| version_of(&tools_for(config, agent))),
        py::time_time(),
    )
}

/// Whether `agent` reads a calendar: the phone's, or one of its mailboxes' (caldav.rs).
pub fn calendar_on(config: &Config, agent: &str) -> bool {
    crate::calendar::enabled(config, agent) || crate::caldav::shared_with(config, agent)
}

/// Changes whenever the tools do: Hermes keeps the list it got when it connected, so a new tool needs a new entry.
pub fn tools_version() -> String {
    static VERSION: LazyLock<String> = LazyLock::new(|| version_of(tools()));
    VERSION.clone()
}

/// The bridge as the agents' Hermes reaches it: this machine, unless the agents' gateways run elsewhere (`peer_url`,
/// set for spaces in containers of their own). The tools' version rides along (`?v=`), ignored by the bridge: a
/// changed entry makes Hermes connect again and fetch the new list.
pub fn tools_url(config: &Config) -> String {
    // A household linked or unlinked changes the list (the family's tools): a new version, so Hermes fetches it.
    let version =
        if crate::family_news::has_family(config) { version_of(install_tools(config)) } else { tools_version() };
    url_with(config, &version)
}

/// `tools_url` for a given tools version.
pub fn url_with(config: &Config, version: &str) -> String {
    let base = match config.peer_url.as_deref() {
        Some(peer) if peer.ends_with("/peer") => format!("{}{PATH}", &peer[..peer.len() - "/peer".len()]),
        _ => format!("http://127.0.0.1:{}{PATH}", config.port),
    };
    format!("{base}?v={version}")
}

pub fn load_tokens(config: &Config) -> IndexMap<String, String> {
    let Ok(text) = std::fs::read(store_path(config)) else { return IndexMap::new() };
    let Ok(data) = serde_json::from_slice::<Value>(&text) else { return IndexMap::new() };
    match data.get("tokens") {
        Some(Value::Object(tokens)) => tokens.iter().map(|(k, v)| (k.clone(), py::py_str(v))).collect(),
        _ => IndexMap::new(),
    }
}

pub fn save_tokens(config: &Config, tokens: &IndexMap<String, String>) -> std::io::Result<()> {
    let path = store_path(config);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("part");
    let map: Dict = tokens.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect();
    std::fs::write(&tmp, format!("{}\n", py::Dumps::new().indent(1).dumps(&json!({"tokens": map}))))?;
    py::chmod(&tmp.to_string_lossy(), 0o600)?;
    std::fs::rename(&tmp, &path)
}

/// The agent a token belongs to (its digest in the store).
pub fn agent_for(config: &Config, token: &str) -> Option<AgentConfig> {
    let wanted = digest(token);
    for (name, known) in load_tokens(config) {
        if crate::auth::constant_time_eq(known.as_bytes(), wanted.as_bytes()) {
            return config.agent(Some(&name)).cloned();
        }
    }
    None
}

/// The agent's Hermes knows the bridge's tools: an MCP entry with its own token in its profile. Agents set up by
/// hand are left as they are. Returns whether the profile was changed (a new conversation picks it up). Err: a
/// `hermes config set` that failed (Python's RuntimeError/OSError).
pub async fn register(config: &Config, agent: &AgentConfig, run: &Run) -> anyhow::Result<bool> {
    crate::mcp_sessions::poke(); // its tools may have changed: its open MCP streams check now (mcp_sessions.rs)
    crate::notebook::sync_soul(config, agent);
    crate::archives::sync_soul(config, agent);
    crate::souls::sync_payments(config, agent);
    let tools = tools_for(config, agent);
    let url = if tools.as_slice() != install_tools(config) {
        url_with(config, &version_of(&tools))
    } else {
        tools_url(config)
    };
    let changed = register_at(config, agent, run, &url).await?;
    // The card payment tools (card_pay.rs): their own entry, with the same token.
    let paying = crate::card_pay::register(config, agent, run).await.unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "card payment tools not registered");
        false
    });
    // The vault's account tools (vault_logins.rs): their own entry too.
    let accounts = crate::vault_logins::register(config, agent, run).await.unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "vault account tools not registered");
        false
    });
    // The connectors it makes (connectors.rs): their tools' entry, and one per connector the person approved.
    let connecting = crate::connectors::register(config, agent, run).await.unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "connector tools not registered");
        false
    });
    // « L'agent prépare, tu paies » (handover.rs) for every agent, and Link Agent Wallet (link_wallet.rs) for the
    // agents the person chose, where it is on.
    let handing = crate::handover::register(config, agent, run).await.unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "checkout tool not registered");
        false
    });
    let link = crate::link_wallet::register(config, agent, run).await.unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "Link tools not registered");
        false
    });
    // What its kind keeps in view, the rest behind Hermes' tool search (tool_diet.rs): after the entries above, so
    // the bridge tools it keeps direct are the ones it has now.
    let diet = crate::tool_diet::apply(config, agent).unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "tools in view not set");
        false
    });
    // The providers EUrouter tries first (model_route.rs): at each start, the migration of existing agents too.
    let routed = crate::model_route::apply(config, agent).unwrap_or_else(|exc| {
        tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "model provider routing not set");
        false
    });
    // The thinking level the person chose, or GLM's « low », after the `hermes config set` above (agents_admin.rs).
    let thinking = crate::agents_admin::heal_thinking_of(config, agent);
    Ok(changed || paying || accounts || connecting || handing || link || diet || routed || thinking)
}

/// `register` with the tools' URL given (tests: a later version of the tools).
pub async fn register_at(config: &Config, agent: &AgentConfig, run: &Run, url: &str) -> anyhow::Result<bool> {
    let Some(folder) = shares::profile_dir(config, agent) else { return Ok(false) };
    if !folder.is_dir() {
        return Ok(false);
    }
    let profile = shares::profile_config(&folder);
    let servers = profile.get("mcp_servers").filter(|v| py::truthy(v));
    let entry = servers.and_then(|s| s.get(SERVER_NAME)).filter(|v| py::truthy(v)).cloned().unwrap_or(json!({}));
    let mut tokens = load_tokens(config);
    let header = match &entry {
        Value::Object(map) => match map.get("headers").filter(|v| py::truthy(v)).and_then(|h| h.get("Authorization")) {
            Some(v) => py::py_str(v),
            None => String::new(),
        },
        _ => String::new(),
    };
    let current = header.strip_prefix("Bearer ").unwrap_or("").to_string();
    let known = !current.is_empty() && tokens.get(&agent.name) == Some(&digest(&current));
    let mut same = false;
    if let Value::Object(map) = &entry {
        let timeout = py::py_str(map.get("timeout").unwrap_or(&Value::Null));
        same = map.get("url") == Some(&Value::String(url.to_string())) && known && timeout == TOOL_TIMEOUT.to_string();
    }
    // An isolated sandbox's file tools (sandbox.rs): a second entry, « untrusted » so Hermes asks the person before
    // each write or delete, with the same token.
    let files = crate::sandbox::isolates(config, agent);
    let files_entry = servers.and_then(|s| s.get(crate::sandbox::SERVER_NAME)).cloned().unwrap_or(json!({}));
    let files_same = !files
        || (files_entry.get("url") == Some(&json!(crate::sandbox::tools_url(config)))
            && files_entry.get("trust") == Some(&json!("untrusted"))
            && files_entry.get("headers").and_then(|h| h.get("Authorization")) == Some(&json!(header)));
    if same && files_same {
        return Ok(false);
    }
    let token = if known { current } else { py::token_urlsafe(32) };
    // the profiles' folder is <home>/profiles/<name>
    let home = folder.parent().and_then(Path::parent).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let env: BTreeMap<String, String> = [("HERMES_HOME".to_string(), home)].into_iter().collect();
    let base = vec![config.hermes.bin.clone(), "-p".into(), agent.name.clone(), "config".into(), "set".into()];
    let with = |key: String, value: String| {
        let mut args = base.clone();
        args.extend([key, value]);
        args
    };
    if files {
        let name = crate::sandbox::SERVER_NAME;
        run(with(format!("mcp_servers.{name}.url"), crate::sandbox::tools_url(config)), env.clone()).await?;
        run(with(format!("mcp_servers.{name}.headers.Authorization"), format!("Bearer {token}")), env.clone()).await?;
        run(with(format!("mcp_servers.{name}.trust"), "untrusted".into()), env.clone()).await?;
        tracing::info!(agent = %agent.name, "file tools registered (writes and deletes approved by the person)");
        if same {
            return Ok(true);
        }
    }
    run(with(format!("mcp_servers.{SERVER_NAME}.url"), url.to_string()), env.clone()).await?;
    run(with(format!("mcp_servers.{SERVER_NAME}.headers.Authorization"), format!("Bearer {token}")), env.clone())
        .await?;
    run(with(format!("mcp_servers.{SERVER_NAME}.timeout"), TOOL_TIMEOUT.to_string()), env).await?;
    tokens.insert(agent.name.clone(), digest(&token));
    save_tokens(config, &tokens)?;
    tracing::info!(agent = %agent.name, "agent tools registered");
    Ok(true)
}

/// At start: every agent this install manages gets the tools (agents made before them, or moved since).
pub async fn register_all(config: SharedConfig, run: Run) {
    let snapshot = config.get();
    for agent in snapshot.agents.values() {
        if let Err(exc) = register(&snapshot, agent, &run).await {
            tracing::warn!(agent = %agent.name, error = %last_chars(&exc.to_string(), 200), "agent tools not registered");
        }
    }
}

/// `text[-n:]`.
fn last_chars(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(n)..].iter().collect()
}

/// A deleted agent: its token goes, and its pages.
pub fn forget(config: &Config, name: &str) {
    pages::forget(config, name);
    crate::approvals_log::forget(config, name);
    crate::maps::forget(config, name);
    crate::notebook::forget(config, name);
    crate::archives::forget(config, name);
    let mut tokens = load_tokens(config);
    if tokens.shift_remove(name).is_some() {
        if let Err(err) = save_tokens(config, &tokens) {
            tracing::warn!(agent = %name, error = %err, "agent tools token not removed");
        }
    }
}

// --- the audio ---------------------------------------------------------------------------------------------------

/// `re.split(r"(?<=[<ends>])\s+", text)`.
fn split_after(text: &str, ends: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev: Option<char> = None;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if tk::is_py_space(c) && prev.map(|p| ends.contains(&p)).unwrap_or(false) {
            while chars.peek().map(|n| tk::is_py_space(*n)).unwrap_or(false) {
                chars.next();
            }
            out.push(std::mem::take(&mut current));
            prev = Some(c);
            continue;
        }
        current.push(c);
        prev = Some(c);
    }
    out.push(current);
    out
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

/// `text` in pieces of at most `limit` characters, cut between sentences, else at commas, else between words.
fn pieces(text: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for unit in split_after(text, &['.', '!', '?', '…']) {
        let unit = tk::strip(&unit).to_string();
        if unit.is_empty() {
            continue;
        }
        let parts: Vec<String> = if char_len(&unit) <= limit {
            vec![unit]
        } else {
            split_after(&unit, &[',', ';', ':']).into_iter().filter(|p| !tk::strip(p).is_empty()).collect()
        };
        for part in parts {
            let mut part: Vec<char> = part.chars().collect();
            while part.len() > limit {
                let found = part[..limit].iter().rposition(|c| *c == ' ');
                let cut = match found {
                    Some(i) if i > limit / 2 => i,
                    _ => limit,
                };
                let head: String = part[..cut].iter().collect();
                out.push(tk::strip(&head).to_string());
                let rest: String = part[cut..].iter().collect();
                part = tk::strip(&rest).chars().collect();
            }
            let part: String = part.into_iter().collect();
            if part.is_empty() {
                continue;
            }
            match out.last_mut() {
                Some(last) if char_len(last) + 1 + char_len(&part) <= limit => *last = format!("{last} {part}"),
                _ => out.push(part),
            }
        }
    }
    out
}

/// `re.split(r"\n\s*\n", text)`.
fn paragraphs(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            let mut j = i + 1;
            let mut last_newline = None;
            while j < chars.len() && tk::is_py_space(chars[j]) {
                if chars[j] == '\n' {
                    last_newline = Some(j);
                }
                j += 1;
            }
            if let Some(k) = last_newline {
                out.push(chars[start..i].iter().collect());
                start = k + 1;
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    out.push(chars[start..].iter().collect());
    out
}

/// `(piece, pause after it in ms)`: paragraphs (blank lines) kept apart by a longer pause. `MEDIA:` lines go.
pub fn split_for_speech(text: &str, limit: usize) -> Vec<(String, u32)> {
    let kept: Vec<String> =
        crate::hermes::splitlines(text).into_iter().filter(|line| !tk::strip(line).starts_with("MEDIA:")).collect();
    let text = kept.join("\n");
    let mut chunks = Vec::new();
    for paragraph in paragraphs(&text) {
        let paragraph = tk::squeeze(&paragraph);
        if paragraph.is_empty() {
            continue;
        }
        let found = pieces(&paragraph, limit);
        let count = found.len();
        for (index, piece) in found.into_iter().enumerate() {
            chunks.push((piece, if index + 1 == count { PAUSE_PARAGRAPH_MS } else { PAUSE_SENTENCES_MS }));
        }
    }
    chunks
}

fn silence(ms: u32) -> Vec<u8> {
    vec![0u8; 2 * (OUTPUT_RATE as usize * ms as usize / 1000)]
}

fn slug(title: &str) -> String {
    let text = tk::dashes(&tk::ascii_fold(&tk::spaced_punctuation(title)).to_lowercase());
    let text = tk::head(text.trim_matches('-'), 40).to_string();
    if text.is_empty() {
        "audio".into()
    } else {
        text
    }
}

/// What generated audio says it is, in its metadata: synthetic speech made by an AI agent.
pub fn ai_mark(agent: &str) -> String {
    format!("AI-generated: synthetic voice by the AI agent {agent} (BipAgents). digitalsourcetype={AI_SOURCE}")
}

/// A RIFF LIST/INFO chunk (ICMT comment, ISFT software) at the end of a WAV, its RIFF size updated.
pub fn tag_wav(path: &Path, comment: &str, software: &str) -> std::io::Result<()> {
    fn field(key: &[u8], text: &str) -> Vec<u8> {
        let mut data = text.as_bytes().to_vec();
        data.push(0);
        if data.len() % 2 == 1 {
            data.push(0);
        }
        let mut out = key.to_vec();
        out.extend((data.len() as u32).to_le_bytes());
        out.extend(data);
        out
    }
    let mut body = b"INFO".to_vec();
    body.extend(field(b"ICMT", comment));
    body.extend(field(b"ISFT", software));
    let mut handle = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    handle.seek(SeekFrom::End(0))?;
    handle.write_all(b"LIST")?;
    handle.write_all(&(body.len() as u32).to_le_bytes())?;
    handle.write_all(&body)?;
    let size = handle.stream_position()? - 8;
    handle.seek(SeekFrom::Start(4))?;
    handle.write_all(&(size as u32).to_le_bytes())?;
    Ok(())
}

/// An .m4a next to the WAV when ffmpeg is here (a tenth of the size), else the WAV itself.
async fn encode(wav_path: &Path, comment: &str) -> PathBuf {
    let Some(ffmpeg) = crate::spaces::which("ffmpeg") else { return wav_path.to_path_buf() };
    let target = wav_path.with_extension("m4a");
    let mut args: Vec<String> = ["-nostdin", "-y", "-loglevel", "error", "-i"].iter().map(|s| s.to_string()).collect();
    args.push(wav_path.to_string_lossy().into_owned());
    args.extend(["-c:a", "aac", "-b:a", "64k"].iter().map(|s| s.to_string()));
    if !comment.is_empty() {
        args.extend([
            "-metadata".into(),
            format!("comment={comment}"),
            "-metadata".into(),
            format!("description={comment}"),
            "-metadata".into(),
            "encoder=BipAgents".into(),
        ]);
    }
    args.push(target.to_string_lossy().into_owned());
    let output = tokio::process::Command::new(ffmpeg)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;
    let failed = match &output {
        Ok(out) => !out.status.success() || !target.exists(),
        Err(_) => true,
    };
    if failed {
        let err = match &output {
            Ok(out) => String::from_utf8_lossy(&out.stderr).into_owned(),
            Err(err) => err.to_string(),
        };
        tracing::warn!(error = %last_chars(&err, 200), "audio not encoded, kept as WAV");
        let _ = std::fs::remove_file(&target);
        return wav_path.to_path_buf();
    }
    let _ = std::fs::remove_file(wav_path);
    target
}

/// Python's `round()` to an int (ties to even).
fn py_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// A voice per speaker, in order of appearance: the one asked for (from `voices`), else voices of the person's
/// voice model when it reads (natural voices make a better podcast than Bips), the agent's own leading, then
/// different ones of the same family first (« naturelle » with « naturelle »); without a voice model, Bips.
pub fn cast_from(
    config: &Config,
    available: &[Dict],
    speakers: &[String],
    wanted: &IndexMap<String, String>,
) -> anyhow::Result<IndexMap<String, String>> {
    let all: Vec<String> = available.iter().map(|e| tk::str_or_empty(e.get("voice"))).collect();
    let allowed: HashSet<&String> = all.iter().collect();
    for (speaker, voice) in wanted {
        if !allowed.contains(voice) {
            return Err(value_error(format!("unknown voice for {speaker}: {voice} (see list_voices)")));
        }
    }
    let own = all.first().cloned().unwrap_or_default();
    let model: Vec<String> = all.iter().filter(|v| v.starts_with("server:")).cloned().collect();
    let server_reads = config.voice_server.as_ref().map(|s| s.uses.get("reading").copied().unwrap_or(true));
    let mut pool: Vec<String> = if !model.is_empty() && server_reads == Some(true) {
        model
    } else {
        let family = own.split(':').next().unwrap_or("").to_string();
        all.iter().filter(|v| v.split(':').next().unwrap_or("") == family).cloned().collect()
    };
    if !pool.contains(&own) {
        // the agent speaks with a Bip: a natural voice of the model leads, not a mascot's
        pool.sort_by_key(|v| if v.contains("natur") { 0 } else { 1 });
    }
    let lead = if pool.contains(&own) || pool.is_empty() { own.clone() } else { pool[0].clone() };
    let name = lead.rsplit('/').next().unwrap_or("").to_string();
    let family =
        if name.matches('-').count() >= 2 { name.split('-').nth(1).unwrap_or("").to_string() } else { String::new() };
    let mut ranked: Vec<String> =
        pool.iter().filter(|v| !family.is_empty() && v.contains(&format!("-{family}-"))).cloned().collect();
    ranked.extend(pool.iter().cloned());
    ranked.extend(all.iter().cloned());
    let mut cast = IndexMap::new();
    let mut used: HashSet<String> = wanted.values().cloned().collect();
    for speaker in speakers {
        if let Some(voice) = wanted.get(speaker) {
            cast.insert(speaker.clone(), voice.clone());
            continue;
        }
        let choice = if !used.contains(&lead) {
            lead.clone()
        } else {
            ranked.iter().find(|v| !used.contains(*v)).cloned().unwrap_or_else(|| lead.clone())
        };
        cast.insert(speaker.clone(), choice.clone());
        used.insert(choice);
    }
    Ok(cast)
}

/// Audio files and podcasts made for an agent (`create_audio`, `create_podcast`). One audio at a time per agent (a
/// Pocket's voices per language, and when they were read (kept a while).
type PocketVoices = (Instant, IndexMap<String, Vec<String>>);

/// second request waits for the first).
pub struct AudioMaker {
    pub config: SharedConfig,
    pub tts: Arc<TtsService>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    pocket_cache: Mutex<Option<PocketVoices>>,
}

impl AudioMaker {
    pub fn new(config: SharedConfig, tts: Arc<TtsService>) -> Self {
        AudioMaker { config, tts, locks: Mutex::new(HashMap::new()), pocket_cache: Mutex::new(None) }
    }

    fn lock_of(&self, agent: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap_or_else(|e| e.into_inner()).entry(agent.to_string()).or_default().clone()
    }

    /// The Bips Pocket serves, by language (its `/health`), kept a few minutes.
    pub async fn pocket_voices(&self) -> IndexMap<String, Vec<String>> {
        let Some(pocket) = self.tts.pocket.as_ref() else { return IndexMap::new() };
        let now = Instant::now();
        if let Some((at, voices)) = self.pocket_cache.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if now.duration_since(*at) < Duration::from_secs(300) {
                return voices.clone();
            }
        }
        let found: Option<Value> = async {
            let resp = self
                .tts
                .client
                .get(format!("{}/health", pocket.url))
                .timeout(Duration::from_secs(5))
                .send()
                .await
                .ok()?;
            if resp.status().as_u16() != 200 {
                return None;
            }
            let data: Value = resp.json().await.ok()?;
            data.get("voices").cloned()
        }
        .await;
        let mut voices = IndexMap::new();
        if let Some(Value::Object(map)) = found {
            for (k, names) in map {
                let list = match names {
                    Value::Array(items) => items.iter().map(py::py_str).collect(),
                    Value::String(s) => s.chars().map(|c| c.to_string()).collect(),
                    _ => Vec::new(),
                };
                voices.insert(k, list);
            }
        }
        *self.pocket_cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, voices.clone()));
        voices
    }

    /// The voices an agent may give a podcast's hosts: its own first, then its voice model's in its language,
    /// then the Bips in its language.
    pub async fn voices(&self, agent: &AgentConfig) -> Vec<Dict> {
        let config = self.config.get();
        let own = voice_for(&config, Some(agent), Some("reading"));
        let language = voice_language(&own);
        let entry = |voice: String, kind: &str| -> Dict {
            let mut d = Dict::new();
            d.insert("voice".into(), Value::String(voice));
            d.insert("kind".into(), Value::String(kind.into()));
            d
        };
        let mut found = vec![entry(own.clone(), "your voice")];
        if config.voice_server.is_some() {
            for name in self.tts.server_voices(300.0).await {
                let prefix = name.split('-').next().unwrap_or("");
                if prefix.chars().count() == 2 && prefix != language {
                    continue; // « en-naturelle-s47 »: another language's voice
                }
                found.push(entry(format!("server:{language}/{name}"), "voice model"));
            }
        }
        let bips = self.pocket_voices().await.get(&language).cloned().unwrap_or_default();
        for name in bips {
            let voice = if language == "fr" { format!("pocket:{name}") } else { format!("pocket:{language}/{name}") };
            found.push(entry(voice, "Bip"));
        }
        let mut seen = HashSet::new();
        found.into_iter().filter(|e| seen.insert(tk::str_or_empty(e.get("voice")))).collect()
    }

    /// A voice per speaker, in order of appearance (see [`cast_from`]). Err: a wanted voice that is not offered
    /// (Python's ValueError).
    pub async fn cast(
        &self,
        agent: &AgentConfig,
        speakers: &[String],
        wanted: &IndexMap<String, String>,
    ) -> anyhow::Result<IndexMap<String, String>> {
        let available = self.voices(agent).await;
        cast_from(&self.config.get(), &available, speakers, wanted)
    }

    /// PCM16 at 24 kHz, the pieces of `text` read one after the other (no pause after the last).
    async fn speak(&self, text: &str, voice: &str, relay: Option<String>) -> Result<Vec<u8>, TtsError> {
        let chunks = split_for_speech(text, CHUNK_CHARS);
        let mut pcm = Vec::new();
        let count = chunks.len();
        for (index, (piece, pause)) in chunks.into_iter().enumerate() {
            let options = SynthOptions { normalized: false, max_chars: Some(0), relay: relay.clone() };
            let audio = self.tts.synthesize(&piece, Some(voice), "pcm16", BACKGROUND, options).await?;
            let mut data = if audio.channels == 1 { audio.data.to_vec() } else { mono(&audio.data, audio.channels) };
            if audio.sample_rate != OUTPUT_RATE {
                data = Resampler::new(audio.sample_rate, OUTPUT_RATE).feed(&data, true);
            }
            pcm.extend(data);
            if index + 1 < count {
                pcm.extend(silence(pause));
            }
        }
        Ok(pcm)
    }

    async fn save(&self, agent: &AgentConfig, pcm: &[u8], title: &str) -> anyhow::Result<Dict> {
        let config = self.config.get();
        let folder = audio_root(&config).join(&agent.name);
        std::fs::create_dir_all(&folder)?;
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let wav_path = folder.join(format!("{stamp}-{}.wav", slug(title)));
        std::fs::write(&wav_path, crate::tts::pcm16_to_wav(pcm, OUTPUT_RATE, 1))?;
        let shown = if agent.display_name.is_empty() { &agent.name } else { &agent.display_name };
        let mark = ai_mark(shown);
        tag_wav(&wav_path, &mark, "BipAgents")?;
        let last = encode(&wav_path, &mark).await;
        let file = last.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut made = Dict::new();
        made.insert("path".into(), Value::String(format!("{MEDIA_PREFIX}/{}/{file}", agent.name)));
        made.insert("seconds".into(), json!(pcm.len() as f64 / 2.0 / OUTPUT_RATE as f64));
        crate::storage_watch::mark_dirty(); // a big write: the place's space measured again soon
        Ok(made)
    }

    pub async fn create(&self, agent: &AgentConfig, text: &str, title: &str) -> anyhow::Result<Dict> {
        let text = tk::strip(text);
        if text.is_empty() {
            return Err(value_error("the text is empty"));
        }
        let length = char_len(text);
        if length > MAX_CHARS {
            return Err(value_error(format!(
                "the text is too long ({length} characters, at most {MAX_CHARS}): split it"
            )));
        }
        if split_for_speech(text, CHUNK_CHARS).is_empty() {
            return Err(value_error("nothing to read once MEDIA: lines are removed"));
        }
        let voice = voice_for(&self.config.get(), Some(agent), Some("reading"));
        let lock = self.lock_of(&agent.name);
        let guard = lock.lock().await;
        let started = Instant::now();
        let pcm = self.speak(text, &voice, relay_for(Some(agent))).await?;
        let title = if title.is_empty() { tk::head(text, 40) } else { title };
        let made = self.save(agent, &pcm, title).await?;
        drop(guard);
        let seconds = made.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
        tracing::info!(agent = %agent.name, chars = length, seconds = py_round(seconds),
            took = py_round(started.elapsed().as_secs_f64()), "agent audio made");
        Ok(made)
    }

    /// `turns`: the tool's list as given (items that are not objects are skipped); `voices`: a voice per host.
    pub async fn podcast(
        &self,
        agent: &AgentConfig,
        turns: &[Value],
        title: &str,
        voices: Option<&Dict>,
    ) -> anyhow::Result<Dict> {
        let mut clean: Vec<(String, String)> = Vec::new();
        for turn in turns {
            let Value::Object(turn) = turn else { continue };
            let speaker = tk::head(&tk::squeeze(&tk::str_or_empty(turn.get("speaker"))), 40).to_string();
            let text = tk::strip(&tk::str_or_empty(turn.get("text"))).to_string();
            if !speaker.is_empty() && !text.is_empty() && !split_for_speech(&text, CHUNK_CHARS).is_empty() {
                clean.push((speaker, text));
            }
        }
        if clean.is_empty() {
            return Err(value_error("no turn to read: each turn needs a speaker and a text"));
        }
        if clean.len() > MAX_TURNS {
            return Err(value_error(format!(
                "too many turns ({}, at most {MAX_TURNS}): make two episodes",
                clean.len()
            )));
        }
        let total: usize = clean.iter().map(|(_, t)| char_len(t)).sum();
        if total > MAX_CHARS {
            return Err(value_error(format!(
                "the podcast is too long ({total} characters, at most {MAX_CHARS}): make two episodes"
            )));
        }
        let mut speakers: Vec<String> = Vec::new();
        for (speaker, _) in &clean {
            if !speakers.contains(speaker) {
                speakers.push(speaker.clone());
            }
        }
        if speakers.len() > MAX_SPEAKERS {
            return Err(value_error(format!("too many speakers ({}, at most {MAX_SPEAKERS})", speakers.len())));
        }
        let wanted: IndexMap<String, String> = voices
            .map(|v| v.iter().filter(|(k, _)| speakers.contains(k)).map(|(k, v)| (k.clone(), py::py_str(v))).collect())
            .unwrap_or_default();
        let cast = self.cast(agent, &speakers, &wanted).await?;
        let relay = relay_for(Some(agent));
        let lock = self.lock_of(&agent.name);
        let guard = lock.lock().await;
        let started = Instant::now();
        let mut pcm = Vec::new();
        let first = cast[&speakers[0]].clone();
        for (index, (speaker, text)) in clean.iter().enumerate() {
            let voice = &cast[speaker];
            pcm.extend(self.speak(text, voice, if *voice == first { relay.clone() } else { None }).await?);
            if index + 1 < clean.len() {
                pcm.extend(silence(PAUSE_TURN_MS));
            }
        }
        let mut made = self.save(agent, &pcm, if title.is_empty() { "podcast" } else { title }).await?;
        drop(guard);
        let seconds = made.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
        tracing::info!(agent = %agent.name, turns = clean.len(), speakers = speakers.len(), seconds = py_round(seconds),
            took = py_round(started.elapsed().as_secs_f64()), "agent podcast made");
        let cast_map: Dict = cast.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
        made.insert("cast".into(), Value::Object(cast_map));
        Ok(made)
    }
}

pub fn purge_audio(config: &Config, retention_days: i64, now: Option<f64>) -> usize {
    let root = audio_root(config);
    if !root.is_dir() {
        return 0;
    }
    let now = now.filter(|n| *n != 0.0).unwrap_or_else(py::time_time);
    let cutoff = now - retention_days as f64 * 86400.0;
    let mut removed = 0;
    for folder in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let Ok(entries) = std::fs::read_dir(folder.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_link = std::fs::symlink_metadata(&path).map(|m| m.file_type().is_symlink()).unwrap_or(true);
            if path.is_file()
                && !is_link
                && tk::mtime(&path).map(|m| m < cutoff).unwrap_or(false)
                && std::fs::remove_file(&path).is_ok()
            {
                removed += 1;
            }
        }
    }
    removed
}

// --- MCP (streamable HTTP, JSON-RPC) ------------------------------------------------------------------------------

fn rpc_result(request_id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": request_id, "result": result})
}

fn rpc_error(request_id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}})
}

fn duration(seconds: f64) -> String {
    let total = py_round(seconds);
    let (minutes, rest) = (total.div_euclid(60), total.rem_euclid(60));
    if minutes != 0 {
        format!("{minutes} min {rest:02} s")
    } else {
        format!("{rest} s")
    }
}

/// create_page, list_pages, read_page, delete_page. Err: what Python lets through (an OSError writing the page).
pub fn page_tool(config: &Config, agent: &AgentConfig, name: &str, arguments: &Dict) -> anyhow::Result<Dict> {
    let page = tk::strip(&tk::str_or_empty(arguments.get("page"))).to_string();
    let caught = |err: anyhow::Error| -> anyhow::Result<Dict> {
        match err.downcast_ref::<pages::ValueError>() {
            Some(message) => Ok(tool_text(&message.0, true)),
            None => Err(err),
        }
    };
    if name == LIST_PAGES {
        let found = pages::list_pages(config, &agent.name);
        if found.is_empty() {
            return Ok(tool_text("No page yet.", false));
        }
        let lines: Vec<String> = found
            .iter()
            .map(|p| {
                format!(
                    "{}: {} (changed {} ago, version {})",
                    py::py_str(p.get("page").unwrap_or(&Value::Null)),
                    tk::str_or_empty(p.get("title")),
                    pages::age(p, None),
                    py::py_str(p.get("version").unwrap_or(&Value::Null))
                )
            })
            .collect();
        return Ok(tool_text(&lines.join("\n"), false));
    }
    if name == READ_PAGE {
        return match pages::source(config, &agent.name, &page) {
            Ok(text) => Ok(tool_text(&text, false)),
            Err(err) => caught(err),
        };
    }
    if name == DELETE_PAGE {
        if !pages::delete(config, &agent.name, &page) {
            return Ok(tool_text(&format!("unknown page id: {page} (see list_pages)"), true));
        }
        tracing::info!(agent = %agent.name, page = %page, "agent page deleted");
        return Ok(tool_text(&format!("Page {page} deleted."), false));
    }
    let made = match pages::save(
        config,
        &agent.name,
        &tk::str_or_empty(arguments.get("html")),
        &tk::str_or_empty(arguments.get("title")),
        if page.is_empty() { None } else { Some(&page) },
    ) {
        Ok(made) => made,
        Err(err) => return caught(err),
    };
    let id = py::py_str(made.get("page").unwrap_or(&Value::Null));
    let version = made.get("version").and_then(Value::as_i64).unwrap_or(0);
    let bytes = made.get("bytes").and_then(Value::as_i64).unwrap_or(0);
    tracing::info!(agent = %agent.name, page = %id, version = version, bytes = bytes, "agent page made");
    let outside: Vec<String> = match made.get("outside") {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
        _ => Vec::new(),
    };
    let warning = if outside.is_empty() {
        String::new()
    } else {
        format!(
            "\nWarning: the page asks for things on the network, which will not load (it runs offline): {}. Put them \
inline, or do without, and update the page.",
            outside.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
        )
    };
    let path = py::py_str(made.get("path").unwrap_or(&Value::Null));
    // its place in the Carnet: the page it is about; the health agent's pages stay in the private health part
    let health = crate::notebook::health_agent(config, &agent.name);
    let wanted = tk::strip(&tk::str_or_empty(arguments.get("notebook"))).to_string();
    let mut linked = String::new();
    let mut link: Option<(String, &str)> = None;
    if !wanted.is_empty() {
        let parts: Vec<crate::notebook::Part> = if health {
            vec![crate::notebook::Part::Main, crate::notebook::Part::Health]
        } else {
            vec![crate::notebook::Part::Main]
        };
        match crate::notebook::find(config, &parts, &wanted) {
            Some(mut found) if crate::notebook::KINDS.contains(&found.page.kind().as_str()) => {
                if found.page.id().is_empty() {
                    // a page without an id yet (made by hand): given one, so the link survives a rename
                    let _guard = crate::notebook::lock();
                    let before = found.page.render();
                    found.page.ensure_id();
                    let who = crate::notebook::Who::agent(config, &agent.name);
                    crate::notebook::commit(
                        config,
                        found.part,
                        &found.path,
                        Some(&before),
                        Some(&found.page.render()),
                        &who,
                        "id",
                        "",
                    )?;
                }
                linked = format!("\nLinked to the Carnet page {} ({}).", found.page.title, found.path);
                link = Some((found.page.id(), found.part.as_str()));
            }
            _ => {
                linked = format!(
                    "\nNo Carnet page « {wanted} »: the page is not linked (notebook_search finds the page; file the subject first if it has none)."
                );
            }
        }
    }
    if link.is_some() || health {
        pages::set_link(config, &agent.name, &id, link.as_ref().map(|(i, p)| (i.as_str(), *p)), health)?;
    }
    Ok(tool_text(
        &format!(
            "MEDIA:{path}\nPage {}: id {id}, version {version}.{warning}{linked}\nTo show it to the person, put the MEDIA \
line above alone on its own line in your reply, exactly as it is, where the page belongs in your text (the app shows it \
there, live): keep a sentence or two before it and the rest after.",
            if version > 1 { "updated" } else { "ready" }
        ),
        false,
    ))
}

async fn blocking<F>(work: F) -> anyhow::Result<Dict>
where
    F: FnOnce() -> anyhow::Result<Dict> + Send + 'static,
{
    tokio::task::spawn_blocking(work).await?
}

/// One tool call. Err: what Python lets through (a store that cannot be read or written): a 500.
pub async fn call_tool(
    maker: &AudioMaker,
    agent: &AgentConfig,
    name: &str,
    arguments: &Dict,
    push: Option<Arc<PushService>>,
    jobs: Option<Arc<Jobs>>,
) -> anyhow::Result<Dict> {
    if name == PROPOSE_TASK {
        return crate::jobs::propose_tool(jobs.as_deref(), agent, arguments).await;
    }
    if name == crate::jobs::START_TASK {
        return crate::jobs::start_tool(jobs.as_ref(), agent, arguments).await;
    }
    if name == crate::notebook::fill::TOOL_NAME {
        return crate::notebook::fill::tool_call(jobs.as_ref(), agent).await;
    }
    let config = maker.config.get();
    if WRITING_TOOLS.contains(&name) || name == crate::agent_files::TOOL_NAME {
        // the place full, or this agent's work paused for space: said plainly, and a card for the person
        let (c, agent_name) = (config.clone(), agent.name.clone());
        let refused = tokio::task::spawn_blocking(move || crate::storage_watch::refuse_tool(&c, &agent_name)).await?;
        if let Some(refused) = refused {
            return Ok(refused);
        }
    }
    if name == crate::storage_watch::TOOL_NAME {
        let (c, agent_name, arguments) = (config.clone(), agent.name.clone(), arguments.clone());
        return blocking(move || Ok(crate::storage_watch::call(&c, &agent_name, &arguments))).await;
    }
    if name == crate::archives::records_fill::TOOL {
        if !crate::archives::enabled(&config, &agent.name) {
            return Ok(tool_text("No archive is connected to you: the person adds one in your page of the app.", true));
        }
        return crate::archives::records_fill::tool_call(config, agent, arguments).await;
    }
    if name == crate::weather::TOOL_NAME {
        if !crate::weather::enabled(&config, &agent.name) {
            return Ok(tool_text(
                "The weather is off for you: the person switches « Météo » on in your page of the app.",
                true,
            ));
        }
        return Ok(crate::weather::call(&config, arguments).await);
    }
    if name == crate::routes::TOOL_NAME {
        return Ok(crate::routes::call(crate::weather::http(), &config, arguments).await);
    }
    if name == crate::dates::TOOL_NAME {
        let (config, agent) = (config.clone(), agent.clone());
        let arguments = arguments.clone();
        return blocking(move || Ok(crate::dates::call(&config, &agent, &arguments))).await;
    }
    if name == crate::maps::TOOL_NAME {
        return Ok(crate::maps::call(&config, &agent.name, arguments).await);
    }
    if name == crate::templates::FIND_TOOL {
        return Ok(crate::templates::find_call(&config, agent, arguments).await);
    }
    if name == crate::meet::TOOL_NAME {
        let (config, agent, arguments) = (config.clone(), agent.clone(), arguments.clone());
        return blocking(move || Ok(crate::meet::call(&config, &agent, &arguments))).await;
    }
    if crate::guide::TOOL_NAMES.contains(&name) {
        let (config, agent, name, arguments) = (config.clone(), agent.clone(), name.to_string(), arguments.clone());
        return blocking(move || crate::guide::call(&config, &agent, &name, &arguments)).await;
    }
    let (agent_c, name_c, arguments_c) = (agent.clone(), name.to_string(), arguments.clone());
    if name == crate::health::TOOL_NAME {
        return blocking(move || {
            if !crate::health::enabled(&config, &agent_c.name) {
                return Ok(tool_text(
                    "The person hasn't shared their health data with you: they switch « Santé » on in your page of \
the BipAgents app.",
                    true,
                ));
            }
            Ok(crate::health::call(&config, &agent_c.name, &arguments_c))
        })
        .await;
    }
    if PAGE_TOOLS.contains(&name) {
        return blocking(move || page_tool(&config, &agent_c, &name_c, &arguments_c)).await;
    }
    if name == crate::agent_files::TOOL_NAME {
        return blocking(move || Ok(crate::agent_files::call(&config, &agent_c, &arguments_c))).await;
    }
    if crate::archives::TOOL_NAMES.contains(&name) {
        return blocking(move || {
            if !crate::archives::enabled(&config, &agent_c.name) {
                return Ok(tool_text(
                    "No archive is connected to you: the person adds one in your page of the app.",
                    true,
                ));
            }
            crate::archives::call(&config, &agent_c.name, &name_c, &arguments_c, &mailbox::default_connect())
        })
        .await;
    }
    if mailbox::MAIL_TOOL_NAMES.contains(&name) {
        return blocking(move || {
            mailbox::call(&config, &agent_c.name, &name_c, &arguments_c, &mailbox::default_connect())
        })
        .await;
    }
    if crate::notebook::tools::TOOL_NAMES.contains(&name) {
        let result =
            blocking(move || crate::notebook::tools::call(&config, &agent_c.name, &name_c, &arguments_c)).await?;
        let reminder = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .is_some_and(|t| t.contains("phone reminders"));
        if let (Some(push), true) = (push, reminder) {
            // The person's phone woken to add the task's reminder now (a silent push; else when the app opens).
            let agent_name = agent.name.clone();
            tokio::spawn(async move {
                push.notify_silent(&agent_name, "reminders", Dict::new()).await;
            });
        }
        return Ok(result);
    }
    if KNOWLEDGE_TOOLS.contains(&name) {
        return blocking(move || knowledge_tool(&config, &agent_c, &name_c, &arguments_c)).await;
    }
    if reminders::REMINDER_TOOL_NAMES.contains(&name) {
        let result =
            blocking(move || reminders::call(&config, &agent_c.name, &name_c, &arguments_c, None, None)).await?;
        let failed = result.get("isError").map(py::truthy).unwrap_or(false);
        if let Some(push) = push {
            if name != "reminders_list" && !failed {
                // The person's iPhone woken to do it now (a silent push; else when the app opens).
                let agent_name = agent.name.clone();
                tokio::spawn(async move {
                    push.notify_silent(&agent_name, "reminders", Dict::new()).await;
                });
            }
        }
        return Ok(result);
    }
    if crate::calendar::TOOL_NAMES.contains(&name) {
        let (config_s, agent_s) = (config.clone(), agent.name.clone());
        let sources = tokio::task::spawn_blocking(move || crate::caldav::sources(&config_s, &agent_s)).await?;
        let phone = crate::calendar::enabled(&config, &agent.name);
        if let Some(done) = crate::caldav::handle(name, arguments, &sources, phone).await {
            return Ok(done);
        }
        let remote = match crate::calendar::range(arguments, None) {
            Ok((from, to)) if name == "calendar_list" => crate::caldav::listed(&sources, from, to).await,
            _ => crate::caldav::Remote::of(&sources),
        };
        let result =
            blocking(move || crate::calendar::call(&config, &agent_c.name, &name_c, &arguments_c, None, &remote))
                .await?;
        let asked = result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str)
            .is_some_and(|t| t.starts_with(crate::calendar::ASKED));
        if let Some(push) = push {
            if asked {
                // The person's phone woken to do it now (a silent push; else when the app opens).
                let agent_name = agent.name.clone();
                tokio::spawn(async move {
                    push.notify_silent(&agent_name, "calendar", Dict::new()).await;
                });
            }
        }
        return Ok(result);
    }
    if crate::contacts::TOOL_NAMES.contains(&name) {
        let result =
            blocking(move || crate::contacts::call(&config, &agent_c.name, &name_c, &arguments_c, None)).await?;
        let asked = result.get("content").and_then(|c| c.get(0)).and_then(|c| c.get("text")).and_then(Value::as_str)
            == Some(crate::contacts::ASKED);
        if let Some(push) = push {
            if asked {
                // The person's phone woken to do it now (a silent push; else when the app opens).
                let agent_name = agent.name.clone();
                tokio::spawn(async move {
                    push.notify_silent(&agent_name, "contacts", Dict::new()).await;
                });
            }
        }
        return Ok(result);
    }
    if name == LIST_VOICES {
        let lines: Vec<String> = maker
            .voices(agent)
            .await
            .iter()
            .map(|e| format!("{} ({})", tk::str_or_empty(e.get("voice")), tk::str_or_empty(e.get("kind"))))
            .collect();
        return Ok(tool_text(&lines.join("\n"), false));
    }
    if name != CREATE_AUDIO && name != CREATE_PODCAST {
        return Ok(tool_text(&format!("unknown tool: {name}"), true));
    }
    let title = tk::str_or_empty(arguments.get("title"));
    let made = if name == CREATE_PODCAST {
        let turns = match arguments.get("turns") {
            Some(Value::Array(items)) => items.clone(),
            _ => Vec::new(),
        };
        let voices = match arguments.get("voices") {
            Some(Value::Object(map)) => map.clone(),
            _ => Dict::new(),
        };
        maker.podcast(agent, &turns, &title, Some(&voices)).await
    } else {
        maker.create(agent, &tk::str_or_empty(arguments.get("text")), &title).await
    };
    let made = match made {
        Ok(made) => made,
        Err(err) => {
            if let Some(message) = err.downcast_ref::<ValueError>() {
                return Ok(tool_text(&message.0, true));
            }
            if let Some(exc) = err.downcast_ref::<TtsError>() {
                tracing::warn!(agent = %agent.name, error = %exc, "agent audio failed");
                return Ok(tk::tool_failure(
                    &format!("the voice failed ({exc}); try again later"),
                    "I couldn't make the audio right now; I'll try again in a moment.",
                ));
            }
            return Err(err);
        }
    };
    let cast: String = match made.get("cast") {
        Some(Value::Object(map)) => {
            map.iter().map(|(speaker, voice)| format!("\n- {speaker}: {}", py::py_str(voice))).collect()
        }
        _ => String::new(),
    };
    let seconds = made.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
    let voices = if cast.is_empty() { String::new() } else { format!("\nVoices:{cast}") };
    // the MEDIA line first: a shortened view of the result (a history search's) still has it (2026-10-28)
    Ok(tool_text(
        &format!(
            "MEDIA:{}\nAudio ready ({}): to give it to the person, put the MEDIA line above alone at the end of your \
reply, exactly as it is (a new file each time: never one from an earlier turn).{voices}",
            py::py_str(made.get("path").unwrap_or(&Value::Null)),
            duration(seconds),
        ),
        false,
    ))
}

fn agent_of(state: &AppState, headers: &HeaderMap) -> ApiResult<AgentConfig> {
    let header = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("");
    let token = tk::strip(header.strip_prefix("Bearer ").unwrap_or(header));
    let agent = if token.is_empty() { None } else { agent_for(&state.config(), token) };
    agent.ok_or_else(|| crate::web::HttpError::new(401, "unknown agent"))
}

fn empty(status: u16) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    response
}

/// Aborts the tool's work when the stream is dropped (the client went away).
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

const INSTRUCTIONS: &str = "BipAgents: create_audio reads a text aloud with your voice; create_podcast makes a \
conversation between hosts with different voices; create_page makes a small web page \
(a dashboard, a tracker, a chart, a tool) on any subject, shown in the app; each \
gives a MEDIA: line for your reply. Any other file you made (a spreadsheet, a document, a picture) reaches \
the person when you save it in /envoi (or /workspace when your sandbox has no /envoi) and end your reply with a \
line MEDIA:/envoi/<name>. mail_search, mail_read, mail_attachment and mail_draft reach the \
mailboxes the person connected for you (drafts only: you never send). send_files_to_agent gives files to \
another of your person's agents (it can't see your files); message_agent asks one of them and brings back its answer.";

/// The `Mcp-Session-Id` the client sent.
fn session_header(headers: &HeaderMap) -> Option<String> {
    headers.get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string)
}

/// The `v=` of the query (the tools' version when the agent's entry was written: `url_with`).
fn version_in(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| pair.strip_prefix("v=")).map(str::to_string)
}

/// The session the request names, checked against its agent: Err(404) when it is another agent's.
fn session_of(headers: &HeaderMap, query: Option<&str>, agent: &str) -> ApiResult<Option<String>> {
    let Some(id) = session_header(headers) else { return Ok(None) };
    match crate::mcp_sessions::SESSIONS.check(&id, agent, version_in(query).as_deref()) {
        crate::mcp_sessions::Known::Other => Err(crate::web::HttpError::new(404, "unknown session")),
        crate::mcp_sessions::Known::Adopted => {
            tracing::info!(agent = %agent, "an MCP session from before this bridge started taken on");
            Ok(Some(id))
        }
        crate::mcp_sessions::Known::Yes => Ok(Some(id)),
    }
}

async fn mcp(
    State(state): State<AppState>,
    uri: axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let agent = agent_of(&state, &headers)?;
    let Ok(message) = serde_json::from_slice::<Value>(&body) else {
        return Ok(crate::web::json_response(400, &rpc_error(&Value::Null, -32700, "parse error")));
    };
    let message = match message {
        Value::Object(map) if map.get("jsonrpc") == Some(&json!("2.0")) => map,
        _ => return Ok(crate::web::json_response(400, &rpc_error(&Value::Null, -32600, "invalid request"))),
    };
    let method = message.get("method").cloned().unwrap_or(Value::Null);
    let request_id = message.get("id").cloned().unwrap_or(Value::Null);
    let params = match message.get("params") {
        Some(Value::Object(map)) => map.clone(),
        // Python: `params.get` on a list or a string → AttributeError, a 500
        Some(v) if py::truthy(v) => return Err(anyhow::anyhow!("AttributeError: params has no attribute 'get'").into()),
        _ => Dict::new(),
    };
    if request_id.is_null() {
        // a notification (`notifications/initialized`…) or a response: nothing to answer
        return Ok(empty(202));
    }
    let method_name = method.as_str().unwrap_or("");
    let session = if method_name == "initialize" { None } else { session_of(&headers, uri.query(), &agent.name)? };
    if method_name == "initialize" {
        let version = match params.get("protocolVersion") {
            Some(v) if py::truthy(v) => py::py_str(v),
            _ => "2025-06-18".into(),
        };
        let result = json!({
            "protocolVersion": version,
            // the tools it is given change while it runs: told on its session's stream (mcp_sessions.rs)
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": SERVER_NAME, "version": crate::VERSION},
            "instructions": INSTRUCTIONS,
        });
        let mut response = crate::web::json_response(200, &rpc_result(&request_id, result));
        let id = crate::mcp_sessions::SESSIONS.open(&agent.name);
        if let Ok(value) = HeaderValue::from_str(&id) {
            response.headers_mut().insert("mcp-session-id", value);
        }
        return Ok(response);
    }
    if method_name == "ping" {
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, json!({}))));
    }
    if method_name == "tools/list" {
        let tools = tools_for(&state.config(), &agent);
        if let Some(id) = session.as_deref() {
            crate::mcp_sessions::SESSIONS.listed(id, &version_of(&tools));
        }
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, json!({"tools": tools}))));
    }
    if method_name != "tools/call" {
        let shown = if method.is_null() { "None".to_string() } else { py::py_str(&method) };
        return Ok(crate::web::json_response(
            200,
            &rpc_error(&request_id, -32601, &format!("method not found: {shown}")),
        ));
    }
    let name = tk::str_or_empty(params.get("name"));
    let arguments = match params.get("arguments") {
        Some(Value::Object(map)) => map.clone(),
        _ => Dict::new(),
    };
    // a deferred tool's call that the model's constrained names ended on an in-view tool (tool_diet.rs)
    let name = crate::tool_diet::reroute(&state.config(), &agent, &name, &arguments);
    if crate::family_agent::is_family(&state.config(), &agent.name) {
        // the family's agent: only the family's tools, in every turn, its creator's too (family_agent.rs)
        if let Some(closed) = crate::family_agent::closed_tool(&name) {
            tracing::info!(agent = %agent.name, tool = %name, "tool closed for the family's agent");
            return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(closed))));
        }
        if crate::family_agent::TOOL_NAMES.contains(&name.as_str()) {
            let result = crate::family_agent::call_tool(&state.config(), &agent, &name, &arguments)?;
            return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(result))));
        }
    }
    if crate::project_agent::is_project(&state.config(), &agent.name) {
        // a project agent: only its few tools, in every turn (project_agent.rs)
        if let Some(closed) = crate::project_agent::closed_tool(&name) {
            tracing::info!(agent = %agent.name, tool = %name, "tool closed for a project agent");
            return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(closed))));
        }
    }
    if crate::agent_shares::relative_turn(&state.config(), &agent.name)
        || crate::rooms_remote::guest_turn_going(&state.config(), &agent.name)
        || crate::peers::household_turn(&agent.name)
    {
        // a relative (or a relative's discussion, or another household's agent) talks to this agent: the person's own
        // data stays closed (agent_shares.rs; Hermes closes its own tools for that run, patch 0018)
        if let Some(closed) = crate::agent_shares::closed_tool(&name) {
            tracing::info!(agent = %agent.name, tool = %name, "tool closed during a relative's turn");
            return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(closed))));
        }
    } else if crate::read_shares::shared_turn(&state, &agent.name) {
        // a conversation shown to people the person invited: the same doors closed (read_shares.rs)
        if let Some(closed) = crate::read_shares::closed_tool(&name) {
            tracing::info!(agent = %agent.name, tool = %name, "tool closed in a shared conversation");
            return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(closed))));
        }
    }
    // The brake (loop_brake.rs), per turn (the run the hub follows, else the routine's): the same write twice, too many
    // writes, or a routine going round in circles, refused in plain words.
    let turn = state.hub.turn_of(&agent.name, &name);
    // the files made in this turn (audio, pages, maps), for the reply that ends it (made_files.rs)
    let turn_run = turn.as_ref().map(|t| t.run_id.clone());
    if let Some(refused) = crate::loop_brake::TURNS.check(
        &state.config().brake,
        &agent.name,
        turn.as_ref(),
        &name,
        &arguments,
        py::time_time(),
    ) {
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(refused))));
    }
    let wrote = move |agent: &str, name: &str, arguments: &Dict, result: &mut Dict| {
        if !result.get("isError").map(py::truthy).unwrap_or(false) {
            crate::loop_brake::TURNS.record(agent, turn.as_ref(), name, arguments);
        }
        // a routine's limits getting close: told with this call's result
        if let Some(note) = crate::loop_brake::TURNS.take_note(agent, turn.as_ref()) {
            crate::loop_brake::add_note(result, &note);
        }
    };
    if name == crate::agent_messages::TOOL_NAME {
        let mut result = crate::agent_messages::call(&state.http, Some(state.hub.clone()), &state.config(), &agent, &arguments).await;
        wrote(&agent.name, &name, &arguments, &mut result);
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(result))));
    }
    if crate::family_news::TOOL_NAMES.contains(&name.as_str()) {
        let mut result = crate::family_news::call_tool(&state.http, &state.config(), &agent, &name, &arguments).await?;
        wrote(&agent.name, &name, &arguments, &mut result);
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(result))));
    }
    let agent_name = agent.name.clone();
    let jobs = crate::jobs::jobs_of(&state);
    let push = state.push.clone();
    let maker = state.audio_maker.clone();
    let work = tokio::spawn(async move {
        let mut result = call_tool(&maker, &agent, &name, &arguments, Some(push), Some(jobs)).await;
        if let Ok(done) = &mut result {
            wrote(&agent.name, &name, &arguments, done);
            if crate::made_files::makes_files(&name) {
                crate::made_files::MADE.note_result(&agent.name, done, turn_run.as_deref(), py::time_time());
            }
        }
        result
    });
    let accept = headers.get("accept").and_then(|v| v.to_str().ok()).unwrap_or("");
    if !accept.contains("text/event-stream") {
        let result = work.await.map_err(anyhow::Error::from)??;
        return Ok(crate::web::json_response(200, &rpc_result(&request_id, Value::Object(result))));
    }
    let guard = AbortOnDrop(work.abort_handle());
    let tools_of = (state.clone(), agent_name.clone());
    let events = async_stream::stream! {
        let _guard = guard;
        let mut work = work;
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(KEEPALIVE_SECONDS), &mut work).await {
                Err(_) => yield Ok::<Bytes, Infallible>(Bytes::from_static(b": reading aloud\n\n")),
                Ok(Ok(Ok(result))) => {
                    // its tools changed and its session's stream may be closed: told here too, before the result
                    if let Some(id) = session.as_deref() {
                        let (state, name) = &tools_of;
                        let config = state.config();
                        if let Some(agent) = config.agent(Some(name)) {
                            if crate::mcp_sessions::SESSIONS.due(id, &crate::mcp_sessions::fingerprint(&config, agent)) {
                                tracing::info!(agent = %name, "tools changed: told with a call's result");
                                yield Ok(Bytes::from(format!("event: message\ndata: {}\n\n", crate::mcp_sessions::LIST_CHANGED)));
                            }
                        }
                    }
                    let payload = py::dumps(&rpc_result(&request_id, Value::Object(result)));
                    yield Ok(Bytes::from(format!("event: message\ndata: {payload}\n\n")));
                    break;
                }
                Ok(Ok(Err(err))) => {
                    tracing::error!(exc = %format!("{err:#}"), "Exception in ASGI application");
                    break;
                }
                Ok(Err(err)) => {
                    tracing::error!(exc = %err, "Exception in ASGI application");
                    break;
                }
            }
        }
    };
    let mut response = Response::new(Body::from_stream(events));
    response.headers_mut().insert("content-type", HeaderValue::from_static("text/event-stream; charset=utf-8"));
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// GET: the session's server stream, where `notifications/tools/list_changed` goes when the agent's tools change
/// (mcp_sessions.rs). Without a session (a client that never got one): no stream, as before.
async fn server_stream(State(state): State<AppState>, uri: axum::http::Uri, headers: HeaderMap) -> ApiResult<Response> {
    let agent = agent_of(&state, &headers)?;
    let Some(id) = session_of(&headers, uri.query(), &agent.name)? else {
        let mut response = empty(405); // no server-initiated stream
        response.headers_mut().insert("allow", HeaderValue::from_static("POST"));
        return Ok(response);
    };
    let sessions = &crate::mcp_sessions::SESSIONS;
    let Some(number) = sessions.new_stream(&id) else { return Err(crate::web::HttpError::new(404, "unknown session")) };
    let name = agent.name;
    let events = async_stream::stream! {
        let mut quiet_since = Instant::now();
        loop {
            if !sessions.serves(&id, number) {
                break; // ended (DELETE), or a newer stream of the session took over
            }
            let config = state.config();
            let Some(agent) = config.agent(Some(&name)).cloned() else { break };
            if sessions.due(&id, &crate::mcp_sessions::fingerprint(&config, &agent)) {
                tracing::info!(agent = %name, "tools changed: told on its MCP stream");
                yield Ok::<Bytes, Infallible>(Bytes::from(format!("event: message\ndata: {}\n\n", crate::mcp_sessions::LIST_CHANGED)));
                quiet_since = Instant::now();
            } else if quiet_since.elapsed().as_secs_f64() >= crate::mcp_sessions::KEEPALIVE_SECONDS {
                yield Ok(Bytes::from_static(b": keepalive\n\n"));
                quiet_since = Instant::now();
            }
            drop(config);
            sessions.wait(crate::mcp_sessions::CHECK_SECONDS).await;
        }
    };
    let mut response = Response::new(Body::from_stream(events));
    response.headers_mut().insert("content-type", HeaderValue::from_static("text/event-stream; charset=utf-8"));
    response.headers_mut().insert("cache-control", HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// DELETE: the session ends (its stream with it).
async fn end_session(State(state): State<AppState>, headers: HeaderMap) -> ApiResult<Response> {
    let agent = agent_of(&state, &headers)?;
    if let Some(id) = session_header(&headers) {
        crate::mcp_sessions::SESSIONS.end(&id, &agent.name);
    }
    Ok(empty(200))
}

/// `agent_tools_router()`: the agents' MCP tools, each agent with its own token. Mounted at the root.
pub fn router() -> Router<AppState> {
    Router::new().route(PATH, post(mcp).get(server_stream).delete(end_session))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_text_is_read_in_short_pieces_paragraph_by_paragraph() {
        let text = format!(
            "{}\n\nDeuxième partie, plus courte.\nMEDIA:/tmp/old.mp3\n",
            "Bonjour et bienvenue. ".repeat(30).trim()
        );
        let chunks = split_for_speech(&text, 120);
        assert!(chunks.iter().all(|(p, _)| p.chars().count() <= 120));
        let joined: Vec<&str> = chunks.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(joined.join(" ").matches("Bonjour et bienvenue.").count(), 30);
        assert_eq!(chunks.last().unwrap(), &("Deuxième partie, plus courte.".to_string(), PAUSE_PARAGRAPH_MS));
        assert_eq!(chunks.iter().filter(|(_, p)| *p == PAUSE_PARAGRAPH_MS).count(), 2);
        let long_one = format!("{}.", vec!["une idée assez longue"; 20].join(", "));
        assert!(split_for_speech(&long_one, 60).iter().all(|(p, _)| p.chars().count() <= 60));
    }

    #[test]
    fn python_splits_and_names() {
        assert_eq!(paragraphs("a\n \n\nb\nc\n\n"), vec!["a", "b\nc", ""]);
        assert_eq!(split_after("Un. Deux!  Trois", &['.', '!']), vec!["Un.", "Deux!", "Trois"]);
        assert_eq!(slug("Podcast du matin"), "podcast-du-matin");
        assert_eq!(slug("!!"), "audio");
        assert_eq!(duration(65.4), "1 min 05 s");
        assert_eq!(duration(2.5), "2 s");
    }

    #[test]
    fn a_wav_is_tagged_with_its_ai_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        std::fs::write(&path, crate::tts::pcm16_to_wav(&[0, 0, 1, 0], OUTPUT_RATE, 1)).unwrap();
        tag_wav(&path, "abc", "BipAgents").unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize, data.len() - 8);
        assert_eq!(&data[48..52], b"LIST");
        assert!(data.windows(4).any(|w| w == b"ICMT"));
    }
}
