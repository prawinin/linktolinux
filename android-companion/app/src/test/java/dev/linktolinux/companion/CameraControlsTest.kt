package dev.linktolinux.companion

import org.junit.Assert.*
import org.junit.Test

class CameraControlsTest {
    @Test fun rotationCyclesAndRejectsInvalidStoredValues() {
        assertEquals(listOf(90, 180, 270, 0), listOf(0, 90, 180, 270).map { CameraSelection.nextRotation(it) })
        assertEquals(0, CameraSelection.validRotation(45))
        assertEquals(270, CameraSelection.validRotation(270))
    }
    @Test fun choosesPreferredLensAndFallsBackToAvailableCamera() {
        val cameras = listOf(CameraOption("rear", false, setOf(CameraSize(640, 480))), CameraOption("front", true, setOf(CameraSize(640, 480))))
        assertEquals("front", CameraSelection.select(cameras, true).id)
        assertEquals("rear", CameraSelection.select(cameras.take(1), true).id)
    }
    @Test fun sharesEncoderSizeAcrossLensesWhenPossible() {
        val rear = CameraOption("rear", false, setOf(CameraSize(1280, 720), CameraSize(640, 480)))
        val front = CameraOption("front", true, setOf(CameraSize(640, 480), CameraSize(1920, 1080)))
        assertEquals(CameraSize(640, 480), CameraSelection.size(rear, listOf(rear, front)))
        assertEquals(CameraSize(1280, 720), CameraSelection.size(rear, listOf(rear)))
    }
}
