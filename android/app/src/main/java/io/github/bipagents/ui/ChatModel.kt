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
    val notes = VoiceNotes(application)
    private val app = application
    /** The turn came from a voice note and the policy says to answer out loud. */
    private var replyByVoice = false
    /** A voice note waiting for its session (a new conversation gets one when the message goes). */
    private var pendingNote: RecordedNote? = null

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
            load()
            _state.value.session?.let { s -> runs.getString(s, null)?.let { follow(it, null) } }
            watch()
        }
    }

    private suspend fun load() {
        val agent = _state.value.agent ?: return
        val session = _state.value.session ?: return _state.update { it.copy(items = emptyList()) }
        if (_state.value.items == null) kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { ThreadCache.load(app, session) }?.let { cached ->
            // Shown at once from the phone's copy; the call below only updates what changed.
            _state.update { it.copy(items = thread(cached.messages), title = it.title ?: cached.title) }
        }
        try {
            val messages = store.hermes(agent)?.messages(session) ?: emptyList()
            _state.update { it.copy(items = thread(messages), error = null) }
            val title = _state.value.title
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { ThreadCache.save(app, session, ThreadCache.Entry(title, messages)) }
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

    fun send(text: String, attachments: List<Attachment> = emptyList()): Boolean {
        val agent = _state.value.agent ?: return false
        val trimmed = text.trim()
        if ((trimmed.isEmpty() && attachments.isEmpty()) || following != null) return false
        val hermes = store.hermes(agent) ?: return false
        val shown = (attachments.joinToString("") { (if (it.isImage) "🖼️ " else "📎 ") + it.name + "\n" } + trimmed).trim()
        _state.update { it.copy(live = Live(shown), error = null, mood = Mood.THINKING) }
        following = viewModelScope.launch {
            try {
                val title = trimmed.ifEmpty { attachments.first().name }.take(60)
                val session = _state.value.session ?: hermes.createSession(title).also { s ->
                    _state.update { it.copy(session = s.id, title = s.title ?: title) }
                    store.noteSession(agent, s)
                }.id
                pendingNote?.let { notes.add(session, it) }
                pendingNote = null
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
                val run = hermes.startRun(session, input)
                runs.edit().putString(session, run).apply()
                stream(run, trimmed)
            } catch (error: Throwable) {
                if (error is kotlinx.coroutines.CancellationException) throw error
                _state.update { it.copy(live = null, error = error, mood = Mood.IDLE) }
                following = null
            }
        }
        return true
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
        if (replyByVoice) {
            replyByVoice = false
            speakTurn()
        }
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
        /** The bridge reads at most 20 000 characters; a reply that long is better read than heard anyway. */
        const val MAX_SPOKEN = 8000

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

/** The phone's copy of each conversation (its last 300 messages), so a thread opens at once and reads offline. */
object ThreadCache {
    @kotlinx.serialization.Serializable
    data class Entry(val title: String?, val messages: List<Message>)

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
