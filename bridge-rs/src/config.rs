//! Bridge configuration (TOML).
//!
//! Default path: `~/.config/hermes-ios/bridge.toml`, overridable with `BRIDGE_CONFIG`.
//! Secrets may be given inline (`bridge_key`, `hermes_key`, `ntfy_token`) or through a file
//! (`bridge_key_file`, `hermes_key_file`, `ntfy_token_file`), e.g. a systemd credential.
//!
//! `parse_config` works on the TOML document as a JSON value (`serde_json::Value`, what Python's `tomllib` gives
//! as dicts), with Python's own conversions (`str()`, `int()`, `float()`, truthiness), so the same file gives the
//! same configuration. The configuration changes at runtime (agents created, moved, deleted; a voice picked):
//! the running bridge holds it in a [`SharedConfig`].

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::py;

pub const DEFAULT_CONFIG_PATH: &str = "~/.config/hermes-ios/bridge.toml";
pub const DEFAULT_DATA_DIR: &str = "~/.local/share/hermes-bridge";
pub const PLACEHOLDER_PREFIXES: [&str; 3] = ["CHANGE-ME", "CHANGEME", "<"];

/// Apple's push hosts, by environment (`[apns] base_url` overrides both, for tests).
pub const APNS_HOSTS: [(&str, &str); 2] =
    [("production", "https://api.push.apple.com"), ("sandbox", "https://api.sandbox.push.apple.com")];
/// Google's FCM API and OAuth token endpoint (`[fcm] base_url` / `token_url` override them, for tests).
pub const FCM_BASE_URL: &str = "https://fcm.googleapis.com";
pub const MET_URL: &str = "https://api.met.no/weatherapi/locationforecast/2.0/complete";
pub const GEOCODER_URL: &str = "https://nominatim.openstreetmap.org/search";
pub const ROUTES_URL: &str = "https://routes.googleapis.com/directions/v2:computeRoutes";
pub const FCM_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

impl ConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        ConfigError(message.into())
    }
}

type Result<T> = std::result::Result<T, ConfigError>;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct AgentConfig {
    pub name: String,
    pub display_name: String,
    pub hermes_url: String,
    pub hermes_key: String,
    pub ntfy_topic: Option<String>,
    pub ntfy_token: Option<String>,
    pub upload_dir_host: Option<String>,
    pub upload_dir_container: Option<String>,
    pub voice: Option<String>,
    /// Its voice on the person's own voice server (`server:<lang>/<id>`), for the uses that go there.
    pub server_voice: Option<String>,
    /// Its look in the apps (the iPhone's choice, kept here so the other apps show the same Bip): a category id and
    /// an optional custom colour (`#RRGGBB`).
    pub category: Option<String>,
    pub color: Option<String>,
    /// Its Bip's costume chosen in an app (« Tenue », a costume id of the apps' pixel.json), and whether it dresses up
    /// for the events (Halloween, Noël…): None or true, yes. Left out of the serialized config when unset (the
    /// Python bridge's golden files have no such keys).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outfit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub costumes: Option<bool>,
    /// The agent's Hermes address as the app reaches it on the tailnet (`tailscale serve`), for the pairing QR code.
    pub public_url: Option<String>,
    /// The agent has its own space (container, sandbox): it may hold credentials from the vault. Agents of the
    /// shared hub never do.
    pub isolated: bool,
    /// How to run Python in the agent's Hermes (e.g. `podman exec -i hermes-vie /opt/hermes/venv/bin/python`)
    /// and its HERMES_HOME as that Python sees it: the vault writes the agent's items there.
    pub python: Vec<String>,
    pub hermes_home: Option<String>,
    /// Hermes' code folder, when that Python needs it (`hermes_bootstrap`).
    pub hermes_root: Option<String>,
    /// Its own space (spaces.py): the HERMES_HOME of the gateway that serves it alone.
    pub space: Option<String>,
    /// Its tools run in a sandbox container (or its space is a container of its own).
    pub sandboxed: bool,
    /// A viewer page of its browser set up by hand (e.g. a noVNC on the tailnet): the app opens it as « Bureau ».
    pub desktop_url: Option<String>,
    /// Its browser's VNC server set up by hand (ws://…/websockify or tcp://host:port) and the file holding its VNC
    /// password: the bridge logs in itself and relays the desktop to the app (Face ID there), see desktop.py.
    pub desktop_vnc: Option<String>,
    pub desktop_vnc_password_file: Option<String>,
    /// Set up by hand, its administrator mounts the shared folders (shares.py) when it starts: `"external"`, and the
    /// command to run after the person changes them (e.g. restarting its container once it is idle).
    pub shares: String,
    pub shares_apply: Option<String>,
    /// Set up by hand: the HERMES_HOME of the gateway serving it, on this machine (its profile is profiles/<name>).
    /// The app may then change its personality, language and thinking — files the bridge writes there; its tools,
    /// its container and its deletion stay with its administrator.
    pub host_home: Option<String>,
}

pub const VOICE_USES: [&str; 3] = ["live", "replies", "reading"];

/// The person's own voice model: any server with OpenAI's `POST /v1/audio/speech` (Fish Audio Drama 3 on
/// audio.cpp, Kokoro, Kyutai…). Its voices are `server:<lang>/<id>`; Pocket's Bips stand in when it is down.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceServerConfig {
    pub url: String,
    pub model: String,
    /// The rate of the raw PCM it streams (`response_format: pcm`, audio.cpp: its model's, 44100 for Drama 3); None:
    /// it gets whole WAV files.
    pub stream_rate: Option<u32>,
    /// Its voices, when it cannot list them itself (`GET /v1/audio/voices`).
    pub voices: Vec<String>,
    /// What it speaks: the Live, the replies to voice notes, the reading of messages and reports. The rest: the Bips.
    pub uses: IndexMap<String, bool>,
    /// Seconds a Live sentence or a spoken reply waits for its first sound (plus the text's own length when the
    /// server gives whole files) before the agent's Bip says it instead: a busy GPU never leaves the person in silence.
    pub first_audio_seconds: f64,
}

