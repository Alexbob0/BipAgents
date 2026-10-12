import HermesKit
import SwiftUI

/// « Créer un agent »: on an install that manages its agents, a kind (or « Autre », described by the person), a name
/// the person gives, and the bridge creates it on the server. No terminal, no keys to copy.
struct CreateAgentFlow: View {
    let install: AgentStore.CreatableInstall
    var onCreated: () -> Void

    @Environment(AgentStore.self) private var store
    @State private var kind: BridgeClient.AgentKind?
    @State private var name = ""
    @State private var role = ""
    @State private var tone = ""
    @State private var rules = ""
    @State private var language = AgentLanguage.device
    @State private var category = AgentCategory.tech
    @State private var isCreating = false
    @State private var errorMessage: String?

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 10), count: 3)

    var body: some View {
        Group {
            if let kind { details(for: kind) } else { kinds }
        }
        .background(Theme.background)
        .navigationTitle(kind == nil ? String(localized: "Quel genre d’agent ?") : String(localized: "Ton agent"))
        .navigationBarTitleDisplayMode(.inline)
        .animation(.snappy, value: kind)
    }

    // MARK: Step 1: the kind

    private var kinds: some View {
        ScrollView {
            LazyVGrid(columns: columns, spacing: 10) {
                ForEach(install.kinds) { kind in
                    Button {
                        if let category = AgentCategory(rawValue: kind.id) { self.category = category }
                        self.kind = kind
                    } label: {
                        VStack(spacing: 6) {
                            MascotView(appearance: AgentAppearance(category: AgentCategory(rawValue: kind.id) ?? .tech),
                                       animated: false)
                                .frame(width: 58, height: 58)
                                .overlay(alignment: .topTrailing) {
                                    if kind.custom {
                                        Image(systemName: "slider.horizontal.3")
                                            .font(.system(size: 13, weight: .bold))
                                            .foregroundStyle(Theme.ink)
                                            .padding(5)
                                            .background(Theme.card, in: .circle)
                                    }
                                }
                            Text(kind.custom ? String(localized: "Autre") : kind.label)
                                .font(Theme.body(14, weight: .heavy))
                                .foregroundStyle(Theme.ink)
                            Text(kind.custom ? String(localized: "Sur mesure") : kind.description)
                                .font(Theme.body(11.5))
                                .foregroundStyle(Theme.muted)
                                .multilineTextAlignment(.center)
                                .lineLimit(2)
                        }
                        .padding(.vertical, 14)
                        .padding(.horizontal, 8)
                        .frame(maxWidth: .infinity, minHeight: 150)
                        .background((AgentCategory(rawValue: kind.id)?.palette.tint ?? Theme.card),
                                    in: .rect(cornerRadius: 22, style: .continuous))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(16)
        }
    }

    // MARK: Step 2: its name (and, for « Autre », what it does)

    private func details(for kind: BridgeClient.AgentKind) -> some View {
        Form {
            Section {
                VStack(spacing: 10) {
                    MascotView(appearance: AgentAppearance(category: category), mood: isCreating ? .thinking : .happy,
                               animated: isCreating)
                        .frame(width: 110, height: 110)
                    TextField("Son nom", text: $name)
                        .font(Theme.title(20))
                        .multilineTextAlignment(.center)
                        .frame(height: 44)
                        .background(Theme.card, in: .capsule)
                        .padding(.horizontal, 30)
                        .disabled(isCreating)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 12)
                .listRowBackground(category.palette.tint)
            } footer: {
                if !kind.custom { Text(kind.description) }
            }
            if kind.custom {
                Section("Ce qu’il fait pour toi") {
                    TextField("Ex. suit mes plantes et me dit quand les arroser", text: $role, axis: .vertical)
                        .lineLimit(3...6)
                }
                Section("Son caractère") {
                    TextField("Son ton (ex. direct, chaleureux, drôle)", text: $tone)
                    TextField("Ce qu’il doit toujours ou ne jamais faire", text: $rules, axis: .vertical)
                        .lineLimit(2...4)
                    Picker("Langue", selection: $language) {
                        ForEach(AgentLanguage.allCases) { Text($0.nativeName).tag($0) }
                    }
                }
                Section("Sa couleur") {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 10) {
                            ForEach(AgentCategory.allCases) { option in
                                Button { category = option } label: {
                                    Circle()
                                        .fill(option.palette.main)
                                        .frame(width: 34, height: 34)
                                        .padding(3)
                                        .overlay(Circle().stroke(Theme.ink, lineWidth: category == option ? 2 : 0))
                                }
                                .buttonStyle(.plain)
                                .accessibilityLabel(Text(option.label))
                            }
                        }
                        .padding(.vertical, 4)
                    }
                }
            }
            if let errorMessage {
                Section { Text(errorMessage).foregroundStyle(Theme.danger) }
            }
            Section {
                Button {
                    create(kind)
                } label: {
                    HStack {
                        Spacer()
                        if isCreating {
                            ProgressView().padding(.trailing, 6)
                            Text("Je prépare \(name.trimmingCharacters(in: .whitespaces))…")
                        } else {
                            Text("Créer")
                        }
                        Spacer()
                    }
                    .font(Theme.body(16, weight: .heavy))
                }
                .disabled(!canCreate(kind) || isCreating)
            } footer: {
                if isCreating { Text("Ça peut prendre jusqu’à une demi-minute, le temps que le serveur le démarre.") }
            }
        }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button("Genres", systemImage: "chevron.left") { self.kind = nil }.disabled(isCreating)
            }
        }
        .navigationBarBackButtonHidden()
    }

    private func canCreate(_ kind: BridgeClient.AgentKind) -> Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && (!kind.custom || role.trimmingCharacters(in: .whitespaces).count >= 3)
    }

    private func create(_ kind: BridgeClient.AgentKind) {
        let name = name.trimmingCharacters(in: .whitespaces)
        let custom = kind.custom ? BridgeClient.CustomAgent(role: role, tone: tone, rules: rules,
                                                            language: language.nativeName, category: category.rawValue) : nil
        isCreating = true
        errorMessage = nil
        Task {
            defer { isCreating = false }
            do {
                let bridge = BridgeClient(url: install.bridgeURL, key: install.bridgeKey)
                // An « Autre » agent speaks the language picked for it; a ready-made one, the iPhone's.
                let spoken = kind.custom ? language : AgentLanguage.device
                let agent = try await bridge.createAgent(name: name, kind: kind.custom ? nil : kind.id, custom: custom,
                                                         language: spoken)
                try store.addDiscovered(agent, bridgeURL: install.bridgeURL, bridgeKey: install.bridgeKey,
                                        lanURL: install.lanURL, lanFingerprint: install.lanFingerprint, category: category,
                                        language: spoken)
                onCreated()
            } catch {
                errorMessage = String(localized: "Le serveur n’a pas pu créer cet agent. Réessaie dans un instant.")
            }
        }
    }
}
