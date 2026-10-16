package io.github.bipagents.push

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import io.github.bipagents.BuildConfig
import io.github.bipagents.R
import io.github.bipagents.model.AgentStore
import com.google.firebase.FirebaseApp
import com.google.firebase.FirebaseOptions
import com.google.firebase.messaging.FirebaseMessaging
import kotlinx.coroutines.tasks.await
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** Notifications through Firebase Cloud Messaging: the app registers its token with each bridge, which sends a data
 *  message; [BipMessagingService] builds the notification, fetching the text over the tailnet. */
object Push {
    const val MESSAGES = "messages"
    const val APPROVALS = "approvals"

    val configured get() = BuildConfig.FIREBASE_APP_ID.isNotEmpty() && BuildConfig.FIREBASE_PROJECT_ID.isNotEmpty()

    fun start(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(NotificationChannel(MESSAGES, context.getString(R.string.channel_messages), NotificationManager.IMPORTANCE_HIGH))
        manager.createNotificationChannel(NotificationChannel(APPROVALS, context.getString(R.string.channel_approvals), NotificationManager.IMPORTANCE_HIGH))
        if (!configured || FirebaseApp.getApps(context).isNotEmpty()) return
        FirebaseApp.initializeApp(context, FirebaseOptions.Builder()
            .setApplicationId(BuildConfig.FIREBASE_APP_ID)
            .setApiKey(BuildConfig.FIREBASE_API_KEY)
            .setProjectId(BuildConfig.FIREBASE_PROJECT_ID)
            .setGcmSenderId(BuildConfig.FIREBASE_SENDER_ID)
            .build())
    }

    /** Tells each bridge where to reach this phone, for its agents (`POST /v1/devices`, environment `fcm`). */
    suspend fun register(store: AgentStore, token: String? = null) {
        if (!configured) return
        val fcm = token ?: runCatching { FirebaseMessaging.getInstance().token.await() }.getOrNull() ?: return
        for ((bridge, agents) in store.bridges()) {
            runCatching {
                bridge.api.call("v1/devices", "POST", buildJsonObject {
                    put("token", JsonPrimitive(fcm))
                    put("environment", JsonPrimitive("fcm"))
                    put("agent_ids", JsonArray(agents.map { JsonPrimitive(it.bridgeName) }))
                })
            }
        }
    }
}
