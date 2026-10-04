import Foundation
import HermesKit
import Observation
import VoiceKit

/// Hands-free voice call with one agent: VoiceKit listens/speaks, Hermes answers in a session.
@Observable
final class CallModel {
    let agent: AgentProfile
    let voice: VoiceEngine
    private(set) var sessionID: String?
    private(set) var lastUtterance: String?
    private(set) var reply = ""
    private(set) var toolInProgress: ToolEvent?
    private(set) var pendingApproval: ApprovalRequest?
    private(set) var startedAt: Date?
    var errorMessage: String?

    private let store: AgentStore
    private let client: HermesClient?
    private let isDemo: Bool
    private var runID: String?

    init(agent: AgentProfile, sessionID: String?, store: AgentStore) {
        self.agent = agent
        self.sessionID = sessionID
        self.store = store
        client = store.client(for: agent)
        isDemo = store.isDemo
        voice = VoiceEngine(tts: store.ttsProvider(for: agent))
    }

    var mood: MascotMood {
        if pendingApproval != nil { return .asking }
        switch voice.state {
        case .idle: return .happy
        case .listening: return .listening
        case .thinking: return .thinking
        case .speaking: return .speaking
        }
    }

    func start() async {
        if isDemo {
            startedAt = .now
            return
        }
        guard client != nil else {
            errorMessage = "Clé d’accès introuvable."
            return
        }
        guard await VoiceEngine.requestPermissions() else {
            errorMessage = "Autorise le micro et la reconnaissance vocale dans Réglages."
            return
        }
        do {
            try await voice.startCall(onUtterance: { [weak self] text in
                self?.answer(text) ?? AsyncThrowingStream { $0.finish() }
            }, onInterrupt: { [weak self] in
                self?.stopRun()
            })
            startedAt = .now
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// After a failed start (mic busy, permission just granted…).
    func retry() async {
        errorMessage = nil
        voice.endCall()
        await start()
    }

    func hangUp() {
        voice.endCall()
        stopRun()
    }

    func resolve(_ choice: ApprovalChoice) {
        guard let request = pendingApproval, let client else { return }
        pendingApproval = nil
        Task { _ = try? await client.approve(runID: request.runID, choice: choice, requestID: request.requestID) }
    }

    /// Sends the utterance to Hermes and turns the event stream into text for the voice engine.
    private func answer(_ text: String) -> AsyncThrowingStream<String, any Error> {
        lastUtterance = text
        reply = ""
        toolInProgress = nil
        guard let client else { return AsyncThrowingStream { $0.finish() } }
        return AsyncThrowingStream { continuation in
            let task = Task { @MainActor in
                do {
                    let sessionID = try await self.ensureSession(client: client, title: text)
                    for try await event in client.chatStream(sessionID: sessionID, input: MessageInput(text: text)) {
                        if let id = event.runID { self.runID = id }
                        switch event.kind {
                        case .delta(let delta):
                            self.reply += delta
                            continuation.yield(delta)
                        case .tool(let tool):
                            self.toolInProgress = tool.status == .started ? tool : nil
                        case .approvalRequest(let request):
                            self.pendingApproval = request
                            continuation.yield("J’ai besoin de ton accord, regarde l’écran. ")
                        case .runCompleted(let outcome), .assistantCompleted(let outcome):
                            if self.reply.isEmpty, let output = outcome.output {
                                self.reply = output
                                continuation.yield(output)
                            }
                        case .runFailed(let outcome):
                            continuation.yield(outcome.error ?? "Désolé, ça n’a pas marché.")
                        default:
                            break
                        }
                    }
                    continuation.finish()
                } catch {
                    self.errorMessage = ConversationModel.describe(error)
                    continuation.finish(throwing: error)
                }
                self.runID = nil
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func ensureSession(client: HermesClient, title: String) async throws -> String {
        if let sessionID { return sessionID }
        let session = try await client.createSession(title: "Appel · " + String(title.prefix(40)))
        sessionID = session.id
        store.noteSession(session, for: agent)
        return session.id
    }

    private func stopRun() {
        guard let client, let runID else { return }
        self.runID = nil
        Task { _ = try? await client.stop(runID: runID) }
    }
}
