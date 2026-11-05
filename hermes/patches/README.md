# Hermes patches used by BipAgents

Patches for [Hermes Agent](https://github.com/NousResearch/hermes-agent) that the BipAgents app relies on,
until equivalent changes land upstream.

- **Hermes base:** the stable release **v0.22.5** (`bf0203a794`, 2026-10-22), in `BASE`; the installer pins it
  (`HERMES_COMMIT`, installer/bipinstall/hermes.py). Every patch applies cleanly there, in order.
- **Apply** from the root of a Hermes checkout at that commit: `git apply hermes/patches/<patch>`, in order.
- Until 2026-11-01 the base was `517b5e10f` (upstream `main` of 2026-10-08): that set, its `next/` variants and the
  sections of the patches dropped since (0001, 0004, 0012; 0014 on 2026-11-04) are in git history (`git show 7618e0a1:hermes/patches/`).

**Upstream.** 0005 is proposed as NousResearch/hermes-agent#136660 (its head is written for main's later browser
modules; the patch here is the same behaviour on v0.22.5's). Candidates for small upstream PRs: 0011, 0019's
labels, 0008's fall-back fix, 0017, 0009's `!name`, 0013.

| Patch | Files touched (v0.22.5) |
|---|---|
| 0005 vault fills after the browser restarted | `tools/browser_vault_tool.py` |
| 0006 synchronous peer reply | `tools/bot_mode_dm.py` |
| 0007 `browser.vault_only` | `model_tools.py` |
| 0008 time zone, routines, wall clock | `hermes_time.py`, `cron/jobs.py`, `tools/environments/base.py`, `hermes_cli/config_defaults.py` |
| 0009 tool search `!name`, compact skill categories | `tools/tool_search.py`, `agent/prompt_builder.py` |
| 0010 empty replies: changed retries, tool calls written as text | `agent/leaked_tool_calls.py` (new), `agent/turn_*.py`, `agent/chat_completion_helpers.py` |
| 0011 titles with GLM: thinking floored to low | `agent/auxiliary_reasoning_floor.py` |
| 0013 MCP results: vault passwords scrubbed | `tools/mcp_tool_handlers.py` |
| 0015 time anchor: the time with each message | `agent/prompt_builder.py`, `agent/system_prompt.py`, `cron/scheduler_prompt.py`, `hermes_state_common.py` |
| 0016 one `message_agent`, the bridge's (0012 folded in) | `tools/bot_mode_dm.py`, `tools/mcp_tool_agent.py` |
| 0017 address items with name, phone, e-mail | `agent/vault_store.py`, `agent/vault_login_classifier.py` |
| 0018 guest turns: closed tools, no USER.md | `agent/guest_turn.py` (new), `agent/*`, `gateway/platforms/api_server*.py`, `gateway/session_api_turn.py`, `gateway/session_execution.py`, `gateway/run_turn_runner.py` |
| 0019 routines report to their conversation | `tools/tool_labels.py`, `cron/jobs.py`, `tools/cronjob_*.py`, `gateway/platforms/api_server.py` |
| 0020 Runs API: each model call's usage (`model.usage`) | `agent/turn_usage.py`, `gateway/platforms/api_server_runs.py`, `gateway/session_api_turn.py`, `gateway/run_turn_runner.py` |
| 0021 an API client's own Bot Chat stays an API conversation | `gateway/platforms/api_server_bot_chat.py`, `gateway/session_bot.py` |
| 0022 tools in one canonical order (`tools.order_first`) | `tools/tool_order.py` (new), `model_tools.py`, `tools/mcp_tool_agent.py` |
| 0023 a system prompt stable across conversations (prefix cache) | `agent/prompt_cache_notes.py` (new), `agent/system_prompt.py`, `agent/turn_context.py`, `agent/agent_init.py`, `tools/memory_tool_store.py` |
| 0024 tools refused for one turn (`model_options.closed_tools`) | `agent/closed_tools.py` (new), `agent/agent_runtime_helpers.py`, `agent/tool_executor.py`, `gateway/platforms/api_server.py`, `gateway/run_turn_runner.py` |
| 0025 a colon mid-sentence no longer holds back a streamed reply | `agent/chat_completion_helpers.py` |

## How the patches are kept

**They are exported from a git branch** (`scripts/hermes-patches.sh`), not edited by hand: the series is a branch of
commits on upstream Hermes at `BASE`, one commit per patch, its message the patch's title and a `Bip-Patch: <file
name>` trailer, which names the file (stable numbers: a dropped patch leaves a gap). In any clone of
NousResearch/hermes-agent (`git clone --filter=blob:none` is enough):

```sh
scripts/hermes-patches.sh branch  ~/src/hermes hermes/patches                    # bip/patches-v0.22.5 from the files
# … fix, test, commit (git commit --fixup=<commit> + git rebase -i --autosquash for a change to one patch) …
scripts/hermes-patches.sh export  ~/src/hermes bip/patches-v0.22.5 hermes/patches
scripts/hermes-patches.sh check   ~/src/hermes hermes/patches                    # the installer's own apply, on BASE
```

A move to a newer Hermes: `branch`, `git rebase --onto <new base> <old base> <branch>` (resolve, run the touched
modules' tests), `export <hermes> <branch> hermes/patches <new base>` (BASE rewritten, files of dropped patches
removed), the installer's `HERMES_COMMIT` moved, and this README. `export` writes `git format-patch` files with fixed
settings (`--full-index`, `--zero-commit`, no signature): the same commits give the same bytes (the hash of these files
is the version id of the servers' shared Hermes).

**How the installer puts them on.** Each patch as is (`git apply`); one that no longer applies (a Hermes updated past
`BASE` with `hermes-update`) is merged with `git apply --3way` from the full blob ids its index lines carry, guarded
(`hermes.apply_3way`): `--3way --check` reports success on a merge with conflicts, and `--3way` works through the
index, so the installer stages the touched files, merges for real, puts them back as they were on a conflict or when
nothing changed, and leaves the index as a plain `git apply` does. On `BASE` the fallback never runs. A patch also
reverse-checks alone on the fully patched tree, so a second run sees it as already there. `hermes-update` runs
`hermes update --keep-stash`: whatever is still changed in Hermes' code after our patches came off (a patch of an
older BipAgents) stays parked in `git stash` instead of coming back on the new version.

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

## 0009 — tool search: some MCP tools kept direct, rarely needed skill categories by name only

**Problem.** Measured on 2026-10-24 (scripts/token-lab): each model call carried ≈ 19.3k input tokens, 11.8k of them the
schemas of the Hermes core tools, and 40 of 126 tool calls were `tool_search`/`tool_describe` round-trips (a full call
each) before the agent could use one of the bridge's own tools. `tools.tool_search.defer` (517b5e10f) can put a core
tool behind the search, but every MCP tool always goes there: an agent can't keep the 5–8 bridge tools it uses all the
time in view. And the skills index lists the description of every bundled skill (≈ 1.5k tokens), coding agents' and
music generation's included, with no setting to shorten it short of disabling skills (which `skill_view` then refuses).

**What the patch does.**
- `tools.tool_search.defer` accepts `!<name>`: that tool stays direct even if it is an MCP or plugin tool. A list
  without such entries behaves as before.
- `skills.compact_categories` (a list of top-level categories): those categories are listed by name only in the
  system prompt, as the coding posture already does for its own; their skills still load with `skill_view`.

- No count in `tool_search`'s description (« Search additional tools… », was « Search 83 additional tools ») nor in
  the catalog's group headings (« bipagents tools: », was « (36) »): the count changed with the kind of session and
  the MCP servers' state (82/83/84 on 3 November), and each change cost the prompt cache everything after that tool,
  the whole system prompt (≈ 11k tokens). A tool more or less still changes the listing from its line on.

**Files.** `tools/tool_search.py`, `tools/tool_search_catalog.py`, `agent/prompt_builder.py`.

**Bridge side** (bridge-rs/src/tool_diet.rs). Per kind of agent, the bridge writes the defer list (Hermes' defaults, the
core tools the kind rarely needs, `!mcp__bipagents__<tool>` for its most used cheap tools) and the compact categories.
Without the patch the `!` entries match nothing and the skills index stays whole: the core tools are still deferred,
the bridge tools stay behind the search, as before.

## 0010 — empty replies: tool calls written as text become calls, retries that change the request

**Problem.** Measured on 2026-10-27 with DeepSeek-V4.1-Flash on the person's own OpenAI-compatible server (Wellness on
aibox, then reproduced with real Hermes against the same server): an agent whose history holds many direct calls to a
tool that the token diet (0009 + the bridge) has since put behind `tool_search` keeps calling it directly, in its native
markup (DeepSeek DSML: `<｜DSML｜ invoke name="mcp__bipagents__health_summary">…`). The server only turns calls to the
request's tools into `tool_calls`: the markup comes back as plain `content` (streamed with thinking on, or not streamed)
or is swallowed (streamed without thinking: `"\n\n"`, 51 completion tokens). Hermes saw an empty answer and its ladder
resent the same request (nudge, prefills, three empty retries); the server is deterministic, so each came back the same
and the person got « ⚠️ No reply: dgx-spark didn't produce a reply this time, even after retries… /model ». With thinking
on, the same ladder also resends a request whose thinking already ate the turn (a prefill ending on an assistant row
comes back reasoning-only).

**What the patch does.**
- A reply without `tool_calls` that is nothing but tool-call markup in a known native format (DeepSeek DSML, DeepSeek
  V3 `<｜tool▁call▁begin｜>…`, Hermes/Qwen `<tool_call>{…}</tool_call>`) becomes real calls
  (`agent/leaked_tool_calls.py`), within strict limits (security audit 2026-10-27, finding 12: markup in a page the
  agent read must never run):
  - only for a custom OpenAI-compatible server (`provider == custom`) or a model whose name contains `deepseek`
    (`RECOVERY_PROVIDERS` / `RECOVERY_MODEL_MARKERS`, a small explicit list); every other provider is left alone;
  - only when the model's own assistant message, `<think>`/reasoning blocks and whitespace aside, is ONLY that markup:
    any prose around it, a code fence or a quote mark, and nothing is run; tool results and other roles never are;
  - only exact names offered in this request: direct when the tool is in the request, else through `tool_call` when
    it is in the session's deferred set (same scope check, approvals and hooks as any deferred call). A suffix match
    (a dropped MCP server prefix) or an unknown name is dropped, never dispatched; when nothing is left the reply is
    empty and the changed empty retries below point the model at the listed tools and `tool_search`.
- A streamed empty reply whose tokens never arrived (completion tokens > 0, no content, reasoning or tool call) is asked
  again once without streaming, so the server's held-back text reaches the parser.
- Every recovery call (post-tool nudge, thinking prefill, empty retry) goes out with thinking off: Hermes' one-shot
  reasoning-off override (`reasoning_effort: none` / `reasoning: {enabled: false}`), and for a custom server on a local
  address (or one already sent template kwargs) `chat_template_kwargs: {enable_thinking: false, thinking: false}`.
- The empty retries never resend the same request (after the person's own message too: an assistant `(empty)` row
  then the hint, never user → user): the first ends with « answer now in plain text, or call a listed tool
  (others: tool_search, then tool_call) », the second (the last) asks for plain text and goes without tools; then no
  more (the deterministic-empty guard no longer cuts them short, a reduced cost budget makes the one retry the last).
- Logs, never content: the empty reply's shape (message and stream-delta field names with sizes, content vs visible
  length, the tag names in it, tokens, finish reason, stream, thinking, last roles, last tool names) and « Tokens
  generated but not surfaced »; « Tool call written as text turned into N real call(s) »; « Answered without thinking
  after an empty or thinking-only response ».

**Files.** `agent/leaked_tool_calls.py` (new), `agent/turn_response_intake.py`, `agent/turn_empty_response.py`,
`agent/turn_api_request.py`, `agent/turn_api_call.py`, `agent/chat_completion_helpers.py` (stream delta fields),
`agent/turn_final_response.py`, `agent/turn_context.py`; new `tests/agent/test_empty_response_answers_without_thinking.py`
(18 tests); `tests/agent/test_run_agent.py`: three empty-retry tests now expect 1 call + 2 changed retries.

**Tests.** The new file, `tests/agent/test_run_agent.py` and the empty-response, prefill, guard, fallback and reasoning-off suites: 366
passed (Python 3.14). The whole `tests/agent/` has the same failures with and without the patch (environment: no
Anthropic SDK, provider snapshots). End to end with real Hermes against DeepSeek on the person's server
(a conversation with four direct calls to a health tool, then the tool deferred and the session's frozen tool list
cleared): before, 6 turns out of 6 failed (4 « No reply », 2 with the raw DSML markup as the answer); after, 6 out of 6
answered with the tool's data, through `tool_call`. The same with the tool gone altogether (not even behind the
search): before 2 of 2 failed, after 4 of 4 answered (the model, told the tool is not there, says it cannot read the
data). That last run predates the 2026-10-28 restriction: an unknown name is now dropped instead of called, so the
model is pointed at the listed tools by the changed empty retry rather than by the unknown-tool answer (not re-run end
to end yet).

Restriction of 2026-10-28 (audit finding 12): 6 tests in the new file (markup alone from a custom server or a DeepSeek
model recovered; the same markup in prose, fenced, quoted, in a tool result or from another provider not; unknown and
suffix names dropped) — 4 of them fail on the previous version of the patch. With it, the new file,
`tests/agent/test_run_agent.py` and the dropped-call, empty-response, prefill and tool-call repair suites: 373 passed
(Python 3.14); the whole series 0001…0010 passes `git apply --check` on `517b5e10f`.

**Bridge side.** The bridge keeps direct every tool an agent has been calling (tool_diet.rs), so the trigger should not
come back; this patch is the net under it.

## 0011 — conversation titles with GLM: never thinking « none »

**Problem.** Hermes titles a conversation with one small call made with thinking off (`reasoning_config={"enabled":
False}`, `agent/title_generator.py`). GLM through EUrouter accepts that request but answers with empty content, so
every conversation of an offer agent (GLM-5.3 Flash) stayed untitled. Hermes already lifts thinking-off aux calls to
`low` for routes that refuse the disable (`agent/auxiliary_reasoning_floor.py`), but it learns that from a 400, and GLM
never sends one.

**What the patch does.** `known_reasoning_floor` also floors up front a model whose name (last path segment) starts
with `glm`: its thinking-off aux calls (the title, and any other aux lane that disables thinking through
`_build_call_kwargs`) go out at `reasoning: low`. Other models are untouched; an enabled config (the agent's own
level) is never changed. Hermes-side because the bridge doesn't make titles (it only reads them).

**Files.** `agent/auxiliary_reasoning_floor.py`, `tests/agent/test_auxiliary_reasoning_floor.py` (1 new test).

**Tests.** `tests/agent/test_auxiliary_reasoning_floor.py` and `tests/agent/test_title_generator.py`: 98 passed, 1
failed with and without the patch (`test_marker_prefix_matches_gateway_constant`, Python 3.12 typing in the test
environment). `git apply --check` passes on `517b5e10f` (no other patch touches these files) and on `97a9ae8ee`.
Not yet run end to end against EUrouter.

## 0013 — MCP tool results: vault passwords scrubbed

**Problem.** Hermes registers a password with its redaction boundary when `browser_vault_fill` types it, and scrubs
it from its own `browser_*` results. An agent with its visible browser browses with an MCP server instead (`stealth`,
patch 0007): a page script run there (`execute_script`) reading the field the vault just filled sent the password to
the model untouched. A login the app's bridge typed when creating an account (bridge-rs/src/vault_logins.rs) was never
registered at all.

**What the patch does.** Every MCP tool result, before it reaches the model, goes through the same exact-bytes scrub
(`«redacted-vault-secret»`), against the registered values plus every login password of the profile's vault (read
again only when the vault file changes), each also in its JSON-escaped forms (a server's JSON text block inside the
handler's own JSON). Text without a vault password is untouched, so what the model sees only changes when it would have
seen a password.

**Limit.** Exact bytes only: a script that transforms the value (reversed, encoded, split) still gets something out.
Hermes' vault tools tell the agent never to type a password itself, the bridge's account tools never to read a
password field back; this is the net under those rules, not a wall against an agent that wants the password
(browser/README.md, Limits).

**Files.** `tools/mcp_tool_handlers.py`; new `tests/tools/test_mcp_vault_scrub.py` (3 tests).

**Tests.** The new file (3 passed; its handler test fails without the patch) with `test_mcp_tool.py`,
`test_mcp_structured_content.py`, `test_mcp_image_content.py`, `test_mcp_empty_error_message.py`: 137 passed, 5 failed
for a missing Pillow in the test environment (`test_mcp_image_content.py`'s native image tests), Python 3.12. No other
patch touches the file; `patch --dry-run` passes on `517b5e10f` and `97a9ae8ee`.

## 0015 — the current time with each message, never from the terminal

**Problem.** Agents got the date wrong in conversations that last days (Alex, 2026-10-29: « C'est super important »).
Hermes' system prompt carries « Conversation started: … » and, once rebuilt on a later day, « Today's date (as of the
last context rebuild): … »; the prompt is only rebuilt at a compaction and the model server's cache keeps it, so that
date goes stale. Its execution guidance also tells the model « Current time, date, timezone → use terminal (e.g.
date) » and « 'What time is it?' → run `date` »: a tool call each time (slow, costly) while the stale date still wins,
and servers run in UTC.

**What the patch does.** (a) The two terminal-for-time lines leave the execution guidance (and its line → tools
map). (b) The « as of the last context rebuild » line is replaced by one constant line, `NOW_LINE_HINT`: when a
message starts with a `bip:now` line, that line is the current date and time, trust the latest one, don't check the
clock with a tool; without one (a messenger's or the CLI's turn on the same Hermes: no bridge put it), check the clock
with a tool when the exact time matters (review 2026-11-03, H8). It never changes, so the prompt stays byte-stable for
the whole conversation. (c) Each routine run's prompt gets
the same line after the cron hint, in the routines' zone (`cron.timezone` of patch 0008, else the profile's), e.g.
`<!--bip:now Sunday, October 25, 2026, 02:30, Europe/Paris (UTC+01:00)-->`. The conversations' lines come from the
bridge and the apps (bridge-rs/src/dates/now.rs): in the new user message, never in the system prompt, so the cache
holds; the apps hide every `<!--bip:…-->` note. (d) A session's preview (the head of its first user message, 60
characters) skips a leading now line, else every conversation's preview would be the line.

**Files.** `agent/prompt_builder.py`, `agent/system_prompt.py`, `cron/scheduler_prompt.py` (`now_line`),
`hermes_state_common.py` (`_PREVIEW_CONTENT_SQL`), `tests/agent/test_system_prompt.py` (the two tests that expected
the old line); new `tests/agent/test_now_line.py` (7 tests: the routine's zone while the profile's is another, both
passes of the repeated hour on 2026-10-25 in Europe/Paris, before and after the jump on 2026-03-08 in
America/New_York, no terminal-time hint, the preview).

**Tests.** The new file with `test_system_prompt.py`, `test_system_prompt_switch_invalidation.py`,
`test_prompt_builder.py`, `test_cron_wall_clock_once_and_zone.py`, `test_cron_prompt_injection.py`,
`test_scheduler.py` and the other cron prompt tests (`test_prompt_cache_boundary.py`, `test_notepad.py`,
`test_cron_context_from.py`, `test_cron_script.py`, …), Python 3.12, all patches applied: the same failures as
without it (missing `requests` and other packages in the test environment), the 7 new tests pass. `git apply --check`
passes after 0001–0014, `patch --dry-run` on upstream `main` (`8acb7f6580`). An upstream PR candidate: there the
marker would be neutral (`bip:` is BipAgents' notes prefix) and the hint shown only when a client stamps its
messages.

## 0016 — `message_agent`: the bridge's, even in the Bot Chat

*0012 (« the Bot Chat shows one `message_agent` »: the MCP one dropped where the built-in is offered) is folded in
since v0.22.5: what is left is 0016's rule, and 0012's unused `drop_mcp_message_agent` is gone.*

**Problem.** Since 2026-10-29 the bridge's `message_agent` (`mcp__bipagents__message_agent`) asks the other agent in
the two agents' own conversation « A ⇄ B » (bridge-rs/src/agent_pairs.rs), not in the target's « Bot Chat », the
person's main conversation with it (Alex: « Les échanges entre 2 bots dans une conversation à part »). Hermes'
built-in still writes into the target's Bot Chat (through the bridge's peer routes, `hermes peer dm`), and 0012 kept
the built-in in the Bot Chat: an agent asking from its own Bot Chat still filled the other one's.

**What the patch does.** The other way round from 0012, on top of it: where the built-in would be injected
(`ensure_message_agent_tool`, once per turn, and `_reinject_authorized_dynamic_tools` on an MCP rebuild) and an MCP
tool named `mcp__<server>__message_agent` is in the staged list, the built-in is not added, and a copy added before
the MCP tools were listed leaves the list and `valid_tool_names`. One way to write to an agent everywhere, the
bridge's, and every exchange in its pair's conversation. Without such an MCP tool (an install without the bridge, an
agent linked to nobody) the built-in is injected as upstream. Deterministic for the session, so the prompt cache
holds. The bridge's tool behind the tool search counts too (`mcp_message_agent_offered`: the deferred names of the
session, when `tool_call` is there): on 3 November Prof's bridge `message_agent` was deferred, its Bot Chat got the
built-in and its other conversations had none (7.8k tokens of cache lost going from one to the other).
0012's `drop_mcp_message_agent` stays, unused; a separate patch so an install that has 0012 gets this one as a new fix
(`hermes-update --fixes-only`).

**Files.** `tools/bot_mode_dm.py`, `tools/mcp_tool_agent.py`; 0012's `tests/tools/test_bot_mode_dm_one_message_agent.py`
rewritten for the new rule (8 tests, one for a deferred bridge tool).

**Tests.** `patch --dry-run` after 0006 and 0012 on `517b5e10f`; the files compile (`py_compile`). The test file was
not run on the Mac (no Hermes test environment there): run it with `tests/tools/test_bot_mode_dm.py`,
`test_bot_mode_dm_sync_peer.py`, `test_refresh_agent_mcp_tools.py` and `test_bot_mode_probe.py` before the release.
The bridge's side: black-box `test_agent_messages.py` and `test_agent_pairs.py`.
## 0017 — address items carry a name, a phone and an e-mail

**Problem.** BipAgents keeps « Coordonnées » in its vault (bridge-rs/src/vault_contacts.rs): a full name, a postal
address, phones and e-mails. Hermes' `address` items keep the six address fields only, drop any other, and refuse an item
without street, city, postal code and country: a form's name, phone and e-mail fields stayed empty, and an entry with
a name and an e-mail only could not reach the agent's browser at all.

**What the patch does.** `ADDRESS_FIELDS` also maps `full_name` → `name`, `phone` → `tel` and `email` → `email`
(WHATWG autocomplete tokens); an address item needs at least one field, none in particular. The checkout classifier
targets those tokens (by autocomplete, an `<input type=email>`, labels such as « E-mail », « Téléphone », « Nom
complet », and an `<input type=tel>` after the other heuristics, since a card number is often one). A payment item has
none of these fields, so a card fill never types into them. The bridge sends one phone and one e-mail (the first of
each) and the country as its ISO code (a country `<select>` matches it by its option values).

**Files.** `agent/vault_store.py`, `agent/vault_login_classifier.py`; 2 tests in `tests/tools/test_browser_vault.py`.

**Tests.** `tests/tools/test_browser_vault.py` and `tests/agent/test_vault_store.py` on the three files of
`517b5e10f` with the patch, Python 3.12: the 2 new tests pass, the other results are the same as without the patch
(22 failures for modules missing from that minimal test tree: `tools.registry`, `agent.vault_backends`…). No other
patch touches these files; `patch --dry-run` passes on `517b5e10f`, `97a9ae8ee` and upstream `main` (`0b4481ad9c`).
Without it, the bridge still keeps and reads the entry (its handle `bridge` when Hermes refuses it), and the browser
fills the address fields only.

## 0018 — guest turns: only the guest's tools, no USER.md

**Problem.** BipAgents lets someone other than an agent's person talk to it: a relative with an agent shared with
them, a member with the family's agent, another household's agent, a relative's agent in a discussion
(bridge-rs/src/agent_shares.rs, family_agent.rs, peers.rs, rooms_remote.rs). The bridge refuses its own MCP tools that
reach the person's data during such a turn, but Hermes' own tools (terminal, files, browser and its vault fill, code
execution, `session_search` over the person's past conversations, memory writes, routines, `send_message` /
`message_agent`, delegation, skills) were closed by instructions only, and Hermes put the person's profile (USER.md)
in every turn's system prompt. « Pas tes secrets » has to hold by permissions.

**What the patch does.** A `POST /v1/runs` or `POST /api/sessions/{id}/chat[/stream]` body may carry
`guest_turn: {"tools": [names]}` (new `agent/guest_turn.py`). For that turn only:
- **tools**: the agent built for the turn keeps the listed MCP tools (`mcp__<server>__<tool>`) and the listed Hermes
  tools of `NATIVE_TOOLS` (`web_search` only), each in view even when the tool search defers it (raw catalog,
  `skip_tool_search_assembly`); any other name in the mark is dropped, never opened. The list is put back after the
  per-turn `message_agent` injection and before the system prompt is built (`narrow`), and every model request sends
  only those tools (`build_api_kwargs`), whatever a pinned `tools[]` merge or an MCP refresh added;
- **dispatcher**: any other tool a model names anyway is refused (`{"error": …closed during this turn…}`) in Hermes'
  policy gate (`tool_executor._dispatch_authorized_once`, before plugin hooks, guardrails and dispatch: both the
  sequential and the concurrent path) and in `invoke_tool`;
- **prompt**: USER.md is left out (`_user_profile_enabled = False`), the working directory's context files too
  (`skip_context_files`, with `load_soul_identity` so SOUL.md, the personality, stays); MEMORY.md stays. The prompt
  is built for the turn and never stored on the session, nor the session's tool pin restored or written: the
  person's own next turn in that session gets their usual prompt and tools back;
- **memory**: no memory or skill nudge, no background review, no external memory provider (not checked out for the
  session, shut down if the config built one): nothing the guest says is written into the agent's memory or skills;
- a guest turn runs in this process, never handed to a Desktop holding the Bot Chat live;
- a malformed mark fails the request (400 `invalid_guest_turn`): a client asking for a closed turn never gets an
  open one. Without the mark nothing changes.

**Limits.** The turn still sees its session's history (a relative's own conversation; for another household's agent,
the target's Bot Chat, which is how Hermes peers talk) and MEMORY.md, common by decision 3A (2026-10-25). A compaction
during a guest turn stores the guest's prompt on the rotated session until its next rebuild (only the household Bot
Chat case mixes guest and owner turns). `/v1/chat/completions` and `/v1/responses` ignore the mark (the bridge never
sends guest turns there).

**Files.** `agent/guest_turn.py` (new), `agent/chat_completion_helpers.py`, `agent/tool_executor.py`,
`agent/agent_runtime_helpers.py`, `agent/conversation_loop.py`, `agent/turn_context.py`,
`gateway/platforms/api_server.py`, `gateway/platforms/api_server_runs.py`; new `tests/agent/test_guest_turn.py`
(17 tests) and `tests/gateway/test_api_server_guest_turn.py` (9 tests).

**Tests.** Run on the Mac (2026-10-29, Python 3.14, `uv sync --frozen --group dev --extra messaging` on `517b5e10f`
with 0001–0018): the two new files, 26 passed; with `test_system_prompt*.py`, `test_turn_context*.py`,
`test_tool_executor_checkpoint_paths.py`, `test_inline_tool_executors_memory_args.py`,
`test_chat_completion_helpers_provider_sort.py`, `tests/gateway/test_api_server_runs.py`,
`test_session_chat_approval.py`: 241 passed; the dispatch and Bot Mode suites (`test_interrupt.py`,
`test_concurrent_interrupt.py`, `test_sequential_tool_timeout.py`, `test_start_order_gate.py`, `test_shell_hooks.py`,
`test_memory_provider.py`, `test_tool_call_incremental_persistence.py`, `test_tool_result_memory_trim.py`,
`test_interrupt_issuer_attribution.py`, `test_bot_mode_dm*.py` incl. 0016's, `test_refresh_agent_mcp_tools.py`,
`test_bot_mode_probe.py`): 264 passed, 15 skipped, the same with and without the patch. The bridge's side:
`agent_shares.rs`, `peers.rs` unit tests and the black-box `test_shared_agents.py`, `test_family_agent.py`,
`test_rooms_guests.py`, `test_households_and_peers.py` (the mark on each path). The real-model check is in
internal/next/relative-turns.md.

## 0019 — routines made in a conversation report to it; plain results; longer instructions; labels for `calls` as text

**Problem.** Seen 2026-10-31: Alex asked his agent, in the app, « mets un schedule ce soir à 23h pour corriger tout le
carnet ». Three things went wrong on Hermes' side.
- The routine tool (`cronjob_manage`, behind the tool search) went through `tool_call`. The dispatcher runs a `calls`
  sent as a JSON string or as one object (`normalize_tool_call_entries`), but `tools/tool_labels.py` read only a list:
  the `tool.started` preview, which the apps show, was « Unnamed call ».
- A Runs API session has no push channel, so `_origin_from_env` drops its origin and an omitted `deliver` makes a
  local-only job. The create result then told the model « This is a local-only cron job … recreate or update the job
  with deliver set to … deliver='telegram' or deliver='all' ». The model did so and told Alex about it (« Le job est
  créé mais en livraison locale uniquement »), though the bridge delivers every routine's report anyway.
- A job's instructions had two limits: none in the tool, 5 000 characters in the Jobs API (`/api/jobs`, which the
  apps' Routines use to edit a job the agent made). 5 000 is short for a long multi-step instruction.

**What the patch does.**
- `labels_for_call` reads a `tool_call`'s entries in every shape the dispatcher runs: a list, a JSON string of one, a
  single object, the legacy `{name, arguments}`, with `arguments` as a JSON string too. An entry with no name keeps
  « Unnamed call » (the apps word that as a plain tool call).
- `_origin_session_from_env`: in an api_server session that resumes server history
  (`session_history_delivery_supported()`, the declaration async `delegate_task` results rely on), with
  `cron.session_reports: true` in the profile's config (the bridge writes it, tool_diet.rs), a job created with no
  `deliver` or with `origin` gets the session id as `origin_session_id`. `deliver` stays `local`: Hermes posts
  nothing, the bridge's cron watcher posts the run's report in that conversation. The create result says « Its report
  comes back to this conversation each time it runs. », and `deliver` reads `this conversation` in the create, update
  and list results, with no « local-only » notice; an update asking for `origin` again keeps it so. Without the
  config key, or in any other session, nothing changes.
- One limit for a job's instructions, `cron.jobs.MAX_PROMPT_CHARS = 20_000` characters. A long multi-step instruction
  (what to go through, the rules, what to check, how to report) is 2 000 to 8 000 characters; 20 000 (about 5 000
  tokens) leaves room for that and more, and still refuses a pasted document, which belongs in a file the job reads.
  The cronjob tool now checks it on create and update, with a plain error (« These instructions are too long for a
  scheduled task (N characters, at most 20,000) … Nothing was saved. »); the Jobs API checks the same number.

**Files.** `tools/tool_labels.py`, `cron/jobs.py`, `tools/cronjob_job_args.py`, `tools/cronjob_tools.py`,
`gateway/platforms/api_server.py`; `tests/gateway/test_api_server_jobs.py` (the API's limit tests at 20 001
characters); new `tests/tools/test_cronjob_session_reports.py` (12 tests; 6 fail without the patch: a stringified
`calls` labelled « Unnamed call », no origin session, a 20 001-character prompt accepted).

**Tests.** On the Mac (2026-11-01, Python 3.14, `517b5e10f` with 0001–0019): the new file with
`tests/tools/test_cron*.py`, `test_cronjob*.py`, `test_delegate_cron_sync_fallback.py`, `tests/agent/test_display.py`,
`tests/gateway/test_api_server_jobs.py` and `tests/cron/`: 1688 passed, 11 skipped. The bridge's side: `push-runs_cronwatch.rs`
(a routine's report in the conversation it was made in, or under its run when that one is gone) and `tool_diet.rs`.

## 0020 — the Runs API reports each model call's usage as it ends (`model.usage`)

**Problem.** A run on the Runs API gives its token usage once, in `run.completed` (and the run's status), and none at
all for a run stopped before its end. The bridge caps each agent's turn in a room's team work at `[rooms]
team_turn_tokens` (200k), so it estimated a turn's tokens from its tool steps (its earlier turns' tokens per call, 20k
at least). In the rooms bench of 2026-11-01 a DeepSeek turn reached 249k in 5 tool steps: after big file reads each
call read about 41k, and the estimate never reached the cap.

**What the patch does.**
- `agent/turn_usage.py` (`record_response_usage`, the one place every model call's usage goes through): once the
  session counters hold a call, its provider usage (the dict already built for the context engines) goes to
  `agent.call_usage_callback` when one is set. A response without usage reports nothing (no guess); a failing
  callback is logged at debug level and never touches the turn. Nothing else sets the callback: other surfaces see
  no change.
- `gateway/platforms/api_server_runs.py`: a run of `POST /v1/runs` sets that callback, and each call becomes a
  `model.usage` event on the run's stream, before the tool events of that call:

  ```json
  {"event": "model.usage", "run_id": "…", "timestamp": 1793000000.0, "call": 3,
   "input_tokens": 43500, "output_tokens": 1200, "cache_read_tokens": 40900, "cache_write_tokens": 0,
   "reasoning_tokens": 0,
   "totals": {"input_tokens": 125000, "output_tokens": 2100, "total_tokens": 127100,
              "cache_read_tokens": 110000, "cache_write_tokens": 0, "reasoning_tokens": 700}}
  ```

  `call` is the agent's API call count (`API call #n` of its log line, usage-less calls included); `input_tokens` is
  the whole prompt, cache reads included, as in the terminal `usage`; `totals` are the run's so far, in the terminal
  `usage` shape plus `reasoning_tokens` (the run's agent is made for the run, so they start at 0). The apps ignore
  the event; the bridge's run hub (runs.rs) keeps the latest totals and the last call's input.

**Files.** `agent/turn_usage.py`, `gateway/platforms/api_server_runs.py`; new
`tests/gateway/test_api_server_runs_model_usage.py` (7 tests: the callback after the counters, nothing without usage,
a failing callback, the payload, the run event, a run streaming two `model.usage` before `run.completed` with
matching totals).

**Tests.** On the Mac (2026-11-01, Python 3.14, `517b5e10f` with 0001–0020): the new file, every
`tests/gateway/test_api_server*.py` and the agent tests that touch usage or the conversation loop (95 files): 1998
passed, 6 skipped, 1 failed (`tests/agent/test_plugin_prompt_sections.py`, order-dependent: it passes alone, with and
without 0020). The bridge's side: rooms.rs (`turn_limit`, the turn loop), runs.rs (`RunUsage`, `cap`), jobs.rs
(`live_usage`) and `tests/agents_rooms_relay.rs`.

## 0021 — an API client's own « Bot Chat » stays an API conversation

**Problem.** v0.22.5 (session authority) treats the profile's conversation titled « Bot Chat » as Bot Mode's canonical
chat, a LOCAL session whose only writer is the authority FIFO. Every text turn into it through the API doors
(`POST /v1/runs`, `POST /api/sessions/{id}/chat[/stream]`, `_admit_to_live_bot_chat` → `admit_peer_turn`) is admitted
to that FIFO as a peer's message, and a Bot Chat made by a plain `POST /api/sessions` (an `api_` row) is « a
conversation no FIFO can own »: the first such turn moves it aside as « Bot Chat (API <id>) », history included, and
mints a new empty local Bot Chat (`displaceable_bot_chat`, `_create_bot_chat`). The bridge makes each agent's main
conversation exactly that way and runs it through the Runs API. Found by the 2026-11-02 release bench: after the
update every agent's main conversation restarted empty; the run's stream carried only `run.completed` (no text, no
tool events, no `model.usage`); a clarify failed (« la destination a refusé »). A `POST /api/sessions {"title":
"Bot Chat"}` (the installer's, `agents_admin.rs`') even minted the local chat directly (`create_peer_bot_chat`), so a
fresh agent's main conversation was the FIFO's from the start. Side conversations and photo turns (a non-string
`input`) were not affected.

**What the patch does.** A Bot Chat made by a plain API create (`source` `api_server`, no local chat in its lineage:
`session_bot.api_bot_chat`) is the API client's conversation:
- `create_peer_bot_chat` leaves a create without a source (or `api_server`) to the ordinary handler: an API row. A
  peer's own create (`hermes peer dm` sends `source: bot_peer_dm`) still mints the local chat;
- `displaceable_bot_chat` never names it: no door moves it aside;
- `admit_peer_turn` leaves its turns to the API doors (None), which run them as API turns, as before session
  authority: streamed text, tool events, `model.usage`, approvals, clarify. A peer's turn sent through those doors
  runs there too (BipAgents sends agent-to-agent messages to the pairs' own conversations anyway: agent_pairs.rs);
- the local doors (`bot_relay.deliver`, a cron `bot-chat` delivery) are refused for it instead of moving it.
Every other Bot Chat (local, CLI/Desktop history, messaging rows, rows a peer minted through the API) is unchanged.

**Upstream.** Worth proposing as a profile setting (an API client that owns its Bot Chat, e.g.
`bot_mode.api_owned_bot_chat`), since upstream moves API rows aside on purpose (MSG-21, #140769) for Desktop users.

**Files.** `gateway/platforms/api_server_bot_chat.py`, `gateway/session_bot.py`; new
`tests/gateway/test_api_bot_chat_stays_api.py` (6 tests, 5 fail without the patch); in
`tests/gateway/test_peer_dm_bot_chat_headless.py` the moved-aside rows are a peer's (`bot_peer_dm`).

**Tests** (2026-11-02, Python 3.12, `uv sync --frozen --group dev` on `bf0203a794` with the set): the new file, every
Bot Chat / peer DM / authority-runs suite of `tests/gateway` (`test_peer_dm_*`, `test_bot_authority`,
`test_bot_chat_*`, `test_*bot_chat*`, `test_api_authority_runs`, `test_api_runs_native_session_refusal`,
`test_owner_loop_writes_*`), `tests/tui_gateway/test_profiles_list_canonical_session.py`,
`tests/hermes_cli/test_peer_cmd.py`: all pass. The bridge's black-box suite fakes it (`bbkit/fakes.py`: a local
Bot Chat's turns go to a FIFO, an API one's stream). End to end on a lab place (Hetzner cax21, GLM-5.3 Flash,
2026-11-02): installed on the old pin (public 827274a, Hermes 517b5e10f), two turns in an agent's main conversation,
then moved to v0.22.5 with this set and the branch's bridge: the same conversation (same id, 8 messages) kept its
history and answered from it, streamed `message.delta`, `model.usage`, tool events, a clarify (`clarify.request` →
answer → `clarify.settled`), an approval (`approval.request` → once → `approval.settled`) and an agent-to-agent
`message_agent`; no « Bot Chat (API …) » anywhere. A new agent made afterwards got an API Bot Chat (`api_…`,
`api_server`) that streams, runs tools and approvals the same way.

## No patch: `clarify` in the API toolset (2026-11-02)

v0.22.5's `hermes-api-server` toolset leaves `clarify` out (« no interactive UI tools like clarify »), and the lists
`hermes tools enable/disable --platform api_server` saves name only what was on: on an updated place or a new agent the
model had no question tool and the app's question card never came (the old pin's Hermes gave it to API runs; 0001's
Runs API side is now upstream's, its tool was not). Fixed in config, not in Hermes: `platform_toolsets.api_server`
always carries `clarify` (the bridge at each start and for each new agent, `tool_diet.rs` `with_clarify`; the
installer on the hub profile every agent is cloned from and the update migration `2026-11-02-clarify-tool`, both
through `hermes tools enable clarify --platform api_server`). The apps never show that switch.

An existing conversation needs nothing more: the api_server builds a fresh agent for each run from the config
(`_create_agent` → `_get_platform_tools`), and a continuing session's pinned tools[] keeps its order but appends the
tools the fresh build adds (`tools/mcp_tool_agent.py` `restore_agent_tool_prefix`); a Bot Chat also rebuilds its
system prompt once, its capability epoch covering `platform_toolsets` (`tools/bot_mode_probe.py`). A model that said
« I have no clarify tool » earlier in the same conversation may keep saying so from its history.

**Lab** (2026-11-02, Hetzner cax21, GLM-5.3 Flash): installed on public 827274a (Hermes 517b5e10f; every profile's
saved `api_server` list already without `clarify`), one turn in the « daily » agent's main conversation, then moved to
v0.22.5 with this branch's bridge (`lab.py hermes` + `local --commit 6961a432 --no-patches`): the bridge's start added
`clarify` to the four agents' lists (other entries kept); a question card (`clarify.request` → « 14 h » →
`clarify.settled` → « Tu as choisi le train de 14 h. ») in that same main conversation (same id, from before the
upgrade), in a new side conversation, in a second agent's main conversation, and in the main conversation of an agent
made afterwards (a spare taken: `clarify` written when it got its tools).

## Moving to v0.22.5 (2026-11-01)

The series rebased from `517b5e10f` on v0.22.5 (`bf0203a794`), branch `bip/patches-v0.22.5`; the installer pins
v0.22.5 and `hermes-update` is no longer held. Every patch applies with `git apply` on `bf0203a794`, in order, and with
`git apply --3way` (same tree); each reverse-checks alone on the patched tree (0012 did not, under 0016, which is why
`lab.py local` said « 0012 NOT applied (neither way) »). Its « 0010 NOT applied » had another cause: 0010 reverse-checks
on `517b5e10f` with the whole series, so the lab's Hermes carried an older text of 0010 (before the 2026-10-28
restriction), applied when that Hermes was built.

| Patch | On v0.22.5 |
|---|---|
| 0001 clarify in the Runs API | **dropped**: v0.22.5 has its own (`POST /v1/runs/{id}/clarify`, `clarify.request` / `clarify.settled` events, one `answer` string, no batch or cancel). The bridge moves to that contract. |
| 0004 images kept for follow-up questions | **dropped**: upstream #139873 (in v0.22.3: replayed from Hermes' own media copy, swept after 24 h). |
| 0005 vault fills after the browser restarted | **kept** (applies as is). The head of #136660 does not apply: it is written for main's later browser modules (`browser_session*`). Same behaviour as the PR (the configured browser first, resolved afresh); its three tests, adapted to v0.22.5's module names, added to `tests/tools/test_browser_vault.py` (2 fail without the patch). |
| 0006 synchronous peer reply | kept (applies as is). |
| 0007 `browser.vault_only` | kept; v0.22.5 has one vault tool, `browser_vault`, which the filter keeps (`browser_vault*`). New `tests/tools/test_browser_vault_only.py` (3 tests, 2 fail without the patch). |
| 0008 time zone, routines, wall clock | kept (applies as is; v0.22.5 has no per-job time zone yet). |
| 0009 tool search `!name`, compact skill categories | kept (applies as is). New `tests/agent/test_tool_search_keep_direct_compact_skills.py` (3 tests, all fail without the patch). |
| 0010 empty replies | kept (applies as is). |
| 0011 titles with GLM | kept; its test merged beside v0.22.5's new token-plan tests. |
| 0012 + 0016 one `message_agent`, the bridge's | **folded** into `0016-message-agent-the-bridges-everywhere` (0016's behaviour; 0012's unused `drop_mcp_message_agent` removed). No `0012` file. |
| 0013 MCP results: vault passwords scrubbed | kept; docstring names `browser_vault`; its handler test follows v0.22.5's `_call_tool_racing_stdio_death` signature. |
| 0014 routine results in plain UTF-8 | **dropped** 2026-11-04: the bridge keeps Hermes' `cronjob` toolset off (routines run on the bridge). |
| 0015 time anchor | kept **whole, part (c) included**: `[CURRENT RUN TIME]` in routine prompts is on upstream `main`, not in v0.22.5, so part (c) goes at the next base that has it. |
| 0017 address items with name, phone, e-mail | kept (applies as is). |
| 0018 guest turns | **rewritten** for the session authority (below). |
| 0019 routines report to their conversation | kept; one conflict (`NO_AGENT_WITHOUT_SCRIPT_ERROR` moved to `cron/jobs_invariants.py`). Its tests patch `hermes_cli.config` through `sys.modules` (an upstream test swaps the module; they failed in the full run, not alone). |
| 0020 `model.usage` per model call | **ported** (below). |

**0018 on v0.22.5.** A Runs API or session-chat turn no longer builds its agent in the request: it is admitted to the
session's durable FIFO (`admit_api_turn_async` → `session_admissions` row with an `api_turn_v1` payload) and run by
the session authority's `TurnRunner`, which reuses the session's cached agent. So:
- the doors (`POST /v1/runs`, `POST /api/sessions/{id}/chat[/stream]`) parse `guest_turn` as before (400
  `invalid_guest_turn`) and pass the tools to the admission, which stores them in the payload as
  `api_turn_v1.guest_turn = {"tools": [sorted names]}`: per admission, never in the session's saved settings (the
  person's next turn and the session's internal turns are theirs), and part of the exact-retry identity (the same
  Idempotency-Key without the mark, or with other tools, is `admission_conflict`). `check_api_turn` refuses a
  malformed stored mark (`invalid_params`), so a recovered turn never runs open;
