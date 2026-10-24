# Hermes patches used by BipAgents

Patches for [Hermes Agent](https://github.com/NousResearch/hermes-agent) that the BipAgents app relies on,
until equivalent changes land upstream.

- **Hermes base commit:** `517b5e10f` (upstream `main` of 2026-10-08, v0.21.6+140). Every patch applies
  cleanly there.
- **Apply** from the root of a Hermes checkout at that commit: `git apply hermes/patches/<patch>`.
- **Upstream check:** `git apply --check` against upstream `main` at `a62979dc36` (2026-10-10, 380 commits
  after the base). It was run on a temporary index, without installing anything.

Numbers 0002 and 0003 were local patches of an install set up by hand: 0003's synchronous peer reply became
0006, 0002 is no longer needed (Hermes filters the roster agents see on its own).

**`next/`: the same patches for a newer Hermes.** People update Hermes whenever they want
(`install.sh hermes-update`). When a patch no longer applies there, the installer tries its version in `next/`,
rebased on upstream `main` at `97a9ae8ee` (2026-10-11): 0001 and 0004 (0005 applies as is). Same behaviour and
wire contract as the base patches; refreshed when upstream moves past them.

**Upstream.** 0005 is proposed as NousResearch/hermes-agent#136660. 0001 is offered to the open #116259 (branch
`Alexbob0:feat/runs-api-clarify`, rebased on `97a9ae8ee`, 80 tests passing) rather than as a competing PR. 0004 is
replaced upstream by NousResearch/hermes-agent#139873 (same replay window, using the copy Hermes already keeps of each
image, which is swept after 24 h); 0004 goes once a Hermes update brings it.

| Patch | On `517b5e10f` | On upstream `main` (`a62979dc36`) |
|---|---|---|
| 0001 api_server clarify | applies | **does not apply as is**: context conflict in `gateway/platforms/api_server_runs.py` (upstream switched `"web.Request"` annotations to `web.Request`); `git apply --3way` applies `api_server.py` cleanly and only needs that conflict resolved. Upstream has no clarify support in the Runs API yet. |
| 0004 image store | applies | applies |
| 0005 vault fill re-attach | applies | not checked yet |
| 0006 synchronous peer reply | applies | applies (`f86276d2eb`, 2026-10-13) |
| 0007 browser vault only | applies | applies (`97a9ae8ee`, checked 2026-10-22) |
| 0008 time zone, routines, wall clock | applies | applies (`97a9ae8ee`, checked 2026-10-24) |

## 0001 — `clarify` in the Runs API

**Problem.** `platform_toolsets.api_server` can enable the `clarify` tool, but the api_server adapter never
gives the agent a clarify callback. An agent driven through `POST /v1/runs` that asks a question gets
"Clarify tool is not available in this execution context", so an app cannot show the agent's question
with its choices. Approvals already work end to end.

**What the patch does.** It mirrors the approval bridge:
- a per-run clarify callback sends a `clarify.request` event on the run's SSE stream, sets the run status
  to `waiting_for_input` and waits for the answer, a stop or `agent.clarify_timeout`; batches of
  questions are supported;
- `POST /v1/runs/{run_id}/clarify` answers (`{"request_id", "answers": {qid: text | [texts] | null}}`) or
  dismisses (`{"request_id", "cancel": true}`); 404 when nothing is pending, 409 when already resolved,
  400 on incomplete answers;
- the run emits `clarify.responded` and `clarify.cancelled` (reason `timeout`, `stopped` or `cancelled`),
  and `/stop`, run retirement and the orphan sweep release a waiting question;
- `GET /v1/capabilities` advertises `"run_clarify": true`.

Clients that never receive `clarify.request` see no change.

**Files.** `gateway/platforms/api_server.py`, `gateway/platforms/api_server_runs.py`.

**Tests.** No dedicated test file. The existing `tests/gateway/test_api_server_runs*.py` pass, and the
flow was checked end to end against a live gateway.

## 0004 — keep images in the conversation for follow-up questions

