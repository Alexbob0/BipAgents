package io.github.bipagents.voice

import android.content.Context
import android.speech.tts.TextToSpeech
import android.speech.tts.UtteranceProgressListener
import kotlinx.coroutines.CompletableDeferred
import java.util.Locale
import java.util.UUID

/** The phone's own voice (« Voix de l'iPhone »): Android's text-to-speech, no server involved. Picked as an agent's
 *  voice, it reads « Listen », the spoken replies and the Live. */
object PhoneVoice {
    const val ID = "system"

    private var tts: TextToSpeech? = null
    private var ready = CompletableDeferred<Boolean>()
    private val done = HashMap<String, CompletableDeferred<Unit>>()

    private fun engine(context: Context): TextToSpeech = tts ?: TextToSpeech(context.applicationContext) { status ->
        ready.complete(status == TextToSpeech.SUCCESS)
    }.also { t ->
        tts = t
        t.setOnUtteranceProgressListener(object : UtteranceProgressListener() {
            override fun onStart(id: String?) {}
            override fun onDone(id: String?) { id?.let { synchronized(done) { done.remove(it) }?.complete(Unit) } }
            @Deprecated("Deprecated in Java") override fun onError(id: String?) { onDone(id) }
            override fun onStop(id: String?, interrupted: Boolean) { onDone(id) }
        })
    }

    /** Says `text` in `language` and returns once it's said (or stopped). */
    suspend fun say(context: Context, text: String, language: String?, voiceCall: Boolean = false) {
        val t = engine(context)
        if (!ready.await()) return
        language?.let { t.language = Locale.forLanguageTag(it) }
        t.setAudioAttributes(android.media.AudioAttributes.Builder()
            .setUsage(if (voiceCall) android.media.AudioAttributes.USAGE_VOICE_COMMUNICATION else android.media.AudioAttributes.USAGE_MEDIA)
            .setContentType(android.media.AudioAttributes.CONTENT_TYPE_SPEECH).build())
        val id = UUID.randomUUID().toString()
        val finished = CompletableDeferred<Unit>()
        synchronized(done) { done[id] = finished }
        // Long texts in pieces under the engine's limit.
        val max = TextToSpeech.getMaxSpeechInputLength() - 100
        val parts = text.chunked(max)
        val queued = parts.mapIndexed { i, part -> t.speak(part, TextToSpeech.QUEUE_ADD, null, if (i == parts.lastIndex) id else "$id-$i") }
        if (queued.any { it == TextToSpeech.ERROR }) { reset(); return }   // the engine went away: a new one next time
        // At most the time it would take to say it (an engine that dies never says it's done).
        try { kotlinx.coroutines.withTimeoutOrNull(5_000L + 150L * text.length) { finished.await() } ?: reset() }
        finally { synchronized(done) { done.remove(id) } }
    }

    private fun reset() {
        runCatching { tts?.shutdown() }
        tts = null
        ready = CompletableDeferred()
    }

    fun stop() {
        tts?.stop()
        synchronized(done) { done.values.toList().also { done.clear() } }.forEach { it.complete(Unit) }
    }
}
