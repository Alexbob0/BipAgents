import HermesKit
import SwiftUI

/// When a routine runs, in words people pick (« Chaque lundi à 8:00 »), turned into what Hermes stores: a cron
/// expression, an interval or a date. Anything else Hermes accepts stays as typed (`.custom`).
enum RoutineSchedule: Hashable {
    case daily(hour: Int, minute: Int)
    case weekdays(hour: Int, minute: Int)
    case weekly(weekday: Int, hour: Int, minute: Int)      // 1 = lundi … 7 = dimanche
    case monthly(day: Int, hour: Int, minute: Int)
    case everyHours(Int)
    case once(Date)
    case custom(String)

    enum Kind: String, CaseIterable, Identifiable {
        case daily, weekdays, weekly, monthly, everyHours, once, custom
        var id: String { rawValue }
        var label: LocalizedStringKey {
            switch self {
            case .daily: "Chaque jour"
            case .weekdays: "En semaine"
            case .weekly: "Chaque semaine"
            case .monthly: "Chaque mois"
            case .everyHours: "Toutes les X heures"
            case .once: "Une seule fois"
            case .custom: "Avancé"
            }
        }
    }

    var kind: Kind {
        switch self {
        case .daily: .daily
        case .weekdays: .weekdays
        case .weekly: .weekly
        case .monthly: .monthly
        case .everyHours: .everyHours
        case .once: .once
        case .custom: .custom
        }
    }

    /// What Hermes stores.
    var hermes: String {
        switch self {
        case .daily(let h, let m): "\(m) \(h) * * *"
        case .weekdays(let h, let m): "\(m) \(h) * * 1-5"
        case .weekly(let d, let h, let m): "\(m) \(h) * * \(d % 7)"
        case .monthly(let day, let h, let m): "\(m) \(h) \(day) * *"
        case .everyHours(let n): "every \(n)h"
        case .once(let date):
            // With the phone's offset (« 2026-10-12T08:00:00+02:00 »): the same instant whatever Hermes' timezone.
            date.formatted(Date.ISO8601FormatStyle(timeZoneSeparator: .colon, timeZone: .current))
        case .custom(let text): text
        }
    }

    /// Reads what Hermes stored back into words (cron, `every Nm/h`, a date); anything else is `.custom`.
    init(hermes text: String) {
        let parts = text.split(separator: " ").map(String.init)
        if parts.count == 5, let m = Int(parts[0]), let h = Int(parts[1]), parts[3] == "*" {
            switch (parts[2], parts[4]) {
            case ("*", "*"): self = .daily(hour: h, minute: m); return
            case ("*", "1-5"), ("*", "MON-FRI"): self = .weekdays(hour: h, minute: m); return
            case ("*", let d): if let day = Int(d), (0...7).contains(day) {
                self = .weekly(weekday: day == 0 ? 7 : day, hour: h, minute: m); return
            }
            case (let d, "*"): if let day = Int(d), (1...31).contains(day) {
                self = .monthly(day: day, hour: h, minute: m); return
            }
            default: break
            }
        }
        if let match = text.firstMatch(of: /^every (\d+)([mh])$/), let n = Int(match.output.1) {
            let hours = match.output.2 == "h" ? n : n / 60
            if hours >= 1, match.output.2 == "h" || n % 60 == 0 { self = .everyHours(hours); return }
        }
        if let date = try? Date(text, strategy: .iso8601) ?? Date(text + "Z", strategy: .iso8601) {
            self = .once(date)
            return
        }
        self = .custom(text)
    }

    var label: String {
        func time(_ h: Int, _ m: Int) -> String { String(format: "%d:%02d", h, m) }
        switch self {
        case .daily(let h, let m): return String(localized: "Chaque jour à \(time(h, m))")
        case .weekdays(let h, let m): return String(localized: "En semaine à \(time(h, m))")
        case .weekly(let d, let h, let m):
            let day = Calendar.current.standaloneWeekdaySymbols[d % 7]
            return String(localized: "Chaque \(day) à \(time(h, m))")
        case .monthly(let day, let h, let m): return String(localized: "Le \(day) de chaque mois à \(time(h, m))")
        case .everyHours(let n): return String(localized: "Toutes les \(n) h")
        case .once(let date): return date.formatted(date: .abbreviated, time: .shortened)
        case .custom(let text): return text
        }
    }
}

/// An agent's routines: what it does on its own, and when. Its reports land in its Discussion and the Boîte.
struct RoutinesView: View {
    let agent: AgentProfile

