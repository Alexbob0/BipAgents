import Foundation
import HermesKit
import Observation
import SwiftUI

enum AppGroup {
    /// `group.$(BIP_BUNDLE_ID)` (Config/Base.xcconfig), written into Info.plist at build time.
    static let identifier = Bundle.main.object(forInfoDictionaryKey: "BIPAppGroup") as? String ?? "group.io.github.bipagents"
}

/// An agent as the app knows it: Hermes connection settings plus its look.
struct AgentProfile: Codable, Identifiable, Hashable, Sendable {
    var config: AgentConfig
    var appearance: AgentAppearance

    nonisolated var id: UUID { config.id }
    nonisolated var name: String { config.name }
}

enum AgentReachability: Equatable, Sendable {
    case unknown, checking, online, unauthorized, offline(String)
    /// Reached through the bridge's door on the local network (the tailnet does not answer).
    case local
    /// The agents' server answers, but not the model they use (its machine off, an expired API key).
    case modelDown

    var label: String {
        switch self {
        case .unknown, .checking: "…"
        case .online: String(localized: "En ligne")
        case .local: String(localized: "En ligne · réseau local")
        case .modelDown: String(localized: "Modèle injoignable")
        case .unauthorized: String(localized: "Clé refusée")
        case .offline: String(localized: "Hors tailnet")
        }
    }
}

/// Configured agents, persisted as JSON in Application Support; secrets go to the Keychain.
@Observable
final class AgentStore {
    private(set) var agents: [AgentProfile] = [] {
        didSet { LocalLink.updatePins(for: agents) }
    }
    private(set) var reachability: [UUID: AgentReachability] = [:]
    /// Most recent session per agent, for the home card preview.
    private(set) var latestSession: [UUID: HermesSession] = [:] {
        didSet { if persistsPreviews { Self.savePreviews(latestSession) } }
    }
    /// The home cards' last message survives a relaunch: shown at once, refreshed by `refreshAll`.
    @ObservationIgnored private var persistsPreviews = false
    private static var previewsURL: URL { URL.cachesDirectory.appending(path: "agent-previews.json") }

    private let fileURL: URL
    /// Sample data only: never touches the network or the Keychain.
    private(set) var isDemo = false

    init(fileURL: URL = AgentStore.defaultFileURL) {
        self.fileURL = fileURL
        load()
        if fileURL == AgentStore.defaultFileURL {  // not the demo / preview stores
            if let data = try? Data(contentsOf: Self.previewsURL),
               let saved = try? JSONDecoder().decode([UUID: HermesSession].self, from: data) {
                latestSession = saved.filter { id, _ in agents.contains { $0.id == id } }
            }
            persistsPreviews = true
        }
    }

    private static func savePreviews(_ sessions: [UUID: HermesSession]) {
        guard let data = try? JSONEncoder().encode(sessions) else { return }
        try? data.write(to: previewsURL, options: .atomic)
    }

