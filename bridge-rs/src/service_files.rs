//! A file of the agent's own space handed to one of its services (owner request 2026-11-06: the Compta agent fetches
//! the invoices missing for bank payments from the suppliers' portals and gives them to Kerfy Compta). Without it the
//! model would read the PDF and paste megabytes of base64 into a tool call: slow, costly, and cut by any limit.
//!
//! `send_file_to_service(service, path)` (MCP, on the `bipagents_files` entry, « untrusted »: Hermes asks the person
//! before it runs, until they allow it for good):
//!
//! - `service` is one of the agent's own MCP services (its profile's `mcp_servers`, added from the app: services.rs),
//!   never one of the bridge's own entries nor another agent's;
//! - `path` is one of the agent's own working files, as it sees them (`/telechargements/<name>`: what its visible
//!   browser downloaded; `/envoi`, `/workspace`, `/root`, `/partage`, the files it was sent: sent_files.rs
//!   `open_working`), opened name by name without following a link, never a hidden or secret-looking file, at most
//!   `MAX_BYTES`;
//! - the bridge POSTs the bytes to `<the service's MCP address>/uploads` with the service's own bearer token (from
//!   the profile's `.env`, which the agent never reads), `X-Filename` (percent-encoded) and `X-Sha256`; no redirect
//!   is followed (the token never goes elsewhere);
//! - the service keeps the file aside and answers `{upload: "<id>"}`; the agent then gives that id to the service's own
//!   tool (Kerfy Compta: `piece_add` with `upload`), which goes through Hermes' approval as before. Nothing enters the
//!   service's records by the upload alone.
//!
//! The convention, for any service that wants files: `POST <mcp url>/uploads`, raw body, `Authorization: Bearer`,
//! `X-Filename`, `X-Sha256`; 200/201 with JSON `{"upload": "…"}`, or an error status with `{"error": "…"}`.
//!
//! Each hand-off is logged (`bipbridge::agents`, « file sent to a service »): the agent, the service, the file's name,
//! size and SHA-256, the answer's status. Never the token, never the content.
//!
//! No Python counterpart.

use std::io::Read;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{AgentConfig, Config};
use crate::tools_knowledge::{tool_failure, tool_text};
use crate::Dict;

pub const TOOL_NAME: &str = "send_file_to_service";
/// The largest file handed over (an invoice is a few hundred KB).
pub const MAX_BYTES: u64 = 10 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// The bridge's own entries in a profile: never a target.
const OWN_ENTRIES: [&str; 3] = ["bipagents", "stealth", "bip"];

pub fn tool() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": format!("Hand one of your files to one of your services that takes files (e.g. « compta »): \
    a PDF your browser downloaded ({d}/<name>), or a file in /envoi, /workspace or the files you were sent. The bridge \
    sends it with the service's own access; you get back an upload id to give to that service's tool (for compta: \
    piece_add with `upload`). Your browser's downloads are not in your terminal: give `path: {d}` alone to list \
    them. Never read a file to paste its content into a tool.", d = crate::browsers::DOWNLOADS),
        "inputSchema": {"type": "object", "properties": {
            "service": {"type": "string", "description": "the service's name, as in your tools (e.g. compta)"},
            "path": {"type": "string", "description": format!("the file as you see it, e.g. {}/facture-2026-10.pdf", crate::browsers::DOWNLOADS)},
        }, "required": ["service", "path"]},
        "annotations": {"readOnlyHint": false, "destructiveHint": false, "openWorldHint": true},
    })
}

