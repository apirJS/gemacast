package com.apir.gemacast

import android.view.View
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

internal class WebViewSystemBars(private val root: View) {
    private val basePadding = root.viewportPadding()
    private val barTypes =
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()

    fun install() {
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(barTypes)
            val padding = basePadding + ViewportPadding(bars.left, bars.top, bars.right, bars.bottom)
            if (view.viewportPadding() != padding) {
                view.setPadding(padding.left, padding.top, padding.right, padding.bottom)
            }

            // The WebView must not apply system-bar insets a second time.
            WindowInsetsCompat.Builder(insets).setInsets(barTypes, Insets.NONE).build()
        }
        root.addOnLayoutChangeListener { _, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom ->
            if (left != oldLeft || top != oldTop || right != oldRight || bottom != oldBottom) {
                refresh()
            }
        }
        refresh()
    }

    fun refresh() {
        root.post { ViewCompat.requestApplyInsets(root) }
    }
}

private fun View.viewportPadding() =
    ViewportPadding(paddingLeft, paddingTop, paddingRight, paddingBottom)
