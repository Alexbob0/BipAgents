//! TTS client (Kyutai 1.6B, optional Kyutai Pocket TTS, optional voice server of the person's own: Fish Audio Drama 3,
//! Kokoro…): FIFO scheduler per engine, WAV -> PCM16 conversion, resampling to 24 kHz, small LRU cache.
//!
//! Kyutai (`context/kyutai_server.py`) serializes synthesis behind a global lock and returns whole
//! files. The bridge mirrors that with its own queue so requests reach Kyutai strictly in arrival
//! order (FIFO), interactive requests (voice sentences, `/v1/tts/sentence`) ahead of background ones
//! (outbox pre-synthesis), and a cancelled waiter simply leaves the queue.
//!
//! Port of `bridge/bipbridge/tts.py` (owner group: voice-speech). The resampling is a windowed-sinc filter of our
//! own (Python uses soxr's HQ): the same lengths and a clean signal, not the same samples.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures::stream::BoxStream;
use futures::StreamExt;
use indexmap::IndexMap;
use serde_json::{json, Value};

use crate::config::VoiceServerConfig;
use crate::hermes::httpx_error_name;
use crate::numbers_fr::prepare_for_synthesis_with;
use crate::pronounce::Spelling;
use crate::pronunciations::Lexicon;
use crate::speech_intl;
use crate::textproc::{normalize_for_speech, strip_emoji};
use crate::voice_speech::pystr;

/// Priorities of the TTS queue: what a person waits for first, then the rest.
pub const INTERACTIVE: u8 = 0;
pub const BACKGROUND: u8 = 1;

/// client format -> (Kyutai response_format, content type)
pub const FORMATS: [(&str, &str, &str); 4] = [
    ("pcm16", "wav", "application/octet-stream"),
    ("wav", "wav", "audio/wav"),
    ("opus", "opus", "audio/ogg"),
    ("mp3", "mp3", "audio/mpeg"),
];

/// `FORMATS[fmt]`: (the engine's response_format, the content type).
pub fn format_info(fmt: &str) -> Option<(&'static str, &'static str)> {
    FORMATS.iter().find(|(f, _, _)| *f == fmt).map(|(_, r, c)| (*r, *c))
}

fn content_type_of(fmt: &str) -> &'static str {
    format_info(fmt).map(|(_, c)| c).unwrap_or("application/octet-stream")
}

pub const POCKET_PREFIX: &str = "pocket:";
pub const SERVER_PREFIX: &str = "server:";
/// A voice server's voice, as an earlier app named the Drama 3 ones (`drama:fr-mascotte-s17`).
pub const DRAMA_PREFIX: &str = "drama:";
/// What the app plays (and Kyutai, Pocket produce).
pub const OUTPUT_RATE: u32 = 24000;
pub const CHARS_PER_SECOND: f64 = 15.0;
/// After a missed deadline, how long the Bips speak instead of the voice server.
pub const SLOW_SERVER_SECONDS: f64 = 30.0;
pub const POCKET_ATTEMPTS: u32 = 2;
/// Seconds.
pub const POCKET_RETRY_DELAY: f64 = 0.5;

/// `TtsError(message, status=502)`: routes answer `status` with the message as detail.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct TtsError {
    pub message: String,
    pub status: u16,
}

impl TtsError {
    pub fn new(message: impl Into<String>, status: u16) -> Self {
        TtsError { message: message.into(), status }
    }

    /// `TtsError(message)`: an engine's failure (502).
    fn upstream(message: impl Into<String>) -> Self {
        TtsError::new(message, 502)
    }
}

/// What a voice-server wait ends with: an error, or its deadline missed (`asyncio.TimeoutError`).
enum ServerFailure {
    Timeout,
    Error(TtsError),
}

impl From<TtsError> for ServerFailure {
    fn from(e: TtsError) -> Self {
        ServerFailure::Error(e)
    }
}

// -- scheduler -----------------------------------------------------------------------------------

#[derive(Default)]
struct SchedulerState {
    busy: bool,
    counter: u64,
    /// (priority, arrival) of the waiters still in the heap; cancelled ones stay until popped (as Python's).
    heap: BinaryHeap<Reverse<(u8, u64)>>,
    senders: HashMap<u64, tokio::sync::oneshot::Sender<()>>,
    cancelled: std::collections::HashSet<u64>,
    /// Handed the slot, not yet taken by its task.
    handed: std::collections::HashSet<u64>,
}

impl SchedulerState {
    fn pending(&self) -> usize {
        self.heap.len() - self.cancelled.len()
    }

    /// `_release`: the slot handed to the next waiter still waiting (it stays busy), else free.
    fn release(&mut self) {
        while let Some(Reverse((_, id))) = self.heap.pop() {
            if self.cancelled.remove(&id) {
                continue;
            }
            if let Some(sender) = self.senders.remove(&id) {
                if sender.send(()).is_ok() {
                    self.handed.insert(id);
                    return; // hand over; stays busy
                }
            }
        }
        self.busy = false;
    }
}

/// One holder at a time; waiters served by (priority, arrival order).
#[derive(Default)]
pub struct FifoScheduler {
    state: Mutex<SchedulerState>,
}

/// The slot, held until dropped.
pub struct Slot {
    scheduler: Arc<FifoScheduler>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.scheduler.lock().release();
    }
}

/// A waiter's place in the queue: dropped while waiting (its task cancelled), it leaves the queue, or passes on the
/// slot it was handed just before.
struct Waiting {
    scheduler: Arc<FifoScheduler>,
    id: u64,
    armed: bool,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut state = self.scheduler.lock();
        if state.handed.remove(&self.id) {
            state.release(); // slot was handed to us just before cancellation
        } else if state.senders.remove(&self.id).is_some() {
            state.cancelled.insert(self.id);
        }
    }
}

impl FifoScheduler {
    fn lock(&self) -> std::sync::MutexGuard<'_, SchedulerState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Requests running or waiting.
    pub fn depth(&self) -> usize {
        let state = self.lock();
        state.pending() + usize::from(state.busy)
    }

    /// `async with scheduler.slot(priority)`: waits for its turn.
    pub async fn slot(self: &Arc<Self>, priority: u8) -> Slot {
        let receiver = {
            let mut state = self.lock();
            if !state.busy && state.pending() == 0 {
                state.busy = true;
                return Slot { scheduler: self.clone() };
            }
            let id = state.counter;
            state.counter += 1;
            let (sender, receiver) = tokio::sync::oneshot::channel();
            state.heap.push(Reverse((priority, id)));
            state.senders.insert(id, sender);
            (id, receiver)
        };
        let (id, receiver) = receiver;
        let mut waiting = Waiting { scheduler: self.clone(), id, armed: true };
        let _ = receiver.await;
        waiting.armed = false;
        self.lock().handed.remove(&id);
        Slot { scheduler: self.clone() }
    }
}

// -- audio formats -------------------------------------------------------------------------------

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    data.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Python's `min(1.0, x)` then `max(-1.0, …)` (a NaN gives 1.0, as there).
fn clamp_unit(x: f64) -> f64 {
    let low = if x < 1.0 { x } else { 1.0 };
    if low > -1.0 {
        low
    } else {
        -1.0
    }
}

