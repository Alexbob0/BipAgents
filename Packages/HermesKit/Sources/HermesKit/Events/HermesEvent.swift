import Foundation

// ALL server-event → model mapping lives in this file, so the envelope can be adjusted in one place
// once verified against the live server (see README "To verify against the live server").
//
// Known server facts (hermes v0.21.5, context/hermes-approval-and-events.txt):
// - every event is built by `_run_event(run_id, type, **fields)`; the exact key carrying the type
//   (`type` vs `event`) is not confirmed, hence the lenient lookup below;
// - `/v1/runs/{id}/events` emits `message.delta {delta}`, `message.interim {text, already_streamed}`;
// - `/api/sessions/{id}/chat/stream` emits `assistant.delta`, `assistant.commentary {message_id, text,
//   already_streamed}`, `assistant.completed {completed, partial, interrupted}`;
// - both emit `tool.started {tool, preview}`, `tool.completed {tool, duration, error, preview}`,
//   `tool.failed`, `approval.request`, `approval.responded`, `run.completed|failed|cancelled|interrupted`.

/// A decoded event from a Hermes SSE stream (`chat/stream` or `/v1/runs/{id}/events`).
public struct HermesEvent: Sendable, Hashable {
    public enum Kind: Sendable, Hashable {
        /// Answer text to append.
        case delta(String)
        /// Mid-turn commentary. When `alreadyStreamed` is true the text also went out as deltas.
        case interim(text: String, alreadyStreamed: Bool)
        /// Private reasoning / thinking text to append to the collapsible block.
        case reasoning(String)
        /// `tool.started` / `tool.completed` / `tool.failed` (status `.failed` also when `error` is truthy).
        case tool(ToolEvent)
        case approvalRequest(ApprovalRequest)
        case approvalResponded(choice: ApprovalChoice?, requestID: String?)
        /// `clarify.request`: the agent asks the user to choose / answer (docs/hermes-clarify-api.md).
        case clarifyRequest(ClarifyRequest)
        /// `clarify.responded` (`answers`) or `clarify.cancelled` (`answers` nil).
        case clarifyResolved(requestID: String?, answers: [String: String]?)
        /// `assistant.completed` on `chat/stream` (final text + real completed/partial/interrupted flags).
        case assistantCompleted(RunOutcome)
        case runCompleted(RunOutcome)
        case runFailed(RunOutcome)
        case runCancelled(RunOutcome)
        case runInterrupted(RunOutcome)
        case runStopping
        case runSteered(accepted: Bool)
        case subagentStarted(SubagentInfo)
        case subagentCompleted(SubagentInfo)
        case unknown(type: String, raw: JSONValue)
    }

    public var kind: Kind
    /// Event type as resolved from the envelope (e.g. `message.delta`).
    public var type: String
    public var runID: String?
    public var sessionID: String?
    /// SSE `id:` field, if the server sets one.
    public var eventID: String?
    public var raw: JSONValue

    public init(kind: Kind, type: String, runID: String? = nil, sessionID: String? = nil, eventID: String? = nil, raw: JSONValue = .null) {
        self.kind = kind
        self.type = type
        self.runID = runID
        self.sessionID = sessionID
        self.eventID = eventID
        self.raw = raw
    }

    /// True for `run.completed|failed|cancelled|interrupted`.
    public var isTerminal: Bool {
        switch kind {
        case .runCompleted, .runFailed, .runCancelled, .runInterrupted: true
        default: false
        }
    }
}

/// `subagent.start` / `subagent.complete` payload.
public struct SubagentInfo: Sendable, Hashable {
    public var delegationID: String?
    public var childSessionID: String?
    public var goal: String?
    public var status: String?
    public var summary: String?
    public var duration: TimeInterval?
    public var raw: JSONValue
}

// MARK: - Decoding

extension HermesEvent {
    /// Event-type keys in the JSON envelope, in priority order.
    static let typeKeys = ["type", "event", "event_type"]
    static let runIDKeys = ["run_id", "runId"]

    /// Decodes one SSE frame. Returns `nil` for frames that carry nothing (empty data, OpenAI `[DONE]`).
    public init?(sse: SSEEvent) {
        let data = sse.data.trimmingCharacters(in: .whitespacesAndNewlines)
        if data == "[DONE]" { return nil }
        if data.isEmpty && sse.event == nil { return nil }

        let json = data.isEmpty ? JSONValue.object([:]) : (try? JSONValue.parse(data)) ?? .string(sse.data)
        guard json.objectValue != nil else {
            self.init(kind: .unknown(type: sse.event ?? "message", raw: json), type: sse.event ?? "message", eventID: sse.id, raw: json)
            return
        }
        self.init(json: json, sseEventName: sse.event, eventID: sse.id)
    }

