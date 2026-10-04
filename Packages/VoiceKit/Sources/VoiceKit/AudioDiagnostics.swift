import AVFoundation

/// One-line description of the audio route, logged when capture fails (device-only issues).
enum AudioDiagnostics {
    static func describe() -> String {
        #if os(iOS)
        let session = AVAudioSession.sharedInstance()
        let inputs = session.currentRoute.inputs.map { "\($0.portType.rawValue)" }.joined(separator: ",")
        let outputs = session.currentRoute.outputs.map { "\($0.portType.rawValue)" }.joined(separator: ",")
        return "category=\(session.category.rawValue) mode=\(session.mode.rawValue) inputAvailable=\(session.isInputAvailable) "
            + "inputs=[\(inputs)] outputs=[\(outputs)] rate=\(session.sampleRate) inChannels=\(session.inputNumberOfChannels) "
            + "record=\(AVAudioApplication.shared.recordPermission.rawValue)"
        #else
        return "macOS"
        #endif
    }
}