/// Parse a RIFF/WAVE file (PCM 16-bit, PCM 24/32-bit or IEEE float 32) into little-endian
/// int16 PCM. Returns `(pcm, sample_rate, channels)`.
pub fn wav_to_pcm16(data: &[u8]) -> Result<(Vec<u8>, u32, u16), TtsError> {
    if data.len() < 12 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err(TtsError::upstream("Kyutai returned something that is not a WAV file"));
    }
    // A truncated chunk header is Python's struct.error: an uncaught exception there (a 500 for the route).
    let truncated = || TtsError::new("Internal Server Error", 500);
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pcm: &[u8] = &[];
    while pos + 8 <= data.len() {
        let cid = &data[pos..pos + 4];
        let size = u32_at(data, pos + 4).unwrap_or(0) as usize;
        let body_start = pos + 8;
        let body_end = if size != 0xFFFF_FFFF { data.len().min(body_start + size) } else { data.len() };
        if cid == b"fmt " {
            let mut tag = u16_at(data, body_start).ok_or_else(truncated)?;
            let channels = u16_at(data, body_start + 2).ok_or_else(truncated)?;
            let rate = u32_at(data, body_start + 4).ok_or_else(truncated)?;
            let bits = u16_at(data, body_start + 14).ok_or_else(truncated)?;
            if tag == 0xFFFE && size >= 26 {
                // WAVE_FORMAT_EXTENSIBLE: real tag in sub-format GUID
                tag = u16_at(data, body_start + 24).ok_or_else(truncated)?;
            }
            fmt = Some((tag, channels, rate, bits));
        } else if cid == b"data" {
            pcm = &data[body_start.min(data.len())..body_end.max(body_start.min(data.len()))];
            break;
        }
        pos = body_start + size + (size & 1);
    }
    let Some((tag, channels, rate, bits)) = fmt else {
        return Err(TtsError::upstream("WAV without fmt chunk"));
    };
    let out = if tag == 1 && bits == 16 {
        pcm[..pcm.len() - pcm.len() % 2].to_vec()
    } else if tag == 3 && bits == 32 {
        let mut out = Vec::with_capacity(pcm.len() / 2);
        for chunk in pcm.as_chunks::<4>().0 {
            let x = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64;
            let v = (clamp_unit(x) * 32767.0) as i16; // int() truncates toward zero
            out.extend(v.to_le_bytes());
        }
        out
    } else if tag == 1 && (bits == 24 || bits == 32) {
        let step = (bits / 8) as usize;
        let n = pcm.len() / step;
        let mut out = Vec::with_capacity(n * 2);
        for i in 0..n {
            out.extend(&pcm[i * step + step - 2..i * step + step]);
        }
        out
    } else {
        return Err(TtsError::upstream(format!("unsupported WAV encoding (tag={tag}, bits={bits})")));
    };
    Ok((out, rate, channels))
}

/// A WAV file around PCM16 data.
pub fn pcm16_to_wav(pcm: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend(b"RIFF");
    out.extend((36 + pcm.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes());
    out.extend(channels.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * channels as u32 * 2).to_le_bytes());
    out.extend((channels * 2).to_le_bytes());
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((pcm.len() as u32).to_le_bytes());
    out.extend(pcm);
    out
}

/// The first channel of interleaved PCM16.
pub fn mono(pcm: &[u8], channels: u16) -> Vec<u8> {
    let channels = channels.max(1) as usize;
    let usable = pcm.len() - pcm.len() % (2 * channels);
    pcm[..usable].as_chunks::<2>().0.iter().step_by(channels).flat_map(|s| [s[0], s[1]]).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub data: Bytes,
    pub content_type: String,
    pub format: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub cached: bool,
}

impl Audio {
    fn new(data: impl Into<Bytes>, content_type: &str, format: &str) -> Audio {
        Audio {
            data: data.into(),
            content_type: content_type.into(),
            format: format.into(),
            sample_rate: 24000,
            channels: 1,
            cached: false,
        }
    }
}

/// `pocket:en/colibri` -> `en`, `server:en/af_bella` -> `en`, `drama:en-naturelle-s47` -> `en`; French for
/// `pocket:colibri` and Kyutai voices.
pub fn voice_language(voice: &str) -> String {
    for prefix in [POCKET_PREFIX, SERVER_PREFIX] {
        if let Some(rest) = voice.strip_prefix(prefix) {
            if let Some((lang, _)) = rest.split_once('/') {
                if speech_intl::LANGUAGES.contains(&lang) {
                    return lang.to_string();
                }
            }
        }
    }
    if let Some(rest) = voice.strip_prefix(DRAMA_PREFIX) {
        let lang = rest.split('-').next().unwrap_or("");
        if speech_intl::LANGUAGES.contains(&lang) {
            return lang.to_string();
        }
    }
    "fr".to_string()
}

pub fn is_server_voice(voice: &str) -> bool {
    voice.starts_with(SERVER_PREFIX) || voice.starts_with(DRAMA_PREFIX)
}

/// The id the voice server knows: `server:fr/af_bella` -> `af_bella`, `drama:fr-mascotte-s17` as it is.
pub fn server_voice_id(voice: &str) -> String {
    if let Some(rest) = voice.strip_prefix(DRAMA_PREFIX) {
        return rest.to_string();
    }
    let rest = voice.strip_prefix(SERVER_PREFIX).unwrap_or(voice);
    match rest.split_once('/') {
        Some((lang, name)) if speech_intl::LANGUAGES.contains(&lang) => name.to_string(),
        _ => rest.to_string(),
    }
}

/// The Pocket voice standing in for a voice server's when it is down: a lively Bip for a mascot, a calm one
/// otherwise, in the voice's language.
pub fn server_fallback(voice: &str) -> String {
    let lang = voice_language(voice);
    let name = if voice.contains("mascotte") || voice.contains("mascot") { "colibri" } else { "galet" };
    if lang == "fr" {
        format!("{POCKET_PREFIX}{name}")
    } else {
        format!("{POCKET_PREFIX}{lang}/{name}")
    }
}

/// The voice ids in a server's answer: `{"voices": ["a", …]}` (audio.cpp, Kokoro), `{"voices": [{"id": …}]}`,
/// Kyutai's `{"aliases": {name: path}}`, or a bare list.
pub fn voice_names(data: &Value) -> Vec<String> {
    // data.get("voices", data.get("aliases", data.get("data"))): a present key wins even when null
    let mut data = match data {
        Value::Object(map) => {
            map.get("voices").or_else(|| map.get("aliases")).or_else(|| map.get("data")).cloned().unwrap_or(Value::Null)
        }
        other => other.clone(),
    };
    if let Value::Object(map) = &data {
        data = Value::Array(map.keys().map(|k| Value::String(k.clone())).collect());
    }
    let Value::Array(items) = data else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            Value::String(_) => Some(item.clone()),
            Value::Object(m) => {
                let pick = |k: &str| m.get(k).filter(|v| crate::py::truthy(v)).cloned();
                Some(pick("id").or_else(|| pick("name")).or_else(|| pick("voice_id")).unwrap_or(Value::Null))
            }
            _ => None,
        })
        .filter(crate::py::truthy)
        .map(|name| crate::py::py_str(&name))
        .collect()
}

// -- resampling ----------------------------------------------------------------------------------

/// Zero crossings of the filter on each side, at the lower of the two rates.
const SINC_ZEROS: f64 = 24.0;
/// Table points per input sample.
const TABLE_STEPS: usize = 128;
const KAISER_BETA: f64 = 8.6;

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let half = x / 2.0;
    for k in 1..50 {
        term *= half / k as f64;
        let t2 = term * term;
        sum += t2;
        if t2 < sum * 1e-12 {
            break;
        }
    }
    sum
}

