import AVFoundation
import Testing
@testable import VoiceKit

@Suite("Audio conversion")
struct AudioConversionTests {
    private func chunk(frames: Int, rate: Double) -> PCMChunk {
        let samples = (0..<frames).map { Int16(sin(Double($0) * 0.05) * 12_000) }
        return PCMChunk(samples: samples.withUnsafeBufferPointer { Data(buffer: $0) }, sampleRate: rate)
    }

    @Test func chunkRoundTripsThroughPCMBuffer() throws {
        let original = chunk(frames: 480, rate: 24_000)
        let buffer = try #require(original.pcmBuffer())
        #expect(buffer.frameLength == 480)
        #expect(buffer.format.sampleRate == 24_000)
        #expect(buffer.int16Data() == original.samples)
    }

    @Test func convertsBridgeAudioToThePlayerFormat() throws {
        let converter = FormatConverter(outputFormat: AudioGraph.playerFormat)
        let source = try #require(chunk(frames: 2_400, rate: 24_000).pcmBuffer())
        let output = try #require(converter.convert(source, streaming: false))
        #expect(output.format == AudioGraph.playerFormat)
        #expect(output.frameLength == 2_400)
        #expect(AudioLevel.rms(output, frames: 0..<Int(output.frameLength)) > 0.1)
    }

    @Test func resamplesOtherRatesWithoutLosingTheTail() throws {
        let converter = FormatConverter(outputFormat: AudioGraph.playerFormat)
        for _ in 0..<2 { // the converter is reused across sentences
            let source = try #require(chunk(frames: 1_600, rate: 16_000).pcmBuffer())
            let output = try #require(converter.convert(source, streaming: false))
            #expect(abs(Int(output.frameLength) - 2_400) <= 32)
        }
    }

    @Test func emptyChunkHasNoBuffer() {
        #expect(PCMChunk(samples: Data(), sampleRate: 24_000).pcmBuffer() == nil)
        #expect(PCMChunk(samples: Data([1]), sampleRate: 24_000).pcmBuffer() == nil)
    }

    @Test func levelsMapToDisplayRange() {
        #expect(AudioLevel.display(0) == 0)
        #expect(AudioLevel.display(1) == 1)
        #expect(AudioLevel.display(0.001) < AudioLevel.display(0.1))
        var level = 0.0
        level = AudioLevel.smoothed(level, toward: 1)
        #expect(level > 0.5) // fast attack
        let peak = level
        level = AudioLevel.smoothed(level, toward: 0)
        #expect(level > peak * 0.8) // slow release
    }
}

@Suite("System TTS", .enabled(if: !AVSpeechSynthesisVoice.speechVoices().isEmpty), .timeLimit(.minutes(1)))
struct SystemTTSProviderTests {
    @Test func rendersFrenchSpeechToPCM() async throws {
        let chunk = try await SystemTTSProvider().synthesize("Bonjour, je suis là.")
        #expect(chunk.sampleRate > 0)
        #expect(chunk.duration > .milliseconds(300))
    }

    @Test func emptySentenceRendersNothing() async throws {
        #expect(try await SystemTTSProvider().synthesize("  ").frameCount == 0)
    }
}
