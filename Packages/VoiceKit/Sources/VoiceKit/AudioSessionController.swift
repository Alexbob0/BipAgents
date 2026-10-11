import AVFoundation

/// Owns the `AVAudioSession` configuration and its notifications. No-op on macOS (no audio session).
///
/// - `.voice` (call): `.playAndRecord` + `.voiceChat` (enables the voice-processing echo canceller path),
///   `.defaultToSpeaker` (otherwise playback goes to the earpiece), `.allowBluetoothHFP`.
/// - `.dictation` (voice note): the same without `.voiceChat`. Nothing plays while recording, so there is no echo
///   to cancel, and the voice-processing unit takes half a second to deliver its first buffer.
/// - `.playback` ("Écouter" outside a call): `.playback` + `.spokenAudio`, so AirPods stay on A2DP
///   (HFP would drop them to narrow-band mono) and no microphone is opened.
@MainActor
final class AudioSessionController {
    enum Purpose: Sendable { case voice, dictation, playback }

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

    /// Off the main thread: activating the session takes tens of milliseconds, during which the screen would freeze.
    func activate(_ purpose: Purpose) async throws {
        #if os(iOS)
        try await Task.detached(priority: .userInitiated) { try Self.configure(purpose) }.value
        #endif
        if activePurpose == nil { AudioSessionUsage.begin() }
        activePurpose = purpose
    }

    #if os(iOS)
    nonisolated private static func configure(_ purpose: Purpose) throws {
        let session = AVAudioSession.sharedInstance()
        switch purpose {
        case .voice:
            try session.setCategory(.playAndRecord, mode: .voiceChat, options: [.defaultToSpeaker, .allowBluetoothHFP])
            try? session.setPreferredIOBufferDuration(0.01) // finer level frames → quicker barge-in
        case .dictation:
            try session.setCategory(.playAndRecord, mode: .default, options: [.defaultToSpeaker, .allowBluetoothHFP])
        case .playback:
            try session.setCategory(.playback, mode: .spokenAudio, options: [])
        }
        try session.setActive(true)
    }
    #endif

    func deactivate() {
        guard activePurpose != nil else { return }
        activePurpose = nil
        AudioSessionUsage.end()
        #if os(iOS)
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
        #endif
    }
}
