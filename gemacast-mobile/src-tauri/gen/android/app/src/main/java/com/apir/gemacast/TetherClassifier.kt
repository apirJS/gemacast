package com.apir.gemacast

/**
 * Classifies the phone's *tethering* interfaces so a Wi-Fi hotspot is never
 * mistaken for a USB cable.
 *
 * This is a pure function over `(interfaceName, ipv4)` pairs, deliberately split
 * out of [MainActivity] so it is JVM-unit-testable — the Android APIs that would
 * answer the question directly are not available to us:
 * `WifiManager.getSoftApConfiguration()` and `registerSoftApCallback()` are
 * `@SystemApi` gated on the signature permission `NETWORK_SETTINGS`, and the
 * older `getWifiApConfiguration()` has been blocked for non-system apps since
 * Android 9. So we infer from the tethering interface instead.
 *
 * Android assigns a fixed gateway per tethering transport, which is a stronger
 * signal than the vendor-specific interface name:
 *
 * | transport | gateway |
 * | --- | --- |
 * | USB / RNDIS | `192.168.42.1` |
 * | Wi-Fi hotspot (soft AP) | `192.168.43.1` |
 * | Bluetooth | `192.168.44.1` |
 *
 * Android 11+ can randomise the hotspot subnet, so names are the backstop.
 *
 * Note this reports only *that* a hotspot is up, never its band. The band is not
 * readable here at all — but it does not need to be: the PC is a Wi-Fi client of
 * this AP, so the PC measures the real channel and `LinkPair::effective_link()`
 * resolves the pair. Reporting a band from this side would override that
 * measurement with a guess.
 */
object TetherClassifier {
    /** One network interface, reduced to what the classification needs. */
    data class Iface(val name: String, val ipv4: List<String>)

    enum class Tether {
        /** Phone is sharing over Wi-Fi (soft AP) — a radio link. */
        HOTSPOT,

        /** Phone is sharing over a USB cable (RNDIS). */
        USB,

        /** Not tethering. */
        NONE,
    }

    private val USB_SUBNETS = listOf("192.168.42.", "192.168.45.", "172.20.10.")
    private const val HOTSPOT_SUBNET = "192.168.43."

    /**
     * Soft-AP interface names by vendor. None of these contain "wlan", which is
     * exactly why a name-only check misfiled them as cables.
     */
    private fun isSoftApName(name: String): Boolean {
        val n = name.lowercase()
        return n.startsWith("ap") || n.contains("softap") || n.contains("swlan") || n.contains("p2p")
    }

    private fun isUsbName(name: String): Boolean {
        val n = name.lowercase()
        return n.startsWith("usb") || n.contains("rndis") || n.contains("ndis")
    }

    /**
     * Returns the active tethering transport.
     *
     * USB wins a tie: when both a cable and a hotspot are up, the cable is the
     * better link and is what the user is most likely streaming over.
     */
    fun classify(interfaces: List<Iface>): Tether {
        var sawHotspot = false

        for (iface in interfaces) {
            if (iface.ipv4.isEmpty()) continue

            // Subnet evidence first — it does not depend on vendor naming.
            if (iface.ipv4.any { ip -> USB_SUBNETS.any { ip.startsWith(it) } }) return Tether.USB
            if (iface.ipv4.any { it.startsWith(HOTSPOT_SUBNET) }) {
                sawHotspot = true
                continue
            }

            if (isUsbName(iface.name)) return Tether.USB
            if (isSoftApName(iface.name)) sawHotspot = true
        }

        return if (sawHotspot) Tether.HOTSPOT else Tether.NONE
    }
}
