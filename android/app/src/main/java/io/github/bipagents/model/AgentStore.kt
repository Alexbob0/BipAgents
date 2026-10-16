package io.github.bipagents.model

import android.content.Context
import io.github.bipagents.net.Api
import io.github.bipagents.net.Bridge
import io.github.bipagents.net.Hermes
import io.github.bipagents.net.HttpError
import io.github.bipagents.net.Session
import io.github.bipagents.net.json
import io.github.bipagents.net.obj
import io.github.bipagents.net.str
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.serialization.builtins.ListSerializer

/** The person's agents (App/Model/AgentStore.swift): their settings in the app's preferences, their keys in [Secrets]. */
class AgentStore(context: Context) {
    private val prefs = context.getSharedPreferences("agents", Context.MODE_PRIVATE)
    private val secrets = Secrets(context)
    private val serializer = ListSerializer(Agent.serializer())

    private val _agents = MutableStateFlow(load())
    val agents: StateFlow<List<Agent>> = _agents.asStateFlow()

    /** The latest conversation of each agent, for its home card (kept: the cards and threads open offline too). */
    private val latestSerializer = kotlinx.serialization.builtins.MapSerializer(kotlinx.serialization.serializer<String>(), Session.serializer())
    private val _latest = MutableStateFlow<Map<String, Session>>(
        prefs.getString("latest", null)?.let { runCatching { json.decodeFromString(latestSerializer, it) }.getOrNull() } ?: emptyMap())
    val latest: StateFlow<Map<String, Session>> = _latest.asStateFlow()

    /** How each agent answered lately (absent: not asked yet). */
    enum class Reach { ONLINE, LOCAL, MODEL_DOWN, UNAUTHORIZED, OFFLINE }
    private val _reach = MutableStateFlow<Map<String, Reach>>(emptyMap())
    val reach: StateFlow<Map<String, Reach>> = _reach.asStateFlow()
    /** Agents whose server doesn't answer at all (the home screen says what to do). */
    private val _lost = MutableStateFlow<Set<String>>(emptySet())
    val lost: StateFlow<Set<String>> = _lost.asStateFlow()

    /** Something a server has that this phone doesn't, or no longer: added, connected or removed in one tap. */
    sealed interface News {
        val bridgeUrl: String
        val bridgeKey: String
        val id: String
        data class Added(override val bridgeUrl: String, override val bridgeKey: String, val agent: io.github.bipagents.net.ListedAgent) : News {
            override val id get() = "$bridgeUrl|added|${agent.id}"
        }
        data class Unclaimed(override val bridgeUrl: String, override val bridgeKey: String, val profile: io.github.bipagents.net.UnclaimedProfile) : News {
            override val id get() = "$bridgeUrl|unclaimed|${profile.id}"
        }
        data class Gone(override val bridgeUrl: String, override val bridgeKey: String, val agent: Agent) : News {
            override val id get() = "$bridgeUrl|gone|${agent.id}"
        }
    }
    private val _news = MutableStateFlow<List<News>>(emptyList())
    val news: StateFlow<List<News>> = _news.asStateFlow()

    private fun load(): List<Agent> =
        prefs.getString("list", null)?.let { runCatching { json.decodeFromString(serializer, it) }.getOrNull() } ?: emptyList()

    private fun save(list: List<Agent>) {
        prefs.edit().putString("list", json.encodeToString(serializer, list)).apply()
        _agents.value = list
        pinDoors()
    }

    private fun pinDoors() = io.github.bipagents.net.LocalLink.updatePins(_agents.value.mapNotNull { a ->
        a.lanUrl?.let { u -> a.lanFingerprint?.let { u to it } } })

    init { pinDoors() }

    fun agent(id: String): Agent? = _agents.value.firstOrNull { it.id == id }

    /** Its Hermes right now: on the tailnet, or through its bridge's door on the local network. */
    fun hermes(agent: Agent): Hermes? = secrets.keys(agent.id)?.let { keys ->
        val door = agent.lanUrl?.takeIf { io.github.bipagents.net.LocalLink.isActive(agent.id) }
        if (door != null) Hermes(Api(door.trimEnd('/') + "/hermes/" + agent.bridgeName, keys.apiKey, local = true))
        else Hermes(Api(agent.baseUrl, keys.apiKey))
    }

