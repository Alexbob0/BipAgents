import Foundation

// Everything that shapes the request body of a user turn lives in this file
// (`POST /api/sessions/{id}/chat/stream`, `POST /v1/runs`), so it can be adjusted in one place.
//
// - No images: `{"input": "text"}`.
// - Images: `{"input": [{"type":"text","text":…}, {"type":"image_url","image_url":{"url":"data:image/jpeg;base64,…"}}]}`
//   (OpenAI multimodal parts; Hermes documents inline images for `chat` and `chat/stream`).
// - Documents (pdf, xlsx, csv, txt, docx…): the api_server rejects non-image content, so documents are
//   uploaded out of band (`DocumentUploader`) and referenced in the text, the same way the Telegram
//   adapter hands documents to the agent as cached files on disk.

/// What the user sends: text plus attachments.
public struct MessageInput: Sendable, Hashable, ExpressibleByStringLiteral {
    public var text: String
    public var attachments: [Attachment]

    public init(text: String, attachments: [Attachment] = []) {
        self.text = text
        self.attachments = attachments
    }

    public init(stringLiteral value: String) {
        self.init(text: value)
    }
}

public enum Attachment: Sendable, Hashable {
    /// Already downscaled (≤ 1600 px) and encoded by the caller.
    case image(data: Data, mimeType: String = "image/jpeg")
    case document(filename: String, mimeType: String, data: Data)
}

/// A document that now exists server-side, ready to be referenced in the message text.
public struct DocumentReference: Sendable, Hashable {
    public var filename: String
    public var mimeType: String
    public var size: Int
    public var path: String

    public init(filename: String, mimeType: String, size: Int, path: String) {
        self.filename = filename
        self.mimeType = mimeType
        self.size = size
        self.path = path
    }
}

/// A turn ready to be sent: documents are uploaded and referenced, images are inline.
public struct PreparedInput: Sendable, Hashable {
    public struct Image: Sendable, Hashable {
        public var data: Data
        public var mimeType: String
    }

    /// User text followed by the document reference block, if any.
    public var text: String
    public var images: [Image]
    public var documents: [DocumentReference]

    public init(text: String, images: [Image] = [], documents: [DocumentReference] = []) {
        self.text = text
        self.images = images
        self.documents = documents
    }

    /// The `input` field: a plain string, or multimodal parts when there are images.
    public var input: JSONValue {
        guard !images.isEmpty else { return .string(text) }
        var parts: [JSONValue] = text.isEmpty ? [] : [["type": "text", "text": .string(text)]]
        for image in images {
            let url = "data:\(image.mimeType);base64,\(image.data.base64EncodedString())"
            parts.append(["type": "image_url", "image_url": ["url": .string(url)]])
        }
        return .array(parts)
    }

    /// Request body for `chat/stream` and `/v1/runs`, plus caller-provided fields.
    public func body(merging extra: [String: JSONValue] = [:]) -> JSONValue {
        .object(extra.merging(["input": input]) { _, new in new })
    }
}

extension MessageInput {
    /// Uploads documents (sequentially) and builds the request payload.
    /// Throws `HermesError.documentUploaderUnavailable` when documents are present without an uploader.
    public func prepared(uploader: (any DocumentUploader)? = nil) async throws -> PreparedInput {
        var images: [PreparedInput.Image] = []
        var documents: [DocumentReference] = []
        for attachment in attachments {
            switch attachment {
            case .image(let data, let mimeType):
                images.append(.init(data: data, mimeType: mimeType))
            case .document(let filename, let mimeType, let data):
                guard let uploader else { throw HermesError.documentUploaderUnavailable }
                let uploaded = try await uploader.upload(filename: filename, mimeType: mimeType, data: data)
                documents.append(DocumentReference(filename: uploaded.filename ?? filename, mimeType: mimeType,
                                                   size: uploaded.size ?? data.count, path: uploaded.path))
            }
        }
        return PreparedInput(text: Self.compose(text: text, documents: documents), images: images, documents: documents)
    }

    /// User text, a blank line, then one reference line per document.
    public static func compose(text: String, documents: [DocumentReference]) -> String {
        let block = documents.map(referenceLine).joined(separator: "\n")
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return [trimmed, block].filter { !$0.isEmpty }.joined(separator: "\n\n")
    }

    /// `[Pièce jointe : rapport.pdf (application/pdf, 1,2 Mo) → /home/hermes/.hermes/uploads/…]`
    public static func referenceLine(_ document: DocumentReference) -> String {
        "[Pièce jointe : \(document.filename) (\(document.mimeType), \(formatSize(document.size))) → \(document.path)]"
    }

    /// French byte count, decimal units: `512 o`, `12 Ko`, `1,2 Mo`, `3,4 Go`.
    public static func formatSize(_ bytes: Int) -> String {
        let units = ["o", "Ko", "Mo", "Go"]
        var value = Double(bytes)
        var unit = 0
        while value >= 1000, unit < units.count - 1 {
            value /= 1000
            unit += 1
        }
        if unit == 0 { return "\(bytes) o" }
        let rounded = (value * 10).rounded() / 10
        let number = rounded.rounded() == rounded || rounded >= 100
            ? String(Int(rounded.rounded()))
            : String(format: "%.1f", rounded).replacingOccurrences(of: ".", with: ",")
        return "\(number) \(units[unit])"
    }
}
