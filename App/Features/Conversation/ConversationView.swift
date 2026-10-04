import HermesKit
import SwiftUI

struct ConversationView: View {
    let agent: AgentProfile
    let sessionID: String?
    var start: ConversationStart = .none

    @Environment(AgentStore.self) private var store
    @Environment(InboxStore.self) private var inbox
    @Environment(\.scenePhase) private var scenePhase
    @State private var model: ConversationModel?

    var body: some View {
        Group {
            if let model {
                ConversationContent(model: model, start: start)
            } else {
                Color.clear
            }
        }
        .background(Theme.background)
        .task {
            if model == nil {
                let model = ConversationModel(agent: agent, sessionID: sessionID, store: store)
                self.model = model
                await model.load()
                model.reattachIfNeeded()
            }
            if let sessionID = model?.sessionID { inbox.dismissMissedReplies(sessionID: sessionID) }
        }
        .onAppear {
            model?.reattachIfNeeded()
            if let sessionID = model?.sessionID { inbox.dismissMissedReplies(sessionID: sessionID) }
        }
        .onDisappear { model?.detach() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model?.reattachIfNeeded() }
        }
    }
}

private struct ConversationContent: View {
    @Bindable var model: ConversationModel
    var start: ConversationStart
    @State private var isCalling = false
    @State private var draft = ""
    @State private var attachments: [LocalAttachment] = []

    private var palette: AgentPalette { model.agent.appearance.palette }

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 12) {
                if model.items.isEmpty && !model.isRunning {
                    ConversationEmptyState(agent: model.agent)
                }
                ForEach(model.items) { item in
                    row(for: item)
                }
                if model.isRunning, !model.isWaitingForApproval, !(model.items.last?.isStreamingAssistant ?? false) {
                    TypingIndicator(appearance: model.agent.appearance, interim: model.interim)
                }
                if let error = model.errorMessage {
                    NoticeRow(text: error, isError: true)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .defaultScrollAnchor(.bottom)
        .scrollDismissesKeyboard(.interactively)
        // A tap anywhere in the thread puts the keyboard away (buttons in it still work: simultaneous).
        .simultaneousGesture(TapGesture().onEnded {
            UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
        })
        .safeAreaInset(edge: .bottom) {
            Composer(
                text: $draft,
                attachments: $attachments,
                palette: palette,
                placeholder: "Message à \(model.agent.name)",
                isRunning: model.isRunning,
                voice: model.voice,
                onSend: send,
                onStop: model.stop,
                start: start,
                onDictated: { text, recording in
                    model.send(text: text, attachments: attachments, voiceNote: recording)
                    attachments = []
                },
                appearance: model.agent.appearance
            )
        }
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar) // full height for the thread, like messaging apps
        .fullScreenCover(isPresented: $isCalling) {
            NavigationStack { CallView(agent: model.agent, sessionID: model.sessionID) }
        }
        .toolbar {
            ToolbarItem(placement: .principal) {
                HStack(spacing: 8) {
                    MascotView(appearance: model.agent.appearance, mood: model.isRunning ? .thinking : .happy, animated: model.isRunning)
                        .frame(width: 34, height: 34)
                        .background(palette.tint, in: .circle)
                    VStack(alignment: .leading, spacing: 0) {
                        Text(model.agent.name).font(Theme.title(16))
                        if let title = model.title {
                            Text(title).font(Theme.body(12)).foregroundStyle(Theme.ink2).lineLimit(1)
                        }
                    }
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { isCalling = true } label: {
                    Image(systemName: "waveform").foregroundStyle(palette.deep)
                }
                .accessibilityLabel("Live avec \(model.agent.name)")
            }
        }
    }

    @ViewBuilder
    private func row(for item: ChatItem) -> some View {
        switch item.kind {
        case .user(let text, let attachments):
            if let note = model.voiceNotes[item.id], attachments.isEmpty {
                VoiceNoteBubble(note: note, player: model.player)
            } else {
                UserBubble(text: text, attachments: attachments)
            }
        case .assistant(let text, let isStreaming):
            VStack(alignment: .leading, spacing: 6) {
                AssistantRow(appearance: model.agent.appearance, text: text, isStreaming: isStreaming)
                if let reply = model.voiceReplies[item.id] {
                    VoiceReplyView(state: reply, palette: palette, player: model.player) { model.stopVoiceReply(item.id) }
                        .padding(.leading, 40)
                } else if !isStreaming {
                    ListenButton(palette: palette, isPlaying: model.speakingItemID == item.id) { model.toggleSpeech(of: item) }
                        .padding(.leading, 40)
                }
            }
        case .reasoning(let text):
            ReasoningChip(text: text)
        case .tools(let tools):
            ToolCard(tools: tools, palette: palette)
        case .approval(let request, let resolved):
            ApprovalCard(request: request, resolved: resolved, agent: model.agent) { choice in
                model.resolve(request, with: choice)
            }
        case .notice(let text):
            NoticeRow(text: text)
        }
    }

    private func send() {
        model.send(text: draft, attachments: attachments)
        draft = ""
        attachments = []
    }
}

