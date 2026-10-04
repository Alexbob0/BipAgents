import Foundation

public enum RunStatus: Sendable, Hashable, Codable, RawRepresentable {
    case running
    case waitingForApproval
    case completed
    case failed
    case cancelled
    case interrupted
    case stopping
    case unknown(String)

    public init(rawValue: String) {
        switch rawValue.lowercased() {
        case "running", "started", "in_progress": self = .running
        case "waiting_for_approval": self = .waitingForApproval
        case "completed": self = .completed
        case "failed": self = .failed
        case "cancelled", "canceled": self = .cancelled
        case "interrupted": self = .interrupted
        case "stopping": self = .stopping
        default: self = .unknown(rawValue)
        }
    }

    public var rawValue: String {
        switch self {
        case .running: "running"
        case .waitingForApproval: "waiting_for_approval"
        case .completed: "completed"
        case .failed: "failed"
        case .cancelled: "cancelled"
        case .interrupted: "interrupted"
        case .stopping: "stopping"
        case .unknown(let raw): raw
        }
    }

    public var isTerminal: Bool {
        switch self {
        case .completed, .failed, .cancelled, .interrupted: true
        default: false
        }
    }

    public init(from decoder: Decoder) throws {
        self.init(rawValue: try decoder.singleValueContainer().decode(String.self))
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

/// Token accounting (`usage` on `run.completed` and `GET /v1/runs/{id}`).
public struct TokenUsage: Sendable, Codable, Hashable {
    public var inputTokens: Int?
    public var outputTokens: Int?
    public var totalTokens: Int?
    public var cacheReadTokens: Int?
    public var cacheWriteTokens: Int?

    public init(inputTokens: Int? = nil, outputTokens: Int? = nil, totalTokens: Int? = nil,
                cacheReadTokens: Int? = nil, cacheWriteTokens: Int? = nil) {
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.totalTokens = totalTokens
        self.cacheReadTokens = cacheReadTokens
        self.cacheWriteTokens = cacheWriteTokens
    }
}

/// How a turn ended (terminal `run.*` events, `assistant.completed`).
public struct RunOutcome: Sendable, Hashable {
    public var output: String?
    public var error: String?
    public var usage: TokenUsage?
    public var completed: Bool?
    public var partial: Bool?
    public var interrupted: Bool?
    /// e.g. `interrupted_by_user`, `max_iterations_reached(60/60)`.
    public var turnExitReason: String?
    /// Steer text that never reached the agent; replay it as the next user turn.
    public var pendingSteer: String?

    public init(output: String? = nil, error: String? = nil, usage: TokenUsage? = nil, completed: Bool? = nil,
                partial: Bool? = nil, interrupted: Bool? = nil, turnExitReason: String? = nil, pendingSteer: String? = nil) {
        self.output = output
        self.error = error
        self.usage = usage
        self.completed = completed
        self.partial = partial
        self.interrupted = interrupted
        self.turnExitReason = turnExitReason
        self.pendingSteer = pendingSteer
    }
}

/// `GET /v1/runs/{id}`.
public struct HermesRun: Sendable, Hashable {
    public var runID: String
    public var status: RunStatus
    public var sessionID: String?
    public var model: String?
    public var outcome: RunOutcome
    /// Present while the gateway drains for shutdown; the run will end `interrupted` at the latest.
    public var shutdownRequestedAt: Date?
    public var raw: JSONValue
}

/// Response of `POST /v1/runs`.
public struct RunHandle: Sendable, Hashable {
    public var runID: String
    public var status: RunStatus
    /// `Idempotency-Replayed: true` — an identical earlier request already created this run.
    public var replayed: Bool
}
