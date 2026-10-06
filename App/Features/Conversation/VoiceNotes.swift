import AVFoundation
import HermesKit
import MediaPlayer
import AVKit
import Observation
import SwiftUI
import VoiceKit

/// What the lock screen and the Dynamic Island show for the audio playing.
struct NowPlayingInfo {
    var title: String
    var artist: String?
    var artwork: UIImage?

    /// An agent's audio: its name, and its Bip as the artwork (the PNG exported for notifications).
    static func agent(_ agent: AgentProfile, title: String) -> NowPlayingInfo {
        let artwork = AgentAvatars.folder.flatMap { UIImage(contentsOfFile: $0.appending(path: "\(agent.id.uuidString).png").path(percentEncoded: false)) }
        return NowPlayingInfo(title: title, artist: agent.name, artwork: artwork)
    }
}

/// Plays voice notes, agents' spoken replies and their audio files (a podcast), one at a time: pause keeps the
/// position, ±15 s and seeking, and — for the shared player — the lock screen / Dynamic Island controls.
@Observable
final class VoiceNotePlayer {
    /// The app's player: keeps playing across screens and with the phone locked.
    static let shared = VoiceNotePlayer(publishesNowPlaying: true)

    /// The file loaded (playing or paused).
    private(set) var loadedURL: URL?
    private(set) var isPlaying = false
    private(set) var currentTime: TimeInterval = 0
    private(set) var duration: TimeInterval = 0

    /// The file playing right now (not paused).
    var playingURL: URL? { isPlaying ? loadedURL : nil }
    var progress: Double { duration > 0 ? currentTime / duration : 0 }

    @ObservationIgnored private var player: AVAudioPlayer?
    @ObservationIgnored private var ticker: Task<Void, Never>?
    @ObservationIgnored private var info: NowPlayingInfo?
    @ObservationIgnored private let publishesNowPlaying: Bool
    @ObservationIgnored private var holdsSession = false

    init(publishesNowPlaying: Bool = false) {
        self.publishesNowPlaying = publishesNowPlaying
        if publishesNowPlaying { configureRemoteCommands() }
    }

    /// Play `url` (or pause / resume it if it is the one loaded).
    func toggle(_ url: URL, info: NowPlayingInfo? = nil) {
        if loadedURL == url {
            isPlaying ? pause() : resume()
            return
        }
        guard load(url, info: info) else { return }
        resume()
    }

    func pause() {
        player?.pause()
        isPlaying = false
        ticker?.cancel()
        sync()
        updateNowPlaying()
    }

    func resume() {
        guard let player else { return }
        try? AVAudioSession.sharedInstance().setCategory(.playback, mode: .spokenAudio)
        try? AVAudioSession.sharedInstance().setActive(true)
        if !holdsSession { AudioSessionUsage.begin(); holdsSession = true }
        player.play()
        isPlaying = true
        startTicker()
        updateNowPlaying()
    }

    /// Jump to `time` in `url`, loading it (paused) if needed.
    func seek(_ url: URL, to time: TimeInterval, info: NowPlayingInfo? = nil) {
        if loadedURL != url { guard load(url, info: info) else { return } }
        guard let player else { return }
        player.currentTime = min(max(0, time), player.duration)
        sync()
        updateNowPlaying()
    }

    func skip(by seconds: TimeInterval) {
        guard let url = loadedURL else { return }
        seek(url, to: currentTime + seconds)
    }

    func stop() {
        ticker?.cancel()
        ticker = nil
        player?.stop()
        if holdsSession { AudioSessionUsage.end(); holdsSession = false }
        player = nil
        loadedURL = nil
        isPlaying = false
        currentTime = 0
        duration = 0
        info = nil
        if publishesNowPlaying { MPNowPlayingInfoCenter.default().nowPlayingInfo = nil }
    }

    private func load(_ url: URL, info: NowPlayingInfo?) -> Bool {
        stop()
        guard let player = try? AVAudioPlayer(contentsOf: url) else { return false }
        player.prepareToPlay()
        self.player = player
        self.info = info
        loadedURL = url
        duration = player.duration
        currentTime = 0
        return true
    }

