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

    /** The looks chosen in another app (the iPhone), kept by each bridge: the same Bips here. */
    suspend fun syncLooks() {
        for ((bridge, mine) in bridges()) {
            val listed = runCatching { bridge.agents() }.getOrNull() ?: continue
            for (agent in mine) {
                val found = listed.firstOrNull { it.id.equals(agent.bridgeName, true) } ?: continue
                if (found.category != null && (found.category != agent.category || found.color != agent.color)) {
                    update(agent.copy(category = found.category, color = found.color))
                }
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
