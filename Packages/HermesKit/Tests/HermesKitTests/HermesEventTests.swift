import Foundation
import Testing
@testable import HermesKit

@Suite("Event decoding")
struct HermesEventTests {
    private func decode(_ json: String, sseEvent: String? = nil) throws -> HermesEvent {
        try #require(HermesEvent(sse: SSEEvent(event: sseEvent, data: json)))
    }

    @Test(arguments: [
        #"{"type":"assistant.delta","run_id":"r1","delta":"Bon"}"#,
        #"{"event":"message.delta","run_id":"r1","delta":"Bon"}"#,
        #"{"type":"message.delta","run_id":"r1","text":"Bon"}"#,
        #"{"type":"assistant.delta","run_id":"r1","content":"Bon"}"#,
        #"{"type":"message.delta","run_id":"r1","delta":{"content":"Bon"}}"#,
    ])
    func deltaVariants(_ json: String) throws {
        let event = try decode(json)
        #expect(event.kind == .delta("Bon"))
        #expect(event.runID == "r1")
    }

    @Test func typeFallsBackToSSEEventName() throws {
        let event = try decode(#"{"run_id":"r1","delta":"x"}"#, sseEvent: "message.delta")
        #expect(event.kind == .delta("x"))
        #expect(event.type == "message.delta")
    }

    @Test func nestedDataEnvelope() throws {
        let event = try decode(#"{"event":"message.delta","run_id":"r1","data":{"delta":"y"}}"#)
        #expect(event.kind == .delta("y"))
    }

    @Test func interim() throws {
        #expect(try decode(#"{"type":"message.interim","run_id":"r","text":"Je cherche","already_streamed":true}"#).kind
            == .interim(text: "Je cherche", alreadyStreamed: true))
        #expect(try decode(#"{"type":"assistant.commentary","message_id":"m","text":"Hmm"}"#).kind
            == .interim(text: "Hmm", alreadyStreamed: false))
    }

    @Test(arguments: ["reasoning.delta", "assistant.reasoning", "thinking.delta", "custom.reasoning_chunk"])
    func reasoning(_ type: String) throws {
        #expect(try decode(#"{"type":"\#(type)","delta":"hmm"}"#).kind == .reasoning("hmm"))
    }

