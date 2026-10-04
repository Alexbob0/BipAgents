import HermesKit
import SwiftUI

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
                        ForEach(store.agents) { agent in
                            AgentCard(agent: agent, reachability: store.reachability[agent.id] ?? .unknown, latest: store.latestSession[agent.id])
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .padding(.bottom, 32)
            }
            .refreshable { await store.refreshAll() }
        }
        .background(Theme.background)
        .toolbar(.hidden, for: .navigationBar)
        .sheet(isPresented: $isAddingAgent) { AddAgentFlow() }
    }

}

enum AgentRoute: Hashable {
    case sessions(AgentProfile)
    case conversation(AgentProfile, sessionID: String?)
    case call(AgentProfile)
}

struct AgentCard: View {
    var agent: AgentProfile
    var reachability: AgentReachability
    var latest: HermesSession?

    private var palette: AgentPalette { agent.appearance.palette }

    var body: some View {
        VStack(spacing: 12) {
            NavigationLink(value: AgentRoute.sessions(agent)) {
                HStack(alignment: .center, spacing: 12) {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(agent.name)
                            .font(Theme.display(25))
                            .foregroundStyle(Theme.ink)
                        HStack(spacing: 8) {
                            CategoryChip(appearance: agent.appearance)
                            ReachabilityLabel(reachability: reachability)
                        }
                    }
                    Spacer(minLength: 0)
                    MascotView(appearance: agent.appearance, mood: reachability.mascotMood)
                        .frame(width: 92, height: 92)
                        .padding(.vertical, -22)
                }
            }
            .buttonStyle(.plain)

            if let latest, let preview = latest.lastMessagePreview {
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: latest.id)) {
                    VStack(alignment: .leading, spacing: 3) {
                        HStack {
                            Text(latest.title ?? "Dernière conversation")
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
                    .background(Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
                }
                .buttonStyle(.plain)
            }

            HStack(spacing: 10) {
                NavigationLink(value: AgentRoute.call(agent)) {
                    Label("Parler", systemImage: "mic.fill")
                }
                .buttonStyle(.pill)
                NavigationLink(value: AgentRoute.conversation(agent, sessionID: latest?.id)) {
                    Label("Écrire", systemImage: "keyboard")
                }
                .buttonStyle(.pill(.secondary))
            }
        }
        .padding(18)
        .background(palette.tint, in: .rect(cornerRadius: 30, style: .continuous))
    }
}

extension AgentReachability {
    var mascotMood: MascotMood {
        switch self {
        case .online, .unknown: .happy
        case .checking: .thinking
        case .unauthorized: .asking
        case .offline: .sleeping
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

    var body: some View {
        HStack(spacing: 5) {
            Circle().fill(color).frame(width: 7, height: 7)
            Text(reachability.label)
        }
        .font(Theme.body(13, weight: .bold))
        .foregroundStyle(Theme.ink2)
    }

    private var color: Color {
        switch reachability {
        case .online: Theme.online
        case .unauthorized, .offline: Theme.danger
        case .unknown, .checking: Theme.muted
        }
    }
}

struct EmptyAgentsView: View {
    var add: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            MascotView(appearance: AgentAppearance(category: .tech), mood: .sleeping)
                .frame(width: 120, height: 120)
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
