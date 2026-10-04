import Foundation

/// Where `PlaybackQueue` sends audio: an `AVAudioPlayerNode` in production, a recorder in tests.
@MainActor
protocol AudioOutput: AnyObject {
    associatedtype Buffer
    /// Plays `buffer` after everything already scheduled. `completion` runs on the main actor once the
    /// buffer has been played back — or dropped by `stop()` (the queue ignores those late calls).
    func schedule(_ buffer: Buffer, completion: @escaping @MainActor @Sendable () -> Void)
    func play()
    /// Stops at once and drops every scheduled buffer.
    func stop()
}

/// Ordered audio queue for one reply on top of an `AudioOutput`.
///
/// Keeps up to `window` buffers scheduled on the output (≥ 2 so the next buffer is always queued at the
/// hardware when the current one ends: no gap even if the main actor is briefly busy), the rest pending.
/// `stop()` is synchronous (barge-in); a generation counter makes completions from dropped buffers no-ops.
@MainActor
final class PlaybackQueue<Output: AudioOutput> {
    let output: Output
    let window: Int

    private(set) var pending: [Output.Buffer] = []
    /// Scheduled on the output and not yet played back, oldest first.
    private(set) var scheduled: [Output.Buffer] = []
    private var generation = 0
    /// Counts `stop()` calls only (unlike `generation`, which `resetOutput()` also bumps).
    private var stops = 0
    private var inputFinished = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    init(output: Output, window: Int = 3) {
        self.output = output
        self.window = max(2, window)
    }

    /// Nothing scheduled or pending.
    var isEmpty: Bool { pending.isEmpty && scheduled.isEmpty }

    func enqueue(_ buffer: Output.Buffer) {
        pending.append(buffer)
        pump()
    }

    /// No more buffers for this reply: returns once all of them have been played, or on `stop()`.
    /// Cancelling the waiting task stops playback.
    func finishAndWait() async {
        inputFinished = true
        guard !isEmpty else { return finishDrain() }
        let stopsSoFar = stops
        await withTaskCancellationHandler {
            await withCheckedContinuation { waiters.append($0) }
        } onCancel: {
            // Only if nothing else stopped it meanwhile: the next reply may already be queued.
            Task { @MainActor in if self.stops == stopsSoFar { self.stop() } }
        }
    }

    /// Barge-in: silences the output and drops everything, synchronously.
    func stop() {
        generation += 1
        stops += 1
        output.stop()
        pending.removeAll()
        scheduled.removeAll()
        finishDrain()
    }

    /// The output was rebuilt (route change, interruption): reschedule everything not yet played,
    /// starting again from the beginning of the buffer that was playing.
    func resetOutput() {
        generation += 1
        pending = scheduled + pending
        scheduled.removeAll()
        pump()
    }

    private func pump() {
        while scheduled.count < window, !pending.isEmpty {
            let buffer = pending.removeFirst()
            scheduled.append(buffer)
            let current = generation
            output.schedule(buffer) { [weak self] in self?.bufferPlayed(generation: current) }
        }
        if !scheduled.isEmpty { output.play() }
    }

    private func bufferPlayed(generation played: Int) {
        guard played == generation, !scheduled.isEmpty else { return }
        scheduled.removeFirst()
        pump()
        if inputFinished, isEmpty { finishDrain() }
    }

    private func finishDrain() {
        inputFinished = false
        let resumed = waiters
        waiters.removeAll()
        resumed.forEach { $0.resume() }
    }
}