/// PCM16 mono from one rate to another, chunk by chunk, keeping its filter state across chunks (no click at the
/// joins): a Kaiser-windowed sinc low-pass (Python: soxr's HQ). The output is aligned with the input (no delay) and,
/// once `last` is fed, holds ceil(n × target / source) samples.
pub struct Resampler {
    pub source: u32,
    pub target: u32,
    carry: Vec<u8>,
    /// Input samples from `first` on (older ones are no longer needed).
    input: Vec<f32>,
    first: usize,
    received: usize,
    produced: usize,
    half_width: f64,
    cutoff: f64,
    table: Vec<f32>,
}

impl Resampler {
    pub fn new(source: u32, target: u32) -> Self {
        let source = source.max(1);
        let target = target.max(1);
        let ratio = target as f64 / source as f64;
        // the cut-off (cycles per input sample) a little below the lower Nyquist frequency
        let cutoff = 0.5 * ratio.min(1.0) * 0.96;
        let half_width = SINC_ZEROS / (2.0 * cutoff);
        let points = (half_width * TABLE_STEPS as f64).ceil() as usize + 2;
        let i0_beta = bessel_i0(KAISER_BETA);
        let table = (0..points)
            .map(|i| {
                let x = i as f64 / TABLE_STEPS as f64;
                if x >= half_width {
                    return 0.0;
                }
                let arg = 2.0 * cutoff * x;
                let sinc =
                    if arg == 0.0 { 1.0 } else { (std::f64::consts::PI * arg).sin() / (std::f64::consts::PI * arg) };
                let r = x / half_width;
                let window = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / i0_beta;
                (2.0 * cutoff * sinc * window) as f32
            })
            .collect();
        Resampler {
            source,
            target,
            carry: Vec::new(),
            input: Vec::new(),
            first: 0,
            received: 0,
            produced: 0,
            half_width,
            cutoff,
            table,
        }
    }

    fn kernel(&self, x: f64) -> f32 {
        let x = x.abs();
        if x >= self.half_width {
            return 0.0;
        }
        let pos = x * TABLE_STEPS as f64;
        let i = pos as usize;
        let frac = (pos - i as f64) as f32;
        let a = self.table[i];
        let b = self.table.get(i + 1).copied().unwrap_or(0.0);
        a + (b - a) * frac
    }

    fn sample(&self, index: isize) -> f32 {
        if index < self.first as isize {
            return 0.0;
        }
        self.input.get(index as usize - self.first).copied().unwrap_or(0.0)
    }

    pub fn feed(&mut self, data: &[u8], last: bool) -> Vec<u8> {
        let _ = self.cutoff;
        let mut bytes = std::mem::take(&mut self.carry);
        bytes.extend_from_slice(data);
        let whole = bytes.len() - bytes.len() % 2;
        self.carry = bytes[whole..].to_vec();
        for s in bytes[..whole].as_chunks::<2>().0 {
            self.input.push(i16::from_le_bytes([s[0], s[1]]) as f32);
        }
        self.received += whole / 2;
        let step = self.source as f64 / self.target as f64;
        let total = if last {
            ((self.received as u128 * self.target as u128).div_ceil(self.source as u128)) as usize
        } else {
            usize::MAX
        };
        let mut out = Vec::new();
        loop {
            if self.produced >= total {
                break;
            }
            let t = self.produced as f64 * step;
            let reach = (t + self.half_width).ceil() as usize;
            if !last && reach >= self.received {
                break; // the filter needs samples still to come
            }
            let lo = (t - self.half_width).floor() as isize;
            let hi = (t + self.half_width).ceil() as isize;
            let mut acc = 0f32;
            for i in lo.max(0)..=hi {
                acc += self.sample(i) * self.kernel(t - i as f64);
            }
            let v = acc.round().clamp(-32768.0, 32767.0) as i16;
            out.extend(v.to_le_bytes());
            self.produced += 1;
        }
        // drop what no later output needs
        let next_t = self.produced as f64 * step;
        let keep_from = ((next_t - self.half_width).floor().max(0.0) as usize).min(self.received);
        if keep_from > self.first {
            self.input.drain(..keep_from - self.first);
            self.first = keep_from;
        }
        out
    }
}

// -- the service ---------------------------------------------------------------------------------

/// A TTS server: Kyutai, Pocket, or the person's own voice server.
pub struct Engine {
    pub name: String,
    pub url: String,
    pub scheduler: Arc<FifoScheduler>,
    pub health_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Which {
    Kyutai,
    Pocket,
    Server,
}

/// `TtsService(...)`'s keyword arguments.
#[derive(Debug, Clone)]
pub struct TtsOptions {
    pub cache_entries: usize,
    pub timeout: f64,
    pub max_chars: usize,
    pub sentence_max_chars: usize,
    pub pocket_url: Option<String>,
    /// The server's Pocket version (« 4 », « 3.3.0 »): see `Spelling::for_engine`.
    pub pocket_engine: String,
    pub voice_server: Option<VoiceServerConfig>,
    pub words: IndexMap<String, IndexMap<String, String>>,
}

impl Default for TtsOptions {
    fn default() -> Self {
        TtsOptions {
            cache_entries: 256,
            timeout: 120.0,
            max_chars: 1000,
            sentence_max_chars: 180,
            pocket_url: None,
            pocket_engine: "4".into(),
            voice_server: None,
            words: IndexMap::new(),
        }
    }
}

/// `synthesize`'s optional arguments.
#[derive(Debug, Clone, Default)]
pub struct SynthOptions {
    pub normalized: bool,
    pub max_chars: Option<usize>,
    /// The Bip speaking instead of a voice server's voice when the server fails or is too slow.
    pub relay: Option<String>,
}

type CacheKey = (String, String, String);

/// Kyutai TTS 1.6B (GPU) plus, optionally, Kyutai Pocket TTS (CPU) for the Bips' voices.
///
/// A voice named `pocket:<name>` (e.g. `pocket:colibri`) goes to Pocket in French, `pocket:<lang>/<name>`
/// (`en`, `es`, `de`) to Pocket's model for that language; `server:<lang>/<id>` (or an earlier app's
/// `drama:<id>`) to the person's voice server; any other voice to Kyutai. Each engine has its own FIFO queue (they
/// run on different hardware, and each generates one thing at a time). When Pocket is not configured, down or
/// failing before its first audio, a French request falls back to Kyutai's default voice (that audio is not
/// cached); another language fails instead, so the app reads it with the iPhone's voice in the right language.
/// When the voice server is not configured or fails before its first audio, a Pocket Bip of the same language stands
/// in.
pub struct TtsService {
    pub client: reqwest::Client,
    pub base_url: String,
    pub default_voice: String,
    pub options: TtsOptions,
    pub scheduler: Arc<FifoScheduler>,
    /// None: Pocket only (no GPU); then the default voice is a Pocket one and nothing falls back to Kyutai.
    pub kyutai: Option<Engine>,
    pub pocket: Option<Engine>,
    pub server: Option<Engine>,
    pub words: Lexicon,
    /// Engine requests actually made (diagnostics / tests).
    pub calls: AtomicUsize,
    server_voices: Mutex<Option<(Instant, Vec<String>)>>,
    slow_until: Mutex<Option<Instant>>,
    cache: Mutex<IndexMap<CacheKey, Audio>>,
}

impl TtsService {
    pub fn new(client: reqwest::Client, base_url: Option<&str>, default_voice: &str, options: TtsOptions) -> Self {
        let base_url = base_url.unwrap_or("").trim_end_matches('/').to_string();
        let scheduler = Arc::new(FifoScheduler::default());
        let kyutai = (!base_url.is_empty()).then(|| Engine {
            name: "kyutai".into(),
            url: base_url.clone(),
            scheduler: scheduler.clone(),
            health_path: "/health".into(),
        });
        let pocket = options.pocket_url.as_ref().filter(|u| !u.is_empty()).map(|url| Engine {
            name: "pocket".into(),
            url: url.trim_end_matches('/').to_string(),
            scheduler: Arc::new(FifoScheduler::default()),
            health_path: "/health".into(),
        });
        let server = options.voice_server.as_ref().map(|vs| Engine {
            name: "server".into(),
            url: vs.url.clone(),
            scheduler: Arc::new(FifoScheduler::default()),
            health_path: "/v1/models".into(),
        });
        let words = Lexicon::new(&options.words);
        TtsService {
            client,
            base_url,
            default_voice: default_voice.to_string(),
            options,
            scheduler,
            kyutai,
            pocket,
            server,
            words,
            calls: AtomicUsize::new(0),
            server_voices: Mutex::new(None),
            slow_until: Mutex::new(None),
            cache: Mutex::new(IndexMap::new()),
        }
    }

