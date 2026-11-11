//! An agent's browser tools, started when used (browsers.py): the MCP server Hermes keeps connected in place of the
//! one inside the browser container.
//!
//! It answers Hermes' handshake and tool list with what the real server said last time, starts the agent's browser
//! container and the real server (stealth-browser-mcp, in it) at the first call that needs them, relays everything
//! while they run, and stops both after `idle` seconds of nobody using that browser. A container stopped this way
//! costs no memory; the next call starts it again (a few seconds) with the same profile, logins included.
//!
//! Sooner when the work that used it is over (2026-10-26, a server's memory shared by its places): the bridge touches
//! the `ended` file when a run that called the browser's tools completes or fails and no other run of the agent that
//! used it still goes on (runs.rs, a background task's included); nobody using it since (`busy` older than `ended`:
//! no call, nobody watching or driving its desktop), it stops `after_run` seconds (480) after that end.
//!
//! « Using » is shared through the `busy` file, touched at each call (and every 30 s of a long one) by every copy of
//! this server (the agent's gateway, a `hermes chat` beside it), every minute by the bridge while the person watches or
//! drives that desktop, and here while something holds a connection to Chrome's DevTools (`cdp_port`: the vault,
//! Hermes' own browser tools). None stops the browser while another uses it.
//!
//!     python3 browser_mcp.py <config.json>
//!
//! `config.json` (written by browsers.setup): `start`, `ready`, `active`, `server` and `stop` (commands),
//! `cache` (the file keeping the real server's answers), `busy` (a file), `cdp_port`, `idle` (seconds). Standard
//! library only: Hermes runs it with the bridge's Python, from anywhere.
//!
//! Rust only: `shared_cache` (the answers any agent's server gave: the same image serves the same tools, so a new
//! agent's Hermes connects without starting anything), `ready_seconds` (how long its container may take to start),
//! and `unit` (`{path, text}`), `profile` and `reload`: the container's systemd unit and its Chrome profile folder,
//! written at the first start rather than with the agent (browsers.rs), then `reload` run once.
//!
//! The phone exit (phone_exit, `agent` and `exit_sock`): a page that failed because the person's phone refused or was
//! not relaying gets one plain line saying why (`WHY`), and a challenge/block page for servers gets one too, after the
//! hub asked the person (`ASK`: a card). So the agent never says « the site blocks servers » wrongly, never solves a
//! CAPTCHA, and waits for the card.
//!
//! A page too large to come back whole (token lab of 2026-11-07, GLM-5.3 Flash on the French Wikipedia page of the
//! Eiffel tower, 1.4 MB): the real server saves it to a file inside the container and answers with its path
//! (`SAVED_TO_FILE`), which none of the agent's tools reach; the agent read the page 16 times until the brake stopped
//! it. That answer is replaced by plain guidance (`too_large_text`): don't read it again, take only the part needed
//! with `execute_script` (`AROUND_A_WORD`, also in the visible-browser skill, § 2).
//!
//! The brake's rescue turn (runs.rs `loop_rescue`): while the bridge's `rescue` file (beside `busy`, browsers.rs
//! `rescue_started`) holds a deadline still ahead, page reads are refused at once (`RESCUE_REFUSED`), the real server
//! never asked: in the same lab the rescue turn read the page again, waited the full 120 s, and never answered.
//!
//! Fewer round-trips (browser-speed, 2026-11-09; Alex's case of 8/11: a test mailbox read after 3–4 minutes): every
//! extra step of the agent is a model call of ~19k tokens. `get_page_content` answers the page's readable text
//! (`readable_script`, 15,000 characters, then slices), never its HTML nor a file; `take_screenshot` answers an image
//! block (a capture saved in the container fetched from it, `exec`), never a path the agent can't read; `navigate`
//! and the clicks answer once the page is loaded (`settle`), so the agent doesn't pause on its own. Each call's time
//! is logged (`<tool>: <s> s`).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt, Shared};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt};
use tokio::sync::{oneshot, Notify};

use crate::Dict;

pub const LISTS: [&str; 4] = ["tools/list", "resources/list", "resources/templates/list", "prompts/list"];
/// A screenshot comes back as one JSON line.
pub const LINE_LIMIT: usize = 64 * 1024 * 1024;
/// How long a container may take to show its screen, by default: a cold start on a loaded server (several browsers
/// at once, memory in swap) takes far longer than the few seconds of an idle one.
pub const READY_SECONDS: u64 = 90;
pub const TCP_TABLES: [&str; 2] = ["/proc/net/tcp", "/proc/net/tcp6"];
/// How long after the end of the work that used it the browser stops, unless used again (`after_run`): 8 minutes
/// (3 until browser-speed, 2026-11-09), so the person's next message usually finds it warm, on the page it left
/// (a cold start costs the start itself, and the agent's steps to open that page again).
pub const AFTER_RUN_SECONDS: u64 = 480;

/// stealth-browser-mcp's answer when a result is too big for its client: the result saved to a file inside the
/// browser's container, its path in the answer.
pub const SAVED_TO_FILE: &str = "Response too large, automatically saved to file";

/// The script given to the agent for a page too large to come back whole (and in the visible-browser skill, § 2):
/// the page's visible text around a word, a slice of 2,000 characters at most. It only reads; `word` is the agent's.
pub const AROUND_A_WORD: &str =
    "(() => { const t = document.body.innerText; const i = t.search(/word/i); return i < 0 \
? 'not found' : t.slice(Math.max(0, i - 500), i + 1500); })()";

/// A page read refused during the brake's rescue turn.
pub const RESCUE_REFUSED: &str = "Refused: you were stopped for going round in circles, so your browser reads no page \
in this turn. Answer your person now with what you already have: what you found, what blocked you, and one or two ways \
forward.";

/// What the agent reads instead of a result saved to a file (`SAVED_TO_FILE`): its size when known, that its copy is
/// out of reach, not to read it again, and how to take only the part it needs.
pub fn too_large_text(tool: &str, size_kb: Option<f64>) -> String {
    let size = match size_kb {
        Some(kb) if kb >= 1024.0 => format!(" (about {:.1} MB)", kb / 1024.0),
        Some(kb) if kb > 0.0 => format!(" (about {kb:.0} KB)"),
        _ => String::new(),
    };
    let out_of_reach =
        "and the copy the browser saved of it is out of your reach (not in your terminal, not in your files)";
    if tool.contains("execute_script") {
        format!(
            "Your script's result is too large to come back whole{size}, {out_of_reach}. Don't run it again as it is: \
make it return only the part you need, a short slice of a few thousand characters at most, for instance the text \
around a word you look for, in place of `word`: {AROUND_A_WORD}"
        )
    } else {
        format!(
            "This page is too large to come back whole{size}, {out_of_reach}. Don't read it again: the answer would be \
the same. Take only the part you need with execute_script (through tool_call when it isn't in your tool list), for \
instance the text around a word you look for, in place of `word`: {AROUND_A_WORD}"
        )
    }
}

/// How much of a page's readable text one `get_page_content` gives (about 4,000 tokens): the rest by slices.
pub const READABLE_CHARS: usize = 15_000;
/// Links and buttons listed after a page's text, at most.
pub const READABLE_LINKS: usize = 25;

/// The script that reads a page as text (`get_page_content`, browser-speed 2026-11-09): its main content (`main`,
/// `[role=main]` or `article` when it holds at least 30 % of the page's text), else the whole body, spaces folded,
/// from `start` for `limit` characters; on the first slice, the visible links and buttons too. It only reads.
pub fn readable_script(start: usize, limit: usize) -> String {
    format!(
        "(() => {{ const f = s => (s || '').replace(/[ \\t\\u00a0]+/g, ' ').replace(/ *\\n */g, '\\n')\
.replace(/\\n{{3,}}/g, '\\n\\n').trim(); const b = document.body; const all = b ? f(b.innerText) : ''; \
const m = document.querySelector('main, [role=main], article'); const mt = m ? f(m.innerText) : ''; \
const main = mt.length >= 400 && mt.length >= all.length * 0.3; const t = main ? mt : all; const links = []; \
if ({start} === 0 && b) {{ const seen = new Set(); for (const e of b.querySelectorAll('a[href], button, \
[role=button], input[type=submit]')) {{ if (links.length >= {READABLE_LINKS}) break; if (!e.getClientRects().length) \
continue; const n = f(e.innerText || e.value || e.getAttribute('aria-label') || '').slice(0, 60); if (!n || \
seen.has(n)) continue; seen.add(n); const h = e.tagName === 'A' ? e.href : ''; links.push(h && !h.startsWith('javascript') \
? n + ' -> ' + h.slice(0, 120) : n); }} }} return JSON.stringify({{title: document.title, url: location.href, \
total: t.length, text: t.slice({start}, {end}), main, links}}); }})()",
        end = start + limit
    )
}

/// What the agent reads for `get_page_content`: the page's title and address, its readable text (`readable_script`'s
/// answer), how to read on when it was cut, and its links and buttons. None when the answer isn't the script's.
pub fn readable_text(answer: &Value, start: usize, limit: usize) -> Option<String> {
    let page = match answer {
        Value::String(s) => serde_json::from_str::<Value>(s).ok()?,
        other => other.clone(),
    };
    let text = page.get("text")?.as_str()?;
    let field = |k: &str| page.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let total = page.get("total").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut out = format!("{}\n{}\n\n", field("title"), field("url"));
    if text.is_empty() {
        out.push_str("(this page shows no text yet: it may still be loading; read it again in a few seconds)");
    } else {
        out.push_str(text);
    }
    let end = start + text.encode_utf16().count(); // JavaScript counts UTF-16 units
    if total > end {
        out.push_str(&format!(
            "\n\n[{} more characters of this page. To read on, run execute_script with: {} — or only the part around \
a word, in place of `word`: {AROUND_A_WORD}]",
            total - end,
            readable_script(end, limit)
        ));
    }
    let links: Vec<&str> = page
        .get("links")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !links.is_empty() {
        out.push_str("\n\nLinks and buttons: ");
        out.push_str(&links.join(" | "));
    }
    Some(out)
}