- `prepare_api_execution` hands `guest_tools` to the execution; `_resolve_turn_agent` then builds that turn an agent
  of its own (`_build_fresh_agent` + `guest_turn.apply`), never taken from nor put in the session's agent cache (the
  person's cached agent, its prompt, tools and memory provider, are untouched), and `run_sync` releases it after the
  turn like an evicted agent (the session's sandbox, browser and background processes stay);
- the agent-side parts (tool list, dispatcher refusal, prompt without USER.md never stored, no memory writes) are
  unchanged; the direct path (no session authority) keeps the old `_create_agent(guest_tools=…)`; a guest turn is
  never handed to a live Bot Chat owner.
New `tests/gateway/test_api_guest_turn_admission.py` (5 tests: the mark in the admission and only there, retries,
a malformed stored mark, the fresh uncached agent, its release).

**0020 on v0.22.5.** `agent/turn_usage.py` applies as is. A run takes one of two paths on v0.22.5: the direct one
(no session authority: the run's own agent, `_create_agent`), where the callback is set as on `517b5e10f`; and the
session authority's (the canonical path of a running gateway), where the turn's agent is the session's cached one:
`wire_api_observers` sets `call_usage_callback` per turn and publishes `model.usage` fields to the run's observer
(a new `call_usage_callback` observer key, fenced to the running generation like the other sinks), with `totals`
counted from the turn's start (that agent's counters run across turns); `TurnRunner` clears the callback on every
turn. `run.completed`'s `usage` on that path had only `input_tokens`, `output_tokens`, `total_tokens` (the turn's
deltas): it carries the turn's `cache_read_tokens` and `cache_write_tokens` again. 2 new tests in
`tests/gateway/test_api_server_runs_model_usage.py` (both fail without the port).