    @Environment(AgentStore.self) private var store
    @State private var routines: [Routine] = []
    @State private var isLoading = true
    @State private var errorMessage: String?
    @State private var editing: Routine?
    @State private var isAdding = false

    private var palette: AgentPalette { agent.appearance.palette }

    var body: some View {
        List {
            if let errorMessage {
                Section { NoticeRow(text: errorMessage, isError: true) }.listRowBackground(Color.clear)
            }
            Section {
                ForEach(routines) { routine in
                    Button { editing = routine } label: { row(routine) }
                        .buttonStyle(.plain)
                        .swipeActions {
                            Button("Supprimer", systemImage: "trash", role: .destructive) { delete(routine) }
                            Button(routine.isPaused ? "Reprendre" : "Pause",
                                   systemImage: routine.isPaused ? "play.fill" : "pause.fill") { toggle(routine) }
                                .tint(palette.deep)
                        }
                        .swipeActions(edge: .leading) {
                            Button("Lancer", systemImage: "bolt.fill") { run(routine) }.tint(.orange)
                        }
                }
            } footer: {
                if !isLoading && routines.isEmpty {
                    Text("\(agent.name) n’a pas encore de routine. Par exemple : « Chaque lundi à 8:00, fais le point sur ma semaine ».")
                }
            }
        }
        .navigationTitle("Routines")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Nouvelle routine", systemImage: "plus") { isAdding = true }
            }
        }
        .overlay { if isLoading && routines.isEmpty { ProgressView() } }
        .refreshable { await load() }
        .task { await load() }
        .sheet(isPresented: $isAdding) {
            NavigationStack { RoutineEditor(agent: agent, routine: nil) { await load() } }
        }
        .sheet(item: $editing) { routine in
            NavigationStack { RoutineEditor(agent: agent, routine: routine) { await load() } }
        }
    }

    private func row(_ routine: Routine) -> some View {
        HStack(spacing: 12) {
            Image(systemName: routine.isPaused ? "pause.circle.fill" : "clock.fill")
                .font(.system(size: 22))
                .foregroundStyle(routine.isPaused ? Theme.muted : palette.deep)
            VStack(alignment: .leading, spacing: 2) {
                Text(routine.name).font(Theme.body(16, weight: .bold)).foregroundStyle(Theme.ink)
                Text(RoutineSchedule(hermes: routine.schedule).label)
                    .font(Theme.body(13.5)).foregroundStyle(Theme.ink2)
                if routine.isPaused {
                    Text("En pause").font(Theme.body(12.5, weight: .heavy)).foregroundStyle(Theme.muted)
                } else if let next = routine.nextRunAt {
                    Text("Prochaine fois \(next.formatted(.relative(presentation: .named)))")
                        .font(Theme.body(12.5)).foregroundStyle(Theme.muted)
                }
            }
            Spacer()
            if routine.lastStatus == "error" {
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Theme.danger)
                    .accessibilityLabel("Dernière exécution en erreur")
            }
        }
        .contentShape(.rect)
    }

    private func load() async {
        guard let client = store.client(for: agent) else { return }
        defer { isLoading = false }
        do {
            routines = try await client.routines().sorted { ($0.isPaused ? 1 : 0, $0.name) < ($1.isPaused ? 1 : 0, $1.name) }
            errorMessage = nil
        } catch {
            errorMessage = ConversationModel.describe(error)
        }
    }

    private func toggle(_ routine: Routine) {
        act { try await $0.setRoutine(id: routine.id, paused: !routine.isPaused) }
    }

    private func run(_ routine: Routine) {
        act { try await $0.runRoutine(id: routine.id) }
    }

    private func delete(_ routine: Routine) {
        routines.removeAll { $0.id == routine.id }
        act { try await $0.deleteRoutine(id: routine.id) }
    }

    private func act(_ action: @escaping (HermesClient) async throws -> Void) {
        guard let client = store.client(for: agent) else { return }
        Task {
            do { try await action(client) } catch { errorMessage = ConversationModel.describe(error) }
            await load()
        }
    }
}

/// A new routine or changes to one: a name, what to do, and when (in words).
struct RoutineEditor: View {
    let agent: AgentProfile
    let routine: Routine?
    var onSaved: () async -> Void

