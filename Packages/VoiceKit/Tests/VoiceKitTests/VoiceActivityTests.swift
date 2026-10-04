import Foundation
import Testing
@testable import VoiceKit

/// Builds 20 ms frames: `(duration, level, hasTranscript)` segments played back to back.
private func frames(_ segments: [(TimeInterval, Float, Bool)], step: TimeInterval = 0.02) -> [VoiceSample] {
    var samples: [VoiceSample] = []
    var t: TimeInterval = 0
    for (duration, level, transcript) in segments {
        let end = t + duration
        while t < end - 1e-9 {
            samples.append(VoiceSample(timestamp: t, rmsLevel: level, hasPartialTranscript: transcript))
            t += step
        }
    }
    return samples
}

@Suite("End of utterance")
struct EndOfUtteranceDetectorTests {
    private func run(_ samples: [VoiceSample], configuration: EndOfUtteranceDetector.Configuration = .init()) -> [(TimeInterval, EndOfUtteranceDetector.Event)] {
        var detector = EndOfUtteranceDetector(configuration: configuration)
        return samples.compactMap { sample in detector.process(sample).map { (sample.timestamp, $0) } }
    }

    @Test func detectsStartAndEndAfterDefaultSilence() throws {
        let events = run(frames([(0.3, 0.001, false), (1.0, 0.1, true), (1.0, 0.001, true)]))
        #expect(events.map(\.1) == [.speechStarted, .utteranceEnded])
        let start = try #require(events.first?.0)
        let end = try #require(events.last?.0)
        #expect(start >= 0.4 && start < 0.45) // 100 ms of sustained level after 0.3 s
        #expect(abs(end - (1.28 + 0.7)) < 0.03) // last loud frame at 1.28 s + 700 ms
    }

    @Test func shortPausesDoNotEndTheUtterance() {
        let events = run(frames([(0.5, 0.1, true), (0.5, 0.0, true), (0.5, 0.1, true), (1.0, 0.0, true)]))
        #expect(events.map(\.1) == [.speechStarted, .utteranceEnded])
    }

    @Test func silenceThresholdIsConfigurable() {
        let samples = frames([(0.5, 0.1, true), (0.5, 0.0, true)])
        #expect(run(samples).map(\.1) == [.speechStarted])
        #expect(run(samples, configuration: .init(silenceDuration: 0.4)).map(\.1) == [.speechStarted, .utteranceEnded])
    }

    @Test func noiseWithoutTranscriptIsDiscarded() {
        let events = run(frames([(0.5, 0.1, false), (1.0, 0.0, false), (0.5, 0.1, true), (1.0, 0.0, true)]))
        #expect(events.map(\.1) == [.speechStarted, .speechStarted, .utteranceEnded])
    }

    @Test func clicksShorterThanMinimumAreIgnored() {
        #expect(run(frames([(0.04, 0.5, false), (1.0, 0.0, false)])).isEmpty)
    }

    @Test func lingeringTranscriptDoesNotRestartSpeech() {
        // After an utterance ends, the partial transcript stays non-empty until the app resets the recogniser.
        let events = run(frames([(0.5, 0.1, true), (2.0, 0.0, true)]))
        #expect(events.map(\.1) == [.speechStarted, .utteranceEnded])
    }
}

@Suite("Barge-in")
struct BargeInDetectorTests {
    private func run(_ samples: [VoiceSample], playing: Bool = true) -> [TimeInterval] {
        var detector = BargeInDetector()
        if playing { detector.playbackStarted() }
        return samples.compactMap { sample in detector.process(sample).map { _ in sample.timestamp } }
    }

    @Test func firesOnceOnSustainedSpeechWithTranscript() throws {
        let hits = run(frames([(0.2, 0.01, false), (0.2, 0.2, false), (0.6, 0.2, true)]))
        #expect(hits.count == 1)
        let hit = try #require(hits.first)
        #expect(abs(hit - 0.44) < 0.03) // 250 ms after onset at 0.2 s
    }

    @Test func requiresTranscript() {
        #expect(run(frames([(1.0, 0.3, false)])).isEmpty) // loud echo / music, no words
    }

    @Test func requiresMinimumDuration() {
        #expect(run(frames([(0.1, 0.3, true), (0.5, 0.0, true)])).isEmpty)
    }

    @Test func toleratesShortDipsBetweenSyllables() {
        #expect(run(frames([(0.14, 0.2, true), (0.08, 0.0, true), (0.14, 0.2, true)])).count == 1)
        #expect(run(frames([(0.14, 0.2, true), (0.3, 0.0, true), (0.14, 0.2, true)])).isEmpty)
    }

    @Test func ignoredWithoutPlaybackAndRearmedOnNextPlayback() {
        let speech = frames([(0.5, 0.2, true)])
        #expect(run(speech, playing: false).isEmpty)

        var detector = BargeInDetector()
        detector.playbackStarted()
        #expect(speech.compactMap { detector.process($0) } == [.bargeIn])
        detector.playbackStopped()
        #expect(speech.compactMap { detector.process($0) }.isEmpty)
        detector.playbackStarted()
        #expect(speech.compactMap { detector.process($0) } == [.bargeIn])
    }
}
