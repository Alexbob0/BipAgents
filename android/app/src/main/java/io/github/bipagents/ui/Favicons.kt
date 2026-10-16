package io.github.bipagents.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Language
import androidx.compose.material3.Icon
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.bipagents.net.Http
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import okhttp3.Request

/** Site icons before links in the agents' replies (Favicons.swift), loaded on demand and kept for the session. From
 *  DuckDuckGo's icon service: one address per host, no cookies, and the site itself does not learn the reply was read. */
object Favicons {
    private val icons = mutableStateMapOf<String, ImageBitmap>()
    private val requested = HashSet<String>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)

    fun id(host: String) = "fav:$host"

    private fun request(host: String) {
        if (!requested.add(host)) return
        scope.launch {
            val bitmap = runCatching {
                Http.client.newCall(Request.Builder().url("https://icons.duckduckgo.com/ip3/$host.ico").build()).execute().use { r ->
                    if (!r.isSuccessful) null else r.body.bytes().let { BitmapFactory.decodeByteArray(it, 0, it.size) }
                }
            }.getOrNull()?.takeIf { it.width >= 8 } ?: return@launch
            withContext(Dispatchers.Main) { icons[host] = bitmap.asImageBitmap() }
        }
    }

    /** The inline icons for these hosts: the site's, or a globe until it comes (or if it has none). */
    @Composable
    fun content(hosts: Set<String>, tint: Color): Map<String, InlineTextContent> = hosts.associate { host ->
        request(host)
        id(host) to InlineTextContent(Placeholder(20.sp, 16.sp, PlaceholderVerticalAlign.TextCenter)) {
            Box(Modifier.fillMaxSize().padding(end = 4.dp)) {
                val icon = icons[host]
                if (icon != null) Image(icon, null, Modifier.fillMaxSize().clip(RoundedCornerShape(4.dp)))
                else Icon(Icons.Rounded.Language, null, Modifier.fillMaxSize(), tint = tint)
            }
        }
    }
}