    fun bridge(agent: Agent): Bridge? {
        val key = secrets.keys(agent.id)?.bridgeKey ?: return null
        agent.lanUrl?.takeIf { io.github.bipagents.net.LocalLink.isActive(agent.id) }?.let { return Bridge(Api(it, key, local = true)) }
        val url = agent.bridgeUrl ?: return null
        return Bridge(Api(url, key))
    }

    /** The bridge on the tailnet, whatever the door. */
    private fun tailnetBridge(agent: Agent): Bridge? {
        val key = secrets.keys(agent.id)?.bridgeKey ?: return null
        return Bridge(Api(agent.bridgeUrl ?: return null, key))
    }

    /** Each bridge once (several agents share one). */
    fun bridges(): List<Pair<Bridge, List<Agent>>> =
        _agents.value.filter { bridge(it) != null }.groupBy { it.bridgeUrl!! }.map { (_, agents) -> bridge(agents.first())!! to agents }

    fun add(agent: Agent, keys: AgentKeys) {
        val existing = _agents.value.firstOrNull { same(it, agent) }
        val kept = existing?.let { agent.copy(id = it.id, category = agent.category ?: it.category, color = agent.color ?: it.color) } ?: agent
        secrets.setKeys(kept.id, keys)
        save(_agents.value.filterNot { it.id == kept.id } + kept)
    }

    fun update(agent: Agent) = save(_agents.value.map { if (it.id == agent.id) agent else it })

    /** The agent moved to its own space: its new address (through the bridge). */
    fun relocate(agent: Agent, url: String) = save(_agents.value.map { if (it.id == agent.id) it.copy(baseUrl = url) else it })

    fun remove(agent: Agent) {
        secrets.setKeys(agent.id, null)
        save(_agents.value.filterNot { it.id == agent.id })
    }

    fun move(from: Int, to: Int) {
        val list = _agents.value.toMutableList()
        list.add(to, list.removeAt(from))
        save(list)
    }

    /** One scan adds a whole install: its bridge lists the agents, each with its address and key. */
    suspend fun pair(pairing: Pairing): Int = when (pairing) {
        is Pairing.Single -> { add(pairing.agent, pairing.keys); 1 }
        is Pairing.Install -> {
            val listed = Bridge(Api(pairing.bridgeUrl, pairing.bridgeKey)).agents()
                .filter { it.url != null && it.key != null && Pairing.acceptable(it.url) }
            if (listed.isEmpty()) throw HttpError(404, null)
            for (a in listed) {
                add(Agent(name = a.name, baseUrl = a.url!!, bridgeUrl = pairing.bridgeUrl, bridgeAgent = a.id.lowercase(),
                    category = a.category, color = a.color, lanUrl = pairing.lanUrl, lanFingerprint = pairing.lanFingerprint),
                    AgentKeys(a.key!!, pairing.bridgeKey))
            }
            listed.size
        }
    }

    /** Looks changed here that the bridge hasn't taken yet (offline): sent again, never overwritten by its own. */
    private fun pending(): Set<String> = prefs.getStringSet("looks-pending", emptySet()) ?: emptySet()
    private fun setPending(id: String, on: Boolean) =
        prefs.edit().putStringSet("looks-pending", if (on) pending() + id else pending() - id).apply()

    /** The Bip voice the bridge reads with (`pocket:en/galet` in English…), as the iPhone sends it. */
    fun spoken(voice: String, language: String?): String {
        val lang = language ?: java.util.Locale.getDefault().language
        return if (lang == "fr" || !voice.startsWith("pocket:")) voice else "pocket:$lang/" + voice.removePrefix("pocket:").substringAfter('/')
    }

    /** The agent's Bip voice, picked here: kept on this phone and by its bridge (its voice-model voice untouched). */
    suspend fun setVoice(agent: Agent, voice: String) = setVoices(agent, voice, agent.serverVoice)

    /** Its Bip (also the relay of a voice-model voice) and its voice on the person's voice server (null: none). */
    suspend fun setVoices(agent: Agent, voice: String, serverVoice: String?) {
        val changed = agent.copy(voice = voice, serverVoice = serverVoice)
        update(changed)
        runCatching { bridge(changed)?.setVoices(changed.bridgeName, spoken(voice, changed.language), serverVoice) }
    }

    /** The language the agent is spoken to in (speech recognition, voices) and writes first in. */
    suspend fun setLanguage(agent: Agent, lang: String) {
        val changed = agent.copy(language = lang)
        update(changed)
        runCatching { bridge(changed)?.setLanguage(changed.bridgeName, lang) }
        changed.voice?.let { v -> runCatching { bridge(changed)?.setVoices(changed.bridgeName, spoken(v, lang), changed.serverVoice) } }
    }