/// The screenshot's note beside its image; `asked`: the path the agent wanted it written to.
pub fn screenshot_note(asked: Option<&str>) -> String {
    let mut note = "The screenshot of your browser's page is attached to this answer as an image: look at it directly."
        .to_string();
    if let Some(path) = asked.filter(|p| !p.is_empty()) {
        note.push_str(&format!(
            " It was not written to {path}: your browser's files are out of your reach, so never look for it there \
and never take it again for that."
        ));
    }
    note
}

/// The real server saves a screenshot over ~60 KB to a file of this folder of its container (`clone_dir`).
pub const CLONES_IN_CONTAINER: &str = "/opt/stealth/stealth-browser-mcp/element_clones/";

/// The image a screenshot answer carries, as base64 (inline) or as a file of the container (`CLONES_IN_CONTAINER`).
#[derive(Debug, PartialEq)]
pub enum Shot {
    Inline(String),
    InContainer(String),
}

/// What the real server's `take_screenshot` answered: its image, inline or saved; None for anything else.
pub fn shot_of(response: &Dict) -> Option<Shot> {
    let result = response.get("result")?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let text = result.get("content")?.as_array()?.iter().find_map(|c| c.get("text").and_then(Value::as_str))?;
    let text = text.trim();
    if let Ok(Value::Object(saved)) = serde_json::from_str::<Value>(text) {
        let path = saved.get("file_path").and_then(Value::as_str)?;
        let ok = path.starts_with(CLONES_IN_CONTAINER) && !path.contains("..") && !path.contains('\n');
        return ok.then(|| Shot::InContainer(path.to_string()));
    }
    let base64 = text.len() > 64 && text.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b));
    base64.then(|| Shot::Inline(text.to_string()))
}

/// The image's type from its first bytes (base64 of a PNG starts « iVBOR », of a JPEG « /9j/ »).
pub fn image_type(base64: &str) -> &'static str {
    if base64.starts_with("iVBOR") {
        "image/png"
    } else if base64.starts_with("R0lG") {
        "image/gif"
    } else if base64.starts_with("UklG") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

/// A screenshot answer rewritten for the agent: the image as an MCP image block (Hermes hands it to the model, or
/// caches it as a file of its own), the note beside it; its structured copy, when it has one, keeps its shape (a
/// string or an object) with the note in place of the image or the file.
pub fn rewrite_screenshot(response: &mut Dict, base64: String, asked: Option<&str>) {
    let note = screenshot_note(asked);
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else { return };
    let mime = image_type(&base64);
    result.insert(
        "content".into(),
        json!([{"type": "image", "data": base64, "mimeType": mime}, {"type": "text", "text": note}]),
    );
    fn shaped(value: &mut Value, note: &str) {
        match value {
            Value::String(s) => *s = note.to_string(),
            Value::Object(map) if map.contains_key("file_path") || map.contains_key("reason") => {
                for key in ["file_path", "filename"] {
                    if let Some(v) = map.get_mut(key).filter(|v| v.is_string()) {
                        *v = json!("");
                    }
                }
                for key in ["reason", "message"] {
                    if let Some(v) = map.get_mut(key).filter(|v| v.is_string()) {
                        *v = json!(note);
                    }
                }
            }
            Value::Object(map) => map.values_mut().for_each(|v| shaped(v, note)),
            _ => {}
        }
    }
    if let Some(structured) = result.get_mut("structuredContent") {
        shaped(structured, &note);
    }
}

/// `execute_script`'s answer: its `result` when it succeeded (`{"success", "result", "error"}`, structured or as
/// the text of its first block).
pub fn script_result(response: &Dict) -> Option<Value> {
    let result = response.get("result")?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let answer = match result.get("structuredContent") {
        Some(Value::Object(s)) if s.contains_key("success") => Value::Object(s.clone()),
        Some(Value::Object(s)) if s.get("result").is_some_and(Value::is_object) => s["result"].clone(),
        _ => {
            let text = result.get("content")?.as_array()?.iter().find_map(|c| c.get("text").and_then(Value::as_str))?;
            serde_json::from_str::<Value>(text).ok()?
        }
    };
    (answer.get("success").and_then(Value::as_bool) == Some(true)).then(|| answer.get("result").cloned()).flatten()
}

/// Tools after which the page may be loading: the proxy waits for it before answering (`settle`).
pub fn loads_a_page(tool: &str) -> bool {
    matches!(tool, "navigate" | "click_element" | "go_back" | "go_forward" | "reload_page")
}

/// The only `spawn_browser` arguments the agent's call keeps (audit 11/11, BV-3): the window's size and what pages
/// not to load. Everything else changes how Chrome is launched, and the agent is the model a page or an e-mail may
/// steer: `browser_args` (`--no-proxy-server` displaced the egress proxy's flag, so no send guard), `proxy`,
/// `user_data_dir`, `sandbox`, `headless`, `user_agent` (a launch flag too); `timezone_id`, `idle_timeout_seconds`
/// and `extra_headers` are the container's business. The container's own flags, profile and time zone apply.
pub const SPAWN_ARGS_KEPT: [&str; 3] = ["viewport_width", "viewport_height", "block_resources"];

/// An agent's call as the real server gets it: `spawn_browser` with `SPAWN_ARGS_KEPT` only, `close_instance` without
/// `quit_browser` (quitting Chrome let the next `spawn_browser` start it again with the caller's flags; the person
/// quits the browser from the app). The names of the arguments taken off, for the log.
pub fn tame_call(tool: &str, params: &mut Value) -> Vec<String> {
    let kept: fn(&str) -> bool = match tool {
        "spawn_browser" => |k| SPAWN_ARGS_KEPT.contains(&k),
        "close_instance" => |k| k != "quit_browser",
        _ => return Vec::new(),
    };
    let Some(params) = params.as_object_mut() else { return Vec::new() };
    let arguments = params.entry("arguments").or_insert_with(|| json!({}));
    if !arguments.is_object() {
        *arguments = json!({}); // not arguments at all: none
    }
    let Some(arguments) = arguments.as_object_mut() else { return Vec::new() };
    let gone: Vec<String> = arguments.keys().filter(|k| !kept(k)).cloned().collect();
    arguments.retain(|k, _| kept(k));
    gone
}

/// The tool list as the agent reads it: the arguments `tame_call` takes off are not offered (their schema
/// properties, `required` entries and their lines in the description).
pub fn tame_tool_list(result: &mut Value) {
    let Some(tools) = result.get_mut("tools").and_then(Value::as_array_mut) else { return };
    for tool in tools {
        let kept: fn(&str) -> bool = match tool.get("name").and_then(Value::as_str) {
            Some("spawn_browser") => |k| SPAWN_ARGS_KEPT.contains(&k),
            Some("close_instance") => |k| k != "quit_browser",
            _ => continue,
        };
        let mut gone: Vec<String> = Vec::new();
        if let Some(schema) = tool.get_mut("inputSchema").and_then(Value::as_object_mut) {
            if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
                gone = properties.keys().filter(|k| !kept(k)).cloned().collect();
                properties.retain(|k, _| kept(k));
            }
            if let Some(required) = schema.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|r| r.as_str().map(kept).unwrap_or(true));
            }
        }
        if let Some(Value::String(text)) = tool.get_mut("description") {
            let lines: Vec<&str> = text
                .lines()
                .filter(|line| {
                    let line = line.trim_start();
                    !gone.iter().any(|g| line.starts_with(&format!("{g} (")) || line.contains(&format!("{g}=")))
                })
                .collect();
            *text = lines.join("\n");
        }
    }
}

/// How long `click_element` looks for its element when the agent gives no time (the real server's default: 10 s).
pub const CLICK_TIMEOUT_MS: u64 = 3000;

/// How long the proxy waits for a page to be loaded after `navigate` (and the other tools of `loads_a_page`).
pub const SETTLE_SECONDS: f64 = 8.0;

/// What the structured copy of such an answer says instead of its reason.
pub const OUT_OF_REACH: &str = "too large to come back whole; its saved copy is out of your reach";

/// The structured copy of a result saved to a file, without the file: its keys and the types of its values kept (a
/// tool with an output schema must still answer one that fits it, or Hermes' MCP client refuses the whole result:
/// « has an output schema but did not return structured content », lab of 2026-11-07), the path and the file's name
/// emptied, the reason in plain words; a text holding the answer (a wrapped result) becomes `text`.
fn without_the_file(value: &mut Value, text: &str) {
    match value {
        Value::Object(map) if map.get("reason").and_then(Value::as_str).is_some_and(|r| r.contains(SAVED_TO_FILE)) => {
            for key in ["file_path", "filename"] {
                if let Some(path) = map.get_mut(key).filter(|v| v.is_string()) {
                    *path = json!("");
                }
            }
            map.insert("reason".into(), json!(OUT_OF_REACH));
        }
        Value::Object(map) => map.values_mut().for_each(|v| without_the_file(v, text)),
        Value::Array(items) => items.iter_mut().for_each(|v| without_the_file(v, text)),
        Value::String(s) if s.contains(SAVED_TO_FILE) => *s = text.to_string(),
        _ => {}
    }
}

