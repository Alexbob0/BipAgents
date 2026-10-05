import SwiftUI

/// The agents' scheduled tasks (seen by the bridge) and whether each one notifies. A muted task's
/// replies still land in the Boîte, without a notification.
struct CronJobsView: View {
    @Environment(AgentStore.self) private var store
    @State private var jobs: [UUID: [BridgeClient.CronJob]] = [:]
    @State private var errorMessage: String?
    @State private var isLoading = true

    var body: some View {
        List {
            ForEach(store.agents) { agent in
                if let agentJobs = jobs[agent.id], !agentJobs.isEmpty {
                    Section {
                        ForEach(agentJobs) { job in
                            Toggle(isOn: binding(for: job, agent: agent)) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(job.name ?? job.job).font(Theme.body(16, weight: .bold))
                                    if let seen = job.lastSeen {
                                        Text("Dernier message \(seen.formatted(.relative(presentation: .named)))")
                                            .font(Theme.body(12.5)).foregroundStyle(Theme.ink2)
                                    }
                                }
                            }
                            .tint(agent.appearance.palette.deep)
                        }
                    } header: {
                        HStack(spacing: 8) {
                            MascotAvatar(appearance: agent.appearance, size: 24)
                            Text(agent.name)
                        }
                    }
                }
            }
            if !isLoading && jobs.values.allSatisfy(\.isEmpty) {
                Text("Aucune tâche planifiée vue pour l’instant. Elles apparaissent ici après leur premier message.")
                    .font(Theme.body(14)).foregroundStyle(Theme.ink2)
            }
            if let errorMessage {
                Text(errorMessage).font(Theme.body(14)).foregroundStyle(Theme.danger)
            }
        }
        .navigationTitle("Tâches planifiées")
        .navigationBarTitleDisplayMode(.inline)
        .safeAreaInset(edge: .bottom) {
            Text("Désactivée, une tâche range ses messages dans la Boîte sans notification.")
                .font(Theme.body(12.5)).foregroundStyle(Theme.muted)
                .padding(.horizontal, 24).padding(.bottom, 8)
        }
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        isLoading = true
        defer { isLoading = false }
        var failures = 0
        for agent in store.agents {
            guard let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { continue }
            do {
                jobs[agent.id] = try await bridge.cronJobs(agent: agent.bridgeName)
            } catch {
                failures += 1
            }
        }
        errorMessage = failures > 0 ? "Certains bridges sont injoignables (Tailscale ?) ou pas à jour." : nil
    }

    private func binding(for job: BridgeClient.CronJob, agent: AgentProfile) -> Binding<Bool> {
        Binding {
            jobs[agent.id]?.first { $0.id == job.id }?.notify ?? job.notify
        } set: { notify in
            guard let index = jobs[agent.id]?.firstIndex(where: { $0.id == job.id }),
                  let bridge = BridgeClient(agent: agent, secrets: store.secrets(for: agent)) else { return }
            jobs[agent.id]?[index].notify = notify
            Task {
                do {
                    try await bridge.setCronNotify(agent: job.agent, job: job.job, notify: notify)
                } catch {
                    jobs[agent.id]?[index].notify = !notify
                    errorMessage = "Réglage non enregistré : bridge injoignable."
                }
            }
        }
    }
}
