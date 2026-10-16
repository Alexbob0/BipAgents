package io.github.bipagents.ui

import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.media.MediaMetadataRetriever
import android.net.Uri
import android.webkit.MimeTypeMap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.InsertDriveFile
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.GraphicEq
import androidx.compose.material.icons.rounded.Pause
import androidx.compose.material.icons.rounded.PlayArrow
import androidx.compose.material.icons.rounded.Replay
import androidx.compose.material.icons.rounded.Share
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.FileProvider
import io.github.bipagents.R
import io.github.bipagents.model.Look
import io.github.bipagents.net.Bridge
import io.github.bipagents.voice.AudioPlayer
import io.github.bipagents.voice.NowPlaying
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.io.File

/** Files the agents send (« MEDIA:<path> »), fetched from the bridge once and kept in the cache. */
object MediaFiles {
    private val audio = setOf("mp3", "m4a", "aac", "wav", "ogg", "opus", "flac")
    private val images = setOf("png", "jpg", "jpeg", "gif", "webp", "heic")
    private val videos = setOf("mp4", "m4v", "mov", "webm")
    private fun ext(name: String) = name.substringAfterLast('.', "").lowercase()
    fun isAudio(name: String) = ext(name) in audio
    fun isImage(name: String) = ext(name) in images
    fun isVideo(name: String) = ext(name) in videos

    suspend fun fetch(context: Context, bridge: Bridge, path: String): File {
        val name = path.substringAfterLast('/').ifEmpty { "file" }
        val key = java.security.MessageDigest.getInstance("SHA-1").digest(path.toByteArray()).take(8).joinToString("") { "%02x".format(it) }
        val file = File(context.cacheDir, "media/$key").apply { mkdirs() }.resolve(name)
        if (file.exists() && file.length() > 0) return file
        val bytes = bridge.media(path)
        withContext(Dispatchers.IO) { file.writeBytes(bytes) }
        return file
    }

    fun uri(context: Context, file: File): Uri = FileProvider.getUriForFile(context, context.packageName + ".files", file)
    fun mime(name: String) = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext(name)) ?: "application/octet-stream"

    /** In the app that reads it. */
    fun open(context: Context, file: File, name: String = file.name) {
        runCatching {
            context.startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(uri(context, file), mime(name)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION))
        }.onFailure { android.widget.Toast.makeText(context, R.string.no_app_for_file, android.widget.Toast.LENGTH_SHORT).show() }
    }

    /** The share sheet (save to Files, Photos, send…), under a readable name. */
    fun share(context: Context, file: File, name: String = file.name) {
        val clean = name.replace('/', '-').replace(':', '-').let { if ('.' in it || file.extension.isEmpty()) it else "$it.${file.extension}" }
        val copy = File(context.cacheDir, "share/${System.nanoTime()}").apply { mkdirs() }.resolve(clean)
        val target = runCatching { file.copyTo(copy, overwrite = true) }.getOrDefault(file)
        val send = Intent(Intent.ACTION_SEND).setType(mime(clean)).putExtra(Intent.EXTRA_STREAM, uri(context, target))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        context.startActivity(Intent.createChooser(send, null))
    }

    fun duration(file: File): Long = runCatching {
        MediaMetadataRetriever().use { it.setDataSource(file.path); it.extractMetadata(MediaMetadataRetriever.METADATA_KEY_DURATION)?.toLong() }
    }.getOrNull() ?: 0

    /** At most `max` pixels on its longer side. */
    fun image(file: File, max: Int = 1600): ImageBitmap? = runCatching {
        val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(file.path, bounds)
        var sample = 1
        while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= max) sample *= 2
        BitmapFactory.decodeFile(file.path, BitmapFactory.Options().apply { inSampleSize = sample })?.asImageBitmap()
    }.getOrNull()
}

/** A file an agent produced: audio as a card (full player), images and videos inline, anything else opens in its app.
 *  `hideIfMissing`: a file merely named (a sub-task's report) shows only if it exists. */
