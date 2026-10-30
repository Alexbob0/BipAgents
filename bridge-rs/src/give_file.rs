//! Files an agent makes for its person, given where the person can get them (Alex, 2026-10-30: agents said « c'est
//! disponible dans ton dossier » about a file saved on the server, which the person can't open).
//!
//! `give_file(path, title?)` (MCP, agent_tools.rs, behind Hermes' tool search) takes a file the agent sees (its
//! `/envoi`, `/workspace`, `/root`, the files it was sent… as `MEDIA:` lines do, sent_files.rs) and:
//!
//! - when a cloud drive is connected for this agent (« Se connecter avec… », connections.rs: a catalog service with a
//!   `drive` entry, connected, this agent among its agents; the first in the catalog's order), uploads it into a
//!   « BipAgents » folder there through the service's own MCP server, with the access token the bridge keeps, and
//!   gives back the link, and a ready `MEDIA:` line when the file may also go in the conversation;
//! - otherwise gives back the `MEDIA:` line for the reply (the file kept by the bridge now, sent_files.rs `capture`),
//!   the way the one-to-one and room replies deliver files;
//! - a file over the conversation's limit (`[limits] upload_max_bytes`) goes to the drive only (up to
//!   `DRIVE_MAX_BYTES`); without a drive, a plain sentence that it is too big.
//!
//! The family's agent and the project agents get the tool too, never with the person's drive: what they make is for a
//! family or friends, so it goes in the conversation. In a relative's turn the tool is closed (agent_shares.rs), like
//! the person's other files. The upload is no new approval: it is the person's own drive, which they connected and
//! gave this agent, and the file is one the agent made for them.
//!
//! No drive service is in the catalog yet (2026-10-30: Notion and Finary only): until one is added there, every file
//! goes in the conversation.
//!
//! No Python counterpart.

use std::io::Read;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::connections::{Connection, Drive, Service, Stored};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "give_file";
/// The folder the files go to in the person's drive.
pub const FOLDER: &str = "BipAgents";
/// The largest file put in a drive (sent whole, in base64, in one MCP call).
pub const DRIVE_MAX_BYTES: u64 = 100 * 1024 * 1024;
const TITLE_MAX: usize = 80;
const HTTP_TIMEOUT: Duration = Duration::from_secs(180);
const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Give your person a file you made (a document, a spreadsheet, a PDF, a picture, a sound): it goes \
    to their cloud drive when one is connected (you get the link), else into the conversation (you get a MEDIA: line). \
    Put what it gives you in your reply.",
        "inputSchema": {"type": "object", "properties": {
            "path": {"type": "string", "description": "the file as you see it, e.g. /envoi/devis.pdf"},
            "title": {"type": "string", "description": "optional: the file's name for your person", "maxLength": TITLE_MAX},
        }, "required": ["path"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": false},
    })
}

/// Whether `agent` may put files in the person's drive: not the family's agent nor a project agent (shared).
fn may_use_drive(config: &Config, agent: &str) -> bool {
    !crate::family_agent::is_family(config, agent) && !crate::project_agent::is_project(config, agent)
}

/// The drive `agent`'s files go to: the first service of `catalog` with a drive, connected, that the person gave
/// this agent.
pub fn drive_for<'a>(catalog: &'a [Service], stored: &Stored, agent: &str) -> Option<(&'a Service, Connection)> {
    catalog.iter().filter(|svc| svc.drive.is_some()).find_map(|svc| {
        let conn = stored.connections.get(&svc.id)?;
        (conn.state == "connected" && conn.agents.iter().any(|a| a == agent)).then(|| (svc, conn.clone()))
    })
}

