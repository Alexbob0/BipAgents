import HermesKit
import QuickLook
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
    private static let bottom = "thread-bottom"
    @Bindable var model: ConversationModel
    var start: ConversationStart
    @State private var isCalling = false
    @State private var draft = ""
    @State private var attachments: [LocalAttachment] = []

    private var palette: AgentPalette { model.agent.appearance.palette }

    /// How many of the latest items are drawn (« Messages précédents » shows more).
    @State private var visibleCount = 80

    var body: some View {
        ScrollViewReader { proxy in
        ScrollView {
            // A plain VStack: a LazyVStack guesses row heights and, when the thread's height jumps (a reply
            // rebuilt after a reconnection), could leave the view scrolled past its content, blank.
            VStack(alignment: .leading, spacing: 12) {
                if model.items.count > visibleCount {
                    Button("Messages précédents", systemImage: "arrow.up") { visibleCount += 80 }
                        .buttonStyle(.pill(.soft, height: 40))
                        .frame(maxWidth: 240)
                        .frame(maxWidth: .infinity)
                }
                if model.items.isEmpty && !model.isRunning {
                    ConversationEmptyState(agent: model.agent)
                }
                let visible = Array(model.items.suffix(visibleCount))
                ForEach(Array(visible.enumerated()), id: \.element.id) { index, item in
                    if let day = newDay(at: index, in: visible) {
                        DaySeparator(date: day)
                    }
                    row(for: item, turnEnd: isTurnEnd(item))
                }
                if model.isRunning, !model.isWaitingForApproval, !(model.items.last?.isStreamingAssistant ?? false) {
                    TypingIndicator(appearance: model.agent.appearance, interim: model.interim)
                }
                if let error = model.errorMessage {
                    NoticeRow(text: error, isError: true)
                }
                Color.clear.frame(height: 1).id(Self.bottom)
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 12)
        }
        .defaultScrollAnchor(.bottom)
        .onChange(of: model.items.last?.id) { _, _ in
            // A message just sent (or the thread rebuilt): show the end of the conversation.
            if case .user? = model.items.last?.kind {
                withAnimation(.snappy) { proxy.scrollTo(Self.bottom, anchor: .bottom) }
            }
        }
        .onChange(of: model.isRunning) { _, running in
            if !running { proxy.scrollTo(Self.bottom, anchor: .bottom) }
        }
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
    private func row(for item: ChatItem, turnEnd: Bool) -> some View {
        switch item.kind {
        case .user(let text, let attachments):
            VStack(alignment: .trailing, spacing: 4) {
                if let title = ChatText.instructionTitle(for: text, inCronSession: model.sessionID?.hasPrefix("cron_") == true,
                                                          isFirstUserMessage: isFirstUser(item)) {
                    InstructionCard(title: title, text: text, palette: palette)
                } else if let note = model.voiceNotes[item.id], attachments.isEmpty {
                    VoiceNoteBubble(note: note, player: model.player)
                } else {
                    UserBubble(text: text, attachments: attachments)
                }
                TimeLabel(date: item.date)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
        case .assistant(let text, let isStreaming):
            if isStreaming || !ChatText.visible(text).isEmpty || !ChatText.media(in: text).isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    AssistantRow(appearance: model.agent.appearance, text: text, isStreaming: isStreaming)
                    if !isStreaming {
                        ForEach(ChatText.media(in: text), id: \.self) { path in
                            MediaRow(path: path, palette: palette, player: model.player, load: model.mediaFile(for:))
                        }
                    }
                    // « Écouter » and the time under the turn's final reply only, not under the agent's
                    // running commentary between tools.
                    if let reply = model.voiceReplies[item.id] {
                        VoiceReplyView(state: reply, palette: palette, player: model.player) { model.stopVoiceReply(item.id) }
                            .padding(.leading, 40)
                    } else if !isStreaming && turnEnd {
                        ListenButton(palette: palette, isPlaying: model.speakingItemID == item.id) { model.toggleSpeech(of: item) }
                            .padding(.leading, 40)
                    }
                    if turnEnd && !isStreaming {
                        TimeLabel(date: item.date).padding(.leading, 40)
                    }
                    // The agent asks to pick an option: one tap answers (latest reply only).
                    if !isStreaming, !model.isRunning, item.id == model.items.last?.id {
                        QuickReplies(options: ChatText.choices(in: text), palette: palette) { option in
                            model.send(text: option, attachments: [])
                        }
                    }
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
        case .question(let request, let state):
            QuestionCard(request: request, state: state, agent: model.agent) { answers in
                model.answer(request, with: answers)
            }
        case .notice(let text):
            NoticeRow(text: text)
        }
    }

    /// The last assistant text before the next message from the user (or the end): the turn's reply.
    private func isTurnEnd(_ item: ChatItem) -> Bool {
        guard let index = model.items.firstIndex(where: { $0.id == item.id }) else { return true }
        for next in model.items[(index + 1)...] {
            switch next.kind {
            case .user: return true
            case .assistant: return false
            default: continue
            }
        }
        return true
    }

    private func isFirstUser(_ item: ChatItem) -> Bool {
        model.items.first { if case .user = $0.kind { true } else { false } }?.id == item.id
    }

    /// The day to announce before `visible[index]` when it starts a new day.
    private func newDay(at index: Int, in visible: [ChatItem]) -> Date? {
        guard let date = visible[index].date else { return nil }
        let previous = visible[..<index].last { $0.date != nil }?.date
        guard let previous else { return index == 0 ? date : nil }
        return Calendar.current.isDate(previous, inSameDayAs: date) ? nil : date
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

    @State private var viewing: ViewedPhoto?
    @State private var quickLook: URL?

    private struct ViewedPhoto: Identifiable {
        let id = UUID()
        let image: UIImage
    }

    private var photos: [(LocalAttachment, UIImage)] {
        attachments.compactMap { attachment in PhotoThumbnails.image(for: attachment).map { (attachment, $0) } }
    }

    private var others: [LocalAttachment] {
        attachments.filter { PhotoThumbnails.image(for: $0) == nil }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            // Photos as real thumbnails: tap for full screen.
            ForEach(photos, id: \.0.id) { attachment, image in
                Button { viewing = ViewedPhoto(image: UIImage(data: attachment.data) ?? image) } label: {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFill()
                        .frame(width: thumbnailSize(of: image).width, height: thumbnailSize(of: image).height)
                        .clipShape(.rect(cornerRadius: 16, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Photo \(attachment.filename), toucher pour l’agrandir")
            }
            // Files: tap opens them in Quick Look when the app kept a copy.
            ForEach(others) { attachment in
                Button { quickLook = attachment.fileURL } label: {
                    AttachmentChip(attachment: attachment, onDark: true)
                }
                .buttonStyle(.plain)
                .allowsHitTesting(attachment.fileURL != nil)
            }
            if !text.isEmpty {
                Text(text)
                    .font(Theme.body(16))
                    .padding(.horizontal, attachments.isEmpty ? 0 : 6)
                    .padding(.vertical, attachments.isEmpty ? 0 : 2)
            }
        }
        .foregroundStyle(Theme.onInk)
        .padding(attachments.isEmpty ? EdgeInsets(top: 10, leading: 14, bottom: 10, trailing: 14) : EdgeInsets(top: 6, leading: 6, bottom: text.isEmpty ? 6 : 10, trailing: 6))
        .background(Theme.ink, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 22, bottomTrailingRadius: 6, topTrailingRadius: 22, style: .continuous))
        .frame(maxWidth: 300, alignment: .trailing)
        .frame(maxWidth: .infinity, alignment: .trailing)
        .textSelection(.enabled)
        .fullScreenCover(item: $viewing) { photo in PhotoViewer(image: photo.image) }
        .quickLookPreview($quickLook)
    }

    /// Keeps the photo's proportions within 240 × 240 pt.
    private func thumbnailSize(of image: UIImage) -> CGSize {
        let size = image.size
        guard size.width > 0, size.height > 0 else { return CGSize(width: 240, height: 240) }
        let scale = min(240 / size.width, 240 / size.height)
        return CGSize(width: max(80, size.width * scale), height: max(80, size.height * scale))
    }
}

struct AssistantRow: View {
    var appearance: AgentAppearance
    var text: String
    var isStreaming: Bool
    private let favicons = Favicons.shared

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            MascotAvatar(appearance: appearance, size: 30)
            rendered
                .foregroundStyle(Theme.ink)
                .lineSpacing(3)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// The reply with each link preceded by its site's icon (links open in Safari).
    private var rendered: Text {
        let attributed = markdown
        var result = Text("")
        var previousLink: URL?
        for run in attributed.runs {
            if let link = run.link, link != previousLink, let host = link.host() {
                let icon = favicons.icon(for: host).map { Image(uiImage: $0) } ?? Image(systemName: "globe")
                result = Text("\(result)\(Text(icon).font(.system(size: 13)).foregroundStyle(appearance.palette.deep).baselineOffset(-2)) ")
            }
            previousLink = run.link
            result = Text("\(result)\(Text(AttributedString(attributed[run.range])))")
        }
        return result
    }

    private var markdown: AttributedString {
        let shown = ChatText.visible(text)
        let source = isStreaming ? shown + " ▍" : shown
        var result = (try? AttributedString(markdown: source, options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace))) ?? AttributedString(source)
        Self.linkBareURLs(in: &result)
        // An explicit .font on the Text would flatten **bold**/*italic*; set the font per run instead.
        for run in result.runs {
            let intent = run.inlinePresentationIntent ?? []
            var font = Theme.body(16, weight: intent.contains(.stronglyEmphasized) ? .heavy : .medium)
            if intent.contains(.emphasized) { font = font.italic() }
            if intent.contains(.code) { font = .system(size: 15, design: .monospaced) }
            result[run.range].font = font
            if run.link != nil {
                result[run.range].foregroundColor = appearance.palette.deep
                result[run.range].underlineStyle = .single
            }
        }
        return result
    }

    private static let detector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)

    /// Plain "https://…" or "www.…" in the text become links too (not inside code), shown short:
    /// "fr.wikipedia.org › Caféine" rather than the whole address.
    private static func linkBareURLs(in text: inout AttributedString) {
        guard let detector else { return }
        let plain = String(text.characters)
        // Last match first, so replacing one does not move the next ones.
        for match in detector.matches(in: plain, range: NSRange(plain.startIndex..., in: plain)).reversed() {
            guard let url = match.url, url.scheme?.hasPrefix("http") == true,
                  let range = Range(match.range, in: plain),
                  let lower = AttributedString.Index(range.lowerBound, within: text),
                  let upper = AttributedString.Index(range.upperBound, within: text) else { continue }
            let span = lower..<upper
            // The markdown parser may already have linked it (autolink): the visible text is still the address.
            guard let first = text[span].runs.first,
                  text[span].runs.allSatisfy({ !($0.inlinePresentationIntent ?? []).contains(.code) }) else { continue }
            let target = first.link ?? url
            var label = AttributedString(shortLabel(for: target), attributes: first.attributes)
            label.link = target
            text.replaceSubrange(span, with: label)
        }
    }

    static func shortLabel(for url: URL) -> String {
        var host = url.host() ?? url.absoluteString
        if host.hasPrefix("www.") { host.removeFirst(4) }
        let last = url.lastPathComponent.removingPercentEncoding ?? url.lastPathComponent
        guard !last.isEmpty, last != "/" else { return host }
        let page = last.replacingOccurrences(of: "_", with: " ").replacingOccurrences(of: "-", with: " ")
        return host + " › " + (page.count > 32 ? String(page.prefix(31)) + "…" : page)
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
    @State private var expanded = false

    /// A long, finished series shows its first two tools and « N autres ».
    private var collapsed: Bool { !expanded && tools.count > 3 && !tools.contains { $0.status == .started } }
    private var shown: [ToolEvent] { collapsed ? Array(tools.prefix(2)) : tools }

    var body: some View {
        VStack(spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.offset) { index, tool in
                if index > 0 { Divider().overlay(Theme.line) }
                HStack(spacing: 10) {
                    Image(systemName: Self.symbol(for: tool.tool))
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.deep)
                        .frame(width: 30, height: 30)
                        .background(palette.tint, in: .rect(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(tool.tool).font(Theme.mono).foregroundStyle(Theme.ink)
                        if let preview = tool.preview.map { ChatText.toolSummary($0) }, !preview.isEmpty {
                            Text(preview).font(Theme.body(13)).foregroundStyle(Theme.ink2).lineLimit(1)
                        }
                    }
                    Spacer(minLength: 4)
                    status(tool)
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 9)
            }
            if collapsed {
                Divider().overlay(Theme.line)
                Button { withAnimation(.snappy) { expanded = true } } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "chevron.down")
                        Text("\(tools.count - shown.count) autres outils")
                    }
                    .font(Theme.body(13, weight: .heavy))
                    .foregroundStyle(palette.deep)
                    .frame(maxWidth: .infinity, minHeight: 36)
                    .contentShape(.rect)
                }
                .buttonStyle(.plain)
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
