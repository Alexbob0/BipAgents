import Foundation
import HermesKit

/// A proactive message an agent sent on its own (cron check-in…), stored by the bridge's outbox.
struct OutboxItem: Identifiable, Hashable, Sendable, Codable {
    var id: String
    var agent: String
    var title: String?
    var text: String
    var createdAt: Date
    var sessionID: String?
    var hasAudio: Bool
}

/// Client for the agent's bridge (voice, files, push, outbox). Base URL and key come from the agent's settings.
struct BridgeClient: Sendable {
    var baseURL: URL
    var key: String
    var session: URLSession = .shared

    /// On the tailnet, or through the LAN door when the tailnet is down (see `LocalLink`).
    init?(agent: AgentProfile, secrets: AgentSecrets?) {
        guard let url = LocalLink.bridgeURL(for: agent), let key = secrets?.bridgeKey else { return nil }
        baseURL = url
        self.key = key
        session = LocalLink.session(for: agent)
    }

    /// The bridge on the tailnet, whatever the current route (to refresh the LAN door's details).
    init?(tailnetOf agent: AgentProfile, secrets: AgentSecrets?) {
        guard let url = agent.config.bridgeURL, let key = secrets?.bridgeKey else { return nil }
        baseURL = url
        self.key = key
    }

    /// A bridge reached by its address and key alone (an install's QR code, before any agent exists here).
    init(url: URL, key: String) {
        baseURL = url
        self.key = key
    }

    /// One agent of an install, as `GET /v1/agents` lists it.
    struct InstallAgent: Sendable {
        var id: String
        var name: String
        var url: URL
        var key: String
    }

