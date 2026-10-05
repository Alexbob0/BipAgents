import Foundation

/// Client for one Hermes api_server (one agent / profile). Bearer auth with the agent's key.
public struct HermesClient: Sendable {
    public let baseURL: URL
    public let urlSession: URLSession
    private let apiKey: String

    public init(baseURL: URL, apiKey: String, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.apiKey = apiKey
        self.urlSession = session
    }

    // MARK: - Health & capabilities

    public func health() async throws -> HermesHealth {
        let json = try await send("GET", "/health")
        return HermesHealth(status: json["status"]?.stringValue ?? "unknown")
    }

    public func capabilities() async throws -> HermesCapabilities {
        ResponseMapping.capabilities(try await send("GET", "/v1/capabilities"))
    }

    // MARK: - Sessions

    public func listSessions(limit: Int = 50, offset: Int = 0) async throws -> [HermesSession] {
        let json = try await send("GET", "/api/sessions", query: ["limit": "\(limit)", "offset": "\(offset)"])
        return ResponseMapping.list(json, keys: ["sessions"]).compactMap(ResponseMapping.session)
    }

    public func createSession(title: String? = nil) async throws -> HermesSession {
        let body: JSONValue = title.map { ["title": .string($0)] } ?? [:]
        return try sessionOrThrow(try await send("POST", "/api/sessions", body: body))
    }

    public func session(id: String) async throws -> HermesSession {
        try sessionOrThrow(try await send("GET", "/api/sessions/\(Self.escape(id))"))
    }

    public func renameSession(id: String, title: String) async throws {
        _ = try await send("PATCH", "/api/sessions/\(Self.escape(id))", body: ["title": .string(title)])
    }

    public func deleteSession(id: String) async throws {
        _ = try await send("DELETE", "/api/sessions/\(Self.escape(id))")
    }

    /// Transcript. `inlineImages: false` turns images into `[image]` placeholders (kilobytes instead of megabytes).
    public func messages(sessionID: String, includeCompacted: Bool = false, inlineImages: Bool = false) async throws -> [HermesMessage] {
        let json = try await send("GET", "/api/sessions/\(Self.escape(sessionID))/messages",
                                  query: ["include_compacted": "\(includeCompacted)", "inline_images": "\(inlineImages)"])
        return ResponseMapping.list(json, keys: ["messages"]).enumerated()
            .compactMap { ResponseMapping.message($0.element, index: $0.offset) }
    }

    public func forkSession(id: String, title: String? = nil) async throws -> HermesSession {
        let body: JSONValue = title.map { ["title": .string($0)] } ?? [:]
        return try sessionOrThrow(try await send("POST", "/api/sessions/\(Self.escape(id))/fork", body: body))
    }

    // MARK: - Chat

    /// Runs one turn on a session and streams its events (`POST /api/sessions/{id}/chat/stream`).
    /// Documents are uploaded first through `uploader`; upload errors surface through the stream.
    public func chatStream(sessionID: String, input: MessageInput, uploader: (any DocumentUploader)? = nil) -> AsyncThrowingStream<HermesEvent, any Error> {
        eventStream {
            let prepared = try await input.prepared(uploader: uploader)
            return try request("POST", "/api/sessions/\(Self.escape(sessionID))/chat/stream", body: prepared.body(), accept: "text/event-stream")
        }
    }

    /// Streams a turn whose input is already prepared.
    public func chatStream(sessionID: String, prepared: PreparedInput) -> AsyncThrowingStream<HermesEvent, any Error> {
        eventStream {
            try request("POST", "/api/sessions/\(Self.escape(sessionID))/chat/stream", body: prepared.body(), accept: "text/event-stream")
        }
    }

    // MARK: - Runs

    /// `POST /v1/runs`. Pass an `idempotencyKey` (1–255 visible ASCII chars) to make retries safe.
    public func createRun(
        input: PreparedInput,
        sessionID: String? = nil,
        instructions: String? = nil,
        idempotencyKey: String? = nil
    ) async throws -> RunHandle {
        var extra: [String: JSONValue] = [:]
        if let sessionID { extra["session_id"] = .string(sessionID) }
        if let instructions { extra["instructions"] = .string(instructions) }
        var request = try self.request("POST", "/v1/runs", body: input.body(merging: extra))
        if let idempotencyKey { request.setValue(idempotencyKey, forHTTPHeaderField: "Idempotency-Key") }
        let (data, response) = try await HTTP.send(request, session: urlSession)
        let json = try HTTP.json(data)
        guard let runID = json["run_id"]?.lenientString ?? json["id"]?.lenientString else {
            throw HermesError.invalidResponse("Run creation response has no run_id")
        }
        return RunHandle(
            runID: runID,
            status: RunStatus(rawValue: json["status"]?.stringValue ?? "running"),
            replayed: response.value(forHTTPHeaderField: "Idempotency-Replayed")?.lowercased() == "true"
        )
    }

    public func createRun(
        input: MessageInput,
        sessionID: String? = nil,
        instructions: String? = nil,
        idempotencyKey: String? = nil,
        uploader: (any DocumentUploader)? = nil
    ) async throws -> RunHandle {
        try await createRun(input: try await input.prepared(uploader: uploader), sessionID: sessionID,
                            instructions: instructions, idempotencyKey: idempotencyKey)
    }

