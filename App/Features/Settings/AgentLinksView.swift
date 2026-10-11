import HermesKit
import SwiftUI

/// « Qui peut écrire à qui » : the person turns on, agent by agent, which others it may message. The bridge
/// carries those messages and refuses every other one.
struct AgentLinksView: View {
    @Environment(AgentStore.self) private var store
    @State private var bridge: BridgeClient?
    @State private var agents: [BridgeClient.LinkAgent] = []
    @State private var links: Set<BridgeClient.AgentLink> = []
    @State private var needsSetup: [String] = []
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var errorMessage: String?

    var body: some View {
        List {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            // Ours first, then the agents of linked households: one of theirs may only write to ours.
            ForEach(agents.filter { $0.household == nil } + agents.filter { $0.household != nil }) { sender in
                Section {
                    ForEach(agents.filter { $0.id != sender.id && (sender.household == nil || $0.household == nil) }) { target in
                        Toggle(isOn: binding(sender.id, target.id)) {
                            Text("Peut écrire à \(target.name)")
                        }
                        .disabled(isSaving)
                    }
                } header: {
                    Text(sender.name)
                } footer: {
                    if needsSetup.contains(sender.id) {
                        Text("\(sender.name) a été installé à la main : sur le serveur, lance « python -m bipbridge peer-token --agent \(sender.id) » et ajoute ce qu’il affiche à sa configuration.")
                    }
                }
            }
            Section {
                Label("Un agent ne peut écrire qu’aux agents que tu as cochés ici. Il ne reçoit jamais leurs clés : c’est ton serveur qui transmet le message, et il refuse tout le reste. Les agents qui partagent l’espace commun peuvent aussi s’écrire entre eux. Pour les agents d’un proche (Réglages › Famille), il faut que vous l’ayez coché tous les deux, chacun chez soi.",
                      systemImage: "arrow.left.arrow.right")
                    .font(Theme.body(13, weight: .regular)).foregroundStyle(Theme.ink2)
            }
            .listRowBackground(Color.clear)
        }
        .navigationTitle("Qui peut écrire à qui")
        .overlay { if isLoading { ProgressView() } }
        .refreshable { await load() }
        .task { await load() }
    }

    private func binding(_ from: String, _ to: String) -> Binding<Bool> {
        let link = BridgeClient.AgentLink(from: from, to: to)
        return Binding(get: { links.contains(link) }, set: { on in
            var next = links
            if on { next.insert(link) } else { next.remove(link) }
            Task { await save(next) }
        })
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        var lastError: (any Error)?
        for agent in store.agents {
            guard let candidate = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            do {
                guard let found = try await withTimeout(10, { try await candidate.agentLinks() }) else { continue }
                bridge = candidate
                agents = found.agents
                links = found.links
                errorMessage = nil
                return
            } catch {
                lastError = error   // unreachable right now: say so, not « too old »
            }
        }
        errorMessage = lastError.map { ConversationModel.describe($0) }
            ?? String(localized: "Aucun de tes serveurs ne transmet encore les messages entre agents (bridge à mettre à jour).")
    }

    private func save(_ next: Set<BridgeClient.AgentLink>) async {
        guard let bridge else { return }
        let before = links
        links = next
        isSaving = true
        defer { isSaving = false }
        do {
            needsSetup = try await bridge.setAgentLinks(next)
            errorMessage = nil
        } catch {
            links = before
            errorMessage = ConversationModel.describe(error)
        }
    }
}