    /// Decodes a JSON event envelope. The type comes from the JSON (`type`/`event`) or the SSE `event:` name.
    public init(json: JSONValue, sseEventName: String? = nil, eventID: String? = nil) {
        let fields = LenientFields(json)
        let type = Self.resolveType(json: json, sseEventName: sseEventName)
        let runID = fields.string(Self.runIDKeys[0], Self.runIDKeys[1])
        self.init(
            kind: Self.kind(type: type, fields: fields, runID: runID, raw: json),
            type: type,
            runID: runID,
            sessionID: fields.string("session_id", "sessionId"),
            eventID: eventID,
            raw: json
        )
    }

    /// Hermes event names are dotted (`message.delta`). Approval payloads spread arbitrary fields at
    /// the top level, so a non-dotted `type` (e.g. a command category) must not win over a dotted one.
    static func resolveType(json: JSONValue, sseEventName: String?) -> String {
        let candidates = typeKeys.compactMap { json[$0]?.stringValue } + [sseEventName].compactMap { $0 }
        return candidates.first { $0.contains(".") } ?? candidates.first ?? "message"
    }

    static func kind(type: String, fields f: LenientFields, runID: String?, raw: JSONValue) -> Kind {
        switch type {
        case "assistant.delta", "message.delta", "response.output_text.delta":
            return .delta(text(f, "delta", "text", "content") ?? "")

        case "assistant.commentary", "message.interim":
            return .interim(text: text(f, "text", "content", "delta") ?? "", alreadyStreamed: f.bool("already_streamed") ?? false)

        case "reasoning.delta", "reasoning", "assistant.reasoning", "assistant.reasoning.delta", "message.reasoning",
             "reasoning.available", "thinking.delta", "assistant.thinking":
            return .reasoning(text(f, "delta", "text", "content", "reasoning") ?? "")

        case "tool.started", "tool.start", "tool.completed", "tool.complete", "tool.failed":
            return .tool(toolEvent(type: type, f))

        case "approval.request", "approval.required":
            return .approvalRequest(approval(f, runID: runID))

        case "approval.responded", "approval.resolved":
            return .approvalResponded(choice: f.string("choice").flatMap(ApprovalChoice.init(lenient:)),
                                      requestID: f.string("request_id"))

        case "clarify.request", "clarify.required":
            return .clarifyRequest(clarify(f, runID: runID))

        case "clarify.responded", "clarify.resolved":
            let answers = f.value("answers")?.objectValue?.compactMapValues { $0.lenientString }
            return .clarifyResolved(requestID: f.string("request_id"), answers: answers ?? [:])

        case "clarify.cancelled", "clarify.canceled", "clarify.expired":
            return .clarifyResolved(requestID: f.string("request_id"), answers: nil)

        case "assistant.completed", "message.complete", "message.completed":
            return .assistantCompleted(outcome(f))

        case "run.completed": return .runCompleted(outcome(f))
        case "run.failed": return .runFailed(outcome(f))
        case "run.cancelled", "run.canceled": return .runCancelled(outcome(f))
        case "run.interrupted": return .runInterrupted(outcome(f))
        case "run.stopping": return .runStopping
        case "run.steered": return .runSteered(accepted: f.bool("accepted") ?? true)

        case "subagent.start", "subagent.started": return .subagentStarted(subagent(f, raw: raw))
        case "subagent.complete", "subagent.completed": return .subagentCompleted(subagent(f, raw: raw))

        default:
            // Unlisted reasoning/thinking delta names still feed the reasoning block.
            if type.contains("reasoning") || type.contains("thinking"), let delta = text(f, "delta", "text") {
                return .reasoning(delta)
            }
            return .unknown(type: type, raw: raw)
        }
    }

    // MARK: Field helpers

    /// Text that may be a plain string or nested one level (`{"delta": {"content": "…"}}`).
    private static func text(_ f: LenientFields, _ keys: String...) -> String? {
        guard let value = f.value(keys) else { return nil }
        if let string = value.stringValue { return string }
        return LenientFields(value, nestedIn: []).value("content", "text", "delta")?.stringValue
    }

