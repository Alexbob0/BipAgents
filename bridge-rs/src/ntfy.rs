//! ntfy -> outbox: one long-lived `GET {ntfy}/{topic}/json` subscription per agent.
//!
//! Each `message` event is stored in the outbox, its audio pre-synthesized (Kyutai mp3, background
//! priority so live voice is never delayed), then pushed (`MESSAGE`) to the agent's devices. The
//! last ntfy message id is persisted and sent back as `since=` on reconnection, so nothing is lost
//! or duplicated across restarts (within ntfy's cache window).
//!
//! Port of `bridge/bipbridge/ntfy.py` (owner group: push-runs).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use base64::Engine;
use chrono::{TimeZone, Utc};
use futures::future::{BoxFuture, Shared};
use futures::{FutureExt, StreamExt};
use regex::Regex;
use serde_json::{json, Value};
use tokio::task::{AbortHandle, JoinHandle};

use crate::config::{AgentConfig, SharedConfig};
use crate::hermes::HermesError;
use crate::media::{extract_media, resolve, without_media_lines};
use crate::push::pyx::{self, PyError};
use crate::push::PushService;
use crate::store::{iso, NewOutbox, Row, Store};
use crate::tts::{SynthOptions, TtsService, BACKGROUND};
use crate::Dict;

pub const BACKOFF_START: f64 = 0.5;
pub const BACKOFF_MAX: f64 = 8.0;

/// `^session[:_=](.+)$` (Python's `$` also matches before a final newline).
static SESSION_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^session[:_=](.+)\n?\z").expect("valid regex"));
static CRON_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*Cronjob Response:\s*(?P<name>[^\n]*?)\s*(?:\(job_id:\s*[\w-]+\))?\s*\n\s*-{3,}\s*\n?")
        .expect("valid regex")
});
/// Footer notes Hermes appends (wording varies by version), possibly several.
static CRON_FOOTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\n\s*(?:Note: The agent cannot see this message|To stop or manage this job)[^\n]*\s*$")
        .expect("valid regex")
});

/// Whether a `MEDIA:` path is an audio file (by its extension).
fn is_audio(path: &str) -> bool {
    crate::media::content_type_of(path).is_some_and(|t| t.starts_with("audio/"))
}

/// A message's audio being made (`ensure_audio`): the mp3's path once done, None when it failed. Clones await the
/// same work; it goes on when every waiter gave up (Python's task under `asyncio.shield`).
pub type AudioTask = Shared<BoxFuture<'static, Option<String>>>;

/// Hermes wraps cron deliveries in a header (« Cronjob Response: <job> (job_id: …) » + dashes) and a
/// footer note. Returns `(job name, agent output)`; other messages come back unchanged.
pub fn unwrap_cron(text: &str) -> (Option<String>, String) {
    let text = pyx::strip(text);
    let Some(caps) = CRON_HEADER.captures(text) else {
        return (None, text.to_string());
    };
    let end = caps.get(0).map(|m| m.end()).unwrap_or(0);
    let mut body = pyx::strip(&text[end..]).to_string();
    loop {
        let wrapped = format!("\n{body}");
        let trimmed = pyx::strip(&CRON_FOOTER.replace_all(&wrapped, "")).to_string();
        if trimmed == body {
            break;
        }
        body = trimmed;
    }
    let name = pyx::strip(caps.name("name").map(|m| m.as_str()).unwrap_or("")).to_string();
    (if name.is_empty() { None } else { Some(name) }, body)
}

/// ntfy's `Authorization`: Basic for `user:password`, Bearer for an access token (`tk_…`).
pub fn auth_header(token: Option<&str>) -> Option<(&'static str, String)> {
    let token = token.filter(|t| !t.is_empty())?;
    if token.contains(':') && !token.starts_with("tk_") {
        return Some((
            "Authorization",
            format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(token.as_bytes())),
        ));
    }
    Some(("Authorization", format!("Bearer {token}")))
}

/// `datetime.fromtimestamp(t, tz=timezone.utc)` (microseconds rounded half to even, as Python does).
fn from_timestamp(t: f64) -> Option<chrono::DateTime<Utc>> {
    let whole = t.trunc();
    let mut us = (t.fract() * 1e6).round_ties_even() as i64;
    let mut secs = whole as i64;
    if us >= 1_000_000 {
        secs += 1;
        us -= 1_000_000;
    } else if us < 0 {
        secs -= 1;
        us += 1_000_000;
    }
    Utc.timestamp_opt(secs, (us * 1000) as u32).single()
}

