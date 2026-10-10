# Hermes patches used by BipAgents

Patches for [Hermes Agent](https://github.com/NousResearch/hermes-agent) that the BipAgents app relies on,
until equivalent changes land upstream.

- **Hermes base commit:** `517b5e10f` (upstream `main` of 2026-10-08, v0.21.6+140). Every patch applies
  cleanly there.
- **Apply** from the root of a Hermes checkout at that commit: `git apply hermes/patches/<patch>`.
- **Upstream check:** `git apply --check` against upstream `main` at `a62979dc36` (2026-10-10, 380 commits
  after the base). It was run on a temporary index, without installing anything.

Numbers 0002 and 0003 are not published yet.

| Patch | On `517b5e10f` | On upstream `main` (`a62979dc36`) |
|---|---|---|
| 0001 api_server clarify | applies | **does not apply as is**: context conflict in `gateway/platforms/api_server_runs.py` (upstream switched `"web.Request"` annotations to `web.Request`); `git apply --3way` applies `api_server.py` cleanly and only needs that conflict resolved. Upstream has no clarify support in the Runs API yet. |
| 0004 image store | applies | applies |

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
- When a user message is saved, its images (`data:` or remote URLs) are written once to
  `<HERMES_HOME>/image_store/<sha256>.<ext>`, with a small reference file per message under
  `image_store/refs/`. The database row and the session messages API stay text only, with no base64.
- When a request is built, and only for a model that sees images natively, the 3 most recent images
  of the conversation are put back as `image_url` parts where their `[screenshot]` marker was. Older
  markers read `[image envoyée plus tôt]` ("image sent earlier"). The result depends only on the
  history, so the server's prefix cache stays valid from one turn to the next.
- Cleanup: each session keeps references to its 3 newest images only; deleting a session drops its
  references; a sweep at gateway startup drops references whose message is gone (deleted session,
  compaction). A file is removed once no reference points to it.

**Files.** `agent/image_store.py` (new), `agent/session_persistence.py`, `agent/turn_context.py`,
`gateway/run.py`, `hermes_state_sessions.py`, `tests/agent/test_image_store_local.py` (new, 11 tests).

**Upstream note.** The placeholder text for older images is in French; an upstream version would use
English or make it configurable.
