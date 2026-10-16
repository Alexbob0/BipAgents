package io.github.bipagents.ui

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioTrack
import io.github.bipagents.model.Look
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import java.text.Normalizer
import kotlin.math.PI
import kotlin.math.exp
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin

/**
 * The Bips' « Animalese » (BipBabble.swift, docs/play): each letter becomes a tiny syllable coloured by its vowel,
 * pitched per Bip and per mood. A few harmonics shaped like vowel formants: no model, no network. Silent when the
 * phone is, or when « Sons des Bips » is off.
 */
object Babble {
    private const val RATE = 24000
    private val vowels = mapOf('a' to (800.0 to 1250.0), 'e' to (480.0 to 1850.0), 'i' to (300.0 to 2300.0),
        'o' to (520.0 to 900.0), 'u' to (330.0 to 850.0), 'y' to (300.0 to 2100.0))
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    fun enabled(context: Context) = context.getSharedPreferences("settings", Context.MODE_PRIVATE).getBoolean("bipSounds", true)
    fun setEnabled(context: Context, on: Boolean) =
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putBoolean("bipSounds", on).apply()

    private data class Voice(var base: Double, var syl: Double = 0.055, var spread: Double = 2.5, var drift: Double = 0.0,
                             var bounce: Double = 0.0, var vol: Double = 0.32)

    private fun voice(look: Look, mood: Mood) = Voice(look.category.pitch).apply {
        when (mood) {
            Mood.GIGGLING -> { base *= 1.25; syl = 0.045; bounce = 3.0 }
            Mood.SURPRISED -> { base *= 1.15; drift = 5.0 }
            Mood.CONTENT -> { base *= 0.92; syl = 0.075; spread = 1.2; vol = 0.25 }
            Mood.DIZZY -> { syl = 0.07; drift = -7.0; spread = 3.5 }
            Mood.GRUMPY -> { base *= 0.72; syl = 0.065; spread = 1.0; drift = -2.0 }
            Mood.WINKING -> drift = 2.0
            else -> {}
        }
    }

    fun say(context: Context, text: String, look: Look, mood: Mood) {
        if (!enabled(context)) return
        if (context.getSystemService(AudioManager::class.java).ringerMode != AudioManager.RINGER_MODE_NORMAL) return
        scope.launch {
            val samples = synthesize(text, look, mood) ?: return@launch
            val track = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_ASSISTANCE_SONIFICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build())
                .setAudioFormat(AudioFormat.Builder().setSampleRate(RATE).setChannelMask(AudioFormat.CHANNEL_OUT_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT).build())
                .setBufferSizeInBytes(samples.size * 2).setTransferMode(AudioTrack.MODE_STATIC).build()
            track.write(samples, 0, samples.size)
            track.play()
            kotlinx.coroutines.delay(samples.size * 1000L / RATE + 200)
            track.release()
        }
    }

    private fun synthesize(text: String, look: Look, mood: Mood): ShortArray? {
        val v = voice(look, mood)
        val letters = Normalizer.normalize(text.lowercase(), Normalizer.Form.NFD).replace(Regex("\\p{Mn}+"), "").toList()
        val sayable = letters.filter { it.isLetter() }
        if (sayable.isEmpty()) return null
        val ending = letters.lastOrNull { !it.isWhitespace() && it != '♡' }
        val drift = v.drift + when (ending) { '?' -> 4.0; '!' -> 2.0; '…' -> -3.0; else -> 0.0 }
        val out = ArrayList<FloatArray>()
        var spoken = 0
        var last = vowels.getValue('a')
        for (ch in letters) {
            if (ch.isWhitespace() || ch == ',') { out += FloatArray((RATE * v.syl * 0.8).toInt()); continue }
            if (!ch.isLetter()) continue
            val progress = if (sayable.size > 1) spoken.toDouble() / (sayable.size - 1) else 0.0
            val semis = ((ch.code % 5) - 2) / 2.0 * v.spread + drift * progress + (if (spoken % 2 == 0) v.bounce else -v.bounce / 2)
            val f0 = v.base * 2.0.pow(semis / 12)
            vowels[ch]?.let { last = it }
            val onset = when (ch) { in "bdgkpqt" -> 'p'; in "cfhjsvxz" -> 'f'; else -> ' ' }
            out += syllable(f0, vowels[ch] ?: last, v.syl, onset, v.vol)
            spoken++
        }
        val total = out.sumOf { it.size }
        val samples = ShortArray(total)
        var at = 0
        for (part in out) for (x in part) samples[at++] = (x.coerceIn(-1f, 1f) * 32767).toInt().toShort()
        return samples
    }

    private fun syllable(f0: Double, formants: Pair<Double, Double>, dur: Double, onset: Char, vol: Double): FloatArray {
        val n = (RATE * dur).toInt()
        val (f1, f2) = formants
        val harmonics = ArrayList<Pair<Int, Double>>()
        var k = 1
        while (k * f0 < min(5000.0, RATE / 2.2)) {
            val f = k * f0
            val shape = exp(-((f - f1) / 260).pow(2)) + 0.7 * exp(-((f - f2) / 320).pow(2)) + 0.06
            harmonics += k to shape / k.toDouble().pow(0.6)
            k++
        }
        val norm = harmonics.sumOf { it.second }
        val attack = RATE * 0.004
        val release = RATE * 0.018
        val onsetLen = when (onset) { 'p' -> (RATE * 0.006).toInt(); 'f' -> (RATE * 0.014).toInt(); else -> 0 }
        val out = FloatArray(n)
        var phase = 0.0
        for (i in 0 until n) {
            phase += 2 * PI * f0 * (1 + 0.04 * i / n) / RATE
            var value = 0.0
            for ((h, a) in harmonics) value += a * sin(h * phase)
            value /= norm
            var sample = value * min(1.0, i / attack) * min(1.0, (n - i) / release)
            if (i < onsetLen) {
                val burst = (Math.random() * 2 - 1) * (if (onset == 'p') 0.5 else 0.22)
                sample = sample * i / onsetLen + burst * (1 - i.toDouble() / onsetLen)
            }
            out[i] = (sample * vol).toFloat()
        }
        return out
    }
}
