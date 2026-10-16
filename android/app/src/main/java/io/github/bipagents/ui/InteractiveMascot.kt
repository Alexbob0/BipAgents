package io.github.bipagents.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import io.github.bipagents.R
import io.github.bipagents.model.Look
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.hypot
import kotlin.math.min
import kotlin.math.sign

/**
 * A Bip that reacts (InteractiveMascot.swift, docs/play): a tap gets a hop, a wink, a twirl, a surprise or a « thanks »;
 * three quick taps or a fast rub tickle it; a slow stroke makes it purr; a long press then release makes it jump
 * (« Boing ! »); dragging it, it follows with its eyes; pestered too much, it gets dizzy, then sulks. Each reaction
 * says something in a bubble, babbled in the Bip's voice.
 */
@Composable
fun InteractiveMascot(look: Look, size: Dp, modifier: Modifier = Modifier, baseMood: Mood = Mood.IDLE, bubbleLeading: Boolean = false) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var reaction by remember { mutableStateOf<Mood?>(null) }
    var sulking by remember { mutableStateOf(false) }
    var bubble by remember { mutableStateOf<String?>(null) }
    var blush by remember { mutableStateOf(0f) }
    var gaze by remember { mutableStateOf(Offset.Zero) }
    val dx = remember { Animatable(0f) }
    val dy = remember { Animatable(0f) }
    val sx = remember { Animatable(1f) }
    val sy = remember { Animatable(1f) }
    val rot = remember { Animatable(0f) }
    val particles = remember { mutableStateListOf<Particle>() }
    var moodJob by remember { mutableStateOf<Job?>(null) }
    var bubbleJob by remember { mutableStateOf<Job?>(null) }
    val taps = remember { mutableListOf<Long>() }
    val pokes = remember { mutableListOf<Long>() }

    fun show(mood: Mood, seconds: Double) {
        reaction = mood
        moodJob?.cancel()
        moodJob = scope.launch { delay((seconds * 1000).toLong()); if (!sulking) reaction = null }
    }
    fun say(id: Int) {
        val words = context.resources.getStringArray(id).random()
        bubble = words
        bubbleJob?.cancel()
        bubbleJob = scope.launch { delay(1500); bubble = null }
        Babble.say(context, words, look, reaction ?: baseMood)
    }
    fun rest(ms: Int = 450) = scope.launch {
        launch { dx.animateTo(0f, tween(ms)) }; launch { dy.animateTo(0f, tween(ms)) }
        launch { sx.animateTo(1f, tween(ms)) }; launch { sy.animateTo(1f, tween(ms)) }
        launch { rot.animateTo(0f, tween(ms)) }
    }
    fun jump(h: Float) = scope.launch {
        launch { dy.animateTo(-h, tween(200, easing = FastOutSlowInEasing)) }
        launch { sx.animateTo(0.88f, tween(200)) }; sy.animateTo(1.14f, tween(200))
        launch { dy.animateTo(0f, spring(Spring.DampingRatioMediumBouncy, Spring.StiffnessLow)) }
        launch { sx.animateTo(1f, tween(450)) }; sy.animateTo(1f, tween(450))
    }
    fun pop(f: Float) = scope.launch {
        launch { sx.animateTo(f, tween(180)) }; sy.animateTo(f, tween(180))
        launch { sx.animateTo(1f, spring(Spring.DampingRatioMediumBouncy)) }; sy.animateTo(1f, spring(Spring.DampingRatioMediumBouncy))
    }
    fun spin() = scope.launch { rot.animateTo(360f, tween(700, easing = FastOutSlowInEasing)); rot.snapTo(0f) }
    fun shake(times: Int, angle: Float) = scope.launch {
        repeat(times) { i -> rot.animateTo(if (i % 2 == 1) -angle else angle, tween(70)) }
        rot.animateTo(0f, tween(300))
    }
    fun burst(heart: Boolean, count: Int) {
        repeat(count) { particles += Particle(heart, (Math.random() * 70 - 35).toFloat(), (Math.random() * 250).toLong()) }
        scope.launch { delay(1900); repeat(count) { if (particles.isNotEmpty()) particles.removeAt(0) } }
    }
    fun blushUp() { blush = 1f; scope.launch { delay(1600); blush = 0f } }

    fun tapped() {
        if (sulking) return
        when ((0..4).random()) {
            0 -> { show(Mood.IDLE, 0.8); say(R.array.bip_hop); jump(26f) }
            1 -> { show(Mood.WINKING, 1.1); say(R.array.bip_wink); pop(1.08f) }
            2 -> { show(Mood.GIGGLING, 0.9); say(R.array.bip_spin); spin() }
            3 -> { show(Mood.SURPRISED, 1.0); say(R.array.bip_surprised); pop(1.14f) }
            else -> { show(Mood.CONTENT, 1.4); say(R.array.bip_thanks); burst(true, 3); blushUp() }
        }
    }
    fun giggle() { show(Mood.GIGGLING, 1.8); say(R.array.bip_giggle); burst(false, 4); shake(8, 9f) }
    fun pet() { show(Mood.CONTENT, 2.0); say(R.array.bip_pet); burst(true, 2); blushUp() }
    fun annoyed() {
        sulking = true
        show(Mood.DIZZY, 1.8); say(R.array.bip_dizzy); shake(5, 14f)
        scope.launch {
            delay(1800); reaction = Mood.GRUMPY; say(R.array.bip_grumpy)
            delay(2400); sulking = false; reaction = null
        }
    }
    /** True when the touch already made it react (tickled, annoyed). */
    fun touchedDown(): Boolean {
        val now = System.currentTimeMillis()
        taps.removeAll { now - it > 1200 }; taps += now
        pokes.removeAll { now - it > 6000 }; pokes += now
        if (sulking) return true
        if (pokes.size >= 8) { pokes.clear(); taps.clear(); annoyed(); return true }
        if (taps.size >= 3) { taps.clear(); giggle(); return true }
        return false
    }

    val pose = Pose(dx.value, dy.value, sx.value, sy.value, rot.value, gaze.x, gaze.y, blush)
    Box(modifier.size(size), contentAlignment = Alignment.Center) {
        Mascot(look, Modifier.size(size).pointerInput(look) {
            val unit = this.size.width / 120f   // the drawing's units
            awaitEachGesture {
                val down = awaitFirstDown()
                var handled = touchedDown()
                var moved = 0f
                var squeezed = false
                val start = System.currentTimeMillis()
                var lastX = down.position.x
                var lastY = down.position.y
                var dir = 0f
                val reversals = ArrayList<Long>()
                var travelled = 0f
                var petted = false
                val squeeze = scope.launch {
                    delay(350)
                    if (moved < 15 * unit && !sulking) {
                        squeezed = true
                        launch { sx.animateTo(1.14f, tween(250)) }; sy.animateTo(0.82f, tween(250))
                    }
                }
                val rubber = { d: Float -> sign(d) * min(abs(d), 60f * (1 - exp(-abs(d) / 60f))) }
                while (true) {
                    val event = awaitPointerEvent()
                    val change = event.changes.firstOrNull { it.id == down.id } ?: break
                    if (!change.pressed) break
                    val ddx = (change.position.x - down.position.x) / unit
                    val ddy = (change.position.y - down.position.y) / unit
                    moved = hypot(ddx, ddy) * unit
                    if (moved > 15 * unit) {
                        squeeze.cancel()
                        if (squeezed) { squeezed = false; scope.launch { launch { sx.animateTo(1f, tween(300)) }; sy.animateTo(1f, tween(300)) } }
                    }
                    scope.launch { dx.snapTo(rubber(ddx) / 1.4f); dy.snapTo(rubber(ddy) / 1.4f) }
                    gaze = Offset(ddx / 60f, ddy / 60f)
                    // Fast back and forth = tickle, slow long stroke = petting.
                    val now = System.currentTimeMillis()
                    val stepX = change.position.x - lastX
                    travelled += (abs(stepX) + abs(change.position.y - lastY)) / unit
                    val d = sign(stepX)
                    if (d != 0f && dir != 0f && d != dir && abs(stepX) > unit) reversals += now
                    if (d != 0f) dir = d
                    lastX = change.position.x; lastY = change.position.y
                    reversals.removeAll { now - it > 900 }
                    if (!sulking && reversals.size >= 4) { reversals.clear(); giggle(); handled = true }
                    else if (!sulking && !petted && now - start > 600 && travelled > 45 && reversals.size <= 1) { petted = true; pet(); handled = true }
                    change.consume()
                }
                squeeze.cancel()
                if (squeezed && !sulking) { show(Mood.SURPRISED, 0.9); say(R.array.bip_boing); jump(48f); handled = true }
                else if (moved > 6 * unit) { rest(); handled = true }
                if (!handled) tapped()
                gaze = Offset.Zero
            }
        }, mood = reaction ?: baseMood, pose = pose)
        for (p in particles) Particle(p, look)
        // The bubble may be wider than the Bip: above it, or on its left when it sits at the edge (a home card).
        val place = if (bubbleLeading) Modifier.align(Alignment.CenterEnd).wrapContentWidth(Alignment.End, unbounded = true).offset(x = -size * 0.82f)
            else Modifier.align(Alignment.TopCenter).wrapContentWidth(unbounded = true).offset(y = -size / 6)
        AnimatedVisibility(bubble != null, place, enter = fadeIn() + scaleIn(), exit = fadeOut()) {
            Text(bubble ?: "", style = Bip.text(14, FontWeight.Black, look.deep), maxLines = 1, softWrap = false,
                modifier = Modifier.background(Bip.card, RoundedCornerShape(50)).padding(horizontal = 12.dp, vertical = 6.dp))
        }
    }
}

data class Particle(val heart: Boolean, val x: Float, val delayMs: Long, val id: Long = System.nanoTime())

@Composable
private fun Particle(p: Particle, look: Look) {
    val rise = remember(p.id) { Animatable(0f) }
    androidx.compose.runtime.LaunchedEffect(p.id) { delay(p.delayMs); rise.animateTo(1f, tween(1500)) }
    Text(if (p.heart) "♥" else "✦", style = Bip.text(20, FontWeight.Black, if (p.heart) Color(0xFFFF5C8A) else look.deep),
        modifier = Modifier.offset { IntOffset((p.x * 2).dp.roundToPx(), (-60 * rise.value).dp.roundToPx()) }
            .graphicsLayer { alpha = if (rise.value == 0f) 0f else 1f - rise.value; scaleX = 0.6f + rise.value; scaleY = 0.6f + rise.value })
}
