package io.github.bipagents.widget

import android.appwidget.AppWidgetManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.glance.ColorFilter
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Box
import androidx.glance.layout.Column as GColumn
import androidx.glance.layout.Row as GRow
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxSize as gFillMaxSize
import androidx.glance.layout.fillMaxWidth as gFillMaxWidth
import androidx.glance.layout.padding as gPadding
import androidx.glance.layout.size as gSize
import androidx.glance.layout.width
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import io.github.bipagents.BipApp
import io.github.bipagents.MainActivity
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.ui.Bip
import io.github.bipagents.ui.BipTheme
import io.github.bipagents.ui.ChatText
import io.github.bipagents.ui.Mascot
import io.github.bipagents.ui.mascotBitmap
import io.github.bipagents.ui.tap

/** One agent on the home screen (BipWidgets): its Bip, name and last message. A tap opens its conversation; the
 *  microphone opens it already recording a voice note. The agent is chosen when the widget is placed. */
class AgentWidget : GlanceAppWidget() {
    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val widget = GlanceAppWidgetManager(context).getAppWidgetId(id)
        val store = (context.applicationContext as BipApp).store
        val agent = WidgetAgents.agent(context, widget)?.let(store::agent) ?: store.agents.value.firstOrNull()
        val preview = agent?.let { store.latest.value[it.id]?.preview }?.let(ChatText::preview)
        val bip = agent?.let { mascotBitmap(it.look, 256) }
        provideContent { Content(context, agent, preview, bip) }
    }

    @Composable
    private fun Content(context: Context, agent: Agent?, preview: String?, bip: android.graphics.Bitmap?) {
        if (agent == null) {
            Box(GlanceModifier.gFillMaxSize().background(ColorProvider(Bip.card)).cornerRadius(24.dp), contentAlignment = Alignment.Center) {
                Text(context.getString(R.string.widget_no_agent), style = TextStyle(color = ColorProvider(Bip.ink2)))
            }
            return
        }
        val look = agent.look
        GRow(GlanceModifier.gFillMaxSize().background(ColorProvider(look.lightTint)).cornerRadius(24.dp).gPadding(12.dp)
            .clickable(actionStartActivity(link(context, agent, null))), verticalAlignment = Alignment.CenterVertically) {
            bip?.let { Image(ImageProvider(it), agent.name, GlanceModifier.gSize(56.dp)) }
            Spacer(GlanceModifier.width(10.dp))
            GColumn(GlanceModifier.defaultWeight()) {
                Text(agent.name, maxLines = 1, style = TextStyle(fontSize = 17.sp, fontWeight = FontWeight.Bold, color = ColorProvider(Bip.ink)))
                Text(preview ?: context.getString(R.string.widget_tap_to_talk), maxLines = 2,
                    style = TextStyle(fontSize = 13.sp, color = ColorProvider(android.graphics.Color.parseColor("#4A5163").let { androidx.compose.ui.graphics.Color(it) })))
            }
            Spacer(GlanceModifier.width(8.dp))
            Box(GlanceModifier.gSize(44.dp).background(ColorProvider(look.deep)).cornerRadius(22.dp).clickable(actionStartActivity(link(context, agent, "voice"))),
                contentAlignment = Alignment.Center) {
                Image(ImageProvider(R.drawable.ic_mic), context.getString(R.string.hold_to_talk), GlanceModifier.gSize(22.dp),
                    colorFilter = ColorFilter.tint(ColorProvider(androidx.compose.ui.graphics.Color.White)))
            }
        }
    }

    private fun link(context: Context, agent: Agent, start: String?) = Intent(Intent.ACTION_VIEW,
        Uri.parse("bipagents://agent/${agent.id}" + (start?.let { "?start=$it" } ?: "")), context, MainActivity::class.java)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)

    companion object {
        /** The widgets show what changed (a reply, a look). */
        suspend fun refresh(context: Context) = runCatching { AgentWidget().updateAll(context) }
    }
}

class AgentWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = AgentWidget()
}

/** Which agent each widget shows. */
object WidgetAgents {
    private fun prefs(context: Context) = context.getSharedPreferences("widgets", Context.MODE_PRIVATE)
    fun agent(context: Context, widget: Int): String? = prefs(context).getString("w$widget", null)
    fun set(context: Context, widget: Int, agent: String) = prefs(context).edit().putString("w$widget", agent).apply()
}

/** Placing the widget: which agent it shows. */
class WidgetConfigActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val widget = intent.getIntExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID)
        setResult(RESULT_CANCELED, Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, widget))
        val agents = (application as BipApp).store.agents.value
        setContent {
            BipTheme {
                Column(Modifier.fillMaxSize().background(Bip.ground).statusBarsPadding().padding(18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    androidx.compose.material3.Text(getString(R.string.widget_choose_agent), style = Bip.text(24, androidx.compose.ui.text.font.FontWeight.Black))
                    for (agent in agents) Row(Modifier.fillMaxWidth().background(agent.look.tint, RoundedCornerShape(20.dp)).tap {
                        WidgetAgents.set(this@WidgetConfigActivity, widget, agent.id)
                        kotlinx.coroutines.runBlocking { AgentWidget.refresh(this@WidgetConfigActivity) }
                        setResult(RESULT_OK, Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, widget))
                        finish()
                    }.padding(14.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Mascot(agent.look, Modifier.size(44.dp), animated = false)
                        androidx.compose.material3.Text(agent.name, style = Bip.text(18, androidx.compose.ui.text.font.FontWeight.ExtraBold))
                    }
                }
            }
        }
    }
}

private typealias Modifier = androidx.compose.ui.Modifier
