package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Reply
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.NotificationsOff
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.BipApp
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Inbox
import io.github.bipagents.voice.Speaker
import kotlinx.coroutines.launch

/** The Inbox (InboxView.swift): approvals waiting first, then what the agents sent on their own — scheduled tasks'
 *  reports, replies finished while away — newest first, by agent if wanted. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InboxScreen(store: AgentStore, onOpen: (agent: String, session: String?) -> Unit, onLive: (String) -> Unit) {
    val context = LocalContext.current
    val inbox = (context.applicationContext as BipApp).inbox
    val agents by store.agents.collectAsState()
    val state by inbox.state.collectAsState()
    val scope = rememberCoroutineScope()
    var filter by remember { mutableStateOf<String?>(null) }
    var confirmingClear by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    // The unread marks stay while the Inbox is open; leaving it marks everything seen.
    val seen = remember { state.lastSeen }
    LaunchedEffect(agents) { inbox.refresh(store) }
    DisposableEffect(Unit) { onDispose { inbox.markAllSeen() } }
    val visible = state.entries.filter { filter == null || it.agent.id == filter }
    if (confirmingClear) androidx.compose.material3.AlertDialog(onDismissRequest = { confirmingClear = false },
        title = { Text(stringResource(R.string.clear_all_title)) },
        text = { Text(stringResource(R.string.clear_all_body)) },
        confirmButton = { androidx.compose.material3.TextButton({ inbox.dismissAll(store); confirmingClear = false }) {
            Text(stringResource(R.string.clear_all), color = Bip.danger) } },
        dismissButton = { androidx.compose.material3.TextButton({ confirmingClear = false }) { Text(stringResource(R.string.cancel)) } })
    val unreachable = stringResource(R.string.some_unreachable)
    val mutedFormat = stringResource(R.string.muted_notice)
    val thisTask = stringResource(R.string.this_task)
    val muteFailed = stringResource(R.string.mute_failed)
    PullToRefreshBox(state.loading, { scope.launch { inbox.refresh(store) } }, Modifier.fillMaxSize()) {
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 180.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp)) {
            item {
                Row(Modifier.statusBarsPadding().padding(top = 18.dp), verticalAlignment = Alignment.Bottom) {
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.inbox_overline), style = Bip.text(15, FontWeight.Bold, Bip.ink2))
                        Text(stringResource(R.string.tab_inbox), style = Bip.text(36, FontWeight.Black))
                    }
                    if (state.entries.isNotEmpty()) RoundButton(Icons.Rounded.DeleteOutline, stringResource(R.string.clear_all), { confirmingClear = true }, size = 48.dp)
                }
            }
            if (agents.size > 1) item {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip(stringResource(R.string.filter_all), null, filter == null) { filter = null }
                    for (agent in agents) FilterChip(agent.name, agent.look.main, filter == agent.id) { filter = agent.id }
                }
            }
            if (state.unreachable) item { Text(unreachable, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
            listOfNotNull(error, notice).forEach { line -> item { Text(line, style = Bip.text(14, FontWeight.Bold, Bip.ink2)) } }
            items(state.approvals.filter { filter == null || it.agent.id == filter }, key = { it.id }) { pending ->
                ApprovalCard(pending.agent.look, pending.agent.name, pending.request) { choice ->
                    scope.launch {
                        try {
                            store.hermes(pending.agent)?.approve(pending.request.run, choice, pending.request.requestId)
                            inbox.resolved(pending)
                        } catch (e: Throwable) {
                            error = describe(context, e)
                            inbox.refresh(store)   // already answered or expired: what is left
                        }
                    }
                }
            }
            if (visible.isEmpty() && state.approvals.isEmpty() && !state.loading) item {
                Column(Modifier.fillMaxWidth().padding(top = 60.dp), horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Mascot(io.github.bipagents.model.Look.of(io.github.bipagents.model.Category.DAILY), Modifier.size(120.dp), Mood.SLEEPING)
                    Text(stringResource(R.string.nothing_new), style = Bip.text(20, FontWeight.ExtraBold))
                    Text(stringResource(R.string.inbox_empty), style = Bip.text(15, color = Bip.ink2), textAlign = TextAlign.Center)
                }
            }
            items(visible, key = { it.id }) { entry ->
                InboxCard(entry, unread = (entry.item.time ?: 0) > seen,
                    reply = { onOpen(entry.agent.id, entry.item.session) }, live = { onLive(entry.agent.id) },
                    dismiss = { inbox.dismiss(entry, store) },
                    mute = Inbox.cronJob(entry.item.session)?.let { job ->
                        {
                            scope.launch {
                                notice = try {
                                    store.bridge(entry.agent)!!.setCronNotify(entry.agent.bridgeName, job, false)
                                    mutedFormat.format(entry.item.title?.substringBefore(" · ")?.ifBlank { null } ?: thisTask)
                                } catch (e: Throwable) { muteFailed }
                            }
                        }
                    })
            }
        }
    }
}

@Composable
private fun FilterChip(label: String, color: Color?, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.height(36.dp).background(if (selected) Bip.ink else Bip.card, RoundedCornerShape(50)).tap(onClick).padding(horizontal = 15.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
        color?.let { Box(Modifier.size(8.dp).background(it, CircleShape)) }
        Text(label, style = Bip.text(14, FontWeight.ExtraBold, if (selected) Bip.card else Bip.ink))
    }
}

/** One message: the agent, its time and task, an unread dot, ✕; the text; its audio; Reply and Live. A tap opens the
 *  conversation; a long press copies, shares, mutes the task or deletes. */
