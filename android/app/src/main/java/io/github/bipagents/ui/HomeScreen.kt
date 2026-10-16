package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.runtime.setValue
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
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Category
import io.github.bipagents.net.Session
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** Home: one card per agent (App/Features/Agents/AgentsView.swift). */
@Composable
fun HomeScreen(store: AgentStore, onAdd: () -> Unit, onAgent: (String) -> Unit, onChat: (String, String?) -> Unit,
               onLive: (String) -> Unit, onVoice: (String, io.github.bipagents.voice.RecordedNote) -> Unit) {
    val agents by store.agents.collectAsState()
    val latest by store.latest.collectAsState()
    val online by store.online.collectAsState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(agents.map { it.id }) {
        agents.forEach { scope.launch { store.refresh(it) } }
        store.syncLooks()
    }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 120.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp)) {
        item {
            Row(Modifier.statusBarsPadding().padding(top = 18.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(SimpleDateFormat("EEEE d MMMM", androidx.compose.ui.platform.LocalConfiguration.current.locales[0]).format(Date()).replaceFirstChar(Char::uppercase),
                        style = Bip.text(16, FontWeight.Bold, Bip.ink2))
                    Text(stringResource(R.string.home_hello), style = Bip.text(36, FontWeight.Black))
                }
                RoundButton(Icons.Rounded.Add, stringResource(R.string.add_agent), onAdd, size = 52.dp)
            }
        }
        if (agents.isEmpty()) item { Welcome(onAdd) }
        items(agents, key = { it.id }) { agent ->
            AgentCard(agent, latest[agent.id], online[agent.id], onAgent = { onAgent(agent.id) },
                onChat = { onChat(agent.id, latest[agent.id]?.id) }, onLive = { onLive(agent.id) }, onVoice = { onVoice(agent.id, it) })
        }
    }
}

/** One agent (AgentsView's card): its name opens its page, the Bip reacts, the latest message opens that conversation,
 *  and Audio (hold) / Write / Live. */
@Composable
private fun AgentCard(agent: Agent, latest: Session?, online: Boolean?, onAgent: () -> Unit, onChat: () -> Unit,
                      onLive: () -> Unit, onVoice: (io.github.bipagents.voice.RecordedNote) -> Unit) {
    val look = agent.look
    Column(Modifier.fillMaxWidth().background(look.tint, RoundedCornerShape(30.dp)).padding(18.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Column(Modifier.weight(1f).tap(onAgent), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(agent.name, style = Bip.text(25, FontWeight.Black), maxLines = 1, overflow = TextOverflow.Ellipsis)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Chip(stringResource(categoryLabel(look.category)), look.main, look.deep, Bip.card)
                    when (online) {
                        true -> Chip(stringResource(R.string.online), Bip.online)
                        false -> Chip(stringResource(R.string.offline), Bip.muted)
                        null -> {}
                    }
                }
            }
            // Outside the link: tapping the Bip makes it react instead of opening the agent.
            Box(Modifier.size(92.dp, 64.dp), contentAlignment = Alignment.Center) {
                InteractiveMascot(look, 92.dp, Modifier.requiredSize(92.dp), bubbleLeading = true)
            }
        }
        if (latest != null && latest.preview != null) {
            Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap(onChat)
                .padding(horizontal = 14.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Row {
                    Text(if (latest.isBotChat) stringResource(R.string.main_thread) else latest.title ?: stringResource(R.string.last_conversation),
                        style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), maxLines = 1, overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f))
                    Text(relative(latest.updated), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted))
                }
                Text(ChatText.preview(latest.preview), style = Bip.text(15, FontWeight.SemiBold), maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AudioHoldButton(look, agent.language, Modifier.weight(1f), onVoice)
            CardButton(stringResource(R.string.write), Icons.Rounded.Keyboard, dark = false, Modifier.weight(1f), onChat)
            CardButton(stringResource(R.string.live), Icons.Rounded.GraphicEq, dark = true, Modifier.weight(1f), onLive)
        }
    }
}

@Composable
private fun CardButton(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, dark: Boolean, modifier: Modifier, onClick: () -> Unit) {
    Row(modifier.height(52.dp).background(if (dark) Bip.ink else Bip.card, RoundedCornerShape(26.dp)).tap(onClick),
        horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, Modifier.size(22.dp), tint = if (dark) Bip.card else Bip.ink)
        Spacer(Modifier.size(6.dp))
        Text(label, style = Bip.text(17, FontWeight.ExtraBold, if (dark) Bip.card else Bip.ink), maxLines = 1)
    }
}

/** Audio: hold to speak to the agent from home; released, the note goes to its latest conversation. */
@Composable
private fun AudioHoldButton(look: io.github.bipagents.model.Look, language: String?, modifier: Modifier,
                            onVoice: (io.github.bipagents.voice.RecordedNote) -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()
    val recorder = androidx.compose.runtime.remember { io.github.bipagents.voice.Recorder(context.applicationContext) }
    var recording by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    val askMic = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.RequestPermission()) {}
    Row(modifier.height(52.dp).background(if (recording) Bip.danger else Bip.card, RoundedCornerShape(26.dp))
        .pointerInput(Unit) {
            awaitEachGesture {
                awaitFirstDown()
                if (!recorder.allowed) { askMic.launch(android.Manifest.permission.RECORD_AUDIO); return@awaitEachGesture }
                recording = true
                io.github.bipagents.voice.Speaker.stop()
                scope.launch { if (!recorder.start(language)) recording = false }
                do { val event = awaitPointerEvent() } while (event.changes.any { it.pressed })
                recording = false
                scope.launch { recorder.stop()?.let(onVoice) }
            }
        }, horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
        Icon(Icons.Rounded.Mic, null, Modifier.size(22.dp), tint = if (recording) Bip.card else Bip.ink)
        Spacer(Modifier.size(6.dp))
        Text(stringResource(if (recording) R.string.release_to_send else R.string.audio), style = Bip.text(17, FontWeight.ExtraBold,
            if (recording) Bip.card else Bip.ink), maxLines = 1)
    }
}

@Composable
private fun Welcome(onAdd: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(30.dp)).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            listOf(Category.WELLNESS, Category.FINANCE, Category.DAILY, Category.WORK, Category.LEARNING).forEach { InteractiveMascot(io.github.bipagents.model.Look.of(it), 56.dp) }
        }
        Text(stringResource(R.string.welcome_title), style = Bip.text(22, FontWeight.Black))
        Text(stringResource(R.string.welcome_body), style = Bip.text(16, color = Bip.ink2))
        Row(Modifier.background(Bip.ink, RoundedCornerShape(26.dp)).tap(onAdd).padding(horizontal = 22.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.QrCodeScanner, null, tint = Bip.card)
            Spacer(Modifier.size(8.dp))
            Text(stringResource(R.string.scan_qr), style = Bip.text(17, FontWeight.ExtraBold, Bip.card))
        }
    }
}

fun categoryLabel(c: Category): Int = when (c) {
    Category.WELLNESS -> R.string.cat_wellness
    Category.FINANCE -> R.string.cat_finance
    Category.DAILY -> R.string.cat_daily
    Category.WORK -> R.string.cat_work
    Category.LEARNING -> R.string.cat_learning
    Category.HOME -> R.string.cat_home
    Category.CREATIVE -> R.string.cat_creative
    Category.TECH -> R.string.cat_tech
}