    public func getRun(id: String) async throws -> HermesRun {
        ResponseMapping.run(try await send("GET", "/v1/runs/\(Self.escape(id))"), fallbackID: id)
    }

    /// `GET /v1/runs/{id}/events` (SSE). Re-subscribable after a disconnect while the run is buffered (5 min).
    public func runEvents(runID: String) -> AsyncThrowingStream<HermesEvent, any Error> {
        eventStream { try request("GET", "/v1/runs/\(Self.escape(runID))/events", accept: "text/event-stream") }
    }

    /// `POST /v1/runs/{id}/approval` with `{"choice": …, "request_id"?: …}`.
    @discardableResult
    public func approve(runID: String, choice: ApprovalChoice, requestID: String? = nil) async throws -> ApprovalResult {
        var body: [String: JSONValue] = ["choice": .string(choice.rawValue)]
        if let requestID { body["request_id"] = .string(requestID) }
        return ResponseMapping.approvalResult(try await send("POST", "/v1/runs/\(Self.escape(runID))/approval", body: .object(body)))
    }

    /// `POST /v1/runs/{id}/clarify` with `{"request_id": …, "answers": {questionID: answer}}`, or
    /// `{"request_id": …, "cancel": true}` when `answers` is nil (docs/hermes-clarify-api.md).
    public func answerClarify(runID: String, requestID: String, answers: [String: String]?) async throws {
        var body: [String: JSONValue] = ["request_id": .string(requestID)]
        if let answers {
            body["answers"] = .object(answers.mapValues { .string($0) })
        } else {
            body["cancel"] = .bool(true)
        }
        _ = try await send("POST", "/v1/runs/\(Self.escape(runID))/clarify", body: .object(body))
    }

    /// `POST /v1/runs/{id}/stop`. Returns immediately with `stopping`; the run settles as `cancelled`.
    @discardableResult
    public func stop(runID: String) async throws -> RunStatus {
        let json = try await send("POST", "/v1/runs/\(Self.escape(runID))/stop", body: [:])
        return RunStatus(rawValue: json["status"]?.stringValue ?? "stopping")
    }

    /// `POST /v1/runs/{id}/steer`: queued guidance delivered at the next tool boundary (409 unless `running`).
    public func steer(runID: String, text: String) async throws {
        _ = try await send("POST", "/v1/runs/\(Self.escape(runID))/steer", body: ["input": .string(text)])
    }

    // MARK: - Plumbing

    func request(_ method: String, _ path: String, query: [String: String] = [:], body: JSONValue? = nil,
                 accept: String = "application/json") throws -> URLRequest {
        guard var components = URLComponents(url: baseURL, resolvingAgainstBaseURL: false) else {
            throw HermesError.invalidResponse("Invalid base URL \(baseURL)")
        }
        let basePath = components.percentEncodedPath.hasSuffix("/") ? String(components.percentEncodedPath.dropLast()) : components.percentEncodedPath
        components.percentEncodedPath = basePath + path
        if !query.isEmpty {
            components.queryItems = query.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        }
        guard let url = components.url else { throw HermesError.invalidResponse("Invalid path \(path)") }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue(accept, forHTTPHeaderField: "Accept")
        if let body {
            request.httpBody = try body.encoded()
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        if accept == "text/event-stream" {
            // Idle timeout: the server sends `: keepalive` every 10 s, so silence means a dead link.
            request.timeoutInterval = 45
        }
        return request
    }

    private func send(_ method: String, _ path: String, query: [String: String] = [:], body: JSONValue? = nil) async throws -> JSONValue {
        let (data, _) = try await HTTP.send(try request(method, path, query: query, body: body), session: urlSession)
        return try HTTP.json(data)
    }

    private func sessionOrThrow(_ json: JSONValue) throws -> HermesSession {
        guard let session = ResponseMapping.session(json) else {
            throw HermesError.invalidResponse("Session payload has no id")
        }
        return session
    }

    private func eventStream(_ makeRequest: @escaping @Sendable () async throws -> URLRequest) -> AsyncThrowingStream<HermesEvent, any Error> {
        let session = urlSession
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let request = try await makeRequest()
                    let (bytes, response): (URLSession.AsyncBytes, URLResponse)
                    do {
                        (bytes, response) = try await session.bytes(for: request)
                    } catch {
                        throw HTTP.mapTransportError(error)
                    }
                    guard let http = response as? HTTPURLResponse else { throw HermesError.invalidResponse("Not an HTTP response") }
                    guard (200..<300).contains(http.statusCode) else {
                        var body = Data()
                        for try await byte in bytes.prefix(64 * 1024) { body.append(byte) }
                        throw HTTP.error(status: http.statusCode, body: body)
                    }
                    do {
                        for try await sse in bytes.sseEvents {
                            if let event = HermesEvent(sse: sse) { continuation.yield(event) }
                        }
                    } catch {
                        throw HTTP.mapTransportError(error)
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Percent-encodes a path segment (ids must not introduce `/`).
    static func escape(_ segment: String) -> String {
        var allowed = CharacterSet.urlPathAllowed
        allowed.remove(charactersIn: "/?#")
        return segment.addingPercentEncoding(withAllowedCharacters: allowed) ?? segment
    }
}