impl VoiceServerConfig {
    pub fn new(url: impl Into<String>) -> Self {
        VoiceServerConfig {
            url: url.into(),
            model: "tts-1".into(),
            stream_rate: None,
            voices: Vec::new(),
            uses: VOICE_USES.iter().map(|u| (u.to_string(), true)).collect(),
            first_audio_seconds: 5.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApnsConfig {
    pub team_id: String,
    pub key_id: String,
    pub p8_path: String,
    pub bundle_id: String,
    /// Default for devices registered without one.
    pub environment: String,
    /// Tests only: another host instead of Apple's (`base_url`: both environments; `sandbox_url`: the sandbox's,
    /// before `base_url`). Empty: Apple's ([`APNS_HOSTS`]).
    pub base_url: String,
    pub sandbox_url: String,
}

impl Default for ApnsConfig {
    fn default() -> Self {
        ApnsConfig {
            team_id: String::new(),
            key_id: String::new(),
            p8_path: String::new(),
            bundle_id: "com.kerfystudio.bipagents".into(),
            environment: "production".into(),
            base_url: String::new(),
            sandbox_url: String::new(),
        }
    }
}

impl ApnsConfig {
    pub fn enabled(&self) -> bool {
        !self.team_id.is_empty() && !self.key_id.is_empty() && !self.p8_path.is_empty() && !self.bundle_id.is_empty()
    }

    /// The APNs host for a device's environment (`sandbox_url`, `base_url` when set).
    pub fn host(&self, environment: &str) -> String {
        if environment == "sandbox" && !self.sandbox_url.is_empty() {
            return self.sandbox_url.clone();
        }
        if !self.base_url.is_empty() {
            return self.base_url.clone();
        }
        let env = if environment == "sandbox" { "sandbox" } else { "production" };
        APNS_HOSTS.iter().find(|(e, _)| *e == env).map(|(_, h)| h.to_string()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limits {
    pub upload_max_bytes: u64,
    pub upload_retention_days: i64,
    pub upload_file_mode: u32,
    pub upload_dir_mode: u32,
    pub sentence_max_chars: usize,
    pub tts_max_chars: usize,
    pub tts_cache_entries: usize,
    pub watch_max_seconds: u64,
    pub purge_interval_seconds: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            upload_max_bytes: 50 * 1024 * 1024,
            upload_retention_days: 30,
            upload_file_mode: 0o640,
            upload_dir_mode: 0o750,
            sentence_max_chars: 180,
            tts_max_chars: 1000,
            tts_cache_entries: 256,
            watch_max_seconds: 7200,
            purge_interval_seconds: 6 * 3600,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OutboxConfig {
    pub db_path: String,
    pub audio_dir: String,
    pub retention_days: i64,
    pub audio_max_chars: usize,
    pub audio_wait_seconds: f64,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        OutboxConfig {
            db_path: format!("{DEFAULT_DATA_DIR}/bridge.db"),
            audio_dir: format!("{DEFAULT_DATA_DIR}/audio"),
            retention_days: 90,
            audio_max_chars: 2000,
            audio_wait_seconds: 8.0,
        }
    }
}

/// The person's own Hermes, managed by this bridge (an install made by the installer): lets the app create
/// agents. Empty `bin` = agents are not managed here (set up by hand, e.g. in containers of their own).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct HermesInstall {
    pub bin: String,
    pub home: String,
    /// The multiplex gateway, local (http://127.0.0.1:8642).
    pub hub_url: String,
    /// The same on the tailnet, given to the app.
    pub public_hub_url: String,
    /// Hermes in a container: its HERMES_HOME as the agents see it (`home` is the same folder on the host), and
    /// `bin` a wrapper that runs `hermes` inside the container.
    pub container_home: String,
    /// How to run Hermes' own Python, and its code folder (the vault writes the agents' items with it).
    pub python: Vec<String>,
    pub root: String,
    /// The agents cannot read this bridge's files (their tools run in a sandbox, or their Hermes in a container that
    /// does not see them). Without it the bridge keeps no credentials: an agent with a terminal or the file tools,
    /// running under the same account, could read the vault, its key and every agent's key in this file.
    pub isolated: bool,
}

impl HermesInstall {
    pub fn enabled(&self) -> bool {
        !self.bin.is_empty() && !self.hub_url.is_empty()
    }
}

/// Agents' own spaces (spaces.py): where they live, their local ports, and how their gateway is started.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpacesConfig {
    pub root: String,
    pub first_port: u16,
    /// "auto": podman/docker sandbox when found; "none": never.
    pub sandbox: String,
    /// Own commands instead of systemd/launchd: "{agent}" and "{home}" filled in.
    pub start: Option<String>,
    pub stop: Option<String>,
    /// A space in a container of its own (start/stop commands): its HERMES_HOME as the agent sees it, for the
    /// attachments' folder (e.g. /home/hermes/.hermes). Empty: the same path as on this host.
    pub container_home: String,
    /// own_space.rs `tidy_at_start`: seconds after the bridge starts before the agents other people talk to that
    /// still live in the shared hub are moved out (default 300; the tests shorten it).
    #[serde(skip)]
    pub tidy_after: f64,
}

impl Default for SpacesConfig {
    fn default() -> Self {
        SpacesConfig {
            root: format!("{DEFAULT_DATA_DIR}/spaces"),
            first_port: 8660,
            sandbox: "auto".into(),
            start: None,
            stop: None,
            container_home: String::new(),
            tidy_after: 300.0,
        }
    }
}

/// The agents' sandboxes (sandbox.rs): no folder of this machine inside, no internet but the package registries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SandboxConfig {
    /// "auto": on Linux with podman (rootless); "on": wherever podman is found (tests, or a podman whose volumes this
    /// account can write); "off": the sandboxes mount the agents' folders as before (Hermes then asks before every
    /// risky command).
    pub isolate: String,
    /// The sandboxes' own network, without a route out: an IPv4 /24; the registries' proxy takes its `.2`.
    pub subnet: String,
    /// The only hosts the sandboxes reach, through the proxy: HTTPS (443), and plain HTTP (80) for Debian's apt.
    pub registries: Vec<String>,
    /// The proxy's image (default: the agents' sandbox image, already on this machine).
    pub image: String,
    /// The send guard (send_guard.rs): KB an agent may send per hour to one site its person did not validate, and per
    /// day across such sites (None: the defaults, 64 and 256).
    pub send_site_kb_hour: Option<u64>,
    pub send_day_kb: Option<u64>,
    /// The offer's limits (None: on for an offer place, off on a self-install; 0: off): MB an agent may download per
    /// day, new connections per agent per minute, CPUs per sandbox (podman `--cpus`), the piracy/tracker blocklist.
    pub download_day_mb: Option<u64>,
    pub connections_per_minute: Option<u64>,
    pub cpus: Option<f64>,
    pub blocklist: Option<bool>,
    /// Where the blocklist comes from (domains, one per line, `#` comments; fetched weekly).
    pub blocklist_urls: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            isolate: "auto".into(),
            subnet: "10.233.80.0/24".into(),
            registries: crate::sandbox::REGISTRIES.iter().map(|s| s.to_string()).collect(),
            image: String::new(),
            send_site_kb_hour: None,
            send_day_kb: None,
            download_day_mb: None,
            connections_per_minute: None,
            cpus: None,
            blocklist: None,
            blocklist_urls: crate::send_guard::BLOCKLIST_URLS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Each agent's visible browser (browsers.py): a real Chrome on a screen of its own, in a container of its own.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrowserConfig {
    /// Every agent this install creates gets one.
    pub enabled: bool,
    /// "podman" or "docker" (default: the one found).
    pub runtime: String,
    pub image: String,
    /// Its DevTools and VNC ports, on 127.0.0.1 only.
    pub first_port: u16,
    /// How many from first_port: this install's own range (others on the machine have theirs).
    pub ports: u16,
    pub root: String,
    /// "auto": /dev/dri when there is one; "none": software rendering.
    pub gpu: String,
    /// e.g. "fr-FR" (default: the bridge's locale).
    pub language: String,
    /// e.g. "Europe/Paris" (default: the bridge's).
    pub timezone: String,
    /// Started when the agent uses it, stopped after this long unused; 0: always on.
    pub idle_minutes: u32,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        BrowserConfig {
            enabled: false,
            runtime: String::new(),
            image: "bipagents-browser".into(),
            first_port: 9600,
            ports: 400,
            root: format!("{DEFAULT_DATA_DIR}/browsers"),
            gpu: "auto".into(),
            language: String::new(),
            timezone: String::new(),
            idle_minutes: 10,
        }
    }
}

/// This bridge relays the notifications of other installs on the server, which have no APNs key of their own
/// (another person's bridge): each install gets its own key, listed here as key -> install name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RelayConfig {
    pub clients: IndexMap<String, String>,
    pub per_hour: i64,
}

impl Default for RelayConfig {
    fn default() -> Self {
        RelayConfig { clients: IndexMap::new(), per_hour: 600 }
    }
}

impl RelayConfig {
    pub fn enabled(&self) -> bool {
        !self.clients.is_empty()
    }
}

/// The local-network door (no Tailscale): the same bridge over HTTPS with a self-signed certificate the app pins,
/// Hermes relayed under /hermes/<agent>/. Off by default: everything else only listens on 127.0.0.1.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LanConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// Advertised to the app; detected when empty.
    pub address: Option<String>,
    pub cert_dir: String,
}

impl Default for LanConfig {
    fn default() -> Self {
        LanConfig { enabled: false, host: "0.0.0.0".into(), port: 8650, address: None, cert_dir: String::new() }
    }
}

/// The `weather` tool's providers (weather.rs). Their public services by default; tests point them at fakes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WeatherConfig {
    /// MET Norway's Locationforecast 2.0 (`complete`: hourly, then 6-hourly up to about 9 days).
    pub met_url: String,
    /// A Nominatim search endpoint (OpenStreetMap's public one by default: 1 request per second at most).
    pub geocoder_url: String,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        WeatherConfig { met_url: MET_URL.into(), geocoder_url: GEOCODER_URL.into() }
    }
}

/// The `route` tool's paid routing (routes.rs): with a Google Maps Platform key, durations and distances besides the
/// link to open the maps app. The key never reaches the agents.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MapsConfig {
    #[serde(skip)]
    pub google_key: Option<String>,
    /// Google's Routes API (`computeRoutes`); tests point it at a fake.
    pub routes_url: String,
    /// The maps' tiles (maps.rs): « osm » (OpenStreetMap, the only one for now; a paid provider can come later).
    pub tiles: String,
    /// The tiles' address, `{z}/{x}/{y}` filled in; tests point it at a fake.
    pub tile_url: String,
    /// The interactive maps in pages (map_tiles.rs): « openfreemap » (vector tiles through the bridge) or « off »
    /// (pages keep the drawn picture).
    pub vector: String,
    /// The vector provider's address (its style at `<url>/styles/<vector_style>`); tests point it at a fake.
    pub vector_url: String,
    pub vector_style: String,
}

impl Default for MapsConfig {
    fn default() -> Self {
        MapsConfig {
            google_key: None,
            routes_url: ROUTES_URL.into(),
            tiles: "osm".into(),
            tile_url: OSM_TILE_URL.into(),
            vector: "openfreemap".into(),
            vector_url: OPENFREEMAP_URL.into(),
            vector_style: "positron".into(),
        }
    }
}

pub const OSM_TILE_URL: &str = "https://tile.openstreetmap.org/{z}/{x}/{y}.png";
pub const OPENFREEMAP_URL: &str = "https://tiles.openfreemap.org";

/// `[notebook]`: the Carnet's nightly pass (notebook/tidy.rs): a small model, its prices and a hard monthly cap.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotebookConfig {
    /// The model's name at the install's model provider (empty: each agent's own model).
    pub model: String,
    /// Another OpenAI-compatible API than the agents' (with its `key`); empty: the agents' own router and key.
    pub base_url: String,
    #[serde(skip)]
    pub key: Option<String>,
    /// Euros per million tokens, in and out (to count what each run costs).
    pub price_in: f64,
    pub price_out: f64,
    /// Euros per month at most for the nightly pass, per install: reached, it waits for the next month.
    pub monthly_cap: f64,
    /// The local hour it runs at (0–23).
    pub hour: u32,
    /// What one run sends at most (characters of conversation and of « À ranger »).
    pub max_input_chars: usize,
    /// Purchases worth a warranty reminder and shown among the « important » ones: goods from this price (euros, or
    /// the same number in another currency) — notebook/records.rs.
    pub warranty_min_eur: f64,
}

impl Default for NotebookConfig {
    fn default() -> Self {
        NotebookConfig {
            // The agents' own model by default: a name that exists at their provider, whichever it is (a fixed default
            // such as `mistral-small-latest` is unknown at EUrouter). Prices on the high side of cheap models (e.g.
            // DeepSeek V4.1 Flash), so the cap holds even when the model is a bit dearer than the usual small one.
            model: String::new(),
            base_url: String::new(),
            key: None,
            price_in: 0.50,
            price_out: 1.50,
            monthly_cap: 0.10,
            hour: 3,
            max_input_chars: 12000,
            warranty_min_eur: 50.0,
        }
    }
}

/// `[jobs]`: long work handed to background tasks (jobs.rs, decided 2026-10-22). An agent starts a task by itself only
/// when the person asked for that work, within `direct_budget_eur` (counted at the `[notebook]` prices, the bridge's
/// one price list); above it, it proposes. The bridge's safety net (runs.rs) steers once into a conversation's run
/// that goes past `handoff_tool_calls` tool calls or `handoff_seconds`, asking the agent to hand the rest over. Never
/// while a sub-agent (`delegate_task`) works for the run; a run browsing with the agent's visible browser (a browser
/// tool among its last `handoff_browser_window` calls: the person watches it live) has its own thresholds,
/// `handoff_browser_tool_calls` / `handoff_browser_seconds` (0: never steered, the default). A steered run that ends
/// without handing over has its task started by the bridge (`handoff_rescue`, 2026-10-24).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobsConfig {
    /// Euros of tokens a task the person asked for may take without their go (≈ 0.20 €).
    pub direct_budget_eur: f64,
    /// The safety net on or off.
    pub handoff: bool,
    pub handoff_tool_calls: u32,
    pub handoff_seconds: f64,
    /// A browsing run's thresholds (0: none of that kind; both 0: a browsing run is never steered).
    pub handoff_browser_tool_calls: u32,
    pub handoff_browser_seconds: f64,
    /// How many of the run's last tool calls are looked at to tell a browsing run.
    pub handoff_browser_window: u32,
    /// A steered run that ended without handing over: the bridge starts (or proposes) the task itself.
    pub handoff_rescue: bool,
}

