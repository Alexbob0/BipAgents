import HermesKit
import SwiftUI

struct SessionsView: View {
    let agent: AgentProfile

    @Environment(AgentStore.self) private var store
    @State private var sessions: [HermesSession] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var renaming: HermesSession?
    @State private var newTitle = ""

    private var palette: AgentPalette { agent.appearance.palette }

    var body: some View {
        List {
            Section {
                header
                    .listRowInsets(EdgeInsets())
                    .listRowBackground(Color.clear)
            }
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }
                    .listRowBackground(Color.clear)
            }
            Section {
                ForEach(sessions) { session in
                    NavigationLink(value: AgentRoute.conversation(agent, sessionID: session.id)) {
                        SessionRow(session: session)
                    }
                    .swipeActions {
                        Button("Supprimer", systemImage: "trash", role: .destructive) { delete(session) }
                        Button("Renommer", systemImage: "pencil") {
                            newTitle = session.title ?? ""
                            renaming = session
                        }
                    }
                }
            } header: {
                if !sessions.isEmpty { Text("Conversations").font(Theme.body(13, weight: .heavy)) }
            }
        }
        .scrollContentBackground(.hidden)
        .background(alignment: .top) {
            palette.tint
                .frame(height: 420)
                .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: 36, bottomTrailingRadius: 36))
                .ignoresSafeArea()
        }
        .background(Theme.background)
        .refreshable { await load() }
        .task { await load() }
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: nil)) {
                    Image(systemName: "square.and.pencil")
                }
                .accessibilityLabel("Nouvelle conversation")
            }
        }
        .alert("Renommer", isPresented: Binding(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
            TextField("Titre", text: $newTitle)
            Button("Annuler", role: .cancel) {}
            Button("OK") { if let renaming { rename(renaming, to: newTitle) } }
        }
    }

    private var header: some View {
        VStack(spacing: 6) {
            MascotView(appearance: agent.appearance, mood: isLoading ? .thinking : (store.reachability[agent.id] ?? .unknown).mascotMood)
                .frame(width: 120, height: 120)
            Text(agent.name).font(Theme.display(30))
            HStack(spacing: 8) {
                CategoryChip(appearance: agent.appearance)
                ReachabilityLabel(reachability: store.reachability[agent.id] ?? .unknown)
            }
            HStack(spacing: 10) {
                NavigationLink(value: AgentRoute.call(agent)) { Label("Appeler", systemImage: "phone.fill") }
                    .buttonStyle(.pill(.primary, height: 46))
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: nil)) { Label("Nouvelle", systemImage: "square.and.pencil") }
                    .buttonStyle(.pill(.secondary, height: 46))
            }
            .padding(.top, 10)
            .padding(.horizontal, 40)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 12)
    }

    private func load() async {
        guard let client = store.client(for: agent) else {
            errorMessage = "Clé d’accès introuvable."
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            sessions = try await client.listSessions(limit: 50)
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func delete(_ session: HermesSession) {
        guard let client = store.client(for: agent) else { return }
        sessions.removeAll { $0.id == session.id }
        Task {
            do { try await client.deleteSession(id: session.id) } catch { await load() }
        }
    }

    private func rename(_ session: HermesSession, to title: String) {
        guard let client = store.client(for: agent), !title.isEmpty else { return }
        if let index = sessions.firstIndex(where: { $0.id == session.id }) { sessions[index].title = title }
        Task { try? await client.renameSession(id: session.id, title: title) }
    }
}

struct SessionRow: View {
    var session: HermesSession

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(session.title ?? "Sans titre")
                    .font(Theme.body(16, weight: .heavy))
                    .lineLimit(1)
                Spacer()
                if let date = session.updatedAt ?? session.createdAt {
                    Text(date, format: .relative(presentation: .named))
                        .font(Theme.body(13, weight: .bold))
                        .foregroundStyle(Theme.muted)
                }
            }
            if let preview = session.lastMessagePreview {
                Text(preview)
                    .font(Theme.body(14))
                    .foregroundStyle(Theme.ink2)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 4)
    }
}

struct CallPlaceholderView: View {
    let agent: AgentProfile

    var body: some View {
        VStack(spacing: 18) {
            Spacer()
            MascotView(appearance: agent.appearance, mood: .listening)
                .frame(width: 200, height: 200)
            Text("Le mode appel arrive au jalon 2")
                .font(Theme.title(20))
            Text("Dictée sur l’iPhone, voix Kyutai et interruption à la voix.")
                .font(Theme.body(15))
                .foregroundStyle(Theme.ink2)
                .multilineTextAlignment(.center)
            Spacer()
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(agent.appearance.palette.tint)
    }
}
