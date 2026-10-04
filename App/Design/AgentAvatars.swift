import SwiftUI

/// Each agent's Bip as a PNG in the App Group, for the notification extension: pushes are shown as
/// messages sent by the agent, with its Bip as the sender's picture (`NotificationService.asMessage`).
@MainActor
enum AgentAvatars {
    static var folder: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: AppGroup.identifier)?
            .appending(path: "avatars", directoryHint: .isDirectory)
    }

    static func export(_ agents: [AgentProfile]) {
        guard let folder else { return }
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        for agent in agents {
            let avatar = MascotView(appearance: agent.appearance, animated: false)
                .padding(20)
                .frame(width: 160, height: 160)
                .background(agent.appearance.palette.tint)
                .environment(\.colorScheme, .light)
            let renderer = ImageRenderer(content: avatar)
            renderer.scale = 3
            guard let png = renderer.uiImage?.pngData() else { continue }
            try? png.write(to: folder.appending(path: "\(agent.id.uuidString).png"), options: .atomic)
        }
    }
}
