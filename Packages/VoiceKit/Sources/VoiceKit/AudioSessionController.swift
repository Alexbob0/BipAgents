import AVFoundation

/// Owns the `AVAudioSession` configuration and its notifications. No-op on macOS (no audio session).
///
/// - `.voice` (dictation, call): `.playAndRecord` + `.voiceChat` (enables the voice-processing echo
///   canceller path), `.defaultToSpeaker` (otherwise playback goes to the earpiece), `.allowBluetoothHFP`.
/// - `.playback` ("Écouter" outside a call): `.playback` + `.spokenAudio`, so AirPods stay on A2DP
///   (HFP would drop them to narrow-band mono) and no microphone is opened.
@MainActor
final class AudioSessionController {
    enum Purpose { case voice, playback }

    /// `began`, `shouldResume`.
    var onInterruption: ((Bool, Bool) -> Void)?
    /// `true` when the previous output disappeared (headphones unplugged…).
    var onRouteChange: ((Bool) -> Void)?
    var onMediaServicesReset: (() -> Void)?

    private(set) var activePurpose: Purpose?
    nonisolated(unsafe) private var observers: [NSObjectProtocol] = []

    init() {
        #if os(iOS)
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] note in
                let type = (note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt).flatMap(AVAudioSession.InterruptionType.init)
                let options = (note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt).map(AVAudioSession.InterruptionOptions.init)
                MainActor.assumeIsolated {
                    guard let self, self.activePurpose != nil, let type else { return }
                    self.onInterruption?(type == .began, options?.contains(.shouldResume) ?? false)
                }
            },
            center.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] note in
                let reason = (note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt).flatMap(AVAudioSession.RouteChangeReason.init)
                MainActor.assumeIsolated {
                    guard let self, self.activePurpose != nil else { return }
                    self.onRouteChange?(reason == .oldDeviceUnavailable)
                }
            },
            center.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, self.activePurpose != nil else { return }
                    self.onMediaServicesReset?()
                }
            },
        ]
        #endif
    }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    func activate(_ purpose: Purpose) throws {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        switch purpose {
        case .voice:
            try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothHFP])
            try? session.setPreferredIOBufferDuration(0.01) // finer level frames → quicker barge-in
        case .playback:
            try session.setCategory(.playback, mode: .spokenAudio, options: [])
        }
        try session.setActive(true)
        #endif
        if activePurpose == nil { AudioSessionUsage.begin() }
        activePurpose = purpose
    }

    func deactivate() {
        guard activePurpose != nil else { return }
        activePurpose = nil
        AudioSessionUsage.end()
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
    }
}
