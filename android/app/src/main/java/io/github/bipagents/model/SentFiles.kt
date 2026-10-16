package io.github.bipagents.model

import android.content.Context
import io.github.bipagents.net.json
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.builtins.MapSerializer
import kotlinx.serialization.builtins.serializer
import java.io.File
import java.util.UUID

/** A photo or file the person sent, kept on the phone (Hermes only hands the text back). */
@Serializable
data class StoredFile(val kind: String, val name: String, val mime: String, val file: String, val size: Int) {
    val isImage get() = kind == "image"
}

/** The photos and files sent in each conversation (AttachmentStore.swift), so they survive a reload. Entries are matched
 *  back to the history by text, in order. Kept in the app's files (the cache may be cleared). */
class SentFiles(context: Context) {
    @Serializable
    data class Entry(val text: String, val files: List<StoredFile>)

    val folder = File(context.filesDir, "attachments").apply { mkdirs() }
    private val indexFile = File(folder, "index.json")
    private val serializer = MapSerializer(String.serializer(), ListSerializer(Entry.serializer()))
    private val index: MutableMap<String, List<Entry>> =
        runCatching { json.decodeFromString(serializer, indexFile.readText()).toMutableMap() }.getOrDefault(mutableMapOf())

    /** Writes the files of a message being sent. */
    fun write(attachments: List<Attachment>): List<StoredFile> = attachments.mapNotNull { a ->
        val ext = a.name.substringAfterLast('.', "")
        val name = UUID.randomUUID().toString() + if (ext.isEmpty()) "" else ".$ext"
        runCatching { File(folder, name).writeBytes(a.bytes) }.getOrNull() ?: return@mapNotNull null
        StoredFile(if (a.isImage) "image" else "document", a.name, a.mime, name, a.bytes.size)
    }

    @Synchronized
    fun add(session: String, text: String, files: List<StoredFile>) {
        if (files.isEmpty()) return
        index[session] = (index[session] ?: emptyList()) + Entry(text, files)
        runCatching { indexFile.writeText(json.encodeToString(serializer, index)) }
    }

    @Synchronized
    fun entries(session: String): List<Entry> = index[session] ?: emptyList()

    fun file(stored: StoredFile) = File(folder, stored.file)

    companion object {
        /** Letters and digits only, lowercased: Hermes may not hand a text back byte for byte. */
        fun normalized(text: String) = text.lowercase().filter { it.isLetterOrDigit() }
    }
}
