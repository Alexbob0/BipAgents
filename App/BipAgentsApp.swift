import SwiftUI

@main
struct BipAgentsApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    /// `-demo` launch argument: two sample agents, no network, for screenshots and design review.
    @State private var agents = ProcessInfo.processInfo.arguments.contains("-demo") ? AgentStore.preview : AgentStore()
    @State private var router = Router()
    @State private var inbox = InboxStore()
    @State private var quickVoice = QuickVoiceCenter()
    @Environment(\.scenePhase) private var scenePhase

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
            .onChange(of: agents.agents, initial: true) { _, profiles in AgentAvatars.export(profiles) }
            .onChange(of: scenePhase) { _, phase in
                if phase == .background { ConversationModel.detachAll() }
            }
            .task {
                appDelegate.attach(store: agents, router: router)
                ConversationModel.onMissedReply = { [inbox] agent, sessionID, runID, text in
                    inbox.addMissedReply(agent: agent, sessionID: sessionID, runID: runID, text: text)
                }
                if !agents.isDemo { await appDelegate.enablePushIfPossible() }
                #if DEBUG
                // `-testNowPlaying`: plays a bundled voice preview with artwork through the shared player (lock screen,
                // Dynamic Island), to check the MediaPlayer callbacks without a server.
                if ProcessInfo.processInfo.arguments.contains("-testNowPlaying"),
                   let url = Bundle.main.url(forResource: "voice-colibri", withExtension: "m4a") {
                    VoiceNotePlayer.shared.toggle(url, info: NowPlayingInfo(title: "Test", artist: "Bip", artwork: UIImage(systemName: "star.fill")))
                }
                #endif
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
                            Group {
                                switch route {
                                case .sessions(let agent): SessionsView(agent: agent)
                                case .conversation(let agent, let sessionID, let start): ConversationView(agent: agent, sessionID: sessionID, start: start)
                                case .call(let agent): CallView(agent: agent, sessionID: agents.mainSessionID(for: agent))
                                }
                            }
                            // A notification replaces the screen on top with another agent's conversation: same place
                            // in the stack, so SwiftUI would keep the old screen (and its state) without a new identity.
                            .id(route)
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
