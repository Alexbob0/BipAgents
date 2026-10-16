package io.github.bipagents

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.Tune
import androidx.compose.material.icons.rounded.GridView
import androidx.compose.material3.Icon
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import io.github.bipagents.ui.AddAgentScreen
import io.github.bipagents.ui.Bip
import io.github.bipagents.ui.BipTheme
import io.github.bipagents.ui.ChatScreen
import io.github.bipagents.ui.HomeScreen
import io.github.bipagents.ui.InboxScreen
import io.github.bipagents.ui.SessionsScreen
import io.github.bipagents.ui.SettingsScreen
import io.github.bipagents.ui.tap

class MainActivity : ComponentActivity() {
    private var opened by androidx.compose.runtime.mutableStateOf<Pair<String, String?>?>(null)

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        intent.getStringExtra("agent")?.let { opened = it to intent.getStringExtra("session") }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        intent.getStringExtra("agent")?.let { opened = it to intent.getStringExtra("session") }
        enableEdgeToEdge()
        val store = (application as BipApp).store
        // Debug builds only: `adb shell am start -n io.github.bipagents/.MainActivity --es pair '<payload>'` adds an
        // install as its QR code would (the emulator's camera can't see a code on the Mac's screen).
        // Debug builds only: `--es push '{"agent":"wellness","category":"APPROVAL","run_id":"…"}'` shows the notification
        // a push with that data would (without a Firebase project).
        if (BuildConfig.DEBUG) intent.getStringExtra("push")?.let { payload ->
            val data = io.github.bipagents.net.parseJson(payload).let { it as? kotlinx.serialization.json.JsonObject }
                ?.mapValues { (_, v) -> (v as? kotlinx.serialization.json.JsonPrimitive)?.content ?: v.toString() } ?: emptyMap()
            lifecycleScope.launch { io.github.bipagents.push.Notifier(this@MainActivity).handle(data) }
        }
        if (BuildConfig.DEBUG) intent.getStringExtra("pair")?.let { payload ->
            io.github.bipagents.model.Pairing.parse(payload)?.let { lifecycleScope.launch { runCatching { store.pair(it) } } }
        }
        // Notifications: the permission (Android 13+), then this phone's token to each bridge (again when agents change).
        if (android.os.Build.VERSION.SDK_INT >= 33 && io.github.bipagents.push.Push.configured &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        lifecycleScope.launch { store.agents.collect { io.github.bipagents.push.Push.register(store) } }
        setContent {
            BipTheme {
                val nav = rememberNavController()
                // A notification tapped: its conversation.
                androidx.compose.runtime.LaunchedEffect(opened) {
                    opened?.let { (agent, session) -> nav.navigate("chat/$agent" + (session?.let { "?session=$it" } ?: "")) }
                    opened = null
                }
                val route = nav.currentBackStackEntryAsState().value?.destination?.route
                Box(Modifier.fillMaxSize().background(Bip.ground)) {
                    NavHost(nav, startDestination = "home") {
                        composable("home") {
                            HomeScreen(store, onAdd = { nav.navigate("add") }, onOpen = { agent -> nav.navigate("chat/$agent") })
                        }
                        composable("inbox") { InboxScreen(store, onOpen = { agent, session -> nav.navigate("chat/$agent?session=$session") }) }
                        composable("settings") { SettingsScreen(store, onAdd = { nav.navigate("add") }) }
                        composable("add") { AddAgentScreen(store, onDone = { nav.popBackStack() }) }
                        composable("chat/{agent}?session={session}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            ChatScreen(store, agent, entry.arguments?.getString("session"), onBack = { nav.popBackStack() },
                                onSessions = { nav.navigate("sessions/$agent") })
                        }
                        composable("sessions/{agent}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            SessionsScreen(store, agent, onBack = { nav.popBackStack() }, onOpen = { session ->
                                nav.navigate("chat/$agent?session=${session ?: ""}") { popUpTo("home") }
                            })
                        }
                    }
                    if (route in tabs.map { it.route }) TabBar(nav, route, Modifier.align(Alignment.BottomCenter))
                }
            }
        }
    }
}

private data class Tab(val route: String, val label: Int, val icon: ImageVector)

private val tabs = listOf(
    Tab("home", R.string.tab_agents, Icons.Rounded.GridView),
    Tab("inbox", R.string.tab_inbox, Icons.Outlined.Inbox),
    Tab("settings", R.string.tab_settings, Icons.Outlined.Tune),
)

/** The floating tab bar of the iPhone app. */
@Composable
private fun TabBar(nav: NavHostController, current: String?, modifier: Modifier) {
    Surface(modifier.navigationBarsPadding().padding(bottom = 10.dp), shape = RoundedCornerShape(36.dp), color = Bip.card,
        shadowElevation = 10.dp) {
        Row(Modifier.padding(6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (tab in tabs) {
                val on = tab.route == current
                Column(Modifier.background(if (on) Bip.line else Bip.card, RoundedCornerShape(30.dp))
                    .tap { if (!on) nav.navigate(tab.route) { popUpTo("home"); launchSingleTop = true } }
                    .padding(horizontal = 22.dp, vertical = 7.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Icon(tab.icon, null, Modifier.size(24.dp), tint = Bip.ink)
                    Text(stringResource(tab.label), style = Bip.text(12, if (on) FontWeight.ExtraBold else FontWeight.SemiBold))
                }
            }
        }
    }
}
