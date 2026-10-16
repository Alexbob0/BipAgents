package io.github.bipagents.ui

import androidx.compose.foundation.background
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.BuildConfig
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore

/** Settings: the agents (add, remove) and the app's version. More comes with the next phases. */
@Composable
fun SettingsScreen(store: AgentStore, onAdd: () -> Unit) {
    val agents by store.agents.collectAsState()
    val online by store.online.collectAsState()
    var removing by remember { mutableStateOf<Agent?>(null) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 120.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item {
            Column(Modifier.statusBarsPadding().padding(top = 18.dp, bottom = 6.dp)) {
                Text("BipAgents ${BuildConfig.VERSION_NAME}", style = Bip.text(16, FontWeight.Bold, Bip.ink2))
                Text(stringResource(R.string.tab_settings), style = Bip.text(36, FontWeight.Black))
            }
        }
        item { Text(stringResource(R.string.agents).uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted)) }
        items(agents, key = { it.id }) { agent ->
            Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(12.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Mascot(agent.look, Modifier.size(40.dp), animated = false)
                Column(Modifier.weight(1f)) {
                    Text(agent.name, style = Bip.text(16, FontWeight.ExtraBold))
                    Text(android.net.Uri.parse(agent.baseUrl).host ?: agent.baseUrl, style = Bip.text(13, color = Bip.muted),
                        maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                }
                when (online[agent.id]) {
                    true -> Chip(stringResource(R.string.online), Bip.online)
                    false -> Chip(stringResource(R.string.offline), Bip.muted)
                    null -> {}
                }
                Icon(Icons.Rounded.DeleteOutline, stringResource(R.string.remove), tint = Bip.muted, modifier = Modifier.tap { removing = agent })
            }
        }
        item {
            Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap(onAdd).padding(14.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(Icons.Rounded.Add, null, tint = Bip.accent)
                Text(stringResource(R.string.add_agent), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent))
            }
        }
    }
    removing?.let { agent ->
        AlertDialog(onDismissRequest = { removing = null },
            title = { Text(stringResource(R.string.remove_title, agent.name)) },
            text = { Text(stringResource(R.string.remove_body)) },
            confirmButton = { TextButton({ store.remove(agent); removing = null }) { Text(stringResource(R.string.remove), color = Bip.danger) } },
            dismissButton = { TextButton({ removing = null }) { Text(stringResource(R.string.cancel)) } })
    }
}
