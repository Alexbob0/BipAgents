package io.github.bipagents.push

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.widget.Toast
import androidx.core.app.NotificationManagerCompat
import io.github.bipagents.BipApp
import io.github.bipagents.R
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/** « Allow once » / « Deny » from the notification, without opening the app. */
class ApprovalReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val store = (context.applicationContext as BipApp).store
        val agent = store.agent(intent.getStringExtra("agent") ?: return) ?: return
        val run = intent.getStringExtra("run") ?: return
        val choice = intent.getStringExtra("choice") ?: return
        val pending = goAsync()
        CoroutineScope(Dispatchers.IO).launch {
            val ok = runCatching { store.hermes(agent)?.approve(run, choice, intent.getStringExtra("request")) }.isSuccess
            launch(Dispatchers.Main) {
                if (ok) NotificationManagerCompat.from(context).cancel(intent.getIntExtra("notification", 0))
                else Toast.makeText(context, R.string.error_unreachable, Toast.LENGTH_LONG).show()
                pending.finish()
            }
        }
    }
}
