//! Reading open, sending guarded (owner decision 2026-10-28, internal/next/send-guard.md): the limits the agents'
//! proxy (`assets/sandbox/egress_proxy.py`) applies, written in its policy file by sandbox_sites.rs; the offer's
//! abuse limits; the piracy and torrent trackers' blocklist; the visible browser's way through the same proxy.
//!
//! - **Sending**: what a program (or the agent's browser) sends a site its person did not validate is counted; past
//!   `send_site_kb_hour` (64 KB) to one site in an hour, or `send_day_kb` (256 KB) a day across such sites, the proxy
//!   cuts the tunnel and the person gets ONE card (sandbox_sites.rs). Measured on 2026-10-28 through the proxy (TLS
//!   application data, the handshake left out): an HTTPS GET with curl ≈ 160–260 bytes, a `pip download requests`
//!   ≈ 3 KB per host, a shallow `git clone` ≈ 0.9 KB, a form post of 6 KB of text ≈ 8.8 KB. A page load, an API call,
//!   an install, a login or a form of a few KB never comes near; a file, a mailbox or an export does. Slow leaks: about
//!   1 000 small requests a day to sites nobody validated reach the day's total (a card, then the person validates an
//!   API the agent keeps using). The browser's requests are small TLS records whose first 4 KB are not counted (its
//!   pages make many), an upload's 16 KB records are.
//! - **The offer** (`config.cloud`, a place Kerfy hosts): a download limit per agent per day (2 GB), new connections
//!   per agent per minute (300), `--cpus` per sandbox (1.5), the blocklist. A self-install keeps none of them unless
//!   its `[sandbox]` sets them: its machine, its responsibility (the agents' rule against piracy stays, souls.rs).
//! - **The blocklist**: public lists of piracy and torrent trackers' names (`blocklist_urls`), fetched by the bridge
//!   once a week and kept in the proxy's policy folder; a list that can't be fetched leaves the last one, or none
//!   (fail open: a list is a courtesy to the host, not the wall).
//! - **The browser** (browsers.rs): on a place whose sandboxes are isolated with podman, each agent's Chrome joins
//!   `bipagents-browsers` with an address of its own (from its ports) and goes through the proxy (`--proxy-server`),
//!   which knows it by that address: CONNECT only, no TLS interception, WebRTC kept to proxied traffic.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::AppState;
use crate::config::Config;

/// Public lists of piracy and BitTorrent sites' names (The Block List Project, domains only, Unlicense).
pub const BLOCKLIST_URLS: [&str; 2] = [
    "https://blocklistproject.github.io/Lists/alt-version/piracy-nl.txt",
    "https://blocklistproject.github.io/Lists/alt-version/torrent-nl.txt",
];
pub const BLOCKLIST_FILE: &str = "blocklist.txt";
/// The proxy's counts of the day, written each minute.
pub const USAGE_FILE: &str = "usage.json";
pub const BROWSER_NETWORK: &str = "bipagents-browsers";
const BLOCKLIST_EVERY: f64 = 7.0 * 86400.0;
const BLOCKLIST_MAX: usize = 20 * 1024 * 1024;
const BLOCKLIST_NAMES: usize = 500_000;

pub const SEND_SITE_KB_HOUR: u64 = 64;
pub const SEND_DAY_KB: u64 = 256;
pub const OFFER_DOWNLOAD_DAY_MB: u64 = 2048;
pub const OFFER_CONNECTIONS_PER_MINUTE: u64 = 300;
pub const OFFER_CPUS: f64 = 1.5;

/// The limits in the proxy's policy file (bytes; 0: none).
pub fn limits(config: &Config) -> Value {
    let s = &config.sandbox;
    let offer = |set: Option<u64>, default: u64| set.unwrap_or(if config.cloud { default } else { 0 });
    json!({
        "send_site_hour": s.send_site_kb_hour.unwrap_or(SEND_SITE_KB_HOUR) * 1024,
        "send_day": s.send_day_kb.unwrap_or(SEND_DAY_KB) * 1024,
        "download_day": offer(s.download_day_mb, OFFER_DOWNLOAD_DAY_MB) * 1024 * 1024,
        "connections_minute": offer(s.connections_per_minute, OFFER_CONNECTIONS_PER_MINUTE),
    })
}

/// CPUs per sandbox (`--cpus`), if any.
pub fn cpus(config: &Config) -> Option<f64> {
    let cpus = config.sandbox.cpus.unwrap_or(if config.cloud { OFFER_CPUS } else { 0.0 });
    (cpus > 0.0).then_some(cpus)
}

pub fn blocklist_on(config: &Config) -> bool {
    config.sandbox.blocklist.unwrap_or(config.cloud)
}

pub fn blocklist_path(config: &Config) -> PathBuf {
    crate::sandbox_sites::policy_dir(config).join(BLOCKLIST_FILE)
}

