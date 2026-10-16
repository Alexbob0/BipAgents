package io.github.bipagents

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import androidx.compose.animation.AnimatedContentTransitionScope
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.collectAsState
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
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
import io.github.bipagents.ui.LiveScreen
import io.github.bipagents.ui.AgentScreen
import io.github.bipagents.ui.SettingsScreen
import io.github.bipagents.ui.tap

class MainActivity : androidx.fragment.app.FragmentActivity() {
    /** A conversation to open: from a notification, or a link (`bipagents://agent/<id>[?start=voice|keyboard]`, the widget). */
    private data class Open(val agent: String, val session: String?, val start: String? = null)
    private var opened by androidx.compose.runtime.mutableStateOf<Open?>(null)

    private fun read(intent: android.content.Intent) {
        intent.getStringExtra("agent")?.let { opened = Open(it, intent.getStringExtra("session")); return }
        val uri = intent.data ?: return
        if (uri.scheme != "bipagents" || uri.host != "agent") return
        val id = uri.pathSegments.firstOrNull() ?: return
        val store = (application as BipApp).store
        if (store.agent(id) == null) return
        // Its ongoing conversation, as « Write » opens it.
        opened = Open(id, store.latest.value[id]?.id, uri.getQueryParameter("start"))
    }

    override fun onNewIntent(intent: android.content.Intent) {
        super.onNewIntent(intent)
        read(intent)
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        read(intent)
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
        // The Inbox's count on its tab, from the start (and on each return to the app).
        lifecycle.addObserver(androidx.lifecycle.LifecycleEventObserver { _, event ->
            if (event == androidx.lifecycle.Lifecycle.Event.ON_START) lifecycleScope.launch { runCatching { (application as BipApp).inbox.refresh(store) } }
        })
        setContent {
            BipTheme {
                val nav = rememberNavController()
                // A notification tapped: its conversation.
                androidx.compose.runtime.LaunchedEffect(opened) {
                    opened?.let { o -> nav.navigate(chatRoute(o.agent, o.session, o.start)) }
                    opened = null
                }
                val route = nav.currentBackStackEntryAsState().value?.destination?.route
                Box(Modifier.fillMaxSize().background(Bip.ground)) {
                    NavHost(nav, startDestination = "home",
                        // The iPhone's: tabs switch at once, screens slide in from the right, the Live rises from below.
                        enterTransition = {
                            when {
                                isTab(initialState.destination.route) && isTab(targetState.destination.route) -> EnterTransition.None
                                isLive(targetState.destination.route) -> slideInVertically(tween(320)) { it }
                                else -> slideIntoContainer(AnimatedContentTransitionScope.SlideDirection.Start, tween(320))
                            }
                        },
                        exitTransition = {
                            when {
                                isTab(initialState.destination.route) && isTab(targetState.destination.route) -> ExitTransition.None
                                isLive(targetState.destination.route) -> ExitTransition.None
                                else -> slideOutOfContainer(AnimatedContentTransitionScope.SlideDirection.Start, tween(320)) { it / 4 }
                            }
                        },
                        popEnterTransition = {
                            if (isLive(initialState.destination.route)) EnterTransition.None
                            else slideIntoContainer(AnimatedContentTransitionScope.SlideDirection.End, tween(320)) { it / 4 }
                        },
                        popExitTransition = {
                            if (isLive(initialState.destination.route)) slideOutVertically(tween(320)) { it }
                            else slideOutOfContainer(AnimatedContentTransitionScope.SlideDirection.End, tween(320))
                        }) {
                        composable("home") {
                            HomeScreen(store, onAdd = { nav.navigate("add") }, onAgent = { agent -> nav.navigate("agent/$agent") },
                                onChat = { agent, session, start -> nav.navigate(chatRoute(agent, session, start)) },
                                onLive = { agent -> nav.navigate("live/$agent") },
                                onVoice = { agent, note ->
                                    // A voice note from home goes to the agent's latest conversation, sent from there.
                                    io.github.bipagents.ui.PendingVoice.put(agent, note)
                                    nav.navigate("chat/$agent" + (store.latest.value[agent]?.id?.let { "?session=$it" } ?: ""))
                                })
                        }
                        composable("inbox") {
                            InboxScreen(store, onOpen = { agent, session -> nav.navigate("chat/$agent" + (session?.let { "?session=$it" } ?: "")) },
                                onLive = { agent -> nav.navigate("live/$agent") })
                        }
                        composable("settings") {
                            SettingsScreen(store, onAdd = { nav.navigate("add") }, onAgent = { nav.navigate("options/$it") },
                                onPage = { nav.navigate("settings/$it") })
                        }
                        composable("settings/{page}") { entry ->
                            val back = { nav.popBackStack(); Unit }
                            when (entry.arguments?.getString("page")) {
                                "links" -> io.github.bipagents.ui.AgentLinksScreen(store, back)
                                "shares" -> io.github.bipagents.ui.AgentSharesScreen(store, back)
                                "family" -> io.github.bipagents.ui.FamilyScreen(store, back)
                                "vault" -> io.github.bipagents.ui.VaultScreen(store, back)
                                "browsers" -> io.github.bipagents.ui.ConsolesScreen(store, back)
                                "cron" -> io.github.bipagents.ui.CronJobsScreen(store, back)
                                "voicemodel" -> io.github.bipagents.ui.VoiceModelScreen(store, back)
                                "diagnostics" -> io.github.bipagents.ui.DiagnosticsScreen(store, back)
                            }
                        }
                        composable("add") { AddAgentScreen(store, onDone = { nav.popBackStack() }, onCreate = { nav.navigate("create") }) }
                        composable("create") {
                            io.github.bipagents.ui.CreateAgentScreen(store, onBack = { nav.popBackStack() },
                                onCreated = { nav.popBackStack("home", false) })
                        }
                        composable("chat/{agent}?session={session}&start={start}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            ChatScreen(store, agent, entry.arguments?.getString("session"), start = entry.arguments?.getString("start"),
                                onBack = { nav.popBackStack() },
                                onAgent = { nav.navigate("agent/$agent") },
                                onLive = { session -> nav.navigate("live/$agent" + (session?.let { "?session=$it" } ?: "")) },
                                onForward = { target, session -> nav.navigate("chat/$target" + (session?.let { "?session=$it" } ?: "")) })
                        }
                        composable("live/{agent}?session={session}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            LiveScreen(store, agent, entry.arguments?.getString("session"), onEnd = { nav.popBackStack() },
                                onKeyboard = { session ->
                                    nav.popBackStack()
                                    nav.navigate("chat/$agent" + (session?.let { "?session=$it" } ?: ""))
                                })
                        }
                        composable("agent/{agent}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            AgentScreen(store, agent, onBack = { nav.popBackStack() },
                                onOpen = { session -> nav.navigate("chat/$agent?session=${session ?: ""}") },
                                onLive = { nav.navigate("live/$agent") }, onSettings = { nav.navigate("options/$agent") })
                        }
                        composable("options/{agent}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            io.github.bipagents.ui.AgentOptionsScreen(store, agent, onBack = { nav.popBackStack() },
                                onVoice = { nav.navigate("voice/$agent") }, onDesktop = { nav.navigate("desktop/$agent") },
                                onRemoved = { nav.popBackStack("home", false) },
                                onRoutines = { nav.navigate("routines/$agent") }, onSettings = { nav.navigate("agentsettings/$agent") })
                        }
                        composable("routines/{agent}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            io.github.bipagents.ui.RoutinesScreen(store, agent, onBack = { nav.popBackStack() })
                        }
                        composable("agentsettings/{agent}") { entry ->
                            val agent = entry.arguments?.getString("agent") ?: return@composable
                            io.github.bipagents.ui.AgentSettingsScreen(store, agent, onBack = { nav.popBackStack() },
                                onDeleted = { nav.popBackStack("home", false) })
                        }
                        composable("voice/{agent}") { entry ->
                            io.github.bipagents.ui.VoiceScreen(store, entry.arguments?.getString("agent") ?: return@composable, onBack = { nav.popBackStack() })
                        }
                        composable("desktop/{agent}") { entry ->
                            io.github.bipagents.ui.DesktopScreen(store, entry.arguments?.getString("agent") ?: return@composable, onClose = { nav.popBackStack() })
                        }
                    }
                    // As on the iPhone: on the tabs and on an agent's page, not in a conversation or the Live.
                    if (route in tabs.map { it.route } || route == "agent/{agent}") Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                        io.github.bipagents.ui.MiniPlayer()
                        TabBar(nav, route, Modifier)
                    }
                }
            }
        }
    }
}

