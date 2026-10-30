//! Port of tests/test_tts.py: TtsService against fake engines (Kyutai, Pocket, the person's own voice server).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use bipbridge::config::VoiceServerConfig;
use bipbridge::tts::{wav_to_pcm16, SynthOptions, TtsOptions, TtsService, BACKGROUND, INTERACTIVE};
use common::fakes::{pcm_for, FakeBackend};
use common::spawn_fake;
use futures::StreamExt;
use indexmap::IndexMap;
use serde_json::{json, Value};

fn service(backend: &FakeBackend, options: TtsOptions) -> Arc<TtsService> {
    Arc::new(TtsService::new(reqwest::Client::new(), Some(&backend.kyutai_url), "5476", options))
}

fn opts() -> SynthOptions {
    SynthOptions::default()
}

fn inputs(backend: &FakeBackend) -> Vec<Value> {
    backend.with(|s| s.kyutai_inputs.iter().map(|b| b["input"].clone()).collect())
}

#[tokio::test]
async fn pcm16_strips_wav_header_and_sends_kyutai_params() {
    let backend = FakeBackend::start().await;
    let tts = service(&backend, TtsOptions::default());
    let audio = tts
        .synthesize("Bonjour **toi** [lien](http://x.y).", Some("4193"), "pcm16", INTERACTIVE, opts())
        .await
        .unwrap();
    assert_eq!(
        backend.with(|s| s.kyutai_inputs.clone()),
        vec![json!({"model": "kyutai-tts", "input": "Bonjour toi lien.", "voice": "4193", "response_format": "wav"})]
    );
    assert_eq!(audio.data.to_vec(), pcm_for("Bonjour toi lien."));
    assert_eq!(
        (audio.sample_rate, audio.channels, audio.content_type.as_str()),
        (24000, 1, "application/octet-stream")
    );
}

#[tokio::test]
async fn fifo_order_and_no_parallel_calls() {
    let backend = FakeBackend::start().await;
    backend.with(|s| s.kyutai_delay = Duration::from_millis(20));
    let tts = service(&backend, TtsOptions::default());
    let texts: Vec<String> = ["un", "deux", "trois", "quatre", "cinq", "six", "sept", "huit"]
        .iter()
        .map(|w| format!("Phrase numéro {w}."))
        .collect();
    let mut tasks = Vec::new();
    for text in &texts {
        let (tts, text) = (tts.clone(), text.clone());
        tasks.push(tokio::spawn(async move { tts.synthesize(&text, None, "pcm16", INTERACTIVE, opts()).await }));
        tokio::task::yield_now().await; // arrival order = creation order
    }
    let mut results = Vec::new();
    for task in tasks {
        results.push(task.await.unwrap().unwrap().data.to_vec());
    }
    assert_eq!(inputs(&backend), texts.iter().map(|t| json!(t)).collect::<Vec<_>>());
    assert_eq!(backend.kyutai_max_active.load(Ordering::SeqCst), 1);
    assert_eq!(results, texts.iter().map(|t| pcm_for(t)).collect::<Vec<_>>());
}

#[tokio::test]
async fn interactive_overtakes_waiting_background_but_not_running() {
    let backend = FakeBackend::start().await;
    backend.with(|s| s.kyutai_delay = Duration::from_millis(30));
    let tts = service(&backend, TtsOptions::default());
    let spawn = |text: &'static str, fmt: &'static str, priority: u8| {
        let tts = tts.clone();
        tokio::spawn(async move { tts.synthesize(text, None, fmt, priority, opts()).await })
    };
    let first = spawn("Arrière plan un.", "mp3", BACKGROUND);
    tokio::time::sleep(Duration::from_millis(5)).await;
    let second = spawn("Arrière plan deux.", "mp3", BACKGROUND);
    tokio::task::yield_now().await;
    let third = spawn("Voix directe.", "pcm16", INTERACTIVE);
    for t in [first, second, third] {
        t.await.unwrap().unwrap();
    }
    assert_eq!(inputs(&backend), vec![json!("Arrière plan un."), json!("Voix directe."), json!("Arrière plan deux.")]);
}

