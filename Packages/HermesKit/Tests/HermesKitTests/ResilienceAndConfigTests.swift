import Foundation
import Synchronization
import Testing
@testable import HermesKit

@Suite("Agent provisioning")
struct AgentProvisioningTests {
    @Test func decodesQRPayloadAndSeparatesSecrets() throws {
        let qr = #"{"name":"Wellness","baseURL":"https://server.example.ts.net:8642","apiKey":"k-123","voice":5476,"bridgeURL":"https://server.example.ts.net:8643","bridgeKey":"b-456"}"#
        let provisioning = try AgentProvisioning(qrPayload: qr)
        #expect(provisioning.config.name == "Wellness")
        #expect(provisioning.config.baseURL.absoluteString == "https://server.example.ts.net:8642")
        #expect(provisioning.config.voice == "5476")
        #expect(provisioning.config.bridgeURL?.port == 8643)
        #expect(provisioning.secrets == AgentSecrets(apiKey: "k-123", bridgeKey: "b-456"))

        // Secrets never end up in the persisted config.
        let encoded = String(decoding: try JSONEncoder().encode(provisioning.config), as: UTF8.self)
        #expect(!encoded.contains("k-123"))
        #expect(!encoded.contains("b-456"))
    }

    @Test func lanDoorComesWithItsCertificate() throws {
        let print = String(repeating: "ab", count: 32)
        let qr = #"{"name":"Vie","baseURL":"https://h.ts.net:8644","apiKey":"k","lan":{"url":"https://192.168.8.10:8650","fingerprint":"\#(print.uppercased())"}}"#
        let provisioning = try AgentProvisioning(qrPayload: qr)
        #expect(provisioning.config.lanURL?.absoluteString == "https://192.168.8.10:8650")
        #expect(provisioning.config.lanFingerprint == print)
        // Without a valid fingerprint the door is ignored (a self-signed certificate must be pinned).
        let unpinned = try AgentProvisioning(qrPayload: #"{"name":"Vie","baseURL":"https://h.ts.net:8644","apiKey":"k","lan":{"url":"https://192.168.8.10:8650"}}"#)
        #expect(unpinned.config.lanURL == nil)
    }

    @Test func minimalPayload() throws {
        let provisioning = try AgentProvisioning(qrPayload: #"{"name":"Vie","baseURL":"https://h.ts.net:8644","apiKey":"k"}"#)
        #expect(provisioning.config.voice == nil)
        #expect(provisioning.config.bridgeURL == nil)
        #expect(provisioning.secrets.bridgeKey == nil)
    }

    @Test func rejectsInvalidPayloads() {
        #expect(throws: AgentConfigError.invalidJSON) { try AgentProvisioning(qrPayload: "not json") }
        #expect(throws: AgentConfigError.missingField("apiKey")) {
            try AgentProvisioning(qrPayload: #"{"name":"X","baseURL":"https://h"}"#)
        }
        #expect(throws: AgentConfigError.invalidURL(field: "baseURL", value: "ftp://h")) {
            try AgentProvisioning(qrPayload: #"{"name":"X","baseURL":"ftp://h","apiKey":"k"}"#)
        }
    }
}

@Suite("Backoff")
struct BackoffTests {
    @Test func growsExponentiallyAndCaps() {
        let backoff = Backoff()
        #expect((0...6).map { backoff.delay(forAttempt: $0, unitRandom: 0.5) } == [0.5, 1, 2, 4, 8, 8, 8])
    }

    @Test func appliesBoundedJitter() {
        let backoff = Backoff()
        #expect(abs(backoff.delay(forAttempt: 0, unitRandom: 0) - 0.4) < 1e-9)
        #expect(abs(backoff.delay(forAttempt: 0, unitRandom: 1) - 0.6) < 1e-9)
        #expect(backoff.delay(forAttempt: 10, unitRandom: 1) == 8) // never above the cap
    }

    @Test func nextAdvancesAndResets() {
        var backoff = Backoff()
        for _ in 0..<3 { _ = backoff.next() }
        #expect(backoff.attempt == 3)
        backoff.reset()
        #expect(backoff.attempt == 0)
        #expect(backoff.next() <= .milliseconds(600))
    }
}

@Suite("RunResumer")
struct RunResumerTests {
    private let fastBackoff = Backoff(initial: 0.001, maximum: 0.002)

    @Test func pollsThenResubscribesUntilTerminal() async throws {
        let server = StubServer { request in
            switch request.path {
            case "/v1/runs/r1": return .json(["run_id": "r1", "status": "running"])
            case "/v1/runs/r1/events":
                return .sse("data: {\"type\":\"message.delta\",\"run_id\":\"r1\",\"delta\":\"Hi\"}\n\ndata: {\"type\":\"run.completed\",\"run_id\":\"r1\",\"output\":\"Hi\"}\n\n")
            default: return .json([:], status: 500)
            }
        }
        let updates = try await RunResumer(client: server.client(), backoff: fastBackoff).resume(runID: "r1").collect()
        #expect(updates.count == 3)
        guard case .status(let run) = updates[0] else { Issue.record("expected status"); return }
        #expect(run.status == .running)
        #expect(updates[1] == .event(HermesEvent(kind: .delta("Hi"), type: "message.delta", runID: "r1",
                                                 raw: ["type": "message.delta", "run_id": "r1", "delta": "Hi"])))
        guard case .event(let last) = updates[2] else { Issue.record("expected event"); return }
        #expect(last.isTerminal)
    }

    @Test func stopsWhenRunAlreadyTerminal() async throws {
        let server = StubServer { _ in .json(["run_id": "r2", "status": "completed", "output": "Done."]) }
        let updates = try await RunResumer(client: server.client(), backoff: fastBackoff).resume(runID: "r2").collect()
        #expect(updates.count == 1)
        #expect(server.requests.count == 1)
    }

    @Test func reportsExpiredRuns() async throws {
        let server = StubServer { _ in .json(["error": ["message": "Run not found"]], status: 404) }
        let updates = try await RunResumer(client: server.client(), backoff: fastBackoff).resume(runID: "gone").collect()
        #expect(updates == [.expired])
    }

    @Test func retriesTransportFailuresThenGivesUp() async throws {
        let server = StubServer { _ in .failure(.networkConnectionLost) }
        let resumer = RunResumer(client: server.client(), backoff: fastBackoff, maxConsecutiveFailures: 2)
        await #expect(throws: HermesError.unreachable(.networkConnectionLost)) {
            _ = try await resumer.resume(runID: "r").collect()
        }
        #expect(server.requests.count == 3)
    }

    @Test func fallsBackToPollingWhenEventBufferExpired() async throws {
        let polls = PollCounter()
        let server = StubServer { request in
            if request.path.hasSuffix("/events") { return .json(["error": ["message": "expired"]], status: 404) }
            return .json(["run_id": "r", "status": polls.next() < 2 ? "running" : "completed"])
        }
        let updates = try await RunResumer(client: server.client(), backoff: fastBackoff).resume(runID: "r").collect()
        #expect(updates.count == 3)
        #expect(server.requests.filter { $0.path.hasSuffix("/events") }.count == 1)
    }
}

final class PollCounter: Sendable {
    private let count = Mutex(0)
    func next() -> Int { count.withLock { defer { $0 += 1 }; return $0 } }
}
