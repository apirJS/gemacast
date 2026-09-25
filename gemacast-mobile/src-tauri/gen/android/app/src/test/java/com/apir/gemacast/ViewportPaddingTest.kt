package com.apir.gemacast

import org.junit.Assert.assertEquals
import org.junit.Test

class ViewportPaddingTest {
    @Test
    fun chargingOverlayInsetsReturnToTheOriginalPaddingWhenTheOverlayCloses() {
        val base = ViewportPadding(0, 0, 0, 0)
        val normal = ViewportPadding(0, 24, 0, 24)
        val chargingOverlay = ViewportPadding(0, 60, 0, 24)

        assertEquals(ViewportPadding(0, 60, 0, 24), base + chargingOverlay)
        assertEquals(ViewportPadding(0, 24, 0, 24), base + normal)
    }

    @Test
    fun systemBarInsetsPreserveExistingContainerPadding() {
        val base = ViewportPadding(8, 4, 8, 4)
        val bars = ViewportPadding(0, 24, 0, 20)

        assertEquals(ViewportPadding(8, 28, 8, 24), base + bars)
    }
}
