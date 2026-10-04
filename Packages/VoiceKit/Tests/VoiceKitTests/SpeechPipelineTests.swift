import Foundation
import Synchronization
import Testing
@testable import VoiceKit

/// TTS whose audio is the sentence's UTF-8 text, with per-sentence latency; tracks concurrency and cancellation.
private final class SlowTTS: TTSProvider {
    private struct State {
        var requests: [String] = []
        var inFlight = 0
        var maxInFlight = 0
        var cancelled = 0
        var replies = 0
    }

    private let delays: [String: Duration]
    private let defaultDelay: Duration
    private let failing: Set<String>
    private let state = Mutex(State())

    init(delays: [String: Duration] = [:], defaultDelay: Duration = .milliseconds(5), failing: Set<String> = []) {
        self.delays = delays
        self.defaultDelay = defaultDelay
        self.failing = failing
    }

    var requests: [String] { state.withLock { $0.requests } }
    var maxInFlight: Int { state.withLock { $0.maxInFlight } }
    var cancelled: Int { state.withLock { $0.cancelled } }
    var replies: Int { state.withLock { $0.replies } }

    func beginReply() { state.withLock { $0.replies += 1 } }

    func synthesize(_ sentence: String) async throws -> PCMChunk {
        state.withLock {
            $0.requests.append(sentence)
            $0.inFlight += 1
            $0.maxInFlight = max($0.maxInFlight, $0.inFlight)
        }
        defer { state.withLock { $0.inFlight -= 1 } }
        do {
            try await Task.sleep(for: delays[sentence] ?? defaultDelay)
        } catch {
            state.withLock { $0.cancelled += 1 }
            throw error
        }
        if failing.contains(sentence) { throw TTSError.httpStatus(500) }
        return PCMChunk(samples: Data(sentence.utf8), sampleRate: 24_000)
    }
}

/// Streaming TTS: each word of a segment is one chunk (its UTF-8 text), after a per-segment delay.
private final class StreamingTTS: TTSProvider {
    private let delays: [String: Duration]
    private let failAfterFirstChunk: Set<String>
    let requests = Mutex<[String]>([])

    init(delays: [String: Duration] = [:], failAfterFirstChunk: Set<String> = []) {
        self.delays = delays
        self.failAfterFirstChunk = failAfterFirstChunk
    }

    var streamsAudio: Bool { true }

    func synthesize(_ sentence: String) async throws -> PCMChunk { fatalError("streaming only") }

    func stream(_ text: String) -> AsyncThrowingStream<PCMChunk, any Error> {
        requests.withLock { $0.append(text) }
        let words = text.split(separator: " ").map { String($0) + " " }
        let delay = delays[text] ?? .milliseconds(2)
        let failing = failAfterFirstChunk.contains(text)
        return AsyncThrowingStream.producing { yield in
            for (index, word) in words.enumerated() {
                try await Task.sleep(for: delay)
                if failing, index == 1 { throw TTSError.httpStatus(500) }
                yield(PCMChunk(samples: Data(word.utf8), sampleRate: 24_000))
            }
        }
    }
}

/// Every emitted chunk as (segment index, audio-as-text).
private func collectChunks(_ events: AsyncThrowingStream<SpeechPipeline.Event, any Error>) async throws -> [(Int, String)] {
    var chunks: [(Int, String)] = []
    for try await event in events {
        if case .sentence(let index, _, let audio) = event { chunks.append((index, String(decoding: audio.samples, as: UTF8.self))) }
    }
    return chunks
}

private func stream(_ deltas: [String]) -> AsyncThrowingStream<String, any Error> {
    AsyncThrowingStream { continuation in
        deltas.forEach { continuation.yield($0) }
        continuation.finish()
    }
}

/// Spoken sentences as (text, audio-as-text) pairs.
private func collect(_ events: AsyncThrowingStream<SpeechPipeline.Event, any Error>) async throws -> [String] {
    var spoken: [String] = []
    for try await event in events {
        if case .sentence(let index, let text, let audio) = event {
            #expect(index == spoken.count)
            #expect(String(decoding: audio.samples, as: UTF8.self) == text)
            spoken.append(text)
        }
    }
    return spoken
}

