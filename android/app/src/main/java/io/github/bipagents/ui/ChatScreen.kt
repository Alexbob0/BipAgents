package io.github.bipagents.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.Mic
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.ui.input.pointer.pointerInput
import io.github.bipagents.voice.RecordedNote
import io.github.bipagents.voice.Recorder
import io.github.bipagents.voice.Speaker
import androidx.compose.material.icons.rounded.AttachFile
import androidx.compose.material.icons.rounded.Image
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.platform.LocalContext
import io.github.bipagents.model.Attachment
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.heightIn
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.List
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.Build
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Look
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.Spacer
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.Reply
import io.github.bipagents.net.RunEvent

@Composable
fun ChatScreen(store: AgentStore, agentId: String, session: String?, onBack: () -> Unit, onAgent: () -> Unit, onLive: (String?) -> Unit,
               onForward: (agent: String, session: String?) -> Unit) {
    val model: ChatModel = viewModel(key = "chat-$agentId-${session ?: "main"}")
    LaunchedEffect(Unit) {
        model.start(agentId, session)
        PendingVoice.take(agentId)?.let(model::sendVoice)
    }
    val state by model.state.collectAsState()
    val agent = state.agent ?: store.agent(agentId) ?: return
    val context = LocalContext.current
    // Kept across leaving the conversation (and how a forwarded message arrives).
    var draft by remember(agentId, session) { mutableStateOf(Drafts.get(context, agentId, session)) }
    LaunchedEffect(draft) { kotlinx.coroutines.delay(400); Drafts.set(context, agentId, session, draft) }
    val scope = rememberCoroutineScope()
    val attachments = remember { mutableStateListOf<Attachment>() }
    fun take(uris: List<Uri>) = scope.launch {
        uris.forEach { uri -> withContext(Dispatchers.IO) { Attachment.read(context, uri) }?.let(attachments::add) }
    }
    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(5)) { take(it) }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { take(it) }
    val list = rememberLazyListState()
    // The thread sticks to its end (replies streaming, photos loading) until the person scrolls up; then a button
    // brings the end back.
    var stick by remember { mutableStateOf(true) }
    suspend fun toEnd(animated: Boolean = false) {
        val last = list.layoutInfo.totalItemsCount - 1
        if (last < 0) return
        if (animated) list.animateScrollToItem(last, Int.MAX_VALUE) else list.scrollToItem(last, Int.MAX_VALUE)
    }
    LaunchedEffect(list) {
        androidx.compose.runtime.snapshotFlow { list.isScrollInProgress }.collect { scrolling -> if (!scrolling) stick = !list.canScrollForward }
    }
    LaunchedEffect(list) {
        androidx.compose.runtime.snapshotFlow { list.canScrollForward to list.layoutInfo.totalItemsCount }.collect { (more, _) ->
            if (more && stick && !list.isScrollInProgress) toEnd()
        }
    }
    val away = !stick && list.canScrollForward
    // A message just sent: the end of the conversation.
    LaunchedEffect(state.live?.sent) { if (state.live?.sent != null) { stick = true; toEnd(animated = true) } }
    // The latest messages, « Earlier messages » for more (the iPhone's 80 per page).
    var visibleCount by rememberSaveable { mutableStateOf(80) }
    val live = state.live
    val thread = remember(state.items, live) {
        (state.items ?: emptyList()) + listOfNotNull(live?.sent?.let { Item.User("live-sent", it, false, null, live.files) }) + (live?.items ?: emptyList())
    }
    val folds = remember(thread, live != null) { ThreadLayout.workFolds(thread, live != null) }
    val owners = remember(thread) { ThreadLayout.exchangeOwners(thread) }
    var showsWork by remember { mutableStateOf(WorkDetail.shown(context)) }
    var toldWhereWork by remember { mutableStateOf(false) }
    val openWork = remember { mutableStateMapOf<String, Boolean>() }
    val openExchanges = remember { mutableStateMapOf<String, Boolean>() }
    val firstUser = thread.firstOrNull { it is Item.User }?.key
    val cron = state.session?.startsWith("cron_") == true

    /** One line of the thread, as the iPhone draws it (ConversationView.row). */
    @Composable
    fun Line(index: Int, turnEnd: Boolean) {
        when (val item = thread[index]) {
            is Item.User -> {
                val teammate = ChatText.teammate(item.text)
                val instruction = ChatText.instruction(item.text, cron, item.key == firstUser)
                when {
                    teammate != null -> TeammateCard(teammate.name, teammate.body, teammateLook(store, teammate), clock(item.time))
                    instruction != null -> Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        InstructionCard(instructionTitle(instruction), item.text, agent.look)
                        if (instruction is ChatText.Instruction.Subtask) ChatText.mediaPaths(item.text).forEach { path ->
                            Box(Modifier.fillMaxWidth()) { MediaRow(path, agent.look, agent.name, hideIfMissing = true) }
                        }
                        if (item.time != null) Text(clock(item.time), style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(end = 4.dp))
                    }
                    else -> MessageActions(item.text, agent.id, onForward) {
                        model.notes.find(state.session, item.text)?.let { VoiceBubble(it, item.key, clock(item.time)) }
                            ?: UserBubble(item.text, item.image, clock(item.time), item.files)
                    }
                }
            }
            is Item.Reply -> Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                ThreadLayout.answered(thread, index)?.let { name ->
                    Row(Modifier.padding(start = 36.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        androidx.compose.material3.Icon(Icons.AutoMirrored.Rounded.Reply, null, Modifier.size(13.dp), tint = Bip.muted)
                        Text(stringResource(R.string.reply_to, name), style = Bip.text(13, FontWeight.ExtraBold, Bip.muted))
                    }
                }
                MessageActions(item.text, agent.id, onForward) {
                    Reply(agent.look, item.text, if (turnEnd && !item.streaming) clock(item.time) else "", streaming = item.streaming,
                        listen = if (turnEnd && !item.streaming) ({ model.listen(item.key, item.text) }) else null, listenKey = item.key, name = agent.name)
                }
                // The agent asks to pick an option: one tap answers (latest reply only).
                if (!item.streaming && live == null && index == thread.lastIndex) {
                    val options = remember(item.text) { ChatText.choices(item.text) }
                    if (options.isNotEmpty()) QuickReplies(options, agent.look) { model.send(it) }
                }
            }
            is Item.Reasoning -> ReasoningChip(item.text)
            is Item.Tools -> ToolCard(item.steps, agent.look)
            is Item.Scheduled -> Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                val label = scheduledLabel(item.message.title, item.message.session)
                ScheduledLabel(label, agent.look)
                MessageActions(item.message.text, agent.id, onForward) {
                    Reply(agent.look, item.message.text, clock(item.time),
                        listen = if (item.message.hasAudio) ({ model.playScheduled(item.message) }) else ({ model.listen(item.key, item.message.text) }),
                        listenKey = if (item.message.hasAudio) "outbox-${item.message.id}" else item.key, name = agent.name)
                }
            }
        }
    }

    androidx.compose.runtime.CompositionLocalProvider(LocalBridge provides store.bridge(agent)) {
    Column(Modifier.fillMaxSize().imePadding()) {
        // The iPhone's: back on the left, the Bip and its name in the middle (its page), Live on the right.
        Box(Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 6.dp)) {
            RoundButton(Icons.AutoMirrored.Rounded.ArrowBack, "Back", onBack, size = 48.dp)
            Row(Modifier.align(Alignment.Center).tap(onAgent), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Mascot(agent.look, Modifier.size(36.dp), state.mood)
                Text(agent.name, style = Bip.text(17, FontWeight.ExtraBold), maxLines = 1)
            }
            Box(Modifier.align(Alignment.CenterEnd)) {
                RoundButton(Icons.Rounded.GraphicEq, stringResource(R.string.live), { onLive(state.session) }, size = 48.dp, tint = agent.look.deep)
            }
        }
        MiniPlayer()
        Box(Modifier.weight(1f).fillMaxWidth()) {
        LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (state.items?.isEmpty() == true && state.live == null) item {
                Column(Modifier.fillMaxWidth().padding(top = 60.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Mascot(agent.look, Modifier.size(110.dp), Mood.CONTENT)
                    Text(stringResource(R.string.empty_thread, agent.name), style = Bip.text(17, FontWeight.Bold, Bip.ink2))
                }
            }
            val all = thread
            if (all.size > visibleCount) item(key = "earlier") {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    Pill("↑  " + stringResource(R.string.earlier_messages), dark = false) { visibleCount += 80 }
                }
            }
            val first = (all.size - visibleCount).coerceAtLeast(0)
            var previousDay: Long? = null
            for (index in first until all.size) {
                val item = all[index]
                item.time?.let { time ->
                    val day = dayOf(time)
                    if (day != previousDay) item(key = "day-${item.key}") { DaySeparator(time) }
                    previousDay = day
                }
                val owner = owners[item.key]
                val fold = folds[item.key]
                when {
                    // Exchanges between agents (Bot Mode) fold into one line, like the work.
                    owner != null -> {
                        val teammate = (item as? Item.User)?.let { ChatText.teammate(it.text) }
                        if (owner == item.key && teammate != null) {
                            item(key = "x-${item.key}") {
                                ExchangeChip(teammate.name, ThreadLayout.isRequest(all, index), teammateLook(store, teammate),
                                    openExchanges[owner] == true) { openExchanges[owner] = openExchanges[owner] != true }
                            }
                            if (openExchanges[owner] == true) item(key = item.key) { Line(index, false) }
                        } else if (openExchanges[owner] == true) item(key = item.key) { Line(index, ThreadLayout.turnEnd(all, index)) }
                    }
                    // How the agent worked (tools, reasoning, « Je vais chercher… ») folds into one line.
                    fold != null -> if (showsWork) {
                        if (fold == item.key || index == first) item(key = "w-${item.key}") {
                            WorkChip(openWork[fold] == true, { openWork[fold] = openWork[fold] != true }) {
                                showsWork = false
                                WorkDetail.set(context, false)
                                toldWhereWork = true
                            }
                        }
                        if (openWork[fold] == true) item(key = item.key) { Line(index, false) }
                    }
                    else -> item(key = item.key) { Line(index, ThreadLayout.turnEnd(all, index)) }
                }
            }
            if (state.live != null && state.approval == null && state.clarify == null && state.live?.streaming != true) item(key = "typing") {
                Typing(agent.look, state.live?.interim)
            }
            state.approval?.let { item(key = "approval") { ApprovalCard(agent.look, agent.name, it, model::approve) } }
            state.clarify?.let { item(key = "clarify") { ClarifyCard(agent.look, agent.name, it, model::answer) } }
        }
        if (away) Box(Modifier.align(Alignment.BottomEnd).padding(14.dp)) {
            RoundButton(Icons.Rounded.KeyboardArrowDown, stringResource(R.string.latest_messages), {
                scope.launch { stick = true; toEnd(animated = true) }
            }, size = 44.dp)
        }
        }
        state.error?.let { error ->
            Text(describe(error), style = Bip.text(14, FontWeight.Bold, Bip.danger),
                modifier = Modifier.fillMaxWidth().tap(model::dismissError).padding(horizontal = 18.dp, vertical = 6.dp))
        }
        if (attachments.isNotEmpty()) Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 14.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            attachments.forEach { file ->
                Text((if (file.isImage) "🖼️ " else "📎 ") + file.name + "  ✕", style = Bip.text(14, FontWeight.Bold), maxLines = 1,
                    modifier = Modifier.background(Bip.card, RoundedCornerShape(50)).tap { attachments.remove(file) }
                        .padding(horizontal = 12.dp, vertical = 7.dp))
            }
        }
        Composer(draft, { draft = it }, stringResource(R.string.message_to, agent.name), running = state.live != null,
            ready = draft.isNotBlank() || attachments.isNotEmpty(), look = agent.look, language = agent.language,
            onVoice = model::sendVoice,
            onPhoto = { photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
            onFile = { files.launch(arrayOf("*/*")) },
            onSend = { if (model.send(draft, attachments.toList())) { draft = ""; Drafts.set(context, agentId, session, ""); attachments.clear() } },
            onStop = model::stop)
    }
    }
    if (toldWhereWork) androidx.compose.material3.AlertDialog({ toldWhereWork = false }, confirmButton = {
        androidx.compose.material3.TextButton({ toldWhereWork = false }) { Text("OK") }
    }, title = { Text(stringResource(R.string.work_hidden_title)) }, text = { Text(stringResource(R.string.work_hidden_body)) })
}

