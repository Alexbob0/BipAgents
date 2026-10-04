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
    /// Audio for `text` as it is produced, in order. Default: `synthesize` as a single chunk.
    func stream(_ text: String) -> AsyncThrowingStream<PCMChunk, any Error>
    /// Whether `stream` really delivers audio progressively, so a long segment starts playing as fast as a
    /// short one. Default: false.
    var streamsAudio: Bool { get }
}

extension TTSProvider {
    public func beginReply() {}

    public func stream(_ text: String) -> AsyncThrowingStream<PCMChunk, any Error> {
        AsyncThrowingStream.producing { yield in yield(try await synthesize(text)) }
    }

    public var streamsAudio: Bool { false }
}

extension AsyncThrowingStream where Failure == any Error {
    /// Runs `body` in a task that `yield`s elements; cancelling the consumer cancels the task.
    public static func producing(_ body: @escaping @Sendable (_ yield: (Element) -> Void) async throws -> Void) -> Self
    where Element: Sendable {
        let (stream, continuation) = makeStream()
        let task = Task {
            do {
                try await body { continuation.yield($0) }
                continuation.finish()
            } catch {
                continuation.finish(throwing: error)
            }
        }
        continuation.onTermination = { _ in task.cancel() }
        return stream
    }
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
///
/// `stream(_:)` uses `POST /v1/tts/stream` (same body, chunked PCM relayed from Kyutai as it is produced:
/// first audio after ~0.75 s whatever the text length), and falls back to `/v1/tts/sentence` on an older
/// bridge (404).
public struct BridgeTTSProvider: TTSProvider {
    public var bridgeURL: URL
    public var bridgeKey: String
    public var voice: String
    public var session: URLSession
    /// Per-request timeout. Generous on purpose: the bridge queues sentences for Kyutai (one at a time), so
    /// a prefetched sentence legitimately waits behind the one being synthesized. An unreachable bridge
    /// fails fast anyway (connection refused, offline…), long before this.
    public var timeout: TimeInterval = 30
    /// Use `/v1/tts/stream` for `stream(_:)`.
    public var streaming = true
    /// Bytes per streamed chunk: the first one small so playback starts at once, then larger.
    var firstChunkBytes = 4_800   // 100 ms at 24 kHz
    var chunkBytes = 19_200       // 400 ms

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

    func request(for sentence: String, path: String = "v1/tts/sentence") throws -> URLRequest {
        var request = URLRequest(url: bridgeURL.appending(path: path), timeoutInterval: timeout)
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
        #if DEBUG
        let started = Date.now
        #endif
        let (data, response) = try await session.data(for: request(for: sentence))
        guard let http = response as? HTTPURLResponse else { throw TTSError.invalidResponse }
        guard (200..<300).contains(http.statusCode) else { throw TTSError.httpStatus(http.statusCode) }
        let rate = http.value(forHTTPHeaderField: "X-Sample-Rate").flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        let samples = data.count.isMultiple(of: 2) ? data : data.dropLast() // never split a sample
        let chunk = PCMChunk(samples: Data(samples), sampleRate: rate.flatMap { $0 > 0 ? $0 : nil } ?? Self.defaultSampleRate)
        #if DEBUG
        let audio = Double(samples.count) / 2 / chunk.sampleRate
        print(String(format: "[voice] bridge TTS %.2fs for %.2fs of audio (%d chars, cache %@): “%@”",
                     Date.now.timeIntervalSince(started), audio, sentence.count,
                     http.value(forHTTPHeaderField: "X-Cache") ?? "?", String(sentence.prefix(40))))
        #endif
        return chunk
    }

    public var streamsAudio: Bool { streaming }

    public func stream(_ text: String) -> AsyncThrowingStream<PCMChunk, any Error> {
        guard streaming else { return AsyncThrowingStream.producing { yield in yield(try await synthesize(text)) } }
        return AsyncThrowingStream.producing { yield in try await streamChunks(text, yield) }
    }

