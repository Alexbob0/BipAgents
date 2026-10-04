import Observation
import SwiftUI
import VoiceKit

/// Push-to-talk state shared by the mic button (gesture) and the live transcript panel.
@Observable
final class DictationController {
    enum Phase: Equatable { case idle, holding, locked }

    private(set) var phase: Phase = .idle
    private(set) var startedAt: Date?
    /// "Maintiens pour enregistrer", shown briefly after a too-short press (WhatsApp behaviour).
    private(set) var showsHoldHint = false
    /// Why the last recording could not start (permissions, busy mic…), shown in the composer.
    var failure: String?
    var dragOffset: CGSize = .zero
    private var hintTask: Task<Void, Never>?

    static let cancelDistance: CGFloat = 90
    static let lockDistance: CGFloat = 80

    var isActive: Bool { phase != .idle }
    var willCancel: Bool { phase == .holding && dragOffset.width < -Self.cancelDistance }

    func begin(_ voice: VoiceEngine) {
        guard phase == .idle else { return }
        phase = .holding
        startedAt = .now
        failure = nil
        Task {
            // First use: ask for mic + speech recognition here rather than failing silently.
            guard await VoiceEngine.requestPermissions() else {
                cancel(voice)
                failure = "Autorise le micro et la reconnaissance vocale dans Réglages."
                return
            }
            // Recorded too: the message is sent as a voice note (audio + transcript).
            let url = VoiceNotePlayer.cacheURL(name: "note-\(UUID().uuidString).m4a")
            do {
                try await voice.startDictation(recordingTo: url)
            } catch {
                cancel(voice)
                failure = voice.lastError ?? "Micro indisponible."
            }
        }
    }

    func flashHoldHint() {
        hintTask?.cancel()
        showsHoldHint = true
        hintTask = Task {
            try? await Task.sleep(for: .seconds(1.8))
            if !Task.isCancelled { showsHoldHint = false }
        }
    }

    func lock() {
        if phase == .holding { phase = .locked }
    }

    /// The transcript and, unless it was too short to be worth keeping, the recording.
    func finish(_ voice: VoiceEngine) async -> (text: String, recording: VoiceRecording?) {
        reset()
        guard var recording = await voice.finishRecording() else { return ("", nil) }
        recording.transcript = recording.transcript.trimmingCharacters(in: .whitespacesAndNewlines)
        guard recording.duration >= 0.6 else {
            try? FileManager.default.removeItem(at: recording.url)
            return (recording.transcript, nil)
        }
        return (recording.transcript, recording)
    }

    func cancel(_ voice: VoiceEngine) {
        voice.cancelDictation()
        reset()
    }

    private func reset() {
        phase = .idle
        startedAt = nil
        dragOffset = .zero
    }
}

/// Mic button: hold to record a voice note (release sends, slide left cancels, slide up locks); a short tap only shows a hint.
struct DictationButton: View {
    var voice: VoiceEngine
    var controller: DictationController
    var palette: AgentPalette
    var onDictated: (String, VoiceRecording?) -> Void

    @State private var pressStart: Date?

    var body: some View {
        ZStack {
            if controller.isActive {
                Circle().fill(palette.main.opacity(0.3))
                    .frame(width: 46, height: 46)
                    .scaleEffect(1.5 + voice.inputLevel * 0.8)
                    .animation(.easeOut(duration: 0.12), value: voice.inputLevel)
            }
            Image(systemName: controller.phase == .locked ? "arrow.up" : "mic.fill")
                .font(.system(size: 19, weight: .bold))
                .foregroundStyle(.white)
                .frame(width: 46, height: 46)
                .background(palette.deep, in: .circle)
                .scaleEffect(controller.phase == .holding ? 1.35 : 1)
                .offset(controller.phase == .holding ? CGSize(width: min(0, controller.dragOffset.width), height: min(0, controller.dragOffset.height)) : .zero)
        }
        .animation(.snappy(duration: 0.18), value: controller.phase)
        .contentShape(.circle)
        .gesture(controller.phase == .locked ? nil : holdGesture)
        .onTapGesture { if controller.phase == .locked { send() } }
        .sensoryFeedback(.impact(weight: .medium), trigger: controller.phase)
        .accessibilityLabel(controller.phase == .locked ? "Envoyer le message vocal" : "Maintenir pour enregistrer un message vocal")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction {
            // VoiceOver can't hold: first activation records (locked), the second sends.
            if controller.phase == .idle {
                controller.begin(voice)
                controller.lock()
            } else {
                send()
            }
        }
    }