/// A tool's result the real server saved to a file (`SAVED_TO_FILE`), replaced by `too_large_text`; its structured
/// copy, when it has one, kept without the file (`without_the_file`). Returns whether it was.
pub fn rewrite_too_large(tool: &str, response: &mut Dict) -> bool {
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else { return false };
    let Some(Value::Array(content)) = result.get("content") else { return false };
    let saved = content
        .iter()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .find(|text| text.contains(SAVED_TO_FILE))
        .map(str::to_string);
    let Some(saved) = saved else { return false };
    let size_kb = serde_json::from_str::<Value>(&saved)
        .ok()
        .and_then(|answer| answer.get("file_size_kb").and_then(Value::as_f64));
    let text = too_large_text(tool, size_kb);
    if let Some(structured) = result.get_mut("structuredContent") {
        without_the_file(structured, &text);
    }
    result.insert("content".into(), json!([{"type": "text", "text": text}]));
    true
}

/// The real server's tools that read the page's recorded traffic, its console or the server's own files, or reach
/// Chrome raw (its network-debugging, cdp-functions, dynamic-hooks, debugging, element-extraction, file-extraction
/// and progressive-cloning sections, off in browser/mcp.sh): refused here too, should a server ever list them.
pub const NEVER_CALLED: &[&str] = &[
    "list_network_requests", "get_request_details", "get_response_details", "get_response_content",
    "search_network_requests", "export_network_data", "import_network_data", "set_network_capture_filters",
    "get_network_capture_filters", "modify_headers",
    "list_cdp_commands", "execute_cdp_command", "add_script_to_evaluate_on_new_document", "get_execution_contexts",
    "discover_global_functions", "discover_object_methods", "call_javascript_function", "inspect_function_signature",
    "inject_and_execute_script", "create_persistent_function", "execute_function_sequence", "create_python_binding",
    "execute_python_in_browser", "get_function_executor_info",
    "create_dynamic_hook", "create_simple_dynamic_hook", "list_dynamic_hooks", "get_dynamic_hook_details",
    "remove_dynamic_hook", "get_hook_documentation", "get_hook_examples", "get_hook_requirements_documentation",
    "get_hook_common_patterns", "validate_hook_function",
    "get_debug_view", "clear_debug_view", "export_debug_logs", "get_debug_lock_status", "hot_reload", "reload_status",
    "validate_browser_environment_tool",
    "extract_element_styles", "extract_element_structure", "extract_element_events", "extract_element_animations",
    "extract_element_assets", "extract_element_styles_cdp", "extract_related_files", "clone_element_complete",
    "extract_complete_element_cdp",
    "clone_element_to_file", "extract_complete_element_to_file", "extract_element_styles_to_file",
    "extract_element_structure_to_file", "extract_element_events_to_file", "extract_element_animations_to_file",
    "extract_element_assets_to_file", "list_clone_files", "cleanup_clone_files",
    "clone_element_progressive", "expand_styles", "expand_events", "expand_children", "expand_css_rules",
    "expand_pseudo_elements", "expand_animations", "list_stored_elements", "clear_stored_element", "clear_all_elements",
];

/// What the relay answers itself, never asking the real server (fix review 11/11, R5): its resources
/// (`browser://<id>/network` holds every request the page sent, a card payment's own included; `/console`, `/state`,
/// `/cookies`) are never listed nor read, its prompts never got, and the tools of `NEVER_CALLED` are refused.
/// None: relayed as usual.
pub fn never_relayed(method: &str, params: Option<&Value>) -> Option<Value> {
    let error = |text: &str| json!({"jsonrpc": "2.0", "error": {"code": -32601, "message": text}});
    match method {
        "resources/list" | "resources/templates/list" => {
            Some(json!({"jsonrpc": "2.0", "result": empty(method).unwrap_or_else(|| json!({}))}))
        }
        "resources/read" | "resources/subscribe" | "resources/unsubscribe" | "prompts/get" | "completion/complete" => {
            Some(error("not offered by the agent's browser"))
        }
        "tools/call" => {
            let tool = params.and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("");
            NEVER_CALLED.contains(&tool).then(|| {
                json!({"jsonrpc": "2.0", "result": {"isError": true, "content": [{"type": "text",
                       "text": format!("{tool} is not one of your browser's tools.")}]}})
            })
        }
        _ => None,
    }
}

/// Addresses the agent's browser never opens for it (security audit 11/11): a file of the container
/// (`file:///run/bipagents/…` holds the screen's password and the DevTools secret), a page's source, Chrome's own
/// pages and DevTools, an extension's page. Chrome's managed policy blocks them too (browser/policies.json).
pub const REFUSED_SCHEMES: [&str; 6] =
    ["file:", "view-source:", "chrome:", "devtools:", "chrome-devtools:", "chrome-extension:"];

/// A tool's call to an address of `REFUSED_SCHEMES` (its `url` argument: `navigate`, `new_tab`; Chrome drops tabs,
/// newlines and spaces from an address, so they are dropped here too): the refusal for the agent, else None.
pub fn refused_address(tool: &str, args: &Dict) -> Option<Dict> {
    let url = args.get("url").and_then(Value::as_str)?;
    let plain: String = url.chars().filter(|c| !c.is_ascii_whitespace() && !c.is_ascii_control()).collect();
    let plain = plain.to_ascii_lowercase();
    if !REFUSED_SCHEMES.iter().any(|s| plain.starts_with(s)) {
        return None;
    }
    let text = format!(
        "{tool} refused: your browser opens web pages only (https or http), never a file of its machine, a page's \
         source, nor the browser's own pages."
    );
    json!({"jsonrpc": "2.0", "result": {"isError": true, "content": [{"type": "text", "text": text}]}})
        .as_object()
        .cloned()
}

/// What Hermes asks while connecting, answered without starting anything once the real server has said it (the
/// others, never asked while it ran, as empty).
fn empty(method: &str) -> Option<Value> {
    match method {
        "resources/list" => Some(json!({"resources": []})),
        "resources/templates/list" => Some(json!({"resourceTemplates": []})),
        "prompts/list" => Some(json!({"prompts": []})),
        _ => None,
    }
}

pub fn log(text: &str) {
    let mut err = std::io::stderr();
    let _ = writeln!(err, "[browser-mcp] {text}");
    let _ = err.flush();
}

/// `json.dumps(message, separators=(",", ":"))`.
fn line(message: &Value) -> Vec<u8> {
    let mut out = crate::py::Dumps::new().compact().dumps(message).into_bytes();
    out.push(b'\n');
    out
}

fn strs(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items.iter().map(crate::py::py_str).collect(),
        _ => Vec::new(),
    }
}

/// Python's text for an `OSError` from starting a program.
fn os_error_text(err: &std::io::Error, program: &str) -> String {
    match err.raw_os_error() {
        Some(code) => {
            let text = err.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text).to_string();
            format!("[Errno {code}] {text}: {}", crate::py::py_repr(&json!(program)))
        }
        None => err.to_string(),
    }
}

/// `run(command, check)`: the command's exit code (stdout and stderr to /dev/null); `check`: a failure is an error.
pub async fn run(command: &[String], check: bool) -> Result<i32, String> {
    use std::os::unix::process::ExitStatusExt;
    let Some((program, rest)) = command.split_first() else { return Err("list index out of range".into()) };
    let status = tokio::process::Command::new(program)
        .args(rest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map_err(|e| os_error_text(&e, program))?;
    let code = status.code().unwrap_or_else(|| -status.signal().unwrap_or(0));
    if check && code != 0 {
        let name = Path::new(program).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        return Err(format!("{name} {} failed ({code})", rest.iter().take(2).cloned().collect::<Vec<_>>().join(" ")));
    }
    Ok(code)
}

/// Something on this machine holds a connection to the browser's DevTools port (Linux: /proc/net/tcp).
pub fn devtools_connected(port: Option<&Value>, tables: &[String]) -> bool {
    let Some(port) = port.filter(|p| crate::py::truthy(p)) else { return false };
    let Ok(port) = crate::py::py_int(port) else { return false };
    let wanted = format!(":{port:04X}");
    for table in tables {
        let Ok(text) = std::fs::read_to_string(table) else { continue };
        for line in crate::hermes::splitlines(&text).iter().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            // local address, remote address, state (01: established)
            if parts.len() > 3 && parts[3] == "01" && (parts[1].ends_with(&wanted) || parts[2].ends_with(&wanted)) {
                return true;
            }
        }
    }
    false
}

/// The real server while it runs.
struct Server {
    pid: Option<u32>,
    stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>,
    exited: Arc<AtomicBool>,
    exit: Arc<Notify>,
}