/// Polls `condition` for up to ~2 s.
private func eventually(_ condition: () -> Bool) async -> Bool {
    for _ in 0..<200 {
        if condition() { return true }
        try? await Task.sleep(for: .milliseconds(10))
    }
    return condition()
}

@Suite("Speech pipeline")
struct SpeechPipelineTests {
    @Test func splitsDeltasAndSpeaksInOrderEvenWhenSynthesisFinishesOutOfOrder() async throws {
        let tts = SlowTTS(delays: ["Un.": .milliseconds(120), "Deux.": .milliseconds(5), "Trois ?": .milliseconds(5)])
        let pipeline = SpeechPipeline(tts: tts, maxInFlight: 3, grouping: false)
        let spoken = try await collect(pipeline.events(for: stream(["U", "n. De", "ux. **Tro", "is** ? Fin"])))
        #expect(spoken == ["Un.", "Deux.", "Trois ?", "Fin"])
        // Requests are issued in order; concurrent ones may reach the provider in any order.
        #expect(Set(tts.requests) == ["Un.", "Deux.", "Trois ?", "Fin"] && tts.requests.count == 4)
        #expect(tts.replies == 1)
    }

    @Test func emitsTextStartedBeforeAudio() async throws {
        let pipeline = SpeechPipeline(tts: SlowTTS())
        var kinds: [String] = []
        for try await event in pipeline.events(for: stream(["Bonjour. ", "Salut."])) {
            switch event {
            case .textStarted: kinds.append("text")
            case .sentence: kinds.append("audio")
            }
        }
        #expect(kinds == ["text", "audio", "audio"])
    }

    @Test func speaksTheFirstSentenceBeforeTheReplyEnds() async throws {
        let (deltas, input) = AsyncThrowingStream<String, any Error>.makeStream()
        let events = SpeechPipeline(tts: SlowTTS()).events(for: deltas)
        var iterator = events.makeAsyncIterator()
        input.yield("Bonjour Sam. Je réfléchis")
        _ = try await iterator.next() // textStarted
        guard case .sentence(_, let first, _)? = try await iterator.next() else { Issue.record("no sentence"); return }
        #expect(first == "Bonjour Sam.")
        input.yield(" encore.")
        input.finish()
        guard case .sentence(_, let second, _)? = try await iterator.next() else { Issue.record("no sentence"); return }
        #expect(second == "Je réfléchis encore.")
        #expect(try await iterator.next() == nil)
    }

    @Test func boundsConcurrentSynthesis() async throws {
        let tts = SlowTTS(defaultDelay: .milliseconds(15))
        let text = (1...8).map { "Phrase \($0)." }.joined(separator: " ")
        let spoken = try await collect(SpeechPipeline(tts: tts, maxInFlight: 2, grouping: false).events(for: stream([text])))
        #expect(spoken.count == 8)
        #expect(tts.maxInFlight == 2) // the awaited sentence + one prefetch
    }

    @Test func cancellingStopsTextStreamAndSynthesis() async throws {
        let tts = SlowTTS(delays: ["Un.": .milliseconds(1)], defaultDelay: .seconds(10))
        let terminated = Mutex(false)
        let (deltas, input) = AsyncThrowingStream<String, any Error>.makeStream()
        input.onTermination = { _ in terminated.withLock { $0 = true } }
        input.yield("Un. Deux. Trois. Quatre. ")

        let firstSpoken = Mutex(false)
        let consumer = Task {
            for try await event in SpeechPipeline(tts: tts, maxInFlight: 2, grouping: false).events(for: deltas) {
                if case .sentence = event { firstSpoken.withLock { $0 = true } }
            }
        }
        #expect(await eventually { firstSpoken.withLock { $0 } && tts.requests.count == 3 })
        consumer.cancel()
        #expect(await eventually { terminated.withLock { $0 } && tts.cancelled == 2 })
        #expect(Set(tts.requests) == ["Un.", "Deux.", "Trois."]) // "Quatre." never requested
        input.yield("Cinq. ")
        try? await Task.sleep(for: .milliseconds(50))
        #expect(tts.requests.count == 3)
    }