/// The agent's service `name`: its MCP address and its token (None: the service has none). Err(why) for the agent.
pub fn service_of(config: &Config, agent: &AgentConfig, name: &str) -> Result<(String, Option<String>), String> {
    let name = name.trim();
    let own = OWN_ENTRIES.contains(&name)
        || name.starts_with("bipagents_")
        || name.starts_with(crate::connectors::ENTRY_PREFIX);
    let listed = crate::services::listing(config, agent);
    let mine: Vec<String> = listed
        .iter()
        .filter(|s| s.get("url").and_then(Value::as_str).is_some())
        .map(|s| s.get("name").map(crate::py::py_str).unwrap_or_default())
        .filter(|n| {
            !OWN_ENTRIES.contains(&n.as_str())
                && !n.starts_with("bipagents_")
                && !n.starts_with(crate::connectors::ENTRY_PREFIX)
        })
        .collect();
    let found = (!own && crate::services::NAME_RE.is_match(name))
        .then(|| listed.iter().find(|s| s.get("name").map(crate::py::py_str).as_deref() == Some(name)))
        .flatten();
    let Some(entry) = found else {
        return Err(if mine.is_empty() {
            format!("No service « {name} »: you have no service that takes files.")
        } else {
            format!("No service « {name} ». Your services: {}.", mine.join(", "))
        });
    };
    if entry.get("enabled").and_then(Value::as_bool) == Some(false) {
        return Err(format!("The service « {name} » is turned off."));
    }
    let url = entry.get("url").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("The service « {name} » has no web address to send a file to."));
    }
    let env = crate::services::profile_home(config, agent).join("profiles").join(&agent.name).join(".env");
    let key = crate::services::env_name(name);
    let token = std::fs::read_to_string(env).ok().and_then(|text| {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
        })
    });
    Ok((url, token))
}

/// `<mcp url>/uploads`, its query left out.
pub fn uploads_url(url: &str) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    format!("{base}/uploads")
}

/// The name as sent in `X-Filename`: percent-encoded UTF-8, its last component only.
pub fn header_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The agent's file behind `path`, read whole (blocking): (its name, its bytes). Err(why) for the agent.
pub fn read_file(config: &Config, agent: &AgentConfig, path: &str) -> Result<(String, Vec<u8>), String> {
    let wanted = path.trim().trim_start_matches("MEDIA:").trim();
    let (at, file) = crate::sent_files::open_working(config, agent, wanted).map_err(|why| {
        let downloads = crate::browsers::recent_downloads(config, &agent.name, 8);
        let listed = if downloads.is_empty() {
            String::new()
        } else {
            format!(" Your browser's latest downloads: {}.", downloads.join(", "))
        };
        format!("{wanted}: {why}. Copy a file you made into /envoi first.{listed}")
    })?;
    let size = file.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
    if size == 0 {
        return Err(format!("{at} is empty (a download still running?)."));
    }
    if size > MAX_BYTES {
        return Err(format!("{at} is too large to hand over (max {} MB).", MAX_BYTES / (1024 * 1024)));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|e| format!("{at}: not readable ({e})"))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format!("{at} is too large to hand over."));
    }
    let name = at.rsplit('/').next().unwrap_or(&at).to_string();
    Ok((name, bytes))
}

/// What the service answered, for the agent: Ok(upload id) or Err(its words, cut short).
fn answer_of(status: u16, body: &str) -> Result<String, String> {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    if (200..300).contains(&status) {
        let id = parsed.as_ref().and_then(|v| v.get("upload")).and_then(Value::as_str).map(str::trim);
        return match id {
            Some(id) if !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_graphic()) => {
                Ok(id.to_string())
            }
            _ => Err(format!("HTTP {status} without an upload id: this service may not take files")),
        };
    }
    let said = parsed
        .as_ref()
        .and_then(|v| v.get("error").or_else(|| v.get("message")))
        .and_then(Value::as_str)
        .unwrap_or(body);
    let said: String = said.chars().filter(|c| !c.is_control()).take(300).collect();
    Err(match status {
        404 | 405 => format!("HTTP {status}: this service does not take files"),
        401 | 403 => format!("HTTP {status}: the service refused its access ({said})"),
        _ => format!("HTTP {status}: {said}"),
    })
}