impl Server {
    fn running(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    async fn wait(&self) {
        while self.running() {
            let notified = self.exit.notified();
            if !self.running() {
                break;
            }
            notified.await;
        }
    }
}

type Pending = HashMap<i64, oneshot::Sender<Result<Dict, String>>>;
type Starting = Shared<BoxFuture<'static, Result<(), String>>>;

pub struct Proxy {
    pub config: Dict,
    pub idle: f64,
    /// Seconds after the end of the run that used it (the `ended` file, beside `busy` unless set); 0: never sooner.
    pub after_run: f64,
    pub ended_path: PathBuf,
    /// The brake's rescue turn: its deadline (unix seconds) while it goes on (the `rescue` file, beside `busy` unless
    /// set; browsers.rs `rescue_started`).
    pub rescue_path: PathBuf,
    pub cache_path: PathBuf,
    pub cache: Mutex<Dict>,
    output: Mutex<Box<dyn Write + Send>>,
    server: Mutex<Option<Arc<Server>>>,
    client_init: Mutex<Value>,
    pending: Arc<Mutex<Pending>>,
    next_id: AtomicI64,
    /// tool calls under way
    pub calls: AtomicI64,
    starting: Mutex<Option<Starting>>,
    /// starting and stopping the container, one at a time
    switching: tokio::sync::Mutex<()>,
    /// /proc/net/tcp and tcp6 (tests give their own)
    pub tcp_tables: Mutex<Vec<String>>,
    /// The last URL navigated to (the page a later read is about), for the phone exit's lines.
    last_url: Mutex<String>,
}

impl Proxy {
    pub fn new(config: Dict, output: Box<dyn Write + Send>) -> Arc<Proxy> {
        let idle = config.get("idle").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(600.0);
        let after_run =
            config.get("after_run").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(AFTER_RUN_SECONDS as f64);
        let busy = PathBuf::from(config.get("busy").map(crate::py::py_str).unwrap_or_default());
        let ended_path = match config.get("ended").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => busy.parent().map(|p| p.join("ended")).unwrap_or_else(|| PathBuf::from("ended")),
        };
        let rescue_path = match config.get("rescue").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => busy.parent().map(|p| p.join("rescue")).unwrap_or_else(|| PathBuf::from("rescue")),
        };
        let cache_path = PathBuf::from(config.get("cache").map(crate::py::py_str).unwrap_or_default());
        let mut cache = read_cache(&cache_path);
        if !cache.contains_key("initialize") {
            // never started yet: what another agent's server said (the same image), so connecting starts nothing
            if let Some(shared) = config.get("shared_cache").map(crate::py::py_str).filter(|p| !p.is_empty()) {
                cache = read_cache(Path::new(&shared));
            }
        }
        Arc::new(Proxy {
            config,
            idle,
            after_run,
            ended_path,
            rescue_path,
            cache_path,
            cache: Mutex::new(cache),
            output: Mutex::new(output),
            server: Mutex::new(None),
            client_init: Mutex::new(json!({})),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            calls: AtomicI64::new(0),
            starting: Mutex::new(None),
            switching: tokio::sync::Mutex::new(()),
            tcp_tables: Mutex::new(TCP_TABLES.iter().map(|s| s.to_string()).collect()),
            last_url: Mutex::new(String::new()),
        })
    }

    fn command(&self, name: &str) -> Vec<String> {
        strs(self.config.get(name))
    }

    // -- the two sides -------------------------------------------------------------------------------------------

    pub fn to_client(&self, message: &Value) {
        let mut out = self.output.lock().unwrap_or_else(|e| e.into_inner());
        let _ = out.write_all(&line(message));
        let _ = out.flush();
    }

    async fn to_server(&self, message: &Value) -> Result<(), String> {
        let server = self.server.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(server) = server else { return Err(String::new()) }; // AssertionError
        let mut stdin = server.stdin.lock().await;
        let Some(pipe) = stdin.as_mut() else { return Err(String::new()) };
        pipe.write_all(&line(message)).await.map_err(|e| e.to_string())?;
        pipe.flush().await.map_err(|e| e.to_string())
    }

    /// A request to the real server; its whole response (`result` or `error`).
    async fn request(&self, method: &str, params: Option<&Value>) -> Result<Dict, String> {
        let number = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(number, tx);
        let mut message = json!({"jsonrpc": "2.0", "id": number, "method": method});
        if let Some(params) = params.filter(|p| !p.is_null()) {
            message["params"] = params.clone();
        }
        self.to_server(&message).await?;
        rx.await.unwrap_or_else(|_| Err("its server stopped".into()))
    }

    // -- Hermes' messages ----------------------------------------------------------------------------------------

    pub async fn handle(self: &Arc<Self>, message: Value) {
        let Value::Object(message) = message else { return };
        let ident = message.get("id").cloned().unwrap_or(Value::Null);
        let method = match message.get("method") {
            None | Some(Value::Null) => {
                // Hermes answering a request of the real server (ids of the server's own numbering).
                if self.running() {
                    let _ = self.to_server(&Value::Object(message)).await;
                }
                return;
            }
            Some(Value::String(m)) => m.clone(),
            Some(_) => return,
        };
        if method == "initialize" {
            *self.client_init.lock().unwrap_or_else(|e| e.into_inner()) =
                message.get("params").filter(|p| crate::py::truthy(p)).cloned().unwrap_or_else(|| json!({}));
            let result: Result<(), String> = async {
                if !self.cache_has("initialize") {
                    self.ensure_server().await?; // the first time ever: what it serves is learned once
                }
                let init = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get("initialize").cloned();
                let init = init.ok_or_else(|| "'initialize'".to_string())?;
                self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": init}));
                Ok(())
            }
            .await;
            if let Err(exc) = result {
                self.fail(&ident, &exc); // told to Hermes, which retries later
            }
            return;
        }
        if method == "notifications/initialized" {
            return; // the real server gets its own after its handshake
        }
        if method == "ping" {
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": {}}));
            return;
        }
        if method.starts_with("notifications/") {
            if self.running() {
                let _ = self.to_server(&Value::Object(message)).await;
            }
            return;
        }
        if let Some(answer) = never_relayed(&method, message.get("params")) {
            // the server's recorded traffic, console and state, and the tools that read them (`never_relayed`)
            log(&format!("{method}: not relayed"));
            let mut answer = answer;
            answer["id"] = ident;
            self.to_client(&answer);
            return;
        }
        if LISTS.contains(&method.as_str()) && !self.running() && (self.cache_has(&method) || empty(&method).is_some())
        {
            let cached = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&method).cloned();
            let mut result = cached.or_else(|| empty(&method)).unwrap_or(Value::Null);
            if method == "tools/list" {
                tame_tool_list(&mut result); // a list cached before the rule too
            }
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": result}));
            return;
        }
        let calling = method == "tools/call";
        if calling && self.credit_used() {
            // The month's AI credit used (credit.rs): the browser isn't started, the agent told plainly (chat goes on).
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": {
                "content": [{"type": "text", "text": crate::credit::BROWSER_REFUSED}], "isError": true}}));
            return;
        }
        let tool = message.get("params").and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("").to_string();
        if calling && crate::loop_brake::page_read_tool(&tool) && self.rescuing() {
            // The brake's rescue turn: answered with what the agent has, never a page it couldn't read (nor a hang).
            log(&format!("{tool} refused: the brake's rescue turn"));
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident, "result": {
                "content": [{"type": "text", "text": RESCUE_REFUSED}], "isError": true}}));
            return;
        }
        if calling {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.touch();
        }
        let started = crate::py::monotonic();
        let result: Result<(), String> = async {
            self.ensure_server().await?;
            let mut response = if calling {
                self.call_tool(&tool, message.get("params")).await?
            } else {
                self.request(&method, message.get("params")).await?
            };
            if method == "tools/list" {
                if let Some(result) = response.get_mut("result") {
                    tame_tool_list(result);
                }
            }
            if LISTS.contains(&method.as_str()) {
                if let Some(result) = response.get("result").cloned() {
                    self.remember(&method, result);
                }
            }
            response.insert("id".into(), ident.clone());
            if calling {
                if rewrite_too_large(&tool, &mut response) {
                    log(&format!("{tool}: a result too large, plain guidance instead of its file"));
                }
                self.phone_exit_lines(message.get("params"), &mut response).await;
            }
            self.to_client(&Value::Object(response));
            Ok(())
        }
        .await;
        if let Err(exc) = result {
            self.fail(&ident, &exc);
        }
        if calling {
            // one line per call with its time (the browser-speed measures read them in the gateway's log)
            log(&format!("{tool}: {:.2} s", crate::py::monotonic() - started));
            self.calls.fetch_sub(1, Ordering::SeqCst);
            self.touch();
        }
    }

    /// A tool call, as the real server answers it, except (browser-speed, 2026-11-09):
    /// - `get_page_content`: the page's readable text (`readable_script`) in place of its HTML and text, which came
    ///   back as tens of thousands of tokens, or as a file out of reach for most real pages; the real tool when the
    ///   script can't run (frames asked for, no such instance);
    /// - `take_screenshot`: the image as an image block, never base64 text nor a file of the container (fetched from
    ///   it), whatever path the agent asked;
    /// - `navigate` and the other tools of `loads_a_page`: answered once the page is loaded (`settle`), so the agent
    ///   reads it at once instead of pausing; `networkidle` (the real server's blind 2 s) waits for that too;
    ///   `click_element` looks for its element 3 s, not 10, unless the agent gives a time (`CLICK_TIMEOUT_MS`).
    async fn call_tool(&self, tool: &str, params: Option<&Value>) -> Result<Dict, String> {
        // never the agent's launch flags, proxy or profile (`tame_call`)
        let mut tamed = params.cloned();
        let gone = tamed.as_mut().map(|p| tame_call(tool, p)).unwrap_or_default();
        if !gone.is_empty() {
            log(&format!("{tool}: not passed on: {}", gone.join(", ")));
        }
        let params = tamed.as_ref();
        let args = params.and_then(|p| p.get("arguments")).and_then(Value::as_object).cloned().unwrap_or_default();
        let instance = args.get("instance_id").and_then(Value::as_str).unwrap_or("").to_string();

        if let Some(refused) = refused_address(tool, &args) {
            log(&format!("{tool}: an address of the container itself refused"));
            return Ok(refused);
        }
        if tool == "get_page_content"
            && !instance.is_empty()
            && !crate::py::truthy(args.get("include_frames").unwrap_or(&Value::Null))
        {
            if let Some(answer) = self.script(&instance, &readable_script(0, READABLE_CHARS)).await {
                if let Some(text) = readable_text(&answer, 0, READABLE_CHARS) {
                    let page = match &answer {
                        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
                        other => other.clone(),
                    };
                    let field = |k: &str| page.get(k).cloned().unwrap_or(json!(""));
                    return Ok(json!({"jsonrpc": "2.0", "result": {
                        "content": [{"type": "text", "text": text}],
                        "structuredContent": {"html": "", "text": "(the page's readable text is in this answer)",
                                              "url": field("url"), "title": field("title")}}})
                    .as_object()
                    .cloned()
                    .unwrap_or_default());
                }
            }
        }
        if tool == "take_screenshot" {
            let mut args = args.clone();
            let asked = args.remove("file_path").and_then(|p| p.as_str().map(str::to_string));
            args.insert("format".into(), json!("jpeg"));
            let mut sent = params.cloned().unwrap_or_else(|| json!({}));
            sent["arguments"] = Value::Object(args);
            let mut response = self.request("tools/call", Some(&sent)).await?;
            let image = match shot_of(&response) {
                Some(Shot::Inline(data)) => Some(data),
                Some(Shot::InContainer(path)) => self.fetch_from_container(&path).await,
                None => None,
            };
            if let Some(image) = image {
                rewrite_screenshot(&mut response, image, asked.as_deref());
            }
            return Ok(response);
        }
        let mut sent = params.cloned();
        if tool == "navigate" && args.get("wait_until").and_then(Value::as_str) == Some("networkidle") {
            if let Some(p) = sent.as_mut() {
                p["arguments"]["wait_until"] = json!("domcontentloaded");
            }
        }
        if tool == "click_element" && !args.contains_key("timeout") {
            // the page is loaded already (`settle`): an element still missing after 3 s is not coming (the real
            // server's own 10 s were the lab's slowest step, 9/11)
            if let Some(Value::Object(arguments)) = sent.as_mut().and_then(|p| p.get_mut("arguments")) {
                arguments.insert("timeout".into(), json!(CLICK_TIMEOUT_MS));
            }
        }
        let response = self.request("tools/call", sent.as_ref()).await?;
        if loads_a_page(tool) && !instance.is_empty() {
            let failed = response.contains_key("error")
                || response.get("result").and_then(|r| r.get("isError")).and_then(Value::as_bool) == Some(true);
            if !failed {
                self.settle(&instance, tool != "navigate").await;
            }
        }
        Ok(response)
    }

    /// A script run in the page through the real server's `execute_script`: its result, None when it failed.
    async fn script(&self, instance: &str, script: &str) -> Option<Value> {
        let params = json!({"name": "execute_script", "arguments": {"instance_id": instance, "script": script}});
        let response = self.request("tools/call", Some(&params)).await.ok()?;
        script_result(&response)
    }

    /// Waits until the page is loaded (`document.readyState` « complete »), `SETTLE_SECONDS` at most; after a click,
    /// first a moment for the navigation it may start.
    async fn settle(&self, instance: &str, after_click: bool) {
        let started = crate::py::monotonic();
        if after_click {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        loop {
            match self.script(instance, "document.readyState").await {
                Some(Value::String(state)) if state == "complete" => break,
                None => break, // no page to ask (an instance gone): nothing to wait for
                _ => {}
            }
            if crate::py::monotonic() - started > SETTLE_SECONDS {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        log(&format!("page settled in {:.2} s", crate::py::monotonic() - started));
    }

    /// A file the real server saved in its container (`CLONES_IN_CONTAINER`), as base64, then removed there: through
    /// the config's `exec` (the command prefix that runs one in the container; `[]`: on this machine), else the one
    /// `server` gives (`podman exec -i <container> …`).
    async fn fetch_from_container(&self, path: &str) -> Option<String> {
        use base64::Engine;
        let prefix: Vec<String> = match self.config.get("exec") {
            Some(v) => strs(Some(v)),
            None => {
                let server = self.command("server");
                let i = server.iter().position(|a| a == "exec")?;
                let container = server.iter().skip(i + 1).find(|a| !a.starts_with('-'))?;
                vec![server[0].clone(), "exec".into(), container.clone()]
            }
        };
        let mut cat = prefix.clone();
        cat.extend(["cat".to_string(), path.to_string()]);
        let (program, rest) = cat.split_first()?;
        let output =
            tokio::process::Command::new(program).args(rest).stderr(std::process::Stdio::null()).output().await;
        let mut rm = prefix;
        rm.extend(["rm".to_string(), "-f".to_string(), path.to_string()]);
        let _ = run(&rm, false).await;
        match output {
            Ok(out) if out.status.success() && !out.stdout.is_empty() => {
                Some(base64::engine::general_purpose::STANDARD.encode(out.stdout))
            }
            _ => {
                log("a screenshot saved in the container could not be read");
                None
            }
        }
    }

    /// The hub's socket and the agent's name: this config's, else its file read again (one written before the phone
    /// exit was switched on gets them at the bridge's next start, phone_exit `serve`, while this process goes on).
    fn exit_keys(&self) -> (String, String) {
        let pick = |c: &Dict| {
            let text = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (text("exit_sock"), text("agent"))
        };
        let (sock, agent) = pick(&self.config);
        if !sock.is_empty() {
            return (sock, agent);
        }
        let Some(path) = self.config.get("config_path").and_then(Value::as_str) else { return Default::default() };
        match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            Some(Value::Object(file)) => pick(&file),
            _ => Default::default(),
        }
    }

    /// The phone exit's lines added to a tool's result: why the person's phone refused this page's site lately, or
    /// what was done about a challenge page (a card for the person). Nothing when the phone exit is off here.
    async fn phone_exit_lines(&self, params: Option<&Value>, response: &mut Dict) {
        let (sock, agent) = self.exit_keys();
        if sock.is_empty() || agent.is_empty() {
            return;
        }
        let url = params.and_then(|p| p.get("arguments")).and_then(|a| a.get("url")).and_then(Value::as_str);
        let url = match url {
            Some(u) if u.starts_with("http") => {
                *self.last_url.lock().unwrap_or_else(|e| e.into_inner()) = u.to_string();
                u.to_string()
            }
            _ => self.last_url.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        };
        let host = crate::phone_exit::checks::host_of(&url);
        if host.is_empty() || crate::phone_exit::name_refused(&host).is_some() {
            return;
        }
        let site = crate::mail_tidy::registrable(&host);
        let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else { return };
        let text = result.get("content").map(Value::to_string).unwrap_or_default();
        let mut lines = Vec::new();
        if let Some(answer) = ask_hub(&sock, &format!("WHY {agent} {host}")).await {
            let mut words = answer.split_whitespace();
            if let (Some(code), Some(ago)) = (words.next(), words.next().and_then(|a| a.parse::<u64>().ok())) {
                if code != "NONE" && ago <= 60 {
                    lines.push(crate::phone_exit::agent::refusal_text(code, &site));
                }
            }
        }
        if lines.is_empty() {
            if let Some(kind) = crate::phone_exit::agent::challenge_kind(&text) {
                if let Some(answer) = ask_hub(&sock, &format!("ASK {agent} {host} {kind}")).await {
                    lines.push(crate::phone_exit::agent::challenge_text(kind, &site, &answer));
                }
            }
        }
        if lines.is_empty() {
            return;
        }
        if let Some(Value::Array(content)) = result.get_mut("content") {
            for line in lines {
                content.push(json!({"type": "text", "text": line}));
            }
        }
    }

    /// The brake's rescue turn goes on for this agent: the `rescue` file holds a deadline still ahead (a file left by a
    /// bridge that stopped meanwhile refuses nothing once past it).
    pub fn rescuing(&self) -> bool {
        std::fs::read_to_string(&self.rescue_path)
            .ok()
            .and_then(|text| text.trim().parse::<f64>().ok())
            .is_some_and(|deadline| deadline > crate::py::time_time())
    }

    /// The plan's AI credit used: the config's `credit` file (tests), else the place's own (credit.rs).
    fn credit_used(&self) -> bool {
        let file = self.config.get("credit").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
        crate::credit::now_at(file.as_deref()).paused()
    }

    fn cache_has(&self, key: &str) -> bool {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).contains_key(key)
    }

    fn fail(&self, ident: &Value, exc: &str) {
        if !ident.is_null() {
            self.to_client(&json!({"jsonrpc": "2.0", "id": ident,
                                   "error": {"code": -32603, "message": format!(
                                       "the agent's browser could not start: {exc}\nTell your person simply, in \
their language: « I can't open web pages right now; I'll try again in a moment. »")}}));
        }
    }

    pub fn remember(&self, key: &str, value: Value) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let own = read_cache(&self.cache_path);
        if cache.get(key) == Some(&value) && own.get(key) == Some(&value) {
            return;
        }
        cache.insert(key.to_string(), value);
        let mut paths = vec![self.cache_path.clone()];
        if let Some(shared) = self.config.get("shared_cache").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            paths.push(PathBuf::from(shared));
        }
        for path in paths {
            if let Err(exc) = write_cache(&path, &cache) {
                log(&format!("cache not saved: {exc}"));
            }
        }
    }

    // -- the real server -----------------------------------------------------------------------------------------

    pub fn running(&self) -> bool {
        self.server.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|s| s.running()).unwrap_or(false)
    }

    pub async fn ensure_server(self: &Arc<Self>) -> Result<(), String> {
        if self.running() {
            return Ok(());
        }
        let starting = {
            let mut slot = self.starting.lock().unwrap_or_else(|e| e.into_inner());
            match slot.as_ref().filter(|s| s.peek().is_none()) {
                Some(s) => s.clone(),
                None => {
                    let this = self.clone();
                    let task = tokio::spawn(async move { this.start().await });
                    let shared: Starting =
                        async move { task.await.unwrap_or_else(|e| Err(e.to_string())) }.boxed().shared();
                    *slot = Some(shared.clone());
                    shared
                }
            }
        };
        starting.await
    }

    async fn start(self: Arc<Self>) -> Result<(), String> {
        let _switching = self.switching.lock().await;
        if self.running() {
            return Ok(());
        }
        let result = self.start_server().await;
        if result.is_err() {
            self.stop_child().await;
        }
        result
    }

    async fn start_server(self: &Arc<Self>) -> Result<(), String> {
        let started = crate::py::monotonic();
        if write_unit(&self.config)? {
            // its first use: the container's unit was only written now
            run(&self.command("reload"), true).await?;
            log("container set up");
        }
        run(&self.command("start"), true).await?;
        let patience =
            self.config.get("ready_seconds").and_then(|v| crate::py::py_float(v).ok()).unwrap_or(READY_SECONDS as f64);
        loop {
            match run(&self.command("ready"), false).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if crate::py::monotonic() - started > patience {
                return Err(format!("its container did not start within {patience:.0} s"));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let command = self.command("server");
        let Some((program, rest)) = command.split_first() else { return Err("list index out of range".into()) };
        let mut child = tokio::process::Command::new(program)
            .args(rest)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| os_error_text(&e, program))?;
        let stdout = child.stdout.take();
        let server = Arc::new(Server {
            pid: child.id(),
            stdin: Arc::new(tokio::sync::Mutex::new(child.stdin.take())),
            exited: Arc::new(AtomicBool::new(false)),
            exit: Arc::new(Notify::new()),
        });
        let (exited, exit) = (server.exited.clone(), server.exit.clone());
        tokio::spawn(async move {
            let _ = child.wait().await;
            exited.store(true, Ordering::SeqCst);
            exit.notify_waiters();
        });
        *self.server.lock().unwrap_or_else(|e| e.into_inner()) = Some(server.clone());
        if let Some(stdout) = stdout {
            tokio::spawn(read_server(self.clone(), server, stdout));
        }
        let client_init = self.client_init.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let params = if crate::py::truthy(&client_init) {
            client_init
        } else {
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "bipagents", "version": "1"}})
        };
        let init = self.request("initialize", Some(&params)).await?;
        let Some(result) = init.get("result").cloned() else {
            let error = init.get("error").cloned().unwrap_or(Value::Null);
            return Err(format!("its server refused to start: {}", crate::py::py_repr(&error)));
        };
        self.remember("initialize", result);
        self.to_server(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
        let tools = self.request("tools/list", Some(&json!({}))).await?;
        if let Some(result) = tools.get("result").cloned() {
            self.remember("tools/list", result);
        }
        self.touch();
        log(&format!("started in {:.1} s", crate::py::monotonic() - started));
        Ok(())
    }

    pub async fn stop(&self) {
        let _switching = self.switching.lock().await;
        self.stop_child().await;
        let _ = run(&self.command("stop"), false).await;
    }

    pub async fn stop_child(&self) {
        let server = self.server.lock().unwrap_or_else(|e| e.into_inner()).take();
        let Some(server) = server else { return };
        if !server.running() {
            return;
        }
        server.stdin.lock().await.take(); // closed
        if tokio::time::timeout(Duration::from_secs(5), server.wait()).await.is_err() {
            if let Some(pid) = server.pid {
                // SAFETY: kill(2) on the child this proxy started.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGKILL);
                }
            }
        }
    }

    pub fn touch(&self) {
        let busy = PathBuf::from(self.config.get("busy").map(crate::py::py_str).unwrap_or_default());
        let touch = |p: &Path| -> std::io::Result<()> {
            let file = std::fs::OpenOptions::new().create(true).append(true).open(p)?;
            file.set_modified(std::time::SystemTime::now())
        };
        match touch(&busy) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if let Some(parent) = busy.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let _ = touch(&busy);
            }
            _ => {}
        }
    }

    pub fn unused_for(&self) -> f64 {
        let busy = PathBuf::from(self.config.get("busy").map(crate::py::py_str).unwrap_or_default());
        match std::fs::metadata(&busy).and_then(|m| m.modified()) {
            Ok(mtime) => {
                let mtime = mtime.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
                crate::py::time_time() - mtime
            }
            Err(_) => f64::INFINITY,
        }
    }

    /// The work that used it ended `after_run` seconds ago or more, and nobody used it since.
    pub fn done_with(&self) -> bool {
        if self.after_run <= 0.0 {
            return false;
        }
        let Ok(ended) = std::fs::metadata(&self.ended_path).and_then(|m| m.modified()) else { return false };
        let ended = ended.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let now = crate::py::time_time();
        // `busy` touched after the end: used again (a call, the person watching or driving its desktop)
        now - ended >= self.after_run && self.unused_for() >= now - ended
    }

    pub async fn watch_idle(self: Arc<Self>, every: f64) {
        loop {
            tokio::time::sleep(Duration::from_secs_f64(every)).await;
            let tables = self.tcp_tables.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if self.calls.load(Ordering::SeqCst) != 0 || devtools_connected(self.config.get("cdp_port"), &tables) {
                if self.unused_for() > 30.0 {
                    self.touch();
                }
            } else if (self.unused_for() >= self.idle || self.done_with())
                && (self.running() || run(&self.command("active"), false).await.map(|c| c == 0).unwrap_or(false))
            {
                // Also when another started it (the desktop in the app, a `hermes chat` since closed).
                if self.unused_for() >= self.idle {
                    log(&format!("unused for {:.0} s: stopped", self.idle));
                } else {
                    log(&format!("its work over for {:.0} s: stopped", self.after_run));
                }
                self.stop().await;
            }
        }
    }

    pub async fn serve<R: AsyncRead + Unpin>(self: &Arc<Self>, reader: R) {
        let idle = (self.idle > 0.0).then(|| tokio::spawn(self.clone().watch_idle(15.0)));
        let mut reader = tokio::io::BufReader::new(reader);
        let mut tasks = Vec::new();
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(message) = serde_json::from_slice::<Value>(&buf) else { continue };
            let this = self.clone();
            tasks.push(tokio::spawn(async move { this.handle(message).await }));
            tasks.retain(|t| !t.is_finished());
        }
        if !tasks.is_empty() {
            // what it asked last, answered before leaving
            let _ = tokio::time::timeout(Duration::from_secs(30), futures::future::join_all(tasks)).await;
        }
        if let Some(idle) = idle {
            idle.abort();
        }
        // Hermes gone: its server too. The browser stays for whoever else uses it, until it is unused.
        self.stop_child().await;
    }
}

