//! The Boîte's messages (`outbox` table): stored, their audio pre-synthesized (background priority, so a Live is
//! never delayed), then pushed (`MESSAGE`) to the agent's devices. The bridge's own producers hand them in:
//! background turns, routines' reports, imports, the credit's and the mail rules' notes, a space's files.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use serde_json::{json, Value};
use tokio::task::{AbortHandle, JoinHandle};

use crate::config::{AgentConfig, SharedConfig};
use crate::media::{extract_media, resolve, without_media_lines};
use crate::notifications::BoiteFrom;
use crate::push::pyx;
use crate::push::PushService;
use crate::store::{NewOutbox, Row, Store};
use crate::tts::{SynthOptions, TtsService, BACKGROUND};

/// Whether a `MEDIA:` path is an audio file (by its extension).
fn is_audio(path: &str) -> bool {
    crate::media::content_type_of(path).is_some_and(|t| t.starts_with("audio/"))
}

/// A message's audio being made (`ensure_audio`): the mp3's path once done, None when it failed. Clones await the
/// same work; it goes on when every waiter gave up.
pub type AudioTask = Shared<BoxFuture<'static, Option<String>>>;

/// A message for the Boîte. `id`: the producer's key, stored once (a second one with the same id is dropped).
#[derive(Debug, Clone, Default)]
pub struct BoiteMessage {
    pub id: Option<String>,
    pub text: String,
    pub title: Option<String>,
    pub session_id: Option<String>,
    /// Files given besides the text's `MEDIA:` lines (paths as the agent sees them).
    pub files: Vec<String>,
}

impl BoiteMessage {
    pub fn new(text: impl Into<String>) -> Self {
        BoiteMessage { text: text.into(), ..Default::default() }
    }
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
    pub async fn ingest(
        self: &Arc<Self>,
        agent: &AgentConfig,
        message: BoiteMessage,
        notify: bool,
    ) -> anyhow::Result<Option<Row>> {
        let text = pyx::strip(&message.text).to_string();
        let (spoken, media_paths) = extract_media(&text);
        if spoken.is_empty() && media_paths.is_empty() {
            return Ok(None);
        }
        // The files the apps show are the message's attachments (2026-11-03: items, no `MEDIA:` line left in its
        // text; `offset` keeps a page's place in it), and those `files` the sender gives; the first mp3 the bridge
        // serves is the message's own audio instead (`attach_media_audio`). Any other audio (a podcast in .m4a or
        // .wav, an mp3 the bridge can't reach) stays a file the app plays.
        let own_mp3 = self.own_mp3(&media_paths);
        let kept = without_media_lines(&text, |path| own_mp3.as_ref().is_some_and(|(p, _)| p == path));
        let (mut text, mut files) = crate::threads::timeline::visible(&kept, "", 0);
        for path in &message.files {
            if !files.iter().any(|f| f["path"] == path.as_str()) {
                files.push(crate::threads::timeline::file_attachment(path, None));
            }
        }
        for file in files.iter_mut() {
            file.as_object_mut().map(|f| f.remove("id"));
        }
        if text.is_empty() && files.is_empty() {
            text = "🎧".into();
        }
        let attachments = (!files.is_empty()).then(|| Value::Array(files).to_string());
        let extra = NewOutbox { title: message.title, ntfy_id: message.id, session_id: message.session_id, attachments };
        let Some(item) = self.store.add_outbox(&agent.name, &text, extra).await? else {
            return Ok(None); // duplicate (already ingested)
        };
        tracing::info!(agent = %agent.name, id = %crate::py::py_str(&item["id"]), chars = pyx::len(&text), "outbox message stored");
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
        let session_id = item.get("session_id").and_then(Value::as_str);
        if let Some(threads) = crate::threads::upgrade(&self.push.threads) {
            threads.touch(&agent.name, session_id, "boite").await;
        }
        let id = crate::py::py_str(&item["id"]);
        let (text, _) = extract_media(item.get("text").and_then(Value::as_str).unwrap_or("")); // its words only
        // whose: a background task's hand-back follows « Me prévenir », a routine's report its own mute
        let ntfy_id = item.get("ntfy_id").and_then(Value::as_str).unwrap_or("");
        let from = if ntfy_id.starts_with("handback:") {
            BoiteFrom::Task
        } else if ntfy_id.starts_with("hermes-session:") || ntfy_id.starts_with("routine:") {
            BoiteFrom::Routine
        } else {
            BoiteFrom::Other
        };
        self.push.notify_message(&agent.name, &id, &text, session_id, from, notify).await;
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
