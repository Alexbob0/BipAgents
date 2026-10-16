package io.github.bipagents.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import androidx.compose.ui.unit.dp
import io.github.bipagents.R

/** App/Design/Theme.swift: ink on a pale ground, white cards, the Bips' colours. */
object Bip {
    /** Dark mode (Settings › Appearance: as the phone, light or dark). Read by every colour below. */
    var dark by androidx.compose.runtime.mutableStateOf(false)
    /** The Hermes style (Settings › Appearance): closer to Hermes Agent's own design — flat, Nous blue when light,
     *  gold on black when dark, quieter type, pixel characters. */
    var hermes by androidx.compose.runtime.mutableStateOf(false)
    private fun pick(light: Long, dark: Long, hermesLight: Long = light, hermesDark: Long = dark) =
        Color(if (hermes) { if (this.dark) hermesDark else hermesLight } else if (this.dark) dark else light)

    val ink get() = pick(0xFF16181D, 0xFFF2F3F7, 0xFF17171A, 0xFFFFF8DC)
    val ink2 get() = pick(0xFF4A5163, 0xFFA4A9B8, 0xFF5B5E69, 0xFFB8A57A)
    val muted get() = pick(0xFF8A90A0, 0xFF8A90A2, 0xFF6B6E78, 0xFF9A8A5A)
    val ground get() = pick(0xFFF4F5F8, 0xFF0F1115, 0xFFF8FAFF, 0xFF0F1012)
    val card get() = pick(0xFFFFFFFF, 0xFF1A1D24, 0xFFFFFFFF, 0xFF16171B)
    val line get() = pick(0xFFE4E6EC, 0xFF2A2E37, 0xFFE6E8EF, 0xFF3A2A1A)
    /** What sits on a card (its latest message, a secondary button): white on the Bips' tinted cards, a field on Hermes'. */
    val onCard get() = if (hermes) pick(0, 0, 0xFFEEF2FB, 0xFF201E19) else card
    val online = Color(0xFF3CC37A)
    val danger = Color(0xFFC9472A)
    val accent get() = pick(0xFF4F7CFF, 0xFF7C9CFF, 0xFF0053FD, 0xFFFFC21A)
    /** The main action's fill: ink for the Bips, Nous blue (light) or gold (dark) for Hermes; and its text. */
    val primary get() = if (hermes) pick(0, 0, 0xFF0053FD, 0xFFFFC21A) else ink
    val onPrimary get() = if (hermes) pick(0, 0, 0xFFFFFFFF, 0xFF1A1205) else card
    /** Buttons and fields: capsules for the Bips, squarer for Hermes. */
    val control: androidx.compose.ui.graphics.Shape get() =
        if (hermes) androidx.compose.foundation.shape.RoundedCornerShape(if (dark) 6.dp else 10.dp) else androidx.compose.foundation.shape.RoundedCornerShape(50)

    @OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
    private fun nunito(weight: Int) = Font(R.font.nunito, FontWeight(weight), variationSettings = FontVariation.Settings(FontVariation.weight(weight)))

    /** Nunito: rounded like the iPhone's SF Pro Rounded. */
    val rounded = FontFamily(listOf(400, 600, 700, 800, 900).map(::nunito))

    fun text(size: Int, weight: FontWeight = FontWeight.Normal, color: Color = ink, lineHeight: TextUnit = TextUnit.Unspecified): TextStyle {
        if (!hermes) return TextStyle(fontFamily = rounded, fontSize = size.sp, fontWeight = weight, color = color, lineHeight = lineHeight)
        // Hermes is quieter: the Bips' heavy weights step down a notch; monospaced and a little smaller in the dark.
        val calmer = when {
            weight >= FontWeight.ExtraBold -> FontWeight.SemiBold
            weight >= FontWeight.SemiBold -> FontWeight.Medium
            else -> FontWeight.Normal
        }
        val scale = if (dark) 0.9f else 1f
        return TextStyle(fontFamily = if (dark) FontFamily.Monospace else FontFamily.Default, fontSize = (size * scale).sp, fontWeight = calmer,
            color = color, lineHeight = if (lineHeight == TextUnit.Unspecified) lineHeight else (lineHeight.value * scale).sp)
    }
}

/** Light or dark: as the phone is set, or always one of them. */
object Appearance {
    const val SYSTEM = "system"; const val LIGHT = "light"; const val DARK = "dark"
    var mode by androidx.compose.runtime.mutableStateOf(SYSTEM)
    /** « bips » or « hermes ». */
    var style by androidx.compose.runtime.mutableStateOf("bips")
    fun load(context: android.content.Context) {
        val prefs = context.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE)
        mode = prefs.getString("colorMode", SYSTEM) ?: SYSTEM
        style = prefs.getString("appStyle", "bips") ?: "bips"
        Bip.hermes = style == "hermes"
    }
    fun setStyle(context: android.content.Context, value: String) {
        style = value
        Bip.hermes = value == "hermes"
        context.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE).edit().putString("appStyle", value).apply()
    }
    fun set(context: android.content.Context, value: String) {
        mode = value
        context.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE).edit().putString("colorMode", value).apply()
    }
}

@Composable
fun BipTheme(content: @Composable () -> Unit) {
    val dark = when (Appearance.mode) {
        Appearance.DARK -> true
        Appearance.LIGHT -> false
        else -> androidx.compose.foundation.isSystemInDarkTheme()
    }
    if (Bip.dark != dark) Bip.dark = dark
    val view = androidx.compose.ui.platform.LocalView.current
    if (!view.isInEditMode) androidx.compose.runtime.SideEffect {
        (view.context as? android.app.Activity)?.window?.let { window ->
            androidx.core.view.WindowCompat.getInsetsController(window, view).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
    }
    MaterialTheme(
        colorScheme = (if (dark) androidx.compose.material3.darkColorScheme(primary = Bip.ink, onPrimary = Bip.card, background = Bip.ground,
            surface = Bip.card, onSurface = Bip.ink, onBackground = Bip.ink, secondaryContainer = Bip.line, surfaceContainer = Bip.card,
            surfaceContainerHigh = Bip.card, surfaceContainerHighest = Bip.line)
        else lightColorScheme(primary = Bip.ink, onPrimary = Color.White, background = Bip.ground, surface = Bip.card,
            onSurface = Bip.ink, onBackground = Bip.ink, secondaryContainer = Bip.line)),
        typography = MaterialTheme.typography.let { t ->
            t.copy(bodyLarge = t.bodyLarge.copy(fontFamily = Bip.rounded), bodyMedium = t.bodyMedium.copy(fontFamily = Bip.rounded),
                labelLarge = t.labelLarge.copy(fontFamily = Bip.rounded), titleLarge = t.titleLarge.copy(fontFamily = Bip.rounded))
        },
        content = content,
    )
}