/// `shutil.copyfile`: the content only (the new file gets the default mode, not the source's).
fn copyfile(src: &str, dst: &str) -> std::io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    let mut output = std::fs::File::create(dst)?;
    std::io::copy(&mut input, &mut output)?;
    Ok(())
}

/// Stores messages, synthesizes their audio and sends the push.
pub struct OutboxService {
    pub config: SharedConfig,
    pub store: Store,
    pub tts: Arc<TtsService>,
    pub push: Arc<PushService>,
    audio_tasks: Mutex<HashMap<String, (AudioTask, AbortHandle)>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl OutboxService {
    pub fn new(config: SharedConfig, store: Store, tts: Arc<TtsService>, push: Arc<PushService>) -> Self {
        OutboxService {
            config,
            store,
            tts,
            push,
            audio_tasks: Mutex::new(HashMap::new()),
            tasks: Mutex::new(Vec::new()),
        }
    }

    pub fn audio_file(&self, item_id: &str) -> String {
        crate::py::join(&self.config.get().outbox.audio_dir, &format!("{item_id}.mp3"))
    }

    /// Stores a message and pushes it (`notify=false`: stored silently, still in the Boîte).
    /// `replace`: a message already stored under the same id but with another text is updated and pushed again.
    pub async fn ingest(
        self: &Arc<Self>,
        agent: &AgentConfig,
        message: &Dict,
        notify: bool,
        replace: bool,
    ) -> anyhow::Result<Option<Row>> {
        let raw = message.get("message").filter(|m| crate::py::truthy(m)).map(crate::py::py_str).unwrap_or_default();
        let (job_title, text) = unwrap_cron(&raw);
        let (spoken, media_paths) = extract_media(&text);
        if spoken.is_empty() && media_paths.is_empty() {
            return Ok(None);
        }
        // The files the apps show stay in the text as its `MEDIA:` lines, in their places (a report's page, a
        // picture, a document: 2026-10-26, a hand-back's « Le rapport complet : » had lost its page); the first mp3
        // the bridge serves is the message's own audio instead (`attach_media_audio`), its line taken out. Any other
        // audio line stays (a podcast in .m4a or .wav, an mp3 the bridge can't reach): the app plays it as a file.
        let own_mp3 = self.own_mp3(&media_paths);
        let mut text = without_media_lines(&text, |path| own_mp3.as_ref().is_some_and(|(p, _)| p == path));
        if text.is_empty() {
            text = "🎧".into();
        }
        let title = match message.get("title") {
            Some(Value::String(t)) => Some(t.clone()),
            _ => job_title,
        };
        let mut session_id = None;
        let tags: Vec<Value> = match message.get("tags") {
            None => Vec::new(),
            Some(v) if !crate::py::truthy(v) => Vec::new(),
            Some(Value::Array(items)) => items.clone(),
            Some(Value::String(s)) => s.chars().map(|c| json!(c.to_string())).collect(),
            Some(Value::Object(map)) => map.keys().map(|k| json!(k)).collect(),
            Some(_) => return Err(PyError::new("TypeError", "the message's tags are not iterable").into()),
        };
        for tag in &tags {
            if let Some(caps) = SESSION_TAG.captures(&crate::py::py_str(tag)) {
                session_id = Some(caps[1].to_string());
            }
        }
        let mut sent_at = None;
        if let Some(t) = message.get("time") {
            let seconds = match t {
                Value::Number(n) => n.as_f64(),
                Value::Bool(b) => Some(*b as i64 as f64),
                _ => None,
            };
            if let Some(seconds) = seconds {
                let when =
                    from_timestamp(seconds).ok_or_else(|| PyError::new("OverflowError", "timestamp out of range"))?;
                sent_at = Some(iso(when));
            }
        }
        let ntfy_id = match message.get("id") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => Some(crate::py::py_str(other)),
        };
        let extra = NewOutbox { title: title.clone(), ntfy_id: ntfy_id.clone(), sent_at, session_id };
        let mut item = self.store.add_outbox(&agent.name, &text, extra).await?;
        let chars = pyx::len(&text);
        let mut notify = notify;
        if item.is_none() && replace && message.get("id").map(crate::py::truthy).unwrap_or(false) {
            let id = message.get("id").map(crate::py::py_str).unwrap_or_default();
            item = self.store.replace_outbox(&agent.name, &id, &text, title.as_deref()).await?;
            if let Some(row) = &item {
                // a correction of a message already pushed (a reply stored too early, or one an older bridge wrote
                // differently, read again after a restart): the Boîte shows the new text, no second push (aibox
                // 2026-10-26: the morning routine's podcast pushed again after `bipagents update`)
                notify = false;
                tracing::info!(agent = %agent.name, id = %crate::py::py_str(&row["id"]), chars = chars, "outbox message replaced");
            }
        }
        let Some(item) = item else {
            return Ok(None); // duplicate (already ingested)
        };
        tracing::info!(agent = %agent.name, id = %crate::py::py_str(&item["id"]), chars = chars, "outbox message stored");
        let item = match &own_mp3 {
            Some((_, host)) => self.attach_media_audio(item, host).await?,
            None => item,
        };
        let (this, agent, delivered) = (self.clone(), agent.clone(), item.clone());
        let handle = tokio::spawn(async move { this.deliver(&agent, &delivered, notify).await });
        let mut tasks = self.tasks.lock().unwrap_or_else(|e| e.into_inner());
        tasks.retain(|t| !t.is_finished());
        tasks.push(handle);
        Ok(Some(item))
    }

