import Foundation

/// Keeps the photos and files sent in each session, so they survive a reload: Hermes only hands the text
/// back. Entries are matched back to the history by text, in order (like `VoiceNoteStore`). Files live in
/// Application Support (Caches can be purged by iOS).
final class AttachmentStore {
    struct StoredFile: Codable, Hashable {
        var kind: String // "image" | "document"
        var filename: String
        var mimeType: String
        var file: String
        var size: Int
    }

    struct Entry: Codable, Hashable {
        /// The text sent with the files (may be empty).
        var text: String
        var files: [StoredFile]
    }

    static let shared = AttachmentStore()

    let folder: URL
    private let indexURL: URL
    private var index: [String: [Entry]]

    init(folder: URL = URL.applicationSupportDirectory.appending(path: "attachments", directoryHint: .isDirectory)) {
        self.folder = folder
        indexURL = folder.appending(path: "index.json")
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        index = (try? JSONDecoder().decode([String: [Entry]].self, from: Data(contentsOf: indexURL))) ?? [:]
    }

    /// Writes the files of a message just sent; returns them with their stored location.
    func add(text: String, attachments: [LocalAttachment], sessionID: String) -> [LocalAttachment] {
        var stored: [StoredFile] = []
        var result: [LocalAttachment] = []
        for var attachment in attachments {
            let ext = (attachment.filename as NSString).pathExtension
            let name = UUID().uuidString + (ext.isEmpty ? "" : ".\(ext)")
            let url = folder.appending(path: name)
            guard (try? attachment.data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])) != nil else {
                result.append(attachment)
                continue
            }
            stored.append(StoredFile(kind: attachment.kind == .image ? "image" : "document", filename: attachment.filename,
                                     mimeType: attachment.mimeType, file: name, size: attachment.data.count))
            attachment.fileURL = url
            result.append(attachment)
        }
        if !stored.isEmpty {
            index[sessionID, default: []].append(Entry(text: text, files: stored))
            save()
        }
        return result
    }

    func entries(for sessionID: String) -> [Entry] { index[sessionID] ?? [] }

    /// The stored files as attachments. Images are read (they are shown); documents only point to their file.
    func attachments(of entry: Entry) -> [LocalAttachment] {
        entry.files.compactMap { stored in
            let url = folder.appending(path: stored.file)
            guard FileManager.default.fileExists(atPath: url.path(percentEncoded: false)) else { return nil }
            let isImage = stored.kind == "image"
            let data = isImage ? ((try? Data(contentsOf: url)) ?? Data()) : Data()
            return LocalAttachment(kind: isImage ? .image : .document, filename: stored.filename, mimeType: stored.mimeType,
                                   data: data, fileURL: url, storedSize: stored.size)
        }
    }

    private func save() {
        try? JSONEncoder().encode(index).write(to: indexURL, options: .atomic)
    }
}
