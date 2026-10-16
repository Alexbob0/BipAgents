package io.github.bipagents.ui

import android.app.Application
import android.content.Context
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import io.github.bipagents.BipApp
import io.github.bipagents.model.Agent
import io.github.bipagents.net.HttpError
import io.github.bipagents.net.Message
import io.github.bipagents.net.ToolStep
import io.github.bipagents.net.InboxItem
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
import kotlinx.coroutines.flow.firstOrNull
import io.github.bipagents.net.str
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonPrimitive
import android.util.Base64
import io.github.bipagents.model.Attachment
import io.github.bipagents.voice.RecordedNote
import io.github.bipagents.voice.Speaker
import io.github.bipagents.voice.VoiceNotes
import io.github.bipagents.voice.VoiceReply
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject

/** One line of the thread. */
sealed interface Item {
    val key: String
    val time: Long?
    data class User(override val key: String, val text: String, val image: Boolean, override val time: Long?,
                    val files: List<io.github.bipagents.model.StoredFile> = emptyList()) : Item
    data class Reply(override val key: String, val text: String, override val time: Long?, val streaming: Boolean = false) : Item
    data class Reasoning(override val key: String, val text: String, override val time: Long?) : Item
    data class Tools(override val key: String, val steps: List<ToolStep>, override val time: Long?) : Item
    /** A scheduled task's report or a proactive message (the bridge's outbox), at its time in the Bot Chat. */
    data class Scheduled(override val key: String, val message: InboxItem, override val time: Long?) : Item
}

/** The turn in progress: what was sent, then what the agent does as it comes (text, reasoning, tools). */
data class Live(val sent: String?, val items: List<Item> = emptyList(), val run: String? = null, val interim: String? = null,
                val files: List<io.github.bipagents.model.StoredFile> = emptyList()) {
    /** Grows with every event (follows the end of the thread). */
    val size get() = items.sumOf { when (it) { is Item.Reply -> it.text.length; is Item.Reasoning -> it.text.length; is Item.Tools -> it.steps.size; else -> 1 } }
    val streaming get() = (items.lastOrNull() as? Item.Reply)?.streaming == true
}

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
    /** The agent's thinking level, when the app may change it. */
    val thinking: String? = null,
)

/** A conversation (ConversationModel.swift): turns sent as Hermes runs, followed through the bridge. */
class ChatModel(application: Application) : AndroidViewModel(application) {
    private val store = (application as BipApp).store
    private val sentFiles = io.github.bipagents.model.SentFiles(application)
    private val runs = application.getSharedPreferences("runs", Context.MODE_PRIVATE)
    private val _state = MutableStateFlow(ChatState())
    val state: StateFlow<ChatState> = _state.asStateFlow()
    private var following: Job? = null
    private var started = false
    val notes = VoiceNotes(application)
    private val app = application
    /** The turn came from a voice note and the policy says to answer out loud. */
    private var replyByVoice = false
    /** A voice note waiting for its session (a new conversation gets one when the message goes). */
    private var pendingNote: RecordedNote? = null
    /** The app is in the background: no following, no « looking at it » for the bridge. */
    private var detached = false

    fun start(agentId: String, session: String?) {
        if (started) return
        started = true
        val agent = store.agent(agentId) ?: return
        _state.update { it.copy(agent = agent, session = session?.ifEmpty { null }) }
        viewModelScope.launch {
            if (session == null) {
                // The agent's ongoing conversation: its Bot Chat, else the latest one.
                val sessions = runCatching { store.hermes(agent)?.allSessions(3) { all -> all.any(Session::isBotChat) } }
                val main = sessions.getOrNull()?.let(Session::main)
                // Server out of reach: the one found last time, from the phone's copy.
                val id = main?.id ?: if (sessions.isFailure) store.latest.value[agent.id]?.id else null
                _state.update { it.copy(session = id, title = main?.title, error = sessions.exceptionOrNull()) }
            }
            launch { store.bridge(agent)?.let { b -> runCatching { b.thinking(agent.bridgeName) }.getOrNull() }?.let { t -> _state.update { it.copy(thinking = t) } } }
            load()
            _state.value.session?.let { s -> runs.getString(s, null)?.let { follow(it, runs.getString("sent.$s", null), reattaching = true) } }
            watch()
        }
    }