    /// The first mp3 among `paths` the bridge serves (`[media] roots`, its own audio folder…): `(path, host file)`.
    fn own_mp3(&self, paths: &[String]) -> Option<(String, String)> {
        let config = self.config.get();
        paths.iter().filter(|p| p.to_lowercase().ends_with(".mp3")).find_map(|path| {
            let host = resolve(path, &config.media_roots).or_else(|| crate::agent_tools::served_media(&config, path))?;
            Some((path.clone(), host))
        })
    }

    /// The message's own mp3 (a podcast, `host`) becomes its audio instead of a synthesized reading.
    async fn attach_media_audio(&self, item: Row, host: &str) -> anyhow::Result<Row> {
        let id = crate::py::py_str(&item["id"]);
        let target = self.audio_file(&id);
        crate::py::makedirs(&crate::py::dirname(&target), 0o700)?;
        let (src, dst) = (host.to_string(), target.clone());
        tokio::task::spawn_blocking(move || copyfile(&src, &dst)).await??;
        self.store.set_outbox_audio(&id, &target).await?;
        let mut item = item;
        item.insert("audio_path".into(), json!(target));
        Ok(item)
    }

    /// Wait (bounded) for the pre-synthesized audio, then push.
    async fn deliver(self: &Arc<Self>, agent: &AgentConfig, item: &Row, notify: bool) {
        if let Some(task) = self.ensure_audio(item, Some(agent)) {
            let wait = self.config.get().outbox.audio_wait_seconds.max(0.0);
            // push now when it takes longer; the audio route serves it once ready
            let _ = tokio::time::timeout(Duration::from_secs_f64(wait), task).await;
        }
        if !notify {
            return;
        }
        let id = crate::py::py_str(&item["id"]);
        let (text, _) = extract_media(item.get("text").and_then(Value::as_str).unwrap_or("")); // its words only
        let session_id = item.get("session_id").and_then(Value::as_str);
        let title = item.get("title").and_then(Value::as_str);
        self.push.notify_message(&agent.name, &id, &text, session_id, title).await;
    }

    pub async fn close(&self) {
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *self.tasks.lock().unwrap_or_else(|e| e.into_inner()));
        let audio: Vec<(AudioTask, AbortHandle)> =
            self.audio_tasks.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        for task in &tasks {
            task.abort();
        }
        for (_, abort) in &audio {
            abort.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
        for (task, _) in audio {
            let _ = task.await;
        }
    }