/// The names in a list as published: plain names, a hosts file's (`0.0.0.0 name`), adblock's (`||name^`); comments
/// and anything that isn't a name of the internet left out.
pub fn parse_list(text: &str, into: &mut std::collections::BTreeSet<String>) {
    for line in text.lines() {
        let line = line.split(['#', '!']).next().unwrap_or("").trim();
        let word = line.split_whitespace().last().unwrap_or("");
        let word = word.trim_start_matches("||").trim_end_matches('^').trim_end_matches('.').to_ascii_lowercase();
        if word.is_empty() || into.len() >= BLOCKLIST_NAMES {
            continue;
        }
        let labels: Vec<&str> = word.split('.').collect();
        let named = labels.len() >= 2
            && labels.iter().all(|l| {
                !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            && labels.last().is_some_and(|t| t.chars().all(|c| c.is_ascii_alphabetic()) || t.starts_with("xn--"));
        if named && word != "localhost.localdomain" {
            into.insert(word);
        }
    }
}

fn age(path: &std::path::Path) -> Option<f64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.elapsed().map(|d| d.as_secs_f64()).unwrap_or(0.0))
}

/// The blocklist kept up to date (a week old: fetched again); off: removed. Never fails: a list that can't be
/// fetched leaves the one kept.
pub async fn refresh_blocklist(config: &Config) {
    let path = blocklist_path(config);
    if !blocklist_on(config) || config.sandbox.blocklist_urls.is_empty() {
        if path.exists() {
            let _ = std::fs::remove_file(&path);
            tracing::info!(target: "bipbridge::sandbox", "blocklist removed (not on this place)");
        }
        return;
    }
    if age(&path).is_some_and(|a| a < BLOCKLIST_EVERY) {
        return;
    }
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(60)).build() {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut names = std::collections::BTreeSet::new();
    let mut fetched = 0;
    for url in &config.sandbox.blocklist_urls {
        let body = async {
            let response = client.get(url).send().await.ok()?.error_for_status().ok()?;
            let bytes = response.bytes().await.ok()?;
            (bytes.len() <= BLOCKLIST_MAX).then(|| String::from_utf8_lossy(&bytes).into_owned())
        }
        .await;
        match body {
            Some(text) => {
                parse_list(&text, &mut names);
                fetched += 1;
            }
            None => tracing::warn!(target: "bipbridge::sandbox", url = %url, "blocklist not fetched (the last one kept)"),
        }
    }
    if fetched == 0 || names.is_empty() {
        return;
    }
    let mut body = String::from("# piracy and torrent trackers (send_guard.rs), one name per line\n");
    for name in &names {
        body.push_str(name);
        body.push('\n');
    }
    let tmp = path.with_extension("part");
    let written = std::fs::create_dir_all(crate::sandbox_sites::policy_dir(config))
        .and_then(|_| std::fs::write(&tmp, body))
        .and_then(|_| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => tracing::info!(target: "bipbridge::sandbox", names = names.len(), "blocklist updated"),
        Err(exc) => tracing::warn!(target: "bipbridge::sandbox", error = %exc, "blocklist not written"),
    }
}

/// The blocklist checked every six hours where the sandboxes are isolated (and when the proxy is set up, sandbox.rs).
pub async fn watch(state: AppState) {
    loop {
        tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        let config = state.config();
        if crate::sandbox::engine(&config).is_some() {
            refresh_blocklist(&config).await;
        }
    }
}

/// Today's counts per agent, as the proxy wrote them (`{sent, received, unvalidated, connections, held}`, bytes).
pub fn usage(config: &Config, agent: &str) -> Value {
    let path = crate::sandbox_sites::reports_dir(config).join(USAGE_FILE);
    let Ok(file) = crate::sandbox::open_plain(&path) else { return Value::Null };
    let mut text = String::new();
    {
        use std::io::Read;
        let _ = file.take(1024 * 1024).read_to_string(&mut text);
    }
    let Ok(all) = serde_json::from_str::<Value>(&text) else { return Value::Null };
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if all.get("day").and_then(Value::as_str) != Some(today.as_str()) {
        return Value::Null;
    }
    all.get("agents").and_then(|a| a.get(agent)).cloned().unwrap_or(Value::Null)
}

// --- the browser ---------------------------------------------------------------------------------------------------

/// `10.233.80.0/24` → `10.233.82.0/24`: the browsers' network, after the connectors'.
pub fn browser_subnet(subnet: &str) -> Option<String> {
    let connectors = crate::sandbox::connector_subnet(subnet)?;
    crate::sandbox::connector_subnet(&connectors)
}

/// The proxy's address on the browsers' network.
pub fn browser_proxy_ip(subnet: &str) -> Option<String> {
    crate::sandbox::proxy_ip(&browser_subnet(subnet)?)
}

/// Whether the agents' browsers go through the proxy here: the sandboxes isolated with podman, the browsers run by
/// podman too (the networks are podman's).
pub fn browser_routed(config: &Config) -> bool {
    crate::browsers::runtime(config).is_some_and(|r| routed_with(config, &r))
}

fn routed_with(config: &Config, engine: &str) -> bool {
    config.browser.enabled
        && crate::sandbox::engine(config).is_some()
        && std::path::Path::new(engine).file_name().is_some_and(|n| n == "podman")
}

