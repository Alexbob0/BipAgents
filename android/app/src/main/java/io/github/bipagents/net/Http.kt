package io.github.bipagents.net

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/** A server's refusal (`status`), or no answer at all (`status` 0). */
class HttpError(val status: Int, message: String?) : IOException(message ?: "HTTP $status") {
    val unreachable get() = status == 0
}

object Http {
    val client: OkHttpClient = OkHttpClient.Builder()
        .connectTimeout(15, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .build()

    /** For event streams: the server sends a keepalive every 10 s, so 45 s of silence means a dead link. */
    val streaming: OkHttpClient = client.newBuilder().readTimeout(45, TimeUnit.SECONDS).build()

    private val JSON = "application/json".toMediaType()

    fun body(value: JsonElement): RequestBody = json.encodeToString(JsonElement.serializer(), value).toRequestBody(JSON)
}

/** One HTTP API with a base address and a bearer key (an agent's Hermes, or a bridge). */
class Api(val base: String, private val key: String) {
    fun url(path: String, query: Map<String, String?> = emptyMap()): HttpUrl {
        val builder = (base.trimEnd('/') + "/" + path.trimStart('/')).toHttpUrl().newBuilder()
        query.forEach { (k, v) -> if (v != null) builder.addQueryParameter(k, v) }
        return builder.build()
    }

    fun request(path: String, method: String = "GET", body: RequestBody? = null, query: Map<String, String?> = emptyMap(),
                headers: Map<String, String> = emptyMap()): Request {
        val builder = Request.Builder().url(url(path, query)).header("Authorization", "Bearer $key")
        headers.forEach { (k, v) -> builder.header(k, v) }
        val payload = body ?: if (method == "GET" || method == "DELETE") null else "{}".toRequestBody("application/json".toMediaType())
        return builder.method(method, payload).build()
    }

    suspend fun bytes(path: String, method: String = "GET", body: RequestBody? = null, query: Map<String, String?> = emptyMap(),
                      headers: Map<String, String> = emptyMap()): ByteArray = withContext(Dispatchers.IO) {
        // The body is read here, off the main thread (Android refuses network reads there).
        Http.client.newCall(request(path, method, body, query, headers)).await().use { response ->
            if (!response.isSuccessful) throw failure(response)
            response.body.bytes()
        }
    }

    /** A streamed body (the bridge's PCM speech), read as it arrives; closing the stream ends the call. */
    suspend fun open(path: String, body: JsonElement): java.io.InputStream = withContext(Dispatchers.IO) {
        val response = Http.streaming.newCall(request(path, "POST", Http.body(body))).execute()
        if (!response.isSuccessful) { val error = failure(response); response.close(); throw error }
        object : java.io.FilterInputStream(response.body.byteStream()) {
            override fun close() { super.close(); response.close() }
        }
    }

    suspend fun call(path: String, method: String = "GET", body: JsonElement? = null, query: Map<String, String?> = emptyMap(),
                     headers: Map<String, String> = emptyMap()): JsonElement? {
        val bytes = bytes(path, method, body?.let { Http.body(it) }, query, headers)
        return if (bytes.isEmpty()) null else parseJson(bytes.decodeToString())
    }

    /** An event stream (SSE): each event's type (a dotted name wins over a plain one) and its JSON. */
    fun events(path: String, query: Map<String, String?> = emptyMap(), body: JsonElement? = null): Flow<ServerEvent> = callbackFlow {
        val call = Http.streaming.newCall(request(path, if (body == null) "GET" else "POST", body?.let { Http.body(it) }, query,
            mapOf("Accept" to "text/event-stream")))
        val reader = launch(Dispatchers.IO) {
            try {
                call.execute().use { response ->
                    if (!response.isSuccessful) throw failure(response)
                    val source = response.body.source()
                    var name: String? = null
                    val data = StringBuilder()
                    while (true) {
                        val line = source.readUtf8Line() ?: break
                        if (line.isEmpty()) {
                            // Waits for a slow reader rather than dropping events (a long reply's deltas).
                            ServerEvent.of(name, data.toString())?.let { send(it) }
                            name = null
                            data.clear()
                            continue
                        }
                        if (line.startsWith(":")) continue
                        val colon = line.indexOf(':')
                        val field = if (colon < 0) line else line.substring(0, colon)
                        val value = if (colon < 0) "" else line.substring(colon + 1).removePrefix(" ")
                        when (field) {
                            "event" -> name = value
                            "data" -> { if (data.isNotEmpty()) data.append('\n'); data.append(value) }
                        }
                    }
                }
                close()
            } catch (error: Throwable) {
                close(if (error is IOException && error !is HttpError) HttpError(0, error.message) else error)
            }
        }
        awaitClose { call.cancel(); reader.cancel() }
    }

    private fun failure(response: Response): HttpError {
        val text = runCatching { response.peekBody(64 * 1024).string() }.getOrNull()
        val detail = parseJson(text ?: "").obj()?.let { it.str("detail", "message") ?: it["error"].obj().str("message") }
        return HttpError(response.code, detail ?: text?.take(200))
    }
}

data class ServerEvent(val type: String, val data: JsonObject) {
    companion object {
        fun of(name: String?, text: String): ServerEvent? {
            if (text.isBlank() && name == null) return null
            if (text.trim() == "[DONE]") return null
            val obj = parseJson(text).obj() ?: JsonObject(emptyMap())
            val candidates = listOfNotNull(obj.str("type"), obj.str("event"), obj.str("event_type"), name)
            val type = candidates.firstOrNull { "." in it } ?: candidates.firstOrNull() ?: "message"
            return ServerEvent(type, obj)
        }
    }
}

suspend fun Call.await(): Response = suspendCancellableCoroutine { continuation ->
    enqueue(object : Callback {
        override fun onResponse(call: Call, response: Response) = continuation.resume(response)
        override fun onFailure(call: Call, e: IOException) {
            if (!continuation.isCancelled) continuation.resumeWithException(HttpError(0, e.message))
        }
    })
    continuation.invokeOnCancellation { cancel() }
}
