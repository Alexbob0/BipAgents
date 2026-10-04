/// App-wide count of features currently holding the audio session (voice engines, voice-note players), so
/// incidental sounds — the Bips' babble — never switch the session category under a live call or a reply.
@MainActor
public enum AudioSessionUsage {
    public private(set) static var active = 0

    public static var isIdle: Bool { active == 0 }

    public static func begin() { active += 1 }

    public static func end() { active = max(0, active - 1) }
}
