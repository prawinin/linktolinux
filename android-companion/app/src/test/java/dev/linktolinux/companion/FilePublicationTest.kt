package dev.linktolinux.companion

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

class FilePublicationTest {
    @Test fun cancellationBeforeCommitPreventsPublication() {
        val publication = FilePublication()
        assertTrue(publication.cancel())
        assertFalse(publication.begin())
    }
    @Test fun publicationWinsCancellationOnceCommitStarts() {
        val publication = FilePublication()
        assertTrue(publication.begin())
        assertFalse(publication.cancel())
        assertTrue(publication.committing)
    }
    @Test fun concurrentCancellationAndPublicationHaveOneWinner() {
        val executor = Executors.newFixedThreadPool(2)
        try {
            repeat(100) {
                val start = CountDownLatch(1)
                val publication = FilePublication()
                val commit = executor.submit<Boolean> { start.await(); publication.begin() }
                val cancel = executor.submit<Boolean> { start.await(); publication.cancel() }
                start.countDown()
                assertTrue(commit.get(1, TimeUnit.SECONDS) xor cancel.get(1, TimeUnit.SECONDS))
            }
        } finally { executor.shutdownNow() }
    }
}
