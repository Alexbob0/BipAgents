import HermesKit
import SwiftUI

struct SessionsView: View {
    let agent: AgentProfile

    @Environment(AgentStore.self) private var store
    @Environment(Router.self) private var router
    @State private var sessions: [HermesSession] = []
    @State private var isLoading = false
    @State private var errorMessage: String?
    @State private var renaming: HermesSession?
    @State private var newTitle = ""
    /// Sessions read so far: more pages come as the list is scrolled to its end.
    @State private var nextOffset = 0
    @State private var reachedEnd = true
    @State private var isLoadingMore = false
    @State private var showsAllTasks = false

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
            // Bot Mode: the agent's permanent « Bot Chat », pinned above everything else (never deleted here).
            if let botChat = sessions.first(where: AgentStore.isBotChat) {
                Section {
                    NavigationLink(value: AgentRoute.conversation(agent, sessionID: botChat.id)) {
                        PinnedThreadRow(session: botChat, preview: store.latestSession[agent.id]?.id == botChat.id
                                            ? store.latestSession[agent.id]?.lastMessagePreview : nil,
                                        palette: palette)
                    }
                }
            }
            sessionSection("Conversations", Self.newestFirst(sessions.filter { !AgentStore.isBotChat($0) && !AgentStore.isScheduledTask($0) }))
            let tasks = Self.newestFirst(sessions.filter(AgentStore.isScheduledTask))
            sessionSection("Tâches planifiées", showsAllTasks ? tasks : Array(tasks.prefix(5)))
            if tasks.count > 5 && !showsAllTasks {
                Button("Afficher les \(tasks.count - 5) autres") { withAnimation { showsAllTasks = true } }
                    .font(Theme.body(14, weight: .bold))
                    .foregroundStyle(palette.deep)
                    .listRowBackground(Color.clear)
            }
            if !reachedEnd {
                // The end of what was read: the next page loads when it comes into view.
                HStack { Spacer(); ProgressView(); Spacer() }
                    .listRowBackground(Color.clear)
                    .task(id: nextOffset) { await loadMore() }  // again after each page while still in view
            }
        }
        .scrollContentBackground(.hidden)
        .background(alignment: .top) {
            palette.tint
                .frame(height: 480)
                .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: 36, bottomTrailingRadius: 36))
                .ignoresSafeArea()
        }
        .background(Theme.background)
        .tabBarHidden(false)
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

    @ViewBuilder
    private func sessionSection(_ title: LocalizedStringKey, _ rows: [HermesSession]) -> some View {
        if !rows.isEmpty {
            Section {
                ForEach(rows) { session in
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
                Text(title).font(Theme.body(13, weight: .heavy))
            }
        }
    }

    private var header: some View {
        VStack(spacing: 6) {
            InteractiveMascot(appearance: agent.appearance,
                              baseMood: isLoading ? .thinking : (store.reachability[agent.id] ?? .unknown).mascotMood, size: 120)
            Text(agent.name).font(Theme.display(30))
            HStack(spacing: 8) {
                CategoryChip(appearance: agent.appearance)
                ReachabilityLabel(reachability: store.reachability[agent.id] ?? .unknown)
            }
            // Buttons, not NavigationLinks: two links in one List row act as a single cell and a tap
            // pushed both (the call ended up hidden under the new conversation).
            HStack(spacing: 10) {
                Button { router.agentsPath.append(AgentRoute.call(agent)) } label: { Label("Live", systemImage: "waveform") }
                    .buttonStyle(.pill(.primary, height: 46))
                Button { router.agentsPath.append(AgentRoute.conversation(agent, sessionID: nil)) } label: {
                    Label("Nouvelle", systemImage: "square.and.pencil")
                }
                .buttonStyle(.pill(.secondary, height: 46))
            }
            .padding(.top, 10)
            .padding(.horizontal, 40)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 52) // room for the Bip's speech bubble (the list row clips above it)
        .padding(.bottom, 12)
    }

    private func load() async {
        guard let client = store.client(for: agent) else {
            errorMessage = String(localized: "Clé d’accès introuvable.")
            return
        }
        isLoading = true
        defer { isLoading = false }
        do {
            // Enough pages for a screenful of conversations, even after many scheduled-task runs.
            let result = try await AgentStore.sessions(from: client) { read in
                read.filter { !AgentStore.isScheduledTask($0) && !AgentStore.isBotChat($0) }.count >= 20
            }
            sessions = result.sessions
            nextOffset = result.sessions.count
            reachedEnd = result.complete
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func loadMore() async {
        guard !reachedEnd, !isLoadingMore, let client = store.client(for: agent) else { return }
        isLoadingMore = true
        defer { isLoadingMore = false }
        guard let result = try? await AgentStore.sessions(from: client, offset: nextOffset, maxPages: 1) else { return }
        sessions += result.sessions.filter { session in !sessions.contains { $0.id == session.id } }
        nextOffset += result.sessions.count
        reachedEnd = result.complete
    }

    /// Most recent activity first; the server's order breaks ties (and stands when dates are missing).
    private static func newestFirst(_ sessions: [HermesSession]) -> [HermesSession] {
        sessions.enumerated().sorted { a, b in
            let da = a.element.updatedAt ?? a.element.createdAt, db = b.element.updatedAt ?? b.element.createdAt
            if let da, let db, da != db { return da > db }
            return a.offset < b.offset
        }.map(\.element)
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
                Text(session.title ?? String(localized: "Sans titre"))
                    .font(Theme.body(16, weight: .heavy))
                    .lineLimit(1)
                Spacer()
                if let date = session.updatedAt ?? session.createdAt {
                    Text(date, format: .relative(presentation: .named))
                        .font(Theme.body(13, weight: .bold))
                        .foregroundStyle(Theme.muted)
                }
            }
            if let preview = session.lastMessagePreview.map(ChatText.preview) {
                Text(preview)
                    .font(Theme.body(14))
                    .foregroundStyle(Theme.ink2)
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 4)
    }
}

/// The agent's permanent thread (Bot Mode « Bot Chat »), pinned on top of its conversations.
struct PinnedThreadRow: View {
    var session: HermesSession
    var preview: String?
    var palette: AgentPalette

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "pin.fill")
                .font(.system(size: 14, weight: .bold))
                .foregroundStyle(palette.deep)
                .frame(width: 34, height: 34)
                .background(palette.tint, in: .rect(cornerRadius: 11, style: .continuous))
            VStack(alignment: .leading, spacing: 3) {
                HStack {
                    Text("Discussion").font(Theme.body(16, weight: .heavy))
                    Spacer()
                    if let date = session.updatedAt ?? session.createdAt {
                        Text(date, format: .relative(presentation: .named))
                            .font(Theme.body(13, weight: .bold))
                            .foregroundStyle(Theme.muted)
                    }
                }
                Text(preview.map(ChatText.preview) ?? String(localized: "Le fil permanent de l’agent"))
                    .font(Theme.body(14))
                    .foregroundStyle(Theme.ink2)
                    .lineLimit(2)
            }
        }
        .padding(.vertical, 4)
    }
}
