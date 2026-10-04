import SwiftUI

@main
struct BipAgentsApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    /// `-demo` launch argument: two sample agents, no network, for screenshots and design review.
    @State private var agents = ProcessInfo.processInfo.arguments.contains("-demo") ? AgentStore.preview : AgentStore()
    @State private var router = Router()
    @State private var inbox = InboxStore()
    @State private var quickVoice = QuickVoiceCenter()

    var body: some Scene {
        WindowGroup {
            Group {
                // `-demo -screen conversation|call` opens straight on that screen (design review, screenshots).
                if let screen = UserDefaults.standard.string(forKey: "screen"), !screen.isEmpty, let agent = agents.agents.first {
                    NavigationStack {
                        switch screen {
                        case "call": CallView(agent: agent)
                        default: ConversationView(agent: agent, sessionID: nil)
                        }
                    }
                } else {
                    RootView()
                }
            }
            .environment(agents)
            .environment(router)
            .environment(inbox)
            .environment(quickVoice)
            .tint(Theme.ink)
            .fontDesign(.rounded)
            .task {
                appDelegate.store = agents
                appDelegate.router = router
                ConversationModel.onMissedReply = { [inbox] agent, sessionID, runID, text in
                    inbox.addMissedReply(agent: agent, sessionID: sessionID, runID: runID, text: text)
                }
                if !agents.isDemo { await appDelegate.enablePushIfPossible() }
            }
        }
    }
}

struct RootView: View {
    @Environment(AgentStore.self) private var agents
    @Environment(Router.self) private var router
    @Environment(InboxStore.self) private var inbox
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var router = router
        TabView(selection: $router.tab) {
            Tab("Agents", systemImage: "circle.grid.2x2.fill", value: AppTab.agents) {
                NavigationStack(path: $router.agentsPath) {
                    AgentsView()
                        .navigationDestination(for: AgentRoute.self) { route in
                            switch route {
                            case .sessions(let agent): SessionsView(agent: agent)
                            case .conversation(let agent, let sessionID, let start): ConversationView(agent: agent, sessionID: sessionID, start: start)
                            case .call(let agent): CallView(agent: agent, sessionID: agents.mainSessionID(for: agent))
                            }
                        }
                }
            }
            Tab("Boîte", systemImage: "tray.fill", value: AppTab.inbox) {
                NavigationStack { InboxView() }
            }
            .badge(inbox.unreadCount)
            Tab("Réglages", systemImage: "slider.horizontal.3", value: AppTab.settings) {
                NavigationStack { SettingsView() }
            }
        }
        .task(id: scenePhase) {
            guard scenePhase == .active else { return }
            await agents.refreshAll()
            await inbox.refresh(agents: agents)
        }
    }
}