    @Test func failedSegmentIsSkipped() async throws {
        let tts = SlowTTS(failing: ["Deux."])
        let spoken = try await collect(SpeechPipeline(tts: tts, grouping: false).events(for: stream(["Un. Deux. Trois."])))
        #expect(spoken == ["Un.", "Trois."])
    }

    @Test func groupsSentencesThatPiledUpDuringSynthesis() async throws {
        let tts = SlowTTS(delays: ["Un.": .milliseconds(150)])
        let (deltas, input) = AsyncThrowingStream<String, any Error>.makeStream()
        let consumer = Task { try await collect(SpeechPipeline(tts: tts).events(for: deltas)) }
        input.yield("Un. ")
        try await Task.sleep(for: .milliseconds(30)) // "Un." is being synthesized…
        input.yield("Deux. Trois. Quatre.")         // …while these arrive
        input.finish()
        let spoken = try await consumer.value
        #expect(spoken == ["Un.", "Deux. Trois. Quatre."])
        #expect(tts.requests == ["Un.", "Deux. Trois. Quatre."])
    }

    @Test func firstSegmentIsTheFirstClauseOfALongSentence() async throws {
        let tts = SlowTTS()
        let sentence = "Voici trois conseils adaptés à ta situation, en partant de ce que tu m’as dit hier soir."
        let spoken = try await collect(SpeechPipeline(tts: tts).events(for: stream([sentence])))
        #expect(spoken == ["Voici trois conseils adaptés à ta situation,", "en partant de ce que tu m’as dit hier soir."])
    }

    @Test func splitHeadFallsBackToAWordBoundary() {
        let (head, tail) = SpeechPipeline.splitHead("Un deux trois quatre cinq six sept huit neuf dix onze douze treize", maxLength: 30)
        #expect(head == "Un deux trois quatre cinq six")
        #expect(tail == "sept huit neuf dix onze douze treize")
        #expect(SpeechPipeline.splitHead("Court.", maxLength: 30).tail == nil)
    }

    @Test func textStreamErrorEndsTheReply() async {
        let deltas = AsyncThrowingStream<String, any Error> { continuation in
            continuation.yield("Un. ")
            continuation.finish(throwing: URLError(.networkConnectionLost))
        }
        await #expect(throws: URLError.self) {
            _ = try await collect(SpeechPipeline(tts: SlowTTS()).events(for: deltas))
        }
    }

    @Test func emptyReplyFinishesWithoutAudio() async throws {
        let tts = SlowTTS()
        #expect(try await collect(SpeechPipeline(tts: tts).events(for: stream(["```", "\ncode\n", "```\n"]))).isEmpty)
        #expect(tts.requests.isEmpty)
    }

    @Test func streamedChunksPlayInSegmentOrder() async throws {
        // Segment 0 streams slowly, segment 1 fast: its chunks must wait until segment 0 is complete.
        let tts = StreamingTTS(delays: ["Un deux trois.": .milliseconds(30), "Quatre cinq.": .milliseconds(1)])
        let pipeline = SpeechPipeline(tts: tts, maxInFlight: 2, grouping: false)
        let chunks = try await collectChunks(pipeline.events(for: stream(["Un deux trois. Quatre cinq."])))
        #expect(chunks.map(\.0) == [0, 0, 0, 1, 1])
        #expect(chunks.map(\.1).joined() == "Un deux trois. Quatre cinq. ")
    }

    @Test func streamingProviderGetsTheWholeFirstSentence() async throws {
        let tts = StreamingTTS()
        let sentence = "Voici trois conseils adaptés à ta situation, en partant de ce que tu m’as dit hier soir."
        _ = try await collectChunks(SpeechPipeline(tts: tts).events(for: stream([sentence])))
        #expect(tts.requests.withLock { $0 } == [sentence]) // no head split: no extra request warm-up
    }

    @Test func segmentFailingMidStreamKeepsWhatPlayedAndMovesOn() async throws {
        let tts = StreamingTTS(failAfterFirstChunk: ["Un deux trois."])
        let chunks = try await collectChunks(SpeechPipeline(tts: tts, grouping: false).events(for: stream(["Un deux trois. Quatre cinq."])))
        #expect(chunks.map(\.1) == ["Un ", "Quatre ", "cinq. "])
        #expect(chunks.map(\.0) == [0, 1, 1])
    }
}
