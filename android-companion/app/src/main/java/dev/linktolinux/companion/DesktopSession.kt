package dev.linktolinux.companion

import java.io.Closeable
import java.io.DataInputStream
import java.io.DataOutputStream
import java.io.EOFException
import java.io.IOException
import java.net.ConnectException
import java.net.InetSocketAddress
import java.net.Socket
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.json.JSONObject

enum class FailureKind { PAIRING, REFUSED, TIMEOUT, NETWORK, PROTOCOL }
class SessionFailure(val kind: FailureKind, message: String, cause: Throwable? = null) : IOException(message, cause)

class DesktopSession(private val endpoint: DesktopEndpoint, private val deviceId: String, private val deviceName: String) : Closeable {
    @Volatile var serverFeatures: Set<String> = emptySet()
        private set
    private val socket = Socket()
    private val queue = ArrayBlockingQueue<JSONObject>(64)
    @Volatile private var closed = false
    @Volatile private var writerFailure: IOException? = null
    fun send(message: JSONObject): Boolean {
        if (closed) return false
        if (queue.offer(message)) return true
        writerFailure = IOException("Connection is too slow. Reconnect on a stronger Wi-Fi network.")
        socket.close()
        return false
    }
    fun run(onConnected: () -> Unit, onMessage: (JSONObject) -> Unit, heartbeat: () -> JSONObject) {
        var writer: Thread? = null
        val timer = Executors.newSingleThreadScheduledExecutor()
        try {
            if (closed) return
            socket.connect(InetSocketAddress(endpoint.host, endpoint.port), 5000)
            socket.soTimeout = 10000
            socket.tcpNoDelay = true
            socket.keepAlive = true
            val input = DataInputStream(socket.getInputStream())
            val output = DataOutputStream(socket.getOutputStream())
            write(output, JSONObject().put("type", "hello").put("protocolVersion", 2)
                .put("deviceId", deviceId).put("deviceName", deviceName).put("token", endpoint.code).put("features", org.json.JSONArray().put("files-v1")))
            val ack = read(input)
            if (ack.optString("type") == "error" && ack.optString("code") == "pairing_rejected")
                throw SessionFailure(FailureKind.PAIRING, "Pairing rejected. Copy the current pairing code from Linux.")
            if (ack.optString("type") != "hello_ack")
                throw SessionFailure(FailureKind.PROTOCOL, "Unexpected desktop handshake. Update both apps and check the desktop port.")
            if (ack.optInt("protocolVersion", 2) != 2) throw SessionFailure(FailureKind.PROTOCOL, "Update both the Linux app and Android companion.")
            val features = ack.optJSONArray("features")
            serverFeatures = if (features == null) emptySet() else (0 until features.length()).mapNotNull { features.opt(it) as? String }.toSet()
            socket.soTimeout = 180000
            onConnected()
            writer = Thread({
                try {
                    while (!closed) write(output, queue.poll(5, TimeUnit.SECONDS) ?: continue)
                } catch (error: InterruptedException) { Thread.currentThread().interrupt() }
                catch (error: IOException) { writerFailure = error; socket.close() }
            }, "companion-writer").apply { start() }
            timer.scheduleWithFixedDelay({ if (!closed) send(heartbeat()) }, 0, 5, TimeUnit.SECONDS)
            while (!closed) onMessage(read(input))
        } catch (error: Exception) {
            if (!closed) throw when (error) {
                is SessionFailure -> error
                is ConnectException -> SessionFailure(FailureKind.REFUSED, "Desktop refused the connection. Open LinkToLinux on Linux and check its port.", error)
                is SocketTimeoutException -> SessionFailure(FailureKind.TIMEOUT, "Connection timed out. Check both devices use the same Wi-Fi and allow LinkToLinux through the desktop firewall.", error)
                is UnknownHostException -> SessionFailure(FailureKind.NETWORK, "Desktop address could not be found. Check the address shown in Linux.", error)
                is EOFException -> SessionFailure(FailureKind.NETWORK, "Desktop connection interrupted. Reconnecting when Linux is available.", error)
                else -> SessionFailure(FailureKind.NETWORK, writerFailure?.message ?: "Connection lost. Check Wi-Fi and reconnect.", error)
            }
        } finally {
            close()
            timer.shutdownNow()
            writer?.interrupt()
            try { writer?.join(1000) }
            catch (error: InterruptedException) { Thread.currentThread().interrupt() }
        }
    }
    override fun close() { closed = true; socket.close() }
    private fun read(input: DataInputStream): JSONObject {
        val idleTimeout = socket.soTimeout
        val first = input.read()
        if (first < 0) throw EOFException()
        socket.soTimeout = 30000
        try {
            val length = (first shl 24) or (input.readUnsignedByte() shl 16) or
                (input.readUnsignedByte() shl 8) or input.readUnsignedByte()
            if (length !in 1..(32 * 1024 * 1024)) throw SessionFailure(FailureKind.PROTOCOL, "Desktop sent an invalid frame. Update both apps.")
            val bytes = ByteArray(length)
            input.readFully(bytes)
            return JSONObject(String(bytes, Charsets.UTF_8))
        } finally { socket.soTimeout = idleTimeout }
    }
    private fun write(output: DataOutputStream, message: JSONObject) {
        val bytes = message.toString().toByteArray(Charsets.UTF_8)
        require(bytes.size <= 32 * 1024 * 1024) { "Frame too large" }
        output.writeInt(bytes.size)
        output.write(bytes)
        output.flush()
    }
}
