package io.github.bipagents.ui

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Bolt
import androidx.compose.material.icons.rounded.DeleteOutline
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PauseCircle
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.Routine
import kotlinx.coroutines.launch
import java.util.Calendar

/** When a routine runs, in words (« Every Monday at 8:00 »), turned into what Hermes stores: a cron expression, an
 *  interval or a date. Anything else Hermes accepts stays as typed (Custom). */
sealed interface RoutineSchedule {
    data class Daily(val hour: Int, val minute: Int) : RoutineSchedule
    data class Weekdays(val hour: Int, val minute: Int) : RoutineSchedule
    /** 1 = Monday … 7 = Sunday. */
    data class Weekly(val weekday: Int, val hour: Int, val minute: Int) : RoutineSchedule
    data class Monthly(val day: Int, val hour: Int, val minute: Int) : RoutineSchedule
    data class EveryHours(val hours: Int) : RoutineSchedule
    data class Once(val at: Long) : RoutineSchedule
    data class Custom(val text: String) : RoutineSchedule

    /** What Hermes stores. */
    val hermes: String get() = when (this) {
        is Daily -> "$minute $hour * * *"
        is Weekdays -> "$minute $hour * * 1-5"
        is Weekly -> "$minute $hour * * ${weekday % 7}"
        is Monthly -> "$minute $hour $day * *"
        is EveryHours -> "every ${hours}h"
        // With the phone's offset: the same instant whatever Hermes' timezone.
        is Once -> java.time.OffsetDateTime.ofInstant(java.time.Instant.ofEpochMilli(at), java.time.ZoneId.systemDefault())
            .withNano(0).format(java.time.format.DateTimeFormatter.ISO_OFFSET_DATE_TIME)
        is Custom -> text
    }

    fun label(context: Context): String {
        fun time(h: Int, m: Int) = "%d:%02d".format(h, m)
        return when (this) {
            is Daily -> context.getString(R.string.schedule_daily_at, time(hour, minute))
            is Weekdays -> context.getString(R.string.schedule_weekdays_at, time(hour, minute))
            is Weekly -> context.getString(R.string.schedule_weekly_at, weekdayName(weekday), time(hour, minute))
            is Monthly -> context.getString(R.string.schedule_monthly_at, day, time(hour, minute))
            is EveryHours -> context.getString(R.string.schedule_every_hours, hours)
            is Once -> java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.MEDIUM, java.text.DateFormat.SHORT).format(java.util.Date(at))
            is Custom -> text
        }
    }

    companion object {
        /** Monday … Sunday in the phone's language. */
        fun weekdayName(day: Int): String = java.time.DayOfWeek.of(day).getDisplayName(java.time.format.TextStyle.FULL, java.util.Locale.getDefault())

        /** Reads what Hermes stored back into words (cron, `every Nm/h`, a date); anything else is Custom. */
        fun of(text: String): RoutineSchedule {
            val parts = text.trim().split(Regex("\\s+"))
            val m = parts.getOrNull(0)?.toIntOrNull()
            val h = parts.getOrNull(1)?.toIntOrNull()
            if (parts.size == 5 && m != null && h != null && parts[3] == "*") {
                val (dom, dow) = parts[2] to parts[4]
                when {
                    dom == "*" && dow == "*" -> return Daily(h, m)
                    dom == "*" && (dow == "1-5" || dow.equals("MON-FRI", true)) -> return Weekdays(h, m)
                    dom == "*" && dow.toIntOrNull() in 0..7 -> return Weekly(dow.toInt().let { if (it == 0) 7 else it }, h, m)
                    dow == "*" && dom.toIntOrNull() in 1..31 -> return Monthly(dom.toInt(), h, m)
                }
            }
            Regex("^every (\\d+)([mh])$").find(text.trim())?.let { r ->
                val n = r.groupValues[1].toInt()
                if (r.groupValues[2] == "h" && n >= 1) return EveryHours(n)
                if (n >= 60 && n % 60 == 0) return EveryHours(n / 60)
            }
            runCatching { return Once(java.time.OffsetDateTime.parse(text.trim()).toInstant().toEpochMilli()) }
            runCatching { return Once(java.time.LocalDateTime.parse(text.trim()).atZone(java.time.ZoneOffset.UTC).toInstant().toEpochMilli()) }
            return Custom(text)
        }
    }
}

