package io.github.bipagents.ui

import android.content.Intent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Home
import androidx.compose.material.icons.rounded.Key
import androidx.compose.material.icons.rounded.Lock
import androidx.compose.material.icons.rounded.QrCode
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Share
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
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.AgentLink
import io.github.bipagents.net.Bridge
import io.github.bipagents.net.CannotHold
import io.github.bipagents.net.ConsoleDevice
import io.github.bipagents.net.CronJob
import io.github.bipagents.net.Family
import io.github.bipagents.net.LinkAgent
import io.github.bipagents.net.ShareAgent
import io.github.bipagents.net.ShareRead
import io.github.bipagents.net.Vault
import io.github.bipagents.net.VaultItem
import kotlinx.coroutines.launch

// -- the pieces every settings page uses ---------------------------------------------------------------------------

@Composable
fun SettingsPage(title: String, onBack: () -> Unit, actions: @Composable () -> Unit = {}, content: LazyListScope.() -> Unit) {
    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar(title, onBack, actions = actions)
        LazyColumn(contentPadding = PaddingValues(16.dp, 0.dp, 16.dp, 48.dp), verticalArrangement = Arrangement.spacedBy(8.dp), content = content)
    }
}

@Composable
fun SectionTitle(text: String) = Text(text.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.padding(top = 14.dp))

@Composable
fun SectionNote(text: String, color: Color = Bip.muted) = Text(text, style = Bip.text(13, color = color))

@Composable
fun CardColumn(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(vertical = 4.dp)) { content() }
}

@Composable
fun ToggleRow(title: String, checked: Boolean, onChange: (Boolean) -> Unit, subtitle: String? = null, enabled: Boolean = true, tint: Color = Bip.accent) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = Bip.text(16, FontWeight.Bold, if (enabled) Bip.ink else Bip.muted))
            subtitle?.let { Text(it, style = Bip.text(13, color = Bip.ink2)) }
        }
        Switch(checked, onChange, enabled = enabled, colors = SwitchDefaults.colors(checkedTrackColor = tint))
    }
}

@Composable
fun NavRow(icon: ImageVector, title: String, detail: String? = null, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().tap(onClick).padding(horizontal = 16.dp, vertical = 13.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp)) {
        Icon(icon, null, tint = Bip.accent)
        Column(Modifier.weight(1f)) {
            Text(title, style = Bip.text(16, FontWeight.Bold))
            detail?.let { Text(it, style = Bip.text(13, color = Bip.ink2)) }
        }
        Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = Bip.muted)
    }
}

@Composable
private fun Loading() = Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted))

/** The first of the person's bridges that answers `ask` (null: too old), with its answer. */
private suspend fun <T> firstBridge(store: AgentStore, ask: suspend (Bridge) -> T?): Triple<Bridge?, T?, Throwable?> {
    var last: Throwable? = null
    for ((bridge, _) in store.bridges()) {
        try {
            ask(bridge)?.let { return Triple(bridge, it, null) }
        } catch (e: Throwable) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            last = e
        }
    }
    return Triple(null, null, last)
}

// -- who may write to whom ---------------------------------------------------------------------------------------

/** « Who can write to whom »: agent by agent, which others it may message; the bridge carries those and refuses the rest. */
@Composable
fun AgentLinksScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var bridge by remember { mutableStateOf<Bridge?>(null) }
    var agents by remember { mutableStateOf<List<LinkAgent>?>(null) }
    var links by remember { mutableStateOf(setOf<AgentLink>()) }
    var needsSetup by remember { mutableStateOf(listOf<String>()) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val tooOld = stringResource(R.string.links_too_old)
    LaunchedEffect(Unit) {
        val (b, found, e) = firstBridge(store) { it.agentLinks() }
        bridge = b; agents = found?.first ?: emptyList(); links = found?.second ?: emptySet()
        error = e?.let { describe(context, it) } ?: if (found == null) tooOld else null
    }
    fun set(link: AgentLink, on: Boolean) {
        val before = links
        val next = if (on) links + link else links - link
        links = next; saving = true
        scope.launch {
            try { needsSetup = bridge!!.setAgentLinks(next); error = null } catch (e: Throwable) { links = before; error = describe(context, e) }
            saving = false
        }
    }
    SettingsPage(stringResource(R.string.who_writes), onBack) {
        error?.let { item { SectionNote(it, Bip.danger) } }
        if (agents == null) item { Loading() }
        val all = agents ?: emptyList()
        // Ours first, then the agents of linked households (one of theirs may only write to ours).
        for (sender in all.filter { it.household == null } + all.filter { it.household != null }) {
            item { SectionTitle(sender.name) }
            item {
                CardColumn {
                    for (target in all.filter { it.id != sender.id && (sender.household == null || it.household == null) }) {
                        val link = AgentLink(sender.id, target.id)
                        ToggleRow(stringResource(R.string.can_write_to, target.name), link in links, { set(link, it) }, enabled = !saving)
                    }
                }
            }
            if (sender.id in needsSetup) item { SectionNote(stringResource(R.string.links_needs_setup, sender.name, sender.id)) }
        }
        item { SectionNote(stringResource(R.string.links_help)) }
    }
}

