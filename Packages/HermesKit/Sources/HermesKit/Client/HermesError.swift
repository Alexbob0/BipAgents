import Foundation

public enum HermesError: Error, Sendable, Hashable {
    /// 401 / 403: missing or wrong key.
    case unauthorized(message: String?)
    /// 429: the gateway's concurrent-run cap is reached; back off and retry.
    case tooManyRuns(message: String?)
    /// Any other non-2xx response, with the decoded OpenAI-style `{"error": {"message", "code"}}` body.
    case http(status: Int, message: String?, code: String?)
    /// Transport failure (off-tailnet, DNS, TLS, timeout, connection lost…).
    case unreachable(URLError.Code)
    /// 2xx response whose body could not be interpreted.
    case invalidResponse(String)
    /// The message has document attachments but no `DocumentUploader` was provided.
    case documentUploaderUnavailable
    /// The operation is not implemented by this uploader / server.
    case unsupported(String)

    public var status: Int? {
        switch self {
        case .unauthorized: 401
        case .tooManyRuns: 429
        case .http(let status, _, _): status
        default: nil
        }
    }

    /// Worth retrying with backoff.
    public var isRetryable: Bool {
        switch self {
        case .unreachable, .tooManyRuns: true
        case .http(let status, _, _): status == 408 || status >= 500
        default: false
        }
    }

    public var serverMessage: String? {
        switch self {
        case .unauthorized(let message), .tooManyRuns(let message), .http(_, let message, _): message
        case .invalidResponse(let message), .unsupported(let message): message
        default: nil
        }
    }
}

extension HermesError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case .unauthorized(let message): "Unauthorized" + (message.map { ": \($0)" } ?? "")
        case .tooManyRuns(let message): message ?? "Too many concurrent runs"
        case .http(let status, let message, let code):
            "HTTP \(status)" + (code.map { " [\($0)]" } ?? "") + (message.map { ": \($0)" } ?? "")
        case .unreachable(let code): "Server unreachable (URLError \(code.rawValue))"
        case .invalidResponse(let message): "Invalid response: \(message)"
        case .documentUploaderUnavailable: "No document uploader configured"
        case .unsupported(let message): message
        }
    }
}

/// Shared HTTP plumbing for `HermesClient` and the uploaders.
enum HTTP {
    static func send(_ request: URLRequest, session: URLSession) async throws -> (Data, HTTPURLResponse) {
        let (data, response): (Data, URLResponse)
        do {
            (data, response) = try await session.data(for: request)
        } catch {
            throw mapTransportError(error)
        }
        guard let http = response as? HTTPURLResponse else { throw HermesError.invalidResponse("Not an HTTP response") }
        guard (200..<300).contains(http.statusCode) else { throw error(status: http.statusCode, body: data) }
        return (data, http)
    }

    static func json(_ data: Data) throws -> JSONValue {
        if data.isEmpty { return .null }
        do { return try JSONValue.parse(data) } catch {
            throw HermesError.invalidResponse(String(decoding: data.prefix(200), as: UTF8.self))
        }
    }

    /// Decodes `{"error": {"message", "code"}}`, `{"error": "…"}`, `{"detail": …}` or plain text bodies.
    static func error(status: Int, body: Data) -> HermesError {
        let json = try? JSONValue.parse(body)
        let errorValue = json?["error"]
        let message = errorValue?["message"]?.stringValue
            ?? errorValue?.stringValue
            ?? json?["detail"]?.stringValue
            ?? json?["message"]?.stringValue
            ?? (json == nil ? String(decoding: body.prefix(500), as: UTF8.self).nilIfEmpty : nil)
        let code = errorValue?["code"]?.lenientString ?? errorValue?["type"]?.stringValue
        switch status {
        case 401, 403: return .unauthorized(message: message)
        case 429: return .tooManyRuns(message: message)
        default: return .http(status: status, message: message, code: code)
        }
    }

    static func mapTransportError(_ error: any Error) -> any Error {
        guard let urlError = error as? URLError else { return error }
        return urlError.code == .cancelled ? CancellationError() : HermesError.unreachable(urlError.code)
    }
}
