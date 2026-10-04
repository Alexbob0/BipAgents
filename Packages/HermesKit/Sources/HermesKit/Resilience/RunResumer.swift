import Foundation

/// Re-attaches to a run after a disconnect (app backgrounded, VPN flap): polls `GET /v1/runs/{id}`,
/// re-subscribes to `GET /v1/runs/{id}/events`, and retries transport failures with backoff
/// until the run reaches a terminal state.
///
/// Note: whether a re-subscription replays already-delivered events is unverified; after a resume,
/// the caller should resync the transcript with `messages(sessionID:)` once the run is terminal.
public struct RunResumer: Sendable {
    public enum Update: Sendable, Hashable {
        /// Current run state (emitted on every poll).
        case status(HermesRun)
        case event(HermesEvent)
        /// The server no longer knows the run (statuses are retained only briefly after the end):
        /// resync the transcript with `messages(sessionID:)`.
        case expired
    }

    public var client: HermesClient
    public var backoff: Backoff
    /// Consecutive retryable failures tolerated before the stream fails.
    public var maxConsecutiveFailures: Int

    public init(client: HermesClient, backoff: Backoff = Backoff(), maxConsecutiveFailures: Int = 10) {
        self.client = client
        self.backoff = backoff
        self.maxConsecutiveFailures = maxConsecutiveFailures
    }

    public func resume(runID: String) -> AsyncThrowingStream<Update, any Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    try await run(runID: runID, yield: { continuation.yield($0) })
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func run(runID: String, yield: @Sendable (Update) -> Void) async throws {
        var backoff = backoff
        var failures = 0
        var eventsAvailable = true

        while true {
            try Task.checkCancellation()
            do {
                let run: HermesRun
                do {
                    run = try await client.getRun(id: runID)
                } catch HermesError.http(404, _, _) {
                    yield(.expired)
                    return
                }
                yield(.status(run))
                if run.status.isTerminal { return }

                if eventsAvailable {
                    do {
                        for try await event in client.runEvents(runID: runID) {
                            failures = 0
                            backoff.reset()
                            yield(.event(event))
                            if event.isTerminal { return }
                        }
                    } catch HermesError.http(404, _, _) {
                        // Event buffer expired (unconsumed for 5 min): fall back to status polling.
                        eventsAvailable = false
                    }
                }
            } catch let error as HermesError where error.isRetryable {
                failures += 1
                if failures > maxConsecutiveFailures { throw error }
            }
            try await Task.sleep(for: backoff.next())
        }
    }
}