    fn voice_server(&self) -> Option<&VoiceServerConfig> {
        self.options.voice_server.as_ref()
    }

    fn engine(&self, which: Which) -> &Engine {
        match which {
            Which::Kyutai => self.kyutai.as_ref(),
            Which::Pocket => self.pocket.as_ref(),
            Which::Server => self.server.as_ref(),
        }
        .expect("routed to a configured engine")
    }

    fn route(&self, voice: &str) -> Result<(Which, String), TtsError> {
        if is_server_voice(voice) {
            if self.server.is_some() {
                return Ok((Which::Server, server_voice_id(voice)));
            }
            return self.route(&server_fallback(voice));
        }
        if let Some(name) = voice.strip_prefix(POCKET_PREFIX) {
            if self.pocket.is_some() {
                return Ok((Which::Pocket, name.to_string()));
            }
            tracing::warn!(voice = %voice, "pocket voice requested but pocket is not configured");
            if voice_language(voice) != "fr" || self.kyutai.is_none() {
                return Err(TtsError::new("pocket is not configured", 503));
            }
            return Ok((Which::Kyutai, self.default_voice.clone()));
        }
        if self.kyutai.is_none() {
            if !self.default_voice.starts_with(POCKET_PREFIX) {
                return Err(TtsError::new("kyutai is not configured", 503));
            }
            return self.route(&self.default_voice.clone()); // a Kyutai voice asked for, no Kyutai here: the default Bip
        }
        Ok((Which::Kyutai, voice.to_string()))
    }

    /// Pocket phrases the text itself (a pause per sentence, longer between lines): keep the line breaks.
    fn paragraphs(&self, voice: &str) -> bool {
        voice.starts_with(POCKET_PREFIX) && self.pocket.is_some()
    }

    /// Respelled names only for this server's Pocket when it is the old engine (< 4); every other engine reads the
    /// raw text.
    fn spelling(&self, voice: &str) -> Spelling {
        if voice.starts_with(POCKET_PREFIX) && self.pocket.is_some() {
            Spelling::for_engine(Some(&self.options.pocket_engine))
        } else {
            Spelling::Raw
        }
    }

    fn cache_get(&self, key: &CacheKey) -> Option<Audio> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let audio = cache.shift_remove(key)?;
        cache.insert(key.clone(), audio.clone());
        Some(audio)
    }

    fn cache_put(&self, key: CacheKey, audio: Audio) {
        if self.options.cache_entries == 0 {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.shift_remove(&key);
        cache.insert(key, audio);
        while cache.len() > self.options.cache_entries {
            cache.shift_remove_index(0);
        }
    }

    fn prepare(
        &self,
        text: &str,
        normalized: bool,
        max_chars: Option<usize>,
        language: &str,
        paragraphs: bool,
        spelling: Spelling,
    ) -> Result<String, TtsError> {
        let text = strip_emoji(text); // never read out, and Pocket stumbles on them
        let clean = if normalized {
            pystr::strip(&text).to_string()
        } else {
            normalize_for_speech(&text, self.options.sentence_max_chars, paragraphs)
        };
        let clean = self.words.apply(&clean, language); // « Garmin » as it is said, before numbers and CamelCase
                                                        // names respelled, numbers in words, parentheses as pauses
        let clean = if language == "fr" {
            prepare_for_synthesis_with(&clean, spelling)
        } else {
            speech_intl::prepare_for_synthesis_with(&clean, language, spelling)
        };
        if clean.is_empty() {
            return Err(TtsError::new("empty text after normalization", 400));
        }
        let limit = max_chars.unwrap_or(self.options.max_chars);
        let length = pystr::len(&clean);
        if limit != 0 && length > limit {
            return Err(TtsError::new(format!("text too long ({length} > {limit} chars)"), 413));
        }
        Ok(clean)
    }

    /// A Bip's text as the phone speaks it itself (its own Pocket, the Bips' voices on the phone): (the text
    /// prepared as for this server's Pocket, its language code, the Bip). None for a voice that isn't a Bip's.
    pub fn for_phone(
        &self,
        text: &str,
        voice: Option<&str>,
        max_chars: Option<usize>,
        engine: Option<&str>,
    ) -> Result<Option<(String, String, String)>, TtsError> {
        let voice = voice.filter(|v| !v.is_empty()).unwrap_or(&self.default_voice);
        let Some(rest) = voice.strip_prefix(POCKET_PREFIX) else {
            return Ok(None);
        };
        let language = voice_language(voice);
        let bip = rest.rsplit('/').next().unwrap_or(rest).to_string();
        let spelling = Spelling::for_engine(engine); // the phone's own Pocket package
        Ok(Some((self.prepare(text, false, max_chars, &language, true, spelling)?, language, bip)))
    }

    /// Seconds an interactive request waits for the voice server's first sound once it is sent (not its turn in
    /// the queue: the earlier sentences of a Live are not the server's slowness); background ones (the outbox's
    /// reading) wait as long as it takes.
    fn server_deadline(&self, text: &str, priority: u8) -> Option<f64> {
        let server = self.voice_server()?;
        if priority != INTERACTIVE {
            return None;
        }
        let streaming = server.stream_rate.map(|r| r != 0).unwrap_or(false);
        Some(server.first_audio_seconds + if streaming { 0.0 } else { pystr::len(text) as f64 / CHARS_PER_SECOND })
    }

    /// It just missed a deadline: for a while, interactive requests go to the Bips at once (one voice throughout a
    /// Live, not one sentence each).
    fn server_slow(&self) -> bool {
        self.slow_until.lock().unwrap_or_else(|e| e.into_inner()).map(|until| Instant::now() < until).unwrap_or(false)
    }

    fn mark_server_slow(&self, chars: usize) {
        *self.slow_until.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Instant::now() + Duration::from_secs_f64(SLOW_SERVER_SECONDS));
        tracing::warn!(chars = chars, "voice server too slow, the Bips speak for a while");
    }