// -- who may read what -------------------------------------------------------------------------------------------

/** « Who can read what »: agent by agent, whose shared folder it may read (read-only). */
@Composable
fun AgentSharesScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var bridge by remember { mutableStateOf<Bridge?>(null) }
    var agents by remember { mutableStateOf<List<ShareAgent>?>(null) }
    var reads by remember { mutableStateOf(setOf<ShareRead>()) }
    var error by remember { mutableStateOf<String?>(null) }
    var saving by remember { mutableStateOf(false) }
    val tooOld = stringResource(R.string.shares_too_old)
    LaunchedEffect(Unit) {
        val (b, found, e) = firstBridge(store) { it.agentShares() }
        bridge = b; agents = found?.first ?: emptyList(); reads = found?.second ?: emptySet()
        error = e?.let { describe(context, it) } ?: if (found == null) tooOld else null
    }
    fun set(read: ShareRead, on: Boolean) {
        val before = reads
        val next = if (on) reads + read else reads - read
        reads = next; saving = true
        scope.launch {
            try { reads = bridge!!.setAgentShares(next); error = null } catch (e: Throwable) { reads = before; error = describe(context, e) }
            saving = false
        }
    }
    val able = agents?.filter { it.whyNot == null } ?: emptyList()
    val unable = agents?.filter { it.whyNot != null } ?: emptyList()
    SettingsPage(stringResource(R.string.who_reads), onBack) {
        error?.let { item { SectionNote(it, Bip.danger) } }
        if (agents == null) item { Loading() }
        if (agents != null && able.size < 2 && error == null) item { SectionNote(stringResource(R.string.shares_need_two)) }
        if (able.size >= 2) for (reader in able) {
            item { SectionTitle(reader.name) }
            item {
                CardColumn {
                    for (source in able.filter { it.id != reader.id }) {
                        val read = ShareRead(reader.id, source.id)
                        ToggleRow(stringResource(R.string.can_read_from, source.name), read in reads, { set(read, it) }, enabled = !saving)
                    }
                }
            }
            if (reader.external) item { SectionNote(stringResource(R.string.shares_external, reader.name)) }
        }
        if (unable.isNotEmpty()) {
            item { SectionTitle(stringResource(R.string.no_sharing)) }
            item {
                CardColumn {
                    for (agent in unable) Column(Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) {
                        Text(agent.name, style = Bip.text(15, FontWeight.ExtraBold))
                        Text(stringResource(if (agent.whyNot == "no_sandbox") R.string.shares_no_sandbox else R.string.shares_by_hand),
                            style = Bip.text(13, color = Bip.ink2))
                    }
                }
            }
        }
        item { SectionNote(stringResource(R.string.shares_help)) }
    }
}

// -- family ------------------------------------------------------------------------------------------------------

