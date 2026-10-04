import Foundation

/// A decision for `POST /v1/runs/{id}/approval`.
public enum ApprovalChoice: String, Sendable, Codable, Hashable, CaseIterable {
    case once, session, always, deny

    /// Accepts the server's aliases (`approve`, `approved`, `allow` → `once`).
    public init?(lenient raw: String) {
        let value = raw.trimmingCharacters(in: .whitespaces).lowercased()
        switch value {
        case "approve", "approved", "allow", "yes": self = .once
        case "denied", "reject", "no": self = .deny
        default: self.init(rawValue: value)
        }
    }

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        guard let choice = ApprovalChoice(lenient: raw) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "Unknown approval choice \(raw)"))
        }
        self = choice
    }
}

/// Payload of an `approval.request` event: the run is parked in `waiting_for_approval`.
public struct ApprovalRequest: Sendable, Codable, Hashable, Identifiable {
    public var runID: String
    public var requestID: String?
    /// Already redacted server-side.
    public var command: String?
    public var description: String?
    /// Subset of once/session/always/deny the client may send back.
    public var choices: [ApprovalChoice]
    public var sessionID: String?
    /// Set by the app (the server does not know about app-side agent ids).
    public var agentID: UUID?
    /// Every other field of the event (e.g. `pattern_key`, `smart_denied`, `timestamp`).
    public var extra: [String: JSONValue]

    public var id: String { requestID.map { "\(runID)/\($0)" } ?? runID }

    public init(
        runID: String,
        requestID: String? = nil,
        command: String? = nil,
        description: String? = nil,
        choices: [ApprovalChoice] = [.once, .deny],
        sessionID: String? = nil,
        agentID: UUID? = nil,
        extra: [String: JSONValue] = [:]
    ) {
        self.runID = runID
        self.requestID = requestID
        self.command = command
        self.description = description
        self.choices = choices
        self.sessionID = sessionID
        self.agentID = agentID
        self.extra = extra
    }
}

/// Body of a successful approval resolution.
public struct ApprovalResult: Sendable, Hashable {
    public var runID: String?
    public var choice: ApprovalChoice?
    public var requestID: String?
    /// Number of pending approvals resolved.
    public var resolved: Int?
}
