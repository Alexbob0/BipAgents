import AppIntents
import SwiftUI
import WidgetKit

// One widget per agent, chosen when editing the widget: its Bip, its name and its last message. Tapping it opens
// the agent's « Discussion »; the microphone opens it already recording a voice note. (iOS keeps the long press for
// its own menu: a widget cannot record while held.) Data comes from the app (`WidgetExport`), through the App Group.

@main
struct BipWidgets: WidgetBundle {
    var body: some Widget {
        AgentWidget()
    }
}

// MARK: - What the app wrote

struct WidgetAgent: Codable, Hashable {
    var id: String
    var name: String
    var main: UInt32
    var deep: UInt32
    var tintLight: UInt32
    var tintDark: UInt32
    var preview: String?
    var updatedAt: Date?
}

enum SharedData {
    static var container: URL? {
        let group = Bundle.main.object(forInfoDictionaryKey: "BIPAppGroup") as? String ?? "group.io.github.bipagents"
        return FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
    }

    static func agents() -> [WidgetAgent] {
        guard let url = container?.appending(path: "widgets.json"), let data = try? Data(contentsOf: url) else { return [] }
        return (try? JSONDecoder().decode([WidgetAgent].self, from: data)) ?? []
    }

    static func bip(_ id: String) -> UIImage? {
        guard let url = container?.appending(path: "avatars/\(id)-bip.png") else { return nil }
        return UIImage(contentsOfFile: url.path(percentEncoded: false))
    }

    static func link(_ id: String, start: String? = nil) -> URL {
        URL(string: "bipagents://agent/\(id)" + (start.map { "?start=\($0)" } ?? ""))!
    }
}

extension Color {
    init(hex: UInt32) {
        self.init(red: Double((hex >> 16) & 0xFF) / 255, green: Double((hex >> 8) & 0xFF) / 255,
                  blue: Double(hex & 0xFF) / 255)
    }
}

// MARK: - Choosing the agent

struct AgentEntity: AppEntity {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Agent"
    static let defaultQuery = AgentQuery()

    var id: String
    var name: String

    var displayRepresentation: DisplayRepresentation { DisplayRepresentation(title: "\(name)") }
}

struct AgentQuery: EntityQuery {
    func entities(for identifiers: [String]) async throws -> [AgentEntity] {
        SharedData.agents().filter { identifiers.contains($0.id) }.map { AgentEntity(id: $0.id, name: $0.name) }
    }

    func suggestedEntities() async throws -> [AgentEntity] {
        SharedData.agents().map { AgentEntity(id: $0.id, name: $0.name) }
    }

    func defaultResult() async -> AgentEntity? {
        SharedData.agents().first.map { AgentEntity(id: $0.id, name: $0.name) }
    }
}

struct SelectAgentIntent: WidgetConfigurationIntent {
    static let title: LocalizedStringResource = "Choisir l’agent"
    static let description = IntentDescription("L’agent que ce widget montre.")

    @Parameter(title: "Agent")
    var agent: AgentEntity?
}

/// The microphone: opens the app on the agent's « Discussion », a voice note already recording.
struct RecordVoiceNoteIntent: AppIntent {
    static let title: LocalizedStringResource = "Message vocal"
    static let isDiscoverable = false

    @Parameter(title: "Agent")
    var agentID: String

    init() {}

    init(agentID: String) {
        self.agentID = agentID
    }

    func perform() async throws -> some IntentResult & OpensIntent {
        .result(opensIntent: OpenURLIntent(SharedData.link(agentID, start: "voice")))
    }
}

// MARK: - Timeline

struct AgentEntry: TimelineEntry {
    var date: Date
    var agent: WidgetAgent?
}

struct AgentProvider: AppIntentTimelineProvider {
    func placeholder(in context: Context) -> AgentEntry {
        AgentEntry(date: .now, agent: WidgetAgent(id: "", name: "Bip", main: 0xFF7B5C, deep: 0xC94A2E, tintLight: 0xFFF1EC,
                                                 tintDark: 0x3A2A26, preview: nil, updatedAt: nil))
    }

    func snapshot(for configuration: SelectAgentIntent, in context: Context) async -> AgentEntry {
        entry(for: configuration)
    }

    func timeline(for configuration: SelectAgentIntent, in context: Context) async -> Timeline<AgentEntry> {
        // The app reloads the widgets when something changes; the hourly refresh only keeps « 2 h » ages honest.
        Timeline(entries: [entry(for: configuration)], policy: .after(.now.addingTimeInterval(3600)))
    }

