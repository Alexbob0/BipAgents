# The bridge's black-box suite

This suite decides whether the Rust bridge (`bridge-rs/`) may replace the Python one (`bridge/bipbridge/`). It runs the
bridge as a separate process and talks to it the way the apps, the agents, other bridges and the administrator do:
over HTTP, SSE, WebSocket and the command line. It reads and writes nothing of the bridge's code. The only Python it
imports is its own harness (`bbkit/`), so the same tests run against both implementations.

All 246 tests pass against the Python bridge. The full run takes about 10 minutes.

## Running it

From `bridge/`, with a Python that has pytest, httpx, websockets, uvicorn, starlette, cryptography, PyJWT and PyYAML.
The main checkout's virtualenv has all of them.

```sh
# Against the Python bridge (the default: this Python's `-m bipbridge serve`, run from bridge/)
.venv/bin/python -m pytest tests_blackbox -q -p no:cacheprovider

# Against the Rust bridge
BRIDGE_CMD="../bridge-rs/target/release/bipbridge serve" \
  .venv/bin/python -m pytest tests_blackbox -q -p no:cacheprovider

# One file, or one test, with the bridge's log when it fails (printed under « Captured stdout teardown »)
... -m pytest tests_blackbox/test_voice_and_approvals.py -q -k follow

# Without the 11 tests that wait for the bridge's own timers (jobs' 10 s polls, the cron watcher, keepalives…): ~4 min
... -m pytest tests_blackbox -q -p no:cacheprovider -m "not slow"
```

`BRIDGE_CMD` is split like a shell command line and must end with `serve`. The CLI tests use the same command
without `serve`, followed by the subcommand they need: `check`, `genkey`, `qr --json [--agent X]`, `console`,
`peer-token --agent X`, `shares-mounts --agent X`, `spaces start`, `adopt`, `browser remove --agent X`. The suite
needs no network and runs nothing as root. It uses about 30 local ports.

## What a test sees

**The bridge process** (`bbkit/bridge.py`). Each test gets a folder of its own containing `bridge.toml` (mode 0600,
written from a dict), `data/`, `home/` and `bridge.log`. The process runs with a fixed, small environment:

- `BRIDGE_CONFIG=<folder>/bridge.toml`
- `HOME=<folder>/home`
- `PATH=<fake bin>:/usr/bin:/bin:/usr/sbin:/sbin`
- `TZ=UTC`
- `LANG=en_US.UTF-8`
- `NO_PROXY=*`
- stdin `/dev/null`

The suite waits for `GET /health` to answer 200, then sends SIGTERM at the end of the test. The default config is
`tests/conftest.py`'s `config_dict`, pointed at the fakes, with `[cron] watch = false`. Tests that need the cron
watcher turn it back on.

**The fakes** (`bbkit/fakes.py`, `bbkit/imap.py`). Each fake is a server on 127.0.0.1, started once per session and
reset before each test. Each one records every request it gets.

| Fake | Stands in for | Notes |
|---|---|---|
| Hermes api_server | Hermes | Several agents: the root one and the hub's `/p/<name>`. It serves sessions, messages, chat, `/v1/runs` (start, status, stop, SSE events from a script with gates, approval), capabilities and toolsets, and counts tokens while a run goes on. |
| Kyutai TTS | Kyutai | Same fake audio as `tests/conftest.py`. |
| Pocket TTS | Pocket | Same fake audio as `tests/conftest.py`. |
| Voice server | The person's own voice server | Drama 3 streaming 44.1 kHz, whole WAV files, mp3, a busy GPU, an endless take, `/v1/audio/voices` or `/v1/voices`. |
| ntfy | ntfy | `/v1/health` and `/<topic>/json` streams. |
| APNs | Apple's push service | `/production/3/device/<token>` and `/sandbox/3/device/<token>`, over plain HTTP/1.1. Real APNs uses HTTP/2 over TLS, which both bridges negotiate by ALPN. |
| FCM | Google's push service | `/v1/projects/<id>/messages:send` plus the OAuth `/token` endpoint. |
| Publisher's push relay | `relay/` | `POST /v1/relay/push` |
| Another bridge's relay | `[push] relay_url` | |
| A model | The model's API | `/v1/models` and `/v1/chat/completions` (the mailbox scan's facts). |
| IMAP server | A mailbox | Reached like Proton Mail Bridge on this machine: plain IMAP, then STARTTLS with a self-signed certificate. |
| Dropper | An unreachable host | Accepts each connection and closes it at once. |

