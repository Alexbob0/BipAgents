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
        if calendar.isDateInToday(date) { return "Aujourd’hui" }
        if calendar.isDateInYesterday(date) { return "Hier" }
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
                        Text(expanded ? "Toucher pour replier" : "Toucher pour afficher")
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

    @State private var file: URL?
    @State private var duration: TimeInterval = 0
    @State private var failed = false
    @State private var quickLook: URL?

    private var name: String { path.split(separator: "/").last.map(String.init) ?? path }
    private var isAudio: Bool { ["mp3", "m4a", "aac", "wav", "ogg", "opus"].contains((name as NSString).pathExtension.lowercased()) }

    var body: some View {
        Group {
            if let file, isAudio {
                VoiceNoteControl(url: file, duration: duration, waveform: VoiceReplyView.placeholderWave, player: player,
                                 foreground: palette.deep, accent: palette.deep)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(palette.tint, in: .capsule)
            } else {
                Button {
                    if let file { quickLook = file } else { Task { await fetch() } }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: failed ? "exclamationmark.triangle" : (isAudio ? "waveform" : "doc"))
                        Text(failed ? "Fichier indisponible" : name).lineLimit(1)
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
        }
        .padding(.leading, 40)
        .task { await fetch() }
        .quickLookPreview($quickLook)
    }

    private func fetch() async {
        guard file == nil else { return }
        do {
            let url = try await load(path)
            duration = VoiceNotePlayer.duration(of: url)
            failed = false
            file = url
        } catch {
            failed = true
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