    @Test func toolLifecycle() throws {
        let started = try decode(#"{"type":"tool.started","run_id":"r","tool":"terminal","preview":"ls"}"#)
        #expect(started.kind == .tool(ToolEvent(tool: "terminal", preview: "ls", status: .started)))

        let completed = try decode(#"{"type":"tool.completed","tool":"terminal","duration":1.5,"error":false,"preview":"ok"}"#)
        #expect(completed.kind == .tool(ToolEvent(tool: "terminal", preview: "ok", status: .completed, duration: 1.5)))

        let flagged = try decode(#"{"type":"tool.completed","tool":"terminal","duration":0.1,"error":true,"preview":"BLOCKED: denied"}"#)
        #expect(flagged.kind == .tool(ToolEvent(tool: "terminal", preview: "BLOCKED: denied", status: .failed, duration: 0.1)))

        let failed = try decode(#"{"type":"tool.failed","tool":"web_search","error":"timeout"}"#)
        #expect(failed.kind == .tool(ToolEvent(tool: "web_search", status: .failed, error: "timeout")))
    }

    @Test func approvalRequestWithChoicesAndExtras() throws {
        let json = #"""
        {"type":"approval.request","run_id":"run_9","request_id":"req_1","command":"rm -rf ***","description":"recursive delete",
         "pattern_key":"rm_rf","smart_denied":false,"choices":["once","session","always","deny"]}
        """#
        let event = try decode(json)
        guard case .approvalRequest(let request) = event.kind else { Issue.record("not an approval"); return }
        #expect(request.runID == "run_9")
        #expect(request.requestID == "req_1")
        #expect(request.command == "rm -rf ***")
        #expect(request.description == "recursive delete")
        #expect(request.choices == [.once, .session, .always, .deny])
        #expect(request.extra["pattern_key"] == "rm_rf")
        #expect(request.extra["smart_denied"] == false)
        #expect(request.extra["choices"] == nil)
    }

    @Test func approvalTypeIsNotShadowedBySpreadFields() throws {
        // approval_data is spread into the envelope; a non-dotted `type` must not hide the event name.
        let event = try decode(#"{"type":"dangerous_command","event":"approval.request","run_id":"r","choices":["once","deny"]}"#)
        guard case .approvalRequest(let request) = event.kind else { Issue.record("not an approval"); return }
        #expect(request.choices == [.once, .deny])
        #expect(request.extra["type"] == nil)
    }

    @Test func approvalChoicesDefaultAndAliases() throws {
        guard case .approvalRequest(let request) = try decode(#"{"type":"approval.request","run_id":"r"}"#).kind else {
            Issue.record("not an approval"); return
        }
        #expect(request.choices == [.once, .deny])
        #expect(ApprovalChoice(lenient: "Approve") == .once)
        #expect(ApprovalChoice(lenient: "allow") == .once)
        #expect(ApprovalChoice(lenient: "nope") == nil)
    }

    @Test func approvalResponded() throws {
        #expect(try decode(#"{"type":"approval.responded","run_id":"r","choice":"session","request_id":"q","resolved":1}"#).kind
            == .approvalResponded(choice: .session, requestID: "q"))
    }

    @Test func terminalEvents() throws {
        let completed = try decode(#"{"type":"run.completed","run_id":"r","output":"Done.","usage":{"input_tokens":50,"output_tokens":200,"total_tokens":250,"cache_read_tokens":40}}"#)
        #expect(completed.kind == .runCompleted(RunOutcome(
            output: "Done.", usage: TokenUsage(inputTokens: 50, outputTokens: 200, totalTokens: 250, cacheReadTokens: 40))))
        #expect(completed.isTerminal)

        let failed = try decode(#"{"type":"run.failed","error":"provider down","completed":false,"turn_exit_reason":"max_iterations_reached(60/60)"}"#)
        #expect(failed.kind == .runFailed(RunOutcome(error: "provider down", completed: false, turnExitReason: "max_iterations_reached(60/60)")))

        let cancelled = try decode(#"{"type":"run.cancelled","completed":false,"interrupted":true,"turn_exit_reason":"interrupted_by_user","pending_steer":"also X"}"#)
        #expect(cancelled.kind == .runCancelled(RunOutcome(completed: false, interrupted: true, turnExitReason: "interrupted_by_user", pendingSteer: "also X")))

        let interrupted = try decode(#"{"type":"run.interrupted","error":{"message":"Gateway shutdown interrupted the run."}}"#)
        #expect(interrupted.kind == .runInterrupted(RunOutcome(error: "Gateway shutdown interrupted the run.")))
        #expect(interrupted.isTerminal)
    }

    @Test func lifecycleEvents() throws {
        #expect(try decode(#"{"type":"run.stopping","run_id":"r"}"#).kind == .runStopping)
        #expect(try decode(#"{"type":"run.steered","run_id":"r","accepted":true}"#).kind == .runSteered(accepted: true))
        #expect(try decode(#"{"type":"assistant.completed","text":"Hi","completed":true,"partial":false}"#).kind
            == .assistantCompleted(RunOutcome(output: "Hi", completed: true, partial: false)))
        #expect(try !decode(#"{"type":"run.stopping"}"#).isTerminal)
    }

    @Test func subagents() throws {
        guard case .subagentStarted(let start) = try decode(#"{"type":"subagent.start","delegation_id":"d1","goal":"research"}"#).kind else {
            Issue.record("not subagent.start"); return
        }
        #expect(start.delegationID == "d1")
        #expect(start.goal == "research")

        guard case .subagentCompleted(let done) = try decode(#"{"type":"subagent.complete","delegation_id":"d1","child_session_id":"c1","status":"completed","summary":"ok","duration":12}"#).kind else {
            Issue.record("not subagent.complete"); return
        }
        #expect(done.childSessionID == "c1")
        #expect(done.status == "completed")
        #expect(done.duration == 12)
    }

    @Test func unknownAndEmptyFrames() throws {
        let event = try decode(#"{"type":"run.queued","run_id":"r","position":2}"#)
        #expect(event.kind == .unknown(type: "run.queued", raw: ["type": "run.queued", "run_id": "r", "position": 2]))
        #expect(event.runID == "r")

        #expect(HermesEvent(sse: SSEEvent(data: "[DONE]")) == nil)
        #expect(HermesEvent(sse: SSEEvent(data: "")) == nil)
        #expect(HermesEvent(sse: SSEEvent(event: "ping", data: "not json"))?.kind == .unknown(type: "ping", raw: "not json"))
    }

    @Test func runIDAndSessionIDFromEnvelope() throws {
        let event = try decode(#"{"type":"tool.started","run_id":"run_1","session_id":"s_1","tool":"x"}"#)
        #expect(event.runID == "run_1")
        #expect(event.sessionID == "s_1")
    }
}
