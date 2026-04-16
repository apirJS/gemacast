package com.apir.gemacast

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Binder
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationCompat
import androidx.media.app.NotificationCompat.MediaStyle

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import android.content.Context
import android.net.wifi.WifiManager

class GemaCastService : Service() {
    companion object {
        const val CHANNEL_ID = "GemaCastChannel"
        const val NOTIFICATION_ID = 1
        var isRunning = false
            private set
    }

    inner class LocalBinder : Binder() {
        fun getService(): GemaCastService = this@GemaCastService
    }

    private val binder = LocalBinder()
    private lateinit var mediaSession: MediaSessionCompat
    private lateinit var audioManager: AudioManager
    private var audioFocusRequest: AudioFocusRequest? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var highPerfWifiLock: WifiManager.WifiLock? = null
    private var lowLatencyWifiLock: WifiManager.WifiLock? = null
    private val scope = CoroutineScope(Dispatchers.IO)

    private var isPlayingState = true

    override fun onCreate() {
        super.onCreate()
        audioManager = getSystemService(AUDIO_SERVICE) as AudioManager
        mediaSession = MediaSessionCompat(this, "GemaCastSession").apply {
            setCallback(object : MediaSessionCompat.Callback() {
                override fun onPlay() {
                    sendUdpCommand("RESUME")
                    updatePlaybackState(true)
                }

                override fun onPause() {
                    sendUdpCommand("STOP_STREAM")
                    updatePlaybackState(false)
                }

                override fun onStop() {
                    sendUdpCommand("DISCONNECT")
                }
            })
            isActive = true
        }
        updatePlaybackState(true)
        createNotificationChannel()
        acquireWakeLock()
    }

