import Foundation

/// A Hermes scheduled task (`/api/jobs`), shown in the app as a routine: what the agent does, and when.
public struct Routine: Sendable, Hashable, Identifiable {
    public var id: String
    public var name: String
    public var prompt: String
    /// The schedule as stored by Hermes: a cron expression (`0 8 * * 1-5`), an interval (`every 2h`) or a date.
    public var schedule: String
    public var scheduleDisplay: String?
    /// `scheduled`, `paused`, `completed`, `error`…
    public var state: String
    public var enabled: Bool
    public var nextRunAt: Date?
    public var lastRunAt: Date?
    public var lastStatus: String?

    public var isPaused: Bool { state == "paused" || !enabled }

    init?(json: JSONValue) {
        guard let id = json["id"]?.lenientString else { return nil }
        self.id = id
        name = json["name"]?.stringValue ?? id
        prompt = json["prompt"]?.stringValue ?? ""
        let schedule = json["schedule"]
        self.schedule = schedule?["expr"]?.stringValue ?? schedule?["run_at"]?.stringValue
            ?? schedule?["minutes"]?.intValue.map { "every \($0)m" } ?? schedule?.stringValue
            ?? json["schedule_display"]?.stringValue ?? ""
        scheduleDisplay = json["schedule_display"]?.stringValue
        state = json["state"]?.stringValue ?? "scheduled"
        enabled = json["enabled"]?.boolValue ?? true
        nextRunAt = json["next_run_at"]?.dateValue
        lastRunAt = json["last_run_at"]?.dateValue
        lastStatus = json["last_status"]?.stringValue
    }
}

extension HermesClient {
    /// `GET /api/jobs?include_disabled=true`: every routine of this agent, paused ones included.
    public func routines() async throws -> [Routine] {
        let json = try await send("GET", "/api/jobs", query: ["include_disabled": "true"])
        return (json["jobs"]?.arrayValue ?? []).compactMap(Routine.init(json:))
    }

    /// `POST /api/jobs`: delivered locally (the bridge puts its report in the Boîte and notifies).
    public func createRoutine(name: String, schedule: String, prompt: String) async throws -> Routine {
        let body: JSONValue = ["name": .string(name), "schedule": .string(schedule), "prompt": .string(prompt),
                               "deliver": "local"]
        return try routine(from: try await send("POST", "/api/jobs", body: body))
    }

    /// `PATCH /api/jobs/{id}`: only the given fields change.
    public func updateRoutine(id: String, name: String? = nil, schedule: String? = nil, prompt: String? = nil) async throws -> Routine {
        var fields: [String: JSONValue] = [:]
        if let name { fields["name"] = .string(name) }
        if let schedule { fields["schedule"] = .string(schedule) }
        if let prompt { fields["prompt"] = .string(prompt) }
        return try routine(from: try await send("PATCH", "/api/jobs/\(Self.escape(id))", body: .object(fields)))
    }

    public func deleteRoutine(id: String) async throws {
        _ = try await send("DELETE", "/api/jobs/\(Self.escape(id))")
    }

    public func setRoutine(id: String, paused: Bool) async throws {
        _ = try await send("POST", "/api/jobs/\(Self.escape(id))/\(paused ? "pause" : "resume")")
    }

    /// `POST /api/jobs/{id}/run`: now, once, besides its schedule.
    public func runRoutine(id: String) async throws {
        _ = try await send("POST", "/api/jobs/\(Self.escape(id))/run")
    }

    private func routine(from json: JSONValue) throws -> Routine {
        guard let routine = (json["job"]).flatMap(Routine.init(json:)) else {
            throw HermesError.invalidResponse("job missing")
        }
        return routine
    }
}
