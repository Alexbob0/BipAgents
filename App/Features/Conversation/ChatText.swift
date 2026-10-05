import Foundation

/// Turns raw Hermes / model text into what the thread shows.
enum ChatText {
    /// The reply without tool-call markup a model sometimes writes as text instead of calling the tool
    /// (`<tool_call>execute_code<arg_key>code</arg_key><arg_value>…</tool_call>`), including an
    /// unfinished one while the reply streams.
    static func visible(_ text: String) -> String {
        var result = text
        if result.contains("MEDIA:") { result = result.replacing(/(?m)^[ \t]*MEDIA:[ \t]*\S+[ \t]*$\n?/, with: "") }
        guard result.contains("<") else { return result.trimmingCharacters(in: .whitespacesAndNewlines) }
        result = result.replacing(/<tool_call>[\s\S]*?(?:<\/tool_call>|$)/, with: "")
        result = result.replacing(/<\/?(?:arg_key|arg_value|tool_call|tool_response|function_calls?|invoke|parameter)\b[^>]*>/, with: "")
        return result.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Files the reply points to with « MEDIA:<path> » lines (the podcast mp3…), shown as players.
    static func media(in text: String) -> [String] {
        guard text.contains("MEDIA:") else { return [] }
        return text.matches(of: /(?m)^[ \t]*MEDIA:[ \t]*(\S+)[ \t]*$/).map { String($0.output.1) }
    }

    /// The choices an agent asks the user to pick from (« 1. … 2. … » or « A) … B) … » after a question),
    /// offered as quick replies. Plain lists in an answer (tips, steps) are left alone: the message must
    /// ask something, and offer 2 to 5 short options.
    static func choices(in text: String) -> [String] {
        let visible = visible(text)
        let asks = visible.contains("?") || visible.range(of: #"\b(choisis|choisir|préfères|option|laquelle|lequel|veux-tu|souhaites|dis-moi)\b"#,
                                                         options: [.regularExpression, .caseInsensitive]) != nil
        guard asks else { return [] }
        var options: [String] = []
        for line in visible.split(whereSeparator: \.isNewline) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard let match = trimmed.firstMatch(of: /^(?:\*\*)?(?:[1-9]|[A-Ea-e])[.)]\s*(?:\*\*)?\s*(.+)$/) else { continue }
            let option = String(match.output.1).replacingOccurrences(of: "**", with: "").trimmingCharacters(in: .whitespaces)
            guard !option.isEmpty else { continue }
            options.append(option)
        }
        guard (2...5).contains(options.count), options.allSatisfy({ $0.count <= 140 }) else { return [] }
        return options
    }

    /// A long prompt the user did not type (skill instructions, a scheduled task's brief): its card title.
    static func instructionTitle(for text: String, inCronSession: Bool, isFirstUserMessage: Bool) -> String? {
        let skills = text.matches(of: /invoked the "([^"]+)" skill/).map { String($0.output.1) }
        if inCronSession && isFirstUserMessage {
            return skills.isEmpty ? "Consigne de la tâche planifiée" : "Tâche planifiée · \(skills.joined(separator: ", "))"
        }
        if text.hasPrefix("[IMPORTANT:"), !skills.isEmpty {
            return "Consigne · \(skills.joined(separator: ", "))"
        }
        return nil
    }

    /// One readable line for a tool card: the query, command or path rather than raw JSON / tool output.
    static func toolSummary(_ preview: String) -> String {
        var text = preview.replacing(/<\/?untrusted_tool_result[^>]*>/, with: "")
        text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.hasPrefix("{"), let data = text.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            let keys = ["query", "command", "path", "file_path", "url", "pattern", "name", "code", "content", "text", "input", "output"]
            if let value = keys.lazy.compactMap({ object[$0] as? String }).first(where: { !$0.isEmpty }) {
                text = value
            } else if let first = object.values.compactMap({ $0 as? String }).first {
                text = first
            }
        }
        let line = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? text
        return line.trimmingCharacters(in: .whitespaces)
    }
}
