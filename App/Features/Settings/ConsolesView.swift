import HermesKit
import SwiftUI

/// « Navigateurs » : the web console (the agents from a computer's browser, `/console/` on the bridge). The person
/// lets a browser in with the code it shows, sees the browsers logged in and logs any of them out.
struct ConsolesView: View {
    @Environment(AgentStore.self) private var store
    @State private var code = ""
    @State private var name = String(localized: "Ordinateur")
    @State private var devices: [Bridge] = []
    @State private var isLoading = true
    @State private var isApproving = false
    @State private var notice: (text: String, isError: Bool)?

    struct Bridge: Identifiable {
        var client: BridgeClient
        var devices: [BridgeClient.ConsoleDevice]
        var id: URL { client.baseURL }
    }

    /// Each bridge once (several agents share one).
    private var bridges: [BridgeClient] {
        var seen = Set<URL>()
        return store.agents.compactMap { agent in
            guard let client = BridgeClient(agent: agent, secrets: store.secrets(for: agent)),
                  seen.insert(client.baseURL).inserted else { return nil }
            return client
        }
    }

    /// The console's address on the tailnet, to type on the computer.
    private var consoleAddresses: [String] {
        var seen = Set<String>()
        return store.agents.compactMap { $0.config.bridgeURL?.appending(path: "console/").absoluteString }
            .filter { seen.insert($0).inserted }
    }

    var body: some View {
        List {
            Section {
                TextField("Code à 6 chiffres", text: $code)
                    .keyboardType(.numberPad)
                    .font(Theme.body(22, weight: .heavy).monospacedDigit())
                    .onChange(of: code) { _, new in code = String(new.filter(\.isNumber).prefix(6)) }
                TextField("Nom de cet ordinateur", text: $name)
                Button {
                    Task { await approve() }
                } label: {
                    if isApproving { ProgressView() } else { Text("Approuver ce navigateur") }
                }
                .disabled(code.count != 6 || isApproving)
            } header: {
                Text("Approuver un navigateur")
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Sur l’ordinateur (connecté à ton Tailscale), ouvre l’adresse ci-dessous et clique « Afficher un code ». Le navigateur reste connecté 30 jours.")
                    ForEach(consoleAddresses, id: \.self) { address in
                        Text(address).font(Theme.mono).textSelection(.enabled)
                    }
                }
            }
            if let notice {
                Section { NoticeRow(text: notice.text, isError: notice.isError) }.listRowBackground(Color.clear)
            }
            ForEach(devices) { bridge in
                Section {
                    if bridge.devices.isEmpty {
                        Text("Aucun navigateur connecté.").foregroundStyle(Theme.muted)
                    }
                    ForEach(bridge.devices) { device in
                        VStack(alignment: .leading, spacing: 2) {
                            Text(device.name).font(Theme.body(16, weight: .bold))
                            if let seen = device.seen {
                                Text("Utilisé \(seen, format: .relative(presentation: .named))")
                                    .font(Theme.body(13, weight: .regular)).foregroundStyle(Theme.muted)
                            }
                        }
                        .swipeActions {
                            Button("Déconnecter", systemImage: "xmark.circle", role: .destructive) {
                                Task { await revoke(device, on: bridge.client) }
                            }
                        }
                    }
                } header: {
                    Text(devices.count > 1 ? "Connectés à \(bridge.client.baseURL.host() ?? "")" : "Navigateurs connectés")
                } footer: {
                    Text("Glisse vers la gauche pour déconnecter un navigateur.")
                }
            }
        }
        .navigationTitle("Navigateurs")
        .overlay { if isLoading && devices.isEmpty { ProgressView() } }
        .refreshable { await load() }
        .task { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        var found: [Bridge] = []
        for client in bridges {
            if let list = (try? await withTimeout(10, { try await client.consoleDevices() })) ?? nil {
                found.append(Bridge(client: client, devices: list.sorted { ($0.seen ?? .distantPast) > ($1.seen ?? .distantPast) }))
            }
        }
        devices = found
        if found.isEmpty, !bridges.isEmpty {
            notice = (String(localized: "Ton serveur n’a pas encore la console web : mets-le à jour (bipagents update)."), true)
        }
    }

    private func approve() async {
        isApproving = true
        defer { isApproving = false }
        let label = name.trimmingCharacters(in: .whitespaces).isEmpty ? String(localized: "Ordinateur") : name
        var lastError: (any Error)?
        for client in bridges {
            do {
                if try await client.approveConsole(code: code, name: label) {
                    notice = (String(localized: "C’est fait : le navigateur s’ouvre sur tes agents."), false)
                    code = ""
                    await load()
                    return
                }
            } catch {
                lastError = error
            }
        }
        notice = (lastError.map { ConversationModel.describe($0) }
                  ?? String(localized: "Aucun navigateur n’attend avec ce code : vérifie-le, ou affiches-en un nouveau (il dure 5 minutes)."), true)
    }

    private func revoke(_ device: BridgeClient.ConsoleDevice, on client: BridgeClient) async {
        do {
            try await client.revokeConsole(device.id)
            await load()
        } catch {
            notice = (ConversationModel.describe(error), true)
        }
    }
}
