import Foundation

/// One message of a session transcript.
public struct HermesMessage: Sendable, Codable, Hashable, Identifiable {
    public enum Role: String, Sendable, Codable, Hashable, CaseIterable {
        case user, assistant, tool, system, notice
    }

    public var id: String
    public var role: Role
    public var text: String
    public var createdAt: Date?
    public var attachments: [MessageAttachment]
    public var toolEvents: [ToolEvent]
    public var reasoning: String?

    public init(
        id: String,
        role: Role,
        text: String,
        createdAt: Date? = nil,
        attachments: [MessageAttachment] = [],
        toolEvents: [ToolEvent] = [],
        reasoning: String? = nil
    ) {
        self.id = id
        self.role = role
        self.text = text
        self.createdAt = createdAt
        self.attachments = attachments
        self.toolEvents = toolEvents
        self.reasoning = reasoning
    }
}

/// An attachment as it appears in a transcript (images are `[image]` placeholders when `inline_images=false`).
public struct MessageAttachment: Sendable, Codable, Hashable {
    public enum Kind: String, Sendable, Codable, Hashable {
        case image, document
    }

    public var kind: Kind
    /// `data:` URI, remote URL or server-side path, when known.
    public var url: String?
    public var filename: String?
    public var mimeType: String?

    public init(kind: Kind, url: String? = nil, filename: String? = nil, mimeType: String? = nil) {
        self.kind = kind
        self.url = url
        self.filename = filename
        self.mimeType = mimeType
    }
}

/// A tool call lifecycle step (`tool.started` / `tool.completed` / `tool.failed`) or a tool call from history.
public struct ToolEvent: Sendable, Codable, Hashable {
    public enum Status: String, Sendable, Codable, Hashable {
        case started, completed, failed
    }

    public var tool: String
    /// Arguments preview (started) or redacted, truncated result preview (completed/failed).
    public var preview: String?
    public var status: Status
    /// Seconds.
    public var duration: TimeInterval?
    public var error: String?
    /// Correlates start and completion when the server provides an id.
    public var callID: String?

    public init(
        tool: String,
        preview: String? = nil,
        status: Status,
        duration: TimeInterval? = nil,
        error: String? = nil,
        callID: String? = nil
    ) {
        self.tool = tool
        self.preview = preview
        self.status = status
        self.duration = duration
        self.error = error
        self.callID = callID
    }
}
