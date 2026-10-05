🇬🇧 English · [🇫🇷 Français](README.fr.md)

# BipAgents bridge (`bipbridge`)

A small Python service (FastAPI + uvicorn) that runs on **aibox** next to the Hermes gateways and
serves the BipAgents iOS app over the tailnet (SPEC §B3, §C2, addendum `docs/aibox-fichiers.md`):

| Feature | Route | What the bridge does |
|---|---|---|
| Per-sentence TTS | `POST /v1/tts/sentence` | normalizes the text (markdown, links), calls Kyutai `/v1/audio/speech`, returns 24 kHz mono int16 PCM (or Opus/WAV); global FIFO queue + LRU cache |
| Streaming voice | `WS /v1/voice` | follows a Hermes run (`GET /v1/runs/{id}/events`), splits the deltas into sentences, synthesizes them in order, sends `sentence` (JSON) + audio (binary) |
| Files | `POST /v1/files` | drops PDF/Excel/text… files into a folder mounted in the Hermes container and returns the path **as seen by the agent** |
| Push | `POST/DELETE /v1/devices` | registers APNs device tokens (SQLite), sends pushes over HTTP/2 + ES256 JWT token |
| Proactive messages | `GET /v1/outbox…` | subscribes to each agent's ntfy topic, stores the messages, pre-synthesizes the audio (mp3), pushes `MESSAGE` |
| Approvals | `POST /v1/watch`, `POST /v1/approve`, `GET /v1/approvals` | watches a run in the background, pushes `APPROVAL` if nobody is following it, relays the answer to Hermes |
| Health | `GET /health` | no authentication: version + Kyutai/ntfy reachability |

The bridge listens **only on `127.0.0.1:8643`**; HTTPS exposure is handled by `tailscale serve`.
It contains no secrets: everything lives in `~/.config/hermes-ios/` (mode 600).

## 1. Installation on aibox (venv + systemd --user, recommended)

Prerequisites: Python ≥ 3.11 (3.12 targeted; the code also runs on 3.9+), Kyutai on `:8097`, ntfy on `:8645`,
Hermes api_server on `:8642` (wellness) and `:8644` (vie) — SPEC §B1, §B4.

```bash
# 1. Code: copy the repo's bridge/ folder to ~/hermes-bridge (git clone, scp or SMB ~/Public)
mkdir -p ~/hermes-bridge && cp -r <repo>/bridge/. ~/hermes-bridge/
cd ~/hermes-bridge

# 2. venv (python3.12 if installed, otherwise the system python3 if it is >= 3.11)
python3.12 -m venv .venv || python3 -m venv .venv
.venv/bin/pip install --upgrade pip
.venv/bin/pip install -r requirements.txt

# 3. Configuration
mkdir -p ~/.config/hermes-ios && chmod 700 ~/.config/hermes-ios
cp bridge.example.toml ~/.config/hermes-ios/bridge.toml
chmod 600 ~/.config/hermes-ios/bridge.toml
.venv/bin/python -m bipbridge genkey          # -> put it in bridge_key (and give it to the app)
$EDITOR ~/.config/hermes-ios/bridge.toml       # Hermes keys, ntfy tokens, APNs, upload folders
.venv/bin/python -m bipbridge check            # validates the config (rejects CHANGE-ME values)

# 4. Upload folders visible to the agents (Hermes home mounted in the containers)
mkdir -p ~/hermes-agent/hermes-home/profiles/{wellness,vie}/uploads

# 5. systemd user unit
mkdir -p ~/.config/systemd/user
cp systemd/hermes-bridge.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now hermes-bridge
journalctl --user -u hermes-bridge -f          # JSON logs

# 6. Local checks
curl -s http://127.0.0.1:8643/health
curl -s -H "Authorization: Bearer $BRIDGE_KEY" http://127.0.0.1:8643/v1/agents

# 7. Exposure on the tailnet (HTTPS, real certificate, nothing on the Internet)
sudo tailscale serve --bg --https=8643 http://127.0.0.1:8643
# from the Mac or the iPhone:
curl https://aibox.example.ts.net:8643/health
```