**The fake executables** (`bbkit/fakebin.py`). These are first on the bridge's `PATH`, and `[hermes] bin` names the
fake `hermes`. The fakes are `hermes`, `podman`, `docker`, `systemctl`, `systemd-run`, `launchctl`, `ffmpeg` and
`fake-gateway`. Each run is recorded as JSON lines (argv, `HERMES_HOME`, stdin), and each fake answers like the real
command:

- `hermes profile create` makes the profile folder.
- `hermes config set` writes the profile's `config.yaml` or `.env`.
- `hermes profile delete` leaves Hermes' tombstone.
- `ffmpeg` copies its input to its output.
- `fake-gateway` serves a space's Hermes on the port the bridge wrote in its `.env`.

Per-test rules can make a command fail or print something (`bridge.commands.rule(...)`).

**Files written before or read after.** The suite checks the on-disk contract directly:

- SQLite tables, created with the Python schema before a start.
- `console.json`, `peers.json`, `agent-tools.json`, `households.json`, `reminders.json`, `device-keys.json`.
- The Fernet stores: `vault/` and `mail/`, decrypted with their key.
- `jobs/`, `mail-scans/`, `briefs/`, `call-recaps/`, `agent-pages/`, `agent-audio/`, `favicons/`, `sleep.json`.
- The bridge's own TOML (agents added, rewritten or removed; `[model]`, `[voice_server]`, `bridge_key` after a
  reopen).
- Profiles' `SOUL.md`, `config.yaml`, `.env`, `profile.yaml`, skills, `lease.json` and knowledge pages.
- File modes: 0600 and 0700 for private files, 0640 and 0644 for uploads, 0777 for share folders.

**Log lines.** The bridge's stdout must be JSON lines with the Python field names (`ts`, `level`, `logger`, `msg`,
plus each message's fields). About fifteen tests check a message and its fields: `device registered`,
`bridge started`, `wrong key`, `address blocked`, `run watch resumed`, `agent created`, `hermes woken`, the purges,
and others.

## Config keys added for this suite

None of these keys change anything when they are left out.

- **`[apns] base_url`** sends pushes to another host instead of Apple's, for both environments. **`[apns] sandbox_url`**
  sets the sandbox environment's host and takes precedence over `base_url`. Both default to empty, which means Apple's
  hosts. The suite sets both, so it can check that a sandbox device goes to the sandbox host.
  - In Python: `config.ApnsConfig.base_url` / `sandbox_url`, used by `apns.ApnsClient.send`.
  - In Rust: `ApnsConfig::base_url` / `sandbox_url` (`ApnsConfig::host`).
- **`[fcm] base_url`** replaces Google's FCM API (default `https://fcm.googleapis.com`). **`[fcm] token_url`** replaces
  the OAuth endpoint. Without `token_url`, the bridge uses the service account's `token_uri`, and Google's endpoint if
  that is missing.
  - In Python: `Config.fcm_base_url` / `fcm_token_url`, passed to `FcmClient`.
  - In Rust: `Config::fcm_base_url` / `fcm_token_url`, passed to `FcmClient::new` (same fallback to `token_uri`).

`tests/test_config.py::test_push_hosts_can_be_replaced_for_tests` covers these keys in the Python suite, and all 374
Python tests pass.

## Coverage: route → tests

The tables were generated from the routers (`@router.<method>(…)`) and the tests' requests, matched by method and path,
then completed by hand for the MCP endpoint (its calls go through `bbkit/mcp.py`), the voice WebSocket and the static
mounts. Every route has at least one test.

The original suite had about 310 client calls in 41 files. All of its HTTP-level behaviour is ported, and many of its
unit tests are ported through the routes too:

