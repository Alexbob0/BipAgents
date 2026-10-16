package io.github.bipagents.voice

import java.io.File
import java.io.RandomAccessFile

/** A 16-bit mono WAV written as the audio arrives; the sizes in its header are filled in at the end. */
class WavWriter(val file: File, private val rate: Int) {
    private val out = RandomAccessFile(file.apply { parentFile?.mkdirs() }, "rw").apply { setLength(0); write(ByteArray(44)) }
    var bytes = 0L
        private set

    fun append(data: ByteArray, length: Int = data.size) {
        out.write(data, 0, length)
        bytes += length
    }

    /** Seconds of audio written. */
    val seconds get() = bytes / 2.0 / rate

    fun close() {
        out.seek(0)
        out.write(header(bytes, rate))
        out.close()
    }

    companion object {
        fun header(dataBytes: Long, rate: Int): ByteArray {
            val b = java.nio.ByteBuffer.allocate(44).order(java.nio.ByteOrder.LITTLE_ENDIAN)
            b.put("RIFF".toByteArray()).putInt((36 + dataBytes).toInt()).put("WAVE".toByteArray())
            b.put("fmt ".toByteArray()).putInt(16).putShort(1).putShort(1).putInt(rate).putInt(rate * 2).putShort(2).putShort(16)
            b.put("data".toByteArray()).putInt(dataBytes.toInt())
            return b.array()
        }
    }
}
