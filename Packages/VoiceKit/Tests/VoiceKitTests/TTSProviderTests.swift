import Foundation
import Synchronization
import Testing
@testable import VoiceKit

// MARK: - Bridge

/// URLProtocol stub; each test uses its own host so suites can run in parallel.
private final class StubURLProtocol: URLProtocol, @unchecked Sendable {
    struct Reply: Sendable {
        var status: Int
        var headers: [String: String]
        var body: Data
    }

    static let replies = Mutex<[String: Reply]>([:])
    /// Per "host/path" overrides of `replies`.
    static let pathReplies = Mutex<[String: Reply]>([:])
    static let requests = Mutex<[String: (URLRequest, Data)]>([:])

    static func session(host: String, reply: Reply) -> URLSession {
        replies.withLock { $0[host] = reply }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [StubURLProtocol.self]
        return URLSession(configuration: configuration)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let host = request.url?.host() ?? ""
        Self.requests.withLock { $0[host] = (request, Self.body(of: request)) }
        let path = request.url?.path() ?? ""
        guard let reply = Self.pathReplies.withLock({ $0[host + path] }) ?? Self.replies.withLock({ $0[host] }),
              let response = HTTPURLResponse(url: request.url!, statusCode: reply.status, httpVersion: "HTTP/1.1", headerFields: reply.headers) else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: reply.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    /// URLSession moves `httpBody` into a stream before it reaches the protocol.
    private static func body(of request: URLRequest) -> Data {
        if let body = request.httpBody { return body }
        guard let stream = request.httpBodyStream else { return Data() }
        stream.open()
        defer { stream.close() }
        var data = Data()
        var chunk = [UInt8](repeating: 0, count: 4096)
        while stream.hasBytesAvailable {
            let count = stream.read(&chunk, maxLength: chunk.count)
            guard count > 0 else { break }
            data.append(chunk, count: count)
        }
        return data
    }
}

@Suite("Bridge TTS")
struct BridgeTTSProviderTests {
    private func provider(host: String, reply: StubURLProtocol.Reply) -> BridgeTTSProvider {
        BridgeTTSProvider(bridgeURL: URL(string: "https://\(host):8643")!, bridgeKey: "secret-key", voice: "5476",
                          session: StubURLProtocol.session(host: host, reply: reply))
    }

    @Test func sendsSentenceRequestAndParsesSampleRate() async throws {
        let pcm = Data([0x01, 0x00, 0xFF, 0x7F, 0x00, 0x80])
        let tts = provider(host: "shape.test", reply: .init(status: 200, headers: ["X-Sample-Rate": "16000", "X-Channels": "1"], body: pcm))
        let chunk = try await tts.synthesize("Bonjour Sam.")
        #expect(chunk == PCMChunk(samples: pcm, sampleRate: 16_000))
        #expect(chunk.frameCount == 3)

        let (request, body) = try #require(StubURLProtocol.requests.withLock { $0["shape.test"] })
        #expect(request.httpMethod == "POST")
        #expect(request.url?.absoluteString == "https://shape.test:8643/v1/tts/sentence")
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer secret-key")
        #expect(request.value(forHTTPHeaderField: "Content-Type") == "application/json")
        let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: String])
        #expect(json == ["text": "Bonjour Sam.", "voice": "5476", "format": "pcm16"])
    }

    @Test func defaultsTo24kHzWithoutHeader() async throws {
        let tts = provider(host: "default-rate.test", reply: .init(status: 200, headers: [:], body: Data(count: 480)))
        let chunk = try await tts.synthesize("Salut.")
        #expect(chunk.sampleRate == 24_000)
        #expect(chunk.duration == .milliseconds(10))
    }

    @Test func ignoresInvalidSampleRateAndOddTrailingByte() async throws {
        let tts = provider(host: "odd.test", reply: .init(status: 200, headers: ["X-Sample-Rate": "abc"], body: Data([1, 2, 3])))
        let chunk = try await tts.synthesize("Salut.")
        #expect(chunk.sampleRate == 24_000)
        #expect(chunk.samples == Data([1, 2]))
    }

    @Test func throwsOnHTTPError() async {
        let tts = provider(host: "down.test", reply: .init(status: 503, headers: [:], body: Data()))
        await #expect(throws: TTSError.httpStatus(503)) { try await tts.synthesize("Salut.") }
    }

    @Test func streamsChunksFromTheStreamRoute() async throws {
        var pcm = Data((0..<10_001).map { UInt8(truncatingIfNeeded: $0) })
        let tts = provider(host: "stream.test", reply: .init(status: 200, headers: ["X-Sample-Rate": "24000"], body: pcm))
        #expect(tts.streamsAudio)
        var chunks: [PCMChunk] = []
        for try await chunk in tts.stream("Bonjour Sam.") { chunks.append(chunk) }
        let (request, body) = try #require(StubURLProtocol.requests.withLock { $0["stream.test"] })
        #expect(request.url?.path() == "/v1/tts/stream")
        let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: String])
        #expect(json == ["text": "Bonjour Sam.", "voice": "5476", "format": "pcm16"])
        #expect(chunks.first?.samples.count == 4_800) // small first chunk: playback starts at once
        #expect(chunks.allSatisfy { $0.samples.count.isMultiple(of: 2) && $0.sampleRate == 24_000 })
        pcm.removeLast() // odd trailing byte dropped
        #expect(chunks.reduce(Data()) { $0 + $1.samples } == pcm)
    }

    @Test func streamFallsBackToSentenceRouteOnOlderBridge() async throws {
        let pcm = Data([1, 0, 2, 0])
        StubURLProtocol.pathReplies.withLock { $0["old.test/v1/tts/stream"] = .init(status: 404, headers: [:], body: Data()) }
        let tts = provider(host: "old.test", reply: .init(status: 200, headers: [:], body: pcm))
        var chunks: [PCMChunk] = []
        for try await chunk in tts.stream("Salut.") { chunks.append(chunk) }
        #expect(chunks == [PCMChunk(samples: pcm, sampleRate: 24_000)])
        #expect(StubURLProtocol.requests.withLock { $0["old.test"] }?.0.url?.path() == "/v1/tts/sentence")
    }

    @Test func streamThrowsOnHTTPError() async {
        let tts = provider(host: "stream-down.test", reply: .init(status: 502, headers: [:], body: Data()))
        await #expect(throws: TTSError.httpStatus(502)) { for try await _ in tts.stream("Salut.") {} }
    }

    @Test func throwsWhenUnreachable() async {
        let session = StubURLProtocol.session(host: "other.test", reply: .init(status: 200, headers: [:], body: Data()))
        let tts = BridgeTTSProvider(bridgeURL: URL(string: "https://unreachable.test")!, bridgeKey: "k", voice: "v", session: session)
        await #expect(throws: URLError.self) { try await tts.synthesize("Salut.") }
    }
}

