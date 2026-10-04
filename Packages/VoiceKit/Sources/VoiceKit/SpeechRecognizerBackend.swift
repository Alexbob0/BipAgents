import AVFoundation
import Speech

/// One utterance of on-device streaming recognition. Fresh per utterance: a recogniser's text never
/// resets by itself, and the end-of-utterance / barge-in detectors need "transcript non-empty" to mean
/// "the user said something since the last reset".
protocol SpeechRecognitionSession: AnyObject, Sendable {
    /// Running transcript (finalised + volatile text) each time it changes.
    var transcripts: AsyncStream<String> { get }
    /// Mic audio (mono). Called on an audio thread.
    func append(_ buffer: AVAudioPCMBuffer)
    /// Ends the audio and returns the final transcript ("" if nothing was recognised). Bounded wait.
    func finish() async -> String
    func cancel()
}

/// Creates sessions synchronously so no audio is lost between utterances.
protocol SpeechRecognizerBackend: Sendable {
    func makeSession() throws -> any SpeechRecognitionSession
}

enum SpeechRecognition {
    /// How long `finish()` waits for the recogniser's final result before using the last partial.
    static let finalResultTimeout: Duration = .milliseconds(1500)

    /// `SpeechAnalyzer`/`SpeechTranscriber` (iOS 26+) when the locale's model is installed, otherwise
    /// `SFSpeechRecognizer` on device (and the model download is started for next time).
    static func makeBackend(locale: Locale) async throws -> any SpeechRecognizerBackend {
        if #available(iOS 26, macOS 26, *), let backend = await AnalyzerBackend.makeIfInstalled(locale: locale) {
            return backend
        }
        return try SFSpeechBackend(locale: locale)
    }

    static func joined(_ parts: [String]) -> String {
        parts.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }.joined(separator: " ")
    }
}

/// Resolves a session's final text once, from the recogniser, a timeout, or a cancellation.
final class FinalTranscript: @unchecked Sendable {
    private let lock = NSLock()
    private var latest = ""
    private var final: String?
    private var waiters: [CheckedContinuation<String, Never>] = []

    var current: String { lock.withLock { latest } }

    func update(_ text: String) { lock.withLock { latest = text } }

    func resolve(_ text: String? = nil) {
        let (value, resumed): (String, [CheckedContinuation<String, Never>]) = lock.withLock {
            if final == nil { final = text ?? latest }
            defer { waiters.removeAll() }
            return (final!, waiters)
        }
        resumed.forEach { $0.resume(returning: value) }
    }

    func wait(timeout: Duration) async -> String {
        let timer = Task { [weak self] in
            try? await Task.sleep(for: timeout)
            self?.resolve()
        }
        defer { timer.cancel() }
        return await withCheckedContinuation { continuation in
            let ready: String? = lock.withLock {
                if let final { return final }
                waiters.append(continuation)
                return nil
            }
            if let ready { continuation.resume(returning: ready) }
        }
    }
}

// MARK: - SFSpeechRecognizer (iOS 18–25, or while the SpeechTranscriber model is not installed)

final class SFSpeechBackend: SpeechRecognizerBackend, @unchecked Sendable {
    private let recognizer: SFSpeechRecognizer

    init(locale: Locale) throws {
        guard let recognizer = SFSpeechRecognizer(locale: locale), recognizer.isAvailable else {
            throw VoiceError.recognizerUnavailable
        }
        self.recognizer = recognizer
    }

    func makeSession() throws -> any SpeechRecognitionSession {
        SFSpeechSession(recognizer: recognizer)
    }
}

final class SFSpeechSession: SpeechRecognitionSession, @unchecked Sendable {
    let transcripts: AsyncStream<String>
    private let continuation: AsyncStream<String>.Continuation
    private let request = SFSpeechAudioBufferRecognitionRequest()
    private let result = FinalTranscript()
    private var task: SFSpeechRecognitionTask?

    init(recognizer: SFSpeechRecognizer) {
        (transcripts, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(1))
        request.shouldReportPartialResults = true
        request.requiresOnDeviceRecognition = recognizer.supportsOnDeviceRecognition
        request.taskHint = .dictation
        request.addsPunctuation = true
        task = recognizer.recognitionTask(with: request, resultHandler: Self.handler(result, continuation))
    }

    func append(_ buffer: AVAudioPCMBuffer) { request.append(buffer) }

    func finish() async -> String {
        request.endAudio()
        let text = await result.wait(timeout: SpeechRecognition.finalResultTimeout)
        continuation.finish()
        return text
    }

    func cancel() {
        task?.cancel()
        result.resolve()
        continuation.finish()
    }

    private static func handler(_ result: FinalTranscript, _ continuation: AsyncStream<String>.Continuation)
        -> @Sendable (SFSpeechRecognitionResult?, (any Error)?) -> Void {
        { recognition, error in
            if let recognition {
                let text = recognition.bestTranscription.formattedString
                result.update(text)
                continuation.yield(text)
                if recognition.isFinal { result.resolve(text) }
            }
            if error != nil { result.resolve() } // e.g. "no speech detected": keep the last partial
        }
    }
}

