package io.github.bipagents.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
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
    val ink = Color(0xFF16181D)
    val ink2 = Color(0xFF4A5163)
    val muted = Color(0xFF8A90A0)
    val ground = Color(0xFFF4F5F8)
    val card = Color(0xFFFFFFFF)
    val line = Color(0xFFE4E6EC)
    val online = Color(0xFF3CC37A)
    val danger = Color(0xFFC9472A)
    val accent = Color(0xFF4F7CFF)

    @OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
    private fun nunito(weight: Int) = Font(R.font.nunito, FontWeight(weight), variationSettings = FontVariation.Settings(FontVariation.weight(weight)))

    /** Nunito: rounded like the iPhone's SF Pro Rounded. */
    val rounded = FontFamily(listOf(400, 600, 700, 800, 900).map(::nunito))

    fun text(size: Int, weight: FontWeight = FontWeight.Normal, color: Color = ink, lineHeight: TextUnit = TextUnit.Unspecified) =
        TextStyle(fontFamily = rounded, fontSize = size.sp, fontWeight = weight, color = color, lineHeight = lineHeight)
}

@Composable
fun BipTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = lightColorScheme(primary = Bip.ink, onPrimary = Color.White, background = Bip.ground, surface = Bip.card,
            onSurface = Bip.ink, onBackground = Bip.ink, secondaryContainer = Bip.line),
        typography = MaterialTheme.typography.let { t ->
            t.copy(bodyLarge = t.bodyLarge.copy(fontFamily = Bip.rounded), bodyMedium = t.bodyMedium.copy(fontFamily = Bip.rounded),
                labelLarge = t.labelLarge.copy(fontFamily = Bip.rounded), titleLarge = t.titleLarge.copy(fontFamily = Bip.rounded))
        },
        content = content,
    )
}
