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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.Session

/** An agent's conversations (SessionsView.swift): its main thread, the others, then scheduled tasks (folded). */
@Composable
fun SessionsScreen(store: AgentStore, agentId: String, onBack: () -> Unit, onOpen: (String?) -> Unit) {
    val agent = store.agent(agentId) ?: return
    val context = LocalContext.current
    var sessions by remember { mutableStateOf<List<Session>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var showTasks by remember { mutableStateOf(false) }
    LaunchedEffect(agentId) {
        try {
            sessions = store.hermes(agent)?.allSessions(4) { all -> all.count { !it.isScheduled } >= 30 }
        } catch (e: Throwable) {
            error = describe(context, e)
        }
    }
    val all = sessions ?: emptyList()
    val talks = all.filter { !it.isBotChat && !it.isScheduled }.sortedByDescending { it.updated ?: 0 }
    val tasks = all.filter { it.isScheduled }.sortedByDescending { it.updated ?: 0 }

    Column(Modifier.fillMaxSize()) {
        TopBar(agent.name, onBack, leading = { Mascot(agent.look, Modifier.size(40.dp)) },
            actions = { RoundButton(Icons.Rounded.Add, stringResource(R.string.new_conversation), { onOpen(null) }, size = 44.dp) })
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            error?.let { item { Text(it, style = Bip.text(15, FontWeight.Bold, Bip.danger)) } }
            if (sessions == null && error == null) item { Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted)) }
            items(all.filter { it.isBotChat }, key = { it.id }) { Row(it, onOpen) }
            if (talks.isNotEmpty()) item { Header(stringResource(R.string.conversations)) }
            items(talks, key = { it.id }) { Row(it, onOpen) }
            if (tasks.isNotEmpty()) item {
                Text((if (showTasks) "▾ " else "▸ ") + stringResource(R.string.scheduled_tasks) + " (${tasks.size})",
                    style = Bip.text(14, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.tap { showTasks = !showTasks }.padding(vertical = 8.dp))
            }
            if (showTasks) items(tasks, key = { it.id }) { Row(it, onOpen) }
        }
    }
}

@Composable
private fun Header(text: String) =
    Text(text.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.padding(top = 10.dp, start = 4.dp))

@Composable
private fun Row(session: Session, onOpen: (String?) -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(16.dp)).tap { onOpen(session.id) }.padding(14.dp)) {
        Row {
            Text(if (session.isBotChat) stringResource(R.string.main_thread) else session.title ?: stringResource(R.string.untitled),
                style = Bip.text(16, FontWeight.ExtraBold), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Text(relative(session.updated), style = Bip.text(13, FontWeight.Bold, Bip.muted))
        }
        session.preview?.let { Text(ChatText.preview(it), style = Bip.text(14, color = Bip.ink2), maxLines = 2, overflow = TextOverflow.Ellipsis) }
    }
}
