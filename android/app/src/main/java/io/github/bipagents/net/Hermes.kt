package io.github.bipagents.net

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.util.UUID

@kotlinx.serialization.Serializable
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

@kotlinx.serialization.Serializable
data class Message(
    val id: String,
    val role: String,
    val text: String,
    val time: Long?,
    val hasImage: Boolean,
    val tools: List<ToolStep>,
    val reasoning: String? = null,
)

/** A tool call (HermesKit's ToolEvent): from a run's `tool.*` events or the stored history. */
@kotlinx.serialization.Serializable
data class ToolStep(
    val name: String,
    /** Arguments (started) or a cut result (done). */
    val preview: String? = null,
    val status: Status = Status.DONE,
    /** Seconds. */
    val duration: Double? = null,
    val error: String? = null,
    val callId: String? = null,
) {
    enum class Status { STARTED, DONE, FAILED }
}

data class RunStatus(val status: String, val approval: RunEvent.Approval?) {
    val terminal get() = status in setOf("completed", "failed", "cancelled", "canceled", "interrupted", "expired", "succeeded")

    companion object {
        fun of(o: JsonObject?, run: String): RunStatus {
            val s = (o?.get("run") as? JsonObject) ?: o
            val status = s.str("status")?.lowercase() ?: "unknown"
            // Hermes keeps the approval.request in a parked run's status: a client that missed the event can still answer.
            val approval = if (status == "waiting_for_approval") (s?.get("approval") as? JsonObject)?.let { a ->
                RunEvent.of(ServerEvent("approval.request", JsonObject(a + ("run_id" to JsonPrimitive(run))))) as? RunEvent.Approval
            } else null
            return RunStatus(status, approval)
        }
    }
}

/** A Hermes scheduled task (`/api/jobs`), shown as a routine: what the agent does, and when. */
data class Routine(val id: String, val name: String, val prompt: String, val schedule: String, val state: String, val enabled: Boolean,
                   val nextRun: Long?, val lastStatus: String?) {
    val paused get() = state == "paused" || !enabled

    companion object {
        fun of(o: JsonObject): Routine? {
            val id = o.str("id") ?: return null
            val s = o["schedule"]
            val schedule = s.obj().str("expr", "run_at") ?: s.obj().long("minutes")?.let { "every ${it}m" } ?: s.text()
                ?: o.str("schedule_display") ?: ""
            return Routine(id, o.str("name") ?: id, o.str("prompt") ?: "", schedule, o.str("state") ?: "scheduled",
                o.bool("enabled") ?: true, o.time("next_run_at"), o.str("last_status"))
        }
    }
}

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

    suspend fun session(id: String): Session? = Session.of(api.call("api/sessions/${seg(id)}").obj() ?: JsonObject(emptyMap()))

    suspend fun renameSession(id: String, title: String) {
        api.call("api/sessions/${seg(id)}", "PATCH", buildJsonObject { put("title", title) })
    }

    suspend fun deleteSession(id: String) { api.call("api/sessions/${seg(id)}", "DELETE") }

    // -- routines (Hermes' scheduled tasks, `/api/jobs`) --------------------------------------------------------------

    suspend fun routines(): List<Routine> =
        api.call("api/jobs", query = mapOf("include_disabled" to "true")).list("jobs").mapNotNull(Routine::of)

    /** Delivered locally: the bridge puts its report in the Inbox and notifies. */
    suspend fun createRoutine(name: String, schedule: String, prompt: String) {
        api.call("api/jobs", "POST", buildJsonObject { put("name", name); put("schedule", schedule); put("prompt", prompt); put("deliver", "local") })
    }

    suspend fun updateRoutine(id: String, name: String, schedule: String, prompt: String) {
        api.call("api/jobs/${seg(id)}", "PATCH", buildJsonObject { put("name", name); put("schedule", schedule); put("prompt", prompt) })
    }

    suspend fun deleteRoutine(id: String) { api.call("api/jobs/${seg(id)}", "DELETE") }
    suspend fun pauseRoutine(id: String, paused: Boolean) { api.call("api/jobs/${seg(id)}/${if (paused) "pause" else "resume"}", "POST", JsonObject(emptyMap())) }
    /** Now, once, besides its schedule. */
    suspend fun runRoutine(id: String) { api.call("api/jobs/${seg(id)}/run", "POST", JsonObject(emptyMap())) }

    suspend fun createSession(title: String?): Session {
        val body = buildJsonObject { if (title != null) put("title", title) }
        return Session.of(api.call("api/sessions", "POST", body).obj() ?: JsonObject(emptyMap()))
            ?: throw HttpError(502, "Session payload has no id")
    }

    /** `latest`: only that many of the latest messages. */
    suspend fun messages(session: String, latest: Int? = null): List<Message> =
        api.call("api/sessions/${seg(session)}/messages", query = mapOf("include_compacted" to "false", "inline_images" to "false",
            "limit" to latest?.toString(), "order" to latest?.let { "latest" }))
            .list("messages").mapIndexedNotNull { index, o -> message(o, index) }

    /** `POST /v1/runs`: the turn runs on the server whatever happens to the phone. Returns the run's id. */
    suspend fun startRun(session: String, input: JsonObject): String {
        val body = JsonObject(input + ("session_id" to JsonPrimitive(session)))
        val reply = api.call("v1/runs", "POST", body, headers = mapOf("Idempotency-Key" to UUID.randomUUID().toString())).obj()
        return reply.str("run_id", "id") ?: throw HttpError(502, "Run creation response has no run_id")
    }

    /** One turn streamed on the connection (`chat/stream`): the Live's way, with each event's run id. Thinking off: a
     *  spoken answer right away, whatever the agent's level in written conversations. */
    fun chatStream(session: String, text: String): Flow<ServerEvent> = api.events("api/sessions/${seg(session)}/chat/stream",
        body = buildJsonObject {
            put("input", text)
            put("model_options", buildJsonObject { put("reasoning", buildJsonObject { put("enabled", false) }) })
        })

    fun runEvents(run: String): Flow<RunEvent> = api.events("v1/runs/${seg(run)}/events").mapNotNull(RunEvent::of)

    /** A run's status (`GET /v1/runs/{id}`): `status`, and the approval it waits for. */
    suspend fun run(run: String): RunStatus = RunStatus.of(api.call("v1/runs/${seg(run)}").obj(), run)

    /** One turn streamed on the connection (`chat/stream`): for a server that rejects runs. */
    fun chatTurn(session: String, input: JsonObject): Flow<RunEvent> =
        api.events("api/sessions/${seg(session)}/chat/stream", body = input).mapNotNull(RunEvent::of)

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
        val tools = o["tool_calls"].arr()?.mapNotNull { call ->
            val c = call.obj() ?: return@mapNotNull null
            val f = c["function"].obj()
            ToolStep(f.str("name") ?: c.str("name") ?: "tool", f.str("arguments") ?: c.str("arguments"), callId = c.str("id"))
        }.orEmpty().toMutableList()
        if (role == "tool") o.str("tool_name", "name")?.let { tools += ToolStep(it, text.ifEmpty { null }, callId = o.str("tool_call_id")) }
        return Message(o.str("id", "message_id") ?: "$index", role, text, o.time("created_at", "timestamp", "ts"), hasImage, tools,
            o.str("reasoning", "reasoning_content", "thinking")?.ifEmpty { null })
    }

    companion object {
        fun seg(s: String): String = java.net.URLEncoder.encode(s, "UTF-8").replace("+", "%20")
    }
}
