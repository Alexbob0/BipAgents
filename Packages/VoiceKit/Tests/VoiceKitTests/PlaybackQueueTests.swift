import Testing
@testable import VoiceKit

/// Records what the queue does to the output; the test plays the role of the audio hardware.
@MainActor
private final class FakeOutput: AudioOutput {
    var scheduled: [(buffer: Int, completion: @MainActor @Sendable () -> Void)] = []
    var history: [Int] = []
    var stops = 0
    var plays = 0

    func schedule(_ buffer: Int, completion: @escaping @MainActor @Sendable () -> Void) {
        scheduled.append((buffer, completion))
        history.append(buffer)
    }

    func play() { plays += 1 }

    func stop() {
        stops += 1
        scheduled.removeAll()
    }

    /// The oldest scheduled buffer finished playing.
    @discardableResult
    func playNext() -> Int {
        let (buffer, completion) = scheduled.removeFirst()
        completion()
        return buffer
    }
}

@MainActor
@Suite("Playback queue")
struct PlaybackQueueTests {
    @Test func keepsWindowScheduledAndPlaysInOrder() {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output, window: 3)
        (1...5).forEach(queue.enqueue)
        #expect(output.scheduled.map(\.buffer) == [1, 2, 3])
        #expect(queue.pending == [4, 5])
        #expect(output.plays > 0)

        var played: [Int] = []
        while !output.scheduled.isEmpty {
            played.append(output.playNext())
            // At least two buffers stay scheduled while more are available: no gap between sentences.
            if !queue.pending.isEmpty { #expect(output.scheduled.count >= 2) }
        }
        #expect(played == [1, 2, 3, 4, 5])
        #expect(queue.isEmpty)
    }

    @Test func windowIsAtLeastTwo() {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output, window: 1)
        (1...3).forEach(queue.enqueue)
        #expect(output.scheduled.count == 2)
    }

    @Test func finishAndWaitReturnsOnceEverythingPlayed() async {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output)
        (1...4).forEach(queue.enqueue)
        var finished = false
        let waiter = Task { await queue.finishAndWait(); finished = true }
        await Task.yield()
        for _ in 0..<3 { output.playNext() }
        await Task.yield()
        #expect(!finished)
        output.playNext()
        await waiter.value
        #expect(finished)
    }

    @Test func finishAndWaitOnEmptyQueueReturnsImmediately() async {
        let queue = PlaybackQueue(output: FakeOutput())
        await queue.finishAndWait()
        #expect(queue.isEmpty)
    }

    @Test func stopDropsEverythingAndIgnoresLateCompletions() async {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output)
        (1...5).forEach(queue.enqueue)
        output.playNext()
        let stale = output.scheduled
        let waiter = Task { await queue.finishAndWait() }
        await Task.yield()

        queue.stop()
        #expect(output.stops == 1)
        #expect(queue.isEmpty)
        await waiter.value // stop releases the waiter

        // AVAudioPlayerNode calls completions of dropped buffers after stop(): they must be no-ops.
        stale.forEach { $0.completion() }
        #expect(queue.isEmpty)

        // The next reply starts clean.
        output.history.removeAll()
        queue.enqueue(10)
        queue.enqueue(11)
        #expect(output.history == [10, 11])
        stale.forEach { $0.completion() }
        #expect(output.scheduled.map(\.buffer) == [10, 11])
    }

    @Test func resetOutputReschedulesUnplayedBuffersInOrder() {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output, window: 3)
        (1...5).forEach(queue.enqueue)
        #expect(output.playNext() == 1)
        let stale = output.scheduled // 2, 3, 4 on the old player node

        output.scheduled.removeAll() // graph rebuilt: new player node, nothing scheduled
        queue.resetOutput()
        #expect(output.scheduled.map(\.buffer) == [2, 3, 4])
        stale.forEach { $0.completion() } // old node's late callbacks
        #expect(output.scheduled.map(\.buffer) == [2, 3, 4])

        var played: [Int] = []
        while !output.scheduled.isEmpty { played.append(output.playNext()) }
        #expect(played == [2, 3, 4, 5])
    }

    @Test func cancellingTheWaiterStopsPlayback() async {
        let output = FakeOutput()
        let queue = PlaybackQueue(output: output)
        (1...3).forEach(queue.enqueue)
        let waiter = Task { await queue.finishAndWait() }
        await Task.yield()
        waiter.cancel()
        await waiter.value
        #expect(output.stops == 1)
        #expect(queue.isEmpty)
    }
}
