import AVKit
import QuickLook
import SwiftUI

/// « Aujourd’hui », « Hier », « lundi 5 octobre » between days of a conversation.
struct DaySeparator: View {
    var date: Date

    var body: some View {
        Text(label)
            .font(Theme.body(12.5, weight: .heavy))
            .foregroundStyle(Theme.muted)
            .padding(.horizontal, 12)
            .frame(height: 26)
            .background(Theme.card, in: .capsule)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
            .accessibilityAddTraits(.isHeader)
    }

    private var label: String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return String(localized: "Aujourd’hui") }
        if calendar.isDateInYesterday(date) { return String(localized: "Hier") }
        let sameYear = calendar.isDate(date, equalTo: .now, toGranularity: .year)
        let style = Date.FormatStyle.dateTime.weekday(.wide).day().month(.wide)
        return (sameYear ? date.formatted(style) : date.formatted(style.year())).capitalizedFirst
    }
}

/// Small « 07:53 » under a message.
struct TimeLabel: View {
    var date: Date?

    var body: some View {
        if let date {
            Text(date, format: .dateTime.hour().minute())
                .font(Theme.body(11.5, weight: .bold))
                .foregroundStyle(Theme.muted)
                .padding(.horizontal, 4)
        }
    }
}

/// A long prompt the user did not type (a scheduled task's brief, skill instructions), folded.
struct InstructionCard: View {
    var title: String
    var text: String
    var palette: AgentPalette
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 10) {
                    Image(systemName: "doc.text")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.deep)
                        .frame(width: 30, height: 30)
                        .background(palette.tint, in: .rect(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(title).font(Theme.body(14, weight: .heavy)).foregroundStyle(Theme.ink).lineLimit(1)
                        Text(expanded ? String(localized: "Toucher pour replier") : String(localized: "Toucher pour afficher"))
                            .font(Theme.body(12)).foregroundStyle(Theme.muted)
                    }
                    Spacer(minLength: 4)
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(Theme.muted)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if expanded {
                ScrollView {
                    Text(text)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Theme.ink2)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 320)
            }
        }
        .padding(12)
        .background(Theme.card, in: .rect(cornerRadius: 20, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Theme.line))
        .frame(maxWidth: 320, alignment: .trailing)
    }
}

/// A file an agent produced (« MEDIA:<path> »): audio plays inline, anything else opens in Quick Look.
struct MediaRow: View {
    var path: String
    var palette: AgentPalette
    var player: VoiceNotePlayer
    var load: (String) async throws -> URL
    /// A file merely mentioned (in a sub-task report): shown only if it exists.
    var hideIfMissing = false
    /// Lock screen / Dynamic Island title and agent for an audio file.
    var info: NowPlayingInfo? = nil

    @State private var file: URL?
    @State private var shareURL: URL?
    @State private var image: UIImage?
    @State private var duration: TimeInterval = 0
    @State private var failed = false
    @State private var quickLook: URL?
    @State private var viewing: ViewedPhoto?

    private var name: String { path.split(separator: "/").last.map(String.init) ?? path }
    private var isAudio: Bool { Self.isAudio(name) }
    private var isVideo: Bool { ["mp4", "m4v", "mov"].contains((name as NSString).pathExtension.lowercased()) }

    static func isAudio(_ path: String) -> Bool {
        ["mp3", "m4a", "aac", "wav", "ogg", "opus"].contains((path as NSString).pathExtension.lowercased())
    }

    static func isImage(_ path: String) -> Bool {
        ["png", "jpg", "jpeg", "gif", "webp", "heic"].contains((path as NSString).pathExtension.lowercased())
    }

    var body: some View {
        if failed && hideIfMissing {
            EmptyView()
        } else {
            content
        }
    }

