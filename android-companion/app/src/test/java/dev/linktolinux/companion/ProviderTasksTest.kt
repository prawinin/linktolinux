package dev.linktolinux.companion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException
import java.util.concurrent.atomic.AtomicInteger

class ProviderTasksTest {
    @Test(timeout = 3000) fun ignoredCancellationIsBoundedAndLateResourcesAreDiscarded() {
        val release = CountDownLatch(1)
        val discarded = CountDownLatch(2)
        val cancelled = AtomicInteger()
        try {
            repeat(2) {
                assertThrows(TimeoutException::class.java) {
                    ProviderTasks.call(timeoutMs = 40, cancelled = { cancelled.incrementAndGet() }, discard = { _: String -> discarded.countDown() }) {
                        while (release.count > 0) {
                            try { release.await() }
                            catch (error: InterruptedException) { Thread.interrupted() }
                        }
                        "late descriptor"
                    }
                }
            }
            assertThrows(RejectedExecutionException::class.java) { ProviderTasks.call(cancelled = { cancelled.incrementAndGet() }) { "third request" } }
            assertEquals(2, cancelled.get())
        } finally { release.countDown() }
        org.junit.Assert.assertTrue(discarded.await(1, TimeUnit.SECONDS))
    }
}