Updating: copy `bipbridge/` (and `requirements.txt`) again, `pip install -r requirements.txt`,
`systemctl --user restart hermes-bridge`. Restarting the bridge does not affect Hermes (no running turn is killed);
an ongoing voice conversation is simply cut off and the app can `follow` again with `from_seq`.

### Permissions on uploaded files (check once)

Files are created by the `aibox` user with mode `640` (folders `750`). They are readable inside the
container only if the agent's user is mapped to `aibox` there (e.g. `--userns=keep-id`):

```bash
podman exec hermes-gateway id
podman exec hermes-gateway ls -ln /home/hermes/.hermes/profiles/wellness/uploads/
# after a test upload from the app:
podman exec hermes-gateway cat /home/hermes/.hermes/profiles/wellness/uploads/<YYYY-MM>/<file>
```

If the container cannot read them: set `upload_file_mode = "644"` and `upload_dir_mode = "755"` in `[limits]`
(or a `setfacl` ACL for the mapped UID). Files older than 30 days are purged automatically
(only those written by the bridge: `YYYY-MM/<12 hex>-<name>`).

### ntfy: read token for the bridge

The bridge **reads** the `hermes-wellness-out` / `hermes-vie-out` topics (where Hermes publishes via `deliver: ntfy`):

```bash
podman exec -it ntfy ntfy user add bridge
podman exec -it ntfy ntfy access bridge 'hermes-*-out' read-only
podman exec -it ntfy ntfy token add bridge        # -> ntfy_token of each agent
```

The last ntfy id read is stored (SQLite) and sent back as `since=` on reconnection (backoff 0.5 → 8 s):
no message is lost as long as it is still in the ntfy cache (12 h by default).

### APNs

In Apple Developer: *Keys* → new key with *Apple Push Notifications service (APNs)* → download
`AuthKey_<KEYID>.p8` (only once). Copy it to `~/.config/hermes-ios/` (mode 600) and fill in `[apns]`
(`team_id`, `key_id`, `p8_path`, `bundle_id = "io.github.bipagents"`). `environment` is the default for devices
that don't specify one: `sandbox` for an app launched from Xcode, `production` for TestFlight/App Store
(the app sends its own on registration). Without a filled-in `[apns]`, the bridge runs without push.

### Option: podman container

```bash
podman build -t localhost/hermes-bridge:0.1.0 -f Containerfile .
podman run -d --name hermes-bridge --network=host --userns=keep-id \
  -v ~/.config/hermes-ios:$HOME/.config/hermes-ios:ro -e BRIDGE_CONFIG=$HOME/.config/hermes-ios/bridge.toml \
  -v ~/.local/share/hermes-bridge:$HOME/.local/share/hermes-bridge \
  -v ~/hermes-agent/hermes-home/profiles:$HOME/hermes-agent/hermes-home/profiles \
  -e HOME=$HOME localhost/hermes-bridge:0.1.0
```

`--network=host` is required to reach Kyutai/ntfy/Hermes on 127.0.0.1; the folders are mounted **at the same
path** as on the host so the config stays identical. No `:Z` on `hermes-home` (relabeling would break
access for the Hermes containers). In `bridge.toml`, `p8_path` must then point under `/config/`.

## 2. Configuration (`~/.config/hermes-ios/bridge.toml`)

See `bridge.example.toml` (commented). The path can be changed with `BRIDGE_CONFIG`. A warning is logged if the
file is readable by group or others. Every secret also accepts the `<name>_file = "path"` form.