impl Default for JobsConfig {
    fn default() -> Self {
        JobsConfig {
            direct_budget_eur: 0.20,
            handoff: true,
            handoff_tool_calls: 15,
            handoff_seconds: 180.0,
            handoff_browser_tool_calls: 0,
            handoff_browser_seconds: 0.0,
            handoff_browser_window: 5,
            handoff_rescue: true,
        }
    }
}

/// `[rooms]`: group discussions between the person's agents (rooms.rs, 2026-10-25). The router asks a small model
/// (`router_model`, the install's model as the Carnet finds it) only when the @names and the rule found nobody, at
/// most `router_daily_max` times a day; an agent's @ in a reply gives another agent a turn, up to `max_hops` hops past
/// the person's message (at most 2); a turn not done after `turn_seconds` is stopped (unless it waits for an approval).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RoomsConfig {
    pub router_model: bool,
    pub router_daily_max: u32,
    pub max_hops: u32,
    pub turn_seconds: f64,
}

impl Default for RoomsConfig {
    fn default() -> Self {
        RoomsConfig { router_model: true, router_daily_max: 300, max_hops: 1, turn_seconds: 300.0 }
    }
}

/// `[guide]` (guide_team.rs): a team proposed in one card. `team_max`: agents on one team card (1 to 6, 6 by
/// default); `agents_max`: the place's agents at most, the hook the offer may set (0, the default: no limit; the card
/// then shows how many places are left); `first_turns`: each new team agent writes its first message (on by default).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GuideConfig {
    pub team_max: usize,
    pub agents_max: usize,
    pub first_turns: bool,
}

impl Default for GuideConfig {
    fn default() -> Self {
        GuideConfig { team_max: 6, agents_max: 0, first_turns: true }
    }
}

/// `[archive_records]`: records filled from an agent's archives (archives/records_fill.rs): the models the person may
/// pick for the one-time pass, the price counted for a model whose price is unknown, the caps of a sample and of the
/// nightly pass on new documents.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArchiveRecordsConfig {
    /// `[[archive_records.models]]`: {id, label, price_in, price_out} at the agents' own provider.
    pub models: Vec<ExtractModel>,
    /// Euros per million tokens counted for a model of unknown price (on the high side: the cap holds).
    pub unknown_price_in: f64,
    pub unknown_price_out: f64,
    /// Euros a sample may take at most.
    pub sample_cap_eur: f64,
    /// Euros the nightly pass on new documents may take per archive and night (a local model: free).
    pub nightly_cap_eur: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtractModel {
    pub id: String,
    pub label: String,
    pub price_in: f64,
    pub price_out: f64,
}