private extension ChatItem {
    var isStreamingAssistant: Bool {
        if case .assistant(_, true) = kind { true } else { false }
    }
}

// MARK: - Rows

struct UserBubble: View {
    var text: String
    var attachments: [LocalAttachment]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(attachments) { attachment in
                AttachmentChip(attachment: attachment, onDark: true)
            }
            if !text.isEmpty {
                Text(text)
                    .font(Theme.body(16))
                    .padding(.horizontal, attachments.isEmpty ? 0 : 6)
                    .padding(.vertical, attachments.isEmpty ? 0 : 2)
            }
        }
        .foregroundStyle(Theme.onInk)
        .padding(attachments.isEmpty ? EdgeInsets(top: 10, leading: 14, bottom: 10, trailing: 14) : EdgeInsets(top: 6, leading: 6, bottom: 10, trailing: 6))
        .background(Theme.ink, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 22, bottomTrailingRadius: 6, topTrailingRadius: 22, style: .continuous))
        .frame(maxWidth: 300, alignment: .trailing)
        .frame(maxWidth: .infinity, alignment: .trailing)
        .textSelection(.enabled)
    }
}

struct AssistantRow: View {
    var appearance: AgentAppearance
    var text: String
    var isStreaming: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            MascotAvatar(appearance: appearance, size: 30)
            Text(markdown)
                .foregroundStyle(Theme.ink)
                .lineSpacing(3)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var markdown: AttributedString {
        let source = isStreaming ? text + " ▍" : text
        var result = (try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(source)
        // An explicit .font on the Text would flatten **bold**/*italic*; set the font per run instead.
        for run in result.runs {
            let intent = run.inlinePresentationIntent ?? []
            var font = Theme.body(16, weight: intent.contains(.stronglyEmphasized) ? .heavy : .medium)
            if intent.contains(.emphasized) { font = font.italic() }
            if intent.contains(.code) { font = .system(size: 15, design: .monospaced) }
            result[run.range].font = font
        }
        return result
    }
}

struct ListenButton: View {
    var palette: AgentPalette
    var isPlaying: Bool
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: isPlaying ? "stop.fill" : "play.fill")
                    .font(.system(size: 10, weight: .black))
                    .foregroundStyle(.white)
                    .frame(width: 24, height: 24)
                    .background(palette.deep, in: .circle)
                Text(isPlaying ? "Arrêter" : "Écouter")
                    .font(Theme.body(13, weight: .heavy))
                    .foregroundStyle(palette.deep)
            }
            .padding(.leading, 5)
            .padding(.trailing, 13)
            .frame(height: 34)
            .background(palette.tint, in: .capsule)
        }
        .buttonStyle(.plain)
    }
}

struct MascotAvatar: View {
    var appearance: AgentAppearance
    var size: CGFloat
    var mood: MascotMood = .happy

    var body: some View {
        MascotView(appearance: appearance, mood: mood, animated: false)
            .padding(size * 0.08)
            .frame(width: size, height: size)
            .background(appearance.palette.tint, in: .circle)
    }
}

struct ReasoningChip: View {
    var text: String
    @State private var isExpanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(.snappy) { isExpanded.toggle() }
            } label: {
                Label("Réflexion", systemImage: "lightbulb")
                    .font(Theme.body(13, weight: .bold))
                    .foregroundStyle(Theme.ink2)
                    .padding(.horizontal, 12)
                    .frame(height: 30)
                    .background(Theme.card, in: .capsule)
                    .overlay(Capsule().stroke(Theme.line))
            }
            .buttonStyle(.plain)
            if isExpanded {
                Text(text)
                    .font(Theme.body(14, weight: .medium))
                    .foregroundStyle(Theme.ink2)
                    .padding(12)
                    .background(Theme.card, in: .rect(cornerRadius: 16))
            }
        }
        .padding(.leading, 40)
    }
}

struct ToolCard: View {
    var tools: [ToolEvent]
    var palette: AgentPalette

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(tools.enumerated()), id: \.offset) { index, tool in
                if index > 0 { Divider().overlay(Theme.line) }
                HStack(spacing: 10) {
                    Image(systemName: Self.symbol(for: tool.tool))
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.deep)
                        .frame(width: 30, height: 30)
                        .background(palette.tint, in: .rect(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(tool.tool).font(Theme.mono).foregroundStyle(Theme.ink)
                        if let preview = tool.preview, !preview.isEmpty {
                            Text(preview).font(Theme.body(13)).foregroundStyle(Theme.ink2).lineLimit(1)
                        }
                    }
                    Spacer(minLength: 4)
                    status(tool)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
            }
        }
        .background(Theme.card, in: .rect(cornerRadius: 20, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Theme.line))
        .padding(.leading, 40)
    }

    @ViewBuilder
    private func status(_ tool: ToolEvent) -> some View {
        switch tool.status {
        case .started:
            ProgressView().controlSize(.small)
        case .completed:
            HStack(spacing: 4) {
                Image(systemName: tool.error == nil ? "checkmark" : "exclamationmark.triangle.fill")
                    .foregroundStyle(tool.error == nil ? palette.deep : Theme.danger)
                if let duration = tool.duration {
                    Text(duration, format: .number.precision(.fractionLength(1))).monospacedDigit() + Text(" s")
                }
            }
            .font(Theme.body(12, weight: .bold))
            .foregroundStyle(Theme.muted)
        case .failed:
            Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.danger)
        }
    }

    static func symbol(for tool: String) -> String {
        switch tool.lowercased() {
        case let t where t.contains("terminal") || t.contains("shell"): "terminal"
        case let t where t.contains("web") || t.contains("search") || t.contains("browser"): "globe"
        case let t where t.contains("memory"): "brain"
        case let t where t.contains("file") || t.contains("read") || t.contains("write"): "doc.text"
        case let t where t.contains("image") || t.contains("vision"): "photo"
        case let t where t.contains("cron") || t.contains("schedule"): "calendar"
        default: "wrench.and.screwdriver"
        }
    }
}

