package io.github.bipagents.voice

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaPlayer
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File
import java.io.InputStream

/** One voice at a time: a reply streamed from the bridge (PCM, played as it arrives and kept as a file), or a file
 *  (a voice note, a reply heard before). `playing` is the key of what plays, for the buttons that show it. */
object Speaker {
    enum class Phase { PREPARING, PLAYING }
    data class Now(val key: String, val phase: Phase)

    private val _now = MutableStateFlow<Now?>(null)
    val now: StateFlow<Now?> = _now.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var job: Job? = null
    private var player: MediaPlayer? = null

    fun stop() {
        PhoneVoice.stop()
        job?.cancel()
        job = null
        player?.runCatching { stop(); release() }
        player = null
        _now.value = null
    }

    /** Reads `text` with the phone's own voice (an agent whose voice is the phone's). */
    fun system(context: android.content.Context, key: String, text: String, language: String?, onDone: () -> Unit = {}) {
        stop()
        AudioPlayer.pause()
        _now.value = Now(key, Phase.PLAYING)
        job = scope.launch {
            PhoneVoice.say(context, text, language)
            if (_now.value?.key == key) { _now.value = null; onDone() }
        }
    }

    /** Plays `file`; `onDone` when it ends (not when stopped). */
    fun play(key: String, file: File, onDone: () -> Unit = {}) {
        stop()
        AudioPlayer.pause()
        val p = MediaPlayer()
        player = p
        runCatching {
            p.setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            p.setDataSource(file.path)
            p.setOnCompletionListener { if (player === p) { p.release(); player = null; _now.value = null; onDone() } }
            p.prepare()
            p.start()
            _now.value = Now(key, Phase.PLAYING)
        }.onFailure { stop() }
    }

    /**
     * Plays raw 24 kHz mono s16le audio as `open()` delivers it, and keeps it in `cache` (a WAV) for next time.
     * Throws (to the caller's `onError`) when the stream can't start, so the caller can fall back.
     */
    fun stream(key: String, cache: File, rate: Int = 24000, open: suspend () -> InputStream, onError: (Throwable) -> Unit = {}) {
        AudioPlayer.pause()
        stop()
        _now.value = Now(key, Phase.PREPARING)
        job = scope.launch {
            val size = AudioTrack.getMinBufferSize(rate, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT)
            val track = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setAudioFormat(AudioFormat.Builder().setSampleRate(rate).setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(size * 4).setTransferMode(AudioTrack.MODE_STREAM).build()
            val partial = File(cache.path + ".part")
            val wav = WavWriter(partial, rate)
            try {
                open().use { input ->
                    val buffer = ByteArray(4096)
                    var odd: Byte? = null
                    while (isActive) {
                        val n = input.read(buffer)
                        if (n < 0) break
                        if (n == 0) continue
                        if (_now.value?.phase != Phase.PLAYING) { track.play(); _now.value = Now(key, Phase.PLAYING) }
                        // Samples are 2 bytes; a read may split one.
                        var data = buffer.copyOf(n)
                        odd?.let { data = byteArrayOf(it) + data; odd = null }
                        if (data.size % 2 == 1) { odd = data.last(); data = data.copyOf(data.size - 1) }
                        track.write(data, 0, data.size)
                        wav.append(data)
                    }
                }
                wav.close()
                if (isActive && wav.bytes > 0) {
                    partial.renameTo(cache)
                    // Let the end of the buffer play out before saying it's over.
                    val frames = wav.bytes / 2
                    while (isActive && track.playbackHeadPosition < frames) kotlinx.coroutines.delay(50)
                }
            } catch (error: Throwable) {
                wav.runCatching { close() }
                partial.delete()
                if (error !is kotlinx.coroutines.CancellationException) onError(error)
            } finally {
                partial.delete()
                track.runCatching { stop(); release() }
                if (_now.value?.key == key) _now.value = null
            }
        }
    }
}
