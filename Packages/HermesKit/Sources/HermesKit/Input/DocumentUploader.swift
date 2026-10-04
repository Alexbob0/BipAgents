import Foundation

/// Puts a non-image document somewhere the agent can read it, and returns its server-side path.
public protocol DocumentUploader: Sendable {
    func upload(filename: String, mimeType: String, data: Data) async throws -> UploadedDocument
}

public struct UploadedDocument: Sendable, Hashable, Codable {
    /// Path (or reference) the agent can open, e.g. `/home/hermes/.hermes/uploads/…/rapport.pdf`.
    public var path: String
    public var filename: String?
    public var size: Int?

    public init(path: String, filename: String? = nil, size: Int? = nil) {
        self.path = path
        self.filename = filename
        self.size = size
    }
}

/// Uploads through the bridge: `POST {bridgeURL}/v1/files`, `multipart/form-data` with fields
/// `agent` and `file`, Bearer bridge key. Response: `{"path": "...", "filename": "...", "size": n}`.
/// The bridge stores the file inside the agent's Hermes home so the agent's tools can read it.
public struct BridgeDocumentUploader: DocumentUploader {
    public var bridgeURL: URL
    public var bridgeKey: String
    /// Agent identifier known to the bridge (e.g. `wellness`).
    public var agent: String
    public var session: URLSession

    public init(bridgeURL: URL, bridgeKey: String, agent: String, session: URLSession = .shared) {
        self.bridgeURL = bridgeURL
        self.bridgeKey = bridgeKey
        self.agent = agent
        self.session = session
    }

    public func upload(filename: String, mimeType: String, data: Data) async throws -> UploadedDocument {
        var form = MultipartFormData()
        form.addField(name: "agent", value: agent)
        form.addFile(name: "file", filename: filename, mimeType: mimeType, data: data)

        var request = URLRequest(url: bridgeURL.appending(path: "v1/files"))
        request.httpMethod = "POST"
        request.setValue("Bearer \(bridgeKey)", forHTTPHeaderField: "Authorization")
        request.setValue(form.contentType, forHTTPHeaderField: "Content-Type")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.httpBody = form.body

        let (body, _) = try await HTTP.send(request, session: session)
        let json = try HTTP.json(body)
        guard let path = json["path"]?.stringValue, !path.isEmpty else {
            throw HermesError.invalidResponse("Upload response has no path")
        }
        return UploadedDocument(path: path, filename: json["filename"]?.stringValue, size: json["size"]?.intValue ?? data.count)
    }
}

/// Minimal `multipart/form-data` encoder.
struct MultipartFormData {
    let boundary = "HermesKit-\(UUID().uuidString)"
    private(set) var body = Data()

    var contentType: String { "multipart/form-data; boundary=\(boundary)" }

    mutating func addField(name: String, value: String) {
        appendPart(disposition: "form-data; name=\"\(Self.escape(name))\"", contentType: nil, data: Data(value.utf8))
    }

    mutating func addFile(name: String, filename: String, mimeType: String, data: Data) {
        appendPart(disposition: "form-data; name=\"\(Self.escape(name))\"; filename=\"\(Self.escape(filename))\"",
                   contentType: mimeType, data: data)
    }

    private mutating func appendPart(disposition: String, contentType: String?, data: Data) {
        if body.isEmpty == false { body.removeLast(Self.closing(boundary).count) }
        var head = "--\(boundary)\r\nContent-Disposition: \(disposition)\r\n"
        if let contentType { head += "Content-Type: \(contentType)\r\n" }
        head += "\r\n"
        body.append(Data(head.utf8))
        body.append(data)
        body.append(Data("\r\n".utf8))
        body.append(Self.closing(boundary))
    }

    private static func closing(_ boundary: String) -> Data { Data("--\(boundary)--\r\n".utf8) }

    /// Quoted-string safe: no quotes, no line breaks.
    private static func escape(_ value: String) -> String {
        value.replacingOccurrences(of: "\"", with: "%22")
            .replacingOccurrences(of: "\r", with: "")
            .replacingOccurrences(of: "\n", with: "")
    }
}
