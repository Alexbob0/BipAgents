import Foundation

/// Incrementally cuts streamed assistant text into speakable sentences for sentence-by-sentence TTS.
///
/// Cuts on `.` `!` `?` `…` followed by whitespace, on line breaks (paragraphs, list items, headings),
/// or — failing that — near `maxLength` characters on a word boundary (preferring `,` `;` `:`).
/// Does not cut on decimals (`3.5`), times (`23 h 15`), French abbreviations (`M.`, `Mme.`, `p. ex.`,
/// `etc.` mid-sentence), ordered-list markers (`1.`) or inside URLs. Fenced code blocks are skipped.
/// Every emitted sentence is normalised with `SpeechNormalizer`.
public struct SentenceSplitter: Sendable {
    public let maxLength: Int

    private var buffer: [Character] = []
    private var atLineStart = true
    private var inCodeFence = false

    public init(maxLength: Int = 180) {
        self.maxLength = max(20, maxLength)
    }

    /// Appends a streamed delta; returns the sentences it completes (possibly none).
    public mutating func feed(_ delta: String) -> [String] {
        buffer.append(contentsOf: delta)
        return drain(final: false)
    }

    /// End of stream: returns every remaining sentence and resets the splitter.
    public mutating func flush() -> [String] {
        var sentences = drain(final: true)
        if !inCodeFence, let tail = SpeechNormalizer.normalize(String(buffer)) { sentences.append(tail) }
        reset()
        return sentences
    }

    /// Drops buffered text (e.g. on barge-in).
    public mutating func reset() {
        buffer.removeAll()
        atLineStart = true
        inCodeFence = false
    }

    // MARK: - Cutting

    private struct Cut {
        /// Characters `[0, end)` form the sentence (0 = drop).
        var end: Int
        /// Characters removed from the buffer.
        var consumed: Int
    }

    private enum Decision { case boundary, keepGoing, wait }
    private enum FenceStep { case notFence, wait, skip(Cut) }

    static let terminators: Set<Character> = [".", "!", "?", "…"]
    /// Closing marks that stay attached to the sentence they end.
    static let closers: Set<Character> = [")", "]", "\"", "'", "»", "”", "’", "*", "_", "`"]
    /// Title abbreviations, case-sensitive: never end a sentence (`M. Dupont`).
    static let titles: Set<String> = ["M", "MM", "Mme", "Mmes", "Mlle", "Mlles", "Dr", "Pr", "Me", "Mgr", "St", "Ste"]
    /// Abbreviations that are practically never sentence-final.
    static let inlineAbbreviations: Set<String> = ["p", "pp", "ex", "cf", "c.-à-d", "c-à-d", "i.e", "e.g", "vs", "av", "apr",
                                                   "réf", "fig", "chap", "vol", "tél", "n°", "no", "art", "éd", "coll", "trad"]
    /// Abbreviations that end a sentence only when an uppercase word follows (`etc. Puis…`).
    static let trailingAbbreviations: Set<String> = ["etc", "env", "approx", "min", "max", "al", "ibid", "sq"]

    private mutating func drain(final: Bool) -> [String] {
        var sentences: [String] = []
        while let cut = nextCut(final: final) {
            let raw = String(buffer[..<cut.end])
            buffer.removeFirst(cut.consumed)
            atLineStart = false // restored by the next leading-whitespace skip if a newline follows
            if let sentence = SpeechNormalizer.normalize(raw) { sentences.append(sentence) }
        }
        return sentences
    }