impl Default for ArchiveRecordsConfig {
    fn default() -> Self {
        ArchiveRecordsConfig {
            models: Vec::new(),
            unknown_price_in: 3.0,
            unknown_price_out: 15.0,
            sample_cap_eur: 1.0,
            nightly_cap_eur: 0.05,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    pub bridge_key: String,
    pub host: String,
    pub port: u16,
    pub log_level: String,
    /// Kyutai TTS 1.6B (GPU): the human voice and the podcast. None = off (Pocket only, e.g. on a Mac mini or a VPS).
    pub kyutai_url: Option<String>,
    pub kyutai_timeout: f64,
    pub default_voice: String,
    /// Kyutai Pocket TTS (CPU) for the Bips' voices, used by voices named "pocket:<name>". None = off.
    pub pocket_url: Option<String>,
    /// The version of the server's Pocket engine (`[pocket] engine`, default "3.3.0"; the installer's migration
    /// pocket-engine writes "4.0.1" once the server's Pocket says it runs it): below 4 (3.3.0) the names and
    /// English words are respelled for it (pronounce.rs); from 4 on, the model reads them from the raw text.
    pub pocket_engine: String,
    /// The person's own voice model (`[voice_server]`; an older `[drama] url` is one). None = the Bips only.
    pub voice_server: Option<VoiceServerConfig>,
    /// Words the voices say wrong, respelled by language (`[pronunciations.fr] Garmin = "Garmine"`).
    pub pronunciations: IndexMap<String, IndexMap<String, String>>,
    /// Scheduled-task replies read from Hermes sessions into the outbox (see cronwatch.py).
    pub cron_watch: bool,
    pub cron_interval_seconds: f64,
    pub cron_window_hours: f64,
    /// A background sub-agent's report left without a reply: the agent writes the message itself (handback.rs).
    pub cron_subtask_handback: bool,
    /// Files the agents point to with « MEDIA:<path> » lines: agent-side prefix -> host directory.
    pub media_roots: IndexMap<String, String>,
    pub ntfy_url: String,
    pub push_previews: bool,
    pub watch_followed_runs: bool,
    pub apns: ApnsConfig,
    /// The Firebase project's service account (JSON), for the Android app's notifications.
    pub fcm_service_account: Option<String>,
    /// Tests only: Google's FCM API and OAuth token endpoint replaced (`[fcm] base_url`, `[fcm] token_url`; without
    /// `token_url`, the service account's `token_uri`, else Google's).
    pub fcm_base_url: String,
    pub fcm_token_url: Option<String>,
    pub outbox: OutboxConfig,
    pub limits: Limits,
    pub agents: IndexMap<String, AgentConfig>,
    pub lan: LanConfig,
    pub relay: RelayConfig,
    pub hermes: HermesInstall,
    pub spaces: SpacesConfig,
    pub browser: BrowserConfig,
    /// `[sandbox]` (sandbox.rs): the agents' sandboxes isolated. Not in the serialized form (the golden config).
    #[serde(skip)]
    pub sandbox: SandboxConfig,
    /// `[weather]` and `[maps]` (weather.rs, routes.rs): not in the serialized form, which matches the Python's.
    #[serde(skip)]
    pub weather: WeatherConfig,
    #[serde(skip)]
    pub maps: MapsConfig,
    /// `[notebook]` (notebook/tidy.rs): not in the serialized form either.
    #[serde(skip)]
    pub notebook: NotebookConfig,
    /// `[jobs]` (jobs.rs, runs.rs): not in the serialized form either.
    #[serde(skip)]
    pub jobs: JobsConfig,
    /// `[brake]` (loop_brake.rs): a turn going round in circles stopped. Not in the serialized form either.
    #[serde(skip)]
    pub brake: crate::loop_brake::Settings,
    /// `[rooms]` (rooms.rs): not in the serialized form either.
    #[serde(skip)]
    pub rooms: RoomsConfig,
    /// `[guide]` (guide_team.rs): not in the serialized form either.
    #[serde(skip)]
    pub guide: GuideConfig,
    /// `[connections]` (connections.rs): not in the serialized form either.
    #[serde(skip)]
    pub connections: crate::connections::Settings,
    /// `[watchers]` (watchers.rs): not in the serialized form either.
    #[serde(skip)]
    pub watchers: crate::watchers::Settings,
    /// `[agent_restart]` (agent_restart.rs): not in the serialized form either.
    #[serde(skip)]
    pub agent_restart: crate::agent_restart::Settings,
    /// `[link]` (link_wallet.rs): Link Agent Wallet, off unless switched on and configured. Not serialized either.
    #[serde(skip)]
    pub link: crate::link_wallet::Settings,
    /// `[archive_records]` (archives/records_fill.rs): not in the serialized form either.
    #[serde(skip)]
    pub archive_records: ArchiveRecordsConfig,
    /// `[vault] cards` (cards.rs): whether the person may turn card payments on in the app. "auto" (or "on"): yes,
    /// except on a place of the all-included offer or a « no computer » server; "off": never. Payments stay off
    /// until the person turns them on. Not in the serialized form either.
    #[serde(skip)]
    pub cards: String,
    /// `[payments]` (cards.rs): the offer's switch for agent card payments on its places. Not serialized either.
    #[serde(skip)]
    pub payments: PaymentsConfig,
    /// `[[archives]]` (archives/): folders an existing install connects to its agents, searched on demand. Their
    /// tables as written (checked by archives/). Not in the serialized form either.
    #[serde(skip)]
    pub archives: Vec<Value>,
    /// Notifications through another bridge's relay instead of APNs (an install without the .p8 key).
    pub push_relay_url: Option<String>,
    pub push_relay_key: Option<String>,
    /// The publisher's push relay (relay/): devices registered there give this bridge a relay token instead of
    /// their own (the app does it when this bridge has no Apple key). None: off.
    pub push_service_url: Option<String>,
    /// Optional check that the model answers (`GET <url>`, e.g. its /v1/models), for « model unreachable ».
    pub model_health_url: Option<String>,
    pub model_health_key: Option<String>,
    /// `[model] provider_order`: the providers EUrouter tries first for every model call, so their prompt cache works
    /// (model_route.rs; default entrim, melious; `[]`: EUrouter picks per call).
    pub model_provider_order: Vec<String>,
    /// The bridge's address on the tailnet (`tailscale serve`), for the pairing QR code.
    pub public_url: Option<String>,
    /// The bridge as the agents reach it, for messages between agents (peers.py). Default:
    /// http://127.0.0.1:<port>/peer; e.g. http://host.containers.internal:8643/peer for agents in containers.
    pub peer_url: Option<String>,
    /// A server the app created in the person's own account (« no computer »): the door is on the Internet, the
    /// agents are reached through it (/hermes/<agent>/), the web console is off and wrong keys are limited (guard.py).
    pub cloud: bool,
    /// Until the app claims that server (cloud_setup.py): the one-time pairing secret, and the installer's progress.
    pub setup_file: Option<String>,
    pub setup_progress: Option<String>,
    pub source_path: Option<String>,
}

/// `[payments]`: on a place of the all-included offer, whether agents may pay with a card the person keeps on their
/// phone (cards.rs, « expérimental » during the beta). Kerfy Studio writes it into the offer's places' bridge.toml; it
/// turns it on for everyone at once, and off the same way. Self-hosted installs ignore it (`[vault] cards`).
#[derive(Debug, Clone, PartialEq)]
pub struct PaymentsConfig {
    /// `offer_cards`: false by default. Off: the apps don't show card payments on the offer's places, and no agent
    /// pays with a card there (the hand-over, handover.rs, stays).
    pub offer_cards: bool,
    /// `offer_cap_max_eur`: the highest cap a person may set on a card there (150 € by default, 1 to 1000).
    pub offer_cap_max_eur: i64,
}

impl Default for PaymentsConfig {
    fn default() -> Self {
        PaymentsConfig { offer_cards: false, offer_cap_max_eur: 150 }
    }
}

impl Config {
    /// A configuration with every default (tests build on it).
    pub fn with_key(bridge_key: impl Into<String>) -> Self {
        Config {
            bridge_key: bridge_key.into(),
            host: "127.0.0.1".into(),
            port: 8643,
            log_level: "info".into(),
            kyutai_url: Some("http://127.0.0.1:8097".into()),
            kyutai_timeout: 120.0,
            default_voice: "5476".into(),
            pocket_url: None,
            pocket_engine: "3.3.0".into(),
            voice_server: None,
            pronunciations: IndexMap::new(),
            cron_watch: true,
            cron_interval_seconds: 30.0,
            cron_window_hours: 12.0,
            cron_subtask_handback: true,
            media_roots: IndexMap::new(),
            ntfy_url: "http://127.0.0.1:8645".into(),
            push_previews: false,
            watch_followed_runs: true,
            apns: ApnsConfig::default(),
            fcm_service_account: None,
            fcm_base_url: FCM_BASE_URL.into(),
            fcm_token_url: None,
            outbox: OutboxConfig::default(),
            limits: Limits::default(),
            agents: IndexMap::new(),
            lan: LanConfig::default(),
            relay: RelayConfig::default(),
            hermes: HermesInstall::default(),
            spaces: SpacesConfig::default(),
            browser: BrowserConfig::default(),
            sandbox: SandboxConfig::default(),
            weather: WeatherConfig::default(),
            maps: MapsConfig::default(),
            notebook: NotebookConfig::default(),
            jobs: JobsConfig::default(),
            brake: crate::loop_brake::Settings::default(),
            rooms: RoomsConfig::default(),
            guide: GuideConfig::default(),
            connections: crate::connections::Settings::default(),
            watchers: crate::watchers::Settings::default(),
            agent_restart: crate::agent_restart::Settings::default(),
            link: crate::link_wallet::Settings::default(),
            archive_records: ArchiveRecordsConfig::default(),
            cards: "auto".into(),
            payments: PaymentsConfig::default(),
            archives: Vec::new(),
            push_relay_url: None,
            push_relay_key: None,
            push_service_url: None,
            model_health_url: None,
            model_health_key: None,
            model_provider_order: crate::model_route::DEFAULT_ORDER.iter().map(|s| s.to_string()).collect(),
            public_url: None,
            peer_url: None,
            cloud: false,
            setup_file: None,
            setup_progress: None,
            source_path: None,
        }
    }

    /// The agent named `name` (case and surrounding spaces ignored).
    pub fn agent(&self, name: Option<&str>) -> Option<&AgentConfig> {
        let name = name?;
        if name.is_empty() {
            return None;
        }
        self.agents.get(&name.trim().to_lowercase())
    }

    /// The folder next to the database (`os.path.dirname(config.outbox.db_path)`): where the bridge keeps its
    /// other files (vault/, peers.json, console.json, briefs/…).
    pub fn data_dir(&self) -> String {
        py::dirname(&self.outbox.db_path)
    }
}

/// The running bridge's configuration: a snapshot for readers (`get`), changes made one at a time (`update`).
/// Python mutates its `Config` in place (`config.agents[name] = agent`); here a change makes a new snapshot, so a
/// reader never sees half a change and never holds a lock across an `await`.
#[derive(Clone)]
pub struct SharedConfig {
    current: Arc<ArcSwap<Config>>,
    writer: Arc<Mutex<()>>,
}

impl SharedConfig {
    pub fn new(config: Config) -> Self {
        SharedConfig { current: Arc::new(ArcSwap::from_pointee(config)), writer: Arc::new(Mutex::new(())) }
    }

    /// The configuration as it is now.
    pub fn get(&self) -> Arc<Config> {
        self.current.load_full()
    }

    /// Changes the configuration (writers wait for each other); returns what `change` returns.
    pub fn update<R>(&self, change: impl FnOnce(&mut Config) -> R) -> R {
        let _guard = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = (*self.current.load_full()).clone();
        let out = change(&mut next);
        self.current.store(Arc::new(next));
        crate::mcp_sessions::poke(); // an agent's tools may have changed: its open MCP streams check now
        out
    }

    /// One agent's configuration replaced (or added), as Python's `config.agents[agent.name] = agent`.
    pub fn put_agent(&self, agent: AgentConfig) {
        self.update(|c| {
            c.agents.insert(agent.name.clone(), agent);
        });
    }

    /// `config.agents.pop(name, None)` (the order of the others kept).
    pub fn remove_agent(&self, name: &str) -> Option<AgentConfig> {
        self.update(|c| c.agents.shift_remove(name))
    }
}

// -- the config file on disk ----------------------------------------------------------------------
//
// Several programs change bridge.toml: this bridge (an agent made, moved, renamed or deleted, a voice picked…), its
// command line (`browser add`, `adopt`, `space enable`) and the installer (migrations, family, moves, restores:
// installer/bipinstall/bridgeconf.py). On 2026-10-26 (aibox) rewrites running at once broke it twice, the second
// time leaving nothing but four agents' voices. Every writer now goes the same way ([`update_source`]):
// - `flock` on `bridge.toml.lock` beside it, held from the read to the rename (other processes), [`source_lock`] in
//   this one;
// - the new text checked ([`refused_change`]): it parses, keeps the bridge's key and every agent's address (but the
//   agent being removed), else nothing is written;
// - written to a temporary file in the same folder, synced, renamed over it (0600);
// - `bridge.toml.last-good` kept: what this bridge last loaded or wrote, put back at start when the file doesn't load
//   ([`load_config_at_start`]).

/// The lock file other processes take too (installer/bipinstall/bridgeconf.py uses the same name).
pub const LOCK_SUFFIX: &str = ".lock";
/// The copy the bridge last loaded or wrote.
pub const LAST_GOOD_SUFFIX: &str = ".last-good";
/// How long a writer waits for another's lock before giving up (a write takes milliseconds).
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// One writer at a time in this process (the file lock orders the processes): two at once left it cut in two (aibox
/// 2026-10-26: the app set every agent's voice at once; one rewrite read the file while another had just emptied it).
pub fn source_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `<file><suffix>` beside it (`bridge.toml.lock`, `bridge.toml.last-good`).
pub fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The config file's lock for other processes, held until the returned file is closed.
fn lock_source(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let fh = options.open(beside(path, LOCK_SUFFIX))?;
    same_owner(&fh, path)?;
    let deadline = std::time::Instant::now() + LOCK_WAIT;
    loop {
        match fh.try_lock() {
            Ok(()) => return Ok(fh),
            Err(std::fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{}: another program kept it locked", path.display()),
                ));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
    }
}

/// A file root makes (an administrator's command) goes to the config's owner: the bridge runs as that account.
fn same_owner(file: &std::fs::File, like: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return Ok(());
    }
    let folder = like.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let reference = std::fs::metadata(like).or_else(|_| std::fs::metadata(folder))?;
    std::os::unix::fs::fchown(file, Some(reference.uid()), Some(reference.gid()))
}

/// `text` as the whole file: a temporary file beside it (0600 from the start: it holds keys), synced, renamed over
/// it, so a reader never sees it half written.
fn write_file(target: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("bridge.toml");
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = target.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut fh = options.open(&temporary)?;
        fh.write_all(text.as_bytes())?;
        fh.sync_all()?;
        same_owner(&fh, target)?;
        std::fs::rename(&temporary, target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written?;
    if let Some(folder) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        let _ = std::fs::File::open(folder).and_then(|d| d.sync_all()); // the rename itself on disk
    }
    Ok(())
}

/// The bridge's config file changed by `change` (given its text, read under the lock; "" when there is none): the
/// new text checked, then written whole. No agent may go: see [`update_source_removing`].
pub fn update_source(path: &str, change: impl FnOnce(&str) -> std::io::Result<String>) -> std::io::Result<()> {
    update_source_removing(path, &[], change)
}

/// [`update_source`] where the agents named in `removing` may go (an agent deleted, its section rewritten).
pub fn update_source_removing(
    path: &str,
    removing: &[&str],
    change: impl FnOnce(&str) -> std::io::Result<String>,
) -> std::io::Result<()> {
    let target = Path::new(path);
    let _inside = source_lock();
    let _outside = lock_source(target)?;
    let old = match std::fs::read_to_string(target) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let new = change(&old)?;
    if new == old {
        return Ok(());
    }
    if let Some(why) = refused_change(&old, &new, removing) {
        tracing::error!(target: "bipbridge::config", file = %path, reason = %why, "config change refused: the file stays as it was");
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{path}: change refused, {why}")));
    }
    write_file(target, &new)?;
    if loads(&new, path).is_ok() {
        keep_last_good(target, &new);
    }
    Ok(())
}

/// Why `new` mustn't replace `old` as the bridge's config, None when it may: it must parse, keep the bridge's key
/// (`bridge_key` or `bridge_key_file`) and every agent's `hermes_url` (but the agents in `removing`), and load in the
/// bridge when `old` did. Over a file that doesn't parse, only a file that loads is written.
pub fn refused_change(old: &str, new: &str, removing: &[&str]) -> Option<String> {
    let new_doc: toml::Table = match new.parse() {
        Ok(doc) => doc,
        Err(e) => return Some(format!("the new text isn't valid TOML ({})", e.message().trim())),
    };
    if old.trim().is_empty() {
        return None;
    }
    let Ok(old_doc) = old.parse::<toml::Table>() else {
        return match loads(new, "bridge.toml") {
            Ok(_) => None,
            Err(e) => Some(format!("the file doesn't parse and the new text wouldn't load either ({e})")),
        };
    };
    let has_key = |doc: &toml::Table| {
        ["bridge_key", "bridge_key_file"].iter().any(|k| doc.get(*k).is_some_and(|v| v.as_str() != Some("")))
    };
    if has_key(&old_doc) && !has_key(&new_doc) {
        return Some("it would lose bridge_key".into());
    }
    let agents = |doc: &toml::Table| -> IndexMap<String, bool> {
        match doc.get("agents") {
            Some(toml::Value::Table(t)) => t
                .iter()
                .map(|(name, v)| {
                    let url = v.get("hermes_url").and_then(|u| u.as_str()).is_some_and(|u| !u.trim().is_empty());
                    (name.trim().to_lowercase(), url)
                })
                .collect(),
            _ => IndexMap::new(),
        }
    };
    let (before, after) = (agents(&old_doc), agents(&new_doc));
    let going: Vec<String> = removing.iter().map(|n| n.trim().to_lowercase()).collect();
    for (name, had_url) in &before {
        if *had_url && !going.contains(name) && !after.get(name).copied().unwrap_or(false) {
            return Some(format!("it would lose agent {name} (or its hermes_url)"));
        }
    }
    if loads(old, "bridge.toml").is_ok() {
        if let Err(e) = loads(new, "bridge.toml") {
            return Some(format!("the bridge couldn't load it ({e})"));
        }
    }
    None
}

/// `text` loaded as the bridge would (TOML, then [`parse_config`]).
fn loads(text: &str, path: &str) -> Result<Config> {
    let doc: toml::Table =
        text.parse().map_err(|e: toml::de::Error| ConfigError(format!("invalid TOML in {path}: {}", e.message())))?;
    parse_config(&toml_to_json(toml::Value::Table(doc)), Some(path))
}

/// `bridge.toml.last-good` set to `text` (a file that loads: never a broken one).
fn keep_last_good(target: &Path, text: &str) {
    let good = beside(target, LAST_GOOD_SUFFIX);
    if std::fs::read_to_string(&good).is_ok_and(|kept| kept == text) {
        return;
    }
    if let Err(e) = write_file(&good, text) {
        tracing::warn!(target: "bipbridge::config", file = %good.display(), error = %e, "last good copy of the config not kept");
    }
}

/// Set when this bridge put its config's last good copy back at start (/health's `config_restored`).
static RESTORED: Mutex<Option<Value>> = Mutex::new(None);

/// `{"at", "broken"}` when the config was restored at start: when, and the name the broken file was kept under.
pub fn restored() -> Option<Value> {
    RESTORED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The config of a bridge that starts serving: loaded as [`load_config`] does, and kept as `bridge.toml.last-good`.
/// A file that doesn't load (cut, emptied, debris in it, gone), or that lost every agent the last good copy had, is
/// set aside as `bridge.toml.broken-<time>` and the last good copy put back, said loudly in the log and in /health.
/// With no last good copy, the error is today's; with one that doesn't load either, the error names the copies.
pub fn load_config_at_start(path: Option<&str>) -> Result<Config> {
    let path = match path {
        Some(p) => expand(p),
        None => config_path(),
    };
    let target = PathBuf::from(&path);
    let good = beside(&target, LAST_GOOD_SUFFIX);
    let (problem, first_error) = match read_config(&path) {
        Ok((text, config)) => match agents_gone(&config, &good) {
            None => {
                keep_last_good(&target, &text);
                return Ok(config);
            }
            Some(why) => (why, Ok(config)),
        },
        Err(e) => (e.0.clone(), Err(e)),
    };
    if !good.exists() {
        return first_error;
    }
    let good_path = good.to_string_lossy().into_owned();
    let backup = match read_config(&good_path) {
        Ok((text, _)) => text,
        Err(e) => {
            return Err(ConfigError(format!(
                "{problem}; its last good copy can't be used either ({e}). Copies kept beside it: {}",
                kept_copies(&target).join(", ")
            )));
        }
    };
    let broken = beside(&target, &format!(".broken-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let put_back = (|| {
        let _inside = source_lock();
        let _outside = lock_source(&target)?;
        let moved = target.exists();
        if moved {
            std::fs::rename(&target, &broken)?;
        }
        write_file(&target, &backup)?;
        Ok::<bool, std::io::Error>(moved)
    })();
    let moved = match put_back {
        Ok(moved) => moved,
        Err(e) => return Err(ConfigError(format!("{problem}; its last good copy {good_path} couldn't be put back: {e}"))),
    };
    let kept_as = if moved { broken.display().to_string() } else { "-".into() };
    tracing::error!(
        target: "bipbridge::config",
        file = %path,
        problem = %problem,
        kept_as = %kept_as,
        "CONFIG RESTORED: the bridge's config was broken, its last good copy is back"
    );
    let broken_name = broken.file_name().map(|n| Value::String(n.to_string_lossy().into_owned()));
    *RESTORED.lock().unwrap_or_else(|e| e.into_inner()) = Some(serde_json::json!({
        "at": crate::py::time_time(),
        "broken": if moved { broken_name.unwrap_or(Value::Null) } else { Value::Null },
    }));
    read_config(&path).map(|(_, c)| c)
}

/// Why a config that loads is no good all the same: it has no agent any more while its last good copy had some.
fn agents_gone(config: &Config, good: &Path) -> Option<String> {
    if !config.agents.is_empty() {
        return None;
    }
    let kept: toml::Table = std::fs::read_to_string(good).ok()?.parse().ok()?;
    let count = kept.get("agents").and_then(|a| a.as_table()).map(|a| a.len()).unwrap_or(0);
    (count > 0).then(|| format!("it has no agent any more (its last good copy has {count})"))
}

/// The copies of the config kept beside it (its last good one, broken ones set aside), as paths.
pub fn kept_copies(target: &Path) -> Vec<String> {
    let Some(name) = target.file_name().map(|n| n.to_string_lossy().into_owned()) else { return Vec::new() };
    let folder = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let (good, broken) = (format!("{name}{LAST_GOOD_SUFFIX}"), format!("{name}.broken-"));
    let mut found: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| *n == good || n.starts_with(&broken))
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found.iter().map(|n| folder.join(n).to_string_lossy().into_owned()).collect()
}

// -- parsing ---------------------------------------------------------------------------------------

fn expand(path: &str) -> String {
    py::expand(path)
}

/// `table.get(key)` (None for a missing key or a non-table).
fn get<'a>(table: &'a Value, key: &str) -> Option<&'a Value> {
    table.as_object().and_then(|t| t.get(key))
}

