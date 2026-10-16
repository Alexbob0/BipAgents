package io.github.bipagents.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.Keyboard
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.QrCodeScanner
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.model.Category
import io.github.bipagents.net.Session
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** Home: one card per agent (App/Features/Agents/AgentsView.swift). */
@Composable
fun HomeScreen(store: AgentStore, onAdd: () -> Unit, onOpen: (String) -> Unit, onLive: (String) -> Unit) {
    val agents by store.agents.collectAsState()
    val latest by store.latest.collectAsState()
    val online by store.online.collectAsState()
    val scope = rememberCoroutineScope()
    LaunchedEffect(agents) { agents.forEach { scope.launch { store.refresh(it) } } }

    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 18.dp, end = 18.dp, bottom = 120.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp)) {
        item {
            Row(Modifier.statusBarsPadding().padding(top = 18.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(SimpleDateFormat("EEEE d MMMM", androidx.compose.ui.platform.LocalConfiguration.current.locales[0]).format(Date()).replaceFirstChar(Char::uppercase),
                        style = Bip.text(16, FontWeight.Bold, Bip.ink2))
                    Text(stringResource(R.string.home_hello), style = Bip.text(36, FontWeight.Black))
                }
                RoundButton(Icons.Rounded.Add, stringResource(R.string.add_agent), onAdd, size = 52.dp)
            }
        }
        if (agents.isEmpty()) item { Welcome(onAdd) }
        items(agents, key = { it.id }) { agent -> AgentCard(agent, latest[agent.id], online[agent.id], { onLive(agent.id) }) { onOpen(agent.id) } }
    }
}

@Composable
private fun AgentCard(agent: Agent, latest: Session?, online: Boolean?, onLive: () -> Unit, onOpen: () -> Unit) {
    val look = agent.look
    Column(Modifier.fillMaxWidth().background(look.tint, RoundedCornerShape(30.dp)).tap(onOpen).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Row(verticalAlignment = Alignment.Top) {
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(agent.name, style = Bip.text(28, FontWeight.Black), maxLines = 1, overflow = TextOverflow.Ellipsis)
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Chip(stringResource(categoryLabel(look)), look.main, look.deep, Bip.card)
                    when (online) {
                        true -> Chip(stringResource(R.string.online), Bip.online)
                        false -> Chip(stringResource(R.string.offline), Bip.muted)
                        null -> {}
                    }
                }
            }
            Mascot(look, Modifier.size(96.dp))
        }
        if (latest != null && (latest.preview != null || latest.title != null)) {
            Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).padding(14.dp)) {
                Row {
                    Text(if (latest.isBotChat) stringResource(R.string.main_thread) else latest.title ?: "", style = Bip.text(14, FontWeight.ExtraBold, Bip.ink2),
                        maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text(relative(latest.updated), style = Bip.text(14, FontWeight.ExtraBold, Bip.ink2))
                }
                latest.preview?.let {
                    Spacer(Modifier.height(2.dp))
                    Text(ChatText.preview(it), style = Bip.text(16, FontWeight.SemiBold), maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(Modifier.weight(1f).background(Bip.card, RoundedCornerShape(26.dp)).tap(onOpen).padding(vertical = 14.dp),
                horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.Keyboard, null, tint = Bip.ink)
                Spacer(Modifier.size(8.dp))
                Text(stringResource(R.string.write), style = Bip.text(18, FontWeight.ExtraBold))
            }
            Row(Modifier.weight(1f).background(Bip.ink, RoundedCornerShape(26.dp)).tap(onLive).padding(vertical = 14.dp),
                horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                Icon(Icons.Rounded.GraphicEq, null, tint = Bip.card)
                Spacer(Modifier.size(8.dp))
                Text(stringResource(R.string.live), style = Bip.text(18, FontWeight.ExtraBold, Bip.card))
            }
        }
    }
}

@Composable
private fun Welcome(onAdd: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(30.dp)).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            listOf(Category.WELLNESS, Category.FINANCE, Category.DAILY, Category.WORK, Category.LEARNING).forEach { Mascot(it, Modifier.size(56.dp)) }
        }
        Text(stringResource(R.string.welcome_title), style = Bip.text(22, FontWeight.Black))
        Text(stringResource(R.string.welcome_body), style = Bip.text(16, color = Bip.ink2))
        Row(Modifier.background(Bip.ink, RoundedCornerShape(26.dp)).tap(onAdd).padding(horizontal = 22.dp, vertical = 14.dp),
            verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Rounded.QrCodeScanner, null, tint = Bip.card)
            Spacer(Modifier.size(8.dp))
            Text(stringResource(R.string.scan_qr), style = Bip.text(17, FontWeight.ExtraBold, Bip.card))
        }
    }
}

fun categoryLabel(c: Category): Int = when (c) {
    Category.WELLNESS -> R.string.cat_wellness
    Category.FINANCE -> R.string.cat_finance
    Category.DAILY -> R.string.cat_daily
    Category.WORK -> R.string.cat_work
    Category.LEARNING -> R.string.cat_learning
    Category.HOME -> R.string.cat_home
    Category.CREATIVE -> R.string.cat_creative
    Category.TECH -> R.string.cat_tech
}
