package dev.linktolinux.companion

import java.io.IOException
import java.util.concurrent.Callable
import java.util.concurrent.SynchronousQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

internal object ProviderTasks {
    // A provider can ignore cancellation. Bound stranded Binder calls without blocking transfer workers.
    private val executor = ThreadPoolExecutor(2, 2, 30, TimeUnit.SECONDS, SynchronousQueue())
    fun <T : Any> call(timeoutMs: Long = 30000, cancelled: () -> Unit, isCancelled: () -> Boolean = { false }, discard: (T) -> Unit = { Unit }, block: () -> T): T {
        val lock = Any()
        var abandoned = false
        var completed: T? = null
        val future = executor.submit(Callable {
            val value = block()
            val expired = synchronized(lock) { if (abandoned) true else { completed = value; false } }
            if (expired) { discard(value); throw IOException("File provider request expired") }
            value
        })
        return try {
            val deadline = System.nanoTime() + timeoutMs * 1000000
            var value: T? = null
            while (value == null) {
                if (isCancelled()) throw IOException("File provider request cancelled")
                val remaining = deadline - System.nanoTime()
                if (remaining <= 0) throw java.util.concurrent.TimeoutException("File provider timed out")
                try { value = future.get((remaining / 1000000).coerceIn(1, 250), TimeUnit.MILLISECONDS) }
                catch (error: java.util.concurrent.TimeoutException) {
                    if (System.nanoTime() >= deadline) throw error
                }
            }
            synchronized(lock) { completed = null }
            value
        } catch (error: Exception) {
            val stale = synchronized(lock) { abandoned = true; completed.also { completed = null } }
            future.cancel(true)
            stale?.let(discard)
            cancelled()
            throw error
        }
    }
}