| Key | Default | Purpose |
|---|---|---|
| `bridge_key` | — (required) | the app's single Bearer key (≥ 32 chars, `python -m bipbridge genkey`) |
| `host`, `port` | `127.0.0.1`, `8643` | local listening address |
| `[kyutai] enabled`, `url`, `default_voice` | `true`, `http://127.0.0.1:8097`, `5476` | Kyutai 1.6B TTS (GPU); `enabled = false`: Pocket only |
| `[pocket] url`, `default_voice` | absent (disabled), `pocket:loutre` | Kyutai Pocket TTS (`pocket/`) for the Bips' voices; `default_voice` is used when Kyutai is disabled |
| `[ntfy] url` | `http://127.0.0.1:8645` | |
| `[apns] team_id, key_id, p8_path, bundle_id, environment` | push disabled if empty | |
| `[push] previews` | `false` | `false`: generic notification body, the text travels over the tailnet |
| `[push] watch_followed_runs` | `true` | a run followed in voice mode stays watched (approvals) after the WebSocket closes |
| `[outbox] db_path, audio_dir, retention_days, audio_max_chars, audio_wait_seconds` | `~/.local/share/hermes-bridge/…`, 90 d, 2000, 8 s | |
| `[limits] upload_max_mb, upload_retention_days, upload_file_mode, sentence_max_chars, tts_max_chars, watch_max_minutes` | 50, 30, `640`, 180, 1000, 120 | |
| `[agents.<id>] display_name, hermes_url, hermes_key, ntfy_topic, ntfy_token, upload_dir_host, upload_dir_container, voice` | | lowercase `<id>` = identifier used by the app (`wellness`, `vie`) |

## 3. What the iPhone calls

Base: `https://aibox.example.ts.net:8643`, header `Authorization: Bearer <bridge key>` everywhere except `/health`
(otherwise 401 + `WWW-Authenticate: Bearer`; WebSocket rejected with 403 / code 1008). The agent identifier is
case-insensitive (`wellness`, `vie`).

- **Launch / Settings**: `GET /health` (status dot), `GET /v1/agents` (agents known to the bridge).
- **Notifications**: on startup, `POST /v1/devices {token, environment, agent_ids}`; on sign-out
  `DELETE /v1/devices/{token}`.
- **Voice mode**: after starting the Hermes turn (chat/stream) and noting `run_id`, open `WS /v1/voice`, send
  `follow`, play each binary message received after its `sentence` header; barge-in: `{"type":"cancel"}` + `POST /v1/runs/{id}/stop`
  on the Hermes side. Simple variant: `POST /v1/tts/sentence` per sentence.
- **Going to the background during a run**: `POST /v1/watch {agent, run_id}` → `APPROVAL` push if an approval comes in,
  silent `run_finished` push at the end.
- **Notification Service Extension**: `MESSAGE` → `GET /v1/outbox/{outbox_id}` + `GET /v1/outbox/{id}/audio` (mp3);
  `APPROVAL` → `GET /v1/approvals?agent=` to display the command; Approve/Deny actions → `POST /v1/approve`.
- **Inbox**: `GET /v1/outbox?since=<created_at of the last item>` when returning to the foreground.
- **Files**: `POST /v1/files` (multipart `agent`, `file`) then a `[Pièce jointe : … → <path>]` line in the message.

The detailed contract (exact payloads) is in the next section.

## 4. API contract

### `GET /health` (public)
`{"ok": true, "version": "0.1.0", "kyutai": bool, "ntfy": bool, "apns": bool, "tts_queue": n}`

### `GET /v1/agents`
`{"agents": [{"id": "wellness", "name": "Wellness", "voice": "5476", "uploads": true, "inbox": true}]}`

