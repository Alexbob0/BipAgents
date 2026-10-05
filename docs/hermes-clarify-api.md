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
