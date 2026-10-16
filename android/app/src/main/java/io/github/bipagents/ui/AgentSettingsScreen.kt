package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.MenuBook
import androidx.compose.material.icons.rounded.Build
import androidx.compose.material.icons.rounded.Checklist
import androidx.compose.material.icons.rounded.Code
import androidx.compose.material.icons.rounded.Description
import androidx.compose.material.icons.rounded.ExpandLess
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Extension
import androidx.compose.material.icons.rounded.Groups
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.ImageSearch
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material.icons.rounded.Lightbulb
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.Palette
import androidx.compose.material.icons.rounded.Psychology
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Terminal
import androidx.compose.material.icons.rounded.TravelExplore
import androidx.compose.material.icons.rounded.Visibility
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.AgentSettings
import io.github.bipagents.net.CannotHold
import io.github.bipagents.net.CatalogService
import io.github.bipagents.net.Toolset
import kotlinx.coroutines.launch

/** How a Hermes toolset reads in the app (ToolInfo): the everyday ones first, in plain words. */
data class ToolInfo(val id: String, val label: Int, val icon: ImageVector, val detail: Int?, val sensitive: Boolean = false) {
    companion object {
        val main = listOf(
            ToolInfo("web", R.string.tool_web, Icons.Rounded.Search, R.string.tool_web_detail),
            ToolInfo("browser", R.string.tool_browser, Icons.Rounded.TravelExplore, R.string.tool_browser_detail, sensitive = true),
            ToolInfo("vision", R.string.tool_vision, Icons.Rounded.Visibility, R.string.tool_vision_detail),
            ToolInfo("image_gen", R.string.tool_image_gen, Icons.Rounded.Palette, null),
            ToolInfo("memory", R.string.tool_memory, Icons.Rounded.Psychology, R.string.tool_memory_detail),
            ToolInfo("session_search", R.string.tool_session_search, Icons.Rounded.ImageSearch, null),
            ToolInfo("cronjob", R.string.tool_cronjob, Icons.Rounded.Schedule, R.string.tool_cronjob_detail),
            ToolInfo("delegation", R.string.tool_delegation, Icons.Rounded.Groups, R.string.tool_delegation_detail),
            ToolInfo("todo", R.string.tool_todo, Icons.Rounded.Checklist, null),
            ToolInfo("skills", R.string.tool_skills, Icons.AutoMirrored.Rounded.MenuBook, null),
            ToolInfo("file", R.string.tool_file, Icons.Rounded.Description, R.string.on_the_server, sensitive = true),
            ToolInfo("terminal", R.string.tool_terminal, Icons.Rounded.Terminal, R.string.on_the_server, sensitive = true),
            ToolInfo("code_execution", R.string.tool_code, Icons.Rounded.Code, R.string.on_the_server, sensitive = true),
        )
        /** Tools that act on the machine: turning one on asks first. */
        val sensitive = setOf("browser", "file", "terminal", "code_execution", "computer_use")
        /** Not a choice for the person: the app's questions go through it. */
        val hidden = setOf("clarify")

        /** « 🔍 Web Search & Scraping » → « Web Search & Scraping ». */
        fun plain(label: String) = label.dropWhile { !it.isLetterOrDigit() }.trim()
    }
}

/** An agent this install created, changed from the app (AgentSettingsView): personality, thinking, tools, services,
 *  its own space, deleting it. Hermes reads them on each run: a change counts from the next message. */
