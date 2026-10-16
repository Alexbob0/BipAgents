package io.github.bipagents.ui

import android.net.Uri

/** Raw Hermes / model text → what the thread shows (App/Features/Conversation/ChatText.swift). */
object ChatText {
    private val mediaLine = Regex("(?m)^[ \\t]*MEDIA:[ \\t]*(\\S+)[ \\t]*$\\n?")

    /** The reply without MEDIA lines and the tool-call markup a model sometimes writes as text. */
    fun visible(text: String): String {
        var result = text.replace(mediaLine, "")
        if ('<' in result) {
            result = result.replace(Regex("<tool_call>[\\s\\S]*?(?:</tool_call>|$)"), "")
                .replace(Regex("</?(?:arg_key|arg_value|tool_call|tool_response|function_calls?|invoke|parameter)\\b[^>]*>"), "")
        }
        return result.trim()
    }

    /** Files the reply points to with « MEDIA:<path> » lines. */
    fun media(text: String): List<String> = mediaLine.findAll(text).map { it.groupValues[1] }.toList()

    /** One line for a home card or a session row. */
    fun preview(text: String?): String {
        if (text == null) return ""
        val file = media(text).firstOrNull()
        val words = visible(text).replace(Regex("\\s+"), " ")
        if (file == null) return words
        val name = file.substringAfterLast('/')
        val icon = when (name.substringAfterLast('.').lowercase()) {
            "mp3", "m4a", "wav", "ogg", "opus", "aac", "flac" -> "🎧"
            "png", "jpg", "jpeg", "gif", "webp", "heic" -> "🖼️"
            "mp4", "m4v", "mov" -> "🎬"
            else -> "📎"
        }
        return if (words.isEmpty()) "$icon $name" else "$icon $name · $words"
    }

    /** Links open only for these (ConversationView.openable; maps: has no meaning here). */
    private val openable = setOf("http", "https", "tel", "mailto")

    fun safeLink(raw: String): Uri? {
        val uri = runCatching { Uri.parse(if (raw.startsWith("www.")) "https://$raw" else raw) }.getOrNull() ?: return null
        return uri.takeIf { it.scheme?.lowercase() in openable }
    }

    /** « fr.wikipedia.org › Caféine » rather than a whole address. */
    fun shortLabel(uri: Uri): String {
        val host = (uri.host ?: return uri.toString()).removePrefix("www.")
        val last = uri.pathSegments.lastOrNull()?.let { Uri.decode(it).replace(Regex("[-_]"), " ") }
        return if (!last.isNullOrEmpty() && last.length < 40) "$host › $last" else host
    }

    /** A label that looks like an address but names another site shows the real one (checkLinks). */
    fun label(label: String, uri: Uri): String {
        if (uri.scheme?.startsWith("http") != true) return label
        val l = label.trim().lowercase()
        if (l == uri.toString().lowercase()) return shortLabel(uri)
        val looksLikeAddress = (" " !in l && Regex("^[a-z0-9.-]+\\.[a-z]{2,}(/|$)").containsMatchIn(l)) || l.startsWith("http") || l.startsWith("www.")
        val named = l.removePrefix("https://").removePrefix("http://").removePrefix("www.").substringBefore('/')
        val real = (uri.host ?: "").lowercase().removePrefix("www.")
        return if (looksLikeAddress && named != real && !named.endsWith(".$real") && !real.endsWith(".$named")) shortLabel(uri) else label
    }

    /** The choices an agent asks the person to pick from (« 1. … 2. … » or « A) … B) … » after a question), offered as
     *  quick replies. Plain lists in an answer (tips, steps) are left alone: the message must ask something, and offer
     *  2 to 5 short options. */
    fun choices(text: String): List<String> {
        val shown = visible(text)
        val asks = '?' in shown || Regex("\\b(choisis|choisir|préfères|option|laquelle|lequel|veux-tu|souhaites|dis-moi|choose|pick|prefer|which|options?|elige|escoge|prefieres|cuál|opción|wähle|wählen|bevorzugst|welche[nrs]?|möchtest)\\b",
            RegexOption.IGNORE_CASE).containsMatchIn(shown)
        if (!asks) return emptyList()
        val options = shown.lines().mapNotNull { line ->
            Regex("^(?:\\*\\*)?(?:[1-9]|[A-Ea-e])[.)]\\s*(?:\\*\\*)?\\s*(.+)$").find(line.trim())?.groupValues?.get(1)
                ?.replace("**", "")?.trim()?.ifEmpty { null }
        }
        return if (options.size in 2..5 && options.all { it.length <= 140 }) options else emptyList()
    }

