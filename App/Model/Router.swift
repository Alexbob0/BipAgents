import Observation
import SwiftUI

enum AppTab: Hashable { case agents, inbox, settings }

/// App-wide navigation state, so notifications and deep links can open a screen.
@Observable
final class Router {
    /// `-tab inbox|settings` launch argument opens on that tab (screenshots, design review).
    var tab: AppTab = switch UserDefaults.standard.string(forKey: "tab") {
    case "inbox": .inbox
    case "settings": .settings
    default: .agents
    }
    var agentsPath = NavigationPath()

    func open(_ route: AgentRoute) {
        tab = .agents
        agentsPath = NavigationPath()
        agentsPath.append(route)
    }
}