private fun teammateLook(store: AgentStore, teammate: ChatText.Teammate) = store.agents.value.firstOrNull {
    it.bridgeName.equals(teammate.handle, ignoreCase = true) || it.name.equals(teammate.name, ignoreCase = true)
}?.look

@Composable
private fun instructionTitle(instruction: ChatText.Instruction): String = when (instruction) {
    is ChatText.Instruction.Scheduled -> if (instruction.skills.isEmpty()) stringResource(R.string.scheduled_brief)
        else stringResource(R.string.scheduled_skills, instruction.skills)
    is ChatText.Instruction.Skill -> stringResource(R.string.skill_brief, instruction.skills)
    ChatText.Instruction.Subtask -> stringResource(R.string.subtask_result)
}

@Composable
private fun UserBubble(text: String, image: Boolean, time: String, files: List<io.github.bipagents.model.StoredFile> = emptyList()) {
    val context = LocalContext.current
    val folder = remember { java.io.File(context.filesDir, "attachments") }
    // Files shown as such: their reference lines (« [Pièce jointe : … → …] ») would repeat them.
    val shown = if (files.isEmpty()) (if (image) "🖼️ " else "") + text
        else text.lines().filterNot { it.startsWith("[Pièce jointe : ") && it.endsWith("]") }.joinToString("\n").trim()
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
        files.filter { it.isImage }.forEach { stored ->
            val file = folder.resolve(stored.file)
            var bitmap by remember(stored.file) { mutableStateOf<androidx.compose.ui.graphics.ImageBitmap?>(null) }
            LaunchedEffect(stored.file) { bitmap = withContext(Dispatchers.IO) { MediaFiles.image(file, 900) } }
            var viewing by remember { mutableStateOf(false) }
            bitmap?.let { b ->
                androidx.compose.foundation.Image(b, stored.name, Modifier.widthIn(max = 240.dp).heightIn(max = 300.dp)
                    .clip(RoundedCornerShape(18.dp)).tap { viewing = true }, contentScale = androidx.compose.ui.layout.ContentScale.Fit)
                if (viewing) PhotoViewer(b, { viewing = false }) { MediaFiles.share(context, file, stored.name) }
            }
        }
        files.filterNot { it.isImage }.forEach { stored ->
            Row(Modifier.background(Bip.card, RoundedCornerShape(50)).border(1.dp, Bip.line, RoundedCornerShape(50))
                .tap { MediaFiles.open(context, folder.resolve(stored.file), stored.name) }.padding(horizontal = 14.dp, vertical = 9.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                androidx.compose.material3.Icon(Icons.Rounded.AttachFile, null, Modifier.size(16.dp), tint = Bip.ink2)
                Text(stored.name, style = Bip.text(14, FontWeight.Bold), maxLines = 1, modifier = Modifier.widthIn(max = 220.dp))
                Text(io.github.bipagents.model.Attachment.formatSize(stored.size), style = Bip.text(12, FontWeight.Bold, Bip.muted))
            }
        }
        if (shown.isNotBlank()) Text(shown, style = Bip.text(17, FontWeight.SemiBold, Bip.card, lineHeight = 23.sp),
            modifier = Modifier.widthIn(max = 310.dp).background(Bip.ink, RoundedCornerShape(22.dp, 22.dp, 6.dp, 22.dp))
                .padding(horizontal = 16.dp, vertical = 10.dp))
        if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(end = 4.dp))
    }
}

