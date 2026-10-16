package io.github.bipagents.ui

import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Forward
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material.icons.rounded.TextFields
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import io.github.bipagents.BipApp
import io.github.bipagents.R

/** The saved unsent text of a conversation (kept across leaving it); also how a forwarded message reaches the composer
 *  of another agent's conversation. `session`: the conversation opened (null: the agent's ongoing one, "" a new one). */
object Drafts {
    private fun prefs(context: Context) = context.getSharedPreferences("drafts", Context.MODE_PRIVATE)
    private fun key(agent: String, session: String?) = "draft.$agent." + when (session) { null -> "main"; "" -> "new"; else -> session }
    fun get(context: Context, agent: String, session: String?): String = prefs(context).getString(key(agent, session), "") ?: ""
    fun set(context: Context, agent: String, session: String?, text: String) {
        prefs(context).edit().apply { if (text.isBlank()) remove(key(agent, session)) else putString(key(agent, session), text) }.apply()
    }
}

/** Long press on a message, as in messaging apps: copy, share, select a passage, forward to another agent. */
@Composable
fun MessageActions(text: String, agentId: String, onForward: (agent: String, session: String?) -> Unit, modifier: Modifier = Modifier,
                   content: @Composable () -> Unit) {
    val context = LocalContext.current
    val haptics = LocalHapticFeedback.current
    var menu by remember { mutableStateOf(false) }
    var forwarding by remember { mutableStateOf(false) }
    var selecting by remember { mutableStateOf(false) }
    Box(modifier.pointerInput(text) {
        detectTapGestures(onLongPress = {
            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
            forwarding = false
            menu = true
        })
    }) {
        content()
        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
            if (!forwarding) {
                val copied = stringResource(R.string.copied)
                Item(stringResource(R.string.copy), Icons.Rounded.ContentCopy) {
                    menu = false
                    ShareText.copy(context, text)
                    // Android 13+ shows its own confirmation.
                    if (android.os.Build.VERSION.SDK_INT < 33) Toast.makeText(context, copied, Toast.LENGTH_SHORT).show()
                }
                Item(stringResource(R.string.share), Icons.Rounded.Share) { menu = false; ShareText.share(context, text) }
                Item(stringResource(R.string.select_text), Icons.Rounded.TextFields) { menu = false; selecting = true }
                val others = (context.applicationContext as BipApp).store.agents.collectAsState().value.filter { it.id != agentId }
                if (others.isNotEmpty()) Item(stringResource(R.string.forward_to), Icons.AutoMirrored.Rounded.Forward) { forwarding = true }
            } else {
                val others = (context.applicationContext as BipApp).store.agents.collectAsState().value.filter { it.id != agentId }
                for (target in others) DropdownMenuItem({ Text(target.name, style = Bip.text(16, FontWeight.Bold)) }, {
                    menu = false
                    // Into the conversation « Write » opens: the agent's latest one.
                    val session = (context.applicationContext as BipApp).store.latest.value[target.id]?.id
                    Drafts.set(context, target.id, session, ShareText.plain(text))
                    onForward(target.id, session)
                }, leadingIcon = { Mascot(target.look, Modifier.size(28.dp), animated = false) })
            }
        }
    }
    if (selecting) Dialog({ selecting = false }) {
        Column(Modifier.fillMaxWidth().heightIn(max = 560.dp).background(Bip.card, RoundedCornerShape(26.dp)).padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp)) {
            SelectionContainer(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                Text(ShareText.plain(text), style = Bip.text(17, FontWeight.Medium))
            }
            Pill(stringResource(R.string.done), dark = true) { selecting = false }
        }
    }
}

@Composable
private fun Item(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, onClick: () -> Unit) {
    DropdownMenuItem({ Text(label, style = Bip.text(16, FontWeight.Bold)) }, onClick, leadingIcon = { Icon(icon, null) })
}

/** « Aujourd'hui », « Hier », « lundi 5 octobre » (ThreadDecorations' DaySeparator). */
@Composable
fun DaySeparator(time: Long) {
    val label = dayLabel(LocalContext.current, time)
    Box(Modifier.fillMaxWidth().padding(vertical = 4.dp), contentAlignment = androidx.compose.ui.Alignment.Center) {
        Text(label, style = Bip.text(13, FontWeight.ExtraBold, Bip.muted),
            modifier = Modifier.background(Bip.card, RoundedCornerShape(50)).padding(horizontal = 12.dp, vertical = 4.dp))
    }
}

fun dayOf(time: Long): Long {
    val c = java.util.Calendar.getInstance().apply { timeInMillis = time }
    return c.get(java.util.Calendar.YEAR) * 1000L + c.get(java.util.Calendar.DAY_OF_YEAR)
}

fun dayLabel(context: Context, time: Long): String {
    val now = System.currentTimeMillis()
    return when (dayOf(time)) {
        dayOf(now) -> context.getString(R.string.today)
        dayOf(now - 86_400_000) -> context.getString(R.string.yesterday)
        else -> {
            val locale = context.resources.configuration.locales[0]
            val sameYear = java.util.Calendar.getInstance().apply { timeInMillis = time }.get(java.util.Calendar.YEAR) ==
                java.util.Calendar.getInstance().get(java.util.Calendar.YEAR)
            val pattern = android.text.format.DateFormat.getBestDateTimePattern(locale, if (sameYear) "EEEEdMMMM" else "EEEEdMMMMyyyy")
            java.text.SimpleDateFormat(pattern, locale).format(java.util.Date(time))
        }
    }
}
