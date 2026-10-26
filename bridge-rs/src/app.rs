//! FastAPI application factory: routes, services and background tasks.
//!
//! [`Services`] is Python's `Services` (everything the routes share), reached through [`AppState`] (a cheap
//! handle every handler gets with `State(state): State<AppState>`). [`create_app`] builds the services (Python
//! does it in the lifespan), heals what earlier bridges left, starts the background tasks and assembles the
//! router exactly as Python's `create_app` mounts each module: the same prefixes, the same routers behind the
//! bridge key.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use futures::StreamExt;
use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::LazyLock;
use tokio::task::JoinHandle;

use crate::agent_tools::AudioMaker;
use crate::apns::ApnsClient;
use crate::auth::{require_bridge_key, Caller};
use crate::config::{AgentConfig, Config, SharedConfig};
use crate::console::ConsoleStore;
use crate::cronwatch::{CronWatcher, Presence};
use crate::desktop::Tickets;
use crate::fcm::FcmClient;
use crate::guard::Guard;
use crate::hermes::HermesClient;
use crate::households::Households;
use crate::jobs::{jobs_of, Jobs};
use crate::lan::lan_info;
use crate::live_brief::Briefs;
use crate::logs::{redact6, J};
use crate::mail_scan::Scans;
use crate::ntfy::{ntfy_reachable, NtfySubscriber, OutboxService};
use crate::peers::Links;
use crate::push::{PushService, Sender};
use crate::relay::RelayClient;
use crate::runs::RunHub;
use crate::sleep::Sleep;
use crate::store::{iso, parse_since, utcnow, Row, Store};
use crate::tts::{SynthOptions, TtsError, TtsOptions, TtsService, INTERACTIVE};
use crate::vault::Vault;
use crate::voices::{relay_for, voice_for};
use crate::web::{self, check_max_len, ApiResult, FieldError, HttpError, Json, QueryMap, Validate};
use crate::{agents_admin, py, VERSION};

pub static RUN_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:\-]{1,256}$").expect("valid regex"));
pub static DEVICE_TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]{32,512}$").expect("valid regex"));
pub static SESSION_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_.:-]{1,200}$").expect("valid regex"));
pub const SSE_KEEPALIVE_SECONDS: f64 = 10.0;
pub const MESSAGE_MAX_CHARS: usize = 8000;

/// Everything that needs a running event loop (created in the lifespan).
pub struct Services {
    pub config: SharedConfig,
    /// The shared HTTP client (Hermes, TTS engines, ntfy, relays): no total timeout, 30 s to connect; set
    /// `.timeout(…)` on each request as Python's per-request timeouts (its default is 30 s per phase).
    pub http: reqwest::Client,
    /// APNs' client (HTTP/2), 15 s.
    pub apns_http: reqwest::Client,
    pub store: Store,
    pub tts: Arc<TtsService>,
    pub hermes: HermesClient,
    pub briefs: Arc<Briefs>,
    pub sleep: Arc<Sleep>,
    pub audio_maker: Arc<AudioMaker>,
    pub apns: Option<Arc<ApnsClient>>,
    /// "apns", "relay" or None.
    pub push_mode: Option<&'static str>,
    pub fcm: Option<Arc<FcmClient>>,
    /// The publisher's relay, for the apps that registered there (any install without an Apple key of its own).
    pub push_service: Option<Arc<RelayClient>>,
    pub push: Arc<PushService>,
    pub hub: Arc<RunHub>,
    pub outbox: Arc<OutboxService>,
    pub presence: Arc<Presence>,
    pub vault: Arc<Vault>,
    pub links: Arc<Links>,
    pub households: Arc<Households>,
    pub desktop_tickets: Tickets,
    /// Desktops set up by hand: the person took over (input passes).
    pub desktop_control: Mutex<HashMap<String, bool>>,
    /// Mail scans and background jobs, made at first use (mail_scan::scans_of, jobs::jobs_of).
    pub scans: OnceLock<Arc<Scans>>,
    pub jobs: OnceLock<Arc<Jobs>>,
    /// Group discussions between the person's agents (rooms::rooms_of).
    pub rooms: OnceLock<Arc<crate::rooms::Rooms>>,
    /// Wrong keys counted by address (guard.py): Python's `app.state.guard`.
    pub guard: Guard,
    /// The web consoles (console.py): Python's `app.state.console`.
    pub console: ConsoleStore,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

/// The handle every route gets (`State(state): State<AppState>`); derefs to [`Services`].
#[derive(Clone)]
pub struct AppState(pub Arc<Services>);

impl Deref for AppState {
    type Target = Services;
    fn deref(&self) -> &Services {
        &self.0
    }
}

impl Services {
    pub fn new(config: Config) -> anyhow::Result<Services> {
        let data_dir = config.data_dir();
        let shared = SharedConfig::new(config.clone());
        let http =
            reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).pool_max_idle_per_host(20).build()?;
        let apns_http = reqwest::Client::builder().timeout(Duration::from_secs(15)).build()?;
        let store = Store::open(&config.outbox.db_path)?;
        let tts = Arc::new(TtsService::new(
            http.clone(),
            config.kyutai_url.as_deref(),
            &config.default_voice,
            TtsOptions {
                cache_entries: config.limits.tts_cache_entries,
                timeout: config.kyutai_timeout,
                max_chars: config.limits.tts_max_chars,
                sentence_max_chars: config.limits.sentence_max_chars,
                pocket_url: config.pocket_url.clone(),
                pocket_engine: config.pocket_engine.clone(),
                voice_server: config.voice_server.clone(),
                words: config.pronunciations.clone(),
            },
        ));
        let hermes = HermesClient::new(http.clone());
        let briefs = Arc::new(Briefs::new(&py::join(&data_dir, "briefs"), hermes.clone(), shared.clone()));
        let routines_store = store.clone();
        let routines: crate::sleep::Routines = Arc::new(move || {
            let store = routines_store.clone();
            Box::pin(async move { routines(&store).await })
        });
        let (up_config, up_hermes) = (shared.clone(), hermes.clone());
        let hermes_up: crate::sleep::HermesUp = Arc::new(move || {
            let (config, hermes) = (up_config.get(), up_hermes.clone());
            Box::pin(async move { hermes_up(&config, &hermes).await })
        });
        let sleep = Arc::new(Sleep::new(shared.clone(), routines, hermes_up));
        let audio_maker = Arc::new(AudioMaker::new(shared.clone(), tts.clone()));
        let mut apns = None;
        if config.apns.enabled() {
            if std::path::Path::new(&config.apns.p8_path).exists() {
                apns = Some(Arc::new(ApnsClient::new(config.apns.clone(), apns_http.clone())));
            } else {
                tracing::warn!(path = %config.apns.p8_path, "APNs key file missing");
            }
        }
        // Without its own APNs key, an install (another person's bridge) sends through the administrator's relay.
        let relay = || match (&config.push_relay_url, &config.push_relay_key) {
            (Some(url), Some(key)) if !url.is_empty() && !key.is_empty() => {
                Some(Arc::new(RelayClient::new(url, key, http.clone())))
            }
            _ => None,
        };
        let sender = match &apns {
            Some(client) => Some(Sender::Apns(client.clone())),
            None => relay().map(Sender::Relay),
        };
        let push_mode = if apns.is_some() {
            Some("apns")
        } else if sender.is_some() {
            Some("relay")
        } else {
            None
        };
        // Android: FCM with the Firebase project's key, else through the same relay as Apple's pushes.
        let mut fcm = None;
        if let Some(account) = config.fcm_service_account.as_deref() {
            if std::path::Path::new(account).exists() {
                fcm = Some(Arc::new(FcmClient::new(
                    account,
                    http.clone(),
                    &config.fcm_base_url,
                    config.fcm_token_url.as_deref(),
                )?));
            } else {
                tracing::warn!(path = %account, "FCM service account missing");
            }
        }
        let android = match &fcm {
            Some(client) => Some(Sender::Fcm(client.clone())),
            None => relay().map(Sender::Relay),
        };
        // The publisher's relay, for the apps that registered there (any install without an Apple key of its own).
        let push_service = config
            .push_service_url
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(|url| Arc::new(RelayClient::new(url, "", http.clone())));
        let push = Arc::new(PushService::new(shared.clone(), store.clone(), sender, android, push_service.clone()));
        let hub = Arc::new(
            RunHub::new(hermes.clone(), Some(push.clone()), config.limits.watch_max_seconds, Some(store.clone()))
                .with_config(shared.clone()),
        );
        let outbox = Arc::new(OutboxService::new(shared.clone(), store.clone(), tts.clone(), push.clone()));
        Ok(Services {
            config: shared,
            http,
            apns_http,
            store,
            tts,
            hermes,
            briefs,
            sleep,
            audio_maker,
            apns,
            push_mode,
            fcm,
            push_service,
            push,
            hub,
            outbox,
            presence: Arc::new(Presence::new(45.0)),
            vault: Arc::new(Vault::new(&py::join(&data_dir, "vault"))),
            links: Arc::new(Links::new(&py::join(&data_dir, "peers.json"))),
            households: Arc::new(Households::new(&crate::households::path_for(&config))),
            desktop_tickets: Tickets::new(),
            desktop_control: Mutex::new(HashMap::new()),
            scans: OnceLock::new(),
            jobs: OnceLock::new(),
            rooms: OnceLock::new(),
            guard: Guard::default(),
            console: ConsoleStore::new(&py::join(&data_dir, "console.json")),
            tasks: Mutex::new(Vec::new()),
        })
    }

