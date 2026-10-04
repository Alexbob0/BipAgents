import AVFoundation

/// Thread-safe bridge between the audio threads and the main actor.
///
/// Tap blocks run on AVAudioEngine's own threads: they only compute levels, push them into `readings`
/// (consumed on the main actor) and hand mic audio to the current recognition session. They never touch
/// main-actor state.
final class CaptureSink: @unchecked Sendable {
    enum Source: Sendable { case input, output }

    struct Reading: Sendable {
        var source: Source
        /// Host-time seconds of the start of the frame.
        var timestamp: TimeInterval
        var rms: Float
    }

    let readings: AsyncStream<Reading>
    private let continuation: AsyncStream<Reading>.Continuation
    private let lock = NSLock()
    private var session: (any SpeechRecognitionSession)?
    /// Voice-note recording: the file is created on the first input buffer (its rate is only known then).
    private var recording: (url: URL, file: AVAudioFile?, frames: AVAudioFramePosition, rate: Double)?

    /// Level frames of ~20 ms, whatever buffer size the tap delivers: detector timing stays fine-grained.
    static let frameDuration: TimeInterval = 0.02

    init() {
        (readings, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(512))
    }

    deinit {
        continuation.finish()
    }

    /// Routes mic audio to `session` from now on; returns the previous one (to finish or cancel).
    @discardableResult
    func swapSession(_ session: (any SpeechRecognitionSession)?) -> (any SpeechRecognitionSession)? {
        lock.withLock {
            let previous = self.session
            self.session = session
            return previous
        }
    }

    func isCurrent(_ session: any SpeechRecognitionSession) -> Bool {
        lock.withLock { self.session === session }
    }

    func receive(_ buffer: AVAudioPCMBuffer, at time: AVAudioTime, from source: Source) {
        let start = HostClock.seconds(of: time)
        let rate = buffer.format.sampleRate
        let total = Int(buffer.frameLength)
        let step = max(1, Int(rate * Self.frameDuration))
        var offset = 0
        while offset < total {
            let end = min(total, offset + step)
            continuation.yield(Reading(source: source, timestamp: start + Double(offset) / rate,
                                       rms: AudioLevel.rms(buffer, frames: offset..<end)))
            offset = end
        }
        guard source == .input else { return }
        let (session, recording) = lock.withLock { (self.session, self.recording) }
        guard session != nil || recording != nil, let mono = buffer.monoCopy() else { return }
        session?.append(mono)
        if recording != nil { write(mono) }
    }

    // MARK: Voice-note recording (AAC in .m4a)

    func startRecording(to url: URL) {
        lock.withLock { recording = (url, nil, 0, 0) }
    }

    /// Closes the file; returns its URL and duration (nil if nothing was recorded).
    func stopRecording() -> (url: URL, duration: TimeInterval)? {
        lock.withLock {
            defer { recording = nil }
            guard let recording, recording.file != nil, recording.frames > 0 else { return nil }
            return (recording.url, Double(recording.frames) / recording.rate) // the file closes when released
        }
    }

    private func write(_ buffer: AVAudioPCMBuffer) {
        lock.withLock {
            guard var current = recording else { return }
            if current.file == nil {
                let settings: [String: Any] = [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: buffer.format.sampleRate,
                                               AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 48_000]
                current.file = try? AVAudioFile(forWriting: current.url, settings: settings,
                                                commonFormat: .pcmFormatFloat32, interleaved: false)
                current.rate = buffer.format.sampleRate
            }
            guard let file = current.file, (try? file.write(from: buffer)) != nil else { return }
            current.frames += AVAudioFramePosition(buffer.frameLength)
            recording = current
        }
    }

    /// Built in a nonisolated context on purpose: a closure formed inside a `@MainActor` method would be
    /// main-actor isolated and trap when AVAudioEngine calls it on its render thread.
    nonisolated static func tap(_ sink: CaptureSink, _ source: Source) -> AVAudioNodeTapBlock {
        { buffer, time in sink.receive(buffer, at: time, from: source) }
    }
}

