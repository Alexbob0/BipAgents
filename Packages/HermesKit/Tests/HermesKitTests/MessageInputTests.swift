import Foundation
import Synchronization
import Testing
@testable import HermesKit

/// Records uploads and returns a predictable path.
final class RecordingUploader: DocumentUploader {
    let uploads = Mutex<[String]>([])

    func upload(filename: String, mimeType: String, data: Data) async throws -> UploadedDocument {
        uploads.withLock { $0.append(filename) }
        return UploadedDocument(path: "/home/hermes/.hermes/uploads/\(filename)", filename: filename, size: data.count)
    }
}

@Suite("Message input")
struct MessageInputTests {
    @Test func plainTextIsAString() async throws {
        let prepared = try await MessageInput(text: "Bonjour").prepared()
        #expect(prepared.input == "Bonjour")
        #expect(prepared.body() == ["input": "Bonjour"])
    }

    @Test func imagesBecomeMultimodalParts() async throws {
        let jpeg = Data([0xFF, 0xD8, 0xFF])
        let input = MessageInput(text: "Qu'est-ce que c'est ?", attachments: [.image(data: jpeg)])
        let prepared = try await input.prepared()
        #expect(prepared.input == [
            ["type": "text", "text": "Qu'est-ce que c'est ?"],
            ["type": "image_url", "image_url": ["url": "data:image/jpeg;base64,/9j/"]],
        ])
        let imageOnly = try await MessageInput(text: "", attachments: [.image(data: jpeg, mimeType: "image/png")]).prepared()
        #expect(imageOnly.input == [["type": "image_url", "image_url": ["url": "data:image/png;base64,/9j/"]]])
    }

    @Test func documentsAreUploadedAndReferenced() async throws {
        let uploader = RecordingUploader()
        let input = MessageInput(text: "Analyse ce rapport  ", attachments: [
            .document(filename: "rapport.pdf", mimeType: "application/pdf", data: Data(count: 1_234_567)),
            .document(filename: "notes.txt", mimeType: "text/plain", data: Data(count: 512)),
        ])
        let prepared = try await input.prepared(uploader: uploader)
        #expect(uploader.uploads.withLock { $0 } == ["rapport.pdf", "notes.txt"])
        #expect(prepared.text == """
        Analyse ce rapport

        [Pièce jointe : rapport.pdf (application/pdf, 1,2 Mo) → /home/hermes/.hermes/uploads/rapport.pdf]
        [Pièce jointe : notes.txt (text/plain, 512 o) → /home/hermes/.hermes/uploads/notes.txt]
        """)
        #expect(prepared.input == .string(prepared.text))
        #expect(prepared.documents.count == 2)
    }

    @Test func documentsAndImagesCombine() async throws {
        let input = MessageInput(text: "", attachments: [
            .image(data: Data([1])),
            .document(filename: "data.csv", mimeType: "text/csv", data: Data(count: 12_000)),
        ])
        let prepared = try await input.prepared(uploader: RecordingUploader())
        #expect(prepared.input == [
            ["type": "text", "text": "[Pièce jointe : data.csv (text/csv, 12 Ko) → /home/hermes/.hermes/uploads/data.csv]"],
            ["type": "image_url", "image_url": ["url": "data:image/jpeg;base64,AQ=="]],
        ])
    }

    @Test func documentsWithoutUploaderFail() async {
        let input = MessageInput(text: "x", attachments: [.document(filename: "a.pdf", mimeType: "application/pdf", data: Data())])
        await #expect(throws: HermesError.documentUploaderUnavailable) { _ = try await input.prepared() }
    }

    @Test(arguments: [(0, "0 o"), (999, "999 o"), (1_000, "1 Ko"), (12_345, "12,3 Ko"), (1_234_567, "1,2 Mo"),
                      (150_000_000, "150 Mo"), (3_400_000_000, "3,4 Go")])
    func sizeFormatting(_ bytes: Int, _ expected: String) {
        #expect(MessageInput.formatSize(bytes) == expected)
    }

    @Test func bridgeUploaderSendsMultipart() async throws {
        let server = StubServer { _ in .json(["path": "/home/hermes/.hermes/uploads/abc/rapport.pdf", "filename": "rapport.pdf", "size": 3]) }
        let uploader = BridgeDocumentUploader(bridgeURL: URL(string: "https://\(server.host):8643")!, bridgeKey: "bridge-key",
                                              agent: "wellness", session: server.session)
        let uploaded = try await uploader.upload(filename: "rapport.pdf", mimeType: "application/pdf", data: Data("PDF".utf8))
        #expect(uploaded == UploadedDocument(path: "/home/hermes/.hermes/uploads/abc/rapport.pdf", filename: "rapport.pdf", size: 3))

        let request = try #require(server.requests.first)
        #expect(request.method == "POST")
        #expect(request.path == "/v1/files")
        #expect(request.header("Authorization") == "Bearer bridge-key")
        let contentType = try #require(request.header("Content-Type"))
        #expect(contentType.hasPrefix("multipart/form-data; boundary="))
        let boundary = String(contentType.split(separator: "=", maxSplits: 1)[1])
        let body = String(decoding: request.body, as: UTF8.self)
        #expect(body == """
        --\(boundary)\r
        Content-Disposition: form-data; name="agent"\r
        \r
        wellness\r
        --\(boundary)\r
        Content-Disposition: form-data; name="file"; filename="rapport.pdf"\r
        Content-Type: application/pdf\r
        \r
        PDF\r
        --\(boundary)--\r

        """)
    }
}

@Suite("Run input")
struct RunInputTests {
    @Test func photosGoAsOneUserMessage() throws {
        let prepared = PreparedInput(text: "Décris", images: [.init(data: Data([1, 2]), mimeType: "image/jpeg")])
        let body = prepared.runBody(merging: ["session_id": "s1"])
        let messages = try #require(body["input"]?.arrayValue)
        #expect(messages.count == 1 && messages[0]["role"]?.stringValue == "user")
        #expect(messages[0]["content"]?.arrayValue?.count == 2)
        #expect(body["session_id"]?.stringValue == "s1")
        #expect(PreparedInput(text: "Salut").runBody()["input"]?.stringValue == "Salut")
    }
}
