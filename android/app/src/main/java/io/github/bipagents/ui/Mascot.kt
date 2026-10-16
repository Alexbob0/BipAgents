package io.github.bipagents.ui

import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.vector.PathParser
import io.github.bipagents.model.Category
import io.github.bipagents.model.Category.Accessory
import io.github.bipagents.model.Category.Body
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.max
import kotlin.math.sin

/** What a Bip looks like doing (App/Design/MascotView.swift moods). */
enum class Mood { IDLE, THINKING, SPEAKING, ASKING, CONTENT, LISTENING, SLEEPING }

private val INK = Color(0xFF16181D)
private val GOLD = Color(0xFFFFD84D)
private val CHEEK = Color(0xFFFF6F91)
private val MOUTH = Color(0xFF2B1A1F)
private val TONGUE = Color(0xFFFF7A93)

private val paths = HashMap<String, Path>()
private fun svg(d: String): Path = paths.getOrPut(d) { PathParser().parsePathString(d).toPath() }

/** A Bip, drawn in its 120×120 design space and animated (blink, bob, thinking bubbles), as in docs/play. */
@Composable
fun Mascot(category: Category, modifier: Modifier = Modifier, mood: Mood = Mood.IDLE, animated: Boolean = true) {
    val t by produceState(0f, animated) {
        if (!animated) return@produceState
        val start = withFrameMillis { it }
        while (true) withFrameMillis { value = (it - start) / 1000f }
    }
    Canvas(modifier) {
        val s = size.minDimension / 120f
        translate((size.width - 120f * s) / 2, (size.height - 120f * s) / 2) {
            scale(s, s, pivot = Offset.Zero) { bip(category, mood, t) }
        }
    }
}

private fun DrawScope.oval(cx: Float, cy: Float, rx: Float, ry: Float, color: Color, alpha: Float = 1f) =
    drawOval(color, Offset(cx - rx, cy - ry), Size(2 * rx, 2 * max(0f, ry)), alpha = alpha)

private fun DrawScope.line(d: String, color: Color = INK, width: Float = 3f) =
    drawPath(svg(d), color, style = Stroke(width, cap = StrokeCap.Round, join = StrokeJoin.Round))

internal fun DrawScope.bip(c: Category, mood: Mood, t: Float) {
    val period = if (mood == Mood.SPEAKING || mood == Mood.LISTENING) 0.9f else 3f
    val bob = -2f - 2f * sin(t * 2 * PI.toFloat() / period)
    oval(60f, 113f, 32f, 4.5f, INK, 0.1f)
    for (x in listOf(45f, 75f)) {
        oval(x, 106f, 10.5f, 6.5f, c.deep)
        oval(x - 3, 104.5f, 3.5f, 1.8f, Color.White, 0.25f)
    }
    translate(0f, bob) {
        accessory(c, false, t)
        body(c)
        accessory(c, true, t)
        rotate(-30f, Offset(43f, 44f)) { oval(43f, 44f, 12f, 7f, Color.White, 0.32f) }
        oval(37f, 76f, 6.5f, 4f, CHEEK, 0.5f)
        oval(83f, 76f, 6.5f, 4f, CHEEK, 0.5f)
        face(c, mood, t)
    }
}

private fun DrawScope.body(c: Category) {
    when (c.body) {
        Body.BLOB -> drawPath(svg("M60 24c25 0 42 17 42 43 0 25-16 39-42 39S18 92 18 67c0-26 17-43 42-43z"), c.main)
        Body.SQUIRCLE -> drawRoundRect(c.main, Offset(19f, 28f), Size(82f, 78f), androidx.compose.ui.geometry.CornerRadius(32f))
        Body.EGG -> drawPath(svg("M60 20c21 0 38 28 38 52 0 21-16 34-38 34S22 93 22 72c0-24 17-52 38-52z"), c.main)
        Body.CLOUD -> drawPath(svg("M38 46c2-14 12-22 24-22 13 0 23 9 24 22 10 2 17 11 17 22 0 22-18 38-43 38S17 90 17 68c0-12 8-21 21-22z"), c.main)
    }
}

