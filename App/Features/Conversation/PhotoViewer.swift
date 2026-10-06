import ImageIO
import SwiftUI

/// A sent photo, full screen: pinch or double-tap to zoom, drag to pan, swipe down (or ✕) to close,
/// share button.
/// A photo opened full screen (`.fullScreenCover(item:)`).
struct ViewedPhoto: Identifiable {
    let id = UUID()
    let image: UIImage
}

struct PhotoViewer: View {
    var image: UIImage
    @Environment(\.dismiss) private var dismiss

    @State private var scale: CGFloat = 1
    @State private var lastScale: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var lastOffset: CGSize = .zero

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            Image(uiImage: image)
                .resizable()
                .scaledToFit()
                .scaleEffect(scale)
                .offset(offset)
                .gesture(magnify.simultaneously(with: drag))
                .onTapGesture(count: 2) {
                    withAnimation(.snappy) {
                        if scale > 1 { reset() } else { scale = 2.5; lastScale = 2.5 }
                    }
                }
                .accessibilityLabel("Photo envoyée")
        }
        .overlay(alignment: .top) {
            HStack {
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 44, height: 44)
                        .background(.ultraThinMaterial, in: .circle)
                }
                .accessibilityLabel("Fermer")
                Spacer()
                ShareLink(item: Image(uiImage: image), preview: SharePreview("Photo", image: Image(uiImage: image))) {
                    Image(systemName: "square.and.arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .frame(width: 44, height: 44)
                        .background(.ultraThinMaterial, in: .circle)
                }
                .accessibilityLabel("Partager")
            }
            .foregroundStyle(.white)
            .padding(.horizontal, 16)
        }
        .statusBarHidden()
    }

    private var magnify: some Gesture {
        MagnifyGesture()
            .onChanged { value in scale = max(1, min(6, lastScale * value.magnification)) }
            .onEnded { _ in
                lastScale = scale
                if scale <= 1.01 { withAnimation(.snappy) { reset() } }
            }
    }

    private var drag: some Gesture {
        DragGesture()
            .onChanged { value in
                if scale > 1 {
                    offset = CGSize(width: lastOffset.width + value.translation.width,
                                    height: lastOffset.height + value.translation.height)
                } else {
                    offset = CGSize(width: 0, height: max(0, value.translation.height)) // pull down to close
                }
            }
            .onEnded { value in
                if scale > 1 {
                    lastOffset = offset
                } else if value.translation.height > 120 {
                    dismiss()
                } else {
                    withAnimation(.snappy) { offset = .zero }
                }
            }
    }

    private func reset() {
        scale = 1
        lastScale = 1
        offset = .zero
        lastOffset = .zero
    }
}

/// Downsampled photos for the thread (decoded once, not on every redraw).
enum PhotoThumbnails {
    private static let cache = NSCache<NSString, UIImage>()

    static func image(for attachment: LocalAttachment) -> UIImage? {
        guard attachment.kind == .image, !attachment.data.isEmpty else { return nil }
        let key = attachment.id.uuidString as NSString
        if let cached = cache.object(forKey: key) { return cached }
        let options = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithData(attachment.data as CFData, options),
              let thumbnail = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                  kCGImageSourceCreateThumbnailFromImageAlways: true,
                  kCGImageSourceCreateThumbnailWithTransform: true,
                  kCGImageSourceThumbnailMaxPixelSize: 720,
              ] as CFDictionary) else { return nil }
        let image = UIImage(cgImage: thumbnail)
        cache.setObject(image, forKey: key)
        return image
    }
}