    /// The configuration as it is now (a snapshot).
    pub fn config(&self) -> Arc<Config> {
        self.config.get()
    }

    /// A background task, stopped with the bridge.
    pub fn spawn<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        self.tasks.lock().unwrap_or_else(|e| e.into_inner()).push(handle);
    }

    /// Python's `stop()`: background tasks cancelled, the hub and the outbox closed, the database closed.
    pub async fn stop(&self) {
        if let Some(jobs) = self.jobs.get() {
            jobs.stop_followers();
        }
        if let Some(rooms) = self.rooms.get() {
            rooms.stop_workers();
        }
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        self.hub.close().await;
        self.outbox.close().await;
        self.store.close();
    }
}

/// When each scheduled task last ran (sleep.py: a place with routines stays up).
async fn routines(store: &Store) -> Vec<String> {
    match store.cron_jobs(None).await {
        Ok(rows) => rows.iter().filter(|r| py::truthy(&r["last_seen"])).map(|r| py::py_str(&r["last_seen"])).collect(),
        Err(_) => Vec::new(),
    }
}

async fn hermes_up(config: &Config, hermes: &HermesClient) -> bool {
    match config.agents.values().next() {
        None => true,
        Some(agent) => hermes.capabilities(agent).await.is_ok(),
    }
}

impl AppState {
    /// Python's `start_background()`.
    pub fn start_background(&self) {
        let config = self.config();
        for agent in config.agents.values() {
            if agent.ntfy_topic.as_deref().map(|t| !t.is_empty()).unwrap_or(false) {
                let sub = Arc::new(NtfySubscriber::new(
                    self.http.clone(),
                    &config.ntfy_url,
                    agent.clone(),
                    self.store.clone(),
                    self.outbox.clone(),
                ));
                self.spawn(sub.run());
            }
        }
        if config.cron_watch {
            let watcher = Arc::new(CronWatcher::new(
                self.config.clone(),
                self.hermes.clone(),
                self.outbox.clone(),
                config.cron_interval_seconds,
                config.cron_window_hours,
                Some(self.store.clone()),
                Some(self.hub.clone()),
                Some(self.push.clone()),
                Some(self.presence.clone()),
            ));
            self.spawn(watcher.run());
        }
        self.spawn(purge_loop(self.clone()));
        self.spawn(crate::notebook::tidy::scheduler(self.clone()));
        self.spawn(crate::dates::run_forever(self.clone()));
        self.spawn(crate::connections::run_forever(self.clone())); // renews the services' access tokens
        self.spawn(crate::archives::scheduler(self.config.clone()));
        crate::agent_email::start(self);
        self.spawn(self.briefs.clone().run());
        self.spawn(self.briefs.clone().sweep_forever());
        if self.sleep.enabled() {
            // a server of the offer: the guichet's seat agent reads whether a run goes on (runs.rs BUSY_MARKER)
            self.spawn(self.hub.clone().mark_busy_forever(self.sleep.home.clone(), 60.0));
            self.spawn(self.sleep.clone().run_forever());
        }
        let hub = self.hub.clone();
        let agents = config.agents.clone();
        self.spawn(async move {
            hub.resume(agents).await;
        });
        let rooms = crate::rooms::rooms_of(self);
        self.spawn(async move {
            rooms.resume();
        });
        let jobs = jobs_of(self);
        self.spawn(async move {
            jobs.clone().resume_following().await;
            crate::archives::records_fill::resume_all(jobs.config.get(), jobs.tell.clone());
            crate::notebook::fill::follow(&jobs);
        });
        if config.hermes.enabled() {
            self.spawn(crate::sandbox::isolate_all(self.config.clone(), agents_admin::default_run()));
            self.spawn(crate::sandbox_sites::watch(self.clone()));
            self.spawn(crate::shares::mirror_forever(self.config.clone()));
            self.spawn(crate::agent_tools::register_all(self.config.clone(), agents_admin::default_run()));
            let shared = self.config.clone();
            self.spawn(async move { crate::browsers::vault_only_all(&shared.get()).await });
            if config.cloud {
                // a place of the reserve: its first agent made before its claim; what a claim left, finished
                self.spawn(crate::cloud_setup::prepare_when_installed(self.config.clone(), self.http.clone()));
                if crate::cloud_setup::has_pending(&config) {
                    let (shared, http) = (self.config.clone(), self.http.clone());
                    self.spawn(async move {
                        crate::cloud_setup::finish_pending(&shared, &http, &agents_admin::default_run()).await;
                    });
                }
            }
        }
    }
}

async fn purge_loop(state: AppState) {
    tokio::time::sleep(Duration::from_secs(5)).await;
    loop {
        let config = state.config();
        let result: anyhow::Result<()> = async {
            for agent in config.agents.values() {
                if let Some(root) = agent.upload_dir_host.clone().filter(|r| !r.is_empty()) {
                    let days = crate::files_kept::kept_days(&config);
                    if days <= 0 {
                        continue; // kept always: the person's choice
                    }
                    let removed =
                        tokio::task::spawn_blocking(move || crate::files::purge_uploads(&root, days, None)).await?;
                    if removed > 0 {
                        tracing::info!(agent = %agent.name, removed = removed, "uploads purged");
                        // and from its sandbox's copy (sandbox.rs)
                        let (c, name) = (config.clone(), agent.name.clone());
                        tokio::task::spawn_blocking(move || crate::sandbox::sync_agent(&c, &name)).await??;
                    }
                }
            }
            let days = crate::files_kept::kept_days(&config);
            if days > 0 {
                let c = config.clone();
                let removed = tokio::task::spawn_blocking(move || crate::sent_files::purge(&c, days, None)).await?;
                if removed > 0 {
                    tracing::info!(removed = removed, "files the agents sent purged");
                }
            }
            let (c, days) = (config.clone(), config.outbox.retention_days);
            let removed = tokio::task::spawn_blocking(move || crate::agent_tools::purge_audio(&c, days, None)).await?;
            if removed > 0 {
                tracing::info!(removed = removed, "agent audio purged");
            }
            let removed = state.outbox.purge().await?;
            if removed > 0 {
                tracing::info!(removed = removed, "outbox purged");
            }
            Ok(())
        }
        .await;
        if let Err(exc) = result {
            tracing::error!(exc = %format!("{exc:#}"), "purge failed");
        }
        tokio::time::sleep(Duration::from_secs(config.limits.purge_interval_seconds)).await;
    }
}

// -- request models ----------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct CronJobPrefs {
    #[serde(deserialize_with = "web::lax::bool")]
    pub notify: bool,
}
impl Validate for CronJobPrefs {}

#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub agent: Option<String>,
    /// What it is for (`live`, `replies`, `reading`): with no `voice`, the agent's voice for that use.
    #[serde(default, rename = "use")]
    pub use_: Option<String>,
    /// /v1/tts/prepare: the version of the Pocket package the phone speaks with (« 3.3.0 », « 4.0.1 », from its
    /// manifest). Below 4 the text is respelled for the old voices; missing (an app from before 2026-10-25), the phone
    /// is taken to have the 3.3.0 packages (`PHONE_POCKET_DEFAULT`).
    #[serde(default)]
    pub engine: Option<String>,
}

fn default_format() -> String {
    "pcm16".into()
}

impl Validate for TtsRequest {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "text", Some(&self.text), 20000);
        check_max_len(errors, "voice", self.voice.as_deref(), 300);
        check_max_len(errors, "engine", self.engine.as_deref(), 40);
        check_max_len(errors, "use", self.use_.as_deref(), 20);
    }
}

#[derive(Debug, Deserialize)]
pub struct DeviceRequest {
    pub token: String,
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub agent_ids: Vec<String>,
}
impl Validate for DeviceRequest {}

#[derive(Debug, Deserialize)]
pub struct WatchRequest {
    pub agent: String,
    pub run_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
}
impl Validate for WatchRequest {}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub agent: String,
    pub run_id: String,
    pub choice: String,
    #[serde(default)]
    pub request_id: Option<String>,
}

impl Validate for ApproveRequest {
    fn validate(&self, errors: &mut Vec<FieldError>) {
        check_max_len(errors, "choice", Some(&self.choice), 32);
        check_max_len(errors, "request_id", self.request_id.as_deref(), 256);
    }
}

