import AVFoundation

/// On-device `AVSpeechSynthesizer` (best-quality installed voice for `language`, novelty voices excluded),
/// rendered to PCM with `write(_:toBufferCallback:)` so it goes through the same playback queue, echo
/// cancellation and level metering as the bridge audio. The fallback that keeps the app from going mute.
public struct SystemTTSProvider: TTSProvider {
    public var language: String

    public init(language: String = "fr-FR") {
        self.language = language
    }

    public func synthesize(_ sentence: String) async throws -> PCMChunk {
        let text = sentence.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return PCMChunk(samples: Data(), sampleRate: 22_050) }
        return try await SpeechRender(text: text, language: language).run()
    }

    /// Best installed voice: premium > enhanced > default; exact language match preferred over the base language.
    static func bestVoice(for language: String) -> AVSpeechSynthesisVoice? {
        let base = language.split(separator: "-").first.map(String.init) ?? language
        let candidates = AVSpeechSynthesisVoice.speechVoices().filter {
            ($0.language == language || $0.language.hasPrefix(base + "-")) && !$0.voiceTraits.contains(.isNoveltyVoice)
        }
        return candidates.max { lhs, rhs in
            (lhs.quality.rawValue, lhs.language == language ? 1 : 0) < (rhs.quality.rawValue, rhs.language == language ? 1 : 0)
        } ?? AVSpeechSynthesisVoice(language: language)
    }
}

/// One `write` call. The synthesizer must stay alive until the final (empty) buffer arrives; buffers come
/// on a private queue, hence the lock.
private final class SpeechRender: @unchecked Sendable {
    private let synthesizer = AVSpeechSynthesizer()
    private let utterance: AVSpeechUtterance
    private let lock = NSLock()
    private var samples = Data()
    private var sampleRate: Double = 0
    private var continuation: CheckedContinuation<PCMChunk, any Error>?
    private var done = false

    init(text: String, language: String) {
        utterance = AVSpeechUtterance(string: text)
        utterance.voice = SystemTTSProvider.bestVoice(for: language)
    }

    func run() async throws -> PCMChunk {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                lock.withLock { self.continuation = continuation }
                if Task.isCancelled { return finish(.failure(CancellationError())) }
                synthesizer.write(utterance, toBufferCallback: Self.callback(for: self))
            }
        } onCancel: {
            finish(.failure(CancellationError()))
        }
    }

    /// Built outside any actor context: the callback runs on the synthesizer's own queue.
    private static func callback(for render: SpeechRender) -> AVSpeechSynthesizer.BufferCallback {
        { buffer in render.receive(buffer) }
    }

    private func receive(_ buffer: AVAudioBuffer) {
        guard let pcm = buffer as? AVAudioPCMBuffer else {
            return finish(.failure(TTSError.synthesisFailed("format audio inattendu")))
        }
        guard pcm.frameLength > 0 else { return finish(nil) } // empty buffer = end of utterance
        let data = pcm.int16Data()
        lock.withLock {
            sampleRate = pcm.format.sampleRate
            samples.append(data)
        }
    }

    /// `nil` = success with what was rendered.
    private func finish(_ failure: Result<Never, any Error>?) {
        let (continuation, chunk): (CheckedContinuation<PCMChunk, any Error>?, PCMChunk) = lock.withLock {
            guard !done, let continuation = self.continuation else { return (nil, PCMChunk(samples: Data(), sampleRate: 0)) }
            done = true
            self.continuation = nil
            return (continuation, PCMChunk(samples: samples, sampleRate: sampleRate > 0 ? sampleRate : 22_050))
        }
        guard let continuation else { return }
        if case .failure(let error)? = failure {
            synthesizer.stopSpeaking(at: .immediate)
            continuation.resume(throwing: error)
        } else {
            continuation.resume(returning: chunk)
        }
    }
}
