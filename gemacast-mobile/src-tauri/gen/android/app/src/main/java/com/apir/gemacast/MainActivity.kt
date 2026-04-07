package com.apir.gemacast

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.activity.enableEdgeToEdge
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import java.io.File

class MainActivity : TauriActivity() {
    private var gemaCastService: GemaCastService? = null
    private var serviceBound = false

    private val serviceConnection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            val binder = service as GemaCastService.LocalBinder
            gemaCastService = binder.getService()
            serviceBound = true
        }

        override fun onServiceDisconnected(name: ComponentName?) {
            gemaCastService = null
            serviceBound = false
        }
    }

    private fun isStreamingActive(): Boolean {
        return File(cacheDir, ".streaming_active").exists()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 101)
            }
        }
    }

    override fun onStart() {
        super.onStart()
        Intent(this, GemaCastService::class.java).also { intent ->
            bindService(intent, serviceConnection, Context.BIND_AUTO_CREATE)
        }
    }

    override fun onPause() {
        if (isStreamingActive()) {
            val sIntent = Intent(this, GemaCastService::class.java).apply { action = "START" }
            try {
                ContextCompat.startForegroundService(this, sIntent)
            } catch (e: Exception) {
                e.printStackTrace()
            }
        }
        super.onPause() // MUST be called to prevent SuperNotCalledException
    }

    override fun onStop() {
        super.onStop() // MUST be called to prevent SuperNotCalledException
    }

    override fun onResume() {
        super.onResume()

        // If the user disconnected from the notification while the app was in background,
        // the flag will be gone but the service may still be running. Clean it up.
        if (GemaCastService.isRunning && !isStreamingActive()) {
            val sIntent = Intent(this, GemaCastService::class.java).apply { action = "STOP" }
            try { startService(sIntent) } catch (_: Exception) {}
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
    }

    override fun onDestroy() {
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
        }
        super.onDestroy()
    }
}
