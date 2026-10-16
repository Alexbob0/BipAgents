package io.github.bipagents.ui

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import io.github.bipagents.model.Category
import io.github.bipagents.model.Look
import kotlin.math.floor

/** The Hermes style's characters (PixelMascot.swift): a pixel character per category, in the spirit of Hermes Agent's
 *  own sprite. 16 × 18 pixels; the agent's colours paint its clothes and accessories, so a custom colour shows. */
@Composable
fun PixelMascot(look: Look, modifier: Modifier = Modifier) {
    val sprite = PixelSprites.of(look.category)
    Canvas(modifier) {
        // Whole device pixels per sprite pixel, so edges stay sharp at any size.
        val cell = maxOf(1f, floor(minOf(size.width / PixelSprites.WIDTH, size.height / PixelSprites.HEIGHT)))
        val origin = Offset(((size.width - cell * PixelSprites.WIDTH) / 2).let(::floor), ((size.height - cell * PixelSprites.HEIGHT) / 2).let(::floor))
        sprite.rows.forEachIndexed { y, row ->
            row.forEachIndexed { x, letter ->
                if (letter == '.') return@forEachIndexed
                val color = when (letter) {
                    in sprite.main -> look.main
                    in sprite.deep -> look.deep
                    else -> sprite.colors[letter] ?: return@forEachIndexed
                }
                drawRect(color, Offset(origin.x + x * cell, origin.y + y * cell), Size(cell, cell))
            }
        }
    }
}

object PixelSprites {
    const val WIDTH = 16
    const val HEIGHT = 18

    class Sprite(val rows: List<String>, val colors: Map<Char, Color>, val main: String, val deep: String)

    // Generated from the iPhone's sprites (the design canvas's « D · Personnages pixel »).
    fun of(category: Category): Sprite = when (category) {
        Category.DAILY -> Sprite(listOf(
            "ak............ka",
            "aa............aa",
            "qa.kkkkkkkkkk.aq",
            "aakaawwaaaaaakaa",
            "aqkaaaaaaaaaakqa",
            "kakaaaaggaaaakak",
            ".kkaaaaaaaaaakk.",
            "..gggggggggggg..",
            "..kskwsssskwsk..",
            "..kskksssskksk..",
            "..kskksssskksk..",
            "..kpsssmmssspk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('a' to Color(0xFFE8E6E1), 'g' to Color(0xFFE6B422), 'h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 'q' to Color(0xFFA9A49A), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "c", "")
        Category.WELLNESS -> Sprite(listOf(
            "................",
            "................",
            "................",
            "....kkkkkkkk..a.",
            "...hhhhhhhhhha..",
            "..kaaaaaaaaaak..",
            "..khhhhhhhhhhk..",
            "..khhshhhhshhk..",
            "..hskwsssskwsh..",
            ".khskksssskkshk.",
            ".khskksssskkshk.",
            ".khpsssmmsssphk.",
            ".khksssssssskhk.",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "ca", "")
        Category.FINANCE -> Sprite(listOf(
            "................",
            "................",
            "................",
            "....kkkkkkkk....",
            "...hhhhhhhhhh...",
            "..khehhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..kffffshffffk..",
            "..kfkwffffkwfk..",
            "..kfkkfssfkkfk..",
            "..kfkkfssfkkfk..",
            "..kffffmmffffk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kcckaakcck...",
            "..kccccaacccck..",
            "..kccccaacccck..",
        ), mapOf('a' to Color(0xFFE6B422), 'c' to Color(0xFF2B2D35), 'e' to Color(0xFF7A5A40), 'f' to Color(0xFFE6B422), 'h' to Color(0xFF3B2A1E), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "", "")
        Category.WORK -> Sprite(listOf(
            "................",
            ".....h...hh.....",
            "....h.h.hh.h....",
            "....kkkkkkkk....",
            "..aaaaaaaaaaaa..",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            ".aahhhsshshhhaa.",
            ".aaskwsssskwsaa.",
            ".aaskksssskksaa.",
            ".aaskksssskksaa.",
            "..kpsssmmssspk..",
            "...aaassssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "ca", "")
        Category.LEARNING -> Sprite(listOf(
            "......hehh......",
            ".....khhhhk.....",
            "......hhhh......",
            "....kkkkkkkk....",
            "...hhhhhhhhhh...",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..kffffssffffk..",
            "..hfkwffffkwfh..",
            "..hfkkfssfkkfh..",
            "..hfkkfssfkkfh..",
            "..kffffmmffffk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('e' to Color(0xFF6E5040), 'h' to Color(0xFF4A3426), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "cf", "")
        Category.HOME -> Sprite(listOf(
            ".......ww.......",
            "....kkkkkkkk....",
            "...aaaaaaaaaa...",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..bbababbababb..",
            "..bbbbbbbababb..",
            "..kssssssssssk..",
            "..kskksssskksk..",
            "..kssssssssssk..",
            "..kpsssmmssspk..",
            "...kssssssssk...",
            "....kkkkkkkk....",
            ".......ss.......",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "ca", "b")
        Category.CREATIVE -> Sprite(listOf(
            "........k.......",
            "........a.......",
            "..aaaaaaaaaaaa..",
            ".aaaaaaaaaaaaa..",
            "...hhhhhhhhhh...",
            "..khhhhhhhhhhk..",
            "..khhhhhhhhhhk..",
            "..khhsssssshhk..",
            "..hskwsssskwsh..",
            ".khskksssskkshk.",
            ".khskksssskkshk.",
            ".khpsssmmsssphk.",
            ".khksssssssskhk.",
            ".kh.kkkkkkkk.hk.",
            ".kh....ss....hk.",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'w' to Color(0xFFFFFFFF)), "ca", "")
        Category.TECH -> Sprite(listOf(
            "................",
            "................",
            "...kkkkkkkkkk...",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            "..kaaaaaaaaaak..",
            ".kassssssssssak.",
            ".kakkkkkkkkkkak.",
            ".kakvwvvvvwvkak.",
            ".kakkkkkkkkkkak.",
            ".kapsssmmssspak.",
            ".kaksssssssskak.",
            ".ka.kkkkkkkk.ak.",
            ".ka....ss....ak.",
            "...kccksskcck...",
            "..kcccccccccck..",
            "..kcccccccccck..",
        ), mapOf('a' to Color(0xFF4A5063), 'c' to Color(0xFF4A5063), 'h' to Color(0xFF141414), 'k' to Color(0xFF1A1A2E), 'm' to Color(0xFF9C4A44), 'p' to Color(0xFFF2A0A0), 's' to Color(0xFFF4D3B2), 'v' to Color(0xFF5DE2E7), 'w' to Color(0xFFFFFFFF)), "", "")
    }
}
