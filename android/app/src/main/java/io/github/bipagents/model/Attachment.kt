package io.github.bipagents.model

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.provider.OpenableColumns
import java.io.ByteArrayOutputStream
import kotlin.math.max

/** A photo or a file to send with a message (HermesKit's Attachment): photos go inline to the model, files through
 *  the bridge (`POST /v1/files`) into a folder the agent reads. */
data class Attachment(val name: String, val mime: String, val bytes: ByteArray, val isImage: Boolean) {
    override fun equals(other: Any?) = other is Attachment && other.name == name && other.bytes.contentEquals(bytes)
    override fun hashCode() = name.hashCode() * 31 + bytes.contentHashCode()

    companion object {
        /** Longest side of a photo sent to the model (Composer.swift's JPEG re-encoding). */
        private const val MAX_SIDE = 1600

        fun read(context: Context, uri: Uri): Attachment? {
            val resolver = context.contentResolver
            val mime = resolver.getType(uri) ?: "application/octet-stream"
            val name = resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(0) else null
            } ?: uri.lastPathSegment ?: "file"
            val bytes = resolver.openInputStream(uri)?.use { it.readBytes() } ?: return null
            if (!mime.startsWith("image/") || mime == "image/gif") return Attachment(name, mime, bytes, false)
            val bitmap = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return Attachment(name, mime, bytes, false)
            val scale = MAX_SIDE.toFloat() / max(bitmap.width, bitmap.height)
            val sized = if (scale < 1) Bitmap.createScaledBitmap(bitmap, (bitmap.width * scale).toInt(), (bitmap.height * scale).toInt(), true) else bitmap
            val jpeg = ByteArrayOutputStream().also { sized.compress(Bitmap.CompressFormat.JPEG, 85, it) }.toByteArray()
            return Attachment(name.substringBeforeLast('.') + ".jpg", "image/jpeg", jpeg, true)
        }

        /** `[Pièce jointe : rapport.pdf (application/pdf, 1,2 Mo) → /home/hermes/…]` (MessageInput.referenceLine). */
        fun referenceLine(name: String, mime: String, size: Int, path: String) = "[Pièce jointe : $name ($mime, ${formatSize(size)}) → $path]"

        /** French byte count, decimal units: `512 o`, `12 Ko`, `1,2 Mo`. */
        fun formatSize(bytes: Int): String {
            val units = listOf("o", "Ko", "Mo", "Go")
            var value = bytes.toDouble()
            var unit = 0
            while (value >= 1000 && unit < units.size - 1) { value /= 1000; unit++ }
            if (unit == 0) return "$bytes o"
            val rounded = Math.round(value * 10) / 10.0
            val number = if (rounded == Math.rint(rounded) || rounded >= 100) Math.round(rounded).toString()
                else String.format(java.util.Locale.ROOT, "%.1f", rounded).replace('.', ',')
            return "$number ${units[unit]}"
        }
    }
}