/// `data.get(key, {}) or {}`.
fn table<'a>(data: &'a Value, key: &str) -> &'a Value {
    static EMPTY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let empty = EMPTY.get_or_init(|| Value::Object(Map::new()));
    match get(data, key) {
        Some(v) if py::truthy(v) => v,
        _ => empty,
    }
}

/// `bool(table.get(key))` — present and truthy.
fn has(table: &Value, key: &str) -> bool {
    get(table, key).map(py::truthy).unwrap_or(false)
}

/// `str(table.get(key, default))`.
fn s(table: &Value, key: &str, default: &str) -> String {
    match get(table, key) {
        Some(v) => py::py_str(v),
        None => default.to_string(),
    }
}

/// `str(table[key]) if table.get(key) else None`.
fn opt_s(table: &Value, key: &str) -> Option<String> {
    if has(table, key) {
        get(table, key).map(py::py_str)
    } else {
        None
    }
}

/// `bool(table.get(key, default))`.
fn b(table: &Value, key: &str, default: bool) -> bool {
    get(table, key).map(py::truthy).unwrap_or(default)
}

fn value_error(message: String) -> ConfigError {
    ConfigError(message)
}

/// `int(table.get(key, default))`.
fn i(table: &Value, key: &str, default: i64) -> Result<i64> {
    match get(table, key) {
        Some(v) => py::py_int(v).map_err(value_error),
        None => Ok(default),
    }
}

/// `float(table.get(key, default))`.
fn f(table: &Value, key: &str, default: f64) -> Result<f64> {
    match get(table, key) {
        Some(v) => py::py_float(v).map_err(value_error),
        None => Ok(default),
    }
}

/// A count that may be left out (None), never below 0.
fn opt_u(table: &Value, key: &str) -> Result<Option<u64>> {
    match get(table, key) {
        Some(v) => Ok(Some(py::py_int(v).map_err(value_error)?.max(0) as u64)),
        None => Ok(None),
    }
}

fn narrow<T: TryFrom<i64>>(value: i64, what: &str) -> Result<T> {
    T::try_from(value).map_err(|_| ConfigError(format!("{what}: {value} is out of range")))
}

fn secret(table: &Value, key: &str, where_: &str, required: bool) -> Result<Option<String>> {
    let file_key = format!("{key}_file");
    let mut value: Option<String> = match get(table, key) {
        Some(Value::Null) | None => None,
        Some(v) => Some(py::py_str(v)),
    };
    let falsy = get(table, key).map(|v| !py::truthy(v)).unwrap_or(true);
    if falsy && has(table, &file_key) {
        let path = expand(&py::py_str(get(table, &file_key).unwrap_or(&Value::Null)));
        match std::fs::read_to_string(&path) {
            Ok(text) => value = Some(text.trim().to_string()),
            Err(exc) => {
                return Err(ConfigError(format!("{where_}.{file_key}: cannot read {path}: {}", os_error(&exc))));
            }
        }
    }
    let value = value.map(|v| v.trim().to_string());
    let empty = value.as_deref().map(str::is_empty).unwrap_or(true);
    if required && empty {
        return Err(ConfigError(format!("{where_}.{key} (or {file_key}) is required")));
    }
    if let Some(v) = value.as_deref() {
        let upper = v.to_uppercase();
        if !v.is_empty() && PLACEHOLDER_PREFIXES.iter().any(|p| upper.starts_with(p)) {
            return Err(ConfigError(format!("{where_}.{key} still holds a placeholder value")));
        }
    }
    Ok(value.filter(|v| !v.is_empty()))
}