private fun DrawScope.accessory(c: Category, front: Boolean, t: Float) {
    if (!front) when (c.accessory) {
        Accessory.SPROUT -> {
            line("M60 27c0-7 1-13 3-17", c.deep, 4f)
            drawPath(svg("M63 12c5-8 15-9 21-4-5 7-14 9-21 4z"), c.deep)
            drawPath(svg("M61 15c-5-7-14-8-19-3 5 6 13 8 19 3z"), c.main)
            drawPath(svg("M61 15c-5-7-14-8-19-3 5 6 13 8 19 3z"), c.deep, style = Stroke(2f))
        }
        Accessory.COIN -> {
            val p = sin(t * 2 * PI.toFloat() / 2.4f)
            translate(60f, 12f - 1.5f - 1.5f * p) {
                rotate(6f + 6f * p, Offset.Zero) {
                    oval(0f, 0f, 9f, 9f, GOLD)
                    drawCircle(c.deep, 9f, Offset.Zero, style = Stroke(3f))
                    line("M0 -4.5v9", c.deep, 2.6f)
                }
            }
            line("M42 10l-4-3M78 10l4-3", c.deep, 2.5f)
        }
        Accessory.EARS -> for (x in listOf(35f, 85f)) { oval(x, 36f, 12f, 12f, c.main); oval(x, 36f, 5.5f, 5.5f, Color(0xFFFFB3C1)) }
        Accessory.ANTENNA -> {
            val glow = 0.775f + 0.225f * cos(t * 2 * PI.toFloat() / 1.6f)
            line("M60 30V13", c.deep, 3.5f)
            oval(60f, 10f, 6.5f, 6.5f, c.main, glow)
            drawCircle(c.deep, 6.5f, Offset(60f, 10f), style = Stroke(3f))
        }
        Accessory.TUFT -> drawPath(svg("M50 28c-2-8 2-14 8-17-1 6 2 9 4 12 1-6 5-10 11-11-3 5-3 10-2 16z"), c.deep)
        else -> {}
    } else when (c.accessory) {
        Accessory.STAR -> {
            val star = "M60 3l3.6 7.4 8.1 1.1-5.9 5.6 1.5 8-7.3-3.9-7.3 3.9 1.5-8-5.9-5.6 8.1-1.1z"
            drawPath(svg(star), GOLD)
            drawPath(svg(star), c.deep, style = Stroke(2.2f, join = StrokeJoin.Round))
        }
        Accessory.ROOF -> line("M33 35L60 11l27 24", c.deep, 6f)
        else -> {}
    }
}

private fun blink(c: Category, t: Float): Float {
    val phase = ((t + c.ordinal * 1.37f) % 4.6f) / 4.6f
    if (phase <= 0.93f) return 1f
    return max(0.1f, abs((phase - 0.93f) / 0.07f * 2 - 1))
}

private fun DrawScope.eyes(c: Category, t: Float, rx: Float, ry: Float, y: Float, xs: List<Float>, glint: Float, blinks: Boolean = true) {
    val s = if (blinks) blink(c, t) else 1f
    for (x in xs) {
        oval(x, y, rx, ry * s, INK)
        if (s > 0.6f) oval(x + rx * 0.38f, y - ry * 0.4f, glint, glint, Color.White)
    }
}

private fun DrawScope.face(c: Category, mood: Mood, t: Float) {
    when (mood) {
        Mood.LISTENING -> { eyes(c, t, 7f, 9f, 63f, listOf(46f, 74f), 2.6f); oval(60f, 81f, 3.6f, 4.2f, INK) }
        Mood.THINKING -> {
            val pulse = 0.25f + 0.2f * (1 + sin(t * 2 * PI.toFloat() / 1.4f))
            eyes(c, t, 5f, 6.5f, 61f, listOf(49f, 75f), 1.8f, blinks = false)
            line("M54 80h12")
            for ((x, y, r) in listOf(Triple(93f, 30f, 3f), Triple(101f, 21f, 4.4f), Triple(110f, 10f, 6f))) oval(x, y, r, r, c.deep, pulse)
        }
        Mood.SPEAKING -> {
            val open = 0.3f + 0.7f * (0.5f + 0.5f * sin(t * 2 * PI.toFloat() / 0.56f))
            eyes(c, t, 5.5f, 7.5f, 64f, listOf(47f, 73f), 2f)
            translate(60f, 81f) { scale(1f, open, pivot = Offset.Zero) { oval(0f, -1f, 8f, 7f, MOUTH); oval(0f, 3f, 5f, 2.6f, TONGUE) } }
        }
        Mood.ASKING -> { eyes(c, t, 5.5f, 7.5f, 64f, listOf(47f, 73f), 2f); line("M39 54l11-4M81 54l-11-4"); line("M53 81q3.5-3 7 0t7 0") }
        Mood.SLEEPING -> { line("M41 65q6 5 12 0M67 65q6 5 12 0"); oval(60f, 80f, 3f, 2.2f, INK) }
        Mood.CONTENT -> { line("M41 64q6 5 12 0M67 64q6 5 12 0"); line("M54 78q6 5 12 0") }
        Mood.IDLE -> { eyes(c, t, 5.5f, 7.5f, 64f, listOf(47f, 73f), 2f); line("M53 78q7 6 14 0") }
    }
}

/** A Bip as a bitmap, for notifications (the iPhone's avatar in its notifications). */
fun mascotBitmap(category: Category, px: Int = 192): android.graphics.Bitmap {
    val bitmap = android.graphics.Bitmap.createBitmap(px, px, android.graphics.Bitmap.Config.ARGB_8888)
    val canvas = androidx.compose.ui.graphics.Canvas(android.graphics.Canvas(bitmap))
    androidx.compose.ui.graphics.drawscope.CanvasDrawScope().draw(androidx.compose.ui.unit.Density(1f),
        androidx.compose.ui.unit.LayoutDirection.Ltr, canvas, Size(px.toFloat(), px.toFloat())) {
        drawCircle(category.tint)
        scale(px / 140f, px / 140f, pivot = Offset.Zero) { translate(10f, 12f) { bip(category, Mood.IDLE, 0.5f) } }
    }
    return bitmap
}
