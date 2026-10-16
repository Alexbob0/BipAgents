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
}