    private suspend fun load() {
        val agent = _state.value.agent ?: return
        val session = _state.value.session ?: return _state.update { it.copy(items = emptyList()) }
        if (_state.value.items == null) kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { ThreadCache.load(app, session) }?.let { cached ->
            // Shown at once from the phone's copy; the call below only updates what changed.
            _state.update { it.copy(items = restore(thread(cached.messages, cached.outbox), session), title = it.title ?: cached.title) }
        }
        try {
            val hermes = store.hermes(agent) ?: return
            if (_state.value.title == null) runCatching { hermes.session(session) }.getOrNull()?.title?.let { t -> _state.update { it.copy(title = t) } }
            val messages = hermes.messages(session)
            val outbox = if (_state.value.title.equals("Bot Chat", ignoreCase = true)) scheduled(agent) else emptyList()
            _state.update { it.copy(items = restore(thread(messages, outbox), session), error = null) }
            val title = _state.value.title
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { ThreadCache.save(app, session, ThreadCache.Entry(title, messages, outbox)) }
        } catch (error: HttpError) {
            _state.update { it.copy(items = it.items ?: emptyList(), error = error) }
        }
    }

    /** The photos and files sent, back on their messages (matched by text, in order). */
    private fun restore(items: List<Item>, session: String): List<Item> {
        val pending = sentFiles.entries(session).map { it to io.github.bipagents.model.SentFiles.normalized(it.text) }.toMutableList()
        if (pending.isEmpty()) return items
        return items.map { item ->
            if (item !is Item.User || pending.isEmpty()) return@map item
            val history = io.github.bipagents.model.SentFiles.normalized(item.text)
            // The history may add document references to the text; a photo sent alone comes back empty or short.
            val match = pending.indexOfFirst { (_, sent) -> if (sent.isEmpty()) history.length <= 12 else sent in history }
            if (match < 0) item else item.copy(files = pending.removeAt(match).first.files)
        }
    }

    /** The agent's scheduled-task reports and proactive messages of the last 30 days, from its bridge. */
    private suspend fun scheduled(agent: Agent): List<InboxItem> {
        val bridge = store.bridge(agent) ?: return emptyList()
        val since = System.currentTimeMillis() - 30L * 24 * 3600 * 1000
        return runCatching { bridge.inbox() }.getOrDefault(emptyList()).filter { it.agent == agent.bridgeName && (it.time ?: 0) > since }
    }

    /** The audio of an outbox item (a podcast, or its synthesized voice), kept on the phone. */
    fun playScheduled(item: InboxItem) {
        if (Speaker.now.value?.key == "outbox-${item.id}") return Speaker.stop()
        val agent = _state.value.agent ?: return
        val bridge = store.bridge(agent) ?: return
        val file = java.io.File(app.cacheDir, "outbox").apply { mkdirs() }.resolve(item.id.hashCode().toUInt().toString(16) + ".mp3")
        viewModelScope.launch {
            try {
                if (!file.exists()) {
                    val bytes = bridge.api.bytes("v1/outbox/${io.github.bipagents.net.Hermes.seg(item.id)}/audio")
                    kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { file.writeBytes(bytes) }
                }
                Speaker.play("outbox-${item.id}", file)
            } catch (error: Throwable) {
                _state.update { it.copy(error = error) }
            }
        }
    }

    /** Turns the agent takes on its own (a routine, a reply that came meanwhile) show up while the thread is open; the
     *  bridge also learns that the person is looking at it (no push for what is on screen). During a turn, Hermes is
     *  asked whether the run is still going (its events may never come) or waits for an approval not shown. */
    private fun watch() = viewModelScope.launch {
        var count: Long? = null
        while (isActive) {
            delay(8000)
            if (detached) continue
            val agent = _state.value.agent ?: continue
            val session = _state.value.session ?: continue
            if (following != null) { checkRun(agent); continue }
            val bridge = store.bridge(agent) ?: continue
            val now = runCatching {
                bridge.api.call("v1/sessions/${session}/state", query = mapOf("agent" to agent.bridgeName))
            }.getOrNull()?.let { (it as? kotlinx.serialization.json.JsonObject)?.get("message_count") as? JsonPrimitive }?.content?.toLongOrNull()
            if (count != null && now != null && now != count) load()
            count = now ?: count
        }
    }

