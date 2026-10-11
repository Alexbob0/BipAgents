import HermesKit
import SwiftUI
import WidgetKit

/// What the home-screen widgets show, written to the App Group (`widgets.json`, and each Bip on a transparent
/// background in `avatars/<id>-bip.png`): the widget extension has no network and no Keychain, only this. Rewritten
/// when an agent, its look or its last message changes; the widgets are then reloaded.
@MainActor
enum WidgetExport {
    struct Agent: Codable, Equatable {
        var id: String
        var name: String
        var main: UInt32
        var deep: UInt32
        var tintLight: UInt32
        var tintDark: UInt32
        var preview: String?
        var updatedAt: Date?
    }

    private static var lastWritten: [Agent] = []

    static func export(_ agents: [AgentProfile], latest: [UUID: HermesSession]) {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier)
        else { return }
        let rows = agents.map { agent in
            let palette = agent.appearance.palette
            return Agent(id: agent.id.uuidString, name: agent.name, main: hex(palette.main), deep: hex(palette.deep),
                         tintLight: hex(palette.tint, .light), tintDark: hex(palette.tint, .dark),
                         preview: latest[agent.id]?.lastMessagePreview.map(ChatText.preview),
                         updatedAt: latest[agent.id]?.updatedAt)
        }
        guard rows != lastWritten else { return }
        let looksChanged = rows.map { [$0.id, String($0.main)] } != lastWritten.map { [$0.id, String($0.main)] }
        lastWritten = rows
        if looksChanged { exportBips(agents, to: container.appending(path: "avatars", directoryHint: .isDirectory)) }
        guard let data = try? JSONEncoder().encode(rows) else { return }
        try? data.write(to: container.appending(path: "widgets.json"), options: .atomic)
        WidgetCenter.shared.reloadAllTimelines()
    }

    /// Each Bip alone, without the tinted square of the notification avatars: it sits on the widget's own colour.
    private static func exportBips(_ agents: [AgentProfile], to folder: URL) {
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        for agent in agents {
            let renderer = ImageRenderer(content: MascotView(appearance: agent.appearance, animated: false)
                .frame(width: 120, height: 120)
                .environment(\.colorScheme, .light))
            renderer.scale = 3
            guard let png = renderer.uiImage?.pngData() else { continue }
            try? png.write(to: folder.appending(path: "\(agent.id.uuidString)-bip.png"), options: .atomic)
        }
    }

    private static func hex(_ color: Color, _ style: UIUserInterfaceStyle = .light) -> UInt32 {
        let resolved = UIColor(color).resolvedColor(with: UITraitCollection(userInterfaceStyle: style))
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        resolved.getRed(&r, green: &g, blue: &b, alpha: &a)
        let byte = { (v: CGFloat) in UInt32(max(0, min(255, (v * 255).rounded()))) }
        return byte(r) << 16 | byte(g) << 8 | byte(b)
    }
}

/// `bipagents://agent/<id>[?start=voice|keyboard]`: a widget opens the agent's « Discussion » (its permanent
/// thread), recording a voice note already when asked.
enum AppLink {
    @MainActor
    static func open(_ url: URL, store: AgentStore, router: Router) {
        guard url.scheme == "bipagents", url.host() == "agent",
              let id = UUID(uuidString: url.pathComponents.dropFirst().first ?? ""),
              let agent = store.agents.first(where: { $0.id == id }) else { return }
        let start = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems?.first { $0.name == "start" }?.value
        router.open(.conversation(agent, sessionID: store.mainSessionID(for: agent),
                                  start: start == "voice" ? .voiceNote : start == "keyboard" ? .keyboard : .none))
    }
}
