package com.apir.gemacast

internal data class ViewportPadding(
    val left: Int,
    val top: Int,
    val right: Int,
    val bottom: Int,
) {
    operator fun plus(other: ViewportPadding) =
        ViewportPadding(
            left + other.left,
            top + other.top,
            right + other.right,
            bottom + other.bottom,
        )
}
