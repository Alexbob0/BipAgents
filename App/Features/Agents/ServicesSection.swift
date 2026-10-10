import HermesKit
import SwiftUI

/// « Services connectés »: what an agent reaches beyond its built-in tools (home automation, GitHub, any MCP server).
/// A service with a token needs the agent's own space; the app offers the move where it is refused.
struct ServicesSection: View {
    let agent: AgentProfile
    @Binding var settings: BridgeClient.AgentSettings
    let bridge: BridgeClient?
    let onMoveToOwnSpace: () -> Void

    @State private var isAdding = false
    @State private var results: [String: String] = [:]
    @State private var testing: String?
    @State private var errorMessage: String?

    var body: some View {
        Section {
            ForEach(settings.services) { service in
                HStack(spacing: 12) {
                    Image(systemName: icon(service)).foregroundStyle(agent.appearance.palette.deep).frame(width: 26)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(service.id).font(Theme.body(15.5, weight: .bold))
                        Text(results[service.id] ?? service.url.flatMap { URL(string: $0)?.host() } ?? String(localized: "Installé sur le serveur"))
                            .font(Theme.body(12.5, weight: .regular)).foregroundStyle(Theme.ink2).lineLimit(2)
                    }
                    Spacer()
                    if testing == service.id {
                        ProgressView()
                    } else if service.catalog != nil {
                        Button("Tester") { Task { await test(service) } }.buttonStyle(.borderless)
                    }
                }
                .swipeActions {
                    if service.catalog != nil {
                        Button("Retirer", systemImage: "trash", role: .destructive) { Task { await remove(service) } }
                    }
                }
            }
            Button("Connecter un service", systemImage: "plus") { isAdding = true }
            if let errorMessage {
                Text(errorMessage).font(Theme.body(12.5, weight: .regular)).foregroundStyle(Theme.danger)
            }
        } header: {
            Text("Services connectés")
        } footer: {
            Text("Ce que \(agent.name) peut joindre en plus de ses outils : ta maison connectée, GitHub ou tout serveur MCP. Il s’en sert dès la minute suivante.")
        }
        .sheet(isPresented: $isAdding) {
            NavigationStack {
                ConnectServiceView(agent: agent, isolated: settings.isolated, bridge: bridge,
                                   onMoveToOwnSpace: { isAdding = false; onMoveToOwnSpace() }) {
                    await reload()
                }
            }
        }
    }

    private func icon(_ service: BridgeClient.ConnectedService) -> String {
        switch service.catalog {
        case "home_assistant": "house"
        case "github": "chevron.left.forwardslash.chevron.right"
        case nil: "terminal"
        default: "puzzlepiece.extension"
        }
    }

    private func reload() async {
        if let bridge, let updated = try? await bridge.agentSettings(agent.bridgeName) { settings = updated }
    }

    private func test(_ service: BridgeClient.ConnectedService) async {
        guard let bridge else { return }
        testing = service.id
        defer { testing = nil }
        do {
            let result = try await bridge.testService(service.id, of: agent.bridgeName)
            results[service.id] = result.ok
                ? (result.tools.map { String(localized: "Joint : \($0) outils") } ?? String(localized: "Joint"))
                : String(localized: "Injoignable : \(result.message ?? "?")")
        } catch {
            results[service.id] = ConversationModel.describe(error)
        }
    }

    private func remove(_ service: BridgeClient.ConnectedService) async {
        guard let bridge else { return }
        do {
            try await bridge.disconnectService(service.id, from: agent.bridgeName)
            await reload()
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}

/// The catalog, then the chosen service's fields (its token in a secure field).
struct ConnectServiceView: View {
    let agent: AgentProfile
    let isolated: Bool
    let bridge: BridgeClient?
    let onMoveToOwnSpace: () -> Void
    let onSaved: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var catalog: [BridgeClient.CatalogService] = []
    @State private var chosen: BridgeClient.CatalogService?
    @State private var values: [String: String] = [:]
    @State private var isSaving = false
    @State private var errorMessage: String?
    @State private var needsOwnSpace = false

    var body: some View {
        Form {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            if needsOwnSpace {
                Section {
                    Button("Lui donner son propre espace", systemImage: "lock.shield") { onMoveToOwnSpace() }
                } footer: {
                    Text("Ce service garde un jeton ou touche à un compte personnel : \(agent.name) doit d’abord avoir son propre espace.")
                }
            }
            if let chosen {
                Section {
                    ForEach(chosen.fields, id: \.id) { field in
                        if field.secret {
                            SecureField(field.label, text: binding(field.id))
                        } else {
                            TextField(field.placeholder.isEmpty ? field.label : "\(field.label) (\(field.placeholder))",
                                      text: binding(field.id))
                                .textInputAutocapitalization(.never).autocorrectionDisabled()
                                .keyboardType(field.id == "address" ? .URL : .default)
                        }
                    }
                } header: {
                    Label(chosen.label, systemImage: chosen.icon)
                } footer: {
                    Text(chosen.description)
                }
            } else {
                Section {
                    ForEach(catalog) { service in
                        Button { choose(service) } label: {
                            Label {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(service.label).font(Theme.body(15.5, weight: .bold)).foregroundStyle(Theme.ink)
                                    Text(service.description).font(Theme.body(12.5, weight: .regular))
                                        .foregroundStyle(Theme.ink2).lineLimit(3)
                                }
                            } icon: {
                                Image(systemName: service.icon).foregroundStyle(agent.appearance.palette.deep)
                            }
                        }
                    }
                }
            }
        }
        .scrollDismissesKeyboard(.interactively)
        .navigationTitle("Connecter un service")
        .navigationBarTitleDisplayMode(.inline)
        .overlay { if catalog.isEmpty && errorMessage == nil { ProgressView() } }
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Annuler") { dismiss() } }
            if chosen != nil {
                ToolbarItem(placement: .confirmationAction) {
                    if isSaving { ProgressView() } else { Button("Connecter") { Task { await save() } }.disabled(!canSave) }
                }
            }
        }
        .task {
            guard let bridge else { return }
            do { catalog = try await bridge.toolCatalog() } catch { errorMessage = ConversationModel.describe(error) }
        }
    }

    private var canSave: Bool {
        guard let chosen else { return false }
        return chosen.fields.allSatisfy { $0.optional || !(values[$0.id] ?? "").trimmingCharacters(in: .whitespaces).isEmpty }
    }

    private func binding(_ id: String) -> Binding<String> {
        Binding(get: { values[id] ?? "" }, set: { values[id] = $0 })
    }

    private func choose(_ service: BridgeClient.CatalogService) {
        chosen = service
        values = [:]
        needsOwnSpace = service.personal && !isolated
    }

    private func save() async {
        guard let bridge, let chosen else { return }
        isSaving = true
        defer { isSaving = false }
        do {
            try await bridge.connectService(chosen.id, values: values, to: agent.bridgeName)
            values = [:]
            await onSaved()
            dismiss()
        } catch let refused as BridgeClient.CannotHold {
            needsOwnSpace = refused.agents.values.contains("needs_own_space")
            errorMessage = refused.agents.values.first.map { VaultItemEditor.reason($0, name: agent.name) }
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}