    private suspend fun checkRun(agent: Agent) {
        val run = _state.value.live?.run ?: return
        val status = runCatching { store.hermes(agent)?.run(run) }.getOrNull() ?: return
        if (status.approval != null && _state.value.approval == null) _state.update { it.copy(approval = status.approval, mood = Mood.ASKING) }
        else if (status.terminal && following != null && !detached) {
            // Already over on Hermes, its events never came: the history has the reply.
            following?.cancel()
            finish(agent, _state.value.session, finished = true)
        }
    }

    fun send(text: String, attachments: List<Attachment> = emptyList()): Boolean {
        val agent = _state.value.agent ?: return false
        val trimmed = text.trim()
        if ((trimmed.isEmpty() && attachments.isEmpty()) || following != null) return false
        val hermes = store.hermes(agent) ?: return false
        val stored = sentFiles.write(attachments)
        _state.update { it.copy(live = Live(trimmed, files = stored), error = null, mood = Mood.THINKING) }
        following = viewModelScope.launch {
            var session: String? = null
            try {
                val title = trimmed.ifEmpty { attachments.first().name }.take(60)
                session = _state.value.session ?: hermes.createSession(title).also { s ->
                    _state.update { it.copy(session = s.id, title = s.title ?: title) }
                    store.noteSession(agent, s)
                }.id
                pendingNote?.let { notes.add(session, it) }
                pendingNote = null
                sentFiles.add(session, trimmed, stored)
                // Files go to the bridge first; the message names where the agent finds them.
                val documents = attachments.filter { !it.isImage }.map { file ->
                    val bridge = store.bridge(agent) ?: throw HttpError(404, "no bridge for files")
                    Attachment.referenceLine(file.name, file.mime, file.bytes.size, bridge.upload(agent.bridgeName, file.name, file.mime, file.bytes))
                }
                val words = listOf(trimmed, documents.joinToString("\n")).filter { it.isNotEmpty() }.joinToString("\n\n")
                val images = attachments.filter { it.isImage }
                val input = if (images.isEmpty()) buildJsonObject { put("input", JsonPrimitive(words)) } else buildJsonObject {
                    // /v1/runs reads multimodal input as a list of messages: one user message whose content is the parts.
                    put("input", JsonArray(listOf(buildJsonObject {
                        put("role", JsonPrimitive("user"))
                        put("content", JsonArray(listOfNotNull(
                            words.takeIf { it.isNotEmpty() }?.let { buildJsonObject { put("type", JsonPrimitive("text")); put("text", JsonPrimitive(it)) } },
                        ) + images.map { image ->
                            buildJsonObject {
                                put("type", JsonPrimitive("image_url"))
                                put("image_url", buildJsonObject {
                                    put("url", JsonPrimitive("data:${image.mime};base64," + Base64.encodeToString(image.bytes, Base64.NO_WRAP)))
                                })
                            }
                        }))
                    })))
                }
                // A run lives on the server whatever happens to this connection, and can be followed again later. A
                // server that rejects runs (or photos in runs) gets chat/stream instead.
                var run: String? = null
                var ackLost = false
                if (agent.id !in runsUnavailable && !(images.isNotEmpty() && agent.id in imagesUnavailableInRuns)) {
                    try {
                        run = hermes.startRun(session, input)
                    } catch (error: HttpError) {
                        when {
                            // The run may have started before the answer broke: never resend (the agent would do the
                            // task twice), wait for the reply in the transcript.
                            error.status >= 500 -> ackLost = true
                            error.status in setOf(400, 404, 405, 413, 415, 422) ->
                                if (images.isNotEmpty()) imagesUnavailableInRuns += agent.id else runsUnavailable += agent.id
                            else -> throw error
                        }
                    }
                }
                when {
                    ackLost -> { waitForReply(hermes, session, trimmed); finish(agent, session, finished = true) }
                    run != null -> {
                        runs.edit().putString(session, run).putString("sent.$session", trimmed).apply()
                        stream(run, trimmed)
                    }
                    else -> {
                        hermes.chatTurn(session, input).firstOrNull { apply(it) }
                        finish(agent, session, finished = true)
                    }
                }
            } catch (error: Throwable) {
                if (error is kotlinx.coroutines.CancellationException) throw error
                if (session != null && runs.getString(session, null) == null) {
                    _state.update { it.copy(error = error) }
                    finish(agent, session, finished = false)
                } else {
                    _state.update { it.copy(live = null, error = error, mood = Mood.IDLE) }
                    following = null
                }
            }
        }
        return true
    }