    /// Python's tests set `_slow_until = 0.0`: the voice server is trusted again.
    pub fn clear_slow(&self) {
        *self.slow_until.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// `relay`: the Bip speaking instead of a voice server's voice when the server fails or is too slow (else a
    /// Bip of the same language).
    pub async fn synthesize(
        &self,
        text: &str,
        voice: Option<&str>,
        fmt: &str,
        priority: u8,
        options: SynthOptions,
    ) -> Result<Audio, TtsError> {
        if format_info(fmt).is_none() {
            return Err(TtsError::new(format!("unsupported format: {fmt}"), 400));
        }
        let voice = voice.filter(|v| !v.is_empty()).unwrap_or(&self.default_voice).to_string();
        let language = voice_language(&voice);
        let clean = self.prepare(
            text,
            options.normalized,
            options.max_chars,
            &language,
            self.paragraphs(&voice),
            self.spelling(&voice),
        )?;
        let key: CacheKey = (voice.clone(), fmt.to_string(), clean.clone());
        if let Some(hit) = self.cache_get(&key) {
            return Ok(Audio { cached: true, ..hit });
        }
        let (which, engine_voice) = self.route(&voice)?;
        let again = |standin: String| {
            let options = SynthOptions { normalized: options.normalized, max_chars: options.max_chars, relay: None };
            let text = text.to_string();
            let fmt = fmt.to_string();
            async move { Box::pin(self.synthesize(&text, Some(&standin), &fmt, priority, options)).await }
        };
        if which == Which::Server {
            let standin = options.relay.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| server_fallback(&voice));
            let deadline = self.server_deadline(&clean, priority);
            if deadline.is_some() && self.server_slow() {
                return again(standin).await;
            }
            let result: Result<Audio, ServerFailure> = async {
                let _slot = self.engine(Which::Server).scheduler.slot(priority).await;
                if deadline.is_some() && self.server_slow() {
                    // missed by an earlier one while this waited
                    return Err(ServerFailure::Timeout);
                }
                with_deadline(deadline, self.call_server(&clean, &engine_voice, fmt)).await
            }
            .await;
            return match result {
                Ok(audio) => {
                    self.cache_put(key, audio.clone());
                    Ok(audio)
                }
                Err(ServerFailure::Timeout) => {
                    if !self.server_slow() {
                        self.mark_server_slow(pystr::len(&clean));
                    }
                    again(standin).await
                }
                Err(ServerFailure::Error(exc)) => {
                    if exc.status < 500 && (fmt == "pcm16" || fmt == "wav") {
                        return Err(exc);
                    }
                    tracing::warn!(error = %exc, format = %fmt, "voice server failed, using a pocket voice");
                    again(standin).await
                }
            };
        }
        let attempts = if which == Which::Pocket { POCKET_ATTEMPTS } else { 1 };
        let mut audio = None;
        for attempt in 1..=attempts {
            let result = async {
                let _slot = self.engine(which).scheduler.slot(priority).await;
                if let Some(hit) = self.cache_get(&key) {
                    // an identical request may have just finished
                    return Ok(Err(Audio { cached: true, ..hit }));
                }
                self.call_engine(which, &clean, &engine_voice, fmt).await.map(Ok)
            }
            .await;
            match result {
                Ok(Err(hit)) => return Ok(hit),
                Ok(Ok(made)) => {
                    audio = Some(made);
                    break;
                }
                Err(exc) => {
                    if which == Which::Pocket && language != "fr" && exc.status < 500 {
                        // e.g. this language's voices are not installed: 503 so the app reads it with the iPhone's voice
                        return Err(TtsError::new(format!("pocket {language} voice unavailable: {exc}"), 503));
                    }
                    if which == Which::Kyutai || exc.status < 500 {
                        return Err(exc);
                    }
                    if attempt < attempts {
                        tracing::info!(error = %exc, "pocket failed, retrying");
                        tokio::time::sleep(Duration::from_secs_f64(POCKET_RETRY_DELAY)).await;
                        continue;
                    }
                    if language != "fr" || self.kyutai.is_none() {
                        return Err(exc);
                    }
                    tracing::warn!(error = %exc, text = %pystr::head(&clean, 60), "pocket failed, falling back to kyutai");
                    let _slot = self.engine(Which::Kyutai).scheduler.slot(priority).await;
                    return self.call_engine(Which::Kyutai, &clean, &self.default_voice, fmt).await;
                }
            }
        }
        let audio = audio.expect("an attempt made the audio");
        self.cache_put(key, audio.clone());
        Ok(audio)
    }

