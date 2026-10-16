package io.github.bipagents.model

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb

/** A Bip's look (AgentAppearance): its category's shape and accessory, in its category's colours or a custom one. */
data class Look(val category: Category, val main: Color, val deep: Color, val lightTint: Color, val darkTint: Color = mixDark(main)) {
    /** A pale surface in light mode; in dark mode a low wash of the main colour (AgentPalette). */
    val tint: Color get() = if (io.github.bipagents.ui.Bip.dark) darkTint else lightTint
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
            val dark = Color(android.graphics.Color.HSVToColor(floatArrayOf(hsv[0], hsv[1] * 0.45f, 0.22f)))
            return Look(category, main, deep, tint, dark)
        }

        /** 18 % of the main colour into the dark card colour. */
        fun mixDark(main: Color): Color {
            val card = Color(0xFF1A1D24)
            fun mix(a: Float, b: Float) = a * 0.18f + b * 0.82f
            return Color(mix(main.red, card.red), mix(main.green, card.green), mix(main.blue, card.blue))
        }
    }
}
