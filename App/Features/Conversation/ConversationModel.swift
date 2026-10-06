import CryptoKit
import Foundation
import HermesKit
import Observation
import UIKit
import VoiceKit

/// One row of the conversation thread.
struct ChatItem: Identifiable, Equatable {
    enum Kind: Equatable {
        case user(text: String, attachments: [LocalAttachment])
        case assistant(text: String, isStreaming: Bool)
        case reasoning(text: String)
        case tools([ToolEvent])
        case approval(ApprovalRequest, resolved: ApprovalChoice?)
        /// The agent asks a question (`clarify`): pending, answered, or expired.
        case question(ClarifyRequest, state: QuestionState)
        case notice(String)
        /// A scheduled task's report or a proactive message (bridge outbox), shown at its time in the agent's
        /// « Discussion » so everything the agent says lives in one place.
        case scheduled(OutboxItem)
    }

    let id: UUID
    var kind: Kind
    /// When it was sent or received (history: from Hermes, may be unknown).
    var date: Date?

    init(id: UUID = UUID(), _ kind: Kind, date: Date? = .now) {
        self.id = id
        self.kind = kind
        self.date = date
    }
}

enum QuestionState: Equatable {
    case pending
    case answered([String: String])
    /// Expired, stopped or cancelled: the agent went on without an answer.
    case closed
}

/// A file or photo picked on the phone, before/after sending.
struct LocalAttachment: Identifiable, Equatable {
    enum Kind: Equatable { case image, document }
    let id = UUID()
    var kind: Kind
    var filename: String
    var mimeType: String
    var data: Data
    /// Where the app keeps it (`AttachmentStore`): opened full screen / in Quick Look.
    var fileURL: URL? = nil
    /// Byte size when `data` is not loaded (a stored document).
    var storedSize: Int? = nil

    var byteCount: Int { storedSize ?? data.count }

    var hermesAttachment: Attachment {
        switch kind {
        case .image: .image(data: data, mimeType: mimeType)
        case .document: .document(filename: filename, mimeType: mimeType, data: data)
        }
    }
}

/// The agent's spoken version of a reply to a voice note.
enum VoiceReplyState: Equatable {
    case preparing
    /// Playing while Kyutai still produces it (recorded at the same time).
    case streaming
    case ready(URL, duration: TimeInterval)
    case unavailable
}

@Observable
final class ConversationModel {
    let agent: AgentProfile
    let voice: VoiceEngine
    private(set) var speakingItemID: UUID?
    /// User messages sent as voice notes (audio + transcript), by item id.
    private(set) var voiceNotes: [UUID: VoiceRecording] = [:]
    /// Spoken replies to voice notes, by assistant item id.
    private(set) var voiceReplies: [UUID: VoiceReplyState] = [:]
    let player = VoiceNotePlayer()
    private let streamer = StreamingSpeechPlayer()
    private var voiceTask: Task<Void, Never>?
    private var replyByVoice = false
    private(set) var sessionID: String?
    private(set) var title: String?
    private(set) var items: [ChatItem] = []
    private(set) var runID: String?
    private(set) var isRunning = false
    private(set) var isWaitingForApproval = false
    private(set) var interim: String?
    var errorMessage: String?

    private let store: AgentStore
    private let client: HermesClient?
    private let uploader: (any DocumentUploader)?
    private let bridge: BridgeClient?
    private var streamTask: Task<Void, Never>?
    /// Keeps the reply streaming for ~30 s after the screen locks, so short replies still arrive live.
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid

