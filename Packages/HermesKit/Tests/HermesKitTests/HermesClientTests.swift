import Foundation
import Testing
@testable import HermesKit

@Suite("HermesClient")
struct HermesClientTests {
    @Test func sendsBearerAuthAndDecodesHealth() async throws {
        let server = StubServer { _ in .json(["status": "ok"]) }
        let health = try await server.client(apiKey: "s3cret").health()
        #expect(health.isOK)
        let request = try #require(server.requests.first)
        #expect(request.header("Authorization") == "Bearer s3cret")
        #expect(request.method == "GET")
        #expect(request.path == "/health")
    }

    @Test func respectsBaseURLPathPrefix() async throws {
        let server = StubServer { _ in .json(["status": "ok"]) }
        let client = HermesClient(baseURL: server.baseURL.appending(path: "p/wellness/"), apiKey: "k", session: server.session)
        _ = try await client.health()
        #expect(server.requests.first?.path == "/p/wellness/health")
    }

    @Test func capabilitiesAreDecodedLeniently() async throws {
        let server = StubServer { _ in
            .json([
                "object": "hermes.api_server.capabilities", "platform": "hermes-agent", "model": "wellness",
                "features": ["run_submission": true, "run_events_sse": true, "run_approval": true, "run_stop": false,
                             "idempotency": ["supported": true, "store": "sqlite"], "browser_extension_control": ["enabled": false]],
                "endpoints": ["session_list": "/api/sessions"],
            ])
        }
        let capabilities = try await server.client().capabilities()
        #expect(capabilities.model == "wellness")
        #expect(capabilities.supports("run_submission"))
        #expect(!capabilities.supports("run_stop"))
        #expect(capabilities.supports("idempotency"))
        #expect(!capabilities.supports("browser_extension_control"))
        #expect(!capabilities.supports("absent"))
        #expect(capabilities.missingRequiredFeatures.isEmpty)
        #expect(capabilities.endpoints["session_list"] == "/api/sessions")
    }

    @Test func sessionsCRUD() async throws {
        let server = StubServer { request in
            switch (request.method, request.path) {
            case ("GET", "/api/sessions"):
                return .json(["sessions": [
                    ["id": "s1", "title": "Sommeil", "updated_at": 1_760_000_000, "preview": "Bonne nuit", "message_count": 4],
                    ["session_id": "s2", "title": "", "last_active": "2026-10-04T08:15:00Z"],
                ]])
            case ("POST", "/api/sessions"): return .json(["session": ["id": "s3", "title": "Nouveau"]])
            case ("GET", "/api/sessions/s%2F1"): return .json(["id": "s/1"])
            case ("POST", "/api/sessions/s1/fork"): return .json(["id": "s4", "parent_session_id": "s1"])
            default: return .json(["ok": true])
            }
        }
        let client = server.client()

        let sessions = try await client.listSessions(limit: 20, offset: 40)
        #expect(sessions.map(\.id) == ["s1", "s2"])
        #expect(sessions[0].title == "Sommeil")
        #expect(sessions[0].lastMessagePreview == "Bonne nuit")
        #expect(sessions[0].updatedAt == Date(timeIntervalSince1970: 1_760_000_000))
        #expect(sessions[1].title == nil)
        #expect(sessions[1].updatedAt == ISO8601DateFormatter().date(from: "2026-10-04T08:15:00Z"))
        #expect(server.requests[0].query == ["limit": "20", "offset": "40"])

        #expect(try await client.createSession(title: "Nouveau").id == "s3")
        #expect(server.requests[1].json == ["title": "Nouveau"])

        #expect(try await client.session(id: "s/1").id == "s/1")

        try await client.renameSession(id: "s1", title: "Renamed")
        #expect(server.requests[3].method == "PATCH")
        #expect(server.requests[3].path == "/api/sessions/s1")
        #expect(server.requests[3].json == ["title": "Renamed"])

        try await client.deleteSession(id: "s1")
        #expect(server.requests[4].method == "DELETE")

        let fork = try await client.forkSession(id: "s1", title: "alt")
        #expect(fork.parentSessionID == "s1")
    }