/** An agent's routines: what it does on its own, and when. Its reports land in its Bot Chat and the Inbox. */
@Composable
fun RoutinesScreen(store: AgentStore, agentId: String, onBack: () -> Unit) {
    val agents by store.agents.collectAsState()
    val agent = agents.firstOrNull { it.id == agentId } ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var routines by remember { mutableStateOf<List<Routine>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var editing by remember { mutableStateOf<Routine?>(null) }
    var adding by remember { mutableStateOf(false) }
    suspend fun load() {
        try {
            routines = store.hermes(agent)?.routines()?.sortedWith(compareBy({ it.paused }, { it.name.lowercase() }))
            error = null
        } catch (e: Throwable) {
            error = describe(context, e)
            if (routines == null) routines = emptyList()
        }
    }
    fun act(action: suspend (io.github.bipagents.net.Hermes) -> Unit) = scope.launch {
        val hermes = store.hermes(agent) ?: return@launch
        try { action(hermes) } catch (e: Throwable) { error = describe(context, e) }
        load()
    }
    LaunchedEffect(agentId) { load() }
    Column(Modifier.fillMaxSize()) {
        TopBar(stringResource(R.string.routines), onBack) {
            RoundButton(Icons.Rounded.Add, stringResource(R.string.new_routine), { adding = true }, size = 44.dp)
        }
        LazyColumn(contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            error?.let { item { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) } }
            if (routines == null) item { Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted)) }
            items(routines ?: emptyList(), key = { it.id }) { routine ->
                RoutineRow(routine, agent.look, onOpen = { editing = routine },
                    onToggle = { act { it.pauseRoutine(routine.id, !routine.paused) } },
                    onRun = { act { it.runRoutine(routine.id) } },
                    onDelete = { routines = routines?.filterNot { it.id == routine.id }; act { it.deleteRoutine(routine.id) } })
            }
            if (routines?.isEmpty() == true) item {
                Text(stringResource(R.string.routines_empty, agent.name), style = Bip.text(14, color = Bip.ink2))
            }
        }
    }
    if (adding || editing != null) RoutineEditor(agent.name, editing, onDismiss = { saved ->
        adding = false; editing = null
        if (saved) scope.launch { load() }
    }) { name, schedule, prompt ->
        val hermes = store.hermes(agent) ?: return@RoutineEditor
        val id = editing?.id
        if (id != null) hermes.updateRoutine(id, name, schedule, prompt) else hermes.createRoutine(name, schedule, prompt)
    }
}

@Composable
private fun RoutineRow(routine: Routine, look: io.github.bipagents.model.Look, onOpen: () -> Unit, onToggle: () -> Unit, onRun: () -> Unit,
                       onDelete: () -> Unit) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    Box {
        Row(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp))
            .pointerInput(routine.id) { detectTapGestures(onTap = { onOpen() }, onLongPress = { menu = true }) }
            .padding(horizontal = 16.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Icon(if (routine.paused) Icons.Rounded.PauseCircle else Icons.Rounded.Schedule, null, Modifier.size(26.dp),
                tint = if (routine.paused) Bip.muted else look.deep)
            Column(Modifier.weight(1f)) {
                Text(routine.name, style = Bip.text(16, FontWeight.ExtraBold))
                Text(RoutineSchedule.of(routine.schedule).label(context), style = Bip.text(14, color = Bip.ink2))
                if (routine.paused) Text(stringResource(R.string.paused), style = Bip.text(13, FontWeight.ExtraBold, Bip.muted))
                else routine.nextRun?.let { Text(stringResource(R.string.next_time, relative(it).replaceFirstChar { c -> c.lowercase() }), style = Bip.text(13, color = Bip.muted)) }
            }
            if (routine.lastStatus == "error") Icon(Icons.Rounded.Warning, stringResource(R.string.last_run_failed), tint = Bip.danger)
            Box(Modifier.size(38.dp).background(look.tint, CircleShape).tap(onToggle), contentAlignment = Alignment.Center) {
                Icon(if (routine.paused) Icons.Rounded.PlayArrow else Icons.Rounded.Pause,
                    stringResource(if (routine.paused) R.string.resume else R.string.pause), Modifier.size(20.dp), tint = look.deep)
            }
        }
        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
            DropdownMenuItem({ Text(stringResource(R.string.run_now), style = Bip.text(16, FontWeight.Bold)) }, { menu = false; onRun() },
                leadingIcon = { Icon(Icons.Rounded.Bolt, null) })
            DropdownMenuItem({ Text(stringResource(if (routine.paused) R.string.resume else R.string.pause), style = Bip.text(16, FontWeight.Bold)) },
                { menu = false; onToggle() }, leadingIcon = { Icon(if (routine.paused) Icons.Rounded.PlayArrow else Icons.Rounded.Pause, null) })
            DropdownMenuItem({ Text(stringResource(R.string.delete), style = Bip.text(16, FontWeight.Bold, Bip.danger)) }, { menu = false; onDelete() },
                leadingIcon = { Icon(Icons.Rounded.DeleteOutline, null, tint = Bip.danger) })
        }
    }
}

