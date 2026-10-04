import AVFoundation

/// Plays streamed speech (`TTSProvider.stream`) as it arrives and records it to an AAC file at the same
/// time, so a spoken reply starts in about a second and can be replayed later as a voice note.
///
/// Uses the `.playback` / `.spokenAudio` session (AirPods stay on A2DP, no microphone) and registers with
/// `AudioSessionUsage` while it holds the session.
@MainActor
public final class StreamingSpeechPlayer {
    public enum Failure: Error {
        /// The stream ended before any audio.
        case noAudio
    }

    private let engine = AVAudioEngine()
    private let node = AVAudioPlayerNode()
    private let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 1, interleaved: false)!
    private lazy var converter = FormatConverter(outputFormat: format)
    private var configured = false
    private var pending = 0
    private var drained: CheckedContinuation<Void, Never>?
    private var generation = 0

    public private(set) var isPlaying = false

    public init() {}

    /// Plays `chunks` and writes them to `url` (`.m4a`). Returns the audio duration once everything has
    /// been played. Throws the stream's error if it fails before any audio; a failure after the first
    /// chunk ends playback early and still returns what was recorded. Cancelling stops playback.
    public func play(_ chunks: AsyncThrowingStream<PCMChunk, any Error>, recordingTo url: URL) async throws -> TimeInterval {
        stop()
        generation += 1
        let current = generation
        try activate()
        AudioSessionUsage.begin()
        isPlaying = true
        defer {
            if generation == current { finishPlayback() }
            AudioSessionUsage.end()
        }

        var file: AVAudioFile?
        var frames: AVAudioFramePosition = 0
        do {
            for try await chunk in chunks {
                try Task.checkCancellation()
                guard generation == current else { throw CancellationError() }
                guard let source = chunk.pcmBuffer(), let buffer = converter.convert(source, streaming: true) else { continue }
                if file == nil {
                    file = try AVAudioFile(forWriting: url, settings: [
                        AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: format.sampleRate,
                        AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 48_000,
                    ], commonFormat: .pcmFormatFloat32, interleaved: false)
                }
                try file?.write(from: buffer)
                frames += AVAudioFramePosition(buffer.frameLength)
                schedule(buffer, generation: current)
            }
        } catch {
            guard frames > 0, !(error is CancellationError), !Task.isCancelled else { throw error }
            #if DEBUG
            print("[voice] streamed reply cut after \(Double(frames) / format.sampleRate)s: \(error)")
            #endif
        }
        guard frames > 0 else { throw Failure.noAudio }
        file = nil // closes and finalizes the file
        await withTaskCancellationHandler {
            await waitUntilPlayed()
        } onCancel: {
            Task { @MainActor in self.stop() }
        }
        try Task.checkCancellation()
        guard generation == current else { throw CancellationError() } // stopped by `stop()`
        return Double(frames) / format.sampleRate
    }

    /// Stops at once; a `play` in progress then throws `CancellationError`.
    public func stop() {
        generation += 1
        if configured { node.stop() }
        finishPlayback()
    }

    private func activate() throws {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        try session.setCategory(.playback, mode: .spokenAudio)
        try session.setActive(true)
        #endif
        if !configured {
            engine.attach(node)
            engine.connect(node, to: engine.mainMixerNode, format: format)
            configured = true
        }
        if !engine.isRunning { try engine.start() }
        node.play()
    }

    private func schedule(_ buffer: AVAudioPCMBuffer, generation scheduled: Int) {
        pending += 1
        node.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.generation == scheduled else { return }
                self.pending -= 1
                if self.pending == 0 { self.resumeDrain() }
            }
        }
    }

    private func waitUntilPlayed() async {
        guard pending > 0, isPlaying else { return }
        await withCheckedContinuation { drained = $0 }
    }

    private func resumeDrain() {
        drained?.resume()
        drained = nil
    }

    private func finishPlayback() {
        pending = 0
        isPlaying = false
        resumeDrain()
        if engine.isRunning { engine.stop() }
    }
}
