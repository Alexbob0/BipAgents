# The bridge's black-box suite

This suite is the bridge's HTTP contract. It was written to decide whether the Rust bridge (`bridge-rs/`) could
replace the Python one (`bridge/bipbridge/`, retired on 2026-10-21, in the history), and now guards the Rust one. It
runs the bridge as a separate process and talks to it the way the apps, the agents, other bridges and the
administrator do: over HTTP, SSE, WebSocket and the command line. It reads and writes nothing of the bridge's code.
The only Python it imports is its own harness (`bbkit/`).

All tests pass (1 skipped) against the Rust bridge's release build. The full run takes about 4 minutes (Apple M4).

## Running it

From `bridge/`, with a Python that has the harness' packages (`tests_blackbox/requirements.txt`: pytest, httpx,
websockets, uvicorn, starlette, cryptography, PyJWT and PyYAML), after `cargo build --release` in `bridge-rs/`.

```sh
# Against the release build (the default: bridge-rs/target/release/bipbridge serve, or $CARGO_TARGET_DIR/release/)
python -m pytest tests_blackbox -q -p no:cacheprovider

# Against another binary (BRIDGE_SWITCH_CMD: the one test_switch.py hands the data to, e.g. an earlier release)
BRIDGE_CMD="/path/to/bipbridge serve" python -m pytest tests_blackbox -q -p no:cacheprovider

# One file, or one test, with the bridge's log when it fails (printed under « Captured stdout teardown »)
... -m pytest tests_blackbox/test_voice_and_approvals.py -q -k follow

# Without the 29 tests that wait for the bridge's own timers (jobs' 10 s polls, the cron watcher, keepalives…)
... -m pytest tests_blackbox -q -p no:cacheprovider -m "not slow"
```

`BRIDGE_CMD` is split like a shell command line and must end with `serve`. The CLI tests use the same command
without `serve`, followed by the subcommand they need: `check`, `genkey`, `qr --json [--agent X]`, `console`,
`peer-token --agent X`, `shares-mounts --agent X`, `agent-tools --agent X`, `spaces start`, `adopt`, `browser remove --agent X`. The suite
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
- `BIPAGENTS_MAIL_GUARD_PORT=<a free port>` (a place of the offer's e-mail guard otherwise listens on a port fixed by
  the account's uid, which another run's bridges hold)
- stdin `/dev/null`

No wait is unbounded: `wait_until` takes a timeout, the HTTP client 60 s, `read_sse` 120 s for a stream that never
ends (its keepalives would keep the read timeout from firing). Read a run's progress from a log file, not through
`| tail`: the tail shows nothing until pytest ends, so a run looks stalled while it goes on.

The suite waits for `GET /health` to answer 200, then sends SIGTERM at the end of the test. The default config is
`tests/conftest.py`'s `config_dict`, pointed at the fakes, with `[cron] watch = false`. Tests that need the cron
watcher turn it back on.

**The fakes** (`bbkit/fakes.py`, `bbkit/imap.py`). Each fake is a server on 127.0.0.1, started once per session and
reset before each test. Each one records every request it gets.

| Fake | Stands in for | Notes |
|---|---|---|
| Hermes api_server | Hermes | Several agents: the root one and the hub's `/p/<name>`. It serves sessions, messages, chat, `/v1/runs` (start, status, stop, SSE events from a script with gates, approval), capabilities and toolsets, and counts tokens while a run goes on. |
| Pocket TTS | Pocket | Same fake audio as `tests/conftest.py`. |
| Voice server | The person's own voice server | Drama 3 streaming 44.1 kHz, whole WAV files, mp3, a busy GPU, an endless take, `/v1/audio/voices` or `/v1/voices`. |
| APNs | Apple's push service | `/production/3/device/<token>` and `/sandbox/3/device/<token>`, over plain HTTP/1.1. Real APNs uses HTTP/2 over TLS, which both bridges negotiate by ALPN. |
| FCM | Google's push service | `/v1/projects/<id>/messages:send` plus the OAuth `/token` endpoint. |
| Publisher's push relay | `relay/` | `POST /v1/relay/push` |
| A model | The model's API | `/v1/models` and `/v1/chat/completions` (the mailbox scan's facts). |
| IMAP server | A mailbox | Reached like Proton Mail Bridge on this machine: plain IMAP, then STARTTLS with a self-signed certificate. IDLE tells new mail while the bridge waits (`idling` counts the connections in IDLE). |
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
  hosts. The suite sets both, so it can check that a sandbox device goes to the sandbox host
  (`ApnsConfig::base_url` / `sandbox_url`, `ApnsConfig::host`).
- **`[fcm] base_url`** replaces Google's FCM API (default `https://fcm.googleapis.com`). **`[fcm] token_url`** replaces
  the OAuth endpoint. Without `token_url`, the bridge uses the service account's `token_uri`, and Google's endpoint if
  that is missing (`Config::fcm_base_url` / `fcm_token_url`, passed to `FcmClient::new`).

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
7. **The search snippet of a multipart mail starts with its MIME lines.** It is `BODY[TEXT]` as IMAP gives it,
   spaces folded.