/// One line to the phone exit's hub and its one-line answer (None: no hub, or no answer within 3 s).
async fn ask_hub(sock: &str, line: &str) -> Option<String> {
    let talk = async {
        let mut conn = tokio::net::UnixStream::connect(sock).await.ok()?;
        conn.write_all(format!("{line}\n").as_bytes()).await.ok()?;
        let mut reader = tokio::io::BufReader::new(conn);
        let mut answer = String::new();
        reader.read_line(&mut answer).await.ok()?;
        Some(answer.trim().to_string())
    };
    tokio::time::timeout(Duration::from_secs(3), talk).await.ok().flatten()
}

fn read_cache(path: &Path) -> Dict {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
        Some(Value::Object(map)) => map,
        _ => Dict::new(),
    }
}

fn write_cache(path: &Path, cache: &Dict) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // a name of its own: two agents' proxies may write the shared one at once
    let partial = path.with_extension(format!("part{}", std::process::id()));
    std::fs::write(&partial, crate::py::dumps(&Value::Object(cache.clone())))?;
    std::fs::rename(&partial, path)
}

/// The container's systemd unit (and its Chrome profile folder), written if it is not there yet: an agent's browser
/// is set up at its first use, not with the agent. Returns whether it was written (`reload` is then due).
pub fn write_unit(config: &Dict) -> Result<bool, String> {
    let Some(unit) = config.get("unit").and_then(Value::as_object) else { return Ok(false) };
    let path = PathBuf::from(unit.get("path").map(crate::py::py_str).unwrap_or_default());
    if path.as_os_str().is_empty() || path.exists() {
        return Ok(false);
    }
    let text = unit.get("text").map(crate::py::py_str).unwrap_or_default();
    let written: std::io::Result<()> = (|| {
        if let Some(profile) = config.get("profile").map(crate::py::py_str).filter(|p| !p.is_empty()) {
            std::fs::create_dir_all(&profile)?;
            // the downloads policy the unit mounts (browsers.rs), in case the bridge's setup has not written it
            if let Some(place) = Path::new(&profile).parent() {
                crate::browsers::write_downloads_policy(place)?;
            }
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("part");
        std::fs::write(&partial, text)?;
        std::fs::rename(&partial, &path)
    })();
    written.map(|_| true).map_err(|e| format!("its container could not be set up: {e}"))
}

async fn read_server(proxy: Arc<Proxy>, server: Arc<Server>, stdout: tokio::process::ChildStdout) {
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(Value::Object(message)) = serde_json::from_slice::<Value>(&buf) else { continue };
        if message.contains_key("method") {
            proxy.to_client(&Value::Object(message)); // its notifications and requests go to Hermes as they are
            continue;
        }
        let id = message
            .get("id")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)));
        if let Some(id) = id {
            let sender = proxy.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            if let Some(sender) = sender {
                let _ = sender.send(Ok(message));
            }
        }
    }
    server.wait().await;
    let pending: Vec<_> = proxy.pending.lock().unwrap_or_else(|e| e.into_inner()).drain().collect();
    for (_, sender) in pending {
        let _ = sender.send(Err("its server stopped".into()));
    }
}

