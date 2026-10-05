import HermesKit
import Observation
import SwiftUI
import VoiceKit

/// Voice notes sent straight from the Agents screen (hold « Audio »): record, release to send into the
/// agent's ongoing conversation, and follow the reply on the card without opening the thread.
@Observable
final class QuickVoiceCenter {
    enum Status: Equatable {
        case recording(since: Date)
        case waiting
        case replied(String)
        case failed(String)
    }

    private(set) var status: [UUID: Status] = [:]
    let voice = VoiceEngine()

    /// Conversations kept alive until their turn finishes (the reply is followed even if nothing shows it).
    @ObservationIgnored private var conversations: [UUID: ConversationModel] = [:]

    func startRecording(_ agent: AgentProfile) {
        guard !isBusy else { return }
        status[agent.id] = .recording(since: .now)
        Task {
            guard await VoiceEngine.requestPermissions() else {
                status[agent.id] = .failed(String(localized: "Autorise le micro dans Réglages."))
                return
            }
            do {
                voice.configuration.locale = agent.language.locale
                try await voice.startDictation(recordingTo: VoiceNotePlayer.cacheURL(name: "note-\(UUID().uuidString).m4a"))
            } catch {
                status[agent.id] = .failed(String(localized: "Micro indisponible."))
            }
        }
    }

    func cancelRecording(_ agent: AgentProfile) {
        voice.cancelDictation()
        status[agent.id] = nil
    }

    func finishRecording(_ agent: AgentProfile, store: AgentStore) {
        Task {
            guard let recording = await voice.finishRecording() else {
                status[agent.id] = nil
                return
            }
            let transcript = recording.transcript.trimmingCharacters(in: .whitespacesAndNewlines)
            guard recording.duration >= 0.6, !transcript.isEmpty else {
                try? FileManager.default.removeItem(at: recording.url)
                status[agent.id] = .failed(String(localized: "Je n’ai rien entendu, réessaie."))
                return
            }
            send(recording, transcript: transcript, to: agent, store: store)
        }
    }

    func clear(_ agent: AgentProfile) {
        status[agent.id] = nil
    }

    var isBusy: Bool {
        status.values.contains { if case .recording = $0 { true } else { false } }
    }

    private func send(_ recording: VoiceRecording, transcript: String, to agent: AgentProfile, store: AgentStore) {
        let model = ConversationModel(agent: agent, sessionID: store.mainSessionID(for: agent), store: store)
        conversations[agent.id] = model
        status[agent.id] = .waiting
        model.send(text: transcript, attachments: [], voiceNote: recording)
        Task {
            while model.isRunning { try? await Task.sleep(for: .milliseconds(300)) }
            if model.wasHandedOff {
                // The conversation was opened and follows the reply itself.
                status[agent.id] = nil
                if conversations[agent.id] === model { conversations[agent.id] = nil }
                return
            }
            let reply = model.items.reversed().lazy.compactMap { item -> String? in
                if case .assistant(let text, _) = item.kind, !text.isEmpty { text } else { nil }
            }.first
            if let reply {
                status[agent.id] = .replied(reply)
            } else {
                status[agent.id] = .failed(model.errorMessage ?? String(localized: "Pas de réponse."))
            }
            // Keep the model a little longer if it is preparing a spoken reply (smart voice replies).
            try? await Task.sleep(for: .seconds(120))
            if conversations[agent.id] === model { conversations[agent.id] = nil }
        }
    }
}

/// « Audio » on an agent card: tap opens the conversation with a recording started; hold records right
/// here (release sends, slide away cancels).
///
/// A tap recognizer opens the conversation. The long press starts recording once the hold reaches 0.3 s,
/// and its `onPressingChanged(false)` — also sent when the scroll view cancels the touch — always ends it,
/// so a recording can never be left running. A side drag recognizer only measures how far the finger slid.
struct AudioHoldButton: View {
    var agent: AgentProfile
    var height: CGFloat = 50
    var onTap: () -> Void