    @Test func messagesQueryAndMapping() async throws {
        let server = StubServer { _ in
            .json(["messages": [
                ["role": "user", "content": [["type": "text", "text": "Regarde"], ["type": "image_url", "image_url": ["url": "[image]"]]],
                 "timestamp": 1_760_000_000],
                ["id": 42, "role": "assistant", "content": "", "reasoning": "Je réfléchis",
                 "tool_calls": [["id": "c1", "type": "function", "function": ["name": "terminal", "arguments": "{\"command\":\"ls\"}"]]]],
                ["role": "tool", "tool_name": "terminal", "tool_call_id": "c1", "content": "README.md"],
                ["role": "assistant", "content": "Voilà."],
            ]])
        }
        let messages = try await server.client().messages(sessionID: "s1")
        #expect(server.requests[0].path == "/api/sessions/s1/messages")
        #expect(server.requests[0].query == ["include_compacted": "false", "inline_images": "false"])

        #expect(messages.count == 4)
        #expect(messages[0].role == .user)
        #expect(messages[0].text == "Regarde")
        #expect(messages[0].attachments == [MessageAttachment(kind: .image, url: "[image]")])
        #expect(messages[1].id == "42")
        #expect(messages[1].reasoning == "Je réfléchis")
        #expect(messages[1].toolEvents == [ToolEvent(tool: "terminal", preview: "{\"command\":\"ls\"}", status: .completed, callID: "c1")])
        #expect(messages[2].role == .tool)
        #expect(messages[2].toolEvents.first?.tool == "terminal")
        #expect(messages[3].text == "Voilà.")
    }

