# HermesKit

Pure-Swift client for the Hermes Agent `api_server` (Swift 6, strict concurrency, no dependencies).
Platforms: iOS 18+, macOS 15+. Tests: `swift test` (Swift Testing, `URLProtocol` stub server, SSE fixtures).

## Public API

**Client**: `HermesClient(baseURL:apiKey:session:)`, which is `Sendable` and uses Bearer auth.
- `health()` → `HermesHealth`, `capabilities()` → `HermesCapabilities` (`supports(_:)`, `missingRequiredFeatures`)
- Sessions: `listSessions(limit:offset:)`, `createSession(title:)`, `session(id:)`, `renameSession(id:title:)` (PATCH),
  `deleteSession(id:)`, `messages(sessionID:includeCompacted:inlineImages:)`, `forkSession(id:title:)`
- Chat: `chatStream(sessionID:input:uploader:)` / `chatStream(sessionID:prepared:)` → `AsyncThrowingStream<HermesEvent, Error>`
- Runs: `createRun(input:sessionID:instructions:idempotencyKey:)` → `RunHandle`, `getRun(id:)` → `HermesRun`,
  `runEvents(runID:)` (SSE stream), `approve(runID:choice:requestID:)` → `ApprovalResult`, `stop(runID:)`, `steer(runID:text:)`
- Errors: `HermesError` (`.unauthorized`, `.tooManyRuns` (429), `.http(status:message:code:)` decoded from
  `{"error":{"message","code"}}`, `.unreachable(URLError.Code)`, `.invalidResponse`, `.documentUploaderUnavailable`, `.unsupported`),
  with `isRetryable`.

**Events** (`Events/HermesEvent.swift`, the only file that maps server events): `HermesEvent { kind, type, runID, sessionID, eventID, raw }`
with `Kind`: `.delta`, `.interim(text:alreadyStreamed:)`, `.reasoning`, `.tool(ToolEvent)`, `.approvalRequest(ApprovalRequest)`,
`.approvalResponded`, `.assistantCompleted`, `.runCompleted/.runFailed/.runCancelled/.runInterrupted(RunOutcome)`, `.runStopping`,
`.runSteered`, `.subagentStarted/.subagentCompleted(SubagentInfo)`, `.unknown(type:raw:)`. `isTerminal`.

**SSE**: `SSEParser` (push-based, byte level), `SSEEventSequence` / `someByteSequence.sseEvents` over any
`AsyncSequence<UInt8>` such as `URLSession.AsyncBytes`.

**Input** (`Input/MessageInput.swift`, the only file that shapes a turn's request body): `MessageInput(text:attachments:)`, where
`Attachment` is `.image(data:mimeType:)` or `.document(filename:mimeType:data:)`. `prepared(uploader:)` gives a `PreparedInput`
(`input`: a plain string, or OpenAI multimodal parts when images are attached; `body(merging:)`). Documents are uploaded,
then referenced in the text as
`[Pièce jointe : rapport.pdf (application/pdf, 1,2 Mo) → /home/hermes/.hermes/uploads/…]`.
- `DocumentUploader` protocol → `UploadedDocument { path, filename, size }`
- `BridgeDocumentUploader(bridgeURL:bridgeKey:agent:session:)` sends `POST {bridgeURL}/v1/files` as multipart (`agent`, `file`)
- Not `POST /v1/artifacts/upload`: verified on aibox (2026-10-04), it is the browser-extension broker's one-shot, 5-minute store
  (404 unless `browser.extension_control.enabled`), never readable by the agent. Documents always go through the bridge.

**Models**: `AgentConfig`, `AgentSecrets`, `AgentProvisioning(qrPayload:)` (QR JSON → config + secrets), `HermesSession`,
`HermesMessage` (+ `MessageAttachment`), `ToolEvent`, `ApprovalRequest`, `ApprovalChoice` (accepts aliases), `RunStatus`,
`HermesRun`, `RunHandle`, `RunOutcome`, `TokenUsage`, `JSONValue`.

**Resilience**: `Backoff` (0.5 s → 8 s, ±20 % jitter), `RunResumer(client:backoff:maxConsecutiveFailures:).resume(runID:)`
→ stream of `.status(HermesRun)` / `.event(HermesEvent)` / `.expired`. It polls, re-subscribes, falls back to polling
when the event buffer has expired, and stops on a terminal state.

## To verify against the live server

Mapping is deliberately lenient. Check each point below with `curl -N` on the real instance, then tighten
`HermesEvent.swift` / `ResponseMapping.swift` / `MessageInput.swift`:

1. **Event envelope**: which key carries the event name (`type` or `event`), whether the SSE `event:` line is also set,
   and whether fields are flat or nested under `data`. Also check that `run_id` is on every event, including the first one
   of `chat/stream` (SPEC §A4).
2. **Delta and commentary fields**: `assistant.delta` / `message.delta` text key (`delta`?), `assistant.commentary` keys, and
   the final text key of `assistant.completed`.
3. **Reasoning**: whether `chat/stream` or `/v1/runs/{id}/events` emit reasoning deltas at all, and under which event name.
4. **Tool events**: `tool.completed.error` type (bool or string), whether `tool_call_id` exists to pair start and completion,
   and the shape of the `tool.failed` payload.
5. **Approval**: `approval.request` keys (`request_id`, `command`, `description`). Its `approval_data` is spread at the top level,
   so check that no field there collides with the type key.
6. **`chat/stream` body for attachments**: does `input` accept the multimodal parts array, or is a separate `attachments` field
   expected (SPEC §A4 mentions `"attachments"?`)? Same question for `POST /v1/runs`, which is documented as "simple `input` string".
7. **Sessions API shapes**: the list wrapper key and row fields (`title`, `updated_at`/`last_active`, preview, `message_count`);
   the create and fork response (bare object or `{"session": …}`); the `messages` wrapper, content format, `tool_calls`,
   reasoning field, timestamps and ids.
8. **Run re-subscription**: whether `GET /v1/runs/{id}/events` replays events already delivered or only sends new ones;
   what happens after the 5 min buffer expiry (404 or an empty stream); whether runs started by `chat/stream` are reachable on
   `/v1/runs/{id}*`; and whether `GET /v1/runs/{id}` returns 404 once the brief retention ends.
9. ~~Artifacts upload~~: checked on aibox, not usable for agent files (see above).
10. **Bridge `POST /v1/files`** (to be built, SPEC §B3): it must store files somewhere visible inside the Hermes container
    (`/home/hermes/.hermes/uploads/…`) and return that in-container path.
11. **Capabilities**: actual feature keys (`run_approval`, `session_*`) and object-valued features.