/** A new routine or changes to one: a name, what to do, and when (in words). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RoutineEditor(agentName: String, routine: Routine?, onDismiss: (saved: Boolean) -> Unit, onSave: suspend (String, String, String) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val parsed = remember(routine) { routine?.let { RoutineSchedule.of(it.schedule) } }
    var name by remember { mutableStateOf(routine?.name ?: "") }
    var prompt by remember { mutableStateOf(routine?.prompt ?: "") }
    var kind by remember { mutableIntStateOf(when (parsed) {
        is RoutineSchedule.Weekdays -> 1; is RoutineSchedule.Weekly -> 2; is RoutineSchedule.Monthly -> 3
        is RoutineSchedule.EveryHours -> 4; is RoutineSchedule.Once -> 5; is RoutineSchedule.Custom -> 6; else -> 0 }) }
    var hour by remember { mutableIntStateOf(when (parsed) { is RoutineSchedule.Daily -> parsed.hour; is RoutineSchedule.Weekdays -> parsed.hour
        is RoutineSchedule.Weekly -> parsed.hour; is RoutineSchedule.Monthly -> parsed.hour; else -> 8 }) }
    var minute by remember { mutableIntStateOf(when (parsed) { is RoutineSchedule.Daily -> parsed.minute; is RoutineSchedule.Weekdays -> parsed.minute
        is RoutineSchedule.Weekly -> parsed.minute; is RoutineSchedule.Monthly -> parsed.minute; else -> 0 }) }
    var weekday by remember { mutableIntStateOf((parsed as? RoutineSchedule.Weekly)?.weekday ?: 1) }
    var monthDay by remember { mutableIntStateOf(((parsed as? RoutineSchedule.Monthly)?.day ?: 1).coerceAtMost(28)) }
    var hours by remember { mutableIntStateOf((parsed as? RoutineSchedule.EveryHours)?.hours ?: 2) }
    var once by remember { mutableStateOf((parsed as? RoutineSchedule.Once)?.at ?: (System.currentTimeMillis() + 3_600_000)) }
    var custom by remember { mutableStateOf((parsed as? RoutineSchedule.Custom)?.text ?: "") }
    var saving by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var pickingTime by remember { mutableStateOf(false) }
    var pickingDate by remember { mutableStateOf(false) }
    val schedule = when (kind) {
        0 -> RoutineSchedule.Daily(hour, minute); 1 -> RoutineSchedule.Weekdays(hour, minute)
        2 -> RoutineSchedule.Weekly(weekday, hour, minute); 3 -> RoutineSchedule.Monthly(monthDay, hour, minute)
        4 -> RoutineSchedule.EveryHours(hours); 5 -> RoutineSchedule.Once(once); else -> RoutineSchedule.Custom(custom.trim())
    }
    val kinds = listOf(R.string.freq_daily, R.string.freq_weekdays, R.string.freq_weekly, R.string.freq_monthly, R.string.freq_every_hours,
        R.string.freq_once, R.string.freq_custom)
    Dialog({ onDismiss(false) }, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(Bip.ground).statusBarsPadding().navigationBarsPadding().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.cancel), style = Bip.text(16, FontWeight.Bold, Bip.ink2), modifier = Modifier.tap { onDismiss(false) }.padding(8.dp))
                Text(stringResource(if (routine == null) R.string.new_routine else R.string.routine), style = Bip.text(17, FontWeight.ExtraBold),
                    modifier = Modifier.weight(1f), textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                val ready = !saving && name.isNotBlank() && prompt.isNotBlank() && schedule.hermes.isNotEmpty()
                Text(stringResource(R.string.save), style = Bip.text(16, FontWeight.ExtraBold, if (ready) Bip.accent else Bip.muted),
                    modifier = Modifier.tap {
                        if (!ready) return@tap
                        saving = true; error = null
                        scope.launch {
                            try {
                                onSave(name.trim(), schedule.hermes, prompt.trim())
                                onDismiss(true)
                            } catch (e: Throwable) {
                                if (e is kotlinx.coroutines.CancellationException) throw e
                                error = describe(context, e)
                                saving = false
                            }
                        }
                    }.padding(8.dp))
            }
            Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                error?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
                Label(stringResource(R.string.name))
                Field(name, { name = it }, stringResource(R.string.routine_name_hint))
                Label(stringResource(R.string.what_to_do, agentName))
                Field(prompt, { prompt = it }, stringResource(R.string.routine_prompt_hint), minLines = 3)
                Text(stringResource(R.string.routine_report_help), style = Bip.text(13, color = Bip.muted))
                Label(stringResource(R.string.schedule_when))
                Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(vertical = 4.dp)) {
                    var menu by remember { mutableStateOf(false) }
                    Box {
                        SettingRow(stringResource(R.string.frequency), stringResource(kinds[kind])) { menu = true }
                        DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                            kinds.forEachIndexed { i, label -> DropdownMenuItem({ Text(stringResource(label), style = Bip.text(16, FontWeight.Bold)) },
                                { menu = false; kind = i }) }
                        }
                    }
                    if (kind == 2) {
                        var days by remember { mutableStateOf(false) }
                        Box {
                            SettingRow(stringResource(R.string.day), RoutineSchedule.weekdayName(weekday).replaceFirstChar { it.uppercase() }) { days = true }
                            DropdownMenu(days, { days = false }, containerColor = Bip.card) {
                                for (d in 1..7) DropdownMenuItem({ Text(RoutineSchedule.weekdayName(d).replaceFirstChar { it.uppercase() },
                                    style = Bip.text(16, FontWeight.Bold)) }, { days = false; weekday = d })
                            }
                        }
                    }
                    if (kind == 3) Stepper(stringResource(R.string.day_of_month, monthDay), { monthDay = (monthDay - 1).coerceAtLeast(1) },
                        { monthDay = (monthDay + 1).coerceAtMost(28) })
                    if (kind in 0..3) SettingRow(stringResource(R.string.time), "%d:%02d".format(hour, minute)) { pickingTime = true }
                    if (kind == 4) Stepper(stringResource(R.string.every_n_hours, hours), { hours = (hours - 1).coerceAtLeast(1) },
                        { hours = (hours + 1).coerceAtMost(24) })
                    if (kind == 5) SettingRow(stringResource(R.string.on_date), RoutineSchedule.Once(once).label(context)) { pickingDate = true }
                    if (kind == 6) Box(Modifier.padding(12.dp)) { Field(custom, { custom = it }, "0 8 * * 1-5 · every 30m") }
                }
            }
        }
    }
    if (pickingTime) {
        val state = androidx.compose.material3.rememberTimePickerState(hour, minute, is24Hour = android.text.format.DateFormat.is24HourFormat(context))
        androidx.compose.material3.AlertDialog({ pickingTime = false }, confirmButton = {
            androidx.compose.material3.TextButton({ hour = state.hour; minute = state.minute; pickingTime = false }) { Text("OK") }
        }, text = { androidx.compose.material3.TimePicker(state) })
    }
    if (pickingDate) {
        val state = androidx.compose.material3.rememberDatePickerState(once)
        androidx.compose.material3.DatePickerDialog({ pickingDate = false }, confirmButton = {
            androidx.compose.material3.TextButton({
                state.selectedDateMillis?.let { day ->
                    // The day picked (UTC midnight) at the hour already chosen, in the phone's timezone.
                    val picked = Calendar.getInstance(java.util.TimeZone.getTimeZone("UTC")).apply { timeInMillis = day }
                    val keep = Calendar.getInstance().apply { timeInMillis = once }
                    once = Calendar.getInstance().apply {
                        set(picked.get(Calendar.YEAR), picked.get(Calendar.MONTH), picked.get(Calendar.DAY_OF_MONTH),
                            keep.get(Calendar.HOUR_OF_DAY), keep.get(Calendar.MINUTE), 0)
                    }.timeInMillis
                }
                pickingDate = false; pickingTime = false
                kind = 5
            }) { Text("OK") }
        }) { androidx.compose.material3.DatePicker(state) }
    }
}

@Composable
private fun Label(text: String) = Text(text.uppercase(), style = Bip.text(12, FontWeight.ExtraBold, Bip.muted), modifier = Modifier.padding(top = 10.dp))

@Composable
fun Field(value: String, onChange: (String) -> Unit, hint: String, minLines: Int = 1, secret: Boolean = false) {
    TextField(value, onChange, Modifier.fillMaxWidth().heightIn(min = 52.dp), placeholder = { Text(hint, style = Bip.text(15, color = Bip.muted)) },
        textStyle = Bip.text(16), minLines = minLines, shape = RoundedCornerShape(16.dp),
        visualTransformation = if (secret) androidx.compose.ui.text.input.PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
        colors = TextFieldDefaults.colors(focusedContainerColor = Bip.card, unfocusedContainerColor = Bip.card,
            focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent))
}

@Composable
fun SettingRow(title: String, value: String, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().tap(onClick).padding(horizontal = 16.dp, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
        Text(value, style = Bip.text(15, FontWeight.Bold, Bip.ink2))
    }
}

@Composable
private fun Stepper(label: String, minus: () -> Unit, plus: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(label, style = Bip.text(16, FontWeight.Bold), modifier = Modifier.weight(1f))
        Pill("−", dark = false, onClick = minus)
        Pill("+", dark = false, onClick = plus)
    }
}