@Composable
private fun InboxCard(entry: Inbox.Entry, unread: Boolean, reply: () -> Unit, live: () -> Unit, dismiss: () -> Unit, mute: (() -> Unit)?) {
    val context = LocalContext.current
    val look = entry.agent.look
    var menu by remember { mutableStateOf(false) }
    Box {
        Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(26.dp))
            .pointerInput(entry.id) { detectTapGestures(onTap = { reply() }, onLongPress = { menu = true }) }.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size(38.dp).background(look.tint, RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
                    Mascot(look, Modifier.size(32.dp), animated = false)
                }
                Column(Modifier.weight(1f)) {
                    Text(entry.agent.name, style = Bip.text(14, FontWeight.Black))
                    Text(listOfNotNull(clock(entry.item.time).ifEmpty { null },
                        entry.item.title ?: if (entry.item.id.startsWith("reply-")) stringResource(R.string.reply_title) else null).joinToString(" · "),
                        style = Bip.text(13, FontWeight.Bold, Bip.muted), maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                if (unread) Box(Modifier.size(10.dp).background(Bip.danger, CircleShape))
                Box(Modifier.size(30.dp).background(Bip.ground, CircleShape).tap(dismiss), contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Close, stringResource(R.string.remove), Modifier.size(15.dp), tint = Bip.muted)
                }
            }
            Text(ShareText.plain(entry.item.text), style = Bip.text(16), maxLines = 8, overflow = TextOverflow.Ellipsis)
            if (entry.item.hasAudio) {
                val now by Speaker.now.collectAsState()
                val key = "outbox-${entry.item.id}"
                val playing = now?.key == key
                Row(Modifier.fillMaxWidth().background(look.tint, RoundedCornerShape(18.dp)).tap { OutboxAudio.toggle(context, entry) }.padding(8.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Box(Modifier.size(34.dp).background(look.deep, CircleShape), contentAlignment = Alignment.Center) {
                        Icon(if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow, null, Modifier.size(18.dp), tint = Color.White)
                    }
                    Text(stringResource(if (playing) R.string.playing else R.string.listen), style = Bip.text(14, FontWeight.ExtraBold, look.deep))
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PillButton(stringResource(R.string.reply), Icons.AutoMirrored.Rounded.Reply, Bip.ink, Bip.card, reply)
                PillButton(stringResource(R.string.live), Icons.Rounded.GraphicEq, Bip.ground, Bip.ink, live)
            }
        }
        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
            @Composable fun Entry(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, color: Color = Bip.ink, action: () -> Unit) =
                DropdownMenuItem({ Text(label, style = Bip.text(16, FontWeight.Bold, color)) }, { menu = false; action() },
                    leadingIcon = { Icon(icon, null, tint = color) })
            Entry(stringResource(R.string.copy), Icons.Rounded.ContentCopy) { ShareText.copy(context, entry.item.text) }
            Entry(stringResource(R.string.share), Icons.Rounded.Share) { ShareText.share(context, entry.item.text) }
            if (mute != null) Entry(stringResource(R.string.mute_task), Icons.Rounded.NotificationsOff, action = mute)
            Entry(stringResource(R.string.delete), Icons.Rounded.DeleteOutline, Bip.danger, dismiss)
        }
    }
}

@Composable
private fun PillButton(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, background: Color, color: Color, onClick: () -> Unit) {
    Row(Modifier.height(42.dp).background(background, RoundedCornerShape(50)).tap(onClick).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
        Icon(icon, null, Modifier.size(17.dp), tint = color)
        Text(label, style = Bip.text(15, FontWeight.ExtraBold, color))
    }
}

/** An outbox message's audio (a podcast, or its synthesized voice), kept on the phone. */
object OutboxAudio {
    fun toggle(context: android.content.Context, entry: Inbox.Entry) {
        val key = "outbox-${entry.item.id}"
        if (Speaker.now.value?.key == key) return Speaker.stop()
        val store = (context.applicationContext as BipApp).store
        val bridge = store.bridge(entry.agent) ?: return
        val file = java.io.File(context.cacheDir, "outbox").apply { mkdirs() }.resolve(entry.item.id.hashCode().toUInt().toString(16) + ".mp3")
        kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Main).launch {
            runCatching {
                if (!file.exists()) {
                    val bytes = bridge.outboxAudio(entry.item.id)
                    kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { file.writeBytes(bytes) }
                }
                Speaker.play(key, file)
            }.onFailure { android.widget.Toast.makeText(context, describe(context, it), android.widget.Toast.LENGTH_SHORT).show() }
        }
    }
}