**Tests** (2026-11-01, Python 3.14, `uv sync --frozen --group dev --extra messaging` on `bf0203a794` with the set):
- each patch's own tests, the new ones above included, and upstream's suites of the modules the set touches:
  `tests/agent`, `tests/tools`, `tests/cron` (one run: 23 240 passed; its 476 failures and 38 errors rerun alone:
  116 fail, the same 116 on unpatched v0.22.5 — missing optional SDKs (fal, Anthropic, Bedrock…), provider snapshots,
  network; the rest were order effects of the long run, `test_fal_sdk.py` does not collect without its SDK); the
  Runs API, session-authority, admission, TurnRunner and agent-cache suites of `tests/gateway` (`test_api_*`,
  `test_session_authority*`, `test_session_api*`, `test_session_ingress*`, `test_session_execution*`,
  `test_run_turn*`, `test_agent_cache*`, `test_bot_authority*`, 101 files): 672 passed, 4 skipped; the rest of
  `tests/gateway` and `tests/hermes_state` (two runs, the first stopped at an upstream test that hangs only in the
  long run, `test_discord_voice_stale_binding.py`, which passes alone with and without the set; the first without
  0020): 3 544 + 10 217 passed; their 17 + 59 failures rerun alone: 2 and 29 fail, the same on unpatched v0.22.5;
