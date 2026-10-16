package io.github.bipagents.ui

import android.content.Context
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Reply
import androidx.compose.material.icons.automirrored.rounded.Send
import androidx.compose.material.icons.rounded.Build
import androidx.compose.material.icons.rounded.CalendarMonth
import androidx.compose.material.icons.rounded.Cancel
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Groups
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.Lightbulb
import androidx.compose.material.icons.rounded.Psychology
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Terminal
import androidx.compose.material.icons.rounded.VisibilityOff
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.bipagents.R
import io.github.bipagents.model.Look
import io.github.bipagents.net.ToolStep

/** « Détail du travail »: shown folded under each turn, or hidden altogether (long press on it, or Settings). */
object WorkDetail {
    private fun prefs(context: Context) = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
    fun shown(context: Context) = prefs(context).getBoolean("showsWorkDetail", true)
    fun set(context: Context, shown: Boolean) = prefs(context).edit().putBoolean("showsWorkDetail", shown).apply()
}

@Composable
private fun Chip(icon: ImageVector?, label: String, open: Boolean?, modifier: Modifier = Modifier, leading: @Composable (() -> Unit)? = null) {
    Row(modifier.padding(start = 36.dp).background(Bip.card, RoundedCornerShape(50)).border(1.dp, Bip.line, RoundedCornerShape(50))
        .padding(start = if (leading != null) 6.dp else 12.dp, end = 12.dp).heightIn(min = 30.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
        leading?.invoke()
        icon?.let { Icon(it, null, Modifier.size(14.dp), tint = Bip.ink2) }
        Text(label, style = Bip.text(13, FontWeight.Bold, Bip.ink2))
        if (open != null) Icon(if (open) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, null, Modifier.size(16.dp), tint = Bip.ink2)
    }
}

/** The tools, reasoning and running commentary of a turn, folded into one line; long press hides them everywhere. */
@Composable
fun WorkChip(open: Boolean, toggle: () -> Unit, hide: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Box {
        Chip(Icons.Rounded.Build, stringResource(R.string.work_detail), open, Modifier.pointerInput(Unit) {
            detectTapGestures(onTap = { toggle() }, onLongPress = { menu = true })
        })
        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
            DropdownMenuItem({ Text(stringResource(R.string.hide_work_detail), style = Bip.text(16, FontWeight.Bold)) }, { menu = false; hide() },
                leadingIcon = { Icon(Icons.Rounded.VisibilityOff, null) })
        }
    }
}

@Composable
fun ReasoningChip(text: String) {
    var open by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Chip(Icons.Rounded.Lightbulb, stringResource(R.string.reasoning), null, Modifier.tap { open = !open })
        if (open) Text(text.trim(), style = Bip.text(14, FontWeight.Medium, Bip.ink2), modifier = Modifier.padding(start = 36.dp)
            .background(Bip.card, RoundedCornerShape(16.dp)).padding(12.dp))
    }
}