@Composable
fun MediaRow(path: String, look: Look, agentName: String, hideIfMissing: Boolean = false) {
    val context = LocalContext.current
    val bridge = LocalBridge.current
    val name = path.substringAfterLast('/')
    var file by remember(path) { mutableStateOf<File?>(null) }
    var image by remember(path) { mutableStateOf<ImageBitmap?>(null) }
    var failed by remember(path) { mutableStateOf(false) }
    var attempt by remember(path) { mutableStateOf(0) }
    LaunchedEffect(path, attempt) {
        if (bridge == null) { failed = true; return@LaunchedEffect }
        failed = false
        try {
            val f = MediaFiles.fetch(context, bridge, path)
            if (MediaFiles.isImage(name)) image = withContext(Dispatchers.IO) { MediaFiles.image(f) }
            file = f
        } catch (e: Throwable) {
            if (e is kotlinx.coroutines.CancellationException) throw e
            failed = true
        }
    }
    if (failed && hideIfMissing) return
    val f = file
    Row(Modifier.padding(start = 36.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        when {
            f != null && MediaFiles.isAudio(name) -> AudioCard(NowPlaying(f, NowPlayingTitle.of(name), agentName, look), look)
            image != null -> {
                var viewing by remember { mutableStateOf(false) }
                Image(image!!, name, Modifier.widthIn(max = 240.dp).heightIn(max = 300.dp).clip(RoundedCornerShape(18.dp)).tap { viewing = true },
                    contentScale = ContentScale.Fit)
                if (viewing) PhotoViewer(image!!, { viewing = false }) { MediaFiles.share(context, f!!, name) }
            }
            f != null && MediaFiles.isVideo(name) -> VideoClip(f)
            else -> Row(Modifier.background(look.tint, RoundedCornerShape(50)).tap {
                if (f != null) MediaFiles.open(context, f, name) else if (failed) attempt++
            }.padding(horizontal = 14.dp).height(38.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(if (failed) Icons.Rounded.Warning else if (MediaFiles.isAudio(name)) Icons.Rounded.GraphicEq else Icons.AutoMirrored.Rounded.InsertDriveFile,
                    null, Modifier.size(16.dp), tint = look.deep)
                Text(if (failed) stringResource(R.string.file_unavailable) else name, style = Bip.text(13, FontWeight.Bold, look.deep), maxLines = 1,
                    overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 220.dp))
                if (f == null && !failed) CircularProgressIndicator(Modifier.size(14.dp), color = look.deep, strokeWidth = 2.dp)
            }
        }
        // An audio card opens the full player, which has its own Share button.
        if (f != null && !MediaFiles.isAudio(name)) Box(Modifier.size(38.dp).background(look.tint, CircleShape).tap { MediaFiles.share(context, f, name) },
            contentAlignment = Alignment.Center) {
            Icon(Icons.Rounded.Share, stringResource(R.string.share), Modifier.size(17.dp), tint = look.deep)
        }
    }
}

object NowPlayingTitle {
    /** « podcast_du_matin-2026-10-06.mp3 » → « Podcast du matin 2026-10-06 ». */
    fun of(name: String) = name.substringBeforeLast('.').replace('_', ' ').replace(Regex("\\s+"), " ").trim()
        .replaceFirstChar { it.uppercase() }
}

private fun time(ms: Long): String { val s = (ms / 1000).toInt(); return "%d:%02d".format(s / 60, s % 60) }
fun speed(rate: Float): String = "×" + java.text.NumberFormat.getInstance().apply { maximumFractionDigits = 2 }.format(rate)

/** A long audio in a conversation (a podcast): the Bip, title, agent and time left, play; a tap opens the full player. */
@Composable
fun AudioCard(now: NowPlaying, look: Look) {
    val context = LocalContext.current
    val state by AudioPlayer.state.collectAsState()
    val loaded = state.now?.file == now.file
    var duration by remember(now.file) { mutableLongStateOf(0L) }
    LaunchedEffect(now.file) { duration = withContext(Dispatchers.IO) { MediaFiles.duration(now.file) } }
    var full by remember { mutableStateOf(false) }
    DisposableEffect(now.file) {
        AudioPlayer.cardsOnScreen += now.file.path
        onDispose { AudioPlayer.cardsOnScreen -= now.file.path }
    }
    val total = if (loaded && state.duration > 0) state.duration else duration
    Column(Modifier.widthIn(max = 320.dp).fillMaxWidth().background(Bip.card, RoundedCornerShape(22.dp))
        .border(1.dp, look.main.copy(alpha = 0.25f), RoundedCornerShape(22.dp)).tap { full = true }.padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Artwork(look, 56.dp)
            Column(Modifier.weight(1f)) {
                Text(now.title, style = Bip.text(15, FontWeight.ExtraBold), maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(listOfNotNull(now.artist, time(if (loaded) (total - state.position).coerceAtLeast(0) else total),
                    speed(state.rate).takeIf { loaded && state.rate != 1f }).joinToString("  "), style = Bip.text(13, FontWeight.Bold, Bip.ink2))
            }
            PlayButton(loaded && state.playing, look.deep, 44.dp) { AudioPlayer.toggle(context, now) }
        }
        if (loaded) LinearProgressIndicator({ if (total > 0) state.position.toFloat() / total else 0f }, Modifier.fillMaxWidth(),
            color = look.deep, trackColor = look.tint, drawStopIndicator = {})
    }
    if (full) FullPlayer(now, look, total) { full = false }
}

@Composable
private fun PlayButton(playing: Boolean, tint: Color, size: androidx.compose.ui.unit.Dp, onClick: () -> Unit) {
    Box(Modifier.size(size).background(tint, CircleShape).tap(onClick), contentAlignment = Alignment.Center) {
        Icon(if (playing) Icons.Rounded.Pause else Icons.Rounded.PlayArrow, stringResource(if (playing) R.string.pause else R.string.listen),
            Modifier.size(size * 0.5f), tint = Color.White)
    }
}

/** The agent's Bip as the audio's artwork (a waveform when unknown). */
@Composable
fun Artwork(look: Look?, size: androidx.compose.ui.unit.Dp) {
    Box(Modifier.size(size).clip(RoundedCornerShape(size * 0.22f)).background(look?.tint ?: Bip.line), contentAlignment = Alignment.Center) {
        if (look != null) Mascot(look, Modifier.size(size * 0.86f), animated = false)
        else Icon(Icons.Rounded.GraphicEq, null, Modifier.size(size * 0.4f), tint = Bip.ink2)
    }
}

/** The audio playing whose card is out of sight: pinned at the top of the conversation, above the tab bar elsewhere.
 *  Play / pause, ✕ to stop, a tap opens the full player. */
@Composable
fun MiniPlayer(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val state by AudioPlayer.state.collectAsState()
    val now = state.now ?: return
    if (now.file.path in AudioPlayer.cardsOnScreen) return
    val tint = now.look?.deep ?: Bip.ink
    var full by remember { mutableStateOf(false) }
    Box(modifier.padding(horizontal = 12.dp, vertical = 6.dp)) {
        Column(Modifier.fillMaxWidth().background(Bip.card, RoundedCornerShape(18.dp)).border(1.dp, Bip.line, RoundedCornerShape(18.dp))
            .tap { full = true }) {
            Row(Modifier.padding(start = 8.dp, end = 6.dp, top = 7.dp, bottom = 5.dp), verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Artwork(now.look, 38.dp)
                Column(Modifier.weight(1f)) {
                    Text(now.title, style = Bip.text(14, FontWeight.ExtraBold), maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(listOfNotNull(now.artist, "-" + time((state.duration - state.position).coerceAtLeast(0))).joinToString("  "),
                        style = Bip.text(12, FontWeight.Bold, Bip.ink2))
                }
                PlayButton(state.playing, tint, 36.dp) { AudioPlayer.toggle(context, now) }
                Box(Modifier.size(32.dp).tap { AudioPlayer.stop() }, contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Close, stringResource(R.string.stop_playback), Modifier.size(18.dp), tint = Bip.ink2)
                }
            }
            LinearProgressIndicator({ if (state.duration > 0) state.position.toFloat() / state.duration else 0f },
                Modifier.fillMaxWidth().padding(horizontal = 14.dp).height(2.5.dp), color = tint, trackColor = Color.Transparent, drawStopIndicator = {})
        }
    }
    if (full) FullPlayer(now, now.look, state.duration) { full = false }
}

/** The full player: the Bip large, scrubbing, −15 / play / +15, speed and share. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FullPlayer(now: NowPlaying, look: Look?, duration: Long, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val state by AudioPlayer.state.collectAsState()
    val loaded = state.now?.file == now.file
    val tint = look?.deep ?: Bip.ink
    val total = if (loaded && state.duration > 0) state.duration else duration
    var dragging by remember { mutableStateOf<Float?>(null) }
    val elapsed = dragging?.toLong() ?: if (loaded) state.position else 0L
    ModalBottomSheet(onDismiss, sheetState = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = Bip.ground) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 24.dp, vertical = 8.dp),
            horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Artwork(look, 150.dp)
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                Text(now.title, style = Bip.text(20, FontWeight.ExtraBold), textAlign = TextAlign.Center, maxLines = 2)
                now.artist?.let { Text(it, style = Bip.text(15, FontWeight.Bold, Bip.ink2)) }
            }
            Column {
                Slider(elapsed.toFloat().coerceIn(0f, maxOf(total, 1).toFloat()), { dragging = it }, valueRange = 0f..maxOf(total, 1).toFloat(),
                    onValueChangeFinished = { dragging?.let { AudioPlayer.seek(context, now, it.toLong()) }; dragging = null },
                    colors = SliderDefaults.colors(thumbColor = tint, activeTrackColor = tint))
                Row {
                    Text(time(elapsed), style = Bip.text(12, FontWeight.ExtraBold, Bip.ink2))
                    Spacer(Modifier.weight(1f))
                    Text("-" + time((total - elapsed).coerceAtLeast(0)), style = Bip.text(12, FontWeight.ExtraBold, Bip.ink2))
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(34.dp)) {
                Box(Modifier.size(48.dp).tap { if (loaded) AudioPlayer.skip(context, -15_000) }, contentAlignment = Alignment.Center) {
                    Skip15(back = true)
                }
                PlayButton(loaded && state.playing, tint, 70.dp) { AudioPlayer.toggle(context, now) }
                Box(Modifier.size(48.dp).tap { if (loaded) AudioPlayer.skip(context, 15_000) else AudioPlayer.seek(context, now, 15_000) },
                    contentAlignment = Alignment.Center) {
                    Skip15(back = false)
                }
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                var menu by remember { mutableStateOf(false) }
                Box {
                    Text(speed(state.rate), style = Bip.text(15, FontWeight.ExtraBold, tint), modifier = Modifier
                        .background(tint.copy(alpha = 0.12f), RoundedCornerShape(50)).tap { menu = true }.padding(horizontal = 16.dp, vertical = 8.dp))
                    DropdownMenu(menu, { menu = false }, containerColor = Bip.card) {
                        for (rate in AudioPlayer.rates) DropdownMenuItem({ Text((if (rate == state.rate) "✓  " else "") + speed(rate),
                            style = Bip.text(16, FontWeight.Bold)) }, { menu = false; AudioPlayer.setRate(context, rate) })
                    }
                }
                Spacer(Modifier.weight(1f))
                Box(Modifier.size(48.dp).tap { MediaFiles.share(context, now.file, now.title + "." + now.file.extension) }, contentAlignment = Alignment.Center) {
                    Icon(Icons.Rounded.Share, stringResource(R.string.share), Modifier.size(22.dp), tint = tint)
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

/** The iPhone's « gobackward.15 » / « goforward.15 ». */
@Composable
private fun Skip15(back: Boolean) {
    Box(contentAlignment = Alignment.Center) {
        Icon(Icons.Rounded.Replay, stringResource(if (back) R.string.back_15 else R.string.forward_15),
            Modifier.size(34.dp).graphicsLayer(scaleX = if (back) 1f else -1f), tint = Bip.ink)
        Text("15", style = Bip.text(9, FontWeight.Black), modifier = Modifier.padding(top = 3.dp))
    }
}

/** A photo full screen: pinch or double-tap to zoom, drag to pan, ✕ to close, share. */
@Composable
fun PhotoViewer(image: ImageBitmap, onDismiss: () -> Unit, onShare: (() -> Unit)? = null) {
    Dialog(onDismiss, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        var scale by remember { mutableFloatStateOf(1f) }
        var offset by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            Image(image, null, Modifier.fillMaxSize()
                .pointerInput(Unit) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        scale = (scale * zoom).coerceIn(1f, 6f)
                        offset = if (scale == 1f) androidx.compose.ui.geometry.Offset.Zero else offset + pan
                    }
                }
                .pointerInput(Unit) {
                    detectTapGestures(onDoubleTap = {
                        if (scale > 1f) { scale = 1f; offset = androidx.compose.ui.geometry.Offset.Zero } else scale = 2.5f
                    })
                }
                .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y), contentScale = ContentScale.Fit)
            Row(Modifier.fillMaxWidth().statusBarsPadding().padding(12.dp), horizontalArrangement = Arrangement.SpaceBetween) {
                RoundButton(Icons.Rounded.Close, stringResource(R.string.close), onDismiss, size = 44.dp, background = Color.White.copy(alpha = 0.18f),
                    tint = Color.White)
                if (onShare != null) RoundButton(Icons.Rounded.Share, stringResource(R.string.share), onShare, size = 44.dp,
                    background = Color.White.copy(alpha = 0.18f), tint = Color.White)
            }
        }
    }
}

/** A video an agent produced, played inline (its own controls). */
@Composable
fun VideoClip(file: File) {
    val context = LocalContext.current
    val player = remember(file) {
        androidx.media3.exoplayer.ExoPlayer.Builder(context).build().apply {
            setMediaItem(androidx.media3.common.MediaItem.fromUri(Uri.fromFile(file)))
            prepare()
        }
    }
    DisposableEffect(player) { onDispose { player.release() } }
    AndroidView({ androidx.media3.ui.PlayerView(it).apply { this.player = player } },
        Modifier.width(260.dp).height(180.dp).clip(RoundedCornerShape(18.dp)).background(Color.Black))
}
