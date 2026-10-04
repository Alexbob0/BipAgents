import Foundation
import Synchronization

/// One block of synthesized speech: signed 16-bit little-endian mono samples.
public struct PCMChunk: Sendable, Equatable {
    /// Int16 LE mono samples.
    public var samples: Data
    public var sampleRate: Double

    public init(samples: Data, sampleRate: Double) {
        self.samples = samples
        self.sampleRate = sampleRate
    }

    public var frameCount: Int { samples.count / 2 }
    public var duration: Duration { sampleRate > 0 ? .seconds(Double(frameCount) / sampleRate) : .zero }
}

/// Turns one sentence into audio. Called concurrently (the pipeline prefetches the next sentence while
/// the current one plays), so implementations must be reentrant.
public protocol TTSProvider: Sendable {
    /// Audio for one sentence.
    func synthesize(_ sentence: String) async throws -> PCMChunk
    /// Called once before the first sentence of each reply (lets `FallbackTTSProvider` retry its primary).
    /// Default: no-op.
    func beginReply()
}

extension TTSProvider {
    public func beginReply() {}
}

public enum TTSError: Error, Sendable, Equatable, CustomStringConvertible {
    case invalidResponse
    case httpStatus(Int)
    case synthesisFailed(String)

    public var description: String {
        switch self {
        case .invalidResponse: "Réponse TTS invalide"
        case .httpStatus(let code): "Bridge TTS : HTTP \(code)"
        case .synthesisFailed(let reason): "Synthèse vocale impossible : \(reason)"
        }
    }
}

/// `POST {bridgeURL}/v1/tts/sentence`, JSON `{"text","voice","format":"pcm16"}`, `Authorization: Bearer bridgeKey`.
/// Response body = raw PCM int16 mono; sample rate from the `X-Sample-Rate` header (default 24000).
public struct BridgeTTSProvider: TTSProvider {
    public var bridgeURL: URL
    public var bridgeKey: String
    public var voice: String
    public var session: URLSession
    /// Per-request timeout. Short on purpose: when the bridge is unreachable the fallback must kick in
    /// before the silence becomes awkward.
    public var timeout: TimeInterval = 6

    public init(bridgeURL: URL, bridgeKey: String, voice: String, session: URLSession = .shared) {
        self.bridgeURL = bridgeURL
        self.bridgeKey = bridgeKey
        self.voice = voice
        self.session = session
    }

    struct Body: Codable, Equatable {
        var text: String
        var voice: String
        var format: String
    }

    static let defaultSampleRate: Double = 24_000

    func request(for sentence: String) throws -> URLRequest {
        var request = URLRequest(url: bridgeURL.appending(path: "v1/tts/sentence"), timeoutInterval: timeout)
        request.httpMethod = "POST"
        request.setValue("Bearer \(bridgeKey)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("application/octet-stream", forHTTPHeaderField: "Accept")
        let encoder = JSONEncoder()
        encoder.outputFormatting = .sortedKeys
        request.httpBody = try encoder.encode(Body(text: sentence, voice: voice, format: "pcm16"))
        return request
    }

    public func synthesize(_ sentence: String) async throws -> PCMChunk {
        let (data, response) = try await session.data(for: request(for: sentence))
        guard let http = response as? HTTPURLResponse else { throw TTSError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else { throw TTSError.httpStatus(http.statusCode) }
        let rate = http.value(forHTTPHeaderField: "X-Sample-Rate").flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        let samples = data.count.isMultiple(of: 2) ? data : data.dropLast() // never split a sample
        return PCMChunk(samples: Data(samples), sampleRate: rate.flatMap { $0 > 0 ? $0 : nil } ?? Self.defaultSampleRate)
    }
}

/// Tries `primary`; on any error (bridge unreachable, HTTP 5xx…) synthesizes that sentence with `fallback`
/// and keeps using `fallback` for the rest of the reply, so the voice does not flip back and forth.
/// `beginReply()` re-arms the primary for the next reply. Cancellation is never treated as a failure.
public struct FallbackTTSProvider: TTSProvider {
    public let primary: any TTSProvider
    public let fallback: any TTSProvider
    private let state = FallbackState()

    public init(primary: any TTSProvider, fallback: any TTSProvider) {
        self.primary = primary
        self.fallback = fallback
    }

    /// Whether the current reply has switched to the fallback.
    public var isUsingFallback: Bool { state.failedOver.withLock { $0 } }

    public func beginReply() {
        state.failedOver.withLock { $0 = false }
        primary.beginReply()
        fallback.beginReply()
    }

    public func synthesize(_ sentence: String) async throws -> PCMChunk {
        if !isUsingFallback {
            do {
                return try await primary.synthesize(sentence)
            } catch {
                if Task.isCancelled || Self.isCancellation(error) { throw error }
                state.failedOver.withLock { $0 = true }
            }
        }
        return try await fallback.synthesize(sentence)
    }

    static func isCancellation(_ error: any Error) -> Bool {
        error is CancellationError || (error as? URLError)?.code == .cancelled
    }
}

/// Shared across copies of the provider (it is a value type handed to concurrent synth tasks).
private final class FallbackState: Sendable {
    let failedOver = Mutex(false)
}
