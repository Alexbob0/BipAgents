import HermesKit
import SwiftUI

/// Add an agent: connection (QR payload or manual fields), then its mascot.
struct AddAgentFlow: View {
    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var baseURL = "https://"
    @State private var apiKey = ""
    @State private var voice = "" // empty: the category's Bip voice (changeable later in Réglages)
    @State private var bridgeURL = ""
    @State private var bridgeKey = ""
    /// The bridge's local-network door, from the QR code only (its certificate fingerprint cannot be typed).
    @State private var lanURL: URL?
    @State private var lanFingerprint: String?
    @State private var appearance = AgentAppearance(category: .daily)
    @State private var categoryWasPicked = false
    @State private var errorMessage: String?
    @State private var step = Step.connection
    @State private var isScanning = false

    enum Step { case connection, style }

    var body: some View {
        NavigationStack {
            Group {
                switch step {
                case .connection: connectionForm
                case .style: AgentStylePicker(name: $name, appearance: $appearance, categoryWasPicked: $categoryWasPicked)
                }
            }
            .background(Theme.background)
            .navigationTitle(step == .connection ? String(localized: "Ajouter un agent") : String(localized: "Son style"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Annuler") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    switch step {
                    case .connection: Button("Suivant", action: goToStyle).disabled(!connectionIsValid)
                    case .style: Button("Ajouter", action: save).fontWeight(.heavy)
                    }
                }
            }
        }
    }

    private var connectionForm: some View {
        Form {
            Section {
                if QRScanner.isAvailable {
                    Button("Scanner le QR code", systemImage: "qrcode.viewfinder") { isScanning = true }
                        .fontWeight(.heavy)
                }
                Button("Coller la configuration", systemImage: "doc.on.clipboard", action: pasteConfiguration)
            } footer: {
                Text("Le QR code affiché par ton serveur contient l’adresse, la clé et la voix de l’agent.")
            }
            Section("Agent") {
                TextField("Nom", text: $name)
                TextField("Adresse", text: $baseURL)
                    .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("Clé API", text: $apiKey)
                TextField("Voix (vide : voix du Bip)", text: $voice)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
            }
            Section {
                TextField("Adresse du bridge", text: $bridgeURL)
                    .keyboardType(.URL).textInputAutocapitalization(.never).autocorrectionDisabled()
                SecureField("Clé du bridge", text: $bridgeKey)
            } header: {
                Text("Bridge (voix, fichiers, notifications)")
            } footer: {
                Text("Les clés sont gardées dans le Trousseau de cet iPhone uniquement.")
            }
            if let errorMessage {
                Section { Text(errorMessage).foregroundStyle(Theme.danger) }
            }
        }
        .scrollContentBackground(.hidden)
        .fullScreenCover(isPresented: $isScanning) {
            ZStack(alignment: .top) {
                QRScanner { payload in
                    if updatesExistingAgent(payload) { isScanning = false; dismiss(); return }
                    isScanning = false
                    if apply(payload) { goToStyle() }
                }
                .ignoresSafeArea()
                HStack {
                    Button { isScanning = false } label: {
                        Image(systemName: "xmark").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
                            .frame(width: 44, height: 44).background(.black.opacity(0.4), in: .circle)
                    }
                    .accessibilityLabel("Fermer")
                    Spacer()
                    Text("Scanne le QR code de l’agent").font(Theme.title(16)).foregroundStyle(.white)
                    Spacer()
                    Color.clear.frame(width: 44, height: 44)
                }
                .padding()
            }
        }
    }

    private var connectionIsValid: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty && URL(string: baseURL)?.host() != nil && !apiKey.isEmpty
    }

    private func pasteConfiguration() {
        guard let string = UIPasteboard.general.string else {
            errorMessage = String(localized: "Le presse-papiers est vide.")
            return
        }
        _ = apply(string)
    }

    /// Fills the form from a provisioning payload (QR / JSON). Returns false if it is not valid.
    private func apply(_ string: String) -> Bool {
        do {
            let provisioning = try AgentProvisioning(qrPayload: string)
            name = provisioning.config.name
            baseURL = provisioning.config.baseURL.absoluteString
            voice = provisioning.config.voice ?? voice
            bridgeURL = provisioning.config.bridgeURL?.absoluteString ?? ""
            apiKey = provisioning.secrets.apiKey
            bridgeKey = provisioning.secrets.bridgeKey ?? ""
            lanURL = provisioning.config.lanURL
            lanFingerprint = provisioning.config.lanFingerprint
            errorMessage = nil
            return true
        } catch {
            errorMessage = String(localized: "Configuration illisible : \(error.localizedDescription)")
            return false
        }
    }

    /// The QR code of an agent already set up (same Hermes address): its connection is updated (new keys, local
    /// address), nothing else changes.
    private func updatesExistingAgent(_ payload: String) -> Bool {
        guard let provisioning = try? AgentProvisioning(qrPayload: payload) else { return false }
        return (try? store.updateConnection(from: provisioning)) == true
    }

    private func goToStyle() {
        if !categoryWasPicked { appearance.category = AgentCategory.suggest(name: name) }
        withAnimation(.snappy) { step = .style }
    }

    private func save() {
        guard let url = URL(string: baseURL) else { return }
        let config = AgentConfig(
            name: name.trimmingCharacters(in: .whitespaces),
            baseURL: url,
            voice: voice.isEmpty ? nil : voice,
            category: appearance.category.rawValue,
            bridgeURL: URL(string: bridgeURL).flatMap { $0.host() == nil ? nil : $0 },
            language: AgentLanguage.device.rawValue,
            lanURL: lanURL,
            lanFingerprint: lanFingerprint
        )
        do {
            try store.add(AgentProfile(config: config, appearance: appearance),
                          secrets: AgentSecrets(apiKey: apiKey, bridgeKey: bridgeKey.isEmpty ? nil : bridgeKey))
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
            step = .connection
        }
    }
}