/** « Family »: this install as a household, linked to a relative's own install (invitation QR code, scanned there). */
@Composable
fun FamilyScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var bridge by remember { mutableStateOf<Bridge?>(null) }
    var family by remember { mutableStateOf<Family?>(null) }
    var loaded by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var invitation by remember { mutableStateOf<String?>(null) }
    var scanning by remember { mutableStateOf(false) }
    var working by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val tooOld = stringResource(R.string.family_too_old)
    val notInvitation = stringResource(R.string.not_family_invitation)
    val linkedTo = stringResource(R.string.linked_to)
    suspend fun load() {
        val (b, found, e) = firstBridge(store) { it.family() }
        bridge = b; family = found; loaded = true
        if (name.isEmpty()) name = found?.name ?: ""
        error = e?.let { describe(context, it) } ?: if (found == null) tooOld else null
    }
    suspend fun saveName() {
        if (name.isBlank()) return
        runCatching { bridge?.setFamilyName(name.trim()) }.onFailure { error = describe(context, it) }
    }
    LaunchedEffect(Unit) { load() }
    val askCamera = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.RequestPermission()) {
        if (it) scanning = true
    }
    SettingsPage(stringResource(R.string.family), onBack) {
        error?.let { item { SectionNote(it, Bip.danger) } }
        message?.let { item { SectionNote(it, Bip.ink2) } }
        if (!loaded) item { Loading() }
        if (family != null) {
            item { SectionTitle(stringResource(R.string.your_household)) }
            item {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Box(Modifier.weight(1f)) { Field(name, { name = it }, stringResource(R.string.your_first_name)) }
                    Pill(stringResource(R.string.save), dark = false) { scope.launch { saveName() } }
                }
            }
            item { SectionNote(stringResource(R.string.household_help, name.ifBlank { "Alex" })) }
            item { SectionTitle(stringResource(R.string.linked_households)) }
            item {
                CardColumn {
                    if (family!!.households.isEmpty()) Text(stringResource(R.string.no_linked_household), style = Bip.text(15, color = Bip.ink2),
                        modifier = Modifier.padding(16.dp))
                    for ((id, label) in family!!.households) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Icon(Icons.Rounded.Home, null, tint = Bip.accent)
                        Text(label, style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
                        Icon(Icons.Rounded.Close, stringResource(R.string.remove), tint = Bip.muted, modifier = Modifier.tap {
                            scope.launch { runCatching { bridge?.unlinkHousehold(id) }.onFailure { error = describe(context, it) }; load() }
                        })
                    }
                }
            }
            item {
                CardColumn {
                    val ready = !working && name.isNotBlank()
                    NavRow(Icons.Rounded.QrCode, stringResource(R.string.invite_relative)) {
                        if (!ready) return@NavRow
                        working = true
                        scope.launch {
                            saveName()
                            try { invitation = bridge?.familyInvitation() } catch (e: Throwable) { error = describe(context, e) }
                            working = false
                        }
                    }
                    NavRow(Icons.Rounded.QrCodeScanner, stringResource(R.string.scan_invitation)) {
                        if (ready) askCamera.launch(android.Manifest.permission.CAMERA)
                    }
                }
            }
            item { SectionNote(stringResource(R.string.family_help)) }
        }
    }
    invitation?.let { payload ->
        val qr = remember(payload) { qrCode(payload) }
        AlertDialog({ invitation = null }, confirmButton = { TextButton({ invitation = null }) { Text("OK") } },
            title = { Text(stringResource(R.string.invitation_of, name)) },
            text = {
                Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Image(qr, null, Modifier.size(240.dp).background(Color.White, RoundedCornerShape(16.dp)).padding(12.dp))
                    Text(stringResource(R.string.invitation_where), style = Bip.text(15), textAlign = TextAlign.Center)
                    Text(stringResource(R.string.invitation_valid), style = Bip.text(13, color = Bip.ink2))
                }
            })
    }
    if (scanning) Dialog({ scanning = false }, DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            Scanner { payload ->
                scanning = false
                val json = runCatching { io.github.bipagents.net.parseJson(payload) as? kotlinx.serialization.json.JsonObject }.getOrNull()
                val url = json?.let { if ((it["kind"] as? kotlinx.serialization.json.JsonPrimitive)?.content == "family") (it["url"] as? kotlinx.serialization.json.JsonPrimitive)?.content else null }
                val code = (json?.get("code") as? kotlinx.serialization.json.JsonPrimitive)?.content
                if (url == null || code.isNullOrEmpty()) { error = notInvitation; return@Scanner }
                working = true
                scope.launch {
                    saveName()
                    try {
                        val other = bridge?.joinFamily(url, code) ?: (json["name"] as? kotlinx.serialization.json.JsonPrimitive)?.content ?: ""
                        message = linkedTo.format(other); error = null
                        load()
                    } catch (e: Throwable) { error = describe(context, e) }
                    working = false
                }
            }
            Row(Modifier.fillMaxWidth().statusBarsPadding().padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                RoundButton(Icons.Rounded.Close, stringResource(R.string.close), { scanning = false }, size = 44.dp,
                    background = Color.Black.copy(alpha = 0.4f), tint = Color.White)
                Text(stringResource(R.string.scan_relative_invitation), style = Bip.text(16, FontWeight.ExtraBold, Color.White),
                    modifier = Modifier.weight(1f), textAlign = TextAlign.Center)
            }
        }
    }
}

/** A QR code (zxing), black on white. */
fun qrCode(text: String, size: Int = 640): ImageBitmap {
    val matrix = com.google.zxing.qrcode.QRCodeWriter().encode(text, com.google.zxing.BarcodeFormat.QR_CODE, size, size,
        mapOf(com.google.zxing.EncodeHintType.ERROR_CORRECTION to com.google.zxing.qrcode.decoder.ErrorCorrectionLevel.M,
            com.google.zxing.EncodeHintType.MARGIN to 1))
    val pixels = IntArray(size * size) { i -> if (matrix[i % size, i / size]) android.graphics.Color.BLACK else android.graphics.Color.WHITE }
    return android.graphics.Bitmap.createBitmap(pixels, size, size, android.graphics.Bitmap.Config.ARGB_8888).asImageBitmap()
}

