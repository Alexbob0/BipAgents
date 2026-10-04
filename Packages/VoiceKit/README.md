# VoiceKit

Voice I/O for the Hermes app (Swift 6, strict concurrency, no dependencies). It covers push-to-talk dictation,
the hands-free call loop with barge-in, and sentence-by-sentence TTS from the bridge with an on-device fallback.
Platforms: iOS 18+, macOS 15+.

- Tests: `swift test` runs on macOS. `xcodebuild test -scheme VoiceKit -destination 'platform=iOS Simulator,name=iPhone 17e'`
  runs them on iOS.
- iOS build: `xcodebuild -scheme VoiceKit -destination 'generic/platform=iOS Simulator' build`

## Public API

### `VoiceEngine` (`@MainActor @Observable`)

`VoiceEngine(configuration: .init(), tts: SystemTTSProvider())`

**Observable state**
- `state`: `.idle`, `.listening`, `.thinking` or `.speaking`.
- `inputLevel` / `outputLevel`: 0…1, smoothed with a fast attack and a slow release, on a -50…-6 dBFS scale.
  They are meant for waveforms and the mascot's mouth.
- `partialTranscript`: the live STT text.
- `metrics`: a `VoiceMetrics`.
- `lastError`.

**Permissions**
- `static requestPermissions() async -> Bool` asks for the microphone and speech recognition.
- `startDictation` and `startCall` throw `VoiceError.permissionDenied` if either one is missing.

**Dictation**
- `startDictation()`, then `finishDictation() -> String` (the final text) or `cancelDictation()`.
- Throws `VoiceError.busy` during a call.

**Call**
- `startCall(onUtterance:onInterrupt:)`, `endCall()`, `interrupt()`.
- The loop: listen, then end of utterance, then `onUtterance(text)` returns the reply's text deltas. The deltas
  are split into sentences, sent to TTS and played as they arrive. Then the engine listens again.
- Barge-in happens when the user speaks during playback (`configuration.bargeInEnabled`) or when `interrupt()`
  is called. It stops the player synchronously, drops the queued audio, cancels the reply stream, calls
  `onInterrupt()` and listens again. It keeps the words the user has already said.

**Speak**
- `speak(_:)` is the "Écouter" button. It cancels the current speech and returns when playback finishes or is
  stopped. Outside a call it uses the `.playback` session, so no microphone is opened and AirPods stay on A2DP.
- During a call it replaces the current reply. It is ignored while dictating.
- `stopSpeaking()`: during a call this is the same as `interrupt()`.

### `VoiceConfiguration`
| Field | Default | Purpose |
|---|---|---|
| `locale` | `fr-FR` | Speech recognition language |
| `endOfUtteranceSilence` | 700 ms | Silence that ends an utterance |
| `bargeInEnabled` | `true` | Interrupt playback when the user speaks |
| `speechThreshold` | 0.02 | Linear RMS level counted as speech for end of utterance |
| `bargeIn` | `BargeInDetector.Configuration()` | Barge-in tuning |

### `VoiceMetrics`
| Metric | From | To |
|---|---|---|
| `lastSTTDuration` | End-of-utterance decision, or `finishDictation()` | Final transcript |
| `lastFirstAudioLatency` | First text delta received | First buffer scheduled |
| `lastBargeInLatency` | Barge-in decision, timed on the host time of the mic frame | Player stopped |

### TTS
- **`TTSProvider`**: `synthesize(_:) async throws -> PCMChunk`, which may be called concurrently. It also has
  `beginReply()`, a no-op by default, called once per reply.
- **`PCMChunk`**: `samples` (Int16 LE mono) and `sampleRate`.
- **`BridgeTTSProvider(bridgeURL:bridgeKey:voice:session:)`**:
  - Sends `POST /v1/tts/sentence` with `{"text","voice","format":"pcm16"}` and `Authorization: Bearer`.
  - Reads the rate from `X-Sample-Rate` (24000 by default) and drops an odd trailing byte.
  - `timeout` defaults to 6 s. Non-2xx responses throw `TTSError.httpStatus`.
- **`SystemTTSProvider(language: "fr-FR")`**:
  - Uses `AVSpeechSynthesizer.write(_:toBufferCallback:)` and converts the result to PCM, so it goes through
    the same playback queue, echo cancellation and level meter.
  - Picks the best installed voice: premium, then enhanced, then default. Novelty voices are excluded.
- **`FallbackTTSProvider(primary:fallback:)`**:
  - If the primary fails, that sentence and the rest of the reply use the fallback, so the voice does not flip
    back and forth.
  - `beginReply()` re-arms the primary. Cancellation never triggers the fallback.
  - `isUsingFallback` reports which one is in use.

### Pure logic
These are unchanged.
- **`SentenceSplitter`**: incremental sentence cutting.
  - It cuts on `. ! ? …` and on line breaks, and falls back to a length cut on a word boundary near `maxLength`.
  - It handles French abbreviations, decimals and times, URLs and code fences.
