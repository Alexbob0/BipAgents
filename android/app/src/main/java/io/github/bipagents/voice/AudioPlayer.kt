package io.github.bipagents.voice

import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.mutableStateListOf
import androidx.core.content.ContextCompat
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MediaMetadata
import androidx.media3.common.Player
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.session.DefaultMediaNotificationProvider
import androidx.media3.session.MediaController
import androidx.media3.session.MediaSession
import androidx.media3.session.MediaSessionService
import androidx.media3.session.SessionToken
import io.github.bipagents.MainActivity
import io.github.bipagents.R
import io.github.bipagents.model.Look
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.io.File

/** What plays: a file an agent sent, its title, the agent (lock screen, artwork). */
data class NowPlaying(val file: File, val title: String, val artist: String? = null, val look: Look? = null)

/** The audio the agents send (a podcast, a story), as VoiceNotePlayer.shared: one at a time, ±15 s, speeds, playing on
 *  with the screen off, controls on the lock screen and in the notification (through [PlaybackService]). */
object AudioPlayer {
    data class State(val now: NowPlaying? = null, val playing: Boolean = false, val position: Long = 0, val duration: Long = 0,
                     val rate: Float = 1f)

    val rates = listOf(0.5f, 1f, 1.25f, 1.5f, 2f)
    private val _state = MutableStateFlow(State())
    val state: StateFlow<State> = _state.asStateFlow()
    /** Audio cards on screen (by file): the mini player shows when the one playing is out of sight. */
    val cardsOnScreen = mutableStateListOf<String>()

    private var controller: MediaController? = null
    private var connecting = false
    private val waiting = mutableListOf<(MediaController) -> Unit>()
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var ticker: Job? = null

    private val listener = object : Player.Listener {
        override fun onEvents(player: Player, events: Player.Events) = sync(player)
    }

    private fun with(context: Context, action: (MediaController) -> Unit) {
        controller?.let { return action(it) }
        waiting += action
        if (connecting) return
        connecting = true
        val app = context.applicationContext
        _state.update { it.copy(rate = app.getSharedPreferences("settings", Context.MODE_PRIVATE).getFloat("playbackRate", 1f)) }
        val future = MediaController.Builder(app, SessionToken(app, ComponentName(app, PlaybackService::class.java))).buildAsync()
        future.addListener({
            connecting = false
            val c = runCatching { future.get() }.getOrNull() ?: return@addListener waiting.clear()
            controller = c
            c.addListener(listener)
            waiting.toList().also { waiting.clear() }.forEach { it(c) }
        }, ContextCompat.getMainExecutor(app))
    }

    private fun item(context: Context, now: NowPlaying): MediaItem {
        val metadata = MediaMetadata.Builder().setTitle(now.title).setArtist(now.artist)
        now.look?.let { look ->
            val png = java.io.ByteArrayOutputStream().also { io.github.bipagents.ui.mascotBitmap(look, 512)
                .compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            metadata.setArtworkData(png, MediaMetadata.PICTURE_TYPE_FRONT_COVER)
        }
        return MediaItem.Builder().setUri(Uri.fromFile(now.file)).setMediaId(now.file.path).setMediaMetadata(metadata.build()).build()
    }

    private fun load(context: Context, c: MediaController, now: NowPlaying) {
        Speaker.stop()
        c.setMediaItem(item(context, now))
        c.prepare()
        c.setPlaybackSpeed(_state.value.rate)
        _state.update { it.copy(now = now, position = 0) }
    }

    fun isLoaded(file: File) = _state.value.now?.file == file

    /** Play / pause; another file replaces the one playing. */
    fun toggle(context: Context, now: NowPlaying) = with(context) { c ->
        if (!isLoaded(now.file)) { load(context, c, now); c.play(); return@with }
        when {
            c.isPlaying -> c.pause()
            else -> { if (c.playbackState == Player.STATE_ENDED) c.seekTo(0); Speaker.stop(); c.play() }
        }
    }

    fun seek(context: Context, now: NowPlaying, ms: Long) = with(context) { c ->
        if (!isLoaded(now.file)) load(context, c, now)
        c.seekTo(ms.coerceAtLeast(0))
        _state.update { it.copy(position = ms.coerceAtLeast(0)) }
    }

    fun skip(context: Context, ms: Long) = with(context) { c -> c.seekTo((c.currentPosition + ms).coerceIn(0, maxOf(0, c.duration))) }

    fun setRate(context: Context, rate: Float) {
        context.getSharedPreferences("settings", Context.MODE_PRIVATE).edit().putFloat("playbackRate", rate).apply()
        _state.update { it.copy(rate = rate) }
        controller?.setPlaybackSpeed(rate)
    }

    fun pause() { controller?.pause() }

    fun stop() {
        controller?.run { stop(); clearMediaItems() }
        _state.update { State(rate = it.rate) }
    }

    private fun sync(player: Player) {
        _state.update {
            it.copy(playing = player.isPlaying, position = player.currentPosition,
                duration = player.duration.takeIf { d -> d != C.TIME_UNSET } ?: it.duration)
        }
        if (player.mediaItemCount == 0) _state.update { State(rate = it.rate) }
        if (player.isPlaying && ticker?.isActive != true) ticker = scope.launch {
            while (isActive && player.isPlaying) {
                _state.update { it.copy(position = player.currentPosition) }
                delay(250)
            }
        }
    }
}

/** Holds the player while audio plays (screen off, app in the background) and gives the system its controls. */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
class PlaybackService : MediaSessionService() {
    private var session: MediaSession? = null

    override fun onCreate() {
        super.onCreate()
        val player = ExoPlayer.Builder(this)
            .setAudioAttributes(AudioAttributes.Builder().setUsage(C.USAGE_MEDIA).setContentType(C.AUDIO_CONTENT_TYPE_SPEECH).build(), true)
            .setHandleAudioBecomingNoisy(true)
            .setSeekBackIncrementMs(15_000).setSeekForwardIncrementMs(15_000)
            .build()
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        // −15 s / +15 s on the lock screen and in the notification, as on the iPhone (not « previous / next »).
        val back = androidx.media3.session.CommandButton.Builder(androidx.media3.session.CommandButton.ICON_SKIP_BACK_15)
            .setPlayerCommand(Player.COMMAND_SEEK_BACK).setDisplayName(getString(R.string.back_15))
            .setSlots(androidx.media3.session.CommandButton.SLOT_BACK).build()
        val forward = androidx.media3.session.CommandButton.Builder(androidx.media3.session.CommandButton.ICON_SKIP_FORWARD_15)
            .setPlayerCommand(Player.COMMAND_SEEK_FORWARD).setDisplayName(getString(R.string.forward_15))
            .setSlots(androidx.media3.session.CommandButton.SLOT_FORWARD).build()
        session = MediaSession.Builder(this, player).setSessionActivity(open).setMediaButtonPreferences(listOf(back, forward)).build()
        setMediaNotificationProvider(DefaultMediaNotificationProvider.Builder(this).build().apply { setSmallIcon(R.drawable.ic_notification) })
    }

    override fun onGetSession(controllerInfo: MediaSession.ControllerInfo): MediaSession? = session

    override fun onTaskRemoved(rootIntent: Intent?) {
        val player = session?.player
        if (player == null || !player.playWhenReady || player.mediaItemCount == 0) stopSelf()
    }

    override fun onDestroy() {
        session?.run { player.release(); release() }
        session = null
        super.onDestroy()
    }
}
