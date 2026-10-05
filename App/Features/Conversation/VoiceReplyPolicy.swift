import AVFoundation
import Foundation

/// When a voice note gets a spoken answer. Generating audio costs Kyutai time on the server and can be
/// unwelcome, so by default it only happens when it is clearly useful; « Écouter » always works on demand.
enum VoiceReplyPolicy: String, CaseIterable, Identifiable {
    /// Never automatic.
    case never
    /// After a voice note, when the user is likely not looking at the screen (headphones, car) or asked for it.
    case smart
    /// After every voice note.
    case always

    var id: String { rawValue }

    static let storageKey = "voiceReplyPolicy"

    static var current: VoiceReplyPolicy {
        UserDefaults.standard.string(forKey: storageKey).flatMap(VoiceReplyPolicy.init(rawValue:)) ?? .smart
    }

    var label: String {
        switch self {
        case .never: String(localized: "Jamais")
        case .smart: String(localized: "Intelligent")
        case .always: String(localized: "Toujours après un vocal")
        }
    }

    func shouldReplyByVoice(to transcript: String) -> Bool {
        switch self {
        case .never: false
        case .always: true
        case .smart: Self.asksForVoice(transcript) || Self.isListeningHandsFree
        }
    }

    /// « réponds-moi en vocal », "read it to me", « léemelo », „lies es mir vor“…: any of the app's languages,
    /// whatever the agent's (people mix them).
    static func asksForVoice(_ text: String) -> Bool {
        let folded = text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: nil)
            .replacingOccurrences(of: "’", with: "'")
        return voiceCues.contains { folded.contains($0) }
    }

    /// Lowercased, without accents (the text is folded the same way).
    static let voiceCues = [
        // Français
        "en vocal", "un vocal", "message vocal", "a l'oral", "a voix haute", "lis le moi", "lis-le moi", "lis la moi",
        "lis-la moi", "lis moi", "lis-moi", "dis le moi", "dis-le moi", "reponds moi a l'oral", "en audio", "audio stp",
        "audio s'il te plait",
        // English
        "voice message", "voice note", "by voice", "read it to me", "read it out", "read me this", "read me the", "out loud", "aloud",
        "as audio", "in audio", "audio please",
        // Español
        "por voz", "mensaje de voz", "nota de voz", "en voz alta", "leemelo", "leemela", "audio por favor",
        "en un audio",
        // Deutsch
        "sprachnachricht", "sprachmemo", "vorlesen", "lies es mir vor", "lies mir", "laut vor", "als audio",
        "per audio", "audio bitte", "per sprache",
    ]

    /// Headphones, AirPods, CarPlay or a car's Bluetooth: the answer is better heard than read.
    static var isListeningHandsFree: Bool {
        let handsFree: Set<AVAudioSession.Port> = [.headphones, .bluetoothA2DP, .bluetoothHFP, .bluetoothLE, .carAudio, .airPlay]
        return AVAudioSession.sharedInstance().currentRoute.outputs.contains { handsFree.contains($0.portType) }
    }
}
