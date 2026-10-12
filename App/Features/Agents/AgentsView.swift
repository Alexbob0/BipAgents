import HermesKit
import SwiftUI
import VoiceKit

struct AgentsView: View {
    @Environment(AgentStore.self) private var store
    @State private var isAddingAgent = false

    var body: some View {
        VStack(spacing: 0) {
            ScreenHeader(overline: Date.now.formatted(.dateTime.weekday(.wide).day().month(.wide)).capitalizedFirst, title: "Salut") {
                HeaderButton(systemImage: "plus", label: "Ajouter un agent") { isAddingAgent = true }
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if store.agents.isEmpty {
                        EmptyAgentsView { isAddingAgent = true }
                    } else {
                        if !store.lostAgents.isEmpty { LostServerNotice() }
                        if !store.serverNews.isEmpty { ServerNewsNotice() }
                        // Beyond two agents the cards go compact: about four fit on a screen.
                        let compact = store.agents.count > 2
                        ForEach(store.agents) { agent in
                            AgentCard(agent: agent, reachability: store.reachability[agent.id] ?? .unknown,
                                      latest: store.latestSession[agent.id], compact: compact)
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .padding(.bottom, 32)
            }
            .refreshable { await store.refreshAll() }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) { MiniPlayer() } // the audio playing, above the tab bar
        .background(Theme.background)
        .toolbar(.hidden, for: .navigationBar)
        .tabBarHidden(false)
        .sheet(isPresented: $isAddingAgent) { AddAgentFlow() }
    }

}

enum AgentRoute: Hashable {
    case sessions(AgentProfile)
    case conversation(AgentProfile, sessionID: String?, start: ConversationStart = .none)
    /// Live: hands-free voice conversation.
    case call(AgentProfile)
}

/// What a conversation does as soon as it opens (from the agent card's buttons).
enum ConversationStart: Hashable {
    case none
    /// « Écrire »: keyboard up.
    case keyboard
    /// « Audio »: a voice note is already recording (locked; ↑ sends, ✕ cancels).
    case voiceNote
}

struct AgentCard: View {
    var agent: AgentProfile
    var reachability: AgentReachability
    var latest: HermesSession?
    /// Smaller Bip, one-line preview, lower buttons (many agents).
    var compact = false

    @Environment(QuickVoiceCenter.self) private var quick
    @Environment(Router.self) private var router

    private var quickStatus: QuickVoiceCenter.Status? { quick.status[agent.id] }

    private var mood: MascotMood {
        switch quickStatus {
        case .recording?: .listening
        case .waiting?: .thinking
        default: reachability.mascotMood
        }
    }

    private var palette: AgentPalette { agent.appearance.palette }
    private var buttonHeight: CGFloat { compact ? 40 : 50 }

    /// The agent's « Bot Chat » (its permanent thread) reads as « Discussion ».
    private func threadTitle(_ session: HermesSession) -> String? {
        AgentStore.isBotChat(session) ? String(localized: "Discussion") : session.title
    }

    /// « 15 min », « 2 h », « 3 j »: how old the last message is, as short as possible.
    static func shortAge(of date: Date, now: Date = .now) -> String {
        let minutes = max(0, Int(now.timeIntervalSince(date) / 60))
        if minutes < 1 { return String(localized: "à l’instant") }
        if minutes < 60 { return String(localized: "\(minutes) min") }
        if minutes < 24 * 60 { return String(localized: "\(minutes / 60) h") }
        return String(localized: "\(minutes / (24 * 60)) j")
    }

    var body: some View {
        VStack(spacing: compact ? 8 : 12) {
            HStack(alignment: .center, spacing: 12) {
                NavigationLink(value: AgentRoute.sessions(agent)) {
                    Group {
                        if compact {
                            // One line: name, category, online dot.
                            HStack(spacing: 8) {
                                Text(agent.name)
                                    .font(Theme.display(20))
                                    .foregroundStyle(Theme.ink)
                                    .lineLimit(1)
                                CategoryChip(appearance: agent.appearance)
                                ReachabilityLabel(reachability: reachability, showsText: false)
                            }
                        } else {
                            VStack(alignment: .leading, spacing: 6) {
                                Text(agent.name)
                                    .font(Theme.display(25))
                                    .foregroundStyle(Theme.ink)
                                HStack(spacing: 8) {
                                    CategoryChip(appearance: agent.appearance)
                                    ReachabilityLabel(reachability: reachability)
                                }
                            }
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
                // Outside the link: tapping the Bip makes it react instead of opening the agent.
                InteractiveMascot(appearance: agent.appearance, baseMood: mood, size: compact ? 48 : 92, bubbleEdge: .leading)
                    .padding(.vertical, compact ? -10 : -22)
            }
            .zIndex(1) // the Bip stays above the preview below when dragged or jumping

            if let quickStatus {
                QuickVoiceStatusView(agent: agent, status: quickStatus, level: quick.voice.inputLevel) {
                    quick.clear(agent)
                    router.agentsPath.append(AgentRoute.conversation(agent, sessionID: latest?.id))
                } onDismiss: {
                    quick.clear(agent)
                }
                .transition(.opacity)
            } else if let latest, let preview = latest.lastMessagePreview.map(ChatText.preview) {
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: latest.id)) {
                    if compact {
                        // One line: « Title · latest message », date on the right.
                        HStack(spacing: 8) {
                            (Text(threadTitle(latest).map { "\($0) · " } ?? "").font(Theme.body(14, weight: .heavy)).foregroundStyle(Theme.muted)
                                + Text(preview).font(Theme.body(14)).foregroundStyle(Theme.ink))
                                .lineLimit(1)
                            Spacer(minLength: 0)
                            if let date = latest.updatedAt {
                                Text(Self.shortAge(of: date))
                                    .font(Theme.body(12, weight: .heavy))
                                    .foregroundStyle(Theme.muted)
                                    .fixedSize()
                            }
                        }
                        .padding(.horizontal, 14)
                        .frame(maxWidth: .infinity, minHeight: 38, alignment: .leading)
                        .background(Theme.hermes ? Theme.field : Theme.card, in: .rect(cornerRadius: 16, style: .continuous))
                    } else {
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(threadTitle(latest) ?? String(localized: "Dernière conversation"))
                            Spacer()
                            if let date = latest.updatedAt {
                                Text(date, format: .relative(presentation: .named))
                            }
                        }
                        .font(Theme.body(12.5, weight: .heavy))
                        .foregroundStyle(Theme.muted)
                        Text(preview)
                            .font(Theme.body(15))
                            .foregroundStyle(Theme.ink)
                            .lineLimit(2)
                            .multilineTextAlignment(.leading)
                    }
                    .padding(.horizontal, 14)
                    .padding(.vertical, 12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.hermes ? Theme.field : Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
                    }
                }
                .buttonStyle(.plain)
            }

            HStack(spacing: 8) {
                AudioHoldButton(agent: agent, height: buttonHeight) {
                    router.agentsPath.append(AgentRoute.conversation(agent, sessionID: latest?.id, start: .voiceNote))
                }
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: latest?.id, start: .keyboard)) {
                    Label("Écrire", systemImage: "keyboard")
                }
                .buttonStyle(.pill(.secondary, height: buttonHeight))
                NavigationLink(value: AgentRoute.call(agent)) {
                    Label("Live", systemImage: "waveform")
                }
                .buttonStyle(.pill(.primary, height: buttonHeight))
            }
            .labelStyle(.compactPill)
        }
        .padding(compact ? 12 : 18)
        .agentCard(palette)
        .animation(.snappy, value: quickStatus)
    }
}