private fun isTab(route: String?) = route in setOf("home", "inbox", "settings")
private fun isLive(route: String?) = route?.startsWith("live/") == true

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
                    .padding(horizontal = 30.dp, vertical = 8.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Box {
                        Icon(tab.icon, null, Modifier.size(28.dp), tint = Bip.ink)
                        // What the Inbox holds that wasn't seen yet (approvals waiting included).
                        if (tab.route == "inbox") {
                            val unread = (LocalContext.current.applicationContext as BipApp).inbox.state.collectAsState().value.unread
                            if (unread > 0) Text(if (unread > 9) "9+" else "$unread", style = Bip.text(11, FontWeight.Black, Bip.card),
                                modifier = Modifier.align(Alignment.TopEnd).offset(x = 10.dp, y = (-4).dp).background(Bip.danger, CircleShape)
                                    .padding(horizontal = 5.dp, vertical = 1.dp))
                        }
                    }
                    Text(stringResource(tab.label), style = Bip.text(13, if (on) FontWeight.ExtraBold else FontWeight.SemiBold))
                }
            }
        }
    }
}

/** A conversation's route: its session (none: the agent's ongoing one), and how it opens (keyboard, voice note). */
fun chatRoute(agent: String, session: String?, start: String? = null): String =
    "chat/$agent" + listOfNotNull(session?.let { "session=$it" }, start?.let { "start=$it" }).joinToString("&").let { if (it.isEmpty()) "" else "?$it" }
