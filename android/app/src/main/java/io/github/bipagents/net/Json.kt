package io.github.bipagents.net

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull

// Lenient reading of Hermes' and the bridge's JSON, like HermesKit's LenientFields: the first of several keys that is
// there, numbers or strings alike. Hermes' shapes are not all documented, so nothing here throws.

val json = Json { ignoreUnknownKeys = true; explicitNulls = false; encodeDefaults = true }

fun parseJson(text: String): JsonElement? = runCatching { json.parseToJsonElement(text) }.getOrNull()

fun JsonElement?.obj(): JsonObject? = this as? JsonObject
fun JsonElement?.arr(): JsonArray? = this as? JsonArray

/** A string, also from a number or a boolean; null when missing, null or empty. */
fun JsonElement?.text(): String? = when (this) {
    is JsonPrimitive -> if (this is JsonNull) null else content.takeIf { it.isNotEmpty() }
    else -> null
}

fun JsonObject?.str(vararg keys: String): String? = keys.firstNotNullOfOrNull { this?.get(it).text() }
fun JsonObject?.bool(vararg keys: String): Boolean? =
    keys.firstNotNullOfOrNull { (this?.get(it) as? JsonPrimitive)?.let { p -> p.booleanOrNull ?: p.content.lowercase().toBooleanStrictOrNull() } }
fun JsonObject?.long(vararg keys: String): Long? =
    keys.firstNotNullOfOrNull { (this?.get(it) as? JsonPrimitive)?.let { p -> p.longOrNull ?: p.doubleOrNull?.toLong() } }
fun JsonObject?.double(vararg keys: String): Double? =
    keys.firstNotNullOfOrNull { (this?.get(it) as? JsonPrimitive)?.doubleOrNull }

/** A list either at the top level or wrapped (`{"sessions": [...]}`, `{"data": [...]}`…). */
fun JsonElement?.list(vararg keys: String): List<JsonObject> {
    val array = this.arr() ?: (keys.toList() + listOf("data", "items", "results")).firstNotNullOfOrNull { this.obj()?.get(it).arr() }
    return array?.mapNotNull { it.obj() } ?: emptyList()
}

/** Epoch milliseconds from an ISO 8601 string or epoch seconds / milliseconds. */
fun JsonObject?.time(vararg keys: String): Long? {
    for (key in keys) {
        val value = this?.get(key) as? JsonPrimitive ?: continue
        value.doubleOrNull?.let { return if (it < 1e12) (it * 1000).toLong() else it.toLong() }
        runCatching { java.time.OffsetDateTime.parse(value.content).toInstant().toEpochMilli() }.getOrNull()?.let { return it }
        runCatching { java.time.Instant.parse(value.content).toEpochMilli() }.getOrNull()?.let { return it }
        runCatching { java.time.LocalDateTime.parse(value.content).toInstant(java.time.ZoneOffset.UTC).toEpochMilli() }.getOrNull()?.let { return it }
    }
    return null
}
