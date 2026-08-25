package com.apir.gemacast

internal data class PcIdentityConfirmationKey(
    val pcId: String,
    val fingerprint: String,
    val pairingCode: String,
)

internal class PcIdentityConfirmationState(
    private val timeoutMillis: Long = DEFAULT_TIMEOUT_MILLIS,
) {
    enum class BeginResult {
        STARTED,
        PENDING,
        APPROVED,
        REJECTED,
        BUSY,
    }

    enum class PollResult {
        PENDING,
        APPROVED,
        REJECTED,
        MISMATCH,
    }

    private data class PendingConfirmation(
        val key: PcIdentityConfirmationKey,
        val expiresAtMillis: Long,
        var result: PollResult = PollResult.PENDING,
    )

    private var pending: PendingConfirmation? = null

    @Synchronized
    fun begin(key: PcIdentityConfirmationKey, nowMillis: Long): BeginResult {
        val current = pending
        if (current == null || nowMillis >= current.expiresAtMillis) {
            pending = PendingConfirmation(key, nowMillis + timeoutMillis)
            return BeginResult.STARTED
        }
        if (current.key != key) {
            return BeginResult.BUSY
        }
        return when (current.result) {
            PollResult.PENDING -> BeginResult.PENDING
            PollResult.APPROVED -> BeginResult.APPROVED
            PollResult.REJECTED -> BeginResult.REJECTED
            PollResult.MISMATCH -> error("A stored confirmation cannot have a mismatched result")
        }
    }

    @Synchronized
    fun complete(
        key: PcIdentityConfirmationKey,
        approved: Boolean,
        nowMillis: Long,
    ): Boolean {
        val current = pending ?: return false
        if (current.key != key || nowMillis >= current.expiresAtMillis) {
            if (nowMillis >= current.expiresAtMillis) {
                pending = null
            }
            return false
        }
        current.result = if (approved) PollResult.APPROVED else PollResult.REJECTED
        return true
    }

    @Synchronized
    fun poll(key: PcIdentityConfirmationKey, nowMillis: Long): PollResult {
        val current = pending ?: return PollResult.MISMATCH
        if (current.key != key) {
            return PollResult.MISMATCH
        }
        if (nowMillis >= current.expiresAtMillis) {
            pending = null
            return PollResult.REJECTED
        }
        val result = current.result
        if (result != PollResult.PENDING) {
            pending = null
        }
        return result
    }

    @Synchronized
    fun cancel(key: PcIdentityConfirmationKey): Boolean {
        if (pending?.key != key) {
            return false
        }
        pending = null
        return true
    }

    @Synchronized
    fun cancelAll() {
        pending = null
    }

    companion object {
        const val DEFAULT_TIMEOUT_MILLIS = 65_000L
    }
}