    private func entry(for configuration: SelectAgentIntent) -> AgentEntry {
        let agents = SharedData.agents()
        let chosen = configuration.agent.flatMap { wanted in agents.first { $0.id == wanted.id } } ?? agents.first
        return AgentEntry(date: .now, agent: chosen)
    }
}

// MARK: - Views

struct AgentWidget: Widget {
    var body: some WidgetConfiguration {
        AppIntentConfiguration(kind: "AgentWidget", intent: SelectAgentIntent.self, provider: AgentProvider()) { entry in
            AgentWidgetView(entry: entry)
        }
        .configurationDisplayName("Un agent")
        .description("Son dernier message. Touche pour lui écrire, le micro pour lui parler.")
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryCircular, .accessoryRectangular])
    }
}

struct AgentWidgetView: View {
    var entry: AgentEntry
    @Environment(\.widgetFamily) private var family
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        if let agent = entry.agent {
            content(agent)
                .widgetURL(SharedData.link(agent.id))
                .containerBackground(for: .widget) {
                    Color(hex: colorScheme == .dark ? agent.tintDark : agent.tintLight)
                }
        } else {
            Text("Ajoute un agent dans l’app BipAgents.")
                .font(.footnote)
                .containerBackground(for: .widget) { Color(.systemBackground) }
        }
    }

    @ViewBuilder
    private func content(_ agent: WidgetAgent) -> some View {
        switch family {
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                bip(agent).padding(6)
            }
        case .accessoryRectangular:
            VStack(alignment: .leading, spacing: 1) {
                Text(agent.name).font(.headline).widgetAccentable()
                Text(agent.preview ?? String(localized: "Touche pour lui écrire")).font(.caption).lineLimit(2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        case .systemMedium:
            HStack(alignment: .top, spacing: 14) {
                VStack(alignment: .leading, spacing: 6) {
                    title(agent)
                    preview(agent, lines: 3)
                    Spacer(minLength: 0)
                    HStack(spacing: 8) {
                        microphone(agent, label: true)
                        Link(destination: SharedData.link(agent.id, start: "keyboard")) {
                            Label("Écrire", systemImage: "keyboard")
                                .font(.system(size: 14, weight: .heavy, design: .rounded))
                                .padding(.horizontal, 12).frame(height: 34)
                                .background(.white.opacity(colorScheme == .dark ? 0.12 : 0.85), in: .capsule)
                        }
                    }
                }
                bip(agent).frame(width: 74, height: 74)
            }
        default:
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .top) {
                    bip(agent).frame(width: 46, height: 46)
                    Spacer()
                    microphone(agent, label: false)
                }
                Spacer(minLength: 0)
                title(agent)
                preview(agent, lines: 2)
            }
        }
    }

    private func bip(_ agent: WidgetAgent) -> some View {
        Group {
            if let image = SharedData.bip(agent.id) {
                Image(uiImage: image).resizable().scaledToFit()
            } else {
                Circle().fill(Color(hex: agent.main))
            }
        }
    }

    private func title(_ agent: WidgetAgent) -> some View {
        Text(agent.name)
            .font(.system(size: 17, weight: .heavy, design: .rounded))
            .foregroundStyle(Color(hex: colorScheme == .dark ? agent.main : agent.deep))
            .lineLimit(1)
    }

    @ViewBuilder
    private func preview(_ agent: WidgetAgent, lines: Int) -> some View {
        if let text = agent.preview, !text.isEmpty {
            Text(text).font(.system(size: 13, design: .rounded)).foregroundStyle(.primary).lineLimit(lines)
        } else {
            Text("Touche pour lui écrire").font(.system(size: 13, design: .rounded)).foregroundStyle(.secondary)
        }
    }

    private func microphone(_ agent: WidgetAgent, label: Bool) -> some View {
        Button(intent: RecordVoiceNoteIntent(agentID: agent.id)) {
            Group {
                if label {
                    Label("Audio", systemImage: "mic.fill").font(.system(size: 14, weight: .heavy, design: .rounded))
                        .padding(.horizontal, 12).frame(height: 34)
                } else {
                    Image(systemName: "mic.fill").font(.system(size: 16, weight: .bold)).frame(width: 38, height: 38)
                }
            }
            .foregroundStyle(.white)
            .background(Color(hex: agent.deep), in: .capsule)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text("Message vocal à \(agent.name)"))
    }
}
