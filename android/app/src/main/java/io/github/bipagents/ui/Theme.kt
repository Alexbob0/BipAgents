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
import io.github.bipagents.R

/** App/Design/Theme.swift: ink on a pale ground, white cards, the Bips' colours. */
object Bip {
    /** Dark mode (Settings › Appearance: as the phone, light or dark). Read by every colour below. */
    var dark by androidx.compose.runtime.mutableStateOf(false)
    private fun pick(light: Long, dark: Long) = Color(if (this.dark) dark else light)

    val ink get() = pick(0xFF16181D, 0xFFF2F3F7)
    val ink2 get() = pick(0xFF4A5163, 0xFFA4A9B8)
    val muted get() = pick(0xFF8A90A0, 0xFF8A90A2)
    val ground get() = pick(0xFFF4F5F8, 0xFF0F1115)
    val card get() = pick(0xFFFFFFFF, 0xFF1A1D24)
    val line get() = pick(0xFFE4E6EC, 0xFF2A2E37)
    val online = Color(0xFF3CC37A)
    val danger = Color(0xFFC9472A)
    val accent get() = pick(0xFF4F7CFF, 0xFF7C9CFF)

    @OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
    private fun nunito(weight: Int) = Font(R.font.nunito, FontWeight(weight), variationSettings = FontVariation.Settings(FontVariation.weight(weight)))

    /** Nunito: rounded like the iPhone's SF Pro Rounded. */
    val rounded = FontFamily(listOf(400, 600, 700, 800, 900).map(::nunito))

    fun text(size: Int, weight: FontWeight = FontWeight.Normal, color: Color = ink, lineHeight: TextUnit = TextUnit.Unspecified) =
        TextStyle(fontFamily = rounded, fontSize = size.sp, fontWeight = weight, color = color, lineHeight = lineHeight)
}

/** Light or dark: as the phone is set, or always one of them. */
object Appearance {
    const val SYSTEM = "system"; const val LIGHT = "light"; const val DARK = "dark"
    var mode by androidx.compose.runtime.mutableStateOf(SYSTEM)
    fun load(context: android.content.Context) {
        mode = context.getSharedPreferences("settings", android.content.Context.MODE_PRIVATE).getString("colorMode", SYSTEM) ?: SYSTEM
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
