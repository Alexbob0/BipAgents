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

    /// The agent's ongoing conversation (most recent session), continued by « Écrire » and « Parler »
    /// — one thread per agent, like a messaging app. `nil` until the agent has one.
    func mainSessionID(for agent: AgentProfile) -> String? { latestSession[agent.id]?.id }

    /// Called when the app starts or continues a session, so it becomes the agent's ongoing conversation.
    func noteSession(_ session: HermesSession, for agent: AgentProfile) {
        var session = session
        session.updatedAt = .now
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
            latestSession[agent.id] = try? await client.listSessions(limit: 1).first
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
    /// In-memory store with two sample agents, for previews.
    static var preview: AgentStore {
        let store = AgentStore(fileURL: URL.temporaryDirectory.appending(path: "preview-agents-\(UUID()).json"))
        store.agents = [
            AgentProfile(config: AgentConfig(name: "Wellness", baseURL: URL(string: "https://aibox.example.ts.net:8642")!, voice: "5476"),
                         appearance: AgentAppearance(category: .wellness)),
            AgentProfile(config: AgentConfig(name: "Vie", baseURL: URL(string: "https://aibox.example.ts.net:8644")!, voice: "5476"),
                         appearance: AgentAppearance(category: .daily)),
        ]
        for agent in store.agents { store.reachability[agent.id] = .online }
        store.latestSession[store.agents[0].id] = HermesSession(id: "demo", title: "Plan sommeil du soir", updatedAt: .now.addingTimeInterval(-900),
            lastMessagePreview: "Ce soir, vise un coucher à 23 h 15 : écrans coupés à 22 h 30, lumière tamisée et chambre à 18 °C.")
        store.latestSession[store.agents[1].id] = HermesSession(id: "demo2", title: "Rangement", updatedAt: .now.addingTimeInterval(-3600),
            lastMessagePreview: "C’est rangé ! Je regarde maintenant les vieux fichiers de logs dans ~/projets…")
        store.isDemo = true
        return store
    }
}
