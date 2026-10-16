package io.github.bipagents.push

import android.app.PendingIntent
import android.content.Intent
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.graphics.drawable.IconCompat
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import io.github.bipagents.BipApp
import io.github.bipagents.MainActivity
import io.github.bipagents.R
import io.github.bipagents.model.Agent
import io.github.bipagents.net.list
import io.github.bipagents.net.obj
import io.github.bipagents.net.str
import io.github.bipagents.ui.ChatText
import io.github.bipagents.ui.mascotBitmap
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull

/** A push from the bridge (NotificationService.swift): the text comes from the bridge over the tailnet, the
 *  notification shows it as a message from the agent's Bip, and an approval gets its buttons. */
class BipMessagingService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        runBlocking { withTimeoutOrNull(15_000) { Push.register((application as BipApp).store, token) } }
    }

    // Runs on FCM's worker thread, with ~10 s to finish.
    override fun onMessageReceived(message: RemoteMessage) = runBlocking { Notifier(this@BipMessagingService).handle(message.data) }
}

/** Builds the notification for a push's data (also reachable from a debug build, to try it without Firebase). */
class Notifier(private val context: android.content.Context) {
    private val store get() = (context.applicationContext as BipApp).store
    private fun getString(id: Int, vararg args: Any) = context.getString(id, *args)

    suspend fun handle(data: Map<String, String>) {
        if (data["silent"] == "1") return
        val agent = store.agents.value.firstOrNull { it.bridgeName == data["agent"] } ?: return
        val approval = data["category"] == "APPROVAL"
        // The bridge is asked for the text, else the alert's body stays.
        val text = withTimeoutOrNull(8_000) { runCatching { fetch(agent, data, approval) }.getOrNull() }
            ?: data["body"] ?: getString(if (approval) R.string.approval_needed else R.string.new_message)
        show(agent, data, text, approval)
    }

    private suspend fun fetch(agent: Agent, data: Map<String, String>, approval: Boolean): String? {
        val bridge = store.bridge(agent) ?: return null
        data["outbox_id"]?.let { id -> return bridge.api.call("v1/outbox/$id").obj().str("text", "body")?.let(::clean) }
        if (approval) {
            val item = bridge.api.call("v1/approvals", query = mapOf("agent" to agent.bridgeName)).list("items")
                .firstOrNull { it.str("run_id") == data["run_id"] }
            return item.str("command")?.let { getString(R.string.wants_to_run, it) } ?: item.str("description")
        }
        data["reply_id"]?.let { id -> return bridge.api.call("v1/replies/$id").obj().str("text")?.let(::clean) }
        return null
    }

    private fun clean(text: String) = ChatText.visible(text).replace(Regex("\\*\\*|__|`|^#+ ", RegexOption.MULTILINE), "").take(1000)

    private fun show(agent: Agent, data: Map<String, String>, text: String, approval: Boolean) {
        val id = (data["run_id"] ?: data["outbox_id"] ?: data["reply_id"] ?: text).hashCode()
        val open = PendingIntent.getActivity(context, id, Intent(context, MainActivity::class.java)
            .putExtra("agent", agent.id).putExtra("session", data["session_id"])
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val bip = Person.Builder().setName(agent.name).setKey(agent.id).setIcon(IconCompat.createWithBitmap(mascotBitmap(agent.look))).build()
        val me = Person.Builder().setName(getString(R.string.me)).build()
        val builder = NotificationCompat.Builder(context, if (approval) Push.APPROVALS else Push.MESSAGES)
            .setSmallIcon(R.drawable.ic_notification)
            .setColor(agent.look.main.toArgbInt())
            .setStyle(NotificationCompat.MessagingStyle(me).addMessage(text, System.currentTimeMillis(), bip))
            .setContentIntent(open)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
        if (approval) {
            val run = data["run_id"] ?: return
            // « Always » is never offered from the app; the others answer from the notification.
            for ((choice, label) in listOf("once" to R.string.allow_once, "deny" to R.string.deny)) {
                val intent = Intent(context, ApprovalReceiver::class.java).putExtra("agent", agent.id).putExtra("run", run)
                    .putExtra("request", data["request_id"]).putExtra("choice", choice).putExtra("notification", id)
                builder.addAction(0, getString(label), PendingIntent.getBroadcast(context, "$id$choice".hashCode(), intent,
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
            }
        }
        if (androidx.core.content.ContextCompat.checkSelfPermission(context, android.Manifest.permission.POST_NOTIFICATIONS)
            == android.content.pm.PackageManager.PERMISSION_GRANTED) {
            NotificationManagerCompat.from(context).notify(id, builder.build())
        }
    }

    private fun androidx.compose.ui.graphics.Color.toArgbInt(): Int = android.graphics.Color.argb(
        (alpha * 255).toInt(), (red * 255).toInt(), (green * 255).toInt(), (blue * 255).toInt())
}
