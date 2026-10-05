import HermesKit
import SwiftUI

struct SettingsView: View {
    @Environment(AgentStore.self) private var store
    @State private var isAddingAgent = false
    @State private var editing: AgentProfile?
    @AppStorage(VoiceReplyPolicy.storageKey) private var voiceReplyPolicy = VoiceReplyPolicy.smart
    @AppStorage(BipBabble.enabledKey) private var bipSounds = true

    private var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "–"
    }

    var body: some View {
        VStack(spacing: 0) {
            ScreenHeader(overline: "BipAgents \(version)", title: "Réglages")
            List {
                Section("Agents") {
                    ForEach(store.agents) { agent in
                        Button { editing = agent } label: {
                            HStack(spacing: 12) {
                                MascotAvatar(appearance: agent.appearance, size: 40)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(agent.name).font(Theme.body(16, weight: .heavy))
                                    Text(agent.config.baseURL.host() ?? agent.config.baseURL.absoluteString)
                                        .font(Theme.mono)
                                        .foregroundStyle(Theme.muted)
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
                    Picker(selection: $voiceReplyPolicy) {
                        ForEach(VoiceReplyPolicy.allCases) { Text($0.label).tag($0) }
                    } label: {
                        Label("Réponse vocale à mes vocaux", systemImage: "waveform")
                    }
                    Toggle(isOn: $bipSounds) {
                        Label("Sons des Bips", systemImage: "speaker.wave.2.bubble")
                    }
                    NavigationLink { CronJobsView() } label: {
                        Label("Tâches planifiées", systemImage: "clock.badge")
                    }
                } header: {
                    Text("Voix")
                } footer: {
                    Text("« Intelligent » : l’agent répond en vocal à un message vocal si tu as des écouteurs ou es en voiture, ou si tu le demandes (« réponds-moi en vocal »). « Écouter » sous chaque réponse génère le vocal à la demande. Pour une conversation en direct, utilise « Live ». Les Bips babillent quand tu joues avec eux (sauf en mode silencieux).")
                }
                Section("Application") {
                    NavigationLink { DiagnosticsView() } label: {
                        Label("Diagnostics et latences", systemImage: "gauge.with.dots.needle.33percent")
                    }
                    Button { UIApplication.shared.open(URL(string: UIApplication.openSettingsURLString)!) } label: {
                        Label("Micro, notifications et Face ID", systemImage: "gear")
                    }
                    LabeledContent("Version", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "–")
                }
                .foregroundStyle(Theme.ink)
            }
            .scrollContentBackground(.hidden)
            .contentMargins(.top, 4, for: .scrollContent)
        }
        .background(Theme.background)
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $isAddingAgent) { AddAgentFlow() }
        .sheet(item: $editing) { agent in EditAgentStyleView(agent: agent) }
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
                    NavigationLink {
                        AgentVoicePicker(voice: $agent.config.voice, category: agent.appearance.category)
                    } label: {
                        HStack {
                            Label("Voix", systemImage: "waveform")
                                .font(Theme.body(16, weight: .heavy))
                            Spacer()
                            Text(AgentVoices.option(for: agent.voice).name)
                                .font(Theme.body(15, weight: .bold))
                                .foregroundStyle(Theme.ink2)
                            Image(systemName: "chevron.right")
                                .font(.system(size: 13, weight: .bold))
                                .foregroundStyle(Theme.muted)
                        }
                        .foregroundStyle(Theme.ink)
                        .padding(.horizontal, 18)
                        .frame(height: 54)
                        .background(Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
                        .padding(.horizontal, 16)
                        .padding(.bottom, 8)
                    }
                    .buttonStyle(.plain)
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
}
