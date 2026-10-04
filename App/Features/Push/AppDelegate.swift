import HermesKit
import UIKit
import UserNotifications

/// Push plumbing: notification categories, APNs registration with each agent's bridge,
/// and the "Approuver / Refuser" actions that resolve a Hermes approval from the lock screen.
final class AppDelegate: NSObject, UIApplicationDelegate {
    var store: AgentStore?
    var router: Router?

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
                UNNotificationAction(identifier: Action.approveOnce, title: "Approuver une fois", options: [.authenticationRequired]),
                UNNotificationAction(identifier: Action.deny, title: "Refuser", options: [.destructive, .authenticationRequired]),
            ], intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.message, actions: [
                UNNotificationAction(identifier: Action.reply, title: "Répondre", options: [.foreground]),
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
            Task { try? await client.registerDevice(token: token, environment: environment, agents: agents) }
        }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: any Error) {
        print("APNs registration failed: \(error.localizedDescription)")
    }
}

extension AppDelegate: UNUserNotificationCenterDelegate {
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let info = response.notification.request.content.userInfo
        let payload = NotificationPayload(
            agent: info["agent"] as? String,
            runID: info["run_id"] as? String,
            requestID: info["request_id"] as? String,
            sessionID: info["session_id"] as? String
        )
        await handle(actionIdentifier: response.actionIdentifier, payload: payload)
    }

    private func handle(actionIdentifier: String, payload: NotificationPayload) async {
        guard let store, let agent = store.agents.first(where: { $0.bridgeName == payload.agent?.lowercased() || $0.name == payload.agent }) else { return }
        switch actionIdentifier {
        case Action.approveOnce, Action.deny:
            guard let runID = payload.runID, let client = store.client(for: agent) else { return }
            let choice: ApprovalChoice = actionIdentifier == Action.deny ? .deny : .once
            _ = try? await client.approve(runID: runID, choice: choice, requestID: payload.requestID)
        default:
            // Tap or "Répondre": open the conversation the message belongs to (or a new one).
            router?.open(.conversation(agent, sessionID: payload.sessionID))
        }
    }
}

private struct NotificationPayload: Sendable {
    var agent: String?
    var runID: String?
    var requestID: String?
    var sessionID: String?
}