/// An OSError as Python prints it: `[Errno 2] No such file or directory: '/path'` (the path is added by callers
/// that need it).
pub fn os_error(err: &std::io::Error) -> String {
    match err.raw_os_error() {
        Some(code) => {
            let text = err.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text).to_string();
            format!("[Errno {code}] {text}")
        }
        None => err.to_string(),
    }
}

fn octal(value: Option<&Value>, default: u32) -> Result<u32> {
    match value {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => narrow(n.as_i64().unwrap_or(0), "mode"),
        Some(v) => {
            let text = py::py_str(v);
            u32::from_str_radix(text.trim().trim_start_matches("0o"), 8)
                .map_err(|_| ConfigError(format!("invalid literal for int() with base 8: {}", py::py_repr(v))))
        }
    }
}

/// Logs a warning when the config file is readable by group/others.
pub fn check_permissions(path: &str) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    let mode = meta.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        tracing::warn!(
            path = path,
            mode = format!("0o{:o}", mode),
            "config file is readable by group/others; run chmod 600"
        );
    }
}

/// `"podman exec -i x python"` or `["podman", "exec", ...]`.
fn command(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(v) if py::truthy(v) => match v {
            Value::Array(items) => items.iter().map(py::py_str).collect(),
            other => shlex::split(&py::py_str(other)).unwrap_or_default(),
        },
        _ => Vec::new(),
    }
}

fn voice_server(data: &Value) -> Result<Option<VoiceServerConfig>> {
    let mut t = table(data, "voice_server").clone();
    let drama = table(data, "drama");
    if !has(&t, "url") && has(drama, "url") && b(drama, "enabled", true) {
        // Set up by an earlier installer: audio.cpp serving Drama 3 (44.1 kHz)
        t = serde_json::json!({"url": get(drama, "url").cloned().unwrap_or(Value::Null), "model": "drama-3", "stream_rate": 44100});
    }
    if !has(&t, "url") || !b(&t, "enabled", true) {
        return Ok(None);
    }
    let uses = VOICE_USES.iter().map(|u| (u.to_string(), b(&t, u, true))).collect();
    let rate = match get(&t, "stream_rate") {
        Some(v) if py::truthy(v) => {
            Some(narrow::<u32>(py::py_int(v).map_err(value_error)?, "voice_server.stream_rate")?)
        }
        _ => None,
    };
    let voices = match get(&t, "voices") {
        Some(Value::Array(items)) => items.iter().map(py::py_str).collect(),
        _ => Vec::new(),
    };
    Ok(Some(VoiceServerConfig {
        url: py::rstrip_slash(&s(&t, "url", "")),
        model: opt_s(&t, "model").unwrap_or_else(|| "tts-1".into()),
        stream_rate: rate,
        voices,
        uses,
        first_audio_seconds: f(&t, "first_audio_seconds", 5.0)?,
    }))
}