**Problem.** The session database stores user messages as text only: an attached image becomes
`[screenshot]`. Each new run rebuilds its history from that database, so after the first turn the model
no longer sees the photo and cannot answer a follow-up question about it.

**What the patch does.**
- When a user message is saved, its `data:` images are written once to
  `<HERMES_HOME>/image_store/<sha256>.<ext>`, with a small reference file per message under
  `image_store/refs/`. The database row and the session messages API stay text only, with no base64.
  A remote URL is not kept for replay: the provider would fetch it again on every request, and an
  expired link would break every later turn. It reads as an older image, and keeps its position so
  the next images stay on their own markers.
- Writes go through `mkdir_under_hermes_home`, so a save that lands after `hermes profile delete`
  never brings a deleted agent's folder back.
- When a request is built, and only for a model that sees images natively, the 3 most recent images
  of the conversation are put back as `image_url` parts where their `[screenshot]` marker was. Older
  markers read `[image envoyée plus tôt]` ("image sent earlier"). The result depends only on the
  history, so the server's prefix cache stays valid from one turn to the next.
- Cleanup: each session keeps references to its 3 newest images only; deleting a session drops its
  references; a sweep at gateway startup drops references whose message is gone (deleted session,
  compaction). A file is removed once no reference points to it.

**Files.** `agent/image_store.py` (new), `agent/session_persistence.py`, `agent/turn_context.py`,
`gateway/run.py`, `hermes_state_sessions.py`, `tests/agent/test_image_store_local.py` (new, 13 tests).