// -- scheduled tasks ---------------------------------------------------------------------------------------------

/** The agents' scheduled tasks (seen by the bridge) and whether each notifies; a muted one still lands in the Inbox. */
@Composable
fun CronJobsScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val agents by store.agents.collectAsState()
    val jobs = remember { mutableStateMapOf<String, List<CronJob>>() }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    val unreachable = stringResource(R.string.bridges_unreachable)
    val notSaved = stringResource(R.string.mute_failed)
    LaunchedEffect(agents) {
        var failures = 0
        for (agent in agents) {
            val bridge = store.bridge(agent) ?: continue
            try { jobs[agent.id] = bridge.cronJobs(agent.bridgeName) } catch (e: Throwable) { failures++ }
        }
        error = if (failures > 0) unreachable else null
        loading = false
    }
    SettingsPage(stringResource(R.string.scheduled_tasks), onBack) {
        error?.let { item { SectionNote(it, Bip.danger) } }
        if (loading) item { Loading() }
        for (agent in agents) {
            val list = jobs[agent.id].orEmpty()
            if (list.isEmpty()) continue
            item {
                Row(Modifier.padding(top = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Mascot(agent.look, Modifier.size(24.dp), animated = false)
                    Text(agent.name.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted))
                }
            }
            item {
                CardColumn {
                    for (job in list) ToggleRow(job.name ?: job.job, job.notify, { on ->
                        jobs[agent.id] = list.map { if (it.job == job.job) it.copy(notify = on) else it }
                        scope.launch {
                            runCatching { store.bridge(agent)!!.setCronNotify(job.agent, job.job, on) }.onFailure {
                                jobs[agent.id] = jobs[agent.id].orEmpty().map { if (it.job == job.job) it.copy(notify = !on) else it }
                                error = notSaved
                            }
                        }
                    }, subtitle = job.lastSeen?.let { stringResource(R.string.last_message, relative(it).replaceFirstChar { c -> c.lowercase() }) },
                        tint = agent.look.deep)
                }
            }
        }
        if (!loading && jobs.values.all { it.isEmpty() }) item { SectionNote(stringResource(R.string.no_scheduled_tasks), Bip.ink2) }
        item { SectionNote(stringResource(R.string.cron_help)) }
    }
}

// -- browsers ----------------------------------------------------------------------------------------------------

/** « Browsers »: the web console. A browser shows a code, approved here; the ones logged in, logged out from here. */
@Composable
fun ConsolesScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var code by remember { mutableStateOf("") }
    val computer = stringResource(R.string.computer)
    var name by remember { mutableStateOf(computer) }
    val devices = remember { mutableStateListOf<Pair<Bridge, List<ConsoleDevice>>>() }
    var loading by remember { mutableStateOf(true) }
    var approving by remember { mutableStateOf(false) }
    var notice by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
    val noConsole = stringResource(R.string.no_console)
    val approved = stringResource(R.string.console_approved)
    val noCode = stringResource(R.string.console_no_code)
    val bridges = remember { store.bridges().map { it.first } }
    val addresses = remember { store.agents.value.mapNotNull { it.bridgeUrl?.trimEnd('/')?.plus("/console/") }.distinct() }
    suspend fun load() {
        loading = true
        val found = bridges.mapNotNull { b -> runCatching { b.consoleDevices() }.getOrNull()?.let { b to it.sortedByDescending { d -> d.seen ?: 0 } } }
        devices.clear(); devices.addAll(found)
        if (found.isEmpty() && bridges.isNotEmpty()) notice = noConsole to true
        loading = false
    }
    LaunchedEffect(Unit) { load() }
    SettingsPage(stringResource(R.string.browsers), onBack) {
        item { SectionTitle(stringResource(R.string.approve_browser)) }
        item {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                androidx.compose.material3.TextField(code, { code = it.filter(Char::isDigit).take(6) }, Modifier.fillMaxWidth(),
                    placeholder = { Text(stringResource(R.string.six_digit_code), style = Bip.text(18, color = Bip.muted)) },
                    textStyle = Bip.text(24, FontWeight.ExtraBold).copy(fontFamily = FontFamily.Monospace), singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), shape = RoundedCornerShape(16.dp),
                    colors = androidx.compose.material3.TextFieldDefaults.colors(focusedContainerColor = Bip.card, unfocusedContainerColor = Bip.card,
                        focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent))
                Field(name, { name = it }, stringResource(R.string.computer_name))
                Box(Modifier.fillMaxWidth().background(if (code.length == 6 && !approving) Bip.ink else Bip.line, RoundedCornerShape(26.dp)).tap {
                    if (code.length != 6 || approving) return@tap
                    approving = true
                    scope.launch {
                        var last: Throwable? = null
                        var done = false
                        for (b in bridges) {
                            try { if (b.approveConsole(code, name.ifBlank { computer })) { done = true; break } } catch (e: Throwable) { last = e }
                        }
                        notice = if (done) approved to false else (last?.let { describe(context, it) } ?: noCode) to true
                        if (done) { code = ""; load() }
                        approving = false
                    }
                }.padding(14.dp), contentAlignment = Alignment.Center) {
                    if (approving) CircularProgressIndicator(Modifier.size(20.dp), color = Bip.card, strokeWidth = 2.dp)
                    else Text(stringResource(R.string.approve_this_browser), style = Bip.text(16, FontWeight.ExtraBold, Bip.card))
                }
            }
        }
        item { SectionNote(stringResource(R.string.console_help)) }
        for (a in addresses) item { Text(a, style = Bip.text(13, FontWeight.Bold, Bip.ink2).copy(fontFamily = FontFamily.Monospace)) }
        notice?.let { (text, bad) -> item { SectionNote(text, if (bad) Bip.danger else Bip.ink2) } }
        if (loading && devices.isEmpty()) item { Loading() }
        for ((bridge, list) in devices) {
            item { SectionTitle(if (devices.size > 1) android.net.Uri.parse(bridge.api.base).host ?: "" else stringResource(R.string.connected_browsers)) }
            item {
                CardColumn {
                    if (list.isEmpty()) Text(stringResource(R.string.no_browser), style = Bip.text(15, color = Bip.muted), modifier = Modifier.padding(16.dp))
                    for (device in list) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(device.name, style = Bip.text(16, FontWeight.Bold))
                            device.seen?.let { Text(stringResource(R.string.used, relative(it).replaceFirstChar { c -> c.lowercase() }), style = Bip.text(13, color = Bip.muted)) }
                        }
                        Text(stringResource(R.string.log_out), style = Bip.text(15, FontWeight.ExtraBold, Bip.danger), modifier = Modifier.tap {
                            scope.launch { runCatching { bridge.revokeConsole(device.id) }.onFailure { notice = describe(context, it) to true }; load() }
                        }.padding(6.dp))
                    }
                }
            }
        }
    }
}

