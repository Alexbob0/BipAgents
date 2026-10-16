package io.github.bipagents.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.UUID

data class Session(
    val id: String,
    val title: String?,
    val updated: Long?,
    val preview: String?,
    val source: String?,
) {
    val isBotChat get() = title.equals("Bot Chat", ignoreCase = true)
    val isScheduled get() = id.startsWith("cron_") || source.equals("cron", ignoreCase = true)

    companion object {
        fun of(o: JsonObject): Session? {
            val s = o["session"].obj() ?: o
            val id = s.str("id", "session_id") ?: return null
            return Session(id, s.str("title", "name"),
                s.time("updated_at", "last_active", "last_activity", "last_message_at", "ended_at", "created_at", "started_at"),
                s.str("preview", "last_message_preview", "last_message", "snippet"), s.str("source", "platform"))
        }

        /** The agent's ongoing conversation: its Bot Chat, else the most recent one that isn't a scheduled task's. */
        fun main(sessions: List<Session>): Session? =
            sessions.firstOrNull { it.isBotChat } ?: sessions.sortedByDescending { it.updated ?: 0 }.firstOrNull { !it.isScheduled }
    }
}

data class Message(
    val id: String,
    val role: String,
    val text: String,
    val time: Long?,
    val hasImage: Boolean,
    val tools: List<String>,
)

/** One agent's Hermes api_server (HermesKit's HermesClient), with the agent's key. */
class Hermes(val api: Api) {
    suspend fun sessions(limit: Int = 50, offset: Int = 0): List<Session> =
        api.call("api/sessions", query = mapOf("limit" to "$limit", "offset" to "$offset")).list("sessions").mapNotNull(Session::of)

    /** Pages until `enough` or the end (scheduled tasks open a session per run and push conversations down the list). */
    suspend fun allSessions(maxPages: Int = 4, enough: (List<Session>) -> Boolean = { false }): List<Session> {
        val all = mutableListOf<Session>()
        var offset = 0
        repeat(maxPages) {
            val page = sessions(50, offset)
            offset += page.size
            all += page.filter { s -> all.none { it.id == s.id } }
            if (page.size < 50 || enough(all)) return all
        }
        return all
    }

    suspend fun createSession(title: String?): Session {
        val body = buildJsonObject { if (title != null) put("title", title) }
        return Session.of(api.call("api/sessions", "POST", body).obj() ?: JsonObject(emptyMap()))
            ?: throw HttpError(502, "Session payload has no id")
    }

    suspend fun messages(session: String): List<Message> =
        api.call("api/sessions/${seg(session)}/messages", query = mapOf("include_compacted" to "false", "inline_images" to "false"))
            .list("messages").mapIndexedNotNull { index, o -> message(o, index) }

    /** `POST /v1/runs`: the turn runs on the server whatever happens to the phone. Returns the run's id. */
    suspend fun startRun(session: String, input: JsonObject): String {
        val body = JsonObject(input + ("session_id" to JsonPrimitive(session)))
        val reply = api.call("v1/runs", "POST", body, headers = mapOf("Idempotency-Key" to UUID.randomUUID().toString())).obj()
        return reply.str("run_id", "id") ?: throw HttpError(502, "Run creation response has no run_id")
    }

    fun runEvents(run: String): Flow<RunEvent> = api.events("v1/runs/${seg(run)}/events").mapNotNull(RunEvent::of)

    suspend fun stop(run: String) { api.call("v1/runs/${seg(run)}/stop", "POST", JsonObject(emptyMap())) }

    suspend fun approve(run: String, choice: String, requestId: String?) {
        api.call("v1/runs/${seg(run)}/approval", "POST", buildJsonObject {
            put("choice", choice)
            if (requestId != null) put("request_id", requestId)
        })
    }

    /** BipAgents' batch contract, or Hermes' upstream one question per request (HermesClient.answer). */
    suspend fun answer(request: RunEvent.Clarify, answers: Map<String, String>?) {
        val path = "v1/runs/${seg(request.run)}/clarify"
        if (!request.perQuestion) {
            api.call(path, "POST", buildJsonObject {
                put("request_id", request.requestId)
                if (answers == null) put("cancel", true)
                else put("answers", JsonObject(answers.mapValues { JsonPrimitive(it.value) }))
            })
            return
        }
        val question = request.questions.firstOrNull()
        var response = answers?.get(question?.id ?: "") ?: answers?.values?.firstOrNull() ?: ""
        if (question?.multiSelect == true && response.isNotEmpty()) response = json.encodeToString(listOf(response))
        api.call(path, "POST", buildJsonObject { put("clarify_id", request.requestId); put("response", response) })
    }

    private fun message(o: JsonObject, index: Int): Message? {
        val role = o.str("role")?.lowercase() ?: return null
        var hasImage = false
        val content = o["content"]
        val text = when {
            content is JsonPrimitive -> content.content
            content.arr() != null -> content.arr()!!.mapNotNull { part ->
                val p = part.obj()
                when (p.str("type")) {
                    "image_url", "input_image", "image" -> { hasImage = true; null }
                    else -> p.str("text") ?: (part as? JsonPrimitive)?.content
                }
            }.joinToString("\n")
            else -> o.str("text") ?: ""
        }
        val tools = o["tool_calls"].arr()?.mapNotNull { call -> call.obj()?.let { it["function"].obj().str("name") ?: it.str("name") } } ?: emptyList()
        return Message(o.str("id", "message_id") ?: "$index", role, text, o.time("created_at", "timestamp", "ts"), hasImage, tools)
    }

    companion object {
        fun seg(s: String): String = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")
    }
}
