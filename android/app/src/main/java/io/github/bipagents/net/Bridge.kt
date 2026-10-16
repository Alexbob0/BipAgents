package io.github.bipagents.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.MultipartBody
import okhttp3.RequestBody.Companion.toRequestBody

data class ListedAgent(val id: String, val name: String, val url: String?, val key: String?)

data class InboxItem(val id: String, val agent: String, val title: String?, val text: String, val time: Long?,
                     val session: String?, val hasAudio: Boolean)

/** The install's bridge, with its key (App/Model/BridgeClient.swift). */
class Bridge(val api: Api) {
    suspend fun agents(): List<ListedAgent> = api.call("v1/agents").obj()?.get("agents").list().mapNotNull { o ->
        val id = o.str("id") ?: return@mapNotNull null
        ListedAgent(id, o.str("name") ?: id.replaceFirstChar(Char::uppercase), o.str("url"), o.str("key"))
    }

    /** A run's events through the bridge: it holds the single Hermes subscription, replays the run from its start, and
     *  knows someone is watching (no push meanwhile). */
    fun runEvents(agent: String, run: String, session: String?): Flow<RunEvent> =
        api.events("v1/runs/${Hermes.seg(run)}/events", mapOf("agent" to agent, "session_id" to session)).mapNotNull(RunEvent::of)

    suspend fun inbox(limit: Int = 100): List<InboxItem> = api.call("v1/outbox", query = mapOf("limit" to "$limit")).list("items").mapNotNull { o ->
        val id = o.str("id") ?: return@mapNotNull null
        InboxItem(id, o.str("agent") ?: "", o.str("title"), o.str("text") ?: "", o.time("sent_at", "created_at"), o.str("session_id"),
            o.bool("has_audio") ?: false)
    }

    suspend fun media(path: String): ByteArray = api.bytes("v1/media", query = mapOf("path" to path))

    /** A file for the agent (`POST /v1/files`): the path where the agent finds it. */
    suspend fun upload(agent: String, name: String, mime: String, bytes: ByteArray): String {
        val body = MultipartBody.Builder().setType(MultipartBody.FORM)
            .addFormDataPart("agent", agent)
            .addFormDataPart("file", name, bytes.toRequestBody(mime.toMediaTypeOrNull()))
            .build()
        val reply = parseJson(api.bytes("v1/files", "POST", body).decodeToString()).obj()
        return reply.str("path") ?: throw HttpError(502, "upload: no path")
    }
}
