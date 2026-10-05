import Foundation
import Intents
import Security
import UserNotifications

/// Enriches agent pushes on the device: the APNs payload only carries ids, the full text and the
/// pre-synthesized audio are fetched from the agent's bridge over the tailnet. If the tailnet is
/// unreachable (VPN off), the notification is shown with the title/body APNs already carried.
final class NotificationService: UNNotificationServiceExtension {
    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestAttempt: UNMutableNotificationContent?
    private var task: Task<Void, Never>?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        self.contentHandler = contentHandler
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        bestAttempt = content
        let userInfo = request.content.userInfo
        guard let agentName = userInfo["agent"] as? String, let agent = SharedAgents.agent(named: agentName) else {
            contentHandler(content)
            return
        }
        let outboxID = userInfo["outbox_id"] as? String
        let runID = userInfo["run_id"] as? String
        let isApproval = userInfo["kind"] == nil // replies and questions carry a kind
        let replyID = userInfo["reply_id"] as? String
        // The system owns these objects; the extension's own contract is "call the handler once, soon".
        nonisolated(unsafe) let pending = content
        nonisolated(unsafe) let deliver = contentHandler
        task = Task {
            if let bridge = agent.bridge {
                if let outboxID {
                    await Self.enrich(pending, outboxID: outboxID, bridge: bridge)
                } else if let runID, isApproval {
                    await Self.enrichApproval(pending, agent: agentName, runID: runID, bridge: bridge)
                } else if let replyID {
                    await Self.enrichReply(pending, replyID: replyID, bridge: bridge)
                }
            }
            deliver(await Self.asMessage(pending, from: agent))
        }
    }

    /// A communication notification: the agent's Bip as the sender's picture (the app icon goes in the
    /// corner), its name as the sender, one conversation per agent. Falls back to the plain notification.
    private static func asMessage(_ content: UNMutableNotificationContent, from agent: SharedAgents.Agent) async -> UNNotificationContent {
        let image = agent.avatar.map { INImage(imageData: $0) }
        let sender = INPerson(personHandle: INPersonHandle(value: agent.id.uuidString, type: .unknown), nameComponents: nil,
                              displayName: agent.name, image: image, contactIdentifier: nil, customIdentifier: agent.id.uuidString)
        let intent = INSendMessageIntent(recipients: nil, outgoingMessageType: .outgoingMessageText, content: content.body,
                                         speakableGroupName: nil, conversationIdentifier: agent.id.uuidString,
                                         serviceName: nil, sender: sender, attachments: nil)
        if let image { intent.setImage(image, forParameterNamed: \.sender) }
        let interaction = INInteraction(intent: intent, response: nil)
        interaction.direction = .incoming
        try? await interaction.donate()
        return (try? content.updating(from: intent)) ?? content
    }

    override func serviceExtensionTimeWillExpire() {
        task?.cancel()
        if let contentHandler, let bestAttempt { contentHandler(bestAttempt) }
    }

    private static func enrich(_ content: UNMutableNotificationContent, outboxID: String, bridge: SharedAgents.Bridge) async {
        let base = bridge.url.appending(path: "v1/outbox").appending(path: outboxID)
        if let data = try? await bridge.get(base),
           let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            if let title = json["title"] as? String, !title.isEmpty { content.subtitle = title }
            if let text = (json["text"] ?? json["body"]) as? String, !text.isEmpty { content.body = text }
        }
        // 200 only: a 202 means the mp3 is still being synthesized, the text alone is shown then.
        if let audio = try? await bridge.get(base.appending(path: "audio")), !audio.isEmpty {
            let file = URL.temporaryDirectory.appending(path: "\(outboxID).mp3")
            if (try? audio.write(to: file)) != nil,
               let attachment = try? UNNotificationAttachment(identifier: "audio", url: file, options: nil) {
                content.attachments = [attachment]
            }
        }
    }

    /// « Reply ready » / question pushes carry an id only: show the agent's text, fetched over the tailnet (it
    /// never goes through Apple). Markdown marks and `MEDIA:` lines are dropped; iOS truncates the rest.
    private static func enrichReply(_ content: UNMutableNotificationContent, replyID: String, bridge: SharedAgents.Bridge) async {
        guard let data = try? await bridge.get(bridge.url.appending(path: "v1/replies").appending(path: replyID)),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              var text = json["text"] as? String else { return }
        text = text.split(separator: "\n").filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("MEDIA:") }.joined(separator: "\n")
        for mark in ["**", "__", "`", "### ", "## ", "# "] { text = text.replacingOccurrences(of: mark, with: "") }
        text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { content.body = String(text.prefix(1000)) }
    }

    /// Approval pushes carry ids only; show the (already redacted) command the agent wants to run.
    private static func enrichApproval(_ content: UNMutableNotificationContent, agent: String, runID: String, bridge: SharedAgents.Bridge) async {
        var components = URLComponents(url: bridge.url.appending(path: "v1/approvals"), resolvingAgainstBaseURL: false)!
        components.queryItems = [URLQueryItem(name: "agent", value: agent)]
        guard let url = components.url, let data = try? await bridge.get(url),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let items = json["items"] as? [[String: Any]],
              let item = items.first(where: { $0["run_id"] as? String == runID }) else { return }
        if let command = item["command"] as? String, !command.isEmpty {
            content.body = String(localized: "Veut lancer : \(command)")
        } else if let description = item["description"] as? String, !description.isEmpty {
            content.body = description
        }
    }
}

/// Minimal read-only view of the app's agents file and Keychain (shared App Group / access group).
enum SharedAgents {
    struct Bridge {
        var url: URL
        var key: String

        func get(_ url: URL) async throws -> Data {
            var request = URLRequest(url: url, timeoutInterval: 8)
            request.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw URLError(.badServerResponse) }
            return data
        }
    }

    struct Agent {
        var id: UUID
        var name: String
        var bridge: Bridge?
        /// The Bip drawn by the app (`AgentAvatars`), PNG.
        var avatar: Data?
    }

    private struct StoredAgent: Decodable {
        struct Config: Decodable { var id: UUID; var name: String; var bridgeURL: URL? }
        var config: Config
    }

    private struct StoredSecrets: Decodable { var bridgeKey: String? }

    static func agent(named name: String) -> Agent? {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Bundle.main.object(forInfoDictionaryKey: "BIPAppGroup") as? String ?? "group.io.github.bipagents"),
              let data = try? Data(contentsOf: container.appending(path: "agents.json")),
              let agents = try? JSONDecoder().decode([StoredAgent].self, from: data),
              let stored = agents.first(where: { $0.config.name.compare(name, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame })
        else { return nil }
        let id = stored.config.id
        let bridge = stored.config.bridgeURL.flatMap { url in bridgeKey(for: id).map { Bridge(url: url, key: $0) } }
        let avatar = try? Data(contentsOf: container.appending(path: "avatars/\(id.uuidString).png"))
        return Agent(id: id, name: stored.config.name, bridge: bridge, avatar: avatar)
    }

    private static func bridgeKey(for agentID: UUID) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "io.github.bipagents.agent",
            kSecAttrAccount as String: agentID.uuidString,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess, let data = result as? Data else { return nil }
        return (try? JSONDecoder().decode(StoredSecrets.self, from: data))?.bridgeKey
    }
}
