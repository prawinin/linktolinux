package dev.linktolinux.companion

import android.os.ParcelFileDescriptor
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import java.io.InputStream
import java.io.IOException
import java.net.SocketTimeoutException

internal class DocumentSource(private val descriptor: ParcelFileDescriptor, private val cancelled: () -> Boolean) : InputStream() {
    @Volatile private var closed = false
    init {
        try {
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                val flags = Os.fcntlInt(descriptor.fileDescriptor, OsConstants.F_GETFL, 0)
                Os.fcntlInt(descriptor.fileDescriptor, OsConstants.F_SETFL, flags or OsConstants.O_NONBLOCK)
            }
        } catch (error: Exception) { descriptor.close(); throw error }
    }
    override fun read(): Int { val byte = ByteArray(1); return if (read(byte, 0, 1) < 0) -1 else byte[0].toInt() and 255 }
    override fun read(bytes: ByteArray, offset: Int, length: Int): Int {
        require(offset >= 0 && length >= 0 && offset <= bytes.size - length)
        if (length == 0) return 0
        return if (android.os.Build.VERSION.SDK_INT >= 30) readPolled(bytes, offset, length)
        else ProviderTasks.call(cancelled = { close() }, isCancelled = cancelled) { readPolled(bytes, offset, length) }
    }
    private fun readPolled(bytes: ByteArray, offset: Int, length: Int): Int {
        val deadline = System.nanoTime() + 30000000000L
        while (true) {
            if (closed || cancelled()) throw IOException("File read cancelled")
            val remaining = deadline - System.nanoTime()
            if (remaining <= 0) throw SocketTimeoutException("File provider read timed out")
            val poll = StructPollfd().apply { fd = descriptor.fileDescriptor; events = OsConstants.POLLIN.toShort() }
            try {
                if (Os.poll(arrayOf(poll), (remaining / 1000000).coerceIn(1, 250).toInt()) == 0) continue
                val count = Os.read(descriptor.fileDescriptor, bytes, offset, length)
                return if (count == 0) -1 else count
            } catch (error: ErrnoException) {
                if (error.errno != OsConstants.EAGAIN && error.errno != OsConstants.EINTR) throw IOException("Cannot read document", error)
            }
        }
    }
    override fun close() { if (!closed) { closed = true; descriptor.close() } }
}
