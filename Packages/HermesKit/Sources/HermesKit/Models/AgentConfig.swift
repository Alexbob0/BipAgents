import Foundation

/// Non-secret configuration of one Hermes agent (one api_server gateway = one profile).
/// Secrets (`apiKey`, `bridgeKey`) live in `AgentSecrets` and belong in the Keychain.
public struct AgentConfig: Sendable, Codable, Hashable, Identifiable {
    public var id: UUID
    public var name: String
    /// e.g. `https://aibox.example.ts.net:8642`
    public var baseURL: URL
    /// Kyutai voice alias, e.g. `5476`.
    public var voice: String?
    public var colorTag: String?
    public var category: String?
    public var bridgeURL: URL?
    public var defaultSessionID: String?
    /// Language the agent is spoken to in (`fr`, `en`, `es`, `de`); `nil` for agents saved before it existed.
    public var language: String?

    public init(
        id: UUID = UUID(),
        name: String,
        baseURL: URL,
        voice: String? = nil,
        colorTag: String? = nil,
        category: String? = nil,
        bridgeURL: URL? = nil,
        defaultSessionID: String? = nil,
        language: String? = nil
    ) {
        self.id = id
        self.name = name
        self.baseURL = baseURL
        self.voice = voice
        self.colorTag = colorTag
        self.category = category
        self.bridgeURL = bridgeURL
        self.defaultSessionID = defaultSessionID
        self.language = language
    }
}

/// Secrets for one agent. Never persist this outside the Keychain.
public struct AgentSecrets: Sendable, Hashable {
    public var apiKey: String
    public var bridgeKey: String?

    public init(apiKey: String, bridgeKey: String? = nil) {
        self.apiKey = apiKey
        self.bridgeKey = bridgeKey
    }
}

public enum AgentConfigError: Error, Sendable, Equatable {
    case invalidJSON
    case missingField(String)
    case invalidURL(field: String, value: String)
}

/// Result of scanning a provisioning QR code:
/// `{"name","baseURL","apiKey","voice","bridgeURL","bridgeKey"}` (SPEC §C3).
/// Config and secrets are returned separately so the caller can route secrets to the Keychain.
public struct AgentProvisioning: Sendable, Hashable {
    public var config: AgentConfig
    public var secrets: AgentSecrets

    public init(qrPayload: String, id: UUID = UUID()) throws {
        try self.init(qrPayload: Data(qrPayload.utf8), id: id)
    }

    public init(qrPayload: Data, id: UUID = UUID()) throws {
        guard let json = try? JSONValue.parse(qrPayload), json.objectValue != nil else {
            throw AgentConfigError.invalidJSON
        }
        let fields = LenientFields(json, nestedIn: [])
        func required(_ keys: String...) throws -> String {
            guard let value = fields.value(keys)?.lenientString?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !value.isEmpty else { throw AgentConfigError.missingField(keys[0]) }
            return value
        }
        func optional(_ keys: String...) -> String? {
            fields.value(keys)?.lenientString?.trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
        }
        func url(_ field: String, _ raw: String) throws -> URL {
            guard let url = URL(string: raw), let scheme = url.scheme?.lowercased(),
                  ["https", "http"].contains(scheme), url.host() != nil else {
                throw AgentConfigError.invalidURL(field: field, value: raw)
            }
            return url
        }

        let name = try required("name")
        let baseURL = try url("baseURL", try required("baseURL", "base_url", "url"))
        let apiKey = try required("apiKey", "api_key", "key")
        let bridgeURL = try optional("bridgeURL", "bridge_url").map { try url("bridgeURL", $0) }

        config = AgentConfig(
            id: id,
            name: name,
            baseURL: baseURL,
            voice: optional("voice"),
            colorTag: optional("color", "colorTag"),
            category: optional("category"),
            bridgeURL: bridgeURL
        )
        secrets = AgentSecrets(apiKey: apiKey, bridgeKey: optional("bridgeKey", "bridge_key"))
    }
}

extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
