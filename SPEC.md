🇬🇧 English · [🇫🇷 Français](SPEC.fr.md)

# SPEC — Native iOS "voice console" app for Hermes agents (aibox)

Version 1 — 2026-10-04. Written on aibox from the actual state of the machine (see `context/aibox-facts.md`).
Audience: Claude Code on the MacBook (Xcode). The "aibox side" parts (§B) will be deployed on aibox
by the Claude Code running there, or over SSH (§D); the Mac writes their code and the contract.

## 0. Executive summary

Goal: replace SimpleX as the Hermes agents' iPhone channel with a **native Swift/SwiftUI iOS app**,
bot-oriented, with **voice without perceptible latency** (streaming both ways, barge-in),
**HITL approvals** via buttons, and **proactive messages** (cron check-ins) received as push notifications.

Non-negotiable principles:
1. **Native** (Swift 6, SwiftUI, current Xcode, iOS 18+ target). No React Native, no Expo, no WebView for the chat.
2. **Tailscale-only transport.** Nothing exposed to the Internet. HTTPS via `tailscale serve` with a real certificate
   (`aibox.example.ts.net`), hence App Transport Security with no exceptions. Push goes through APNs (Apple), the content is fetched over the tailnet.
3. **Streaming everywhere**: persistent WebSocket, audio in frames, TTS played from the first sentence, instant interruption.
4. **Hermes remains the source of truth**: sessions, memory, tools and approvals live in Hermes through its
   `api_server` (OpenAI-compatible + native routes). The app reimplements no agent logic.
5. **Open source from the start**: no secrets, no personal data in the repository. Configuration = list of agents (URL + key) entered in the app, stored in the Keychain.

Components:
- **iOS app** (Mac/Xcode) — §A.
- **aibox side** (already in place or to be installed) — §B: Hermes api_server ×2 profiles, `tailscale serve`, self-hosted **ntfy** server,
  and a small **bridge** service (Python, FastAPI) for voice (sentence-by-sentence streaming TTS, optional server STT) and APNs push.
- **API contract** between the two — §C.

## A. iOS app

