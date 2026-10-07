import SwiftUI
import UIKit

extension View {
    /// Shows or hides the tab bar when this page comes on screen, in step with the navigation animation: going
    /// back from a conversation, the bar slides in with the page (edge swipe included, and hidden again if the
    /// swipe is abandoned). `.toolbar(.hidden, for: .tabBar)` only brings it back once the animation has finished.
    /// Each page of a stack says what it wants: the pages that hide it, and the ones they go back to.
    func tabBarHidden(_ hidden: Bool) -> some View {
        background(TabBarVisibility(hidden: hidden))
    }
}

private struct TabBarVisibility: UIViewControllerRepresentable {
    var hidden: Bool

    func makeUIViewController(context: Context) -> Probe { Probe(hidden: hidden) }
    func updateUIViewController(_ controller: Probe, context: Context) {}

    final class Probe: UIViewController {
        let hidden: Bool

        init(hidden: Bool) {
            self.hidden = hidden
            super.init(nibName: nil, bundle: nil)
        }

        required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }

        /// The page: the ancestor the navigation controller holds.
        private var page: UIViewController {
            var page: UIViewController = self
            while let parent = page.parent, !(parent is UINavigationController) { page = parent }
            return page
        }

        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            let hidden = hidden, page = page
            guard let tabs = page.tabBarController, tabs.isTabBarHidden != hidden else { return }
            guard let coordinator = page.transitionCoordinator else {
                tabs.setTabBarHidden(hidden, animated: animated)
                return
            }
            coordinator.animate(alongsideTransition: { _ in
                tabs.setTabBarHidden(hidden, animated: false)
            }, completion: { [weak page] _ in
                // Swipe abandoned: both pages were told to appear; the one left on top decides.
                guard let page, page.navigationController?.topViewController === page else { return }
                tabs.setTabBarHidden(hidden, animated: false)
            })
        }
    }
}
