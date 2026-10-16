package io.github.bipagents.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Build
import android.os.Bundle
import android.os.ParcelFileDescriptor
import android.speech.RecognitionListener
import android.speech.RecognitionSupport
import android.speech.RecognitionSupportCallback
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import java.io.File
import java.io.FileOutputStream
import java.util.Locale
import kotlin.math.abs
import kotlin.math.min

/** A voice note: what was said, the recording (none when the phone couldn't keep it), its length and its loudness over
 *  time (the bubble's waveform). */
data class RecordedNote(val text: String, val file: File?, val seconds: Double, val levels: List<Float>)

/**
 * Push-to-talk (Dictation.swift). Two ways, picked per phone and language:
 * - **on the device, recording kept** (Android 13+, the language installed for on-device recognition): the app records
 *   the microphone to a WAV and feeds the same audio to the recognizer, so the note can be heard again;
 * - **the phone's recognizer listening itself** otherwise (it works everywhere): it restarts after each pause while the
 *   button is held; the note is text only, and the on-device language is downloaded for next time.
 * The recording never waits for the recognizer (a queue in between), so a recognizer that gives up can't block it.
 */
class Recorder(private val context: Context) {
    private val _partial = MutableStateFlow("")
    val partial: StateFlow<String> = _partial.asStateFlow()
    private val _level = MutableStateFlow(0f)
    val level: StateFlow<Float> = _level.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var record: AudioRecord? = null
    private var recognizer: SpeechRecognizer? = null
    private var pump: Job? = null
    private var writer: Job? = null
    private var feed: Channel<ByteArray>? = null
    private var pipe: ParcelFileDescriptor? = null
    /** The recognizer's end of the pipe: kept open until it's done (it's handed over asynchronously). */
    private var source: ParcelFileDescriptor? = null
    private var wav: WavWriter? = null
    private var tag = "en-US"
    private val levels = mutableListOf<Float>()
    private var live: Session? = null
    private var started = 0L

    val allowed get() = context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    /** One recognition: what's been understood, and when it's over. `listening`: the mic mode, restarted after pauses. */
    private inner class Session(val recordedAudio: Boolean) : RecognitionListener {
        val text = StringBuilder()
        var holding = true
        val done = CompletableDeferred<Unit>()
        private var current = ""
        private fun show() { _partial.value = (text.toString() + " " + current).trim() }
        override fun onPartialResults(partialResults: Bundle?) {
            current = partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull() ?: return
            show()
        }
        override fun onSegmentResults(segmentResults: Bundle) {
            segmentResults.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.let { text.append(' ').append(it) }
            current = ""
            show()
        }
        override fun onEndOfSegmentedSession() { done.complete(Unit) }
        override fun onResults(results: Bundle?) {
            results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.let { text.append(' ').append(it) }
            current = ""
            show()
            if (!recordedAudio) next()
        }
        override fun onError(error: Int) {
            if (!recordedAudio && holding && error in setOf(SpeechRecognizer.ERROR_NO_MATCH, SpeechRecognizer.ERROR_SPEECH_TIMEOUT)) next()
            else { done.complete(Unit); if (recordedAudio) stopFeeding() }
        }
        /** Mic mode: a pause ended one recognition; while the button is held, the next one starts. */
        private fun next() {
            if (holding) recognizer?.startListening(intent()) else done.complete(Unit)
        }
        override fun onReadyForSpeech(params: Bundle?) {}
        override fun onBeginningOfSpeech() {}
        override fun onRmsChanged(rmsdB: Float) {
            if (!recordedAudio) {
                val level = ((rmsdB + 2) / 12).coerceIn(0f, 1f)
                _level.value = level
                if (levels.size < 600) levels += level
            }
        }
        override fun onBufferReceived(buffer: ByteArray?) {}
        override fun onEndOfSpeech() {}
        override fun onEvent(eventType: Int, params: Bundle?) {}
        val result get() = text.toString().trim().ifEmpty { current.trim() }
    }