### A1. Stack and structure
- Swift 6, SwiftUI, structured concurrency (`async/await`, `AsyncStream`), Observation (`@Observable`).
- Networking: `URLSession` (HTTP + SSE via `bytes(for:)`), `URLSessionWebSocketTask` for the voice bridge. No third-party networking dependency.
- Audio: `AVAudioEngine` (capture + playback), `AVAudioSession` category `.playAndRecord`, mode `.voiceChat`, options `.allowBluetooth`, `.defaultToSpeaker`.
- STT v1: **on the iPhone**. Prefer the modern Speech framework (`SpeechAnalyzer` / `SpeechTranscriber`, iOS 26+, on-device, streaming, fr-FR) with fallback to `SFSpeechRecognizer` (`requiresOnDeviceRecognition = true`) on iOS 18/19. v2 option: server STT via the bridge (§B3).
- Push: APNs (token-based auth), `UNUserNotificationCenter`, **Notification Service Extension** (fetches the full content over the tailnet) and `UNNotificationCategory` with "Approve / Deny" actions for approvals.
- Local persistence: SwiftData (cache of sessions/messages, list of agents). Secrets in the Keychain (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`).
- Targets: iPhone first; iPad and Mac Catalyst not a priority, but do nothing that rules them out.
- Repository structure: `App/` (SwiftUI), `Packages/HermesKit` (Hermes API client, pure Swift, testable without UI), `Packages/VoiceKit` (capture, playback, barge-in, bridge WebSocket client), `NotificationService/` (extension), `Tests/`.

### A2. Data model (app side)
- `Agent`: `id`, `name` (e.g. Wellness, Vie), `baseURL` (`https://aibox.example.ts.net:8642`), `apiKey` (Keychain), `voice` (Kyutai alias, e.g. `5476`), `color`, `defaultSessionID?`.
- `Session`: `id` (Hermes id), `agentID`, `title`, `updatedAt`, `lastMessagePreview`. Mirror of `GET /api/sessions`.
- `Message`: `id`, `role` (user/assistant/tool/system/notice), `text`, `createdAt`, `attachments` (images), `toolEvents` ([`ToolEvent`]), `reasoning?` (collapsible), `audioState` (none/synthesizing/playing/played).
- `ToolEvent`: `tool`, `preview`, `status` (started/completed/failed), `duration?`.
- `ApprovalRequest`: `runID`, `requestID?`, `command` (already redacted server-side), `choices` ⊆ {once, session, always, deny}, `agentID`, `sessionID`.
- `Run`: `runID`, `status` (running/waiting_for_approval/completed/failed/cancelled/interrupted/stopping).

### A3. Screens
1. **Agents** (root): one tab or list entry per configured agent; status badge (reachable / off tailnet / invalid key) obtained via `GET /health` + `GET /v1/capabilities`.
2. **Sessions** of an agent: paginated list (`GET /api/sessions?limit=&offset=`), creation (`POST /api/sessions`), renaming (`PATCH`), deletion, "new conversation".
3. **Conversation**: streaming message thread; compact tool cards (name + preview, expandable); collapsed "reasoning" block; **approval card** with buttons (Once / Session / Always / Deny, depending on `choices`); text + photo composer; **mic button** (long press = push-to-talk, tap = hands-free mode); stop button during a generation (`POST /v1/runs/{id}/stop`).
4. **Full-screen voice mode** ("call"): visualizer, live partial transcription, reply as text as it comes in, interruption by voice (barge-in) or by tap, hang up.
5. **Inbox**: proactive messages (cron) received by push, grouped by agent, with "reply" that opens the originating session or creates one.
6. **Settings**: agents (added via a form or via a JSON **QR code** `{name, baseURL, apiKey, voice}` displayed by aibox), bridge (URL, key), voice, STT (on-device / server), diagnostics (tailnet ping, measured latencies: first token, first audio).

### A4. Text flow (Hermes session)
- Sending: `POST /api/sessions/{id}/chat/stream` with `{"input": "...", "attachments"?: [...]}`, reading SSE. Events to handle: `assistant.delta` (append), `assistant.commentary` (temporary grey bubble), `tool.started` / `tool.completed` / `tool.failed` (cards), `approval.request` (approval card, the run moves to `waiting_for_approval`), `run.completed` / `run.failed` / `run.cancelled` (closing), `: keepalive` lines to ignore.
- Run identifier: read `run_id` from the event envelope (each event is a `_run_event(run_id, type, …)`; check the first event received on the real instance and record the exact name in `HermesKit`).
- Approval: `POST /v1/runs/{run_id}/approval` body `{"choice": "once|session|always|deny", "request_id"?: "…"}`. Accepted aliases: approve/approved/allow → once.
- Resuming after a disconnect (iOS VPN, backgrounding): `GET /v1/runs/{run_id}` for the state, `GET /v1/runs/{run_id}/events` to resubscribe, `GET /api/sessions/{id}/messages?inline_images=false` to resync the thread. Unconsumed event buffers expire after 5 min: beyond that, resync via `messages`.
- Images: multimodal `content` (`image_url` as `data:image/jpeg;base64,…`), downscale to ≤ 1600 px before sending.
- Multi-profile: each agent has its own base URL and its own key (two separate gateways, no `/p/<profile>/`).

### A5. Voice flow (goal: < 1 s between end of speech and start of audio, excluding model time)
1. The user speaks. On-device STT produces **partial transcriptions** displayed live; end-of-utterance detection (silence ≈ 600–800 ms, adjustable) triggers sending.
2. The app sends the final text to Hermes (§A4) **and** opens/keeps a WebSocket connection to the bridge (`wss://aibox.example.ts.net:8643/v1/voice`), passing it `{agent, session_id, run_id, voice}`.
3. The bridge subscribes itself to the run's event stream (`/v1/runs/{id}/events`), splits `assistant.delta` into sentences, synthesizes each sentence with Kyutai and sends binary audio frames back to the app (PCM int16 mono 24 kHz, or Opus), preceded by a JSON header `{seq, sentence_index, text, final}`.
   Acceptable variant for v1: the app does the splitting itself and calls the bridge's `POST /v1/tts/sentence` per sentence (HTTP, simpler), as long as playback starts at the first sentence.
4. The app plays the frames via `AVAudioPlayerNode` (scheduling successive buffers, queue). No waiting for the end of generation.
5. **Barge-in**: if capture detects voice (RMS level + non-empty partial STT) during playback: immediate stop of playback, `POST /v1/runs/{id}/stop`, `{"type":"cancel"}` message to the bridge, new utterance. Requires echo cancellation: `AVAudioSession` mode `.voiceChat` + `setVoiceProcessingEnabled(true)` on the input node.
6. Hands-free mode: listen → send → play → listen loop, as long as the call screen is open. Visual state indicator (listening / thinking / speaking).
7. Fallback: if the bridge is unreachable, local synthesis with `AVSpeechSynthesizer` (French voice) so as never to stay silent.

### A6. Push and proactive messages
- On first launch the app requests notification permission, obtains the APNs device token and registers it with the bridge: `POST /v1/devices {token, agent_ids, environment: sandbox|production}` (bridge key as Bearer).
- The bridge sends: (a) a "new proactive message" **alert** with `thread-id` = agent, `mutable-content: 1` and payload `{outbox_id, agent, session_id?}`; (b) an "approval required" **alert** with category `APPROVAL` (Approve/Deny actions) with `{run_id, request_id, agent}`; (c) a **silent** one (`content-available`) to resync.
- The **Notification Service Extension** fetches the full text (`GET /v1/outbox/{id}`) and, if available, the pre-synthesized audio (`GET /v1/outbox/{id}/audio` → mp3 as an attachment) over the tailnet. If the tailnet is unreachable (VPN off), show the title only. Tailscale iOS must be in **VPN on-demand** mode.
- The "Approve / Deny" notification actions call `POST /v1/runs/{run_id}/approval` directly (via the extension/`UNNotificationAction`, in the background).
- Inbox: `GET /v1/outbox?since=` when returning to the foreground.

### A7. Background, robustness, quality
- Background audio mode (`UIBackgroundModes: audio`) to keep playing a reply with the screen off; **CallKit** optional in v2 for call mode from the lock screen.
- WebSocket/SSE reconnection with backoff (0.5 s → 8 s), resumption by `run_id`.
- Metrics shown in Diagnostics: time to first token, first audio, STT duration; exportable log.
- Tests: `HermesKit` tested against a **mock SSE server** (fixtures of the events above); `VoiceKit` tested on sentence splitting and the audio queue.
- Accessibility: Dynamic Type, VoiceOver on cards, haptics at end of utterance.
- Localization: French first, externalized strings.

### A8. Out of scope for v1
Application-level end-to-end encryption (Tailscale is enough), multi-user, watchOS, CarPlay, editing cron jobs from the app (possible later via `/api/jobs`), handling files other than images (the api_server does not accept them).

## B. aibox side

### B1. Enable the Hermes api_server (one per profile)
- In `~/hermes-agent/hermes-home/profiles/wellness/.env`: `API_SERVER_ENABLED=true`, `API_SERVER_HOST=0.0.0.0` (required: the container uses pasta networking, 127.0.0.1 would not be published), `API_SERVER_PORT=8642`, `API_SERVER_KEY=<long random key, ≥ 32 chars>`. Same for `vie` with `API_SERVER_PORT=8644` and a **separate key**.
- In `gateway-run.sh`: add `-p 127.0.0.1:8642:8642` (wellness) and `-p 127.0.0.1:8644:8644` (vie) to `EXTRA_PORTS`. Bind on 127.0.0.1 only: exposure is done through `tailscale serve`.
- Restart the units (`systemctl --user restart hermes-gateway hermes-gateway-vie`) **outside of an ongoing turn** (see log; a SIGKILL mid-turn leaves a 5-min session lease).
- Check: `curl -H "Authorization: Bearer $KEY" http://127.0.0.1:8642/v1/capabilities` must advertise `run_submission`, `run_events_sse`, `run_approval`, `session_*`.
- Option: `gateway.api_server.tool_progress_events` stays `true`; `direct_model_requests` stays `false`.
- Toolsets for the `api_server` platform: add a `platform_toolsets.api_server` entry aligned with the `simplex` one in each profile (otherwise the default applies).

### B2. HTTPS exposure on the tailnet
- `sudo tailscale serve --bg --https=8642 http://127.0.0.1:8642`; same for `8644 → 8644`, `8643 → 8643` (bridge), `8645 → 8645` (ntfy). The HTTPS ports must be accepted by this version of `tailscale serve`; otherwise fall back to 443/8443/10000 with `--set-path` per service, or a local Caddy with `tailscale cert`.
- Result: `https://aibox.example.ts.net:8642/v1/...` reachable only from the tailnet, valid certificate, no firewalld rules.
- Test from the Mac: `curl https://aibox.example.ts.net:8642/health`.

### B3. Voice + push bridge (new service, port 8643, Python 3.12 + FastAPI + uvicorn, rootless podman container or venv + systemd user unit)
Responsibilities:
1. **Sentence-by-sentence streaming TTS**: `POST /v1/tts/sentence {text, voice}` → audio of one sentence (Kyutai `/v1/audio/speech`, `response_format: wav`, 24 kHz) returned as PCM int16 or Opus; and `WS /v1/voice`, which follows a Hermes run (`GET /v1/runs/{id}/events` with the agent's key, stored per agent on the bridge side) and pushes synthesized sentences as they come. Splitting: end of sentence (`.!?…`) or 180 characters; normalization (markdown stripped, lists read naturally). Pre-warming: sentence 1 sent to Kyutai as soon as it is complete; the following ones queued (Kyutai is serialized by a lock: one request at a time, hence a FIFO queue, no parallelism).
2. **Server STT (optional, v2)**: `WS /v1/stt` receiving 16 kHz PCM frames, incremental transcription (faster-whisper `large-v3-turbo` CPU 32 threads first; Kyutai STT `stt-1b-en_fr` in streaming if a ROCm/CPU runtime works). Off the critical path for v1.
3. **Push**: `POST /v1/devices` (APNs registration), **subscription to each agent's ntfy topic** (`GET http://127.0.0.1:8645/<topic>/json`, long stream), storage in a SQLite **outbox** (`id, agent, text, created_at, session_id?, audio_path?`), audio pre-synthesis of the message (Kyutai, mp3), APNs HTTP/2 sending (`.p8` token, Team ID, Key ID, bundle id; `aioapns` library or `httpx` + JWT). Routes `GET /v1/outbox?since=`, `GET /v1/outbox/{id}`, `GET /v1/outbox/{id}/audio`.
4. **Background approvals**: the bridge keeps track of the runs it observes; on `approval.request` with no WebSocket client connected, it sends the `APPROVAL` category push.
5. Auth: a single Bearer "bridge key" (different from the Hermes keys). Structured logging. `GET /health`.
- Bridge secrets (Hermes keys, APNs `.p8`) in `~/.config/hermes-ios/` (mode 600), never in the repository.

### B4. Self-hosted ntfy (port 8645) and switching the crons
- `podman run -d --name ntfy -p 127.0.0.1:8645:80 -v ~/ntfy:/var/cache/ntfy:z docker.io/binwiederhier/ntfy serve` (+ systemd user unit, `auth-default-access: deny-all` and one token per agent).
- Hermes: per profile, in `.env`: `NTFY_SERVER_URL=http://host.containers.internal:8645`, `NTFY_TOPIC=<unused-in-topic>`, `NTFY_PUBLISH_TOPIC=hermes-wellness-out` (resp. `hermes-vie-out`), `NTFY_HOME_CHANNEL=hermes-wellness-out`, `NTFY_TOKEN=…`, `NTFY_ALLOWED_USERS=<in-topic>`.
- Wellness crons "Morning sleep check-in" (08:15) and "Evening sleep plan" (20:00): `deliver: simplex` → `deliver: simplex,ntfy` during the transition, then `ntfy` alone. Technical jobs stay on `local`.
- The text delivered by Hermes is redacted of secrets on the Hermes side before sending.

### B5. Commissioning order on the aibox side
1. B1 + B2 (api_server + HTTPS) → the text app (§A4) becomes testable.
2. B3 TTS part → voice mode.
3. B4 + B3 push part → proactive messages, then shutting down SimpleX and Telegram for these agents.

## C. API contract (what the app expects)

### C1. Hermes api_server (per agent, Bearer = agent key) — full docs: `context/hermes-docs/api-server.md`, route table: `context/hermes-api-routes.txt`
- `GET /health`, `GET /v1/capabilities`, `GET /v1/models`
- Sessions: `GET/POST /api/sessions`, `GET/PATCH/DELETE /api/sessions/{id}`, `GET /api/sessions/{id}/messages?include_compacted=&inline_images=`, `POST /api/sessions/{id}/fork`, `POST /api/sessions/{id}/chat`, `POST /api/sessions/{id}/chat/stream` (SSE)
- Runs: `POST /v1/runs`, `GET /v1/runs/{id}`, `GET /v1/runs/{id}/events` (SSE), `POST /v1/runs/{id}/approval`, `POST /v1/runs/{id}/steer`, `POST /v1/runs/{id}/stop`
- Compat: `POST /v1/chat/completions` (SSE `chat.completion.chunk` + `event: hermes.tool.progress`, headers `X-Hermes-Session-Id`, `X-Hermes-Session-Key`), `POST /v1/responses`
- Cron jobs: `GET/POST /api/jobs`, `GET/PATCH/DELETE /api/jobs/{id}`, `POST /api/jobs/{id}/{pause|resume|run}` (app v2)
- Approval event: see `context/hermes-approval-and-events.txt` (`choices`, redacted `command`, `request_id`).
- Limits: 10 concurrent runs (429 beyond that), unread SSE buffers expire after 5 min, no upload of non-image files.

### C2. Bridge (Bearer = bridge key), base `https://aibox.example.ts.net:8643`
- `GET /health`
- `POST /v1/tts/sentence` `{text, voice, format: "pcm16"|"opus"}` → binary audio, headers `X-Sample-Rate: 24000`, `X-Channels: 1`
- `WS /v1/voice`: client → `{"type":"follow","agent":"wellness","run_id":"…","voice":"5476"}`; server → frames: JSON text message `{"type":"sentence","seq":n,"text":"…"}` followed by a binary audio message; `{"type":"done"}`; client → `{"type":"cancel"}`.
- `WS /v1/stt` (v2): client → binary PCM16 16 kHz; server → `{"type":"partial"|"final","text":"…"}`.
- `POST /v1/devices` `{token, environment, agent_ids}`; `DELETE /v1/devices/{token}`
- `GET /v1/outbox?since=<iso>`, `GET /v1/outbox/{id}`, `GET /v1/outbox/{id}/audio` (audio/mpeg)
- APNs payloads: `{"aps":{"alert":{"title":"Wellness","body":"…"},"thread-id":"wellness","mutable-content":1,"category":"MESSAGE"|"APPROVAL","sound":"default"},"outbox_id":"…","agent":"wellness","run_id":"…","request_id":"…"}`

### C3. Agent config (QR / JSON)
`{"name":"Wellness","baseURL":"https://aibox.example.ts.net:8642","apiKey":"…","voice":"5476","bridgeURL":"https://aibox.example.ts.net:8643","bridgeKey":"…"}`

## D. Claude Code (Mac) access to the Hermes configs on aibox

The `context/` folder (local, not published in the repository) already contains everything needed to write the app and the bridge without touching aibox:
machine facts, Hermes docs for the installed version (api-server, cron, ntfy, open-webui, programmatic integration),
route table, approval contract, Kyutai server source, systemd units.

If **live** access is needed (testing endpoints, reading a specific file):
1. Local **file sharing** (SMB or other) between the server and the Mac to drop copies to read.
2. **SSH over the tailnet**: via an authorized key (`ssh-copy-id` from the Mac) or Tailscale SSH (`sudo tailscale set --ssh`). Only one of the two.
3. **What Claude Code (Mac) may read on aibox** (read-only):
   - `~/hermes-agent/hermes-home/profiles/{wellness,vie}/config.yaml` (sections `gateway`, `platforms`, `platform_toolsets`, `approvals`, `tts`, `stt`), `.../cron/jobs.json`
   - the installed Hermes code: `podman exec hermes-gateway cat /opt/hermes-seed/hermes-agent/<path>` (in particular `gateway/platforms/api_server*.py`, `website/docs/...`)
   - `~/tts-lab/server/kyutai_server.py`, `systemctl --user cat <unit>`, `podman ps`, `ss -ltn`, `tailscale status`
4. **Forbidden from the Mac**: reading or copying `gateway-run.sh`, any `.env`, `~/.config/hermes-secrets/`, `auth.json` (plaintext secrets); restarting, stopping or modifying a systemd unit or a container; modifying a file under `~/hermes-agent/`; touching other accounts' services on the machine. Changes on the aibox side (§B) go through the owner or through aibox's Claude Code, which knows the pitfalls (restart mid-turn, `hermes update` overwriting patches).
5. **Integration tests** from the Mac once §B1–B2 are done: `curl -H "Authorization: Bearer $KEY" https://aibox.example.ts.net:8642/v1/capabilities`, then an SSE `chat/stream` with `curl -N`. Keys are passed out of band (never written to the repository).

## E. Milestones and acceptance criteria
1. **Text**: list the sessions of both agents, stream a reply with tool cards, approve a tool from the card, resume a run after killing the app. Target: first token displayed < 300 ms after the first `assistant.delta` arrives.
2. **Voice**: 5 s utterance → partial transcription visible while speaking; audio of the first sentence starting < 1.5 s after the first `assistant.delta`; barge-in cutting playback < 200 ms; no audio dropout on a 60 s reply.
3. **Proactive**: a test cron `deliver: ntfy` every 5 min arrives as a push in < 10 s, with the audio attached; "reply" opens the right session.
4. **Robustness**: switching LAN → 5G without losing the session; VPN on-demand cut then restored → automatic resync.
5. **Open source**: repository without secrets, installation README (Tailscale, api_server, bridge), MIT license.