    /** The POST's answer was lost: the reply shows up in the transcript (checked every 3 s, for 15 min at most). */
    private suspend fun waitForReply(hermes: io.github.bipagents.net.Hermes, session: String, text: String) {
        live { it.copy(interim = app.getString(io.github.bipagents.R.string.agent_working)) }
        val until = System.currentTimeMillis() + 15 * 60_000
        val asked = io.github.bipagents.model.SentFiles.normalized(text)
        while (System.currentTimeMillis() < until) {
            delay(3000)
            val history = runCatching { hermes.messages(session) }.getOrNull() ?: continue
            val at = history.indexOfLast { it.role == "user" && io.github.bipagents.model.SentFiles.normalized(it.text).contains(asked) }
            if (at >= 0 && history.drop(at + 1).any { it.role == "assistant" && it.text.isNotEmpty() }) return
        }
    }

    private fun follow(run: String, sent: String?, reattaching: Boolean = false) {
        following = viewModelScope.launch { stream(run, sent, reattaching) }
    }

    /** The app went to the background: stop following, so the bridge (which pushes only when nobody follows a run)
     *  notifies when the reply is ready. Followed again when the app comes back. */
    fun detach() {
        detached = true
        if (following != null && _state.value.live?.run != null) {
            following?.cancel()
            following = null
        }
    }

    fun reattach() {
        if (!detached) return
        detached = false
        val session = _state.value.session ?: return
        if (following != null) return
        val run = runs.getString(session, null)
        if (run != null) follow(run, _state.value.live?.sent ?: runs.getString("sent.$session", null), reattaching = true)
        else viewModelScope.launch { load() }
    }

    private suspend fun stream(run: String, sent: String?, reattaching: Boolean = false) {
        val agent = _state.value.agent ?: return
        val session = _state.value.session
        // The message that started the run, until Hermes' history has it (stored with the reply, at the end).
        val shown = sent?.takeUnless { text ->
            val last = (_state.value.items ?: emptyList()).lastOrNull { it is Item.User } as? Item.User
            val n = io.github.bipagents.model.SentFiles.normalized(text)
            last != null && _state.value.items?.lastOrNull() !is Item.User && n.isNotEmpty() &&
                io.github.bipagents.model.SentFiles.normalized(last.text).contains(n) && !reattaching
        }
        _state.update { it.copy(live = Live(shown ?: it.live?.sent, it.live?.items ?: emptyList(), run = run, files = it.live?.files ?: emptyList()),
            mood = Mood.THINKING) }
        var finished = false
        try {
            val bridge = store.bridge(agent)
            val hermes = store.hermes(agent) ?: return
            finished = if (bridge != null && agent.id !in bridgeWithoutRunEvents) {
                followViaBridge(bridge, agent, run, session, reattaching) ?: followViaHermes(hermes, run, reattaching = true)
            } else followViaHermes(hermes, run, reattaching)
        } catch (error: Throwable) {
            if (error is kotlinx.coroutines.CancellationException) throw error
            _state.update { it.copy(error = error) }
        }
        finish(agent, session, finished)
    }

    /** The turn is over (or lost): the thread from Hermes' history, the reply read out if it came from a voice note. */
    private suspend fun finish(agent: Agent, session: String?, finished: Boolean) {
        if (finished && session != null) runs.edit().remove(session).remove("sent.$session").apply()
        load()
        _state.update { it.copy(live = null, approval = null, clarify = null, mood = Mood.IDLE) }
        following = null
        if (replyByVoice) {
            replyByVoice = false
            speakTurn()
        }
        if (session != null) _state.value.items?.lastOrNull()?.let { last ->
            store.noteSession(agent, Session(session, _state.value.title, System.currentTimeMillis(), (last as? Item.Reply)?.text, null))
        }
    }

    /** The reply after the last message, cleared: a replayed run rebuilds it. */
    private fun clearTurn() {
        live { it.copy(items = emptyList(), interim = null) }
        _state.update { it.copy(approval = null, clarify = null) }
    }

