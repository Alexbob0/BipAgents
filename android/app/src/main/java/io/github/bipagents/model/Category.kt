package io.github.bipagents.model

import androidx.compose.ui.graphics.Color

/** The eight kinds of Bips (App/Design/AgentCategory.swift, docs/play): colours, body, accessory. */
enum class Category(val main: Color, val deep: Color, val tint: Color, val body: Body, val accessory: Accessory,
                    private val keywords: List<String>) {
    WELLNESS(Color(0xFF3CC37A), Color(0xFF1D8A4F), Color(0xFFE2F5EA), Body.BLOB, Accessory.SPROUT,
        listOf("well", "santé", "sante", "health", "sommeil", "sleep", "sport", "fitness", "bien-être", "bien etre", "médit", "nutrition", "coach", "salud", "bienestar", "sueño", "gesund", "schlaf")),
    FINANCE(Color(0xFFF6B826), Color(0xFF9A6A00), Color(0xFFFFF3D1), Body.SQUIRCLE, Accessory.COIN,
        listOf("financ", "budget", "banque", "bank", "argent", "money", "invest", "compta", "impôt", "impot", "tax", "dinero", "banco", "impuesto", "geld", "finanz", "steuer")),
    DAILY(Color(0xFFFF7B5C), Color(0xFFC9472A), Color(0xFFFFE8E0), Body.EGG, Accessory.EARS,
        listOf("vie", "life", "perso", "agenda", "quotidien", "organis", "assistant", "planning", "famille", "family", "daily", "vida", "familia", "alltag", "leben", "familie")),
    WORK(Color(0xFF4F7CFF), Color(0xFF2B55D4), Color(0xFFE4EBFF), Body.SQUIRCLE, Accessory.ANTENNA,
        listOf("travail", "work", "pro", "bureau", "client", "projet", "business", "mail", "job", "trabajo", "oficina", "arbeit", "büro")),
    LEARNING(Color(0xFF8E6CF5), Color(0xFF6142D2), Color(0xFFEDE7FF), Body.CLOUD, Accessory.TUFT,
        listOf("appren", "learn", "étude", "etude", "langue", "cours", "school", "prof", "tutor", "aprend", "estudi", "idioma", "escuela", "lern", "schule", "sprach")),
    HOME(Color(0xFF1FB2C1), Color(0xFF0D7F8B), Color(0xFFDDF4F6), Body.EGG, Accessory.ROOF,
        listOf("maison", "home", "domot", "jardin", "cuisine", "courses", "bricol", "garden", "kitchen", "casa", "hogar", "cocina", "haus", "garten", "küche")),
    CREATIVE(Color(0xFFF2649E), Color(0xFFC23774), Color(0xFFFDE4EE), Body.BLOB, Accessory.STAR,
        listOf("créa", "crea", "écri", "ecri", "write", "musique", "music", "design", "photo", "art", "escrit", "música", "creativ", "schreib", "musik", "kunst", "kreativ")),
    TECH(Color(0xFF6B7489), Color(0xFF3F4659), Color(0xFFE7E9EE), Body.SQUIRCLE, Accessory.ANTENNA,
        listOf("tech", "dev", "code", "serveur", "server", "infra", "ops", "admin", "linux", "servidor", "technik"));

    enum class Body { BLOB, SQUIRCLE, EGG, CLOUD }
    enum class Accessory { SPROUT, COIN, EARS, ANTENNA, TUFT, ROOF, STAR }

    /** The id the app and the bridge use (`wellness`, `daily`…). */
    val id: String get() = name.lowercase()

    companion object {
        fun of(id: String?): Category? = entries.firstOrNull { it.id == id?.lowercase() }

        /** Best guess from an agent's name (AgentCategory.suggest); Everyday when nothing matches. */
        fun suggest(name: String, description: String = ""): Category {
            val text = "$name $description".lowercase()
            return entries.map { it to it.keywords.count { k -> k in text } }.filter { it.second > 0 }
                .maxByOrNull { it.second }?.first ?: DAILY
        }
    }
}
