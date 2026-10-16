package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.rounded.EditNote
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.Session

/** An agent's page (SessionsView.swift): its Bip big on its colour, Live and New, then its conversations — the main
 *  thread, the others, then scheduled tasks (folded). */
@Composable
fun AgentScreen(store: AgentStore, agentId: String, onBack: () -> Unit, onOpen: (String?) -> Unit, onLive: () -> Unit) {
    val agent = store.agent(agentId) ?: return
    val context = LocalContext.current
    val online by store.online.collectAsState()
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
    val look = agent.look
    val all = sessions ?: emptyList()
    val talks = all.filter { !it.isBotChat && !it.isScheduled }.sortedByDescending { it.updated ?: 0 }
    val tasks = all.filter { it.isScheduled }.sortedByDescending { it.updated ?: 0 }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 120.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item {
            Column(Modifier.fillMaxWidth().background(look.tint, RoundedCornerShape(bottomStart = 36.dp, bottomEnd = 36.dp))
                .statusBarsPadding().padding(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 24.dp),
                horizontalAlignment = Alignment.CenterHorizontally) {
                Row(Modifier.fillMaxWidth()) {
                    RoundButton(Icons.AutoMirrored.Rounded.ArrowBack, "Back", onBack, size = 48.dp)
                    Spacer(Modifier.weight(1f))
                    RoundButton(Icons.Rounded.EditNote, stringResource(R.string.new_conversation), { onOpen(null) }, size = 48.dp)
                }
                InteractiveMascot(look, 132.dp, Modifier.padding(top = 12.dp))
                Text(agent.name, style = Bip.text(30, FontWeight.Black), modifier = Modifier.padding(top = 4.dp))
                Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Chip(stringResource(categoryLabel(look.category)), look.main, look.deep, Bip.card)
                    when (online[agent.id]) {
                        true -> Chip(stringResource(R.string.online), Bip.online)
                        false -> Chip(stringResource(R.string.offline), Bip.muted)
                        null -> {}
                    }
                }
                Row(Modifier.padding(top = 16.dp).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    PageButton(stringResource(R.string.live), Icons.Rounded.GraphicEq, dark = true, Modifier.weight(1f), onLive)
                    PageButton(stringResource(R.string.new_short), Icons.Rounded.EditNote, dark = false, Modifier.weight(1f)) { onOpen(null) }
                }
            }
        }
        error?.let { item { Text(it, style = Bip.text(15, FontWeight.Bold, Bip.danger), modifier = Modifier.padding(horizontal = 18.dp)) } }
        if (sessions == null && error == null) item { Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted), modifier = Modifier.padding(18.dp)) }
        items(all.filter { it.isBotChat }, key = { it.id }) { SessionRow(it, onOpen) }
        if (talks.isNotEmpty()) item { Header(stringResource(R.string.conversations)) }
        items(talks, key = { it.id }) { SessionRow(it, onOpen) }
        if (tasks.isNotEmpty()) item {
            Text((if (showTasks) "▾ " else "▸ ") + stringResource(R.string.scheduled_tasks) + " (${tasks.size})",
                style = Bip.text(14, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.tap { showTasks = !showTasks }.padding(horizontal = 22.dp, vertical = 8.dp))
        }
        if (showTasks) items(tasks, key = { it.id }) { SessionRow(it, onOpen) }
    }
}

@Composable
private fun PageButton(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, dark: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Row(modifier.height(52.dp).background(if (dark) Bip.ink else Bip.card, RoundedCornerShape(26.dp)).tap(onClick),
        horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, Modifier.size(22.dp), tint = if (dark) Bip.card else Bip.ink)
        Spacer(Modifier.size(8.dp))
        Text(label, style = Bip.text(17, FontWeight.ExtraBold, if (dark) Bip.card else Bip.ink))
    }
}

@Composable
private fun Header(text: String) =
    Text(text.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.padding(top = 10.dp, start = 22.dp))

@Composable
private fun SessionRow(session: Session, onOpen: (String?) -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp).fillMaxWidth().background(Bip.card, RoundedCornerShape(16.dp)).tap { onOpen(session.id) }.padding(14.dp)) {
        Row {
            Text(if (session.isBotChat) stringResource(R.string.main_thread) else session.title ?: stringResource(R.string.untitled),
                style = Bip.text(16, FontWeight.ExtraBold), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
            Text(relative(session.updated), style = Bip.text(13, FontWeight.Bold, Bip.muted))
        }
        session.preview?.let { Text(ChatText.preview(it), style = Bip.text(14, color = Bip.ink2), maxLines = 2, overflow = TextOverflow.Ellipsis) }
    }
}