- the TTS queue and cache, the voice server, and Pocket alone;
- the cron watcher, the runs' resume after a restart, and the jobs' budget;
- spaces, shares, peers, the mailbox and its scan, and the desktop's RFB relay;
- the config errors.

The suite makes about 800 requests in 28 files.

| Route | Module | Tests (`test_<file>.py`, number of test functions) |
|---|---|---|
| `POST /v1/agent-tools/mcp` | agent_tools | agent_tools (6), pages (3), memory_and_notes (3), jobs (2), mailbox (2), agents_admin (1), background_and_protocol (1) — through `bbkit/mcp.py` |
| `GET /v1/agent-tools/mcp` | agent_tools | agent_tools (1) |
| `DELETE /v1/agent-tools/mcp` | agent_tools | agent_tools (1) |
| `GET /v1/agent-kinds` | agents_admin | agents_admin (2) |
| `POST /v1/agents` | agents_admin | agents_admin (10), http_routes (1) |
| `POST /v1/agents/claim` | agents_admin | agents_admin (2) |
| `GET /v1/agents/{agent_id}/settings` | agents_admin | agents_admin (6), browsers (2), shares_and_services (1) |
| `PUT /v1/agents/{agent_id}/language` | agents_admin | agents_admin (2) |
| `GET /v1/agents/{agent_id}/thinking` | agents_admin | agents_admin (1) |
| `PUT /v1/agents/{agent_id}/thinking` | agents_admin | agents_admin (2) |
| `PUT /v1/agents/{agent_id}/model` | agents_admin | agents_admin (2) |
| `PATCH /v1/agents/{agent_id}/settings` | agents_admin | agents_admin (3), browsers (2) |
| `GET /v1/agent-links` | agents_admin | households_and_peers (3) |
| `PUT /v1/agent-links` | agents_admin | households_and_peers (2) |
| `GET /v1/agent-shares` | agents_admin | shares_and_services (3) |
| `PUT /v1/agent-shares` | agents_admin | shares_and_services (5) |
| `GET /v1/tool-catalog` | agents_admin | shares_and_services (1) |
| `POST /v1/agents/{agent_id}/services` | agents_admin | shares_and_services (3) |
| `DELETE /v1/agents/{agent_id}/services/{name}` | agents_admin | shares_and_services (1) |
| `POST /v1/agents/{agent_id}/services/{name}/test` | agents_admin | shares_and_services (1) |
| `POST /v1/agents/{agent_id}/space` | agents_admin | spaces (2) |
| `DELETE /v1/agents/{agent_id}` | agents_admin | agents_admin (3), shares_and_services (1), spaces (1) |
| `GET /v1/agents` | app | http_routes (4), agents_admin (3), cloud_setup (3), console (3), lan (2), place (2), voices (2), background_and_protocol (1), config (1), relay_and_discovery (1), spaces (1), tts (1) |
| `POST /v1/tts/sentence` | app | http_routes (5), tts (1) |
| `POST /v1/tts/prepare` | app | http_routes (1), voices (1) |
| `POST /v1/tts/message` | app | http_routes (2), tts (2) |
| `POST /v1/tts/stream` | app | http_routes (10), tts (1) |
| `POST /v1/files` | app | http_routes (8), agents_admin (2) |
| `POST /v1/devices` | app | http_routes (4), fcm (3), relay_and_discovery (2), background_and_protocol (1), voice_and_approvals (1) |
| `DELETE /v1/devices/{token}` | app | http_routes (2), relay_and_discovery (1) |
| `GET /v1/outbox` | app | outbox_push (3), background_and_protocol (2), media_and_jobs (2), cronwatch (1), fcm (1), http_routes (1) |
| `GET /v1/outbox/{item_id}` | app | outbox_push (1) |
| `GET /v1/outbox/{item_id}/audio` | app | outbox_push (2), background_and_protocol (1), media_and_jobs (1) |
| `GET /v1/media` | app | agent_tools (2), media_and_jobs (1), pages (1) |
| `GET /v1/pages` | app | pages (1) |
| `DELETE /v1/pages/{page}` | app | pages (1) |
| `GET /v1/sessions/{session_id}/state` | app | cronwatch (2) |
| `GET /v1/status` | app | relay_and_discovery (2), agents_admin (1), config (1), vault (1) |
| `GET /v1/pairing` | app | lan (2) |
| `GET /v1/replies/{reply_id}` | app | voice_and_approvals (2), cronwatch (1) |
| `GET /v1/cron-jobs` | app | cronwatch (1), media_and_jobs (1) |
| `PUT /v1/cron-jobs/{agent}/{job}` | app | media_and_jobs (1) |
| `POST /v1/watch` | app | voice_and_approvals (6), background_and_protocol (2), fcm (2), http_routes (1) |
| `GET /v1/runs/{run_id}/events` | app | voice_and_approvals (5), background_and_protocol (1) |
| `POST /v1/approve` | app | voice_and_approvals (2), http_routes (1) |
| `GET /v1/approvals` | app | voice_and_approvals (3), http_routes (1) |
| `GET /health` | app | tts (6), http_routes (3), relay_and_discovery (3), background_and_protocol (1), fcm (1), lan (1), place (1) |
| `GET /v1/setup` | cloud_setup | cloud_setup (8) |
| `POST /v1/setup/claim` | cloud_setup | cloud_setup (7) |
| `GET /console` | console | console (1) |
| `GET /console/` | console | cloud_setup (1), console (1) |
| `POST /console/api/login` | console | console (3) |
| `POST /console/api/pairing` | console | cloud_setup (1), console (1) |
| `POST /console/api/pairing/wait` | console | console (1) |
| `GET /console/api/me` | console | console (1) |
| `POST /console/api/logout` | console | console (1) |
| `POST /v1/console/pairings/{code}` | console | console (1) |
| `GET /v1/console/devices` | console | console (3), cloud_setup (1) |
| `DELETE /v1/console/devices/{device}` | console | console (1) |
| `GET /desktop/{ticket}/` | desktop | desktop (1) |
| `WEBSOCKET /desktop/{ticket}/ws` | desktop | desktop (1) |
| `POST /v1/agents/{agent_id}/desktop` | desktop | desktop (5) |
| `POST /v1/agents/{agent_id}/desktop/control` | desktop | desktop (3) |
| `GET /v1/device-keys` | device_keys | cloud_setup (1) |
| `POST /v1/device-keys` | device_keys | cloud_setup (1) |
| `DELETE /v1/device-keys/{device}` | device_keys | cloud_setup (1) |
| `GET /v1/favicon/{host}` | favicons | place (1) |
| `GET /v1/jobs` | jobs | jobs (6), mailbox (1) |
| `POST /v1/agents/{agent_id}/jobs` | jobs | jobs (5) |
| `POST /v1/jobs/{job_id}/start` | jobs | jobs (1) |
| `POST /v1/jobs/{job_id}/pause` | jobs | jobs (3) |
| `POST /v1/jobs/{job_id}/resume` | jobs | jobs (3) |
| `DELETE /v1/jobs/{job_id}` | jobs | jobs (4) |
| `GET POST PUT PATCH DELETE /hermes/{agent}/{path:path}` | lan | lan (4), console (1) |
| `GET /v1/live/brief` | live_brief | live_brief (2) |
| `POST /v1/live/calls` | live_brief | live_brief (2) |
| `POST /v1/live/recaps` | live_brief | live_brief (1) |
| `POST /v1/live/recaps/take` | live_brief | live_brief (1) |
| `GET /v1/agents/{agent_id}/mail-scan` | mail_scan | mailbox (1) |
| `POST /v1/agents/{agent_id}/mail-scan` | mail_scan | mailbox (2) |
| `POST /v1/agents/{agent_id}/mail-scan/step` | mail_scan | mailbox (2) |
| `POST /v1/agents/{agent_id}/mail-scan/pause` | mail_scan | mailbox (1) |
| `POST /v1/agents/{agent_id}/mail-scan/resume` | mail_scan | mailbox (1) |
| `DELETE /v1/agents/{agent_id}/mail-scan` | mail_scan | mailbox (1) |
| `GET /v1/mail/providers` | mailbox | mailbox (1) |
| `GET /v1/mail/accounts` | mailbox | mailbox (1) |
| `POST /v1/mail/accounts` | mailbox | mailbox (3) |
| `PATCH /v1/mail/accounts/{account_id}` | mailbox | mailbox (1) |
| `DELETE /v1/mail/accounts/{account_id}` | mailbox | mailbox (1) |
| `GET /v1/agents/{agent_id}/memory` | memory_edit | memory_and_notes (3), mailbox (1) |
| `PUT /v1/agents/{agent_id}/memory/{which}` | memory_edit | memory_and_notes (1) |
| `GET /v1/agents/{agent_id}/memory/pages/{path:path}` | memory_edit | memory_and_notes (1) |
| `PUT /v1/agents/{agent_id}/memory/pages/{path:path}` | memory_edit | memory_and_notes (1) |
| `DELETE /v1/agents/{agent_id}/memory/pages/{path:path}` | memory_edit | memory_and_notes (1) |
| `POST /v1/agents/{agent_id}/memory/import` | notes_import | memory_and_notes (1) |
| `GET POST /peer/p/{target}/{path:path}` | peers | households_and_peers (3) |
| `POST /family/accept` | peers | households_and_peers (1) |
| `GET /family/agents` | peers | households_and_peers (2) |
| `GET POST /family/p/{target}/{path:path}` | peers | households_and_peers (1) |
| `GET /v1/family` | peers | households_and_peers (1) |
| `PUT /v1/family/name` | peers | households_and_peers (2) |
| `POST /v1/family/invite` | peers | households_and_peers (2) |
| `POST /v1/family/join` | peers | households_and_peers (2) |
| `DELETE /v1/family/{household}` | peers | households_and_peers (1) |
| `GET /v1/backup` | place_backup | cloud_setup (1), place (1) |
| `POST /v1/backup/now` | place_backup | place (1) |
| `POST /v1/import` | place_import | place (2) |
| `GET /v1/import` | place_import | place (1) |
| `DELETE /v1/import` | place_import | place (1) |
| `GET /v1/import/hello` | place_import | place (2) |
| `PUT /v1/import/archive` | place_import | place (2) |
| `POST /v1/relay/push` | relay | relay_and_discovery (6), background_and_protocol (1) |
| `GET /v1/reminders` | reminders | memory_and_notes (1) |
| `PUT /v1/reminders` | reminders | memory_and_notes (1) |
| `PUT /v1/reminders/agents` | reminders | memory_and_notes (1) |
| `GET /v1/reminders/queue` | reminders | memory_and_notes (1) |
| `POST /v1/reminders/queue/{op_id}` | reminders | memory_and_notes (1) |
| `GET /v1/ssh` | ssh_keys | place (1) |
| `POST /v1/ssh/keys` | ssh_keys | place (2) |
| `DELETE /v1/ssh/keys` | ssh_keys | place (1) |
| `GET /v1/vault` | vault | vault (5), spaces (1) |
| `POST /v1/vault` | vault | vault (6) |
| `PATCH /v1/vault/{item_id}` | vault | vault (2) |
| `DELETE /v1/vault/{item_id}` | vault | vault (1) |
| `GET /v1/voices` | voices | voices (3) |
| `PUT /v1/voices/uses` | voices | voices (3), agent_tools (1) |
| `PUT /v1/agents/{agent_id}/voices` | voices | voices (3) |
| `PUT /v1/agents/{agent_id}/appearance` | voices | voices (1) |
| `WEBSOCKET /v1/voice` | voice_ws | voice_and_approvals (12), http_routes (1) |
| `GET /console/static/…` (mount) | console | console (1) |
| `GET /desktop/static/novnc/…` (mount) | desktop | desktop (1) |

