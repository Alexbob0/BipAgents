import HermesKit
import SwiftUI
import VoiceKit

/// A voice an agent can speak with. `id` is the choice stored with the agent: `pocket:<name>` for the Bips (Kyutai
/// Pocket TTS, `voices/<language>/`), a Kyutai alias otherwise. The bridge receives it per language
/// (`AgentVoices.spoken`). The voices of the person's own voice server are not here: the bridge lists them.
struct AgentVoiceOption: Identifiable, Hashable {
    let id: String
    let name: String
    let character: String
    /// Bundled preview (`App/Resources/Voices/<sample>.m4a`, `<sample>-<lang>.m4a` outside French), if any.
    let sample: String?

    func sampleURL(in language: AgentLanguage) -> URL? {
        sample.flatMap {
            Bundle.main.url(forResource: language == .french ? $0 : "\($0)-\(language.rawValue)", withExtension: "m4a")
        }
    }
}

/// What a voice speaks for: the Live, the spoken replies (to voice notes, « Écouter »), the reading of messages and
/// reports (by the bridge). The person's voice server says which ones it takes (`[voice_server]`).
enum VoiceUse: String, CaseIterable, Identifiable {
    case live, replies, reading
    var id: String { rawValue }

    var label: String {
        switch self {
        case .live: String(localized: "Live")
        case .replies: String(localized: "Réponses vocales")
        case .reading: String(localized: "Lecture des messages")
        }
    }
}

/// The person's own voice model, as the agent's bridge last described it (`GET /v1/voices`).
struct VoiceServerInfo: Codable, Hashable {
    /// Its voice ids, in its order: the app shows « Voix 1, Voix 2… ».
    var voices: [String]
    var uses: [String: Bool]

    func takes(_ use: VoiceUse) -> Bool { uses[use.rawValue] ?? true }

    /// The voices for an agent speaking `language`: ids named after a language (`fr-…`, `en_…`) only in theirs.
    func voices(in language: AgentLanguage) -> [String] {
        let codes = AgentLanguage.allCases.map(\.rawValue)
        return voices.filter { id in
            let prefix = id.prefix(3).lowercased()
            guard prefix.count == 3, ["-", "_"].contains(prefix.last.map(String.init) ?? ""),
                  codes.contains(String(prefix.prefix(2))) else { return true }
            return prefix.hasPrefix(language.rawValue)
        }
    }

    /// « Voix 3 »: its place among the agent's voices.
    func name(of id: String, in language: AgentLanguage) -> String {
        let index = voices(in: language).firstIndex(of: id).map { $0 + 1 }
        return index.map { String(localized: "Voix \($0)") } ?? String(localized: "Voix du serveur")
    }
}

enum AgentVoices {
    /// The Drama 3 voices an earlier app stored as an agent's voice: now the voice server's.
    static let dramaPrefix = "drama:"

    /// Each agent's voice server, by agent id (kept for when the bridge can't be asked).
    private static let serversKey = "voiceServers"
    static func server(for agent: AgentProfile) -> VoiceServerInfo? {
        #if DEBUG
        if UserDefaults.standard.bool(forKey: "demoVoiceServer") {   // design review: `-demoVoiceServer YES`
            return VoiceServerInfo(voices: ["fr-naturelle-s15", "fr-naturelle-s32", "fr-mascotte-s17", "en-naturelle-s47"],
                                   uses: ["live": true, "replies": true, "reading": false])
        }
        #endif
        guard let data = UserDefaults.standard.dictionary(forKey: serversKey)?[agent.id.uuidString] as? Data else { return nil }
        return try? JSONDecoder().decode(VoiceServerInfo.self, from: data)
    }
    static func setServer(_ info: VoiceServerInfo?, for agent: AgentProfile) {
        var all = UserDefaults.standard.dictionary(forKey: serversKey) ?? [:]
        all[agent.id.uuidString] = info.flatMap { try? JSONEncoder().encode($0) }
        UserDefaults.standard.set(all, forKey: serversKey)
    }

    /// A bundled preview of a server voice, when it is one of the Bips' Drama 3 voices (`drama-<id>.m4a`).
    static func bundledSample(ofServerVoice id: String) -> URL? {
        Bundle.main.url(forResource: "drama-\(id)", withExtension: "m4a")
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

    static var all: [AgentVoiceOption] { bips + [classic, iphone] }

    /// Pocket's Bips speak every language; Kyutai's human voice is French.
    static func options(in language: AgentLanguage) -> [AgentVoiceOption] {
        language == .french ? bips + [classic, iphone] : bips + [iphone]
    }

    /// What the bridge receives: `pocket:galet` in French, `pocket:en/galet` in English…
    static func spoken(_ id: String, in language: AgentLanguage) -> String {
        guard language != .french, id.hasPrefix("pocket:") else { return id }
        return "pocket:\(language.rawValue)/" + id.dropFirst("pocket:".count)
    }

    static func option(for id: String) -> AgentVoiceOption {
        all.first { $0.id == id } ?? AgentVoiceOption(id: id, name: id, character: String(localized: "Voix personnalisée"), sample: nil)
    }
}

extension AgentCategory {
    /// The Pocket Bip an agent of this category gets until one is picked.
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
    /// The Bip (or Kyutai, or iPhone) voice picked for this agent in its language, else its category's Bip.
    var voiceChoice: String {
        guard let picked = config.voice,
              AgentVoices.options(in: language).contains(where: { $0.id == picked })
                || (language == .french && !picked.hasPrefix(AgentVoices.dramaPrefix))
        else { return appearance.category.defaultVoice }
        return picked
    }

