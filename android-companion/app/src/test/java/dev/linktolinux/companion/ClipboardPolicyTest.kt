package dev.linktolinux.companion

import org.junit.Assert.*
import org.junit.Test

class ClipboardPolicyTest {
    @Test fun onlyReadsPhoneClipboardWhenFocusedAndConnected() {
        val policy = ClipboardPolicy(enabled = true, connected = true, focused = true)
        assertTrue(policy.canAutoSend)
        assertFalse(policy.copy(focused = false).canAutoSend)
        assertFalse(policy.copy(connected = false).canAutoSend)
        assertFalse(policy.copy(enabled = false).canAutoSend)
        assertTrue(policy.copy(enabled = false).canManualSend)
    }
    @Test fun backgroundDeliveryCanRemainEnabledWithoutReadingPhoneClipboard() {
        val background = ClipboardPolicy(enabled = true, connected = true, focused = false)
        assertTrue(background.canReceive(automatic = true))
        assertFalse(background.canManualSend)
        assertFalse(background.copy(enabled = false).canReceive(automatic = true))
        assertTrue(background.copy(enabled = false).canReceive(automatic = false))
        assertFalse(background.copy(connected = false).canReceive(automatic = false))
    }
    @Test fun preventsEchoesWithoutPreventingExplicitSend() {
        val tracker = ClipboardTracker()
        assertTrue(tracker.shouldSend("first", force = false))
        tracker.remember("first")
        assertFalse(tracker.shouldSend("first", force = false))
        assertTrue(tracker.shouldSend("first", force = true))
        assertTrue(tracker.shouldSend("second", force = false))
        tracker.clear()
        assertTrue(tracker.shouldSend("first", force = false))
    }
}