    /** Through the bridge, which replays the run from its start on each connection: reconnects (screen locked,
     *  network waking up) with a growing delay. Null when the bridge cannot follow this run (then Hermes does). */
    private suspend fun followViaBridge(bridge: io.github.bipagents.net.Bridge, agent: Agent, run: String, session: String?,
                                        reattaching: Boolean): Boolean? {
        val reconnecting = app.getString(io.github.bipagents.R.string.reconnecting)
        val backoff = Backoff()
        var failures = 0
        var connections = 0
        while (true) {
            kotlinx.coroutines.yield()   // stops here once cancelled
            if (connections > 0 || reattaching) { clearTurn(); showPendingApproval(agent, run) }
            if (connections > 0) live { it.copy(interim = reconnecting) }
            connections++
            var lost = false
            var over = false
            try {
                bridge.runEventsRaw(agent.bridgeName, run, session).firstOrNull { event ->
                    failures = 0
                    backoff.reset()
                    if (_state.value.live?.interim == reconnecting) live { it.copy(interim = null) }
                    if (event.type == "bridge.error") {
                        // The bridge keeps retrying an unreachable Hermes; anything else: it lost the run.
                        if (event.data.str("code") != "hermes_unreachable") lost = true
                        return@firstOrNull lost
                    }
                    val e = RunEvent.of(event) ?: return@firstOrNull false
                    over = apply(e)
                    over
                }
            } catch (error: HttpError) {
                if (error.status == 404) { bridgeWithoutRunEvents += agent.id; return null }
                if (!retryable(error)) throw error
                if (++failures > 12) throw error
            }
            if (lost) return null
            if (over) return true
            delay(backoff.next())
        }
    }

    /** Straight from Hermes: its events, and the run's status when the subscription ends or drops. */
    private suspend fun followViaHermes(hermes: io.github.bipagents.net.Hermes, run: String, reattaching: Boolean): Boolean {
        val agent = _state.value.agent ?: return true
        val backoff = Backoff()
        var failures = 0
        var attempts = 0
        while (true) {
            kotlinx.coroutines.yield()   // stops here once cancelled
            if (attempts > 0 || reattaching) clearTurn()
            attempts++
            var over = false
            try {
                hermes.runEvents(run).firstOrNull { e -> apply(e).also { over = it } }
            } catch (error: HttpError) {
                if (error.status == 404) return true
                if (!retryable(error)) throw error
                if (++failures > 12) throw error
            }
            if (over) return true
            val status = runCatching { hermes.run(run) }
            if ((status.exceptionOrNull() as? HttpError)?.status == 404) return true
            status.getOrNull()?.let { s ->
                if (s.terminal) return true
                if (s.approval != null && _state.value.approval == null) _state.update { it.copy(approval = s.approval, mood = Mood.ASKING) }
            }
            showPendingApproval(agent, run)
            delay(backoff.next())
        }
    }

    /** An approval the replay no longer has stays answerable (Hermes keeps it in the run's status). */
    private suspend fun showPendingApproval(agent: Agent, run: String) {
        if (_state.value.approval != null) return
        val approval = runCatching { store.hermes(agent)?.run(run) }.getOrNull()?.approval ?: return
        _state.update { it.copy(approval = approval, mood = Mood.ASKING) }
    }

    private fun retryable(error: HttpError) = error.status == 0 || error.status == 408 || error.status == 429 || error.status >= 500

    private fun live(change: (Live) -> Live) = _state.update { s -> s.copy(live = s.live?.let(change)) }

