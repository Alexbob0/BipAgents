import CoreImage.CIFilterBuiltins
import HermesKit
import SwiftUI

/// « Famille » : this install as a household, linked to a relative's (their own BipAgents install). Once linked, the
/// other household's agents show in « Qui peut écrire à qui », where each person turns on, at home, what they allow.
struct FamilyView: View {
    @Environment(AgentStore.self) private var store
    @State private var bridge: BridgeClient?
    @State private var family: BridgeClient.Family?
    @State private var name = ""
    @State private var invitation: String?
    @State private var isScanning = false
    @State private var isWorking = false
    @State private var isLoading = true
    @State private var message: String?
    @State private var errorMessage: String?

    var body: some View {
        List {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            if let message {
                Section { NoticeRow(text: message, isError: false) }.listRowBackground(Color.clear)
            }
            if family != nil {
                Section {
                    TextField("Ton prénom", text: $name)
                        .textContentType(.givenName)
                        .submitLabel(.done)
                        .onSubmit { Task { await saveName() } }
                } header: {
                    Text("Ton foyer")
                } footer: {
                    Text("Le nom que verront tes proches à côté de tes agents, par exemple « Vie (\(name.isEmpty ? String(localized: "Alex") : name)) ».")
                }
                Section {
                    if family?.households.isEmpty ?? true {
                        Text("Aucun proche relié pour l’instant.").foregroundStyle(Theme.ink2)
                    }
                    ForEach(family?.households ?? []) { household in
                        Label(household.name, systemImage: "house")
                    }
                    .onDelete { offsets in
                        let ids = offsets.compactMap { family?.households[$0].id }
                        Task { for id in ids { await unlink(id) } }
                    }
                } header: {
                    Text("Proches reliés")
                }
                Section {
                    Button("Inviter un proche", systemImage: "qrcode") { Task { await invite() } }
                        .disabled(isWorking || name.trimmingCharacters(in: .whitespaces).isEmpty)
                    if QRScanner.isAvailable {
                        Button("Scanner son invitation", systemImage: "qrcode.viewfinder") { isScanning = true }
                            .disabled(isWorking || name.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                } footer: {
                    Text("Chacun a sa propre installation. L’un montre son invitation, l’autre la scanne avec son iPhone. Ensuite, dans « Qui peut écrire à qui », chacun coche chez soi ce qu’il accepte : un message ne passe que si vous l’avez tous les deux autorisé.")
                }
            }
        }
        .navigationTitle("Famille")
        .overlay { if isLoading { ProgressView() } }
        .task { await load() }
        .refreshable { await load() }
        .sheet(isPresented: Binding(get: { invitation != nil }, set: { if !$0 { invitation = nil } })) {
            if let invitation { InvitationSheet(payload: invitation, name: name) }
        }
        .fullScreenCover(isPresented: $isScanning) {
            ZStack(alignment: .top) {
                QRScanner { payload in
                    isScanning = false
                    if let found = FamilyInvitation(qrPayload: payload) {
                        Task { await join(found) }
                    } else {
                        errorMessage = String(localized: "Ce n’est pas une invitation de famille BipAgents.")
                    }
                }
                .ignoresSafeArea()
                HStack {
                    Button { isScanning = false } label: {
                        Image(systemName: "xmark").font(.system(size: 18, weight: .bold)).foregroundStyle(.white)
                            .frame(width: 44, height: 44).background(.black.opacity(0.4), in: .circle)
                    }
                    .accessibilityLabel("Fermer")
                    Spacer()
                    Text("Scanne l’invitation de ton proche").font(Theme.title(16)).foregroundStyle(.white)
                    Spacer()
                    Color.clear.frame(width: 44, height: 44)
                }
                .padding()
            }
        }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        var lastError: (any Error)?
        for agent in store.agents {
            guard let candidate = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            do {
                guard let found = try await withTimeout(10, { try await candidate.family() }) else { continue }
                bridge = candidate
                family = found
                if name.isEmpty { name = found.name }
                errorMessage = nil
                return
            } catch {
                lastError = error
            }
        }
        errorMessage = lastError.map { ConversationModel.describe($0) }
            ?? String(localized: "Aucun de tes serveurs ne relie encore les familles (bridge à mettre à jour).")
    }

    private func saveName() async {
        guard let bridge, !name.trimmingCharacters(in: .whitespaces).isEmpty else { return }
        do {
            try await bridge.setFamilyName(name)
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func invite() async {
        guard let bridge else { return }
        isWorking = true
        defer { isWorking = false }
        await saveName()
        do {
            invitation = try await bridge.familyInvitation()
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func join(_ found: FamilyInvitation) async {
        guard let bridge else { return }
        isWorking = true
        defer { isWorking = false }
        await saveName()
        do {
            let other = try await bridge.joinFamily(found)
            message = String(localized: "Relié au foyer de \(other). Ses agents sont dans « Qui peut écrire à qui ».")
            errorMessage = nil
            await load()
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func unlink(_ id: String) async {
        guard let bridge else { return }
        do {
            try await bridge.unlinkHousehold(id)
            await load()
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }
}

/// The invitation's QR code, for the relative's iPhone to scan. Valid ten minutes, once.
private struct InvitationSheet: View {
    var payload: String
    var name: String
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            VStack(spacing: 20) {
                if let image = Self.qrCode(payload) {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 280)
                        .padding(16)
                        .background(.white, in: .rect(cornerRadius: 20))
                }
                Text("Sur l’iPhone de ton proche : Réglages › Famille › Scanner son invitation.")
                    .font(Theme.body(16))
                    .multilineTextAlignment(.center)
                Text("Valable 10 minutes, une seule fois.")
                    .font(Theme.body(14))
                    .foregroundStyle(Theme.ink2)
            }
            .padding(24)
            .navigationTitle("Invitation de \(name)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("OK") { dismiss() } } }
        }
    }

    static func qrCode(_ text: String) -> UIImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 10, y: 10)),
              let cgImage = CIContext().createCGImage(output, from: output.extent) else { return nil }
        return UIImage(cgImage: cgImage)
    }
}
