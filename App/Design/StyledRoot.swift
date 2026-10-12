import SwiftUI

/// Applies the chosen style and light or dark mode (Réglages › Apparence) to everything inside: `Theme`'s values,
/// the font design, the tint and the color scheme. Views read `Theme` when drawn, so a change of style (or, in the Hermes style, of light and dark, whose type
/// differs) rebuilds them.
struct StyledRoot<Content: View>: View {
    @AppStorage(AppStyle.key) private var style: AppStyle = .bips
    @AppStorage(ColorMode.key) private var mode: ColorMode = .system
    @Environment(\.colorScheme) private var colorScheme
    @ViewBuilder var content: Content

    var body: some View {
        let _ = Theme.configure(style: style, dark: (mode.scheme ?? colorScheme) == .dark)
        content
            .tint(Theme.hermes ? Theme.primary : Theme.ink)
            .fontDesign(Theme.fontDesign)
            .id(Theme.hermes ? (Theme.hermesDark ? "hermes-dark" : "hermes-light") : "bips")
            .preferredColorScheme(mode.scheme)
    }
}
