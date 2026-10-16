package io.github.bipagents.push

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.widget.Toast
import androidx.core.app.RemoteInput
import io.github.bipagents.BipApp
import io.github.bipagents.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** « Reply » typed in a message's notification: sent to its conversation as a new turn (a Hermes run, followed by the
 *  bridge, which notifies when the answer is ready); the conversation follows it too when opened. */
class ReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val reply = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(KEY)?.toString()?.trim()?.ifEmpty { null } ?: return
        val store = (context.applicationContext as BipApp).store
        val agent = store.agent(intent.getStringExtra("agent") ?: return) ?: return
        val session = intent.getStringExtra("session") ?: return
        val id = intent.getIntExtra("notification", 0)
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            val run = runCatching { store.hermes(agent)?.startRun(session, buildJsonObject { put("input", JsonPrimitive(reply)) }) }.getOrNull()
            if (run != null) context.getSharedPreferences("runs", Context.MODE_PRIVATE).edit()
                .putString(session, run).putString("sent.$session", reply).apply()
            launch(Dispatchers.Main) {
                if (run != null) Notifier(context).replied(agent, id, intent.getStringExtra("text") ?: "", reply)
                else Toast.makeText(context, R.string.error_unreachable, Toast.LENGTH_LONG).show()
                pending.finish()
            }
        }
    }

    companion object { const val KEY = "reply" }
}
