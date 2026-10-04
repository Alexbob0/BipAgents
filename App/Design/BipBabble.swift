import AVFoundation
import VoiceKit

/// The Bips' "Animalese": every letter of a bubble phrase becomes a tiny synthesized syllable, coloured by its
/// vowel, pitched per agent and per mood. No model, no network: a few harmonics shaped like vowel formants.
///
/// Plays in the `.ambient` category (mixes with music, silenced by the ring/silent switch, like game sounds)
/// and only when nothing else in the app holds the audio session, so it never cuts a Live or a voice note.
@MainActor
final class BipBabble {
    static let shared = BipBabble()
    /// `UserDefaults` key of the « Sons des Bips » setting (default on).
    static let enabledKey = "bipSounds"

    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private let format = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1)!
    private var configured = false
    private var phrase = 0

    static var isEnabled: Bool { UserDefaults.standard.object(forKey: enabledKey) as? Bool ?? true }

    func say(_ text: String, appearance: AgentAppearance, mood: MascotMood?) {
        guard Self.isEnabled, AudioSessionUsage.isIdle else { return }
        let voice = BabbleVoice(appearance: appearance, mood: mood)
        let samples = BabbleSynth.render(text, voice: voice, sampleRate: format.sampleRate)
        guard !samples.isEmpty, let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(samples.count)) else { return }
        buffer.frameLength = AVAudioFrameCount(samples.count)
        samples.withUnsafeBufferPointer { buffer.floatChannelData![0].update(from: $0.baseAddress!, count: samples.count) }
        do {
            try AVAudioSession.sharedInstance().setCategory(.ambient)
            try AVAudioSession.sharedInstance().setActive(true)
            if !configured {
                engine.attach(player)
                engine.connect(player, to: engine.mainMixerNode, format: format)
                configured = true
            }
            if !engine.isRunning { try engine.start() }
            player.stop() // a new phrase replaces the previous one
            phrase += 1
            let current = phrase
            player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { _ in
                Task { @MainActor [weak self] in
                    // Release the audio hardware once the Bip is quiet.
                    guard let self, self.phrase == current else { return }
                    self.engine.stop()
                }
            }
            player.play()
        } catch {
            #if DEBUG
            print("[bip] babble unavailable: \(error)")
            #endif
        }
    }
}

/// How one Bip sounds: base pitch from its colour, tempo and melody from its mood.
struct BabbleVoice {
    var basePitch: Double      // Hz
    var syllable: Double       // seconds per letter
    var spread: Double         // semitones of letter-to-letter variation
    var drift: Double          // semitones added from first to last letter
    var bounce: Double         // semitones alternating up/down (giggles)
    var volume: Double

    init(appearance: AgentAppearance, mood: MascotMood?) {
        // Each category gets its own register; custom colours pick one from their hex.
        let registers: [AgentCategory: Double] = [
            .wellness: 520, .finance: 410, .daily: 600, .work: 450,
            .learning: 560, .home: 490, .creative: 640, .tech: 470,
        ]
        basePitch = appearance.customColorHex.map { 420 + Double($0 % 7) * 35 } ?? registers[appearance.category] ?? 520
        syllable = 0.055
        spread = 2.5
        drift = 0
        bounce = 0
        volume = 0.32
        switch mood {
        case .giggling: basePitch *= 1.25; syllable = 0.045; bounce = 3
        case .surprised: basePitch *= 1.15; drift = 5
        case .content: basePitch *= 0.92; syllable = 0.075; spread = 1.2; volume = 0.25
        case .dizzy: syllable = 0.07; drift = -7; spread = 3.5
        case .grumpy: basePitch *= 0.72; syllable = 0.065; spread = 1; drift = -2
        case .winking: drift = 2
        default: break
        }
    }
}

enum BabbleSynth {
    /// Formants (F1, F2) of the vowel each letter is sung on.
    private static let vowels: [Character: (Double, Double)] = [
        "a": (800, 1250), "e": (480, 1850), "i": (300, 2300), "o": (520, 900), "u": (330, 850), "y": (300, 2100),
    ]
    private static let plosives: Set<Character> = ["b", "d", "g", "k", "p", "q", "t"]
    private static let fricatives: Set<Character> = ["c", "f", "h", "j", "s", "v", "x", "z"]