@Composable
private fun Reply(look: io.github.bipagents.model.Look, text: String, time: String, streaming: Boolean = false,
                  listen: (() -> Unit)? = null, listenKey: String? = null, name: String = "") {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (ChatText.visible(text).isNotEmpty()) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mascot(look, Modifier.size(28.dp), animated = false)
            MarkdownText(text, Modifier.weight(1f), link = look.deep)
        }
        if (!streaming) ChatText.media(text).forEach { path -> MediaRow(path, look, name) }
        if (listen != null && ChatText.visible(text).isNotEmpty()) ListenPill(look, listenKey ?: "", listen)
        if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(start = 36.dp))
    }
}

/** « Écouter » under a reply: the Bip's voice, streamed; again to stop. */
@Composable
private fun ListenPill(look: io.github.bipagents.model.Look, key: String, onClick: () -> Unit) {
    val now by Speaker.now.collectAsState()
    val mine = now?.key == key
    Row(Modifier.padding(start = 36.dp).background(look.tint, RoundedCornerShape(50)).tap(onClick).padding(start = 6.dp, end = 14.dp, top = 5.dp, bottom = 5.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.size(26.dp).background(look.deep, CircleShape), contentAlignment = Alignment.Center) {
            when {
                mine && now?.phase == Speaker.Phase.PREPARING -> CircularProgressIndicator(Modifier.size(14.dp), color = Bip.card, strokeWidth = 2.dp)
                mine -> androidx.compose.material3.Icon(Icons.Rounded.Stop, null, Modifier.size(16.dp), tint = Bip.card)
                else -> androidx.compose.material3.Icon(Icons.Rounded.PlayArrow, null, Modifier.size(18.dp), tint = Bip.card)
            }
        }
        Text(stringResource(if (mine) R.string.stop else R.string.listen), style = Bip.text(15, FontWeight.ExtraBold, look.deep))
    }
}

