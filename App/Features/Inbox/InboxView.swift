import AVFoundation
import Observation
import SwiftUI

/// Proactive messages from every agent's bridge outbox, newest first.
@Observable
final class InboxStore {
    struct Entry: Identifiable, Hashable {
        var item: OutboxItem
        var agent: AgentProfile
        var id: String { "\(agent.id)/\(item.id)" }
    }

    /// Replies an agent finished while their conversation was closed (see `ConversationModel.onMissedReply`).
    struct MissedReply: Codable, Hashable {
        var runID: String
        var agent: AgentProfile
        var sessionID: String
        var text: String
        var createdAt: Date
    }

    private var outbox: [Entry] = []
    private var missed: [MissedReply] = InboxStore.loadMissed()
    private static let missedKey = "inbox.missedReplies"

    /// Agent messages (bridge outbox) and missed replies, newest first.
    var entries: [Entry] {
        let replies = missed.map { reply in
            Entry(item: OutboxItem(id: "reply-\(reply.runID)", agent: reply.agent.bridgeName, title: "Réponse",
                                   text: reply.text, createdAt: reply.createdAt, sessionID: reply.sessionID, hasAudio: false),
                  agent: reply.agent)
        }
        return (outbox + replies).sorted { $0.item.createdAt > $1.item.createdAt }
    }
    private(set) var isLoading = false
    private(set) var errorMessage: String?
    private(set) var playingID: String?

    private var player: AVAudioPlayer?
    private let lastSeenKey = "inbox.lastSeen"

    var lastSeen: Date {
        get { UserDefaults.standard.object(forKey: lastSeenKey) as? Date ?? .distantPast }
        set { UserDefaults.standard.set(newValue, forKey: lastSeenKey) }
    }

    var unreadCount: Int { entries.filter { $0.item.createdAt > lastSeen }.count }

    func refresh(agents store: AgentStore) async {
        if store.isDemo {
            outbox = Self.demo(store)
            return
        }
        isLoading = true
        defer { isLoading = false }
        var collected: [Entry] = []
        var failures = 0
        for agent in store.agents {
            guard let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            do {
                let items = try await bridge.outbox(agent: agent.bridgeName)
                collected += items.map { Entry(item: $0, agent: agent) }
            } catch {
                failures += 1
            }
        }
        outbox = collected
        errorMessage = failures > 0 ? "Certains agents sont injoignables (Tailscale ?)." : nil
    }

    func markAllSeen() {
        lastSeen = .now
    }

    func addMissedReply(agent: AgentProfile, sessionID: String, runID: String, text: String) {
        guard !missed.contains(where: { $0.runID == runID }) else { return }
        missed.append(MissedReply(runID: runID, agent: agent, sessionID: sessionID, text: text, createdAt: .now))
        missed = Array(missed.suffix(30))
        saveMissed()
    }

    /// The conversation was opened: its missed replies are read there.
    func dismissMissedReplies(sessionID: String) {
        guard missed.contains(where: { $0.sessionID == sessionID }) else { return }
        missed.removeAll { $0.sessionID == sessionID }
        saveMissed()
    }

    private func saveMissed() {
        UserDefaults.standard.set(try? JSONEncoder().encode(missed), forKey: Self.missedKey)
    }

    private static func loadMissed() -> [MissedReply] {
        guard let data = UserDefaults.standard.data(forKey: missedKey) else { return [] }
        return (try? JSONDecoder().decode([MissedReply].self, from: data)) ?? []
    }

    func togglePlayback(_ entry: Entry, store: AgentStore) async {
        if playingID == entry.id {
            player?.stop()
            playingID = nil
            return
        }
        guard let bridge = BridgeClient(agent: entry.agent, secrets: store.secrets(for: entry.agent)),
              let data = try? await bridge.audio(forOutboxItem: entry.item.id) else { return }
        do {
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .spokenAudio)
            try AVAudioSession.sharedInstance().setActive(true)
            player = try AVAudioPlayer(data: data)
            player?.play()
            playingID = entry.id
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private static func demo(_ store: AgentStore) -> [Entry] {
        guard let first = store.agents.first else { return [] }
        return [Entry(item: OutboxItem(id: "1", agent: first.bridgeName, title: "Plan sommeil du soir",
                                       text: "Ce soir, vise un coucher à 23 h 15 : écrans coupés à 22 h 30, lumière tamisée et chambre à 18 °C.",
                                       createdAt: .now.addingTimeInterval(-600), sessionID: nil, hasAudio: true), agent: first)]
    }
}

struct InboxView: View {
    @Environment(InboxStore.self) private var inbox
    @Environment(AgentStore.self) private var agents
    @Environment(Router.self) private var router
    @State private var filter: UUID?

