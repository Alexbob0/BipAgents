package io.github.bipagents.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import io.github.bipagents.model.Agent
import io.github.bipagents.model.AgentStore
import io.github.bipagents.net.RunEvent
import io.github.bipagents.net.str
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import java.util.Locale
import kotlin.math.max
import kotlin.math.sqrt

/**
 * The Live (CallModel.swift + VoiceKit's call): the phone listens; each thing said goes to the agent (`chat/stream`,
 * thinking off); the reply is spoken sentence by sentence in the Bip's live voice as it streams; speaking over it (or
 * tapping the Bip) interrupts it and stops the turn.
 *
 * Barge-in: while the Bip speaks, the microphone is read with the phone's echo canceller and a voice is detected when
 * it is clearly louder than the Bip's own echo (measured in the first half-second), for a third of a second.
 */
class Live(private val context: Context, private val store: AgentStore, private val agent: Agent, private var session: String?) {
    enum class Phase { STARTING, LISTENING, THINKING, SPEAKING, ENDED }

    data class State(
        val phase: Phase = Phase.STARTING,
        val heard: String = "",
        val reply: String = "",
        val tool: String? = null,
        val approval: RunEvent.Approval? = null,
        val error: String? = null,
    )

    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val audio = context.getSystemService(AudioManager::class.java)
    private var recognizer: SpeechRecognizer? = null
    private var turn: Job? = null
    private var speaker: Job? = null
    private var bargeIn: Job? = null
    private var sentences = Channel<String>(Channel.UNLIMITED)
    private var track: AudioTrack? = null
    private var run: String? = null

    private val language = when (agent.language ?: Locale.getDefault().language) {
        "fr" -> "fr-FR"; "es" -> "es-ES"; "de" -> "de-DE"; "en" -> "en-US"; else -> Locale.getDefault().toLanguageTag()
    }

    fun start() {
        if (context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED ||
            !SpeechRecognizer.isRecognitionAvailable(context)) {
            _state.update { it.copy(error = "mic") }
            return
        }
        // A call: the phone's echo canceller and voice processing, on the loudspeaker unless a headset is there.
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        if (Build.VERSION.SDK_INT >= 31 && !VoiceReply.handsFree(context)) {
            audio.availableCommunicationDevices.firstOrNull { it.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER }?.let(audio::setCommunicationDevice)
        }
        listen()
    }

    fun hangUp() {
        _state.update { it.copy(phase = Phase.ENDED) }
        interrupt(stopRun = true)
        recognizer?.runCatching { cancel(); destroy() }
        recognizer = null
        if (Build.VERSION.SDK_INT >= 31) audio.clearCommunicationDevice()
        audio.mode = AudioManager.MODE_NORMAL
        scope.cancel()
    }

    /** Stops the Bip mid-sentence (tap, or the person speaking) and listens again. */
    fun interrupt(stopRun: Boolean = true) {
        bargeIn?.cancel(); bargeIn = null
        speaker?.cancel(); speaker = null
        sentences.close()
        sentences = Channel(Channel.UNLIMITED)
        track?.runCatching { pause(); flush(); stop(); release() }
        track = null
        if (stopRun) {
            turn?.cancel(); turn = null
            val id = run
            run = null
            if (id != null) CoroutineScope(Dispatchers.IO).launch { runCatching { store.hermes(agent)?.stop(id) } }
        }
        if (_state.value.phase != Phase.ENDED) listen()
    }

    fun approve(choice: String) {
        val request = _state.value.approval ?: return
        _state.update { it.copy(approval = null) }
        scope.launch(Dispatchers.IO) { runCatching { store.hermes(agent)?.approve(request.run, choice, request.requestId) } }
    }

    // -- listening --------------------------------------------------------------------------------------------------

    private fun listen() {
        if (_state.value.phase == Phase.ENDED) return
        _state.update { it.copy(phase = Phase.LISTENING) }
        val r = recognizer ?: SpeechRecognizer.createSpeechRecognizer(context).also { recognizer = it }
        r.setRecognitionListener(object : RecognitionListener {
            override fun onPartialResults(partialResults: Bundle?) {
                partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.let { words ->
                    _state.update { it.copy(heard = words) }
                }
            }
            override fun onResults(results: Bundle?) {
                val said = results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.trim().orEmpty()
                if (said.isEmpty()) listen() else answer(said)
            }
            override fun onError(error: Int) {
                if (_state.value.phase != Phase.LISTENING) return
                // Silence, nothing understood, a busy recognizer: listen again (after a breath for the last two).
                scope.launch {
                    if (error !in setOf(SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT)) delay(600)
                    if (_state.value.phase == Phase.LISTENING) startListening()
                }
            }
            override fun onReadyForSpeech(params: Bundle?) {}
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() {}
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })
        startListening()
    }