    @Test func chatStreamPostsInputAndStreamsFixture() async throws {
        let fixture = try Fixtures.string("chat_stream.sse")
        let server = StubServer { _ in .sse(fixture, chunkSize: 5) }
        let events = try await server.client().chatStream(sessionID: "sess_1", input: "Salut").collect()

        let request = try #require(server.requests.first)
        #expect(request.method == "POST")
        #expect(request.path == "/api/sessions/sess_1/chat/stream")
        #expect(request.header("Accept") == "text/event-stream")
        #expect(request.json == ["input": "Salut"])

        #expect(events.map(\.type) == ["assistant.delta", "tool.started", "tool.completed", "assistant.commentary",
                                       "assistant.delta", "assistant.completed", "run.completed"])
        #expect(events.allSatisfy { $0.runID == "run_abc" })
        let text = events.compactMap { if case .delta(let t) = $0.kind { t } else { nil } }.joined()
        #expect(text == "Bonjour Sam, ça va ?")
        #expect(events[3].kind == .interim(text: "Je regarde…", alreadyStreamed: false))
        #expect(events[2].kind == .tool(ToolEvent(tool: "terminal", preview: "README.md", status: .completed, duration: 0.42, callID: "call_1")))
        #expect(events.last?.isTerminal == true)
    }

    @Test func runEventsWithApprovalFixture() async throws {
        let fixture = try Fixtures.string("run_events_approval.sse")
        let server = StubServer { _ in .sse(fixture, chunkSize: 3) }
        let events = try await server.client().runEvents(runID: "run_xyz").collect()
        #expect(server.requests.first?.path == "/v1/runs/run_xyz/events")
        #expect(events.count == 4)
        guard case .approvalRequest(let approval) = events[1].kind else { Issue.record("expected approval"); return }
        #expect(approval.runID == "run_xyz")
        #expect(approval.requestID == "req-1")
        #expect(approval.command == "rm -rf build/")
        #expect(approval.choices == [.once, .session, .always, .deny])
        #expect(approval.extra["pattern_key"] == "rm_rf")
        #expect(events[2].kind == .approvalResponded(choice: .once, requestID: "req-1"))
        #expect(events[3].kind == .runCompleted(RunOutcome(output: "Fait.")))
    }

    @Test func approvalBody() async throws {
        let server = StubServer { _ in
            .json(["object": "hermes.run.approval_response", "run_id": "run_1", "choice": "once", "request_id": "req-9", "resolved": 1])
        }
        let client = server.client()
        let result = try await client.approve(runID: "run_1", choice: .once, requestID: "req-9")
        #expect(result.resolved == 1)
        #expect(result.choice == .once)
        let request = try #require(server.requests.first)
        #expect(request.method == "POST")
        #expect(request.path == "/v1/runs/run_1/approval")
        #expect(request.header("Content-Type") == "application/json")
        #expect(String(decoding: request.body, as: UTF8.self) == #"{"choice":"once","request_id":"req-9"}"#)

        try await client.approve(runID: "run_1", choice: .deny)
        #expect(server.requests[1].json == ["choice": "deny"])
    }

    @Test func runsCreateGetStopSteer() async throws {
        let server = StubServer { request in
            switch (request.method, request.path) {
            case ("POST", "/v1/runs"):
                return .json(["run_id": "run_abc123", "status": "started"], status: 202, headers: ["Idempotency-Replayed": "true"])
            case ("GET", "/v1/runs/run_abc123"):
                return .json(["object": "hermes.run", "run_id": "run_abc123", "status": "waiting_for_approval", "session_id": "s1",
                              "usage": ["input_tokens": 5], "shutdown_requested_at": 1_760_000_000])
            case ("POST", "/v1/runs/run_abc123/stop"): return .json(["status": "stopping"])
            default: return .json(["ok": true])
            }
        }
        let client = server.client()

        let handle = try await client.createRun(input: MessageInput(text: "Bonjour"), sessionID: "s1", idempotencyKey: "key-1")
        #expect(handle == RunHandle(runID: "run_abc123", status: .running, replayed: true))
        #expect(server.requests[0].header("Idempotency-Key") == "key-1")
        #expect(server.requests[0].json == ["input": "Bonjour", "session_id": "s1"])

        let run = try await client.getRun(id: "run_abc123")
        #expect(run.status == .waitingForApproval)
        #expect(run.sessionID == "s1")
        #expect(run.outcome.usage?.inputTokens == 5)
        #expect(run.shutdownRequestedAt != nil)
        #expect(run.pendingApproval == ApprovalRequest(runID: "run_abc123"))  // no details kept: still answerable

        #expect(try await client.stop(runID: "run_abc123") == .stopping)

        try await client.steer(runID: "run_abc123", text: "plus court")
        #expect(server.requests[3].path == "/v1/runs/run_abc123/steer")
        #expect(server.requests[3].json == ["input": "plus court"])
    }

    @Test func runStatusKeepsThePendingApproval() async throws {
        let server = StubServer { _ in
            .json(["run_id": "run_p", "status": "waiting_for_approval", "last_event": "approval.request",
                   "approval": ["event": "approval.request", "run_id": "run_p", "request_id": "req_7",
                                "command": "rm -rf /tmp/podcast", "description": "delete in root path",
                                "choices": ["once", "session", "always", "deny"]]])
        }
        let approval = try #require(try await server.client().getRun(id: "run_p").pendingApproval)
        #expect(approval.runID == "run_p" && approval.requestID == "req_7" && approval.command == "rm -rf /tmp/podcast")
        #expect(approval.description == "delete in root path" && approval.choices == [.once, .session, .always, .deny])
    }

    @Test func decodesOpenAIStyleErrors() async throws {
        let server = StubServer { _ in
            .json(["error": ["message": "Run has no pending approval: run_1", "type": "invalid_request_error", "code": "approval_not_pending"]], status: 409)
        }
        await #expect(throws: HermesError.http(status: 409, message: "Run has no pending approval: run_1", code: "approval_not_pending")) {
            try await server.client().approve(runID: "run_1", choice: .once)
        }
    }

    @Test func mapsTooManyRunsAndUnauthorized() async throws {
        let busy = StubServer { _ in .json(["error": ["message": "Too many concurrent runs (max 10)"]], status: 429) }
        await #expect(throws: HermesError.tooManyRuns(message: "Too many concurrent runs (max 10)")) {
            _ = try await busy.client().createRun(input: "hi")
        }
        let error = await #expect(throws: HermesError.self) {
            _ = try await busy.client().chatStream(sessionID: "s", input: "hi").collect()
        }
        #expect(error == .tooManyRuns(message: "Too many concurrent runs (max 10)"))
        #expect(error?.isRetryable == true)

        let denied = StubServer { _ in StubResponse(status: 401, headers: [:], chunks: [Data("Unauthorized".utf8)]) }
        await #expect(throws: HermesError.unauthorized(message: "Unauthorized")) {
            _ = try await denied.client().listSessions()
        }
    }

    @Test func mapsTransportFailuresToUnreachable() async throws {
        let server = StubServer { _ in .failure(.notConnectedToInternet) }
        await #expect(throws: HermesError.unreachable(.notConnectedToInternet)) {
            _ = try await server.client().health()
        }
        await #expect(throws: HermesError.unreachable(.notConnectedToInternet)) {
            _ = try await server.client().runEvents(runID: "r").collect()
        }
    }
}