    /// `GET /v1/agents`: every agent of this install that the app can reach (with its tailnet address and key).
    func installAgents() async throws -> [InstallAgent] {
        let json = try JSONSerialization.jsonObject(with: try await send(request("v1/agents"))) as? [String: Any]
        return (json?["agents"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = row["id"] as? String, let raw = row["url"] as? String, let url = URL(string: raw),
                  let key = row["key"] as? String, !key.isEmpty else { return nil }
            return InstallAgent(id: id, name: row["name"] as? String ?? id.capitalized, url: url, key: key)
        }
    }

    /// A kind of agent the bridge can create (`custom` = « Autre », described by the person).
    struct AgentKind: Sendable, Identifiable, Hashable {
        var id: String
        var label: String
        var description: String
        var custom: Bool
    }

    /// `GET /v1/agent-kinds`: nil when this bridge cannot create agents (set up by hand, or an older bridge).
    func agentKinds() async throws -> [AgentKind]? {
        let json = try JSONSerialization.jsonObject(with: try await send(request("v1/agent-kinds"))) as? [String: Any]
        guard json?["can_create"] as? Bool == true else { return nil }
        return (json?["kinds"] as? [[String: Any]] ?? []).compactMap { row in
            guard let id = row["id"] as? String else { return nil }
            return AgentKind(id: id, label: row["label"] as? String ?? id, description: row["description"] as? String ?? "",
                             custom: row["custom"] as? Bool ?? false)
        }
    }

    /// What the person wrote for an « Autre » agent.
    struct CustomAgent: Sendable {
        var role: String
        var tone: String
        var rules: String
        var language: String
        var category: String
    }

    /// `POST /v1/agents`: a new agent on this install (the gateway takes up to half a minute to serve it).
    func createAgent(name: String, kind: String?, custom: CustomAgent?) async throws -> InstallAgent {
        var request = request("v1/agents", method: "POST")
        request.timeoutInterval = 90
        var body: [String: Any] = ["name": name]
        if let kind { body["kind"] = kind }
        if let custom {
            body["custom"] = ["role": custom.role, "tone": custom.tone, "rules": custom.rules,
                              "language": custom.language, "category": custom.category]
        }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let json = try JSONSerialization.jsonObject(with: try await send(request)) as? [String: Any]
        guard let id = json?["id"] as? String, let raw = json?["url"] as? String, let url = URL(string: raw),
              let key = json?["key"] as? String else { throw HermesError.invalidResponse("agent creation") }
        return InstallAgent(id: id, name: json?["name"] as? String ?? name, url: url, key: key)
    }

    /// One group of built-in tools of an agent (`web`, `terminal`…), as Hermes lists it.
    struct Toolset: Sendable, Identifiable, Hashable {
        var id: String
        var label: String
        var description: String
        var enabled: Bool
        var configured: Bool
    }

    /// What can be changed on an agent this install created: its personality and its tools.
    struct AgentSettings: Sendable {
        var soul: String
        var toolsets: [Toolset]
        /// In its own space (not the shared hub): it may hold credentials.
        var isolated: Bool
    }

    /// `GET /v1/agents/{id}/settings`: nil when this bridge does not manage the agent (set up by hand, or an older bridge).
    func agentSettings(_ agent: String) async throws -> AgentSettings? {
        do {
            return Self.settings(from: try await send(request("v1/agents/\(agent)/settings")))
        } catch HermesError.http(let status, _, _) where status == 404 || status == 405 || status == 409 {
            return nil
        }
    }

    /// `PATCH /v1/agents/{id}/settings`: a new personality and/or tools turned on or off. Hermes uses them from the next message.
    @discardableResult
    func updateAgentSettings(_ agent: String, soul: String? = nil, toolsets: [String: Bool]? = nil) async throws -> AgentSettings? {
        var request = request("v1/agents/\(agent)/settings", method: "PATCH")
        request.timeoutInterval = 60
        var body: [String: Any] = [:]
        if let soul { body["soul"] = soul }
        if let toolsets { body["toolsets"] = toolsets }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        return Self.settings(from: try await send(request))
    }

    /// `POST /v1/agents/{id}/space`: the agent moves out of the shared hub into its own space (so it may hold
    /// credentials). Its new address on the tailnet (through the bridge); about half a minute.
    func moveToOwnSpace(_ agent: String) async throws -> URL? {
        var request = request("v1/agents/\(agent)/space", method: "POST")
        request.timeoutInterval = 180
        let json = try JSONSerialization.jsonObject(with: try await send(request)) as? [String: Any]
        return (json?["url"] as? String).flatMap(URL.init(string:))
    }

    /// `DELETE /v1/agents/{id}`: the agent, its conversations, memory and routines, gone from the install.
    func deleteAgent(_ agent: String) async throws {
        var request = request("v1/agents/\(agent)", method: "DELETE")
        request.timeoutInterval = 60
        try await send(request)
    }

    private static func settings(from data: Data) -> AgentSettings? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let soul = json["soul"] as? String else { return nil }
        let toolsets = (json["toolsets"] as? [[String: Any]] ?? []).compactMap { row -> Toolset? in
            guard let id = row["name"] as? String else { return nil }
            return Toolset(id: id, label: row["label"] as? String ?? id, description: row["description"] as? String ?? "",
                           enabled: row["enabled"] as? Bool ?? false, configured: row["configured"] as? Bool ?? true)
        }
        return AgentSettings(soul: soul, toolsets: toolsets, isolated: json["isolated"] as? Bool ?? false)
    }

    /// A credential of the agents' vault, as the app sees it: never its password.
    struct VaultItem: Sendable, Identifiable, Hashable {
        var id: String
        var label: String
        var origin: String
        var identifier: String
        var hasOTP: Bool
        var agents: [String]
    }

    /// An agent of this install, and whether it may hold credentials (`why`: needs_own_space, not_configured).
    struct VaultAgent: Sendable, Identifiable, Hashable {
        var id: String
        var name: String
        var canHold: Bool
        var why: String?
    }

    /// What the person typed; nil fields are left as they are (a password left empty is kept).
    struct VaultLogin: Sendable {
        var label: String?
        var site: String?
        var identifier: String?
        var password: String?
        var otpSecret: String?
        var agents: [String]?
    }