/** A voice note the person sent: its recording (play, waveform, length) above its transcript. */
@Composable
private fun VoiceBubble(note: io.github.bipagents.voice.VoiceNotes.Note, key: String, time: String) {
    val now by Speaker.now.collectAsState()
    val playing = now?.key == key
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        Column(Modifier.widthIn(max = 310.dp).background(Bip.ink, RoundedCornerShape(22.dp, 22.dp, 6.dp, 22.dp))
            .padding(horizontal = 14.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                val audio = note.audio
                Box(Modifier.size(34.dp).background(Bip.card, CircleShape).tap {
                    if (audio == null) return@tap
                    if (playing) Speaker.stop() else Speaker.play(key, audio)
                }, contentAlignment = Alignment.Center) {
                    androidx.compose.material3.Icon(if (audio == null) Icons.Rounded.Mic else if (playing) Icons.Rounded.Stop else Icons.Rounded.PlayArrow,
                        null, tint = Bip.ink)
                }
                Waveform(note.levels, Modifier.width(150.dp).height(26.dp))
                Text(duration(note.seconds), style = Bip.text(13, FontWeight.Bold, Bip.line))
            }
            Text(note.text, style = Bip.text(15, FontWeight.SemiBold, Bip.card))
        }
        if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(top = 3.dp, end = 4.dp))
    }
}

