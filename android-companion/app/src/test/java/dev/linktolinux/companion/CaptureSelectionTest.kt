package dev.linktolinux.companion

import org.junit.Assert.*
import org.junit.Test

class CaptureSelectionTest {
    @Test fun audioFallbackRequiresDesktopSupport() {
        assertEquals(listOf(48000), CaptureSelection.audioRates(false))
        assertEquals(listOf(48000, 44100, 32000, 16000), CaptureSelection.audioRates(true))
    }
    @Test fun retriesWorkingConfigurationsInOrder() {
        val attempted = mutableListOf<Int>()
        val result = CaptureSelection.firstWorking(listOf(48000, 44100), { rate ->
            attempted.add(rate)
            if (rate == 48000) throw IllegalStateException("Unsupported")
            rate
        }, {})
        assertEquals(44100, result)
        assertEquals(listOf(48000, 44100), attempted)
    }
    @Test fun cameraCandidatesIncludeLowerAndPortraitSizes() {
        val selected = CameraOption("rear", false, setOf(CameraSize(1280, 720), CameraSize(640, 480), CameraSize(480, 640)))
        val result = CaptureSelection.sizes(selected, listOf(selected))
        assertEquals(CameraSize(1280, 720), result.first())
        assertTrue(CameraSize(480, 640) in result)
        assertEquals(3, result.size)
    }
    @Test fun exhaustedConfigurationsFailWithCause() {
        try {
            CaptureSelection.firstWorking(listOf(1, 2), { throw IllegalStateException("Rejected $it") }, {})
            fail("Expected configuration failure")
        } catch (error: IllegalStateException) {
            assertEquals("Rejected 2", error.cause?.message)
        }
    }
}
