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

/** What the app may change on an agent this install created (or one set up by hand: personality and thinking only). */
data class AgentSettings(val soul: String, val toolsets: List<Toolset>, val isolated: Boolean, val protected: Boolean,
                         val services: List<ConnectedService>, val thinking: String?, val byHand: Boolean)
data class Toolset(val id: String, val label: String, val description: String, val enabled: Boolean, val configured: Boolean)
data class ConnectedService(val id: String, val url: String?, val catalog: String?)
data class CatalogService(val id: String, val label: String, val description: String, val icon: String, val personal: Boolean,
                          val fields: List<Field>) {
    data class Field(val id: String, val label: String, val placeholder: String, val secret: Boolean, val optional: Boolean)
}
/** The person's own voice server: its voices (« Voix 1, 2… ») and the uses it speaks for. */
data class VoiceServer(val voices: List<String>, val uses: Map<String, Boolean>) {
    fun takes(use: String) = uses[use] ?: true
    /** Ids named after a language (`fr-…`, `en_…`) only in theirs. */
    fun voices(language: String) = voices.filter { id ->
        val p = id.take(3).lowercase()
        if (p.length < 3 || p[2] !in "-_" || p.take(2) !in setOf("fr", "en", "es", "de")) true else p.startsWith(language)
    }
}
/** Refused: an agent can't hold this (`needs_own_space`, `needs_sandbox`, …). */
class CannotHold(val reasons: Map<String, String>) : Exception("cannot hold")

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

    /** Null when this bridge does not manage the agent (or an older bridge). */
    suspend fun agentSettings(agent: String): AgentSettings? = try {
        settings(api.call("v1/agents/${Hermes.seg(agent)}/settings").obj())
    } catch (e: HttpError) {
        if (e.status in setOf(404, 405, 409)) null else throw e
    }

    /** A new personality and/or tools on or off; Hermes uses them from the next message. */
    suspend fun updateAgentSettings(agent: String, soul: String? = null, toolsets: Map<String, Boolean>? = null): AgentSettings? =
        settings(api.call("v1/agents/${Hermes.seg(agent)}/settings", "PATCH", kotlinx.serialization.json.buildJsonObject {
            soul?.let { put("soul", kotlinx.serialization.json.JsonPrimitive(it)) }
            toolsets?.let { t -> put("toolsets", kotlinx.serialization.json.JsonObject(t.mapValues { kotlinx.serialization.json.JsonPrimitive(it.value) })) }
        }).obj())

    private fun settings(o: kotlinx.serialization.json.JsonObject?): AgentSettings? {
        // An empty personality is still one (an agent set up by hand may have none yet).
        val soul = (o?.get("soul") as? kotlinx.serialization.json.JsonPrimitive)?.takeIf { it.isString }?.content ?: return null
        return AgentSettings(soul,
            o?.get("toolsets").arr().orEmpty().mapNotNull { t -> t.obj()?.let { r -> r.str("name")?.let { id ->
                Toolset(id, r.str("label") ?: id, r.str("description") ?: "", r.bool("enabled") ?: false, r.bool("configured") ?: true) } } },
            o.bool("isolated") ?: false, o.bool("protected") ?: true,
            o?.get("services").arr().orEmpty().mapNotNull { t -> t.obj()?.let { r -> r.str("name")?.let { ConnectedService(it, r.str("url"), r.str("catalog")) } } },
            o.str("thinking"), o.bool("by_hand") ?: false)
    }

    suspend fun toolCatalog(): List<CatalogService> = api.call("v1/tool-catalog").list("services").mapNotNull { r ->
        val id = r.str("id") ?: return@mapNotNull null
        CatalogService(id, r.str("label") ?: id, r.str("description") ?: "", r.str("icon") ?: "", r.bool("personal") ?: false,
            r["fields"].arr().orEmpty().mapNotNull { f -> f.obj()?.let { x -> x.str("id")?.let { fid ->
                CatalogService.Field(fid, x.str("label") ?: fid, x.str("placeholder") ?: "", x.bool("secret") ?: false, x.bool("optional") ?: false) } } })
    }

    /** `values`: the catalog entry's fields (its token included). */
    suspend fun connectService(catalog: String, values: Map<String, String>, agent: String) {
        try {
            api.call("v1/agents/${Hermes.seg(agent)}/services", "POST", kotlinx.serialization.json.JsonObject(
                (values + ("catalog" to catalog)).mapValues { kotlinx.serialization.json.JsonPrimitive(it.value) }))
        } catch (e: HttpError) {
            if (e.status == 409) cannotHold(e)?.let { throw it }
            throw e
        }
    }

    private fun cannotHold(e: HttpError): CannotHold? {
        val agents = parseJson(e.raw ?: return null).obj()?.get("detail").obj()?.get("agents") as? kotlinx.serialization.json.JsonObject ?: return null
        return CannotHold(agents.mapValues { (it.value as? kotlinx.serialization.json.JsonPrimitive)?.content ?: "" })
    }

    suspend fun disconnectService(name: String, agent: String) { api.call("v1/agents/${Hermes.seg(agent)}/services/${Hermes.seg(name)}", "DELETE") }

    /** Whether the agent reaches the service, and how many tools it offers. */
    suspend fun testService(name: String, agent: String): Triple<Boolean, String?, Long?> {
        val o = api.call("v1/agents/${Hermes.seg(agent)}/services/${Hermes.seg(name)}/test", "POST", kotlinx.serialization.json.JsonObject(emptyMap())).obj()
        return Triple(o.bool("ok") ?: false, o.str("message"), o.long("tools"))
    }

    /** The agent moves out of the shared hub into its own space (about half a minute); its new address. */
    suspend fun moveToOwnSpace(agent: String): String? =
        api.call("v1/agents/${Hermes.seg(agent)}/space", "POST", kotlinx.serialization.json.JsonObject(emptyMap())).obj().str("url")

    /** The agent, its conversations, memory and routines, gone from the install. */
    suspend fun deleteAgent(agent: String) { api.call("v1/agents/${Hermes.seg(agent)}", "DELETE") }

    /** The person's own voice server, null when there is none (or an older bridge). */
    suspend fun voiceServer(): VoiceServer? = try {
        api.call("v1/voices").obj()?.get("server").obj()?.let { s ->
            VoiceServer(s["voices"].arr().orEmpty().mapNotNull { it.text() },
                (s["uses"] as? kotlinx.serialization.json.JsonObject)?.mapValues { (it.value as? kotlinx.serialization.json.JsonPrimitive)?.content == "true" } ?: emptyMap())
        }
    } catch (e: HttpError) {
        if (e.status in setOf(404, 405)) null else throw e
    }

    suspend fun setVoiceUses(uses: Map<String, Boolean>) {
        api.call("v1/voices/uses", "PUT", kotlinx.serialization.json.JsonObject(uses.mapValues { kotlinx.serialization.json.JsonPrimitive(it.value) }))
    }

    /** A short sentence in `voice` (`server:fr/<id>`, `pocket:en/galet`), as WAV. */
    suspend fun voicePreview(text: String, agent: String, voice: String): ByteArray = api.bytes("v1/tts/sentence", "POST", Http.body(
        kotlinx.serialization.json.buildJsonObject {
            put("text", kotlinx.serialization.json.JsonPrimitive(text)); put("agent", kotlinx.serialization.json.JsonPrimitive(agent))
            put("voice", kotlinx.serialization.json.JsonPrimitive(voice)); put("format", kotlinx.serialization.json.JsonPrimitive("wav"))
        }))

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
