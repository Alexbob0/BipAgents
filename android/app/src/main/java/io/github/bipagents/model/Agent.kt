package io.github.bipagents.model

import kotlinx.serialization.Serializable
import java.util.UUID

/** One agent (one Hermes profile), without its keys: those live in [Secrets]. */
@Serializable
data class Agent(
    val id: String = UUID.randomUUID().toString(),
    val name: String,
    /** Its Hermes, e.g. `https://box.tail1234.ts.net:9202/p/finance`. */
    val baseUrl: String,
    val bridgeUrl: String? = null,
    /** Its id in the bridge's config (`finance`, `vie`…). */
    val bridgeAgent: String? = null,
    val category: String? = null,
    val language: String? = null,
) {
    val look: Category get() = Category.of(category) ?: Category.suggest(name)
    /** The name the bridge knows it by: given by the bridge, else derived from the name as for agents set up by hand. */
    val bridgeName: String get() = bridgeAgent ?: name.lowercase().replace(Regex("[^a-z0-9_-]"), "")
}

data class AgentKeys(val apiKey: String, val bridgeKey: String?)
