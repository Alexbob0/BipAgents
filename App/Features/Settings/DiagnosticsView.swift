import HermesKit
import SwiftUI

/// Reachability and latency of every agent and bridge, plus what each Hermes server advertises.
struct DiagnosticsView: View {
    @Environment(AgentStore.self) private var store
    @State private var results: [UUID: AgentDiagnostics] = [:]
    @State private var isRunning = false

    struct AgentDiagnostics {
        var hermes: Result<Duration, any Error>?
        var bridge: Result<Duration, any Error>?
        var features: [String] = []
    }

    var body: some View {
        List {
            ForEach(store.agents) { agent in
                Section {
                    row("Hermes", value: results[agent.id]?.hermes)
                    if agent.config.bridgeURL != nil {
                        row("Bridge", value: results[agent.id]?.bridge)
                    }
                    if let features = results[agent.id]?.features, !features.isEmpty {
                        Text(features.joined(separator: " · "))
                            .font(Theme.mono)
                            .foregroundStyle(Theme.ink2)
                    }
                } header: {
                    HStack(spacing: 8) {
                        MascotAvatar(appearance: agent.appearance, size: 26)
                        Text(agent.name)
                    }
                }
            }
            Section {
                ShareLink(item: report) { Label("Exporter le rapport", systemImage: "square.and.arrow.up") }
            } footer: {
                Text("Les temps « premier token » et « premier audio » sont mesurés pendant les conversations et les lives.")
            }
        }
        .navigationTitle("Diagnostics")
        .toolbar {
            Button("Relancer", systemImage: "arrow.clockwise") { Task { await run() } }
                .disabled(isRunning)
        }
        .task { await run() }
    }

    private func row(_ title: String, value: Result<Duration, any Error>?) -> some View {
        HStack {
            Text(title).font(Theme.body(16, weight: .bold))
            Spacer()
            switch value {
            case nil:
                ProgressView().controlSize(.small)
            case .success(let duration):
                Label(duration.formatted(.units(allowed: [.milliseconds], width: .narrow)), systemImage: "checkmark.circle.fill")
                    .foregroundStyle(Theme.online)
            case .failure(let error):
                Text(ConversationModel.describe(error))
                    .font(Theme.body(13, weight: .bold))
                    .foregroundStyle(Theme.danger)
                    .multilineTextAlignment(.trailing)
            }
        }
    }

    private func run() async {
        isRunning = true
        defer { isRunning = false }
        results = [:]
        let clock = ContinuousClock()
        for agent in store.agents {
            var diagnostics = AgentDiagnostics()
            if let client = store.client(for: agent) {
                do {
                    let start = clock.now
                    let capabilities = try await client.capabilities()
                    diagnostics.hermes = .success(clock.now - start)
                    diagnostics.features = capabilities.features.filter(\.value).map(\.key).sorted()
                } catch {
                    diagnostics.hermes = .failure(error)
                }
            } else {
                diagnostics.hermes = .failure(HermesError.unauthorized(message: nil))
            }
            if let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) {
                do {
                    let start = clock.now
                    _ = try await bridge.health()
                    diagnostics.bridge = .success(clock.now - start)
                } catch {
                    diagnostics.bridge = .failure(error)
                }
            }
            results[agent.id] = diagnostics
        }
    }

    private var report: String {
        var lines = ["BipAgents — diagnostics \(Date.now.formatted())"]
        for agent in store.agents {
            let diagnostics = results[agent.id]
            lines.append("\n\(agent.name) — \(agent.config.baseURL.absoluteString)")
            lines.append("  Hermes: \(describe(diagnostics?.hermes))")
            if agent.config.bridgeURL != nil { lines.append("  Bridge: \(describe(diagnostics?.bridge))") }
            lines.append("  Features: \(diagnostics?.features.joined(separator: ", ") ?? "-")")
        }
        return lines.joined(separator: "\n")
    }

    private func describe(_ result: Result<Duration, any Error>?) -> String {
        switch result {
        case nil: "…"
        case .success(let duration): duration.formatted(.units(allowed: [.milliseconds], width: .narrow))
        case .failure(let error): "échec — \(ConversationModel.describe(error))"
        }
    }
}