/// `_agent_or_404`.
pub fn agent_or_404(config: &Config, name: Option<&str>) -> ApiResult<AgentConfig> {
    config.agent(name).cloned().ok_or_else(|| HttpError::new(404, "unknown agent"))
}

fn check_run_id(run_id: &str) -> ApiResult<String> {
    if !RUN_ID.is_match(run_id) {
        return Err(HttpError::new(400, "invalid run_id"));
    }
    Ok(run_id.to_string())
}

/// `body.agent`'s agent (404 when named and unknown).
fn body_agent(config: &Config, name: Option<&str>) -> ApiResult<Option<AgentConfig>> {
    match name.filter(|n| !n.is_empty()) {
        None => Ok(None),
        Some(n) => config.agent(Some(n)).cloned().map(Some).ok_or_else(|| HttpError::new(404, "unknown agent")),
    }
}

fn tts_error(exc: TtsError) -> HttpError {
    HttpError::new(exc.status, exc.message)
}

fn outbox_item(row: &Row, state: &AppState) -> Value {
    let id = py::py_str(&row["id"]);
    let has_audio = std::path::Path::new(&state.outbox.audio_file(&id)).exists();
    let get = |k: &str| row.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "id": get("id"), "agent": get("agent"), "title": get("title"), "text": get("text"),
        "created_at": get("created_at"), "sent_at": get("sent_at"), "session_id": get("session_id"),
        "has_audio": has_audio, "audio_url": format!("/v1/outbox/{id}/audio"),
    })
}

fn with_headers(mut response: Response, headers: &[(&'static str, String)]) -> Response {
    for (name, value) in headers {
        if let Ok(v) = HeaderValue::from_str(value) {
            response.headers_mut().insert(*name, v);
        }
    }
    response
}

fn bytes_response(data: bytes::Bytes, content_type: &str, headers: &[(&'static str, String)]) -> Response {
    let mut response = Response::new(Body::from(data));
    let content_type = if content_type.starts_with("text/") && !content_type.contains("charset") {
        format!("{content_type}; charset=utf-8")
    } else {
        content_type.to_string()
    };
    if let Ok(v) = HeaderValue::from_str(&content_type) {
        response.headers_mut().insert("content-type", v);
    }
    with_headers(response, headers)
}

// -- the core routes (Python's build_router: /v1, bridge key) ----------------------------------------

async fn list_agents(State(state): State<AppState>, caller: Caller) -> ApiResult<Response> {
    let config = state.config();
    let agents: Vec<Value> = config
        .agents
        .values()
        .map(|a| {
            let url = a.public_url.clone().filter(|u| !u.is_empty());
            json!({
                "id": a.name, "name": a.display_name,
                "voice": a.voice.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| config.default_voice.clone()),
                "server_voice": a.server_voice, "category": a.category, "color": a.color,
                "outfit": a.outfit.clone().unwrap_or_default(), "costumes": a.costumes.unwrap_or(true),
                "uploads": a.upload_dir_host.as_deref().map(|u| !u.is_empty()).unwrap_or(false),
                "inbox": a.ntfy_topic.as_deref().map(|t| !t.is_empty()).unwrap_or(false),
                "url": a.public_url,
                // Never to a web console: it reaches Hermes through the bridge, which adds the key.
                "key": if url.is_some() && !caller.console { Some(a.hermes_key.clone()) } else { None },
            })
        })
        .collect();
    let unclaimed = agents_admin::unclaimed_profiles(&config);
    let groups = crate::agent_groups::all(&config);
    let agents: Vec<Value> = agents
        .into_iter()
        .map(|mut a| {
            let group = a["id"].as_str().and_then(|id| groups.get(id)).cloned().unwrap_or_else(|| json!(""));
            a["group"] = group;
            a
        })
        .collect();
    Ok(web::ok(
        json!({"bridge": {"url": config.public_url, "lan": lan_info(&config)}, "agents": agents, "unclaimed": unclaimed}),
    ))
}

async fn tts_sentence(State(state): State<AppState>, Json(body): Json<TtsRequest>) -> ApiResult<Response> {
    let config = state.config();
    let agent = body_agent(&config, body.agent.as_deref())?;
    if !["pcm16", "opus", "wav"].contains(&body.format.as_str()) {
        return Err(HttpError::new(400, "format must be pcm16, opus or wav"));
    }
    let voice = body
        .voice
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| voice_for(&config, agent.as_ref(), body.use_.as_deref()));
    let audio = state
        .tts
        .synthesize(
            &body.text,
            Some(&voice),
            &body.format,
            INTERACTIVE,
            SynthOptions { relay: relay_for(agent.as_ref()), ..Default::default() },
        )
        .await
        .map_err(tts_error)?;
    let mut headers = vec![
        ("x-sample-rate", audio.sample_rate.to_string()),
        ("x-channels", audio.channels.to_string()),
        ("x-cache", if audio.cached { "hit" } else { "miss" }.to_string()),
        ("cache-control", "no-store".to_string()),
    ];
    if audio.format == "pcm16" {
        headers.push(("x-sample-format", "s16le".into()));
    }
    Ok(bytes_response(audio.data, &audio.content_type, &headers))
}

/// The text a Bip says, prepared as this bridge would for Pocket (numbers in words, respellings, pauses), for a
/// phone that speaks with the Bips' voices itself (voices/onnx/). `local: false`: not a Bip's voice, the phone
/// asks for the audio as before.
async fn tts_prepare(State(state): State<AppState>, Json(body): Json<TtsRequest>) -> ApiResult<Response> {
    let config = state.config();
    let agent = body_agent(&config, body.agent.as_deref())?;
    let voice = body
        .voice
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| voice_for(&config, agent.as_ref(), body.use_.as_deref()));
    let prepared = state
        .tts
        .for_phone(&body.text, Some(&voice), Some(MESSAGE_MAX_CHARS), body.engine.as_deref())
        .map_err(tts_error)?;
    Ok(web::ok(match prepared {
        None => json!({"local": false, "voice": voice}),
        Some((text, language, bip)) => {
            json!({"local": true, "voice": voice, "text": text, "language": language, "bip": bip})
        }
    }))
}

/// A whole reply as one audio file (voice-message mode). Kyutai gets the full text at once, so it batches the
/// sentences on the GPU: far faster per second of audio than sentence by sentence.
async fn tts_message(State(state): State<AppState>, Json(body): Json<TtsRequest>) -> ApiResult<Response> {
    let config = state.config();
    let agent = body_agent(&config, body.agent.as_deref())?;
    let fmt = if body.format != "pcm16" { body.format.clone() } else { "mp3".to_string() }; // default to a compressed file here
    if !["mp3", "opus", "wav"].contains(&fmt.as_str()) {
        return Err(HttpError::new(400, "format must be mp3, opus or wav"));
    }
    let voice = body
        .voice
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| voice_for(&config, agent.as_ref(), body.use_.as_deref()));
    let audio = state
        .tts
        .synthesize(
            &body.text,
            Some(&voice),
            &fmt,
            INTERACTIVE,
            SynthOptions { max_chars: Some(MESSAGE_MAX_CHARS), relay: relay_for(agent.as_ref()), ..Default::default() },
        )
        .await
        .map_err(tts_error)?;
    Ok(bytes_response(
        audio.data,
        &audio.content_type,
        &[("x-cache", if audio.cached { "hit" } else { "miss" }.to_string()), ("cache-control", "no-store".into())],
    ))
}

/// Raw PCM16 (s16le, mono, 24 kHz) streamed as Kyutai produces it: first audio in ~0.75 s.
/// Errors before the first chunk are plain HTTP errors; the body is chunked audio only.
async fn tts_stream(State(state): State<AppState>, Json(body): Json<TtsRequest>) -> ApiResult<Response> {
    let config = state.config();
    let agent = body_agent(&config, body.agent.as_deref())?;
    let voice = body
        .voice
        .clone()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| voice_for(&config, agent.as_ref(), body.use_.as_deref()));
    let relay = relay_for(agent.as_ref());
    let mut chunks = state.tts.stream(&body.text, Some(&voice), INTERACTIVE, Some(MESSAGE_MAX_CHARS), relay.as_deref());
    let first = match chunks.next().await {
        None => bytes::Bytes::new(),
        Some(Err(exc)) => return Err(tts_error(exc)),
        Some(Ok(chunk)) => chunk,
    };
    let rest = chunks.map(|c| c.map_err(|e| std::io::Error::other(e.message)));
    let stream = futures::stream::iter((!first.is_empty()).then(|| Ok::<_, std::io::Error>(first))).chain(rest);
    let response = Response::new(Body::from_stream(stream));
    Ok(with_headers(
        response,
        &[
            ("content-type", "application/octet-stream".into()),
            ("x-sample-rate", "24000".into()),
            ("x-channels", "1".into()),
            ("x-sample-format", "s16le".into()),
            ("cache-control", "no-store".into()),
        ],
    ))
}

