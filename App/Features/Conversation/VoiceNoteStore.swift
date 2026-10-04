import Foundation
import VoiceKit

/// Remembers which messages of a session were voice notes, so audio survives a reload: Hermes only
/// stores the text. Entries are matched back to the history by text, in order. Files live in
/// Application Support (Caches can be purged by iOS).
final class VoiceNoteStore {
    struct Entry: Codable, Hashable {
        enum Kind: String, Codable { case user, reply }
        var kind: Kind
        /// User: the transcript sent. Reply: the text of the assistant message the audio is shown under.
        var text: String
        var file: String
        var duration: TimeInterval
        var waveform: [Float]
    }

    static let shared = VoiceNoteStore()

    let folder: URL
    private let indexURL: URL
    private var index: [String: [Entry]]

    init(folder: URL = URL.applicationSupportDirectory.appending(path: "voice-notes", directoryHint: .isDirectory)) {
        self.folder = folder
        indexURL = folder.appending(path: "index.json")
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        index = (try? JSONDecoder().decode([String: [Entry]].self, from: Data(contentsOf: indexURL))) ?? [:]
    }

    /// Moves `file` into the store and records it for `sessionID`. Returns the file's new URL.
    @discardableResult
    func add(_ kind: Entry.Kind, text: String, file: URL, duration: TimeInterval, waveform: [Float], sessionID: String) -> URL {
        let name = "\(kind.rawValue)-\(UUID().uuidString).\(file.pathExtension)"
        let destination = folder.appending(path: name)
        do {
            try FileManager.default.moveItem(at: file, to: destination)
        } catch {
            return file
        }
        index[sessionID, default: []].append(Entry(kind: kind, text: text, file: name, duration: duration, waveform: waveform))
        save()
        return destination
    }

    func entries(for sessionID: String) -> [Entry] { index[sessionID] ?? [] }

    func url(of entry: Entry) -> URL { folder.appending(path: entry.file) }

    private func save() {
        try? JSONEncoder().encode(index).write(to: indexURL, options: .atomic)
    }
}
