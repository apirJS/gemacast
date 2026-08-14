package com.apir.gemacast

/**
 * Pure model for the service's authoritative playback state.
 *
 * USER actions are commands only: Android must wait for Rust to acknowledge
 * the operation before changing the MediaSession state. SYNC actions are the
 * authoritative state updates emitted after that acknowledgement.
 */
enum class PlaybackState {
    STOPPED,
    PLAYING,
    PAUSED,
}

data class PlaybackTransition(
    val state: PlaybackState,
    val command: String? = null,
    val cleanup: Boolean = false,
)

object PlaybackStateReducer {
    fun reduce(state: PlaybackState, action: String?): PlaybackTransition = when (action) {
        "USER_PAUSE" -> PlaybackTransition(state, command = "STOP_STREAM")
        "USER_RESUME" -> PlaybackTransition(state, command = "RESUME")
        "USER_DISCONNECT" -> PlaybackTransition(state, command = "DISCONNECT")
        "SYNC_PLAYING" -> PlaybackTransition(PlaybackState.PLAYING)
        "SYNC_PAUSED" -> PlaybackTransition(PlaybackState.PAUSED)
        "SYNC_STOPPED" -> PlaybackTransition(PlaybackState.STOPPED, cleanup = true)
        else -> PlaybackTransition(PlaybackState.STOPPED, cleanup = true)
    }
}