    private fun updatePlaybackState(playing: Boolean) {
        isPlayingState = playing
        val state = if (playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED
        mediaSession.setPlaybackState(
            PlaybackStateCompat.Builder()
                // 0f playback speed tells the system the seekbar shouldn't progress
                .setState(state, PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN, 0f)
                .setActions(
                    PlaybackStateCompat.ACTION_PLAY or
                    PlaybackStateCompat.ACTION_PAUSE or
                    PlaybackStateCompat.ACTION_PLAY_PAUSE or
                    PlaybackStateCompat.ACTION_STOP
                )
                .build()
        )
        // explicitly clearing duration and providing title/artist 
        // to encourage the lockscreen to treat it as a live radio broadcast
        mediaSession.setMetadata(
            MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, "Streaming audio from PC…")
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, "GemaCast Live")
                .putLong(MediaMetadataCompat.METADATA_KEY_DURATION, -1L)
                .build()
        )
        if (isRunning) {
            buildAndShowNotification()
        }
    }

    private fun acquireWakeLock() {
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        wakeLock = pm.newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK,
            "GemaCast::StreamingWakeLock"
        ).also {
            it.acquire(4 * 60 * 60 * 1000L) // 4 hours max
        }
        
        val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        
        @Suppress("DEPRECATION")
        highPerfWifiLock = wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "GemaCast::StreamingHighPerfWifiLock").also {
            it.acquire()
        }
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            lowLatencyWifiLock = wifiManager.createWifiLock(WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "GemaCast::StreamingLowLatencyWifiLock").also {
                it.acquire()
            }
        }
    }

    private val audioFocusChangeListener = AudioManager.OnAudioFocusChangeListener { focusChange ->
        when (focusChange) {
            AudioManager.AUDIOFOCUS_LOSS,
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT -> {
                // A phone call or other long-running media started, so stop streaming completely.
                sendUdpCommand("DISCONNECT")
                scope.launch {
                    kotlinx.coroutines.delay(300)
                    stopStreaming()
                }
            }
            AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK -> {
                // Ignore or lower volume if supported.
            }
        }
    }

    private fun requestAudioFocus() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val attrs = AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_MEDIA)
                .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                .build()
            audioFocusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(attrs)
                .setAcceptsDelayedFocusGain(true)
                .setOnAudioFocusChangeListener(audioFocusChangeListener)
                .build()
            audioManager.requestAudioFocus(audioFocusRequest!!)
        } else {
            @Suppress("DEPRECATION")
            audioManager.requestAudioFocus(audioFocusChangeListener, AudioManager.STREAM_MUSIC, AudioManager.AUDIOFOCUS_GAIN)
        }
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(
                CHANNEL_ID,
                "GemaCast Background Audio",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Keeps audio streaming active in the background"
                setShowBadge(false)
            }
            (getSystemService(NOTIFICATION_SERVICE) as NotificationManager)
                .createNotificationChannel(channel)
        }
    }

    private var cachedIpcPort: Int? = null

    private fun sendUdpCommand(command: String) {
        scope.launch {
            try {
                if (cachedIpcPort == null) {
                    val ipcPortFile = java.io.File(cacheDir, ".ipc_port")
                    if (ipcPortFile.exists()) {
                        cachedIpcPort = ipcPortFile.readText().trim().toIntOrNull()
                    }
                }
                val port = cachedIpcPort ?: return@launch
                
                val socket = DatagramSocket()
                val data = command.toByteArray()
                val packet = DatagramPacket(data, data.size, InetAddress.getByName("127.0.0.1"), port)
                socket.send(packet)
                socket.close()
            } catch (e: Exception) {
                e.printStackTrace()
            }
        }
    }

    private fun buildAndShowNotification() {
        val openIntent = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val pendingOpenIntent = PendingIntent.getActivity(
            this, 0, openIntent,
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        val disconnectIntent = Intent(this, GemaCastService::class.java).apply { action = "DISCONNECT" }
        val pendingDisconnectIntent = PendingIntent.getService(this, 1, disconnectIntent, PendingIntent.FLAG_IMMUTABLE)

        val playPauseActionText = if (isPlayingState) "Stop" else "Resume"
        val playPauseIcon = if (isPlayingState) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play
        val playPauseIntent = Intent(this, GemaCastService::class.java).apply { action = if (isPlayingState) "STOP_STREAM" else "RESUME" }
        val pendingPlayPauseIntent = PendingIntent.getService(this, 4, playPauseIntent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

        val notification = NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("GemaCast")
            .setContentText(if (isPlayingState) "Streaming audio from PC…" else "Paused")
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentIntent(pendingOpenIntent)
            .setOngoing(isPlayingState)
            .setSilent(true)
            .addAction(playPauseIcon, playPauseActionText, pendingPlayPauseIntent)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Disconnect", pendingDisconnectIntent)
            .setStyle(
                MediaStyle()
                    .setShowActionsInCompactView(0, 1)
                    .setMediaSession(mediaSession.sessionToken)
            )
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            "STOP" -> {
                stopStreaming()
                return START_NOT_STICKY
            }
            "DISCONNECT" -> {
                sendUdpCommand("DISCONNECT")
                // Delete the streaming flag so the activity knows we're done
                java.io.File(cacheDir, ".streaming_active").delete()
                // Brief delay to let the UDP command propagate, then stop the service
                scope.launch {
                    kotlinx.coroutines.delay(300)
                    stopStreaming()
                }
                return START_NOT_STICKY
            }
            "RESUME" -> {
                sendUdpCommand("RESUME")
                updatePlaybackState(true)
                return START_STICKY
            }
            "STOP_STREAM" -> {
                sendUdpCommand("STOP_STREAM")
                // Delete the streaming flag so the activity knows we're done
                java.io.File(cacheDir, ".streaming_active").delete()
                // Brief delay to let the UDP command propagate, then stop the service
                scope.launch {
                    kotlinx.coroutines.delay(300)
                    stopStreaming()
                }
                return START_NOT_STICKY
            }
            else -> { // "START" or null
                isRunning = true
                requestAudioFocus()
                updatePlaybackState(true)
            }
        }

        return START_STICKY
    }

    private fun stopStreaming() {
        isRunning = false
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        highPerfWifiLock?.let { if (it.isHeld) it.release() }
        highPerfWifiLock = null
        lowLatencyWifiLock?.let { if (it.isHeld) it.release() }
        lowLatencyWifiLock = null
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            audioFocusRequest?.let { audioManager.abandonAudioFocusRequest(it) }
        } else {
            @Suppress("DEPRECATION")
            audioManager.abandonAudioFocus(null)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
            @Suppress("DEPRECATION")
            stopForeground(true)
        }
        stopSelf()
    }

    override fun onBind(intent: Intent?): IBinder = binder

    override fun onTaskRemoved(rootIntent: Intent?) {
        // User swiped app from recents — stop the service cleanly
        sendUdpCommand("DISCONNECT")
        stopStreaming()
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        super.onDestroy()
        isRunning = false
        wakeLock?.let { if (it.isHeld) it.release() }
        mediaSession.isActive = false
        mediaSession.release()
    }
}
