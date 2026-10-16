package io.github.bipagents.voice

import android.content.Context
import android.media.AudioDeviceInfo
import android.media.AudioManager
import io.github.bipagents.net.json
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import java.io.File
import java.text.Normalizer

/** The person's voice notes, kept with their conversation (VoiceNoteStore.swift): Hermes keeps only the text, the
 *  app finds the recording again by that text. */
class VoiceNotes(context: Context) {
    @Serializable
    data class Note(val session: String, val text: String, val path: String?, val seconds: Double, val levels: List<Float>) {
        val audio: File? get() = path?.let(::File)?.takeIf { it.exists() }
    }

    private val prefs = context.getSharedPreferences("voice-notes", Context.MODE_PRIVATE)
    private val serializer = ListSerializer(Note.serializer())
    private var notes: List<Note> = prefs.getString("list", null)?.let { runCatching { json.decodeFromString(serializer, it) }.getOrNull() } ?: emptyList()

    fun add(session: String, note: RecordedNote) {
        notes = (notes + Note(session, note.text.trim(), note.file?.takeIf { it.exists() }?.path, note.seconds, note.levels)).takeLast(300)
        prefs.edit().putString("list", json.encodeToString(serializer, notes)).apply()
    }

    fun find(session: String?, text: String): Note? =
        notes.lastOrNull { it.session == session && it.text == text.trim() }
}

/** When a voice note gets a spoken answer (VoiceReplyPolicy.swift). */
enum class VoiceReply { NEVER, SMART, ALWAYS;

    companion object {
        fun current(context: Context): VoiceReply =
            runCatching { valueOf(context.getSharedPreferences("settings", Context.MODE_PRIVATE).getString("voiceReply", "SMART")!!) }.getOrDefault(SMART)

        fun set(context: Context, value: VoiceReply) =
            context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putString("voiceReply", value.name).apply()

        fun wanted(context: Context, transcript: String): Boolean = when (current(context)) {
            NEVER -> false
            ALWAYS -> true
            SMART -> asksForVoice(transcript) || handsFree(context)
        }

        private val cues = listOf(
            "en vocal", "un vocal", "message vocal", "a l'oral", "a voix haute", "lis le moi", "lis-le moi", "lis la moi",
            "lis-la moi", "lis moi", "lis-moi", "dis le moi", "dis-le moi", "reponds moi a l'oral", "en audio", "audio stp",
            "audio s'il te plait",
            "voice message", "voice note", "by voice", "read it to me", "read it out", "read me this", "read me the", "out loud", "aloud",
            "as audio", "in audio", "audio please",
            "por voz", "mensaje de voz", "nota de voz", "en voz alta", "leemelo", "leemela", "audio por favor", "en un audio",
            "sprachnachricht", "sprachmemo", "vorlesen", "lies es mir vor", "lies mir", "laut vor", "als audio", "per audio",
            "audio bitte", "per sprache")

        fun asksForVoice(text: String): Boolean {
            val folded = Normalizer.normalize(text.lowercase(), Normalizer.Form.NFD).replace(Regex("\\p{Mn}+"), "").replace('’', '\'')
            return cues.any { it in folded }
        }

        /** Headphones, a car or a Bluetooth speaker: the answer is better heard than read. */
        fun handsFree(context: Context): Boolean {
            val outputs = context.getSystemService(AudioManager::class.java).getDevices(AudioManager.GET_DEVICES_OUTPUTS)
            val kinds = setOf(AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_BLUETOOTH_A2DP,
                AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_BLE_SPEAKER,
                AudioDeviceInfo.TYPE_USB_HEADSET)
            return outputs.any { it.type in kinds }
        }
    }
}
