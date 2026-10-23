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
            bundle_id: "io.github.bipagents".into(),
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
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            isolate: "auto".into(),
            subnet: "10.233.80.0/24".into(),
            registries: crate::sandbox::REGISTRIES.iter().map(|s| s.to_string()).collect(),
            image: String::new(),
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
/// that goes past `handoff_tool_calls` tool calls or `handoff_seconds`, asking the agent to hand the rest over.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JobsConfig {
    /// Euros of tokens a task the person asked for may take without their go (≈ 0.20 €).
    pub direct_budget_eur: f64,
    /// The safety net on or off.
    pub handoff: bool,
    pub handoff_tool_calls: u32,
    pub handoff_seconds: f64,
}

impl Default for JobsConfig {
    fn default() -> Self {
        JobsConfig { direct_budget_eur: 0.20, handoff: true, handoff_tool_calls: 15, handoff_seconds: 180.0 }
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
    /// The person's own voice model (`[voice_server]`; an older `[drama] url` is one). None = the Bips only.
    pub voice_server: Option<VoiceServerConfig>,
    /// Words the voices say wrong, respelled by language (`[pronunciations.fr] Garmin = "Garmine"`).
    pub pronunciations: IndexMap<String, IndexMap<String, String>>,
    /// Scheduled-task replies read from Hermes sessions into the outbox (see cronwatch.py).
    pub cron_watch: bool,
    pub cron_interval_seconds: f64,
    pub cron_window_hours: f64,
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
    /// `[archive_records]` (archives/records_fill.rs): not in the serialized form either.
    #[serde(skip)]
    pub archive_records: ArchiveRecordsConfig,
    /// `[vault] cards` (cards.rs): whether the person may turn card payments on in the app. "auto" (or "on"): yes,
    /// except on a place of the all-included offer or a « no computer » server; "off": never. Payments stay off
    /// until the person turns them on. Not in the serialized form either.
    #[serde(skip)]
    pub cards: String,
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
            voice_server: None,
            pronunciations: IndexMap::new(),
            cron_watch: true,
            cron_interval_seconds: 30.0,
            cron_window_hours: 12.0,
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
            archive_records: ArchiveRecordsConfig::default(),
            cards: "auto".into(),
            archives: Vec::new(),
            push_relay_url: None,
            push_relay_key: None,
            push_service_url: None,
            model_health_url: None,
            model_health_key: None,
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
        bundle_id: s(apns_t, "bundle_id", "io.github.bipagents"),
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
        voice_server,
        pronunciations,
        cron_watch: b(cron, "watch", true),
        cron_interval_seconds: f(cron, "interval_seconds", 30.0)?,
        cron_window_hours: f(cron, "window_hours", 12.0)?,
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
        archive_records,
        cards,
        archives: match data.get("archives") {
            Some(Value::Array(items)) => items.iter().filter(|v| v.is_object()).cloned().collect(),
            _ => Vec::new(),
        },
        push_relay_url,
        push_relay_key,
        push_service_url,
        model_health_url: opt_s(model_t, "health_url"),
        model_health_key: secret(model_t, "health_key", "model", false)?,
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
    parse_config(&toml_to_json(toml::Value::Table(doc)), Some(&path))
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
