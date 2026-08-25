package com.apir.gemacast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaybackStateReducerTest {
    @Test
    fun userCommandsDoNotOptimisticallyChangeAuthoritativeState() {
        val paused = PlaybackStateReducer.reduce(PlaybackState.PLAYING, "USER_PAUSE")
        assertEquals(PlaybackState.PLAYING, paused.state)
        assertEquals("STOP_STREAM", paused.command)
        assertFalse(paused.cleanup)

        val resumed = PlaybackStateReducer.reduce(PlaybackState.PAUSED, "USER_RESUME")
        assertEquals(PlaybackState.PAUSED, resumed.state)
        assertEquals("RESUME", resumed.command)

        val disconnected = PlaybackStateReducer.reduce(PlaybackState.PLAYING, "USER_DISCONNECT")
        assertEquals(PlaybackState.PLAYING, disconnected.state)
        assertEquals("DISCONNECT", disconnected.command)
    }

    @Test
    fun syncActionsSetExactAuthoritativeState() {
        assertEquals(
            PlaybackState.PLAYING,
            PlaybackStateReducer.reduce(PlaybackState.STOPPED, "SYNC_PLAYING").state,
        )
        assertEquals(
            PlaybackState.PAUSED,
            PlaybackStateReducer.reduce(PlaybackState.PLAYING, "SYNC_PAUSED").state,
        )
        val stopped = PlaybackStateReducer.reduce(PlaybackState.PAUSED, "SYNC_STOPPED")
        assertEquals(PlaybackState.STOPPED, stopped.state)
        assertTrue(stopped.cleanup)
        assertNull(stopped.command)
    }

    @Test
    fun nullAndUnknownActionsCleanUpTheService() {
        val nullAction = PlaybackStateReducer.reduce(PlaybackState.PLAYING, null)
        assertEquals(PlaybackState.STOPPED, nullAction.state)
        assertTrue(nullAction.cleanup)

        val unknown = PlaybackStateReducer.reduce(PlaybackState.PLAYING, "START")
        assertEquals(PlaybackState.STOPPED, unknown.state)
        assertTrue(unknown.cleanup)
    }
}
