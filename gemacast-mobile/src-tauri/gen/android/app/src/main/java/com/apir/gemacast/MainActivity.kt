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
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.PowerManager
import android.provider.Settings
import androidx.annotation.Keep
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

    private var multicastLock: android.net.wifi.WifiManager.MulticastLock? = null

    private fun acquireMulticastLock() {
        try {
            val wifiManager = applicationContext.getSystemService(Context.WIFI_SERVICE) as android.net.wifi.WifiManager
            multicastLock = wifiManager.createMulticastLock("GemaCast::DiscoveryMulticastLock").also {
                it.setReferenceCounted(false)
                it.acquire()
            }
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    private fun releaseMulticastLock() {
        try {
            multicastLock?.let { if (it.isHeld) it.release() }
            multicastLock = null
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    private fun isStreamingActive(): Boolean {
        return File(cacheDir, ".streaming_active").exists()
    }

    @Keep
    fun getTransportType(): String {
        return try {
            val connectivityManager = getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val activeNetwork = connectivityManager.activeNetwork
            val caps = connectivityManager.getNetworkCapabilities(activeNetwork)
            
            val activeTransports = mutableSetOf<String>()
            if (caps != null) {
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) activeTransports.add("WIFI")
                if (caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) activeTransports.add("ETHERNET")
            }
            
            val networkType = if (activeTransports.isEmpty()) "NONE" else activeTransports.joinToString(",")

            val intentFilter = android.content.IntentFilter("android.hardware.usb.action.USB_STATE")
            val usbIntent = registerReceiver(null, intentFilter)
            val usbConnected = usbIntent?.extras?.getBoolean("connected") ?: false

            val adbActive = android.provider.Settings.Global.getInt(
                contentResolver, 
                android.provider.Settings.Global.ADB_ENABLED, 0
            ) != 0

            val adbStatus = if (usbConnected && adbActive) "ADB_ON" else "ADB_OFF"

            "${networkType}|${adbStatus}"
        } catch (e: Exception) {
            "ERROR: ${e.message}"
        }
    }

    @Keep
    fun syncServiceState(action: String, isExclusive: Boolean) {
        try {
            val intent = Intent(this, GemaCastService::class.java).apply {
                this.action = action
                putExtra("EXCLUSIVE_MODE", isExclusive)
            }
            startService(intent)
        } catch (e: Exception) {
            e.printStackTrace()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
                ActivityCompat.requestPermissions(this, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 101)
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
            val powerManager = getSystemService(POWER_SERVICE) as PowerManager
            if (!powerManager.isIgnoringBatteryOptimizations(packageName)) {
                android.app.AlertDialog.Builder(this)
                    .setTitle("Battery Optimization")
                    .setMessage("To prevent audio stuttering when the screen is off, GemaCast needs to be excluded from battery optimizations. Please disable battery optimization for GemaCast in the next screen.")
                    .setPositiveButton("Allow") { _, _ ->
                        try {
                            val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                                data = Uri.parse("package:$packageName")
                            }
                            startActivity(intent)
                        } catch (e: Exception) {
                            e.printStackTrace()
                        }
                    }
                    .setNegativeButton("Not Now", null)
                    .show()
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
        releaseMulticastLock()
        super.onPause() // MUST be called to prevent SuperNotCalledException
    }

    override fun onStop() {
        if (serviceBound) {
            unbindService(serviceConnection)
            serviceBound = false
            gemaCastService = null
        }
        super.onStop() // MUST be called to prevent SuperNotCalledException
    }

    override fun onResume() {
        super.onResume()
        acquireMulticastLock()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
    }

    override fun onDestroy() {
        super.onDestroy()
    }
}
