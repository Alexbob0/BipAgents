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

    fun blocks(text: String): List<Block> {
        val lines = ChatText.visible(text).split("\n")
        val out = mutableListOf<Block>()
        var i = 0
        while (i < lines.size) {
            val line = lines[i]
            when {
                line.trimStart().startsWith("```") -> {
                    val body = mutableListOf<String>()
                    i++
                    while (i < lines.size && !lines[i].trimStart().startsWith("```")) body += lines[i++]
                    i++
                    out += Block.Code(body.joinToString("\n"))
                }
                tableAt(lines, i) -> {
                    val header = cells(line)
                    val align = cells(lines[i + 1]).map { c ->
                        when { c.startsWith(":") && c.endsWith(":") -> TextAlign.Center; c.endsWith(":") -> TextAlign.End; else -> TextAlign.Start }
                    }
                    i += 2
                    val rows = mutableListOf<List<String>>()
                    while (i < lines.size && '|' in lines[i] && lines[i].isNotBlank()) rows += cells(lines[i++])
                    out += Block.Table(header, align, rows)
                }
                Regex("^\\s*#{1,6}\\s").containsMatchIn(line) -> {
                    val level = line.trim().takeWhile { it == '#' }.length
                    out += Block.Heading(level, line.trim().trimStart('#').trim()); i++
                }
                item.containsMatchIn(line) -> {
                    val ordered = line.trimStart().first().isDigit()
                    val items = mutableListOf<String>()
                    while (i < lines.size && item.containsMatchIn(lines[i])) {
                        var entry = lines[i++].replace(item, "")
                        while (i < lines.size && Regex("^\\s{2,}\\S").containsMatchIn(lines[i]) && !item.containsMatchIn(lines[i])) entry += " " + lines[i++].trim()
                        items += entry
                    }
                    out += Block.Items(ordered, items)
                }
                line.trimStart().startsWith(">") -> {
                    val quote = mutableListOf<String>()
                    while (i < lines.size && lines[i].trimStart().startsWith(">")) quote += lines[i++].trimStart().removePrefix(">").removePrefix(" ")
                    out += Block.Quote(quote.joinToString("\n"))
                }
                Regex("^\\s*(-{3,}|\\*{3,})\\s*$").matches(line) -> { out += Block.Rule; i++ }
                line.isBlank() -> i++
                else -> {
                    val para = mutableListOf<String>()
                    while (i < lines.size && lines[i].isNotBlank() && !Regex("^\\s*(```|#{1,6}\\s|[-*•]\\s|\\d+[.)]\\s|>)").containsMatchIn(lines[i]) && !tableAt(lines, i)) para += lines[i++]
                    if (para.isEmpty()) para += lines[i++]
                    out += Block.Paragraph(para.joinToString("\n"))
                }
            }
        }
        return out
    }

    private val inline = Regex("(`[^`\\n]+`)|(\\*\\*[^*\\n]+?\\*\\*)|(\\*[^*\\n]+?\\*|\\b_[^_\\n]+?_\\b)|(\\[[^\\]\\n]+\\]\\([^)\\s]+\\))|((?:https?://|www\\.)[^\\s<>()]+[^\\s<>().,;:!?'\"»])")

    private val links = TextLinkStyles(SpanStyle(color = Bip.accent, textDecoration = TextDecoration.Underline))

    fun inline(text: String): AnnotatedString = buildAnnotatedString { appendInline(text) }

    private fun AnnotatedString.Builder.appendInline(text: String) {
        var last = 0
        for (m in inline.findAll(text)) {
            append(text.substring(last, m.range.first))
            val (code, bold, italic, md, bare) = m.destructured
            when {
                code.isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = Bip.line)) { append(code.trim('`')) }
                bold.isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.ExtraBold)) { appendInline(bold.removeSurrounding("**")) }
                italic.isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { appendInline(italic.substring(1, italic.length - 1)) }
                md.isNotEmpty() -> {
                    val split = md.lastIndexOf("](")
                    link(md.substring(1, split), md.substring(split + 2, md.length - 1))
                }
                else -> link(bare, bare)
            }
            last = m.range.last + 1
        }
        append(text.substring(last))
    }

    private fun AnnotatedString.Builder.link(label: String, raw: String) {
        val uri = ChatText.safeLink(raw) ?: return append(label)
        withLink(LinkAnnotation.Url(uri.toString(), links)) { append(ChatText.label(label, uri)) }
    }
}

/** A reply, rendered. */
@Composable
fun MarkdownText(text: String, modifier: Modifier = Modifier, style: TextStyle = Bip.text(16, lineHeight = 22.sp)) {
    val blocks = remember(text) { Markdown.blocks(text) }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (block in blocks) when (block) {
            is Block.Paragraph -> Text(Markdown.inline(block.text), style = style)
            is Block.Heading -> Text(Markdown.inline(block.text), style = style.copy(fontWeight = FontWeight.ExtraBold,
                fontSize = (if (block.level <= 1) 20 else if (block.level == 2) 18 else 16).sp))
            is Block.Items -> Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                block.items.forEachIndexed { n, entry ->
                    Row {
                        Text(if (block.ordered) "${n + 1}." else "•", style = style, modifier = Modifier.width(22.dp))
                        Text(Markdown.inline(entry), style = style)
                    }
                }
            }
            is Block.Code -> Text(block.text, style = style.copy(fontFamily = FontFamily.Monospace, fontSize = 13.sp),
                modifier = Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(12.dp))
                    .border(1.dp, Bip.line, RoundedCornerShape(12.dp)).horizontalScroll(rememberScrollState()).padding(12.dp))
            is Block.Quote -> Row {
                Box(Modifier.width(3.dp).background(Bip.line))
                Text(Markdown.inline(block.text), style = style.copy(color = Bip.ink2), modifier = Modifier.padding(start = 10.dp))
            }
            is Block.Table -> ChatTable(block, style)
            Block.Rule -> HorizontalDivider(color = Bip.line)
        }
    }
}

/** A table as a grid: each column as wide as its content up to a cap, rows as tall as their tallest cell. */
@Composable
private fun ChatTable(table: Block.Table, style: TextStyle) {
    val columns = table.header.size
    val cellStyle = style.copy(fontSize = 14.sp)
    val shape = RoundedCornerShape(12.dp)
    Box(Modifier.horizontalScroll(rememberScrollState()).background(Bip.line, shape).border(1.dp, Bip.line, shape)
        .clip(shape)) {
        Layout(content = {
            table.header.forEachIndexed { c, h ->
                Text(Markdown.inline(h), style = cellStyle.copy(fontWeight = FontWeight.ExtraBold, color = Bip.ink2,
                    textAlign = table.align.getOrElse(c) { TextAlign.Start }), modifier = Modifier.background(Bip.ground).padding(horizontal = 12.dp, vertical = 8.dp))
            }
            for (row in table.rows) for (c in 0 until columns) {
                Text(Markdown.inline(row.getOrElse(c) { "" }), style = cellStyle.copy(textAlign = table.align.getOrElse(c) { TextAlign.Start }),
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
