import Foundation
import Synchronization
@testable import HermesKit

/// A canned HTTP response; `chunks` are delivered one by one to exercise incremental parsing.
struct StubResponse: Sendable {
    var status = 200
    var headers: [String: String] = ["Content-Type": "application/json"]
    var chunks: [Data] = []
    var error: URLError?

    static func json(_ json: JSONValue, status: Int = 200, headers: [String: String] = [:]) -> StubResponse {
        StubResponse(status: status, headers: ["Content-Type": "application/json"].merging(headers) { _, new in new },
                     chunks: [(try? json.encoded()) ?? Data()])
    }

    static func sse(_ text: String, chunkSize: Int = 7) -> StubResponse {
        let bytes = Array(text.utf8)
        let chunks = stride(from: 0, to: bytes.count, by: chunkSize).map { Data(bytes[$0..<min($0 + chunkSize, bytes.count)]) }
        return StubResponse(status: 200, headers: ["Content-Type": "text/event-stream"], chunks: chunks)
    }

    static func failure(_ code: URLError.Code) -> StubResponse {
        StubResponse(error: URLError(code))
    }
}

struct RecordedRequest: Sendable {
    var method: String
    var path: String
    var query: [String: String]
    var headers: [String: String]
    var body: Data

    var json: JSONValue? { try? JSONValue.parse(body) }
    func header(_ name: String) -> String? {
        headers.first { $0.key.caseInsensitiveCompare(name) == .orderedSame }?.value
    }
}

/// Routes requests by host to per-test handlers, so tests can run in parallel.
final class StubURLProtocol: URLProtocol, @unchecked Sendable {
    typealias Handler = @Sendable (RecordedRequest) -> StubResponse

    static let handlers = Mutex<[String: Handler]>([:])

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}

    override func startLoading() {
        guard let url = request.url, let host = url.host(),
              let handler = Self.handlers.withLock({ $0[host] }) else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotFindHost))
            return
        }
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
        let recorded = RecordedRequest(
            method: request.httpMethod ?? "GET",
            path: components?.percentEncodedPath ?? url.path(),
            query: Dictionary((components?.queryItems ?? []).map { ($0.name, $0.value ?? "") }, uniquingKeysWith: { $1 }),
            headers: request.allHTTPHeaderFields ?? [:],
            body: request.httpBody ?? request.httpBodyStream.map(Self.readAll) ?? Data()
        )
        let response = handler(recorded)
        if let error = response.error {
            client?.urlProtocol(self, didFailWithError: error)
            return
        }
        let http = HTTPURLResponse(url: url, statusCode: response.status, httpVersion: "HTTP/1.1", headerFields: response.headers)!
        client?.urlProtocol(self, didReceive: http, cacheStoragePolicy: .notAllowed)
        for chunk in response.chunks { client?.urlProtocol(self, didLoad: chunk) }
        client?.urlProtocolDidFinishLoading(self)
    }

    private static func readAll(_ stream: InputStream) -> Data {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            if count <= 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}

/// One fake server per test: a unique host, a URLSession routed to the stub, and a request log.
final class StubServer: Sendable {
    let host = "stub-\(UUID().uuidString.lowercased()).test"
    let session: URLSession
    private let log = Mutex<[RecordedRequest]>([])

    var baseURL: URL { URL(string: "https://\(host):8642")! }
    var requests: [RecordedRequest] { log.withLock { $0 } }

    init(_ handler: @escaping @Sendable (RecordedRequest) -> StubResponse) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        session = URLSession(configuration: configuration)
        StubURLProtocol.handlers.withLock { [host] handlers in
            handlers[host] = { [weak self] request in
                self?.log.withLock { $0.append(request) }
                return handler(request)
            }
        }
    }

    deinit {
        StubURLProtocol.handlers.withLock { [host] in $0[host] = nil }
    }

    func client(apiKey: String = "test-key") -> HermesClient {
        HermesClient(baseURL: baseURL, apiKey: apiKey, session: session)
    }
}

enum Fixtures {
    static func string(_ name: String) throws -> String {
        guard let url = Bundle.module.url(forResource: name, withExtension: nil, subdirectory: "Fixtures") else {
            throw CocoaError(.fileNoSuchFile)
        }
        return try String(contentsOf: url, encoding: .utf8)
    }
}

extension AsyncThrowingStream {
    func collect() async throws -> [Element] {
        var result: [Element] = []
        for try await element in self { result.append(element) }
        return result
    }
}