    /// The message's audio, being made (started now if needed); None when it has none to make.
    pub fn ensure_audio(self: &Arc<Self>, item: &Row, agent: Option<&AgentConfig>) -> Option<AudioTask> {
        if let Some(path) = item.get("audio_path").filter(|p| crate::py::truthy(p)) {
            if std::path::Path::new(&crate::py::py_str(path)).exists() {
                return None;
            }
        }
        let config = self.config.get();
        // its words read, not its files' lines
        let (text, files) = extract_media(item.get("text").and_then(Value::as_str).unwrap_or(""));
        if text.is_empty() || pyx::len(&text) > config.outbox.audio_max_chars {
            return None;
        }
        if files.iter().any(|f| is_audio(f)) {
            // a message that brings its own audio (a routine's podcast in .m4a) is never read aloud on top: the
            // iPhone took the reading for the message's audio and left the podcast out (aibox 2026-10-30: the
            // routine's text arrived, its podcast didn't)
            return None;
        }
        let id = crate::py::py_str(&item["id"]);
        let mut audio_tasks = self.audio_tasks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((task, _)) = audio_tasks.get(&id) {
            return Some(task.clone());
        }
        let owner = item.get("agent").and_then(Value::as_str);
        let agent = agent.cloned().or_else(|| config.agent(owner).cloned());
        let voice = crate::voices::voice_for(&config, agent.as_ref(), Some("reading"));
        let this = self.clone();
        let task_id = id.clone();
        let handle = tokio::spawn(async move {
            let path = this.synthesize(&task_id, &text, &voice).await;
            this.audio_tasks.lock().unwrap_or_else(|e| e.into_inner()).remove(&task_id);
            path
        });
        let abort = handle.abort_handle();
        let task: AudioTask = async move { handle.await.ok().flatten() }.boxed().shared();
        audio_tasks.insert(id, (task.clone(), abort));
        Some(task)
    }

    async fn synthesize(&self, item_id: &str, text: &str, voice: &str) -> Option<String> {
        let options = SynthOptions { max_chars: Some(0), ..Default::default() };
        let audio = match self.tts.synthesize(text, Some(voice), "mp3", BACKGROUND, options).await {
            Ok(audio) => audio,
            Err(exc) => {
                tracing::warn!(id = %item_id, error = %exc.message, "outbox audio failed");
                return None;
            }
        };
        let path = self.audio_file(item_id);
        let written: anyhow::Result<()> = async {
            crate::py::makedirs(&crate::py::dirname(&path), 0o700)?;
            let tmp = format!("{path}.part");
            let (t, p, data) = (tmp.clone(), path.clone(), audio.data.clone());
            tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                std::fs::write(&t, &data)?;
                std::fs::rename(&t, &p)
            })
            .await??;
            self.store.set_outbox_audio(item_id, &path).await?;
            Ok(())
        }
        .await;
        match written {
            Ok(()) => Some(path),
            Err(exc) => {
                tracing::error!(exc = %format!("{exc:#}"), id = %item_id, "Task exception was never retrieved");
                None
            }
        }
    }

    pub async fn purge(&self) -> anyhow::Result<usize> {
        let removed = self.store.purge_outbox(self.config.get().outbox.retention_days).await?;
        for row in &removed {
            if let Some(path) = row.get("audio_path").filter(|p| crate::py::truthy(p)) {
                let _ = std::fs::remove_file(crate::py::py_str(path));
            }
        }
        Ok(removed.len())
    }
}

pub struct NtfySubscriber {
    pub client: reqwest::Client,
    pub base_url: String,
    pub agent: AgentConfig,
    pub store: Store,
    pub outbox: Arc<OutboxService>,
    pub backoff_start: f64,
    pub backoff_max: f64,
    connected: AtomicBool,
}

impl NtfySubscriber {
    pub fn new(
        client: reqwest::Client,
        base_url: &str,
        agent: AgentConfig,
        store: Store,
        outbox: Arc<OutboxService>,
    ) -> Self {
        NtfySubscriber {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            agent,
            store,
            outbox,
            backoff_start: BACKOFF_START,
            backoff_max: BACKOFF_MAX,
            connected: AtomicBool::new(false),
        }
    }

    /// Whether the subscription is open now.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Follows the agent's ntfy topic for ever (reconnecting with a backoff).
    pub async fn run(self: Arc<Self>) {
        let mut backoff = self.backoff_start;
        loop {
            match self.stream_once().await {
                Ok(got_data) => {
                    if got_data {
                        backoff = self.backoff_start;
                    }
                }
                Err(exc) => {
                    let transport = match exc.downcast_ref::<HermesError>() {
                        Some(e) => e.is_transport(),
                        None => exc.downcast_ref::<reqwest::Error>().is_some(),
                    };
                    if transport {
                        tracing::info!(agent = %self.agent.name, error = pyx::error_name(&exc), "ntfy stream lost");
                    } else {
                        tracing::error!(exc = %format!("{exc:#}"), agent = %self.agent.name, "ntfy subscriber error");
                    }
                }
            }
            self.connected.store(false, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs_f64(backoff)).await;
            backoff = (backoff * 2.0).min(self.backoff_max);
        }
    }

