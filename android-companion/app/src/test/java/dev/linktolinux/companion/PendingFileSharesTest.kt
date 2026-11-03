package dev.linktolinux.companion

import org.junit.Assert.*
import org.junit.Test

class PendingFileSharesTest {
    @Test fun sharesAreConsumedOnceAndCanSurviveRecreation() {
        val shares = PendingFileShares()
        shares.add(listOf("content://documents/one", "content://documents/one"))
        assertEquals(1, shares.snapshot().size)
        val recreated = PendingFileShares()
        recreated.add(shares.snapshot())
        assertEquals(listOf("content://documents/one"), recreated.take())
        assertTrue(recreated.take().isEmpty())
    }
    @Test fun rejectsUnsafeSchemesAndOversizedBatchesWithoutDroppingExistingShares() {
        val shares = PendingFileShares()
        shares.add(listOf("content://documents/kept"))
        for (invalid in listOf(listOf("file:///private/file"), listOf("https://example.test/file"), List(17) { "content://documents/$it" })) {
            try { shares.add(invalid); fail("Unsafe selection accepted") } catch (expected: IllegalArgumentException) { assertEquals(1, shares.snapshot().size) }
        }
        shares.clear()
        assertTrue(shares.snapshot().isEmpty())
    }
}
