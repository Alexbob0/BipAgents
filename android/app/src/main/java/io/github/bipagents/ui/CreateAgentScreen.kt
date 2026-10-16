package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Category
import io.github.bipagents.model.Look
import io.github.bipagents.net.AgentKind
import kotlinx.coroutines.launch

/** « Create an agent » (CreateAgentFlow): on an install that manages its agents, a kind (or « Other », described by
 *  the person), a name, and the bridge creates it on the server. No terminal, no keys to copy. */
@Composable
fun CreateAgentScreen(store: AgentStore, onBack: () -> Unit, onCreated: () -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val scope = rememberCoroutineScope()
    val device = java.util.Locale.getDefault().language.takeIf { l -> LANGUAGES.any { it.first == l } } ?: "en"
    var install by remember { mutableStateOf<AgentStore.Creatable?>(null) }
    var kind by remember { mutableStateOf<AgentKind?>(null) }
    var name by remember { mutableStateOf("") }
    var role by remember { mutableStateOf("") }
    var tone by remember { mutableStateOf("") }
    var rules by remember { mutableStateOf("") }
    var language by remember { mutableStateOf(device) }
    var category by remember { mutableStateOf(Category.TECH) }
    var creating by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val failed = stringResource(R.string.create_failed)
    LaunchedEffect(Unit) { install = store.creatable(device) }
    val k = kind
    Column(Modifier.fillMaxSize().imePadding()) {
        TopBar(stringResource(if (k == null) R.string.which_kind else R.string.your_agent), { if (k != null && !creating) kind = null else if (!creating) onBack() })
        if (install == null) { Text(stringResource(R.string.loading), style = Bip.text(15, color = Bip.muted), modifier = Modifier.padding(16.dp)); return@Column }
        if (k == null) {
            LazyVerticalGrid(GridCells.Fixed(3), contentPadding = PaddingValues(16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp),
                verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(install!!.kinds, key = { it.id }) { option ->
                    val c = Category.entries.firstOrNull { it.id == option.id }
                    Column(Modifier.heightIn(min = 150.dp).background(c?.let { Look.of(it).tint } ?: Bip.card, RoundedCornerShape(22.dp))
                        .tap { c?.let { category = it }; kind = option }.padding(horizontal = 8.dp, vertical = 14.dp),
                        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Mascot(Look.of(c ?: Category.TECH), Modifier.size(58.dp), animated = false)
                        Text(if (option.custom) stringResource(R.string.kind_other) else option.label, style = Bip.text(14, FontWeight.ExtraBold), textAlign = TextAlign.Center)
                        Text(if (option.custom) stringResource(R.string.kind_custom) else option.description, style = Bip.text(12, color = Bip.muted),
                            textAlign = TextAlign.Center, maxLines = 2)
                    }
                }
            }
            return@Column
        }
        Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Column(Modifier.fillMaxWidth().background(Look.of(category).tint, RoundedCornerShape(22.dp)).padding(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Mascot(Look.of(category), Modifier.size(110.dp), if (creating) Mood.THINKING else Mood.CONTENT, animated = creating)
                Box(Modifier.padding(horizontal = 30.dp)) { Field(name, { name = it }, stringResource(R.string.its_name)) }
            }
            if (!k.custom) Text(k.description, style = Bip.text(13, color = Bip.muted))
            if (k.custom) {
                SectionTitle(stringResource(R.string.what_it_does))
                Field(role, { role = it }, stringResource(R.string.what_it_does_hint), minLines = 3)
                SectionTitle(stringResource(R.string.its_character))
                Field(tone, { tone = it }, stringResource(R.string.its_tone_hint))
                Field(rules, { rules = it }, stringResource(R.string.its_rules_hint), minLines = 2)
                var languages by remember { mutableStateOf(false) }
                Box {
                    CardColumn { SettingRow(stringResource(R.string.language), LANGUAGES.first { it.first == language }.second) { languages = true } }
                    DropdownMenu(languages, { languages = false }, containerColor = Bip.card) {
                        for ((code, label) in LANGUAGES) DropdownMenuItem({ Text(label, style = Bip.text(16, FontWeight.Bold)) }, { languages = false; language = code })
                    }
                }
                SectionTitle(stringResource(R.string.its_colour))
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    for (c in Category.entries) Box(Modifier.size(40.dp).border(if (category == c) 2.dp else 0.dp, Bip.ink, CircleShape).padding(3.dp)
                        .background(c.main, CircleShape).tap { category = c })
                }
            }
            error?.let { Text(it, style = Bip.text(14, FontWeight.Bold, Bip.danger)) }
            val ready = name.isNotBlank() && (!k.custom || role.trim().length >= 3) && !creating
            Box(Modifier.padding(top = 8.dp).fillMaxWidth().background(if (ready) Bip.ink else Bip.line, RoundedCornerShape(26.dp)).tap {
                if (!ready) return@tap
                creating = true; error = null
                scope.launch {
                    try {
                        val i = install!!
                        // An « Other » agent speaks the language picked for it; a ready-made one, the phone's.
                        val spoken = if (k.custom) language else device
                        val custom = if (k.custom) mapOf("role" to role, "tone" to tone, "rules" to rules,
                            "language" to LANGUAGES.first { it.first == language }.second, "category" to category.id) else null
                        val created = io.github.bipagents.net.Bridge(io.github.bipagents.net.Api(i.bridgeUrl, i.bridgeKey))
                            .createAgent(name.trim(), if (k.custom) null else k.id, custom, spoken)
                        val agent = store.addListed(created, i.bridgeUrl, i.bridgeKey, category.id, spoken)
                        store.agent(agent.id)?.let { a -> runCatching { store.setLook(a, category, null) } }
                        onCreated()
                    } catch (e: Throwable) {
                        if (e is kotlinx.coroutines.CancellationException) throw e
                        error = failed; creating = false
                    }
                }
            }.padding(14.dp), contentAlignment = Alignment.Center) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (creating) CircularProgressIndicator(Modifier.size(18.dp), color = Bip.card, strokeWidth = 2.dp)
                    Text(if (creating) stringResource(R.string.preparing, name.trim()) else stringResource(R.string.create),
                        style = Bip.text(16, FontWeight.ExtraBold, Bip.card))
                }
            }
            if (creating) Text(stringResource(R.string.create_can_take), style = Bip.text(13, color = Bip.muted))
        }
    }
}