/// The name the file gets in the drive: the title (its extension added when it has none), else the file's own name.
pub fn drive_name(title: &str, path: &str) -> String {
    let own = path.rsplit('/').next().unwrap_or(path).to_string();
    let clean: String = title
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(TITLE_MAX)
        .collect();
    let clean = clean.trim_matches('.').trim().to_string();
    if clean.is_empty() {
        return own;
    }
    let ext = own.rsplit_once('.').filter(|(stem, _)| !stem.is_empty()).map(|(_, e)| e);
    match ext {
        Some(ext) if !clean.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) => format!("{clean}.{ext}"),
        _ => clean,
    }
}

/// The file as found: where the agent sees it, its size, its bytes when they go to a drive, and whether it may go in
/// the conversation (kept by the bridge for its `MEDIA:` line).
struct Found {
    at: String,
    size: u64,
    bytes: Option<Vec<u8>>,
    media: Result<(), String>,
}

enum NotFound {
    TooBig,
    Missing(String),
}

/// The agent's file behind `path` (blocking: files, and `podman cp` for a file only in its sandbox).
fn find(config: &Config, agent: &AgentConfig, path: &str, for_drive: bool) -> Result<Found, NotFound> {
    let limit = config.limits.upload_max_bytes;
    let (at, file) = match crate::sent_files::open_own(config, agent, path) {
        Ok(found) => found,
        Err(here) => match crate::sent_files::capture(config, agent, path) {
            // only in its sandbox: copied out (at most the conversation's limit)
            Ok(copy) => match std::fs::File::open(&copy) {
                Ok(file) => (path.to_string(), file),
                Err(e) => return Err(NotFound::Missing(e.to_string())),
            },
            Err(why) if why.starts_with("too large") => return Err(NotFound::TooBig),
            Err(why) => return Err(NotFound::Missing(format!("{here}; {why}"))),
        },
    };
    let size = file.metadata().map(|m| m.len()).map_err(|e| NotFound::Missing(e.to_string()))?;
    let media = if size <= limit {
        crate::sent_files::capture(config, agent, &at).map(|_| ())
    } else {
        Err("too large".into())
    };
    let bytes = if for_drive && size <= DRIVE_MAX_BYTES {
        let mut bytes = Vec::with_capacity(size as usize);
        file.take(DRIVE_MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| NotFound::Missing(e.to_string()))?;
        Some(bytes)
    } else {
        None
    };
    Ok(Found { at, size, bytes, media })
}

fn megabytes(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// One call, for `agent`.
pub async fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    call_with(config, agent, arguments, crate::connections::catalog()).await
}

