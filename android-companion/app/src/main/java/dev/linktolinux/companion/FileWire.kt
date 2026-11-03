package dev.linktolinux.companion

import java.io.Closeable
import java.io.EOFException
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.nio.ByteBuffer
import java.nio.channels.SelectionKey
import java.nio.channels.Selector
import java.nio.channels.SocketChannel
import java.security.MessageDigest
import org.json.JSONObject

object FileTransferPolicy {
    const val MAX_SIZE = 8589934592L
    fun name(value: String): String {
        require(value.isNotBlank() && value != "." && value != ".." && value.toByteArray(Charsets.UTF_8).size <= 240 && value.none { it == '/' || it == '\\' || it.code < 32 || it.code == 127 }) { "Invalid file name" }
        return value
    }
    fun id(value: String): String { require(value.matches(Regex("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"))) { "Invalid transfer ID" }; return value }
    fun token(value: String): String { require(value.matches(Regex("[0-9a-f]{32}"))) { "Invalid transfer token" }; return value }
    fun mime(value: String): String = if (value.length <= 127 && value.matches(Regex("[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+"))) value else "application/octet-stream"
    fun size(message: JSONObject): Long? {
        if (message.isNull("size")) return null
        val raw = message.get("size")
        require(raw is Long || raw is Int) { "Invalid file size" }
        return (raw as Number).toLong().also { require(it in 0..MAX_SIZE) { "File exceeds 8 GiB" } }
    }
}

class TimedFileSocket private constructor(private val channel: SocketChannel, private val selector: Selector, var timeoutMs: Long) : Closeable {
    companion object {
        fun connect(host: String, port: Int, timeoutMs: Long = 5000): TimedFileSocket {
            val channel = SocketChannel.open()
            val selector = Selector.open()
            val wire = TimedFileSocket(channel, selector, timeoutMs)
            try {
                channel.configureBlocking(false)
                channel.register(selector, SelectionKey.OP_CONNECT)
                if (!channel.connect(InetSocketAddress(host, port))) {
                    val deadline = System.nanoTime() + timeoutMs * 1000000
                    while (!channel.finishConnect()) wire.ready(SelectionKey.OP_CONNECT, deadline)
                }
                channel.socket().tcpNoDelay = true
                return wire
            } catch (error: Exception) { wire.close(); throw error }
        }
        fun accept(channel: SocketChannel, timeoutMs: Long): TimedFileSocket {
            channel.configureBlocking(false)
            val selector = Selector.open()
            channel.register(selector, SelectionKey.OP_READ)
            return TimedFileSocket(channel, selector, timeoutMs)
        }
    }
    private fun ready(operation: Int, deadline: Long) {
        if (!channel.isOpen || !selector.isOpen) throw IOException("Transfer cancelled")
        val remaining = deadline - System.nanoTime()
        if (remaining <= 0) throw SocketTimeoutException("File transfer timed out")
        channel.keyFor(selector).interestOps(operation)
        selector.select((remaining / 1000000).coerceAtLeast(1))
        selector.selectedKeys().clear()
    }
    private fun transfer(buffer: ByteBuffer, writing: Boolean) {
        val deadline = System.nanoTime() + timeoutMs * 1000000
        while (buffer.hasRemaining()) {
            if (!channel.isOpen) throw IOException("Transfer cancelled")
            val count = if (writing) channel.write(buffer) else channel.read(buffer)
            if (count < 0) throw EOFException("File stream ended early")
            if (count == 0) ready(if (writing) SelectionKey.OP_WRITE else SelectionKey.OP_READ, deadline)
            if (System.nanoTime() > deadline && buffer.hasRemaining()) throw SocketTimeoutException("File transfer timed out")
        }
    }
    fun bytes(bytes: ByteArray, length: Int = bytes.size) { transfer(ByteBuffer.wrap(bytes, 0, length), true) }
    fun read(length: Int): ByteArray = ByteArray(length).also { transfer(ByteBuffer.wrap(it), false) }
    fun int(value: Int) { bytes(ByteBuffer.allocate(4).putInt(value).array()) }
    fun int(): Int = ByteBuffer.wrap(read(4)).int
    fun json(value: JSONObject) { val bytes = value.toString().toByteArray(Charsets.UTF_8); require(bytes.size in 1..4096); int(bytes.size); bytes(bytes) }
    fun json(): JSONObject { val size = int(); require(size in 1..4096) { "Invalid data frame" }; return JSONObject(String(read(size), Charsets.UTF_8)) }
    override fun close() { try { channel.close() } finally { selector.wakeup(); selector.close() } }
}

data class FileReceipt(val size: Long, val sha256: String)
object FileWire {
    private fun hash(digest: MessageDigest): String = digest.digest().joinToString("") { "%02x".format(it.toInt() and 255) }
    fun send(wire: TimedFileSocket, input: InputStream, expected: Long?, progress: (Long) -> Unit): FileReceipt {
        val digest = MessageDigest.getInstance("SHA-256"); val buffer = ByteArray(65536); var total = 0L
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            if (count == 0) continue
            total += count; require(total <= FileTransferPolicy.MAX_SIZE && (expected == null || total <= expected)) { "File size changed or exceeds limit" }
            wire.int(count); wire.bytes(buffer, count); digest.update(buffer, 0, count); progress(total)
        }
        require(expected == null || total == expected) { "File size changed" }
        val receipt = FileReceipt(total, hash(digest)); wire.int(0)
        wire.json(JSONObject().put("type", "file_end").put("size", total).put("sha256", receipt.sha256))
        val ack = wire.json(); require(ack.optString("type") == "file_complete" && FileTransferPolicy.size(ack) == total && ack.optString("sha256") == receipt.sha256) { "File acknowledgement failed" }
        return receipt
    }
    fun receive(wire: TimedFileSocket, output: OutputStream, expected: Long?, progress: (Long) -> Unit): FileReceipt {
        val digest = MessageDigest.getInstance("SHA-256"); var total = 0L
        while (true) {
            val count = wire.int(); require(count in 0..65536) { "Invalid file chunk" }; if (count == 0) break
            total += count; require(total <= FileTransferPolicy.MAX_SIZE && (expected == null || total <= expected)) { "File exceeds expected size" }
            val bytes = wire.read(count); output.write(bytes); digest.update(bytes); progress(total)
        }
        val receipt = FileReceipt(total, hash(digest)); val end = wire.json()
        require(end.optString("type") == "file_end" && FileTransferPolicy.size(end) == total && end.optString("sha256") == receipt.sha256 && (expected == null || expected == total)) { "File integrity check failed" }
        output.flush(); return receipt
    }
}