async fn upload_file(State(state): State<AppState>, request: Request) -> ApiResult<Response> {
    Ok(web::ok(crate::files::handle_upload(state, request).await?))
}

async fn register_device(State(state): State<AppState>, Json(body): Json<DeviceRequest>) -> ApiResult<Response> {
    let config = state.config();
    let mut token = body.token.trim().replace([' ', '<', '>'], "");
    let environment =
        body.environment.clone().filter(|e| !e.is_empty()).unwrap_or_else(|| config.apns.environment.clone());
    if environment == "relay" {
        // a relay token from the publisher's relay: kept as it is (case-sensitive)
        if state.push_service.is_none() {
            return Err(HttpError::new(409, "this bridge has no push relay set up"));
        }
        if !crate::relay::RELAY_TOKEN.is_match(&token) {
            return Err(HttpError::new(400, "invalid relay token"));
        }
    } else if environment == "fcm" {
        // the Android app (Firebase Cloud Messaging); its tokens are case-sensitive
        if !crate::fcm::FCM_TOKEN.is_match(&token) {
            return Err(HttpError::new(400, "invalid FCM token"));
        }
    } else {
        if !DEVICE_TOKEN.is_match(&token) {
            return Err(HttpError::new(400, "invalid device token (hex expected)"));
        }
        if environment != "sandbox" && environment != "production" {
            return Err(HttpError::new(400, "environment must be sandbox, production or fcm"));
        }
        token = token.to_lowercase();
    }
    let agent_ids: Vec<String> =
        body.agent_ids.iter().map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect();
    let unknown: Vec<&String> = agent_ids.iter().filter(|a| !config.agents.contains_key(*a)).collect();
    if !unknown.is_empty() {
        return Err(HttpError::json(400, json!({"error": "unknown agent_ids", "agent_ids": unknown})));
    }
    let device = state.store.upsert_device(&token, &environment, &agent_ids).await?;
    tracing::info!(token = %redact6(&token), env = %environment, agents = ?J(&agent_ids), "device registered");
    Ok(web::ok(Value::Object(device)))
}

async fn delete_device(State(state): State<AppState>, Path(token): Path<String>) -> ApiResult<Response> {
    let token = token.trim().to_string();
    if state.store.delete_device(&token).await? || state.store.delete_device(&token.to_lowercase()).await? {
        tracing::info!(token = %redact6(&token), "device removed");
    }
    Ok(web::no_content())
}

async fn list_outbox(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let since = q.get("since").map(str::to_string);
    let agent = q.get("agent").map(str::to_string);
    let limit = web::query_int(&q, "limit", Some(100), Some(1), Some(500))?.unwrap_or(100);
    let mut since_dt = None;
    if let Some(since) = since.filter(|s| !s.is_empty()) {
        since_dt = Some(parse_since(&since).map_err(|_| HttpError::new(400, "invalid since (ISO 8601 expected)"))?);
    }
    let agent_name = match agent.filter(|a| !a.is_empty()) {
        Some(a) => Some(agent_or_404(&config, Some(&a))?.name),
        None => None,
    };
    let rows = state.store.list_outbox(since_dt, agent_name.as_deref(), limit).await?;
    let items: Vec<Value> = rows.iter().map(|r| outbox_item(r, &state)).collect();
    Ok(web::ok(json!({"items": items, "server_time": iso(utcnow())})))
}

async fn get_outbox(State(state): State<AppState>, Path(item_id): Path<String>) -> ApiResult<Response> {
    let row = state.store.get_outbox(&item_id).await?.ok_or_else(HttpError::not_found)?;
    Ok(web::ok(outbox_item(&row, &state)))
}

async fn get_outbox_audio(
    State(state): State<AppState>,
    Path(item_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let row = state.store.get_outbox(&item_id).await?.ok_or_else(HttpError::not_found)?;
    let path = state.outbox.audio_file(&item_id);
    if !std::path::Path::new(&path).exists() {
        let Some(task) = state.outbox.ensure_audio(&row, None) else {
            return Err(HttpError::new(404, "no audio for this message"));
        };
        if tokio::time::timeout(Duration::from_secs(20), task).await.is_err() {
            let mut response = web::json_response(202, &json!({"status": "pending"}));
            response.headers_mut().insert("retry-after", HeaderValue::from_static("3"));
            return Ok(response);
        }
        if !std::path::Path::new(&path).exists() {
            return Err(HttpError::new(503, "audio synthesis failed"));
        }
    }
    Ok(web::file_response(&path, "audio/mpeg", Some(&format!("{item_id}.mp3")), &[], &headers).await)
}

/// A file an agent pointed to with « MEDIA:<path> »: under `[media] roots` (and the bridge's audio and pages), or in
/// the places of the agent `agent` names (sent_files.rs: its sandbox's /envoi, its workspace…), from the bridge's
/// copy. An app that names no agent (before 2026-10-22) gets such a copy only when a single agent has that path.
async fn get_media(State(state): State<AppState>, q: QueryMap, headers: HeaderMap) -> ApiResult<Response> {
    let path = web::query_str(&q, "path", true, Some(1024))?.unwrap_or_default();
    let agent = web::query_str(&q, "agent", false, Some(64))?.filter(|a| !a.is_empty());
    let config = state.config();
    let host = match crate::agent_tools::served_media(&config, &path) {
        Some(host) => host,
        None => {
            let agent_cfg = match agent.as_deref() {
                Some(name) => match config.agent(Some(name)) {
                    Some(agent) => Some(agent.clone()),
                    None => {
                        tracing::warn!(
                            target: "bipbridge::media",
                            agent = %name,
                            path = %path,
                            why = "unknown agent",
                            "media file asked for and not found"
                        );
                        return Err(HttpError::not_found());
                    }
                },
                None => None,
            };
            let (c, p) = (config.clone(), path.clone());
            let found = tokio::task::spawn_blocking(move || match &agent_cfg {
                Some(agent) => crate::sent_files::capture(&c, agent, &p),
                None => crate::sent_files::find_unique(&c, &p).ok_or_else(|| "no agent named, no single copy".into()),
            })
            .await
            .map_err(anyhow::Error::from)?;
            match found {
                Ok(copy) => copy.to_string_lossy().into_owned(),
                Err(why) => {
                    tracing::warn!(
                        target: "bipbridge::media",
                        agent = agent.as_deref().unwrap_or("-"),
                        path = %path,
                        why = %why,
                        "media file asked for and not found"
                    );
                    return Err(HttpError::not_found());
                }
            }
        }
    };
    let mut extra = vec![("x-content-type-options", "nosniff".to_string())];
    if host.to_lowercase().ends_with(".html") {
        // A page an agent made (or an .html file from its sandbox): never a document of this origin, even opened by
        // hand; kept by the app but asked again each time (its ETag: a new version shows, the same one is a 304).
        extra.push(("content-security-policy", format!("sandbox allow-scripts; {}", crate::pages::POLICY)));
        extra.push(("cache-control", "no-cache".into()));
    } else if crate::media::is_document(&host) {
        extra.push(("content-security-policy", "sandbox; default-src 'none'".into()));
    }
    let media_type = crate::media::content_type(&host);
    Ok(web::file_response(&host, &media_type, Some(&py::basename(&host)), &extra, &headers).await)
}

/// The pages an agent made (each one's `path` is for `/v1/media`), the last changed first.
async fn list_pages(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let config = state.config();
    let agent_cfg = agent_or_404(&config, Some(&agent))?;
    let found = tokio::task::spawn_blocking(move || crate::pages::list_pages(&config, &agent_cfg.name))
        .await
        .map_err(anyhow::Error::from)?;
    let pages: Vec<Value> = found
        .iter()
        .map(|p| {
            let mut out = Map::new();
            for k in ["page", "path", "title", "created", "updated", "version", "bytes"] {
                out.insert(k.into(), p.get(k).cloned().unwrap_or(Value::Null));
            }
            // its place in the Carnet, when it has one
            for k in ["notebook", "private"] {
                if let Some(v) = p.get(k) {
                    out.insert(k.into(), v.clone());
                }
            }
            Value::Object(out)
        })
        .collect();
    Ok(web::ok(json!({"pages": pages})))
}

async fn delete_page(State(state): State<AppState>, Path(page): Path<String>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let config = state.config();
    let agent_cfg = agent_or_404(&config, Some(&agent))?;
    let (c, name, p) = (config.clone(), agent_cfg.name.clone(), page.clone());
    let deleted =
        tokio::task::spawn_blocking(move || crate::pages::delete(&c, &name, &p)).await.map_err(anyhow::Error::from)?;
    if !deleted {
        return Err(HttpError::not_found());
    }
    tracing::info!(agent = %agent_cfg.name, page = %page, "agent page deleted");
    Ok(web::ok(json!({"deleted": page})))
}

/// Polled by an open conversation: its message count (to show turns the agent takes on its own),
/// and « the user is looking at it » (no push for what is on screen).
async fn session_state(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    q: QueryMap,
) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let config = state.config();
    let agent_cfg = agent_or_404(&config, Some(&agent))?;
    if !SESSION_ID.is_match(&session_id) {
        return Err(HttpError::new(400, "invalid session_id"));
    }
    state.presence.touch(&agent_cfg.name, &session_id);
    let session = match state.hermes.session(&agent_cfg, &session_id).await {
        Ok(s) => s,
        Err(exc) => {
            if let Some(status) = exc.status() {
                return Err(HttpError::new(
                    if [401, 403, 404].contains(&status) { status } else { 502 },
                    "Hermes refused",
                ));
            }
            if exc.is_transport() {
                return Err(HttpError::new(502, "Hermes unreachable"));
            }
            return Err(anyhow::anyhow!(exc).into());
        }
    };
    let count =
        if session.contains_key("message_count") { session.get("message_count") } else { session.get("messages") };
    // isinstance(count, int): a bool is one in Python.
    let count = match count {
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => Value::Number(n.clone()),
        Some(Value::Bool(b)) => Value::Bool(*b),
        _ => Value::Null,
    };
    let updated = session
        .get("updated_at")
        .filter(|v| py::truthy(v))
        .or_else(|| session.get("last_active"))
        .cloned()
        .unwrap_or(Value::Null);
    // the agent's Boîte messages (a hand-back, a routine's report): they show in the conversation too (Bot Chat, all
    // of them) without changing its message count; the app reloads when this changes
    let outbox = state.store.outbox_marker(&agent_cfg.name).await.ok();
    Ok(web::ok(json!({"session_id": session_id, "message_count": count, "updated_at": updated, "outbox": outbox})))
}