/// Category grid (with mascots) plus custom colors. Also used to edit an existing agent.
struct AgentStylePicker: View {
    @Binding var name: String
    @Binding var appearance: AgentAppearance
    @Binding var categoryWasPicked: Bool

    private let columns = Array(repeating: GridItem(.flexible(), spacing: 8), count: 4)

    var body: some View {
        ScrollView {
            VStack(spacing: 18) {
                VStack(spacing: 10) {
                    InteractiveMascot(appearance: appearance, size: 132)
                        .animation(.bouncy, value: appearance)
                    TextField("Nom", text: $name)
                        .font(Theme.title(20))
                        .multilineTextAlignment(.center)
                        .frame(width: 220, height: 44)
                        .background(Theme.card, in: .capsule)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 20)
                .background(appearance.palette.tint, in: .rect(cornerRadius: 32, style: .continuous))

                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text("Catégorie").font(Theme.title(15))
                        Spacer()
                        if !categoryWasPicked {
                            Label("Suggérée d’après son nom", systemImage: "checkmark")
                                .font(Theme.body(12, weight: .heavy))
                                .foregroundStyle(appearance.palette.deep)
                        }
                    }
                    LazyVGrid(columns: columns, spacing: 8) {
                        ForEach(AgentCategory.allCases) { category in
                            categoryTile(category)
                        }
                    }
                    Text("Ou ta couleur").font(Theme.title(15)).padding(.top, 8)
                    HStack(spacing: 10) {
                        ForEach(AgentAppearance.customChoices, id: \.self) { hex in
                            Button {
                                appearance.customColorHex = appearance.customColorHex == hex ? nil : hex
                            } label: {
                                Circle()
                                    .fill(Color(hex: hex))
                                    .frame(width: 34, height: 34)
                                    .padding(3)
                                    .overlay(Circle().stroke(Theme.ink, lineWidth: appearance.customColorHex == hex ? 2 : 0))
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Couleur \(String(hex, radix: 16))")
                        }
                    }
                }
            }
            .padding(20)
        }
    }

    private func categoryTile(_ category: AgentCategory) -> some View {
        let selected = appearance.category == category
        return Button {
            appearance.category = category
            appearance.customColorHex = nil
            categoryWasPicked = true
        } label: {
            VStack(spacing: 2) {
                MascotView(appearance: AgentAppearance(category: category), animated: selected)
                    .frame(width: 54, height: 54)
                Text(category.label)
                    .font(Theme.body(12, weight: .heavy))
                    .foregroundStyle(category.palette.deep)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(category.palette.tint, in: .rect(cornerRadius: 20, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Theme.ink, lineWidth: selected ? 3 : 0))
        }
        .buttonStyle(.plain)
        .sensoryFeedback(.selection, trigger: selected)
    }
}
