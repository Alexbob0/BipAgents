import HermesKit
import SwiftUI

/// Réglages › Voix › Ton modèle de voix: what the person's own voice server speaks for (the Live, the spoken replies,
/// the reading of messages), on each install that has one. The Bips speak for the rest.
struct VoiceServerView: View {
    @Environment(AgentStore.self) private var store
    @State private var errorMessage: String?
    /// Bumped once the bridge said yes: the switches read the saved setting again.
    @State private var revision = 0

    /// One agent per bridge with a voice server: the setting is the bridge's.
    private var installs: [AgentProfile] {
        var seen = Set<URL?>()
        return store.agents.filter { AgentVoices.server(for: $0) != nil && seen.insert($0.config.bridgeURL).inserted }
    }

    var body: some View {
        List {
            ForEach(installs) { agent in
                Section {
                    ForEach(VoiceUse.allCases) { use in
                        Toggle(isOn: binding(use, of: agent)) { Text(use.label) }
                    }
                } header: {
                    if installs.count > 1 { Text(agent.config.bridgeURL?.host() ?? agent.name) }
                } footer: {
                    if agent.id == installs.last?.id {
                        Text("Activé : la voix choisie sur ton serveur pour chaque agent (Réglages › Agents › Voix). Désactivé : son Bip, qui répond presque instantanément. Si le serveur ne répond pas, le Bip prend le relais.")
                    }
                }
            }
            if let errorMessage {
                Text(errorMessage).foregroundStyle(.red)
            }
        }
        .navigationTitle("Ton modèle de voix")
        .navigationBarTitleDisplayMode(.inline)
    }

    private func binding(_ use: VoiceUse, of agent: AgentProfile) -> Binding<Bool> {
        Binding {
            _ = revision
            return AgentVoices.server(for: agent)?.takes(use) ?? false
        } set: { on in
            Task {
                do {
                    try await store.setVoiceUses([use.rawValue: on], via: agent)
                    errorMessage = nil
                    revision += 1
                } catch {
                    errorMessage = String(localized: "Le réglage n’a pas pu être enregistré sur le serveur. Réessaie dans un instant.")
                }
            }
        }
    }
}