    /** True once the run is over. */
    private fun apply(event: RunEvent): Boolean {
        // Something else follows a text: that text is complete.
        if (event is RunEvent.Reasoning || event is RunEvent.Tool || event is RunEvent.Approval || event is RunEvent.Clarify) live { l ->
            l.copy(items = l.items.map { if (it is Item.Reply && it.streaming) it.copy(streaming = false) else it })
        }
        when (event) {
            is RunEvent.Delta -> {
                live { l ->
                    val last = l.items.lastOrNull()
                    l.copy(interim = null, items = if (last is Item.Reply && last.streaming) l.items.dropLast(1) + last.copy(text = last.text + event.text)
                        else l.items + Item.Reply("live-${l.items.size}", event.text, null, streaming = true))
                }
                _state.update { it.copy(mood = Mood.SPEAKING) }
            }
            is RunEvent.Interim -> if (!event.alreadyStreamed && event.text.isNotBlank()) live { it.copy(interim = event.text.trim()) }
            is RunEvent.Reasoning -> live { l ->
                val last = l.items.lastOrNull()
                l.copy(items = if (last is Item.Reasoning) l.items.dropLast(1) + last.copy(text = last.text + event.text)
                    else l.items + Item.Reasoning("live-${l.items.size}", event.text, null))
            }
            is RunEvent.Tool -> {
                live { l -> l.copy(items = upsert(l.items, event.step)) }
                if (event.started) _state.update { it.copy(mood = Mood.THINKING) }
            }
            is RunEvent.Approval -> _state.update { it.copy(approval = event, mood = Mood.ASKING) }
            RunEvent.ApprovalResolved -> _state.update { it.copy(approval = null, mood = Mood.THINKING) }
            is RunEvent.Clarify -> _state.update { it.copy(clarify = event, mood = Mood.ASKING) }
            RunEvent.ClarifyResolved -> _state.update { it.copy(clarify = null, mood = Mood.THINKING) }
            is RunEvent.Finished -> {
                if (event.failed) _state.update { it.copy(error = HttpError(500, event.error)) }
                val output = event.output
                if (output != null) live { l ->
                    if (l.items.any { it is Item.Reply }) l else l.copy(items = l.items + Item.Reply("live-${l.items.size}", output, null))
                }
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

    /** The agent's thinking from its next message (the bridge sets it in Hermes); back as it was if refused. */
    fun setThinking(level: String) {
        val agent = _state.value.agent ?: return
        val before = _state.value.thinking ?: return
        _state.update { it.copy(thinking = level) }
        viewModelScope.launch {
            runCatching { store.bridge(agent)?.setThinking(agent.bridgeName, level) }.onFailure { e ->
                _state.update { it.copy(thinking = before, error = e) }
            }
        }
    }

    /** A voice note: its transcript goes as the message, its recording stays with the conversation. */
    fun sendVoice(note: RecordedNote) {
        pendingNote = note
        replyByVoice = VoiceReply.wanted(app, note.text)
        if (!send(note.text)) { pendingNote = null; replyByVoice = false }
    }

    /** The turn's reply, read out: every reply after the last thing the person said, as one voice message. */
    private fun speakTurn() {
        val items = _state.value.items ?: return
        val after = items.subList(items.indexOfLast { it is Item.User } + 1, items.size).filterIsInstance<Item.Reply>()
        val last = after.lastOrNull() ?: return
        listen(last.key, after.joinToString("\n\n") { it.text }, "replies")
    }

    /** « Listen »: the bridge reads `text` in the agent's voice for that use, streamed (first words in about a second),
     *  and keeps it; heard before, it plays from the phone. Pressed again while it plays: stop. */
    fun listen(key: String, text: String, use: String = "reading") {
        if (Speaker.now.value?.key == key) return Speaker.stop()
        val agent = _state.value.agent ?: return
        val words = ChatText.visible(text).take(MAX_SPOKEN)
        if (words.isEmpty()) return
        val name = java.security.MessageDigest.getInstance("SHA-1").digest((agent.id + use + words).toByteArray())
            .joinToString("") { "%02x".format(it) }.take(20)
        val folder = java.io.File(app.cacheDir, "voice").apply { mkdirs() }
        val wav = folder.resolve("$name.wav")
        val mp3 = folder.resolve("$name.mp3")
        when {
            wav.exists() -> return Speaker.play(key, wav)
            mp3.exists() -> return Speaker.play(key, mp3)
        }
        val bridge = store.bridge(agent) ?: return _state.update { it.copy(error = HttpError(404, null)) }
        val body = buildJsonObject {
            put("text", JsonPrimitive(words)); put("agent", JsonPrimitive(agent.bridgeName)); put("use", JsonPrimitive(use))
        }
        Speaker.stream(key, wav, open = { bridge.api.open("v1/tts/stream", body) }, onError = {
            // An older bridge, or a voice that only makes whole files: one file for the reply instead.
            viewModelScope.launch {
                try {
                    val audio = bridge.api.bytes("v1/tts/message", "POST", io.github.bipagents.net.Http.body(
                        JsonObject(body + ("format" to JsonPrimitive("mp3")))))
                    mp3.writeBytes(audio)
                    Speaker.play(key, mp3)
                } catch (error: Throwable) {
                    Speaker.stop()
                    _state.update { it.copy(error = error) }
                }
            }
        })
    }

    companion object {
        /** Agents whose server rejected `POST /v1/runs` (or photos in runs) during this launch. */
        private val runsUnavailable = HashSet<String>()
        private val imagesUnavailableInRuns = HashSet<String>()
        /** Agents whose bridge does not follow runs (an older one). */
        private val bridgeWithoutRunEvents = HashSet<String>()

        /** The bridge reads at most 20 000 characters; a reply that long is better read than heard anyway. */
        const val MAX_SPOKEN = 8000

        /** The thread from Hermes' stored messages (ConversationModel.items(from:)). Consecutive tool calls share one
         *  card, and a tool's result (its own message) only completes the call already listed. Reports from the outbox go
         *  in at their time. */
        fun thread(messages: List<Message>, outbox: List<InboxItem> = emptyList()): List<Item> {
            val out = mutableListOf<Item>()
            fun addTools(steps: List<ToolStep>, time: Long?, key: String) {
                val last = out.lastOrNull()
                if (last is Item.Tools) out[out.size - 1] = last.copy(steps = last.steps + steps) else out += Item.Tools(key, steps, time)
            }
            for ((i, m) in messages.withIndex()) {
                val k = "${m.id}-$i"
                if (m.role == "tool") {
                    // Calls listed since the person's last message (the commentary may sit between call and result).
                    val listed = out.subList(out.indexOfLast { it is Item.User } + 1, out.size).filterIsInstance<Item.Tools>().flatMap { it.steps }
                    val fresh = m.tools.filter { r -> listed.none { if (it.callId != null) it.callId == r.callId else it.name == r.name } }
                    if (fresh.isNotEmpty()) addTools(fresh, m.time, "t$k")
                    continue
                }
                m.reasoning?.let { out += Item.Reasoning("th$k", it, m.time) }
                if (m.tools.isNotEmpty()) addTools(m.tools, m.time, "t$k")
                when (m.role) {
                    "user" -> out += Item.User("u$k", m.text, m.hasImage, m.time)
                    "assistant" -> if (ChatText.visible(m.text).isNotEmpty() || ChatText.media(m.text).isNotEmpty()) out += Item.Reply("r$k", m.text, m.time)
                }
            }
            for (o in outbox.sortedBy { it.time ?: 0 }) {
                val at = o.time ?: Long.MAX_VALUE
                val index = out.indexOfFirst { (it.time ?: Long.MIN_VALUE) > at }.takeIf { it >= 0 } ?: out.size
                out.add(index, Item.Scheduled("o${o.id}", o, o.time))
            }
            return out
        }

        /** A tool event: completes the call already listed (same id, or same name still running), else a new line. */
        fun upsert(items: List<Item>, step: ToolStep): List<Item> {
            val last = items.lastOrNull()
            if (last !is Item.Tools) return items + Item.Tools("live-${items.size}", listOf(step), null)
            val steps = last.steps.toMutableList()
            val index = if (step.status == ToolStep.Status.STARTED) -1 else steps.indexOfLast {
                it.status == ToolStep.Status.STARTED && (it.callId == step.callId || (it.callId == null && it.name == step.name))
            }
            if (index >= 0) steps[index] = step.copy(preview = step.preview ?: steps[index].preview) else steps += step
            return items.dropLast(1) + last.copy(steps = steps)
        }
    }
}

/** The phone's copy of each conversation (its last 300 messages), so a thread opens at once and reads offline. */
object ThreadCache {
    @kotlinx.serialization.Serializable
    data class Entry(val title: String?, val messages: List<Message>, val outbox: List<InboxItem> = emptyList())

    private fun file(context: Context, session: String): java.io.File {
        val key = java.security.MessageDigest.getInstance("SHA-256").digest(session.toByteArray()).take(12)
            .joinToString("") { "%02x".format(it) }
        return java.io.File(context.cacheDir, "threads").resolve("$key.json")
    }

    fun load(context: Context, session: String): Entry? = runCatching {
        io.github.bipagents.net.json.decodeFromString<Entry>(file(context, session).readText())
    }.getOrNull()

    fun save(context: Context, session: String, entry: Entry) {
        runCatching {
            val f = file(context, session)
            f.parentFile?.mkdirs()
            val tmp = java.io.File(f.path + ".tmp")
            tmp.writeText(io.github.bipagents.net.json.encodeToString(entry.copy(messages = entry.messages.takeLast(300))))
            tmp.renameTo(f)
        }
    }
}

/** Delays between reconnections: 0.5 s doubling up to 8 s, ±20 % (HermesKit's Backoff). */
class Backoff(private val initial: Long = 500, private val maximum: Long = 8000) {
    private var attempt = 0
    fun reset() { attempt = 0 }
    fun next(): Long {
        val base = minOf(maximum.toDouble(), initial * Math.pow(2.0, attempt.toDouble()))
        attempt++
        return (base * (0.8 + Math.random() * 0.4)).toLong()
    }
}