#[tokio::test]
async fn cancelled_waiter_leaves_queue() {
    let backend = FakeBackend::start().await;
    backend.with(|s| s.kyutai_delay = Duration::from_millis(30));
    let tts = service(&backend, TtsOptions::default());
    let spawn = |text: &'static str| {
        let tts = tts.clone();
        tokio::spawn(async move { tts.synthesize(text, None, "pcm16", INTERACTIVE, opts()).await })
    };
    let a = spawn("Un.");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let b = spawn("Deux.");
    let c = spawn("Trois.");
    tokio::task::yield_now().await;
    b.abort();
    a.await.unwrap().unwrap();
    c.await.unwrap().unwrap();
    assert!(b.await.unwrap_err().is_cancelled());
    assert_eq!(inputs(&backend), vec![json!("Un."), json!("Trois.")]);
    assert_eq!(tts.scheduler.depth(), 0);
}

#[tokio::test]
async fn lru_cache_hits_and_evicts() {
    let backend = FakeBackend::start().await;
    let tts = service(&backend, TtsOptions { cache_entries: 2, ..Default::default() });
    let first = tts.synthesize("Salut.", None, "pcm16", INTERACTIVE, opts()).await.unwrap();
    let again = tts.synthesize("Salut.", None, "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert!(!first.cached && again.cached && inputs(&backend).len() == 1);
    tts.synthesize("Salut.", Some("4193"), "pcm16", INTERACTIVE, opts()).await.unwrap(); // other voice = other key
    tts.synthesize("Autre.", None, "pcm16", INTERACTIVE, opts()).await.unwrap(); // evicts ("5476", "Salut.")
    tts.synthesize("Salut.", None, "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(inputs(&backend).len(), 4);
}

#[tokio::test]
async fn errors() {
    let backend = FakeBackend::start().await;
    let tts = service(&backend, TtsOptions { max_chars: 20, ..Default::default() });
    let empty = tts.synthesize("```\ncode\n```", None, "pcm16", INTERACTIVE, opts()).await.unwrap_err();
    assert_eq!(empty.status, 400);
    let long = tts.synthesize(&"x ".repeat(50), None, "pcm16", INTERACTIVE, opts()).await.unwrap_err();
    assert_eq!(long.status, 413);
    backend.with(|s| s.kyutai_status = 500);
    let upstream = tts.synthesize("Ok.", None, "pcm16", INTERACTIVE, opts()).await.unwrap_err();
    assert_eq!(upstream.status, 502);
    let bad = tts.synthesize("Ok.", None, "flac", INTERACTIVE, opts()).await.unwrap_err();
    assert_eq!((bad.status, bad.message.as_str()), (400, "unsupported format: flac"));
}

#[tokio::test]
async fn pocket_only_routes_everything_to_pocket_and_never_to_kyutai() {
    let backend = FakeBackend::start().await;
    let tts = Arc::new(TtsService::new(
        reqwest::Client::new(),
        None,
        "pocket:loutre",
        TtsOptions { pocket_url: Some(backend.pocket_url.clone()), ..Default::default() },
    ));
    tts.synthesize("Bonjour.", Some("5476"), "pcm16", INTERACTIVE, opts()).await.unwrap(); // a Kyutai voice: the default Bip speaks instead
    assert_eq!(backend.with(|s| s.pocket_inputs.last().unwrap()["voice"].clone()), "loutre");
    assert!(inputs(&backend).is_empty());
    backend.with(|s| s.pocket_status = 503);
    assert!(tts.synthesize("Encore.", Some("pocket:loutre"), "pcm16", INTERACTIVE, opts()).await.is_err());
    assert!(inputs(&backend).is_empty());
    assert_eq!(tts.reachable(None).await, None);
}

// -- The person's voice server (here Drama 3 on audio.cpp, as an earlier install set it up) ------------------------

fn tone(seconds: f64, rate: u32) -> Vec<u8> {
    let n = (seconds * rate as f64) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * i as f64 / rate as f64).sin()) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn wav(pcm: &[u8], rate: u32, channels: u16) -> Vec<u8> {
    let mut out = b"RIFF".to_vec();
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

fn drama_config(url: &str) -> VoiceServerConfig {
    let mut server = VoiceServerConfig::new(url);
    server.model = "drama-3".into();
    server.stream_rate = Some(44100);
    server
}

fn drama_service(backend: &FakeBackend, server: VoiceServerConfig) -> Arc<TtsService> {
    Arc::new(TtsService::new(
        reqwest::Client::new(),
        Some(&backend.kyutai_url),
        "5476",
        TtsOptions { pocket_url: Some(backend.pocket_url.clone()), voice_server: Some(server), ..Default::default() },
    ))
}

type Seen = Arc<Mutex<Vec<Value>>>;

async fn body_json(request: Request) -> Value {
    let raw = axum::body::to_bytes(request.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&raw).unwrap_or(Value::Null)
}

async fn collect(tts: &Arc<TtsService>, text: &str, voice: &str, relay: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut stream = tts.stream(text, Some(voice), INTERACTIVE, None, relay);
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.unwrap());
    }
    out
}

