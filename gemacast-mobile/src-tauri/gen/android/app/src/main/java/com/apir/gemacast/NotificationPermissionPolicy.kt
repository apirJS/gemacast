package com.apir.gemacast

/**
 * What the app is currently able to do about posting notifications.
 *
 * Denying `POST_NOTIFICATIONS` does not stop the foreground service — audio keeps
 * streaming either way. What is lost is the notification itself, and with it the
 * only Stop/Resume and Disconnect controls that exist outside the app (see the
 * two `addAction` calls in `GemaCastService.buildAndShowNotification`). That is
 * why the state is worth surfacing rather than ignoring.
 */
enum class NotificationPermissionState {
    /** Below Android 13: granted at install time, nothing to ask for. */
    NOT_REQUIRED,
    GRANTED,

    /** Refused, but the system will still show a dialog if asked again. */
    DENIED,

    /** Refused for good. Only the system Settings screen can undo this. */
    BLOCKED,
}

/** What to do about the state when the app starts. */
enum class NotificationPermissionAction {
    NOTHING,

    /** Show our own explanation, then hand off to the system dialog. */
    EXPLAIN_THEN_REQUEST,

    /**
     * Asking again is a no-op, so say nothing at startup and let the in-app
     * notice offer the Settings deep link instead.
     */
    DEFER_TO_SETTINGS,
}

/**
 * Pure decision layer for the `POST_NOTIFICATIONS` runtime permission.
 *
 * Split out of `MainActivity` so it is JVM-unit-testable, the same reason
 * `PlaybackStateReducer` and `PcIdentityConfirmationState` are separate files.
 *
 * The rule worth having tests for is [state]: `shouldShowRequestPermissionRationale`
 * answers **false** in two opposite situations — before the very first request,
 * and after the user has permanently denied. Reading it alone therefore cannot
 * distinguish "never asked" from "blocked", and the two need opposite handling.
 * The persisted `hasAsked` flag is what separates them, and it is the only reason
 * that flag exists.
 */
object NotificationPermissionPolicy {
    /** The `Build.VERSION_CODES.TIRAMISU` value, inlined so this file needs no Android imports. */
    const val TIRAMISU = 33

    /**
     * @param sdkInt `Build.VERSION.SDK_INT`.
     * @param granted result of `checkSelfPermission(POST_NOTIFICATIONS) == PERMISSION_GRANTED`.
     * @param hasAsked whether this install has ever reached the system dialog.
     * @param systemWillShowRationale result of `shouldShowRequestPermissionRationale`.
     */
    fun state(
        sdkInt: Int,
        granted: Boolean,
        hasAsked: Boolean,
        systemWillShowRationale: Boolean,
    ): NotificationPermissionState = when {
        sdkInt < TIRAMISU -> NotificationPermissionState.NOT_REQUIRED
        granted -> NotificationPermissionState.GRANTED
        // Not yet asked: the system dialog is still available regardless of what
        // `shouldShowRequestPermissionRationale` says, so this is not BLOCKED.
        !hasAsked -> NotificationPermissionState.DENIED
        systemWillShowRationale -> NotificationPermissionState.DENIED
        else -> NotificationPermissionState.BLOCKED
    }

    fun startupAction(state: NotificationPermissionState): NotificationPermissionAction = when (state) {
        NotificationPermissionState.NOT_REQUIRED,
        NotificationPermissionState.GRANTED,
        -> NotificationPermissionAction.NOTHING
        NotificationPermissionState.DENIED -> NotificationPermissionAction.EXPLAIN_THEN_REQUEST
        NotificationPermissionState.BLOCKED -> NotificationPermissionAction.DEFER_TO_SETTINGS
    }

    /** Wire value read over JNI by the Rust `PlatformService` port. */
    fun wireValue(state: NotificationPermissionState): String = state.name
}
