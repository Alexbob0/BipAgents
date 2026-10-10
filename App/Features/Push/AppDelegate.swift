import HermesKit
import UIKit
import UserNotifications

/// Push plumbing: notification categories, APNs registration with each agent's bridge,
/// and the "Approuver / Refuser" actions that resolve a Hermes approval from the lock screen.
@MainActor
final class AppDelegate: NSObject, UIApplicationDelegate {
    private(set) var store: AgentStore?
    private(set) var router: Router?
    /// A notification tapped before the app was ready (cold launch from the notification).
    private var pendingTap: (action: String, payload: NotificationPayload)?

    /// Called once the app's state exists; replays a notification tapped during launch.
    func attach(store: AgentStore, router: Router) {
        self.store = store
        self.router = router
        if let (action, payload) = pendingTap {
            pendingTap = nil
            Task { await handle(actionIdentifier: action, payload: payload) }
        }
    }

    enum Category {
        static let approval = "APPROVAL"
        static let message = "MESSAGE"
    }

    enum Action {
        static let approveOnce = "APPROVE_ONCE"
        static let deny = "DENY"
        static let reply = "REPLY"
    }

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories([
            UNNotificationCategory(identifier: Category.approval, actions: [
                UNNotificationAction(identifier: Action.approveOnce, title: String(localized: "Approuver une fois"), options: [.authenticationRequired]),
                UNNotificationAction(identifier: Action.deny, title: String(localized: "Refuser"), options: [.destructive, .authenticationRequired]),
            ], intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.message, actions: [
                UNNotificationAction(identifier: Action.reply, title: String(localized: "Répondre"), options: [.foreground]),
            ], intentIdentifiers: []),
        ])
        return true
    }

    /// Asks for permission once at least one agent has a bridge, then registers with APNs.
    func enablePushIfPossible() async {
        guard let store, store.agents.contains(where: { $0.config.bridgeURL != nil }) else { return }
        let granted = (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        if granted { UIApplication.shared.registerForRemoteNotifications() }
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        guard let store else { return }
        #if DEBUG
        let environment = "sandbox"
        #else
        let environment = "production"
        #endif
        // One registration per bridge, listing the agents it serves.
        var byBridge: [URL: (BridgeClient, [String])] = [:]
        for agent in store.agents {
            guard let client = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            byBridge[client.baseURL, default: (client, [])].1.append(agent.bridgeName)
        }
        for (client, agents) in byBridge.values {
            Task {
                do {
                    try await client.registerDevice(token: token, environment: environment, agents: agents)
                    #if DEBUG
                    print("[push] device registered with \(client.baseURL.host() ?? "?") (\(environment)) for \(agents)")
                    #endif
                } catch {
                    print("[push] device registration failed: \(error)")
                }
            }
        }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: any Error) {
        print("APNs registration failed: \(error.localizedDescription)")
    }
}

extension AppDelegate: UNUserNotificationCenterDelegate {
    // Completion-handler variants on purpose: with the async ones, Swift calls UIKit's completion handler
    // from a background thread and UIKit aborts (« Call must be made on main thread »).
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        let action = response.actionIdentifier
        #if DEBUG
        print("[push] tapped: action=\(action) agent=\(info["agent"] ?? "-") session=\(info["session_id"] ?? "-") outbox=\(info["outbox_id"] ?? "-") kind=\(info["kind"] ?? "-")")
        #endif
        let payload = NotificationPayload(
            agent: info["agent"] as? String,
            runID: info["run_id"] as? String,
            requestID: info["request_id"] as? String,
            sessionID: info["session_id"] as? String,
            outboxID: info["outbox_id"] as? String
        )
        nonisolated(unsafe) let done = completionHandler // called once, on the main actor
        Task { @MainActor in
            await self.handle(actionIdentifier: action, payload: payload)
            done()
        }
    }

    @MainActor
    private func handle(actionIdentifier: String, payload: NotificationPayload) async {
        guard let store, let router else {
            #if DEBUG
            print("[push] app not ready, tap kept for later")
            #endif
            pendingTap = (actionIdentifier, payload)
            return
        }
        guard let agent = store.agents.first(where: { $0.bridgeName == payload.agent?.lowercased() || $0.name == payload.agent }) else {
            #if DEBUG
            print("[push] no agent matches \(payload.agent ?? "-") among \(store.agents.map(\.bridgeName))")
            #endif
            return
        }
        #if DEBUG
        print("[push] opening \(agent.name), session \(payload.sessionID ?? "-")")
        #endif
        switch actionIdentifier {
        case Action.approveOnce, Action.deny:
            guard let runID = payload.runID, let client = store.client(for: agent) else { return }
            let choice: ApprovalChoice = actionIdentifier == Action.deny ? .deny : .once
            _ = try? await client.approve(runID: runID, choice: choice, requestID: payload.requestID)
        default:
            var runSession = payload.sessionID
            if runSession == nil, payload.outboxID == nil, let runID = payload.runID, let client = store.client(for: agent) {
                runSession = (try? await client.getRun(id: runID))?.sessionID // older bridge: ask Hermes
            }
            if payload.outboxID == nil, let runID = payload.runID, let session = runSession {
                // An approval (or a reply): its conversation, re-attached to the run, which asks Hermes for the card.
                ConversationModel.noteActiveRun(runID, sessionID: session)
                router.open(.conversation(agent, sessionID: session))
            } else if payload.outboxID != nil || (payload.sessionID == nil && payload.runID != nil) {
                // A scheduled task's report or a proactive message: it is in the agent's Discussion (and the Boîte).
                // An approval without its conversation (older bridge): the Discussion too, where the card shows up
                // (asked to Hermes) rather than an empty new conversation.
                var main = store.latestSession[agent.id]
                if main.map(AgentStore.isBotChat) != true, let client = store.client(for: agent),
                   let sessions = try? await AgentStore.sessions(from: client, maxPages: 4,
                                                                 until: { $0.contains(where: AgentStore.isBotChat) }).sessions {
                    main = AgentStore.mainThread(in: sessions)  // cold launch: not loaded yet
                }
                if let main, AgentStore.isBotChat(main) {
                    router.open(.conversation(agent, sessionID: main.id))
                } else if payload.outboxID != nil {
                    router.tab = .inbox
                } else {
                    router.open(.conversation(agent, sessionID: nil))
                }
            } else {
                // A reply, an approval, or « Répondre »: the conversation it belongs to (or a new one).
                router.open(.conversation(agent, sessionID: payload.sessionID))
            }
        }
    }
}

private struct NotificationPayload: Sendable {
    var agent: String?
    var runID: String?
    var requestID: String?
    var sessionID: String?
    var outboxID: String?
}
