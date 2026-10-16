package io.github.bipagents.voice

/**
 * Streamed reply text cut into sentences to speak one by one (VoiceKit's SentenceSplitter, simplified): on `.` `!` `?`
 * `…` followed by a space, on line breaks, or near [max] characters at a comma or a space. Not on decimals (`3.5`),
 * common abbreviations (`M.`, `etc.` mid-sentence, `p. ex.`) or list numbers (`1.`); code blocks are skipped.
 */
class Sentences(private val max: Int = 180) {
    private val buffer = StringBuilder()
    private var inCode = false

    fun feed(delta: String): List<String> { buffer.append(delta); return drain(false) }

    fun flush(): List<String> = drain(true).also { buffer.clear(); inCode = false }

    fun reset() { buffer.clear(); inCode = false }

    private fun drain(final: Boolean): List<String> {
        val out = mutableListOf<String>()
        while (true) {
            val cut = nextCut(final) ?: break
            val sentence = clean(buffer.substring(0, cut.first))
            buffer.delete(0, cut.second)
            if (sentence != null) out += sentence
        }
        return out
    }

    /** (end of the sentence, characters consumed), or null to wait for more text. */
    private fun nextCut(final: Boolean): Pair<Int, Int>? {
        if (buffer.isEmpty()) return null
        val text = buffer.toString()
        // Code fences: everything up to the closing fence is dropped.
        val fence = text.indexOf("```")
        if (inCode) {
            if (fence < 0) return if (final) 0 to text.length else null
            inCode = false
            val end = text.indexOf('\n', fence).let { if (it < 0) fence + 3 else it + 1 }
            return 0 to end
        }
        val newline = text.indexOf('\n')
        for (i in text.indices) {
            if (i == fence) {
                inCode = true
                return i to (text.indexOf('\n', i).let { if (it < 0) text.length.also { if (!final) return i to i } else it + 1 })
            }
            if (i == newline) return i to i + 1
            val c = text[i]
            if (c in ".!?…") {
                if (i + 1 >= text.length) return if (final) text.length to text.length else null
                var j = i + 1
                while (j < text.length && text[j] in ")]\"'»”’*_`") j++
                if (j >= text.length) return if (final) text.length to text.length else null
                if (!text[j].isWhitespace()) continue
                if (c == '.' && !endsSentence(text, i, j)) continue
                return j to j
            }
        }
        if (text.length > max) {
            val window = text.substring(0, max)
            val at = listOf(", ", "; ", ": ").maxOf { window.lastIndexOf(it) }.takeIf { it > max / 3 }?.plus(1)
                ?: window.lastIndexOf(' ').takeIf { it > 0 } ?: max
            return at to at
        }
        return if (final) text.length to text.length else null
    }

    private fun endsSentence(text: String, dot: Int, next: Int): Boolean {
        val word = text.substring(0, dot).takeLastWhile { !it.isWhitespace() && it != '(' }
        if (word.isEmpty()) return true
        if (word.all { it.isDigit() } && (dot < 4 || text.substring(0, dot - word.length).trimEnd().let { it.isEmpty() || it.endsWith('\n') })) return false   // « 1. » list marker
        if (word.lowercase() in setOf("p", "pp", "ex", "cf", "vs", "env", "art", "fig", "tél", "n°", "no")) return false
        if (word in setOf("M", "MM", "Mme", "Mlle", "Dr", "Pr", "Me", "St", "Mr", "Mrs", "Ms")) return false
        if (word.lowercase() == "etc") return next + 1 < text.length && text[next + 1].isUpperCase()
        return true
    }

    /** Markdown marks and MEDIA lines out; null when nothing is left to say. */
    private fun clean(raw: String): String? {
        val text = raw.replace(Regex("^\\s*MEDIA:\\S+\\s*$", RegexOption.MULTILINE), "")
            .replace(Regex("\\[([^\\]]+)\\]\\([^)]+\\)"), "$1")
            .replace(Regex("[*_`#>|]+"), " ")
            .replace(Regex("^\\s*[-•]\\s+"), "")
            .replace(Regex("\\s+"), " ").trim()
        return text.takeIf { it.any(Char::isLetterOrDigit) }
    }
}