    static func render(_ text: String, voice: BabbleVoice, sampleRate: Double) -> [Float] {
        let letters = Array(text.lowercased().folding(options: .diacriticInsensitive, locale: .init(identifier: "fr")))
        let sayable = letters.filter(\.isLetter)
        guard !sayable.isEmpty else { return [] }
        let ending = letters.last { !$0.isWhitespace && $0 != "♡" }
        var finalDrift = voice.drift
        if ending == "?" { finalDrift += 4 }
        if ending == "!" { finalDrift += 2 }
        if ending == "…" { finalDrift -= 3 }

        var out: [Float] = []
        var spoken = 0
        var lastVowel: (Double, Double) = (800, 1250)
        var noise = SystemRandomNumberGenerator()
        for letter in letters {
            if letter.isWhitespace || letter == "," {
                out += [Float](repeating: 0, count: Int(sampleRate * voice.syllable * 0.8))
                continue
            }
            guard letter.isLetter else { continue }
            let progress = sayable.count > 1 ? Double(spoken) / Double(sayable.count - 1) : 0
            // Letter → stable pitch offset (same word, same tune), plus mood drift and bounce.
            let scalar = Double(letter.unicodeScalars.first!.value)
            var semitones = (scalar.truncatingRemainder(dividingBy: 5) - 2) / 2 * voice.spread
            semitones += finalDrift * progress
            semitones += spoken.isMultiple(of: 2) ? voice.bounce : -voice.bounce / 2
            let f0 = voice.basePitch * pow(2, semitones / 12)
            if let formants = vowels[letter] { lastVowel = formants }
            let formants = vowels[letter] ?? lastVowel
            out += syllable(f0: f0, formants: formants, duration: voice.syllable, sampleRate: sampleRate,
                            onset: plosives.contains(letter) ? .plosive : fricatives.contains(letter) ? .fricative : .none,
                            volume: voice.volume, rng: &noise)
            spoken += 1
        }
        return out
    }

    private enum Onset { case none, plosive, fricative }

    private static func syllable(f0: Double, formants: (Double, Double), duration: Double, sampleRate: Double,
                                 onset: Onset, volume: Double, rng: inout SystemRandomNumberGenerator) -> [Float] {
        let count = Int(sampleRate * duration)
        // Harmonic amplitudes shaped by two Gaussian formant bumps (a cheap vowel colour).
        var harmonics: [(Double, Double)] = []
        var k = 1.0
        while k * f0 < min(5_000, sampleRate / 2.2) {
            let f = k * f0
            let shape = exp(-pow((f - formants.0) / 260, 2)) + 0.7 * exp(-pow((f - formants.1) / 320, 2)) + 0.06
            harmonics.append((k, shape / pow(k, 0.6)))
            k += 1
        }
        let norm = harmonics.reduce(0) { $0 + $1.1 }
        var samples = [Float](repeating: 0, count: count)
        var phase = 0.0
        let attack = sampleRate * 0.004, release = sampleRate * 0.018
        let onsetLength = onset == .none ? 0 : Int(sampleRate * (onset == .plosive ? 0.006 : 0.014))
        for i in 0..<count {
            let t = Double(i)
            // A little upward glide inside each syllable makes it sound "spoken" rather than beeped.
            phase += 2 * .pi * f0 * (1 + 0.04 * t / Double(count)) / sampleRate
            var value = 0.0
            for (k, amp) in harmonics { value += amp * sin(k * phase) }
            value /= norm
            let envelope = min(1, t / attack) * min(1, (Double(count) - t) / release)
            var sample = value * envelope
            if i < onsetLength {
                let burst = Double.random(in: -1...1, using: &rng) * (onset == .plosive ? 0.5 : 0.22)
                sample = sample * Double(i) / Double(onsetLength) + burst * (1 - Double(i) / Double(onsetLength))
            }
            samples[i] = Float(sample * volume)
        }
        return samples
    }
}