/// `call` with the services' catalog given (tests: a drive service).
pub async fn call_with(config: &Config, agent: &AgentConfig, arguments: &Dict, catalog: &[Service]) -> Dict {
    let raw = arguments.get("path").map(crate::py::py_str).unwrap_or_default();
    let path = raw.trim().trim_start_matches("MEDIA:").trim();
    let path = path.strip_prefix("file://").unwrap_or(path).to_string();
    if path.is_empty() {
        return tool_text("Give `path`: the file as you see it, e.g. /envoi/devis.pdf.", true);
    }
    let title = arguments.get("title").map(crate::py::py_str).unwrap_or_default();
    let drive_allowed = may_use_drive(config, &agent.name);
    let drive = if drive_allowed {
        let (c, name) = (config.clone(), agent.name.clone());
        let stored = tokio::task::spawn_blocking(move || crate::connections::store_of(&c).read().map(|s| (s, name)))
            .await
            .ok()
            .and_then(Result::ok);
        stored.and_then(|(stored, name)| drive_for(catalog, &stored, &name))
    } else {
        None
    };
    let (c, a, p, for_drive) = (config.clone(), agent.clone(), path.clone(), drive.is_some());
    let found = match tokio::task::spawn_blocking(move || find(&c, &a, &p, for_drive)).await {
        Ok(found) => found,
        Err(e) => Err(NotFound::Missing(e.to_string())),
    };
    let limit = config.limits.upload_max_bytes;
    let too_big = |max: u64| {
        let can_connect = drive_allowed && drive.is_none() && catalog.iter().any(|s| s.drive.is_some());
        tool_failure(
            &format!("{path} is too large to send in the conversation (at most {} MB).", megabytes(max)),
            if can_connect {
                "This file is too big to send you here: connect your cloud drive in the app and I'll put it there."
            } else {
                "This file is too big to send you here."
            },
        )
    };
    let found = match found {
        Ok(found) => found,
        Err(NotFound::TooBig) => return too_big(limit),
        Err(NotFound::Missing(why)) => {
            return tool_text(
                &format!(
                    "{path}: no file I can give ({why}). Save the file you made in /envoi (or /workspace) and give \
that path."
                ),
                true,
            )
        }
    };
    let media_line = format!("MEDIA:{}", found.at);
    if let Some((svc, conn)) = &drive {
        let label = svc.label();
        match &found.bytes {
            Some(bytes) => {
                let name = drive_name(&title, &found.at);
                let drive = svc.drive.as_ref().expect("a drive service");
                match upload(&conn.server.mcp_url, &conn.tokens.access_token, drive, &name, bytes).await {
                    Ok(link) => {
                        tracing::info!(agent = %agent.name, service = %svc.id, bytes = found.size, "file put in the person's drive");
                        let also = if found.media.is_ok() {
                            format!(
                                "\nIf they'd rather have it in the conversation too, add this line alone in your reply:\n{media_line}"
                            )
                        } else {
                            String::new()
                        };
                        return tool_text(
                            &format!(
                                "Put in your person's {label}, folder « {FOLDER} », as « {name} »: {link}\nGive them \
this link in your reply and say it is in their {label}, folder {FOLDER}.{also}"
                            ),
                            false,
                        );
                    }
                    Err(why) => {
                        tracing::warn!(agent = %agent.name, service = %svc.id, error = %why, "file not put in the drive");
                        if found.media.is_err() {
                            return tool_failure(
                                &format!("{label} refused the file ({why}), and it is too large for the conversation."),
                                &format!("I couldn't put this file in your {label}; I'll try again."),
                            );
                        }
                    }
                }
            }
            None => return too_big(DRIVE_MAX_BYTES),
        }
    }
    match found.media {
        Ok(()) => tool_text(
            &format!(
                "{media_line}\nPut the line above alone on its own line in your reply: your person gets the file in \
the app. Never tell them it is in a folder."
            ),
            false,
        ),
        Err(_) if found.size > limit => too_big(limit),
        Err(why) => tool_failure(
            &format!("{path} can't be sent in the conversation ({why})."),
            "I can't send you this kind of file here.",
        ),
    }
}

// -- the drive's MCP server ---------------------------------------------------------------------------------------------

/// `value` with each `{key}` of its strings replaced.
fn fill(value: &Value, vars: &[(&str, &str)]) -> Value {
    match value {
        Value::String(s) => {
            let mut out = s.clone();
            for (key, with) in vars {
                let mark = format!("{{{key}}}");
                if out.contains(&mark) {
                    out = out.replace(&mark, with);
                }
            }
            Value::String(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|v| fill(v, vars)).collect()),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), fill(v, vars))).collect()),
        other => other.clone(),
    }
}

/// The JSON-RPC answer with `id` in a streamable HTTP response: one JSON message (or a batch), or an event stream.
pub fn answer_of(content_type: &str, body: &str, id: u64) -> Option<Value> {
    let pick = |v: Value| -> Option<Value> {
        match v {
            Value::Array(items) => items.into_iter().find(|m| m.get("id").and_then(Value::as_u64) == Some(id)),
            m if m.get("id").and_then(Value::as_u64) == Some(id) => Some(m),
            _ => None,
        }
    };
    if content_type.contains("text/event-stream") {
        let body = body.replace("\r\n", "\n");
        return body.split("\n\n").find_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            serde_json::from_str(&data.join("\n")).ok().and_then(pick)
        });
    }
    serde_json::from_str(body).ok().and_then(pick)
}

