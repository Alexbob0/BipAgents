import HermesKit
import SwiftUI

struct SettingsView: View {
    @Environment(AgentStore.self) private var store
    @State private var isAddingAgent = false
    @AppStorage(VoiceReplyPolicy.storageKey) private var voiceReplyPolicy = VoiceReplyPolicy.smart
    @AppStorage(BipBabble.enabledKey) private var bipSounds = true
    @AppStorage(AppStyle.key) private var style: AppStyle = .bips
    @AppStorage(ColorMode.key) private var colorMode: ColorMode = .system
    @AppStorage(AgentWork.key) private var showsWorkDetail = true

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "–"
    }

    var body: some View {
        VStack(spacing: 0) {
            ScreenHeader(overline: "BipAgents \(version)", title: "Réglages")
            List {
                Section("Agents") {
                    ForEach(store.agents) { agent in
                        NavigationLink { AgentOptionsView(agent: agent) } label: {
                            HStack(spacing: 12) {
                                MascotAvatar(appearance: agent.appearance, size: 40)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(agent.name).font(Theme.body(16, weight: .heavy))
                                    Text(agent.config.baseURL.host() ?? agent.config.baseURL.absoluteString)
                                        .font(Theme.mono)
                                        .foregroundStyle(Theme.muted)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                }
                                Spacer()
                                ReachabilityLabel(reachability: store.reachability[agent.id] ?? .unknown)
                            }
                        }
                        .foregroundStyle(Theme.ink)
                        .swipeActions {
                            Button("Supprimer", systemImage: "trash", role: .destructive) { store.remove(agent) }
                        }
                    }
                    .onMove(perform: store.move)
                    Button("Ajouter un agent", systemImage: "plus") { isAddingAgent = true }
                }
                Section {
                    NavigationLink { AgentLinksView() } label: {
                        Label("Qui peut écrire à qui", systemImage: "arrow.left.arrow.right")
                    }
                    NavigationLink { AgentSharesView() } label: {
                        Label("Qui peut lire quoi", systemImage: "folder.badge.person.crop")
                    }
                    NavigationLink { FamilyView() } label: {
                        Label("Famille", systemImage: "house.and.flag")
                    }
                } header: {
                    Text("Entre agents")
                } footer: {
                    Text("Les messages et les données que tes agents s’échangent, et les agents de tes proches.")
                }
                Section {
                    NavigationLink { VaultView() } label: {
                        Label("Coffre des agents", systemImage: "lock.shield")
                    }
                } header: {
                    Text("Sécurité")
                } footer: {
                    Text("Les identifiants de sites que tes agents utilisent sans jamais les voir.")
                }
                Section("Automatismes") {
                    NavigationLink { CronJobsView() } label: {
                        Label("Tâches planifiées", systemImage: "clock.badge")
                    }
                }
                Section {
                    Picker(selection: $voiceReplyPolicy) {
                        ForEach(VoiceReplyPolicy.allCases) { Text($0.label).tag($0) }
                    } label: {
                        Label("Réponse vocale", systemImage: "waveform")
                    }
                    if store.agents.contains(where: { AgentVoices.server(for: $0) != nil }) {
                        NavigationLink { VoiceServerView() } label: {
                            Label("Ton modèle de voix", systemImage: "server.rack")
                        }
                    }
                } header: {
                    Text("Voix")
                } footer: {
                    Text("« Intelligent » : en vocal si tu as des écouteurs, es en voiture ou le demandes. « Écouter » sous une réponse la lit à la demande, « Live » pour parler en direct.")
                }
                Section {
                    Toggle(isOn: $showsWorkDetail) {
                        Label("Détail du travail", systemImage: "wrench.and.screwdriver")
                    }
                } header: {
                    Text("Discussions")
                } footer: {
                    Text("Sous chaque réponse, un pli qui montre ce que l’agent a fait : ses recherches, ses commandes, ses messages en chemin.")
                }
                Section {
                    Picker(selection: $style) {
                        Text("Bips").tag(AppStyle.bips)
                        Text("Hermes").tag(AppStyle.hermes)
                    } label: {
                        Label("Style", systemImage: "paintpalette")
                    }
                    Picker(selection: $colorMode) {
                        Text("Auto").tag(ColorMode.system)
                        Text("Clair").tag(ColorMode.light)
                        Text("Sombre").tag(ColorMode.dark)
                    } label: {
                        Label("Mode", systemImage: "circle.lefthalf.filled")
                    }
                    if style == .bips {
                        Toggle(isOn: $bipSounds) {
                            Label("Sons des Bips", systemImage: "speaker.wave.2.bubble")
                        }
                    }
                } header: {
                    Text("Apparence")
                } footer: {
                    style == .bips
                        ? Text("Les Bips babillent quand tu joues avec eux, sauf en mode silencieux. « Auto » suit le réglage de l’iPhone.")
                        : Text("Hermes : plus proche du design de Hermes Agent, avec des personnages en pixels. « Auto » suit le réglage de l’iPhone.")
                }
                Section("Application") {
                    Button { UIApplication.shared.open(URL(string: UIApplication.openSettingsURLString)!) } label: {
                        Label("Micro, notifications et Face ID", systemImage: "gear")
                    }
                    .foregroundStyle(Theme.ink) // a row like the others, not an accent-colored button
                    NavigationLink { DiagnosticsView() } label: {
                        Label("Diagnostics et latences", systemImage: "gauge.with.dots.needle.33percent")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .contentMargins(.top, 4, for: .scrollContent)
        }
        .background(Theme.background)
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $isAddingAgent) { AddAgentFlow() }
    }
}