@Composable
fun AgentSettingsScreen(store: AgentStore, agentId: String, onBack: () -> Unit, onDeleted: () -> Unit) {
    val agents by store.agents.collectAsState()
    val agent = agents.firstOrNull { it.id == agentId } ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val bridge = store.bridge(agent)
    var settings by remember { mutableStateOf<AgentSettings?>(null) }
    var soul by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }
    var savedSoul by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var others by remember { mutableStateOf(false) }
    var confirming by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    var confirmMove by remember { mutableStateOf(false) }
    var moving by remember { mutableStateOf(false) }
    var connecting by remember { mutableStateOf(false) }
    LaunchedEffect(agentId) {
        try {
            settings = bridge?.agentSettings(agent.bridgeName)
            soul = settings?.soul ?: ""
        } catch (e: Throwable) { error = describe(context, e) }
    }
    fun setTool(id: String, on: Boolean) {
        val before = settings ?: return
        settings = before.copy(toolsets = before.toolsets.map { if (it.id == id) it.copy(enabled = on) else it })
        saving = true
        scope.launch {
            try {
                bridge?.updateAgentSettings(agent.bridgeName, toolsets = mapOf(id to on))?.let { s -> settings = settings?.copy(toolsets = s.toolsets) }
                error = null
            } catch (e: Throwable) {
                settings = before
                error = describe(context, e)
            }
            saving = false
        }
    }
    val s = settings
    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar(stringResource(R.string.personality_tools), onBack) {
            if (saving) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = agent.look.deep)
            else if (savedSoul) Text("✓", style = Bip.text(20, FontWeight.Black, agent.look.deep))
            else if (s != null && soul.isNotBlank() && soul != s.soul) Text(stringResource(R.string.save), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent),
                modifier = Modifier.tap {
                    saving = true
                    scope.launch {
                        try {
                            bridge?.updateAgentSettings(agent.bridgeName, soul = soul)
                            settings = settings?.copy(soul = soul); savedSoul = true; error = null
                        } catch (e: Throwable) { error = describe(context, e) }
                        saving = false
                    }
                }.padding(8.dp))
        }
        if (s == null) {
            Text(error ?: stringResource(R.string.loading), style = Bip.text(15, color = if (error != null) Bip.danger else Bip.muted),
                modifier = Modifier.padding(16.dp))
            return@Column
        }
        LazyColumn(contentPadding = PaddingValues(16.dp, 0.dp, 16.dp, 40.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            error?.let { item { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) } }
            item { Header(stringResource(R.string.personality)) }
            item { Field(soul, { soul = it; savedSoul = false }, "", minLines = 8) }
            item { Footer(stringResource(R.string.personality_help, agent.name)) }
            s.thinking?.let { thinking ->
                item {
                    var menu by remember { mutableStateOf(false) }
                    Box(Modifier.padding(top = 12.dp)) {
                        Card { SettingRow(stringResource(R.string.thinking), thinkingName(thinking)) { menu = true } }
                        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                            for (level in listOf("none", "low", "medium", "high", "xhigh")) DropdownMenuItem(
                                { Text((if (level == thinking) "✓  " else "") + thinkingName(level), style = Bip.text(16, FontWeight.Bold)) }, {
                                    menu = false
                                    settings = settings?.copy(thinking = level)
                                    scope.launch {
                                        runCatching { bridge?.setThinking(agent.bridgeName, level) }.onFailure { e ->
                                            settings = settings?.copy(thinking = thinking); error = describe(context, e)
                                        }
                                    }
                                })
                        }
                    }
                }
                item { Footer(stringResource(R.string.thinking_help, agent.name)) }
            }
            if (s.byHand) {
                item { Card { Row(Modifier.padding(16.dp)) { Text(stringResource(R.string.set_up_by_hand), style = Bip.text(16, FontWeight.Bold)) } } }
                item { Footer(stringResource(R.string.set_up_by_hand_help)) }
                return@LazyColumn
            }
            item { Header(stringResource(R.string.what_it_can_do, agent.name)) }
            item {
                Card {
                    for (info in ToolInfo.main) s.toolsets.firstOrNull { it.id == info.id }?.let { t ->
                        ToolRow(stringResource(info.label), info.detail?.let { stringResource(it) }, info.icon, t, agent, saving) { on ->
                            if (on && info.sensitive) confirming = info.id else setTool(info.id, on)
                        }
                    }
                    Row(Modifier.fillMaxWidth().tap { others = !others }.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.other_tools), style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
                        Icon(if (others) Icons.Rounded.ExpandLess else Icons.Rounded.ExpandMore, null, tint = Bip.muted)
                    }
                    if (others) for (t in s.toolsets.filter { t -> ToolInfo.main.none { it.id == t.id } && t.id !in ToolInfo.hidden }) {
                        ToolRow(ToolInfo.plain(t.label), t.description.ifBlank { null }, Icons.Rounded.Extension, t, agent, saving) { on ->
                            if (on && t.id in ToolInfo.sensitive) confirming = t.id else setTool(t.id, on)
                        }
                    }
                }
            }
            item { Footer(stringResource(R.string.tools_help)) }
            item { Header(stringResource(R.string.connected_services)) }
            item {
                ServicesCard(agent, s, bridge, onChanged = { updated -> settings = updated }) { connecting = true }
            }
            item { Footer(stringResource(R.string.services_help, agent.name)) }
            item { Header(stringResource(R.string.own_space)) }
            item {
                Card {
                    when {
                        s.isolated -> Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            Icon(Icons.Rounded.Lock, null, tint = agent.look.deep)
                            Text(stringResource(R.string.has_own_space), style = Bip.text(16, FontWeight.Bold))
                        }
                        moving -> Row(Modifier.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp, color = agent.look.deep)
                            Text(stringResource(R.string.moving_to_own_space), style = Bip.text(16, FontWeight.Bold))
                        }
                        else -> Row(Modifier.fillMaxWidth().tap { confirmMove = true }.padding(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            Icon(Icons.Rounded.Lock, null, tint = Bip.accent)
                            Text(stringResource(R.string.give_own_space), style = Bip.text(16, FontWeight.Bold, Bip.accent))
                        }
                    }
                }
            }
            item { Footer(stringResource(if (s.isolated) R.string.own_space_help_isolated else R.string.own_space_help_shared, agent.name)) }
            item {
                Box(Modifier.padding(top = 12.dp).fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap { if (!deleting) confirmDelete = true }
                    .padding(16.dp)) {
                    Text(stringResource(R.string.delete_agent, agent.name), style = Bip.text(16, FontWeight.ExtraBold, Bip.danger))
                }
            }
            item { Footer(stringResource(R.string.delete_agent_help)) }
        }
    }
    confirming?.let { id ->
        val label = ToolInfo.main.firstOrNull { it.id == id }?.let { stringResource(it.label) } ?: ToolInfo.plain(s?.toolsets?.firstOrNull { it.id == id }?.label ?: id)
        AlertDialog({ confirming = null }, title = { Text(stringResource(R.string.give_tool, label, agent.name)) },
            text = { Text(stringResource(if (s?.protected != false) R.string.give_tool_protected else R.string.give_tool_unprotected, agent.name)) },
            confirmButton = { TextButton({ confirming = null; setTool(id, true) }) { Text(stringResource(R.string.turn_on)) } },
            dismissButton = { TextButton({ confirming = null }) { Text(stringResource(R.string.cancel)) } })
    }
    if (confirmMove) AlertDialog({ confirmMove = false }, title = { Text(stringResource(R.string.give_own_space_title, agent.name)) },
        text = { Text(stringResource(R.string.own_space_explanation, agent.name)) },
        confirmButton = { TextButton({
            confirmMove = false; moving = true
            scope.launch {
                try {
                    bridge?.moveToOwnSpace(agent.bridgeName)?.let { url -> store.relocate(agent, url) }
                    settings = settings?.copy(isolated = true); error = null
                } catch (e: Throwable) { error = describe(context, e) }
                moving = false
            }
        }) { Text(stringResource(R.string.give_own_space_confirm)) } },
        dismissButton = { TextButton({ confirmMove = false }) { Text(stringResource(R.string.cancel)) } })
    if (confirmDelete) AlertDialog({ confirmDelete = false }, title = { Text(stringResource(R.string.delete_agent_title, agent.name)) },
        text = { Text(stringResource(R.string.delete_agent_body)) },
        confirmButton = { TextButton({
            confirmDelete = false; deleting = true
            scope.launch {
                try {
                    bridge?.deleteAgent(agent.bridgeName)
                    store.remove(agent)
                    onDeleted()
                } catch (e: Throwable) { error = describe(context, e) }
                deleting = false
            }
        }) { Text(stringResource(R.string.delete_agent, agent.name), color = Bip.danger) } },
        dismissButton = { TextButton({ confirmDelete = false }) { Text(stringResource(R.string.cancel)) } })
    if (connecting && s != null) ConnectService(agent, s.isolated, bridge, onDismiss = { connecting = false },
        onMoveToOwnSpace = { connecting = false; confirmMove = true }) {
        connecting = false
        scope.launch { runCatching { bridge?.agentSettings(agent.bridgeName) }.getOrNull()?.let { settings = it } }
    }
}

