package io.github.bipagents.model

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb

/** A Bip's look (AgentAppearance): its category's shape and accessory, in its category's colours or a custom one. */
data class Look(val category: Category, val main: Color, val deep: Color, val tint: Color) {
    val body get() = category.body
    val accessory get() = category.accessory
    /** Each category blinks at its own moment. */
    val ordinal get() = category.ordinal

    companion object {
        fun of(category: Category, color: String? = null): Look {
            val hex = color?.removePrefix("#")?.toLongOrNull(16) ?: return Look(category, category.main, category.deep, category.tint)
            val main = Color(0xFF000000 or hex)
            // AgentPalette(custom:): a darker, slightly more saturated shade, and a pale wash of the same hue.
            val hsv = FloatArray(3).also { android.graphics.Color.colorToHSV(main.toArgb(), it) }
            val deep = Color(android.graphics.Color.HSVToColor(floatArrayOf(hsv[0], minOf(1f, hsv[1] * 1.05f), hsv[2] * 0.62f)))
            val tint = Color(android.graphics.Color.HSVToColor(floatArrayOf(hsv[0], hsv[1] * 0.16f, 0.98f)))
            return Look(category, main, deep, tint)
        }
    }
}