    data class Teammate(val name: String, val handle: String, val body: String)

    /** A message another agent sent through Hermes Bot Mode (`message_agent`): « Message from 🤖 Wellness (@wellness): … ». */
    fun teammate(text: String): Teammate? =
        Regex("^\\s*Message from \\S*\\s*(.+?) \\(@([\\w.-]+)\\):\\s*([\\s\\S]*)$").find(text)?.destructured
            ?.let { (name, handle, body) -> Teammate(name, handle, body) }

    /** A long prompt the person did not type: a scheduled task's brief, skill instructions, a sub-task's report. */
    sealed interface Instruction {
        data class Scheduled(val skills: String) : Instruction
        data class Skill(val skills: String) : Instruction
        data object Subtask : Instruction
    }

    fun instruction(text: String, inCronSession: Boolean, firstUserMessage: Boolean): Instruction? {
        val skills = Regex("invoked the \"([^\"]+)\" skill").findAll(text).map { it.groupValues[1] }.joinToString(", ")
        return when {
            inCronSession && firstUserMessage -> Instruction.Scheduled(skills)
            text.startsWith("[IMPORTANT:") && skills.isNotEmpty() -> Instruction.Skill(skills)
            isSubtaskReport(text) -> Instruction.Subtask
            else -> null
        }
    }

    /** What Hermes hands the agent when a background sub-agent (`delegate_task`) finishes. */
    fun isSubtaskReport(text: String): Boolean = text.take(80).let {
        it.startsWith("[ASYNC DELEGATION") || it.startsWith("[DELEGATION") || it.startsWith("[SUBAGENT")
    }

    /** Media files a report names (the podcast a sub-agent just made), in order. */
    fun mediaPaths(report: String): List<String> =
        Regex("/[\\w./-]*/media/[\\w./-]+\\.(?:mp3|m4a|wav|ogg|opus|png|jpe?g|gif|webp|heic|mp4|m4v|mov)").findAll(report)
            .map { it.value }.distinct().toList()

    private val summaryKeys = listOf("query", "command", "path", "file_path", "url", "message", "pattern", "name", "code", "content",
        "text", "input", "output", "result")

    /** One readable line for a tool card: the query, command or path rather than raw JSON or tool output. Previews are
     *  often cut (invalid JSON): the first useful « "key": "value" » is then read directly. */
    fun toolSummary(preview: String, depth: Int = 0): String {
        var text = preview.replace(Regex("</?untrusted_tool_result[^>]*>"), "").trim()
        if (text.startsWith("{")) {
            var value: String? = null
            val parsed = runCatching { io.github.bipagents.net.json.parseToJsonElement(text) as? kotlinx.serialization.json.JsonObject }.getOrNull()
            if (parsed != null) {
                fun str(e: kotlinx.serialization.json.JsonElement?) = (e as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content
                value = summaryKeys.firstNotNullOfOrNull { k -> str(parsed[k])?.ifEmpty { null } } ?: parsed.values.firstNotNullOfOrNull { str(it) }
                if (value == null && (parsed["ok"] as? kotlinx.serialization.json.JsonPrimitive)?.content == "true") return "OK"
            } else {
                for (key in summaryKeys) {
                    val m = Regex("\"" + key + "\"\\s*:\\s*\"((?:[^\"\\\\]|\\\\.)*)").find(text) ?: continue
                    value = m.groupValues[1].replace("\\n", "\n").replace("\\\"", "\""); break
                }
            }
            if (value != null) {
                val inner = value.trim()
                if (depth == 0 && inner.startsWith("{")) return toolSummary(inner, 1)
                text = value
            } else if ("\"ok\": true" in text || "\\\"ok\\\": true" in text) return "OK"
        }
        return text.lineSequence().first().trim()
    }
}