    /** The person picks the agent's look here: kept on this phone and by its bridge, for every app. */
    suspend fun setLook(agent: Agent, category: Category, color: String?) {
        val changed = agent.copy(category = category.id, color = color)
        update(changed)
        setPending(agent.id, true)
        if (runCatching { bridge(changed)?.setAppearance(changed.bridgeName, category.id, color) }.isSuccess && bridge(changed) != null) {
            setPending(agent.id, false)
        }
    }

    /** The looks kept by each bridge (chosen on the iPhone, or here before): the same Bips everywhere. The last one
     *  changed wins; a change made here and not sent yet goes first. */
    suspend fun syncLooks() {
        for ((bridge, mine) in bridges()) {
            val listed = runCatching { bridge.agents() }.getOrNull() ?: continue
            for (agent in mine) {
                if (agent.id in pending()) {
                    val category = agent.category ?: agent.look.category.id
                    if (runCatching { bridge.setAppearance(agent.bridgeName, category, agent.color) }.isSuccess) setPending(agent.id, false)
                    continue
                }
                val found = listed.firstOrNull { it.id.equals(agent.bridgeName, true) } ?: continue
                // Its voices as the bridge has them (picked on another phone): `pocket:en/galet` is the Bip `pocket:galet`.
                val bip = found.voice?.takeIf { it.startsWith("pocket:") }?.let { "pocket:" + it.removePrefix("pocket:").substringAfter('/') }
                var synced = agent.copy(voice = bip ?: agent.voice, serverVoice = found.serverVoice)
                if (found.category != null) synced = synced.copy(category = found.category, color = found.color)
                if (synced != agent) update(synced)
            }
        }
    }

    /** The home cards' latest conversation and « online » dot. */
    suspend fun refresh(agent: Agent) {
        fun set(r: Reach) = _reach.update { it + (agent.id to r) }
        val keys = secrets.keys(agent.id) ?: return
        try {
            // The tailnet first, always: the door is only for when it doesn't answer.
            val hermes = Hermes(Api(agent.baseUrl, keys.apiKey))
            val sessions = hermes.allSessions(maxPages = 3) { all -> all.any { it.isBotChat } }
            io.github.bipagents.net.LocalLink.setActive(agent.id, false)
            Session.main(sessions)?.let { s ->
                // Hermes' list preview is the session's *first* message: the card shows the latest one instead.
                val last = runCatching { hermes.messages(s.id, latest = 4) }.getOrNull()
                    ?.lastOrNull { (it.role == "assistant" || it.role == "user") && it.text.isNotBlank() }?.text
                setLatest(agent, s.copy(preview = last ?: s.preview ?: _latest.value[agent.id]?.takeIf { it.id == s.id }?.preview))
            }
            set(Reach.ONLINE)
            _lost.update { it - agent.id }
            refreshDoor(agent)
            // The agents' server answers; does the model they use (its machine off, an expired key)?
            if (bridge(agent)?.modelUp() == false) set(Reach.MODEL_DOWN)
        } catch (error: HttpError) {
            if (error.status == 401 || error.status == 403) return set(Reach.UNAUTHORIZED)
            // Moved on the server (into its own space, from another phone): its bridge knows the new address.
            val moved = runCatching { tailnetBridge(agent)?.agents() }.getOrNull()
                ?.firstOrNull { it.id == agent.bridgeName && it.url != null && it.url != agent.baseUrl }
            if (moved != null) {
                relocate(agent, moved.url!!)
                agent(agent.id)?.let { refresh(it) }
                return
            }
            // Tailscale doesn't answer: the bridge's door on the local network, if it has one.
            val door = agent.lanUrl
            if (door != null) {
                val local = Hermes(Api(door.trimEnd('/') + "/hermes/" + agent.bridgeName, keys.apiKey, local = true))
                val reached = runCatching { local.allSessions(maxPages = 1) }.getOrNull()
                if (reached != null) {
                    io.github.bipagents.net.LocalLink.setActive(agent.id, true)
                    Session.main(reached)?.let { s -> setLatest(agent, s) }
                    set(Reach.LOCAL)
                    _lost.update { it - agent.id }
                    return
                }
                io.github.bipagents.net.LocalLink.setActive(agent.id, false)
            }
            set(Reach.OFFLINE)
            if (error.status == 0) _lost.update { it + agent.id }
        }
    }

