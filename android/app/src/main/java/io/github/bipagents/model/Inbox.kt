package io.github.bipagents.model

import android.content.Context
import io.github.bipagents.net.InboxItem
import io.github.bipagents.net.RunEvent
import io.github.bipagents.net.json
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer

/** The Inbox (InboxStore.swift): the agents' messages from every bridge's outbox, the replies finished while their
 *  conversation was closed, and the approvals waiting, whatever conversation asked. */
class Inbox(context: Context) {
    data class Entry(val item: InboxItem, val agent: Agent) { val id get() = "${agent.id}/${item.id}" }
    data class Pending(val request: RunEvent.Approval, val agent: Agent, val session: String?) { val id get() = "${agent.id}/${request.run}" }

    /** A reply finished while its conversation was closed (from its notification). */
    @Serializable
    data class Missed(val id: String, val agent: String, val session: String?, val text: String, val time: Long)

    data class State(val entries: List<Entry> = emptyList(), val approvals: List<Pending> = emptyList(), val loading: Boolean = false,
                     val unreachable: Boolean = false, val lastSeen: Long = 0) {
        val unread get() = entries.count { (it.item.time ?: 0) > lastSeen } + approvals.size
    }

    private val prefs = context.getSharedPreferences("inbox", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(State(lastSeen = prefs.getLong("lastSeen", 0)))
    val state: StateFlow<State> = _state.asStateFlow()
    private var outbox: List<Entry> = emptyList()
    private val missedSerializer = ListSerializer(Missed.serializer())
    private var missed: List<Missed> = runCatching { json.decodeFromString(missedSerializer, prefs.getString("missed", "[]")!!) }.getOrDefault(emptyList())
    /** Bridge messages removed on this phone (the bridge keeps them for its retention). */
    private var dismissed: Set<String> = prefs.getStringSet("dismissed", emptySet())!!.toSet()

    private fun publish(agents: List<Agent>) {
        val replies = missed.mapNotNull { m ->
            val agent = agents.firstOrNull { it.id == m.agent } ?: return@mapNotNull null
            Entry(InboxItem("reply-${m.id}", agent.bridgeName, null, m.text, m.time, m.session, false), agent)
        }
        _state.update { s -> s.copy(entries = (outbox.filter { it.id !in dismissed } + replies).sortedByDescending { it.item.time ?: 0 }) }
    }

    suspend fun refresh(store: AgentStore) {
        _state.update { it.copy(loading = true) }
        var failures = 0
        val results = coroutineScope {
            store.bridges().map { (bridge, mine) ->
                async {
                    try {
                        val items = bridge.inbox().mapNotNull { item -> mine.firstOrNull { it.bridgeName == item.agent }?.let { Entry(item, it) } }
                        val waiting = mine.flatMap { agent ->
                            runCatching { bridge.approvals(agent.bridgeName) }.getOrDefault(emptyList()).map { (r, s) -> Pending(r, agent, s) }
                        }
                        items to waiting
                    } catch (e: Throwable) {
                        if (e is kotlinx.coroutines.CancellationException) throw e
                        failures++
                        emptyList<Entry>() to emptyList()
                    }
                }
            }.awaitAll()
        }
        outbox = results.flatMap { it.first }
        // Forget removals of messages the bridge no longer has, so the set stays small.
        if (failures == 0) saveDismissed(dismissed.intersect(outbox.map { it.id }.toSet()))
        _state.update { it.copy(approvals = results.flatMap { r -> r.second }, loading = false, unreachable = failures > 0) }
        publish(store.agents.value)
    }

    fun dismiss(entry: Entry, store: AgentStore) {
        if (entry.item.id.startsWith("reply-")) saveMissed(missed.filterNot { "reply-${it.id}" == entry.item.id })
        else saveDismissed(dismissed + entry.id)
        publish(store.agents.value)
    }

    fun dismissAll(store: AgentStore) {
        saveMissed(emptyList())
        saveDismissed(dismissed + outbox.map { it.id })
        publish(store.agents.value)
    }

    fun resolved(pending: Pending) = _state.update { s -> s.copy(approvals = s.approvals.filterNot { it.id == pending.id }) }

    fun markAllSeen() {
        val now = System.currentTimeMillis()
        prefs.edit().putLong("lastSeen", now).apply()
        _state.update { it.copy(lastSeen = now) }
    }

    /** A reply its notification announced: in the Inbox until its conversation is opened. */
    fun addMissed(agent: Agent, session: String?, id: String, text: String, store: AgentStore) {
        if (missed.any { it.id == id }) return
        saveMissed((missed + Missed(id, agent.id, session, text, System.currentTimeMillis())).takeLast(30))
        publish(store.agents.value)
    }

    /** The conversation was opened: its missed replies are read there. */
    fun readSession(session: String, store: AgentStore) {
        if (missed.none { it.session == session }) return
        saveMissed(missed.filterNot { it.session == session })
        publish(store.agents.value)
    }

    private fun saveMissed(list: List<Missed>) {
        missed = list
        prefs.edit().putString("missed", json.encodeToString(missedSerializer, list)).apply()
    }

    private fun saveDismissed(ids: Set<String>) {
        dismissed = ids
        prefs.edit().putStringSet("dismissed", ids).apply()
    }

    companion object {
        /** The scheduled task behind a message (its session is « cron_<job>_<date>_<time> »). */
        fun cronJob(session: String?): String? = session?.let { Regex("^cron_(.+?)_\\d{8}_\\d{6}$").find(it)?.groupValues?.get(1) }
    }
}