    private var visible: [InboxStore.Entry] {
        inbox.entries.filter { filter == nil || $0.agent.id == filter }
    }

    var body: some View {
        VStack(spacing: 0) {
            ScreenHeader(overline: "Messages de tes agents", title: "Boîte")
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    filters
                    if let error = inbox.errorMessage { NoticeRow(text: error, isError: true) }
                    if visible.isEmpty && !inbox.isLoading {
                        emptyState
                    }
                    ForEach(visible) { entry in
                        InboxCard(entry: entry, isPlaying: inbox.playingID == entry.id, isUnread: entry.item.createdAt > inbox.lastSeen) {
                            Task { await inbox.togglePlayback(entry, store: agents) }
                        } reply: {
                            router.open(.conversation(entry.agent, sessionID: entry.item.sessionID))
                        } replyByVoice: {
                            router.open(.call(entry.agent))
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 4)
                .padding(.bottom, 24)
            }
            .refreshable { await inbox.refresh(agents: agents) }
        }
        .background(Theme.background)
        .toolbar(.hidden, for: .navigationBar)
        .onDisappear { inbox.markAllSeen() }
    }

    private var filters: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                chip("Tout", color: nil, selected: filter == nil) { filter = nil }
                ForEach(agents.agents) { agent in
                    chip(agent.name, color: agent.appearance.palette.main, selected: filter == agent.id) { filter = agent.id }
                }
            }
        }
    }

    private func chip(_ title: String, color: Color?, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if let color { Circle().fill(color).frame(width: 8, height: 8) }
                Text(title)
            }
            .font(Theme.body(14, weight: .heavy))
            .foregroundStyle(selected ? Theme.onInk : Theme.ink)
            .padding(.horizontal, 15)
            .frame(height: 36)
            .background(selected ? Theme.ink : Theme.card, in: .capsule)
        }
        .buttonStyle(.plain)
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            InteractiveMascot(appearance: AgentAppearance(category: .daily), baseMood: .sleeping, size: 120)
            Text("Rien de neuf").font(Theme.title(20))
            Text("Les check-ins et rappels de tes agents arriveront ici, avec leur version audio, ainsi que les réponses terminées pendant que tu étais ailleurs.")
                .font(Theme.body(15))
                .foregroundStyle(Theme.ink2)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 60)
    }
}

struct InboxCard: View {
    var entry: InboxStore.Entry
    var isPlaying: Bool
    var isUnread: Bool
    var play: () -> Void
    var reply: () -> Void
    var replyByVoice: () -> Void

    private var palette: AgentPalette { entry.agent.appearance.palette }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                MascotView(appearance: entry.agent.appearance, animated: false)
                    .padding(3)
                    .frame(width: 38, height: 38)
                    .background(palette.tint, in: .rect(cornerRadius: 14, style: .continuous))
                VStack(alignment: .leading, spacing: 0) {
                    Text(entry.agent.name).font(Theme.body(14, weight: .black))
                    HStack(spacing: 4) {
                        Text(entry.item.createdAt, format: .dateTime.hour().minute())
                        if let title = entry.item.title { Text("· \(title)") }
                    }
                    .font(Theme.body(12.5, weight: .bold))
                    .foregroundStyle(Theme.muted)
                    .lineLimit(1)
                }
                Spacer()
                if isUnread { Circle().fill(Theme.danger).frame(width: 10, height: 10) }
            }
            Text(entry.item.text)
                .font(Theme.body(15.5))
                .foregroundStyle(Theme.ink)
                .lineLimit(8)
            if entry.item.hasAudio {
                Button(action: play) {
                    HStack(spacing: 10) {
                        Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                            .font(.system(size: 14, weight: .bold))
                            .foregroundStyle(.white)
                            .frame(width: 34, height: 34)
                            .background(palette.deep, in: .circle)
                        Text(isPlaying ? "Lecture…" : "Écouter")
                            .font(Theme.body(14, weight: .heavy))
                            .foregroundStyle(palette.deep)
                        Spacer()
                    }
                    .padding(8)
                    .background(palette.tint, in: .rect(cornerRadius: 18, style: .continuous))
                }
                .buttonStyle(.plain)
            }
            HStack(spacing: 8) {
                Button(action: reply) { Label("Répondre", systemImage: "arrowshape.turn.up.left.fill") }
                    .buttonStyle(.pill(.primary, height: 42))
                Button(action: replyByVoice) { Label("Live", systemImage: "waveform") }
                    .buttonStyle(.pill(.soft, height: 42))
            }
        }
        .padding(16)
        .card(radius: 26)
    }
}
