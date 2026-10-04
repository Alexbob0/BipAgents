import PhotosUI
import SwiftUI
import UniformTypeIdentifiers
import VoiceKit

struct Composer: View {
    @Binding var text: String
    @Binding var attachments: [LocalAttachment]
    var palette: AgentPalette
    var placeholder: String
    var isRunning: Bool
    var voice: VoiceEngine
    var onSend: () -> Void
    var onStop: () -> Void
    /// Push-to-talk result, sent right away.
    var onDictated: (String, VoiceRecording?) -> Void
    /// Short tap on the mic: open the hands-free call.
    var onCall: () -> Void

    var appearance: AgentAppearance

    @State private var dictation = DictationController()
    @State private var isTakingPhoto = false
    @State private var isScanning = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var isPickingPhotos = false
    @State private var isImportingFiles = false
    @State private var importError: String?
    @FocusState private var isFocused: Bool

    private var canSend: Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || !attachments.isEmpty
    }

    var body: some View {
        VStack(spacing: 8) {
            if dictation.isActive {
                DictationPanel(voice: voice, controller: dictation, appearance: appearance)
                    .padding(.bottom, 6)
            }
            if !attachments.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(attachments) { attachment in
                            AttachmentChip(attachment: attachment, onDark: false) {
                                attachments.removeAll { $0.id == attachment.id }
                            }
                        }
                    }
                    .padding(.horizontal, 12)
                }
            }
            if let importError {
                Text(importError).font(Theme.body(12, weight: .bold)).foregroundStyle(Theme.danger)
            }
            HStack(alignment: .bottom, spacing: 8) {
                Menu {
                    Button("Photos", systemImage: "photo.on.rectangle") { isPickingPhotos = true }
                    Button("Fichiers (PDF, Excel, texte…)", systemImage: "doc") { isImportingFiles = true }
                    if CameraPicker.isAvailable {
                        Button("Caméra", systemImage: "camera") { isTakingPhoto = true }
                    }
                    if DocumentScanner.isAvailable {
                        Button("Scanner un document", systemImage: "doc.viewfinder") { isScanning = true }
                    }
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundStyle(Theme.ink)
                        .frame(width: 46, height: 46)
                        .background(Theme.field, in: .circle)
                }
                .accessibilityLabel("Joindre une photo ou un fichier")

                TextField(placeholder, text: $text, axis: .vertical)
                    .font(Theme.body(16))
                    .lineLimit(1...6)
                    .focused($isFocused)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 12)
                    .background(Theme.field, in: .rect(cornerRadius: 23, style: .continuous))

                trailingButton
            }
            .padding(.horizontal, 12)
        }
        .padding(.top, 10)
        .padding(.bottom, 8)
        .background(.bar)
        .animation(.snappy, value: dictation.isActive)
        .photosPicker(isPresented: $isPickingPhotos, selection: $photoItems, maxSelectionCount: 6, matching: .images)
        .fileImporter(isPresented: $isImportingFiles, allowedContentTypes: [.item], allowsMultipleSelection: true, onCompletion: importFiles)
        .onChange(of: photoItems) { _, items in
            Task { await loadPhotos(items) }
        }
        .fullScreenCover(isPresented: $isTakingPhoto) {
            CameraPicker { data in
                if let jpeg = ImageDownscaler.jpeg(from: data, maxDimension: 1600) {
                    attachments.append(LocalAttachment(kind: .image, filename: "photo.jpg", mimeType: "image/jpeg", data: jpeg))
                }
            }
            .ignoresSafeArea()
        }
        .fullScreenCover(isPresented: $isScanning) {
            DocumentScanner { pdf in
                attachments.append(LocalAttachment(kind: .document, filename: "scan-\(Date.now.formatted(.iso8601.year().month().day())).pdf",
                                                   mimeType: "application/pdf", data: pdf))
            }
            .ignoresSafeArea()
        }
    }

    @ViewBuilder
    private var trailingButton: some View {
        if isRunning {
            Button(action: onStop) {
                Image(systemName: "stop.fill")
                    .font(.system(size: 17, weight: .bold))
                    .foregroundStyle(Theme.onInk)
                    .frame(width: 46, height: 46)
                    .background(Theme.ink, in: .circle)
            }
            .accessibilityLabel("Arrêter")
        } else if canSend {
            Button(action: onSend) {
                Image(systemName: "arrow.up")
                    .font(.system(size: 19, weight: .heavy))
                    .foregroundStyle(.white)
                    .frame(width: 46, height: 46)
                    .background(palette.deep, in: .circle)
            }
            .accessibilityLabel("Envoyer")
            .sensoryFeedback(.impact(weight: .light), trigger: attachments.count)
        } else {
            DictationButton(voice: voice, controller: dictation, palette: palette, onDictated: onDictated, onTap: onCall)
        }
    }

    // MARK: Import

    private func loadPhotos(_ items: [PhotosPickerItem]) async {
        guard !items.isEmpty else { return }
        for item in items {
            guard let data = try? await item.loadTransferable(type: Data.self),
                  let jpeg = ImageDownscaler.jpeg(from: data, maxDimension: 1600) else { continue }
            attachments.append(LocalAttachment(kind: .image, filename: "photo.jpg", mimeType: "image/jpeg", data: jpeg))
        }
        photoItems = []
    }

    private func importFiles(_ result: Result<[URL], any Error>) {
        importError = nil
        switch result {
        case .success(let urls):
            for url in urls {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                guard let data = try? Data(contentsOf: url) else {
                    importError = "Impossible de lire \(url.lastPathComponent)."
                    continue
                }
                let type = UTType(filenameExtension: url.pathExtension) ?? .data
                if type.conforms(to: .image), let jpeg = ImageDownscaler.jpeg(from: data, maxDimension: 1600) {
                    attachments.append(LocalAttachment(kind: .image, filename: url.lastPathComponent, mimeType: "image/jpeg", data: jpeg))
                } else {
                    attachments.append(LocalAttachment(kind: .document, filename: url.lastPathComponent,
                                                       mimeType: type.preferredMIMEType ?? "application/octet-stream", data: data))
                }
            }
        case .failure(let error):
            importError = error.localizedDescription
        }
    }
}

