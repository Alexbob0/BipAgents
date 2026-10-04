import Foundation
import HermesKit
import Observation
import SwiftUI

enum AppGroup {
    static let identifier = "group.io.github.bipagents"
}

/// An agent as the app knows it: Hermes connection settings plus its look.
struct AgentProfile: Codable, Identifiable, Hashable, Sendable {
    var config: AgentConfig
    var appearance: AgentAppearance

    var id: UUID { config.id }
    var name: String { config.name }
}

enum AgentReachability: Equatable, Sendable {
    case unknown, checking, online, unauthorized, offline(String)

    var label: String {
        switch self {
        case .unknown, .checking: "…"
        case .online: "En ligne"
        case .unauthorized: "Clé refusée"
        case .offline: "Hors tailnet"
        }
    }
}

/// Configured agents, persisted as JSON in Application Support; secrets go to the Keychain.
@Observable
final class AgentStore {
    private(set) var agents: [AgentProfile] = []
    private(set) var reachability: [UUID: AgentReachability] = [:]
    /// Most recent session per agent, for the home card preview.
    private(set) var latestSession: [UUID: HermesSession] = [:]

    private let fileURL: URL
    /// Sample data only: never touches the network or the Keychain.
    private(set) var isDemo = false

    init(fileURL: URL = AgentStore.defaultFileURL) {
        self.fileURL = fileURL
        load()
    }

    /// In the App Group container so the notification extension can find each agent's bridge.
    static var defaultFileURL: URL {
        let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier)
        return (container ?? URL.applicationSupportDirectory).appending(path: "agents.json")
    }

    func client(for agent: AgentProfile) -> HermesClient? {
        guard let secrets = Keychain.secrets(for: agent.id) else { return nil }
        return HermesClient(baseURL: agent.config.baseURL, apiKey: secrets.apiKey)
    }

    func secrets(for agent: AgentProfile) -> AgentSecrets? { Keychain.secrets(for: agent.id) }

    /// The agent's ongoing conversation (most recent session), continued by « Audio », « Écrire » and « Live »
    /// — one thread per agent, like a messaging app. `nil` until the agent has one.
    func mainSessionID(for agent: AgentProfile) -> String? { latestSession[agent.id]?.id }

    /// Called when the app starts or continues a session, so it becomes the agent's ongoing conversation.
    /// `preview` is the latest message text when the caller knows it. Session details fetched one by one
    /// carry no preview: keep the one already shown rather than blanking the home card.
    func noteSession(_ session: HermesSession, for agent: AgentProfile, preview: String? = nil) {
        var session = session
        let previous = latestSession[agent.id]
        session.updatedAt = .now
        session.lastMessagePreview = preview ?? session.lastMessagePreview
            ?? (previous?.id == session.id ? previous?.lastMessagePreview : nil)
        if session.title == nil, previous?.id == session.id { session.title = previous?.title }
        latestSession[agent.id] = session
    }

    func add(_ profile: AgentProfile, secrets: AgentSecrets) throws {
        try Keychain.save(secrets, for: profile.id)
        agents.removeAll { $0.id == profile.id }
        agents.append(profile)
        save()
        Task { await refreshReachability(of: profile) }
    }

    func update(_ profile: AgentProfile) {
        guard let index = agents.firstIndex(where: { $0.id == profile.id }) else { return }
        agents[index] = profile
        save()
    }

    func remove(_ profile: AgentProfile) {
        Keychain.delete(for: profile.id)
        agents.removeAll { $0.id == profile.id }
        reachability[profile.id] = nil
        save()
    }

    func move(from source: IndexSet, to destination: Int) {
        agents.move(fromOffsets: source, toOffset: destination)
        save()
    }

    // MARK: Reachability

    func refreshAll() async {
        guard !isDemo else { return }
        await withTaskGroup(of: Void.self) { group in
            for agent in agents {
                group.addTask { await self.refreshReachability(of: agent) }
            }
        }
    }

    func refreshReachability(of agent: AgentProfile) async {
        guard let client = client(for: agent) else {
            reachability[agent.id] = .unauthorized
            return
        }
        reachability[agent.id] = .checking
        do {
            _ = try await client.capabilities()
            reachability[agent.id] = .online
            if var latest = try? await client.listSessions(limit: 1).first {
                // Hermes' list preview is the session's *first* message: show the latest one instead.
                if let history = try? await client.messages(sessionID: latest.id),
                   let last = history.last(where: { ($0.role == .assistant || $0.role == .user) && !$0.text.isEmpty }) {
                    latest.lastMessagePreview = last.text
                } else if latestSession[agent.id]?.id == latest.id {
                    latest.lastMessagePreview = latestSession[agent.id]?.lastMessagePreview
                }
                latestSession[agent.id] = latest
            }
        } catch HermesError.unauthorized(_) {
            reachability[agent.id] = .unauthorized
        } catch {
            reachability[agent.id] = .offline(error.localizedDescription)
        }
    }

    // MARK: Persistence

    private func load() {
        guard let data = try? Data(contentsOf: fileURL) else { return }
        agents = (try? JSONDecoder().decode([AgentProfile].self, from: data)) ?? []
        // The Bips' voices arrived: agents still on the old default Kyutai voice get their category's Bip voice.
        let migrated = "voices.bips.v1"
        if !UserDefaults.standard.bool(forKey: migrated) {
            for index in agents.indices where agents[index].config.voice == "5476" { agents[index].config.voice = nil }
            UserDefaults.standard.set(true, forKey: migrated)
            save()
        }
    }

    private func save() {
        do {
            try FileManager.default.createDirectory(at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            try JSONEncoder().encode(agents).write(to: fileURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch {
            assertionFailure("Could not save agents: \(error)")
        }
    }
}

extension AgentStore {
    /// In-memory store with two sample agents (more with `-demoAgents n`), for previews.
    static var preview: AgentStore {
        let store = AgentStore(fileURL: URL.temporaryDirectory.appending(path: "preview-agents-\(UUID()).json"))
        store.agents = [
            AgentProfile(config: AgentConfig(name: "Wellness", baseURL: URL(string: "https://aibox.example.ts.net:8642")!),
                         appearance: AgentAppearance(category: .wellness)),
            AgentProfile(config: AgentConfig(name: "Vie", baseURL: URL(string: "https://aibox.example.ts.net:8644")!),
                         appearance: AgentAppearance(category: .daily)),
        ]
        // `-demoAgents 5`: more sample agents, to review the compact home cards.
        let extra: [(String, AgentCategory)] = [("Budget", .finance), ("Boulot", .work), ("Maison", .home), ("Atelier", .creative)]
        for (name, category) in extra.prefix(max(0, UserDefaults.standard.integer(forKey: "demoAgents") - 2)) {
            store.agents.append(AgentProfile(config: AgentConfig(name: name, baseURL: URL(string: "https://aibox.example.ts.net:8650")!),
                                             appearance: AgentAppearance(category: category)))
        }
        for agent in store.agents { store.reachability[agent.id] = .online }
        store.latestSession[store.agents[0].id] = HermesSession(id: "demo", title: "Plan sommeil du soir", updatedAt: .now.addingTimeInterval(-900),
            lastMessagePreview: "Ce soir, vise un coucher à 23 h 15 : écrans coupés à 22 h 30, lumière tamisée et chambre à 18 °C.")
        store.latestSession[store.agents[1].id] = HermesSession(id: "demo2", title: "Rangement", updatedAt: .now.addingTimeInterval(-3600),
            lastMessagePreview: "C’est rangé ! Je regarde maintenant les vieux fichiers de logs dans ~/projets…")
        store.isDemo = true
        return store
    }
}
