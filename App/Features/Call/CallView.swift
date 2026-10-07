import HermesKit
import SwiftUI
import VoiceKit

/// Full-screen voice call: the agent's mascot listens, thinks and talks.
struct CallView: View {
    let agent: AgentProfile
    var sessionID: String?

    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var model: CallModel?

    var body: some View {
        ZStack {
            agent.appearance.palette.tint.ignoresSafeArea()
            if let model {
                CallContent(model: model, hangUp: {
                    model.hangUp()
                    dismiss()
                })
            }
        }
        .toolbar(.hidden, for: .navigationBar)
        .tabBarHidden(true)
        .task {
            guard model == nil else { return }
            let model = CallModel(agent: agent, sessionID: sessionID, store: store)
            self.model = model
            await model.start()
        }
        .onDisappear { model?.hangUp() }
    }
}

private struct CallContent: View {
    @Bindable var model: CallModel
    var hangUp: () -> Void
    @Environment(Router.self) private var router

    private var palette: AgentPalette { model.agent.appearance.palette }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            statePills.padding(.top, 12)
            Spacer(minLength: 8)
            mascot
            Spacer(minLength: 8)
            transcript
            Spacer(minLength: 12)
            hint
            controls.padding(.top, 18)
        }
        .padding(.horizontal, 20)
        .padding(.bottom, 12)
        .sheet(item: Binding(get: { model.pendingApproval }, set: { _ in })) { request in
            ApprovalCard(request: request, resolved: nil, agent: model.agent) { choice in model.resolve(choice) }
                .padding()
                .presentationDetents([.medium])
                .interactiveDismissDisabled()
        }
        .sensoryFeedback(.impact(weight: .light), trigger: model.voice.state)
    }

    private var topBar: some View {
        HStack {
            Button(action: hangUp) {
                Image(systemName: "chevron.down").font(.system(size: 20, weight: .bold)).frame(width: 44, height: 44)
            }
            .accessibilityLabel("Réduire le live")
            Spacer()
            VStack(spacing: 0) {
                Text(model.agent.name).font(Theme.title(17))
                if let startedAt = model.startedAt {
                    Text(startedAt, style: .timer).font(Theme.body(13, weight: .bold)).foregroundStyle(Theme.ink2).monospacedDigit()
                }
            }
            Spacer()
            Color.clear.frame(width: 44, height: 44)
        }
        .foregroundStyle(Theme.ink)
    }

    private var statePills: some View {
        HStack(spacing: 4) {
            pill("Écoute", active: model.voice.state == .listening)
            pill("Réflexion", active: model.voice.state == .thinking)
            pill("Parole", active: model.voice.state == .speaking)
        }
    }

    private func pill(_ title: LocalizedStringKey, active: Bool) -> some View {
        HStack(spacing: 6) {
            if active { Circle().fill(palette.main).frame(width: 7, height: 7) }
            Text(title)
        }
        .font(Theme.body(13, weight: .heavy))
        .foregroundStyle(active ? palette.deep : Theme.ink2)
        .padding(.horizontal, 13)
        .frame(height: 30)
        .background(active ? Theme.card : .clear, in: .capsule)
        .animation(.snappy, value: active)
    }

    private var mascot: some View {
        Button { model.voice.interrupt() } label: {
            ZStack {
                if model.voice.state == .listening {
                    ListeningRipples(color: palette.main, level: model.voice.inputLevel)
                }
                MascotView(appearance: model.agent.appearance, mood: model.mood,
                           speakingLevel: model.voice.state == .speaking ? model.voice.outputLevel : nil)
                    .frame(width: 200, height: 200)
            }
            .frame(width: 280, height: 260)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(model.voice.state == .speaking ? String(localized: "Interrompre \(model.agent.name)") : model.agent.name)
    }

    @ViewBuilder
    private var transcript: some View {
        VStack(spacing: 12) {
            if let error = model.errorMessage ?? model.voice.lastError {
                Text(error).font(Theme.body(15, weight: .bold)).foregroundStyle(Theme.danger)
                if model.voice.state == .idle {
                    Button("Réessayer", systemImage: "arrow.clockwise") { Task { await model.retry() } }
                        .buttonStyle(.pill(.primary, height: 44))
                        .frame(maxWidth: 200)
                }
            }
            switch model.voice.state {
            case .listening, .idle:
                Text(model.voice.partialTranscript.isEmpty ? String(localized: "Je t’écoute…") : model.voice.partialTranscript)
                    .font(Theme.display(model.voice.partialTranscript.isEmpty ? 20 : 27))
                    .foregroundStyle(model.voice.partialTranscript.isEmpty ? palette.deep : Theme.ink)
                    .contentTransition(.opacity)
            case .thinking, .speaking:
                if let utterance = model.lastUtterance {
                    Text("« \(utterance) »").font(Theme.body(14, weight: .bold)).foregroundStyle(Theme.ink2)
                }
                if let tool = model.toolInProgress {
                    Label(tool.tool, systemImage: ToolCard.symbol(for: tool.tool))
                        .font(Theme.body(12, weight: .bold))
                        .foregroundStyle(Theme.ink2)
                        .padding(.horizontal, 11)
                        .frame(height: 28)
                        .background(Theme.card, in: .capsule)
                }
                Text(model.reply.isEmpty ? "…" : model.reply)
                    .font(Theme.display(23))
                    .foregroundStyle(Theme.ink)
                    .lineLimit(6)
                    .truncationMode(.head)
            }
        }
        .multilineTextAlignment(.center)
        .frame(maxWidth: .infinity)
        .animation(.snappy, value: model.voice.state)
    }

    private var hint: some View {
        Label(model.voice.state == .speaking ? String(localized: "Parle ou touche \(model.agent.name) pour l’interrompre") : String(localized: "Un petit silence et c’est envoyé"),
              systemImage: model.voice.state == .speaking ? "hand.raised" : "waveform")
            .font(Theme.body(13, weight: .bold))
            .foregroundStyle(Theme.ink2)
    }

    private var controls: some View {
        HStack {
            Spacer()
            Button {
                let route = AgentRoute.conversation(model.agent, sessionID: model.sessionID)
                hangUp()
                router.open(route)
            } label: {
                controlLabel("Clavier", systemImage: "keyboard", background: Theme.card, foreground: Theme.ink)
            }
            Spacer()
            Button { model.voice.interrupt() } label: {
                controlLabel("Interrompre", systemImage: "hand.raised.fill", background: Theme.card, foreground: Theme.ink)
            }
            .disabled(model.voice.state != .speaking)
            Spacer()
            Button(action: hangUp) {
                controlLabel("Terminer", systemImage: "xmark", background: Theme.danger, foreground: .white)
            }
            Spacer()
        }
        .buttonStyle(.plain)
    }

    private func controlLabel(_ title: LocalizedStringKey, systemImage: String, background: Color, foreground: Color) -> some View {
        VStack(spacing: 8) {
            Image(systemName: systemImage)
                .font(.system(size: 24, weight: .semibold))
                .foregroundStyle(foreground)
                .frame(width: 66, height: 66)
                .background(background, in: .circle)
                .shadow(color: .black.opacity(0.06), radius: 10, y: 6)
            Text(title).font(Theme.body(12, weight: .heavy)).foregroundStyle(Theme.ink2)
        }
    }
}

/// Concentric rings that breathe with the microphone level.
struct ListeningRipples: View {
    var color: Color
    var level: Double

    var body: some View {
        TimelineView(.animation) { timeline in
            let t = timeline.date.timeIntervalSinceReferenceDate
            ZStack {
                ForEach(0..<3) { index in
                    let phase = (t / 2.6 + Double(index) / 3).truncatingRemainder(dividingBy: 1)
                    Circle()
                        .fill(color.opacity(0.22 * (1 - phase)))
                        .frame(width: 250, height: 250)
                        .scaleEffect(0.7 + phase * (0.75 + level * 0.4))
                }
            }
        }
        .accessibilityHidden(true)
    }
}