// -- the vault ---------------------------------------------------------------------------------------------------

private data class VaultInstall(val bridge: Bridge, val title: String, val vault: Vault) {
    fun name(agent: String) = vault.agents.firstOrNull { it.id == agent }?.name ?: agent
}

/** The agents' vault: logins they use without ever reading them. Opened with the phone's lock, locked again when
 *  the app leaves the screen. Write-only: a password is replaced, never shown. */
@Composable
fun VaultScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var unlocked by remember { mutableStateOf(false) }
    var installs by remember { mutableStateOf<List<VaultInstall>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf<Pair<VaultInstall, VaultItem?>?>(null) }
    var deleting by remember { mutableStateOf<Pair<VaultInstall, VaultItem>?>(null) }
    val title = stringResource(R.string.open_vault)
    val needsLock = stringResource(R.string.vault_needs_lock)
    val none = stringResource(R.string.vault_none)
    suspend fun load() {
        val found = store.bridges().mapNotNull { (b, _) ->
            runCatching { b.vault() }.getOrNull()?.let { VaultInstall(b, android.net.Uri.parse(b.api.base).host ?: b.api.base, it) }
        }
        installs = found
        error = if (found.isEmpty()) none else null
    }
    fun unlock() {
        val activity = generateSequence(context) { (it as? android.content.ContextWrapper)?.baseContext }
            .filterIsInstance<androidx.fragment.app.FragmentActivity>().firstOrNull() ?: return
        if (!DeviceLock.available(activity)) { error = needsLock; return }
        DeviceLock.ask(activity, title) { ok -> unlocked = ok; if (ok) scope.launch { load() } }
    }
    LaunchedEffect(Unit) { unlock() }
    // Locked again as soon as the app leaves the screen.
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_STOP) { unlocked = false; installs = null; editing = null }
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    SettingsPage(stringResource(R.string.vault), onBack, actions = {
        val target = installs?.firstOrNull { it.vault.protected }
        if (unlocked && target != null) RoundButton(Icons.Rounded.Add, stringResource(R.string.add_login), { editing = target to null }, size = 44.dp)
    }) {
        if (!unlocked) {
            item {
                Column(Modifier.fillMaxWidth().padding(vertical = 40.dp), horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(14.dp)) {
                    Icon(Icons.Rounded.Lock, null, Modifier.size(48.dp), tint = Bip.ink2)
                    Text(stringResource(R.string.vault_locked), style = Bip.text(18, FontWeight.ExtraBold))
                    Pill(stringResource(R.string.unlock), dark = true) { unlock() }
                    error?.let { Text(it, style = Bip.text(14, color = Bip.danger), textAlign = TextAlign.Center) }
                }
            }
            return@SettingsPage
        }
        error?.let { item { SectionNote(it, Bip.danger) } }
        if (installs == null) item { Loading() }
        for (install in installs ?: emptyList()) {
            if ((installs?.size ?: 0) > 1) item { SectionTitle(install.title) }
            if (install.vault.items.isNotEmpty() || install.vault.protected) item {
                CardColumn {
                    for (vaultItem in install.vault.items) {
                        var menu by remember { mutableStateOf(false) }
                        Box {
                            Row(Modifier.fillMaxWidth().tap { editing = install to vaultItem }.padding(horizontal = 16.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                Icon(Icons.Rounded.Key, null, tint = Bip.ink2)
                                Column(Modifier.weight(1f)) {
                                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                        Text(vaultItem.label, style = Bip.text(16, FontWeight.Bold))
                                        if (vaultItem.hasOtp) Text("2FA", style = Bip.text(11, FontWeight.Black),
                                            modifier = Modifier.background(Bip.line, RoundedCornerShape(50)).padding(horizontal = 5.dp, vertical = 1.dp))
                                    }
                                    Text("${android.net.Uri.parse(vaultItem.origin).host ?: vaultItem.origin} · ${vaultItem.identifier}",
                                        style = Bip.text(13, color = Bip.ink2), maxLines = 1)
                                    Text(if (vaultItem.agents.isEmpty()) stringResource(R.string.no_agent) else vaultItem.agents.joinToString(", ") { install.name(it) },
                                        style = Bip.text(13, color = Bip.muted))
                                }
                                Text("⋯", style = Bip.text(20, FontWeight.Black, Bip.muted), modifier = Modifier.tap { menu = true }.padding(6.dp))
                            }
                            DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                                DropdownMenuItem({ Text(stringResource(R.string.delete), style = Bip.text(16, FontWeight.Bold, Bip.danger)) },
                                    { menu = false; deleting = install to vaultItem })
                            }
                        }
                    }
                    if (install.vault.items.isEmpty() && install.vault.protected)
                        Text(stringResource(R.string.vault_empty), style = Bip.text(14, color = Bip.ink2), modifier = Modifier.padding(16.dp))
                }
            }
            if (!install.vault.protected) item { SectionNote(stringResource(R.string.vault_unprotected), Bip.danger) }
        }
        item { SectionNote(stringResource(R.string.vault_help)) }
    }
    deleting?.let { (install, vaultItem) ->
        AlertDialog({ deleting = null }, title = { Text(stringResource(R.string.delete_login, vaultItem.label)) },
            text = { Text(stringResource(R.string.delete_login_body)) },
            confirmButton = { TextButton({
                deleting = null
                scope.launch { runCatching { install.bridge.deleteVaultItem(vaultItem.id) }.onFailure { error = describe(context, it) }; load() }
            }) { Text(stringResource(R.string.delete), color = Bip.danger) } },
            dismissButton = { TextButton({ deleting = null }) { Text(stringResource(R.string.cancel)) } })
    }
    editing?.let { (install, vaultItem) ->
        VaultEditor(store, install, vaultItem, onDismiss = { saved -> editing = null; if (saved) scope.launch { load() } })
    }
}