    /// In the App Group container so the notification extension can find each agent's bridge.
    static var defaultFileURL: URL {
        let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier)
        return (container ?? URL.applicationSupportDirectory).appending(path: "agents.json")
    }

    func client(for agent: AgentProfile) -> HermesClient? {
        guard let secrets = Keychain.secrets(for: agent.id) else { return nil }
        return HermesClient(baseURL: LocalLink.hermesURL(for: agent), apiKey: secrets.apiKey,
                            session: LocalLink.session(for: agent))
    }

    /// An install's QR code: every agent its bridge lists is added, or updated when already here (same Hermes
    /// address). Returns how many were added and updated.
    func addInstall(_ pairing: InstallPairing) async throws -> (added: Int, updated: Int) {
        let bridge = BridgeClient(url: pairing.bridgeURL, key: pairing.bridgeKey)
        let listed = try await withTimeout(15) { try await bridge.installAgents() }
        var added = 0, updated = 0
        for agent in listed {
            let config = AgentConfig(name: agent.name, baseURL: agent.url, bridgeURL: pairing.bridgeURL,
                                     language: AgentLanguage.device.rawValue, lanURL: pairing.lanURL,
                                     lanFingerprint: pairing.lanFingerprint, bridgeAgent: agent.id)
            let secrets = AgentSecrets(apiKey: agent.key, bridgeKey: pairing.bridgeKey)
            if try updateConnection(from: AgentProvisioning(config: config, secrets: secrets)) {
                updated += 1
            } else {
                let category = AgentCategory.suggest(name: agent.name)
                var profile = AgentProfile(config: config, appearance: AgentAppearance(category: category))
                profile.config.category = category.rawValue
                try add(profile, secrets: secrets)
                added += 1
            }
        }
        return (added, updated)
    }

    /// A newly scanned QR code of an agent already here (same Hermes address): new keys, bridge and LAN door,
    /// the rest (Bip, voice, language) kept. Returns false when no agent matches.
    func updateConnection(from provisioning: AgentProvisioning) throws -> Bool {
        let scanned = provisioning.config
        guard let index = agents.firstIndex(where: {
            $0.config.baseURL.host() == scanned.baseURL.host() && $0.config.baseURL.port == scanned.baseURL.port
                && $0.config.baseURL.path() == scanned.baseURL.path()   // agents of one gateway differ by /p/<agent>
        }) else { return false }
        var profile = agents[index]
        profile.config.bridgeURL = scanned.bridgeURL ?? profile.config.bridgeURL
        profile.config.lanURL = scanned.lanURL
        profile.config.lanFingerprint = scanned.lanFingerprint
        profile.config.bridgeAgent = scanned.bridgeAgent ?? profile.config.bridgeAgent
        try Keychain.save(provisioning.secrets, for: profile.id)
        agents[index] = profile
        save()
        Task { await refreshReachability(of: profile) }
        return true
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
        // A scheduled task's conversation, or a side conversation while the agent has a « Bot Chat », never
        // becomes the thread that « Audio », « Écrire » and « Live » continue.
        if session.id != previous?.id, Self.isScheduledTask(session) || previous.map(Self.isBotChat) == true { return }
        session.updatedAt = .now
        session.lastMessagePreview = preview ?? session.lastMessagePreview
            ?? (previous?.id == session.id ? previous?.lastMessagePreview : nil)
        if session.title == nil, previous?.id == session.id { session.title = previous?.title }
        latestSession[agent.id] = session
    }

    /// Hermes lists sessions most recent first, a page at a time. Scheduled tasks open a session per run, so they
    /// can push conversations (and the Bot Chat, opened once) past the first page: pages are read until `enough`
    /// is satisfied, the list ends or `maxPages` is reached. `complete` tells whether the list ended.
    static func sessions(from client: HermesClient, offset: Int = 0, pageSize: Int = 50, maxPages: Int = 6,
                         until enough: ([HermesSession]) -> Bool = { _ in false }) async throws -> (sessions: [HermesSession], complete: Bool) {
        var all: [HermesSession] = []
        var offset = offset
        for _ in 0..<maxPages {
            let page = try await client.listSessions(limit: pageSize, offset: offset)
            offset += page.count
            all += page.filter { session in !all.contains { $0.id == session.id } }
            if page.count < pageSize { return (all, true) }
            if enough(all) { break }
        }
        return (all, false)
    }

    /// Hermes says this session no longer exists (deleted here, elsewhere, or by aibox): its home card preview goes.
    func forgetSession(_ sessionID: String, for agent: AgentProfile) {
        if latestSession[agent.id]?.id == sessionID { latestSession[agent.id] = nil }
    }

    /// The agent's ongoing conversation among its sessions (most recent first): its « Bot Chat » (Hermes
    /// Bot Mode's permanent thread) when it has one, else the most recent one that is not a scheduled task's.
    static func mainThread(in sessions: [HermesSession]) -> HermesSession? {
        sessions.first(where: isBotChat) ?? sessions.first { !isScheduledTask($0) }
    }

    static func isBotChat(_ session: HermesSession) -> Bool {
        session.title?.caseInsensitiveCompare("Bot Chat") == .orderedSame
    }

    static func isScheduledTask(_ session: HermesSession) -> Bool {
        session.id.hasPrefix("cron_") || session.source?.lowercased() == "cron"
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

    /// Tailnet first; when it does not answer, the bridge's door on the local network (if the agent has one). While
    /// the tailnet works, the door's address is refreshed from the bridge, so a new IP needs no new QR code.
    func refreshReachability(of agent: AgentProfile) async {
        guard let secrets = secrets(for: agent) else {
            reachability[agent.id] = .unauthorized
            return
        }
        reachability[agent.id] = .checking
        let tailnet = HermesClient(baseURL: agent.config.baseURL, apiKey: secrets.apiKey)
        do {
            _ = try await withTimeout(8) { try await tailnet.capabilities() }
            LocalLink.setActive(false, for: agent)
            reachability[agent.id] = .online
            await refreshLocalDoor(of: agent, secrets: secrets)
            if let bridge = BridgeClient(tailnetOf: agent, secrets: secrets),
               (try? await withTimeout(8) { try await bridge.modelIsUp() }) == false {
                reachability[agent.id] = .modelDown
            }
        } catch HermesError.unauthorized(_) {
            reachability[agent.id] = .unauthorized
            return
        } catch {
            guard let door = agent.config.lanURL else {
                reachability[agent.id] = .offline(error.localizedDescription)
                return
            }
            let local = HermesClient(baseURL: door.appending(path: "hermes/\(agent.bridgeName)"), apiKey: secrets.apiKey,
                                     session: LocalLink.session)
            do {
                _ = try await withTimeout(5) { try await local.capabilities() }
                LocalLink.setActive(true, for: agent)
                reachability[agent.id] = .local
            } catch {
                LocalLink.setActive(false, for: agent)
                lostAgents.insert(agent.id)
                reachability[agent.id] = .offline(error.localizedDescription)
                return
            }
        }
        lostAgents.remove(agent.id)
        if let client = client(for: agent) {
            if let sessions = try? await Self.sessions(from: client, maxPages: 4, until: { $0.contains(where: Self.isBotChat) }).sessions,
               var latest = Self.mainThread(in: sessions) {
                // Hermes' list preview is the session's *first* message: show the latest one instead.
                if let history = try? await client.messages(sessionID: latest.id),
                   let last = history.last(where: { ($0.role == .assistant || $0.role == .user) && !$0.text.isEmpty }) {
                    latest.lastMessagePreview = ChatText.preview(last.text)
                } else if latestSession[agent.id]?.id == latest.id {
                    latest.lastMessagePreview = latestSession[agent.id]?.lastMessagePreview
                }
                latestSession[agent.id] = latest
            }
        }
    }

    /// Agents unreachable both on the tailnet and on the local network although they have a LAN door: their
    /// address may have changed while the tailnet was down (the home screen says how to get a new QR code).
    private(set) var lostAgents: Set<UUID> = []

    private func refreshLocalDoor(of agent: AgentProfile, secrets: AgentSecrets) async {
        guard let bridge = BridgeClient(tailnetOf: agent, secrets: secrets) else { return }
        let door: (url: URL, fingerprint: String)?
        do { door = try await bridge.pairing() } catch { return }  // an older bridge, or a hiccup: keep what we have
        guard let index = agents.firstIndex(where: { $0.id == agent.id }) else { return }
        guard agents[index].config.lanURL != door?.url || agents[index].config.lanFingerprint != door?.fingerprint else { return }
        #if DEBUG
        print("[lan] \(agent.name): door \(door.map { $0.url.absoluteString } ?? "off")")
        #endif
        agents[index].config.lanURL = door?.url
        agents[index].config.lanFingerprint = door?.fingerprint
        save()
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
        // The first Bip voices (Loutre, Chat, Lutin) were replaced: back to the category's voice.
        if agents.contains(where: { $0.config.voice.map(AgentVoices.retired.contains) == true }) {
            for index in agents.indices where agents[index].config.voice.map(AgentVoices.retired.contains) == true {
                agents[index].config.voice = nil
            }
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
            AgentProfile(config: AgentConfig(name: "Wellness", baseURL: URL(string: "https://server.example.ts.net:8642")!),
                         appearance: AgentAppearance(category: .wellness)),
            AgentProfile(config: AgentConfig(name: "Vie", baseURL: URL(string: "https://server.example.ts.net:8644")!),
                         appearance: AgentAppearance(category: .daily)),
        ]
        // `-demoAgents 5`: more sample agents, to review the compact home cards.
        let extra: [(String, AgentCategory, String, String)] = [
            ("Budget", .finance, String(localized: "Dépenses d’octobre"), String(localized: "Tu as dépensé 420 € en courses ce mois-ci, 12 % de moins qu’en septembre.")),
            ("Boulot", .work, String(localized: "Réunion de lundi"), String(localized: "J’ai préparé l’ordre du jour et envoyé l’invitation à l’équipe pour 10 h.")),
            ("Maison", .home, String(localized: "Chaudière"), String(localized: "Le technicien passe jeudi entre 8 h et 12 h, pense à laisser l’accès au garage.")),
            ("Atelier", .creative, String(localized: "Histoire du soir"), String(localized: "Il était une fois un petit renard qui collectionnait les étoiles filantes…")),
        ]
        for (index, (name, category, title, preview)) in extra.prefix(max(0, UserDefaults.standard.integer(forKey: "demoAgents") - 2)).enumerated() {
            let agent = AgentProfile(config: AgentConfig(name: name, baseURL: URL(string: "https://server.example.ts.net:8650")!),
                                     appearance: AgentAppearance(category: category))
            store.agents.append(agent)
            store.latestSession[agent.id] = HermesSession(id: "demo-extra-\(index)", title: title,
                                                          updatedAt: .now.addingTimeInterval(-Double(index + 2) * 3600), lastMessagePreview: preview)
        }
        for agent in store.agents { store.reachability[agent.id] = .online }
        store.latestSession[store.agents[0].id] = HermesSession(id: "demo", title: String(localized: "Plan sommeil du soir"), updatedAt: .now.addingTimeInterval(-900),
            lastMessagePreview: String(localized: "Ce soir, vise un coucher à 23 h 15 : écrans coupés à 22 h 30, lumière tamisée et chambre à 18 °C."))
        store.latestSession[store.agents[1].id] = HermesSession(id: "demo2", title: String(localized: "Rangement"), updatedAt: .now.addingTimeInterval(-3600),
            lastMessagePreview: String(localized: "C’est rangé ! Je regarde maintenant les vieux fichiers de logs dans ~/projets…"))
        store.isDemo = true
        return store
    }
}