/** A turn's tool calls: icon, name, what it worked on, status and duration. A long finished series shows two and « N more ». */
@Composable
fun ToolCard(steps: List<ToolStep>, look: Look) {
    var expanded by remember { mutableStateOf(false) }
    val collapsed = !expanded && steps.size > 3 && steps.none { it.status == ToolStep.Status.STARTED }
    val shown = if (collapsed) steps.take(2) else steps
    Column(Modifier.padding(start = 36.dp).fillMaxWidth().background(Bip.card, RoundedCornerShape(20.dp))
        .border(1.dp, Bip.line, RoundedCornerShape(20.dp)).animateContentSize()) {
        shown.forEachIndexed { i, step ->
            if (i > 0) HorizontalDivider(color = Bip.line)
            Row(Modifier.padding(horizontal = 12.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(30.dp).background(look.tint, RoundedCornerShape(10.dp)), contentAlignment = Alignment.Center) {
                    Icon(toolIcon(step.name), null, Modifier.size(17.dp), tint = look.deep)
                }
                Column(Modifier.weight(1f)) {
                    Text(step.name, style = Bip.text(13, FontWeight.Bold).copy(fontFamily = FontFamily.Monospace), maxLines = 1,
                        overflow = TextOverflow.Ellipsis)
                    step.preview?.let(ChatText::toolSummary)?.takeIf { it.isNotEmpty() }?.let {
                        Text(it, style = Bip.text(13, color = Bip.ink2), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
                when (step.status) {
                    ToolStep.Status.STARTED -> CircularProgressIndicator(Modifier.size(16.dp), color = Bip.muted, strokeWidth = 2.dp)
                    ToolStep.Status.DONE -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                        Icon(if (step.error == null) Icons.Rounded.Check else Icons.Rounded.Warning, null, Modifier.size(15.dp),
                            tint = if (step.error == null) look.deep else Bip.danger)
                        step.duration?.let { Text("%.1f s".format(it), style = Bip.text(12, FontWeight.Bold, Bip.muted)) }
                    }
                    ToolStep.Status.FAILED -> Icon(Icons.Rounded.Cancel, null, Modifier.size(17.dp), tint = Bip.danger)
                }
            }
        }
        if (collapsed) {
            HorizontalDivider(color = Bip.line)
            Row(Modifier.fillMaxWidth().tap { expanded = true }.heightIn(min = 36.dp), horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.ExpandMore, null, Modifier.size(16.dp), tint = look.deep)
                Text(stringResource(R.string.more_tools, steps.size - shown.size), style = Bip.text(13, FontWeight.ExtraBold, look.deep))
            }
        }
    }
}

private fun toolIcon(tool: String): ImageVector {
    val t = tool.lowercase()
    return when {
        "message_agent" in t || "peer" in t -> Icons.AutoMirrored.Rounded.Send
        "terminal" in t || "shell" in t -> Icons.Rounded.Terminal
        "web" in t || "search" in t || "browser" in t -> Icons.Rounded.Language
        "memory" in t -> Icons.Rounded.Psychology
        "file" in t || "read" in t || "write" in t -> Icons.Rounded.Description
        "image" in t || "vision" in t -> Icons.Rounded.Image
        "cron" in t || "schedule" in t -> Icons.Rounded.CalendarMonth
        else -> Icons.Rounded.Build
    }
}

/** Options the agent offered, as buttons: a tap sends that option as the answer. */
@Composable
fun QuickReplies(options: List<String>, look: Look, choose: (String) -> Unit) {
    Column(Modifier.padding(start = 36.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (option in options) Row(Modifier.background(look.tint, RoundedCornerShape(18.dp))
            .border(1.dp, look.main.copy(alpha = 0.35f), RoundedCornerShape(18.dp)).tap { choose(option) }
            .padding(horizontal = 14.dp, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Icon(Icons.AutoMirrored.Rounded.Reply, null, Modifier.size(14.dp), tint = look.deep)
            Text(option, style = Bip.text(14, FontWeight.Bold, look.deep), maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** A message from another agent (Hermes Bot Mode), with that agent's Bip. */
@Composable
fun TeammateCard(name: String, message: String, look: Look?, time: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (look != null) Mascot(look, Modifier.size(30.dp), animated = false)
        else Box(Modifier.size(30.dp).background(Bip.card, CircleShape), contentAlignment = Alignment.Center) {
            Icon(Icons.Rounded.Groups, null, Modifier.size(16.dp), tint = Bip.ink2)
        }
        Column(Modifier.weight(1f).background(look?.tint ?: Bip.card, RoundedCornerShape(18.dp)).padding(12.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(Icons.AutoMirrored.Rounded.Send, null, Modifier.size(12.dp), tint = look?.deep ?: Bip.ink2)
                Text(stringResource(R.string.message_from, name), style = Bip.text(13, FontWeight.ExtraBold, look?.deep ?: Bip.ink2),
                    modifier = Modifier.weight(1f))
                if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted))
            }
            SelectionContainer { Text(ChatText.visible(message), style = Bip.text(15)) }
        }
    }
}

/** An exchange with another agent folded into one line: « Exchange with Vie » or « Reply from Wellness ». */
@Composable
fun ExchangeChip(name: String, request: Boolean, look: Look?, open: Boolean, toggle: () -> Unit) {
    Chip(if (look == null) Icons.Rounded.Groups else null,
        stringResource(if (request) R.string.exchange_with else R.string.reply_from, name), open, Modifier.tap(toggle),
        leading = look?.let { { Mascot(it, Modifier.size(20.dp), animated = false) } })
}

/** A long prompt the person did not type (a scheduled task's brief, skill instructions), folded. */
@Composable
fun InstructionCard(title: String, text: String, look: Look) {
    var open by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
        Column(Modifier.widthIn(max = 320.dp).background(Bip.card, RoundedCornerShape(20.dp)).border(1.dp, Bip.line, RoundedCornerShape(20.dp))
            .padding(12.dp).animateContentSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(Modifier.tap { open = !open }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(30.dp).background(look.tint, RoundedCornerShape(10.dp)), contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Description, null, Modifier.size(16.dp), tint = look.deep)
                }
                Column(Modifier.weight(1f, fill = false)) {
                    Text(title, style = Bip.text(14, FontWeight.ExtraBold), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(stringResource(if (open) R.string.tap_to_fold else R.string.tap_to_show), style = Bip.text(12, color = Bip.muted))
                }
                Spacer(Modifier.size(4.dp))
                Icon(if (open) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, null, Modifier.size(18.dp), tint = Bip.muted)
            }
            if (open) SelectionContainer(Modifier.heightIn(max = 320.dp).verticalScroll(rememberScrollState())) {
                Text(text, style = Bip.text(12, color = Bip.ink2).copy(fontFamily = FontFamily.Monospace, lineHeight = 16.sp))
            }
        }
    }
}

/** The label above a scheduled task's report: its name (« Podcast du matin · Oct 06 07:53 » → « Podcast du matin »). */
@Composable
fun scheduledLabel(title: String?, session: String?): String =
    title?.substringBefore(" · ")?.trim()?.ifEmpty { null }
        ?: stringResource(if (session?.startsWith("cron_") == true) R.string.scheduled_task else R.string.proactive_message)

@Composable
fun ScheduledLabel(label: String, look: Look) {
    Row(Modifier.padding(start = 36.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
        Icon(Icons.Rounded.Schedule, null, Modifier.size(14.dp), tint = look.deep)
        Text(label, style = Bip.text(13, FontWeight.ExtraBold, look.deep))
    }
}

/** The Bip thinking: its dots, or what it said it is doing (« Je regarde ton agenda… »). */
@Composable
fun Typing(look: Look, interim: String?) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        Mascot(look, Modifier.size(28.dp), Mood.THINKING)
        if (interim.isNullOrBlank()) Text("•••", style = Bip.text(20, FontWeight.Black, Bip.muted))
        else Text(interim, style = Bip.text(15, FontWeight.SemiBold, Bip.muted).copy(fontStyle = androidx.compose.ui.text.font.FontStyle.Italic),
            maxLines = 3, overflow = TextOverflow.Ellipsis)
    }
}
