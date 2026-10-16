package io.github.bipagents.net

import java.net.Socket
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.ConcurrentHashMap
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSocket
import javax.net.ssl.X509ExtendedTrustManager

/** The local-network way to an agent, for when Tailscale is down (no Internet at home, the VPN off): the bridge's
 *  LAN door, over HTTPS with its own self-signed certificate, accepted only if its SHA-256 matches the pinned one.
 *  Hermes is reached through the bridge there (`/hermes/<agent>/…`). The tailnet stays the normal way: the store
 *  switches an agent to the door only when the tailnet doesn't answer. */
object LocalLink {
    /** Agents reached through their door right now. */
    private val active = ConcurrentHashMap.newKeySet<String>()
    /** "host:port" → certificate SHA-256 (hex). */
    private val pins = ConcurrentHashMap<String, String>()

    fun isActive(agent: String) = agent in active
    fun setActive(agent: String, on: Boolean) { if (on) active += agent else active -= agent }

    /** The doors' certificates, refreshed whenever the agents change. */
    fun updatePins(doors: List<Pair<String, String>>) {
        pins.clear()
        for ((url, fingerprint) in doors) {
            val uri = android.net.Uri.parse(url)
            val host = uri.host ?: continue
            pins["$host:${if (uri.port > 0) uri.port else 443}"] = fingerprint.lowercase()
        }
    }

    fun pinned(host: String) = pins.keys.any { it.substringBeforeLast(':') == host }

    fun fingerprint(certificate: X509Certificate): String =
        MessageDigest.getInstance("SHA-256").digest(certificate.encoded).joinToString("") { "%02x".format(it) }

    /** Accepts a door's certificate when it is the pinned one for that host and port; refuses anything else. */
    object Trust : X509ExtendedTrustManager() {
        private fun check(chain: Array<out X509Certificate>?, host: String?, port: Int) {
            val leaf = chain?.firstOrNull() ?: throw CertificateException("no certificate")
            val expected = pins["$host:$port"] ?: throw CertificateException("not a known door")
            if (fingerprint(leaf) != expected) throw CertificateException("not our bridge")
        }
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) {
            check(chain, (socket as? SSLSocket)?.handshakeSession?.peerHost ?: socket?.inetAddress?.hostAddress, socket?.port ?: 443)
        }
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
            check(chain, engine?.peerHost, engine?.peerPort ?: 443)
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) = throw CertificateException("host unknown")
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) = throw CertificateException()
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) = throw CertificateException()
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) = throw CertificateException()
        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }
}
