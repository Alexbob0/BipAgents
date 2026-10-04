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
        case .never: "Jamais"
        case .smart: "Intelligent"
        case .always: "Toujours après un vocal"
        }
    }

    func shouldReplyByVoice(to transcript: String) -> Bool {
        switch self {
        case .never: false
        case .always: true
        case .smart: Self.asksForVoice(transcript) || Self.isListeningHandsFree
        }
    }

    /// « réponds-moi en vocal », « lis-le moi », « dis-le à voix haute », « à l'oral »…
    static func asksForVoice(_ text: String) -> Bool {
        let folded = text.folding(options: [.caseInsensitive, .diacriticInsensitive], locale: Locale(identifier: "fr_FR"))
        let cues = ["en vocal", "un vocal", "message vocal", "a l'oral", "a l’oral", "a voix haute", "lis le moi", "lis-le moi",
                    "lis la moi", "lis-la moi", "lis moi", "lis-moi", "dis le moi", "dis-le moi", "reponds moi a l'oral",
                    "en audio", "audio stp", "audio s'il te plait"]
        return cues.contains { folded.contains($0) }
    }

    /// Headphones, AirPods, CarPlay or a car's Bluetooth: the answer is better heard than read.
    static var isListeningHandsFree: Bool {
        let handsFree: Set<AVAudioSession.Port> = [.headphones, .bluetoothA2DP, .bluetoothHFP, .bluetoothLE, .carAudio, .airPlay]
        return AVAudioSession.sharedInstance().currentRoute.outputs.contains { handsFree.contains($0.portType) }
    }
}
