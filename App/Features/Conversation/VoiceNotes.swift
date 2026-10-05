import AVFoundation
import Observation
import SwiftUI
import VoiceKit

/// Plays voice notes (the user's recordings and the agent's spoken replies), one at a time.
@Observable
final class VoiceNotePlayer {
    private(set) var playingURL: URL?
    private(set) var progress: Double = 0

    @ObservationIgnored private var player: AVAudioPlayer?
    @ObservationIgnored private var ticker: Task<Void, Never>?

    func toggle(_ url: URL) {
        if playingURL == url {
            stop()
            return
        }
        stop()
        do {
            try AVAudioSession.sharedInstance().setCategory(.playback, mode: .spokenAudio)
            try AVAudioSession.sharedInstance().setActive(true)
            let player = try AVAudioPlayer(contentsOf: url)
            player.play()
            self.player = player
            AudioSessionUsage.begin()
            playingURL = url
            ticker = Task { [weak self] in
                while let self, let player = self.player, !Task.isCancelled {
                    self.progress = player.duration > 0 ? player.currentTime / player.duration : 0
                    if !player.isPlaying { self.stop(); return }
                    try? await Task.sleep(for: .milliseconds(100))
                }
            }
        } catch {
            stop()
        }
    }

    func stop() {
        ticker?.cancel()
        ticker = nil
        if let player {
            player.stop()
            AudioSessionUsage.end()
        }
        player = nil
        playingURL = nil
        progress = 0
    }

    static func cacheURL(name: String) -> URL {
        let folder = URL.cachesDirectory.appending(path: "voice-notes", directoryHint: .isDirectory)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        return folder.appending(path: name)
    }

    static func duration(of url: URL) -> TimeInterval {
        (try? AVAudioPlayer(contentsOf: url))?.duration ?? 0
    }
}

/// Play button + waveform + duration, as in messaging apps.
struct VoiceNoteControl: View {
    var url: URL
    var duration: TimeInterval
    var waveform: [Float]
    var player: VoiceNotePlayer
    var foreground: Color
    var accent: Color

    private var isPlaying: Bool { player.playingURL == url }

    var body: some View {
        Button { player.toggle(url) } label: {
            HStack(spacing: 10) {
                Image(systemName: isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 13, weight: .black))
                    .foregroundStyle(accent == foreground ? Theme.ink : .white)
                    .frame(width: 32, height: 32)
                    .background(accent, in: .circle)
                HStack(spacing: 1.5) {
                    ForEach(Array(waveform.enumerated()), id: \.offset) { index, level in
                        let played = isPlaying && Double(index) / Double(max(waveform.count, 1)) < player.progress
                        Capsule()
                            .fill(foreground.opacity(played || !isPlaying ? 1 : 0.45))
                            .frame(width: 2.5, height: 4 + 22 * CGFloat(level))
                    }
                }
                .frame(height: 28)
                Text(Duration.seconds(duration), format: .time(pattern: .minuteSecond))
                    .font(Theme.body(13, weight: .heavy))
                    .monospacedDigit()
                    .foregroundStyle(foreground)
                    .lineLimit(1)
                    .fixedSize()
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(isPlaying ? "Pause" : "Écouter le message vocal")
    }
}

/// The user's voice note: audio on top, transcript below (what was actually sent to the agent).
struct VoiceNoteBubble: View {
    var note: VoiceRecording
    var player: VoiceNotePlayer

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VoiceNoteControl(url: note.url, duration: note.duration, waveform: note.waveform, player: player,
                             foreground: Theme.onInk, accent: Theme.onInk)
            if !note.transcript.isEmpty {
                Text(note.transcript)
                    .font(Theme.body(15))
                    .foregroundStyle(Theme.onInk)
            }
        }
        .padding(EdgeInsets(top: 10, leading: 12, bottom: 12, trailing: 14))
        .background(Theme.ink, in: UnevenRoundedRectangle(topLeadingRadius: 22, bottomLeadingRadius: 22, bottomTrailingRadius: 6, topTrailingRadius: 22, style: .continuous))
        .frame(maxWidth: 300, alignment: .trailing)
        .frame(maxWidth: .infinity, alignment: .trailing)
    }
}

/// The agent's spoken reply under its text: preparing, then a playable voice note.
struct VoiceReplyView: View {
    var state: VoiceReplyState
    var palette: AgentPalette
    var player: VoiceNotePlayer
    var onStop: () -> Void = {}

    var body: some View {
        switch state {
        case .preparing:
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Préparation du message vocal…")
                    .font(Theme.body(13, weight: .bold))
                    .foregroundStyle(palette.deep)
            }
            .padding(.horizontal, 12)
            .frame(height: 38)
            .background(palette.tint, in: .capsule)
        case .streaming:
            Button(action: onStop) {
                HStack(spacing: 8) {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(.white)
                        .frame(width: 28, height: 28)
                        .background(palette.deep, in: .circle)
                    Image(systemName: "waveform")
                        .symbolEffect(.variableColor.iterative, isActive: true)
                        .foregroundStyle(palette.deep)
                    Text("Lecture…")
                        .font(Theme.body(13, weight: .bold))
                        .foregroundStyle(palette.deep)
                }
                .padding(.leading, 5)
                .padding(.trailing, 14)
                .frame(height: 38)
                .background(palette.tint, in: .capsule)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Arrêter la lecture")
        case .ready(let url, let duration):
            VoiceNoteControl(url: url, duration: duration, waveform: Self.placeholderWave, player: player,
                             foreground: palette.deep, accent: palette.deep)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(palette.tint, in: .capsule)
        case .unavailable:
            EmptyView()
        }
    }

    /// Replies are synthesized server-side: draw a pleasant fixed wave rather than a measured one.
    static let placeholderWave: [Float] = (0..<28).map { i in
        let x = Double(i)
        let swell: Double = 0.6 + 0.4 * abs(cos(x * 0.31))
        let level: Double = 0.25 + 0.6 * abs(sin(x * 0.7)) * swell
        return Float(level)
    }
}