    private fun intent(): Intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
        .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
        .putExtra(RecognizerIntent.EXTRA_LANGUAGE, tag)
        .putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
        .putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_COMPLETE_SILENCE_LENGTH_MILLIS, 10_000L)
        .putExtra(RecognizerIntent.EXTRA_SPEECH_INPUT_POSSIBLY_COMPLETE_SILENCE_LENGTH_MILLIS, 10_000L)

    /** Whether this phone recognizes `tag` on the device already (Android 13+); if it could but hasn't the language,
     *  the download starts for next time. */
    private suspend fun installedOnDevice(): Boolean {
        if (Build.VERSION.SDK_INT < 33 || !SpeechRecognizer.isOnDeviceRecognitionAvailable(context)) return false
        val support = CompletableDeferred<RecognitionSupport?>()
        val probe = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
        probe.checkRecognitionSupport(intent(), context.mainExecutor, object : RecognitionSupportCallback {
            override fun onSupportResult(recognitionSupport: RecognitionSupport) { support.complete(recognitionSupport) }
            override fun onError(error: Int) { support.complete(null) }
        })
        val result = withTimeoutOrNull(2000) { support.await() }
        val installed = result?.installedOnDeviceLanguages?.any { it.equals(tag, true) } == true
        // Offered once per language (Android asks the person; the pack is ~60 MB).
        val prefs = context.getSharedPreferences("settings", Context.MODE_PRIVATE)
        if (!installed && result?.supportedOnDeviceLanguages?.any { it.equals(tag, true) } == true && !prefs.getBoolean("offered-$tag", false)) {
            prefs.edit().putBoolean("offered-$tag", true).apply()
            runCatching { probe.triggerModelDownload(intent()) }
        }
        probe.destroy()
        return installed
    }

    /** Starts listening, in the agent's language (`fr`, `en`…). False when the mic or the recognizer is unavailable. */
    suspend fun start(language: String?): Boolean = withContext(Dispatchers.Main) {
        if (!allowed || !SpeechRecognizer.isRecognitionAvailable(context)) return@withContext false
        _partial.value = ""
        levels.clear()
        started = System.currentTimeMillis()
        tag = when (language ?: Locale.getDefault().language) {
            "fr" -> "fr-FR"; "es" -> "es-ES"; "de" -> "de-DE"; "en" -> "en-US"; else -> Locale.getDefault().toLanguageTag()
        }
        if (Build.VERSION.SDK_INT >= 33 && installedOnDevice()) {
            val session = Session(recordedAudio = true)
            val r = SpeechRecognizer.createOnDeviceSpeechRecognizer(context)
            r.setRecognitionListener(session)
            recognizer = r
            live = session
            val (read, write) = ParcelFileDescriptor.createPipe()
            pipe = write
            source = read
            r.startListening(intent()
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE, read)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_CHANNEL_COUNT, 1)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_ENCODING, AudioFormat.ENCODING_PCM_16BIT)
                .putExtra(RecognizerIntent.EXTRA_AUDIO_SOURCE_SAMPLING_RATE, RATE)
                .putExtra(RecognizerIntent.EXTRA_SEGMENTED_SESSION, RecognizerIntent.EXTRA_AUDIO_SOURCE))
            if (startRecording()) return@withContext true
            cancel()
        }
        val session = Session(recordedAudio = false)
        val r = SpeechRecognizer.createSpeechRecognizer(context)
        r.setRecognitionListener(session)
        recognizer = r
        live = session
        r.startListening(intent())
        true
    }

    @Suppress("MissingPermission")   // checked in start()
    private fun startRecording(): Boolean {
        val size = maxOf(AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT), 4096)
        val rec = AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, RATE, AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT, size * 2)
        if (rec.state != AudioRecord.STATE_INITIALIZED) { rec.release(); return false }
        record = rec
        val file = WavWriter(File(context.filesDir, "notes/note-${System.currentTimeMillis()}.wav"), RATE)
        wav = file
        // The recognizer gets its copy through a queue: if it stops reading, the recording goes on.
        val queue = Channel<ByteArray>(capacity = 256, onBufferOverflow = BufferOverflow.DROP_OLDEST)
        feed = queue
        val out = FileOutputStream(pipe!!.fileDescriptor)
        writer = scope.launch { for (chunk in queue) if (runCatching { out.write(chunk) }.isFailure) break }
        rec.startRecording()
        pump = scope.launch {
            val buffer = ByteArray(size)
            var window = 0f
            var count = 0
            while (isActive) {
                val n = rec.read(buffer, 0, buffer.size)
                if (n < 0) break
                if (n == 0) continue
                file.append(buffer, n)
                queue.trySend(buffer.copyOf(n))
                // Loudness: the mean of the samples' size, every ~200 ms.
                var sum = 0L
                for (i in 0 until n / 2) sum += abs((buffer[2 * i].toInt() and 0xff) or (buffer[2 * i + 1].toInt() shl 8))
                window += sum.toFloat() / (n / 2) / 32768f
                count++
                if (count * n >= RATE / 5) {
                    val level = min(1f, window / count * 6f)
                    levels += level
                    _level.value = level
                    window = 0f; count = 0
                }
            }
        }
        return true
    }

    /** No more audio for the recognizer. With `trailing`, a second and a half of silence first: recognizers lag behind
     *  the audio and finalize what they have when it ends. */
    private fun stopFeeding(trailing: Boolean = false) {
        val queue = feed
        feed = null
        val p = pipe
        pipe = null
        scope.launch {
            if (trailing && queue != null) repeat(15) { queue.trySend(ByteArray(RATE / 5)); delay(100) }
            queue?.close()
            writer?.join()
            p?.runCatching { close() }
        }
    }

    /** Ends the note: the transcript (and recording) once the recognizer has finished. Null when nothing was said. */
    suspend fun stop(): RecordedNote? {
        val session = live ?: return null
        live = null
        session.holding = false
        record?.runCatching { stop() }   // the pump's read returns: it ends
        withTimeoutOrNull(1000) { pump?.join() }
        pump?.cancel()
        record?.runCatching { release() }
        record = null
        val file = wav
        wav = null
        withContext(Dispatchers.IO) { file?.close() }
        if (file != null) stopFeeding(trailing = true) else withContext(Dispatchers.Main) { recognizer?.stopListening() }
        withTimeoutOrNull(if (file != null) 8000 else 4000) { session.done.await() }
        val text = session.result
        withContext(Dispatchers.Main) { recognizer?.destroy(); recognizer = null }
        source?.runCatching { close() }
        source = null
        if (text.isBlank()) { file?.file?.delete(); return null }
        val seconds = file?.seconds ?: ((System.currentTimeMillis() - started) / 1000.0)
        return RecordedNote(text, file?.file, seconds, levels.toList())
    }

    fun cancel() {
        live?.holding = false
        live = null
        record?.runCatching { stop(); release() }
        record = null
        pump?.cancel()
        wav?.let { it.close(); it.file.delete() }
        wav = null
        stopFeeding()
        recognizer?.runCatching { cancel(); destroy() }
        recognizer = null
        source?.runCatching { close() }
        source = null
    }

    companion object { const val RATE = 16000 }
}
