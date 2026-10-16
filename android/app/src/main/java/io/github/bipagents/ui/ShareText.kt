package io.github.bipagents.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.text.Html

/**
 * A reply taken out of the app (MessageActions.swift): copied, shared, forwarded. Messaging apps format text their own
 * way, so it goes out in WhatsApp's syntax (`*bold*`, `_italic_`, `~struck~`, `` `code` ``, lists, quotes), which
 * WhatsApp shows formatted and others read cleanly; copying also carries the formatted text (HTML) for the apps that
 * take it (notes, mail, Telegram). Links are written « label : address » with the real address.
 */
object ShareText {
    private val mdLink = Regex("\\[([^\\]\\n]+)\\]\\(([^)\\s]+)\\)")
    private val bold = Regex("\\*\\*([^*\\n]+?)\\*\\*|__([^_\\n]+?)__")
    private val italic = Regex("(?<![*\\w])\\*([^*\\n]+?)\\*(?![*\\w])|(?<![_\\w])_([^_\\n]+?)_(?![_\\w])")
    private val strike = Regex("~~([^~\\n]+?)~~")
    private val heading = Regex("(?m)^#{1,6}\\s+(.+)$")

    /** As read on screen: no Markdown marks, no MEDIA lines. */
    fun plain(text: String): String = ChatText.visible(text)
        .replace(mdLink) { link(it.groupValues[1], it.groupValues[2]) }
        .replace(bold) { it.groupValues[1].ifEmpty { it.groupValues[2] } }
        .replace(strike) { it.groupValues[1] }
        .replace(italic) { it.groupValues[1].ifEmpty { it.groupValues[2] } }
        .replace(heading) { it.groupValues[1] }
        .replace(Regex("(?<!`)`([^`\\n]+)`(?!`)"), "$1")

    /** WhatsApp's syntax. */
    fun whatsapp(text: String): String {
        val out = ArrayList<String>()
        var inCode = false
        for (line in tables(ChatText.visible(text)).split("\n")) {
            if (line.trimStart().startsWith("```")) { inCode = !inCode; out += line; continue }
            if (inCode) { out += line; continue }
            out += line
                .replace(mdLink) { link(it.groupValues[1], it.groupValues[2]) }
                .replace(Regex("^#{1,6}\\s+(.+)$")) { "\u0000" + it.groupValues[1].replace("**", "") + "\u0000" }
                .replace(bold) { "\u0000" + it.groupValues[1].ifEmpty { it.groupValues[2] } + "\u0000" }
                .replace(italic) { "_" + it.groupValues[1].ifEmpty { it.groupValues[2] } + "_" }
                .replace(strike) { "~" + it.groupValues[1] + "~" }
                .replace('\u0000', '*')
                .replace(Regex("^(\\s*)[*•]\\s+"), "$1- ")
        }
        return out.joinToString("\n").trim()
    }

    /** The formatted text as HTML, for the apps that paste rich text. */
    fun html(text: String): String {
        val escaped = Html.escapeHtml(tables(ChatText.visible(text)))
        return escaped
            .replace(mdLink) { "<a href=\"${it.groupValues[2]}\">${it.groupValues[1]}</a>" }
            .replace(bold) { "<b>" + it.groupValues[1].ifEmpty { it.groupValues[2] } + "</b>" }
            .replace(italic) { "<i>" + it.groupValues[1].ifEmpty { it.groupValues[2] } + "</i>" }
            .replace(strike) { "<s>" + it.groupValues[1] + "</s>" }
            .replace(Regex("(?m)^#{1,6}\\s+(.+)$")) { "<b>" + it.groupValues[1] + "</b>" }
            .replace("\n", "<br>")
    }

    /** A table as lines (« Lundi · 7 h 12 · 82 »): grids don't survive a chat app. */
    private fun tables(text: String): String = Markdown.blocks(text).joinToString("\n\n") { block ->
        when (block) {
            is Block.Table -> (listOf(block.header) + block.rows).joinToString("\n") { it.joinToString(" · ") }
            is Block.Paragraph -> block.text
            else -> ""
        }
    }

    private fun link(label: String, raw: String): String {
        val uri = ChatText.safeLink(raw) ?: return label
        val address = uri.toString()
        return if (label.trim() == address || label.trim() == address.removePrefix("https://")) address else "$label : $address"
    }

    fun copy(context: Context, text: String) {
        val clip = ClipData.newHtmlText("BipAgents", whatsapp(text), html(text))
        context.getSystemService(ClipboardManager::class.java).setPrimaryClip(clip)
    }

    fun share(context: Context, text: String) {
        context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, whatsapp(text)), null))
    }
}
