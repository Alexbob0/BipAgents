//! Services connected to an agent (MCP servers), from the app's catalog (tool_catalog.yaml).
//!
//! Written with Hermes' own CLI (`config set` / `config unset`) into the agent's profile, which its gateway
//! re-reads within about a minute: no restart. A token goes to the profile's .env (0600) and the config only
//! refers to it (`Bearer ${MCP_<NAME>_API_KEY}`), so it never appears on a command line or in config.yaml.
//!
//! Port of `bridge/bipbridge/services.py` (owner group: agents).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Value};
use unicode_normalization::UnicodeNormalization;

use crate::agents_admin::pyyaml::{self, Yaml};
use crate::agents_admin::Run;
use crate::config::{AgentConfig, Config};
use crate::Dict;

pub static NAME_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_]{0,31}$").expect("valid regex"));
/// The app's catalog (`tool_catalog.yaml`, embedded).
pub const CATALOG_YAML: &str = crate::assets::TOOL_CATALOG_YAML;

/// Python's `ServiceError` (a `ValueError`): returned inside `anyhow::Error`, found with `downcast_ref`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ServiceError(pub String);

/// Python's `KeyError(name)` of `remove` and `test`: no such service in the agent's profile.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct UnknownService(pub String);

static CATALOG: LazyLock<Vec<Dict>> = LazyLock::new(|| match pyyaml::load(CATALOG_YAML).map(|d| pyyaml::to_json(&d)) {
    Ok(Value::Array(items)) => items.into_iter().filter_map(|i| i.as_object().cloned()).collect(),
    _ => Vec::new(),
});

pub fn catalog() -> Vec<Dict> {
    CATALOG.clone()
}

fn text(item: &Dict, key: &str) -> String {
    item.get(key).map(crate::py::py_str).unwrap_or_default()
}

fn entry(catalog_id: &str) -> Result<Dict, ServiceError> {
    CATALOG
        .iter()
        .find(|item| text(item, "id") == catalog_id)
        .cloned()
        .ok_or_else(|| ServiceError("unknown service".into()))
}

pub fn env_name(name: &str) -> String {
    format!("MCP_{}_API_KEY", name.to_uppercase())
}

/// `urllib.parse.urlsplit(value)`: (scheme, hostname).
fn split_url(value: &str) -> (String, Option<String>) {
    let mut rest = value;
    let mut scheme = String::new();
    if let Some(i) = value.find(':') {
        let candidate = &value[..i];
        let first = candidate.chars().next();
        if i > 0
            && first.map(|c| c.is_ascii_alphabetic()).unwrap_or(false)
            && candidate.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            scheme = candidate.to_ascii_lowercase();
            rest = &value[i + 1..];
        }
    }
    let Some(after) = rest.strip_prefix("//") else { return (scheme, None) };
    let netloc = after.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = netloc.rsplit_once('@').map(|(_, h)| h).unwrap_or(netloc);
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    let host = host.to_lowercase();
    (scheme, if host.is_empty() { None } else { Some(host) })
}

fn web_url(value: &str) -> Result<String, ServiceError> {
    let mut value = value.trim().trim_end_matches('/').to_string();
    if !value.contains("://") && !value.is_empty() {
        value = format!("https://{value}");
    }
    let (scheme, host) = split_url(&value);
    if !(scheme == "http" || scheme == "https") || host.is_none() {
        return Err(ServiceError("the address must be a web address".into()));
    }
    Ok(value)
}

/// What to write for the app's form: name, url, token (or None), and whether it needs the agent's own space.
pub fn plan(body: &Dict) -> Result<Dict, ServiceError> {
    let catalog_id = body.get("catalog").filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default();
    let item = entry(&catalog_id)?;
    let fields: Vec<Dict> = item
        .get("fields")
        .and_then(Value::as_array)
        .map(|f| f.iter().filter_map(|x| x.as_object().cloned()).collect())
        .unwrap_or_default();
    let mut values: IndexMap<String, String> = IndexMap::new();
    for f in &fields {
        let id = text(f, "id");
        let value = body.get(&id).filter(|v| crate::py::truthy(v)).map(crate::py::py_str).unwrap_or_default();
        values.insert(id, value.trim().to_string());
    }
    for f in &fields {
        let optional = f.get("optional").map(crate::py::truthy).unwrap_or(false);
        if values.get(&text(f, "id")).map(String::is_empty).unwrap_or(true) && !optional {
            return Err(ServiceError(format!("{} is required", text(f, "label"))));
        }
    }
    let item_id = text(&item, "id");
    let name = values.get("name").filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| item_id.clone());
    // « Météo » → meteo
    let plain: String = name.nfkd().filter(char::is_ascii).collect::<String>().to_lowercase();
    let mut cleaned = String::new();
    let mut gap = false;
    for c in plain.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            if gap {
                cleaned.push('_');
                gap = false;
            }
            cleaned.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        cleaned.push('_');
    }
    let mut name: String = cleaned.trim_matches('_').chars().take(32).collect();
    if name.is_empty() {
        name = item_id.clone();
    }
    if !NAME_RE.is_match(&name) {
        return Err(ServiceError("invalid name".into()));
    }
    let address = match values.get("address").filter(|a| !a.is_empty()) {
        Some(a) => web_url(a)?,
        None => String::new(),
    };
    let url = web_url(&text(&item, "url").replace("{address}", &address))?;
    let token = values.get("token").filter(|t| !t.is_empty()).cloned();
    let personal = item.get("personal").map(crate::py::truthy).unwrap_or(false);
    let mut out = Dict::new();
    out.insert("name".into(), json!(name));
    out.insert("url".into(), json!(url));
    out.insert("token".into(), json!(token));
    out.insert("catalog".into(), json!(item_id));
    out.insert("needs_own_space".into(), json!(personal || token.is_some()));
    Ok(out)
}

