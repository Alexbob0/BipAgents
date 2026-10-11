import HermesKit
import SwiftUI

/// « Qui peut lire quoi » : agent by agent, whose shared folder it may read. Read-only: the reader sees what the
/// other agent puts in its `/partage` folder, never its memory, conversations, keys or other files.
struct AgentSharesView: View {
    @Environment(AgentStore.self) private var store
    @State private var bridge: BridgeClient?
    @State private var agents: [BridgeClient.ShareAgent] = []
    @State private var reads: Set<BridgeClient.ShareRead> = []
    @State private var isLoading = true
    @State private var isSaving = false
    @State private var errorMessage: String?

    private var able: [BridgeClient.ShareAgent] { agents.filter { $0.whyNot == nil } }
    private var unable: [BridgeClient.ShareAgent] { agents.filter { $0.whyNot != nil } }

    var body: some View {
        List {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            if !isLoading && able.count < 2 && errorMessage == nil {
                Section {
                    NoticeRow(text: String(localized: "Il faut au moins deux agents dont les outils tournent dans un bac à sable pour partager des données."), isError: false)
                }
                .listRowBackground(Color.clear)
            }
            if able.count >= 2 {
                ForEach(able) { reader in
                    Section {
                        ForEach(able.filter { $0.id != reader.id }) { source in
                            Toggle(isOn: binding(reader.id, source.id)) {
                                Text("Peut lire ce que partage \(source.name)")
                            }
                            .disabled(isSaving)
                        }
                    } header: {
                        Text(reader.name)
                    }
                }
            }
            if !unable.isEmpty {
                Section {
                    ForEach(unable) { agent in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(agent.name).font(Theme.body(15, weight: .heavy))
                            Text(agent.whyNot == "no_sandbox"
                                 ? String(localized: "Ses outils tournent sur le serveur lui-même, sans bac à sable : pas de partage.")
                                 : String(localized: "Installé à la main : son administrateur partage ses dossiers sur le serveur."))
                                .font(Theme.body(13)).foregroundStyle(Theme.ink2)
                        }
                    }
                } header: {
                    Text("Sans partage")
                }
            }
            Section {
                Label("Un agent met dans son dossier « partage » ce que les autres peuvent lire. Ceux que tu coches ici le voient, en lecture seule : ils ne peuvent rien y changer, et ne voient rien d’autre (ni sa mémoire, ni ses conversations, ni ses clés).",
                      systemImage: "folder.badge.person.crop")
                    .font(Theme.body(13, weight: .regular)).foregroundStyle(Theme.ink2)
            }
            .listRowBackground(Color.clear)
        }
        .navigationTitle("Qui peut lire quoi")
        .overlay { if isLoading { ProgressView() } }
        .refreshable { await load() }
        .task { await load() }
    }

    private func binding(_ reader: String, _ source: String) -> Binding<Bool> {
        let read = BridgeClient.ShareRead(reader: reader, source: source)
        return Binding(get: { reads.contains(read) }, set: { on in
            var next = reads
            if on { next.insert(read) } else { next.remove(read) }
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
                guard let found = try await withTimeout(10, { try await candidate.agentShares() }) else { continue }
                bridge = candidate
                agents = found.agents
                reads = found.reads
                errorMessage = nil
                return
            } catch {
                lastError = error
            }
        }
        errorMessage = lastError.map { ConversationModel.describe($0) }
            ?? String(localized: "Aucun de tes serveurs ne partage encore de données entre agents (bridge à mettre à jour).")
    }

    private func save(_ next: Set<BridgeClient.ShareRead>) async {
        guard let bridge else { return }
        let before = reads
        reads = next
        isSaving = true
        defer { isSaving = false }
        do {
            reads = try await bridge.setAgentShares(next)
            errorMessage = nil
        } catch {
            reads = before
            errorMessage = ConversationModel.describe(error)
        }
    }
}
