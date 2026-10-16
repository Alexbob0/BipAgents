package io.github.bipagents.ui

import android.media.MediaPlayer
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.Palette
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.RecordVoiceOver
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material.icons.rounded.Web
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Category
import io.github.bipagents.net.Desktop
import kotlinx.coroutines.launch

/** An agent's settings (AgentOptionsView.swift): its look, language and voice, its browser, removing it. */
@Composable
fun AgentOptionsScreen(store: AgentStore, agentId: String, onBack: () -> Unit, onVoice: () -> Unit, onDesktop: () -> Unit, onRemoved: () -> Unit,
                       onRoutines: () -> Unit, onSettings: () -> Unit) {
    val agents by store.agents.collectAsState()
    val agent = agents.firstOrNull { it.id == agentId } ?: return
    val scope = rememberCoroutineScope()
    var picking by remember { mutableStateOf(false) }
    var languages by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf(false) }
    var desktop by remember { mutableStateOf<Desktop?>(null) }
    var desktopChecked by remember { mutableStateOf(false) }
    // Personality and tools: for an agent this install manages (else it's set on the server).
    var managed by remember { mutableStateOf<Boolean?>(null) }
    LaunchedEffect(agentId) {
        launch { managed = runCatching { store.bridge(agent)?.agentSettings(agent.bridgeName) != null }.getOrDefault(false) }
        desktop = runCatching { store.bridge(agent)?.desktop(agent.bridgeName) }.getOrNull()
        desktopChecked = true
    }
    if (picking) LookPicker(agent, onPick = { c, color -> picking = false; scope.launch { store.setLook(agent, c, color) } }, onDismiss = { picking = false })
    if (languages) AlertDialog(onDismissRequest = { languages = false }, containerColor = Bip.card,
        title = { Text(stringResource(R.string.language), style = Bip.text(20, FontWeight.Black)) },
        text = {
            Column {
                for ((code, name) in LANGUAGES) Row(Modifier.fillMaxWidth().tap { languages = false; scope.launch { store.setLanguage(agent, code) } }
                    .padding(vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(name, style = Bip.text(17, FontWeight.Bold), modifier = Modifier.weight(1f))
                    if (language(agent) == code) Icon(Icons.Rounded.Check, null, tint = Bip.accent)
                }
            }
        }, confirmButton = {})
    if (removing) AlertDialog(onDismissRequest = { removing = false },
        title = { Text(stringResource(R.string.remove_title, agent.name)) }, text = { Text(stringResource(R.string.remove_body)) },
        confirmButton = { TextButton({ removing = false; store.remove(agent); onRemoved() }) { Text(stringResource(R.string.remove), color = Bip.danger) } },
        dismissButton = { TextButton({ removing = false }) { Text(stringResource(R.string.cancel)) } })

    Column(Modifier.fillMaxSize()) {
        TopBar(stringResource(R.string.agent_settings_of, agent.name), onBack)
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            item {
                Row(Modifier.fillMaxWidth().background(agent.look.tint, RoundedCornerShape(22.dp)).padding(16.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                    Mascot(agent.look, Modifier.size(56.dp))
                    Column {
                        Text(agent.name, style = Bip.text(19, FontWeight.Black))
                        Text(stringResource(categoryLabel(agent.look.category)), style = Bip.text(14, FontWeight.Bold, agent.look.deep))
                    }
                }
            }
            item {
                Group {
                    OptionRow(Icons.Rounded.Palette, stringResource(R.string.appearance), stringResource(categoryLabel(agent.look.category))) { picking = true }
                    OptionRow(Icons.Rounded.Language, stringResource(R.string.language), LANGUAGES.first { it.first == language(agent) }.second) { languages = true }
                    OptionRow(Icons.Rounded.RecordVoiceOver, stringResource(R.string.voice), voiceSummary(agent), onVoice)
                }
            }
            item {
                Group {
                    OptionRow(Icons.Rounded.Schedule, stringResource(R.string.routines), stringResource(R.string.routines_detail, agent.name), onRoutines)
                    if (managed == true) OptionRow(Icons.Rounded.Tune, stringResource(R.string.personality_tools),
                        stringResource(R.string.personality_tools_detail, agent.name), onSettings)
                    when {
                        desktop != null -> OptionRow(Icons.Rounded.Web, stringResource(R.string.desktop), stringResource(R.string.desktop_help), onDesktop)
                        desktopChecked -> OptionRow(Icons.Rounded.Web, stringResource(R.string.desktop), stringResource(R.string.desktop_none), null)
                        else -> OptionRow(Icons.Rounded.Web, stringResource(R.string.desktop), stringResource(R.string.loading), null)
                    }
                }
            }
            if (managed == false) item { Text(stringResource(R.string.set_by_hand_on_server, agent.name), style = Bip.text(13, color = Bip.muted)) }
            item {
                Box(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap { removing = true }.padding(16.dp)) {
                    Text(stringResource(R.string.remove_from_phone), style = Bip.text(16, FontWeight.ExtraBold, Bip.danger))
                }
            }
        }
    }
}

