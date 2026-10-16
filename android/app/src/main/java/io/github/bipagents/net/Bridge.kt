package io.github.bipagents.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull
import okhttp3.MediaType.Companion.toMediaTypeOrNull
import okhttp3.MultipartBody
import okhttp3.RequestBody.Companion.toRequestBody

data class ListedAgent(val id: String, val name: String, val url: String?, val key: String?,
                       val category: String? = null, val color: String? = null, val voice: String? = null, val serverVoice: String? = null)

data class Desktop(val url: String, val relayed: Boolean, val youHold: Boolean)

@kotlinx.serialization.Serializable
data class InboxItem(val id: String, val agent: String, val title: String?, val text: String, val time: Long?,
                     val session: String?, val hasAudio: Boolean)

/** The install's bridge, with its key (App/Model/BridgeClient.swift). */
class Bridge(val api: Api) {
    suspend fun agents(): List<ListedAgent> = api.call("v1/agents").obj()?.get("agents").list().mapNotNull { o ->
        val id = o.str("id") ?: return@mapNotNull null
        ListedAgent(id, o.str("name") ?: id.replaceFirstChar(Char::uppercase), o.str("url"), o.str("key"), o.str("category"), o.str("color"),
            o.str("voice"), o.str("server_voice"))
    }

    /** A run's events through the bridge: it holds the single Hermes subscription, replays the run from its start, and
     *  knows someone is watching (no push meanwhile). */
    fun runEvents(agent: String, run: String, session: String?): Flow<RunEvent> =
        api.events("v1/runs/${Hermes.seg(run)}/events", mapOf("agent" to agent, "session_id" to session)).mapNotNull(RunEvent::of)

    /** The same, raw: the bridge's own `bridge.error` events included. */
    fun runEventsRaw(agent: String, run: String, session: String?): Flow<ServerEvent> =
        api.events("v1/runs/${Hermes.seg(run)}/events", mapOf("agent" to agent, "session_id" to session))

    /** The agent's thinking level (none, low, medium, high, xhigh), when the app may change it; else null. */
    suspend fun thinking(agent: String): String? = try {
        api.call("v1/agents/${Hermes.seg(agent)}/thinking").obj().str("thinking")
    } catch (e: HttpError) {
        if (e.status in setOf(404, 405, 409)) null else throw e
    }

    suspend fun setThinking(agent: String, level: String) {
        api.call("v1/agents/${Hermes.seg(agent)}/thinking", "PUT", kotlinx.serialization.json.buildJsonObject {
            put("level", kotlinx.serialization.json.JsonPrimitive(level))
        })
    }

    /** The agent's look, kept by the bridge for every app (`PUT /v1/agents/{id}/appearance`). */
    suspend fun setAppearance(agent: String, category: String, color: String?) {
        api.call("v1/agents/${Hermes.seg(agent)}/appearance", "PUT", kotlinx.serialization.json.buildJsonObject {
            put("category", kotlinx.serialization.json.JsonPrimitive(category))
            put("color", color?.let { kotlinx.serialization.json.JsonPrimitive(it) } ?: kotlinx.serialization.json.JsonNull)
        })
    }

    suspend fun setVoices(agent: String, voice: String, serverVoice: String?) {
        api.call("v1/agents/${Hermes.seg(agent)}/voices", "PUT", kotlinx.serialization.json.buildJsonObject {
            put("voice", kotlinx.serialization.json.JsonPrimitive(voice))
            put("server_voice", serverVoice?.let { kotlinx.serialization.json.JsonPrimitive(it) } ?: kotlinx.serialization.json.JsonNull)
        })
    }

    suspend fun setLanguage(agent: String, lang: String) {
        api.call("v1/agents/${Hermes.seg(agent)}/language", "PUT", kotlinx.serialization.json.buildJsonObject {
            put("lang", kotlinx.serialization.json.JsonPrimitive(lang))
        })
    }

    /** `POST /v1/agents/{id}/desktop`: the page showing the agent's browser, or null when it has none running. */
    suspend fun desktop(agent: String): Desktop? = try {
        val o = api.call("v1/agents/${Hermes.seg(agent)}/desktop", "POST").obj()
        when {
            o.str("kind") == "page" -> o.str("url")?.let { Desktop(it, relayed = false, youHold = false) }
            else -> o.str("path")?.let { Desktop(api.base.trimEnd('/') + "/" + it, relayed = true, youHold = o.bool("you_hold") ?: false) }
        }
    } catch (e: HttpError) { if (e.status == 404 || e.status == 409) null else throw e }

    /** The person takes the agent's browser over (its tools wait) or hands it back. */
    suspend fun controlDesktop(agent: String, take: Boolean): Boolean =
        api.call("v1/agents/${Hermes.seg(agent)}/desktop/control", "POST", kotlinx.serialization.json.buildJsonObject {
            put("take", kotlinx.serialization.json.JsonPrimitive(take))
        }).obj().bool("you_hold") ?: false

    suspend fun inbox(limit: Int = 100): List<InboxItem> = api.call("v1/outbox", query = mapOf("limit" to "$limit")).list("items").mapNotNull { o ->
        val id = o.str("id") ?: return@mapNotNull null
        InboxItem(id, o.str("agent") ?: "", o.str("title"), o.str("text") ?: "", o.time("sent_at", "created_at"), o.str("session_id"),
            o.bool("has_audio") ?: false)
    }

    /** Approvals the agent waits for, whatever conversation asked (`GET /v1/approvals`). */
    suspend fun approvals(agent: String): List<Pair<RunEvent.Approval, String?>> =
        api.call("v1/approvals", query = mapOf("agent" to agent)).list("items").mapNotNull { o ->
            val run = o.str("run_id") ?: return@mapNotNull null
            RunEvent.Approval(run, o.str("request_id"), o.str("command"), o.str("description"),
                o["choices"].arr()?.mapNotNull { it.text()?.lowercase() }?.ifEmpty { null } ?: listOf("once", "deny")) to o.str("session_id")
        }

    /** A scheduled task's notifications on or off (`PUT /v1/cron-jobs/{agent}/{job}`). */
    suspend fun setCronNotify(agent: String, job: String, notify: Boolean) {
        api.call("v1/cron-jobs/${Hermes.seg(agent)}/${Hermes.seg(job)}", "PUT", kotlinx.serialization.json.buildJsonObject {
            put("notify", kotlinx.serialization.json.JsonPrimitive(notify))
        })
    }

    suspend fun outboxAudio(id: String): ByteArray = api.bytes("v1/outbox/${Hermes.seg(id)}/audio")

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