/// The configuration a TOML document (as a JSON value) describes. Raises `ConfigError` for what Python refuses.
pub fn parse_config(data: &Value, source_path: Option<&str>) -> Result<Config> {
    let bridge_key = secret(data, "bridge_key", "bridge", true)?.unwrap_or_default();
    if bridge_key.chars().count() < 32 {
        tracing::warn!("bridge_key is shorter than 32 characters");
    }
    let data_dir = expand(&s(data, "data_dir", DEFAULT_DATA_DIR));

    let kyutai = table(data, "kyutai");
    let pocket = table(data, "pocket");
    let cron = table(data, "cron");
    let ntfy = table(data, "ntfy");
    let push = table(data, "push");
    let apns_t = table(data, "apns");
    let outbox_t = table(data, "outbox");
    let fcm_t = table(data, "fcm");
    // `[kyutai] enabled = false` (or `url = ""`) runs on Pocket alone; the default voice is then a Bip's.
    let kyutai_url = if b(kyutai, "enabled", true) {
        py::rstrip_slash(&s(kyutai, "url", "http://127.0.0.1:8097"))
    } else {
        String::new()
    };
    let pocket_url = if has(pocket, "url") { Some(py::rstrip_slash(&s(pocket, "url", ""))) } else { None };
    let voice_server = voice_server(data)?;
    if kyutai_url.is_empty() && pocket_url.is_none() {
        return Err(ConfigError::new("no TTS engine: set [pocket] url, or [kyutai] url"));
    }
    let default_voice = if !kyutai_url.is_empty() {
        s(kyutai, "default_voice", "5476")
    } else {
        s(pocket, "default_voice", "pocket:colibri")
    };
    let limits_t = table(data, "limits");

    let apns = ApnsConfig {
        team_id: opt_s(apns_t, "team_id").unwrap_or_default(),
        key_id: opt_s(apns_t, "key_id").unwrap_or_default(),
        p8_path: if has(apns_t, "p8_path") { expand(&s(apns_t, "p8_path", "")) } else { String::new() },
        bundle_id: s(apns_t, "bundle_id", "com.kerfystudio.bipagents"),
        environment: s(apns_t, "environment", "production"),
        base_url: opt_s(apns_t, "base_url").map(|u| py::rstrip_slash(&u)).unwrap_or_default(),
        sandbox_url: opt_s(apns_t, "sandbox_url").map(|u| py::rstrip_slash(&u)).unwrap_or_default(),
    };
    if apns.environment != "sandbox" && apns.environment != "production" {
        return Err(ConfigError::new("apns.environment must be 'sandbox' or 'production'"));
    }

    let outbox = OutboxConfig {
        db_path: expand(&s(outbox_t, "db_path", &py::join(&data_dir, "bridge.db"))),
        audio_dir: expand(&s(outbox_t, "audio_dir", &py::join(&data_dir, "audio"))),
        retention_days: i(outbox_t, "retention_days", 90)?,
        audio_max_chars: narrow(i(outbox_t, "audio_max_chars", 2000)?, "outbox.audio_max_chars")?,
        audio_wait_seconds: f(outbox_t, "audio_wait_seconds", 8.0)?,
    };

    let limits = Limits {
        upload_max_bytes: narrow(
            (f(limits_t, "upload_max_mb", 50.0)? * 1024.0 * 1024.0) as i64,
            "limits.upload_max_mb",
        )?,
        upload_retention_days: i(limits_t, "upload_retention_days", 30)?,
        upload_file_mode: octal(get(limits_t, "upload_file_mode"), 0o640)?,
        upload_dir_mode: octal(get(limits_t, "upload_dir_mode"), 0o750)?,
        sentence_max_chars: narrow(i(limits_t, "sentence_max_chars", 180)?, "limits.sentence_max_chars")?,
        tts_max_chars: narrow(i(limits_t, "tts_max_chars", 1000)?, "limits.tts_max_chars")?,
        tts_cache_entries: narrow(i(limits_t, "tts_cache_entries", 256)?, "limits.tts_cache_entries")?,
        watch_max_seconds: narrow(
            (f(limits_t, "watch_max_minutes", 120.0)? * 60.0) as i64,
            "limits.watch_max_minutes",
        )?,
        purge_interval_seconds: narrow(
            (f(limits_t, "purge_interval_hours", 6.0)? * 3600.0) as i64,
            "limits.purge_interval_hours",
        )?,
    };

    let mut agents: IndexMap<String, AgentConfig> = IndexMap::new();
    if let Some(Value::Object(raw_agents)) = Some(table(data, "agents")) {
        for (raw_name, t) in raw_agents {
            let name = raw_name.trim().to_lowercase();
            let where_ = format!("agents.{name}");
            if !t.is_object() {
                return Err(ConfigError(format!("{where_} must be a table")));
            }
            let hermes_url = py::rstrip_slash(&s(t, "hermes_url", ""));
            if hermes_url.is_empty() {
                return Err(ConfigError(format!("{where_}.hermes_url is required")));
            }
            let up_host = has(t, "upload_dir_host");
            let up_cont = has(t, "upload_dir_container");
            if up_host != up_cont {
                return Err(ConfigError(format!("{where_}: upload_dir_host and upload_dir_container go together")));
            }
            let display_name = opt_s(t, "display_name").unwrap_or_else(|| capitalize(&name));
            let agent = AgentConfig {
                name: name.clone(),
                display_name,
                hermes_url,
                hermes_key: secret(t, "hermes_key", &where_, true)?.unwrap_or_default(),
                ntfy_topic: opt_s(t, "ntfy_topic"),
                ntfy_token: secret(t, "ntfy_token", &where_, false)?,
                upload_dir_host: opt_s(t, "upload_dir_host").map(|p| expand(&p)),
                upload_dir_container: opt_s(t, "upload_dir_container").map(|p| py::rstrip_slash(&p)),
                voice: opt_s(t, "voice"),
                server_voice: opt_s(t, "server_voice"),
                category: opt_s(t, "category"),
                color: opt_s(t, "color"),
                outfit: opt_s(t, "outfit"),
                costumes: get(t, "costumes").map(py::truthy),
                public_url: opt_s(t, "public_url").map(|p| py::rstrip_slash(&p)),
                isolated: b(t, "isolated", false),
                python: command(get(t, "python")),
                hermes_home: opt_s(t, "hermes_home").map(|p| py::rstrip_slash(&p)),
                hermes_root: opt_s(t, "hermes_root").map(|p| py::rstrip_slash(&p)),
                space: opt_s(t, "space").map(|p| expand(&p)),
                sandboxed: b(t, "sandboxed", false),
                desktop_url: opt_s(t, "desktop_url"),
                desktop_vnc: opt_s(t, "desktop_vnc"),
                desktop_vnc_password_file: opt_s(t, "desktop_vnc_password_file").map(|p| expand(&p)),
                shares: s(t, "shares", ""),
                shares_apply: opt_s(t, "shares_apply"),
                host_home: opt_s(t, "host_home").map(|p| expand(&p)),
            };
            agents.insert(name, agent);
        }
    }

    let relay_t = table(data, "relay");
    let mut clients: IndexMap<String, String> = IndexMap::new();
    if let Value::Object(listed) = table(relay_t, "clients") {
        // [relay.clients] chi = "key"
        for (name, key) in listed {
            let key = py::py_str(key).trim().to_string();
            if !key.is_empty() {
                clients.insert(key, name.clone());
            }
        }
    }
    if has(relay_t, "clients_file") {
        // or one « name key » per line
        let path = expand(&s(relay_t, "clients_file", ""));
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                for line in py_lines(&text) {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() == 2 && !line.trim_start().starts_with('#') {
                        clients.insert(parts[1].to_string(), parts[0].to_string());
                    }
                }
            }
            Err(exc) => return Err(ConfigError(format!("relay.clients_file: cannot read {path}: {}", os_error(&exc)))),
        }
    }
    if clients.keys().any(|k| k.chars().count() < 32) {
        return Err(ConfigError::new("relay: client keys must be at least 32 characters (bipbridge genkey)"));
    }
    let relay = RelayConfig { clients, per_hour: i(relay_t, "per_hour", 600)? };
    let push_relay_url = opt_s(push, "relay_url").map(|u| py::rstrip_slash(&u));
    let push_relay_key = secret(push, "relay_key", "push", push_relay_url.is_some())?;
    let push_service_url = opt_s(push, "service_url").map(|u| py::rstrip_slash(&u));
    let model_t = table(data, "model");
    let hermes_t = table(data, "hermes");
    let mut hermes = HermesInstall {
        bin: opt_s(hermes_t, "bin").map(|p| expand(&p)).unwrap_or_default(),
        home: opt_s(hermes_t, "home").map(|p| expand(&p)).unwrap_or_default(),
        hub_url: py::rstrip_slash(&s(hermes_t, "hub_url", "")),
        public_hub_url: py::rstrip_slash(&s(hermes_t, "public_hub_url", "")),
        container_home: py::rstrip_slash(&s(hermes_t, "container_home", "")),
        python: command(get(hermes_t, "python")),
        root: opt_s(hermes_t, "root").map(|p| expand(&p)).unwrap_or_default(),
        isolated: b(hermes_t, "isolated", false),
    };
    if !hermes.home.is_empty() && hermes.python.is_empty() {
        // Hermes' official install: its own Python under <home>/tools, its code in <home>/hermes-agent.
        let pattern = py::join(&py::join(&hermes.home, "tools"), "python-*/bin/python3");
        let mut found: Vec<String> = glob::glob(&pattern)
            .map(|paths| paths.filter_map(|p| p.ok()).map(|p| p.to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        found.sort();
        let code = py::join(&hermes.home, "hermes-agent");
        if let Some(last) = found.last() {
            if std::path::Path::new(&code).is_dir() {
                hermes.python = vec![last.clone(), "-I".into()];
                if hermes.root.is_empty() {
                    hermes.root = code;
                }
            }
        }
    }
    let spaces_t = table(data, "spaces");
    let spaces = SpacesConfig {
        root: expand(&s(spaces_t, "root", &py::join(&data_dir, "spaces"))),
        first_port: narrow(i(spaces_t, "first_port", 8660)?, "spaces.first_port")?,
        sandbox: s(spaces_t, "sandbox", "auto"),
        start: opt_s(spaces_t, "start"),
        stop: opt_s(spaces_t, "stop"),
        container_home: py::rstrip_slash(&s(spaces_t, "container_home", "")),
        tidy_after: f(spaces_t, "tidy_after", 300.0)?.max(0.0),
    };
    if spaces.sandbox != "auto" && spaces.sandbox != "none" {
        return Err(ConfigError::new("spaces.sandbox must be 'auto' or 'none'"));
    }
    let browser_t = table(data, "browser");
    let browser = BrowserConfig {
        enabled: b(browser_t, "enabled", false),
        runtime: s(browser_t, "runtime", ""),
        image: s(browser_t, "image", "bipagents-browser"),
        first_port: narrow(i(browser_t, "first_port", 9600)?, "browser.first_port")?,
        ports: narrow(i(browser_t, "ports", 400)?.max(2), "browser.ports")?,
        root: expand(&s(browser_t, "root", &py::join(&data_dir, "browsers"))),
        gpu: s(browser_t, "gpu", "auto"),
        language: s(browser_t, "language", ""),
        timezone: s(browser_t, "timezone", ""),
        idle_minutes: narrow(i(browser_t, "idle_minutes", 10)?.max(0), "browser.idle_minutes")?,
    };
    if !["", "podman", "docker"].contains(&browser.runtime.as_str())
        || !["auto", "none"].contains(&browser.gpu.as_str())
    {
        return Err(ConfigError::new("browser.runtime must be 'podman' or 'docker', browser.gpu 'auto' or 'none'"));
    }

    let sandbox_t = table(data, "sandbox");
    let defaults = SandboxConfig::default();
    let sandbox = SandboxConfig {
        isolate: s(sandbox_t, "isolate", &defaults.isolate),
        subnet: s(sandbox_t, "subnet", &defaults.subnet),
        registries: match get(sandbox_t, "registries") {
            Some(Value::Array(items)) => items.iter().map(|h| py::py_str(h).trim().to_lowercase()).collect(),
            _ => defaults.registries,
        },
        image: s(sandbox_t, "image", ""),
        send_site_kb_hour: opt_u(sandbox_t, "send_site_kb_hour")?,
        send_day_kb: opt_u(sandbox_t, "send_day_kb")?,
        download_day_mb: opt_u(sandbox_t, "download_day_mb")?,
        connections_per_minute: opt_u(sandbox_t, "connections_per_minute")?,
        cpus: match get(sandbox_t, "cpus") {
            Some(v) => Some(py::py_float(v).map_err(value_error)?.max(0.0)),
            None => None,
        },
        blocklist: get(sandbox_t, "blocklist").map(py::truthy),
        blocklist_urls: match get(sandbox_t, "blocklist_urls") {
            Some(Value::Array(items)) => items.iter().map(|u| py::py_str(u).trim().to_string()).collect(),
            _ => defaults.blocklist_urls,
        },
    };
    if !["auto", "on", "off"].contains(&sandbox.isolate.as_str()) {
        return Err(ConfigError::new("sandbox.isolate must be 'auto', 'on' or 'off'"));
    }
    if crate::sandbox::proxy_ip(&sandbox.subnet).is_none() {
        return Err(ConfigError::new("sandbox.subnet must be an IPv4 /24, e.g. 10.233.80.0/24"));
    }

    let lan_t = table(data, "lan");
    let setup_t = table(data, "setup");
    let lan = LanConfig {
        enabled: b(lan_t, "enabled", false),
        host: s(lan_t, "host", "0.0.0.0"),
        port: narrow(i(lan_t, "port", 8650)?, "lan.port")?,
        address: opt_s(lan_t, "address").map(|a| a.trim().to_string()),
        cert_dir: expand(&s(lan_t, "cert_dir", &py::join(&data_dir, "lan"))),
    };

    let weather_t = table(data, "weather");
    let weather = WeatherConfig {
        met_url: py::rstrip_slash(&s(weather_t, "met_url", MET_URL)),
        geocoder_url: py::rstrip_slash(&s(weather_t, "geocoder_url", GEOCODER_URL)),
    };
    let maps_t = table(data, "maps");
    let maps = MapsConfig {
        google_key: secret(maps_t, "google_key", "maps", false)?,
        routes_url: py::rstrip_slash(&s(maps_t, "routes_url", ROUTES_URL)),
        tiles: s(maps_t, "tiles", "osm").trim().to_lowercase(),
        tile_url: s(maps_t, "tile_url", OSM_TILE_URL),
        vector: s(maps_t, "vector", "openfreemap").trim().to_lowercase(),
        vector_url: py::rstrip_slash(&s(maps_t, "vector_url", OPENFREEMAP_URL)),
        vector_style: {
            let style = s(maps_t, "vector_style", "positron").trim().to_lowercase();
            if !style.is_empty() && style.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                style
            } else {
                "positron".into()
            }
        },
    };

    let notebook_t = table(data, "notebook");
    let nd = NotebookConfig::default();
    let notebook = NotebookConfig {
        model: s(notebook_t, "model", &nd.model),
        base_url: py::rstrip_slash(&s(notebook_t, "base_url", "")),
        key: secret(notebook_t, "key", "notebook", false)?,
        price_in: f(notebook_t, "price_in", nd.price_in)?,
        price_out: f(notebook_t, "price_out", nd.price_out)?,
        monthly_cap: f(notebook_t, "monthly_cap", nd.monthly_cap)?,
        hour: i(notebook_t, "hour", nd.hour as i64)?.clamp(0, 23) as u32,
        max_input_chars: i(notebook_t, "max_input_chars", nd.max_input_chars as i64)?.clamp(1000, 200_000) as usize,
        warranty_min_eur: f(notebook_t, "warranty_min_eur", nd.warranty_min_eur)?.clamp(0.0, 100_000.0),
    };

    let jobs_t = table(data, "jobs");
    let jd = JobsConfig::default();
    let jobs = JobsConfig {
        direct_budget_eur: f(jobs_t, "direct_budget_eur", jd.direct_budget_eur)?.clamp(0.0, 50.0),
        handoff: b(jobs_t, "handoff", jd.handoff),
        handoff_tool_calls: i(jobs_t, "handoff_tool_calls", jd.handoff_tool_calls as i64)?.clamp(1, 10_000) as u32,
        handoff_seconds: f(jobs_t, "handoff_seconds", jd.handoff_seconds)?.clamp(5.0, 86_400.0),
        handoff_browser_tool_calls: i(jobs_t, "handoff_browser_tool_calls", jd.handoff_browser_tool_calls as i64)?
            .clamp(0, 10_000) as u32,
        handoff_browser_seconds: f(jobs_t, "handoff_browser_seconds", jd.handoff_browser_seconds)?.clamp(0.0, 86_400.0),
        handoff_browser_window: i(jobs_t, "handoff_browser_window", jd.handoff_browser_window as i64)?.clamp(1, 100)
            as u32,
        handoff_rescue: b(jobs_t, "handoff_rescue", jd.handoff_rescue),
    };

    let rooms_t = table(data, "rooms");
    let rmd = RoomsConfig::default();
    let rooms = RoomsConfig {
        router_model: b(rooms_t, "router_model", rmd.router_model),
        router_daily_max: i(rooms_t, "router_daily_max", rmd.router_daily_max as i64)?.clamp(0, 100_000) as u32,
        max_hops: i(rooms_t, "max_hops", rmd.max_hops as i64)?.clamp(0, 2) as u32,
        turn_seconds: f(rooms_t, "turn_seconds", rmd.turn_seconds)?.clamp(10.0, 3600.0),
    };

    let guide_t = table(data, "guide");
    let gd = GuideConfig::default();
    let guide = GuideConfig {
        team_max: i(guide_t, "team_max", gd.team_max as i64)?.clamp(1, 6) as usize,
        agents_max: i(guide_t, "agents_max", gd.agents_max as i64)?.clamp(0, 1000) as usize,
        first_turns: b(guide_t, "first_turns", gd.first_turns),
    };

    let records_t = table(data, "archive_records");
    let rd = ArchiveRecordsConfig::default();
    let mut extract_models = Vec::new();
    if let Some(Value::Array(items)) = records_t.get("models") {
        for item in items.iter().filter(|v| v.is_object()) {
            let id = s(item, "id", "").trim().to_string();
            if id.is_empty() {
                return Err(ConfigError::new("archive_records.models: each model needs an id"));
            }
            let label = s(item, "label", &id);
            extract_models.push(ExtractModel {
                id,
                label,
                price_in: f(item, "price_in", rd.unknown_price_in)?.max(0.0),
                price_out: f(item, "price_out", rd.unknown_price_out)?.max(0.0),
            });
        }
    }
    let archive_records = ArchiveRecordsConfig {
        models: extract_models,
        unknown_price_in: f(records_t, "unknown_price_in", rd.unknown_price_in)?.max(0.0),
        unknown_price_out: f(records_t, "unknown_price_out", rd.unknown_price_out)?.max(0.0),
        sample_cap_eur: f(records_t, "sample_cap_eur", rd.sample_cap_eur)?.clamp(0.0, 20.0),
        nightly_cap_eur: f(records_t, "nightly_cap_eur", rd.nightly_cap_eur)?.clamp(0.0, 5.0),
    };

    let cards = s(table(data, "vault"), "cards", "auto").to_lowercase();
    if !["auto", "on", "off"].contains(&cards.as_str()) {
        return Err(ConfigError::new("vault.cards must be 'auto', 'on' or 'off'"));
    }
    let payments_t = table(data, "payments");
    let payments = PaymentsConfig {
        offer_cards: b(payments_t, "offer_cards", false),
        offer_cap_max_eur: i(payments_t, "offer_cap_max_eur", PaymentsConfig::default().offer_cap_max_eur)?
            .clamp(1, 1000),
    };

    let mut pronunciations = IndexMap::new();
    if let Value::Object(langs) = table(data, "pronunciations") {
        for (lang, t) in langs {
            if let Value::Object(words) = t {
                pronunciations.insert(lang.clone(), words.iter().map(|(k, v)| (k.clone(), py::py_str(v))).collect());
            }
        }
    }
    let mut media_roots = IndexMap::new();
    if let Value::Object(roots) = table(table(data, "media"), "roots") {
        for (k, v) in roots {
            media_roots.insert(k.clone(), expand(&py::py_str(v)));
        }
    }

    Ok(Config {
        bridge_key,
        host: s(data, "host", "127.0.0.1"),
        port: narrow(i(data, "port", 8643)?, "port")?,
        log_level: s(data, "log_level", "info").to_lowercase(),
        kyutai_url: if kyutai_url.is_empty() { None } else { Some(kyutai_url) },
        kyutai_timeout: f(kyutai, "timeout_seconds", 120.0)?,
        default_voice,
        pocket_url,
        pocket_engine: s(pocket, "engine", "3.3.0"),
        voice_server,
        pronunciations,
        cron_watch: b(cron, "watch", true),
        cron_interval_seconds: f(cron, "interval_seconds", 30.0)?,
        cron_window_hours: f(cron, "window_hours", 12.0)?,
        cron_subtask_handback: b(cron, "subtask_handback", true),
        media_roots,
        ntfy_url: py::rstrip_slash(&s(ntfy, "url", "http://127.0.0.1:8645")),
        push_previews: b(push, "previews", false),
        watch_followed_runs: b(push, "watch_followed_runs", true),
        apns,
        fcm_service_account: opt_s(fcm_t, "service_account").map(|p| expand(&p)),
        fcm_base_url: py::rstrip_slash(&opt_s(fcm_t, "base_url").unwrap_or_else(|| FCM_BASE_URL.into())),
        fcm_token_url: opt_s(fcm_t, "token_url"),
        outbox,
        limits,
        agents,
        lan,
        relay,
        hermes,
        spaces,
        browser,
        sandbox,
        weather,
        maps,
        notebook,
        jobs,
        brake: crate::loop_brake::Settings::from_table(table(data, "brake")),
        rooms,
        guide,
        connections: crate::connections::Settings::from_table(table(data, "connections")),
        watchers: crate::watchers::Settings::from_table(table(data, "watchers")),
        agent_restart: crate::agent_restart::Settings::from_table(table(data, "agent_restart")),
        link: crate::link_wallet::Settings::from_table(table(data, "link")),
        archive_records,
        cards,
        payments,
        archives: match data.get("archives") {
            Some(Value::Array(items)) => items.iter().filter(|v| v.is_object()).cloned().collect(),
            _ => Vec::new(),
        },
        push_relay_url,
        push_relay_key,
        push_service_url,
        model_health_url: opt_s(model_t, "health_url"),
        model_health_key: secret(model_t, "health_key", "model", false)?,
        model_provider_order: match get(model_t, "provider_order") {
            Some(Value::Array(items)) => {
                items.iter().map(|p| py::py_str(p).trim().to_lowercase()).filter(|p| !p.is_empty()).collect()
            }
            _ => crate::model_route::DEFAULT_ORDER.iter().map(|s| s.to_string()).collect(),
        },
        public_url: opt_s(data, "public_url").map(|u| py::rstrip_slash(&u)),
        peer_url: opt_s(data, "peer_url").map(|u| py::rstrip_slash(&u)),
        cloud: b(data, "cloud", false),
        setup_file: opt_s(setup_t, "file").map(|p| expand(&p)),
        setup_progress: opt_s(setup_t, "progress").map(|p| expand(&p)),
        source_path: source_path.map(str::to_string),
    })
}

