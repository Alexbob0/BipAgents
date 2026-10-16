package io.github.bipagents.model

import io.github.bipagents.BuildConfig
import io.github.bipagents.net.obj
import io.github.bipagents.net.parseJson
import io.github.bipagents.net.str

/** What a QR code (or a pasted payload) adds: a whole install, or one agent (HermesKit's AgentProvisioning). */
sealed interface Pairing {
    /** `python -m bipbridge qr`: the bridge's address and key; the agents come from `GET /v1/agents`. */
    data class Install(val bridgeUrl: String, val bridgeKey: String, val lanUrl: String? = null, val lanFingerprint: String? = null) : Pairing

    /** One agent's own code. */
    data class Single(val agent: Agent, val keys: AgentKeys) : Pairing

    companion object {
        fun parse(payload: String): Pairing? {
            val o = parseJson(payload.trim()).obj() ?: return null
            if (o["baseURL"] == null && o.str("v") == "2") {
                val url = o.str("bridgeURL")?.takeIf(::acceptable) ?: return null
                val key = o.str("bridgeKey") ?: return null
                // The bridge's local-network door, with its certificate's fingerprint (it can't be typed).
                val lan = o["lan"].obj()
                val fingerprint = lan.str("fingerprint")?.lowercase()?.filter { it.isDigit() || it in 'a'..'f' }?.takeIf { it.length == 64 }
                val door = lan.str("url")?.takeIf { it.startsWith("https://") && fingerprint != null }
                return Install(url, key, door, fingerprint)
            }
            val name = o.str("name") ?: return null
            val base = o.str("baseURL", "base_url", "url")?.takeIf(::acceptable) ?: return null
            val apiKey = o.str("apiKey", "api_key", "key") ?: return null
            val bridge = o.str("bridgeURL", "bridge_url")?.takeIf(::acceptable)
            val agent = Agent(name = name, baseUrl = base, bridgeUrl = bridge,
                bridgeAgent = o.str("agent", "bridgeAgent")?.lowercase(), category = o.str("category"))
            return Single(agent, AgentKeys(apiKey, o.str("bridgeKey", "bridge_key")))
        }

        /** HTTPS (the tailnet's certificates); plain HTTP only in debug builds, for a bridge on the development Mac. */
        fun acceptable(url: String): Boolean =
            url.startsWith("https://") || (BuildConfig.DEBUG && url.startsWith("http://"))
    }
}
