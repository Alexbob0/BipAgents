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
data class LinkAgent(val id: String, val name: String, val managed: Boolean, val household: String?)
data class AgentLink(val from: String, val to: String)
data class ShareAgent(val id: String, val name: String, val whyNot: String?, val external: Boolean)
data class ShareRead(val reader: String, val source: String)
data class Family(val name: String, val url: String?, val households: List<Pair<String, String>>)
data class CronJob(val agent: String, val job: String, val name: String?, val notify: Boolean, val lastSeen: Long?)
data class ConsoleDevice(val id: String, val name: String, val seen: Long?)
data class VaultItem(val id: String, val label: String, val origin: String, val identifier: String, val hasOtp: Boolean, val agents: List<String>)
data class VaultAgent(val id: String, val name: String, val canHold: Boolean, val why: String?)
data class Vault(val items: List<VaultItem>, val agents: List<VaultAgent>, val protected: Boolean)

/** A profile created on the server (`hermes profile create`…) that no app reaches yet. */
data class UnclaimedProfile(val id: String, val name: String, val description: String)
/** A kind of agent the bridge can create (`custom`: « Other », described by the person). */
data class AgentKind(val id: String, val label: String, val description: String, val custom: Boolean)

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

    /** `GET /v1/agents`, all of it: the agents the app can reach, every agent the bridge serves, profiles waiting. */
    suspend fun listing(): Triple<List<ListedAgent>, Set<String>, List<UnclaimedProfile>> {
        val o = api.call("v1/agents").obj()
        val rows = o?.get("agents").list()
        val agents = rows.mapNotNull { r ->
            val id = r.str("id") ?: return@mapNotNull null
            ListedAgent(id, r.str("name") ?: id.replaceFirstChar(Char::uppercase), r.str("url"), r.str("key"), r.str("category"), r.str("color"),
                r.str("voice"), r.str("server_voice"))
        }
        val unclaimed = o?.get("unclaimed").list().mapNotNull { r ->
            r.str("id")?.let { UnclaimedProfile(it, r.str("name") ?: it.replaceFirstChar(Char::uppercase), r.str("description") ?: "") }
        }
        return Triple(agents, rows.mapNotNull { it.str("id") }.toSet(), unclaimed)
    }

    /** A profile found on the server, served to the app (its key; nothing else changes). */
    suspend fun claimProfile(id: String, name: String): ListedAgent {
        val o = api.call("v1/agents/claim", "POST", kotlinx.serialization.json.buildJsonObject {
            put("id", kotlinx.serialization.json.JsonPrimitive(id)); put("name", kotlinx.serialization.json.JsonPrimitive(name))
        }).obj()
        return ListedAgent(o.str("id") ?: throw HttpError(502, "claim"), o.str("name") ?: name, o.str("url"), o.str("key"))
    }

    /** The kinds this bridge can create, in `lang`; null when it can't create agents (set up by hand, or older). */
    suspend fun agentKinds(lang: String): List<AgentKind>? = try {
        val o = api.call("v1/agent-kinds", query = mapOf("lang" to lang)).obj()
        if (o.bool("can_create") != true) null else o?.get("kinds").list().mapNotNull { r ->
            r.str("id")?.let { AgentKind(it, r.str("label") ?: it, r.str("description") ?: "", r.bool("custom") ?: false) }
        }
    } catch (e: HttpError) {
        if (e.status in setOf(404, 405)) null else throw e
    }

    /** A new agent on the server: a kind, or « Other » described by the person (role, tone, rules, language, colour). */
    suspend fun createAgent(name: String, kind: String?, custom: Map<String, String>?, lang: String): ListedAgent {
        val o = api.call("v1/agents", "POST", kotlinx.serialization.json.buildJsonObject {
            put("name", kotlinx.serialization.json.JsonPrimitive(name)); put("lang", kotlinx.serialization.json.JsonPrimitive(lang))
            kind?.let { put("kind", kotlinx.serialization.json.JsonPrimitive(it)) }
            custom?.let { c -> put("custom", kotlinx.serialization.json.JsonObject(c.mapValues { kotlinx.serialization.json.JsonPrimitive(it.value) })) }
        }).obj()
        return ListedAgent(o.str("id") ?: throw HttpError(502, "creation"), o.str("name") ?: name, o.str("url"), o.str("key"))
    }

    /** Whether the agents' model answers (null when the bridge doesn't check it). */
    suspend fun modelUp(): Boolean? = runCatching { api.call("v1/status").obj()?.get("model").obj().bool("ok") }.getOrNull()

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

    // -- between agents ---------------------------------------------------------------------------------------------

    /** Who may write to whom; null on an older bridge. */
    suspend fun agentLinks(): Pair<List<LinkAgent>, Set<AgentLink>>? = orNull404 {
        val o = api.call("v1/agent-links").obj()
        o?.get("agents").list().mapNotNull { r -> r.str("id")?.let { LinkAgent(it, r.str("name") ?: it, r.bool("managed") ?: false, r.str("household")) } } to links(o)
    }

    /** Every allowed link; returns the agents set up by hand that still need their token. */
    suspend fun setAgentLinks(links: Set<AgentLink>): List<String> =
        api.call("v1/agent-links", "PUT", obj("links" to kotlinx.serialization.json.JsonArray(links.map { obj("from" to p(it.from), "to" to p(it.to)) })))
            .obj()?.get("needs_setup").arr()?.mapNotNull { it.text() } ?: emptyList()

    private fun links(o: kotlinx.serialization.json.JsonObject?) =
        o?.get("links").list().mapNotNull { r -> r.str("from")?.let { f -> r.str("to")?.let { AgentLink(f, it) } } }.toSet()

    /** Whose shared folder each agent may read; null on an older bridge. */
    suspend fun agentShares(): Pair<List<ShareAgent>, Set<ShareRead>>? = orNull404 {
        val o = api.call("v1/agent-shares").obj()
        o?.get("agents").list().mapNotNull { r -> r.str("id")?.let { ShareAgent(it, r.str("name") ?: it, r.str("why_not"), r.bool("external") ?: false) } } to reads(o)
    }

    suspend fun setAgentShares(reads: Set<ShareRead>): Set<ShareRead> =
        reads(api.call("v1/agent-shares", "PUT", obj("reads" to kotlinx.serialization.json.JsonArray(reads.map { obj("reader" to p(it.reader), "source" to p(it.source)) }))).obj())

    private fun reads(o: kotlinx.serialization.json.JsonObject?) =
        o?.get("reads").list().mapNotNull { r -> r.str("reader")?.let { a -> r.str("source")?.let { ShareRead(a, it) } } }.toSet()

    /** This install as a household, and those linked to it; null on an older bridge. */
    suspend fun family(): Family? = orNull404 {
        val o = api.call("v1/family").obj()
        Family(o.str("name") ?: "", o.str("url"), o?.get("households").list().mapNotNull { r -> r.str("id")?.let { it to (r.str("name") ?: it) } })
    }

    suspend fun setFamilyName(name: String) { api.call("v1/family/name", "PUT", obj("name" to p(name))) }

    /** The text of the QR code another household scans (valid ten minutes, once). */
    suspend fun familyInvitation(): String = api.bytes("v1/family/invite", "POST", Http.body(obj())).decodeToString()

    /** Another household's invitation, scanned here; its name. */
    suspend fun joinFamily(url: String, code: String): String? =
        api.call("v1/family/join", "POST", obj("url" to p(url), "code" to p(code))).obj().str("name")

    suspend fun unlinkHousehold(id: String) { api.call("v1/family/${Hermes.seg(id)}", "DELETE") }

    // -- scheduled tasks, browsers, vault -----------------------------------------------------------------------------

    suspend fun cronJobs(agent: String): List<CronJob> = api.call("v1/cron-jobs", query = mapOf("agent" to agent)).list("jobs").mapNotNull { r ->
        val a = r.str("agent") ?: return@mapNotNull null
        val j = r.str("job") ?: return@mapNotNull null
        CronJob(a, j, r.str("name"), r.bool("notify") ?: true, r.time("last_seen"))
    }

    /** False when no console of this bridge waits with that code. */
    suspend fun approveConsole(code: String, name: String): Boolean = try {
        api.call("v1/console/pairings/${Hermes.seg(code)}", "POST", obj("name" to p(name))); true
    } catch (e: HttpError) {
        if (e.status == 404) false else throw e
    }

    /** The browsers logged in to the console; null on a bridge without it. */
    suspend fun consoleDevices(): List<ConsoleDevice>? = orNull404 {
        api.call("v1/console/devices").list("devices").mapNotNull { r -> r.str("id")?.let { ConsoleDevice(it, r.str("name") ?: it, r.time("seen")) } }
    }

    suspend fun revokeConsole(id: String) { api.call("v1/console/devices/${Hermes.seg(id)}", "DELETE") }

    /** The agents' vault; null when this bridge has none. Passwords never come back. */
    suspend fun vault(): Vault? = orNull404 {
        val o = api.call("v1/vault").obj()
        Vault(o?.get("items").list().mapNotNull(::vaultItem),
            o?.get("agents").list().mapNotNull { r -> r.str("id")?.let { VaultAgent(it, r.str("name") ?: it, r.bool("can_hold") ?: false, r.str("why")) } },
            o.bool("protected") ?: true)
    }

    private fun vaultItem(r: kotlinx.serialization.json.JsonObject): VaultItem? = r.str("id")?.let {
        VaultItem(it, r.str("label") ?: "", r.str("origin") ?: "", r.str("identifier") ?: "", r.bool("has_otp") ?: false,
            r["agents"].arr()?.mapNotNull { a -> a.text() } ?: emptyList())
    }

    /** New (`id` null) or changed; the password only when given (write-only). */
    suspend fun saveVaultItem(id: String?, label: String, site: String, identifier: String, password: String?, otp: String?, agents: List<String>) {
        val type = when { "@" in identifier -> "email"; identifier.all { it.isDigit() || it in "+ " } -> "phone"; else -> "username" }
        val body = obj("kind" to p("login"), "label" to p(label), "site" to p(site), "identifier" to p(identifier), "identifier_type" to p(type),
            "agents" to kotlinx.serialization.json.JsonArray(agents.map(::p)))
            .let { b -> kotlinx.serialization.json.JsonObject(b + listOfNotNull(password?.takeIf { it.isNotEmpty() }?.let { "password" to p(it) },
                otp?.let { "otp_secret" to p(it) })) }
        try {
            api.call(if (id == null) "v1/vault" else "v1/vault/${Hermes.seg(id)}", if (id == null) "POST" else "PATCH", body)
        } catch (e: HttpError) {
            if (e.status == 409) cannotHold(e)?.let { throw it }
            throw e
        }
    }

    suspend fun deleteVaultItem(id: String) { api.call("v1/vault/${Hermes.seg(id)}", "DELETE") }

    private suspend fun <T> orNull404(block: suspend () -> T): T? = try { block() } catch (e: HttpError) { if (e.status == 404) null else throw e }
    private fun p(value: String) = kotlinx.serialization.json.JsonPrimitive(value)
    private fun obj(vararg pairs: Pair<String, kotlinx.serialization.json.JsonElement>) = kotlinx.serialization.json.JsonObject(pairs.toMap())

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