struct ApprovalCard: View {
    var request: ApprovalRequest
    var resolved: ApprovalChoice?
    var agent: AgentProfile
    var onChoice: (ApprovalChoice) -> Void

    private var palette: AgentPalette { agent.appearance.palette }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 12) {
                MascotView(appearance: agent.appearance, mood: resolved == nil ? .asking : .happy)
                    .padding(4)
                    .frame(width: 56, height: 56)
                    .background(palette.tint, in: .rect(cornerRadius: 20, style: .continuous))
                VStack(alignment: .leading, spacing: 2) {
                    Text(resolved == nil ? "J’ai besoin de ton accord" : "Décision envoyée")
                        .font(Theme.title(17))
                    Text(request.description ?? "\(agent.name) veut lancer cette commande")
                        .font(Theme.body(13.5))
                        .foregroundStyle(Theme.ink2)
                }
            }
            if let command = request.command {
                Text(command)
                    .font(.system(size: 13, design: .monospaced))
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(Theme.field, in: .rect(cornerRadius: 14))
            }
            if let resolved {
                Label(Self.title(for: resolved), systemImage: resolved == .deny ? "xmark" : "checkmark")
                    .font(Theme.body(15, weight: .heavy))
                    .foregroundStyle(resolved == .deny ? Theme.danger : palette.deep)
            } else {
                LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 8) {
                    ForEach(request.choices, id: \.self) { choice in
                        Button(Self.title(for: choice)) { onChoice(choice) }
                            .buttonStyle(.pill(Self.kind(for: choice), height: 46))
                    }
                }
            }
        }
        .padding(16)
        .background(Theme.card, in: .rect(cornerRadius: 28, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 28, style: .continuous).stroke(resolved == nil ? palette.main : Theme.line, lineWidth: 2))
        .shadow(color: palette.deep.opacity(resolved == nil ? 0.16 : 0), radius: 20, y: 10)
        .sensoryFeedback(.warning, trigger: request.id)
    }

    static func title(for choice: ApprovalChoice) -> String {
        switch choice {
        case .once: "Une fois"
        case .session: "Cette session"
        case .always: "Toujours"
        case .deny: "Refuser"
        }
    }

    private static func kind(for choice: ApprovalChoice) -> PillButtonStyle.Kind {
        switch choice {
        case .once: .primary
        case .deny: .danger
        default: .soft
        }
    }
}

struct NoticeRow: View {
    var text: String
    var isError = false

    var body: some View {
        Text(text)
            .font(Theme.body(13, weight: .bold))
            .foregroundStyle(isError ? Theme.danger : Theme.muted)
            .multilineTextAlignment(.center)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
    }
}

struct TypingIndicator: View {
    var appearance: AgentAppearance
    var interim: String?
    @State private var phase = false

    var body: some View {
        HStack(spacing: 10) {
            MascotView(appearance: appearance, mood: .thinking)
                .padding(2)
                .frame(width: 30, height: 30)
                .background(appearance.palette.tint, in: .circle)
            if let interim {
                Text(interim)
                    .font(Theme.body(14, weight: .medium))
                    .foregroundStyle(Theme.ink2)
                    .italic()
            } else {
                HStack(spacing: 5) {
                    ForEach(0..<3) { index in
                        Circle()
                            .fill(Theme.muted)
                            .frame(width: 7, height: 7)
                            .opacity(phase ? 1 : 0.3)
                            .animation(.easeInOut(duration: 0.7).repeatForever().delay(Double(index) * 0.2), value: phase)
                    }
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 11)
                .background(Theme.card, in: .rect(cornerRadius: 16))
                .onAppear { phase = true }
            }
        }
    }
}

struct ConversationEmptyState: View {
    var agent: AgentProfile

    var body: some View {
        VStack(spacing: 12) {
            InteractiveMascot(appearance: agent.appearance, size: 130)
            Text("Dis bonjour à \(agent.name)")
                .font(Theme.title(20))
            Text("Écris, envoie un fichier, ou maintiens le micro pour parler.")
                .font(Theme.body(15))
                .foregroundStyle(Theme.ink2)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 80)
    }
}