    private var holdGesture: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if pressStart == nil {
                    pressStart = .now
                    controller.begin(voice)
                }
                controller.dragOffset = value.translation
                if value.translation.height < -DictationController.lockDistance { controller.lock() }
            }
            .onEnded { _ in
                defer { pressStart = nil }
                let held = pressStart.map { Date.now.timeIntervalSince($0) } ?? 0
                if controller.phase == .locked { return }
                if held < 0.3 {
                    // A tap is not a recording: cancel and explain, like WhatsApp.
                    controller.cancel(voice)
                    controller.flashHoldHint()
                } else if controller.willCancel {
                    controller.cancel(voice)
                } else {
                    send()
                }
            }
    }

    private func send() {
        Task {
            let (text, recording) = await controller.finish(voice)
            if !text.isEmpty { onDictated(text, recording) }
        }
    }
}

/// Live transcript card shown above the composer while dictating.
struct DictationPanel: View {
    var voice: VoiceEngine
    var controller: DictationController
    var appearance: AgentAppearance
    /// Shown while locked (no finger to slide away): discards the recording.
    var onCancel: () -> Void = {}

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                if let startedAt = controller.startedAt {
                    HStack(spacing: 6) {
                        Circle().fill(Theme.danger).frame(width: 9, height: 9)
                        Text(startedAt, style: .timer).monospacedDigit()
                    }
                    .font(Theme.body(15, weight: .black))
                }
                Spacer()
                if controller.phase == .locked {
                    Button(action: onCancel) {
                        Image(systemName: "xmark")
                            .font(.system(size: 13, weight: .black))
                            .foregroundStyle(Theme.ink2)
                            .frame(width: 30, height: 30)
                            .background(Theme.field, in: .circle)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Annuler le message vocal")
                }
                Text(hint)
                    .font(Theme.body(12, weight: .heavy))
                    .foregroundStyle(controller.willCancel ? Theme.danger : appearance.palette.deep)
            }
            LevelBars(level: voice.inputLevel, color: appearance.palette.main)
            Text(voice.partialTranscript.isEmpty ? "Parle, je transcris…" : voice.partialTranscript)
                .font(Theme.display(22))
                .foregroundStyle(voice.partialTranscript.isEmpty ? Theme.muted : Theme.ink)
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(.snappy, value: voice.partialTranscript)
        }
        .padding(18)
        .padding(.top, 8)
        .background(Theme.card, in: .rect(cornerRadius: 28, style: .continuous))
        .overlay(alignment: .topLeading) {
            MascotView(appearance: appearance, mood: .listening)
                .frame(width: 70, height: 70)
                .offset(x: 14, y: -54)
        }
        .shadow(color: .black.opacity(0.14), radius: 24, y: 12)
        .padding(.horizontal, 12)
        .opacity(controller.willCancel ? 0.5 : 1)
        .transition(.move(edge: .bottom).combined(with: .opacity))
    }

    private var hint: String {
        switch controller.phase {
        case .locked: "Verrouillé · touche ↑ pour envoyer"
        case .holding where controller.willCancel: "Relâche pour annuler"
        default: "‹ Glisser pour annuler · ↑ verrouiller"
        }
    }
}

/// Microphone level as a row of bars scrolling with time.
struct LevelBars: View {
    var level: Double
    var color: Color
    @State private var history: [Double] = Array(repeating: 0.05, count: 32)

    var body: some View {
        HStack(alignment: .center, spacing: 3) {
            ForEach(Array(history.enumerated()), id: \.offset) { _, value in
                Capsule().fill(color).frame(width: 4, height: 6 + 30 * value)
            }
        }
        .frame(height: 36)
        .onChange(of: level) { _, newValue in
            history.removeFirst()
            history.append(min(1, max(0.05, newValue)))
        }
        .accessibilityHidden(true)
    }
}
