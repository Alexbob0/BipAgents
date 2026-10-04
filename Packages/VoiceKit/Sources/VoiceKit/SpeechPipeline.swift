import Foundation

/// One reply's text → ordered audio: streamed deltas → `SentenceSplitter` → `TTSProvider` → chunks in order.
///
/// Sentences are synthesized as soon as they are complete, with up to `maxInFlight` requests running
/// (the sentence being waited for plus the prefetch of the next ones), and are emitted strictly in order
/// the moment each one is ready — never waiting for the end of the reply. Cancelling the consumer of
/// `events(for:)` cancels the text stream and every synthesis request.
struct SpeechPipeline: Sendable {
    enum Event: Sendable {
        /// First text delta received (start of the "first audio" latency measurement).
        case textStarted(ContinuousClock.Instant)
        case sentence(index: Int, text: String, audio: PCMChunk)
    }

    var tts: any TTSProvider
    var maxInFlight = 2
    var maxSentenceLength = 180

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

        var sentences: [String] = []
        var ready: [Int: PCMChunk] = [:]
        var synthesis: [Int: Task<Void, Never>] = [:]
        var nextToStart = 0, nextToEmit = 0
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
                sentences += new
            case .textEnded:
                textEnded = true
            case .textFailed(let error):
                return output.finish(throwing: error)
            case .synthesized(let index, .success(let audio)):
                synthesis[index] = nil
                ready[index] = audio
            case .synthesized(_, .failure(let error)):
                return output.finish(throwing: error)
            }

            while let audio = ready.removeValue(forKey: nextToEmit) {
                output.yield(.sentence(index: nextToEmit, text: sentences[nextToEmit], audio: audio))
                nextToEmit += 1
            }
            while nextToStart < sentences.count, nextToStart - nextToEmit < maxInFlight {
                let index = nextToStart, text = sentences[index]
                synthesis[index] = Task {
                    let result: Result<PCMChunk, any Error>
                    do { result = .success(try await tts.synthesize(text)) } catch { result = .failure(error) }
                    step.yield(.synthesized(index, result))
                }
                nextToStart += 1
            }
            if textEnded, nextToEmit == sentences.count { return output.finish() }
        }
        output.finish(throwing: CancellationError())
    }
}
