import HermesKit
import SwiftUI
import VoiceKit

/// A voice an agent can speak with. `id` is the choice stored with the agent: `drama:<id>` for the Bips' natural and
/// mascot voices (Fish Audio Drama 3 on the server's GPU, one language each, `voices/drama/` in the repo),
/// `pocket:<name>` for the quick Bips (Kyutai Pocket TTS, `voices/<language>/`), a Kyutai alias otherwise. The
/// bridge receives it per language (`AgentVoices.spoken`).
struct AgentVoiceOption: Identifiable, Hashable {
    let id: String
    let name: String
    let character: String
    /// Bundled preview (`App/Resources/Voices/<sample>.m4a`, `<sample>-<lang>.m4a` outside French for Pocket), if any.
    let sample: String?
    /// The one language it speaks (Drama 3 voices); nil: any.
    var language: AgentLanguage? = nil
    var isMascot = false

    func sampleURL(in language: AgentLanguage) -> URL? {
        sample.flatMap {
            Bundle.main.url(forResource: language == .french || self.language != nil ? $0 : "\($0)-\(language.rawValue)",
                            withExtension: "m4a")
        }
    }
}

enum AgentVoices {
    static let dramaPrefix = "drama:"

    private static func drama(_ id: String, _ name: String, _ character: String, mascot: Bool = false) -> AgentVoiceOption {
        AgentVoiceOption(id: dramaPrefix + id, name: name, character: character, sample: "drama-\(id)",
                         language: AgentLanguage(rawValue: String(id.prefix(2))), isMascot: mascot)
    }

    /// The Bips' voices on Drama 3: natural (trees) and mascots (birds), from the deepest. Only on a bridge that has it.
    static let dramaVoices: [AgentVoiceOption] = [
        drama("fr-naturelle-s15", "Chêne", String(localized: "Grave et posée")),
        drama("fr-naturelle-s32", "Orme", String(localized: "Grave et chaleureuse")),
        drama("fr-naturelle-s21", "Frêne", String(localized: "Médium et claire")),
        drama("fr-naturelle-s28", "Saule", String(localized: "Aiguë et douce")),
        drama("fr-mascotte-s21", "Grillon", String(localized: "Mascotte espiègle, au timbre médium"), mascot: true),
        drama("fr-mascotte-s17", "Pinson", String(localized: "Mascotte vive et joyeuse"), mascot: true),
        drama("fr-mascotte-s28", "Mésange", String(localized: "Mascotte aiguë et pétillante"), mascot: true),
        drama("en-naturelle-s47", "Oak", String(localized: "Grave et posée")),
        drama("en-naturelle-s92", "Elm", String(localized: "Grave et chaleureuse")),
        drama("en-naturelle-s42", "Ash", String(localized: "Médium et claire")),
        drama("en-naturelle-s57", "Willow", String(localized: "Aiguë et douce")),
        drama("en-mascotte-s47", "Wren", String(localized: "Mascotte espiègle, au timbre médium"), mascot: true),
        drama("en-mascotte-s44", "Finch", String(localized: "Mascotte vive et joyeuse"), mascot: true),
        drama("en-mascotte-s42", "Robin", String(localized: "Mascotte aiguë et pétillante"), mascot: true),
    ]

    static func dramaVoices(in language: AgentLanguage) -> [AgentVoiceOption] {
        dramaVoices.filter { $0.language == language }
    }

    /// Agents whose bridge serves Drama 3, as its `/health` last said (kept for when it can't be asked).
    private static let dramaKey = "dramaAgents"
    static func servesDrama(_ agent: AgentProfile) -> Bool {
        #if DEBUG
        if UserDefaults.standard.bool(forKey: "demoDrama") { return true }   // design review: `-demoDrama YES`
        #endif
        return UserDefaults.standard.stringArray(forKey: dramaKey)?.contains(agent.id.uuidString) == true
    }
    static func setServesDrama(_ on: Bool, for agent: AgentProfile) {
        var ids = Set(UserDefaults.standard.stringArray(forKey: dramaKey) ?? [])
        if on { ids.insert(agent.id.uuidString) } else { ids.remove(agent.id.uuidString) }
        UserDefaults.standard.set(Array(ids).sorted(), forKey: dramaKey)
    }

