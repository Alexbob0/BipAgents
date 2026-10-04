import Foundation

/// Turns one chunk of markdown-ish assistant text into plain text for TTS.
///
/// Removes emphasis/code markers (`**`, `__`, `*`, `_`, `~~`, backticks), heading and quote markers,
/// keeps the text of links and images, reads bare URLs as their host, flattens table rows, and turns
/// list items and headings into standalone sentences ending with a pause (`.`).
public enum SpeechNormalizer {
    /// Returns `nil` when nothing speakable is left (rules, table separators, pure markup).
    public static func normalize(_ raw: String) -> String? {
        var text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if text.allSatisfy({ "-*_=|:+ \t".contains($0) }) { return nil } // rules, table separators
        var needsPause = false

        if text.hasPrefix("#") {
            text = String(text.drop { $0 == "#" })
            needsPause = true
        }
        while text.hasPrefix(">") { text = String(text.dropFirst()).trimmingCharacters(in: .whitespaces) }

        if let first = text.first, "-*+•".contains(first), text.dropFirst().first?.isWhitespace == true {
            text = String(text.dropFirst(2))
            if let task = text.prefixMatch(of: /\[[ xX]\]\s+/) { text = String(text[task.range.upperBound...]) }
            needsPause = true
        } else if let ordered = text.prefixMatch(of: /(\d{1,3})[.)]\s+/) {
            text = "\(ordered.output.1), " + text[ordered.range.upperBound...]
            needsPause = true
        }

        if text.hasPrefix("|") || text.hasSuffix("|") {
            text = text.split(separator: "|").map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: ", ")
            needsPause = true
        }

        text = text.replacing(/!\[([^\]]*)\]\([^)]*\)/) { $0.output.1 }
        text = text.replacing(/\[([^\]]+)\]\([^)]*\)/) { $0.output.1 }
        text = text.replacing(/https?:\/\/(?:www\.)?([^\/\s?#]+)\S*/) { $0.output.1 }
        for marker in ["**", "__", "~~", "`", "*"] { text = text.replacingOccurrences(of: marker, with: "") }
        text = removeMarkupUnderscores(text)

        // Closing quotes/brackets left over from the previous sentence's cut.
        text = String(text.drop { ")]»”’".contains($0) || $0.isWhitespace })
        text = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")

        guard text.contains(where: { $0.isLetter || $0.isNumber }) else { return nil }
        if needsPause, let last = text.last, !".!?…:;,".contains(last) { text += "." }
        return text
    }

    /// Drops `_` used as emphasis (`_mot_`, `__init__`) but keeps it inside identifiers (`file_name`).
    private static func removeMarkupUnderscores(_ text: String) -> String {
        let chars = Array(text)
        var result = ""
        result.reserveCapacity(chars.count)
        for (index, char) in chars.enumerated() {
            if char == "_" {
                let wordBefore = index > 0 && (chars[index - 1].isLetter || chars[index - 1].isNumber)
                let wordAfter = index + 1 < chars.count && (chars[index + 1].isLetter || chars[index + 1].isNumber)
                if !(wordBefore && wordAfter) { continue }
            }
            result.append(char)
        }
        return result
    }
}