    @Environment(AgentStore.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var prompt = ""
    @State private var kind = RoutineSchedule.Kind.daily
    @State private var time = Calendar.current.date(bySettingHour: 8, minute: 0, second: 0, of: .now) ?? .now
    @State private var weekday = 1
    @State private var monthDay = 1
    @State private var hours = 2
    @State private var date = Date.now.addingTimeInterval(3600)
    @State private var custom = ""
    @State private var isSaving = false
    @State private var errorMessage: String?

    private var schedule: RoutineSchedule {
        let parts = Calendar.current.dateComponents([.hour, .minute], from: time)
        let h = parts.hour ?? 8, m = parts.minute ?? 0
        return switch kind {
        case .daily: .daily(hour: h, minute: m)
        case .weekdays: .weekdays(hour: h, minute: m)
        case .weekly: .weekly(weekday: weekday, hour: h, minute: m)
        case .monthly: .monthly(day: monthDay, hour: h, minute: m)
        case .everyHours: .everyHours(hours)
        case .once: .once(date)
        case .custom: .custom(custom.trimmingCharacters(in: .whitespaces))
        }
    }

    var body: some View {
        Form {
            Section("Nom") {
                TextField("Ex. Point du lundi", text: $name)
            }
            Section {
                TextField("Ex. Fais le point sur ma semaine : agenda, courses, rappels", text: $prompt, axis: .vertical)
                    .lineLimit(3...8)
            } header: {
                Text("Ce que \(agent.name) doit faire")
            } footer: {
                Text("Son compte rendu arrive dans sa Discussion et dans la Boîte, avec une notification.")
            }
            Section("Quand") {
                Picker("Fréquence", selection: $kind) {
                    ForEach(RoutineSchedule.Kind.allCases) { Text($0.label).tag($0) }
                }
                switch kind {
                case .daily, .weekdays:
                    DatePicker("Heure", selection: $time, displayedComponents: .hourAndMinute)
                case .weekly:
                    Picker("Jour", selection: $weekday) {
                        ForEach(1...7, id: \.self) { Text(Calendar.current.standaloneWeekdaySymbols[$0 % 7].capitalized).tag($0) }
                    }
                    DatePicker("Heure", selection: $time, displayedComponents: .hourAndMinute)
                case .monthly:
                    Stepper("Le \(monthDay) du mois", value: $monthDay, in: 1...28)
                    DatePicker("Heure", selection: $time, displayedComponents: .hourAndMinute)
                case .everyHours:
                    Stepper("Toutes les \(hours) h", value: $hours, in: 1...24)
                case .once:
                    DatePicker("Le", selection: $date, in: Date.now..., displayedComponents: [.date, .hourAndMinute])
                case .custom:
                    TextField("Ex. 0 8 * * 1-5 ou every 30m", text: $custom)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                }
            }
            if let errorMessage {
                Section { Text(errorMessage).foregroundStyle(Theme.danger) }
            }
        }
        .navigationTitle(routine == nil ? String(localized: "Nouvelle routine") : String(localized: "Routine"))
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) { Button("Annuler") { dismiss() } }
            ToolbarItem(placement: .confirmationAction) {
                Button("Enregistrer", action: save).fontWeight(.heavy)
                    .disabled(isSaving || name.trimmingCharacters(in: .whitespaces).isEmpty
                              || prompt.trimmingCharacters(in: .whitespaces).isEmpty
                              || schedule.hermes.isEmpty)
            }
        }
        .onAppear(perform: fill)
    }

    private func fill() {
        guard let routine else { return }
        name = routine.name
        prompt = routine.prompt
        let parsed = RoutineSchedule(hermes: routine.schedule)
        kind = parsed.kind
        let calendar = Calendar.current
        func at(_ h: Int, _ m: Int) { time = calendar.date(bySettingHour: h, minute: m, second: 0, of: .now) ?? time }
        switch parsed {
        case .daily(let h, let m), .weekdays(let h, let m): at(h, m)
        case .weekly(let d, let h, let m): weekday = d; at(h, m)
        case .monthly(let day, let h, let m): monthDay = min(day, 28); at(h, m)
        case .everyHours(let n): hours = n
        case .once(let when): date = when
        case .custom(let text): custom = text
        }
    }

    private func save() {
        guard let client = store.client(for: agent) else { return }
        isSaving = true
        errorMessage = nil
        let name = name.trimmingCharacters(in: .whitespaces), prompt = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let schedule = schedule.hermes
        Task {
            defer { isSaving = false }
            do {
                if let routine {
                    _ = try await client.updateRoutine(id: routine.id, name: name, schedule: schedule, prompt: prompt)
                } else {
                    _ = try await client.createRoutine(name: name, schedule: schedule, prompt: prompt)
                }
                await onSaved()
                dismiss()
            } catch {
                errorMessage = ConversationModel.describe(error)
            }
        }
    }
}
