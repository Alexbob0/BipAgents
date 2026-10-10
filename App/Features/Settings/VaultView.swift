import HermesKit
import LocalAuthentication
import SwiftUI

/// The agents' vault: logins given to the agents, which use them without ever reading them. Opened by Face ID
/// (or the passcode) and locked again as soon as the app leaves the screen. Write-only: a password is replaced,
/// never shown.
struct VaultView: View {
    @Environment(AgentStore.self) private var store
    @Environment(\.scenePhase) private var scenePhase
    @State private var isUnlocked = false
    @State private var installs: [VaultInstall] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var editing: VaultEditing?
    @State private var deleting: (VaultInstall, BridgeClient.VaultItem)?

    var body: some View {
        List {
            if !isUnlocked {
                Section {
                    VStack(spacing: 14) {
                        Image(systemName: "lock.shield").font(.system(size: 44)).foregroundStyle(Theme.ink2)
                        Text("Le coffre est verrouillé").font(Theme.title(18))
                        Button("Déverrouiller", systemImage: "faceid") { Task { await unlock() } }
                            .buttonStyle(.borderedProminent)
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 24)
                }
                .listRowBackground(Color.clear)
            } else {
                if let errorMessage {
                    Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
                }
                ForEach(installs) { install in
                    Section {
                        ForEach(install.items) { item in
                            Button { editing = VaultEditing(install: install, item: item) } label: { row(item, in: install) }
                                .buttonStyle(.plain)
                                .swipeActions {
                                    Button("Supprimer", systemImage: "trash", role: .destructive) { deleting = (install, item) }
                                }
                        }
                    } header: {
                        if installs.count > 1 { Text(install.title) }
                    } footer: {
                        if install.items.isEmpty {
                            Text("Aucun identifiant pour l’instant. Ajoute celui d’un site, puis choisis quels agents peuvent s’en servir.")
                        }
                    }
                }
                Section {
                    Label("Tes agents remplissent eux-mêmes le formulaire de connexion, seulement sur le site enregistré. Ils ne voient jamais le mot de passe, et l’app ne le réaffiche jamais : tu peux le remplacer ou le supprimer.",
                          systemImage: "eye.slash")
                        .font(Theme.body(13, weight: .regular))
                        .foregroundStyle(Theme.ink2)
                }
                .listRowBackground(Color.clear)
            }
        }
        .navigationTitle("Coffre")
        .overlay { if isUnlocked && isLoading && installs.isEmpty { ProgressView() } }
        .toolbar {
            if isUnlocked, let install = installs.first {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Ajouter un identifiant", systemImage: "plus") {
                        editing = VaultEditing(install: install, item: nil)
                    }
                }
            }
        }
        .refreshable { if isUnlocked { await load() } }
        .task { await unlock() }
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { lock() }
        }
        .onDisappear { lock() }
        .sheet(item: $editing) { editing in
            NavigationStack {
                VaultItemEditor(install: editing.install, item: editing.item, installs: installs) { await load() }
            }
        }
        .confirmationDialog(Text("Supprimer « \(deleting?.1.label ?? "") » ?"),
                            isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }),
                            titleVisibility: .visible) {
            Button("Supprimer", role: .destructive) {
                if let (install, item) = deleting { Task { await delete(item, from: install) } }
            }
            Button("Annuler", role: .cancel) {}
        } message: {
            Text("Les agents qui s’en servaient ne pourront plus se connecter à ce site.")
        }
    }

    private func row(_ item: BridgeClient.VaultItem, in install: VaultInstall) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "key.fill").foregroundStyle(Theme.ink2).frame(width: 28)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(item.label).font(Theme.body(16, weight: .bold)).foregroundStyle(Theme.ink)
                    if item.hasOTP {
                        Text("2FA").font(Theme.body(11, weight: .heavy)).padding(.horizontal, 5).padding(.vertical, 1)
                            .background(Theme.ink.opacity(0.08), in: .capsule)
                    }
                }
                Text("\(URL(string: item.origin)?.host() ?? item.origin) · \(item.identifier)")
                    .font(Theme.body(13)).foregroundStyle(Theme.ink2).lineLimit(1)
                Text(item.agents.isEmpty ? String(localized: "Aucun agent")
                     : item.agents.map { install.name(of: $0) }.formatted(.list(type: .and)))
                    .font(Theme.body(12.5)).foregroundStyle(Theme.muted)
            }
            Spacer()
        }
        .contentShape(.rect)
    }

    // MARK: Lock

    private func unlock() async {
        guard !isUnlocked else { return }
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: &error) else {
            errorMessage = String(localized: "Active un code sur l’iPhone pour utiliser le coffre.")
            return
        }
        do {
            isUnlocked = try await context.evaluatePolicy(.deviceOwnerAuthentication,
                                                         localizedReason: String(localized: "Ouvrir le coffre des agents"))
        } catch {
            isUnlocked = false
        }
        if isUnlocked { await load() }
    }

    private func lock() {
        isUnlocked = false
        installs = []
        editing = nil
    }

    // MARK: Data

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        var found: [VaultInstall] = []
        var seen = Set<String>()
        for agent in store.agents {
            guard let bridgeURL = agent.config.bridgeURL, seen.insert(bridgeURL.absoluteString).inserted,
                  let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            if let vault = try? await withTimeout(10, { try await bridge.vault() }) {
                found.append(VaultInstall(id: bridgeURL.absoluteString, title: bridgeURL.host() ?? bridgeURL.absoluteString,
                                          bridge: bridge, items: vault.items, agents: vault.agents))
            }
        }
        installs = found
        errorMessage = found.isEmpty ? String(localized: "Aucun de tes serveurs n’a de coffre pour l’instant (bridge à mettre à jour).") : nil
    }

    private func delete(_ item: BridgeClient.VaultItem, from install: VaultInstall) async {
        do {
            try await install.bridge.deleteVaultItem(item.id)
            await load()
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}

/// One install's vault (its bridge), with the agents it can give credentials to.
struct VaultInstall: Identifiable, Sendable {
    var id: String
    var title: String
    var bridge: BridgeClient
    var items: [BridgeClient.VaultItem]
    var agents: [BridgeClient.VaultAgent]

    func name(of agent: String) -> String {
        agents.first { $0.id == agent }?.name ?? agent
    }
}

struct VaultEditing: Identifiable {
    var install: VaultInstall
    var item: BridgeClient.VaultItem?
    var id: String { (item?.id ?? "new") + install.id }
}

/// A login: name, site, user, password (write-only), 2FA key, and the agents that may use it.
struct VaultItemEditor: View {
    @State var install: VaultInstall
    let item: BridgeClient.VaultItem?
    let installs: [VaultInstall]
    let onSaved: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var label = ""
    @State private var site = ""
    @State private var identifier = ""
    @State private var password = ""
    @State private var otpSecret = ""
    @State private var removesOTP = false
    @State private var agents: Set<String> = []
    @State private var isSaving = false
    @State private var errorMessage: String?

    private var isNew: Bool { item == nil }

    private var canSave: Bool {
        !label.trimmingCharacters(in: .whitespaces).isEmpty && !site.trimmingCharacters(in: .whitespaces).isEmpty
            && !identifier.trimmingCharacters(in: .whitespaces).isEmpty && (!isNew || !password.isEmpty) && !isSaving
    }

    var body: some View {
        Form {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            if isNew && installs.count > 1 {
                Picker("Serveur", selection: Binding(get: { install.id }, set: { id in
                    if let other = installs.first(where: { $0.id == id }) { install = other; agents = [] }
                })) {
                    ForEach(installs) { Text($0.title).tag($0.id) }
                }
            }
            Section {
                TextField("Nom (ex. SNCF)", text: $label)
                TextField("Site (ex. sncf-connect.com)", text: $site)
                    .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                TextField("E-mail ou identifiant", text: $identifier)
                    .textContentType(.username).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField(isNew ? "Mot de passe" : "Nouveau mot de passe (vide : inchangé)", text: $password)
                    .textContentType(.password)
            } footer: {
                Text("Le site doit être exactement celui de la page de connexion : l’agent ne remplira le mot de passe nulle part ailleurs.")
            }
            Section {
                if let item, item.hasOTP, !removesOTP {
                    LabeledContent("Code 2FA", value: String(localized: "Enregistré"))
                    Button("Retirer le code 2FA", role: .destructive) { removesOTP = true }
                } else {
                    TextField("Clé 2FA (facultative)", text: $otpSecret)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                }
            } footer: {
                Text("La clé de configuration que le site donne pour une app d’authentification (ou le lien otpauth://). L’agent saura alors entrer le code à 6 chiffres.")
            }
            Section {
                ForEach(install.agents) { agent in
                    Toggle(isOn: Binding(get: { agents.contains(agent.id) }, set: { on in
                        if on { agents.insert(agent.id) } else { agents.remove(agent.id) }
                    })) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(agent.name)
                            if let why = agent.why, !agents.contains(agent.id) {
                                Text(Self.reason(why, name: agent.name))
                                    .font(Theme.body(12.5, weight: .regular)).foregroundStyle(Theme.ink2)
                            }
                        }
                    }
                    .disabled(!agent.canHold && !agents.contains(agent.id))
                }
            } header: {
                Text("Agents qui peuvent s’en servir")
            } footer: {
                Text("Un agent ne reçoit des identifiants que s’il a son propre espace sur le serveur : un agent qui partage le sien pourrait lire ceux des autres.")
            }
        }
        .scrollDismissesKeyboard(.interactively)
        .navigationTitle(isNew ? "Nouvel identifiant" : label)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Annuler") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                if isSaving { ProgressView() } else { Button("Enregistrer") { Task { await save() } }.disabled(!canSave) }
            }
        }
        .onAppear {
            guard let item, label.isEmpty else { return }
            label = item.label
            site = item.origin
            identifier = item.identifier
            agents = Set(item.agents)
        }
    }

    static func reason(_ why: String, name: String) -> String {
        why == "needs_own_space"
            ? String(localized: "\(name) partage son espace avec d’autres agents : il lui faut d’abord le sien.")
            : String(localized: "\(name) n’est pas encore branché au coffre sur le serveur.")
    }

    private func save() async {
        isSaving = true
        defer { isSaving = false }
        let otp: String? = removesOTP ? "" : (otpSecret.trimmingCharacters(in: .whitespaces).isEmpty ? nil : otpSecret)
        let login = BridgeClient.VaultLogin(label: label, site: site, identifier: identifier,
                                            password: password.isEmpty ? nil : password, otpSecret: otp,
                                            agents: Array(agents).sorted())
        do {
            try await install.bridge.saveVaultItem(login, id: item?.id)
            password = ""
            await onSaved()
            dismiss()
        } catch let refused as BridgeClient.CannotHold {
            errorMessage = refused.agents.map { Self.reason($0.value, name: install.name(of: $0.key)) }.joined(separator: "\n")
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}
