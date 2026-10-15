import UIKit

/// The orientations the app allows right now: portrait, and landscape too while a screen asks for it (the agent's
/// desktop, wider than the phone).
@MainActor
enum OrientationLock {
    private(set) static var allowed: UIInterfaceOrientationMask = .portrait

    static func allowLandscape(_ on: Bool) {
        allowed = on ? .allButUpsideDown : .portrait
        for case let scene as UIWindowScene in UIApplication.shared.connectedScenes {
            scene.keyWindow?.rootViewController?.setNeedsUpdateOfSupportedInterfaceOrientations()
            if !on { scene.requestGeometryUpdate(.iOS(interfaceOrientations: .portrait)) }
        }
    }
}