struct EditAgentStyleView: View {
    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var agent: AgentProfile
    @State private var picked = true

    init(agent: AgentProfile) {
        _agent = State(initialValue: agent)
    }

    var body: some View {
        NavigationStack {
            AgentStylePicker(name: $agent.config.name, appearance: $agent.appearance, categoryWasPicked: $picked)
                .background(Theme.background)
                .safeAreaInset(edge: .bottom) {
                    VStack(spacing: 8) {
                        Menu {
                            Picker("Langue", selection: language) {
                                ForEach(AgentLanguage.allCases) { Text($0.nativeName).tag($0) }
                            }
                        } label: {
                            settingsRow("Langue", systemImage: "globe", value: agent.language.nativeName)
                        }
                        NavigationLink {
                            AgentVoicePicker(voice: $agent.config.voice, serverVoice: $agent.config.serverVoice,
                                             category: agent.appearance.category, language: agent.language,
                                             server: AgentVoices.server(for: agent), preview: previewServerVoice)
                        } label: {
                            settingsRow("Voix", systemImage: "waveform", value: voiceSummary)
                        }
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal, 16)
                    .padding(.bottom, 8)
                }
                .navigationTitle("Modifier")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Annuler") { dismiss() } }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("OK") {
                            store.update(agent)
                            dismiss()
                        }
                        .fontWeight(.heavy)
                    }
                }
        }
    }

    private var language: Binding<AgentLanguage> {
        Binding { agent.language } set: { agent.config.language = $0.rawValue }
    }

    /// « Voix 2 » (its voice model's) or « Galet »: the voice checked for it.
    private var voiceSummary: String {
        guard let server = AgentVoices.server(for: agent), let id = agent.serverVoiceChoice else {
            return AgentVoices.option(for: agent.voiceChoice).name
        }
        return server.name(of: id, in: agent.language)
    }

    /// The bridge reads a short sentence with a voice of the voice server (cached on the phone).
    private func previewServerVoice(_ id: String) async throws -> URL {
        guard let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { throw HermesError.unsupported("bridge") }
        let text = switch agent.language {
        case .english: "Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon."
        case .spanish: "¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde."
        case .german: "Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag."
        default: "Coucou ! J’ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi."
        }
        let data = try await bridge.voicePreview(text: text, agent: agent.bridgeName, voice: "server:\(agent.language.rawValue)/\(id)")
        let url = VoiceNotePlayer.cacheURL(name: "server-voice-\(agent.language.rawValue)-\(id.replacingOccurrences(of: "/", with: "_")).wav")
        try data.write(to: url)
        return url
    }

    private func settingsRow(_ title: LocalizedStringKey, systemImage: String, value: String, chevron: Bool = true) -> some View {
        HStack {
            Label(title, systemImage: systemImage)
                .font(Theme.body(16, weight: .heavy))
            Spacer()
            Text(value)
                .font(Theme.body(15, weight: .bold))
                .foregroundStyle(Theme.ink2)
            if chevron {
                Image(systemName: "chevron.right")
                    .font(.system(size: 13, weight: .bold))
                    .foregroundStyle(Theme.muted)
            }
        }
        .foregroundStyle(Theme.ink)
        .padding(.horizontal, 18)
        .frame(height: 54)
        .background(Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
    }
}