    private func sync() {
        currentTime = player?.currentTime ?? 0
    }

    private func startTicker() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while let self, let player = self.player, !Task.isCancelled {
                self.currentTime = player.currentTime
                if !player.isPlaying && self.isPlaying {  // reached the end
                    self.stop()
                    return
                }
                try? await Task.sleep(for: .milliseconds(200))
            }
        }
    }

    // MARK: Lock screen, Dynamic Island, AirPlay receivers

    private func updateNowPlaying() {
        guard publishesNowPlaying, let player else { return }
        var now: [String: Any] = [
            MPMediaItemPropertyTitle: info?.title ?? loadedURL?.deletingPathExtension().lastPathComponent ?? "BipAgents",
            MPMediaItemPropertyPlaybackDuration: player.duration,
            MPNowPlayingInfoPropertyElapsedPlaybackTime: player.currentTime,
            MPNowPlayingInfoPropertyPlaybackRate: isPlaying ? 1.0 : 0.0,
            MPNowPlayingInfoPropertyMediaType: MPNowPlayingInfoMediaType.audio.rawValue,
        ]
        if let artist = info?.artist { now[MPMediaItemPropertyArtist] = artist }
        if let image = info?.artwork { now[MPMediaItemPropertyArtwork] = Self.artwork(image) }
        MPNowPlayingInfoCenter.default().nowPlayingInfo = now
    }

    /// MediaPlayer asks for the image on its own queue: the handler must not be main-actor isolated (it was, by the
    /// module's default isolation, and Swift's runtime check stopped the app the moment the lock screen asked).
    nonisolated private static func artwork(_ image: UIImage) -> MPMediaItemArtwork {
        MPMediaItemArtwork(boundsSize: image.size) { @Sendable _ in image }
    }

    private func configureRemoteCommands() {
        let center = MPRemoteCommandCenter.shared()
        Self.handle(center.playCommand) { [weak self] _ in self?.resume() }
        Self.handle(center.pauseCommand) { [weak self] _ in self?.pause() }
        Self.handle(center.togglePlayPauseCommand) { [weak self] _ in
            if let self { self.isPlaying ? self.pause() : self.resume() }
        }
        center.skipBackwardCommand.preferredIntervals = [15]
        Self.handle(center.skipBackwardCommand) { [weak self] _ in self?.skip(by: -15) }
        center.skipForwardCommand.preferredIntervals = [15]
        Self.handle(center.skipForwardCommand) { [weak self] _ in self?.skip(by: 15) }
        Self.handle(center.changePlaybackPositionCommand) { [weak self] time in
            if let self, let url = self.loadedURL, let time { self.seek(url, to: time) }
        }
    }

    /// Lock screen / Dynamic Island buttons: MediaPlayer may call them off the main thread, so the handler is built
    /// outside the main actor and hops to it (`positionTime` for a position change).
    nonisolated private static func handle(_ command: MPRemoteCommand,
                                           _ action: @escaping @MainActor @Sendable (TimeInterval?) -> Void) {
        command.addTarget { event in
            let time = (event as? MPChangePlaybackPositionCommandEvent)?.positionTime
            Task { @MainActor in action(time) }
            return .success
        }
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

/// Play button + waveform + duration, as in messaging apps. A long audio (a podcast, over a minute) gets a real
/// player: −15 s / +15 s, a scrubbable progress bar, elapsed / remaining time and the AirPlay button.
struct VoiceNoteControl: View {
    var url: URL
    var duration: TimeInterval
    var waveform: [Float]
    var player: VoiceNotePlayer
    var foreground: Color
    var accent: Color
    var info: NowPlayingInfo? = nil

    private var isLoaded: Bool { player.loadedURL == url }
    private var isPlaying: Bool { player.playingURL == url }
    private var isLong: Bool { duration >= 60 }

    var body: some View {
        if isLong { longPlayer } else { compact }
    }

    private var playIcon: some View {
        Image(systemName: isPlaying ? "pause.fill" : "play.fill")
            .font(.system(size: isLong ? 17 : 13, weight: .black))
            .foregroundStyle(accent == foreground ? Theme.ink : .white)
            .frame(width: isLong ? 44 : 32, height: isLong ? 44 : 32)
            .background(accent, in: .circle)
    }

    private var compact: some View {
        Button { player.toggle(url, info: info) } label: {
            HStack(spacing: 10) {
                playIcon
                HStack(spacing: 1.5) {
                    ForEach(Array(waveform.enumerated()), id: \.offset) { index, level in
                        let played = isLoaded && Double(index) / Double(max(waveform.count, 1)) < player.progress
                        Capsule()
                            .fill(foreground.opacity(played || !isLoaded ? 1 : 0.45))
                            .frame(width: 2.5, height: 4 + 22 * CGFloat(level))
                    }
                }
                .frame(height: 28)
                Text(Duration.seconds(isLoaded ? player.currentTime : duration), format: .time(pattern: .minuteSecond))
                    .font(Theme.body(13, weight: .heavy))
                    .monospacedDigit()
                    .foregroundStyle(foreground)
                    .lineLimit(1)
                    .fixedSize()
            }
        }
        .buttonStyle(.plain)
        .accessibilityLabel(isPlaying ? String(localized: "Pause") : String(localized: "Écouter le message vocal"))
    }

    private var longPlayer: some View {
        let elapsed = isLoaded ? player.currentTime : 0
        return VStack(spacing: 6) {
            HStack(spacing: 14) {
                skipButton(-15)
                Button { player.toggle(url, info: info) } label: { playIcon }
                    .buttonStyle(.plain)
                    .accessibilityLabel(isPlaying ? String(localized: "Pause") : String(localized: "Écouter"))
                skipButton(15)
                Spacer(minLength: 8)
                AirPlayButton(tint: UIColor(foreground))
                    .frame(width: 34, height: 34)
            }
            Slider(value: Binding(get: { elapsed }, set: { player.seek(url, to: $0, info: info) }), in: 0...max(duration, 1))
                .tint(foreground)
            HStack {
                Text(Duration.seconds(elapsed), format: .time(pattern: .minuteSecond))
                Spacer()
                Text("-") + Text(Duration.seconds(max(0, duration - elapsed)), format: .time(pattern: .minuteSecond))
            }
            .font(Theme.body(12, weight: .heavy))
            .monospacedDigit()
            .foregroundStyle(foreground)
        }
        .frame(minWidth: 240)
    }

    private func skipButton(_ seconds: TimeInterval) -> some View {
        Button {
            if !isLoaded { player.seek(url, to: max(0, seconds), info: info) } else { player.skip(by: seconds) }
        } label: {
            Image(systemName: seconds < 0 ? "gobackward.15" : "goforward.15")
                .font(.system(size: 20, weight: .bold))
                .foregroundStyle(foreground)
                .frame(width: 36, height: 36)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(seconds < 0 ? String(localized: "Reculer de 15 secondes") : String(localized: "Avancer de 15 secondes"))
    }
}

/// The system AirPlay picker (HomePod, Apple TV, AirPlay speakers and TVs).
struct AirPlayButton: UIViewRepresentable {
    var tint: UIColor

    func makeUIView(context: Context) -> AVRoutePickerView {
        let view = AVRoutePickerView()
        view.tintColor = tint
        view.activeTintColor = tint
        view.prioritizesVideoDevices = false
        return view
    }

    func updateUIView(_ view: AVRoutePickerView, context: Context) {
        view.tintColor = tint
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
    var info: NowPlayingInfo? = nil
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
            HStack(spacing: 8) {
                VoiceNoteControl(url: url, duration: duration, waveform: Self.placeholderWave, player: player,
                                 foreground: palette.deep, accent: palette.deep, info: info)
                    .padding(.horizontal, duration >= 60 ? 14 : 10)
                    .padding(.vertical, duration >= 60 ? 10 : 6)
                    .background(palette.tint, in: .rect(cornerRadius: 22, style: .continuous))
                ShareFileButton(url: ShareFile.named(url, String(localized: "Réponse vocale") + " " +
                                                     Date.now.formatted(.dateTime.day().month().hour().minute())), palette: palette)
            }
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