    private mutating func nextCut(final: Bool) -> Cut? {
        let leading = buffer.prefix { $0.isWhitespace }
        if leading.contains(where: \.isNewline) { atLineStart = true }
        buffer.removeFirst(leading.count)
        guard !buffer.isEmpty else { return nil }

        switch fenceStep(final: final) {
        case .notFence: break
        case .wait: return nil
        case .skip(let cut): return cut
        }

        var i = 0
        while i < buffer.count {
            if i >= maxLength { return lengthCut() }
            let c = buffer[i]
            if c.isNewline { return Cut(end: i, consumed: i) }
            guard Self.terminators.contains(c) else { i += 1; continue }

            var j = i + 1
            while j < buffer.count, Self.terminators.contains(buffer[j]) { j += 1 }
            let isSinglePeriod = c == "." && j == i + 1
            while j < buffer.count, Self.closers.contains(buffer[j]) { j += 1 }
            guard j < buffer.count else { return final ? Cut(end: j, consumed: j) : nil }

            if buffer[j].isWhitespace {
                switch isSinglePeriod ? periodDecision(period: i, after: j, final: final) : .boundary {
                case .boundary: return Cut(end: j, consumed: j)
                case .wait: return nil
                case .keepGoing: break
                }
            }
            i = j
        }
        return nil
    }

    /// Skips fenced code blocks (opening line, content, closing line) one line at a time.
    private mutating func fenceStep(final: Bool) -> FenceStep {
        guard atLineStart || inCodeFence else { return .notFence }
        let opensFence = buffer.starts(with: "```") || buffer.starts(with: "~~~")
        if !inCodeFence && !opensFence {
            // Might be the start of a fence still arriving.
            let partialFence = buffer.count < 3 && buffer.allSatisfy { $0 == "`" || $0 == "~" }
            return partialFence && !final ? .wait : .notFence
        }
        guard let newline = buffer.firstIndex(where: \.isNewline) else { return .wait } // need the whole line
        if inCodeFence {
            let line = String(buffer[..<newline]).trimmingCharacters(in: .whitespaces)
            if line.hasPrefix("```") || line.hasPrefix("~~~") { inCodeFence = false }
        } else {
            inCodeFence = true
        }
        return .skip(Cut(end: 0, consumed: newline))
    }

    /// Cut near `maxLength`: prefer a clause break (`,;:`) in the second half, else the last space.
    private func lengthCut() -> Cut? {
        let window = buffer[..<min(maxLength, buffer.count)]
        if let clause = window.indices.last(where: { k in
            k >= maxLength / 2 && ",;:".contains(window[k]) && k + 1 < buffer.count && buffer[k + 1].isWhitespace
        }) {
            return Cut(end: clause + 1, consumed: clause + 1)
        }
        if let space = window.lastIndex(where: \.isWhitespace), space > 0 { return Cut(end: space, consumed: space) }
        if let space = buffer[window.endIndex...].firstIndex(where: \.isWhitespace) { return Cut(end: space, consumed: space) }
        return nil // one giant token (e.g. a URL): wait for whitespace or flush
    }

    /// Is the single `.` at `period` (followed by whitespace at `after`) the end of a sentence?
    private func periodDecision(period: Int, after: Int, final: Bool) -> Decision {
        var start = period
        while start > 0, !buffer[start - 1].isWhitespace { start -= 1 }
        let token = String(String(buffer[start..<period]).drop { "([{\"'«“*_`".contains($0) })
        guard !token.isEmpty else { return .boundary }

        if token.allSatisfy(\.isNumber) {
            // Ordered-list marker ("1. ", "## 2. ") at the start of a line.
            let lineStart = buffer[..<start].lastIndex(where: \.isNewline).map { $0 + 1 }
            let prefixIsMarkup = buffer[(lineStart ?? 0)..<start].allSatisfy { $0.isWhitespace || "#>-*+•".contains($0) }
            return token.count <= 3 && prefixIsMarkup && (lineStart != nil || atLineStart) ? .keepGoing : .boundary
        }
        if Self.titles.contains(token) || Self.inlineAbbreviations.contains(token.lowercased()) { return .keepGoing }
        if token.count == 1, token.first!.isUppercase { return .keepGoing } // initial: "J. Dupont"

        let segments = token.split(separator: ".")
        let isInitialism = segments.count > 1 && segments.allSatisfy { $0.count <= 2 } // "J.-C", "U.S"
        guard isInitialism || Self.trailingAbbreviations.contains(token.lowercased()) else { return .boundary }

        guard let next = buffer[after...].first(where: { !$0.isWhitespace || $0.isNewline }) else {
            return final ? .boundary : .wait
        }
        return next.isNewline || next.isUppercase ? .boundary : .keepGoing
    }
}