### `POST /v1/tts/sentence`
Body: `{"text": "…", "voice"?: "5476", "format"?: "pcm16"|"opus"|"wav" (default pcm16), "agent"?: "wellness"}`
(voice: `voice`, otherwise the agent's, otherwise `default_voice`). Binary 200 response:
- `pcm16`: `Content-Type: application/octet-stream`, signed 16-bit little-endian PCM, mono;
  headers `X-Sample-Rate: 24000`, `X-Channels: 1`, `X-Sample-Format: s16le`
- `opus`: `audio/ogg` (Ogg Opus); `wav`: `audio/wav`
- always `X-Cache: hit|miss`. Errors: 400 (empty text after normalization, format), 404 (agent), 413 (> 1000 chars),
  502 (Kyutai unreachable / error).

### `POST /v1/tts/stream`
Same body as `/v1/tts/sentence` (the format is always PCM16). Relays Kyutai `POST /v1/audio/stream`:
200 response with *chunked* transfer, 24 kHz mono s16le PCM sent as it is produced (first sound ≈ 0.75 s,
whatever the text length), headers `X-Sample-Rate`, `X-Channels`, `X-Sample-Format: s16le`.
Errors that occur before the first audio byte are regular HTTP errors (400, 404, 413 beyond
8000 chars, 502). A Kyutai without a streaming route (404) is replaced by a single-block synthesis. A complete stream
is cached. The app uses it in Live and falls back to `/v1/tts/sentence` if the bridge doesn't know the route.

### Voices and engines
`voice` (body of the TTS routes, or an agent's `voice` in the config): a Kyutai voice (`5476`, `4193`, `5207`, path
`cml-tts/fr/…`) goes through Kyutai 1.6B (GPU); a `pocket:<name>` voice (`pocket:loutre`, `pocket:chat2`, `pocket:lutin`,
`pocket:ours`, `pocket:colibri`, see `voices/french/`) goes through Kyutai Pocket TTS (CPU, `[pocket]` section), with its
own queue. If Pocket is not configured, is unreachable or fails before the first sound, the request
falls back to the default Kyutai voice (that audio is not cached). `/health` reports `"pocket": true|false|null`.

Other languages: `pocket:<language>/<name>` (`pocket:en/loutre`, `pocket:es/ours`, `pocket:de/lutin`, see
`voices/english|spanish|german/`) goes to the Pocket model for that language; the Pocket server receives `voice` =
`en/loutre`. The text is prepared in the voice's language (`bipbridge/speech_intl.py`, numbers via `num2words`:
"$5" → "five dollars", "23:15 Uhr" → "dreiundzwanzig Uhr fünfzehn"). Outside French, a Pocket failure does not
fall back to Kyutai (a French voice): the bridge returns 502/503 and the app reads the text with the iPhone's voice.

### Scheduled tasks and agent files
- Every minute, the bridge rereads each agent's `cron_…` sessions and puts the final answer of each
  task in the Inbox (`[SILENT]` ignored, `[cron]` section). `GET /v1/cron-jobs?agent=` lists the tasks seen
  (`{agent, job, name, notify, last_seen}`); `PUT /v1/cron-jobs/{agent}/{job}` `{"notify": false}` files them
  in the Inbox without a notification.
