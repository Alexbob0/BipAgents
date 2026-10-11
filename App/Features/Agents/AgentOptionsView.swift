import HermesKit
import SwiftUI

/// Everything about one agent, in one place: from the gear on its page, or from Réglages › its row. Its look, language
/// and voice; its routines; its desktop; and for an agent this install created, its personality, tools, services,
/// space and deletion on the server.
struct AgentOptionsView: View {
    let agent: AgentProfile

    @Environment(AgentStore.self) private var store
    @Environment(Router.self) private var router
    @Environment(\.dismiss) private var dismiss
    @State private var settings: BridgeClient.AgentSettings?
    @State private var desktop: BridgeClient.Desktop?
    @State private var desktopError: String?
    @State private var isLoading = true
    @State private var isEditingStyle = false
    @State private var confirmsRemoval = false

    /// The agent as it is now (its look may have just changed).
    private var current: AgentProfile { store.agents.first { $0.id == agent.id } ?? agent }
    private var palette: AgentPalette { current.appearance.palette }

    var body: some View {
        List {
            Section {
                Button { isEditingStyle = true } label: {
                    HStack(spacing: 14) {
                        MascotAvatar(appearance: current.appearance, size: 52)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(current.name).font(Theme.body(17, weight: .heavy)).foregroundStyle(Theme.ink)
                            Text("Apparence, langue et voix").font(Theme.body(13)).foregroundStyle(Theme.ink2)
                        }
                        Spacer()
                        Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(Theme.muted)
                    }
                }
            }
            Section {
                row("Routines", "Ce que \(current.name) fait tout seul, et quand", icon: "clock.arrow.circlepath") {
                    RoutinesView(agent: current)
                }
                if let desktop {
                    row("Bureau", "Son navigateur, en direct", icon: "macwindow") {
                        DesktopView(agent: current, desktop: desktop)
                    }
                } else if let desktopError {
                    Label {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Bureau").font(Theme.body(16, weight: .heavy))
                            Text(desktopError).font(Theme.body(12.5, weight: .regular)).foregroundStyle(Theme.danger)
                        }
                    } icon: {
                        Image(systemName: "macwindow").foregroundStyle(Theme.muted)
                    }
                }
                if let settings {
                    row("Personnalité et outils", "Qui est \(current.name), ce qu’il peut faire", icon: "slider.horizontal.3") {
                        AgentSettingsView(agent: current, settings: settings)
                    }
                }
            } footer: {
                if !isLoading && settings == nil {
                    Text("\(current.name) a été installé à la main sur le serveur : sa personnalité et ses outils se règlent là-bas.")
                }
            }
            Section {
                Button("Retirer de cet iPhone", role: .destructive) { confirmsRemoval = true }
                    .confirmationDialog(Text("Retirer \(current.name) de cet iPhone ?"), isPresented: $confirmsRemoval,
                                        titleVisibility: .visible) {
                        Button("Retirer", role: .destructive) {
                            router.agentsPath = NavigationPath()
                            store.remove(current)
                            dismiss()
                        }
                        Button("Annuler", role: .cancel) {}
                    } message: {
                        Text("Seulement de cette app : \(current.name) reste sur le serveur, avec ses conversations. Tu pourras le rajouter avec le QR code.")
                    }
            }
        }
        .navigationTitle("Réglages de \(current.name)")
        .navigationBarTitleDisplayMode(.inline)
        .tabBarHidden(false)
        .overlay { if isLoading && settings == nil && desktop == nil { ProgressView() } }
        .task { await load() }
        .refreshable { await load() }
        .sheet(isPresented: $isEditingStyle) { EditAgentStyleView(agent: current) }
    }

    private func row<Destination: View>(_ title: LocalizedStringKey, _ subtitle: LocalizedStringKey, icon: String,
                                        @ViewBuilder destination: () -> Destination) -> some View {
        NavigationLink(destination: destination) {
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(title).font(Theme.body(16, weight: .heavy))
                    Text(subtitle).font(Theme.body(13)).foregroundStyle(Theme.ink2)
                }
            } icon: {
                Image(systemName: icon).foregroundStyle(palette.deep)
            }
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        guard let bridge = BridgeClient(agent: current, secrets: store.secrets(for: current)) else { return }
        async let foundSettings = try? withTimeout(8, { try await bridge.agentSettings(current.bridgeName) })
        do {
            desktop = try await withTimeout(8, { try await bridge.desktop(of: current.bridgeName) })
            desktopError = nil
        } catch {
            // Not « no desktop » (that is nil): the server did not answer, say why.
            desktop = nil
            desktopError = ConversationModel.describe(error)
        }
        settings = (await foundSettings) ?? nil
    }
}