/** A login: name, site, user, password (write-only), 2FA key, and the agents that may use it. */
@Composable
private fun VaultEditor(store: AgentStore, start: VaultInstall, item: VaultItem?, onDismiss: (Boolean) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var install by remember { mutableStateOf(start) }
    var label by remember { mutableStateOf(item?.label ?: "") }
    var site by remember { mutableStateOf(item?.origin ?: "") }
    var identifier by remember { mutableStateOf(item?.identifier ?: "") }
    var password by remember { mutableStateOf("") }
    var otp by remember { mutableStateOf("") }
    var removesOtp by remember { mutableStateOf(false) }
    val agents = remember { mutableStateListOf<String>().apply { addAll(item?.agents ?: emptyList()) } }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var moving by remember { mutableStateOf<String?>(null) }
    var movingNow by remember { mutableStateOf<String?>(null) }
    val isNew = item == null
    val ready = label.isNotBlank() && site.isNotBlank() && identifier.isNotBlank() && (!isNew || password.isNotEmpty()) && !saving
    Dialog({ onDismiss(false) }, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(Bip.ground).statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.cancel), style = Bip.text(16, FontWeight.Bold, Bip.ink2), modifier = Modifier.tap { onDismiss(false) }.padding(8.dp))
                Text(if (isNew) stringResource(R.string.new_login) else label, style = Bip.text(17, FontWeight.ExtraBold), modifier = Modifier.weight(1f),
                    textAlign = TextAlign.Center, maxLines = 1)
                Text(stringResource(R.string.save), style = Bip.text(16, FontWeight.ExtraBold, if (ready) Bip.accent else Bip.muted), modifier = Modifier.tap {
                    if (!ready) return@tap
                    saving = true
                    scope.launch {
                        try {
                            install.bridge.saveVaultItem(item?.id, label.trim(), site.trim(), identifier.trim(), password.ifEmpty { null },
                                if (removesOtp) "" else otp.trim().ifEmpty { null }, agents.sorted())
                            onDismiss(true)
                        } catch (refused: CannotHold) {
                            error = refused.reasons.entries.joinToString("\n") { (agent, why) -> holdReason(context, why, install.name(agent)) }
                            saving = false
                        } catch (e: Throwable) {
                            if (e is kotlinx.coroutines.CancellationException) throw e
                            error = describe(context, e); saving = false
                        }
                    }
                }.padding(8.dp))
            }
            Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                error?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
                Field(label, { label = it }, stringResource(R.string.login_name_hint))
                Field(site, { site = it }, stringResource(R.string.login_site_hint))
                Field(identifier, { identifier = it }, stringResource(R.string.login_identifier_hint))
                Field(password, { password = it }, stringResource(if (isNew) R.string.password else R.string.new_password_hint), secret = true)
                SectionNote(stringResource(R.string.login_site_help))
                if (item?.hasOtp == true && !removesOtp) Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.otp_saved), style = Bip.text(15, FontWeight.Bold), modifier = Modifier.weight(1f))
                    Text(stringResource(R.string.remove_otp), style = Bip.text(15, FontWeight.ExtraBold, Bip.danger), modifier = Modifier.tap { removesOtp = true }.padding(6.dp))
                } else Field(otp, { otp = it }, stringResource(R.string.otp_hint))
                SectionNote(stringResource(R.string.otp_help))
                SectionTitle(stringResource(R.string.agents_who_can_use))
                CardColumn {
                    for (agent in install.vault.agents) Column(Modifier.padding(bottom = 4.dp)) {
                        ToggleRow(agent.name, agent.id in agents, { on -> if (on) agents.add(agent.id) else agents.remove(agent.id) },
                            subtitle = when {
                                movingNow == agent.id -> stringResource(R.string.moving_to_own_space)
                                agent.why != null && agent.id !in agents -> holdReason(context, agent.why, agent.name)
                                else -> null
                            }, enabled = agent.canHold || agent.id in agents)
                        if (agent.why == "needs_own_space" && movingNow != agent.id) Text(stringResource(R.string.give_own_space),
                            style = Bip.text(14, FontWeight.ExtraBold, Bip.accent), modifier = Modifier.tap { moving = agent.id }.padding(horizontal = 16.dp, vertical = 4.dp))
                    }
                }
                SectionNote(stringResource(R.string.agents_who_can_use_help))
            }
        }
    }
    moving?.let { id ->
        val agentName = install.name(id)
        AlertDialog({ moving = null }, title = { Text(stringResource(R.string.give_own_space_title, agentName)) },
            text = { Text(stringResource(R.string.own_space_explanation, agentName)) },
            confirmButton = { TextButton({
                moving = null; movingNow = id
                scope.launch {
                    try {
                        install.bridge.moveToOwnSpace(id)?.let { url -> store.agents.value.firstOrNull { it.bridgeName == id }?.let { a -> store.relocate(a, url) } }
                        install.bridge.vault()?.let { install = install.copy(vault = it) }
                        agents.add(id); error = null
                    } catch (e: Throwable) { error = describe(context, e) }
                    movingNow = null
                }
            }) { Text(stringResource(R.string.give_own_space_confirm)) } },
            dismissButton = { TextButton({ moving = null }) { Text(stringResource(R.string.cancel)) } })
    }
}

