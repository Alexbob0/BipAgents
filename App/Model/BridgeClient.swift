import Foundation
import HermesKit

/// A proactive message an agent sent on its own (cron check-in…), stored by the bridge's outbox.
struct OutboxItem: Identifiable, Hashable, Sendable {
    var id: String
    var agent: String
    var title: String?
    var text: String
    var createdAt: Date
    var sessionID: String?
    var hasAudio: Bool
}

/// Client for the aibox bridge (voice, files, push, outbox). Base URL and key come from the agent's settings.
struct BridgeClient: Sendable {
    var baseURL: URL
    var key: String
    var session: URLSession = .shared

    init?(agent: AgentProfile, secrets: AgentSecrets?) {
        guard let url = agent.config.bridgeURL, let key = secrets?.bridgeKey else { return nil }
        baseURL = url
        self.key = key
    }

    func health() async throws -> Bool {
        let (_, response) = try await session.data(for: request("health", authenticated: false))
        return (response as? HTTPURLResponse)?.statusCode == 200
    }

    func registerDevice(token: String, environment: String, agents: [String]) async throws {
        var request = request("v1/devices", method: "POST")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["token": token, "environment": environment, "agent_ids": agents])
        try await send(request)
    }

    /// Ask the bridge to watch a run so an approval request still reaches the phone if the app is closed.
    func watch(agent: String, runID: String) async throws {
        var request = request("v1/watch", method: "POST")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["agent": agent, "run_id": runID])
        try await send(request)
    }

    func outbox(since: Date? = nil, agent: String? = nil) async throws -> [OutboxItem] {
        var components = URLComponents(url: baseURL.appending(path: "v1/outbox"), resolvingAgainstBaseURL: false)!
        var items: [URLQueryItem] = []
        if let since { items.append(URLQueryItem(name: "since", value: since.ISO8601Format())) }
        if let agent { items.append(URLQueryItem(name: "agent", value: agent)) }
        components.queryItems = items.isEmpty ? nil : items
        var request = URLRequest(url: components.url!)
        authorize(&request)
        let data = try await send(request)
        let json = try JSONSerialization.jsonObject(with: data)
        let rows = (json as? [[String: Any]]) ?? ((json as? [String: Any])?["items"] as? [[String: Any]]) ?? []
        return rows.compactMap(Self.item(from:))
    }

    /// A whole reply as one mp3 (voice-message mode): Kyutai batches the sentences, so this is much faster
    /// per second of audio than the call mode's sentence-by-sentence synthesis.
    func messageAudio(text: String, agent: String, voice: String) async throws -> Data {
        var request = request("v1/tts/message", method: "POST")
        request.timeoutInterval = 180
        request.httpBody = try JSONSerialization.data(withJSONObject: ["text": text, "agent": agent, "voice": voice, "format": "mp3"])
        return try await send(request)
    }

    /// The mp3 may still be synthesizing: the bridge answers 202 + Retry-After until it is ready.
    func audio(forOutboxItem id: String, attempts: Int = 5) async throws -> Data {
        for _ in 0..<attempts {
            let (data, response) = try await session.data(for: request("v1/outbox/\(id)/audio"))
            let http = response as? HTTPURLResponse
            switch http?.statusCode {
            case 200: return data
            case 202:
                let delay = Double(http?.value(forHTTPHeaderField: "Retry-After") ?? "") ?? 3
                try await Task.sleep(for: .seconds(delay))
            default:
                throw HermesError.http(status: http?.statusCode ?? 0, message: String(data: data, encoding: .utf8), code: nil)
            }
        }
        throw HermesError.unsupported("Audio pas encore prêt")
    }

    // MARK: Plumbing

    private func request(_ path: String, method: String = "GET", authenticated: Bool = true) -> URLRequest {
        var request = URLRequest(url: baseURL.appending(path: path), timeoutInterval: 15)
        request.httpMethod = method
        if method != "GET" { request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        if authenticated { authorize(&request) }
        return request
    }

    private func authorize(_ request: inout URLRequest) {
        request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
    }

    @discardableResult
    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard (200..<300).contains(status) else {
            if status == 401 || status == 403 { throw HermesError.unauthorized(message: "Clé du bridge refusée") }
            throw HermesError.http(status: status, message: String(data: data, encoding: .utf8), code: nil)
        }
        return data
    }

    /// The bridge sends `2026-10-04T08:15:01.532Z` (milliseconds); accept plain seconds too.
    static func parseDate(_ string: String) -> Date? {
        (try? Date(string, strategy: Date.ISO8601FormatStyle(includingFractionalSeconds: true)))
            ?? (try? Date(string, strategy: .iso8601))
    }

    private static func item(from row: [String: Any]) -> OutboxItem? {
        guard let id = (row["id"] as? String) ?? (row["id"] as? Int).map(String.init),
              let text = (row["text"] ?? row["body"] ?? row["message"]) as? String else { return nil }
        let created = (row["created_at"] as? String).flatMap(Self.parseDate)
            ?? (row["created_at"] as? Double).map { Date(timeIntervalSince1970: $0) }
            ?? .now
        return OutboxItem(
            id: id,
            agent: row["agent"] as? String ?? "",
            title: row["title"] as? String,
            text: text,
            createdAt: created,
            sessionID: row["session_id"] as? String,
            hasAudio: (row["audio_path"] as? String) != nil || (row["has_audio"] as? Bool) == true
        )
    }
}

extension AgentProfile {
    /// The agent's key in the bridge configuration (`wellness`, `vie`…).
    var bridgeName: String {
        name.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil).replacing(" ", with: "-")
    }
}
