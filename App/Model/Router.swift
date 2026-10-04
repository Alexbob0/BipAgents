import Observation
import SwiftUI

enum AppTab: Hashable { case agents, inbox, settings }

/// App-wide navigation state, so notifications and deep links can open a screen.
@Observable
final class Router {
    var tab: AppTab = .agents
    var agentsPath = NavigationPath()

    func open(_ route: AgentRoute) {
        tab = .agents
        agentsPath = NavigationPath()
        agentsPath.append(route)
    }
}