- **`SpeechNormalizer`**: turns markdown into speakable text.
- **`EndOfUtteranceDetector`**: speech starts on the level alone. The utterance ends after silence, but only if
  a transcript exists.
- **`BargeInDetector`**: needs a sustained level and a non-empty transcript, and fires once per playback.

## Architecture (internal)

| Type | Role |
|---|---|
| `AudioSessionController` | `.voice` (`.playAndRecord`, `.voiceChat`, `.defaultToSpeaker`, `.allowBluetoothHFP`) or `.playback` (`.spokenAudio`). Active only while in use. Handles interruptions, route changes and media-services resets. No-op on macOS |
| `AudioGraph` | One `AVAudioEngine`: input with voice processing (AEC), a tap and an `AVAudioPlayerNode` feeding the mixer, plus a mixer tap for `outputLevel`. Rebuilt on `AVAudioEngineConfigurationChange`, and only if the engine actually stopped |
| `CaptureSink` | The audio-thread side. Splits tap buffers into 20 ms RMS frames and sends them to an `AsyncStream`, which the main actor reads to run the detectors. Gives a mono copy of mic audio to the current recogniser session. Never touches main-actor state |
| `SpeechRecognizerBackend` / `SpeechRecognitionSession` | One session per utterance. Uses `SpeechAnalyzer` + `SpeechTranscriber` (iOS 26+) when the `AssetInventory` reports the model installed. Otherwise uses `SFSpeechRecognizer` with `requiresOnDeviceRecognition` and starts the model download for next time. Final-result wait is capped at 1.5 s, after which the last partial is used |
| `SpeechPipeline` | Deltas go through `SentenceSplitter` to TTS. At most 2 syntheses are in flight: the sentence being awaited and the next one, prefetched. Sentences come out strictly in order as soon as each is ready. Cancelling it cancels the text stream and every request |
| `PlaybackQueue` | Ordered queue over an `AudioOutput` protocol (the player node in production, a fake in tests). Keeps 3 buffers scheduled (at least 2), so a 60 s reply has no gaps. `stop()` is synchronous, and a generation counter turns late completions into no-ops. `resetOutput()` replays unheard audio after a graph rebuild |
| `FormatConverter` | `AVAudioConverter` from PCM16 at any rate to the player's fixed Float32 mono 24 kHz format (the mixer handles the hardware rate). Also converts mic audio to the analyzer's format |

The player is connected at a fixed format, so audio that was already converted stays valid when a route change
rebuilds the graph. Each reply has an id, so a cancelled reply that resumes late cannot touch the queue or the
state of the next one.

## To calibrate and validate on a real iPhone

These cannot be checked in the simulator: it has no AEC, no real mic or route changes, and the
`SpeechTranscriber` assets may be missing.

**RMS thresholds** (`speechThreshold` 0.02, `bargeIn.speechThreshold` 0.05)
- They are linear RMS, measured after voice processing, which includes AGC and noise suppression.
- Measure with the speaker playing a reply:
  - echo residue must stay below the barge-in threshold;
  - normal speech at arm's length must clear it.
- Check in a quiet room and in a noisy one, and on the speaker, wired headphones and AirPods.

**Barge-in timing**
- `bargeIn.minSpeechDuration` (250 ms) and `gapTolerance` (120 ms) trade false triggers for reaction time.
- `lastBargeInLatency` must stay under 200 ms. It covers the time from detection to the player stopping,
  including the tap delivery delay and the main-actor hop.
- The IO buffer is set to 10 ms. Check what tap buffer size iOS actually delivers with voice processing on.

**End-of-utterance silence**
- Default 700 ms; the SPEC range is 600–800 ms.
- Check it against people who pause mid-sentence.

**Voice-processing quirks**
- Some devices report multi-channel input when VP is on. The code downmixes channel 0 to mono, which needs
  checking.
- VP ducks other audio and lowers playback volume in `.voiceChat`. Check that the reply is loud enough on the
  speaker.
- Enabling VP can fire an `AVAudioEngineConfigurationChange`. Check there is no rebuild loop or audible glitch
  at call start.
- In the simulator, VP may fail to enable. The engine continues without AEC.

**Speech recognition**
- `SpeechTranscriber` fr-FR model install and first-use latency, and per-utterance session start-up time.
  Audio is buffered meanwhile, so nothing is lost.
- `SFSpeechRecognizer` with two overlapping tasks: the previous one is finalising while the next has started.
- Echo words recognised during playback. The session is reset when playback starts and when it ends.

**Latency**
- First audio under 1.5 s after the first delta.
- `lastSTTDuration` measured with each backend.

**Robustness**
- 60 s reply with no dropouts.
- AirPods connected or disconnected mid-reply: the interrupted sentence replays from its start.
- A phone call interruption, then resume.
- `SystemTTSProvider` voice quality. Install an enhanced or premium French voice in Settings.