@Composable
private fun thinkingName(level: String) = stringResource(when (level) {
    "none" -> R.string.thinking_none; "low" -> R.string.thinking_low; "high" -> R.string.thinking_high
    "xhigh" -> R.string.thinking_xhigh; else -> R.string.thinking_medium
})

@Composable
private fun Header(text: String) = Text(text.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.padding(top = 14.dp))

@Composable
private fun Footer(text: String) = Text(text, style = Bip.text(13, color = Bip.muted))

@Composable
private fun Card(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(vertical = 4.dp)) { content() }
}

@Composable
private fun ToolRow(label: String, detail: String?, icon: ImageVector, toolset: Toolset, agent: Agent, saving: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Icon(icon, null, tint = if (toolset.configured) agent.look.deep else Bip.muted)
        Column(Modifier.weight(1f)) {
            Text(label, style = Bip.text(15, FontWeight.Bold, if (toolset.configured) Bip.ink else Bip.muted))
            detail?.let { Text(it, style = Bip.text(13, color = Bip.ink2), maxLines = 3) }
        }
        Switch(toolset.enabled, onChange, enabled = toolset.configured && !saving,
            colors = SwitchDefaults.colors(checkedTrackColor = agent.look.deep))
    }
}

/** « Connected services »: what the agent reaches beyond its tools (home automation, GitHub, any MCP server). */
@Composable
private fun ServicesCard(agent: Agent, settings: AgentSettings, bridge: io.github.bipagents.net.Bridge?, onChanged: (AgentSettings) -> Unit,
                         onAdd: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val results = remember { mutableStateMapOf<String, String>() }
    var testing by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val reached = stringResource(R.string.service_reached)
    val reachedTools = stringResource(R.string.service_reached_tools)
    val unreachable = stringResource(R.string.service_unreachable)
    Card {
        for (service in settings.services) {
            var menu by remember { mutableStateOf(false) }
            Box {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Icon(when (service.catalog) { "home_assistant" -> Icons.Rounded.Home; "github" -> Icons.Rounded.Code; null -> Icons.Rounded.Terminal
                        else -> Icons.Rounded.Extension }, null, tint = agent.look.deep)
                    Column(Modifier.weight(1f)) {
                        Text(service.id, style = Bip.text(15, FontWeight.Bold))
                        Text(results[service.id] ?: service.url?.let { android.net.Uri.parse(it).host } ?: stringResource(R.string.installed_on_server),
                            style = Bip.text(13, color = Bip.ink2), maxLines = 2)
                    }
                    if (testing == service.id) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    else if (service.catalog != null) {
                        Text(stringResource(R.string.test), style = Bip.text(15, FontWeight.ExtraBold, Bip.accent), modifier = Modifier.tap {
                            testing = service.id
                            scope.launch {
                                results[service.id] = try {
                                    val (ok, message, tools) = bridge!!.testService(service.id, agent.bridgeName)
                                    if (ok) tools?.let { reachedTools.format(it) } ?: reached else unreachable.format(message ?: "?")
                                } catch (e: Throwable) { describe(context, e) }
                                testing = null
                            }
                        }.padding(6.dp))
                        Text("⋯", style = Bip.text(20, FontWeight.Black, Bip.muted), modifier = Modifier.tap { menu = true }.padding(6.dp))
                    }
                }
                DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                    DropdownMenuItem({ Text(stringResource(R.string.remove), style = Bip.text(16, FontWeight.Bold, Bip.danger)) }, {
                        menu = false
                        scope.launch {
                            try {
                                bridge?.disconnectService(service.id, agent.bridgeName)
                                bridge?.agentSettings(agent.bridgeName)?.let(onChanged)
                                error = null
                            } catch (e: Throwable) { error = describe(context, e) }
                        }
                    })
                }
            }
        }
        Text("+  " + stringResource(R.string.connect_service), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent),
            modifier = Modifier.fillMaxWidth().tap(onAdd).padding(16.dp))
        error?.let { Text(it, style = Bip.text(13, color = Bip.danger), modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp)) }
    }
}