    private var content: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if let file, isAudio {
                VoiceNoteControl(url: file, duration: duration, waveform: VoiceReplyView.placeholderWave, player: player,
                                 foreground: palette.deep, accent: palette.deep,
                                 info: info ?? NowPlayingInfo(title: NowPlayingInfo.title(fromFileName: name)), asCard: true)
            } else if let image {
                Button { viewing = ViewedPhoto(image: image) } label: {
                    Image(uiImage: image)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 240, maxHeight: 300)
                        .clipShape(.rect(cornerRadius: 18, style: .continuous))
                }
                .buttonStyle(.plain)
                .accessibilityLabel(name)
            } else if let file, isVideo {
                VideoClip(url: file)
            } else {
                Button {
                    if let file { quickLook = file } else { Task { await fetch() } }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: failed ? "exclamationmark.triangle" : (isAudio ? "waveform" : "doc"))
                        Text(failed ? String(localized: "Fichier indisponible") : name).lineLimit(1)
                        if file == nil && !failed { ProgressView().controlSize(.small) }
                    }
                    .font(Theme.body(13, weight: .bold))
                    .foregroundStyle(palette.deep)
                    .padding(.horizontal, 14)
                    .frame(height: 38)
                    .background(palette.tint, in: .capsule)
                }
                .buttonStyle(.plain)
            }
            // An audio card opens the full player, which has its own Share button.
            if let shareURL, !isAudio { ShareFileButton(url: shareURL, palette: palette) }
        }
        .padding(.leading, 40)
        .task { await fetch() }
        .quickLookPreview($quickLook)
        .fullScreenCover(item: $viewing) { photo in PhotoViewer(image: photo.image) }
    }

    private func fetch() async {
        guard file == nil else { return }
        do {
            let url = try await load(path)
            duration = VoiceNotePlayer.duration(of: url)
            if Self.isImage(name) { image = UIImage(contentsOfFile: url.path(percentEncoded: false)) }
            failed = false
            file = url
            shareURL = ShareFile.named(url, name)
        } catch {
            failed = true
        }
    }
}

/// A video an agent produced, played inline.
struct VideoClip: View {
    var url: URL
    @State private var player: AVPlayer?

    var body: some View {
        VideoPlayer(player: player)
            .frame(width: 260, height: 180)
            .clipShape(.rect(cornerRadius: 18, style: .continuous))
            .onAppear { if player == nil { player = AVPlayer(url: url) } }
            .onDisappear { player?.pause() }
    }
}

/// « Partager »: the share sheet (Save to Files, Save Image / Video to Photos, AirDrop, Messages…).
struct ShareFileButton: View {
    var url: URL
    var palette: AgentPalette

    var body: some View {
        ShareLink(item: url) {
            Image(systemName: "square.and.arrow.up")
                .font(.system(size: 15, weight: .bold))
                .foregroundStyle(palette.deep)
                .frame(width: 38, height: 38)
                .background(palette.tint, in: .circle)
        }
        .accessibilityLabel(String(localized: "Partager"))
    }
}

enum ShareFile {
    /// `file` under a readable name (« Podcast du matin.mp3 » rather than the cache's hashed name), for the share
    /// sheet: a copy in a temporary folder, or `file` itself if the copy fails.
    static func named(_ file: URL, _ name: String) -> URL {
        var clean = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        if (clean as NSString).pathExtension.isEmpty, !file.pathExtension.isEmpty { clean += "." + file.pathExtension }
        let folder = URL.temporaryDirectory.appending(path: "share/\(UUID().uuidString.prefix(8))", directoryHint: .isDirectory)
        let target = folder.appending(path: clean)
        do {
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: file, to: target)
            return target
        } catch {
            return file
        }
    }
}

/// A scheduled task's report (morning podcast, evening plan…) or a proactive message, at its time in the agent's
/// Discussion: the task's name above, then the agent's text and its audio, like any message from the agent.
struct ScheduledMessageRow: View {
    var item: OutboxItem
    var agent: AgentProfile
    var player: VoiceNotePlayer
    var audio: () async throws -> URL
    var media: (String) async throws -> URL

    private var palette: AgentPalette { agent.appearance.palette }

    /// « Podcast du matin · Oct 06 07:53 » → « Podcast du matin ».
    private var label: String {
        let name = item.title?.components(separatedBy: " · ").first?.trimmingCharacters(in: .whitespaces) ?? ""
        if !name.isEmpty { return name }
        return item.sessionID?.hasPrefix("cron_") == true ? String(localized: "Tâche planifiée") : String(localized: "Message proactif")
    }

