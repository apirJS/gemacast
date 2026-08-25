package com.apir.gemacast

import org.junit.Assert.assertEquals
import org.junit.Test

class NotificationPermissionPolicyTest {
    @Test
    fun belowAndroid13NeedsNoRuntimePermission() {
        // Pre-Tiramisu the permission is granted at install time. `granted` can read
        // false there depending on OEM, so the SDK check has to come first.
        assertEquals(
            NotificationPermissionState.NOT_REQUIRED,
            NotificationPermissionPolicy.state(
                sdkInt = 32,
                granted = false,
                hasAsked = false,
                systemWillShowRationale = false,
            ),
        )
        assertEquals(
            NotificationPermissionState.NOT_REQUIRED,
            NotificationPermissionPolicy.state(
                sdkInt = 26,
                granted = true,
                hasAsked = true,
                systemWillShowRationale = true,
            ),
        )
    }

    @Test
    fun aFirstRunIsDeniedNotBlockedEvenThoughTheSystemWouldNotShowARationale() {
        // The rule this whole file exists for. `shouldShowRequestPermissionRationale`
        // is false before the first request AND after a permanent denial, so reading
        // it alone would classify a fresh install as BLOCKED and we would never ask.
        assertEquals(
            NotificationPermissionState.DENIED,
            NotificationPermissionPolicy.state(
                sdkInt = 33,
                granted = false,
                hasAsked = false,
                systemWillShowRationale = false,
            ),
        )
    }

    @Test
    fun theSameSystemAnswerAfterWeHaveAskedMeansBlocked() {
        // Identical `systemWillShowRationale = false` as the case above; only
        // `hasAsked` differs, and it flips the outcome.
        assertEquals(
            NotificationPermissionState.BLOCKED,
            NotificationPermissionPolicy.state(
                sdkInt = 36,
                granted = false,
                hasAsked = true,
                systemWillShowRationale = false,
            ),
        )
    }

    @Test
    fun oneRefusalLeavesTheSystemDialogAvailable() {
        assertEquals(
            NotificationPermissionState.DENIED,
            NotificationPermissionPolicy.state(
                sdkInt = 36,
                granted = false,
                hasAsked = true,
                systemWillShowRationale = true,
            ),
        )
    }

    @Test
    fun grantedWinsOverEveryOtherSignal() {
        assertEquals(
            NotificationPermissionState.GRANTED,
            NotificationPermissionPolicy.state(
                sdkInt = 36,
                granted = true,
                hasAsked = false,
                systemWillShowRationale = true,
            ),
        )
    }

    @Test
    fun startupOnlyInterruptsTheUserWhenAskingCanStillWork() {
        assertEquals(
            NotificationPermissionAction.NOTHING,
            NotificationPermissionPolicy.startupAction(NotificationPermissionState.NOT_REQUIRED),
        )
        assertEquals(
            NotificationPermissionAction.NOTHING,
            NotificationPermissionPolicy.startupAction(NotificationPermissionState.GRANTED),
        )
        assertEquals(
            NotificationPermissionAction.EXPLAIN_THEN_REQUEST,
            NotificationPermissionPolicy.startupAction(NotificationPermissionState.DENIED),
        )
        // Re-requesting when blocked shows nothing at all, so a dialog here would be
        // an interruption with no possible outcome. The in-app notice handles it.
        assertEquals(
            NotificationPermissionAction.DEFER_TO_SETTINGS,
            NotificationPermissionPolicy.startupAction(NotificationPermissionState.BLOCKED),
        )
    }

    @Test
    fun wireValuesMatchWhatTheRustPortParses() {
        // These strings cross JNI into `PlatformService::notification_permission_state`
        // and are matched literally by the frontend, so renaming an enum constant is a
        // breaking change rather than a rename.
        assertEquals("NOT_REQUIRED", NotificationPermissionPolicy.wireValue(NotificationPermissionState.NOT_REQUIRED))
        assertEquals("GRANTED", NotificationPermissionPolicy.wireValue(NotificationPermissionState.GRANTED))
        assertEquals("DENIED", NotificationPermissionPolicy.wireValue(NotificationPermissionState.DENIED))
        assertEquals("BLOCKED", NotificationPermissionPolicy.wireValue(NotificationPermissionState.BLOCKED))
    }
}