/// The link in a `tools/call` result: `structuredContent[link]`, else the first web address of its text.
pub fn link_of(result: &Value, link: &str) -> Option<String> {
    let web = |s: &str| s.starts_with("https://") || s.starts_with("http://");
    if !link.is_empty() {
        if let Some(found) = result.get("structuredContent").and_then(|s| s.get(link)).and_then(Value::as_str) {
            return web(found).then(|| found.to_string());
        }
    }
    let texts = result.get("content").and_then(Value::as_array).into_iter().flatten();
    texts.filter_map(|c| c.get("text").and_then(Value::as_str)).find_map(|text| {
        text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']'))
            .find(|w| web(w))
            .map(|w| w.trim_end_matches(['.', ',', ';']).to_string())
    })
}

/// One JSON-RPC message to the server: the session it gave (on `initialize`) and the answer (None for a notification).
async fn post(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    session: Option<&str>,
    body: &Value,
) -> Result<(Option<String>, Option<Value>), String> {
    let mut request = http
        .post(url)
        .bearer_auth(token)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", PROTOCOL_VERSION)
        .json(body);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.map_err(|e| format!("not reached: {e}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {}", status.as_u16()));
    }
    let session = response.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(str::to_string);
    let kind = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let Some(id) = body.get("id").and_then(Value::as_u64) else { return Ok((session, None)) };
    let text = response.text().await.map_err(|e| e.to_string())?;
    let answer = answer_of(&kind, &text, id).ok_or("no answer")?;
    if let Some(error) = answer.get("error") {
        return Err(error.get("message").and_then(Value::as_str).unwrap_or("error").to_string());
    }
    Ok((session, answer.get("result").cloned()))
}

