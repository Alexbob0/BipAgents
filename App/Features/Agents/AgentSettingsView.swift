import HermesKit
import SwiftUI

/// An agent this install created, changed from the app: its personality, what it may do, or deleting it.
/// Hermes reads both on each run, so a change counts from the next message.
struct AgentSettingsView: View {
    let agent: AgentProfile
    @State var settings: BridgeClient.AgentSettings

    @Environment(AgentStore.self) private var store
    @Environment(Router.self) private var router
    @State private var soul = ""
    @State private var isSaving = false
    @State private var savedSoul = false
    @State private var errorMessage: String?
    @State private var showsOtherTools = false
    @State private var confirming: ToolInfo?
    @State private var confirmsDeletion = false
    @State private var isDeleting = false
    @State private var confirmsMove = false
    @State private var isMoving = false
    @FocusState private var editsSoul: Bool

    private var palette: AgentPalette { agent.appearance.palette }
    private var bridge: BridgeClient? { BridgeClient(agent: agent, secrets: store.secrets(for: agent)) }

    var body: some View {
        Form {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            Section {
                TextEditor(text: $soul)
                    .font(Theme.body(15, weight: .regular))
                    .frame(minHeight: 220)
                    .focused($editsSoul)
                    .onChange(of: soul) { savedSoul = false }
            } header: {
                Text("Personnalité")
            } footer: {
                Text("Son rôle, son ton et ses règles, comme tu les lui expliquerais. \(agent.name) s’en sert dès le prochain message.")
            }
            Section {
                ForEach(ToolInfo.main.compactMap(toolset(for:)), id: \.0.id) { info, toolset in
                    toggle(info, toolset)
                }
                DisclosureGroup("Autres outils", isExpanded: $showsOtherTools) {
                    ForEach(otherToolsets) { toolset in
                        toggle(ToolInfo(id: toolset.id, label: Self.plainLabel(toolset.label), icon: "puzzlepiece.extension",
                                        detail: toolset.description, sensitive: ToolInfo.sensitive.contains(toolset.id)),
                               toolset)
                    }
                }
            } header: {
                Text("Ce que \(agent.name) peut faire")
            } footer: {
                Text("Un outil grisé demande une clé ou un compte à configurer sur le serveur.")
            }
            Section {
                if settings.isolated {
                    Label("Son propre espace", systemImage: "lock.shield.fill")
                } else if isMoving {
                    Label("Installation de son espace…", systemImage: "hourglass")
                } else {
                    Button("Lui donner son propre espace", systemImage: "lock.shield") { confirmsMove = true }
                        .confirmationDialog(Text("Donner son propre espace à \(agent.name) ?"), isPresented: $confirmsMove,
                                            titleVisibility: .visible) {
                            Button("Lui donner son espace") { Task { await moveToOwnSpace() } }
                            Button("Annuler", role: .cancel) {}
                        } message: {
                            Text(OwnSpace.explanation(agent.name))
                        }
                }
            } header: {
                Text("Son espace")
            } footer: {
                Text(settings.isolated
                     ? "\(agent.name) a son espace à lui : il peut se servir d’identifiants du coffre, et les autres agents ne peuvent pas lire ses fichiers."
                     : "\(agent.name) partage le serveur avec les autres agents créés ici. Pour qu’il se serve d’identifiants du coffre, donne-lui son propre espace.")
            }
            Section {
                Button(role: .destructive) { confirmsDeletion = true } label: {
                    HStack {
                        Text("Supprimer \(agent.name)")
                        Spacer()
                        if isDeleting { ProgressView() }
                    }
                }
                .disabled(isDeleting)
                .confirmationDialog("Supprimer \(agent.name) ?", isPresented: $confirmsDeletion, titleVisibility: .visible) {
                    Button("Supprimer \(agent.name)", role: .destructive) { Task { await delete() } }
                    Button("Annuler", role: .cancel) {}
                } message: {
                    Text("Ses conversations, sa mémoire et ses routines sont effacées du serveur. C’est définitif.")
                }
            } footer: {
                Text("Ses conversations, sa mémoire et ses routines sont effacées du serveur.")
            }
        }
        .scrollDismissesKeyboard(.interactively)
        .navigationTitle("Personnalité et outils")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                if isSaving {
                    ProgressView()
                } else if savedSoul {
                    Image(systemName: "checkmark").foregroundStyle(palette.deep).accessibilityLabel("Enregistré")
                } else {
                    Button("Enregistrer") { Task { await saveSoul() } }
                        .disabled(soul.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                  || soul == settings.soul)
                }
            }
        }
        .onAppear { if soul.isEmpty { soul = settings.soul } }
    }

    // MARK: Tools

    private func toolset(for info: ToolInfo) -> (ToolInfo, BridgeClient.Toolset)? {
        settings.toolsets.first { $0.id == info.id }.map { (info, $0) }
    }

    private var otherToolsets: [BridgeClient.Toolset] {
        let main = Set(ToolInfo.main.map(\.id))
        return settings.toolsets.filter { !main.contains($0.id) && !ToolInfo.hidden.contains($0.id) }
    }

    private func toggle(_ info: ToolInfo, _ toolset: BridgeClient.Toolset) -> some View {
        Toggle(isOn: Binding(get: { toolset.enabled }, set: { on in
            if on && info.sensitive {
                confirming = info
            } else {
                Task { await set(info.id, on: on) }
            }
        })) {
            Label {
                VStack(alignment: .leading, spacing: 2) {
                    Text(info.label).font(Theme.body(15.5, weight: .bold))
                    if !info.detail.isEmpty {
                        Text(info.detail).font(Theme.body(12.5, weight: .regular)).foregroundStyle(Theme.ink2)
                    }
                }
            } icon: {
                Image(systemName: info.icon).foregroundStyle(palette.deep)
            }
        }
        .tint(palette.deep)
        .disabled(!toolset.configured || isSaving)
        // On the row itself, so the bubble points at the switch that asked.
        .confirmationDialog(Text("Donner « \(info.label) » à \(agent.name) ?"),
                            isPresented: Binding(get: { confirming?.id == info.id }, set: { if !$0 { confirming = nil } }),
                            titleVisibility: .visible) {
            Button("Activer") { Task { await set(info.id, on: true) } }
            Button("Annuler", role: .cancel) {}
        } message: {
            Text("\(agent.name) pourra agir sur le serveur où il tourne, qu’il partage avec les autres agents créés ici. Ne lui confie pas de mots de passe : les agents qui en gardent auront bientôt leur propre espace.")
        }
    }

    private func set(_ toolset: String, on: Bool) async {
        guard let bridge else { return }
        if let index = settings.toolsets.firstIndex(where: { $0.id == toolset }) { settings.toolsets[index].enabled = on }
        isSaving = true
        defer { isSaving = false }
        do {
            if let updated = try await bridge.updateAgentSettings(agent.bridgeName, toolsets: [toolset: on]) {
                settings.toolsets = updated.toolsets
            }
            errorMessage = nil
        } catch {
            if let index = settings.toolsets.firstIndex(where: { $0.id == toolset }) { settings.toolsets[index].enabled = !on }
            errorMessage = ConversationModel.describe(error)
        }
    }

    // MARK: Personality and deletion

    private func saveSoul() async {
        guard let bridge else { return }
        isSaving = true
        defer { isSaving = false }
        do {
            try await bridge.updateAgentSettings(agent.bridgeName, soul: soul)
            settings.soul = soul
            savedSoul = true
            editsSoul = false
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func moveToOwnSpace() async {
        guard let bridge, let bridgeURL = agent.config.bridgeURL else { return }
        isMoving = true
        defer { isMoving = false }
        do {
            if let url = try await bridge.moveToOwnSpace(agent.bridgeName) {
                store.relocate(bridgeAgent: agent.bridgeName, of: bridgeURL, to: url)
            }
            settings.isolated = true
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func delete() async {
        guard let bridge else { return }
        isDeleting = true
        defer { isDeleting = false }
        do {
            try await bridge.deleteAgent(agent.bridgeName)
            router.agentsPath = NavigationPath()
            store.remove(agent)
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    /// « 🔍 Web Search & Scraping » → « Web Search & Scraping ».
    static func plainLabel(_ label: String) -> String {
        String(label.drop { !$0.isLetter && !$0.isNumber }).trimmingCharacters(in: .whitespaces)
    }
}

/// How a Hermes toolset reads in the app: the everyday ones first, in plain words.
struct ToolInfo: Identifiable, Hashable {
    var id: String
    var label: String
    var icon: String
    var detail: String
    var sensitive = false

    static let main: [ToolInfo] = [
        ToolInfo(id: "web", label: String(localized: "Chercher sur le web"), icon: "magnifyingglass",
                 detail: String(localized: "Recherche et lecture de pages")),
        ToolInfo(id: "browser", label: String(localized: "Naviguer sur des sites"), icon: "safari",
                 detail: String(localized: "Ouvre des pages, clique, remplit des formulaires"), sensitive: true),
        ToolInfo(id: "vision", label: String(localized: "Regarder des images"), icon: "eye",
                 detail: String(localized: "Décrit tes photos et captures")),
        ToolInfo(id: "image_gen", label: String(localized: "Créer des images"), icon: "paintpalette",
                 detail: ""),
        ToolInfo(id: "memory", label: String(localized: "Se souvenir de toi"), icon: "brain",
                 detail: String(localized: "Garde ce que tu lui apprends d’une conversation à l’autre")),
        ToolInfo(id: "session_search", label: String(localized: "Retrouver d’anciennes conversations"), icon: "text.magnifyingglass",
                 detail: ""),
        ToolInfo(id: "cronjob", label: String(localized: "Programmer des routines"), icon: "clock.arrow.circlepath",
                 detail: String(localized: "Quand tu lui demandes « rappelle-moi… »")),
        ToolInfo(id: "delegation", label: String(localized: "Déléguer des sous-tâches"), icon: "person.2",
                 detail: String(localized: "Lance un assistant pour un travail long")),
        ToolInfo(id: "todo", label: String(localized: "Planifier ses tâches"), icon: "checklist", detail: ""),
        ToolInfo(id: "skills", label: String(localized: "Utiliser ses savoir-faire"), icon: "book", detail: ""),
        ToolInfo(id: "file", label: String(localized: "Lire et écrire des fichiers"), icon: "doc",
                 detail: String(localized: "Sur le serveur"), sensitive: true),
        ToolInfo(id: "terminal", label: String(localized: "Lancer des commandes"), icon: "terminal",
                 detail: String(localized: "Sur le serveur"), sensitive: true),
        ToolInfo(id: "code_execution", label: String(localized: "Exécuter du code"), icon: "chevron.left.forwardslash.chevron.right",
                 detail: String(localized: "Sur le serveur"), sensitive: true),
    ]

    /// Tools that act on the machine: turning one on asks first.
    static let sensitive: Set<String> = ["browser", "file", "terminal", "code_execution", "computer_use"]

    /// Not a choice for the person: the app's questions go through it.
    static let hidden: Set<String> = ["clarify"]
}
