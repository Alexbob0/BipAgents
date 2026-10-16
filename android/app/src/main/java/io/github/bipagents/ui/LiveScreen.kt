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
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.material.icons.rounded.PanTool
import androidx.compose.ui.graphics.Color
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

/** The Live (CallView.swift): the Bip big in the middle, listening, thinking and talking; what was heard, what it
 *  answers; tap it (or talk) to interrupt. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun LiveScreen(store: AgentStore, agentId: String, session: String?, onEnd: () -> Unit, onKeyboard: (String?) -> Unit) {
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
    val speaking = state.phase == Live.Phase.SPEAKING
    val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
    LaunchedEffect(state.phase) { haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.TextHandleMove) }
    Column(Modifier.fillMaxSize().background(look.tint).statusBarsPadding().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 12.dp),
        horizontalAlignment = Alignment.CenterHorizontally) {
        // The name and the call's time; ⌄ closes the Live.
        Box(Modifier.fillMaxWidth()) {
            Box(Modifier.size(44.dp).tap(onEnd), contentAlignment = Alignment.Center) {
                Icon(Icons.Rounded.KeyboardArrowDown, stringResource(R.string.hang_up), Modifier.size(30.dp), tint = Bip.ink)
            }
            Column(Modifier.align(Alignment.Center), horizontalAlignment = Alignment.CenterHorizontally) {
                Text(agent.name, style = Bip.text(17, FontWeight.ExtraBold))
                var now by remember { mutableStateOf(System.currentTimeMillis()) }
                LaunchedEffect(Unit) { while (true) { kotlinx.coroutines.delay(1000); now = System.currentTimeMillis() } }
                val seconds = ((now - state.startedAt) / 1000).coerceAtLeast(0)
                Text("%d:%02d".format(seconds / 60, seconds % 60), style = Bip.text(13, FontWeight.Bold, Bip.ink2))
            }
        }
        Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            StatePill(stringResource(R.string.pill_listening), state.phase == Live.Phase.LISTENING, look)
            StatePill(stringResource(R.string.pill_thinking), state.phase == Live.Phase.THINKING, look)
            StatePill(stringResource(R.string.pill_speaking), speaking, look)
        }
        Spacer(Modifier.weight(0.6f))
        Box(Modifier.size(280.dp, 260.dp).combinedClickable(interactionSource = remember { androidx.compose.foundation.interaction.MutableInteractionSource() },
            indication = null,
            onLongClick = { if (io.github.bipagents.BuildConfig.DEBUG) live?.say("Comment mieux dormir ?") },
            onClick = { if (speaking || state.phase == Live.Phase.THINKING) live?.interrupt() }), contentAlignment = Alignment.Center) {
            if (state.phase == Live.Phase.LISTENING) Ripples(look.main, state.inputLevel)
            Mascot(look, Modifier.size(200.dp), mood, pose = Pose(mouth = if (speaking) state.outputLevel else null))
        }
        Spacer(Modifier.weight(0.4f))
        Column(Modifier.fillMaxWidth().weight(1f, fill = false).verticalScroll(rememberScrollState()),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            val error = if (!allowed || state.error == "mic") stringResource(R.string.mic_unavailable) else state.error
            if (error != null) {
                Text(error, style = Bip.text(15, FontWeight.Bold, Bip.danger), textAlign = TextAlign.Center)
                if (state.phase != Live.Phase.SPEAKING && state.phase != Live.Phase.THINKING)
                    Pill("↻  " + stringResource(R.string.retry), dark = true) { if (!allowed) ask.launch(Manifest.permission.RECORD_AUDIO) else live?.retry() }
            }
            when (state.phase) {
                Live.Phase.THINKING, Live.Phase.SPEAKING -> {
                    if (state.heard.isNotEmpty()) Text("« ${state.heard} »", style = Bip.text(14, FontWeight.Bold, Bip.ink2), textAlign = TextAlign.Center)
                    state.tool?.let { tool ->
                        Text("🔧 $tool", style = Bip.text(12, FontWeight.Bold, Bip.ink2),
                            modifier = Modifier.background(Bip.card, RoundedCornerShape(50)).padding(horizontal = 11.dp, vertical = 5.dp))
                    }
                    // The end of the reply (its last six lines or so), as it is said.
                    val reply = ShareText.plain(state.reply)
                    Text(if (reply.isEmpty()) "…" else if (reply.length > 320) "…" + reply.takeLast(320) else reply,
                        style = Bip.text(23, FontWeight.ExtraBold), textAlign = TextAlign.Center)
                }
                else -> Text(state.heard.ifEmpty { stringResource(R.string.live_listening) },
                    style = Bip.text(if (state.heard.isEmpty()) 20 else 27, FontWeight.ExtraBold, if (state.heard.isEmpty()) look.deep else Bip.ink),
                    textAlign = TextAlign.Center)
            }
        }
        Spacer(Modifier.height(12.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            Icon(if (speaking) Icons.Rounded.PanTool else Icons.Rounded.GraphicEq, null, Modifier.size(15.dp), tint = Bip.ink2)
            Text(if (speaking) stringResource(R.string.live_hint_speaking, agent.name) else stringResource(R.string.live_hint_listening),
                style = Bip.text(13, FontWeight.Bold, Bip.ink2))
        }
        Row(Modifier.fillMaxWidth().padding(top = 18.dp), horizontalArrangement = Arrangement.SpaceEvenly) {
            Control(stringResource(R.string.keyboard), Icons.Rounded.Keyboard, Bip.card, Bip.ink) { onKeyboard(live?.conversation ?: session) }
            Control(stringResource(R.string.interrupt), Icons.Rounded.PanTool, Bip.card, if (speaking) Bip.ink else Bip.muted) { if (speaking) live?.interrupt() }
            Control(stringResource(R.string.end), Icons.Rounded.Close, Bip.danger, Color.White, onEnd)
        }
    }
    // The agent asks before running something: answered here, the Live goes on.
    state.approval?.let { request ->
        androidx.compose.material3.ModalBottomSheet({}, sheetState = androidx.compose.material3.rememberModalBottomSheetState(
            skipPartiallyExpanded = true, confirmValueChange = { it != androidx.compose.material3.SheetValue.Hidden }),
            containerColor = Bip.ground, dragHandle = null) {
            Box(Modifier.padding(16.dp).navigationBarsPadding()) { ApprovalCard(look, agent.name, request) { live?.approve(it) } }
        }
    }
}

@Composable
private fun StatePill(label: String, active: Boolean, look: io.github.bipagents.model.Look) {
    Row(Modifier.height(30.dp).background(if (active) Bip.card else Color.Transparent, RoundedCornerShape(50)).padding(horizontal = 13.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        if (active) Box(Modifier.size(7.dp).background(look.main, CircleShape))
        Text(label, style = Bip.text(13, FontWeight.ExtraBold, if (active) look.deep else Bip.ink2))
    }
}

@Composable
private fun Control(label: String, icon: androidx.compose.ui.graphics.vector.ImageVector, background: Color, color: Color, onClick: () -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Box(Modifier.size(66.dp).background(background, CircleShape).tap(onClick), contentAlignment = Alignment.Center) {
            Icon(icon, label, Modifier.size(26.dp), tint = color)
        }
        Text(label, style = Bip.text(12, FontWeight.ExtraBold, Bip.ink2))
    }
}

/** Rings that breathe with the microphone's level. */
@Composable
private fun Ripples(color: Color, level: Float) {
    val t by androidx.compose.runtime.produceState(0f) {
        val start = androidx.compose.runtime.withFrameMillis { it }
        while (true) androidx.compose.runtime.withFrameMillis { value = (it - start) / 1000f }
    }
    androidx.compose.foundation.Canvas(Modifier.size(250.dp)) {
        for (i in 0 until 3) {
            val phase = ((t / 2.6f) + i / 3f) % 1f
            val r = size.minDimension / 2 * (0.7f + phase * (0.75f + level * 0.4f))
            drawCircle(color.copy(alpha = 0.22f * (1 - phase)), r)
        }
    }
}
