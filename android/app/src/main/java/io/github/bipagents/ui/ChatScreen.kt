package io.github.bipagents.ui

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
import androidx.lifecycle.viewmodel.compose.viewModel
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.RunEvent

@Composable
fun ChatScreen(store: AgentStore, agentId: String, session: String?, onBack: () -> Unit, onSessions: () -> Unit) {
    val model: ChatModel = viewModel(key = "chat-$agentId-${session ?: "main"}")
    LaunchedEffect(Unit) { model.start(agentId, session) }
    val state by model.state.collectAsState()
    val agent = state.agent ?: store.agent(agentId) ?: return
    var draft by rememberSaveable(agentId) { mutableStateOf("") }
    val list = rememberLazyListState()
    val count = (state.items?.size ?: 0) + (if (state.live != null) 2 else 0)
    LaunchedEffect(count, state.live?.text?.length, state.approval, state.clarify) {
        val last = list.layoutInfo.totalItemsCount - 1
        if (last >= 0) list.animateScrollToItem(last)
    }

    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar(agent.name, onBack, leading = { Mascot(agent.look, Modifier.size(40.dp), state.mood) },
            actions = { RoundButton(Icons.AutoMirrored.Rounded.List, stringResource(R.string.conversations), onSessions, size = 44.dp) })
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (state.items?.isEmpty() == true && state.live == null) item {
                Column(Modifier.fillMaxWidth().padding(top = 60.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Mascot(agent.look, Modifier.size(110.dp), Mood.CONTENT)
                    Text(stringResource(R.string.empty_thread, agent.name), style = Bip.text(17, FontWeight.Bold, Bip.ink2))
                }
            }
            items(state.items ?: emptyList(), key = { it.key }) { item ->
                when (item) {
                    is Item.User -> UserBubble(item.text, item.image, clock(item.time))
                    is Item.Reply -> Reply(agent.look, item.text, item.tools, clock(item.time))
                    is Item.Tools -> ToolsLine(item.tools)
                }
            }
            state.live?.let { live ->
                live.sent?.let { item(key = "live-sent") { UserBubble(it, false, "") } }
                item(key = "live") { Reply(agent.look, live.text, live.tools, "", typing = live.text.isEmpty()) }
            }
            state.approval?.let { item(key = "approval") { ApprovalCard(it, model::approve) } }
            state.clarify?.let { item(key = "clarify") { ClarifyCard(it, model::answer) } }
        }
        state.error?.let { error ->
            Text(describe(error), style = Bip.text(14, FontWeight.Bold, Bip.danger),
                modifier = Modifier.fillMaxWidth().tap(model::dismissError).padding(horizontal = 18.dp, vertical = 6.dp))
        }
        Composer(draft, { draft = it }, stringResource(R.string.message_to, agent.name), running = state.live != null,
            onSend = { model.send(draft); draft = "" }, onStop = model::stop)
    }
}

@Composable
private fun UserBubble(text: String, image: Boolean, time: String) {
    Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
        Text((if (image) "🖼️ " else "") + text, style = Bip.text(16, FontWeight.SemiBold, Bip.card),
            modifier = Modifier.widthIn(max = 310.dp).background(Bip.ink, RoundedCornerShape(22.dp, 22.dp, 6.dp, 22.dp))
                .padding(horizontal = 16.dp, vertical = 10.dp))
        if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(top = 3.dp, end = 4.dp))
    }
}

@Composable
private fun Reply(look: io.github.bipagents.model.Category, text: String, tools: List<String>, time: String, typing: Boolean = false) {
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (tools.isNotEmpty()) ToolsLine(tools)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Mascot(look, Modifier.size(28.dp), animated = false)
            if (typing) Text("•••", style = Bip.text(20, FontWeight.Black, Bip.muted))
            else MarkdownText(text, Modifier.weight(1f))
        }
        ChatText.media(text).forEach { path ->
            Text("📎 " + path.substringAfterLast('/'), style = Bip.text(14, FontWeight.Bold, Bip.ink2),
                modifier = Modifier.padding(start = 36.dp).background(Bip.card, RoundedCornerShape(50)).padding(horizontal = 12.dp, vertical = 6.dp))
        }
        if (time.isNotEmpty()) Text(time, style = Bip.text(12, FontWeight.Bold, Bip.muted), modifier = Modifier.padding(start = 36.dp))
    }
}