// -- the voice model ---------------------------------------------------------------------------------------------

/** What the person's own voice server speaks for (the Live, spoken replies, reading), per install. */
@Composable
fun VoiceModelScreen(store: AgentStore, onBack: () -> Unit) {
    val scope = rememberCoroutineScope()
    val servers = remember { mutableStateListOf<Pair<Bridge, io.github.bipagents.net.VoiceServer>>() }
    var loading by remember { mutableStateOf(true) }
    var error by remember { mutableStateOf<String?>(null) }
    val notSaved = stringResource(R.string.voice_uses_not_saved)
    LaunchedEffect(Unit) {
        servers.addAll(store.bridges().mapNotNull { (b, _) -> runCatching { b.voiceServer() }.getOrNull()?.let { b to it } })
        loading = false
    }
    SettingsPage(stringResource(R.string.your_voice_model), onBack) {
        if (loading) item { Loading() }
        servers.forEachIndexed { index, (bridge, server) ->
            if (servers.size > 1) item { SectionTitle(android.net.Uri.parse(bridge.api.base).host ?: "") }
            item {
                CardColumn {
                    for ((use, label) in listOf("live" to R.string.use_live, "replies" to R.string.use_replies, "reading" to R.string.use_reading)) {
                        ToggleRow(stringResource(label), server.takes(use), { on ->
                            servers[index] = bridge to server.copy(uses = server.uses + (use to on))
                            scope.launch {
                                runCatching { bridge.setVoiceUses(mapOf(use to on)) }.onFailure {
                                    servers[index] = bridge to server; error = notSaved
                                }
                            }
                        })
                    }
                }
            }
        }
        error?.let { item { SectionNote(it, Bip.danger) } }
        item { SectionNote(stringResource(R.string.voice_model_help)) }
    }
}

