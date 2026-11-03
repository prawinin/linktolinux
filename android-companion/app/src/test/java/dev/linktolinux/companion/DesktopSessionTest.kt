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

class DesktopSessionTest {
    private fun receive(input: DataInputStream): JSONObject {
        val count = input.readInt()
        require(count in 1..65536)
        val bytes = ByteArray(count)
        input.readFully(bytes)
        return JSONObject(String(bytes, Charsets.UTF_8))
    }
    private fun reply(output: DataOutputStream, message: JSONObject) {
        val bytes = message.toString().toByteArray(Charsets.UTF_8)
        output.writeInt(bytes.size)
        output.write(bytes)
        output.flush()
    }
    @Test(timeout = 5000) fun completesRealHandshakeAndDeliversCommands() {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val executor = Executors.newSingleThreadExecutor()
            try {
                val peer = executor.submit {
                    server.accept().use { socket ->
                        socket.soTimeout = 2000
                        val input = DataInputStream(socket.getInputStream())
                        val output = DataOutputStream(socket.getOutputStream())
                        val hello = receive(input)
                        assertEquals("hello", hello.getString("type"))
                        assertEquals("files-v1", hello.getJSONArray("features").getString(0))
                        assertEquals("ABCDEF123456", hello.getString("token"))
                        reply(output, JSONObject().put("type", "hello_ack").put("protocolVersion", 2))
                        reply(output, JSONObject().put("type", "request").put("command", "capture_stop").put("id", "stop"))
                    }
                }
                val session = DesktopSession(DesktopEndpoint(server.inetAddress.hostAddress!!, server.localPort, "ABCDEF123456"), "test-phone", "Test phone")
                var connected = false
                var command = ""
                try {
                    session.use {
                        it.run(onConnected = { connected = true; assertTrue(session.serverFeatures.isEmpty()) }, onMessage = { message ->
                            command = message.optString("command")
                            Thread.currentThread().interrupt()
                            session.close()
                        }, heartbeat = { JSONObject().put("type", "ping") })
                    }
                } finally { Thread.interrupted() }
                peer.get(2, TimeUnit.SECONDS)
                assertTrue(connected)
                assertEquals("capture_stop", command)
            } finally { executor.shutdownNow() }
        }
    }
    @Test(timeout = 5000) fun pairingRejectionIsSpecificAndDoesNotConnect() {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val executor = Executors.newSingleThreadExecutor()
            try {
                val peer = executor.submit {
                    server.accept().use { socket ->
                        receive(DataInputStream(socket.getInputStream()))
                        reply(DataOutputStream(socket.getOutputStream()), JSONObject().put("type", "error").put("code", "pairing_rejected"))
                    }
                }
                val session = DesktopSession(DesktopEndpoint(server.inetAddress.hostAddress!!, server.localPort, "ABCDEF123456"), "test-phone", "Test phone")
                try { session.use { it.run({}, {}, { JSONObject() }) }; fail("Rejected pairing connected") }
                catch (error: SessionFailure) { assertEquals(FailureKind.PAIRING, error.kind) }
                peer.get(2, TimeUnit.SECONDS)
            } finally { executor.shutdownNow() }
        }
    }
    @Test(timeout = 5000) fun refusedConnectionHasActionableFailure() {
        val reservation = ServerSocket(0, 1, InetAddress.getLoopbackAddress())
        val port = reservation.localPort
        val host = reservation.inetAddress.hostAddress!!
        reservation.close()
        try {
            DesktopSession(DesktopEndpoint(host, port, "ABCDEF123456"), "test-phone", "Test phone").use { it.run({}, {}, { JSONObject() }) }
            fail("Closed listener connected")
        } catch (error: SessionFailure) { assertEquals(FailureKind.REFUSED, error.kind) }
    }
    @Test(timeout = 5000) fun exposesFeaturesBeforeConnectedCallback() {
        ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
            val executor = Executors.newSingleThreadExecutor()
            try {
                val peer = executor.submit {
                    server.accept().use { socket ->
                        receive(DataInputStream(socket.getInputStream()))
                        reply(DataOutputStream(socket.getOutputStream()), JSONObject().put("type", "hello_ack").put("protocolVersion", 2).put("features", org.json.JSONArray().put("files-v1")))
                    }
                }
                DesktopSession(DesktopEndpoint(server.inetAddress.hostAddress!!, server.localPort, "ABCDEF123456"), "test-phone", "Test phone").use { session ->
                    session.run({ assertEquals(setOf("files-v1"), session.serverFeatures); session.close() }, {}, { JSONObject() })
                }
                peer.get(2, TimeUnit.SECONDS)
            } finally { executor.shutdownNow() }
        }
    }

    @Test(timeout = 5000) fun interruptedHandshakeIsRetriableWithoutRejectingPairing() {
        for (malformed in listOf(false, true)) {
            ServerSocket(0, 1, InetAddress.getLoopbackAddress()).use { server ->
                val executor = Executors.newSingleThreadExecutor()
                try {
                    val peer = executor.submit {
                        server.accept().use { socket ->
                            socket.soTimeout = 2000
                            receive(DataInputStream(socket.getInputStream()))
                            if (malformed) reply(DataOutputStream(socket.getOutputStream()), JSONObject().put("type", "unexpected"))
                        }
                    }
                    try {
                        DesktopSession(DesktopEndpoint(server.inetAddress.hostAddress!!, server.localPort, "ABCDEF123456"), "test-phone", "Test phone").use {
                            it.run({}, {}, { JSONObject() })
                        }
                        fail("Incomplete handshake connected")
                    } catch (error: SessionFailure) {
                        assertEquals(if (malformed) FailureKind.PROTOCOL else FailureKind.NETWORK, error.kind)
                    }
                    peer.get(2, TimeUnit.SECONDS)
                } finally { executor.shutdownNow() }
            }
        }
    }
}