    private func streamChunks(_ text: String, _ yield: (PCMChunk) -> Void) async throws {
        #if DEBUG
        let started = Date.now
        var firstAudio: TimeInterval?
        #endif
        let (bytes, response) = try await session.bytes(for: request(for: text, path: "v1/tts/stream"))
        guard let http = response as? HTTPURLResponse else { throw TTSError.invalidResponse }
        if http.statusCode == 404 { return yield(try await synthesize(text)) } // bridge without streaming
        guard (200..<300).contains(http.statusCode) else { throw TTSError.httpStatus(http.statusCode) }
        let rate = http.value(forHTTPHeaderField: "X-Sample-Rate").flatMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        let sampleRate = rate.flatMap { $0 > 0 ? $0 : nil } ?? Self.defaultSampleRate
        var buffer = Data(capacity: chunkBytes)
        var target = firstChunkBytes, total = 0
        func emit() {
            let whole = buffer.count - buffer.count % 2 // never split a sample
            guard whole > 0 else { return }
            yield(PCMChunk(samples: buffer.prefix(whole), sampleRate: sampleRate))
            buffer.removeFirst(whole)
            total += whole
            #if DEBUG
            if firstAudio == nil { firstAudio = Date.now.timeIntervalSince(started) }
            #endif
        }
        for try await byte in bytes {
            buffer.append(byte)
            if buffer.count >= target {
                emit()
                target = chunkBytes
            }
        }
        emit()
        #if DEBUG
        print(String(format: "[voice] bridge TTS stream: first audio %.2fs, done %.2fs for %.2fs of audio (%d chars): “%@”",
                     firstAudio ?? -1, Date.now.timeIntervalSince(started), Double(total) / 2 / sampleRate,
                     text.count, String(text.prefix(40))))
        #endif
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
                #if DEBUG
                print("[voice] primary TTS failed: \(error) — \(Self.isOutage(error) ? "switching to fallback" : "skipping sentence")")
                #endif
                // Switch voices only when the primary is really down: mixing voices mid-reply sounds broken.
                guard Self.isOutage(error) else { throw error }
                state.failedOver.withLock { $0 = true }
            }
        }
        return try await fallback.synthesize(sentence)
    }

    public var streamsAudio: Bool { !isUsingFallback && primary.streamsAudio }

    /// Like `synthesize`, but the primary can only be abandoned before its first chunk: once audio has
    /// played in one voice, a failure ends that segment instead of finishing it in another voice.
    public func stream(_ text: String) -> AsyncThrowingStream<PCMChunk, any Error> {
        AsyncThrowingStream.producing { yield in
            if !isUsingFallback {
                var delivered = false
                do {
                    for try await chunk in primary.stream(text) {
                        delivered = true
                        yield(chunk)
                    }
                    return
                } catch {
                    if Task.isCancelled || Self.isCancellation(error) || delivered { throw error }
                    #if DEBUG
                    print("[voice] primary TTS failed: \(error) — \(Self.isOutage(error) ? "switching to fallback" : "skipping segment")")
                    #endif
                    guard Self.isOutage(error) else { throw error }
                    state.failedOver.withLock { $0 = true }
                }
            }
            for try await chunk in fallback.stream(text) { yield(chunk) }
        }
    }

    /// Unreachable, timed out or failing server — as opposed to a request the server rejected (4xx).
    static func isOutage(_ error: any Error) -> Bool {
        switch error {
        case let error as TTSError:
            if case .httpStatus(let status) = error { return status >= 500 }
            return true
        case is URLError:
            return true
        default:
            return true
        }
    }

    static func isCancellation(_ error: any Error) -> Bool {
        error is CancellationError || (error as? URLError)?.code == .cancelled
    }
}

/// Shared across copies of the provider (it is a value type handed to concurrent synth tasks).
private final class FallbackState: Sendable {
    let failedOver = Mutex(false)
}