## What is not covered, and why

**Not reachable without the network, real hardware or another operating system:**

- **Real APNs and FCM.** Real APNs uses HTTP/2 over TLS with Apple's certificates, and real FCM needs Google. The fakes
  speak plain HTTP and check the payloads, headers, JWTs (FCM's RS256 assertion is verified; APNs' ES256 token is only
  checked for its `bearer` prefix) and the dead-token handling.
- **Site icons fetched from DuckDuckGo.** They need the Internet. Only the cache (`<data>/favicons/`), the 404 for a
  site without an icon, and the host validation are tested.
- **IMAP over implicit TLS (port 993) against public providers.** The fake IMAP server is reached the Proton Mail Bridge
  way (STARTTLS on 127.0.0.1, certificate not checked). Gmail-style IMAPS with a CA-signed certificate is not
  exercised.
- **Real podman or docker, browser containers, and a desktop inside a sandbox (`docker exec`).** The suite sees the
  commands only, through the fake `podman`.
  - `bipbridge browser add` and turning a browser on for the first time (`browsers.setup`) need a Linux host with a
    container runtime and the browser image. They are only tested when refused.
  - Turning an existing browser off and on, and `browser remove`, are tested.
- **systemd and launchd services for spaces.** The suite uses `[spaces] start` / `stop`, the administrator's own
  commands. The systemd unit and the launchd plist the bridge writes otherwise depend on the OS.