/** The catalog, then the chosen service's fields (its token hidden). */
@Composable
private fun ConnectService(agent: Agent, isolated: Boolean, bridge: io.github.bipagents.net.Bridge?, onDismiss: () -> Unit,
                           onMoveToOwnSpace: () -> Unit, onSaved: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var catalog by remember { mutableStateOf<List<CatalogService>?>(null) }
    var chosen by remember { mutableStateOf<CatalogService?>(null) }
    val values = remember { mutableStateMapOf<String, String>() }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var needsOwnSpace by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { try { catalog = bridge?.toolCatalog() ?: emptyList() } catch (e: Throwable) { error = describe(context, e); catalog = emptyList() } }
    val c = chosen
    val ready = c != null && !saving && c.fields.all { it.optional || !values[it.id].isNullOrBlank() }
    Dialog(onDismiss, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(Bip.ground).statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.cancel), style = Bip.text(16, FontWeight.Bold, Bip.ink2), modifier = Modifier.tap(onDismiss).padding(8.dp))
                Text(stringResource(R.string.connect_service), style = Bip.text(17, FontWeight.ExtraBold), modifier = Modifier.weight(1f),
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                if (c != null) Text(stringResource(R.string.connect), style = Bip.text(16, FontWeight.ExtraBold, if (ready) Bip.accent else Bip.muted),
                    modifier = Modifier.tap {
                        if (!ready) return@tap
                        saving = true
                        scope.launch {
                            try {
                                bridge?.connectService(c.id, values.toMap(), agent.bridgeName)
                                onSaved()
                            } catch (refused: CannotHold) {
                                needsOwnSpace = "needs_own_space" in refused.reasons.values
                                error = refused.reasons.values.firstOrNull()?.let { why -> holdReason(context, why, agent.name) }
                            } catch (e: Throwable) {
                                if (e is kotlinx.coroutines.CancellationException) throw e
                                error = describe(context, e)
                            }
                            saving = false
                        }
                    }.padding(8.dp))
                else Box(Modifier.size(60.dp, 1.dp))
            }
            Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                error?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
                if (needsOwnSpace) {
                    Text("🔒  " + stringResource(R.string.give_own_space), style = Bip.text(16, FontWeight.ExtraBold, Bip.accent),
                        modifier = Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap(onMoveToOwnSpace).padding(16.dp))
                    Text(stringResource(R.string.service_needs_own_space, agent.name), style = Bip.text(13, color = Bip.muted))
                }
                if (c != null) {
                    Text(c.label, style = Bip.text(18, FontWeight.ExtraBold))
                    Text(c.description, style = Bip.text(14, color = Bip.ink2))
                    for (field in c.fields) Field(values[field.id] ?: "", { values[field.id] = it },
                        if (field.placeholder.isEmpty() || field.secret) field.label else "${field.label} (${field.placeholder})", secret = field.secret)
                } else if (catalog == null) Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted))
                else for (service in catalog!!) Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).tap {
                    chosen = service; values.clear(); needsOwnSpace = service.personal && !isolated
                }.padding(16.dp)) {
                    Text(service.label, style = Bip.text(16, FontWeight.ExtraBold))
                    Text(service.description, style = Bip.text(13, color = Bip.ink2), maxLines = 3)
                }
            }
        }
    }
}

/** Why an agent can't hold a credential (VaultItemEditor.reason). */
fun holdReason(context: android.content.Context, why: String, name: String): String = context.getString(when (why) {
    "needs_own_space" -> R.string.hold_needs_own_space
    "needs_sandbox" -> R.string.hold_needs_sandbox
    "unprotected_install" -> R.string.hold_unprotected
    else -> R.string.hold_not_configured
}, name)