extension AgentReachability {
    var mascotMood: MascotMood {
        switch self {
        case .online, .local, .unknown: .happy
        case .checking: .thinking
        case .unauthorized: .asking
        case .offline: .sleeping
        case .modelDown: .thinking
        }
    }
}

struct CategoryChip: View {
    var appearance: AgentAppearance
    var background: Color = Theme.card

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(appearance.palette.main).frame(width: 7, height: 7)
            Text(appearance.category.label)
        }
        .font(Theme.body(12, weight: .heavy))
        .foregroundStyle(appearance.palette.deep)
        .padding(.horizontal, 9)
        .frame(height: 24)
        .background(background, in: .capsule)
    }
}

struct ReachabilityLabel: View {
    var reachability: AgentReachability
    var showsText = true

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 7, height: 7)
            if showsText { Text(reachability.label) }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(reachability.label)
        .font(Theme.body(13, weight: .bold))
        .foregroundStyle(Theme.ink2)
    }

    private var color: Color {
        switch reachability {
        case .online, .local: Theme.online
        case .unauthorized, .offline: Theme.danger
        case .modelDown: Color.orange
        case .unknown, .checking: Theme.muted
        }
    }
}

/// Neither the tailnet nor the bridge's local door answers: the server may be off, or its local address changed
/// while the tailnet was down (the app learns new addresses on its own only through the tailnet).
struct LostServerNotice: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("Serveur injoignable", systemImage: "wifi.exclamationmark")
                .font(Theme.body(15, weight: .heavy))
                .foregroundStyle(Theme.danger)
            Text("Ni Tailscale ni le réseau local ne répondent. Si l’adresse du serveur a changé, affiche un nouveau QR code sur le serveur, puis scanne-le avec le bouton +.")
                .font(Theme.body(14))
                .foregroundStyle(Theme.ink2)
            Text(verbatim: "python -m bipbridge qr")
                .font(.system(size: 13, design: .monospaced))
                .foregroundStyle(Theme.ink)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(Theme.background, in: .rect(cornerRadius: 8))
                .textSelection(.enabled)
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card, in: .rect(cornerRadius: 22, style: .continuous))
    }
}

