import Foundation

/// Metadata of a Hermes session (mirror of `GET /api/sessions` rows).
public struct HermesSession: Sendable, Codable, Hashable, Identifiable {
    public var id: String
    public var title: String?
    public var createdAt: Date?
    public var updatedAt: Date?
    public var lastMessagePreview: String?
    public var messageCount: Int?
    public var source: String?
    public var model: String?
    public var parentSessionID: String?

    public init(
        id: String,
        title: String? = nil,
        createdAt: Date? = nil,
        updatedAt: Date? = nil,
        lastMessagePreview: String? = nil,
        messageCount: Int? = nil,
        source: String? = nil,
        model: String? = nil,
        parentSessionID: String? = nil
    ) {
        self.id = id
        self.title = title
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.lastMessagePreview = lastMessagePreview
        self.messageCount = messageCount
        self.source = source
        self.model = model
        self.parentSessionID = parentSessionID
    }
}