/// Python's `str.capitalize()`: the first character upper-cased, the rest lower-cased.
pub fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Lines as Python's file iteration gives them, without their ends (`\n`; a `\r\n` keeps no `\r` in `split()`).
fn py_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split_inclusive('\n').map(|l| l.trim_end_matches('\n'))
}

/// `BRIDGE_CONFIG`, else the default path, expanded.
pub fn config_path() -> String {
    match std::env::var("BRIDGE_CONFIG") {
        Ok(p) if !p.is_empty() => expand(&p),
        _ => expand(DEFAULT_CONFIG_PATH),
    }
}

/// A TOML document as the JSON value Python's `tomllib` would give (datetimes as their TOML text).
pub fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect()),
    }
}

/// Reads and parses the config file (`path`, else [`config_path`]).
pub fn load_config(path: Option<&str>) -> Result<Config> {
    let path = match path {
        Some(p) => expand(p),
        None => config_path(),
    };
    read_config(&path).map(|(_, config)| config)
}

/// The config file's text and what it gives.
fn read_config(path: &str) -> Result<(String, Config)> {
    let path = path.to_string();
    let text = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ConfigError(format!("config file not found: {path}")));
        }
        Err(e) => return Err(ConfigError(format!("cannot read {path}: {}", os_error(&e)))),
    };
    let text = String::from_utf8(text).map_err(|e| ConfigError(format!("invalid TOML in {path}: {e}")))?;
    let doc: toml::Table =
        text.parse().map_err(|e: toml::de::Error| ConfigError(format!("invalid TOML in {path}: {}", e.message())))?;
    check_permissions(&path);
    let config = parse_config(&toml_to_json(toml::Value::Table(doc)), Some(&path))?;
    Ok((text, config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({"bridge_key": "k".repeat(40)})
    }

    #[test]
    fn defaults() {
        let c = parse_config(&base(), None).unwrap();
        assert_eq!(c.port, 8643);
        assert_eq!(c.kyutai_url.as_deref(), Some("http://127.0.0.1:8097"));
        assert_eq!(c.default_voice, "5476");
        assert_eq!(c.limits.upload_max_bytes, 50 * 1024 * 1024);
        assert!(c.outbox.db_path.ends_with("/.local/share/hermes-bridge/bridge.db"));
        assert_eq!(c.apns.host("sandbox"), "https://api.sandbox.push.apple.com");
        assert_eq!(c.apns.host("production"), "https://api.push.apple.com");
        assert_eq!(c.fcm_base_url, FCM_BASE_URL);
    }

    #[test]
    fn shared_config_updates_make_new_snapshots() {
        let shared = SharedConfig::new(parse_config(&base(), None).unwrap());
        let before = shared.get();
        shared.put_agent(AgentConfig { name: "lea".into(), ..Default::default() });
        assert!(before.agents.is_empty());
        assert!(shared.get().agent(Some(" LEA ")).is_some());
        assert!(shared.remove_agent("lea").is_some());
        assert!(shared.get().agents.is_empty());
    }

    #[test]
    fn capitalize_like_python() {
        assert_eq!(capitalize("vie"), "Vie");
        assert_eq!(capitalize("mON-coach"), "Mon-coach");
        assert_eq!(capitalize(""), "");
    }
}