fn last_pocket_voice(backend: &FakeBackend) -> Value {
    backend.with(|s| s.pocket_inputs.last().map(|b| b["voice"].clone()).unwrap_or(Value::Null))
}

#[tokio::test]
async fn drama_streams_its_44k_audio_resampled_to_24k() {
    let backend = FakeBackend::start().await;
    let seen: Seen = Arc::default();
    let s2 = seen.clone();
    let url = spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = s2.clone();
        async move {
            let body = body_json(request).await;
            seen.lock().unwrap().push(body);
            let audio = tone(1.0, 44100);
            // odd sizes: chunks need not be sample-aligned
            let chunks: Vec<Result<Bytes, std::io::Error>> =
                audio.chunks(4411).map(|c| Ok(Bytes::copy_from_slice(c))).collect();
            Response::new(Body::from_stream(futures::stream::iter(chunks)))
        }
    })))
    .await;
    let tts = drama_service(&backend, drama_config(&url));
    let text = "Bonjour, il est sept heures cinquante.";
    let out = collect(&tts, text, "drama:fr-mascotte-s17", None).await;
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            json!({"model": "drama-3", "input": text, "voice": "fr-mascotte-s17", "response_format": "pcm", "stream_format": "audio"})
        ]
    );
    assert!((out.len() as i64 / 2 - 24000).abs() <= 50, "{}", out.len()); // one second at 24 kHz
    let again = collect(&tts, text, "drama:fr-mascotte-s17", None).await;
    assert_eq!(again, out);
    assert_eq!(seen.lock().unwrap().len(), 1); // a complete take is cached
}