// -- diagnostics -------------------------------------------------------------------------------------------------

/** Reachability and latency of every agent and bridge, and what each Hermes server advertises; a report to share. */
@Composable
fun DiagnosticsScreen(store: AgentStore, onBack: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val agents by store.agents.collectAsState()
    data class Result(val hermes: String?, val hermesOk: Boolean, val bridge: String?, val bridgeOk: Boolean, val features: List<String>)
    val results = remember { mutableStateMapOf<String, Result>() }
    var running by remember { mutableStateOf(false) }
    suspend fun run() {
        running = true
        results.clear()
        for (agent in agents) {
            var hermes: String? = null; var hermesOk = false; var features = emptyList<String>()
            store.hermes(agent)?.let { h ->
                val start = System.nanoTime()
                try {
                    val caps = h.api.call("v1/capabilities") as? kotlinx.serialization.json.JsonObject
                    hermes = "${(System.nanoTime() - start) / 1_000_000} ms"; hermesOk = true
                    features = (caps?.get("features") as? kotlinx.serialization.json.JsonObject)?.filter { (it.value as? kotlinx.serialization.json.JsonPrimitive)?.content == "true" }
                        ?.keys?.sorted() ?: emptyList()
                } catch (e: Throwable) { hermes = describe(context, e) }
            }
            var bridge: String? = null; var bridgeOk = false
            store.bridge(agent)?.let { b ->
                val start = System.nanoTime()
                try { b.api.call("health"); bridge = "${(System.nanoTime() - start) / 1_000_000} ms"; bridgeOk = true } catch (e: Throwable) { bridge = describe(context, e) }
            }
            results[agent.id] = Result(hermes, hermesOk, bridge, bridgeOk, features)
        }
        running = false
    }
    LaunchedEffect(Unit) { run() }
    fun report(): String = buildString {
        append("BipAgents — diagnostics ${java.text.DateFormat.getDateTimeInstance().format(java.util.Date())}\n")
        for (agent in agents) {
            val r = results[agent.id]
            append("\n${agent.name} — ${agent.baseUrl}\n  Hermes: ${r?.hermes ?: "…"}\n")
            if (agent.bridgeUrl != null) append("  Bridge: ${r?.bridge ?: "…"}\n")
            append("  Features: ${r?.features?.joinToString(", ")?.ifEmpty { "-" } ?: "-"}\n")
        }
    }
    SettingsPage(stringResource(R.string.diagnostics), onBack, actions = {
        RoundButton(Icons.Rounded.Refresh, stringResource(R.string.retry), { if (!running) scope.launch { run() } }, size = 44.dp)
    }) {
        for (agent in agents) {
            val r = results[agent.id]
            item {
                Row(Modifier.padding(top = 14.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Mascot(agent.look, Modifier.size(26.dp), animated = false)
                    Text(agent.name.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted))
                }
            }
            item {
                CardColumn {
                    @Composable fun Line(title: String, value: String?, ok: Boolean) = Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp),
                        verticalAlignment = Alignment.CenterVertically) {
                        Text(title, style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
                        if (value == null) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                        else Text((if (ok) "✓ " else "") + value, style = Bip.text(14, FontWeight.Bold, if (ok) Bip.online else Bip.danger), textAlign = TextAlign.End)
                    }
                    Line("Hermes", r?.hermes, r?.hermesOk == true)
                    if (agent.bridgeUrl != null) Line("Bridge", r?.bridge, r?.bridgeOk == true)
                    r?.features?.takeIf { it.isNotEmpty() }?.let {
                        Text(it.joinToString(" · "), style = Bip.text(12, color = Bip.ink2).copy(fontFamily = FontFamily.Monospace),
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
                    }
                }
            }
        }
        item {
            CardColumn {
                NavRow(Icons.Rounded.Share, stringResource(R.string.export_report)) {
                    context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, report()), null))
                }
            }
        }
    }
}