/// The address of an agent's browser on the browsers' network, from its DevTools port (each browser has two ports
/// in a row from `first_port`): `.10` and up. None past the network's room.
pub fn browser_ip(config: &Config, cdp_port: u16) -> Option<String> {
    let subnet = browser_subnet(&config.sandbox.subnet)?;
    let index = (cdp_port as u32).checked_sub(config.browser.first_port as u32)? / 2;
    let host = 10 + index;
    if host > 250 {
        return None;
    }
    let base = subnet.split('/').next()?.rsplit_once('.')?.0.to_string();
    Some(format!("{base}.{host}"))
}

/// The browser's flags and network when it goes through the proxy: (podman network argument, Chrome's proxy flag).
pub fn browser_egress(config: &Config, engine: &str, cdp_port: u16) -> Option<(String, String)> {
    if !routed_with(config, engine) {
        return None;
    }
    let ip = browser_ip(config, cdp_port)?;
    let proxy = browser_proxy_ip(&config.sandbox.subnet)?;
    Some((
        format!("{BROWSER_NETWORK}:ip={ip}"),
        format!("--proxy-server=http://{proxy}:{}", crate::sandbox::PROXY_PORT),
    ))
}

/// The policy's `browsers`: each browser's address → its agent.
pub fn browsers_map(config: &Config) -> serde_json::Map<String, Value> {
    let mut out = serde_json::Map::new();
    if !browser_routed(config) {
        return out;
    }
    for name in config.agents.keys() {
        if let Some(ip) = crate::browsers::ports_of(config, name)
            .and_then(|ports| ports.get("cdp").copied())
            .and_then(|cdp| browser_ip(config, cdp))
        {
            out.insert(ip, json!(name));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(cloud: bool, sandbox: Value) -> Config {
        crate::config::parse_config(
            &json!({"bridge_key": "k".repeat(32), "data_dir": "/tmp/bip-send-guard-test", "cloud": cloud,
                    "sandbox": sandbox}),
            None,
        )
        .unwrap()
    }

    #[test]
    fn the_offer_has_its_limits_a_self_install_only_the_guard() {
        let offer = config(true, json!({}));
        assert_eq!(
            limits(&offer),
            json!({"send_site_hour": 65536, "send_day": 262144, "download_day": 2048u64 * 1024 * 1024,
                   "connections_minute": 300})
        );
        assert_eq!(cpus(&offer), Some(1.5));
        assert!(blocklist_on(&offer));
        let home = config(false, json!({}));
        assert_eq!(
            limits(&home),
            json!({"send_site_hour": 65536, "send_day": 262144, "download_day": 0, "connections_minute": 0})
        );
        assert_eq!(cpus(&home), None);
        assert!(!blocklist_on(&home));
        // set by hand, either way
        let set = config(
            false,
            json!({"send_site_kb_hour": 128, "send_day_kb": 1024, "download_day_mb": 500, "connections_per_minute": 60,
                   "cpus": 2.0, "blocklist": true}),
        );
        assert_eq!(limits(&set)["send_site_hour"], json!(131072));
        assert_eq!(limits(&set)["download_day"], json!(500u64 * 1024 * 1024));
        assert_eq!((cpus(&set), blocklist_on(&set)), (Some(2.0), true));
        let off = config(true, json!({"cpus": 0, "download_day_mb": 0, "blocklist": false}));
        assert_eq!((cpus(&off), blocklist_on(&off), limits(&off)["download_day"].clone()), (None, false, json!(0)));
        // the sandbox's own arguments: its share of the processors on the offer
        assert!(crate::sandbox::own_args(&offer).contains(&"--cpus=1.5".to_string()));
        assert!(!crate::sandbox::own_args(&home).iter().any(|a| a.starts_with("--cpus")));
    }

    #[test]
    fn lists_in_their_usual_forms() {
        let mut names = std::collections::BTreeSet::new();
        parse_list(
            "# Title: piracy\n0.0.0.0 thepiratebay.org\n1337x.to\n||rarbg.example^\nlocalhost\n0.0.0.0 10.0.0.1\n \
             ! adblock comment\nTracker.OpenTrackr.org.\n",
            &mut names,
        );
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(names, vec!["1337x.to", "rarbg.example", "thepiratebay.org", "tracker.opentrackr.org"]);
    }

    #[test]
    fn each_browser_its_own_address() {
        let c = config(true, json!({}));
        assert_eq!(browser_subnet("10.233.80.0/24").as_deref(), Some("10.233.82.0/24"));
        assert_eq!(browser_proxy_ip("10.233.80.0/24").as_deref(), Some("10.233.82.2"));
        assert_eq!(browser_ip(&c, c.browser.first_port).as_deref(), Some("10.233.82.10"));
        assert_eq!(browser_ip(&c, c.browser.first_port + 2).as_deref(), Some("10.233.82.11"));
        assert_eq!(browser_ip(&c, c.browser.first_port - 2), None);
        assert_eq!(browser_ip(&c, c.browser.first_port + 2 * 241), None);
    }
}