    @Environment(QuickVoiceCenter.self) private var quick
    @Environment(AgentStore.self) private var store
    @State private var recordingStarted = false
    @State private var dragDistance: CGFloat = 0

    private var isRecording: Bool {
        if case .recording = quick.status[agent.id] { true } else { false }
    }

    var body: some View {
        Label(isRecording ? String(localized: "Relâche") : String(localized: "Audio"), systemImage: isRecording ? "waveform" : "mic.fill")
            .labelStyle(.compactPill)
            .font(Theme.body(16, weight: .heavy))
            .frame(maxWidth: .infinity, minHeight: height)
            .foregroundStyle(isRecording ? .white : Theme.ink)
            .background(isRecording ? Theme.danger : Theme.card, in: .capsule)
            .scaleEffect(isRecording ? 1.04 : 1)
            .animation(.snappy(duration: 0.15), value: isRecording)
            .contentShape(.capsule)
            .onTapGesture(perform: onTap) // short press; a hold past 0.3 s goes to the long press instead
            .onLongPressGesture(minimumDuration: 0.3, maximumDistance: .infinity) {
                recordingStarted = true
                quick.startRecording(agent)
            } onPressingChanged: { pressing in
                guard !pressing else {
                    dragDistance = 0
                    return
                }
                guard recordingStarted else { return }
                recordingStarted = false
                if dragDistance > 90 {
                    quick.cancelRecording(agent)
                } else {
                    quick.finishRecording(agent, store: store)
                }
            }
            .simultaneousGesture(
                DragGesture(minimumDistance: 8)
                    .onChanged { dragDistance = hypot($0.translation.width, $0.translation.height) }
            )
            .sensoryFeedback(.impact(weight: .medium), trigger: isRecording)
            .accessibilityLabel("Audio : touche pour ouvrir un message vocal, maintiens pour l’envoyer d’ici")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { onTap() }
    }
}

/// What the card shows about a voice note sent from it: recording, waiting, the reply, or an error.
struct QuickVoiceStatusView: View {
    var agent: AgentProfile
    var status: QuickVoiceCenter.Status
    var level: Double
    var onOpen: () -> Void
    var onDismiss: () -> Void

    var body: some View {
        Group {
            switch status {
            case .recording(let since):
                HStack(spacing: 10) {
                    Circle().fill(Theme.danger).frame(width: 9, height: 9)
                    Text(since, style: .timer).monospacedDigit().font(Theme.body(15, weight: .black))
                    LevelBars(level: level, color: agent.appearance.palette.main)
                    Spacer(minLength: 0)
                }
                .overlay(alignment: .bottomLeading) {
                    Text("Relâche pour envoyer · glisse pour annuler")
                        .font(Theme.body(11.5, weight: .bold))
                        .foregroundStyle(Theme.muted)
                        .offset(y: 18)
                }
                .padding(.bottom, 14)
            case .waiting:
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Envoyé · \(agent.name) réfléchit…")
                        .font(Theme.body(14, weight: .heavy))
                        .foregroundStyle(agent.appearance.palette.deep)
                    Spacer(minLength: 0)
                }
            case .replied(let text):
                Button(action: onOpen) {
                    VStack(alignment: .leading, spacing: 3) {
                        Text("Réponse de \(agent.name)")
                            .font(Theme.body(12.5, weight: .heavy))
                            .foregroundStyle(agent.appearance.palette.deep)
                        Text(text)
                            .font(Theme.body(15))
                            .foregroundStyle(Theme.ink)
                            .lineLimit(3)
                            .multilineTextAlignment(.leading)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
            case .failed(let message):
                HStack {
                    Text(message).font(Theme.body(14, weight: .bold)).foregroundStyle(Theme.danger)
                    Spacer()
                    Button("OK", action: onDismiss).font(Theme.body(14, weight: .heavy))
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
    }
}