val LANGUAGES = listOf("fr" to "Français", "en" to "English", "es" to "Español", "de" to "Deutsch")

fun language(agent: Agent): String = agent.language?.takeIf { l -> LANGUAGES.any { it.first == l } }
    ?: java.util.Locale.getDefault().language.takeIf { l -> LANGUAGES.any { it.first == l } } ?: "en"

/** The Bips' voices (AgentVoices.bips), and each category's until one is picked. */
data class BipVoice(val id: String, val name: Int, val character: Int, val sample: String)

val BIP_VOICES = listOf(
    BipVoice("pocket:mousse", R.string.voice_mousse, R.string.voice_mousse_character, "voice_mousse"),
    BipVoice("pocket:colibri", R.string.voice_colibri, R.string.voice_colibri_character, "voice_colibri"),
    BipVoice("pocket:lumen", R.string.voice_lumen, R.string.voice_lumen_character, "voice_lumen"),
    BipVoice("pocket:galet", R.string.voice_galet, R.string.voice_galet_character, "voice_galet"),
    BipVoice("pocket:ours", R.string.voice_ours, R.string.voice_ours_character, "voice_ours"),
)

fun defaultVoice(c: Category) = when (c) {
    Category.WELLNESS -> "pocket:mousse"
    Category.DAILY -> "pocket:colibri"
    Category.CREATIVE, Category.LEARNING -> "pocket:lumen"
    Category.WORK, Category.TECH -> "pocket:galet"
    Category.FINANCE, Category.HOME -> "pocket:ours"
}

fun voiceOf(agent: Agent) = agent.voice?.takeIf { v -> BIP_VOICES.any { it.id == v } } ?: defaultVoice(agent.look.category)

@Composable
private fun voiceName(agent: Agent): String = stringResource(BIP_VOICES.first { it.id == voiceOf(agent) }.name)

/** « Voice model » or « Galet »: what speaks for the agent. */
@Composable
private fun voiceSummary(agent: Agent): String = if (agent.serverVoice != null) stringResource(R.string.your_voice_model) else voiceName(agent)

@Composable
private fun Group(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(vertical = 4.dp)) { content() }
}

@Composable
private fun OptionRow(icon: ImageVector, title: String, detail: String, onClick: (() -> Unit)?) {
    Row(Modifier.fillMaxWidth().then(if (onClick != null) Modifier.tap(onClick) else Modifier).padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Icon(icon, null, tint = if (onClick != null) Bip.accent else Bip.muted)
        Column(Modifier.weight(1f)) {
            Text(title, style = Bip.text(16, FontWeight.ExtraBold))
            Text(detail, style = Bip.text(13, FontWeight.Medium, Bip.ink2))
        }
        if (onClick != null) Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = Bip.muted)
    }
}

/** « Voix » (AgentVoicePicker): the person's voice model's voices if there is one (with the Bip relaying it), and the
 *  five Bips, each with its preview in the agent's language; one is checked. */