    /// `error` is either a boolean flag, a message, or `{"message": …}`.
    private static func errorMessage(_ value: JSONValue?) -> String? {
        switch value {
        case .string(let message)?: message.nilIfEmpty
        case .object?: value?["message"]?.stringValue ?? value?.jsonString
        default: nil
        }
    }

    private static func toolEvent(type: String, _ f: LenientFields) -> ToolEvent {
        let errorValue = f.value("error")
        // `error` is a flag on tool.completed, but may also carry a message.
        let failed = type == "tool.failed" || (errorValue?.boolValue ?? (errorMessage(errorValue) != nil))
        let status: ToolEvent.Status = type.hasSuffix("start") || type.hasSuffix("started") ? .started : (failed ? .failed : .completed)
        let duration = f.double("duration", "duration_s") ?? f.double("duration_ms").map { $0 / 1000 }
        return ToolEvent(
            tool: f.string("tool", "name", "tool_name") ?? "tool",
            preview: text(f, "preview", "result_preview", "args_preview"),
            status: status,
            duration: duration,
            error: failed && errorValue?.boolValue == nil ? errorMessage(errorValue) : nil,
            callID: f.string("tool_call_id", "call_id")
        )
    }

    static let approvalKnownKeys: Set<String> = ["run_id", "runId", "type", "event", "request_id", "command",
                                                  "description", "choices", "session_id"]

    private static func clarify(_ f: LenientFields, runID: String?) -> ClarifyRequest {
        func question(_ q: LenientFields, fallbackID: String) -> ClarifyRequest.Question? {
            guard let text = q.string("question", "text", "prompt") else { return nil }
            let choices = q.value("choices", "options")?.arrayValue?.compactMap { $0.stringValue ?? $0["label"]?.stringValue } ?? []
            return ClarifyRequest.Question(id: q.string("id", "question_id") ?? fallbackID, question: text, choices: choices,
                                           allowOther: q.bool("allow_other", "allow_free_text") ?? true)
        }
        var questions = (f.value("questions")?.arrayValue ?? []).enumerated().compactMap { index, value in
            question(LenientFields(value, nestedIn: []), fallbackID: "q\(index + 1)")
        }
        if questions.isEmpty, let single = question(f, fallbackID: "q1") { questions = [single] }
        return ClarifyRequest(runID: runID ?? "", requestID: f.string("request_id", "clarify_id", "id") ?? "", questions: questions)
    }

    private static func approval(_ f: LenientFields, runID: String?) -> ApprovalRequest {
        let choices = f.value("choices")?.arrayValue?
            .compactMap { $0.stringValue.flatMap(ApprovalChoice.init(lenient:)) } ?? []
        var extra = f.merged
        for key in approvalKnownKeys { extra[key] = nil }
        return ApprovalRequest(
            runID: runID ?? "",
            requestID: f.string("request_id", "approval_id"),
            command: f.string("command"),
            description: f.string("description", "reason"),
            choices: choices.isEmpty ? [.once, .deny] : choices,
            sessionID: f.string("session_id"),
            extra: extra
        )
    }

    static func usage(_ value: JSONValue?) -> TokenUsage? {
        guard let value, value.objectValue != nil else { return nil }
        let u = LenientFields(value, nestedIn: [])
        return TokenUsage(
            inputTokens: u.int("input_tokens", "prompt_tokens"),
            outputTokens: u.int("output_tokens", "completion_tokens"),
            totalTokens: u.int("total_tokens"),
            cacheReadTokens: u.int("cache_read_tokens"),
            cacheWriteTokens: u.int("cache_write_tokens")
        )
    }

    static func outcome(_ f: LenientFields) -> RunOutcome {
        RunOutcome(
            output: text(f, "output", "final_response", "text", "content"),
            error: errorMessage(f.value("error")),
            usage: usage(f.value("usage")),
            completed: f.bool("completed"),
            partial: f.bool("partial"),
            interrupted: f.bool("interrupted"),
            turnExitReason: f.string("turn_exit_reason"),
            pendingSteer: f.string("pending_steer")
        )
    }

    private static func subagent(_ f: LenientFields, raw: JSONValue) -> SubagentInfo {
        SubagentInfo(
            delegationID: f.string("delegation_id"),
            childSessionID: f.string("child_session_id"),
            goal: f.string("goal", "task", "description"),
            status: f.string("status"),
            summary: f.string("summary"),
            duration: f.double("duration", "duration_s"),
            raw: raw
        )
    }
}
