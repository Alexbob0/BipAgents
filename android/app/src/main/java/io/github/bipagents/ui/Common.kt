package io.github.bipagents.ui

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.net.HttpError
import androidx.compose.ui.platform.LocalContext

/** A click without the ripple, for cards and pills that have their own look. */
fun Modifier.tap(onClick: () -> Unit): Modifier = composed {
    clickable(remember { MutableInteractionSource() }, indication = null, onClick = onClick)
}

/** A round icon button on a white disc (the iPhone app's « + »). */
@Composable
fun RoundButton(icon: ImageVector, label: String, onClick: () -> Unit, size: Dp = 48.dp, background: Color = Bip.card, tint: Color = Bip.ink) {
    Box(Modifier.size(size).background(background, CircleShape).tap(onClick), contentAlignment = Alignment.Center) {
        Icon(icon, label, tint = tint)
    }
}

/** A screen's top bar: back, a title, actions. */
@Composable
fun TopBar(title: String, onBack: () -> Unit, leading: @Composable () -> Unit = {}, actions: @Composable () -> Unit = {}) {
    Row(Modifier.statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        RoundButton(Icons.AutoMirrored.Rounded.ArrowBack, androidx.compose.ui.res.stringResource(io.github.bipagents.R.string.back), onBack, size = 44.dp)
        leading()
        Text(title, style = Bip.text(19, FontWeight.ExtraBold), maxLines = 1, modifier = Modifier.weight(1f))
        actions()
    }
}

/** A dot and a word (« Online »), as on the home cards. */
@Composable
fun Chip(text: String, dot: Color, textColor: Color = Bip.ink2, background: Color? = null) {
    Row(Modifier.then(if (background != null) Modifier.background(background, RoundedCornerShape(50)).padding(horizontal = 10.dp, vertical = 4.dp) else Modifier),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.size(7.dp).background(dot, CircleShape))
        Text(text, style = Bip.text(14, FontWeight.ExtraBold, textColor))
    }
}

@Composable
fun relative(time: Long?): String = when {
    time == null -> ""
    kotlin.math.abs(System.currentTimeMillis() - time) < 60_000 -> androidx.compose.ui.res.stringResource(R.string.just_now)
    else -> DateUtils.getRelativeTimeSpanString(time, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS).toString()
}

fun clock(time: Long?): String =
    if (time == null) "" else java.text.DateFormat.getTimeInstance(java.text.DateFormat.SHORT).format(java.util.Date(time))

/** A failure in the person's words. */
@Composable
fun describe(error: Throwable): String {
    val context = LocalContext.current
    return describe(context, error)
}

fun describe(context: android.content.Context, error: Throwable): String = when {
    error is HttpError && error.unreachable -> context.getString(R.string.error_unreachable)
    error is HttpError && (error.status == 401 || error.status == 403) -> context.getString(R.string.error_key)
    error is HttpError -> context.getString(R.string.error_server, error.status) + (error.message?.let { " — $it" } ?: "")
    else -> error.message ?: error.toString()
}

/** How an agent answered lately (ReachabilityLabel): online, its model down, its key refused, out of reach. */
@Composable
fun ReachChip(reach: io.github.bipagents.model.AgentStore.Reach?) {
    when (reach) {
        io.github.bipagents.model.AgentStore.Reach.ONLINE -> Chip(androidx.compose.ui.res.stringResource(R.string.online), Bip.online)
        io.github.bipagents.model.AgentStore.Reach.LOCAL -> Chip(androidx.compose.ui.res.stringResource(R.string.online_local), Bip.online)
        io.github.bipagents.model.AgentStore.Reach.MODEL_DOWN -> Chip(androidx.compose.ui.res.stringResource(R.string.model_down), Color(0xFFF29A2E))
        io.github.bipagents.model.AgentStore.Reach.UNAUTHORIZED -> Chip(androidx.compose.ui.res.stringResource(R.string.key_refused), Bip.danger)
        io.github.bipagents.model.AgentStore.Reach.OFFLINE -> Chip(androidx.compose.ui.res.stringResource(R.string.offline), Bip.danger)
        null -> {}
    }
}