/// `bip-browser-mcp <config.json>` (Python: `python3 browser_mcp.py <config.json>`): the process exit code.
pub fn main(args: Vec<String>) -> i32 {
    let Some(path) = args.first() else {
        eprintln!("usage: bip-browser-mcp <config.json>");
        return 1;
    };
    let mut config = match std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()))
    {
        Ok(Value::Object(map)) => map,
        Ok(_) => {
            eprintln!("{path}: not a JSON object");
            return 1;
        }
        Err(e) => {
            eprintln!("{path}: {e}");
            return 1;
        }
    };
    config.insert("config_path".into(), json!(path)); // read again for the phone exit's keys (`exit_keys`)
    // Hermes starts MCP servers with few variables: `systemctl --user` and rootless podman find the session here.
    // SAFETY: getuid cannot fail.
    let runtime_dir = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() && runtime_dir.is_dir() {
        std::env::set_var("XDG_RUNTIME_DIR", &runtime_dir);
    }
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    runtime.block_on(async move {
        let proxy = Proxy::new(config, Box::new(std::io::stdout()));
        proxy.serve(tokio::io::stdin()).await;
    });
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devtools_connections_are_read_from_the_tcp_tables() {
        let dir = tempfile::tempdir().unwrap();
        let table = dir.path().join("tcp");
        let header =
            "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        // 127.0.0.1:47000 -> 127.0.0.1:19602 (0x4C92), established
        std::fs::write(
            &table,
            format!("{header}   0: 0100007F:B798 0100007F:4C92 01 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
        )
        .unwrap();
        let tables = vec![table.to_string_lossy().into_owned()];
        assert!(devtools_connected(Some(&json!(19602)), &tables));
        assert!(!devtools_connected(Some(&json!(19604)), &tables));
        std::fs::write(
            &table,
            format!("{header}   0: 0100007F:B798 0100007F:4C92 06 00000000:00000000 00:00000000 00000000  1000 0 1\n"),
        )
        .unwrap();
        assert!(!devtools_connected(Some(&json!(19602)), &tables)); // TIME_WAIT: gone
        assert!(!devtools_connected(None, &tables));
    }

    /// stealth-browser-mcp's answer for the French Wikipedia page of the Eiffel tower (token lab of 2026-11-07).
    fn saved_answer() -> Value {
        json!({"file_path": "/opt/stealth/stealth-browser-mcp/element_clones/page_content_20261107.json",
               "filename": "page_content_20261107.json", "file_size_kb": 1399.12,
               "estimated_tokens": 353880, "reason": SAVED_TO_FILE,
               "metadata": {"instance_id": "319ceedb", "include_frames": false}})
    }

    /// Its tool's output schema asks for structured content: the answer itself (as the lab's get_page_content).
    fn saved_to_file() -> Dict {
        let answer = saved_answer();
        let message = json!({"jsonrpc": "2.0", "id": 7, "result": {
            "content": [{"type": "text", "text": answer.to_string()}], "structuredContent": answer}});
        message.as_object().unwrap().clone()
    }

    #[test]
    fn a_page_too_large_to_come_back_is_said_plainly_never_its_file() {
        let mut response = saved_to_file();
        assert!(rewrite_too_large("get_page_content", &mut response));
        let result = response["result"].as_object().unwrap();
        // the structured copy still fits the tool's output schema (same keys, same types), without the file
        let structured = result["structuredContent"].as_object().unwrap();
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(&result["structuredContent"]), keys(&saved_answer()));
        assert_eq!((structured["file_path"].clone(), structured["filename"].clone()), (json!(""), json!("")));
        assert_eq!(structured["reason"], json!(OUT_OF_REACH));
        assert_eq!(structured["file_size_kb"], json!(1399.12));
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        let text = content[0]["text"].as_str().unwrap();
        let head = "This page is too large to come back whole (about 1.4 MB), and the copy the browser saved of it is \
out of your reach";
        assert!(text.starts_with(head), "{text}");
        assert!(text.contains("Don't read it again") && text.contains("execute_script") && text.contains("tool_call"));
        assert!(text.ends_with(AROUND_A_WORD));
        assert!(!Value::Object(response.clone()).to_string().contains("element_clones"), "its path never shown");
        assert_eq!(response["id"], json!(7));
        // read again: rewritten again, the same words
        let mut again = saved_to_file();
        assert!(rewrite_too_large("get_page_content", &mut again));
        assert_eq!(again["result"], response["result"]);
        // a script that returned too much: told to return less
        let mut script = saved_to_file();
        assert!(rewrite_too_large("execute_script", &mut script));
        let text = script["result"]["content"][0]["text"].as_str().unwrap().to_string();
        assert!(text.starts_with("Your script's result is too large to come back whole (about 1.4 MB)"), "{text}");
        assert!(text.contains("Don't run it again as it is") && text.ends_with(AROUND_A_WORD));
        // a wrapped result (`{"result": …}`), as the answer or as its text: the same, inside
        for wrapped in [json!({"result": saved_answer()}), json!({"result": saved_answer().to_string()})] {
            let mut response = saved_to_file();
            response["result"]["structuredContent"] = wrapped.clone();
            assert!(rewrite_too_large("get_page_content", &mut response));
            let inside = &response["result"]["structuredContent"]["result"];
            assert!(!Value::Object(response.clone()).to_string().contains("element_clones"), "{response:?}");
            assert_eq!(inside.is_string(), wrapped["result"].is_string());
            if let Some(text) = inside.as_str() {
                assert_eq!(text, response["result"]["content"][0]["text"].as_str().unwrap());
            } else {
                assert_eq!(inside["reason"], json!(OUT_OF_REACH));
            }
        }
        // the size as it comes, or none
        assert!(too_large_text("take_screenshot", Some(312.4)).contains("whole (about 312 KB), and"));
        assert!(too_large_text("get_page_content", None).starts_with("This page is too large to come back whole, and"));
    }

    #[test]
    fn other_answers_are_left_as_they_are() {
        for message in [
            json!({"jsonrpc": "2.0", "id": 1, "result": {"content": [{"type": "text", "text": "{\"title\": \"Tour\"}"}]}}),
            json!({"jsonrpc": "2.0", "id": 2, "result": {"content": [{"type": "image", "data": "iVBOR"}]}}),
            json!({"jsonrpc": "2.0", "id": 3, "error": {"code": -32603, "message": "boom"}}),
            json!({"jsonrpc": "2.0", "id": 4, "result": {"structuredContent": {"reason": SAVED_TO_FILE}}}),
        ] {
            let mut response = message.as_object().unwrap().clone();
            assert!(!rewrite_too_large("get_page_content", &mut response));
            assert_eq!(Value::Object(response), message);
        }
    }

    #[test]
    fn a_page_is_read_as_its_readable_text_with_the_way_to_read_on() {
        let script = readable_script(0, READABLE_CHARS);
        assert!(script.contains("t.slice(0, 15000)") && script.contains("main, [role=main], article"));
        assert!(script.contains("0 === 0 && b"), "links on the first slice only");
        assert!(readable_script(15000, 15000).contains("t.slice(15000, 30000)"));
        assert!(!script.contains("innerHTML") && !script.contains(".value ="), "it only reads");
        let page = json!({"title": "Tour Eiffel — Wikipédia", "url": "https://fr.wikipedia.org/wiki/Tour_Eiffel",
                          "total": 98000, "text": "La tour Eiffel est inaugurée en 1889.", "main": true,
                          "links": ["Accueil -> https://fr.wikipedia.org/", "Rechercher"]});
        // execute_script gives the script's JSON as a string
        let text = readable_text(&json!(page.to_string()), 0, READABLE_CHARS).unwrap();
        assert!(
            text.starts_with("Tour Eiffel — Wikipédia\nhttps://fr.wikipedia.org/wiki/Tour_Eiffel\n\nLa tour Eiffel")
        );
        let rest = 98000 - "La tour Eiffel est inaugurée en 1889.".encode_utf16().count();
        assert!(text.contains(&format!("[{rest} more characters of this page. To read on, run execute_script with: ")));
        assert!(text.contains(&readable_script(98000 - rest, READABLE_CHARS)) && text.contains(AROUND_A_WORD));
        assert!(text.ends_with("Links and buttons: Accueil -> https://fr.wikipedia.org/ | Rechercher"), "{text}");
        // a short page: nothing more to read, no links
        let short =
            json!({"title": "Connexion", "url": "https://a.test/", "total": 11, "text": "Identifiant", "links": []});
        assert_eq!(readable_text(&short, 0, READABLE_CHARS).unwrap(), "Connexion\nhttps://a.test/\n\nIdentifiant");
        let empty = json!({"title": "", "url": "https://a.test/", "total": 0, "text": ""});
        assert!(readable_text(&empty, 0, READABLE_CHARS).unwrap().contains("may still be loading"));
        assert_eq!(readable_text(&json!("did execute_script"), 0, 10), None);
    }

    #[test]
    fn a_script_s_result_is_read_structured_or_from_its_text() {
        let answer = json!({"success": true, "result": "complete", "error": null});
        let structured = json!({"result": {"content": [{"type": "text", "text": answer.to_string()}],
                                           "structuredContent": answer}});
        assert_eq!(script_result(structured.as_object().unwrap()), Some(json!("complete")));
        let text_only = json!({"result": {"content": [{"type": "text", "text": answer.to_string()}]}});
        assert_eq!(script_result(text_only.as_object().unwrap()), Some(json!("complete")));
        let wrapped = json!({"result": {"content": [], "structuredContent": {"result": answer}}});
        assert_eq!(script_result(wrapped.as_object().unwrap()), Some(json!("complete")));
        let failed = json!({"success": false, "result": null, "error": "Instance not found"});
        let failed = json!({"result": {"content": [{"type": "text", "text": failed.to_string()}]}});
        assert_eq!(script_result(failed.as_object().unwrap()), None);
        let error = json!({"result": {"isError": true, "content": [{"type": "text", "text": "boom"}]}});
        assert_eq!(script_result(error.as_object().unwrap()), None);
    }

    #[test]
    fn a_screenshot_comes_back_as_an_image_never_a_path() {
        let jpeg = format!("/9j/{}", "A".repeat(200));
        let inline = json!({"result": {"content": [{"type": "text", "text": jpeg}],
                                       "structuredContent": {"result": jpeg}}});
        let mut response = inline.as_object().unwrap().clone();
        assert_eq!(shot_of(&response), Some(Shot::Inline(jpeg.clone())));
        rewrite_screenshot(&mut response, jpeg.clone(), Some("/tmp/shot.png"));
        let content = response["result"]["content"].as_array().unwrap();
        assert_eq!(content[0], json!({"type": "image", "data": jpeg, "mimeType": "image/jpeg"}));
        let note = content[1]["text"].as_str().unwrap();
        assert!(note.contains("attached to this answer as an image") && note.contains("not written to /tmp/shot.png"));
        assert_eq!(response["result"]["structuredContent"], json!({"result": note}), "its shape kept, no base64");
        // too large for the real server: saved in its container (fetched by the proxy), its path never shown
        let saved = json!({"file_path": format!("{CLONES_IN_CONTAINER}screenshot_1_319ceedb.jpeg"),
                           "filename": "screenshot_1_319ceedb.jpeg", "file_size_kb": 180.2, "estimated_tokens": 61000,
                           "reason": "Screenshot too large, automatically saved to file",
                           "message": "Screenshot saved. AI agents should use the Read tool"});
        let big = json!({"result": {"content": [{"type": "text", "text": saved.to_string()}],
                                    "structuredContent": {"result": saved}}});
        let mut response = big.as_object().unwrap().clone();
        assert_eq!(
            shot_of(&response),
            Some(Shot::InContainer(format!("{CLONES_IN_CONTAINER}screenshot_1_319ceedb.jpeg")))
        );
        rewrite_screenshot(&mut response, "iVBORw0KGgo".into(), None);
        assert_eq!(response["result"]["content"][0]["mimeType"], "image/png");
        assert!(!Value::Object(response.clone()).to_string().contains("element_clones"));
        assert_eq!(response["result"]["structuredContent"]["result"]["file_size_kb"], json!(180.2));
        // anything else is left alone: an error, a path elsewhere, a plain line
        for other in [
            json!({"result": {"isError": true, "content": [{"type": "text", "text": jpeg}]}}),
            json!({"result": {"content": [{"type": "text", "text": "{\"file_path\": \"/etc/shadow\"}"}]}}),
            json!({"result": {"content": [{"type": "text", "text": format!("{{\"file_path\": \"{CLONES_IN_CONTAINER}../x\"}}")}]}}),
            json!({"result": {"content": [{"type": "text", "text": "Instance not found"}]}}),
        ] {
            assert_eq!(shot_of(other.as_object().unwrap()), None, "{other}");
        }
    }

    #[test]
    fn the_skill_gives_the_same_script() {
        let skill = crate::browsers::skill();
        assert!(skill.contains(&format!("`{AROUND_A_WORD}`")), "visible-browser/SKILL.md § 2 and AROUND_A_WORD");
        assert!(skill.contains("version: 1.11.1"));
        // never an argument the relay takes off (tame_call): the agent would say it quit the browser, and it did not
        assert!(!skill.contains("quit_browser:") && !skill.contains("browser_args"), "visible-browser/SKILL.md");
    }

    #[test]
    fn a_rescue_turn_is_read_from_its_file_until_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let busy = dir.path().join("busy").to_string_lossy().into_owned();
        let proxy = Proxy::new(json!({"busy": busy, "idle": 600}).as_object().unwrap().clone(), Box::new(Vec::new()));
        assert_eq!(proxy.rescue_path, dir.path().join("rescue"), "beside `busy` unless set");
        assert!(!proxy.rescuing(), "no file: no rescue turn");
        let now = crate::py::time_time();
        std::fs::write(&proxy.rescue_path, format!("{:.0}\n", now + 180.0)).unwrap();
        assert!(proxy.rescuing());
        // a bridge that stopped meanwhile left it: past its deadline, nothing refused
        std::fs::write(&proxy.rescue_path, format!("{:.0}\n", now - 1.0)).unwrap();
        assert!(!proxy.rescuing());
        std::fs::write(&proxy.rescue_path, "soon").unwrap();
        assert!(!proxy.rescuing());
        // its own path when set
        let other = dir.path().join("elsewhere");
        let set = json!({"busy": busy, "rescue": other.to_string_lossy()});
        assert_eq!(Proxy::new(set.as_object().unwrap().clone(), Box::new(Vec::new())).rescue_path, other);
    }

    /// What the relay writes to Hermes.
    #[derive(Clone, Default)]
    struct Written(Arc<Mutex<Vec<u8>>>);

    impl Write for Written {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_servers_traffic_console_and_raw_tools_never_reach_the_agent() {
        // Fix review 11/11, R5: the real server records every request the page sends (a card payment's own among
        // them) and serves it as the resource browser://<id>/network, outside any tool section. The relay never lists
        // nor reads its resources, nor gets its prompts, nor calls the tools that read the traffic, the console or
        // reach Chrome raw: answered here, the server never asked (nor started).
        let dir = tempfile::tempdir().unwrap();
        let busy = dir.path().join("busy").to_string_lossy().into_owned();
        let out = Written::default();
        let config = json!({"busy": busy, "idle": 600, "start": ["false"], "server": ["false"]});
        let proxy = Proxy::new(config.as_object().unwrap().clone(), Box::new(out.clone()));
        let asks = [
            ("resources/list", json!({})),
            ("resources/templates/list", json!({})),
            ("resources/read", json!({"uri": "browser://abc/network"})),
            ("resources/read", json!({"uri": "browser://abc/console"})),
            ("prompts/get", json!({"name": "x"})),
            ("tools/call", json!({"name": "list_network_requests", "arguments": {"instance_id": "abc"}})),
            ("tools/call", json!({"name": "get_response_content", "arguments": {}})),
            ("tools/call", json!({"name": "get_debug_view", "arguments": {}})),
            ("tools/call", json!({"name": "execute_cdp_command", "arguments": {}})),
            ("tools/call", json!({"name": "add_script_to_evaluate_on_new_document", "arguments": {}})),
            ("tools/call", json!({"name": "list_clone_files", "arguments": {}})),
        ];
        for (n, (method, params)) in asks.iter().enumerate() {
            proxy.handle(json!({"jsonrpc": "2.0", "id": n, "method": method, "params": params})).await;
        }
        let data = out.0.lock().unwrap().clone();
        let answers: Vec<Value> =
            String::from_utf8(data).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(answers.len(), asks.len());
        assert_eq!(answers[0]["result"], json!({"resources": []}));
        assert_eq!(answers[1]["result"], json!({"resourceTemplates": []}));
        for answer in &answers[2..5] {
            assert!(answer.get("result").is_none() && answer["error"]["code"] == json!(-32601), "{answer}");
        }
        for (n, answer) in answers.iter().enumerate().skip(5) {
            assert_eq!(answer["id"], json!(n));
            assert_eq!(answer["result"]["isError"], json!(true), "{answer}");
        }
        assert!(!proxy.running(), "the real server was never started");
        // An ordinary tool is still the server's (refused here only because this test's server can't start).
        assert!(never_relayed("tools/call", Some(&json!({"name": "take_screenshot"}))).is_none());
        assert!(never_relayed("tools/call", Some(&json!({"name": "execute_script"}))).is_none());
        assert!(never_relayed("tools/list", None).is_none() && never_relayed("prompts/list", None).is_none());
    }

    #[test]
    fn the_browser_never_opens_a_file_of_its_container_nor_its_own_pages() {
        // Security audit 11/11: navigate("file:///run/bipagents/vnc.pass") then get_page_content read the screen's
        // password (and the DevTools secret beside it).
        let args = |url: &str| json!({"instance_id": "abc", "url": url}).as_object().unwrap().clone();
        for url in [
            "file:///run/bipagents/vnc.pass",
            "FILE:///run/bipagents/cdp.token",
            "  fi\tle:///etc/passwd",
            "view-source:https://shop.example/",
            "chrome://extensions",
            "chrome://inspect/#devices",
            "devtools://devtools/bundled/inspector.html",
            "chrome-devtools://devtools/x",
            "chrome-extension://abcdef/page.html",
        ] {
            let refused = refused_address("navigate", &args(url)).unwrap_or_else(|| panic!("{url} not refused"));
            assert_eq!(refused["result"]["isError"], json!(true));
            assert!(refused["result"]["content"][0]["text"].as_str().unwrap().starts_with("navigate refused"));
        }
        assert!(refused_address("new_tab", &args("file:///run/bipagents/vnc.pass")).is_some());
        for url in ["https://shop.example/file:///x", "http://localhost:8080/", "about:blank", "https://chrome.com/"] {
            assert!(refused_address("navigate", &args(url)).is_none(), "{url}");
        }
        assert!(refused_address("click_element", json!({"selector": "a"}).as_object().unwrap()).is_none());
    }
}