- **Bot Chat** (Hermes Bot Mode): the same watcher rereads each agent's "Bot Chat". A turn the agent
  takes on its own (a teammate's reply via `message_agent`, a routine) is pushed as "reply ready",
  unless the app was following that turn or the conversation is open on the phone. The app signals that a
  conversation is open by polling `GET /v1/sessions/{id}/state?agent=` (→ `message_count`) every
  few seconds, which it also uses to display those turns without a manual reload.
- `GET /v1/media?path=/home/hermes/.hermes/media/…` serves a file designated by an agent's `MEDIA:<path>` line, if it is
  under a `[media.roots]` folder and has a media extension (audio, image, PDF). In an Inbox message,
  the line is removed from the text and an mp3 becomes the message's audio.

### `WS /v1/voice`
Client → server (JSON text):
- `{"type":"follow","agent":"wellness","run_id":"…","voice"?:"5476","format"?:"pcm16"|"opus"|"wav","from_seq"?:0}`
  (`from_seq`: resume after reconnection, sentences numbered < `from_seq` are neither synthesized nor resent;
  a new `follow` cancels the previous one)
- `{"type":"cancel"}` · `{"type":"ping"}`

Server → client:
- `{"type":"following","run_id","agent","format","sample_rate":24000,"channels":1}`
- for each sentence, in order: `{"type":"sentence","seq":n,"text":"…","format","sample_rate","channels","bytes":len}`
  **immediately followed** by a **binary** message (the sentence's audio, raw PCM16 by default)
- `{"type":"done","run_id","reason":"completed"|"failed"|"cancelled"|"interrupted"|"error"|"stream_closed"}`
  (after `cancel`: `{"type":"done","run_id":null,"reason":"cancelled"}` if a follow was active)
- `{"type":"error","code","message"[,"seq"]}` — codes: `unknown_agent`, `invalid_run_id`, `invalid_format`,
  `invalid_json`, `run_not_found`, `hermes_refused`, `hermes_unreachable`, `tts_failed` (sentence skipped, the stream continues),
  `internal` · `{"type":"pong"}`

Splitting: end of sentence `.!?…` followed by whitespace (never `3.5`, nor `M.`/`etc.`/initials/list `1.`), line break,
or ~180 characters on a word boundary (preferably after a comma). Pending text is also spoken at
`tool.started` / `approval.request`. Markdown stripped, links → their text, bare URLs removed, code blocks ignored,
list items terminated with a period. If no delta arrived, the final text of `run.completed` is read.

### `POST /v1/files` (multipart/form-data: `agent`, `file`)
200: `{"path": "/home/hermes/.hermes/profiles/wellness/uploads/2026-10/0a1b2c3d4e5f-Releve_oct.pdf",
"filename": "Releve_oct.pdf", "size": 220512, "content_type": "application/pdf"}`
(`path` = path **inside the Hermes container**; on the host: `{upload_dir_host}/YYYY-MM/<12 hex>-<name>`, mode 640).
Sanitized name: base name only, no control characters, spaces and shell characters → `_`, no leading dot, ≤ 120 chars.
Errors: 400 (invalid multipart / no `file`), 404 (unknown agent), 409 (uploads not configured for the agent),
413 (> 50 MB).

### `POST /v1/devices`
Body: `{"token": "<APNs hex>", "environment"?: "sandbox"|"production", "agent_ids"?: ["wellness","vie"]}`
(empty `agent_ids` = all agents; missing `environment` = `[apns].environment`). 200 response:
`{"token","environment","agent_ids","created_at","updated_at"}`. Registering again updates it. 400: non-hexadecimal token,
invalid environment, `{"detail":{"error":"unknown agent_ids","agent_ids":[…]}}`.

### `DELETE /v1/devices/{token}` → 204 (idempotent)

### `GET /v1/outbox?since=<ISO 8601>&agent=<id>&limit=<1..500, default 100>`
`{"items": [Item…], "server_time": "2026-10-04T08:15:02.120Z"}`, sorted by ascending `created_at`, `since` exclusive
(ISO with `Z`/offset, or Unix seconds). Item:
```json
{"id": "9f3c…(32 hex)", "agent": "wellness", "title": "Check-in", "text": "full text (markdown)",
 "created_at": "2026-10-04T08:15:01.532Z", "sent_at": "2026-10-04T08:15:01.000Z", "session_id": null,
 "has_audio": true, "audio_url": "/v1/outbox/9f3c…/audio"}
```
`created_at` = receipt by the bridge (UTC, ms, fixed format); `sent_at` = ntfy timestamp; `session_id` comes from an
ntfy `session:<id>` tag if there is one. For pagination, pass the last item's `created_at` back as `since`.

### `GET /v1/outbox/{id}` → Item (404 otherwise)

### `GET /v1/outbox/{id}/audio`
200 `audio/mpeg`; if synthesis is still in progress (wait ≤ 20 s): 202 `{"status":"pending"}` + `Retry-After: 3`;
404 if there is no audio (text > `audio_max_chars`); 503 if Kyutai failed.

### `POST /v1/watch`
Body: `{"agent":"wellness","run_id":"…"}` → `{"watching": true, "agent", "run_id", "followers": n, "finished": bool}`.
The bridge subscribes to the run (a single Hermes subscription per run, shared with the WebSocket, with replay for a late
follower) for at most `watch_max_minutes`. On `approval.request` with no client attached (voice WebSocket or the
app's stream): `APPROVAL` push (once per `request_id`). At the end of the run with no client attached: "reply ready" push
(`run.completed` with a reply, `MESSAGE` category, `kind: "reply"`, `session_id` if known), silent push otherwise.

### `GET /v1/runs/{run_id}/events?agent=…&session_id=…`
SSE stream of the run for the app, relayed from the bridge's single subscription to Hermes (Hermes delivers each event
to only one subscriber): same events as Hermes (`event: <type>`, `data: <json>`), `: keepalive` every 10 s.
The run is replayed from its start on every connection (history kept 15 min after the end), so a reconnection
rebuilds the reply identically. Acts as `POST /v1/watch`: as long as the app is listening, no push; when it leaves
(conversation closed, screen locked), the bridge pushes the approval or the "reply ready", which reopens the
`session_id` conversation. `bridge.error` (`run_not_found`, `hermes_refused`, `hermes_unreachable`, `watch_timeout`)
signals a bridge-side problem.

### `POST /v1/approve`
Body: `{"agent":"wellness","run_id":"…","choice":"once"|"session"|"always"|"deny","request_id"?:"…"}` → relayed to
Hermes `POST /v1/runs/{run_id}/approval` with the agent's key; Hermes's status and JSON are returned as is
(200 `{"object":"hermes.run.approval_response",…}`, 409 `approval_not_pending`, …). 502 if Hermes is unreachable.

### `GET /v1/approvals?agent=<id>`
`{"items":[{"agent","run_id","request_id","command","description","choices":["once","session","deny"],"created_at"}]}` —
approvals seen by the bridge and not yet resolved (command already redacted by Hermes).

### APNs payloads
```json
MESSAGE  {"aps":{"alert":{"title":"Wellness","body":"Nouveau message"},"thread-id":"wellness","mutable-content":1,
          "category":"MESSAGE","sound":"default"},"outbox_id":"…","agent":"wellness","session_id":"…"}
APPROVAL {"aps":{"alert":{"title":"Wellness","body":"Approbation requise"},"thread-id":"wellness","mutable-content":1,
          "category":"APPROVAL","sound":"default"},"agent":"wellness","run_id":"…","request_id":"…",
          "choices":["once","session","deny"]}
SILENT   {"aps":{"content-available":1},"agent":"wellness","reason":"run_finished","run_id":"…","status":"run.completed"}
```
`apns-push-type`: `alert` (priority 10) or `background` (priority 5); `apns-collapse-id` = `request_id` for
approvals; `apns-topic` = `bundle_id`. ES256 JWT token renewed every 50 min (and on `ExpiredProviderToken`).
A token returning 410, or 400 `BadDeviceToken`/`DeviceTokenNotForTopic`, is deleted. With `previews = true`, the body
contains an excerpt of the text (or of the command).

## 5. Internals

- **Serialized Kyutai**: one request at a time on the bridge side too, served in arrival order; "interactive"
  priority (voice, `/v1/tts/sentence`) ahead of "background" (outbox audio) — a synthesis that has already started is
  never interrupted. A cancelled request (barge-in) leaves the queue. LRU cache (voice, format, normalized text).
- **Prefetch**: each sentence is queued for Kyutai as soon as it is complete; delivery to the client stays in order.
- **Lenient SSE**: type read from the JSON (`type` or `event`), otherwise from the `event:` line; `: keepalive` ignored;
  text taken from `delta` / `text` / `content`. Text events: `message.delta`, `assistant.delta`.
- **Logs**: JSON on stdout (journald), never any secret or message text; device tokens truncated;
  no uvicorn access log (paths contain tokens).

Known limitations: `WS /v1/stt` (server-side STT, v2) is not implemented. If the local bridge → Hermes connection drops
in the middle of a run and Hermes replays the events on resubscription, a sentence could be repeated
(unlikely: both are on 127.0.0.1).

## 6. Development and tests

```bash
cd bridge
python3 -m venv .venv && .venv/bin/pip install -r requirements-dev.txt
.venv/bin/python -m pytest -q
```

The tests call no real service: Kyutai, Hermes (run SSE, approval), ntfy and APNs are mocked
with `httpx.MockTransport` (`tests/conftest.py`). They cover authentication, TTS FIFO ordering, sentence
splitting, `/v1/files` (path, size, names), devices, ntfy → outbox → push ingestion, the approval push
decision, `/v1/approve`, the APNs client (JWT, renewal) and the voice WebSocket (follow, cancel, errors).
