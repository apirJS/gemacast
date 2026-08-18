package com.apir.gemacast

import com.apir.gemacast.TetherClassifier.Iface
import com.apir.gemacast.TetherClassifier.Tether
import org.junit.Assert.assertEquals
import org.junit.Test

class TetherClassifierTest {
    @Test
    fun hotspotSubnetIsAWifiHotspotNotACable() {
        // The reported bug: a soft-AP interface used to be classified as USB,
        // which disabled the Wi-Fi mode button and picked a cable jitter profile.
        assertEquals(
            Tether.HOTSPOT,
            TetherClassifier.classify(listOf(Iface("ap0", listOf("192.168.43.1")))),
        )
    }

    @Test
    fun usbSubnetIsStillUsb() {
        assertEquals(
            Tether.USB,
            TetherClassifier.classify(listOf(Iface("rndis0", listOf("192.168.42.1")))),
        )
        assertEquals(
            Tether.USB,
            TetherClassifier.classify(listOf(Iface("rndis0", listOf("192.168.45.1")))),
        )
        assertEquals(
            Tether.USB,
            TetherClassifier.classify(listOf(Iface("bridge100", listOf("172.20.10.1")))),
        )
    }

    @Test
    fun vendorSoftApNamesAreRecognisedWhenTheSubnetIsRandomised() {
        // Android 11+ may hand the hotspot a subnet outside 192.168.43.x, so the
        // name has to carry the classification on its own.
        for (name in listOf("ap0", "softap0", "swlan0", "p2p0")) {
            assertEquals(
                "$name should classify as HOTSPOT",
                Tether.HOTSPOT,
                TetherClassifier.classify(listOf(Iface(name, listOf("10.42.7.1")))),
            )
        }
    }

    @Test
    fun usbNamesAreRecognisedWhenTheSubnetIsRandomised() {
        for (name in listOf("usb0", "rndis0")) {
            assertEquals(
                "$name should classify as USB",
                Tether.USB,
                TetherClassifier.classify(listOf(Iface(name, listOf("10.42.7.1")))),
            )
        }
    }

    @Test
    fun ordinaryClientInterfacesAreNotTethering() {
        assertEquals(
            Tether.NONE,
            TetherClassifier.classify(
                listOf(
                    Iface("wlan0", listOf("192.168.1.70")),
                    Iface("lo", listOf("127.0.0.1")),
                    Iface("rmnet_data0", listOf("10.114.7.22")),
                ),
            ),
        )
    }

    @Test
    fun interfacesWithoutAnAddressAreIgnored() {
        // A soft-AP interface exists but is down: no address, so not tethering.
        assertEquals(
            Tether.NONE,
            TetherClassifier.classify(listOf(Iface("ap0", emptyList()))),
        )
    }

    @Test
    fun cableWinsWhenBothTransportsAreUp() {
        // Order must not decide the answer, so assert both orderings.
        val ifaces = listOf(Iface("ap0", listOf("192.168.43.1")), Iface("rndis0", listOf("192.168.42.1")))
        assertEquals(Tether.USB, TetherClassifier.classify(ifaces))
        assertEquals(Tether.USB, TetherClassifier.classify(ifaces.reversed()))
    }

    @Test
    fun anEmptyInterfaceListIsNotTethering() {
        assertEquals(Tether.NONE, TetherClassifier.classify(emptyList()))
    }
}
