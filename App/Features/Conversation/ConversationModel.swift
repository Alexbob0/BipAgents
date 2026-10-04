import Foundation
import HermesKit
import Observation
import VoiceKit

/// One row of the conversation thread.
struct ChatItem: Identifiable, Equatable {
    enum Kind: Equatable {
        case user(text: String, attachments: [LocalAttachment])
        case assistant(text: String, isStreaming: Bool)
        case reasoning(text: String)
        case tools([ToolEvent])
        case approval(ApprovalRequest, resolved: ApprovalChoice?)
        case notice(String)
    }

    let id: UUID
    var kind: Kind

    init(id: UUID = UUID(), _ kind: Kind) {
        self.id = id
        self.kind = kind
    }
}

/// A file or photo picked on the phone, before/after sending.
struct LocalAttachment: Identifiable, Equatable {
    enum Kind: Equatable { case image, document }
    let id = UUID()
    var kind: Kind
    var filename: String
    var mimeType: String
    var data: Data

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
        voice = VoiceEngine(tts: store.ttsProvider(for: agent))
    }

    /// "Écouter" on an assistant message (tap again to stop).
    /// « Écouter » on a reply: generates its voice message on demand (once, then replays it).
    func toggleSpeech(of item: ChatItem) {
        guard case .assistant(let text, _) = item.kind else { return }
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

    /// Sample thread for `-demo` (design review without a server).
    static func demoItems(for agent: AgentProfile) -> [ChatItem] {
        let sheet = LocalAttachment(kind: .document, filename: "sommeil-septembre.xlsx",
                                    mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", data: Data(count: 48_000))
        return [
            ChatItem(.user(text: "Je me suis couché tard hier. Je peux reprendre un café cet après-midi ?", attachments: [])),
            ChatItem(.reasoning(text: "Vérifier ses habitudes de sommeil et la demi-vie de la caféine.")),
            ChatItem(.tools([
                ToolEvent(tool: "memory", preview: "Habitudes de sommeil", status: .completed, duration: 0.3),
                ToolEvent(tool: "web_search", preview: "« caféine demi-vie sommeil »", status: .completed, duration: 1.9),
            ])),
            ChatItem(.assistant(text: "Pas cet après-midi : la caféine met 5 à 6 h à s’éliminer de moitié. Avec un coucher visé à 23 h 15, ta limite est **14 h**. Coup de barre ? Une sieste de 20 min avant 15 h.", isStreaming: false)),
            ChatItem(.user(text: "Voilà mes nuits de septembre, tu vois une tendance ?", attachments: [sheet])),
            ChatItem(.tools([ToolEvent(tool: "terminal", preview: "python3 analyse_sommeil.py sommeil-septembre.xlsx", status: .started)])),
            ChatItem(.approval(ApprovalRequest(runID: "demo", requestID: "1", command: "pip install openpyxl", choices: [.once, .session, .always, .deny]), resolved: nil)),
        ]
    }

    // MARK: Loading

    func load() async {
        guard let client, let sessionID else { return }
        do {
            async let session = client.session(id: sessionID)
            async let history = client.messages(sessionID: sessionID)
            let current = try await session
            title = current.title
            store.noteSession(current, for: agent)
            items = try await history.flatMap(Self.items(from:))
            restoreVoiceNotes(sessionID: sessionID)
        } catch {
            errorMessage = Self.describe(error)
        }
    }

    private static func items(from message: HermesMessage) -> [ChatItem] {
        var result: [ChatItem] = []
        if let reasoning = message.reasoning, !reasoning.isEmpty { result.append(ChatItem(.reasoning(text: reasoning))) }
        if !message.toolEvents.isEmpty { result.append(ChatItem(.tools(message.toolEvents))) }
        switch message.role {
        case .user:
            result.append(ChatItem(.user(text: message.text, attachments: [])))
        case .assistant:
            if !message.text.isEmpty { result.append(ChatItem(.assistant(text: message.text, isStreaming: false))) }
        case .system, .notice:
            if !message.text.isEmpty { result.append(ChatItem(.notice(message.text))) }
        default:
            break
        }
        return result
    }

    // MARK: Sending

    func send(text: String, attachments: [LocalAttachment], voiceNote: VoiceRecording? = nil) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty, !isRunning else { return }
        guard let client else {
            errorMessage = "Clé d’accès introuvable dans le Trousseau."
            return
        }
        let item = ChatItem(.user(text: trimmed, attachments: attachments))
        items.append(item)
        if let voiceNote { voiceNotes[item.id] = voiceNote }
        replyByVoice = voiceNote != nil && VoiceReplyPolicy.current.shouldReplyByVoice(to: trimmed)
        isRunning = true
        errorMessage = nil

        streamTask = Task {
            do {
                let sessionID = try await ensureSession(client: client, firstMessage: trimmed)
                if var note = voiceNote {
                    note.url = VoiceNoteStore.shared.add(.user, text: trimmed, file: note.url, duration: note.duration,
                                                         waveform: note.waveform, sessionID: sessionID)
                    voiceNotes[item.id] = note
                }
                let input = MessageInput(text: trimmed, attachments: attachments.map(\.hermesAttachment))
                for try await event in client.chatStream(sessionID: sessionID, input: input, uploader: uploader) {
                    apply(event)
                }
            } catch is CancellationError {
            } catch {
                errorMessage = Self.describe(error)
            }
            finishRun()
        }
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

    // MARK: Events

    private func apply(_ event: HermesEvent) {
        if let id = event.runID, id != runID {
            runID = id
            // Lets the bridge push the approval request if the app is closed meanwhile.
            if let bridge { Task { [agent] in try? await bridge.watch(agent: agent.bridgeName, runID: id) } }
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
            items.append(ChatItem(.approval(request, resolved: nil)))
        case .approvalResponded(let choice, _):
            isWaitingForApproval = false
            if let choice, let index = items.lastIndex(where: { if case .approval(_, nil) = $0.kind { true } else { false } }),
               case .approval(let request, _) = items[index].kind {
                items[index].kind = .approval(request, resolved: choice)
            }
        case .assistantCompleted(let outcome), .runCompleted(let outcome):
            if let output = outcome.output, !hasStreamedAssistantText { appendAssistant(output) }
        case .runFailed(let outcome):
            items.append(ChatItem(.notice(outcome.error ?? "Le tour a échoué.")))
        case .runCancelled:
            items.append(ChatItem(.notice("Arrêté.")))
        case .runInterrupted(let outcome):
            items.append(ChatItem(.notice(outcome.error ?? "Interrompu.")))
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
    private func restoreVoiceNotes(sessionID: String) {
        var pending = VoiceNoteStore.shared.entries(for: sessionID)
        #if DEBUG
        print("[voice-notes] session \(sessionID): \(pending.count) stored — \(pending.map { "\($0.kind.rawValue): \($0.text)" })")
        for item in items { if case .user(let text, _) = item.kind { print("[voice-notes] history user: \(text.debugDescription)") } }
        #endif
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
        #if DEBUG
        print("[voice-notes] restored \(voiceNotes.count) voice notes, \(voiceReplies.count) replies")
        #endif
    }

    private func finishRun() {
        if case .assistant(let text, true)? = items.last?.kind {
            items[items.count - 1].kind = .assistant(text: text, isStreaming: false)
        }
        if replyByVoice {
            replyByVoice = false
            prepareVoiceReply()
        }
        isRunning = false
        isWaitingForApproval = false
        interim = nil
        runID = nil
        streamTask = nil
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

    /// One audio file for `text` from the bridge (Kyutai), stored with the session and shown under `itemID`.
    private func generateVoice(for itemID: UUID, text: String, anchor: String, autoplay: Bool) {
        guard let bridge else {
            // No bridge: read it live with the on-device voice instead.
            speakLive(itemID: itemID, text: text)
            return
        }
        voiceReplies[itemID] = .preparing
        Task {
            do {
                let data = try await bridge.messageAudio(text: text, agent: agent.bridgeName)
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

    static func describe(_ error: any Error) -> String {
        switch error {
        case HermesError.unauthorized: "Clé refusée par l’agent."
        case HermesError.unreachable: "Agent injoignable. Tailscale est-il connecté ?"
        case HermesError.tooManyRuns: "L’agent est occupé, réessaie dans un instant."
        case HermesError.documentUploaderUnavailable: "Pour envoyer des fichiers, configure le bridge de cet agent dans les réglages."
        case let error as HermesError: error.serverMessage ?? "Erreur du serveur."
        default: error.localizedDescription
        }
    }
}