    /// Its voice on the person's voice server (the id the server knows), if one was picked; an earlier app's Drama 3
    /// voice counts as one.
    var serverVoiceChoice: String? {
        if let picked = config.serverVoice { return picked }
        guard let old = config.voice, old.hasPrefix(AgentVoices.dramaPrefix) else { return nil }
        return String(old.dropFirst(AgentVoices.dramaPrefix.count))
    }

    /// The Bip voice sent to the bridge.
    var voice: String { AgentVoices.spoken(voiceChoice, in: language) }

    /// The voice for `use`: its server voice when the server takes that use, else its Bip.
    func voice(for use: VoiceUse) -> String {
        guard let server = AgentVoices.server(for: self), server.takes(use), let id = serverVoiceChoice else { return voice }
        return "server:\(language.rawValue)/\(id)"
    }

    /// Speaks with the iPhone's own voice (picked in its settings) for `use`, never through the bridge.
    func usesSystemVoice(for use: VoiceUse) -> Bool { voice(for: use) == AgentVoices.iphone.id }
}

/// Voice choice for one agent: its voice on the person's voice server (« Voix 1, Voix 2… »), and its Bip, with a
/// preview of each.
struct AgentVoicePicker: View {
    @Binding var voice: String?
    @Binding var serverVoice: String?
    var category: AgentCategory
    var language: AgentLanguage = .french
    /// The agent's voice server, if any.
    var server: VoiceServerInfo?
    /// A server voice's preview, synthesized by the bridge.
    var preview: ((String) async throws -> URL)?
    @State private var player = VoiceNotePlayer()
    @State private var previews: [String: URL] = [:]
    @State private var loading: String?

    private var defaultVoice: String { category.defaultVoice }

    private var selected: String {
        guard let voice, AgentVoices.options(in: language).contains(where: { $0.id == voice }) else { return defaultVoice }
        return voice
    }

    var body: some View {
        List {
            if let server, !server.voices(in: language).isEmpty {
                Section {
                    serverRow(nil, server: server)
                    ForEach(server.voices(in: language), id: \.self) { serverRow($0, server: server) }
                } header: {
                    Text("Ton modèle de voix")
                } footer: {
                    Text(serverFooter(server))
                }
            }
            Section {
                ForEach(AgentVoices.bips) { row($0) }
            } header: {
                Text("Voix de Bip")
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

    private func serverFooter(_ server: VoiceServerInfo) -> String {
        let uses = VoiceUse.allCases.filter(server.takes).map(\.label)
        guard !uses.isEmpty else {
            return String(localized: "Ton serveur ne parle pour aucun usage pour l’instant : choisis-les dans Réglages › Voix.")
        }
        return String(localized: "Elle parle pour : \(uses.formatted(.list(type: .and))). Le Bip parle pour le reste, et quand le serveur ne répond pas. Les usages se choisissent dans Réglages › Voix.")
    }

    private func serverRow(_ id: String?, server: VoiceServerInfo) -> some View {
        HStack(spacing: 12) {
            Button {
                serverVoice = id
            } label: {
                HStack(spacing: 12) {
                    Image(systemName: serverVoice == id ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 22))
                        .foregroundStyle(serverVoice == id ? Theme.ink : Theme.muted)
                    Text(id.map { server.name(of: $0, in: language) } ?? String(localized: "Aucune : toujours le Bip"))
                        .font(Theme.body(16, weight: .heavy))
                    Spacer(minLength: 0)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if let id, AgentVoices.bundledSample(ofServerVoice: id) != nil || preview != nil {
                previewButton(id: id, name: server.name(of: id, in: language)) {
                    if let bundled = AgentVoices.bundledSample(ofServerVoice: id) { return bundled }
                    if let ready = previews[id] { return ready }
                    guard let preview else { return nil }
                    let url = try await preview(id)
                    previews[id] = url
                    return url
                }
            }
        }
        .padding(.vertical, 4)
    }

    private func previewButton(id: String, name: String, url: @escaping () async throws -> URL?) -> some View {
        let playing = previews[id].map { player.playingURL == $0 } ?? false
            || AgentVoices.bundledSample(ofServerVoice: id).map { player.playingURL == $0 } ?? false
        return Button {
            if playing { player.stop(); return }
            loading = id
            Task {
                defer { loading = nil }
                if let found = try? await url() { player.toggle(found) }
            }
        } label: {
            Group {
                if loading == id {
                    ProgressView().tint(.white)
                } else {
                    Image(systemName: playing ? "stop.fill" : "play.fill").font(.system(size: 13, weight: .bold))
                }
            }
            .foregroundStyle(.white)
            .frame(width: 34, height: 34)
            .background(Theme.ink, in: .circle)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(playing ? String(localized: "Arrêter l’aperçu") : String(localized: "Écouter \(name)"))
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