/// One `AVAudioEngine`: mic input with voice processing (AEC, needed for barge-in) tapped into the
/// `CaptureSink`, and an `AVAudioPlayerNode` → main mixer (tapped for the output level).
///
/// The player is connected with a fixed format (Float32 mono 24 kHz, the bridge's rate) so buffers
/// converted before a route change remain valid after the graph is rebuilt; the mixer does the hardware
/// rate conversion. Rebuilt from scratch on configuration changes (route change, interruption) since
/// formats may change underneath.
@MainActor
final class AudioGraph {
    nonisolated static var playerFormat: AVAudioFormat {
        AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 1, interleaved: false)!
    }

    let engine = AVAudioEngine()
    let player = AVAudioPlayerNode()
    let capturesInput: Bool
    private(set) var voiceProcessingEnabled = false
    private var observer: NSObjectProtocol?

    private let sink: CaptureSink

    init(capture: Bool, voiceProcessing: Bool = true, sink: CaptureSink,
         onConfigurationChange: @escaping @MainActor @Sendable () -> Void) throws {
        capturesInput = capture
        self.sink = sink
        if capture {
            let input = engine.inputNode
            if voiceProcessing {
                do {
                    try input.setVoiceProcessingEnabled(true)
                    voiceProcessingEnabled = true
                } catch {
                    // Simulator / unsupported route: keep going without AEC (barge-in will be less reliable).
                }
            }
            try installInputTap()
        }
        engine.attach(player)
        engine.connect(player, to: engine.mainMixerNode, format: Self.playerFormat)
        engine.mainMixerNode.installTap(onBus: 0, bufferSize: 1024, format: nil, block: CaptureSink.tap(sink, .output))

        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: .main) { _ in
            MainActor.assumeIsolated { onConfigurationChange() }
        }
        engine.prepare()
        do {
            try engine.start()
        } catch {
            stop()
            throw error
        }
    }

    /// Restarts this same engine after a configuration change (Apple's recommended handling). Recreating
    /// a voice-processing engine instead re-triggers the change and loops.
    func restart() throws {
        if capturesInput {
            engine.inputNode.removeTap(onBus: 0)
            try installInputTap()
        }
        engine.prepare()
        try engine.start()
    }

    private func installInputTap() throws {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        #if DEBUG
        print("[voice] input hw=\(input.inputFormat(forBus: 0)) out=\(format) vp=\(voiceProcessingEnabled) — \(AudioDiagnostics.describe())")
        #endif
        guard format.sampleRate > 0, format.channelCount > 0 else { throw VoiceError.microphoneUnavailable }
        input.installTap(onBus: 0, bufferSize: 1024, format: format, block: CaptureSink.tap(sink, .input))
    }

    func stop() {
        if let observer { NotificationCenter.default.removeObserver(observer) }
        observer = nil
        player.stop()
        engine.stop()
        if capturesInput { engine.inputNode.removeTap(onBus: 0) }
        engine.mainMixerNode.removeTap(onBus: 0)
    }
}

/// `AudioOutput` over the current graph's player node (swapped when the graph is rebuilt).
@MainActor
final class PlayerOutput: AudioOutput {
    var node: AVAudioPlayerNode?

    func schedule(_ buffer: AVAudioPCMBuffer, completion: @escaping @MainActor @Sendable () -> Void) {
        guard let node else {
            Task { @MainActor in completion() } // no graph: drop, but keep the queue moving
            return
        }
        // `.dataPlayedBack` fires once the audio has left the speaker, not when it was merely consumed.
        node.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack, completionHandler: Self.hop(completion))
    }

    func play() {
        guard let node, node.engine?.isRunning == true, !node.isPlaying else { return }
        node.play()
    }

    func stop() {
        node?.stop()
    }

    nonisolated private static func hop(_ completion: @escaping @MainActor @Sendable () -> Void)
        -> @Sendable (AVAudioPlayerNodeCompletionCallbackType) -> Void {
        { _ in Task { @MainActor in completion() } }
    }
}
