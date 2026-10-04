import Foundation

/// `GET /health`.
public struct HermesHealth: Sendable, Hashable {
    public var status: String
    public var isOK: Bool { status.lowercased() == "ok" }
}

/// `GET /v1/capabilities`, decoded leniently.
public struct HermesCapabilities: Sendable, Hashable {
    public var platform: String?
    public var model: String?
    /// Feature flags. Object-valued features (`{"supported": true, …}`) are reduced to their
    /// `supported` / `enabled` flag; any other non-null, non-false value counts as supported.
    public var features: [String: Bool]
    public var endpoints: [String: JSONValue]
    public var raw: JSONValue

    public func supports(_ feature: String) -> Bool { features[feature] ?? false }

    /// Features the app relies on (SPEC §B1).
    public static let required = ["run_submission", "run_events_sse", "run_approval"]

    public var missingRequiredFeatures: [String] { Self.required.filter { !supports($0) } }
}