@Composable
private fun Waveform(levels: List<Float>, modifier: Modifier, color: Color = Bip.card) {
    androidx.compose.foundation.Canvas(modifier) {
        val bars = 30
        val values = if (levels.isEmpty()) List(bars) { 0.15f } else List(bars) { i -> levels[(i * levels.size / bars).coerceAtMost(levels.size - 1)] }
        val step = size.width / bars
        values.forEachIndexed { i, v ->
            val h = (0.15f + 0.85f * v.coerceIn(0f, 1f)) * size.height
            drawRoundRect(color, androidx.compose.ui.geometry.Offset(i * step + step * 0.2f, (size.height - h) / 2),
                androidx.compose.ui.geometry.Size(step * 0.6f, h), androidx.compose.ui.geometry.CornerRadius(step))
        }
    }
}

private fun duration(seconds: Double): String { val s = seconds.toInt(); return "%d:%02d".format(s / 60, s % 60) }

/** The conversation's bridge, for the files its replies point to. */
val LocalBridge = androidx.compose.runtime.staticCompositionLocalOf<io.github.bipagents.net.Bridge?> { null }

/** « I need your OK » (the iPhone's approval card): the Bip, what it wants to run, Once / This session / Deny. */
@Composable
private fun ApprovalCard(look: Look, name: String, request: RunEvent.Approval, onChoose: (String) -> Unit) {
    // « Always » is never offered from the app: risky commands keep asking.
    val choices = request.choices.filter { it != "always" }
    Card(border = look.main) {
        CardHeader(look, stringResource(R.string.need_your_ok), stringResource(R.string.approval_subtitle, name))
        request.description?.takeIf { request.command == null }?.let { Text(it, style = Bip.text(15, FontWeight.Medium, Bip.ink2)) }
        request.command?.let {
            Text(it, style = Bip.text(14).copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.fillMaxWidth().background(Bip.ground, RoundedCornerShape(14.dp)).padding(12.dp))
        }
        val first = choices.filter { it != "deny" }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            first.forEachIndexed { i, choice ->
                ChoiceButton(stringResource(if (choice == "session") R.string.this_session else R.string.once),
                    if (i == 0) Bip.ink else Bip.ground, if (i == 0) Bip.card else Bip.ink, Modifier.weight(1f)) { onChoose(choice) }
            }
        }
        if ("deny" in choices) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ChoiceButton(stringResource(R.string.deny), Color(0xFFFDE7E7), Color(0xFFC62828), Modifier.weight(1f)) { onChoose("deny") }
            if (first.size > 1) Spacer(Modifier.weight(1f))
        }
    }
}

