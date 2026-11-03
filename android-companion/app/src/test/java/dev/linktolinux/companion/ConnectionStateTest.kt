package dev.linktolinux.companion

import org.junit.Assert.*
import org.junit.Test

class ConnectionStateTest {
    @Test fun validatesFieldsAndAcceptsCopiedDownloadAddress() {
        val endpoint = DesktopEndpoint.parse("http://desktop.local:29567/download/companion.apk", "1718", "abcd ef12-3456")
        assertEquals("desktop.local", endpoint.host)
        assertEquals(29567, endpoint.port)
        assertEquals("ABCDEF123456", endpoint.code)
        for ((host, port, code, field) in listOf(
            listOf("", "1718", "ABCDEF123456", "HOST"),
            listOf("desktop.local", "0", "ABCDEF123456", "PORT"),
            listOf("desktop.local", "1718", "", "CODE")
        )) {
            try { DesktopEndpoint.parse(host, port, code); fail("Invalid input accepted") }
            catch (error: ConnectionInputException) { assertEquals(field, error.field.name) }
        }
    }
    @Test fun ipv6AddressCanBeReusedAfterNormalization() {
        val first = DesktopEndpoint.parse("[2001:db8::1]", "1718", "ABCDEF123456")
        assertEquals(first, DesktopEndpoint.parse(first.host, first.port.toString(), first.code))
    }
    @Test fun controlsReflectConnectionPermissionsAndCapture() {
        val ready = PermissionSnapshot(true, true, true, true)
        assertFalse(ready.needsAttention)
        assertTrue(ready.copy(camera = false).needsAttention)
        val connecting = ConnectionUiState(ConnectionPhase.CONNECTING, "Connecting")
        assertTrue(connecting.showProgress)
        assertEquals("Cancel connection", connecting.connectLabel)
        assertFalse(connecting.showStopCapture)
        assertFalse(connecting.canSendClipboard)
        val live = ConnectionUiState(ConnectionPhase.CONNECTED, "Connected", true)
        assertTrue(live.showStopCapture)
        assertTrue(live.canSendClipboard)
        assertFalse(live.copy(captureActive = false).showStopCapture)
    }
}
