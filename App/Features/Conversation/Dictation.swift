import Observation
import SwiftUI
import VoiceKit

/// Push-to-talk state shared by the mic button (gesture) and the recording bar.
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
    private var lockTask: Task<Void, Never>?

    static let cancelDistance: CGFloat = 90
    /// Held this long, the recording locks by itself: the finger can let go, ↑ sends.
    static let lockDelay: Duration = .seconds(1)

    var isActive: Bool { phase != .idle }
    var willCancel: Bool { phase == .holding && dragOffset.width < -Self.cancelDistance }

    func begin(_ voice: VoiceEngine) {
        guard phase == .idle else { return }
        phase = .holding
        startedAt = .now
        failure = nil
        lockTask = Task {
            try? await Task.sleep(for: Self.lockDelay)
            if !Task.isCancelled, !willCancel { lock() }
        }
        Task {
            // First use: ask for mic + speech recognition here rather than failing silently.
            guard await VoiceEngine.requestPermissions() else {
                cancel(voice)
                failure = String(localized: "Autorise le micro et la reconnaissance vocale dans Réglages.")
                return
            }
            // Recorded too: the message is sent as a voice note (audio + transcript).
            let url = VoiceNotePlayer.cacheURL(name: "note-\(UUID().uuidString).m4a")
            do {
                try await voice.startDictation(recordingTo: url)
            } catch is CancellationError {
                // Released or cancelled while the mic was still starting: nothing went wrong.
            } catch {
                cancel(voice)
                failure = voice.lastError ?? String(localized: "Micro indisponible.")
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

    /// The person threw the recording away (slid to cancel, or the bin once locked): felt, like the press.
    func discard(_ voice: VoiceEngine) {
        cancel(voice)
        UIImpactFeedbackGenerator(style: .rigid).impactOccurred()
    }

    private func reset() {
        lockTask?.cancel()
        lockTask = nil
        phase = .idle
        startedAt = nil
        dragOffset = .zero
    }
}

/// Mic button: hold to record a voice note (release sends, slide left cancels; held a second, it locks: the finger
/// can let go, ↑ sends); a short tap only shows a hint.
struct DictationButton: View {
    var voice: VoiceEngine
    var controller: DictationController
    var palette: AgentPalette
    var onDictated: (String, VoiceRecording?) -> Void

    @State private var pressStart: Date?
    /// The press began on a locked recording: lifting the finger sends it.
    @State private var pressSends = false

    var body: some View {
        Image(systemName: controller.phase == .locked ? "arrow.up" : "mic.fill")
            .font(.system(size: 19, weight: .bold))
            .foregroundStyle(.white)
            .frame(width: 46, height: 46)
            .background(palette.deep, in: .circle)
            .scaleEffect(controller.phase == .holding ? 1.15 : 1)
            .animation(.easeOut(duration: 0.15), value: controller.phase)
        .contentShape(.circle)
        .overlay {
            PressSurface(onBegan: pressBegan, onMoved: pressMoved, onEnded: pressEnded)
                .accessibilityHidden(true)
        }
        // The press's own haptic is played by the touch surface, at once; this one marks the lock.
        .sensoryFeedback(.impact(weight: .medium), trigger: controller.phase) { _, phase in phase == .locked }
        .accessibilityLabel(controller.phase == .locked ? String(localized: "Envoyer le message vocal") : String(localized: "Maintenir pour enregistrer un message vocal"))
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

    private func pressBegan() {
        pressStart = .now
        pressSends = controller.phase == .locked
        if !pressSends { controller.begin(voice) }
    }

    private func pressMoved(_ translation: CGSize) {
        guard !pressSends, controller.phase == .holding else { return }
        controller.dragOffset = translation
    }

    /// `interrupted`: iOS took the touch back (a system gesture, a call…): the recording is dropped.
    private func pressEnded(interrupted: Bool) {
        defer {
            pressStart = nil
            pressSends = false
        }
        let held = pressStart.map { Date.now.timeIntervalSince($0) } ?? 0
        if pressSends {
            if controller.phase == .locked, !interrupted { send() }
            return
        }
        if controller.phase == .locked { return }
        if interrupted {
            controller.cancel(voice)
        } else if controller.willCancel {
            controller.discard(voice)
        } else if held < 0.3 {
            // A tap is not a recording: cancel and explain, like WhatsApp.
            controller.cancel(voice)
            controller.flashHoldHint()
        } else {
            send()
        }
    }

    private func send() {
        Task {
            let (text, recording) = await controller.finish(voice)
            if !text.isEmpty { onDictated(text, recording) }
        }
    }
}

/// The mic's touch surface, in UIKit: SwiftUI hands a touch to its gestures only once the screen's other recognizers
/// (back swipe, context menus…) have given up on it, about 0.75 s for a finger that does not move. A view gets its
/// touches as they happen, so the press (and its haptic) starts the moment the finger lands.
struct PressSurface: UIViewRepresentable {
    var onBegan: () -> Void
    /// The finger's offset from where it landed.
    var onMoved: (CGSize) -> Void
    var onEnded: (_ interrupted: Bool) -> Void

    func makeUIView(context: Context) -> SurfaceView {
        let view = SurfaceView()
        view.surface = self
        return view
    }

    func updateUIView(_ view: SurfaceView, context: Context) {
        view.surface = self
    }

    final class SurfaceView: UIView {
        var surface: PressSurface?
        private var origin: CGPoint?
        private let haptic = UIImpactFeedbackGenerator(style: .medium)

        override init(frame: CGRect) {
            super.init(frame: frame)
            backgroundColor = .clear
            isMultipleTouchEnabled = false
        }

        required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

        override func didMoveToWindow() {
            super.didMoveToWindow()
            if window != nil { haptic.prepare() }
        }

        override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard origin == nil, let touch = touches.first else { return }
            origin = touch.location(in: nil)
            haptic.impactOccurred()
            haptic.prepare()
            surface?.onBegan()
        }

        override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard let origin, let point = touches.first?.location(in: nil) else { return }
            surface?.onMoved(CGSize(width: point.x - origin.x, height: point.y - origin.y))
        }

        override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard origin != nil else { return }
            origin = nil
            surface?.onEnded(false)
        }

        override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
            guard origin != nil else { return }
            origin = nil
            surface?.onEnded(true)
        }
    }
}

