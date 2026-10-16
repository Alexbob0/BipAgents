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
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Dns
import androidx.compose.material.icons.rounded.FolderShared
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Speed
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.material.icons.rounded.Check
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

/** Settings (SettingsView.swift): the agents, what passes between them, security, automations, voice, conversations,
 *  appearance, the app. */
@Composable
fun SettingsScreen(store: AgentStore, onAdd: () -> Unit, onAgent: (String) -> Unit, onPage: (String) -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val agents by store.agents.collectAsState()
    val reach by store.reach.collectAsState()
    var removing by remember { mutableStateOf<Agent?>(null) }
    var voiceModel by remember { mutableStateOf(false) }
    androidx.compose.runtime.LaunchedEffect(agents) {
        voiceModel = store.bridges().any { (b, _) -> runCatching { b.voiceServer() }.getOrNull() != null }
    }
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
            var menu by remember { mutableStateOf(false) }
            androidx.compose.foundation.layout.Box {
                Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp))
                    .pointerInput(agent.id) { detectTapGestures(onTap = { onAgent(agent.id) }, onLongPress = { menu = true }) }.padding(12.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Mascot(agent.look, Modifier.size(40.dp), animated = false)
                    Column(Modifier.weight(1f)) {
                        Text(agent.name, style = Bip.text(16, FontWeight.ExtraBold))
                        Text(android.net.Uri.parse(agent.baseUrl).host ?: agent.baseUrl, style = Bip.text(13, color = Bip.muted),
                            maxLines = 1, overflow = TextOverflow.MiddleEllipsis)
                    }
                    ReachChip(reach[agent.id])
                    Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = Bip.muted)
                }
                // Long press: the agents' order (home and tabs), or removing one from this phone.
                androidx.compose.material3.DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                    val index = agents.indexOf(agent)
                    if (index > 0) androidx.compose.material3.DropdownMenuItem({ Text(stringResource(R.string.move_up), style = Bip.text(16, FontWeight.Bold)) },
                        { menu = false; store.move(index, index - 1) })
                    if (index < agents.lastIndex) androidx.compose.material3.DropdownMenuItem({ Text(stringResource(R.string.move_down), style = Bip.text(16, FontWeight.Bold)) },
                        { menu = false; store.move(index, index + 1) })
                    androidx.compose.material3.DropdownMenuItem({ Text(stringResource(R.string.remove), style = Bip.text(16, FontWeight.Bold, Bip.danger)) },
                        { menu = false; removing = agent })
                }
            }
        }
        item {
            Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap(onAdd).padding(14.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Icon(Icons.Rounded.Add, null, tint = Bip.accent)
                Text(stringResource(R.string.add_agent), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent))
            }
        }
        item { SectionTitle(stringResource(R.string.between_agents)) }
        item {
            CardColumn {
                NavRow(Icons.Rounded.SwapHoriz, stringResource(R.string.who_writes)) { onPage("links") }
                NavRow(Icons.Rounded.FolderShared, stringResource(R.string.who_reads)) { onPage("shares") }
                NavRow(Icons.Rounded.Home, stringResource(R.string.family)) { onPage("family") }
            }
        }
        item { SectionNote(stringResource(R.string.between_agents_help)) }
        item { SectionTitle(stringResource(R.string.security)) }
        item {
            CardColumn {
                NavRow(Icons.Rounded.Lock, stringResource(R.string.vault)) { onPage("vault") }
                NavRow(Icons.Rounded.Computer, stringResource(R.string.browsers)) { onPage("browsers") }
            }
        }
        item { SectionNote(stringResource(R.string.security_help)) }
        item { SectionTitle(stringResource(R.string.automations)) }
        item { CardColumn { NavRow(Icons.Rounded.Schedule, stringResource(R.string.scheduled_tasks)) { onPage("cron") } } }
        item { VoiceReplySetting() }
        if (voiceModel) item { CardColumn { NavRow(Icons.Rounded.Dns, stringResource(R.string.your_voice_model)) { onPage("voicemodel") } } }
        item { ConversationSettings() }
        item { SectionTitle(stringResource(R.string.appearance_section)) }
        item {
            CardColumn {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.mode), style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
                    Row(Modifier.background(Bip.ground, RoundedCornerShape(50)).padding(3.dp)) {
                        for ((mode, label) in listOf(Appearance.SYSTEM to R.string.mode_auto, Appearance.LIGHT to R.string.mode_light,
                            Appearance.DARK to R.string.mode_dark)) {
                            val on = Appearance.mode == mode
                            Text(stringResource(label), style = Bip.text(14, FontWeight.ExtraBold, if (on) Bip.card else Bip.ink2),
                                modifier = Modifier.background(if (on) Bip.ink else androidx.compose.ui.graphics.Color.Transparent, RoundedCornerShape(50))
                                    .tap { Appearance.set(context, mode) }.padding(horizontal = 12.dp, vertical = 6.dp))
                        }
                    }
                }
                var sounds by remember { mutableStateOf(Babble.enabled(context)) }
                ToggleRow(stringResource(R.string.bip_sounds), sounds, { sounds = it; Babble.setEnabled(context, it) })
            }
        }
        item { SectionNote(stringResource(R.string.appearance_help)) }
        item { SectionTitle(stringResource(R.string.application)) }
        item {
            CardColumn {
                NavRow(Icons.Rounded.Settings, stringResource(R.string.app_permissions)) {
                    context.startActivity(android.content.Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                        android.net.Uri.fromParts("package", context.packageName, null)))
                }
                NavRow(Icons.Rounded.Speed, stringResource(R.string.diagnostics_latencies)) { onPage("diagnostics") }
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

/** When a voice note gets a spoken answer (Réglages › Voix on the iPhone). */
@Composable
private fun VoiceReplySetting() {
    val context = androidx.compose.ui.platform.LocalContext.current
    var choice by remember { mutableStateOf(io.github.bipagents.voice.VoiceReply.current(context)) }
    Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.voice_reply).uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted))
        Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(vertical = 4.dp)) {
            for ((value, label) in listOf(io.github.bipagents.voice.VoiceReply.NEVER to R.string.voice_never,
                io.github.bipagents.voice.VoiceReply.SMART to R.string.voice_smart, io.github.bipagents.voice.VoiceReply.ALWAYS to R.string.voice_always)) {
                Row(Modifier.fillMaxWidth().tap { choice = value; io.github.bipagents.voice.VoiceReply.set(context, value) }.padding(horizontal = 16.dp, vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(label), style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
                    if (choice == value) Icon(androidx.compose.material.icons.Icons.Rounded.Check, null, tint = Bip.accent)
                }
            }
        }
        Text(stringResource(R.string.voice_reply_help), style = Bip.text(13, color = Bip.muted))
    }
}

/** « Discussions »: the agents' work folded under each reply, or hidden. */
@Composable
private fun ConversationSettings() {
    val context = androidx.compose.ui.platform.LocalContext.current
    var work by remember { mutableStateOf(WorkDetail.shown(context)) }
    Column(Modifier.padding(top = 14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.conversations).uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted))
        Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(horizontal = 16.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(R.string.work_detail), style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
            androidx.compose.material3.Switch(work, { work = it; WorkDetail.set(context, it) },
                colors = androidx.compose.material3.SwitchDefaults.colors(checkedTrackColor = Bip.accent))
        }
        Text(stringResource(R.string.work_detail_help), style = Bip.text(13, color = Bip.muted))
    }
}