    /// PCM16 mono 24 kHz chunks as the engine produces them (`POST /v1/audio/stream`): the first
    /// audio arrives after ~0.75 s with Kyutai, ~0.1 s with Pocket, instead of after the whole text.
    /// Validation errors and an engine failure before the first chunk are the stream's first item (Python raises
    /// `TtsError`); an engine without the streaming route (404/405) falls back to one whole-file synthesis. Only a
    /// complete stream is cached.
    pub fn stream(
        self: &Arc<Self>,
        text: &str,
        voice: Option<&str>,
        priority: u8,
        max_chars: Option<usize>,
        relay: Option<&str>,
    ) -> BoxStream<'static, Result<Bytes, TtsError>> {
        let this = self.clone();
        let text = text.to_string();
        let voice = voice.filter(|v| !v.is_empty()).unwrap_or(&self.default_voice).to_string();
        let relay = relay.filter(|r| !r.is_empty()).map(str::to_string);
        async_stream::stream! {
            let language = voice_language(&voice);
            let clean = match this.prepare(&text, false, max_chars, &language, this.paragraphs(&voice), this.spelling(&voice)) {
                Ok(clean) => clean,
                Err(e) => { yield Err(e); return; }
            };
            let key: CacheKey = (voice.clone(), "pcm16".into(), clean.clone());
            if let Some(hit) = this.cache_get(&key) {
                yield Ok(hit.data);
                return;
            }
            let (which, engine_voice) = match this.route(&voice) {
                Ok(r) => r,
                Err(e) => { yield Err(e); return; }
            };
            let mut received: Vec<u8> = Vec::new();
            if which == Which::Server {
                let standin = relay.clone().unwrap_or_else(|| server_fallback(&voice));
                let deadline = this.server_deadline(&clean, priority);
                let mut failure: Option<ServerFailure> = None;
                if deadline.is_some() && this.server_slow() {
                    failure = Some(ServerFailure::Timeout);
                } else {
                    let mut pieces = this.stream_server(clean.clone(), engine_voice.clone(), priority, deadline);
                    while let Some(item) = pieces.next().await {
                        match item {
                            Ok(chunk) => {
                                received.extend_from_slice(&chunk);
                                yield Ok(chunk);
                            }
                            Err(f) => { failure = Some(f); break; }
                        }
                    }
                }
                match failure {
                    Some(ServerFailure::Timeout) => {
                        if !received.is_empty() {
                            return;
                        }
                        if !this.server_slow() {
                            this.mark_server_slow(pystr::len(&clean));
                        }
                        let mut other = this.stream(&text, Some(&standin), priority, max_chars, None);
                        while let Some(item) = other.next().await {
                            yield item;
                        }
                        return;
                    }
                    Some(ServerFailure::Error(exc)) => {
                        if !received.is_empty() {
                            return; // cut mid-stream: the client keeps what it already played, nothing is cached
                        }
                        if exc.status < 500 {
                            yield Err(exc);
                            return;
                        }
                        tracing::warn!(error = %exc, "voice server stream failed, using a pocket voice");
                        let mut other = this.stream(&text, Some(&standin), priority, max_chars, None);
                        while let Some(item) = other.next().await {
                            yield item;
                        }
                        return;
                    }
                    None => {}
                }
                if !received.is_empty() {
                    this.cache_put(key, Audio::new(received, content_type_of("pcm16"), "pcm16"));
                }
                return;
            }
            let attempts = if which == Which::Pocket { POCKET_ATTEMPTS } else { 1 };
            for attempt in 1..=attempts {
                let mut failed: Option<TtsError> = None;
                {
                    let mut chunks = this.stream_engine(which, clean.clone(), engine_voice.clone(), priority);
                    while let Some(item) = chunks.next().await {
                        match item {
                            Ok(chunk) => {
                                received.extend_from_slice(&chunk);
                                yield Ok(chunk);
                            }
                            Err(e) => { failed = Some(e); break; }
                        }
                    }
                }
                let Some(exc) = failed else { break };
                if !received.is_empty() {
                    return; // cut mid-stream: the client keeps what it already played, nothing is cached
                }
                if which == Which::Pocket && language != "fr" && exc.status < 500 {
                    yield Err(TtsError::new(format!("pocket {language} voice unavailable: {exc}"), 503));
                    return;
                }
                if which == Which::Kyutai || exc.status < 500 {
                    yield Err(exc);
                    return;
                }
                if attempt < attempts {
                    // Usually busy finishing a cancelled request (one generation at a time): try once more.
                    tracing::info!(error = %exc, "pocket stream failed, retrying");
                    tokio::time::sleep(Duration::from_secs_f64(POCKET_RETRY_DELAY)).await;
                    continue;
                }
                if language != "fr" || this.kyutai.is_none() {
                    yield Err(exc);
                    return;
                }
                tracing::warn!(error = %exc, text = %pystr::head(&clean, 60), "pocket stream failed, falling back to kyutai");
                let mut chunks = this.stream_engine(Which::Kyutai, clean.clone(), this.default_voice.clone(), priority);
                while let Some(item) = chunks.next().await {
                    let stop = item.is_err();
                    yield item;
                    if stop {
                        return;
                    }
                }
                return;
            }
            if !received.is_empty() {
                this.cache_put(key, Audio::new(received, content_type_of("pcm16"), "pcm16"));
            }
        }
        .boxed()
    }

    fn stream_engine(
        self: &Arc<Self>,
        which: Which,
        text: String,
        voice: String,
        priority: u8,
    ) -> BoxStream<'static, Result<Bytes, TtsError>> {
        let this = self.clone();
        async_stream::stream! {
            let engine = this.engine(which);
            let _slot = engine.scheduler.slot(priority).await;
            this.calls.fetch_add(1, Ordering::SeqCst);
            let body = json!({"input": text, "voice": voice, "format": "pcm16"});
            let timeout = Duration::from_secs_f64(this.options.timeout);
            let mut sent = 0usize;
            let request = this.client.post(format!("{}/v1/audio/stream", engine.url)).json(&body).send();
            let resp = match tokio::time::timeout(timeout, request).await {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    tracing::warn!(engine = %engine.name, error = httpx_error_name(&e), sent = sent, "tts stream failed");
                    yield Err(TtsError::upstream(format!("{} unreachable", engine.name)));
                    return;
                }
                Err(_) => {
                    tracing::warn!(engine = %engine.name, error = "ReadTimeout", sent = sent, "tts stream failed");
                    yield Err(TtsError::upstream(format!("{} unreachable", engine.name)));
                    return;
                }
            };
            let status = resp.status().as_u16();
            let whole_file = status == 404 || status == 405;
            if !whole_file && status != 200 {
                tracing::warn!(engine = %engine.name, status = status, "tts stream error");
                yield Err(TtsError::upstream(format!("{} returned HTTP {status}", engine.name)));
                return;
            }
            if !whole_file {
                let mut body = resp.bytes_stream();
                loop {
                    match tokio::time::timeout(timeout, body.next()).await {
                        Ok(Some(Ok(chunk))) => {
                            if !chunk.is_empty() {
                                sent += chunk.len();
                                yield Ok(chunk);
                            }
                        }
                        Ok(None) => break,
                        Ok(Some(Err(e))) => {
                            tracing::warn!(engine = %engine.name, error = httpx_error_name(&e), sent = sent, "tts stream failed");
                            let message = if sent == 0 { format!("{} unreachable", engine.name) } else { format!("{} stream cut", engine.name) };
                            yield Err(TtsError::upstream(message));
                            return;
                        }
                        Err(_) => {
                            tracing::warn!(engine = %engine.name, error = "ReadTimeout", sent = sent, "tts stream failed");
                            let message = if sent == 0 { format!("{} unreachable", engine.name) } else { format!("{} stream cut", engine.name) };
                            yield Err(TtsError::upstream(message));
                            return;
                        }
                    }
                }
                return;
            }
            drop(resp);
            this.calls.fetch_sub(1, Ordering::SeqCst); // counted again by the whole-file call
            match this.call_engine(which, &text, &voice, "pcm16").await {
                Ok(audio) => yield Ok(audio.data),
                Err(e) => yield Err(e),
            }
        }
        .boxed()
    }

    fn server_body(&self, text: &str, voice: &str, streaming: bool) -> Value {
        let model = self.voice_server().map(|v| v.model.clone()).unwrap_or_default();
        let mut body = json!({"model": model, "input": text, "voice": voice, "response_format": if streaming { "pcm" } else { "wav" }});
        if streaming {
            body["stream_format"] = json!("audio");
        }
        body
    }

    /// Most 24 kHz bytes a take should reach: some models (Drama 3) read sentences again after the last one, so a
    /// take running past twice its expected length (15 characters a second) is cut.
    fn take_limit(text: &str) -> usize {
        let expected = pystr::len(text) as f64 / CHARS_PER_SECOND;
        ((2.0 * expected).max(expected + 4.0) * OUTPUT_RATE as f64 * 2.0) as usize
    }

    fn server_error(status: u16) -> TtsError {
        TtsError::new(
            format!("voice server returned HTTP {status}"),
            if [400, 404, 422].contains(&status) { 400 } else { 502 },
        )
    }

    /// The voice server's raw PCM as it comes, resampled to 24 kHz; a server that does not stream gives its whole
    /// WAV file at once. Closing the stream (the client gone, a take too long, no first sound within `deadline`
    /// seconds once sent: `Timeout`) stops the generation on the server.
    fn stream_server(
        self: &Arc<Self>,
        text: String,
        voice: String,
        priority: u8,
        deadline: Option<f64>,
    ) -> BoxStream<'static, Result<Bytes, ServerFailure>> {
        let this = self.clone();
        async_stream::stream! {
            let rate = this.voice_server().and_then(|v| v.stream_rate).unwrap_or(0);
            let engine = this.engine(Which::Server);
            if rate == 0 {
                let audio = {
                    let _slot = engine.scheduler.slot(priority).await;
                    if deadline.is_some() && this.server_slow() {
                        yield Err(ServerFailure::Timeout);
                        return;
                    }
                    with_deadline(deadline, this.call_server(&text, &voice, "pcm16")).await
                };
                match audio {
                    Ok(audio) => yield Ok(audio.data),
                    Err(e) => yield Err(e),
                }
                return;
            }
            let _slot = engine.scheduler.slot(priority).await;
            if deadline.is_some() && this.server_slow() {
                yield Err(ServerFailure::Timeout);
                return;
            }
            this.calls.fetch_add(1, Ordering::SeqCst);
            let mut resampler = Resampler::new(rate, OUTPUT_RATE);
            let limit = Self::take_limit(&text);
            let timeout = Duration::from_secs_f64(this.options.timeout);
            let mut sent = 0usize;
            let request = this.client.post(format!("{}/v1/audio/speech", engine.url)).json(&this.server_body(&text, &voice, true)).send();
            let resp = match tokio::time::timeout(timeout, request).await {
                Ok(Ok(resp)) => resp,
                Ok(Err(e)) => {
                    tracing::warn!(engine = "server", error = httpx_error_name(&e), sent = sent, "tts stream failed");
                    yield Err(ServerFailure::Error(TtsError::upstream("voice server unreachable")));
                    return;
                }
                Err(_) => {
                    tracing::warn!(engine = "server", error = "ReadTimeout", sent = sent, "tts stream failed");
                    yield Err(ServerFailure::Error(TtsError::upstream("voice server unreachable")));
                    return;
                }
            };
            let status = resp.status().as_u16();
            if status != 200 {
                tracing::warn!(engine = "server", status = status, "tts stream error");
                yield Err(ServerFailure::Error(Self::server_error(status)));
                return;
            }
            let mut pieces = resp.bytes_stream();
            let mut waiting = deadline; // for the first piece only
            loop {
                let wait = waiting.map(Duration::from_secs_f64).unwrap_or(timeout);
                let next = match tokio::time::timeout(wait, pieces.next()).await {
                    Ok(next) => next,
                    Err(_) if waiting.is_some() => {
                        yield Err(ServerFailure::Timeout);
                        return;
                    }
                    Err(_) => {
                        tracing::warn!(engine = "server", error = "ReadTimeout", sent = sent, "tts stream failed");
                        let message = if sent == 0 { "voice server unreachable" } else { "voice server stream cut" };
                        yield Err(ServerFailure::Error(TtsError::upstream(message)));
                        return;
                    }
                };
                let chunk = match next {
                    None => break,
                    Some(Ok(chunk)) => chunk,
                    Some(Err(e)) => {
                        tracing::warn!(engine = "server", error = httpx_error_name(&e), sent = sent, "tts stream failed");
                        let message = if sent == 0 { "voice server unreachable" } else { "voice server stream cut" };
                        yield Err(ServerFailure::Error(TtsError::upstream(message)));
                        return;
                    }
                };
                waiting = None;
                let out = resampler.feed(&chunk, false);
                if !out.is_empty() {
                    sent += out.len();
                    yield Ok(Bytes::from(out));
                }
                if sent > limit {
                    tracing::warn!(chars = pystr::len(&text), seconds = sent as f64 / 2.0 / OUTPUT_RATE as f64, "voice server take too long, cut");
                    return;
                }
            }
            let tail = resampler.feed(&[], true);
            if !tail.is_empty() {
                yield Ok(Bytes::from(tail));
            }
        }
        .boxed()
    }

    /// WAV from the server, as PCM16 at 24 kHz; mp3 or opus asked as such (a server without them fails and a Bip
    /// stands in: no encoder here).
    async fn call_server(&self, text: &str, voice: &str, fmt: &str) -> Result<Audio, TtsError> {
        let engine = self.engine(Which::Server);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut body = self.server_body(text, voice, false);
        if fmt == "mp3" || fmt == "opus" {
            body["response_format"] = json!(fmt);
        }
        let sent = self
            .client
            .post(format!("{}/v1/audio/speech", engine.url))
            .json(&body)
            .timeout(Duration::from_secs_f64(self.options.timeout))
            .send()
            .await;
        let resp = match sent {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(engine = "server", error = httpx_error_name(&e), "tts request failed");
                return Err(TtsError::upstream("voice server unreachable"));
            }
        };
        let status = resp.status().as_u16();
        if status != 200 {
            tracing::warn!(engine = "server", status = status, "tts error");
            return Err(Self::server_error(status));
        }
        let content = match resp.bytes().await {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(engine = "server", error = httpx_error_name(&e), "tts request failed");
                return Err(TtsError::upstream("voice server unreachable"));
            }
        };
        if fmt == "mp3" || fmt == "opus" {
            return Ok(Audio::new(content, content_type_of(fmt), fmt));
        }
        let (mut pcm, rate, channels) = wav_to_pcm16(&content)?;
        if channels != 1 {
            pcm = mono(&pcm, channels);
        }
        if rate != OUTPUT_RATE {
            pcm = Resampler::new(rate, OUTPUT_RATE).feed(&pcm, true);
        }
        pcm.truncate(Self::take_limit(text));
        if fmt == "wav" {
            return Ok(Audio {
                sample_rate: OUTPUT_RATE,
                channels: 1,
                ..Audio::new(pcm16_to_wav(&pcm, OUTPUT_RATE, 1), content_type_of("wav"), "wav")
            });
        }
        Ok(Audio { sample_rate: OUTPUT_RATE, channels: 1, ..Audio::new(pcm, content_type_of("pcm16"), "pcm16") })
    }

    /// The voice server's voices, in its order: `GET /v1/audio/voices` (audio.cpp, Kokoro), Kyutai's
    /// `GET /v1/voices`, else the ones its config lists. Kept a few minutes.
    pub async fn server_voices(&self, max_age: f64) -> Vec<String> {
        let Some(server) = self.voice_server() else {
            return Vec::new();
        };
        let now = Instant::now();
        if let Some((at, voices)) = self.server_voices.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if now.duration_since(*at).as_secs_f64() < max_age {
                return voices.clone();
            }
        }
        let mut found = Vec::new();
        for path in ["/v1/audio/voices", "/v1/voices"] {
            let resp =
                match self.client.get(format!("{}{path}", server.url)).timeout(Duration::from_secs(5)).send().await {
                    Ok(resp) => resp,
                    Err(_) => continue,
                };
            let data = if resp.status().as_u16() == 200 {
                match resp.bytes().await {
                    Ok(body) => match serde_json::from_slice::<Value>(&body) {
                        Ok(v) => v,
                        Err(_) => continue, // ValueError
                    },
                    Err(_) => continue,
                }
            } else {
                Value::Null
            };
            found = voice_names(&data);
            if !found.is_empty() {
                break;
            }
        }
        if found.is_empty() {
            found = server.voices.clone();
        }
        if !found.is_empty() {
            *self.server_voices.lock().unwrap_or_else(|e| e.into_inner()) = Some((now, found.clone()));
        }
        found
    }

    async fn call_engine(&self, which: Which, text: &str, voice: &str, fmt: &str) -> Result<Audio, TtsError> {
        let engine = self.engine(which);
        let (engine_fmt, content_type) = format_info(fmt).unwrap_or(("wav", "application/octet-stream"));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let body = json!({"model": format!("{}-tts", engine.name), "input": text, "voice": voice, "response_format": engine_fmt});
        let sent = self
            .client
            .post(format!("{}/v1/audio/speech", engine.url))
            .json(&body)
            .timeout(Duration::from_secs_f64(self.options.timeout))
            .send()
            .await;
        let resp = match sent {
            Ok(resp) => resp,
            Err(e) => {
                tracing::warn!(engine = %engine.name, error = httpx_error_name(&e), "tts request failed");
                return Err(TtsError::upstream(format!("{} unreachable", engine.name)));
            }
        };
        let status = resp.status().as_u16();
        if status != 200 {
            tracing::warn!(engine = %engine.name, status = status, "tts error");
            return Err(TtsError::upstream(format!("{} returned HTTP {status}", engine.name)));
        }
        let content = match resp.bytes().await {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(engine = %engine.name, error = httpx_error_name(&e), "tts request failed");
                return Err(TtsError::upstream(format!("{} unreachable", engine.name)));
            }
        };
        if fmt == "pcm16" {
            let (pcm, rate, channels) = wav_to_pcm16(&content)?;
            return Ok(Audio { sample_rate: rate, channels, ..Audio::new(pcm, content_type, fmt) });
        }
        Ok(Audio::new(content, content_type, fmt))
    }

    /// Whether the engine (Kyutai when None) answers its health route; None when it is not configured.
    pub async fn reachable(&self, engine: Option<&Engine>) -> Option<bool> {
        let engine = engine.or(self.kyutai.as_ref())?;
        match self
            .client
            .get(format!("{}{}", engine.url, engine.health_path))
            .timeout(Duration::from_secs(2))
            .send()
            .await
        {
            Ok(resp) => Some(resp.status().as_u16() == 200),
            Err(_) => Some(false),
        }
    }
}

