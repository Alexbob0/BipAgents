package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material3.Icon
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.InboxItem
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope

/** The Inbox (InboxView.swift): what the agents sent on their own — scheduled tasks' reports, replies that came while
 *  away — from every bridge, newest first. */
@Composable
fun InboxScreen(store: AgentStore, onOpen: (String, String) -> Unit) {
    val agents by store.agents.collectAsState()
    val context = LocalContext.current
    var items by remember { mutableStateOf<List<Pair<InboxItem, Agent?>>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // Messages removed from the Boîte on this phone (the bridge keeps them for its retention), as on the iPhone.
    val prefs = remember { context.getSharedPreferences("inbox", android.content.Context.MODE_PRIVATE) }
    var dismissed by remember { mutableStateOf(prefs.getStringSet("dismissed", emptySet())!!.toSet()) }
    fun dismiss(ids: Collection<String>) {
        dismissed = dismissed + ids
        prefs.edit().putStringSet("dismissed", dismissed).apply()
    }
    var confirmingClear by remember { mutableStateOf(false) }
    LaunchedEffect(agents) {
        try {
            items = coroutineScope {
                store.bridges().map { (bridge, mine) ->
                    async { bridge.inbox().map { item -> item to mine.firstOrNull { it.bridgeName == item.agent } } }
                }.awaitAll().flatten().sortedByDescending { it.first.time ?: 0 }
            }
        } catch (e: Throwable) {
            error = describe(context, e)
            items = emptyList()
        }
    }
    val visible = (items ?: emptyList()).filter { it.first.id !in dismissed }
    if (confirmingClear) androidx.compose.material3.AlertDialog(onDismissRequest = { confirmingClear = false },
        title = { Text(stringResource(R.string.clear_all_title)) },
        text = { Text(stringResource(R.string.clear_all_body)) },
        confirmButton = { androidx.compose.material3.TextButton({ dismiss(visible.map { it.first.id }); confirmingClear = false }) {
            Text(stringResource(R.string.clear_all), color = Bip.danger) } },
        dismissButton = { androidx.compose.material3.TextButton({ confirmingClear = false }) { Text(stringResource(R.string.cancel)) } })
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 120.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        item {
            Row(Modifier.statusBarsPadding().padding(top = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.tab_inbox), style = Bip.text(36, FontWeight.Black), modifier = Modifier.weight(1f))
                if (visible.isNotEmpty()) RoundButton(Icons.Rounded.DeleteOutline, stringResource(R.string.clear_all), { confirmingClear = true }, size = 48.dp)
            }
        }
        error?.let { item { Text(it, style = Bip.text(15, FontWeight.Bold, Bip.danger)) } }
        if (items == null) item { Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted)) }
        if (items != null && visible.isEmpty() && error == null) item { Text(stringResource(R.string.inbox_empty), style = Bip.text(16, color = Bip.ink2)) }
        items(visible, key = { it.first.id }) { (item, agent) ->
            Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(22.dp)).padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    agent?.let { Mascot(it.look, Modifier.size(36.dp), animated = false) }
                    Column(Modifier.weight(1f)) {
                        Text(item.title ?: agent?.name ?: item.agent, style = Bip.text(16, FontWeight.ExtraBold))
                        Text(listOfNotNull(agent?.name?.takeIf { item.title != null }, relative(item.time)).joinToString(" · "),
                            style = Bip.text(13, FontWeight.Bold, Bip.muted))
                    }
                    Icon(Icons.Rounded.Close, stringResource(R.string.remove), Modifier.size(28.dp).background(Bip.ground, CircleShape)
                        .tap { dismiss(listOf(item.id)) }.padding(5.dp), tint = Bip.muted)
                }
                MarkdownText(item.text, style = Bip.text(15, lineHeight = androidx.compose.ui.unit.TextUnit(21f, androidx.compose.ui.unit.TextUnitType.Sp)))
                if (item.session != null && agent != null) Pill(stringResource(R.string.open_conversation), dark = false) { onOpen(agent.id, item.session) }
            }
        }
    }
}