/// The file put in the drive with its MCP tool: Ok(link).
pub async fn upload(url: &str, token: &str, drive: &Drive, name: &str, bytes: &[u8]) -> Result<String, String> {
    let http = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("BipAgents-bridge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
        "clientInfo": {"name": "BipAgents", "version": env!("CARGO_PKG_VERSION")}}});
    let (session, _) = post(&http, url, token, None, &init).await?;
    let session = session.as_deref();
    post(&http, url, token, session, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await?;
    let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();
    let content = base64::engine::general_purpose::STANDARD.encode(bytes);
    let vars = [("name", name), ("folder", FOLDER), ("mime_type", mime.as_str()), ("content_base64", content.as_str())];
    let arguments = Value::Object(drive.arguments.iter().map(|(k, v)| (k.clone(), fill(v, &vars))).collect());
    drop(content);
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                      "params": {"name": drive.tool, "arguments": arguments}});
    let (_, result) = post(&http, url, token, session, &call).await?;
    let result = result.ok_or("no result")?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(result["content"][0]["text"].as_str().unwrap_or("refused").chars().take(200).collect());
    }
    link_of(&result, &drive.link).ok_or_else(|| "no link in its answer".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use crate::connections::{Server, Tokens};

    type Calls = Arc<Mutex<Vec<(Option<String>, Value)>>>;

    fn setup() -> (tempfile::TempDir, Config, AgentConfig) {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::with_key("k");
        config.outbox.db_path = dir.path().join("data/outbox.db").to_string_lossy().into_owned();
        config.limits.upload_max_bytes = 1000;
        let agent = AgentConfig { name: "wellness".into(), shares: "external".into(), ..AgentConfig::default() };
        config.agents.insert(agent.name.clone(), agent.clone());
        let outbox = crate::agent_files::external_outbox(&config, "wellness");
        std::fs::create_dir_all(&outbox).unwrap();
        std::fs::write(outbox.join("devis.pdf"), b"%PDF-1.7 devis").unwrap();
        std::fs::write(outbox.join("film.mp4"), vec![0u8; 2000]).unwrap();
        (dir, config, agent)
    }

    fn args(v: Value) -> Dict {
        serde_json::from_value(v).unwrap()
    }

    fn text(d: &Dict) -> String {
        d["content"][0]["text"].as_str().unwrap().to_string()
    }

    fn drive_catalog() -> Vec<Service> {
        serde_yaml::from_str(
            r#"
- id: notion
  url: https://mcp.notion.com/mcp
  issuer: https://mcp.notion.com
  texts: {fr: {label: Notion, description: x}}
- id: testdrive
  url: https://drive.example/mcp
  issuer: https://drive.example
  texts: {fr: {label: Test Drive, description: x}}
  drive:
    tool: upload_file
    link: webViewLink
    arguments: {name: "{name}", parent: "{folder}", mimeType: "{mime_type}", content: "{content_base64}"}
"#,
        )
        .unwrap()
    }

    /// A drive's MCP server: initialize (a session), the notification, then tools/call answered as an event stream.
    async fn drive_server(calls: Calls) -> String {
        use axum::body::Body;
        use axum::http::HeaderMap;
        use axum::response::Response;
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(move |headers: HeaderMap, body: axum::Json<Value>| {
                let calls = calls.clone();
                async move {
                    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
                    calls.lock().unwrap().push((header("authorization"), body.0.clone()));
                    let id = body.0.get("id").cloned();
                    match body.0["method"].as_str().unwrap_or("") {
                        "initialize" => Response::builder()
                            .header("content-type", "application/json")
                            .header("mcp-session-id", "s-1")
                            .body(Body::from(
                                json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": PROTOCOL_VERSION}})
                                    .to_string(),
                            ))
                            .unwrap(),
                        "tools/call" if header("mcp-session-id").as_deref() == Some("s-1") => {
                            let result = json!({"jsonrpc": "2.0", "id": id, "result": {
                                "content": [{"type": "text", "text": "uploaded"}],
                                "structuredContent": {"webViewLink": "https://drive.example/f/42"}}});
                            Response::builder()
                                .header("content-type", "text/event-stream")
                                .body(Body::from(format!("event: message\ndata: {result}\n\n")))
                                .unwrap()
                        }
                        _ => Response::builder().status(202).body(Body::empty()).unwrap(),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/mcp")
    }

    fn connect(config: &Config, url: &str, agents: &[&str]) {
        let conn = Connection {
            service: "testdrive".into(),
            server: Server { mcp_url: url.into(), ..Server::default() },
            tokens: Tokens { access_token: "at-1".into(), ..Tokens::default() },
            agents: agents.iter().map(|a| a.to_string()).collect(),
            state: "connected".into(),
            ..Connection::default()
        };
        let mut stored = Stored::default();
        stored.connections.insert("testdrive".into(), conn);
        crate::connections::store_of(config).write(&stored).unwrap();
    }

    #[tokio::test]
    async fn no_drive_a_media_line() {
        let (_dir, config, agent) = setup();
        let done = call_with(&config, &agent, &args(json!({"path": "/envoi/devis.pdf"})), &drive_catalog()).await;
        assert_eq!(done["isError"], false);
        assert!(text(&done).starts_with("MEDIA:/envoi/devis.pdf\n"), "{}", text(&done));
        // kept by the bridge: the app gets it even once the sandbox is reset
        let outbox = crate::agent_files::external_outbox(&config, "wellness");
        std::fs::remove_file(outbox.join("devis.pdf")).unwrap();
        assert!(crate::sent_files::capture(&config, &agent, "/envoi/devis.pdf").is_ok());
        std::fs::write(outbox.join("devis.pdf"), b"%PDF-1.7 devis").unwrap();
        // the MEDIA: prefix the model may copy, a file that isn't there, no path
        let again = call_with(&config, &agent, &args(json!({"path": "MEDIA:/envoi/devis.pdf"})), &[]).await;
        assert!(text(&again).starts_with("MEDIA:/envoi/devis.pdf"));
        let missing = call_with(&config, &agent, &args(json!({"path": "/envoi/none.pdf"})), &[]).await;
        assert_eq!(missing["isError"], true);
        assert!(!text(&missing).contains("MEDIA:"));
        let empty = call_with(&config, &agent, &args(json!({})), &[]).await;
        assert_eq!(empty["isError"], true);
    }

    #[tokio::test]
    async fn too_big_without_a_drive() {
        let (_dir, config, agent) = setup();
        let done = call_with(&config, &agent, &args(json!({"path": "/envoi/film.mp4"})), &drive_catalog()).await;
        assert_eq!(done["isError"], true);
        let said = text(&done);
        assert!(said.contains("too large") && !said.contains("MEDIA:"), "{said}");
        assert!(said.contains("connect your cloud drive"), "a drive service exists: {said}");
        // no drive service at all: just too big
        let done = call_with(&config, &agent, &args(json!({"path": "/envoi/film.mp4"})), &[]).await;
        assert!(text(&done).ends_with("« This file is too big to send you here. »"), "{}", text(&done));
    }

    #[tokio::test]
    async fn a_connected_drive_gets_the_file_and_gives_the_link() {
        let (_dir, config, agent) = setup();
        let calls = Calls::default();
        let url = drive_server(calls.clone()).await;
        connect(&config, &url, &["wellness"]);
        let given = args(json!({"path": "/envoi/devis.pdf", "title": "Devis cuisine"}));
        let done = call_with(&config, &agent, &given, &drive_catalog()).await;
        assert_eq!(done["isError"], false, "{}", text(&done));
        let said = text(&done);
        assert!(said.contains("https://drive.example/f/42") && said.contains("Test Drive"), "{said}");
        assert!(said.contains("MEDIA:/envoi/devis.pdf"), "the fallback line: {said}");
        let seen = calls.lock().unwrap().clone();
        let methods: Vec<&str> = seen.iter().map(|(_, b)| b["method"].as_str().unwrap()).collect();
        assert_eq!(methods, ["initialize", "notifications/initialized", "tools/call"]);
        assert!(seen.iter().all(|(auth, _)| auth.as_deref() == Some("Bearer at-1")));
        let call = &seen[2].1["params"];
        assert_eq!(call["name"], "upload_file");
        assert_eq!(call["arguments"]["name"], "Devis cuisine.pdf");
        assert_eq!(call["arguments"]["parent"], FOLDER);
        assert_eq!(call["arguments"]["mimeType"], "application/pdf");
        let sent = base64::engine::general_purpose::STANDARD.decode(call["arguments"]["content"].as_str().unwrap());
        assert_eq!(sent.unwrap(), b"%PDF-1.7 devis");
        // over the conversation's limit: the drive only, no MEDIA: line
        let big = call_with(&config, &agent, &args(json!({"path": "/envoi/film.mp4"})), &drive_catalog()).await;
        assert_eq!(big["isError"], false);
        assert!(text(&big).contains("https://drive.example/f/") && !text(&big).contains("MEDIA:"), "{}", text(&big));
    }

    #[tokio::test]
    async fn the_drive_only_for_its_agents_and_never_for_a_shared_agent() {
        let (_dir, config, agent) = setup();
        let calls = Calls::default();
        let url = drive_server(calls.clone()).await;
        connect(&config, &url, &["vie"]);
        let done = call_with(&config, &agent, &args(json!({"path": "/envoi/devis.pdf"})), &drive_catalog()).await;
        assert!(text(&done).starts_with("MEDIA:"), "not this agent's drive: {}", text(&done));
        // the family's agent: its files go in the conversation, never the person's drive
        connect(&config, &url, &["wellness"]);
        let state = std::path::PathBuf::from(config.data_dir()).join("family-agent.json");
        std::fs::write(state, r#"{"agent": "wellness"}"#).unwrap();
        assert!(!may_use_drive(&config, "wellness"));
        let done = call_with(&config, &agent, &args(json!({"path": "/envoi/devis.pdf"})), &drive_catalog()).await;
        assert!(text(&done).starts_with("MEDIA:"), "{}", text(&done));
        assert!(calls.lock().unwrap().is_empty());
        // and both kinds of shared agents get the tool
        assert!(crate::family_agent::TOOLS.contains(&TOOL_NAME) && crate::project_agent::TOOLS.contains(&TOOL_NAME));
    }

    #[test]
    fn the_first_connected_drive_in_the_catalogs_order() {
        let catalog = drive_catalog();
        let mut stored = Stored::default();
        assert!(drive_for(&catalog, &stored, "bip").is_none());
        let conn =
            |state: &str| Connection { agents: vec!["bip".into()], state: state.into(), ..Connection::default() };
        stored.connections.insert("notion".into(), conn("connected")); // not a drive
        stored.connections.insert("testdrive".into(), conn("expired"));
        assert!(drive_for(&catalog, &stored, "bip").is_none());
        stored.connections.insert("testdrive".into(), conn("connected"));
        assert_eq!(drive_for(&catalog, &stored, "bip").map(|(s, _)| s.id.as_str()), Some("testdrive"));
        assert!(drive_for(&catalog, &stored, "vie").is_none());
        // the real catalog has no drive yet
        assert!(crate::connections::catalog().iter().all(|s| s.drive.is_none()));
    }

    #[test]
    fn the_name_in_the_drive() {
        assert_eq!(drive_name("", "/envoi/devis.pdf"), "devis.pdf");
        assert_eq!(drive_name("Devis cuisine", "/envoi/devis.pdf"), "Devis cuisine.pdf");
        assert_eq!(drive_name("Devis.PDF", "/envoi/devis.pdf"), "Devis.PDF");
        assert_eq!(drive_name("a/b\\c\n", "/root/x"), "a b c");
        assert_eq!(drive_name("..", "/envoi/.x"), ".x");
    }

    #[test]
    fn answers_in_json_or_an_event_stream() {
        let one = r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true}}"#;
        assert_eq!(answer_of("application/json", one, 2).unwrap()["result"]["ok"], true);
        assert!(answer_of("application/json", one, 1).is_none());
        let stream = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\r\n\r\n\
                      event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":1}}\r\n\r\n";
        assert_eq!(answer_of("text/event-stream", stream, 2).unwrap()["result"]["ok"], 1);
    }

    #[test]
    fn the_link_from_the_result() {
        let structured = json!({"structuredContent": {"webViewLink": "https://drive.example/f/1"}, "content": []});
        assert_eq!(link_of(&structured, "webViewLink").as_deref(), Some("https://drive.example/f/1"));
        let text = json!({"content": [{"type": "text", "text": "Uploaded (https://drive.example/f/2)."}]});
        assert_eq!(link_of(&text, "").as_deref(), Some("https://drive.example/f/2"));
        assert_eq!(link_of(&text, "webViewLink").as_deref(), Some("https://drive.example/f/2"));
        assert!(link_of(&json!({"content": [{"type": "text", "text": "done"}]}), "").is_none());
        let not_web = json!({"structuredContent": {"webViewLink": "javascript:x"}});
        assert!(link_of(&not_web, "webViewLink").is_none());
    }

    #[test]
    fn placeholders_filled_deep() {
        let filled = fill(
            &json!({"a": "{name} in {folder}", "b": ["{mime_type}"], "n": 3}),
            &[("name", "x.pdf"), ("folder", "BipAgents"), ("mime_type", "application/pdf")],
        );
        assert_eq!(filled, json!({"a": "x.pdf in BipAgents", "b": ["application/pdf"], "n": 3}));
    }
}
