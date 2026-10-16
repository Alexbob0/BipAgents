package io.github.bipagents.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CallEnd
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.voice.Live

/** The Live (CallView.swift): the Bip big in the middle, what was heard, what it answers; tap it to interrupt. */
@Composable
fun LiveScreen(store: AgentStore, agentId: String, session: String?, onEnd: () -> Unit) {
    val agent = store.agent(agentId) ?: return
    val context = LocalContext.current
    var allowed by remember { mutableStateOf(context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { allowed = it }
    LaunchedEffect(Unit) { if (!allowed) ask.launch(Manifest.permission.RECORD_AUDIO) }
    val live = remember(allowed) { if (allowed) Live(context.applicationContext, store, agent, session?.ifEmpty { null }) else null }
    DisposableEffect(live) {
        live?.start()
        onDispose { live?.hangUp() }
    }
    val view = LocalView.current
    DisposableEffect(Unit) { view.keepScreenOn = true; onDispose { view.keepScreenOn = false } }
    val state = live?.state?.collectAsState()?.value ?: Live.State()
    val look = agent.look
    val mood = when {
        state.approval != null -> Mood.ASKING
        state.phase == Live.Phase.LISTENING -> Mood.LISTENING
        state.phase == Live.Phase.THINKING -> Mood.THINKING
        state.phase == Live.Phase.SPEAKING -> Mood.SPEAKING
        else -> Mood.CONTENT
    }
    Column(Modifier.fillMaxSize().background(look.tint).statusBarsPadding().navigationBarsPadding().padding(20.dp),
        horizontalAlignment = Alignment.CenterHorizontally) {
        Text(agent.name, style = Bip.text(26, FontWeight.Black))
        Text(stringResource(when {
            !allowed || state.error == "mic" -> R.string.mic_unavailable
            state.phase == Live.Phase.LISTENING -> R.string.live_listening
            state.phase == Live.Phase.THINKING -> R.string.live_thinking
            state.phase == Live.Phase.SPEAKING -> R.string.live_speaking
            else -> R.string.loading
        }), style = Bip.text(16, FontWeight.Bold, look.deep))
        Spacer(Modifier.height(24.dp))
        Box(Modifier.size(220.dp).combinedClickable(interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() },
            indication = null,
            onLongClick = { if (io.github.bipagents.BuildConfig.DEBUG) live?.say("Comment mieux dormir ?") },
            onClick = { if (state.phase == Live.Phase.SPEAKING || state.phase == Live.Phase.THINKING) live?.interrupt() })) {
            Mascot(look, Modifier.fillMaxSize(), mood)
        }
        if (state.phase == Live.Phase.SPEAKING) Text(stringResource(R.string.live_tap_to_interrupt), style = Bip.text(13, FontWeight.Bold, Bip.muted))
        Spacer(Modifier.height(16.dp))
        Column(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (state.heard.isNotEmpty()) Text("« ${state.heard} »", style = Bip.text(17, FontWeight.Bold, Bip.ink2), textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth())
            state.tool?.let { Text("🔧 $it", style = Bip.text(14, FontWeight.Bold, Bip.muted), modifier = Modifier.fillMaxWidth(), textAlign = TextAlign.Center) }
            if (state.reply.isNotEmpty()) MarkdownText(state.reply, Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(20.dp)).padding(16.dp))
            state.approval?.let { request ->
                Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(20.dp)).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(stringResource(R.string.approval_title), style = Bip.text(17, FontWeight.ExtraBold))
                    (request.command ?: request.description)?.let { Text(it, style = Bip.text(15, color = Bip.ink2)) }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Pill(stringResource(R.string.allow_once), dark = true) { live?.approve("once") }
                        Pill(stringResource(R.string.deny), dark = false) { live?.approve("deny") }
                    }
                }
            }
            state.error?.takeIf { it != "mic" }?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
        }
        Box(Modifier.size(72.dp).background(Bip.danger, CircleShape).tap(onEnd), contentAlignment = Alignment.Center) {
            Icon(Icons.Rounded.CallEnd, stringResource(R.string.hang_up), Modifier.size(34.dp), tint = Bip.card)
        }
    }
}
