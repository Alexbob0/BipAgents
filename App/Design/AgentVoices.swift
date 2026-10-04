import HermesKit
import SwiftUI
import VoiceKit

/// A voice an agent can speak with. `id` is what the bridge receives: `pocket:<name>` for the Bips' voices
/// (Kyutai Pocket TTS, `voices/french/` in the repo), a Kyutai alias otherwise.
struct AgentVoiceOption: Identifiable, Hashable {
    let id: String
    let name: String
    let character: String
    /// Bundled preview (`App/Resources/Voices`), if any.
    let sample: String?

    var sampleURL: URL? { sample.flatMap { Bundle.main.url(forResource: $0, withExtension: "m4a") } }
}

enum AgentVoices {
    static let bips: [AgentVoiceOption] = [
        AgentVoiceOption(id: "pocket:loutre", name: "Loutre", character: "Joueuse et complice, chaude et ronde", sample: "voice-loutre"),
        AgentVoiceOption(id: "pocket:colibri", name: "Colibri", character: "Vive et enjouée, claire et chantante", sample: "voice-colibri"),
        AgentVoiceOption(id: "pocket:lutin", name: "Lutin", character: "Farceur et vif, rieur et expressif", sample: "voice-lutin"),
        AgentVoiceOption(id: "pocket:chat2", name: "Chat", character: "Malicieux, ronronnant et amusé", sample: "voice-chat2"),
        AgentVoiceOption(id: "pocket:ours", name: "Ours", character: "Calme et rassurant, grave et doux", sample: "voice-ours"),
    ]
    /// Kyutai 1.6B's human voice, the one of the morning podcast.
    static let classic = AgentVoiceOption(id: "5476", name: "Voix classique", character: "Voix humaine posée, celle du podcast", sample: nil)

    static var all: [AgentVoiceOption] { bips + [classic] }

    static func option(for id: String) -> AgentVoiceOption {
        all.first { $0.id == id } ?? AgentVoiceOption(id: id, name: id, character: "Voix personnalisée", sample: nil)
    }
}

extension AgentCategory {
    /// The Bip voice an agent of this category gets until one is picked.
    var defaultVoice: String {
        switch self {
        case .wellness: "pocket:loutre"
        case .daily: "pocket:colibri"
        case .creative, .learning: "pocket:lutin"
        case .work, .tech: "pocket:chat2"
        case .finance, .home: "pocket:ours"
        }
    }
}

extension AgentProfile {
    /// The voice sent to the bridge: the one picked, else the category's Bip voice.
    var voice: String { config.voice ?? appearance.category.defaultVoice }
}

/// Voice choice for one agent, with a preview of each Bip voice.
struct AgentVoicePicker: View {
    @Binding var voice: String?
    var category: AgentCategory
    @State private var player = VoiceNotePlayer()

    private var selected: String { voice ?? category.defaultVoice }

    var body: some View {
        List {
            Section {
                ForEach(AgentVoices.bips) { row($0) }
            } header: {
                Text("Voix de Bip")
            } footer: {
                Text("Voix synthétiques calculées avec Kyutai Pocket TTS : la réponse démarre presque instantanément.")
            }
            Section {
                row(AgentVoices.classic)
            } footer: {
                Text("La voix humaine de Kyutai, plus posée mais plus lente à démarrer.")
            }
        }
        .navigationTitle("Voix")
        .navigationBarTitleDisplayMode(.inline)
        .onDisappear { player.stop() }
    }

    private func row(_ option: AgentVoiceOption) -> some View {
        HStack(spacing: 12) {
            Button {
                voice = option.id == category.defaultVoice ? nil : option.id
            } label: {
                HStack(spacing: 12) {
                    Image(systemName: selected == option.id ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 22))
                        .foregroundStyle(selected == option.id ? Theme.ink : Theme.muted)
                    VStack(alignment: .leading, spacing: 2) {
                        HStack(spacing: 6) {
                            Text(option.name).font(Theme.body(16, weight: .heavy))
                            if option.id == category.defaultVoice {
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
            if let url = option.sampleURL {
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
                .accessibilityLabel(player.playingURL == url ? "Arrêter l’aperçu" : "Écouter \(option.name)")
            }
        }
        .padding(.vertical, 4)
    }
}
