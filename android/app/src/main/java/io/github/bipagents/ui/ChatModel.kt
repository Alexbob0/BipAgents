package io.github.bipagents.ui

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.bipagents.BipApp
import io.github.bipagents.model.Agent
import io.github.bipagents.net.HttpError
import io.github.bipagents.net.Message
import io.github.bipagents.net.RunEvent
import io.github.bipagents.net.Session
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject

/** One line of the thread. */
sealed interface Item {
    val key: String
    data class User(override val key: String, val text: String, val image: Boolean, val time: Long?) : Item
    data class Reply(override val key: String, val text: String, val tools: List<String>, val time: Long?) : Item
    data class Tools(override val key: String, val tools: List<String>) : Item
}

/** The turn in progress: what was sent, the reply as it streams, the tools used so far. */
data class Live(val sent: String?, val text: String = "", val tools: List<String> = emptyList(), val run: String? = null)

data class ChatState(
    val agent: Agent? = null,
    val session: String? = null,
    val title: String? = null,
    val items: List<Item>? = null,
    val live: Live? = null,
    val approval: RunEvent.Approval? = null,
    val clarify: RunEvent.Clarify? = null,
    val error: Throwable? = null,
    val mood: Mood = Mood.IDLE,
)

/** A conversation (ConversationModel.swift): turns sent as Hermes runs, followed through the bridge. */
class ChatModel(application: Application) : AndroidViewModel(application) {
    private val store = (application as BipApp).store
    private val runs = application.getSharedPreferences("runs", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(ChatState())
    val state: StateFlow<ChatState> = _state.asStateFlow()
    private var following: Job? = null
    private var started = false

    fun start(agentId: String, session: String?) {
        if (started) return
        started = true
        val agent = store.agent(agentId) ?: return
        _state.update { it.copy(agent = agent, session = session?.ifEmpty { null }) }
        viewModelScope.launch {
            if (session == null) {
                // The agent's ongoing conversation: its Bot Chat, else the latest one.
                val sessions = runCatching { store.hermes(agent)?.allSessions(3) { all -> all.any(Session::isBotChat) } }.getOrNull()
                val main = sessions?.let(Session::main)
                _state.update { it.copy(session = main?.id, title = main?.title) }
            }
            load()
            _state.value.session?.let { s -> runs.getString(s, null)?.let { follow(it, null) } }
            watch()
        }
    }

    private suspend fun load() {
        val agent = _state.value.agent ?: return
        val session = _state.value.session ?: return _state.update { it.copy(items = emptyList()) }
        try {
            val messages = store.hermes(agent)?.messages(session) ?: emptyList()
            _state.update { it.copy(items = thread(messages), error = null) }
        } catch (error: HttpError) {
            _state.update { it.copy(items = it.items ?: emptyList(), error = error) }
        }
    }

    /** Turns the agent takes on its own (a routine, a reply that came meanwhile) show up while the thread is open; the
     *  bridge also learns that the person is looking at it (no push for what is on screen). */
    private fun watch() = viewModelScope.launch {
        var count: Long? = null
        while (isActive) {
            delay(8000)
            val agent = _state.value.agent ?: continue
            val session = _state.value.session ?: continue
            if (following != null) continue
            val bridge = store.bridge(agent) ?: continue
            val now = runCatching {
                bridge.api.call("v1/sessions/${session}/state", query = mapOf("agent" to agent.bridgeName))
            }.getOrNull()?.let { (it as? kotlinx.serialization.json.JsonObject)?.get("message_count") as? JsonPrimitive }?.content?.toLongOrNull()
            if (count != null && now != null && now != count) load()
            count = now ?: count
        }
    }

    fun send(text: String) {
        val agent = _state.value.agent ?: return
        val trimmed = text.trim()
        if (trimmed.isEmpty() || following != null) return
        val hermes = store.hermes(agent) ?: return
        _state.update { it.copy(live = Live(trimmed), error = null, mood = Mood.THINKING) }
        following = viewModelScope.launch {
            try {
                val session = _state.value.session ?: hermes.createSession(trimmed.take(60)).also { s ->
                    _state.update { it.copy(session = s.id, title = s.title ?: trimmed.take(60)) }
                    store.noteSession(agent, s)
                }.id
                val run = hermes.startRun(session, buildJsonObject { put("input", JsonPrimitive(trimmed)) })
                runs.edit().putString(session, run).apply()
                stream(run, trimmed)
            } catch (error: Throwable) {
                if (error is kotlinx.coroutines.CancellationException) throw error
                _state.update { it.copy(live = null, error = error, mood = Mood.IDLE) }
                following = null
            }
        }
    }

    private fun follow(run: String, sent: String?) {
        following = viewModelScope.launch { stream(run, sent) }
    }

    private suspend fun stream(run: String, sent: String?) {
        val agent = _state.value.agent ?: return
        val session = _state.value.session
        _state.update { it.copy(live = Live(sent ?: it.live?.sent, run = run), mood = Mood.THINKING) }
        var finished = false
        try {
            val bridge = store.bridge(agent)
            val events = bridge?.runEvents(agent.bridgeName, run, session) ?: store.hermes(agent)!!.runEvents(run)
            events.collect { event -> if (apply(event)) finished = true }
        } catch (error: HttpError) {
            if (error.status == 404) finished = true else _state.update { it.copy(error = error) }
        } catch (error: Throwable) {
            if (error is kotlinx.coroutines.CancellationException) throw error
            _state.update { it.copy(error = error) }
        }
        if (finished && session != null) runs.edit().remove(session).apply()
        load()
        _state.update { it.copy(live = null, approval = null, clarify = null, mood = Mood.IDLE) }
        following = null
        if (session != null) _state.value.items?.lastOrNull()?.let { last ->
            store.noteSession(agent, Session(session, _state.value.title, System.currentTimeMillis(), (last as? Item.Reply)?.text, null))
        }
    }

    /** True once the run is over. */
    private fun apply(event: RunEvent): Boolean {
        when (event) {
            is RunEvent.Delta -> _state.update { s -> s.copy(live = s.live?.copy(text = s.live.text + event.text), mood = Mood.SPEAKING) }
            is RunEvent.Interim -> if (!event.alreadyStreamed && event.text.isNotBlank()) _state.update { s ->
                s.copy(live = s.live?.copy(text = s.live.text + (if (s.live.text.isEmpty()) "" else "\n\n") + event.text + "\n\n"))
            }
            is RunEvent.Tool -> if (event.started) _state.update { s -> s.copy(live = s.live?.copy(tools = s.live.tools + event.name), mood = Mood.THINKING) }
            is RunEvent.Approval -> _state.update { it.copy(approval = event, mood = Mood.ASKING) }
            RunEvent.ApprovalResolved -> _state.update { it.copy(approval = null, mood = Mood.THINKING) }
            is RunEvent.Clarify -> _state.update { it.copy(clarify = event, mood = Mood.ASKING) }
            RunEvent.ClarifyResolved -> _state.update { it.copy(clarify = null, mood = Mood.THINKING) }
            is RunEvent.Finished -> {
                if (event.failed) _state.update { it.copy(error = HttpError(500, event.error)) }
                return true
            }
        }
        return false
    }

    fun stop() {
        val agent = _state.value.agent ?: return
        val run = _state.value.live?.run ?: return
        viewModelScope.launch { runCatching { store.hermes(agent)?.stop(run) }.onFailure { e -> _state.update { it.copy(error = e) } } }
    }

    fun approve(choice: String) {
        val agent = _state.value.agent ?: return
        val request = _state.value.approval ?: return
        _state.update { it.copy(approval = null, mood = Mood.THINKING) }
        viewModelScope.launch {
            try {
                store.hermes(agent)?.approve(request.run, choice, request.requestId)
            } catch (error: Throwable) {
                _state.update { it.copy(approval = request, error = error) }
            }
        }
    }

    fun answer(answers: Map<String, String>?) {
        val agent = _state.value.agent ?: return
        val request = _state.value.clarify ?: return
        _state.update { it.copy(clarify = null) }
        viewModelScope.launch {
            try {
                store.hermes(agent)?.answer(request, answers)
            } catch (error: Throwable) {
                _state.update { it.copy(clarify = request, error = error) }
            }
        }
    }

    fun dismissError() = _state.update { it.copy(error = null) }

    companion object {
        /** The transcript as the app shows it: what the person wrote, the replies, and the tools used between them. */
        fun thread(messages: List<Message>): List<Item> {
            val out = mutableListOf<Item>()
            var tools = mutableListOf<String>()
            for (m in messages) when (m.role) {
                "assistant" -> {
                    tools += m.tools
                    if (ChatText.visible(m.text).isEmpty() && ChatText.media(m.text).isEmpty()) continue
                    out += Item.Reply("r${m.id}-${out.size}", m.text, tools.toList(), m.time)
                    tools = mutableListOf()
                }
                "user" -> {
                    if (tools.isNotEmpty()) { out += Item.Tools("t${out.size}", tools.toList()); tools = mutableListOf() }
                    out += Item.User("u${m.id}-${out.size}", m.text, m.hasImage, m.time)
                }
            }
            if (tools.isNotEmpty()) out += Item.Tools("t${out.size}", tools)
            return out
        }
    }
}