// MARK: - Fallback

/// Scripted TTS: records calls, fails or succeeds on demand.
final class ScriptedTTS: TTSProvider {
    enum Behaviour: Sendable { case succeed, fail, reject, cancel }
    let name: String
    private let state: Mutex<(behaviour: Behaviour, calls: [String], replies: Int)>

    init(_ name: String, _ behaviour: Behaviour = .succeed) {
        self.name = name
        state = Mutex((behaviour, [], 0))
    }

    var calls: [String] { state.withLock { $0.calls } }
    var replies: Int { state.withLock { $0.replies } }
    func set(_ behaviour: Behaviour) { state.withLock { $0.behaviour = behaviour } }

    func beginReply() { state.withLock { $0.replies += 1 } }

    func synthesize(_ sentence: String) async throws -> PCMChunk {
        let behaviour = state.withLock { $0.calls.append(sentence); return $0.behaviour }
        switch behaviour {
        case .succeed: return PCMChunk(samples: Data(name.utf8), sampleRate: 24_000)
        case .fail: throw URLError(.cannotConnectToHost)
        case .reject: throw TTSError.httpStatus(400)
        case .cancel: throw CancellationError()
        }
    }
}

@Suite("Fallback TTS")
struct FallbackTTSProviderTests {
    @Test func usesPrimaryWhileItWorks() async throws {
        let primary = ScriptedTTS("bridge"), fallback = ScriptedTTS("system")
        let tts = FallbackTTSProvider(primary: primary, fallback: fallback)
        let chunk = try await tts.synthesize("Un.")
        #expect(chunk.samples == Data("bridge".utf8))
        #expect(fallback.calls.isEmpty)
        #expect(!tts.isUsingFallback)
    }

    @Test func fallsBackForTheFailingSentenceAndTheRestOfTheReply() async throws {
        let primary = ScriptedTTS("bridge", .fail), fallback = ScriptedTTS("system")
        let tts = FallbackTTSProvider(primary: primary, fallback: fallback)
        tts.beginReply()
        #expect(try await tts.synthesize("Un.").samples == Data("system".utf8))
        primary.set(.succeed) // bridge back up mid-reply: keep the same voice until the reply ends
        #expect(try await tts.synthesize("Deux.").samples == Data("system".utf8))
        #expect(primary.calls == ["Un."])
        #expect(fallback.calls == ["Un.", "Deux."])
        #expect(tts.isUsingFallback)

        tts.beginReply() // next reply retries the bridge
        #expect(try await tts.synthesize("Trois.").samples == Data("bridge".utf8))
        #expect(primary.calls == ["Un.", "Trois."])
        #expect(primary.replies == 2 && fallback.replies == 2)
    }

    @Test func cancellationIsNotAFailure() async {
        let primary = ScriptedTTS("bridge", .cancel), fallback = ScriptedTTS("system")
        let tts = FallbackTTSProvider(primary: primary, fallback: fallback)
        await #expect(throws: CancellationError.self) { try await tts.synthesize("Un.") }
        #expect(fallback.calls.isEmpty)
        #expect(!tts.isUsingFallback)
    }

    @Test func rejectedSentenceDoesNotSwitchVoices() async throws {
        let primary = ScriptedTTS("bridge", .reject), fallback = ScriptedTTS("system")
        let tts = FallbackTTSProvider(primary: primary, fallback: fallback)
        tts.beginReply()
        await #expect(throws: TTSError.self) { try await tts.synthesize("…") }
        #expect(fallback.calls.isEmpty)
        #expect(!tts.isUsingFallback)
        primary.set(.succeed)
        #expect(try await tts.synthesize("Deux.").samples == Data("bridge".utf8))
    }

    @Test func fallbackErrorsPropagate() async {
        let tts = FallbackTTSProvider(primary: ScriptedTTS("bridge", .fail), fallback: ScriptedTTS("system", .fail))
        await #expect(throws: URLError.self) { try await tts.synthesize("Un.") }
    }
}