/// What changed on this iPhone's servers: agents to add (created from another phone or on the server), profiles to
/// connect, agents the server no longer has. One tap each; ✕ hides it for good.
struct ServerNewsNotice: View {
    @Environment(AgentStore.self) private var store
    @State private var working: String?
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("Du nouveau sur ton serveur", systemImage: "sparkles")
                .font(Theme.body(15, weight: .heavy))
                .foregroundStyle(Theme.ink)
            ForEach(store.serverNews) { news in
                row(news)
            }
            if let failure {
                Text(failure).font(Theme.body(13)).foregroundStyle(Theme.danger)
            }
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card, in: .rect(cornerRadius: 22, style: .continuous))
    }

    private func row(_ news: AgentStore.ServerNews) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title(news)).font(Theme.body(15, weight: .heavy)).foregroundStyle(Theme.ink)
                Text(detail(news)).font(Theme.body(13)).foregroundStyle(Theme.ink2)
            }
            Spacer(minLength: 0)
            if working == news.id {
                ProgressView()
            } else {
                Button(action: { Task { await accept(news) } }) {
                    Text(action(news))
                }
                .buttonStyle(.pill(isGone(news) ? .soft : .primary, height: 34))
                .disabled(working != nil)
                Button("Plus tard", systemImage: "xmark") { store.dismiss(news) }
                    .labelStyle(.iconOnly)
                    .font(Theme.body(13, weight: .heavy))
                    .foregroundStyle(Theme.muted)
                    .disabled(working != nil)
            }
        }
    }

    private func isGone(_ news: AgentStore.ServerNews) -> Bool {
        if case .gone = news.kind { return true }
        return false
    }

    private func title(_ news: AgentStore.ServerNews) -> String {
        switch news.kind {
        case .added(let agent): agent.name
        case .unclaimed(let profile): profile.name
        case .gone(let agent): agent.name
        }
    }

    private func detail(_ news: AgentStore.ServerNews) -> String {
        switch news.kind {
        case .added: String(localized: "Ajouté sur ton serveur, pas encore sur cet iPhone")
        case .unclaimed(let profile):
            profile.description.isEmpty ? String(localized: "Créé sur le serveur, pas encore relié à l’app")
                                        : profile.description
        case .gone: String(localized: "N’existe plus sur ton serveur")
        }
    }

    private func action(_ news: AgentStore.ServerNews) -> LocalizedStringKey {
        switch news.kind {
        case .added: "Ajouter"
        case .unclaimed: "Relier"
        case .gone: "Retirer"
        }
    }

    private func accept(_ news: AgentStore.ServerNews) async {
        working = news.id
        failure = nil
        defer { working = nil }
        do {
            try await store.accept(news)
        } catch {
            failure = ConversationModel.describe(error)
        }
    }
}

struct EmptyAgentsView: View {
    var add: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            InteractiveMascot(appearance: AgentAppearance(category: .tech), baseMood: .sleeping, size: 120)
            Text("Aucun agent pour l’instant")
                .font(Theme.title(20))
            Text("Scanne le QR code affiché par ton serveur Hermes, ou saisis son adresse et sa clé.")
                .font(Theme.body(15))
                .foregroundStyle(Theme.ink2)
                .multilineTextAlignment(.center)
            Button("Ajouter un agent", systemImage: "qrcode.viewfinder", action: add)
                .buttonStyle(.pill)
        }
        .padding(24)
        .frame(maxWidth: .infinity)
        .card(radius: 30)
        .padding(.top, 24)
    }
}

#Preview {
    NavigationStack { AgentsView() }
        .environment(AgentStore.preview)
}

private extension View {
    /// The Bips' agent cards are tinted with the agent's color; Hermes' are flat, with a hairline: the color stays on
    /// the character.
    @ViewBuilder
    func agentCard(_ palette: AgentPalette) -> some View {
        if Theme.hermes {
            card(radius: 30)
        } else {
            background(palette.tint, in: .rect(cornerRadius: 30, style: .continuous))
        }
    }
}
