import SwiftUI

/// The header of every tab root (Agents, Boîte, Réglages): a context line, the big rounded title and an
/// optional trailing button — same position and type on every screen, pinned above the content.
struct ScreenHeader<Trailing: View>: View {
    var overline: String
    var title: String
    @ViewBuilder var trailing: Trailing

    var body: some View {
        HStack(alignment: .lastTextBaseline) {
            VStack(alignment: .leading, spacing: 0) {
                Text(overline)
                    .font(Theme.body(15, weight: .bold))
                    .foregroundStyle(Theme.ink2)
                    .lineLimit(1)
                Text(title)
                    .font(Theme.display(34))
                    .foregroundStyle(Theme.ink)
                    .accessibilityAddTraits(.isHeader)
            }
            Spacer(minLength: 12)
            trailing
        }
        .padding(.horizontal, 20)
        .padding(.top, 12)
        .padding(.bottom, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.background)
    }
}

extension ScreenHeader where Trailing == EmptyView {
    init(overline: String, title: String) {
        self.init(overline: overline, title: title) { EmptyView() }
    }
}

/// Round 46 pt header button (e.g. « + » on Agents).
struct HeaderButton: View {
    var systemImage: String
    var label: String
    var action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.system(size: 19, weight: .bold))
                .foregroundStyle(Theme.ink)
                .frame(width: 46, height: 46)
                .card(radius: 23)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

extension String {
    /// « dimanche 4 octobre » → « Dimanche 4 octobre ».
    var capitalizedFirst: String { prefix(1).uppercased() + dropFirst() }
}