    private fun startListening() {
        recognizer?.startListening(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            .putExtra(RecognizerIntent.EXTRA_LANGUAGE, language)
            .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            .putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_COMPLETE_SILENCE_LENGTH_MILLIS, 900L))
    }

    /** Debug builds: as if the person had said `text` (the emulator's microphone hears nothing). */
    fun say(text: String) { recognizer?.cancel(); answer(text) }

    // -- answering --------------------------------------------------------------------------------------------------

    private fun answer(said: String) {
        recognizer?.cancel()
        _state.update { it.copy(phase = Phase.THINKING, heard = said, reply = "", tool = null, error = null) }
        val hermes = store.hermes(agent) ?: return
        val splitter = Sentences()
        // Each turn has its own queue of sentences: the previous one may be closed.
        val queue = Channel<String>(Channel.UNLIMITED)
        sentences = queue
        startSpeaker(queue)
        turn = scope.launch {
            try {
                val id = session ?: withContext(Dispatchers.IO) { hermes.createSession("Live · " + said.take(40)) }
                    .also { store.noteSession(agent, it) }.id
                session = id
                hermes.chatStream(id, said).collect { event ->
                    event.data.str("run_id")?.let { run = it }
                    when (val e = RunEvent.of(event)) {
                        is RunEvent.Delta -> {
                            _state.update { it.copy(reply = it.reply + e.text, tool = null) }
                            splitter.feed(e.text).forEach(queue::trySend)
                        }
                        is RunEvent.Tool -> _state.update { it.copy(tool = if (e.started) e.name else null) }
                        is RunEvent.Approval -> {
                            _state.update { it.copy(approval = e) }
                            queue.trySend(context.getString(io.github.bipagents.R.string.live_needs_approval))
                        }
                        RunEvent.ApprovalResolved -> _state.update { it.copy(approval = null) }
                        is RunEvent.Finished -> if (e.failed) queue.trySend(e.error ?: context.getString(io.github.bipagents.R.string.live_failed))
                            else if (_state.value.reply.isEmpty() && e.output != null) {
                                _state.update { it.copy(reply = e.output) }
                                splitter.feed(e.output).forEach(queue::trySend)
                            }
                        else -> {}
                    }
                    if (event.type == "assistant.completed" && _state.value.reply.isEmpty()) event.data.str("output", "final_response")?.let { out ->
                        _state.update { it.copy(reply = out) }
                        splitter.feed(out).forEach(queue::trySend)
                    }
                }
            } catch (error: Throwable) {
                if (error is kotlinx.coroutines.CancellationException) throw error
                _state.update { it.copy(error = error.message ?: "error") }
            }
            splitter.flush().forEach(queue::trySend)
            run = null
            queue.close()
        }
    }

    /** Speaks the sentences as they come, each one streamed from the bridge into a single audio track. */
    private fun startSpeaker(queue: Channel<String>) {
        val bridge = store.bridge(agent)
        speaker = scope.launch(Dispatchers.IO) {
            val rate = 24000
            val t = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setAudioFormat(AudioFormat.Builder().setSampleRate(rate).setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(AudioTrack.getMinBufferSize(rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT) * 4)
                .setTransferMode(AudioTrack.MODE_STREAM).build()
            track = t
            var written = 0L
            for (sentence in queue) {
                if (bridge == null) break
                val body = buildJsonObject {
                    put("text", JsonPrimitive(sentence)); put("agent", JsonPrimitive(agent.bridgeName)); put("use", JsonPrimitive("live"))
                }
                runCatching {
                    bridge.api.open("v1/tts/stream", body).use { input ->
                        val buffer = ByteArray(4800)
                        var odd: Byte? = null
                        while (isActive) {
                            val n = input.read(buffer)
                            if (n < 0) break
                            if (n == 0) continue
                            var data = buffer.copyOf(n)
                            odd?.let { data = byteArrayOf(it) + data; odd = null }
                            if (data.size % 2 == 1) { odd = data.last(); data = data.copyOf(data.size - 1) }
                            if (t.playState != AudioTrack.PLAYSTATE_PLAYING) {
                                t.play()
                                withContext(Dispatchers.Main) { onSpeaking() }
                            }
                            t.write(data, 0, data.size)
                            written += data.size / 2
                        }
                    }
                }
            }
            // The last words play out, then it's the person's turn.
            while (isActive && written > 0 && t.playbackHeadPosition < written) delay(50)
            t.runCatching { stop(); release() }
            if (track === t) track = null
            withContext(Dispatchers.Main) {
                bargeIn?.cancel(); bargeIn = null
                if (isActive && _state.value.phase != Phase.ENDED) listen()
            }
        }
    }

    private fun onSpeaking() {
        if (_state.value.phase == Phase.SPEAKING || _state.value.phase == Phase.ENDED) return
        _state.update { it.copy(phase = Phase.SPEAKING) }
        watchForVoice()
    }

    @Suppress("MissingPermission")   // checked in start()
    private fun watchForVoice() {
        bargeIn?.cancel()
        bargeIn = scope.launch(Dispatchers.IO) {
            val rate = 16000
            val size = max(AudioRecord.getMinBufferSize(rate, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT), 1280)
            val rec = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, rate, AudioFormat.CHANNEL_IN_MONO,
                AudioFormat.ENCODING_PCM_16BIT, size * 2)
            if (rec.state != AudioRecord.STATE_INITIALIZED) { rec.release(); return@launch }
            val aec = if (AcousticEchoCanceler.isAvailable()) AcousticEchoCanceler.create(rec.audioSessionId)?.apply { enabled = true } else null
            val ns = if (NoiseSuppressor.isAvailable()) NoiseSuppressor.create(rec.audioSessionId)?.apply { enabled = true } else null
            try {
                rec.startRecording()
                val frame = ShortArray(rate / 50)   // 20 ms
                var frames = 0
                var echo = 0.0
                var loud = 0
                while (isActive) {
                    val n = rec.read(frame, 0, frame.size)
                    if (n <= 0) continue
                    var sum = 0.0
                    for (i in 0 until n) sum += frame[i] * frame[i].toDouble()
                    val rms = sqrt(sum / n)
                    frames++
                    if (frames <= 25) { echo = max(echo, rms); continue }   // the first half-second: the Bip's own echo
                    loud = if (rms > max(echo * 2.5, 900.0)) loud + 1 else 0
                    if (loud >= 15) {   // a third of a second of voice
                        withContext(Dispatchers.Main) { interrupt(stopRun = true) }
                        break
                    }
                }
            } finally {
                rec.runCatching { stop(); release() }
                aec?.release(); ns?.release()
            }
        }
    }
}