    static let bips: [AgentVoiceOption] = [
        AgentVoiceOption(id: "pocket:mousse", name: "Mousse", character: String(localized: "Ronde et légère, chaleureuse sans en faire trop"), sample: "voice-mousse"),
        AgentVoiceOption(id: "pocket:colibri", name: String(localized: "Colibri"), character: String(localized: "Vive et enjouée, claire et chantante"), sample: "voice-colibri"),
        AgentVoiceOption(id: "pocket:lumen", name: "Lumen", character: String(localized: "Claire et très articulée, idéale pour les infos"), sample: "voice-lumen"),
        AgentVoiceOption(id: "pocket:galet", name: "Galet", character: String(localized: "Posée et nette, un guide calme"), sample: "voice-galet"),
        AgentVoiceOption(id: "pocket:ours", name: String(localized: "Ours"), character: String(localized: "Calme et rassurant, grave et doux"), sample: "voice-ours"),
    ]
    /// Bip voices that were replaced (the first series): agents that had picked one go back to their category's.
    static let retired: Set<String> = ["pocket:loutre", "pocket:chat2", "pocket:lutin"]
    /// Kyutai 1.6B's human voice, the one of the morning podcast.
    static let classic = AgentVoiceOption(id: "5476", name: String(localized: "Voix classique"), character: String(localized: "Voix humaine posée, celle du podcast"), sample: nil)

    /// Apple's voice, on the phone: no server involved (Live, « Écouter », voice replies).
    static let iphone = AgentVoiceOption(id: "system", name: String(localized: "Voix de l’iPhone"),
                                         character: String(localized: "La voix d’Apple, directement sur le téléphone, sans serveur"), sample: nil)

    static var all: [AgentVoiceOption] { dramaVoices + bips + [classic, iphone] }

    /// Pocket's Bips speak every language, Drama 3's voices only theirs (and only on a bridge that has it); Kyutai's
    /// human voice is French.
    static func options(in language: AgentLanguage, drama: Bool = false) -> [AgentVoiceOption] {
        (drama ? dramaVoices(in: language) : []) + (language == .french ? bips + [classic, iphone] : bips + [iphone])
    }

    /// What the bridge receives: `pocket:galet` in French, `pocket:en/galet` in English, `drama:<id>` as it is…
    static func spoken(_ id: String, in language: AgentLanguage) -> String {
        guard language != .french, id.hasPrefix("pocket:") else { return id }
        return "pocket:\(language.rawValue)/" + id.dropFirst("pocket:".count)
    }

    static func option(for id: String) -> AgentVoiceOption {
        all.first { $0.id == id } ?? AgentVoiceOption(id: id, name: id, character: String(localized: "Voix personnalisée"), sample: nil)
    }
}

extension AgentCategory {
    /// The voice an agent of this category gets until one is picked: a Drama 3 voice of its language when its bridge
    /// has them, else a Pocket Bip.
    func defaultVoice(in language: AgentLanguage, drama: Bool) -> String {
        guard drama, language == .french || language == .english else { return defaultVoice }
        let fr = language == .french
        let id = switch self {
        case .wellness: fr ? "fr-mascotte-s28" : "en-mascotte-s42"
        case .daily: fr ? "fr-mascotte-s17" : "en-mascotte-s44"
        case .creative, .learning: fr ? "fr-naturelle-s28" : "en-naturelle-s57"
        case .work, .tech: fr ? "fr-naturelle-s32" : "en-naturelle-s92"
        case .finance, .home: fr ? "fr-naturelle-s15" : "en-naturelle-s47"
        }
        return AgentVoices.dramaPrefix + id
    }

    /// The Pocket Bip an agent of this category gets until one is picked (no Drama 3).
    var defaultVoice: String {
        switch self {
        case .wellness: "pocket:mousse"
        case .daily: "pocket:colibri"
        case .creative, .learning: "pocket:lumen"
        case .work, .tech: "pocket:galet"
        case .finance, .home: "pocket:ours"
        }
    }
}

