package io.github.bipagents.model

import android.content.Context
import io.github.bipagents.net.Api
import io.github.bipagents.net.Bridge
import io.github.bipagents.net.Hermes
import io.github.bipagents.net.HttpError
import io.github.bipagents.net.Session
import io.github.bipagents.net.json
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

    /** The latest conversation of each agent, for its home card. */
    private val _latest = MutableStateFlow<Map<String, Session>>(emptyMap())
    val latest: StateFlow<Map<String, Session>> = _latest.asStateFlow()

    /** Whether each agent's Hermes answered lately (null: not asked yet). */
    private val _online = MutableStateFlow<Map<String, Boolean>>(emptyMap())
    val online: StateFlow<Map<String, Boolean>> = _online.asStateFlow()

    private fun load(): List<Agent> =
        prefs.getString("list", null)?.let { runCatching { json.decodeFromString(serializer, it) }.getOrNull() } ?: emptyList()

    private fun save(list: List<Agent>) {
        prefs.edit().putString("list", json.encodeToString(serializer, list)).apply()
        _agents.value = list
    }

    fun agent(id: String): Agent? = _agents.value.firstOrNull { it.id == id }

    fun hermes(agent: Agent): Hermes? = secrets.keys(agent.id)?.let { Hermes(Api(agent.baseUrl, it.apiKey)) }

    fun bridge(agent: Agent): Bridge? {
        val url = agent.bridgeUrl ?: return null
        val key = secrets.keys(agent.id)?.bridgeKey ?: return null
        return Bridge(Api(url, key))
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
                    category = a.category, color = a.color),
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
    suspend fun setVoice(agent: Agent, voice: String) {
        val changed = agent.copy(voice = voice)
        update(changed)
        runCatching { bridge(changed)?.setVoices(changed.bridgeName, spoken(voice, changed.language), changed.serverVoice) }
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
        val hermes = hermes(agent) ?: return
        try {
            val sessions = hermes.allSessions(maxPages = 3) { all -> all.any { it.isBotChat } }
            Session.main(sessions)?.let { s -> _latest.update { it + (agent.id to s) } }
            _online.update { it + (agent.id to true) }
        } catch (error: HttpError) {
            _online.update { it + (agent.id to (error.status != 0)) }
        }
    }

    fun noteSession(agent: Agent, session: Session) = _latest.update { it + (agent.id to session) }

    private fun same(a: Agent, b: Agent) =
        (a.bridgeAgent != null && a.bridgeAgent == b.bridgeAgent && a.bridgeUrl == b.bridgeUrl) || a.baseUrl == b.baseUrl
}