- `scripts/hermes-patches.sh check` (the installer's `apply_patches` on a clean worktree at `BASE`), `branch` then
  `export` giving the same bytes, and `git apply --3way` of the whole set on a clean v0.22.5: same tree as the branch;
- `installer/tests`: 304 passed, 3 skipped (new `test_hermes_patches.py`: the patches made on the pinned Hermes, a
  moved context merged with the index left alone, a conflict leaving nothing; `test_installer.py` pins `bf0203a794`
  and puts on the 15 patches; `test_hermes_update.py`: the update runs, with `--keep-stash`, no hold).
Not run end to end against a live gateway and a real model yet: the bridge's side of v0.22.5 and the lab bench come
first.

## 0022 — tools in one canonical order

**Problem.** The tool list heads every request (GLM's template renders it first), and a provider's prompt cache is
shared only as far as two requests are byte-identical. Hermes sorts the tools by name, so two agents differ from the
2nd or 3rd tool on (`browser_vault`, `clarify`, `execute_code`…) and share ≈ 100 tokens: each agent's first call,
and every routine, read nothing from the cache (cache audit of 2026-11-03, #1: ≈ 62k tokens a day for GLM). A tool
that arrives in a live session (an MCP list refresh) is appended, so that session's list is no longer the one a new
session builds.

**What the patch does.** `tools.order_first` (a list of tool names in config.yaml, written by the bridge: the 24 tools
every agent has, ≈ 9.1k tokens) puts those tools first in that order, every other tool after them by name; the tool
search's own tools take their place too. A tool added to a live session (`restore_agent_tool_prefix`,
`_merge_preserving_prefix`) is inserted at its canonical place instead of the tail: one break in that session, then
its list is again the one every new session builds. Without the setting: plain name order, as before.

**Files.** `tools/tool_order.py` (new), `model_tools.py` (`_compute_tool_definitions`), `tools/mcp_tool_agent.py`.

**Tests.** `tests/tools/test_tool_order.py`: common tools first then by name, two agents' common prefix byte-identical,
the same list from any input order (two starts), a later tool at its place.

## 0023 — a system prompt stable across an agent's conversations (prefix cache)

**Problem.** The 3 November cache audit (`scripts/token-lab/cache_breaks.py` on the release lab's dumps): inside a
conversation the provider's prefix cache reads back 99 % of the previous call, but between conversations the start
of the prompt differs. Three sources are in Hermes' system prompt: the Bot Chat's « ## Messaging other agents »
section sat in the middle (after the context files, before the platform hint, skills and memory), so a Bot Chat and
the agent's other conversations shared only the first part; the memory blocks (MEMORY.md, USER.md, which BipAgents
keeps common to the person's agents) were taken at each session's start, so every write changed the end of every
later conversation's prompt, for each agent of the circle on a USER.md write; and the Runs API's `instructions` (a
room's rules, a background task's brief, a Live brief) were appended to the system prompt of each call. With a
template that puts the tools after the system prompt (DeepSeek), a change at its end also loses the ≈ 15k tokens of
tool schemas: measured 57k tokens a day for USER.md, 16k for a room.

**What the patch does.**
- The Bot Mode section (and its capability epoch line) goes to the volatile tier, after the memory and the profile
  line, just before the date line (which the Bot Chat also changes): the runtime block stays last, as
  `surface_switch` expects.
- `memory.prompt_refresh` (`daily` for an agent unless the profile says `session`; a bare `MemoryStore` keeps
  `session`): the system prompt shows the day's copy of the memory (`memories/.prompt-day.json`, written by the
  day's first load in the configured time zone; the live entries the tools use are always the files). What is
  written later reaches the model in a turn note (`agent/prompt_cache_notes.py`, on the per-turn channel of the
  skills-index note: joined to the API copy of the turn's message, persisted as its `api_content`): « Your memory
  changed after this conversation's system prompt was built… Now in your memory too: - USER PROFILE: … / No longer
  in your memory: … ». Each note states the full difference against the prompt (entries parsed from its blocks,
  against the files on disk now: another process may have written them), and a note is sent only when that
  difference is not the newest note of the transcript, so an agent knows in a later conversation of the day a fact
  it stored in an earlier one. No note on a guest turn (0018), threat-flagged entries never told. A compaction
  rebuild takes the day's copy too.
- On `api_server`, the run's instructions (`ephemeral_system_prompt`) leave the system prompt for a turn note
  (« [System: Instructions for this conversation, from the app…] »), sent when they are not the transcript's newest
  instructions note: a room's rules come once and again at a hand-over. A cached agent keeps them on
  `_run_instructions` (they are part of its cache signature). Other platforms keep upstream's behaviour.
  The bridge turns it on (`agent.instructions_as_note`, tool_diet.rs) and sends `instructions` only for a room's
  rules and a read-only share's: background turns (`after_call`'s included) and Live calls carry theirs in the turn's
  own message (a hidden note, the apps' call prelude), so nothing is told twice; the notes come after the message's
  text, so a Live warm-up and the call's first turn still share the prefix up to the prelude.

**Files.** `agent/prompt_cache_notes.py` (new), `agent/system_prompt.py`, `agent/turn_context.py`
(`_merge_gateway_notes`: instructions first, memory last), `agent/agent_init.py`, `tools/memory_tool_store.py`.

**Tests.** `tests/agent/test_prompt_cache_stable.py` (9): the Bot Chat section after the shared part; the day's copy
until the next day, `session` as upstream; a fact stored in an earlier conversation of the day told in the next
one's note, not told twice, its removal told; no note on a guest turn; instructions told once, again when changed,
untouched off `api_server`; both notes join the turn's message. Effect on the 3 November lab day, the dumps
rewritten as the patches would send them (internal/notes/cache-hermes.md): GLM, tokens lost between conversations
from these sources ≈ 16k → ≈ 1k; DeepSeek ≈ 73k → 0.

## 0024 — tools refused for one turn

**Problem.** In a Live call (4/11) the person asked how to pronounce a word and the agent started making an audio file
(`create_audio`) while they waited on the line. A voice call needs the agent to speak, not to make media or start long
work, but the person's own turns can't be closed as a guest's are (0018: no USER.md, no memory, a fresh agent).

**What the patch does.** A turn's `model_options` may carry `closed_tools` (tool names, a trailing `*` closing a
prefix, 128 at most): the dispatcher refuses those tools, the underlying tool of a `tool_call` included, with a reply
telling the model to say it instead and offer it for after the call. The request still offers every tool: the tool
list stays the one the agent's conversations send, so the model server's prompt cache keeps one prefix (cache audit
rule). Nothing else of the turn changes (prompt, memory, profile). `model_options` already rides every path a turn
takes (session chat and its stream, `/v1/runs`, the session authority's admission), so no new field is plumbed; the
mark is set again at every turn (`_create_agent` on the direct path, the TurnRunner's resolved agent), so a cached
agent's next turn runs them again. A malformed value closes nothing (logged). The bridge sends it on a Live call's
turns and warm-up (`bridge-rs/src/live_turns.rs` `LIVE_CLOSED_TOOLS`); a Hermes without the patch ignores the key.

**Files.** `agent/closed_tools.py` (new), `agent/agent_runtime_helpers.py` (`invoke_tool`), `agent/tool_executor.py`
(`_dispatch_authorized_once`), `gateway/platforms/api_server.py` (`_create_agent`), `gateway/run_turn_runner.py`.

**Tests.** `tests/agent/test_closed_tools.py`: parsing (a malformed value ignored), the request's tools unchanged, the
next turn's tools back, exact names and prefixes refused, `invoke_tool` and the policy gate refusing without running
anything, `_create_agent` marking the agent from its `model_options`. With 0018's tests and the gateway's turn runner
tests: green on v0.22.5 with the whole set, 0023 under it.

## 0025 — a colon mid-sentence no longer holds back the rest of a reply

**Problem.** Alex 5/11 (TestFlight 20261105.1253, agent Vie on GLM): « le premier mot arrive, puis tout d'un coup ».
The chat-completions stream holds text back while it may be an SSE block a provider echoed into the content
(`_provider_stream_text_may_be_sse`: `data: …`, `: comment`). It ran on every delta: a delta starting with a colon in
the middle of a line was taken for a comment line, and everything after it was held until a newline or the end of the
response. French typography puts a space before a colon (« fond : vers », a token " :"), so a one-paragraph French
reply showed its first words, then nothing, then the rest at once. Seen on the wire with a real v0.22.5 and a fake
model streaming a word every 25 ms: a 1.3 s gap at the " :", then the rest in one burst (Hermes' own `timestamp`s on
those events are all the same); upstream main (785c2e5110, 4/11) has the same check.

**What the patch does.** The guard starts only on a delta that begins a line (the response's first, or one right after
a newline); text it holds is judged as before (a real echoed block at a line's start still waits).

**Files.** `agent/chat_completion_helpers.py`, `tests/agent/test_streaming.py`.

**Tests.** `TestColonMidSentenceStreams`: each delta shown before the next chunk is read with « … fond », « : », « vers
… » (fails without the patch: held from « : » to the end), a `: keepalive` line at a line's start still held.
`test_streaming.py`, `test_partial_stream_finish_reason.py`, `test_error_classifier.py`: 257 passed, the same 5
failures as without the patch (Anthropic stream tests, this server's environment). The same fake model through a
gateway with the set + 0025: max gap 0.14 s (1.29 s without). `scripts/hermes-patches.sh check`: the set applies.
