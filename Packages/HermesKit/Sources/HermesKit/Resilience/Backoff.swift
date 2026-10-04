import Foundation

/// Exponential backoff with jitter: 0.5 s, 1 s, 2 s, 4 s, 8 s, 8 s… (±20 %, capped at `maximum`).
public struct Backoff: Sendable, Hashable {
    public var initial: TimeInterval
    public var maximum: TimeInterval
    public var multiplier: Double
    /// Relative jitter amplitude (0.2 = ±20 %).
    public var jitter: Double
    public private(set) var attempt = 0

    public init(initial: TimeInterval = 0.5, maximum: TimeInterval = 8, multiplier: Double = 2, jitter: Double = 0.2) {
        self.initial = initial
        self.maximum = maximum
        self.multiplier = multiplier
        self.jitter = jitter
    }

    /// Pure delay computation; `unitRandom` in 0...1 (0.5 = no jitter).
    public func delay(forAttempt attempt: Int, unitRandom: Double) -> TimeInterval {
        let base = min(maximum, initial * pow(multiplier, Double(max(0, attempt))))
        let jittered = base * (1 + jitter * (2 * unitRandom - 1))
        return min(maximum, max(0, jittered))
    }

    /// Delay for the current attempt, then advances.
    public mutating func next() -> Duration {
        defer { attempt += 1 }
        return .seconds(delay(forAttempt: attempt, unitRandom: .random(in: 0...1)))
    }

    public mutating func reset() { attempt = 0 }
}