/// `asyncio.wait_for(future, deadline)` (None: no limit).
async fn with_deadline<F>(deadline: Option<f64>, future: F) -> Result<Audio, ServerFailure>
where
    F: std::future::Future<Output = Result<Audio, TtsError>>,
{
    match deadline {
        None => future.await.map_err(ServerFailure::Error),
        Some(seconds) => match tokio::time::timeout(Duration::from_secs_f64(seconds), future).await {
            Ok(result) => result.map_err(ServerFailure::Error),
            Err(_) => Err(ServerFailure::Timeout),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f64, rate: u32) -> Vec<u8> {
        let n = (seconds * rate as f64) as usize;
        (0..n)
            .flat_map(|i| {
                ((8000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin()) as i16).to_le_bytes()
            })
            .collect()
    }

    fn samples(pcm: &[u8]) -> Vec<f64> {
        pcm.as_chunks::<2>().0.iter().map(|s| i16::from_le_bytes([s[0], s[1]]) as f64).collect()
    }

    #[test]
    fn wav_float32_and_extensible_are_converted() {
        let floats: Vec<u8> = [0.0f32, 0.5, -1.0, 2.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let wav = {
            let mut fmt = Vec::new();
            fmt.extend(3u16.to_le_bytes());
            fmt.extend(1u16.to_le_bytes());
            fmt.extend(24000u32.to_le_bytes());
            fmt.extend((24000u32 * 4).to_le_bytes());
            fmt.extend(4u16.to_le_bytes());
            fmt.extend(32u16.to_le_bytes());
            let mut out = b"RIFF".to_vec();
            out.extend(((4 + 8 + fmt.len() + 8 + floats.len()) as u32).to_le_bytes());
            out.extend(b"WAVEfmt ");
            out.extend((fmt.len() as u32).to_le_bytes());
            out.extend(&fmt);
            out.extend(b"data");
            out.extend((floats.len() as u32).to_le_bytes());
            out.extend(&floats);
            out
        };
        let (pcm, rate, channels) = wav_to_pcm16(&wav).unwrap();
        assert_eq!(samples(&pcm), vec![0.0, 16383.0, -32767.0, 32767.0]);
        assert_eq!((rate, channels), (24000, 1));
        let raw: Vec<u8> = [1i16, -2].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(wav_to_pcm16(&pcm16_to_wav(&raw, 24000, 1)).unwrap().0, raw);
        assert!(wav_to_pcm16(b"nope").is_err());
    }

    #[test]
    fn resampling_44k_to_24k_keeps_length_and_tone() {
        let input = tone(1.0, 44100);
        // odd chunk sizes, not sample-aligned
        let mut r = Resampler::new(44100, 24000);
        let mut out = Vec::new();
        for chunk in input.chunks(4411) {
            out.extend(r.feed(chunk, false));
        }
        out.extend(r.feed(&[], true));
        assert_eq!(out.len() / 2, 24000);
        // the same as one call
        let whole = Resampler::new(44100, 24000).feed(&input, true);
        assert_eq!(whole, out);
        // quality: against the ideal 440 Hz tone at 24 kHz, away from the edges
        let got = samples(&out);
        let (mut signal, mut noise) = (0.0, 0.0);
        for (i, v) in got.iter().enumerate().take(23000).skip(1000) {
            let ideal = 8000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / 24000.0).sin();
            signal += ideal * ideal;
            noise += (v - ideal) * (v - ideal);
        }
        let snr = 10.0 * (signal / noise).log10();
        assert!(snr > 60.0, "SNR {snr:.1} dB");
    }

    #[test]
    fn resampling_removes_what_the_lower_rate_cannot_hold() {
        // 15 kHz at 44.1 kHz is above 12 kHz (24 kHz's Nyquist): filtered out, no alias left
        let n = 44100;
        let input: Vec<u8> = (0..n)
            .flat_map(|i| {
                ((8000.0 * (2.0 * std::f64::consts::PI * 15000.0 * i as f64 / 44100.0).sin()) as i16).to_le_bytes()
            })
            .collect();
        let out = samples(&Resampler::new(44100, 24000).feed(&input, true));
        let rms = (out[1000..23000].iter().map(|v| v * v).sum::<f64>() / 22000.0).sqrt();
        assert!(rms < 30.0, "alias rms {rms}");
        // upsampling 16 kHz -> 24 kHz: 1.5 times the samples
        let up = Resampler::new(16000, 24000).feed(&tone(0.5, 16000), true);
        assert_eq!(up.len() / 2, 12000);
    }

    #[test]
    fn mono_takes_the_first_channel() {
        let stereo: Vec<u8> = [1i16, 2, 3, 4, 5].iter().flat_map(|s| s.to_le_bytes()).collect();
        assert_eq!(samples(&mono(&stereo, 2)), vec![1.0, 3.0]);
    }

    #[test]
    fn voice_language_cases() {
        for (voice, language) in [
            ("pocket:loutre", "fr"),
            ("5476", "fr"),
            ("pocket:en/loutre", "en"),
            ("pocket:es/ours", "es"),
            ("pocket:de/lutin", "de"),
            ("pocket:xx/lutin", "fr"),
        ] {
            assert_eq!(voice_language(voice), language);
        }
    }

    #[tokio::test]
    async fn scheduler_reentrancy_after_release() {
        let sched = Arc::new(FifoScheduler::default());
        {
            let _slot = sched.slot(INTERACTIVE).await;
            assert_eq!(sched.depth(), 1);
        }
        {
            let _slot = sched.slot(BACKGROUND).await;
        }
        assert_eq!(sched.depth(), 0);
    }

    #[tokio::test]
    async fn scheduler_order_and_cancelled_waiters() {
        let sched = Arc::new(FifoScheduler::default());
        let first = sched.slot(BACKGROUND).await;
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for (name, priority) in [("bg", BACKGROUND), ("gone", INTERACTIVE), ("live", INTERACTIVE)] {
            let (sched, order) = (sched.clone(), order.clone());
            tasks.push(tokio::spawn(async move {
                let _slot = sched.slot(priority).await;
                order.lock().unwrap().push(name);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(sched.depth(), 4);
        tasks[1].abort();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(sched.depth(), 3);
        drop(first);
        for task in tasks {
            let _ = task.await;
        }
        assert_eq!(*order.lock().unwrap(), vec!["live", "bg"]);
        assert_eq!(sched.depth(), 0);
    }
}
