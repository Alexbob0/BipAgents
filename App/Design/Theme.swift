import SwiftUI
import UIKit

/// The app's look, chosen in Réglages › Apparence: the Bips (round, playful mascots) or Hermes (closer to Hermes
/// Agent's own design: flat, Nous blue when light, gold on black when dark, pixel characters).
enum AppStyle: String, CaseIterable, Sendable {
    case bips, hermes

    static let key = "appStyle"

    static var saved: AppStyle { AppStyle(rawValue: UserDefaults.standard.string(forKey: key) ?? "") ?? .bips }
}

/// Light or dark: as the iPhone is set (`.system`), or always one of them. Réglages › Apparence.
enum ColorMode: String, CaseIterable, Sendable {
    case system, light, dark

    static let key = "colorMode"

    var scheme: ColorScheme? {
        switch self {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}

/// Semantic colors and type styles. Bips: light by default, dark when the system asks. Hermes: its light and dark
/// looks follow the system too, and its type differs between them (set by `StyledRoot`, which rebuilds the views).
enum Theme {
    static var style: AppStyle = .saved
    /// Hermes in dark mode: gold on black, monospaced type.
    static var hermesDark = false

    static var hermes: Bool { style == .hermes }

    static func configure(style: AppStyle, dark: Bool) {
        self.style = style
        hermesDark = style == .hermes && dark
    }

    static var ink: Color { hermes ? Hermes.ink : Bips.ink }
    static var ink2: Color { hermes ? Hermes.ink2 : Bips.ink2 }
    static var muted: Color { hermes ? Hermes.muted : Bips.muted }
    static var background: Color { hermes ? Hermes.background : Bips.background }
    static var card: Color { hermes ? Hermes.card : Bips.card }
    static var field: Color { hermes ? Hermes.field : Bips.field }
    static var line: Color { hermes ? Hermes.line : Bips.line }
    static let danger = Color(hex: 0xE5484D)
    static let dangerTint = Color(light: 0xFDE7E8, dark: 0x3A1E21)
    static let online = Color(hex: 0x2FB36B)
    /// Text drawn on an `ink` fill.
    static var onInk: Color { hermes ? Hermes.onInk : Bips.onInk }
    /// The main action's fill: ink for the Bips, Nous blue (light) or gold (dark) for Hermes.
    static var primary: Color { hermes ? Hermes.primary : Bips.ink }
    static var onPrimary: Color { hermes ? Hermes.onPrimary : Bips.onInk }
    /// Corner radius of buttons and fields: capsules for the Bips, squarer for Hermes.
    static var controlRadius: CGFloat { hermes ? (hermesDark ? 6 : 10) : 100 }
    static var fontDesign: Font.Design { hermes ? (hermesDark ? .monospaced : .default) : .rounded }

    static func display(_ size: CGFloat) -> Font {
        hermes ? .system(size: hermesDark ? size * 0.88 : size, weight: .bold, design: fontDesign) : .system(size: size, weight: .black, design: .rounded)
    }
    static func title(_ size: CGFloat = 17) -> Font {
        hermes ? .system(size: hermesDark ? size * 0.92 : size, weight: .semibold, design: fontDesign) : .system(size: size, weight: .heavy, design: .rounded)
    }
    static func body(_ size: CGFloat = 16, weight: Font.Weight = .semibold) -> Font {
        guard hermes else { return .system(size: size, weight: weight, design: .rounded) }
        // Hermes is quieter: the Bips' heavy weights step down one notch.
        let calmer: Font.Weight = switch weight {
        case .black, .heavy: .semibold
        case .bold, .semibold: .medium
        default: .regular
        }
        return .system(size: hermesDark ? size * 0.9 : size, weight: calmer, design: fontDesign)
    }
    static let mono = Font.system(size: 12.5, design: .monospaced)

    // Each color made once per style: views compare them (a play button picks its icon's color by whether its fill
    // is the text's), and a dynamic color made anew is never equal to another.
    private enum Bips {
        static let ink = Color(light: 0x16181D, dark: 0xF2F3F7)
        static let ink2 = Color(light: 0x5E6472, dark: 0xA4A9B8)
        static let muted = Color(light: 0x6B7180, dark: 0x8A90A2)
        static let background = Color(light: 0xF3F4F7, dark: 0x0F1115)
        static let card = Color(light: 0xFFFFFF, dark: 0x1A1D24)
        static let field = Color(light: 0xF3F4F7, dark: 0x23262F)
        static let onInk = Color(light: 0xFFFFFF, dark: 0x16181D)
        static let line = Color(light: 0x16181D, dark: 0xFFFFFF).opacity(0.08)
    }

    private enum Hermes {
        static let ink = Color(light: 0x17171A, dark: 0xFFF8DC)
        static let ink2 = Color(light: 0x5B5E69, dark: 0xB8A57A)
        static let muted = Color(light: 0x6B6E78, dark: 0x9A8A5A)
        static let background = Color(light: 0xF8FAFF, dark: 0x0F1012)
        static let card = Color(light: 0xFFFFFF, dark: 0x16171B)
        static let field = Color(light: 0xEEF2FB, dark: 0x201E19)
        static let onInk = Color(light: 0xFFFFFF, dark: 0x0F1012)
        static let line = Color(uiColor: .dynamic(light: UIColor(rgb: 0x17171A).withAlphaComponent(0.09), dark: UIColor(rgb: 0xCD7F32).withAlphaComponent(0.3)))
        static let primary = Color(light: 0x0053FD, dark: 0xFFC21A)
        static let onPrimary = Color(light: 0xFFFFFF, dark: 0x1A1205)
    }
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
    @ViewBuilder
    func body(content: Content) -> some View {
        if Theme.hermes {
            // Flat: a hairline instead of a shadow, squarer corners.
            let shape = RoundedRectangle(cornerRadius: Theme.hermesDark ? 8 : min(radius, 16), style: .continuous)
            content
                .background(Theme.card, in: shape)
                .overlay(shape.stroke(Theme.line, lineWidth: 1))
        } else {
            content
                .background(Theme.card, in: .rect(cornerRadius: radius, style: .continuous))
                .shadow(color: .black.opacity(0.06), radius: 14, y: 8)
        }
    }
}

extension View {
    func card(radius: CGFloat = 22) -> some View { modifier(CardBackground(radius: radius)) }
}

/// Large pill button (squarer in the Hermes style): `.primary` is the main action's fill, `.secondary` is white/field.
struct PillButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, soft, danger }
    var kind: Kind = .primary
    var height: CGFloat = 50

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.body(16, weight: .heavy))
            .frame(maxWidth: .infinity, minHeight: height)
            .foregroundStyle(foreground)
            .background(background, in: .rect(cornerRadius: min(Theme.controlRadius, height / 2), style: .continuous))
            .overlay {
                // Hermes: a secondary button on a card of the same color still shows its edge.
                if Theme.hermes, kind == .secondary {
                    RoundedRectangle(cornerRadius: min(Theme.controlRadius, height / 2), style: .continuous).stroke(Theme.line, lineWidth: 1)
                }
            }
            .opacity(configuration.isPressed ? 0.75 : 1)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.15), value: configuration.isPressed)
    }

    private var foreground: Color {
        switch kind {
        case .primary: Theme.onPrimary
        case .secondary, .soft: Theme.ink
        case .danger: Color(hex: 0xC22B31)
        }
    }

    private var background: Color {
        switch kind {
        case .primary: Theme.primary
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

/// Icon + title tight enough for three pills side by side on an iPhone.
struct CompactPillLabelStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(spacing: 6) {
            configuration.icon
            configuration.title.lineLimit(1).minimumScaleFactor(0.8)
        }
    }
}

extension LabelStyle where Self == CompactPillLabelStyle {
    static var compactPill: CompactPillLabelStyle { CompactPillLabelStyle() }
}

