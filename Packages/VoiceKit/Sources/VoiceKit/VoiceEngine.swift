import AVFoundation
import Observation

/// Voice I/O for the app: push-to-talk dictation, hands-free call (listen → reply → speak → listen, with
/// barge-in) and "Écouter" playback of a message. Everything here runs on the main actor; audio threads
/// talk to it only through `CaptureSink.readings` and recogniser streams.
@MainActor @Observable
public final class VoiceEngine {
    public enum State: Equatable, Sendable { case idle, listening, thinking, speaking }

    public var configuration: VoiceConfiguration
    public var tts: any TTSProvider
    public private(set) var state: State = .idle
    /// 0…1 smoothed mic level, for UI waveforms.
    public private(set) var inputLevel: Double = 0
    /// 0…1 smoothed playback level (drives the mascot's mouth).
    public private(set) var outputLevel: Double = 0
    /// Live STT text while listening.
    public private(set) var partialTranscript: String = ""
    public private(set) var metrics = VoiceMetrics()
    public private(set) var lastError: String?

    private struct Call {
        var onUtterance: @MainActor (String) -> AsyncThrowingStream<String, any Error>
        var onInterrupt: @MainActor () -> Void
    }

    private enum Mode {
        case none, dictation, call(Call), speech
        var isCall: Bool { if case .call = self { return true }; return false }
    }

    @ObservationIgnored private var mode: Mode = .none
    @ObservationIgnored private let audioSession = AudioSessionController()
    @ObservationIgnored private let sink = CaptureSink()
    @ObservationIgnored private let output = PlayerOutput()
    @ObservationIgnored private let playback: PlaybackQueue<PlayerOutput>
    @ObservationIgnored private let formatter = FormatConverter(outputFormat: AudioGraph.playerFormat)
    @ObservationIgnored private var graph: AudioGraph?
    @ObservationIgnored private var recognizer: (any SpeechRecognizerBackend)?
    @ObservationIgnored private var endOfUtterance = EndOfUtteranceDetector()
    @ObservationIgnored private var bargeIn = BargeInDetector()
    @ObservationIgnored private var replyTask: Task<Void, Never>?
    /// Bumped whenever the current reply is abandoned, so a cancelled reply task that resumes late can
    /// never touch the playback queue or the state of the next one.
    @ObservationIgnored private var replyID = 0

    public init(configuration: VoiceConfiguration = .init(), tts: any TTSProvider = SystemTTSProvider()) {
        self.configuration = configuration
        self.tts = tts
        playback = PlaybackQueue(output: output)
        audioSession.onInterruption = { [weak self] began, shouldResume in self?.handleInterruption(began: began, shouldResume: shouldResume) }
        audioSession.onRouteChange = { [weak self] lostOutput in self?.handleRouteChange(lostOutput: lostOutput) }
        audioSession.onMediaServicesReset = { [weak self] in self?.handleMediaServicesReset() }
        Task { [weak self, readings = sink.readings] in
            for await reading in readings { self?.handle(reading) }
        }
    }

    /// Mic + speech recognition permissions.
    public static func requestPermissions() async -> Bool {
        await VoicePermissions.request()
    }

    // MARK: - Dictation

    /// Starts push-to-talk dictation; live text in `partialTranscript`.
    public func startDictation() async throws {
        switch mode {
        case .dictation: return
        case .call: throw VoiceError.busy
        case .speech: stopSpeaking()
        case .none: break
        }
        mode = .dictation
        do {
            try await prepareCapture()
            guard case .dictation = mode else { throw CancellationError() }
        } catch {
            if case .dictation = mode { teardown() }
            fail(error)
            throw error
        }
        startRecognition()
        state = .listening
    }