/// In the composer's place while recording: a blinking dot (a bin once locked), the timer, then the transcript as it
/// comes; before it, how to cancel.
struct RecordingBar: View {
    var voice: VoiceEngine
    var controller: DictationController
    /// Shown while locked (no finger to slide away): discards the recording.
    var onCancel: () -> Void

    var body: some View {
        HStack(spacing: 10) {
            if controller.phase == .locked {
                Button(action: onCancel) {
                    Image(systemName: "trash")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(Theme.danger)
                        .frame(width: 30, height: 30)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Annuler le message vocal")
            } else {
                Circle().fill(Theme.danger).frame(width: 10, height: 10)
                    .phaseAnimator([1, 0.25]) { dot, opacity in dot.opacity(opacity) } animation: { _ in .easeInOut(duration: 0.6) }
                    .frame(width: 30, height: 30)
            }
            if let startedAt = controller.startedAt {
                Text(startedAt, style: .timer)
                    .monospacedDigit()
                    .font(Theme.body(15, weight: .heavy))
                    .foregroundStyle(Theme.ink)
                    .fixedSize()
            }
            line
                .font(Theme.body(15))
                .lineLimit(1)
                .truncationMode(.head)
                .frame(maxWidth: .infinity, alignment: .leading)
                .animation(.easeOut(duration: 0.25), value: voice.partialTranscript.isEmpty)
        }
        .padding(.leading, 8)
        .padding(.trailing, 14)
        .frame(minHeight: 46)
        .background(Theme.field, in: .rect(cornerRadius: 23, style: .continuous))
    }

    @ViewBuilder
    private var line: some View {
        if controller.willCancel {
            Text("Relâche pour annuler").foregroundStyle(Theme.danger)
        } else if !voice.partialTranscript.isEmpty {
            Text(voice.partialTranscript).foregroundStyle(Theme.ink).transition(.opacity)
        } else if controller.phase == .holding {
            Text("‹ Glisser pour annuler").foregroundStyle(Theme.muted)
        } else {
            Text("Touche ↑ pour envoyer").foregroundStyle(Theme.muted)
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
