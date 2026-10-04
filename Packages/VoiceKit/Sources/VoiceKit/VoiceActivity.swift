import Foundation

/// One audio analysis frame from the capture pipeline.
public struct VoiceSample: Sendable, Hashable {
    /// Seconds, monotonic (e.g. host time of the buffer).
    public var timestamp: TimeInterval
    /// Linear RMS level of the frame, 0…1.
    public var rmsLevel: Float
    /// Whether the speech recogniser currently has a non-empty partial transcript.
    public var hasPartialTranscript: Bool

    public init(timestamp: TimeInterval, rmsLevel: Float, hasPartialTranscript: Bool) {
        self.timestamp = timestamp
        self.rmsLevel = rmsLevel
        self.hasPartialTranscript = hasPartialTranscript
    }
}

/// Detects the start and the end of a user utterance from level + transcript samples.
///
/// Speech starts when the level stays above `speechThreshold` for `minSpeechDuration`. The utterance
/// ends after `silenceDuration` below the threshold, provided a transcript exists; level-only bursts
/// without any transcript are discarded silently (noise). The transcript flag deliberately does not start
/// speech on its own: the recogniser's partial text lingers until the app resets it after an utterance.
public struct EndOfUtteranceDetector: Sendable {
    public struct Configuration: Sendable, Hashable {
        public var speechThreshold: Float
        /// Silence that ends an utterance (SPEC: ≈ 600–800 ms, adjustable).
        public var silenceDuration: TimeInterval
        public var minSpeechDuration: TimeInterval

        public init(speechThreshold: Float = 0.02, silenceDuration: TimeInterval = 0.7, minSpeechDuration: TimeInterval = 0.1) {
            self.speechThreshold = speechThreshold
            self.silenceDuration = silenceDuration
            self.minSpeechDuration = minSpeechDuration
        }
    }

    public enum Event: Sendable, Hashable {
        case speechStarted
        case utteranceEnded
    }

    public var configuration: Configuration
    public private(set) var isSpeaking = false
    private var loudSince: TimeInterval?
    private var lastVoiceAt: TimeInterval = 0

    public init(configuration: Configuration = Configuration()) {
        self.configuration = configuration
    }

    public mutating func process(_ sample: VoiceSample) -> Event? {
        let loud = sample.rmsLevel >= configuration.speechThreshold
        if !isSpeaking {
            loudSince = loud ? (loudSince ?? sample.timestamp) : nil
            let sustained = loudSince.map { sample.timestamp - $0 >= configuration.minSpeechDuration } ?? false
            guard sustained else { return nil }
            isSpeaking = true
            lastVoiceAt = sample.timestamp
            return .speechStarted
        }
        if loud {
            lastVoiceAt = sample.timestamp
            return nil
        }
        guard sample.timestamp - lastVoiceAt >= configuration.silenceDuration else { return nil }
        let hadSpeech = sample.hasPartialTranscript
        reset()
        return hadSpeech ? .utteranceEnded : nil
    }

    public mutating func process(timestamp: TimeInterval, rmsLevel: Float, hasPartialTranscript: Bool) -> Event? {
        process(VoiceSample(timestamp: timestamp, rmsLevel: rmsLevel, hasPartialTranscript: hasPartialTranscript))
    }

    public mutating func reset() {
        isSpeaking = false
        loudSince = nil
        lastVoiceAt = 0
    }
}

/// Detects the user talking over TTS playback: level above `speechThreshold` for `minSpeechDuration`
/// (short dips up to `gapTolerance` allowed) **and** a non-empty partial transcript, so echo residue
/// or a cough does not interrupt. Fires once per playback.
public struct BargeInDetector: Sendable {
    public struct Configuration: Sendable, Hashable {
        /// Higher than the end-of-utterance threshold: echo cancellation leaves residue.
        public var speechThreshold: Float
        public var minSpeechDuration: TimeInterval
        public var gapTolerance: TimeInterval

        public init(speechThreshold: Float = 0.05, minSpeechDuration: TimeInterval = 0.25, gapTolerance: TimeInterval = 0.12) {
            self.speechThreshold = speechThreshold
            self.minSpeechDuration = minSpeechDuration
            self.gapTolerance = gapTolerance
        }
    }

    public enum Event: Sendable, Hashable {
        case bargeIn
    }

    public var configuration: Configuration
    public private(set) var isPlaybackActive = false
    private var speechSince: TimeInterval?
    private var lastLoudAt: TimeInterval?
    private var fired = false

    public init(configuration: Configuration = Configuration()) {
        self.configuration = configuration
    }

    public mutating func playbackStarted() {
        isPlaybackActive = true
        fired = false
        speechSince = nil
        lastLoudAt = nil
    }

    public mutating func playbackStopped() {
        isPlaybackActive = false
        speechSince = nil
        lastLoudAt = nil
    }

    public mutating func process(_ sample: VoiceSample) -> Event? {
        guard isPlaybackActive, !fired else { return nil }
        if sample.rmsLevel >= configuration.speechThreshold {
            if speechSince == nil { speechSince = sample.timestamp }
            lastLoudAt = sample.timestamp
        } else if let lastLoudAt, sample.timestamp - lastLoudAt > configuration.gapTolerance {
            speechSince = nil
            self.lastLoudAt = nil
        }
        guard let speechSince, sample.timestamp - speechSince >= configuration.minSpeechDuration,
              sample.hasPartialTranscript else { return nil }
        fired = true
        return .bargeIn
    }

    public mutating func process(timestamp: TimeInterval, rmsLevel: Float, hasPartialTranscript: Bool) -> Event? {
        process(VoiceSample(timestamp: timestamp, rmsLevel: rmsLevel, hasPartialTranscript: hasPartialTranscript))
    }
}