@Composable
private fun ToolsLine(tools: List<String>) {
    Row(Modifier.padding(start = 36.dp).background(Bip.card, RoundedCornerShape(14.dp)).border(1.dp, Bip.line, RoundedCornerShape(14.dp))
        .padding(horizontal = 12.dp, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        androidx.compose.material3.Icon(Icons.Rounded.Build, null, Modifier.size(14.dp), tint = Bip.muted)
        Text(tools.distinct().joinToString(" · "), style = Bip.text(13, FontWeight.Bold, Bip.ink2), maxLines = 2)
    }
}

@Composable
private fun ApprovalCard(request: RunEvent.Approval, onChoose: (String) -> Unit) {
    // « Always » is never offered from the app: risky commands keep asking.
    val choices = request.choices.filter { it != "always" }
    Card(border = Bip.accent) {
        Text(stringResource(R.string.approval_title), style = Bip.text(17, FontWeight.ExtraBold))
        request.description?.let { Text(it, style = Bip.text(15, color = Bip.ink2)) }
        request.command?.let {
            Text(it, style = Bip.text(14).copy(fontFamily = FontFamily.Monospace),
                modifier = Modifier.fillMaxWidth().background(Bip.ground, RoundedCornerShape(10.dp)).padding(10.dp))
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            for (choice in choices) Pill(stringResource(when (choice) {
                "once" -> R.string.allow_once; "session" -> R.string.allow_session; "deny" -> R.string.deny; else -> R.string.allow_once
            }), dark = choice != "deny") { onChoose(choice) }
        }
    }
}

@Composable
private fun ClarifyCard(request: RunEvent.Clarify, onAnswer: (Map<String, String>?) -> Unit) {
    val answers = remember(request) { mutableStateMapOf<String, String>() }
    Card(border = Bip.accent) {
        Text(stringResource(R.string.question), style = Bip.text(17, FontWeight.ExtraBold))
        for (q in request.questions) {
            Text(q.text, style = Bip.text(16, FontWeight.SemiBold))
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                for (choice in q.choices) Pill(choice, dark = answers[q.id] == choice) { answers[q.id] = choice }
            }
            if (q.allowOther) TextField(if (answers[q.id] in q.choices) "" else answers[q.id] ?: "", { answers[q.id] = it },
                Modifier.fillMaxWidth(), placeholder = { Text(stringResource(R.string.your_answer)) }, textStyle = Bip.text(15),
                colors = TextFieldDefaults.colors(focusedContainerColor = Bip.ground, unfocusedContainerColor = Bip.ground,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent), shape = RoundedCornerShape(12.dp))
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Pill(stringResource(R.string.answer), dark = true) { onAnswer(answers.toMap()) }
            Pill(stringResource(R.string.skip), dark = false) { onAnswer(null) }
        }
    }
}

@Composable
private fun Card(border: Color? = null, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(20.dp))
        .then(if (border != null) Modifier.border(2.dp, border, RoundedCornerShape(20.dp)) else Modifier).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }
}

@Composable
fun Pill(text: String, dark: Boolean, onClick: () -> Unit) {
    Text(text, style = Bip.text(15, FontWeight.ExtraBold, if (dark) Bip.card else Bip.ink),
        modifier = Modifier.background(if (dark) Bip.ink else Bip.card, RoundedCornerShape(50))
            .border(1.5.dp, if (dark) Bip.ink else Bip.line, RoundedCornerShape(50)).tap(onClick).padding(horizontal = 16.dp, vertical = 9.dp))
}

@Composable
private fun Composer(draft: String, onDraft: (String) -> Unit, placeholder: String, running: Boolean, onSend: () -> Unit, onStop: () -> Unit) {
    Row(Modifier.navigationBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.Bottom,
        horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        TextField(draft, onDraft, Modifier.weight(1f), placeholder = { Text(placeholder, style = Bip.text(16, color = Bip.muted)) },
            textStyle = Bip.text(16), maxLines = 6, shape = RoundedCornerShape(26.dp),
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
            colors = TextFieldDefaults.colors(focusedContainerColor = Bip.card, unfocusedContainerColor = Bip.card,
                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent))
        if (running) RoundButton(Icons.Rounded.Stop, stringResource(R.string.stop), onStop, size = 52.dp, background = Bip.danger, tint = Bip.card)
        else RoundButton(Icons.Rounded.ArrowUpward, stringResource(R.string.send), { if (draft.isNotBlank()) onSend() }, size = 52.dp,
            background = if (draft.isBlank()) Bip.line else Bip.ink, tint = Bip.card)
    }
}
