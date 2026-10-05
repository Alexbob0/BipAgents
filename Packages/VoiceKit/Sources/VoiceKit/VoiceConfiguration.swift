import Foundation

public struct VoiceConfiguration: Sendable {
    public var locale: Locale = Locale(identifier: "fr-FR")
    /// Silence after speech that ends an utterance in a call (SPEC: ≈ 600–800 ms).
    public var endOfUtteranceSilence: Duration = .milliseconds(700)
    public var bargeInEnabled: Bool = true
    /// Linear RMS (0…1) above which the mic counts as speech for end-of-utterance. Calibrate on device.
    public var speechThreshold: Float = 0.02
    /// Barge-in detector tuning (threshold above echo residue, minimum duration, gap tolerance).
    public var bargeIn: BargeInDetector.Configuration = .init()

    public init(locale: Locale = Locale(identifier: "fr-FR"),
                endOfUtteranceSilence: Duration = .milliseconds(700),
                bargeInEnabled: Bool = true,
                speechThreshold: Float = 0.02,
                bargeIn: BargeInDetector.Configuration = .init()) {
        self.locale = locale
        self.endOfUtteranceSilence = endOfUtteranceSilence
        self.bargeInEnabled = bargeInEnabled
        self.speechThreshold = speechThreshold
        self.bargeIn = bargeIn
    }

    var endOfUtterance: EndOfUtteranceDetector.Configuration {
        .init(speechThreshold: speechThreshold, silenceDuration: endOfUtteranceSilence.timeInterval)
    }
}

/// A recorded voice note: the audio file, its transcript and a coarse waveform (0…1, ~40 bars).
public struct VoiceRecording: Sendable, Hashable {
    public var url: URL
    public var duration: TimeInterval
    public var transcript: String
    public var waveform: [Float]

    public init(url: URL, duration: TimeInterval, transcript: String, waveform: [Float]) {
        self.url = url
        self.duration = duration
        self.transcript = transcript
        self.waveform = waveform
    }
}

public struct VoiceMetrics: Sendable, Equatable {
    /// Speech end (end-of-utterance decision, or `finishDictation()`) → final transcript.
    public var lastSTTDuration: Duration?
    /// First text delta received → first audio scheduled.
    public var lastFirstAudioLatency: Duration?
    /// Speech detected during playback (barge-in decision, timed on the mic buffer) → playback stopped.
    public var lastBargeInLatency: Duration?

    public init(lastSTTDuration: Duration? = nil, lastFirstAudioLatency: Duration? = nil, lastBargeInLatency: Duration? = nil) {
        self.lastSTTDuration = lastSTTDuration
        self.lastFirstAudioLatency = lastFirstAudioLatency
        self.lastBargeInLatency = lastBargeInLatency
    }
}

public enum VoiceError: Error, Sendable, Equatable, CustomStringConvertible {
    case permissionDenied
    case recognizerUnavailable
    case microphoneUnavailable
    case busy

    public var description: String {
        switch self {
        case .permissionDenied: String(localized: "Micro ou reconnaissance vocale non autorisés", bundle: .module)
        case .recognizerUnavailable: String(localized: "Reconnaissance vocale indisponible pour cette langue", bundle: .module)
        case .microphoneUnavailable: String(localized: "Micro indisponible", bundle: .module)
        case .busy: String(localized: "La voix est déjà utilisée", bundle: .module)
        }
    }
}

extension Duration {
    var timeInterval: TimeInterval {
        let (seconds, attoseconds) = components
        return Double(seconds) + Double(attoseconds) * 1e-18
    }
}