@Composable
fun VoiceScreen(store: AgentStore, agentId: String, onBack: () -> Unit) {
    val agents by store.agents.collectAsState()
    val agent = agents.firstOrNull { it.id == agentId } ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var playing by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf<String?>(null) }
    val player = remember { arrayOfNulls<MediaPlayer>(1) }
    var server by remember { mutableStateOf<io.github.bipagents.net.VoiceServer?>(null) }
    var relay by remember { mutableStateOf(false) }
    LaunchedEffect(agentId) { server = runCatching { store.bridge(agent)?.voiceServer() }.getOrNull() }
    DisposableEffect(Unit) { onDispose { player[0]?.release() } }
    val lang = language(agent)
    fun play(id: String, create: () -> MediaPlayer?) {
        player[0]?.release(); player[0] = null
        if (playing == id) { playing = null; return }
        player[0] = create()?.apply { setOnCompletionListener { playing = null }; start() }
        playing = if (player[0] != null) id else null
    }
    fun preview(v: BipVoice) = play(v.id) {
        val name = if (lang == "fr") v.sample else "${v.sample}_$lang"
        val id = context.resources.getIdentifier(name, "raw", context.packageName).takeIf { it != 0 }
            ?: context.resources.getIdentifier(v.sample, "raw", context.packageName)
        MediaPlayer.create(context, id)
    }
    // A voice model's voice: a sentence read by the bridge, kept for the screen.
    fun previewServer(id: String) {
        if (playing == id) return play(id) { null }
        val file = java.io.File(context.cacheDir, "voice-preview-" + id.map { if (it.isLetterOrDigit()) it else '_' }.joinToString("") + "-$lang.wav")
        if (file.exists()) return play(id) { MediaPlayer.create(context, android.net.Uri.fromFile(file)) }
        val bridge = store.bridge(agent) ?: return
        loading = id
        scope.launch {
            runCatching {
                val wav = bridge.voicePreview(previewSentence(lang), agent.bridgeName, "server:$lang/$id")
                kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { file.writeBytes(wav) }
                play(id) { MediaPlayer.create(context, android.net.Uri.fromFile(file)) }
            }
            loading = null
        }
    }
    @Composable
    fun PlayButton(id: String, onClick: () -> Unit) {
        Box(Modifier.size(38.dp).background(agent.look.tint, CircleShape).tap(onClick), contentAlignment = Alignment.Center) {
            if (loading == id) androidx.compose.material3.CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp, color = agent.look.deep)
            else Icon(if (playing == id) Icons.Rounded.Stop else Icons.Rounded.PlayArrow, stringResource(R.string.listen), tint = agent.look.deep)
        }
    }
    val serverVoices = server?.voices(lang) ?: emptyList()
    Column(Modifier.fillMaxSize()) {
        TopBar(stringResource(R.string.voice), onBack)
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            if (serverVoices.isNotEmpty()) {
                item { Text(stringResource(R.string.your_voice_model).uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted)) }
                item {
                    Group {
                        serverVoices.forEachIndexed { i, id ->
                            Row(Modifier.fillMaxWidth().tap { scope.launch { store.setVoices(agent, voiceOf(agent), id) } }
                                .padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                PlayButton(id) { previewServer(id) }
                                Text(stringResource(R.string.voice_n, i + 1), style = Bip.text(16, FontWeight.ExtraBold), modifier = Modifier.weight(1f))
                                if (agent.serverVoice == id) Icon(Icons.Rounded.Check, null, tint = Bip.accent)
                            }
                        }
                    }
                }
                if (agent.serverVoice != null) {
                    item { Group { OptionRow(Icons.Rounded.RecordVoiceOver, stringResource(R.string.relay), voiceName(agent)) { relay = true } } }
                    item {
                        val left = listOf("live" to R.string.use_live, "replies" to R.string.use_replies, "reading" to R.string.use_reading)
                            .filter { server?.takes(it.first) == false }.map { stringResource(it.second) }
                        Text(if (left.isEmpty()) stringResource(R.string.relay_help_all) else stringResource(R.string.relay_help, left.joinToString(", ")),
                            style = Bip.text(13, color = Bip.muted))
                    }
                }
            }
            item { Text(stringResource(R.string.bip_voices).uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted)) }
            item {
                Group {
                    for (v in BIP_VOICES) Row(Modifier.fillMaxWidth().tap { scope.launch { store.setVoices(agent, v.id, null) } }
                        .padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        PlayButton(v.id) { preview(v) }
                        Column(Modifier.weight(1f)) {
                            Text(stringResource(v.name), style = Bip.text(16, FontWeight.ExtraBold))
                            Text(stringResource(v.character), style = Bip.text(13, FontWeight.Medium, Bip.ink2))
                        }
                        if (agent.serverVoice == null && voiceOf(agent) == v.id) Icon(Icons.Rounded.Check, null, tint = Bip.accent)
                    }
                }
            }
            item { Text(stringResource(R.string.bip_voices_help), style = Bip.text(13, color = Bip.muted)) }
        }
    }
    // The Bip that speaks where the voice model doesn't (and when its server doesn't answer).
    if (relay) AlertDialog(onDismissRequest = { relay = false }, containerColor = Bip.card,
        title = { Text(stringResource(R.string.relay), style = Bip.text(20, FontWeight.Black)) },
        text = {
            Column {
                for (v in BIP_VOICES) Row(Modifier.fillMaxWidth().tap { relay = false; scope.launch { store.setVoices(agent, v.id, agent.serverVoice) } }
                    .padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    PlayButton(v.id) { preview(v) }
                    Text(stringResource(v.name), style = Bip.text(17, FontWeight.Bold), modifier = Modifier.weight(1f))
                    if (voiceOf(agent) == v.id) Icon(Icons.Rounded.Check, null, tint = Bip.accent)
                }
            }
        }, confirmButton = {})
}

/** What a voice says as its preview, in the agent's language (not the phone's). */
private fun previewSentence(lang: String) = when (lang) {
    "en" -> "Hi there! I looked at your day: three meetings, and a little time for a walk this afternoon."
    "es" -> "¡Holi! He mirado tu día: tres citas y un rato para caminar esta tarde."
    "de" -> "Huhu! Ich habe mir deinen Tag angesehen: drei Termine und ein bisschen Zeit für einen Spaziergang heute Nachmittag."
    else -> "Coucou ! J’ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi."
}