fn profile_home(config: &Config, agent: &AgentConfig) -> PathBuf {
    let home = agent
        .space
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| Some(config.hermes.home.clone()).filter(|h| !h.is_empty()))
        .unwrap_or_else(|| crate::py::join(&crate::py::expanduser("~"), ".hermes"));
    PathBuf::from(home)
}

pub fn listing(config: &Config, agent: &AgentConfig) -> Vec<Dict> {
    let path = profile_home(config, agent).join("profiles").join(&agent.name).join("config.yaml");
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let Ok(data) = pyyaml::load(&text) else { return Vec::new() };
    let servers = match data.get("mcp_servers") {
        Some(Yaml::Map(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (name, server) in servers {
        if !server.is_map() {
            continue;
        }
        let url = server.get("url").filter(|u| u.truthy()).map(Yaml::py_str).unwrap_or_default();
        let enabled = !matches!(server.get("enabled"), Some(Yaml::Bool(false)));
        let mut item = Dict::new();
        item.insert("name".into(), json!(name.py_str()));
        item.insert("url".into(), if url.is_empty() { Value::Null } else { json!(url) });
        item.insert("command".into(), server.get("command").map(pyyaml::to_json).unwrap_or(Value::Null));
        item.insert("enabled".into(), json!(enabled));
        item.insert("catalog".into(), json!(catalog_of(&url)));
        out.push(item);
    }
    out
}

/// Which catalog entry an existing server looks like (None: set up by hand, e.g. a local command).
pub fn catalog_of(url: &str) -> Option<String> {
    if url.is_empty() {
        return None;
    }
    for item in CATALOG.iter() {
        let pattern = text(item, "url");
        if !pattern.contains("{address}") && url.starts_with(pattern.trim_end_matches('/')) {
            return Some(text(item, "id"));
        }
        let (prefix, suffix) = match pattern.split_once("{address}") {
            Some((p, s)) => (p, s),
            None => (pattern.as_str(), ""),
        };
        if prefix.is_empty() && !suffix.is_empty() && url.ends_with(suffix) {
            return Some(text(item, "id"));
        }
    }
    Some("custom".to_string())
}

/// A `KEY=value` line of a .env set (or removed), the others kept; the file 0600.
pub fn set_env(path: &Path, key: &str, value: Option<&str>) -> std::io::Result<()> {
    let text = if path.exists() { std::fs::read_to_string(path)? } else { String::new() };
    let mut kept: Vec<String> = crate::ssh_keys::splitlines(&text)
        .into_iter()
        .filter(|line| line.split('=').next().unwrap_or("").trim() != key)
        .map(str::to_string)
        .collect();
    if let Some(value) = value {
        kept.push(format!("{key}={value}"));
    }
    std::fs::write(path, format!("{}\n", kept.join("\n")))?;
    crate::py::chmod(&path.to_string_lossy(), 0o600)
}

fn env_of(home: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([("HERMES_HOME".to_string(), home.to_string_lossy().into_owned())])
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

pub async fn add(config: &Config, agent: &AgentConfig, body: &Dict, run: &Run) -> anyhow::Result<Dict> {
    let wanted = plan(body)?;
    if wanted.get("needs_own_space").map(crate::py::truthy).unwrap_or(false) {
        if let Some(why) = crate::vault::holder(config, agent) {
            let reasons: IndexMap<String, String> = [(agent.name.clone(), why)].into_iter().collect();
            return Err(crate::vault::VaultError::CannotHold(reasons).into());
        }
    }
    let home = profile_home(config, agent);
    let env = env_of(&home);
    let name = text(&wanted, "name");
    let url = text(&wanted, "url");
    let catalog_id = text(&wanted, "catalog");
    if listing(config, agent).iter().any(|s| text(s, "name") == name) {
        return Err(ServiceError("a service with this name already exists".into()).into());
    }
    let token = wanted.get("token").and_then(Value::as_str).map(str::to_string);
    if let Some(token) = &token {
        set_env(&home.join("profiles").join(&agent.name).join(".env"), &env_name(&name), Some(token))?;
    }
    let hermes = config.hermes.bin.as_str();
    run(args(&[hermes, "-p", &agent.name, "config", "set", &format!("mcp_servers.{name}.url"), &url]), env.clone())
        .await?;
    if token.is_some() {
        let header = format!("Bearer ${{{}}}", env_name(&name));
        run(
            args(&[
                hermes,
                "-p",
                &agent.name,
                "config",
                "set",
                &format!("mcp_servers.{name}.headers.Authorization"),
                &header,
            ]),
            env.clone(),
        )
        .await?;
    }
    if catalog_id != "home_assistant" {
        // Hermes asks the person before each of its tools that writes (sends, creates, deletes): a page or an e-mail
        // the agent read cannot make it act through the service. Home Assistant (lights, heating) stays direct.
        run(
            args(&[hermes, "-p", &agent.name, "config", "set", &format!("mcp_servers.{name}.trust"), "untrusted"]),
            env.clone(),
        )
        .await?;
    }
    tracing::info!(agent = %agent.name, service = %name, catalog = %catalog_id, "service connected");
    let mut out = Dict::new();
    out.insert("name".into(), json!(name));
    out.insert("url".into(), json!(url));
    out.insert("enabled".into(), json!(true));
    out.insert("catalog".into(), json!(catalog_id));
    Ok(out)
}

fn known(config: &Config, agent: &AgentConfig, name: &str) -> Result<(), UnknownService> {
    if !NAME_RE.is_match(name) || !listing(config, agent).iter().any(|s| text(s, "name") == name) {
        return Err(UnknownService(name.to_string()));
    }
    Ok(())
}

pub async fn remove(config: &Config, agent: &AgentConfig, name: &str, run: &Run) -> anyhow::Result<()> {
    known(config, agent, name)?;
    let home = profile_home(config, agent);
    run(
        args(&[&config.hermes.bin, "-p", &agent.name, "config", "unset", &format!("mcp_servers.{name}")]),
        env_of(&home),
    )
    .await?;
    set_env(&home.join("profiles").join(&agent.name).join(".env"), &env_name(name), None)?;
    tracing::info!(agent = %agent.name, service = %name, "service removed");
    Ok(())
}

static TOOLS_FOUND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Tools discovered: (\d+)").expect("valid regex"));
static TOOLS_COUNT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+) tools?").expect("valid regex"));

/// `hermes mcp test`: connects and lists the tools. Its last line explains a failure (never a secret).
pub async fn test(config: &Config, agent: &AgentConfig, name: &str, run: &Run) -> anyhow::Result<Dict> {
    known(config, agent, name)?;
    let out =
        run(args(&[&config.hermes.bin, "-p", &agent.name, "mcp", "test", name]), env_of(&profile_home(config, agent)))
            .await?;
    let lines: Vec<&str> =
        crate::ssh_keys::splitlines(&out).into_iter().map(str::trim).filter(|l| !l.is_empty()).collect();
    let failed = lines.iter().find(|l| l.starts_with('✗')).copied();
    let tools =
        TOOLS_FOUND.captures(&out).or_else(|| TOOLS_COUNT.captures(&out)).and_then(|c| c[1].parse::<i64>().ok());
    let message: String = failed.unwrap_or("").trim_start_matches(['✗', ' ']).trim().chars().take(300).collect();
    let mut result = Dict::new();
    result.insert("ok".into(), json!(failed.is_none() && lines.iter().any(|l| l.starts_with('✓'))));
    result.insert("message".into(), if message.is_empty() { Value::Null } else { json!(message) });
    result.insert("tools".into(), json!(tools));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(value: Value) -> Dict {
        value.as_object().unwrap().clone()
    }

    /// test_services.py: what the form must hold.
    #[test]
    fn what_the_form_must_hold() {
        assert!(plan(&body(json!({"catalog": "github"}))).is_err());
        assert!(plan(&body(json!({"catalog": "custom", "name": "x", "address": "ftp://x"}))).is_err());
        assert!(plan(&body(json!({"catalog": "nope"}))).is_err());
        assert_eq!(
            plan(&body(json!({"catalog": "github", "token": "t"}))).unwrap()["url"],
            "https://api.githubcopilot.com/mcp"
        );
        assert_eq!(catalog_of("https://api.githubcopilot.com/mcp").as_deref(), Some("github"));
        assert_eq!(catalog_of("https://x.org/mcp").as_deref(), Some("custom"));
        let ha = plan(&body(
            json!({"catalog": "home_assistant", "address": "homeassistant.local:8123", "token": "ha-secret"}),
        ))
        .unwrap();
        assert_eq!(ha["url"], "https://homeassistant.local:8123/api/mcp");
        assert_eq!(
            plan(&body(json!({"catalog": "custom", "name": "Météo", "address": "https://meteo.example/mcp"}))).unwrap()
                ["name"],
            "meteo"
        );
    }
}
