package io.github.bipagents.ui

import android.annotation.SuppressLint
import android.content.ClipboardManager
import android.content.Context
import android.content.pm.ActivityInfo
import android.webkit.JavascriptInterface
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.Desktop
import kotlinx.coroutines.launch
import org.json.JSONObject

/**
 * « Bureau » (DesktopView.swift): the agent's own browser, live, full screen and in landscape too (the bridge's noVNC
 * page has the zoom, the pan and the key bar). « Prendre la main » makes the agent's browser tools wait while the person
 * types (a password…); leaving gives it back.
 */
@SuppressLint("SetJavaScriptEnabled")
@Composable
fun DesktopScreen(store: AgentStore, agentId: String, onClose: () -> Unit) {
    val agent = store.agent(agentId) ?: return
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var desktop by remember { mutableStateOf<Desktop?>(null) }
    var holding by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    val bridge = store.bridge(agent)
    // Its browser shows the person's accounts: the phone's lock first (DesktopView.swift's Face ID).
    var unlocked by remember { mutableStateOf(false) }
    var noLock by remember { mutableStateOf(false) }
    val activity = context as? androidx.fragment.app.FragmentActivity
    val title = stringResource(R.string.unlock_desktop, agent.name)
    LaunchedEffect(Unit) {
        if (activity == null || !DeviceLock.available(activity)) { noLock = true; return@LaunchedEffect }
        DeviceLock.ask(activity, title) { ok -> if (ok) unlocked = true else onClose() }
    }
    LaunchedEffect(unlocked) {
        if (!unlocked) return@LaunchedEffect
        desktop = runCatching { bridge?.desktop(agent.bridgeName) }.getOrNull()
        failed = desktop == null
        holding = desktop?.youHold == true
    }
    // Landscape allowed here only; the hold is given back on leaving.
    DisposableEffect(Unit) {
        val activity = context as? android.app.Activity
        val before = activity?.requestedOrientation
        activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_FULL_USER
        onDispose {
            activity?.requestedOrientation = before ?: ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            if (holding) kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.IO).launch {
                runCatching { bridge?.controlDesktop(agent.bridgeName, false) }
            }
        }
    }
    Box(Modifier.fillMaxSize().background(Color(0xFF111111))) {
        desktop?.let { d ->
            AndroidView(factory = { ctx ->
                WebView(ctx).apply {
                    settings.javaScriptEnabled = true
                    settings.domStorageEnabled = true
                    webViewClient = WebViewClient()
                    // The page asks the app to paste (the web clipboard needs a permission it can't get here).
                    addJavascriptInterface(object {
                        @JavascriptInterface fun post(event: String) {
                            if (event != "paste-refused") return
                            val text = (ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip
                                ?.getItemAt(0)?.coerceToText(ctx)?.toString() ?: return
                            post { evaluateJavascript("window.typeText && window.typeText(${JSONObject.quote(text)})", null) }
                        }
                    }, "BipDesktop")
                    evaluateJavascript("", null)
                    webViewClient = object : WebViewClient() {
                        override fun onPageFinished(view: WebView, url: String) {
                            view.evaluateJavascript("window.webkit={messageHandlers:{desktop:{postMessage:function(e){BipDesktop.post(String(e))}}}}", null)
                        }
                    }
                    loadUrl(d.url)
                }
            }, modifier = Modifier.fillMaxSize().safeDrawingPadding())
        }
        if (noLock) Text(stringResource(R.string.needs_screen_lock), style = Bip.text(16, FontWeight.Bold, Color.White),
            modifier = Modifier.align(Alignment.Center).padding(32.dp))
        if (failed) Text(stringResource(R.string.desktop_none), style = Bip.text(16, FontWeight.Bold, Color.White), modifier = Modifier.align(Alignment.Center))
        Row(Modifier.safeDrawingPadding().padding(10.dp).align(Alignment.TopEnd), horizontalArrangement = Arrangement.spacedBy(8.dp),
            verticalAlignment = Alignment.CenterVertically) {
            if (desktop?.relayed == true) Text(stringResource(if (holding) R.string.hand_back else R.string.take_over),
                style = Bip.text(15, FontWeight.ExtraBold, if (holding) Bip.card else Bip.ink),
                modifier = Modifier.background(if (holding) Bip.danger else Bip.card, RoundedCornerShape(50)).tap {
                    scope.launch { holding = runCatching { bridge?.controlDesktop(agent.bridgeName, !holding) }.getOrNull() ?: holding }
                }.padding(horizontal = 16.dp, vertical = 10.dp))
            RoundButton(Icons.Rounded.Close, stringResource(R.string.cancel), onClose, size = 44.dp)
        }
    }
}