struct AttachmentChip: View {
    var attachment: LocalAttachment
    var onDark: Bool
    var onRemove: (() -> Void)?

    var body: some View {
        HStack(spacing: 10) {
            preview
            VStack(alignment: .leading, spacing: 1) {
                Text(attachment.filename)
                    .font(Theme.body(14, weight: .heavy))
                    .lineLimit(1)
                Text(meta)
                    .font(Theme.body(12))
                    .opacity(0.7)
            }
            .foregroundStyle(onDark ? Theme.onInk : Theme.ink)
            if let onRemove {
                Button(action: onRemove) {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(Theme.muted)
                }
                .accessibilityLabel("Retirer \(attachment.filename)")
            }
        }
        .padding(8)
        .background(onDark ? Theme.onInk.opacity(0.1) : Theme.field, in: .rect(cornerRadius: 16, style: .continuous))
    }

    @ViewBuilder
    private var preview: some View {
        if attachment.kind == .image, let image = UIImage(data: attachment.data) {
            Image(uiImage: image)
                .resizable()
                .scaledToFill()
                .frame(width: 40, height: 40)
                .clipShape(.rect(cornerRadius: 8))
        } else {
            FileBadge(filename: attachment.filename)
        }
    }

    private var meta: String {
        let size = ByteCountFormatter.string(fromByteCount: Int64(attachment.data.count), countStyle: .file)
        return "\(FileBadge.label(for: attachment.filename)) · \(size)"
    }
}

struct FileBadge: View {
    var filename: String

    var body: some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(.white)
            .stroke(Theme.line, lineWidth: 1.5)
            .frame(width: 34, height: 42)
            .overlay(alignment: .bottom) {
                Text(Self.label(for: filename))
                    .font(.system(size: 9, weight: .black, design: .rounded))
                    .lineLimit(1)
                    .fixedSize()
                    .foregroundStyle(.white)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1)
                    .background(Self.color(for: filename), in: .rect(cornerRadius: 4))
                    .padding(.bottom, 6)
            }
    }

    static func label(for filename: String) -> String {
        let ext = (filename as NSString).pathExtension.uppercased()
        return ext.isEmpty ? "DOC" : String(ext.prefix(3))
    }

    static func color(for filename: String) -> Color {
        switch (filename as NSString).pathExtension.lowercased() {
        case "pdf": Color(hex: 0xE5484D)
        case "xls", "xlsx", "numbers", "csv": Color(hex: 0x1D8A4F)
        case "doc", "docx", "pages", "rtf": Color(hex: 0x2B55D4)
        case "ppt", "pptx", "key": Color(hex: 0xC9472A)
        default: Color(hex: 0x5E6472)
        }
    }
}

enum ImageDownscaler {
    /// Re-encodes as JPEG with the longest side ≤ `maxDimension` (SPEC §A4).
    static func jpeg(from data: Data, maxDimension: CGFloat, quality: CGFloat = 0.82) -> Data? {
        guard let image = UIImage(data: data) else { return nil }
        let longest = max(image.size.width, image.size.height)
        let scale = min(1, maxDimension / max(longest, 1))
        let size = CGSize(width: (image.size.width * scale).rounded(), height: (image.size.height * scale).rounded())
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let resized = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }
        return resized.jpegData(compressionQuality: quality)
    }
}
