import Foundation

/// One reply's text → ordered audio: streamed deltas → `SentenceSplitter` → segments → `TTSProvider`.
///
/// Tuned for a serial, non-streaming synthesizer (Kyutai behind the bridge: one request at a time, much
/// faster per second of audio when it gets several sentences at once, since it batches them on the GPU):
/// - the first segment is the reply's first clause only (≤ `firstSegmentLength`), so audio starts fast;
/// - while a segment is being synthesized, the following sentences accumulate and leave together as the
///   next segment (≤ `maxGroupLength`), which keeps the synthesizer ahead of playback;
/// - segments are emitted strictly in order the moment each one is ready, never waiting for the end of
///   the reply. A segment that fails is skipped rather than ending the whole reply.
/// Cancelling the consumer of `events(for:)` cancels the text stream and every synthesis request.
struct SpeechPipeline: Sendable {
    enum Event: Sendable {
        /// First text delta received (start of the "first audio" latency measurement).
        case textStarted(ContinuousClock.Instant)
        /// `index` counts emitted segments; `text` is what `audio` says.
        case sentence(index: Int, text: String, audio: PCMChunk)
    }

    var tts: any TTSProvider
    /// Segments requested at once. 1 suits a serial synthesizer: pending sentences then group up.
    var maxInFlight = 1
    var maxSentenceLength = 180
    /// Group pending sentences into one request (off = one request per sentence).
    var grouping = true
    var maxGroupLength = 700
    var firstSegmentLength = 60

    func events(for deltas: AsyncThrowingStream<String, any Error>) -> AsyncThrowingStream<Event, any Error> {
        let (stream, output) = AsyncThrowingStream<Event, any Error>.makeStream()
        let task = Task { await run(deltas, output) }
        output.onTermination = { _ in task.cancel() }
        return stream
    }

    /// Everything the coordinator loop reacts to, funnelled through one stream so a single loop can
    /// both receive sentences and emit finished audio without either blocking the other.
    private enum Step: Sendable {
        case textStarted(ContinuousClock.Instant)
        case sentences([String])
        case textEnded
        case textFailed(any Error)
        case synthesized(Int, Result<PCMChunk, any Error>)
    }

    private enum Outcome { case audio(PCMChunk), skipped }

    private func run(_ deltas: AsyncThrowingStream<String, any Error>,
                     _ output: AsyncThrowingStream<Event, any Error>.Continuation) async {
        let (steps, step) = AsyncStream<Step>.makeStream()
        let tts = tts
        tts.beginReply()
        let reader = Task { [maxSentenceLength] in
            var splitter = SentenceSplitter(maxLength: maxSentenceLength)
            var started = false
            do {
                for try await delta in deltas {
                    if !started {
                        started = true
                        step.yield(.textStarted(.now))
                    }
                    let sentences = splitter.feed(delta)
                    if !sentences.isEmpty { step.yield(.sentences(sentences)) }
                }
                step.yield(.sentences(splitter.flush()))
                step.yield(.textEnded)
            } catch {
                step.yield(.textFailed(error))
            }
        }

        var pending: [String] = []          // complete sentences not requested yet
        var segments: [String] = []         // requested segment texts, by index
        var ready: [Int: Outcome] = [:]
        var synthesis: [Int: Task<Void, Never>] = [:]
        var nextToEmit = 0, emitted = 0
        var textEnded = false
        defer {
            reader.cancel()
            synthesis.values.forEach { $0.cancel() }
            step.finish()
        }

        for await event in steps {
            if Task.isCancelled { break }
            switch event {
            case .textStarted(let instant):
                output.yield(.textStarted(instant))
            case .sentences(let new):
                pending += new
            case .textEnded:
                textEnded = true
            case .textFailed(let error):
                return output.finish(throwing: error)
            case .synthesized(let index, let result):
                synthesis[index] = nil
                switch result {
                case .success(let audio): ready[index] = .audio(audio)
                case .failure(let error) where FallbackTTSProvider.isCancellation(error): return output.finish(throwing: error)
                case .failure: ready[index] = .skipped
                }
            }

            while let outcome = ready.removeValue(forKey: nextToEmit) {
                if case .audio(let audio) = outcome {
                    output.yield(.sentence(index: emitted, text: segments[nextToEmit], audio: audio))
                    emitted += 1
                }
                nextToEmit += 1
            }
            while !pending.isEmpty, synthesis.count < maxInFlight {
                let index = segments.count, text = nextSegment(from: &pending, isFirst: segments.isEmpty)
                segments.append(text)
                synthesis[index] = Task {
                    let result: Result<PCMChunk, any Error>
                    do { result = .success(try await tts.synthesize(text)) } catch { result = .failure(error) }
                    step.yield(.synthesized(index, result))
                }
            }
            if textEnded, pending.isEmpty, synthesis.isEmpty, nextToEmit == segments.count { return output.finish() }
        }
        output.finish(throwing: CancellationError())
    }

    /// Takes the next segment's text off `pending`.
    private func nextSegment(from pending: inout [String], isFirst: Bool) -> String {
        if isFirst {
            let (head, tail) = Self.splitHead(pending[0], maxLength: firstSegmentLength)
            if let tail { pending[0] = tail } else { pending.removeFirst() }
            return head
        }
        guard grouping else { return pending.removeFirst() }
        var text = pending.removeFirst()
        while let next = pending.first, text.count + 1 + next.count <= maxGroupLength {
            text += " " + next
            pending.removeFirst()
        }
        return text
    }

    /// Cuts a long first sentence at its first natural pause (clause, then word) within `maxLength`.
    static func splitHead(_ sentence: String, maxLength: Int) -> (head: String, tail: String?) {
        guard sentence.count > maxLength else { return (sentence, nil) }
        let window = sentence.prefix(maxLength)
        let minimum = sentence.index(sentence.startIndex, offsetBy: min(15, maxLength))
        var cut: String.Index?
        for separator in [", ", "; ", " : ", " — ", " – "] {
            if let range = window.range(of: separator, options: .backwards), range.lowerBound >= minimum {
                cut = range.upperBound
                break
            }
        }
        if cut == nil, let space = window.lastIndex(of: " "), space >= minimum { cut = sentence.index(after: space) }
        guard let cut else { return (sentence, nil) }
        let head = sentence[..<cut].trimmingCharacters(in: .whitespaces)
        let tail = sentence[cut...].trimmingCharacters(in: .whitespaces)
        return tail.isEmpty ? (sentence, nil) : (head, tail)
    }
}
