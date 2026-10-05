import Foundation
import HermesKit

/// The language an agent is spoken to in: drives on-device speech recognition and its voice. The app's own
/// interface follows the iPhone's language instead (an English iPhone can still talk to a French agent).
enum AgentLanguage: String, CaseIterable, Identifiable, Sendable {
    case french = "fr", english = "en", spanish = "es", german = "de"

    var id: String { rawValue }

    /// Shown in its own language, like iOS does in language lists.
    var nativeName: String {
        switch self {
        case .french: "Français"
        case .english: "English"
        case .spanish: "Español"
        case .german: "Deutsch"
        }
    }

    var locale: Locale {
        switch self {
        case .french: Locale(identifier: "fr-FR")
        case .english: Locale(identifier: "en-US")
        case .spanish: Locale(identifier: "es-ES")
        case .german: Locale(identifier: "de-DE")
        }
    }

    /// The Bip voices and the bridge's text preparation are French only for now: other languages are read by
    /// the iPhone's own voice.
    var usesBridgeVoice: Bool { self == .french }

    /// The iPhone's first preferred language we support, else English.
    static var device: AgentLanguage {
        for identifier in Locale.preferredLanguages {
            if let code = Locale(identifier: identifier).language.languageCode?.identifier,
               let language = AgentLanguage(rawValue: code) {
                return language
            }
        }
        return .english
    }
}

extension AgentProfile {
    /// Agents saved before languages existed were all French.
    var language: AgentLanguage { config.language.flatMap(AgentLanguage.init(rawValue:)) ?? .french }
}
