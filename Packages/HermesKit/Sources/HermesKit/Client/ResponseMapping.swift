import Foundation

// Lenient mapping of REST response bodies → models. Kept in one file, like the event mapping,
// because the exact shapes of the Sessions API rows are not documented.

enum ResponseMapping {
    /// A list either at the top level or wrapped (`{"sessions": [...]}`, `{"data": [...]}`, …).
    static func list(_ json: JSONValue, keys: [String]) -> [JSONValue] {
        if let array = json.arrayValue { return array }
        for key in keys + ["data", "items", "results"] {
            if let array = json[key]?.arrayValue { return array }
        }
        return []
    }

    /// An object either at the top level or wrapped (`{"session": {...}}`).
    static func unwrap(_ json: JSONValue, key: String) -> JSONValue {
        json[key]?.objectValue != nil ? json[key]! : json
    }

    static func session(_ json: JSONValue) -> HermesSession? {
        let f = LenientFields(unwrap(json, key: "session"), nestedIn: [])
        guard let id = f.string("id", "session_id") else { return nil }
        return HermesSession(
            id: id,
            title: f.string("title", "name")?.nilIfEmpty,
            createdAt: f.date("created_at", "started_at", "created"),
            updatedAt: f.date("updated_at", "last_active", "last_activity", "last_message_at", "ended_at"),
            lastMessagePreview: f.string("preview", "last_message_preview", "last_message", "snippet"),
            messageCount: f.int("message_count", "messages"),
            source: f.string("source", "platform"),
            model: f.string("model"),
            parentSessionID: f.string("parent_session_id", "parent_id")
        )
    }

    static func message(_ json: JSONValue, index: Int) -> HermesMessage? {
        let f = LenientFields(json, nestedIn: [])
        guard let rawRole = f.string("role") else { return nil }
        let role = HermesMessage.Role(rawValue: rawRole.lowercased()) ?? (rawRole == "developer" ? .system : .notice)

        var text = ""
        var attachments: [MessageAttachment] = []
        switch f.value("content") {
        case .string(let string)?:
            text = string
        case .array(let parts)?:
            var texts: [String] = []
            for part in parts {
                let p = LenientFields(part, nestedIn: [])
                switch p.string("type") {
                case "image_url", "input_image", "image":
                    let url = p.value("image_url").flatMap { $0.stringValue ?? $0["url"]?.stringValue }
                    attachments.append(MessageAttachment(kind: .image, url: url))
                default:
                    if let t = p.string("text") ?? part.stringValue { texts.append(t) }
                }
            }
            text = texts.joined(separator: "\n")
        default:
            text = f.string("text") ?? ""
        }

        var tools: [ToolEvent] = []
        for call in f.value("tool_calls")?.arrayValue ?? [] {
            let c = LenientFields(call, nestedIn: ["function"])
            tools.append(ToolEvent(tool: c.string("name") ?? "tool", preview: c.string("arguments"),
                                   status: .completed, callID: c.string("id")))
        }
        if role == .tool, let name = f.string("tool_name", "name") {
            tools.append(ToolEvent(tool: name, preview: text.nilIfEmpty, status: .completed, callID: f.string("tool_call_id")))
        }

        return HermesMessage(
            id: f.string("id", "message_id") ?? "\(index)",
            role: role,
            text: text,
            createdAt: f.date("created_at", "timestamp", "ts"),
            attachments: attachments,
            toolEvents: tools,
            reasoning: f.string("reasoning", "reasoning_content", "thinking")?.nilIfEmpty
        )
    }

    static func run(_ json: JSONValue, fallbackID: String) -> HermesRun {
        let f = LenientFields(unwrap(json, key: "run"), nestedIn: [])
        return HermesRun(
            runID: f.string("run_id", "id") ?? fallbackID,
            status: RunStatus(rawValue: f.string("status") ?? "unknown"),
            sessionID: f.string("session_id"),
            model: f.string("model"),
            outcome: HermesEvent.outcome(f),
            shutdownRequestedAt: f.date("shutdown_requested_at"),
            raw: json
        )
    }

    static func capabilities(_ json: JSONValue) -> HermesCapabilities {
        var features: [String: Bool] = [:]
        for (name, value) in json["features"]?.objectValue ?? [:] {
            switch value {
            case .null: features[name] = false
            case .object:
                features[name] = (value["supported"] ?? value["enabled"])?.boolValue ?? true
            default:
                features[name] = value.boolValue ?? true
            }
        }
        // Some flags (e.g. `session_key_header`) live at the top level.
        for (name, value) in json.objectValue ?? [:] where features[name] == nil && value.boolValue == true {
            if case .bool = value { features[name] = true }
        }
        return HermesCapabilities(
            platform: json["platform"]?.stringValue,
            model: json["model"]?.stringValue,
            features: features,
            endpoints: json["endpoints"]?.objectValue ?? [:],
            raw: json
        )
    }

    static func approvalResult(_ json: JSONValue) -> ApprovalResult {
        let f = LenientFields(json, nestedIn: [])
        return ApprovalResult(runID: f.string("run_id"), choice: f.string("choice").flatMap(ApprovalChoice.init(lenient:)),
                              requestID: f.string("request_id"), resolved: f.int("resolved"))
    }
}