/// One call, for `agent`.
pub async fn call(config: &Config, agent: &AgentConfig, arguments: &Dict) -> Dict {
    let service = arguments.get("service").map(crate::py::py_str).unwrap_or_default().trim().to_string();
    let path = arguments.get("path").map(crate::py::py_str).unwrap_or_default();
    if service.is_empty() || path.trim().is_empty() {
        return tool_text("Give `service` (e.g. compta) and `path` (your file as you see it).", true);
    }
    // the browser's downloads listed (the agent's terminal does not see them)
    if path.trim().trim_end_matches('/') == crate::browsers::DOWNLOADS {
        let (c, name) = (config.clone(), agent.name.clone());
        let listed = tokio::task::spawn_blocking(move || crate::browsers::recent_downloads(&c, &name, 20))
            .await
            .unwrap_or_default();
        return tool_text(
            &if listed.is_empty() {
                "Your browser has downloaded nothing yet (or the download is still running).".to_string()
            } else {
                format!("Your browser's latest downloads, newest first:\n{}", listed.join("\n"))
            },
            false,
        );
    }
    let (url, token) = match service_of(config, agent, &service) {
        Ok(found) => found,
        Err(why) => return tool_text(&why, true),
    };
    let (c, a, p) = (config.clone(), agent.clone(), path.clone());
    let read = tokio::task::spawn_blocking(move || read_file(&c, &a, &p)).await;
    let (name, bytes) = match read {
        Ok(Ok(found)) => found,
        Ok(Err(why)) => return tool_text(&why, true),
        Err(_) => return tool_text("The file could not be read.", true),
    };
    let sha = hex::encode(Sha256::digest(&bytes));
    let size = bytes.len();
    let http = match reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("BipAgents-bridge/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(h) => h,
        Err(_) => return tool_text("The bridge could not reach the service.", true),
    };
    let mut request = http
        .post(uploads_url(&url))
        .header("content-type", "application/octet-stream")
        .header("x-filename", header_name(&name))
        .header("x-sha256", &sha)
        .body(bytes);
    if let Some(token) = &token {
        request = request.bearer_auth(token);
    }
    let sent = request.send().await;
    let (status, outcome) = match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            (status, answer_of(status, &body))
        }
        Err(e) => (0, Err(format!("not reached ({})", if e.is_timeout() { "timeout" } else { "connection" }))),
    };
    tracing::info!(
        target: "bipbridge::agents",
        agent = %agent.name,
        service = %service,
        file = %name,
        size,
        sha256 = %sha,
        status,
        ok = outcome.is_ok(),
        "file sent to a service"
    );
    match outcome {
        Ok(upload) => tool_text(
            &format!(
                "Sent {name} ({} KB) to « {service} »: upload {upload}. Now give it to that service's tool (for \
                 compta: piece_add with upload=\"{upload}\"). It waits there a day at most.",
                size.div_ceil(1024)
            ),
            false,
        ),
        Err(why) => tool_failure(
            &format!("{name} was not handed to « {service} »: {why}."),
            &format!("Je n'ai pas pu transmettre {name} au service {service}."),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uploads_address_and_the_name_sent() {
        assert_eq!(uploads_url("http://100.64.0.1:8740/mcp"), "http://100.64.0.1:8740/mcp/uploads");
        assert_eq!(uploads_url("https://x.example/mcp/?v=2"), "https://x.example/mcp/uploads");
        assert_eq!(header_name("/telechargements/Facture été.pdf"), "Facture%20%C3%A9t%C3%A9%2Epdf");
        assert_eq!(header_name("a/b/../c.pdf"), "c%2Epdf");
    }

    #[test]
    fn what_the_service_answered() {
        assert_eq!(answer_of(201, r#"{"upload":"upl_0123"}"#), Ok("upl_0123".into()));
        assert!(answer_of(200, r#"{"ok":true}"#).unwrap_err().contains("may not take files"));
        assert!(answer_of(200, r#"{"upload":"a b"}"#).is_err());
        assert!(answer_of(404, "not found").unwrap_err().contains("does not take files"));
        assert!(answer_of(401, r#"{"error":"a token is needed"}"#).unwrap_err().contains("refused its access"));
        let long = format!(r#"{{"error":"{}\u0007"}}"#, "x".repeat(1000));
        let said = answer_of(400, &long).unwrap_err();
        assert!(said.len() < 320 && !said.contains('\u{7}'));
    }
}