/** « Wellness has a question for you »: a tinted button per choice, then « Other answer… » and « Skip ». */
@Composable
private fun ClarifyCard(look: Look, name: String, request: RunEvent.Clarify, onAnswer: (Map<String, String>?) -> Unit) {
    val answers = remember(request) { mutableStateMapOf<String, String>() }
    var other by remember(request) { mutableStateOf(false) }
    Card(border = look.main) {
        CardHeader(look, stringResource(R.string.question_for_you, name), null)
        for (q in request.questions) {
            Text(q.text, style = Bip.text(17, FontWeight.ExtraBold))
            for (choice in q.choices) {
                val picked = answers[q.id] == choice
                ChoiceButton(choice, if (picked) look.main else look.tint, if (picked) Bip.card else look.deep, Modifier.fillMaxWidth(),
                    alignStart = true) {
                    answers[q.id] = choice
                    if (request.questions.size == 1) onAnswer(answers.toMap())
                }
            }
            if (q.allowOther) {
                if (other) TextField(if (answers[q.id] in q.choices) "" else answers[q.id] ?: "", { answers[q.id] = it },
                    Modifier.fillMaxWidth(), placeholder = { Text(stringResource(R.string.your_answer)) }, textStyle = Bip.text(15),
                    colors = TextFieldDefaults.colors(focusedContainerColor = Bip.ground, unfocusedContainerColor = Bip.ground,
                        focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent), shape = RoundedCornerShape(12.dp))
                else Text(stringResource(R.string.other_answer), style = Bip.text(15, FontWeight.ExtraBold, look.deep), modifier = Modifier.tap { other = true })
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            if (other || request.questions.size > 1) ChoiceButton(stringResource(R.string.answer), Bip.ink, Bip.card, Modifier) { onAnswer(answers.toMap()) }
            Text(stringResource(R.string.skip), style = Bip.text(14, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.tap { onAnswer(null) })
        }
    }
}

@Composable
private fun CardHeader(look: Look, title: String, subtitle: String?) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(52.dp).background(look.tint, RoundedCornerShape(16.dp)), contentAlignment = Alignment.Center) {
            Mascot(look, Modifier.size(44.dp), Mood.ASKING)
        }
        Column {
            Text(title, style = Bip.text(17, FontWeight.ExtraBold))
            subtitle?.let { Text(it, style = Bip.text(13, FontWeight.Medium, Bip.ink2)) }
        }
    }
}