    async fn stream_once(&self) -> anyhow::Result<bool> {
        let cursor = self.store.get_cursor(&self.agent.name).await?;
        let url = format!("{}/{}/json", self.base_url, self.agent.ntfy_topic.as_deref().unwrap_or(""));
        let mut request = self.client.get(url);
        if let Some(cursor) = cursor.filter(|c| !c.is_empty()) {
            request = request.query(&[("since", cursor)]);
        }
        if let Some((name, value)) = auth_header(self.agent.ntfy_token.as_deref()) {
            request = request.header(name, value);
        }
        // httpx.Timeout(connect=5.0, read=120.0): ntfy sends a keepalive every ~45 s
        let resp = match tokio::time::timeout(Duration::from_secs(125), request.send()).await {
            Ok(resp) => resp.map_err(HermesError::from)?,
            Err(_) => return Err(HermesError::ReadTimeout.into()),
        };
        let status = resp.status().as_u16();
        if status != 200 {
            tracing::warn!(agent = %self.agent.name, status = status, "ntfy refused subscription");
            return Ok(false);
        }
        self.connected.store(true, Ordering::SeqCst);
        let mut got_data = false;
        let lines =
            crate::hermes::lines_of(crate::hermes::read_with_timeout(resp.bytes_stream(), Duration::from_secs(120)));
        futures::pin_mut!(lines);
        while let Some(line) = lines.next().await {
            let line = line?;
            let line = pyx::strip(&line);
            if line.is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(line) else { continue };
            got_data = true;
            let Value::Object(event) = event else {
                return Err(PyError::new("AttributeError", "an ntfy event is not an object").into());
            };
            if event.get("event").and_then(Value::as_str) != Some("message") {
                continue;
            }
            self.outbox.ingest(&self.agent, &event, true, false).await?;
            if let Some(id) = event.get("id").filter(|i| crate::py::truthy(i)) {
                self.store.set_cursor(&self.agent.name, &crate::py::py_str(id)).await?;
            }
        }
        Ok(got_data)
    }
}

pub async fn ntfy_reachable(client: &reqwest::Client, base_url: &str) -> bool {
    let url = format!("{}/v1/health", base_url.trim_end_matches('/'));
    match client.get(url).timeout(Duration::from_secs(2)).send().await {
        Ok(resp) => resp.status().as_u16() == 200,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_cron_keeps_the_job_name_and_output() {
        let wrapped = "Cronjob Response: test-push-bipagents (job_id: 0681ac73805e)\n-------------\n\n\
                       Bonjour Alex, ceci est un test.\n\nNote: The agent cannot see this message, and therefore cannot respond to it.";
        assert_eq!(
            unwrap_cron(wrapped),
            (Some("test-push-bipagents".into()), "Bonjour Alex, ceci est un test.".into())
        );
        assert_eq!(
            unwrap_cron("Cronjob Response: Morning feeds\n-------------\n\nLe point du matin."),
            (Some("Morning feeds".into()), "Le point du matin.".into())
        );
        assert_eq!(unwrap_cron("  Un message normal.  "), (None, "Un message normal.".into()));
        let hermes_021 =
            "Cronjob Response: Test push propre (job_id: 1a2b3c4d5e6f)\n-------------\n\nCoucou, test réussi.\n\n\
                          To stop or manage this job, send me a new message (e.g. \"stop reminder Test push propre\").";
        assert_eq!(unwrap_cron(hermes_021), (Some("Test push propre".into()), "Coucou, test réussi.".into()));
        let both =
            format!("{hermes_021}\n\nNote: The agent cannot see this message, and therefore cannot respond to it.");
        assert_eq!(unwrap_cron(&both), (Some("Test push propre".into()), "Coucou, test réussi.".into()));
    }

    #[test]
    fn auth_headers() {
        assert_eq!(auth_header(None), None);
        assert_eq!(auth_header(Some("")), None);
        assert_eq!(auth_header(Some("tk_abc")), Some(("Authorization", "Bearer tk_abc".to_string())));
        assert_eq!(auth_header(Some("tk_a:b")), Some(("Authorization", "Bearer tk_a:b".to_string())));
        assert_eq!(auth_header(Some("user:pass")), Some(("Authorization", "Basic dXNlcjpwYXNz".to_string())));
    }

    #[test]
    fn timestamps_like_python() {
        assert_eq!(iso(from_timestamp(1790000000.0).unwrap()), "2026-09-21T14:13:20.000Z");
        assert_eq!(iso(from_timestamp(1.5).unwrap()), "1970-01-01T00:00:01.500Z");
        assert_eq!(iso(from_timestamp(-1.25).unwrap()), "1969-12-31T23:59:58.750Z");
    }
}
