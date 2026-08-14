package com.apir.gemacast

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PcIdentityConfirmationStateTest {
    private val key = PcIdentityConfirmationKey(
        pcId = "pc-1",
        fingerprint = "ab".repeat(32),
        pairingCode = "123456",
    )

    @Test
    fun approvalCanOnlyBePolledByTheExactRequest() {
        val state = PcIdentityConfirmationState()
        val otherCode = key.copy(pairingCode = "654321")

        assertEquals(PcIdentityConfirmationState.BeginResult.STARTED, state.begin(key, 1_000))
        assertTrue(state.complete(key, approved = true, nowMillis = 2_000))
        assertEquals(
            PcIdentityConfirmationState.PollResult.MISMATCH,
            state.poll(otherCode, 2_001),
        )
        assertEquals(
            PcIdentityConfirmationState.PollResult.APPROVED,
            state.poll(key, 2_002),
        )
        assertEquals(
            PcIdentityConfirmationState.PollResult.MISMATCH,
            state.poll(key, 2_003),
        )
    }

    @Test
    fun expiredConfirmationRejectsLateDialogCallbacks() {
        val state = PcIdentityConfirmationState(timeoutMillis = 100)

        assertEquals(PcIdentityConfirmationState.BeginResult.STARTED, state.begin(key, 1_000))
        assertEquals(
            PcIdentityConfirmationState.PollResult.REJECTED,
            state.poll(key, 1_100),
        )
        assertFalse(state.complete(key, approved = true, nowMillis = 1_101))
    }

    @Test
    fun anotherPairingCannotReplaceAnActiveDialog() {
        val state = PcIdentityConfirmationState()
        val otherPc = key.copy(pcId = "pc-2")

        assertEquals(PcIdentityConfirmationState.BeginResult.STARTED, state.begin(key, 1_000))
        assertEquals(PcIdentityConfirmationState.BeginResult.BUSY, state.begin(otherPc, 1_001))
        assertTrue(state.cancel(key))
        assertEquals(PcIdentityConfirmationState.BeginResult.STARTED, state.begin(otherPc, 1_002))
    }
}