@Composable
private fun ChoiceButton(label: String, background: Color, color: Color, modifier: Modifier, alignStart: Boolean = false, onClick: () -> Unit) {
    Box(modifier.background(background, RoundedCornerShape(16.dp)).tap(onClick).padding(horizontal = 18.dp, vertical = 13.dp),
        contentAlignment = if (alignStart) Alignment.CenterStart else Alignment.Center) {
        Text(label, style = Bip.text(16, FontWeight.ExtraBold, color))
    }
}

@Composable
private fun Card(border: Color? = null, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(26.dp))
        .then(if (border != null) Modifier.border(2.dp, border, RoundedCornerShape(26.dp)) else Modifier).padding(18.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)) { content() }
}

@Composable
fun Pill(text: String, dark: Boolean, onClick: () -> Unit) {
    Text(text, style = Bip.text(15, FontWeight.ExtraBold, if (dark) Bip.card else Bip.ink),
        modifier = Modifier.background(if (dark) Bip.ink else Bip.card, RoundedCornerShape(50))
            .border(1.5.dp, if (dark) Bip.ink else Bip.line, RoundedCornerShape(50)).tap(onClick).padding(horizontal = 16.dp, vertical = 9.dp))
}

@Composable
private fun Composer(draft: String, onDraft: (String) -> Unit, placeholder: String, running: Boolean, ready: Boolean,
                     look: io.github.bipagents.model.Look, language: String?, onVoice: (RecordedNote) -> Unit,
                     onPhoto: () -> Unit, onFile: () -> Unit, onSend: () -> Unit, onStop: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val recorder = remember { Recorder(context.applicationContext) }
    var recording by remember { mutableStateOf<String?>(null) }   // null, "holding", "locked"
    var cancelling by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {}
    val unavailable = stringResource(R.string.mic_unavailable)
    fun begin() {
        if (!recorder.allowed) { askMic.launch(android.Manifest.permission.RECORD_AUDIO); return }
        Speaker.stop()
        recording = "holding"
        cancelling = false
        failure = null
        scope.launch { if (!recorder.start(language)) { recording = null; failure = unavailable } }
    }
    fun finish(send: Boolean) {
        if (recording == null) return
        recording = null
        scope.launch {
            if (!send) { recorder.cancel(); return@launch }
            recorder.stop()?.let(onVoice)
        }
    }
    failure?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger), modifier = Modifier.padding(horizontal = 18.dp)) }
    if (recording != null) {
        val partial by recorder.partial.collectAsState()
        val level by recorder.level.collectAsState()
        Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp).background(Bip.card, RoundedCornerShape(20.dp)).padding(14.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Box(Modifier.size((10 + 10 * level).dp).background(Bip.danger, CircleShape))
                Text(stringResource(if (cancelling) R.string.release_to_cancel else if (recording == "locked") R.string.recording else R.string.slide_to_cancel),
                    style = Bip.text(14, FontWeight.ExtraBold, if (cancelling) Bip.danger else Bip.ink2), modifier = Modifier.weight(1f))
                if (recording == "locked") Text(stringResource(R.string.cancel), style = Bip.text(14, FontWeight.ExtraBold, Bip.danger),
                    modifier = Modifier.tap { finish(false) })
            }
            if (partial.isNotEmpty()) Text(partial, style = Bip.text(16, FontWeight.SemiBold))
        }
    }
    Row(Modifier.navigationBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.Bottom,
        horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Box {
            RoundButton(Icons.Rounded.Add, stringResource(R.string.attach), { menu = true }, size = 52.dp)
            DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                DropdownMenuItem({ Text(stringResource(R.string.attach_photo), style = Bip.text(16, FontWeight.Bold)) }, { menu = false; onPhoto() },
                    leadingIcon = { androidx.compose.material3.Icon(Icons.Rounded.Image, null) })
                DropdownMenuItem({ Text(stringResource(R.string.attach_file), style = Bip.text(16, FontWeight.Bold)) }, { menu = false; onFile() },
                    leadingIcon = { androidx.compose.material3.Icon(Icons.Rounded.AttachFile, null) })
            }
        }
        TextField(draft, onDraft, Modifier.weight(1f), placeholder = { Text(placeholder, style = Bip.text(16, color = Bip.muted)) },
            textStyle = Bip.text(16), maxLines = 6, shape = RoundedCornerShape(26.dp),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            colors = TextFieldDefaults.colors(focusedContainerColor = Bip.card, unfocusedContainerColor = Bip.card,
                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent))
        if (running) RoundButton(Icons.Rounded.Stop, stringResource(R.string.stop), onStop, size = 52.dp, background = Bip.danger, tint = Bip.card)
        else if (recording == "locked") RoundButton(Icons.Rounded.ArrowUpward, stringResource(R.string.send), { finish(true) }, size = 52.dp,
            background = Bip.ink, tint = Bip.card)
        else if (ready) RoundButton(Icons.Rounded.ArrowUpward, stringResource(R.string.send), onSend, size = 52.dp, background = Bip.ink, tint = Bip.card)
        else {
            // Push-to-talk (WhatsApp, the iPhone app): hold to speak, release to send, slide left to cancel, up to lock.
            val density = androidx.compose.ui.platform.LocalDensity.current
            Box(Modifier.size(52.dp).background(if (recording != null) Bip.danger else look.deep, CircleShape)
                .pointerInput(Unit) {
                    awaitEachGesture {
                        val down = awaitFirstDown()
                        begin()
                        var locked = false
                        while (true) {
                            val event = awaitPointerEvent()
                            val change = event.changes.firstOrNull { it.id == down.id } ?: break
                            val dx = change.position.x - down.position.x
                            val dy = change.position.y - down.position.y
                            cancelling = dx < -with(density) { 90.dp.toPx() }
                            if (!locked && dy < -with(density) { 80.dp.toPx() } && recording == "holding") { locked = true; recording = "locked" }
                            if (!change.pressed) break
                        }
                        if (!locked) finish(!cancelling)
                        cancelling = false
                    }
                }, contentAlignment = Alignment.Center) {
                androidx.compose.material3.Icon(Icons.Rounded.Mic, stringResource(R.string.hold_to_talk), tint = Bip.card)
            }
        }
    }
}

/** A voice note recorded on the home screen, handed to the conversation that sends it. */
object PendingVoice {
    private val notes = HashMap<String, io.github.bipagents.voice.RecordedNote>()
    fun put(agent: String, note: io.github.bipagents.voice.RecordedNote) { notes[agent] = note }
    fun take(agent: String) = notes.remove(agent)
}
