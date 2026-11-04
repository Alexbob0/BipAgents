🇬🇧 English · [🇫🇷 Français](hermes-clarify-api.fr.md)

# `clarify` in the Hermes api_server (Runs API) — specification

Hermes 0.21 exposes the `clarify` tool to api_server agents but does not wire it up: `_create_agent()`
passes no `clarify_callback` to `AIAgent`, and the tool answers "Clarify tool is not available in this
execution context". This document describes the wiring BipAgents expects. It **mirrors
approvals** (`_make_approval_notify`, `POST /v1/runs/{id}/approval`).

## Hermes side (fix)

1. In `gateway/platforms/api_server_runs.py`, for each run:
   - set `agent.clarify_callback` (or pass it to `_create_agent()`);
   - the callback registers the question in `tools/clarify_gateway.py` (`register`), emits the
     `clarify.request` event (below), moves the run to the `waiting_for_input` status, then waits for the answer
     (`wait_for_response`) up to `agent.clarify_timeout`;
   - answer received → status `running`, `clarify.responded` event, the tool returns the answer to the agent;
   - timeout or run stopped → `clarify.cancelled` event, the tool returns "no answer"
     (same behavior as a messaging platform with no reply).
2. New route `POST /v1/runs/{run_id}/clarify` that calls `resolve_gateway_clarify`.
3. `clarify_timeout` for the profiles served to the app: **30 min** (users often answer from a
   notification, later on).
4. Capability advertised in `/v1/capabilities`: `"run_clarify": true`.

## SSE events (`GET /v1/runs/{run_id}/events`)

```
event: clarify.request
data: {"type": "clarify.request", "run_id": "run_…", "request_id": "clr_…",
       "questions": [{"id": "q1", "question": "Quel train ?", "choices": ["9h — 19 €", "14h — 35 €"],
                      "allow_other": true}]}
```

- A simple `clarify(question, choices)` question yields a `questions` array with one element (free-form `id`,
  e.g. `"q1"`). The tool's "multiple questions" mode yields one element per question.
- `choices` can be empty (open question); `allow_other` says whether a free-text answer is accepted.

```
event: clarify.responded
data: {"type": "clarify.responded", "run_id": "run_…", "request_id": "clr_…", "answers": {"q1": "9h — 19 €"}}

event: clarify.cancelled
data: {"type": "clarify.cancelled", "run_id": "run_…", "request_id": "clr_…", "reason": "timeout" | "stopped"}
```

## Answer (`POST /v1/runs/{run_id}/clarify`)

```json
{"request_id": "clr_…", "answers": {"q1": "9h — 19 €"}}
```

- The value is the text of a choice, or a free-text answer if `allow_other`.
- `{"request_id": "clr_…", "cancel": true}` cancels (the agent receives "no answer").
- Responses: `200 {"resolved": 1}`; `404` unknown / expired run or question; `409` already answered.

## BipAgents side

- The app shows a "Question" card with one button per choice (a single tap is enough for a simple
  question; "Other…" opens a text field if `allow_other`), and answers through the route above.
- The bridge relays these events like the others and, when the app is not following the run, sends a
  "<agent> has a question" notification that reopens the conversation.

## Since Hermes 0.22.5 (upstream clarify)

Since 2026-11-01 BipAgents runs Hermes 0.22.5, whose « session authority » runs every API turn and has its own
clarify: patch 0001 is dropped, and what is above is kept as its record. The contract now:

- `clarify.request` carries one question at the top level (`question`, `choices`, `multi_select`) with its
  `prompt_id` and `execution_generation`. A batch from the tool comes as one prompt after the other. The
  bridge adds `request_id` (the `prompt_id`), the id the apps and the pushes read.
- The answer is `POST /v1/runs/{run_id}/clarify` with exactly
  `{"request_id": "<prompt_id>", "execution_generation": <n>, "answer": "<text>"}`.
- There is no dismissal. A skipped question is sent as Hermes' skip word, `"\u0000skipped"` (`tools/clarify_gateway.py`
  `SKIPPED`, what a messaging app's typed « skip » becomes): the agent reads it as skipped and the next question of the
  batch still comes. An **empty** `answer` is read as no reply at all (`[user did not respond within 60m]`, the whole
  batch ends): the apps still send an empty answer for « Passer » and the bridge turns it into the skip word (measured
  on v0.22.5, 2026-11-04).
- The first choice comes with Hermes' English « (Recommended) » label; the bridge drops it before the apps see it
  (Hermes strips it from the answer anyway).
- `clarify.settled` and `approval.settled` end a prompt (answered, timed out or gone); they carry no answers.
- Approvals take exactly `{"request_id": "<prompt_id>", "execution_generation": <n>, "choice": …}`. Without
  the generation (a notification's button), the apps and the bridge read the prompt from `GET /v1/runs/{id}`
  (`pending_controls`) first.
- Upstream's `clarify_batch` (all the questions at once) is offered to local viewers only in 0.22.5, not on
  the Runs API.
