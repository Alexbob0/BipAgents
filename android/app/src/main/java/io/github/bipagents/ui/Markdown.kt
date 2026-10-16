package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.draw.clip
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.max
import kotlin.math.min

/** A reply's blocks (ChatText.blocks): paragraphs, headings, lists, code, quotes, tables drawn as grids. */
sealed interface Block {
    data class Paragraph(val text: String) : Block
    data class Heading(val level: Int, val text: String) : Block
    data class Items(val ordered: Boolean, val items: List<String>) : Block
    data class Code(val text: String) : Block
    data class Quote(val text: String) : Block
    data class Table(val header: List<String>, val align: List<TextAlign>, val rows: List<List<String>>) : Block
    data object Rule : Block
}

object Markdown {
    private val item = Regex("^\\s*([-*•]|\\d+[.)])\\s+")
    private fun cells(line: String) = line.trim().removePrefix("|").removeSuffix("|").split("|").map { it.trim() }
    private fun separator(line: String) = '-' in line && Regex("^\\s*\\|?\\s*:?-+:?\\s*(\\|\\s*:?-+:?\\s*)*\\|?\\s*$").matches(line)
    private fun tableAt(lines: List<String>, i: Int) = '|' in lines[i] && i + 1 < lines.size && separator(lines[i + 1])

    /** As the iPhone shows a reply (ChatText.blocks + inline-only Markdown): the text as written, cut only around
     *  tables, which become grids. Lists, headings and code fences stay as typed. */
    fun blocks(text: String): List<Block> {
        val lines = ChatText.visible(text).split("\n")
        val out = mutableListOf<Block>()
        val paragraph = mutableListOf<String>()
        var inCode = false
        fun flush() {
            val joined = paragraph.joinToString("\n").trim('\n')
            if (joined.isNotBlank()) out += Block.Paragraph(joined)
            paragraph.clear()
        }
        var i = 0
        while (i < lines.size) {
            val line = lines[i]
            if (line.trimStart().startsWith("```")) inCode = !inCode
            if (!inCode && tableAt(lines, i)) {
                flush()
                val header = cells(line)
                val align = cells(lines[i + 1]).map { c ->
                    when { c.startsWith(":") && c.endsWith(":") -> TextAlign.Center; c.endsWith(":") -> TextAlign.End; else -> TextAlign.Start }
                }
                i += 2
                val rows = mutableListOf<List<String>>()
                while (i < lines.size && '|' in lines[i] && lines[i].isNotBlank()) rows += cells(lines[i++])
                out += Block.Table(header, align, rows)
                continue
            }
            paragraph += line
            i++
        }
        flush()
        return out
    }

    private val inline = Regex("(`[^`\\n]+`)|(\\*\\*[^*\\n]+?\\*\\*)|(\\*[^*\\n]+?\\*|\\b_[^_\\n]+?_\\b)|(\\[[^\\]\\n]+\\]\\([^)\\s]+\\))|((?:https?://|www\\.)[^\\s<>()]+[^\\s<>().,;:!?'\"»])")

    fun inline(text: String, link: Color = Bip.accent): AnnotatedString = buildAnnotatedString {
        appendInline(text, TextLinkStyles(SpanStyle(color = link, textDecoration = TextDecoration.Underline)))
    }

    private fun AnnotatedString.Builder.appendInline(text: String, links: TextLinkStyles) {
        var last = 0
        for (m in inline.findAll(text)) {
            append(text.substring(last, m.range.first))
            val (code, bold, italic, md, bare) = m.destructured
            when {
                code.isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = Bip.line)) { append(code.trim('`')) }
                bold.isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.ExtraBold)) { appendInline(bold.removeSurrounding("**"), links) }
                italic.isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { appendInline(italic.substring(1, italic.length - 1), links) }
                md.isNotEmpty() -> {
                    val split = md.lastIndexOf("](")
                    link(md.substring(1, split), md.substring(split + 2, md.length - 1), links)
                }
                else -> link(bare, bare, links)
            }
            last = m.range.last + 1
        }
        append(text.substring(last))
    }

    private fun AnnotatedString.Builder.link(label: String, raw: String, links: TextLinkStyles) {
        val uri = ChatText.safeLink(raw) ?: return append(label)
        withLink(LinkAnnotation.Url(uri.toString(), links)) { append(ChatText.label(label, uri)) }
    }
}

/** A reply, rendered as on the iPhone (AssistantRow; Nunito one size up for the same visual size): medium, bold heavier, links in the Bip's colour. */
@Composable
fun MarkdownText(text: String, modifier: Modifier = Modifier, style: TextStyle = Bip.text(17, FontWeight.Medium, lineHeight = 24.sp),
                 link: Color = Bip.accent) {
    val blocks = remember(text) { Markdown.blocks(text) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        for (block in blocks) when (block) {
            is Block.Table -> ChatTable(block, style, link)
            is Block.Paragraph -> Text(Markdown.inline(block.text, link), style = style)
            else -> {}
        }
    }
}

/** A table as a grid: each column as wide as its content up to a cap, rows as tall as their tallest cell. */
@Composable
private fun ChatTable(table: Block.Table, style: TextStyle, link: Color) {
    val columns = table.header.size
    val cellStyle = style.copy(fontSize = 14.sp)
    val shape = RoundedCornerShape(12.dp)
    Box(Modifier.horizontalScroll(rememberScrollState()).background(Bip.line, shape).border(1.dp, Bip.line, shape)
        .clip(shape)) {
        Layout(content = {
            table.header.forEachIndexed { c, h ->
                Text(Markdown.inline(h, link), style = cellStyle.copy(fontWeight = FontWeight.ExtraBold, color = Bip.ink2,
                    textAlign = table.align.getOrElse(c) { TextAlign.Start }), modifier = Modifier.background(Bip.ground).padding(horizontal = 12.dp, vertical = 8.dp))
            }
            for (row in table.rows) for (c in 0 until columns) {
                Text(Markdown.inline(row.getOrElse(c) { "" }, link), style = cellStyle.copy(textAlign = table.align.getOrElse(c) { TextAlign.Start }),
                    modifier = Modifier.background(Bip.card).padding(horizontal = 12.dp, vertical = 8.dp))
            }
        }) { measurables, constraints ->
            val cap = 260.dp.roundToPx()
            val widths = IntArray(columns)
            measurables.forEachIndexed { i, m -> val c = i % columns; widths[c] = max(widths[c], min(cap, m.maxIntrinsicWidth(Constraints.Infinity))) }
            val heights = measurables.chunked(columns).map { r -> r.mapIndexed { c, m -> m.minIntrinsicHeight(widths[c]) }.max() }
            val rows = measurables.chunked(columns).mapIndexed { r, cells ->
                cells.mapIndexed { c, m -> m.measure(Constraints.fixed(widths[c], heights[r])) }
            }
            val divider = 1.dp.roundToPx()
            val width = widths.sum()
            layout(width, heights.sum() + divider * (rows.size - 1)) {
                var y = 0
                rows.forEachIndexed { r, cells ->
                    var x = 0
                    cells.forEachIndexed { c, p -> p.place(x, y); x += widths[c] }
                    y += heights[r] + divider
                }
            }
        }
    }
}