    /// Stops dictation and returns the final transcript.
    public func finishDictation() async -> String {
        guard case .dictation = mode else { return "" }
        let fallback = partialTranscript
        let session = sink.swapSession(nil)
        let start = ContinuousClock.now
        let text = await session?.finish() ?? ""
        metrics.lastSTTDuration = .now - start
        if case .dictation = mode { teardown() }
        return (text.isEmpty ? fallback : text).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    public func cancelDictation() {
        guard case .dictation = mode else { return }
        sink.swapSession(nil)?.cancel()
        teardown()
    }

    // MARK: - Call

    /// Hands-free loop: listen → `onUtterance(text)` → speak the streamed reply sentence by sentence →
    /// listen again. Barge-in (voice during playback, or `interrupt()`) stops playback at once, cancels
    /// the reply stream, calls `onInterrupt()` and listens again.
    public func startCall(onUtterance: @escaping @MainActor (String) -> AsyncThrowingStream<String, any Error>,
                          onInterrupt: @escaping @MainActor () -> Void) async throws {
        switch mode {
        case .dictation: cancelDictation()
        case .speech: stopSpeaking()
        case .call: endCall()
        case .none: break
        }
        mode = .call(Call(onUtterance: onUtterance, onInterrupt: onInterrupt))
        do {
            try await prepareCapture()
            guard mode.isCall else { throw CancellationError() }
        } catch {
            if mode.isCall { teardown() }
            fail(error)
            throw error
        }
        endOfUtterance = EndOfUtteranceDetector(configuration: configuration.endOfUtterance)
        bargeIn = BargeInDetector(configuration: configuration.bargeIn)
        startRecognition()
        state = .listening
    }

    public func endCall() {
        guard mode.isCall else { return }
        sink.swapSession(nil)?.cancel()
        teardown()
    }

    /// Manual barge-in: stops the reply (thinking or speaking) and listens again.
    public func interrupt() {
        switch mode {
        case .call where state == .thinking || state == .speaking:
            interruptReply()
            startRecognition() // drop any echo picked up during playback
        case .speech:
            stopSpeaking()
        default:
            break
        }
    }

    // MARK: - Speech

    /// Speaks `text` (the "Écouter" button). Cancels any current speech; returns when done or stopped.
    /// During a call it replaces the current reply; ignored while dictating.
    public func speak(_ text: String) async {
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        switch mode {
        case .dictation:
            return
        case .call:
            if state == .thinking || state == .speaking { interruptReply() }
        case .speech:
            stopSpeaking()
            fallthrough
        case .none:
            mode = .speech
            do {
                try audioSession.activate(.playback)
                try buildGraph(capture: false)
            } catch {
                fail(error)
                if case .speech = mode { teardown() }
                return
            }
        }
        let (deltas, continuation) = AsyncThrowingStream<String, any Error>.makeStream()
        continuation.yield(text)
        continuation.finish()
        let task = startReply(deltas)
        await withTaskCancellationHandler { await task.value } onCancel: { task.cancel() }
    }

    public func stopSpeaking() {
        switch mode {
        case .speech:
            teardown()
        case .call where state == .speaking || state == .thinking:
            interrupt()
        default:
            break
        }
    }

    // MARK: - Capture

    private func prepareCapture() async throws {
        guard VoicePermissions.granted else { throw VoiceError.permissionDenied }
        recognizer = try await SpeechRecognition.makeBackend(locale: configuration.locale)
        try audioSession.activate(.voice)
        try buildGraph(capture: true)
    }

    private func buildGraph(capture: Bool) throws {
        graph?.stop()
        graph = nil
        output.node = nil
        let graph = try AudioGraph(capture: capture, sink: sink) { [weak self] in self?.configurationChanged() }
        self.graph = graph
        output.node = graph.player
    }

    /// AVAudioEngine stops itself when the hardware format changes (route change, voice processing
    /// toggled by another engine…). A notification for a still-running engine needs no rebuild — and
    /// rebuilding anyway could loop, since a new voice-processing engine can trigger another change.
    private func configurationChanged() {
        guard let graph, !graph.engine.isRunning else { return }
        rebuildGraph()
    }

    /// The engine has stopped and formats may have changed. Rebuild, and replay whatever was not yet heard.
    private func rebuildGraph() {
        guard let capture = graph?.capturesInput else { return }
        do {
            try buildGraph(capture: capture)
            playback.resetOutput()
        } catch {
            fail(error)
            teardown()
        }
    }

    /// A fresh recogniser session for the next utterance; the previous one is cancelled.
    private func startRecognition() {
        partialTranscript = ""
        guard let recognizer, let session = try? recognizer.makeSession() else {
            sink.swapSession(nil)?.cancel()
            return
        }
        sink.swapSession(session)?.cancel()
        Task { [weak self] in
            for await text in session.transcripts {
                guard let self, self.sink.isCurrent(session) else { continue }
                self.partialTranscript = text
            }
        }
    }

    private func handle(_ reading: CaptureSink.Reading) {
        let level = AudioLevel.display(reading.rms)
        switch reading.source {
        case .output:
            outputLevel = AudioLevel.smoothed(outputLevel, toward: level)
        case .input:
            inputLevel = AudioLevel.smoothed(inputLevel, toward: level)
            guard mode.isCall else { return }
            let sample = VoiceSample(timestamp: reading.timestamp, rmsLevel: reading.rms, hasPartialTranscript: !partialTranscript.isEmpty)
            switch state {
            case .listening:
                if endOfUtterance.process(sample) == .utteranceEnded { utteranceEnded() }
            case .speaking:
                if configuration.bargeInEnabled, bargeIn.process(sample) == .bargeIn { bargedIn(detectedAt: reading.timestamp) }
            case .idle, .thinking:
                break
            }
        }
    }

    // MARK: - Reply

    private func utteranceEnded() {
        guard case .call(let call) = mode, let finished = sink.swapSession(nil) else { return }
        endOfUtterance.reset()
        startRecognition() // keep listening for barge-in while the reply is prepared
        state = .thinking
        let id = replyID
        let start = ContinuousClock.now
        replyTask = Task { [weak self] in
            let text = await finished.finish().trimmingCharacters(in: .whitespacesAndNewlines)
            guard let self, id == self.replyID, self.mode.isCall else { return }
            self.metrics.lastSTTDuration = .now - start
            guard !text.isEmpty else {
                self.state = .listening
                return
            }
            await self.playReply(call.onUtterance(text), id: id)
        }
    }

    private func startReply(_ deltas: AsyncThrowingStream<String, any Error>) -> Task<Void, Never> {
        state = .thinking
        let id = replyID
        let task = Task { [weak self] in _ = await self?.playReply(deltas, id: id) }
        replyTask = task
        return task
    }

    /// Streams `deltas` through the speech pipeline into the playback queue, then waits for the audio to
    /// finish. Every step re-checks `id` because an interrupted reply may resume after the next started.
    private func playReply(_ deltas: AsyncThrowingStream<String, any Error>, id: Int) async {
        var textStart: ContinuousClock.Instant?
        var started = false
        do {
            for try await event in SpeechPipeline(tts: tts).events(for: deltas) {
                guard id == replyID else { return }
                switch event {
                case .textStarted(let instant):
                    textStart = instant
                case .sentence(_, _, let audio):
                    guard let source = audio.pcmBuffer(), let buffer = formatter.convert(source, streaming: false) else { continue }
                    if !started {
                        started = true
                        metrics.lastFirstAudioLatency = textStart.map { .now - $0 }
                        playbackStarted()
                    }
                    playback.enqueue(buffer)
                }
            }
        } catch is CancellationError {
            return
        } catch {
            guard id == replyID else { return }
            fail(error) // keep playing whatever was already queued
        }
        guard id == replyID, !Task.isCancelled else { return }
        await playback.finishAndWait()
        guard id == replyID, !Task.isCancelled else { return }
        replyFinished()
    }

    private func playbackStarted() {
        state = .speaking
        guard mode.isCall else { return }
        startRecognition() // only speech heard during playback may count for barge-in
        bargeIn.playbackStarted()
    }

    private func replyFinished() {
        replyTask = nil
        switch mode {
        case .call:
            bargeIn.playbackStopped()
            endOfUtterance.reset()
            startRecognition() // drop echo residue recognised during playback
            state = .listening
        case .speech:
            teardown()
        case .none, .dictation:
            break
        }
    }

    /// Voice during playback. Order matters for latency: silence first, then cancel the reply.
    private func bargedIn(detectedAt timestamp: TimeInterval) {
        playback.stop()
        metrics.lastBargeInLatency = .seconds(max(0, HostClock.now() - timestamp))
        interruptReply()
        // The current recogniser session already holds the start of the user's new utterance: keep it.
    }

    private func interruptReply() {
        playback.stop()
        replyID += 1
        replyTask?.cancel()
        replyTask = nil
        bargeIn.playbackStopped()
        endOfUtterance.reset()
        guard case .call(let call) = mode else { return }
        state = .listening
        call.onInterrupt()
    }

    // MARK: - Session events

    private func handleInterruption(began: Bool, shouldResume: Bool) {
        if began {
            switch mode {
            case .call where state == .thinking || state == .speaking: interruptReply()
            case .speech: teardown()
            default: break
            }
        } else if graph != nil {
            do {
                try audioSession.activate(audioSession.activePurpose ?? .voice)
                rebuildGraph()
            } catch {
                fail(error)
            }
        }
    }

    private func handleRouteChange(lostOutput: Bool) {
        // Headphones unplugged: stop reading aloud (platform convention); a call keeps going on the speaker.
        if lostOutput, case .speech = mode { teardown() }
    }

    private func handleMediaServicesReset() {
        fail(VoiceError.microphoneUnavailable)
        sink.swapSession(nil)?.cancel()
        teardown()
    }

    // MARK: - Teardown

    private func teardown() {
        mode = .none
        replyID += 1
        replyTask?.cancel()
        replyTask = nil
        playback.stop()
        sink.swapSession(nil)?.cancel()
        graph?.stop()
        graph = nil
        output.node = nil
        recognizer = nil
        audioSession.deactivate()
        bargeIn.playbackStopped()
        endOfUtterance.reset()
        state = .idle
        inputLevel = 0
        outputLevel = 0
        partialTranscript = ""
    }

    private func fail(_ error: any Error) {
        guard !(error is CancellationError) else { return }
        lastError = String(describing: error)
    }
}
