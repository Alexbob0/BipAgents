import HermesKit
import SwiftUI

struct SettingsView: View {
    @Environment(AgentStore.self) private var store
    @State private var isAddingAgent = false
    @State private var editing: AgentProfile?

    var body: some View {
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
        .navigationTitle("Réglages")
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