/// Is each agent's Hermes up, and does the model answer? Lets the app tell « server down » from « model
/// down » (its machine off, an expired API key).
async fn status(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let hermes_ok = |agent: AgentConfig| {
        let state = state.clone();
        async move {
            match tokio::time::timeout(Duration::from_secs(5), state.hermes.capabilities(&agent)).await {
                Ok(Ok(_)) => json!({"ok": true}),
                Ok(Err(exc)) => json!({"ok": false, "error": exc.type_name()}),
                Err(_) => json!({"ok": false, "error": "TimeoutError"}),
            }
        }
    };
    let model_ok = async {
        let url = config.model_health_url.clone().filter(|u| !u.is_empty())?;
        let mut request = state.http.get(&url).timeout(Duration::from_secs(5));
        if let Some(key) = config.model_health_key.as_deref().filter(|k| !k.is_empty()) {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        Some(match request.send().await {
            Ok(resp) => json!({"ok": resp.status().as_u16() < 400, "status": resp.status().as_u16()}),
            Err(exc) => json!({"ok": false, "error": crate::hermes::httpx_error_name(&exc)}),
        })
    };
    let names: Vec<String> = config.agents.keys().cloned().collect();
    let checks = futures::future::join_all(config.agents.values().cloned().map(hermes_ok));
    let (results, model) = tokio::join!(checks, model_ok);
    let agents: Map<String, Value> = names.into_iter().zip(results).collect();
    Ok(web::ok(json!({"agents": agents, "model": model, "protected": config.hermes.isolated})))
}

/// The LAN door as it is now (address, certificate): the app refreshes it while the tailnet works.
async fn pairing(State(state): State<AppState>) -> ApiResult<Response> {
    Ok(web::ok(json!({"lan": lan_info(&state.config())})))
}

/// The text of a reply / question push, for the Notification Service Extension (kept 24 h).
async fn get_reply(State(state): State<AppState>, Path(reply_id): Path<String>) -> ApiResult<Response> {
    let text = state.push.text(&reply_id).ok_or_else(HttpError::not_found)?;
    Ok(web::ok(json!({"id": reply_id, "text": text})))
}

/// Scheduled tasks seen by the cron watcher, with whether their replies are pushed.
async fn cron_jobs(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let name = match q.get("agent").filter(|a| !a.is_empty()) {
        Some(a) => Some(agent_or_404(&config, Some(a))?.name),
        None => None,
    };
    let jobs = state.store.cron_jobs(name.as_deref()).await?;
    Ok(web::ok(json!({"jobs": jobs})))
}

async fn set_cron_job(
    State(state): State<AppState>,
    Path((agent, job)): Path<(String, String)>,
    Json(body): Json<CronJobPrefs>,
) -> ApiResult<Response> {
    let name = agent_or_404(&state.config(), Some(&agent))?.name;
    if !state.store.set_cron_notify(&name, &job, body.notify).await? {
        return Err(HttpError::new(404, "unknown job"));
    }
    Ok(web::ok(json!({"agent": name, "job": job, "notify": body.notify})))
}

async fn watch_run(State(state): State<AppState>, Json(body): Json<WatchRequest>) -> ApiResult<Response> {
    let agent = agent_or_404(&state.config(), Some(&body.agent))?;
    let run_id = check_run_id(&body.run_id)?;
    let session_id = body.session_id.as_deref().filter(|s| !s.is_empty() && SESSION_ID.is_match(s));
    let sub = state.hub.watch(&agent, &run_id, session_id, true);
    Ok(web::ok(
        json!({"watching": true, "agent": agent.name, "run_id": run_id, "followers": sub.followers(), "finished": sub.finished()}),
    ))
}

/// The app follows a run through the bridge (SSE): the bridge holds the single Hermes subscription,
/// replays the run from its start (re-attaching after a disconnect is exact), and knows whether
/// someone is watching, so approvals and « reply ready » are pushed only when nobody is.
async fn run_events(State(state): State<AppState>, Path(run_id): Path<String>, q: QueryMap) -> ApiResult<Response> {
    let agent = web::query_required(&q, "agent")?;
    let session_id = web::query_str(&q, "session_id", false, Some(200))?;
    let agent_cfg = agent_or_404(&state.config(), Some(&agent))?;
    let run_id = check_run_id(&run_id)?;
    if let Some(s) = session_id.as_deref() {
        if !SESSION_ID.is_match(s) {
            return Err(HttpError::new(400, "invalid session_id"));
        }
    }
    let mut listener = state.hub.subscribe(&agent_cfg, &run_id, "app", true);
    if let Some(s) = session_id.filter(|s| !s.is_empty()) {
        listener.sub.set_session_id(Some(s));
    }
    let events = async_stream::stream! {
        loop {
            match tokio::time::timeout(Duration::from_secs_f64(SSE_KEEPALIVE_SECONDS), listener.queue.recv()).await {
                Err(_) => {
                    yield Ok::<_, std::io::Error>(bytes::Bytes::from_static(b": keepalive\n\n"));
                }
                Ok(None) | Ok(Some(None)) => break,
                Ok(Some(Some(event))) => {
                    let data = if event.raw.is_empty() { py::Dumps::new().unicode().dumps(&Value::Object(event.data.clone())) } else { event.raw.clone() };
                    let data = data.replace('\n', "\ndata: ");
                    yield Ok(bytes::Bytes::from(format!("event: {}\ndata: {}\n\n", event.r#type, data)));
                }
            }
        }
        listener.close();
    };
    let response = Response::new(Body::from_stream(events));
    Ok(with_headers(
        response,
        &[
            ("content-type", "text/event-stream; charset=utf-8".into()),
            ("cache-control", "no-store".into()),
            ("x-accel-buffering", "no".into()),
        ],
    ))
}

async fn approve(State(state): State<AppState>, Json(body): Json<ApproveRequest>) -> ApiResult<Response> {
    let config = state.config();
    if config.agent(Some(&body.agent)).is_none() && crate::agent_shares::is_shared_with_me(&config, &body.agent) {
        // an agent a relative shares with this person: her answer goes to their bridge (agent_shares.rs)
        let run_id = check_run_id(&body.run_id)?;
        let choice = body.choice.trim().to_lowercase();
        return crate::agent_shares::approve_mine(&state, &body.agent, &run_id, &choice, body.request_id.as_deref())
            .await;
    }
    let agent = agent_or_404(&config, Some(&body.agent))?;
    let run_id = check_run_id(&body.run_id)?;
    if crate::agent_shares::relative_run(&config, &run_id) {
        return Err(HttpError::new(403, "a relative's turn: they answer its approvals"));
    }
    // a card of the bridge's own (a site for the agent's programs, sandbox_sites.rs), answered here
    if let Some(id) = run_id.strip_prefix(crate::sandbox_sites::RUN_PREFIX) {
        let decision = crate::sandbox_sites::decision_of_choice(&body.choice);
        let choice = match decision {
            crate::sandbox_sites::Decision::AllSites => "all_sites",
            crate::sandbox_sites::Decision::DenyAll => "deny",
            _ => "always",
        };
        return match crate::sandbox_sites::answer(&state.config(), &agent.name, id, decision, "notification")
            .map_err(anyhow::Error::from)?
        {
            Some(_) => Ok(web::ok(json!({"ok": true, "choice": choice}))),
            None => Err(HttpError::new(404, "no such request (answered already?)")),
        };
    }
    // a card of the vault's (an account to create with a vault password, vault_logins.rs), answered here
    if let Some(id) = run_id.strip_prefix(crate::vault_logins::RUN_PREFIX) {
        let allow = body.choice.trim().to_lowercase() != "deny";
        return if crate::vault_logins::answer(&state.config(), &agent.name, id, allow, "notification") {
            Ok(web::ok(json!({"ok": true, "choice": if allow { "once" } else { "deny" }})))
        } else {
            Err(HttpError::new(404, "no such request (answered already?)"))
        };
    }
    let request_id = body.request_id.as_deref();
    let (status, payload) =
        match state.hermes.approve(&agent, &run_id, &body.choice.trim().to_lowercase(), request_id).await {
            Ok(r) => r,
            Err(exc) if exc.is_transport() => return Err(HttpError::new(502, "Hermes unreachable")),
            Err(exc) => return Err(anyhow::anyhow!(exc).into()),
        };
    if (200..300).contains(&status) {
        // kept for the conversation, with the request as it was shown (approvals_log.rs)
        let shown = state.hub.pending_approvals(Some(&agent.name)).into_iter().find(|p| {
            p.get("run_id").and_then(Value::as_str) == Some(run_id.as_str())
                && (request_id.is_none() || p.get("request_id").and_then(Value::as_str) == request_id)
        });
        let mut fields = shown.unwrap_or_default();
        fields.insert("run_id".into(), json!(run_id));
        fields.insert("request_id".into(), json!(request_id));
        fields.insert("choice".into(), json!(body.choice));
        fields.insert("via".into(), json!("notification"));
        fields.remove("created_at");
        if let Err(err) = crate::approvals_log::record(&state.config(), &agent.name, &fields) {
            tracing::warn!(agent = %agent.name, error = %err, "approval decision not kept");
        }
        state.hub.forget_approval(&agent.name, &run_id, request_id);
    }
    tracing::info!(agent = %agent.name, run_id = %run_id, status = status, "approval forwarded");
    Ok(web::json_response(status, &payload))
}

/// The approvals the person answered in a conversation (`agent`, `session_id`), the oldest first (approvals_log.rs).
async fn answered_approvals(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let agent = agent_or_404(&config, Some(&web::query_required(&q, "agent")?))?;
    let session = web::query_str(&q, "session_id", false, Some(200))?.filter(|s| !s.is_empty());
    let items = crate::approvals_log::list(&config, &agent.name, session.as_deref());
    Ok(web::ok(json!({"items": items})))
}

/// An app answered an approval (Hermes itself, or through the bridge): kept for the conversation.
async fn record_approval(State(state): State<AppState>, body: axum::body::Bytes) -> ApiResult<Response> {
    let config = state.config();
    let fields = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .ok_or_else(|| HttpError::new(422, "a JSON object"))?;
    let agent = agent_or_404(&config, fields.get("agent").and_then(Value::as_str))?;
    match crate::approvals_log::record(&config, &agent.name, &fields) {
        Ok(entry) => {
            // answered: no longer waiting
            let run = entry.get("run_id").and_then(Value::as_str).unwrap_or_default().to_string();
            state.hub.forget_approval(&agent.name, &run, entry.get("request_id").and_then(Value::as_str));
            Ok(web::ok(json!({"kept": entry})))
        }
        Err(err) => Err(HttpError::new(422, err.to_string())),
    }
}

async fn approvals(State(state): State<AppState>, q: QueryMap) -> ApiResult<Response> {
    let config = state.config();
    let shared = q.get("agent").filter(|a| crate::agent_shares::is_shared_with_me(&config, a)).map(str::to_string);
    let name = match q.get("agent").filter(|a| !a.is_empty() && shared.is_none()) {
        Some(a) => Some(agent_or_404(&config, Some(a))?.name),
        None => None,
    };
    let mut items = if shared.is_some() { Vec::new() } else { state.hub.pending_approvals(name.as_deref()) };
    // a relative's turn with one of this person's agents: theirs to answer, not in this Boîte (agent_shares.rs)
    items
        .retain(|i| !crate::agent_shares::relative_run(&config, i.get("run_id").and_then(Value::as_str).unwrap_or("")));
    // the approvals of this person's own turns with agents relatives share with her
    if name.is_none() {
        items.extend(crate::agent_shares::pending_mine(shared.as_deref()));
    }
    // the bridge's own cards too (a site for an agent's programs, sandbox_sites.rs)
    if shared.is_none() {
        items.extend(crate::sandbox_sites::pending_items(&config, name.as_deref()));
        items.extend(crate::vault_logins::pending_items(&config, name.as_deref()));
    }
    for item in items.iter_mut() {
        let created = item.get("created_at").and_then(Value::as_f64).unwrap_or(0.0);
        let micros = (created * 1_000_000.0).round() as i64;
        let when = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(micros).unwrap_or_default();
        item.insert("created_at".into(), json!(iso(when)));
    }
    Ok(web::ok(json!({"items": items})))
}

/// Python's `build_router()`: the core routes, mounted under /v1 behind the bridge key.
pub fn build_router() -> Router<AppState> {
    Router::new()
        .route("/agents", get(list_agents))
        .route("/tts/sentence", post(tts_sentence))
        .route("/tts/prepare", post(tts_prepare))
        .route("/tts/message", post(tts_message))
        .route("/tts/stream", post(tts_stream))
        .route("/files", post(upload_file))
        .route("/devices", post(register_device))
        .route("/devices/{token}", delete(delete_device))
        .route("/outbox", get(list_outbox))
        .route("/outbox/{item_id}", get(get_outbox))
        .route("/outbox/{item_id}/audio", get(get_outbox_audio))
        .route("/media", get(get_media))
        .route("/pages", get(list_pages))
        .route("/pages/{page}", delete(delete_page))
        .route("/sessions/{session_id}/state", get(session_state))
        .route("/status", get(status))
        .route("/pairing", get(pairing))
        .route("/replies/{reply_id}", get(get_reply))
        .route("/cron-jobs", get(cron_jobs))
        .route("/cron-jobs/{agent}/{job}", put(set_cron_job))
        .route("/watch", post(watch_run))
        .route("/runs/{run_id}/events", get(run_events))
        .route("/approve", post(approve))
        .route("/approvals", get(approvals))
        .route("/approvals/answered", get(answered_approvals).post(record_approval))
}

async fn health(State(state): State<AppState>) -> ApiResult<Response> {
    let config = state.config();
    let tts = &state.tts;
    let (kyutai_ok, ntfy_ok, pocket_ok, server_ok) = tokio::join!(
        tts.reachable(None),
        ntfy_reachable(&state.http, &config.ntfy_url),
        async {
            match tts.pocket.as_ref() {
                Some(engine) => tts.reachable(Some(engine)).await,
                None => None,
            }
        },
        async {
            match tts.server.as_ref() {
                Some(engine) => tts.reachable(Some(engine)).await,
                None => None,
            }
        },
    );
    // « voice_server »: the person's own voice model (null: none; false: set up, down for now). « drama »: the same
    // for the apps that offered Drama 3's voices by name, only when that server is Drama 3.
    let drama = config.voice_server.as_ref().map(|v| v.model.starts_with("drama")).unwrap_or(false);
    Ok(web::ok(json!({
        "ok": true, "version": VERSION, "kyutai": kyutai_ok, "pocket": pocket_ok,
        "voice_server": server_ok, "drama": if drama { json!(server_ok) } else { Value::Null }, "ntfy": ntfy_ok,
        "apns": state.apns.is_some(), "push": state.push_mode,
        // The app registers with the publisher's relay and sends the relay token (environment « relay »).
        "push_relay": state.push_service.is_some(), "fcm": state.fcm.is_some(),
        "tts_queue": tts.scheduler.depth(),
        // The space encrypted at rest (space.rs): « on » or « off » (a locked bridge answers « locked » itself).
        "space": crate::space::health_state(),
        // What this bridge offers that the Python one doesn't (the apps hide the rest).
        "features": ["agent_groups", "files_kept", "calendar", "health_categories", "contacts", "time_zone", "message_search"],
    })))
}

/// A request that only asks whether a route matches its path (see `redirect_slashes`): answered before any handler.
#[derive(Clone, Copy)]
struct Probe;

async fn probe(request: Request, next: Next) -> Response {
    if request.extensions().get::<Probe>().is_some() {
        let mut response = StatusCode::NO_CONTENT.into_response();
        response.headers_mut().insert("x-bip-probe", HeaderValue::from_static("1"));
        return response;
    }
    next.run(request).await
}

/// FastAPI's answer to an unknown path: Starlette's `redirect_slashes` first (the same path with or without its
/// trailing slash matches a route: 307 there), else 404 `{"detail": "Not Found"}`.
async fn not_found(routes: Router, request: Request) -> Response {
    let path = request.uri().path().to_string();
    let websocket = request
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    if path != "/" && !websocket {
        let other = if path.ends_with('/') { path.trim_end_matches('/').to_string() } else { format!("{path}/") };
        let other = if other.is_empty() { "/".to_string() } else { other };
        let query = request.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
        let mut probe_request =
            Request::builder().method(request.method().clone()).uri(format!("{other}{query}")).body(Body::empty());
        if let Ok(probe_request) = probe_request.as_mut() {
            probe_request.extensions_mut().insert(Probe);
        }
        if let Ok(probe_request) = probe_request {
            use tower::ServiceExt;
            let answer = match routes.oneshot(probe_request).await {
                Ok(answer) => answer,
                Err(never) => match never {},
            };
            {
                if answer.headers().contains_key("x-bip-probe") {
                    let (parts, _) = request.into_parts();
                    let scheme = crate::auth::scheme_of(&parts);
                    let host = parts.headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                    let location = format!("{scheme}://{host}{other}{query}");
                    let mut response = StatusCode::TEMPORARY_REDIRECT.into_response();
                    if let Ok(v) = HeaderValue::from_str(&location) {
                        response.headers_mut().insert("location", v);
                    }
                    response.headers_mut().insert("content-length", HeaderValue::from_static("0"));
                    return response;
                }
            }
        }
    }
    HttpError::new(404, "Not Found").into_response()
}

/// The paths served by a static folder (Starlette's StaticFiles answers HEAD; FastAPI's routes don't).
const STATIC_PREFIXES: [&str; 2] = ["/console/static/", "/desktop/static/novnc/"];

/// An HTTP method no route has: a HEAD request to an API route goes through as this one, to get FastAPI's 405.
const HEAD_ELSEWHERE: &str = "X-BIP-HEAD";

/// FastAPI's routes answer no HEAD (axum's `get` would run the GET handler): it reaches the router as
/// [`HEAD_ELSEWHERE`]. axum answers a known path with a wrong method by an empty 405: FastAPI's has a JSON detail
/// and `Allow: GET, POST` (no HEAD).
async fn method_not_allowed_detail(mut request: Request, next: Next) -> Response {
    if request.method() == axum::http::Method::HEAD
        && !STATIC_PREFIXES.iter().any(|p| request.uri().path().starts_with(p))
    {
        if let Ok(method) = axum::http::Method::from_bytes(HEAD_ELSEWHERE.as_bytes()) {
            *request.method_mut() = method;
        }
    }
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    if response.status() != StatusCode::METHOD_NOT_ALLOWED {
        // axum's method fallbacks add `Allow` to what they answer (DELETE /v1/family/name → the unlink's 404)
        response.headers_mut().remove("allow");
        return response;
    }
    if !response.headers().contains_key("content-type") {
        let allow = match first_route_methods(&path) {
            Some(methods) if !methods.is_empty() => Some(methods.to_string()),
            _ => response.headers().get("allow").and_then(|v| v.to_str().ok()).map(|v| {
                v.split(',').map(str::trim).filter(|m| !m.is_empty() && *m != "HEAD").collect::<Vec<_>>().join(", ")
            }),
        };
        let mut json = HttpError::new(405, "Method Not Allowed").into_response();
        if let Some(allow) = allow.and_then(|a| HeaderValue::from_str(&a).ok()) {
            json.headers_mut().insert("allow", allow);
        }
        return json;
    }
    response
}

/// Starlette answers a wrong method with the `Allow` of the FIRST route whose path matches, in declaration order
/// (one route per decorator: `@router.get("/vault")` then `@router.post("/vault")` gives `Allow: GET`); axum lists
/// every method of the path. Python's routes in `app.routes` order (its openapi order), each with that first route's
/// methods, as `HEAD` on the Python bridge answers them; "" = several methods in a Python set, axum's list kept.
const FIRST_ROUTE_METHODS: &[(&str, &str)] = &[
    ("/health", "GET"),
    ("/v1/agents", "GET"),
    ("/v1/tts/sentence", "POST"),
    ("/v1/tts/prepare", "POST"),
    ("/v1/tts/message", "POST"),
    ("/v1/tts/stream", "POST"),
    ("/v1/files", "POST"),
    ("/v1/devices", "POST"),
    ("/v1/devices/{token}", "DELETE"),
    ("/v1/outbox", "GET"),
    ("/v1/outbox/{item_id}", "GET"),
    ("/v1/outbox/{item_id}/audio", "GET"),
    ("/v1/media", "GET"),
    ("/v1/pages", "GET"),
    ("/v1/pages/{page}", "DELETE"),
    ("/v1/sessions/{session_id}/state", "GET"),
    ("/v1/status", "GET"),
    ("/v1/pairing", "GET"),
    ("/v1/replies/{reply_id}", "GET"),
    ("/v1/cron-jobs", "GET"),
    ("/v1/cron-jobs/{agent}/{job}", "PUT"),
    ("/v1/watch", "POST"),
    ("/v1/runs/{run_id}/events", "GET"),
    ("/v1/approve", "POST"),
    ("/v1/approvals", "GET"),
    ("/v1/approvals/answered", "GET"),
    ("/v1/approvals/answered", "POST"),
    ("/v1/relay/push", "POST"),
    ("/v1/agent-kinds", "GET"),
    ("/v1/agents/claim", "POST"),
    ("/v1/agents/{agent_id}/settings", "GET"),
    ("/v1/agents/{agent_id}/language", "PUT"),
    ("/v1/agents/{agent_id}/thinking", "GET"),
    ("/v1/agents/{agent_id}/model", "PUT"),
    ("/v1/agent-links", "GET"),
    ("/v1/agent-shares", "GET"),
    ("/v1/tool-catalog", "GET"),
    ("/v1/agents/{agent_id}/services", "POST"),
    ("/v1/agents/{agent_id}/services/{name}", "DELETE"),
    ("/v1/agents/{agent_id}/services/{name}/test", "POST"),
    ("/v1/agents/{agent_id}/space", "POST"),
    ("/v1/agents/{agent_id}", "DELETE"),
    ("/v1/vault", "GET"),
    ("/v1/vault/{item_id}", "PATCH"),
    ("/v1/backup", "GET"),
    ("/v1/backup/now", "POST"),
    ("/v1/place/move", "GET"),
    ("/v1/place/move/drain", "POST"),
    ("/v1/place/move/resume", "POST"),
    ("/v1/import", "POST"),
    ("/v1/import/hello", "GET"),
    ("/v1/import/archive", "PUT"),
    ("/v1/ssh", "GET"),
    ("/v1/ssh/keys", "POST"),
    ("/v1/device-keys", "GET"),
    ("/v1/device-keys/{device}", "DELETE"),
    ("/v1/agents/{agent_id}/memory", "GET"),
    ("/v1/agents/{agent_id}/memory/{which}", "PUT"),
    ("/v1/agents/{agent_id}/memory/pages/{path:path}", "GET"),
    ("/v1/mail/providers", "GET"),
    ("/v1/mail/accounts", "GET"),
    ("/v1/mail/accounts/{account_id}", "PATCH"),
    ("/v1/agents/{agent_id}/mail-scan", "GET"),
    ("/v1/agents/{agent_id}/mail-scan/step", "POST"),
    ("/v1/agents/{agent_id}/mail-scan/pause", "POST"),
    ("/v1/agents/{agent_id}/mail-scan/resume", "POST"),
    ("/v1/jobs", "GET"),
    ("/v1/agents/{agent_id}/jobs", "POST"),
    ("/v1/jobs/{job_id}/start", "POST"),
    ("/v1/jobs/{job_id}/pause", "POST"),
    ("/v1/jobs/{job_id}/resume", "POST"),
    ("/v1/jobs/{job_id}", "DELETE"),
    ("/v1/agents/{agent_id}/memory/import", "PUT"),
    ("/v1/reminders", "GET"),
    ("/v1/reminders/agents", "PUT"),
    ("/v1/reminders/queue", "GET"),
    ("/v1/reminders/queue/{op_id}", "POST"),
    ("/v1/health-data", "GET"),
    ("/v1/health-data/days", "PUT"),
    ("/v1/voices", "GET"),
    ("/v1/voices/uses", "PUT"),
    ("/v1/agents/{agent_id}/voices", "PUT"),
    ("/v1/agents/{agent_id}/appearance", "PUT"),
    ("/v1/live/brief", "GET"),
    ("/v1/live/calls", "POST"),
    ("/v1/live/recaps", "POST"),
    ("/v1/live/recaps/take", "POST"),
    ("/v1/favicon/{host}", "GET"),
    ("/v1/console/pairings/{code}", "POST"),
    ("/v1/console/devices", "GET"),
    ("/v1/console/devices/{device}", "DELETE"),
    ("/v1/search/messages", "GET"),
    ("/console", "GET"),
    ("/console/", "GET"),
    ("/console/api/login", "POST"),
    ("/console/api/pairing", "POST"),
    ("/console/api/pairing/wait", "POST"),
    ("/console/api/me", "GET"),
    ("/console/api/logout", "POST"),
    ("/v1/setup", "GET"),
    ("/v1/setup/claim", "POST"),
    ("/hermes/{agent}/{path:path}", ""),
    ("/peer/p/{target}/{path:path}", ""),
    ("/v1/agent-tools/mcp", "POST"),
    ("/v1/agent-tools/files", "POST"),
    ("/family/accept", "POST"),
    ("/family/agents", "GET"),
    ("/family/p/{target}/{path:path}", ""),
    ("/v1/family", "GET"),
    ("/v1/family/name", "PUT"),
    ("/v1/family/invite", "POST"),
    ("/v1/family/join", "POST"),
    ("/v1/family/{household}", "DELETE"),
    ("/family/tell", "POST"),
    ("/v1/family/news", "GET"),
    ("/v1/family/proposals/{id}/send", "POST"),
    ("/v1/family/proposals/{id}", "DELETE"),
    ("/v1/family/received/{id}/seen", "POST"),
    ("/v1/agents/{agent_id}/desktop", "POST"),
    ("/v1/agents/{agent_id}/desktop/control", "POST"),
    ("/v1/agents/{agent_id}/desktop/state", "GET"),
    ("/v1/agents/{agent_id}/desktop/open", "POST"),
    ("/desktop/{ticket}/", "GET"),
    ("/desktop/{ticket}/page/", "GET"),
];

/// Starlette's path match: `{x}` one non-empty segment, a final `{x:path}` the rest.
fn template_matches(template: &str, path: &str) -> bool {
    let mut parts = path.split('/');
    for segment in template.split('/') {
        if segment.starts_with('{') && segment.ends_with(":path}") {
            return true;
        }
        match parts.next() {
            Some(part) if segment.starts_with('{') => {
                if part.is_empty() {
                    return false;
                }
            }
            Some(part) if part == segment => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

/// The methods Starlette's 405 lists for `path` (None: no Python route has it).
fn first_route_methods(path: &str) -> Option<&'static str> {
    FIRST_ROUTE_METHODS.iter().find(|(template, _)| template_matches(template, path)).map(|(_, methods)| *methods)
}

/// What `create_app` takes besides the configuration.
#[derive(Debug, Clone)]
pub struct AppOptions {
    /// The ntfy subscribers, the cron watcher, the purge loop… (tests turn them off, as Python's `start_background`).
    pub start_background: bool,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions { start_background: true }
    }
}

/// The application: its router (ready to serve) and its state (to stop it).
pub struct App {
    pub router: Router,
    pub state: AppState,
}

impl App {
    /// Python's lifespan end: `services.stop()`.
    pub async fn shutdown(&self) {
        self.state.stop().await;
    }
}

/// Python's `create_app(config)`, with its lifespan's start.
pub fn create_app(config: Config, options: AppOptions) -> anyhow::Result<App> {
    let services = Services::new(config)?;
    let state = AppState(Arc::new(services));
    let config = state.config();
    agents_admin::heal_thinking(&config);
    agents_admin::heal_souls(&config);
    crate::guide::refresh(&config); // the guide's list of agents and the setup blocks, as this bridge writes them
                                    // The skills about other agents as this bridge writes them (an update may have changed their words).
    crate::peers::write_contacts(&config, &crate::peers::links_of(&state.links.load()), Some(&state.households));
    crate::shares::heal_skills(&config);
    if options.start_background {
        state.start_background();
    }
    let mut agent_names: Vec<&String> = config.agents.keys().collect();
    agent_names.sort();
    tracing::info!(version = VERSION, agents = ?J(&agent_names), apns = state.apns.is_some(), "bridge started");
    let router = assemble(&state, &config);
    Ok(App { router, state })
}

fn assemble(state: &AppState, config: &Config) -> Router {
    let auth = || middleware::from_fn_with_state(state.clone(), require_bridge_key);
    let (import_admin, import_open) = crate::place_import::place_import_routers(None);
    // Everything Python mounts under /v1 with Depends(require_bridge_key), in its order.
    let mut authed = Router::new()
        .merge(build_router())
        .merge(agents_admin::router())
        .merge(crate::agent_groups::router())
        .merge(crate::files_kept::router())
        .merge(crate::dates::router())
        .merge(crate::vault::router())
        .merge(crate::card_pay::app_router())
        .merge(crate::vault_logins::app_router())
        .merge(crate::place_backup::router())
        .merge(crate::place_move::router())
        .merge(import_admin)
        .merge(crate::ssh_keys::router())
        .merge(crate::device_keys::router())
        .merge(crate::memory_edit::router())
        .merge(crate::mailbox::router())
        .merge(crate::agent_email::router())
        .merge(crate::sandbox_sites::router())
        .merge(crate::mail_scan::router())
        .merge(crate::jobs::router())
        .merge(crate::notes_import::router())
        .merge(crate::assistant_import::router())
        .merge(crate::guide::router())
        .merge(crate::rooms::router())
        .merge(crate::reminders::router())
        .merge(crate::notebook::api::router())
        .merge(crate::calendar::router())
        .merge(crate::contacts::router())
        .merge(crate::archives::router())
        .merge(crate::archives::records_fill::router())
        .merge(crate::whatsapp_import::router())
        .merge(crate::health::router())
        .merge(crate::voices::router())
        .merge(crate::live_brief::router())
        .merge(crate::favicons::router())
        .merge(crate::map_tiles::router())
        .merge(crate::message_search::router())
        .merge(crate::connections::router())
        .merge(crate::live_activities::router());
    if !config.cloud {
        authed = authed.merge(crate::console::console_admin_router());
    }
    authed = authed
        .merge(crate::peers::household_admin_router())
        .merge(crate::family_news::router())
        .merge(crate::agent_shares::router())
        .merge(crate::desktop::desktop_admin_router());
    // Under /v1 without the bridge key: a computer sending its agents (the app's one-time code instead), and a
    // server the app created until the app claims it (its own proofs).
    let open = Router::new().merge(import_open).merge(crate::cloud_setup::router());

    let mut app = Router::new()
        .route("/health", get(health))
        .merge(Router::new().nest("/v1", authed.route_layer(auth())))
        .merge(crate::relay::router());
    if !config.cloud {
        // on the Internet (a server the app created), no web console: one door less
        app = app.merge(crate::console::console_router()).nest("/console/static", crate::console::console_files());
    }
    app = app
        .merge(Router::new().nest("/v1", open))
        // The agents' Hermes through the bridge: on the LAN door, and for agents in their own space (their gateway
        // listens on a local port only). Hermes checks each agent's own key.
        .merge(crate::lan::hermes_proxy())
        .merge(crate::peers::peer_router()) // agents' own tokens, checked against the links the person allowed
        .merge(crate::sandbox::router()) // the agents' file tools (shared folders), same tokens
        .merge(crate::card_pay::router()) // the agents' card payment tools, same tokens
        .merge(crate::vault_logins::router()) // the agents' vault tools (accounts with a vault password), same tokens
        .merge(crate::agent_tools::router()) // the agents' MCP tools (create_audio), each agent with its own token
        .merge(crate::connections::callback_router()) // a service's sign-in coming back (its single-use state)
        .merge(crate::peers::family_router()) // the linked households' bridges, with the key each was given
        .merge(crate::family_news::household_router()) // their family news, the same key
        .merge(crate::agent_shares::household_router()) // the agents shared with them, the same key
        .merge(crate::rooms_remote::household_router()) // their agents in this person's discussions, the same key
        .nest("/desktop/static/novnc", crate::desktop::static_files())
        .merge(crate::desktop::desktop_router()) // the page and its stream: a short-lived ticket from the route above
        .route("/v1/voice", get(crate::voice_ws::voice_endpoint));
    // The same routes, answering « matched » without running anything: what `redirect_slashes` asks.
    let routes: Router = app.clone().route_layer(middleware::from_fn(probe)).with_state(state.clone());
    let inner: Router =
        app.fallback(move |request: Request| not_found(routes.clone(), request)).with_state(state.clone());
    // Python's middlewares wrap the whole app (before routing): here around the routed app, as one service.
    // FastAPI reads bodies of any size (axum's extractors stop at 2 MB by default); an exception in a route is a
    // plain-text 500, logged as uvicorn does.
    Router::new()
        .fallback_service(inner)
        .layer(middleware::from_fn(method_not_allowed_detail))
        .layer(middleware::from_fn_with_state(state.clone(), crate::place_move::guard))
        .layer(middleware::from_fn_with_state(state.clone(), crate::files::upload_size_limit))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(internal_error))
}

/// A route that panicked (a bug, or a `todo!()` not ported yet): Starlette's 500, uvicorn's log line.
fn internal_error(panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "panic".to_string());
    tracing::error!(target: "uvicorn.error", exc = %message, "Exception in ASGI application\n");
    HttpError::internal().into_response()
}