The remote-URL, marker-position and deleted-profile fixes come from the maintainer's review of the
upstream PR (NousResearch/hermes-agent#136217, commit `28dd115949d`).

**Upstream note.** The placeholder text for older images is in French; an upstream version would use
English or make it configurable.

## 0005 — vault fills after the browser restarted

**Problem.** A vault fill reaches the page through the task's CDP supervisor. When the browser goes away (closed after
being idle, restarted), the supervisor gives up after a few reconnects and leaves the registry. The next fill (and its
origin check) then asks the local agent-browser daemon for a browser, never the one configured in `browser.cdp_url`:
with a sandboxed terminal that daemon is in the sandbox, and the fill fails with « Could not determine the current
page origin » for the rest of the conversation, even once the configured browser is back. With a browser that an MCP
server starts on demand (BipAgents' visible browser), that is any conversation that outlives one browser.

**What the patch does.** With a configured browser (`browser.cdp_url` or `BROWSER_CDP_URL`) and no live supervisor,
`_ensure_supervisor` attaches to that browser again, its endpoint resolved afresh (a restarted Chromium has a new
browser id). The origin read (`_eval_js`) goes through the same attach before falling back to the CLI. Without a
configured browser nothing changes.

**Files.** `tools/browser_vault_tool.py`.

**Tests.** Checked end to end against a live gateway: in one conversation, a fill, the browser closed, then started
again and a second fill. Proposed upstream with three tests: NousResearch/hermes-agent#136660.

## 0006 — a peer's reply comes back to an api_server session

**Problem.** An agent of the app (an api_server session) that messages an agent on a peer (`message_agent` to
`bip/<agent>`: the bridge, for an agent in its own space) gets « delivered » and is told the reply will come
later. An api_server session has no asynchronous delivery: the agent polls the delivery while the peer answers
into that same busy session, and both wait on each other until the timeouts (10 minutes seen).

**What the patch does.** When the session has no asynchronous delivery (`async_delivery_supported()` false), the
peer message runs `hermes peer dm --json` inline (600 s at most) and returns the peer's reply as the tool result,
asking the peer to answer in text in that turn. A peer that does not answer in time keeps the message in its Bot
Chat and the agent is told not to send it again. Sessions with asynchronous delivery (messaging platforms) are
unchanged.

**Tests.** `tests/tools/test_bot_mode_dm_sync_peer.py` (reply handed back, timeout); the Bot Mode suites still
pass (115).

## 0007 — `browser.vault_only`: the vault without Hermes' own browser

**Problem.** An agent with its visible browser (BipAgents' container: a real Chrome on a screen the person watches in
the app's « Bureau ») browses with the `stealth` MCP tools. Hermes' `browser` toolset has to stay on for the vault
(`browser_vault_*` ride with it and fill that same Chrome through `browser.cdp_url`), which also gives the agent
Hermes' own browser tools (`browser_navigate`, `browser_snapshot`, `browser_exec`…). The agent used them too: on a
browser container stopped while unused they fail (nothing starts it on a DevTools connection), and what they do is
not what the person sees started from « Bureau ».

**What the patch does.** With `browser.vault_only: true` in the profile's config, the tool selection keeps only the
`browser_vault_*` tools of the `browser` toolset; every other `browser_*` tool is left out, as if its toolset were
off. Without the key nothing changes. The bridge sets it for every agent whose visible browser is on (at its setup,
and at the bridge's start for those set up before).

**Files.** `model_tools.py` (`_select_tool_names`).

**Tests.** None upstream; checked with `patch --dry-run` on `517b5e10f` and `97a9ae8ee`.

## 0008 — the person's time zone: terminal, routines kept at home time, one run per wall-clock time

**Problems.** (1) The configured `timezone` reaches the prompts, cron and `execute_code`, but not the terminal: `date`
or a script run with the terminal tool tells the server's time (or the container's UTC). (2) Hermes caches the zone per
config path: a bridge that rewrites `timezone` (the person's phone moved to another zone) is not followed until the
gateway restarts. (3) One zone for everything: someone who travels gets either their routines moved silently, or a
« now » that stays at home time. (4) The autumn clock change ran some routines twice. Europe leaves summer time on
Sunday 25 October 2026 (03:00 CEST → 02:00 CET): a routine at `30 2 * * *` whose run ended in the repeated hour, or
`59 2 * * *` even with a one-minute run, got 02:30 / 02:59 *winter* time the same night, because `compute_next_run`
takes the second fold of the wall clock when the first one is already past (any recompute from « now » in the
repeated hour does the same: the end of a run, `advance`, a restart). Measured with a 30-second tick simulation on
`517b5e10f`: `30 2` (45-minute run) and `59 2` fired twice; `0 2`, `0 3` and `0 1` once. US: the same on 1 November
2026 at 01:xx.

**What the patch does.**
- `cron.timezone` (new key, `""` by default = the profile's `timezone`): the zone cron schedules are read in
  (`hermes_time.get_cron_timezone()`), used by `compute_next_run` and by the stale-schedule check.
- `compute_next_run`: a job pinned to given hours (hour field not a wildcard) runs once per wall-clock time — the
  repeated hour's second pass is never chosen for it; a job whose hour field is a wildcard (`*/15 * * * *`) keeps
  running in the second pass, as cron does. The spring gap is unchanged (02:30 that does not exist runs at 03:30).
- The zone cache is keyed by the config file's modification time: a rewritten `timezone` or `cron.timezone` applies
  at the next call, without a restart.
- Terminal commands (every backend: local, docker, ssh…) run after `export TZ=<the profile's zone>` when one is
  configured (not on Windows); an inline `TZ=… cmd` still wins.

**Files.** `hermes_time.py`, `cron/jobs.py`, `tools/environments/base.py`, `hermes_cli/config_defaults.py`; new
`tests/cron/test_cron_wall_clock_once_and_zone.py` (fall-back runs once with 1- and 45-minute runs, a base in the
second pass, hour-wildcard jobs, the spring gap, `cron.timezone`, a rewritten zone without restart, the terminal TZ).

**Tests.** The new file, and `tests/cron/` plus the timezone, system prompt, timestamp and environment suites: 1648
passed, 55 skipped (Python 3.14). Applies with `patch --dry-run` on `517b5e10f` and `97a9ae8ee`.

**Bridge side.** The bridge writes `timezone` (the phone's zone, at once) and `cron.timezone` (the routines' zone, which
changes only when the person answers the travel question) in each agent's profile; without this patch it restarts the
agent's gateway instead so `timezone` is read again, and `cron.timezone` is ignored (routines follow `timezone`).