// MARK: - SpeechAnalyzer / SpeechTranscriber (iOS 26+)

@available(iOS 26, macOS 26, *)
final class AnalyzerBackend: SpeechRecognizerBackend {
    let locale: Locale
    let format: AVAudioFormat?

    private init(locale: Locale, format: AVAudioFormat?) {
        self.locale = locale
        self.format = format
    }

    static func makeTranscriber(_ locale: Locale) -> SpeechTranscriber {
        SpeechTranscriber(locale: locale, transcriptionOptions: [], reportingOptions: [.volatileResults, .fastResults], attributeOptions: [])
    }

    /// `nil` when the device or locale is unsupported, or the model is not installed yet (download started).
    static func makeIfInstalled(locale: Locale) async -> AnalyzerBackend? {
        guard SpeechTranscriber.isAvailable,
              let supported = await SpeechTranscriber.supportedLocale(equivalentTo: locale) else { return nil }
        let transcriber = makeTranscriber(supported)
        switch await AssetInventory.status(forModules: [transcriber]) {
        case .installed:
            let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber])
            return AnalyzerBackend(locale: supported, format: format)
        case .supported, .downloading:
            Task.detached(priority: .utility) {
                try? await AssetInventory.assetInstallationRequest(supporting: [makeTranscriber(supported)])?.downloadAndInstall()
            }
            return nil
        case .unsupported:
            return nil
        @unknown default:
            return nil
        }
    }

    func makeSession() throws -> any SpeechRecognitionSession {
        AnalyzerSession(locale: locale, format: format)
    }
}

/// Mic buffers are converted to the analyzer's format on the audio thread and buffered in an AsyncStream,
/// so audio captured while the analyzer is still starting up is not lost.
@available(iOS 26, macOS 26, *)
final class AnalyzerSession: SpeechRecognitionSession, @unchecked Sendable {
    let transcripts: AsyncStream<String>
    private let continuation: AsyncStream<String>.Continuation
    private let input: AsyncStream<AnalyzerInput>.Continuation
    private let analyzer: SpeechAnalyzer
    private let result = FinalTranscript()
    private let lock = NSLock()
    private let converter: FormatConverter?
    private var resultsTask: Task<Void, Never>?

    init(locale: Locale, format: AVAudioFormat?) {
        (transcripts, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(1))
        let inputs: AsyncStream<AnalyzerInput>
        (inputs, input) = AsyncStream.makeStream()
        converter = format.map(FormatConverter.init(outputFormat:))
        let transcriber = AnalyzerBackend.makeTranscriber(locale)
        // `.lingering` keeps the model warm between utterances (a session is created per utterance).
        analyzer = SpeechAnalyzer(modules: [transcriber], options: .init(priority: .userInitiated, modelRetention: .lingering))
        resultsTask = Task { [result, continuation, results = transcriber.results] in
            var finalized: [String] = []
            do {
                for try await item in results {
                    let text = String(item.text.characters)
                    if item.isFinal { finalized.append(text) }
                    let transcript = SpeechRecognition.joined(finalized + (item.isFinal ? [] : [text]))
                    result.update(transcript)
                    continuation.yield(transcript)
                }
            } catch {}
            result.resolve(SpeechRecognition.joined(finalized).isEmpty ? nil : SpeechRecognition.joined(finalized))
        }
        Task { [analyzer] in try? await analyzer.start(inputSequence: inputs) }
    }

    func append(_ buffer: AVAudioPCMBuffer) {
        let converted = lock.withLock { converter.map { $0.convert(buffer, streaming: true) } ?? buffer }
        if let converted { input.yield(AnalyzerInput(buffer: converted)) }
    }

    func finish() async -> String {
        input.finish()
        let analyzer = analyzer
        Task { try? await analyzer.finalizeAndFinishThroughEndOfInput() }
        let text = await result.wait(timeout: SpeechRecognition.finalResultTimeout)
        continuation.finish()
        return text
    }

    func cancel() {
        input.finish()
        resultsTask?.cancel()
        result.resolve()
        continuation.finish()
        let analyzer = analyzer
        Task { await analyzer.cancelAndFinishNow() }
    }
}

// MARK: - Permissions

enum VoicePermissions {
    static func request() async -> Bool {
        let microphone = await AVAudioApplication.requestRecordPermission()
        let speech = await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            SFSpeechRecognizer.requestAuthorization(Self.authorizationHandler(continuation))
        }
        return microphone && speech
    }

    static var granted: Bool {
        AVAudioApplication.shared.recordPermission == .granted && SFSpeechRecognizer.authorizationStatus() == .authorized
    }

    private static func authorizationHandler(_ continuation: CheckedContinuation<Bool, Never>)
        -> @Sendable (SFSpeechRecognizerAuthorizationStatus) -> Void {
        { continuation.resume(returning: $0 == .authorized) }
    }
}
