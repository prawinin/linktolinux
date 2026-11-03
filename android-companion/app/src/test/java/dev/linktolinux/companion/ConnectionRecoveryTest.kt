package dev.linktolinux.companion

import java.io.DataInputStream
import java.io.DataOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ConnectionRecoveryTest {
    @Test fun disconnectedDesktopReconnectsWithoutAnActivity() {
        ServerSocket(0, 2, InetAddress.getLoopbackAddress()).use { server ->
            val executor = Executors.newSingleThreadExecutor()
            try {
                val peer = executor.submit {
                    repeat(2) {
                        server.accept().use { socket ->
                            socket.soTimeout = 2000
                            val input = DataInputStream(socket.getInputStream())
                            val size = input.readInt()
                            require(size in 1..4096)
                            input.readFully(ByteArray(size))
                            val ack = JSONObject().put("type", "hello_ack").put("protocolVersion", 2).toString().toByteArray()
                            DataOutputStream(socket.getOutputStream()).apply { writeInt(ack.size); write(ack); flush() }
                        }
                    }
                }
                var active = true
                var connections = 0
                val waits = mutableListOf<Long>()
                val endpoint = DesktopEndpoint(server.inetAddress.hostAddress!!, server.localPort, "ABCDEF123456")
                ConnectionRecovery.run({ active }, connect = {
                    DesktopSession(endpoint, "test-phone", "Test phone").use { session ->
                        try {
                            session.run({ connections++; if (connections == 2) { active = false; session.close() } }, {}, { JSONObject().put("type", "ping") })
                            null
                        } catch (error: SessionFailure) { error.kind }
                    }
                }, onRetry = {}, wait = { waits.add(it) })
                peer.get(3, TimeUnit.SECONDS)
                assertEquals(2, connections)
                assertEquals(listOf(1000L), waits)
            } finally { executor.shutdownNow() }
        }
    }
    @Test fun rejectedPairingAndProtocolErrorsNeverRetry() {
        for (failure in listOf(FailureKind.PAIRING, FailureKind.PROTOCOL)) {
            var attempts = 0
            ConnectionRecovery.run({ true }, { attempts++; failure }, {}, { fail("Permanent failures must not retry") })
            assertEquals(1, attempts)
        }
    }
    @Test fun backoffIsBoundedAndDisconnectPreventsFurtherAttempts() {
        var attempts = 0
        var active = true
        val waits = mutableListOf<Long>()
        ConnectionRecovery.run({ active }, { attempts++; FailureKind.NETWORK }, {}, {
            waits.add(it)
            if (waits.size == 20) active = false
        })
        assertEquals(20, attempts)
        assertEquals(listOf(1000L, 2000L, 4000L, 8000L, 16000L, 30000L), waits.take(6))
        assertTrue(waits.all { it in 1000L..30000L })
    }
    @Test fun interruptedBackoffStopsTheWorker() {
        var attempts = 0
        try {
            ConnectionRecovery.run({ true }, { attempts++; FailureKind.TIMEOUT }, {}, { throw InterruptedException() })
            assertEquals(1, attempts)
            assertTrue(Thread.currentThread().isInterrupted)
        } finally { Thread.interrupted() }
    }
    @Test fun stickyRestartRespectsExplicitDisconnectAndMissingPairing() {
        val endpoint = DesktopEndpoint("localhost", 12345, "ABCDEF123456")
        assertEquals(endpoint, ConnectionRecovery.restore(true, endpoint))
        assertNull(ConnectionRecovery.restore(false, endpoint))
        assertNull(ConnectionRecovery.restore(true, null))
    }
}