    init(agent: AgentProfile, sessionID: String?, store: AgentStore) {
        self.agent = agent
        self.sessionID = sessionID
        self.store = store
        self.client = store.client(for: agent)
        if store.isDemo { items = Self.demoItems(for: agent) }
        if let bridgeURL = agent.config.bridgeURL, let bridgeKey = store.secrets(for: agent)?.bridgeKey {
            uploader = BridgeDocumentUploader(bridgeURL: bridgeURL, bridgeKey: bridgeKey, agent: agent.bridgeName)
        } else {
            uploader = nil
        }
        bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent))
        voice = VoiceEngine(configuration: .init(locale: agent.language.locale), tts: store.ttsProvider(for: agent))
    }

    /// "Écouter" on an assistant message (tap again to stop).
    /// « Écouter » on a reply: generates its voice message on demand (once, then replays it).
    func toggleSpeech(of item: ChatItem) {
        guard case .assistant(let raw, _) = item.kind else { return }
        let text = ChatText.visible(raw)
        if case .ready(let url, _)? = voiceReplies[item.id] {
            player.toggle(url)
            return
        }
        if speakingItemID == item.id {
            voice.stopSpeaking()
            speakingItemID = nil
            return
        }
        generateVoice(for: item.id, text: text, anchor: text, autoplay: true)
    }

    /// Stops a reply that is playing while it is generated (it is generated again on the next « Écouter »).
    func stopVoiceReply(_ itemID: UUID) {
        guard voiceReplies[itemID] == .streaming || voiceReplies[itemID] == .preparing else { return }
        voiceTask?.cancel()
        streamer.stop()
        voiceReplies[itemID] = nil
    }

    /// Sample thread for `-demo` (design review without a server).
    static func demoItems(for agent: AgentProfile) -> [ChatItem] {
        let sheet = LocalAttachment(kind: .document, filename: "sommeil-septembre.xlsx",
                                    mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", data: Data(count: 48_000))
        // A drawn « photo » (a sunset over a hill) to review thumbnails and the full-screen viewer.
        let photoData = UIGraphicsImageRenderer(size: CGSize(width: 900, height: 1200)).jpegData(withCompressionQuality: 0.85) { context in
            let colors = [UIColor(red: 0.98, green: 0.62, blue: 0.42, alpha: 1).cgColor, UIColor(red: 0.42, green: 0.36, blue: 0.75, alpha: 1).cgColor]
            let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors as CFArray, locations: [0, 1])!
            context.cgContext.drawLinearGradient(gradient, start: .zero, end: CGPoint(x: 0, y: 1200), options: [])
            UIColor(red: 1, green: 0.85, blue: 0.55, alpha: 1).setFill()
            UIBezierPath(ovalIn: CGRect(x: 330, y: 520, width: 240, height: 240)).fill()
            UIColor(red: 0.2, green: 0.45, blue: 0.35, alpha: 1).setFill()
            UIBezierPath(ovalIn: CGRect(x: -300, y: 760, width: 1500, height: 900)).fill()
        }
        let photo = LocalAttachment(kind: .image, filename: "coucher-de-soleil.jpg", mimeType: "image/jpeg", data: photoData)
        let evening = Calendar.current.date(bySettingHour: 20, minute: 0, second: 0, of: .now.addingTimeInterval(-86400)) ?? .now
        return [
            ChatItem(.scheduled(OutboxItem(id: "demo-plan", agent: agent.bridgeName, title: String(localized: "Plan sommeil du soir"),
                                           text: String(localized: "Ce soir, vise un coucher à 23 h 15 : écrans coupés à 22 h 30, lumière tamisée et chambre à 18 °C."),
                                           createdAt: evening, sessionID: nil, hasAudio: false)), date: evening),
            ChatItem(.user(text: String(localized: "Regarde le coucher de soleil de ma balade d’hier soir."), attachments: [photo])),
            ChatItem(.user(text: String(localized: "Je me suis couché tard hier. Je peux reprendre un café cet après-midi ?"), attachments: [])),
            ChatItem(.reasoning(text: String(localized: "Vérifier ses habitudes de sommeil et la demi-vie de la caféine."))),
            ChatItem(.tools([
                ToolEvent(tool: "memory", preview: String(localized: "Habitudes de sommeil"), status: .completed, duration: 0.3),
                ToolEvent(tool: "web_search", preview: String(localized: "« caféine demi-vie sommeil »"), status: .completed, duration: 1.9),
            ])),
            ChatItem(.assistant(text: String(localized: "Pas cet après-midi : la caféine met 5 à 6 h à s’éliminer de moitié. Avec un coucher visé à 23 h 15, ta limite est **14 h**. Coup de barre ? Une sieste de 20 min avant 15 h.\n\nSources : [Sleep Foundation](https://www.sleepfoundation.org/nutrition/caffeine-and-sleep) et https://fr.wikipedia.org/wiki/Caféine"), isStreaming: false)),
            ChatItem(.user(text: String(localized: "Voilà mes nuits de septembre, tu vois une tendance ?"), attachments: [sheet])),
            ChatItem(.tools([ToolEvent(tool: "terminal", preview: "python3 analyse_sommeil.py sommeil-septembre.xlsx", status: .started)])),
            ChatItem(.approval(ApprovalRequest(runID: "demo", requestID: "1", command: "pip install openpyxl", choices: [.once, .session, .always, .deny]), resolved: nil)),
            ChatItem(.user(text: "Message from 🤖 Vie (@vie): " + String(localized: "Alex part à Bordeaux samedi, quel train est le moins fatigant vu sa semaine ?"), attachments: [])),
            ChatItem(.assistant(text: String(localized: "Celui de 9h : sa semaine est chargée, mieux vaut arriver tôt et faire une sieste l’après-midi."), isStreaming: false)),
            ChatItem(.question(ClarifyRequest(runID: "demo", requestID: "clr", questions: [
                .init(id: "q1", question: String(localized: "Quel train pour Bordeaux ?"), choices: [String(localized: "9h — 19 €, arrivée 11h30"), String(localized: "14h — 35 €, arrivée 16h30")]),
            ]), state: .pending)),
        ]
    }

    // MARK: Loading

    func load() async {
        guard let client, let sessionID else { return }
        for attempt in 1...3 {
            do {
                try await loadOnce(client: client, sessionID: sessionID)
                errorMessage = nil
                return
            } catch let error as HermesError where error.isRetryable && attempt < 3 {
                // Right after unlocking, Tailscale needs a moment to bring the tunnel back.
                try? await Task.sleep(for: .seconds(Double(attempt)))
            } catch {
                errorMessage = Self.describe(error)
                return
            }
        }
    }

    private func loadOnce(client: HermesClient, sessionID: String) async throws {
        async let session = client.session(id: sessionID)
        async let history = client.messages(sessionID: sessionID)
        async let outbox = scheduledMessages()
        let current = try await session
        title = current.title
        let thread = Self.items(from: try await history)
        items = AgentStore.isBotChat(current) ? Self.merging(await outbox, into: thread) : thread
        await showPendingApprovals()
        store.noteSession(current, for: agent, preview: latestText)
        restoreVoiceNotes(sessionID: sessionID)
        restoreAttachments(sessionID: sessionID)
    }

    /// An approval still waiting for this conversation (seen by the bridge, e.g. asked while the user was away or
    /// before leaving the screen): shown again so the agent is never left waiting for a card that is gone.
    private func showPendingApprovals() async {
        guard let bridge, let sessionID, let pending = try? await bridge.pendingApprovals(agent: agent.bridgeName) else { return }
        for request in pending where request.sessionID == sessionID || request.runID == Self.activeRuns[sessionID] {
            guard !hasApprovalCard(for: request) else { continue }
            items.append(ChatItem(.approval(request, resolved: nil)))
            isWaitingForApproval = true
        }
    }

    private func hasApprovalCard(for request: ApprovalRequest) -> Bool {
        items.contains { if case .approval(let shown, _) = $0.kind { shown.id == request.id } else { false } }
    }

    /// The agent's recent scheduled-task reports and proactive messages (last 30 days), from its bridge.
    private func scheduledMessages() async -> [OutboxItem] {
        guard let bridge, sessionID?.hasPrefix("cron_") != true,
              let items = try? await bridge.outbox(agent: agent.bridgeName) else { return [] }
        let since = Date.now.addingTimeInterval(-30 * 24 * 3600)
        return items.filter { $0.agent == agent.bridgeName && $0.createdAt > since }
    }

    /// `scheduled` inserted by date among the thread's messages (undated messages keep their place).
    static func merging(_ scheduled: [OutboxItem], into thread: [ChatItem]) -> [ChatItem] {
        var result = thread
        for item in scheduled.sorted(by: { $0.createdAt < $1.createdAt }) {
            let index = result.firstIndex { ($0.date ?? .distantPast) > item.createdAt } ?? result.endIndex
            result.insert(ChatItem(.scheduled(item), date: item.createdAt), at: index)
        }
        return result
    }

    /// The audio of an outbox item (the podcast's mp3, or its synthesized voice), cached on the phone.
    func outboxAudio(_ item: OutboxItem) async throws -> URL {
        let folder = URL.cachesDirectory.appending(path: "outbox", directoryHint: .isDirectory)
        let key = SHA256.hash(data: Data(item.id.utf8)).prefix(8).map { String(format: "%02x", $0) }.joined()
        let file = folder.appending(path: "\(key).mp3")
        if FileManager.default.fileExists(atPath: file.path(percentEncoded: false)) { return file }
        guard let bridge else { throw HermesError.unsupported("bridge") }
        let data = try await bridge.audio(forOutboxItem: item.id)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try data.write(to: file, options: .atomic)
        return file
    }

    /// The thread from Hermes' stored messages. Consecutive tool calls share one card, and a tool's result
    /// (its own message, raw output) only completes the call already listed instead of adding a row.
    static func items(from history: [HermesMessage]) -> [ChatItem] {
        var result: [ChatItem] = []
        func addTools(_ events: [ToolEvent], date: Date?) {
            if case .tools(var tools)? = result.last?.kind {
                tools += events
                result[result.count - 1].kind = .tools(tools)
            } else {
                result.append(ChatItem(.tools(events), date: date))
            }
        }
        for message in history {
            let date = message.createdAt
            if message.role == .tool {
                let listed: [ToolEvent] = if case .tools(let tools)? = result.last?.kind { tools } else { [] }
                let fresh = message.toolEvents.filter { result in
                    !listed.contains { $0.callID != nil ? $0.callID == result.callID : $0.tool == result.tool }
                }
                if !fresh.isEmpty { addTools(fresh, date: date) }
                continue
            }
            if let reasoning = message.reasoning, !reasoning.isEmpty { result.append(ChatItem(.reasoning(text: reasoning), date: date)) }
            if !message.toolEvents.isEmpty { addTools(message.toolEvents, date: date) }
            switch message.role {
            case .user:
                result.append(ChatItem(.user(text: message.text, attachments: []), date: date))
            case .assistant:
                if !message.text.isEmpty { result.append(ChatItem(.assistant(text: message.text, isStreaming: false), date: date)) }
            case .system, .notice:
                if !message.text.isEmpty { result.append(ChatItem(.notice(message.text), date: date)) }
            default:
                break
            }
        }
        return result
    }

    // MARK: Sending

    func send(text: String, attachments: [LocalAttachment], voiceNote: VoiceRecording? = nil) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty, !isRunning else { return }
        guard let client else {
            errorMessage = String(localized: "Clé d’accès introuvable dans le Trousseau.")
            return
        }
        let item = ChatItem(.user(text: trimmed, attachments: attachments))
        items.append(item)
        if let voiceNote { voiceNotes[item.id] = voiceNote }
        replyByVoice = voiceNote != nil && VoiceReplyPolicy.current.shouldReplyByVoice(to: trimmed)
        isRunning = true
        errorMessage = nil
        beginBackgroundTask()

        streamTask = Task {
            do {
                let sessionID = try await ensureSession(client: client, firstMessage: trimmed)
                if var note = voiceNote {
                    note.url = VoiceNoteStore.shared.add(.user, text: trimmed, file: note.url, duration: note.duration,
                                                         waveform: note.waveform, sessionID: sessionID)
                    voiceNotes[item.id] = note
                }
                if !attachments.isEmpty {
                    // Hermes keeps only the text: the app keeps the files, to show them again later.
                    let stored = AttachmentStore.shared.add(text: trimmed, attachments: attachments, sessionID: sessionID)
                    if let index = items.firstIndex(where: { $0.id == item.id }) {
                        items[index].kind = .user(text: trimmed, attachments: stored)
                    }
                }
                let input = MessageInput(text: trimmed, attachments: attachments.map(\.hermesAttachment))
                // A run lives on the server whatever happens to this connection (screen locked, app in
                // background) and can be re-attached to afterwards. Photos go the same way unless the
                // server rejected them in a run before; a server that rejects runs falls back to chat/stream.
                let hasImages = input.attachments.contains { if case .image = $0 { true } else { false } }
                var handle: RunHandle?
                var ackLost = false
                if !(hasImages && Self.imagesUnavailableInRuns.contains(agent.id)), !Self.runsUnavailable.contains(agent.id) {
                    do {
                        handle = try await client.createRun(input: input, sessionID: sessionID,
                                                            idempotencyKey: UUID().uuidString, uploader: uploader)
                    } catch let error as HermesError where (error.status ?? 0) >= 500 {
                        // Seen on the server: the run starts, then building the 202 crashes. Never resend (the
                        // agent would do the task twice): wait for its reply in the transcript instead.
                        #if DEBUG
                        print("[run] POST /v1/runs answered \(error), waiting for the reply in the transcript")
                        #endif
                        ackLost = true
                    } catch let error as HermesError where [400, 404, 405, 413, 415, 422].contains(error.status ?? 0) {
                        // Rejected up front: nothing ran, chat/stream is safe. With photos, only photos fall back.
                        #if DEBUG
                        print("[run] POST /v1/runs rejected (\(error)), using chat/stream\(hasImages ? " for photos" : "") for this agent")
                        #endif
                        if hasImages { Self.imagesUnavailableInRuns.insert(agent.id) } else { Self.runsUnavailable.insert(agent.id) }
                    }
                }
                if ackLost {
                    await waitForReply(to: trimmed, client: client, sessionID: sessionID)
                } else if let handle {
                    // Hermes hands each run event to a single subscriber: `follow` goes through the bridge
                    // (which then also knows when to push), never bridge.watch + Hermes side by side.
                    runID = handle.runID
                    Self.activeRuns[sessionID] = handle.runID
                    Self.listeners[sessionID] = self
                    try await follow(runID: handle.runID, client: client)
                } else {
                    for try await event in client.chatStream(sessionID: sessionID, input: input, uploader: uploader) {
                        apply(event)
                    }
                }
            } catch is CancellationError {
            } catch let error as HermesError where error.isRetryable && runID != nil {
                // chat/stream dropped mid-reply: Hermes may have finished meanwhile; resync once active.
                await resyncWhenActive()
            } catch {
                errorMessage = Self.describe(error)
            }
            finishRun()
        }
    }

    /// Agents whose server rejected `POST /v1/runs` during this launch.
    private static var runsUnavailable: Set<UUID> = []
    /// Agents whose server rejected a run with photos during this launch (photos then use chat/stream).
    private static var imagesUnavailableInRuns: Set<UUID> = []
    /// Runs still in progress, by session: a reopened conversation re-attaches to its reply.
    private static var activeRuns: [String: String] = [:]
    /// The model currently following each session's run (only one subscriber gets the events).
    private static var listeners: [String: ConversationModel] = [:]
    private var detaching = false
    /// Stopped listening so another screen could follow the run (see `detach()`).
    private(set) var wasHandedOff = false

    /// Called when a run finishes while no conversation screen follows it (the app puts it in the Boîte).
    static var onMissedReply: ((_ agent: AgentProfile, _ sessionID: String, _ runID: String, _ text: String) -> Void)?
    private static var watchers: [String: Task<Void, Never>] = [:]

    /// The app went to the background: every screen stops listening, so the bridge — which pushes only when
    /// nobody follows a run — notifies when the reply is ready. A suspended app's connection can stay open
    /// for a long time otherwise. Screens re-attach when the app is active again.
    static func detachAll() {
        for model in Array(listeners.values) { model.detach() }
    }

    // MARK: Turns the agent takes on its own

    @ObservationIgnored private var watchTask: Task<Void, Never>?
    @ObservationIgnored private var knownMessageCount: Int?

    /// While the conversation is on screen: every few seconds, asks the bridge for its message count, which also
    /// tells the bridge not to push what is visible. A turn the agent took on its own (a teammate's answer,
    /// a routine) shows up without reopening the conversation.
    func startWatching() {
        guard watchTask == nil, let bridge else { return }
        watchTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(5))
                guard let self, !Task.isCancelled else { return }
                guard UIApplication.shared.applicationState == .active, let sessionID = self.sessionID else { continue }
                guard let count = try? await bridge.sessionMessageCount(agent: self.agent.bridgeName, sessionID: sessionID) else { continue }
                defer { self.knownMessageCount = count }
                // A run in progress (followed, or left while the phone was locked and about to be re-attached and
                // replayed) builds its own reply: reloading meanwhile would add it a second time.
                guard let known = self.knownMessageCount, count > known, !self.isRunning,
                      Self.activeRuns[sessionID] == nil else { continue }
                #if DEBUG
                print("[thread] \(sessionID): \(count - known) new message(s) taken by the agent, reloading")
                #endif
                await self.load()
            }
        }
    }

    func stopWatching() {
        watchTask?.cancel()
        watchTask = nil
        knownMessageCount = nil
    }

    /// The conversation left the screen: stop listening (the run goes on, `reattachIfNeeded` picks it up).
    /// Only one subscriber gets a run's events, so a hidden screen must not keep them.
    func detach() {
        guard streamTask != nil, let sessionID, let runID = Self.activeRuns[sessionID] else { return }
        detaching = true
        wasHandedOff = true
        if Self.listeners[sessionID] === self { Self.listeners[sessionID] = nil }
        streamTask?.cancel()
        if let client { Self.watchInBackground(runID: runID, sessionID: sessionID, agent: agent, client: client) }
    }

    /// Polls the run's status (which takes no events away from a screen) until it ends, unless a conversation
    /// screen re-attaches meanwhile; a finished reply nobody saw goes to `onMissedReply`.
    private static func watchInBackground(runID: String, sessionID: String, agent: AgentProfile, client: HermesClient) {
        guard watchers[runID] == nil else { return }
        watchers[runID] = Task {
            defer { watchers[runID] = nil }
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(4))
                guard activeRuns[sessionID] == runID, listeners[sessionID] == nil else { return }
                guard UIApplication.shared.applicationState == .active else { continue }
                let run: HermesRun
                do {
                    run = try await client.getRun(id: runID)
                } catch HermesError.http(404, _, _) {
                    activeRuns[sessionID] = nil // forgotten by the server: the transcript has the reply
                    return
                } catch {
                    continue
                }
                guard run.status.isTerminal else { continue }
                guard listeners[sessionID] == nil else { return }
                activeRuns[sessionID] = nil
                #if DEBUG
                print("[run] \(runID) finished while away: \(run.status)")
                #endif
                if run.status == .completed, let text = run.outcome.output, !text.isEmpty {
                    onMissedReply?(agent, sessionID, runID, text)
                }
                return
            }
        }
    }

    /// Follows the session's run in progress, if any (conversation reopened while the agent works).
    func reattachIfNeeded() {
        guard !isRunning, let client, let sessionID, let id = Self.activeRuns[sessionID] else { return }
        if let other = Self.listeners[sessionID], other !== self { other.detach() } // e.g. a quick voice note
        Self.listeners[sessionID] = self
        wasHandedOff = false
        #if DEBUG
        print("[run] reopening \(sessionID), re-attaching to \(id)")
        #endif
        isRunning = true
        errorMessage = nil
        runID = id
        streamTask = Task {
            do {
                try await follow(runID: id, client: client, reattaching: true)
            } catch is CancellationError {
            } catch {
                errorMessage = Self.describe(error)
            }
            finishRun()
        }
    }

    /// Bridges without `GET /v1/runs/{id}/events` (older version), by agent.
    private static var bridgeWithoutRunEvents: Set<UUID> = []

    /// Follows a run until it ends: through the bridge when it has the route (one Hermes subscription shared
    /// with the bridge's push logic, replayed from the start on every reconnection), else from Hermes directly.
    private func follow(runID: String, client: HermesClient, reattaching: Bool = false) async throws {
        if let bridge, !Self.bridgeWithoutRunEvents.contains(agent.id) {
            if try await followViaBridge(bridge, runID: runID, reattaching: reattaching) { return }
            // The bridge lost the run (restarted, gone after 15 min): Hermes still knows its status.
            try await followViaHermes(runID: runID, client: client, reattaching: true)
        } else {
            try await followViaHermes(runID: runID, client: client, reattaching: reattaching)
        }
    }

    /// Returns false when the bridge cannot follow this run (older bridge, run unknown to it).
    private func followViaBridge(_ bridge: BridgeClient, runID: String, reattaching: Bool) async throws -> Bool {
        let reconnecting = String(localized: "Reconnexion à l’agent…")
        var failures = 0
        var backoff = Backoff()
        var connections = 0
        while true {
            try Task.checkCancellation()
            if connections > 0 || reattaching {
                clearCurrentTurn() // the bridge replays the run from its start…
                await showPendingApprovals() // …but an approval it no longer has must stay answerable
            }
            if connections > 0 { interim = reconnecting }
            connections += 1
            do {
                for try await event in bridge.runEvents(agent: agent.bridgeName, runID: runID, sessionID: sessionID) {
                    failures = 0
                    backoff.reset()
                    if interim == reconnecting { interim = nil }
                    if event.type == "bridge.error" {
                        let code = event.raw["code"]?.stringValue
                        #if DEBUG
                        print("[run] bridge error on \(runID): \(code ?? "?")")
                        #endif
                        if code == "hermes_unreachable" { continue } // the bridge keeps retrying
                        return false
                    }
                    apply(event)
                    if event.isTerminal { return true }
                }
            } catch let error as HermesError where error.status == 404 {
                Self.bridgeWithoutRunEvents.insert(agent.id)
                return false
            } catch let error as HermesError where error.isRetryable {
                // Screen locked, app in background, Tailscale waking up: reconnect and replay.
                failures += 1
                #if DEBUG
                print("[run] bridge stream for \(runID) lost (\(error)), retry \(failures)")
                #endif
                if failures > 12 { throw error }
            }
            try await Task.sleep(for: backoff.next())
        }
    }

    /// Removes everything after the last user message (a replayed run rebuilds it).
    private func clearCurrentTurn() {
        guard let lastUser = items.lastIndex(where: { if case .user = $0.kind { true } else { false } }) else { return }
        items.removeSubrange(items.index(after: lastUser)...)
        isWaitingForApproval = false
    }

    /// Streams a run's events; after a disconnect, re-attaches (status poll + new subscription) until it ends.
    /// Whether a new subscription replays earlier events is unknown, so a re-attached reply is rebuilt from
    /// what arrives and finally replaced by the run's complete output.
    private func followViaHermes(runID: String, client: HermesClient, reattaching: Bool) async throws {
        var polls = 0
        var reattached = reattaching
        var sawTerminalEvent = false
        var finalOutput: String?
        for try await update in RunResumer(client: client).resume(runID: runID) {
            #if DEBUG
            if case .expired = update { print("[run] \(runID) expired") }
            #endif
            switch update {
            case .status(let run):
                polls += 1
                if polls > 1, !reattached {
                    reattached = true
                    dropPartialReply()
                    interim = String(localized: "Reconnexion à l’agent…")
                    #if DEBUG
                    print("[run] re-attaching to \(runID), status \(run.status)")
                    #endif
                }
                if run.status.isTerminal {
                    interim = nil
                    finalOutput = run.outcome.output
                    switch run.status {
                    case .failed: items.append(ChatItem(.notice(run.outcome.error ?? String(localized: "Le tour a échoué."))))
                    case .cancelled, .interrupted: items.append(ChatItem(.notice(String(localized: "La tâche a été interrompue côté agent."))))
                    default: break
                    }
                } else if reattached {
                    interim = nil
                }
            case .event(let event):
                if event.isTerminal { sawTerminalEvent = true }
                switch event.kind {
                case .assistantCompleted(let outcome), .runCompleted(let outcome):
                    finalOutput = outcome.output ?? finalOutput
                    if !reattached { apply(event) }
                default:
                    apply(event)
                }
            case .expired:
                break
            }
        }
        try Task.checkCancellation()
        // Normal case: the events already built the reply.
        guard reattached || !sawTerminalEvent else { return }
        if let finalOutput, !finalOutput.isEmpty {
            dropPartialReply()
            appendAssistant(finalOutput)
        } else if reattached || !hasStreamedAssistantText {
            await load() // nothing usable left in the run: the stored transcript is the truth
        }
    }

    /// The same reply twice in the current turn (a reload that landed while a re-attached run was replayed):
    /// keeps the first.
    private func dropRepeatedReplies() {
        guard let lastUser = items.lastIndex(where: { if case .user = $0.kind { true } else { false } }) else { return }
        var seen: Set<String> = []
        var index = items.index(after: lastUser)
        while index < items.endIndex {
            if case .assistant(let text, _) = items[index].kind {
                let key = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !key.isEmpty, !seen.insert(key).inserted {
                    items.remove(at: index)
                    continue
                }
            }
            index = items.index(after: index)
        }
    }

    /// Removes the assistant text of the current turn (after the last user message).
    private func dropPartialReply() {
        guard let lastUser = items.lastIndex(where: { if case .user = $0.kind { true } else { false } }) else { return }
        let turn = items.index(after: lastUser)...
        items.replaceSubrange(turn, with: items[turn].filter { if case .assistant = $0.kind { false } else { true } })
    }

    /// The run was submitted but its id never came back: polls the transcript until an assistant message
    /// follows `text`, for up to 15 minutes (the agent keeps working even if the app is suspended meanwhile).
    private func waitForReply(to text: String, client: HermesClient, sessionID: String) async {
        interim = String(localized: "L’agent travaille…")
        let deadline = ContinuousClock.now + .seconds(15 * 60)
        while ContinuousClock.now < deadline, !Task.isCancelled {
            try? await Task.sleep(for: .seconds(3))
            guard UIApplication.shared.applicationState == .active,
                  let history = try? await client.messages(sessionID: sessionID),
                  let asked = history.lastIndex(where: { $0.role == .user && $0.text.contains(text) })
            else { continue }
            if history[history.index(after: asked)...].contains(where: { $0.role == .assistant && !$0.text.isEmpty }) { break }
        }
        interim = nil
        await load()
    }

    /// After a chat/stream connection dropped: wait for the app to be active, then reload the thread.
    private func resyncWhenActive() async {
        interim = String(localized: "Reconnexion à l’agent…")
        while UIApplication.shared.applicationState != .active {
            try? await Task.sleep(for: .milliseconds(500))
            if Task.isCancelled { return }
        }
        interim = nil
        await load()
    }

    private func beginBackgroundTask() {
        endBackgroundTask()
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "hermes-reply") { [weak self] in
            self?.endBackgroundTask()
        }
    }

    private func endBackgroundTask() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }

    private func ensureSession(client: HermesClient, firstMessage: String) async throws -> String {
        if let sessionID { return sessionID }
        let title = firstMessage.isEmpty ? nil : String(firstMessage.prefix(60))
        let session = try await client.createSession(title: title)
        sessionID = session.id
        store.noteSession(session, for: agent)
        self.title = session.title ?? title
        return session.id
    }

    func stop() {
        guard let client, let runID else {
            streamTask?.cancel()
            return
        }
        Task { _ = try? await client.stop(runID: runID) }
    }

    func resolve(_ request: ApprovalRequest, with choice: ApprovalChoice) {
        markApproval(request, resolved: choice)
        guard let client else { return }
        Task {
            do {
                _ = try await client.approve(runID: request.runID, choice: choice, requestID: request.requestID)
                isWaitingForApproval = false
            } catch {
                markApproval(request, resolved: nil)
                errorMessage = Self.describe(error)
            }
        }
    }

    /// Answers the agent's question (`nil`: no answer, the agent goes on without one).
    func answer(_ request: ClarifyRequest, with answers: [String: String]?) {
        setQuestion(request.requestID, state: answers.map { .answered($0) } ?? .closed)
        guard let client else { return }
        Task {
            do {
                try await client.answerClarify(runID: request.runID, requestID: request.requestID, answers: answers)
                isWaitingForApproval = false
            } catch {
                setQuestion(request.requestID, state: .pending)
                errorMessage = Self.describe(error)
            }
        }
    }

    private func setQuestion(_ requestID: String?, state: QuestionState) {
        guard let index = items.lastIndex(where: { item in
            if case .question(let request, _) = item.kind { requestID == nil || request.requestID == requestID } else { false }
        }), case .question(let request, _) = items[index].kind else { return }
        items[index].kind = .question(request, state: state)
    }

    // MARK: Events

    private func apply(_ event: HermesEvent) {
        if let id = event.runID, id != runID {
            runID = id
            // Lets the bridge push the approval request if the app is closed meanwhile.
            if let bridge { Task { [agent] in try? await bridge.watch(agent: agent.bridgeName, runID: id) } }
        }
        switch event.kind {
        case .reasoning, .tool, .approvalRequest, .clarifyRequest:
            endStreamingText() // something else follows a text: that text is complete
        default:
            break // deltas, and unknown events that may sit between deltas of the same text
        }
        switch event.kind {
        case .delta(let text):
            interim = nil
            appendAssistant(text)
        case .interim(let text, let alreadyStreamed):
            if !alreadyStreamed { interim = text }
        case .reasoning(let text):
            if case .reasoning(let existing)? = items.last?.kind {
                items[items.count - 1].kind = .reasoning(text: existing + text)
            } else {
                items.append(ChatItem(.reasoning(text: text)))
            }
        case .tool(let tool):
            upsertTool(tool)
        case .approvalRequest(let request):
            isWaitingForApproval = true
            if !hasApprovalCard(for: request) { items.append(ChatItem(.approval(request, resolved: nil))) }
        case .approvalResponded(let choice, _):
            isWaitingForApproval = false
            if let choice, let index = items.lastIndex(where: { if case .approval(_, nil) = $0.kind { true } else { false } }),
               case .approval(let request, _) = items[index].kind {
                items[index].kind = .approval(request, resolved: choice)
            }
        case .clarifyRequest(let request):
            // A replayed question already answered keeps its answer (the responded event follows).
            if !items.contains(where: { if case .question(let r, _) = $0.kind { r.id == request.id } else { false } }) {
                isWaitingForApproval = true
                items.append(ChatItem(.question(request, state: .pending)))
            }
        case .clarifyResolved(let requestID, let answers):
            isWaitingForApproval = false
            setQuestion(requestID, state: answers.map { .answered($0) } ?? .closed)
        case .assistantCompleted(let outcome), .runCompleted(let outcome):
            if let output = outcome.output, !hasStreamedAssistantText { appendAssistant(output) }
        case .runFailed(let outcome):
            items.append(ChatItem(.notice(outcome.error ?? String(localized: "Le tour a échoué."))))
        case .runCancelled:
            items.append(ChatItem(.notice(String(localized: "Arrêté."))))
        case .runInterrupted(let outcome):
            items.append(ChatItem(.notice(outcome.error ?? String(localized: "Interrompu."))))
        default:
            break
        }
    }

    /// True when the current turn already produced assistant text (so `run.completed.output` is a duplicate).
    private var hasStreamedAssistantText: Bool {
        for item in items.reversed() {
            switch item.kind {
            case .assistant: return true
            case .user: return false
            default: continue
            }
        }
        return false
    }

    /// Marks every assistant text as complete (no cursor, « Écouter » available). Not only the last item:
    /// Hermes may send the reasoning after the reply.
    private func endStreamingText() {
        for index in items.indices {
            if case .assistant(let text, true) = items[index].kind { items[index].kind = .assistant(text: text, isStreaming: false) }
        }
    }

    private func appendAssistant(_ text: String) {
        if case .assistant(let existing, true)? = items.last?.kind {
            items[items.count - 1].kind = .assistant(text: existing + text, isStreaming: true)
        } else {
            items.append(ChatItem(.assistant(text: text, isStreaming: true)))
        }
    }

    private func upsertTool(_ tool: ToolEvent) {
        if case .tools(var tools)? = items.last?.kind {
            if tool.status != .started,
               let index = tools.lastIndex(where: { $0.status == .started && ($0.callID == tool.callID || ($0.callID == nil && $0.tool == tool.tool)) }) {
                var merged = tool
                if merged.preview == nil { merged.preview = tools[index].preview }
                tools[index] = merged
            } else {
                tools.append(tool)
            }
            items[items.count - 1].kind = .tools(tools)
        } else {
            items.append(ChatItem(.tools([tool])))
        }
    }

    private func markApproval(_ request: ApprovalRequest, resolved: ApprovalChoice?) {
        guard let index = items.lastIndex(where: { if case .approval(let r, _) = $0.kind { r.id == request.id } else { false } }) else { return }
        items[index].kind = .approval(request, resolved: resolved)
    }


    /// Re-attaches stored voice notes to the reloaded history (Hermes keeps only the text), in order.
    /// Puts the photos and files sent back on their messages (Hermes keeps only the text), in order.
    private func restoreAttachments(sessionID: String) {
        var pending = AttachmentStore.shared.entries(for: sessionID)
        guard !pending.isEmpty else { return }
        func normalized(_ text: String) -> String {
            String(text.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
        }
        for index in items.indices {
            guard case .user(let text, let current) = items[index].kind, current.isEmpty, !pending.isEmpty else { continue }
            let history = normalized(text)
            // The history may add document references to the text: a stored text found inside it matches.
            // A photo sent without text comes back empty or as a short placeholder.
            guard let match = pending.firstIndex(where: { entry in
                let sent = normalized(entry.text)
                return sent.isEmpty ? history.count <= 12 : history.contains(sent)
            }) else { continue }
            let entry = pending.remove(at: match)
            let restored = AttachmentStore.shared.attachments(of: entry)
            if !restored.isEmpty { items[index].kind = .user(text: entry.text, attachments: restored) }
        }
    }

    private func restoreVoiceNotes(sessionID: String) {
        var pending = VoiceNoteStore.shared.entries(for: sessionID)
        guard !pending.isEmpty else { return }
        // Hermes may not hand the text back byte for byte (prefixes, punctuation, spacing): compare letters
        // and digits only, and accept a stored text found inside the history's.
        func normalized(_ text: String) -> String {
            String(text.lowercased().unicodeScalars.filter { CharacterSet.alphanumerics.contains($0) })
        }
        func matches(_ stored: String, _ history: String) -> Bool {
            let a = normalized(stored), b = normalized(history)
            return !a.isEmpty && (a == b || b.contains(a))
        }
        for item in items {
            let (kind, text): (VoiceNoteStore.Entry.Kind, String)
            switch item.kind {
            case .user(let value, let attachments) where attachments.isEmpty: (kind, text) = (.user, value)
            case .assistant(let value, _): (kind, text) = (.reply, value)
            default: continue
            }
            guard let index = pending.firstIndex(where: { $0.kind == kind && matches($0.text, text) }) else { continue }
            let entry = pending.remove(at: index)
            let url = VoiceNoteStore.shared.url(of: entry)
            guard FileManager.default.fileExists(atPath: url.path(percentEncoded: false)) else { continue }
            switch kind {
            case .user:
                voiceNotes[item.id] = VoiceRecording(url: url, duration: entry.duration, transcript: entry.text, waveform: entry.waveform)
            case .reply:
                voiceReplies[item.id] = .ready(url, duration: entry.duration)
            }
        }
    }

    private func finishRun() {
        if detaching {
            // Left the screen mid-run: the run goes on and stays registered for re-attachment.
            detaching = false
            isRunning = false
            interim = nil
            streamTask = nil
            endBackgroundTask()
            return
        }
        if let sessionID {
            Self.activeRuns[sessionID] = nil
            if Self.listeners[sessionID] === self { Self.listeners[sessionID] = nil }
        }
        endStreamingText()
        dropRepeatedReplies()
        knownMessageCount = nil // our own turn: take the new count as the baseline, no reload
        if replyByVoice {
            replyByVoice = false
            prepareVoiceReply()
        }
        // Keep the home card's « last conversation » preview in step with the thread.
        if let sessionID, let latestText {
            store.noteSession(HermesSession(id: sessionID, title: title), for: agent, preview: latestText)
        }
        isRunning = false
        isWaitingForApproval = false
        interim = nil
        runID = nil
        streamTask = nil
        endBackgroundTask()
    }

    /// Voice note in → voice note out: the whole reply becomes one audio file, played as soon as it is ready.
    /// Voice note in, and the policy says the user can't (or asked not to) read: reply with a voice message.
    private func prepareVoiceReply() {
        // The turn's reply text: every assistant item after the last user message.
        guard let lastUser = items.lastIndex(where: { if case .user = $0.kind { true } else { false } }) else { return }
        let replies = items[(lastUser + 1)...].compactMap { item -> (UUID, String)? in
            if case .assistant(let text, _) = item.kind, !text.isEmpty { (item.id, text) } else { nil }
        }
        guard let target = replies.last else { return }
        generateVoice(for: target.0, text: replies.map(\.1).joined(separator: "\n\n"), anchor: target.1, autoplay: true)
    }

    /// Speaks `text` while Kyutai streams it (first audio in ~1 s), recording it as a voice note stored with the
    /// session under `itemID`. Falls back to the whole-file route, then to the live voice.
    private func generateVoice(for itemID: UUID, text: String, anchor: String, autoplay: Bool) {
        guard autoplay, let bridgeURL = agent.config.bridgeURL, let key = store.secrets(for: agent)?.bridgeKey else {
            return generateVoiceFile(for: itemID, text: text, anchor: anchor, autoplay: autoplay)
        }
        let tts = BridgeTTSProvider(bridgeURL: bridgeURL, bridgeKey: key, voice: agent.voice)
        player.stop()
        voiceTask?.cancel()
        voiceReplies[itemID] = .preparing
        voiceTask = Task {
            let url = VoiceNotePlayer.cacheURL(name: "reply-\(itemID.uuidString).m4a")
            let source = tts.stream(text)
            let chunks = AsyncThrowingStream<PCMChunk, any Error>.producing { yield in
                var first = true
                for try await chunk in source {
                    if first {
                        first = false
                        await MainActor.run { self.voiceReplies[itemID] = .streaming }
                    }
                    yield(chunk)
                }
            }
            do {
                let duration = try await streamer.play(chunks, recordingTo: url)
                var stored = url
                if let sessionID {
                    stored = VoiceNoteStore.shared.add(.reply, text: anchor, file: url, duration: duration, waveform: [], sessionID: sessionID)
                }
                voiceReplies[itemID] = .ready(stored, duration: duration)
            } catch is CancellationError {
                if voiceReplies[itemID] == .streaming || voiceReplies[itemID] == .preparing { voiceReplies[itemID] = nil }
            } catch {
                #if DEBUG
                print("[voice] streamed reply failed (\(error)), using the whole-file route")
                #endif
                generateVoiceFile(for: itemID, text: text, anchor: anchor, autoplay: autoplay)
            }
        }
    }

    /// One audio file for `text` from the bridge (Kyutai), stored with the session and shown under `itemID`.
    private func generateVoiceFile(for itemID: UUID, text: String, anchor: String, autoplay: Bool) {
        guard let bridge else {
            // No bridge: read it live with the on-device voice instead.
            speakLive(itemID: itemID, text: text)
            return
        }
        voiceReplies[itemID] = .preparing
        Task {
            do {
                let data = try await bridge.messageAudio(text: text, agent: agent.bridgeName, voice: agent.voice)
                var url = VoiceNotePlayer.cacheURL(name: "reply-\(itemID.uuidString).mp3")
                try data.write(to: url)
                let duration = VoiceNotePlayer.duration(of: url)
                if let sessionID {
                    url = VoiceNoteStore.shared.add(.reply, text: anchor, file: url, duration: duration, waveform: [], sessionID: sessionID)
                }
                voiceReplies[itemID] = .ready(url, duration: duration)
                if autoplay { player.toggle(url) }
            } catch {
                // Bridge without /v1/tts/message (older version) or unreachable: read it live instead.
                voiceReplies[itemID] = nil
                speakLive(itemID: itemID, text: text)
            }
        }
    }

    private func speakLive(itemID: UUID, text: String) {
        speakingItemID = itemID
        Task {
            await voice.speak(text)
            if speakingItemID == itemID { speakingItemID = nil }
        }
    }

    /// A file an agent pointed to with « MEDIA:<path> », downloaded once through the bridge and kept in Caches.
    func mediaFile(for path: String) async throws -> URL {
        let name = path.split(separator: "/").last.map(String.init) ?? "media"
        let folder = URL.cachesDirectory.appending(path: "media", directoryHint: .isDirectory)
        let key = SHA256.hash(data: Data(path.utf8)).prefix(8).map { String(format: "%02x", $0) }.joined() // stable across launches
        let file = folder.appending(path: "\(key)-\(name)")
        if FileManager.default.fileExists(atPath: file.path(percentEncoded: false)) { return file }
        guard let bridge else { throw HermesError.unsupported("bridge") }
        let data = try await bridge.media(path: path)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try data.write(to: file, options: .atomic)
        return file
    }

    /// Text of the newest user or assistant message (for previews).
    var latestText: String? {
        for item in items.reversed() {
            switch item.kind {
            case .assistant(let text, _) where !ChatText.visible(text).isEmpty: return ChatText.visible(text)
            case .user(let text, _) where !text.isEmpty: return text
            default: continue
            }
        }
        return nil
    }

    static func describe(_ error: any Error) -> String {
        switch error {
        case HermesError.unauthorized: String(localized: "Clé refusée par l’agent.")
        case HermesError.unreachable: String(localized: "Agent injoignable. Tailscale est-il connecté ?")
        case HermesError.tooManyRuns: String(localized: "L’agent est occupé, réessaie dans un instant.")
        case HermesError.documentUploaderUnavailable: String(localized: "Pour envoyer des fichiers, configure le bridge de cet agent dans les réglages.")
        case let error as HermesError: error.serverMessage ?? String(localized: "Erreur du serveur.")
        default: error.localizedDescription
        }
    }
}
