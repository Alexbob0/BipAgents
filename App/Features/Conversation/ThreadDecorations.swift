import SwiftUI

/// « Aujourd’hui », « Hier », « lundi 5 octobre » between days of a conversation.
struct DaySeparator: View {
    var date: Date

    var body: some View {
        Text(label)
            .font(Theme.body(12.5, weight: .heavy))
            .foregroundStyle(Theme.muted)
            .padding(.horizontal, 12)
            .frame(height: 26)
            .background(Theme.card, in: .capsule)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
            .accessibilityAddTraits(.isHeader)
    }

    private var label: String {
        let calendar = Calendar.current
        if calendar.isDateInToday(date) { return "Aujourd’hui" }
        if calendar.isDateInYesterday(date) { return "Hier" }
        let sameYear = calendar.isDate(date, equalTo: .now, toGranularity: .year)
        let style = Date.FormatStyle.dateTime.weekday(.wide).day().month(.wide)
        return (sameYear ? date.formatted(style) : date.formatted(style.year())).capitalizedFirst
    }
}

/// Small « 07:53 » under a message.
struct TimeLabel: View {
    var date: Date?

    var body: some View {
        if let date {
            Text(date, format: .dateTime.hour().minute())
                .font(Theme.body(11.5, weight: .bold))
                .foregroundStyle(Theme.muted)
                .padding(.horizontal, 4)
        }
    }
}

/// A long prompt the user did not type (a scheduled task's brief, skill instructions), folded.
struct InstructionCard: View {
    var title: String
    var text: String
    var palette: AgentPalette
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button { withAnimation(.snappy) { expanded.toggle() } } label: {
                HStack(spacing: 10) {
                    Image(systemName: "doc.text")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(palette.deep)
                        .frame(width: 30, height: 30)
                        .background(palette.tint, in: .rect(cornerRadius: 10))
                    VStack(alignment: .leading, spacing: 1) {
                        Text(title).font(Theme.body(14, weight: .heavy)).foregroundStyle(Theme.ink).lineLimit(1)
                        Text(expanded ? "Toucher pour replier" : "Toucher pour afficher")
                            .font(Theme.body(12)).foregroundStyle(Theme.muted)
                    }
                    Spacer(minLength: 4)
                    Image(systemName: expanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(Theme.muted)
                }
                .contentShape(.rect)
            }
            .buttonStyle(.plain)
            if expanded {
                ScrollView {
                    Text(text)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(Theme.ink2)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 320)
            }
        }
        .padding(12)
        .background(Theme.card, in: .rect(cornerRadius: 20, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).stroke(Theme.line))
        .frame(maxWidth: 320, alignment: .trailing)
    }
}
