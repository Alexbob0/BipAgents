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

@Observable
final class ConversationModel {
    let agent: AgentProfile
    let voice: VoiceEngine
    private(set) var speakingItemID: UUID?
    private(set) var sessionID: String?
    private(set) var title: String?
    private(set) var items: [ChatItem] = []
    private(set) var runID: String?
    private(set) var isRunning = false
    private(set) var isWaitingForApproval = false
    private(set) var interim: String?
    var errorMessage: String?

    private let client: HermesClient?
    private let uploader: (any DocumentUploader)?
    private let bridge: BridgeClient?
    private var streamTask: Task<Void, Never>?

    init(agent: AgentProfile, sessionID: String?, store: AgentStore) {
        self.agent = agent
        self.sessionID = sessionID
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
    func toggleSpeech(of item: ChatItem) {
        guard case .assistant(let text, _) = item.kind else { return }
        if speakingItemID == item.id {
            voice.stopSpeaking()
            speakingItemID = nil
            return
        }
        speakingItemID = item.id
        Task {
            await voice.speak(text)
            if speakingItemID == item.id { speakingItemID = nil }
        }
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
            title = try await session.title
            items = try await history.flatMap(Self.items(from:))
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

    func send(text: String, attachments: [LocalAttachment]) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty || !attachments.isEmpty, !isRunning else { return }
        guard let client else {
            errorMessage = "Clé d’accès introuvable dans le Trousseau."
            return
        }
        items.append(ChatItem(.user(text: trimmed, attachments: attachments)))
        isRunning = true
        errorMessage = nil

        streamTask = Task {
            do {
                let sessionID = try await ensureSession(client: client, firstMessage: trimmed)
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

    private func finishRun() {
        if case .assistant(let text, true)? = items.last?.kind {
            items[items.count - 1].kind = .assistant(text: text, isStreaming: false)
        }
        isRunning = false
        isWaitingForApproval = false
        interim = nil
        runID = nil
        streamTask = nil
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
