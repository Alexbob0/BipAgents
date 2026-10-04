import SwiftUI
import UIKit

/// Semantic colors and type styles of the "Compagnons" look: light by default, dark when the system asks.
enum Theme {
    static let ink = Color(light: 0x16181D, dark: 0xF2F3F7)
    static let ink2 = Color(light: 0x5E6472, dark: 0xA4A9B8)
    static let muted = Color(light: 0x6B7180, dark: 0x8A90A2)
    static let background = Color(light: 0xF3F4F7, dark: 0x0F1115)
    static let card = Color(light: 0xFFFFFF, dark: 0x1A1D24)
    static let field = Color(light: 0xF3F4F7, dark: 0x23262F)
    static let line = Color(light: 0x16181D, dark: 0xFFFFFF).opacity(0.08)
    static let danger = Color(hex: 0xE5484D)
    static let dangerTint = Color(light: 0xFDE7E8, dark: 0x3A1E21)
    static let online = Color(hex: 0x2FB36B)
    /// Text drawn on an `ink` fill.
    static let onInk = Color(light: 0xFFFFFF, dark: 0x16181D)

    static func display(_ size: CGFloat) -> Font { .system(size: size, weight: .black, design: .rounded) }
    static func title(_ size: CGFloat = 17) -> Font { .system(size: size, weight: .heavy, design: .rounded) }
    static func body(_ size: CGFloat = 16, weight: Font.Weight = .semibold) -> Font { .system(size: size, weight: weight, design: .rounded) }
    static let mono = Font.system(size: 12.5, design: .monospaced)
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(.sRGB,
                  red: Double((hex >> 16) & 0xFF) / 255,
                  green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255,
                  opacity: opacity)
    }

    init(light: UInt32, dark: UInt32) {
        self.init(uiColor: .dynamic(light: UIColor(rgb: light), dark: UIColor(rgb: dark)))
    }

    /// Parses `#RRGGBB` or `RRGGBB`.
    init?(hexString: String) {
        let digits = hexString.trimmingCharacters(in: CharacterSet(charactersIn: "# "))
        guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
        self.init(hex: value)
    }
}

extension UIColor {
    convenience init(rgb: UInt32) {
        self.init(red: CGFloat((rgb >> 16) & 0xFF) / 255, green: CGFloat((rgb >> 8) & 0xFF) / 255,
                  blue: CGFloat(rgb & 0xFF) / 255, alpha: 1)
    }

    /// Light/dark color. Built in a nonisolated context on purpose: SwiftUI resolves colors on its async
    /// render thread, and a provider closure formed in main-actor code would trap there (Swift 6 isolation check).
    nonisolated static func dynamic(light: UIColor, dark: UIColor) -> UIColor {
        UIColor { $0.userInterfaceStyle == .dark ? dark : light }
    }
}

struct CardBackground: ViewModifier {
    var radius: CGFloat = 22
    func body(content: Content) -> some View {
        content
            .background(Theme.card, in: .rect(cornerRadius: radius, style: .continuous))
            .shadow(color: .black.opacity(0.06), radius: 14, y: 8)
    }
}

extension View {
    func card(radius: CGFloat = 22) -> some View { modifier(CardBackground(radius: radius)) }
}

/// Large pill button: `.primary` is ink, `.secondary` is white/field.
struct PillButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, soft, danger }
    var kind: Kind = .primary
    var height: CGFloat = 50

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.body(16, weight: .heavy))
            .frame(maxWidth: .infinity, minHeight: height)
            .foregroundStyle(foreground)
            .background(background, in: .capsule)
            .opacity(configuration.isPressed ? 0.75 : 1)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }

    private var foreground: Color {
        switch kind {
        case .primary: Theme.onInk
        case .secondary, .soft: Theme.ink
        case .danger: Color(hex: 0xC22B31)
        }
    }

    private var background: Color {
        switch kind {
        case .primary: Theme.ink
        case .secondary: Theme.card
        case .soft: Theme.field
        case .danger: Theme.dangerTint
        }
    }
}

extension ButtonStyle where Self == PillButtonStyle {
    static var pill: PillButtonStyle { PillButtonStyle() }
    static func pill(_ kind: PillButtonStyle.Kind, height: CGFloat = 50) -> PillButtonStyle { PillButtonStyle(kind: kind, height: height) }
}