    /// `GET /v1/vault`: nil when this bridge has no vault (older bridge).
    func vault() async throws -> (items: [VaultItem], agents: [VaultAgent])? {
        let data: Data
        do {
            data = try await send(request("v1/vault"))
        } catch HermesError.http(let status, _, _) where status == 404 {
            return nil
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let items = (json?["items"] as? [[String: Any]] ?? []).compactMap(Self.vaultItem)
        let agents = (json?["agents"] as? [[String: Any]] ?? []).compactMap { row -> VaultAgent? in
            guard let id = row["id"] as? String else { return nil }
            return VaultAgent(id: id, name: row["name"] as? String ?? id, canHold: row["can_hold"] as? Bool ?? false,
                              why: row["why"] as? String)
        }
        return (items, agents)
    }

    /// The agents that cannot hold credentials yet, from a 409 (agent id -> reason).
    struct CannotHold: Error, Sendable {
        var agents: [String: String]
    }

    /// `POST /v1/vault` (new) or `PATCH /v1/vault/{id}`.
    @discardableResult
    func saveVaultItem(_ login: VaultLogin, id: String? = nil) async throws -> VaultItem {
        var request = request(id.map { "v1/vault/\($0)" } ?? "v1/vault", method: id == nil ? "POST" : "PATCH")
        request.timeoutInterval = 60
        var body: [String: Any] = ["kind": "login"]
        if let value = login.label { body["label"] = value }
        if let value = login.site { body["site"] = value }
        if let value = login.identifier {
            body["identifier"] = value
            body["identifier_type"] = value.contains("@") ? "email"
                : value.allSatisfy({ $0.isNumber || "+ ".contains($0) }) ? "phone" : "username"
        }
        if let value = login.password, !value.isEmpty { body["password"] = value }
        if let value = login.otpSecret { body["otp_secret"] = value }
        if let value = login.agents { body["agents"] = value }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await session.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 409, let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let agents = (json["detail"] as? [String: Any])?["agents"] as? [String: String] {
            throw CannotHold(agents: agents)
        }
        guard (200..<300).contains(status) else {
            if status == 401 || status == 403 { throw HermesError.unauthorized(message: String(localized: "Clé du bridge refusée")) }
            let detail = (try? JSONSerialization.jsonObject(with: data) as? [String: Any])?["detail"] as? String
            throw HermesError.http(status: status, message: detail, code: nil)
        }
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              let item = Self.vaultItem(json) else { throw HermesError.invalidResponse("vault item") }
        return item
    }

    func deleteVaultItem(_ id: String) async throws {
        var request = request("v1/vault/\(id)", method: "DELETE")
        request.timeoutInterval = 60
        try await send(request)
    }

    private static func vaultItem(_ row: [String: Any]) -> VaultItem? {
        guard let id = row["id"] as? String else { return nil }
        return VaultItem(id: id, label: row["label"] as? String ?? "", origin: row["origin"] as? String ?? "",
                         identifier: row["identifier"] as? String ?? "", hasOTP: row["has_otp"] as? Bool ?? false,
                         agents: row["agents"] as? [String] ?? [])
    }

    /// `GET /v1/status`: whether the model answers (nil when the bridge does not check it, or is older).
    func modelIsUp() async throws -> Bool? {
        let json = try JSONSerialization.jsonObject(with: try await send(request("v1/status"))) as? [String: Any]
        return (json?["model"] as? [String: Any])?["ok"] as? Bool
    }

    /// `GET /v1/pairing`: the LAN door as the bridge sees it now (nil when it is off).
    func pairing() async throws -> (url: URL, fingerprint: String)? {
        let json = try JSONSerialization.jsonObject(with: try await send(request("v1/pairing"))) as? [String: Any]
        guard let lan = json?["lan"] as? [String: Any], let raw = lan["url"] as? String, let url = URL(string: raw),
              let fingerprint = lan["fingerprint"] as? String, fingerprint.count == 64 else { return nil }
        return (url, fingerprint.lowercased())
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
    /// The bridge keeps watching the run (pushes its approvals and reply when nobody follows it), across restarts.
    func watch(agent: String, runID: String, sessionID: String? = nil) async throws {
        var request = request("v1/watch", method: "POST")
        var body = ["agent": agent, "run_id": runID]
        if let sessionID { body["session_id"] = sessionID }
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        try await send(request)
    }

    /// `GET /v1/runs/{id}/events`: the run's Hermes events relayed by the bridge, replayed from the start on each
    /// connection. While the app listens, the bridge pushes nothing; once it leaves, approvals and « reply ready »
    /// are pushed. Throws `HermesError.http(404…)` on a bridge without this route.
    func runEvents(agent: String, runID: String, sessionID: String?) -> AsyncThrowingStream<HermesEvent, any Error> {
        var components = URLComponents(url: baseURL.appending(path: "v1/runs/\(runID)/events"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "agent", value: agent)]
            + (sessionID.map { [URLQueryItem(name: "session_id", value: $0)] } ?? [])
        var request = URLRequest(url: components.url!, timeoutInterval: 60) // keepalives every 10 s
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        authorize(&request)
        let session = session
        let (stream, continuation) = AsyncThrowingStream<HermesEvent, any Error>.makeStream()
        let task = Task {
            do {
                let (bytes, response) = try await session.bytes(for: request)
                let status = (response as? HTTPURLResponse)?.statusCode ?? 0
                guard (200..<300).contains(status) else {
                    if status == 401 || status == 403 { throw HermesError.unauthorized(message: String(localized: "Clé du bridge refusée")) }
                    throw HermesError.http(status: status, message: nil, code: nil)
                }
                for try await sse in bytes.sseEvents {
                    if let event = HermesEvent(sse: sse) { continuation.yield(event) }
                }
                continuation.finish()
            } catch let error as URLError {
                continuation.finish(throwing: error.code == .cancelled ? CancellationError() : HermesError.unreachable(error.code))
            } catch {
                continuation.finish(throwing: error)
            }
        }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }

    /// `GET /v1/sessions/{id}/state`: the conversation's message count, and « it is on screen » for the bridge
    /// (no push for what the user is looking at).
    func sessionMessageCount(agent: String, sessionID: String) async throws -> Int? {
        var components = URLComponents(url: baseURL.appending(path: "v1/sessions/\(sessionID)/state"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "agent", value: agent)]
        var request = URLRequest(url: components.url!)
        authorize(&request)
        let json = try JSONSerialization.jsonObject(with: try await send(request)) as? [String: Any]
        return json?["message_count"] as? Int
    }

    /// A file an agent pointed to with « MEDIA:<path> » (`GET /v1/media`).
    func media(path: String) async throws -> Data {
        var components = URLComponents(url: baseURL.appending(path: "v1/media"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "path", value: path)]
        var request = URLRequest(url: components.url!, timeoutInterval: 60)
        authorize(&request)
        return try await send(request)
    }

    /// A scheduled task the bridge has seen, and whether its replies are pushed.
    struct CronJob: Identifiable, Hashable, Sendable {
        var agent: String
        var job: String
        var name: String?
        var notify: Bool
        var lastSeen: Date?
        var id: String { "\(agent)/\(job)" }
    }

    func cronJobs(agent: String) async throws -> [CronJob] {
        var components = URLComponents(url: baseURL.appending(path: "v1/cron-jobs"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "agent", value: agent)]
        var request = URLRequest(url: components.url!)
        authorize(&request)
        let json = try JSONSerialization.jsonObject(with: try await send(request)) as? [String: Any]
        return (json?["jobs"] as? [[String: Any]] ?? []).compactMap { row in
            guard let agent = row["agent"] as? String, let job = row["job"] as? String else { return nil }
            return CronJob(agent: agent, job: job, name: row["name"] as? String, notify: row["notify"] as? Bool ?? true,
                           lastSeen: (row["last_seen"] as? String).flatMap(Self.parseDate))
        }
    }

    /// Approvals the bridge saw and nobody answered yet (`GET /v1/approvals`).
    func pendingApprovals(agent: String) async throws -> [ApprovalRequest] {
        var components = URLComponents(url: baseURL.appending(path: "v1/approvals"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "agent", value: agent)]
        var request = URLRequest(url: components.url!)
        authorize(&request)
        let json = try JSONSerialization.jsonObject(with: try await send(request)) as? [String: Any]
        return (json?["items"] as? [[String: Any]] ?? []).compactMap { row in
            guard let runID = row["run_id"] as? String else { return nil }
            let choices = (row["choices"] as? [String] ?? []).compactMap(ApprovalChoice.init(lenient:))
            return ApprovalRequest(runID: runID, requestID: row["request_id"] as? String, command: row["command"] as? String,
                                   description: row["description"] as? String, choices: choices.isEmpty ? [.once, .deny] : choices,
                                   sessionID: row["session_id"] as? String)
        }
    }

    func setCronNotify(agent: String, job: String, notify: Bool) async throws {
        var request = request("v1/cron-jobs/\(agent)/\(job)", method: "PUT")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["notify": notify])
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
        throw HermesError.unsupported(String(localized: "Audio pas encore prêt"))
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
            if status == 401 || status == 403 { throw HermesError.unauthorized(message: String(localized: "Clé du bridge refusée")) }
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
    /// The agent's key in the bridge configuration (`wellness`, `vie`…): as the bridge gave it, else from the name.
    nonisolated var bridgeName: String {
        config.bridgeAgent ?? name.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil).replacing(" ", with: "-")
    }
}
