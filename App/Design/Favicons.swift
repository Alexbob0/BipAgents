import Observation
import UIKit

/// Site icons shown before links in the agents' replies, loaded on demand and kept for the session.
///
/// Fetched from DuckDuckGo's icon service (`icons.duckduckgo.com/ip3/<host>.ico`): one URL per host, no
/// cookies, and the site itself does not learn the conversation was read. Icons are redrawn at text size
/// with rounded corners so they sit on the text baseline like a glyph.
@MainActor
@Observable
final class Favicons {
    static let shared = Favicons()

    private(set) var icons: [String: UIImage] = [:]
    @ObservationIgnored private var requested: Set<String> = []

    /// The host's icon if already loaded; starts loading it otherwise (the view updates when it arrives).
    func icon(for host: String) -> UIImage? {
        if let icon = icons[host] { return icon }
        if !requested.contains(host) {
            requested.insert(host)
            Task { await load(host) }
        }
        return nil
    }

    private func load(_ host: String) async {
        guard let url = URL(string: "https://icons.duckduckgo.com/ip3/\(host).ico"),
              let (data, response) = try? await URLSession.shared.data(from: url),
              (response as? HTTPURLResponse)?.statusCode == 200,
              let image = UIImage(data: data), image.size.width >= 8 else { return }
        icons[host] = Self.glyph(from: image)
    }

    /// 16 pt square, rounded, so it reads as a small badge before the link.
    private static func glyph(from image: UIImage) -> UIImage {
        let size = CGSize(width: 16, height: 16)
        return UIGraphicsImageRenderer(size: size).image { _ in
            let rect = CGRect(origin: .zero, size: size)
            UIBezierPath(roundedRect: rect, cornerRadius: 4).addClip()
            image.draw(in: rect)
        }
    }
}