    /// Files the message points to, except the audio already played from the outbox.
    private var files: [String] {
        ChatText.media(in: item.text).filter { !(item.hasAudio && MediaRow.isAudio($0)) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(label, systemImage: "clock.fill")
                .font(Theme.body(12.5, weight: .heavy))
                .foregroundStyle(palette.deep)
                .padding(.leading, 40)
            if !ChatText.visible(item.text).isEmpty {
                AssistantRow(appearance: agent.appearance, text: item.text, isStreaming: false)
                    .messageActions(item.text, in: agent)
            }
            if item.hasAudio {
                MediaRow(path: "\(label).mp3", palette: palette, player: player, load: { _ in try await audio() },
                         info: .agent(agent, title: label))
            }
            ForEach(files, id: \.self) { path in
                MediaRow(path: path, palette: palette, player: player, load: media,
                         info: .agent(agent, title: NowPlayingInfo.title(fromFileName: (path as NSString).lastPathComponent)))
            }
            TimeLabel(date: item.createdAt).padding(.leading, 40)
        }
    }
}

/// Options the agent offered, as buttons: a tap sends that option as the answer.
struct QuickReplies: View {
    var options: [String]
    var palette: AgentPalette
    var choose: (String) -> Void

    var body: some View {
        if !options.isEmpty {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(options, id: \.self) { option in
                    Button { choose(option) } label: {
                        HStack(spacing: 8) {
                            Image(systemName: "arrowshape.turn.up.left.fill")
                                .font(.system(size: 11, weight: .bold))
                            Text(option)
                                .multilineTextAlignment(.leading)
                                .lineLimit(2)
                        }
                        .font(Theme.body(14, weight: .bold))
                        .foregroundStyle(palette.deep)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 9)
                        .background(palette.tint, in: .rect(cornerRadius: 18, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).stroke(palette.main.opacity(0.35)))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Répondre : \(option)")
                }
            }
            .padding(.leading, 40)
        }
    }
}

/// A message from another agent (Hermes Bot Mode), with that agent's Bip.
struct TeammateCard: View {
    var name: String
    var message: String
    var appearance: AgentAppearance?
    var date: Date?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Group {
                if let appearance {
                    MascotAvatar(appearance: appearance, size: 30)
                } else {
                    Image(systemName: "person.2.fill").font(.system(size: 13)).frame(width: 30, height: 30)
                        .background(Theme.card, in: .circle)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                HStack(spacing: 6) {
                    Image(systemName: "arrowshape.turn.up.right.fill").font(.system(size: 10, weight: .bold))
                    Text("Message de \(name)")
                    Spacer(minLength: 0)
                    TimeLabel(date: date)
                }
                .font(Theme.body(12.5, weight: .heavy))
                .foregroundStyle(appearance?.palette.deep ?? Theme.ink2)
                Text(ChatText.visible(message))
                    .font(Theme.body(15))
                    .foregroundStyle(Theme.ink)
                    .textSelection(.enabled)
            }
            .padding(12)
            .background(appearance?.palette.tint ?? Theme.card, in: .rect(cornerRadius: 18, style: .continuous))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// An exchange with another agent, folded into one line (like the reasoning): « Échange avec Vie » for a
/// request from Vie and this agent's answer, « Réponse de Wellness » for an answer to this agent's request.
struct ExchangeChip: View {
    var name: String
    var isRequest: Bool
    var appearance: AgentAppearance?
    var isOpen: Bool
    var toggle: () -> Void

    var body: some View {
        Button(action: toggle) {
            HStack(spacing: 7) {
                if let appearance {
                    MascotAvatar(appearance: appearance, size: 20)
                } else {
                    Image(systemName: "person.2.fill").font(.system(size: 11))
                }
                Text(isRequest ? String(localized: "Échange avec \(name)") : String(localized: "Réponse de \(name)"))
                Image(systemName: isOpen ? "chevron.up" : "chevron.down").font(.system(size: 10, weight: .bold))
            }
            .font(Theme.body(13, weight: .bold))
            .foregroundStyle(Theme.ink2)
            .padding(.leading, 6)
            .padding(.trailing, 12)
            .frame(height: 30)
            .background(Theme.card, in: .capsule)
            .overlay(Capsule().stroke(Theme.line))
        }
        .buttonStyle(.plain)
        .padding(.leading, 40)
        .accessibilityHint(isOpen ? String(localized: "Replier l’échange") : String(localized: "Afficher l’échange"))
    }
}