#[tokio::test]
async fn drama_down_speaks_with_a_pocket_bip_of_the_same_language() {
    let backend = FakeBackend::start().await;
    let tts = drama_service(&backend, drama_config("http://127.0.0.1:9")); // refused
    let out = collect(&tts, "Bonjour.", "drama:fr-mascotte-s17", None).await;
    assert_eq!(last_pocket_voice(&backend), "ours");
    assert!(!out.is_empty());
    let audio =
        tts.synthesize("Hello there.", Some("drama:en-naturelle-s47"), "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(last_pocket_voice(&backend), "en/galet");
    assert!(!audio.data.is_empty());
}

#[tokio::test]
async fn a_drama_take_reading_on_and_on_is_cut() {
    let backend = FakeBackend::start().await;
    let url = spawn_fake(Router::new().fallback(any(|| async {
        // 40 s of audio for a 4-character text
        let chunks = futures::stream::iter((0..40).map(|_| Ok::<_, std::io::Error>(Bytes::from(tone(1.0, 44100)))));
        Response::new(Body::from_stream(chunks))
    })))
    .await;
    let tts = drama_service(&backend, drama_config(&url));
    let out = collect(&tts, "Oui.", "drama:fr-naturelle-s15", None).await;
    assert!((out.len() as f64) / 2.0 / 24000.0 < 6.0);
}

#[tokio::test]
async fn drama_whole_files_and_formats_it_cannot_make() {
    let backend = FakeBackend::start().await;
    let url = spawn_fake(Router::new().fallback(any(|request: Request| async move {
        let body = body_json(request).await;
        if body["response_format"] == "mp3" {
            return StatusCode::BAD_REQUEST.into_response(); // audio.cpp makes no mp3
        }
        assert!(body["response_format"] == "wav" && body.get("stream_format").is_none());
        Response::new(Body::from(wav(&tone(0.5, 44100), 44100, 1)))
    })))
    .await;
    let tts = drama_service(&backend, drama_config(&url));
    let audio =
        tts.synthesize("Bonjour à toi.", Some("drama:fr-naturelle-s21"), "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(audio.sample_rate, 24000);
    assert!((audio.data.len() as i64 / 2 - 12000).abs() <= 50);
    let wav_audio =
        tts.synthesize("Bonjour à toi.", Some("drama:fr-naturelle-s21"), "wav", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(&wav_audio.data[..4], b"RIFF");
    assert_eq!(wav_to_pcm16(&wav_audio.data).unwrap().1, 24000);
    // outbox audio: a Bip reads it
    let mp3 =
        tts.synthesize("Bonjour à toi.", Some("drama:fr-naturelle-s21"), "mp3", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(last_pocket_voice(&backend), "galet");
    assert!(mp3.data.starts_with(b"ID3pocket"));
}

#[tokio::test]
async fn drama_voices_without_drama_go_to_pocket() {
    let backend = FakeBackend::start().await;
    let tts = service(&backend, TtsOptions { pocket_url: Some(backend.pocket_url.clone()), ..Default::default() });
    tts.synthesize("Salut.", Some("drama:fr-mascotte-s28"), "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(last_pocket_voice(&backend), "ours");
}

#[tokio::test]
async fn a_server_that_does_not_stream_gives_whole_wav_files_in_its_language() {
    let backend = FakeBackend::start().await;
    let seen: Seen = Arc::default();
    let s2 = seen.clone();
    let url = spawn_fake(Router::new().fallback(any(move |request: Request| {
        let seen = s2.clone();
        async move {
            let body = body_json(request).await;
            seen.lock().unwrap().push(body);
            Response::new(Body::from(wav(&tone(0.5, 24000), 24000, 1)))
        }
    })))
    .await;
    let mut server = VoiceServerConfig::new(&url);
    server.model = "kokoro".into();
    let tts = drama_service(&backend, server);
    let out = collect(&tts, "Hello, it is 7:50.", "server:en/af_bella", None).await;
    let first = seen.lock().unwrap()[0].clone();
    assert!(first["model"] == "kokoro" && first["voice"] == "af_bella" && first["response_format"] == "wav");
    assert!(first["input"].as_str().unwrap().contains("seven")); // English numbers: the voice's language
    assert!((out.len() as i64 / 2 - 12000).abs() <= 50);
}

#[tokio::test]
async fn mp3_from_a_server_that_makes_it() {
    let backend = FakeBackend::start().await;
    let url = spawn_fake(Router::new().fallback(any(|request: Request| async move {
        assert_eq!(body_json(request).await["response_format"], "mp3");
        Response::new(Body::from(&b"ID3server"[..]))
    })))
    .await;
    let tts = drama_service(&backend, VoiceServerConfig::new(&url));
    let audio = tts.synthesize("Bonjour.", Some("server:fr/v1"), "mp3", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(&audio.data[..], b"ID3server");
    assert_eq!(audio.format, "mp3");
}

#[tokio::test]
async fn the_server_voices_in_its_order_from_whichever_route_it_has() {
    let backend = FakeBackend::start().await;
    let audiocpp = spawn_fake(Router::new().fallback(any(|request: Request| async move {
        if request.uri().path() == "/v1/audio/voices" {
            return Json(json!({"voices": ["fr-naturelle-s15", "fr-mascotte-s17"]})).into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    })))
    .await;
    let tts = drama_service(&backend, drama_config(&audiocpp));
    assert_eq!(tts.server_voices(300.0).await, ["fr-naturelle-s15", "fr-mascotte-s17"]);
    let kyutai = spawn_fake(Router::new().fallback(any(|request: Request| async move {
        if request.uri().path() == "/v1/voices" {
            return Json(json!({"default": "5476", "aliases": {"5476": "a.wav", "posee": "b.wav"}})).into_response();
        }
        StatusCode::NOT_FOUND.into_response()
    })))
    .await;
    let tts = drama_service(&backend, VoiceServerConfig::new(&kyutai));
    assert_eq!(tts.server_voices(300.0).await, ["5476", "posee"]);
    let bare = spawn_fake(Router::new().fallback(any(|| async { StatusCode::NOT_FOUND }))).await;
    let mut server = VoiceServerConfig::new(&bare);
    server.voices = vec!["alba".into(), "marius".into()];
    let tts = drama_service(&backend, server);
    assert_eq!(tts.server_voices(300.0).await, ["alba", "marius"]);
}

#[tokio::test]
async fn the_persons_own_respellings_win() {
    let backend = FakeBackend::start().await;
    let mut fr = IndexMap::new();
    fr.insert("Garmin".to_string(), "Garmine".to_string());
    fr.insert("Oura".to_string(), "Ou-ra".to_string());
    let mut words = IndexMap::new();
    words.insert("fr".to_string(), fr);
    let tts =
        service(&backend, TtsOptions { pocket_url: Some(backend.pocket_url.clone()), words, ..Default::default() });
    tts.synthesize("Ta montre garmin et ta bague Oura.", Some("pocket:galet"), "pcm16", INTERACTIVE, opts())
        .await
        .unwrap();
    assert_eq!(
        backend.with(|s| s.pocket_inputs.last().unwrap()["input"].clone()),
        "Ta montre Garmine et ta bague Ou-ra."
    );
    tts.synthesize("Your Garmin.", Some("pocket:en/galet"), "pcm16", INTERACTIVE, opts()).await.unwrap();
    assert_eq!(backend.with(|s| s.pocket_inputs.last().unwrap()["input"].clone()), "Your Garmin.");
    // French respellings stay French
}

#[tokio::test]
async fn a_voice_server_too_slow_for_a_live_sentence_hands_over_to_the_agents_bip() {
    let backend = FakeBackend::start().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let c2 = calls.clone();
    let url = spawn_fake(Router::new().fallback(any(move || {
        let calls = c2.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_secs(5)).await; // a GPU busy with something else
            Response::new(Body::from(wav(&tone(0.5, 24000), 24000, 1)))
        }
    })))
    .await;
    let mut server = VoiceServerConfig::new(&url);
    server.first_audio_seconds = 0.2;
    let tts = drama_service(&backend, server);
    let out = collect(&tts, "Bonjour.", "server:fr/v1", Some("pocket:mousse")).await;
    assert!(!out.is_empty());
    assert_eq!(last_pocket_voice(&backend), "mousse");
    // For a while, the next sentences go to the Bip at once: one voice through the Live.
    let relay = SynthOptions { relay: Some("pocket:mousse".into()), ..Default::default() };
    let audio = tts.synthesize("Ça va ?", Some("server:fr/v1"), "pcm16", INTERACTIVE, relay).await.unwrap();
    assert!(!audio.data.is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(last_pocket_voice(&backend), "mousse");
    // The outbox's reading (background) waits for the server.
    tts.clear_slow();
    let t2 = tts.clone();
    let task =
        tokio::spawn(
            async move { t2.synthesize("Plus tard.", Some("server:fr/v1"), "pcm16", BACKGROUND, opts()).await },
        );
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!task.is_finished());
    task.abort();
}

#[tokio::test]
async fn queued_live_sentences_do_not_count_the_queue_against_the_server() {
    let backend = FakeBackend::start().await;
    let url = spawn_fake(Router::new().fallback(any(|| async {
        tokio::time::sleep(Duration::from_millis(150)).await; // each sentence well within its deadline once sent
        Response::new(Body::from(wav(&tone(0.3, 24000), 24000, 1)))
    })))
    .await;
    let mut server = VoiceServerConfig::new(&url);
    server.first_audio_seconds = 0.25;
    let tts = drama_service(&backend, server);
    let mut tasks = Vec::new();
    for n in 0..4 {
        let tts = tts.clone();
        tasks.push(tokio::spawn(async move {
            let relay = SynthOptions { relay: Some("pocket:mousse".into()), ..Default::default() };
            tts.synthesize(&format!("Phrase {n}."), Some("server:fr/v1"), "pcm16", INTERACTIVE, relay).await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap(); // the 4th waits 0.45 s in the queue: still the server's voice
    }
    assert!(backend.with(|s| s.pocket_inputs.is_empty()));
}