- **The LAN door's WebSocket (`wss://…/v1/voice`).** The door is tested over HTTPS: certificate pinning, the same
  routes, and the Hermes relay.

**Too slow for a test run:**

- **Putting an idle place to sleep.** The bridge checks every 30 minutes, after 14 idle days. Waking a sleeping place
  is tested.
- **Background loops' long periods.** The live brief's key points are covered by one 30 s test. The cron watcher waits
  10 s after start and is covered by two tests.

**Internal choices, not visible from outside.** These are left to the unit tests and to the Rust crate's golden
fixtures:

- the TTS scheduler's priorities (interactive before background);
- the resampler's exact samples;
- the guard's sliding window beyond « 20 wrong keys in 10 minutes block for 15 minutes »;
- the replay backlog's exact size limit (the merging of deltas is tested).

## Found while writing this suite

These are Python behaviours a port must reproduce, or decide about:

1. **French server voices keep their `fr/` prefix.** `tts.server_voice_id("server:fr/af_bella")` returns
   `fr/af_bella`, against its docstring. `speech_intl.LANGUAGES` lists only en, es and de, so French voices reach the
   voice server as `fr/<id>`. `test_agent_tools.py` does not pin the prefix.
2. **Config writes look for the lower-case agent table.** `voices.set_section_keys` and `agents_admin.save_agent` /
   `remove_agent_section` look for `[agents.<lower-case id>]`. A config written `[agents.Wellness]` (as
   `tests/conftest.py`'s is) gets a second `[agents.wellness]` table holding only the new keys. The next start then
   fails: `agents.wellness.hermes_url is required`. `test_voices.py` writes the table lower-case.
3. **Python exception names appear in answers.** Rust must print the same names:
   - `/v1/status` gives `{"ok": false, "error": "ConnectError"}` for a refused connection and `"HermesHTTPError"`
     for an HTTP error;
   - a mailbox that does not answer gives `127.0.0.1 doesn't answer (ConnectionRefusedError)`.
4. **Tool registration hashes the Python serialization of the tools.** The tools' version in the agents' MCP URL
   (`?v=`) is the SHA-256 of `json.dumps(TOOLS, sort_keys=True)`, with Python's separators and ASCII escaping.
   `test_agents_admin.py` checks this from outside. A different hash would make every agent register its tools again
   once after the switch.
5. **The CLI's JSON logs go to stdout.** They are mixed with the printed lines. The CLI tests filter the lines that
   start with `{`.
6. **SIGTERM exit status.** On SIGTERM, uvicorn re-raises the signal once it has stopped, so the process ends with -15.
   The suite accepts 0 or -15, within 10 s.
7. **`/v1/sessions/{id}/state` passes Hermes' `last_active` through as it is.** That includes `0`, where `null` might
   be expected.
8. **The search snippet of a multipart mail starts with its MIME lines.** It is `BODY[TEXT]` as IMAP gives it,
   spaces folded.