    /** The door as the bridge sees it now (it may have moved, or been turned off). */
    private suspend fun refreshDoor(agent: Agent) {
        val bridge = tailnetBridge(agent) ?: return
        val o = runCatching { bridge.api.call("v1/pairing").obj() }.getOrNull() ?: return
        val lan = o["lan"].obj()
        val fingerprint = lan.str("fingerprint")?.lowercase()?.takeIf { it.length == 64 }
        val url = lan.str("url")?.takeIf { fingerprint != null }
        if (url == agent.lanUrl && fingerprint == agent.lanFingerprint) return
        val current = agent(agent.id) ?: return
        save(_agents.value.map { if (it.id == current.id) it.copy(lanUrl = url, lanFingerprint = fingerprint) else it })
    }

    /** Each install this phone knows, asked what it serves now. */
    suspend fun refreshNews() {
        val dismissed = prefs.getStringSet("dismissed-news", emptySet()) ?: emptySet()
        val found = mutableListOf<News>()
        for ((bridge, here) in bridges()) {
            val url = here.first().bridgeUrl ?: continue
            val key = secrets.keys(here.first().id)?.bridgeKey ?: continue
            val (listed, served, unclaimed) = runCatching { bridge.listing() }.getOrNull() ?: continue
            for (a in listed) if (a.url != null && a.key != null && here.none { it.bridgeName == a.id.lowercase() }) found += News.Added(url, key, a)
            for (p in unclaimed) found += News.Unclaimed(url, key, p)
            for (mine in here) if (mine.bridgeName !in served && mine.bridgeName.lowercase() !in served) found += News.Gone(url, key, mine)
        }
        _news.value = found.filter { it.id !in dismissed }
    }

    /** The agent added to this phone (a profile on the server is connected first, up to a minute), or removed. */
    suspend fun accept(news: News, name: String? = null) {
        when (news) {
            is News.Added -> addListed(news.agent, news.bridgeUrl, news.bridgeKey)
            is News.Unclaimed -> addListed(Bridge(Api(news.bridgeUrl, news.bridgeKey)).claimProfile(news.profile.id, name ?: news.profile.name),
                news.bridgeUrl, news.bridgeKey)
            is News.Gone -> remove(news.agent)
        }
        _news.update { list -> list.filterNot { it.id == news.id } }
    }

    /** Not now, and not again for this one. */
    fun dismiss(news: News) {
        prefs.edit().putStringSet("dismissed-news", (prefs.getStringSet("dismissed-news", emptySet()) ?: emptySet()) + news.id).apply()
        _news.update { list -> list.filterNot { it.id == news.id } }
    }

    /** An agent its bridge lists (or just created), with its keys. */
    fun addListed(a: io.github.bipagents.net.ListedAgent, bridgeUrl: String, bridgeKey: String, category: String? = null, language: String? = null): Agent {
        val agent = Agent(name = a.name, baseUrl = a.url ?: throw HttpError(502, "no address"), bridgeUrl = bridgeUrl,
            bridgeAgent = a.id.lowercase(), category = category ?: a.category, color = a.color, language = language)
        add(agent, AgentKeys(a.key ?: throw HttpError(502, "no key"), bridgeKey))
        return agent
    }

    /** One of this phone's installs that can create agents, with the kinds it offers. */
    data class Creatable(val bridgeUrl: String, val bridgeKey: String, val kinds: List<io.github.bipagents.net.AgentKind>)

    suspend fun creatable(lang: String): Creatable? {
        for ((bridge, here) in bridges()) {
            val kinds = runCatching { bridge.agentKinds(lang) }.getOrNull() ?: continue
            val key = secrets.keys(here.first().id)?.bridgeKey ?: continue
            return Creatable(here.first().bridgeUrl!!, key, kinds)
        }
        return null
    }

    fun noteSession(agent: Agent, session: Session) = setLatest(agent, session)

    private fun setLatest(agent: Agent, session: Session) {
        _latest.update { it + (agent.id to session) }
        prefs.edit().putString("latest", json.encodeToString(latestSerializer, _latest.value)).apply()
    }

    private fun same(a: Agent, b: Agent) =
        (a.bridgeAgent != null && a.bridgeAgent == b.bridgeAgent && a.bridgeUrl == b.bridgeUrl) || a.baseUrl == b.baseUrl
}