extension AgentProfile {
    /// The voice picked for this agent in its language, else the category's Bip voice.
    var voiceChoice: String {
        let drama = AgentVoices.servesDrama(self)
        guard let picked = config.voice,
              AgentVoices.options(in: language, drama: drama).contains(where: { $0.id == picked })
                || (language == .french && !picked.hasPrefix(AgentVoices.dramaPrefix))
        else { return appearance.category.defaultVoice(in: language, drama: drama) }
        return picked
    }

    /// The voice sent to the bridge.
    var voice: String { AgentVoices.spoken(voiceChoice, in: language) }

    /// Speaks with the iPhone's own voice (picked in its settings), never through the bridge.
    var usesSystemVoice: Bool { voiceChoice == AgentVoices.iphone.id }
}

/// Voice choice for one agent, with a preview of each Bip voice.
struct AgentVoicePicker: View {
    @Binding var voice: String?
    var category: AgentCategory
    var language: AgentLanguage = .french
    /// The agent's bridge serves Drama 3's voices.
    var drama = false
    @State private var player = VoiceNotePlayer()

    private var defaultVoice: String { category.defaultVoice(in: language, drama: drama) }

    private var selected: String {
        guard let voice, AgentVoices.options(in: language, drama: drama).contains(where: { $0.id == voice }) else { return defaultVoice }
        return voice
    }

    var body: some View {
        List {
            let dramaVoices = drama ? AgentVoices.dramaVoices(in: language) : []
            if !dramaVoices.isEmpty {
                Section {
                    ForEach(dramaVoices.filter { !$0.isMascot }) { row($0) }
                } header: {
                    Text("Voix naturelles")
                }
                Section {
                    ForEach(dramaVoices.filter(\.isMascot)) { row($0) }
                } header: {
                    Text("Voix mascottes")
                } footer: {
                    Text("Voix expressives calculées sur le serveur avec Fish Audio Drama 3. Built with Fish Audio.")
                }
            }
            Section {
                ForEach(AgentVoices.bips) { row($0) }
            } header: {
                Text(dramaVoices.isEmpty ? "Voix de Bip" : "Voix de Bip rapides")
            } footer: {
                Text("Voix synthétiques calculées avec Kyutai Pocket TTS : la réponse démarre presque instantanément.")
            }
            if language == .french {
                Section {
                    row(AgentVoices.classic)
                } footer: {
                    Text("La voix humaine de Kyutai, plus posée mais plus lente à démarrer.")
                }
            }
            Section {
                row(AgentVoices.iphone)
            } footer: {
                Text("Pour une meilleure qualité, télécharge une voix « Premium » ou « Améliorée » dans Réglages iOS › Accessibilité › Contenu énoncé › Voix.")
            }
        }
        .navigationTitle("Voix")
        .navigationBarTitleDisplayMode(.inline)
        .onDisappear { player.stop() }
    }

    private func row(_ option: AgentVoiceOption) -> some View {
        HStack(spacing: 12) {
            Button {
                voice = option.id == defaultVoice ? nil : option.id
            } label: {
                HStack(spacing: 12) {
                    Image(systemName: selected == option.id ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 22))
                        .foregroundStyle(selected == option.id ? Theme.ink : Theme.muted)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(option.name).font(Theme.body(16, weight: .heavy))
                            if option.id == defaultVoice {
                                Text("par défaut").font(Theme.body(11, weight: .bold)).foregroundStyle(Theme.muted)
                            }
                        }
                        Text(option.character).font(Theme.body(13)).foregroundStyle(Theme.ink2)
                    }
                    Spacer(minLength: 0)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if let url = option.sampleURL(in: language) {
                Button {
                    player.toggle(url)
                } label: {
                    Image(systemName: player.playingURL == url ? "stop.fill" : "play.fill")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 34, height: 34)
                        .background(Theme.ink, in: .circle)
                }
                .buttonStyle(.plain)
                .accessibilityLabel(player.playingURL == url ? String(localized: "Arrêter l’aperçu") : String(localized: "Écouter \(option.name)"))
            }
        }
        .padding(.vertical, 4)
    }
}
